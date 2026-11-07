use ahash::AHashMap as HashMap;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct Post {
    pub id: u64,
    pub uuid: String,
    pub thread: String,
    pub author_uuid: String,
    pub timestamp: String,
    pub is_count: bool,
    pub is_valid_count: Option<bool>,
    pub valid_count_number: Option<i64>,
    pub raw_count: Option<String>,
    pub has_comment: bool,
}

#[derive(Debug, Clone)]
pub struct Thread {
    pub id: i32,
    pub uuid: String,
    pub name: String,
    pub validation_type: String,
    pub reset_on_mistakes: bool,
    pub first_valid_count: String,
    pub counts_per_split: i32,
    pub splits_per_get: i32,
    pub split_offset: i32,
    pub allow_double_posts: bool,
    pub created_at_ms: Option<i64>,
}

pub fn displayed_count(raw: &str) -> Option<i64> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut digits = String::with_capacity(trimmed.len());
    for ch in trimmed.chars() {
        match ch {
            '0'..='9' => digits.push(ch),
            ',' | '_' | ' ' => {}
            _ => return None,
        }
    }
    digits.parse::<i64>().ok()
}

impl Thread {
    pub fn get_size(&self) -> i64 {
        self.counts_per_split as i64 * self.splits_per_get as i64
    }

    pub fn milestone_number(&self, post: &Post) -> Option<i64> {
        if self.reset_on_mistakes {
            post.raw_count.as_deref().and_then(displayed_count)
        } else {
            post.valid_count_number
        }
    }

    pub fn is_get(&self, valid_count_number: i64) -> bool {
        if self.split_offset == 0 {
            valid_count_number % self.get_size() == 0
        } else {
            (valid_count_number - self.split_offset as i64) % self.get_size() == 0
                && valid_count_number > self.split_offset as i64
        }
    }

    pub fn is_split(&self, valid_count_number: i64) -> bool {
        if self.split_offset == 0 {
            valid_count_number % self.counts_per_split as i64 == 0
        } else {
            (valid_count_number - self.split_offset as i64) % self.counts_per_split as i64 == 0
                && valid_count_number > self.split_offset as i64
        }
    }
}

#[derive(Debug, Clone)]
pub struct Counter {
    pub id: i32,
    pub uuid: String,
    pub username: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ThreadStats {
    pub leaderboard: HashMap<String, i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gets: Option<HashMap<String, i64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assists: Option<HashMap<String, i64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub palindromes: Option<HashMap<String, i64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repdigits: Option<HashMap<String, i64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accuracy: Option<Vec<AccuracyEntry>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed: Option<Vec<SpeedEntry>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub split_speed: Option<Vec<SplitSpeedEntry>>,
    #[serde(alias = "last_updated")]
    pub last_updated: u64,
    #[serde(alias = "last_updated_uuid")]
    pub last_updated_uuid: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time: Option<HashMap<String, i64>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccuracyEntry {
    pub counter: String,
    pub counts: i64,
    pub mistakes: i64,
    pub accuracy: f64,
    pub score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeedEntry {
    pub start: String,
    pub end: String,
    pub time: i64,
    pub start_count: String,
    pub end_count: String,
    pub start_count_number: i64,
    pub end_count_number: i64,
    pub qualified_counters: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SplitSpeedEntry {
    pub start: String,
    pub end: String,
    pub time: i64,
    pub start_count: String,
    pub end_count: String,
    pub start_count_number: i64,
    pub end_count_number: i64,
    pub qualified_counters: Vec<String>,
    pub is_fake: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct AllStats {
    pub leaderboard: HashMap<String, i64>,
    #[serde(alias = "last_updated")]
    pub last_updated: u64,
    #[serde(alias = "last_updated_uuid")]
    pub last_updated_uuid: String,
    #[serde(skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub time: HashMap<String, i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CounterThreadStats {
    pub posts: i64,
    pub counts: i64,
    pub mistakes: i64,
    pub odds: i64,
    pub evens: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_count: Option<String>,
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub ranks: std::collections::HashMap<String, i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub best_speed_us: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub best_split_us: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub best_speed_start: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub best_speed_end: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub best_split_start: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub best_split_end: Option<i64>,
    #[serde(default)]
    pub seconds_counted: i64,
    #[serde(default)]
    pub seconds_posted: i64,
    #[serde(default)]
    pub last_posted_second: i64,
    #[serde(skip)]
    pub first_posted_second: i64,
    #[serde(default)]
    pub last_second: i64,
    #[serde(skip)]
    pub first_second: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CounterStats {
    pub threads: HashMap<String, CounterThreadStats>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DynamicStatParams {
    pub thread_uuid: String,
    pub split_size: u32,
    pub split_offset: u32,
    pub min_posts_qualification: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DynamicStatRow {
    pub start_uuid: String,
    pub end_uuid: String,
    pub time_diff_ms: Option<i64>,
    pub start_valid_count_number: Option<i64>,
    pub end_valid_count_number: Option<i64>,
    pub qualified_counters: Vec<String>,
}

pub fn combine_leaderboards(base: &mut HashMap<String, i64>, incoming: &HashMap<String, i64>) {
    for (k, v) in incoming {
        *base.entry(k.clone()).or_insert(0) += v;
    }
}

pub fn leaderboard_from_posts(posts: &[Post]) -> HashMap<String, i64> {
    let mut lb = HashMap::new();
    for post in posts {
        if post.is_valid_count == Some(true) {
            *lb.entry(post.author_uuid.clone()).or_insert(0) += 1;
        }
    }
    lb
}

pub fn is_palindrome(s: &str) -> bool {
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    if n == 0 {
        return false;
    }
    for i in 0..n / 2 {
        if chars[i] != chars[n - 1 - i] {
            return false;
        }
    }
    true
}

pub fn is_repdigit(s: &str) -> bool {
    if s.parse::<u64>().map(|n| n < 10).unwrap_or(false) {
        return true;
    }
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= 1 {
        return false;
    }
    chars.iter().all(|&c| c == chars[0])
}

pub fn uuid_parse_to_nano(uuid: &str) -> i64 {
    const SKIP: [usize; 6] = [8, 13, 14, 18, 19, 23];
    const NANO_DIGITS: usize = 19;

    let mut digits = String::with_capacity(NANO_DIGITS);
    for (char_pos, ch) in uuid.chars().enumerate() {
        if digits.len() == NANO_DIGITS {
            break;
        }
        if !SKIP.contains(&char_pos) {
            digits.push(ch);
        }
    }
    digits.parse().unwrap_or(0)
}

pub fn uuid_parse_ms(uuid: &str) -> Option<i64> {
    const SKIP: [usize; 6] = [8, 13, 14, 18, 19, 23];
    const NANO_DIGITS: usize = 19;

    let mut digits = String::with_capacity(NANO_DIGITS);
    for (char_pos, ch) in uuid.chars().enumerate() {
        if digits.len() == NANO_DIGITS {
            break;
        }
        if !SKIP.contains(&char_pos) {
            digits.push(ch);
        }
    }

    if digits.len() != NANO_DIGITS {
        return None;
    }
    let nanos: i64 = digits.parse().ok()?;
    Some(nanos / 1_000_000)
}
