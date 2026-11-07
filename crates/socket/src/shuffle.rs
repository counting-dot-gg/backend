use crate::app::App;
use crate::daily::eastern_day_key;
use crate::gateway::counter_of;
use counting::seedrandom::SeedRandom;
use serde_json::{Value, json};
use socketioxide::extract::SocketRef;
use sqlx::Row;

fn rand36() -> String {
    const A: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut bytes = [0u8; 11];
    getrandom::fill(&mut bytes).expect("the OS random source is available");
    bytes.iter().map(|b| A[*b as usize % 36] as char).collect()
}

struct Game {
    id: i64,
    seed: String,
    fake_seed: String,
    size: i64,
    moves: Vec<i64>,
    status: String,
    score: i64,
    preseeded: bool,
    day: Option<String>,
    user_uuid: Option<String>,
    client_id: Option<String>,
}

fn game_from(r: &sqlx::mysql::MySqlRow) -> Game {
    Game {
        id: r.get::<i32, _>("id") as i64,
        seed: r.get("seed"),
        fake_seed: r.get("fakeSeed"),
        size: r.get::<i32, _>("size") as i64,
        moves: r
            .get::<Option<String>, _>("moves")
            .and_then(|m| serde_json::from_str(&m).ok())
            .unwrap_or_default(),
        status: r.get("gameStatus"),
        score: r.get::<i32, _>("score") as i64,
        preseeded: r.get::<i8, _>("isPreseeded") != 0,
        day: r.get("day"),
        user_uuid: r.get("user_uuid"),
        client_id: r.get("client_id"),
    }
}

const COLS: &str =
    "id, client_id, user_uuid, seed, fakeSeed, size, CAST(moves AS CHAR) AS moves, gameStatus, score, isPreseeded, day";

fn number_seed(seed: &str, secret: &str) -> String {
    if seed.starts_with("daily_") {
        format!("{seed}:{secret}")
    } else {
        seed.to_owned()
    }
}

