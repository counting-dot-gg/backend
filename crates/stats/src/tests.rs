use ahash::AHashMap as HashMap;

use crate::{
    models::{
        Post, SpeedEntry, Thread, ThreadStats, combine_leaderboards, is_palindrome, is_repdigit,
        leaderboard_from_posts, uuid_parse_ms, uuid_parse_to_nano,
    },
    speed::{boundary_lookups_needed, compute_speed, compute_split_speed, detect_fake_split},
    stats_engine::merge_thread_stats,
};

fn make_thread() -> Thread {
    Thread {
        id: 1,
        uuid: "thread-uuid".to_string(),
        name: "main".to_string(),
        validation_type: "main".to_string(),
        reset_on_mistakes: false,
        first_valid_count: "1".to_string(),
        counts_per_split: 100,
        splits_per_get: 10,
        split_offset: 0,
        allow_double_posts: false,
        created_at_ms: Some(0),
    }
}

fn make_post(id: u64, uuid: &str, author: &str, vcn: Option<i64>, raw: Option<&str>) -> Post {
    Post {
        id,
        uuid: uuid.to_string(),
        thread: "thread-uuid".to_string(),
        author_uuid: author.to_string(),
        timestamp: "0".to_string(),
        is_count: vcn.is_some(),
        is_valid_count: vcn.map(|_| true),
        valid_count_number: vcn,
        raw_count: raw.map(|s| s.to_string()),
        has_comment: false,
    }
}

fn make_invalid_post(id: u64, uuid: &str, author: &str) -> Post {
    Post {
        id,
        uuid: uuid.to_string(),
        thread: "thread-uuid".to_string(),
        author_uuid: author.to_string(),
        timestamp: "0".to_string(),
        is_count: false,
        is_valid_count: Some(false),
        valid_count_number: None,
        raw_count: None,
        has_comment: false,
    }
}

const UUID_A: &str = "16775434-5484-4414-8280-835440414280";
const UUID_B: &str = "16775437-7735-4917-a613-888465917613";
const UUID_C: &str = "16775440-0673-4989-9497-568080989497";

#[test]
fn palindrome_single_char_is_true() {
    assert!(is_palindrome("1"));
    assert!(is_palindrome("9"));
}

#[test]
fn palindrome_empty_is_false() {
    assert!(!is_palindrome(""));
}

#[test]
fn palindrome_even_length() {
    assert!(is_palindrome("1221"));
    assert!(is_palindrome("11"));
    assert!(!is_palindrome("12"));
}

#[test]
fn palindrome_odd_length() {
    assert!(is_palindrome("121"));
    assert!(is_palindrome("12321"));
    assert!(!is_palindrome("123"));
}

#[test]
fn palindrome_repdigits_are_palindromes() {
    assert!(is_palindrome("111"));
    assert!(is_palindrome("9999"));
}

#[test]
fn palindrome_large_number() {
    assert!(is_palindrome("1000001"));
    assert!(!is_palindrome("1000002"));
}

#[test]
fn repdigit_single_digit_always_true() {
    for d in 0u64..10 {
        assert!(is_repdigit(&d.to_string()), "{d} should be repdigit");
    }
}

#[test]
fn repdigit_multi_digit_same_chars() {
    assert!(is_repdigit("11"));
    assert!(is_repdigit("111"));
    assert!(is_repdigit("9999"));
    assert!(is_repdigit("22222"));
}

#[test]
fn repdigit_multi_digit_different_chars() {
    assert!(!is_repdigit("12"));
    assert!(!is_repdigit("121"));
    assert!(!is_repdigit("1234"));
}

#[test]
fn repdigit_ten_is_not() {
    assert!(!is_repdigit("10"));
}

#[test]
fn repdigit_empty_is_false() {
    assert!(!is_repdigit(""));
}

#[test]
fn uuid_parse_nano_extracts_timestamp() {
    let ns = uuid_parse_to_nano(UUID_A);
    assert!(
        ns > 1_600_000_000_000_000_000,
        "should be a 2020s nanosecond epoch: {ns}"
    );
}

#[test]
fn uuid_parse_ms_converts_correctly() {
    let ns = uuid_parse_to_nano(UUID_A);
    let ms = uuid_parse_ms(UUID_A).expect("should parse");
    assert_eq!(ms, ns / 1_000_000);
}

#[test]
fn uuid_parse_ms_ordering() {
    let a = uuid_parse_ms(UUID_A).unwrap();
    let b = uuid_parse_ms(UUID_B).unwrap();
    let c = uuid_parse_ms(UUID_C).unwrap();
    assert!(a < b, "A should be before B");
    assert!(b < c, "B should be before C");
}

#[test]
fn uuid_parse_ms_delta_reasonable() {
    let a = uuid_parse_ms(UUID_A).unwrap();
    let b = uuid_parse_ms(UUID_B).unwrap();
    assert_eq!(a, 1_677_543_454_844, "must match uuidParse(UUID_A)");
    assert_eq!(b, 1_677_543_777_359, "must match uuidParse(UUID_B)");
    let delta = b - a;
    assert_eq!(delta, 322_515, "delta should be 322515 ms, got {delta}");
}

#[test]
fn uuid_parse_invalid_returns_zero_or_none() {
    assert_eq!(uuid_parse_to_nano("not-a-uuid"), 0);
    assert!(uuid_parse_ms("not-a-uuid").is_none());
}

