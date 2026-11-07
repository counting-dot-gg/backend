use crate::db::{self, has_role};
use crate::entity;
use crate::error::{ApiError, ApiResult};
use crate::http::{JsonBody, Reply, js_number, reply};
use crate::jobs;
use crate::session::Authed;
use crate::state::AppState;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use cgg_web::now_ms;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/site-goals", get(list))
        .route("/site-goals/{goal_key}/contributors", get(contributors))
        .route("/site-goals/{goal_key}/contribute", post(contribute))
}

#[derive(Default)]
pub struct GoalCache {
    inner: tokio::sync::Mutex<Option<(Instant, Value)>>,
}

impl GoalCache {
    pub async fn clear(&self) {
        *self.inner.lock().await = None;
    }
}

fn num(v: &Value) -> f64 {
    js_number(v)
}

fn view(g: &Value) -> Value {
    let unlocked_at = &g["unlockedAt"];
    json!({
        "goalKey": g["goalKey"],
        "name": g["name"],
        "description": g["description"],
        "funding": g["funding"],
        "targetCost": entity::js_number(num(&g["targetCost"])),
        "raised": entity::js_number(num(&g["raised"])),
        "unlocked": !unlocked_at.is_null(),
        "unlockedAt": if unlocked_at.is_null() { Value::Null } else { entity::js_number(num(unlocked_at)) },
        "unlockedBy": g["unlockedBy"],
    })
}

pub async fn list_goals(state: &AppState) -> ApiResult<Value> {
    let mut g = state.goals.inner.lock().await;
    if let Some((at, v)) = g.as_ref()
        && at.elapsed() <= Duration::from_secs(10)
    {
        return Ok(v.clone());
    }
    let rows = sqlx::query("SELECT * FROM site_goal WHERE active = 1 ORDER BY sortOrder ASC, id ASC")
        .fetch_all(&state.pool)
        .await?;
    let v = Value::Array(rows.iter().map(|r| view(&entity::SITE_GOAL.json(r))).collect());
    *g = Some((Instant::now(), v.clone()));
    Ok(v)
}

async fn list(State(state): State<AppState>) -> ApiResult<Reply> {
    Ok(reply(list_goals(&state).await?))
}

pub async fn find_by_key(state: &AppState, key: &str) -> ApiResult<Option<Value>> {
    db::one(
        state,
        &entity::SITE_GOAL,
        "SELECT * FROM site_goal WHERE goalKey = ? LIMIT 1",
        key,
    )
    .await
}

async fn contributors(State(state): State<AppState>, Path(goal_key): Path<String>) -> ApiResult<Reply> {
    if find_by_key(&state, &goal_key).await?.is_none() {
        return Err(ApiError::bare(StatusCode::NOT_FOUND));
    }
    let rows: Vec<(String, rust_decimal::Decimal)> = sqlx::query_as(
        "SELECT c.counterUuid AS counterUuid, SUM(c.amount) AS amount FROM site_goal_contribution c \
         WHERE c.goalKey = ? GROUP BY c.counterUuid ORDER BY amount DESC LIMIT 25",
    )
    .bind(&goal_key)
    .fetch_all(&state.pool)
    .await?;
    if rows.is_empty() {
        return Ok(reply(json!([])));
    }
    let uuids: Vec<String> = rows.iter().map(|r| r.0.clone()).collect();
    let counters = db::find_counters(&state, &uuids).await?;
    let out: Vec<Value> = rows
        .iter()
        .map(|(uuid, amount)| {
            let c = counters.iter().find(|c| c["uuid"] == *uuid);
            json!({
                "uuid": uuid,
                "username": c.map_or(json!("unknown"), |c| c["username"].clone()),
                "color": c.map_or(Value::Null, |c| c["color"].clone()),
                "amount": entity::js_number(amount.to_string().parse::<f64>().unwrap_or(0.0)),
            })
        })
        .collect();
    Ok(reply(out))
}

