pub mod api;
pub mod auth;
pub mod bingo;
pub mod counter;
pub mod emote;
pub mod features;
pub mod feedback;
pub mod metrics;
pub mod rank;
pub mod server;
pub mod sitegoals;
pub mod thread;
pub mod user;

use crate::error::ApiError;
use crate::state::AppState;
use crate::throttle::{Limit, Rule};
use crate::{cors, http, session, throttle};
use axum::Router;
use axum::extract::{Request, State};
use axum::middleware::{Next, from_fn, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use std::time::{Duration, Instant};
use tower_http::services::ServeDir;
use tower_http::set_header::SetResponseHeaderLayer;

pub fn throttle_rule(key: &str) -> Rule {
    const GENEROUS: Limit = Limit {
        limit: 150,
        ttl: Duration::from_secs(60),
    };
    match key.strip_prefix("GET ") {
        Some(p) if p.ends_with("/thread/{name}") || p.ends_with("/thread/recentOnLoadFull/{thread_name}") => {
            Rule::Custom(GENEROUS)
        }
        _ => Rule::Default,
    }
}

fn api() -> Router<AppState> {
    Router::new()
        .merge(features::routes())
        .merge(sitegoals::routes())
        .merge(server::routes())
        .merge(emote::routes())
        .merge(crate::macros::routes())
        .merge(user::routes())
        .merge(feedback::routes())
        .merge(api::routes())
        .merge(auth::routes())
        .merge(metrics::routes())
        .merge(bingo::routes())
        .merge(rank::routes())
        .merge(counter::routes())
        .merge(thread::routes())
}

async fn not_found(req: Request) -> Response {
    ApiError::route_not_found(req.method().as_str(), req.uri().path()).into_response()
}

async fn timeout(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let limit = state.cfg.request_timeout;
    let what = format!("{} {}", req.method(), req.uri());
    let start = Instant::now();
    match tokio::time::timeout(limit, next.run(req)).await {
        Ok(r) => r,
        Err(_) => {
            tracing::warn!("HTTP timeout {what} after {}ms", start.elapsed().as_millis());
            ApiError::timeout(limit.as_millis()).into_response()
        }
    }
}

pub fn build(state: AppState) -> Router {
    let uploads = Router::new()
        .nest_service("/uploads", ServeDir::new(&state.cfg.uploads_dir))
        .layer(SetResponseHeaderLayer::if_not_present(
            axum::http::header::CACHE_CONTROL,
            axum::http::HeaderValue::from_static("public, max-age=604800"),
        ));
    Router::new()
        .nest("/api", api())
        .route_layer(from_fn_with_state(state.clone(), throttle::layer))
        .route_layer(from_fn(http::post_created))
        .merge(uploads)
        .fallback(not_found)
        .method_not_allowed_fallback(not_found)
        .layer(from_fn_with_state(state.clone(), session::layer))
        .layer(from_fn_with_state(state.clone(), timeout))
        .layer(from_fn_with_state(state.clone(), cors::layer))
        .with_state(state)
}

pub fn start_background(state: &AppState) {
    crate::daily_report::start(state);
    let s = state.clone();
    tokio::spawn(async move {
        match auth::counter_role_total(&s).await {
            Ok(n) => s
                .caches
                .counter_role_total
                .store(n, std::sync::atomic::Ordering::Relaxed),
            Err(e) => tracing::warn!("counter role total failed: {e:?}"),
        }
        if let Err(e) = api::startup_tasks(&s).await {
            tracing::error!("startup tasks failed: {e:?}");
        }
        if let Err(e) = sitegoals::create_goals_in_code(&s).await {
            tracing::error!("site goals seed failed: {e:?}");
        }
        let mut tick = tokio::time::interval(Duration::from_secs(15 * 60));
        loop {
            tick.tick().await;
            match user::compute_pref_stats(&s).await {
                Ok(v) => *s.caches.pref_stats.lock().unwrap() = Some(v),
                Err(e) => tracing::warn!("Failed to refresh prefStats cache: {e:?}"),
            }
        }
    });
    let s = state.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(60));
        loop {
            tick.tick().await;
            s.throttle.sweep();
        }
    });
}
