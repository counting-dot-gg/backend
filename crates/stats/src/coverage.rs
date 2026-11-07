use ahash::AHashMap as HashMap;
use ahash::AHashSet as HashSet;
use anyhow::Result;
use redis::aio::ConnectionManager;

pub const MS_PER_DAY: u32 = 86_400_000;
pub const SECS_PER_DAY: u32 = 86_400;

pub const MS_KEY: &str = "stats:ms_claimed";
pub const MS_CLAIMS_KEY: &str = "stats:ms_claims";

pub const CS_KEY: &str = "stats:cs_claimed";
pub const CS_CLAIMS_KEY: &str = "stats:cs_claims";

pub const CS_PER_DAY: u32 = 8_640_000;

pub fn seconds_key(counter_uuid: &str) -> String {
    format!("stats:counter_seconds:{counter_uuid}")
}

#[inline]
fn set_bit(bits: &mut [u8], index: u32) -> bool {
    let byte = (index >> 3) as usize;
    let mask = 1u8 << (7 - (index & 7));
    let was = bits[byte] & mask != 0;
    bits[byte] |= mask;
    !was
}

fn millipede_owners(owners: &[u16], names: &[String]) -> Vec<String> {
    let mut seen: HashSet<u16> = HashSet::new();
    for sec in 0..(SECS_PER_DAY as usize) {
        let base = sec * 1000;
        if base + 1000 > owners.len() {
            break;
        }
        let slot = &owners[base..base + 1000];
        if slot.contains(&NO_OWNER) {
            continue;
        }
        seen.extend(slot.iter().copied());
    }
    seen.into_iter().map(|o| names[o as usize].clone()).collect()
}

fn ending_slot(raw_count: &str) -> Option<u32> {
    if raw_count.is_empty() || !raw_count.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let tail = &raw_count[raw_count.len().saturating_sub(5)..];
    tail.parse::<u32>().ok()
}

pub const NO_OWNER: u16 = u16::MAX;

fn analyse_ownership(owners: &[u16], names: &[String]) {
    let mut runs: u64 = 0;
    let mut claims: u64 = 0;
    let mut longest: u32 = 0;
    let mut run_len: u32 = 0;
    let mut prev: u16 = NO_OWNER;

    let mut rle_bytes: u64 = 0;
    for &o in owners {
        if o == NO_OWNER {
            continue;
        }
        claims += 1;
        if o == prev && run_len > 0 {
            run_len += 1;
        } else {
            if run_len > 0 {
                longest = longest.max(run_len);
                rle_bytes += varint_len(prev as u64) + varint_len(run_len as u64);
            }
            runs += 1;
            run_len = 1;
            prev = o;
        }
    }
    if run_len > 0 {
        longest = longest.max(run_len);
        rle_bytes += varint_len(prev as u64) + varint_len(run_len as u64);
    }

    let mean_run = if runs > 0 { claims as f64 / runs as f64 } else { 0.0 };
    tracing::info!(
        claimants = names.len(),
        claims,
        runs,
        mean_run = format_args!("{mean_run:.2}"),
        longest_run = longest,
        flat_2b_mb = format_args!("{:.1}", (claims * 2) as f64 / 1_048_576.0),
        huffman_est_mb = format_args!("{:.1}", (claims as f64 * 3.831 / 8.0) / 1_048_576.0),
        rle_varint_mb = format_args!("{:.1}", rle_bytes as f64 / 1_048_576.0),
        "Ownership map: how compressible is it?"
    );
}

pub const MILLIPEDE_KEY: &str = "stats:millipede_counters";

pub const CLAIMS_BY_HOUR_KEY: &str = "stats:ms_claims_by_hour";

pub const OWNER_TABLE_KEY: &str = "stats:ms_owner_table";

pub fn owner_key(prefix: &str, hour: u32, minute: u32) -> String {
    format!("stats:{prefix}_owner:{hour}:{minute}")
}

pub fn owner_pending_key(prefix: &str, hour: u64, minute: u64) -> String {
    format!("stats:{prefix}_owner_pending:{hour}:{minute}")
}

fn pending_pos(idx: u64, slots_per_second: u64) -> (u64, u64, u64) {
    let per_minute = 60 * slots_per_second;
    let per_hour = 60 * per_minute;
    (idx / per_hour, (idx % per_hour) / per_minute, idx % per_minute)
}

async fn absorb_pending(
    redis: &mut ConnectionManager,
    prefix: &str,
    owners: &[u16],
    slots_per_second: u64,
) -> Result<()> {
    let per_minute = 60 * slots_per_second;
    let mut absorbed = 0usize;
    let mut kept = 0usize;

    for hour in 0..24u64 {
        for minute in 0..60u64 {
            let key = owner_pending_key(prefix, hour, minute);
            let fields: Vec<String> = redis::cmd("HKEYS").arg(&key).query_async(redis).await?;
            if fields.is_empty() {
                continue;
            }

            let base = (hour * 60 + minute) * per_minute;
            let mut covered: Vec<String> = Vec::new();
            for field in fields {
                let Ok(slot) = field.parse::<u64>() else { continue };
                let idx = (base + slot) as usize;
                if owners.get(idx).is_some_and(|&o| o != NO_OWNER) {
                    covered.push(field);
                } else {
                    kept += 1;
                }
            }

            if !covered.is_empty() {
                absorbed += covered.len();
                redis::cmd("HDEL")
                    .arg(&key)
                    .arg(&covered)
                    .query_async::<ConnectionManager, ()>(redis)
                    .await?;
            }
        }
    }

    tracing::info!(prefix, absorbed, kept, "Folded pending ownership into chunks");
    Ok(())
}

pub const MS_OWNER_PREFIX: &str = "ms";
pub const CS_OWNER_PREFIX: &str = "cs";
pub const CS_OWNER_TABLE_KEY: &str = "stats:cs_owner_table";

