use super::store::{get_bytes, hget, hgetall};
use crate::entity::js_number;
use crate::state::AppState;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

pub const MS_PER_DAY: f64 = 86_400_000.0;
pub const SECS_PER_DAY: f64 = 86_400.0;
pub const CS_PER_DAY: f64 = 8_640_000.0;
const MS_PER_HOUR: i64 = 3_600_000;
const CS_PER_HOUR: i64 = 360_000;
const BYTES_PER_SECOND: i64 = 125;
const BYTES_PER_HOUR: i64 = MS_PER_HOUR / 8;
pub const ENDINGS_TOTAL: f64 = 100_000.0;

#[derive(Default)]
pub struct CoverageCaches {
    ms: tokio::sync::Mutex<Option<(Instant, Value)>>,
    cs: tokio::sync::Mutex<Option<(Instant, Value)>>,
}

fn conn(state: &AppState) -> crate::redis_conn::Redis {
    state.redis.clone()
}

async fn bitcounts(state: &AppState, key: &str, per_hour: i64) -> Result<(f64, Vec<f64>), redis::RedisError> {
    let mut pipe = redis::pipe();
    pipe.cmd("BITCOUNT").arg(key);
    for h in 0..24 {
        let start = h * per_hour / 8;
        pipe.cmd("BITCOUNT").arg(key).arg(start).arg(start + per_hour / 8 - 1);
    }
    let r: Vec<Option<i64>> = pipe.query_async(&mut conn(state)).await?;
    let at = |i: usize| r.get(i).copied().flatten().unwrap_or(0) as f64;
    Ok((at(0), (0..24).map(|h| at(h + 1)).collect()))
}

async fn coverage(state: &AppState, key: &str, per_hour: i64, total: f64) -> Result<Value, redis::RedisError> {
    let (claimed, hours) = bitcounts(state, key, per_hour).await?;
    let by_hour: Vec<Value> = hours
        .iter()
        .enumerate()
        .map(|(h, c)| json!({ "hour": h, "claimed": js_number(*c), "pct": js_number(c / per_hour as f64 * 100.0) }))
        .collect();
    Ok(
        json!({ "claimed": js_number(claimed), "total": js_number(total), "pct": js_number(claimed / total * 100.0), "byHour": by_hour }),
    )
}

pub async fn ms_coverage(state: &AppState) -> Result<Value, redis::RedisError> {
    let mut g = state.coverage.ms.lock().await;
    if let Some((at, v)) = g.as_ref()
        && at.elapsed() < Duration::from_secs(60)
    {
        return Ok(v.clone());
    }
    let v = coverage(state, "stats:ms_claimed", MS_PER_HOUR, MS_PER_DAY).await?;
    *g = Some((Instant::now(), v.clone()));
    Ok(v)
}

pub async fn cs_coverage(state: &AppState) -> Result<Value, redis::RedisError> {
    let mut g = state.coverage.cs.lock().await;
    if let Some((at, v)) = g.as_ref()
        && at.elapsed() < Duration::from_secs(60)
    {
        return Ok(v.clone());
    }
    let v = coverage(state, "stats:cs_claimed", CS_PER_HOUR, CS_PER_DAY).await?;
    *g = Some((Instant::now(), v.clone()));
    Ok(v)
}

async fn getrange(state: &AppState, key: &str, start: i64, end: i64) -> Vec<u8> {
    redis::cmd("GETRANGE")
        .arg(key)
        .arg(start)
        .arg(end)
        .query_async::<_, Vec<u8>>(&mut conn(state))
        .await
        .unwrap_or_default()
}

fn hour_ok(h: f64) -> bool {
    h.fract() == 0.0 && (0.0..=23.0).contains(&h)
}

fn second_ok(s: f64) -> bool {
    s.fract() == 0.0 && (0.0..=3599.0).contains(&s)
}

pub async fn second_counts(state: &AppState, hour: f64) -> Option<Vec<u32>> {
    if !hour_ok(hour) {
        return None;
    }
    let start = hour as i64 * BYTES_PER_HOUR;
    let slice = getrange(state, "stats:ms_claimed", start, start + BYTES_PER_HOUR - 1).await;
    let mut out = vec![0u32; 3600];
    for (sec, o) in out.iter_mut().enumerate() {
        let base = sec * BYTES_PER_SECOND as usize;
        if base >= slice.len() {
            break;
        }
        let end = (base + BYTES_PER_SECOND as usize).min(slice.len());
        *o = slice[base..end].iter().map(|b| b.count_ones()).sum();
    }
    Some(out)
}

pub async fn ms_bits(state: &AppState, hour: f64, second: f64) -> Option<Value> {
    if !hour_ok(hour) || !second_ok(second) {
        return None;
    }
    let start = hour as i64 * BYTES_PER_HOUR + second as i64 * BYTES_PER_SECOND;
    let buf = getrange(state, "stats:ms_claimed", start, start + BYTES_PER_SECOND - 1).await;
    let claimed: u32 = buf.iter().map(|b| b.count_ones()).sum();
    Some(json!({ "blob": STANDARD.encode(&buf), "claimed": claimed }))
}

pub async fn cs_second_counts(state: &AppState, hour: f64) -> Option<Vec<u32>> {
    if !hour_ok(hour) {
        return None;
    }
    let start = hour as i64 * CS_PER_HOUR / 8;
    let slice = getrange(state, "stats:cs_claimed", start, start + CS_PER_HOUR / 8 - 1).await;
    let mut out = vec![0u32; 3600];
    if slice.is_empty() {
        return Some(out);
    }
    for (sec, o) in out.iter_mut().enumerate() {
        let mut n = 0;
        for i in 0..100 {
            let bit = sec * 100 + i;
            if slice.get(bit >> 3).is_some_and(|b| b & (1 << (7 - (bit & 7))) != 0) {
                n += 1;
            }
        }
        *o = n;
    }
    Some(out)
}

