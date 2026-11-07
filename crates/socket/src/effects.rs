use crate::app::{App, now_ms};
use crate::config::KPartTarget;
use crate::daily::{eastern_day_key, uuid_ms};
use crate::model::{CounterRef, Post, ThreadMeta};
use crate::splits::{self, Kind};
use crate::thread::{Msg, PostJob};
use crate::wal::{Origin, WalRecord};
use counting::js::parse_int;
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;

pub async fn handle(app: &Arc<App>, rec: WalRecord) {
    if let WalRecord::Post { post, origin, .. } = rec
        && origin != Origin::System
    {
        on_post(app, &post, &origin).await;
    }
}

async fn on_post(app: &Arc<App>, post: &Post, origin: &Origin) {
    let Some(counter) = app.counter(&post.author_uuid).await else {
        return;
    };
    let Some(thread) = app.thread_meta(&post.thread) else {
        return;
    };

    let rolled = {
        let d = app.daily.lock();
        d.last_updated
            .as_ref()
            .is_some_and(|l| eastern_day_key(uuid_ms(l)) != eastern_day_key(uuid_ms(&post.uuid)))
    };
    if rolled {
        new_day(app).await;
    }

    let mut emit_total_posts = None;
    let mut emit_total_counts = None;
    let mut milestones: Vec<(String, String, String, Value)> = Vec::new();
    let mut daily_count = None;
    {
        let mut d = app.daily.lock();
        d.last_updated = Some(post.uuid.clone());
        if let Some(tp) = d.total_posts.filter(|t| *t != 0) {
            let tp = tp + 1;
            d.total_posts = Some(tp);
            emit_total_posts = Some(tp);
            if tp % 1_000_000 == 0 {
                milestones.push((
                    format!(
                        "We just hit {tp} posts... Congrats to {} for {} accomplishment in {} (for the post {}).",
                        counter.username,
                        counter.pronoun(2),
                        title(&thread),
                        post.raw_text
                    ),
                    format!(
                        "We just hit {tp} posts... Congrats to {} for {} accomplishment in {}. https://counting.gg/thread/{}/{}",
                        counter.username,
                        counter.pronoun(2),
                        title(&thread),
                        thread.name,
                        post.uuid
                    ),
                    format!("Woo! {} just posted post #{tp}!", counter.username),
                    json!({ "kind": "posts", "n": tp }),
                ));
            }
        }
        if post.is_valid_count {
            daily_count = Some(d.add_count(&thread.uuid, &counter));
            if let Some(tc) = d.total_counts.filter(|t| *t != 0) {
                let tc = tc + 1;
                d.total_counts = Some(tc);
                emit_total_counts = Some(tc);
                let cc = post.count_content.clone().unwrap_or_default();
                if tc % 1_000_000 == 0 {
                    milestones.push((
                        format!(
                            "We just hit {tc} counts... Congrats to {} for {} accomplishment in {} (for the count {cc}).",
                            counter.username,
                            counter.pronoun(2),
                            title(&thread)
                        ),
                        format!(
                            "We just hit {tc} counts... Congrats to {} for {} accomplishment in {} (for the count {cc}). https://counting.gg/thread/{}/{}",
                            counter.username,
                            counter.pronoun(2),
                            title(&thread),
                            thread.name,
                            post.uuid
                        ),
                        format!("Yay! {} just counted #{tc}!", counter.username),
                        json!({ "kind": "counts", "n": tc }),
                    ));
                }
                if post.valid_count_number.is_some_and(|n| n % 1_000_000 == 0) {
                    let n = post.valid_count_number.unwrap();
                    milestones.push((
                        format!(
                            "We just hit {n} counts in {}... Congrats to {} for {} accomplishment in {} (for the count {cc}).",
                            title(&thread),
                            counter.username,
                            counter.pronoun(2),
                            title(&thread)
                        ),
                        format!(
                            "We just hit {n} counts in {}... Congrats to {} for {} accomplishment in {} (for the count {cc}). https://counting.gg/thread/{}/{}",
                            title(&thread),
                            counter.username,
                            counter.pronoun(2),
                            title(&thread),
                            thread.name,
                            post.uuid
                        ),
                        format!("Yay! {} just counted #{n} in {}!", counter.username, title(&thread)),
                        json!({ "kind": "thread_counts", "n": n, "threadName": thread.name, "threadTitle": title(&thread) }),
                    ));
                }
            }
            if thread.name == "main"
                && let Some(b) = d.bank.as_mut()
            {
                *b += 1;
            }
        }
    }
    for (system, discord, celebration, mut detail) in milestones {
        app.system_message(&system).await;
        if let Some(channel) = &app.cfg.discord.milestone_channel {
            app.bot
                .emit("sendMessage", json!({ "channel": channel, "message": discord }));
        }
        let f = |k: &str| counter.fields.get(k).cloned().unwrap_or(Value::Null);
        detail["message"] = json!(celebration);
        detail["counter"] = json!({ "uuid": counter.uuid, "name": f("name"), "color": f("color"), "discordId": f("discordId"), "avatar": f("avatar") });
        app.to_all("celebration", &detail).await;
    }

    if post.is_valid_count {
        crate::feats::on_valid_count(app, &counter, post).await;
        if let Some(n) = daily_count {
            let best = day_best(app, &counter.uuid, &eastern_day_key(uuid_ms(&post.uuid))).await;
            if let Some(m) = daily_milestone(n, best) {
                app.to_counter(&counter.uuid, "milestone", &m).await;
            }
        }
        let first_count_in_thread_today = app
            .daily
            .lock()
            .threads
            .get(&thread.uuid)
            .map(|m| m.values().map(|e| e.counts).sum::<i64>())
            == Some(1);
        app.achievements.record(
            &counter.uuid,
            stats::achievements::LiveEvent::ValidCount {
                counts_today: daily_count,
                thread_validation: &thread.validation_type,
                first_count_in_thread_today,
            },
        );
        if let Some(xp) = xp_for(post, &thread) {
            app.add_xp(&counter.uuid, xp).await;
        }
    }

    let msg = json!({
        "post": post,
        "counter": &counter.json,
        "thread": thread.to_json(),
        "total_counts": emit_total_counts,
        "total_posts": emit_total_posts,
        "total_xp": app.xp_total.load(Ordering::Relaxed),
    });
    let _ = app
        .io
        .to("thread_all")
        .except(format!("thread_{}", thread.uuid))
        .emit("post", &msg)
        .await;

    kpart_alert(app, post, &thread);
    if let Some(c) = &post.comment
        && !c.is_empty()
    {
        crate::ggbot::spawn(app, c, &counter, &thread);
    }
    if post.has_comment {
        mentions(app, post).await;
    }
    if thread.name == "no_counting" {
        match origin {
            Origin::Socket => app.bot.emit(
                "sendChatMirror",
                json!({
                    "username": counter.fields.get("name"),
                    "color": counter.fields.get("color").and_then(|c| c.as_str()).unwrap_or(""),
                    "text": post.raw_text,
                    "discordId": counter.fields.get("discordId"),
                    "avatar": counter.fields.get("avatar"),
                    "counterUsername": counter.username,
                    "postUUID": post.uuid,
                    "timestamp": post.timestamp,
                    "timeSinceLastPost": post.time_since_last_post,
                }),
            ),
            Origin::Discord { message_id: Some(id) } => app.bot.emit(
                "storeChatMirrorInbound",
                json!({ "discordMessageId": id, "postUUID": post.uuid }),
            ),
            _ => {}
        }
    }
}

