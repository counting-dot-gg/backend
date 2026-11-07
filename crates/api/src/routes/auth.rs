use crate::db;
use crate::entity;
use crate::error::{ApiError, ApiResult};
use crate::http::{Query, Reply, empty, reply};
use crate::session::{Authed, MaybeUser, Session, SessionUser};
use crate::state::AppState;
use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::Response;
use axum::routing::{get, post};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};
use sqlx::Row;

pub const SITE_VERSION: &str = "v1.3.8";
const DISCORD_AUTHORIZE: &str = "https://discord.com/api/oauth2/authorize";
const DISCORD_TOKEN: &str = "https://discord.com/api/oauth2/token";
const DISCORD_ME: &str = "https://discord.com/api/users/@me";

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/auth/login", get(login))
        .route("/auth/redirect", get(redirect))
        .route("/auth/login_localhost", get(login_localhost))
        .route("/auth/redirect_localhost", get(redirect_localhost))
        .route("/auth/login_dynamic", get(login_dynamic))
        .route("/auth/redirect_dynamic", get(redirect_dynamic))
        .route("/auth/reddit", get(reddit_callback))
        .route("/auth/reddit_login", get(reddit_login))
        .route("/auth/reddit_token", get(reddit_token))
        .route("/auth/reddit_logout", get(reddit_logout))
        .route("/auth/status", get(status))
        .route("/auth/logout", post(logout))
}

fn found(location: &str) -> Response {
    let mut r = Response::new(Body::from(format!("Found. Redirecting to {location}")));
    *r.status_mut() = StatusCode::FOUND;
    if let Ok(v) = HeaderValue::from_str(location) {
        r.headers_mut().insert(header::LOCATION, v);
    }
    r.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    r
}

fn passport_redirect(location: &str) -> Response {
    let mut r = Response::new(Body::empty());
    *r.status_mut() = StatusCode::FOUND;
    if let Ok(v) = HeaderValue::from_str(location) {
        r.headers_mut().insert(header::LOCATION, v);
    }
    r.headers_mut()
        .insert(header::CONTENT_LENGTH, HeaderValue::from_static("0"));
    r
}

fn enc(s: &str) -> String {
    urlencoding::encode(s).into_owned()
}

fn authorize_url(state: &AppState, callback: &str) -> String {
    format!(
        "{DISCORD_AUTHORIZE}?response_type=code&redirect_uri={}&scope=identify&client_id={}",
        enc(callback),
        enc(&state.cfg.discord.client_id)
    )
}

fn dynamic_callback(headers: &HeaderMap) -> (String, String) {
    let h = |k: &str| headers.get(k).and_then(|v| v.to_str().ok()).map(str::to_owned);
    let proto = h("x-forwarded-proto").unwrap_or_else(|| "http".into());
    let host = h("x-forwarded-host")
        .or_else(|| h("host"))
        .unwrap_or_else(|| "localhost:3001".into());
    (proto, host)
}

const LOGIN_STATE_COOKIE: &str = "cgg_login_state";

fn start_login(state: &AppState, callback: &str) -> Response {
    let nonce = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(crate::session::secure_bytes::<16>());
    let mut r = passport_redirect(&format!("{}&state={nonce}", authorize_url(state, callback)));
    if let Ok(v) = HeaderValue::from_str(&login_state_cookie(state, &nonce)) {
        r.headers_mut().append(header::SET_COOKIE, v);
    }
    r
}

fn login_state_cookie(state: &AppState, nonce: &str) -> String {
    let mut out = format!("{LOGIN_STATE_COOKIE}={nonce}; Path=/; Max-Age=600; HttpOnly; SameSite=Lax");
    if let Some(d) = &state.cfg.cookie_domain {
        out.push_str(&format!("; Domain={d}"));
    }
    if !state.cfg.insecure_cookies {
        out.push_str("; Secure");
    }
    out
}

