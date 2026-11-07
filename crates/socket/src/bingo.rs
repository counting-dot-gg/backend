use crate::app::{App, now_ms};
use crate::gateway::user_of;
use crate::model::Counter;
use counting::seedrandom::SeedRandom;
use dashmap::DashMap;
use serde_json::{Map, Value, json};
use socketioxide::extract::{SocketRef, TryData};
use sqlx::Row;
use sqlx::mysql::{MySqlPool, MySqlRow};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, LazyLock, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

const BOARD: usize = 25;
const MAX_TEAMS: usize = 3;
const DEFAULT_GRACE_SECONDS: i64 = 300;
const UPDATE_MIN_INTERVAL: Duration = Duration::from_millis(500);
const TEAM_IDS: [(&str, &str); 3] = [("wave", "Wave"), ("blaze", "Blaze"), ("radiant", "Radiant")];
const RANKS: [&str; 9] = [
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
const NOT_BINGO_THREADS: [&str; 2] = ["test", "test2"];

fn bingo_thread(name: &str) -> bool {
    !NOT_BINGO_THREADS.contains(&name)
}

const GIMMICK_TYPES: [&str; 5] = [
    "roll",
    "lrwoed_score",
    "number_shuffle_win",
    "bars_within_ms",
    "count_attempts",
];
const CHAT_MAX_HISTORY: isize = 60;
const CHAT_TTL: i64 = 60 * 60 * 25;
const CHAT_COOLDOWN_MS: i64 = 1000;
const CHAT_MAX_LEN: usize = 400;

type R<T> = Result<T, String>;

fn err<T>(m: &str) -> R<T> {
    Err(m.to_owned())
}

fn db<E: std::fmt::Display>(e: E) -> String {
    tracing::warn!("bingo db: {e}");
    "Unknown error".into()
}

fn rand36(n: usize) -> String {
    const A: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    (0..n).map(|_| A[fastrand::usize(..36)] as char).collect()
}

#[derive(Clone, Debug)]
struct Game {
    id: i64,
    host: String,
    status: String,
    mode: String,
    rank: String,
    season_id: i64,
    allow_join: bool,
    join_mode: String,
    team_swap_mode: String,
    join_code: Option<String>,
    time_limit_seconds: i64,
    created_at: i64,
    started_at: i64,
    ends_at: i64,
    grace_ends_at: i64,
    finished_at: i64,
    winner_user: Option<String>,
    winner_team: Option<i64>,
    winner_at: i64,
    winner_team_size: i64,
    board_seed: Option<String>,
    min_time_seconds: Option<i64>,
    line_at: i64,
}

#[derive(Clone, Debug)]
struct Team {
    id: i64,
    name: String,
    locked: bool,
    created_at: i64,
}

#[derive(Clone, Debug)]
struct Member {
    id: i64,
    team_id: i64,
    user: String,
    role: String,
    status: String,
    joined_at: i64,
    left_at: i64,
}

#[derive(Clone, Debug)]
struct Square {
    id: i64,
    index: i64,
    kind: String,
    rank: String,
    thread_uuid: Option<String>,
    target: i64,
    params: Option<Map<String, Value>>,
    progress_value: i64,
    unlocked: bool,
    unlocker_user: Option<String>,
    unlocker_team: Option<i64>,
    unlock_at: i64,
    last_progress_at: i64,
    thread_name: Option<String>,
    thread_title: Option<String>,
}

fn i(r: &MySqlRow, c: &str) -> i64 {
    r.try_get::<i64, _>(c)
        .or_else(|_| r.try_get::<i32, _>(c).map(i64::from))
        .unwrap_or(0)
}
fn oi(r: &MySqlRow, c: &str) -> Option<i64> {
    r.try_get::<Option<i64>, _>(c)
        .ok()
        .flatten()
        .or_else(|| r.try_get::<Option<i32>, _>(c).ok().flatten().map(i64::from))
}
fn b(r: &MySqlRow, c: &str) -> bool {
    r.try_get::<i8, _>(c).unwrap_or(0) != 0
}

const GAME_COLS: &str = "id, hostUserUuid, status, mode, `rank`, seasonId, allowJoin, joinMode, teamSwapMode, joinCode, \
    timeLimitSeconds, createdAt, startedAt, endsAt, graceEndsAt, finishedAt, winnerUserUuid, winnerTeamId, winnerAt, \
    winnerTeamSize, boardSeed, minTimeSeconds, lineAt";

fn game_from(r: &MySqlRow) -> Game {
    Game {
        id: i(r, "id"),
        host: r.get("hostUserUuid"),
        status: r.get("status"),
        mode: r.get("mode"),
        rank: r.get("rank"),
        season_id: i(r, "seasonId"),
        allow_join: b(r, "allowJoin"),
        join_mode: r.get("joinMode"),
        team_swap_mode: r.get("teamSwapMode"),
        join_code: r.get("joinCode"),
        time_limit_seconds: i(r, "timeLimitSeconds"),
        created_at: i(r, "createdAt"),
        started_at: i(r, "startedAt"),
        ends_at: i(r, "endsAt"),
        grace_ends_at: i(r, "graceEndsAt"),
        finished_at: i(r, "finishedAt"),
        winner_user: r.get("winnerUserUuid"),
        winner_team: oi(r, "winnerTeamId"),
        winner_at: i(r, "winnerAt"),
        winner_team_size: i(r, "winnerTeamSize"),
        board_seed: r.get("boardSeed"),
        min_time_seconds: oi(r, "minTimeSeconds"),
        line_at: i(r, "lineAt"),
    }
}

fn team_from(r: &MySqlRow) -> Team {
    Team {
        id: i(r, "id"),
        name: r.get("name"),
        locked: b(r, "locked"),
        created_at: i(r, "createdAt"),
    }
}

fn member_from(r: &MySqlRow) -> Member {
    Member {
        id: i(r, "id"),
        team_id: i(r, "teamId"),
        user: r.get("userUuid"),
        role: r.get("role"),
        status: r.get("status"),
        joined_at: i(r, "joinedAt"),
        left_at: i(r, "leftAt"),
    }
}

const SQUARE_COLS: &str = "id, gameId, `index`, challengeType, `rank`, threadUuid, rankChallengeId, target, \
    CAST(params AS CHAR) AS params, progressValue, isUnlocked, mostRecentUnlockerUserUuid, mostRecentUnlockerTeamId, \
    mostRecentUnlockAt, lastProgressAt, createdAt, threadName, threadTitle";

fn square_from(r: &MySqlRow) -> Square {
    Square {
        id: i(r, "id"),
        index: i(r, "index"),
        kind: r.get("challengeType"),
        rank: r.get("rank"),
        thread_uuid: r.get("threadUuid"),
        target: i(r, "target"),
        params: r
            .get::<Option<String>, _>("params")
            .and_then(|p| serde_json::from_str::<Value>(&p).ok())
            .and_then(|v| v.as_object().cloned()),
        progress_value: i(r, "progressValue"),
        unlocked: b(r, "isUnlocked"),
        unlocker_user: r.get("mostRecentUnlockerUserUuid"),
        unlocker_team: oi(r, "mostRecentUnlockerTeamId"),
        unlock_at: i(r, "mostRecentUnlockAt"),
        last_progress_at: i(r, "lastProgressAt"),
        thread_name: r.get("threadName"),
        thread_title: r.get("threadTitle"),
    }
}

impl Square {
    fn param_f(&self, k: &str) -> Option<f64> {
        self.params.as_ref().and_then(|p| p.get(k)).and_then(Value::as_f64)
    }
    fn param_s(&self, k: &str) -> Option<String> {
        self.params
            .as_ref()
            .and_then(|p| p.get(k))
            .and_then(Value::as_str)
            .map(str::to_owned)
    }
    fn params_value(&self) -> Value {
        self.params.clone().map_or(Value::Null, Value::Object)
    }
    fn json(&self) -> Value {
        let score_to_beat = self.params.as_ref().and_then(|p| p.get("_scoreToBeat")).cloned();
        let mut v = json!({
            "id": self.id, "index": self.index, "challengeType": self.kind, "rank": self.rank,
            "threadUuid": self.thread_uuid, "threadName": self.thread_name, "threadTitle": self.thread_title,
            "target": self.target, "params": self.params_value(),
        });
        if let Some(s) = score_to_beat {
            v["scoreToBeat"] = s;
        }
        for (k, val) in [
            ("progressValue", json!(self.progress_value)),
            ("isUnlocked", json!(self.unlocked)),
            ("mostRecentUnlockerUserUuid", json!(self.unlocker_user)),
            ("mostRecentUnlockerTeamId", json!(self.unlocker_team)),
            ("mostRecentUnlockAt", json!(self.unlock_at)),
        ] {
            v[k] = val;
        }
        v
    }
}

async fn game_db(app: &App, id: i64) -> R<Option<Game>> {
    let r = sqlx::query(&format!("SELECT {GAME_COLS} FROM bingogame WHERE id = ?"))
        .bind(id)
        .fetch_optional(&app.pool)
        .await
        .map_err(db)?;
    Ok(r.as_ref().map(game_from))
}

async fn save_game_db(app: &App, g: &Game) -> R<()> {
    sqlx::query(
        "UPDATE bingogame SET status = ?, mode = ?, `rank` = ?, allowJoin = ?, joinMode = ?, teamSwapMode = ?, joinCode = ?, \
         timeLimitSeconds = ?, startedAt = ?, endsAt = ?, graceEndsAt = ?, finishedAt = ?, winnerUserUuid = ?, \
         winnerTeamId = ?, winnerAt = ?, winnerTeamSize = ?, boardSeed = ?, minTimeSeconds = ?, lineAt = ? WHERE id = ?",
    )
    .bind(&g.status)
    .bind(&g.mode)
    .bind(&g.rank)
    .bind(g.allow_join)
    .bind(&g.join_mode)
    .bind(&g.team_swap_mode)
    .bind(&g.join_code)
    .bind(g.time_limit_seconds)
    .bind(g.started_at)
    .bind(g.ends_at)
    .bind(g.grace_ends_at)
    .bind(g.finished_at)
    .bind(&g.winner_user)
    .bind(g.winner_team)
    .bind(g.winner_at)
    .bind(g.winner_team_size)
    .bind(&g.board_seed)
    .bind(g.min_time_seconds)
    .bind(g.line_at)
    .bind(g.id)
    .execute(&app.pool)
    .await
    .map_err(db)?;
    Ok(())
}

async fn teams(app: &App, game: i64) -> R<Vec<Team>> {
    let rows = sqlx::query(
        "SELECT id, name, locked, createdAt FROM bingogameteam WHERE gameId = ? ORDER BY createdAt ASC, id ASC",
    )
    .bind(game)
    .fetch_all(&app.pool)
    .await
    .map_err(db)?;
    Ok(rows.iter().map(team_from).collect())
}

const MEMBER_COLS: &str = "id, gameId, teamId, userUuid, role, status, joinedAt, leftAt";

async fn members_db(app: &App, game: i64, active_only: bool) -> R<Vec<Member>> {
    let sql = if active_only {
        format!("SELECT {MEMBER_COLS} FROM bingogamemember WHERE gameId = ? AND status = 'active' ORDER BY id")
    } else {
        format!("SELECT {MEMBER_COLS} FROM bingogamemember WHERE gameId = ? ORDER BY id")
    };
    let rows = sqlx::query(&sql).bind(game).fetch_all(&app.pool).await.map_err(db)?;
    Ok(rows.iter().map(member_from).collect())
}

async fn member_db(app: &App, game: i64, user: &str) -> R<Option<Member>> {
    let r = sqlx::query(&format!(
        "SELECT {MEMBER_COLS} FROM bingogamemember WHERE gameId = ? AND userUuid = ? ORDER BY id LIMIT 1"
    ))
    .bind(game)
    .bind(user)
    .fetch_optional(&app.pool)
    .await
    .map_err(db)?;
    Ok(r.as_ref().map(member_from))
}

async fn save_member_db(app: &App, m: &Member) -> R<()> {
    sqlx::query("UPDATE bingogamemember SET teamId = ?, role = ?, status = ?, leftAt = ? WHERE id = ?")
        .bind(m.team_id)
        .bind(&m.role)
        .bind(&m.status)
        .bind(m.left_at)
        .bind(m.id)
        .execute(&app.pool)
        .await
        .map_err(db)?;
    Ok(())
}

async fn squares_db(app: &App, game: i64) -> R<Vec<Square>> {
    let rows = sqlx::query(&format!(
        "SELECT {SQUARE_COLS} FROM bingogamesquare WHERE gameId = ? ORDER BY `index` ASC"
    ))
    .bind(game)
    .fetch_all(&app.pool)
    .await
    .map_err(db)?;
    Ok(rows.iter().map(square_from).collect())
}

async fn save_square_db(app: &App, s: &Square) -> R<()> {
    sqlx::query(
        "UPDATE bingogamesquare SET params = ?, progressValue = ?, isUnlocked = ?, mostRecentUnlockerUserUuid = ?, \
         mostRecentUnlockerTeamId = ?, mostRecentUnlockAt = ?, lastProgressAt = ? WHERE id = ?",
    )
    .bind(s.params.as_ref().map(|p| Value::Object(p.clone()).to_string()))
    .bind(s.progress_value)
    .bind(s.unlocked)
    .bind(&s.unlocker_user)
    .bind(s.unlocker_team)
    .bind(s.unlock_at)
    .bind(s.last_progress_at)
    .bind(s.id)
    .execute(&app.pool)
    .await
    .map_err(db)?;
    Ok(())
}

struct Mem {
    game: Game,
    members: Vec<Member>,
    squares: Vec<Square>,
    progress: HashMap<(i64, String), i64>,
    applied_seq: u64,
    current_seq: Option<u64>,
    last_ack: Option<tokio::sync::oneshot::Receiver<()>>,
}

static MEM: LazyLock<DashMap<i64, Arc<parking_lot::Mutex<Mem>>>> = LazyLock::new(DashMap::new);
static LOCKS: LazyLock<DashMap<i64, Arc<tokio::sync::Mutex<()>>>> = LazyLock::new(DashMap::new);

async fn game_lock<T>(_app: &App, _scope: &str, key: &str, f: impl Future<Output = T>) -> T {
    let id: i64 = key.parse().unwrap_or(0);
    let m = LOCKS
        .entry(id)
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone();
    let _g = m.lock().await;
    f.await
}

fn mem(id: i64) -> Option<Arc<parking_lot::Mutex<Mem>>> {
    MEM.get(&id).map(|e| e.clone())
}

fn mem_with(f: impl Fn(&Mem) -> bool) -> Option<Arc<parking_lot::Mutex<Mem>>> {
    MEM.iter().find(|e| f(&e.value().lock())).map(|e| e.value().clone())
}

async fn load_mem(app: &App, g: Game) -> R<()> {
    if MEM.contains_key(&g.id) {
        return Ok(());
    }
    let members = members_db(app, g.id, false).await?;
    let squares = squares_db(app, g.id).await?;
    let mut progress = HashMap::new();
    if !squares.is_empty() {
        let marks = vec!["?"; squares.len()].join(",");
        let sql = format!(
            "SELECT squareId, userUuid, progressValue FROM bingosquareuserprogress WHERE squareId IN ({marks})"
        );
        let mut q = sqlx::query_as::<_, (i32, String, i32)>(&sql);
        for s in &squares {
            q = q.bind(s.id);
        }
        for (sq, user, v) in q.fetch_all(&app.pool).await.map_err(db)? {
            progress.insert((sq as i64, user), v as i64);
        }
    }
    let applied_seq = squares
        .iter()
        .filter_map(|s| s.param_f("_appliedSeq"))
        .fold(0.0, f64::max) as u64;
    MEM.entry(g.id).or_insert_with(|| {
        Arc::new(parking_lot::Mutex::new(Mem {
            game: g,
            members,
            squares,
            progress,
            applied_seq,
            current_seq: None,
            last_ack: None,
        }))
    });
    Ok(())
}

fn lane(game: i64) -> u32 {
    u32::MAX - (game as u32 & 0x00ff_ffff)
}

fn wal_write(app: &App, m: &mut Mem, rec: crate::wal::WalRecord) {
    let Some(w) = app.wal.get() else {
        tracing::error!("[bingo] WAL not ready; a row was not written");
        return;
    };
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let action: crate::wal::Action = Box::pin(async move {
        let _ = tx.send(());
    });
    w.append(crate::wal::Append {
        payload: serde_json::to_vec(&rec).unwrap_or_default(),
        action: Some(action),
        lane: lane(m.game.id),
    });
    m.last_ack = Some(rx);
}

fn game_row(g: &Game) -> crate::wal::BingoGameRow {
    crate::wal::BingoGameRow {
        id: g.id,
        status: g.status.clone(),
        mode: g.mode.clone(),
        rank: g.rank.clone(),
        allow_join: g.allow_join,
        join_mode: g.join_mode.clone(),
        team_swap_mode: g.team_swap_mode.clone(),
        join_code: g.join_code.clone(),
        time_limit_seconds: g.time_limit_seconds,
        started_at: g.started_at,
        ends_at: g.ends_at,
        grace_ends_at: g.grace_ends_at,
        finished_at: g.finished_at,
        winner_user: g.winner_user.clone(),
        winner_team: g.winner_team,
        winner_at: g.winner_at,
        winner_team_size: g.winner_team_size,
        board_seed: g.board_seed.clone(),
        min_time_seconds: g.min_time_seconds,
        line_at: g.line_at,
    }
}

const FINISHED_KEEP: Duration = Duration::from_secs(600);

async fn game(app: &App, id: i64) -> R<Option<Game>> {
    if let Some(m) = mem(id) {
        return Ok(Some(m.lock().game.clone()));
    }
    let g = game_db(app, id).await?;
    match g {
        Some(g) if g.status == "running" => {
            load_mem(app, g).await?;
            Ok(mem(id).map(|m| m.lock().game.clone()))
        }
        other => Ok(other),
    }
}

async fn save_game(app: &App, g: &Game) -> R<()> {
    let Some(m) = mem(g.id) else {
        return save_game_db(app, g).await;
    };
    let finished_now = {
        let mut m = m.lock();
        let was = m.game.status.clone();
        m.game = g.clone();
        wal_write(app, &mut m, crate::wal::WalRecord::BingoGame(game_row(g)));
        was != "finished" && g.status == "finished"
    };
    if finished_now {
        let id = g.id;
        tokio::spawn(async move {
            tokio::time::sleep(FINISHED_KEEP).await;
            MEM.remove(&id);
            LOCKS.remove(&id);
        });
    }
    Ok(())
}

async fn members(app: &App, game_id: i64, active_only: bool) -> R<Vec<Member>> {
    match mem(game_id) {
        Some(m) => Ok(m
            .lock()
            .members
            .iter()
            .filter(|x| !active_only || x.status == "active")
            .cloned()
            .collect()),
        None => members_db(app, game_id, active_only).await,
    }
}

async fn member(app: &App, game_id: i64, user: &str) -> R<Option<Member>> {
    match mem(game_id) {
        Some(m) => Ok(m.lock().members.iter().find(|x| x.user == user).cloned()),
        None => member_db(app, game_id, user).await,
    }
}

async fn save_member(app: &App, mb: &Member) -> R<()> {
    let Some(m) = mem_with(|m| m.members.iter().any(|x| x.id == mb.id)) else {
        return save_member_db(app, mb).await;
    };
    let mut m = m.lock();
    if let Some(x) = m.members.iter_mut().find(|x| x.id == mb.id) {
        *x = mb.clone();
    }
    let row = crate::wal::BingoMemberRow {
        id: mb.id,
        team_id: mb.team_id,
        role: mb.role.clone(),
        status: mb.status.clone(),
        left_at: mb.left_at,
    };
    wal_write(app, &mut m, crate::wal::WalRecord::BingoMember(row));
    Ok(())
}

async fn squares(app: &App, game_id: i64) -> R<Vec<Square>> {
    match mem(game_id) {
        Some(m) => Ok(m.lock().squares.clone()),
        None => squares_db(app, game_id).await,
    }
}

async fn save_square(app: &App, sq: &Square) -> R<()> {
    let Some(m) = mem_with(|m| m.squares.iter().any(|x| x.id == sq.id)) else {
        return save_square_db(app, sq).await;
    };
    let mut m = m.lock();
    let mut sq = sq.clone();
    if let Some(seq) = m.current_seq {
        sq.params
            .get_or_insert_with(Map::new)
            .insert("_appliedSeq".into(), json!(seq));
    }
    if let Some(x) = m.squares.iter_mut().find(|x| x.id == sq.id) {
        *x = sq.clone();
    }
    let row = crate::wal::BingoSquareRow {
        id: sq.id,
        params: sq.params.as_ref().map(|p| Value::Object(p.clone()).to_string()),
        progress_value: sq.progress_value,
        unlocked: sq.unlocked,
        unlocker_user: sq.unlocker_user.clone(),
        unlocker_team: sq.unlocker_team,
        unlock_at: sq.unlock_at,
        last_progress_at: sq.last_progress_at,
    };
    wal_write(app, &mut m, crate::wal::WalRecord::BingoSquare(row));
    Ok(())
}

async fn active_game_of(app: &App, user: &str) -> R<Option<(i64, i64)>> {
    let r: Option<(i32, i32)> = sqlx::query_as(
        "SELECT m.gameId, m.teamId FROM bingogamemember m JOIN bingogame g ON g.id = m.gameId \
         WHERE m.userUuid = ? AND m.status = 'active' AND g.status IN ('lobby', 'running') LIMIT 1",
    )
    .bind(user)
    .fetch_optional(&app.pool)
    .await
    .map_err(db)?;
    Ok(r.map(|(g, t)| (g as i64, t as i64)))
}

fn normalize_mode(m: &str) -> String {
    let m = m.to_lowercase();
    if ["bingo", "lockout", "bingo-lockout"].contains(&m.as_str()) {
        m
    } else {
        "bingo".into()
    }
}

fn is_lockout(g: &Game) -> bool {
    matches!(normalize_mode(&g.mode).as_str(), "lockout" | "bingo-lockout")
}

fn identity(name: &str) -> (&'static str, &'static str) {
    let n = name.to_lowercase();
    TEAM_IDS
        .iter()
        .copied()
        .find(|(k, _)| *k == n.trim())
        .unwrap_or(TEAM_IDS[0])
}

fn counter_card(c: Option<&Arc<Counter>>, uuid: &str) -> Value {
    let f = |k: &str| {
        c.and_then(|c| c.fields.get(k).cloned())
            .filter(|v| !v.is_null() && v.as_str() != Some(""))
    };
    let name = f("name").or_else(|| f("username")).unwrap_or(json!(uuid));
    let username = f("username").or_else(|| f("name")).unwrap_or(json!(uuid));
    json!({
        "uuid": uuid,
        "id": c.and_then(|c| c.id).unwrap_or(0),
        "discordId": f("discordId").unwrap_or(json!("")),
        "name": name,
        "username": username,
        "color": f("color").unwrap_or(json!("")),
        "roles": c.map(|c| c.roles.clone()).unwrap_or_default(),
        "rainbow": f("rainbow").unwrap_or(json!(1)),
        "avatar": f("avatar").unwrap_or(json!("")),
        "cardStyle": f("cardStyle").unwrap_or(json!("")),
        "cardBorderStyle": f("cardBorderStyle").unwrap_or(json!("")),
        "pronouns": f("pronouns").unwrap_or(json!(["", "", "", ""])),
        "title": f("title").unwrap_or(json!("")),
        "xp": f("xp").unwrap_or(json!(0)),
        "emoji": f("emoji"),
    })
}

fn lines() -> Vec<[usize; 5]> {
    let mut v = Vec::new();
    for r in 0..5 {
        v.push([r * 5, r * 5 + 1, r * 5 + 2, r * 5 + 3, r * 5 + 4]);
    }
    for c in 0..5 {
        v.push([c, c + 5, c + 10, c + 15, c + 20]);
    }
    v.push([0, 6, 12, 18, 24]);
    v.push([4, 8, 12, 16, 20]);
    v
}

fn winning_lines(sq: &[Square]) -> Vec<(i64, Vec<usize>)> {
    if sq.len() < BOARD {
        return vec![];
    }
    let mut owner: [Option<i64>; BOARD] = [None; BOARD];
    for s in sq {
        if (0..BOARD as i64).contains(&s.index) && s.unlocked {
            owner[s.index as usize] = s.unlocker_team.filter(|t| *t != 0);
        }
    }
    lines()
        .into_iter()
        .filter_map(|l| {
            let first = owner[l[0]]?;
            l.iter().all(|i| owner[*i] == Some(first)).then(|| (first, l.to_vec()))
        })
        .collect()
}

async fn game_state(app: &App, id: i64) -> R<Value> {
    let g = game(app, id).await?.ok_or("Game not found.")?;
    let ms = members(app, id, false).await?;
    let ts = teams(app, id).await?;
    let team_by: HashMap<i64, &Team> = ts.iter().map(|t| (t.id, t)).collect();
    let uuids: Vec<String> = ms.iter().map(|m| m.user.clone()).collect();
    let counters: HashMap<String, Arc<Counter>> = app
        .counters_of(&uuids)
        .await
        .into_iter()
        .map(|c| (c.uuid.clone(), c))
        .collect();
    let sq = squares(app, id).await?;
    let wl = winning_lines(&sq);
    let visible: Vec<&(i64, Vec<usize>)> = match g.winner_team {
        Some(w) => wl.iter().filter(|(t, _)| *t == w).collect(),
        None => wl.iter().collect(),
    };
    let winner_username = g.winner_user.as_ref().map(|u| {
        counters
            .get(u)
            .map(|c| {
                if c.username.is_empty() {
                    c.fields.get("name").and_then(|v| v.as_str()).unwrap_or(u).to_owned()
                } else {
                    c.username.clone()
                }
            })
            .unwrap_or_else(|| u.clone())
    });
    let members_json: Vec<Value> = ms
        .iter()
        .filter(|m| g.status != "lobby" || m.status == "active")
        .map(|m| {
            let team = team_by.get(&m.team_id);
            let (key, name) = identity(team.map_or("", |t| t.name.as_str()));
            json!({
                "counter": counter_card(counters.get(&m.user), &m.user),
                "role": m.role, "status": m.status, "teamId": m.team_id,
                "teamName": team.map_or(name.to_string(), |t| t.name.clone()), "teamKey": key,
                "joinedAt": m.joined_at, "leftAt": m.left_at,
            })
        })
        .collect();
    let teams_json: Vec<Value> = ts
        .iter()
        .map(|t| {
            let (key, name) = identity(&t.name);
            json!({ "id": t.id, "key": key, "name": name, "locked": t.locked })
        })
        .collect();
    Ok(json!({
        "game": {
            "id": g.id, "hostUserUuid": g.host, "status": g.status, "mode": normalize_mode(&g.mode), "rank": g.rank,
            "allowJoin": g.allow_join, "joinMode": if g.join_mode.is_empty() { "closed".to_string() } else { g.join_mode.clone() },
            "joinCode": g.join_code, "timeLimitSeconds": g.time_limit_seconds, "minTimeSeconds": g.min_time_seconds,
            "createdAt": g.created_at, "startedAt": g.started_at, "endsAt": g.ends_at, "graceEndsAt": g.grace_ends_at,
            "finishedAt": g.finished_at, "winnerUserUuid": g.winner_user, "winnerTeamId": g.winner_team,
            "winnerAt": g.winner_at, "winnerTeamSize": g.winner_team_size, "winnerUsername": winner_username,
            "winningLines": visible.iter().map(|(t, l)| json!({ "teamId": t, "line": l })).collect::<Vec<_>>(),
            "winningLine": visible.first().map(|(_, l)| json!(l)).unwrap_or(Value::Null),
        },
        "members": members_json,
        "teams": teams_json,
        "teamSwapMode": if g.team_swap_mode.is_empty() { "locked".to_string() } else { g.team_swap_mode.clone() },
        "squares": sq.iter().map(Square::json).collect::<Vec<_>>(),
    }))
}

struct Throttle {
    last: Instant,
    due: Option<Instant>,
}

static UPDATES: LazyLock<DashMap<i64, Throttle>> = LazyLock::new(DashMap::new);
static UNLOCK_SEQ: LazyLock<DashMap<i64, u64>> = LazyLock::new(DashMap::new);

static LOBBY: LazyLock<parking_lot::Mutex<(Option<Instant>, bool)>> =
    LazyLock::new(|| parking_lot::Mutex::new((None, false)));
const LOBBY_MIN_INTERVAL: Duration = Duration::from_secs(3);

fn lobby_soon(app: &Arc<App>) {
    let wait = {
        let mut l = LOBBY.lock();
        if l.1 {
            return;
        }
        l.1 = true;
        l.0.map_or(Duration::ZERO, |t| LOBBY_MIN_INTERVAL.saturating_sub(t.elapsed()))
    };
    let app = app.clone();
    tokio::spawn(async move {
        tokio::time::sleep(wait).await;
        *LOBBY.lock() = (Some(Instant::now()), false);
        lobby_update(&app).await;
    });
}

pub fn game_update(app: &Arc<App>, id: i64, force: bool) {
    game_update_within(app, id, force, UPDATE_MIN_INTERVAL);
}

fn score_update(app: &Arc<App>, id: i64) {
    game_update_within(app, id, false, SCORE_STATE_INTERVAL);
    lobby_board_soon(app, id);
}
const SCORE_STATE_INTERVAL: Duration = Duration::from_secs(5);

fn game_update_within(app: &Arc<App>, id: i64, force: bool, min: Duration) {
    let now = Instant::now();
    if !force {
        let mut e = UPDATES.entry(id).or_insert(Throttle {
            last: now.checked_sub(min).unwrap_or(now),
            due: None,
        });
        let at = e.last + min;
        if at > now {
            if e.due.is_none_or(|d| at < d) {
                e.due = Some(at);
                let app = app.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(at - now).await;
                    game_update_within(&app, id, true, min);
                });
            }
            return;
        }
    }
    UPDATES.insert(id, Throttle { last: now, due: None });
    let ticket = {
        let mut e = UPDATE_GEN.entry(id).or_insert(0);
        *e += 1;
        *e
    };
    let app = app.clone();
    tokio::spawn(async move {
        let lock = LOCKS
            .entry(id)
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        let _held = lock.lock().await;
        if let Ok(payload) = game_state(&app, id).await {
            let newest = {
                let mut sent = UPDATE_SENT.entry(id).or_insert(0);
                let ok = *sent < ticket;
                if ok {
                    *sent = ticket;
                }
                ok
            };
            if newest {
                app.to(
                    format!("bingo_game_{id}"),
                    "bingo_game_update",
                    &json!({ "gameId": id, "payload": payload }),
                )
                .await;
            }
        }
        if min == UPDATE_MIN_INTERVAL {
            lobby_soon(&app);
        }
    });
}

