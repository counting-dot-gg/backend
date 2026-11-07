use super::packed;
use crate::state::AppState;
use serde_json::{Map, Value, json};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::time::{Duration, Instant};

pub const RANK_METRICS: [&str; 7] = ["counts", "posts", "mistakes", "accuracy", "speed", "split", "time"];

#[derive(Default)]
pub struct StatsCaches {
    index: tokio::sync::Mutex<Option<(Instant, Value)>>,
    ranklists: tokio::sync::Mutex<HashMap<String, (Instant, Value)>>,
}

fn conn(state: &AppState) -> crate::redis_conn::Redis {
    state.redis.clone()
}

pub async fn try_get_bytes(state: &AppState, key: &str) -> Result<Option<Vec<u8>>, redis::RedisError> {
    redis::cmd("GET")
        .arg(key)
        .query_async::<_, Option<Vec<u8>>>(&mut conn(state))
        .await
        .inspect_err(|e| tracing::warn!("redis GET {key}: {e}"))
}

pub async fn get_bytes(state: &AppState, key: &str) -> Option<Vec<u8>> {
    try_get_bytes(state, key).await.ok().flatten()
}

pub async fn get_json(state: &AppState, key: &str) -> Option<Value> {
    let b = get_bytes(state, key).await?;
    serde_json::from_slice(&b).ok()
}

pub async fn mget(state: &AppState, keys: &[String]) -> Vec<Option<Vec<u8>>> {
    let mut out = Vec::with_capacity(keys.len());
    for chunk in keys.chunks(256) {
        let r: Vec<Option<Vec<u8>>> = redis::cmd("MGET")
            .arg(chunk)
            .query_async(&mut conn(state))
            .await
            .unwrap_or_else(|e| {
                tracing::warn!("redis MGET: {e}");
                vec![None; chunk.len()]
            });
        out.extend(r);
    }
    out
}

pub async fn hgetall(state: &AppState, key: &str) -> Vec<(String, String)> {
    redis::cmd("HGETALL")
        .arg(key)
        .query_async::<_, Vec<(String, String)>>(&mut conn(state))
        .await
        .unwrap_or_default()
}

pub async fn hget(state: &AppState, key: &str, field: &str) -> Option<String> {
    redis::cmd("HGET")
        .arg(key)
        .arg(field)
        .query_async::<_, Option<String>>(&mut conn(state))
        .await
        .ok()
        .flatten()
}

pub async fn lean_index(state: &AppState) -> Value {
    let mut g = state.stats.index.lock().await;
    if let Some((at, v)) = g.as_ref()
        && at.elapsed() < Duration::from_secs(30)
    {
        return v.clone();
    }
    let v = match try_get_bytes(state, "stats:lean_index").await {
        Ok(Some(b)) => serde_json::from_slice(&b).unwrap_or_else(|_| json!({})),
        Ok(None) => json!({}),
        Err(_) => return g.as_ref().map_or_else(|| json!({}), |(_, stale)| stale.clone()),
    };
    *g = Some((Instant::now(), v.clone()));
    v
}

pub async fn lean_totals_for_range(state: &AppState, start: Option<&str>, end: Option<&str>) -> Map<String, Value> {
    let index = lean_index(state).await;
    let mut out = Map::new();
    for (name, stats) in index.as_object().into_iter().flatten() {
        match (start, end) {
            (Some(s), Some(e)) => {
                let mut sum = 0f64;
                for (day, n) in stats["days"].as_object().into_iter().flatten() {
                    if day.as_str() >= s && day.as_str() <= e {
                        sum += n.as_f64().unwrap_or(0.0);
                    }
                }
                if sum > 0.0 {
                    out.insert(name.clone(), crate::entity::js_number(sum));
                }
            }
            _ => {
                if stats["all"].as_f64().unwrap_or(0.0) > 0.0 {
                    out.insert(name.clone(), stats["all"].clone());
                }
            }
        }
    }
    out
}

pub async fn rank_list(state: &AppState, thread: &str, metric: &str) -> Value {
    let key = format!("stats:thread_ranklist:{thread}:{metric}");
    let mut g = state.stats.ranklists.lock().await;
    if let Some((at, v)) = g.get(&key)
        && at.elapsed() < Duration::from_secs(30)
    {
        return v.clone();
    }
    let v = match try_get_bytes(state, &key).await {
        Ok(Some(b)) => serde_json::from_slice(&b).unwrap_or(Value::Null),
        Ok(None) => Value::Null,
        Err(_) => return g.get(&key).map_or(Value::Null, |(_, stale)| stale.clone()),
    };
    g.insert(key, (Instant::now(), v.clone()));
    v
}