async fn day_best(app: &App, uuid: &str, today: &str) -> i64 {
    if let Some(e) = app.day_best.get(uuid).filter(|e| e.0 == today) {
        return e.1;
    }
    let mut r = app.redis.clone();
    let raw: Option<String> = redis::cmd("GET")
        .arg(format!("stats:counter_days:{uuid}"))
        .query_async(&mut r)
        .await
        .ok()
        .flatten();
    let best = raw
        .and_then(|s| serde_json::from_str::<std::collections::HashMap<String, i64>>(&s).ok())
        .and_then(|days| {
            days.into_iter()
                .filter(|(d, _)| d.as_str() < today)
                .map(|(_, n)| n)
                .max()
        })
        .unwrap_or(0);
    app.day_best.insert(uuid.to_owned(), (today.to_owned(), best));
    best
}

fn daily_milestone(n: i64, best: i64) -> Option<Value> {
    (best >= 50 && n == best + 1).then(|| json!({ "kind": "daily_best", "n": n, "best": best }))
}

fn title(t: &ThreadMeta) -> String {
    t.row
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or(&t.name)
        .to_owned()
}

fn totd(t: &ThreadMeta) -> bool {
    t.row.get("threadOfTheDay").and_then(|v| v.as_bool()).unwrap_or(false)
}

