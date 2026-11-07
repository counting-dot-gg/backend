use crate::achievements;
use crate::db::{self, has_role};
use crate::entity;
use crate::error::{ApiError, ApiResult};
use crate::http::{JsonBody, Query, Reply, empty, js_number, reply};
use crate::session::{Authed, MaybeUser, SessionUser};
use crate::state::AppState;
use crate::statsread::{coverage, store};
use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use serde_json::{Map, Value, json};
use stats::achievements::{AchievementRef, LiveEvent};
use std::sync::atomic::Ordering;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/counter/info", get(info))
        .route("/counter/load_counter", post(load_counter))
        .route("/counter/load_counters", post(load_counters))
        .route("/counter/counters/{page}", get(counters_page))
        .route("/counter/countersByXP/{page}", get(counters_by_xp))
        .route("/counter/achievements/{uuid}", get(achievements_for))
        .route("/counter/achievement/{id}", get(achievement))
        .route("/counter/makeDiscordVerified", post(make_discord_verified))
        .route("/counter/makeCounter", post(make_counter))
        .route("/counter/awardAchievement", post(award_achievement))
        .route("/counter/denyCounter", post(deny_counter))
        .route("/counter/update_profile", post(update_profile))
        .route("/counter/profile_page/{uuid}", get(profile_page))
        .route("/counter/unapproved", get(unapproved))
        .route("/counter/admin/list", get(admin_list))
        .route("/counter/admin/update", post(admin_update))
        .route("/counter/register", post(register))
        .route("/counter/modToggleBan", post(mod_toggle_ban))
        .route("/counter/modToggleMute", post(mod_toggle_mute))
        .route("/counter/getMentions", get(get_mentions))
        .route("/counter/changeMessageReadStatus", post(change_read_status))
        .route("/counter/disconnectAll", get(disconnect_all))
        .route("/counter/adminSyncDiscordRoles", post(sync_roles))
        .route("/counter/adminSendSystemMessage", post(send_system_message))
        .route("/counter/createItem", post(create_item))
}

pub fn is_uuid(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 36 {
        return false;
    }
    let hex = |r: std::ops::Range<usize>| b[r].iter().all(u8::is_ascii_hexdigit);
    let dashes = b[8] == b'-' && b[13] == b'-' && b[18] == b'-' && b[23] == b'-';
    if !(dashes && hex(0..8) && hex(9..13) && hex(14..18) && hex(19..23) && hex(24..36)) {
        return false;
    }
    let lower = s.to_ascii_lowercase();
    if lower == "00000000-0000-0000-0000-000000000000" || lower == "ffffffff-ffff-ffff-ffff-ffffffffffff" {
        return true;
    }
    (b'1'..=b'8').contains(&b[14]) && matches!(b[19].to_ascii_lowercase(), b'8' | b'9' | b'a' | b'b')
}

fn bare(s: StatusCode) -> ApiError {
    ApiError::bare(s)
}

pub fn thread_visible_to(thread: &Value, counter: Option<&Value>) -> bool {
    let visible = thread["visibleTo"].as_array();
    let has = |r: &str| visible.is_some_and(|v| v.iter().any(|x| x == r));
    match counter {
        Some(c) if has_role(c, "admin") => true,
        Some(c) => {
            has("all")
                || c["roles"]
                    .as_array()
                    .is_some_and(|rs| rs.iter().any(|r| r.as_str().is_some_and(has)))
        }
        None => has("all"),
    }
}

async fn info(State(state): State<AppState>, MaybeUser(user): MaybeUser) -> ApiResult<Reply> {
    let Some(user) = user else { return Ok(empty()) };
    db::find_counter(&state, &user.uuid)
        .await?
        .map(reply)
        .ok_or_else(|| bare(StatusCode::BAD_REQUEST))
}

async fn first_post(state: &AppState, author_id: &Value) -> ApiResult<Option<String>> {
    let Some(id) = author_id.as_i64().filter(|i| *i > 0) else {
        return Ok(None);
    };
    let row: Option<(String, Option<u16>)> =
        sqlx::query_as("SELECT uuid, threadId FROM post WHERE authorId = ? ORDER BY id ASC LIMIT 1")
            .bind(id)
            .fetch_optional(&state.pool)
            .await?;
    let Some((uuid, Some(tid))) = row else { return Ok(None) };
    let thread = db::one(
        state,
        &entity::THREAD,
        "SELECT * FROM thread WHERE id = ? LIMIT 1",
        &tid.to_string(),
    )
    .await?;
    Ok(thread
        .filter(|t| thread_visible_to(t, None))
        .and_then(|t| t["name"].as_str().map(|n| format!("{n}/{uuid}"))))
}

