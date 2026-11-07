use ahash::AHashMap as HashMap;
use anyhow::Result;
use futures_util::TryStreamExt;
use rayon::prelude::*;
use redis::{AsyncCommands, aio::ConnectionManager};
use serde::Serialize;
use sqlx::MySqlPool;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{
    counter_stats::update_counter_stats,
    db::{
        get_last_mistake_post, get_max_post_id, get_max_post_id_before_ts, get_min_post_id_after_ts, load_all_threads,
        stream_posts_range,
    },
    models::{
        AccuracyEntry, AllStats, CounterStats, CounterThreadStats, Post, Thread, ThreadStats, combine_leaderboards,
        is_palindrome, is_repdigit, leaderboard_from_posts, uuid_parse_ms,
    },
    redis_client::{get_all_stats, get_counter_stats, get_last_updated_post_id, get_thread_stats},
    speed::{boundary_lookups_needed, compute_speed, compute_split_speed},
};

const DEFAULT_BATCH_SIZE: u64 = 200_000;

#[derive(Debug, Clone, Default)]
pub struct StatsOptions {
    pub force_init: bool,
    pub target_thread: Option<String>,
    pub date_range: Option<(i64, i64)>,
}

static SPEED_TAILS: std::sync::OnceLock<std::sync::Mutex<HashMap<String, Vec<Post>>>> = std::sync::OnceLock::new();

fn speed_tails() -> &'static std::sync::Mutex<HashMap<String, Vec<Post>>> {
    SPEED_TAILS.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

static LAST_SECONDS: std::sync::OnceLock<std::sync::Mutex<crate::day_time::LastSeconds>> = std::sync::OnceLock::new();

fn last_seconds_store() -> &'static std::sync::Mutex<crate::day_time::LastSeconds> {
    LAST_SECONDS.get_or_init(|| std::sync::Mutex::new(Default::default()))
}

fn take_last_seconds(opts: &StatsOptions) -> crate::day_time::LastSeconds {
    if opts.force_init || !continues_live(opts) {
        return Default::default();
    }
    std::mem::take(&mut *last_seconds_store().lock().unwrap_or_else(|e| e.into_inner()))
}

fn keep_last_seconds(opts: &StatsOptions, last: crate::day_time::LastSeconds) {
    if continues_live(opts) {
        *last_seconds_store().lock().unwrap_or_else(|e| e.into_inner()) = last;
    }
}

fn carry_tail<'a>(posts: impl DoubleEndedIterator<Item = &'a Post>, need: usize) -> Vec<Post> {
    let cap = need * 2 + 100;
    let mut tail: Vec<Post> = Vec::with_capacity(cap.min(4096));
    let mut valid_seen = 0usize;
    for p in posts.rev() {
        tail.push(p.clone());
        if p.is_valid_count == Some(true) {
            valid_seen += 1;
            if valid_seen > need {
                break;
            }
        }
        if tail.len() >= cap {
            break;
        }
    }
    tail.reverse();
    tail
}

fn continues_live(opts: &StatsOptions) -> bool {
    opts.target_thread.is_none() && opts.date_range.is_none()
}

fn take_speed_tails(opts: &StatsOptions) -> HashMap<String, Vec<Post>> {
    if opts.force_init || !continues_live(opts) {
        return HashMap::new();
    }
    std::mem::take(&mut *speed_tails().lock().unwrap_or_else(|e| e.into_inner()))
}

fn keep_speed_tails(opts: &StatsOptions, tails: HashMap<String, Vec<Post>>) {
    if continues_live(opts) {
        *speed_tails().lock().unwrap_or_else(|e| e.into_inner()) = tails;
    }
}

