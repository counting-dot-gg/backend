use crate::config::ClickhouseConfig;
use crate::cursor::{Checkpoints, CursorFile, Position};
use crate::metrics::Histogram;
use crate::model::Post;
use crate::wal::{Committed, Reader, WalRecord};
use anyhow::{Result, bail};
use post_codec::{Context, Flag, Plain, derive, tsv};
use sqlx::mysql::MySqlPool;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

pub const CURSOR: &str = "clickhouse";

const POST_TABLE: &[&str] = &[
    "threadId UInt16",
    "id UInt64 CODEC(Delta, ZSTD(3))",
    "tsUs UInt64 CODEC(Delta, T64, ZSTD(3))",
    "uuidRest UInt32 CODEC(T64, ZSTD(3))",
    "flags UInt32 CODEC(ZSTD(3))",
    "validCountNumber Int64 CODEC(Delta, ZSTD(3))",
    "pos Int64 CODEC(Delta, ZSTD(3))",
    "streak Int32 CODEC(DoubleDelta, ZSTD(3))",
    "countContent String CODEC(ZSTD(9))",
    "comment String CODEC(ZSTD(9))",
    "thread LowCardinality(String)",
    "authorId UInt32 CODEC(ZSTD(3))",
    "authorUUID LowCardinality(String)",
    "macroPresetId UInt32 CODEC(ZSTD(3))",
    "macroPresetVersionId UInt32 CODEC(ZSTD(3))",
    "tsRaw String CODEC(ZSTD(9))",
    "uuidRaw String CODEC(ZSTD(9))",
    "rawTextRaw String CODEC(ZSTD(9))",
    "rawCountRaw String CODEC(ZSTD(9))",
    "encVersion UInt8",
    "INDEX ix_id id TYPE minmax GRANULARITY 1",
    "INDEX ix_ts tsUs TYPE minmax GRANULARITY 1",
    "INDEX ix_pos pos TYPE minmax GRANULARITY 1",
];

const POST_TABLE_TAIL: &str = "ENGINE = MergeTree ORDER BY (threadId, id) SETTINGS enable_block_number_column = 1, \
    enable_block_offset_column = 1, non_replicated_deduplication_window = 1000";

const REPAIRS_TABLE: &str = "CREATE TABLE IF NOT EXISTS post_gap_repairs (id UInt64,     field LowCardinality(String), old Nullable(String), new Nullable(String)) ENGINE = ReplacingMergeTree     ORDER BY (id, field) SETTINGS non_replicated_deduplication_window = 1000";

const AUTHOR_TABLE: &str = "CREATE TABLE IF NOT EXISTS post_by_author (authorId UInt32, \
    id UInt64 CODEC(Delta, ZSTD(3)), threadId UInt16 CODEC(ZSTD(3)), valid UInt8) ENGINE = MergeTree \
    ORDER BY (authorId, id) SETTINGS index_granularity = 1024, non_replicated_deduplication_window = 1000";

#[derive(Default)]
pub struct Stats {
    pub state: AtomicU64,
    pub flush: Histogram,
    pub visible: Histogram,
    pub flushes: AtomicU64,
    pub failures: AtomicU64,
    pub posts: AtomicU64,
    pub deletes: AtomicU64,
    pub gap_repairs: AtomicU64,
    pub mismatches: AtomicU64,
    pub last_write_unix_ms: AtomicU64,
    pub backfill_posts: AtomicU64,
    pub backfill_at: AtomicU64,
    pub backfill_upto: AtomicU64,
    pub server_up: AtomicU64,
    pub server_parts: AtomicU64,
    pub server_bytes: AtomicU64,
    pub server_rows: AtomicU64,
    pub server_merges: AtomicU64,
}

#[derive(Clone, Copy)]
pub enum State {
    Starting = 1,
    Backfilling = 2,
    Following = 3,
    Detached = 4,
    Stopped = 5,
}

pub fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

pub fn visible_after(timestamp: &str) -> Option<Duration> {
    let made_us = derive::ts_us_of(timestamp)?;
    let now_us = unix_ms() * 1000;
    Some(Duration::from_micros(now_us.saturating_sub(made_us)))
}

impl Stats {
    fn set(&self, s: State) {
        self.state.store(s as u64, Ordering::Relaxed);
    }

    fn add(counter: &AtomicU64, n: u64) {
        counter.fetch_add(n, Ordering::Relaxed);
    }

    fn wrote(&self) {
        self.last_write_unix_ms.store(unix_ms(), Ordering::Relaxed);
    }

    fn written(&self, e: &Encoded) {
        Stats::add(&self.gap_repairs, e.repairs.lines().count() as u64);
        Stats::add(&self.mismatches, e.mismatches.len() as u64);
        for ts in &e.timestamps {
            if let Some(d) = visible_after(ts) {
                self.visible.record(d);
            }
        }
        self.wrote();
    }

