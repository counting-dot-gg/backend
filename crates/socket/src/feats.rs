use crate::app::App;
use crate::daily::{eastern_day_key, uuid_ms};
use crate::model::{Counter, Post, ThreadMeta};
use serde_json::{Value, json};
use std::collections::HashMap;

pub fn person(c: &Counter) -> Value {
    let f = |k: &str| c.fields.get(k).cloned().unwrap_or(Value::Null);
    json!({ "uuid": c.uuid, "name": f("name"), "username": c.username, "color": f("color"), "discordId": f("discordId"), "avatar": f("avatar") })
}

#[derive(Clone, Copy)]
pub struct Lifetime {
    pub counts: i64,
    pub joined_ms: i64,
}

pub async fn on_valid_count(app: &App, counter: &Counter, post: &Post) {
    let Some(author_id) = counter.id else { return };
    let life = match app.lifetime.get_mut(&counter.uuid) {
        Some(mut l) => {
            l.counts += 1;
            *l
        }
        None => {
            let Some(l) = load_lifetime(app, counter, author_id, post).await else {
                return;
            };
            *app.lifetime
                .entry(counter.uuid.clone())
                .and_modify(|e| e.counts += 1)
                .or_insert(l)
        }
    };
    let n = life.counts;
    if matches!(n, 10_000 | 50_000 | 100_000) || (n > 0 && n % 1_000_000 == 0) {
        app.to_all(
            "counter_milestone",
            &json!({ "kind": "lifetime_counts", "n": n, "counter": person(counter) }),
        )
        .await;
    }
    anniversary(app, counter, life.joined_ms, uuid_ms(&post.uuid)).await;
}

async fn load_lifetime(app: &App, counter: &Counter, author_id: u32, post: &Post) -> Option<Lifetime> {
    let before: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM post FORCE INDEX (n_author_id_valid) WHERE authorId = ? AND id < ? AND isValidCount = 1",
    )
    .bind(author_id)
    .bind(post.id)
    .fetch_one(&app.pool)
    .await
    .ok()?;
    let joined_ms = uuid_ms(&counter.uuid).max(0);
    Some(Lifetime {
        counts: before + 1,
        joined_ms,
    })
}

async fn anniversary(app: &App, counter: &Counter, joined_ms: i64, now_ms: i64) {
    let today = eastern_day_key(now_ms);
    if app.anniversary_checked.get(&counter.uuid).is_some_and(|d| *d == today) {
        return;
    }
    app.anniversary_checked.insert(counter.uuid.clone(), today.clone());
    if joined_ms <= 0 {
        return;
    }
    let first = eastern_day_key(joined_ms);
    let (Some((fy, fmd)), Some((ty, tmd))) = (first.split_once('-'), today.split_once('-')) else {
        return;
    };
    let years = ty.parse::<i64>().unwrap_or(0) - fy.parse::<i64>().unwrap_or(0);
    if fmd != tmd || years < 1 {
        return;
    }
    let mut r = app.redis.clone();
    let fresh: Option<String> = redis::cmd("SET")
        .arg(format!("feats:anniversary:{}:{ty}", counter.uuid))
        .arg(1)
        .arg("NX")
        .arg("EX")
        .arg(3 * 86400)
        .query_async(&mut r)
        .await
        .ok()
        .flatten();
    if fresh.is_some() {
        app.to_all(
            "counter_milestone",
            &json!({ "kind": "anniversary", "years": years, "counter": person(counter) }),
        )
        .await;
    }
}

static RUN_RECORDS: std::sync::LazyLock<parking_lot::Mutex<(bool, Option<std::time::Instant>)>> =
    std::sync::LazyLock::new(|| parking_lot::Mutex::new((false, None)));

async fn run_records_on(app: &App) -> bool {
    let stale = RUN_RECORDS
        .lock()
        .1
        .is_none_or(|t| t.elapsed() > std::time::Duration::from_secs(30));
    if stale {
        let on: Option<i8> =
            sqlx::query_scalar("SELECT enabled FROM feature_flag WHERE flagKey = 'run_records' LIMIT 1")
                .fetch_optional(&app.pool)
                .await
                .ok()
                .flatten();
        *RUN_RECORDS.lock() = (on.unwrap_or(0) != 0, Some(std::time::Instant::now()));
    }
    RUN_RECORDS.lock().0
}