#[test]
fn leaderboard_only_counts_valid() {
    let posts = vec![
        make_post(1, "u1", "alice", Some(1), Some("1")),
        make_post(2, "u2", "alice", Some(2), Some("2")),
        make_invalid_post(3, "u3", "alice"),
        make_post(4, "u4", "bob", Some(3), Some("3")),
    ];
    let lb = leaderboard_from_posts(&posts);
    assert_eq!(lb.get("alice"), Some(&2));
    assert_eq!(lb.get("bob"), Some(&1));
}

#[test]
fn leaderboard_empty_input() {
    let lb = leaderboard_from_posts(&[]);
    assert!(lb.is_empty());
}

#[test]
fn leaderboard_all_invalid() {
    let posts = vec![make_invalid_post(1, "u1", "alice"), make_invalid_post(2, "u2", "bob")];
    let lb = leaderboard_from_posts(&posts);
    assert!(lb.is_empty());
}

#[test]
fn combine_leaderboards_adds_values() {
    let mut base: HashMap<String, i64> = [("alice".to_string(), 5)].into_iter().collect();
    let incoming: HashMap<String, i64> = [("alice".to_string(), 3), ("bob".to_string(), 2)].into_iter().collect();
    combine_leaderboards(&mut base, &incoming);
    assert_eq!(base["alice"], 8);
    assert_eq!(base["bob"], 2);
}

#[test]
fn combine_leaderboards_empty_incoming() {
    let mut base: HashMap<String, i64> = [("alice".to_string(), 5)].into_iter().collect();
    combine_leaderboards(&mut base, &HashMap::new());
    assert_eq!(base["alice"], 5);
}

#[test]
fn thread_get_size() {
    let t = make_thread();
    assert_eq!(t.get_size(), 1000);
}

#[test]
fn thread_is_get_no_offset() {
    let t = make_thread();
    assert!(t.is_get(1000));
    assert!(t.is_get(2000));
    assert!(!t.is_get(100));
    assert!(!t.is_get(999));
}

#[test]
fn thread_is_split_no_offset() {
    let t = make_thread();
    assert!(t.is_split(100));
    assert!(t.is_split(200));
    assert!(!t.is_split(99));
    assert!(!t.is_split(101));
}

#[test]
fn thread_is_get_with_offset() {
    let mut t = make_thread();
    t.split_offset = 50;
    assert!(t.is_get(1050));
    assert!(t.is_get(2050));
    assert!(!t.is_get(1000));
    assert!(!t.is_get(50));
}

#[test]
fn thread_is_split_with_offset() {
    let mut t = make_thread();
    t.split_offset = 50;
    assert!(t.is_split(150));
    assert!(t.is_split(250));
    assert!(!t.is_split(100));
    assert!(!t.is_split(50));
}

#[test]
fn boundary_lookups_first_split_needs_no_lookup() {
    let t = make_thread();
    let gets = vec![make_post(1, "u1", "alice", Some(100), Some("100"))];
    let lookups = boundary_lookups_needed(&gets, &t, true);
    assert!(lookups.is_empty(), "vcn=100 == split_size, no prior boundary");
}

#[test]
fn boundary_lookups_second_split_needs_lookup() {
    let t = make_thread();
    let gets = vec![make_post(1, "u1", "alice", Some(200), Some("200"))];
    let lookups = boundary_lookups_needed(&gets, &t, true);
    assert_eq!(lookups.len(), 1);
    assert_eq!(lookups[0], ("thread-uuid".to_string(), 100));
}

#[test]
fn boundary_lookups_for_gets() {
    let t = make_thread();
    let gets = vec![make_post(1, "u1", "alice", Some(2000), Some("2000"))];
    let lookups = boundary_lookups_needed(&gets, &t, false);
    assert_eq!(lookups.len(), 1);
    assert_eq!(lookups[0], ("thread-uuid".to_string(), 1000));
}

#[test]
fn boundary_lookups_empty_gets() {
    let t = make_thread();
    let lookups = boundary_lookups_needed(&[], &t, false);
    assert!(lookups.is_empty());
}

fn fake_post_with_uuid(id: u64, uuid: &str, author: &str) -> Post {
    Post {
        id,
        uuid: uuid.to_string(),
        thread: "thread-uuid".to_string(),
        author_uuid: author.to_string(),
        timestamp: "0".to_string(),
        is_count: true,
        is_valid_count: Some(true),
        valid_count_number: Some(id as i64),
        raw_count: Some(id.to_string()),
        has_comment: false,
    }
}

#[test]
fn detect_fake_not_triggered_with_normal_gaps() {
    let posts = vec![
        fake_post_with_uuid(1, UUID_A, "alice"),
        fake_post_with_uuid(2, UUID_B, "bob"),
        fake_post_with_uuid(3, UUID_C, "alice"),
    ];
    assert!(!detect_fake_split(&posts));
}

#[test]
fn detect_fake_empty_posts() {
    assert!(!detect_fake_split(&[]));
}

#[test]
fn detect_fake_skips_empty_uuids() {
    let mut p = fake_post_with_uuid(1, UUID_A, "alice");
    p.uuid = String::new();
    assert!(!detect_fake_split(&[p]));
}

fn speed_post(id: u64, uuid: &str, author: &str, vcn: i64, raw: &str) -> Post {
    Post {
        id,
        uuid: uuid.to_string(),
        thread: "thread-uuid".to_string(),
        author_uuid: author.to_string(),
        timestamp: "0".to_string(),
        is_count: true,
        is_valid_count: Some(true),
        valid_count_number: Some(vcn),
        raw_count: Some(raw.to_string()),
        has_comment: false,
    }
}

