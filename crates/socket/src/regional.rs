use crate::app::{App, now_ms};
use crate::gateway::counter_of;
use crate::model::{Counter, Post, ThreadMeta};
use crate::regional_log::{Sample, SampleLog};
use crate::thread::{Decided, Msg, PostJob, ThreadHandle};
use crate::wal::Origin;
use axum::Json;
use axum::http::{HeaderMap, StatusCode};
use cgg_decide::{Book, Decision};
use hmac::{Hmac, Mac};
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::Sha256;
use socketioxide::extract::SocketRef;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const REGIONS: [&str; 8] = ["wnam", "enam", "weur", "eeur", "apac", "apac-ne", "apac-se", "oc"];
const MAX_CLOCK_SKEW_MS: i64 = 60_000;
const WORKER_TIMEOUT: Duration = Duration::from_secs(5);
const DAY_MS: i64 = 86_400_000;
const RECENT_HASHES: usize = 4096;
const HANDED_HASHES: usize = 512;
const FAILED_CHECKS_BEFORE_RECLAIM: u32 = 2;
const REPORT_CHECK_EVERY_MS: i64 = 5_000;
const STATUS_MAX_POSTS: usize = 200;
const MAX_WAITING: usize = 5_000;
const HANDOFF_ROUNDS: usize = 3;
const STRAGGLER_CHECKS: usize = 8;
const UNREACHABLE_BEFORE_GIVING_UP: usize = 2;
const LATENCY_FRESH_MS: i64 = 30 * 60_000;
const RECENT_POSTS: usize = 30;
const RECENT_WINDOW_MS: f64 = 60_000.0;
const AUTO_EVERY: Duration = Duration::from_secs(1);
const FINISH_MOVES: Duration = Duration::from_secs(10);

#[derive(Debug, Clone)]
pub struct RegionalConfig {
    pub threads: Vec<String>,
    pub worker_url: Option<String>,
    pub secret: String,
    pub cooldown_ms: i64,
    pub lull_ms: i64,
    pub daily_post_limit: u64,
    pub health_every: Duration,
    pub stall_ms: u64,
    pub auto: bool,
    pub auto_countdown_ms: i64,
    pub auto_cooldown_ms: i64,
    pub auto_idle_ms: i64,
    pub auto_min_posts: usize,
    pub manual_hold_ms: i64,
    pub snooze_ms: i64,
}

