use crate::error::ApiError;
use crate::state::AppState;
use axum::extract::{ConnectInfo, MatchedPath, Request, State};
use axum::http::HeaderMap;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use dashmap::DashMap;
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy)]
pub struct Limit {
    pub limit: usize,
    pub ttl: Duration,
}

pub const DEFAULT: Limit = Limit {
    limit: 60,
    ttl: Duration::from_secs(60),
};

#[derive(Debug, Clone, Copy)]
pub enum Rule {
    Default,
    Custom(Limit),
}

#[derive(Default)]
pub struct Throttler {
    hits: DashMap<String, VecDeque<Instant>>,
}

impl Throttler {
    pub fn allow(&self, key: String, max: usize, ttl: Duration) -> bool {
        let now = Instant::now();
        let mut q = self.hits.entry(key).or_default();
        while q.front().is_some_and(|t| *t <= now) {
            q.pop_front();
        }
        if q.len() >= max {
            return false;
        }
        q.push_back(now + ttl);
        true
    }

    pub fn sweep(&self) {
        let now = Instant::now();
        self.hits.retain(|_, q| q.back().is_some_and(|t| *t > now));
    }
}

pub fn forwarded_ip(headers: &HeaderMap, peer: Option<SocketAddr>, hops: usize) -> String {
    let direct = peer.map_or_else(|| "none".to_owned(), |p| canonical(p.ip()));
    if hops == 0 {
        return direct;
    }
    let chain: Vec<String> = headers
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect();
    let mut addrs = vec![direct];
    addrs.extend(chain.into_iter().rev());
    let idx = hops.min(addrs.len() - 1);
    addrs.swap_remove(idx)
}

fn canonical(ip: std::net::IpAddr) -> String {
    match ip {
        std::net::IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => format!("::ffff:{v4}"),
            None => v6.to_string(),
        },
        v4 => v4.to_string(),
    }
}

pub fn client_ip(state: &AppState, headers: &HeaderMap, peer: Option<SocketAddr>) -> String {
    if let Some(secret) = &state.cfg.ssr_client_ip_key {
        let presented = headers.get("x-ssr-client-ip-key").and_then(|v| v.to_str().ok());
        if presented.is_some_and(|p| constant_time_eq(p.as_bytes(), secret.as_bytes()))
            && let Some(f) = headers
                .get("x-counting-client-ip")
                .and_then(|v| v.to_str().ok())
                .filter(|f| !f.is_empty() && f.len() < 64)
        {
            return f.to_owned();
        }
    }
    forwarded_ip(headers, peer, state.cfg.trust_proxy_hops)
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[derive(Clone)]
pub struct ClientIp(pub String);

static CLIENT_IPS_LOGGED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
const CLIENT_IPS_TO_LOG: usize = 50;

fn log_first_client_ips(state: &AppState, headers: &HeaderMap, peer: Option<SocketAddr>, chosen: &str) {
    if CLIENT_IPS_LOGGED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= CLIENT_IPS_TO_LOG {
        return;
    }
    let h = |k: &str| headers.get(k).and_then(|v| v.to_str().ok()).unwrap_or("-").to_owned();
    let ssr = headers.contains_key("x-ssr-client-ip-key");
    tracing::info!(
        "client ip: chosen={chosen} hops={} peer={} ssr={ssr} x-forwarded-for=[{}] cf-connecting-ip={}",
        state.cfg.trust_proxy_hops,
        peer.map_or("-".into(), |p| p.ip().to_string()),
        h("x-forwarded-for"),
        h("cf-connecting-ip"),
    );
}

pub async fn layer(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    let peer = req.extensions().get::<ConnectInfo<SocketAddr>>().map(|c| c.0);
    let ip = client_ip(&state, req.headers(), peer);
    log_first_client_ips(&state, req.headers(), peer, &ip);
    req.extensions_mut().insert(ClientIp(ip.clone()));
    let route = req
        .extensions()
        .get::<MatchedPath>()
        .map(|m| m.as_str().to_owned())
        .unwrap_or_else(|| req.uri().path().to_owned());
    let key = format!("{} {route}", req.method());
    let rule = crate::routes::throttle_rule(&key);
    let allowed = match rule {
        Rule::Default => state
            .throttle
            .allow(format!("{key}|{ip}"), DEFAULT.limit * 2, DEFAULT.ttl),
        Rule::Custom(l) => state.throttle.allow(format!("{key}|{ip}"), l.limit * 2, l.ttl),
    };
    if !allowed {
        tracing::warn!("IP {ip} throttled on {key}");
        return ApiError::throttled().into_response();
    }
    next.run(req).await
}

impl<S: Send + Sync> axum::extract::FromRequestParts<S> for ClientIp {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut axum::http::request::Parts, _: &S) -> Result<Self, Self::Rejection> {
        Ok(parts
            .extensions
            .get::<ClientIp>()
            .cloned()
            .unwrap_or(ClientIp("none".into())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trusted_hops_pick_the_address_that_many_in_from_the_right() {
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", "1.1.1.1, 2.2.2.2, 3.3.3.3".parse().unwrap());
        let peer: SocketAddr = "127.0.0.1:5000".parse().unwrap();
        assert_eq!(forwarded_ip(&h, Some(peer), 0), "127.0.0.1");
        assert_eq!(forwarded_ip(&h, Some(peer), 1), "3.3.3.3");
        assert_eq!(forwarded_ip(&h, Some(peer), 2), "2.2.2.2");
        assert_eq!(forwarded_ip(&h, Some(peer), 9), "1.1.1.1");
    }

    #[test]
    fn a_window_admits_up_to_the_limit() {
        let t = Throttler::default();
        assert!(t.allow("k".into(), 2, Duration::from_secs(60)));
        assert!(t.allow("k".into(), 2, Duration::from_secs(60)));
        assert!(!t.allow("k".into(), 2, Duration::from_secs(60)));
        assert!(t.allow("other".into(), 2, Duration::from_secs(60)));
    }
}