pub async fn cs_bits(state: &AppState, hour: f64, second: f64) -> Option<Value> {
    if !hour_ok(hour) || !second_ok(second) {
        return None;
    }
    let first = hour as i64 * CS_PER_HOUR + second as i64 * 100;
    let start = first / 8;
    let end = (first + 99) / 8;
    let buf = getrange(state, "stats:cs_claimed", start, end).await;
    let offset = (first - start * 8) as usize;
    let mut out = [0u8; 13];
    let mut claimed = 0;
    for i in 0..100usize {
        let bit = offset + i;
        if buf.get(bit >> 3).is_some_and(|b| b & (1 << (7 - (bit & 7))) != 0) {
            out[i >> 3] |= 1 << (7 - (i & 7));
            claimed += 1;
        }
    }
    Some(json!({ "blob": STANDARD.encode(out), "claimed": claimed }))
}

fn or0(s: &str) -> f64 {
    let n = crate::http::js_to_number(s);
    if n.is_nan() { 0.0 } else { n }
}

pub async fn claim_leaderboard(state: &AppState, key: &str, limit: usize) -> Vec<(String, f64)> {
    let mut v: Vec<(String, f64)> = hgetall(state, key)
        .await
        .into_iter()
        .map(|(k, n)| (k, or0(&n)))
        .collect();
    v.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    v.truncate(limit);
    v
}

pub async fn counter_claims(state: &AppState, key: &str, uuid: &str) -> f64 {
    hget(state, key, uuid).await.map_or(0.0, |s| or0(&s))
}

pub async fn claims_by_hour(state: &AppState, uuid: &str) -> Option<Vec<f64>> {
    let raw = hget(state, "stats:ms_claims_by_hour", uuid)
        .await
        .filter(|r| !r.is_empty())?;
    let parts: Vec<f64> = raw.split(',').map(or0).collect();
    (parts.len() == 24).then_some(parts)
}

pub fn decode_seconds(buf: Option<&[u8]>) -> Vec<u64> {
    let Some(buf) = buf.filter(|b| !b.is_empty()) else {
        return Vec::new();
    };
    match buf[0] {
        0 => {
            let mut out = Vec::new();
            let mut i = 1;
            let mut uvarint = || -> Option<u64> {
                let mut val = 0u64;
                let mut shift = 0;
                loop {
                    let byte = *buf.get(i)?;
                    i += 1;
                    val += u64::from(byte & 0x7f) << shift;
                    if byte & 0x80 == 0 {
                        return Some(val);
                    }
                    shift += 7;
                    if shift > 35 {
                        return None;
                    }
                }
            };
            let Some(n) = uvarint() else { return out };
            let mut acc = 0;
            for _ in 0..n {
                let Some(d) = uvarint() else { return out };
                acc += d;
                out.push(acc);
            }
            out
        }
        1 => {
            let mut out = Vec::new();
            for (b, byte) in buf.iter().enumerate().skip(1) {
                if *byte == 0 {
                    continue;
                }
                for bit in 0..8 {
                    if byte & (1 << (7 - bit)) != 0 {
                        out.push(((b - 1) * 8 + bit) as u64);
                    }
                }
            }
            out
        }
        _ => Vec::new(),
    }
}

pub async fn counter_seconds(state: &AppState, uuid: &str, include: bool) -> Value {
    let buf = get_bytes(state, &format!("stats:counter_seconds:{uuid}")).await;
    let secs = decode_seconds(buf.as_deref());
    let mut by_hour = vec![0u32; 24];
    for s in &secs {
        if let Some(h) = by_hour.get_mut((*s / 3600) as usize) {
            *h += 1;
        }
    }
    let mut out = json!({
        "covered": secs.len(),
        "total": js_number(SECS_PER_DAY),
        "pct": js_number(secs.len() as f64 / SECS_PER_DAY * 100.0),
        "byHour": by_hour,
    });
    if let Some(b) = &buf {
        out["blob"] = json!(STANDARD.encode(b));
    }
    if include {
        out["seconds"] = json!(secs);
    }
    out
}

pub async fn counter_endings(state: &AppState, uuid: &str) -> Value {
    let buf = get_bytes(state, &format!("stats:counter_endings:{uuid}")).await;
    let vals = decode_seconds(buf.as_deref());
    let mut by_block = vec![0u32; 100];
    for v in &vals {
        if (*v as f64) < ENDINGS_TOTAL {
            by_block[(*v / 1000) as usize] += 1;
        }
    }
    json!({
        "covered": vals.len(),
        "total": js_number(ENDINGS_TOTAL),
        "pct": js_number(vals.len() as f64 / ENDINGS_TOTAL * 100.0),
        "blob": buf.map_or(Value::Null, |b| json!(STANDARD.encode(b))),
        "byBlock": by_block,
    })
}

pub async fn sitewide_counting_seconds(state: &AppState) -> Value {
    let counted = hgetall(state, "stats:seconds_by_counter").await;
    let posted = hgetall(state, "stats:posted_seconds_by_counter").await;
    let sum = |v: &[(String, String)]| v.iter().map(|(_, n)| or0(n)).sum::<f64>();
    json!({ "seconds": js_number(sum(&counted)), "postedSeconds": js_number(sum(&posted)), "counters": posted.len() })
}
