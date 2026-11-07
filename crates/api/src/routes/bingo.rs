use crate::db;
use crate::entity;
use crate::error::{ApiError, ApiResult};
use crate::http::{Query, Reply, js_number, reply};
use crate::rank::rank_index;
use crate::state::AppState;
use axum::Router;
use axum::extract::{Path, State};
use axum::routing::get;
use serde_json::{Value, json};
use sqlx::Row;
use std::collections::{BTreeSet, HashMap};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/bingo/completed", get(completed))
        .route("/bingo/stats/{uuid}", get(stats))
        .route("/bingo/{game_id}", get(game))
}

fn int_of(row: &sqlx::mysql::MySqlRow, k: &str) -> i64 {
    js_number(&entity::row_json(row)[k]) as i64
}

async fn completed(State(state): State<AppState>, q: Query) -> ApiResult<Reply> {
    let p = |k: &str| {
        let n = counting::js::parse_int(q.str(k).unwrap_or(""), Some(10));
        n.is_finite().then_some(n as i64)
    };
    let offset = p("offset").map_or(0, |o| o.max(0));
    let limit = p("limit").map_or(25, |l| l.clamp(1, 100));
    let (total,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM bingogame WHERE status = 'finished'")
        .fetch_one(&state.pool)
        .await?;
    let games: Vec<Value> =
        sqlx::query("SELECT * FROM bingogame WHERE status = 'finished' ORDER BY finishedAt DESC LIMIT ? OFFSET ?")
            .bind(limit)
            .bind(offset)
            .fetch_all(&state.pool)
            .await?
            .iter()
            .map(|r| entity::BINGO_GAME.json(r))
            .collect();
    if games.is_empty() {
        return Ok(reply(
            json!({ "games": [], "total": 0, "offset": offset, "limit": limit }),
        ));
    }
    let formatted = format_games(&state, &games).await?;
    Ok(reply(
        json!({ "games": formatted, "total": total, "offset": offset, "limit": limit }),
    ))
}

async fn game(State(state): State<AppState>, Path(game_id): Path<String>) -> ApiResult<Reply> {
    let id = counting::js::parse_int(&game_id, Some(10));
    if !(id.is_finite() && id.fract() == 0.0 && id > 0.0 && counting::js::num_to_string(id) == game_id) {
        return Err(ApiError::bad_request("Invalid game id."));
    }
    let g = sqlx::query("SELECT * FROM bingogame WHERE id = ? LIMIT 1")
        .bind(id as i64)
        .fetch_optional(&state.pool)
        .await?
        .map(|r| entity::BINGO_GAME.json(&r));
    let Some(g) = g else {
        return Err(ApiError::not_found("No such game."));
    };
    let mut f = format_games(&state, std::slice::from_ref(&g)).await?;
    if f.is_empty() {
        return Err(ApiError::not_found("No such game."));
    }
    Ok(reply(f.remove(0)))
}

fn all_lines() -> Vec<[i64; 5]> {
    let mut lines = Vec::new();
    for i in 0..5 {
        lines.push([0, 1, 2, 3, 4].map(|c| i * 5 + c));
        lines.push([0, 1, 2, 3, 4].map(|r| r * 5 + i));
    }
    lines.push([0, 6, 12, 18, 24]);
    lines.push([4, 8, 12, 16, 20]);
    lines
}

fn winning_lines(squares: &[Value], team: i64) -> Vec<Value> {
    let mut owner: HashMap<i64, Value> = HashMap::new();
    for s in squares.iter().filter(|s| s["isUnlocked"] == Value::Bool(true)) {
        owner.insert(s["index"].as_i64().unwrap_or(-1), s["mostRecentUnlockerTeamId"].clone());
    }
    all_lines()
        .into_iter()
        .filter(|l| l.iter().all(|i| owner.get(i).and_then(Value::as_i64) == Some(team)))
        .map(|line| json!({ "teamId": team, "line": line }))
        .collect()
}

fn win_reason(game: &Value, squares: &[Value], quit: &BTreeSet<i64>) -> Value {
    let Some(w) = game["winnerTeamId"].as_i64().filter(|w| *w != 0) else {
        return Value::Null;
    };
    if !winning_lines(squares, w).is_empty() {
        return json!("bingo");
    }
    if quit.iter().any(|t| *t != w) {
        return json!("forfeit");
    }
    if game["mode"].as_str().unwrap_or("").contains("lockout") {
        json!("squares")
    } else {
        json!("forfeit")
    }
}

async fn by_games(state: &AppState, sql: &str, ids: &[Value]) -> ApiResult<Vec<sqlx::mysql::MySqlRow>> {
    db::rows(state, &sql.replace("{ids}", &db::placeholders(ids.len())), ids).await
}

pub async fn format_games(state: &AppState, games: &[Value]) -> ApiResult<Vec<Value>> {
    let ids: Vec<Value> = games.iter().map(|g| g["id"].clone()).collect();
    let squares: Vec<Value> = by_games(
        state,
        "SELECT * FROM bingogamesquare WHERE gameId IN ({ids}) ORDER BY `index` ASC",
        &ids,
    )
    .await?
    .iter()
    .map(|r| entity::BINGO_GAME_SQUARE.json(r))
    .collect();
    let members: Vec<Value> = by_games(
        state,
        "SELECT * FROM bingogamemember WHERE gameId IN ({ids}) AND status = 'active'",
        &ids,
    )
    .await?
    .iter()
    .map(|r| entity::BINGO_GAME_MEMBER.json(r))
    .collect();
    let teams: Vec<Value> = by_games(
        state,
        "SELECT * FROM bingogameteam WHERE gameId IN ({ids}) ORDER BY createdAt ASC, id ASC",
        &ids,
    )
    .await?
    .iter()
    .map(|r| entity::BINGO_GAME_TEAM.json(r))
    .collect();
    let quitters = by_games(
        state,
        "SELECT gameId, teamId FROM bingogamemember WHERE gameId IN ({ids}) AND status IN ('forfeit_vote', 'left')",
        &ids,
    )
    .await?;
    let mut quit: HashMap<i64, BTreeSet<i64>> = HashMap::new();
    for r in &quitters {
        quit.entry(int_of(r, "gameId")).or_default().insert(int_of(r, "teamId"));
    }
    let counts = by_games(
        state,
        "SELECT gameId, COUNT(*) AS players FROM bingogamemember WHERE status = 'active' AND gameId IN ({ids}) GROUP BY gameId",
        &ids,
    )
    .await?;
    let count_of = |id: i64| {
        counts
            .iter()
            .find(|r| int_of(r, "gameId") == id)
            .map_or(0, |r| int_of(r, "players"))
    };
    let mut uuids: Vec<String> = Vec::new();
    for g in games {
        if let Some(h) = g["hostUserUuid"].as_str() {
            uuids.push(h.to_owned());
        }
    }
    for m in &members {
        if let Some(u) = m["userUuid"].as_str() {
            uuids.push(u.to_owned());
        }
    }
    uuids.sort();
    uuids.dedup();
    let counters = db::find_counters(state, &uuids).await?;
    let counter = |u: &Value| counters.iter().find(|c| c["uuid"] == *u);
    let s = |c: Option<&Value>, k: &str| {
        c.and_then(|c| c[k].as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    Ok(games
        .iter()
        .map(|g| {
            let gid = g["id"].as_i64().unwrap_or_default();
            let sq: Vec<Value> = squares.iter().filter(|s| s["gameId"].as_i64() == Some(gid)).cloned().collect();
            let host_c = counter(&g["hostUserUuid"]);
            let host = s(host_c, "username")
                .or_else(|| s(host_c, "name"))
                .or_else(|| s(host_c, "uuid"))
                .map_or(g["hostUserUuid"].clone(), Value::from);
            let winner_team = g["winnerTeamId"].as_i64().filter(|w| *w != 0);
            json!({
                "id": gid,
                "host": host,
                "status": g["status"],
                "mode": g["mode"],
                "rank": g["rank"],
                "winnerTeamId": g["winnerTeamId"],
                "winReason": win_reason(g, &sq, quit.get(&gid).unwrap_or(&BTreeSet::new())),
                "allowJoin": g["allowJoin"],
                "players": count_of(gid),
                "startedAt": g["startedAt"],
                "finishedAt": g["finishedAt"],
                "timeLimitSeconds": g["timeLimitSeconds"],
                "winnerUserUuid": g["winnerUserUuid"],
                "winnerAt": g["winnerAt"],
                "winningLines": winner_team.map_or(json!([]), |w| json!(winning_lines(&sq, w))),
                "members": members.iter().filter(|m| m["gameId"].as_i64() == Some(gid)).map(|m| {
                    let c = counter(&m["userUuid"]);
                    let team = teams.iter().find(|t| t["id"] == m["teamId"]);
                    json!({
                        "userUuid": m["userUuid"],
                        "username": s(c, "username").or_else(|| s(c, "name")).map_or(m["userUuid"].clone(), Value::from),
                        "displayName": s(c, "name").or_else(|| s(c, "username")).map_or(m["userUuid"].clone(), Value::from),
                        "avatar": s(c, "avatar"),
                        "discordId": s(c, "discordId"),
                        "color": s(c, "color"),
                        "teamId": m["teamId"],
                        "teamName": team.and_then(|t| t["name"].as_str()).unwrap_or(""),
                        "role": m["role"],
                    })
                }).collect::<Vec<_>>(),
                "teams": teams.iter().filter(|t| t["gameId"].as_i64() == Some(gid)).map(|t| json!({
                    "id": t["id"],
                    "name": t["name"],
                    "key": t["name"].as_str().unwrap_or("").to_lowercase(),
                })).collect::<Vec<_>>(),
                "squares": sq.iter().map(|q| {
                    let mut o = json!({
                        "id": q["id"], "index": q["index"], "challengeType": q["challengeType"], "rank": q["rank"],
                        "threadUuid": q["threadUuid"], "threadName": q["threadName"], "threadTitle": q["threadTitle"],
                        "target": q["target"], "params": q["params"],
                        "progressValue": q["progressValue"], "isUnlocked": q["isUnlocked"],
                        "mostRecentUnlockerUserUuid": q["mostRecentUnlockerUserUuid"],
                        "mostRecentUnlockerTeamId": q["mostRecentUnlockerTeamId"],
                        "mostRecentUnlockAt": q["mostRecentUnlockAt"],
                    });
                    if let Some(v) = q["params"].get("_scoreToBeat") {
                        o["scoreToBeat"] = v.clone();
                    }
                    o
                }).collect::<Vec<_>>(),
            })
        })
        .collect())
}

async fn stats(State(state): State<AppState>, Path(uuid): Path<String>) -> ApiResult<Reply> {
    if !(uuid.len() == 36 && uuid.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')) {
        return Err(ApiError::bad_request("Invalid counter uuid."));
    }
    let mine = sqlx::query(
        "SELECT m.gameId, m.teamId, g.rank, g.startedAt, g.lineAt, g.winnerTeamId FROM bingogamemember m JOIN bingogame g ON g.id = m.gameId \
         WHERE m.userUuid = ? AND g.status = 'finished' AND g.startedAt > 0 AND (m.leftAt IS NULL OR m.leftAt = 0 OR m.leftAt > g.startedAt)",
    )
    .bind(&uuid)
    .fetch_all(&state.pool)
    .await?;
    let mine: Vec<Value> = mine.iter().map(entity::row_json).collect();
    let gids: Vec<Value> = mine.iter().map(|r| json!(js_number(&r["gameId"]) as i64)).collect();
    let everyone: Vec<Value> = if gids.is_empty() {
        Vec::new()
    } else {
        by_games(
            &state,
            "SELECT m.gameId, m.teamId, m.userUuid FROM bingogamemember m JOIN bingogame g ON g.id = m.gameId \
             WHERE m.gameId IN ({ids}) AND (m.leftAt IS NULL OR m.leftAt = 0 OR m.leftAt > g.startedAt)",
            &gids,
        )
        .await?
        .iter()
        .map(entity::row_json)
        .collect()
    };
    let mut claims: HashMap<String, i64> = HashMap::new();
    for r in sqlx::query("SELECT kind, COUNT(*) n FROM bingoclaim WHERE userUuid = ? GROUP BY kind")
        .bind(&uuid)
        .fetch_all(&state.pool)
        .await?
    {
        claims.insert(
            r.try_get::<String, _>("kind").unwrap_or_default(),
            r.try_get::<i64, _>("n").unwrap_or(0),
        );
    }
    let mut others: BTreeSet<String> = BTreeSet::new();
    let (mut won, mut lines) = (0, 0);
    let mut best: Vec<(String, Value)> = Vec::new();
    for r in &mine {
        let gid = js_number(&r["gameId"]) as i64;
        let team = js_number(&r["teamId"]) as i64;
        let players: Vec<&Value> = everyone
            .iter()
            .filter(|p| js_number(&p["gameId"]) as i64 == gid)
            .collect();
        for p in &players {
            if let Some(u) = p["userUuid"].as_str()
                && u != uuid
            {
                others.insert(u.to_owned());
            }
        }
        if r["winnerTeamId"].is_null() || js_number(&r["winnerTeamId"]) as i64 != team {
            continue;
        }
        won += 1;
        let line_at = js_number(&r["lineAt"]);
        let started = js_number(&r["startedAt"]);
        if !(line_at > 0.0 && line_at >= started) {
            continue;
        }
        lines += 1;
        let mut sizes: Vec<(i64, i64)> = Vec::new();
        for p in &players {
            let t = js_number(&p["teamId"]) as i64;
            match sizes.iter_mut().find(|x| x.0 == t) {
                Some(s) => s.1 += 1,
                None => sizes.push((t, 1)),
            }
        }
        let mine_size = sizes.iter().find(|x| x.0 == team).map_or(1, |x| x.1);
        let mut rest: Vec<i64> = sizes.iter().filter(|x| x.0 != team).map(|x| x.1).collect();
        rest.sort_by(|a, b| b.cmp(a));
        let category = if !rest.is_empty() {
            "versus"
        } else if mine_size > 1 {
            "coop"
        } else {
            "solo"
        };
        let ms = line_at - started;
        let rank = r["rank"].as_str().unwrap_or("").to_owned();
        let key = format!("{rank}:{category}");
        let format: Vec<String> = std::iter::once(mine_size).chain(rest).map(|n| n.to_string()).collect();
        let entry = json!({
            "rank": rank, "category": category, "ms": entity::js_number(ms), "gameId": gid,
            "format": format.join("v"), "at": entity::js_number(line_at),
        });
        match best.iter_mut().find(|x| x.0 == key) {
            Some(slot) if ms < slot.1["ms"].as_f64().unwrap_or(f64::INFINITY) => slot.1 = entry,
            Some(_) => {}
            None => best.push((key, entry)),
        }
    }
    let order = ["solo", "coop", "versus"];
    let mut records: Vec<Value> = best.into_iter().map(|x| x.1).collect();
    records.sort_by_key(|r| {
        (
            rank_index(r["rank"].as_str().unwrap_or("")),
            order.iter().position(|o| r["category"] == *o).unwrap_or(9),
        )
    });
    Ok(reply(json!({
        "played": mine.len(),
        "won": won,
        "lines": lines,
        "claims": claims.get("claim").copied().unwrap_or(0),
        "steals": claims.get("steal").copied().unwrap_or(0),
        "saves": claims.get("save").copied().unwrap_or(0),
        "playedWith": others.len(),
        "records": records,
    })))
}