static UPDATE_GEN: LazyLock<DashMap<i64, u64>> = LazyLock::new(DashMap::new);
static UPDATE_SENT: LazyLock<DashMap<i64, u64>> = LazyLock::new(DashMap::new);

async fn lobby_update(app: &App) {
    LOBBY_CACHE.lock().await.take();
    app.to(LOBBY_ROOM, "bingo_list_update", &[0u8; 0]).await;
}
const LOBBY_ROOM: &str = "bingo_lobby";

static LOBBY_DIRTY: LazyLock<parking_lot::Mutex<(HashSet<i64>, bool)>> =
    LazyLock::new(|| parking_lot::Mutex::new((HashSet::new(), false)));
static LOBBY_SEEN: LazyLock<DashMap<i64, HashMap<i64, (bool, Option<i64>)>>> = LazyLock::new(DashMap::new);
const LOBBY_BOARD_INTERVAL: Duration = Duration::from_secs(1);

fn lobby_board_soon(app: &Arc<App>, id: i64) {
    {
        let mut d = LOBBY_DIRTY.lock();
        d.0.insert(id);
        if d.1 {
            return;
        }
        d.1 = true;
    }
    let app = app.clone();
    tokio::spawn(async move {
        tokio::time::sleep(LOBBY_BOARD_INTERVAL).await;
        let ids: Vec<i64> = {
            let mut d = LOBBY_DIRTY.lock();
            d.1 = false;
            d.0.drain().collect()
        };
        for id in ids {
            let Ok(sq) = squares(&app, id).await else { continue };
            let changed: Vec<Value> = {
                let mut seen = LOBBY_SEEN.entry(id).or_default();
                sq.iter()
                    .filter(|s| {
                        let now = (s.unlocked, s.unlocker_team);
                        seen.insert(s.id, now) != Some(now)
                    })
                    .map(Square::json)
                    .collect()
            };
            if changed.is_empty() {
                continue;
            }
            let lines: Vec<Value> = winning_lines(&sq)
                .iter()
                .map(|(t, l)| json!({ "teamId": t, "line": l }))
                .collect();
            LOBBY_CACHE.lock().await.take();
            app.to(
                LOBBY_ROOM,
                "bingo_lobby_squares",
                &json!({ "gameId": id, "squares": changed, "winningLines": lines }),
            )
            .await;
        }
    });
}

async fn square_delta(app: &App, id: i64, squares: Vec<Value>) {
    if squares.is_empty() {
        return;
    }
    app.to(
        format!("bingo_game_{id}"),
        "bingo_square_delta",
        &json!({ "gameId": id, "squares": squares }),
    )
    .await;
}

async fn square_unlocked(app: &App, id: i64, data: Value) {
    let seq = {
        let mut e = UNLOCK_SEQ.entry(id).or_insert(0);
        *e += 1;
        *e
    };
    let mut v = json!({ "gameId": id, "unlockSeq": seq });
    if let Some(o) = data.as_object() {
        for (k, x) in o {
            v[k] = x.clone();
        }
    }
    app.to(format!("bingo_game_{id}"), "bingo_square_unlocked", &v).await;
}

