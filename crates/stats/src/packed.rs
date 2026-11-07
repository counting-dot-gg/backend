use ahash::AHashMap as HashMap;
use anyhow::{Result, bail};
use serde::Serialize;

use crate::models::{AccuracyEntry, SpeedEntry, SplitSpeedEntry, ThreadStats};

pub const MAGIC: &[u8; 4] = b"CGP2";

pub const ENC_RAW: u8 = 0;
pub const ENC_BROTLI: u8 = 1;

const COMPRESS_MIN_BYTES: usize = 64 * 1024;

fn put_uvarint(out: &mut Vec<u8>, mut n: u64) {
    while n >= 0x80 {
        out.push((n as u8) | 0x80);
        n >>= 7;
    }
    out.push(n as u8);
}

fn put_ivarint(out: &mut Vec<u8>, n: i64) {
    put_uvarint(out, ((n << 1) ^ (n >> 63)) as u64);
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Header<'a> {
    leaderboard: &'a HashMap<String, i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    gets: Option<&'a HashMap<String, i64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    assists: Option<&'a HashMap<String, i64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    palindromes: Option<&'a HashMap<String, i64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    repdigits: Option<&'a HashMap<String, i64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    accuracy: Option<&'a Vec<AccuracyEntry>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    time: Option<&'a HashMap<String, i64>>,
    speed_count: usize,
    split_speed_count: usize,
    last_updated: u64,
    last_updated_uuid: &'a str,
}

fn uuid_to_bytes(uuid: &str) -> Option<[u8; 16]> {
    let hex: Vec<u8> = uuid.bytes().filter(|b| *b != b'-').collect();
    if hex.len() != 32 {
        return None;
    }
    let mut out = [0u8; 16];
    for i in 0..16 {
        let hi = (hex[i * 2] as char).to_digit(16)?;
        let lo = (hex[i * 2 + 1] as char).to_digit(16)?;
        out[i] = ((hi << 4) | lo) as u8;
    }
    Some(out)
}

struct Dicts {
    counters: Vec<String>,
    counter_idx: HashMap<String, u64>,
    spans: Vec<i64>,
    span_idx: HashMap<i64, u64>,
}

impl Dicts {
    fn new() -> Self {
        Dicts {
            counters: Vec::new(),
            counter_idx: HashMap::new(),
            spans: Vec::new(),
            span_idx: HashMap::new(),
        }
    }
    fn counter(&mut self, uuid: &str) -> u64 {
        if let Some(i) = self.counter_idx.get(uuid) {
            return *i;
        }
        let i = self.counters.len() as u64;
        self.counters.push(uuid.to_string());
        self.counter_idx.insert(uuid.to_string(), i);
        i
    }
    fn span(&mut self, span: i64) -> u64 {
        if let Some(i) = self.span_idx.get(&span) {
            return *i;
        }
        let i = self.spans.len() as u64;
        self.spans.push(span);
        self.span_idx.insert(span, i);
        i
    }
}

