use crate::App;
use crate::format::{hex_color, reply_speed};
use anyhow::Result;
use redis::AsyncCommands;
use serde_json::{Value, json};
use serenity::all::{
    ChannelId, CreateEmbed, CreateEmbedAuthor, CreateEmbedFooter, CreateMessage, EditMessage, MessageId, Timestamp,
};
use std::sync::Arc;
use std::time::Duration;

const BATCH: usize = 10;
const FLUSH_AFTER: Duration = Duration::from_secs(2);
pub const KEEP_SECS: u64 = 7 * 24 * 60 * 60;

struct Entry {
    at: f64,
    embed: CreateEmbed,
    post_uuid: String,
}

#[derive(Default)]
pub struct Buffer {
    queue: std::sync::Mutex<Vec<Entry>>,
    timer: std::sync::atomic::AtomicBool,
    sender: std::sync::OnceLock<tokio::sync::mpsc::UnboundedSender<Vec<Entry>>>,
}

fn take_batch(q: &mut Vec<Entry>, partial: bool) -> Option<Vec<Entry>> {
    if q.is_empty() || (!partial && q.len() < BATCH) {
        return None;
    }
    q.sort_by(|a, b| a.at.total_cmp(&b.at));
    let n = q.len().min(BATCH);
    Some(q.drain(..n).collect())
}

pub fn avatar_url(discord_id: Option<&str>, avatar: Option<&str>) -> String {
    match (discord_id, avatar) {
        (Some(d), Some(a)) if !d.is_empty() && a.chars().count() > 5 => {
            format!("https://cdn.discordapp.com/avatars/{d}/{a}")
        }
        _ => "https://cdn.discordapp.com/embed/avatars/0.png".to_owned(),
    }
}

fn str_of(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn mirror_embed(d: &Value) -> CreateEmbed {
    let color = str_of(&d["color"])
        .filter(|c| !c.is_empty())
        .map_or(Some(0x5865f2), |c| hex_color(&c));
    let username = str_of(&d["username"]).unwrap_or_default();
    let mut author = CreateEmbedAuthor::new(username).icon_url(avatar_url(
        str_of(&d["discordId"]).as_deref(),
        str_of(&d["avatar"]).as_deref(),
    ));
    if let Some(u) = str_of(&d["counterUsername"]).filter(|u| !u.is_empty()) {
        author = author.url(format!("https://counting.gg/counter/{u}"));
    }
    let at = str_of(&d["timestamp"])
        .and_then(|t| t.split('.').next().and_then(|i| i.parse::<i64>().ok()))
        .and_then(|ms| Timestamp::from_millis(ms).ok())
        .unwrap_or_else(Timestamp::now);
    let mut embed = CreateEmbed::new()
        .author(author)
        .description(str_of(&d["text"]).unwrap_or_default())
        .timestamp(at);
    if let Some(c) = color {
        embed = embed.color(c);
    }
    if let Some(since) = str_of(&d["timeSinceLastPost"])
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|s| *s >= 0.0)
    {
        embed = embed.footer(CreateEmbedFooter::new(reply_speed(since)));
    }
    embed
}

pub fn queue(app: &Arc<App>, d: &Value) {
    if app.cfg.chat_mirror_channel.is_none() {
        return;
    }
    let (full, waiting) = {
        let mut q = app.mirror.queue.lock().unwrap();
        q.push(Entry {
            at: str_of(&d["timestamp"]).and_then(|t| t.parse().ok()).unwrap_or(f64::MAX),
            embed: mirror_embed(d),
            post_uuid: str_of(&d["postUUID"]).unwrap_or_default(),
        });
        let full = take_batch(&mut q, false);
        (full, !q.is_empty())
    };
    if let Some(batch) = full {
        dispatch(app, batch);
    }
    if waiting {
        schedule(app);
    }
}

fn dispatch(app: &Arc<App>, batch: Vec<Entry>) {
    let tx = app.mirror.sender.get_or_init(|| {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Vec<Entry>>();
        let app = app.clone();
        tokio::spawn(async move {
            while let Some(batch) = rx.recv().await {
                if let Err(e) = send_batch(&app, batch).await {
                    tracing::warn!("flushChatMirror error: {e:#}");
                }
            }
        });
        tx
    });
    let _ = tx.send(batch);
}

fn schedule(app: &Arc<App>) {
    if app.mirror.timer.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    tokio::spawn(async move {
        tokio::time::sleep(FLUSH_AFTER).await;
        app.mirror.timer.store(false, std::sync::atomic::Ordering::SeqCst);
        let (batch, waiting) = {
            let mut q = app.mirror.queue.lock().unwrap();
            let batch = take_batch(&mut q, true);
            (batch, !q.is_empty())
        };
        if let Some(batch) = batch {
            dispatch(&app, batch);
        }
        if waiting {
            schedule(&app);
        }
    });
}

