use crate::{App, mirror, roles};
use serde_json::json;
use serenity::all::{
    ChannelId, Command, Context, CreateCommand, CreateInteractionResponse, CreateInteractionResponseMessage,
    EventHandler, Guild, GuildId, Interaction, Member, Message, MessageId, Permissions, ReactionType, Ready,
    UnavailableGuild,
};
use serenity::async_trait;
use sqlx::Row;
use std::sync::Arc;

pub struct Handler {
    pub app: Arc<App>,
}

const MIN_ACCOUNT_AGE_MS: i64 = 1000 * 60 * 60 * 24 * 14;

const NOT_REGISTERED: &str = "I don't see an account under this discord account registered on the website. Click the \"Login with Discord\" button on the site and log in there to gain access to this command. If you have already done so, please contact a moderator and we can help you. <https://counting.gg>";

struct SiteCounter {
    uuid: String,
    roles: Vec<String>,
}

impl SiteCounter {
    fn has(&self, role: &str) -> bool {
        self.roles.iter().any(|r| r == role)
    }

    fn can_chat(&self) -> bool {
        self.has("counter") && !self.has("banned") && !self.has("muted")
    }
}

fn parse_roles(raw: &str) -> Vec<String> {
    serde_json::from_str(raw).unwrap_or_else(|_| raw.split(',').filter(|r| !r.is_empty()).map(str::to_owned).collect())
}

async fn counter_by_discord(app: &App, discord_id: &str) -> Option<SiteCounter> {
    let row = sqlx::query("SELECT uuid, roles FROM counters WHERE discordId = ? LIMIT 1")
        .bind(discord_id)
        .fetch_optional(&app.pool)
        .await
        .ok()??;
    Some(SiteCounter {
        uuid: row.try_get("uuid").unwrap_or_default(),
        roles: parse_roles(&row.try_get::<String, _>("roles").unwrap_or_default()),
    })
}

async fn react(ctx: &Context, msg: &Message, emoji: char) {
    let _ = msg.react(ctx, ReactionType::Unicode(emoji.to_string())).await;
}

impl Handler {
    async fn verify(&self, ctx: &Context, msg: &Message) {
        let app = &self.app;
        let Some(member) = &msg.member else { return };
        if let Some(r) = app.cfg.roles.get("discord_verified_role")
            && member.roles.iter().any(|x| x.get() == *r)
        {
            return;
        }
        let discord_id = msg.author.id.to_string();
        let Some(c) = counter_by_discord(app, &discord_id).await else {
            let _ = msg.reply(ctx, NOT_REGISTERED).await;
            react(ctx, msg, '\u{274c}').await;
            return;
        };
        let give = |key: &'static str| roles::add_named(app, &discord_id, key);
        if c.has("counter") {
            let key = if c.has("banned") {
                "incendiliated_role"
            } else if c.has("muted") {
                "muted_role"
            } else {
                "counter_role"
            };
            let _ = give(key).await;
            react(ctx, msg, '\u{2705}').await;
            return;
        }
        let age = cgg_web::now_ms() - msg.author.id.created_at().unix_timestamp() * 1000;
        let (key, role) = if age < MIN_ACCOUNT_AGE_MS {
            ("manual_verification_needed_role", "manual_verification_needed")
        } else {
            ("discord_verified_role", "discord_verified")
        };
        let _ = give(key).await;
        react(ctx, msg, '\u{2705}').await;
        if let Err(e) = sqlx::query("UPDATE counters SET roles = ? WHERE uuid = ?")
            .bind(json!([role]).to_string())
            .bind(&c.uuid)
            .execute(&app.pool)
            .await
        {
            tracing::warn!("verify for {discord_id}: {e}");
        }
    }

