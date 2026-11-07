use crate::app::{App, now_ms};
use crate::daily::eastern_day_key;
use crate::model::{Counter, Post, ThreadMeta};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use parking_lot::Mutex;
use serde_json::{Map, Value, json};
use socketioxide::extract::SocketRef;
use sqlx::mysql::MySqlRow;
use sqlx::{MySqlPool, Row};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

pub const RANK_ORDER: [&str; 9] = [
    "bronze",
    "silver",
    "gold",
    "platinum",
    "emerald",
    "diamond",
    "countmeister",
    "grandcounter",
    "peak",
];
const LRWOED_PSEUDO_THREAD: &str = "00000000-0000-0000-0000-000000000001";
const SHUFFLE_PSEUDO_THREAD: &str = "00000000-0000-0000-0000-000000000002";
const BINGO_ONLY_TYPES: [&str; 1] = ["thread_breadth"];

fn rank_idx(r: &str) -> isize {
    RANK_ORDER.iter().position(|x| *x == r).map_or(-1, |i| i as isize)
}

fn gg_per_rank(r: &str) -> Option<i64> {
    Some(match r {
        "bronze" => 300,
        "silver" => 700,
        "gold" => 1400,
        "platinum" => 2500,
        "emerald" => 4000,
        "diamond" => 6500,
        "countmeister" => 10000,
        "grandcounter" => 16000,
        _ => return None,
    })
}

pub fn division_from_gg(rank: &str, gg: i64) -> i64 {
    let Some(total) = gg_per_rank(rank) else { return 3 };
    let per = total / 3;
    if gg < per {
        1
    } else if gg < per * 2 {
        2
    } else {
        3
    }
}

fn next_rank(r: &str) -> Option<&'static str> {
    let i = rank_idx(r);
    if i < 0 || i as usize == RANK_ORDER.len() - 1 {
        None
    } else {
        Some(RANK_ORDER[i as usize + 1])
    }
}

pub fn repeat_penalty(base: i64, prior: i64) -> i64 {
    let factor = if prior <= 0 {
        1.0
    } else {
        (1.0 / (1.0 + 0.25 * prior as f64)).max(0.25)
    };
    ((base as f64 * factor).round() as i64).max(1)
}

fn chain_key(c: &Template) -> String {
    let scope = c.thread_uuid.as_deref().unwrap_or("sitewide");
    let p = |k: &str| {
        c.params
            .as_ref()
            .and_then(|p| p.get(k))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_owned()
    };
    match c.kind.as_str() {
        "thread_counts" | "counts_in_day" | "time_counting" | "count_attempts" | "strict_counts" | "gets_landed"
        | "assists_landed" | "thread_spread" | "fast_gets" | "lrwoed_days" | "daily_shuffle_wins" => {
            format!("{}:{scope}", c.kind)
        }
        "accuracy_rate" | "split_under_ms" | "get_under_ms" | "bars_within_ms" => {
            format!("{}:{scope}:{}", c.kind, p("validationType"))
        }
        "roll" => format!("{}:{scope}:{}", c.kind, p("direction")),
        "lrwoed_score" | "number_shuffle_win" => format!("{}:{}", c.kind, p("validationType")),
        "bingo_win" | "bingo_complete_squares" => format!("{}:{}", c.kind, c.bingo_category.as_deref().unwrap_or("")),
        other => other.to_owned(),
    }
}

fn window_decode(w: Option<&str>) -> (usize, Vec<u8>) {
    let Some(w) = w else { return (0, vec![]) };
    let Some((n, b64)) = w.split_once(':') else {
        return (0, vec![]);
    };
    let total = counting::js::parse_int(n, Some(10));
    if !total.is_finite() || total < 0.0 {
        return (0, vec![]);
    }
    let clean: String = b64
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '+' || *c == '/')
        .collect();
    let buf = base64::engine::GeneralPurpose::new(
        &base64::alphabet::STANDARD,
        base64::engine::GeneralPurposeConfig::new()
            .with_decode_padding_mode(base64::engine::DecodePaddingMode::Indifferent)
            .with_decode_allow_trailing_bits(true),
    )
    .decode(clean.as_bytes())
    .unwrap_or_default();
    (total as usize, buf)
}

fn bit_pos(len: usize, k: usize) -> (usize, usize) {
    (len - 1 - k / 8, k % 8)
}

pub fn window_append(w: Option<&str>, valid: bool, max_len: usize) -> String {
    let (mut total, mut buf) = window_decode(w);
    let byte_len = max_len.div_ceil(8);
    if buf.len() < byte_len {
        let mut grown = vec![0u8; byte_len - buf.len()];
        grown.extend_from_slice(&buf);
        buf = grown;
    }
    if total < max_len {
        total += 1;
    } else {
        for k in 0..max_len.saturating_sub(1) {
            let (fb, fp) = bit_pos(buf.len(), k + 1);
            let (tb, tp) = bit_pos(buf.len(), k);
            if (buf[fb] >> fp) & 1 == 1 {
                buf[tb] |= 1 << tp;
            } else {
                buf[tb] &= !(1 << tp);
            }
        }
    }
    if total > 0 {
        let (b, p) = bit_pos(buf.len(), total - 1);
        if b < buf.len() {
            if valid {
                buf[b] |= 1 << p;
            } else {
                buf[b] &= !(1 << p);
            }
        }
    }
    format!("{total}:{}", STANDARD.encode(&buf))
}

pub fn window_valid(w: Option<&str>, size: usize) -> Option<usize> {
    let (total, buf) = window_decode(w);
    if total < size || size == 0 {
        return None;
    }
    Some(
        (total - size..total)
            .filter(|&k| {
                let (b, p) = bit_pos(buf.len(), k);
                b < buf.len() && buf[b] & (1 << p) != 0
            })
            .count(),
    )
}

pub fn accuracy_needed(size: usize, min_percent: f64) -> usize {
    ((min_percent / 100.0) * size as f64 - 1e-6).ceil().max(0.0) as usize
}

pub fn window_pct(w: Option<&str>, size: usize) -> Option<f64> {
    let (total, buf) = window_decode(w);
    if total < size {
        return None;
    }
    let eff = total.min(size);
    if eff == 0 {
        return None;
    }
    let mut n = 0;
    for k in total - eff..total {
        let (b, p) = bit_pos(buf.len(), k);
        if b < buf.len() && buf[b] & (1 << p) != 0 {
            n += 1;
        }
    }
    Some(((n as f64 / eff as f64) * 100.0).round())
}

fn tally_thread(log: &mut Log, thread: &str, n: i64, score: impl Fn(&Map<String, Value>) -> i64) -> i64 {
    let mut counts: Map<String, Value> = log
        .accuracy_window
        .as_deref()
        .and_then(|w| serde_json::from_str(w).ok())
        .unwrap_or_default();
    let v = counts.get(thread).and_then(Value::as_i64).unwrap_or(0) + n;
    counts.insert(thread.to_owned(), json!(v));
    log.accuracy_window = Some(Value::Object(counts.clone()).to_string());
    score(&counts)
}

fn nth_largest(counts: &Map<String, Value>, n: usize) -> i64 {
    let mut v: Vec<i64> = counts.values().filter_map(Value::as_i64).collect();
    v.sort_unstable_by(|a, b| b.cmp(a));
    v.get(n - 1).copied().unwrap_or(0)
}

fn day_number(ms: i64) -> i64 {
    eastern_day_key(ms).replace('-', "").parse().unwrap_or(0)
}

fn param_f(params: &Option<Value>, k: &str) -> Option<f64> {
    params.as_ref().and_then(|p| p.get(k)).and_then(Value::as_f64)
}

fn roll_qualifies(roll: f64, params: &Option<Value>) -> bool {
    let dir = params.as_ref().and_then(|p| p.get("direction")).and_then(Value::as_str);
    if dir == Some("over") {
        roll >= param_f(params, "threshold").unwrap_or(f64::INFINITY)
    } else {
        roll <= param_f(params, "threshold").unwrap_or(0.0)
    }
}

fn under_max_ms(ms: f64, params: &Option<Value>) -> bool {
    ms <= param_f(params, "maxMs").unwrap_or(f64::INFINITY)
}

fn validation_type_ok(params: &Option<Value>, vt: &str) -> bool {
    match params
        .as_ref()
        .and_then(|p| p.get("validationType"))
        .and_then(Value::as_str)
    {
        Some(req) if !req.is_empty() => req == vt,
        _ => true,
    }
}

