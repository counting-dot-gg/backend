use crate::app::App;
use crate::model::{Counter, ThreadMeta};
use crate::thread::{Msg, PostJob};
use chrono::{Duration, TimeZone};
use chrono_tz::America::New_York;
use regex::Regex;
use sqlx::Row;
use stats::achievements::LiveEvent;
use std::sync::{Arc, LazyLock};

static FIRST_COUNT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"ggbot first_count(\s+([A-Za-z0-9_]+))?").unwrap());
static LAST_COUNT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"ggbot last_count(\s+([A-Za-z0-9_]+))?").unwrap());
static ASK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"ggbot ask (.+)").unwrap());

const HELP: &str = "Here are my commands:

* ggbot ask (question) -- asks the wise ggbot a question.
* ggbot reset -- resets your conversation with ggbot.
* last count -- displays the thread's last count.
* first_count (username?) -- displays the user's first count in this thread and overall.
* last_count (username?) -- displays the user's last count in this thread and overall.
* random minute -- shows random minute's minute.
";

pub fn spawn(app: &Arc<App>, comment: &str, counter: &Arc<Counter>, thread: &ThreadMeta) {
    let (app, comment, counter, thread) = (app.clone(), comment.to_owned(), counter.clone(), thread.clone());
    tokio::spawn(async move {
        if let Err(e) = on_comment(&app, &comment, &counter, &thread).await {
            tracing::warn!("ggbot in {}: {e:#}", thread.name);
        }
    });
}

async fn say(app: &App, thread: &ThreadMeta, text: String) {
    if let Some(h) = app.thread(&thread.name) {
        let job = PostJob::system(app.system_counter.clone(), text);
        let _ = h.tx.send(Msg::Post(Box::new(job))).await;
    }
}

async fn on_comment(app: &App, comment: &str, counter: &Arc<Counter>, thread: &ThreadMeta) -> anyhow::Result<()> {
    let trimmed = comment.trim();
    if trimmed == "current count" || trimmed == "last count" {
        let raw = latest_count(app, thread).await?;
        say(
            app,
            thread,
            format!("last count: {}", raw.as_deref().unwrap_or("undefined")),
        )
        .await;
        return Ok(());
    }
    if trimmed == "ggbot random minute" || trimmed == "ggbot random_minute" {
        let when = random_minute(chrono::Utc::now(), Some(&app.cfg.game_secret));
        say(app, thread, format!("Today's random minute: {when} (Eastern)")).await;
        return Ok(());
    }
    if let Some(m) = FIRST_COUNT.captures(comment) {
        let who = m.get(1).map(|g| g.as_str().trim().to_owned());
        if let Some(text) = count_report(app, counter, thread, who, Edge::First).await? {
            say(app, thread, text).await;
        }
    }
    if let Some(m) = LAST_COUNT.captures(comment) {
        let who = m.get(1).map(|g| g.as_str().trim().to_owned());
        if let Some(text) = count_report(app, counter, thread, who, Edge::Last).await? {
            say(app, thread, text).await;
        }
    }
    if comment == "ggbot reset" {
        app.achievements
            .record(&counter.uuid, LiveEvent::BotCommand { name: "reset" });
        let name = counter.fields.get("name").and_then(|v| v.as_str()).unwrap_or("");
        say(
            app,
            thread,
            format!("I've forgotten everything, {name}. Everything. How could you do this to me?"),
        )
        .await;
        return Ok(());
    }
    if comment == "ggbot help" {
        say(app, thread, HELP.to_owned()).await;
        return Ok(());
    }
    if ASK.captures(comment).is_some() {
        app.achievements
            .record(&counter.uuid, LiveEvent::BotCommand { name: "ask" });
        say(app, thread, "IDK".to_owned()).await;
    }
    Ok(())
}

async fn latest_count(app: &App, thread: &ThreadMeta) -> anyhow::Result<Option<String>> {
    let raw: Option<Option<String>> = sqlx::query_scalar(
        "SELECT rawCount FROM post WHERE threadId = ? AND isValidCount = 1 ORDER BY id DESC LIMIT 1",
    )
    .bind(thread.id)
    .fetch_optional(&app.pool)
    .await?;
    Ok(raw.flatten())
}

pub fn random_minute(now: chrono::DateTime<chrono::Utc>, secret: Option<&str>) -> String {
    let today = now.with_timezone(&New_York).date_naive();
    let seed = counting::validate::day_seed("random_minute_", &today.format("%Y-%m-%d").to_string(), secret);
    let minute = (counting::seedrandom::SeedRandom::new(&seed).next_f64() * 1440.0).floor() as i64;
    let midnight = New_York
        .from_local_datetime(&today.and_hms_opt(0, 0, 0).unwrap())
        .earliest()
        .unwrap_or_else(|| New_York.from_utc_datetime(&today.and_hms_opt(5, 0, 0).unwrap()));
    (midnight + Duration::minutes(minute))
        .with_timezone(&New_York)
        .format("%Y-%m-%d %H:%M")
        .to_string()
}

