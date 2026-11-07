use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::client::Client;
use crate::env::{Target, ThreadSpec, seed};
use crate::server::Socket;

#[derive(Clone, Copy)]
pub struct Load {
    pub threads: usize,
    pub watchers: usize,
    pub seconds: u64,
}

pub type Metrics = BTreeMap<String, f64>;

#[derive(Serialize, Deserialize)]
pub struct Report {
    pub label: String,
    pub rounds: Vec<Metrics>,
    pub median: Metrics,
}

pub async fn round(bin: &Path, target: &Target, workdir: &Path, load: Load) -> Result<Metrics> {
    target.reset().await?;
    let pool = target.pool().await?;
    let names: Vec<String> = (0..load.threads.max(1)).map(|i| format!("b_{i}")).collect();
    let specs: Vec<ThreadSpec> = names
        .iter()
        .map(|n| ThreadSpec {
            name: n,
            allow_double_posts: true,
        })
        .collect();
    let accounts = seed(&pool, target, &specs, names.len() + load.watchers).await?;
    let socket = Socket::start(bin, target, workdir, false).await?;

    let mut watchers = Vec::new();
    for (i, acc) in accounts[names.len()..].iter().enumerate() {
        let mut w = Client::connect(socket.port, Some(&acc.cookie)).await?;
        w.watch(&names[i % names.len()]).await?;
        watchers.push(w.drain());
    }

    let mut m = Metrics::new();
    let warmup = closed_loop(socket.port, &accounts[0].cookie, &names[0], 1, Duration::from_secs(1)).await?;
    let single = closed_loop(
        socket.port,
        &accounts[0].cookie,
        &names[0],
        warmup.next,
        Duration::from_secs(load.seconds),
    )
    .await?;
    m.insert("single.p50_ms".into(), percentile(&single.latencies_ms, 0.50));
    m.insert("single.p99_ms".into(), percentile(&single.latencies_ms, 0.99));

    let started = Instant::now();
    let mut runs = Vec::new();
    for (i, name) in names.iter().enumerate() {
        let (cookie, name) = (accounts[i].cookie.clone(), name.clone());
        let first = if i == 0 { single.next } else { 1 };
        let port = socket.port;
        let seconds = load.seconds;
        runs.push(tokio::spawn(async move {
            closed_loop(port, &cookie, &name, first, Duration::from_secs(seconds)).await
        }));
    }
    let mut all = Vec::new();
    for r in runs {
        all.extend(r.await??.latencies_ms);
    }
    let elapsed = started.elapsed().as_secs_f64();
    m.insert("parallel.posts_per_s".into(), all.len() as f64 / elapsed);
    m.insert("parallel.p50_ms".into(), percentile(&all, 0.50));
    m.insert("parallel.p99_ms".into(), percentile(&all, 0.99));

    let expected = (warmup.latencies_ms.len() + single.latencies_ms.len() + all.len()) as i64;
    let drain_started = Instant::now();
    loop {
        let stored: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM post").fetch_one(&pool).await?;
        if stored >= expected {
            break;
        }
        ensure!(
            drain_started.elapsed() < Duration::from_secs(60),
            "only {stored} of {expected} posts reached MySQL"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    m.insert(
        "parallel.persist_drain_ms".into(),
        drain_started.elapsed().as_secs_f64() * 1000.0,
    );
    for w in watchers {
        w.abort();
    }
    drop(socket);
    Ok(m)
}

struct Run {
    latencies_ms: Vec<f64>,
    next: i64,
}

async fn closed_loop(port: u16, cookie: &str, thread: &str, first: i64, duration: Duration) -> Result<Run> {
    let mut c = Client::connect(port, Some(cookie)).await?;
    c.watch(thread).await?;
    let mut latencies_ms = Vec::new();
    let mut n = first;
    let started = Instant::now();
    while started.elapsed() < duration {
        let text = n.to_string();
        let sent = Instant::now();
        c.post(thread, &text)?;
        let seen = c.own_post(&text, Duration::from_secs(10)).await?;
        latencies_ms.push((seen.at - sent).as_secs_f64() * 1000.0);
        n += 1;
    }
    Ok(Run { latencies_ms, next: n })
}

pub fn percentile(values: &[f64], p: f64) -> f64 {
    if values.is_empty() {
        return f64::NAN;
    }
    let mut v = values.to_vec();
    v.sort_by(f64::total_cmp);
    v[((v.len() - 1) as f64 * p).round() as usize]
}

pub fn median_of(rounds: &[Metrics]) -> Metrics {
    let mut out = Metrics::new();
    if let Some(first) = rounds.first() {
        for key in first.keys() {
            let values: Vec<f64> = rounds.iter().filter_map(|r| r.get(key).copied()).collect();
            out.insert(key.clone(), percentile(&values, 0.5));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles_pick_the_nearest_rank() {
        let v = [5.0, 1.0, 3.0, 2.0, 4.0];
        assert_eq!(percentile(&v, 0.5), 3.0);
        assert_eq!(percentile(&v, 0.99), 5.0);
        assert!(percentile(&[], 0.5).is_nan());
    }

    #[test]
    fn median_is_taken_per_metric() {
        let r = |a: f64| Metrics::from([("x".to_string(), a)]);
        assert_eq!(median_of(&[r(3.0), r(1.0), r(2.0)])["x"], 2.0);
    }
}