#[test]
fn a_run_starting_before_the_batch_is_dropped_not_truncated() {
    let thread = make_thread();

    let all_posts = vec![
        speed_post(9500, UUID_A, "alice", 1950, "1950"),
        speed_post(9600, UUID_B, "bob", 1999, "1999"),
        speed_post(9700, UUID_C, "alice", 2000, "2000"),
    ];
    let gets = vec![all_posts[2].clone()];

    let prev = speed_post(10, UUID_A, "alice", 1000, "1000");
    let mut boundary_posts = HashMap::new();
    boundary_posts.insert((thread.uuid.clone(), 1000i64), prev);

    let result = compute_speed(&all_posts, &[], &gets, &thread, &boundary_posts, None);
    assert!(
        result.is_empty(),
        "a partial run is a different record, not a slower one: got {result:?}"
    );
}

#[test]
fn a_lookback_restores_a_run_that_starts_in_an_earlier_batch() {
    let thread = make_thread();

    let prev_get = speed_post(10, UUID_A, "alice", 1000, "1000");
    let lookback = vec![
        prev_get.clone(),
        speed_post(20, UUID_B, "bob", 1001, "1001"),
        speed_post(30, UUID_C, "alice", 1500, "1500"),
    ];
    let batch = vec![
        speed_post(9500, UUID_A, "alice", 1950, "1950"),
        speed_post(9700, UUID_B, "bob", 2000, "2000"),
    ];
    let gets = vec![batch[1].clone()];

    let mut boundary_posts = HashMap::new();
    boundary_posts.insert((thread.uuid.clone(), 1000i64), prev_get);

    let result = compute_speed(&batch, &lookback, &gets, &thread, &boundary_posts, None);
    assert_eq!(result.len(), 1, "the lookback completes the window");
    let entry = &result[0];
    assert_eq!(
        entry.start_count_number, 1000,
        "the run is measured from the previous get, not from the batch edge"
    );
    assert_eq!(entry.end_count_number, 2000);
    assert_eq!(
        entry.end_count_number - entry.start_count_number,
        thread.get_size(),
        "a get record spans exactly one run"
    );
}

#[test]
fn a_fully_loaded_run_still_produces_a_record() {
    let thread = make_thread();

    let all_posts = vec![
        speed_post(10, UUID_A, "alice", 1000, "1000"),
        speed_post(9500, UUID_B, "bob", 1950, "1950"),
        speed_post(9700, UUID_C, "alice", 2000, "2000"),
    ];
    let gets = vec![all_posts[2].clone()];

    let mut boundary_posts = HashMap::new();
    boundary_posts.insert((thread.uuid.clone(), 1000i64), all_posts[0].clone());

    let result = compute_speed(&all_posts, &[], &gets, &thread, &boundary_posts, None);
    assert_eq!(result.len(), 1, "the boundary is loaded, so the run is whole");
}

#[test]
fn compute_speed_basic() {
    let thread = make_thread();
    let all_posts = vec![
        speed_post(10, UUID_A, "alice", 1, "1"),
        speed_post(20, UUID_B, "bob", 500, "500"),
        speed_post(30, UUID_C, "alice", 1000, "1000"),
    ];
    let gets = vec![all_posts[2].clone()];
    let boundary_posts = HashMap::new();

    let result = compute_speed(&all_posts, &[], &gets, &thread, &boundary_posts, None);
    assert_eq!(result.len(), 1);
    let entry = &result[0];
    assert!(entry.time > 0, "time should be positive");
    assert!(
        entry.time < 600_000 * 1_000,
        "time should be under the 10-minute threshold, in us: {}",
        entry.time
    );
    assert!(
        entry.time > 551_000_000 && entry.time < 552_000_000,
        "expected ~551.9s expressed in microseconds, got {}",
        entry.time
    );
    assert_eq!(entry.start_count_number, 1);
    assert_eq!(entry.end_count_number, 1000);
}

#[test]
fn compute_speed_no_gets() {
    let thread = make_thread();
    let all_posts: Vec<Post> = vec![];
    let result = compute_speed(&all_posts, &[], &[], &thread, &HashMap::new(), None);
    assert!(result.is_empty());
}

#[test]
fn compute_speed_rejects_zero_time() {
    let thread = make_thread();
    let p = speed_post(10, UUID_A, "alice", 1000, "1000");
    let gets = vec![p.clone()];
    let all_posts = vec![p];
    let result = compute_speed(&all_posts, &[], &gets, &thread, &HashMap::new(), None);
    assert!(result.is_empty());
}

#[test]
fn compute_split_speed_basic() {
    let thread = make_thread();
    let all_posts = vec![
        speed_post(10, UUID_A, "alice", 1, "1"),
        speed_post(20, UUID_B, "bob", 50, "50"),
        speed_post(30, UUID_C, "alice", 100, "100"),
    ];
    let splits = vec![all_posts[2].clone()];
    let result = compute_split_speed(&all_posts, &[], &splits, &thread, &HashMap::new(), None);
    assert_eq!(result.len(), 1);
    let entry = &result[0];
    assert!(entry.time > 0);
    assert_eq!(entry.start_count_number, 1);
    assert_eq!(entry.end_count_number, 100);
}

#[test]
fn compute_split_speed_no_splits() {
    let thread = make_thread();
    let result = compute_split_speed(&[], &[], &[], &thread, &HashMap::new(), None);
    assert!(result.is_empty());
}

