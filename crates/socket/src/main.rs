#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

mod achievements;
mod app;
mod auto;
mod bingo;
mod bingo_window;
mod commands;
mod config;
mod connection;
mod cursor;
mod daily;
mod db;
mod diagnosis;
mod effects;
mod feats;
mod features;
mod gateway;
mod ggbot;
mod history;
mod ids;
mod lock;
mod metrics;
mod misc;
mod model;
mod monitor;
mod persisted;
mod persister;
mod rank;
mod regional;
mod regional_log;
mod shuffle;
mod splits;
mod sync;
mod thread;
mod wal;
mod writer_lock;

use anyhow::Result;
use app::App;
use axum::Router;
use axum::routing::{get, post};
use axum::serve::ListenerExt;
use cgg_web::nest;
use cursor::{Checkpoints, CursorFile, Position};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tower_http::cors::{AllowOrigin, CorsLayer};

#[tokio::main]
async fn main() -> Result<()> {
    cgg_core::load_env();
    cgg_core::init_logging("info,sqlx::query=warn");
    let cfg = config::Config::from_env()?;
    let t0 = Instant::now();
    tracing::info!(
        "cgg-socket: db {} wal {} ({:?}) tz {}",
        cfg.mysql.database,
        cfg.wal_dir.display(),
        cfg.wal_mode,
        cfg.server_tz
    );
    let pool = cfg.mysql.pool(24).await?;
    let redis = redis::aio::ConnectionManager::new(redis::Client::open(cfg.redis_url.as_str())?).await?;
    if std::env::args().nth(1).as_deref() == Some("seed-totals") {
        return daily::seed_totals(&pool, &mut redis.clone(), &cfg.mysql.database).await;
    }

    writer_lock::acquire(&cfg.mysql).await?.keep();

    let last_on_disk = wal::recover(&cfg.wal_dir)?;
    let checkpoint = persister::ensure_checkpoint_table(&pool).await?;
    let pstats = Arc::new(persister::PersistStats::default());
    let persisted = Arc::new(wal::Committed::new(checkpoint));
    let p = persister::Persister {
        pool: pool.clone(),
        dir: cfg.wal_dir.clone(),
        batch: cfg.persist_batch,
        stats: pstats.clone(),
        persisted: persisted.clone(),
    };
    let checkpoint = if last_on_disk > checkpoint {
        p.drain_backlog(checkpoint, last_on_disk).await?
    } else {
        checkpoint
    };
    pstats.checkpoint.store(checkpoint, Ordering::Relaxed);
    persisted.set(checkpoint);
    let wal_seq = last_on_disk.max(checkpoint);

    let max_id = db::max_post_id(&pool).await?;
    let effects_at = CursorFile::new(&cfg.wal_dir, "effects").load().unwrap_or(Position {
        seq: wal_seq,
        post_id: max_id,
    });
    let rank_at = CursorFile::new(&cfg.wal_dir, "rank").load().unwrap_or(Position {
        seq: wal_seq,
        post_id: max_id,
    });
    let bingo_at = CursorFile::new(&cfg.wal_dir, "bingo").load().unwrap_or(Position {
        seq: wal_seq,
        post_id: max_id,
    });
    let checkpoints = Arc::new(Checkpoints::default());
    let effects_cp = checkpoints.register("effects", effects_at.seq);
    let rank_cp = checkpoints.register("rank", rank_at.seq);
    let bingo_cp = checkpoints.register("bingo", bingo_at.seq);

    let threads = db::load_threads(&pool).await?;
    let metrics = Arc::new(metrics::Metrics::default());
    let (layer, io) = socketioxide::SocketIo::builder()
        .req_path(cfg.socket_path.clone())
        .ping_interval(Duration::from_secs(10))
        .ping_timeout(Duration::from_secs(10))
        .build_layer();
    let w = wal::Wal::start(
        wal::Options {
            dir: cfg.wal_dir.clone(),
            mode: cfg.wal_mode,
            sync_interval: cfg.wal_sync_interval,
            segment_bytes: cfg.wal_segment_bytes,
        },
        wal_seq,
        tokio::runtime::Handle::current(),
    )?;
    let clock = Arc::new(ids::Clock::default());
    let ctx = Arc::new(thread::Ctx {
        io: io.clone(),
        wal: w.clone(),
        clock: clock.clone(),
        next_id: AtomicU64::new(max_id + 1),
        server_tz: cfg.server_tz,
        game_secret: cfg.game_secret.clone(),
        metrics: metrics.clone(),
        app: Default::default(),
    });
    let app = Arc::new(App {
        cfg: cfg.clone(),
        pool: pool.clone(),
        io: io.clone(),
        redis,
        threads: Default::default(),
        threads_by_uuid: Default::default(),
        threads_by_id: Default::default(),
        counters: Default::default(),
        connections_per_ip: Default::default(),
        limits: Default::default(),
        metrics: metrics.clone(),
        bot: nest::Client::connect("bot", cfg.bot_addr.clone()),
        stats: nest::Client::connect("stats", cfg.stats_addr.clone()),
        online: Default::default(),
        daily: Default::default(),
        achievements: achievements::Achievements::spawn(pool.clone(), io.clone()),
        xp_total: AtomicI64::new(0),
        chat_last_read: Default::default(),
        day_best: Default::default(),
        run_best: Default::default(),
        thread_best: Default::default(),
        lifetime: Default::default(),
        anniversary_checked: Default::default(),
        rank_role_sent: Default::default(),
        misc: Default::default(),
        clock: clock.clone(),
        system_counter: Arc::new(model::Counter::system()),
        wal: Default::default(),
        regional: Default::default(),
    });
    let _ = app.wal.set(w.clone());
    let _ = ctx.app.set(Arc::downgrade(&app));

    let now_ms = ctx.clock.now_ms();
    let boots = join_all(
        threads
            .iter()
            .map(|t| Box::pin(db::thread_boot(&pool, t, now_ms)))
            .collect(),
    )
    .await;
    for (meta, boot) in threads.into_iter().zip(boots) {
        let boot = boot?;
        let lv_counter = match boot.recent_valid.first() {
            Some(p) => app.counter(&p.author_uuid).await,
            None => None,
        };
        let st = thread::boot_state(&meta, boot, lv_counter);
        let (name, uuid, id) = (meta.name.clone(), meta.uuid.clone(), meta.id);
        let h = thread::spawn(meta, st, ctx.clone());
        app.threads.insert(name, h.clone());
        app.threads_by_uuid.insert(uuid, h.clone());
        app.threads_by_id.insert(id, h);
    }

    regional::load(&app).await?;
    regional::start_health(app.clone());
    regional::start_auto(app.clone());
    let daily = daily::load(&app, effects_at.post_id).await?;
    *app.daily.lock() = daily;
    let xp: Option<i64> = sqlx::query_scalar("SELECT CAST(SUM(xp) AS SIGNED) FROM counters")
        .fetch_one(&pool)
        .await
        .ok()
        .flatten();
    app.xp_total.store(xp.unwrap_or(0), Ordering::Relaxed);
    let online: Option<f64> = sqlx::query_scalar("SELECT SUM(CAST(timeOnline AS DOUBLE)) FROM users")
        .fetch_one(&pool)
        .await
        .ok()
        .flatten();
    app.online.lock().total_ms = online.unwrap_or(0.0);
    misc::init(app.clone()).await;
    bingo::start(app.clone()).await;
    tracing::info!(
        "{} threads ready, next post id {}, wal seq {wal_seq}",
        app.threads.len(),
        max_id + 1
    );

    let committed = w.committed.clone();
    {
        let (committed, checkpoints) = (committed.clone(), checkpoints.clone());
        tokio::spawn(async move {
            if let Err(e) = p.run(committed, checkpoint, checkpoints).await {
                tracing::error!("persister stopped: {e:#}");
                std::process::exit(71);
            }
        });
    }
    tokio::spawn(effects::run(app.clone(), committed.clone(), effects_at, effects_cp));
    tokio::spawn(rank::run(app.clone(), persisted.clone(), rank_at, rank_cp));
    tokio::spawn(bingo::run(app.clone(), committed.clone(), bingo_at, bingo_cp));
    tokio::spawn(sync::run(app.clone(), ctx.clone()));

    {
        let app = app.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(60));
            loop {
                tick.tick().await;
                save_totals(&app).await;
                gateway::sweep_limits(&app);
            }
        });
    }

    monitor::start(app.clone(), pstats.clone());

    {
        let app = app.clone();
        let port = cfg.tcp_port;
        let handler: nest::Handler = Arc::new(move |cmd, data| {
            let app = app.clone();
            Box::pin(commands::handle(app, cmd, data))
        });
        tokio::spawn(async move {
            if let Err(e) = nest::serve(port, handler).await {
                tracing::error!("TCP command server: {e:#}");
            }
        });
    }

    {
        use socketioxide::handler::ConnectHandler;
        let (a1, a2) = (app.clone(), app.clone());
        let handler = move |s: socketioxide::extract::SocketRef| gateway::on_connect(s, a1.clone());
        let auth = move |s: socketioxide::extract::SocketRef| gateway::authenticate(s, a2.clone());
        io.ns("/", handler.with(auth));
    }

    let origins: Vec<_> = cfg.cors_origins.iter().filter_map(|o| o.parse().ok()).collect();
    let metrics_route = {
        let (app, pstats, checkpoints) = (app.clone(), pstats.clone(), checkpoints.clone());
        move || {
            let (app, pstats, checkpoints) = (app.clone(), pstats.clone(), checkpoints.clone());
            async move { render_metrics(&app, &pstats, &checkpoints) }
        }
    };
    let health_route = {
        let (app, pstats, checkpoints) = (app.clone(), pstats.clone(), checkpoints.clone());
        move || {
            let (app, pstats, checkpoints) = (app.clone(), pstats.clone(), checkpoints.clone());
            async move { health(&app, &pstats, &checkpoints) }
        }
    };
    let ingest_route = {
        let app = app.clone();
        move |headers: axum::http::HeaderMap, body: axum::body::Bytes| {
            let app = app.clone();
            async move { regional::ingest(&app, &headers, &body).await }
        }
    };
    let pull_route = |what: &'static str| {
        let app = app.clone();
        move |headers: axum::http::HeaderMap, body: axum::body::Bytes| {
            let app = app.clone();
            async move { regional::pull(&app, &headers, &body, what).await }
        }
    };
    let router = Router::new()
        .route("/metrics", get(metrics_route))
        .route("/healthz", get(health_route))
        .route("/do/ingest", post(ingest_route))
        .route("/do/counter", post(pull_route("counter")))
        .route("/do/snapshot", post(pull_route("snapshot")))
        .route("/do/late", post(pull_route("late")))
        .layer(layer)
        .layer(
            CorsLayer::new()
                .allow_origin(AllowOrigin::list(origins))
                .allow_credentials(true),
        );

    let v4 = tokio::net::TcpListener::bind(SocketAddr::from(([0, 0, 0, 0], cfg.port))).await?;
    let v6 = nest::bind_v6_only(std::net::Ipv6Addr::UNSPECIFIED, cfg.port).ok();
    tracing::info!(
        "listening on :{} (ipv6 {}) (boot {:.1}s)",
        cfg.port,
        v6.is_some(),
        t0.elapsed().as_secs_f64()
    );
    if let Some(v6) = v6 {
        let r = router.clone();
        let v6 = v6.tap_io(no_delay);
        tokio::spawn(async move { axum::serve(v6, r.into_make_service_with_connect_info::<SocketAddr>()).await });
    }
    {
        let (app, persisted) = (app.clone(), persisted.clone());
        tokio::spawn(async move {
            stop_signal().await;
            tracing::info!("shutting down: finishing moves, then flushing misc and totals");
            regional::finish_moves(&app).await;
            misc::flush(&app).await;
            save_totals(&app).await;
            let target = app.wal.get().map(|w| w.committed.get()).unwrap_or(0);
            let deadline = Instant::now() + Duration::from_secs(5);
            while persisted.get() < target && Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            let left = target.saturating_sub(persisted.get());
            if left == 0 {
                tracing::info!("stopped; MySQL holds everything (seq {target})");
            } else {
                tracing::warn!("stopped with {left} WAL records not yet in MySQL; the next start replays them");
            }
            std::process::exit(0);
        });
    }
    axum::serve(
        v4.tap_io(no_delay),
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

fn no_delay(tcp: &mut tokio::net::TcpStream) {
    let _ = tcp.set_nodelay(true);
}

async fn stop_signal() {
    let pm2 = async {
        match pm2_shutdown_message() {
            Some(rx) => {
                let _ = rx.await;
            }
            None => std::future::pending::<()>().await,
        }
    };
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = signal(SignalKind::terminate()).expect("SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
            _ = pm2 => tracing::info!("pm2 asked to shut down"),
        }
    }
    #[cfg(not(unix))]
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = pm2 => tracing::info!("pm2 asked to shut down"),
    }
}

