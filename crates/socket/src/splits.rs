use crate::app::App;
use crate::config::KPartTarget;
use crate::daily::uuid_ns;
use crate::db::{POST_COLS, post_from_row};
use crate::model::{Post, ThreadMeta};
use counting::js::parse_int;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone, Copy)]
pub enum Kind {
    Numeric,
    Letters,
    CountNumber { per_split: i64, per_get: i64 },
}

pub fn spawn(app: &Arc<App>, thread: ThreadMeta, post: Post, kind: Kind, target: Option<KPartTarget>) {
    let app = app.clone();
    tokio::spawn(async move {
        match run(&app, &thread, &post, kind).await {
            Ok(Some(outcome)) => deliver(&app, &thread, &post, outcome, target).await,
            Ok(None) => {}
            Err(e) => tracing::warn!("splits for {} in {}: {e:#}", post.uuid, thread.name),
        }
    });
}

struct Outcome {
    emits: Vec<Value>,
    get: Option<Get>,
}

struct Get {
    splits: Vec<i64>,
    reached: String,
    replay: String,
}

pub fn js_round(x: f64) -> i64 {
    (x + 0.5).floor() as i64
}

fn ts(p: &Post) -> f64 {
    p.timestamp.parse().unwrap_or(f64::NAN)
}

fn thread_start(t: &ThreadMeta) -> f64 {
    (uuid_ns(&t.uuid) / 1000) as f64 / 1000.0
}

pub fn replay_path(thread_name: &str, start: Option<i64>, end: Option<i64>) -> String {
    let from = start.filter(|s| *s > 0).unwrap_or(1);
    let end = end.map_or("null".to_owned(), |e| e.to_string());
    format!(
        "/replay/{}?startValidCountNumber={from}&endValidCountNumber={end}",
        encode_uri_component(thread_name)
    )
}