fn speed_entry(start: &str, end: &str) -> SpeedEntry {
    SpeedEntry {
        start: start.to_string(),
        end: end.to_string(),
        time: 1000,
        start_count: "1".to_string(),
        end_count: "1000".to_string(),
        start_count_number: 1,
        end_count_number: 1000,
        qualified_counters: vec!["alice".to_string()],
    }
}

#[test]
fn merge_thread_stats_adds_leaderboards() {
    let mut base = ThreadStats::default();
    base.leaderboard.insert("alice".into(), 5);
    base.leaderboard.insert("bob".into(), 2);

    let mut delta = ThreadStats::default();
    delta.leaderboard.insert("alice".into(), 3);
    delta.leaderboard.insert("carol".into(), 7);

    merge_thread_stats(&mut base, delta);

    assert_eq!(base.leaderboard.get("alice"), Some(&8), "overlapping key sums");
    assert_eq!(base.leaderboard.get("bob"), Some(&2), "untouched key kept");
    assert_eq!(base.leaderboard.get("carol"), Some(&7), "new key added");
}

#[test]
fn merge_thread_stats_promotes_none_to_some() {
    let mut base = ThreadStats::default();
    assert!(base.gets.is_none());

    let mut delta = ThreadStats::default();
    let mut gets = HashMap::new();
    gets.insert("alice".to_string(), 1i64);
    delta.gets = Some(gets);

    merge_thread_stats(&mut base, delta);
    assert_eq!(base.gets.as_ref().and_then(|g| g.get("alice")), Some(&1));
}

#[test]
fn merge_thread_stats_keeps_base_when_delta_absent() {
    let mut base = ThreadStats::default();
    let mut gets = HashMap::new();
    gets.insert("alice".to_string(), 4i64);
    base.gets = Some(gets);

    merge_thread_stats(&mut base, ThreadStats::default());
    assert_eq!(base.gets.as_ref().and_then(|g| g.get("alice")), Some(&4));
}

#[test]
fn merge_thread_stats_concatenates_speed_records() {
    let mut base = ThreadStats {
        speed: Some(vec![speed_entry("a", "b")]),
        ..Default::default()
    };

    let delta = ThreadStats {
        speed: Some(vec![speed_entry("b", "c"), speed_entry("c", "d")]),

        ..Default::default()
    };

    merge_thread_stats(&mut base, delta);

    let speed = base.speed.expect("speed present");
    assert_eq!(speed.len(), 3, "records append rather than replace");
    assert_eq!(speed[0].start, "a");
    assert_eq!(speed[2].end, "d");
}

#[test]
fn merge_thread_stats_takes_highest_last_updated() {
    let mut base = ThreadStats {
        last_updated: 500,
        last_updated_uuid: "old".into(),
        ..Default::default()
    };

    let newer = ThreadStats {
        last_updated: 900,

        last_updated_uuid: "new".into(),

        ..Default::default()
    };
    merge_thread_stats(&mut base, newer);
    assert_eq!(base.last_updated, 900);
    assert_eq!(base.last_updated_uuid, "new");

    let older = ThreadStats {
        last_updated: 100,

        last_updated_uuid: "older".into(),

        ..Default::default()
    };
    merge_thread_stats(&mut base, older);
    assert_eq!(base.last_updated, 900);
    assert_eq!(base.last_updated_uuid, "new");
}

#[test]
fn merge_thread_stats_is_associative_over_batches() {
    let mut one_shot = ThreadStats::default();
    let mut d = ThreadStats::default();
    d.leaderboard.insert("alice".into(), 3);
    d.leaderboard.insert("bob".into(), 1);
    d.speed = Some(vec![speed_entry("a", "b"), speed_entry("b", "c")]);
    merge_thread_stats(&mut one_shot, d);

    let mut split = ThreadStats::default();
    let mut d1 = ThreadStats::default();
    d1.leaderboard.insert("alice".into(), 2);
    d1.speed = Some(vec![speed_entry("a", "b")]);
    merge_thread_stats(&mut split, d1);
    let mut d2 = ThreadStats::default();
    d2.leaderboard.insert("alice".into(), 1);
    d2.leaderboard.insert("bob".into(), 1);
    d2.speed = Some(vec![speed_entry("b", "c")]);
    merge_thread_stats(&mut split, d2);

    assert_eq!(split.leaderboard.get("alice"), one_shot.leaderboard.get("alice"));
    assert_eq!(split.leaderboard.get("bob"), one_shot.leaderboard.get("bob"));
    assert_eq!(
        split.speed.as_ref().map(|s| s.len()),
        one_shot.speed.as_ref().map(|s| s.len()),
    );
}

#[test]
fn ts_ms_key_is_thirteen_digits() {
    assert_eq!(crate::db::ts_ms_key(1_677_064_981_003), "1677064981003");
    assert_eq!(crate::db::ts_ms_key(1_787_882_401_164), "1787882401164");
}

#[test]
fn ts_ms_key_orders_like_the_stored_values() {
    let key = crate::db::ts_ms_key(1_677_064_981_003);
    assert!(key.as_str() < "1677064981003.905", "equal ms sorts before its fraction");
    assert!(
        key.as_str() > "1677064981002.999",
        "an earlier post sorts before the key"
    );
    assert!(key.as_str() < "1677064981004", "a later whole ms sorts after the key");
}

#[test]
fn ts_ms_key_lexicographic_matches_numeric() {
    let a = crate::db::ts_ms_key(1_677_064_981_003);
    let b = crate::db::ts_ms_key(1_787_882_401_164);
    assert!(a < b, "string order must track time order across the archive");
    assert_eq!(a.len(), b.len(), "fixed width is what makes that true");
}

