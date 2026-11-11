use crate::error::{ApiError, ApiResult};
use crate::http::{Reply, reply};
use crate::session::Authed;
use crate::state::AppState;
use axum::Router;
use axum::extract::State;
use axum::routing::get;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use cgg_web::now_ms;
use hmac::{Hmac, Mac};
use serde_json::json;
use sha2::Sha256;

const TOKEN_TTL_MS: i64 = 30 * 60 * 1000;

pub fn routes() -> Router<AppState> {
    Router::new().route("/do/token", get(token))
}

pub fn sign(secret: &str, uuid: &str, exp: i64) -> String {
    let body = format!("{uuid}.{exp}");
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac accepts any key length");
    mac.update(body.as_bytes());
    format!("{body}.{}", URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()))
}

async fn token(State(state): State<AppState>, Authed(user): Authed) -> ApiResult<Reply> {
    let Some(secret) = state.cfg.do_token_secret.as_deref() else {
        return Err(ApiError::not_found("regional counting is off"));
    };
    let exp = now_ms() + TOKEN_TTL_MS;
    Ok(reply(json!({ "token": sign(secret, &user.uuid, exp), "exp": exp })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_matches_the_worker() {
        assert_eq!(
            sign("secret", "17192906-4250-4569-8230-530657569230", 1_700_000_000_000),
            "17192906-4250-4569-8230-530657569230.1700000000000.44hXuGTqLMht1DpmBa0gcZfRjZ3LRaV8uqy8G-RBapM"
        );
    }
}
