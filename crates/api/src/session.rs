use crate::entity;
use crate::error::ApiError;
use crate::state::AppState;
use axum::extract::{FromRequestParts, Request, State};
use axum::http::header::{COOKIE, SET_COOKIE};
use axum::http::request::Parts;
use axum::http::{HeaderValue, Method};
use axum::middleware::Next;
use axum::response::Response;
use base64::Engine;
use base64::engine::general_purpose::{STANDARD_NO_PAD, URL_SAFE_NO_PAD};
use cgg_web::now_ms;
use cgg_web::session::{connect_sids, unsign};
use chrono::{TimeZone, Utc};
use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::Sha256;
use std::sync::{Arc, Mutex};

pub const MAX_AGE_MS: i64 = 60_000 * 60 * 24 * 180;

#[derive(Debug, Clone)]
pub struct SessionUser {
    pub uuid: String,
    pub discord_id: String,
    pub json: Value,
}

impl SessionUser {
    pub fn from_row(json: Value) -> Option<SessionUser> {
        Some(SessionUser {
            uuid: json["uuid"].as_str()?.to_owned(),
            discord_id: json["discordId"].as_str()?.to_owned(),
            json,
        })
    }

    pub fn str(&self, key: &str) -> Option<&str> {
        self.json[key].as_str()
    }
}

#[derive(Debug, Default)]
pub struct SessionState {
    pub sid: Option<String>,
    pub data: Option<Value>,
    pub user: Option<SessionUser>,
    pub change: Option<Change>,
}

#[derive(Debug, Clone)]
pub enum Change {
    LogIn(Value),
    LogOut,
}

#[derive(Clone, Default)]
pub struct Session(pub Arc<Mutex<SessionState>>);

impl Session {
    pub fn user(&self) -> Option<SessionUser> {
        self.0.lock().unwrap().user.clone()
    }

    pub fn data(&self) -> Option<Value> {
        self.0.lock().unwrap().data.clone()
    }

    pub fn log_in(&self, user: Value) {
        let mut s = self.0.lock().unwrap();
        s.user = SessionUser::from_row(user.clone());
        s.change = Some(Change::LogIn(user));
    }

    pub fn log_out(&self) {
        let mut s = self.0.lock().unwrap();
        s.user = None;
        s.change = Some(Change::LogOut);
    }
}

impl<S: Send + Sync> FromRequestParts<S> for Session {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        Ok(parts.extensions.get::<Session>().cloned().unwrap_or_default())
    }
}

pub struct Authed(pub SessionUser);

impl<S: Send + Sync> FromRequestParts<S> for Authed {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, s: &S) -> Result<Self, Self::Rejection> {
        let session = Session::from_request_parts(parts, s).await?;
        session.user().map(Authed).ok_or_else(ApiError::guard)
    }
}

pub struct MaybeUser(pub Option<SessionUser>);

impl<S: Send + Sync> FromRequestParts<S> for MaybeUser {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, s: &S) -> Result<Self, Self::Rejection> {
        Ok(MaybeUser(Session::from_request_parts(parts, s).await?.user()))
    }
}

pub fn sign(sid: &str, secret: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac accepts any key");
    mac.update(sid.as_bytes());
    format!("s:{sid}.{}", STANDARD_NO_PAD.encode(mac.finalize().into_bytes()))
}

pub fn secure_bytes<const N: usize>() -> [u8; N] {
    let mut bytes = [0u8; N];
    getrandom::fill(&mut bytes).expect("the OS random source is available");
    bytes
}

pub fn new_sid() -> String {
    URL_SAFE_NO_PAD.encode(secure_bytes::<24>())
}

pub fn hmac_tag(secret: &str, purpose: &str, value: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac accepts any key");
    mac.update(purpose.as_bytes());
    mac.update(b"\0");
    mac.update(value.as_bytes());
    URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
}

fn encode_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn utc_string(ms: i64) -> String {
    Utc.timestamp_millis_opt(ms)
        .single()
        .map(|t| t.format("%a, %d %b %Y %H:%M:%S GMT").to_string())
        .unwrap_or_default()
}

pub fn set_cookie_value(state: &AppState, sid: &str, expires_ms: i64) -> String {
    let cfg = &state.cfg;
    let mut out = format!("connect.sid={}", encode_component(&sign(sid, &cfg.cookie_secret)));
    if let Some(d) = &cfg.cookie_domain {
        out.push_str(&format!("; Domain={d}"));
    }
    out.push_str(&format!("; Path=/; Expires={}; HttpOnly", utc_string(expires_ms)));
    if cfg.insecure_cookies {
        out.push_str("; SameSite=Lax");
    } else {
        out.push_str("; Secure; SameSite=None");
    }
    out
}

pub fn cookie_json(state: &AppState, expires_ms: i64) -> Value {
    let cfg = &state.cfg;
    let mut c = json!({
        "originalMaxAge": MAX_AGE_MS,
        "expires": entity::js_iso(Utc.timestamp_millis_opt(expires_ms).single().unwrap_or_default()),
        "secure": !cfg.insecure_cookies,
        "httpOnly": true,
    });
    if let Some(d) = &cfg.cookie_domain {
        c["domain"] = json!(d);
    }
    c["path"] = json!("/");
    c["sameSite"] = json!(if cfg.insecure_cookies { "lax" } else { "none" });
    c
}

async fn load(state: &AppState, sid: &str) -> Option<Value> {
    let row: Option<(String,)> = sqlx::query_as("SELECT json FROM session WHERE expiredAt > ? AND id = ?")
        .bind(now_ms())
        .bind(sid)
        .fetch_optional(&state.pool)
        .await
        .map_err(|e| tracing::warn!("session lookup failed: {e}"))
        .ok()
        .flatten();
    serde_json::from_str(&row?.0).ok()
}