pub fn encode(ts: &ThreadStats) -> Result<Vec<u8>> {
    let header = Header {
        leaderboard: &ts.leaderboard,
        gets: ts.gets.as_ref(),
        assists: ts.assists.as_ref(),
        palindromes: ts.palindromes.as_ref(),
        repdigits: ts.repdigits.as_ref(),
        accuracy: ts.accuracy.as_ref(),
        time: ts.time.as_ref(),
        speed_count: ts.speed.as_ref().map_or(0, |v| v.len()),
        split_speed_count: ts.split_speed.as_ref().map_or(0, |v| v.len()),
        last_updated: ts.last_updated,
        last_updated_uuid: &ts.last_updated_uuid,
    };
    let header_json = serde_json::to_vec(&header)?;

    let speed = ts.speed.as_deref().unwrap_or(&[]);
    let split = ts.split_speed.as_deref().unwrap_or(&[]);

    let mut d = Dicts::new();
    let mut speed_body = Vec::new();
    let mut prev: i64 = 0;
    put_uvarint(&mut speed_body, speed.len() as u64);
    for e in speed {
        put_ivarint(&mut speed_body, e.start_count_number - prev);
        prev = e.start_count_number;
        let span = d.span(e.end_count_number - e.start_count_number);
        put_uvarint(&mut speed_body, span);
        put_ivarint(&mut speed_body, e.time);
        put_uvarint(&mut speed_body, e.qualified_counters.len() as u64);
        for c in &e.qualified_counters {
            let i = d.counter(c);
            put_uvarint(&mut speed_body, i);
        }
    }

    let mut split_body = Vec::new();
    prev = 0;
    put_uvarint(&mut split_body, split.len() as u64);
    for e in split {
        put_ivarint(&mut split_body, e.start_count_number - prev);
        prev = e.start_count_number;
        let span = d.span(e.end_count_number - e.start_count_number);
        put_uvarint(&mut split_body, span);
        put_ivarint(&mut split_body, e.time);
        put_uvarint(&mut split_body, u64::from(e.is_fake));
        put_uvarint(&mut split_body, e.qualified_counters.len() as u64);
        for c in &e.qualified_counters {
            let i = d.counter(c);
            put_uvarint(&mut split_body, i);
        }
    }

    let mut out =
        Vec::with_capacity(header_json.len() + speed_body.len() + split_body.len() + d.counters.len() * 16 + 64);
    put_uvarint(&mut out, header_json.len() as u64);
    out.extend_from_slice(&header_json);

    put_uvarint(&mut out, d.counters.len() as u64);
    for c in &d.counters {
        match uuid_to_bytes(c) {
            Some(b) => {
                out.push(0);
                out.extend_from_slice(&b);
            }
            None => {
                out.push(1);
                put_uvarint(&mut out, c.len() as u64);
                out.extend_from_slice(c.as_bytes());
            }
        }
    }

    put_uvarint(&mut out, d.spans.len() as u64);
    for s in &d.spans {
        put_ivarint(&mut out, *s);
    }

    out.extend_from_slice(&speed_body);
    out.extend_from_slice(&split_body);

    let mut framed = Vec::with_capacity(out.len() + 5);
    framed.extend_from_slice(MAGIC);
    if out.len() >= COMPRESS_MIN_BYTES {
        let mut compressed = Vec::new();
        let params = brotli::enc::BrotliEncoderParams {
            quality: 5,
            size_hint: out.len(),
            ..Default::default()
        };
        brotli::BrotliCompress(&mut out.as_slice(), &mut compressed, &params)?;
        if compressed.len() < out.len() {
            framed.push(ENC_BROTLI);
            framed.extend_from_slice(&compressed);
            return Ok(framed);
        }
    }
    framed.push(ENC_RAW);
    framed.extend_from_slice(&out);
    Ok(framed)
}

pub fn is_packed(bytes: &[u8]) -> bool {
    bytes.len() >= 4 && &bytes[0..4] == MAGIC
}

struct R<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> R<'a> {
    fn u(&mut self) -> Result<u64> {
        let mut out: u64 = 0;
        let mut shift = 0;
        loop {
            if self.i >= self.b.len() {
                bail!("varint ran past end of buffer");
            }
            let byte = self.b[self.i];
            self.i += 1;
            out |= ((byte & 0x7f) as u64) << shift;
            if byte & 0x80 == 0 {
                return Ok(out);
            }
            shift += 7;
            if shift > 63 {
                bail!("varint too long");
            }
        }
    }
    fn i(&mut self) -> Result<i64> {
        let u = self.u()?;
        Ok(((u >> 1) as i64) ^ -((u & 1) as i64))
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.i + n > self.b.len() {
            bail!("read past end of buffer");
        }
        let s = &self.b[self.i..self.i + n];
        self.i += n;
        Ok(s)
    }
}