fn encode_uri_component(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

pub fn letters_to_decimal(letters: &str) -> i64 {
    let n = parse_int(letters, None);
    if letters.is_empty() || (n.is_finite() && n != 0.0) {
        return 0;
    }
    letters
        .encode_utf16()
        .fold(0i64, |acc, c| acc * 26 + (c as i64 - 65 + 1))
}

pub fn decimal_to_letters(mut n: i64) -> String {
    let mut out = Vec::new();
    while n > 0 {
        out.push(char::from(b'A' + ((n - 1) % 26) as u8));
        n = (n - 1) / 26;
    }
    out.iter().rev().collect()
}

async fn by_raw(app: &App, t: &ThreadMeta, raw: &str) -> anyhow::Result<Option<Post>> {
    let sql = format!(
        "SELECT {POST_COLS} FROM post WHERE threadId = ? AND rawCount = ? AND isValidCount = 1 ORDER BY id DESC LIMIT 1"
    );
    let row = sqlx::query(&sql).bind(t.id).bind(raw).fetch_optional(&app.pool).await?;
    Ok(row.as_ref().map(post_from_row))
}

async fn last_mistake(app: &App, t: &ThreadMeta) -> anyhow::Result<Option<Post>> {
    if !t.reset_on_mistakes {
        return Ok(None);
    }
    let sql = format!(
        "SELECT {POST_COLS} FROM post WHERE threadId = ? AND isCount = 1 AND isValidCount = 0 ORDER BY id DESC LIMIT 1"
    );
    let row = sqlx::query(&sql).bind(t.id).fetch_optional(&app.pool).await?;
    Ok(row.as_ref().map(post_from_row))
}

async fn by_numbers(app: &App, t: &ThreadMeta, numbers: &[i64]) -> anyhow::Result<HashMap<i64, Post>> {
    let mut out = HashMap::new();
    if numbers.is_empty() {
        return Ok(out);
    }
    let marks = vec!["?"; numbers.len()].join(", ");
    let sql = format!(
        "SELECT {POST_COLS} FROM post WHERE threadId = ? AND isValidCount = 1 AND validCountNumber IN ({marks}) ORDER BY id ASC"
    );
    let mut q = sqlx::query(&sql).bind(t.id);
    for n in numbers {
        q = q.bind(n);
    }
    for row in q.fetch_all(&app.pool).await? {
        let p = post_from_row(&row);
        if let Some(n) = p.valid_count_number {
            out.entry(n).or_insert(p);
        }
    }
    Ok(out)
}

fn split_json(post: &Post, thread: &ThreadMeta, number: String, split: i64, start: Option<i64>) -> Value {
    json!({
        "number": number,
        "split": split,
        "validCountNumber": post.valid_count_number,
        "startValidCountNumber": start,
        "replay": replay_path(&thread.name, start, post.valid_count_number),
    })
}

struct Span {
    first: f64,
    start: Option<Post>,
}

fn get_outcome(thread: &ThreadMeta, post: &Post, span: Span, middle: Vec<Post>, suffix: &str) -> Outcome {
    let mut stamps = vec![span.first];
    let mut splits = Vec::new();
    for p in &middle {
        splits.push(js_round(ts(p) - stamps[stamps.len() - 1]));
        stamps.push(ts(p));
    }
    splits.push(js_round(ts(post) - stamps[stamps.len() - 1]));
    splits.push(js_round(ts(post) - stamps[0]));
    let start_vcn = span.start.as_ref().and_then(|p| p.valid_count_number);
    let previous = middle.last().or(span.start.as_ref()).and_then(|p| p.valid_count_number);
    let raw = post.raw_count.clone().unwrap_or_default();
    let emits = vec![
        split_json(post, thread, raw.clone(), splits[splits.len() - 2], previous),
        split_json(
            post,
            thread,
            format!("{raw} ({suffix})"),
            splits[splits.len() - 1],
            start_vcn,
        ),
    ];
    Outcome {
        emits,
        get: Some(Get {
            splits,
            reached: raw,
            replay: replay_path(&thread.name, start_vcn, post.valid_count_number),
        }),
    }
}

fn single_outcome(thread: &ThreadMeta, post: &Post, span: Span) -> Outcome {
    let from = span.start.as_ref().map_or(span.first, ts);
    let start_vcn = span.start.as_ref().and_then(|p| p.valid_count_number);
    let split = js_round(ts(post) - from);
    Outcome {
        emits: vec![split_json(
            post,
            thread,
            post.raw_count.clone().unwrap_or_default(),
            split,
            start_vcn,
        )],
        get: None,
    }
}

async fn opening(app: &App, t: &ThreadMeta) -> anyhow::Result<Span> {
    let start = last_mistake(app, t).await?;
    let first = start.as_ref().map_or_else(|| thread_start(t), ts);
    Ok(Span { first, start })
}

async fn span_at(app: &App, t: &ThreadMeta, raw: &str) -> anyhow::Result<Option<Span>> {
    Ok(by_raw(app, t, raw).await?.map(|p| Span {
        first: ts(&p),
        start: Some(p),
    }))
}

async fn run(app: &App, t: &ThreadMeta, post: &Post, kind: Kind) -> anyhow::Result<Option<Outcome>> {
    let raw = post.raw_count.clone().unwrap_or_default();
    match kind {
        Kind::Numeric => {
            let n = parse_int(&raw, None);
            if !n.is_finite() {
                return Ok(None);
            }
            let n = n as i64;
            let odd = t.name == "odds" || t.name == "odds_double";
            if n % 1000 == 0 || (odd && n % 1000 == 1) {
                let span = if n - 1000 < 1 {
                    opening(app, t).await?
                } else {
                    let Some(s) = span_at(app, t, &(n - 1000).to_string()).await? else {
                        return Ok(None);
                    };
                    s
                };
                let mut middle = Vec::new();
                for i in 1..10 {
                    let Some(p) = by_raw(app, t, &(n - 1000 + 100 * i).to_string()).await? else {
                        return Ok(None);
                    };
                    middle.push(p);
                }
                Ok(Some(get_outcome(t, post, span, middle, "K")))
            } else {
                let span = if n - 100 < 1 {
                    opening(app, t).await?
                } else {
                    let Some(s) = span_at(app, t, &(n - 100).to_string()).await? else {
                        return Ok(None);
                    };
                    s
                };
                Ok(Some(single_outcome(t, post, span)))
            }
        }
        Kind::Letters => {
            let d = letters_to_decimal(&raw);
            if raw.ends_with("AA") {
                let span = if raw == "AAA" {
                    opening(app, t).await?
                } else {
                    let Some(s) = span_at(app, t, &decimal_to_letters(d - 676)).await? else {
                        return Ok(None);
                    };
                    s
                };
                let mut middle = Vec::new();
                for i in 1..26 {
                    let Some(p) = by_raw(app, t, &decimal_to_letters(d - 676 + 26 * i)).await? else {
                        return Ok(None);
                    };
                    middle.push(p);
                }
                Ok(Some(get_outcome(t, post, span, middle, "K")))
            } else {
                let span = if raw == "AA" {
                    opening(app, t).await?
                } else {
                    let Some(s) = span_at(app, t, &decimal_to_letters(d - 26)).await? else {
                        return Ok(None);
                    };
                    s
                };
                Ok(Some(single_outcome(t, post, span)))
            }
        }
        Kind::CountNumber { per_split, per_get } => {
            if t.reset_on_mistakes {
                return Ok(None);
            }
            let Some(vcn) = post.valid_count_number else {
                return Ok(None);
            };
            let get_number = per_split * per_get;
            if vcn % get_number == 0 {
                let base = vcn - get_number;
                let mut wanted: Vec<i64> = (1..per_get).map(|i| base + per_split * i).collect();
                if base >= 1 {
                    wanted.insert(0, base);
                }
                wanted.retain(|n| *n >= 1);
                let mut found = by_numbers(app, t, &wanted).await?;
                let span = if base < 1 {
                    Span {
                        first: thread_start(t),
                        start: None,
                    }
                } else {
                    let Some(p) = found.remove(&base) else {
                        return Ok(None);
                    };
                    Span {
                        first: ts(&p),
                        start: Some(p),
                    }
                };
                let mut middle = Vec::new();
                for i in 1..per_get {
                    let Some(p) = found.remove(&(base + per_split * i)) else {
                        return Ok(None);
                    };
                    middle.push(p);
                }
                Ok(Some(get_outcome(t, post, span, middle, "Get")))
            } else {
                let span = if vcn - per_split < 1 {
                    Span {
                        first: thread_start(t),
                        start: None,
                    }
                } else {
                    let row = by_numbers(app, t, &[vcn - per_split]).await?.remove(&(vcn - per_split));
                    let Some(p) = row else { return Ok(None) };
                    Span {
                        first: ts(&p),
                        start: Some(p),
                    }
                };
                Ok(Some(single_outcome(t, post, span)))
            }
        }
    }
}

async fn deliver(app: &Arc<App>, t: &ThreadMeta, post: &Post, outcome: Outcome, target: Option<KPartTarget>) {
    let tj = t.to_json();
    for split in &outcome.emits {
        crate::commands::emit_splits(app, &tj, split).await;
    }
    let Some(get) = outcome.get else { return };
    app.add_xp(&post.author_uuid, if t.allow_double_posts { 150 } else { 500 })
        .await;
    if let Some(target) = target {
        app.bot.emit(
            "kpartAlert",
            json!({
                "channelID": target.channel,
                "kPartRole": target.role,
                "title": format!("{} just reached {}", crate::ggbot::thread_title(t), get.reached),
                "threadName": t.name,
                "replay": get.replay,
                "splits": get.splits,
            }),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_round_trip() {
        assert_eq!(letters_to_decimal("A"), 1);
        assert_eq!(letters_to_decimal("AA"), 27);
        assert_eq!(decimal_to_letters(27), "AA");
        assert_eq!(decimal_to_letters(letters_to_decimal("ZZ") + 1), "AAA");
        assert_eq!(letters_to_decimal("12"), 0);
    }

    #[test]
    fn js_rounding_goes_up_at_half() {
        assert_eq!(js_round(2.5), 3);
        assert_eq!(js_round(-2.5), -2);
    }

    #[test]
    fn replay_starts_at_one_without_a_start() {
        assert_eq!(
            replay_path("main thread", None, Some(5000)),
            "/replay/main%20thread?startValidCountNumber=1&endValidCountNumber=5000"
        );
    }
}
