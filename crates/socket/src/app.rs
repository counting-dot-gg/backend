use crate::achievements::Achievements;
use crate::config::Config;
use crate::daily::Daily;
use crate::db;
use crate::ids;
use crate::metrics::Metrics;
use crate::misc::Misc;
use crate::model::{Counter, CounterRef, ThreadMeta};
use crate::thread::ThreadHandle;
use cgg_web::nest;
use dashmap::DashMap;
use parking_lot::Mutex;
use serde::Serialize;
use serde_json::{Value, json};
use socketioxide::SocketIo;
use sqlx::MySqlPool;
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, OnceLock};

#[derive(Default)]
pub struct Online {
    pub users: HashMap<String, (Vec<String>, i64)>,
    pub total_ms: f64,
}

pub struct App {
    pub cfg: Config,
    pub pool: MySqlPool,
    pub io: SocketIo,
    pub redis: redis::aio::ConnectionManager,
    pub threads: DashMap<String, Arc<ThreadHandle>>,
    pub threads_by_uuid: DashMap<String, Arc<ThreadHandle>>,
    pub threads_by_id: DashMap<u32, Arc<ThreadHandle>>,
    pub counters: DashMap<String, CounterRef>,
    pub connections_per_ip: DashMap<String, usize>,
    pub limits: DashMap<String, Mutex<crate::gateway::Limits>>,
    pub metrics: Arc<Metrics>,
    pub bot: nest::Client,
    pub stats: nest::Client,
    pub online: Mutex<Online>,
    pub daily: Mutex<Daily>,
    pub achievements: Achievements,
    pub xp_total: AtomicI64,
    pub chat_last_read: DashMap<String, String>,
    pub day_best: DashMap<String, (String, i64)>,
    pub run_best: DashMap<(String, String), (Option<i64>, Option<i64>)>,
    pub thread_best: DashMap<String, (Option<i64>, Option<i64>)>,
    pub lifetime: DashMap<String, crate::feats::Lifetime>,
    pub anniversary_checked: DashMap<String, String>,
    pub rank_role_sent: DashMap<String, String>,
    pub misc: Mutex<Misc>,
    pub clock: Arc<ids::Clock>,
    pub system_counter: CounterRef,
    pub wal: OnceLock<crate::wal::Wal>,
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

pub fn epoch_ms_f64() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_micros() as f64
        / 1000.0
}

impl App {
    pub async fn to<T: Serialize + ?Sized>(&self, room: impl Into<String>, ev: &str, data: &T) {
        let _ = self.io.to(room.into()).emit(ev, data).await;
    }

    pub async fn to_all<T: Serialize + ?Sized>(&self, ev: &str, data: &T) {
        let _ = self.io.emit(ev, data).await;
    }

    pub async fn to_counter<T: Serialize + ?Sized>(&self, uuid: &str, ev: &str, data: &T) {
        self.to(format!("counter_{uuid}"), ev, data).await;
    }

    pub async fn system_message(&self, text: &str) {
        let post = self.fake_system_post(text);
        self.to_all("post", &json!({ "post": post, "counter": &self.system_counter.json }))
            .await;
    }

    pub fn fake_system_post(&self, text: &str) -> Value {
        json!({
            "id": 0, "rawText": text, "comment": text, "reactions": [], "rawCount": null, "thread": null,
            "timeSinceLastCount": "0", "timeSinceLastPost": "0", "timestamp": now_ms().to_string(),
            "uuid": ids::uuid_for(self.clock.now_ns()), "authorUUID": "[SYSTEM]", "stricken": false,
            "isDeleted": false, "isCommentDeleted": false, "hasComment": true, "hasThreeCharComment": true,
            "isCount": false, "isValidCount": false, "validCountNumber": null,
        })
    }

    pub async fn counter(&self, uuid: &str) -> Option<CounterRef> {
        if uuid == "[SYSTEM]" {
            return Some(self.system_counter.clone());
        }
        if let Some(c) = self.counters.get(uuid) {
            return Some(c.clone());
        }
        self.reload_counter(uuid).await
    }

    pub async fn reload_counter(&self, uuid: &str) -> Option<CounterRef> {
        match db::counter_by_uuid(&self.pool, uuid).await {
            Ok(Some(c)) => {
                let c: CounterRef = Arc::new(c);
                self.counters.insert(uuid.to_owned(), c.clone());
                Some(c)
            }
            Ok(None) => {
                self.counters.remove(uuid);
                None
            }
            Err(e) => {
                tracing::warn!("counter lookup failed: {e}");
                self.counters.get(uuid).map(|c| c.clone())
            }
        }
    }