async fn available_days(state: &AppState, thread: &str) -> Vec<String> {
    let index = lean_index(state).await;
    if thread == "all" {
        let mut set: BTreeSet<String> = BTreeSet::new();
        for t in index.as_object().into_iter().flatten().map(|x| x.1) {
            for d in t["days"].as_object().into_iter().flatten().map(|x| x.0) {
                set.insert(d.clone());
            }
        }
        return set.into_iter().collect();
    }
    let mut v: Vec<String> = index[thread]["days"]
        .as_object()
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    v.sort();
    v
}

pub fn resolve_days(
    keys: &[String],
    date: Option<&str>,
    start: Option<&str>,
    end: Option<&str>,
    all_days: bool,
) -> Vec<String> {
    if let Some(d) = date {
        return vec![d.to_owned()];
    }
    if start.is_none() && end.is_none() {
        return if all_days { keys.to_vec() } else { Vec::new() };
    }
    if keys.is_empty() {
        return Vec::new();
    }
    let mut s = start.map(str::to_owned);
    let mut e = end.map(str::to_owned);
    if s.is_none() && e.is_some() {
        s = Some(keys[0].clone());
    }
    if s.is_some() && e.is_none() {
        e = s.clone();
    }
    let (Some(s), Some(e)) = (s, e) else { return Vec::new() };
    keys.iter().filter(|k| **k >= s && **k <= e).cloned().collect()
}

fn is_date_key(d: &str) -> bool {
    let b = d.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

fn parse_entry(raw: Option<&Vec<u8>>, lean: bool) -> Option<Value> {
    let raw = raw.filter(|r| !r.is_empty())?;
    if packed::is_packed(raw) {
        return if lean {
            packed::decode_header(raw)
        } else {
            packed::decode(raw)
        }
        .map_err(|e| tracing::warn!("[stats-store] undecodable entry: {e}"))
        .ok();
    }
    serde_json::from_slice(raw).ok()
}

pub struct ThreadStatsOpts<'a> {
    pub date: Option<&'a str>,
    pub start: Option<&'a str>,
    pub end: Option<&'a str>,
    pub full: bool,
    pub all_days: bool,
    pub include_rollup: bool,
}

impl Default for ThreadStatsOpts<'_> {
    fn default() -> Self {
        ThreadStatsOpts {
            date: None,
            start: None,
            end: None,
            full: false,
            all_days: false,
            include_rollup: true,
        }
    }
}

pub async fn load_thread_stats(state: &AppState, thread: &str, o: ThreadStatsOpts<'_>) -> Map<String, Value> {
    let lean = !o.full;
    let has_filter = o.date.is_some() || o.start.is_some() || o.end.is_some();
    let needs_calendar = o.date.is_none() && (has_filter || o.all_days);
    let keys = if needs_calendar {
        available_days(state, thread).await
    } else {
        Vec::new()
    };
    let days: Vec<String> = resolve_days(&keys, o.date, o.start, o.end, o.all_days)
        .into_iter()
        .filter(|d| is_date_key(d))
        .collect();
    let wanted: HashSet<String> = days.iter().cloned().collect();
    let mut months: Vec<String> = days.iter().map(|d| d[..7].to_owned()).collect();
    months.sort();
    months.dedup();
    let data_keys: Vec<String> = if thread == "all" {
        days.iter().map(|d| format!("stats:all_day:{d}")).collect()
    } else {
        months
            .iter()
            .map(|m| format!("stats:thread_month:{thread}:{m}"))
            .collect()
    };
    let rollup_key = if thread == "all" {
        "stats:all".to_owned()
    } else if lean {
        format!("stats:thread_lean:{thread}")
    } else {
        format!("stats:thread:{thread}")
    };
    let mut all_keys = Vec::new();
    if o.include_rollup {
        all_keys.push(rollup_key);
    }
    all_keys.extend(data_keys);
    let values = mget(state, &all_keys).await;
    let off = usize::from(o.include_rollup);
    let mut out = Map::new();
    if o.include_rollup
        && let Some(r) = parse_entry(values.first().and_then(Option::as_ref), lean)
    {
        out.insert("all".into(), r);
    }
    if thread == "all" {
        for (i, d) in days.iter().enumerate() {
            if let Some(e) = parse_entry(values.get(i + off).and_then(Option::as_ref), lean) {
                out.insert(d.clone(), e);
            }
        }
    } else {
        for (i, m) in months.iter().enumerate() {
            let Some(raw) = values.get(i + off).and_then(Option::as_ref) else {
                continue;
            };
            if !packed::is_packed_month(raw) {
                continue;
            }
            match packed::decode_month(raw, m, lean, Some(&wanted)) {
                Ok(entries) => {
                    for (d, e) in entries {
                        out.insert(d, e);
                    }
                }
                Err(e) => tracing::warn!("[stats-store] undecodable month: {e}"),
            }
        }
    }
    if o.full && thread != "all" {
        let overrides = fake_overrides(state, thread).await;
        if !overrides.is_empty() {
            let markers: HashMap<String, String> = hgetall(state, &format!("stats:split_fake_by:{thread}"))
                .await
                .into_iter()
                .collect();
            for entry in out.values_mut() {
                apply_overrides(entry, &overrides, &markers);
            }
        }
    }
    out
}

