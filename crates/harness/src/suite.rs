use anyhow::{Result, bail, ensure};
use serde::Serialize;
use serde_json::{Value, json};
use sqlx::MySqlPool;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::time::{Duration, Instant};

use crate::client::Client;
use crate::env::{Account, Target, ThreadSpec, seed};
use crate::server::Socket;

const WAIT: Duration = Duration::from_secs(15);
const ACCOUNTS_PER_CASE: usize = 2;

#[derive(Serialize)]
pub struct CaseResult {
    pub name: &'static str,
    pub passed: bool,
    pub ms: u128,
    pub error: Option<String>,
}

struct Ctx {
    socket: Socket,
    target: Target,
    pool: MySqlPool,
    accounts: Vec<Account>,
    first_account: usize,
}

impl Ctx {
    fn account(&self, who: usize) -> &Account {
        &self.accounts[self.first_account + who]
    }

    async fn client(&self, who: usize) -> Result<Client> {
        let acc = self.account(who);
        Ok(Client::connect(self.socket.port, Some(&acc.cookie))
            .await?
            .authored_by(&acc.uuid))
    }

    async fn watching(&self, who: usize, thread: &str) -> Result<Client> {
        let mut c = self.client(who).await?;
        c.watch(thread).await?;
        Ok(c)
    }
}

type Case = for<'a> fn(&'a mut Ctx) -> Pin<Box<dyn Future<Output = Result<()>> + 'a>>;

const THREADS: [ThreadSpec; 12] = [
    ThreadSpec {
        name: "h_order",
        allow_double_posts: false,
    },
    ThreadSpec {
        name: "h_mistake",
        allow_double_posts: false,
    },
    ThreadSpec {
        name: "h_double",
        allow_double_posts: false,
    },
    ThreadSpec {
        name: "h_rapid",
        allow_double_posts: true,
    },
    ThreadSpec {
        name: "h_anon",
        allow_double_posts: true,
    },
    ThreadSpec {
        name: "h_react",
        allow_double_posts: true,
    },
    ThreadSpec {
        name: "h_origin",
        allow_double_posts: true,
    },
    ThreadSpec {
        name: "h_expired",
        allow_double_posts: true,
    },
    ThreadSpec {
        name: "h_restart",
        allow_double_posts: true,
    },
    ThreadSpec {
        name: "h_ggbot",
        allow_double_posts: true,
    },
    ThreadSpec {
        name: "h_moddel",
        allow_double_posts: true,
    },
    ThreadSpec {
        name: "h_split",
        allow_double_posts: true,
    },
];

const CASES: [(&str, Case); 17] = [
    ("counts_alternate_in_order", |c| Box::pin(counts_alternate_in_order(c))),
    ("wrong_count_is_rejected", |c| Box::pin(wrong_count_is_rejected(c))),
    ("double_post_is_rejected", |c| Box::pin(double_post_is_rejected(c))),
    ("rapid_posts_keep_arrival_order", |c| {
        Box::pin(rapid_posts_keep_arrival_order(c))
    }),
    ("anonymous_post_is_ignored", |c| Box::pin(anonymous_post_is_ignored(c))),
    ("reaction_is_broadcast_and_saved", |c| {
        Box::pin(reaction_is_broadcast_and_saved(c))
    }),
    ("cookie_from_another_site_is_ignored", |c| {
        Box::pin(cookie_from_another_site_is_ignored(c))
    }),
    ("expired_session_is_ignored", |c| {
        Box::pin(expired_session_is_ignored(c))
    }),
    ("connections_per_ip_are_capped", |c| {
        Box::pin(connections_per_ip_are_capped(c))
    }),
    ("repeated_chat_watch_broadcasts_once", |c| {
        Box::pin(repeated_chat_watch_broadcasts_once(c))
    }),
    ("reddit_post_is_not_relayed", |c| {
        Box::pin(reddit_post_is_not_relayed(c))
    }),
    ("crash_recovers_from_wal", |c| Box::pin(crash_recovers_from_wal(c))),
    ("connection_report_names_a_slow_upload", |c| {
        Box::pin(connection_report_names_a_slow_upload(c))
    }),
    ("ggbot_answers_in_the_thread", |c| {
        Box::pin(ggbot_answers_in_the_thread(c))
    }),
    ("moderator_deletes_a_comment", |c| {
        Box::pin(moderator_deletes_a_comment(c))
    }),
    ("chosen_shuffle_seed_pays_no_xp", |c| {
        Box::pin(chosen_shuffle_seed_pays_no_xp(c))
    }),
    ("split_lands_at_a_hundred", |c| Box::pin(split_lands_at_a_hundred(c))),
];