    pub async fn counters_of(&self, uuids: &[String]) -> Vec<CounterRef> {
        let mut missing = Vec::new();
        for u in uuids {
            if !self.counters.contains_key(u) && u != "[SYSTEM]" {
                missing.push(u.clone());
            }
        }
        missing.sort();
        missing.dedup();
        if !missing.is_empty() {
            match db::counters_by_uuids(&self.pool, &missing).await {
                Ok(cs) => {
                    for c in cs {
                        self.counters.insert(c.uuid.clone(), Arc::new(c));
                    }
                }
                Err(e) => tracing::warn!("counters lookup failed: {e}"),
            }
        }
        let mut seen = std::collections::HashSet::new();
        uuids
            .iter()
            .filter(|u| seen.insert((*u).clone()))
            .filter_map(|u| self.counters.get(u).map(|c| c.clone()))
            .collect()
    }

    pub fn put_counter(&self, c: Counter) -> CounterRef {
        let c = Arc::new(c);
        self.counters.insert(c.uuid.clone(), c.clone());
        c
    }

    pub fn thread(&self, name: &str) -> Option<Arc<ThreadHandle>> {
        self.threads.get(name).map(|h| h.clone())
    }

    pub fn thread_by_uuid(&self, uuid: &str) -> Option<Arc<ThreadHandle>> {
        self.threads_by_uuid.get(uuid).map(|h| h.clone())
    }

    pub fn thread_meta(&self, uuid: &str) -> Option<ThreadMeta> {
        self.thread_by_uuid(uuid).map(|h| h.meta.read().clone())
    }

    pub async fn add_xp(&self, uuid: &str, xp: i64) {
        if let Err(e) = sqlx::query("UPDATE counters SET xp = xp + ? WHERE uuid = ?")
            .bind(xp)
            .bind(uuid)
            .execute(&self.pool)
            .await
        {
            tracing::warn!("xp update for {uuid} failed: {e}");
            return;
        }
        self.xp_total.fetch_add(xp, Ordering::Relaxed);
        self.announce_xp(uuid, xp).await;
    }

    pub async fn announce_xp(&self, uuid: &str, xp: i64) {
        self.to_counter(uuid, "xp", &xp).await;
        if let Some(c) = self.reload_counter(uuid).await {
            let total = c.xp();
            let (old, new) = (level_of(total - xp as f64), level_of(total));
            if old != new {
                self.to_counter(uuid, "levelUp", &json!({ "level": new })).await;
                if let Some(d) = c
                    .fields
                    .get("discordId")
                    .and_then(|v| v.as_str())
                    .filter(|d| !d.is_empty())
                {
                    let level: i64 = new.parse().unwrap_or(1);
                    self.bot
                        .emit("syncLevelRoles", json!({ "discordId": d, "level": level }));
                }
            }
        }
    }

    pub async fn user_json(&self, uuid: &str) -> Option<Value> {
        db::user_json(&self.pool, uuid).await.ok().flatten()
    }

    pub async fn push_user(&self, uuid: &str) {
        if let Some(u) = self.user_json(uuid).await {
            self.to_counter(uuid, "setUser", &u).await;
        }
    }

    pub async fn add_money(&self, uuid: &str, money: i64) {
        if let Err(e) = sqlx::query("UPDATE users SET money = money + ? WHERE uuid = ?")
            .bind(money)
            .bind(uuid)
            .execute(&self.pool)
            .await
        {
            tracing::warn!("money update for {uuid} failed: {e}");
            return;
        }
        self.push_user(uuid).await;
    }

    pub async fn user_items(&self, uuid: &str) -> Value {
        db::user_items(&self.pool, uuid).await.unwrap_or(Value::Null)
    }
}

pub use cgg_web::level_of;

#[cfg(test)]
mod tests {
    #[test]
    fn levels() {
        assert_eq!(super::level_of(0.0), "1");
        assert_eq!(super::level_of(99.0), "1");
        assert_eq!(super::level_of(100.0), "2");
        assert_eq!(super::level_of(2e7), "65");
        assert_eq!(super::level_of(-5.0), "1");
    }
}
