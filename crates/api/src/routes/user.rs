use crate::consts::*;
use crate::db::{self, has_role};
use crate::entity;
use crate::error::{ApiError, ApiResult};
use crate::http::{JsonBody, Query, Reply, empty, js_number, reply};
use crate::session::{Authed, SessionUser};
use crate::state::AppState;
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use serde_json::{Map, Value, json};
use std::sync::LazyLock;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/user/prefs", post(update_prefs))
        .route("/user/threadPrefs", post(update_thread_prefs))
        .route("/user/threadPacePrefs", post(update_thread_pace_prefs))
        .route("/user/deleteThreadPrefs", delete(delete_thread_prefs))
        .route("/user/prefStats", get(pref_stats))
        .route("/user/shop", get(shop))
}

const PREF_KEYS: [&str; 28] = [
    "pref_discord_pings",
    "pref_online",
    "pref_load_from_bottom",
    "pref_custom_stricken",
    "pref_nightMode",
    "pref_clear",
    "pref_standardize_format",
    "pref_strike_color",
    "pref_submit_shortcut",
    "pref_time_since_last_count",
    "pref_post_style",
    "pref_post_style_mobile",
    "pref_reply_time_interval",
    "pref_night_mode_colors",
    "pref_post_position",
    "pref_hide_stricken",
    "pref_hide_thread_picker",
    "pref_highlight_last_count",
    "pref_highlight_last_count_color",
    "pref_sound_on_stricken",
    "pref_stricken_count_opacity",
    "pref_post_font_size",
    "pref_timestamp_display",
    "pref_show_latency",
    "pref_pace_mode",
    "pref_pace_counts",
    "pref_chat_default",
    "pref_milestones",
];

fn is_bool(v: &Value) -> bool {
    v.is_boolean()
}

fn is_integer(v: &Value) -> bool {
    v.as_f64().is_some_and(|f| f.is_finite() && f.fract() == 0.0)
}

fn loose(v: &Value) -> f64 {
    match v {
        Value::Array(a) if a.is_empty() => 0.0,
        Value::Array(a) if a.len() == 1 => loose(&a[0]),
        Value::Object(_) | Value::Array(_) => f64::NAN,
        other => js_number(other),
    }
}

fn in_range(v: &Value, lo: f64, hi: f64) -> bool {
    let n = loose(v);
    lo <= n && n <= hi
}

fn one_of(v: &Value, options: &[&str]) -> bool {
    v.as_str().is_some_and(|s| options.contains(&s))
}

fn hex_after_hash(v: &Value) -> Result<bool, ()> {
    match v {
        Value::Null => Ok(is_hex6("")),
        Value::String(s) => Ok(is_hex6(counting::js::trim(s).get(1..).unwrap_or(""))),
        _ => Err(()),
    }
}