impl RegionalConfig {
    pub fn from_env() -> RegionalConfig {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        let secs = |k: &str, default: i64| var(k).and_then(|v| v.parse::<i64>().ok()).unwrap_or(default) * 1000;
        RegionalConfig {
            threads: var("DO_THREADS")
                .map(|v| {
                    v.split(',')
                        .map(|t| t.trim().to_owned())
                        .filter(|t| !t.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
            worker_url: var("DO_WORKER_URL").map(|u| u.trim_end_matches('/').to_owned()),
            secret: var("DO_SHARED_SECRET").unwrap_or_default(),
            cooldown_ms: var("DO_MOVE_COOLDOWN_SECS")
                .and_then(|v| v.parse::<i64>().ok())
                .unwrap_or(600)
                * 1000,
            lull_ms: var("DO_MOVE_LULL_MS").and_then(|v| v.parse().ok()).unwrap_or(5000),
            daily_post_limit: var("DO_DAILY_POST_LIMIT")
                .and_then(|v| v.parse().ok())
                .unwrap_or(1_600_000),
            health_every: Duration::from_secs(var("DO_HEALTH_SECS").and_then(|v| v.parse().ok()).unwrap_or(10).max(1)),
            stall_ms: var("DO_STALL_SECS").and_then(|v| v.parse::<u64>().ok()).unwrap_or(10) * 1000,
            auto: var("DO_AUTO").is_none_or(|v| v != "off"),
            auto_countdown_ms: var("DO_AUTO_COUNTDOWN_MS").and_then(|v| v.parse().ok()).unwrap_or(3000),
            auto_cooldown_ms: secs("DO_AUTO_COOLDOWN_SECS", 60),
            auto_idle_ms: secs("DO_AUTO_IDLE_SECS", 3600),
            auto_min_posts: var("DO_AUTO_MIN_POSTS").and_then(|v| v.parse().ok()).unwrap_or(5),
            manual_hold_ms: secs("DO_MANUAL_HOLD_SECS", 600),
            snooze_ms: secs("DO_AUTO_SNOOZE_SECS", 300),
        }
    }

    pub fn movable(&self, thread: &str) -> bool {
        self.worker_url.is_some() && !self.secret.is_empty() && self.threads.iter().any(|t| t == thread)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ownership {
    pub region: Option<String>,
    pub epoch: u64,
    pub moving: bool,
    pub moved_at_ms: i64,
    pub last_seq: u64,
    pub target: Option<String>,
    pub manual_until_ms: i64,
    pub picked_at_ms: i64,
    pub snoozed_until_ms: i64,
    pub pending: Option<Pending>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Pending {
    pub target: Option<String>,
    pub at_ms: i64,
}

#[derive(Clone, Copy, PartialEq)]
enum Mover {
    Person,
    Auto,
}

#[derive(Default, Debug, Clone, Copy, PartialEq)]
pub struct Budget {
    pub day: i64,
    pub posts: u64,
}

impl Budget {
    fn roll(&mut self, now: i64) {
        let today = now.div_euclid(DAY_MS);
        if self.day != today {
            *self = Budget { day: today, posts: 0 };
        }
    }

    pub fn add(&mut self, now: i64, posts: u64, limit: u64) -> bool {
        self.roll(now);
        let was_open = self.posts < limit;
        self.posts += posts;
        was_open && self.posts >= limit
    }

    pub fn spent(&mut self, now: i64, limit: u64) -> bool {
        self.roll(now);
        self.posts >= limit
    }
}

#[derive(Default)]
struct RecentHashes {
    order: VecDeque<String>,
    seen: HashSet<String>,
}

impl RecentHashes {
    fn newest(&self, n: usize) -> Vec<String> {
        self.order.iter().rev().take(n).rev().cloned().collect()
    }

    fn first_time(&mut self, hash: &str) -> bool {
        if self.seen.contains(hash) {
            return false;
        }
        self.seen.insert(hash.to_owned());
        self.order.push_back(hash.to_owned());
        if self.order.len() > RECENT_HASHES
            && let Some(old) = self.order.pop_front()
        {
            self.seen.remove(&old);
        }
        true
    }
}

#[derive(Default)]
pub struct Regional {
    owners: Mutex<HashMap<String, Ownership>>,
    ingest_lock: tokio::sync::Mutex<()>,
    http: reqwest::Client,
    pub samples: SampleLog,
    budget: Mutex<Budget>,
    recent: Mutex<HashMap<String, RecentHashes>>,
    failed_checks: Mutex<HashMap<String, u32>>,
    last_report_check: Mutex<HashMap<String, i64>>,
    waiting: Mutex<HashMap<String, Vec<PostJob>>>,
    passing: tokio::sync::Mutex<()>,
    stopping: std::sync::atomic::AtomicBool,
    latency: Mutex<HashMap<String, HashMap<String, (f64, i64)>>>,
}

pub enum Admitted {
    Here(Box<PostJob>),
    Waiting,
    Elsewhere(Box<PostJob>),
}

impl Regional {
    pub fn admit(&self, thread: &str, job: PostJob) -> Admitted {
        let owners = self.owners.lock();
        match owners.get(thread) {
            Some(o) if o.moving => {
                let mut waiting = self.waiting.lock();
                let queue = waiting.entry(thread.to_owned()).or_default();
                if queue.len() < MAX_WAITING {
                    queue.push(job);
                }
                Admitted::Waiting
            }
            Some(o) if o.region.is_some() => Admitted::Elsewhere(Box::new(job)),
            _ => Admitted::Here(Box::new(job)),
        }
    }

    fn stopping(&self) -> bool {
        self.stopping.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn any_moving(&self) -> bool {
        self.owners.lock().values().any(|o| o.moving)
    }

    pub fn note_latency(&self, counter: &str, place: &str, ms: f64, now: i64) {
        self.latency
            .lock()
            .entry(counter.to_owned())
            .or_default()
            .insert(place.to_owned(), (ms, now));
    }

    fn latencies(&self, now: i64, authors: &[String]) -> crate::auto::Latencies {
        let all = self.latency.lock();
        authors
            .iter()
            .filter_map(|a| {
                let fresh = all
                    .get(a)?
                    .iter()
                    .filter(|(_, (_, at))| now - at <= LATENCY_FRESH_MS)
                    .map(|(place, (ms, _))| (place.clone(), *ms))
                    .collect();
                Some((a.clone(), fresh))
            })
            .collect()
    }

    fn take_waiting(&self, thread: &str) -> Vec<PostJob> {
        self.waiting.lock().remove(thread).unwrap_or_default()
    }

    fn put_back(&self, thread: &str, mut jobs: Vec<PostJob>) {
        let mut waiting = self.waiting.lock();
        let queue = waiting.entry(thread.to_owned()).or_default();
        jobs.append(queue);
        *queue = jobs;
    }

    fn hand_to_actor(&self, thread: &str, h: &ThreadHandle, finish: bool) {
        let mut owners = self.owners.lock();
        for job in self.take_waiting(thread) {
            if !self.first_time(thread, job.post_hash.as_ref()) {
                continue;
            }
            if h.tx.try_send(Msg::Post(Box::new(job))).is_err() {
                tracing::warn!("regional: {thread}'s actor queue is full; a post that waited for a move was dropped");
            }
        }
        if finish && let Some(o) = owners.get_mut(thread) {
            o.moving = false;
        }
    }

    fn hand_over(&self, thread: &str, last: bool) -> (Vec<PostJob>, bool) {
        let mut owners = self.owners.lock();
        let jobs = self.take_waiting(thread);
        let done = jobs.is_empty() || last;
        if done && let Some(o) = owners.get_mut(thread) {
            o.moving = false;
        }
        (jobs, done)
    }

    pub fn get(&self, thread: &str) -> Ownership {
        self.owners.lock().get(thread).cloned().unwrap_or_default()
    }

    pub fn is_remote(&self, thread: &str) -> bool {
        self.owners
            .lock()
            .get(thread)
            .is_some_and(|o| o.region.is_some() || o.moving)
    }

    fn known_hashes(&self, thread: &str) -> Vec<String> {
        self.recent
            .lock()
            .get(thread)
            .map(|r| r.newest(HANDED_HASHES))
            .unwrap_or_default()
    }

    pub fn first_time(&self, thread: &str, hash: Option<&Value>) -> bool {
        let Some(hash) = hash.and_then(Value::as_str).filter(|h| !h.is_empty()) else {
            return true;
        };
        self.recent
            .lock()
            .entry(thread.to_owned())
            .or_default()
            .first_time(hash)
    }

    fn regions_open(&self, limit: u64) -> bool {
        !self.budget.lock().spent(now_ms(), limit)
    }

    pub fn remote_threads(&self) -> Vec<String> {
        self.owners
            .lock()
            .iter()
            .filter(|(_, o)| o.region.is_some() && !o.moving)
            .map(|(t, _)| t.clone())
            .collect()
    }

    fn update(&self, thread: &str, change: impl FnOnce(&mut Ownership)) -> Ownership {
        let mut owners = self.owners.lock();
        let o = owners.entry(thread.to_owned()).or_default();
        change(o);
        o.clone()
    }
}

pub async fn load(app: &App) -> anyhow::Result<()> {
    crate::regional_log::start(app.pool.clone(), app.regional.samples.clone()).await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS cgg_thread_owner (thread VARCHAR(255) NOT NULL PRIMARY KEY, \
         region VARCHAR(16) NULL, epoch BIGINT UNSIGNED NOT NULL DEFAULT 0, \
         lastSeq BIGINT UNSIGNED NOT NULL DEFAULT 0, movedAt BIGINT NOT NULL DEFAULT 0)",
    )
    .execute(&app.pool)
    .await?;
    let today = now_ms().div_euclid(DAY_MS) * DAY_MS;
    let ingested: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM cgg_regional_sample WHERE kind = 'ingest' AND at >= ?")
            .bind(today)
            .fetch_one(&app.pool)
            .await?;
    *app.regional.budget.lock() = Budget {
        day: today / DAY_MS,
        posts: ingested.max(0) as u64,
    };
    tracing::info!(
        "regional: {ingested} posts through regions today, limit {}",
        app.cfg.regional.daily_post_limit
    );
    let rows: Vec<(String, Option<String>, u64, u64, i64)> =
        sqlx::query_as("SELECT thread, region, epoch, lastSeq, movedAt FROM cgg_thread_owner")
            .fetch_all(&app.pool)
            .await?;
    let mut owners = app.regional.owners.lock();
    for (thread, region, epoch, last_seq, moved_at_ms) in rows {
        if region.is_some() {
            tracing::info!(
                "regional: {thread} is owned by the {} object (epoch {epoch})",
                region.as_deref().unwrap_or("?")
            );
        }
        owners.insert(
            thread,
            Ownership {
                region,
                epoch,
                moving: false,
                moved_at_ms,
                last_seq,
                target: None,
                manual_until_ms: 0,
                picked_at_ms: 0,
                snoozed_until_ms: 0,
                pending: None,
            },
        );
    }
    Ok(())
}

async fn save(app: &App, thread: &str, o: &Ownership) {
    let saved = sqlx::query(
        "INSERT INTO cgg_thread_owner (thread, region, epoch, lastSeq, movedAt) VALUES (?, ?, ?, ?, ?) \
         ON DUPLICATE KEY UPDATE region = VALUES(region), epoch = VALUES(epoch), lastSeq = VALUES(lastSeq), \
         movedAt = VALUES(movedAt)",
    )
    .bind(thread)
    .bind(&o.region)
    .bind(o.epoch)
    .bind(o.last_seq)
    .bind(o.moved_at_ms)
    .execute(&app.pool)
    .await;
    if let Err(e) = saved {
        tracing::warn!("regional: saving the owner of {thread}: {e}");
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn signature(secret: &str, timestamp: &str, body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac accepts any key length");
    mac.update(timestamp.as_bytes());
    mac.update(b".");
    mac.update(body);
    hex(&mac.finalize().into_bytes())
}

fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub fn verified(secret: &str, headers: &HeaderMap, body: &[u8], now: i64) -> bool {
    let header = |k: &str| headers.get(k).and_then(|v| v.to_str().ok());
    let (Some(timestamp), Some(sig)) = (header("x-cgg-ts"), header("x-cgg-sig")) else {
        return false;
    };
    let fresh = timestamp
        .parse::<i64>()
        .is_ok_and(|t| (now - t).abs() <= MAX_CLOCK_SKEW_MS);
    fresh && same(signature(secret, timestamp, body).as_bytes(), sig.as_bytes())
}

async fn call_worker(app: &App, path: &str, body: Value) -> anyhow::Result<Value> {
    let cfg = &app.cfg.regional;
    let base = cfg
        .worker_url
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("DO_WORKER_URL is not set"))?;
    let body = serde_json::to_vec(&body)?;
    let timestamp = now_ms().to_string();
    let response = app
        .regional
        .http
        .post(format!("{base}{path}"))
        .header("content-type", "application/json")
        .header("x-cgg-ts", &timestamp)
        .header("x-cgg-sig", signature(&cfg.secret, &timestamp, &body))
        .body(body)
        .timeout(WORKER_TIMEOUT)
        .send()
        .await?;
    let status = response.status();
    let reply: Value = response.json().await.unwrap_or(Value::Null);
    anyhow::ensure!(status.is_success(), "worker {path} answered {status}: {reply}");
    Ok(reply)
}

fn auto_paused(o: &Ownership, now: i64, cfg: &RegionalConfig) -> bool {
    !cfg.auto || o.manual_until_ms > now || o.snoozed_until_ms > now
}

pub fn info(app: &App, thread: &str) -> Value {
    let o = app.regional.get(thread);
    let cfg = &app.cfg.regional;
    let now = now_ms();
    let paused_until = o.manual_until_ms.max(o.snoozed_until_ms);
    json!({
        "auto": !auto_paused(&o, now, cfg),
        "pausedUntil": (cfg.auto && paused_until > now).then_some(paused_until),
        "pending": o.pending.as_ref().map(|p| json!({
            "target": p.target.as_deref().unwrap_or(crate::auto::VPS),
            "inMs": (p.at_ms - now).max(0),
        })),
        "thread_name": thread,
        "movable": cfg.movable(thread),
        "region": o.region,
        "epoch": o.epoch,
        "moving": o.moving,
        "worker": cfg.worker_url.as_deref().map(|u| u.replacen("https://", "wss://", 1).replacen("http://", "ws://", 1)),
        "cooldownUntil": o.picked_at_ms + cfg.cooldown_ms,
        "regionsOpen": app.regional.regions_open(cfg.daily_post_limit),
        "burstPerSec": app.cfg.post_burst_per_sec,
        "target": if o.moving { o.target.clone() } else { None },
    })
}

async fn broadcast(app: &App, thread: &str) {
    let Some(h) = app.thread(thread) else { return };
    let room = format!("thread_{}", h.meta.read().uuid);
    app.to(room, "thread_owner", &info(app, thread)).await;
}

fn last_post_ms(h: &ThreadHandle) -> Option<i64> {
    h.ring
        .read()
        .back()
        .and_then(|p| p.timestamp.parse::<f64>().ok())
        .map(|t| t as i64)
}

fn target_region(requested: &str) -> Result<Option<String>, &'static str> {
    match requested {
        "auto" | "vps" => Ok(None),
        r => REGIONS
            .iter()
            .find(|known| **known == r)
            .map(|known| Some((*known).to_owned()))
            .ok_or("bad_region"),
    }
}

fn claim_move(
    app: &App,
    thread: &str,
    h: &ThreadHandle,
    target: &Option<String>,
    mover: Mover,
) -> Result<(), &'static str> {
    let cfg = &app.cfg.regional;
    let now = now_ms();
    if app.regional.stopping() {
        return Err("stopping");
    }
    let mut owners = app.regional.owners.lock();
    let o = owners.entry(thread.to_owned()).or_default();
    if o.moving {
        return Err("moving");
    }
    if &o.region == target {
        return Err("already_there");
    }
    if target.is_some() && app.regional.budget.lock().spent(now, cfg.daily_post_limit) {
        return Err("budget");
    }
    if mover == Mover::Person && now - o.picked_at_ms < cfg.cooldown_ms {
        return Err("cooldown");
    }
    let post_clock_ms = app.clock.now_ms() as i64;
    if mover == Mover::Person && last_post_ms(h).is_some_and(|t| post_clock_ms - t < cfg.lull_ms) {
        return Err("busy");
    }
    o.moving = true;
    o.target = target.clone();
    Ok(())
}

pub async fn request_move(app: &App, s: &SocketRef, data: &Value) {
    let Some(thread) = data.get("thread_name").and_then(Value::as_str) else {
        return;
    };
    let refuse = |reason: &str| {
        s.emit(
            "thread_region_refused",
            &json!({ "thread_name": thread, "reason": reason }),
        )
        .ok();
    };
    if !app.cfg.regional.movable(thread) {
        return refuse("not_movable");
    }
    let Some(counter) = counter_of(app, s).await else {
        return refuse("signed_out");
    };
    let Some(h) = app.thread(thread) else {
        return refuse("no_thread");
    };
    if !h.meta.read().may_post(&counter) {
        return refuse("not_allowed");
    }
    if !portable(&h.meta.read()) {
        return refuse("not_movable");
    }
    let asked = data.get("region").and_then(Value::as_str).unwrap_or("auto");
    if asked == "auto" {
        app.regional.update(thread, |o| {
            o.manual_until_ms = 0;
            o.snoozed_until_ms = 0;
        });
        tracing::info!("regional: {} turns auto on for {thread}", counter.username);
        return broadcast(app, thread).await;
    }
    let target = match target_region(asked) {
        Ok(t) => t,
        Err(reason) => return refuse(reason),
    };
    if let Err(reason) = claim_move(app, thread, &h, &target, Mover::Person) {
        return refuse(reason);
    }
    let hold = app.cfg.regional.manual_hold_ms;
    app.regional.update(thread, |o| {
        o.manual_until_ms = now_ms() + hold;
        o.picked_at_ms = now_ms();
        o.pending = None;
    });
    tracing::info!(
        "regional: {} moves {thread} to {}",
        counter.username,
        target.as_deref().unwrap_or("the VPS")
    );
    if perform(app, thread, &h, target, Some(counter.uuid.clone()))
        .await
        .is_err()
    {
        refuse("failed");
    }
}

async fn perform(
    app: &App,
    thread: &str,
    h: &ThreadHandle,
    target: Option<String>,
    counter: Option<String>,
) -> Result<(), ()> {
    broadcast(app, thread).await;
    let from = app.regional.get(thread).region;
    let started = Instant::now();
    let outcome = relocate(app, thread, h, target.clone()).await;
    let mut sample = Sample {
        at: now_ms(),
        kind: "move",
        thread: Some(thread.to_owned()),
        region: target.clone(),
        counter,
        epoch: Some(app.regional.get(thread).epoch),
        ms: Some(started.elapsed().as_secs_f64() * 1000.0),
        ..Default::default()
    };
    let route = format!(
        "{}>{}",
        from.as_deref().unwrap_or("vps"),
        target.as_deref().unwrap_or("vps")
    );
    let moved = match outcome {
        Ok(colo) => {
            sample.colo = colo;
            sample.detail = Some(route);
            Ok(())
        }
        Err(e) => {
            tracing::warn!("regional: moving {thread} failed: {e:#}");
            sample.detail = Some(format!("{route} failed: {e}").chars().take(255).collect());
            Err(())
        }
    };
    app.regional.samples.record(sample);
    app.regional.update(thread, |o| o.moving = false);
    broadcast(app, thread).await;
    moved
}

pub async fn cancel_pending(app: &App, s: &SocketRef, data: &Value) {
    let Some(thread) = data.get("thread_name").and_then(Value::as_str) else {
        return;
    };
    let Some(counter) = counter_of(app, s).await else {
        return;
    };
    let Some(h) = app.thread(thread) else { return };
    if !h.meta.read().may_post(&counter) {
        return;
    }
    let snooze = app.cfg.regional.snooze_ms;
    let cancelled = {
        let mut owners = app.regional.owners.lock();
        owners.get_mut(thread).is_some_and(|o| {
            let had = o.pending.take().is_some();
            if had {
                o.snoozed_until_ms = now_ms() + snooze;
            }
            had
        })
    };
    if cancelled {
        tracing::info!("regional: {} cancels the automatic move of {thread}", counter.username);
        broadcast(app, thread).await;
    }
}

fn place_of(region: &Option<String>) -> &str {
    region.as_deref().unwrap_or(crate::auto::VPS)
}

fn region_of(place: &str) -> Option<String> {
    (place != crate::auto::VPS).then(|| place.to_owned())
}

fn recent_authors(h: &ThreadHandle, post_now_ms: f64) -> Vec<String> {
    h.ring
        .read()
        .iter()
        .rev()
        .take(RECENT_POSTS)
        .filter(|p| p.author_uuid != "[SYSTEM]")
        .filter(|p| post_now_ms - counting::js::parse_float(&p.timestamp) <= RECENT_WINDOW_MS)
        .map(|p| p.author_uuid.clone())
        .collect()
}

fn wanted(app: &App, h: &ThreadHandle, current: &Option<String>) -> Option<Option<String>> {
    let authors = recent_authors(h, app.clock.now_ms());
    if authors.is_empty() || authors.len() < app.cfg.regional.auto_min_posts {
        return None;
    }
    let latency = app.regional.latencies(now_ms(), &authors);
    let shares = crate::auto::shares(authors.iter().map(String::as_str));
    let places: Vec<&str> = std::iter::once(crate::auto::VPS).chain(REGIONS).collect();
    let costs = crate::auto::costs(&shares, &latency, &places);
    crate::auto::choose(place_of(current), &costs).map(|p| region_of(&p))
}

async fn auto_move(app: &App, thread: &str, h: &ThreadHandle, target: Option<String>) {
    if claim_move(app, thread, h, &target, Mover::Auto).is_err() {
        return;
    }
    tracing::info!(
        "regional: auto moves {thread} to {}",
        target.as_deref().unwrap_or("the VPS")
    );
    let _ = perform(app, thread, h, target, None).await;
}

async fn auto_step(app: &App, thread: &str) {
    let cfg = &app.cfg.regional;
    let Some(h) = app.thread(thread) else { return };
    if !cfg.movable(thread) || !portable(&h.meta.read()) {
        return;
    }
    let o = app.regional.get(thread);
    if o.moving {
        return;
    }
    let now = now_ms();
    if let Some(p) = o.pending.clone() {
        let still = !auto_paused(&o, now, cfg) && wanted(app, &h, &o.region) == Some(p.target.clone());
        if !still {
            app.regional.update(thread, |o| o.pending = None);
            return broadcast(app, thread).await;
        }
        if now >= p.at_ms {
            app.regional.update(thread, |o| o.pending = None);
            auto_move(app, thread, &h, p.target).await;
        }
        return;
    }
    if auto_paused(&o, now, cfg) {
        return;
    }
    let post_now = app.clock.now_ms() as i64;
    if last_post_ms(&h).is_none_or(|t| post_now - t > cfg.auto_idle_ms) {
        if o.region.is_some() {
            auto_move(app, thread, &h, None).await;
        }
        return;
    }
    if now - o.moved_at_ms < cfg.auto_cooldown_ms || !app.regional.regions_open(cfg.daily_post_limit) {
        return;
    }
    if let Some(target) = wanted(app, &h, &o.region) {
        let at_ms = now + cfg.auto_countdown_ms;
        app.regional
            .update(thread, |o| o.pending = Some(Pending { target, at_ms }));
        broadcast(app, thread).await;
    }
}

pub async fn finish_moves(app: &App) {
    app.regional.stopping.store(true, std::sync::atomic::Ordering::Relaxed);
    let deadline = Instant::now() + FINISH_MOVES;
    while app.regional.any_moving() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    if app.regional.any_moving() {
        tracing::warn!("regional: stopping with a move still unfinished after {FINISH_MOVES:?}");
    }
}

pub fn start_auto(app: Arc<App>) {
    if !app.cfg.regional.auto {
        return;
    }
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(AUTO_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            for thread in app.cfg.regional.threads.clone() {
                auto_step(&app, &thread).await;
            }
        }
    });
}

fn claim_reclaim(app: &App, thread: &str) -> Option<String> {
    if app.regional.stopping() {
        return None;
    }
    let mut owners = app.regional.owners.lock();
    let o = owners.get_mut(thread)?;
    if o.moving || o.region.is_none() {
        return None;
    }
    o.moving = true;
    o.target = None;
    o.region.clone()
}

pub async fn reclaim(app: &App, thread: &str, reason: &str) {
    let Some(region) = claim_reclaim(app, thread) else {
        return;
    };
    tracing::warn!("regional: taking {thread} back from {region}: {reason}");
    broadcast(app, thread).await;
    let started = Instant::now();
    let outcome = match app.thread(thread) {
        Some(h) => relocate(app, thread, &h, None).await.map(|_| ()),
        None => Err(anyhow::anyhow!("no thread")),
    };
    if let Err(e) = &outcome {
        tracing::warn!("regional: taking {thread} back failed: {e:#}");
    }
    app.regional.samples.record(Sample {
        at: now_ms(),
        kind: "move",
        thread: Some(thread.to_owned()),
        epoch: Some(app.regional.get(thread).epoch),
        ms: Some(started.elapsed().as_secs_f64() * 1000.0),
        detail: Some(
            format!(
                "{region}>vps reclaim: {reason}{}",
                outcome.err().map_or(String::new(), |e| format!(" failed: {e}"))
            )
            .chars()
            .take(255)
            .collect(),
        ),
        ..Default::default()
    });
    app.regional.failed_checks.lock().remove(thread);
    app.regional.update(thread, |o| o.moving = false);
    broadcast(app, thread).await;
}

async fn reclaim_all(app: &App, reason: &str) {
    for thread in app.regional.remote_threads() {
        reclaim(app, &thread, reason).await;
    }
}

enum Health {
    Fine,
    Unreachable,
    Stuck(u64),
}

async fn health(app: &App, thread: &str) -> Health {
    let o = app.regional.get(thread);
    let Some(region) = o.region else { return Health::Fine };
    let status = call_worker(
        app,
        "/admin/status",
        json!({ "thread": thread, "region": region, "epoch": o.epoch }),
    )
    .await;
    match status {
        Ok(reply) if reply["owning"].as_bool() != Some(true) => Health::Unreachable,
        Ok(reply) => match reply["stuckMs"].as_u64().unwrap_or(0) {
            ms if ms >= app.cfg.regional.stall_ms => Health::Stuck(ms),
            _ => Health::Fine,
        },
        Err(e) => {
            tracing::warn!("regional: {region} status for {thread}: {e:#}");
            Health::Unreachable
        }
    }
}

async fn check(app: &App, thread: &str, failures_allowed: u32) {
    match health(app, thread).await {
        Health::Fine => {
            app.regional.failed_checks.lock().remove(thread);
            return;
        }
        Health::Stuck(ms) => {
            let reason = format!("no way back to the VPS for {} s", ms / 1000);
            return reclaim(app, thread, &reason).await;
        }
        Health::Unreachable => {}
    }
    let failures = {
        let mut failed = app.regional.failed_checks.lock();
        let n = failed.entry(thread.to_owned()).or_default();
        *n += 1;
        *n
    };
    if failures >= failures_allowed {
        reclaim(app, thread, "unreachable").await;
    }
}

pub fn start_health(app: Arc<App>) {
    let every = app.cfg.regional.health_every;
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(every);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            if !app.regional.regions_open(app.cfg.regional.daily_post_limit) {
                reclaim_all(&app, "daily budget").await;
                continue;
            }
            for thread in app.regional.remote_threads() {
                check(&app, &thread, FAILED_CHECKS_BEFORE_RECLAIM).await;
            }
        }
    });
}

