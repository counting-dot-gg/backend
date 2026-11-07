use crate::app::App;
use crate::model::CounterRef;
use chrono::TimeZone;
use indexmap::IndexMap;
use serde_json::{Map, Value, json};
use sqlx::Row;
use std::collections::HashMap;

pub struct Entry {
    pub counter: Option<CounterRef>,
    pub counts: i64,
}

#[derive(Default)]
pub struct Daily {
    pub threads: IndexMap<String, IndexMap<String, Entry>>,
    pub all: IndexMap<String, Entry>,
    pub total_counts: Option<i64>,
    pub total_posts: Option<i64>,
    pub last_updated: Option<String>,
    pub bank: Option<i64>,
    pub robs: Vec<Value>,
    pub rob_counters: Vec<CounterRef>,
}

fn entries_json(m: &IndexMap<String, Entry>) -> Value {
    let mut o = Map::new();
    for (uuid, e) in m {
        let v = match &e.counter {
            Some(c) => json!({ "counter": &c.json, "counts": e.counts }),
            None => json!({ "counts": e.counts }),
        };
        o.insert(uuid.clone(), v);
    }
    Value::Object(o)
}

impl Daily {
    pub fn hoc(&self, thread_uuid: &str) -> Value {
        self.threads.get(thread_uuid).map_or_else(|| json!({}), entries_json)
    }

    pub fn leaderboard(&self) -> Value {
        let mut o = Map::new();
        for (t, m) in &self.threads {
            o.insert(t.clone(), entries_json(m));
        }
        o.insert("all".into(), entries_json(&self.all));
        o.insert("total_counts".into(), json!(self.total_counts));
        o.insert("total_posts".into(), json!(self.total_posts));
        if let Some(l) = &self.last_updated {
            o.insert("last_updated".into(), json!(l));
        }
        Value::Object(o)
    }

    pub fn robs_json(&self) -> Value {
        json!({
            "robs": self.robs,
            "counters": self.rob_counters.iter().map(|c| &c.json).collect::<Vec<_>>(),
        })
    }

    pub fn add_count(&mut self, thread_uuid: &str, counter: &CounterRef) -> i64 {
        let t = self.threads.entry(thread_uuid.to_owned()).or_default();
        t.entry(counter.uuid.clone())
            .or_insert(Entry {
                counter: Some(counter.clone()),
                counts: 0,
            })
            .counts += 1;
        let a = self.all.entry(counter.uuid.clone()).or_insert(Entry {
            counter: Some(counter.clone()),
            counts: 0,
        });
        a.counts += 1;
        a.counts
    }
}

pub fn eastern_day_bounds(ms: i64) -> (i64, i64) {
    let ny = chrono_tz::America::New_York;
    let d = ny.timestamp_millis_opt(ms).unwrap().date_naive();
    let start = ny
        .from_local_datetime(&d.and_hms_opt(0, 0, 0).unwrap())
        .earliest()
        .unwrap();
    let next = d.succ_opt().unwrap();
    let end = ny
        .from_local_datetime(&next.and_hms_opt(0, 0, 0).unwrap())
        .earliest()
        .unwrap();
    (start.timestamp_millis(), end.timestamp_millis())
}

pub fn eastern_day_key(ms: i64) -> String {
    chrono_tz::America::New_York
        .timestamp_millis_opt(ms)
        .unwrap()
        .format("%Y-%m-%d")
        .to_string()
}

pub fn uuid_ms(uuid: &str) -> i64 {
    uuid_ns(uuid) / 1_000_000
}

pub fn uuid_ns(uuid: &str) -> i64 {
    let mut ns = String::new();
    for (i, ch) in uuid.chars().enumerate() {
        if ns.len() == 19 {
            break;
        }
        if matches!(i, 8 | 13 | 14 | 18 | 19 | 23) {
            continue;
        }
        ns.push(ch);
    }
    ns.parse::<i64>().unwrap_or(0)
}

pub fn totals_key(database: &str) -> String {
    format!("cgg:socket:totals:{database}")
}

