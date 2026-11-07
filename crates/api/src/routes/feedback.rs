use crate::db::{self, has_role};
use crate::entity;
use crate::error::{ApiError, ApiResult};
use crate::http::{JsonBody, Query, Reply, reply};
use crate::routes::user::truthy;
use crate::session::{Authed, MaybeUser, SessionUser};
use crate::state::AppState;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use cgg_web::now_ms;
use serde_json::{Value, json};
use sqlx::Row;

const KINDS: [&str; 2] = ["bug", "feature"];
const STATUSES: [&str; 4] = ["open", "planned", "done", "declined"];
const MAX_TITLE: usize = 200;
const MAX_BODY: usize = 8000;
const MAX_COMMENT: usize = 2000;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/feedback/list", get(list))
        .route("/feedback/item/{id}", get(get_one))
        .route("/feedback/create", post(create))
        .route("/feedback/vote/{id}", post(vote))
        .route("/feedback/comment/{id}", post(comment))
        .route("/feedback/comment/{id}/delete", post(delete_comment))
        .route("/feedback/moderate/{id}", post(moderate))
}

fn int(s: &str) -> Option<i64> {
    let n = counting::js::parse_int(s, Some(10));
    n.is_finite().then_some(n as i64)
}

fn not_found() -> ApiError {
    ApiError::bare(StatusCode::NOT_FOUND)
}

async fn viewer(state: &AppState, user: Option<&SessionUser>) -> ApiResult<Option<Value>> {
    match user {
        Some(u) => db::find_counter(state, &u.uuid).await,
        None => Ok(None),
    }
}

async fn author(state: &AppState, user: &SessionUser) -> ApiResult<Value> {
    match viewer(state, Some(user)).await? {
        Some(c) if !has_role(&c, "muted") && !has_role(&c, "banned") => Ok(c),
        _ => Err(ApiError::bare(StatusCode::FORBIDDEN)),
    }
}

fn is_staff(c: Option<&Value>) -> bool {
    c.is_some_and(|c| has_role(c, "admin") || has_role(c, "mod"))
}

fn can_see(item: &Value, c: Option<&Value>) -> bool {
    if item["isPrivate"] != Value::Bool(true) {
        return true;
    }
    let Some(c) = c else { return false };
    item["authorUuid"] == c["uuid"] || is_staff(Some(c))
}

fn or_str(a: &Value, b: &Value, fallback: &str) -> Value {
    let s = |v: &Value| v.as_str().filter(|s| !s.is_empty()).map(str::to_owned);
    json!(s(a).or_else(|| s(b)).unwrap_or_else(|| fallback.to_owned()))
}

fn shape_author(c: Option<&Value>, uuid: &Value) -> Value {
    let n = Value::Null;
    let c = c.unwrap_or(&n);
    let s = |k: &str| c[k].as_str().filter(|s| !s.is_empty()).map(str::to_owned);
    json!({
        "uuid": s("uuid").map_or(uuid.clone(), Value::from),
        "username": or_str(&c["username"], &c["name"], "unknown"),
        "name": or_str(&c["name"], &c["username"], "unknown"),
        "color": s("color").unwrap_or_default(),
        "discordId": s("discordId").unwrap_or_default(),
        "avatar": s("avatar").unwrap_or_default(),
        "emoji": s("emoji").map_or(Value::Null, Value::from),
    })
}

fn shape(item: &Value, author: Option<&Value>, has_voted: bool, comment_count: i64) -> Value {
    json!({
        "id": item["id"], "title": item["title"], "body": item["body"], "kind": item["kind"],
        "status": item["status"], "isPrivate": item["isPrivate"], "score": item["score"],
        "createdAt": item["createdAt"], "updatedAt": item["updatedAt"], "resolvedAt": item["resolvedAt"],
        "staffNote": item["staffNote"], "author": shape_author(author, &item["authorUuid"]),
        "hasVoted": has_voted, "commentCount": comment_count,
    })
}

