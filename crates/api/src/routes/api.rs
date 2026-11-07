use crate::db::{self, has_role};
use crate::entity;
use crate::error::{ApiError, ApiResult};
use crate::http::{JsonBody, Reply, WithStatus, empty, reply};
use crate::jobs;
use crate::session::{Authed, MaybeUser};
use crate::state::AppState;
use crate::throttle::ClientIp;
use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use cgg_web::now_ms;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/reportClientError", post(report_client_error))
        .route("/api/purchaseItem", post(purchase_item))
        .route("/api/admin/stats/run", post(admin_stats_run))
        .route("/api/admin/stats/status", get(admin_stats_status))
        .route("/api/defaultPage", get(default_page))
        .route("/api/createBlogPost", post(create_blog_post))
        .route("/api/updateBlogPost", post(update_blog_post))
        .route("/api/hideBlogPost", post(hide_blog_post))
        .route("/api/unlockReward", post(unlock_reward))
}

#[derive(Default)]
pub struct ClientErrors {
    seen: Mutex<(HashMap<String, i64>, i64, u32)>,
}

fn cut(v: &Value, n: usize) -> Value {
    v.as_str().map_or(Value::Null, |s| {
        let units: Vec<u16> = s.encode_utf16().take(n).collect();
        json!(String::from_utf16_lossy(&units))
    })
}

async fn report_client_error(
    State(state): State<AppState>,
    MaybeUser(user): MaybeUser,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    body: JsonBody,
) -> ApiResult<Reply> {
    let key = user.as_ref().map_or_else(|| format!("ip:{ip}"), |u| u.uuid.clone());
    let now = now_ms();
    let sent = {
        let mut g = state.client_errors.seen.lock().unwrap();
        let (seen, window_at, count) = &mut *g;
        let last = seen.get(&key).copied().unwrap_or(0);
        if now - last < 60_000 {
            false
        } else {
            if now - *window_at > 60_000 {
                *window_at = now;
                *count = 0;
            }
            if *count >= 20 {
                false
            } else {
                seen.insert(key, now);
                *count += 1;
                if seen.len() > 500 {
                    seen.retain(|_, t| now - *t <= 60_000);
                }
                true
            }
        }
    };
    if sent {
        let ua = headers
            .get("user-agent")
            .and_then(|v| v.to_str().ok())
            .map(|s| json!(s));
        let mut payload = json!({
            "scope": cut(body.get("scope"), 80),
            "url": cut(body.get("url"), 400),
            "message": cut(body.get("message"), 1000),
            "digest": cut(body.get("digest"), 100),
            "stack": cut(body.get("stack"), 1500),
            "userAgent": ua.map_or(Value::Null, |u| cut(&u, 400)),
            "username": user.as_ref().and_then(|u| u.str("username").map(str::to_owned)).unwrap_or_else(|| "anonymous".into()),
        });
        if let Some(o) = payload.as_object_mut() {
            o.retain(|_, v| !v.is_null());
        }
        state.bot.emit("logClientError", payload);
    }
    Ok(reply(json!({ "sent": sent })))
}

async fn purchase_item(State(state): State<AppState>, Authed(user): Authed, body: JsonBody) -> ApiResult<Reply> {
    let id = body.get("item_id");
    if !crate::routes::user::truthy(id) {
        return Err(ApiError::bare(StatusCode::BAD_REQUEST));
    }
    let item = match id.as_i64().or_else(|| id.as_str().and_then(|s| s.parse().ok())) {
        Some(id) => sqlx::query("SELECT * FROM item WHERE id = ? LIMIT 1")
            .bind(id)
            .fetch_optional(&state.pool)
            .await?
            .map(|r| entity::ITEM.json(&r)),
        None => None,
    };
    let Some(item) = item else {
        return Err(ApiError::bare(StatusCode::BAD_REQUEST));
    };
    let s = state.clone();
    tokio::spawn(async move {
        if let Err(e) = jobs::shop_purchase(&s, &user.json, &item).await {
            tracing::error!("shopPurchase failed: {e:?}");
        }
    });
    Ok(empty())
}

async fn assert_admin(state: &AppState, uuid: &str) -> ApiResult<()> {
    match db::find_counter(state, uuid).await? {
        Some(c) if has_role(&c, "admin") => Ok(()),
        _ => Err(ApiError::bare(StatusCode::FORBIDDEN)),
    }
}