pub fn validate_prefs(prefs: &Map<String, Value>) -> Result<bool, ()> {
    let get = |k: &str| prefs.get(k);
    let mut checks: Vec<(&str, bool)> = Vec::new();
    let v = |k: &str| get(k).cloned().unwrap_or(Value::Null);
    for k in [
        "pref_discord_pings",
        "pref_online",
        "pref_load_from_bottom",
        "pref_highlight_last_count",
        "pref_hide_thread_picker",
        "pref_time_since_last_count",
    ] {
        checks.push((k, !is_bool(&v(k))));
    }
    let interval = v("pref_reply_time_interval");
    checks.push(("pref_reply_time_interval", !interval.is_number()));
    checks.push(("pref_reply_time_interval", !in_range(&interval, 1.0, 10000.0)));
    checks.push((
        "pref_stricken_count_opacity",
        !in_range(&v("pref_stricken_count_opacity"), 0.0, 1.0),
    ));
    checks.push(("pref_reply_time_interval", !is_integer(&interval)));
    checks.push((
        "pref_night_mode_colors",
        !one_of(&v("pref_night_mode_colors"), &NIGHT_MODE_COLORS),
    ));
    checks.push(("pref_post_position", !one_of(&v("pref_post_position"), &POST_POSITION)));
    checks.push(("pref_hide_stricken", !one_of(&v("pref_hide_stricken"), &HIDE_STRICKEN)));
    checks.push((
        "pref_standardize_format",
        !one_of(&v("pref_standardize_format"), &STANDARDIZE_FORMAT),
    ));
    checks.push(("pref_nightMode", !one_of(&v("pref_nightMode"), &NIGHT_MODE)));
    checks.push((
        "pref_submit_shortcut",
        !one_of(&v("pref_submit_shortcut"), &SUBMIT_SHORTCUT),
    ));
    checks.push((
        "pref_custom_stricken",
        !one_of(&v("pref_custom_stricken"), &CUSTOM_STRICKEN),
    ));
    checks.push((
        "pref_sound_on_stricken",
        !one_of(&v("pref_sound_on_stricken"), &SOUND_ON_STRICKEN),
    ));
    checks.push(("pref_pace_mode", !one_of(&v("pref_pace_mode"), &PACE_MODE)));
    let counts = v("pref_pace_counts");
    let n = loose(&counts);
    let whole_in_range = n.is_finite() && n.fract() == 0.0 && (1.0..=1_000_000.0).contains(&n);
    checks.push(("pref_pace_counts", !counts.is_null() && !whole_in_range));
    if let Some(cd) = get("pref_chat_default") {
        checks.push(("pref_chat_default", !one_of(cd, &CHAT_DEFAULT)));
    }
    if let Some(m) = get("pref_milestones") {
        checks.push(("pref_milestones", !is_bool(m)));
    }
    checks.push(("pref_post_style", !one_of(&v("pref_post_style"), &POST_STYLE)));
    checks.push((
        "pref_post_style_mobile",
        !one_of(&v("pref_post_style_mobile"), &POST_STYLE),
    ));
    checks.push(("pref_clear", !one_of(&v("pref_clear"), &CLEAR)));
    checks.push(("pref_strike_color", !hex_after_hash(&v("pref_strike_color"))?));
    checks.push((
        "pref_highlight_last_count_color",
        !hex_after_hash(&v("pref_highlight_last_count_color"))?,
    ));
    let font = v("pref_post_font_size");
    checks.push(("pref_post_font_size", !font.is_number()));
    checks.push(("pref_post_font_size", !is_integer(&font)));
    checks.push(("pref_post_font_size", !in_range(&font, 10.0, 32.0)));
    let failed: Vec<&str> = checks
        .into_iter()
        .filter(|(field, _)| get(field).is_some())
        .filter(|(_, bad)| *bad)
        .map(|(f, _)| f)
        .collect();
    if !failed.is_empty() {
        tracing::info!("validateUpdatePrefs failed: {failed:?}");
    }
    Ok(failed.is_empty())
}

static NAME_ALLOWED: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"[\p{L}\p{M}\s_0-9-]+").expect("valid regex"));

pub fn name_allowed(name: &str) -> bool {
    NAME_ALLOWED.replace_all(name, "").is_empty()
}

async fn owned_items(state: &AppState, uuid: &str) -> ApiResult<Vec<(String, String)>> {
    Ok(sqlx::query_as(
        "SELECT item.category, item.internal_name FROM useritem ui INNER JOIN item ON item.id = ui.itemId WHERE ui.userUuid = ?",
    )
    .bind(uuid)
    .fetch_all(&state.pool)
    .await?)
}