use crate::models::SplitSpeedEntry;
use crate::packed::{decode, encode, is_packed};

fn sp(scn: i64, ecn: i64, time: i64, qc: &[&str]) -> SpeedEntry {
    SpeedEntry {
        start: "16775352-2406-4441-8902-229830441902".into(),
        end: "16775356-4394-4163-8903-314438163903".into(),
        time,
        start_count: format!("{scn}"),
        end_count: format!("{ecn}"),
        start_count_number: scn,
        end_count_number: ecn,
        qualified_counters: qc.iter().map(|s| s.to_string()).collect(),
    }
}

fn spl(scn: i64, ecn: i64, time: i64, is_fake: bool, qc: &[&str]) -> SplitSpeedEntry {
    SplitSpeedEntry {
        start: "16775352-2406-4441-8902-229830441902".into(),
        end: "16775356-4394-4163-8903-314438163903".into(),
        time,
        start_count: format!("{scn}"),
        end_count: format!("{ecn}"),
        start_count_number: scn,
        end_count_number: ecn,
        qualified_counters: qc.iter().map(|s| s.to_string()).collect(),
        is_fake,
    }
}

const CA: &str = "16770964-5364-4843-9581-443269843581";
const CB: &str = "16770978-7202-4761-a754-496674761754";

fn sample() -> ThreadStats {
    let mut ts = ThreadStats::default();
    ts.leaderboard.insert(CA.into(), 1234);
    ts.leaderboard.insert(CB.into(), 99);
    let mut gets = HashMap::new();
    gets.insert(CA.to_string(), 7i64);
    ts.gets = Some(gets);
    ts.speed = Some(vec![
        sp(1, 1000, 419877, &[CA]),
        sp(1000, 2000, 1234, &[CA, CB]),
        sp(2000, 2805, 98, &[]),
    ]);
    ts.split_speed = Some(vec![spl(1, 100, 55, false, &[CB]), spl(100, 200, 66, true, &[CA, CB])]);
    ts.last_updated = 97_066_595;
    ts.last_updated_uuid = "16775352-2406-4441-8902-229830441902".into();
    ts
}

#[test]
fn packed_is_self_identifying() {
    let bytes = encode(&sample()).expect("encodes");
    assert!(is_packed(&bytes), "must carry the magic so readers can tell");
    assert!(!is_packed(b"{\"leaderboard\":{}}"), "plain JSON is not packed");
    assert!(!is_packed(b""), "empty is not packed");
}

#[test]
fn packed_round_trips_everything_it_stores() {
    let original = sample();
    let back = decode(&encode(&original).unwrap()).unwrap();

    assert_eq!(back.leaderboard, original.leaderboard);
    assert_eq!(back.gets, original.gets);
    assert_eq!(back.last_updated, original.last_updated);
    assert_eq!(back.last_updated_uuid, original.last_updated_uuid);

    let (os, bs) = (original.speed.unwrap(), back.speed.unwrap());
    assert_eq!(os.len(), bs.len());
    for (o, b) in os.iter().zip(bs.iter()) {
        assert_eq!(b.start_count_number, o.start_count_number);
        assert_eq!(
            b.end_count_number, o.end_count_number,
            "span dictionary must restore it"
        );
        assert_eq!(b.time, o.time);
        assert_eq!(b.qualified_counters, o.qualified_counters);
    }

    let (ol, bl) = (original.split_speed.unwrap(), back.split_speed.unwrap());
    assert_eq!(ol.len(), bl.len());
    for (o, b) in ol.iter().zip(bl.iter()) {
        assert_eq!(b.start_count_number, o.start_count_number);
        assert_eq!(b.end_count_number, o.end_count_number);
        assert_eq!(b.time, o.time);
        assert_eq!(b.is_fake, o.is_fake, "the fake flag is a bit, and must survive");
        assert_eq!(b.qualified_counters, o.qualified_counters);
    }
}

#[test]
fn packed_omits_the_derivable_fields() {
    let back = decode(&encode(&sample()).unwrap()).unwrap();
    let s = &back.speed.unwrap()[0];
    assert!(s.start.is_empty(), "uuid comes from post, not from here");
    assert!(s.end.is_empty());
    assert!(s.start_count.is_empty(), "rawCount comes from post");
    assert!(s.end_count.is_empty());
}

#[test]
fn packed_handles_empty_and_absent_arrays() {
    let mut ts = ThreadStats::default();
    ts.leaderboard.insert(CA.into(), 1);
    let back = decode(&encode(&ts).unwrap()).unwrap();
    assert!(back.speed.is_none());
    assert!(back.split_speed.is_none());
    assert_eq!(back.leaderboard.get(CA), Some(&1));
}

#[test]
fn packed_survives_non_monotonic_and_negative_deltas() {
    let ts = ThreadStats {
        speed: Some(vec![
            sp(5000, 6000, 10, &[CA]),
            sp(10, 1010, 20, &[CA]),
            sp(7000, 8000, 30, &[CA]),
        ]),
        ..Default::default()
    };
    let back = decode(&encode(&ts).unwrap()).unwrap();
    let got: Vec<i64> = back.speed.unwrap().iter().map(|e| e.start_count_number).collect();
    assert_eq!(got, vec![5000, 10, 7000]);
}

#[test]
fn packed_rejects_truncated_input() {
    let bytes = encode(&sample()).unwrap();
    for cut in [5, 12, 40, bytes.len() - 1] {
        assert!(decode(&bytes[..cut]).is_err(), "truncated at {cut} must not decode");
    }
}

