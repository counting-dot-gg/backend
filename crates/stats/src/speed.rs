use ahash::AHashMap as HashMap;

use crate::models::{Post, SpeedEntry, SplitSpeedEntry, Thread, displayed_count, uuid_parse_to_nano};

const US_PER_MS: i64 = 1_000;
const MAX_SPEED_TIME_US: i64 = 600_000 * US_PER_MS;
const SELF_REPLY_THRESHOLD_NS: i64 = 60_000_000;
const FAKE_AVERAGE_THRESHOLD_NS: i64 = 80_000_000;
const MIN_STREAK_LENGTH: usize = 2;
const MIN_AUTHOR_POSTS_FOR_FAKE_CHECK: usize = 3;

pub fn compute_speed(
    all_thread_posts: &[Post],
    lookback: &[Post],
    gets: &[Post],
    thread: &Thread,
    boundary_posts: &HashMap<(String, i64), Post>,
    last_mistake_post: Option<&Post>,
) -> Vec<SpeedEntry> {
    let mut speed = Vec::new();
    let mut clipped = 0usize;
    let mut broken = 0usize;

    let (loaded, mistakes) = if thread.reset_on_mistakes {
        (
            loaded_posts(lookback, all_thread_posts),
            mistakes_in(lookback, all_thread_posts),
        )
    } else {
        (Vec::new(), Vec::new())
    };

    for (i, current_get) in gets.iter().enumerate() {
        let prev_get = if thread.reset_on_mistakes {
            match resolve_reset_boundary(
                current_get,
                thread.get_size(),
                thread,
                &loaded,
                &mistakes,
                boundary_posts,
                last_mistake_post,
            ) {
                Some(p) => p,
                None => {
                    broken += 1;
                    continue;
                }
            }
        } else if i == 0 {
            find_prev_boundary_for_get(current_get, thread, boundary_posts, last_mistake_post)
                .unwrap_or_else(|| make_fake_post(thread))
        } else {
            gets[i - 1].clone()
        };

        let window_loaded = earliest_available(lookback, all_thread_posts)
            .map(|earliest| prev_get.id >= earliest)
            .unwrap_or(false);
        if prev_get.id != 0 && !window_loaded {
            clipped += 1;
            continue;
        }

        let posts_between = collect_window(lookback, all_thread_posts, prev_get.id, current_get.id);

        let first_valid = match posts_between.iter().find(|p| p.is_valid_count == Some(true)) {
            Some(p) => p.clone(),
            None => continue,
        };

        let speed_time =
            (uuid_parse_to_nano(&posts_between.last().unwrap().uuid) - uuid_parse_to_nano(&first_valid.uuid)) / 1_000;

        if speed_time <= 0 || speed_time >= MAX_SPEED_TIME_US {
            continue;
        }

        let qualified = qualified_counters(&posts_between, Some(&first_valid.author_uuid));
        speed.push(SpeedEntry {
            start: first_valid.uuid.clone(),
            end: posts_between.last().unwrap().uuid.clone(),
            time: speed_time,
            start_count: first_valid.raw_count.clone().unwrap_or_default(),
            end_count: posts_between.last().unwrap().raw_count.clone().unwrap_or_default(),
            start_count_number: first_valid.valid_count_number.unwrap_or(0),
            end_count_number: posts_between.last().unwrap().valid_count_number.unwrap_or(0),
            qualified_counters: qualified,
        });
    }

    if clipped > 0 {
        tracing::debug!(
            thread = thread.name,
            clipped,
            "Get runs skipped: the batch did not hold the whole run"
        );
    }
    if broken > 0 {
        tracing::debug!(
            thread = thread.name,
            broken,
            "Get runs skipped: the window crossed a reset"
        );
    }
    speed
}

pub const SPLITS_KEPT_PER_COUNTER: usize = 100;

