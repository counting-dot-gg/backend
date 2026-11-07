pub mod validation;

use crate::db::{self, has_role};
use crate::entity::{self, Entity};
use crate::error::{ApiError, ApiResult};
use crate::http::{JsonBody, Query, Reply, reply};
use crate::session::{Authed, MaybeUser, SessionUser};
use crate::state::AppState;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use serde_json::{Map, Value, json};
use sqlx::Row;
use sqlx::mysql::MySqlRow;

const DAILY_UPDATE_LIMIT: i64 = 300;
const HARD_MAX_LIMIT: i64 = 100;
const HARD_MAX_SUMMARY_IDS: usize = 100;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/macro-presets", post(create).get(list))
        .route("/macro-presets/handle/{handle}", get(by_handle))
        .route("/macro-presets/summaries", get(summaries))
        .route("/macro-presets/{id}", get(by_id).patch(patch).delete(remove))
        .route("/macro-presets/{id}/versions", get(versions))
        .route("/macro-presets/{id}/versions/{version}", get(version_by_number))
        .route("/macro-presets/{id}/thread-usage", get(thread_usage))
        .route("/macro-presets/{id}/update-requests", post(enqueue_update))
        .route(
            "/users/me/preferences/macro-preset",
            get(get_user_pref).put(set_user_pref),
        )
        .route("/threads/{thread_id}/preferences/macro-preset", put(set_thread_pref))
        .route("/threads/{thread_id}/macro-presets/{id}/apply", post(track_apply))
        .route("/threads/{thread_id}/macro-presets/recommended", get(recommended))
        .route("/threads/{thread_id}/macro-presets/popular", get(popular))
        .route("/threads/{thread_id}/macro-presets/active", get(active))
}

fn parse_int_pipe(raw: &str) -> ApiResult<i64> {
    let ok = {
        let d = raw.strip_prefix('-').unwrap_or(raw);
        !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit())
    };
    match (ok, raw.parse::<i64>()) {
        (true, Ok(n)) => Ok(n),
        _ => Err(ApiError::bad_request("Validation failed (numeric string is expected)")),
    }
}

fn ensure_enabled() -> ApiResult<()> {
    let v = cgg_core::var("MACRO_PRESETS_V1").unwrap_or_default().to_lowercase();
    if v == "0" || v == "false" || v == "off" {
        return Err(ApiError::not_found("Macro presets feature is disabled"));
    }
    Ok(())
}

fn not_found() -> ApiError {
    ApiError::not_found("Macro preset not found")
}

fn utf16(s: &str) -> usize {
    s.encode_utf16().count()
}

fn trim(s: &str) -> String {
    counting::js::trim(s).to_owned()
}

fn normalize_handle(raw: Option<&str>) -> Option<String> {
    let n = trim(raw.unwrap_or("")).to_lowercase();
    let b = n.as_bytes();
    let ok = (3..=64).contains(&b.len())
        && b[0].is_ascii_lowercase()
        && b[1..]
            .iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_' || *c == b'-');
    ok.then_some(n)
}

fn clamp_limit(raw: Option<&str>, fallback: i64) -> i64 {
    let n = raw.map(|r| counting::js::parse_int(r, Some(10))).unwrap_or(f64::NAN);
    if n.is_nan() {
        return fallback;
    }
    (n.floor() as i64).clamp(1, HARD_MAX_LIMIT)
}

use crate::db::rows;

fn nested(row: &MySqlRow, e: &Entity, alias: &str) -> Value {
    let key = format!("{alias}_{}", e.columns[0].column);
    let present = row
        .try_get_raw(key.as_str())
        .map(|v| !sqlx::ValueRef::is_null(&v))
        .unwrap_or(false);
    if present {
        e.json_prefixed(row, &format!("{alias}_"))
    } else {
        Value::Null
    }
}

fn preset_with_owner(row: &MySqlRow) -> Value {
    let mut p = entity::MACRO_PRESET.json_prefixed(row, "mg_");
    p["ownerCounter"] = nested(row, &entity::COUNTER, "oc");
    p
}

fn preset_owner_select() -> String {
    format!(
        "SELECT {}, {} FROM macro_preset mg LEFT JOIN counters oc ON oc.uuid = mg.ownerCounterUuid",
        entity::MACRO_PRESET.select("mg"),
        entity::COUNTER.select("oc")
    )
}

async fn latest_version_number(state: &AppState, preset_id: i64) -> ApiResult<Value> {
    let r: Option<(i32,)> = sqlx::query_as(
        "SELECT versionNumber FROM macro_preset_version WHERE macroPresetId = ? ORDER BY versionNumber DESC LIMIT 1",
    )
    .bind(preset_id)
    .fetch_optional(&state.pool)
    .await?;
    Ok(r.map_or(Value::Null, |(n,)| json!(n)))
}

async fn find_preset(state: &AppState, where_sql: &str, args: &[Value], with_owner: bool) -> ApiResult<Option<Value>> {
    let sql = if with_owner {
        format!("{} WHERE {where_sql} LIMIT 1", preset_owner_select())
    } else {
        format!(
            "SELECT {} FROM macro_preset mg WHERE {where_sql} LIMIT 1",
            entity::MACRO_PRESET.select("mg")
        )
    };
    let r = rows(state, &sql, args).await?;
    Ok(r.first().map(|row| {
        if with_owner {
            preset_with_owner(row)
        } else {
            entity::MACRO_PRESET.json_prefixed(row, "mg_")
        }
    }))
}

async fn public_preset(state: &AppState, id: i64) -> ApiResult<Value> {
    find_preset(
        state,
        "mg.id = ? AND mg.isDeleted = 0 AND mg.visibility = 'PUBLIC'",
        &[json!(id)],
        false,
    )
    .await?
    .ok_or_else(not_found)
}

async fn find_thread(state: &AppState, thread_id: &str) -> ApiResult<Option<Value>> {
    let t = trim(thread_id);
    if t.is_empty() {
        return Err(ApiError::bad_request("threadId is required"));
    }
    if let Some(th) = db::find_thread_by_uuid(state, &t).await? {
        return Ok(Some(th));
    }
    if t.bytes().all(|b| b.is_ascii_digit()) {
        return db::one(state, &entity::THREAD, "SELECT * FROM thread WHERE id = ? LIMIT 1", &t).await;
    }
    Ok(None)
}

