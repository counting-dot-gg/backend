use crate::App;
use anyhow::Result;
use serenity::all::{GuildId, Member, RoleId, UserId};
use sqlx::Row;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

fn user_id(discord_id: &str) -> Option<UserId> {
    discord_id.parse::<u64>().ok().filter(|n| *n != 0).map(UserId::new)
}

fn guild(app: &App) -> Option<GuildId> {
    app.cfg.guild.map(GuildId::new)
}

fn managing(app: &App) -> Option<GuildId> {
    if app.cfg.manage_roles && app.discord().is_some() {
        guild(app)
    } else {
        None
    }
}

async fn member(app: &App, g: GuildId, user: UserId) -> Option<Member> {
    g.member(app.discord()?.as_ref(), user).await.ok()
}

async fn set_role(app: &App, g: GuildId, user: UserId, role: u64, on: bool) -> Result<()> {
    let Some(http) = app.discord() else { return Ok(()) };
    if on {
        http.add_member_role(g, user, RoleId::new(role), None).await?;
    } else {
        http.remove_member_role(g, user, RoleId::new(role), None).await?;
    }
    Ok(())
}

async fn named(app: &App, discord_id: &str, key: &str, on: bool) -> Result<()> {
    let (Some(g), Some(user), Some(role)) = (managing(app), user_id(discord_id), app.cfg.roles.get(key)) else {
        return Ok(());
    };
    if member(app, g, user).await.is_none() {
        tracing::info!("No user found");
        return Ok(());
    }
    set_role(app, g, user, *role, on).await
}

pub async fn add_named(app: &App, discord_id: &str, key: &str) -> Result<()> {
    named(app, discord_id, key, true).await
}

pub async fn remove_named(app: &App, discord_id: &str, key: &str) -> Result<()> {
    named(app, discord_id, key, false).await
}

async fn wear_only(app: &App, g: GuildId, m: &Member, want: Option<u64>, tier: &[u64]) {
    let has = |r: u64| m.roles.contains(&RoleId::new(r));
    for r in tier.iter().copied().filter(|r| Some(*r) != want && has(*r)) {
        if let Err(e) = set_role(app, g, m.user.id, r, false).await {
            tracing::info!("roles for {} failed: {e}", m.user.id);
        }
    }
    if let Some(w) = want.filter(|w| !has(*w))
        && let Err(e) = set_role(app, g, m.user.id, w, true).await
    {
        tracing::info!("roles for {} failed: {e}", m.user.id);
    }
}

fn level_tier(app: &App) -> Vec<u64> {
    app.cfg.level_roles.iter().map(|(_, r)| *r).collect()
}

fn rank_tier(app: &App) -> Vec<u64> {
    app.cfg.rank_roles.iter().map(|(_, r)| *r).collect()
}

pub async fn sync_level(app: &App, discord_id: &str, level: i64) -> Result<()> {
    let (Some(g), Some(user)) = (managing(app), user_id(discord_id)) else {
        return Ok(());
    };
    if let Some(m) = member(app, g, user).await {
        wear_only(app, g, &m, app.cfg.level_role_for(level), &level_tier(app)).await;
    }
    Ok(())
}

pub async fn sync_rank(app: &App, discord_id: &str, rank: Option<&str>) -> Result<()> {
    let (Some(g), Some(user)) = (managing(app), user_id(discord_id)) else {
        return Ok(());
    };
    if let Some(m) = member(app, g, user).await {
        let want = rank.and_then(|r| app.cfg.rank_role_for(r));
        wear_only(app, g, &m, want, &rank_tier(app)).await;
    }
    Ok(())
}

pub async fn all_members(app: &App, g: GuildId) -> Result<Vec<Member>> {
    let Some(http) = app.discord() else {
        return Ok(Vec::new());
    };
    let mut out: Vec<Member> = Vec::new();
    let mut after = None;
    loop {
        let page = g.members(http.as_ref(), Some(1000), after).await?;
        let n = page.len();
        after = page.last().map(|m| m.user.id);
        out.extend(page);
        if n < 1000 {
            return Ok(out);
        }
    }
}

fn marks(n: usize) -> String {
    vec!["?"; n].join(", ")
}