#[derive(Clone, Debug)]
pub struct Template {
    pub id: String,
    pub kind: String,
    pub rank: String,
    pub sequence: i64,
    pub gg_reward: i64,
    pub reward_type: String,
    pub target: i64,
    pub params: Option<Value>,
    pub thread_uuid: Option<String>,
    pub bingo_category: Option<String>,
    pub season_id: i64,
    pub base_wait_ms: Option<i64>,
}

#[derive(Clone, Debug)]
struct Log {
    id: i64,
    counter_uuid: String,
    challenge_id: String,
    season_id: Option<i64>,
    context: String,
    bingo_game_id: Option<i64>,
    progress: i64,
    target: i64,
    last_credited_second: Option<i64>,
    accuracy_window: Option<String>,
    completed_at: Option<i64>,
    cancelled_at: Option<i64>,
    gg_reward: i64,
    gg_awarded: i64,
    assignment_ordinal: i64,
    created_at: i64,
    updated_at: i64,
    seen_at: Option<i64>,
    entrance_seen_at: Option<i64>,
    available_at: Option<i64>,
}

#[derive(Clone, Debug)]
struct RankRow {
    id: i64,
    counter_uuid: String,
    thread_uuid: Option<String>,
    season_id: Option<i64>,
    rank: String,
    division: i64,
    gg: i64,
    gg_total: i64,
}

fn i64_of(r: &MySqlRow, c: &str) -> Option<i64> {
    r.try_get::<Option<i64>, _>(c)
        .ok()
        .flatten()
        .or_else(|| r.try_get::<Option<i32>, _>(c).ok().flatten().map(i64::from))
}

fn log_from(r: &MySqlRow) -> Log {
    Log {
        id: i64_of(r, "id").unwrap_or(0),
        counter_uuid: r.get("counterUuid"),
        challenge_id: r.get("challengeId"),
        season_id: i64_of(r, "seasonId"),
        context: r.get("context"),
        bingo_game_id: i64_of(r, "bingoGameId"),
        progress: i64_of(r, "progress").unwrap_or(0),
        target: i64_of(r, "target").unwrap_or(0),
        last_credited_second: i64_of(r, "lastCreditedSecond"),
        accuracy_window: r.get("accuracyWindow"),
        completed_at: i64_of(r, "completedAt"),
        cancelled_at: i64_of(r, "cancelledAt"),
        gg_reward: i64_of(r, "ggReward").unwrap_or(0),
        gg_awarded: i64_of(r, "ggAwarded").unwrap_or(0),
        assignment_ordinal: i64_of(r, "assignmentOrdinal").unwrap_or(0),
        created_at: i64_of(r, "createdAt").unwrap_or(0),
        updated_at: i64_of(r, "updatedAt").unwrap_or(0),
        seen_at: i64_of(r, "seenAt"),
        entrance_seen_at: i64_of(r, "entranceSeenAt"),
        available_at: i64_of(r, "availableAt"),
    }
}

fn rank_from(r: &MySqlRow) -> RankRow {
    RankRow {
        id: i64_of(r, "id").unwrap_or(0),
        counter_uuid: r.get("counterUuid"),
        thread_uuid: r.get("threadUuid"),
        season_id: i64_of(r, "seasonId"),
        rank: r.get("rank"),
        division: i64_of(r, "division").unwrap_or(1),
        gg: i64_of(r, "gg").unwrap_or(0),
        gg_total: i64_of(r, "ggTotal").unwrap_or(0),
    }
}

impl Log {
    fn json(&self) -> Value {
        json!({
            "id": self.id, "counterUuid": self.counter_uuid, "challengeId": self.challenge_id,
            "seasonId": self.season_id, "context": self.context, "bingoGameId": self.bingo_game_id,
            "progress": self.progress, "target": self.target, "lastCreditedSecond": self.last_credited_second,
            "accuracyWindow": self.accuracy_window, "completedAt": self.completed_at,
            "cancelledAt": self.cancelled_at, "ggReward": self.gg_reward, "ggAwarded": self.gg_awarded,
            "assignmentOrdinal": self.assignment_ordinal, "createdAt": self.created_at,
            "updatedAt": self.updated_at, "seenAt": self.seen_at, "entranceSeenAt": self.entrance_seen_at,
            "availableAt": self.available_at,
        })
    }
}

impl RankRow {
    fn json(&self) -> Value {
        json!({
            "id": self.id, "counterUuid": self.counter_uuid, "threadUuid": self.thread_uuid,
            "seasonId": self.season_id, "rank": self.rank, "division": self.division, "gg": self.gg,
            "ggTotal": self.gg_total,
        })
    }
}

const LOG_COLS: &str = "id, counterUuid, challengeId, seasonId, context, bingoGameId, progress, target, \
    lastCreditedSecond, accuracyWindow, completedAt, cancelledAt, ggReward, ggAwarded, assignmentOrdinal, \
    createdAt, updatedAt, seenAt, entranceSeenAt, availableAt";

struct Season {
    id: Option<i64>,
    at: Instant,
}

struct Templates {
    season: i64,
    all: Vec<Template>,
    at: Instant,
}

static SEASON: LazyLock<Mutex<Option<Season>>> = LazyLock::new(|| Mutex::new(None));
static TEMPLATES: LazyLock<Mutex<Option<Templates>>> = LazyLock::new(|| Mutex::new(None));
static TIME_TALLY: LazyLock<Mutex<HashMap<String, (i64, i64)>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

async fn active_season(pool: &MySqlPool) -> Option<i64> {
    if let Some(s) = SEASON
        .lock()
        .as_ref()
        .filter(|s| s.at.elapsed() < Duration::from_secs(60))
    {
        return s.id;
    }
    let id: Option<i64> = sqlx::query_scalar("SELECT CAST(id AS SIGNED) FROM rankseason WHERE endedAt IS NULL LIMIT 1")
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
    *SEASON.lock() = Some(Season { id, at: Instant::now() });
    id
}

async fn templates(pool: &MySqlPool, season: i64) -> Vec<Template> {
    if let Some(t) = TEMPLATES
        .lock()
        .as_ref()
        .filter(|t| t.season == season && t.at.elapsed() < Duration::from_secs(60))
    {
        return t.all.clone();
    }
    let rows = sqlx::query(
        "SELECT id, type, `rank`, sequence, ggReward, rewardType, target, CAST(params AS CHAR) AS params, threadUuid, \
         bingoCategory, seasonId, baseReassignmentWaitMs FROM rankchallenge WHERE seasonId = ? \
         ORDER BY `rank` ASC, sequence ASC, id ASC",
    )
    .bind(season)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    let all: Vec<Template> = rows
        .iter()
        .map(|r| Template {
            id: r.get("id"),
            kind: r.get("type"),
            rank: r.get("rank"),
            sequence: i64_of(r, "sequence").unwrap_or(0),
            gg_reward: i64_of(r, "ggReward").unwrap_or(0),
            reward_type: r.try_get::<String, _>("rewardType").unwrap_or_else(|_| "gg".into()),
            target: i64_of(r, "target").unwrap_or(0),
            params: r
                .get::<Option<String>, _>("params")
                .and_then(|p| serde_json::from_str(&p).ok()),
            thread_uuid: r.get("threadUuid"),
            bingo_category: r.get("bingoCategory"),
            season_id: i64_of(r, "seasonId").unwrap_or(0),
            base_wait_ms: i64_of(r, "baseReassignmentWaitMs"),
        })
        .collect();
    *TEMPLATES.lock() = Some(Templates {
        season,
        all: all.clone(),
        at: Instant::now(),
    });
    all
}

async fn with_lock<T>(app: &App, counter: &str, f: impl Future<Output = T>) -> T {
    crate::lock::run(app, "rankeval", counter, f).await
}

#[derive(Default)]
struct Collector {
    logs: indexmap::IndexMap<i64, Log>,
    ranks: Vec<RankRow>,
    rank_ups: Vec<Value>,
}

impl Collector {
    fn add_log(&mut self, l: &Log) {
        self.logs.insert(l.id, l.clone());
    }
    fn add_rank(&mut self, r: &RankRow) {
        let key = r.thread_uuid.clone();
        self.ranks.retain(|x| x.thread_uuid != key);
        self.ranks.push(r.clone());
    }
    fn is_empty(&self) -> bool {
        self.logs.is_empty() && self.ranks.is_empty() && self.rank_ups.is_empty()
    }
}