async fn contribute(
    State(state): State<AppState>,
    Authed(user): Authed,
    Path(goal_key): Path<String>,
    body: JsonBody,
) -> ApiResult<Reply> {
    let Some(counter) = db::find_counter(&state, &user.uuid).await? else {
        return Err(ApiError::bare(StatusCode::NOT_FOUND));
    };
    if !has_role(&counter, "counter") || has_role(&counter, "banned") || has_role(&counter, "muted") {
        return Err(ApiError::bad_request("Not eligible to contribute."));
    }
    let goal = match find_by_key(&state, &goal_key).await? {
        Some(g) if g["active"].as_bool() == Some(true) => g,
        _ => return Err(ApiError::bare(StatusCode::NOT_FOUND)),
    };
    if !goal["unlockedAt"].is_null() {
        return Err(ApiError::bad_request("That goal is already funded."));
    }
    let value = if goal["funding"] == "solo" {
        num(&goal["targetCost"])
    } else {
        num(body.get("amount")).floor()
    };
    if !value.is_finite() || value <= 0.0 {
        return Err(ApiError::bad_request("Amount must be a positive whole number."));
    }
    let remaining = num(&goal["targetCost"]) - num(&goal["raised"]);
    if value > remaining {
        return Err(ApiError::bad_request(format!(
            "Only {} left to raise on that goal.",
            counting::js::num_to_string(remaining)
        )));
    }
    if num(&user.json["money"]) < value {
        return Err(ApiError::bad_request("Not enough money."));
    }
    let amount = value as i64;
    let s = state.clone();
    let uuid = user.uuid.clone();
    tokio::spawn(async move {
        if let Err(e) = jobs::site_goal_contribute(&s, &uuid, &goal_key, amount).await {
            tracing::error!("siteGoalContribute failed: {e:?}");
        }
    });
    Ok(reply(json!({ "queued": true, "amount": entity::js_number(value) })))
}

pub async fn apply_contribution(
    state: &AppState,
    goal_key: &str,
    counter_uuid: &str,
    amount: i64,
) -> ApiResult<Result<bool, &'static str>> {
    let goal = match find_by_key(state, goal_key).await? {
        Some(g) if g["active"].as_bool() == Some(true) => g,
        _ => return Ok(Err("no_such_goal")),
    };
    if !goal["unlockedAt"].is_null() {
        return Ok(Err("already_unlocked"));
    }
    let target = num(&goal["targetCost"]) as i64;
    let raised = num(&goal["raised"]) as i64;
    let remaining = target - raised;
    if remaining <= 0 {
        return Ok(Err("already_funded"));
    }
    if goal["funding"] == "solo" && amount != target {
        return Ok(Err("solo_requires_full_price"));
    }
    if amount > remaining {
        return Ok(Err("exceeds_remaining"));
    }
    let now = now_ms();
    sqlx::query("INSERT INTO site_goal_contribution (goalKey, counterUuid, amount, createdAt) VALUES (?, ?, ?, ?)")
        .bind(goal_key)
        .bind(counter_uuid)
        .bind(amount)
        .bind(now)
        .execute(&state.pool)
        .await?;
    let completed = raised + amount >= target;
    if completed {
        sqlx::query("UPDATE site_goal SET raised = ?, unlockedAt = ?, unlockedBy = ? WHERE goalKey = ?")
            .bind(raised + amount)
            .bind(now)
            .bind(counter_uuid)
            .bind(goal_key)
            .execute(&state.pool)
            .await?;
    } else {
        sqlx::query("UPDATE site_goal SET raised = ? WHERE goalKey = ?")
            .bind(raised + amount)
            .bind(goal_key)
            .execute(&state.pool)
            .await?;
    }
    state.goals.clear().await;
    Ok(Ok(completed))
}

const GOALS_IN_CODE: [(&str, &str, Option<&str>, &str, i64, i64, bool); 1] = [(
    "crystal_ball",
    "Crystal Ball",
    Some("A crystal ball with daily wisdom appears on the front page."),
    "community",
    2_000_000,
    10,
    true,
)];

pub async fn create_goals_in_code(state: &AppState) -> ApiResult<()> {
    for (key, name, description, funding, target, sort, active) in GOALS_IN_CODE {
        if find_by_key(state, key).await?.is_none() {
            sqlx::query(
                "INSERT INTO site_goal (goalKey, name, description, funding, targetCost, sortOrder, active, createdAt) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(key)
            .bind(name)
            .bind(description)
            .bind(funding)
            .bind(target)
            .bind(sort)
            .bind(active)
            .bind(now_ms())
            .execute(&state.pool)
            .await?;
        } else {
            sqlx::query(
                "UPDATE site_goal SET name = ?, description = ?, funding = ?, targetCost = ?, sortOrder = ?, active = ? WHERE goalKey = ?",
            )
            .bind(name)
            .bind(description)
            .bind(funding)
            .bind(target)
            .bind(sort)
            .bind(active)
            .bind(key)
            .execute(&state.pool)
            .await?;
        }
    }
    state.goals.clear().await;
    Ok(())
}