    pub fn render(&self, out: &mut String) {
        let state = self.state.load(Ordering::Relaxed);
        if state == 0 {
            return;
        }
        self.flush.write(
            out,
            "cgg_clickhouse_flush_seconds",
            "one WAL batch written to ClickHouse",
        );
        self.visible.write(
            out,
            "cgg_clickhouse_visible_seconds",
            "post made to readable in ClickHouse",
        );
        let load = |a: &AtomicU64| a.load(Ordering::Relaxed) as f64;
        let mut line = |name: &str, kind: &str, v: f64| {
            let _ = writeln!(out, "# TYPE {name} {kind}\n{name} {v}");
        };
        line("cgg_clickhouse_state", "gauge", state as f64);
        line("cgg_clickhouse_flushes_total", "counter", load(&self.flushes));
        line("cgg_clickhouse_failures_total", "counter", load(&self.failures));
        line("cgg_clickhouse_posts_total", "counter", load(&self.posts));
        line("cgg_clickhouse_comment_deletes_total", "counter", load(&self.deletes));
        line("cgg_clickhouse_gap_repairs_total", "counter", load(&self.gap_repairs));
        line("cgg_clickhouse_mismatches_total", "counter", load(&self.mismatches));
        line(
            "cgg_clickhouse_last_write_timestamp_seconds",
            "gauge",
            load(&self.last_write_unix_ms) / 1e3,
        );
        line(
            "cgg_clickhouse_backfill_posts_total",
            "counter",
            load(&self.backfill_posts),
        );
        line("cgg_clickhouse_backfill_at_id", "gauge", load(&self.backfill_at));
        line("cgg_clickhouse_backfill_upto_id", "gauge", load(&self.backfill_upto));
        line("cgg_clickhouse_up", "gauge", load(&self.server_up));
        line("cgg_clickhouse_active_parts", "gauge", load(&self.server_parts));
        line("cgg_clickhouse_disk_bytes", "gauge", load(&self.server_bytes));
        line("cgg_clickhouse_rows", "gauge", load(&self.server_rows));
        line("cgg_clickhouse_merges", "gauge", load(&self.server_merges));
    }
}

async fn poll_server(client: Client, stats: Arc<Stats>) {
    let sql = format!(
        "SELECT (SELECT count() FROM system.parts WHERE database = '{db}' AND active), \
         (SELECT sum(bytes_on_disk) FROM system.parts WHERE database = '{db}' AND active), \
         (SELECT sum(rows) FROM system.parts WHERE database = '{db}' AND table = 'post' AND active), \
         (SELECT count() FROM system.merges WHERE database = '{db}') FORMAT TSV",
        db = client.cfg.database
    );
    loop {
        let next = match client.exec(&sql, &[]).await {
            Ok(body) => {
                let cols: Vec<u64> = body.trim().split('\t').map(|v| v.parse().unwrap_or(0)).collect();
                for (gauge, v) in [
                    &stats.server_parts,
                    &stats.server_bytes,
                    &stats.server_rows,
                    &stats.server_merges,
                ]
                .into_iter()
                .zip(cols)
                {
                    gauge.store(v, Ordering::Relaxed);
                }
                stats.server_up.store(1, Ordering::Relaxed);
                Duration::from_secs(30)
            }
            Err(_) => {
                stats.server_up.store(0, Ordering::Relaxed);
                Duration::from_secs(5)
            }
        };
        tokio::time::sleep(next).await;
    }
}

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    cfg: ClickhouseConfig,
}

impl Client {
    fn request(&self, params: &[(&str, &str)]) -> reqwest::RequestBuilder {
        let mut req = self.http.post(&self.cfg.url).query(params);
        if !params.iter().any(|(k, _)| *k == "database") {
            req = req.query(&[("database", self.cfg.database.as_str())]);
        }
        if let Some(u) = &self.cfg.user {
            req = req.header("X-ClickHouse-User", u);
        }
        if let Some(p) = &self.cfg.password {
            req = req.header("X-ClickHouse-Key", p);
        }
        req
    }

    async fn send(req: reqwest::RequestBuilder) -> Result<String> {
        let res = req.timeout(Duration::from_secs(60)).send().await?;
        let status = res.status();
        let body = res.text().await?;
        if !status.is_success() {
            bail!("clickhouse {status}: {}", body.chars().take(500).collect::<String>());
        }
        Ok(body)
    }

    async fn exec(&self, sql: &str, params: &[(&str, &str)]) -> Result<String> {
        Self::send(self.request(params).body(sql.to_owned())).await
    }

    async fn insert(&self, sql: &str, token: &str, data: String) -> Result<()> {
        let params = [("query", sql), ("insert_deduplication_token", token)];
        Self::send(self.request(&params).body(data)).await.map(|_| ())
    }
}

#[derive(Debug, Clone, Default)]
pub struct ThreadState {
    ctx: Context,
    last_id: u64,
}

struct CommentDelete {
    id: Option<u64>,
    uuid: String,
    raw_text: String,
}

