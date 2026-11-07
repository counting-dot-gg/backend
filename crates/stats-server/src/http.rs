use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    middleware,
    response::IntoResponse,
    routing::{get, post},
};
use serde::Deserialize;
use std::sync::Arc;

use crate::{
    AppState,
    admin::{self, StartError},
    auth,
};
use stats::{
    dynamic_stats::run_dynamic_query,
    models::DynamicStatParams,
    stats_engine::{StatsOptions, run_stats},
};

pub fn router(state: Arc<AppState>) -> Router {
    let protected = Router::new()
        .route("/init", post(init_full))
        .route("/init/range", post(init_range))
        .route("/query", post(dynamic_query))
        .route("/admin/run", post(admin_run))
        .route("/admin/status", get(admin_status))
        .route_layer(middleware::from_fn(auth::require_api_key));

    Router::new()
        .route("/health", get(health))
        .merge(protected)
        .with_state(state)
}

async fn health(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let last_run = state.last_run_secs.load(std::sync::atomic::Ordering::Relaxed);
    Json(serde_json::json!({
        "status": "ok",
        "last_run_unix": last_run,
        "built_at": env!("BUILT_AT"),
    }))
}

#[derive(Deserialize, Default)]
pub struct InitParams {
    #[serde(default)]
    pub force: bool,
    pub thread: Option<String>,
}

async fn init_full(State(state): State<Arc<AppState>>, Json(params): Json<InitParams>) -> impl IntoResponse {
    let job_lock = state.job_lock.clone();
    let pool = state.pool.clone();
    let boundary_pool = state.boundary_pool.clone();
    let redis = state.redis.clone();

    let lock_result = job_lock.try_write_owned();
    let lock = match lock_result {
        Ok(g) => g,
        Err(_) => {
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({ "error": "job already running" })),
            )
                .into_response();
        }
    };

    let cancel = state.shutdown.clone();
    let achievements = state.achievements.clone();

    tokio::spawn(async move {
        let _guard = lock;
        let mut redis = redis.lock().await;

        let opts = StatsOptions {
            force_init: params.force,
            target_thread: params.thread.clone(),
            date_range: None,
        };

        if let Err(e) = run_stats(&pool, &boundary_pool, &mut redis, opts, cancel).await {
            tracing::error!("init run_stats error: {e}");
        }

        if params.thread.is_none() {
            let threads = match stats::db::load_all_threads(&pool).await {
                Ok(t) => t,
                Err(e) => {
                    tracing::error!("init achievement check load_threads error: {e}");
                    return;
                }
            };
            if let Err(e) = achievements.check(&pool, &mut redis, &threads).await {
                tracing::error!("init achievement check error: {e}");
            }
        }
    });

    (StatusCode::ACCEPTED, Json(serde_json::json!({ "status": "started" }))).into_response()
}

#[derive(Deserialize)]
pub struct RangeParams {
    pub start: String,
    pub end: String,
    pub thread: Option<String>,
}

async fn init_range(State(state): State<Arc<AppState>>, Json(params): Json<RangeParams>) -> impl IntoResponse {
    let (start_ms, end_ms) = match parse_date_range(&params.start, &params.end) {
        Some(r) => r,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "invalid date format, use YYYY-MM-DD" })),
            )
                .into_response();
        }
    };

    let job_lock = state.job_lock.clone();
    let pool = state.pool.clone();
    let boundary_pool = state.boundary_pool.clone();
    let redis = state.redis.clone();

    let lock = match job_lock.try_write_owned() {
        Ok(g) => g,
        Err(_) => {
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({ "error": "job already running" })),
            )
                .into_response();
        }
    };

    let cancel = state.shutdown.clone();

    tokio::spawn(async move {
        let _guard = lock;
        let mut redis = redis.lock().await;

        let mut current_ms = start_ms;
        let day_ms: i64 = 86_400_000;

        while current_ms < end_ms {
            if cancel.is_cancelled() {
                break;
            }
            let day_end_ms = (current_ms + day_ms).min(end_ms);
            let opts = StatsOptions {
                force_init: true,
                target_thread: params.thread.clone(),
                date_range: Some((current_ms, day_end_ms)),
            };
            if let Err(e) = run_stats(&pool, &boundary_pool, &mut redis, opts, cancel.clone()).await {
                tracing::error!("range run_stats error: {e}");
                break;
            }
            current_ms += day_ms;
        }
    });

    (StatusCode::ACCEPTED, Json(serde_json::json!({ "status": "started" }))).into_response()
}

async fn dynamic_query(State(state): State<Arc<AppState>>, Json(params): Json<DynamicStatParams>) -> impl IntoResponse {
    let job_lock = state.job_lock.clone();
    let _guard = job_lock.read_owned().await;
    let pool = state.pool.clone();

    match run_dynamic_query(&pool, &params).await {
        Ok(rows) => Json(serde_json::json!({ "results": rows })).into_response(),
        Err(e) => {
            tracing::error!("dynamic_query error: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response()
        }
    }
}

fn parse_date_range(start: &str, end: &str) -> Option<(i64, i64)> {
    let start_ms = parse_date_to_ms(start)?;
    let end_ms = parse_date_to_ms(end)? + 86_400_000;
    if end_ms <= start_ms {
        return None;
    }
    Some((start_ms, end_ms))
}

fn parse_date_to_ms(date: &str) -> Option<i64> {
    let parts: Vec<&str> = date.split('-').collect();
    if parts.len() != 3 {
        return None;
    }
    let y: i32 = parts[0].parse().ok()?;
    let m: u32 = parts[1].parse().ok()?;
    let d: u32 = parts[2].parse().ok()?;

    let naive = chrono::NaiveDate::from_ymd_opt(y, m, d)?.and_hms_opt(0, 0, 0)?;
    let offset_secs: i64 = 5 * 3600;
    let unix_secs = naive.and_utc().timestamp() + offset_secs;
    Some(unix_secs * 1000)
}

#[derive(Deserialize)]
pub struct AdminRunParams {
    pub kind: String,
    #[serde(default)]
    pub target: Option<String>,
}

async fn admin_run(State(state): State<Arc<AppState>>, Json(params): Json<AdminRunParams>) -> impl IntoResponse {
    match admin::start(state, &params.kind, params.target).await {
        Ok(job) => (
            StatusCode::ACCEPTED,
            Json(serde_json::json!({ "started": true, "job": job })),
        )
            .into_response(),
        Err(StartError::AlreadyRunning(job)) => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "started": false,
                "job": job,
                "reason": format!("A {} job is already running.", job.kind),
            })),
        )
            .into_response(),
        Err(StartError::BadRequest(reason)) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "started": false, "job": null, "reason": reason })),
        )
            .into_response(),
    }
}

async fn admin_status(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let job = state.job.lock().await.clone();
    Json(serde_json::json!({ "job": job }))
}
