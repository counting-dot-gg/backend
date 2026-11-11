use anyhow::{Context, Result, bail};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalMode {
    Durable,
    Fast,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub port: u16,
    pub cookie_secret: String,
    pub game_secret: String,
    pub mysql: MysqlConfig,
    pub wal_dir: PathBuf,
    pub wal_mode: WalMode,
    pub wal_sync_interval: Duration,
    pub wal_segment_bytes: u64,
    pub persist_batch: usize,
    pub server_tz: chrono_tz::Tz,
    pub site_version: String,
    pub regional: crate::regional::RegionalConfig,
    pub cors_origins: Vec<String>,
    pub trust_proxy_hops: usize,
    pub max_connections_per_ip: usize,
    pub discord: DiscordIds,
    pub socket_path: String,
    pub post_burst_per_sec: usize,
    pub rate_limit_ip_factor: usize,
    pub rate_limits: bool,
    pub tcp_port: u16,
    pub bot_addr: String,
    pub stats_addr: String,
    pub redis_url: String,
    pub host_url: String,
}

#[derive(Debug, Clone)]
pub struct DiscordIds {
    pub milestone_channel: Option<String>,
    pub alert_channel: Option<String>,
    pub kpart_main: Option<KPartTarget>,
    pub kpart_other: Option<KPartTarget>,
    pub kpart_side: Option<KPartTarget>,
}

#[derive(Debug, Clone)]
pub struct KPartTarget {
    pub channel: String,
    pub role: String,
}

impl DiscordIds {
    fn from_env() -> DiscordIds {
        DiscordIds::from_lookup(var)
    }

    fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> DiscordIds {
        let id = |k: &str| lookup(k).filter(|v| !v.trim().is_empty());
        let kpart = |prefix: &str| {
            Some(KPartTarget {
                channel: id(&format!("KPART_{prefix}_CHANNEL_ID"))?,
                role: id(&format!("KPART_{prefix}_ROLE_ID"))?,
            })
        };
        DiscordIds {
            milestone_channel: id("DISCORD_MILESTONE_CHANNEL_ID"),
            alert_channel: id("DISCORD_ALERT_CHANNEL_ID"),
            kpart_main: kpart("MAIN"),
            kpart_other: kpart("OTHER"),
            kpart_side: kpart("SIDE"),
        }
    }
}

pub use cgg_core::MysqlConfig;
use cgg_core::var;

impl Config {
    pub fn from_env() -> Result<Self> {
        let wal_mode = match var("WAL_MODE").as_deref().unwrap_or("durable") {
            "durable" => WalMode::Durable,
            "fast" => WalMode::Fast,
            other => bail!("WAL_MODE must be durable or fast, not {other}"),
        };
        let mut cors_origins: Vec<String> = [var("HOST_URL"), var("WWW_HOST_URL")].into_iter().flatten().collect();
        cors_origins.extend(
            [
                "http://localhost:3000",
                "https://www.counting.gg",
                "https://counting.gg",
            ]
            .map(String::from),
        );
        Ok(Config {
            port: var("SOCKET_PORT")
                .or_else(|| var("RUST_WS_PORT"))
                .or_else(|| var("WEBSOCKET_PORT"))
                .unwrap_or("8805".into())
                .parse()?,
            cookie_secret: var("COOKIE_SECRET").context("COOKIE_SECRET must be set")?,
            game_secret: var("GAME_SECRET").or_else(|| var("COOKIE_SECRET")).unwrap_or_default(),
            mysql: MysqlConfig::from_env()?,
            wal_dir: var("WAL_DIR").unwrap_or("wal".into()).into(),
            wal_mode,
            wal_sync_interval: Duration::from_micros(var("WAL_SYNC_US").unwrap_or("2000".into()).parse()?),
            wal_segment_bytes: var("WAL_SEGMENT_MB").unwrap_or("64".into()).parse::<u64>()? << 20,
            persist_batch: var("PERSIST_BATCH").unwrap_or("1000".into()).parse()?,
            server_tz: var("SERVER_TZ")
                .unwrap_or("America/New_York".into())
                .parse()
                .map_err(|e| anyhow::anyhow!("SERVER_TZ: {e}"))?,
            site_version: var("SITE_VERSION").unwrap_or("v1.3.8".into()),
            regional: crate::regional::RegionalConfig::from_env(),
            cors_origins,
            trust_proxy_hops: var("TRUST_PROXY_HOPS").unwrap_or("0".into()).parse()?,
            max_connections_per_ip: var("MAX_CONNECTIONS_PER_IP").unwrap_or("300".into()).parse()?,
            discord: DiscordIds::from_env(),
            socket_path: var("SOCKET_PATH").unwrap_or("/socket.io".into()),
            post_burst_per_sec: var("POST_BURST_PER_SEC").unwrap_or("25".into()).parse()?,
            rate_limit_ip_factor: var("RATE_LIMIT_IP_FACTOR").unwrap_or("4".into()).parse()?,
            rate_limits: var("RATE_LIMITS").as_deref() != Some("off"),
            tcp_port: var("WEBSOCKET_APP_PORT").unwrap_or("3005".into()).parse()?,
            bot_addr: format!("localhost:{}", var("BOT_PORT").unwrap_or("3003".into())),
            stats_addr: format!("localhost:{}", var("STATS_PORT").unwrap_or("3002".into())),
            redis_url: var("REDIS_URL").unwrap_or_else(|| {
                format!(
                    "redis://{}:{}/",
                    var("REDIS_HOST").unwrap_or("127.0.0.1".into()),
                    var("REDIS_PORT").unwrap_or("6382".into())
                )
            }),
            host_url: var("HOST_URL").unwrap_or_default(),
        })
    }
}

#[cfg(test)]
mod discord_ids_tests {
    use super::*;
    use std::collections::HashMap;

    fn ids(pairs: &[(&str, &str)]) -> DiscordIds {
        let env: HashMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        DiscordIds::from_lookup(|k| env.get(k).cloned())
    }

    #[test]
    fn unset_or_blank_ids_switch_messages_off() {
        let d = ids(&[("DISCORD_MILESTONE_CHANNEL_ID", " ")]);
        assert!(d.milestone_channel.is_none());
        assert!(d.alert_channel.is_none());
        assert!(d.kpart_main.is_none());
    }

    #[test]
    fn a_kpart_target_needs_both_channel_and_role() {
        let d = ids(&[
            ("KPART_MAIN_CHANNEL_ID", "1"),
            ("KPART_MAIN_ROLE_ID", "2"),
            ("KPART_SIDE_CHANNEL_ID", "3"),
        ]);
        let main = d.kpart_main.unwrap();
        assert_eq!((main.channel.as_str(), main.role.as_str()), ("1", "2"));
        assert!(d.kpart_side.is_none());
    }
}