fn stats_url(state: &AppState, path: &str) -> String {
    format!(
        "{}{path}",
        state.cfg.stats_http_url.as_deref().unwrap_or("http://127.0.0.1:8806")
    )
}

fn stats_request(state: &AppState, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
    let mut r = state
        .http
        .request(method, stats_url(state, path))
        .timeout(Duration::from_secs(10))
        .header("content-type", "application/json");
    if let Some(k) = state.cfg.stats_api_key.as_deref().filter(|k| !k.is_empty()) {
        r = r.header("x-api-key", k);
    }
    r
}

fn fetch_error(e: &reqwest::Error) -> &'static str {
    if e.is_timeout() {
        "This operation was aborted"
    } else {
        "fetch failed"
    }
}

async fn admin_stats_run(State(state): State<AppState>, Authed(user): Authed, body: JsonBody) -> ApiResult<Reply> {
    assert_admin(&state, &user.uuid).await?;
    let allowed = ["all", "day", "counter", "achievements", "gaps"];
    let Some(kind) = body.str("kind").filter(|k| allowed.contains(k)) else {
        return Err(ApiError::bad_request(format!(
            "kind must be one of {}",
            allowed.join(", ")
        )));
    };
    let target = body.get("target");
    let res = stats_request(&state, reqwest::Method::POST, "/admin/run")
        .body(
            json!({ "kind": kind, "target": if target.is_null() { Value::Null } else { target.clone() } }).to_string(),
        )
        .send()
        .await;
    let out = match res {
        Ok(r) => {
            let status = r.status().as_u16();
            match r.json::<Value>().await {
                Ok(v) if v.is_object() => v,
                _ => json!({ "started": false, "job": null, "reason": format!("Stats service returned {status}.") }),
            }
        }
        Err(e) => {
            json!({ "started": false, "job": null, "reason": format!("Could not reach the stats service: {}", fetch_error(&e)) })
        }
    };
    Ok(reply(out))
}

async fn admin_stats_status(State(state): State<AppState>, Authed(user): Authed) -> ApiResult<Reply> {
    assert_admin(&state, &user.uuid).await?;
    let job = match stats_request(&state, reqwest::Method::GET, "/admin/status")
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => r.json::<Value>().await.ok().map_or(Value::Null, |b| b["job"].clone()),
        _ => Value::Null,
    };
    Ok(reply(json!({ "job": job })))
}

async fn default_page(State(state): State<AppState>) -> ApiResult<Reply> {
    let page = state
        .socket_calls
        .call("defaultPage", json!({}), Duration::from_secs(10))
        .await
        .ok_or_else(|| ApiError::Internal(anyhow::anyhow!("socket did not answer defaultPage")))?;
    Ok(reply(page))
}

fn created(v: Value) -> WithStatus {
    WithStatus(StatusCode::CREATED, reply(v))
}

fn trimmed_len(v: &Value) -> Option<(String, usize)> {
    let s = counting::js::trim(v.as_str()?).to_owned();
    let n = s.encode_utf16().count();
    Some((s, n))
}

async fn blog_author(state: &AppState, uuid: &str, block_muted: bool) -> ApiResult<Value> {
    let Some(c) = db::find_counter(state, uuid).await? else {
        return Err(ApiError::bare(StatusCode::BAD_REQUEST));
    };
    if block_muted && (has_role(&c, "muted") || has_role(&c, "banned")) {
        return Err(ApiError::bare(StatusCode::FORBIDDEN));
    }
    Ok(c)
}

async fn create_blog_post(
    State(state): State<AppState>,
    Authed(user): Authed,
    body: JsonBody,
) -> ApiResult<WithStatus> {
    let counter = blog_author(&state, &user.uuid, true).await?;
    let (Some((title, tl)), Some((text, bl))) = (trimmed_len(body.get("title")), trimmed_len(body.get("body"))) else {
        return Err(ApiError::bare(StatusCode::BAD_REQUEST));
    };
    if !(1..=200).contains(&tl) || !(1..=20000).contains(&bl) {
        return Err(ApiError::bare(StatusCode::BAD_REQUEST));
    }
    let blog = jobs::create_blog(&state, &counter, &title, &text, body.get("tags")).await?;
    Ok(created(blog))
}