fn login_state_matches(headers: &HeaderMap, presented: Option<&str>) -> bool {
    let Some(presented) = presented.filter(|p| !p.is_empty()) else {
        return false;
    };
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|p| p.trim().strip_prefix(LOGIN_STATE_COOKIE)?.strip_prefix('='))
        .any(|v| v == presented)
}

async fn login(State(state): State<AppState>) -> Response {
    start_login(&state, &state.cfg.discord.redirect_url)
}

async fn login_localhost(State(state): State<AppState>) -> Response {
    start_login(&state, &state.cfg.discord.localhost_redirect_url)
}

async fn login_dynamic(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let (proto, host) = dynamic_callback(&headers);
    start_login(&state, &format!("{proto}://{host}/api/auth/redirect_dynamic"))
}

struct DiscordProfile {
    id: String,
    username: String,
    discriminator: String,
    avatar: Option<String>,
    access: String,
    refresh: String,
}

async fn exchange(state: &AppState, code: &str, callback: &str) -> ApiResult<DiscordProfile> {
    let form = [
        ("grant_type", "authorization_code"),
        ("redirect_uri", callback),
        ("client_id", state.cfg.discord.client_id.as_str()),
        ("client_secret", state.cfg.discord.client_secret.as_str()),
        ("code", code),
    ];
    let body = form
        .iter()
        .map(|(k, v)| format!("{k}={}", enc(v)))
        .collect::<Vec<_>>()
        .join("&");
    let tok: Value = state
        .http
        .post(DISCORD_TOKEN)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .map_err(|e| ApiError::Internal(e.into()))?
        .json()
        .await
        .map_err(|e| ApiError::Internal(e.into()))?;
    let Some(access) = tok["access_token"].as_str() else {
        return Err(ApiError::bare(StatusCode::UNAUTHORIZED));
    };
    let me: Value = state
        .http
        .get(DISCORD_ME)
        .bearer_auth(access)
        .send()
        .await
        .map_err(|e| ApiError::Internal(e.into()))?
        .json()
        .await
        .map_err(|e| ApiError::Internal(e.into()))?;
    let Some(id) = me["id"].as_str() else {
        return Err(ApiError::Internal(anyhow::anyhow!("failed to fetch user profile")));
    };
    Ok(DiscordProfile {
        id: id.to_owned(),
        username: me["username"].as_str().unwrap_or_default().to_owned(),
        discriminator: me["discriminator"].as_str().unwrap_or_default().to_owned(),
        avatar: me["avatar"].as_str().map(str::to_owned),
        access: access.to_owned(),
        refresh: tok["refresh_token"].as_str().unwrap_or_default().to_owned(),
    })
}

async fn create_user(state: &AppState, p: &DiscordProfile) -> ApiResult<()> {
    let ns = state.clock.now_ns();
    let uuid = cgg_web::ids::uuid_for(ns);
    sqlx::query(
        "INSERT INTO users (uuid, discord_id, access_token, refresh_token, username, discriminator, pref_online, pref_discord_pings, \
         pref_load_from_bottom, pref_time_since_last_count, pref_custom_stricken, pref_strike_color, pref_submit_shortcut, pref_clear, \
         pref_standardize_format, pref_nightMode, pref_post_style, pref_post_style_mobile, pref_reply_time_interval, pref_night_mode_colors, \
         pref_post_position, pref_hide_stricken, money, timeOnline, inventory, reddit, redditAccess, redditRefresh) \
         VALUES (?, ?, ?, ?, ?, ?, 1, 1, 0, 0, 'Disabled', '#cccccc', 'Enter', 'Custom', 'Disabled', 'System', 'Default', 'Default', 100, \
         'Default', 'Left', 'Disabled', 2000, '0', '[]', '', '', '')",
    )
    .bind(&uuid)
    .bind(&p.id)
    .bind(&p.access)
    .bind(&p.refresh)
    .bind(&p.username)
    .bind(&p.discriminator)
    .execute(&state.pool)
    .await?;
    sqlx::query(
        "INSERT INTO useritem (unlocked, userUuid, itemId) SELECT 1, ?, id FROM item WHERE unlockMethod = 'default'",
    )
    .bind(&uuid)
    .execute(&state.pool)
    .await?;
    Ok(())
}

