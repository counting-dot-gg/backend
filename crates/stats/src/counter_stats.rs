use ahash::AHashMap as HashMap;

use crate::models::{CounterStats, Post};

pub fn update_counter_stats(
    stats: &mut CounterStats,
    posts: &[&Post],
    _counter_uuid: &str,
    thread_by_uuid: &HashMap<String, crate::models::Thread>,
) {
    let counter_posts: &[&Post] = posts;

    if counter_posts.is_empty() {
        return;
    }

    let mut by_thread: HashMap<&str, Vec<&Post>> = HashMap::new();
    for &post in counter_posts {
        by_thread.entry(post.thread.as_str()).or_default().push(post);
    }

    fn tally_seconds(entry: &mut crate::models::CounterThreadStats, posts: &[&Post]) {
        tally_posted_seconds(entry, posts);
        let mut secs: Vec<i64> = posts
            .iter()
            .filter(|p| p.is_valid_count == Some(true))
            .filter_map(|p| crate::models::uuid_parse_ms(&p.uuid))
            .map(|ms| ms / 1000)
            .collect();
        if secs.is_empty() {
            return;
        }
        secs.sort_unstable();
        secs.dedup();

        entry.seconds_counted = secs.len() as i64;
        entry.first_second = secs[0];
        entry.last_second = secs[secs.len() - 1];
    }

    fn tally_posted_seconds(entry: &mut crate::models::CounterThreadStats, posts: &[&Post]) {
        let mut secs: Vec<i64> = posts
            .iter()
            .filter_map(|p| crate::models::uuid_parse_ms(&p.uuid))
            .map(|ms| ms / 1000)
            .collect();
        if secs.is_empty() {
            return;
        }
        secs.sort_unstable();
        secs.dedup();

        entry.seconds_posted = secs.len() as i64;
        entry.first_posted_second = secs[0];
        entry.last_posted_second = secs[secs.len() - 1];
    }

    {
        let entry = stats.threads.entry("all".to_string()).or_default();
        tally_seconds(entry, counter_posts);
        for &post in counter_posts {
            entry.posts += 1;
            if post.is_count {
                if post.is_valid_count == Some(true) {
                    entry.counts += 1;
                    if let Some(raw) = &post.raw_count
                        && let Ok(n) = raw.parse::<i64>()
                    {
                        if n % 2 == 1 {
                            entry.odds += 1;
                        } else {
                            entry.evens += 1;
                        }
                    }
                } else {
                    entry.mistakes += 1;
                }
            }
            if entry.first_count.is_none() && post.is_count && post.is_valid_count == Some(true) {
                let thread_name = thread_by_uuid
                    .get(&post.thread)
                    .map(|t| t.name.as_str())
                    .unwrap_or("unknown");
                entry.first_count = Some(format!("{}/{}", thread_name, post.uuid));
            }
        }
    }

    for (thread_uuid, thread_posts) in &by_thread {
        let thread_name = match thread_by_uuid.get(*thread_uuid) {
            Some(t) => &t.name,
            None => continue,
        };
        let entry = stats.threads.entry(thread_name.clone()).or_default();
        tally_seconds(entry, thread_posts);
        for &post in thread_posts {
            entry.posts += 1;
            if post.is_count {
                if post.is_valid_count == Some(true) {
                    entry.counts += 1;
                    if let Some(raw) = &post.raw_count
                        && let Ok(n) = raw.parse::<i64>()
                    {
                        if n % 2 == 1 {
                            entry.odds += 1;
                        } else {
                            entry.evens += 1;
                        }
                    }
                } else {
                    entry.mistakes += 1;
                }
            }
            if entry.first_count.is_none() && post.is_count && post.is_valid_count == Some(true) {
                entry.first_count = Some(format!("{}/{}", thread_name, post.uuid));
            }
        }
    }
}

fn take_faster(
    best: &mut Option<i64>,
    start: &mut Option<i64>,
    end: &mut Option<i64>,
    inc: Option<i64>,
    inc_start: Option<i64>,
    inc_end: Option<i64>,
) {
    let Some(t) = inc else { return };
    if best.is_none_or(|b| t < b) {
        *best = Some(t);
        *start = inc_start;
        *end = inc_end;
    }
}