fn pm2_shutdown_message() -> Option<tokio::sync::oneshot::Receiver<()>> {
    let fd: i32 = std::env::var("NODE_CHANNEL_FD").ok()?.parse().ok()?;
    let mut channel = ipc_channel(fd)?;
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::Builder::new()
        .name("pm2-ipc".into())
        .spawn(move || {
            use std::io::Read;
            let mut seen = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                match channel.read(&mut chunk) {
                    Ok(0) | Err(_) => return,
                    Ok(n) => seen.extend_from_slice(&chunk[..n]),
                }
                if contains_shutdown_payload_after_any_frame_header(&seen) {
                    let _ = tx.send(());
                    return;
                }
                if seen.len() > 64 {
                    seen.drain(..seen.len() - PM2_SHUTDOWN_PAYLOAD.len());
                }
            }
        })
        .ok()?;
    Some(rx)
}

const PM2_SHUTDOWN_PAYLOAD: &[u8] = b"\"shutdown\"";

fn contains_shutdown_payload_after_any_frame_header(bytes: &[u8]) -> bool {
    bytes
        .windows(PM2_SHUTDOWN_PAYLOAD.len())
        .any(|w| w == PM2_SHUTDOWN_PAYLOAD)
}

#[cfg(unix)]
fn ipc_channel(fd: i32) -> Option<std::fs::File> {
    use std::os::unix::io::FromRawFd;
    Some(unsafe { std::fs::File::from_raw_fd(fd) })
}

