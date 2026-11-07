use anyhow::{Context, Result};
use cgg_core::{MysqlConfig, var};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Config {
    pub port: u16,
    pub cookie_secret: String,
    pub cookie_domain: Option<String>,
    pub insecure_cookies: bool,
    pub mysql: MysqlConfig,
    pub redis_url: String,
    pub host_url: Option<String>,
    pub www_host_url: Option<String>,
    pub trust_proxy_hops: usize,
    pub ssr_client_ip_key: Option<String>,
    pub socket_addr: String,
    pub bot_addr: String,
    pub request_timeout: Duration,
    pub select_timeout_ms: u64,
    pub uploads_dir: PathBuf,
    pub stats_http_url: Option<String>,
    pub stats_api_key: Option<String>,
    pub discord: DiscordOauth,
    pub reddit: RedditOauth,
}

#[derive(Debug, Clone, Default)]
pub struct DiscordOauth {
    pub client_id: String,
    pub client_secret: String,
    pub redirect_url: String,
    pub localhost_redirect_url: String,
}

#[derive(Debug, Clone, Default)]
pub struct RedditOauth {
    pub client_id: String,
    pub client_secret: String,
    pub redirect_url: String,
}

fn or_empty(k: &str) -> String {
    var(k).unwrap_or_default()
}

impl Config {
    pub fn from_env() -> Result<Config> {
        let positive_ms = |k: &str, d: u64| {
            var(k)
                .and_then(|v| v.parse::<f64>().ok())
                .filter(|n| n.is_finite() && *n > 0.0)
                .map_or(d, |n| n as u64)
        };
        let redis_url = var("REDIS_URL").unwrap_or_else(|| {
            format!(
                "redis://{}:{}",
                var("REDIS_HOST").unwrap_or("127.0.0.1".into()),
                var("REDIS_PORT").unwrap_or("6379".into())
            )
        });
        Ok(Config {
            port: var("PORT").unwrap_or("3001".into()).parse().context("PORT")?,
            cookie_secret: var("COOKIE_SECRET").context("COOKIE_SECRET must be set")?,
            cookie_domain: var("COOKIE_DOMAIN"),
            insecure_cookies: var("HTTPS").as_deref() == Some("false"),
            mysql: MysqlConfig::from_env()?,
            redis_url,
            host_url: var("HOST_URL"),
            www_host_url: var("WWW_HOST_URL"),
            trust_proxy_hops: var("TRUST_PROXY_HOPS").unwrap_or("0".into()).parse().unwrap_or(0),
            ssr_client_ip_key: var("SSR_CLIENT_IP_KEY"),
            socket_addr: format!("127.0.0.1:{}", var("WEBSOCKET_APP_PORT").unwrap_or("3005".into())),
            bot_addr: format!("127.0.0.1:{}", var("BOT_PORT").unwrap_or("3003".into())),
            request_timeout: Duration::from_millis(positive_ms("REQUEST_TIMEOUT_MS", 120_000)),
            select_timeout_ms: positive_ms("MYSQL_SELECT_TIMEOUT_MS", 60_000),
            uploads_dir: var("UPLOADS_DIR").unwrap_or("uploads".into()).into(),
            stats_http_url: var("STATS_HTTP_URL"),
            stats_api_key: var("STATS_API_KEY"),
            discord: DiscordOauth {
                client_id: or_empty("DISCORD_CLIENT_ID"),
                client_secret: or_empty("DISCORD_CLIENT_SECRET"),
                redirect_url: or_empty("DISCORD_REDIRECT_URL"),
                localhost_redirect_url: or_empty("DISCORD_LOCALHOST_REDIRECT_URL"),
            },
            reddit: RedditOauth {
                client_id: or_empty("REDDIT_CLIENT_ID"),
                client_secret: or_empty("REDDIT_CLIENT_SECRET"),
                redirect_url: or_empty("REDDIT_REDIRECT_URL"),
            },
        })
    }
}