pub async fn report_unreachable(app: &App, s: &SocketRef, data: &Value) {
    let Some(thread) = data.get("thread_name").and_then(Value::as_str) else {
        return;
    };
    if !app.cfg.regional.movable(thread) || counter_of(app, s).await.is_none() {
        return;
    }
    let now = now_ms();
    {
        let mut last = app.regional.last_report_check.lock();
        let at = last.entry(thread.to_owned()).or_default();
        if now - *at < REPORT_CHECK_EVERY_MS {
            return;
        }
        *at = now;
    }
    check(app, thread, 1).await;
}

pub fn landed(ring: &VecDeque<Arc<Post>>, author: &str, asked: &[(String, String)]) -> Vec<String> {
    asked
        .iter()
        .filter(|(_, text)| {
            let text = counting::js::trim(text);
            ring.iter().any(|p| p.author_uuid == author && p.raw_text == text)
        })
        .map(|(hash, _)| hash.clone())
        .collect()
}

pub async fn post_status(app: &App, s: &SocketRef, data: &Value) {
    let Some(thread) = data.get("thread_name").and_then(Value::as_str) else {
        return;
    };
    let Some(counter) = counter_of(app, s).await else {
        return;
    };
    let Some(h) = app.thread(thread) else { return };
    let asked: Vec<(String, String)> = data
        .get("posts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(STATUS_MAX_POSTS)
        .filter_map(|p| Some((p.get("h")?.as_str()?.to_owned(), p.get("text")?.as_str()?.to_owned())))
        .collect();
    let known = landed(&h.ring.read(), &counter.uuid, &asked);
    s.emit("post_status", &json!({ "thread_name": thread, "known": known }))
        .ok();
}

pub async fn client_sample(app: &App, s: &SocketRef, data: &Value) {
    let Some(counter) = counter_of(app, s).await else {
        return;
    };
    let now = now_ms();
    if let Some(sample) = crate::regional_log::from_client(data, &counter.uuid, now) {
        if let (Some(place), Some(ms)) = (&sample.region, sample.ms)
            && matches!(sample.kind, "probe" | "ping" | "ack")
        {
            app.regional.note_latency(&counter.uuid, place, ms, now);
        }
        app.regional.samples.record(sample);
    }
}

async fn release(app: &App, thread: &str, region: &str, epoch: u64) -> anyhow::Result<()> {
    let released = call_worker(
        app,
        "/admin/release",
        json!({ "thread": thread, "region": region, "epoch": epoch }),
    )
    .await?;
    wait_for_seq(app, thread, released["lastSeq"].as_u64().unwrap_or(0)).await
}

async fn take_back(app: &App, thread: &str) {
    let _serial = app.regional.ingest_lock.lock().await;
    let o = app.regional.update(thread, |o| {
        o.region = None;
        o.epoch += 1;
        o.last_seq = 0;
        o.moved_at_ms = now_ms();
    });
    save(app, thread, &o).await;
}

async fn relocate(app: &App, thread: &str, h: &ThreadHandle, target: Option<String>) -> anyhow::Result<Option<String>> {
    let moved = shift(app, thread, h, target).await;
    if app.regional.get(thread).region.is_none() {
        app.regional.hand_to_actor(thread, h, true);
    }
    moved
}

async fn bring_back(app: &App, thread: &str, h: &ThreadHandle) -> anyhow::Result<()> {
    let current = app.regional.get(thread);
    if let Some(region) = &current.region {
        if let Err(e) = release(app, thread, region, current.epoch).await {
            tracing::warn!("regional: {region} is slow to release {thread} ({e:#}); collecting its last posts");
            match collect_unforwarded(app, thread, region, current.epoch).await {
                Ok(()) => {
                    take_back(app, thread).await;
                    settle(h).await?;
                    return Ok(());
                }
                Err(e) => tracing::warn!("regional: collecting {thread}'s last posts from {region} failed: {e:#}"),
            }
            if let Err(e) = await_stragglers(app, thread, region, current.epoch).await {
                tracing::warn!("regional: taking {thread} back from {region} without its last posts: {e:#}");
            }
        }
        take_back(app, thread).await;
        settle(h).await?;
    }
    Ok(())
}

async fn collect_unforwarded(app: &App, thread: &str, region: &str, epoch: u64) -> anyhow::Result<()> {
    let reply = call_worker(
        app,
        "/admin/unforwarded",
        json!({ "thread": thread, "region": region, "epoch": epoch }),
    )
    .await?;
    let last = reply["lastSeq"].as_u64().unwrap_or(0);
    let posts: Vec<IngestPost> = serde_json::from_value(reply["posts"].clone())?;
    let reached = accept_posts(app, thread, epoch, posts)
        .await
        .map_err(|(status, body)| anyhow::anyhow!("{status}: {body}"))?;
    if reached < last {
        anyhow::bail!("{region} decided up to {last} but only {reached} arrived");
    }
    Ok(())
}

async fn await_stragglers(app: &App, thread: &str, region: &str, epoch: u64) -> anyhow::Result<()> {
    let mut unreachable = 0;
    for _ in 0..STRAGGLER_CHECKS {
        let status = call_worker(
            app,
            "/admin/status",
            json!({ "thread": thread, "region": region, "epoch": epoch }),
        )
        .await;
        match status {
            Ok(reply) if reply["owning"].as_bool() == Some(true) => {
                unreachable = 0;
                if release(app, thread, region, epoch).await.is_ok() {
                    return Ok(());
                }
            }
            Ok(reply) => {
                unreachable = 0;
                if wait_for_seq(app, thread, reply["lastSeq"].as_u64().unwrap_or(0))
                    .await
                    .is_ok()
                {
                    return Ok(());
                }
            }
            Err(e) => {
                unreachable += 1;
                if unreachable >= UNREACHABLE_BEFORE_GIVING_UP {
                    return Err(e);
                }
            }
        }
    }
    anyhow::bail!("its posts never arrived")
}

fn passed(j: &PostJob) -> Value {
    json!({
        "author": j.counter.uuid,
        "text": j.text,
        "hash": j.post_hash,
        "m": [j.macro_preset_id, j.macro_preset_version_id],
    })
}

pub async fn pass_on(app: &App, thread: &str, jobs: &[PostJob]) -> anyhow::Result<()> {
    let _in_order = app.regional.passing.lock().await;
    let o = app.regional.get(thread);
    let region = o.region.ok_or_else(|| anyhow::anyhow!("{thread} has no region"))?;
    let reply = call_worker(
        app,
        "/admin/posts",
        json!({ "thread": thread, "region": region, "epoch": o.epoch, "posts": jobs.iter().map(passed).collect::<Vec<_>>() }),
    )
    .await?;
    tell_rejected(app, thread, &reply).await;
    Ok(())
}

#[derive(Deserialize)]
struct LatePost {
    author: String,
    text: String,
    hash: Option<String>,
    m: Option<Vec<Option<i64>>>,
}

async fn accept_late(app: &App, thread: &str, posts: Vec<LatePost>) {
    let Some(h) = app.thread(thread) else { return };
    for p in posts {
        let Some(counter) = app.counter(&p.author).await else {
            continue;
        };
        let m = p.m.unwrap_or_default();
        let job = PostJob {
            counter,
            text: p.text,
            post_hash: p.hash.map(Value::String),
            macro_preset_id: m.first().copied().flatten(),
            macro_preset_version_id: m.get(1).copied().flatten(),
            received: Instant::now(),
            origin: Origin::Socket,
        };
        match app.regional.admit(thread, job) {
            Admitted::Here(job) => {
                if app.regional.first_time(thread, job.post_hash.as_ref()) {
                    let _ = h.tx.send(Msg::Post(job)).await;
                }
            }
            Admitted::Waiting => {}
            Admitted::Elsewhere(job) => {
                if let Err(e) = pass_on(app, thread, std::slice::from_ref(&*job)).await {
                    tracing::warn!("regional: passing a late post in {thread} on: {e:#}");
                }
            }
        }
    }
}

async fn shift(app: &App, thread: &str, h: &ThreadHandle, target: Option<String>) -> anyhow::Result<Option<String>> {
    bring_back(app, thread, h).await?;
    let Some(region) = target else { return Ok(None) };
    app.regional.hand_to_actor(thread, h, false);
    let (meta, book) = snapshot(h).await?;
    let epoch = app.regional.get(thread).epoch + 1;
    let adopted = call_worker(
        app,
        "/admin/adopt",
        json!({
            "thread": thread,
            "region": region,
            "epoch": epoch,
            "meta": meta.to_json(),
            "book": book,
            "hashes": app.regional.known_hashes(thread),
            "limits": limits(app),
        }),
    )
    .await?;
    if adopted["ok"].as_bool() != Some(true) {
        anyhow::bail!(
            "{region} refused {thread}: {}",
            adopted["reason"].as_str().unwrap_or("could not load it")
        );
    }
    let o = app.regional.update(thread, |o| {
        o.region = Some(region.clone());
        o.epoch = epoch;
        o.last_seq = 0;
        o.moved_at_ms = now_ms();
    });
    save(app, thread, &o).await;
    if let Err(e) = pass_waiting(app, thread, &region, epoch).await {
        bring_back(app, thread, h).await?;
        return Err(e);
    }
    Ok(adopted["colo"].as_str().map(str::to_owned))
}

async fn pass_waiting(app: &App, thread: &str, region: &str, epoch: u64) -> anyhow::Result<()> {
    for round in 1..=HANDOFF_ROUNDS {
        let (jobs, done) = app.regional.hand_over(thread, round == HANDOFF_ROUNDS);
        if jobs.is_empty() {
            return Ok(());
        }
        let posts: Vec<Value> = jobs.iter().map(passed).collect();
        let passed = call_worker(
            app,
            "/admin/posts",
            json!({ "thread": thread, "region": region, "epoch": epoch, "posts": posts }),
        )
        .await;
        match passed {
            Ok(reply) => tell_rejected(app, thread, &reply).await,
            Err(e) => {
                app.regional.update(thread, |o| o.moving = true);
                app.regional.put_back(thread, jobs);
                return Err(e);
            }
        }
        if done {
            return Ok(());
        }
    }
    Ok(())
}

async fn tell_rejected(app: &App, thread: &str, reply: &Value) {
    for r in reply["rejected"].as_array().into_iter().flatten() {
        let Some(author) = r["author"].as_str() else { continue };
        app.to_counter(
            author,
            "post_rejected",
            &json!({ "thread_name": thread, "post_hash": r["hash"], "reason": r["reason"] }),
        )
        .await;
    }
}

fn portable(meta: &ThreadMeta) -> bool {
    !matches!(
        counting::ValidationType::from_name(&meta.validation_type),
        counting::ValidationType::Randomhour | counting::ValidationType::Randomminute
    )
}

fn limits(app: &App) -> Value {
    json!({ "burstPerSec": app.cfg.post_burst_per_sec, "on": app.cfg.rate_limits })
}

async fn snapshot(h: &ThreadHandle) -> anyhow::Result<(ThreadMeta, Book)> {
    let (reply, taken) = tokio::sync::oneshot::channel();
    h.tx.send(Msg::Snapshot(reply))
        .await
        .map_err(|_| anyhow::anyhow!("thread actor is gone"))?;
    Ok(tokio::time::timeout(Duration::from_secs(5), taken).await??)
}

async fn owner_when_settled(app: &App, thread: &str) -> Option<(String, u64)> {
    for _ in 0..100 {
        let o = app.regional.get(thread);
        if !o.moving {
            return o.region.map(|r| (r, o.epoch));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    None
}

pub async fn meta_changed(app: &App, thread: &str) {
    let Some((region, epoch)) = owner_when_settled(app, thread).await else {
        return;
    };
    let Some(h) = app.thread(thread) else { return };
    let meta = h.meta.read().to_json();
    let pushed = call_worker(
        app,
        "/admin/meta",
        json!({ "thread": thread, "region": region, "epoch": epoch, "meta": meta }),
    )
    .await;
    if let Err(e) = pushed {
        tracing::warn!("regional: telling {region} about {thread}'s new settings: {e:#}");
    }
}

pub async fn counter_changed(app: &App, c: &Counter) {
    for thread in app.regional.remote_threads() {
        let o = app.regional.get(&thread);
        let Some(region) = o.region else { continue };
        let pushed = call_worker(
            app,
            "/admin/counter",
            json!({ "thread": thread, "region": region, "epoch": o.epoch, "counter": &c.fields }),
        )
        .await;
        if let Err(e) = pushed {
            tracing::warn!("regional: telling {region} about {}: {e:#}", c.username);
        }
    }
}

#[derive(Deserialize)]
struct Pull {
    thread: Option<String>,
    epoch: Option<u64>,
    uuid: Option<String>,
    posts: Option<Vec<LatePost>>,
}

pub async fn pull(app: &App, headers: &HeaderMap, body: &[u8], what: &str) -> (StatusCode, Json<Value>) {
    let cfg = &app.cfg.regional;
    if cfg.secret.is_empty() || !verified(&cfg.secret, headers, body, now_ms()) {
        return reply(StatusCode::UNAUTHORIZED, json!({ "error": "bad_signature" }));
    }
    let Ok(asked) = serde_json::from_slice::<Pull>(body) else {
        return reply(StatusCode::BAD_REQUEST, json!({ "error": "bad_body" }));
    };
    match (what, asked) {
        (
            "late",
            Pull {
                thread: Some(thread),
                posts: Some(posts),
                ..
            },
        ) => {
            accept_late(app, &thread, posts).await;
            reply(StatusCode::OK, json!({ "ok": true }))
        }
        ("counter", Pull { uuid: Some(uuid), .. }) => match app.counter(&uuid).await {
            Some(c) => reply(StatusCode::OK, json!({ "counter": &c.fields })),
            None => reply(StatusCode::NOT_FOUND, json!({ "error": "no_counter" })),
        },
        (
            "snapshot",
            Pull {
                thread: Some(thread),
                epoch: Some(epoch),
                ..
            },
        ) => {
            let current = app.regional.get(&thread);
            if current.region.is_none() || current.epoch != epoch {
                return reply(
                    StatusCode::CONFLICT,
                    json!({ "error": "stale_epoch", "epoch": current.epoch }),
                );
            }
            let Some(h) = app.thread(&thread) else {
                return reply(StatusCode::NOT_FOUND, json!({ "error": "no_thread" }));
            };
            match snapshot(&h).await {
                Ok((meta, book)) => reply(
                    StatusCode::OK,
                    json!({
                        "meta": meta.to_json(),
                        "book": book,
                        "hashes": app.regional.known_hashes(&thread),
                        "limits": limits(app),
                        "lastSeq": current.last_seq,
                    }),
                ),
                Err(e) => reply(StatusCode::SERVICE_UNAVAILABLE, json!({ "error": e.to_string() })),
            }
        }
        _ => reply(StatusCode::BAD_REQUEST, json!({ "error": "bad_body" })),
    }
}

async fn settle(h: &ThreadHandle) -> anyhow::Result<()> {
    let (done, settled) = tokio::sync::oneshot::channel();
    h.tx.send(Msg::Barrier(done))
        .await
        .map_err(|_| anyhow::anyhow!("thread actor is gone"))?;
    tokio::time::timeout(Duration::from_secs(5), settled).await??;
    Ok(())
}

async fn wait_for_seq(app: &App, thread: &str, seq: u64) -> anyhow::Result<()> {
    for _ in 0..50 {
        if app.regional.get(thread).last_seq >= seq {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    anyhow::bail!("forwarded posts never reached seq {seq}")
}

#[derive(Deserialize)]
struct IngestBody {
    thread: String,
    epoch: u64,
    posts: Vec<IngestPost>,
}

#[derive(Deserialize)]
struct IngestPost {
    seq: u64,
    hash: Option<String>,
    at: Option<f64>,
    decision: Decision,
}

fn reply(status: StatusCode, body: Value) -> (StatusCode, Json<Value>) {
    (status, Json(body))
}

pub async fn ingest(app: &App, headers: &HeaderMap, body: &[u8]) -> (StatusCode, Json<Value>) {
    let cfg = &app.cfg.regional;
    if cfg.secret.is_empty() || !verified(&cfg.secret, headers, body, now_ms()) {
        return reply(StatusCode::UNAUTHORIZED, json!({ "error": "bad_signature" }));
    }
    let Ok(batch) = serde_json::from_slice::<IngestBody>(body) else {
        return reply(StatusCode::BAD_REQUEST, json!({ "error": "bad_body" }));
    };
    match accept_posts(app, &batch.thread, batch.epoch, batch.posts).await {
        Ok(last_seq) => reply(StatusCode::OK, json!({ "ok": true, "lastSeq": last_seq })),
        Err((status, body)) => reply(status, body),
    }
}

async fn accept_posts(
    app: &App,
    thread: &str,
    epoch: u64,
    mut posts: Vec<IngestPost>,
) -> Result<u64, (StatusCode, Value)> {
    let cfg = &app.cfg.regional;
    let _serial = app.regional.ingest_lock.lock().await;
    let current = app.regional.get(thread);
    if current.region.is_none() || current.epoch != epoch {
        return Err((
            StatusCode::CONFLICT,
            json!({ "error": "stale_epoch", "epoch": current.epoch }),
        ));
    }
    let Some(h) = app.thread(thread) else {
        return Err((StatusCode::NOT_FOUND, json!({ "error": "no_thread" })));
    };
    posts.sort_by_key(|p| p.seq);
    let mut last = current.last_seq;
    let mut accepted = 0u64;
    let arrived = now_ms();
    for post in posts.into_iter().filter(|p| p.seq > current.last_seq) {
        if post.seq != last + 1 {
            break;
        }
        let author = post.decision.post.author_uuid.clone();
        app.regional.samples.record(Sample {
            at: arrived,
            kind: "ingest",
            thread: Some(thread.to_owned()),
            region: current.region.clone(),
            counter: Some(author.clone()),
            epoch: Some(epoch),
            seq: Some(post.seq),
            ms: post.at.map(|accepted| arrived as f64 - accepted),
            ..Default::default()
        });
        accepted += 1;
        let hash = post.hash.map(Value::String);
        if !app.regional.first_time(thread, hash.as_ref()) {
            last = post.seq;
            continue;
        }
        if let Some(counter) = app.counter(&author).await {
            let decided = Decided {
                decision: post.decision,
                counter,
                post_hash: hash,
                received: Instant::now(),
                origin: Origin::Durable { epoch, seq: post.seq },
            };
            if h.tx.send(Msg::Decided(Box::new(decided))).await.is_err() {
                break;
            }
        }
        last = post.seq;
    }
    let o = app.regional.update(thread, |o| {
        if o.epoch == epoch {
            o.last_seq = o.last_seq.max(last);
        }
    });
    save(app, thread, &o).await;
    let limit = cfg.daily_post_limit;
    if app.regional.budget.lock().add(arrived, accepted, limit) {
        tracing::warn!("regional: {limit} posts through regions today; moving every thread back to the VPS");
    }
    Ok(o.last_seq)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signed(secret: &str, timestamp: i64, body: &[u8]) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("x-cgg-ts", timestamp.to_string().parse().unwrap());
        h.insert(
            "x-cgg-sig",
            signature(secret, &timestamp.to_string(), body).parse().unwrap(),
        );
        h
    }

    #[test]
    fn signatures_check_secret_body_and_age() {
        let body = br#"{"thread":"test2"}"#;
        let now = 1_800_000_000_000;
        assert!(verified("s", &signed("s", now, body), body, now));
        assert!(!verified("other", &signed("s", now, body), body, now));
        assert!(!verified("s", &signed("s", now, body), b"{}", now));
        assert!(!verified("s", &signed("s", now - 120_000, body), body, now));
        assert!(!verified("s", &HeaderMap::new(), body, now));
    }

    #[test]
    fn signature_matches_the_worker() {
        assert_eq!(
            signature("secret", "1700000000000", br#"{"thread":"test2","epoch":1}"#),
            "768b7fd0d480635040ca09c69f84528f1af3c86846418b36eb92087381f62f33"
        );
    }

    #[test]
    fn the_budget_closes_once_at_the_limit_and_reopens_the_next_utc_day() {
        let mut b = Budget::default();
        let day = 20_000 * DAY_MS;
        assert!(!b.add(day + 10, 2, 3));
        assert!(!b.spent(day + 10, 3));
        assert!(b.add(day + 20, 1, 3));
        assert!(b.spent(day + 20, 3));
        assert!(!b.add(day + 30, 5, 3));
        assert!(!b.spent(day + DAY_MS, 3));
        assert_eq!(b.posts, 0);
    }

    #[test]
    fn a_hash_is_new_only_once_until_it_ages_out() {
        let mut r = RecentHashes::default();
        assert!(r.first_time("a"));
        assert!(!r.first_time("a"));
        for i in 0..RECENT_HASHES {
            assert!(r.first_time(&i.to_string()));
        }
        assert!(r.first_time("a"));
    }

    #[test]
    fn the_newest_hashes_are_handed_over_oldest_first() {
        let mut r = RecentHashes::default();
        for h in ["a", "b", "c", "d"] {
            r.first_time(h);
        }
        assert_eq!(r.newest(2), ["c", "d"]);
        assert_eq!(r.newest(10), ["a", "b", "c", "d"]);
    }

    fn ring_post(author: &str, text: &str) -> Arc<Post> {
        Arc::new(Post {
            uuid: String::new(),
            timestamp: String::new(),
            time_since_last_count: String::new(),
            time_since_last_post: String::new(),
            raw_text: text.to_owned(),
            is_count: true,
            is_valid_count: true,
            valid_count_number: None,
            count_content: None,
            raw_count: None,
            stricken: false,
            thread: String::new(),
            thread_id: 0,
            has_comment: false,
            has_three_char_comment: false,
            comment: None,
            author_uuid: author.to_owned(),
            author_id: None,
            is_deleted: false,
            is_comment_deleted: false,
            reactions: Value::Null,
            macro_preset_id: None,
            macro_preset_version_id: None,
            id: 0,
        })
    }

    #[test]
    fn a_post_has_landed_only_if_the_same_author_posted_that_text() {
        let ring: VecDeque<Arc<Post>> = [ring_post("me", "41"), ring_post("you", "42")].into();
        let asked = [
            ("a".to_owned(), " 41 ".to_owned()),
            ("b".to_owned(), "42".to_owned()),
            ("c".to_owned(), "43".to_owned()),
        ];
        assert_eq!(landed(&ring, "me", &asked), vec!["a".to_owned()]);
    }

    #[test]
    fn only_known_regions_are_targets() {
        assert_eq!(target_region("auto"), Ok(None));
        assert_eq!(target_region("vps"), Ok(None));
        assert_eq!(target_region("weur"), Ok(Some("weur".to_owned())));
        assert_eq!(target_region("mars"), Err("bad_region"));
    }
}
