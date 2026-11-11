use crate::ids;
use crate::metrics::Metrics;
use crate::model::{BroadcastPost, Counter, CounterRef, Post, PostExtras, ThreadMeta};
use crate::wal::{Append, Origin, Wal, WalRecord};
use cgg_decide::{Book, Decision, Dice, Job, apply, judge};
use counting::js;
use parking_lot::RwLock;
use serde_json::{Value, json};
use socketioxide::SocketIo;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

pub use cgg_decide::author_key;

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

pub struct Decided {
    pub decision: Decision,
    pub counter: CounterRef,
    pub post_hash: Option<Value>,
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
    Decided(Box<Decided>),
    Snapshot(tokio::sync::oneshot::Sender<(ThreadMeta, Book)>),
    Update(Box<ThreadMeta>),
    Sync(Box<ThreadMeta>),
    SetThreadOfTheDay(bool),
    Barrier(tokio::sync::oneshot::Sender<()>),
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

pub struct State {
    pub book: Book,
    pub last_valid_counter: Option<CounterRef>,
}

struct Actor {
    h: Arc<ThreadHandle>,
    ctx: Arc<Ctx>,
    meta: ThreadMeta,
    room: String,
    st: State,
    local_change: Option<Instant>,
}

pub fn spawn(meta: ThreadMeta, st: State, ctx: Arc<Ctx>) -> Arc<ThreadHandle> {
    let (tx, rx) = mpsc::channel(4096);
    let last_count = match (&st.book.last_valid, &st.last_valid_counter) {
        (Some(p), Some(c)) => Some((p.clone(), c.clone())),
        _ => None,
    };
    let ring: VecDeque<Arc<Post>> = st.book.last_post.iter().cloned().collect();
    let h = Arc::new(ThreadHandle {
        meta: RwLock::new(meta.clone()),
        tx,
        last_count: RwLock::new(last_count),
        ring: RwLock::new(ring),
    });
    let a = Actor {
        h: h.clone(),
        ctx,
        room: format!("thread_{}", meta.uuid),
        meta,
        st,
        local_change: None,
    };
    tokio::spawn(a.run(rx));
    h
}

fn ms(d: Duration) -> f64 {
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
            tokio::time::sleep(Duration::from_millis(50)).await;
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
        *self.h.meta.write() = self.meta.clone();
        if let Some(app) = self.ctx.app.get().and_then(|a| a.upgrade())
            && app.regional.is_remote(&self.meta.name)
        {
            let name = self.meta.name.clone();
            tokio::spawn(async move { crate::regional::meta_changed(&app, &name).await });
        }
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
                Msg::Decided(d) => self.decided(*d),
                Msg::Snapshot(reply) => {
                    let _ = reply.send((self.meta.clone(), self.st.book.clone()));
                }
                Msg::Update(meta) => self.adopt(*meta, false),
                Msg::Sync(meta) => {
                    let recent = self.local_change.is_some_and(|t| t.elapsed() < Duration::from_secs(10));
                    self.adopt(*meta, recent);
                }
                Msg::Barrier(done) => {
                    let _ = done.send(());
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

    fn post_ns(&self) -> u64 {
        let last = self.st.book.last_post.as_ref().map(|p| p.timestamp.as_str());
        cgg_decide::ids::ns_after(last, self.ctx.clock.now_ns())
    }

    fn reject(&self, job: &PostJob, reason: &'static str) {
        self.ctx.metrics.rejected.fetch_add(1, Ordering::Relaxed);
        let Some(hash) = job.post_hash.clone() else { return };
        let Some(app) = self.ctx.app.get().and_then(|a| a.upgrade()) else {
            return;
        };
        let author = job.counter.uuid.clone();
        let thread = self.meta.name.clone();
        tokio::spawn(async move {
            app.to_counter(
                &author,
                "post_rejected",
                &json!({ "thread_name": thread, "post_hash": hash, "reason": reason }),
            )
            .await;
        });
    }

    fn post(&mut self, job: PostJob) {
        let picked = Instant::now();
        let queue_wait = picked - job.received;
        self.ctx.metrics.queue.record(queue_wait);
        let mut rng = fastrand::f64;
        let mut dice = Dice {
            now_ms: self.ctx.clock.now_ms(),
            server_tz: self.ctx.server_tz,
            secret: Some(&self.ctx.game_secret),
            random: &mut rng,
        };
        let judged = judge(
            &self.meta,
            &self.st.book,
            &Job {
                counter: &job.counter,
                text: &job.text,
                system: job.origin == Origin::System,
                ns: self.post_ns(),
                macro_preset_id: job.macro_preset_id,
                macro_preset_version_id: job.macro_preset_version_id,
            },
            &mut dice,
        );
        match judged {
            Err(reason) => self.reject(&job, reason),
            Ok(d) => self.commit(
                d,
                Committing {
                    counter: job.counter,
                    post_hash: job.post_hash,
                    origin: job.origin,
                    received: job.received,
                    picked,
                    queue_wait,
                },
            ),
        }
    }

    fn decided(&mut self, d: Decided) {
        let picked = Instant::now();
        let queue_wait = picked - d.received;
        self.ctx.metrics.queue.record(queue_wait);
        self.commit(
            d.decision,
            Committing {
                counter: d.counter,
                post_hash: d.post_hash,
                origin: d.origin,
                received: d.received,
                picked,
                queue_wait,
            },
        );
    }

    fn commit(&mut self, mut d: Decision, how: Committing) {
        let Committing {
            counter,
            post_hash,
            origin,
            received,
            picked,
            queue_wait,
        } = how;
        d.post.id = self.ctx.next_id.fetch_add(1, Ordering::Relaxed);
        let post = Arc::new(d.post);
        let bans_changed = apply(&mut self.meta, &mut self.st.book, &post, &d.bans);
        if d.bang
            && let (Some(roll), Some(chance)) = (d.roll, d.chance)
        {
            self.bang(&counter, roll, chance);
        }
        let mut extras = PostExtras {
            post_hash,
            scheduling_ms: ms(queue_wait),
            roll: d.roll,
            chance: d.chance,
            ..Default::default()
        };
        if post.is_valid_count {
            self.st.last_valid_counter = Some(counter.clone());
            *self.h.last_count.write() = Some((post.clone(), counter.clone()));
        }
        if post.stricken && self.meta.reset_on_mistakes {
            *self.h.last_count.write() = None;
        }
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
            origin,
        })
        .expect("serialise post");
        let handle = self.h.clone();
        let io = self.ctx.io.clone();
        let room = self.room.clone();
        let streak = self.st.book.streak;
        let metrics = self.ctx.metrics.clone();
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
                let ms = |d: Duration| d.as_secs_f64() * 1000.0;
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

static SLOW_POST: std::sync::LazyLock<Duration> = std::sync::LazyLock::new(|| {
    Duration::from_micros(
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
        book: Book {
            last_valid,
            recent_valid: recent,
            last_post: b.last_post.map(Arc::new),
            streak: b.streak,
            slow_last: b.slow_recent.into_iter().map(|(a, t)| (author_key(&a), t)).collect(),
        },
    }
}

struct Committing {
    counter: CounterRef,
    post_hash: Option<Value>,
    origin: Origin,
    received: Instant,
    picked: Instant,
    queue_wait: Duration,
}