pub async fn seed_totals(
    pool: &sqlx::MySqlPool,
    redis: &mut redis::aio::ConnectionManager,
    database: &str,
) -> anyhow::Result<()> {
    let t0 = std::time::Instant::now();
    let max_id: i64 = sqlx::query_scalar("SELECT CAST(COALESCE(MAX(id), 0) AS SIGNED) FROM post")
        .fetch_one(pool)
        .await?;
    tracing::info!("counting valid posts up to id {max_id}...");
    let counts: i64 =
        sqlx::query_scalar("SELECT CAST(COUNT(*) AS SIGNED) FROM post WHERE isValidCount = 1 AND id <= ?")
            .bind(max_id)
            .fetch_one(pool)
            .await?;
    let v = serde_json::json!({ "counts": counts, "maxId": max_id, "at": crate::app::now_ms() }).to_string();
    redis::cmd("SET")
        .arg(totals_key(database))
        .arg(&v)
        .query_async::<_, ()>(redis)
        .await?;
    tracing::info!("totals snapshot {v} stored in {:.1}s", t0.elapsed().as_secs_f64());
    Ok(())
}

pub async fn load(app: &App, upto_id: u64) -> anyhow::Result<Daily> {
    let pool = &app.pool;
    let now = crate::app::now_ms();
    let (start, end) = eastern_day_bounds(now);
    let mut d = Daily::default();

    let rows = sqlx::query(
        "SELECT thread, authorUUID, CAST(SUM(isValidCount = 1) AS SIGNED) AS valid, MIN(id) AS first \
         FROM post WHERE timestamp > ? AND timestamp < ? AND id <= ? \
         GROUP BY thread, authorUUID ORDER BY first",
    )
    .bind(start.to_string())
    .bind(end.to_string())
    .bind(upto_id as i64)
    .fetch_all(pool)
    .await?;
    let authors: Vec<String> = rows.iter().map(|r| r.get::<String, _>("authorUUID")).collect();
    let counters: HashMap<String, CounterRef> = app
        .counters_of(&authors)
        .await
        .into_iter()
        .map(|c| (c.uuid.clone(), c))
        .collect();
    for t in app.threads_by_uuid.iter() {
        d.threads.insert(t.key().clone(), IndexMap::new());
    }
    for r in &rows {
        let (thread, author): (String, String) = (r.get("thread"), r.get("authorUUID"));
        let valid: i64 = r.get("valid");
        let c = counters.get(&author).cloned();
        d.threads.entry(thread).or_default().insert(
            author.clone(),
            Entry {
                counter: c.clone(),
                counts: valid,
            },
        );
        d.all.entry(author).or_insert(Entry { counter: c, counts: 0 }).counts += valid;
    }

    let max_id: i64 = sqlx::query_scalar("SELECT CAST(COALESCE(MAX(id), 0) AS SIGNED) FROM post WHERE id <= ?")
        .bind(upto_id as i64)
        .fetch_one(pool)
        .await?;
    let mut redis = app.redis.clone();
    let snap: Option<String> = redis::cmd("GET")
        .arg(totals_key(&app.cfg.mysql.database))
        .query_async(&mut redis)
        .await
        .ok()
        .flatten();
    let snap: Option<(i64, i64)> = snap
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| Some((v["counts"].as_i64()?, v["maxId"].as_i64()?)));
    let total_counts = match snap {
        Some((counts, snap_max)) if snap_max > max_id => {
            tracing::warn!("totals snapshot is ahead of MySQL ({snap_max} > {max_id}); using it as is");
            counts
        }
        Some((counts, snap_max)) => {
            let since: i64 = sqlx::query_scalar(
                "SELECT CAST(COUNT(*) AS SIGNED) FROM post WHERE id > ? AND id <= ? AND isValidCount = 1",
            )
            .bind(snap_max)
            .bind(max_id)
            .fetch_one(pool)
            .await?;
            counts + since
        }
        _ => {
            tracing::info!("counting valid counts sitewide (no totals snapshot)...");
            sqlx::query_scalar("SELECT CAST(COUNT(*) AS SIGNED) FROM post WHERE isValidCount = 1 AND id <= ?")
                .bind(max_id)
                .fetch_one(pool)
                .await?
        }
    };
    d.total_counts = Some(total_counts);
    d.total_posts = Some(max_id);

    if let Some(main) = app.thread("main") {
        let main_id = main.meta.read().id;
        let rob_post: Option<String> = sqlx::query_scalar("SELECT postUUID FROM rob ORDER BY id DESC LIMIT 1")
            .fetch_optional(pool)
            .await?;
        let after_id: Option<i64> = match rob_post {
            Some(u) => {
                sqlx::query_scalar("SELECT CAST(id AS SIGNED) FROM post WHERE uuid = ? LIMIT 1")
                    .bind(u)
                    .fetch_optional(pool)
                    .await?
            }
            None => None,
        };
        let bank: i64 = match after_id {
            Some(a) => {
                sqlx::query_scalar(
                    "SELECT CAST(COUNT(*) AS SIGNED) FROM post FORCE INDEX (n_thread_valid_id) \
                     WHERE threadId = ? AND isValidCount = 1 AND id > ?",
                )
                .bind(main_id)
                .bind(a)
                .fetch_one(pool)
                .await?
            }
            None => 0,
        };
        d.bank = Some(bank);
    }

    d.robs = today_robs(app, now).await?;
    let uuids: Vec<String> = d
        .robs
        .iter()
        .filter_map(|r| r["counterUUID"].as_str().map(str::to_owned))
        .collect();
    d.rob_counters = app.counters_of(&uuids).await;
    Ok(d)
}

