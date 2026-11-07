use crate::state::AppState;
use axum::extract::{Request, State};
use axum::http::header::{
    ACCESS_CONTROL_ALLOW_CREDENTIALS, ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS,
    ACCESS_CONTROL_ALLOW_ORIGIN, ACCESS_CONTROL_REQUEST_HEADERS, CONTENT_LENGTH, ORIGIN, VARY,
};
use axum::http::{HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::Response;

const FIXED: [&str; 5] = [
    "https://counting.gg",
    "https://www.counting.gg",
    "http://localhost:3000",
    "http://localhost",
    "https://localhost",
];

fn private_lan(origin: &str) -> bool {
    let Some(rest) = origin.strip_prefix("http://") else {
        return false;
    };
    let Some((host, port)) = rest.rsplit_once(':') else {
        return false;
    };
    if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let parts: Vec<&str> = host.split('.').collect();
    if parts.len() != 4
        || !parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
    {
        return false;
    }
    let second: u32 = parts[1].parse().unwrap_or(999);
    match parts[0] {
        "10" => true,
        "192" => parts[1] == "168",
        "172" => (16..=31).contains(&second) && parts[1].len() == 2,
        _ => false,
    }
}

pub fn allowed(state: &AppState, origin: &str) -> bool {
    let cfg = &state.cfg;
    FIXED.contains(&origin)
        || origin == "https://localhost:3000"
        || cfg.host_url.as_deref() == Some(origin)
        || cfg.www_host_url.as_deref() == Some(origin)
        || (cfg.insecure_cookies && private_lan(origin))
}

pub async fn layer(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let origin = req
        .headers()
        .get(ORIGIN)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let ok = origin.as_deref().is_some_and(|o| allowed(&state, o));
    if req.method() == Method::OPTIONS {
        let requested = req.headers().get(ACCESS_CONTROL_REQUEST_HEADERS).cloned();
        let mut res = Response::new(axum::body::Body::empty());
        *res.status_mut() = StatusCode::NO_CONTENT;
        let h = res.headers_mut();
        if let (true, Some(o)) = (ok, origin.as_deref()) {
            if let Ok(v) = HeaderValue::from_str(o) {
                h.insert(ACCESS_CONTROL_ALLOW_ORIGIN, v);
            }
            h.insert(ACCESS_CONTROL_ALLOW_CREDENTIALS, HeaderValue::from_static("true"));
        }
        h.append(VARY, HeaderValue::from_static("Origin"));
        h.insert(
            ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("GET,HEAD,PUT,PATCH,POST,DELETE"),
        );
        if let Some(r) = requested {
            h.insert(ACCESS_CONTROL_ALLOW_HEADERS, r);
            h.append(VARY, HeaderValue::from_static("Access-Control-Request-Headers"));
        }
        h.insert(CONTENT_LENGTH, HeaderValue::from_static("0"));
        return res;
    }
    let mut res = next.run(req).await;
    let h = res.headers_mut();
    if let (true, Some(o)) = (ok, origin.as_deref()) {
        if let Ok(v) = HeaderValue::from_str(o) {
            h.insert(ACCESS_CONTROL_ALLOW_ORIGIN, v);
        }
        h.insert(ACCESS_CONTROL_ALLOW_CREDENTIALS, HeaderValue::from_static("true"));
    }
    h.append(VARY, HeaderValue::from_static("Origin"));
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lan_origins_follow_the_private_ranges() {
        assert!(private_lan("http://10.0.0.236:3000"));
        assert!(private_lan("http://192.168.1.4:3000"));
        assert!(private_lan("http://172.16.0.1:80"));
        assert!(!private_lan("http://172.32.0.1:80"));
        assert!(!private_lan("http://10.0.0.236"));
        assert!(!private_lan("https://10.0.0.236:3000"));
        assert!(!private_lan("http://11.0.0.1:3000"));
    }
}
