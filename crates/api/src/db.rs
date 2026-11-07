use crate::entity::{self, Entity};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use serde_json::Value;
use sqlx::mysql::MySqlRow;

pub fn has_role(counter: &Value, role: &str) -> bool {
    counter["roles"]
        .as_array()
        .is_some_and(|r| r.iter().any(|x| x.as_str() == Some(role)))
}

pub async fn one(state: &AppState, e: &Entity, sql: &str, arg: &str) -> ApiResult<Option<Value>> {
    let row: Option<MySqlRow> = sqlx::query(sql).bind(arg).fetch_optional(&state.pool).await?;
    Ok(row.map(|r| e.json(&r)))
}

pub async fn find_counter(state: &AppState, uuid: &str) -> ApiResult<Option<Value>> {
    one(
        state,
        &entity::COUNTER,
        "SELECT * FROM counters WHERE uuid = ? LIMIT 1",
        uuid,
    )
    .await
}

pub async fn find_counter_by_username(state: &AppState, username: &str) -> ApiResult<Option<Value>> {
    one(
        state,
        &entity::COUNTER,
        "SELECT * FROM counters WHERE username = ? LIMIT 1",
        username,
    )
    .await
}

pub async fn find_user(state: &AppState, uuid: &str) -> ApiResult<Option<Value>> {
    one(state, &entity::USER, "SELECT * FROM users WHERE uuid = ? LIMIT 1", uuid).await
}

pub async fn find_thread_by_name(state: &AppState, name: &str) -> ApiResult<Option<Value>> {
    one(
        state,
        &entity::THREAD,
        "SELECT * FROM thread WHERE name = ? LIMIT 1",
        name,
    )
    .await
}

pub async fn find_thread_by_uuid(state: &AppState, uuid: &str) -> ApiResult<Option<Value>> {
    one(
        state,
        &entity::THREAD,
        "SELECT * FROM thread WHERE uuid = ? LIMIT 1",
        uuid,
    )
    .await
}

pub async fn require_admin(state: &AppState, uuid: &str) -> ApiResult<Value> {
    match find_counter(state, uuid).await? {
        Some(c) if has_role(&c, "admin") => Ok(c),
        _ => Err(ApiError::bare(axum::http::StatusCode::NOT_FOUND)),
    }
}

pub fn placeholders(n: usize) -> String {
    vec!["?"; n.max(1)].join(", ")
}

pub async fn find_counters(state: &AppState, uuids: &[String]) -> ApiResult<Vec<Value>> {
    if uuids.is_empty() {
        return Ok(Vec::new());
    }
    let sql = format!("SELECT * FROM counters WHERE uuid IN ({})", placeholders(uuids.len()));
    let mut q = sqlx::query(&sql);
    for u in uuids {
        q = q.bind(u);
    }
    let rows = q.fetch_all(&state.pool).await?;
    Ok(rows.iter().map(|r| entity::COUNTER.json(r)).collect())
}

fn bind_all<'q>(
    mut q: sqlx::query::Query<'q, sqlx::MySql, sqlx::mysql::MySqlArguments>,
    args: &'q [Value],
) -> sqlx::query::Query<'q, sqlx::MySql, sqlx::mysql::MySqlArguments> {
    for a in args {
        q = match a {
            Value::String(s) => q.bind(s.as_str()),
            Value::Number(n) if n.is_i64() => q.bind(n.as_i64()),
            Value::Number(n) if n.is_u64() => q.bind(n.as_u64()),
            Value::Number(n) => q.bind(n.as_f64()),
            Value::Bool(b) => q.bind(*b),
            Value::Null => q.bind(Option::<String>::None),
            other => q.bind(other.to_string()),
        };
    }
    q
}

pub async fn rows(state: &AppState, sql: &str, args: &[Value]) -> ApiResult<Vec<MySqlRow>> {
    Ok(bind_all(sqlx::query(sql), args).fetch_all(&state.pool).await?)
}

pub async fn exec(state: &AppState, sql: &str, args: &[Value]) -> ApiResult<sqlx::mysql::MySqlQueryResult> {
    Ok(bind_all(sqlx::query(sql), args).execute(&state.pool).await?)
}

pub async fn find_thread_by_uuid_value(state: &AppState, uuid: &Value) -> ApiResult<Option<Value>> {
    match uuid.as_str() {
        Some(u) => find_thread_by_uuid(state, u).await,
        None => Ok(None),
    }
}