pub async fn find_user_by_discord_id(state: &AppState, discord_id: &str) -> Option<Value> {
    let row = sqlx::query("SELECT * FROM users WHERE discord_id = ? LIMIT 1")
        .bind(discord_id)
        .fetch_optional(&state.pool)
        .await
        .map_err(|e| tracing::warn!("session user lookup failed: {e}"))
        .ok()
        .flatten()?;
    Some(entity::USER.json(&row))
}

pub async fn layer(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    let header = req
        .headers()
        .get_all(COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .collect::<Vec<_>>()
        .join("; ");
    let presented = connect_sids(&header);
    let mut st = SessionState::default();
    for raw in &presented {
        let Some(sid) = unsign(raw, &state.cfg.cookie_secret) else {
            continue;
        };
        if let Some(data) = load(&state, &sid).await {
            st.sid = Some(sid);
            st.data = Some(data);
            break;
        }
        if presented.len() == 1 {
            break;
        }
    }
    if let Some(discord_id) = st
        .data
        .as_ref()
        .and_then(|d| d["passport"]["user"]["discordId"].as_str())
    {
        st.user = find_user_by_discord_id(&state, discord_id)
            .await
            .and_then(SessionUser::from_row);
    }
    let had_session = st.sid.is_some();
    let session = Session(Arc::new(Mutex::new(st)));
    req.extensions_mut().insert(session.clone());
    let is_options = req.method() == Method::OPTIONS;

    let mut res = next.run(req).await;

    if state.cfg.cookie_domain.is_some() && presented.len() > 1 {
        res.headers_mut().append(
            SET_COOKIE,
            HeaderValue::from_static(
                "connect.sid=; Path=/; Expires=Thu, 01 Jan 1970 00:00:00 GMT; HttpOnly; Secure; SameSite=None",
            ),
        );
    }
    if is_options {
        return res;
    }
    let (sid, change) = {
        let s = session.0.lock().unwrap();
        (s.sid.clone(), s.change.clone())
    };
    let expires = now_ms() + MAX_AGE_MS;
    match change {
        Some(Change::LogOut) => {
            if let Some(sid) = sid {
                let _ = sqlx::query("DELETE FROM session WHERE id = ?")
                    .bind(&sid)
                    .execute(&state.pool)
                    .await;
            }
        }
        Some(Change::LogIn(user)) => {
            if let Some(old) = &sid {
                let _ = sqlx::query("DELETE FROM session WHERE id = ?")
                    .bind(old)
                    .execute(&state.pool)
                    .await;
            }
            let sid = new_sid();
            let data = json!({ "cookie": cookie_json(&state, expires), "passport": { "user": user } });
            if save(&state, &sid, &data, expires).await {
                append_cookie(&state, &mut res, &sid, expires);
            }
        }
        None if had_session => {
            let sid = sid.unwrap_or_default();
            append_cookie(&state, &mut res, &sid, expires);
            let pool = state.pool.clone();
            tokio::spawn(async move {
                let _ = sqlx::query("UPDATE session SET expiredAt = ? WHERE id = ?")
                    .bind(expires)
                    .bind(&sid)
                    .execute(&pool)
                    .await;
            });
        }
        None => {}
    }
    res
}

fn append_cookie(state: &AppState, res: &mut Response, sid: &str, expires: i64) {
    if let Ok(v) = HeaderValue::from_str(&set_cookie_value(state, sid, expires)) {
        res.headers_mut().append(SET_COOKIE, v);
    }
}

async fn save(state: &AppState, sid: &str, data: &Value, expires: i64) -> bool {
    sqlx::query(
        "INSERT INTO session (id, expiredAt, json) VALUES (?, ?, ?) \
         ON DUPLICATE KEY UPDATE expiredAt = VALUES(expiredAt), json = VALUES(json)",
    )
    .bind(sid)
    .bind(expires)
    .bind(data.to_string())
    .execute(&state.pool)
    .await
    .map_err(|e| tracing::error!("session save failed: {e}"))
    .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_ids_are_long_and_never_repeat() {
        let ids: std::collections::HashSet<String> = (0..1000).map(|_| new_sid()).collect();
        assert_eq!(ids.len(), 1000);
        assert!(ids.iter().all(|id| id.len() == 32));
    }

    #[test]
    fn hmac_tags_depend_on_secret_purpose_and_value() {
        let base = hmac_tag("secret", "reddit-link", "a");
        assert_eq!(base, hmac_tag("secret", "reddit-link", "a"));
        assert_ne!(base, hmac_tag("secret", "reddit-link", "b"));
        assert_ne!(base, hmac_tag("other", "reddit-link", "a"));
        assert_ne!(base, hmac_tag("secret", "other", "a"));
    }

    #[test]
    fn signed_cookie_matches_cookie_signature() {
        assert_eq!(
            sign("abc", "secret"),
            "s:abc.mUba1OAOkT/Ivo5dP34RCkqegy+D+wnDRShdeGONig4"
        );
        assert_eq!(
            encode_component("s:abc.mUba1OAOkT/Ivo5dP34RCkqegy+D+wnDRShdeGONig4"),
            "s%3Aabc.mUba1OAOkT%2FIvo5dP34RCkqegy%2BD%2BwnDRShdeGONig4"
        );
    }

    #[test]
    fn expiry_uses_the_js_utc_string_format() {
        assert_eq!(utc_string(0), "Thu, 01 Jan 1970 00:00:00 GMT");
    }

    #[test]
    fn new_session_ids_look_like_uid_safe_ones() {
        let sid = new_sid();
        assert_eq!(sid.len(), 32);
        assert!(sid.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'));
    }
}