#[cfg(windows)]
fn ipc_channel(fd: i32) -> Option<std::fs::File> {
    use std::os::windows::io::FromRawHandle;
    let handle = unsafe { libc::get_osfhandle(fd) };
    (handle != -1).then(|| unsafe { std::fs::File::from_raw_handle(handle as _) })
}

async fn save_totals(app: &App) {
    let (counts, posts) = {
        let d = app.daily.lock();
        (d.total_counts, d.total_posts)
    };
    let (Some(counts), Some(max_id)) = (counts, posts) else {
        return;
    };
    let mut redis = app.redis.clone();
    let v = serde_json::json!({ "counts": counts, "maxId": max_id, "at": app::now_ms() }).to_string();
    let _: Result<(), _> = redis::cmd("SET")
        .arg(daily::totals_key(&app.cfg.mysql.database))
        .arg(v)
        .query_async(&mut redis)
        .await;
}

fn health(app: &App, pstats: &persister::PersistStats, checkpoints: &Checkpoints) -> (axum::http::StatusCode, String) {
    let limit = |k: &str, d: u64| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
    let committed = app.wal.get().map(|w| w.committed.get()).unwrap_or(0);
    let persist_lag = committed.saturating_sub(pstats.checkpoint.load(Ordering::Relaxed));
    let mut problems = Vec::new();
    if persist_lag > limit("HEALTH_MAX_PERSIST_LAG", 2_000) {
        problems.push(format!("mysql {persist_lag} records behind"));
    }
    let mut consumers = serde_json::Map::new();
    for (name, at) in checkpoints.snapshot() {
        let lag = committed.saturating_sub(at);
        consumers.insert(name.to_string(), lag.into());
        if lag > limit("HEALTH_MAX_CONSUMER_LAG", 20_000) {
            problems.push(format!("{name} {lag} records behind"));
        }
    }
    let body = serde_json::json!({
        "ok": problems.is_empty(), "problems": problems, "walSeq": committed,
        "persistLag": persist_lag, "consumerLag": consumers,
        "sockets": app.metrics.sockets.load(Ordering::Relaxed),
    });
    let code = if problems.is_empty() {
        axum::http::StatusCode::OK
    } else {
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    };
    (code, body.to_string())
}