pub async fn play(app: &std::sync::Arc<App>, s: &SocketRef, data: &Value) {
    let pool = &app.pool;
    let client_id = s.id.to_string();
    let mut is_daily = data["isDaily"].as_bool().unwrap_or(false) || data["isDaily"].as_i64().is_some_and(|v| v != 0);
    let today = eastern_day_key(crate::app::now_ms());
    let mut size = data["size"]
        .as_f64()
        .filter(|n| *n >= 2.0 && *n <= 20.0 && n.fract() == 0.0)
        .map_or(10, |n| n as i64);
    let counter = counter_of(app, s).await;
    if counter.as_ref().is_some_and(|c| c.has_role("banned")) {
        return;
    }
    let mut preseeded = true;
    let (seed, fake_seed);
    let chosen_by_client = data["seed"].as_str().is_some_and(|s| !s.is_empty());
    match data.get("seed_id").filter(|v| !v.is_null()) {
        None => {
            seed = if is_daily {
                format!("daily_{today}")
            } else if let Some(sd) = data["seed"].as_str().filter(|s| !s.is_empty()) {
                sd.to_owned()
            } else {
                format!("{}{}", rand36(), rand36())
            };
            fake_seed = if is_daily { today.clone() } else { rand36() };
            preseeded = is_daily || chosen_by_client;
        }
        Some(seed_id) => {
            let fid = seed_id
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| seed_id.to_string());
            let first = sqlx::query(&format!(
                "SELECT {COLS} FROM numbershuffle WHERE fakeSeed = ? ORDER BY id ASC LIMIT 1"
            ))
            .bind(&fid)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten()
            .map(|r| game_from(&r));
            match first {
                Some(g) if g.seed.starts_with("daily_") => {
                    seed = g.seed;
                    fake_seed = g.fake_seed;
                    is_daily = true;
                    size = g.size;
                }
                Some(g) if g.status != "in_progress" => {
                    seed = g.seed;
                    fake_seed = g.fake_seed;
                    size = g.size;
                }
                Some(g)
                    if counter
                        .as_ref()
                        .is_some_and(|c| g.user_uuid.as_deref() == Some(&c.uuid)) =>
                {
                    seed = g.seed;
                    fake_seed = g.fake_seed;
                    size = g.size;
                }
                Some(g) if counter.is_none() && g.client_id.as_deref() == Some(&client_id) => {
                    seed = g.seed;
                    fake_seed = g.fake_seed;
                    size = g.size;
                }
                _ => {
                    seed = format!("{}{}", rand36(), rand36());
                    fake_seed = rand36();
                    preseeded = false;
                }
            }
        }
    }

    let existing = match &counter {
        Some(c) => {
            sqlx::query(&format!(
                "SELECT {COLS} FROM numbershuffle WHERE user_uuid = ? AND seed = ? LIMIT 1"
            ))
            .bind(&c.uuid)
            .bind(&seed)
            .fetch_optional(pool)
            .await
        }
        None => {
            sqlx::query(&format!(
                "SELECT {COLS} FROM numbershuffle WHERE client_id = ? AND user_uuid IS NULL AND seed = ? LIMIT 1"
            ))
            .bind(&client_id)
            .bind(&seed)
            .fetch_optional(pool)
            .await
        }
    };
    let Ok(existing) = existing else { return };
    let (mut game, is_new) = match existing {
        Some(r) => (game_from(&r), false),
        None => {
            if !(2..=20).contains(&size) {
                return;
            }
            let day = is_daily.then(|| seed.replace("daily_", ""));
            let moves = vec![0i64; size as usize];
            let r = sqlx::query(
                "INSERT INTO numbershuffle (client_id, user_uuid, seed, fakeSeed, size, moves, gameStatus, score, isPreseeded, day) \
                 VALUES (?, ?, ?, ?, ?, ?, 'in_progress', 0, ?, ?)",
            )
            .bind(&client_id)
            .bind(counter.as_ref().map(|c| c.uuid.clone()))
            .bind(&seed)
            .bind(&fake_seed)
            .bind(size)
            .bind(serde_json::to_string(&moves).unwrap())
            .bind(preseeded)
            .bind(&day)
            .execute(pool)
            .await;
            let Ok(r) = r else { return };
            (
                Game {
                    id: r.last_insert_id() as i64,
                    seed: seed.clone(),
                    fake_seed: fake_seed.clone(),
                    size,
                    moves,
                    status: "in_progress".into(),
                    score: 0,
                    preseeded,
                    day,
                    user_uuid: counter.as_ref().map(|c| c.uuid.clone()),
                    client_id: Some(client_id.clone()),
                },
                true,
            )
        }
    };
    let mut rng = SeedRandom::new(&number_seed(&game.seed, &app.cfg.game_secret));
    let numbers: Vec<i64> = (0..game.size)
        .map(|_| (rng.next_f64() * 1000.0).floor() as i64 + 1)
        .collect();

    if (counter.is_some() && game.status != "in_progress") || is_new {
        return emit(
            app,
            s,
            &game,
            &numbers,
            counter.as_ref().map(|c| c.uuid.as_str()),
            json!({}),
        )
        .await;
    }
    let Some(spot) = data["newMoveSpot"].as_f64() else {
        return emit(
            app,
            s,
            &game,
            &numbers,
            counter.as_ref().map(|c| c.uuid.as_str()),
            json!({}),
        )
        .await;
    };
    let next = numbers.get(game.score as usize).copied().unwrap_or(0);
    let legal = spot >= 0.0
        && spot.fract() == 0.0
        && (spot as usize) < game.moves.len()
        && game.moves[spot as usize] == 0
        && !game.moves[..spot as usize].iter().any(|m| *m > 0 && *m > next)
        && !game.moves[spot as usize + 1..].iter().any(|m| *m > 0 && *m < next);
    if !legal {
        return emit(
            app,
            s,
            &game,
            &numbers,
            counter.as_ref().map(|c| c.uuid.as_str()),
            json!({}),
        )
        .await;
    }
    game.moves[spot as usize] = next;
    game.score += 1;
    let mut details = json!({});
    if game.score == game.size {
        game.status = "won".into();
        if let Some(c) = &counter {
            let daily = game.seed.starts_with("daily_");
            if daily {
                app.add_xp(&c.uuid, game.size * 10).await;
            } else if !game.preseeded && game.size == 20 {
                app.add_xp(&c.uuid, 20000).await;
            }
            app.achievements.record(
                &c.uuid,
                stats::achievements::LiveEvent::ShuffleWon {
                    daily,
                    preseeded: game.preseeded,
                    size: game.size,
                },
            );
            crate::bingo::on_number_shuffle_win(app, c, game.size, game.preseeded).await;
            crate::rank::on_number_shuffle_win(app, c, game.size, game.preseeded, daily).await;
        }
    } else {
        let next = numbers[game.score as usize];
        if possible_moves(&game.moves, next).is_empty() {
            game.status = "lost".into();
        }
        details = json!({ "fresh_loss": true });
    }
    let _ = sqlx::query("UPDATE numbershuffle SET moves = ?, score = ?, gameStatus = ? WHERE id = ?")
        .bind(serde_json::to_string(&game.moves).unwrap())
        .bind(game.score)
        .bind(&game.status)
        .bind(game.id)
        .execute(pool)
        .await;
    emit(
        app,
        s,
        &game,
        &numbers,
        counter.as_ref().map(|c| c.uuid.as_str()),
        details,
    )
    .await;
}

