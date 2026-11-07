use anyhow::{Context, Result};
use base64::Engine;
use base64::engine::general_purpose::STANDARD_NO_PAD;
use hmac::{Hmac, Mac};
use serde_json::json;
use sha2::Sha256;
use sqlx::mysql::MySqlConnectOptions;
use sqlx::{ConnectOptions, Executor, MySqlConnection, MySqlPool};

use crate::{guard, schema};

pub const SCHEMA: &str = include_str!("../schema.sql");

#[derive(Clone)]
pub struct Target {
    pub mysql: cgg_core::MysqlConfig,
    pub redis_url: String,
    pub cookie_secret: String,
}

impl Target {
    pub fn from_env() -> Result<Target> {
        Target::new(guard::DATABASE_PREFIX, format!("harness-{:016x}", fastrand::u64(..)))
    }

    pub fn dev() -> Result<Target> {
        Target::new(
            &format!("{}_dev", guard::DATABASE_PREFIX),
            "cgg-dev-cookie-secret".into(),
        )
    }

    fn new(default_db: &str, cookie_secret: String) -> Result<Target> {
        let mut mysql = cgg_core::MysqlConfig::from_env()?;
        mysql.database = cgg_core::var("HARNESS_DB").unwrap_or_else(|| default_db.into());
        if let Some(host) = cgg_core::var("HARNESS_MYSQL_HOST") {
            mysql.host = host;
        }
        guard::check_mysql(&mysql.host, &mysql.database)?;
        let redis_url = cgg_core::var("HARNESS_REDIS_URL").unwrap_or_else(|| "redis://127.0.0.1:6379/9".into());
        guard::check_redis(&redis_url)?;
        Ok(Target {
            mysql,
            redis_url,
            cookie_secret,
        })
    }

    fn server_options(&self) -> MySqlConnectOptions {
        MySqlConnectOptions::new()
            .host(&self.mysql.host)
            .port(self.mysql.port)
            .username(&self.mysql.user)
            .password(&self.mysql.password)
            .charset("utf8mb4")
    }

    pub async fn reset(&self) -> Result<()> {
        let mut server = self
            .server_options()
            .connect()
            .await
            .with_context(|| format!("connect to MySQL at {}:{}", self.mysql.host, self.mysql.port))?;
        let db = &self.mysql.database;
        server
            .execute(format!("DROP DATABASE IF EXISTS `{db}`").as_str())
            .await?;
        server
            .execute(format!("CREATE DATABASE `{db}` CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci").as_str())
            .await?;
        let mut conn: MySqlConnection = self.mysql.connect_options().connect().await?;
        schema::load(&mut conn, SCHEMA).await?;
        let client = redis::Client::open(self.redis_url.as_str())?;
        let mut r = client
            .get_multiplexed_async_connection()
            .await
            .with_context(|| format!("connect to Redis at {}", self.redis_url))?;
        redis::cmd("FLUSHDB").query_async::<_, ()>(&mut r).await?;
        Ok(())
    }

    pub async fn has_data(&self) -> Result<bool> {
        let mut server = self.server_options().connect().await?;
        let found: Option<String> = sqlx::query_scalar(
            "SELECT CAST(schema_name AS CHAR) FROM information_schema.schemata WHERE schema_name = ?",
        )
        .bind(&self.mysql.database)
        .fetch_optional(&mut server)
        .await?;
        Ok(found.is_some())
    }

    pub async fn pool(&self) -> Result<MySqlPool> {
        self.mysql.pool(4).await
    }
}

pub const SITE_ORIGIN: &str = "http://harness.test";

pub struct Account {
    pub uuid: String,
    pub cookie: String,
}

pub struct ThreadSpec<'a> {
    pub name: &'a str,
    pub allow_double_posts: bool,
}

pub async fn seed(
    pool: &MySqlPool,
    target: &Target,
    threads: &[ThreadSpec<'_>],
    accounts: usize,
) -> Result<Vec<Account>> {
    for (i, t) in threads.iter().enumerate() {
        sqlx::query(
            "INSERT INTO thread (uuid, name, title, description, rules, firstValidCount, validationType, \
             resetOnMistakes, allowDoublePosts, visibleTo, updatableBy, locked, autoValidated, moderators, \
             verifiers, countBans, postBans, shortDescription, color1, color2, category) \
             VALUES (?, ?, ?, '', '', '1', 'main', 0, ?, '[\"all\"]', '[\"counter\"]', 0, 1, '[]', '[]', '[]', '[]', '', '#000', '#fff', 'Harness')",
        )
        .bind(format!("harness-thread-{i}"))
        .bind(t.name)
        .bind(t.name)
        .bind(t.allow_double_posts)
        .execute(pool)
        .await?;
    }
    let mut out = Vec::with_capacity(accounts);
    for n in 0..accounts {
        let uuid = format!("harness-counter-{n:05}");
        sqlx::query(
            "INSERT INTO counters (uuid, discordId, name, color, roles, avatar, cardStyle, cardBorderStyle, pronouns, title, username) \
             VALUES (?, ?, ?, '#888888', '[\"counter\"]', '', 'Default', 'Default', '[]', '', ?)",
        )
        .bind(&uuid)
        .bind(format!("harness{n}"))
        .bind(format!("harness {n}"))
        .bind(format!("harness_{n}"))
        .execute(pool)
        .await?;
        let sid = format!("harness-session-{n:05}");
        let session =
            json!({ "cookie": {}, "passport": { "user": { "uuid": uuid, "discordId": format!("harness{n}") } } });
        sqlx::query("INSERT INTO session (expiredAt, id, json) VALUES (?, ?, ?)")
            .bind(i64::MAX / 2)
            .bind(&sid)
            .bind(session.to_string())
            .execute(pool)
            .await?;
        out.push(account(target, n));
    }
    Ok(out)
}

pub async fn expired_session_cookie(pool: &MySqlPool, target: &Target, uuid: &str) -> Result<String> {
    let sid = format!("harness-expired-{uuid}");
    let session = json!({ "cookie": {}, "passport": { "user": { "uuid": uuid } } });
    sqlx::query("INSERT INTO session (expiredAt, id, json) VALUES (?, ?, ?)")
        .bind(1_000i64)
        .bind(&sid)
        .bind(session.to_string())
        .execute(pool)
        .await?;
    Ok(format!("connect.sid={}", sign(&sid, &target.cookie_secret)))
}

pub fn account(target: &Target, n: usize) -> Account {
    Account {
        uuid: format!("harness-counter-{n:05}"),
        cookie: format!(
            "connect.sid={}",
            sign(&format!("harness-session-{n:05}"), &target.cookie_secret)
        ),
    }
}

pub fn sign(sid: &str, secret: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac accepts any key");
    mac.update(sid.as_bytes());
    format!("s:{sid}.{}", STANDARD_NO_PAD.encode(mac.finalize().into_bytes()))
}

#[cfg(test)]
mod tests {
    #[test]
    fn cookie_signature_matches_express_session() {
        assert_eq!(
            super::sign("abc", "secret"),
            "s:abc.mUba1OAOkT/Ivo5dP34RCkqegy+D+wnDRShdeGONig4"
        );
    }
}