pub async fn run(bin: &Path, target: &Target, workdir: &Path, only: Option<&str>) -> Result<Vec<CaseResult>> {
    target.reset().await?;
    let pool = target.pool().await?;
    let accounts = seed(&pool, target, &THREADS, ACCOUNTS_PER_CASE * CASES.len()).await?;
    let socket = Socket::start(bin, target, workdir, true).await?;
    let mut ctx = Ctx {
        socket,
        target: target.clone(),
        pool,
        accounts,
        first_account: 0,
    };
    let mut results = Vec::new();
    for (i, (name, case)) in CASES
        .iter()
        .enumerate()
        .filter(|(_, (n, _))| only.is_none_or(|o| n.contains(o)))
    {
        ctx.first_account = i * ACCOUNTS_PER_CASE;
        let started = Instant::now();
        let outcome = case(&mut ctx).await;
        let r = CaseResult {
            name,
            passed: outcome.is_ok(),
            ms: started.elapsed().as_millis(),
            error: outcome.err().map(|e| format!("{e:#}")),
        };
        println!(
            "{} {name} ({} ms){}",
            if r.passed { "PASS" } else { "FAIL" },
            r.ms,
            r.error.as_deref().map(|e| format!(": {e}")).unwrap_or_default()
        );
        results.push(r);
    }
    Ok(results)
}

fn verdict(e: &crate::client::Event) -> (bool, Option<i64>) {
    let p = &e.data["post"];
    (
        p["isValidCount"].as_bool().unwrap_or(false),
        p["validCountNumber"].as_i64(),
    )
}

async fn post_and_see(c: &mut Client, thread: &str, text: &str) -> Result<(bool, Option<i64>)> {
    c.post(thread, text)?;
    Ok(verdict(&c.own_post(text, WAIT).await?))
}

async fn counts_alternate_in_order(ctx: &mut Ctx) -> Result<()> {
    let mut a = ctx.watching(0, "h_order").await?;
    let mut b = ctx.watching(1, "h_order").await?;
    for n in 1..=40i64 {
        let poster = if n % 2 == 1 { &mut a } else { &mut b };
        let seen = post_and_see(poster, "h_order", &n.to_string()).await?;
        ensure!(seen == (true, Some(n)), "count {n} came back as {seen:?}");
    }
    Ok(())
}

async fn wrong_count_is_rejected(ctx: &mut Ctx) -> Result<()> {
    let mut a = ctx.watching(0, "h_mistake").await?;
    let mut b = ctx.watching(1, "h_mistake").await?;
    ensure!(post_and_see(&mut a, "h_mistake", "1").await?.0, "1 should be valid");
    ensure!(
        !post_and_see(&mut b, "h_mistake", "3").await?.0,
        "3 after 1 should be invalid"
    );
    let seen = post_and_see(&mut b, "h_mistake", "2").await?;
    ensure!(
        seen == (true, Some(2)),
        "2 should still be the next count, came back as {seen:?}"
    );
    Ok(())
}

async fn double_post_is_rejected(ctx: &mut Ctx) -> Result<()> {
    let mut a = ctx.watching(0, "h_double").await?;
    let mut b = ctx.watching(1, "h_double").await?;
    ensure!(post_and_see(&mut a, "h_double", "1").await?.0, "1 should be valid");
    ensure!(
        !post_and_see(&mut a, "h_double", "2").await?.0,
        "a second post in a row should be invalid"
    );
    let seen = post_and_see(&mut b, "h_double", "2").await?;
    ensure!(
        seen == (true, Some(2)),
        "another counter's 2 should be valid, came back as {seen:?}"
    );
    Ok(())
}