fn bytes_to_uuid(b: &[u8]) -> String {
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

pub fn decode(bytes: &[u8]) -> Result<ThreadStats> {
    if !is_packed(bytes) {
        bail!("not packed stats");
    }
    let inflated: Vec<u8>;
    let body: &[u8] = match bytes.get(4) {
        Some(&ENC_RAW) => &bytes[5..],
        Some(&ENC_BROTLI) => {
            let mut out = Vec::new();
            brotli::BrotliDecompress(&mut &bytes[5..], &mut out)?;
            inflated = out;
            &inflated
        }
        _ => bail!("unknown packed encoding flag"),
    };
    let mut r = R { b: body, i: 0 };

    let hlen = r.u()? as usize;
    let header_json = r.take(hlen)?;
    let hv: serde_json::Value = serde_json::from_slice(header_json)?;

    let n_counters = r.u()? as usize;
    let mut counters = Vec::with_capacity(n_counters);
    for _ in 0..n_counters {
        let tag = r.take(1)?[0];
        if tag == 0 {
            counters.push(bytes_to_uuid(r.take(16)?));
        } else {
            let l = r.u()? as usize;
            counters.push(String::from_utf8_lossy(r.take(l)?).into_owned());
        }
    }

    let n_spans = r.u()? as usize;
    let mut spans = Vec::with_capacity(n_spans);
    for _ in 0..n_spans {
        spans.push(r.i()?);
    }

    let read_counters = |r: &mut R| -> Result<Vec<String>> {
        let n = r.u()? as usize;
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            let idx = r.u()? as usize;
            v.push(counters.get(idx).cloned().unwrap_or_default());
        }
        Ok(v)
    };

    let n_speed = r.u()? as usize;
    let mut speed = Vec::with_capacity(n_speed);
    let mut prev: i64 = 0;
    for _ in 0..n_speed {
        let scn = prev + r.i()?;
        prev = scn;
        let span = *spans.get(r.u()? as usize).unwrap_or(&0);
        let time = r.i()?;
        let qc = read_counters(&mut r)?;
        speed.push(SpeedEntry {
            start: String::new(),
            end: String::new(),
            time,
            start_count: String::new(),
            end_count: String::new(),
            start_count_number: scn,
            end_count_number: scn + span,
            qualified_counters: qc,
        });
    }

    let n_split = r.u()? as usize;
    let mut split = Vec::with_capacity(n_split);
    prev = 0;
    for _ in 0..n_split {
        let scn = prev + r.i()?;
        prev = scn;
        let span = *spans.get(r.u()? as usize).unwrap_or(&0);
        let time = r.i()?;
        let is_fake = r.u()? == 1;
        let qc = read_counters(&mut r)?;
        split.push(SplitSpeedEntry {
            start: String::new(),
            end: String::new(),
            time,
            start_count: String::new(),
            end_count: String::new(),
            start_count_number: scn,
            end_count_number: scn + span,
            qualified_counters: qc,
            is_fake,
        });
    }

    let obj =
        |k: &str| -> Option<HashMap<String, i64>> { hv.get(k).and_then(|v| serde_json::from_value(v.clone()).ok()) };

    Ok(ThreadStats {
        leaderboard: obj("leaderboard").unwrap_or_default(),
        gets: obj("gets"),
        assists: obj("assists"),
        palindromes: obj("palindromes"),
        repdigits: obj("repdigits"),
        accuracy: hv.get("accuracy").and_then(|v| serde_json::from_value(v.clone()).ok()),
        time: obj("time"),
        speed: if speed.is_empty() { None } else { Some(speed) },
        split_speed: if split.is_empty() { None } else { Some(split) },
        last_updated: hv.get("lastUpdated").and_then(|v| v.as_u64()).unwrap_or(0),
        last_updated_uuid: hv
            .get("lastUpdatedUuid")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
    })
}

pub const MONTH_MAGIC: &[u8; 4] = b"CGM1";

const MONTH_COMPRESS_MIN_BYTES: usize = 2 * 1024;

pub fn is_packed_month(bytes: &[u8]) -> bool {
    bytes.len() >= 5 && &bytes[0..4] == MONTH_MAGIC
}

pub fn encode_month(days: &std::collections::BTreeMap<u32, ThreadStats>) -> Result<Vec<u8>> {
    let mut payload = Vec::new();
    put_uvarint(&mut payload, days.len() as u64);
    for (day, ts) in days {
        let entry = encode(ts)?;
        put_uvarint(&mut payload, *day as u64);
        put_uvarint(&mut payload, entry.len() as u64);
        payload.extend_from_slice(&entry);
    }

    let mut out = Vec::with_capacity(payload.len() + 5);
    out.extend_from_slice(MONTH_MAGIC);
    if payload.len() >= MONTH_COMPRESS_MIN_BYTES {
        let mut compressed = Vec::new();
        let params = brotli::enc::BrotliEncoderParams {
            quality: 5,
            size_hint: payload.len(),
            ..Default::default()
        };
        brotli::BrotliCompress(&mut payload.as_slice(), &mut compressed, &params)?;
        if compressed.len() < payload.len() {
            out.push(ENC_BROTLI);
            out.extend_from_slice(&compressed);
            return Ok(out);
        }
    }
    out.push(ENC_RAW);
    out.extend_from_slice(&payload);
    Ok(out)
}

pub fn decode_month(bytes: &[u8]) -> Result<std::collections::BTreeMap<u32, ThreadStats>> {
    if !is_packed_month(bytes) {
        bail!("not a packed month");
    }
    let inflated: Vec<u8>;
    let body: &[u8] = match bytes.get(4) {
        Some(&ENC_RAW) => &bytes[5..],
        Some(&ENC_BROTLI) => {
            let mut out = Vec::new();
            brotli::BrotliDecompress(&mut &bytes[5..], &mut out)?;
            inflated = out;
            &inflated
        }
        _ => bail!("unknown month encoding flag"),
    };

    let mut r = R { b: body, i: 0 };
    let n = r.u()? as usize;
    let mut days = std::collections::BTreeMap::new();
    for _ in 0..n {
        let day = r.u()? as u32;
        let len = r.u()? as usize;
        let entry = r.take(len)?;
        days.insert(day, decode(entry)?);
    }
    Ok(days)
}