pub fn xp_for(post: &Post, t: &ThreadMeta) -> Option<i64> {
    if post.raw_count.as_deref() == Some(t.first_valid_count.as_str()) {
        return None;
    }
    let raw = post.raw_count.as_deref().map_or(f64::NAN, |r| parse_int(r, None));
    let mut xp: f64 = if !t.allow_double_posts && t.validation_type != "tugofwar" {
        5.0
    } else {
        1.0
    };
    match t.validation_type.as_str() {
        "slow" | "bars" => xp += 20.0,
        "yoco" => xp = 100.0,
        "tslc" => xp = if t.reset_on_mistakes { raw - 1.0 } else { raw * 5.0 },
        "1inx" => xp = if raw < 5.0 { 5.0 } else { raw },
        "waitx" => xp += 45.0,
        "randomhour" => xp *= 2.0,
        "randomminute" => xp *= 20.0,
        _ => {}
    }
    if totd(t) {
        xp *= 2.0;
    }
    xp.is_finite().then_some(xp as i64)
}

fn kpart_alert(app: &Arc<App>, post: &Post, t: &ThreadMeta) {
    if !post.is_valid_count {
        return;
    }
    let vt = t.validation_type.as_str();
    const NUMERIC: [&str; 13] = [
        "main",
        "slow",
        "bars",
        "parity",
        "yoco",
        "roulette",
        "tslc",
        "randomhour",
        "randomminute",
        "waitx",
        "1inx",
        "wait_2",
        "fast_tslc",
    ];
    let ids = &app.cfg.discord;
    let side_or_other = || {
        if t.allow_double_posts {
            ids.kpart_other.clone()
        } else {
            ids.kpart_side.clone()
        }
    };
    let run = |kind: Kind, target: Option<KPartTarget>| splits::spawn(app, t.clone(), post.clone(), kind, target);
    if NUMERIC.contains(&vt) {
        let c = post.raw_count.as_deref().map_or(f64::NAN, |r| parse_int(r, None));
        let odd_thread = t.name == "odds" || t.name == "odds_double";
        if c % 100.0 == 0.0 || (odd_thread && c % 100.0 == 1.0) {
            let target = if t.name == "main" {
                ids.kpart_main.clone()
            } else {
                side_or_other()
            };
            run(Kind::Numeric, target);
        }
        return;
    }
    if vt == "letters" {
        if post.raw_count.as_deref().is_some_and(|r| r.ends_with('A')) {
            run(Kind::Letters, side_or_other());
        }
        return;
    }
    let Some(n) = post.valid_count_number.filter(|n| *n > 1) else {
        return;
    };
    let (per_split, per_get) = match vt {
        "alphanumerics" => (36, 36),
        "base62" => (62, 62),
        "base64" => (64, 16),
        "coordinates" | "wave" | "incremental_odds" | "username" | "roman" | "words" => (100, 10),
        "mayan" => (20, 20),
        "permutations" | "factoradic" => (120, 6),
        "dates" => (487, 3),
        "military" | "time" => (60, 15),
        "binary" | "hexadecimal" | "quaternary" | "octonary" => (64, 16),
        "ternary" | "nonary" => (81, 9),
        "quinary" => (125, 5),
        "senary" => (108, 12),
        "septenary" => (49, 21),
        "no_repdigits" => (29, 29),
        _ => (
            t.row.get("countsPerSplit").and_then(|v| v.as_i64()).unwrap_or(0),
            t.row.get("splitsPerGet").and_then(|v| v.as_i64()).unwrap_or(0),
        ),
    };
    if per_split > 0 && per_get > 0 && n % per_split == 0 {
        run(Kind::CountNumber { per_split, per_get }, side_or_other());
    }
}