async fn build_delta(pool: &MySqlPool, season: i64, c: &Collector) -> Value {
    let all = templates(pool, season).await;
    let by_id: HashMap<&str, &Template> = all.iter().map(|t| (t.id.as_str(), t)).collect();
    let (mut progress, mut completions) = (vec![], vec![]);
    for log in c.logs.values() {
        let mut j = log.json();
        let t = by_id.get(log.challenge_id.as_str());
        let (pos, total) = t.map_or((1, 1), |t| {
            let mut chain: Vec<&Template> = all
                .iter()
                .filter(|s| {
                    s.kind == t.kind && s.rank == t.rank && s.season_id == t.season_id && chain_key(s) == chain_key(t)
                })
                .collect();
            chain.sort_by_key(|s| s.sequence);
            let pos = chain.iter().position(|s| s.id == t.id).map_or(0, |p| p + 1);
            (pos.max(1), chain.len().max(1))
        });
        j["type"] = json!(t.map_or("thread_counts", |t| t.kind.as_str()));
        j["params"] = t.and_then(|t| t.params.clone()).unwrap_or(Value::Null);
        j["threadUuid"] = json!(t.and_then(|t| t.thread_uuid.clone()));
        j["rank"] = json!(t.map_or("bronze", |t| t.rank.as_str()));
        j["sequencePosition"] = json!(pos);
        j["sequenceTotal"] = json!(total);
        j["baseReassignmentWaitMs"] = json!(t.and_then(|t| t.base_wait_ms));
        if log.completed_at.is_some() {
            completions.push(j)
        } else {
            progress.push(j)
        }
    }
    let thread_rank = c.ranks.iter().find(|r| r.thread_uuid.is_some()).map(|r| r.json());
    let sitewide = c.ranks.iter().find(|r| r.thread_uuid.is_none()).map(|r| r.json());
    json!({
        "progress": progress,
        "completions": completions,
        "threadRank": thread_rank,
        "sitewideRank": sitewide,
        "rankUpEvents": c.rank_ups,
    })
}

struct Engine<'a> {
    pool: &'a MySqlPool,
    season: i64,
    all: Vec<Template>,
}

impl<'a> Engine<'a> {
    fn of(&self, kind: &str, thread: Option<&str>) -> Vec<Template> {
        self.all
            .iter()
            .filter(|t| t.kind == kind && t.thread_uuid.as_deref() == thread)
            .cloned()
            .collect()
    }

    async fn open_logs(&self, counter: &str, ids: &[String]) -> anyhow::Result<Vec<Log>> {
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let marks = vec!["?"; ids.len()].join(",");
        let sql = format!(
            "SELECT {LOG_COLS} FROM challengelog WHERE counterUuid = ? AND challengeId IN ({marks}) AND seasonId = ? \
             AND completedAt IS NULL AND cancelledAt IS NULL ORDER BY id"
        );
        let mut q = sqlx::query(&sql).bind(counter);
        for i in ids {
            q = q.bind(i);
        }
        Ok(q.bind(self.season)
            .fetch_all(self.pool)
            .await?
            .iter()
            .map(log_from)
            .collect())
    }