async fn game_ended(app: &App, id: i64, data: Value) {
    UPDATES.remove(&id);
    UNLOCK_SEQ.remove(&id);
    UPDATE_GEN.remove(&id);
    UPDATE_SENT.remove(&id);
    let mut v = json!({ "gameId": id });
    if let Some(o) = data.as_object() {
        for (k, x) in o {
            v[k] = x.clone();
        }
    }
    app.to(format!("bingo_game_{id}"), "bingo_game_ended", &v).await;
}

#[allow(clippy::too_many_arguments)]
struct Claim {
    game_id: i64,
    square_id: i64,
    square_index: i64,
    rank: String,
    season_id: i64,
    user: Option<String>,
    team: i64,
    prev_team: Option<i64>,
    at: i64,
}

static CLAIMS: OnceLock<mpsc::UnboundedSender<Claim>> = OnceLock::new();

fn record_claim(c: Claim, pool: &MySqlPool) {
    let tx = CLAIMS.get_or_init(|| {
        let (tx, mut rx) = mpsc::unbounded_channel::<Claim>();
        let pool = pool.clone();
        tokio::spawn(async move {
            let mut held: HashMap<i64, HashSet<i64>> = HashMap::new();
            while let Some(c) = rx.recv().await {
                if let std::collections::hash_map::Entry::Vacant(e) = held.entry(c.square_id) {
                    let rows = sqlx::query("SELECT DISTINCT teamId FROM bingoclaim WHERE squareId = ?")
                        .bind(c.square_id)
                        .fetch_all(&pool)
                        .await
                        .unwrap_or_default();
                    e.insert(rows.iter().map(|r| r.get::<i32, _>("teamId") as i64).collect());
                }
                let teams = held.entry(c.square_id).or_default();
                let kind = match c.prev_team {
                    None => "claim",
                    Some(_) if teams.contains(&c.team) => "save",
                    Some(_) => "steal",
                };
                teams.insert(c.team);
                let r = sqlx::query(
                    "INSERT INTO bingoclaim (gameId, squareId, squareIndex, `rank`, seasonId, userUuid, teamId, prevTeamId, kind, `at`)                      VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(c.game_id)
                .bind(c.square_id)
                .bind(c.square_index)
                .bind(&c.rank)
                .bind(c.season_id)
                .bind(&c.user)
                .bind(c.team)
                .bind(c.prev_team)
                .bind(kind)
                .bind(c.at)
                .execute(&pool)
                .await;
                if let Err(e) = r {
                    tracing::warn!("bingoclaim insert failed: {e}");
                }
            }
        });
        tx
    });
    let _ = tx.send(c);
}

async fn apply_score(
    app: &App,
    g: &Game,
    sq: &mut Square,
    team: i64,
    score: f64,
    higher: bool,
    threshold: f64,
    now: i64,
    unlocker: Option<(&str, &str, i64)>,
    batch: Option<&mut Batch>,
) -> (bool, bool) {
    if team == 0 || !score.is_finite() || !threshold.is_finite() {
        return (false, false);
    }
    if is_lockout(g) && sq.unlocked && sq.unlocker_team.is_some_and(|t| t != 0 && t != team) {
        return (false, false);
    }
    let mut params = sq.params.clone().unwrap_or_default();
    let mut scores = params
        .get("_teamScores")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();
    let mut at = params
        .get("_teamScoreAt")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();
    let key = team.to_string();
    let existing = scores.get(&key).and_then(Value::as_f64);
    let improved = match existing {
        None => true,
        Some(e) => {
            if higher {
                score > e
            } else {
                score < e
            }
        }
    };
    if !improved {
        return (false, false);
    }
    scores.insert(key.clone(), json!(score));
    at.insert(key, json!(now));
    let mut best: Option<(i64, f64, f64)> = None;
    for (k, v) in &scores {
        let (Ok(tid), Some(s)) = (k.parse::<i64>(), v.as_f64()) else {
            continue;
        };
        if tid <= 0 {
            continue;
        }
        let t = at.get(k).and_then(Value::as_f64).unwrap_or(9007199254740991.0);
        best = match best {
            None => Some((tid, s, t)),
            Some(bv) => {
                let better = if higher { s > bv.1 } else { s < bv.1 };
                if better || (s == bv.1 && t < bv.2) {
                    Some((tid, s, t))
                } else {
                    Some(bv)
                }
            }
        };
    }
    let Some((best_team, best_score, _)) = best else {
        return (false, false);
    };
    let unlocks = if higher {
        best_score >= threshold
    } else {
        best_score <= threshold
    };
    let prev_owner = sq.unlocker_team.filter(|t| *t != 0);
    let prev_unlocked = sq.unlocked;
    let owner_changed = unlocks && Some(best_team) != prev_owner;
    params.insert("_teamScores".into(), Value::Object(scores));
    params.insert("_teamScoreAt".into(), Value::Object(at));
    params.insert("_scoreMode".into(), json!(if higher { "higher" } else { "lower" }));
    params.insert("_scoreToBeat".into(), json!(best_score));
    sq.params = Some(params);
    sq.progress_value = best_score.round().max(0.0) as i64;
    sq.last_progress_at = now;
    sq.unlocked = unlocks;
    if unlocks && owner_changed {
        let mine = unlocker.filter(|u| u.2 == best_team);
        if unlocker.is_some() {
            record_claim(
                Claim {
                    game_id: g.id,
                    square_id: sq.id,
                    square_index: sq.index,
                    rank: g.rank.clone(),
                    season_id: g.season_id,
                    user: mine.map(|u| u.0.to_owned()),
                    team: best_team,
                    prev_team: prev_owner,
                    at: now,
                },
                &app.pool,
            );
        }
        sq.unlocker_team = Some(best_team);
        sq.unlocker_user = mine.map(|u| u.0.to_owned());
        sq.unlock_at = now;
        let mut d = json!({
            "gameId": g.id, "squareId": sq.id, "index": sq.index, "challengeType": sq.kind,
            "unlockerTeamId": best_team, "unlockedAt": now,
        });
        if let Some(u) = mine {
            d["unlockerUserUuid"] = json!(u.0);
            d["unlockerUsername"] = json!(u.1);
        }
        square_unlocked(app, g.id, d).await;
    }
    let owner_flag = owner_changed || (!prev_unlocked && unlocks);
    match batch {
        Some(bt) => {
            bt.dirty.insert(sq.id);
            if let Some(u) = unlocker {
                bt.progress(sq.id, u.0, higher, score, now);
            }
        }
        None => {
            let _ = save_square(app, sq).await;
            if let Some(u) = unlocker {
                let mut one = Batch::default();
                one.progress(sq.id, u.0, higher, score, now);
                one.persist_progress(app).await;
            }
        }
    }
    (true, owner_flag)
}

#[derive(Default)]
struct Batch {
    dirty: HashSet<i64>,
    entries: HashMap<(i64, String), (i64, Option<i64>, i64)>,
}

impl Batch {
    fn progress(&mut self, square: i64, user: &str, higher: bool, score: f64, at: i64) {
        let e = self.entries.entry((square, user.to_owned())).or_insert((0, None, at));
        if higher {
            e.0 += 1;
        } else {
            e.1 = Some(score.round().max(0.0) as i64);
        }
        e.2 = at;
    }

    async fn persist_progress(&self, app: &App) {
        for ((square, user), (add, set, at)) in &self.entries {
            if let Some(m) = mem_with(|m| m.squares.iter().any(|x| x.id == *square)) {
                let mut m = m.lock();
                let key = (*square, user.clone());
                let v = set.unwrap_or(m.progress.get(&key).copied().unwrap_or(0) + add);
                m.progress.insert(key, v);
                let row = crate::wal::BingoProgressRow {
                    square_id: *square,
                    user: user.clone(),
                    value: v,
                    at: *at,
                };
                wal_write(app, &mut m, crate::wal::WalRecord::BingoProgress(row));
                continue;
            }
            let cur: Option<(i32, i32)> = sqlx::query_as(
                "SELECT id, progressValue FROM bingosquareuserprogress WHERE squareId = ? AND userUuid = ? LIMIT 1",
            )
            .bind(square)
            .bind(user)
            .fetch_optional(&app.pool)
            .await
            .ok()
            .flatten();
            match cur {
                Some((id, pv)) => {
                    let v = set.unwrap_or(pv as i64 + add);
                    let _ = sqlx::query(
                        "UPDATE bingosquareuserprogress SET progressValue = ?, lastProgressAt = ? WHERE id = ?",
                    )
                    .bind(v)
                    .bind(at)
                    .bind(id)
                    .execute(&app.pool)
                    .await;
                }
                None => {
                    let v = set.unwrap_or(*add);
                    let _ = sqlx::query(
                        "INSERT INTO bingosquareuserprogress (squareId, userUuid, progressValue, lastProgressAt) VALUES (?, ?, ?, ?)",
                    )
                    .bind(square)
                    .bind(user)
                    .bind(v)
                    .bind(at)
                    .execute(&app.pool)
                    .await;
                }
            }
        }
    }
}

async fn finalize(
    app: &Arc<App>,
    g: &mut Game,
    sq: &[Square],
    unlocked_now: bool,
    winner: Option<(&str, i64)>,
    now: i64,
) {
    let grace_expired = g.grace_ends_at != 0 && now >= g.grace_ends_at;
    if !unlocked_now && !grace_expired {
        score_update(app, g.id);
        return;
    }
    let mut wl = winning_lines(sq);
    let held = g
        .winner_team
        .filter(|t| g.grace_ends_at != 0 && wl.iter().any(|(w, _)| w == t));
    if let Some(h) = held {
        wl.sort_by_key(|(t, _)| *t != h);
    }
    let win_team = wl.first().map(|(t, _)| *t);
    let team_lines: Vec<Vec<usize>> = win_team.map_or(vec![], |w| {
        wl.iter().filter(|(t, _)| *t == w).map(|(_, l)| l.clone()).collect()
    });
    let active = members(app, g.id, true).await.unwrap_or_default();
    let min_ends = g.min_time_seconds.map_or(0, |s| g.started_at + s * 1000);
    let before_min = min_ends > 0 && now < min_ends;
    if let Some(w) = win_team {
        let team_members: Vec<&Member> = active.iter().filter(|m| m.team_id == w).collect();
        let winner_user = match winner {
            Some((u, t)) if t == w => Some(u.to_owned()),
            _ => team_members.first().map(|m| m.user.clone()),
        };
        g.winner_user = winner_user;
        g.winner_team = Some(w);
        g.winner_at = now;
        g.winner_team_size = team_members.len() as i64;
        if g.grace_ends_at == 0 || held != Some(w) {
            g.line_at = now;
        }
        if before_min && !grace_expired {
            g.grace_ends_at = min_ends;
            let _ = save_game(app, g).await;
            schedule_expiry(app, g);
            game_update(app, g.id, true);
            return;
        }
        g.grace_ends_at = 0;
        g.status = "finished".into();
        cancel_expiry(g.id);
        g.finished_at = now;
        let _ = save_game(app, g).await;
        crate::rank::cancel_bingo_game_instances(app, g.id).await;
        game_update(app, g.id, true);
        game_ended(
            app,
            g.id,
            json!({
                "winnerTeamId": w, "winnerUserUuid": g.winner_user, "winnerTeamSize": g.winner_team_size,
                "winningLine": wl.first().map(|(_, l)| json!(l)).unwrap_or(Value::Null),
                "winningLines": team_lines, "endedAt": now,
            }),
        )
        .await;
        lobby_update(app).await;
        return;
    }
    if g.grace_ends_at != 0 {
        g.grace_ends_at = 0;
        g.winner_user = None;
        g.winner_team = None;
        g.winner_at = 0;
        g.winner_team_size = 0;
        let _ = save_game(app, g).await;
        game_update(app, g.id, true);
        lobby_board_soon(app, g.id);
        return;
    }
    score_update(app, g.id);
}

async fn expire_if_due(app: &Arc<App>, g: &mut Game, now: i64) -> bool {
    if !(g.ends_at != 0 && now > g.ends_at) {
        return false;
    }
    g.status = "finished".into();
    cancel_expiry(g.id);
    g.finished_at = now;
    if g.grace_ends_at != 0 {
        g.winner_team = None;
        g.winner_user = None;
        g.winner_at = 0;
        g.winner_team_size = 0;
    }
    g.grace_ends_at = 0;
    g.allow_join = false;
    if is_lockout(g) && g.winner_team.is_none() {
        let sq = squares(app, g.id).await.unwrap_or_default();
        let mut count: HashMap<i64, usize> = HashMap::new();
        for s in &sq {
            if let Some(t) = s.unlocker_team.filter(|t| s.unlocked && *t != 0) {
                *count.entry(t).or_default() += 1;
            }
        }
        let mut ranked: Vec<(i64, usize)> = count.into_iter().collect();
        ranked.sort_by_key(|r| std::cmp::Reverse(r.1));
        let tie = ranked.len() > 1 && ranked[1].1 == ranked[0].1;
        if let Some(&(team, _)) = ranked.first().filter(|_| !tie) {
            let tm: Vec<Member> = members(app, g.id, true)
                .await
                .unwrap_or_default()
                .into_iter()
                .filter(|m| m.team_id == team)
                .collect();
            if !tm.is_empty() {
                g.winner_team = Some(team);
                g.winner_user = tm.first().map(|m| m.user.clone());
                g.winner_at = now;
                g.winner_team_size = tm.len() as i64;
                let _ = save_game(app, g).await;
                crate::rank::cancel_bingo_game_instances(app, g.id).await;
                game_update(app, g.id, true);
                game_ended(
                    app,
                    g.id,
                    json!({
                        "winnerTeamId": team, "winnerUserUuid": g.winner_user, "winnerTeamSize": g.winner_team_size,
                        "winningLine": null, "winningLines": [], "endedAt": now, "reason": "lockout_tiebreak",
                    }),
                )
                .await;
                lobby_update(app).await;
                return true;
            }
        }
    }
    let _ = save_game(app, g).await;
    crate::rank::cancel_bingo_game_instances(app, g.id).await;
    game_update(app, g.id, true);
    game_ended(
        app,
        g.id,
        json!({
            "winnerTeamId": null, "winnerUserUuid": null, "winnerTeamSize": 0, "winningLine": null,
            "winningLines": [], "endedAt": now, "reason": "timeout",
        }),
    )
    .await;
    lobby_update(app).await;
    true
}

async fn finalize_if_team_out(app: &Arc<App>, game_id: i64, team: i64) {
    let all = members(app, game_id, false).await.unwrap_or_default();
    let tm: Vec<&Member> = all.iter().filter(|m| m.team_id == team).collect();
    if !tm
        .iter()
        .all(|m| matches!(m.status.as_str(), "forfeit_vote" | "left" | "forfeit" | "kicked"))
    {
        return;
    }
    for m in tm {
        if m.status == "forfeit_vote" {
            let mut m = m.clone();
            m.status = "forfeit".into();
            let _ = save_member(app, &m).await;
        }
    }
    let Ok(Some(mut g)) = game(app, game_id).await else {
        return;
    };
    if g.status != "running" {
        return;
    }
    let all = members(app, game_id, false).await.unwrap_or_default();
    if all.is_empty() {
        return;
    }
    let mut team_ids: Vec<i64> = all.iter().map(|m| m.team_id).collect();
    team_ids.sort();
    team_ids.dedup();
    let still_in: Vec<i64> = team_ids
        .iter()
        .copied()
        .filter(|t| {
            all.iter()
                .any(|m| m.team_id == *t && (m.status == "active" || m.status == "forfeit_vote"))
        })
        .collect();
    let finish = if team_ids.len() > 1 {
        still_in.len() <= 1
    } else {
        still_in.is_empty()
    };
    if !finish {
        return;
    }
    let now = now_ms();
    g.status = "finished".into();
    cancel_expiry(g.id);
    g.finished_at = now;
    if let [w] = still_in[..] {
        let winners: Vec<Member> = all
            .iter()
            .filter(|m| m.team_id == w && (m.status == "active" || m.status == "forfeit_vote"))
            .cloned()
            .collect();
        g.winner_team = Some(w);
        g.winner_user = winners.first().map(|m| m.user.clone());
        g.winner_at = now;
        g.winner_team_size = winners.len() as i64;
    } else {
        g.winner_team = None;
        g.winner_user = None;
        g.winner_at = now;
        g.winner_team_size = 0;
    }
    let _ = save_game(app, &g).await;
    crate::rank::cancel_bingo_game_instances(app, g.id).await;
    game_update(app, g.id, true);
    game_ended(
        app,
        g.id,
        json!({
            "winnerTeamId": g.winner_team, "winnerUserUuid": g.winner_user, "winnerTeamSize": g.winner_team_size,
            "winningLine": null, "endedAt": now,
        }),
    )
    .await;
    lobby_update(app).await;
}

static EXPIRY: LazyLock<DashMap<i64, tokio::task::AbortHandle>> = LazyLock::new(DashMap::new);

fn cancel_expiry(id: i64) {
    if let Some((_, h)) = EXPIRY.remove(&id) {
        h.abort();
    }
}

fn schedule_expiry(app: &Arc<App>, g: &Game) {
    cancel_expiry(g.id);
    if g.status != "running" {
        return;
    }
    let Some(due) = [g.ends_at, g.grace_ends_at].into_iter().filter(|t| *t > 0).min() else {
        return;
    };
    let delay = (due - now_ms()).max(0) as u64 + 250;
    let app2 = app.clone();
    let id = g.id;
    let h = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(delay)).await;
        EXPIRY.remove(&id);
        sweep(&app2).await;
    });
    EXPIRY.insert(id, h.abort_handle());
}