async fn mentions(app: &App, post: &Post) {
    let Some(c) = &post.comment else { return };
    let mut seen: Vec<String> = Vec::new();
    let mut found = 0;
    let b = c.as_bytes();
    let mut i = 0;
    while i < b.len() && found < 3 {
        if b[i] == b'@' {
            let start = i + 1;
            let mut j = start;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'-' || b[j] == b'_') {
                j += 1;
            }
            if j > start {
                found += 1;
                let name: String = c[start..j].chars().take(25).collect();
                let up = name.to_uppercase();
                if !seen.contains(&up) {
                    seen.push(up);
                    let uuid: Option<String> =
                        sqlx::query_scalar("SELECT uuid FROM counters WHERE username = ? LIMIT 1")
                            .bind(&name)
                            .fetch_optional(&app.pool)
                            .await
                            .ok()
                            .flatten();
                    if let Some(u) = uuid {
                        let _ =
                            sqlx::query("INSERT INTO mentions (readStatus, postUUID, counterUuid) VALUES (0, ?, ?)")
                                .bind(&post.uuid)
                                .bind(u)
                                .execute(&app.pool)
                                .await;
                    }
                }
            }
            i = j.max(i + 1);
        } else {
            i += 1;
        }
    }
}

async fn new_day(app: &Arc<App>) {
    tracing::info!("new day: thread of the day lottery and fresh leaderboards");
    let old = app.threads_by_uuid.iter().map(|h| h.meta.read().clone()).find(totd);
    if let Some(old) = old {
        lottery(app, &old).await;
    }
    if let Some(new) = pick_thread_of_the_day(app).await {
        app.system_message(&format!("New thread of the day: {}", title(&new)))
            .await;
    }
    let (uuids, totals) = {
        let mut d = app.daily.lock();
        let totals = (d.total_counts, d.total_posts);
        d.all.clear();
        for m in d.threads.values_mut() {
            m.clear();
        }
        let uuids: Vec<String> = d.threads.keys().cloned().collect();
        (uuids, totals)
    };
    let _ = totals;
    for u in uuids {
        app.to(format!("thread_{u}"), "dailyHOC", &json!({})).await;
    }
    if let Ok(robs) = crate::daily::today_robs(app, now_ms()).await {
        let ids: Vec<String> = robs
            .iter()
            .filter_map(|r| r["counterUUID"].as_str().map(str::to_owned))
            .collect();
        let counters = app.counters_of(&ids).await;
        let mut d = app.daily.lock();
        d.robs = robs;
        d.rob_counters = counters;
    }
}

async fn lottery(app: &Arc<App>, old: &ThreadMeta) {
    let entries: Vec<(String, i64)> = app
        .daily
        .lock()
        .threads
        .get(&old.uuid)
        .map(|m| m.iter().map(|(u, e)| (u.clone(), e.counts)).collect())
        .unwrap_or_default();
    let mut sum = 0i64;
    let mut odds = Vec::new();
    for (u, c) in &entries {
        odds.push((u.clone(), sum + 1, sum + c));
        sum += c;
    }
    let winning = (fastrand::f64() * sum as f64).ceil() as i64;
    let jackpot = if old.allow_double_posts {
        (sum as f64 / 5.0).ceil() as i64
    } else {
        sum
    };
    let mut text = format!(
        "Congratulations to the {} counters who counted in {} today! Time for the lottery...\n\nWe're going to roll a random number between 1 and {sum}.\n\nThe jackpot for today is {jackpot}.\n\n",
        entries.len(),
        title(old)
    );
    let mut winner: Option<CounterRef> = None;
    for (u, min, max) in &odds {
        let Some(c) = app.counter(u).await else { continue };
        if c.uuid == "[SYSTEM]" {
            continue;
        }
        text += &format!("{} is hoping for a number between {min} and {max}...\n\n", c.username);
        if winning >= *min && winning <= *max && winner.is_none() {
            winner = Some(c);
        }
    }
    let Some(w) = winner else {
        tracing::info!("lottery: no winner (number {winning}, jackpot {jackpot})");
        return;
    };
    text += &format!(
        "# The number is... {winning}! Congrats to @{}!\n\n{jackpot} has been deposited in your account. The IRS will contact you shortly.",
        w.username
    );
    app.add_money(&w.uuid, jackpot).await;
    if let Some(h) = app.thread_by_uuid(&old.uuid) {
        let job = PostJob::system(app.system_counter.clone(), text);
        let _ = h.tx.send(Msg::Post(Box::new(job))).await;
    }
}