async fn update_blog_post(
    State(state): State<AppState>,
    Authed(user): Authed,
    body: JsonBody,
) -> ApiResult<WithStatus> {
    let counter = blog_author(&state, &user.uuid, true).await?;
    let Some(uuid) = body.str("uuid").filter(|u| !u.is_empty()) else {
        return Err(ApiError::bare(StatusCode::BAD_REQUEST));
    };
    let as_text = |v: &Value| {
        if v.is_null() {
            Some((String::new(), 0))
        } else {
            trimmed_len(v)
        }
    };
    let (Some((_, tl)), Some((_, bl))) = (as_text(body.get("title")), as_text(body.get("body"))) else {
        return Err(ApiError::Internal(anyhow::anyhow!("title/body not strings")));
    };
    if !(1..=200).contains(&tl) || !(1..=20000).contains(&bl) {
        return Err(ApiError::bare(StatusCode::BAD_REQUEST));
    }
    let blog = jobs::update_blog(
        &state,
        &counter,
        uuid,
        body.get("title"),
        body.get("body"),
        body.get("tags"),
    )
    .await?;
    Ok(created(blog))
}

async fn hide_blog_post(State(state): State<AppState>, Authed(user): Authed, body: JsonBody) -> ApiResult<WithStatus> {
    let counter = blog_author(&state, &user.uuid, false).await?;
    let Some(uuid) = body.str("uuid").filter(|u| !u.is_empty()) else {
        return Err(ApiError::bare(StatusCode::BAD_REQUEST));
    };
    let hidden = crate::routes::user::truthy(body.get("hidden"));
    Ok(created(jobs::hide_blog(&state, &counter, uuid, hidden).await?))
}

async fn unlock_reward(State(state): State<AppState>, Authed(user): Authed, body: JsonBody) -> ApiResult<WithStatus> {
    let level = body.get("level");
    if !crate::routes::user::truthy(level) {
        return Err(ApiError::bare(StatusCode::BAD_REQUEST));
    }
    let level = crate::http::js_number(level);
    Ok(created(jobs::unlock_xp_reward(&state, &user.uuid, level).await?))
}

const ITEMS_IN_CODE: [(&str, &str, i64); 2] = [
    ("abc", "db19c6cc-42e0-4981-8edf-b670aeb30728", 1),
    ("def", "9822a4b3-7aee-422f-a9a6-3900b22bb784", 2),
];

pub async fn startup_tasks(state: &AppState) -> ApiResult<()> {
    for (name, uuid, level) in ITEMS_IN_CODE {
        let existing: Option<(i32,)> = sqlx::query_as("SELECT id FROM item WHERE uuid = ? LIMIT 1")
            .bind(uuid)
            .fetch_optional(&state.pool)
            .await?;
        let args = [
            json!(name),
            json!(name),
            json!("ok"),
            json!("xp"),
            json!("ok"),
            json!("ok"),
            json!(0),
            json!(0),
            json!(level),
            json!(2),
        ];
        match existing {
            Some((id,)) => {
                let mut a = args.to_vec();
                a.push(json!(id));
                db::exec(
                    state,
                    "UPDATE item SET name = ?, internal_name = ?, unlockDescription = ?, unlockMethod = ?, description = ?, category = ?, price = ?, quantity = ?, levelToUnlock = ?, season = ? WHERE id = ?",
                    &a,
                )
                .await?;
            }
            None => {
                let mut a = vec![json!(uuid)];
                a.extend(args);
                db::exec(
                    state,
                    "INSERT INTO item (uuid, name, internal_name, unlockDescription, unlockMethod, description, category, price, quantity, levelToUnlock, season) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    &a,
                )
                .await?;
            }
        }
    }
    for d in progression::definitions() {
        let (name, description, public, max) = (d.name, d.description, d.public, d.max_progress);
        let existing: Option<(i32,)> = sqlx::query_as("SELECT id FROM achievements WHERE name = ? LIMIT 1")
            .bind(name)
            .fetch_optional(&state.pool)
            .await?;
        match existing {
            Some((id,)) => {
                sqlx::query(
                    "UPDATE achievements SET name = ?, description = ?, isPublic = ?, maxProgress = ? WHERE id = ?",
                )
                .bind(name)
                .bind(description)
                .bind(public)
                .bind(max)
                .bind(id)
                .execute(&state.pool)
                .await?;
            }
            None => {
                sqlx::query("INSERT INTO achievements (name, description, isPublic, maxProgress) VALUES (?, ?, ?, ?)")
                    .bind(name)
                    .bind(description)
                    .bind(public)
                    .bind(max)
                    .execute(&state.pool)
                    .await?;
            }
        }
    }
    Ok(())
}
