use crate::regional::REGIONS;
use parking_lot::Mutex;
use serde_json::Value;
use sqlx::{MySqlPool, QueryBuilder};
use std::sync::Arc;
use std::time::Duration;

const FLUSH_EVERY: Duration = Duration::from_secs(5);
const MAX_BUFFERED: usize = 50_000;
const CLIENT_KINDS: [&str; 3] = ["ack", "ping", "probe"];

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Sample {
    pub at: i64,
    pub kind: &'static str,
    pub thread: Option<String>,
    pub region: Option<String>,
    pub colo: Option<String>,
    pub counter: Option<String>,
    pub epoch: Option<u64>,
    pub seq: Option<u64>,
    pub ms: Option<f64>,
    pub detail: Option<String>,
}

#[derive(Default, Clone)]
pub struct SampleLog {
    buffer: Arc<Mutex<Vec<Sample>>>,
}

impl SampleLog {
    pub fn record(&self, sample: Sample) {
        let mut buffer = self.buffer.lock();
        if buffer.len() < MAX_BUFFERED {
            buffer.push(sample);
        }
    }

    fn take(&self) -> Vec<Sample> {
        std::mem::take(&mut *self.buffer.lock())
    }
}

pub async fn start(pool: MySqlPool, log: SampleLog) -> anyhow::Result<()> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS cgg_regional_sample (id BIGINT UNSIGNED NOT NULL AUTO_INCREMENT PRIMARY KEY, \
         at BIGINT NOT NULL, kind VARCHAR(16) NOT NULL, thread VARCHAR(255) NULL, region VARCHAR(16) NULL, \
         colo VARCHAR(16) NULL, counter VARCHAR(64) NULL, epoch BIGINT UNSIGNED NULL, seq BIGINT UNSIGNED NULL, \
         ms DOUBLE NULL, detail VARCHAR(255) NULL, KEY kind_at (kind, at))",
    )
    .execute(&pool)
    .await?;
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(FLUSH_EVERY);
        loop {
            tick.tick().await;
            let batch = log.take();
            if !batch.is_empty()
                && let Err(e) = insert(&pool, &batch).await
            {
                tracing::warn!("regional samples: dropped {} after {e}", batch.len());
            }
        }
    });
    Ok(())
}

async fn insert(pool: &MySqlPool, batch: &[Sample]) -> sqlx::Result<()> {
    for chunk in batch.chunks(1000) {
        let mut q = QueryBuilder::new(
            "INSERT INTO cgg_regional_sample (at, kind, thread, region, colo, counter, epoch, seq, ms, detail) ",
        );
        q.push_values(chunk, |mut row, s| {
            row.push_bind(s.at)
                .push_bind(s.kind)
                .push_bind(&s.thread)
                .push_bind(&s.region)
                .push_bind(&s.colo)
                .push_bind(&s.counter)
                .push_bind(s.epoch)
                .push_bind(s.seq)
                .push_bind(s.ms)
                .push_bind(&s.detail);
        });
        q.build().execute(pool).await?;
    }
    Ok(())
}

pub fn from_client(data: &Value, counter: &str, now: i64) -> Option<Sample> {
    let kind = CLIENT_KINDS
        .iter()
        .find(|k| data.get("kind").and_then(Value::as_str) == Some(**k))?;
    let ms = data
        .get("ms")
        .and_then(Value::as_f64)
        .filter(|m| m.is_finite() && (0.0..60_000.0).contains(m))?;
    let region = data
        .get("region")
        .and_then(Value::as_str)
        .filter(|r| *r == "vps" || REGIONS.contains(r))?;
    let text = |key: &str, max: usize| {
        data.get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty() && s.len() <= max)
            .map(str::to_owned)
    };
    Some(Sample {
        at: now,
        kind,
        thread: text("thread", 255),
        region: Some(region.to_owned()),
        counter: Some(counter.to_owned()),
        ms: Some(ms),
        detail: text("detail", 64),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn client_samples_need_a_known_kind_region_and_sane_ms() {
        let ok = from_client(
            &json!({ "kind": "ack", "region": "weur", "ms": 21.5, "thread": "test2" }),
            "c",
            7,
        )
        .unwrap();
        assert_eq!(
            (ok.kind, ok.region.as_deref(), ok.ms, ok.at),
            ("ack", Some("weur"), Some(21.5), 7)
        );
        assert!(from_client(&json!({ "kind": "ping", "region": "vps", "ms": 15 }), "c", 7).is_some());
        assert!(from_client(&json!({ "kind": "nope", "region": "weur", "ms": 1 }), "c", 7).is_none());
        assert!(from_client(&json!({ "kind": "ack", "region": "mars", "ms": 1 }), "c", 7).is_none());
        assert!(from_client(&json!({ "kind": "ack", "region": "weur", "ms": -1 }), "c", 7).is_none());
        assert!(from_client(&json!({ "kind": "ack", "region": "weur", "ms": 90_000 }), "c", 7).is_none());
    }
}