async fn require_user(state: &AppState, user: &SessionUser) -> ApiResult<()> {
    if db::find_user(state, &user.uuid).await?.is_none() {
        return Err(ApiError::not_found("User not found"));
    }
    Ok(())
}

async fn counter_for(state: &AppState, user: &SessionUser) -> ApiResult<Value> {
    db::one(
        state,
        &entity::COUNTER,
        "SELECT * FROM counters WHERE discordId = ? LIMIT 1",
        &user.discord_id,
    )
    .await?
    .ok_or_else(|| ApiError::not_found("Counter not found"))
}

fn can_create(counter: &Value) -> ApiResult<()> {
    if !has_role(counter, "counter") {
        return Err(ApiError::forbidden(
            "Only users with the counter role can create or update macro presets",
        ));
    }
    if has_role(counter, "banned") || has_role(counter, "muted") {
        return Err(ApiError::forbidden(
            "Muted or banned users cannot create or update macro presets",
        ));
    }
    Ok(())
}

fn assert_owner(user: &SessionUser, preset: &Value) -> ApiResult<()> {
    if user.discord_id.is_empty() || preset["ownerCounter"]["discordId"].as_str() != Some(user.discord_id.as_str()) {
        return Err(ApiError::forbidden("Only the owner can modify this macro preset"));
    }
    Ok(())
}