    async fn save_log(&self, l: &Log) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE challengelog SET progress = ?, updatedAt = ?, lastCreditedSecond = ?, accuracyWindow = ?, \
             completedAt = ?, cancelledAt = ?, ggAwarded = ? WHERE id = ?",
        )
        .bind(l.progress)
        .bind(l.updated_at)
        .bind(l.last_credited_second)
        .bind(&l.accuracy_window)
        .bind(l.completed_at)
        .bind(l.cancelled_at)
        .bind(l.gg_awarded)
        .bind(l.id)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    async fn find_rank(&self, counter: &str, thread: Option<&str>) -> anyhow::Result<Option<RankRow>> {
        let r = sqlx::query(
            "SELECT id, counterUuid, threadUuid, seasonId, `rank`, division, gg, ggTotal FROM threadrank \
             WHERE counterUuid = ? AND threadUuid <=> ? AND seasonId = ? LIMIT 1",
        )
        .bind(counter)
        .bind(thread)
        .bind(self.season)
        .fetch_optional(self.pool)
        .await?;
        Ok(r.as_ref().map(rank_from))
    }

    async fn insert_rank(&self, counter: &str, thread: Option<&str>) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO threadrank (counterUuid, threadUuid, seasonId, `rank`, division, gg, ggTotal) VALUES (?, ?, ?, 'bronze', 1, 0, 0)",
        )
        .bind(counter)
        .bind(thread)
        .bind(self.season)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    async fn upsert_rank(&self, counter: &str, thread: Option<&str>) -> anyhow::Result<RankRow> {
        if let Some(r) = self.find_rank(counter, thread).await? {
            return Ok(r);
        }
        if let Ok(()) = self.insert_rank(counter, thread).await
            && let Some(t) = thread
        {
            Box::pin(self.assign_for_counter_thread(counter, t)).await?;
        }
        self.find_rank(counter, thread)
            .await?
            .ok_or_else(|| anyhow::anyhow!("ThreadRank upsert failed unexpectedly"))
    }

    async fn save_rank(&self, r: &RankRow) -> anyhow::Result<()> {
        sqlx::query("UPDATE threadrank SET `rank` = ?, division = ?, gg = ?, ggTotal = ? WHERE id = ?")
            .bind(&r.rank)
            .bind(r.division)
            .bind(r.gg)
            .bind(r.gg_total)
            .bind(r.id)
            .execute(self.pool)
            .await?;
        Ok(())
    }

    async fn insert_rank_up(&self, r: &RankRow, from: (&str, i64), to: (&str, i64), now: i64) -> anyhow::Result<Value> {
        let res = sqlx::query(
            "INSERT INTO rankupevent (counterUuid, threadUuid, seasonId, fromRank, fromDivision, toRank, toDivision, createdAt, seenAt) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, NULL)",
        )
        .bind(&r.counter_uuid)
        .bind(&r.thread_uuid)
        .bind(r.season_id)
        .bind(from.0)
        .bind(from.1)
        .bind(to.0)
        .bind(to.1)
        .bind(now)
        .execute(self.pool)
        .await?;
        Ok(json!({
            "counterUuid": r.counter_uuid, "threadUuid": r.thread_uuid, "seasonId": r.season_id,
            "fromRank": from.0, "fromDivision": from.1, "toRank": to.0, "toDivision": to.1,
            "createdAt": now, "seenAt": null, "id": res.last_insert_id(),
        }))
    }

    async fn create_fresh(
        &self,
        t: &Template,
        counter: &str,
        gg_reward: i64,
        available_at: Option<i64>,
        ordinal: i64,
    ) -> anyhow::Result<Log> {
        let now = now_ms();
        let r = sqlx::query(
            "INSERT INTO challengelog (counterUuid, challengeId, seasonId, context, bingoGameId, progress, target, \
             accuracyWindow, completedAt, cancelledAt, ggReward, ggAwarded, assignmentOrdinal, availableAt, createdAt, updatedAt) \
             VALUES (?, ?, ?, 'rank', NULL, 0, ?, NULL, NULL, NULL, ?, 0, ?, ?, ?, ?)",
        )
        .bind(counter)
        .bind(&t.id)
        .bind(self.season)
        .bind(t.target)
        .bind(gg_reward)
        .bind(ordinal)
        .bind(available_at)
        .bind(now)
        .bind(now)
        .execute(self.pool)
        .await?;
        if self.find_rank(counter, t.thread_uuid.as_deref()).await?.is_none() {
            let _ = self.insert_rank(counter, t.thread_uuid.as_deref()).await;
        }
        Ok(Log {
            id: r.last_insert_id() as i64,
            counter_uuid: counter.into(),
            challenge_id: t.id.clone(),
            season_id: Some(self.season),
            context: "rank".into(),
            bingo_game_id: None,
            progress: 0,
            target: t.target,
            last_credited_second: None,
            accuracy_window: None,
            completed_at: None,
            cancelled_at: None,
            gg_reward,
            gg_awarded: 0,
            assignment_ordinal: ordinal,
            created_at: now,
            updated_at: now,
            seen_at: None,
            entrance_seen_at: None,
            available_at,
        })
    }

    async fn create_if_missing(&self, t: &Template, counter: &str) -> anyhow::Result<Option<Log>> {
        let open: Option<i64> = sqlx::query_scalar(
            "SELECT CAST(id AS SIGNED) FROM challengelog WHERE counterUuid = ? AND challengeId = ? AND seasonId = ? \
             AND completedAt IS NULL AND cancelledAt IS NULL LIMIT 1",
        )
        .bind(counter)
        .bind(&t.id)
        .bind(self.season)
        .fetch_optional(self.pool)
        .await?;
        if open.is_some() {
            return Ok(None);
        }
        Ok(Some(self.create_fresh(t, counter, t.gg_reward, None, 0).await?))
    }

    async fn assign_auto_jump(&self, counter: &str, below: &str, scope: Scope<'_>) -> anyhow::Result<()> {
        let bi = rank_idx(below);
        let ts: Vec<Template> = self
            .all
            .iter()
            .filter(|t| rank_idx(&t.rank) > bi && t.sequence == 0 && t.reward_type == "auto_jump")
            .filter(|t| match scope {
                Scope::Thread(th) => t.thread_uuid.as_deref() == th,
            })
            .cloned()
            .collect();
        for t in ts {
            self.create_if_missing(&t, counter).await?;
        }
        Ok(())
    }

    async fn assign_for_counter_thread(&self, counter: &str, thread: &str) -> anyhow::Result<()> {
        let ts: Vec<Template> = self
            .all
            .iter()
            .filter(|t| {
                t.rank == "bronze"
                    && t.thread_uuid.as_deref() == Some(thread)
                    && t.sequence == 0
                    && !BINGO_ONLY_TYPES.contains(&t.kind.as_str())
            })
            .cloned()
            .collect();
        for t in ts {
            self.create_if_missing(&t, counter).await?;
        }
        self.assign_auto_jump(counter, "bronze", Scope::Thread(Some(thread)))
            .await
    }

    async fn assign_next(
        &self,
        done: &Template,
        counter: &str,
        current_rank: &str,
        completed_at: Option<i64>,
    ) -> anyhow::Result<Option<Log>> {
        let key = chain_key(done);
        let combo: Vec<&Template> = self
            .all
            .iter()
            .filter(|c| c.kind == done.kind && c.rank == done.rank && chain_key(c) == key)
            .collect();
        if let Some(next) = combo.iter().find(|c| c.sequence == done.sequence + 1) {
            return self.create_if_missing(next, counter).await;
        }
        let Some(highest) = combo
            .iter()
            .copied()
            .reduce(|m, c| if c.sequence > m.sequence { c } else { m })
        else {
            return Ok(None);
        };
        if highest.rank != current_rank {
            let start = self
                .all
                .iter()
                .find(|c| c.kind == done.kind && c.rank == current_rank && chain_key(c) == key && c.sequence == 0);
            return match start {
                Some(s) => self.create_if_missing(s, counter).await,
                None => Ok(None),
            };
        }
        let prior: Option<i64> = sqlx::query_scalar(
            "SELECT CAST(assignmentOrdinal AS SIGNED) FROM challengelog WHERE counterUuid = ? AND challengeId = ? AND seasonId = ? \
             ORDER BY id DESC LIMIT 1",
        )
        .bind(counter)
        .bind(&highest.id)
        .bind(self.season)
        .fetch_optional(self.pool)
        .await?;
        let prior = prior.map_or(0, |o| o + 1);
        let gg = repeat_penalty(highest.gg_reward, prior);
        let available = highest.base_wait_ms.map(|w| completed_at.unwrap_or_else(now_ms) + w);
        Ok(Some(self.create_fresh(highest, counter, gg, available, prior).await?))
    }

    async fn reconcile_on_rank_up(&self, counter: &str, scope: Option<&str>, new_rank: &str) -> anyhow::Result<()> {
        self.assign_auto_jump(counter, new_rank, Scope::Thread(scope)).await?;
        let ni = rank_idx(new_rank);
        let stale: Vec<&Template> = self
            .all
            .iter()
            .filter(|t| rank_idx(&t.rank) < ni && t.thread_uuid.as_deref() == scope)
            .collect();
        if stale.is_empty() {
            return Ok(());
        }
        let ids: Vec<String> = stale.iter().map(|t| t.id.clone()).collect();
        let open = self.open_logs(counter, &ids).await?;
        if open.is_empty() {
            return Ok(());
        }
        let now = now_ms();
        let new_starts: Vec<&Template> = self
            .all
            .iter()
            .filter(|t| {
                t.rank == new_rank
                    && t.thread_uuid.as_deref() == scope
                    && t.sequence == 0
                    && !BINGO_ONLY_TYPES.contains(&t.kind.as_str())
            })
            .collect();
        let mut replaced = std::collections::HashSet::new();
        for mut log in open {
            let Some(t) = stale.iter().find(|t| t.id == log.challenge_id) else {
                continue;
            };
            log.cancelled_at = Some(now);
            log.updated_at = now;
            self.save_log(&log).await?;
            let key = chain_key(t);
            if !replaced.insert(key.clone()) {
                continue;
            }
            if let Some(ns) = new_starts.iter().find(|c| chain_key(c) == key) {
                self.create_if_missing(ns, counter).await?;
            }
        }
        Ok(())
    }

    async fn award_gg(&self, r: &mut RankRow, gg: i64, col: &mut Collector) -> anyhow::Result<()> {
        r.gg_total += gg;
        r.gg += gg;
        let now = now_ms();
        let mut events: Vec<((String, i64), (String, i64))> = Vec::new();
        let mut rank_ups: Vec<(String, String)> = Vec::new();
        loop {
            let from = (r.rank.clone(), r.division);
            match gg_per_rank(&r.rank) {
                None => {
                    if r.division != 3 {
                        events.push((from.clone(), (from.0.clone(), 3)));
                    }
                    r.division = 3;
                    break;
                }
                Some(total) if r.gg >= total => match next_rank(&r.rank) {
                    Some(next) => {
                        let old = r.rank.clone();
                        r.gg = 0;
                        r.rank = next.to_owned();
                        r.division = division_from_gg(next, 0);
                        events.push((from, (next.to_owned(), r.division)));
                        rank_ups.push((old, next.to_owned()));
                        continue;
                    }
                    None => {
                        r.gg = total;
                        r.division = 3;
                        break;
                    }
                },
                Some(_) => {
                    let nd = division_from_gg(&r.rank, r.gg);
                    if nd > r.division {
                        events.push((from.clone(), (from.0.clone(), nd)));
                    }
                    r.division = nd;
                    break;
                }
            }
        }
        for (_, new) in &rank_ups {
            self.reconcile_on_rank_up(&r.counter_uuid, r.thread_uuid.as_deref(), new)
                .await?;
        }
        self.save_rank(r).await?;
        for (from, to) in events {
            let e = self.insert_rank_up(r, (&from.0, from.1), (&to.0, to.1), now).await?;
            col.rank_ups.push(e);
        }
        Ok(())
    }

    async fn award_rank(&self, r: &mut RankRow, target: &str, col: &mut Collector) -> anyhow::Result<()> {
        if rank_idx(target) <= rank_idx(&r.rank) {
            return Ok(());
        }
        let from = (r.rank.clone(), r.division);
        r.rank = target.to_owned();
        r.division = 1;
        r.gg = 0;
        self.save_rank(r).await?;
        let e = self.insert_rank_up(r, (&from.0, from.1), (target, 1), now_ms()).await?;
        col.rank_ups.push(e);
        self.reconcile_on_rank_up(&r.counter_uuid, r.thread_uuid.as_deref(), target)
            .await
    }

    async fn complete(
        &self,
        t: &Template,
        log: &mut Log,
        r: &mut RankRow,
        now: i64,
        col: &mut Collector,
    ) -> anyhow::Result<()> {
        log.completed_at = Some(now);
        log.gg_awarded = log.gg_reward;
        self.save_log(log).await?;
        col.add_log(log);
        if t.reward_type == "auto_jump" {
            self.award_rank(r, &t.rank, col).await?;
            col.add_rank(r);
        } else if r.rank == t.rank {
            self.award_gg(r, log.gg_reward, col).await?;
            col.add_rank(r);
        }
        if t.reward_type != "auto_jump"
            && let Some(next) = self
                .assign_next(t, &log.counter_uuid, &r.rank, log.completed_at)
                .await?
        {
            col.add_log(&next);
        }
        Ok(())
    }

    async fn chain(
        &self,
        ts: &[Template],
        counter: &str,
        r: &mut RankRow,
        credit: Option<(i64, i64)>,
        col: &mut Collector,
    ) -> anyhow::Result<()> {
        if ts.is_empty() {
            return Ok(());
        }
        let ids: Vec<String> = ts.iter().map(|t| t.id.clone()).collect();
        let now = now_ms();
        for mut log in self.open_logs(counter, &ids).await? {
            let Some(t) = ts.iter().find(|t| t.id == log.challenge_id) else {
                continue;
            };
            if log.available_at.is_some_and(|a| now < a) {
                continue;
            }
            if let Some((second, _)) = credit
                && log.last_credited_second == Some(second)
            {
                continue;
            }
            log.progress += credit.map_or(1, |c| c.1);
            log.updated_at = now;
            if let Some((second, _)) = credit {
                log.last_credited_second = Some(second);
            }
            if log.progress >= t.target {
                self.complete(t, &mut log, r, now, col).await?;
            } else {
                self.save_log(&log).await?;
                col.add_log(&log);
            }
        }
        Ok(())
    }

    async fn counts_in_day_chain(
        &self,
        ts: &[Template],
        counter: &str,
        r: &mut RankRow,
        col: &mut Collector,
    ) -> anyhow::Result<()> {
        if ts.is_empty() {
            return Ok(());
        }
        let ids: Vec<String> = ts.iter().map(|t| t.id.clone()).collect();
        let now = now_ms();
        let today = eastern_day_key(now);
        for mut log in self.open_logs(counter, &ids).await? {
            let Some(t) = ts.iter().find(|t| t.id == log.challenge_id) else {
                continue;
            };
            if eastern_day_key(log.updated_at) != today {
                log.progress = 0;
            }
            log.progress += 1;
            log.updated_at = now;
            if log.progress >= t.target {
                self.complete(t, &mut log, r, now, col).await?;
            } else {
                self.save_log(&log).await?;
                col.add_log(&log);
            }
        }
        Ok(())
    }

    async fn thread_counts(&self, counter: &str, thread: &str, col: &mut Collector) -> anyhow::Result<()> {
        let ts = self.of("thread_counts", Some(thread));
        if !ts.is_empty() {
            let mut r = self.upsert_rank(counter, Some(thread)).await?;
            self.chain(&ts, counter, &mut r, None, col).await?;
        }
        Ok(())
    }

    async fn sitewide_counts(&self, counter: &str, thread: &ThreadMeta, col: &mut Collector) -> anyhow::Result<()> {
        let mut ts = self.of("thread_counts", None);
        if !thread.allow_double_posts {
            ts.extend(self.of("strict_counts", None));
        }
        ts.extend(self.of("thread_spread", None));
        if ts.is_empty() {
            return Ok(());
        }
        let mut r = self.upsert_rank(counter, None).await?;
        let ids: Vec<String> = ts.iter().map(|t| t.id.clone()).collect();
        let now = now_ms();
        for mut log in self.open_logs(counter, &ids).await? {
            let Some(t) = ts.iter().find(|t| t.id == log.challenge_id) else {
                continue;
            };
            if log.available_at.is_some_and(|a| now < a) {
                continue;
            }
            if t.kind == "thread_spread" {
                let min = param_f(&t.params, "minThreads").unwrap_or(1.0).max(1.0) as usize;
                log.progress = tally_thread(&mut log, &thread.uuid, 1, |counts| nth_largest(counts, min));
            } else {
                log.progress += 1;
            }
            log.updated_at = now;
            if log.progress >= t.target {
                self.complete(t, &mut log, &mut r, now, col).await?;
            } else {
                self.save_log(&log).await?;
                col.add_log(&log);
            }
        }
        Ok(())
    }

    async fn sitewide_get(
        &self,
        counter: &str,
        thread: &str,
        kinds: &[&str],
        cps: Option<f64>,
        col: &mut Collector,
    ) -> anyhow::Result<()> {
        let mut ts: Vec<Template> = vec![];
        for k in kinds {
            ts.extend(self.of(k, None));
        }
        ts.retain(|t| {
            t.kind != "fast_gets"
                || cps.is_some_and(|c| c >= param_f(&t.params, "minCountsPerSecond").unwrap_or(f64::INFINITY))
        });
        if ts.is_empty() {
            return Ok(());
        }
        let ids: Vec<String> = ts.iter().map(|t| t.id.clone()).collect();
        let logs = self.open_logs(counter, &ids).await?;
        if logs.is_empty() {
            return Ok(());
        }
        let mut r = self.upsert_rank(counter, None).await?;
        let now = now_ms();
        for mut log in logs {
            let Some(t) = ts.iter().find(|t| t.id == log.challenge_id) else {
                continue;
            };
            if log.available_at.is_some_and(|a| now < a) {
                continue;
            }
            let distinct = t
                .params
                .as_ref()
                .and_then(|p| p.get("distinctThreads"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if distinct {
                log.progress = tally_thread(&mut log, thread, 1, |counts| counts.len() as i64);
            } else {
                log.progress += 1;
            }
            log.updated_at = now;
            if log.progress >= t.target {
                self.complete(t, &mut log, &mut r, now, col).await?;
            } else {
                self.save_log(&log).await?;
                col.add_log(&log);
            }
        }
        Ok(())
    }

    async fn count_attempts(&self, counter: &str, thread: &str, col: &mut Collector) -> anyhow::Result<()> {
        let ts = self.of("count_attempts", Some(thread));
        if ts.is_empty() {
            return Ok(());
        }
        let mut r = self.upsert_rank(counter, Some(thread)).await?;
        self.chain(&ts, counter, &mut r, None, col).await
    }

    async fn counts_in_day(&self, counter: &str, thread: &str, col: &mut Collector) -> anyhow::Result<()> {
        let ts = self.of("counts_in_day", Some(thread));
        if !ts.is_empty() {
            let mut r = self.upsert_rank(counter, Some(thread)).await?;
            self.counts_in_day_chain(&ts, counter, &mut r, col).await?;
        }
        let sw = self.of("counts_in_day", None);
        if !sw.is_empty() {
            let mut r = self.upsert_rank(counter, None).await?;
            self.counts_in_day_chain(&sw, counter, &mut r, col).await?;
        }
        Ok(())
    }

    async fn accuracy(
        &self,
        counter: &str,
        thread: &ThreadMeta,
        valid: bool,
        col: &mut Collector,
    ) -> anyhow::Result<()> {
        let mut matching = self.of("accuracy_rate", None);
        matching.extend(self.of("accuracy_rate", Some(&thread.uuid)));
        matching.retain(|c| validation_type_ok(&c.params, &thread.validation_type));
        if matching.is_empty() {
            return Ok(());
        }
        let ids: Vec<String> = matching.iter().map(|t| t.id.clone()).collect();
        let now = now_ms();
        for mut log in self.open_logs(counter, &ids).await? {
            let Some(t) = matching.iter().find(|t| t.id == log.challenge_id) else {
                continue;
            };
            let mut r = self.upsert_rank(counter, t.thread_uuid.as_deref()).await?;
            let size = param_f(&t.params, "windowSize").unwrap_or(20.0) as usize;
            let min = param_f(&t.params, "minAccuracyPercent").unwrap_or(100.0);
            log.accuracy_window = Some(window_append(log.accuracy_window.as_deref(), valid, size));
            log.updated_at = now;
            let valid_n = window_valid(log.accuracy_window.as_deref(), size);
            if valid_n.is_some_and(|n| n >= accuracy_needed(size, min)) {
                self.complete(t, &mut log, &mut r, now, col).await?;
            } else {
                self.save_log(&log).await?;
                col.add_log(&log);
            }
        }
        Ok(())
    }

    async fn time_counting(&self, counter: &str, thread: &str, ts_ms: f64, col: &mut Collector) -> anyhow::Result<()> {
        let second = (ts_ms / 1000.0).floor();
        if !second.is_finite() {
            return Ok(());
        }
        let second = second as i64;
        let mut due: Vec<(Option<String>, i64)> = vec![];
        {
            let mut tally = TIME_TALLY.lock();
            for scope in [Some(thread.to_owned()), None] {
                let key = format!("{counter}|{}", scope.as_deref().unwrap_or("sitewide"));
                let e = tally.entry(key).or_insert((0, 0));
                if e.1 == second {
                    continue;
                }
                e.1 = second;
                e.0 += 1;
                if e.0 >= 15 {
                    due.push((scope, e.0));
                    e.0 = 0;
                }
            }
            if !due.is_empty() && tally.len() >= 5000 {
                let cutoff = second - 3600;
                tally.retain(|_, t| t.1 >= cutoff);
            }
        }
        if due.is_empty() {
            return Ok(());
        }
        for (scope, amount) in due {
            let ts = self.of("time_counting", scope.as_deref());
            if ts.is_empty() {
                continue;
            }
            let mut r = self.upsert_rank(counter, scope.as_deref()).await?;
            self.chain(&ts, counter, &mut r, Some((second, amount)), col).await?;
        }
        Ok(())
    }

    async fn bars_speed(&self, counter: &str, thread: &str, ms: f64, col: &mut Collector) -> anyhow::Result<()> {
        let ts: Vec<Template> = self
            .of("bars_within_ms", Some(thread))
            .into_iter()
            .filter(|c| under_max_ms(ms, &c.params))
            .collect();
        if ts.is_empty() {
            return Ok(());
        }
        let mut r = self.upsert_rank(counter, Some(thread)).await?;
        self.chain(&ts, counter, &mut r, None, col).await
    }
}

#[derive(Clone, Copy)]
enum Scope<'a> {
    Thread(Option<&'a str>),
}

async fn engine(app: &App) -> Option<Engine<'_>> {
    let season = active_season(&app.pool).await?;
    let all = templates(&app.pool, season).await;
    Some(Engine {
        pool: &app.pool,
        season,
        all,
    })
}