pub fn merge_counter_stats(base: &mut CounterStats, incoming: &CounterStats) {
    for (thread_name, inc_stats) in &incoming.threads {
        let entry = base.threads.entry(thread_name.clone()).or_default();
        entry.posts += inc_stats.posts;
        entry.counts += inc_stats.counts;
        entry.mistakes += inc_stats.mistakes;
        entry.odds += inc_stats.odds;
        entry.evens += inc_stats.evens;
        if entry.first_count.is_none() {
            entry.first_count = inc_stats.first_count.clone();
        }
        add_distinct_seconds_without_double_counting_the_seam(
            (&mut entry.seconds_posted, &mut entry.last_posted_second),
            (
                inc_stats.seconds_posted,
                inc_stats.first_posted_second,
                inc_stats.last_posted_second,
            ),
        );
        add_distinct_seconds_without_double_counting_the_seam(
            (&mut entry.seconds_counted, &mut entry.last_second),
            (inc_stats.seconds_counted, inc_stats.first_second, inc_stats.last_second),
        );

        take_faster(
            &mut entry.best_speed_us,
            &mut entry.best_speed_start,
            &mut entry.best_speed_end,
            inc_stats.best_speed_us,
            inc_stats.best_speed_start,
            inc_stats.best_speed_end,
        );
        take_faster(
            &mut entry.best_split_us,
            &mut entry.best_split_start,
            &mut entry.best_split_end,
            inc_stats.best_split_us,
            inc_stats.best_split_start,
            inc_stats.best_split_end,
        );
    }
}

fn add_distinct_seconds_without_double_counting_the_seam(
    (total, last_second): (&mut i64, &mut i64),
    (batch_total, batch_first, batch_last): (i64, i64, i64),
) {
    if batch_total == 0 {
        return;
    }
    let seam_already_counted = *last_second != 0 && batch_first == *last_second;
    *total += batch_total - i64::from(seam_already_counted);
    *last_second = (*last_second).max(batch_last);
}

#[cfg(test)]
mod pb_tests {
    use super::*;
    use crate::models::{CounterStats, CounterThreadStats};

    fn with_bests(speed: Option<i64>, split: Option<i64>) -> CounterStats {
        let mut cs = CounterStats::default();
        cs.threads.insert(
            "main".to_string(),
            CounterThreadStats {
                best_speed_us: speed,
                best_split_us: split,
                best_speed_start: speed.map(|t| t / 1_000),
                best_speed_end: speed.map(|t| t / 1_000 + 1_000),
                ..Default::default()
            },
        );
        cs
    }

    #[test]
    fn bests_take_the_minimum_not_the_sum() {
        let mut base = with_bests(Some(30_000_000), Some(2_500_000));
        merge_counter_stats(&mut base, &with_bests(Some(28_000_000), Some(3_000_000)));

        let t = &base.threads["main"];
        assert_eq!(t.best_speed_us, Some(28_000_000), "the faster run wins");
        assert_eq!(t.best_split_us, Some(2_500_000), "a slower split must not replace it");
        assert_eq!(t.best_speed_start, Some(28_000), "the faster run's own start");
    }

    #[test]
    fn a_first_record_becomes_the_best() {
        let mut base = with_bests(None, None);
        merge_counter_stats(&mut base, &with_bests(Some(45_000_000), None));
        let t = &base.threads["main"];
        assert_eq!(t.best_speed_us, Some(45_000_000));
        assert_eq!(t.best_split_us, None, "nothing arrived for splits");
    }

    #[test]
    fn an_empty_delta_leaves_a_best_alone() {
        let mut base = with_bests(Some(31_000_000), Some(1_900_000));
        merge_counter_stats(&mut base, &with_bests(None, None));
        let t = &base.threads["main"];
        assert_eq!(t.best_speed_us, Some(31_000_000));
        assert_eq!(t.best_split_us, Some(1_900_000));
    }

    #[test]
    fn counts_still_sum_alongside_bests() {
        let mut a = CounterStats::default();
        a.threads.insert(
            "main".into(),
            CounterThreadStats {
                counts: 10,
                best_speed_us: Some(9),
                ..Default::default()
            },
        );
        let mut b = CounterStats::default();
        b.threads.insert(
            "main".into(),
            CounterThreadStats {
                counts: 5,
                best_speed_us: Some(7),
                ..Default::default()
            },
        );

        merge_counter_stats(&mut a, &b);
        assert_eq!(a.threads["main"].counts, 15);
        assert_eq!(a.threads["main"].best_speed_us, Some(7));
    }
}