fn huffman_lengths(freqs: &[u64]) -> Vec<u8> {
    #[derive(Clone)]
    struct Node {
        weight: u64,
        symbol: Option<usize>,
        kids: Option<(usize, usize)>,
    }

    let mut arena: Vec<Node> = Vec::new();
    let mut live: Vec<usize> = Vec::new();
    for (sym, &f) in freqs.iter().enumerate() {
        if f == 0 {
            continue;
        }
        arena.push(Node {
            weight: f,
            symbol: Some(sym),
            kids: None,
        });
        live.push(arena.len() - 1);
    }

    let mut lengths = vec![0u8; freqs.len()];
    if live.is_empty() {
        return lengths;
    }
    if live.len() == 1 {
        lengths[arena[live[0]].symbol.unwrap()] = 1;
        return lengths;
    }

    live.sort_by_key(|&i| arena[i].weight);
    while live.len() > 1 {
        let a = live.remove(0);
        let b = live.remove(0);
        let w = arena[a].weight + arena[b].weight;
        arena.push(Node {
            weight: w,
            symbol: None,
            kids: Some((a, b)),
        });
        let node = arena.len() - 1;
        let at = live.partition_point(|&i| arena[i].weight <= w);
        live.insert(at, node);
    }

    let mut stack = vec![(live[0], 0u8)];
    while let Some((idx, depth)) = stack.pop() {
        match arena[idx].kids {
            Some((l, r)) => {
                stack.push((l, depth + 1));
                stack.push((r, depth + 1));
            }
            None => lengths[arena[idx].symbol.unwrap()] = depth.max(1),
        }
    }
    lengths
}

fn canonical_codes(lengths: &[u8]) -> Vec<(u32, u8)> {
    let max_len = lengths.iter().copied().max().unwrap_or(0);
    let mut out = vec![(0u32, 0u8); lengths.len()];
    let mut code: u32 = 0;
    for len in 1..=max_len {
        for (sym, &l) in lengths.iter().enumerate() {
            if l == len {
                out[sym] = (code, len);
                code += 1;
            }
        }
        code <<= 1;
    }
    out
}

struct BitWriter {
    out: Vec<u8>,
    cur: u8,
    bits: u8,
}

impl BitWriter {
    fn new() -> Self {
        BitWriter {
            out: Vec::new(),
            cur: 0,
            bits: 0,
        }
    }
    fn push(&mut self, code: u32, len: u8) {
        for i in (0..len).rev() {
            self.cur = (self.cur << 1) | (((code >> i) & 1) as u8);
            self.bits += 1;
            if self.bits == 8 {
                self.out.push(self.cur);
                self.cur = 0;
                self.bits = 0;
            }
        }
    }
    fn finish(mut self) -> Vec<u8> {
        if self.bits > 0 {
            self.cur <<= 8 - self.bits;
            self.out.push(self.cur);
        }
        self.out
    }
}

fn encode_ownership(owners: &[u16], names: &[String], slots_per_second: usize) -> (Vec<u8>, Vec<(u32, u32, Vec<u8>)>) {
    let mut freqs = vec![0u64; names.len()];
    for &o in owners {
        if o != NO_OWNER {
            freqs[o as usize] += 1;
        }
    }

    let lengths = huffman_lengths(&freqs);
    let codes = canonical_codes(&lengths);

    let table = serde_json::json!({
        "names": names,
        "lengths": lengths,
    });
    let table_bytes = serde_json::to_vec(&table).unwrap_or_default();

    let mut chunks = Vec::with_capacity(1440);
    for hour in 0..24u32 {
        for minute in 0..60u32 {
            let base = (hour as usize * 3600 + minute as usize * 60) * slots_per_second;
            let mut body: Vec<u8> = Vec::new();
            for sec in 0..60usize {
                let mut w = BitWriter::new();
                let start = base + sec * slots_per_second;
                for ms in start..start + slots_per_second {
                    let o = owners[ms];
                    if o == NO_OWNER {
                        continue;
                    }
                    let (code, len) = codes[o as usize];
                    w.push(code, len);
                }
                let bits = w.finish();
                put_uvarint(&mut body, bits.len() as u64);
                body.extend_from_slice(&bits);
            }
            chunks.push((hour, minute, body));
        }
    }
    (table_bytes, chunks)
}

fn varint_len(mut n: u64) -> u64 {
    let mut len = 1;
    while n >= 0x80 {
        n >>= 7;
        len += 1;
    }
    len
}

#[inline]
pub fn ms_of_day(unix_ms: i64) -> u32 {
    unix_ms.rem_euclid(MS_PER_DAY as i64) as u32
}

pub enum Coverage {
    Full {
        ms_bits: Vec<u8>,
        claims: HashMap<String, i64>,
        seconds: HashMap<String, Vec<u8>>,
        owners: Vec<u16>,
        owner_names: Vec<String>,
        owner_ids: HashMap<String, u16>,
        cs_bits: Vec<u8>,
        cs_claims: HashMap<String, i64>,
        cs_owners: Vec<u16>,
        endings: HashMap<String, Vec<u8>>,
    },
    Incremental {
        authors: Vec<String>,
        author_ids: HashMap<String, u32>,
        endings: HashMap<u32, HashSet<u32>>,
        touched: Vec<(u32, u32)>,
        seconds: HashMap<u32, HashSet<u32>>,
    },
}

impl Coverage {
    pub fn new(force_init: bool) -> Self {
        if force_init {
            Coverage::Full {
                ms_bits: vec![0u8; (MS_PER_DAY / 8) as usize],
                claims: HashMap::new(),
                seconds: HashMap::new(),
                owners: vec![NO_OWNER; MS_PER_DAY as usize],
                owner_names: Vec::new(),
                owner_ids: HashMap::new(),
                cs_bits: vec![0u8; (CS_PER_DAY / 8) as usize],
                cs_claims: HashMap::new(),
                cs_owners: vec![NO_OWNER; CS_PER_DAY as usize],
                endings: HashMap::new(),
            }
        } else {
            Coverage::Incremental {
                authors: Vec::new(),
                author_ids: HashMap::new(),
                endings: HashMap::new(),
                touched: Vec::new(),
                seconds: HashMap::new(),
            }
        }
    }

    pub fn observe(&mut self, unix_ms: i64, author_uuid: &str) {
        let ms = ms_of_day(unix_ms);
        let sec = ms / 1000;

        match self {
            Coverage::Full {
                ms_bits,
                claims,
                seconds,
                owners,
                owner_names,
                owner_ids,
                cs_bits,
                cs_claims,
                cs_owners,
                ..
            } => {
                let id = match owner_ids.get(author_uuid) {
                    Some(id) => *id,
                    None => {
                        let id = owner_names.len() as u16;
                        owner_names.push(author_uuid.to_string());
                        owner_ids.insert(author_uuid.to_string(), id);
                        id
                    }
                };

                if set_bit(cs_bits, ms / 10) {
                    *cs_claims.entry(author_uuid.to_string()).or_insert(0) += 1;
                    cs_owners[(ms / 10) as usize] = id;
                }
                if set_bit(ms_bits, ms) {
                    *claims.entry(author_uuid.to_string()).or_insert(0) += 1;
                    owners[ms as usize] = id;
                }
                let bits = seconds
                    .entry(author_uuid.to_string())
                    .or_insert_with(|| vec![0u8; (SECS_PER_DAY / 8) as usize]);
                set_bit(bits, sec);
            }
            Coverage::Incremental {
                authors,
                author_ids,
                touched,
                seconds,
                ..
            } => {
                let id = match author_ids.get(author_uuid) {
                    Some(id) => *id,
                    None => {
                        let id = authors.len() as u32;
                        authors.push(author_uuid.to_string());
                        author_ids.insert(author_uuid.to_string(), id);
                        id
                    }
                };
                touched.push((ms, id));
                seconds.entry(id).or_default().insert(sec);
            }
        }
    }

