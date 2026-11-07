use crate::app::{App, now_ms};
use crate::model::CounterRef;
use crate::thread::{Msg, PostJob};
use crate::wal::Origin;
use crate::{features, history, misc, rank, shuffle};
use cgg_web::session as auth;
use serde_json::{Value, json};
use socketioxide::extract::{SocketRef, TryData};
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

#[derive(Clone)]
struct WatchingChat;

#[derive(Clone)]
pub struct Identity {
    pub user: Option<String>,
    pub ip: String,
    pub has_cookie: bool,
    pub counts_toward_ip_cap: bool,
}

pub fn user_of(s: &SocketRef) -> Option<String> {
    s.extensions.get::<Identity>().and_then(|i| i.user.clone())
}

pub fn ip_of(s: &SocketRef) -> String {
    s.extensions.get::<Identity>().map(|i| i.ip.clone()).unwrap_or_default()
}

pub async fn counter_of(app: &App, s: &SocketRef) -> Option<CounterRef> {
    match user_of(s) {
        Some(u) => app.counter(&u).await,
        None => None,
    }
}

#[derive(Default)]
pub struct Limits {
    hits: VecDeque<Instant>,
}

impl Limits {
    fn admit(&mut self, now: Instant, window: Duration, max: usize) -> bool {
        while self.hits.front().is_some_and(|t| now - *t >= window) {
            self.hits.pop_front();
        }
        if self.hits.len() >= max {
            return false;
        }
        self.hits.push_back(now);
        true
    }
}

#[derive(Clone, Copy)]
pub enum Guard {
    Ws,
    Burst,
    PostBurst,
    Misc,
}

fn admit(app: &App, s: &SocketRef, event: &str, guards: &[Guard]) -> bool {
    if !app.cfg.rate_limits {
        return true;
    }
    let ip = format!("ip:{}", ip_of(s));
    let tiers: Vec<(String, usize)> = match user_of(s) {
        Some(u) => vec![(format!("u:{u}"), 1), (ip, app.cfg.rate_limit_ip_factor)],
        None => vec![(ip, 1)],
    };
    let now = Instant::now();
    for g in guards {
        let (window, max, scope) = match g {
            Guard::Ws => (Duration::from_secs(60), 1500, Some("ws")),
            Guard::Burst => (Duration::from_secs(1), 15, Some("burst")),
            Guard::PostBurst => (Duration::from_secs(1), app.cfg.post_burst_per_sec, Some("burst")),
            Guard::Misc => (Duration::from_secs(60), 610, None),
        };
        for (who, factor) in &tiers {
            let key = match g {
                Guard::Ws => format!("{who}|{event}"),
                Guard::Burst | Guard::PostBurst => format!("{who}_burst|{event}"),
                Guard::Misc => format!("{who}|{event}|100ms"),
            };
            let ok = app
                .limits
                .entry(key)
                .or_default()
                .lock()
                .admit(now, window, max * factor);
            if !ok {
                app.metrics.rate_limited.fetch_add(1, Ordering::Relaxed);
                if let Some(scope) = scope {
                    s.emit("rate_limited", &json!({ "scope": scope })).ok();
                }
                return false;
            }
        }
    }
    true
}

pub fn admit_ws(app: &App, s: &SocketRef, event: &str) -> bool {
    admit(app, s, event, &[Guard::Ws])
}

pub fn sweep_limits(app: &App) {
    let now = Instant::now();
    app.limits.retain(|_, l| {
        l.lock()
            .hits
            .back()
            .is_some_and(|t| now - *t < Duration::from_secs(120))
    });
}

fn client_ip(s: &SocketRef, hops: usize) -> String {
    let parts = s.req_parts();
    let peer = parts
        .extensions
        .get::<axum::extract::ConnectInfo<SocketAddr>>()
        .map(|c| c.0.ip().to_string())
        .unwrap_or_else(|| "none".into());
    if hops == 0 {
        return peer;
    }
    let Some(fwd) = parts.headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) else {
        return peer;
    };
    let chain: Vec<&str> = fwd.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
    match chain.len().checked_sub(hops).and_then(|i| chain.get(i)) {
        Some(c) if c.len() < 64 => c.to_string(),
        _ => peer,
    }
}

