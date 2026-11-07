use crate::app::{App, now_ms};
use crate::daily::eastern_day_bounds;
use crate::db;
use crate::gateway::{counter_of, ip_of, user_of};
use crate::model::Post;
use crate::wal::{Append, DeletedComment, WalRecord};
use dashmap::DashMap;
use serde_json::{Map, Value, json};
use socketioxide::extract::SocketRef;
use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

static OVERLAY: LazyLock<DashMap<String, (Arc<Post>, Instant)>> = LazyLock::new(DashMap::new);
static MUTATE: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

pub async fn find_post(app: &App, uuid: &str) -> Option<Arc<Post>> {
    if let Some(p) = OVERLAY.get(uuid) {
        return Some(p.0.clone());
    }
    for h in app.threads_by_uuid.iter() {
        if let Some(p) = h.ring_find(uuid) {
            return Some(p);
        }
    }
    db::post_by_uuid(&app.pool, uuid).await.ok().flatten().map(Arc::new)
}

fn remember(app: &App, post: Arc<Post>) {
    if let Some(h) = app.thread_by_uuid(&post.thread) {
        h.ring_replace(post.clone());
    }
    OVERLAY.insert(post.uuid.clone(), (post, Instant::now()));
    if OVERLAY.len() > 10_000 {
        OVERLAY.retain(|_, (_, t)| t.elapsed() < Duration::from_secs(60));
    }
}

static EMOTES: LazyLock<parking_lot::Mutex<(HashSet<String>, Option<Instant>)>> =
    LazyLock::new(|| parking_lot::Mutex::new((HashSet::new(), None)));

async fn is_dynamic_emote(app: &App, id: &str) -> bool {
    let stale = EMOTES.lock().1.is_none_or(|t| t.elapsed() > Duration::from_secs(60));
    if stale
        && let Ok(rows) = sqlx::query_scalar::<_, String>("SELECT name FROM emote WHERE approved = 1")
            .fetch_all(&app.pool)
            .await
    {
        *EMOTES.lock() = (
            rows.into_iter().map(|n| n.to_lowercase()).collect(),
            Some(Instant::now()),
        );
    }
    EMOTES.lock().0.contains(&id.to_lowercase())
}

pub async fn update_reactions(app: &App, s: &SocketRef, data: &Value) {
    let (Some(post_uuid), Some(id)) = (data["post_uuid"].as_str(), data["id"].as_str()) else {
        return;
    };
    let _g = MUTATE.lock().await;
    let Some(post) = find_post(app, post_uuid).await else {
        return;
    };
    let Some(counter) = counter_of(app, s).await else {
        return;
    };
    let Some(thread) = app.thread_by_uuid(&post.thread) else {
        return;
    };
    let valid = cgg_web::reactions::builtin(id) || is_dynamic_emote(app, id).await;
    if !valid || !counter.has_role("counter") || counter.has_role("banned") || counter.has_role("muted") {
        return;
    }
    let mut reactions: Map<String, Value> = post.reactions.as_object().cloned().unwrap_or_default();
    match reactions.get_mut(id).and_then(|v| v.as_array_mut()) {
        None => {
            reactions.insert(id.to_owned(), json!([counter.uuid]));
        }
        Some(list) => match list.iter().position(|u| u.as_str() == Some(&counter.uuid)) {
            Some(i) => {
                list.remove(i);
                if list.is_empty() {
                    reactions.remove(id);
                }
            }
            None => list.push(json!(counter.uuid)),
        },
    }
    if reactions.len() >= 11 {
        return;
    }
    app.achievements
        .record(&counter.uuid, stats::achievements::LiveEvent::Reacted { reaction: id });
    let mut p = (*post).clone();
    p.reactions = Value::Object(reactions.clone());
    let p = Arc::new(p);
    remember(app, p.clone());
    let rec = WalRecord::PostUpdate {
        uuid: p.uuid.clone(),
        id: Some(p.id),
        reactions: Some(p.reactions.clone()),
        deleted_comment: None,
    };
    let (room, lane) = {
        let m = thread.meta.read();
        (format!("thread_{}", m.uuid), m.id)
    };
    let io = app.io.clone();
    let msg = json!({ "post_uuid": p.uuid, "reactions": reactions });
    app_wal(app).append(Append {
        payload: serde_json::to_vec(&rec).unwrap(),
        action: Some(Box::pin(async move {
            let _ = io.to(room).emit("updateReaction", &msg).await;
        })),
        lane,
    });
}