async fn load_counter(State(state): State<AppState>, body: JsonBody) -> ApiResult<Reply> {
    let key = crate::http::js_string(body.get("uuid"));
    let counter = if is_uuid(&key) {
        db::find_counter(&state, &key).await?
    } else {
        db::find_counter_by_username(&state, &key).await?
    };
    let Some(counter) = counter.filter(|c| has_role(c, "counter") || has_role(c, "bot")) else {
        return Err(bare(StatusCode::NOT_FOUND));
    };
    let uuid = counter["uuid"].as_str().unwrap_or_default().to_owned();
    let fallback = || reply(json!({ "loadedCounter": counter, "loadedCounterStats": {} }));
    let Some(stats) = store::counter_stats(&state, &uuid).await.filter(|s| s.is_object()) else {
        return Ok(fallback());
    };
    let first = first_post(&state, &counter["id"]).await?;
    let mut real = Map::new();
    for (name, v) in stats.as_object().into_iter().flatten() {
        if name == "all" {
            let mut all = v.clone();
            if let (Some(f), Some(o)) = (&first, all.as_object_mut()) {
                o.insert("first_count".into(), json!(f));
            }
            real.insert("all".into(), all);
            continue;
        }
        let Some(thread) = db::find_thread_by_name(&state, name)
            .await?
            .filter(|t| thread_visible_to(t, None))
        else {
            return Ok(fallback());
        };
        let mut entry = v.clone();
        if let Some(o) = entry.as_object_mut() {
            o.insert("thread_name".into(), json!(name));
        }
        real.insert(crate::http::js_string(&thread["title"]), entry);
    }
    let (days, time_days, seconds, endings) = tokio::join!(
        store::counter_days(&state, &uuid, "counter_days"),
        store::counter_days(&state, &uuid, "counter_time_days"),
        coverage::counter_seconds(&state, &uuid, false),
        coverage::counter_endings(&state, &uuid),
    );
    Ok(reply(json!({
        "endingsCoverage": endings,
        "loadedCounter": counter,
        "loadedCounterStats": real,
        "contributionDays": days,
        "timeDays": time_days,
        "secondsCoverage": seconds,
    })))
}

async fn load_counters(State(state): State<AppState>, body: JsonBody) -> ApiResult<Reply> {
    let uuids: Vec<String> = body
        .get("uuid")
        .as_array()
        .map(|a| a.iter().map(crate::http::js_string).collect())
        .unwrap_or_default();
    Ok(reply(db::find_counters(&state, &uuids).await?))
}

fn page_offset(page: &str) -> i64 {
    let n = counting::js::parse_int(page, Some(10));
    ((if n.is_finite() { n } else { 1.0 }).max(1.0) as i64 - 1) * 50
}

async fn counters_page(State(state): State<AppState>, Path(page): Path<String>) -> ApiResult<Reply> {
    let rows = sqlx::query("SELECT * FROM counters WHERE roles LIKE '%counter%' ORDER BY id ASC LIMIT 50 OFFSET ?")
        .bind(page_offset(&page))
        .fetch_all(&state.pool)
        .await?;
    let total = state.caches.counter_role_total.load(Ordering::Relaxed);
    Ok(reply(
        json!({ "counters": entity::COUNTER.rows(&rows), "pageCount": [total] }),
    ))
}

async fn counters_by_xp(State(state): State<AppState>, Path(page): Path<String>) -> ApiResult<Reply> {
    let rows = sqlx::query("SELECT * FROM counters WHERE roles LIKE '%counter%' ORDER BY xp DESC LIMIT 50 OFFSET ?")
        .bind(page_offset(&page))
        .fetch_all(&state.pool)
        .await?;
    let total = state.caches.counter_role_total.load(Ordering::Relaxed);
    Ok(reply(
        json!({ "counters": entity::COUNTER.rows(&rows), "pageCount": [total] }),
    ))
}

fn strip_secret(a: &mut Value) {
    if let Some(o) = a.as_object_mut() {
        o.remove("description");
        o.remove("maxProgress");
    }
}

async fn all_achievements(state: &AppState) -> ApiResult<Vec<Value>> {
    Ok(sqlx::query("SELECT * FROM achievements")
        .fetch_all(&state.pool)
        .await?
        .iter()
        .map(|r| entity::ACHIEVEMENT.json(r))
        .collect())
}

async fn achievements_for(
    State(state): State<AppState>,
    MaybeUser(user): MaybeUser,
    Path(uuid): Path<String>,
) -> ApiResult<Reply> {
    let mut uuid = Some(uuid).filter(|u| !u.is_empty() && u != "undefined");
    if uuid.is_none()
        && let Some(u) = &user
    {
        uuid = Some(u.uuid.clone());
    }
    if let Some(u) = uuid.clone()
        && !is_uuid(&u)
        && let Some(c) = db::find_counter_by_username(&state, &u).await?
    {
        uuid = c["uuid"].as_str().map(str::to_owned);
    }
    let all = all_achievements(&state).await?;
    let mine: Vec<Value> = match &uuid {
        Some(u) => sqlx::query("SELECT * FROM counter_achievements WHERE counterUUID = ?")
            .bind(u)
            .fetch_all(&state.pool)
            .await?
            .iter()
            .map(|r| entity::COUNTER_ACHIEVEMENT.json(r))
            .filter(|ca| {
                ca["isComplete"] == Value::Bool(true)
                    || all
                        .iter()
                        .any(|a| a["id"] == ca["achievementId"] && a["isPublic"] == Value::Bool(true))
            })
            .collect(),
        None => Vec::new(),
    };
    let earned: Vec<Value> = sqlx::query("SELECT * FROM achievements WHERE countersEarned > 0 OR isPublic = 1")
        .fetch_all(&state.pool)
        .await?
        .iter()
        .map(|r| {
            let mut a = entity::ACHIEVEMENT.json(r);
            if a["isPublic"] != Value::Bool(true) {
                strip_secret(&mut a);
            }
            a
        })
        .collect();
    Ok(reply(json!({ "user_achievements": mine, "all_achievements": earned })))
}

