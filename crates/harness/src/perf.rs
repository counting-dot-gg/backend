use crate::bench::{Metrics, percentile};
use crate::env::{Target, sign};
use crate::schema;
use crate::server::{free_port, is_os_variable};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use sqlx::{ConnectOptions, MySqlConnection};
use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;

const FIXTURE: &str = include_str!("../fixture.sql");
const FIXTURE_THREAD_UUID: &str = "20000000-0000-4000-8000-000000000001";
const FIXTURE_COUNTER: &str = "10000000-0000-4000-8000-000000000003";
const SEEDED_POSTS: usize = 20_000;

#[derive(Clone, Copy)]
pub struct Load {
    pub seconds: u64,
    pub clients: usize,
    pub burst: usize,
}

const ENDPOINTS: [(&str, bool); 12] = [
    ("/api/auth/status", true),
    ("/api/features", false),
    ("/api/thread/all", false),
    ("/api/thread/fx_main", false),
    ("/api/thread/recentOnLoadFull/fx_main", true),
    ("/api/counter/counters/1", false),
    ("/api/counter/countersByXP/1", false),
    ("/api/thread/blogs/1", false),
    ("/api/emote", false),
    ("/api/rank/season", false),
    ("/api/site-goals", false),
    ("/api/user/shop", true),
];

fn base_command(bin: &Path, target: &Target, workdir: &Path, log_name: &str) -> Result<Command> {
    std::fs::create_dir_all(workdir)?;
    let bin = std::fs::canonicalize(bin).with_context(|| format!("binary {}", bin.display()))?;
    let log = std::fs::File::create(workdir.join(log_name))?;
    let m = &target.mysql;
    let mut cmd = Command::new(bin);
    cmd.current_dir(workdir)
        .env_clear()
        .envs(std::env::vars().filter(|(k, _)| is_os_variable(k)))
        .env("MYSQL_DB_HOST", &m.host)
        .env("MYSQL_DB_PORT", m.port.to_string())
        .env("MYSQL_DB_USERNAME", &m.user)
        .env("MYSQL_DB_PASSWORD", &m.password)
        .env("MYSQL_DB_DATABASE", &m.database)
        .env("REDIS_URL", &target.redis_url)
        .env("COOKIE_SECRET", &target.cookie_secret)
        .env("WEBSOCKET_APP_PORT", free_port()?.to_string())
        .env("STATS_PORT", free_port()?.to_string())
        .env("HOST_URL", crate::env::SITE_ORIGIN)
        .env("HTTP_PROXY", "http://127.0.0.1:9")
        .env("HTTPS_PROXY", "http://127.0.0.1:9")
        .env("NO_PROXY", "localhost,127.0.0.1,::1")
        .env("RUST_LOG", "warn")
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log))
        .kill_on_drop(true);
    Ok(cmd)
}

fn rss_mb(child: &Child) -> Option<f64> {
    let pid = child.id()?;
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let kb: f64 = status
        .lines()
        .find(|l| l.starts_with("VmRSS:"))?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()?;
    Some(kb / 1024.0)
}

async fn seed_posts(target: &Target) -> Result<()> {
    let mut conn: MySqlConnection = target.mysql.connect_options().connect().await?;
    schema::load(&mut conn, FIXTURE).await?;
    let start = 1000usize;
    for chunk in (0..SEEDED_POSTS).collect::<Vec<_>>().chunks(1000) {
        let rows: Vec<String> = chunk
            .iter()
            .map(|i| {
                let n = start + i;
                let comment = if i % 7 == 0 { "nice one" } else { "" };
                format!(
                    "('31000000-0000-4000-8000-{n:012}', '{}', '1000', '1000', '{n}{}', 1, 1, '{n}', '{n}', 0, '{FIXTURE_THREAD_UUID}', {}, '{comment}', '{FIXTURE_COUNTER}', 0, 0, '[]', {n}, 1, 3)",
                    1_790_000_000_000u64 + n as u64 * 1000,
                    if comment.is_empty() { String::new() } else { format!(" {comment}") },
                    u8::from(!comment.is_empty()),
                )
            })
            .collect();
        sqlx::query(&format!(
            "INSERT INTO post (uuid, timestamp, timeSinceLastCount, timeSinceLastPost, rawText, isCount, isValidCount, \
             countContent, rawCount, stricken, thread, hasComment, comment, authorUUID, isDeleted, isCommentDeleted, \
             reactions, validCountNumber, threadId, authorId) VALUES {}",
            rows.join(", ")
        ))
        .execute(&mut conn)
        .await?;
    }
    Ok(())
}

