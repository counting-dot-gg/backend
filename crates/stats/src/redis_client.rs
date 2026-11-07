use anyhow::Result;
use redis::{AsyncCommands, Client, aio::ConnectionManager};
use serde::{Serialize, de::DeserializeOwned};

use crate::models::{AllStats, CounterStats, ThreadStats};

pub async fn create_connection_manager() -> Result<ConnectionManager> {
    let url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1/".into());
    let client = Client::open(url)?;
    let mgr = ConnectionManager::new(client).await?;
    Ok(mgr)
}

pub async fn set_json<T: Serialize>(conn: &mut ConnectionManager, key: &str, value: &T) -> Result<()> {
    let json = serde_json::to_string(value)?;
    conn.set::<_, _, ()>(key, json).await?;
    Ok(())
}

pub async fn get_json<T: DeserializeOwned>(conn: &mut ConnectionManager, key: &str) -> Result<Option<T>> {
    let raw: Option<String> = conn.get(key).await?;
    match raw {
        Some(s) => Ok(Some(serde_json::from_str(&s)?)),
        None => Ok(None),
    }
}

pub async fn set_thread_stats(
    conn: &mut ConnectionManager,
    thread_name: &str,
    stats: &ThreadStats,
    date: Option<&str>,
) -> Result<()> {
    let key = match date {
        Some(d) => format!("stats:thread_day:{}:{}", thread_name, d),
        None => format!("stats:thread:{}", thread_name),
    };
    set_json(conn, &key, stats).await
}

pub async fn get_thread_stats(
    conn: &mut ConnectionManager,
    thread_name: &str,
    date: Option<&str>,
) -> Result<Option<ThreadStats>> {
    let key = match date {
        Some(d) => format!("stats:thread_day:{}:{}", thread_name, d),
        None => format!("stats:thread:{}", thread_name),
    };
    let raw: Option<Vec<u8>> = conn.get(&key).await?;
    let Some(raw) = raw else { return Ok(None) };

    if crate::packed::is_packed(&raw) {
        return Ok(Some(crate::packed::decode(&raw)?));
    }
    match serde_json::from_slice::<ThreadStats>(&raw) {
        Ok(v) => Ok(Some(v)),
        Err(e) => {
            tracing::warn!(key, error = %e, "Unreadable thread stats; treating as absent");
            Ok(None)
        }
    }
}

pub async fn set_counter_stats(
    conn: &mut ConnectionManager,
    counter_uuid: &str,
    stats: &CounterStats,
    date: Option<&str>,
) -> Result<()> {
    let key = match date {
        Some(d) => format!("stats:counter_day:{}:{}", counter_uuid, d),
        None => format!("stats:counter:{}", counter_uuid),
    };
    set_json(conn, &key, stats).await?;

    if date.is_none()
        && let Some(all) = stats.threads.get("all")
        && (all.seconds_counted > 0 || all.seconds_posted > 0)
    {
        let mut pipe = redis::pipe();
        pipe.cmd("HSET")
            .arg(SECONDS_BY_COUNTER_KEY)
            .arg(counter_uuid)
            .arg(all.seconds_counted)
            .ignore();
        pipe.cmd("HSET")
            .arg(POSTED_SECONDS_BY_COUNTER_KEY)
            .arg(counter_uuid)
            .arg(all.seconds_posted)
            .ignore();
        pipe.query_async::<ConnectionManager, ()>(conn).await?;
    }
    Ok(())
}

pub const SECONDS_BY_COUNTER_KEY: &str = "stats:seconds_by_counter";

pub const POSTED_SECONDS_BY_COUNTER_KEY: &str = "stats:posted_seconds_by_counter";

pub async fn get_counter_stats(
    conn: &mut ConnectionManager,
    counter_uuid: &str,
    date: Option<&str>,
) -> Result<Option<CounterStats>> {
    let key = match date {
        Some(d) => format!("stats:counter_day:{}:{}", counter_uuid, d),
        None => format!("stats:counter:{}", counter_uuid),
    };
    get_json(conn, &key).await
}

pub async fn set_all_stats(conn: &mut ConnectionManager, stats: &AllStats, date: Option<&str>) -> Result<()> {
    let key = match date {
        Some(d) => format!("stats:all_day:{}", d),
        None => "stats:all".to_string(),
    };
    set_json(conn, &key, stats).await
}

pub async fn get_all_stats(conn: &mut ConnectionManager, date: Option<&str>) -> Result<Option<AllStats>> {
    let key = match date {
        Some(d) => format!("stats:all_day:{}", d),
        None => "stats:all".to_string(),
    };
    get_json(conn, &key).await
}

pub async fn get_last_updated_post_id(conn: &mut ConnectionManager) -> Result<u64> {
    let stats: Option<AllStats> = get_all_stats(conn, None).await?;
    Ok(stats.map(|s| s.last_updated).unwrap_or(0))
}

pub async fn day_stats_exist(conn: &mut ConnectionManager, day: &str) -> Result<bool> {
    let n: i64 = conn.exists(format!("stats:all_day:{day}")).await?;
    Ok(n == 1)
}