async fn list(State(state): State<AppState>, MaybeUser(user): MaybeUser, q: Query) -> ApiResult<Reply> {
    let c = viewer(&state, user.as_ref()).await?;
    let num = |k: &str, d: i64| q.str(k).filter(|s| !s.is_empty()).map_or(Some(d), int);
    let limit = num("limit", 25).map_or(25, |l| l.clamp(1, 100));
    let offset = num("offset", 0).map_or(0, |o| o.max(0));
    let mut wh: Vec<String> = Vec::new();
    let mut args: Vec<Value> = Vec::new();
    if let Some(k) = q.str("kind").filter(|k| KINDS.contains(k)) {
        wh.push("f.kind = ?".into());
        args.push(json!(k));
    }
    let statuses: Vec<String> = q
        .str("status")
        .unwrap_or("")
        .split(',')
        .map(|s| counting::js::trim(s).to_owned())
        .filter(|s| STATUSES.contains(&s.as_str()))
        .collect();
    if !statuses.is_empty() {
        wh.push(format!("f.status IN ({})", db::placeholders(statuses.len())));
        args.extend(statuses.iter().map(|s| json!(s)));
    }
    if q.str("mine") == Some("true")
        && let Some(c) = &c
    {
        wh.push("f.authorUuid = ?".into());
        args.push(c["uuid"].clone());
    }
    if is_staff(c.as_ref()) {
    } else if let Some(c) = &c {
        wh.push("(f.isPrivate = false OR f.authorUuid = ?)".into());
        args.push(c["uuid"].clone());
    } else {
        wh.push("f.isPrivate = false".into());
    }
    let where_sql = if wh.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", wh.join(" AND "))
    };
    let order = if q.str("sort") == Some("new") {
        "f.createdAt DESC"
    } else {
        "f.score DESC, f.createdAt DESC"
    };
    let total_rows = db::rows(
        &state,
        &format!("SELECT COUNT(DISTINCT f.id) AS n FROM feedback f {where_sql}"),
        &args,
    )
    .await?;
    let total = total_rows
        .first()
        .and_then(|r| r.try_get::<i64, _>("n").ok())
        .unwrap_or(0);
    let items: Vec<Value> = db::rows(
        &state,
        &format!("SELECT * FROM feedback f {where_sql} ORDER BY {order} LIMIT {limit} OFFSET {offset}"),
        &args,
    )
    .await?
    .iter()
    .map(|r| entity::FEEDBACK.json(r))
    .collect();
    if items.is_empty() {
        return Ok(reply(
            json!({ "items": [], "total": 0, "offset": offset, "limit": limit }),
        ));
    }
    let ids: Vec<Value> = items.iter().map(|i| i["id"].clone()).collect();
    let mut authors_uuids: Vec<String> = Vec::new();
    for i in &items {
        if let Some(a) = i["authorUuid"].as_str()
            && !authors_uuids.iter().any(|x| x == a)
        {
            authors_uuids.push(a.to_owned());
        }
    }
    let authors = db::find_counters(&state, &authors_uuids).await?;
    let ph = db::placeholders(ids.len());
    let voted: Vec<i64> = match &c {
        Some(c) => {
            let mut a = ids.clone();
            a.push(c["uuid"].clone());
            db::rows(
                &state,
                &format!("SELECT feedbackId FROM feedback_vote WHERE feedbackId IN ({ph}) AND voterUuid = ?"),
                &a,
            )
            .await?
            .iter()
            .filter_map(|r| r.try_get::<i32, _>("feedbackId").ok().map(i64::from))
            .collect()
        }
        None => Vec::new(),
    };
    let counts: Vec<(i64, i64)> = db::rows(
        &state,
        &format!("SELECT feedbackId, COUNT(*) AS n FROM feedback_comment WHERE feedbackId IN ({ph}) AND deleted = false GROUP BY feedbackId"),
        &ids,
    )
    .await?
    .iter()
    .filter_map(|r| Some((i64::from(r.try_get::<i32, _>("feedbackId").ok()?), r.try_get::<i64, _>("n").ok()?)))
    .collect();
    let out: Vec<Value> = items
        .iter()
        .map(|i| {
            let id = i["id"].as_i64().unwrap_or_default();
            let a = authors.iter().find(|a| a["uuid"] == i["authorUuid"]);
            let n = counts.iter().find(|x| x.0 == id).map_or(0, |x| x.1);
            shape(i, a, voted.contains(&id), n)
        })
        .collect();
    Ok(reply(
        json!({ "items": out, "total": total, "offset": offset, "limit": limit }),
    ))
}

async fn find_item(state: &AppState, id: Option<i64>) -> ApiResult<Option<Value>> {
    let Some(id) = id else { return Ok(None) };
    let r = sqlx::query("SELECT * FROM feedback WHERE id = ? LIMIT 1")
        .bind(id)
        .fetch_optional(&state.pool)
        .await?;
    Ok(r.map(|r| entity::FEEDBACK.json(&r)))
}

