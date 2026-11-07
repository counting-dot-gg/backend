use anyhow::{Result, anyhow, bail};
use serde_json::{Map, Value, json};
use std::collections::HashSet;

const MAGIC: &[u8; 4] = b"CGP2";
const MONTH_MAGIC: &[u8; 4] = b"CGM1";

pub fn is_packed(b: &[u8]) -> bool {
    b.len() >= 5 && &b[..4] == MAGIC
}

pub fn is_packed_month(b: &[u8]) -> bool {
    b.len() >= 5 && &b[..4] == MONTH_MAGIC
}

fn body(b: &[u8]) -> Result<Vec<u8>> {
    match b[4] {
        0 => Ok(b[5..].to_vec()),
        1 => {
            let mut out = Vec::new();
            brotli::BrotliDecompress(&mut &b[5..], &mut out)?;
            Ok(out)
        }
        e => bail!("unknown packed encoding flag {e}"),
    }
}

struct Reader<'a> {
    b: &'a [u8],
    i: usize,
}

impl Reader<'_> {
    fn u(&mut self) -> Result<f64> {
        let mut out = 0f64;
        let mut shift = 0i32;
        loop {
            let byte = *self.b.get(self.i).ok_or_else(|| anyhow!("varint past end of buffer"))?;
            self.i += 1;
            out += f64::from(byte & 0x7f) * 2f64.powi(shift);
            if byte & 0x80 == 0 {
                return Ok(out);
            }
            shift += 7;
            if shift > 63 {
                bail!("varint too long");
            }
        }
    }

    fn i64(&mut self) -> Result<f64> {
        let u = self.u()?;
        let half = (u / 2.0).floor();
        Ok(if u % 2.0 == 0.0 { half } else { -half - 1.0 })
    }

    fn take(&mut self, n: usize) -> Result<&[u8]> {
        if self.i + n > self.b.len() {
            bail!("read past end of buffer");
        }
        let s = &self.b[self.i..self.i + n];
        self.i += n;
        Ok(s)
    }
}

fn uuid_of(b: &[u8]) -> String {
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

fn num(f: f64) -> Value {
    crate::entity::js_number(f)
}

fn header_fields(header: &Value, out: &mut Map<String, Value>) {
    out.insert(
        "leaderboard".into(),
        header
            .get("leaderboard")
            .filter(|v| !v.is_null())
            .cloned()
            .unwrap_or(json!({})),
    );
    for k in ["gets", "assists", "palindromes", "repdigits", "accuracy", "time"] {
        if let Some(v) = header.get(k).filter(|v| crate::routes::user::truthy(v)) {
            out.insert(k.into(), v.clone());
        }
    }
}

fn or_default(header: &Value, k: &str, d: Value) -> Value {
    header.get(k).filter(|v| !v.is_null()).cloned().unwrap_or(d)
}

pub fn decode_header(buf: &[u8]) -> Result<Value> {
    if !is_packed(buf) {
        bail!("not packed stats");
    }
    let b = body(buf)?;
    let mut r = Reader { b: &b, i: 0 };
    let n = r.u()? as usize;
    let header: Value = serde_json::from_slice(r.take(n)?)?;
    let mut out = Map::new();
    header_fields(&header, &mut out);
    out.insert("speedCount".into(), or_default(&header, "speedCount", json!(0)));
    out.insert(
        "splitSpeedCount".into(),
        or_default(&header, "splitSpeedCount", json!(0)),
    );
    out.insert("lastUpdated".into(), or_default(&header, "lastUpdated", json!(0)));
    out.insert(
        "lastUpdatedUuid".into(),
        or_default(&header, "lastUpdatedUuid", json!("")),
    );
    Ok(Value::Object(out))
}

pub fn decode(buf: &[u8]) -> Result<Value> {
    if !is_packed(buf) {
        bail!("not packed stats");
    }
    let b = body(buf)?;
    let mut r = Reader { b: &b, i: 0 };
    let hl = r.u()? as usize;
    let header: Value = serde_json::from_slice(r.take(hl)?)?;
    let nc = r.u()? as usize;
    let mut counters = Vec::with_capacity(nc);
    for _ in 0..nc {
        let tag = r.take(1)?[0];
        counters.push(if tag == 0 {
            uuid_of(r.take(16)?)
        } else {
            let n = r.u()? as usize;
            String::from_utf8_lossy(r.take(n)?).into_owned()
        });
    }
    let ns = r.u()? as usize;
    let mut spans = Vec::with_capacity(ns);
    for _ in 0..ns {
        spans.push(r.i64()?);
    }
    let block = |r: &mut Reader<'_>, with_fake: bool| -> Result<Vec<Value>> {
        let n = r.u()? as usize;
        let mut out = Vec::with_capacity(n);
        let mut prev = 0f64;
        for _ in 0..n {
            let start = prev + r.i64()?;
            prev = start;
            let span = spans.get(r.u()? as usize).copied().unwrap_or(0.0);
            let time = r.i64()? / 1000.0;
            let fake = if with_fake { Some(r.u()? == 1.0) } else { None };
            let qn = r.u()? as usize;
            let mut q = Vec::with_capacity(qn);
            for _ in 0..qn {
                q.push(json!(counters.get(r.u()? as usize).cloned().unwrap_or_default()));
            }
            let mut rec = Map::new();
            rec.insert("startCountNumber".into(), num(start));
            rec.insert("endCountNumber".into(), num(start + span));
            rec.insert("time".into(), num(time));
            rec.insert("qualifiedCounters".into(), Value::Array(q));
            if let Some(f) = fake {
                rec.insert("isFake".into(), json!(f));
            }
            out.push(Value::Object(rec));
        }
        Ok(out)
    };
    let speed = block(&mut r, false)?;
    let split = block(&mut r, true)?;
    let mut out = Map::new();
    header_fields(&header, &mut out);
    out.insert(
        "speedCount".into(),
        or_default(&header, "speedCount", json!(speed.len())),
    );
    out.insert(
        "splitSpeedCount".into(),
        or_default(&header, "splitSpeedCount", json!(split.len())),
    );
    if !speed.is_empty() {
        out.insert("speed".into(), Value::Array(speed));
    }
    if !split.is_empty() {
        out.insert("splitSpeed".into(), Value::Array(split));
    }
    out.insert("lastUpdated".into(), or_default(&header, "lastUpdated", json!(0)));
    out.insert(
        "lastUpdatedUuid".into(),
        or_default(&header, "lastUpdatedUuid", json!("")),
    );
    Ok(Value::Object(out))
}

pub fn decode_month(
    buf: &[u8],
    month: &str,
    lean: bool,
    wanted: Option<&HashSet<String>>,
) -> Result<Vec<(String, Value)>> {
    if !is_packed_month(buf) {
        bail!("not a packed month");
    }
    let b = body(buf)?;
    let mut r = Reader { b: &b, i: 0 };
    let n = r.u()? as usize;
    let mut out = Vec::new();
    for _ in 0..n {
        let dom = r.u()? as u32;
        let len = r.u()? as usize;
        let day = format!("{month}-{dom:02}");
        let entry = r.take(len)?;
        if wanted.is_some_and(|w| !w.contains(&day)) {
            continue;
        }
        out.push((day, if lean { decode_header(entry)? } else { decode(entry)? }));
    }
    Ok(out)
}
