use super::store::{get_bytes, hgetall};
use crate::state::AppState;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::time::{Duration, Instant};

#[derive(Clone, Copy)]
pub enum Unit {
    Ms,
    Cs,
}

impl Unit {
    fn table_key(self) -> &'static str {
        match self {
            Unit::Ms => "stats:ms_owner_table",
            Unit::Cs => "stats:cs_owner_table",
        }
    }

    fn prefix(self) -> &'static str {
        match self {
            Unit::Ms => "ms",
            Unit::Cs => "cs",
        }
    }

    fn bitmap(self) -> &'static str {
        match self {
            Unit::Ms => "stats:ms_claimed",
            Unit::Cs => "stats:cs_claimed",
        }
    }

    fn per_second(self) -> usize {
        match self {
            Unit::Ms => 1000,
            Unit::Cs => 100,
        }
    }
}

#[derive(Clone)]
pub struct Table {
    names: Vec<String>,
    codes: Vec<HashMap<u64, usize>>,
    max_len: usize,
}

#[derive(Default)]
pub struct OwnerCaches {
    ms: tokio::sync::Mutex<Option<(Instant, Table)>>,
    cs: tokio::sync::Mutex<Option<(Instant, Table)>>,
}

fn build(names: Vec<String>, lengths: &[usize]) -> Table {
    let max_len = lengths.iter().copied().max().unwrap_or(0);
    let mut codes: Vec<HashMap<u64, usize>> = vec![HashMap::new(); max_len + 1];
    let mut code = 0u64;
    for len in 1..=max_len {
        for (sym, l) in lengths.iter().enumerate() {
            if *l == len {
                codes[len].insert(code, sym);
                code += 1;
            }
        }
        code <<= 1;
    }
    Table { names, codes, max_len }
}

async fn table(state: &AppState, unit: Unit) -> Option<Table> {
    let slot = match unit {
        Unit::Ms => &state.owners.ms,
        Unit::Cs => &state.owners.cs,
    };
    let mut g = slot.lock().await;
    if let Some((at, t)) = g.as_ref()
        && at.elapsed() < Duration::from_secs(300)
    {
        return Some(t.clone());
    }
    let raw = get_bytes(state, unit.table_key()).await?;
    let parsed: Value = serde_json::from_slice(&raw).ok()?;
    let names: Vec<String> = parsed["names"].as_array()?.iter().map(crate::http::js_string).collect();
    let lengths: Vec<usize> = parsed["lengths"]
        .as_array()?
        .iter()
        .map(|l| l.as_u64().unwrap_or(0) as usize)
        .collect();
    let t = build(names, &lengths);
    *g = Some((Instant::now(), t.clone()));
    Some(t)
}

async fn getrange(state: &AppState, key: &str, start: usize, end: usize) -> Vec<u8> {
    redis::cmd("GETRANGE")
        .arg(key)
        .arg(start)
        .arg(end)
        .query_async::<_, Vec<u8>>(&mut state.redis.clone())
        .await
        .unwrap_or_default()
}

pub async fn second_owners(state: &AppState, hour: f64, second: f64, unit: Unit) -> Option<Value> {
    if !(hour.fract() == 0.0
        && (0.0..=23.0).contains(&hour)
        && second.fract() == 0.0
        && (0.0..=3599.0).contains(&second))
    {
        return None;
    }
    let (h, sec) = (hour as usize, second as usize);
    let per = unit.per_second();
    let t = table(state, unit).await?;
    let minute = sec / 60;
    let in_minute = sec % 60;
    let first_bit = (h * 3600 + sec) * per;
    let start = first_bit / 8;
    let end = (first_bit + per - 1) / 8;
    let claimed = getrange(state, unit.bitmap(), start, end).await;
    let chunk = get_bytes(state, &format!("stats:{}_owner:{h}:{minute}", unit.prefix()))
        .await
        .filter(|c| !c.is_empty())?;
    let pending_raw = hgetall(state, &format!("stats:{}_owner_pending:{h}:{minute}", unit.prefix())).await;
    let base = in_minute * per;
    let mut pending: HashMap<usize, String> = HashMap::new();
    for (slot, uuid) in pending_raw {
        if let Ok(slot) = slot.parse::<i64>() {
            let off = slot - base as i64;
            if off >= 0 && (off as usize) < per {
                pending.insert(off as usize, uuid);
            }
        }
    }
    let bit_offset = first_bit - start * 8;
    let mut i = 0usize;
    let mut bits: Option<&[u8]> = None;
    for s in 0..=in_minute {
        let mut len = 0usize;
        let mut shift = 0u32;
        loop {
            let byte = *chunk.get(i)?;
            i += 1;
            len += ((byte & 0x7f) as usize) << shift;
            if byte & 0x80 == 0 {
                break;
            }
            shift += 7;
        }
        if s == in_minute {
            bits = Some(&chunk[i.min(chunk.len())..(i + len).min(chunk.len())]);
        }
        i += len;
    }
    let bits = bits?;
    let mut owners: Vec<Value> = vec![Value::Null; per];
    let mut counts: Vec<(String, u64)> = Vec::new();
    let add = |name: &str, counts: &mut Vec<(String, u64)>| match counts.iter_mut().find(|c| c.0 == name) {
        Some(c) => c.1 += 1,
        None => counts.push((name.to_owned(), 1)),
    };
    let mut bit_pos = 0usize;
    for (ms, owner) in owners.iter_mut().enumerate() {
        let bit = bit_offset + ms;
        let is_claimed = claimed.get(bit >> 3).is_some_and(|b| b & (1 << (7 - (bit & 7))) != 0);
        if !is_claimed {
            continue;
        }
        if let Some(fresh) = pending.get(&ms) {
            *owner = json!(fresh);
            add(fresh, &mut counts);
            continue;
        }
        let (mut acc, mut acc_len, mut sym) = (0u64, 0usize, None);
        while acc_len <= t.max_len {
            let Some(byte) = bits.get(bit_pos >> 3) else { break };
            let code_bit = u64::from((byte >> (7 - (bit_pos & 7))) & 1);
            bit_pos += 1;
            acc = (acc << 1) | code_bit;
            acc_len += 1;
            if let Some(hit) = t.codes.get(acc_len).and_then(|m| m.get(&acc)) {
                sym = Some(*hit);
                break;
            }
        }
        let Some(sym) = sym else { break };
        if let Some(name) = t.names.get(sym).filter(|n| !n.is_empty()) {
            *owner = json!(name);
            add(name, &mut counts);
        }
    }
    counts.sort_by_key(|c| std::cmp::Reverse(c.1));
    let tally: Vec<Value> = counts
        .into_iter()
        .map(|(u, c)| json!({ "counterUuid": u, "claims": c }))
        .collect();
    Some(json!({ "hour": h, "second": sec, "owners": owners, "tally": tally }))
}
