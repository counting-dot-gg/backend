use crate::entity;
use crate::state::AppState;
use cgg_web::now_ms;
use serde_json::json;
use stats::achievements::{AchievementRef, Award, LiveEvent};

pub fn record(state: &AppState, counter: &str, event: LiveEvent<'_>) {
    for award in progression::live_awards(&event) {
        let (state, counter) = (state.clone(), counter.to_owned());
        tokio::spawn(async move {
            let r = match award {
                Award::Earn(r) => earn(&state, &counter, r).await,
                Award::Progress(r, add) => progress(&state, &counter, r, add).await,
            };
            if let Err(e) = r {
                tracing::warn!("achievement for {counter}: {e:#}");
            }
        });
    }
}

async fn def(state: &AppState, r: &AchievementRef) -> anyhow::Result<Option<serde_json::Value>> {
    let row = match r {
        AchievementRef::Id(id) => {
            sqlx::query("SELECT * FROM achievements WHERE id = ?")
                .bind(id)
                .fetch_optional(&state.pool)
                .await?
        }
        AchievementRef::Name(n) => {
            sqlx::query("SELECT * FROM achievements WHERE name = ? LIMIT 1")
                .bind(n)
                .fetch_optional(&state.pool)
                .await?
        }
    };
    Ok(row.map(|r| entity::ACHIEVEMENT.json(&r)))
}

pub async fn earn(state: &AppState, counter: &str, r: AchievementRef) -> anyhow::Result<()> {
    let Some(mut a) = def(state, &r).await? else {
        anyhow::bail!("Achievement {r:?} not found.");
    };
    let id = a["id"].as_i64().unwrap_or_default();
    let max = a["maxProgress"].as_i64().unwrap_or_default();
    let existing: Option<(i32, i8)> = sqlx::query_as(
        "SELECT id, isComplete FROM counter_achievements WHERE counterUUID = ? AND achievementId = ? LIMIT 1",
    )
    .bind(counter)
    .bind(id)
    .fetch_optional(&state.pool)
    .await?;
    let now = now_ms().to_string();
    match existing {
        Some((_, done)) if done != 0 => return Ok(()),
        Some((row, _)) => {
            sqlx::query("UPDATE counter_achievements SET isComplete = 1, lastChecked = 1, progress = ?, timestamp = ? WHERE id = ?")
                .bind(max)
                .bind(&now)
                .bind(row)
                .execute(&state.pool)
                .await?;
        }
        None => {
            sqlx::query(
                "INSERT INTO counter_achievements (counterUUID, achievementId, isComplete, lastChecked, progress, timestamp) VALUES (?, ?, 1, 1, ?, ?)",
            )
            .bind(counter)
            .bind(id)
            .bind(max)
            .bind(&now)
            .execute(&state.pool)
            .await?;
        }
    }
    sqlx::query("UPDATE achievements SET countersEarned = countersEarned + 1 WHERE id = ?")
        .bind(id)
        .execute(&state.pool)
        .await?;
    a["countersEarned"] = json!(a["countersEarned"].as_i64().unwrap_or_default() + 1);
    state.socket.emit(
        "emitAchievementEarned",
        json!({ "counter_uuid": counter, "achievement": a }),
    );
    Ok(())
}

async fn progress(state: &AppState, counter: &str, r: AchievementRef, add: i64) -> anyhow::Result<()> {
    let Some(a) = def(state, &r).await? else {
        anyhow::bail!("Achievement {r:?} not found.");
    };
    let id = a["id"].as_i64().unwrap_or_default();
    let max = a["maxProgress"].as_i64().unwrap_or_default();
    let existing: Option<(i32, i8, i32)> = sqlx::query_as(
        "SELECT id, isComplete, progress FROM counter_achievements WHERE counterUUID = ? AND achievementId = ? LIMIT 1",
    )
    .bind(counter)
    .bind(id)
    .fetch_optional(&state.pool)
    .await?;
    let total = match existing {
        Some((_, done, _)) if done != 0 => return Ok(()),
        Some((_, _, p)) => i64::from(p) + add,
        None => add,
    };
    if total >= max {
        return Box::pin(earn(state, counter, AchievementRef::Id(id as i32))).await;
    }
    match existing {
        Some((row, _, _)) => {
            sqlx::query("UPDATE counter_achievements SET lastChecked = 1, progress = ? WHERE id = ?")
                .bind(total)
                .bind(row)
                .execute(&state.pool)
                .await?;
        }
        None => {
            sqlx::query("INSERT INTO counter_achievements (counterUUID, achievementId, isComplete, lastChecked, progress) VALUES (?, ?, 0, 1, ?)")
                .bind(counter)
                .bind(id)
                .bind(total)
                .execute(&state.pool)
                .await?;
        }
    }
    Ok(())
}
