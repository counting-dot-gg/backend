use crate::db;
use crate::entity;
use crate::error::{ApiError, ApiResult};
use crate::http::{JsonBody, Reply, reply};
use crate::session::Authed;
use crate::state::AppState;
use axum::Router;
use axum::extract::State;
use axum::routing::get;
use cgg_web::now_ms;
use serde_json::{Map, Value, json};
use std::time::{Duration, Instant};

const KNOWN_FLAGS: [(&str, &str); 2] = [
    ("bingo", "Bingo games, the /bingo page and the thread sidebar card."),
    ("run_records", "Thread records and split/speed PBs in milestones."),
];

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/features", get(list).put(set))
        .route("/features/admin", get(admin_list))
        .route("/features/bingo-window", get(bingo_window).put(set_bingo_window))
}

#[derive(Default)]
pub struct FlagCache {
    inner: tokio::sync::Mutex<Option<(Instant, Map<String, Value>)>>,
}

impl FlagCache {
    pub async fn all(&self, state: &AppState) -> ApiResult<Map<String, Value>> {
        let mut g = self.inner.lock().await;
        if let Some((at, m)) = g.as_ref()
            && at.elapsed() <= Duration::from_secs(5)
        {
            return Ok(m.clone());
        }
        let rows: Vec<(String, Option<i8>)> = sqlx::query_as("SELECT flagKey, enabled FROM feature_flag")
            .fetch_all(&state.pool)
            .await?;
        let mut m = Map::new();
        for (k, e) in rows {
            m.insert(k, Value::Bool(e.unwrap_or(0) != 0));
        }
        *g = Some((Instant::now(), m.clone()));
        Ok(m)
    }

    pub async fn set(&self, key: &str, enabled: bool) {
        if let Some((_, m)) = self.inner.lock().await.as_mut() {
            m.insert(key.to_owned(), Value::Bool(enabled));
        }
    }
}

async fn list(State(state): State<AppState>) -> ApiResult<Reply> {
    Ok(reply(Value::Object(state.flags.all(&state).await?)))
}

async fn flag_row(state: &AppState, key: &str) -> ApiResult<Option<Value>> {
    db::one(
        state,
        &entity::FEATURE_FLAG,
        "SELECT * FROM feature_flag WHERE flagKey = ? LIMIT 1",
        key,
    )
    .await
}

async fn admin_list(State(state): State<AppState>, Authed(user): Authed) -> ApiResult<Reply> {
    db::require_admin(&state, &user.uuid).await?;
    let rows = sqlx::query("SELECT * FROM feature_flag ORDER BY flagKey ASC")
        .fetch_all(&state.pool)
        .await?;
    let rows: Vec<Value> = rows.iter().map(|r| entity::FEATURE_FLAG.json(r)).collect();
    let out: Vec<Value> = KNOWN_FLAGS
        .iter()
        .map(|(key, description)| {
            let row = rows.iter().find(|r| r["flagKey"] == *key);
            let pick = |k: &str| row.map(|r| r[k].clone()).filter(|v| !v.is_null());
            json!({
                "flagKey": key,
                "description": pick("description").unwrap_or(json!(description)),
                "enabled": pick("enabled").unwrap_or(json!(false)),
                "updatedAt": pick("updatedAt").unwrap_or(Value::Null),
                "updatedBy": pick("updatedBy").unwrap_or(Value::Null),
            })
        })
        .collect();
    Ok(reply(out))
}

