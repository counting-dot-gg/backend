use crate::{App, bingo, logs, mirror, roles};
use serde_json::{Value, json};
use std::sync::Arc;

pub async fn handle(app: Arc<App>, cmd: String, d: Value) -> Option<Value> {
    let s = |k: &str| d[k].as_str().unwrap_or("").to_owned();
    let result = match cmd.as_str() {
        "logClientError" => logs::client_error(&app, &d).await,
        "logMute" => logs::restriction(&app, &d["muter"], &d["counterToMute"], logs::Restriction::Mute).await,
        "logBan" => logs::restriction(&app, &d["banner"], &d["counterToBan"], logs::Restriction::Ban).await,
        "addDiscordRole" => roles::add_named(&app, &s("discordId"), &s("role")).await,
        "removeDiscordRole" => roles::remove_named(&app, &s("discordId"), &s("role")).await,
        "syncLevelRoles" => roles::sync_level(&app, &s("discordId"), d["level"].as_i64().unwrap_or(0)).await,
        "syncRankRoles" => roles::sync_rank(&app, &s("discordId"), d["rank"].as_str()).await,
        "syncAllRoles" => roles::sync_all(&app).await,
        "logDelete" => logs::delete(&app, &d["deleter"], &d["counter"], &d["post"]).await,
        "logCustomMessage" => logs::custom(&app, &s("message")).await,
        "logCustomMessageWithAttachments" => {
            logs::custom_with_attachments(&app, &s("message"), &d["attachments"]).await
        }
        "logLock" => logs::lock(&app, &d["locker"], &d["thread"], false).await,
        "logSilentLock" => logs::lock(&app, &d["locker"], &d["thread"], true).await,
        "newUserLog" => logs::new_user(&app, &s("newCounterUUID")).await,
        "sendWelcome" => logs::welcome(&app, &d["newCounter"]).await,
        "kpartAlert" => logs::kpart(&app, &d).await,
        "dailyStatsReport" => logs::daily_stats(&app, &d).await,
        "sendMessage" => logs::plain(&app, &s("channel"), &s("message")).await,
        "sendBingoTeamChat" => bingo::send(&app, &d).await,
        "sendChatMirror" => {
            mirror::queue(&app, &d);
            Ok(())
        }
        "chatMirrorDeletePost" => mirror::delete_post(&app, &s("postUUID")).await,
        "storeChatMirrorInbound" => mirror::store_inbound(&app, &s("discordMessageId"), &s("postUUID")).await,
        other => {
            tracing::warn!("unknown bot command {other}");
            return None;
        }
    };
    if let Err(e) = result {
        tracing::warn!("bot command {cmd} failed: {e:#}");
    }
    Some(json!(true))
}