async fn validate_counter(state: &AppState, user: &Value, avatar: Option<&str>, discord_id: &str) -> ApiResult<()> {
    let avatar = avatar.filter(|a| !a.is_empty()).unwrap_or("0");
    let uuid = user["uuid"].as_str().unwrap_or_default();
    if db::find_counter(state, uuid).await?.is_some() {
        sqlx::query("UPDATE counters SET avatar = ? WHERE uuid = ?")
            .bind(avatar)
            .bind(uuid)
            .execute(&state.pool)
            .await?;
        return Ok(());
    }
    sqlx::query(
        "INSERT INTO counters (id, uuid, name, username, color, discordId, roles, avatar, cardStyle, cardBorderStyle, pronouns, title, xp) \
         VALUES (?, ?, '', '', '', ?, '[\"unverified\"]', ?, 'default', 'no_border_square', '[\"\",\"\",\"\",\"\"]', 'COUNTER', 0)",
    )
    .bind(user["id"].as_i64())
    .bind(uuid)
    .bind(discord_id)
    .bind(avatar)
    .execute(&state.pool)
    .await?;
    crate::rank::assign_new_counter(state, uuid).await
}

async fn complete_login(
    state: &AppState,
    session: &Session,
    q: &Query,
    headers: &HeaderMap,
    callback: &str,
) -> ApiResult<Option<Response>> {
    let Some(code) = q.str("code").filter(|c| !c.is_empty()) else {
        if q.str("error").is_some() {
            return Err(ApiError::bare(StatusCode::UNAUTHORIZED));
        }
        return Ok(Some(start_login(state, callback)));
    };
    if q.str("state").is_none_or(str::is_empty) {
        return Ok(Some(start_login(state, callback)));
    }
    if !login_state_matches(headers, q.str("state")) {
        return Ok(Some(found(&format!(
            "{}/",
            state.cfg.host_url.as_deref().unwrap_or("")
        ))));
    }
    let p = exchange(state, code, callback).await?;
    match session::find_by_discord(state, &p.id).await? {
        Some(_) => {
            sqlx::query("UPDATE users SET username = ?, discriminator = ?, access_token = ?, refresh_token = ? WHERE discord_id = ?")
                .bind(&p.username)
                .bind(&p.discriminator)
                .bind(&p.access)
                .bind(&p.refresh)
                .bind(&p.id)
                .execute(&state.pool)
                .await?;
        }
        None => create_user(state, &p).await?,
    }
    let Some(user) = session::find_by_discord(state, &p.id).await? else {
        return Err(ApiError::bare(StatusCode::UNAUTHORIZED));
    };
    validate_counter(state, &user, p.avatar.as_deref(), &p.id).await?;
    session.log_in(user);
    Ok(None)
}

mod session {
    use super::*;

    pub async fn find_by_discord(state: &AppState, discord_id: &str) -> ApiResult<Option<Value>> {
        Ok(crate::session::find_user_by_discord_id(state, discord_id).await)
    }
}

async fn redirect(State(state): State<AppState>, s: Session, q: Query, headers: HeaderMap) -> ApiResult<Response> {
    let callback = state.cfg.discord.redirect_url.clone();
    if let Some(r) = complete_login(&state, &s, &q, &headers, &callback).await? {
        return Ok(r);
    }
    Ok(found(&format!(
        "{}/register",
        state.cfg.host_url.as_deref().unwrap_or("")
    )))
}

async fn redirect_localhost(
    State(state): State<AppState>,
    s: Session,
    q: Query,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let callback = state.cfg.discord.localhost_redirect_url.clone();
    if let Some(r) = complete_login(&state, &s, &q, &headers, &callback).await? {
        return Ok(r);
    }
    Ok(found(&format!(
        "{}/register",
        state.cfg.host_url.as_deref().unwrap_or("")
    )))
}