static SWEEP_STATUS: LazyLock<parking_lot::Mutex<(Option<i64>, usize)>> =
    LazyLock::new(|| parking_lot::Mutex::new((None, 0)));

pub async fn sweep(app: &Arc<App>) {
    let token = format!("{}-{}-{}", std::process::id(), now_ms(), fastrand::u64(..));
    let mut redis = app.redis.clone();
    let got: Result<Option<String>, _> = redis::cmd("SET")
        .arg("bingo_sweep_lock")
        .arg(&token)
        .arg("PX")
        .arg(25_000)
        .arg("NX")
        .query_async(&mut redis)
        .await;
    if !matches!(got, Ok(Some(_))) {
        return;
    }
    let running: Vec<i64> = sqlx::query_scalar::<_, i32>("SELECT id FROM bingogame WHERE status = 'running'")
        .fetch_all(&app.pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(i64::from)
        .collect();
    let now = now_ms();
    let mut finished = 0;
    for id in &running {
        let done = game_lock(app, "bingo_game", &id.to_string(), async {
            let Ok(Some(mut g)) = game(app, *id).await else {
                return 0;
            };
            if g.status != "running" {
                return 0;
            }
            if expire_if_due(app, &mut g, now).await {
                return 1;
            }
            if g.grace_ends_at > 0 && now >= g.grace_ends_at {
                let sq = squares(app, *id).await.unwrap_or_default();
                finalize(app, &mut g, &sq, false, None, now).await;
                return 1;
            }
            0
        })
        .await;
        finished += done;
    }
    finished += sweep_lobbies(app, now).await;
    let open = crate::bingo_window::now_state(app).await.open;
    if WINDOW_OPEN.swap(open, std::sync::atomic::Ordering::Relaxed) != open {
        lobby_update(app).await;
    }
    *SWEEP_STATUS.lock() = (Some(now), finished);
    let hb =
        json!({ "at": now, "pid": std::process::id(), "running": running.len(), "finished": finished }).to_string();
    let _: Result<(), _> = redis::cmd("SET")
        .arg("bingo_sweep_heartbeat")
        .arg(hb)
        .arg("EX")
        .arg(3600)
        .query_async(&mut redis)
        .await;
    let _: Result<i64, _> = redis::Script::new(
        "if redis.call('get', KEYS[1]) == ARGV[1] then return redis.call('del', KEYS[1]) else return 0 end",
    )
    .key("bingo_sweep_lock")
    .arg(&token)
    .invoke_async(&mut redis)
    .await;
}

fn sweep_status() -> Value {
    let (last, n) = *SWEEP_STATUS.lock();
    let next = now_ms() + 30_000 - (now_ms() % 30_000);
    json!({ "registered": true, "running": true, "nextRun": next, "lastRun": last, "lastSweptCount": n })
}

pub async fn start(app: Arc<App>) {
    LazyLock::force(&BOOT_MS);
    let running = sqlx::query(&format!("SELECT {GAME_COLS} FROM bingogame WHERE status = 'running'"))
        .fetch_all(&app.pool)
        .await
        .unwrap_or_default();
    for r in &running {
        schedule_expiry(&app, &game_from(r));
    }
    if !running.is_empty() {
        tracing::info!("[bingo] re-armed expiry timers for {} running game(s)", running.len());
    }
    tokio::spawn(async move {
        loop {
            let wait = 30_000 - (now_ms() % 30_000) as u64;
            tokio::time::sleep(Duration::from_millis(wait)).await;
            sweep(&app).await;
        }
    });
}

const GETS_LANDED_MAX_PER_GET: i64 = 1500;

fn gets_landed_target(counts_target: i64, per_get: i64) -> i64 {
    ((counts_target as f64 / 2.0 / per_get.max(1) as f64 / 2.0).round() as i64).max(1)
}

fn gets_landed_template(rank: &str, season: i64, thread: String, target: i64) -> crate::rank::Template {
    crate::rank::Template {
        id: String::new(),
        kind: "gets_landed".into(),
        rank: rank.into(),
        sequence: 0,
        gg_reward: 0,
        reward_type: "gg".into(),
        target,
        params: Some(json!({})),
        thread_uuid: Some(thread),
        bingo_category: None,
        season_id: season,
        base_wait_ms: None,
    }
}

fn shuffle<T>(v: &mut [T], rng: &mut SeedRandom) {
    for i in (1..v.len()).rev() {
        let j = (rng.next_f64() * (i + 1) as f64).floor() as usize;
        v.swap(i, j);
    }
}

async fn generate_board(app: &App, g: &Game, seed: &str) -> R<()> {
    let mut rng = SeedRandom::new(seed);
    let active = members(app, g.id, true).await?;
    let mut size: HashMap<i64, usize> = HashMap::new();
    for m in &active {
        *size.entry(m.team_id).or_default() += 1;
    }
    let any_solo = size.values().any(|n| *n == 1);
    let cats: &[&str] = if any_solo {
        &["Double Counting"]
    } else {
        &["Traditional", "Double Counting"]
    };
    let mut eligible_threads: Vec<(String, i64)> = app
        .threads_by_uuid
        .iter()
        .filter(|h| {
            let m = h.meta.read();
            bingo_thread(&m.name)
                && m.row
                    .get("category")
                    .and_then(|c| c.as_str())
                    .is_some_and(|c| cats.contains(&c))
        })
        .map(|h| {
            let m = h.meta.read();
            let n = |k: &str| m.row.get(k).and_then(Value::as_i64).unwrap_or(0);
            (h.key().clone(), n("countsPerSplit") * n("splitsPerGet"))
        })
        .collect();
    eligible_threads.sort();
    let eligible: HashSet<String> = eligible_threads.iter().map(|t| t.0.clone()).collect();
    let rank = g.rank.as_str();
    let season = g.season_id;
    let of = |kind: &'static str| async move { crate::rank::sequence_zero(app, season, kind, rank).await };
    let thread_scoped = |v: Vec<crate::rank::Template>| -> Vec<crate::rank::Template> {
        v.into_iter()
            .filter(|t| t.thread_uuid.as_ref().is_some_and(|u| eligible.contains(u)))
            .collect()
    };
    let mut shortfalls: Vec<String> = vec![];
    let mut used: HashSet<String> = HashSet::new();
    let pick = |mut c: Vec<crate::rank::Template>, n: usize, used: &mut HashSet<String>, rng: &mut SeedRandom| {
        shuffle(&mut c, rng);
        let mut out = vec![];
        let mut skipped = 0;
        for t in c {
            if out.len() >= n {
                break;
            }
            match &t.thread_uuid {
                Some(u) if !used.contains(u) => {
                    used.insert(u.clone());
                    out.push(t);
                }
                _ => skipped += 1,
            }
        }
        (out, skipped)
    };

    let sitewide: Vec<_> = of("thread_counts")
        .await
        .into_iter()
        .filter(|t| t.thread_uuid.is_none())
        .collect();
    if sitewide.is_empty() {
        shortfalls.push("sitewide thread_counts needs 1, only 0 available (0 already used)".into());
    }
    let center =
        (!sitewide.is_empty()).then(|| sitewide[(rng.next_f64() * sitewide.len() as f64).floor() as usize].clone());
    let mut picked: Vec<crate::rank::Template> = vec![];
    let mut core_c: HashMap<&'static str, Vec<crate::rank::Template>> = HashMap::new();
    for (kind, n, label) in [
        ("thread_counts", 10, "thread thread_counts"),
        ("split_under_ms", 5, "split_under_ms"),
        ("get_under_ms", 4, "get_under_ms"),
    ] {
        let cands = thread_scoped(of(kind).await);
        core_c.insert(kind, cands.clone());
        let avail = cands.len();
        let (got, skipped) = pick(cands, n, &mut used, &mut rng);
        if got.len() < n {
            shortfalls.push(format!(
                "{label} needs {n}, only {avail} available ({skipped} already used)"
            ));
        }
        picked.extend(got);
    }
    let mut counts_target: HashMap<&str, i64> = HashMap::new();
    for t in &core_c["thread_counts"] {
        if let Some(u) = t.thread_uuid.as_deref() {
            let e = counts_target.entry(u).or_insert(t.target);
            *e = (*e).min(t.target);
        }
    }
    let landed_c: Vec<&(String, i64)> = eligible_threads
        .iter()
        .filter(|(u, per_get)| {
            *per_get > 0 && *per_get <= GETS_LANDED_MAX_PER_GET && counts_target.contains_key(u.as_str())
        })
        .collect();
    let landed_free: Vec<&(String, i64)> = landed_c.iter().copied().filter(|(u, _)| !used.contains(u)).collect();
    let gets_landed = if landed_free.is_empty() {
        shortfalls.push(format!(
            "gets_landed needs 1, only {} available ({} already used)",
            landed_c.len(),
            used.len()
        ));
        None
    } else {
        let (u, _) = landed_free[(rng.next_f64() * landed_free.len() as f64).floor() as usize].clone();
        let target = landed_c
            .iter()
            .map(|(t, per_get)| gets_landed_target(counts_target[t.as_str()], *per_get))
            .min()
            .unwrap_or(1);
        used.insert(u.clone());
        Some(gets_landed_template(rank, season, u, target))
    };
    let breadth_c: Vec<_> = of("thread_breadth")
        .await
        .into_iter()
        .filter(|t| t.thread_uuid.is_none())
        .collect();
    let breadth =
        (!breadth_c.is_empty()).then(|| breadth_c[(rng.next_f64() * breadth_c.len() as f64).floor() as usize].clone());
    let acc_c = thread_scoped(of("accuracy_rate").await);
    let acc_avail = acc_c.len();
    let (mut acc, acc_skipped) = pick(acc_c.clone(), 1, &mut used, &mut rng);
    if acc.is_empty() {
        shortfalls.push(format!(
            "accuracy_rate needs 1, only {acc_avail} available ({acc_skipped} already used)"
        ));
    }
    core_c.insert("accuracy_rate", acc_c);
    let filler_n = 2 - usize::from(breadth.is_some());
    let mut filler = 0;
    for _ in 0..filler_n {
        let mut kinds: Vec<&str> = ["accuracy_rate", "thread_counts", "split_under_ms", "get_under_ms"]
            .into_iter()
            .filter(|k| *k != "accuracy_rate" || acc.len() < 2)
            .collect();
        shuffle(&mut kinds, &mut rng);
        for kind in kinds {
            let (got, _) = pick(core_c[kind].clone(), 1, &mut used, &mut rng);
            if let Some(t) = got.into_iter().next() {
                if kind == "accuracy_rate" {
                    acc.push(t);
                } else {
                    picked.push(t);
                }
                filler += 1;
                break;
            }
        }
    }
    if filler < filler_n {
        shortfalls.push(format!(
            "filler needs {filler_n}, only {filler} available ({} already used)",
            used.len()
        ));
    }
    let mut gimmick_types = GIMMICK_TYPES.to_vec();
    shuffle(&mut gimmick_types, &mut rng);
    let mut gimmick = None;
    for kind in gimmick_types {
        let thread_required = matches!(kind, "bars_within_ms" | "count_attempts" | "roll");
        let rows: Vec<_> = of(kind)
            .await
            .into_iter()
            .filter(|t| match &t.thread_uuid {
                Some(u) => thread_required && app.thread_meta(u).is_some_and(|m| bingo_thread(&m.name)),
                None => !thread_required,
            })
            .collect();
        if !rows.is_empty() {
            gimmick = Some(rows[(rng.next_f64() * rows.len() as f64).floor() as usize].clone());
            break;
        }
    }
    if gimmick.is_none() {
        shortfalls.push("gimmick needs 1, only 0 available (0 already used)".into());
    }
    if center.is_none() {
        shortfalls.push("center (sitewide) needs 1, only 0 available".into());
    }
    if !shortfalls.is_empty() {
        return Err(format!(
            "Not enough unique bingo challenges to fill the board: {}",
            shortfalls.join("; ")
        ));
    }
    let mut indices: Vec<i64> = (0..BOARD as i64).filter(|i| *i != 12).collect();
    shuffle(&mut indices, &mut rng);
    let mut order: Vec<(i64, crate::rank::Template)> = vec![(12, center.unwrap())];
    let rest = picked
        .into_iter()
        .chain(gets_landed)
        .chain(acc)
        .chain(breadth)
        .chain(gimmick);
    order.extend(indices.iter().copied().zip(rest));
    let now = now_ms();
    for (index, t) in order {
        let speed = matches!(t.kind.as_str(), "split_under_ms" | "get_under_ms" | "bars_within_ms");
        let meta = t.thread_uuid.as_ref().and_then(|u| app.thread_meta(u));
        let name = meta.as_ref().map(|m| m.name.clone());
        let title = meta.as_ref().map(|m| {
            m.row
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or(&m.name)
                .to_owned()
        });
        sqlx::query(
            "INSERT INTO bingogamesquare (gameId, `index`, challengeType, `rank`, threadUuid, rankChallengeId, target, params, \
             progressValue, isUnlocked, mostRecentUnlockAt, lastProgressAt, createdAt, threadName, threadTitle) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, 0, 0, 0, 0, ?, ?, ?)",
        )
        .bind(g.id)
        .bind(index)
        .bind(&t.kind)
        .bind(&t.rank)
        .bind(&t.thread_uuid)
        .bind((!t.id.is_empty()).then_some(t.id.as_str()))
        .bind(if speed { 1 } else { t.target })
        .bind(t.params.as_ref().map(|p| p.to_string()))
        .bind(now)
        .bind(name)
        .bind(title)
        .execute(&app.pool)
        .await
        .map_err(db)?;
    }
    Ok(())
}

async fn ensure_teams(app: &App, game_id: i64, now: i64) -> R<Vec<Team>> {
    let existing = teams(app, game_id).await?;
    for i in existing.len()..MAX_TEAMS {
        let (key, name) = TEAM_IDS[i % 3];
        sqlx::query("INSERT INTO bingogameteam (gameId, name, isDefault, locked, createdAt) VALUES (?, ?, ?, ?, ?)")
            .bind(game_id)
            .bind(name)
            .bind(i == 0)
            .bind(key == "radiant")
            .bind(now + (i - existing.len()) as i64)
            .execute(&app.pool)
            .await
            .map_err(db)?;
    }
    teams(app, game_id).await
}

async fn create(app: &App, host: &str, allow_join: bool) -> R<Value> {
    if active_game_of(app, host).await?.is_some() {
        return err("User already has an active bingo game.");
    }
    let season = crate::rank::season(app)
        .await
        .ok_or("No active season; start a season before creating a bingo game.")?;
    let now = now_ms();
    let r = sqlx::query(
        "INSERT INTO bingogame (hostUserUuid, status, mode, `rank`, seasonId, allowJoin, joinMode, timeLimitSeconds, createdAt, boardSeed, minTimeSeconds) \
         VALUES (?, 'lobby', 'bingo', 'bronze', ?, ?, ?, 10800, ?, ?, ?)",
    )
    .bind(host)
    .bind(season)
    .bind(allow_join)
    .bind(if allow_join { "free" } else { "closed" })
    .bind(now)
    .bind(rand36(13))
    .bind(None::<i64>)
    .execute(&app.pool)
    .await
    .map_err(db)?;
    let id = r.last_insert_id() as i64;
    let ts = ensure_teams(app, id, now).await?;
    let host_team = ts.first().ok_or("Failed to initialize teams for new bingo game.")?;
    sqlx::query("INSERT INTO bingogamemember (gameId, teamId, userUuid, role, status, joinedAt) VALUES (?, ?, ?, 'host', 'active', ?)")
        .bind(id)
        .bind(host_team.id)
        .bind(host)
        .bind(now)
        .execute(&app.pool)
        .await
        .map_err(db)?;
    crate::rank::assign_for_bingo_game(app, id, &[host.to_owned()]).await;
    Ok(json!({ "gameId": id }))
}

async fn join(app: &Arc<App>, id: i64, user: &str, code: Option<&str>) -> R<Value> {
    if active_game_of(app, user).await?.is_some() {
        return err("User already has an active bingo game.");
    }
    let g = game(app, id).await?.ok_or("Game not found.")?;
    if g.status != "lobby" {
        return err("Game is not joinable.");
    }
    if g.join_mode == "closed" || !g.allow_join {
        return err("Game does not allow joins.");
    }
    if g.join_mode == "code"
        && code.is_none_or(|c| c.to_uppercase() != g.join_code.clone().unwrap_or_default().to_uppercase())
    {
        return err("Invalid join code.");
    }
    if let Some(mut m) = member(app, id, user).await? {
        if m.status == "left" {
            m.status = "active".into();
            m.left_at = 0;
            save_member(app, &m).await?;
            game_update(app, id, false);
            lobby_update(app).await;
            return Ok(json!({ "gameId": id }));
        }
        return err("User already joined this game.");
    }
    let now = now_ms();
    let ts = ensure_teams(app, id, now).await?;
    if ts.is_empty() {
        return err("No teams available for this game.");
    }
    let active = members(app, id, true).await?;
    let candidates: Vec<&Team> = ts.iter().take(MAX_TEAMS).collect();
    let size = |t: &Team| active.iter().filter(|m| m.team_id == t.id).count();
    let mut best: Option<&Team> = None;
    for c in candidates {
        best = Some(match best {
            None => c,
            Some(bt) => {
                if size(c) < size(bt) {
                    c
                } else if size(c) > size(bt) {
                    bt
                } else if c.created_at < bt.created_at {
                    c
                } else {
                    bt
                }
            }
        });
    }
    let team = best.ok_or("Unable to assign a team.")?;
    sqlx::query("INSERT INTO bingogamemember (gameId, teamId, userUuid, role, status, joinedAt) VALUES (?, ?, ?, 'player', 'active', ?)")
        .bind(id)
        .bind(team.id)
        .bind(user)
        .bind(now)
        .execute(&app.pool)
        .await
        .map_err(db)?;
    crate::rank::assign_for_bingo_game(app, id, &[user.to_owned()]).await;
    Ok(json!({ "gameId": id }))
}

async fn host_check(app: &App, id: i64, user: &str, msg: &str) -> R<Member> {
    match member(app, id, user).await? {
        Some(m) if m.role == "host" => Ok(m),
        _ => Err(msg.to_owned()),
    }
}

async fn start_game(app: &Arc<App>, id: i64, user: &str) -> R<Value> {
    let mut g = game(app, id).await?.ok_or("Game not found.")?;
    host_check(app, id, user, "Only host can start the game.").await?;
    if g.status != "lobby" {
        return err("Game already started.");
    }
    let now = now_ms();
    let existing: i64 = sqlx::query_scalar("SELECT CAST(COUNT(*) AS SIGNED) FROM bingogamesquare WHERE gameId = ?")
        .bind(id)
        .fetch_one(&app.pool)
        .await
        .map_err(db)?;
    if existing == 0 {
        let seed = g.board_seed.clone().unwrap_or_else(|| rand36(13));
        g.board_seed = Some(seed.clone());
        generate_board(app, &g, &seed).await?;
    }
    let ms = members(app, id, true).await?;
    if g.min_time_seconds.is_none() {
        let playing: HashSet<i64> = ms.iter().map(|m| m.team_id).collect();
        let grace = if playing.len() >= 2 { DEFAULT_GRACE_SECONDS } else { 0 };
        g.min_time_seconds = Some(if grace < g.time_limit_seconds { grace } else { 0 });
    }
    g.status = "running".into();
    g.started_at = now;
    g.ends_at = now + g.time_limit_seconds * 1000;
    g.grace_ends_at = 0;
    save_game(app, &g).await?;
    schedule_expiry(app, &g);
    Ok(json!({ "gameId": id }))
}

async fn kick(app: &App, id: i64, host: &str, target: &str) -> R<Value> {
    let g = game(app, id).await?.ok_or("Game not found.")?;
    if g.status != "lobby" {
        return err("Cannot kick after game start.");
    }
    host_check(app, id, host, "Only host can kick.").await?;
    let mut t = member(app, id, target).await?.ok_or("Member not found.")?;
    if t.role == "host" {
        return err("Cannot kick host.");
    }
    t.status = "kicked".into();
    t.left_at = now_ms();
    save_member(app, &t).await?;
    Ok(json!({ "gameId": id, "targetUserUuid": target }))
}

async fn leave_lobby(app: &Arc<App>, id: i64, user: &str) -> R<Value> {
    let g = game(app, id).await?.ok_or("Game not found.")?;
    if g.status != "lobby" {
        return err("Can only leave before the game starts.");
    }
    let mut m = member(app, id, user)
        .await?
        .filter(|m| m.status == "active")
        .ok_or("Member not active.")?;
    if m.role == "host" {
        return err("Host cannot leave — cancel the game instead.");
    }
    m.status = "left".into();
    m.left_at = now_ms();
    save_member(app, &m).await?;
    game_update(app, id, false);
    lobby_update(app).await;
    Ok(json!({ "gameId": id }))
}

async fn forfeit(app: &Arc<App>, id: i64, user: &str) -> R<Value> {
    game_lock(app, "bingo_game", &id.to_string(), async {
        let g = game(app, id)
            .await?
            .filter(|g| g.status == "running")
            .ok_or("Game is not running.")?;
        let mut m = member(app, id, user)
            .await?
            .filter(|m| m.status == "active")
            .ok_or("Member not active.")?;
        m.status = "forfeit_vote".into();
        save_member(app, &m).await?;
        finalize_if_team_out(app, g.id, m.team_id).await;
        game_update(app, id, false);
        Ok(json!({ "gameId": id }))
    })
    .await
}

async fn leave_game(app: &Arc<App>, id: i64, user: &str) -> R<Value> {
    game_lock(app, "bingo_game", &id.to_string(), async {
        game(app, id)
            .await?
            .filter(|g| g.status == "running")
            .ok_or("Game is not running.")?;
        let mut m = member(app, id, user)
            .await?
            .filter(|m| m.status == "active" || m.status == "forfeit_vote")
            .ok_or("Member is not in the game.")?;
        m.status = "left".into();
        m.left_at = now_ms();
        save_member(app, &m).await?;
        finalize_if_team_out(app, id, m.team_id).await;
        game_update(app, id, false);
        Ok(json!({ "gameId": id }))
    })
    .await
}

async fn cancel(app: &Arc<App>, id: i64, user: &str) -> R<Value> {
    let mut g = game(app, id).await?.ok_or("Game not found.")?;
    if g.status != "lobby" {
        return err("Only lobby games can be cancelled.");
    }
    host_check(app, id, user, "Only host can cancel the game.").await?;
    close_lobby(app, &mut g, Some(user)).await?;
    Ok(json!({ "gameId": id }))
}

async fn close_lobby(app: &Arc<App>, g: &mut Game, by: Option<&str>) -> R<()> {
    let id = g.id;
    let now = now_ms();
    g.status = "finished".into();
    cancel_expiry(id);
    g.finished_at = now;
    g.grace_ends_at = 0;
    g.allow_join = false;
    save_game(app, g).await?;
    crate::rank::cancel_bingo_game_instances(app, id).await;
    game_update(app, id, true);
    game_ended(
        app,
        id,
        json!({
            "winnerTeamId": null, "winnerUserUuid": null, "winnerTeamSize": 0, "winningLine": null, "winningLines": [],
            "endedAt": now, "reason": "cancelled", "canceledByUserUuid": by,
        }),
    )
    .await;
    lobby_update(app).await;
    Ok(())
}

static WINDOW_OPEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

const LOBBY_IDLE_MS: i64 = 10 * 60_000;
const LOBBY_MAX_MS: i64 = 60 * 60_000;

fn lobby_due(now: i64, created: i64, seen: i64) -> Option<&'static str> {
    if now - created >= LOBBY_MAX_MS {
        Some("open an hour")
    } else if now - seen >= LOBBY_IDLE_MS {
        Some("host away")
    } else {
        None
    }
}

