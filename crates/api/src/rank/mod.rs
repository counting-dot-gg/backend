use crate::db;
use crate::entity;
use crate::error::ApiResult;
use crate::http::js_number;
use crate::state::AppState;
use cgg_web::now_ms;
use serde_json::{Value, json};

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

pub const CHALLENGE_TYPES: [&str; 21] = [
    "thread_counts",
    "split_under_ms",
    "get_under_ms",
    "bars_within_ms",
    "roll",
    "lrwoed_score",
    "number_shuffle_win",
    "bingo_win",
    "bingo_complete_squares",
    "accuracy_rate",
    "counts_in_day",
    "count_attempts",
    "time_counting",
    "thread_breadth",
    "strict_counts",
    "gets_landed",
    "assists_landed",
    "thread_spread",
    "fast_gets",
    "lrwoed_days",
    "daily_shuffle_wins",
];

pub const SITEWIDE_ONLY_TYPES: [&str; 8] = [
    "thread_breadth",
    "strict_counts",
    "gets_landed",
    "assists_landed",
    "thread_spread",
    "fast_gets",
    "lrwoed_days",
    "daily_shuffle_wins",
];

pub const BINGO_ONLY_TYPES: [&str; 1] = ["thread_breadth"];
pub const BINGO_CATEGORIES: [&str; 7] = [
    "volume",
    "speed_split",
    "speed_get",
    "rng",
    "gimmick",
    "minigame",
    "meta",
];

pub fn gg_per_rank(rank: &str) -> f64 {
    match rank {
        "bronze" => 300.0,
        "silver" => 700.0,
        "gold" => 1400.0,
        "platinum" => 2500.0,
        "emerald" => 4000.0,
        "diamond" => 6500.0,
        "countmeister" => 10000.0,
        "grandcounter" => 16000.0,
        _ => f64::INFINITY,
    }
}

pub fn default_wait_ms(kind: &str) -> Option<i64> {
    matches!(kind, "split_under_ms" | "get_under_ms").then_some(86_400_000)
}

pub fn rank_index(rank: &str) -> i64 {
    RANK_ORDER.iter().position(|r| *r == rank).map_or(-1, |i| i as i64)
}

fn s<'a>(c: &'a Value, k: &str) -> &'a str {
    c[k].as_str().unwrap_or("")
}

fn param(c: &Value, k: &str) -> String {
    match &c["params"][k] {
        Value::Null => String::new(),
        v => crate::http::js_string(v),
    }
}

fn scope(c: &Value) -> String {
    c["threadUuid"].as_str().unwrap_or("sitewide").to_owned()
}

pub fn chain_key(c: &Value) -> String {
    let t = s(c, "type");
    match t {
        "thread_counts" | "counts_in_day" | "time_counting" | "count_attempts" => format!("{t}:{}", scope(c)),
        "accuracy_rate" | "split_under_ms" | "get_under_ms" | "bars_within_ms" => {
            format!("{t}:{}:{}", scope(c), param(c, "validationType"))
        }
        "roll" => format!("{t}:{}:{}", scope(c), param(c, "direction")),
        "lrwoed_score" | "number_shuffle_win" => format!("{t}:{}", param(c, "validationType")),
        "bingo_win" | "bingo_complete_squares" => format!("{t}:{}", c["bingoCategory"].as_str().unwrap_or("")),
        _ => t.to_owned(),
    }
}

