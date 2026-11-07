use crate::error::ApiError;
use axum::body::{Body, Bytes};
use axum::extract::{FromRequest, Request};
use axum::http::header::CONTENT_TYPE;
use axum::http::{HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value};

pub const BODY_LIMIT: usize = 100 * 1024;

pub struct Reply(pub Value);

#[derive(Clone, Copy)]
pub struct KeepStatus;

pub fn reply(v: impl Into<Value>) -> Reply {
    Reply(v.into())
}

pub fn empty() -> Reply {
    Reply(Value::Null)
}

pub fn js_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

impl IntoResponse for Reply {
    fn into_response(self) -> Response {
        match self.0 {
            Value::Null => Response::new(Body::empty()),
            v @ (Value::Object(_) | Value::Array(_)) => axum::Json(v).into_response(),
            other => {
                let mut r = Response::new(Body::from(js_string(&other)));
                r.headers_mut()
                    .insert(CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
                r
            }
        }
    }
}

pub struct WithStatus(pub StatusCode, pub Reply);

impl IntoResponse for WithStatus {
    fn into_response(self) -> Response {
        let mut r = self.1.into_response();
        *r.status_mut() = self.0;
        r.extensions_mut().insert(KeepStatus);
        r
    }
}

pub async fn post_created(req: Request, next: Next) -> Response {
    let is_post = req.method() == Method::POST;
    let mut res = next.run(req).await;
    if is_post && res.status() == StatusCode::OK && res.extensions().get::<KeepStatus>().is_none() {
        *res.status_mut() = StatusCode::CREATED;
    }
    res
}

pub struct JsonBody(pub Value);

impl JsonBody {
    pub fn get(&self, key: &str) -> &Value {
        self.0.get(key).unwrap_or(&Value::Null)
    }

    pub fn str(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(Value::as_str)
    }
}

impl<S: Send + Sync> FromRequest<S> for JsonBody {
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let ct = req
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        let bytes = Bytes::from_request(req, state).await.map_err(|_| {
            ApiError::raw(
                StatusCode::PAYLOAD_TOO_LARGE,
                serde_json::json!({ "statusCode": 413, "message": "request entity too large" }),
            )
        })?;
        if bytes.len() > BODY_LIMIT {
            return Err(ApiError::raw(
                StatusCode::PAYLOAD_TOO_LARGE,
                serde_json::json!({ "statusCode": 413, "message": "request entity too large" }),
            ));
        }
        if ct.contains("json") {
            if bytes.iter().all(u8::is_ascii_whitespace) {
                return Ok(JsonBody(Value::Object(Map::new())));
            }
            return serde_json::from_slice(&bytes)
                .map(JsonBody)
                .map_err(|e| ApiError::bad_request(format!("Unexpected token in JSON at position {}", e.column())));
        }
        if ct.contains("application/x-www-form-urlencoded") {
            let map: Map<String, Value> = url_pairs(&bytes)
                .into_iter()
                .map(|(k, v)| (k, Value::String(v)))
                .collect();
            return Ok(JsonBody(Value::Object(map)));
        }
        Ok(JsonBody(Value::Object(Map::new())))
    }
}

fn url_pairs(bytes: &[u8]) -> Vec<(String, String)> {
    String::from_utf8_lossy(bytes)
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            let dec = |s: &str| {
                urlencoding::decode(&s.replace('+', " "))
                    .map(|c| c.into_owned())
                    .unwrap_or_default()
            };
            (dec(k), dec(v))
        })
        .collect()
}

pub fn query_map(query: Option<&str>) -> Map<String, Value> {
    url_pairs(query.unwrap_or("").as_bytes())
        .into_iter()
        .map(|(k, v)| (k, Value::String(v)))
        .collect()
}

pub struct Query(pub Map<String, Value>);

impl Query {
    pub fn str(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(Value::as_str)
    }
}

impl<S: Send + Sync> axum::extract::FromRequestParts<S> for Query {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut axum::http::request::Parts, _: &S) -> Result<Self, Self::Rejection> {
        Ok(Query(query_map(parts.uri.query())))
    }
}

pub fn js_number(v: &Value) -> f64 {
    match v {
        Value::Number(n) => n.as_f64().unwrap_or(f64::NAN),
        Value::String(s) => js_to_number(s),
        Value::Bool(b) => f64::from(u8::from(*b)),
        Value::Null => 0.0,
        _ => f64::NAN,
    }
}

pub fn js_to_number(s: &str) -> f64 {
    let t = counting::js::trim(s);
    if t.is_empty() {
        return 0.0;
    }
    let radix = |p: &str, r: u32| {
        let digits = &t[p.len()..];
        if digits.is_empty() {
            return f64::NAN;
        }
        u64::from_str_radix(digits, r).map_or(f64::NAN, |n| n as f64)
    };
    let lower = t.to_ascii_lowercase();
    if lower.starts_with("0x") {
        return radix("0x", 16);
    }
    if lower.starts_with("0b") {
        return radix("0b", 2);
    }
    if lower.starts_with("0o") {
        return radix("0o", 8);
    }
    match t {
        "Infinity" | "+Infinity" => return f64::INFINITY,
        "-Infinity" => return f64::NEG_INFINITY,
        _ => {}
    }
    let valid = t.bytes().all(|b| b.is_ascii_digit() || b"+-.eE".contains(&b)) && !lower.contains("inf");
    if !valid {
        return f64::NAN;
    }
    t.parse::<f64>().unwrap_or(f64::NAN)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_follows_js_number_rules() {
        assert_eq!(js_to_number(" 12 "), 12.0);
        assert_eq!(js_to_number(""), 0.0);
        assert!(js_to_number("12abc").is_nan());
        assert_eq!(js_to_number("0x10"), 16.0);
        assert_eq!(js_to_number("1e3"), 1000.0);
        assert_eq!(js_to_number(".5"), 0.5);
    }
}
