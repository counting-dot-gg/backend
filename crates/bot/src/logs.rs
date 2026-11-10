use crate::App;
use crate::format::{clamp, fancy_time, thousands, utf16_chunks};
use anyhow::Result;
use base64::Engine;
use serde_json::Value;
use serenity::all::{CreateAttachment, CreateEmbed, CreateEmbedFooter, CreateMessage, Timestamp};
use sqlx::Row;

const BLUE: u32 = 0x0099ff;

fn st<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str().unwrap_or("")
}

fn js(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "undefined".to_owned(),
        other => other.to_string(),
    }
}

fn has_role(counter: &Value, role: &str) -> bool {
    counter["roles"]
        .as_array()
        .is_some_and(|r| r.iter().any(|x| x.as_str() == Some(role)))
}

fn time_uuid() -> String {
    cgg_web::ids::uuid_for(cgg_web::ids::Clock::default().now_ns())
}

fn blue(title: &str) -> CreateEmbed {
    CreateEmbed::new().color(BLUE).title(title).timestamp(Timestamp::now())
}

pub async fn client_error(app: &App, p: &Value) -> Result<()> {
    let f = |k: &str| p[k].as_str();
    let mut embed = CreateEmbed::new()
        .color(0xef4444)
        .title(format!("Client error: {}", clamp(f("scope"), 60)))
        .description(format!("```\n{}\n```", clamp(f("message"), 900)))
        .field("URL", clamp(f("url"), 300), false)
        .field("Who", clamp(f("username"), 80), true)
        .field("Digest", clamp(f("digest"), 80), true)
        .field("Agent", clamp(f("userAgent"), 300), false)
        .timestamp(Timestamp::now());
    if let Some(stack) = f("stack").filter(|s| !s.is_empty()) {
        embed = embed.field("Stack", format!("```\n{}\n```", clamp(Some(stack), 900)), false);
    }
    app.send(app.cfg.log_channel, CreateMessage::new().embed(embed)).await?;
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Restriction {
    Ban,
    Mute,
}

pub async fn restriction(app: &App, actor: &Value, target: &Value, kind: Restriction) -> Result<()> {
    let (title, role, actor_label, id_label, verb) = match kind {
        Restriction::Ban => ("Ban", "banned", "Banner", "banTimeUUID", "ban"),
        Restriction::Mute => ("Mute", "muted", "Muter", "muteTimeUUID", "mute"),
    };
    let on = has_role(target, role);
    let embed = blue(title)
        .description(format!(
            "Toggling {verb} for counter {} ({}): {on}",
            st(target, "username"),
            st(target, "uuid")
        ))
        .field(actor_label, st(actor, "username"), false)
        .field(id_label, time_uuid(), false);
    app.send(app.cfg.mod_log_channel, CreateMessage::new().embed(embed))
        .await?;
    let role_key = match kind {
        Restriction::Ban => "incendiliated_role",
        Restriction::Mute => "muted_role",
    };
    let discord_id = st(target, "discordId");
    if on {
        crate::roles::add_named(app, discord_id, role_key).await
    } else {
        crate::roles::remove_named(app, discord_id, role_key).await
    }
}

async fn thread_name(app: &App, uuid: &str) -> Result<String> {
    let name: Option<String> = sqlx::query_scalar("SELECT name FROM thread WHERE uuid = ? LIMIT 1")
        .bind(uuid)
        .fetch_optional(&app.pool)
        .await?;
    Ok(name.unwrap_or_else(|| "undefined".to_owned()))
}

pub async fn delete(app: &App, deleter: &Value, counter: &Value, post: &Value) -> Result<()> {
    if app.cfg.log_channel.is_none() {
        return Ok(());
    }
    let thread = thread_name(app, st(post, "thread")).await?;
    let comment = st(post, "comment").replace('`', "\\`");
    let chunks = utf16_chunks(&comment, 1990);
    let profile = |c: &Value| format!("{} (https://counting.gg/counter/{})", st(c, "username"), st(c, "uuid"));
    for (j, chunk) in chunks.iter().enumerate() {
        let mut embed = blue("Delete")
            .field("Author", profile(counter), false)
            .field("Deleter", profile(deleter), false)
            .field(
                "Link",
                format!("https://counting.gg/thread/{thread}/{}", st(post, "uuid")),
                false,
            )
            .field(
                "Context",
                format!("https://counting.gg/thread/{thread}?context={}", st(post, "uuid")),
                false,
            );
        if chunks.len() > 1 {
            embed = embed.description(format!("Part {}", j + 1));
        }
        app.send(
            app.cfg.log_channel,
            CreateMessage::new().embed(embed).content(format!("```{chunk}```")),
        )
        .await?;
    }
    Ok(())
}

pub async fn custom(app: &App, message: &str) -> Result<()> {
    for chunk in utf16_chunks(message, 1500) {
        app.send(app.cfg.log_channel, CreateMessage::new().content(chunk))
            .await?;
    }
    Ok(())
}

pub async fn custom_with_attachments(app: &App, message: &str, attachments: &Value) -> Result<()> {
    let files: Vec<CreateAttachment> = attachments
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|a| {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(a["base64"].as_str()?)
                .ok()?;
            Some(CreateAttachment::bytes(bytes, a["name"].as_str().unwrap_or("file")))
        })
        .collect();
    let chunks = utf16_chunks(message, 1500);
    let mut batches = files.chunks(10);
    let first = batches.next().map(<[CreateAttachment]>::to_vec).unwrap_or_default();
    if chunks.is_empty() {
        if !first.is_empty() {
            app.send(app.cfg.log_channel, CreateMessage::new().add_files(first))
                .await?;
        }
    } else {
        let last = chunks.len() - 1;
        let mut first = Some(first);
        for (i, chunk) in chunks.into_iter().enumerate() {
            let mut msg = CreateMessage::new().content(chunk);
            if i == last {
                msg = msg.add_files(first.take().unwrap_or_default());
            }
            app.send(app.cfg.log_channel, msg).await?;
        }
    }
    for batch in batches {
        app.send(app.cfg.log_channel, CreateMessage::new().add_files(batch.to_vec()))
            .await?;
    }
    Ok(())
}