pub fn prune_split_records(records: &mut Vec<SplitSpeedEntry>, keep: usize) {
    let mut per_author: HashMap<(String, bool), usize> = HashMap::new();
    let over = records.iter().any(|r| {
        r.qualified_counters.iter().any(|a| {
            let n = per_author.entry((a.clone(), r.is_fake)).or_insert(0);
            *n += 1;
            *n > keep
        })
    });
    if !over {
        return;
    }
    records.sort_by_key(|r| r.time);
    per_author.clear();
    records.retain(|r| {
        let wanted = r
            .qualified_counters
            .iter()
            .any(|a| per_author.get(&(a.clone(), r.is_fake)).copied().unwrap_or(0) < keep);
        if wanted {
            for a in &r.qualified_counters {
                *per_author.entry((a.clone(), r.is_fake)).or_insert(0) += 1;
            }
        }
        wanted
    });
}

pub fn compute_split_speed(
    all_thread_posts: &[Post],
    lookback: &[Post],
    splits: &[Post],
    thread: &Thread,
    boundary_posts: &HashMap<(String, i64), Post>,
    last_mistake_post: Option<&Post>,
) -> Vec<SplitSpeedEntry> {
    let mut split_speed: Vec<SplitSpeedEntry> = Vec::new();
    let mut clipped = 0usize;

    let (loaded, mistakes) = if thread.reset_on_mistakes {
        (
            loaded_posts(lookback, all_thread_posts),
            mistakes_in(lookback, all_thread_posts),
        )
    } else {
        (Vec::new(), Vec::new())
    };
    let mut broken = 0usize;

    for (i, current_split) in splits.iter().enumerate() {
        let prev_split = if thread.reset_on_mistakes {
            match resolve_reset_boundary(
                current_split,
                thread.counts_per_split as i64,
                thread,
                &loaded,
                &mistakes,
                boundary_posts,
                last_mistake_post,
            ) {
                Some(p) => p,
                None => {
                    broken += 1;
                    continue;
                }
            }
        } else if i == 0 {
            find_prev_boundary_for_split(current_split, thread, boundary_posts, last_mistake_post)
                .unwrap_or_else(|| make_fake_post(thread))
        } else {
            splits[i - 1].clone()
        };

        let window_loaded = earliest_available(lookback, all_thread_posts)
            .map(|earliest| prev_split.id >= earliest)
            .unwrap_or(false);
        if prev_split.id != 0 && !window_loaded {
            clipped += 1;
            continue;
        }

        let posts_between = collect_window(lookback, all_thread_posts, prev_split.id, current_split.id);

        let first_valid = match posts_between.iter().find(|p| p.is_valid_count == Some(true)) {
            Some(p) => p.clone(),
            None => continue,
        };

        let speed_time =
            (uuid_parse_to_nano(&posts_between.last().unwrap().uuid) - uuid_parse_to_nano(&first_valid.uuid)) / 1_000;

        if speed_time <= 0 || speed_time >= thread.counts_per_split as i64 * MAX_SPEED_TIME_US {
            continue;
        }

        let fake_time_threshold = thread.counts_per_split as i64 * MAX_SPEED_TIME_US;
        let is_fake = if fake_time_threshold > 0 && speed_time < fake_time_threshold {
            detect_fake_split(&posts_between)
        } else {
            false
        };

        let qualified = qualified_counters(&posts_between, Some(&first_valid.author_uuid));

        {
            split_speed.push(SplitSpeedEntry {
                start: first_valid.uuid.clone(),
                end: posts_between.last().unwrap().uuid.clone(),
                time: speed_time,
                start_count: first_valid.raw_count.clone().unwrap_or_default(),
                end_count: posts_between.last().unwrap().raw_count.clone().unwrap_or_default(),
                start_count_number: first_valid.valid_count_number.unwrap_or(0),
                end_count_number: posts_between.last().unwrap().valid_count_number.unwrap_or(0),
                qualified_counters: qualified,
                is_fake,
            });
        }
    }

    prune_split_records(&mut split_speed, SPLITS_KEPT_PER_COUNTER);
    if clipped > 0 {
        tracing::debug!(
            thread = thread.name,
            clipped,
            "Split runs skipped: the batch did not hold the whole run"
        );
    }
    if broken > 0 {
        tracing::debug!(
            thread = thread.name,
            broken,
            "Split runs skipped: the window crossed a reset"
        );
    }
    split_speed
}