fn field(s: &Value, e: &Value) -> String {
    format!("{}:{}", js_num_str(s), js_num_str(e))
}

fn js_num_str(v: &Value) -> String {
    match v {
        Value::Number(n) => counting::js::num_to_string(n.as_f64().unwrap_or(f64::NAN)),
        Value::Null => "undefined".into(),
        other => crate::http::js_string(other),
    }
}

pub async fn fake_overrides(state: &AppState, thread: &str) -> HashMap<String, bool> {
    hgetall(state, &format!("stats:split_fake:{thread}"))
        .await
        .into_iter()
        .map(|(k, v)| (k, v == "1"))
        .collect()
}

fn apply_overrides(entry: &mut Value, overrides: &HashMap<String, bool>, markers: &HashMap<String, String>) {
    let Some(recs) = entry.get_mut("splitSpeed").and_then(Value::as_array_mut) else {
        return;
    };
    for rec in recs {
        let f = field(&rec["startCountNumber"], &rec["endCountNumber"]);
        if let Some(v) = overrides.get(&f) {
            rec["isFake"] = json!(v);
            if *v && let Some(by) = markers.get(&f).filter(|b| !b.is_empty()) {
                rec["fakeMarkedBy"] = json!(by);
            }
        }
    }
}

pub async fn set_split_fake(
    state: &AppState,
    thread: &str,
    start: f64,
    end: f64,
    fake: bool,
    by: Option<&str>,
) -> anyhow::Result<()> {
    let f = format!(
        "{}:{}",
        counting::js::num_to_string(start),
        counting::js::num_to_string(end)
    );
    let mut pipe = redis::pipe();
    pipe.atomic()
        .cmd("HSET")
        .arg(format!("stats:split_fake:{thread}"))
        .arg(&f)
        .arg(if fake { "1" } else { "0" });
    match by.filter(|b| fake && !b.is_empty()) {
        Some(b) => pipe
            .cmd("HSET")
            .arg(format!("stats:split_fake_by:{thread}"))
            .arg(&f)
            .arg(b),
        None => pipe.cmd("HDEL").arg(format!("stats:split_fake_by:{thread}")).arg(&f),
    };
    pipe.query_async::<_, ()>(&mut conn(state)).await?;
    Ok(())
}

pub async fn split_fake_marker(state: &AppState, thread: &str, start: f64, end: f64) -> Value {
    let f = format!(
        "{}:{}",
        counting::js::num_to_string(start),
        counting::js::num_to_string(end)
    );
    let flag = hget(state, &format!("stats:split_fake:{thread}"), &f).await;
    let by = hget(state, &format!("stats:split_fake_by:{thread}"), &f).await;
    let mut out = Map::new();
    if let Some(fl) = flag {
        out.insert("isFake".into(), json!(fl == "1"));
    }
    if let Some(b) = by.filter(|b| !b.is_empty()) {
        out.insert("markedBy".into(), json!(b));
    }
    Value::Object(out)
}

pub async fn counter_days(state: &AppState, uuid: &str, kind: &str) -> Value {
    get_json(state, &format!("stats:{kind}:{uuid}"))
        .await
        .unwrap_or_else(|| json!({}))
}

pub async fn counter_stats(state: &AppState, uuid: &str) -> Option<Value> {
    let v = get_json(state, &format!("stats:counter:{uuid}")).await?;
    match v.get("threads") {
        Some(t) if !t.is_null() => Some(t.clone()),
        _ if v.is_null() => None,
        _ => Some(v),
    }
}
