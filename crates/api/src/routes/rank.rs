use crate::db::{self, has_role};
use crate::entity;
use crate::error::{ApiError, ApiResult};
use crate::http::{JsonBody, Query, Reply, js_number, reply};
use crate::rank::*;
use crate::session::Authed;
use crate::state::AppState;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use cgg_web::now_ms;
use serde_json::{Map, Value, json};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/rank/season", get(season))
        .route("/rank/seasons", get(seasons))
        .route("/rank/challenges", get(challenges))
        .route("/rank/challenges/{id}", get(challenge_by_id))
        .route("/rank/thread/{thread_name}", get(thread_leaderboard))
        .route("/rank/counter/{username}", get(counter_profile))
        .route("/rank/rankups/{username}", get(rank_ups))
        .route("/rank/replay/{username}", get(replay))
        .route("/rank/replay/{username}/unseen-count", get(unseen_count))
        .route(
            "/rank/replay/{username}/seen-completion/{log_id}",
            post(seen_completion),
        )
        .route("/rank/replay/{username}/seen-entrance/{log_id}", post(seen_entrance))
        .route("/rank/replay/{username}/seen-rankup/{event_id}", post(seen_rankup))
        .route("/rank/sitewide", get(sitewide))
        .route("/rank/stats/speed", get(speed))
        .route("/rank/stats/projection", get(projection))
        .route("/rank/stats/volume", get(volume))
        .route("/rank/admin/challenges", post(create_challenge))
        .route("/rank/admin/challenges/batch", post(create_batch).patch(update_batch))
        .route(
            "/rank/admin/challenges/{id}",
            axum::routing::patch(update_challenge).delete(delete_challenge),
        )
        .route("/rank/admin/counter/{username}/thread-rank", post(set_thread_rank))
        .route("/rank/admin/cleanup-bad-chain-assignments", get(cleanup_chains))
        .route("/rank/admin/season/start", post(start_season))
        .route("/rank/admin/season/end", post(end_season))
}

fn pint(q: &Query, k: &str) -> Option<f64> {
    q.str(k)
        .filter(|s| !s.is_empty())
        .map(|s| counting::js::parse_int(s, None))
}

async fn resolved_season(state: &AppState, q: &Query) -> ApiResult<Option<i64>> {
    if let Some(s) = pint(q, "seasonId") {
        return Ok(Some(s as i64));
    }
    Ok(active_season(state).await?.and_then(|s| s["id"].as_i64()))
}

async fn season_by_id(state: &AppState, id: i64) -> ApiResult<Value> {
    Ok(db::one(
        state,
        &entity::RANK_SEASON,
        "SELECT * FROM rankseason WHERE id = ? LIMIT 1",
        &id.to_string(),
    )
    .await?
    .unwrap_or(Value::Null))
}

async fn counter_named(state: &AppState, username: &str) -> ApiResult<Value> {
    db::find_counter_by_username(state, username)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("Counter not found: {username}")))
}

async fn require_admin(state: &AppState, uuid: &str) -> ApiResult<Value> {
    match db::find_counter(state, uuid).await? {
        Some(c) if has_role(&c, "admin") => Ok(c),
        _ => Err(ApiError::bare(StatusCode::FORBIDDEN)),
    }
}

async fn season(State(state): State<AppState>) -> ApiResult<Reply> {
    Ok(reply(active_season(&state).await?.unwrap_or(Value::Null)))
}

async fn seasons(State(state): State<AppState>) -> ApiResult<Reply> {
    let rows = sqlx::query("SELECT * FROM rankseason ORDER BY startedAt DESC")
        .fetch_all(&state.pool)
        .await?;
    Ok(reply(entity::RANK_SEASON.rows(&rows)))
}

fn validate_rank(r: &str) -> ApiResult<()> {
    if RANK_ORDER.contains(&r) {
        Ok(())
    } else {
        Err(ApiError::bad_request(format!("Unknown rank: {r}")))
    }
}

fn validate_type(t: &str) -> ApiResult<()> {
    if CHALLENGE_TYPES.contains(&t) {
        Ok(())
    } else {
        Err(ApiError::bad_request(format!("Unknown challenge type: {t}")))
    }
}

async fn challenges(State(state): State<AppState>, q: Query) -> ApiResult<Reply> {
    let mut wh: Vec<String> = Vec::new();
    let mut args: Vec<Value> = Vec::new();
    if let Some(r) = q.str("rank").filter(|s| !s.is_empty()) {
        validate_rank(r)?;
        wh.push("`rank` = ?".into());
        args.push(json!(r));
    }
    if let Some(t) = q.str("type").filter(|s| !s.is_empty()) {
        validate_type(t)?;
        wh.push("type = ?".into());
        args.push(json!(t));
    }
    let sitewide = q.str("sitewideOnly") == Some("true");
    if sitewide {
        wh.push("threadUuid IS NULL".into());
    } else if let Some(t) = q.str("threadUuid").filter(|s| !s.is_empty()) {
        wh.push("threadUuid = ?".into());
        args.push(json!(t));
    }
    if let Some(s) = pint(&q, "seasonId") {
        wh.push("seasonId = ?".into());
        args.push(json!(s as i64));
    }
    let context = q.str("context") == Some("true");
    let max = if context { 100.0 } else { 5000.0 };
    let top_n = pint(&q, "topN").unwrap_or(5.0) as i64;
    let limit = pint(&q, "limit").map_or(50.0, |l| l.min(max)) as i64;
    let offset = pint(&q, "offset").unwrap_or(0.0) as i64;
    let where_sql = if wh.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", wh.join(" AND "))
    };
    let (total,): (i64,) = {
        let r = db::rows(
            &state,
            &format!("SELECT COUNT(*) AS n FROM rankchallenge {where_sql}"),
            &args,
        )
        .await?;
        (r.first()
            .map(|r| js_number(&entity::row_json(r)["n"]) as i64)
            .unwrap_or(0),)
    };
    let items: Vec<Value> = db::rows(
        &state,
        &format!(
            "SELECT * FROM rankchallenge {where_sql} ORDER BY `rank` ASC, sequence ASC LIMIT {limit} OFFSET {offset}"
        ),
        &args,
    )
    .await?
    .iter()
    .map(|r| entity::RANK_CHALLENGE.json(r))
    .collect();
    if !context {
        return Ok(reply(json!({ "total": total, "items": items })));
    }
    Ok(reply(
        json!({ "total": total, "items": attach_context(&state, items, top_n).await? }),
    ))
}