pub fn detect_fake_split(posts_between: &[Post]) -> bool {
    let mut times_by_author: HashMap<&str, Vec<i64>> = HashMap::new();

    for post in posts_between {
        if post.uuid.is_empty() || post.author_uuid.is_empty() {
            continue;
        }
        times_by_author
            .entry(&post.author_uuid)
            .or_default()
            .push(uuid_parse_to_nano(&post.uuid));
    }

    for times in times_by_author.values() {
        if times.len() < MIN_AUTHOR_POSTS_FOR_FAKE_CHECK {
            continue;
        }

        let gaps: Vec<i64> = times.windows(2).map(|w| w[1] - w[0]).collect();

        if has_suspicious_streak(&gaps) {
            return true;
        }
    }

    false
}

fn has_suspicious_streak(gaps: &[i64]) -> bool {
    let mut streak_start: Option<usize> = None;

    for (idx, &gap) in gaps.iter().enumerate() {
        if gap < SELF_REPLY_THRESHOLD_NS {
            if streak_start.is_none() {
                streak_start = Some(idx);
            }
        } else if let Some(start) = streak_start {
            let streak_len = idx - start;
            if streak_len >= MIN_STREAK_LENGTH && streak_fails_average_check(gaps, start, idx) {
                return true;
            }
            streak_start = None;
        }
    }

    if let Some(start) = streak_start {
        let streak_len = gaps.len() - start;
        if streak_len >= MIN_STREAK_LENGTH && streak_fails_average_check(gaps, start, gaps.len()) {
            return true;
        }
    }

    false
}

fn streak_fails_average_check(gaps: &[i64], streak_start: usize, streak_end: usize) -> bool {
    if streak_start == 0 {
        return true;
    }
    let mut include_start = streak_start - 1;
    loop {
        let window = &gaps[include_start..streak_end];
        let avg = window.iter().sum::<i64>() / window.len() as i64;
        if avg > FAKE_AVERAGE_THRESHOLD_NS {
            return false;
        }
        if include_start == 0 {
            break;
        }
        include_start -= 1;
    }
    true
}

fn collect_window(lookback: &[Post], batch: &[Post], from_id: u64, to_id: u64) -> Vec<Post> {
    let mut out = collect_posts_between(lookback, from_id, to_id);
    out.extend(collect_posts_between(batch, from_id, to_id));
    out
}

fn earliest_available(lookback: &[Post], batch: &[Post]) -> Option<u64> {
    lookback.first().or_else(|| batch.first()).map(|p| p.id)
}

fn collect_posts_between(all_posts: &[Post], from_id: u64, to_id: u64) -> Vec<Post> {
    if from_id > to_id {
        return Vec::new();
    }
    let lo = all_posts.partition_point(|p| p.id < from_id);
    let hi = all_posts.partition_point(|p| p.id <= to_id);
    if lo >= hi {
        return Vec::new();
    }
    all_posts[lo..hi].to_vec()
}

fn qualified_counters(posts: &[Post], first_valid_author: Option<&str>) -> Vec<String> {
    let mut counts: HashMap<&str, i64> = HashMap::new();
    for post in posts {
        *counts.entry(&post.author_uuid).or_insert(0) += 1;
    }
    if let Some(author) = first_valid_author
        && let Some(c) = counts.get_mut(author)
    {
        *c -= 1;
    }

    let top = counts.values().copied().max().unwrap_or(0);
    if top <= 0 {
        return vec![];
    }
    counts
        .into_iter()
        .filter(|(_, c)| *c * 4 >= top)
        .map(|(uuid, _)| uuid.to_string())
        .collect()
}

