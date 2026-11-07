use cgg_core::var;
use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub struct Config {
    pub token: Option<String>,
    pub api_proxy: Option<String>,
    pub port: u16,
    pub socket_addr: String,
    pub redis_url: String,
    pub host_url: String,
    pub guild: Option<u64>,
    pub manage_roles: bool,
    pub admin_user: Option<u64>,
    pub log_channel: Option<u64>,
    pub mod_log_channel: Option<u64>,
    pub security_channel: Option<u64>,
    pub welcome_channel: Option<u64>,
    pub welcome_role: Option<u64>,
    pub help_channel: Option<u64>,
    pub daily_stats_channel: Option<u64>,
    pub verify_channel: Option<u64>,
    pub chat_mirror_channel: Option<u64>,
    pub bingo_chat_channel: Option<u64>,
    pub roles: HashMap<String, u64>,
    pub level_roles: Vec<(i64, u64)>,
    pub rank_roles: Vec<(String, u64)>,
}

pub const ROLE_KEYS: [(&str, &str); 7] = [
    ("counter_role", "DISCORD_COUNTER_ROLE_ID"),
    ("discord_verified_role", "DISCORD_VERIFIED_ROLE_ID"),
    ("manual_verification_needed_role", "DISCORD_MANUAL_VERIFICATION_ROLE_ID"),
    ("muted_role", "DISCORD_MUTED_ROLE_ID"),
    ("incendiliated_role", "DISCORD_BANNED_ROLE_ID"),
    ("admin_role", "DISCORD_ADMIN_ROLE_ID"),
    ("moderator_role", "DISCORD_MODERATOR_ROLE_ID"),
];

impl Config {
    pub fn from_env() -> anyhow::Result<Config> {
        Config::from_lookup(var)
    }

    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> anyhow::Result<Config> {
        let text = |k: &str| lookup(k).map(|v| v.trim().to_owned()).filter(|v| !v.is_empty());
        let id = |k: &str| text(k).and_then(|v| v.parse::<u64>().ok());
        let pairs = |k: &str| -> Vec<(String, u64)> {
            text(k)
                .unwrap_or_default()
                .split(',')
                .filter_map(|p| {
                    let (key, v) = p.split_once(':')?;
                    Some((key.trim().to_owned(), v.trim().parse().ok()?))
                })
                .collect()
        };
        let mut level_roles: Vec<(i64, u64)> = pairs("DISCORD_LEVEL_ROLES")
            .into_iter()
            .filter_map(|(k, v)| Some((k.parse().ok()?, v)))
            .collect();
        level_roles.sort_by_key(|r| std::cmp::Reverse(r.0));
        Ok(Config {
            token: text("DISCORD_BOT_TOKEN"),
            api_proxy: text("DISCORD_API_PROXY"),
            port: text("BOT_PORT").unwrap_or("3003".into()).parse()?,
            socket_addr: format!("127.0.0.1:{}", text("WEBSOCKET_APP_PORT").unwrap_or("3005".into())),
            redis_url: text("REDIS_URL").unwrap_or_else(|| {
                format!(
                    "redis://{}:{}",
                    text("REDIS_HOST").unwrap_or("127.0.0.1".into()),
                    text("REDIS_PORT").unwrap_or("6382".into())
                )
            }),
            host_url: text("HOST_URL").unwrap_or_default(),
            guild: id("DISCORD_GUILD_ID"),
            manage_roles: text("DISCORD_MANAGE_ROLES").as_deref() == Some("true"),
            admin_user: id("DISCORD_ADMIN_USER_ID"),
            log_channel: id("DISCORD_LOG_CHANNEL_ID"),
            mod_log_channel: id("DISCORD_MOD_LOG_CHANNEL_ID"),
            security_channel: id("DISCORD_SECURITY_CHANNEL_ID"),
            welcome_channel: id("DISCORD_WELCOME_CHANNEL_ID"),
            welcome_role: id("DISCORD_WELCOME_ROLE_ID"),
            help_channel: id("DISCORD_HELP_CHANNEL_ID"),
            daily_stats_channel: id("DISCORD_DAILY_STATS_CHANNEL_ID"),
            verify_channel: id("DISCORD_VERIFY_CHANNEL_ID"),
            chat_mirror_channel: id("CHAT_MIRROR_CHANNEL_ID"),
            bingo_chat_channel: id("BINGO_CHAT_CHANNEL_ID"),
            roles: ROLE_KEYS
                .iter()
                .filter_map(|(key, env)| Some(((*key).to_owned(), id(env)?)))
                .collect(),
            level_roles,
            rank_roles: pairs("DISCORD_RANK_ROLES"),
        })
    }

    pub fn admin_mention(&self) -> String {
        self.admin_user.map_or(String::new(), |u| format!("<@{u}>"))
    }

    pub fn level_role_for(&self, level: i64) -> Option<u64> {
        self.level_roles.iter().find(|(min, _)| level >= *min).map(|(_, r)| *r)
    }

    pub fn rank_role_for(&self, rank: &str) -> Option<u64> {
        self.rank_roles.iter().find(|(k, _)| k == rank).map(|(_, r)| *r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(pairs: &[(&str, &str)]) -> Config {
        let m: HashMap<String, String> = pairs.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect();
        Config::from_lookup(|k| m.get(k).cloned()).unwrap()
    }

    #[test]
    fn blank_ids_are_off() {
        let c = cfg(&[("DISCORD_LOG_CHANNEL_ID", " "), ("DISCORD_GUILD_ID", "12")]);
        assert_eq!(c.log_channel, None);
        assert_eq!(c.guild, Some(12));
        assert!(!c.manage_roles);
    }

    #[test]
    fn level_roles_pick_the_highest_reached() {
        let c = cfg(&[("DISCORD_LEVEL_ROLES", "20:1,65:5,40:3")]);
        assert_eq!(c.level_role_for(64), Some(3));
        assert_eq!(c.level_role_for(65), Some(5));
        assert_eq!(c.level_role_for(19), None);
    }

    #[test]
    fn rank_roles_by_name() {
        let c = cfg(&[("DISCORD_RANK_ROLES", "bronze:7, gold:9")]);
        assert_eq!(c.rank_role_for("gold"), Some(9));
        assert_eq!(c.rank_role_for("peak"), None);
    }
}
