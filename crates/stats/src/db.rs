use anyhow::Result;
use futures_util::TryStreamExt;
use sqlx::{FromRow, MySqlPool};
use std::collections::HashMap;

use crate::models::{Counter, Post, Thread, uuid_parse_ms};

#[derive(FromRow)]
struct ThreadRow {
    id: i32,
    uuid: String,
    name: String,
    #[sqlx(rename = "validationType")]
    validation_type: String,
    #[sqlx(rename = "resetOnMistakes")]
    reset_on_mistakes: bool,
    #[sqlx(rename = "firstValidCount")]
    first_valid_count: String,
    #[sqlx(rename = "countsPerSplit")]
    counts_per_split: i32,
    #[sqlx(rename = "splitsPerGet")]
    splits_per_get: i32,
    #[sqlx(rename = "allowDoublePosts")]
    allow_double_posts: bool,
    #[sqlx(rename = "splitOffset")]
    split_offset: i32,
}

#[derive(FromRow)]
struct PostRow {
    id: i64,
    uuid: String,
    thread: String,
    #[sqlx(rename = "authorUUID")]
    author_uuid: String,
    timestamp: String,
    #[sqlx(rename = "isCount")]
    is_count: bool,
    #[sqlx(rename = "isValidCount")]
    is_valid_count: Option<bool>,
    #[sqlx(rename = "validCountNumber")]
    valid_count_number: Option<i64>,
    #[sqlx(rename = "rawCount")]
    raw_count: Option<String>,
    #[sqlx(rename = "hasComment")]
    has_comment: bool,
}

impl From<PostRow> for Post {
    fn from(r: PostRow) -> Self {
        Post {
            id: r.id as u64,
            uuid: r.uuid,
            thread: r.thread,
            author_uuid: r.author_uuid,
            timestamp: r.timestamp,
            is_count: r.is_count,
            is_valid_count: r.is_valid_count,
            valid_count_number: r.valid_count_number,
            raw_count: r.raw_count,
            has_comment: r.has_comment,
        }
    }
}

#[derive(FromRow)]
struct MaxIdRow {
    max_id: Option<i64>,
}

#[derive(FromRow)]
struct CounterRow {
    id: i32,
    uuid: String,
    username: String,
}

pub async fn create_pool() -> Result<MySqlPool> {
    create_pool_with_max(10).await
}

pub async fn create_boundary_pool() -> Result<MySqlPool> {
    create_pool_with_max(4).await
}

async fn create_pool_with_max(max_connections: u32) -> Result<MySqlPool> {
    cgg_core::MysqlConfig::from_env()?.pool(max_connections).await
}

pub async fn load_all_threads(pool: &MySqlPool) -> Result<Vec<Thread>> {
    let rows: Vec<ThreadRow> = sqlx::query_as(
        "SELECT id, uuid, name, validationType, resetOnMistakes, firstValidCount,
                countsPerSplit, splitsPerGet, splitOffset, allowDoublePosts
         FROM thread",
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| {
            let created_at_ms = uuid_parse_ms(&r.uuid);
            Thread {
                id: r.id,
                uuid: r.uuid,
                name: r.name,
                validation_type: r.validation_type,
                reset_on_mistakes: r.reset_on_mistakes,
                first_valid_count: r.first_valid_count,
                counts_per_split: r.counts_per_split,
                splits_per_get: r.splits_per_get,
                split_offset: r.split_offset,
                allow_double_posts: r.allow_double_posts,
                created_at_ms,
            }
        })
        .collect())
}

pub fn thread_by_uuid(threads: &[Thread]) -> HashMap<String, Thread> {
    threads.iter().map(|t| (t.uuid.clone(), t.clone())).collect()
}

pub fn thread_by_name(threads: &[Thread]) -> HashMap<String, Thread> {
    threads.iter().map(|t| (t.name.clone(), t.clone())).collect()
}

#[derive(FromRow)]
struct IdRow {
    id: i64,
}

pub async fn get_max_post_id(pool: &MySqlPool) -> Result<u64> {
    let row: MaxIdRow = sqlx::query_as("SELECT MAX(id) AS max_id FROM post")
        .fetch_one(pool)
        .await?;
    Ok(row.max_id.unwrap_or(0) as u64)
}

pub(crate) fn ts_ms_key(ts_ms: i64) -> String {
    format!("{:013}", ts_ms.max(0))
}

pub async fn get_max_post_id_before_ts(pool: &MySqlPool, ts_ms: i64) -> Result<u64> {
    let row: Option<IdRow> = sqlx::query_as("SELECT id FROM post WHERE timestamp < ? ORDER BY timestamp DESC LIMIT 1")
        .bind(ts_ms_key(ts_ms))
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| r.id as u64).unwrap_or(0))
}

pub async fn get_min_post_id_after_ts(pool: &MySqlPool, ts_ms: i64) -> Result<Option<u64>> {
    let row: Option<IdRow> = sqlx::query_as("SELECT id FROM post WHERE timestamp >= ? ORDER BY timestamp ASC LIMIT 1")
        .bind(ts_ms_key(ts_ms))
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| r.id as u64))
}

