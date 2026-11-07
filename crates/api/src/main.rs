mod achievements;
mod cache;
mod config;
mod consts;
mod cors;
mod daily_report;
mod db;
mod entity;
mod error;
mod http;
mod jobs;
mod macros;
mod rank;
mod redis_conn;
mod routes;
mod session;
mod state;
mod statsread;
mod throttle;
mod unified_diff;
mod users;

use anyhow::Result;
use cgg_web::nest;
use sqlx::Executor;
use sqlx::mysql::MySqlPoolOptions;
use state::{AppState, Inner};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<()> {
    cgg_core::load_env();
    cgg_core::init_logging("info");
    let cfg = config::Config::from_env()?;
    let select_timeout = cfg.select_timeout_ms;
    let pool = MySqlPoolOptions::new()
        .max_connections(cgg_core::var("MYSQL_POOL").and_then(|v| v.parse().ok()).unwrap_or(20))
        .after_connect(move |conn, _| {
            Box::pin(async move {
                conn.execute(format!("SET SESSION max_execution_time = {select_timeout}").as_str())
                    .await?;
                Ok(())
            })
        })
        .connect_with(cfg.mysql.connect_options().timezone(None).pipes_as_concat(false))
        .await?;
    let redis = redis_conn::Redis::new(cfg.redis_url.as_str())?;
    redis.warm_up().await;
    let state = AppState(Arc::new(Inner {
        socket: nest::Client::connect("socket", cfg.socket_addr.clone()),
        socket_calls: nest::Caller::new("socket", cfg.socket_addr.clone()),
        bot: nest::Client::connect("bot", cfg.bot_addr.clone()),
        throttle: Default::default(),
        http: reqwest::Client::builder().timeout(Duration::from_secs(20)).build()?,
        caches: Default::default(),
        flags: Default::default(),
        goals: Default::default(),
        jobs: Default::default(),
        client_errors: Default::default(),
        clock: Default::default(),
        stats: Default::default(),
        coverage: Default::default(),
        owners: Default::default(),
        pool,
        redis,
        cfg,
    }));
    tracing::info!("trust proxy hops: {}", state.cfg.trust_proxy_hops);
    tracing::info!(
        "session cookie domain: {}",
        state.cfg.cookie_domain.as_deref().unwrap_or("(host-only)")
    );
    routes::start_background(&state);
    let app = tower::Layer::layer(
        &tower_http::normalize_path::NormalizePathLayer::trim_trailing_slash(),
        routes::build(state.clone()),
    );
    let listener = listen(state.cfg.port)?;
    tracing::info!("Running on PORT {}", state.cfg.port);
    axum::serve(
        listener,
        axum::ServiceExt::<axum::extract::Request>::into_make_service_with_connect_info::<SocketAddr>(app),
    )
    .with_graceful_shutdown(shutdown())
    .await?;
    Ok(())
}

fn listen(port: u16) -> Result<tokio::net::TcpListener> {
    use socket2::{Domain, Protocol, Socket, Type};
    let dual = || -> std::io::Result<Socket> {
        let s = Socket::new(Domain::IPV6, Type::STREAM, Some(Protocol::TCP))?;
        s.set_only_v6(false)?;
        s.bind(&SocketAddr::from((std::net::Ipv6Addr::UNSPECIFIED, port)).into())?;
        Ok(s)
    };
    let sock = match dual() {
        Ok(s) => s,
        Err(_) => {
            let s = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;
            s.bind(&SocketAddr::from((std::net::Ipv4Addr::UNSPECIFIED, port)).into())?;
            s
        }
    };
    #[cfg(unix)]
    sock.set_reuse_address(true)?;
    sock.listen(1024)?;
    sock.set_nonblocking(true)?;
    Ok(tokio::net::TcpListener::from_std(sock.into())?)
}

async fn shutdown() {
    let ctrl_c = tokio::signal::ctrl_c();
    #[cfg(unix)]
    {
        let mut term =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("SIGTERM handler");
        tokio::select! {
            _ = ctrl_c => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = ctrl_c.await;
    }
}