pub async fn on_run(app: &App, thread: &ThreadMeta, vcn: i64, ms: f64, interval: i64, is_get: bool) {
    if interval <= 0 || !(ms > 0.0) {
        return;
    }
    if !run_records_on(app).await {
        return;
    }
    let us = (ms * 1000.0).round() as i64;
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT authorUUID, uuid FROM post FORCE INDEX (n_thread_validcountnum) WHERE threadId = ? AND isValidCount = 1 \
         AND validCountNumber BETWEEN ? AND ? ORDER BY id",
    )
    .bind(thread.id)
    .bind(vcn - interval + 1)
    .bind(vcn)
    .fetch_all(&app.pool)
    .await
    .unwrap_or_default();
    if rows.is_empty() || looks_fake(&rows) {
        return;
    }
    let runners = crate::rank::qualified_counters(rows.iter().map(|(a, _)| a.as_str()));
    let title = thread
        .row
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or(&thread.name)
        .to_owned();
    let kind = if is_get { "speed" } else { "split" };

    if !app.thread_best.contains_key(&thread.name) {
        let loaded = stored_thread_bests(app, &thread.name).await;
        app.thread_best.entry(thread.name.clone()).or_insert(loaded);
    }
    let record_prev = lower(
        &mut app.thread_best.get_mut(&thread.name).expect("inserted above"),
        is_get,
        us,
    );
    if let Some(p) = record_prev {
        let mut people = Vec::new();
        for uuid in &runners {
            if let Some(c) = app.counter(uuid).await {
                people.push(person(&c));
            }
        }
        let msg = json!({
            "kind": kind, "ms": ms, "prevMs": p as f64 / 1000.0,
            "threadName": thread.name, "threadTitle": title, "counters": people,
        });
        app.to_all("thread_record", &msg).await;
    }

    for uuid in runners {
        let key = (uuid.clone(), thread.name.clone());
        if !app.run_best.contains_key(&key) {
            let loaded = stored_run_bests(app, &uuid, &thread.name).await;
            app.run_best.entry(key.clone()).or_insert(loaded);
        }
        let prev = lower(&mut app.run_best.get_mut(&key).expect("inserted above"), is_get, us);
        if let (Some(p), None) = (prev, record_prev) {
            let msg = json!({
                "kind": format!("{kind}_pb"), "ms": ms, "prevMs": p as f64 / 1000.0,
                "threadName": thread.name, "threadTitle": title,
            });
            app.to_counter(&uuid, "milestone", &msg).await;
        }
    }
}

fn lower(best: &mut (Option<i64>, Option<i64>), is_get: bool, us: i64) -> Option<i64> {
    let slot = if is_get { &mut best.1 } else { &mut best.0 };
    let prev = *slot;
    if prev.is_none_or(|p| us < p) {
        *slot = Some(us);
    }
    prev.filter(|p| us < *p)
}

fn looks_fake(rows: &[(String, String)]) -> bool {
    const SELF_REPLY_NS: i64 = 60_000_000;
    const AVERAGE_NS: i64 = 80_000_000;
    let mut by_author: HashMap<&str, Vec<i64>> = HashMap::new();
    for (a, u) in rows {
        by_author.entry(a).or_default().push(crate::daily::uuid_ns(u));
    }
    let fails_average = |gaps: &[i64], start: usize, end: usize| {
        if start == 0 {
            return true;
        }
        (0..start).rev().all(|from| {
            let w = &gaps[from..end];
            w.iter().sum::<i64>() / w.len() as i64 <= AVERAGE_NS
        })
    };
    by_author.values().filter(|t| t.len() >= 3).any(|times| {
        let gaps: Vec<i64> = times.windows(2).map(|w| w[1] - w[0]).collect();
        let mut start: Option<usize> = None;
        for (i, &g) in gaps.iter().enumerate() {
            if g < SELF_REPLY_NS {
                start.get_or_insert(i);
            } else if let Some(s) = start.take()
                && i - s >= 2
                && fails_average(&gaps, s, i)
            {
                return true;
            }
        }
        start.is_some_and(|s| gaps.len() - s >= 2 && fails_average(&gaps, s, gaps.len()))
    })
}

async fn stored_run_bests(app: &App, uuid: &str, thread: &str) -> (Option<i64>, Option<i64>) {
    let t = redis_json(app, &format!("stats:counter:{uuid}"))
        .await
        .map(|v| v["threads"][thread].clone())
        .unwrap_or(Value::Null);
    (t["best_split_us"].as_i64(), t["best_speed_us"].as_i64())
}