pub async fn run_stats(
    pool: &MySqlPool,
    boundary_pool: &MySqlPool,
    redis: &mut ConnectionManager,
    opts: StatsOptions,
    cancel: CancellationToken,
) -> Result<()> {
    let run_start = std::time::Instant::now();

    let all_threads = load_all_threads(pool).await?;
    let thread_by_uuid: HashMap<String, Thread> = all_threads.iter().map(|t| (t.uuid.clone(), t.clone())).collect();
    let thread_by_name: HashMap<String, Thread> = all_threads.iter().map(|t| (t.name.clone(), t.clone())).collect();
    let thread_ids: std::collections::HashMap<String, i32> =
        all_threads.iter().map(|t| (t.uuid.clone(), t.id)).collect();

    let scoped_thread: Option<&Thread> = opts.target_thread.as_deref().and_then(|name| thread_by_name.get(name));

    if opts.target_thread.is_some() && scoped_thread.is_none() {
        tracing::warn!(thread = ?opts.target_thread, "Target thread not found, skipping");
        return Ok(());
    }

    let (start_id, end_id) = resolve_id_range(pool, redis, &opts, scoped_thread).await?;
    if start_id > end_id {
        tracing::info!("No new posts to process (already up to date)");
        return Ok(());
    }

    let total_posts = end_id.saturating_sub(start_id) + 1;
    let batch_size = std::env::var("STATS_BATCH_SIZE")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_BATCH_SIZE);
    let total_batches = total_posts.div_ceil(batch_size);

    tracing::info!(
        start_id,
        end_id,
        total_posts,
        total_batches,
        batch_size,
        "Stats run starting"
    );

    let skip_global = scoped_thread.is_some();
    let day_scoped = opts.date_range.is_some();
    let date_key = None::<&str>;

    let mut all_stats: AllStats = if opts.force_init {
        AllStats::default()
    } else {
        get_all_stats(redis, date_key).await?.unwrap_or_default()
    };

    let threads_to_process: Vec<&Thread> = if let Some(t) = scoped_thread {
        vec![t]
    } else {
        all_threads.iter().collect()
    };

    let mut thread_stats: HashMap<String, ThreadStats> = HashMap::new();
    for thread in &threads_to_process {
        let existing = if opts.force_init {
            ThreadStats::default()
        } else {
            get_thread_stats(redis, &thread.name, date_key)
                .await?
                .unwrap_or_default()
        };
        thread_stats.insert(thread.name.clone(), existing);
    }

    let mut day_thread_stats: HashMap<String, HashMap<String, ThreadStats>> = HashMap::new();
    let mut day_all_stats: HashMap<String, AllStats> = HashMap::new();
    let mut flushed_days: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut flushed_all_days: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut lean_days: HashMap<String, HashMap<String, i64>> = HashMap::new();
    let mut thread_tails: HashMap<String, Vec<Post>> = take_speed_tails(&opts);

    let mut counter_days: HashMap<String, HashMap<String, i64>> = HashMap::new();
    let mut counter_time_days: HashMap<String, HashMap<String, i64>> = HashMap::new();
    let mut last_seconds = take_last_seconds(&opts);
    let mut coverage = crate::coverage::Coverage::new(opts.force_init && opts.date_range.is_none());
    let day_overwrite_ok = opts.force_init;
    let mut last_post_uuid = String::new();

    let mut counter_stats: HashMap<String, CounterStats> = HashMap::new();
    let mut dirty_counters: std::collections::HashSet<String> = std::collections::HashSet::new();
    const COUNTER_FLUSH_EVERY: u64 = 20;
    let mut total_processed: u64 = 0;
    let mut batch_num: u64 = 0;

    let mysql_readers: u64 = std::env::var("STATS_MYSQL_READERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let reader_batch = batch_size / mysql_readers;

    let (tx, mut rx) = mpsc::channel::<Result<(Vec<Post>, u64)>>(1);
    let prefetch_pool = pool.clone();
    let prefetch_cancel = cancel.clone();
    tokio::spawn(async move {
        let mut round_start = start_id;
        while round_start <= end_id {
            if prefetch_cancel.is_cancelled() {
                break;
            }
            let round_end = (round_start + batch_size - 1).min(end_id);
            let fetch_start = std::time::Instant::now();
            let mut handles = Vec::new();
            let mut seg_start = round_start;
            while seg_start <= round_end {
                let seg_end = (seg_start + reader_batch - 1).min(round_end);
                let p = prefetch_pool.clone();
                handles.push(tokio::spawn(async move {
                    stream_posts_range(&p, seg_start, seg_end)
                        .try_collect::<Vec<Post>>()
                        .await
                        .map_err(anyhow::Error::from)
                }));
                seg_start = seg_end + 1;
            }

            let mut combined: Vec<Post> = Vec::with_capacity(batch_size as usize);
            for h in handles {
                match h.await {
                    Ok(Ok(posts)) => combined.extend(posts),
                    Ok(Err(e)) => {
                        if prefetch_cancel.is_cancelled() {
                            return;
                        }
                        let _ = tx.send(Err(e)).await;
                        return;
                    }
                    Err(e) => {
                        if prefetch_cancel.is_cancelled() {
                            return;
                        }
                        let _ = tx.send(Err(anyhow::anyhow!("join error: {e}"))).await;
                        return;
                    }
                }
            }
            let t_sort = std::time::Instant::now();
            combined.sort_unstable_by_key(|p| p.id);
            let sort_ms = t_sort.elapsed().as_millis() as u64;
            let mysql_ms = fetch_start.elapsed().as_millis() as u64;
            tracing::info!(mysql_ms, sort_ms, rows = combined.len(), "Prefetch round complete");

            let done = combined.is_empty();
            let send_result = tokio::select! {
                r = tx.send(Ok((combined, mysql_ms))) => r,
                _ = prefetch_cancel.cancelled() => break,
            };
            if send_result.is_err() || done {
                break;
            }
            round_start = round_end + 1;
        }
    });

    while let Some(batch_result) = {
        let t_recv = std::time::Instant::now();
        let r = rx.recv().await;
        let recv_wait_ms = t_recv.elapsed().as_millis() as u64;
        if recv_wait_ms > 100 {
            tracing::info!(recv_wait_ms, "Waited for next batch from prefetch");
        }
        r
    } {
        let (posts, mysql_ms) = match batch_result {
            Ok((p, _)) if p.is_empty() => break,
            Ok((p, ms)) => (p, ms),
            Err(e) => return Err(e),
        };
        if let Some(last) = posts.last() {
            last_post_uuid = last.uuid.clone();
        }

        if cancel.is_cancelled() {
            tracing::info!(
                batches_done = batch_num,
                posts_processed = total_processed,
                "Cancellation requested — flushing and stopping cleanly"
            );
            if !day_scoped {
                flush_thread_stats(redis, &all_stats, &thread_stats, skip_global, date_key, true).await?;
                flush_dirty_counters(redis, &counter_stats, &dirty_counters, skip_global, date_key).await?;
            }
            keep_speed_tails(&opts, thread_tails);
            return Ok(());
        }

        batch_num += 1;
        let batch_wall = std::time::Instant::now();
        let batch_count = posts.len() as u64;
        total_processed += batch_count;

        let t_compute_start = std::time::Instant::now();

        let mut posts_by_thread: HashMap<&str, Vec<&Post>> = HashMap::new();
        for post in &posts {
            if let Some(thread) = thread_by_uuid.get(post.thread.as_str())
                && scoped_thread.map(|s| s.uuid == thread.uuid).unwrap_or(true)
            {
                posts_by_thread.entry(post.thread.as_str()).or_default().push(post);
            }
        }

        let mut all_boundary_lookups: Vec<(String, i64)> = Vec::new();
        let mut reset_thread_uuids: Vec<(String, i32)> = Vec::new();
        let mut thread_post_slices: Vec<(Thread, Vec<Post>, u64, Vec<Post>)> = Vec::new();
        let mut batch_lookbacks: HashMap<String, Vec<Post>> = HashMap::new();

        for (thread_uuid, thread_ref_posts) in &posts_by_thread {
            let thread = match thread_by_uuid.get(*thread_uuid) {
                Some(t) => t,
                None => continue,
            };
            let last_updated = thread_stats.get(&thread.name).map(|ts| ts.last_updated).unwrap_or(0);
            let thread_posts: Vec<Post> = thread_ref_posts
                .iter()
                .filter(|p| p.id > last_updated)
                .map(|p| (*p).clone())
                .collect();
            if thread_posts.is_empty() {
                continue;
            }

            let (gets, _, splits) = classify_posts(&thread_posts, thread);
            if !gets.is_empty() {
                all_boundary_lookups.extend(boundary_lookups_needed(&gets, thread, false));
            }
            if !splits.is_empty() {
                all_boundary_lookups.extend(boundary_lookups_needed(&splits, thread, true));
            }
            if thread.reset_on_mistakes {
                reset_thread_uuids.push((thread.uuid.clone(), thread.id));
            }

            let lookback = thread_tails.get(*thread_uuid).cloned().unwrap_or_default();
            batch_lookbacks.insert(thread.name.clone(), lookback.clone());

            let need = thread.get_size().max(1) as usize;
            thread_tails.insert(
                (*thread_uuid).to_string(),
                carry_tail(lookback.iter().chain(thread_posts.iter()), need),
            );

            thread_post_slices.push((thread.clone(), thread_posts, last_updated, lookback));
        }
        all_boundary_lookups.sort_unstable();
        all_boundary_lookups.dedup();

        let t_boundary_start = std::time::Instant::now();
        let (boundary_map, last_mistake_results) = tokio::join!(
            crate::db::get_boundary_posts_batch(boundary_pool, &all_boundary_lookups, &thread_ids),
            fetch_last_mistakes(boundary_pool, &reset_thread_uuids),
        );
        let boundary_map = boundary_map?;
        let last_mistake_map = last_mistake_results?;
        let boundary_ms = t_boundary_start.elapsed().as_millis() as u64;

        let t_rayon_start = std::time::Instant::now();
        let thread_deltas: Vec<(String, ThreadStats)> = thread_post_slices
            .into_par_iter()
            .map(|(thread, thread_posts, _last_updated, lookback)| {
                let mut ts = ThreadStats::default();

                let lb = leaderboard_from_posts(&thread_posts);
                ts.leaderboard = lb;

                let (gets, assists, splits) = classify_posts(&thread_posts, &thread);

                if is_palindrome_thread(&thread) {
                    let palins: Vec<Post> = thread_posts
                        .iter()
                        .filter(|p| {
                            p.is_valid_count == Some(true) && p.raw_count.as_deref().map(is_palindrome).unwrap_or(false)
                        })
                        .cloned()
                        .collect();
                    let repd: Vec<Post> = thread_posts
                        .iter()
                        .filter(|p| {
                            p.is_valid_count == Some(true) && p.raw_count.as_deref().map(is_repdigit).unwrap_or(false)
                        })
                        .cloned()
                        .collect();
                    if !palins.is_empty() {
                        ts.palindromes = Some(leaderboard_from_posts(&palins));
                    }
                    if !repd.is_empty() {
                        ts.repdigits = Some(leaderboard_from_posts(&repd));
                    }
                }

                if !gets.is_empty() {
                    ts.gets = Some(leaderboard_from_posts(&gets));
                }
                if !assists.is_empty() {
                    ts.assists = Some(leaderboard_from_posts(&assists));
                }

                let last_mistake = last_mistake_map.get(&thread.uuid).and_then(|o| o.as_ref());

                if !gets.is_empty() {
                    let needed = boundary_lookups_needed(&gets, &thread, false);
                    let local_map: HashMap<(String, i64), Post> = needed
                        .iter()
                        .filter_map(|k| boundary_map.get(k).map(|v| (k.clone(), v.clone())))
                        .collect();
                    let new_speed = compute_speed(&thread_posts, &lookback, &gets, &thread, &local_map, last_mistake);
                    if !new_speed.is_empty() {
                        ts.speed = Some(new_speed);
                    }
                }

                if !splits.is_empty() {
                    let needed = boundary_lookups_needed(&splits, &thread, true);
                    let local_map: HashMap<(String, i64), Post> = needed
                        .iter()
                        .filter_map(|k| boundary_map.get(k).map(|v| (k.clone(), v.clone())))
                        .collect();
                    let new_split =
                        compute_split_speed(&thread_posts, &lookback, &splits, &thread, &local_map, last_mistake);
                    if !new_split.is_empty() {
                        ts.split_speed = Some(new_split);
                    }
                }

                if let Some(last) = thread_posts.last() {
                    ts.last_updated = last.id;
                    ts.last_updated_uuid = last.uuid.clone();
                }

                (thread.name.clone(), ts)
            })
            .collect();

        let rayon_ms = t_rayon_start.elapsed().as_millis() as u64;

        let mut pb_deltas: HashMap<String, CounterStats> = HashMap::new();
        for (thread_name, delta) in &thread_deltas {
            if let Some(speed) = &delta.speed {
                for rec in speed {
                    for counter in &rec.qualified_counters {
                        let cs = pb_deltas.entry(counter.to_string()).or_default();
                        let entry = cs.threads.entry(thread_name.clone()).or_default();
                        if entry.best_speed_us.is_none_or(|b| rec.time < b) {
                            entry.best_speed_us = Some(rec.time);
                            entry.best_speed_start = Some(rec.start_count_number);
                            entry.best_speed_end = Some(rec.end_count_number);
                        }
                    }
                }
            }
            if let Some(splits) = &delta.split_speed {
                for rec in splits {
                    if rec.is_fake {
                        continue;
                    }
                    for counter in &rec.qualified_counters {
                        let cs = pb_deltas.entry(counter.to_string()).or_default();
                        let entry = cs.threads.entry(thread_name.clone()).or_default();
                        if entry.best_split_us.is_none_or(|b| rec.time < b) {
                            entry.best_split_us = Some(rec.time);
                            entry.best_split_start = Some(rec.start_count_number);
                            entry.best_split_end = Some(rec.end_count_number);
                        }
                    }
                }
            }
        }
        for (counter, delta) in pb_deltas {
            if !counter_stats.contains_key(&counter) {
                let base = if opts.force_init {
                    CounterStats::default()
                } else {
                    get_counter_stats(redis, &counter, date_key).await?.unwrap_or_default()
                };
                counter_stats.insert(counter.clone(), base);
            }
            let cs = counter_stats.get_mut(&counter).expect("just inserted above");
            crate::counter_stats::merge_counter_stats(cs, &delta);
            dirty_counters.insert(counter);
        }

        for (thread_name, delta) in thread_deltas {
            merge_thread_stats(thread_stats.entry(thread_name).or_default(), delta);
        }

        if !skip_global {
            let mut posts_by_day_thread: HashMap<String, HashMap<&str, Vec<&Post>>> = HashMap::new();
            let mut time_points: Vec<crate::day_time::Point> = Vec::with_capacity(posts.len());
            for post in &posts {
                let Some(ms) = uuid_parse_ms(&post.uuid) else { continue };
                time_points.push(crate::day_time::Point {
                    counter: &post.author_uuid,
                    thread: thread_by_uuid.get(post.thread.as_str()).map(|t| t.name.as_str()),
                    ms,
                });
                let day = ms_to_date_key(ms);
                if post.is_valid_count == Some(true) {
                    *counter_days
                        .entry(post.author_uuid.clone())
                        .or_default()
                        .entry(day.clone())
                        .or_insert(0) += 1;
                    coverage.observe(ms, &post.author_uuid);

                    if let Some(raw) = post.raw_count.as_deref()
                        && thread_by_uuid
                            .get(post.thread.as_str())
                            .map(|t| !t.allow_double_posts)
                            .unwrap_or(false)
                    {
                        coverage.observe_ending(raw, &post.author_uuid);
                    }
                }
                if let Some(thread) = thread_by_uuid.get(post.thread.as_str()) {
                    posts_by_day_thread
                        .entry(day)
                        .or_default()
                        .entry(thread.name.as_str())
                        .or_default()
                        .push(post);
                }
            }

            let mut day_lookbacks: HashMap<(String, String), Vec<Post>> = HashMap::new();
            {
                let mut days_by_thread: HashMap<&str, Vec<&String>> = HashMap::new();
                for (day, thread_map) in &posts_by_day_thread {
                    for name in thread_map.keys() {
                        days_by_thread.entry(*name).or_default().push(day);
                    }
                }
                for (name, mut days) in days_by_thread {
                    let Some(thread) = thread_by_name.get(name) else {
                        continue;
                    };
                    let need = thread.get_size().max(1) as usize;
                    days.sort();
                    let mut ctx = batch_lookbacks.get(name).cloned().unwrap_or_default();
                    for day in days {
                        let day_posts = &posts_by_day_thread[day][name];
                        let next = carry_tail(ctx.iter().chain(day_posts.iter().copied()), need);
                        day_lookbacks.insert((day.clone(), name.to_string()), std::mem::replace(&mut ctx, next));
                    }
                }
            }

            let day_thread_pairs: Vec<(String, String, Vec<Post>)> = posts_by_day_thread
                .iter()
                .flat_map(|(day, thread_map)| {
                    thread_map.iter().map(move |(thread_name, day_posts)| {
                        let owned: Vec<Post> = day_posts.iter().map(|p| (*p).clone()).collect();
                        (day.clone(), thread_name.to_string(), owned)
                    })
                })
                .collect();

            let day_deltas: Vec<(String, String, ThreadStats)> = day_thread_pairs
                .into_par_iter()
                .filter_map(|(day, thread_name, day_posts_owned)| {
                    let thread = thread_by_name.get(thread_name.as_str())?;
                    let mut day_ts = ThreadStats {
                        leaderboard: leaderboard_from_posts(&day_posts_owned),
                        ..Default::default()
                    };

                    let (day_gets, day_assists, day_splits) = classify_posts(&day_posts_owned, thread);

                    if !day_gets.is_empty() {
                        day_ts.gets = Some(leaderboard_from_posts(&day_gets));
                    }
                    if !day_assists.is_empty() {
                        day_ts.assists = Some(leaderboard_from_posts(&day_assists));
                    }
                    if is_palindrome_thread(thread) {
                        let palins: Vec<Post> = day_posts_owned
                            .iter()
                            .filter(|p| {
                                p.is_valid_count == Some(true)
                                    && p.raw_count.as_deref().map(is_palindrome).unwrap_or(false)
                            })
                            .cloned()
                            .collect();
                        let repd: Vec<Post> = day_posts_owned
                            .iter()
                            .filter(|p| {
                                p.is_valid_count == Some(true)
                                    && p.raw_count.as_deref().map(is_repdigit).unwrap_or(false)
                            })
                            .cloned()
                            .collect();
                        if !palins.is_empty() {
                            day_ts.palindromes = Some(leaderboard_from_posts(&palins));
                        }
                        if !repd.is_empty() {
                            day_ts.repdigits = Some(leaderboard_from_posts(&repd));
                        }
                    }
                    if !day_splits.is_empty() {
                        let needed = boundary_lookups_needed(&day_splits, thread, true);
                        let local_map: HashMap<(String, i64), Post> = needed
                            .iter()
                            .filter_map(|k| boundary_map.get(k).map(|v| (k.clone(), v.clone())))
                            .collect();
                        let day_lookback = day_lookbacks
                            .get(&(day.clone(), thread_name.clone()))
                            .map(|v| v.as_slice())
                            .unwrap_or(&[]);
                        let new_split =
                            compute_split_speed(&day_posts_owned, day_lookback, &day_splits, thread, &local_map, None);
                        if !new_split.is_empty() {
                            day_ts.split_speed = Some(new_split);
                        }
                    }
                    if !day_gets.is_empty() {
                        let needed = boundary_lookups_needed(&day_gets, thread, false);
                        let local_map: HashMap<(String, i64), Post> = needed
                            .iter()
                            .filter_map(|k| boundary_map.get(k).map(|v| (k.clone(), v.clone())))
                            .collect();
                        let day_lookback = day_lookbacks
                            .get(&(day.clone(), thread_name.clone()))
                            .map(|v| v.as_slice())
                            .unwrap_or(&[]);
                        let new_speed =
                            compute_speed(&day_posts_owned, day_lookback, &day_gets, thread, &local_map, None);
                        if !new_speed.is_empty() {
                            day_ts.speed = Some(new_speed);
                        }
                    }

                    Some((day, thread_name, day_ts))
                })
                .collect();

            for (day, thread_name, day_ts) in day_deltas {
                let day_total: i64 = day_ts.leaderboard.values().sum();
                if day_total > 0 {
                    *lean_days
                        .entry(thread_name.clone())
                        .or_default()
                        .entry(day.clone())
                        .or_insert(0) += day_total;
                }
                merge_thread_stats(
                    day_thread_stats
                        .entry(day.clone())
                        .or_default()
                        .entry(thread_name)
                        .or_default(),
                    day_ts,
                );
            }

            for ((counter, scope), days) in crate::day_time::tally(time_points, &mut last_seconds) {
                for (day, secs) in days {
                    if scope == crate::day_time::ALL {
                        *counter_time_days
                            .entry(counter.clone())
                            .or_default()
                            .entry(day.clone())
                            .or_insert(0) += secs;
                        *day_all_stats
                            .entry(day)
                            .or_default()
                            .time
                            .entry(counter.clone())
                            .or_insert(0) += secs;
                    } else {
                        *day_thread_stats
                            .entry(day)
                            .or_default()
                            .entry(scope.clone())
                            .or_default()
                            .time
                            .get_or_insert_with(HashMap::new)
                            .entry(counter.clone())
                            .or_insert(0) += secs;
                    }
                }
            }

            for (day, thread_map) in &posts_by_day_thread {
                let all_day_posts: Vec<Post> = thread_map.values().flatten().map(|p| (*p).clone()).collect();
                let day_lb = leaderboard_from_posts(&all_day_posts);
                combine_leaderboards(&mut day_all_stats.entry(day.clone()).or_default().leaderboard, &day_lb);
            }

            const MONTH_RETAIN: usize = 2;
            let mut months: Vec<String> = day_thread_stats
                .keys()
                .map(|d| d[..7].to_string())
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .collect();
            if months.len() > MONTH_RETAIN {
                months.sort_unstable();
                let evict_upto = months.len() - MONTH_RETAIN;
                for month in months.into_iter().take(evict_upto) {
                    let days: Vec<String> = day_thread_stats
                        .keys()
                        .filter(|d| d.starts_with(&month))
                        .cloned()
                        .collect();
                    let mut batch: HashMap<String, HashMap<String, ThreadStats>> = HashMap::new();
                    for day in days {
                        if let Some(map) = day_thread_stats.remove(&day) {
                            batch.insert(day.clone(), map);
                        }
                        if let Some(all) = day_all_stats.remove(&day) {
                            flush_day_all_stats(redis, &day, &all, day_overwrite_ok, &mut flushed_all_days).await?;
                        }
                    }
                    flush_month_thread_stats(redis, &month, &batch, day_overwrite_ok, &mut flushed_days).await?;
                }
            }
        }

        let compute_ms = t_compute_start.elapsed().as_millis() as u64;

        let t_counter_start = std::time::Instant::now();
        if !skip_global {
            let new_lb = leaderboard_from_posts(&posts);
            combine_leaderboards(&mut all_stats.leaderboard, &new_lb);

            let mut posts_by_author: HashMap<&str, Vec<&Post>> = HashMap::new();
            for post in &posts {
                posts_by_author.entry(post.author_uuid.as_str()).or_default().push(post);
            }

            let counter_deltas: Vec<(String, CounterStats)> = posts_by_author
                .par_iter()
                .map(|(author, author_posts)| {
                    let mut cs = CounterStats::default();
                    update_counter_stats(&mut cs, author_posts, author, &thread_by_uuid);
                    (author.to_string(), cs)
                })
                .collect();

            for (author, delta) in counter_deltas {
                if !counter_stats.contains_key(&author) {
                    let base = if opts.force_init {
                        CounterStats::default()
                    } else {
                        get_counter_stats(redis, &author, date_key).await?.unwrap_or_default()
                    };
                    counter_stats.insert(author.clone(), base);
                }
                let cs = counter_stats.get_mut(&author).expect("just inserted above");
                crate::counter_stats::merge_counter_stats(cs, &delta);
                dirty_counters.insert(author);
            }
        }
        let counter_ms = t_counter_start.elapsed().as_millis() as u64;

        let t_redis_start = std::time::Instant::now();
        if !day_scoped {
            flush_thread_stats(redis, &all_stats, &thread_stats, skip_global, date_key, false).await?;
            if batch_num.is_multiple_of(COUNTER_FLUSH_EVERY) {
                flush_dirty_counters(redis, &counter_stats, &dirty_counters, skip_global, date_key).await?;
                dirty_counters.clear();
            }
        }
        let redis_ms = t_redis_start.elapsed().as_millis() as u64;

        let elapsed = run_start.elapsed().as_secs_f64();
        let pct = total_processed as f64 / total_posts as f64 * 100.0;
        let posts_per_sec = if elapsed > 0.0 {
            total_processed as f64 / elapsed
        } else {
            0.0
        };
        let eta_secs = if posts_per_sec > 0.0 {
            (total_posts.saturating_sub(total_processed)) as f64 / posts_per_sec
        } else {
            0.0
        };

        tracing::info!(
            batch = batch_num,
            total_batches,
            pct = format_args!("{pct:.1}%"),
            posts_per_sec = format_args!("{posts_per_sec:.0}"),
            eta_secs = format_args!("{eta_secs:.0}"),
            mysql_ms,
            boundary_ms,
            rayon_ms,
            compute_ms,
            counter_ms,
            redis_ms,
            batch_ms = batch_wall.elapsed().as_millis(),
            "Batch complete"
        );
    }

    if !skip_global {
        all_stats.last_updated = end_id;
        all_stats.last_updated_uuid = last_post_uuid;
    }
    if !day_scoped {
        if !skip_global && date_key.is_none() {
            load_all_counters_for_ranking(redis, &thread_stats, &all_stats, &mut counter_stats).await?;
            let rank_lists = apply_thread_ranks(&mut counter_stats);
            build_accuracy_boards(&mut thread_stats, &counter_stats);
            flush_thread_ranklists(redis, &rank_lists).await?;
        }
        flush_thread_stats(redis, &all_stats, &thread_stats, skip_global, date_key, true).await?;
        flush_counter_stats(redis, &counter_stats, skip_global, date_key).await?;
    }

    if !skip_global {
        let remaining = day_thread_stats.len();
        let mut by_month: HashMap<String, HashMap<String, HashMap<String, ThreadStats>>> = HashMap::new();
        for (day, day_thread_map) in &day_thread_stats {
            by_month
                .entry(day[..7].to_string())
                .or_default()
                .insert(day.clone(), day_thread_map.clone());
        }
        for (month, batch) in &by_month {
            flush_month_thread_stats(redis, month, batch, day_overwrite_ok, &mut flushed_days).await?;
        }
        for (day, day_all) in &day_all_stats {
            flush_day_all_stats(redis, day, day_all, day_overwrite_ok, &mut flushed_all_days).await?;
        }
        tracing::info!(
            days_resident = remaining,
            days_total = flushed_days.len(),
            "Flushed per-day stats buckets"
        );

        flush_lean_index(redis, &thread_stats, &lean_days, opts.force_init, day_scoped).await?;
        let write = DayMapWrite::for_run(&opts);
        flush_counter_days(redis, "stats:counter_days", &counter_days, &write).await?;
        flush_counter_days(redis, "stats:counter_time_days", &counter_time_days, &write).await?;
        coverage.flush(redis).await?;
    }

    tracing::info!(
        elapsed_secs = format_args!("{:.1}", run_start.elapsed().as_secs_f64()),
        total_processed,
        "Stats run complete"
    );
    keep_speed_tails(&opts, thread_tails);
    keep_last_seconds(&opts, last_seconds);
    Ok(())
}