#[test]
fn packed_is_much_smaller_than_json() {
    let ts = sample();
    let json = serde_json::to_vec(&ts).unwrap();
    let packed = encode(&ts).unwrap();
    assert!(
        packed.len() * 2 < json.len(),
        "packed {} vs json {} — the point of this format",
        packed.len(),
        json.len()
    );
}

fn qc_post(id: u64, author: &str) -> Post {
    Post {
        id,
        uuid: format!("1677543{:03}-5484-4414-8280-835440414280", id % 1000),
        thread: "thread-uuid".into(),
        author_uuid: author.into(),
        timestamp: "1677543454844.905".into(),
        is_count: true,
        is_valid_count: Some(true),
        valid_count_number: Some(id as i64),
        raw_count: Some(id.to_string()),
        has_comment: false,
    }
}

fn qc(posts: &[Post], first: Option<&str>) -> Vec<String> {
    let mut v = crate::speed::qualified_counters_for_test(posts, first);
    v.sort();
    v
}

#[test]
fn qualified_threshold_is_an_exact_quarter_not_integer_division() {
    let mut posts = Vec::new();
    for i in 0..14 {
        posts.push(qc_post(i, "top"));
    }
    for i in 0..3 {
        posts.push(qc_post(100 + i, "three"));
    }
    for i in 0..4 {
        posts.push(qc_post(200 + i, "four"));
    }

    let got = qc(&posts, None);
    assert!(got.contains(&"top".to_string()));
    assert!(got.contains(&"four".to_string()), "4 >= 14/4 = 3.5");
    assert!(
        !got.contains(&"three".to_string()),
        "3 is below 3.5 — `top / 4` truncating to 3 is the bug this guards"
    );
}

#[test]
fn qualified_excludes_the_timing_post_from_its_author() {
    let mut posts = Vec::new();
    for i in 0..16 {
        posts.push(qc_post(i, "top"));
    }
    for i in 0..4 {
        posts.push(qc_post(200 + i, "four"));
    }

    let without = qc(&posts, None);
    assert!(without.contains(&"four".to_string()), "4 >= 16/4 = 4");

    let with = qc(&posts, Some("four"));
    assert!(
        !with.contains(&"four".to_string()),
        "the post that starts the window is a timing marker, not part of the run"
    );
    assert!(with.contains(&"top".to_string()), "everyone else is unaffected");
}

#[test]
fn qualified_discounts_only_one_post_and_only_that_author() {
    let mut posts = Vec::new();
    for i in 0..8 {
        posts.push(qc_post(i, "a"));
    }
    for i in 0..8 {
        posts.push(qc_post(100 + i, "b"));
    }
    let got = qc(&posts, Some("a"));
    assert!(got.contains(&"a".to_string()));
    assert!(got.contains(&"b".to_string()));
}

#[test]
fn qualified_handles_empty_and_unknown_author() {
    assert!(qc(&[], None).is_empty());
    assert!(qc(&[], Some("nobody")).is_empty());
    let posts = vec![qc_post(1, "a")];
    assert_eq!(qc(&posts, Some("ghost")), vec!["a".to_string()]);
}

#[test]
fn qualified_returns_nothing_when_the_only_post_is_the_timing_post() {
    let posts = vec![qc_post(1, "a")];
    assert!(
        qc(&posts, Some("a")).is_empty(),
        "discounting the only post leaves nobody with a positive count"
    );
}

use crate::packed::{decode_month, encode_month, is_packed_month};

#[test]
fn month_round_trips_its_days() {
    let mut days = std::collections::BTreeMap::new();
    for dom in [1u32, 7, 15, 28] {
        let mut ts = ThreadStats::default();
        ts.leaderboard.insert(CA.into(), dom as i64 * 10);
        ts.speed = Some(vec![sp(dom as i64 * 1000, dom as i64 * 1000 + 500, 4242, &[CA])]);
        days.insert(dom, ts);
    }
    let bytes = encode_month(&days).expect("encodes");
    assert!(is_packed_month(&bytes));

    let back = decode_month(&bytes).expect("decodes");
    assert_eq!(back.keys().copied().collect::<Vec<_>>(), vec![1, 7, 15, 28]);
    for (dom, ts) in &back {
        assert_eq!(ts.leaderboard.get(CA), Some(&(*dom as i64 * 10)));
        let s = ts.speed.as_ref().expect("speed survived");
        assert_eq!(s[0].start_count_number, *dom as i64 * 1000);
        assert_eq!(s[0].time, 4242);
    }
}

#[test]
fn month_handles_a_single_day_and_an_empty_month() {
    let mut one = std::collections::BTreeMap::new();
    one.insert(9u32, ThreadStats::default());
    assert_eq!(decode_month(&encode_month(&one).unwrap()).unwrap().len(), 1);

    let empty = std::collections::BTreeMap::new();
    assert!(decode_month(&encode_month(&empty).unwrap()).unwrap().is_empty());
}