fn app_wal(app: &App) -> &crate::wal::Wal {
    app.wal.get().expect("wal started")
}

pub async fn delete_comment(app: &App, s: &SocketRef, data: &Value) {
    let Some(uuid) = data["uuid"].as_str() else { return };
    let Some(deleter) = counter_of(app, s).await else {
        return;
    };
    delete_comment_as(app, &deleter, uuid).await;
}

pub async fn delete_comment_as(app: &App, deleter: &crate::model::Counter, uuid: &str) {
    let _g = MUTATE.lock().await;
    let Some(post) = find_post(app, uuid).await else { return };
    let Some(thread) = app.thread_meta(&post.thread) else {
        return;
    };
    let Some(poster) = app.counter(&post.author_uuid).await else {
        return;
    };
    let allowed = deleter.has_role("admin")
        || deleter.has_role("mod")
        || deleter.uuid == poster.uuid
        || deleter.uuid == "[SYSTEM]";
    if !allowed || !post.has_comment || post.is_comment_deleted || post.is_deleted {
        return;
    }
    app.bot.emit(
        "logDelete",
        json!({ "deleter": &deleter.json, "counter": &poster.json, "post": &*post }),
    );
    app.bot.emit(
        "chatMirrorDeletePost",
        json!({ "postUUID": post.uuid, "deletedText": post.raw_text }),
    );
    let mut p = (*post).clone();
    p.has_comment = false;
    p.is_comment_deleted = true;
    p.comment = None;
    p.raw_text = p.count_content.clone().unwrap_or_default();
    let p = Arc::new(p);
    remember(app, p.clone());
    let rec = WalRecord::PostUpdate {
        uuid: p.uuid.clone(),
        id: Some(p.id),
        reactions: None,
        deleted_comment: Some(DeletedComment {
            raw_text: p.raw_text.clone(),
        }),
    };
    let io = app.io.clone();
    let room = format!("thread_{}", thread.uuid);
    let chat = thread.name == "no_counting";
    let out = p.json_with_nulls();
    app_wal(app).append(Append {
        payload: serde_json::to_vec(&rec).unwrap(),
        action: Some(Box::pin(async move {
            let _ = io.to(room).emit("deleteComment", &out).await;
            if chat {
                let _ = io.to("site_chat").emit("deleteComment", &out).await;
            }
        })),
        lane: thread.id,
    });
}

static ROB: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