#[derive(Default)]
struct Batch {
    posts: Vec<Plain>,
    deletes: Vec<CommentDelete>,
    first_seq: u64,
    last_seq: u64,
    started: Option<Instant>,
}

impl Batch {
    fn note(&mut self, seq: u64) {
        if self.first_seq == 0 {
            self.first_seq = seq;
        }
        self.last_seq = seq;
        self.started.get_or_insert_with(Instant::now);
    }

    fn delete_comment(&mut self, d: CommentDelete) {
        let buffered = self
            .posts
            .iter_mut()
            .find(|p| d.id.map_or(p.uuid == d.uuid, |id| p.id == id));
        match buffered {
            Some(p) => {
                p.has_comment = false;
                p.is_comment_deleted = true;
                p.comment = None;
                p.raw_text = d.raw_text;
            }
            None => self.deletes.push(d),
        }
    }
}

pub fn plain_of(post: &Post) -> Option<Plain> {
    Some(Plain {
        id: post.id,
        uuid: post.uuid.clone(),
        timestamp: post.timestamp.clone(),
        time_since_last_count: Some(post.time_since_last_count.clone()),
        time_since_last_post: Some(post.time_since_last_post.clone()),
        raw_text: post.raw_text.clone(),
        is_count: post.is_count,
        is_valid_count: Some(post.is_valid_count),
        count_content: post.count_content.clone(),
        raw_count: post.raw_count.clone(),
        stricken: post.stricken,
        thread: post.thread.clone(),
        thread_id: u16::try_from(post.thread_id).ok()?,
        has_comment: post.has_comment,
        has_three_char_comment: post.has_three_char_comment,
        comment: post.comment.clone(),
        author_uuid: post.author_uuid.clone(),
        author_id: post.author_id.unwrap_or(0),
        is_deleted: post.is_deleted,
        is_comment_deleted: post.is_comment_deleted,
        valid_count_number: post.valid_count_number,
        macro_preset_id: post.macro_preset_id.and_then(|v| u32::try_from(v).ok()),
        macro_preset_version_id: post.macro_preset_version_id.and_then(|v| u32::try_from(v).ok()),
    })
}

fn timestamp_from(ts_us: u64, ts_raw: Option<String>, flags: u32) -> String {
    match ts_raw {
        Some(raw) if flags & Flag::TsRaw.bit() != 0 => raw,
        _ => derive::timestamp_of(ts_us),
    }
}

fn tsv_rows(body: &str) -> impl Iterator<Item = Vec<Option<String>>> + '_ {
    body.lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.split('\t').map(tsv::unescape).collect())
}

fn num<T: std::str::FromStr + Default>(cols: &[Option<String>], i: usize) -> T {
    cols.get(i)
        .and_then(|c| c.as_deref())
        .and_then(|s| s.parse().ok())
        .unwrap_or_default()
}

fn text(cols: &[Option<String>], i: usize) -> Option<String> {
    cols.get(i).cloned().flatten()
}

async fn load_threads(c: &Client) -> Result<HashMap<u16, ThreadState>> {
    let latest = c
        .exec(
            "SELECT threadId, max(id), argMax(tsUs, id), argMax(tsRaw, id), argMax(flags, id), argMax(pos, id), \
             argMax(streak, id) FROM post GROUP BY threadId FORMAT TSV",
            &[],
        )
        .await?;
    let mut threads = HashMap::new();
    for cols in tsv_rows(&latest) {
        let flags = num(&cols, 4);
        threads.insert(
            num::<u16>(&cols, 0),
            ThreadState {
                last_id: num(&cols, 1),
                ctx: Context {
                    last_post_ts: Some(timestamp_from(num(&cols, 2), text(&cols, 3), flags)),
                    last_valid_ts: None,
                    pos: num(&cols, 5),
                    streak: num(&cols, 6),
                },
            },
        );
    }
    let valid = c
        .exec(
            &format!(
                "SELECT threadId, argMax(tsUs, id), argMax(tsRaw, id), argMax(flags, id) FROM post \
                 WHERE bitAnd(flags, {}) != 0 GROUP BY threadId FORMAT TSV",
                Flag::Valid.bit()
            ),
            &[],
        )
        .await?;
    for cols in tsv_rows(&valid) {
        if let Some(t) = threads.get_mut(&num::<u16>(&cols, 0)) {
            t.ctx.last_valid_ts = Some(timestamp_from(num(&cols, 1), text(&cols, 2), num(&cols, 3)));
        }
    }
    Ok(threads)
}

