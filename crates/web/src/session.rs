use base64::Engine;
use base64::engine::general_purpose::STANDARD_NO_PAD;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use sqlx::MySqlPool;

pub fn unsign(raw: &str, secret: &str) -> Option<String> {
    let val = raw.strip_prefix("s:")?;
    let dot = val.rfind('.')?;
    let (sid, sig) = (&val[..dot], &val[dot + 1..]);
    let sig = STANDARD_NO_PAD.decode(sig).ok()?;
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).ok()?;
    mac.update(sid.as_bytes());
    mac.verify_slice(&sig).ok()?;
    (!sid.is_empty()).then(|| sid.to_owned())
}

pub fn connect_sids(cookie_header: &str) -> Vec<String> {
    cookie_header
        .split(';')
        .map(str::trim)
        .filter_map(|p| p.strip_prefix("connect.sid="))
        .map(|v| {
            urlencoding::decode(v)
                .map(|c| c.into_owned())
                .unwrap_or_else(|_| v.to_owned())
        })
        .collect()
}

pub async fn resolve_user(pool: &MySqlPool, cookie_header: &str, secret: &str) -> Option<String> {
    for sid in connect_sids(cookie_header).iter().filter_map(|raw| unsign(raw, secret)) {
        let row: Option<(String,)> = match sqlx::query_as("SELECT json FROM session WHERE id = ? AND expiredAt > ?")
            .bind(&sid)
            .bind(crate::now_ms())
            .fetch_optional(pool)
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("session lookup failed: {e}");
                continue;
            }
        };
        let Some((json,)) = row else { continue };
        let uuid = serde_json::from_str::<serde_json::Value>(&json)
            .ok()
            .and_then(|v| v["passport"]["user"]["uuid"].as_str().map(str::to_owned));
        if uuid.is_some() {
            return uuid;
        }
    }
    None
}

pub fn origin_allowed(allowed: &[String], origin: Option<&str>) -> bool {
    match origin {
        None => true,
        Some(o) => allowed
            .iter()
            .any(|a| a.trim_end_matches('/') == o.trim_end_matches('/')),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_listed_origins_may_send_cookies() {
        let allowed = vec!["https://counting.gg".to_string(), "http://localhost:3000/".to_string()];
        assert!(origin_allowed(&allowed, Some("https://counting.gg")));
        assert!(origin_allowed(&allowed, Some("http://localhost:3000")));
        assert!(origin_allowed(&allowed, None));
        assert!(!origin_allowed(&allowed, Some("https://counting.gg.evil.example")));
        assert!(!origin_allowed(&allowed, Some("http://counting.gg")));
    }

    #[test]
    fn unsign_matches_cookie_signature() {
        let signed = "s:abc.mUba1OAOkT/Ivo5dP34RCkqegy+D+wnDRShdeGONig4";
        assert_eq!(unsign(signed, "secret").as_deref(), Some("abc"));
        assert_eq!(unsign(signed, "other"), None);
        assert_eq!(
            unsign("abc.mUba1OAOkT/Ivo5dP34RCkqegy+D+wnDRShdeGONig4", "secret"),
            None
        );
        assert_eq!(
            connect_sids("a=1; connect.sid=s%3Ax.y; connect.sid=z"),
            vec!["s:x.y", "z"]
        );
    }
}