async fn full(state: &AppState, id: i64, c: Option<&Value>) -> ApiResult<Value> {
    let item = match find_item(state, Some(id)).await? {
        Some(i) if can_see(&i, c) => i,
        _ => return Err(not_found()),
    };
    let author = match item["authorUuid"].as_str() {
        Some(a) => db::find_counter(state, a).await?,
        None => None,
    };
    let voted = match c {
        Some(c) => sqlx::query("SELECT id FROM feedback_vote WHERE feedbackId = ? AND voterUuid = ? LIMIT 1")
            .bind(id)
            .bind(c["uuid"].as_str())
            .fetch_optional(&state.pool)
            .await?
            .is_some(),
        None => false,
    };
    let comments: Vec<Value> =
        sqlx::query("SELECT * FROM feedback_comment WHERE feedbackId = ? ORDER BY createdAt ASC")
            .bind(id)
            .fetch_all(&state.pool)
            .await?
            .iter()
            .map(|r| entity::FEEDBACK_COMMENT.json(r))
            .collect();
    let mut uuids: Vec<String> = Vec::new();
    for cm in &comments {
        if let Some(a) = cm["authorUuid"].as_str()
            && !uuids.iter().any(|x| x == a)
        {
            uuids.push(a.to_owned());
        }
    }
    let commenters = db::find_counters(state, &uuids).await?;
    let live = comments.iter().filter(|c| c["deleted"] != Value::Bool(true)).count() as i64;
    let mut out = shape(&item, author.as_ref(), voted, live);
    out["comments"] = Value::Array(
        comments
            .iter()
            .map(|cm| {
                let deleted = cm["deleted"] == Value::Bool(true);
                json!({
                    "id": cm["id"],
                    "body": if deleted { Value::Null } else { cm["body"].clone() },
                    "deleted": cm["deleted"],
                    "createdAt": cm["createdAt"],
                    "author": shape_author(commenters.iter().find(|x| x["uuid"] == cm["authorUuid"]), &cm["authorUuid"]),
                })
            })
            .collect(),
    );
    Ok(out)
}

async fn get_one(
    State(state): State<AppState>,
    MaybeUser(user): MaybeUser,
    Path(id): Path<String>,
) -> ApiResult<Reply> {
    let Some(id) = int(&id) else {
        return Err(ApiError::bare(StatusCode::BAD_REQUEST));
    };
    let c = viewer(&state, user.as_ref()).await?;
    Ok(reply(full(&state, id, c.as_ref()).await?))
}

fn len16(s: &str) -> usize {
    s.encode_utf16().count()
}

async fn create(State(state): State<AppState>, Authed(user): Authed, body: JsonBody) -> ApiResult<Reply> {
    let c = author(&state, &user).await?;
    let title = counting::js::trim(body.str("title").unwrap_or("")).to_owned();
    let text = counting::js::trim(body.str("body").unwrap_or("")).to_owned();
    if title.is_empty() || len16(&title) > MAX_TITLE {
        return Err(ApiError::bad_request("Title is required."));
    }
    if text.is_empty() || len16(&text) > MAX_BODY {
        return Err(ApiError::bad_request("Description is required."));
    }
    let kind = body.str("kind").filter(|k| KINDS.contains(k)).unwrap_or("bug");
    let now = now_ms();
    let res = sqlx::query(
        "INSERT INTO feedback (authorUuid, title, body, kind, isPrivate, status, score, createdAt, updatedAt) VALUES (?, ?, ?, ?, ?, 'open', 1, ?, ?)",
    )
    .bind(c["uuid"].as_str())
    .bind(&title)
    .bind(&text)
    .bind(kind)
    .bind(truthy(body.get("isPrivate")))
    .bind(now)
    .bind(now)
    .execute(&state.pool)
    .await?;
    let id = res.last_insert_id() as i64;
    sqlx::query("INSERT INTO feedback_vote (feedbackId, voterUuid, createdAt) VALUES (?, ?, ?)")
        .bind(id)
        .bind(c["uuid"].as_str())
        .bind(now)
        .execute(&state.pool)
        .await?;
    Ok(reply(full(&state, id, Some(&c)).await?))
}

