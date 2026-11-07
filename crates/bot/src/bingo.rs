use crate::App;
use crate::format::hex_color;
use crate::mirror::{KEEP_SECS, avatar_url};
use anyhow::Result;
use dashmap::DashSet;
use redis::AsyncCommands;
use serde_json::Value;
use serenity::all::{
    AutoArchiveDuration, Channel, ChannelId, ChannelType, CreateEmbed, CreateEmbedAuthor, CreateEmbedFooter,
    CreateMessage, CreateThread, Timestamp, UserId,
};

#[derive(Default)]
pub struct Threads {
    creating: tokio::sync::Mutex<()>,
    added: DashSet<(u64, u64)>,
}

fn thread_name(game: i64, team: Option<i64>, team_name: &str) -> String {
    match team {
        None => format!("bingo #{game}"),
        Some(_) => {
            let full: Vec<u16> = format!("bingo #{game} \u{b7} {team_name}").encode_utf16().collect();
            String::from_utf16_lossy(&full[..full.len().min(100)])
        }
    }
}

async fn thread_for(
    app: &App,
    parent: u64,
    game: i64,
    team: Option<i64>,
    team_name: &str,
) -> Result<Option<ChannelId>> {
    let Some(http) = app.discord() else { return Ok(None) };
    let key = format!("{game}:{}", team.map_or("all".to_owned(), |t| t.to_string()));
    let _guard = app.bingo.creating.lock().await;
    let mut r = app.redis().await?;
    let known: Option<String> = r.get(format!("bingo_chat:dthread:{key}")).await?;
    if let Some(id) = known.and_then(|k| k.parse::<u64>().ok())
        && let Ok(Channel::Guild(c)) = http.get_channel(ChannelId::new(id)).await
        && c.thread_metadata.is_some()
    {
        return Ok(Some(c.id));
    }
    let builder = match team {
        None => CreateThread::new(thread_name(game, team, team_name))
            .kind(ChannelType::PublicThread)
            .auto_archive_duration(AutoArchiveDuration::OneDay),
        Some(_) => CreateThread::new(thread_name(game, team, team_name))
            .kind(ChannelType::PrivateThread)
            .invitable(false)
            .auto_archive_duration(AutoArchiveDuration::OneDay),
    };
    let thread = ChannelId::new(parent).create_thread(http.as_ref(), builder).await?;
    let _: () = r
        .set_ex(format!("bingo_chat:dthread:{key}"), thread.id.to_string(), KEEP_SECS)
        .await?;
    let _: () = r
        .set_ex(format!("bingo_chat:dthread_of:{}", thread.id), &key, KEEP_SECS)
        .await?;
    Ok(Some(thread.id))
}

pub async fn send(app: &App, d: &Value) -> Result<()> {
    let game = d["gameId"].as_i64().unwrap_or(0);
    let team = d["teamId"].as_i64();
    let team_name = d["teamName"].as_str().unwrap_or("");
    let m = &d["message"];
    let text = match &m["text"] {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    };
    let Some(parent) = app.cfg.bingo_chat_channel else {
        let place = match team {
            None => format!("bingo #{game}"),
            Some(_) => format!("bingo #{game} \u{b7} {team_name}"),
        };
        let author = m["authorUsername"].as_str().unwrap_or("undefined");
        return crate::logs::custom(app, &format!("`[{place}]` **{author}**: {text}")).await;
    };
    let Some(thread) = thread_for(app, parent, game, team, team_name).await? else {
        return Ok(());
    };
    let username = m["authorUsername"].as_str().filter(|u| !u.is_empty());
    let discord_id = m["authorDiscordId"].as_str().filter(|u| !u.is_empty());
    let color = m["authorColor"]
        .as_str()
        .and_then(hex_color)
        .filter(|c| *c != 0)
        .unwrap_or(0x5865f2);
    let mut author = CreateEmbedAuthor::new(username.unwrap_or("unknown"))
        .icon_url(avatar_url(discord_id, m["authorAvatar"].as_str()));
    if let Some(u) = username {
        author = author.url(format!("https://counting.gg/counter/{u}"));
    }
    let at = m["timestamp"]
        .as_i64()
        .and_then(|ms| Timestamp::from_millis(ms).ok())
        .unwrap_or_else(Timestamp::now);
    let mut embed = CreateEmbed::new()
        .color(color)
        .author(author)
        .description(text)
        .timestamp(at);
    if team.is_none()
        && let Some(t) = m["authorTeamName"].as_str().filter(|t| !t.is_empty())
    {
        embed = embed.footer(CreateEmbedFooter::new(t));
    }
    if team.is_some()
        && let (Some(id), Some(http)) = (discord_id.and_then(|d| d.parse::<u64>().ok()), app.discord())
        && app.bingo.added.insert((thread.get(), id))
    {
        let _ = thread.add_thread_member(http.as_ref(), UserId::new(id)).await;
    }
    app.send(Some(thread.get()), CreateMessage::new().embed(embed)).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn team_thread_names_fit_discord() {
        assert_eq!(thread_name(4, None, "x"), "bingo #4");
        assert_eq!(thread_name(4, Some(1), "Reds"), "bingo #4 \u{b7} Reds");
        assert_eq!(thread_name(4, Some(1), &"a".repeat(200)).encode_utf16().count(), 100);
    }
}