async fn achievement(
    State(state): State<AppState>,
    MaybeUser(user): MaybeUser,
    Path(id): Path<String>,
) -> ApiResult<Reply> {
    let row = sqlx::query(
        "SELECT * FROM achievements WHERE ((isPublic = 0 AND countersEarned > 0) OR isPublic = 1) AND id = ? LIMIT 1",
    )
    .bind(&id)
    .fetch_optional(&state.pool)
    .await?;
    let Some(mut a) = row.map(|r| entity::ACHIEVEMENT.json(&r)) else {
        return Err(bare(StatusCode::NOT_FOUND));
    };
    let cas: Vec<Value> = sqlx::query("SELECT * FROM counter_achievements WHERE achievementId = ?")
        .bind(a["id"].as_i64())
        .fetch_all(&state.pool)
        .await?
        .iter()
        .map(|r| entity::COUNTER_ACHIEVEMENT.json(r))
        .collect();
    let mut earners: Vec<String> = Vec::new();
    for ca in cas.iter().filter(|c| c["isComplete"] == Value::Bool(true)) {
        if let Some(u) = ca["counterUUID"].as_str()
            && !earners.iter().any(|e| e == u)
        {
            earners.push(u.to_owned());
        }
    }
    let public = a["isPublic"] == Value::Bool(true);
    if !public && earners.is_empty() {
        return Err(bare(StatusCode::NOT_FOUND));
    }
    let counters = db::find_counters(&state, &earners).await?;
    if !public && user.as_ref().is_none_or(|u| !earners.contains(&u.uuid)) {
        strip_secret(&mut a);
    }
    Ok(reply(
        json!({ "achievement": a, "counterAchievements": cas, "countersWhoEarnedAchievement": counters }),
    ))
}

async fn require_admin_forbidden(state: &AppState, user: &SessionUser) -> ApiResult<Value> {
    match db::find_counter(state, &user.uuid).await? {
        Some(c) if has_role(&c, "admin") => Ok(c),
        _ => Err(bare(StatusCode::FORBIDDEN)),
    }
}

async fn require_admin_hidden(state: &AppState, user: &SessionUser) -> ApiResult<Value> {
    match db::find_counter(state, &user.uuid).await? {
        Some(c) if has_role(&c, "admin") => Ok(c),
        _ => Err(bare(StatusCode::NOT_FOUND)),
    }
}

async fn set_roles(state: &AppState, uuid: &str, roles: &[&str]) -> ApiResult<Option<Value>> {
    let Some(c) = db::find_counter(state, uuid).await? else {
        return Ok(None);
    };
    let mut sets = vec!["roles = ?".to_owned()];
    let mut args = vec![json!(json!(roles).to_string())];
    if has_role(&c, "banned") || (has_role(&c, "muted") && crate::routes::user::truthy(&c["avatar"])) {
        sets.push("avatar = '0'".into());
    }
    if has_role(&c, "banned") && crate::routes::user::truthy(&c["title"]) {
        sets.push("title = 'BANNED'".into());
    } else if has_role(&c, "muted") && crate::routes::user::truthy(&c["title"]) {
        sets.push("title = 'MUTED'".into());
    }
    args.push(json!(uuid));
    db::exec(
        state,
        &format!("UPDATE counters SET {} WHERE uuid = ?", sets.join(", ")),
        &args,
    )
    .await?;
    db::find_counter(state, uuid).await
}

fn ids_of(body: &JsonBody) -> Vec<String> {
    match body.get("counter_ids") {
        Value::Array(a) => a.iter().map(crate::http::js_string).collect(),
        Value::String(s) => s.chars().map(|c| c.to_string()).collect(),
        _ => Vec::new(),
    }
}

async fn make_discord_verified(
    State(state): State<AppState>,
    Authed(user): Authed,
    body: JsonBody,
) -> ApiResult<Reply> {
    require_admin_forbidden(&state, &user).await?;
    for id in ids_of(&body) {
        let Some(c) = set_roles(&state, &id, &["discord_verified"]).await? else {
            return Err(ApiError::Internal(anyhow::anyhow!("counter {id} not found")));
        };
        state.bot.emit(
            "addDiscordRole",
            json!({ "discordId": c["discordId"], "role": "discord_verified_role" }),
        );
        state.bot.emit(
            "removeDiscordRole",
            json!({ "discordId": c["discordId"], "role": "manual_verification_needed_role" }),
        );
    }
    Ok(empty())
}

pub fn format_ordinal(n: i64) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

fn announce_join(state: &AppState, c: &Value) {
    state.socket.emit(
        "counterJoined",
        json!({ "counter": { "uuid": c["uuid"], "name": c["name"], "username": c["username"], "color": c["color"], "discordId": c["discordId"], "avatar": c["avatar"] } }),
    );
}

async fn refresh_counter_total(state: &AppState) -> i64 {
    let n = crate::routes::auth::counter_role_total(state).await.unwrap_or(0);
    state.caches.counter_role_total.store(n, Ordering::Relaxed);
    n
}

async fn make_counter(State(state): State<AppState>, Authed(user): Authed, body: JsonBody) -> ApiResult<Reply> {
    require_admin_forbidden(&state, &user).await?;
    for id in ids_of(&body) {
        let Some(c) = set_roles(&state, &id, &["counter"]).await? else {
            return Err(ApiError::Internal(anyhow::anyhow!("counter {id} not found")));
        };
        let uuid = c["uuid"].as_str().unwrap_or_default().to_owned();
        state.bot.emit(
            "addDiscordRole",
            json!({ "discordId": c["discordId"], "role": "counter_role" }),
        );
        state.bot.emit(
            "removeDiscordRole",
            json!({ "discordId": c["discordId"], "role": "discord_verified_role" }),
        );
        achievements::record(&state, &uuid, LiveEvent::JoinedAsCounter);
        state.bot.emit("sendWelcome", json!({ "newCounter": c }));
        announce_join(&state, &c);
        let total = refresh_counter_total(&state).await;
        state.socket.emit(
            "sendSystemMessage",
            json!({ "message": format!(
                "{} ({}) has joined counting.gg as its {} member! Welcome!",
                crate::http::js_string(&c["name"]),
                crate::http::js_string(&c["username"]),
                format_ordinal(total)
            ) }),
        );
    }
    Ok(empty())
}

