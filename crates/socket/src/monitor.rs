use crate::app::{App, now_ms};
use crate::persister::PersistStats;
use serde_json::{Value, json};
use sqlx::Row;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

const SLOW_MS: u64 = 5_000;
const DEBOUNCE_MS: u64 = 60_000;
const LAG_RECORDS: u64 = 2_000;
const QUERY_TIMEOUT: Duration = Duration::from_secs(3);

static APP: OnceLock<Arc<App>> = OnceLock::new();
static LAST_ALERT: AtomicU64 = AtomicU64::new(0);

pub fn start(app: Arc<App>, stats: Arc<PersistStats>) {
    let _ = APP.set(app.clone());
    tokio::spawn(async move {
        let mut seen_failures = stats.failures.load(Ordering::Relaxed);
        let mut seen_batches = stats.batches.load(Ordering::Relaxed);
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            let committed = app.wal.get().map(|w| w.committed.get()).unwrap_or(0);
            let lag = committed.saturating_sub(stats.checkpoint.load(Ordering::Relaxed));
            let failures = stats.failures.load(Ordering::Relaxed);
            let batches = stats.batches.load(Ordering::Relaxed);
            let commit_ms = stats.last_commit_us.load(Ordering::Relaxed) / 1000;
            if failures > seen_failures {
                alert(
                    &app,
                    "🚨 MYSQL WRITES FAILING",
                    None,
                    json!({ "failures": failures - seen_failures, "lagRecords": lag }),
                )
                .await;
            } else if lag > LAG_RECORDS {
                alert(&app, "🚨 MYSQL BEHIND", None, json!({ "lagRecords": lag })).await;
            } else if batches > seen_batches && commit_ms >= SLOW_MS {
                alert(
                    &app,
                    "⚠️ SLOW MYSQL COMMIT",
                    Some(commit_ms),
                    json!({ "lagRecords": lag }),
                )
                .await;
            }
            seen_failures = failures;
            seen_batches = batches;
        }
    });
}

pub fn slow_post(ms: u64, thread: &str) {
    if ms < SLOW_MS {
        return;
    }
    let Some(app) = APP.get().cloned() else { return };
    let thread = thread.to_owned();
    tokio::spawn(async move { alert(&app, "⚠️ SLOW POST", Some(ms), json!({ "thread": thread })).await });
}

async fn alert(app: &App, prefix: &str, latency_ms: Option<u64>, context: Value) {
    let now = now_ms() as u64;
    let last = LAST_ALERT.load(Ordering::Relaxed);
    if now.saturating_sub(last) < DEBOUNCE_MS
        || LAST_ALERT
            .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
    {
        return;
    }
    let (processes, status, trx, innodb) = tokio::join!(
        process_list(app),
        global_status(app),
        innodb_trx(app),
        innodb_status(app)
    );
    let mut lines = vec![format!(
        "**{prefix}** — {}",
        chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    )];
    if let Some(ms) = latency_ms {
        lines.push(format!(
            "Latency: **{:.1}s** (threshold: {}s)",
            ms as f64 / 1000.0,
            SLOW_MS / 1000
        ));
    }
    if context.as_object().is_some_and(|o| !o.is_empty()) {
        lines.push(format!("Context: `{context}`"));
    }
    lines.push(String::new());
    lines.push(format!("**System** | {}", system_info()));
    lines.push(format!("**Process** | {}", process_info()));
    lines.push(format!("**MySQL**  | {status}"));
    for (title, body) in [
        ("Active transactions (InnoDB)", &trx),
        ("Long-running queries (>3s)", &processes),
        ("InnoDB status (locks/deadlocks)", &innodb),
    ] {
        if !body.is_empty() {
            lines.push(String::new());
            lines.push(format!("**{title}**"));
            lines.push(format!("```\n{body}\n```"));
        }
    }
    if processes.is_empty() {
        lines.push("No queries running >3s at alert time.".into());
    }
    let message: String = lines.join("\n").chars().take(2000).collect();
    tracing::warn!("monitor alert:\n{message}");
    if let Some(channel) = &app.cfg.discord.alert_channel {
        app.bot
            .emit("sendMessage", json!({ "channel": channel, "message": message }));
    }
}

async fn rows(app: &App, sql: &str) -> Result<Vec<sqlx::mysql::MySqlRow>, String> {
    match tokio::time::timeout(QUERY_TIMEOUT, sqlx::query(sql).fetch_all(&app.pool)).await {
        Ok(Ok(r)) => Ok(r),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err("timed out".into()),
    }
}

fn text(r: &sqlx::mysql::MySqlRow, col: &str) -> String {
    r.try_get::<String, _>(col)
        .or_else(|_| {
            r.try_get::<Vec<u8>, _>(col)
                .map(|b| String::from_utf8_lossy(&b).into_owned())
        })
        .unwrap_or_default()
}