async fn flush_thread_stats(
    redis: &mut ConnectionManager,
    all_stats: &AllStats,
    thread_stats: &HashMap<String, ThreadStats>,
    skip_global: bool,
    date_key: Option<&str>,
    include_speed: bool,
) -> Result<()> {
    let mut pipe = redis::pipe();

    if !skip_global {
        let key = match date_key {
            Some(d) => format!("stats:all_day:{d}"),
            None => "stats:all".to_string(),
        };
        pipe.set(key, serde_json::to_string(all_stats)?).ignore();
    }

    for (thread_name, ts) in thread_stats {
        let (full_key, lean_key) = thread_keys(thread_name, date_key);
        let payload = if include_speed {
            crate::packed::encode(ts)?
        } else {
            crate::packed::encode(&ThreadStats {
                speed: None,
                split_speed: None,
                ..ts.clone()
            })?
        };
        pipe.set(full_key, payload).ignore();
        if date_key.is_none() {
            pipe.set(lean_key, serde_json::to_string(&ThreadStatsLean::from(ts))?)
                .ignore();
        }
    }

    pipe.query_async::<ConnectionManager, ()>(redis).await?;
    Ok(())
}

fn thread_keys(thread_name: &str, _date_key: Option<&str>) -> (String, String) {
    (
        format!("stats:thread:{thread_name}"),
        format!("stats:thread_lean:{thread_name}"),
    )
}