    async fn bingo_inbound(&self, ctx: &Context, msg: &Message, key: String) {
        let app = &self.app;
        let text = msg.content.trim();
        if text.is_empty() {
            return;
        }
        let Some(c) = counter_by_discord(app, &msg.author.id.to_string()).await else {
            return;
        };
        if !c.can_chat() {
            return;
        }
        let (game, team) = key.split_once(':').unwrap_or((&key, "all"));
        let team = if team == "all" {
            json!(null)
        } else {
            json!(team.parse::<i64>().ok())
        };
        let is_mod = self.manages_threads(ctx, msg).await;
        app.socket.emit(
            "bingoChatFromDiscord",
            json!({
                "gameId": game.parse::<i64>().ok(),
                "teamId": team,
                "counterUuid": c.uuid,
                "text": text,
                "discordMessageId": msg.id.to_string(),
                "isMod": is_mod,
            }),
        );
    }

    async fn manages_threads(&self, ctx: &Context, msg: &Message) -> bool {
        let (Some(gid), Some(parent)) = (msg.guild_id, self.app.cfg.bingo_chat_channel) else {
            return false;
        };
        let Ok(member) = gid.member(ctx, msg.author.id).await else {
            return false;
        };
        let Some(guild) = ctx.cache.guild(gid) else {
            return false;
        };
        let Some(channel) = guild.channels.get(&ChannelId::new(parent)) else {
            return false;
        };
        let p = guild.user_permissions_in(channel, &member);
        p.contains(Permissions::MANAGE_THREADS) || p.contains(Permissions::ADMINISTRATOR)
    }

    async fn mirror_inbound(&self, msg: &Message) {
        let app = &self.app;
        let text = msg.content.trim();
        if text.is_empty() {
            return;
        }
        let Some(c) = counter_by_discord(app, &msg.author.id.to_string()).await else {
            return;
        };
        if !c.can_chat() {
            return;
        }
        app.socket.emit(
            "discordChatPost",
            json!({
                "counter_uuid": c.uuid,
                "thread_name": "no_counting",
                "text": text,
                "discord_message_id": msg.id.to_string(),
                "server_received_at": cgg_web::now_ms(),
            }),
        );
    }
}

async fn server_row(app: &App, guild: GuildId, active_only: bool) -> Option<bool> {
    let sql = if active_only {
        "SELECT isBanned FROM servers WHERE discordID = ? AND isActive = 1 LIMIT 1"
    } else {
        "SELECT isBanned FROM servers WHERE discordID = ? LIMIT 1"
    };
    let row = sqlx::query(sql)
        .bind(guild.to_string())
        .fetch_optional(&app.pool)
        .await
        .ok()??;
    Some(row.try_get::<i8, _>("isBanned").unwrap_or(0) != 0)
}

async fn set_server_active(app: &App, guild: GuildId, active: bool) {
    let _ = sqlx::query("UPDATE servers SET isActive = ? WHERE discordID = ?")
        .bind(active)
        .bind(guild.to_string())
        .execute(&app.pool)
        .await;
}