pub fn possible_moves(moves: &[i64], n: i64) -> Vec<usize> {
    (0..moves.len())
        .filter(|&i| moves[i] == 0)
        .filter(|&i| {
            let lower_ok = moves[..i].iter().rev().find(|m| **m != 0).is_none_or(|m| n >= *m);
            let upper_ok = moves[i + 1..].iter().find(|m| **m != 0).is_none_or(|m| n <= *m);
            lower_ok && upper_ok
        })
        .collect()
}

async fn emit(
    app: &std::sync::Arc<App>,
    s: &SocketRef,
    g: &Game,
    numbers: &[i64],
    counter: Option<&str>,
    details: Value,
) {
    let random_number = if g.score == g.size {
        Value::Null
    } else {
        json!(numbers[g.score as usize])
    };
    let mut out = json!({});
    if g.status != "in_progress" {
        let scores: Vec<i32> =
            sqlx::query_scalar("SELECT score FROM numbershuffle WHERE gameStatus <> 'in_progress' AND seed = ?")
                .bind(&g.seed)
                .fetch_all(&app.pool)
                .await
                .unwrap_or_default();
        let mut hist = vec![0i64; g.size as usize + 1];
        for sc in scores {
            if let Some(b) = hist.get_mut(sc as usize) {
                *b += 1;
            }
        }
        out["all_scores"] = json!(hist);
        if let Some(c) = counter {
            let best: Option<(i32, String)> = sqlx::query_as(
                "SELECT score, fakeSeed FROM numbershuffle WHERE gameStatus <> 'in_progress' AND size = ? AND user_uuid = ? \
                 ORDER BY score DESC LIMIT 1",
            )
            .bind(g.size)
            .bind(c)
            .fetch_optional(&app.pool)
            .await
            .ok()
            .flatten();
            out["highScore"] = json!(best.as_ref().map(|b| b.0));
            out["highScoreSeed"] = json!(best.map(|b| b.1));
        }
    }
    let base = json!({
        "id": g.id, "size": g.size, "score": g.score, "day": g.day, "seed_id": g.fake_seed, "moves": g.moves,
        "randomNumber": random_number, "gameStatus": g.status,
    });
    for (k, v) in base
        .as_object()
        .unwrap()
        .iter()
        .chain(details.as_object().unwrap().iter())
    {
        out[k] = v.clone();
    }
    if out["randomNumber"].is_null() {
        out.as_object_mut().unwrap().remove("randomNumber");
    }
    s.emit("number_shuffle", &out).ok();
}

pub async fn games(app: &App, s: &SocketRef) {
    let Some(c) = counter_of(app, s).await else { return };
    if c.has_role("banned") {
        return;
    }
    let rows = sqlx::query(
        "SELECT score, size, fakeSeed, gameStatus FROM numbershuffle WHERE user_uuid = ? ORDER BY id DESC LIMIT 1000",
    )
    .bind(&c.uuid)
    .fetch_all(&app.pool)
    .await
    .unwrap_or_default();
    let out: Vec<Value> = rows
        .iter()
        .map(|r| {
            json!({
                "score": r.get::<i32, _>("score"),
                "size": r.get::<i32, _>("size"),
                "gameStatus": r.get::<String, _>("gameStatus"),
                "seed": r.get::<String, _>("fakeSeed"),
            })
        })
        .collect();
    s.emit("number_shuffle_games", &out).ok();
}

#[cfg(test)]
mod tests {
    #[test]
    fn moves() {
        assert_eq!(super::possible_moves(&[0, 0, 0], 5), vec![0, 1, 2]);
        assert_eq!(super::possible_moves(&[3, 0, 9, 0], 5), vec![1]);
        assert_eq!(super::possible_moves(&[3, 0, 4, 0], 5), vec![3]);
    }
}

#[cfg(test)]
mod seed_tests {
    use super::number_seed;

    #[test]
    fn daily_numbers_depend_on_the_secret() {
        assert_ne!(
            number_seed("daily_2026-11-05", "a"),
            number_seed("daily_2026-11-05", "b")
        );
    }

    #[test]
    fn other_games_use_their_own_seed() {
        assert_eq!(number_seed("abc123", "a"), "abc123");
    }
}
