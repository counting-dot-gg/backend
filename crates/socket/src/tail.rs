use crate::app::App;
use crate::model::Post;
use crate::thread::{RING, ThreadHandle};
use serde_json::{Value, json};
use socketioxide::extract::SocketRef;
use std::collections::VecDeque;
use std::sync::Arc;

pub fn newest(ring: &VecDeque<Arc<Post>>, limit: usize) -> Vec<Arc<Post>> {
    ring.iter().rev().take(limit).cloned().collect()
}

pub fn after(ring: &VecDeque<Arc<Post>>, uuid: &str) -> Option<Vec<Arc<Post>>> {
    let at = ring.iter().position(|p| p.uuid == uuid)?;
    Some(ring.iter().skip(at + 1).cloned().collect())
}

pub fn reloaded(ring: &VecDeque<Arc<Post>>, stored: Vec<Post>) -> VecDeque<Arc<Post>> {
    let newest_stored = stored.iter().map(|p| p.id).max().unwrap_or(0);
    let unpersisted = ring.iter().filter(|p| p.id > newest_stored).cloned();
    let mut out: VecDeque<Arc<Post>> = stored.into_iter().map(Arc::new).chain(unpersisted).collect();
    let excess = out.len().saturating_sub(RING);
    out.drain(..excess);
    out
}

fn posts_json(posts: &[Arc<Post>]) -> Vec<Value> {
    posts.iter().map(|p| p.json_with_nulls()).collect()
}

pub fn newest_command(app: &App, d: &Value) -> Option<Value> {
    let h = app.thread(d["thread_name"].as_str()?)?;
    let limit = d["limit"].as_u64().unwrap_or(50).min(RING as u64) as usize;
    let ring = h.ring.read();
    let posts = newest(&ring, limit);
    let whole_thread = ring.len() < RING;
    Some(json!({ "posts": posts_json(&posts), "wholeThread": whole_thread }))
}

pub fn post_command(app: &App, d: &Value) -> Value {
    let found = d["uuid"]
        .as_str()
        .and_then(|uuid| crate::features::find_post_in_memory(app, uuid));
    json!({ "post": found.map(|p| p.json_with_nulls()) })
}

pub async fn reload(app: &App, h: &ThreadHandle) -> anyhow::Result<usize> {
    let thread_id = h.meta.read().id;
    let stored = crate::db::thread_tail(&app.pool, thread_id).await?;
    let mut ring = h.ring.write();
    *ring = reloaded(&ring, stored);
    Ok(ring.len())
}

pub async fn reload_command(app: &App, d: &Value) -> Value {
    let Some(h) = d["thread_name"].as_str().and_then(|n| app.thread(n)) else {
        return json!({ "ok": false, "error": "unknown thread" });
    };
    match reload(app, &h).await {
        Ok(n) => json!({ "ok": true, "posts": n }),
        Err(e) => json!({ "ok": false, "error": e.to_string() }),
    }
}

pub async fn catch_up(app: &App, s: &SocketRef, h: &ThreadHandle, name: &str, newest_seen: &str) {
    let missed = after(&h.ring.read(), newest_seen);
    let complete = missed.is_some();
    let posts = missed.unwrap_or_default();
    let authors: Vec<String> = posts.iter().map(|p| p.author_uuid.clone()).collect();
    let counters = app.counters_of(&authors).await;
    let counters: Vec<_> = counters.iter().map(|c| &c.json).collect();
    s.emit(
        "catchUp",
        &json!({ "thread_name": name, "posts": posts_json(&posts), "counters": counters, "complete": complete }),
    )
    .ok();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn post(id: u64) -> Post {
        serde_json::from_value(json!({
            "uuid": format!("u{id}"), "timestamp": "1", "timeSinceLastCount": "1", "timeSinceLastPost": "1",
            "rawText": "1", "isCount": true, "isValidCount": true, "stricken": false, "thread": "t", "threadId": 1,
            "hasComment": false, "hasThreeCharComment": false, "authorUUID": "a", "authorId": 1,
            "isDeleted": false, "isCommentDeleted": false, "reactions": {}, "macroPresetId": null,
            "macroPresetVersionId": null, "id": id
        }))
        .unwrap()
    }

    fn ring(ids: impl IntoIterator<Item = u64>) -> VecDeque<Arc<Post>> {
        ids.into_iter().map(|i| Arc::new(post(i))).collect()
    }

    #[test]
    fn newest_comes_newest_first() {
        let r = ring(1..=5);
        let ids: Vec<u64> = newest(&r, 3).iter().map(|p| p.id).collect();
        assert_eq!(ids, [5, 4, 3]);
    }

    #[test]
    fn catch_up_returns_only_posts_after_the_one_seen() {
        let r = ring(1..=5);
        let ids: Vec<u64> = after(&r, "u3").unwrap().iter().map(|p| p.id).collect();
        assert_eq!(ids, [4, 5]);
        assert!(after(&r, "u5").unwrap().is_empty());
        assert!(after(&r, "u99").is_none());
    }

    #[test]
    fn reload_takes_mysql_but_keeps_posts_not_yet_stored() {
        let r = ring([1, 2, 3, 4]);
        let stored = vec![post(1), post(3)];
        let ids: Vec<u64> = reloaded(&r, stored).iter().map(|p| p.id).collect();
        assert_eq!(ids, [1, 3, 4]);
    }

    #[test]
    fn reload_keeps_at_most_a_ring() {
        let r = ring([RING as u64 + 10]);
        let stored: Vec<Post> = (1..=RING as u64).map(post).collect();
        let out = reloaded(&r, stored);
        assert_eq!(out.len(), RING);
        assert_eq!(out.back().unwrap().id, RING as u64 + 10);
        assert_eq!(out.front().unwrap().id, 2);
    }
}