async fn award_achievement(State(state): State<AppState>, Authed(user): Authed, body: JsonBody) -> ApiResult<Reply> {
    require_admin_forbidden(&state, &user).await?;
    let uuid = crate::http::js_string(body.get("counter_uuid"));
    let Some(c) = db::find_counter(&state, &uuid).await? else {
        return Err(bare(StatusCode::BAD_REQUEST));
    };
    let id = js_number(body.get("achievement_id"));
    if id.is_finite()
        && let Err(e) = achievements::earn(&state, &uuid, AchievementRef::Id(id as i32)).await
    {
        tracing::warn!("{e:#}");
    }
    Ok(reply(json!(format!(
        "Achievement {} given to {}",
        crate::http::js_string(body.get("achievement_id")),
        crate::http::js_string(&c["name"])
    ))))
}

async fn deny_counter(State(state): State<AppState>, Authed(user): Authed, body: JsonBody) -> ApiResult<Reply> {
    require_admin_forbidden(&state, &user).await?;
    for id in ids_of(&body) {
        if set_roles(&state, &id, &["denied"]).await?.is_none() {
            return Err(ApiError::Internal(anyhow::anyhow!("counter {id} not found")));
        }
    }
    Ok(empty())
}

fn profile_writable(c: &Value) -> ApiResult<()> {
    if has_role(c, "banned") || has_role(c, "muted") || has_role(c, "denied") {
        return Err(ApiError::forbidden("Your account cannot update its profile."));
    }
    Ok(())
}

async fn update_profile(State(state): State<AppState>, Authed(user): Authed, body: JsonBody) -> ApiResult<Reply> {
    let Some(c) = db::find_counter(&state, &user.uuid).await? else {
        return Err(bare(StatusCode::BAD_REQUEST));
    };
    profile_writable(&c)?;
    let bio = body.0.get("bio");
    if let Some(b) = bio
        && b.as_str().is_none_or(|s| s.encode_utf16().count() > 500)
    {
        return Err(ApiError::bad_request("Bio too long"));
    }
    let links = body.0.get("socialLinks");
    if let Some(l) = links {
        let Some(arr) = l.as_array().filter(|a| a.len() <= 8) else {
            return Err(ApiError::bad_request("Too many links"));
        };
        for link in arr {
            if !crate::routes::user::truthy(&link["platform"]) || !crate::routes::user::truthy(&link["handle"]) {
                return Err(ApiError::bad_request("Invalid link"));
            }
            if !link["platform"]
                .as_str()
                .is_some_and(|p| p == "discord" || p == "reddit")
            {
                return Err(ApiError::bad_request("Unknown platform"));
            }
            if link["handle"].as_str().is_none_or(|h| h.encode_utf16().count() > 100) {
                return Err(ApiError::bad_request("Handle too long"));
            }
        }
    }
    let existing = db::one(
        &state,
        &entity::COUNTER_PROFILE_PAGE,
        "SELECT * FROM counter_profile_pages WHERE counterUUID = ?",
        &user.uuid,
    )
    .await?;
    let mut profile = existing.clone().unwrap_or_else(|| json!({ "counterUUID": user.uuid }));
    if let Some(b) = bio.and_then(Value::as_str) {
        let t = counting::js::trim(b);
        profile["bio"] = if t.is_empty() { Value::Null } else { json!(t) };
    }
    if let Some(l) = links {
        profile["socialLinks"] = l.clone();
    }
    let col = |v: &Value| if v.is_null() { Value::Null } else { json!(v.to_string()) };
    if existing.is_some() {
        db::exec(
            &state,
            "UPDATE counter_profile_pages SET bio = ?, socialLinks = ? WHERE counterUUID = ?",
            &[profile["bio"].clone(), col(&profile["socialLinks"]), json!(user.uuid)],
        )
        .await?;
    } else {
        db::exec(
            &state,
            "INSERT INTO counter_profile_pages (counterUUID, bio, socialLinks) VALUES (?, ?, ?)",
            &[json!(user.uuid), profile["bio"].clone(), col(&profile["socialLinks"])],
        )
        .await?;
    }
    Ok(reply(profile))
}

async fn profile_page(State(state): State<AppState>, Path(uuid): Path<String>) -> ApiResult<Reply> {
    let p = db::one(
        &state,
        &entity::COUNTER_PROFILE_PAGE,
        "SELECT * FROM counter_profile_pages WHERE counterUUID = ?",
        &uuid,
    )
    .await?;
    Ok(reply(p.unwrap_or_else(
        || json!({ "counterUUID": uuid, "bio": null, "socialLinks": null }),
    )))
}

async fn pending(state: &AppState) -> ApiResult<Vec<Value>> {
    Ok(sqlx::query("SELECT * FROM counters WHERE roles LIKE '%unverified%' OR roles LIKE '%discord_verified%' OR roles LIKE '%manual_verification_needed%'")
        .fetch_all(&state.pool)
        .await?
        .iter()
        .map(|r| entity::COUNTER.json(r))
        .collect())
}