fn render_metrics(app: &App, pstats: &persister::PersistStats, checkpoints: &Checkpoints) -> String {
    let Some(w) = app.wal.get() else { return String::new() };
    let committed = w.committed.get() as f64;
    let cp = pstats.checkpoint.load(Ordering::Relaxed) as f64;
    let disk: u64 = wal::segments(&app.cfg.wal_dir)
        .map(|v| {
            v.iter()
                .filter_map(|(_, p)| std::fs::metadata(p).ok())
                .map(|m| m.len())
                .sum()
        })
        .unwrap_or(0);
    let s = &w.stats;
    let batches = s.batches.load(Ordering::Relaxed).max(1) as f64;
    let mut extra: Vec<(String, f64)> = vec![
        ("cgg_wal_committed_seq".into(), committed),
        ("cgg_wal_records_total".into(), s.records.load(Ordering::Relaxed) as f64),
        ("cgg_wal_batches_total".into(), batches),
        ("cgg_wal_syncs_total".into(), s.syncs.load(Ordering::Relaxed) as f64),
        ("cgg_wal_max_batch".into(), s.max_batch.load(Ordering::Relaxed) as f64),
        (
            "cgg_wal_mean_write_seconds".into(),
            s.write_us.load(Ordering::Relaxed) as f64 / batches / 1e6,
        ),
        ("cgg_wal_disk_bytes".into(), disk as f64),
        ("cgg_persist_checkpoint_seq".into(), cp),
        ("cgg_persist_lag_records".into(), committed - cp),
        (
            "cgg_persist_rows_total".into(),
            pstats.rows.load(Ordering::Relaxed) as f64,
        ),
        (
            "cgg_persist_failures_total".into(),
            pstats.failures.load(Ordering::Relaxed) as f64,
        ),
        (
            "cgg_persist_last_commit_seconds".into(),
            pstats.last_commit_us.load(Ordering::Relaxed) as f64 / 1e6,
        ),
    ];
    for (name, at) in checkpoints.snapshot() {
        extra.push((
            format!("cgg_consumer_lag_records{{consumer=\"{name}\"}}"),
            committed - at as f64,
        ));
    }
    let refs: Vec<(&str, f64)> = extra.iter().map(|(k, v)| (k.as_str(), *v)).collect();
    let mut out = app.metrics.render(&refs);
    s.wait
        .write(&mut out, "cgg_wal_wait_seconds", "append to writer pickup");
    s.sync.write(&mut out, "cgg_wal_sync_seconds", "batch write + sync");
    s.handoff
        .write(&mut out, "cgg_wal_handoff_seconds", "durable to broadcast start");
    out
}