async fn validate_counter(
    state: &AppState,
    user: &SessionUser,
    c: &Value,
    real: Option<&Value>,
) -> ApiResult<Result<(), String>> {
    let items = owned_items(state, &user.uuid).await?;
    let owns = |cat: &str, v: &Value| {
        items
            .iter()
            .any(|(ic, name)| ic == cat && Some(name.as_str()) == v.as_str())
    };
    let (Some(real), Some(name), Some(username), Some(color)) =
        (real, c["name"].as_str(), c["username"].as_str(), c["color"].as_str())
    else {
        return Ok(Err("the server hit an error checking your profile".into()));
    };
    let banned = has_role(real, "banned");
    let muted = has_role(real, "muted");
    let title_bad = if !banned && !muted {
        !owns("title", &c["title"])
    } else if banned {
        c["title"] != "BANNED"
    } else {
        c["title"] != "MUTED"
    };
    let emoji = &c["emoji"];
    let emoji_bad = !emoji.is_null() && emoji != "" && !owns("emoji", emoji);
    let name_t = counting::js::trim(name);
    let user_t = counting::js::trim(username);
    let color_t = counting::js::trim(color);
    let rainbow = &c["rainbow"];
    let rn = rainbow.as_f64();
    let pronouns = c["pronouns"].as_array();
    let pronouns_bad = pronouns.is_none_or(|p| {
        p.len() != 4
            || !PRONOUNS
                .iter()
                .any(|set| set.iter().zip(p.iter()).all(|(a, b)| b.as_str() == Some(*a)))
    });
    let checks = [
        (!owns("card", &c["cardStyle"]), "card style not owned"),
        (!owns("border", &c["cardBorderStyle"]), "card border not owned"),
        (title_bad, "title not owned"),
        (emoji_bad, "emoji not owned"),
        (name_t.encode_utf16().count() < 1, "name too short"),
        (name_t.encode_utf16().count() > 25, "name too long"),
        (user_t.encode_utf16().count() < 1, "username too short"),
        (user_t.encode_utf16().count() > 25, "username too long"),
        (
            !NAME_ALLOWED.replace_all(name, "").is_empty(),
            "name has disallowed characters",
        ),
        (
            username
                .chars()
                .any(|ch| !(ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')),
            "username has disallowed characters",
        ),
        (color_t.encode_utf16().count() != 7, "color is not 7 characters"),
        (!is_integer(rainbow), "rainbow is not an integer"),
        (
            rn.is_some_and(|r| r < 1.0) || (rn.is_none() && loose(rainbow) < 1.0),
            "rainbow below 1",
        ),
        (
            rn.is_some_and(|r| r > 100_000.0) || (rn.is_none() && loose(rainbow) > 100_000.0),
            "rainbow above 100000",
        ),
        (pronouns.is_none(), "pronouns not an array"),
        (pronouns_bad, "pronouns not a known set"),
        (!color_t.starts_with('#'), "color missing leading #"),
        (!is_hex6(color_t.get(1..).unwrap_or("")), "color is not valid hex"),
    ];
    let failed: Vec<&str> = checks.iter().filter(|(b, _)| *b).map(|(_, n)| *n).collect();
    if failed.is_empty() {
        return Ok(Ok(()));
    }
    tracing::info!("Prefs rejected for {username}: {}", failed.join("; "));
    Ok(Err(failed.join("; ")))
}

async fn update_prefs(State(state): State<AppState>, Authed(user): Authed, body: JsonBody) -> ApiResult<Reply> {
    let prefs = body.get("prefsUpdate");
    let counter_update = body.get("counterUpdate");
    if !truthy(prefs) || !truthy(counter_update) {
        return Err(ApiError::bare(StatusCode::BAD_REQUEST));
    }
    let real = db::find_counter(&state, &user.uuid).await?;
    let prefs_map = prefs.as_object().cloned().unwrap_or_default();
    let valid =
        validate_prefs(&prefs_map).map_err(|_| ApiError::Internal(anyhow::anyhow!("prefs validation threw")))?;
    let player = validate_counter(&state, &user, counter_update, real.as_ref()).await?;
    if !(valid && player.is_ok()) {
        let reason = match player {
            Err(r) => r,
            Ok(()) if !valid => "one of your preference values is out of range".into(),
            Ok(()) => "unknown".into(),
        };
        return Err(ApiError::bad_request(format!("Couldn't save preferences: {reason}.")));
    }
    let real = real.unwrap_or(Value::Null);
    let mut cu = counter_update.clone();
    if has_role(&real, "banned") {
        cu["title"] = json!("BANNED");
    } else if has_role(&real, "muted") {
        cu["title"] = json!("MUTED");
    }
    if real["name"] != cu["name"] {
        let message = format!(
            "Counter {} ({}) just changed their name from {} to {}",
            crate::http::js_string(&real["username"]),
            crate::http::js_string(&real["uuid"]),
            crate::http::js_string(&real["name"]),
            crate::http::js_string(&cu["name"]),
        );
        tracing::info!("{message}");
        state.bot.emit("logCustomMessage", json!({ "message": message }));
    }
    let mut sets = Vec::new();
    let mut args = Vec::new();
    for k in PREF_KEYS {
        if let Some(v) = prefs_map.get(k) {
            sets.push(format!("`{k}` = ?"));
            let v = if k == "pref_strike_color" {
                v.as_str().map_or(v.clone(), |s| json!(counting::js::trim(s)))
            } else {
                v.clone()
            };
            args.push(v);
        }
    }
    if !sets.is_empty() {
        args.push(json!(user.uuid));
        db::exec(
            &state,
            &format!("UPDATE users SET {} WHERE uuid = ?", sets.join(", ")),
            &args,
        )
        .await?;
    }
    let emoji = if truthy(&cu["emoji"]) {
        cu["emoji"].clone()
    } else {
        Value::Null
    };
    db::exec(
        &state,
        "UPDATE counters SET name = ?, color = ?, cardStyle = ?, cardBorderStyle = ?, title = ?, emoji = ?, rainbow = ?, pronouns = ? WHERE uuid = ?",
        &[
            cu["name"].clone(),
            cu["color"].clone(),
            cu["cardStyle"].clone(),
            cu["cardBorderStyle"].clone(),
            cu["title"].clone(),
            emoji,
            cu["rainbow"].clone(),
            json!(cu["pronouns"].to_string()),
            json!(user.uuid),
        ],
    )
    .await?;
    Ok(empty())
}

pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

const THREAD_PREF_COLUMNS: [&str; 26] = [
    "enabled",
    "pref_online",
    "pref_discord_pings",
    "pref_load_from_bottom",
    "pref_strike_color",
    "pref_submit_shortcut",
    "pref_clear",
    "pref_nightMode",
    "pref_standardize_format",
    "pref_time_since_last_count",
    "pref_custom_stricken",
    "pref_post_style",
    "pref_post_style_mobile",
    "pref_reply_time_interval",
    "pref_night_mode_colors",
    "pref_post_position",
    "pref_hide_stricken",
    "pref_highlight_last_count",
    "pref_highlight_last_count_color",
    "pref_sound_on_stricken",
    "pref_hide_thread_picker",
    "pref_stricken_count_opacity",
    "pref_timestamp_display",
    "pref_pace_mode",
    "pref_pace_counts",
    "pref_show_latency",
];

async fn thread_pref_id(state: &AppState, user: &str, thread: &str) -> ApiResult<Option<i32>> {
    let r: Option<(i32,)> =
        sqlx::query_as("SELECT id FROM thread_preferences WHERE userUuid = ? AND threadUuid = ? LIMIT 1")
            .bind(user)
            .bind(thread)
            .fetch_optional(&state.pool)
            .await?;
    Ok(r.map(|r| r.0))
}

async fn upsert_thread_pref(state: &AppState, user: &str, thread: &str, values: &[(&str, Value)]) -> ApiResult<()> {
    let existing = thread_pref_id(state, user, thread).await?;
    let mut args: Vec<Value> = values.iter().map(|(_, v)| v.clone()).collect();
    match existing {
        Some(id) => {
            if values.is_empty() {
                return Ok(());
            }
            let sets: Vec<String> = values.iter().map(|(k, _)| format!("`{k}` = ?")).collect();
            args.push(json!(id));
            db::exec(
                state,
                &format!("UPDATE thread_preferences SET {} WHERE id = ?", sets.join(", ")),
                &args,
            )
            .await?;
        }
        None => {
            let mut cols: Vec<String> = values.iter().map(|(k, _)| format!("`{k}`")).collect();
            cols.extend(["userUuid".into(), "threadUuid".into()]);
            args.extend([json!(user), json!(thread)]);
            db::exec(
                state,
                &format!(
                    "INSERT INTO thread_preferences ({}) VALUES ({})",
                    cols.join(", "),
                    db::placeholders(cols.len())
                ),
                &args,
            )
            .await?;
        }
    }
    Ok(())
}

async fn update_thread_prefs(State(state): State<AppState>, Authed(user): Authed, body: JsonBody) -> ApiResult<Reply> {
    let prefs = body.get("prefsUpdate");
    if !truthy(prefs) {
        return Err(ApiError::bare(StatusCode::BAD_REQUEST));
    }
    let Some(tuuid) = prefs["thread"]["uuid"].as_str().filter(|u| !u.is_empty()) else {
        return Err(ApiError::bad_request("prefsUpdate.thread.uuid is required."));
    };
    let Some(thread) = db::find_thread_by_uuid(&state, tuuid).await? else {
        return Err(ApiError::bare(StatusCode::NOT_FOUND));
    };
    let map = prefs.as_object().cloned().unwrap_or_default();
    let valid = validate_prefs(&map).map_err(|_| ApiError::Internal(anyhow::anyhow!("prefs validation threw")))?;
    if !valid {
        return Err(ApiError::bad_request(
            "Thread prefs invalid: one of the values is out of range.",
        ));
    }
    let values: Vec<(&str, Value)> = THREAD_PREF_COLUMNS
        .iter()
        .filter_map(|k| map.get(*k).map(|v| (*k, v.clone())))
        .collect();
    let thread_uuid = thread["uuid"].as_str().unwrap_or(tuuid);
    upsert_thread_pref(&state, &user.uuid, thread_uuid, &values).await?;
    Ok(empty())
}

async fn update_thread_pace_prefs(
    State(state): State<AppState>,
    Authed(user): Authed,
    body: JsonBody,
) -> ApiResult<Reply> {
    let Some(thread) = db::find_thread_by_uuid_value(&state, body.get("threadUUID")).await? else {
        return Err(ApiError::bare(StatusCode::NOT_FOUND));
    };
    let mut values: Vec<(&str, Value)> = Vec::new();
    let mode = body.get("pref_pace_mode");
    if !mode.is_null() {
        if !one_of(mode, &PACE_MODE) {
            return Err(ApiError::bad_request("pref_pace_mode invalid option"));
        }
        values.push(("pref_pace_mode", mode.clone()));
    }
    let counts = body.get("pref_pace_counts");
    if !counts.is_null() {
        let n = loose(counts).floor();
        if !n.is_finite() || !(1.0..=1_000_000.0).contains(&n) {
            return Err(ApiError::bad_request("pref_pace_counts out of range"));
        }
        values.push(("pref_pace_counts", json!(n as i64)));
    }
    let target = body.get("pref_pace_target_ms");
    if !target.is_null() {
        let n = loose(target).floor();
        if !n.is_finite() || !(0.0..=86_400_000.0).contains(&n) {
            return Err(ApiError::bad_request("pref_pace_target_ms out of range"));
        }
        values.push(("pref_pace_target_ms", json!(n as i64)));
    }
    let tuuid = thread["uuid"].as_str().unwrap_or_default();
    upsert_thread_pref(&state, &user.uuid, tuuid, &values).await?;
    Ok(reply(json!({ "saved": true })))
}

async fn delete_thread_prefs(State(state): State<AppState>, Authed(user): Authed, q: Query) -> ApiResult<Reply> {
    let Some(tuuid) = q.str("threadUUID").filter(|t| !t.is_empty()) else {
        return Err(ApiError::bare(StatusCode::BAD_REQUEST));
    };
    let Some(thread) = db::find_thread_by_uuid(&state, tuuid).await? else {
        return Err(ApiError::bare(StatusCode::NOT_FOUND));
    };
    sqlx::query("DELETE FROM thread_preferences WHERE userUuid = ? AND threadUuid = ? LIMIT 1")
        .bind(&user.uuid)
        .bind(thread["uuid"].as_str())
        .execute(&state.pool)
        .await?;
    Ok(empty())
}

const PREF_STAT_COLUMNS: [&str; 18] = [
    "pref_clear",
    "pref_submit_shortcut",
    "pref_post_style",
    "pref_post_style_mobile",
    "pref_custom_stricken",
    "pref_hide_stricken",
    "pref_sound_on_stricken",
    "pref_night_mode_colors",
    "pref_nightMode",
    "pref_standardize_format",
    "pref_post_position",
    "pref_online",
    "pref_discord_pings",
    "pref_load_from_bottom",
    "pref_time_since_last_count",
    "pref_highlight_last_count",
    "pref_hide_thread_picker",
    "pref_show_latency",
];

pub async fn compute_pref_stats(state: &AppState) -> ApiResult<Value> {
    let min_xp = LEVELS.iter().find(|l| l.level == "20").map_or(54321, |l| l.min_xp);
    let cols: Vec<String> = PREF_STAT_COLUMNS.iter().map(|c| format!("u.`{c}` AS `{c}`")).collect();
    let rows = sqlx::query(&format!(
        "SELECT {} FROM users u INNER JOIN counters c ON c.uuid = u.uuid WHERE c.xp >= ?",
        cols.join(", ")
    ))
    .bind(min_xp)
    .fetch_all(&state.pool)
    .await?;
    let mut stats = Map::new();
    for c in PREF_STAT_COLUMNS {
        stats.insert(c.into(), json!({}));
    }
    let total = rows.len();
    if total == 0 {
        return Ok(Value::Object(stats));
    }
    let mut counts: Vec<Vec<(String, usize)>> = vec![Vec::new(); PREF_STAT_COLUMNS.len()];
    for r in &rows {
        let raw = entity::row_json(r);
        for (i, c) in PREF_STAT_COLUMNS.iter().enumerate() {
            let v = &raw[*c];
            if v.is_null() || v == "" {
                continue;
            }
            let key = crate::http::js_string(v);
            match counts[i].iter_mut().find(|(k, _)| *k == key) {
                Some(slot) => slot.1 += 1,
                None => counts[i].push((key, 1)),
            }
        }
    }
    for (i, c) in PREF_STAT_COLUMNS.iter().enumerate() {
        let m: Map<String, Value> = counts[i]
            .iter()
            .map(|(k, n)| {
                let pct = ((*n as f64 / total as f64) * 1000.0).round() / 10.0;
                (k.clone(), entity::js_number(pct))
            })
            .collect();
        stats.insert((*c).into(), Value::Object(m));
    }
    Ok(Value::Object(stats))
}

async fn pref_stats(State(state): State<AppState>) -> ApiResult<Reply> {
    let v = state
        .caches
        .pref_stats
        .lock()
        .unwrap()
        .clone()
        .unwrap_or_else(|| json!({}));
    Ok(reply(v))
}

async fn shop(State(state): State<AppState>, Authed(_user): Authed) -> ApiResult<Reply> {
    let rows = sqlx::query("SELECT * FROM item WHERE unlockMethod = 'shop'")
        .fetch_all(&state.pool)
        .await?;
    Ok(reply(entity::ITEM.rows(&rows)))
}
