mod admin;
mod auth;
mod http;

use stats::achievements::AchievementEngine;
use stats::{db, redis_client, stats_engine, tz};

use anyhow::{Context, Result};
use redis::aio::ConnectionManager;
use sqlx::MySqlPool;
use std::sync::{Arc, atomic::AtomicU64};
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

pub struct AppState {
    pub pool: MySqlPool,
    pub boundary_pool: MySqlPool,
    pub redis: Arc<Mutex<ConnectionManager>>,
    pub job_lock: Arc<RwLock<()>>,
    pub last_run_secs: Arc<AtomicU64>,
    pub shutdown: CancellationToken,
    pub job: Arc<Mutex<Option<admin::AdminJob>>>,
    pub achievements: Arc<dyn AchievementEngine>,
}

#[tokio::main]
async fn main() -> Result<()> {
    cgg_core::load_env();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_env("RUST_LOG").add_directive("stats=info".parse().unwrap()),
        )
        .init();

    let pool = db::create_pool().await.context("failed to connect to MySQL")?;
    let boundary_pool = db::create_boundary_pool()
        .await
        .context("failed to connect to MySQL (boundary pool)")?;
    let redis_mgr = redis_client::create_connection_manager()
        .await
        .context("failed to connect to Redis")?;
    let shutdown = CancellationToken::new();
    let achievement_engine = progression::engine();

    let argv: Vec<String> = std::env::args().skip(1).collect();
    let skip_achievements = argv.iter().any(|a| a == "--no-achievements");
    if let Some(opts) = parse_oneshot(argv.iter().cloned()) {
        install_shutdown_handler(shutdown.clone());
        let mut redis = redis_mgr;
        tracing::info!(
            force = opts.force_init,
            thread = ?opts.target_thread,
            range = ?opts.date_range,
            "Running one-shot stats job"
        );
        stats_engine::run_stats(&pool, &boundary_pool, &mut redis, opts.clone(), shutdown.clone())
            .await
            .context("one-shot stats run failed")?;

        if !skip_achievements && opts.target_thread.is_none() {
            let threads = db::load_all_threads(&pool).await?;
            achievement_engine
                .check(&pool, &mut redis, &threads)
                .await
                .context("one-shot achievement check failed")?;
        }
        tracing::info!("One-shot stats job complete");
        return Ok(());
    }

    let state = Arc::new(AppState {
        pool: pool.clone(),
        boundary_pool: boundary_pool.clone(),
        redis: Arc::new(Mutex::new(redis_mgr)),
        job_lock: Arc::new(RwLock::new(())),
        last_run_secs: Arc::new(AtomicU64::new(0)),
        shutdown: shutdown.clone(),
        job: Arc::new(Mutex::new(None)),
        achievements: achievement_engine,
    });

    let bg_state = state.clone();
    tokio::spawn(async move {
        let interval_secs = std::env::var("STATS_INTERVAL_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(60);
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(interval_secs));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = ticker.tick() => {}
                _ = bg_state.shutdown.cancelled() => {
                    tracing::info!("Background loop shutting down");
                    return;
                }
            }

            tracing::info!("Background incremental stats run starting");
            let _guard = bg_state.job_lock.clone().write_owned().await;
            let mut redis = bg_state.redis.lock().await;

            let threads = match db::load_all_threads(&bg_state.pool).await {
                Ok(t) => t,
                Err(e) => {
                    tracing::error!("background stats: load threads failed: {e}");
                    continue;
                }
            };

            let opts = stats_engine::StatsOptions::default();
            if let Err(e) = stats_engine::run_stats(
                &bg_state.pool,
                &bg_state.boundary_pool,
                &mut redis,
                opts,
                bg_state.shutdown.clone(),
            )
            .await
            {
                tracing::error!("background stats: run_stats failed: {e}");
            }

            if let Err(e) = bg_state.achievements.check(&bg_state.pool, &mut redis, &threads).await {
                tracing::error!("background stats: achievement check failed: {e}");
            }

            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            bg_state.last_run_secs.store(now, std::sync::atomic::Ordering::Relaxed);

            tracing::info!("Background incremental stats run complete");
        }
    });

    let sweep_state = state.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(120)).await;
        let mut last_swept: Option<String> = None;

        loop {
            if sweep_state.shutdown.is_cancelled() {
                return;
            }
            let now_ms = chrono::Utc::now().timestamp_millis();
            let today = tz::ms_to_date_key(now_ms);
            let past_midnight_min = tz::day_start_ms(&today)
                .map(|start| (now_ms - start) / 60_000)
                .unwrap_or(0);

            let due = last_swept.as_deref() != Some(today.as_str())
                && (last_swept.is_none() || past_midnight_min >= 4 * 60 + 20);

            if due {
                last_swept = Some(today.clone());
                let _guard = sweep_state.job_lock.clone().write_owned().await;
                match admin::backfill_missing_days(&sweep_state, 30).await {
                    Ok((found, repaired)) => {
                        if found > 0 {
                            tracing::info!(found, repaired, "[stats-gaps] sweep complete");
                        }
                    }
                    Err(e) => tracing::error!(error = %e, "[stats-gaps] sweep failed"),
                }
            }

            tokio::select! {
                _ = tokio::time::sleep(std::time::Duration::from_secs(600)) => {}
                _ = sweep_state.shutdown.cancelled() => return,
            }
        }
    });

    let port = std::env::var("STATS_PORT")
        .ok()
        .and_then(|v| v.parse::<u16>().ok())
        .unwrap_or(8806);

    let app = http::router(state.clone());
    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
    tracing::info!("Stats service listening on {addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;

    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            tracing::info!("Shutdown signal received — stopping");
            shutdown.cancel();
        })
        .await?;

    Ok(())
}

fn parse_oneshot<I: Iterator<Item = String>>(args: I) -> Option<stats_engine::StatsOptions> {
    let args: Vec<String> = args.collect();
    if !args.iter().any(|a| a == "--init") {
        return None;
    }

    let value_of = |flag: &str| -> Option<String> {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };

    let date_range = match (value_of("--from"), value_of("--to")) {
        (Some(from), Some(to)) => match (tz::day_start_ms(&from), tz::day_end_ms(&to)) {
            (Some(a), Some(b)) => Some((a, b)),
            _ => {
                tracing::error!("--from/--to must be YYYY-MM-DD");
                None
            }
        },
        _ => None,
    };

    Some(stats_engine::StatsOptions {
        force_init: true,
        target_thread: value_of("--thread"),
        date_range,
    })
}

fn install_shutdown_handler(token: CancellationToken) {
    tokio::spawn(async move {
        shutdown_signal().await;
        tracing::info!("Shutdown signal received — finishing current batch");
        token.cancel();
    });
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c().await.expect("failed to install Ctrl-C handler");
    };

    #[cfg(unix)]
    {
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler");
        tokio::select! {
            _ = ctrl_c => {}
            _ = sigterm.recv() => {}
        }
    }

    #[cfg(not(unix))]
    ctrl_c.await;
}