async fn login(target: &Target) -> Result<String> {
    let pool = target.pool().await?;
    let sid = "bench-session";
    let data = json!({
        "cookie": { "originalMaxAge": 3_600_000, "expires": "2099-01-01T00:00:00.000Z", "httpOnly": true, "path": "/" },
        "passport": { "user": { "uuid": FIXTURE_COUNTER, "discordId": "fxdiscord3" } }
    });
    sqlx::query("INSERT INTO session (id, expiredAt, json) VALUES (?, ?, ?)")
        .bind(sid)
        .bind(4_102_444_800_000i64)
        .bind(data.to_string())
        .execute(&pool)
        .await?;
    Ok(format!("connect.sid={}", sign(sid, &target.cookie_secret)))
}

struct Api {
    base: String,
    http: reqwest::Client,
    cookie: String,
    next_ip: AtomicU64,
}

impl Api {
    async fn get(&self, path: &str, authed: bool) -> Result<(u16, f64)> {
        let n = self.next_ip.fetch_add(1, Ordering::Relaxed);
        let ip = format!("10.{}.{}.{}", (n >> 16) & 255, (n >> 8) & 255, n & 255);
        let mut req = self
            .http
            .get(format!("{}{path}", self.base))
            .header("x-forwarded-for", ip);
        if authed {
            req = req.header("cookie", &self.cookie);
        }
        let sent = Instant::now();
        let res = req.send().await?;
        let status = res.status().as_u16();
        res.bytes().await?;
        Ok((status, sent.elapsed().as_secs_f64() * 1000.0))
    }

    async fn closed_loop(&self, offset: usize, duration: Duration) -> Result<Vec<f64>> {
        let mut out = Vec::new();
        let started = Instant::now();
        let mut i = offset;
        while started.elapsed() < duration {
            let (path, authed) = ENDPOINTS[i % ENDPOINTS.len()];
            let (status, ms) = self.get(path, authed).await?;
            ensure!(status < 500, "{path} answered {status} under load");
            out.push(ms);
            i += 1;
        }
        Ok(out)
    }
}