pub async fn rob(app: &App, s: &SocketRef) {
    let _g = ROB.lock().await;
    let Some(counter) = counter_of(app, s).await else {
        return;
    };
    if counter.has_role("banned") || !counter.has_role("counter") {
        return;
    }
    let now = now_ms();
    let (midnight, _) = eastern_day_bounds(now);
    let recent: Option<String> =
        sqlx::query_scalar("SELECT timestamp FROM rob WHERE counterUUID = ? ORDER BY id DESC LIMIT 1")
            .bind(&counter.uuid)
            .fetch_optional(&app.pool)
            .await
            .ok()
            .flatten();
    if let Some(ts) = recent {
        if counting::js::parse_float(&ts) >= midnight as f64 {
            return;
        }
        let last_rob = counter
            .fields
            .get("lastRob")
            .and_then(|v| v.as_str())
            .map(counting::js::parse_float);
        if last_rob.is_some_and(|l| l >= midnight as f64) {
            return;
        }
    }
    let Some(bank) = app.daily.lock().bank else { return };
    let Some(main) = app.thread("main") else { return };
    let newest = main.ring.read().back().map(|p| p.uuid.clone());
    let main_id = main.meta.read().id;
    let newest = match newest {
        Some(u) => u,
        None => match sqlx::query_scalar::<_, String>(
            "SELECT uuid FROM post FORCE INDEX (n_thread_id) WHERE threadId = ? ORDER BY id DESC LIMIT 1",
        )
        .bind(main_id)
        .fetch_optional(&app.pool)
        .await
        {
            Ok(Some(u)) => u,
            _ => return,
        },
    };
    let rob_time = now.to_string();
    let id = match sqlx::query("INSERT INTO rob (counterUUID, postUUID, moneyRobbed, timestamp) VALUES (?, ?, ?, ?)")
        .bind(&counter.uuid)
        .bind(&newest)
        .bind(bank)
        .bind(&rob_time)
        .execute(&app.pool)
        .await
    {
        Ok(r) => r.last_insert_id(),
        Err(e) => {
            tracing::warn!("rob insert failed: {e}");
            return;
        }
    };
    app.add_money(&counter.uuid, bank).await;
    let saved = json!({
        "counterUUID": counter.uuid, "postUUID": newest, "moneyRobbed": bank, "timestamp": rob_time, "id": id,
    });
    let main_uuid = main.meta.read().uuid.clone();
    let robs = {
        let mut d = app.daily.lock();
        d.bank = Some(0);
        if d.threads.contains_key(&main_uuid) {
            d.robs.push(saved);
            d.rob_counters.push(counter.clone());
            Some(d.robs_json())
        } else {
            None
        }
    };
    if let Some(r) = robs {
        app.to(format!("thread_{main_uuid}"), "dailyRobs", &r).await;
    }
    let _ = sqlx::query("UPDATE counters SET lastRob = ? WHERE uuid = ?")
        .bind(&rob_time)
        .bind(&counter.uuid)
        .execute(&app.pool)
        .await;
    let updated = app.put_counter(counter.with("lastRob", json!(rob_time)));
    s.emit("setCounter", &updated.json).ok();
    s.emit("rob_result", &json!({ "amount": bank })).ok();
    app.to_all("bank", &0).await;
}

fn categories_valid(update: &Value) -> bool {
    let Some(list) = update.as_array() else { return false };
    let items_ok = list.iter().all(|i| {
        i.is_object()
            && i["name"].as_str().is_some_and(|n| n.encode_utf16().count() <= 200)
            && i["threadUUIDs"]
                .as_array()
                .is_some_and(|a| a.iter().all(Value::is_string))
            && i["expanded"].is_boolean()
    });
    items_ok && list.iter().any(|i| i["name"].as_str() == Some("Other"))
}

pub async fn update_categories(app: &App, s: &SocketRef, data: &Value) {
    let Some(user) = user_of(s) else { return };
    let update = &data["update"];
    if !categories_valid(update) {
        return;
    }
    let changed = data["miscSettingsChanged"].as_f64().unwrap_or(f64::NAN);
    let row: Option<(i32, String, i64)> =
        sqlx::query_as("SELECT id, CAST(categories AS CHAR), lastUpdated FROM miscsettings WHERE userUuid = ?")
            .bind(&user)
            .fetch_optional(&app.pool)
            .await
            .ok()
            .flatten();
    let now = now_ms();
    let mut created_user: Option<Value> = None;
    let id = match row {
        Some((id, _, last)) => {
            if changed < (last - 2000) as f64 {
                s.emit("popupMessage", &"New settings set on another tab, please refresh")
                    .ok();
                return;
            }
            id as u64
        }
        None if {
            created_user = app.user_json(&user).await;
            created_user.is_none()
        } =>
        {
            return;
        }
        None => match sqlx::query("INSERT INTO miscsettings (categories, lastUpdated, userUuid) VALUES ('[]', ?, ?)")
            .bind(now)
            .bind(&user)
            .execute(&app.pool)
            .await
        {
            Ok(r) => r.last_insert_id(),
            Err(_) => return,
        },
    };
    if sqlx::query("UPDATE miscsettings SET categories = ?, lastUpdated = ? WHERE id = ?")
        .bind(update.to_string())
        .bind(now)
        .bind(id)
        .execute(&app.pool)
        .await
        .is_err()
    {
        return;
    }
    let mut settings = json!({ "categories": update, "lastUpdated": now });
    if let Some(mut u) = created_user {
        u["miscSettings"] = Value::Null;
        settings["user"] = u;
    }
    settings["id"] = json!(id);
    app.to_counter(&user, "updateMiscSettings", &settings).await;
    s.emit("miscSettingsLastUpdated", &now).ok();
}

