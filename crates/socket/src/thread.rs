use crate::ids;
use crate::metrics::Metrics;
use crate::model::{BroadcastPost, Counter, CounterRef, Post, PostExtras, ThreadMeta};
use crate::wal::{Append, Origin, Wal, WalRecord};
use counting::js::{self, len16, parse_float};
use counting::{Env, Input, ThreadRules, ValidationType, parse_input, validate};
use parking_lot::RwLock;
use serde_json::{Value, json};
use socketioxide::SocketIo;
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use tokio::sync::mpsc;

pub struct Ctx {
    pub io: SocketIo,
    pub wal: Wal,
    pub clock: Arc<ids::Clock>,
    pub next_id: AtomicU64,
    pub server_tz: chrono_tz::Tz,
    pub game_secret: String,
    pub metrics: Arc<Metrics>,
    pub app: std::sync::OnceLock<std::sync::Weak<crate::app::App>>,
}

pub struct PostJob {
    pub counter: CounterRef,
    pub text: String,
    pub post_hash: Option<Value>,
    pub macro_preset_id: Option<i64>,
    pub macro_preset_version_id: Option<i64>,
    pub received: Instant,
    pub origin: Origin,
}

impl PostJob {
    pub fn system(counter: CounterRef, text: String) -> PostJob {
        let text: String = text
            .encode_utf16()
            .take(4096)
            .collect::<Vec<u16>>()
            .pipe(|u| String::from_utf16_lossy(&u));
        PostJob {
            counter,
            text,
            post_hash: None,
            macro_preset_id: None,
            macro_preset_version_id: None,
            received: Instant::now(),
            origin: Origin::System,
        }
    }
}

trait Pipe: Sized {
    fn pipe<R>(self, f: impl FnOnce(Self) -> R) -> R {
        f(self)
    }
}
impl<T> Pipe for T {}

pub enum Msg {
    Post(Box<PostJob>),
    Update(Box<ThreadMeta>),
    Sync(Box<ThreadMeta>),
    SetThreadOfTheDay(bool),
}

pub const RING: usize = 256;

pub struct ThreadHandle {
    pub meta: RwLock<ThreadMeta>,
    pub tx: mpsc::Sender<Msg>,
    pub last_count: RwLock<Option<(Arc<Post>, CounterRef)>>,
    pub ring: RwLock<VecDeque<Arc<Post>>>,
}

impl ThreadHandle {
    pub fn ring_find(&self, uuid: &str) -> Option<Arc<Post>> {
        self.ring.read().iter().rev().find(|p| p.uuid == uuid).cloned()
    }

    pub fn ring_replace(&self, post: Arc<Post>) {
        let mut r = self.ring.write();
        if let Some(slot) = r.iter_mut().find(|p| p.uuid == post.uuid) {
            *slot = post;
        }
    }
}

pub fn author_key(uuid: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    uuid.hash(&mut h);
    h.finish()
}

pub struct State {
    pub last_valid: Option<Arc<Post>>,
    pub last_valid_counter: Option<CounterRef>,
    pub recent_valid: VecDeque<Arc<Post>>,
    pub last_post: Option<Arc<Post>>,
    pub streak: u64,
    pub slow_last: HashMap<u64, f64>,
}

struct Actor {
    h: Arc<ThreadHandle>,
    ctx: Arc<Ctx>,
    meta: ThreadMeta,
    vt: ValidationType,
    room: String,
    st: State,
    local_change: Option<Instant>,
}

pub fn spawn(meta: ThreadMeta, st: State, ctx: Arc<Ctx>) -> Arc<ThreadHandle> {
    let (tx, rx) = mpsc::channel(4096);
    let last_count = match (&st.last_valid, &st.last_valid_counter) {
        (Some(p), Some(c)) => Some((p.clone(), c.clone())),
        _ => None,
    };
    let ring: VecDeque<Arc<Post>> = st.last_post.iter().cloned().collect();
    let h = Arc::new(ThreadHandle {
        meta: RwLock::new(meta.clone()),
        tx,
        last_count: RwLock::new(last_count),
        ring: RwLock::new(ring),
    });
    let a = Actor {
        h: h.clone(),
        ctx,
        vt: ValidationType::from_name(&meta.validation_type),
        room: format!("thread_{}", meta.uuid),
        meta,
        st,
        local_change: None,
    };
    tokio::spawn(a.run(rx));
    h
}