async fn insert_version(
    state: &AppState,
    preset_id: i64,
    number: i64,
    note: Option<&str>,
    by: &str,
    layout: &Value,
    entries: &Value,
) -> ApiResult<i64> {
    let res = sqlx::query(
        "INSERT INTO macro_preset_version (versionNumber, changeNote, macroPresetId, createdByCounterUuid, keyboardLayout) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(number)
    .bind(note)
    .bind(preset_id)
    .bind(by)
    .bind(if layout.is_null() { None } else { Some(layout.to_string()) })
    .execute(&state.pool)
    .await?;
    let version_id = res.last_insert_id() as i64;
    for e in entries.as_array().into_iter().flatten() {
        sqlx::query("INSERT INTO macro_entry (triggerKey, macroType, payloadJson, versionId) VALUES (?, ?, ?, ?)")
            .bind(e["triggerKey"].as_str())
            .bind(e["macroType"].as_str())
            .bind(e["payloadJson"].to_string())
            .bind(version_id)
            .execute(&state.pool)
            .await?;
    }
    Ok(version_id)
}

async fn create(State(state): State<AppState>, Authed(user): Authed, body: JsonBody) -> ApiResult<Reply> {
    ensure_enabled()?;
    let name = trim(body.str("name").unwrap_or(""));
    if name.is_empty() || utf16(&name) < 3 || utf16(&name) > 64 {
        return Err(ApiError::bad_request("name must be between 3 and 64 characters"));
    }
    let Some(handle) = normalize_handle(body.str("handle")) else {
        return Err(ApiError::bad_request(
            "handle must be 3-64 chars, start with a lowercase letter, and then use lowercase letters, numbers, underscores, or hyphens",
        ));
    };
    let taken: Option<(i32,)> = sqlx::query_as("SELECT id FROM macro_preset WHERE handle = ? LIMIT 1")
        .bind(&handle)
        .fetch_optional(&state.pool)
        .await?;
    if taken.is_some() {
        return Err(ApiError::bad_request("handle is already taken"));
    }
    let description = trim(body.str("description").unwrap_or(""));
    if utf16(&description) > 280 {
        return Err(ApiError::bad_request("description exceeds max length 280"));
    }
    let note = Some(trim(body.str("changeNote").unwrap_or(""))).filter(|n| !n.is_empty());
    if note.as_deref().is_some_and(|n| utf16(n) > 280) {
        return Err(ApiError::bad_request("changeNote exceeds max length 280"));
    }
    let entries = match body.get("entries") {
        Value::Null => json!([]),
        e => e.clone(),
    };
    let layout = validation::validate_version(&entries, body.get("keyboardLayout"))?;
    let owner = counter_for(&state, &user).await?;
    can_create(&owner)?;
    let owner_uuid = owner["uuid"].as_str().unwrap_or_default().to_owned();
    let res = sqlx::query("INSERT INTO macro_preset (name, handle, description, visibility, ownerCounterUuid) VALUES (?, ?, ?, 'PUBLIC', ?)")
        .bind(&name)
        .bind(&handle)
        .bind(&description)
        .bind(&owner_uuid)
        .execute(&state.pool)
        .await?;
    let id = res.last_insert_id() as i64;
    insert_version(&state, id, 1, note.as_deref(), &owner_uuid, &layout, &entries).await?;
    let mut preset = find_preset(&state, "mg.id = ?", &[json!(id)], false)
        .await?
        .unwrap_or(Value::Null);
    preset["ownerCounter"] = owner;
    Ok(reply(preset))
}

async fn with_latest(state: &AppState, preset: Value) -> ApiResult<Reply> {
    let id = preset["id"].as_i64().unwrap_or_default();
    let latest = latest_version_number(state, id).await?;
    Ok(reply(json!({ "preset": preset, "latestVersionNumber": latest })))
}

async fn by_id(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Reply> {
    let id = parse_int_pipe(&id)?;
    ensure_enabled()?;
    let p = find_preset(&state, "mg.id = ? AND mg.isDeleted = 0", &[json!(id)], true)
        .await?
        .ok_or_else(not_found)?;
    with_latest(&state, p).await
}

async fn by_handle(State(state): State<AppState>, Path(handle): Path<String>) -> ApiResult<Reply> {
    ensure_enabled()?;
    let handle = normalize_handle(Some(&handle)).ok_or_else(not_found)?;
    let p = find_preset(
        &state,
        "mg.handle = ? AND mg.isDeleted = 0 AND mg.visibility = 'PUBLIC'",
        &[json!(handle)],
        true,
    )
    .await?
    .ok_or_else(not_found)?;
    with_latest(&state, p).await
}

async fn list(State(state): State<AppState>, MaybeUser(user): MaybeUser, q: Query) -> ApiResult<Reply> {
    ensure_enabled()?;
    let int_or = |k: &str, d: f64| {
        let n = counting::js::parse_int(q.str(k).filter(|s| !s.is_empty()).unwrap_or(""), Some(10));
        if n.is_nan() { d } else { n }
    };
    let page_raw = int_or("page", 1.0);
    let limit_raw = int_or("limit", 20.0);
    let page = if page_raw.is_nan() { 1 } else { page_raw.max(1.0) as i64 };
    let limit = limit_raw.clamp(1.0, 100.0) as i64;
    let skip = (page - 1) * limit;

    let mut wh = vec!["mg.isDeleted = 0".to_owned(), "mg.visibility = 'PUBLIC'".to_owned()];
    let mut args: Vec<Value> = Vec::new();
    let mut join = String::new();
    let mut join_args: Vec<Value> = Vec::new();
    let mut order = "mg.updatedAt DESC".to_owned();
    let thread_id = trim(q.str("threadId").unwrap_or(""));
    if !thread_id.is_empty()
        && let Ok(Some(t)) = find_thread(&state, &thread_id).await
        && !t["id"].is_null()
    {
        join = " LEFT JOIN thread_macro_preset_usage threadUsage ON threadUsage.macroPresetId = mg.id AND threadUsage.threadUuid = ?".into();
        join_args.push(t["uuid"].clone());
        order = "threadUsage.appliesCount DESC, mg.updatedAt DESC".into();
    }
    let mine = matches!(q.str("mine"), Some("1" | "true" | "yes"));
    if mine {
        let Some(u) = &user else {
            return Ok(reply(json!({ "page": page, "limit": limit, "total": 0, "items": [] })));
        };
        wh.push("oc.discordId = ?".into());
        args.push(json!(u.discord_id));
    }
    let search = trim(q.str("search").unwrap_or(""));
    if !search.is_empty() {
        let like = format!("%{search}%");
        let lowered = format!("%{}%", search.to_lowercase());
        wh.push(
            "(mg.name LIKE ? OR mg.handle LIKE ? OR mg.description LIKE ? OR oc.name LIKE ? OR oc.username LIKE ? OR oc.uuid LIKE ? \
             OR EXISTS (SELECT 1 FROM macro_preset_version mgv INNER JOIN macro_entry me ON me.versionId = mgv.id WHERE mgv.macroPresetId = mg.id \
               AND (me.triggerKey LIKE ? OR me.macroType LIKE ? OR LOWER(CAST(me.payloadJson AS CHAR)) LIKE ?)) \
             OR EXISTS (SELECT 1 FROM thread_macro_preset_preference tmgp INNER JOIN thread t ON t.uuid = tmgp.threadUuid \
               WHERE tmgp.macroPresetId = mg.id AND tmgp.enabled = 1 AND (t.name LIKE ? OR t.title LIKE ?)))"
                .into(),
        );
        args.extend([
            json!(like),
            json!(like),
            json!(like),
            json!(like),
            json!(like),
            json!(like),
            json!(like),
            json!(like),
            json!(lowered),
            json!(like),
            json!(like),
        ]);
    }
    let owner = trim(q.str("owner").unwrap_or(""));
    if !owner.is_empty() {
        let like = format!("%{owner}%");
        wh.push("(oc.name LIKE ? OR oc.username LIKE ? OR oc.uuid LIKE ?)".into());
        args.extend([json!(like), json!(like), json!(like)]);
    }
    let thread_filter = trim(q.str("thread").unwrap_or(""));
    if !thread_filter.is_empty() {
        let like = format!("%{thread_filter}%");
        wh.push(
            "EXISTS (SELECT 1 FROM thread_macro_preset_preference tmgp INNER JOIN thread t ON t.uuid = tmgp.threadUuid \
             WHERE tmgp.macroPresetId = mg.id AND tmgp.enabled = 1 AND (t.name LIKE ? OR t.title LIKE ?))"
                .into(),
        );
        args.extend([json!(like), json!(like)]);
    }
    let from = format!(
        "FROM macro_preset mg LEFT JOIN counters oc ON oc.uuid = mg.ownerCounterUuid{join} WHERE {}",
        wh.join(" AND ")
    );
    let all_args: Vec<Value> = join_args.iter().chain(args.iter()).cloned().collect();
    let count_rows = rows(&state, &format!("SELECT COUNT(DISTINCT mg.id) AS n {from}"), &all_args).await?;
    let total: i64 = count_rows
        .first()
        .and_then(|r| r.try_get::<i64, _>("n").ok())
        .unwrap_or(0);
    let sql = format!(
        "SELECT {}, {} {from} ORDER BY {order} LIMIT {limit} OFFSET {skip}",
        entity::MACRO_PRESET.select("mg"),
        entity::COUNTER.select("oc")
    );
    let found = rows(&state, &sql, &all_args).await?;
    let items: Vec<Value> = found
        .iter()
        .map(|r| {
            let p = preset_with_owner(r);
            let oc = &p["ownerCounter"];
            let owner = if oc.is_null() {
                Value::Null
            } else {
                let name = oc["name"].as_str().filter(|s| !s.is_empty()).map_or(oc["username"].clone(), |s| json!(s));
                let avatar = oc["avatar"].as_str().filter(|s| !s.is_empty()).map_or(json!("0"), |s| json!(s));
                json!({ "uuid": oc["uuid"], "discordId": oc["discordId"], "name": name, "username": oc["username"], "avatar": avatar })
            };
            json!({
                "id": p["id"], "name": p["name"], "handle": p["handle"], "description": p["description"],
                "visibility": p["visibility"], "isDeleted": p["isDeleted"], "createdAt": p["createdAt"],
                "updatedAt": p["updatedAt"], "ownerCounter": owner,
            })
        })
        .collect();
    Ok(reply(
        json!({ "page": page, "limit": limit, "total": total, "items": items }),
    ))
}

fn entry_json(row: &MySqlRow, prefix: &str) -> Value {
    entity::MACRO_ENTRY.json_prefixed(row, prefix)
}

async fn summaries(State(state): State<AppState>, q: Query) -> ApiResult<Reply> {
    ensure_enabled()?;
    let mut ids: Vec<i64> = Vec::new();
    for raw in q.str("ids").unwrap_or("").split(',') {
        let n = counting::js::parse_int(counting::js::trim(raw), Some(10));
        if n.is_finite() && n > 0.0 && !ids.contains(&(n as i64)) {
            ids.push(n as i64);
        }
    }
    ids.truncate(HARD_MAX_SUMMARY_IDS);
    if ids.is_empty() {
        return Ok(reply(json!({ "items": [] })));
    }
    let usage_limit = clamp_limit(q.str("usageLimit").filter(|s| !s.is_empty()), 3);
    let ph = db::placeholders(ids.len());
    let id_args: Vec<Value> = ids.iter().map(|i| json!(i)).collect();
    let public: Vec<i64> = rows(
        &state,
        &format!("SELECT id FROM macro_preset WHERE id IN ({ph}) AND isDeleted = 0 AND visibility = 'PUBLIC'"),
        &id_args,
    )
    .await?
    .iter()
    .filter_map(|r| r.try_get::<i32, _>("id").ok().map(i64::from))
    .collect();
    if public.is_empty() {
        return Ok(reply(json!({ "items": [] })));
    }
    let pph = db::placeholders(public.len());
    let pargs: Vec<Value> = public.iter().map(|i| json!(i)).collect();
    let latest = rows(
        &state,
        &format!(
            "SELECT v.id AS versionId, v.versionNumber, v.macroPresetId FROM macro_preset_version v INNER JOIN \
             (SELECT macroPresetId, MAX(versionNumber) AS versionNumber FROM macro_preset_version WHERE macroPresetId IN ({pph}) GROUP BY macroPresetId) latest \
             ON latest.macroPresetId = v.macroPresetId AND latest.versionNumber = v.versionNumber"
        ),
        &pargs,
    )
    .await?;
    let mut latest_by: Vec<(i64, i64, i64)> = Vec::new();
    for r in &latest {
        let p = r.try_get::<Option<i32>, _>("macroPresetId").ok().flatten().unwrap_or(0) as i64;
        let v = r.try_get::<i32, _>("versionId").unwrap_or(0) as i64;
        let n = r.try_get::<i32, _>("versionNumber").unwrap_or(0) as i64;
        if p > 0 && v > 0 && n > 0 {
            latest_by.retain(|x| x.0 != p);
            latest_by.push((p, v, n));
        }
    }
    let mut entries_by: Vec<(i64, Vec<Value>)> = Vec::new();
    if !latest_by.is_empty() {
        let vids: Vec<Value> = latest_by.iter().map(|x| json!(x.1)).collect();
        let sql = format!(
            "SELECT {}, {} FROM macro_entry e LEFT JOIN macro_preset_version v ON v.id = e.versionId WHERE e.versionId IN ({}) ORDER BY e.triggerKey ASC",
            entity::MACRO_ENTRY.select("e"),
            entity::MACRO_PRESET_VERSION.select("v"),
            db::placeholders(vids.len())
        );
        for r in rows(&state, &sql, &vids).await? {
            let mut e = entry_json(&r, "e_");
            let version = nested(&r, &entity::MACRO_PRESET_VERSION, "v");
            let vid = version["id"].as_i64();
            e["version"] = version;
            let Some(pid) = latest_by.iter().find(|x| Some(x.1) == vid).map(|x| x.0) else {
                continue;
            };
            match entries_by.iter_mut().find(|x| x.0 == pid) {
                Some(slot) => slot.1.push(e),
                None => entries_by.push((pid, vec![e])),
            }
        }
    }
    let usage = rows(
        &state,
        &format!(
            "SELECT mp.id AS macroPresetId, t.uuid AS threadId, t.name AS threadName, t.title AS threadTitle, COUNT(pref.id) AS appliesCount \
             FROM thread_macro_preset_preference pref INNER JOIN macro_preset mp ON mp.id = pref.macroPresetId \
             INNER JOIN thread t ON t.uuid = pref.threadUuid WHERE pref.enabled = 1 AND mp.id IN ({pph}) \
             GROUP BY mp.id, t.uuid, t.name, t.title ORDER BY mp.id ASC, appliesCount DESC"
        ),
        &pargs,
    )
    .await?;
    let items: Vec<Value> = public
        .iter()
        .map(|pid| {
            let mine: Vec<Value> = usage
                .iter()
                .filter(|r| r.try_get::<i32, _>("macroPresetId").ok().map(i64::from) == Some(*pid))
                .map(|r| {
                    json!({
                        "threadId": r.try_get::<String, _>("threadId").ok(),
                        "threadName": r.try_get::<String, _>("threadName").ok(),
                        "threadTitle": r.try_get::<String, _>("threadTitle").ok(),
                        "appliesCount": r.try_get::<i64, _>("appliesCount").unwrap_or(0),
                    })
                })
                .collect();
            let total = mine.len();
            json!({
                "macroPresetId": pid,
                "latestVersionNumber": latest_by.iter().find(|x| x.0 == *pid).map_or(Value::Null, |x| json!(x.2)),
                "entries": entries_by.iter().find(|x| x.0 == *pid).map_or(json!([]), |x| json!(x.1)),
                "threadUsage": mine.into_iter().take(usage_limit as usize).collect::<Vec<_>>(),
                "threadUsageTotal": total,
            })
        })
        .collect();
    Ok(reply(json!({ "items": items })))
}

async fn owned_preset(state: &AppState, user: &SessionUser, id: i64) -> ApiResult<Value> {
    let p = find_preset(state, "mg.id = ? AND mg.isDeleted = 0", &[json!(id)], true)
        .await?
        .ok_or_else(not_found)?;
    assert_owner(user, &p)?;
    can_create(&counter_for(state, user).await?)?;
    Ok(p)
}

async fn patch(
    State(state): State<AppState>,
    Authed(user): Authed,
    Path(id): Path<String>,
    body: JsonBody,
) -> ApiResult<Reply> {
    let id = parse_int_pipe(&id)?;
    ensure_enabled()?;
    let mut p = owned_preset(&state, &user, id).await?;
    if let Some(n) = body.0.get("name") {
        let name = trim(n.as_str().unwrap_or(""));
        if name.is_empty() || utf16(&name) < 3 || utf16(&name) > 64 {
            return Err(ApiError::bad_request("name must be between 3 and 64 characters"));
        }
        p["name"] = json!(name);
    }
    if let Some(d) = body.0.get("description") {
        let description = trim(d.as_str().unwrap_or(""));
        if utf16(&description) > 280 {
            return Err(ApiError::bad_request("description exceeds max length 280"));
        }
        p["description"] = json!(description);
    }
    sqlx::query("UPDATE macro_preset SET name = ?, description = ?, visibility = 'PUBLIC' WHERE id = ?")
        .bind(p["name"].as_str())
        .bind(p["description"].as_str())
        .bind(id)
        .execute(&state.pool)
        .await?;
    let fresh = find_preset(&state, "mg.id = ?", &[json!(id)], false)
        .await?
        .unwrap_or(Value::Null);
    p["updatedAt"] = fresh["updatedAt"].clone();
    p["visibility"] = json!("PUBLIC");
    Ok(reply(p))
}

async fn remove(State(state): State<AppState>, Authed(user): Authed, Path(id): Path<String>) -> ApiResult<Reply> {
    let id = parse_int_pipe(&id)?;
    ensure_enabled()?;
    owned_preset(&state, &user, id).await?;
    sqlx::query("UPDATE macro_preset SET isDeleted = 1 WHERE id = ?")
        .bind(id)
        .execute(&state.pool)
        .await?;
    Ok(reply(json!({ "success": true })))
}

async fn live_preset(state: &AppState, id: i64) -> ApiResult<()> {
    find_preset(state, "mg.id = ? AND mg.isDeleted = 0", &[json!(id)], false)
        .await?
        .ok_or_else(not_found)?;
    Ok(())
}

fn version_select() -> String {
    format!(
        "SELECT {}, {} FROM macro_preset_version v LEFT JOIN counters cb ON cb.uuid = v.createdByCounterUuid",
        entity::MACRO_PRESET_VERSION.select("v"),
        entity::COUNTER.select("cb")
    )
}

fn version_json(row: &MySqlRow) -> Value {
    let mut v = entity::MACRO_PRESET_VERSION.json_prefixed(row, "v_");
    v["createdByCounter"] = nested(row, &entity::COUNTER, "cb");
    v
}

async fn versions(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Reply> {
    let id = parse_int_pipe(&id)?;
    ensure_enabled()?;
    live_preset(&state, id).await?;
    let r = rows(
        &state,
        &format!(
            "{} WHERE v.macroPresetId = ? ORDER BY v.versionNumber DESC",
            version_select()
        ),
        &[json!(id)],
    )
    .await?;
    Ok(reply(Value::Array(r.iter().map(version_json).collect())))
}

async fn entries_of(state: &AppState, version_id: i64) -> ApiResult<Vec<Value>> {
    let r = rows(
        state,
        "SELECT * FROM macro_entry WHERE versionId = ? ORDER BY triggerKey ASC",
        &[json!(version_id)],
    )
    .await?;
    Ok(r.iter().map(|row| entity::MACRO_ENTRY.json(row)).collect())
}

async fn version_by_number(
    State(state): State<AppState>,
    Path((id, number)): Path<(String, String)>,
) -> ApiResult<Reply> {
    let id = parse_int_pipe(&id)?;
    let number = parse_int_pipe(&number)?;
    ensure_enabled()?;
    live_preset(&state, id).await?;
    let r = rows(
        &state,
        &format!(
            "{} WHERE v.macroPresetId = ? AND v.versionNumber = ? LIMIT 1",
            version_select()
        ),
        &[json!(id), json!(number)],
    )
    .await?;
    let Some(row) = r.first() else {
        return Err(ApiError::not_found("Macro preset version not found"));
    };
    let mut v = version_json(row);
    let vid = v["id"].as_i64().unwrap_or_default();
    v["entries"] = json!(entries_of(&state, vid).await?);
    Ok(reply(v))
}

async fn thread_usage(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Reply> {
    let id = parse_int_pipe(&id)?;
    ensure_enabled()?;
    let p = public_preset(&state, id).await?;
    let base = "FROM thread_macro_preset_preference pref INNER JOIN macro_preset mp ON mp.id = pref.macroPresetId \
                INNER JOIN thread t ON t.uuid = pref.threadUuid WHERE pref.enabled = 1 AND mp.id = ? AND mp.isDeleted = 0 AND mp.visibility = 'PUBLIC'";
    let total = rows(
        &state,
        &format!("SELECT COUNT(DISTINCT t.uuid) AS total {base}"),
        &[json!(id)],
    )
    .await?;
    let total = total
        .first()
        .and_then(|r| r.try_get::<i64, _>("total").ok())
        .unwrap_or(0);
    let r = rows(
        &state,
        &format!(
            "SELECT t.uuid AS threadId, t.name AS threadName, t.title AS threadTitle, COUNT(pref.id) AS appliesCount, MAX(pref.updatedAt) AS lastAppliedAt \
             {base} GROUP BY t.uuid, t.name, t.title ORDER BY appliesCount DESC, lastAppliedAt DESC"
        ),
        &[json!(id)],
    )
    .await?;
    let items: Vec<Value> = r
        .iter()
        .map(|row| {
            let raw = entity::row_json(row);
            json!({
                "threadId": raw["threadId"], "threadName": raw["threadName"], "threadTitle": raw["threadTitle"],
                "appliesCount": row.try_get::<i64, _>("appliesCount").unwrap_or(0),
                "lastAppliedAt": raw["lastAppliedAt"],
            })
        })
        .collect();
    Ok(reply(
        json!({ "macroPresetId": p["id"], "total": total, "items": items }),
    ))
}

async fn enqueue_update(
    State(state): State<AppState>,
    Authed(user): Authed,
    Path(id): Path<String>,
    body: JsonBody,
) -> ApiResult<Reply> {
    let id = parse_int_pipe(&id)?;
    ensure_enabled()?;
    if !body.0.is_object() {
        return Err(ApiError::bad_request("Invalid update payload"));
    }
    let note = Some(trim(body.str("changeNote").unwrap_or(""))).filter(|n| !n.is_empty());
    if note.as_deref().is_some_and(|n| utf16(n) > 280) {
        return Err(ApiError::bad_request("changeNote exceeds max length 280"));
    }
    let entries = match body.get("entries") {
        Value::Null => json!([]),
        e => e.clone(),
    };
    let layout = validation::validate_version(&entries, body.get("keyboardLayout"))?;
    let preset = find_preset(&state, "mg.id = ? AND mg.isDeleted = 0", &[json!(id)], true)
        .await?
        .ok_or_else(not_found)?;
    assert_owner(&user, &preset)?;
    let requester = counter_for(&state, &user).await?;
    can_create(&requester)?;
    let ruuid = requester["uuid"].as_str().unwrap_or_default().to_owned();
    let today: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM macro_update_queue WHERE requestedByCounterUuid = ? AND createdAt >= UTC_DATE() AND createdAt < UTC_DATE() + INTERVAL 1 DAY",
    )
    .bind(&ruuid)
    .fetch_one(&state.pool)
    .await?;
    let payload = body.0.to_string();
    if today.0 >= DAILY_UPDATE_LIMIT {
        sqlx::query(
            "INSERT INTO macro_update_queue (requestPayloadJson, status, errorMessage, processedAt, macroPresetId, requestedByCounterUuid) VALUES (?, 'REJECTED_DAILY_LIMIT', ?, NOW(), ?, ?)",
        )
        .bind(&payload)
        .bind(format!("Exceeded {DAILY_UPDATE_LIMIT} macro updates for UTC day"))
        .bind(id)
        .bind(&ruuid)
        .execute(&state.pool)
        .await?;
        return Err(ApiError::raw(
            StatusCode::TOO_MANY_REQUESTS,
            json!({ "statusCode": 429, "message": format!("Daily macro update limit reached ({DAILY_UPDATE_LIMIT}/day)") }),
        ));
    }
    if same_as_latest(&state, id, &entries, &layout).await? {
        return Err(ApiError::bad_request(
            "No macro changes detected. Update entries before saving a new version.",
        ));
    }
    let res = sqlx::query("INSERT INTO macro_update_queue (requestPayloadJson, status, macroPresetId, requestedByCounterUuid) VALUES (?, 'PENDING', ?, ?)")
        .bind(&payload)
        .bind(id)
        .bind(&ruuid)
        .execute(&state.pool)
        .await?;
    let qid = res.last_insert_id() as i64;
    sqlx::query("UPDATE macro_update_queue SET status = 'PROCESSING' WHERE id = ?")
        .bind(qid)
        .execute(&state.pool)
        .await?;
    let outcome = async {
        let layout = validation::validate_version(&entries, body.get("keyboardLayout"))?;
        if same_as_latest(&state, id, &entries, &layout).await? {
            return Err(ApiError::bad_request(
                "No macro changes detected. Update entries before saving a new version.",
            ));
        }
        let next: (Option<i32>,) =
            sqlx::query_as("SELECT MAX(versionNumber) FROM macro_preset_version WHERE macroPresetId = ?")
                .bind(id)
                .fetch_one(&state.pool)
                .await?;
        insert_version(
            &state,
            id,
            next.0.unwrap_or(0) as i64 + 1,
            note.as_deref(),
            &ruuid,
            &layout,
            &entries,
        )
        .await?;
        Ok(())
    }
    .await;
    match &outcome {
        Ok(()) => {
            sqlx::query("UPDATE macro_update_queue SET status = 'SUCCEEDED', errorMessage = NULL, processedAt = NOW() WHERE id = ?")
                .bind(qid)
                .execute(&state.pool)
                .await?;
        }
        Err(e) => {
            let msg = match e {
                ApiError::Http { message, .. } => crate::http::js_string(message),
                _ => "Unknown error".into(),
            };
            sqlx::query(
                "UPDATE macro_update_queue SET status = 'FAILED', errorMessage = ?, processedAt = NOW() WHERE id = ?",
            )
            .bind(msg.chars().take(500).collect::<String>())
            .bind(qid)
            .execute(&state.pool)
            .await?;
        }
    }
    outcome?;
    let r = rows(
        &state,
        &format!(
            "SELECT {}, {}, {} FROM macro_update_queue q LEFT JOIN macro_preset mg ON mg.id = q.macroPresetId LEFT JOIN counters rc ON rc.uuid = q.requestedByCounterUuid WHERE q.id = ?",
            entity::MACRO_UPDATE_QUEUE.select("q"),
            entity::MACRO_PRESET.select("mg"),
            entity::COUNTER.select("rc")
        ),
        &[json!(qid)],
    )
    .await?;
    let out = r.first().map_or(Value::Null, |row| {
        let mut q = entity::MACRO_UPDATE_QUEUE.json_prefixed(row, "q_");
        q["macroPreset"] = nested(row, &entity::MACRO_PRESET, "mg");
        q["requestedByCounter"] = nested(row, &entity::COUNTER, "rc");
        q
    });
    Ok(reply(out))
}

fn normalize_json(v: &Value) -> Value {
    match v {
        Value::Array(a) => Value::Array(a.iter().map(normalize_json).collect()),
        Value::Object(o) => {
            let mut keys: Vec<&String> = o.keys().collect();
            keys.sort();
            Value::Object(
                keys.into_iter()
                    .map(|k| (k.clone(), normalize_json(&o[k])))
                    .collect::<Map<_, _>>(),
            )
        }
        Value::Number(n) => n.as_f64().map_or(v.clone(), entity::js_number),
        other => other.clone(),
    }
}

fn config_key(entries: &[Value], layout: &Value) -> Value {
    let mut norm: Vec<Value> = entries
        .iter()
        .map(|e| {
            json!({
                "triggerKey": e["triggerKey"],
                "macroType": e["macroType"],
                "payloadJson": normalize_json(if e["payloadJson"].is_null() { &Value::Null } else { &e["payloadJson"] }),
            })
        })
        .collect();
    norm.sort_by_key(|e| e.to_string());
    json!({ "entries": norm, "keyboardLayout": if layout.is_null() { Value::Null } else { normalize_json(layout) } })
}

async fn same_as_latest(state: &AppState, preset_id: i64, entries: &Value, layout: &Value) -> ApiResult<bool> {
    let r = rows(
        state,
        "SELECT id, keyboardLayout FROM macro_preset_version WHERE macroPresetId = ? ORDER BY versionNumber DESC LIMIT 1",
        &[json!(preset_id)],
    )
    .await?;
    let Some(row) = r.first() else {
        return Ok(false);
    };
    let vid: i32 = row.try_get("id")?;
    let stored_layout = entity::cell(row, "keyboardLayout", entity::Kind::Json);
    let stored: Vec<Value> = entries_of(state, i64::from(vid))
        .await?
        .into_iter()
        .map(|e| {
            let p = if e["payloadJson"].is_null() {
                json!({})
            } else {
                e["payloadJson"].clone()
            };
            json!({ "triggerKey": e["triggerKey"], "macroType": e["macroType"], "payloadJson": p })
        })
        .collect();
    let incoming: Vec<Value> = entries
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|e| {
            let p = if e["payloadJson"].is_null() {
                json!({})
            } else {
                e["payloadJson"].clone()
            };
            json!({ "triggerKey": e["triggerKey"], "macroType": e["macroType"], "payloadJson": p })
        })
        .collect();
    Ok(config_key(&stored, &stored_layout) == config_key(&incoming, layout))
}

async fn preset_id_of_pref(state: &AppState, sql: &str, args: &[Value]) -> ApiResult<Option<(Option<i64>, bool)>> {
    let r = rows(state, sql, args).await?;
    Ok(r.first().map(|row| {
        let pid = row
            .try_get::<Option<i32>, _>("macroPresetId")
            .ok()
            .flatten()
            .map(i64::from);
        let enabled = row.try_get::<i8, _>("enabled").map(|e| e != 0).unwrap_or(false);
        (pid, enabled)
    }))
}

async fn user_pref_preset(state: &AppState, uuid: &str) -> ApiResult<Option<i64>> {
    Ok(preset_id_of_pref(
        state,
        "SELECT p.macroPresetId, 1 AS enabled FROM user_macro_preset_preference p INNER JOIN macro_preset mp ON mp.id = p.macroPresetId WHERE p.userUuid = ? LIMIT 1",
        &[json!(uuid)],
    )
    .await?
    .and_then(|(p, _)| p))
}

async fn get_user_pref(State(state): State<AppState>, Authed(user): Authed) -> ApiResult<Reply> {
    ensure_enabled()?;
    require_user(&state, &user).await?;
    let pid = user_pref_preset(&state, &user.uuid).await?;
    let latest = match pid {
        Some(p) => latest_version_number(&state, p).await?,
        None => Value::Null,
    };
    Ok(reply(json!({ "macroPresetId": pid, "latestVersionNumber": latest })))
}

fn optional_id(v: &Value) -> Option<i64> {
    match v {
        Value::Null => None,
        other => Some(crate::http::js_number(other) as i64),
    }
}

async fn set_user_pref(State(state): State<AppState>, Authed(user): Authed, body: JsonBody) -> ApiResult<Reply> {
    ensure_enabled()?;
    require_user(&state, &user).await?;
    let preset = match optional_id(body.get("macroPresetId")) {
        Some(id) => Some(public_preset(&state, id).await?),
        None => None,
    };
    let pid = preset.as_ref().and_then(|p| p["id"].as_i64());
    let existing: Option<(i32,)> =
        sqlx::query_as("SELECT id FROM user_macro_preset_preference WHERE userUuid = ? LIMIT 1")
            .bind(&user.uuid)
            .fetch_optional(&state.pool)
            .await?;
    match existing {
        Some((id,)) => {
            sqlx::query("UPDATE user_macro_preset_preference SET macroPresetId = ?, updatedAt = CURRENT_TIMESTAMP(6) WHERE id = ?")
                .bind(pid)
                .bind(id)
                .execute(&state.pool)
                .await?;
        }
        None => {
            sqlx::query("INSERT INTO user_macro_preset_preference (userUuid, macroPresetId) VALUES (?, ?)")
                .bind(&user.uuid)
                .bind(pid)
                .execute(&state.pool)
                .await?;
        }
    }
    let latest = match pid {
        Some(p) => latest_version_number(&state, p).await?,
        None => Value::Null,
    };
    Ok(reply(json!({ "macroPresetId": pid, "latestVersionNumber": latest })))
}

async fn thread_or_404(state: &AppState, thread_id: &str) -> ApiResult<Value> {
    find_thread(state, thread_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Thread not found"))
}

async fn set_thread_pref(
    State(state): State<AppState>,
    Authed(user): Authed,
    Path(thread_id): Path<String>,
    body: JsonBody,
) -> ApiResult<Reply> {
    ensure_enabled()?;
    let Some(enabled) = body.get("enabled").as_bool() else {
        return Err(ApiError::bad_request("enabled must be a boolean"));
    };
    require_user(&state, &user).await?;
    let thread = thread_or_404(&state, &thread_id).await?;
    let tuuid = thread["uuid"].as_str().unwrap_or_default().to_owned();
    let preset = match optional_id(body.get("macroPresetId")) {
        Some(id) => Some(public_preset(&state, id).await?),
        None => None,
    };
    let pid = preset.as_ref().and_then(|p| p["id"].as_i64());
    let existing: Option<(i32,)> =
        sqlx::query_as("SELECT id FROM thread_macro_preset_preference WHERE userUuid = ? AND threadUuid = ? LIMIT 1")
            .bind(&user.uuid)
            .bind(&tuuid)
            .fetch_optional(&state.pool)
            .await?;
    match existing {
        Some((id,)) => {
            sqlx::query("UPDATE thread_macro_preset_preference SET enabled = ?, macroPresetId = ?, updatedAt = CURRENT_TIMESTAMP(6) WHERE id = ?")
                .bind(enabled)
                .bind(pid)
                .bind(id)
                .execute(&state.pool)
                .await?;
        }
        None => {
            sqlx::query("INSERT INTO thread_macro_preset_preference (enabled, userUuid, threadUuid, macroPresetId) VALUES (?, ?, ?, ?)")
                .bind(enabled)
                .bind(&user.uuid)
                .bind(&tuuid)
                .bind(pid)
                .execute(&state.pool)
                .await?;
        }
    }
    let latest = match pid {
        Some(p) => latest_version_number(&state, p).await?,
        None => Value::Null,
    };
    Ok(reply(
        json!({ "threadId": tuuid, "enabled": enabled, "macroPresetId": pid, "latestVersionNumber": latest }),
    ))
}

async fn track_apply(
    State(state): State<AppState>,
    Authed(user): Authed,
    Path((thread_id, id)): Path<(String, String)>,
) -> ApiResult<Reply> {
    let id = parse_int_pipe(&id)?;
    ensure_enabled()?;
    require_user(&state, &user).await?;
    let thread = thread_or_404(&state, &thread_id).await?;
    let tuuid = thread["uuid"].as_str().unwrap_or_default().to_owned();
    let preset = public_preset(&state, id).await?;
    let pid = preset["id"].as_i64().unwrap_or(id);
    let existing: Option<(i32, i32)> = sqlx::query_as(
        "SELECT id, appliesCount FROM thread_macro_preset_usage WHERE threadUuid = ? AND macroPresetId = ? LIMIT 1",
    )
    .bind(&tuuid)
    .bind(pid)
    .fetch_optional(&state.pool)
    .await?;
    let count = match existing {
        Some((uid, n)) => {
            sqlx::query("UPDATE thread_macro_preset_usage SET appliesCount = ?, lastAppliedAt = CURRENT_TIMESTAMP(6) WHERE id = ?")
                .bind(n + 1)
                .bind(uid)
                .execute(&state.pool)
                .await?;
            n + 1
        }
        None => {
            sqlx::query(
                "INSERT INTO thread_macro_preset_usage (appliesCount, threadUuid, macroPresetId) VALUES (1, ?, ?)",
            )
            .bind(&tuuid)
            .bind(pid)
            .execute(&state.pool)
            .await?;
            1
        }
    };
    Ok(reply(
        json!({ "threadId": tuuid, "macroPresetId": pid, "appliesCount": count }),
    ))
}

async fn usage_for_thread(state: &AppState, thread_uuid: &str) -> ApiResult<(Vec<Value>, i64)> {
    let base = "FROM thread_macro_preset_preference pref INNER JOIN macro_preset mp ON mp.id = pref.macroPresetId \
                INNER JOIN thread t ON t.uuid = pref.threadUuid WHERE pref.enabled = 1 AND t.uuid = ? AND mp.isDeleted = 0 AND mp.visibility = 'PUBLIC'";
    let total = rows(
        state,
        &format!("SELECT COUNT(DISTINCT mp.id) AS total {base}"),
        &[json!(thread_uuid)],
    )
    .await?;
    let total = total
        .first()
        .and_then(|r| r.try_get::<i64, _>("total").ok())
        .unwrap_or(0);
    let r = rows(
        state,
        &format!(
            "SELECT mp.id AS macroPresetId, COUNT(pref.id) AS appliesCount, MAX(pref.updatedAt) AS lastAppliedAt {base} \
             GROUP BY mp.id ORDER BY appliesCount DESC, lastAppliedAt DESC"
        ),
        &[json!(thread_uuid)],
    )
    .await?;
    let ids: Vec<i64> = r
        .iter()
        .filter_map(|row| row.try_get::<i32, _>("macroPresetId").ok().map(i64::from))
        .filter(|i| *i > 0)
        .collect();
    if ids.is_empty() {
        return Ok((Vec::new(), total));
    }
    let id_args: Vec<Value> = ids.iter().map(|i| json!(i)).collect();
    let presets: Vec<Value> = rows(
        state,
        &format!(
            "{} WHERE mg.id IN ({}) AND mg.isDeleted = 0 AND mg.visibility = 'PUBLIC'",
            preset_owner_select(),
            db::placeholders(ids.len())
        ),
        &id_args,
    )
    .await?
    .iter()
    .map(preset_with_owner)
    .collect();
    let items = r
        .iter()
        .filter_map(|row| {
            let pid = row.try_get::<i32, _>("macroPresetId").ok().map(i64::from)?;
            let preset = presets.iter().find(|p| p["id"].as_i64() == Some(pid))?;
            let raw = entity::row_json(row);
            Some(json!({
                "id": pid,
                "appliesCount": row.try_get::<i64, _>("appliesCount").unwrap_or(0),
                "lastAppliedAt": raw["lastAppliedAt"],
                "macroPreset": preset,
            }))
        })
        .collect();
    Ok((items, total))
}

async fn thread_presets(state: &AppState, thread_id: &str) -> ApiResult<Reply> {
    ensure_enabled()?;
    let thread = thread_or_404(state, thread_id).await?;
    let tuuid = thread["uuid"].as_str().unwrap_or_default();
    let (items, total) = usage_for_thread(state, tuuid).await?;
    Ok(reply(json!({ "threadId": tuuid, "total": total, "items": items })))
}

async fn recommended(State(state): State<AppState>, Path(thread_id): Path<String>) -> ApiResult<Reply> {
    thread_presets(&state, &thread_id).await
}

async fn popular(State(state): State<AppState>, Path(thread_id): Path<String>) -> ApiResult<Reply> {
    thread_presets(&state, &thread_id).await
}

fn none_runtime(source: &str) -> Reply {
    reply(json!({
        "source": source, "enabled": false, "macroPresetId": null, "macroPresetVersionId": null,
        "macroPresetVersionNumber": null, "entries": [],
    }))
}

async fn active(
    State(state): State<AppState>,
    Authed(user): Authed,
    Path(thread_id): Path<String>,
) -> ApiResult<Reply> {
    ensure_enabled()?;
    require_user(&state, &user).await?;
    let thread = thread_or_404(&state, &thread_id).await?;
    let tuuid = thread["uuid"].as_str().unwrap_or_default().to_owned();
    let pref = preset_id_of_pref(
        &state,
        "SELECT p.enabled, mp.id AS macroPresetId FROM thread_macro_preset_preference p LEFT JOIN macro_preset mp ON mp.id = p.macroPresetId WHERE p.userUuid = ? AND p.threadUuid = ? LIMIT 1",
        &[json!(user.uuid), json!(tuuid)],
    )
    .await?;
    let (source, preset) = match pref {
        Some((Some(pid), true)) => ("thread", Some(public_preset(&state, pid).await?)),
        Some((None, true)) => return Ok(none_runtime("thread")),
        _ => match user_pref_preset(&state, &user.uuid).await? {
            Some(pid) => ("global", Some(public_preset(&state, pid).await?)),
            None => ("none", None),
        },
    };
    let Some(preset) = preset else {
        return Ok(none_runtime("none"));
    };
    let pid = preset["id"].as_i64().unwrap_or_default();
    let latest = rows(
        &state,
        "SELECT * FROM macro_preset_version WHERE macroPresetId = ? ORDER BY versionNumber DESC LIMIT 1",
        &[json!(pid)],
    )
    .await?;
    let Some(v) = latest.first().map(|r| entity::MACRO_PRESET_VERSION.json(r)) else {
        return Ok(none_runtime("none"));
    };
    let entries = entries_of(&state, v["id"].as_i64().unwrap_or_default()).await?;
    Ok(reply(json!({
        "source": source,
        "enabled": true,
        "macroPresetId": pid,
        "macroPresetVersionId": v["id"],
        "macroPresetVersionNumber": v["versionNumber"],
        "entries": entries,
        "keyboardLayout": v["keyboardLayout"],
    })))
}