async fn prepare(c: &Client) -> Result<HashMap<u16, ThreadState>> {
    c.exec(
        &format!("CREATE DATABASE IF NOT EXISTS {}", c.cfg.database),
        &[("database", "default")],
    )
    .await?;
    c.exec(
        &format!(
            "CREATE TABLE IF NOT EXISTS post ({}) {POST_TABLE_TAIL}",
            POST_TABLE.join(", ")
        ),
        &[],
    )
    .await?;
    c.exec(AUTHOR_TABLE, &[]).await?;
    c.exec(REPAIRS_TABLE, &[]).await?;
    c.exec(
        &format!(
            "INSERT INTO post_by_author SELECT authorId, id, threadId, bitAnd(flags, {}) != 0 FROM post \
             WHERE id > (SELECT max(id) FROM post_by_author)",
            Flag::Valid.bit()
        ),
        &[],
    )
    .await?;
    load_threads(c).await
}

async fn resolve_id(c: &Client, uuid: &str) -> Result<Option<u64>> {
    let Some((ns, _)) = derive::uuid_parts(uuid) else {
        return Ok(None);
    };
    let ts_us = ns / 1000;
    let Some(rest) = derive::uuid_rest(uuid, ts_us) else {
        return Ok(None);
    };
    let body = c
        .exec(
            &format!(
                "SELECT id FROM post WHERE tsUs = {ts_us} AND uuidRest = {rest} AND bitAnd(flags, {}) = 0 \
                 LIMIT 1 FORMAT TSV",
                Flag::UuidRaw.bit()
            ),
            &[],
        )
        .await?;
    Ok(body.trim().parse().ok())
}

async fn apply_delete(c: &Client, d: &CommentDelete) -> Result<()> {
    let id = match d.id {
        Some(id) => Some(id),
        None => resolve_id(c, &d.uuid).await?,
    };
    let Some(id) = id else {
        tracing::warn!("clickhouse: comment delete for unknown post {}", d.uuid);
        return Ok(());
    };
    let derived = format!(
        "if(bitAnd(flags, {}) != 0, countContent, '')",
        Flag::CountContentPresent.bit()
    );
    let cleared = Flag::HasComment.bit() | Flag::CommentPresent.bit() | Flag::RawTextRaw.bit();
    let sql = format!(
        "UPDATE post SET comment = '', rawTextRaw = if({derived} = {{text:String}}, '', {{text:String}}), \
         flags = bitOr(bitAnd(flags, bitNot(toUInt32({cleared}))), toUInt32({} + if({derived} = {{text:String}}, 0, {}))) \
         WHERE id = {id}",
        Flag::CommentDeleted.bit(),
        Flag::RawTextRaw.bit()
    );
    c.exec(&sql, &[("param_text", d.raw_text.as_str())]).await.map(|_| ())
}