static REDDIT_LAG: LazyLock<DashMap<String, (Vec<f64>, Instant)>> = LazyLock::new(DashMap::new);

pub async fn reddit_submit(app: &App, s: &SocketRef, data: &Value) {
    let Some(counter) = counter_of(app, s).await else {
        return;
    };
    if counter.has_role("banned") || counter.has_role("muted") {
        return;
    }
    let Some(user) = app.user_json(&counter.uuid).await else {
        return;
    };
    let Some(reddit) = user["reddit"].as_str().filter(|r| !r.is_empty()).map(str::to_owned) else {
        return;
    };
    let message = data["message"].as_str().unwrap_or("").trim_end().to_owned();
    let thread_id = data["thread_id"].as_str().unwrap_or("").to_owned();
    let lags = {
        let mut e = REDDIT_LAG
            .entry(counter.uuid.clone())
            .or_insert((vec![0.0], Instant::now()));
        if e.1.elapsed() > Duration::from_secs(300) {
            e.0 = vec![0.0];
        }
        if let Some(prev) = data["prevLag"].as_array() {
            e.0.extend(prev.iter().filter_map(Value::as_f64));
        } else if let Some(p) = data["prevLag"].as_f64() {
            e.0.push(p);
        }
        let start = e.0.len().saturating_sub(51);
        let mut lags = e.0[start..].to_vec();
        lags.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        e.0 = lags.clone();
        e.1 = Instant::now();
        lags
    };
    let median = if lags.is_empty() {
        0.0
    } else if lags.len() % 2 == 0 {
        (lags[lags.len() / 2 - 1] + lags[lags.len() / 2]) / 2.0
    } else {
        lags[lags.len() / 2]
    };
    let now = now_ms();
    app.to(
        format!("reddit_{thread_id}"),
        "reddit_post",
        &json!({
            "id": crate::ids::uuid_for(app.clock.now_ns()),
            "message": message,
            "username": reddit,
            "timestamp": now as f64 + median,
            "real_timestamp": now,
            "median_lag": median,
            "counter": &counter.json,
        }),
    )
    .await;
}

pub async fn stats_splits(app: &App, s: &SocketRef, data: &Value) {
    let Some(counter) = counter_of(app, s).await else {
        return;
    };
    if counter.has_role("banned") || counter.has_role("muted") {
        return;
    }
    let Some(user) = app.user_json(&counter.uuid).await else {
        return;
    };
    let n = |k: &str| data[k].as_f64().unwrap_or(f64::NAN);
    let (size, offset, min_posts) = (n("split_size"), n("split_offset"), n("min_posts_qualification"));
    if !(size >= 10.0) || min_posts < 0.0 || min_posts > size || offset < 0.0 || offset > size {
        return;
    }
    let Some(name) = data["thread_name"].as_str() else {
        return;
    };
    let Some(h) = app.thread(name) else { return };
    let thread = {
        let m = h.meta.read();
        if !m.visible_to_counter(Some(&counter)) {
            return;
        }
        m.to_json()
    };
    app.stats.emit(
        "addStatRequestToQueue",
        json!({
            "user": user,
            "counter": &counter.json,
            "thread": thread,
            "split_size": data["split_size"],
            "split_offset": data["split_offset"],
            "min_posts_qualification": data["min_posts_qualification"],
        }),
    );
}

