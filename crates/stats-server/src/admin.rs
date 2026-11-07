use anyhow::{Result, anyhow};
use serde::Serialize;
use std::sync::Arc;

use crate::AppState;
use stats::{
    counter_stats::update_counter_stats,
    db,
    models::{CounterStats, Post},
    redis_client,
    stats_engine::{StatsOptions, run_stats},
};

pub const KINDS: [&str; 5] = ["all", "day", "counter", "achievements", "gaps"];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminJob {
    pub id: String,
    pub kind: String,
    pub target: Option<String>,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub status: String,
    pub error: Option<String>,
    pub note: Option<String>,
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

pub enum StartError {
    AlreadyRunning(AdminJob),
    BadRequest(String),
}

pub async fn start(state: Arc<AppState>, kind: &str, target: Option<String>) -> Result<AdminJob, StartError> {
    if !KINDS.contains(&kind) {
        return Err(StartError::BadRequest(format!(
            "kind must be one of {}",
            KINDS.join(", ")
        )));
    }
    match kind {
        "day" if !is_day(target.as_deref()) => return Err(StartError::BadRequest("A day must be YYYY-MM-DD.".into())),
        "counter" if target.as_deref().unwrap_or("").is_empty() => {
            return Err(StartError::BadRequest("A counter uuid is required.".into()));
        }
        _ => {}
    }

    let lock = state.job_lock.clone().try_write_owned().map_err(|_| {
        StartError::AlreadyRunning(
            state
                .job
                .try_lock()
                .ok()
                .and_then(|j| j.clone())
                .unwrap_or_else(unknown_running_job),
        )
    })?;

    let job = AdminJob {
        id: format!("{kind}-{}", now_ms()),
        kind: kind.to_string(),
        target: target.clone(),
        started_at: now_ms(),
        finished_at: None,
        status: "running".into(),
        error: None,
        note: None,
    };
    *state.job.lock().await = Some(job.clone());
    tracing::info!(kind, ?target, "[stats-admin] starting");

    let bg = state.clone();
    let kind_owned = kind.to_string();
    tokio::spawn(async move {
        let _guard = lock;
        let started = now_ms();
        let outcome = run_kind(&bg, &kind_owned, target.as_deref()).await;

        let mut slot = bg.job.lock().await;
        if let Some(j) = slot.as_mut() {
            j.finished_at = Some(now_ms());
            match outcome {
                Ok(note) => {
                    j.status = "done".into();
                    j.note = note;
                    tracing::info!(
                        kind = %kind_owned,
                        ms = now_ms() - started,
                        "[stats-admin] finished"
                    );
                }
                Err(e) => {
                    j.status = "failed".into();
                    j.error = Some(e.to_string());
                    tracing::error!(kind = %kind_owned, error = %e, "[stats-admin] FAILED");
                }
            }
        }
    });

    Ok(job)
}

fn unknown_running_job() -> AdminJob {
    AdminJob {
        id: "unknown".into(),
        kind: "unknown".into(),
        target: None,
        started_at: 0,
        finished_at: None,
        status: "running".into(),
        error: None,
        note: None,
    }
}

fn is_day(s: Option<&str>) -> bool {
    let Some(s) = s else { return false };
    s.len() == 10
        && s.as_bytes()[4] == b'-'
        && s.as_bytes()[7] == b'-'
        && s.bytes().enumerate().all(|(i, b)| {
            if i == 4 || i == 7 {
                b == b'-'
            } else {
                b.is_ascii_digit()
            }
        })
}

use stats::tz::{day_end_ms, day_start_ms};

const DAY_MS: i64 = 86_400_000;

async fn run_kind(state: &AppState, kind: &str, target: Option<&str>) -> Result<Option<String>> {
    match kind {
        "all" => {
            let mut redis = state.redis.lock().await;
            run_stats(
                &state.pool,
                &state.boundary_pool,
                &mut redis,
                StatsOptions {
                    force_init: true,
                    ..Default::default()
                },
                state.shutdown.clone(),
            )
            .await?;
            drop(redis);
            let threads = db::load_all_threads(&state.pool).await?;
            let mut redis = state.redis.lock().await;
            state.achievements.check(&state.pool, &mut redis, &threads).await?;
            Ok(None)
        }

        "day" => {
            let day = target.ok_or_else(|| anyhow!("missing day"))?;
            rebuild_day(state, day).await?;
            Ok(Some(format!("rebuilt {day}")))
        }

        "achievements" => {
            let threads = db::load_all_threads(&state.pool).await?;
            let mut redis = state.redis.lock().await;
            state.achievements.check(&state.pool, &mut redis, &threads).await?;
            Ok(None)
        }

        "counter" => {
            let uuid = target.ok_or_else(|| anyhow!("missing counter uuid"))?;
            let note = recalculate_counter(state, uuid).await?;
            Ok(Some(note))
        }

        "gaps" => {
            let (found, repaired) = backfill_missing_days(state, 30).await?;
            Ok(Some(format!("{found} found, {repaired} rebuilt")))
        }

        other => Err(anyhow!("unknown kind {other}")),
    }
}

async fn rebuild_day(state: &AppState, day: &str) -> Result<()> {
    let start = day_start_ms(day).ok_or_else(|| anyhow!("bad day {day}"))?;
    let end = day_end_ms(day).ok_or_else(|| anyhow!("bad day {day}"))?;
    let mut redis = state.redis.lock().await;
    run_stats(
        &state.pool,
        &state.boundary_pool,
        &mut redis,
        StatsOptions {
            force_init: true,
            target_thread: None,
            date_range: Some((start, end)),
        },
        state.shutdown.clone(),
    )
    .await
}

async fn recalculate_counter(state: &AppState, uuid: &str) -> Result<String> {
    use futures_util::TryStreamExt;

    let counter = db::find_counter_by_uuid(&state.pool, uuid)
        .await?
        .ok_or_else(|| anyhow!("No counter with uuid {uuid}"))?;

    let threads = db::load_all_threads(&state.pool).await?;
    let thread_by_uuid: ahash::AHashMap<String, stats::models::Thread> =
        threads.iter().map(|t| (t.uuid.clone(), t.clone())).collect();

    let mut stats = CounterStats::default();
    let mut processed: u64 = 0;
    let mut stream = db::stream_posts_by_author(&state.pool, counter.id);

    let mut chunk: Vec<Post> = Vec::with_capacity(10_000);
    while let Some(post) = stream.try_next().await? {
        chunk.push(post);
        if chunk.len() == 10_000 {
            let refs: Vec<&Post> = chunk.iter().collect();
            update_counter_stats(&mut stats, &refs, uuid, &thread_by_uuid);
            processed += chunk.len() as u64;
            chunk.clear();
        }
    }
    if !chunk.is_empty() {
        let refs: Vec<&Post> = chunk.iter().collect();
        update_counter_stats(&mut stats, &refs, uuid, &thread_by_uuid);
        processed += chunk.len() as u64;
    }

    let mut redis = state.redis.lock().await;
    redis_client::set_counter_stats(&mut redis, uuid, &stats, None).await?;

    Ok(format!("{} posts, {} threads", processed, stats.threads.len()))
}

pub(crate) async fn backfill_missing_days(state: &AppState, max_days: i64) -> Result<(usize, usize)> {
    let newest = db::get_newest_post_ms(&state.pool).await?;
    let Some(newest) = newest else {
        return Ok((0, 0));
    };

    let mut candidates: Vec<String> = Vec::new();
    {
        let mut redis = state.redis.lock().await;
        for back in 0..max_days {
            let day_ms = newest - back * DAY_MS;
            let day = stats::tz::ms_to_date_key(day_ms);
            let exists: bool = redis_client::day_stats_exist(&mut redis, &day).await?;
            if exists {
                continue;
            }
            let (Some(start), Some(end)) = (day_start_ms(&day), day_end_ms(&day)) else {
                continue;
            };
            if db::posts_exist_between(&state.pool, start, end).await? {
                candidates.push(day);
            }
        }
    }

    let found = candidates.len();
    let mut repaired = 0usize;
    for day in candidates {
        if state.shutdown.is_cancelled() {
            break;
        }
        match rebuild_day(state, &day).await {
            Ok(()) => {
                repaired += 1;
                tracing::info!(day, "[stats-admin] rebuilt missing day");
            }
            Err(e) => tracing::error!(day, error = %e, "[stats-admin] could not rebuild day"),
        }
    }
    Ok((found, repaired))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_day_accepts_iso_dates() {
        assert!(is_day(Some("2026-09-16")));
        assert!(is_day(Some("2023-02-22")));
    }

    #[test]
    fn is_day_rejects_everything_else() {
        assert!(!is_day(None));
        assert!(!is_day(Some("")));
        assert!(!is_day(Some("2026-9-16")), "must be zero padded");
        assert!(!is_day(Some("2026/09/16")), "separator must be a dash");
        assert!(!is_day(Some("2026-09-16T00:00")), "no trailing time");
        assert!(!is_day(Some("not-a-date")));
        assert!(!is_day(Some("20260916")));
    }

    #[test]
    fn kinds_match_what_the_api_offers() {
        assert_eq!(KINDS.len(), 5);
        for k in ["all", "day", "counter", "achievements", "gaps"] {
            assert!(KINDS.contains(&k), "{k} must stay supported");
        }
    }
}