fn thread_ranklist_key(thread_name: &str, metric: &str) -> String {
    format!("stats:thread_ranklist:{thread_name}:{metric}")
}

#[derive(Serialize)]
struct RankListPayload<'a> {
    metric: &'a str,
    entries: &'a [(String, f64)],
}

async fn flush_thread_ranklists(
    redis: &mut ConnectionManager,
    lists: &[(String, String, Vec<(String, f64)>)],
) -> Result<()> {
    if lists.is_empty() {
        return Ok(());
    }

    let mut pipe = redis::pipe();
    for (thread_name, metric, entries) in lists {
        let payload = RankListPayload { metric, entries };
        pipe.set(
            thread_ranklist_key(thread_name, metric),
            serde_json::to_string(&payload)?,
        )
        .ignore();
    }
    pipe.query_async::<ConnectionManager, ()>(redis).await?;
    Ok(())
}

#[derive(Debug, PartialEq)]
enum DayMapWrite {
    Replace,
    ReplaceDays(String, String),
    Add,
}

impl DayMapWrite {
    fn for_run(opts: &StatsOptions) -> Self {
        match (opts.force_init, opts.date_range) {
            (true, None) => DayMapWrite::Replace,
            (true, Some((from, to))) => DayMapWrite::ReplaceDays(ms_to_date_key(from), ms_to_date_key(to - 1)),
            (false, _) => DayMapWrite::Add,
        }
    }