pub async fn lrwoed_score(app: &Arc<App>, s: &SocketRef, data: &Value) {
    let Some(counter) = counter_of(app, s).await else {
        return;
    };
    let Some(score) = data["score"].as_f64() else { return };
    tracing::info!(
        "{} ({}) just played lrwoed. Score: {score}",
        counter.username,
        counter.uuid
    );
    crate::bingo::on_lrwoed(app, &counter, score).await;
    crate::rank::on_lrwoed(app, &counter, score).await;
}

fn spread(v: &Value) -> Option<String> {
    let num = |x: &Value| {
        let n = x.as_f64()?;
        (n.is_finite() && (0.0..=600_000.0).contains(&n)).then(|| n.round() as i64)
    };
    Some(format!("{},{},{}", num(&v["p50"])?, num(&v["p95"])?, num(&v["max"])?))
}

pub async fn client_latency(app: &App, s: &SocketRef, r: &Value) {
    let n = r["n"].as_f64().filter(|n| n.is_finite() && *n > 0.0 && *n <= 100_000.0);
    let thread = r["thread"].as_str().filter(|t| t.encode_utf16().count() <= 64);
    let transport = r["transport"].as_str().filter(|t| t.encode_utf16().count() <= 16);
    let (Some(n), Some(thread), Some(transport)) = (n, thread, transport) else {
        return;
    };
    let (Some(rt), Some(proc_), Some(unacc)) = (
        spread(&r["roundTrip"]),
        spread(&r["processing"]),
        spread(&r["unaccounted"]),
    ) else {
        return;
    };
    let p95 = |k: &str| Duration::from_millis(r[k]["p95"].as_f64().unwrap_or(0.0).round() as u64);
    let m = &app.metrics;
    m.client_round_trip.record(p95("roundTrip"));
    m.client_processing.record(p95("processing"));
    m.client_unaccounted.record(p95("unaccounted"));
    let opt_p95 = |k: &str| {
        spread(&r[k])?;
        (k != "ping" || r[k]["max"].as_f64()? > 0.0).then(|| p95(k))
    };
    for (k, h) in [
        ("uplink", &m.client_uplink),
        ("downlink", &m.client_downlink),
        ("ping", &m.client_ping),
        ("loopLag", &m.client_loop_lag),
        ("paint", &m.client_paint),
    ] {
        if let Some(d) = opt_p95(k) {
            h.record(d);
        }
    }
    if r["longtask"].as_bool() == Some(true)
        && let Some(b) = r["blockedMs"]
            .as_f64()
            .filter(|b| b.is_finite() && (0.0..=600_000.0).contains(b))
    {
        m.client_blocked.record(Duration::from_millis(b.round() as u64));
    }
    let reports = if transport == "websocket" {
        &m.client_reports_ws
    } else {
        &m.client_reports_polling
    };
    reports.fetch_add(1, Ordering::Relaxed);
    m.client_counts.fetch_add(n as u64, Ordering::Relaxed);
    let bands: Vec<String> = ["scheduling", "dbRead", "dbWrite", "other"]
        .iter()
        .map(|k| (r["bands"][k].as_f64().unwrap_or(0.0).round() as i64).to_string())
        .collect();
    let mut redis = app.redis.clone();
    let _: Result<String, _> = redis::cmd("XADD")
        .arg("metrics:client_latency")
        .arg("MAXLEN")
        .arg("~")
        .arg(50_000)
        .arg("*")
        .arg("t")
        .arg(now_ms())
        .arg("clientT")
        .arg(counting::js::num_to_string(r["t"].as_f64().unwrap_or(f64::NAN)))
        .arg("counter")
        .arg(user_of(s).unwrap_or_else(|| "anon".into()))
        .arg("thread")
        .arg(thread)
        .arg("transport")
        .arg(transport)
        .arg("n")
        .arg(counting::js::num_to_string(n))
        .arg("rt")
        .arg(rt)
        .arg("proc")
        .arg(proc_)
        .arg("unacc")
        .arg(unacc)
        .arg("ping")
        .arg(spread(&r["ping"]).unwrap_or_else(|| "0,0,0".into()))
        .arg("blocked")
        .arg((r["blockedMs"].as_f64().unwrap_or(0.0).round() as i64).max(0))
        .arg("bands")
        .arg(bands.join(","))
        .arg("up")
        .arg(spread(&r["uplink"]).unwrap_or_else(|| "0,0,0".into()))
        .arg("down")
        .arg(spread(&r["downlink"]).unwrap_or_else(|| "0,0,0".into()))
        .arg("loop")
        .arg(spread(&r["loopLag"]).unwrap_or_else(|| "0,0,0".into()))
        .arg("paint")
        .arg(spread(&r["paint"]).unwrap_or_else(|| "0,0,0".into()))
        .query_async(&mut redis)
        .await;
    crate::connection::record(app, s, "l", crate::connection::from_latency_report(r, now_ms())).await;
}

