use crate::db;
use crate::entity;
use crate::error::ApiResult;
use crate::http::{Reply, empty, reply};
use crate::state::AppState;
use axum::Router;
use axum::extract::{Path, State};
use axum::routing::get;
use serde_json::json;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/server/count", get(count))
        .route("/server/all", get(all))
        .route("/server/server/{server_id}", get(one))
}

async fn count(State(state): State<AppState>) -> ApiResult<Reply> {
    let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM servers WHERE isActive = 1")
        .fetch_one(&state.pool)
        .await?;
    Ok(reply(json!({ "count": n })))
}

async fn all(State(state): State<AppState>) -> ApiResult<Reply> {
    let rows = sqlx::query("SELECT * FROM servers WHERE isActive = 1")
        .fetch_all(&state.pool)
        .await?;
    Ok(reply(entity::SERVER.rows(&rows)))
}

async fn one(State(state): State<AppState>, Path(server_id): Path<String>) -> ApiResult<Reply> {
    let server = db::one(
        &state,
        &entity::SERVER,
        "SELECT * FROM servers WHERE isActive = 1 AND discordID = ? LIMIT 1",
        &server_id,
    )
    .await?;
    Ok(server.map_or_else(empty, reply))
}