pub async fn sync_all(app: &App) -> Result<()> {
    let Some(g) = managing(app) else { return Ok(()) };
    let members = all_members(app, g).await?;
    let by_id: HashMap<String, &Member> = members.iter().map(|m| (m.user.id.to_string(), m)).collect();
    let ids: Vec<&String> = by_id.keys().collect();
    let mut levels = 0usize;
    for page in ids.chunks(1000) {
        let sql = format!(
            "SELECT discordId, xp FROM counters WHERE discordId IN ({})",
            marks(page.len())
        );
        let mut q = sqlx::query(&sql);
        for id in page {
            q = q.bind(id.as_str());
        }
        for row in q.fetch_all(&app.pool).await? {
            let id: String = row.try_get("discordId").unwrap_or_default();
            let xp = row
                .try_get::<f64, _>("xp")
                .or_else(|_| row.try_get::<i64, _>("xp").map(|x| x as f64))
                .unwrap_or(0.0);
            let level: i64 = cgg_web::level_of(xp).parse().unwrap_or(1);
            if let Some(m) = by_id.get(&id) {
                wear_only(app, g, m, app.cfg.level_role_for(level), &level_tier(app)).await;
                levels += 1;
            }
        }
    }
    tracing::info!("Level roles synced for {} members ({levels} counters)", members.len());
    let mut ranked: HashMap<String, String> = HashMap::new();
    for page in ids.chunks(1000) {
        let sql = format!(
            "SELECT c.discordId, r.`rank` FROM counters c \
             JOIN threadrank r ON r.counterUuid = c.uuid AND r.threadUuid IS NULL \
             JOIN rankseason s ON s.id = r.seasonId AND s.endedAt IS NULL \
             WHERE c.discordId IN ({})",
            marks(page.len())
        );
        let mut q = sqlx::query(&sql);
        for id in page {
            q = q.bind(id.as_str());
        }
        for row in q.fetch_all(&app.pool).await? {
            ranked.insert(
                row.try_get("discordId").unwrap_or_default(),
                row.try_get("rank").unwrap_or_default(),
            );
        }
    }
    let tier = rank_tier(app);
    for (id, m) in &by_id {
        let rank = ranked.get(id);
        let has = tier.iter().any(|r| m.roles.contains(&RoleId::new(*r)));
        if rank.is_some() || has {
            let want = rank.and_then(|r| app.cfg.rank_role_for(r));
            wear_only(app, g, m, want, &tier).await;
        }
    }
    tracing::info!("Rank roles synced for {} ranked members", ranked.len());
    Ok(())
}

pub fn start_avatar_checks(app: &Arc<App>) {
    let app = app.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(15 * 60));
        tick.tick().await;
        loop {
            tick.tick().await;
            if let Err(e) = check_avatars(&app).await {
                tracing::warn!("avatar check failed: {e:#}");
            }
        }
    });
}

async fn check_avatars(app: &App) -> Result<()> {
    let Some(g) = guild(app) else { return Ok(()) };
    let members = all_members(app, g).await?;
    if members.is_empty() {
        return Ok(());
    }
    let avatars: HashMap<String, Option<String>> = members
        .iter()
        .map(|m| (m.user.id.to_string(), m.user.avatar.map(|a| a.to_string())))
        .collect();
    let rows = sqlx::query(
        "SELECT uuid, name, discordId, avatar, roles FROM counters WHERE roles LIKE '%counter%' ORDER BY id ASC",
    )
    .fetch_all(&app.pool)
    .await?;
    let (mut updated, mut removed) = (0, 0);
    for r in rows {
        let uuid: String = r.try_get("uuid").unwrap_or_default();
        let discord: String = r.try_get("discordId").unwrap_or_default();
        let current: Option<String> = r.try_get("avatar").ok().flatten();
        let roles: String = r.try_get("roles").unwrap_or_default();
        let restricted = roles.contains("banned") || roles.contains("muted");
        let next = match avatars.get(&discord) {
            Some(Some(a)) if Some(a) != current.as_ref() && !restricted => Some(a.clone()),
            Some(_) => None,
            None if current.as_deref() != Some("0") => Some("0".to_owned()),
            None => None,
        };
        if let Some(next) = next {
            sqlx::query("UPDATE counters SET avatar = ? WHERE uuid = ?")
                .bind(&next)
                .bind(&uuid)
                .execute(&app.pool)
                .await?;
            if next == "0" {
                removed += 1;
            } else {
                updated += 1;
            }
        }
    }
    tracing::info!("Checking discord avatars complete. Updated: {updated}, removed: {removed}");
    Ok(())
}