static HEALTH_LAST: LazyLock<DashMap<String, Instant>> = LazyLock::new(DashMap::new);

pub async fn client_health(app: &App, s: &SocketRef, r: &Value) {
    {
        let id = s.id.to_string();
        if HEALTH_LAST
            .get(&id)
            .is_some_and(|t| t.elapsed() < Duration::from_secs(20))
        {
            return;
        }
        HEALTH_LAST.insert(id, Instant::now());
    }
    let num = |v: &Value, max: f64| {
        v.as_f64()
            .filter(|n| n.is_finite() && (0.0..=max).contains(n))
            .map(|n| (n * 100.0).round() / 100.0)
    };
    let int = |v: &Value, max: f64| num(v, max).map(|n| n.round() as i64);
    let text = |v: &Value, max: usize| {
        v.as_str()
            .filter(|t| t.encode_utf16().count() <= max)
            .map(str::to_owned)
    };
    let trip = |v: &Value| -> Option<Value> {
        Some(
            json!({ "p50": int(&v["p50"], 600_000.0)?, "p95": int(&v["p95"], 600_000.0)?, "max": int(&v["max"], 600_000.0)? }),
        )
    };
    let Some(route) = text(&r["route"], 64) else { return };
    let top: Vec<Value> = r["msg"]["top"]
        .as_array()
        .map(|a| {
            a.iter()
                .take(12)
                .filter_map(|e| {
                    Some(json!([
                        text(&e[0], 40)?,
                        int(&e[1], 1_000_000.0)?,
                        int(&e[2], 1e9)?,
                        int(&e[3], 600_000.0).unwrap_or(0)
                    ]))
                })
                .collect()
        })
        .unwrap_or_default();
    let own = &r["own"];
    let lf = &r["loaf"];
    let loaf = lf.is_object().then(|| {
        let scripts: Vec<Value> = lf["scripts"]
            .as_array()
            .map(|a| {
                a.iter()
                    .take(8)
                    .filter_map(|e| {
                        Some(json!([
                            text(&e[0], 160)?,
                            int(&e[1], 600_000.0)?,
                            int(&e[2], 100_000.0)?,
                            int(&e[3], 600_000.0).unwrap_or(0)
                        ]))
                    })
                    .collect()
            })
            .unwrap_or_default();
        json!({
            "n": int(&lf["n"], 100_000.0), "block": int(&lf["block"], 600_000.0),
            "style": int(&lf["style"], 600_000.0), "render": int(&lf["render"], 600_000.0), "scripts": scripts,
        })
    });
    let j = json!({
        "route": route,
        "tr": text(&r["tr"], 16),
        "vis": num(&r["vis"], 1.0),
        "lag": trip(&r["lag"]),
        "lt": { "n": int(&r["lt"]["n"], 100_000.0), "ms": int(&r["lt"]["ms"], 600_000.0) },
        "fr": { "n": int(&r["fr"]["n"], 100_000.0), "jank": int(&r["fr"]["jank"], 100_000.0), "max": int(&r["fr"]["max"], 600_000.0) },
        "ping": trip(&r["ping"]),
        "own": int(&own["n"], 100_000.0).filter(|n| *n > 0).map(|n| json!({ "n": n, "rt": trip(&own["rt"]), "proc": trip(&own["proc"]) })),
        "msg": { "total": int(&r["msg"]["total"], 10_000_000.0), "bytes": int(&r["msg"]["bytes"], 1e10), "top": top },
        "loaf": loaf,
        "mem": int(&r["mem"], 100_000.0),
        "dom": int(&r["dom"], 10_000_000.0),
        "pings": { "sent": int(&r["pings"]["sent"], 10_000.0), "back": int(&r["pings"]["back"], 10_000.0) },
        "conn": conn_of(&r["conn"]),
        "age": int(&r["age"], 1e7),
        "dev": {
            "b": text(&r["dev"]["b"], 16), "os": text(&r["dev"]["os"], 16), "m": r["dev"]["m"].as_bool(),
            "cores": int(&r["dev"]["cores"], 1024.0), "mem": num(&r["dev"]["mem"], 1024.0),
            "net": text(&r["dev"]["net"], 8), "rtt": int(&r["dev"]["rtt"], 60_000.0),
            "dl": num(&r["dev"]["dl"], 100_000.0), "w": int(&r["dev"]["w"], 20_000.0),
        },
    });
    let mut redis = app.redis.clone();
    let _: Result<String, _> = redis::cmd("XADD")
        .arg("metrics:client_health")
        .arg("MAXLEN")
        .arg("~")
        .arg(200_000)
        .arg("*")
        .arg("t")
        .arg(now_ms())
        .arg("c")
        .arg(user_of(s).unwrap_or_else(|| "anon".into()))
        .arg("j")
        .arg(j.to_string())
        .query_async(&mut redis)
        .await;
    crate::connection::record(app, s, "h", crate::connection::from_health(&j, now_ms())).await;
}