fn log_ip_choice(s: &SocketRef, chosen: &str, hops: usize) {
    static SEEN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    static LIMIT: std::sync::LazyLock<u32> = std::sync::LazyLock::new(|| {
        std::env::var("IP_DEBUG_CONNECTIONS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(50)
    });
    if SEEN.fetch_add(1, Ordering::Relaxed) >= *LIMIT {
        return;
    }
    let parts = s.req_parts();
    let h = |k: &str| {
        parts
            .headers
            .get(k)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("-")
            .to_owned()
    };
    let peer = parts
        .extensions
        .get::<axum::extract::ConnectInfo<SocketAddr>>()
        .map(|c| c.0.ip().to_string())
        .unwrap_or("-".into());
    tracing::info!(
        "client ip: chosen={chosen} hops={hops} peer={peer} x-forwarded-for=[{}] cf-connecting-ip={} x-real-ip={}",
        h("x-forwarded-for"),
        h("cf-connecting-ip"),
        h("x-real-ip"),
    );
}

pub async fn authenticate(s: SocketRef, app: Arc<App>) -> Result<(), String> {
    let ip = client_ip(&s, app.cfg.trust_proxy_hops);
    log_ip_choice(&s, &ip, app.cfg.trust_proxy_hops);
    let counts_toward_ip_cap = !is_local_or_unknown(&ip);
    if counts_toward_ip_cap && !take_ip_slot(&app, &ip) {
        app.metrics.rate_limited.fetch_add(1, Ordering::Relaxed);
        return Err("too_many_connections".into());
    }
    let headers = &s.req_parts().headers;
    let origin = headers.get("origin").and_then(|v| v.to_str().ok());
    let cookie = headers
        .get("cookie")
        .and_then(|v| v.to_str().ok())
        .filter(|_| auth::origin_allowed(&app.cfg.cors_origins, origin))
        .map(str::to_owned);
    let mut ident = Identity {
        user: None,
        ip,
        has_cookie: cookie.is_some(),
        counts_toward_ip_cap,
    };
    if let Some(cookie) = cookie {
        ident.user = auth::resolve_user(&app.pool, &cookie, &app.cfg.cookie_secret).await;
        if let Some(u) = &ident.user {
            app.counter(u).await;
        }
    }
    s.extensions.insert(ident);
    Ok(())
}

macro_rules! on {
    ($s:expr, $app:expr, $ev:literal, [$($g:expr),*], |$a:ident, $so:ident, $d:ident| $body:expr) => {{
        let app = $app.clone();
        $s.on($ev, move |$so: SocketRef, TryData::<Value>(data): TryData<Value>| {
            let $a = app.clone();
            async move {
                if !admit(&$a, &$so, $ev, &[$($g),*]) {
                    return;
                }
                #[allow(unused_variables)]
                let $d = data.unwrap_or(Value::Null);
                $body;
            }
        });
    }};
}

async fn rank_live(app: &App, s: &SocketRef, d: &Value) {
    let Some(on) = d.as_bool() else { return };
    let Some(u) = user_of(s) else { return };
    let Some(c) = app.counter(&u).await else { return };
    let room = format!("rank_{}", c.uuid);
    if on { s.join(room) } else { s.leave(room) }
}

pub async fn on_connect(s: SocketRef, app: Arc<App>) {
    app.metrics.sockets.fetch_add(1, Ordering::Relaxed);
    if let Some(ident) = s.extensions.get::<Identity>() {
        if let Some(u) = &ident.user {
            {
                let mut o = app.online.lock();
                let e = o.users.entry(u.clone()).or_insert_with(|| (Vec::new(), now_ms()));
                e.0.push(s.id.to_string());
            }
            if let Some(c) = app.counter(u).await {
                s.join(format!("counter_{}", c.uuid));
                s.join(format!("rank_{}", c.uuid));
            }
        }
        if ident.has_cookie {
            s.emit("site_version", &app.cfg.site_version).ok();
        }
    }

    on!(s, app, "watch", [], |a, s, d| watch(&a, &s, &d).await);
    on!(s, app, "rank_live", [], |a, s, d| rank_live(&a, &s, &d).await);
    on!(s, app, "watchRock", [Guard::Misc], |a, s, d| misc::watch_rock(&a, &s)
        .await);
    on!(s, app, "rock", [Guard::Misc], |a, s, d| misc::rock(&a).await);
    on!(s, app, "watchSky", [Guard::Misc], |a, s, d| misc::watch_sky(&a, &s)
        .await);
    on!(s, app, "skyClick", [Guard::Misc], |a, s, d| misc::sky_click(&a, &d)
        .await);
    on!(s, app, "watch_rank_sitewide", [], |a, s, d| {
        s.join("thread_all");
        rank::emit_sitewide_initial(&a, &s).await
    });
    on!(s, app, "rob", [], |a, s, d| features::rob(&a, &s).await);
    on!(s, app, "updateCategories", [Guard::Ws], |a, s, d| {
        features::update_categories(&a, &s, &d).await
    });
    on!(s, app, "deleteComment", [Guard::Ws], |a, s, d| {
        features::delete_comment(&a, &s, &d).await
    });
    on!(s, app, "updateReactions", [Guard::Ws], |a, s, d| {
        features::update_reactions(&a, &s, &d).await
    });
    on!(s, app, "chat_watch", [Guard::Ws], |a, s, d| {
        let first_time = s.extensions.insert(WatchingChat).is_none();
        s.join("site_chat");
        if first_time {
            let n = a.io.within("site_chat").sockets().len();
            a.to("site_chat", "chat_watcher_count", &n).await;
        }
        if let Some(u) = user_of(&s) {
            let last = a.chat_last_read.get(&u).map(|v| v.clone());
            s.emit("chat_last_read", &last).ok();
        }
    });
    on!(s, app, "chat_mark_read", [], |a, s, d| {
        if let (Some(u), Some(p)) = (user_of(&s), d.as_str().filter(|p| !p.is_empty())) {
            a.chat_last_read.insert(u, p.to_owned());
        }
    });
    handle_posts_one_at_a_time_in_arrival_order(&s, &app);
    on!(s, app, "number_shuffle", [Guard::Ws, Guard::Burst], |a, s, d| {
        shuffle::play(&a, &s, &d).await
    });
    on!(s, app, "number_shuffle_games", [Guard::Ws, Guard::Burst], |a, s, d| {
        shuffle::games(&a, &s).await
    });
    on!(s, app, "generate_challenge", [Guard::Ws, Guard::Burst], |a, s, d| {});
    on!(s, app, "getOlder", [Guard::Ws], |a, s, d| history::page(
        &a,
        &s,
        &d,
        history::Dir::Older,
        false
    )
    .await);
    on!(s, app, "getNewer", [Guard::Ws], |a, s, d| history::page(
        &a,
        &s,
        &d,
        history::Dir::Newer,
        false
    )
    .await);
    on!(s, app, "getOlderChats", [Guard::Ws], |a, s, d| history::page(
        &a,
        &s,
        &d,
        history::Dir::Older,
        true
    )
    .await);
    on!(s, app, "getNewerChats", [Guard::Ws], |a, s, d| history::page(
        &a,
        &s,
        &d,
        history::Dir::Newer,
        true
    )
    .await);
    on!(s, app, "leave_threads", [Guard::Ws], |a, s, d| leave_prefixed(
        &s, "thread_"
    ));
    on!(s, app, "query_queue_leave", [Guard::Ws], |a, s, d| leave_prefixed(
        &s,
        "query_queue"
    ));
    on!(s, app, "leave_reddit", [Guard::Ws], |a, s, d| leave_prefixed(
        &s, "reddit_"
    ));
    on!(s, app, "reddit_submit", [], |a, s, d| features::reddit_submit(
        &a, &s, &d
    )
    .await);
    on!(s, app, "reddit_watch", [], |a, s, d| {
        if let Some(t) = d.as_str() {
            s.join(format!("reddit_{t}"));
        }
    });
    on!(s, app, "stats_splits", [], |a, s, d| features::stats_splits(&a, &s, &d)
        .await);
    on!(s, app, "query_queue", [], |a, s, d| s.join("query_queue"));
    on!(s, app, "lrwoedScore", [], |a, s, d| features::lrwoed_score(&a, &s, &d)
        .await);
    on!(s, app, "latency_ping", [], |a, s, d| {
        let t = d.get("t").filter(|t| t.is_number()).cloned().unwrap_or(Value::Null);
        let h = d.get("h").filter(|h| h.is_number()).cloned().unwrap_or(Value::Null);
        s.emit(
            "latency_pong",
            &json!({ "t": t, "s": crate::app::epoch_ms_f64(), "h": h }),
        )
        .ok()
    });
    on!(s, app, "client_latency", [], |a, s, d| features::client_latency(
        &a, &s, &d
    )
    .await);
    on!(s, app, "connection_check", [Guard::Misc], |a, s, d| {
        crate::connection::check(&a, &s, &d).await
    });
    on!(s, app, "client_health", [], |a, s, d| features::client_health(
        &a, &s, &d
    )
    .await);
    on!(s, app, "pad_debug", [], |a, s, d| features::pad_debug(&a, &s, &d));
    crate::bingo::register(&s, &app);

    s.on_disconnect({
        let app = app.clone();
        move |s: SocketRef, reason: socketioxide::socket::DisconnectReason| {
            let app = app.clone();
            async move {
                crate::connection::on_disconnect(&app, &s, reason).await;
                on_disconnect(&app, &s).await
            }
        }
    });
}

fn handle_posts_one_at_a_time_in_arrival_order(s: &SocketRef, app: &Arc<App>) {
    let (arrivals, mut queue) = tokio::sync::mpsc::unbounded_channel::<(SocketRef, Value)>();
    let drain = app.clone();
    tokio::spawn(async move {
        while let Some((s, d)) = queue.recv().await {
            post(&drain, &s, d).await;
        }
    });
    let a = app.clone();
    s.on("post", move |so: SocketRef, TryData::<Value>(data): TryData<Value>| {
        if admit(&a, &so, "post", &[Guard::Ws, Guard::PostBurst]) {
            let _ = arrivals.send((so, data.unwrap_or(Value::Null)));
        }
        std::future::ready(())
    });
}

fn leave_prefixed(s: &SocketRef, prefix: &str) {
    let rooms: Vec<_> = s.rooms().into_iter().filter(|r| r.starts_with(prefix)).collect();
    s.leave(rooms);
}

fn is_local_or_unknown(ip: &str) -> bool {
    ip.parse::<std::net::IpAddr>().map_or(true, |a| a.is_loopback())
}

fn take_ip_slot(app: &App, ip: &str) -> bool {
    let mut n = app.connections_per_ip.entry(ip.to_owned()).or_insert(0);
    if *n >= app.cfg.max_connections_per_ip {
        return false;
    }
    *n += 1;
    true
}

fn release_ip_slot(app: &App, ip: &str) {
    if let dashmap::mapref::entry::Entry::Occupied(mut e) = app.connections_per_ip.entry(ip.to_owned()) {
        *e.get_mut() -= 1;
        if *e.get() == 0 {
            e.remove();
        }
    }
}

async fn on_disconnect(app: &App, s: &SocketRef) {
    app.metrics.sockets.fetch_sub(1, Ordering::Relaxed);
    if let Some(ident) = s.extensions.get::<Identity>()
        && ident.counts_toward_ip_cap
    {
        release_ip_slot(app, &ident.ip);
    }
    crate::bingo::on_disconnect(s);
    features::client_health_gone(s);
    if let Some(u) = user_of(s) {
        let ended = {
            let mut o = app.online.lock();
            let sid = s.id.to_string();
            let mut ended = None;
            if let Some((socks, login)) = o.users.get_mut(&u) {
                socks.retain(|x| *x != sid);
                if socks.is_empty() {
                    ended = Some(now_ms() - *login);
                }
            }
            if ended.is_some() {
                o.users.remove(&u);
            }
            ended
        };
        if let Some(ms) = ended {
            features::add_time_online(app, &u, ms).await;
        }
    }
    if s.extensions.get::<WatchingChat>().is_some() {
        let n = app.io.within("site_chat").sockets().len();
        app.to("site_chat", "chat_watcher_count", &n).await;
    }
}

async fn post(app: &App, s: &SocketRef, data: Value) {
    let received = Instant::now();
    let Some(counter) = counter_of(app, s).await else {
        return;
    };
    let Some(name) = data.get("thread_name").and_then(Value::as_str) else {
        return;
    };
    let Some(text) = data.get("text").and_then(Value::as_str) else {
        return;
    };
    let Some(h) = app.thread(name) else { return };
    if !h.meta.read().visible_to_counter(Some(&counter)) {
        return;
    }
    let num = |v: Option<&Value>| v.and_then(Value::as_i64);
    let mm = data.get("macroMetadata");
    let job = PostJob {
        counter,
        text: text.to_owned(),
        post_hash: data.get("post_hash").filter(|v| !v.is_null()).cloned(),
        macro_preset_id: num(mm.and_then(|m| m.get("macroPresetId"))),
        macro_preset_version_id: num(mm.and_then(|m| m.get("macroPresetVersionId"))),
        received,
        origin: Origin::Socket,
    };
    if h.tx.send(Msg::Post(Box::new(job))).await.is_err() {
        tracing::error!("thread actor for {name} is gone");
    }
}

async fn watch(app: &App, s: &SocketRef, name: &Value) {
    let Some(name) = name.as_str() else { return };
    let counter = counter_of(app, s).await;
    if name == "all" {
        s.join("thread_all");
        if let Some(page) = features::default_page_socket(app) {
            s.emit("defaultPage", &page).ok();
        }
        return;
    }
    let Some(h) = app.thread(name) else { return };
    let (uuid, roulette, post_bans, is_main) = {
        let m = h.meta.read();
        if !m.visible_to_counter(counter.as_deref()) {
            return;
        }
        (
            m.uuid.clone(),
            m.validation_type == "roulette",
            m.post_bans.clone(),
            m.name == "main",
        )
    };
    let room = format!("thread_{uuid}");
    let stale: Vec<_> = s
        .rooms()
        .into_iter()
        .filter(|r| r.starts_with("thread_") && r != "thread_all" && *r != room)
        .collect();
    s.leave(stale);
    s.join(room.clone());
    let splits = features::cached_splits(app, &uuid).await;
    if !splits.is_empty() {
        s.emit("recent_splits", &splits).ok();
    }
    let watchers = app.io.within(room.clone()).sockets().len();
    app.to(room, "watcher_count", &watchers).await;
    let last = h.last_count.read().clone();
    if let Some((post, c)) = last {
        s.emit("lastCount", &json!({ "lastCount": &*post, "lastCounter": &c.json }))
            .ok();
        let (hoc, robs) = {
            let d = app.daily.lock();
            (
                d.threads.contains_key(&uuid).then(|| d.hoc(&uuid)),
                is_main.then(|| d.robs_json()),
            )
        };
        if let Some(hoc) = hoc {
            s.emit("dailyHOC", &hoc).ok();
            if let Some(r) = robs {
                s.emit("dailyRobs", &r).ok();
            }
        }
    }
    let bank = app.daily.lock().bank;
    if let Some(b) = bank {
        s.emit("bank", &b).ok();
    }
    if roulette {
        let cs = app.counters_of(&post_bans).await;
        let list: Vec<_> = cs.iter().map(|c| &c.json).collect();
        s.emit("postBans", &json!({ "counters": list })).ok();
    }
}