pub async fn api_round(bin: &Path, target: &Target, workdir: &Path, load: Load) -> Result<Metrics> {
    target.reset().await?;
    seed_posts(target).await?;
    let cookie = login(target).await?;
    let port = free_port()?;
    let mut child = base_command(bin, target, workdir, "api.log")?
        .env("PORT", port.to_string())
        .env("BOT_PORT", free_port()?.to_string())
        .env("STATS_HTTP_URL", "http://127.0.0.1:9")
        .env("UPLOADS_DIR", std::fs::canonicalize(workdir)?.join("uploads"))
        .env("TRUST_PROXY_HOPS", "1")
        .spawn()
        .context("start api")?;
    let api = Arc::new(Api {
        base: format!("http://127.0.0.1:{port}"),
        http: reqwest::Client::builder()
            .no_proxy()
            .pool_max_idle_per_host(256)
            .build()?,
        cookie,
        next_ip: AtomicU64::new(1),
    });
    let started = Instant::now();
    loop {
        if let Some(s) = child.try_wait()? {
            bail!(
                "api exited during startup ({s}); see {}",
                workdir.join("api.log").display()
            );
        }
        if let Ok((200, _)) = api.get("/api/thread/fx_main", false).await {
            break;
        }
        ensure!(started.elapsed() < Duration::from_secs(60), "api not ready after 60s");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    for (path, authed) in ENDPOINTS {
        let (status, _) = api.get(path, authed).await?;
        ensure!(status == 200, "{path} answered {status} before the benchmark");
    }
    api.closed_loop(0, Duration::from_secs(1)).await?;
    let mut m = Metrics::new();
    let single = api.closed_loop(0, Duration::from_secs(load.seconds)).await?;
    m.insert("single.p50_ms".into(), percentile(&single, 0.50));
    m.insert("single.p99_ms".into(), percentile(&single, 0.99));
    let t0 = Instant::now();
    let mut runs = Vec::new();
    for c in 0..load.clients {
        let api = api.clone();
        let seconds = load.seconds;
        runs.push(tokio::spawn(async move {
            api.closed_loop(c, Duration::from_secs(seconds)).await
        }));
    }
    let mut all = Vec::new();
    for r in runs {
        all.extend(r.await??);
    }
    m.insert(
        "parallel.req_per_s".into(),
        all.len() as f64 / t0.elapsed().as_secs_f64(),
    );
    m.insert("parallel.p50_ms".into(), percentile(&all, 0.50));
    m.insert("parallel.p99_ms".into(), percentile(&all, 0.99));
    if let Some(rss) = rss_mb(&child) {
        m.insert("rss_mb".into(), rss);
    }
    drop(child);
    Ok(m)
}

struct Arrival {
    at: Instant,
    channel: String,
    body: String,
}

async fn fake_discord(tx: mpsc::UnboundedSender<Arrival>) -> Result<u16> {
    use axum::extract::{Path as AxPath, State};
    use axum::routing::post;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let ids = Arc::new(AtomicU64::new(1000));
    let app = axum::Router::new()
        .route(
            "/api/v10/channels/{channel}/messages",
            post(
                |State((tx, ids)): State<(mpsc::UnboundedSender<Arrival>, Arc<AtomicU64>)>,
                 AxPath(channel): AxPath<String>,
                 body: String| async move {
                    let _ = tx.send(Arrival {
                        at: Instant::now(),
                        channel: channel.clone(),
                        body,
                    });
                    axum::Json(json!({
                        "id": ids.fetch_add(1, Ordering::Relaxed).to_string(), "channel_id": channel,
                        "author": { "id": "1", "username": "bench", "discriminator": "0", "avatar": null },
                        "content": "", "timestamp": "2026-01-01T00:00:00.000000+00:00", "edited_timestamp": null,
                        "tts": false, "mention_everyone": false, "mentions": [], "mention_roles": [],
                        "attachments": [], "embeds": [], "pinned": false, "type": 0
                    }))
                },
            ),
        )
        .with_state((tx, ids));
    tokio::spawn(async move { axum::serve(listener, app).await });
    Ok(port)
}

const LOG_CHANNEL: &str = "101";
const MOD_CHANNEL: &str = "102";
const MIRROR_CHANNEL: &str = "103";
const KPART_CHANNEL: &str = "104";

fn bot_command(i: usize) -> (&'static str, Value) {
    let tag = format!("bm{i}x");
    match i % 4 {
        0 => ("logCustomMessage", json!({ "message": format!("{tag} custom") })),
        1 => (
            "logClientError",
            json!({ "scope": "bench", "url": "/thread/main", "message": tag, "userAgent": "bench" }),
        ),
        2 => (
            "kpartAlert",
            json!({
                "channelID": KPART_CHANNEL, "kPartRole": "105", "title": tag, "threadName": "main",
                "replay": "/replay/main?startValidCountNumber=1&endValidCountNumber=1000",
                "splits": [61000, 59000, 60000, 58000, 62000, 61000, 60000, 59000, 60000, 61000, 601000]
            }),
        ),
        _ => (
            "logLock",
            json!({ "locker": { "name": "bench" }, "thread": { "name": tag, "uuid": "t", "locked": true } }),
        ),
    }
}

fn tag_of(body: &str) -> Option<usize> {
    let at = body.find("bm")?;
    let rest = &body[at + 2..];
    let end = rest.find('x')?;
    rest[..end].parse().ok()
}

pub async fn bot_round(bin: &Path, target: &Target, workdir: &Path, load: Load) -> Result<Metrics> {
    target.reset().await?;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let discord = fake_discord(tx).await?;
    let port = free_port()?;
    let mut child = base_command(bin, target, workdir, "bot.log")?
        .env("BOT_PORT", port.to_string())
        .env("DISCORD_BOT_TOKEN", "bench-token")
        .env("DISCORD_API_PROXY", format!("http://127.0.0.1:{discord}"))
        .env("DISCORD_LOG_CHANNEL_ID", LOG_CHANNEL)
        .env("DISCORD_MOD_LOG_CHANNEL_ID", MOD_CHANNEL)
        .env("CHAT_MIRROR_CHANNEL_ID", MIRROR_CHANNEL)
        .spawn()
        .context("start bot")?;
    let started = Instant::now();
    while tokio::net::TcpStream::connect(("127.0.0.1", port)).await.is_err() {
        if let Some(s) = child.try_wait()? {
            bail!(
                "bot exited during startup ({s}); see {}",
                workdir.join("bot.log").display()
            );
        }
        ensure!(started.elapsed() < Duration::from_secs(30), "bot not ready after 30s");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let bot = cgg_web::nest::Client::connect("bench", format!("127.0.0.1:{port}"));
    let mut m = Metrics::new();
    let mut i = 0usize;
    let mut single = Vec::new();
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(load.seconds) {
        let (cmd, data) = bot_command(i);
        let sent = Instant::now();
        bot.emit(cmd, data);
        loop {
            let a = tokio::time::timeout(Duration::from_secs(10), rx.recv())
                .await
                .context("bot sent nothing to Discord within 10s")?
                .context("fake Discord stopped")?;
            if tag_of(&a.body) == Some(i) {
                if i > 0 {
                    single.push((a.at - sent).as_secs_f64() * 1000.0);
                }
                break;
            }
        }
        i += 1;
    }
    m.insert("single.p50_ms".into(), percentile(&single, 0.50));
    m.insert("single.p99_ms".into(), percentile(&single, 0.99));

    let first = i;
    let mut sent_at: HashMap<usize, Instant> = HashMap::new();
    let t1 = Instant::now();
    for n in first..first + load.burst {
        let (cmd, data) = bot_command(n);
        sent_at.insert(n, Instant::now());
        bot.emit(cmd, data);
    }
    let mut burst = Vec::new();
    let mut last = t1;
    while burst.len() < load.burst {
        let a = tokio::time::timeout(Duration::from_secs(30), rx.recv())
            .await
            .with_context(|| format!("only {} of {} burst messages reached Discord", burst.len(), load.burst))?
            .context("fake Discord stopped")?;
        if let Some(sent) = tag_of(&a.body).and_then(|t| sent_at.get(&t)) {
            burst.push((a.at - *sent).as_secs_f64() * 1000.0);
            last = a.at;
        }
    }
    m.insert(
        "burst.msgs_per_s".into(),
        load.burst as f64 / (last - t1).as_secs_f64().max(0.001),
    );
    m.insert("burst.p99_ms".into(), percentile(&burst, 0.99));

    let posts = (load.burst / 10).max(1) * 10;
    let t2 = Instant::now();
    for n in 0..posts {
        bot.emit(
            "sendChatMirror",
            json!({
                "username": format!("user{n}"), "color": "#336699", "text": format!("message {n}"),
                "discordId": "42", "avatar": "abcdef123", "counterUsername": "user",
                "postUUID": format!("mirror-{n}"), "timestamp": (1_790_000_000_000u64 + n as u64).to_string(),
                "timeSinceLastPost": "900"
            }),
        );
    }
    let mut batches = 0;
    let mut mirrored = 0;
    while mirrored < posts {
        let a = tokio::time::timeout(Duration::from_secs(30), rx.recv())
            .await
            .with_context(|| format!("only {mirrored} of {posts} mirrored posts reached Discord"))?
            .context("fake Discord stopped")?;
        if a.channel == MIRROR_CHANNEL {
            batches += 1;
            mirrored += a.body.matches("\"description\"").count();
            last = a.at;
        }
    }
    ensure!(
        batches == posts / 10,
        "{posts} mirrored posts took {batches} messages, not {}",
        posts / 10
    );
    m.insert("mirror.drain_ms".into(), (last - t2).as_secs_f64() * 1000.0);
    if let Some(rss) = rss_mb(&child) {
        m.insert("rss_mb".into(), rss);
    }
    drop(child);
    Ok(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_are_found_in_bodies() {
        assert_eq!(tag_of(r#"{"content":"bm42x custom"}"#), Some(42));
        assert_eq!(tag_of(r#"{"content":"nothing"}"#), None);
    }
}