pub async fn today_robs(app: &App, now: i64) -> anyhow::Result<Vec<Value>> {
    let (start, _) = eastern_day_bounds(now);
    let rows = sqlx::query(
        "SELECT id, counterUUID, postUUID, CAST(moneyRobbed AS CHAR) AS moneyRobbed, timestamp FROM rob \
         WHERE timestamp >= ? ORDER BY id",
    )
    .bind(start)
    .fetch_all(&app.pool)
    .await?;
    Ok(rows.iter().map(rob_json).collect())
}

pub fn rob_json(r: &sqlx::mysql::MySqlRow) -> Value {
    json!({
        "id": r.get::<i32, _>("id"),
        "counterUUID": r.get::<String, _>("counterUUID"),
        "postUUID": r.get::<String, _>("postUUID"),
        "moneyRobbed": r.get::<String, _>("moneyRobbed"),
        "timestamp": r.get::<String, _>("timestamp"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_and_uuid_time() {
        let ms = chrono::Utc
            .with_ymd_and_hms(2026, 3, 8, 12, 0, 0)
            .unwrap()
            .timestamp_millis();
        let (s, e) = eastern_day_bounds(ms);
        assert_eq!(e - s, 23 * 3600 * 1000);
        assert_eq!(eastern_day_key(ms), "2026-03-08");
        assert_eq!(uuid_ms("17913212-2616-4321-9917-258305321917"), 1791321226163);
    }

    #[test]
    fn fall_back_day_is_25_hours() {
        let first_1am = chrono::Utc
            .with_ymd_and_hms(2026, 11, 1, 5, 30, 0)
            .unwrap()
            .timestamp_millis();
        let second_1am = chrono::Utc
            .with_ymd_and_hms(2026, 11, 1, 6, 30, 0)
            .unwrap()
            .timestamp_millis();
        let (s, e) = eastern_day_bounds(first_1am);
        assert_eq!(e - s, 25 * 3600 * 1000);
        assert_eq!(eastern_day_bounds(second_1am), (s, e), "both 01:30s are the same day");
        assert_eq!(eastern_day_key(first_1am), "2026-11-01");
        assert_eq!(eastern_day_key(second_1am), "2026-11-01");
        assert_eq!(
            s,
            chrono::Utc
                .with_ymd_and_hms(2026, 11, 1, 4, 0, 0)
                .unwrap()
                .timestamp_millis()
        );
        assert_eq!(
            e,
            chrono::Utc
                .with_ymd_and_hms(2026, 11, 2, 5, 0, 0)
                .unwrap()
                .timestamp_millis()
        );
        assert_eq!(eastern_day_key(e - 1), "2026-11-01");
        assert_eq!(eastern_day_key(e), "2026-11-02");
    }
}