async fn redirect_dynamic(
    State(state): State<AppState>,
    s: Session,
    q: Query,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let (proto, host) = dynamic_callback(&headers);
    let callback = format!("{proto}://{host}/api/auth/redirect_dynamic");
    if let Some(r) = complete_login(&state, &s, &q, &headers, &callback).await? {
        return Ok(r);
    }
    let frontend = host.strip_suffix(":3001").map_or(host.clone(), |h| format!("{h}:3000"));
    Ok(found(&format!("{proto}://{frontend}/register")))
}

fn not_logged_in() -> ApiError {
    ApiError::unauthorized("User not logged in")
}

fn reddit_key(state: &AppState) -> Option<[u8; 32]> {
    let secret = cgg_core::var("REDDIT_TOKEN_SECRET").unwrap_or_else(|| state.cfg.cookie_secret.clone());
    if secret.is_empty() {
        return None;
    }
    let mut key = [0u8; 32];
    let params = scrypt::Params::new(14, 8, 1, 32).ok()?;
    scrypt::scrypt(secret.as_bytes(), b"countgg.reddit.token.v1", &params, &mut key).ok()?;
    Some(key)
}

pub fn encrypt_token(state: &AppState, plain: Option<&str>) -> Option<String> {
    let plain = plain.filter(|p| !p.is_empty())?;
    let Some(key) = reddit_key(state) else {
        return Some(plain.to_owned());
    };
    let cipher = Aes256Gcm::new_from_slice(&key).ok()?;
    let iv: [u8; 12] = crate::session::secure_bytes();
    let mut sealed = cipher
        .encrypt(
            Nonce::from_slice(&iv),
            Payload {
                msg: plain.as_bytes(),
                aad: b"",
            },
        )
        .ok()?;
    let tag = sealed.split_off(sealed.len() - 16);
    Some(format!(
        "v1:{}:{}:{}",
        STANDARD.encode(iv),
        STANDARD.encode(tag),
        STANDARD.encode(sealed)
    ))
}

pub fn decrypt_token(state: &AppState, stored: Option<&str>) -> Option<String> {
    let stored = stored.filter(|s| !s.is_empty())?;
    let Some(rest) = stored.strip_prefix("v1:") else {
        return Some(stored.to_owned());
    };
    let key = reddit_key(state)?;
    let mut parts = rest.split(':');
    let iv = STANDARD.decode(parts.next()?).ok()?;
    let tag = STANDARD.decode(parts.next()?).ok()?;
    let mut ct = STANDARD.decode(parts.next()?).ok()?;
    if iv.len() != 12 {
        return None;
    }
    ct.extend_from_slice(&tag);
    let cipher = Aes256Gcm::new_from_slice(&key).ok()?;
    let plain = cipher.decrypt(Nonce::from_slice(&iv), ct.as_slice()).ok()?;
    String::from_utf8(plain).ok()
}

async fn reddit_post(state: &AppState, url: &str, params: &[(&str, &str)]) -> Option<Value> {
    let q = params
        .iter()
        .map(|(k, v)| format!("{k}={}", enc(v)))
        .collect::<Vec<_>>()
        .join("&");
    let r = state
        .http
        .post(format!("{url}?{q}"))
        .basic_auth(&state.cfg.reddit.client_id, Some(&state.cfg.reddit.client_secret))
        .header("user-agent", "counting.gg")
        .send()
        .await
        .ok()?;
    if !r.status().is_success() {
        return None;
    }
    r.json().await.ok()
}

