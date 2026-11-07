use ahash::AHashMap as HashMap;

pub const ALL: &str = "";

pub type LastSeconds = HashMap<(String, String), i64>;

pub type DaySeconds = HashMap<(String, String), HashMap<String, i64>>;

pub struct Point<'a> {
    pub counter: &'a str,
    pub thread: Option<&'a str>,
    pub ms: i64,
}

pub fn tally<'a>(points: impl IntoIterator<Item = Point<'a>>, last: &mut LastSeconds) -> DaySeconds {
    let mut secs: HashMap<(&str, &str), Vec<i64>> = HashMap::new();
    for p in points {
        let s = p.ms.div_euclid(1000);
        secs.entry((p.counter, ALL)).or_default().push(s);
        if let Some(t) = p.thread {
            secs.entry((p.counter, t)).or_default().push(s);
        }
    }

    let mut out: DaySeconds = HashMap::new();
    for ((counter, scope), mut v) in secs {
        v.sort_unstable();
        v.dedup();
        let key = (counter.to_string(), scope.to_string());
        let prev = last.get(&key).copied().unwrap_or(i64::MIN);
        let days = out.entry(key.clone()).or_default();
        let mut newest = prev;
        for s in v.into_iter().filter(|&s| s > prev) {
            *days.entry(crate::tz::ms_to_date_key(s * 1000)).or_insert(0) += 1;
            newest = s;
        }
        if days.is_empty() {
            out.remove(&key);
        }
        if newest > prev {
            last.insert(key, newest);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt<'a>(counter: &'a str, thread: &'a str, ms: i64) -> Point<'a> {
        Point {
            counter,
            thread: Some(thread),
            ms,
        }
    }

    fn get(d: &DaySeconds, counter: &str, scope: &str, day: &str) -> i64 {
        d.get(&(counter.to_string(), scope.to_string()))
            .and_then(|m| m.get(day))
            .copied()
            .unwrap_or(0)
    }

    const NOON: i64 = 1_768_496_400_000;

    #[test]
    fn distinct_seconds_per_scope() {
        let mut last = LastSeconds::new();
        let d = tally(
            [
                pt("a", "main", NOON),
                pt("a", "main", NOON + 500),
                pt("a", "main", NOON + 1000),
                pt("a", "slow", NOON + 1200),
            ],
            &mut last,
        );
        assert_eq!(get(&d, "a", "main", "2026-01-15"), 2);
        assert_eq!(get(&d, "a", "slow", "2026-01-15"), 1);
        assert_eq!(get(&d, "a", ALL, "2026-01-15"), 2);
    }

    #[test]
    fn seam_second_counted_once() {
        let mut last = LastSeconds::new();
        let a = tally([pt("a", "main", NOON), pt("a", "main", NOON + 1100)], &mut last);
        let b = tally([pt("a", "main", NOON + 1900), pt("a", "main", NOON + 2000)], &mut last);
        let total = get(&a, "a", "main", "2026-01-15") + get(&b, "a", "main", "2026-01-15");
        assert_eq!(total, 3, "seconds 0, 1, 2 -- the shared second 1 only once");
        assert_eq!(get(&a, "a", ALL, "2026-01-15") + get(&b, "a", ALL, "2026-01-15"), 3);
    }

    #[test]
    fn a_batch_entirely_inside_the_seam_second_adds_nothing() {
        let mut last = LastSeconds::new();
        tally([pt("a", "main", NOON + 100)], &mut last);
        let b = tally([pt("a", "main", NOON + 900)], &mut last);
        assert!(b.is_empty());
    }

    #[test]
    fn seconds_split_at_eastern_midnight() {
        let mut last = LastSeconds::new();
        let midnight = NOON + 12 * 3_600_000;
        let d = tally([pt("a", "main", midnight - 1000), pt("a", "main", midnight)], &mut last);
        assert_eq!(get(&d, "a", "main", "2026-01-15"), 1);
        assert_eq!(get(&d, "a", "main", "2026-01-16"), 1);
    }

    #[test]
    fn counters_do_not_share_seconds() {
        let mut last = LastSeconds::new();
        let d = tally([pt("a", "main", NOON), pt("b", "main", NOON)], &mut last);
        assert_eq!(get(&d, "a", "main", "2026-01-15"), 1);
        assert_eq!(get(&d, "b", "main", "2026-01-15"), 1);
    }

    #[test]
    fn unknown_thread_still_counts_overall() {
        let mut last = LastSeconds::new();
        let d = tally(
            [Point {
                counter: "a",
                thread: None,
                ms: NOON,
            }],
            &mut last,
        );
        assert_eq!(get(&d, "a", ALL, "2026-01-15"), 1);
        assert_eq!(d.len(), 1);
    }
}
