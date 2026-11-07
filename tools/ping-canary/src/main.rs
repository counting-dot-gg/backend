use axum::Router;
use serde_json::{Value, json};
use socketioxide::SocketIo;
use socketioxide::extract::{SocketRef, TryData};
use std::time::{Duration, Instant};

const TICK: Duration = Duration::from_millis(50);
const REPORT: Duration = Duration::from_secs(30);

#[tokio::main]
async fn main() {
    let addr = std::env::var("CANARY_ADDR").unwrap_or_else(|_| "127.0.0.1:8899".into());
    let path = std::env::var("CANARY_PATH").unwrap_or_else(|_| "/socket.io".into());
    let (layer, io) = SocketIo::builder().req_path(path.clone()).build_layer();
    io.ns("/", async |s: SocketRef| {
        s.on("latency_ping", async |s: SocketRef, TryData::<Value>(d): TryData<Value>| {
            let t = d.ok().and_then(|d| d.get("t").filter(|t| t.is_number()).cloned()).unwrap_or(Value::Null);
            s.emit("latency_pong", &json!({ "t": t })).ok();
        });
    });
    tokio::spawn(wake_lag());
    let listener = tokio::net::TcpListener::bind(&addr).await.expect("bind");
    println!("{} ping-canary on {addr}{path} (latency_ping only)", stamp());
    axum::serve(listener, Router::new().layer(layer)).await.expect("serve");
}

async fn wake_lag() {
    let mut lags: Vec<f64> = Vec::new();
    let mut since = Instant::now();
    loop {
        let t0 = Instant::now();
        tokio::time::sleep(TICK).await;
        lags.push((t0.elapsed().saturating_sub(TICK)).as_secs_f64() * 1000.0);
        if since.elapsed() >= REPORT {
            lags.sort_by(|a, b| a.total_cmp(b));
            let q = |p: f64| lags[((lags.len() as f64 * p) as usize).min(lags.len() - 1)];
            println!(
                "{} wake lag n={} p50={:.1}ms p99={:.1}ms max={:.1}ms over 20ms={}",
                stamp(),
                lags.len(),
                q(0.5),
                q(0.99),
                lags[lags.len() - 1],
                lags.iter().filter(|l| **l > 20.0).count()
            );
            lags.clear();
            since = Instant::now();
        }
    }
}

fn stamp() -> String {
    let s = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) % 86_400;
    format!("{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
}
