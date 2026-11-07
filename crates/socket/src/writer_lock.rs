use crate::config::MysqlConfig;
use anyhow::{Result, bail};
use sqlx::{ConnectOptions, MySqlConnection};
use std::time::Duration;

pub fn lock_name(database: &str) -> String {
    format!("cgg-socket:{database}")
}

pub struct WriterLock {
    conn: MySqlConnection,
    name: String,
    cfg: MysqlConfig,
}

enum Take {
    Got(MySqlConnection),
    HeldBy(Option<i64>),
}

async fn take(cfg: &MysqlConfig, name: &str) -> Result<Take> {
    let mut conn = cfg.connect_options().connect().await?;
    let got: Option<i64> = sqlx::query_scalar("SELECT CAST(GET_LOCK(?, 0) AS SIGNED)")
        .bind(name)
        .fetch_one(&mut conn)
        .await?;
    if got == Some(1) {
        return Ok(Take::Got(conn));
    }
    let holder: Option<i64> = sqlx::query_scalar("SELECT CAST(IS_USED_LOCK(?) AS SIGNED)")
        .bind(name)
        .fetch_one(&mut conn)
        .await?;
    Ok(Take::HeldBy(holder))
}

pub async fn acquire(cfg: &MysqlConfig) -> Result<WriterLock> {
    let name = lock_name(&cfg.database);
    match take(cfg, &name).await? {
        Take::Got(conn) => {
            tracing::info!("writer lock {name} held");
            Ok(WriterLock {
                conn,
                name,
                cfg: cfg.clone(),
            })
        }
        Take::HeldBy(holder) => bail!(
            "another socket server is running against `{}` (MySQL connection {} holds the lock `{name}`); \
             stop it first (pm2 stop socket / socket-rs)",
            cfg.database,
            holder.map_or("?".into(), |h| h.to_string())
        ),
    }
}

impl WriterLock {
    pub fn keep(mut self) {
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(30)).await;
                let mine: Result<Option<i64>, _> =
                    sqlx::query_scalar("SELECT CAST(IS_USED_LOCK(?) = CONNECTION_ID() AS SIGNED)")
                        .bind(&self.name)
                        .fetch_one(&mut self.conn)
                        .await;
                if let Ok(Some(1)) = mine {
                    continue;
                }
                tracing::warn!("writer lock {}: connection lost ({mine:?}); re-taking", self.name);
                loop {
                    match take(&self.cfg, &self.name).await {
                        Ok(Take::Got(conn)) => {
                            tracing::info!("writer lock {} held again", self.name);
                            self.conn = conn;
                            break;
                        }
                        Ok(Take::HeldBy(holder)) => {
                            tracing::error!(
                                "writer lock {} is now held by MySQL connection {holder:?}: another socket server started; \
                                 exiting so two never write at once",
                                self.name
                            );
                            std::process::exit(75);
                        }
                        Err(e) => {
                            tracing::warn!("writer lock {}: MySQL unreachable ({e}); retrying", self.name);
                            tokio::time::sleep(Duration::from_secs(2)).await;
                        }
                    }
                }
            }
        });
    }
}