    fn merge(&self, stored: HashMap<String, i64>, run: &HashMap<String, i64>) -> HashMap<String, i64> {
        match self {
            DayMapWrite::Replace => run.clone(),
            DayMapWrite::ReplaceDays(from, to) => {
                let mut out: HashMap<String, i64> = stored
                    .into_iter()
                    .filter(|(day, _)| day.as_str() < from.as_str() || day.as_str() > to.as_str())
                    .collect();
                out.extend(run.iter().map(|(d, n)| (d.clone(), *n)));
                out
            }
            DayMapWrite::Add => {
                let mut out = stored;
                for (day, n) in run {
                    *out.entry(day.clone()).or_insert(0) += n;
                }
                out
            }
        }
    }
}

async fn flush_counter_days(
    redis: &mut ConnectionManager,
    prefix: &str,
    counter_days: &HashMap<String, HashMap<String, i64>>,
    write: &DayMapWrite,
) -> Result<()> {
    if counter_days.is_empty() {
        return Ok(());
    }

    let mut pipe = redis::pipe();
    for (counter_uuid, days) in counter_days {
        let key = format!("{prefix}:{counter_uuid}");

        let stored: HashMap<String, i64> = if *write == DayMapWrite::Replace {
            HashMap::new()
        } else {
            crate::redis_client::get_json(redis, &key).await?.unwrap_or_default()
        };
        let merged = write.merge(stored, days);

        pipe.set(key, serde_json::to_string(&merged)?).ignore();
    }
    pipe.query_async::<ConnectionManager, ()>(redis).await?;

    tracing::info!(prefix, counters = counter_days.len(), "Wrote per-day counter maps");
    Ok(())
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct LeanThreadTotals {
    pub all: i64,
    pub days: HashMap<String, i64>,
}

async fn flush_lean_index(
    redis: &mut ConnectionManager,
    thread_stats: &HashMap<String, ThreadStats>,
    lean_days: &HashMap<String, HashMap<String, i64>>,
    force_init: bool,
    day_scoped: bool,
) -> Result<()> {
    const KEY: &str = "stats:lean_index";

    let mut index: HashMap<String, LeanThreadTotals> = if force_init && !day_scoped {
        HashMap::new()
    } else {
        crate::redis_client::get_json(redis, KEY).await?.unwrap_or_default()
    };

    if !day_scoped {
        for (name, ts) in thread_stats {
            index.entry(name.clone()).or_default().all = ts.leaderboard.values().sum();
        }
    }
    for (name, days) in lean_days {
        let entry = index.entry(name.clone()).or_default();
        for (day, n) in days {
            if day_scoped {
                entry.days.insert(day.clone(), *n);
            } else {
                *entry.days.entry(day.clone()).or_insert(0) += n;
            }
        }
    }

    crate::redis_client::set_json(redis, KEY, &index).await?;
    tracing::info!(threads = index.len(), day_scoped, "Wrote lean stats index");
    Ok(())
}

pub fn merge_thread_stats(base: &mut ThreadStats, delta: ThreadStats) {
    combine_leaderboards(&mut base.leaderboard, &delta.leaderboard);
    if let Some(lb) = delta.gets {
        combine_leaderboards(base.gets.get_or_insert_with(HashMap::new), &lb);
    }
    if let Some(lb) = delta.assists {
        combine_leaderboards(base.assists.get_or_insert_with(HashMap::new), &lb);
    }
    if let Some(lb) = delta.palindromes {
        combine_leaderboards(base.palindromes.get_or_insert_with(HashMap::new), &lb);
    }
    if let Some(lb) = delta.repdigits {
        combine_leaderboards(base.repdigits.get_or_insert_with(HashMap::new), &lb);
    }
    if let Some(lb) = delta.time {
        combine_leaderboards(base.time.get_or_insert_with(HashMap::new), &lb);
    }
    if let Some(v) = delta.speed {
        base.speed.get_or_insert_with(Vec::new).extend(v);
    }
    if let Some(v) = delta.split_speed {
        let merged = base.split_speed.get_or_insert_with(Vec::new);
        merged.extend(v);
        crate::speed::prune_split_records(merged, crate::speed::SPLITS_KEPT_PER_COUNTER);
    }
    if delta.last_updated > base.last_updated {
        base.last_updated = delta.last_updated;
        base.last_updated_uuid = delta.last_updated_uuid;
    }
}

async fn flush_month_thread_stats(
    redis: &mut ConnectionManager,
    month: &str,
    batch: &HashMap<String, HashMap<String, ThreadStats>>,
    overwrite_ok: bool,
    flushed_months: &mut std::collections::HashSet<String>,
) -> Result<()> {
    if batch.is_empty() {
        return Ok(());
    }

    let mut by_thread: HashMap<String, std::collections::BTreeMap<u32, ThreadStats>> = HashMap::new();
    for (day, threads) in batch {
        let dom: u32 = match day.get(8..10).and_then(|d| d.parse().ok()) {
            Some(d) => d,
            None => continue,
        };
        for (thread_name, ts) in threads {
            by_thread
                .entry(thread_name.clone())
                .or_default()
                .insert(dom, ts.clone());
        }
    }

    let first_write = flushed_months.insert(month.to_string());
    let fresh = overwrite_ok && first_write;

    for (thread_name, mut days) in by_thread {
        let key = month_key(&thread_name, month);
        if !fresh {
            let existing: Option<Vec<u8>> = redis.get(&key).await?;
            if let Some(bytes) = existing
                && crate::packed::is_packed_month(&bytes)
            {
                match crate::packed::decode_month(&bytes) {
                    Ok(stored) => {
                        for (dom, ts) in stored {
                            match days.get_mut(&dom) {
                                Some(acc) => {
                                    let mut merged = ts;
                                    merge_thread_stats(&mut merged, acc.clone());
                                    *acc = merged;
                                }
                                None => {
                                    days.insert(dom, ts);
                                }
                            }
                        }
                    }
                    Err(e) => tracing::warn!(key, error = %e, "Unreadable month; replacing"),
                }
            }
        }
        let payload = crate::packed::encode_month(&days)?;
        redis.set::<_, _, ()>(&key, payload).await?;
    }
    Ok(())
}

fn month_key(thread_name: &str, month: &str) -> String {
    format!("stats:thread_month:{thread_name}:{month}")
}

async fn flush_day_all_stats(
    redis: &mut ConnectionManager,
    day: &str,
    day_all: &AllStats,
    overwrite_ok: bool,
    flushed_days: &mut std::collections::HashSet<String>,
) -> Result<()> {
    let key = format!("stats:all_day:{day}");
    let first_write = flushed_days.insert(day.to_string());
    let to_store = if overwrite_ok && first_write {
        day_all.clone()
    } else {
        let mut existing: AllStats = crate::redis_client::get_json(redis, &key).await?.unwrap_or_default();
        combine_leaderboards(&mut existing.leaderboard, &day_all.leaderboard);
        combine_leaderboards(&mut existing.time, &day_all.time);
        existing
    };
    redis.set::<_, _, ()>(key, serde_json::to_string(&to_store)?).await?;
    Ok(())
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ThreadStatsLean<'a> {
    leaderboard: &'a HashMap<String, i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    gets: Option<&'a HashMap<String, i64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    assists: Option<&'a HashMap<String, i64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    palindromes: Option<&'a HashMap<String, i64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    repdigits: Option<&'a HashMap<String, i64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    accuracy: Option<&'a Vec<AccuracyEntry>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    time: Option<&'a HashMap<String, i64>>,
    speed_count: usize,
    split_speed_count: usize,
    last_updated: u64,
    last_updated_uuid: &'a str,
}

impl<'a> From<&'a ThreadStats> for ThreadStatsLean<'a> {
    fn from(ts: &'a ThreadStats) -> Self {
        ThreadStatsLean {
            leaderboard: &ts.leaderboard,
            gets: ts.gets.as_ref(),
            assists: ts.assists.as_ref(),
            palindromes: ts.palindromes.as_ref(),
            repdigits: ts.repdigits.as_ref(),
            accuracy: ts.accuracy.as_ref(),
            time: ts.time.as_ref(),
            speed_count: ts.speed.as_ref().map_or(0, |v| v.len()),
            split_speed_count: ts.split_speed.as_ref().map_or(0, |v| v.len()),
            last_updated: ts.last_updated,
            last_updated_uuid: &ts.last_updated_uuid,
        }
    }
}