async fn unapproved(State(state): State<AppState>, MaybeUser(user): MaybeUser) -> ApiResult<Reply> {
    let Some(user) = user else {
        return Err(bare(StatusCode::NOT_FOUND));
    };
    require_admin_hidden(&state, &user).await?;
    Ok(reply(pending(&state).await?))
}

async fn admin_list(State(state): State<AppState>, Authed(user): Authed, q: Query) -> ApiResult<Reply> {
    require_admin_hidden(&state, &user).await?;
    let p = counting::js::parse_int(q.str("page").unwrap_or("1"), Some(10));
    let page = if p.is_nan() || p == 0.0 { 1 } else { (p as i64).max(1) };
    let search = q.str("q").map(counting::js::trim).filter(|s| !s.is_empty());
    let (where_sql, args): (&str, Vec<Value>) = match search {
        Some(s) => {
            let like = format!("%{s}%");
            (
                "WHERE c.username LIKE ? OR c.name LIKE ? OR c.uuid LIKE ? OR c.discordId LIKE ?",
                vec![json!(like), json!(like), json!(like), json!(like)],
            )
        }
        None => ("", vec![]),
    };
    let total_rows = db::rows(
        &state,
        &format!("SELECT COUNT(*) AS n FROM counters c {where_sql}"),
        &args,
    )
    .await?;
    let total = total_rows
        .first()
        .map_or(0, |r| js_number(&entity::row_json(r)["n"]) as i64);
    let rows: Vec<Value> = db::rows(
        &state,
        &format!(
            "SELECT * FROM counters c {where_sql} ORDER BY c.id DESC LIMIT 50 OFFSET {}",
            (page - 1) * 50
        ),
        &args,
    )
    .await?
    .iter()
    .map(|r| entity::COUNTER.json(r))
    .collect();
    let uuids: Vec<Value> = rows.iter().map(|c| c["uuid"].clone()).collect();
    let money: Vec<(String, f64)> = if uuids.is_empty() {
        Vec::new()
    } else {
        db::rows(
            &state,
            &format!(
                "SELECT uuid, money FROM users WHERE uuid IN ({})",
                db::placeholders(uuids.len())
            ),
            &uuids,
        )
        .await?
        .iter()
        .map(|r| {
            let j = entity::row_json(r);
            (crate::http::js_string(&j["uuid"]), js_number(&j["money"]))
        })
        .collect()
    };
    let counters: Vec<Value> = rows
        .into_iter()
        .map(|mut c| {
            let m = money.iter().find(|(u, _)| c["uuid"] == *u).map_or(0.0, |x| x.1);
            c["money"] = entity::js_number(m);
            c
        })
        .collect();
    Ok(reply(
        json!({ "total": total, "page": page, "pageCount": (total as f64 / 50.0).ceil() as i64, "counters": counters }),
    ))
}

const COUNTER_FIELDS: [&str; 13] = [
    "name",
    "username",
    "color",
    "roles",
    "cardStyle",
    "cardBorderStyle",
    "pronouns",
    "title",
    "xp",
    "rainbow",
    "emoji",
    "avatar",
    "discordId",
];
const USER_FIELDS: [(&str, &str); 6] = [
    ("money", "money"),
    ("timeOnline", "timeOnline"),
    ("reddit", "reddit"),
    ("redditAccess", "redditAccess"),
    ("redditRefresh", "redditRefresh"),
    ("discordId", "discord_id"),
];

fn json_col(k: &str, v: &Value) -> Value {
    if matches!(k, "roles" | "pronouns") && !v.is_null() {
        json!(v.to_string())
    } else {
        v.clone()
    }
}