async fn attach_context(state: &AppState, items: Vec<Value>, top_n: i64) -> ApiResult<Vec<Value>> {
    if items.is_empty() {
        return Ok(items);
    }
    let ids: Vec<Value> = items.iter().map(|c| c["id"].clone()).collect();
    let ph = db::placeholders(ids.len());
    let counts = db::rows(
        state,
        &format!("SELECT challengeId, COUNT(*) AS n FROM challengelog WHERE challengeId IN ({ph}) AND completedAt IS NOT NULL GROUP BY challengeId"),
        &ids,
    )
    .await?;
    let counts: Vec<Value> = counts.iter().map(entity::row_json).collect();
    let logs: Vec<Value> = db::rows(
        state,
        &format!("SELECT * FROM challengelog WHERE challengeId IN ({ph}) AND completedAt IS NOT NULL ORDER BY completedAt ASC"),
        &ids,
    )
    .await?
    .iter()
    .map(|r| entity::CHALLENGE_LOG.json(r))
    .collect();
    let mut top: Vec<(Value, Vec<&Value>)> = Vec::new();
    for l in &logs {
        match top.iter_mut().find(|x| x.0 == l["challengeId"]) {
            Some(slot) => {
                if (slot.1.len() as i64) < top_n {
                    slot.1.push(l);
                }
            }
            None => top.push((l["challengeId"].clone(), if top_n > 0 { vec![l] } else { vec![] })),
        }
    }
    let mut uuids: Vec<String> = top
        .iter()
        .flat_map(|x| x.1.iter().filter_map(|l| l["counterUuid"].as_str().map(str::to_owned)))
        .collect();
    uuids.sort();
    uuids.dedup();
    let counters = db::find_counters(state, &uuids).await?;
    Ok(items
        .into_iter()
        .map(|mut c| {
            let id = c["id"].clone();
            let completers: Vec<Value> = top
                .iter()
                .find(|x| x.0 == id)
                .map(|x| x.1.clone())
                .unwrap_or_default()
                .iter()
                .map(|l| {
                    let k = counters.iter().find(|k| k["uuid"] == l["counterUuid"]);
                    json!({
                        "counterUuid": l["counterUuid"],
                        "username": k.map_or(json!(""), |k| k["username"].clone()),
                        "name": k.map_or(json!(""), |k| k["name"].clone()),
                        "completedAt": entity::js_number(js_number(&l["completedAt"])),
                        "ggAwarded": l["ggAwarded"],
                    })
                })
                .collect();
            c["completionCount"] = counts
                .iter()
                .find(|r| r["challengeId"] == id)
                .map_or(json!(0), |r| json!(js_number(&r["n"]) as i64));
            c["topCompleters"] = json!(completers);
            c
        })
        .collect())
}

async fn challenge_by_id(State(state): State<AppState>, Path(id): Path<String>, q: Query) -> ApiResult<Reply> {
    let t = templates(&state, "id = ?", &[json!(id)]).await?;
    if t.is_empty() {
        return Err(ApiError::not_found(format!("Challenge not found: {id}")));
    }
    let top_n = pint(&q, "topN").unwrap_or(10.0) as i64;
    let mut c = attach_context(&state, t, top_n).await?;
    Ok(reply(c.remove(0)))
}