pub async fn lock(app: &App, locker: &Value, thread: &Value, silent: bool) -> Result<()> {
    let how = if silent { "(silent) " } else { "" };
    let embed = blue("Thread Lock")
        .description(format!(
            "Toggling {how}thread lock for thread {} ({}): {}",
            st(thread, "name"),
            st(thread, "uuid"),
            js(&thread["locked"])
        ))
        .field("Locker", st(locker, "name"), false)
        .field("lockTimeUUID", time_uuid(), false);
    app.send(app.cfg.mod_log_channel, CreateMessage::new().embed(embed))
        .await?;
    Ok(())
}

pub async fn new_user(app: &App, uuid: &str) -> Result<()> {
    let Some(row) =
        sqlx::query("SELECT name, username, CAST(roles AS CHAR) AS roles FROM counters WHERE uuid = ? LIMIT 1")
            .bind(uuid)
            .fetch_optional(&app.pool)
            .await?
    else {
        return Ok(());
    };
    let name: String = row.try_get("name").unwrap_or_default();
    let username: String = row.try_get("username").unwrap_or_default();
    let roles: String = row.try_get("roles").unwrap_or_default();
    let is_counter = serde_json::from_str::<Vec<String>>(&roles)
        .unwrap_or_else(|_| roles.split(',').map(str::to_owned).collect())
        .iter()
        .any(|r| r == "counter");
    let embed = blue("New counter")
        .description(format!("New counter alert... {name} / {username} ({uuid})"))
        .field("Needs verifying?", if is_counter { "No" } else { "Yes" }, false);
    app.send(
        app.cfg.log_channel,
        CreateMessage::new().content(app.cfg.admin_mention()).embed(embed),
    )
    .await?;
    Ok(())
}