async fn admin_update(State(state): State<AppState>, Authed(user): Authed, body: JsonBody) -> ApiResult<Reply> {
    let admin = require_admin_hidden(&state, &user).await?;
    let uuid = body.str("uuid").filter(|u| !u.is_empty()).map(str::to_owned);
    let cc = body.get("counterChanges").as_object().cloned().unwrap_or_default();
    let uc = body.get("userChanges").as_object().cloned().unwrap_or_default();
    let Some(uuid) = uuid else {
        return Err(ApiError::bad_request("uuid is required."));
    };
    if cc.is_empty() && uc.is_empty() {
        return Err(ApiError::bad_request("Nothing to change."));
    }
    if admin["uuid"] == uuid
        && let Some(roles) = cc.get("roles").and_then(Value::as_array)
        && !roles.iter().any(|r| r == "admin")
    {
        return Err(ApiError::bad_request(
            "You cannot remove your own admin role here. Ask another admin.",
        ));
    }
    let Some(counter) = db::find_counter(&state, &uuid).await? else {
        return Err(ApiError::not_found(format!("Counter {uuid} not found.")));
    };
    let mut cp: Map<String, Value> = COUNTER_FIELDS
        .iter()
        .filter_map(|k| cc.get(*k).map(|v| ((*k).to_owned(), v.clone())))
        .collect();
    let mut up: Map<String, Value> = USER_FIELDS
        .iter()
        .filter_map(|(k, _)| uc.get(*k).map(|v| ((*k).to_owned(), v.clone())))
        .collect();
    if let Some(name) = cp.get("username").filter(|n| **n != counter["username"])
        && let Some(clash) = db::find_counter_by_username(&state, &crate::http::js_string(name)).await?
        && clash["uuid"] != uuid
    {
        return Err(ApiError::bad_request(format!(
            "Another counter already uses the username \"{}\".",
            crate::http::js_string(name)
        )));
    }
    let new_discord = cp
        .get("discordId")
        .filter(|v| !v.is_null())
        .or_else(|| up.get("discordId").filter(|v| !v.is_null()))
        .cloned();
    if let Some(nd) = new_discord.filter(|nd| *nd != counter["discordId"]) {
        let id = crate::http::js_string(&nd);
        let digits = |s: &str| (17..=20).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit());
        let snowflake = digits(&id);
        let retired = {
            let n = id.bytes().take_while(u8::is_ascii_digit).count();
            (17..=20).contains(&n) && id.len() > n && !id.as_bytes()[n].is_ascii_digit()
        };
        if !snowflake && !retired {
            return Err(ApiError::bad_request(format!(
                "\"{id}\" is not a Discord ID. Use 17-20 digits, or a snowflake with a suffix (e.g. \"123456789012345678-retired\") to retire one."
            )));
        }
        if let Some(c) = db::one(
            &state,
            &entity::COUNTER,
            "SELECT * FROM counters WHERE discordId = ? LIMIT 1",
            &id,
        )
        .await?
            && c["uuid"] != uuid
        {
            return Err(ApiError::bad_request(format!(
                "Discord ID {id} already belongs to @{}.",
                crate::http::js_string(&c["username"])
            )));
        }
        if let Some(u) = db::one(
            &state,
            &entity::USER,
            "SELECT * FROM users WHERE discord_id = ? LIMIT 1",
            &id,
        )
        .await?
            && u["uuid"] != uuid
        {
            return Err(ApiError::bad_request(format!(
                "Discord ID {id} is already attached to another user row ({}).",
                crate::http::js_string(&u["uuid"])
            )));
        }
        cp.insert("discordId".into(), json!(id));
        up.insert("discordId".into(), json!(id));
        tracing::warn!(
            "discord id change for counter {} ({uuid}) -> {id}",
            crate::http::js_string(&counter["username"])
        );
    }
    if !cp.is_empty() {
        let sets: Vec<String> = cp.keys().map(|k| format!("`{k}` = ?")).collect();
        let mut args: Vec<Value> = cp.iter().map(|(k, v)| json_col(k, v)).collect();
        args.push(json!(uuid));
        db::exec(
            &state,
            &format!("UPDATE counters SET {} WHERE uuid = ?", sets.join(", ")),
            &args,
        )
        .await?;
    }
    if !up.is_empty() {
        if db::find_user(&state, &uuid).await?.is_none() {
            return Err(ApiError::not_found(format!(
                "Counter {uuid} has no user row, so user fields can't be written."
            )));
        }
        if let Some(m) = up.get("money") {
            let n = js_number(m);
            up.insert("money".into(), json!(if n.is_nan() { 0 } else { n as i64 }));
        }
        let sets: Vec<String> = up
            .keys()
            .map(|k| {
                format!(
                    "`{}` = ?",
                    USER_FIELDS.iter().find(|(p, _)| p == k).map_or(k.as_str(), |x| x.1)
                )
            })
            .collect();
        let mut args: Vec<Value> = up.values().cloned().collect();
        args.push(json!(uuid));
        db::exec(
            &state,
            &format!("UPDATE users SET {} WHERE uuid = ?", sets.join(", ")),
            &args,
        )
        .await?;
    }
    if cp.contains_key("roles") {
        refresh_counter_total(&state).await;
    }
    tracing::info!(
        "Admin {} edited counter {uuid}",
        crate::http::js_string(&admin["username"])
    );
    Ok(db::find_counter(&state, &uuid).await?.map_or_else(empty, reply))
}

pub fn validate_update(d: &Value) -> Option<bool> {
    let name = d["name"].as_str()?;
    let username = d["username"].as_str()?;
    let color = d["color"].as_str()?;
    let nt = counting::js::trim(name);
    let ut = counting::js::trim(username);
    let ct = counting::js::trim(color);
    let len = |s: &str| s.encode_utf16().count();
    let pronoun = js_number(&d["pronouns"]);
    let bad = [
        len(nt) < 1,
        len(nt) > 25,
        len(ut) < 1,
        len(ut) > 25,
        !crate::routes::user::name_allowed(name),
        username
            .chars()
            .any(|c| !(c.is_ascii_alphanumeric() || c == '-' || c == '_')),
        len(ct) != 7,
        !ct.starts_with('#'),
        !crate::consts::is_hex6(ct.get(1..).unwrap_or("")),
        pronoun < 0.0,
        pronoun > 2.0,
    ];
    Some(!bad.iter().any(|b| *b))
}