fn conn_of(c: &Value) -> Value {
    if !c.is_object() {
        return Value::Null;
    }
    let num = |v: &Value, max: f64| {
        v.as_f64()
            .filter(|n| n.is_finite() && (0.0..=max).contains(n))
            .map(f64::round)
    };
    let drops: Vec<Value> = c["drops"]
        .as_array()
        .map(|a| {
            a.iter()
                .take(20)
                .filter_map(|d| {
                    let reason = d[1].as_str().filter(|r| r.encode_utf16().count() <= 32)?;
                    Some(json!([num(&d[0], 1e15)?, reason, num(&d[2], 86_400_000.0)]))
                })
                .collect()
        })
        .unwrap_or_default();
    let edge = c["edge"].is_object().then(|| {
        json!({
            "ms": num(&c["edge"]["ms"], 60_000.0),
            "colo": c["edge"]["colo"].as_str().filter(|s| s.len() <= 8 && s.chars().all(|ch| ch.is_ascii_alphanumeric())),
        })
    });
    json!({
        "drops": drops,
        "attempts": num(&c["attempts"], 10_000.0),
        "offlineMs": num(&c["offlineMs"], 3_600_000.0),
        "netChanges": num(&c["netChanges"], 1000.0),
        "edge": edge,
    })
}

pub fn client_health_gone(s: &SocketRef) {
    HEALTH_LAST.remove(&s.id.to_string());
}

static PADLOG: LazyLock<DashMap<String, (Instant, u32)>> = LazyLock::new(DashMap::new);

pub fn pad_debug(app: &App, s: &SocketRef, body: &Value) {
    let Some(events) = body.as_array() else { return };
    {
        let mut e = PADLOG.entry(s.id.to_string()).or_insert((Instant::now(), 0));
        if e.0.elapsed() > Duration::from_secs(60) {
            *e = (Instant::now(), 0);
        }
        e.1 += 1;
        if e.1 > 60 {
            return;
        }
    }
    let who = user_of(s)
        .and_then(|u| app.counters.get(&u).map(|c| c.username.clone()))
        .unwrap_or_else(|| "anon".into());
    for e in events.iter().take(100) {
        let line: String = e.to_string().chars().take(400).collect();
        tracing::info!("[padlog] {who} {line}");
    }
    let _ = ip_of(s);
}