#[test]
fn month_compresses_when_it_is_worth_it() {
    let mut days = std::collections::BTreeMap::new();
    for dom in 1u32..=30 {
        let mut ts = ThreadStats::default();
        for i in 0..40 {
            ts.leaderboard
                .insert(format!("1677096{i:01}-5364-4843-9581-44326984358{i:01}"), 7);
        }
        ts.speed = Some(vec![sp(dom as i64, dom as i64 + 100, 999, &[CA, CB])]);
        days.insert(dom, ts);
    }
    let bytes = encode_month(&days).unwrap();
    assert_eq!(bytes[4], 1, "should have taken the brotli branch");

    let raw: usize = days.values().map(|t| encode(t).unwrap().len()).sum();
    assert!(
        bytes.len() * 2 < raw,
        "packed month {} vs loose days {raw}",
        bytes.len()
    );
    assert_eq!(decode_month(&bytes).unwrap().len(), 30, "still round trips");
}

#[test]
fn month_rejects_a_day_entry_and_vice_versa() {
    let ts = sample();
    assert!(
        decode_month(&encode(&ts).unwrap()).is_err(),
        "a day entry is not a month"
    );
    let mut days = std::collections::BTreeMap::new();
    days.insert(1u32, ts);
    assert!(
        decode(&encode_month(&days).unwrap()).is_err(),
        "a month is not a day entry"
    );
}

#[test]
fn month_rejects_truncated_input() {
    let mut days = std::collections::BTreeMap::new();
    days.insert(3u32, sample());
    let bytes = encode_month(&days).unwrap();
    for cut in [5, 9, bytes.len() - 1] {
        assert!(decode_month(&bytes[..cut]).is_err(), "truncated at {cut}");
    }
}

use crate::counter_stats::merge_counter_stats;
use crate::models::{CounterStats, CounterThreadStats};

fn cs(thread: &str, posts: i64, counts: i64, first: Option<&str>) -> CounterStats {
    let mut c = CounterStats::default();
    c.threads.insert(
        thread.to_string(),
        CounterThreadStats {
            posts,
            counts,
            mistakes: 0,
            odds: counts / 2,
            evens: counts - counts / 2,
            first_count: first.map(|s| s.to_string()),
            ranks: Default::default(),
            best_speed_us: None,
            best_split_us: None,
            best_speed_start: None,
            best_speed_end: None,
            best_split_start: None,
            best_split_end: None,
            seconds_counted: 0,
            last_second: 0,
            first_second: 0,
            seconds_posted: 0,
            last_posted_second: 0,
            first_posted_second: 0,
        },
    );
    c
}

#[test]
fn incremental_adds_to_stored_history_rather_than_replacing_it() {
    let mut stored = cs("main", 5000, 4800, Some("main/first-uuid"));
    let delta = cs("main", 41, 37, Some("main/recent-uuid"));

    merge_counter_stats(&mut stored, &delta);

    let t = &stored.threads["main"];
    assert_eq!(t.posts, 5041, "the delta adds to history, it does not become it");
    assert_eq!(t.counts, 4837);
    assert_eq!(
        t.first_count.as_deref(),
        Some("main/first-uuid"),
        "first count is the earliest ever, not the newest seen"
    );
}

#[test]
fn incremental_can_introduce_a_thread_the_counter_had_not_posted_in() {
    let mut stored = cs("main", 100, 90, Some("main/a"));
    let delta = cs("binary", 7, 7, Some("binary/b"));
    merge_counter_stats(&mut stored, &delta);
    assert_eq!(stored.threads["main"].posts, 100, "untouched thread is preserved");
    assert_eq!(stored.threads["binary"].posts, 7, "new thread is added");
}

#[test]
fn merging_into_an_empty_base_is_just_the_delta() {
    let mut base = CounterStats::default();
    merge_counter_stats(&mut base, &cs("main", 3, 3, Some("main/x")));
    assert_eq!(base.threads["main"].posts, 3);
    assert_eq!(base.threads["main"].first_count.as_deref(), Some("main/x"));
}

#[test]
fn repeated_merges_accumulate() {
    let mut stored = CounterStats::default();
    for _ in 0..60 {
        merge_counter_stats(&mut stored, &cs("main", 10, 9, Some("main/x")));
    }
    assert_eq!(stored.threads["main"].posts, 600);
    assert_eq!(stored.threads["main"].counts, 540);
}

fn make_resetting_thread() -> Thread {
    Thread {
        id: 72,
        uuid: "thread-uuid".to_string(),
        name: "fast_tslc".to_string(),
        validation_type: "fast_tslc".to_string(),
        reset_on_mistakes: true,
        first_valid_count: "1".to_string(),
        counts_per_split: 10,
        splits_per_get: 5,
        split_offset: 0,
        allow_double_posts: false,
        created_at_ms: Some(0),
    }
}

fn uuid_at_nanos(ns: i64) -> String {
    let d: Vec<char> = format!("{ns:019}").chars().collect();
    format!(
        "{}-{}-4{}-8{}-{}00000000000",
        d[0..8].iter().collect::<String>(),
        d[8..12].iter().collect::<String>(),
        d[12..15].iter().collect::<String>(),
        d[15..18].iter().collect::<String>(),
        d[18],
    )
}

const T0: i64 = 1_677_543_400_000_000_000;

fn speed_post_at(id: u64, ns: i64, author: &str, vcn: i64, raw: &str) -> Post {
    let uuid = uuid_at_nanos(ns);
    Post {
        id,
        uuid,
        ..speed_post(id, UUID_A, author, vcn, raw)
    }
}

fn mistake_post(id: u64, uuid: &str, author: &str) -> Post {
    Post {
        id,
        uuid: uuid.to_string(),
        thread: "thread-uuid".to_string(),
        author_uuid: author.to_string(),
        timestamp: "0".to_string(),
        is_count: true,
        is_valid_count: Some(false),
        valid_count_number: None,
        raw_count: Some("nope".to_string()),
        has_comment: false,
    }
}