#[derive(Default)]
struct Encoded {
    threads: HashMap<u16, ThreadState>,
    rows: String,
    authors: String,
    repairs: String,
    mismatches: Vec<(u64, Vec<&'static str>)>,
    first_id: u64,
    last_id: u64,
    timestamps: Vec<String>,
}

fn push_nullable(v: Option<&str>, out: &mut String) {
    match v {
        Some(s) => tsv::escape(s, out),
        None => out.push_str("\\N"),
    }
}

fn encode_batch(threads: &HashMap<u16, ThreadState>, posts: &[Plain]) -> Encoded {
    let mut out = Encoded {
        threads: threads.clone(),
        ..Encoded::default()
    };
    for p in posts {
        let t = out.threads.entry(p.thread_id).or_default();
        if p.id <= t.last_id {
            continue;
        }
        let checked = post_codec::encode_checked(&t.ctx, p);
        tsv::write_row(&checked.row, &mut out.rows);
        out.rows.push('\n');
        out.authors.push_str(&format!(
            "{}\t{}\t{}\t{}\n",
            p.author_id,
            p.id,
            p.thread_id,
            u8::from(p.is_valid())
        ));
        for r in checked.repairs.iter().filter(|r| !r.is_float_noise()) {
            out.repairs.push_str(&format!("{}\t{}\t", r.id, r.field));
            push_nullable(r.old.as_deref(), &mut out.repairs);
            out.repairs.push('\t');
            push_nullable(r.new.as_deref(), &mut out.repairs);
            out.repairs.push('\n');
        }
        if !checked.mismatches.is_empty() {
            out.mismatches.push((p.id, checked.mismatches));
        }
        if out.first_id == 0 {
            out.first_id = p.id;
        }
        out.last_id = p.id;
        out.timestamps.push(p.timestamp.clone());
        t.ctx.advance(p);
        t.last_id = p.id;
    }
    out
}

async fn write_encoded(c: &Client, e: &Encoded, token: &str) -> Result<()> {
    if !e.repairs.is_empty() {
        c.insert(
            "INSERT INTO post_gap_repairs (id, field, old, new) FORMAT TSV",
            token,
            e.repairs.clone(),
        )
        .await?;
    }
    if e.rows.is_empty() {
        return Ok(());
    }
    c.insert(
        &format!("INSERT INTO post ({}) FORMAT TSV", tsv::ROW_COLUMNS),
        token,
        e.rows.clone(),
    )
    .await?;
    c.insert(
        "INSERT INTO post_by_author (authorId, id, threadId, valid) FORMAT TSV",
        token,
        e.authors.clone(),
    )
    .await
}

async fn flush(
    c: &Client,
    stats: &Stats,
    threads: &HashMap<u16, ThreadState>,
    batch: &Batch,
) -> Result<HashMap<u16, ThreadState>> {
    let started = Instant::now();
    let encoded = encode_batch(threads, &batch.posts);
    for (id, fields) in &encoded.mismatches {
        tracing::error!("clickhouse: post {id} does not round-trip: {fields:?}");
    }
    write_encoded(c, &encoded, &format!("{}-{}", batch.first_seq, batch.last_seq)).await?;
    for d in &batch.deletes {
        apply_delete(c, d).await?;
    }
    stats.flush.record(started.elapsed());
    Stats::add(&stats.flushes, 1);
    Stats::add(&stats.posts, encoded.timestamps.len() as u64);
    Stats::add(&stats.deletes, batch.deletes.len() as u64);
    stats.written(&encoded);
    Ok(encoded.threads)
}

fn read_record(reader: &mut Reader, upto: u64) -> Result<Option<(u64, Option<WalRecord>)>> {
    let Some((seq, payload)) = reader.next(upto)? else {
        return Ok(None);
    };
    let rec = serde_json::from_slice::<WalRecord>(&payload)
        .inspect_err(|e| tracing::error!("clickhouse: WAL record {seq} unreadable: {e}"))
        .ok();
    Ok(Some((seq, rec)))
}

fn take(batch: &mut Batch, seq: u64, rec: Option<WalRecord>) {
    batch.note(seq);
    let Some(rec) = rec else { return };
    match rec {
        WalRecord::Post { post, .. } => {
            if let Some(p) = plain_of(&post) {
                batch.posts.push(p);
            }
        }
        WalRecord::PostUpdate {
            uuid,
            id,
            deleted_comment: Some(d),
            ..
        } => batch.delete_comment(CommentDelete {
            id,
            uuid,
            raw_text: d.raw_text,
        }),
        _ => {}
    }
}

async fn mysql_chunk(pool: &MySqlPool, after: u64, upto: u64, limit: usize) -> Result<Vec<Plain>> {
    let rows = sqlx::query(&format!(
        "SELECT {} FROM post WHERE id > ? AND id <= ? ORDER BY id LIMIT ?",
        tsv::PLAIN_COLUMNS
    ))
    .bind(after)
    .bind(upto)
    .bind(limit as u64)
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(post_codec::mysql::plain).collect::<Result<_, _>>()?)
}

fn fields(line: &str) -> Vec<Option<String>> {
    line.split('\t').map(tsv::unescape).collect()
}

fn first_difference(sent: &str, stored: &str) -> Option<String> {
    let mut stored_lines = stored.lines().filter(|l| !l.is_empty());
    for line in sent.lines().filter(|l| !l.is_empty()) {
        let want = fields(line);
        match stored_lines.next().map(fields) {
            Some(got) if got == want => {}
            Some(got) => return Some(format!("sent {want:?}, stored {got:?}")),
            None => return Some(format!("sent {want:?}, stored nothing")),
        }
    }
    stored_lines.next().map(|l| format!("unexpected stored row {l}"))
}

async fn stored_rows(c: &Client, first: u64, last: u64) -> Result<String> {
    c.exec(
        &format!(
            "SELECT {} FROM post WHERE id BETWEEN {first} AND {last} ORDER BY id FORMAT TSV",
            tsv::ROW_COLUMNS
        ),
        &[],
    )
    .await
}

pub enum Halt {
    Detach(String),
    Fatal(String),
}

pub struct Backfilled {
    pub threads: HashMap<u16, ThreadState>,
    pub posts: u64,
    pub repairs: u64,
}

fn last_copied(threads: &HashMap<u16, ThreadState>) -> u64 {
    threads.values().map(|t| t.last_id).max().unwrap_or(0)
}

pub struct Backfill<'a> {
    pub client: &'a Client,
    pub pool: &'a MySqlPool,
    pub chunk: usize,
    pub rate: u64,
    pub halt: &'a (dyn Fn() -> Option<Halt> + Sync),
    pub stats: &'a Stats,
}

