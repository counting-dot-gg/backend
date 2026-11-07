use axum::{
    body::Body,
    http::{Request, StatusCode},
    middleware::Next,
    response::Response,
};

pub async fn require_api_key(req: Request<Body>, next: Next) -> Result<Response, StatusCode> {
    let expected = std::env::var("STATS_API_KEY").unwrap_or_default();
    if expected.is_empty() {
        return Ok(next.run(req).await);
    }

    let provided = req
        .headers()
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if provided != expected {
        return Err(StatusCode::UNAUTHORIZED);
    }

    Ok(next.run(req).await)
}
