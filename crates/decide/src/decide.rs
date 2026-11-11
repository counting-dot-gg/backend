use crate::ids::{timestamp_for, uuid_for};
use crate::model::{Counter, Post, ThreadMeta};
use counting::js::{self, len16, parse_float};
use counting::{Env, Input, ThreadRules, ValidationType, parse_input, validate};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

pub fn author_key(uuid: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    uuid.hash(&mut h);
    h.finish()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Book {
    pub last_valid: Option<Arc<Post>>,
    pub recent_valid: VecDeque<Arc<Post>>,
    pub last_post: Option<Arc<Post>>,
    pub streak: u64,
    pub slow_last: HashMap<u64, f64>,
}

pub struct Job<'a> {
    pub counter: &'a Counter,
    pub text: &'a str,
    pub system: bool,
    pub ns: u64,
    pub macro_preset_id: Option<i64>,
    pub macro_preset_version_id: Option<i64>,
}

pub struct Dice<'a> {
    pub now_ms: f64,
    pub server_tz: chrono_tz::Tz,
    pub secret: Option<&'a str>,
    pub random: &'a mut dyn FnMut() -> f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BanList {
    Count,
    Post,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ban {
    pub list: BanList,
    pub uuid: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decision {
    pub post: Post,
    pub roll: Option<f64>,
    pub chance: Option<f64>,
    pub bans: Vec<Ban>,
    pub bang: bool,
}

pub fn rejection(meta: &ThreadMeta, c: &Counter, text: &str) -> Option<&'static str> {
    if meta.locked {
        Some("locked")
    } else if meta.post_bans.contains(&c.uuid) || c.has_role("banned") {
        Some("banned")
    } else if !(1..=4096).contains(&len16(js::trim(text))) {
        Some("length")
    } else if !meta.may_post(c) {
        Some("not_allowed")
    } else {
        None
    }
}