async fn rapid_posts_keep_arrival_order(ctx: &mut Ctx) -> Result<()> {
    let mut a = ctx.watching(0, "h_rapid").await?;
    for n in 1..=20 {
        a.post("h_rapid", &n.to_string())?;
    }
    for n in 1..=20i64 {
        let e = a
            .wait_for(WAIT, |e| e.name == "post")
            .await
            .map_err(|e| e.context(format!("waiting for post {n} of 20")))?;
        let text = e.data["post"]["rawText"].as_str().unwrap_or_default().to_owned();
        ensure!(text == n.to_string(), "expected {n}, got {text}");
        ensure!(
            verdict(&e) == (true, Some(n)),
            "count {n} came back as {:?}",
            verdict(&e)
        );
    }
    Ok(())
}

async fn anonymous_post_is_ignored(ctx: &mut Ctx) -> Result<()> {
    let mut observer = ctx.watching(0, "h_anon").await?;
    let mut anon = Client::connect(ctx.socket.port, None).await?;
    anon.watch("h_anon").await?;
    anon.post("h_anon", "1")?;
    observer.quiet_for(Duration::from_secs(1), |e| e.name == "post").await
}

async fn reaction_is_broadcast_and_saved(ctx: &mut Ctx) -> Result<()> {
    let mut a = ctx.watching(0, "h_react").await?;
    let b = ctx.watching(1, "h_react").await?;
    a.post("h_react", "1")?;
    let posted = a.own_post("1", WAIT).await?;
    let post_uuid = posted.data["post"]["uuid"].as_str().unwrap_or_default().to_owned();
    b.emit("updateReactions", json!({ "post_uuid": post_uuid, "id": "smile" }))?;
    let reactor = ctx.account(1).uuid.clone();
    let update = a
        .wait_for(WAIT, |e| {
            e.name == "updateReaction" && e.data["post_uuid"] == post_uuid.as_str()
        })
        .await?;
    ensure!(
        update.data["reactions"]["smile"][0] == reactor.as_str(),
        "broadcast reactions were {}",
        update.data["reactions"]
    );
    poll(Duration::from_secs(10), || async {
        let saved: Option<String> = sqlx::query_scalar("SELECT CAST(reactions AS CHAR) FROM post WHERE uuid = ?")
            .bind(&post_uuid)
            .fetch_optional(&ctx.pool)
            .await?;
        Ok(saved.is_some_and(|r| r.contains("smile") && r.contains(&reactor)))
    })
    .await
}

async fn cookie_from_another_site_is_ignored(ctx: &mut Ctx) -> Result<()> {
    let mut observer = ctx.watching(0, "h_origin").await?;
    let cookie = ctx.account(1).cookie.clone();
    let author = ctx.account(1).uuid.clone();

    let mut foreign = Client::connect_with(
        ctx.socket.port,
        &[("cookie", &cookie), ("origin", "https://attacker.example")],
    )
    .await?;
    foreign.watch("h_origin").await?;
    foreign.post("h_origin", "1")?;
    observer.quiet_for(Duration::from_secs(1), |e| e.name == "post").await?;

    let mut own = Client::connect_with(
        ctx.socket.port,
        &[("cookie", &cookie), ("origin", crate::env::SITE_ORIGIN)],
    )
    .await?
    .authored_by(&author);
    own.watch("h_origin").await?;
    own.post("h_origin", "1")?;
    own.own_post("1", WAIT).await?;
    Ok(())
}

async fn expired_session_is_ignored(ctx: &mut Ctx) -> Result<()> {
    let mut observer = ctx.watching(0, "h_expired").await?;
    let uuid = ctx.account(1).uuid.clone();
    let cookie = crate::env::expired_session_cookie(&ctx.pool, &ctx.target, &uuid).await?;
    let mut stale = Client::connect(ctx.socket.port, Some(&cookie)).await?;
    stale.watch("h_expired").await?;
    stale.post("h_expired", "1")?;
    observer.quiet_for(Duration::from_secs(1), |e| e.name == "post").await
}