pub fn stream_posts_range(
    pool: &MySqlPool,
    start_id: u64,
    end_id: u64,
) -> impl futures_util::Stream<Item = Result<Post, sqlx::Error>> + '_ {
    sqlx::query_as::<_, PostRow>(
        "SELECT id, uuid, thread, authorUUID, timestamp, isCount, isValidCount, validCountNumber, rawCount, hasComment \
         FROM post WHERE id >= ? AND id <= ? ORDER BY id ASC",
    )
    .bind(start_id as i64)
    .bind(end_id as i64)
    .fetch(pool)
    .map_ok(Post::from)
}

pub fn stream_posts_by_author<'a>(
    pool: &'a MySqlPool,
    author_id: i32,
) -> impl futures_util::Stream<Item = Result<Post, sqlx::Error>> + 'a {
    sqlx::query_as::<_, PostRow>(
        "SELECT id, uuid, thread, authorUUID, timestamp, isCount, isValidCount, validCountNumber, rawCount, hasComment \
         FROM post WHERE authorId = ? ORDER BY id ASC",
    )
    .bind(author_id)
    .fetch(pool)
    .map_ok(Post::from)
}

pub async fn get_boundary_posts_batch(
    pool: &MySqlPool,
    lookups: &[(String, i64)],
    thread_ids: &HashMap<String, i32>,
) -> Result<HashMap<(String, i64), Post>> {
    if lookups.is_empty() {
        return Ok(HashMap::new());
    }

    let mut by_thread: HashMap<&str, Vec<i64>> = HashMap::new();
    for (thread_uuid, vcn) in lookups {
        by_thread.entry(thread_uuid.as_str()).or_default().push(*vcn);
    }

    let mut map = HashMap::new();
    let handles: Vec<_> = by_thread
        .into_iter()
        .filter_map(|(thread_uuid, vcns)| {
            let Some(&thread_id) = thread_ids.get(thread_uuid) else {
                tracing::warn!(thread = thread_uuid, "No thread id; skipping boundary lookups");
                return None;
            };
            let pool = pool.clone();
            Some(tokio::spawn(async move {
                let placeholders = vcns.iter().map(|_| "?").collect::<Vec<_>>().join(",");
                let sql = format!(
                    "SELECT id, uuid, thread, authorUUID, timestamp, isCount, isValidCount, \
                     validCountNumber, rawCount, hasComment \
                     FROM post WHERE threadId = ? AND isValidCount = 1 \
                     AND validCountNumber IN ({placeholders})"
                );
                let mut q = sqlx::query_as::<_, PostRow>(&sql).bind(thread_id);
                for vcn in &vcns {
                    q = q.bind(vcn);
                }
                let rows: Vec<PostRow> = q.fetch_all(&pool).await?;
                Ok::<Vec<Post>, anyhow::Error>(rows.into_iter().map(Post::from).collect())
            }))
        })
        .collect();

    for handle in handles {
        let posts = handle.await??;
        for post in posts {
            if let Some(vcn) = post.valid_count_number {
                map.insert((post.thread.clone(), vcn), post);
            }
        }
    }
    Ok(map)
}

pub async fn get_last_mistake_post(pool: &MySqlPool, thread_id: i32) -> Result<Option<Post>> {
    let row: Option<PostRow> = sqlx::query_as(
        "SELECT id, uuid, thread, authorUUID, timestamp, isCount, isValidCount, validCountNumber, rawCount, hasComment \
         FROM post WHERE threadId = ? AND isCount = 1 AND isValidCount = 0 ORDER BY id DESC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(Post::from))
}

pub async fn get_newest_post_ms(pool: &MySqlPool) -> Result<Option<i64>> {
    let row: Option<(String,)> = sqlx::query_as("SELECT timestamp FROM post ORDER BY timestamp DESC LIMIT 1")
        .fetch_optional(pool)
        .await?;
    Ok(row.and_then(|(ts,)| ts.split('.').next().and_then(|w| w.parse::<i64>().ok())))
}

pub async fn posts_exist_between(pool: &MySqlPool, from_ms: i64, to_ms: i64) -> Result<bool> {
    let row: Option<IdRow> = sqlx::query_as("SELECT id FROM post WHERE timestamp >= ? AND timestamp < ? LIMIT 1")
        .bind(ts_ms_key(from_ms))
        .bind(ts_ms_key(to_ms))
        .fetch_optional(pool)
        .await?;
    Ok(row.is_some())
}

pub async fn find_counter_by_uuid(pool: &MySqlPool, uuid: &str) -> Result<Option<Counter>> {
    let row: Option<CounterRow> = sqlx::query_as("SELECT id, uuid, username FROM counters WHERE uuid = ? LIMIT 1")
        .bind(uuid)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| Counter {
        id: r.id,
        uuid: r.uuid,
        username: r.username,
    }))
}