pub async fn on_count_attempt(app: &App, post: &Post, counter: &Counter, thread: &ThreadMeta, rolled: bool) {
    if !post.is_count {
        return;
    }
    let roll_thread = matches!(thread.validation_type.as_str(), "1inx" | "incremental_odds");
    let r = with_lock(app, &counter.uuid, async {
        let e = engine(app).await?;
        let mut col = Collector::default();
        let res: anyhow::Result<()> = async {
            if post.is_valid_count {
                let bar_ms = bar_time(app, post, thread).await;
                e.thread_counts(&counter.uuid, &thread.uuid, &mut col).await?;
                e.sitewide_counts(&counter.uuid, thread, &mut col).await?;
                e.accuracy(&counter.uuid, thread, true, &mut col).await?;
                e.counts_in_day(&counter.uuid, &thread.uuid, &mut col).await?;
                e.count_attempts(&counter.uuid, &thread.uuid, &mut col).await?;
                e.time_counting(
                    &counter.uuid,
                    &thread.uuid,
                    counting::js::parse_float(&post.timestamp),
                    &mut col,
                )
                .await?;
                if let Some(ms) = bar_ms {
                    e.bars_speed(&counter.uuid, &thread.uuid, ms, &mut col).await?;
                }
            } else {
                e.accuracy(&counter.uuid, thread, false, &mut col).await?;
                if rolled || !roll_thread {
                    e.count_attempts(&counter.uuid, &thread.uuid, &mut col).await?;
                }
                e.time_counting(
                    &counter.uuid,
                    &thread.uuid,
                    counting::js::parse_float(&post.timestamp),
                    &mut col,
                )
                .await?;
            }
            Ok(())
        }
        .await;
        if let Err(err) = res {
            tracing::error!("[rank-eval] count attempt: {err:#}");
        }
        if col.is_empty() {
            return None;
        }
        Some(build_delta(&app.pool, e.season, &col).await)
    })
    .await;
    let Some(delta) = r else { return };
    emit_delta(app, &counter.uuid, Some(&thread.uuid), &delta).await;
    for (key, room) in [
        ("threadRank", format!("thread_{}", thread.uuid)),
        ("sitewideRank", "thread_all".to_string()),
    ] {
        if let Some(rr) = delta.get(key).filter(|v| !v.is_null()) {
            let entry = leaderboard_entry(rr, counter);
            app.to(room, "rank_leaderboard_updated", &entry).await;
        }
    }
}