#[cfg(test)]
mod seconds_counted_tests {
    use super::*;
    use crate::models::{CounterStats, CounterThreadStats};

    fn delta(distinct: i64, first: i64, last: i64) -> CounterStats {
        let mut cs = CounterStats::default();
        cs.threads.insert(
            "all".to_string(),
            CounterThreadStats {
                seconds_counted: distinct,
                first_second: first,
                last_second: last,
                ..Default::default()
            },
        );
        cs
    }

    #[test]
    fn a_second_spanning_two_batches_counts_once() {
        let mut base = delta(10, 100, 200);
        merge_counter_stats(&mut base, &delta(5, 200, 260));

        let t = &base.threads["all"];
        assert_eq!(t.seconds_counted, 14, "10 + 5 minus the shared second");
        assert_eq!(t.last_second, 260);
    }

    #[test]
    fn a_clean_boundary_adds_everything() {
        let mut base = delta(10, 100, 200);
        merge_counter_stats(&mut base, &delta(5, 201, 260));
        assert_eq!(base.threads["all"].seconds_counted, 15);
    }

    #[test]
    fn the_first_batch_is_not_docked_a_second() {
        let mut base = CounterStats::default();
        merge_counter_stats(&mut base, &delta(7, 500, 560));
        let t = &base.threads["all"];
        assert_eq!(t.seconds_counted, 7, "no earlier batch, so nothing overlaps");
        assert_eq!(t.last_second, 560);
    }

    #[test]
    fn an_empty_batch_changes_nothing() {
        let mut base = delta(10, 100, 200);
        merge_counter_stats(&mut base, &delta(0, 0, 0));
        let t = &base.threads["all"];
        assert_eq!(t.seconds_counted, 10);
        assert_eq!(t.last_second, 200, "the marker must survive a quiet batch");
    }

    #[test]
    fn repeated_counts_in_one_second_are_one_second() {
        let mut base = CounterStats::default();
        merge_counter_stats(&mut base, &delta(1, 42, 42));
        assert_eq!(base.threads["all"].seconds_counted, 1);
    }
}

#[cfg(test)]
mod posted_seconds_tests {
    use super::*;
    use crate::models::{CounterStats, CounterThreadStats};

    fn d(posted: i64, first: i64, last: i64) -> CounterStats {
        let mut cs = CounterStats::default();
        cs.threads.insert(
            "all".to_string(),
            CounterThreadStats {
                seconds_posted: posted,
                first_posted_second: first,
                last_posted_second: last,
                ..Default::default()
            },
        );
        cs
    }

    #[test]
    fn a_posted_second_spanning_two_batches_counts_once() {
        let mut base = d(10, 100, 200);
        merge_counter_stats(&mut base, &d(5, 200, 260));
        assert_eq!(base.threads["all"].seconds_posted, 14);
        assert_eq!(base.threads["all"].last_posted_second, 260);
    }

    #[test]
    fn mistakes_add_posting_time_but_not_counting_time() {
        let mut base = CounterStats::default();
        let mut inc = d(6, 10, 15);
        inc.threads.get_mut("all").unwrap().seconds_counted = 0;
        merge_counter_stats(&mut base, &inc);

        let t = &base.threads["all"];
        assert_eq!(t.seconds_posted, 6, "the time was still spent");
        assert_eq!(t.seconds_counted, 0, "but nothing landed");
    }

    #[test]
    fn posting_time_covers_counting_time() {
        let mut base = CounterStats::default();
        let mut inc = d(9, 10, 20);
        inc.threads.get_mut("all").unwrap().seconds_counted = 4;
        inc.threads.get_mut("all").unwrap().first_second = 11;
        inc.threads.get_mut("all").unwrap().last_second = 19;
        merge_counter_stats(&mut base, &inc);

        let t = &base.threads["all"];
        assert!(t.seconds_posted >= t.seconds_counted, "every count is also a post");
        assert_eq!((t.seconds_posted, t.seconds_counted), (9, 4));
    }
}