fn mirror_all_time_seconds(pipe: &mut redis::Pipeline, counter_uuid: &str, cs: &CounterStats, date_key: Option<&str>) {
    if date_key.is_some() {
        return;
    }
    let Some(all) = cs.threads.get("all") else {
        return;
    };
    if all.seconds_counted == 0 && all.seconds_posted == 0 {
        return;
    }
    pipe.cmd("HSET")
        .arg(crate::redis_client::SECONDS_BY_COUNTER_KEY)
        .arg(counter_uuid)
        .arg(all.seconds_counted)
        .ignore();
    pipe.cmd("HSET")
        .arg(crate::redis_client::POSTED_SECONDS_BY_COUNTER_KEY)
        .arg(counter_uuid)
        .arg(all.seconds_posted)
        .ignore();
}

const RANKED_METRICS: [(&str, bool); 7] = [
    ("counts", false),
    ("posts", false),
    ("mistakes", false),
    ("accuracy", false),
    ("speed", true),
    ("split", true),
    ("time", false),
];

fn rank_metric_value(t: &CounterThreadStats, metric: &str, prior: (f64, f64)) -> Option<f64> {
    match metric {
        "counts" => Some(t.counts as f64),
        "posts" => Some(t.posts as f64),
        "mistakes" => Some(t.mistakes as f64),
        "accuracy" => {
            let attempts = (t.counts + t.mistakes) as f64;
            if attempts <= 0.0 {
                return None;
            }
            let (alpha, beta) = prior;
            Some(accuracy_score(t.counts, t.counts + t.mistakes, alpha, beta))
        }
        "speed" => t.best_speed_us.map(|v| v as f64),
        "split" => t.best_split_us.map(|v| v as f64),
        "time" => (t.seconds_posted > 0).then_some(t.seconds_posted as f64),
        _ => None,
    }
}

fn rank_display_value(t: &CounterThreadStats, metric: &str) -> Option<f64> {
    match metric {
        "accuracy" => {
            let attempts = (t.counts + t.mistakes) as f64;
            if attempts <= 0.0 {
                return None;
            }
            Some(t.counts as f64 / attempts * 100.0)
        }
        _ => rank_metric_value(t, metric, (0.0, 0.0)),
    }
}

fn apply_thread_ranks(counter_stats: &mut HashMap<String, CounterStats>) -> Vec<(String, String, Vec<(String, f64)>)> {
    let mut rank_lists: Vec<(String, String, Vec<(String, f64)>)> = Vec::new();
    let mut members: HashMap<String, Vec<String>> = HashMap::new();
    for (counter_uuid, cs) in counter_stats.iter() {
        for thread_name in cs.threads.keys() {
            members
                .entry(thread_name.clone())
                .or_default()
                .push(counter_uuid.clone());
        }
    }

    for (thread_name, counters) in members {
        let rates: Vec<f64> = counters
            .iter()
            .filter_map(|u| counter_stats.get(u)?.threads.get(&thread_name))
            .filter(|t| t.counts + t.mistakes >= PRIOR_FIT_MIN_ATTEMPTS)
            .map(|t| t.counts as f64 / (t.counts + t.mistakes) as f64)
            .collect();
        let prior = fit_accuracy_prior(&rates);

        for (metric, lower_is_better) in RANKED_METRICS {
            let mut ordered: Vec<(&str, f64)> = counters
                .iter()
                .filter_map(|u| {
                    let t = counter_stats.get(u)?.threads.get(&thread_name)?;
                    rank_metric_value(t, metric, prior).map(|v| (u.as_str(), v))
                })
                .collect();

            ordered.sort_unstable_by(|a, b| {
                let ord = if lower_is_better {
                    a.1.partial_cmp(&b.1)
                } else {
                    b.1.partial_cmp(&a.1)
                };
                ord.unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.0.cmp(b.0))
            });

            let rows: Vec<(String, f64)> = ordered
                .iter()
                .map(|(u, _)| {
                    let shown = counter_stats
                        .get(*u)
                        .and_then(|cs| cs.threads.get(&thread_name))
                        .and_then(|t| rank_display_value(t, metric))
                        .unwrap_or(0.0);
                    ((*u).to_string(), shown)
                })
                .collect();
            rank_lists.push((thread_name.clone(), metric.to_string(), rows));

            let positions: Vec<(String, i64)> = ordered
                .into_iter()
                .enumerate()
                .map(|(i, (u, _))| (u.to_string(), i as i64 + 1))
                .collect();
            for (counter_uuid, position) in positions {
                if let Some(cs) = counter_stats.get_mut(&counter_uuid)
                    && let Some(entry) = cs.threads.get_mut(&thread_name)
                {
                    entry.ranks.insert(metric.to_string(), position);
                }
            }
        }
    }

    rank_lists
}

async fn load_all_counters_for_ranking(
    redis: &mut ConnectionManager,
    thread_stats: &HashMap<String, ThreadStats>,
    all_stats: &AllStats,
    counter_stats: &mut HashMap<String, CounterStats>,
) -> Result<()> {
    let mut wanted: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for ts in thread_stats.values() {
        wanted.extend(ts.leaderboard.keys().map(|k| k.as_str()));
    }
    wanted.extend(all_stats.leaderboard.keys().map(|k| k.as_str()));

    let missing: Vec<String> = wanted
        .into_iter()
        .filter(|u| !counter_stats.contains_key(*u))
        .map(|u| u.to_string())
        .collect();
    if missing.is_empty() {
        return Ok(());
    }

    let mut loaded = 0usize;
    for uuid in missing {
        if let Some(cs) = get_counter_stats(redis, &uuid, None).await? {
            counter_stats.insert(uuid, cs);
            loaded += 1;
        }
    }
    tracing::debug!(loaded, "Loaded counters not touched this run, for ranking");
    Ok(())
}

const PRIOR_FIT_MIN_ATTEMPTS: i64 = 50;
const CONFIDENCE_Z: f64 = 1.645;

fn accuracy_score(counts: i64, attempts: i64, alpha: f64, beta: f64) -> f64 {
    if attempts <= 0 {
        return 0.0;
    }
    let n = attempts as f64;
    let mean = (counts as f64 + alpha) / (n + alpha + beta);
    let variance = (mean * (1.0 - mean) / (n + alpha + beta + 1.0)).max(0.0);
    (mean - CONFIDENCE_Z * variance.sqrt()).clamp(0.0, 1.0)
}
const FALLBACK_PRIOR_STRENGTH: f64 = 50.0;