fn leaderboard_entry(rr: &Value, c: &Counter) -> Value {
    let f = |k: &str| c.fields.get(k).cloned().filter(|v| !v.is_null()).unwrap_or(json!(""));
    json!({
        "counterUuid": c.uuid, "username": c.username, "name": f("name"), "avatar": f("avatar"),
        "discordId": f("discordId"), "color": f("color"), "rank": rr["rank"], "division": rr["division"],
        "gg": rr["gg"], "ggTotal": rr["ggTotal"], "threadUuid": rr["threadUuid"],
    })
}

async fn emit_delta(app: &App, counter: &str, thread: Option<&str>, delta: &Value) {
    let mut m = Map::new();
    m.insert("threadUuid".into(), json!(thread));
    let meta = thread.and_then(|t| app.thread_meta(t));
    m.insert("threadName".into(), json!(meta.as_ref().map(|t| t.name.clone())));
    m.insert(
        "threadTitle".into(),
        json!(meta.as_ref().map(|t| {
            t.row
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or(&t.name)
                .to_owned()
        })),
    );
    if let Some(o) = delta.as_object() {
        for (k, v) in o {
            m.insert(k.clone(), v.clone());
        }
    }
    app.to(format!("rank_{counter}"), "rank_updated", &Value::Object(m))
        .await;
    sync_rank_role(app, counter, delta).await;
}

async fn sync_rank_role(app: &App, counter: &str, delta: &Value) {
    let Some(rank) = delta
        .get("sitewideRank")
        .and_then(|r| r.get("rank"))
        .and_then(|r| r.as_str())
    else {
        return;
    };
    if app.rank_role_sent.get(counter).is_some_and(|r| *r == rank) {
        return;
    }
    let Some(c) = app.counter(counter).await else { return };
    let Some(d) = c
        .fields
        .get("discordId")
        .and_then(|v| v.as_str())
        .filter(|d| !d.is_empty())
    else {
        return;
    };
    app.bot.emit("syncRankRoles", json!({ "discordId": d, "rank": rank }));
    app.rank_role_sent.insert(counter.to_owned(), rank.to_owned());
}

async fn bar_time(app: &App, post: &Post, thread: &ThreadMeta) -> Option<f64> {
    let n = post.valid_count_number.filter(|n| *n > 1)?;
    let prev: Option<String> = sqlx::query_scalar(
        "SELECT timestamp FROM post FORCE INDEX (n_thread_validcountnum) WHERE threadId = ? AND validCountNumber = ? LIMIT 1",
    )
    .bind(thread.id)
    .bind(n - 1)
    .fetch_optional(&app.pool)
    .await
    .ok()
    .flatten();
    let prev = counting::js::parse_float(&prev?);
    let cur = counting::js::parse_float(&post.timestamp);
    ((cur / 3_600_000.0).floor() > (prev / 3_600_000.0).floor()).then(|| (cur % 3_600_000.0).round())
}

pub async fn on_roll(app: &App, counter: &Counter, roll: f64, thread: &ThreadMeta) {
    let r = with_lock(app, &counter.uuid, async {
        let e = engine(app).await?;
        let eligible: Vec<Template> = e
            .of("roll", Some(&thread.uuid))
            .into_iter()
            .filter(|c| roll_qualifies(roll, &c.params))
            .collect();
        if eligible.is_empty() {
            return None;
        }
        let mut col = Collector::default();
        let res: anyhow::Result<()> = async {
            let mut r = e.upsert_rank(&counter.uuid, Some(&thread.uuid)).await?;
            e.chain(&eligible, &counter.uuid, &mut r, None, &mut col).await
        }
        .await;
        if let Err(err) = res {
            tracing::error!("[rank-eval] roll: {err:#}");
        }
        if col.is_empty() {
            None
        } else {
            Some(build_delta(&app.pool, e.season, &col).await)
        }
    })
    .await;
    if let Some(d) = r {
        emit_delta(app, &counter.uuid, Some(&thread.uuid), &d).await;
    }
}

async fn minigame(app: &App, counter: &Counter, kind: &str, pseudo: &str, qualifies: impl Fn(&Template) -> bool) {
    let r = with_lock(app, &counter.uuid, async {
        let e = engine(app).await?;
        let eligible: Vec<Template> = e
            .all
            .iter()
            .filter(|t| t.kind == kind)
            .filter(|t| qualifies(t))
            .cloned()
            .collect();
        if eligible.is_empty() {
            return None;
        }
        let mut col = Collector::default();
        let res: anyhow::Result<()> = async {
            let mut r = e.upsert_rank(&counter.uuid, Some(pseudo)).await?;
            e.chain(&eligible, &counter.uuid, &mut r, None, &mut col).await
        }
        .await;
        if let Err(err) = res {
            tracing::error!("[rank-eval] {kind}: {err:#}");
        }
        if col.is_empty() {
            None
        } else {
            Some(build_delta(&app.pool, e.season, &col).await)
        }
    })
    .await;
    if let Some(d) = r {
        emit_delta(app, &counter.uuid, None, &d).await;
    }
}