async fn upsert(
    state: &AppState,
    key: &str,
    description: &str,
    by: &str,
    enabled: Option<bool>,
    config: Option<String>,
) -> ApiResult<()> {
    let existing = flag_row(state, key).await?;
    let now = now_ms();
    match existing {
        Some(row) => {
            let id = row["id"].as_i64().unwrap_or_default();
            let keep_description = config.is_some() && enabled.is_none();
            let enabled = enabled.unwrap_or(row["enabled"].as_bool().unwrap_or(false));
            let description = if keep_description {
                row["description"].as_str().map(str::to_owned)
            } else {
                Some(description.to_owned())
            };
            let config = config.or_else(|| row["config"].as_str().map(str::to_owned));
            sqlx::query("UPDATE feature_flag SET enabled = ?, description = ?, config = ?, updatedAt = ?, updatedBy = ? WHERE id = ?")
                .bind(enabled)
                .bind(description)
                .bind(config)
                .bind(now)
                .bind(by)
                .bind(id)
                .execute(&state.pool)
                .await?;
        }
        None => {
            sqlx::query("INSERT INTO feature_flag (flagKey, enabled, description, config, updatedAt, updatedBy) VALUES (?, ?, ?, ?, ?, ?)")
                .bind(key)
                .bind(enabled.unwrap_or(false))
                .bind(description)
                .bind(config)
                .bind(now)
                .bind(by)
                .execute(&state.pool)
                .await?;
        }
    }
    Ok(())
}

async fn set(State(state): State<AppState>, Authed(user): Authed, body: JsonBody) -> ApiResult<Reply> {
    let admin = db::require_admin(&state, &user.uuid).await?;
    let Some((key, description)) = body
        .str("key")
        .and_then(|k| KNOWN_FLAGS.iter().find(|(f, _)| *f == k))
        .copied()
    else {
        return Err(ApiError::bad_request("Unknown feature flag."));
    };
    let Some(enabled) = body.get("enabled").as_bool() else {
        return Err(ApiError::bad_request("enabled must be a boolean."));
    };
    let by = admin["uuid"].as_str().unwrap_or_default();
    upsert(&state, key, description, by, Some(enabled), None).await?;
    state.flags.set(key, enabled).await;
    Ok(reply(flag_row(&state, key).await?.unwrap_or(Value::Null)))
}

async fn bingo_config(state: &AppState) -> ApiResult<Value> {
    let row = flag_row(state, "bingo").await?;
    Ok(row
        .and_then(|r| r["config"].as_str().map(str::to_owned))
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or(Value::Null))
}

async fn bingo_window(State(state): State<AppState>, Authed(user): Authed) -> ApiResult<Reply> {
    db::require_admin(&state, &user.uuid).await?;
    let c = bingo_config(&state).await?;
    let days = if c["days"].is_array() {
        c["days"].clone()
    } else {
        json!([5])
    };
    let over = if c["override"].is_null() {
        Value::Null
    } else {
        c["override"].clone()
    };
    Ok(reply(json!({ "days": days, "override": over })))
}

fn is_weekday(v: &Value) -> bool {
    v.as_f64().is_some_and(|d| d.fract() == 0.0 && (0.0..7.0).contains(&d))
}

async fn set_bingo_window(State(state): State<AppState>, Authed(user): Authed, body: JsonBody) -> ApiResult<Reply> {
    let admin = db::require_admin(&state, &user.uuid).await?;
    let days = match body.get("days").as_array() {
        Some(d) if d.iter().all(is_weekday) => d.iter().filter_map(Value::as_f64).map(|d| d as i64).collect::<Vec<_>>(),
        _ => return Err(ApiError::bad_request("days must be weekdays 0-6.")),
    };
    let over = body.get("override");
    let o = if over.is_null() {
        Value::Null
    } else {
        let Some(open) = over["open"].as_bool() else {
            return Err(ApiError::bad_request("override.open must be a boolean."));
        };
        let until = &over["until"];
        if !until.is_null() && !until.as_f64().is_some_and(|u| u > now_ms() as f64) {
            return Err(ApiError::bad_request("override.until must be a future time."));
        }
        json!({ "open": open, "until": until })
    };
    let mut unique: Vec<i64> = Vec::new();
    for d in days {
        if !unique.contains(&d) {
            unique.push(d);
        }
    }
    unique.sort_by_key(|d| d.to_string());
    let value = json!({ "days": unique, "override": o });
    let by = admin["uuid"].as_str().unwrap_or_default();
    upsert(&state, "bingo", KNOWN_FLAGS[0].1, by, None, Some(value.to_string())).await?;
    Ok(reply(value))
}