fn resolve_reset_boundary(
    current: &Post,
    size: i64,
    thread: &Thread,
    loaded: &[&Post],
    mistakes: &[&Post],
    boundary_posts: &HashMap<(String, i64), Post>,
    last_mistake_post: Option<&Post>,
) -> Option<Post> {
    let now = current.raw_count.as_deref().and_then(displayed_count)?;

    if now > size {
        let want = now - size;
        let target_vcn = current.valid_count_number? - size;
        let candidate = loaded
            .iter()
            .rev()
            .find(|p| p.valid_count_number == Some(target_vcn) && p.id < current.id)
            .map(|p| (*p).clone())
            .or_else(|| boundary_posts.get(&(thread.uuid.clone(), target_vcn)).cloned())?;
        let had = candidate.raw_count.as_deref().and_then(displayed_count)?;
        return if had == want { Some(candidate) } else { None };
    }

    if now == size {
        return mistakes
            .iter()
            .rev()
            .find(|m| m.id < current.id)
            .map(|m| (*m).clone())
            .or_else(|| last_mistake_post.filter(|m| m.id < current.id).cloned())
            .or_else(|| Some(make_fake_post(thread)));
    }

    None
}

fn mistakes_in<'a>(lookback: &'a [Post], all_thread_posts: &'a [Post]) -> Vec<&'a Post> {
    let mut out: Vec<&Post> = lookback
        .iter()
        .chain(all_thread_posts.iter())
        .filter(|p| p.is_count && p.is_valid_count == Some(false))
        .collect();
    out.sort_unstable_by_key(|p| p.id);
    out.dedup_by_key(|p| p.id);
    out
}

fn loaded_posts<'a>(lookback: &'a [Post], all_thread_posts: &'a [Post]) -> Vec<&'a Post> {
    let mut out: Vec<&Post> = lookback.iter().chain(all_thread_posts.iter()).collect();
    out.sort_unstable_by_key(|p| p.id);
    out.dedup_by_key(|p| p.id);
    out
}

fn find_prev_boundary_for_get(
    current_get: &Post,
    thread: &Thread,
    boundary_posts: &HashMap<(String, i64), Post>,
    last_mistake: Option<&Post>,
) -> Option<Post> {
    let vcn = current_get.valid_count_number?;
    let get_size = thread.get_size();
    if vcn > get_size {
        boundary_posts.get(&(thread.uuid.clone(), vcn - get_size)).cloned()
    } else if thread.reset_on_mistakes {
        last_mistake.cloned()
    } else {
        None
    }
}

fn find_prev_boundary_for_split(
    current_split: &Post,
    thread: &Thread,
    boundary_posts: &HashMap<(String, i64), Post>,
    last_mistake: Option<&Post>,
) -> Option<Post> {
    let vcn = current_split.valid_count_number?;
    let split_size = thread.counts_per_split as i64;
    if vcn > split_size {
        boundary_posts.get(&(thread.uuid.clone(), vcn - split_size)).cloned()
    } else if thread.reset_on_mistakes {
        last_mistake.cloned()
    } else {
        None
    }
}

fn make_fake_post(thread: &Thread) -> Post {
    Post {
        id: 0,
        uuid: String::new(),
        thread: thread.uuid.clone(),
        author_uuid: String::new(),
        timestamp: thread.created_at_ms.unwrap_or(0).to_string(),
        is_count: false,
        is_valid_count: None,
        valid_count_number: None,
        raw_count: None,
        has_comment: false,
    }
}

pub fn boundary_lookups_needed(gets: &[Post], thread: &Thread, is_split: bool) -> Vec<(String, i64)> {
    let step = if is_split {
        thread.counts_per_split as i64
    } else {
        thread.get_size()
    };

    let first = match gets.first() {
        Some(p) => p,
        None => return Vec::new(),
    };
    let milestone = match thread.milestone_number(first) {
        Some(n) => n,
        None => return Vec::new(),
    };
    if milestone <= step {
        return Vec::new();
    }
    match first.valid_count_number {
        Some(vcn) if vcn > step => vec![(thread.uuid.clone(), vcn - step)],
        _ => Vec::new(),
    }
}

#[cfg(test)]
pub fn qualified_counters_for_test(posts: &[Post], first_valid_author: Option<&str>) -> Vec<String> {
    qualified_counters(posts, first_valid_author)
}