pub async fn on_lrwoed(app: &App, counter: &Counter, score: f64) {
    minigame(app, counter, "lrwoed_score", LRWOED_PSEUDO_THREAD, |t| {
        score <= param_f(&t.params, "maxScore").unwrap_or(f64::INFINITY)
    })
    .await;
    if score > 0.0 {
        daily(app, counter, "lrwoed_days", |t| {
            score <= param_f(&t.params, "maxScore").unwrap_or(f64::INFINITY)
        })
        .await;
    }
}

pub async fn on_number_shuffle_win(app: &App, counter: &Counter, size: i64, preseeded: bool, daily_win: bool) {
    minigame(app, counter, "number_shuffle_win", SHUFFLE_PSEUDO_THREAD, |t| {
        !preseeded && size as f64 >= param_f(&t.params, "minSize").unwrap_or(0.0)
    })
    .await;
    if daily_win {
        daily(app, counter, "daily_shuffle_wins", |_| true).await;
    }
}

async fn daily(app: &App, counter: &Counter, kind: &str, qualifies: impl Fn(&Template) -> bool) {
    let r = with_lock(app, &counter.uuid, async {
        let e = engine(app).await?;
        let eligible: Vec<Template> = e.of(kind, None).into_iter().filter(|t| qualifies(t)).collect();
        if eligible.is_empty() {
            return None;
        }
        let mut col = Collector::default();
        let res: anyhow::Result<()> = async {
            let mut r = e.upsert_rank(&counter.uuid, None).await?;
            e.chain(
                &eligible,
                &counter.uuid,
                &mut r,
                Some((day_number(now_ms()), 1)),
                &mut col,
            )
            .await
        }
        .await;
        if let Err(err) = res {
            tracing::error!("[rank-eval] {kind}: {err:#}");
        }
        if col.is_empty() {
            None
        } else {
            Some(build_delta(&app.pool, e.season, &col).await)
        }
    })
    .await;
    if let Some(d) = r {
        emit_delta(app, &counter.uuid, None, &d).await;
    }
}

pub async fn on_split(app: &App, thread: &ThreadMeta, split: &Value) {
    let Some(e) = engine(app).await else { return };
    let number = split["number"].as_str().unwrap_or("");
    let is_get = number.ends_with(" (Get)") || number.ends_with(" (K)");
    let lookup = number.trim_end_matches(" (Get)").trim_end_matches(" (K)");
    let ms = split["split"].as_f64().unwrap_or(f64::NAN);
    let vcn = match split["validCountNumber"].as_i64() {
        Some(n) => Some(n),
        None => sqlx::query_scalar::<_, Option<i32>>(
            "SELECT /*+ MAX_EXECUTION_TIME(10000) */ validCountNumber FROM post WHERE threadId = ? AND rawCount = ? \
             AND isValidCount = 1 ORDER BY id DESC LIMIT 1",
        )
        .bind(thread.id)
        .bind(lookup)
        .fetch_optional(&app.pool)
        .await
        .ok()
        .flatten()
        .flatten()
        .map(i64::from),
    };
    let Some(vcn) = vcn else { return };
    if !crate::persisted::await_count(&app.pool, thread.id, vcn, Duration::from_secs(2)).await {
        tracing::warn!(thread = %thread.name, vcn, "[rank split] closing post not in MySQL after 2s; scoring what is there");
    }
    let cps = thread.row.get("countsPerSplit").and_then(Value::as_i64).unwrap_or(0);
    let spg = thread.row.get("splitsPerGet").and_then(Value::as_i64).unwrap_or(0);
    crate::feats::on_run(app, thread, vcn, ms, if is_get { cps * spg } else { cps }, is_get).await;
    if is_get {
        sitewide_get(app, &e, thread, vcn, ms, cps * spg).await;
    }
    let (kind, interval) = if is_get {
        if cps * spg <= 0 {
            return;
        }
        ("get_under_ms", cps * spg)
    } else {
        ("split_under_ms", cps)
    };
    let eligible: Vec<Template> = e
        .of(kind, Some(&thread.uuid))
        .into_iter()
        .filter(|c| validation_type_ok(&c.params, &thread.validation_type) && under_max_ms(ms, &c.params))
        .collect();
    if eligible.is_empty() || interval <= 0 {
        return;
    }
    let authors: Vec<String> = sqlx::query_scalar(
        "SELECT authorUUID FROM post FORCE INDEX (n_thread_validcountnum) WHERE threadId = ? AND isValidCount = 1 \
         AND validCountNumber BETWEEN ? AND ?",
    )
    .bind(thread.id)
    .bind(vcn - interval + 1)
    .bind(vcn)
    .fetch_all(&app.pool)
    .await
    .unwrap_or_default();
    if authors.is_empty() {
        return;
    }
    let credited = credited_authors(thread, authors);
    if credited.is_empty() {
        return;
    }
    for uuid in credited {
        let r = with_lock(app, &uuid, async {
            let mut col = Collector::default();
            let res: anyhow::Result<()> = async {
                let mut r = e.upsert_rank(&uuid, Some(&thread.uuid)).await?;
                e.chain(&eligible, &uuid, &mut r, None, &mut col).await
            }
            .await;
            if let Err(err) = res {
                tracing::error!("[rank-eval] split: {err:#}");
            }
            if col.is_empty() {
                None
            } else {
                Some(build_delta(&app.pool, e.season, &col).await)
            }
        })
        .await;
        if let Some(d) = r {
            emit_delta(app, &uuid, Some(&thread.uuid), &d).await;
        }
    }
}

fn credited_authors(thread: &ThreadMeta, authors: Vec<String>) -> Vec<String> {
    if thread.allow_double_posts {
        let mut counts: HashMap<&str, i64> = HashMap::new();
        for a in &authors {
            *counts.entry(a.as_str()).or_default() += 1;
        }
        let top = *counts.values().max().unwrap_or(&0) as f64;
        let q: Vec<String> = counts
            .iter()
            .filter(|(_, c)| **c as f64 >= top * 0.25)
            .map(|(a, _)| a.to_string())
            .collect();
        if q.len() != 1 { vec![] } else { q }
    } else {
        let mut seen = std::collections::HashSet::new();
        authors.into_iter().filter(|a| seen.insert(a.clone())).collect()
    }
}

pub(crate) fn qualified_counters<'s>(authors: impl Iterator<Item = &'s str>) -> Vec<String> {
    let mut counts: HashMap<&str, i64> = HashMap::new();
    for a in authors {
        *counts.entry(a).or_default() += 1;
    }
    let top = counts.values().copied().max().unwrap_or(0);
    if top <= 0 {
        return vec![];
    }
    counts
        .into_iter()
        .filter(|(_, c)| *c * 4 >= top)
        .map(|(a, _)| a.to_owned())
        .collect()
}

async fn sitewide_get(app: &App, e: &Engine<'_>, thread: &ThreadMeta, vcn: i64, ms: f64, interval: i64) {
    let has = |k: &str| !e.of(k, None).is_empty();
    let assists = has("assists_landed") && !thread.allow_double_posts;
    let (gets, fast) = (has("gets_landed"), has("fast_gets") && interval > 0 && ms > 0.0);
    if !gets && !assists && !fast {
        return;
    }
    let from = if fast { vcn - interval + 1 } else { vcn - 1 };
    let rows: Vec<(i32, String)> = sqlx::query_as(
        "SELECT validCountNumber, authorUUID FROM post FORCE INDEX (n_thread_validcountnum) WHERE threadId = ? AND isValidCount = 1 \
         AND validCountNumber BETWEEN ? AND ?",
    )
    .bind(thread.id)
    .bind(from.max(1))
    .bind(vcn)
    .fetch_all(&app.pool)
    .await
    .unwrap_or_default();
    let at = |n: i64| rows.iter().find(|(v, _)| i64::from(*v) == n).map(|(_, a)| a.clone());
    let mut due: Vec<(String, Vec<&str>)> = vec![];
    let mut add = |who: String, kind: &'static str| match due.iter_mut().find(|(w, _)| *w == who) {
        Some((_, ks)) => ks.push(kind),
        None => due.push((who, vec![kind])),
    };
    if gets && let Some(a) = at(vcn) {
        add(a, "gets_landed");
    }
    if assists && let Some(a) = at(vcn - 1) {
        add(a, "assists_landed");
    }
    let cps = (fast && rows.len() as i64 >= interval).then(|| interval as f64 / (ms / 1000.0));
    if cps.is_some() {
        for a in qualified_counters(rows.iter().map(|(_, a)| a.as_str())) {
            add(a, "fast_gets");
        }
    }
    for (who, kinds) in due {
        let r = with_lock(app, &who, async {
            let mut col = Collector::default();
            if let Err(err) = e.sitewide_get(&who, &thread.uuid, &kinds, cps, &mut col).await {
                tracing::error!("[rank-eval] sitewide get: {err:#}");
            }
            if col.is_empty() {
                None
            } else {
                Some(build_delta(&app.pool, e.season, &col).await)
            }
        })
        .await;
        if let Some(d) = r {
            emit_delta(app, &who, None, &d).await;
        }
    }
}

