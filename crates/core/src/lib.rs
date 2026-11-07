use anyhow::{Context, Result};
use sqlx::MySqlPool;
use sqlx::mysql::{MySqlConnectOptions, MySqlPoolOptions};

pub fn load_env() {
    dotenvy::dotenv().ok();
}

pub fn var(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.trim().is_empty())
}

pub fn init_logging(default_filter: &str) {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| default_filter.into()),
        )
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stdout()))
        .init();
}

#[derive(Debug, Clone)]
pub struct MysqlConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
    pub database: String,
}

impl MysqlConfig {
    pub fn from_env() -> Result<Self> {
        Ok(MysqlConfig {
            host: var("MYSQL_DB_HOST").unwrap_or("localhost".into()),
            port: var("MYSQL_DB_PORT")
                .unwrap_or("3306".into())
                .parse()
                .context("MYSQL_DB_PORT is not a port number")?,
            user: var("MYSQL_DB_USERNAME").unwrap_or("root".into()),
            password: var("MYSQL_DB_PASSWORD").unwrap_or_default(),
            database: var("MYSQL_DB_DATABASE").unwrap_or("cgg".into()),
        })
    }

    pub fn connect_options(&self) -> MySqlConnectOptions {
        MySqlConnectOptions::new()
            .host(&self.host)
            .port(self.port)
            .username(&self.user)
            .password(&self.password)
            .database(&self.database)
            .charset("utf8mb4")
    }

    pub async fn pool(&self, max: u32) -> Result<MySqlPool> {
        Ok(MySqlPoolOptions::new()
            .max_connections(max)
            .connect_with(self.connect_options())
            .await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_variables_count_as_unset() {
        unsafe { std::env::set_var("CGG_CORE_TEST_BLANK", "  ") };
        assert_eq!(var("CGG_CORE_TEST_BLANK"), None);
    }
}