pub async fn welcome(app: &App, counter: &Value) -> Result<()> {
    let id = st(counter, "discordId");
    let help = app.cfg.help_channel.map_or(String::new(), |c| format!("<#{c}>"));
    let embed = blue("New user just joined :)").description(format!(
        "Welcome, <@{id}> ({})! You have been approved and can count now. Check out our [main thread](https://counting.gg/thread/main) and [all other threads](https://counting.gg/threads). Feel free to ask questions in {help}. Have a great time!",
        st(counter, "name")
    ));
    let role = app.cfg.welcome_role.map_or(String::new(), |r| format!("<@&{r}>, "));
    app.send(
        app.cfg.welcome_channel,
        CreateMessage::new()
            .content(format!("{role}it's time to welcome <@{id}>!"))
            .embed(embed),
    )
    .await?;
    Ok(())
}

pub fn kpart_embeds(host: &str, title: &str, thread_name: &str, replay: &str, splits: &[i64]) -> Vec<CreateEmbed> {
    const MAX_FIELDS: usize = 19;
    let total = splits.len() + 1;
    let pages = total.div_ceil(MAX_FIELDS);
    (0..pages)
        .map(|i| {
            let start = i * MAX_FIELDS;
            let end = (start + MAX_FIELDS).min(total);
            let mut embed = CreateEmbed::new().color(BLUE).title(title);
            if total >= MAX_FIELDS && end == total {
                embed = embed.description(format!("Part {}", i + 1));
            }
            embed = embed
                .field("Thread Link", format!("<{host}/thread/{thread_name}>"), false)
                .field("Replay", format!("<{host}{replay}>"), false);
            let page = &splits[start.min(splits.len())..end.min(splits.len())];
            for (j, split) in page.iter().enumerate() {
                let n = j + start + 1;
                let name = if splits.len() == n {
                    "Thread time".to_owned()
                } else {
                    format!("Split {n}")
                };
                embed = embed.field(name, fancy_time(*split), splits.len() != j + 1);
            }
            embed.timestamp(Timestamp::now())
        })
        .collect()
}

pub async fn kpart(app: &App, d: &Value) -> Result<()> {
    let channel = st(d, "channelID").parse::<u64>().ok();
    let role = st(d, "kPartRole");
    let splits: Vec<i64> = d["splits"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_i64)
        .collect();
    for embed in kpart_embeds(
        &app.cfg.host_url,
        st(d, "title"),
        st(d, "threadName"),
        st(d, "replay"),
        &splits,
    ) {
        app.send(
            channel,
            CreateMessage::new().content(format!("<@&{role}>")).embed(embed),
        )
        .await?;
    }
    Ok(())
}

pub async fn daily_stats(app: &App, d: &Value) -> Result<()> {
    let medals = ["\u{1f947} Gold", "\u{1f948} Silver", "\u{1f949} Bronze"];
    let mut embed = CreateEmbed::new()
        .color(BLUE)
        .title(format!("Daily Stats: {}", st(d, "dateLabel")))
        .description(format!(
            "We counted {} counts today, {} of which were in the main thread.",
            thousands(d["totalAll"].as_i64().unwrap_or(0)),
            thousands(d["totalMain"].as_i64().unwrap_or(0))
        ))
        .footer(CreateEmbedFooter::new("Medalists are sitewide and main."));
    for (i, m) in d["medals"].as_array().into_iter().flatten().take(3).enumerate() {
        let main = match (m["mainName"].as_str(), m["mainCount"].as_i64()) {
            (Some(n), Some(c)) => format!("| {n}: {c}"),
            _ => String::new(),
        };
        embed = embed.field(
            medals[i],
            format!("{}: {} {main}", st(m, "name"), m["count"].as_i64().unwrap_or(0)),
            false,
        );
    }
    app.send(app.cfg.daily_stats_channel, CreateMessage::new().embed(embed))
        .await?;
    Ok(())
}

pub async fn plain(app: &App, channel: &str, message: &str) -> Result<()> {
    app.send(channel.parse().ok(), CreateMessage::new().content(message))
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_k_fits_one_embed() {
        let embeds = kpart_embeds("https://x", "Main just reached 5000", "main", "/replay/main", &[1; 11]);
        assert_eq!(embeds.len(), 1);
    }

    #[test]
    fn long_gets_page_their_splits() {
        let embeds = kpart_embeds("https://x", "t", "letters", "/r", &[1; 27]);
        assert_eq!(embeds.len(), 2);
    }
}
