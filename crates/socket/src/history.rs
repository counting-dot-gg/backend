use crate::app::App;
use crate::db;
use crate::gateway::counter_of;
use crate::model::Post;
use serde_json::{Value, json};
use socketioxide::extract::SocketRef;
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq)]
pub enum Dir {
    Older,
    Newer,
}

pub async fn page(app: &App, s: &SocketRef, data: &Value, dir: Dir, chats: bool) {
    const TAKE: usize = 50;
    let (Some(name), Some(anchor_uuid)) = (data["thread_name"].as_str(), data["uuid"].as_str()) else {
        return;
    };
    let counter = counter_of(app, s).await;
    let Some(h) = app.thread(name) else { return };
    let thread_id = {
        let m = h.meta.read();
        if !m.visible_to_counter(counter.as_deref()) {
            return;
        }
        m.id
    };
    let ring: Vec<Arc<Post>> = h.ring.read().iter().cloned().collect();
    let anchor_id = match ring.iter().find(|p| p.uuid == anchor_uuid) {
        Some(p) => p.id,
        None => match db::post_id_in_thread(&app.pool, anchor_uuid, thread_id).await {
            Ok(Some(id)) => id,
            _ => return,
        },
    };
    let keep = |p: &Post| !chats || p.has_three_char_comment;
    let mut merged: BTreeMap<u64, Post> = BTreeMap::new();
    for p in &ring {
        let side = match dir {
            Dir::Older => p.id < anchor_id,
            Dir::Newer => p.id > anchor_id,
        };
        if side && keep(p) {
            merged.insert(p.id, (**p).clone());
        }
    }
    match db::posts_around(&app.pool, thread_id, anchor_id, dir == Dir::Older, chats, TAKE).await {
        Ok(rows) => {
            for p in rows {
                merged.entry(p.id).or_insert(p);
            }
        }
        Err(e) => tracing::warn!("history query failed: {e}"),
    }
    let mut posts: Vec<Post> = match dir {
        Dir::Older => merged.into_values().rev().take(TAKE).collect(),
        Dir::Newer => merged.into_values().take(TAKE).collect(),
    };
    let complete = posts.len() < TAKE;
    posts.retain(|p| p.uuid != anchor_uuid);
    let authors: Vec<String> = posts.iter().map(|p| p.author_uuid.clone()).collect();
    let counters = app.counters_of(&authors).await;
    let counters: Vec<_> = counters.iter().map(|c| &c.json).collect();
    let (event, flag) = match (dir, chats) {
        (Dir::Older, false) => ("loadOldCounts", "isOldest"),
        (Dir::Newer, false) => ("loadNewCounts", "isNewest"),
        (Dir::Older, true) => ("loadOldChats", "isOldest"),
        (Dir::Newer, true) => ("loadNewChats", "isNewest"),
    };
    let posts: Vec<Value> = posts.iter().map(Post::json_with_nulls).collect();
    let mut out = json!({ "recentCounts": posts, "counters": counters });
    if complete {
        out[flag] = json!(true);
    }
    s.emit(event, &out).ok();
}
