use crate::app::{App, now_ms};
use dashmap::DashMap;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

static LOCAL: LazyLock<DashMap<String, Arc<tokio::sync::Mutex<()>>>> = LazyLock::new(DashMap::new);

pub async fn run<T>(app: &App, prefix: &str, key: &str, f: impl Future<Output = T>) -> T {
    let full = format!("{prefix}:{key}");
    let m = LOCAL
        .entry(full.clone())
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone();
    let guard = m.lock().await;
    let token = format!("{}-{}-{}", std::process::id(), now_ms(), fastrand::u64(..));
    let mut redis = app.redis.clone();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut held = false;
    loop {
        let r: Result<Option<String>, _> = redis::cmd("SET")
            .arg(&full)
            .arg(&token)
            .arg("PX")
            .arg(10_000)
            .arg("NX")
            .query_async(&mut redis)
            .await;
        match r {
            Ok(Some(_)) => {
                held = true;
                break;
            }
            Ok(None) if Instant::now() < deadline => tokio::time::sleep(Duration::from_millis(5)).await,
            Ok(None) => {
                tracing::warn!("[lock] {full} not acquired in 5000ms; running unlocked");
                break;
            }
            Err(e) => {
                tracing::warn!("[lock] {full} unavailable; running unlocked: {e}");
                break;
            }
        }
    }
    let out = f.await;
    if held {
        let _: Result<i64, _> = redis::Script::new(
            "if redis.call('get', KEYS[1]) == ARGV[1] then return redis.call('del', KEYS[1]) else return 0 end",
        )
        .key(&full)
        .arg(&token)
        .invoke_async(&mut redis)
        .await;
    }
    drop(guard);
    LOCAL.remove_if(&full, |_, v| Arc::strong_count(v) <= 2);
    out
}
