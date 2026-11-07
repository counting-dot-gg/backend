use crate::app::App;
use crate::features;
use crate::model::ThreadMeta;
use crate::thread::{Msg, PostJob};
use crate::wal::Origin;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Instant;

pub async fn handle(app: Arc<App>, cmd: String, d: Value) -> Option<Value> {
    let s = |k: &str| d[k].as_str().unwrap_or("").to_owned();
    match cmd.as_str() {
        "latestCounts" => Some(features::latest_counts(&app)),
        "defaultPage" => {
            let page = features::default_page_tcp(&app).await;
            if page.is_none() {
                tracing::warn!("No dailyLeaderboard for DefaultPage!");
            }
            page
        }
        "addBotMessage" => {
            let name = d["thread"]["name"].as_str().unwrap_or("");
            if let Some(h) = app.thread(name) {
                let job = PostJob::system(app.system_counter.clone(), s("message"));
                let _ = h.tx.send(Msg::Post(Box::new(job))).await;
            }
            Some(json!(true))
        }
        "testLol" => {
            tracing::info!("Lol! {d}");
            app.to(
                "thread_16764044-4158-4577-a036-482352577036",
                "split",
                &json!({ "number": "69420", "split": 123 }),
            )
            .await;
            Some(json!(true))
        }
        "updateThread" => {
            let thread = &d["thread"];
            if let Some(meta) = ThreadMeta::from_json(thread) {
                if let Some(h) = app.thread_by_uuid(&meta.uuid) {
                    let _ = h.tx.send(Msg::Update(Box::new(meta.clone()))).await;
                }
                let mut msg = json!({ "threadInfo": thread });
                if !d["newPost"].is_null() {
                    msg["newPost"] = d["newPost"].clone();
                }
                app.to(format!("thread_{}", meta.uuid), "thread_update", &msg).await;
            }
            Some(json!(true))
        }
        "statsResults" => {
            let uuid = d["counter"]["uuid"].as_str().unwrap_or("");
            app.to_counter(
                uuid,
                "stats_results",
                &json!({ "results": d["results"], "counters": d["counters"] }),
            )
            .await;
            Some(json!(true))
        }
        "updateUser" => {
            app.push_user(&s("uuid")).await;
            app.reload_counter(&s("uuid")).await;
            Some(json!(true))
        }
        "updateUserItems" => {
            let items = app.user_items(&s("uuid")).await;
            app.to_counter(&s("uuid"), "setItems", &items).await;
            Some(json!(true))
        }
        "updateUserMoney" => {
            let uuid = s("user_uuid");
            if let Some(mut u) = app.user_json(&uuid).await {
                let money = counting::js::parse_float(u["money"].as_str().unwrap_or("0"))
                    + d["money"]
                        .as_f64()
                        .unwrap_or_else(|| counting::js::parse_float(d["money"].as_str().unwrap_or("0")));
                u["money"] = json!(money);
                app.to_counter(&uuid, "setUser", &u).await;
            }
            Some(json!(true))
        }
        "bingoChatFromDiscord" => {
            crate::bingo::chat_from_discord(&app, &d).await;
            Some(json!(true))
        }
        "emitSplits" => {
            emit_splits(&app, &d["thread"], &d["split"]).await;
            Some(json!(true))
        }
        "emitAchievementEarned" => {
            app.to_counter(&s("counter_uuid"), "achievement", &d["achievement"])
                .await;
            Some(json!(true))
        }
        "emitForcePageNavToCounter" => {
            app.to_counter(
                &s("counter_uuid"),
                "forcePageNav",
                &json!({ "forcePageNav": d["page_location"] }),
            )
            .await;
            Some(json!(true))
        }
        "counterJoined" => {
            app.to_all("counter_joined", &json!({ "counter": d["counter"] })).await;
            Some(json!(true))
        }
        "emitSystemMessage" | "sendSystemMessage" => {
            app.system_message(&s("message")).await;
            Some(json!(true))
        }
        "emitUnlockInventoryItem" => {
            let team = d["team"].as_str().filter(|t| !t.is_empty());
            match team {
                Some(t) => {
                    app.to(
                        t.to_owned(),
                        "new_inventory",
                        &json!({ "newItem": d["inventory_item"], "unlocker": d["unlocker"], "teamUnlock": true }),
                    )
                    .await
                }
                None => {
                    let uuid = d["unlocker"]["uuid"].as_str().unwrap_or("");
                    app.to_counter(
                        uuid,
                        "new_inventory",
                        &json!({ "newItem": d["inventory_item"], "unlocker": d["unlocker"] }),
                    )
                    .await
                }
            }
            Some(json!(true))
        }
        "disconnectAllClients" => {
            tracing::info!("Closing socket...");
            let _ = app.io.disconnect().await;
            Some(json!(true))
        }
        "updateCounterXP" => {
            let xp = d["xp"].as_f64().unwrap_or(0.0) as i64;
            app.announce_xp(&s("counter_uuid"), xp).await;
            Some(json!(true))
        }
        "unlockItem" => {
            let uuid = s("counter_uuid");
            app.to_counter(&uuid, "item", &json!({ "item": d["item"] })).await;
            let items = app.user_items(&uuid).await;
            app.to_counter(&uuid, "setItems", &items).await;
            Some(json!(true))
        }
        "broadcastChatPost" => {
            app.to(
                "site_chat",
                "chat_post",
                &json!({ "post": d["post"], "counter": d["counter"] }),
            )
            .await;
            Some(json!(true))
        }
        "deleteCommentAsSystem" => {
            features::delete_comment_as(&app, &app.system_counter, &s("uuid")).await;
            Some(json!(true))
        }
        "discordChatPost" => {
            discord_post(&app, &d).await;
            Some(json!(true))
        }
        other => {
            tracing::warn!("unknown TCP command {other}");
            None
        }
    }
}

async fn discord_post(app: &App, d: &Value) {
    let Some(counter) = app.counter(d["counter_uuid"].as_str().unwrap_or("")).await else {
        return;
    };
    let Some(name) = d["thread_name"].as_str() else { return };
    let Some(h) = app.thread(name) else { return };
    let job = PostJob {
        counter,
        text: d["text"].as_str().unwrap_or("").to_owned(),
        post_hash: None,
        macro_preset_id: None,
        macro_preset_version_id: None,
        received: Instant::now(),
        origin: Origin::Discord {
            message_id: d["discord_message_id"].as_str().map(str::to_owned),
        },
    };
    let _ = h.tx.send(Msg::Post(Box::new(job))).await;
}

pub async fn emit_splits(app: &Arc<App>, thread: &Value, split: &Value) {
    let Some(uuid) = thread["uuid"].as_str() else { return };
    features::cache_split(app, uuid, split).await;
    app.to(format!("thread_{uuid}"), "split", split).await;
    if let Some(meta) = app.thread_meta(uuid) {
        crate::bingo::on_split(app, &meta, split).await;
        crate::rank::on_split(app, &meta, split).await;
    }
}