async fn vote(State(state): State<AppState>, Authed(user): Authed, Path(id): Path<String>) -> ApiResult<Reply> {
    let c = author(&state, &user).await?;
    let item = match find_item(&state, int(&id)).await? {
        Some(i) if can_see(&i, Some(&c)) => i,
        _ => return Err(not_found()),
    };
    let id = item["id"].as_i64().unwrap_or_default();
    let existing: Option<(i32,)> =
        sqlx::query_as("SELECT id FROM feedback_vote WHERE feedbackId = ? AND voterUuid = ? LIMIT 1")
            .bind(id)
            .bind(c["uuid"].as_str())
            .fetch_optional(&state.pool)
            .await?;
    match existing {
        Some((vid,)) => {
            sqlx::query("DELETE FROM feedback_vote WHERE id = ?")
                .bind(vid)
                .execute(&state.pool)
                .await?;
        }
        None => {
            sqlx::query("INSERT INTO feedback_vote (feedbackId, voterUuid, createdAt) VALUES (?, ?, ?)")
                .bind(id)
                .bind(c["uuid"].as_str())
                .bind(now_ms())
                .execute(&state.pool)
                .await?;
        }
    }
    let (score,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM feedback_vote WHERE feedbackId = ?")
        .bind(id)
        .fetch_one(&state.pool)
        .await?;
    sqlx::query("UPDATE feedback SET score = ? WHERE id = ?")
        .bind(score)
        .bind(id)
        .execute(&state.pool)
        .await?;
    Ok(reply(
        json!({ "id": id, "score": score, "hasVoted": existing.is_none() }),
    ))
}

async fn comment(
    State(state): State<AppState>,
    Authed(user): Authed,
    Path(id): Path<String>,
    body: JsonBody,
) -> ApiResult<Reply> {
    let c = author(&state, &user).await?;
    let item = match find_item(&state, int(&id)).await? {
        Some(i) if can_see(&i, Some(&c)) => i,
        _ => return Err(not_found()),
    };
    let text = counting::js::trim(body.str("body").unwrap_or("")).to_owned();
    if text.is_empty() || len16(&text) > MAX_COMMENT {
        return Err(ApiError::bad_request("Comment is required."));
    }
    let id = item["id"].as_i64().unwrap_or_default();
    sqlx::query("INSERT INTO feedback_comment (feedbackId, authorUuid, body, createdAt) VALUES (?, ?, ?, ?)")
        .bind(id)
        .bind(c["uuid"].as_str())
        .bind(&text)
        .bind(now_ms())
        .execute(&state.pool)
        .await?;
    Ok(reply(full(&state, id, Some(&c)).await?))
}

async fn delete_comment(
    State(state): State<AppState>,
    Authed(user): Authed,
    Path(id): Path<String>,
) -> ApiResult<Reply> {
    let c = author(&state, &user).await?;
    let Some(cid) = int(&id) else { return Err(not_found()) };
    let Some(cm) = sqlx::query("SELECT * FROM feedback_comment WHERE id = ? LIMIT 1")
        .bind(cid)
        .fetch_optional(&state.pool)
        .await?
        .map(|r| entity::FEEDBACK_COMMENT.json(&r))
    else {
        return Err(not_found());
    };
    if cm["authorUuid"] != c["uuid"] && !is_staff(Some(&c)) {
        return Err(ApiError::bare(StatusCode::FORBIDDEN));
    }
    sqlx::query("UPDATE feedback_comment SET deleted = true WHERE id = ?")
        .bind(cid)
        .execute(&state.pool)
        .await?;
    Ok(reply(
        full(&state, cm["feedbackId"].as_i64().unwrap_or_default(), Some(&c)).await?,
    ))
}

async fn moderate(
    State(state): State<AppState>,
    Authed(user): Authed,
    Path(id): Path<String>,
    body: JsonBody,
) -> ApiResult<Reply> {
    let c = author(&state, &user).await?;
    if !is_staff(Some(&c)) {
        return Err(ApiError::bare(StatusCode::FORBIDDEN));
    }
    let Some(item) = find_item(&state, int(&id)).await? else {
        return Err(not_found());
    };
    let id = item["id"].as_i64().unwrap_or_default();
    let now = now_ms();
    let mut sets = vec!["updatedAt = ?".to_owned()];
    let mut args = vec![json!(now)];
    if let Some(status) = body.0.get("status") {
        let Some(s) = status.as_str().filter(|s| STATUSES.contains(s)) else {
            return Err(ApiError::bad_request("Unknown status."));
        };
        sets.push("status = ?".into());
        args.push(json!(s));
        sets.push("resolvedAt = ?".into());
        args.push(json!(if s == "open" { 0 } else { now }));
    }
    if let Some(p) = body.0.get("isPrivate") {
        sets.push("isPrivate = ?".into());
        args.push(json!(truthy(p)));
    }
    if let Some(note) = body.0.get("staffNote") {
        let t: String = counting::js::trim(note.as_str().unwrap_or(""))
            .chars()
            .take(MAX_BODY)
            .collect();
        sets.push("staffNote = ?".into());
        args.push(if t.is_empty() { Value::Null } else { json!(t) });
    }
    args.push(json!(id));
    db::exec(
        &state,
        &format!("UPDATE feedback SET {} WHERE id = ?", sets.join(", ")),
        &args,
    )
    .await?;
    Ok(reply(full(&state, id, Some(&c)).await?))
}