async fn register(State(state): State<AppState>, Authed(user): Authed, body: JsonBody) -> ApiResult<Reply> {
    let update = body.get("update").clone();
    match validate_update(&update) {
        Some(true) => {}
        Some(false) => return Err(ApiError::bad_request("Validation failed.")),
        None => return Err(ApiError::Internal(anyhow::anyhow!("register payload missing fields"))),
    }
    let Some(counter) = db::find_counter(&state, &user.uuid).await? else {
        return Err(ApiError::bad_request("Counter not found..."));
    };
    profile_writable(&counter)?;
    if let Some(name) = update["username"].as_str().filter(|n| !n.is_empty())
        && let Some(c) = db::find_counter_by_username(&state, name).await?
        && c["uuid"] != user.uuid
    {
        return Err(ApiError::bad_request(
            "Another counter with this username already exists.",
        ));
    }
    let mut with_updates = counter.clone();
    if let (Some(o), Some(u)) = (with_updates.as_object_mut(), update.as_object()) {
        for (k, v) in u {
            o.insert(k.clone(), v.clone());
        }
    }
    let first = !crate::routes::user::truthy(&counter["color"]);
    let mut roles: Vec<String> = counter["roles"]
        .as_array()
        .map(|a| a.iter().map(crate::http::js_string).collect())
        .unwrap_or_default();
    if first {
        roles = vec!["counter".into()];
        state.bot.emit(
            "addDiscordRole",
            json!({ "discordId": counter["discordId"], "role": "counter_role" }),
        );
        state.bot.emit(
            "removeDiscordRole",
            json!({ "discordId": counter["discordId"], "role": "discord_verified_role" }),
        );
        achievements::record(&state, &user.uuid, LiveEvent::JoinedAsCounter);
        state.bot.emit("sendWelcome", json!({ "newCounter": with_updates }));
        announce_join(&state, &with_updates);
    }
    let pronoun_set = crate::consts::PRONOUNS
        .get(js_number(&update["pronouns"]) as usize)
        .map_or(Value::Null, |p| json!(p));
    db::exec(
        &state,
        "UPDATE counters SET name = ?, username = ?, color = ?, pronouns = ?, roles = ? WHERE uuid = ?",
        &[
            json!(counting::js::trim(update["name"].as_str().unwrap_or(""))),
            json!(counting::js::trim(update["username"].as_str().unwrap_or(""))),
            json!(counting::js::trim(update["color"].as_str().unwrap_or(""))),
            if pronoun_set.is_null() {
                Value::Null
            } else {
                json!(pronoun_set.to_string())
            },
            json!(json!(roles).to_string()),
            json!(user.uuid),
        ],
    )
    .await?;
    if first {
        let (s, uuid) = (state.clone(), user.uuid.clone());
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            s.bot.emit("newUserLog", json!({ "newCounterUUID": uuid }));
        });
    }
    if roles.iter().any(|r| r == "counter") {
        refresh_counter_total(&state).await;
    }
    Ok(empty())
}

async fn moderator(state: &AppState, user: &SessionUser) -> ApiResult<Value> {
    match db::find_counter(state, &user.uuid).await? {
        Some(c) if has_role(&c, "mod") => Ok(c),
        _ => Err(bare(StatusCode::NOT_FOUND)),
    }
}

async fn toggle(state: &AppState, actor: &Value, uuid: &str, mute: bool) -> ApiResult<Reply> {
    let Some(mut target) = db::find_counter(state, uuid).await? else {
        return Err(ApiError::Internal(anyhow::anyhow!("counter {uuid} not found")));
    };
    if has_role(&target, "admin") {
        return Ok(empty());
    }
    let role = if mute { "muted" } else { "banned" };
    let mut roles: Vec<Value> = target["roles"].as_array().cloned().unwrap_or_default();
    if let Some(i) = roles.iter().position(|r| r == role) {
        roles.remove(i);
        target["title"] = json!("COUNTER");
    } else {
        if mute {
            target["name"] = target["username"].clone();
        }
        roles.push(json!(role));
        target["title"] = json!(if mute { "MUTED" } else { "BANNED" });
        target["avatar"] = json!("0");
    }
    target["roles"] = Value::Array(roles);
    db::exec(
        state,
        "UPDATE counters SET roles = ?, title = ?, avatar = ?, name = ? WHERE uuid = ?",
        &[
            json!(target["roles"].to_string()),
            target["title"].clone(),
            target["avatar"].clone(),
            target["name"].clone(),
            json!(uuid),
        ],
    )
    .await?;
    if mute {
        state
            .bot
            .emit("logMute", json!({ "muter": actor, "counterToMute": target }));
    } else {
        state
            .bot
            .emit("logBan", json!({ "banner": actor, "counterToBan": target }));
    }
    Ok(empty())
}

async fn mod_toggle_ban(State(state): State<AppState>, Authed(user): Authed, body: JsonBody) -> ApiResult<Reply> {
    let m = moderator(&state, &user).await?;
    toggle(&state, &m, &crate::http::js_string(body.get("uuid")), false).await
}

async fn mod_toggle_mute(State(state): State<AppState>, Authed(user): Authed, body: JsonBody) -> ApiResult<Reply> {
    let m = moderator(&state, &user).await?;
    toggle(&state, &m, &crate::http::js_string(body.get("uuid")), true).await
}