    pub fn observe_ending(&mut self, raw_count: &str, author_uuid: &str) {
        let Some(slot) = ending_slot(raw_count) else { return };

        match self {
            Coverage::Full { endings, .. } => {
                let bits = endings
                    .entry(author_uuid.to_string())
                    .or_insert_with(|| vec![0u8; (ENDINGS_DOMAIN / 8) as usize]);
                set_bit(bits, slot);
            }
            Coverage::Incremental {
                authors,
                author_ids,
                endings,
                ..
            } => {
                let id = match author_ids.get(author_uuid) {
                    Some(id) => *id,
                    None => {
                        let id = authors.len() as u32;
                        authors.push(author_uuid.to_string());
                        author_ids.insert(author_uuid.to_string(), id);
                        id
                    }
                };
                endings.entry(id).or_default().insert(slot);
            }
        }
    }

    pub async fn flush(self, redis: &mut ConnectionManager) -> Result<()> {
        match self {
            Coverage::Full {
                ms_bits,
                claims,
                seconds,
                owners,
                owner_names,
                cs_bits,
                cs_claims,
                cs_owners,
                endings,
                ..
            } => {
                let claimed: u64 = ms_bits.iter().map(|b| b.count_ones() as u64).sum();

                analyse_ownership(&owners, &owner_names);

                let mut by_hour: HashMap<&str, [u32; 24]> = HashMap::new();
                for (ms, &o) in owners.iter().enumerate() {
                    if o == NO_OWNER {
                        continue;
                    }
                    let hour = ms / 3_600_000;
                    by_hour.entry(owner_names[o as usize].as_str()).or_insert([0u32; 24])[hour] += 1;
                }
                let mut hour_pipe = redis::pipe();
                hour_pipe.del(CLAIMS_BY_HOUR_KEY).ignore();
                for (uuid, counts) in &by_hour {
                    let packed = counts.iter().map(|n| n.to_string()).collect::<Vec<_>>().join(",");
                    hour_pipe.hset(CLAIMS_BY_HOUR_KEY, *uuid, packed).ignore();
                }
                hour_pipe.query_async::<ConnectionManager, ()>(redis).await?;

                let millipede = millipede_owners(&owners, &owner_names);
                let mut mp = redis::pipe();
                mp.del(MILLIPEDE_KEY).ignore();
                for uuid in &millipede {
                    mp.sadd(MILLIPEDE_KEY, uuid.as_str()).ignore();
                }
                mp.query_async::<ConnectionManager, ()>(redis).await?;
                tracing::info!(
                    counters = millipede.len(),
                    "Counters owning a millisecond in a completed second"
                );

                let (cs_table, cs_chunks) = encode_ownership(&cs_owners, &owner_names, 100);
                let cs_bytes: usize = cs_chunks.iter().map(|(_, _, b)| b.len()).sum();
                let mut cs_pipe = redis::pipe();
                cs_pipe.set(CS_OWNER_TABLE_KEY, cs_table).ignore();
                for (hour, minute, body) in &cs_chunks {
                    cs_pipe
                        .set(owner_key(CS_OWNER_PREFIX, *hour, *minute), body.clone())
                        .ignore();
                }
                cs_pipe.query_async::<ConnectionManager, ()>(redis).await?;
                absorb_pending(redis, CS_OWNER_PREFIX, &cs_owners, 100).await?;
                tracing::info!(
                    chunks = cs_chunks.len(),
                    mb = format_args!("{:.1}", cs_bytes as f64 / 1_048_576.0),
                    "Wrote centisecond ownership"
                );

                let (table, chunks) = encode_ownership(&owners, &owner_names, 1000);
                let owner_bytes: usize = chunks.iter().map(|(_, _, b)| b.len()).sum();
                let mut pipe = redis::pipe();
                pipe.set(OWNER_TABLE_KEY, table).ignore();
                for (hour, minute, body) in &chunks {
                    pipe.set(owner_key(MS_OWNER_PREFIX, *hour, *minute), body.clone())
                        .ignore();
                }
                pipe.query_async::<ConnectionManager, ()>(redis).await?;
                absorb_pending(redis, MS_OWNER_PREFIX, &owners, 1000).await?;
                tracing::info!(
                    chunks = chunks.len(),
                    mb = format_args!("{:.1}", owner_bytes as f64 / 1_048_576.0),
                    "Wrote millisecond ownership"
                );

                let mut pipe = redis::pipe();
                let cs_claimed: u64 = cs_bits.iter().map(|b| b.count_ones() as u64).sum();
                pipe.set(CS_KEY, cs_bits).ignore();
                pipe.del(CS_CLAIMS_KEY).ignore();
                for (uuid, n) in &cs_claims {
                    pipe.hset(CS_CLAIMS_KEY, uuid, *n).ignore();
                }
                tracing::info!(
                    claimed = cs_claimed,
                    pct = format_args!("{:.2}%", cs_claimed as f64 / CS_PER_DAY as f64 * 100.0),
                    "Rebuilt centisecond coverage"
                );

                pipe.set(MS_KEY, ms_bits).ignore();
                pipe.del(MS_CLAIMS_KEY).ignore();
                for (uuid, n) in &claims {
                    pipe.hset(MS_CLAIMS_KEY, uuid, *n).ignore();
                }
                for (uuid, bits) in &endings {
                    let set: Vec<u32> = (0..ENDINGS_DOMAIN)
                        .filter(|s| bits[(s >> 3) as usize] & (1 << (7 - (s & 7))) != 0)
                        .collect();
                    pipe.set(endings_key(uuid), encode_endings(&set)).ignore();
                }
                tracing::info!(counters = endings.len(), "Wrote digit-ending coverage");

                for (uuid, bits) in &seconds {
                    let set: Vec<u32> = (0..SECS_PER_DAY)
                        .filter(|s| bits[(s >> 3) as usize] & (1 << (7 - (s & 7))) != 0)
                        .collect();
                    pipe.set(seconds_key(uuid), encode_seconds(&set)).ignore();
                }
                pipe.query_async::<ConnectionManager, ()>(redis).await?;

                tracing::info!(
                    claimed,
                    pct = format_args!("{:.2}%", claimed as f64 / MS_PER_DAY as f64 * 100.0),
                    counters = seconds.len(),
                    "Rebuilt time-of-day coverage"
                );
            }

            Coverage::Incremental {
                authors,
                touched,
                seconds,
                endings,
                ..
            } => {
                if touched.is_empty() {
                    return Ok(());
                }

                let mut pipe = redis::pipe();
                for (ms, _) in &touched {
                    pipe.cmd("SETBIT").arg(MS_KEY).arg(*ms).arg(1);
                }
                let prior: Vec<u8> = pipe.query_async(redis).await?;

                let mut cs_pipe = redis::pipe();
                for (ms, _) in &touched {
                    cs_pipe.cmd("SETBIT").arg(CS_KEY).arg(*ms / 10).arg(1);
                }
                let cs_prior: Vec<u8> = cs_pipe.query_async(redis).await?;

                let mut cs_claims: HashMap<&str, i64> = HashMap::new();
                for ((_, author_id), was_set) in touched.iter().zip(cs_prior.iter()) {
                    if *was_set == 0 {
                        *cs_claims.entry(authors[*author_id as usize].as_str()).or_insert(0) += 1;
                    }
                }
                let mut cs_credit = redis::pipe();
                for (author, n) in &cs_claims {
                    cs_credit
                        .cmd("HINCRBY")
                        .arg(CS_CLAIMS_KEY)
                        .arg(*author)
                        .arg(*n)
                        .ignore();
                }

                for ((ms, author_id), was_set) in touched.iter().zip(cs_prior.iter()) {
                    if *was_set != 0 {
                        continue;
                    }
                    let (hour, minute, slot) = pending_pos(*ms as u64 / 10, 100);
                    cs_credit
                        .cmd("HSET")
                        .arg(owner_pending_key(CS_OWNER_PREFIX, hour, minute))
                        .arg(slot)
                        .arg(authors[*author_id as usize].as_str())
                        .ignore();
                }
                cs_credit.query_async::<ConnectionManager, ()>(redis).await?;

                let mut claims: HashMap<&str, i64> = HashMap::new();
                for ((_, author_id), was_set) in touched.iter().zip(prior.iter()) {
                    if *was_set == 0 {
                        *claims.entry(authors[*author_id as usize].as_str()).or_insert(0) += 1;
                    }
                }

                let mut pipe = redis::pipe();
                for (author, n) in &claims {
                    pipe.cmd("HINCRBY").arg(MS_CLAIMS_KEY).arg(*author).arg(*n).ignore();
                }

                for ((ms, author_id), was_set) in touched.iter().zip(prior.iter()) {
                    if *was_set != 0 {
                        continue;
                    }
                    let (hour, minute, slot) = pending_pos(*ms as u64, 1000);
                    pipe.cmd("HSET")
                        .arg(owner_pending_key(MS_OWNER_PREFIX, hour, minute))
                        .arg(slot)
                        .arg(authors[*author_id as usize].as_str())
                        .ignore();
                }
                pipe.query_async::<ConnectionManager, ()>(redis).await?;

                for (author_id, secs) in &seconds {
                    let key = seconds_key(&authors[*author_id as usize]);
                    let existing: Option<Vec<u8>> = redis::cmd("GET").arg(&key).query_async(redis).await?;
                    let mut merged = existing.map(|b| decode_seconds(&b)).unwrap_or_default();
                    merged.extend(secs.iter().copied());
                    redis::cmd("SET")
                        .arg(&key)
                        .arg(encode_seconds(&merged))
                        .query_async::<ConnectionManager, ()>(redis)
                        .await?;
                }

                for (author_id, slots) in &endings {
                    let key = endings_key(&authors[*author_id as usize]);
                    let existing: Option<Vec<u8>> = redis::cmd("GET").arg(&key).query_async(redis).await?;
                    let mut merged = existing.map(|b| decode_seconds(&b)).unwrap_or_default();
                    merged.extend(slots.iter().copied());
                    redis::cmd("SET")
                        .arg(&key)
                        .arg(encode_endings(&merged))
                        .query_async::<ConnectionManager, ()>(redis)
                        .await?;
                }

                let newly: i64 = claims.values().sum();
                if newly > 0 {
                    tracing::info!(newly, touched = touched.len(), "Claimed new milliseconds");
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ms_of_day_wraps_at_midnight_utc() {
        assert_eq!(ms_of_day(0), 0);
        assert_eq!(ms_of_day(1), 1);
        assert_eq!(ms_of_day(MS_PER_DAY as i64), 0, "exactly one day on is midnight again");
        assert_eq!(ms_of_day(MS_PER_DAY as i64 - 1), MS_PER_DAY - 1);
        assert_eq!(ms_of_day(1_677_064_981_003), 40_981_003);
    }

    #[test]
    fn ms_of_day_handles_pre_epoch_without_going_negative() {
        assert_eq!(ms_of_day(-1), MS_PER_DAY - 1);
    }

    #[test]
    fn a_millisecond_is_claimed_by_whoever_reaches_it_first() {
        let mut c = Coverage::new(true);
        c.observe(1_677_064_981_003, "alice");
        c.observe(1_677_064_981_003 + MS_PER_DAY as i64, "bob");
        c.observe(1_677_064_981_004, "bob");

        let Coverage::Full { claims, .. } = &c else {
            panic!("expected full")
        };
        assert_eq!(claims.get("alice"), Some(&1), "alice was first to that slot");
        assert_eq!(claims.get("bob"), Some(&1), "bob got only the slot alice had not taken");
    }

    #[test]
    fn seconds_are_personal_not_a_land_grab() {
        let mut c = Coverage::new(true);
        c.observe(1_677_064_981_003, "alice");
        c.observe(1_677_064_981_900, "bob");

        let Coverage::Full { seconds, claims, .. } = &c else {
            panic!("expected full")
        };
        let sec = 40_981u32;
        for who in ["alice", "bob"] {
            let bits = &seconds[who];
            assert!(
                bits[(sec >> 3) as usize] & (1 << (7 - (sec & 7))) != 0,
                "{who} covered that second too — seconds are not exclusive"
            );
        }
        assert_eq!(claims.values().sum::<i64>(), 2, "two distinct ms slots claimed");
    }

    #[test]
    fn repeated_counts_in_one_slot_claim_it_once() {
        let mut c = Coverage::new(true);
        for _ in 0..50 {
            c.observe(1_677_064_981_003, "alice");
        }
        let Coverage::Full { claims, ms_bits, .. } = &c else {
            panic!("expected full")
        };
        assert_eq!(claims.get("alice"), Some(&1), "claimable once, not fifty times");
        let set: u64 = ms_bits.iter().map(|b| b.count_ones() as u64).sum();
        assert_eq!(set, 1);
    }

    #[test]
    fn incremental_defers_the_first_come_decision_to_redis() {
        let mut c = Coverage::new(false);
        c.observe(1_677_064_981_003, "alice");
        c.observe(1_677_064_981_003, "bob");
        let Coverage::Incremental { authors, touched, .. } = &c else {
            panic!("expected incremental")
        };
        assert_eq!(touched.len(), 2, "both are offered; Redis decides");
        assert_eq!(
            authors[touched[0].1 as usize], "alice",
            "earliest post is offered first"
        );
    }

    #[test]
    fn incremental_interns_authors_and_dedupes_seconds() {
        let mut c = Coverage::new(false);
        c.observe(1_677_064_981_003, "alice");
        c.observe(1_677_064_981_400, "alice");
        c.observe(1_677_064_981_900, "alice");
        c.observe(1_677_064_982_000, "bob");

        let Coverage::Incremental {
            authors,
            touched,
            seconds,
            ..
        } = &c
        else {
            panic!("expected incremental")
        };
        assert_eq!(authors.len(), 2, "one entry per distinct author, not per post");
        assert_eq!(touched.len(), 4, "every post still offers its millisecond");
        let alice = seconds.get(&0).expect("alice interned first");
        assert_eq!(alice.len(), 1, "three posts in one second are one covered second");
        assert_eq!(seconds.get(&1).map(|s| s.len()), Some(1), "bob covered his own");
    }

    #[test]
    fn bitmap_is_exactly_one_bit_per_slot() {
        let Coverage::Full { ms_bits, .. } = Coverage::new(true) else {
            panic!()
        };
        assert_eq!(ms_bits.len(), 10_800_000, "86.4M bits = 10.8MB");
        let mut c = Coverage::new(true);
        c.observe(1_677_064_981_003, "alice");
        let Coverage::Full { seconds, .. } = &c else { panic!() };
        assert_eq!(seconds["alice"].len(), 10_800, "86,400 bits = 10.8KB per counter");
    }
}

const ENC_SPARSE: u8 = 0;
const ENC_DENSE: u8 = 1;

const DENSE_BYTES: usize = (SECS_PER_DAY / 8) as usize;

fn put_uvarint(out: &mut Vec<u8>, mut n: u64) {
    while n >= 0x80 {
        out.push((n as u8) | 0x80);
        n >>= 7;
    }
    out.push(n as u8);
}

fn get_uvarint(b: &[u8], i: &mut usize) -> Option<u64> {
    let mut out = 0u64;
    let mut shift = 0;
    loop {
        let byte = *b.get(*i)?;
        *i += 1;
        out |= ((byte & 0x7f) as u64) << shift;
        if byte & 0x80 == 0 {
            return Some(out);
        }
        shift += 7;
        if shift > 63 {
            return None;
        }
    }
}

pub const ENDINGS_DOMAIN: u32 = 100_000;
const ENDINGS_DENSE_BYTES: usize = (ENDINGS_DOMAIN / 8) as usize;

pub fn endings_key(counter_uuid: &str) -> String {
    format!("stats:counter_endings:{counter_uuid}")
}

pub fn encode_seconds(secs: &[u32]) -> Vec<u8> {
    encode_slots(secs, DENSE_BYTES)
}

pub fn encode_endings(values: &[u32]) -> Vec<u8> {
    encode_slots(values, ENDINGS_DENSE_BYTES)
}

fn encode_slots(secs: &[u32], dense_bytes: usize) -> Vec<u8> {
    let mut sorted: Vec<u32> = secs.to_vec();
    sorted.sort_unstable();
    sorted.dedup();

    let mut sparse = Vec::with_capacity(sorted.len() + 2);
    sparse.push(ENC_SPARSE);
    put_uvarint(&mut sparse, sorted.len() as u64);
    let mut prev = 0u32;
    for s in &sorted {
        put_uvarint(&mut sparse, (s - prev) as u64);
        prev = *s;
    }
    if sparse.len() <= dense_bytes {
        return sparse;
    }

    let mut dense = vec![0u8; dense_bytes + 1];
    dense[0] = ENC_DENSE;
    for s in &sorted {
        dense[1 + (s >> 3) as usize] |= 1 << (7 - (s & 7));
    }
    dense
}

pub async fn load_counter_endings(redis: &mut ConnectionManager, counter_uuid: &str) -> Result<Vec<u32>> {
    let blob: Option<Vec<u8>> = redis::cmd("GET")
        .arg(endings_key(counter_uuid))
        .query_async(redis)
        .await?;
    Ok(blob.map(|b| decode_seconds(&b)).unwrap_or_default())
}

pub async fn load_counter_seconds(redis: &mut ConnectionManager, counter_uuid: &str) -> Result<Vec<u32>> {
    let blob: Option<Vec<u8>> = redis::cmd("GET")
        .arg(seconds_key(counter_uuid))
        .query_async(redis)
        .await?;
    Ok(blob.map(|b| decode_seconds(&b)).unwrap_or_default())
}

pub fn decode_seconds(bytes: &[u8]) -> Vec<u32> {
    match bytes.first() {
        Some(&ENC_SPARSE) => {
            let mut i = 1usize;
            let Some(n) = get_uvarint(bytes, &mut i) else {
                return Vec::new();
            };
            let mut out = Vec::with_capacity(n as usize);
            let mut acc = 0u32;
            for _ in 0..n {
                let Some(d) = get_uvarint(bytes, &mut i) else {
                    return out;
                };
                acc += d as u32;
                out.push(acc);
            }
            out
        }
        Some(&ENC_DENSE) => {
            let mut out = Vec::new();
            for (byte_i, byte) in bytes[1..].iter().enumerate() {
                if *byte == 0 {
                    continue;
                }
                for bit in 0..8 {
                    if byte & (1 << (7 - bit)) != 0 {
                        out.push((byte_i as u32) * 8 + bit);
                    }
                }
            }
            out
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod encoding_tests {
    use super::*;

    #[test]
    fn sparse_round_trips() {
        let secs = vec![0, 1, 2, 500, 35_581, 86_399];
        assert_eq!(decode_seconds(&encode_seconds(&secs)), secs);
    }

    #[test]
    fn a_handful_of_seconds_costs_a_handful_of_bytes() {
        let nine: Vec<u32> = (0..9).map(|i| 1000 + i * 37).collect();
        let enc = encode_seconds(&nine);
        assert_eq!(enc[0], ENC_SPARSE);
        assert!(enc.len() < 30, "nine seconds took {} bytes", enc.len());
        assert_eq!(decode_seconds(&enc), nine);
    }

    #[test]
    fn a_full_collection_switches_to_dense_and_still_round_trips() {
        let all: Vec<u32> = (0..SECS_PER_DAY).collect();
        let enc = encode_seconds(&all);
        assert_eq!(enc[0], ENC_DENSE, "86,400 deltas would be larger than the bitmap");
        assert_eq!(enc.len(), DENSE_BYTES + 1);
        assert_eq!(decode_seconds(&enc).len(), SECS_PER_DAY as usize);
    }

    #[test]
    fn encoding_picks_whichever_is_smaller() {
        let few: Vec<u32> = (0..500).map(|i| i * 7).collect();
        let many: Vec<u32> = (0..40_000).map(|i| i * 2).collect();
        assert_eq!(encode_seconds(&few)[0], ENC_SPARSE);
        assert_eq!(encode_seconds(&many)[0], ENC_DENSE);
        assert!(encode_seconds(&few).len() < DENSE_BYTES);
        assert_eq!(decode_seconds(&encode_seconds(&many)).len(), 40_000);
    }

    #[test]
    fn dense_bit_order_matches_redis_setbit() {
        let enc = encode_seconds(&[0]);
        assert_eq!(enc[0], ENC_SPARSE);
        let dense = encode_seconds(&(0..SECS_PER_DAY).step_by(2).collect::<Vec<u32>>());
        assert_eq!(dense[0], ENC_DENSE);
        assert_eq!(dense[1], 0xAA, "bit 0 must be the high bit of byte 0");
        assert_eq!(decode_seconds(&dense)[..3], [0, 2, 4]);
    }

    #[test]
    fn duplicates_and_disorder_are_normalised() {
        let messy = vec![500u32, 1, 500, 1, 86_399, 0];
        assert_eq!(decode_seconds(&encode_seconds(&messy)), vec![0, 1, 500, 86_399]);
    }

    #[test]
    fn empty_and_malformed_decode_to_nothing_rather_than_panicking() {
        assert!(decode_seconds(&[]).is_empty());
        assert!(decode_seconds(&[9, 9, 9]).is_empty());
        assert!(decode_seconds(&encode_seconds(&[])).is_empty());
        let mut enc = encode_seconds(&[1, 2, 3, 4, 5]);
        enc.truncate(3);
        let _ = decode_seconds(&enc);
    }
}

#[cfg(test)]
mod live_redis_tests {
    use super::*;

    async fn connect() -> Option<ConnectionManager> {
        let url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6382".to_string());
        let client = redis::Client::open(url).ok()?;
        ConnectionManager::new(client).await.ok()
    }

    #[tokio::test]
    #[ignore]
    async fn replaying_claimed_milliseconds_changes_nothing() {
        let Some(mut redis) = connect().await else {
            eprintln!("no redis; skipping");
            return;
        };

        let before: u64 = redis::cmd("BITCOUNT")
            .arg(MS_KEY)
            .query_async(&mut redis)
            .await
            .expect("bitcount");
        assert!(before > 0, "run --init first; nothing to replay against");

        let mut claimed = Vec::new();
        let mut probe = 0u32;
        while claimed.len() < 64 && probe < MS_PER_DAY {
            let bit: u8 = redis::cmd("GETBIT")
                .arg(MS_KEY)
                .arg(probe)
                .query_async(&mut redis)
                .await
                .expect("getbit");
            if bit == 1 {
                claimed.push(probe);
            }
            probe += 7919;
        }
        assert!(!claimed.is_empty(), "expected some claimed slots");

        let claims_before: Vec<(String, String)> = redis::cmd("HGETALL")
            .arg(MS_CLAIMS_KEY)
            .query_async(&mut redis)
            .await
            .expect("hgetall");

        let mut cov = Coverage::new(false);
        for ms in &claimed {
            cov.observe(*ms as i64, "replay-test-author");
        }
        cov.flush(&mut redis).await.expect("flush");

        let after: u64 = redis::cmd("BITCOUNT")
            .arg(MS_KEY)
            .query_async(&mut redis)
            .await
            .expect("bitcount");
        assert_eq!(after, before, "replay must not claim new milliseconds");

        let claims_after: Vec<(String, String)> = redis::cmd("HGETALL")
            .arg(MS_CLAIMS_KEY)
            .query_async(&mut redis)
            .await
            .expect("hgetall");
        assert!(
            !claims_after.iter().any(|(k, _)| k == "replay-test-author"),
            "an author who claimed nothing must not appear in the standings"
        );
        assert_eq!(claims_before.len(), claims_after.len(), "standings unchanged");

        let _: () = redis::cmd("DEL")
            .arg(seconds_key("replay-test-author"))
            .query_async(&mut redis)
            .await
            .expect("del");
    }

    #[tokio::test]
    #[ignore]
    async fn merging_into_a_stored_seconds_blob_only_ever_adds() {
        let Some(mut redis) = connect().await else {
            eprintln!("no redis; skipping");
            return;
        };

        let keys: Vec<String> = redis::cmd("KEYS")
            .arg("stats:counter_seconds:*")
            .query_async(&mut redis)
            .await
            .expect("keys");
        let Some(key) = keys.into_iter().next() else {
            eprintln!("no counter seconds stored; run --init first");
            return;
        };
        let uuid = key.rsplit(':').next().unwrap().to_string();

        let original: Vec<u8> = redis::cmd("GET").arg(&key).query_async(&mut redis).await.expect("get");
        let before = decode_seconds(&original);
        assert!(!before.is_empty(), "picked an empty counter");

        let fresh = (0..SECS_PER_DAY)
            .find(|s| !before.contains(s))
            .expect("counter covers the entire day; pick another");

        let mut cov = Coverage::new(false);
        cov.observe(before[0] as i64 * 1000, &uuid);
        cov.observe(fresh as i64 * 1000, &uuid);
        cov.flush(&mut redis).await.expect("flush");

        let updated: Vec<u8> = redis::cmd("GET").arg(&key).query_async(&mut redis).await.expect("get");
        let after = decode_seconds(&updated);

        for s in &before {
            assert!(after.contains(s), "second {s} was lost in the merge");
        }
        assert!(after.contains(&fresh), "the new second was not added");
        assert_eq!(after.len(), before.len() + 1, "exactly one second added");
        assert!(after.windows(2).all(|w| w[0] < w[1]), "must stay sorted");

        let _: () = redis::cmd("SET")
            .arg(&key)
            .arg(original)
            .query_async(&mut redis)
            .await
            .expect("restore");
    }
}

#[cfg(test)]
mod ownership_tests {
    use super::*;

    #[test]
    fn canonical_codes_are_prefix_free() {
        let freqs = vec![100u64, 50, 25, 12, 6, 3, 1];
        let lengths = huffman_lengths(&freqs);
        let codes = canonical_codes(&lengths);

        for (a, &(ca, la)) in codes.iter().enumerate() {
            if la == 0 {
                continue;
            }
            for (b, &(cb, lb)) in codes.iter().enumerate() {
                if a == b || lb == 0 || la > lb {
                    continue;
                }
                assert_ne!(cb >> (lb - la), ca, "code {a} is a prefix of code {b}");
            }
        }
    }

    #[test]
    fn frequent_symbols_get_shorter_codes() {
        let freqs = vec![1u64, 1_000_000, 10, 100_000];
        let lengths = huffman_lengths(&freqs);
        assert!(lengths[1] <= lengths[3], "the most common symbol must be cheapest");
        assert!(lengths[3] <= lengths[2]);
        assert!(lengths[2] <= lengths[0]);
    }

    #[test]
    fn unused_symbols_get_no_code() {
        let freqs = vec![10u64, 0, 5, 0];
        let lengths = huffman_lengths(&freqs);
        assert_eq!(lengths[1], 0);
        assert_eq!(lengths[3], 0);
        assert!(lengths[0] > 0 && lengths[2] > 0);
    }

    #[test]
    fn a_single_claimant_still_gets_one_bit() {
        let lengths = huffman_lengths(&[42u64]);
        assert_eq!(lengths[0], 1);
    }

    #[test]
    fn ownership_round_trips_through_the_chunks() {
        let mut owners = vec![NO_OWNER; MS_PER_DAY as usize];
        let names: Vec<String> = ["alice", "bob", "carol"].iter().map(|s| s.to_string()).collect();

        let mut expected: Vec<(usize, u16)> = Vec::new();
        for (i, ms) in (0..MS_PER_DAY as usize).step_by(9_973).enumerate() {
            let owner = if i % 7 == 0 {
                2
            } else if i % 3 == 0 {
                1
            } else {
                0
            };
            owners[ms] = owner;
            expected.push((ms, owner));
        }
        owners[61_000] = 0;
        owners[61_001] = 2;

        let (table, chunks) = encode_ownership(&owners, &names, 1000);
        assert!(!table.is_empty());
        assert_eq!(chunks.len(), 1440);

        let parsed: serde_json::Value = serde_json::from_slice(&table).expect("table json");
        let lengths: Vec<u8> = parsed["lengths"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap() as u8)
            .collect();
        let codes = canonical_codes(&lengths);

        for (hour, minute, body) in &chunks {
            let base = ((hour * 3600 + minute * 60) * 1000) as usize;
            let mut i = 0usize;
            for sec in 0..60usize {
                let len = get_uvarint(body, &mut i).expect("chunk length") as usize;
                let bits = &body[i..i + len];
                i += len;

                let mut bit_pos = 0usize;
                let start = base + sec * 1000;
                for ms in start..start + 1000 {
                    if owners[ms] == NO_OWNER {
                        continue;
                    }
                    let mut acc: u32 = 0;
                    let mut acc_len: u8 = 0;
                    let decoded = loop {
                        let byte = bits[bit_pos >> 3];
                        let bit = (byte >> (7 - (bit_pos & 7))) & 1;
                        bit_pos += 1;
                        acc = (acc << 1) | bit as u32;
                        acc_len += 1;
                        if let Some(sym) = codes.iter().position(|&(c, l)| l == acc_len && c == acc) {
                            break sym as u16;
                        }
                        assert!(acc_len < 32, "no code matched");
                    };
                    assert_eq!(decoded, owners[ms], "wrong owner at ms {ms}");
                }
            }
        }
    }
}

#[cfg(test)]
mod millipede_tests {
    use super::*;

    fn names() -> Vec<String> {
        vec!["alice".to_string(), "bob".to_string(), "carol".to_string()]
    }

    #[test]
    fn an_incomplete_second_credits_nobody() {
        let mut owners = vec![NO_OWNER; MS_PER_DAY as usize];
        for o in owners[0..999].iter_mut() {
            *o = 0;
        }
        assert!(millipede_owners(&owners, &names()).is_empty());
    }

    #[test]
    fn a_complete_second_credits_every_owner_in_it() {
        let mut owners = vec![NO_OWNER; MS_PER_DAY as usize];
        for (i, o) in owners[0..1000].iter_mut().enumerate() {
            *o = if i == 500 { 1 } else { 0 };
        }
        owners[5_000] = 2;

        let mut got = millipede_owners(&owners, &names());
        got.sort();
        assert_eq!(got, vec!["alice".to_string(), "bob".to_string()]);
        assert!(!got.contains(&"carol".to_string()), "carol's second is not full");
    }

    #[test]
    fn owning_a_single_millisecond_of_a_full_second_qualifies() {
        let mut owners = vec![NO_OWNER; MS_PER_DAY as usize];
        let base = 42 * 1000;
        for o in owners[base..base + 1000].iter_mut() {
            *o = 0;
        }
        owners[base + 999] = 2;
        let mut got = millipede_owners(&owners, &names());
        got.sort();
        assert_eq!(got, vec!["alice".to_string(), "carol".to_string()]);
    }
}

#[cfg(test)]
mod centisecond_tests {
    use super::*;

    #[test]
    fn a_centisecond_covers_ten_milliseconds() {
        let mut c = Coverage::new(true);
        c.observe(0, "alice");
        c.observe(3, "bob");
        c.observe(9, "carol");
        c.observe(10, "bob");

        let Coverage::Full { claims, cs_claims, .. } = &c else {
            panic!("expected full")
        };
        assert_eq!(claims.values().sum::<i64>(), 4);
        assert_eq!(cs_claims.get("alice"), Some(&1));
        assert_eq!(cs_claims.get("bob"), Some(&1), "bob took centisecond 1");
        assert_eq!(cs_claims.get("carol"), None, "carol's centisecond was taken");
        assert_eq!(cs_claims.values().sum::<i64>(), 2);
    }

    #[test]
    fn centisecond_bitmap_is_one_bit_per_slot() {
        let Coverage::Full { cs_bits, .. } = Coverage::new(true) else {
            panic!()
        };
        assert_eq!(cs_bits.len(), (CS_PER_DAY / 8) as usize);
        assert_eq!(cs_bits.len(), 1_080_000, "8.64M bits is 1.08MB");
    }

    #[test]
    fn the_last_centisecond_of_the_day_is_in_range() {
        let mut c = Coverage::new(true);
        c.observe(MS_PER_DAY as i64 - 1, "alice");
        let Coverage::Full { cs_bits, .. } = &c else { panic!() };
        let last = CS_PER_DAY - 1;
        assert!(
            cs_bits[(last >> 3) as usize] & (1 << (7 - (last & 7))) != 0,
            "the final millisecond belongs to the final centisecond"
        );
    }
}

#[cfg(test)]
mod endings_tests {
    use super::*;

    #[test]
    fn only_the_last_five_digits_count() {
        let mut c = Coverage::new(true);
        c.observe_ending("1234567890", "alice");
        c.observe_ending("67890", "bob");

        let Coverage::Full { endings, .. } = &c else {
            panic!("expected full")
        };
        let slot = 67_890u32;
        for who in ["alice", "bob"] {
            let bits = &endings[who];
            assert!(
                bits[(slot >> 3) as usize] & (1 << (7 - (slot & 7))) != 0,
                "{who} should hold ending 67890"
            );
        }
    }

    #[test]
    fn a_number_and_the_same_number_plus_100000_share_a_slot() {
        let mut c = Coverage::new(true);
        c.observe_ending("1", "alice");
        c.observe_ending("100001", "bob");
        c.observe_ending("900001", "carol");

        let Coverage::Full { endings, .. } = &c else { panic!() };
        let slot = 1u32;
        for who in ["alice", "bob", "carol"] {
            let bits = &endings[who];
            assert!(
                bits[(slot >> 3) as usize] & (1 << (7 - (slot & 7))) != 0,
                "{who} should hold the 00001 slot"
            );
            let set: u32 = bits.iter().map(|b| b.count_ones()).sum();
            assert_eq!(set, 1, "{who} should hold exactly one ending");
        }
    }

    #[test]
    fn short_counts_land_on_themselves() {
        let cases = [
            ("0", 0u32),
            ("7", 7),
            ("300", 300),
            ("00300", 300),
            ("100300", 300),
            ("999300", 99_300),
            ("99999", 99_999),
            ("100000", 0),
            ("4294967296", 67_296),
        ];
        for (raw, want) in cases {
            assert_eq!(ending_slot(raw), Some(want), "{raw} should map to {want}");
        }
    }

    #[test]
    fn every_value_below_100000_is_its_own_slot() {
        let mut c = Coverage::new(true);
        for n in 0..1000u32 {
            c.observe_ending(&n.to_string(), "alice");
        }
        let Coverage::Full { endings, .. } = &c else { panic!() };
        let set: u32 = endings["alice"].iter().map(|b| b.count_ones()).sum();
        assert_eq!(set, 1000, "1,000 distinct values should take 1,000 slots");
        for n in [0u32, 1, 42, 300, 999] {
            assert!(
                endings["alice"][(n >> 3) as usize] & (1 << (7 - (n & 7))) != 0,
                "slot {n} should be held"
            );
        }
    }

    #[test]
    fn non_decimal_counts_are_ignored() {
        let mut c = Coverage::new(true);
        for value in ["MMMMDCLV", "FWA", "1F4A9", "12:34", "-5", "", "1,000", "3.14", "0x1F"] {
            c.observe_ending(value, "alice");
        }
        let Coverage::Full { endings, .. } = &c else { panic!() };
        assert!(endings.get("alice").is_none(), "no slot should have been taken");
    }

    #[test]
    fn the_displayed_value_decides_the_slot() {
        let mut c = Coverage::new(true);
        c.observe_ending("100000", "alice");
        let Coverage::Full { endings, .. } = &c else { panic!() };
        let bits = &endings["alice"];
        assert!(bits[0] & 0x80 != 0, "100000 ends in 00000");
        let set: u32 = bits.iter().map(|b| b.count_ones()).sum();
        assert_eq!(set, 1);
    }

    #[test]
    fn absurdly_long_counts_still_resolve() {
        let mut c = Coverage::new(true);
        c.observe_ending("999999999999999999999999912345", "alice");
        let Coverage::Full { endings, .. } = &c else { panic!() };
        let slot = 12_345u32;
        assert!(endings["alice"][(slot >> 3) as usize] & (1 << (7 - (slot & 7))) != 0);
    }

    #[test]
    fn endings_are_not_a_land_grab() {
        let mut c = Coverage::new(true);
        c.observe_ending("500", "alice");
        c.observe_ending("100500", "bob");

        let Coverage::Full { endings, .. } = &c else { panic!() };
        assert_eq!(endings.len(), 2, "both counters recorded it");
    }

    #[test]
    fn the_bitmap_covers_the_whole_domain() {
        let mut c = Coverage::new(true);
        c.observe_ending("99999", "alice");
        let Coverage::Full { endings, .. } = &c else { panic!() };
        assert_eq!(endings["alice"].len(), 12_500, "100,000 bits is 12,500 bytes");
        let last = 99_999u32;
        assert!(endings["alice"][(last >> 3) as usize] & (1 << (7 - (last & 7))) != 0);
    }

    #[test]
    fn endings_use_the_same_hybrid_encoding_as_seconds() {
        let all: Vec<u32> = (0..ENDINGS_DOMAIN).collect();
        let dense = encode_endings(&all);
        assert_eq!(dense[0], 1, "a full set is dense");
        assert_eq!(dense.len(), 12_501);
        assert_eq!(decode_seconds(&dense).len(), ENDINGS_DOMAIN as usize);

        let few: Vec<u32> = (0..40).map(|i| i * 37).collect();
        let sparse = encode_endings(&few);
        assert_eq!(sparse[0], 0, "a handful stays sparse");
        assert!(sparse.len() < 100, "40 endings took {} bytes", sparse.len());
        assert_eq!(decode_seconds(&sparse), few);
    }

    #[test]
    fn incremental_collects_distinct_endings() {
        let mut c = Coverage::new(false);
        c.observe_ending("12345", "alice");
        c.observe_ending("112345", "alice");
        c.observe_ending("12346", "alice");
        c.observe_ending("12345", "bob");

        let Coverage::Incremental { authors, endings, .. } = &c else {
            panic!()
        };
        assert_eq!(authors.len(), 2);
        assert_eq!(endings[&0].len(), 2, "alice reached two distinct endings");
        assert_eq!(endings[&1].len(), 1);
    }
}