async fn reddit_callback(State(state): State<AppState>, MaybeUser(user): MaybeUser, q: Query) -> ApiResult<Response> {
    let user = user.ok_or_else(not_logged_in)?;
    if q.str("state") != Some(reddit_state(&state, &user.uuid).as_str()) {
        return Err(ApiError::bare(StatusCode::FORBIDDEN));
    }
    let code = q.str("code").unwrap_or("");
    let data = reddit_post(
        &state,
        "https://www.reddit.com/api/v1/access_token",
        &[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", state.cfg.reddit.redirect_url.as_str()),
        ],
    )
    .await
    .ok_or_else(|| ApiError::Internal(anyhow::anyhow!("reddit token exchange failed")))?;
    let access = data["access_token"].as_str().unwrap_or_default().to_owned();
    let refresh = data["refresh_token"].as_str().map(str::to_owned);
    let me: Option<Value> = match state
        .http
        .get("https://oauth.reddit.com/api/v1/me")
        .bearer_auth(&access)
        .header("user-agent", "counting.gg")
        .send()
        .await
    {
        Ok(r) => r.json().await.ok(),
        Err(_) => None,
    };
    if let Some(name) = me.as_ref().and_then(|m| m["name"].as_str()) {
        sqlx::query("UPDATE users SET reddit = ?, redditAccess = ?, redditRefresh = ? WHERE uuid = ?")
            .bind(name)
            .bind(&access)
            .bind(encrypt_token(&state, refresh.as_deref()))
            .bind(&user.uuid)
            .execute(&state.pool)
            .await?;
    }
    Ok(found(&format!(
        "{}/r/livecounting",
        state.cfg.host_url.as_deref().unwrap_or("")
    )))
}

fn reddit_state(state: &AppState, uuid: &str) -> String {
    crate::session::hmac_tag(&state.cfg.cookie_secret, "reddit-link", uuid)
}

async fn reddit_login(State(state): State<AppState>, MaybeUser(user): MaybeUser) -> ApiResult<Response> {
    let user = user.ok_or_else(not_logged_in)?;
    let st = reddit_state(&state, &user.uuid);
    let url = format!(
        "https://www.reddit.com/api/v1/authorize?client_id={}&response_type=code&state={st}&redirect_uri={}&duration=permanent&scope=identity,read,submit",
        state.cfg.reddit.client_id, state.cfg.reddit.redirect_url
    );
    Ok(found(&url))
}

async fn reddit_token(State(state): State<AppState>, Authed(user): Authed) -> ApiResult<Reply> {
    let Some(stored) = decrypt_token(&state, user.str("redditRefresh")) else {
        return Err(ApiError::conflict("Reddit link has no refresh token; re-link."));
    };
    let refreshed = reddit_post(
        &state,
        "https://www.reddit.com/api/v1/access_token",
        &[("grant_type", "refresh_token"), ("refresh_token", &stored)],
    )
    .await
    .filter(|d| d["access_token"].is_string());
    let Some(data) = refreshed else {
        sqlx::query("UPDATE users SET redditAccess = NULL, redditRefresh = NULL WHERE uuid = ?")
            .bind(&user.uuid)
            .execute(&state.pool)
            .await?;
        return Err(ApiError::conflict("Reddit refused the refresh token; re-link."));
    };
    let access = data["access_token"].as_str().unwrap_or_default().to_owned();
    match data["refresh_token"].as_str().filter(|r| !r.is_empty()) {
        Some(r) => {
            sqlx::query("UPDATE users SET redditAccess = ?, redditRefresh = ? WHERE uuid = ?")
                .bind(&access)
                .bind(encrypt_token(&state, Some(r)))
                .bind(&user.uuid)
                .execute(&state.pool)
                .await?;
        }
        None => {
            sqlx::query("UPDATE users SET redditAccess = ? WHERE uuid = ?")
                .bind(&access)
                .bind(&user.uuid)
                .execute(&state.pool)
                .await?;
        }
    }
    Ok(reply(json!({ "redditAccess": access })))
}