fn fit_accuracy_prior(rates: &[f64]) -> (f64, f64) {
    let n = rates.len() as f64;
    if n < 2.0 {
        return (FALLBACK_PRIOR_STRENGTH * 0.99, FALLBACK_PRIOR_STRENGTH * 0.01);
    }
    let mean = rates.iter().sum::<f64>() / n;
    let var = rates.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / n;

    if var <= 0.0 || var >= mean * (1.0 - mean) {
        return (
            FALLBACK_PRIOR_STRENGTH * mean.clamp(1e-6, 1.0 - 1e-6),
            FALLBACK_PRIOR_STRENGTH * (1.0 - mean).clamp(1e-6, 1.0 - 1e-6),
        );
    }
    let concentration = (mean * (1.0 - mean) / var - 1.0).clamp(1.0, 100_000.0);
    (mean * concentration, (1.0 - mean) * concentration)
}

fn build_accuracy_boards(
    thread_stats: &mut HashMap<String, ThreadStats>,
    counter_stats: &HashMap<String, CounterStats>,
) {
    let mut by_thread: HashMap<&str, Vec<(&str, i64, i64)>> = HashMap::new();
    for (counter_uuid, cs) in counter_stats {
        for (thread_name, t) in &cs.threads {
            if t.counts + t.mistakes > 0 {
                by_thread
                    .entry(thread_name.as_str())
                    .or_default()
                    .push((counter_uuid.as_str(), t.counts, t.mistakes));
            }
        }
    }

    for (thread_name, rows) in by_thread {
        let Some(ts) = thread_stats.get_mut(thread_name) else {
            continue;
        };

        let mut sample: Vec<f64> = rows
            .iter()
            .filter(|(_, c, m)| c + m >= PRIOR_FIT_MIN_ATTEMPTS)
            .map(|(_, c, m)| *c as f64 / (*c + *m) as f64)
            .collect();
        if sample.len() < 2 {
            sample = rows.iter().map(|(_, c, m)| *c as f64 / (*c + *m) as f64).collect();
        }
        let (alpha, beta) = fit_accuracy_prior(&sample);

        let mut board: Vec<AccuracyEntry> = rows
            .into_iter()
            .map(|(counter, counts, mistakes)| {
                let attempts = counts + mistakes;
                AccuracyEntry {
                    counter: counter.to_string(),
                    counts,
                    mistakes,
                    accuracy: counts as f64 / attempts as f64,
                    score: accuracy_score(counts, attempts, alpha, beta),
                }
            })
            .collect();

        board.sort_unstable_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.counts.cmp(&a.counts))
                .then_with(|| a.counter.cmp(&b.counter))
        });
        ts.accuracy = Some(board);
    }
}

async fn flush_dirty_counters(
    redis: &mut ConnectionManager,
    counter_stats: &HashMap<String, CounterStats>,
    dirty: &std::collections::HashSet<String>,
    skip_global: bool,
    date_key: Option<&str>,
) -> Result<()> {
    if skip_global || dirty.is_empty() {
        return Ok(());
    }
    let mut pipe = redis::pipe();
    for counter_uuid in dirty {
        if let Some(cs) = counter_stats.get(counter_uuid) {
            let key = match date_key {
                Some(d) => format!("stats:counter_day:{counter_uuid}:{d}"),
                None => format!("stats:counter:{counter_uuid}"),
            };
            pipe.set(key, serde_json::to_string(cs)?).ignore();
            mirror_all_time_seconds(&mut pipe, counter_uuid, cs, date_key);
        }
    }
    pipe.query_async::<ConnectionManager, ()>(redis).await?;
    Ok(())
}

async fn flush_counter_stats(
    redis: &mut ConnectionManager,
    counter_stats: &HashMap<String, CounterStats>,
    skip_global: bool,
    date_key: Option<&str>,
) -> Result<()> {
    if skip_global || counter_stats.is_empty() {
        return Ok(());
    }

    let mut pipe = redis::pipe();
    for (counter_uuid, cs) in counter_stats {
        let key = match date_key {
            Some(d) => format!("stats:counter_day:{counter_uuid}:{d}"),
            None => format!("stats:counter:{counter_uuid}"),
        };
        pipe.set(key, serde_json::to_string(cs)?).ignore();
        mirror_all_time_seconds(&mut pipe, counter_uuid, cs, date_key);
    }
    pipe.query_async::<ConnectionManager, ()>(redis).await?;
    Ok(())
}

pub use crate::tz::ms_to_date_key;

async fn resolve_id_range(
    pool: &MySqlPool,
    redis: &mut ConnectionManager,
    opts: &StatsOptions,
    _scoped_thread: Option<&Thread>,
) -> Result<(u64, u64)> {
    let end_id = if let Some((_, end_ts)) = opts.date_range {
        get_max_post_id_before_ts(pool, end_ts).await?
    } else {
        get_max_post_id(pool).await?
    };

    if end_id == 0 {
        return Ok((1, 0));
    }

    let start_id = if let Some((start_ts, _)) = opts.date_range {
        get_min_post_id_after_ts(pool, start_ts).await?.unwrap_or(end_id + 1)
    } else if opts.force_init {
        1u64
    } else {
        let last = get_last_updated_post_id(redis).await?;
        if last > 0 {
            tracing::info!(resume_from_id = last + 1, "Resuming from checkpoint");
        }
        last + 1
    };

    Ok((start_id, end_id))
}

fn classify_posts(posts: &[Post], thread: &Thread) -> (Vec<Post>, Vec<Post>, Vec<Post>) {
    let mut gets = Vec::new();
    let mut assists = Vec::new();
    let mut splits = Vec::new();
    for post in posts {
        if post.is_valid_count != Some(true) {
            continue;
        }
        let n = match thread.milestone_number(post) {
            Some(n) => n,
            None => continue,
        };
        if thread.is_get(n) {
            gets.push(post.clone());
        }
        if thread.is_split(n) {
            splits.push(post.clone());
        }
        if thread.is_get(n + 1) {
            assists.push(post.clone());
        }
    }
    (gets, assists, splits)
}

fn is_palindrome_thread(thread: &Thread) -> bool {
    matches!(
        thread.validation_type.as_str(),
        "main"
            | "slow"
            | "bars"
            | "yoco"
            | "roulette"
            | "tslc"
            | "randomhour"
            | "randomminute"
            | "waitx"
            | "1inx"
            | "parity"
            | "by3"
    )
}

async fn fetch_last_mistakes(pool: &MySqlPool, threads: &[(String, i32)]) -> Result<HashMap<String, Option<Post>>> {
    let handles: Vec<_> = threads
        .iter()
        .map(|(uuid, thread_id)| {
            let pool = pool.clone();
            let uuid = uuid.clone();
            let thread_id = *thread_id;
            tokio::spawn(async move {
                let post = get_last_mistake_post(&pool, thread_id).await?;
                Ok::<(String, Option<Post>), anyhow::Error>((uuid, post))
            })
        })
        .collect();

    let mut map = HashMap::new();
    for h in handles {
        let (uuid, post) = h.await??;
        map.insert(uuid, post);
    }
    Ok(map)
}

#[cfg(test)]
mod accuracy_tests {
    use super::*;
    use crate::models::CounterThreadStats;

    fn counter(counts: i64, mistakes: i64) -> CounterStats {
        let mut cs = CounterStats::default();
        cs.threads.insert(
            "main".to_string(),
            CounterThreadStats {
                counts,
                mistakes,
                ..Default::default()
            },
        );
        cs
    }

    fn board(rows: &[(&str, i64, i64)]) -> Vec<AccuracyEntry> {
        let mut thread_stats: HashMap<String, ThreadStats> = HashMap::new();
        thread_stats.insert("main".to_string(), ThreadStats::default());
        let mut counter_stats: HashMap<String, CounterStats> = HashMap::new();
        for (name, c, m) in rows {
            counter_stats.insert(name.to_string(), counter(*c, *m));
        }
        build_accuracy_boards(&mut thread_stats, &counter_stats);
        thread_stats["main"].accuracy.clone().unwrap()
    }

    #[test]
    fn a_tiny_perfect_record_does_not_beat_a_huge_near_perfect_one() {
        let rows = &[
            ("veteran", 544_500i64, 5_500i64),
            ("newcomer", 20, 0),
            ("midcomer", 110, 0),
            ("a", 90_000, 10_000),
            ("b", 40_000, 1_000),
            ("c", 9_000, 1_000),
            ("d", 5_000, 400),
            ("e", 700, 300),
            ("f", 300, 400),
            ("g", 2_000, 90),
        ];
        let b = board(rows);
        let pos = |who: &str| b.iter().position(|e| e.counter == who).unwrap();
        assert!(
            pos("veteran") < pos("newcomer"),
            "99% over 550k must outrank 100% over 20: {b:?}"
        );
        assert!(
            pos("veteran") < pos("midcomer"),
            "and must outrank 100% over 110, which is the case that slipped through: {b:?}"
        );

        let newcomer = &b[pos("newcomer")];
        assert!((newcomer.accuracy - 1.0).abs() < 1e-9);
        assert!(newcomer.score < 1.0, "the score is shrunk even though accuracy is 1.0");
        assert!(
            b[pos("midcomer")].score > newcomer.score,
            "110 perfect attempts must be worth more than 20"
        );
    }