impl Backfill<'_> {
    async fn retry<T, Fut: Future<Output = Result<T>>, F: Fn() -> Fut>(&self, what: &str, f: F) -> Result<T, Halt> {
        let mut backoff = Duration::from_millis(200);
        loop {
            match f().await {
                Ok(v) => return Ok(v),
                Err(e) => {
                    Stats::add(&self.stats.failures, 1);
                    tracing::error!("clickhouse: backfill {what} failed: {e:#}; retrying in {backoff:?}");
                    if let Some(h) = (self.halt)() {
                        return Err(h);
                    }
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(Duration::from_secs(30));
                }
            }
        }
    }

    async fn verify(&self, encoded: &Encoded) -> Result<(), Halt> {
        if encoded.first_id == 0 {
            return Ok(());
        }
        let stored = self
            .retry("verify", || stored_rows(self.client, encoded.first_id, encoded.last_id))
            .await?;
        match first_difference(&encoded.rows, &stored) {
            None => Ok(()),
            Some(diff) => Err(Halt::Fatal(format!(
                "posts {}..={} read back differently: {diff}",
                encoded.first_id, encoded.last_id
            ))),
        }
    }

    pub async fn run(&self, mut threads: HashMap<u16, ThreadState>, upto: u64) -> Result<Backfilled, Halt> {
        let started = Instant::now();
        let mut after = last_copied(&threads);
        let mut posts_done = 0u64;
        let mut repairs_done = 0u64;
        let mut logged = 0;
        self.stats.backfill_upto.store(upto, Ordering::Relaxed);
        self.stats.backfill_at.store(after, Ordering::Relaxed);
        if after < upto {
            self.stats.set(State::Backfilling);
            tracing::info!("clickhouse: backfilling posts {}..={upto} from MySQL", after + 1);
        }
        while after < upto {
            if let Some(h) = (self.halt)() {
                return Err(h);
            }
            let posts = self
                .retry("read", || mysql_chunk(self.pool, after, upto, self.chunk))
                .await?;
            let Some(last) = posts.last().map(|p| p.id) else {
                break;
            };
            let prior = threads.clone();
            let encoded = tokio::task::spawn_blocking(move || encode_batch(&prior, &posts))
                .await
                .map_err(|e| Halt::Fatal(format!("encode panicked: {e}")))?;
            if let Some((id, fields)) = encoded.mismatches.first() {
                Stats::add(&self.stats.mismatches, encoded.mismatches.len() as u64);
                return Err(Halt::Fatal(format!(
                    "post {id} does not round-trip ({fields:?}); {} in this chunk",
                    encoded.mismatches.len()
                )));
            }
            let token = format!("backfill-{}-{last}", after + 1);
            self.retry("insert", || write_encoded(self.client, &encoded, &token))
                .await?;
            self.verify(&encoded).await?;
            let copied = encoded.timestamps.len() as u64;
            posts_done += copied;
            Stats::add(&self.stats.backfill_posts, copied);
            Stats::add(&self.stats.gap_repairs, encoded.repairs.lines().count() as u64);
            self.stats.backfill_at.store(last, Ordering::Relaxed);
            self.stats.wrote();
            repairs_done += encoded.repairs.lines().count() as u64;
            threads = encoded.threads;
            after = last;
            if posts_done - logged >= 1_000_000 {
                logged = posts_done;
                tracing::info!(
                    "clickhouse: backfilled {posts_done} posts up to {after} ({:.0}/s, {repairs_done} gap repairs)",
                    posts_done as f64 / started.elapsed().as_secs_f64().max(0.001)
                );
            }
            if self.rate > 0 {
                let due = Duration::from_secs_f64(posts_done as f64 / self.rate as f64);
                if let Some(wait) = due.checked_sub(started.elapsed()) {
                    tokio::time::sleep(wait).await;
                }
            }
        }
        if posts_done > 0 {
            tracing::info!(
                "clickhouse: backfill done, {posts_done} posts in {:.0?} ({repairs_done} gap repairs)",
                started.elapsed()
            );
        }
        Ok(Backfilled {
            threads,
            posts: posts_done,
            repairs: repairs_done,
        })
    }
}

pub struct Sink {
    pub cfg: ClickhouseConfig,
    pub dir: PathBuf,
    pub committed: Arc<Committed>,
    pub persisted: Arc<Committed>,
    pub mysql: cgg_core::MysqlConfig,
    pub checkpoints: Arc<Checkpoints>,
    pub checkpoint: Arc<AtomicU64>,
    pub start: Position,
    pub stats: Arc<Stats>,
}

impl Sink {
    fn lag_halt(&self, seq: u64) -> Option<Halt> {
        let lag = self.committed.get().saturating_sub(seq);
        (lag > self.cfg.max_lag_records)
            .then(|| Halt::Detach(format!("{lag} WAL records behind (limit {})", self.cfg.max_lag_records)))
    }

    async fn retry<T, Fut: Future<Output = Result<T>>, F: Fn() -> Fut>(
        &self,
        what: &str,
        seq: u64,
        f: F,
    ) -> Result<T, Halt> {
        let mut backoff = Duration::from_millis(200);
        loop {
            match f().await {
                Ok(v) => return Ok(v),
                Err(e) => {
                    Stats::add(&self.stats.failures, 1);
                    tracing::error!("clickhouse: {what} failed: {e:#}; retrying in {backoff:?}");
                    if let Some(h) = self.lag_halt(seq) {
                        return Err(h);
                    }
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(Duration::from_secs(30));
                }
            }
        }
    }