async fn reddit_logout(State(state): State<AppState>, MaybeUser(user): MaybeUser) -> ApiResult<Response> {
    let user = user.ok_or_else(not_logged_in)?;
    let revoke = |token: Option<String>, hint: &'static str| {
        let s = state.clone();
        async move {
            if let Some(t) = token.filter(|t| !t.is_empty()) {
                reddit_post(
                    &s,
                    "https://www.reddit.com/api/v1/revoke_token",
                    &[("token", &t), ("token_type_hint", hint)],
                )
                .await;
            }
        }
    };
    revoke(decrypt_token(&state, user.str("redditRefresh")), "refresh_token").await;
    revoke(user.str("redditAccess").map(str::to_owned), "access_token").await;
    sqlx::query("UPDATE users SET reddit = NULL, redditAccess = NULL, redditRefresh = NULL WHERE uuid = ?")
        .bind(&user.uuid)
        .execute(&state.pool)
        .await?;
    Ok(found(&format!(
        "{}/r/livecounting",
        state.cfg.host_url.as_deref().unwrap_or("")
    )))
}

async fn full_user(state: &AppState, uuid: &str) -> ApiResult<Value> {
    let Some(mut user) = db::find_user(state, uuid).await? else {
        return Ok(Value::Null);
    };
    if let Some(o) = user.as_object_mut() {
        o.remove("accessToken");
        o.remove("refreshToken");
    }
    let items = sqlx::query("SELECT * FROM useritem WHERE userUuid = ?")
        .bind(uuid)
        .fetch_all(&state.pool)
        .await?;
    user["userItems"] = entity::USER_ITEM.rows(&items);
    let challenges = sqlx::query("SELECT * FROM challenge WHERE userUuid = ?")
        .bind(uuid)
        .fetch_all(&state.pool)
        .await?;
    user["userChallenges"] = entity::DAILY_CHALLENGE.rows(&challenges);
    let misc =
        sqlx::query("SELECT m.* FROM miscsettings m INNER JOIN users u ON u.miscSettingsId = m.id WHERE u.uuid = ?")
            .bind(uuid)
            .fetch_optional(&state.pool)
            .await?;
    user["miscSettings"] = misc.map_or(Value::Null, |r| entity::MISC_SETTINGS.json(&r));
    let prefs = sqlx::query(&format!(
        "SELECT {}, {} FROM thread_preferences tp LEFT JOIN thread t ON t.uuid = tp.threadUuid WHERE tp.userUuid = ?",
        entity::THREAD_PREFERENCES.select("tp"),
        entity::THREAD.select("t")
    ))
    .bind(uuid)
    .fetch_all(&state.pool)
    .await?;
    user["threadPreferences"] = Value::Array(
        prefs
            .iter()
            .map(|r| {
                let mut p = entity::THREAD_PREFERENCES.json_prefixed(r, "tp_");
                let has_thread = r.try_get::<Option<String>, _>("t_uuid").ok().flatten().is_some();
                p["thread"] = if has_thread {
                    entity::THREAD.json_prefixed(r, "t_")
                } else {
                    Value::Null
                };
                p
            })
            .collect(),
    );
    Ok(user)
}

async fn user_items(state: &AppState, uuid: &str) -> ApiResult<Value> {
    let rows =
        sqlx::query("SELECT item.* FROM useritem ui INNER JOIN item ON item.id = ui.itemId WHERE ui.userUuid = ?")
            .bind(uuid)
            .fetch_all(&state.pool)
            .await?;
    Ok(entity::ITEM.rows(&rows))
}

pub async fn misc_settings(state: &AppState, uuid: &str) -> ApiResult<Value> {
    let row = sqlx::query("SELECT * FROM miscsettings WHERE userUuid = ? LIMIT 1")
        .bind(uuid)
        .fetch_optional(&state.pool)
        .await?;
    if let Some(r) = row {
        return Ok(entity::MISC_SETTINGS.json(&r));
    }
    let now = cgg_web::now_ms();
    let res = sqlx::query("INSERT INTO miscsettings (categories, lastUpdated, userUuid) VALUES ('[]', ?, ?)")
        .bind(now)
        .bind(uuid)
        .execute(&state.pool)
        .await?;
    Ok(json!({ "categories": [], "lastUpdated": now, "id": res.last_insert_id() }))
}