pub fn uuid_v4() -> String {
    let mut b: [u8; 16] = std::array::from_fn(|_| fastrand::u8(..));
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

pub async fn active_season(state: &AppState) -> ApiResult<Option<Value>> {
    let r = sqlx::query("SELECT * FROM rankseason WHERE endedAt IS NULL LIMIT 1")
        .fetch_optional(&state.pool)
        .await?;
    Ok(r.map(|r| entity::RANK_SEASON.json(&r)))
}

pub async fn templates(state: &AppState, where_sql: &str, args: &[Value]) -> ApiResult<Vec<Value>> {
    let sql = if where_sql.is_empty() {
        "SELECT * FROM rankchallenge".to_owned()
    } else {
        format!("SELECT * FROM rankchallenge WHERE {where_sql}")
    };
    Ok(db::rows(state, &sql, args)
        .await?
        .iter()
        .map(|r| entity::RANK_CHALLENGE.json(r))
        .collect())
}

fn thread_clause(thread: &Value) -> (&'static str, Vec<Value>) {
    if thread.is_null() {
        ("threadUuid IS NULL", vec![])
    } else {
        ("threadUuid = ?", vec![thread.clone()])
    }
}

async fn ensure_thread_rank(state: &AppState, counter: &str, thread: &Value, season: i64) -> ApiResult<()> {
    let (tc, ta) = thread_clause(thread);
    let mut args = vec![json!(counter), json!(season)];
    args.extend(ta);
    let existing = db::rows(
        state,
        &format!("SELECT id FROM threadrank WHERE counterUuid = ? AND seasonId = ? AND {tc} LIMIT 1"),
        &args,
    )
    .await?;
    if existing.is_empty() {
        db::exec(
            state,
            "INSERT INTO threadrank (counterUuid, threadUuid, seasonId, `rank`, division, gg, ggTotal) VALUES (?, ?, ?, 'bronze', 1, 0, 0)",
            &[json!(counter), thread.clone(), json!(season)],
        )
        .await?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn create_fresh(
    state: &AppState,
    template: &Value,
    counter: &str,
    season: i64,
    context: &str,
    bingo_game: Option<i64>,
    gg_reward: f64,
    available_at: Option<f64>,
    ordinal: i64,
) -> ApiResult<()> {
    let now = now_ms();
    db::exec(
        state,
        "INSERT INTO challengelog (counterUuid, challengeId, seasonId, context, bingoGameId, progress, target, accuracyWindow, completedAt, \
         cancelledAt, ggReward, ggAwarded, assignmentOrdinal, availableAt, createdAt, updatedAt) \
         VALUES (?, ?, ?, ?, ?, 0, ?, NULL, NULL, NULL, ?, 0, ?, ?, ?, ?)",
        &[
            json!(counter),
            template["id"].clone(),
            json!(season),
            json!(context),
            bingo_game.map_or(Value::Null, Value::from),
            json!(js_number(&template["target"]) as i64),
            json!(gg_reward as i64),
            json!(ordinal),
            available_at.map_or(Value::Null, |a| json!(a as i64)),
            json!(now),
            json!(now),
        ],
    )
    .await?;
    if context == "rank" {
        ensure_thread_rank(state, counter, &template["threadUuid"], season).await?;
    }
    Ok(())
}

pub async fn create_if_missing(
    state: &AppState,
    template: &Value,
    counter: &str,
    season: i64,
    context: &str,
    bingo_game: Option<i64>,
) -> ApiResult<()> {
    let open = db::rows(
        state,
        "SELECT id FROM challengelog WHERE counterUuid = ? AND challengeId = ? AND seasonId = ? AND completedAt IS NULL AND cancelledAt IS NULL LIMIT 1",
        &[json!(counter), template["id"].clone(), json!(season)],
    )
    .await?;
    if !open.is_empty() {
        return Ok(());
    }
    create_fresh(
        state,
        template,
        counter,
        season,
        context,
        bingo_game,
        js_number(&template["ggReward"]),
        None,
        0,
    )
    .await
}

fn higher_ranks(below: &str) -> Vec<Value> {
    let idx = rank_index(below);
    RANK_ORDER
        .iter()
        .enumerate()
        .filter(|(i, _)| *i as i64 > idx)
        .map(|(_, r)| json!(r))
        .collect()
}

enum ThreadFilter<'a> {
    Any,
    Is(&'a Value),
}

async fn assign_auto_jump(
    state: &AppState,
    counter: &str,
    season: i64,
    below: &str,
    filter: ThreadFilter<'_>,
) -> ApiResult<()> {
    let higher = higher_ranks(below);
    if higher.is_empty() {
        return Ok(());
    }
    let mut where_sql = format!(
        "`rank` IN ({}) AND seasonId = ? AND sequence = 0 AND rewardType = 'auto_jump'",
        db::placeholders(higher.len())
    );
    let mut args = higher;
    args.push(json!(season));
    if let ThreadFilter::Is(t) = filter {
        let (tc, ta) = thread_clause(t);
        where_sql.push_str(&format!(" AND {tc}"));
        args.extend(ta);
    }
    for t in templates(state, &where_sql, &args).await? {
        create_if_missing(state, &t, counter, season, "rank", None).await?;
    }
    Ok(())
}

fn not_bingo_only() -> String {
    format!(
        "type NOT IN ({})",
        BINGO_ONLY_TYPES
            .iter()
            .map(|t| format!("'{t}'"))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

pub async fn assign_eligible_for_counter(state: &AppState, counter: &str, season: i64) -> ApiResult<()> {
    let ts = templates(
        state,
        &format!(
            "`rank` = 'bronze' AND seasonId = ? AND sequence = 0 AND {}",
            not_bingo_only()
        ),
        &[json!(season)],
    )
    .await?;
    for t in ts {
        create_if_missing(state, &t, counter, season, "rank", None).await?;
    }
    assign_auto_jump(state, counter, season, "bronze", ThreadFilter::Any).await
}

pub async fn assign_new_counter(state: &AppState, uuid: &str) -> ApiResult<()> {
    if let Some(season) = active_season(state).await?
        && let Some(id) = season["id"].as_i64()
    {
        assign_eligible_for_counter(state, uuid, id).await?;
    }
    Ok(())
}

async fn all_counter_uuids(state: &AppState) -> ApiResult<Vec<String>> {
    Ok(sqlx::query_scalar("SELECT uuid FROM counters")
        .fetch_all(&state.pool)
        .await?)
}

pub async fn assign_eligible_for_template(state: &AppState, template: &Value, season: i64) -> ApiResult<()> {
    if template["sequence"].as_i64() != Some(0) || BINGO_ONLY_TYPES.contains(&s(template, "type")) {
        return Ok(());
    }
    let thread = &template["threadUuid"];
    let (tc, ta) = thread_clause(thread);
    let scoped_args: Vec<Value> = std::iter::once(json!(season)).chain(ta.clone()).collect();
    let scoped = db::rows(
        state,
        &format!("SELECT counterUuid, `rank` FROM threadrank WHERE seasonId = ? AND {tc}"),
        &scoped_args,
    )
    .await?;
    let ranked: Vec<(String, String)> = scoped
        .iter()
        .map(|r| {
            let j = entity::row_json(r);
            (s(&j, "counterUuid").to_owned(), s(&j, "rank").to_owned())
        })
        .collect();
    if s(template, "rewardType") == "auto_jump" {
        for c in all_counter_uuids(state).await? {
            let current = ranked.iter().find(|r| r.0 == c).map_or("bronze", |r| r.1.as_str());
            assign_auto_jump(state, &c, season, current, ThreadFilter::Is(thread)).await?;
        }
        return Ok(());
    }
    let target_rank = s(template, "rank");
    let mut uuids: Vec<String> = ranked
        .iter()
        .filter(|r| r.1 == target_rank)
        .map(|r| r.0.clone())
        .collect();
    if target_rank == "bronze" {
        for c in all_counter_uuids(state).await? {
            if !ranked.iter().any(|r| r.0 == c) && !uuids.contains(&c) {
                uuids.push(c);
            }
        }
    }
    for c in uuids {
        create_if_missing(state, template, &c, season, "rank", None).await?;
    }
    Ok(())
}

pub async fn reconcile_on_rank_up(
    state: &AppState,
    counter: &str,
    old_rank: &str,
    thread: &Value,
    new_rank: &str,
    season: Option<i64>,
) -> ApiResult<()> {
    let _ = old_rank;
    let Some(season) = season else { return Ok(()) };
    assign_auto_jump(state, counter, season, new_rank, ThreadFilter::Is(thread)).await?;
    let new_idx = rank_index(new_rank);
    let stale: Vec<Value> = RANK_ORDER
        .iter()
        .enumerate()
        .filter(|(i, _)| (*i as i64) < new_idx)
        .map(|(_, r)| json!(r))
        .collect();
    if stale.is_empty() {
        return Ok(());
    }
    let (tc, ta) = thread_clause(thread);
    let mut args = stale.clone();
    args.push(json!(season));
    args.extend(ta.clone());
    let stale_templates = templates(
        state,
        &format!(
            "`rank` IN ({}) AND seasonId = ? AND {tc}",
            db::placeholders(stale.len())
        ),
        &args,
    )
    .await?;
    if stale_templates.is_empty() {
        return Ok(());
    }
    let ids: Vec<Value> = stale_templates.iter().map(|t| t["id"].clone()).collect();
    let mut largs = vec![json!(counter)];
    largs.extend(ids.clone());
    largs.push(json!(season));
    let open = db::rows(
        state,
        &format!(
            "SELECT id, challengeId FROM challengelog WHERE counterUuid = ? AND challengeId IN ({}) AND seasonId = ? AND completedAt IS NULL AND cancelledAt IS NULL",
            db::placeholders(ids.len())
        ),
        &largs,
    )
    .await?;
    if open.is_empty() {
        return Ok(());
    }
    let now = now_ms();
    let mut nargs = vec![json!(new_rank), json!(season)];
    nargs.extend(ta);
    let new_templates = templates(
        state,
        &format!(
            "`rank` = ? AND seasonId = ? AND {tc} AND sequence = 0 AND {}",
            not_bingo_only()
        ),
        &nargs,
    )
    .await?;
    let mut replaced: Vec<String> = Vec::new();
    for log in &open {
        let j = entity::row_json(log);
        let Some(t) = stale_templates.iter().find(|t| t["id"] == j["challengeId"]) else {
            continue;
        };
        db::exec(
            state,
            "UPDATE challengelog SET cancelledAt = ?, updatedAt = ? WHERE id = ?",
            &[json!(now), json!(now), j["id"].clone()],
        )
        .await?;
        let key = chain_key(t);
        if replaced.contains(&key) {
            continue;
        }
        replaced.push(key.clone());
        if let Some(start) = new_templates.iter().find(|c| chain_key(c) == key) {
            create_if_missing(state, start, counter, season, "rank", None).await?;
        }
    }
    Ok(())
}

pub async fn cleanup_bad_chains(state: &AppState, dry_run: bool) -> ApiResult<Value> {
    let all = templates(state, "", &[]).await?;
    let nonzero: Vec<Value> = all
        .iter()
        .filter(|t| t["sequence"].as_i64().unwrap_or(0) > 0)
        .map(|t| t["id"].clone())
        .collect();
    if nonzero.is_empty() {
        return Ok(json!({ "cancelled": 0, "details": [] }));
    }
    let open: Vec<Value> = db::rows(
        state,
        &format!(
            "SELECT * FROM challengelog WHERE challengeId IN ({}) AND completedAt IS NULL AND cancelledAt IS NULL",
            db::placeholders(nonzero.len())
        ),
        &nonzero,
    )
    .await?
    .iter()
    .map(|r| entity::CHALLENGE_LOG.json(r))
    .collect();
    let chain_of = |t: &Value| format!("{}|{}|{}", chain_key(t), s(t, "rank"), t["seasonId"]);
    let mut cancel: Vec<Value> = Vec::new();
    let mut details: Vec<Value> = Vec::new();
    for log in &open {
        let Some(t) = all.iter().find(|t| t["id"] == log["challengeId"]) else {
            continue;
        };
        let key = chain_of(t);
        let seq = t["sequence"].as_i64().unwrap_or(0);
        let Some(pred) = all
            .iter()
            .find(|c| chain_of(c) == key && c["sequence"].as_i64() == Some(seq - 1))
        else {
            continue;
        };
        let done = db::rows(
            state,
            "SELECT id FROM challengelog WHERE counterUuid = ? AND challengeId = ? AND seasonId <=> ? AND completedAt IS NOT NULL LIMIT 1",
            &[log["counterUuid"].clone(), pred["id"].clone(), log["seasonId"].clone()],
        )
        .await?;
        if !done.is_empty() {
            continue;
        }
        cancel.push(log["id"].clone());
        details.push(json!({
            "counterUuid": log["counterUuid"], "challengeId": log["challengeId"],
            "type": t["type"], "rank": t["rank"], "sequence": t["sequence"],
        }));
    }
    if !dry_run && !cancel.is_empty() {
        let now = now_ms();
        let mut args = vec![json!(now), json!(now)];
        args.extend(cancel.iter().cloned());
        db::exec(
            state,
            &format!(
                "UPDATE challengelog SET cancelledAt = ?, updatedAt = ? WHERE id IN ({})",
                db::placeholders(cancel.len())
            ),
            &args,
        )
        .await?;
    }
    Ok(json!({ "cancelled": cancel.len(), "details": details }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chain_keys_follow_the_ts_rules() {
        assert_eq!(
            chain_key(&json!({ "type": "thread_counts", "threadUuid": null })),
            "thread_counts:sitewide"
        );
        assert_eq!(
            chain_key(&json!({ "type": "roll", "threadUuid": "t", "params": { "direction": "under" } })),
            "roll:t:under"
        );
        assert_eq!(
            chain_key(&json!({ "type": "bingo_win", "bingoCategory": "rng" })),
            "bingo_win:rng"
        );
        assert_eq!(chain_key(&json!({ "type": "fast_gets" })), "fast_gets");
    }
}