async fn send_batch(app: &App, batch: Vec<Entry>) -> Result<()> {
    let (embeds, uuids): (Vec<CreateEmbed>, Vec<String>) = batch.into_iter().map(|e| (e.embed, e.post_uuid)).unzip();
    let Some(msg) = app
        .send(app.cfg.chat_mirror_channel, CreateMessage::new().embeds(embeds))
        .await?
    else {
        return Ok(());
    };
    let uuids: Vec<String> = uuids.into_iter().filter(|u| !u.is_empty()).collect();
    if uuids.is_empty() {
        return Ok(());
    }
    let mut r = app.redis().await?;
    let _: () = r
        .set_ex(
            format!("chat_mirror:msg:{}", msg.id),
            json!(uuids).to_string(),
            KEEP_SECS,
        )
        .await?;
    for (i, uuid) in uuids.iter().enumerate() {
        let _: () = r
            .set_ex(format!("chat_mirror:post:{uuid}"), format!("{}:{i}", msg.id), KEEP_SECS)
            .await?;
    }
    Ok(())
}

pub async fn store_inbound(app: &App, discord_message_id: &str, post_uuid: &str) -> Result<()> {
    let mut r = app.redis().await?;
    let _: () = r
        .set_ex(
            format!("chat_mirror:discord_in:{discord_message_id}"),
            post_uuid,
            KEEP_SECS,
        )
        .await?;
    let _: () = r
        .set_ex(
            format!("chat_mirror:post_to_discord:{post_uuid}"),
            discord_message_id,
            KEEP_SECS,
        )
        .await?;
    Ok(())
}

pub async fn delete_post(app: &App, post_uuid: &str) -> Result<()> {
    let (Some(channel), Some(http)) = (app.cfg.chat_mirror_channel, app.discord()) else {
        return Ok(());
    };
    if app.deleting_from_discord.contains(post_uuid) {
        return Ok(());
    }
    let channel = ChannelId::new(channel);
    let mut r = app.redis().await?;
    let outbound: Option<String> = r.get(format!("chat_mirror:post:{post_uuid}")).await?;
    if let Some(v) = outbound {
        if let Some((msg, idx)) = v.split_once(':')
            && let (Ok(msg), Ok(idx)) = (msg.parse::<u64>(), idx.parse::<usize>())
            && let Ok(found) = channel.message(http.as_ref(), MessageId::new(msg)).await
        {
            let embeds: Vec<CreateEmbed> = found
                .embeds
                .into_iter()
                .enumerate()
                .map(|(i, e)| {
                    let c = CreateEmbed::from(e);
                    if i == idx { c.description("[deleted]") } else { c }
                })
                .collect();
            let _ = channel
                .edit_message(http.as_ref(), MessageId::new(msg), EditMessage::new().embeds(embeds))
                .await;
        }
        let _: () = r.del(format!("chat_mirror:post:{post_uuid}")).await?;
    }
    let inbound: Option<String> = r.get(format!("chat_mirror:post_to_discord:{post_uuid}")).await?;
    if let Some(id) = inbound {
        if let Ok(n) = id.parse::<u64>() {
            let _ = channel.delete_message(http.as_ref(), MessageId::new(n)).await;
        }
        let _: () = r
            .del(&[
                format!("chat_mirror:discord_in:{id}"),
                format!("chat_mirror:post_to_discord:{post_uuid}"),
            ])
            .await?;
    }
    Ok(())
}

pub async fn deleted_in_discord(app: &Arc<App>, message_id: u64) -> Result<()> {
    let mut r = app.redis().await?;
    let outbound: Option<String> = r.get(format!("chat_mirror:msg:{message_id}")).await?;
    let inbound: Option<String> = r.get(format!("chat_mirror:discord_in:{message_id}")).await?;
    let uuids: Vec<String> = match (outbound, inbound) {
        (Some(o), _) => serde_json::from_str(&o).unwrap_or_default(),
        (None, Some(i)) => vec![i],
        _ => Vec::new(),
    };
    if uuids.is_empty() {
        return Ok(());
    }
    for uuid in &uuids {
        app.deleting_from_discord.insert(uuid.clone());
        app.socket.emit("deleteCommentAsSystem", json!({ "uuid": uuid }));
    }
    let a = app.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(30)).await;
        for uuid in uuids {
            a.deleting_from_discord.remove(&uuid);
        }
    });
    let _: () = r
        .del(&[
            format!("chat_mirror:msg:{message_id}"),
            format!("chat_mirror:discord_in:{message_id}"),
        ])
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(at: f64) -> Entry {
        Entry {
            at,
            embed: CreateEmbed::new(),
            post_uuid: String::new(),
        }
    }

    #[test]
    fn full_batches_leave_at_once_and_in_post_order() {
        let mut q: Vec<Entry> = (0..9).rev().map(|i| entry(i as f64)).collect();
        assert!(take_batch(&mut q, false).is_none());
        q.push(entry(100.0));
        let batch = take_batch(&mut q, false).unwrap();
        assert_eq!(batch.len(), 10);
        assert_eq!(batch[0].at, 0.0);
        assert!(q.is_empty());
    }

    #[test]
    fn the_timer_sends_a_partial_batch() {
        let mut q = vec![entry(1.0), entry(2.0)];
        assert_eq!(take_batch(&mut q, true).unwrap().len(), 2);
        assert!(take_batch(&mut q, true).is_none());
    }
}