static LOBBY_HOST_SEEN: LazyLock<DashMap<i64, i64>> = LazyLock::new(DashMap::new);

async fn sweep_lobbies(app: &Arc<App>, now: i64) -> usize {
    let lobbies: Vec<(i64, String, i64)> = sqlx::query_as::<_, (i32, String, i64)>(
        "SELECT id, hostUserUuid, CAST(COALESCE(createdAt, 0) AS SIGNED) FROM bingogame WHERE status = 'lobby'",
    )
    .fetch_all(&app.pool)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|(id, h, c)| (i64::from(id), h, c))
    .collect();
    let present: HashSet<(String, String)> = GAME_ROOM
        .iter()
        .filter_map(|e| GAME_ROOM_USER.get(e.key()).map(|u| (e.value().clone(), u.clone())))
        .collect();
    let mut closed = 0;
    for (id, host, created) in lobbies {
        let seen = if present.contains(&(format!("bingo_game_{id}"), host)) {
            LOBBY_HOST_SEEN.insert(id, now);
            now
        } else {
            *LOBBY_HOST_SEEN.entry(id).or_insert(*BOOT_MS)
        };
        let Some(why) = lobby_due(now, created, seen) else {
            continue;
        };
        let done = game_lock(app, "bingo_game", &id.to_string(), async {
            let Ok(Some(mut g)) = game(app, id).await else {
                return false;
            };
            g.status == "lobby" && close_lobby(app, &mut g, None).await.is_ok()
        })
        .await;
        if done {
            LOBBY_HOST_SEEN.remove(&id);
            closed += 1;
            tracing::info!("[bingo] closed lobby {id}: {why}");
        }
    }
    closed
}

async fn update_settings(app: &Arc<App>, id: i64, user: &str, d: &Value) -> R<Value> {
    let mut g = game(app, id).await?.ok_or("Game not found.")?;
    match member(app, id, user).await? {
        Some(m) if m.role == "host" => {}
        _ => return err("Only host can change game settings."),
    }
    if g.status != "lobby" {
        return err("Game already started.");
    }
    if let Some(r) = d.get("rank").filter(|v| !v.is_null()) {
        let r = r.as_str().unwrap_or("").to_lowercase();
        if !RANKS.contains(&r.as_str()) {
            return Err(format!("Unknown rank: {}", d["rank"].as_str().unwrap_or("")));
        }
        g.rank = r;
    }
    if let Some(m) = d.get("mode").filter(|v| !v.is_null()) {
        g.mode = normalize_mode(m.as_str().unwrap_or(""));
    }
    if let Some(t) = d.get("timeLimitSeconds").filter(|v| !v.is_null()) {
        let secs = t.as_f64().map(f64::round).unwrap_or(f64::NAN);
        if !secs.is_finite() || !(60.0..=86400.0).contains(&secs) {
            return err("Time limit must be between 1 minute and 24 hours.");
        }
        g.time_limit_seconds = secs as i64;
        if g.min_time_seconds.is_some_and(|m| m >= g.time_limit_seconds) {
            g.min_time_seconds = Some(0);
        }
    }
    if let Some(t) = d.get("minTimeSeconds") {
        if t.is_null() || t.as_f64() == Some(0.0) {
            g.min_time_seconds = Some(0);
        } else {
            let secs = t.as_f64().map(f64::round).unwrap_or(f64::NAN);
            if !secs.is_finite() || !(60.0..=86400.0).contains(&secs) {
                return err("The grace period must be between 1 minute and 24 hours.");
            }
            if secs as i64 >= g.time_limit_seconds {
                return err("The grace period must be shorter than the time limit.");
            }
            g.min_time_seconds = Some(secs as i64);
        }
    }
    if let Some(t) = d.get("teamSwapMode").filter(|v| !v.is_null()) {
        let t = t.as_str().unwrap_or("");
        if !["locked", "open"].contains(&t) {
            return err("Invalid team swap mode.");
        }
        g.team_swap_mode = t.into();
    }
    if let Some(j) = d.get("joinMode").filter(|v| !v.is_null()) {
        let j = j.as_str().unwrap_or("");
        if !["closed", "free", "code"].contains(&j) {
            return err("Invalid join mode.");
        }
        g.join_mode = j.into();
        g.allow_join = j != "closed";
        if j == "code" && g.join_code.is_none() {
            g.join_code = Some(rand36(6).to_uppercase());
        }
    }
    save_game(app, &g).await?;
    game_update(app, id, true);
    Ok(json!({ "gameId": id }))
}

async fn move_member(app: &Arc<App>, id: i64, actor: &str, target: &str, team_id: i64) -> R<Value> {
    let g = game(app, id).await?.ok_or("Game not found.")?;
    if g.status != "lobby" {
        return err("Cannot change teams after game has started.");
    }
    let a = member(app, id, actor).await?.ok_or("You are not in this game.")?;
    let host = a.role == "host";
    if !host && actor != target {
        return err("Only the host can move other members.");
    }
    let mut t = sqlx::query(&format!(
        "SELECT {MEMBER_COLS} FROM bingogamemember WHERE gameId = ? AND userUuid = ? AND status = 'active' LIMIT 1"
    ))
    .bind(id)
    .bind(target)
    .fetch_optional(&app.pool)
    .await
    .map_err(db)?
    .as_ref()
    .map(member_from)
    .ok_or("Target member not found.")?;
    let ts = teams(app, id).await?;
    let dest = ts.iter().find(|x| x.id == team_id).ok_or("Target team not found.")?;
    if dest.locked && !host {
        return err("That team is locked.");
    }
    if !host && g.team_swap_mode == "locked" {
        return err("Team swapping is disabled by the host.");
    }
    t.team_id = team_id;
    save_member(app, &t).await?;
    game_update(app, id, false);
    Ok(json!({ "gameId": id }))
}

async fn team_lock(app: &Arc<App>, id: i64, actor: &str, team_id: i64, locked: bool) -> R<Value> {
    host_check(app, id, actor, "Only the host can lock/unlock teams.").await?;
    game(app, id)
        .await?
        .filter(|g| g.status == "lobby")
        .ok_or("Game not in lobby.")?;
    let found = teams(app, id).await?.into_iter().any(|t| t.id == team_id);
    if !found {
        return err("Team not found.");
    }
    sqlx::query("UPDATE bingogameteam SET locked = ? WHERE id = ?")
        .bind(locked)
        .bind(team_id)
        .execute(&app.pool)
        .await
        .map_err(db)?;
    game_update(app, id, true);
    Ok(json!({ "gameId": id }))
}

async fn action(app: &Arc<App>, user: &str, action: &str, d: &Value) -> R<Value> {
    let gid = d["gameId"].as_f64().map(|v| v as i64).unwrap_or(0);
    let s = |k: &str| d[k].as_str().unwrap_or("").to_owned();
    if action == "create" && !crate::bingo_window::now_state(app).await.open {
        let admin = app.counter(user).await.is_some_and(|c| c.has_role("admin"));
        if !admin {
            return err("Bingo is closed right now.");
        }
    }
    match action {
        "create" => create(app, user, d["allowJoin"].as_bool() != Some(false)).await,
        "join" => join(app, gid, user, d["joinCode"].as_str()).await,
        "start" => start_game(app, gid, user).await,
        "kick" => kick(app, gid, user, &s("targetUserUuid")).await,
        "forfeit" => forfeit(app, gid, user).await,
        "leave" => leave_lobby(app, gid, user).await,
        "leave_game" => leave_game(app, gid, user).await,
        "cancel" => cancel(app, gid, user).await,
        "update_settings" => update_settings(app, gid, user, d).await,
        "move_member" => {
            move_member(
                app,
                gid,
                user,
                &s("targetUserUuid"),
                d["targetTeamId"].as_f64().unwrap_or(0.0) as i64,
            )
            .await
        }
        "update_team_lock" => {
            team_lock(
                app,
                gid,
                user,
                d["teamId"].as_f64().unwrap_or(0.0) as i64,
                d["locked"].as_bool().unwrap_or(false),
            )
            .await
        }
        _ => err("Unknown action."),
    }
}

static FLAG: LazyLock<parking_lot::Mutex<(bool, Option<Instant>)>> =
    LazyLock::new(|| parking_lot::Mutex::new((false, None)));

async fn bingo_enabled(app: &App) -> bool {
    let stale = FLAG.lock().1.is_none_or(|t| t.elapsed() > Duration::from_secs(30));
    if stale {
        let on: Option<i8> = sqlx::query_scalar("SELECT enabled FROM feature_flag WHERE flagKey = 'bingo' LIMIT 1")
            .fetch_optional(&app.pool)
            .await
            .ok()
            .flatten();
        *FLAG.lock() = (on.unwrap_or(0) != 0, Some(Instant::now()));
    }
    FLAG.lock().0
}

async fn access(app: &App, user: Option<&str>) -> R<()> {
    let user = user.ok_or("Log in to play bingo.")?;
    let c = app.counter(user).await;
    let banned = c.as_ref().is_none_or(|c| c.has_role("banned") || c.has_role("muted"));
    if banned {
        return err("You cannot play bingo right now.");
    }
    if !c.unwrap().has_role("admin") && !bingo_enabled(app).await {
        return err("Bingo is not available right now.");
    }
    Ok(())
}