async fn connections_per_ip_are_capped(ctx: &mut Ctx) -> Result<()> {
    const CAP: usize = 300;
    let ip = [("x-forwarded-for", "203.0.113.7")];
    let mut open = Vec::with_capacity(CAP);
    for n in 0..CAP {
        open.push(
            Client::connect_with(ctx.socket.port, &ip)
                .await
                .map_err(|e| e.context(format!("connection {} of {CAP}", n + 1)))?,
        );
    }
    ensure!(
        Client::connect_with(ctx.socket.port, &ip).await.is_err(),
        "connection {} from one IP was accepted",
        CAP + 1
    );
    ensure!(
        Client::connect_with(ctx.socket.port, &[("x-forwarded-for", "203.0.113.8")])
            .await
            .is_ok(),
        "another IP was refused"
    );
    open.pop();
    poll(Duration::from_secs(5), || async {
        Ok(Client::connect_with(ctx.socket.port, &ip).await.is_ok())
    })
    .await
    .map_err(|e| e.context("a closed connection did not free its slot"))
}

async fn repeated_chat_watch_broadcasts_once(ctx: &mut Ctx) -> Result<()> {
    let mut listener = ctx.client(0).await?;
    listener.emit("chat_watch", Value::Null)?;
    listener.wait_for(WAIT, |e| e.name == "chat_watcher_count").await?;
    let spammer = ctx.client(1).await?;
    for _ in 0..50 {
        spammer.emit("chat_watch", Value::Null)?;
    }
    listener.wait_for(WAIT, |e| e.name == "chat_watcher_count").await?;
    listener
        .quiet_for(Duration::from_secs(1), |e| e.name == "chat_watcher_count")
        .await
}

async fn reddit_post_is_not_relayed(ctx: &mut Ctx) -> Result<()> {
    let mut viewer = Client::connect(ctx.socket.port, None).await?;
    viewer.emit("reddit_watch", json!("harness"))?;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let anon = Client::connect(ctx.socket.port, None).await?;
    anon.emit("reddit_post", json!("harness"))?;
    viewer.quiet_for(Duration::from_secs(1), |e| e.name == "post").await
}

async fn crash_recovers_from_wal(ctx: &mut Ctx) -> Result<()> {
    let mut a = ctx.watching(0, "h_restart").await?;
    for n in 1..=10 {
        post_and_see(&mut a, "h_restart", &n.to_string()).await?;
    }
    drop(a);
    ctx.socket.kill_and_restart().await?;
    let mut a = ctx.watching(0, "h_restart").await?;
    let seen = post_and_see(&mut a, "h_restart", "11").await?;
    ensure!(seen == (true, Some(11)), "after a crash, 11 came back as {seen:?}");
    let stored = || async {
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM post p JOIN thread t ON t.uuid = p.thread WHERE t.name = 'h_restart'",
        )
        .fetch_one(&ctx.pool)
        .await
    };
    if poll(Duration::from_secs(15), || async { Ok(stored().await? == 11) })
        .await
        .is_err()
    {
        bail!("{} of 11 posts reached MySQL", stored().await?);
    }
    Ok(())
}