    async fn catch_up(
        &self,
        client: &Client,
        pool: &MySqlPool,
        start: Position,
    ) -> Result<HashMap<u16, ThreadState>, Halt> {
        let threads = self.retry("start", start.seq, || prepare(client)).await?;
        let halt = || self.lag_halt(start.seq);
        let backfill = Backfill {
            client,
            pool,
            chunk: self.cfg.backfill_chunk,
            rate: self.cfg.backfill_rate,
            halt: &halt,
            stats: &self.stats,
        };
        Ok(backfill.run(threads, start.post_id).await?.threads)
    }

    async fn follow(&self, client: &Client, pool: &MySqlPool, start: Position, checkpoint: &AtomicU64) -> Halt {
        let mut threads = match self.catch_up(client, pool, start).await {
            Ok(t) => t,
            Err(h) => return h,
        };
        self.stats.set(State::Following);
        tracing::info!(
            "clickhouse: following WAL from {} ({} threads)",
            start.seq,
            threads.len()
        );
        let mut reader = match Reader::open(&self.dir, start.seq) {
            Ok(r) => r,
            Err(e) => return Halt::Detach(format!("cannot reopen WAL at {}: {e:#}", start.seq)),
        };
        let file = CursorFile::new(&self.dir, CURSOR);
        let mut pos = start;
        let mut batch = Batch::default();
        loop {
            if let Some(h) = self.lag_halt(pos.seq) {
                return h;
            }
            let notified = self.committed.notify.notified();
            let upto = self.committed.get();
            let mut read_any = false;
            while batch.posts.len() < self.cfg.batch {
                match read_record(&mut reader, upto) {
                    Ok(Some((seq, rec))) => {
                        read_any = true;
                        take(&mut batch, seq, rec);
                    }
                    Ok(None) => break,
                    Err(e) => {
                        tracing::error!("clickhouse: WAL read failed: {e:#}");
                        break;
                    }
                }
            }
            let due = batch
                .started
                .is_some_and(|t| t.elapsed() >= self.cfg.flush_every || batch.posts.len() >= self.cfg.batch);
            if due {
                let what = format!("flush {}..={}", batch.first_seq, batch.last_seq);
                threads = match self
                    .retry(&what, batch.first_seq, || flush(client, &self.stats, &threads, &batch))
                    .await
                {
                    Ok(t) => t,
                    Err(h) => return h,
                };
                pos.seq = batch.last_seq;
                pos.post_id = pos.post_id.max(batch.posts.iter().map(|p| p.id).max().unwrap_or(0));
                if let Err(e) = file.store(pos) {
                    tracing::error!("clickhouse: cursor save failed: {e:#}");
                }
                checkpoint.store(pos.seq, Ordering::Relaxed);
                batch = Batch::default();
                continue;
            }
            if read_any {
                continue;
            }
            let wait = batch.started.map_or(Duration::from_millis(250), |t| {
                self.cfg.flush_every.saturating_sub(t.elapsed())
            });
            let _ = tokio::time::timeout(wait, notified).await;
        }
    }

    async fn reattach(&self, client: &Client, pool: &MySqlPool) -> (Position, Arc<AtomicU64>) {
        let mut backoff = Duration::from_secs(1);
        loop {
            let ready = async {
                prepare(client).await?;
                let seq = self.persisted.get();
                let checkpoint = self.checkpoints.register(CURSOR, seq);
                match crate::db::max_post_id(pool).await {
                    Ok(post_id) => Ok((Position { seq, post_id }, checkpoint)),
                    Err(e) => {
                        self.checkpoints.unregister(CURSOR);
                        Err(e)
                    }
                }
            };
            match ready.await {
                Ok(v) => return v,
                Err(e) => {
                    tracing::error!("clickhouse: reattach failed: {e:#}; retrying in {backoff:?}");
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(Duration::from_secs(60));
                }
            }
        }
    }

    pub fn spawn(self) -> std::io::Result<std::thread::JoinHandle<()>> {
        std::thread::Builder::new().name("clickhouse".into()).spawn(move || {
            match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(rt) => rt.block_on(self.run()),
                Err(e) => tracing::error!("clickhouse: runtime failed to start: {e}"),
            }
        })
    }

    async fn connect(&self) -> MySqlPool {
        let mut backoff = Duration::from_secs(1);
        loop {
            match self.mysql.pool(2).await {
                Ok(pool) => return pool,
                Err(e) => {
                    tracing::error!("clickhouse: MySQL connect failed: {e:#}; retrying in {backoff:?}");
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(Duration::from_secs(60));
                }
            }
        }
    }

    async fn run(self) {
        let client = Client {
            http: reqwest::Client::new(),
            cfg: self.cfg.clone(),
        };
        self.stats.set(State::Starting);
        tokio::spawn(poll_server(client.clone(), self.stats.clone()));
        let pool = self.connect().await;
        let mut start = self.start;
        let mut checkpoint = self.checkpoint.clone();
        loop {
            match self.follow(&client, &pool, start, &checkpoint).await {
                Halt::Fatal(why) => {
                    self.stats.set(State::Stopped);
                    self.checkpoints.unregister(CURSOR);
                    tracing::error!("clickhouse: stopped: {why}");
                    return;
                }
                Halt::Detach(why) => {
                    self.stats.set(State::Detached);
                    self.checkpoints.unregister(CURSOR);
                    tracing::error!("clickhouse: {why}; detached from the WAL, will refill from MySQL");
                    (start, checkpoint) = self.reattach(&client, &pool).await;
                    self.stats.set(State::Starting);
                    tracing::info!("clickhouse: reattached at WAL {} / post {}", start.seq, start.post_id);
                }
            }
        }
    }
}