    #[test]
    fn among_established_counters_the_better_record_still_wins() {
        let b = board(&[
            ("sharp", 99_000, 1_000),
            ("sloppy", 90_000, 10_000),
            ("mid", 95_000, 5_000),
            ("other", 80_000, 20_000),
        ]);
        let pos = |who: &str| b.iter().position(|e| e.counter == who).unwrap();
        assert!(pos("sharp") < pos("mid"));
        assert!(pos("mid") < pos("sloppy"));
        assert!(pos("sloppy") < pos("other"));
    }

    #[test]
    fn a_degenerate_distribution_falls_back_instead_of_exploding() {
        let (alpha, beta) = fit_accuracy_prior(&[0.99, 0.99, 0.99, 0.99]);
        assert!(alpha > 0.0 && beta > 0.0, "got alpha={alpha} beta={beta}");
        assert!(alpha.is_finite() && beta.is_finite());

        let (a2, b2) = fit_accuracy_prior(&[]);
        assert!(a2 > 0.0 && b2 > 0.0);
    }
}

#[cfg(test)]
mod rank_tests {
    use super::*;
    use crate::models::CounterThreadStats;

    fn entry(counts: i64, posts: i64, speed: Option<i64>, split: Option<i64>) -> CounterStats {
        let mut cs = CounterStats::default();
        cs.threads.insert(
            "main".to_string(),
            CounterThreadStats {
                counts,
                posts,
                best_speed_us: speed,
                best_split_us: split,
                ..Default::default()
            },
        );
        cs
    }

    #[test]
    fn each_metric_is_ranked_by_itself() {
        let mut cs: HashMap<String, CounterStats> = HashMap::new();
        cs.insert(
            "grinder".into(),
            entry(100_000, 120_000, Some(90_000_000), Some(9_000_000)),
        );
        cs.insert("sprinter".into(), entry(500, 600, Some(1_000_000), Some(100_000)));
        cs.insert(
            "middling".into(),
            entry(10_000, 12_000, Some(30_000_000), Some(3_000_000)),
        );

        apply_thread_ranks(&mut cs);
        let rank = |who: &str, metric: &str| cs[who].threads["main"].ranks[metric];

        assert_eq!(rank("grinder", "counts"), 1, "most counts is first on counts");
        assert_eq!(rank("sprinter", "counts"), 3);

        assert_eq!(rank("sprinter", "speed"), 1, "fastest get is first on speed");
        assert_eq!(rank("grinder", "speed"), 3, "slowest get is last on speed");
        assert_eq!(rank("sprinter", "split"), 1);
        assert_eq!(rank("grinder", "split"), 3);

        assert_eq!(rank("grinder", "posts"), 1);
    }

    #[test]
    fn a_counter_without_a_record_is_not_ranked_on_it() {
        let mut cs: HashMap<String, CounterStats> = HashMap::new();
        cs.insert("has_one".into(), entry(10, 10, Some(5_000_000), None));
        cs.insert("has_none".into(), entry(20, 20, None, None));

        apply_thread_ranks(&mut cs);
        let t = |who: &str| &cs[who].threads["main"].ranks;

        assert_eq!(t("has_one").get("speed"), Some(&1));
        assert_eq!(t("has_none").get("speed"), None, "never set a get time");
        assert_eq!(t("has_none").get("split"), None);
        assert_eq!(t("has_none").get("counts"), Some(&1));
        assert_eq!(t("has_one").get("counts"), Some(&2));
    }

    #[test]
    fn time_counting_ranks_by_seconds_posted() {
        let mut cs: HashMap<String, CounterStats> = HashMap::new();
        for (who, secs) in [("long", 7_200), ("short", 60), ("none", 0)] {
            let mut e = entry(10, 10, None, None);
            e.threads.get_mut("main").unwrap().seconds_posted = secs;
            cs.insert(who.into(), e);
        }

        let lists = apply_thread_ranks(&mut cs);
        let rank = |who: &str| cs[who].threads["main"].ranks.get("time").copied();
        assert_eq!(rank("long"), Some(1));
        assert_eq!(rank("short"), Some(2));
        assert_eq!(rank("none"), None, "no time spent, no place on the board");

        let (_, _, rows) = lists.iter().find(|(t, m, _)| t == "main" && m == "time").unwrap();
        assert_eq!(rows, &vec![("long".to_string(), 7_200.0), ("short".to_string(), 60.0)]);
    }
}

#[cfg(test)]
mod day_map_write_tests {
    use super::*;

    fn map(pairs: &[(&str, i64)]) -> HashMap<String, i64> {
        pairs.iter().map(|(d, n)| (d.to_string(), *n)).collect()
    }

    #[test]
    fn a_range_rebuild_keeps_days_outside_the_range() {
        let stored = map(&[("2026-01-01", 5), ("2026-01-10", 7), ("2026-02-01", 9)]);
        let run = map(&[("2026-01-10", 3)]);
        let w = DayMapWrite::ReplaceDays("2026-01-05".into(), "2026-01-20".into());
        assert_eq!(
            w.merge(stored, &run),
            map(&[("2026-01-01", 5), ("2026-01-10", 3), ("2026-02-01", 9)])
        );
    }

    #[test]
    fn a_range_rebuild_clears_in_range_days_it_found_nothing_on() {
        let stored = map(&[("2026-01-10", 7)]);
        let w = DayMapWrite::ReplaceDays("2026-01-05".into(), "2026-01-20".into());
        assert_eq!(w.merge(stored, &HashMap::new()), HashMap::new());
    }

    #[test]
    fn incremental_adds_and_full_rebuild_replaces() {
        let stored = map(&[("2026-01-10", 7)]);
        let run = map(&[("2026-01-10", 3)]);
        assert_eq!(DayMapWrite::Add.merge(stored.clone(), &run), map(&[("2026-01-10", 10)]));
        assert_eq!(DayMapWrite::Replace.merge(stored, &run), run);
    }

    #[test]
    fn range_bounds_are_inclusive_eastern_days() {
        let opts = StatsOptions {
            force_init: true,
            target_thread: None,
            date_range: Some((
                crate::tz::day_start_ms("2026-01-05").unwrap(),
                crate::tz::day_end_ms("2026-01-20").unwrap(),
            )),
        };
        assert_eq!(
            DayMapWrite::for_run(&opts),
            DayMapWrite::ReplaceDays("2026-01-05".into(), "2026-01-20".into())
        );
    }
}

#[cfg(test)]
mod speed_tail_tests {
    use super::*;

    fn opts(force_init: bool, thread: Option<&str>, range: Option<(i64, i64)>) -> StatsOptions {
        StatsOptions {
            force_init,
            target_thread: thread.map(String::from),
            date_range: range,
        }
    }

    #[test]
    fn live_runs_hand_their_tails_on_and_scoped_runs_leave_them_alone() {
        let live = opts(false, None, None);
        let mut tails = HashMap::new();
        tails.insert("thread-a".to_string(), Vec::new());
        keep_speed_tails(&live, tails);

        assert!(take_speed_tails(&opts(false, Some("main"), None)).is_empty());
        keep_speed_tails(&opts(false, Some("main"), None), HashMap::new());
        assert!(take_speed_tails(&opts(false, None, Some((0, 1)))).is_empty());

        let got = take_speed_tails(&live);
        assert!(got.contains_key("thread-a"), "tails survived to the next run");
        assert!(take_speed_tails(&live).is_empty());

        keep_speed_tails(&live, got);
        assert!(take_speed_tails(&opts(true, None, None)).is_empty());
        let mut rebuilt = HashMap::new();
        rebuilt.insert("thread-b".to_string(), Vec::new());
        keep_speed_tails(&opts(true, None, None), rebuilt);
        assert!(take_speed_tails(&live).contains_key("thread-b"));
    }
}