pub fn judge(meta: &ThreadMeta, book: &Book, job: &Job, dice: &mut Dice) -> Result<Decision, &'static str> {
    let c = job.counter;
    if !job.system
        && let Some(reason) = rejection(meta, c, job.text)
    {
        return Err(reason);
    }
    let vt = ValidationType::from_name(&meta.validation_type);
    let is_bot = c.has_role("bot");
    let mut input = js::trim(job.text).to_owned();
    let parsed = parse_input(&input, vt, is_bot, meta.auto_validated);
    let uuid = uuid_for(job.ns, (dice.random)());
    let timestamp = timestamp_for(job.ns);
    let ts = parse_float(&timestamp);
    let since = |p: &Option<Arc<Post>>| p.as_ref().map_or(0.0, |p| ts - parse_float(&p.timestamp));
    let tslc = since(&book.last_valid);
    let tslp = since(&book.last_post);

    let mut comment = parsed.comment.clone();
    let mut has_comment = comment.as_deref().is_some_and(|s| !js::trim(s).is_empty());
    let mut comment_deleted = false;
    if c.has_role("muted") && has_comment {
        comment = None;
        comment_deleted = true;
        has_comment = false;
        input = parsed.count_content.clone().unwrap_or_default();
    }

    let me = author_key(&c.uuid);
    let prev = |p: &Arc<Post>| p.as_prev(author_key(&p.author_uuid));
    let last_prev = book.last_valid.as_ref().map(prev);
    let recent: Vec<counting::PrevCount> = book.recent_valid.iter().take(2).map(prev).collect();
    let within_hour = vt == ValidationType::Slow
        && book
            .slow_last
            .get(&me)
            .is_some_and(|&t| t > validate::timestamp_minus_ms(ts, 3_600_000.0) && t < ts);
    let rules = ThreadRules {
        vt,
        first_valid_count: &meta.first_valid_count,
        allow_double_posts: meta.allow_double_posts,
        reset_on_mistakes: meta.reset_on_mistakes,
    };
    let vin = Input {
        author: me,
        is_bot,
        username_len16: len16(&c.username),
        count_banned: meta.count_bans.contains(&c.uuid),
        raw_count: parsed.raw_count.as_deref(),
        last_valid: last_prev.as_ref(),
        recent_valid: [recent.first(), recent.get(1)],
        post_timestamp: ts,
        time_since_last_count: tslc,
        author_counted_within_hour: within_hour,
        newest_valid_count_number: book.recent_valid.front().map(|p| p.valid_count_number),
    };
    let v = {
        let mut env = Env {
            now_ms: dice.now_ms,
            server_tz: dice.server_tz,
            random: &mut *dice.random,
            secret: dice.secret,
        };
        validate(&rules, &vin, &mut env)
    };
    let is_valid = v.is_valid;
    let stricken = !is_valid && parsed.is_count && meta.auto_validated;
    let three = comment.as_deref().is_some_and(|s| len16(js::trim(s)) >= 3);
    let post = Post {
        uuid,
        timestamp,
        time_since_last_count: js::num_to_string(tslc),
        time_since_last_post: js::num_to_string(tslp),
        raw_text: js::trim(&input).to_owned(),
        is_count: parsed.is_count,
        is_valid_count: is_valid,
        valid_count_number: v.valid_count_number,
        count_content: parsed.count_content.clone(),
        raw_count: parsed.raw_count.clone(),
        stricken,
        thread: meta.uuid.clone(),
        thread_id: meta.id,
        has_comment,
        has_three_char_comment: three,
        comment,
        author_uuid: c.uuid.clone(),
        author_id: c.id,
        is_deleted: false,
        is_comment_deleted: comment_deleted,
        reactions: serde_json::json!({}),
        macro_preset_id: job.macro_preset_id,
        macro_preset_version_id: job.macro_preset_version_id,
        id: 0,
    };
    let (mut roll, mut chance) = match v.roll.filter(|_| post.is_count) {
        Some((r, c)) => (Some(r), Some(c)),
        None => (None, None),
    };
    let mut bans = Vec::new();
    let mut bang = false;
    let ban = |list| Ban {
        list,
        uuid: c.uuid.clone(),
    };
    match meta.name.as_str() {
        "tug_of_war_avoid_0" if is_valid && post.raw_count.as_deref() == Some("0") => bans.push(ban(BanList::Count)),
        "russian_roulette" if is_valid => {
            let (odds, r) = (1.0 / 10000.0, (dice.random)());
            chance = Some(odds);
            roll = Some(r);
            if r < odds {
                bans.push(ban(BanList::Post));
                bang = true;
            }
        }
        "yoco" if is_valid => bans.push(ban(BanList::Post)),
        _ => {}
    }
    if stricken
        && meta.reset_on_mistakes
        && matches!(meta.name.as_str(), "no_mistakes_or_ban" | "no_mistakes_or_ban_double")
    {
        bans.push(ban(BanList::Post));
    }
    Ok(Decision {
        post,
        roll,
        chance,
        bans,
        bang,
    })
}

pub fn apply(meta: &mut ThreadMeta, book: &mut Book, post: &Arc<Post>, bans: &[Ban]) -> bool {
    if post.stricken {
        book.streak = 0;
    }
    if post.is_valid_count {
        book.last_valid = Some(post.clone());
        book.recent_valid.push_front(post.clone());
        book.recent_valid.truncate(3);
        book.streak += 1;
        if ValidationType::from_name(&meta.validation_type) == ValidationType::Slow {
            book.slow_last
                .insert(author_key(&post.author_uuid), parse_float(&post.timestamp));
        }
    }
    if post.stricken && meta.reset_on_mistakes {
        book.last_valid = None;
    }
    book.last_post = Some(post.clone());
    for b in bans {
        match b.list {
            BanList::Count => meta.count_bans.push(b.uuid.clone()),
            BanList::Post => meta.post_bans.push(b.uuid.clone()),
        }
    }
    !bans.is_empty()
}