async fn poll<F, Fut>(timeout: Duration, mut check: F) -> Result<()>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<bool>>,
{
    let started = Instant::now();
    while started.elapsed() < timeout {
        if check().await? {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    bail!("condition not met within {timeout:?}")
}

pub fn summary(results: &[CaseResult]) -> Value {
    json!({ "passed": results.iter().all(|r| r.passed), "cases": results })
}

fn spread(p50: u64, p95: u64, max: u64) -> Value {
    json!({ "p50": p50, "p95": p95, "max": max })
}

fn latency_report(round_trip: u64, uplink: u64, downlink: u64) -> Value {
    json!({
        "t": 0, "thread": "h_conn", "transport": "websocket", "n": 20,
        "roundTrip": spread(round_trip / 2, round_trip, round_trip + 10),
        "processing": spread(3, 5, 8),
        "unaccounted": spread(round_trip / 2, round_trip, round_trip + 10),
        "bands": { "scheduling": 0, "dbRead": 0, "dbWrite": 0, "other": 0 },
        "ping": spread(60, uplink.max(80), uplink.max(80) + 5),
        "blockedMs": 0, "longtask": true,
        "uplink": spread(40, uplink, uplink + 5),
        "downlink": spread(40, downlink, downlink + 5),
        "loopLag": spread(1, 4, 9), "paint": spread(8, 16, 20),
    })
}

async fn report_from(c: &mut Client, timeout: Duration) -> Result<Value> {
    Ok(c.wait_for(timeout, |e| e.name == "connection_report").await?.data)
}

async fn connection_report_names_a_slow_upload(ctx: &mut Ctx) -> Result<()> {
    let staff_uuid = ctx.account(1).uuid.clone();
    sqlx::query("UPDATE counters SET roles = '[\"counter\",\"admin\"]' WHERE uuid = ?")
        .bind(&staff_uuid)
        .execute(&ctx.pool)
        .await?;
    let target_uuid = ctx.account(0).uuid.clone();
    let target_name: String = sqlx::query_scalar("SELECT username FROM counters WHERE uuid = ?")
        .bind(&target_uuid)
        .fetch_one(&ctx.pool)
        .await?;
    let mut crowd = vec![ctx.client(1).await?];
    for _ in 0..3 {
        crowd.push(Client::connect(ctx.socket.port, None).await?);
    }
    for c in &crowd {
        c.emit("client_latency", latency_report(120, 60, 50))?;
    }
    let mut target = ctx.client(0).await?;
    for _ in 0..3 {
        target.emit("client_latency", latency_report(4500, 4400, 90))?;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    target.emit("connection_check", json!({ "minutes": 30 }))?;
    let own = report_from(&mut target, WAIT).await?;
    ensure!(own["verdict"] == "upload", "own verdict was {}", own["verdict"]);
    ensure!(own["target"]["self"] == true, "target was {}", own["target"]);
    let proof = &own["proof"]["evidence"][0];
    ensure!(
        proof[0] == "upload" && proof[1].as_f64() == Some(4400.0) && proof[2].as_f64().is_some_and(|o| o <= 60.0),
        "proof was {proof}"
    );
    let mut nosy = ctx.client(0).await?;
    nosy.emit("connection_check", json!({ "username": "harness_anyone" }))?;
    let refused = report_from(&mut nosy, WAIT).await?;
    ensure!(
        refused["error"] == "forbidden",
        "a counter looking up someone else got {refused}"
    );
    let mut staff = ctx.client(1).await?;
    staff.emit("connection_check", json!({ "username": target_name, "minutes": 30 }))?;
    let looked_up = report_from(&mut staff, WAIT).await?;
    ensure!(
        looked_up["verdict"] == "upload" && looked_up["target"]["uuid"] == target_uuid.as_str(),
        "staff lookup was {} for {}",
        looked_up["verdict"],
        looked_up["target"]
    );
    Ok(())
}

async fn ggbot_answers_in_the_thread(ctx: &mut Ctx) -> Result<()> {
    let mut a = ctx.watching(0, "h_ggbot").await?;
    a.post("h_ggbot", "1 ggbot help")?;
    let reply = a
        .wait_for(WAIT, |e| e.name == "post" && e.data["post"]["authorUUID"] == "[SYSTEM]")
        .await?;
    let text = reply.data["post"]["rawText"].as_str().unwrap_or_default();
    ensure!(text.starts_with("Here are my commands"), "bot said {text}");
    Ok(())
}

async fn moderator_deletes_a_comment(ctx: &mut Ctx) -> Result<()> {
    let mod_uuid = ctx.account(1).uuid.clone();
    sqlx::query("UPDATE counters SET roles = '[\"counter\",\"mod\"]' WHERE uuid = ?")
        .bind(&mod_uuid)
        .execute(&ctx.pool)
        .await?;
    let mut a = ctx.watching(0, "h_moddel").await?;
    let m = ctx.watching(1, "h_moddel").await?;
    a.post("h_moddel", "1 hello there")?;
    let posted = a.own_post("1 hello there", WAIT).await?;
    let uuid = posted.data["post"]["uuid"].as_str().unwrap_or_default().to_owned();
    m.emit("deleteComment", json!({ "uuid": uuid }))?;
    a.wait_for(WAIT, |e| e.name == "deleteComment" && e.data["uuid"] == uuid.as_str())
        .await?;
    Ok(())
}

async fn split_lands_at_a_hundred(ctx: &mut Ctx) -> Result<()> {
    let mut a = ctx.watching(0, "h_split").await?;
    for n in 1..=100 {
        a.post("h_split", &n.to_string())?;
        a.own_post(&n.to_string(), WAIT).await?;
        tokio::time::sleep(Duration::from_millis(45)).await;
    }
    let split = a.wait_for(WAIT, |e| e.name == "split").await?;
    ensure!(split.data["number"] == "100", "split was {}", split.data);
    ensure!(split.data["validCountNumber"] == 100, "split was {}", split.data);
    Ok(())
}

async fn xp_of(ctx: &Ctx, uuid: &str) -> Result<i64> {
    Ok(
        sqlx::query_scalar("SELECT CAST(xp AS SIGNED) FROM counters WHERE uuid = ?")
            .bind(uuid)
            .fetch_one(&ctx.pool)
            .await?,
    )
}

fn shuffle_spots(seed: &str, size: usize) -> Vec<usize> {
    let mut rng = counting::seedrandom::SeedRandom::new(seed);
    let numbers: Vec<i64> = (0..size)
        .map(|_| (rng.next_f64() * 1000.0).floor() as i64 + 1)
        .collect();
    numbers
        .iter()
        .enumerate()
        .map(|(i, n)| numbers.iter().filter(|m| *m < n).count() + numbers[..i].iter().filter(|m| *m == n).count())
        .collect()
}

async fn win_shuffle(ctx: &Ctx, client: &mut Client, start: Value) -> Result<String> {
    client.emit("number_shuffle", start)?;
    let game = client.wait_for(WAIT, |e| e.name == "number_shuffle").await?;
    let seed_id = game.data["seed_id"].as_str().unwrap_or_default().to_owned();
    let seed: String = sqlx::query_scalar("SELECT seed FROM numbershuffle WHERE fakeSeed = ? ORDER BY id DESC LIMIT 1")
        .bind(&seed_id)
        .fetch_one(&ctx.pool)
        .await?;
    let mut status = String::new();
    for spot in shuffle_spots(&seed, 20) {
        tokio::time::sleep(Duration::from_millis(80)).await;
        client.emit("number_shuffle", json!({ "newMoveSpot": spot, "seed_id": seed_id }))?;
        let e = client.wait_for(WAIT, |e| e.name == "number_shuffle").await?;
        status = e.data["gameStatus"].as_str().unwrap_or_default().to_owned();
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    Ok(status)
}

async fn chosen_shuffle_seed_pays_no_xp(ctx: &mut Ctx) -> Result<()> {
    let uuid = ctx.account(0).uuid.clone();
    let mut a = ctx.client(0).await?;
    let before = xp_of(ctx, &uuid).await?;
    let status = win_shuffle(ctx, &mut a, json!({ "isDaily": false, "size": 20 })).await?;
    ensure!(status == "won", "server-seeded game ended {status}");
    let after_server_seed = xp_of(ctx, &uuid).await?;
    ensure!(
        after_server_seed - before == 20000,
        "a server-seeded win paid {}",
        after_server_seed - before
    );
    let chosen = json!({ "isDaily": false, "size": 20, "seed": format!("harness{}", fastrand::u64(..)) });
    let status = win_shuffle(ctx, &mut a, chosen).await?;
    ensure!(status == "won", "chosen-seed game ended {status}");
    let after_chosen = xp_of(ctx, &uuid).await?;
    ensure!(
        after_chosen == after_server_seed,
        "a chosen-seed win paid {}",
        after_chosen - after_server_seed
    );
    Ok(())
}