async fn counter_cards(app: &App, uuids: &[String]) -> HashMap<String, Arc<Counter>> {
    app.counters_of(uuids)
        .await
        .into_iter()
        .map(|c| (c.uuid.clone(), c))
        .collect()
}

pub async fn emit_sitewide_initial(app: &App, s: &SocketRef) {
    let Some(season) = active_season(&app.pool).await else {
        return;
    };
    let rows = sqlx::query(
        "SELECT id, counterUuid, threadUuid, seasonId, `rank`, division, gg, ggTotal FROM threadrank \
         WHERE seasonId = ? AND threadUuid IS NULL AND ggTotal > 0 ORDER BY ggTotal DESC, id ASC",
    )
    .bind(season)
    .fetch_all(&app.pool)
    .await
    .unwrap_or_default();
    let ranks: Vec<RankRow> = rows.iter().map(rank_from).collect();
    let ids: Vec<String> = ranks.iter().map(|r| r.counter_uuid.clone()).collect();
    let cards = counter_cards(app, &ids).await;
    let entries: Vec<Value> = ranks
        .iter()
        .map(|r| {
            let c = cards.get(&r.counter_uuid);
            let f = |k: &str| {
                c.and_then(|c| c.fields.get(k).cloned())
                    .filter(|v| !v.is_null())
                    .unwrap_or(json!(""))
            };
            json!({
                "counterUuid": r.counter_uuid, "username": c.map_or(String::new(), |c| c.username.clone()),
                "name": f("name"), "avatar": f("avatar"), "discordId": f("discordId"), "color": f("color"),
                "rank": r.rank, "division": r.division, "gg": r.gg_total, "ggTotal": r.gg_total, "threadUuid": null,
            })
        })
        .collect();
    s.emit(
        "rank_leaderboard_initial",
        &json!({ "threadUuid": null, "entries": entries }),
    )
    .ok();
}

pub async fn run(
    app: Arc<App>,
    committed: Arc<crate::wal::Committed>,
    start: crate::cursor::Position,
    cp: Arc<std::sync::atomic::AtomicU64>,
) {
    let dir = app.cfg.wal_dir.clone();
    let a2 = app.clone();
    let r = crate::cursor::follow(dir, "rank", committed, start, cp, move |_seq, rec| {
        let app = a2.clone();
        async move {
            let crate::wal::WalRecord::Post { post, roll, .. } = rec else {
                return;
            };
            let Some(counter) = app.counter(&post.author_uuid).await else {
                return;
            };
            let Some(thread) = app.thread_meta(&post.thread) else {
                return;
            };
            let roll = roll.filter(|r| r.is_finite());
            if let Some(roll) = roll {
                on_roll(&app, &counter, roll, &thread).await;
            }
            on_count_attempt(&app, &post, &counter, &thread, roll.is_some()).await;
        }
    })
    .await;
    if let Err(e) = r {
        tracing::error!("rank consumer stopped: {e:#}");
        std::process::exit(73);
    }
}

pub async fn assign_for_bingo_game(app: &App, game_id: i64, participants: &[String]) {
    let Some(season) = active_season(&app.pool).await else {
        return;
    };
    let ts: Vec<Template> = templates(&app.pool, season)
        .await
        .into_iter()
        .filter(|t| t.rank == "bronze" && (t.kind == "bingo_win" || t.kind == "bingo_complete_squares"))
        .collect();
    for counter in participants {
        for t in &ts {
            let open: Option<i64> = sqlx::query_scalar(
                "SELECT CAST(id AS SIGNED) FROM challengelog WHERE counterUuid = ? AND challengeId = ? AND seasonId = ? \
                 AND completedAt IS NULL AND cancelledAt IS NULL LIMIT 1",
            )
            .bind(counter)
            .bind(&t.id)
            .bind(season)
            .fetch_optional(&app.pool)
            .await
            .ok()
            .flatten();
            if open.is_some() {
                continue;
            }
            let now = now_ms();
            let r = sqlx::query(
                "INSERT INTO challengelog (counterUuid, challengeId, seasonId, context, bingoGameId, progress, target, \
                 accuracyWindow, completedAt, cancelledAt, ggReward, ggAwarded, assignmentOrdinal, availableAt, createdAt, updatedAt) \
                 VALUES (?, ?, ?, 'bingo', ?, 0, ?, NULL, NULL, NULL, ?, 0, 0, NULL, ?, ?)",
            )
            .bind(counter)
            .bind(&t.id)
            .bind(season)
            .bind(game_id)
            .bind(t.target)
            .bind(t.gg_reward)
            .bind(now)
            .bind(now)
            .execute(&app.pool)
            .await;
            if let Err(e) = r {
                tracing::warn!("bingo challenge assignment failed: {e}");
            }
        }
    }
}

pub async fn cancel_bingo_game_instances(app: &App, game_id: i64) {
    let now = now_ms();
    let _ = sqlx::query(
        "UPDATE challengelog SET cancelledAt = ?, updatedAt = ? WHERE bingoGameId = ? AND completedAt IS NULL AND cancelledAt IS NULL",
    )
    .bind(now)
    .bind(now)
    .bind(game_id)
    .execute(&app.pool)
    .await;
}

pub async fn season(app: &App) -> Option<i64> {
    active_season(&app.pool).await
}

pub async fn sequence_zero(app: &App, season: i64, kind: &str, rank: &str) -> Vec<Template> {
    templates(&app.pool, season)
        .await
        .into_iter()
        .filter(|t| t.kind == kind && t.rank == rank && t.sequence == 0)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn divisions_and_penalty() {
        assert_eq!(division_from_gg("bronze", 99), 1);
        assert_eq!(division_from_gg("bronze", 100), 2);
        assert_eq!(division_from_gg("bronze", 200), 3);
        assert_eq!(division_from_gg("peak", 0), 3);
        assert_eq!(repeat_penalty(100, 0), 100);
        assert_eq!(repeat_penalty(100, 1), 80);
        assert_eq!(repeat_penalty(100, 100), 25);
        assert_eq!(next_rank("grandcounter"), Some("peak"));
        assert_eq!(next_rank("peak"), None);
    }

    #[test]
    fn accuracy_window_matches_node() {
        let w = window_append(None, true, 5);
        assert_eq!(w, "1:AQ==");
        assert_eq!(window_append(Some(&w), false, 5), "2:AQ==");
        let mut x: Option<String> = None;
        for i in 0..7 {
            x = Some(window_append(x.as_deref(), i % 3 != 0, 5));
        }
        assert_eq!(x.as_deref(), Some("5:DQ=="));
        assert_eq!(window_pct(x.as_deref(), 5), Some(60.0));
        let mut y: Option<String> = None;
        for i in 0..45 {
            y = Some(window_append(y.as_deref(), (i * 7) % 5 != 0, 20));
        }
        assert_eq!(y.as_deref(), Some("20:D3ve"));
        assert_eq!(window_pct(y.as_deref(), 20), Some(80.0));
        assert_eq!(window_valid(y.as_deref(), 20), Some(16));
        assert_eq!(window_valid(x.as_deref(), 20), None, "not full yet");
    }

    #[test]
    fn accuracy_threshold_is_a_count() {
        assert_eq!(accuracy_needed(2500, 98.2), 2455);
        assert!(2458 >= accuracy_needed(2500, 98.2));
        assert!(2454 < accuracy_needed(2500, 98.2));
        assert_eq!(accuracy_needed(20, 95.0), 19);
        assert_eq!(accuracy_needed(350, 96.5), 338);
        assert_eq!(accuracy_needed(20, 100.0), 20);
        let mut w: Option<String> = None;
        for i in 0..2600 {
            w = Some(window_append(
                w.as_deref(),
                !(i >= 100 && (i - 100) % 59 == 0 && i < 100 + 59 * 42),
                2500,
            ));
        }
        assert_eq!(window_valid(w.as_deref(), 2500), Some(2458));
    }
}