fn ms(d: std::time::Duration) -> f64 {
    round2(d.as_secs_f64() * 1000.0)
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

impl Actor {
    fn bang(&self, c: &Counter, roll: f64, chance: f64) {
        let tx = self.h.tx.clone();
        let app = self.ctx.app.get().and_then(|a| a.upgrade());
        let name = c
            .fields
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or(&c.username)
            .to_owned();
        let uuid = c.uuid.clone();
        tokio::spawn(async move {
            let Some(app) = app else { return };
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            let text = format!(
                "BANG!!!!! {name} was just fucking KILLED!!! JESUS CHRIST!!!

 (roll: {}, chance: {})",
                js::num_to_string(roll),
                js::num_to_string(chance)
            );
            let _ = tx
                .send(Msg::Post(Box::new(PostJob::system(app.system_counter.clone(), text))))
                .await;
            app.add_xp(&uuid, 10000).await;
        });
    }

    fn adopt(&mut self, mut meta: ThreadMeta, keep_local: bool) {
        meta.id = self.meta.id;
        if keep_local {
            meta.count_bans = self.meta.count_bans.clone();
            meta.post_bans = self.meta.post_bans.clone();
            if let Some(v) = self.meta.row.get("threadOfTheDay").cloned() {
                meta.row.insert("threadOfTheDay".into(), v);
            }
        }
        self.meta = meta;
        self.vt = ValidationType::from_name(&self.meta.validation_type);
        *self.h.meta.write() = self.meta.clone();
    }

    async fn run(mut self, mut rx: mpsc::Receiver<Msg>) {
        while let Some(m) = rx.recv().await {
            match m {
                Msg::Post(job) => {
                    let name = self.meta.name.clone();
                    let author = job.counter.uuid.clone();
                    let text: String = job.text.chars().take(80).collect();
                    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.post(*job)));
                    if let Err(e) = r {
                        let why = e
                            .downcast_ref::<&str>()
                            .map(|s| s.to_string())
                            .or_else(|| e.downcast_ref::<String>().cloned());
                        tracing::error!(
                            "thread {name}: post by {author} panicked ({}); post dropped, thread keeps running. Text: {text:?}",
                            why.unwrap_or_else(|| "?".into())
                        );
                    }
                }
                Msg::Update(meta) => self.adopt(*meta, false),
                Msg::Sync(meta) => {
                    let recent = self
                        .local_change
                        .is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(10));
                    self.adopt(*meta, recent);
                }
                Msg::SetThreadOfTheDay(on) => {
                    self.local_change = Some(Instant::now());
                    self.meta.row.insert("threadOfTheDay".into(), json!(on));
                    *self.h.meta.write() = self.meta.clone();
                    let rec = WalRecord::ThreadFlags {
                        thread_id: self.meta.id,
                        thread_of_the_day: on,
                    };
                    self.ctx.wal.append(Append {
                        payload: serde_json::to_vec(&rec).unwrap(),
                        action: None,
                        lane: self.meta.id,
                    });
                }
            }
        }
    }

    fn reject(&self) {
        self.ctx.metrics.rejected.fetch_add(1, Ordering::Relaxed);
    }

    fn post(&mut self, job: PostJob) {
        let picked = Instant::now();
        let queue_wait = picked - job.received;
        self.ctx.metrics.queue.record(queue_wait);
        let c: &Counter = &job.counter;

        let trimmed_len = len16(js::trim(&job.text));
        let system = job.origin == Origin::System;
        if !system
            && (self.meta.locked
                || self.meta.post_bans.contains(&c.uuid)
                || !(1..=4096).contains(&trimmed_len)
                || !self.meta.may_post(c))
        {
            return self.reject();
        }

        let is_bot = c.has_role("bot");
        let mut input = js::trim(&job.text).to_owned();
        let parsed = parse_input(&input, self.vt, is_bot, self.meta.auto_validated);
        let ns = self.ctx.clock.now_ns();
        let uuid = ids::uuid_for(ns);
        let timestamp = ids::timestamp_for(ns);
        let ts = parse_float(&timestamp);
        let tslc = self
            .st
            .last_valid
            .as_ref()
            .map_or(0.0, |p| ts - parse_float(&p.timestamp));
        let tslp = self
            .st
            .last_post
            .as_ref()
            .map_or(0.0, |p| ts - parse_float(&p.timestamp));

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
        let last_prev = self
            .st
            .last_valid
            .as_ref()
            .map(|p| p.as_prev(author_key(&p.author_uuid)));
        let recent: Vec<counting::PrevCount> = self
            .st
            .recent_valid
            .iter()
            .take(2)
            .map(|p| p.as_prev(author_key(&p.author_uuid)))
            .collect();
        let within_hour = self.vt == ValidationType::Slow
            && self
                .st
                .slow_last
                .get(&me)
                .is_some_and(|&t| t > validate::timestamp_minus_ms(ts, 3_600_000.0) && t < ts);
        let rules = ThreadRules {
            vt: self.vt,
            first_valid_count: &self.meta.first_valid_count,
            allow_double_posts: self.meta.allow_double_posts,
            reset_on_mistakes: self.meta.reset_on_mistakes,
        };
        let vin = Input {
            author: me,
            is_bot,
            username_len16: len16(&c.username),
            count_banned: self.meta.count_bans.contains(&c.uuid),
            raw_count: parsed.raw_count.as_deref(),
            last_valid: last_prev.as_ref(),
            recent_valid: [recent.first(), recent.get(1)],
            post_timestamp: ts,
            time_since_last_count: tslc,
            author_counted_within_hour: within_hour,
            newest_valid_count_number: self.st.recent_valid.front().map(|p| p.valid_count_number),
        };
        let mut rng = fastrand::f64;
        let mut env = Env {
            now_ms: self.ctx.clock.now_ms(),
            server_tz: self.ctx.server_tz,
            random: &mut rng,
            secret: Some(&self.ctx.game_secret),
        };
        let v = validate(&rules, &vin, &mut env);
        let is_valid = v.is_valid;
        let stricken = !is_valid && parsed.is_count && self.meta.auto_validated;

        let three = comment.as_deref().is_some_and(|s| len16(js::trim(s)) >= 3);
        let post = Arc::new(Post {
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
            thread: self.meta.uuid.clone(),
            thread_id: self.meta.id,
            has_comment,
            has_three_char_comment: three,
            comment,
            author_uuid: c.uuid.clone(),
            author_id: c.id,
            is_deleted: false,
            is_comment_deleted: comment_deleted,
            reactions: json!({}),
            macro_preset_id: job.macro_preset_id,
            macro_preset_version_id: job.macro_preset_version_id,
            id: self.ctx.next_id.fetch_add(1, Ordering::Relaxed),
        });
        let mut extras = PostExtras {
            post_hash: job.post_hash,
            scheduling_ms: ms(queue_wait),
            ..Default::default()
        };
        if let Some((roll, chance)) = v.roll.filter(|_| post.is_count) {
            extras.roll = Some(roll);
            extras.chance = Some(chance);
        }

        let mut bans_changed = false;
        match self.meta.name.as_str() {
            "tug_of_war_avoid_0" if is_valid && post.raw_count.as_deref() == Some("0") => {
                self.meta.count_bans.push(c.uuid.clone());
                bans_changed = true;
            }
            "russian_roulette" if is_valid => {
                let (chance, roll) = (1.0 / 10000.0, fastrand::f64());
                extras.chance = Some(chance);
                extras.roll = Some(roll);
                if roll < chance {
                    self.meta.post_bans.push(c.uuid.clone());
                    bans_changed = true;
                    self.bang(c, roll, chance);
                }
            }
            "yoco" if is_valid => {
                self.meta.post_bans.push(c.uuid.clone());
                bans_changed = true;
            }
            _ => {}
        }

        if stricken {
            self.st.streak = 0;
        }
        if is_valid {
            self.st.last_valid = Some(post.clone());
            self.st.last_valid_counter = Some(job.counter.clone());
            self.st.recent_valid.push_front(post.clone());
            self.st.recent_valid.truncate(3);
            self.st.streak += 1;
            if self.vt == ValidationType::Slow {
                self.st.slow_last.insert(me, ts);
            }
            *self.h.last_count.write() = Some((post.clone(), job.counter.clone()));
        }
        if stricken && self.meta.reset_on_mistakes {
            self.st.last_valid = None;
            *self.h.last_count.write() = None;
            if matches!(
                self.meta.name.as_str(),
                "no_mistakes_or_ban" | "no_mistakes_or_ban_double"
            ) {
                self.meta.post_bans.push(post.author_uuid.clone());
                bans_changed = true;
            }
        }
        self.st.last_post = Some(post.clone());
        if bans_changed {
            self.local_change = Some(Instant::now());
            *self.h.meta.write() = self.meta.clone();
        }
        let built = Instant::now();
        self.ctx.metrics.process.record(built - picked);

        let payload = serde_json::to_vec(&WalRecord::Post {
            post: (*post).clone(),
            roll: extras.roll,
            chance: extras.chance,
            origin: job.origin.clone(),
        })
        .expect("serialise post");
        let handle = self.h.clone();
        let io = self.ctx.io.clone();
        let room = self.room.clone();
        let streak = self.st.streak;
        let counter = job.counter.clone();
        let metrics = self.ctx.metrics.clone();
        let received = job.received;
        let picked_at = picked;
        let thread_name = self.meta.name.clone();
        let thread_uuid = self.meta.uuid.clone();
        let chat = self.meta.name == "no_counting";
        let thread_update = bans_changed.then(|| self.meta.to_json());
        let post_b = post.clone();
        let action = Box::pin(async move {
            {
                let mut ring = handle.ring.write();
                ring.push_back(post_b.clone());
                if ring.len() > RING {
                    ring.pop_front();
                }
            }
            let now = Instant::now();
            metrics.wal.record(now - built);
            metrics.total.record(now - received);
            let total = (now - received).as_secs_f64() * 1000.0;
            extras.db_write_ms = ms(now - built);
            extras.pre_broadcast_ms = 0.0;
            extras.processing_latency = total;
            extras.server_at = crate::app::epoch_ms_f64();
            extras.other_ms = round2((total - extras.scheduling_ms - extras.db_write_ms).max(0.0));
            let msg = json!({
                "post": BroadcastPost { post: &post_b, extras: &extras },
                "counter": &counter.json,
                "thread_streak": streak,
            });
            let b0 = Instant::now();
            let _ = io.to(room.clone()).emit("post", &msg).await;
            let sent = Instant::now();
            metrics.broadcast.record(sent - b0);
            crate::monitor::slow_post((sent - received).as_millis() as u64, &thread_name);
            if sent - received >= *SLOW_POST {
                let ms = |d: std::time::Duration| d.as_secs_f64() * 1000.0;
                tracing::warn!(
                    "slow post in {thread_name}: {:.2} ms (queue {:.2}, process {:.2}, wal {:.2}, broadcast {:.2})",
                    ms(sent - received),
                    ms(picked_at - received),
                    ms(built - picked_at),
                    ms(now - built),
                    ms(sent - b0),
                );
            }
            if post_b.is_valid_count && !post_b.stricken {
                let _ = io
                    .to("thread_all")
                    .emit(
                        "latest_count",
                        &json!({
                            "threadUuid": thread_uuid,
                            "rawCount": post_b.raw_count,
                            "validCountNumber": post_b.valid_count_number,
                        }),
                    )
                    .await;
            }
            if chat {
                let _ = io
                    .to("site_chat")
                    .emit(
                        "chat_post",
                        &json!({ "post": BroadcastPost { post: &post_b, extras: &extras }, "counter": &counter.json }),
                    )
                    .await;
            }
            if let Some(t) = thread_update {
                let _ = io.to(room).emit("thread_update", &json!({ "threadInfo": t })).await;
            }
        });
        self.ctx.wal.append(Append {
            payload,
            action: Some(action),
            lane: self.meta.id,
        });
        if bans_changed {
            let rec = WalRecord::ThreadBans {
                thread_id: self.meta.id,
                count_bans: self.meta.count_bans.clone(),
                post_bans: self.meta.post_bans.clone(),
            };
            self.ctx.wal.append(Append {
                payload: serde_json::to_vec(&rec).unwrap(),
                action: None,
                lane: self.meta.id,
            });
        }
        self.ctx.metrics.posts.fetch_add(1, Ordering::Relaxed);
    }
}

static SLOW_POST: std::sync::LazyLock<std::time::Duration> = std::sync::LazyLock::new(|| {
    std::time::Duration::from_micros(
        (std::env::var("SLOW_POST_MS")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(10.0)
            * 1000.0) as u64,
    )
});

pub fn boot_state(meta: &ThreadMeta, b: crate::db::ThreadBoot, last_valid_counter: Option<CounterRef>) -> State {
    let recent: VecDeque<Arc<Post>> = b.recent_valid.into_iter().map(Arc::new).collect();
    let mut last_valid = recent.front().cloned();
    if meta.reset_on_mistakes
        && let (Some(lv), Some(m)) = (&last_valid, b.last_stricken_id)
        && m > lv.id
    {
        last_valid = None;
    }
    State {
        last_valid_counter: last_valid.as_ref().and(last_valid_counter),
        last_valid,
        recent_valid: recent,
        last_post: b.last_post.map(Arc::new),
        streak: b.streak,
        slow_last: b.slow_recent.into_iter().map(|(a, t)| (author_key(&a), t)).collect(),
    }
}
