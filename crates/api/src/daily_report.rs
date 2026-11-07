use crate::entity;
use crate::state::AppState;
use base64::Engine;
use chrono::{DateTime, Duration, NaiveDate, TimeZone, Timelike, Utc};
use chrono_tz::America::{Chicago, New_York};
use serde_json::{Value, json};
use sqlx::Row;
use std::io::Write;

pub fn start(state: &AppState) {
    let s = state.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(until_next_hour_mark(Utc::now())).await;
            if Utc::now().with_timezone(&New_York).hour() != 0 {
                continue;
            }
            let day = report_day(Utc::now());
            tracing::info!("daily report for {day}");
            if let Err(e) = report(&s, day).await {
                tracing::error!("daily report for {day} failed: {e:#}");
            }
            if let Err(e) = upload_day(&s, day).await {
                tracing::error!("daily post upload for {day} failed: {e:#}");
            }
        }
    });
}

fn until_next_hour_mark(now: DateTime<Utc>) -> std::time::Duration {
    let hour = now
        .with_minute(1)
        .and_then(|t| t.with_second(0))
        .and_then(|t| t.with_nanosecond(0))
        .unwrap_or(now);
    let next = if hour > now { hour } else { hour + Duration::hours(1) };
    (next - now).to_std().unwrap_or_default()
}

pub fn report_day(now: DateTime<Utc>) -> NaiveDate {
    now.with_timezone(&Chicago).date_naive()
}

fn eastern_bounds(day: NaiveDate) -> (i64, i64) {
    let at = |d: NaiveDate| {
        New_York
            .from_local_datetime(&d.and_hms_opt(0, 0, 0).unwrap())
            .earliest()
            .map_or(0, |t| t.timestamp() * 1000)
    };
    (at(day), at(day.succ_opt().unwrap_or(day)))
}

struct Place {
    uuid: String,
    count: i64,
}

async fn leaderboard(state: &AppState, day: NaiveDate, thread_id: Option<i64>) -> anyhow::Result<(i64, Vec<Place>)> {
    let (start, end) = eastern_bounds(day);
    let in_thread = if thread_id.is_some() { " AND threadId = ?" } else { "" };
    let sql = format!(
        "SELECT authorUUID, COUNT(*) AS n, MIN(id) AS first FROM post \
         WHERE timestamp >= ? AND timestamp < ? AND isValidCount = 1{in_thread} \
         GROUP BY authorUUID ORDER BY n DESC, first ASC"
    );
    let mut q = sqlx::query(&sql).bind(start.to_string()).bind(end.to_string());
    if let Some(t) = thread_id {
        q = q.bind(t);
    }
    let rows = q.fetch_all(&state.pool).await?;
    let places: Vec<Place> = rows
        .iter()
        .map(|r| Place {
            uuid: r
                .try_get::<Option<String>, _>("authorUUID")
                .ok()
                .flatten()
                .unwrap_or_default(),
            count: r.try_get("n").unwrap_or(0),
        })
        .collect();
    let total = places.iter().map(|p| p.count).sum();
    Ok((total, places.into_iter().take(3).collect()))
}

async fn names(state: &AppState, places: &[Place]) -> anyhow::Result<Vec<String>> {
    let mut out = Vec::new();
    for p in places {
        let name: Option<String> = sqlx::query_scalar("SELECT name FROM counters WHERE uuid = ? LIMIT 1")
            .bind(&p.uuid)
            .fetch_optional(&state.pool)
            .await?;
        out.push(name.unwrap_or_default());
    }
    Ok(out)
}

async fn report(state: &AppState, day: NaiveDate) -> anyhow::Result<()> {
    let main: Option<i64> = sqlx::query_scalar("SELECT id FROM thread WHERE name = 'main' LIMIT 1")
        .fetch_optional(&state.pool)
        .await?;
    let (total_all, top_all) = leaderboard(state, day, None).await?;
    let (total_main, top_main) = match main {
        Some(id) => leaderboard(state, day, Some(id)).await?,
        None => (0, Vec::new()),
    };
    let (all_names, main_names) = (names(state, &top_all).await?, names(state, &top_main).await?);
    let medals: Vec<Value> = top_all
        .iter()
        .enumerate()
        .map(|(i, p)| {
            json!({
                "name": all_names[i],
                "count": p.count,
                "mainName": main_names.get(i),
                "mainCount": top_main.get(i).map(|m| m.count),
            })
        })
        .collect();
    state.bot.emit(
        "dailyStatsReport",
        json!({
            "dateLabel": chrono::Local::now().format("%a %b %d %Y").to_string(),
            "totalAll": total_all,
            "totalMain": total_main,
            "medals": medals,
        }),
    );
    Ok(())
}

const UPLOAD_DROPPED: [&str; 6] = [
    "comment",
    "hasComment",
    "rawText",
    "isDeleted",
    "isCommentDeleted",
    "reactions",
];

async fn upload_day(state: &AppState, day: NaiveDate) -> anyhow::Result<()> {
    let (Some(token), Some(repo), Some(user)) = (
        cgg_core::var("GITHUB_TOKEN"),
        cgg_core::var("GITHUB_REPO"),
        cgg_core::var("GITHUB_USER"),
    ) else {
        return Ok(());
    };
    let (start, end) = eastern_bounds(day);
    let rows = sqlx::query("SELECT * FROM post WHERE timestamp > ? AND timestamp < ? ORDER BY id ASC")
        .bind(start as f64)
        .bind(end as f64)
        .fetch_all(&state.pool)
        .await?;
    let posts: Vec<Value> = rows
        .iter()
        .map(|r| {
            let mut p = entity::POST.json(r);
            if let Some(o) = p.as_object_mut() {
                for k in UPLOAD_DROPPED {
                    o.remove(k);
                }
            }
            p
        })
        .collect();
    let body = serde_json::to_vec(&posts)?;
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(&body)?;
    let content = base64::engine::general_purpose::STANDARD.encode(gz.finish()?);
    let day = day.format("%Y-%m-%d");
    let res = state
        .http
        .put(format!(
            "https://api.github.com/repos/{user}/{repo}/contents/data/{day}.json.gz"
        ))
        .bearer_auth(token)
        .header("User-Agent", "counting.gg")
        .json(&json!({ "message": format!("Daily post dump for {day}"), "content": content }))
        .timeout(std::time::Duration::from_secs(120))
        .send()
        .await?;
    tracing::info!("daily post upload for {day}: {}", res.status());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_midnight_run_reports_the_day_that_just_ended() {
        let just_after_midnight_eastern = Utc.with_ymd_and_hms(2026, 11, 6, 5, 1, 0).unwrap();
        assert_eq!(
            report_day(just_after_midnight_eastern),
            NaiveDate::from_ymd_opt(2026, 11, 5).unwrap()
        );
    }

    #[test]
    fn wakes_at_one_minute_past_each_hour() {
        let now = Utc.with_ymd_and_hms(2026, 11, 5, 4, 30, 0).unwrap();
        assert_eq!(until_next_hour_mark(now).as_secs(), 31 * 60);
        let on_mark = Utc.with_ymd_and_hms(2026, 11, 5, 4, 1, 0).unwrap();
        assert_eq!(until_next_hour_mark(on_mark).as_secs(), 3600);
    }

    #[test]
    fn eastern_day_spans_dst_change() {
        let (s, e) = eastern_bounds(NaiveDate::from_ymd_opt(2026, 11, 1).unwrap());
        assert_eq!(e - s, 25 * 3600 * 1000);
    }
}