async fn join_all<F: Future + Unpin>(mut fs: Vec<F>) -> Vec<F::Output> {
    use std::task::Poll;
    let mut done: Vec<Option<F::Output>> = (0..fs.len()).map(|_| None).collect();
    std::future::poll_fn(|cx| {
        let mut pending = false;
        for (i, f) in fs.iter_mut().enumerate() {
            if done[i].is_none() {
                match std::pin::Pin::new(f).poll(cx) {
                    Poll::Ready(v) => done[i] = Some(v),
                    Poll::Pending => pending = true,
                }
            }
        }
        if pending { Poll::Pending } else { Poll::Ready(()) }
    })
    .await;
    done.into_iter().map(Option::unwrap).collect()
}

#[cfg(test)]
mod pm2_shutdown_tests {
    use super::contains_shutdown_payload_after_any_frame_header as finds;

    #[test]
    fn windows_frame_with_sixteen_byte_header() {
        let mut frame = vec![1u8, 0, 0, 0, 0, 0, 0, 0, 12, 0, 0, 0, 0, 0, 0, 0];
        frame.extend_from_slice(b"\"shutdown\"\n");
        assert!(finds(&frame));
    }

    #[test]
    fn unix_bare_json_line() {
        assert!(finds(b"\"shutdown\"\n"));
    }

    #[test]
    fn other_messages_are_ignored() {
        assert!(!finds(b"{\"type\":\"process:msg\",\"data\":\"ping\"}\n"));
    }
}