async fn stored_thread_bests(app: &App, thread: &str) -> (Option<i64>, Option<i64>) {
    let mut r = app.redis.clone();
    let raw: Option<Vec<u8>> = redis::cmd("GET")
        .arg(format!("stats:thread:{thread}"))
        .query_async(&mut r)
        .await
        .ok()
        .flatten();
    const UNKNOWN: (Option<i64>, Option<i64>) = (Some(0), Some(0));
    let Some(raw) = raw else { return UNKNOWN };
    match bests_from_rollup(&raw) {
        Ok((split, speed)) => (split.or(Some(0)), speed.or(Some(0))),
        Err(e) => {
            tracing::warn!("[feats] stats:thread:{thread} unreadable ({e}); no thread records for it until restart");
            UNKNOWN
        }
    }
}

fn bests_from_rollup(raw: &[u8]) -> Result<(Option<i64>, Option<i64>), String> {
    let t = if stats::packed::is_packed(raw) {
        stats::packed::decode(raw).map_err(|e| e.to_string())?
    } else {
        serde_json::from_slice::<stats::models::ThreadStats>(raw).map_err(|e| e.to_string())?
    };
    let split = t
        .split_speed
        .unwrap_or_default()
        .into_iter()
        .filter(|r| !r.is_fake && r.time > 0)
        .map(|r| r.time)
        .min();
    let speed = t
        .speed
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.time > 0)
        .map(|r| r.time)
        .min();
    Ok((split, speed))
}

async fn redis_json(app: &App, key: &str) -> Option<Value> {
    let mut r = app.redis.clone();
    let raw: Option<String> = redis::cmd("GET").arg(key).query_async(&mut r).await.ok().flatten();
    raw.and_then(|s| serde_json::from_str(&s).ok())
}

#[cfg(test)]
mod rollup_tests {
    use super::bests_from_rollup;
    use stats::models::{SpeedEntry, SplitSpeedEntry, ThreadStats};

    fn split(n: i64, time: i64, is_fake: bool) -> SplitSpeedEntry {
        SplitSpeedEntry {
            start: String::new(),
            end: String::new(),
            time,
            start_count: String::new(),
            end_count: String::new(),
            start_count_number: n,
            end_count_number: n + 99,
            qualified_counters: vec!["00000000-0000-0000-0000-000000000001".into()],
            is_fake,
        }
    }

    fn rollup() -> ThreadStats {
        ThreadStats {
            split_speed: Some(vec![
                split(1, 30_000_000, false),
                split(101, 22_907_000, false),
                split(201, 1_900_000, true),
                split(301, 24_909_000, false),
            ]),
            speed: Some(vec![SpeedEntry {
                start: String::new(),
                end: String::new(),
                time: 5_000_000,
                start_count: String::new(),
                end_count: String::new(),
                start_count_number: 1,
                end_count_number: 1000,
                qualified_counters: vec!["00000000-0000-0000-0000-000000000001".into()],
            }]),
            ..Default::default()
        }
    }

    #[test]
    fn reads_the_packed_rollup_a_full_init_writes() {
        let raw = stats::packed::encode(&rollup()).unwrap();
        assert!(stats::packed::is_packed(&raw));
        assert_eq!(bests_from_rollup(&raw), Ok((Some(22_907_000), Some(5_000_000))));
    }

    #[test]
    fn still_reads_json_from_older_builds() {
        let raw = serde_json::to_vec(&rollup()).unwrap();
        assert_eq!(bests_from_rollup(&raw), Ok((Some(22_907_000), Some(5_000_000))));
    }

    #[test]
    #[ignore]
    fn live_rollups_match_the_leaderboard() {
        let url = std::env::var("CGG_STATS_REDIS").expect("CGG_STATS_REDIS");
        let mut con = redis::Client::open(url).unwrap().get_connection().unwrap();
        for pair in std::env::var("CGG_EXPECT").expect("CGG_EXPECT").split(',') {
            let (thread, want) = pair.split_once('=').unwrap();
            let raw: Vec<u8> = redis::cmd("GET")
                .arg(format!("stats:thread:{thread}"))
                .query(&mut con)
                .unwrap();
            let (split, _) = bests_from_rollup(&raw).unwrap();
            assert_eq!(split, Some(want.parse::<i64>().unwrap()), "{thread}");
        }
    }

    #[test]
    fn garbage_is_an_error_not_a_missing_record() {
        assert!(bests_from_rollup(b" nonsense").is_err());
    }
}