async fn add_member_to_server(app: &App, guild: GuildId, member_id: &str) -> anyhow::Result<()> {
    let server = guild.to_string();
    let user: Option<String> = sqlx::query_scalar("SELECT uuid FROM users WHERE discord_id = ? LIMIT 1")
        .bind(member_id)
        .fetch_optional(&app.pool)
        .await?;
    let Some(user) = user else { return Ok(()) };
    sqlx::query(
        "INSERT INTO userserver (userID, serverID) SELECT ?, ? FROM DUAL \
         WHERE NOT EXISTS (SELECT 1 FROM userserver WHERE userID = ? AND serverID = ?)",
    )
    .bind(&user)
    .bind(&server)
    .bind(&user)
    .bind(&server)
    .execute(&app.pool)
    .await?;
    let counter: Option<String> = sqlx::query_scalar("SELECT uuid FROM counters WHERE discordId = ? LIMIT 1")
        .bind(member_id)
        .fetch_optional(&app.pool)
        .await?;
    let Some(counter) = counter else { return Ok(()) };
    sqlx::query(
        "INSERT INTO counterserver (counterID, serverID) SELECT ?, ? FROM DUAL \
         WHERE NOT EXISTS (SELECT 1 FROM counterserver WHERE counterID = ? AND serverID = ?)",
    )
    .bind(&counter)
    .bind(&server)
    .bind(&counter)
    .bind(&server)
    .execute(&app.pool)
    .await?;
    Ok(())
}

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, ctx: Context, ready: Ready) {
        tracing::info!("Discord bot online! Logged in as {}", ready.user.name);
        let commands = vec![CreateCommand::new("ping").description("Replies with Pong!")];
        if let Err(e) = Command::set_global_commands(&ctx, commands).await {
            tracing::warn!("command registration failed: {e}");
        }
        let app = self.app.clone();
        tokio::spawn(async move {
            if let Err(e) = roles::sync_all(&app).await {
                tracing::warn!("role sync failed: {e:#}");
            }
        });
    }

    async fn guild_create(&self, _ctx: Context, guild: Guild, is_new: Option<bool>) {
        if is_new != Some(true) {
            return;
        }
        tracing::info!("Bot joined server: {} (ID: {})", guild.name, guild.id);
        let app = self.app.clone();
        let Some(banned) = server_row(&app, guild.id, false).await else {
            return;
        };
        if banned {
            tracing::info!("Server is banned: {} (ID: {})", guild.name, guild.id);
            if let Some(http) = app.discord() {
                let _ = guild.id.leave(http.as_ref()).await;
            }
        }
        set_server_active(&app, guild.id, true).await;
        tokio::spawn(async move {
            match roles::all_members(&app, guild.id).await {
                Ok(members) => {
                    for m in members {
                        if let Err(e) = add_member_to_server(&app, guild.id, &m.user.id.to_string()).await {
                            tracing::warn!("server member {}: {e:#}", m.user.id);
                        }
                    }
                }
                Err(e) => tracing::warn!("members of {}: {e:#}", guild.id),
            }
        });
    }

    async fn guild_delete(&self, _ctx: Context, incomplete: UnavailableGuild, _full: Option<Guild>) {
        if incomplete.unavailable {
            return;
        }
        tracing::info!("Bot left server: {}", incomplete.id);
        if server_row(&self.app, incomplete.id, false).await.is_some() {
            set_server_active(&self.app, incomplete.id, false).await;
        }
    }

    async fn guild_member_addition(&self, _ctx: Context, member: Member) {
        if server_row(&self.app, member.guild_id, true).await.is_some()
            && let Err(e) = add_member_to_server(&self.app, member.guild_id, &member.user.id.to_string()).await
        {
            tracing::warn!("server member {}: {e:#}", member.user.id);
        }
    }

    async fn interaction_create(&self, ctx: Context, interaction: Interaction) {
        if let Interaction::Command(cmd) = interaction
            && cmd.data.name == "ping"
        {
            let reply = CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content("Pong!"));
            let _ = cmd.create_response(&ctx, reply).await;
        }
    }

    async fn message(&self, ctx: Context, msg: Message) {
        let cfg = &self.app.cfg;
        if msg.content == "!verify" && Some(msg.channel_id.get()) == cfg.verify_channel {
            self.verify(&ctx, &msg).await;
        }
        if msg.author.bot {
            return;
        }
        if cfg.bingo_chat_channel.is_some() {
            let key: Option<String> = match self.app.redis().await {
                Ok(mut r) => redis::AsyncCommands::get(&mut r, format!("bingo_chat:dthread_of:{}", msg.channel_id))
                    .await
                    .ok()
                    .flatten(),
                Err(_) => None,
            };
            if let Some(key) = key {
                self.bingo_inbound(&ctx, &msg, key).await;
                return;
            }
        }
        if Some(msg.channel_id.get()) == cfg.chat_mirror_channel {
            self.mirror_inbound(&msg).await;
        }
    }

    async fn message_delete(
        &self,
        _ctx: Context,
        channel_id: ChannelId,
        deleted_message_id: MessageId,
        _guild_id: Option<GuildId>,
    ) {
        if Some(channel_id.get()) != self.app.cfg.chat_mirror_channel {
            return;
        }
        if let Err(e) = mirror::deleted_in_discord(&self.app, deleted_message_id.get()).await {
            tracing::warn!("messageDelete mirror error: {e:#}");
        }
    }
}