async fn running_bingo_game(state: &AppState, uuid: &str) -> Value {
    sqlx::query_scalar::<_, i32>(
        "SELECT m.gameId FROM bingogamemember m INNER JOIN bingogame g ON g.id = m.gameId \
         WHERE m.userUuid = ? AND m.status = 'active' AND g.status = 'running' LIMIT 1",
    )
    .bind(uuid)
    .fetch_optional(&state.pool)
    .await
    .ok()
    .flatten()
    .map_or(Value::Null, Value::from)
}

pub async fn counter_role_total(state: &AppState) -> ApiResult<i64> {
    let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM counters WHERE roles LIKE '%counter%'")
        .fetch_one(&state.pool)
        .await?;
    Ok(n)
}

async fn status(State(state): State<AppState>, MaybeUser(user): MaybeUser) -> ApiResult<Reply> {
    let total = state
        .caches
        .counter_role_total
        .load(std::sync::atomic::Ordering::Relaxed);
    let Some(user) = user else {
        return Ok(reply(
            json!({ "site_version": SITE_VERSION, "totalCounters": total, "bingoGameId": null }),
        ));
    };
    let SessionUser { uuid, .. } = &user;
    let (full, counter, items, misc, unread, bingo) = tokio::join!(
        full_user(&state, uuid),
        db::find_counter(&state, uuid),
        user_items(&state, uuid),
        misc_settings(&state, uuid),
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM mentions WHERE counterUuid = ? AND readStatus = 0")
            .bind(uuid)
            .fetch_one(&state.pool),
        running_bingo_game(&state, uuid),
    );
    let counter = counter?;
    let mut out = json!({
        "user": full?,
        "counter": counter.clone().unwrap_or(Value::Null),
        "items": items?,
        "miscSettings": misc?,
        "site_version": SITE_VERSION,
        "totalCounters": total,
        "unreadMentionCount": unread?,
        "bingoGameId": bingo,
    });
    if counter.is_some() {
        let ranks = sqlx::query("SELECT * FROM threadrank WHERE counterUuid = ?")
            .bind(uuid)
            .fetch_all(&state.pool)
            .await?;
        out["threadRanks"] = entity::THREAD_RANK.rows(&ranks);
    }
    if out["user"].is_null() {
        out.as_object_mut().map(|o| o.remove("user"));
    }
    if out["counter"].is_null() {
        out.as_object_mut().map(|o| o.remove("counter"));
    }
    Ok(reply(out))
}

async fn logout(s: Session) -> ApiResult<Reply> {
    if s.user().is_some() || s.data().is_some() {
        s.log_out();
    }
    Ok(empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_state_must_match_the_cookie() {
        let mut h = HeaderMap::new();
        h.insert(
            header::COOKIE,
            HeaderValue::from_static("a=1; cgg_login_state=abc; connect.sid=x"),
        );
        assert!(login_state_matches(&h, Some("abc")));
        assert!(!login_state_matches(&h, Some("abd")));
        assert!(!login_state_matches(&h, Some("")));
        assert!(!login_state_matches(&h, None));
        assert!(!login_state_matches(&HeaderMap::new(), Some("abc")));
    }

    #[test]
    fn authorize_url_matches_passport_order() {
        let url = format!(
            "{DISCORD_AUTHORIZE}?response_type=code&redirect_uri={}&scope=identify&client_id={}",
            enc("http://x/api/auth/redirect"),
            enc("123")
        );
        assert_eq!(
            url,
            "https://discord.com/api/oauth2/authorize?response_type=code&redirect_uri=http%3A%2F%2Fx%2Fapi%2Fauth%2Fredirect&scope=identify&client_id=123"
        );
    }
}