async fn get_mentions(State(state): State<AppState>, Authed(user): Authed, q: Query) -> ApiResult<Reply> {
    let Some(c) = db::find_counter(&state, &user.uuid).await? else {
        return Err(bare(StatusCode::FORBIDDEN));
    };
    let uuid = c["uuid"].clone();
    let from = q.str("from").map(crate::http::js_to_number).filter(|f| *f > 0.0);
    let mut args = vec![uuid.clone()];
    let extra = match from {
        Some(f) => {
            args.push(json!(f));
            " AND m.id < ?"
        }
        None => "",
    };
    let rows = db::rows(
        &state,
        &format!("SELECT {}, {} FROM mentions m LEFT JOIN counters c ON c.uuid = m.counterUuid WHERE m.counterUuid = ?{extra} ORDER BY m.id DESC LIMIT 50", entity::MENTIONS.select("m"), entity::COUNTER.select("c")),
        &args,
    )
    .await?;
    let mut mentions: Vec<Value> = rows
        .iter()
        .map(|r| {
            let mut m = entity::MENTIONS.json_prefixed(r, "m_");
            m["counter"] = entity::COUNTER.json_prefixed(r, "c_");
            m
        })
        .collect();
    let mut posts = Vec::new();
    let mut threads: Vec<Value> = Vec::new();
    let mut authors: Vec<String> = Vec::new();
    for m in mentions.iter_mut() {
        if let Some(p) = db::one(
            &state,
            &entity::POST,
            "SELECT * FROM post WHERE uuid = ? LIMIT 1",
            m["postUUID"].as_str().unwrap_or(""),
        )
        .await?
        {
            if let Some(t) = db::find_thread_by_uuid_value(&state, &p["thread"]).await? {
                threads.push(t);
            } else {
                threads.push(Value::Null);
            }
            if let Some(a) = p["authorUUID"].as_str()
                && !authors.iter().any(|x| x == a)
            {
                authors.push(a.to_owned());
            }
            posts.push(p);
        }
        if m["readStatus"] == Value::Bool(false) {
            m["readStatus"] = Value::Bool(true);
            db::exec(
                &state,
                "UPDATE mentions SET readStatus = 1 WHERE id = ?",
                &[m["id"].clone()],
            )
            .await?;
        }
    }
    let counters = db::find_counters(&state, &authors).await?;
    Ok(reply(
        json!({ "mentions": mentions, "posts": posts, "counters": counters, "threads": threads }),
    ))
}

async fn change_read_status(State(state): State<AppState>, Authed(user): Authed, body: JsonBody) -> ApiResult<Reply> {
    let Some(c) = db::find_counter(&state, &user.uuid).await? else {
        return Err(bare(StatusCode::NOT_FOUND));
    };
    let ids: Vec<Value> = body.get("messageIDs").as_array().cloned().unwrap_or_default();
    let new_status = crate::routes::user::truthy(body.get("newReadStatus"));
    if ids.is_empty() {
        return Ok(reply(json!([])));
    }
    let mut args = vec![c["uuid"].clone()];
    args.extend(ids.iter().cloned());
    args.push(json!(!new_status));
    let rows = db::rows(
        &state,
        &format!(
            "SELECT {}, {} FROM mentions m LEFT JOIN counters c ON c.uuid = m.counterUuid WHERE m.counterUuid = ? AND m.postUUID IN ({}) AND m.readStatus = ? ORDER BY m.id DESC LIMIT 1000",
            entity::MENTIONS.select("m"),
            entity::COUNTER.select("c"),
            db::placeholders(ids.len())
        ),
        &args,
    )
    .await?;
    let mut out = Vec::new();
    for r in &rows {
        let mut m = entity::MENTIONS.json_prefixed(r, "m_");
        m["counter"] = entity::COUNTER.json_prefixed(r, "c_");
        m["readStatus"] = json!(new_status);
        db::exec(
            &state,
            "UPDATE mentions SET readStatus = ? WHERE id = ?",
            &[json!(new_status), m["id"].clone()],
        )
        .await?;
        out.push(m);
    }
    Ok(reply(json!(out)))
}

async fn disconnect_all(State(state): State<AppState>, Authed(user): Authed) -> ApiResult<Reply> {
    require_admin_forbidden(&state, &user).await?;
    state.socket.emit("disconnectAllClients", json!({}));
    Ok(empty())
}

async fn sync_roles(State(state): State<AppState>, Authed(user): Authed) -> ApiResult<Reply> {
    require_admin_hidden(&state, &user).await?;
    state.bot.emit("syncAllRoles", json!({}));
    Ok(reply(json!({ "started": true })))
}

async fn send_system_message(State(state): State<AppState>, Authed(user): Authed, body: JsonBody) -> ApiResult<Reply> {
    require_admin_hidden(&state, &user).await?;
    state
        .socket
        .emit("sendSystemMessage", json!({ "message": body.get("message") }));
    Ok(reply(json!(true)))
}

async fn create_item(State(state): State<AppState>, Authed(user): Authed, body: JsonBody) -> ApiResult<Reply> {
    let Some(c) = db::find_counter(&state, &user.uuid).await? else {
        return Err(bare(StatusCode::NOT_FOUND));
    };
    if has_role(&c, "admin") {
        let f = |k: &str| body.get(k).clone();
        db::exec(
            &state,
            "INSERT INTO item (uuid, name, internal_name, description, category, unlockMethod, unlockDescription, price, quantity, levelToUnlock, achievementId) VALUES (UUID(), ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            &[f("name"), f("internal_name"), f("description"), f("category"), f("unlockMethod"), f("unlockDescription"), f("price"), f("quantity"), f("levelToUnlock"), f("achievementId")],
        )
        .await?;
    }
    Ok(empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinals_match_intl() {
        assert_eq!(format_ordinal(1), "1st");
        assert_eq!(format_ordinal(12), "12th");
        assert_eq!(format_ordinal(23), "23rd");
        assert_eq!(format_ordinal(111), "111th");
        assert_eq!(format_ordinal(102), "102nd");
    }

    #[test]
    fn uuids_validate_like_the_uuid_package() {
        assert!(is_uuid("16770632-9392-4503-8201-136327503201"));
        assert!(!is_uuid("pull"));
        assert!(!is_uuid("16770632-9392-0503-8201-136327503201"));
    }
}