async fn user_stats(app: &App, user: &str) -> Value {
    static COLS: LazyLock<String> = LazyLock::new(|| {
        GAME_COLS
            .split(',')
            .map(|c| format!("g.{}", c.trim()))
            .collect::<Vec<_>>()
            .join(", ")
    });
    let rows = sqlx::query(&format!(
        "SELECT {}, m.teamId AS memberTeamId FROM bingogamemember m JOIN bingogame g ON g.id = m.gameId WHERE m.userUuid = ?",
        *COLS
    ))
    .bind(user)
    .fetch_all(&app.pool)
    .await
    .unwrap_or_default();
    let team_of: HashMap<i64, i64> = rows.iter().map(|r| (i(r, "id"), i(r, "memberTeamId"))).collect();
    let mut games: Vec<Game> = rows.iter().map(game_from).collect();
    games.sort_by_key(|g| g.id);
    games.dedup_by_key(|g| g.id);
    let completed: Vec<&Game> = games.iter().filter(|g| g.status == "finished").collect();
    let wins: Vec<&&Game> = completed
        .iter()
        .filter(|g| g.winner_team.is_some_and(|w| team_of.get(&g.id) == Some(&w)))
        .collect();
    let best = |v: &[&&Game]| v.iter().map(|g| g.finished_at - g.started_at).filter(|d| *d > 0).min();
    let solo: Vec<&&Game> = wins.iter().copied().filter(|g| g.winner_team_size == 1).collect();
    let group: Vec<&&Game> = wins.iter().copied().filter(|g| g.winner_team_size > 1).collect();
    json!({
        "gamesPlayed": games.len(),
        "completedGames": completed.len(),
        "totalWins": wins.len(),
        "soloWins": solo.len(),
        "groupWins": group.len(),
        "soloPbMs": best(&solo),
        "groupPbMs": best(&group),
    })
}

static LOBBY_CACHE: LazyLock<tokio::sync::Mutex<Option<(Instant, Arc<Value>)>>> =
    LazyLock::new(|| tokio::sync::Mutex::new(None));
const LOBBY_CACHE_TTL: Duration = Duration::from_secs(1);

async fn lobby_games_shared(app: &App) -> Arc<Value> {
    let mut c = LOBBY_CACHE.lock().await;
    if let Some((at, v)) = c.as_ref()
        && at.elapsed() < LOBBY_CACHE_TTL
    {
        return v.clone();
    }
    let v = Arc::new(lobby_games(app).await);
    *c = Some((Instant::now(), v.clone()));
    v
}

async fn lobby_games(app: &App) -> Value {
    let rows = sqlx::query(&format!(
        "SELECT {GAME_COLS} FROM bingogame WHERE status IN ('lobby', 'running') ORDER BY status ASC, createdAt DESC"
    ))
    .fetch_all(&app.pool)
    .await
    .unwrap_or_default();
    let games: Vec<Game> = rows.iter().map(game_from).collect();
    let mut out = vec![];
    for g in &games {
        let ms = members(app, g.id, true).await.unwrap_or_default();
        let ts = teams(app, g.id).await.unwrap_or_default();
        let sq = squares(app, g.id).await.unwrap_or_default();
        let mut uuids: Vec<String> = ms.iter().map(|m| m.user.clone()).collect();
        uuids.push(g.host.clone());
        let counters: HashMap<String, Arc<Counter>> = app
            .counters_of(&uuids)
            .await
            .into_iter()
            .map(|c| (c.uuid.clone(), c))
            .collect();
        let host = counters
            .get(&g.host)
            .map(|c| c.username.clone())
            .filter(|u| !u.is_empty())
            .unwrap_or_else(|| g.host.clone());
        out.push(json!({
            "id": g.id, "host": host, "status": g.status, "mode": normalize_mode(&g.mode), "rank": g.rank,
            "allowJoin": g.allow_join, "joinMode": if g.join_mode.is_empty() { "closed".to_string() } else { g.join_mode.clone() },
            "players": ms.len(), "startedAt": g.started_at, "finishedAt": g.finished_at,
            "teamSwapMode": if g.team_swap_mode.is_empty() { "locked" } else { g.team_swap_mode.as_str() },
            "endsAt": g.ends_at, "graceEndsAt": g.grace_ends_at, "winnerTeamId": g.winner_team,
            "winningLines": winning_lines(&sq)
                .iter()
                .filter(|(t, _)| g.winner_team.is_none_or(|w| w == *t))
                .map(|(t, l)| json!({ "teamId": t, "line": l }))
                .collect::<Vec<_>>(),
            "members": ms.iter().map(|m| {
                let t = ts.iter().find(|t| t.id == m.team_id);
                let (key, name) = identity(t.map_or("", |t| t.name.as_str()));
                json!({
                    "counter": counter_card(counters.get(&m.user), &m.user), "role": m.role, "status": m.status,
                    "teamId": m.team_id, "teamName": t.map_or(name.to_string(), |t| t.name.clone()), "teamKey": key,
                })
            }).collect::<Vec<_>>(),
            "teams": ts.iter().map(|t| { let (k, n) = identity(&t.name); json!({ "id": t.id, "key": k, "name": n, "locked": t.locked }) }).collect::<Vec<_>>(),
            "squares": sq.iter().map(|s| {
                let mut j = s.json();
                if let Some(o) = j.as_object_mut() { o.remove("threadTitle"); o.remove("threadName"); }
                j["threadName"] = json!(s.thread_name);
                j["threadTitle"] = json!(s.thread_title);
                j
            }).collect::<Vec<_>>(),
        }));
    }
    Value::Array(out)
}

async fn lobby(app: &App, user: Option<&str>) -> Value {
    let out = lobby_games_shared(app).await;
    let (stats, mine) = match user {
        Some(u) => (
            user_stats(app, u).await,
            active_game_of(app, u).await.ok().flatten().map(|(g, _)| g),
        ),
        None => (Value::Null, None),
    };
    let window = crate::bingo_window::json(&crate::bingo_window::now_state(app).await);
    json!({ "activeGames": &*out, "stats": stats, "myActiveGameId": mine, "window": window })
}

#[derive(Clone)]
pub struct Event {
    user: String,
    username: String,
    thread_uuid: String,
    validation_type: String,
    valid: bool,
    is_count: bool,
    rolled: bool,
    at: i64,
}

static ACTIVE_CACHE: LazyLock<DashMap<String, (i64, Instant)>> = LazyLock::new(DashMap::new);

async fn active_game_cached(app: &App, user: &str) -> Option<i64> {
    if let Some(e) = ACTIVE_CACHE.get(user)
        && e.1.elapsed() < Duration::from_secs(3)
    {
        return Some(e.0);
    }
    match active_game_of(app, user).await.ok().flatten() {
        Some((g, _)) => {
            ACTIVE_CACHE.insert(user.to_owned(), (g, Instant::now()));
            Some(g)
        }
        None => {
            ACTIVE_CACHE.remove(user);
            None
        }
    }
}

pub async fn run(
    app: Arc<App>,
    committed: Arc<crate::wal::Committed>,
    start: crate::cursor::Position,
    cp: Arc<std::sync::atomic::AtomicU64>,
) {
    let dir = app.cfg.wal_dir.clone();
    let a2 = app.clone();
    let r = crate::cursor::follow(dir, "bingo", committed, start, cp, move |seq, rec| {
        let app = a2.clone();
        async move { handle(&app, seq, rec).await }
    })
    .await;
    if let Err(e) = r {
        tracing::error!("bingo consumer stopped: {e:#}");
        std::process::exit(73);
    }
}

async fn handle(app: &Arc<App>, seq: u64, rec: crate::wal::WalRecord) {
    let crate::wal::WalRecord::Post { post, origin, roll, .. } = rec else {
        return;
    };
    if origin == crate::wal::Origin::System {
        return;
    }
    let (Some(counter), Some(thread)) = (app.counter(&post.author_uuid).await, app.thread_meta(&post.thread)) else {
        return;
    };
    if let Some(done) = on_post(app, seq, &post, &counter, &thread, roll.is_some_and(|r| r.is_finite())).await {
        let _ = done.await;
    }
    if let Some(r) = roll.filter(|r| r.is_finite())
        && let Some(done) = on_roll(app, &counter, r).await
    {
        let _ = done.await;
    }
}

async fn on_post(
    app: &Arc<App>,
    seq: u64,
    post: &crate::model::Post,
    counter: &Counter,
    thread: &crate::model::ThreadMeta,
    rolled: bool,
) -> Option<tokio::sync::oneshot::Receiver<()>> {
    let gid = active_game_cached(app, &counter.uuid).await?;
    let username = if counter.username.is_empty() {
        counter
            .fields
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or(&counter.uuid)
            .to_owned()
    } else {
        counter.username.clone()
    };
    let ev = Event {
        user: counter.uuid.clone(),
        username,
        thread_uuid: thread.uuid.clone(),
        validation_type: thread.validation_type.clone(),
        valid: post.is_valid_count,
        is_count: post.is_count,
        rolled,
        at: counting::js::parse_float(&post.timestamp) as i64,
    };
    game_lock(app, "bingo_game", &gid.to_string(), async {
        game(app, gid).await.ok()??;
        let m = mem(gid)?;
        {
            let mut m = m.lock();
            if seq <= m.applied_seq {
                return None;
            }
            m.current_seq = Some(seq);
            m.last_ack = None;
        }
        apply_batch(app, gid, vec![ev]).await;
        let mut m = m.lock();
        m.current_seq = None;
        m.applied_seq = seq;
        m.last_ack.take()
    })
    .await
}

async fn apply_batch(app: &Arc<App>, gid: i64, events: Vec<Event>) {
    let Ok(Some(mut g)) = game(app, gid).await else { return };
    if g.status != "running" {
        return;
    }
    let live: Vec<Event> = if g.ends_at > 0 {
        events.into_iter().filter(|e| e.at <= g.ends_at).collect()
    } else {
        events
    };
    if live.is_empty() {
        return;
    }
    let ms = members(app, gid, true).await.unwrap_or_default();
    let by_user: HashMap<&str, &Member> = ms.iter().map(|m| (m.user.as_str(), m)).collect();
    let Ok(mut sq) = squares(app, gid).await else { return };
    if sq.is_empty() {
        return;
    }
    let mut batch = Batch::default();
    let mut line_may_change = false;
    let mut winner: Option<(String, i64)> = None;
    for ev in &live {
        let Some(m) = by_user.get(ev.user.as_str()) else {
            continue;
        };
        if score_event(app, ev, m, &g, &mut sq, &mut batch).await {
            line_may_change = true;
            winner = Some((ev.user.clone(), m.team_id));
        }
        if winner.is_none() {
            winner = Some((ev.user.clone(), m.team_id));
        }
    }
    let mut changed = vec![];
    for s in sq.iter().filter(|s| batch.dirty.contains(&s.id)) {
        let _ = save_square(app, s).await;
        changed.push(s.json());
    }
    square_delta(app, gid, changed).await;
    batch.persist_progress(app).await;
    let w = winner.as_ref().map(|(u, t)| (u.as_str(), *t));
    finalize(app, &mut g, &sq, line_may_change, w, now_ms()).await;
}

async fn score_event(app: &App, ev: &Event, m: &Member, g: &Game, sq: &mut [Square], batch: &mut Batch) -> bool {
    let mut line = false;
    let unlocker = (ev.user.as_str(), ev.username.as_str(), m.team_id);
    for s in sq.iter_mut() {
        if s.thread_uuid.as_ref().is_some_and(|t| *t != ev.thread_uuid) {
            continue;
        }
        let kind = s.kind.clone();
        let (is_acc, is_attempts) = (kind == "accuracy_rate", kind == "count_attempts");
        if !is_acc && !is_attempts && !ev.valid {
            continue;
        }
        let team_key = m.team_id.to_string();
        if is_acc {
            let size = s.param_f("windowSize").unwrap_or(20.0) as usize;
            let min = s.param_f("minAccuracyPercent").unwrap_or(100.0);
            if s.param_s("validationType")
                .is_some_and(|v| !v.is_empty() && v != ev.validation_type)
                || !ev.is_count
            {
                continue;
            }
            let p = s.params.get_or_insert_with(Map::new);
            let mut windows = p
                .get("_accuracyWindows")
                .and_then(|v| v.as_object())
                .cloned()
                .unwrap_or_default();
            let prev = windows.get(&team_key).and_then(|v| v.as_str()).map(str::to_owned);
            let updated = crate::rank::window_append(prev.as_deref(), ev.valid, size);
            windows.insert(team_key.clone(), json!(updated));
            p.insert("_accuracyWindows".into(), Value::Object(windows));
            let mut streaks = p
                .get("_accuracyStreaks")
                .and_then(|v| v.as_object())
                .cloned()
                .unwrap_or_default();
            let next = if ev.valid {
                streaks.get(&team_key).and_then(Value::as_i64).unwrap_or(0) + 1
            } else {
                0
            };
            streaks.insert(team_key.clone(), json!(next));
            p.insert("_accuracyStreaks".into(), Value::Object(streaks));
            let pct = crate::rank::window_pct(Some(&updated), size).unwrap_or(0.0);
            let score = if pct >= 100.0 {
                100.0 + (next - size as i64).max(0) as f64
            } else {
                pct
            };
            let (changed, owner) = apply_score(
                app,
                g,
                s,
                m.team_id,
                score,
                true,
                min,
                ev.at,
                Some(unlocker),
                Some(batch),
            )
            .await;
            if !changed {
                batch.dirty.insert(s.id);
            }
            line |= owner;
            continue;
        }
        if kind == "thread_breadth" {
            let p = s.params.get_or_insert_with(Map::new);
            let mut per = p
                .get("_teamThreadCounts")
                .and_then(|v| v.as_object())
                .cloned()
                .unwrap_or_default();
            let mut counts = per
                .get(&team_key)
                .and_then(|v| v.as_object())
                .cloned()
                .unwrap_or_default();
            let n = counts.get(&ev.thread_uuid).and_then(Value::as_i64).unwrap_or(0) + 1;
            counts.insert(ev.thread_uuid.clone(), json!(n));
            let mut ranked: Vec<i64> = counts.values().filter_map(Value::as_i64).collect();
            per.insert(team_key.clone(), Value::Object(counts));
            p.insert("_teamThreadCounts".into(), Value::Object(per));
            let min_threads = (p.get("minThreads").and_then(Value::as_f64).unwrap_or(1.0) as usize).max(1);
            ranked.sort_by(|a, b| b.cmp(a));
            let score = if ranked.len() >= min_threads {
                ranked[min_threads - 1] as f64
            } else {
                0.0
            };
            let target = s.target as f64;
            let (changed, owner) = apply_score(
                app,
                g,
                s,
                m.team_id,
                score,
                true,
                target,
                ev.at,
                Some(unlocker),
                Some(batch),
            )
            .await;
            if !changed {
                batch.dirty.insert(s.id);
            }
            line |= owner;
            continue;
        }
        let roll_thread = matches!(ev.validation_type.as_str(), "1inx" | "incremental_odds");
        if is_attempts && !ev.valid && roll_thread && !ev.rolled {
            continue;
        }
        if (is_attempts && ev.is_count) || kind == "thread_counts" {
            let cur = s
                .params
                .as_ref()
                .and_then(|p| p.get("_teamScores"))
                .and_then(|t| t.get(&team_key))
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            let target = s.target as f64;
            let (_, owner) = apply_score(
                app,
                g,
                s,
                m.team_id,
                cur + 1.0,
                true,
                target,
                ev.at,
                Some(unlocker),
                Some(batch),
            )
            .await;
            line |= owner;
        }
    }
    line
}

async fn single_event<F>(app: &Arc<App>, counter: &Counter, pred: F) -> Option<tokio::sync::oneshot::Receiver<()>>
where
    F: Fn(&Square) -> Option<(f64, bool, f64)>,
{
    let (gid, _) = active_game_of(app, &counter.uuid).await.ok().flatten()?;
    let username = if counter.username.is_empty() {
        counter.uuid.clone()
    } else {
        counter.username.clone()
    };
    game_lock(app, "bingo_game", &gid.to_string(), async {
        let Ok(Some(mut g)) = game(app, gid).await else { return };
        if g.status != "running" {
            return;
        }
        let Ok(Some(m)) = member(app, gid, &counter.uuid).await else {
            return;
        };
        if m.status != "active" {
            return;
        }
        let Ok(mut sq) = squares(app, gid).await else { return };
        let now = now_ms();
        let mut unlocked = false;
        let mut moved: Vec<Value> = vec![];
        for s in sq.iter_mut() {
            if let Some((score, higher, threshold)) = pred(s) {
                let (changed, _) = apply_score(
                    app,
                    &g,
                    s,
                    m.team_id,
                    score,
                    higher,
                    threshold,
                    now,
                    Some((&counter.uuid, &username, m.team_id)),
                    None,
                )
                .await;
                unlocked |= changed;
                if changed {
                    moved.push(s.json());
                }
            }
        }
        square_delta(app, gid, moved).await;
        finalize(app, &mut g, &sq, unlocked, Some((&counter.uuid, m.team_id)), now).await;
    })
    .await;
    mem(gid).and_then(|m| m.lock().last_ack.take())
}

