mod bingo;
mod commands;
mod config;
mod events;
mod format;
mod logs;
mod mirror;
mod roles;

use anyhow::Result;
use cgg_web::nest;
use config::Config;
use dashmap::DashSet;
use redis::aio::ConnectionManager;
use serenity::all::{ChannelId, CreateMessage, GatewayIntents, Http, HttpBuilder, Message};
use serenity::cache::Cache;
use sqlx::MySqlPool;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::sync::OnceCell;

pub struct App {
    pub cfg: Config,
    pub pool: MySqlPool,
    pub socket: nest::Client,
    redis_url: String,
    redis: OnceCell<ConnectionManager>,
    pub http: Option<Arc<Http>>,
    pub cache: OnceLock<Arc<Cache>>,
    pub mirror: mirror::Buffer,
    pub deleting_from_discord: DashSet<String>,
    pub bingo: bingo::Threads,
}

impl App {
    pub async fn redis(&self) -> Result<ConnectionManager> {
        let conn = self
            .redis
            .get_or_try_init(|| async {
                let client = redis::Client::open(self.redis_url.as_str())?;
                tokio::time::timeout(Duration::from_secs(2), ConnectionManager::new(client))
                    .await
                    .map_err(|_| anyhow::anyhow!("redis connect timed out"))?
                    .map_err(anyhow::Error::from)
            })
            .await?;
        Ok(conn.clone())
    }

    pub fn discord(&self) -> Option<&Arc<Http>> {
        self.http.as_ref()
    }

    pub async fn send(&self, channel: Option<u64>, msg: CreateMessage) -> Result<Option<Message>> {
        let (Some(http), Some(channel)) = (self.discord(), channel) else {
            return Ok(None);
        };
        Ok(Some(ChannelId::new(channel).send_message(http, msg).await?))
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    cgg_core::load_env();
    cgg_core::init_logging("info");
    let cfg = Config::from_env()?;
    let pool = cgg_core::MysqlConfig::from_env()?.pool(5).await?;
    let http = cfg.token.as_deref().map(|t| {
        let builder = HttpBuilder::new(t);
        let builder = match &cfg.api_proxy {
            Some(p) => builder.proxy(p.as_str()).ratelimiter_disabled(true),
            None => builder,
        };
        Arc::new(builder.build())
    });
    let app = Arc::new(App {
        socket: nest::Client::connect("socket", cfg.socket_addr.clone()),
        redis_url: cfg.redis_url.clone(),
        redis: OnceCell::new(),
        http,
        cache: OnceLock::new(),
        mirror: Default::default(),
        deleting_from_discord: DashSet::new(),
        bingo: Default::default(),
        pool,
        cfg,
    });
    if app.cfg.token.is_some() && app.cfg.api_proxy.is_none() {
        let a = app.clone();
        tokio::spawn(async move {
            loop {
                if let Err(e) = run_gateway(a.clone()).await {
                    tracing::error!("discord gateway stopped: {e:#}");
                }
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
        });
        roles::start_avatar_checks(&app);
    } else if app.cfg.token.is_none() {
        tracing::warn!("DISCORD_BOT_TOKEN not set: Discord is off");
    }
    let port = app.cfg.port;
    let handler: nest::Handler = Arc::new(move |cmd, data| {
        let app = app.clone();
        Box::pin(async move { commands::handle(app, cmd, data).await })
    });
    tracing::info!("Bot: Running on PORT {port}");
    tokio::select! {
        r = nest::serve(port, handler) => r,
        _ = tokio::signal::ctrl_c() => Ok(()),
    }
}

async fn run_gateway(app: Arc<App>) -> Result<()> {
    let token = app.cfg.token.clone().unwrap_or_default();
    let intents = GatewayIntents::GUILDS
        | GatewayIntents::GUILD_MEMBERS
        | GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::MESSAGE_CONTENT
        | GatewayIntents::GUILD_MESSAGE_REACTIONS
        | GatewayIntents::DIRECT_MESSAGES
        | GatewayIntents::DIRECT_MESSAGE_REACTIONS;
    let mut client = serenity::Client::builder(&token, intents)
        .event_handler(events::Handler { app: app.clone() })
        .await?;
    let _ = app.cache.set(client.cache.clone());
    client.start().await?;
    Ok(())
}