fn card(k: Option<&Value>) -> [(&'static str, Value); 5] {
    let f = |key: &str| k.map_or(json!(""), |k| if k[key].is_null() { json!("") } else { k[key].clone() });
    [
        ("username", f("username")),
        ("name", f("name")),
        ("avatar", f("avatar")),
        ("discordId", f("discordId")),
        ("color", f("color")),
    ]
}

async fn thread_leaderboard(State(state): State<AppState>, Path(name): Path<String>, q: Query) -> ApiResult<Reply> {
    let Some(thread) = db::find_thread_by_name(&state, &name).await? else {
        return Err(ApiError::not_found(format!("Thread not found: {name}")));
    };
    let season = resolved_season(&state, &q).await?;
    let thread_view = json!({ "uuid": thread["uuid"], "name": thread["name"], "title": thread["title"] });
    let min = pint(&q, "minGG").filter(|m| m.is_finite());
    let ranks: Vec<Value> = match season {
        Some(sid) => {
            let mut args = vec![thread["uuid"].clone(), json!(sid)];
            let extra = match min {
                Some(m) => {
                    args.push(json!(m as i64));
                    " AND ggTotal >= ?"
                }
                None => "",
            };
            db::rows(
                &state,
                &format!("SELECT * FROM threadrank WHERE threadUuid = ? AND seasonId = ?{extra} ORDER BY ggTotal DESC, id ASC"),
                &args,
            )
            .await?
            .iter()
            .map(|r| entity::THREAD_RANK.json(r))
            .collect()
        }
        None => Vec::new(),
    };
    let season_v = match season {
        Some(s) => season_by_id(&state, s).await?,
        None => Value::Null,
    };
    if ranks.is_empty() {
        return Ok(reply(
            json!({ "season": season_v, "thread": thread_view, "entries": [] }),
        ));
    }
    let mut uuids: Vec<String> = ranks
        .iter()
        .filter_map(|r| r["counterUuid"].as_str().map(str::to_owned))
        .collect();
    uuids.sort();
    uuids.dedup();
    let counters = db::find_counters(&state, &uuids).await?;
    let entries: Vec<Value> = ranks
        .into_iter()
        .map(|mut r| {
            let k = counters.iter().find(|k| k["uuid"] == r["counterUuid"]);
            for (key, v) in card(k) {
                r[key] = v;
            }
            r
        })
        .collect();
    Ok(reply(
        json!({ "season": season_v, "thread": thread_view, "entries": entries }),
    ))
}

async fn join_templates(state: &AppState, logs: &[Value]) -> ApiResult<Vec<Value>> {
    let mut ids: Vec<Value> = Vec::new();
    for l in logs {
        if !ids.contains(&l["challengeId"]) {
            ids.push(l["challengeId"].clone());
        }
    }
    let ts = if ids.is_empty() {
        Vec::new()
    } else {
        templates(state, &format!("id IN ({})", db::placeholders(ids.len())), &ids).await?
    };
    let mut combos: Vec<(String, Vec<Value>)> = Vec::new();
    for t in &ts {
        let key = format!("{}|{}|{}", t["type"], t["rank"], t["seasonId"]);
        if combos.iter().any(|c| c.0 == key) {
            continue;
        }
        let siblings = templates(
            state,
            "type = ? AND `rank` = ? AND seasonId = ?",
            &[t["type"].clone(), t["rank"].clone(), t["seasonId"].clone()],
        )
        .await?;
        combos.push((key, siblings));
    }
    let seq_info = |t: &Value| {
        let key = format!("{}|{}|{}", t["type"], t["rank"], t["seasonId"]);
        let siblings = combos
            .iter()
            .find(|c| c.0 == key)
            .map(|c| c.1.as_slice())
            .unwrap_or(&[]);
        let mut chain: Vec<&Value> = siblings.iter().filter(|s| chain_key(s) == chain_key(t)).collect();
        chain.sort_by_key(|s| s["sequence"].as_i64());
        let pos = chain.iter().position(|s| s["id"] == t["id"]).map_or(0, |p| p + 1);
        (pos.max(1), chain.len().max(1))
    };
    Ok(logs
        .iter()
        .map(|l| {
            let t = ts.iter().find(|t| t["id"] == l["challengeId"]);
            let (pos, total) = t.map_or((1, 1), &seq_info);
            let mut o = l.clone();
            o["type"] = t.map_or(json!("thread_counts"), |t| t["type"].clone());
            o["params"] = t.map_or(Value::Null, |t| t["params"].clone());
            o["threadUuid"] = t.map_or(Value::Null, |t| t["threadUuid"].clone());
            o["rank"] = t.map_or(json!("bronze"), |t| t["rank"].clone());
            o["sequencePosition"] = json!(pos);
            o["sequenceTotal"] = json!(total);
            o["baseReassignmentWaitMs"] = t.map_or(Value::Null, |t| t["baseReassignmentWaitMs"].clone());
            o
        })
        .collect())
}

fn logs_of(rows: &[sqlx::mysql::MySqlRow]) -> Vec<Value> {
    rows.iter().map(|r| entity::CHALLENGE_LOG.json(r)).collect()
}

async fn counter_profile(State(state): State<AppState>, Path(username): Path<String>, q: Query) -> ApiResult<Reply> {
    let counter = counter_named(&state, &username).await?;
    let uuid = counter["uuid"].clone();
    let season = resolved_season(&state, &q).await?;
    let limit = pint(&q, "completionsLimit").map_or(20.0, |l| l.min(100.0)) as i64;
    let offset = pint(&q, "completionsOffset").unwrap_or(0.0) as i64;
    let ranks_raw: Vec<Value> = match season {
        Some(s) => {
            db::rows(
                &state,
                "SELECT * FROM threadrank WHERE counterUuid = ? AND seasonId = ? ORDER BY ggTotal DESC, id ASC",
                &[uuid.clone(), json!(s)],
            )
            .await?
        }
        None => {
            db::rows(
                &state,
                "SELECT * FROM threadrank WHERE counterUuid = ? ORDER BY ggTotal DESC, id ASC",
                std::slice::from_ref(&uuid),
            )
            .await?
        }
    }
    .iter()
    .map(|r| entity::THREAD_RANK.json(r))
    .collect();
    let mut tids: Vec<String> = ranks_raw
        .iter()
        .filter_map(|r| r["threadUuid"].as_str().map(str::to_owned))
        .collect();
    tids.sort();
    tids.dedup();
    let threads: Vec<Value> = if tids.is_empty() {
        Vec::new()
    } else {
        let a: Vec<Value> = tids.iter().map(|t| json!(t)).collect();
        db::rows(
            &state,
            &format!(
                "SELECT uuid, name, title FROM thread WHERE uuid IN ({})",
                db::placeholders(a.len())
            ),
            &a,
        )
        .await?
        .iter()
        .map(entity::row_json)
        .collect()
    };
    let ranks: Vec<Value> = ranks_raw
        .into_iter()
        .map(|mut r| {
            let name = r["threadUuid"]
                .as_str()
                .and_then(|u| threads.iter().find(|t| t["uuid"] == u))
                .map_or(Value::Null, |t| {
                    t["title"]
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .map_or(t["name"].clone(), |s| json!(s))
                });
            r["threadName"] = name;
            r
        })
        .collect();
    let (progress_raw, completions_raw) = match season {
        Some(s) => (
            logs_of(&db::rows(&state, "SELECT * FROM challengelog WHERE counterUuid = ? AND seasonId = ? AND completedAt IS NULL AND cancelledAt IS NULL ORDER BY id ASC", &[uuid.clone(), json!(s)]).await?),
            logs_of(&db::rows(&state, "SELECT * FROM challengelog WHERE counterUuid = ? AND seasonId = ? AND completedAt IS NOT NULL ORDER BY completedAt DESC, id DESC LIMIT 500", &[uuid.clone(), json!(s)]).await?),
        ),
        None => (Vec::new(), Vec::new()),
    };
    let mut all: Vec<Value> = progress_raw.clone();
    all.extend(completions_raw.iter().cloned());
    let joined = join_templates(&state, &all).await?;
    let (progress, completions) = joined.split_at(progress_raw.len());
    let progress = progress.to_vec();
    let mut completions = completions.to_vec();
    completions.sort_by(|a, b| {
        let d = rank_index(b["rank"].as_str().unwrap_or("")) - rank_index(a["rank"].as_str().unwrap_or(""));
        if d != 0 {
            return d.cmp(&0);
        }
        let g = js_number(&b["ggReward"]) - js_number(&a["ggReward"]);
        g.partial_cmp(&0.0).unwrap_or(std::cmp::Ordering::Equal)
    });
    let total = completions.len();
    let start = (offset.max(0) as usize).min(total);
    let end = (start + limit.max(0) as usize).min(total);
    let recent: Vec<Value> = completions[start..end].to_vec();
    let _ = &mut completions;
    Ok(reply(json!({
        "counter": {
            "uuid": counter["uuid"], "username": counter["username"], "name": counter["name"],
            "avatar": counter["avatar"], "discordId": counter["discordId"], "color": counter["color"],
        },
        "ranks": ranks,
        "challengeProgress": progress,
        "recentCompletions": recent,
        "recentCompletionsTotal": total,
    })))
}

async fn rank_ups(State(state): State<AppState>, Path(username): Path<String>, q: Query) -> ApiResult<Reply> {
    let counter = counter_named(&state, &username).await?;
    let Some(season) = resolved_season(&state, &q).await? else {
        return Ok(reply(json!({ "startedAt": null, "events": [] })));
    };
    let scope = q.str("scope").unwrap_or("sitewide");
    let scope_v = if scope == "sitewide" { Value::Null } else { json!(scope) };
    let first = db::rows(
        &state,
        "SELECT MIN(l.createdAt) AS at FROM challengelog l JOIN rankchallenge t ON t.id = l.challengeId \
         WHERE l.counterUuid = ? AND l.seasonId = ? AND l.context = 'rank' AND t.threadUuid <=> ?",
        &[counter["uuid"].clone(), json!(season), scope_v.clone()],
    )
    .await?;
    let at = first
        .first()
        .map(entity::row_json)
        .map_or(Value::Null, |r| r["at"].clone());
    let thread_sql = if scope_v.is_null() {
        "threadUuid IS NULL"
    } else {
        "threadUuid = ?"
    };
    let mut args = vec![counter["uuid"].clone(), json!(season)];
    if !scope_v.is_null() {
        args.push(scope_v);
    }
    let events: Vec<Value> = db::rows(
        &state,
        &format!("SELECT * FROM rankupevent WHERE counterUuid = ? AND seasonId = ? AND {thread_sql} ORDER BY createdAt ASC"),
        &args,
    )
    .await?
    .iter()
    .map(|r| {
        let e = entity::RANK_UP_EVENT.json(r);
        json!({ "toRank": e["toRank"], "toDivision": e["toDivision"], "createdAt": entity::js_number(js_number(&e["createdAt"])) })
    })
    .collect();
    Ok(reply(json!({
        "startedAt": if at.is_null() { Value::Null } else { entity::js_number(js_number(&at)) },
        "events": events,
    })))
}

async fn replay(State(state): State<AppState>, Path(username): Path<String>, q: Query) -> ApiResult<Reply> {
    let counter = counter_named(&state, &username).await?;
    let uuid = counter["uuid"].clone();
    let season = resolved_season(&state, &q).await?;
    let Some(sid) = season else {
        return Ok(reply(json!({ "unseenCompletions": [], "unseenRankUps": [] })));
    };
    let thread = q.str("threadUuid").filter(|t| !t.is_empty());
    let mut args = vec![uuid.clone(), json!(sid)];
    let extra = match thread {
        Some(t) => {
            args.push(json!(t));
            " AND EXISTS (SELECT 1 FROM rankchallenge rc WHERE rc.id = log.challengeId AND (rc.threadUuid IS NULL OR rc.threadUuid = ?))"
        }
        None => "",
    };
    let raw = logs_of(
        &db::rows(
            &state,
            &format!(
                "SELECT log.* FROM challengelog log WHERE log.counterUuid = ? AND log.seasonId = ? AND log.completedAt IS NOT NULL AND log.seenAt IS NULL{extra} \
                 ORDER BY log.completedAt ASC, log.id ASC LIMIT 100"
            ),
            &args,
        )
        .await?,
    );
    let rankups: Vec<Value> = db::rows(
        &state,
        "SELECT * FROM rankupevent e WHERE e.counterUuid = ? AND e.seasonId = ? AND e.seenAt IS NULL ORDER BY e.createdAt ASC LIMIT 100",
        &[uuid, json!(sid)],
    )
    .await?
    .iter()
    .map(|r| entity::RANK_UP_EVENT.json(r))
    .collect();
    let joined = join_templates(&state, &raw).await?;
    let mut ids: Vec<Value> = Vec::new();
    for l in &raw {
        if !ids.contains(&l["challengeId"]) {
            ids.push(l["challengeId"].clone());
        }
    }
    let ts = if ids.is_empty() {
        Vec::new()
    } else {
        templates(&state, &format!("id IN ({})", db::placeholders(ids.len())), &ids).await?
    };
    let unseen: Vec<Value> = joined
        .into_iter()
        .map(|mut l| {
            let key = ts
                .iter()
                .find(|t| t["id"] == l["challengeId"])
                .map_or_else(|| crate::http::js_string(&l["challengeId"]), chain_key);
            l["chainKey"] = json!(key);
            l
        })
        .collect();
    Ok(reply(json!({ "unseenCompletions": unseen, "unseenRankUps": rankups })))
}

async fn unseen_count(State(state): State<AppState>, Path(username): Path<String>, q: Query) -> ApiResult<Reply> {
    let counter = counter_named(&state, &username).await?;
    let Some(sid) = resolved_season(&state, &q).await? else {
        return Ok(reply(json!({ "total": 0, "byThread": {}, "sitewide": 0 })));
    };
    let logs = logs_of(
        &db::rows(
            &state,
            "SELECT * FROM challengelog log WHERE log.counterUuid = ? AND log.seasonId = ? AND log.completedAt IS NOT NULL AND log.seenAt IS NULL",
            &[counter["uuid"].clone(), json!(sid)],
        )
        .await?,
    );
    let mut ids: Vec<Value> = Vec::new();
    for l in &logs {
        if !ids.contains(&l["challengeId"]) {
            ids.push(l["challengeId"].clone());
        }
    }
    let ts = if ids.is_empty() {
        Vec::new()
    } else {
        templates(&state, &format!("id IN ({})", db::placeholders(ids.len())), &ids).await?
    };
    let mut by: Map<String, Value> = Map::new();
    let mut sitewide = 0;
    for l in &logs {
        match ts
            .iter()
            .find(|t| t["id"] == l["challengeId"])
            .and_then(|t| t["threadUuid"].as_str())
        {
            None => sitewide += 1,
            Some(t) => {
                let n = by.get(t).and_then(Value::as_i64).unwrap_or(0) + 1;
                by.insert(t.to_owned(), json!(n));
            }
        }
    }
    Ok(reply(
        json!({ "total": logs.len(), "byThread": by, "sitewide": sitewide }),
    ))
}

async fn require_self(state: &AppState, uuid: &str, username: &str) -> ApiResult<Value> {
    let c = counter_named(state, username).await?;
    if c["uuid"] != uuid {
        return Err(ApiError::bare(StatusCode::FORBIDDEN));
    }
    Ok(c)
}

async fn mark(
    state: &AppState,
    table: &str,
    column: &str,
    id: &str,
    counter: &Value,
    missing: String,
) -> ApiResult<()> {
    let n = counting::js::parse_int(id, None);
    let row = if n.is_finite() {
        db::rows(
            state,
            &format!("SELECT * FROM {table} WHERE id = ? LIMIT 1"),
            &[json!(n as i64)],
        )
        .await?
    } else {
        Vec::new()
    };
    let Some(r) = row.first().map(entity::row_json) else {
        return Err(ApiError::not_found(missing));
    };
    if r["counterUuid"] != counter["uuid"] {
        return Err(ApiError::not_found(missing));
    }
    if r[column].is_null() {
        db::exec(
            state,
            &format!("UPDATE {table} SET {column} = ? WHERE id = ?"),
            &[json!(now_ms()), r["id"].clone()],
        )
        .await?;
    }
    Ok(())
}

async fn seen_completion(
    State(state): State<AppState>,
    Authed(user): Authed,
    Path((username, log_id)): Path<(String, String)>,
) -> ApiResult<Reply> {
    let c = require_self(&state, &user.uuid, &username).await?;
    let missing = format!(
        "Challenge log not found for this counter: {}",
        counting::js::num_to_string(counting::js::parse_int(&log_id, None))
    );
    mark(&state, "challengelog", "seenAt", &log_id, &c, missing).await?;
    Ok(reply(json!({ "ok": true })))
}

async fn seen_entrance(
    State(state): State<AppState>,
    Authed(user): Authed,
    Path((username, log_id)): Path<(String, String)>,
) -> ApiResult<Reply> {
    let c = require_self(&state, &user.uuid, &username).await?;
    let missing = format!(
        "Challenge log not found for this counter: {}",
        counting::js::num_to_string(counting::js::parse_int(&log_id, None))
    );
    mark(&state, "challengelog", "entranceSeenAt", &log_id, &c, missing).await?;
    Ok(reply(json!({ "ok": true })))
}

async fn seen_rankup(
    State(state): State<AppState>,
    Authed(user): Authed,
    Path((username, event_id)): Path<(String, String)>,
) -> ApiResult<Reply> {
    let c = require_self(&state, &user.uuid, &username).await?;
    let missing = format!(
        "Rank-up event not found for this counter: {}",
        counting::js::num_to_string(counting::js::parse_int(&event_id, None))
    );
    mark(&state, "rankupevent", "seenAt", &event_id, &c, missing).await?;
    Ok(reply(json!({ "ok": true })))
}

async fn sitewide(State(state): State<AppState>, q: Query) -> ApiResult<Reply> {
    let Some(sid) = resolved_season(&state, &q).await? else {
        return Ok(reply(json!({ "season": null, "entries": [], "total": 0 })));
    };
    let (total,): (i64,) = sqlx::query_as(
        "SELECT COUNT(DISTINCT tr.counterUuid) FROM threadrank tr WHERE tr.seasonId = ? AND tr.threadUuid IS NULL AND tr.ggTotal > 0",
    )
    .bind(sid)
    .fetch_one(&state.pool)
    .await?;
    let season = season_by_id(&state, sid).await?;
    if total == 0 {
        return Ok(reply(json!({ "season": season, "entries": [], "total": 0 })));
    }
    let limit = pint(&q, "limit").map(|l| l.min(100.0) as i64);
    let offset = pint(&q, "offset").unwrap_or(0.0) as i64;
    let lim = limit.map_or_else(|| "18446744073709551615".to_owned(), |l| l.to_string());
    let rows: Vec<Value> = db::rows(
        &state,
        &format!(
            "SELECT * FROM threadrank tr WHERE tr.seasonId = ? AND tr.threadUuid IS NULL AND tr.ggTotal > 0 ORDER BY tr.ggTotal DESC, tr.id ASC LIMIT {lim} OFFSET {offset}"
        ),
        &[json!(sid)],
    )
    .await?
    .iter()
    .map(|r| entity::THREAD_RANK.json(r))
    .collect();
    if rows.is_empty() {
        return Ok(reply(json!({ "season": season, "entries": [], "total": total })));
    }
    let uuids: Vec<String> = rows
        .iter()
        .filter_map(|r| r["counterUuid"].as_str().map(str::to_owned))
        .collect();
    let counters = db::find_counters(&state, &uuids).await?;
    let entries: Vec<Value> = rows
        .iter()
        .map(|r| {
            let k = counters.iter().find(|k| k["uuid"] == r["counterUuid"]);
            let mut e = Map::new();
            e.insert("counterUuid".into(), r["counterUuid"].clone());
            for (key, v) in card(k) {
                e.insert(key.into(), v);
            }
            e.insert("totalGg".into(), r["ggTotal"].clone());
            e.insert("rank".into(), r["rank"].clone());
            e.insert("division".into(), r["division"].clone());
            Value::Object(e)
        })
        .collect();
    Ok(reply(json!({ "season": season, "entries": entries, "total": total })))
}

async fn speed(State(state): State<AppState>, q: Query) -> ApiResult<Reply> {
    let zero = json!({ "p10": 0, "p25": 0, "p50": 0, "p75": 0, "p90": 0, "sampleSize": 0 });
    let or = |k: &str, d: f64| pint(&q, k).filter(|n| *n != 0.0 && !n.is_nan()).unwrap_or(d) as i64;
    let cps = or("countsPerSplit", 100.0);
    let spg = or("splitsPerGet", 10.0);
    let n = if q.str("type") == Some("get") { cps * spg } else { cps };
    let Some(thread) = db::find_thread_by_uuid(&state, q.str("threadUuid").unwrap_or("")).await? else {
        return Ok(reply(zero));
    };
    let tid = thread["id"].clone();
    let max: Option<i64> = sqlx::query_scalar("SELECT MAX(p.validCountNumber) FROM post p WHERE p.threadId = ?")
        .bind(tid.as_i64())
        .fetch_one(&state.pool)
        .await?;
    let min_vcn = max.unwrap_or(0) - n * 10000;
    let raw: Vec<Option<String>> = sqlx::query_scalar(
        "SELECT p.timeSinceLastCount FROM post p WHERE p.threadId = ? AND p.validCountNumber > ? AND p.isValidCount = 1 \
         AND MOD(p.validCountNumber, ?) = 0 AND p.timeSinceLastCount IS NOT NULL ORDER BY p.validCountNumber DESC LIMIT 10000",
    )
    .bind(tid.as_i64())
    .bind(min_vcn)
    .bind(n)
    .fetch_all(&state.pool)
    .await?;
    if raw.is_empty() {
        return Ok(reply(zero));
    }
    let mut values: Vec<f64> = raw
        .iter()
        .map(|v| counting::js::parse_float(v.as_deref().unwrap_or("")))
        .filter(|v| *v > 0.0)
        .collect();
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let pct = |p: f64| -> Value {
        if values.is_empty() {
            return Value::Null;
        }
        let i = ((p / 100.0) * (values.len() as f64 - 1.0)).floor() as usize;
        entity::js_number((values[i] + 0.5).floor())
    };
    Ok(reply(json!({
        "p10": pct(10.0), "p25": pct(25.0), "p50": pct(50.0), "p75": pct(75.0), "p90": pct(90.0),
        "sampleSize": values.len(),
    })))
}

async fn projection(State(state): State<AppState>, q: Query) -> ApiResult<Reply> {
    let Some(counter) = db::find_counter_by_username(&state, q.str("username").unwrap_or("")).await? else {
        return Ok(reply(Value::Null));
    };
    let Some(sid) = resolved_season(&state, &q).await? else {
        return Ok(reply(Value::Null));
    };
    let row = db::rows(
        &state,
        "SELECT * FROM threadrank WHERE counterUuid = ? AND seasonId = ? AND threadUuid IS NULL LIMIT 1",
        &[counter["uuid"].clone(), json!(sid)],
    )
    .await?;
    let Some(rank_row) = row.first().map(|r| entity::THREAD_RANK.json(r)) else {
        return Ok(reply(Value::Null));
    };
    let since = now_ms() - 7 * 24 * 60 * 60 * 1000;
    let sum = db::rows(
        &state,
        "SELECT SUM(log.ggAwarded) AS totalGg FROM challengelog log WHERE log.counterUuid = ? AND log.seasonId = ? AND log.completedAt IS NOT NULL AND log.completedAt > ?",
        &[counter["uuid"].clone(), json!(sid), json!(since)],
    )
    .await?;
    let recent = sum.first().map(entity::row_json).map_or(0.0, |r| {
        let n = counting::js::parse_int(&crate::http::js_string(&r["totalGg"]), None);
        if n.is_nan() { 0.0 } else { n }
    });
    let daily = (recent / 7.0 + 0.5).floor();
    let rank = rank_row["rank"].as_str().unwrap_or("bronze");
    let idx = rank_index(rank);
    let next = if idx >= 0 && (idx as usize) < RANK_ORDER.len() - 1 {
        json!(RANK_ORDER[idx as usize + 1])
    } else if idx < 0 {
        json!(RANK_ORDER[0])
    } else {
        Value::Null
    };
    let total = gg_per_rank(rank);
    let needed = if total.is_finite() {
        total - js_number(&rank_row["gg"])
    } else {
        0.0
    };
    Ok(reply(json!({
        "dailyGgAvg": entity::js_number(daily),
        "currentGgTotal": rank_row["ggTotal"],
        "targetRank": next,
        "targetRankGgNeeded": entity::js_number(needed),
        "daysToTarget": if daily > 0.0 && needed > 0.0 { entity::js_number((needed / daily).ceil()) } else { Value::Null },
        "currentRank": rank_row["rank"],
        "currentDivision": rank_row["division"],
    })))
}

async fn volume(State(state): State<AppState>, q: Query) -> ApiResult<Reply> {
    let Some(sid) = resolved_season(&state, &q).await? else {
        return Ok(reply(json!([])));
    };
    let season = season_by_id(&state, sid).await?;
    if season.is_null() {
        return Ok(reply(json!([])));
    }
    let bucket_days = pint(&q, "bucketDays").unwrap_or(7.0);
    let bucket_ms = bucket_days * 86_400_000.0;
    let started = js_number(&season["startedAt"]);
    let thread = q.str("threadUuid").filter(|t| !t.is_empty());
    let mut args = vec![json!(started as i64), json!(bucket_ms), json!(sid)];
    let extra = match thread {
        Some(t) => {
            args.push(json!(t));
            "log.challengeId IN (SELECT id FROM rankchallenge WHERE threadUuid = ?)"
        }
        None => "1=1",
    };
    let rows = db::rows(
        &state,
        &format!(
            "SELECT FLOOR((log.completedAt - ?) / ?) AS bucket, COUNT(*) AS count FROM challengelog log \
             WHERE log.completedAt IS NOT NULL AND log.seasonId = ? AND {extra} GROUP BY bucket ORDER BY bucket ASC LIMIT 12"
        ),
        &args,
    )
    .await?;
    let out: Vec<Value> = rows
        .iter()
        .map(|r| {
            let j = entity::row_json(r);
            let b = js_number(&j["bucket"]);
            let start = started + b * bucket_ms;
            let day = chrono::DateTime::from_timestamp_millis(start as i64)
                .map(|t| entity::js_iso(t)[..10].to_owned())
                .unwrap_or_default();
            json!({ "bucket": day, "count": js_number(&j["count"]) as i64 })
        })
        .collect();
    Ok(reply(json!(out)))
}

fn sorted_json(v: &Value) -> String {
    fn norm(v: &Value) -> Value {
        match v {
            Value::Object(o) => {
                let mut k: Vec<&String> = o.keys().collect();
                k.sort();
                Value::Object(k.into_iter().map(|k| (k.clone(), norm(&o[k]))).collect())
            }
            Value::Array(a) => Value::Array(a.iter().map(norm).collect()),
            other => other.clone(),
        }
    }
    norm(v).to_string()
}

fn build_challenge(dto: &Value, season: i64) -> ApiResult<Value> {
    let t = dto["type"].as_str().unwrap_or("").to_owned();
    validate_type(&t)?;
    let r = dto["rank"].as_str().unwrap_or("").to_owned();
    validate_rank(&r)?;
    let cat = match dto["bingoCategory"].as_str().filter(|c| !c.is_empty()) {
        Some(c) if BINGO_CATEGORIES.contains(&c) => json!(c),
        Some(c) => return Err(ApiError::bad_request(format!("Unknown bingo category: {c}"))),
        None => Value::Null,
    };
    let wait = if dto.get("baseReassignmentWaitMs").is_some() {
        dto["baseReassignmentWaitMs"].clone()
    } else {
        default_wait_ms(&t).map_or(Value::Null, Value::from)
    };
    let c = json!({
        "type": t, "rank": r, "sequence": dto["sequence"], "ggReward": dto["ggReward"], "target": dto["target"],
        "params": dto["params"], "threadUuid": dto["threadUuid"], "bingoCategory": cat,
        "rewardType": dto["rewardType"].as_str().unwrap_or("gg"), "seasonId": season, "clonedFromId": null,
        "createdAt": now_ms(), "baseReassignmentWaitMs": wait,
    });
    validate_thread(&c)?;
    validate_params(&c)?;
    Ok(c)
}

fn validate_thread(c: &Value) -> ApiResult<()> {
    let t = c["type"].as_str().unwrap_or("");
    let has = !c["threadUuid"].is_null() && c["threadUuid"] != "";
    if [
        "split_under_ms",
        "get_under_ms",
        "bars_within_ms",
        "count_attempts",
        "roll",
    ]
    .contains(&t)
        && !has
    {
        return Err(ApiError::bad_request(format!(
            "{t} challenges require a threadUuid; they can't be sitewide."
        )));
    }
    if SITEWIDE_ONLY_TYPES.contains(&t) && has {
        return Err(ApiError::bad_request(format!(
            "{t} challenges are sitewide; they cannot have a threadUuid."
        )));
    }
    Ok(())
}

fn validate_params(c: &Value) -> ApiResult<()> {
    let t = c["type"].as_str().unwrap_or("");
    if t == "thread_breadth" || t == "thread_spread" {
        let n = js_number(&c["params"]["minThreads"]);
        if !(n.is_finite() && n.fract() == 0.0 && n >= 1.0) || c["params"]["minThreads"].is_null() {
            return Err(ApiError::bad_request(format!(
                "{t} challenges need params.minThreads, a whole number of threads >= 1."
            )));
        }
    }
    if t == "fast_gets" {
        let r = js_number(&c["params"]["minCountsPerSecond"]);
        if !r.is_finite() || r <= 0.0 {
            return Err(ApiError::bad_request(
                "fast_gets challenges need params.minCountsPerSecond, a rate > 0.",
            ));
        }
    }
    Ok(())
}

fn validate_sequence(c: &Value, siblings: &[Value]) -> ApiResult<()> {
    let combo: Vec<&Value> = siblings
        .iter()
        .filter(|s| s["type"] == c["type"] && s["rank"] == c["rank"] && chain_key(s) == chain_key(c))
        .collect();
    if let Some(d) = combo.iter().find(|s| s["sequence"].as_f64() == c["sequence"].as_f64()) {
        return Err(ApiError::bad_request(format!(
            "sequence {} is already used by another {}/{} challenge in this chain (id={}). Sequence values must be unique within a chain.",
            crate::http::js_string(&c["sequence"]),
            crate::http::js_string(&c["type"]),
            crate::http::js_string(&c["rank"]),
            crate::http::js_string(&d["id"])
        )));
    }
    let defined = |v: &Value| (!v.is_null()).then(|| js_number(v));
    let mut seqs: Vec<Option<f64>> = combo.iter().map(|s| defined(&s["sequence"])).collect();
    seqs.push(defined(&c["sequence"]));
    seqs.sort_by(|a, b| match (a, b) {
        (Some(x), Some(y)) => x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
    for (i, s) in seqs.iter().enumerate() {
        if *s != Some(i as f64) {
            let list: Vec<String> = seqs
                .iter()
                .map(|x| x.map(counting::js::num_to_string).unwrap_or_default())
                .collect();
            let shown = if c["sequence"].is_null() {
                "undefined".to_owned()
            } else {
                crate::http::js_string(&c["sequence"])
            };
            return Err(ApiError::bad_request(format!(
                "sequence {} would leave a gap in this chain; sequences for a {}/{} chain must be contiguous starting at 0 (currently: {}).",
                shown,
                crate::http::js_string(&c["type"]),
                crate::http::js_string(&c["rank"]),
                list.join(", ")
            )));
        }
    }
    Ok(())
}

async fn siblings(state: &AppState, c: &Value, season: &Value, exclude: Option<&Value>) -> ApiResult<Vec<Value>> {
    let rows = templates(
        state,
        "type = ? AND `rank` = ? AND seasonId = ?",
        &[c["type"].clone(), c["rank"].clone(), season.clone()],
    )
    .await?;
    Ok(rows
        .into_iter()
        .filter(|r| exclude.is_none_or(|e| r["id"] != *e))
        .collect())
}

fn json_col(v: &Value) -> Value {
    if v.is_null() { Value::Null } else { json!(v.to_string()) }
}

async fn insert_challenge(state: &AppState, c: &Value) -> ApiResult<Value> {
    let id = uuid_v4();
    db::exec(
        state,
        "INSERT INTO rankchallenge (id, type, `rank`, sequence, ggReward, rewardType, target, params, threadUuid, bingoCategory, seasonId, clonedFromId, baseReassignmentWaitMs, createdAt) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        &[
            json!(id), c["type"].clone(), c["rank"].clone(), c["sequence"].clone(), c["ggReward"].clone(),
            c["rewardType"].clone(), c["target"].clone(), json_col(&c["params"]), c["threadUuid"].clone(),
            c["bingoCategory"].clone(), c["seasonId"].clone(), c["clonedFromId"].clone(),
            c["baseReassignmentWaitMs"].clone(), c["createdAt"].clone(),
        ],
    )
    .await?;
    Ok(templates(state, "id = ?", &[json!(id)])
        .await?
        .into_iter()
        .next()
        .unwrap_or(Value::Null))
}

async fn require_season(state: &AppState, msg: &str) -> ApiResult<i64> {
    active_season(state)
        .await?
        .and_then(|s| s["id"].as_i64())
        .ok_or_else(|| ApiError::bad_request(msg.to_owned()))
}

async fn create_challenge(State(state): State<AppState>, Authed(user): Authed, body: JsonBody) -> ApiResult<Reply> {
    require_admin(&state, &user.uuid).await?;
    let season = require_season(&state, "No active season. Start a season before creating challenges.").await?;
    let c = build_challenge(&body.0, season)?;
    validate_sequence(&c, &siblings(&state, &c, &json!(season), None).await?)?;
    let saved = insert_challenge(&state, &c).await?;
    assign_eligible_for_template(&state, &saved, season).await?;
    Ok(reply(saved))
}

fn same_template(a: &Value, b: &Value) -> bool {
    let n = |v: &Value| if v.is_null() { None } else { Some(js_number(v)) };
    a["type"] == b["type"]
        && a["rank"] == b["rank"]
        && a["threadUuid"] == b["threadUuid"]
        && chain_key(a) == chain_key(b)
        && js_number(&a["sequence"]) == js_number(&b["sequence"])
        && js_number(&a["target"]) == js_number(&b["target"])
        && js_number(&a["ggReward"]) == js_number(&b["ggReward"])
        && a["rewardType"] == b["rewardType"]
        && n(&a["baseReassignmentWaitMs"]) == n(&b["baseReassignmentWaitMs"])
        && a["bingoCategory"] == b["bingoCategory"]
        && sorted_json(&a["params"]) == sorted_json(&b["params"])
}

async fn create_batch(State(state): State<AppState>, Authed(user): Authed, body: JsonBody) -> ApiResult<Reply> {
    require_admin(&state, &user.uuid).await?;
    let season = require_season(&state, "No active season — start a season before creating challenges.").await?;
    let dtos = body.0.as_array().cloned().unwrap_or_default();
    let built: Vec<Value> = dtos
        .iter()
        .map(|d| build_challenge(d, season))
        .collect::<ApiResult<_>>()?;
    let mut accepted: Vec<Value> = Vec::new();
    let mut existing: Vec<Value> = Vec::new();
    for c in built {
        let db_sib = siblings(&state, &c, &json!(season), None).await?;
        if let Some(same) = db_sib.iter().find(|s| {
            s["type"] == c["type"]
                && s["rank"] == c["rank"]
                && chain_key(s) == chain_key(&c)
                && js_number(&s["sequence"]) == js_number(&c["sequence"])
        }) {
            if !same_template(same, &c) {
                db::exec(
                    &state,
                    "UPDATE rankchallenge SET target = ?, ggReward = ?, rewardType = ?, params = ?, baseReassignmentWaitMs = ?, bingoCategory = ? WHERE id = ?",
                    &[c["target"].clone(), c["ggReward"].clone(), c["rewardType"].clone(), json_col(&c["params"]), c["baseReassignmentWaitMs"].clone(), c["bingoCategory"].clone(), same["id"].clone()],
                )
                .await?;
            }
            existing.push(
                templates(&state, "id = ?", &[same["id"].clone()])
                    .await?
                    .into_iter()
                    .next()
                    .unwrap_or(Value::Null),
            );
            continue;
        }
        let mut all = db_sib.clone();
        all.extend(accepted.iter().cloned());
        validate_sequence(&c, &all)?;
        accepted.push(c);
    }
    let mut saved = Vec::new();
    for c in &accepted {
        saved.push(insert_challenge(&state, c).await?);
    }
    let s = state.clone();
    let to_assign = saved.clone();
    tokio::spawn(async move {
        for c in to_assign {
            if let Err(e) = assign_eligible_for_template(&s, &c, season).await {
                tracing::error!("[rank] assigning new template {} failed: {e:?}", c["id"]);
            }
        }
    });
    saved.extend(existing);
    Ok(reply(json!(saved)))
}

async fn find_challenge(state: &AppState, id: &str) -> ApiResult<Value> {
    templates(state, "id = ?", &[json!(id)])
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| ApiError::not_found(format!("Challenge not found: {id}")))
}

async fn require_active_for_edit(state: &AppState, season: &Value) -> ApiResult<()> {
    let active = active_season(state).await?;
    if active.is_none_or(|a| a["id"].as_i64() != season.as_i64()) {
        return Err(ApiError::bad_request(
            "This challenge belongs to a past season and is read-only. Only the active season's challenges can be edited.",
        ));
    }
    Ok(())
}

async fn update_one(state: &AppState, id: &str, dto: &Value) -> ApiResult<Value> {
    let mut c = find_challenge(state, id).await?;
    require_active_for_edit(state, &c["seasonId"]).await?;
    let set = |k: &str| !dto[k].is_null();
    if let Some(r) = dto["rank"].as_str() {
        validate_rank(r)?;
        c["rank"] = json!(r);
    } else if set("rank") {
        validate_rank(&crate::http::js_string(&dto["rank"]))?;
    }
    for k in ["sequence", "ggReward", "target", "rewardType"] {
        if set(k) {
            c[k] = dto[k].clone();
        }
    }
    for k in ["params", "threadUuid", "baseReassignmentWaitMs"] {
        if dto.get(k).is_some() {
            c[k] = dto[k].clone();
        }
    }
    if dto.get("bingoCategory").is_some() {
        c["bingoCategory"] = match dto["bingoCategory"].as_str().filter(|x| !x.is_empty()) {
            Some(x) if BINGO_CATEGORIES.contains(&x) => json!(x),
            Some(x) => return Err(ApiError::bad_request(format!("Unknown bingo category: {x}"))),
            None => Value::Null,
        };
    }
    validate_thread(&c)?;
    validate_params(&c)?;
    validate_sequence(&c, &siblings(state, &c, &c["seasonId"], Some(&c["id"])).await?)?;
    db::exec(
        state,
        "UPDATE rankchallenge SET `rank` = ?, sequence = ?, ggReward = ?, target = ?, params = ?, threadUuid = ?, bingoCategory = ?, rewardType = ?, baseReassignmentWaitMs = ? WHERE id = ?",
        &[c["rank"].clone(), c["sequence"].clone(), c["ggReward"].clone(), c["target"].clone(), json_col(&c["params"]), c["threadUuid"].clone(), c["bingoCategory"].clone(), c["rewardType"].clone(), c["baseReassignmentWaitMs"].clone(), c["id"].clone()],
    )
    .await?;
    let saved = find_challenge(state, id).await?;
    if saved["sequence"].as_i64() == Some(0)
        && let Some(sid) = saved["seasonId"].as_i64()
    {
        assign_eligible_for_template(state, &saved, sid).await?;
    }
    Ok(saved)
}

async fn update_challenge(
    State(state): State<AppState>,
    Authed(user): Authed,
    Path(id): Path<String>,
    body: JsonBody,
) -> ApiResult<Reply> {
    require_admin(&state, &user.uuid).await?;
    Ok(reply(update_one(&state, &id, &body.0).await?))
}

async fn update_batch(State(state): State<AppState>, Authed(user): Authed, body: JsonBody) -> ApiResult<Reply> {
    require_admin(&state, &user.uuid).await?;
    let mut out = Vec::new();
    for u in body.0.as_array().cloned().unwrap_or_default() {
        out.push(update_one(&state, &crate::http::js_string(&u["id"]), &u["dto"]).await?);
    }
    Ok(reply(json!(out)))
}

async fn delete_challenge(
    State(state): State<AppState>,
    Authed(user): Authed,
    Path(id): Path<String>,
) -> ApiResult<Reply> {
    require_admin(&state, &user.uuid).await?;
    let c = find_challenge(&state, &id).await?;
    require_active_for_edit(&state, &c["seasonId"]).await?;
    db::exec(&state, "DELETE FROM rankchallenge WHERE id = ?", &[c["id"].clone()]).await?;
    Ok(reply(json!({ "deleted": id })))
}

async fn set_thread_rank(
    State(state): State<AppState>,
    Authed(user): Authed,
    Path(username): Path<String>,
    body: JsonBody,
) -> ApiResult<Reply> {
    require_admin(&state, &user.uuid).await?;
    let counter = counter_named(&state, &username).await?;
    let uuid = counter["uuid"].clone();
    let season = match body.get("seasonId").as_i64() {
        Some(s) => s,
        None => require_season(&state, "No active season and no seasonId provided.").await?,
    };
    let rank = body.str("rank").unwrap_or("").to_owned();
    validate_rank(&rank)?;
    let division = if rank == "peak" {
        json!(3)
    } else {
        body.get("division").clone()
    };
    let thread = body.get("threadUuid").clone();
    let tclause = if thread.is_null() {
        "threadUuid IS NULL"
    } else {
        "threadUuid = ?"
    };
    let mut args = vec![uuid.clone(), json!(season)];
    if !thread.is_null() {
        args.push(thread.clone());
    }
    let existing = db::rows(
        &state,
        &format!("SELECT * FROM threadrank WHERE counterUuid = ? AND seasonId = ? AND {tclause} LIMIT 1"),
        &args,
    )
    .await?;
    let old = existing.first().map(|r| entity::THREAD_RANK.json(r));
    let old_rank = old
        .as_ref()
        .and_then(|o| o["rank"].as_str().map(str::to_owned))
        .unwrap_or_else(|| "bronze".into());
    let id = match &old {
        Some(o) => {
            db::exec(
                &state,
                "UPDATE threadrank SET `rank` = ?, division = ?, gg = ? WHERE id = ?",
                &[json!(rank), division.clone(), body.get("gg").clone(), o["id"].clone()],
            )
            .await?;
            o["id"].clone()
        }
        None => {
            let r = db::exec(
                &state,
                "INSERT INTO threadrank (counterUuid, threadUuid, seasonId, `rank`, division, gg, ggTotal) VALUES (?, ?, ?, ?, ?, ?, 0)",
                &[uuid.clone(), thread.clone(), json!(season), json!(rank), division.clone(), body.get("gg").clone()],
            )
            .await?;
            json!(r.last_insert_id())
        }
    };
    if old_rank != rank {
        reconcile_on_rank_up(
            &state,
            uuid.as_str().unwrap_or(""),
            &old_rank,
            &thread,
            &rank,
            Some(season),
        )
        .await?;
    }
    let saved = db::rows(&state, "SELECT * FROM threadrank WHERE id = ?", &[id]).await?;
    Ok(reply(
        saved.first().map_or(Value::Null, |r| entity::THREAD_RANK.json(r)),
    ))
}

async fn cleanup_chains(State(state): State<AppState>, Authed(user): Authed, q: Query) -> ApiResult<Reply> {
    require_admin(&state, &user.uuid).await?;
    Ok(reply(
        cleanup_bad_chains(&state, q.str("dryRun") != Some("false")).await?,
    ))
}

async fn start_season(State(state): State<AppState>, Authed(user): Authed, body: JsonBody) -> ApiResult<Reply> {
    require_admin(&state, &user.uuid).await?;
    if let Some(existing) = active_season(&state).await? {
        return Err(ApiError::bad_request(format!(
            "Season \"{}\" is already active (id={}). End it first.",
            crate::http::js_string(&existing["name"]),
            crate::http::js_string(&existing["id"])
        )));
    }
    let previous = db::rows(&state, "SELECT * FROM rankseason ORDER BY startedAt DESC LIMIT 1", &[]).await?;
    let previous = previous.first().map(|r| entity::RANK_SEASON.json(r));
    let res = db::exec(
        &state,
        "INSERT INTO rankseason (name, startedAt, endedAt) VALUES (?, ?, NULL)",
        &[body.get("name").clone(), json!(now_ms())],
    )
    .await?;
    let sid = res.last_insert_id() as i64;
    if let Some(prev) = previous {
        let old = templates(&state, "seasonId = ?", &[prev["id"].clone()]).await?;
        let now = now_ms();
        let mut clones = Vec::new();
        for t in &old {
            let mut c = t.clone();
            c["seasonId"] = json!(sid);
            c["clonedFromId"] = t["id"].clone();
            c["createdAt"] = json!(now);
            c["rewardType"] = json!("gg");
            clones.push(insert_challenge(&state, &c).await?);
        }
        for c in &clones {
            assign_eligible_for_template(&state, c, sid).await?;
        }
    }
    Ok(reply(season_by_id(&state, sid).await?))
}

async fn end_season(State(state): State<AppState>, Authed(user): Authed) -> ApiResult<Reply> {
    require_admin(&state, &user.uuid).await?;
    let Some(s) = active_season(&state).await? else {
        return Err(ApiError::bad_request("No active season to end."));
    };
    db::exec(
        &state,
        "UPDATE rankseason SET endedAt = ? WHERE id = ?",
        &[json!(now_ms()), s["id"].clone()],
    )
    .await?;
    Ok(reply(season_by_id(&state, s["id"].as_i64().unwrap_or_default()).await?))
}