pub async fn on_lrwoed(app: &Arc<App>, counter: &Counter, score: f64) {
    if !score.is_finite() || score <= 0.0 {
        return;
    }
    single_event(app, counter, |s| {
        (s.kind == "lrwoed_score" && score <= s.param_f("maxScore").unwrap_or(f64::INFINITY))
            .then(|| (score, false, s.param_f("maxScore").unwrap_or(8.0)))
    })
    .await;
}

pub async fn on_number_shuffle_win(app: &Arc<App>, counter: &Counter, size: i64, preseeded: bool) {
    if preseeded {
        return;
    }
    single_event(app, counter, |s| {
        (s.kind == "number_shuffle_win" && size as f64 >= s.param_f("minSize").unwrap_or(0.0))
            .then(|| (size as f64, true, s.param_f("minSize").unwrap_or(10.0)))
    })
    .await;
}

async fn on_roll(app: &Arc<App>, counter: &Counter, roll: f64) -> Option<tokio::sync::oneshot::Receiver<()>> {
    if !roll.is_finite() {
        return None;
    }
    single_event(app, counter, |s| {
        if s.kind != "roll" {
            return None;
        }
        let over = s.param_s("direction").as_deref() == Some("over");
        let threshold = s.param_f("threshold");
        let ok = if over {
            roll >= threshold.unwrap_or(f64::INFINITY)
        } else {
            roll <= threshold.unwrap_or(0.0)
        };
        ok.then(|| (roll, over, threshold.unwrap_or(if over { 0.999 } else { 0.001 })))
    })
    .await
}

pub async fn on_split(app: &Arc<App>, thread: &crate::model::ThreadMeta, split: &Value) {
    if !bingo_thread(&thread.name) {
        return;
    }
    let games: Vec<i64> = sqlx::query_scalar::<_, i32>("SELECT id FROM bingogame WHERE status = 'running'")
        .fetch_all(&app.pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(i64::from)
        .collect();
    if games.is_empty() {
        return;
    }
    let number = split["number"].as_str().unwrap_or("");
    let is_get = number.ends_with(" (Get)") || number.ends_with(" (K)");
    let raw = number.trim_end_matches(" (Get)").trim_end_matches(" (K)").to_owned();
    let ms = split["split"].as_f64().unwrap_or(f64::NAN);
    let cps = thread.row.get("countsPerSplit").and_then(Value::as_i64).unwrap_or(0);
    let spg = thread.row.get("splitsPerGet").and_then(Value::as_i64).unwrap_or(0);
    let per_get = cps * spg;
    let wanted: &[&str] = if is_get {
        &["get_under_ms", "gets_landed"]
    } else {
        &["split_under_ms", "bars_within_ms"]
    };
    let marks = vec!["?"; games.len()].join(",");
    let kinds = wanted.iter().map(|k| format!("'{k}'")).collect::<Vec<_>>().join(",");
    let sql = format!(
        "SELECT DISTINCT challengeType FROM bingogamesquare WHERE gameId IN ({marks}) AND threadUuid = ? AND challengeType IN ({kinds})"
    );
    let mut q = sqlx::query_scalar::<_, String>(&sql);
    for g in &games {
        q = q.bind(g);
    }
    let present: Vec<String> = q.bind(&thread.uuid).fetch_all(&app.pool).await.unwrap_or_default();
    if present.is_empty() {
        return;
    }
    let vcn = split["validCountNumber"].as_i64();
    let t_in = now_ms();
    let Some(closing) = closing_post(app, thread, &raw, vcn).await else {
        tracing::warn!(thread = %thread.name, ?vcn, number, "[bingo split] dropped: closing post not in MySQL after 2s");
        return;
    };
    let lag = t_in - counting::js::parse_float(&closing.1) as i64;
    let interval = if is_get { per_get } else { cps };
    let ctx = if interval > 0 && present.iter().any(|k| k != "gets_landed") {
        let c = interval_context(app, thread, &closing, interval).await;
        if c.is_none() {
            tracing::warn!(thread = %thread.name, vcn = closing.2, interval, lag, "[bingo split] no interval context (previous boundary missing, bad timestamps, or nobody qualified)");
        }
        c
    } else {
        None
    };
    for gid in games {
        let (ctx, closing) = (&ctx, &closing);
        game_lock(app, "bingo_game", &gid.to_string(), async move {
            let Ok(Some(mut g)) = game(app, gid).await else { return };
            if g.status != "running" {
                return;
            }
            let author = closing.0.as_str();
            let closer = member(app, gid, author).await.ok().flatten().filter(|m| m.status == "active");
            let winner = match ctx {
                Some(c) => interval_winner(app, gid, c).await,
                None => None,
            };
            if closer.is_none() && winner.is_none() {
                return;
            }
            let Ok(mut sq) = squares(app, gid).await else { return };
            let now = now_ms();
            let mut unlocked = false;
        let mut moved: Vec<Value> = vec![];
            for s in sq.iter_mut() {
                if s.thread_uuid.as_deref() != Some(thread.uuid.as_str()) || !wanted.contains(&s.kind.as_str()) {
                    continue;
                }
                if s.kind == "gets_landed" {
                    let Some(m) = &closer else { continue };
                    let n = closing.2 as f64;
                    if s.param_f("_lastGetVcn").is_some_and(|last| n <= last) {
                        continue;
                    }
                    let mut p = s.params.clone().unwrap_or_default();
                    p.insert("_lastGetVcn".into(), json!(closing.2));
                    s.params = Some(p);
                    let target = s.target.max(1) as f64;
                    let have = s
                        .params
                        .as_ref()
                        .and_then(|p| p.get("_teamScores"))
                        .and_then(|v| v.get(m.team_id.to_string()))
                        .and_then(Value::as_f64)
                        .unwrap_or(0.0);
                    let (changed, _) = apply_score(
                        app, &g, s, m.team_id, (have + 1.0).min(target), true, target, now, Some((author, author, m.team_id)), None,
                    )
                    .await;
                    unlocked |= changed;
                    if changed {
                        moved.push(s.json());
                    }
                    continue;
                }
                let Some(ctx) = ctx else { continue };
                let Some((team, user)) = &winner else {
                    tracing::debug!(game = gid, thread = %thread.name, vcn = closing.2, "[bingo split] no player qualified for the interval");
                    continue;
                };
                if s.param_s("validationType").is_some_and(|v| !v.is_empty() && v != thread.validation_type) {
                    continue;
                }
                if s.kind == "bars_within_ms" && thread.validation_type != "bars" {
                    continue;
                }
                let max = s.param_f("maxMs").unwrap_or(f64::NAN);
                if !max.is_finite() || max <= 0.0 {
                    continue;
                }
                let t = if is_get { ctx.ms } else { ms };
                if t > max {
                    continue;
                }
                let (changed, _) =
                    apply_score(app, &g, s, *team, t, false, max, now, Some((user, user, *team)), None).await;
                tracing::debug!(game = gid, thread = %thread.name, vcn = closing.2, ms = t, team, changed, lag, "[bingo split] scored");
                unlocked |= changed;
                if changed {
                    moved.push(s.json());
                }
            }
            square_delta(app, gid, moved).await;
            let credit = winner.as_ref().map(|(t, u)| (u.as_str(), *t)).or(closer.as_ref().map(|m| (author, m.team_id)));
            finalize(app, &mut g, &sq, unlocked, credit, now).await;
        })
        .await;
    }
}

struct Interval {
    ms: f64,
    qualified: HashSet<String>,
    counts: HashMap<String, (i64, i64)>,
}

async fn interval_winner(app: &App, gid: i64, c: &Interval) -> Option<(i64, String)> {
    pick_interval_winner(&members(app, gid, true).await.ok()?, c)
}

fn pick_interval_winner(active: &[Member], c: &Interval) -> Option<(i64, String)> {
    let mut per: BTreeMap<i64, (i64, i64, String, i64)> = BTreeMap::new();
    for m in active {
        let Some(&(n, last)) = c.counts.get(&m.user) else {
            continue;
        };
        let e = per.entry(m.team_id).or_insert((0, 0, String::new(), 0));
        e.0 += n;
        e.1 = e.1.max(last);
        if c.qualified.contains(&m.user) && (n > e.3 || (n == e.3 && m.user < e.2)) {
            e.2 = m.user.clone();
            e.3 = n;
        }
    }
    per.into_iter()
        .filter(|(_, e)| !e.2.is_empty())
        .max_by_key(|(_, e)| (e.0, e.1))
        .map(|(t, e)| (t, e.2))
}

async fn closing_post(
    app: &App,
    thread: &crate::model::ThreadMeta,
    raw: &str,
    vcn: Option<i64>,
) -> Option<(String, String, i32)> {
    if let Some(n) = vcn {
        crate::persisted::await_count(&app.pool, thread.id, n, Duration::from_secs(2)).await;
    }
    match vcn {
        Some(n) => sqlx::query_as(
            "SELECT authorUUID, timestamp, validCountNumber FROM post WHERE threadId = ? AND validCountNumber = ? AND isValidCount = 1 ORDER BY id DESC LIMIT 1",
        )
        .bind(thread.id)
        .bind(n)
        .fetch_optional(&app.pool)
        .await
        .ok()?,
        None => sqlx::query_as(
            "SELECT authorUUID, timestamp, validCountNumber FROM post WHERE threadId = ? AND rawCount = ? AND isValidCount = 1 ORDER BY id DESC LIMIT 1",
        )
        .bind(thread.id)
        .bind(raw)
        .fetch_optional(&app.pool)
        .await
        .ok()?,
    }
}

async fn interval_context(
    app: &App,
    thread: &crate::model::ThreadMeta,
    closing: &(String, String, i32),
    size: i64,
) -> Option<Interval> {
    let (_, ts, n) = closing.clone();
    let prev_n = n as i64 - size;
    if prev_n < 0 {
        return None;
    }
    let (_, prev_ts, _): (String, String, i32) = sqlx::query_as(
        "SELECT authorUUID, timestamp, validCountNumber FROM post WHERE threadId = ? AND validCountNumber = ? AND isValidCount = 1 ORDER BY id DESC LIMIT 1",
    )
    .bind(thread.id)
    .bind(prev_n)
    .fetch_optional(&app.pool)
    .await
    .ok()??;
    let (cur, prev) = (counting::js::parse_float(&ts), counting::js::parse_float(&prev_ts));
    if !(cur > prev) {
        return None;
    }
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT authorUUID, CAST(COUNT(1) AS SIGNED), CAST(MAX(validCountNumber) AS SIGNED) FROM post WHERE threadId = ? AND isValidCount = 1 \
         AND validCountNumber > ? AND validCountNumber <= ? GROUP BY authorUUID",
    )
    .bind(thread.id)
    .bind(prev_n)
    .bind(n)
    .fetch_all(&app.pool)
    .await
    .ok()?;
    let top = rows.iter().map(|r| r.1).max()? as f64;
    let qualified: HashSet<String> = rows
        .iter()
        .filter(|r| r.1 as f64 >= top * 0.25)
        .map(|r| r.0.clone())
        .collect();
    let counts: HashMap<String, (i64, i64)> = rows.into_iter().map(|(a, n, last)| (a, (n, last))).collect();
    (!qualified.is_empty()).then_some(Interval {
        ms: cur - prev,
        qualified,
        counts,
    })
}

async fn admin_square(app: &Arc<App>, gid: i64, index: i64, team: Option<i64>, score: Option<f64>) -> R<Value> {
    game_lock(app, "bingo_game", &gid.to_string(), async {
        let mut g = game(app, gid).await?.ok_or("Game not found.")?;
        if g.status == "finished" {
            return err("Game already finished.");
        }
        let mut sq = squares(app, gid).await?;
        let pos = sq.iter().position(|s| s.index == index).ok_or("Square not found.")?;
        let now = now_ms();
        {
            let s = &mut sq[pos];
            let mut p = s.params.clone().unwrap_or_default();
            match team {
                None => {
                    for k in [
                        "_teamScores",
                        "_teamScoreAt",
                        "_scoreToBeat",
                        "_accuracyWindows",
                        "_accuracyStreaks",
                        "_teamThreadCounts",
                    ] {
                        p.remove(k);
                    }
                    s.params = Some(p);
                    s.progress_value = 0;
                    s.unlocked = false;
                    s.unlocker_team = None;
                    s.unlocker_user = None;
                    s.unlock_at = 0;
                }
                Some(t) => {
                    if !teams(app, gid).await?.iter().any(|x| x.id == t) {
                        return err("Team is not in this game.");
                    }
                    let (threshold, higher) = match s.kind.as_str() {
                        "split_under_ms" | "get_under_ms" | "bars_within_ms" => {
                            (s.param_f("maxMs").filter(|v| *v != 0.0).unwrap_or(1.0), false)
                        }
                        "lrwoed_score" => (s.param_f("maxScore").filter(|v| *v != 0.0).unwrap_or(1.0), false),
                        "accuracy_rate" => (
                            s.param_f("minAccuracyPercent").filter(|v| *v != 0.0).unwrap_or(100.0),
                            true,
                        ),
                        "number_shuffle_win" => (s.param_f("minSize").filter(|v| *v != 0.0).unwrap_or(1.0), true),
                        "roll" => (
                            s.param_f("threshold").unwrap_or(0.0),
                            s.param_s("direction").as_deref() == Some("over"),
                        ),
                        _ => ((if s.target == 0 { 1 } else { s.target }) as f64, true),
                    };
                    let applied = score.filter(|v| v.is_finite()).unwrap_or(threshold);
                    let unlocks = if higher {
                        applied >= threshold
                    } else {
                        applied <= threshold
                    };
                    let mut scores = p
                        .get("_teamScores")
                        .and_then(|v| v.as_object())
                        .cloned()
                        .unwrap_or_default();
                    let mut at = p
                        .get("_teamScoreAt")
                        .and_then(|v| v.as_object())
                        .cloned()
                        .unwrap_or_default();
                    scores.insert(t.to_string(), json!(applied));
                    at.insert(t.to_string(), json!(now));
                    p.insert("_teamScores".into(), Value::Object(scores));
                    p.insert("_teamScoreAt".into(), Value::Object(at));
                    p.insert("_scoreMode".into(), json!(if higher { "higher" } else { "lower" }));
                    p.insert("_scoreToBeat".into(), json!(applied));
                    s.params = Some(p);
                    s.progress_value = applied.round().max(0.0) as i64;
                    s.unlocked = unlocks;
                    s.unlocker_team = unlocks.then_some(t);
                    s.unlocker_user = None;
                    s.unlock_at = if unlocks { now } else { 0 };
                }
            }
            save_square(app, s).await?;
        }
        finalize(app, &mut g, &sq, true, None, now).await;
        game_update(app, gid, true);
        Ok(json!({ "gameId": gid }))
    })
    .await
}

#[derive(Clone, Copy)]
enum ChatScope {
    Team(i64),
    Game,
}

impl ChatScope {
    fn tag(self) -> String {
        match self {
            ChatScope::Team(t) => t.to_string(),
            ChatScope::Game => "all".into(),
        }
    }
    fn room(self, gid: i64) -> String {
        format!("bingo_chat_{gid}_{}", self.tag())
    }
    fn key(self, gid: i64) -> String {
        format!("bingo_chat:{gid}:{}", self.tag())
    }
    fn team(self) -> Option<i64> {
        match self {
            ChatScope::Team(t) => Some(t),
            ChatScope::Game => None,
        }
    }
}

struct ChatAccess {
    team: i64,
    team_name: String,
    team_size: i64,
    teams_playing: i64,
}

impl ChatAccess {
    fn team_open(&self) -> bool {
        self.team_size >= 2
    }
    fn game_open(&self) -> bool {
        self.teams_playing >= 2
    }
}

async fn chat_access(app: &App, gid: i64, user: &str) -> Option<ChatAccess> {
    let me = member(app, gid, user)
        .await
        .ok()
        .flatten()
        .filter(|m| m.status == "active")?;
    let (team_size, teams_playing): (i64, i64) = sqlx::query_as(
        "SELECT CAST(COALESCE(SUM(teamId = ?), 0) AS SIGNED), CAST(COUNT(DISTINCT teamId) AS SIGNED) \
         FROM bingogamemember WHERE gameId = ? AND status = 'active'",
    )
    .bind(me.team_id)
    .bind(gid)
    .fetch_one(&app.pool)
    .await
    .ok()?;
    let name: Option<String> = sqlx::query_scalar("SELECT name FROM bingogameteam WHERE id = ?")
        .bind(me.team_id)
        .fetch_optional(&app.pool)
        .await
        .ok()
        .flatten();
    Some(ChatAccess {
        team: me.team_id,
        team_name: name.unwrap_or_else(|| "Team".into()),
        team_size,
        teams_playing,
    })
}

async fn chat_history(app: &App, gid: i64, sc: ChatScope) -> Vec<Value> {
    let mut redis = app.redis.clone();
    let raw: Vec<String> = redis::cmd("LRANGE")
        .arg(sc.key(gid))
        .arg(0)
        .arg(-1)
        .query_async(&mut redis)
        .await
        .unwrap_or_default();
    raw.iter().filter_map(|m| serde_json::from_str(m).ok()).collect()
}