pub async fn add_time_online(app: &App, user: &str, ms: i64) {
    let cur: Option<String> = sqlx::query_scalar("SELECT timeOnline FROM users WHERE uuid = ?")
        .bind(user)
        .fetch_optional(&app.pool)
        .await
        .ok()
        .flatten();
    let Some(cur) = cur else { return };
    let base = counting::js::parse_float(&cur);
    let total = if base.is_nan() { 0.0 } else { base } + ms as f64;
    let _ = sqlx::query("UPDATE users SET timeOnline = ? WHERE uuid = ?")
        .bind(counting::js::num_to_string(total))
        .bind(user)
        .execute(&app.pool)
        .await;
    app.online.lock().total_ms += ms as f64;
}

pub fn default_page_socket(app: &App) -> Option<Value> {
    let users_online = app.online.lock().users.len();
    let total = app.online.lock().total_ms;
    let d = app.daily.lock();
    d.total_counts?;
    let lb = d.leaderboard();
    Some(json!({
        "users_online": users_online,
        "total_online_time": total,
        "total_counts": d.total_counts,
        "total_posts": d.total_posts,
        "total_xp": app.xp_total.load(Ordering::Relaxed),
        "daily_leaderboard": lb["all"],
        "all_leaderboards": lb,
    }))
}

pub async fn default_page_tcp(app: &App) -> Option<Value> {
    let total: Option<f64> = sqlx::query_scalar("SELECT SUM(CAST(timeOnline AS DOUBLE)) FROM users")
        .fetch_one(&app.pool)
        .await
        .ok()
        .flatten();
    let users_online = app.online.lock().users.len();
    let d = app.daily.lock();
    d.total_counts?;
    let lb = d.leaderboard();
    Some(json!({
        "usersOnline": users_online,
        "totalOnlineTime": total.unwrap_or(0.0),
        "totalCounts": d.total_counts,
        "totalPosts": d.total_posts,
        "totalXP": app.xp_total.load(Ordering::Relaxed),
        "dailyLeaderboard": lb["all"],
        "allLeaderboards": lb,
    }))
}

pub fn latest_counts(app: &App) -> Value {
    let mut m = Map::new();
    for h in app.threads_by_uuid.iter() {
        if let Some((p, _)) = h.last_count.read().as_ref() {
            m.insert(
                h.key().clone(),
                json!({ "rawCount": p.raw_count, "validCountNumber": p.valid_count_number }),
            );
        }
    }
    Value::Object(m)
}

pub async fn cached_splits(app: &App, thread_uuid: &str) -> Vec<Value> {
    let mut redis = app.redis.clone();
    let raw: Vec<String> = redis::cmd("LRANGE")
        .arg(format!("thread_splits:{thread_uuid}"))
        .arg(0)
        .arg(49)
        .query_async(&mut redis)
        .await
        .unwrap_or_default();
    raw.iter().filter_map(|s| serde_json::from_str(s).ok()).collect()
}

pub async fn cache_split(app: &App, thread_uuid: &str, split: &Value) {
    let key = format!("thread_splits:{thread_uuid}");
    let mut redis = app.redis.clone();
    let _: Result<(), _> = redis::pipe()
        .atomic()
        .cmd("LPUSH")
        .arg(&key)
        .arg(split.to_string())
        .ignore()
        .cmd("LTRIM")
        .arg(&key)
        .arg(0)
        .arg(49)
        .ignore()
        .cmd("EXPIRE")
        .arg(&key)
        .arg(60 * 60 * 24 * 7)
        .ignore()
        .query_async(&mut redis)
        .await;
}