pub async fn backfill_command(cfg: ClickhouseConfig, pool: MySqlPool) -> Result<()> {
    let client = Client {
        http: reqwest::Client::new(),
        cfg: cfg.clone(),
    };
    let threads = prepare(&client).await?;
    let upto = crate::db::max_post_id(&pool).await?;
    let never = || None;
    let stats = Stats::default();
    let backfill = Backfill {
        client: &client,
        pool: &pool,
        chunk: cfg.backfill_chunk,
        rate: cfg.backfill_rate,
        halt: &never,
        stats: &stats,
    };
    match backfill.run(threads, upto).await {
        Ok(done) => {
            println!(
                "backfilled {} posts up to {upto}, {} gap repairs",
                done.posts, done.repairs
            );
            Ok(())
        }
        Err(Halt::Fatal(why) | Halt::Detach(why)) => bail!("backfill stopped: {why}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(id: u64, uuid: &str) -> Plain {
        Plain {
            id,
            uuid: uuid.into(),
            comment: Some("hi".into()),
            has_comment: true,
            raw_text: "5hi".into(),
            ..Plain::default()
        }
    }

    #[test]
    fn comment_delete_applies_to_a_buffered_post() {
        let mut b = Batch::default();
        b.posts.push(plain(7, "u7"));
        b.delete_comment(CommentDelete {
            id: None,
            uuid: "u7".into(),
            raw_text: "5".into(),
        });
        assert!(b.deletes.is_empty());
        assert_eq!(b.posts[0].comment, None);
        assert!(b.posts[0].is_comment_deleted);
        assert_eq!(b.posts[0].raw_text, "5");
    }

    fn post(thread_id: u16, id: u64, valid: bool) -> Plain {
        Plain {
            id,
            thread_id,
            uuid: format!("u{id}"),
            timestamp: format!("{}", 1_700_000_000_000 + id),
            time_since_last_post: Some("1".into()),
            time_since_last_count: Some("1".into()),
            is_count: true,
            is_valid_count: Some(valid),
            valid_count_number: valid.then_some(id as i64),
            ..Plain::default()
        }
    }

    #[test]
    fn replayed_posts_are_written_once() {
        let posts = [post(1, 1, true), post(1, 2, true), post(2, 3, true)];
        let first = encode_batch(&HashMap::new(), &posts);
        let replay = [post(1, 2, true), post(2, 3, true), post(1, 4, false)];
        let second = encode_batch(&first.threads, &replay);
        assert_eq!(first.rows.lines().count(), 3);
        assert_eq!(second.rows.lines().count(), 1);
        assert_eq!(second.authors.lines().count(), 1);
        assert_eq!(second.threads[&1].last_id, 4);
        assert_eq!(second.threads[&1].ctx.streak, -1);
        assert_eq!(second.threads[&1].ctx.pos, 2);
    }

    #[test]
    fn a_failed_flush_leaves_thread_state_untouched() {
        let start = HashMap::new();
        let encoded = encode_batch(&start, &[post(1, 1, true)]);
        assert!(start.is_empty());
        assert_eq!(encoded.threads[&1].last_id, 1);
    }

    #[test]
    fn metrics_are_silent_while_the_writer_is_off() {
        let stats = Stats::default();
        let mut out = String::new();
        stats.render(&mut out);
        assert!(out.is_empty());
        stats.set(State::Following);
        stats.render(&mut out);
        assert!(out.contains(
            "cgg_clickhouse_state 3
"
        ));
        assert!(out.contains(
            "cgg_clickhouse_visible_seconds_count 0
"
        ));
    }

    #[test]
    fn visibility_is_measured_from_the_post_timestamp() {
        let made = format!("{}.5", unix_ms() - 2_000);
        let d = visible_after(&made).unwrap();
        assert!((1_900..2_200).contains(&d.as_millis()), "{d:?}");
        assert_eq!(visible_after("not a time"), None);
    }

    #[test]
    fn comment_delete_for_an_older_post_is_queued() {
        let mut b = Batch::default();
        b.delete_comment(CommentDelete {
            id: Some(3),
            uuid: "u3".into(),
            raw_text: String::new(),
        });
        assert_eq!(b.deletes.len(), 1);
    }
}