static CHAT_ROOM: LazyLock<DashMap<String, Vec<String>>> = LazyLock::new(DashMap::new);
static GAME_ROOM: LazyLock<DashMap<String, String>> = LazyLock::new(DashMap::new);
static GAME_ROOM_USER: LazyLock<DashMap<String, String>> = LazyLock::new(DashMap::new);
static BOOT_MS: LazyLock<i64> = LazyLock::new(now_ms);

fn chat_leave(s: &SocketRef) {
    if let Some((_, prev)) = CHAT_ROOM.remove(&s.id.to_string()) {
        for r in prev {
            s.leave(r);
        }
    }
}

async fn chat_open(app: &App, s: &SocketRef, d: &Value) {
    let gid = d["gameId"].as_f64().unwrap_or(0.0) as i64;
    chat_leave(s);
    let access = match user_of(s).filter(|_| gid != 0) {
        Some(u) => chat_access(app, gid, &u).await,
        None => None,
    };
    let mut state = json!({ "available": false, "gameId": gid, "game": { "available": false } });
    let Some(a) = access else {
        s.emit("bingo_chat_state", &state).ok();
        return;
    };
    let mut rooms = vec![];
    if a.team_open() {
        let sc = ChatScope::Team(a.team);
        s.join(sc.room(gid));
        rooms.push(sc.room(gid));
        state["available"] = json!(true);
        state["teamId"] = json!(a.team);
        state["teamName"] = json!(a.team_name);
        state["teamSize"] = json!(a.team_size);
        state["messages"] = json!(chat_history(app, gid, sc).await);
    }
    if a.game_open() {
        let sc = ChatScope::Game;
        s.join(sc.room(gid));
        rooms.push(sc.room(gid));
        state["game"] = json!({ "available": true, "messages": chat_history(app, gid, sc).await });
    }
    CHAT_ROOM.insert(s.id.to_string(), rooms);
    s.emit("bingo_chat_state", &state).ok();
}

async fn chat_send(app: &App, s: &SocketRef, d: &Value) {
    let Some(user) = user_of(s) else { return };
    let gid = d["gameId"].as_f64().unwrap_or(0.0) as i64;
    let r: R<()> = async {
        let a = chat_access(app, gid, &user).await.ok_or("No chat available.")?;
        let sc = if d["scope"].as_str() == Some("game") {
            if !a.game_open() {
                return err("No game chat available.");
            }
            ChatScope::Game
        } else {
            if !a.team_open() {
                return err("No team chat available.");
            }
            ChatScope::Team(a.team)
        };
        let raw = match &d["text"] {
            Value::Null => String::new(),
            Value::String(t) => t.clone(),
            other => other.to_string(),
        };
        let text: String = counting::js::trim(&raw)
            .encode_utf16()
            .take(CHAT_MAX_LEN)
            .collect::<Vec<u16>>()
            .pipe_str();
        if text.is_empty() {
            return err("Empty message.");
        }
        if app
            .counter(&user)
            .await
            .is_none_or(|c| c.has_role("banned") || c.has_role("muted"))
        {
            return err("Muted.");
        }
        let mut redis = app.redis.clone();
        let ok: Option<String> = redis::cmd("SET")
            .arg(format!("bingo_chat_cd:{user}"))
            .arg("1")
            .arg("PX")
            .arg(CHAT_COOLDOWN_MS)
            .arg("NX")
            .query_async(&mut redis)
            .await
            .map_err(db)?;
        if ok.is_none() {
            return err("Slow down.");
        }
        let msg = chat_message(app, gid, sc, &user, Some((a.team, &a.team_name)), text, None).await;
        app.bot.emit(
            "sendBingoTeamChat",
            json!({ "gameId": gid, "teamId": sc.team(), "teamName": a.team_name, "message": msg }),
        );
        Ok(())
    }
    .await;
    if let Err(e) = r {
        s.emit("bingo_chat_error", &json!({ "error": e })).ok();
    }
}

async fn chat_message(
    app: &App,
    gid: i64,
    sc: ChatScope,
    user: &str,
    author_team: Option<(i64, &str)>,
    text: String,
    discord_id: Option<&str>,
) -> Value {
    let c = app.counter(user).await;
    let f = |k: &str| {
        c.as_ref()
            .and_then(|c| c.fields.get(k))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_owned()
    };
    let author = if !f("username").is_empty() {
        f("username")
    } else if !f("name").is_empty() {
        f("name")
    } else {
        "unknown".into()
    };
    let now = now_ms();
    let mut msg = json!({
        "id": format!("{now}-{}", rand36(7)), "gameId": gid, "teamId": sc.team(),
        "scope": if sc.team().is_some() { "team" } else { "game" }, "authorUuid": user,
        "authorUsername": author, "authorColor": f("color"), "authorAvatar": f("avatar"),
        "authorDiscordId": f("discordId"), "text": text, "timestamp": now,
    });
    if let (ChatScope::Game, Some((t, name))) = (sc, author_team) {
        msg["authorTeamId"] = json!(t);
        msg["authorTeamName"] = json!(name);
    }
    if let Some(id) = discord_id {
        msg["discordMessageId"] = json!(id);
    }
    let key = sc.key(gid);
    let mut redis = app.redis.clone();
    let _: Result<(), _> = redis::pipe()
        .atomic()
        .cmd("RPUSH")
        .arg(&key)
        .arg(msg.to_string())
        .ignore()
        .cmd("LTRIM")
        .arg(&key)
        .arg(-CHAT_MAX_HISTORY)
        .arg(-1)
        .ignore()
        .cmd("EXPIRE")
        .arg(&key)
        .arg(CHAT_TTL)
        .ignore()
        .query_async(&mut redis)
        .await;
    app.to(sc.room(gid), "bingo_chat_message", &msg).await;
    msg
}

pub async fn chat_from_discord(app: &App, d: &Value) {
    let gid = d["gameId"].as_i64().unwrap_or(0);
    let (Some(user), Some(raw)) = (d["counterUuid"].as_str(), d["text"].as_str()) else {
        return;
    };
    let sc = match d["teamId"].as_i64() {
        Some(t) => ChatScope::Team(t),
        None => ChatScope::Game,
    };
    let found: Option<i64> = match sc {
        ChatScope::Team(t) => sqlx::query_scalar("SELECT id FROM bingogameteam WHERE id = ? AND gameId = ?")
            .bind(t)
            .bind(gid),
        ChatScope::Game => sqlx::query_scalar("SELECT id FROM bingogame WHERE id = ?").bind(gid),
    }
    .fetch_optional(&app.pool)
    .await
    .ok()
    .flatten();
    if found.is_none() {
        return;
    }
    let access = chat_access(app, gid, user).await;
    let player = access.as_ref().is_some_and(|a| match sc {
        ChatScope::Team(t) => a.team == t && a.team_open(),
        ChatScope::Game => a.game_open(),
    });
    if !player && !d["isMod"].as_bool().unwrap_or(false) {
        return;
    }
    let text: String = counting::js::trim(raw)
        .encode_utf16()
        .take(CHAT_MAX_LEN)
        .collect::<Vec<u16>>()
        .pipe_str();
    if text.is_empty() {
        return;
    }
    let team = access
        .as_ref()
        .filter(|_| player)
        .map(|a| (a.team, a.team_name.as_str()));
    chat_message(app, gid, sc, user, team, text, d["discordMessageId"].as_str()).await;
}

trait PipeStr {
    fn pipe_str(self) -> String;
}
impl PipeStr for Vec<u16> {
    fn pipe_str(self) -> String {
        String::from_utf16_lossy(&self)
    }
}

pub fn register(s: &SocketRef, app: &Arc<App>) {
    for ev in [
        "bingo_create",
        "bingo_join",
        "bingo_start",
        "bingo_kick",
        "bingo_forfeit",
        "bingo_leave",
        "bingo_leave_game",
        "bingo_cancel",
        "bingo_update_settings",
        "bingo_move_member",
        "bingo_update_team_lock",
    ] {
        let app = app.clone();
        let act = ev.trim_start_matches("bingo_");
        s.on(ev, move |s: SocketRef, TryData::<Value>(d): TryData<Value>| {
            let app = app.clone();
            async move {
                let d = d.unwrap_or(Value::Null);
                if !crate::gateway::admit_ws(&app, &s, ev) {
                    return;
                }
                let user = user_of(&s);
                if let Err(e) = access(&app, user.as_deref()).await {
                    s.emit("bingo_action_result", &json!({ "action": act, "error": e }))
                        .ok();
                    return;
                }
                let user = user.unwrap();
                match action(&app, &user, act, &d).await {
                    Ok(result) => {
                        let msg = json!({ "action": act, "result": result });
                        s.emit("bingo_action_result", &msg).ok();
                        app.to_counter(&user, "bingo_action_result", &msg).await;
                        if let Some(gid) = result["gameId"].as_i64() {
                            game_update(&app, gid, true);
                        }
                        lobby_update(&app).await;
                    }
                    Err(e) => {
                        let msg = json!({ "action": act, "error": e });
                        s.emit("bingo_action_result", &msg).ok();
                        app.to_counter(&user, "bingo_action_result", &msg).await;
                    }
                }
            }
        });
    }
    macro_rules! on {
        ($ev:literal, |$a:ident, $s:ident, $d:ident| $body:expr) => {{
            let app = app.clone();
            s.on($ev, move |$s: SocketRef, TryData::<Value>(d): TryData<Value>| {
                let $a = app.clone();
                async move {
                    #[allow(unused_variables)]
                    let $d = d.unwrap_or(Value::Null);
                    if !crate::gateway::admit_ws(&$a, &$s, $ev) {
                        return;
                    }
                    $body;
                }
            });
        }};
    }
    on!("bingo_chat_open", |a, s, d| chat_open(&a, &s, &d).await);
    on!("bingo_chat_send", |a, s, d| chat_send(&a, &s, &d).await);
    on!("bingo_chat_leave", |a, s, d| chat_leave(&s));
    on!("bingo_admin_square", |a, s, d| {
        let Some(user) = user_of(&s) else { return };
        let admin = a.counter(&user).await.is_some_and(|c| c.has_role("admin"));
        if !admin {
            s.emit(
                "bingo_action_result",
                &json!({ "action": "admin_square", "error": "Not allowed." }),
            )
            .ok();
            return;
        }
        let team = d
            .get("teamId")
            .filter(|v| !v.is_null())
            .and_then(Value::as_f64)
            .map(|v| v as i64);
        let r = admin_square(
            &a,
            d["gameId"].as_f64().unwrap_or(0.0) as i64,
            d["index"].as_f64().unwrap_or(-1.0) as i64,
            team,
            d["score"].as_f64(),
        )
        .await;
        let msg = match r {
            Ok(result) => json!({ "action": "admin_square", "result": result }),
            Err(e) => json!({ "action": "admin_square", "error": e }),
        };
        s.emit("bingo_action_result", &msg).ok()
    });
    on!("bingo_admin_sweep", |a, s, d| {
        let Some(user) = user_of(&s) else { return };
        if !a.counter(&user).await.is_some_and(|c| c.has_role("admin")) {
            s.emit("bingo_sweep_status", &json!({ "error": "Not allowed." })).ok();
            return;
        }
        if d["run"].as_bool().unwrap_or(false) {
            sweep(&a).await;
        }
        s.emit("bingo_sweep_status", &sweep_status()).ok()
    });
    on!("bingo_list", |a, s, d| {
        s.join(LOBBY_ROOM);
        let data = lobby(&a, user_of(&s).as_deref()).await;
        s.emit("bingo_list", &data).ok()
    });
    on!("bingo_list_leave", |a, s, d| {
        s.leave(LOBBY_ROOM);
    });
    on!("bingo_game", |a, s, d| {
        let Some(user) = user_of(&s) else {
            s.emit("bingo_game", &json!({ "error": "Unauthorized" })).ok();
            return;
        };
        let gid = d["gameId"].as_f64().unwrap_or(0.0) as i64;
        let r: R<Value> = async {
            if gid == 0 {
                return err("Invalid game id.");
            }
            let g = game(&a, gid).await?.ok_or("Game not found.")?;
            if g.status == "lobby" && !g.allow_join && member(&a, gid, &user).await?.is_none() {
                return err("Not authorized to view this game.");
            }
            let room = format!("bingo_game_{gid}");
            if let Some((_, prev)) = GAME_ROOM.remove(&s.id.to_string())
                && prev != room
            {
                s.leave(prev);
            }
            s.join(room.clone());
            GAME_ROOM.insert(s.id.to_string(), room);
            GAME_ROOM_USER.insert(s.id.to_string(), user.clone());
            game_state(&a, gid).await
        }
        .await;
        match r {
            Ok(p) => s.emit("bingo_game", &p).ok(),
            Err(e) => s.emit("bingo_game", &json!({ "error": e })).ok(),
        }
    });
    on!("bingo_active", |a, s, d| {
        let Some(user) = user_of(&s) else {
            s.emit("bingo_active", &json!({ "gameId": null, "error": "Unauthorized" }))
                .ok();
            return;
        };
        let active = active_game_of(&a, &user).await.ok().flatten().map(|(g, _)| g);
        s.emit("bingo_active", &json!({ "gameId": active })).ok()
    });
}

pub fn on_disconnect(s: &SocketRef) {
    CHAT_ROOM.remove(&s.id.to_string());
    GAME_ROOM.remove(&s.id.to_string());
    GAME_ROOM_USER.remove(&s.id.to_string());
}

#[cfg(test)]
mod tests {
    use super::{Interval, Member, pick_interval_winner};

    fn member_on(user: &str, team: i64) -> Member {
        Member {
            id: 0,
            team_id: team,
            user: user.into(),
            role: "player".into(),
            status: "active".into(),
            joined_at: 0,
            left_at: 0,
        }
    }

    fn interval(counts: &[(&str, i64, i64)]) -> Interval {
        let top = counts.iter().map(|c| c.1).max().unwrap_or(0) as f64;
        Interval {
            ms: 1000.0,
            qualified: counts
                .iter()
                .filter(|c| c.1 as f64 >= top * 0.25)
                .map(|c| c.0.to_string())
                .collect(),
            counts: counts.iter().map(|c| (c.0.to_string(), (c.1, c.2))).collect(),
        }
    }

    #[test]
    fn split_goes_to_the_team_that_counted_most_not_the_closer() {
        let players = [member_on("wave", 1), member_on("blaze", 2)];
        let w = pick_interval_winner(&players, &interval(&[("wave", 9, 99), ("blaze", 1, 100)]));
        assert_eq!(w, Some((1, "wave".into())));
        let w = pick_interval_winner(&players, &interval(&[("wave", 9, 99), ("outsider", 1, 100)]));
        assert_eq!(w, Some((1, "wave".into())));
    }

    #[test]
    fn split_counts_add_up_per_team() {
        let players = [member_on("w1", 1), member_on("w2", 1), member_on("b1", 2)];
        let w = pick_interval_winner(&players, &interval(&[("w1", 3, 98), ("w2", 3, 100), ("b1", 4, 99)]));
        assert_eq!(w, Some((1, "w1".into())));
    }

    #[test]
    fn split_tie_goes_to_the_team_whose_count_came_last() {
        let players = [member_on("wave", 1), member_on("blaze", 2)];
        let w = pick_interval_winner(&players, &interval(&[("wave", 5, 99), ("blaze", 5, 100)]));
        assert_eq!(w, Some((2, "blaze".into())));
        let w = pick_interval_winner(&players, &interval(&[("wave", 5, 100), ("blaze", 5, 99)]));
        assert_eq!(w, Some((1, "wave".into())));
    }

    #[test]
    fn split_needs_a_qualified_player_and_ignores_non_players() {
        let players = [member_on("wave", 1)];
        assert_eq!(
            pick_interval_winner(&players, &interval(&[("outsider", 20, 99), ("wave", 1, 100)])),
            None
        );
        let w = pick_interval_winner(&players, &interval(&[("outsider", 6, 99), ("wave", 4, 100)]));
        assert_eq!(w, Some((1, "wave".into())));
    }

    use super::{gets_landed_target, lobby_due};

    #[test]
    fn lobbies_close_when_the_host_is_away_or_after_an_hour() {
        let m = 60_000;
        assert_eq!(lobby_due(59 * m, 0, 59 * m), None);
        assert_eq!(lobby_due(60 * m, 0, 60 * m), Some("open an hour"));
        assert_eq!(lobby_due(20 * m, 0, 11 * m), None);
        assert_eq!(lobby_due(20 * m, 0, 10 * m), Some("host away"));
    }

    #[test]
    fn gets_landed_scales_with_the_counts_challenge_and_never_drops_below_one() {
        assert_eq!(gets_landed_target(450, 1000), 1);
        assert_eq!(gets_landed_target(4000, 1000), 1);
        assert_eq!(gets_landed_target(15000, 1000), 4);
        assert_eq!(gets_landed_target(15000, 500), 8);
        assert_eq!(gets_landed_target(10, 1000), 1);
        assert_eq!(gets_landed_target(1000, 0), 250);
    }
}