async fn pick_thread_of_the_day(app: &Arc<App>) -> Option<ThreadMeta> {
    let seed = counting::validate::day_seed(
        "thread_of_the_day_",
        &eastern_day_key(now_ms()),
        Some(&app.cfg.game_secret),
    );
    let mut eligible: Vec<ThreadMeta> = app
        .threads_by_uuid
        .iter()
        .map(|h| h.meta.read().clone())
        .filter(|t| {
            let cat = t.row.get("category").and_then(|v| v.as_str()).unwrap_or("");
            t.visible_to.iter().any(|v| v.contains("all"))
                && ["Traditional", "Double Counting", "No Mistakes"].contains(&cat)
                && !totd(t)
                && t.updatable_by.iter().any(|r| r.contains("counter"))
                && !t.locked
                && t.auto_validated
        })
        .collect();
    eligible.sort_by(|a, b| a.uuid.cmp(&b.uuid));
    if eligible.is_empty() {
        return None;
    }
    let idx = (counting::seedrandom::SeedRandom::new(&seed).next_f64() * eligible.len() as f64).floor() as usize;
    let new = eligible[idx].clone();
    let old: Vec<_> = app
        .threads_by_uuid
        .iter()
        .filter(|h| totd(&h.meta.read()))
        .map(|h| h.clone())
        .collect();
    for h in old {
        let _ = h.tx.send(Msg::SetThreadOfTheDay(false)).await;
    }
    if let Some(h) = app.thread_by_uuid(&new.uuid) {
        let _ = h.tx.send(Msg::SetThreadOfTheDay(true)).await;
    }
    let mut new = new;
    new.row.insert("threadOfTheDay".into(), json!(true));
    tracing::info!(
        "thread of the day {seed}: {} of {} ({})",
        idx,
        eligible.len(),
        title(&new)
    );
    Some(new)
}

pub async fn run(
    app: Arc<App>,
    committed: Arc<crate::wal::Committed>,
    start: crate::cursor::Position,
    cp: Arc<std::sync::atomic::AtomicU64>,
) {
    let dir = app.cfg.wal_dir.clone();
    let a2 = app.clone();
    let r = crate::cursor::follow(dir, "effects", committed, start, cp, move |_seq, rec| {
        let app = a2.clone();
        async move {
            let t0 = Instant::now();
            handle(&app, rec).await;
            app.metrics.effects.record(t0.elapsed());
        }
    })
    .await;
    if let Err(e) = r {
        tracing::error!("effects consumer stopped: {e:#}");
        std::process::exit(72);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(vt: &str, double: bool) -> ThreadMeta {
        ThreadMeta {
            id: 1,
            uuid: "t".into(),
            name: "x".into(),
            validation_type: vt.into(),
            first_valid_count: "1".into(),
            reset_on_mistakes: false,
            allow_double_posts: double,
            auto_validated: true,
            locked: false,
            visible_to: vec![],
            updatable_by: vec![],
            count_bans: vec![],
            post_bans: vec![],
            row: Default::default(),
        }
    }

    #[test]
    fn xp_rules() {
        let mut p: Post = serde_json::from_value(json!({
            "uuid": "u", "timestamp": "1", "timeSinceLastCount": "0", "timeSinceLastPost": "0", "rawText": "7",
            "isCount": true, "isValidCount": true, "rawCount": "7", "stricken": false, "thread": "t", "threadId": 1,
            "hasComment": false, "hasThreeCharComment": false, "authorUUID": "a", "authorId": 1, "isDeleted": false,
            "isCommentDeleted": false, "reactions": {}, "macroPresetId": null, "macroPresetVersionId": null,
            "id": 1
        }))
        .unwrap();
        assert_eq!(xp_for(&p, &meta("main", false)), Some(5));
        assert_eq!(xp_for(&p, &meta("main", true)), Some(1));
        assert_eq!(xp_for(&p, &meta("slow", false)), Some(25));
        assert_eq!(xp_for(&p, &meta("1inx", false)), Some(7));
        assert_eq!(xp_for(&p, &meta("tugofwar", false)), Some(1));
        p.raw_count = Some("1".into());
        assert_eq!(xp_for(&p, &meta("main", false)), None);
    }
}