#[test]
fn a_get_on_a_resetting_thread_is_read_off_the_displayed_count() {
    let thread = make_resetting_thread();
    let showing_three = speed_post(9000, UUID_A, "alice", 5500, "3");
    let showing_fifty = speed_post(9001, UUID_B, "bob", 5501, "50");

    assert_eq!(
        thread.milestone_number(&showing_three),
        Some(3),
        "the displayed count is the milestone here, not 5500"
    );
    assert!(!thread.is_get(thread.milestone_number(&showing_three).unwrap()));
    assert!(
        thread.is_get(thread.milestone_number(&showing_fifty).unwrap()),
        "50 displayed counts is a get however deep the thread is"
    );

    let plain = make_thread();
    assert_eq!(plain.milestone_number(&showing_three), Some(5500));
}

#[test]
fn a_split_whose_window_crosses_a_reset_is_dropped() {
    let thread = make_resetting_thread();

    let posts = vec![
        speed_post_at(100, T0, "alice", 4870, "2"),
        mistake_post(101, UUID_B, "bob"),
        speed_post_at(102, T0 + 200_000_000, "bob", 4871, "1"),
        speed_post_at(103, T0 + 505_000_000, "alice", 4880, "2"),
    ];
    let splits = vec![posts[3].clone()];

    let result = compute_split_speed(&posts, &[], &splits, &thread, &HashMap::new(), None);
    assert!(
        result.is_empty(),
        "2 -> reset -> 2 is not ten counts in a row: got {result:?}"
    );
}

#[test]
fn an_unbroken_split_on_a_resetting_thread_still_counts() {
    let thread = make_resetting_thread();

    let mut posts = vec![speed_post_at(200, T0, "alice", 2720, "36")];
    for i in 1..=10 {
        posts.push(speed_post_at(
            200 + i as u64,
            T0 + i * 1_000_000_000,
            if i % 2 == 0 { "bob" } else { "alice" },
            2720 + i,
            &(36 + i).to_string(),
        ));
    }
    let splits = vec![posts[10].clone()];

    let result = compute_split_speed(&posts, &[], &splits, &thread, &HashMap::new(), None);
    assert_eq!(
        result.len(),
        1,
        "36 -> 46 with no mistake is a real split: got {result:?}"
    );
}

#[test]
fn the_first_split_of_a_run_is_timed_from_the_reset() {
    let thread = make_resetting_thread();

    let mut posts = vec![
        speed_post_at(300, T0, "alice", 900, "27"),
        mistake_post(301, UUID_B, "bob"),
    ];
    for i in 1..=10 {
        posts.push(speed_post_at(
            301 + i as u64,
            T0 + (i + 1) * 1_000_000_000,
            if i % 2 == 0 { "bob" } else { "alice" },
            900 + i,
            &i.to_string(),
        ));
    }
    let splits = vec![posts[11].clone()];

    let result = compute_split_speed(&posts, &[], &splits, &thread, &HashMap::new(), None);
    assert_eq!(result.len(), 1, "a clean 1..10 run is a split: got {result:?}");
}

#[test]
fn displayed_counts_parse_only_when_they_are_numbers() {
    use crate::models::displayed_count;
    assert_eq!(displayed_count("50"), Some(50));
    assert_eq!(displayed_count("1,234"), Some(1234));
    assert_eq!(displayed_count(" 7 "), Some(7));
    assert_eq!(displayed_count("XLII"), None, "roman numerals are not decimal");
    assert_eq!(displayed_count(""), None);
}

#[test]
fn prune_keeps_each_counters_fastest_not_their_first() {
    use crate::speed::prune_split_records;
    let mut v: Vec<SplitSpeedEntry> = (0..5)
        .map(|i| spl(i * 50, i * 50 + 50, 20_000_000 - i, false, &[CA]))
        .collect();
    v.push(spl(1000, 1050, 8_411_000, false, &[CA]));
    prune_split_records(&mut v, 3);
    assert_eq!(v.len(), 3);
    assert!(v.iter().any(|r| r.time == 8_411_000), "the record survives");
}

#[test]
fn prune_keeps_a_split_any_credited_counter_still_wants() {
    use crate::speed::prune_split_records;
    let mut v = vec![
        spl(0, 50, 1_000, false, &[CA]),
        spl(50, 100, 2_000, false, &[CA]),
        spl(100, 150, 3_000, false, &[CA, CB]),
    ];
    prune_split_records(&mut v, 2);
    assert_eq!(v.len(), 3);
}

#[test]
fn prune_gives_fake_and_real_separate_allowances() {
    use crate::speed::prune_split_records;
    let mut v = vec![
        spl(0, 50, 1_000, true, &[CA]),
        spl(50, 100, 1_100, true, &[CA]),
        spl(100, 150, 9_000, false, &[CA]),
    ];
    prune_split_records(&mut v, 1);
    assert!(v.iter().any(|r| !r.is_fake), "fakes cannot crowd out the real run");
    assert_eq!(v.iter().filter(|r| r.is_fake).count(), 1);
}

#[test]
fn prune_leaves_lists_under_the_cap_untouched_and_in_order() {
    use crate::speed::prune_split_records;
    let mut v = vec![spl(0, 50, 5_000, false, &[CA]), spl(50, 100, 1_000, false, &[CA])];
    prune_split_records(&mut v, 100);
    assert_eq!(v.iter().map(|r| r.time).collect::<Vec<_>>(), vec![5_000, 1_000]);
}