fn clip(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

async fn process_list(app: &App) -> String {
    let sql = "SELECT CAST(TIME AS SIGNED) t, COMMAND c, CAST(COALESCE(INFO, '') AS CHAR) q \
               FROM information_schema.PROCESSLIST WHERE COMMAND <> 'Sleep' AND TIME >= 3 ORDER BY TIME DESC LIMIT 15";
    match rows(app, sql).await {
        Ok(rs) => rs
            .iter()
            .map(|r| {
                format!(
                    "[{}s] {} | {}",
                    r.try_get::<i64, _>("t").unwrap_or(0),
                    text(r, "c"),
                    clip(&text(r, "q"), 180)
                )
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Err(e) => format!("(PROCESSLIST failed: {e})"),
    }
}

async fn global_status(app: &App) -> String {
    let sql = "SHOW GLOBAL STATUS WHERE Variable_name IN \
               ('Threads_connected','Threads_running','Innodb_row_lock_waits','Innodb_row_lock_time_avg','Slow_queries')";
    match rows(app, sql).await {
        Ok(rs) => {
            let m: std::collections::HashMap<String, String> = rs
                .iter()
                .map(|r| (text(r, "Variable_name"), text(r, "Value")))
                .collect();
            let g = |k: &str| m.get(k).cloned().unwrap_or_default();
            format!(
                "threads: {}/{} (conn/running) | row_lock_waits: {} avg_ms: {} | slow_queries: {}",
                g("Threads_connected"),
                g("Threads_running"),
                g("Innodb_row_lock_waits"),
                g("Innodb_row_lock_time_avg"),
                g("Slow_queries")
            )
        }
        Err(e) => format!("(SHOW GLOBAL STATUS failed: {e})"),
    }
}

async fn innodb_trx(app: &App) -> String {
    let sql = "SELECT CAST(trx_state AS CHAR) s, CAST(trx_started AS CHAR) st, CAST(trx_rows_locked AS CHAR) l, \
               CAST(trx_lock_structs AS CHAR) ls, CAST(COALESCE(trx_query, 'no query') AS CHAR) q \
               FROM information_schema.INNODB_TRX WHERE trx_started < NOW() - INTERVAL 3 SECOND LIMIT 10";
    match rows(app, sql).await {
        Ok(rs) => rs
            .iter()
            .map(|r| {
                format!(
                    "[{}] started={} locks={} structs={} | {}",
                    text(r, "s"),
                    text(r, "st"),
                    text(r, "l"),
                    text(r, "ls"),
                    clip(&text(r, "q"), 150)
                )
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Err(e) => format!("(INNODB_TRX failed: {e})"),
    }
}

async fn innodb_status(app: &App) -> String {
    let full = match rows(app, "SHOW ENGINE INNODB STATUS").await {
        Ok(rs) => rs.first().map(|r| text(r, "Status")).unwrap_or_default(),
        Err(e) => return format!("(SHOW ENGINE INNODB STATUS failed: {e})"),
    };
    let boundary = |mut i: usize| {
        while !full.is_char_boundary(i) {
            i -= 1;
        }
        i
    };
    let window = |from: usize, len: usize| -> String {
        full[boundary(from)..boundary((from + len).min(full.len()))]
            .trim()
            .to_owned()
    };
    let mut parts = Vec::new();
    if let Some(i) = full.find("LATEST DETECTED DEADLOCK") {
        parts.push(window(i, 824));
    }
    if let Some(w) = full.find("LOCK WAIT") {
        let from = full[..w].rfind("TRANSACTION").filter(|t| w - t <= 400).unwrap_or(w);
        parts.push(window(from, (w - from) + 409));
    }
    clip(&parts.join("\n---\n"), 800)
}

fn system_info() -> String {
    let load = std::fs::read_to_string("/proc/loadavg")
        .ok()
        .map(|s| s.split_whitespace().take(3).collect::<Vec<_>>().join("/"))
        .unwrap_or_else(|| "-".into());
    let mem = std::fs::read_to_string("/proc/meminfo").ok().and_then(|s| {
        let kb = |k: &str| {
            s.lines()
                .find(|l| l.starts_with(k))?
                .split_whitespace()
                .nth(1)?
                .parse::<f64>()
                .ok()
        };
        let (total, avail) = (kb("MemTotal:")?, kb("MemAvailable:")?);
        Some(format!(
            "{:.0}/{:.0}MB ({:.1}%)",
            (total - avail) / 1024.0,
            total / 1024.0,
            100.0 * (total - avail) / total
        ))
    });
    format!("load: {load} (1m/5m/15m) | mem: {}", mem.unwrap_or_else(|| "-".into()))
}

fn process_info() -> String {
    let rss = std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmRSS:"))?
                .split_whitespace()
                .nth(1)?
                .parse::<f64>()
                .ok()
        })
        .map(|kb| format!("{:.0}MB", kb / 1024.0))
        .unwrap_or_else(|| "-".into());
    format!("rss: {rss}")
}