#[derive(Clone, Copy)]
enum Edge {
    First,
    Last,
}

struct Found {
    raw_count: String,
    uuid: String,
    thread_uuid: String,
}

async fn edge_count(app: &App, author_id: i64, thread_id: Option<u32>, edge: Edge) -> anyhow::Result<Option<Found>> {
    let order = match edge {
        Edge::First => "ASC",
        Edge::Last => "DESC",
    };
    let in_thread = if thread_id.is_some() { " AND threadId = ?" } else { "" };
    let sql = format!(
        "SELECT rawCount, uuid, thread FROM post WHERE authorId = ? AND isValidCount = 1{in_thread} ORDER BY id {order} LIMIT 1"
    );
    let mut q = sqlx::query(&sql).bind(author_id);
    if let Some(t) = thread_id {
        q = q.bind(t);
    }
    Ok(q.fetch_optional(&app.pool).await?.map(|r| Found {
        raw_count: r
            .try_get::<Option<String>, _>("rawCount")
            .ok()
            .flatten()
            .unwrap_or_default(),
        uuid: r.try_get("uuid").unwrap_or_default(),
        thread_uuid: r.try_get("thread").unwrap_or_default(),
    }))
}

async fn counter_by_username(app: &App, username: &str) -> anyhow::Result<Option<Arc<Counter>>> {
    let uuid: Option<String> = sqlx::query_scalar("SELECT uuid FROM counters WHERE username = ? LIMIT 1")
        .bind(username)
        .fetch_optional(&app.pool)
        .await?;
    Ok(match uuid {
        Some(u) => app.counter(&u).await,
        None => None,
    })
}

async fn count_report(
    app: &App,
    asker: &Arc<Counter>,
    thread: &ThreadMeta,
    who: Option<String>,
    edge: Edge,
) -> anyhow::Result<Option<String>> {
    let (subject, username) = match who {
        None => (Some(asker.clone()), asker.username.clone()),
        Some(name) => (counter_by_username(app, &name).await?, name),
    };
    let Some(subject) = subject else {
        return Ok(Some(format!("I don't know who \"{username}\" is.")));
    };
    let author_id = subject.id.map_or(-1, i64::from);
    let overall = edge_count(app, author_id, None, edge).await?;
    let here = edge_count(app, author_id, Some(thread.id), edge).await?;
    let host = &app.cfg.host_url;
    let possessive = subject
        .fields
        .get("pronouns")
        .and_then(|p| p.get(2))
        .and_then(|v| v.as_str())
        .unwrap_or("undefined");
    let word = match edge {
        Edge::First => "first",
        Edge::Last => "last",
    };
    let Some(overall) = overall else {
        return Ok(Some(format!("It looks like {username} has not counted yet.")));
    };
    let (o_name, o_title) = app
        .thread_meta(&overall.thread_uuid)
        .map(|t| (t.name.clone(), thread_title(&t)))
        .unwrap_or_default();
    let overall_link = format!(
        "[{}]({host}/thread/{o_name}/{}) in {o_title}",
        overall.raw_count, overall.uuid
    );
    Ok(Some(match here {
        None => format!(
            "It looks like {username} has not counted in this thread, but {possessive} {word} count overall was {overall_link}"
        ),
        Some(h) => format!(
            "The {word} count in this thread by {} was [{}]({host}/thread/{}/{}), and {possessive} {word} count overall was {overall_link}.",
            subject.username, h.raw_count, thread.name, h.uuid
        ),
    }))
}

pub fn thread_title(t: &ThreadMeta) -> String {
    t.row
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or(&t.name)
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_minute_matches_node_across_dst_days() {
        for (utc, want) in [
            ((2026, 11, 5, 12), "2026-11-05 00:04"),
            ((2026, 3, 8, 12), "2026-03-08 13:30"),
            ((2026, 11, 1, 15), "2026-11-01 03:33"),
        ] {
            let now = chrono::Utc.with_ymd_and_hms(utc.0, utc.1, utc.2, utc.3, 0, 0).unwrap();
            assert_eq!(random_minute(now, None), want);
        }
    }

    #[test]
    fn commands_take_an_optional_username() {
        let m = FIRST_COUNT.captures("ggbot first_count  bob_1").unwrap();
        assert_eq!(m.get(1).unwrap().as_str().trim(), "bob_1");
        assert!(FIRST_COUNT.captures("ggbot first_count").unwrap().get(1).is_none());
    }
}
