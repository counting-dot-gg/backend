use crate::config::WalMode;
use crate::model::Post;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::future::Future;
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t")]
pub enum WalRecord {
    Post {
        post: Post,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        roll: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        chance: Option<f64>,
        #[serde(default)]
        origin: Origin,
    },
    ThreadBans {
        thread_id: u32,
        count_bans: Vec<String>,
        post_bans: Vec<String>,
    },
    PostUpdate {
        uuid: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reactions: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        deleted_comment: Option<DeletedComment>,
    },
    ThreadFlags {
        thread_id: u32,
        thread_of_the_day: bool,
    },
    BingoGame(BingoGameRow),
    BingoMember(BingoMemberRow),
    BingoSquare(BingoSquareRow),
    BingoProgress(BingoProgressRow),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BingoGameRow {
    pub id: i64,
    pub status: String,
    pub mode: String,
    pub rank: String,
    pub allow_join: bool,
    pub join_mode: String,
    pub team_swap_mode: String,
    pub join_code: Option<String>,
    pub time_limit_seconds: i64,
    pub started_at: i64,
    pub ends_at: i64,
    pub grace_ends_at: i64,
    pub finished_at: i64,
    pub winner_user: Option<String>,
    pub winner_team: Option<i64>,
    pub winner_at: i64,
    pub winner_team_size: i64,
    pub board_seed: Option<String>,
    pub min_time_seconds: Option<i64>,
    #[serde(default)]
    pub line_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BingoMemberRow {
    pub id: i64,
    pub team_id: i64,
    pub role: String,
    pub status: String,
    pub left_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BingoSquareRow {
    pub id: i64,
    pub params: Option<String>,
    pub progress_value: i64,
    pub unlocked: bool,
    pub unlocker_user: Option<String>,
    pub unlocker_team: Option<i64>,
    pub unlock_at: i64,
    pub last_progress_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BingoProgressRow {
    pub square_id: i64,
    pub user: String,
    pub value: i64,
    pub at: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(tag = "k")]
pub enum Origin {
    #[default]
    Socket,
    Discord {
        message_id: Option<String>,
    },
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeletedComment {
    pub raw_text: String,
}

pub type Action = Pin<Box<dyn Future<Output = ()> + Send>>;

pub struct Append {
    pub payload: Vec<u8>,
    pub action: Option<Action>,
    pub lane: u32,
}

pub struct Committed {
    seq: AtomicU64,
    pub notify: tokio::sync::Notify,
}

impl Committed {
    pub fn new(seq: u64) -> Committed {
        Committed {
            seq: AtomicU64::new(seq),
            notify: tokio::sync::Notify::new(),
        }
    }

    pub fn get(&self) -> u64 {
        self.seq.load(Ordering::Acquire)
    }

    pub fn set(&self, seq: u64) {
        self.seq.store(seq, Ordering::Release);
        self.notify.notify_waiters();
    }
}

#[derive(Default)]
pub struct WalStats {
    pub batches: AtomicU64,
    pub records: AtomicU64,
    pub bytes: AtomicU64,
    pub syncs: AtomicU64,
    pub write_us: AtomicU64,
    pub max_batch: AtomicU64,
    pub wait: crate::metrics::Histogram,
    pub sync: crate::metrics::Histogram,
    pub handoff: crate::metrics::Histogram,
}

#[derive(Clone)]
pub struct Wal {
    tx: mpsc::SyncSender<(Append, Instant)>,
    pub committed: Arc<Committed>,
    pub stats: Arc<WalStats>,
}

const HEADER: usize = 16;

fn segment_name(first_seq: u64) -> String {
    format!("wal-{first_seq:020}.log")
}

pub fn segments(dir: &Path) -> Result<Vec<(u64, PathBuf)>> {
    let mut v = Vec::new();
    for e in fs::read_dir(dir)? {
        let p = e?.path();
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if let Some(n) = name.strip_prefix("wal-").and_then(|n| n.strip_suffix(".log"))
            && let Ok(seq) = n.parse()
        {
            v.push((seq, p));
        }
    }
    v.sort();
    Ok(v)
}

fn frame(seq: u64, payload: &[u8], out: &mut Vec<u8>) {
    let mut h = crc32fast::Hasher::new();
    h.update(&seq.to_le_bytes());
    h.update(payload);
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&h.finalize().to_le_bytes());
    out.extend_from_slice(&seq.to_le_bytes());
    out.extend_from_slice(payload);
}

#[cfg(unix)]
fn sync_dir(dir: &Path) -> std::io::Result<()> {
    File::open(dir)?.sync_all()
}
#[cfg(not(unix))]
fn sync_dir(_dir: &Path) -> std::io::Result<()> {
    Ok(())
}

fn write_zeros(f: &mut File, mut bytes: u64) -> std::io::Result<()> {
    let zeros = vec![0u8; 1 << 20];
    while bytes > 0 {
        let n = bytes.min(zeros.len() as u64) as usize;
        f.write_all(&zeros[..n])?;
        bytes -= n as u64;
    }
    Ok(())
}

fn preallocate(path: &Path, bytes: u64) -> std::io::Result<File> {
    let mut f = OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(path)?;
    write_zeros(&mut f, bytes)?;
    f.sync_all()?;
    f.seek(SeekFrom::Start(0))?;
    Ok(f)
}

fn logical_end(path: &Path) -> Result<(u64, Option<u64>)> {
    let mut r = BufReader::new(File::open(path)?);
    let (mut off, mut last) = (0, None);
    while let FrameRead::Frame(seq, len) = read_frame(&mut r)? {
        off += (HEADER + len) as u64;
        last = Some(seq);
    }
    Ok((off, last))
}

const SPARE_PREFIX: &str = "wal-spare-";

struct Spares {
    req: mpsc::Sender<()>,
    ready: mpsc::Receiver<PathBuf>,
}

impl Spares {
    fn start(dir: PathBuf, bytes: u64) -> Result<Spares> {
        for e in fs::read_dir(&dir)? {
            let p = e?.path();
            if p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(SPARE_PREFIX))
            {
                let _ = fs::remove_file(&p);
            }
        }
        let (req, req_rx) = mpsc::channel::<()>();
        let (ready_tx, ready) = mpsc::channel();
        std::thread::Builder::new().name("wal-prealloc".into()).spawn(move || {
            for (n, ()) in req_rx.into_iter().enumerate() {
                let p = dir.join(format!("{SPARE_PREFIX}{n}.tmp"));
                match preallocate(&p, bytes) {
                    Ok(_) => {
                        if ready_tx.send(p).is_err() {
                            return;
                        }
                    }
                    Err(e) => tracing::warn!("wal: preallocating {}: {e}", p.display()),
                }
            }
        })?;
        let _ = req.send(());
        Ok(Spares { req, ready })
    }
}

pub fn recover(dir: &Path) -> Result<u64> {
    fs::create_dir_all(dir)?;
    let segs = segments(dir)?;
    let mut last = 0;
    for (i, (_, path)) in segs.iter().enumerate() {
        let newest = i + 1 == segs.len();
        let mut f = OpenOptions::new().read(true).write(true).open(path)?;
        let mut r = BufReader::new(&mut f);
        let mut off: u64 = 0;
        loop {
            match read_frame(&mut r)? {
                FrameRead::Frame(seq, payload_len) => {
                    if last != 0 && seq != last + 1 {
                        if off != 0 {
                            bail!("{}: seq {seq} follows {last} inside a segment", path.display());
                        }
                        tracing::warn!(
                            "wal: {} starts at seq {seq}; this log has nothing for {}..={}",
                            path.display(),
                            last + 1,
                            seq - 1
                        );
                    }
                    last = seq;
                    off += (HEADER + payload_len) as u64;
                }
                FrameRead::Eof => break,
                FrameRead::Torn | FrameRead::Corrupt => {
                    if !newest {
                        bail!("{}: damaged frame at {off} in a sealed segment", path.display());
                    }
                    drop(r);
                    tracing::warn!("wal: clearing torn tail of {} from {off}", path.display());
                    let len = f.metadata()?.len();
                    f.seek(SeekFrom::Start(off))?;
                    write_zeros(&mut f, len - off)?;
                    f.sync_all()?;
                    break;
                }
            }
        }
    }
    Ok(last)
}

enum FrameRead {
    Frame(u64, usize),
    Eof,
    Torn,
    Corrupt,
}

fn read_exact_or_eof(r: &mut impl Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..])? {
            0 => break,
            k => n += k,
        }
    }
    Ok(n)
}

fn is_end(h: &[u8; HEADER]) -> bool {
    h.iter().all(|&b| b == 0)
}

fn read_frame(r: &mut impl Read) -> Result<FrameRead> {
    let mut h = [0u8; HEADER];
    let n = read_exact_or_eof(r, &mut h)?;
    if n == 0 || (n == HEADER && is_end(&h)) {
        return Ok(FrameRead::Eof);
    }
    if n < HEADER {
        return Ok(FrameRead::Torn);
    }
    let len = u32::from_le_bytes(h[0..4].try_into().unwrap()) as usize;
    let crc = u32::from_le_bytes(h[4..8].try_into().unwrap());
    let seq = u64::from_le_bytes(h[8..16].try_into().unwrap());
    let mut payload = vec![0u8; len];
    if read_exact_or_eof(r, &mut payload)? < len {
        return Ok(FrameRead::Torn);
    }
    let mut c = crc32fast::Hasher::new();
    c.update(&seq.to_le_bytes());
    c.update(&payload);
    if c.finalize() != crc {
        return Ok(FrameRead::Corrupt);
    }
    Ok(FrameRead::Frame(seq, len))
}

pub struct Options {
    pub dir: PathBuf,
    pub mode: WalMode,
    pub sync_interval: Duration,
    pub segment_bytes: u64,
}

impl Wal {
    pub fn start(opts: Options, last_seq: u64, rt: tokio::runtime::Handle) -> Result<Wal> {
        let (tx, rx) = mpsc::sync_channel::<(Append, Instant)>(1 << 16);
        let committed = Arc::new(Committed {
            seq: AtomicU64::new(last_seq),
            notify: tokio::sync::Notify::new(),
        });
        let stats = Arc::new(WalStats::default());
        let (atx, mut arx) = tokio::sync::mpsc::unbounded_channel::<(Vec<(u32, Action)>, Instant)>();
        let st = stats.clone();
        let rt2 = rt.clone();
        rt.spawn(async move {
            let mut lanes: std::collections::HashMap<u32, tokio::sync::mpsc::UnboundedSender<Action>> =
                std::collections::HashMap::new();
            while let Some((batch, durable_at)) = arx.recv().await {
                st.handoff.record(durable_at.elapsed());
                for (lane, a) in batch {
                    let tx = lanes.entry(lane).or_insert_with(|| {
                        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Action>();
                        rt2.spawn(async move {
                            while let Some(a) = rx.recv().await {
                                a.await;
                            }
                        });
                        tx
                    });
                    let _ = tx.send(a);
                }
            }
        });
        let mut w = Writer::open(opts, last_seq + 1)?;
        let (c2, s2) = (committed.clone(), stats.clone());
        std::thread::Builder::new().name("wal-writer".into()).spawn(move || {
            if let Err(e) = w.run(rx, &c2, &s2, &atx) {
                tracing::error!("wal writer failed: {e:#}; exiting");
                std::process::exit(70);
            }
        })?;
        Ok(Wal { tx, committed, stats })
    }

    pub fn append(&self, a: Append) {
        if self.tx.send((a, Instant::now())).is_err() {
            tracing::error!("wal writer gone");
            std::process::exit(70);
        }
    }
}

struct Writer {
    opts: Options,
    spares: Spares,
    file: File,
    size: u64,
    next_seq: u64,
    last_sync: Instant,
    dirty: bool,
}

impl Writer {
    fn open(opts: Options, next_seq: u64) -> Result<Writer> {
        let spares = Spares::start(opts.dir.clone(), opts.segment_bytes)?;
        let segs = segments(&opts.dir)?;
        let newest = match segs.last() {
            Some((first, p)) => {
                let (end, last) = logical_end(p)?;
                let continues = last.map_or(*first == next_seq, |l| l + 1 == next_seq);
                continues.then(|| (p.clone(), end))
            }
            None => None,
        };
        let (file, size) = match newest {
            Some((p, end)) if end < opts.segment_bytes => {
                let mut f = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(&p)
                    .with_context(|| p.display().to_string())?;
                f.seek(SeekFrom::Start(end))?;
                (f, end)
            }
            _ => {
                let p = opts.dir.join(segment_name(next_seq));
                (
                    preallocate(&p, opts.segment_bytes).with_context(|| p.display().to_string())?,
                    0,
                )
            }
        };
        sync_dir(&opts.dir)?;
        Ok(Writer {
            opts,
            spares,
            file,
            size,
            next_seq,
            last_sync: Instant::now(),
            dirty: false,
        })
    }

    fn rotate(&mut self) -> Result<()> {
        self.file.sync_data()?;
        let path = self.opts.dir.join(segment_name(self.next_seq));
        self.file = match self.spares.ready.try_recv() {
            Ok(spare) => {
                fs::rename(&spare, &path)?;
                OpenOptions::new().read(true).write(true).open(&path)?
            }
            Err(_) => {
                tracing::warn!("wal: no preallocated segment ready; growing {}", path.display());
                OpenOptions::new().create(true).truncate(true).write(true).open(&path)?
            }
        };
        let _ = self.spares.req.send(());
        sync_dir(&self.opts.dir)?;
        self.size = 0;
        Ok(())
    }

    fn sync(&mut self, stats: &WalStats) -> Result<()> {
        self.file.sync_data()?;
        stats.syncs.fetch_add(1, Ordering::Relaxed);
        self.last_sync = Instant::now();
        self.dirty = false;
        Ok(())
    }

    fn run(
        &mut self,
        rx: mpsc::Receiver<(Append, Instant)>,
        committed: &Committed,
        stats: &WalStats,
        actions: &tokio::sync::mpsc::UnboundedSender<(Vec<(u32, Action)>, Instant)>,
    ) -> Result<()> {
        let mut buf = Vec::with_capacity(1 << 20);
        loop {
            let first = match self.opts.mode {
                WalMode::Durable => match rx.recv() {
                    Ok(a) => a,
                    Err(_) => return Ok(()),
                },
                WalMode::Fast => match rx.recv_timeout(self.opts.sync_interval) {
                    Ok(a) => a,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        if self.dirty {
                            self.sync(stats)?;
                        }
                        continue;
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
                },
            };
            let mut batch = vec![first];
            while batch.len() < 4096 {
                match rx.try_recv() {
                    Ok(a) => batch.push(a),
                    Err(_) => break,
                }
            }
            if self.size > 0 && self.size >= self.opts.segment_bytes {
                self.rotate()?;
            }
            let t0 = Instant::now();
            buf.clear();
            let mut acts = Vec::with_capacity(batch.len());
            for (a, queued) in batch.iter_mut() {
                stats.wait.record(t0 - *queued);
                frame(self.next_seq, &a.payload, &mut buf);
                self.next_seq += 1;
                if let Some(act) = a.action.take() {
                    acts.push((a.lane, act));
                }
            }
            self.file.write_all(&buf)?;
            self.size += buf.len() as u64;
            self.dirty = true;
            if self.opts.mode == WalMode::Durable || self.last_sync.elapsed() >= self.opts.sync_interval {
                self.sync(stats)?;
            }
            let durable_at = Instant::now();
            stats.sync.record(durable_at - t0);
            stats
                .write_us
                .fetch_add((durable_at - t0).as_micros() as u64, Ordering::Relaxed);
            stats.batches.fetch_add(1, Ordering::Relaxed);
            stats.records.fetch_add(batch.len() as u64, Ordering::Relaxed);
            stats.bytes.fetch_add(buf.len() as u64, Ordering::Relaxed);
            stats.max_batch.fetch_max(batch.len() as u64, Ordering::Relaxed);
            committed.seq.store(self.next_seq - 1, Ordering::Release);
            committed.notify.notify_waiters();
            if !acts.is_empty() {
                let _ = actions.send((acts, durable_at));
            }
        }
    }
}

enum Advance {
    Moved,
    Confirm,
    Stay,
}

pub struct Reader {
    dir: PathBuf,
    first_seq: u64,
    file: Option<BufReader<File>>,
    offset: u64,
    last: u64,
    started: bool,
}

impl Reader {
    pub fn open(dir: &Path, after_seq: u64) -> Result<Reader> {
        let mut r = Reader {
            dir: dir.to_owned(),
            first_seq: 0,
            file: None,
            offset: 0,
            last: after_seq,
            started: false,
        };
        let segs = segments(dir)?;
        let start = segs.iter().rev().find(|(s, _)| *s <= after_seq + 1).or(segs.first());
        if let Some((s, p)) = start {
            r.first_seq = *s;
            r.file = Some(BufReader::with_capacity(1 << 20, File::open(p)?));
        }
        while r.skip_one(after_seq)? {}
        Ok(r)
    }

    fn skip_one(&mut self, after: u64) -> Result<bool> {
        let Some(f) = self.file.as_mut() else { return Ok(false) };
        let pos = self.offset;
        let mut h = [0u8; HEADER];
        if read_exact_or_eof(f, &mut h)? < HEADER || is_end(&h) {
            f.seek(SeekFrom::Start(pos))?;
            return Ok(false);
        }
        let len = u32::from_le_bytes(h[0..4].try_into().unwrap()) as u64;
        let seq = u64::from_le_bytes(h[8..16].try_into().unwrap());
        if seq > after {
            f.seek(SeekFrom::Start(pos))?;
            return Ok(false);
        }
        f.seek_relative(len as i64)?;
        self.offset = pos + HEADER as u64 + len;
        Ok(true)
    }

    pub fn next(&mut self, upto: u64) -> Result<Option<(u64, Vec<u8>)>> {
        if upto <= self.last {
            return Ok(None);
        }
        let mut fresh = false;
        let mut gap_seen = false;
        loop {
            let Some(f) = self.file.as_mut() else {
                if let Some((s, p)) = segments(&self.dir)?.into_iter().next() {
                    self.first_seq = s;
                    self.file = Some(BufReader::with_capacity(1 << 20, File::open(p)?));
                    self.offset = 0;
                    continue;
                }
                return Ok(None);
            };
            let pos = self.offset;
            let mut h = [0u8; HEADER];
            let n = read_exact_or_eof(f, &mut h)?;
            let end = n == 0 || (n == HEADER && is_end(&h));
            if n < HEADER || end {
                f.seek(SeekFrom::Start(pos))?;
                if !fresh {
                    fresh = true;
                    continue;
                }
                if end {
                    match self.advance_segment(gap_seen)? {
                        Advance::Moved => {
                            fresh = false;
                            gap_seen = false;
                            continue;
                        }
                        Advance::Confirm => {
                            gap_seen = true;
                            continue;
                        }
                        Advance::Stay => {}
                    }
                }
                return Ok(None);
            }
            let len = u32::from_le_bytes(h[0..4].try_into().unwrap()) as usize;
            let crc = u32::from_le_bytes(h[4..8].try_into().unwrap());
            let seq = u64::from_le_bytes(h[8..16].try_into().unwrap());
            if seq > upto {
                f.seek(SeekFrom::Start(pos))?;
                return Ok(None);
            }
            let mut payload = vec![0u8; len];
            let complete = read_exact_or_eof(f, &mut payload)? == len && {
                let mut c = crc32fast::Hasher::new();
                c.update(&seq.to_le_bytes());
                c.update(&payload);
                c.finalize() == crc
            };
            if !complete {
                f.seek(SeekFrom::Start(pos))?;
                if !fresh {
                    fresh = true;
                    continue;
                }
                bail!("wal: damaged frame at seq {seq} (committed data)");
            }
            if seq != self.last + 1 {
                if self.started {
                    bail!("wal: seq {seq} follows {} in {}", self.last, self.dir.display());
                }
                tracing::warn!("wal: reader wanted seq {} but the log resumes at {seq}", self.last + 1);
            }
            self.started = true;
            self.offset = pos + (HEADER + len) as u64;
            self.last = seq;
            return Ok(Some((seq, payload)));
        }
    }

    fn advance_segment(&mut self, confirmed: bool) -> Result<Advance> {
        let next = segments(&self.dir)?.into_iter().find(|(s, _)| *s > self.first_seq);
        let Some((s, p)) = next else { return Ok(Advance::Stay) };
        if s != self.last + 1 {
            if !confirmed {
                return Ok(Advance::Confirm);
            }
            tracing::warn!(
                "wal: reader crossing a gap: {} has nothing for {}..={}",
                self.dir.display(),
                self.last + 1,
                s - 1
            );
            self.started = false;
        }
        self.first_seq = s;
        self.file = Some(BufReader::with_capacity(1 << 20, File::open(p)?));
        self.offset = 0;
        Ok(Advance::Moved)
    }
}

pub fn gc(dir: &Path, checkpoint: u64) -> Result<usize> {
    let segs = segments(dir)?;
    let mut removed = 0;
    for w in segs.windows(2) {
        let (_, path) = &w[0];
        let next_first = w[1].0;
        if next_first <= checkpoint + 1 {
            fs::remove_file(path)?;
            removed += 1;
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("cgg-wal-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn write_read_recover_rotate() {
        let dir = tmp("basic");
        let last = recover(&dir).unwrap();
        assert_eq!(last, 0);
        let opts = Options {
            dir: dir.clone(),
            mode: WalMode::Durable,
            sync_interval: Duration::from_millis(1),
            segment_bytes: 200,
        };
        let wal = Wal::start(opts, last, tokio::runtime::Handle::current()).unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut got = Vec::new();
        for i in 0..50u32 {
            let tx = tx.clone();
            wal.append(Append {
                payload: format!("record-{i}").into_bytes(),
                action: Some(Box::pin(async move {
                    tx.send(i).unwrap();
                })),
                lane: 0,
            });
            if i >= 25 {
                while got.len() <= i as usize {
                    got.push(rx.recv().await.unwrap());
                }
            }
        }
        assert_eq!(got, (0..50).collect::<Vec<_>>(), "actions run in order");
        assert_eq!(wal.committed.get(), 50);
        assert!(segments(&dir).unwrap().len() > 1, "rotated");

        let mut r = Reader::open(&dir, 20).unwrap();
        let (seq, p) = r.next(u64::MAX).unwrap().unwrap();
        assert_eq!((seq, p.as_slice()), (21, b"record-20".as_slice()));
        let mut n = 1;
        while r.next(u64::MAX).unwrap().is_some() {
            n += 1;
        }
        assert_eq!(n, 30);

        let (_, newest) = segments(&dir).unwrap().pop().unwrap();
        let (end, _) = logical_end(&newest).unwrap();
        let mut f = OpenOptions::new().write(true).open(&newest).unwrap();
        f.seek(SeekFrom::Start(end)).unwrap();
        f.write_all(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20])
            .unwrap();
        drop(f);
        assert_eq!(recover(&dir).unwrap(), 50);
        assert_eq!(logical_end(&newest).unwrap().0, end);

        assert!(gc(&dir, 40).unwrap() > 0);
        let mut r = Reader::open(&dir, 40).unwrap();
        assert_eq!(r.next(u64::MAX).unwrap().unwrap().0, 41);
    }

    fn record(seq: u64) -> Vec<u8> {
        let mut b = Vec::new();
        frame(seq, format!("record-{seq}").as_bytes(), &mut b);
        b
    }

    async fn append_n(wal: &Wal, n: u64) {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        for i in 0..n {
            let tx = tx.clone();
            wal.append(Append {
                payload: format!("r{i}").into_bytes(),
                action: Some(Box::pin(async move { tx.send(()).unwrap() })),
                lane: 0,
            });
        }
        for _ in 0..n {
            rx.recv().await.unwrap();
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn segments_are_preallocated_and_continued() {
        let dir = tmp("prealloc");
        let opts = || Options {
            dir: dir.clone(),
            mode: WalMode::Durable,
            sync_interval: Duration::from_millis(1),
            segment_bytes: 512,
        };
        let wal = Wal::start(opts(), recover(&dir).unwrap(), tokio::runtime::Handle::current()).unwrap();
        append_n(&wal, 10).await;
        let (_, seg) = segments(&dir).unwrap().pop().unwrap();
        assert_eq!(fs::metadata(&seg).unwrap().len(), 512, "full size from the start");
        let last = recover(&dir).unwrap();
        assert_eq!(last, 10);
        let wal = Wal::start(opts(), last, tokio::runtime::Handle::current()).unwrap();
        for _ in 0..20 {
            append_n(&wal, 10).await;
        }
        assert!(segments(&dir).unwrap().len() > 2);
        let mut r = Reader::open(&dir, 0).unwrap();
        let mut seqs = Vec::new();
        while let Some((seq, _)) = r.next(u64::MAX).unwrap() {
            seqs.push(seq);
        }
        assert_eq!(seqs, (1..=210).collect::<Vec<_>>());
        assert_eq!(recover(&dir).unwrap(), 210);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_stuck_lane_does_not_hold_up_another() {
        let dir = tmp("lanes");
        let opts = Options {
            dir: dir.clone(),
            mode: WalMode::Durable,
            sync_interval: Duration::from_millis(1),
            segment_bytes: 1 << 20,
        };
        let wal = Wal::start(opts, recover(&dir).unwrap(), tokio::runtime::Handle::current()).unwrap();
        let (release, held) = tokio::sync::oneshot::channel::<()>();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let t1 = tx.clone();
        wal.append(Append {
            payload: b"a".to_vec(),
            action: Some(Box::pin(async move {
                let _ = held.await;
                t1.send("1a").unwrap();
            })),
            lane: 1,
        });
        let t2 = tx.clone();
        wal.append(Append {
            payload: b"b".to_vec(),
            action: Some(Box::pin(async move {
                t2.send("1b").unwrap();
            })),
            lane: 1,
        });
        let t3 = tx.clone();
        wal.append(Append {
            payload: b"c".to_vec(),
            action: Some(Box::pin(async move {
                t3.send("2a").unwrap();
            })),
            lane: 2,
        });
        let first = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("lane 2 ran")
            .unwrap();
        assert_eq!(first, "2a");
        release.send(()).unwrap();
        assert_eq!(rx.recv().await.unwrap(), "1a");
        assert_eq!(rx.recv().await.unwrap(), "1b");
    }

    #[test]
    fn reader_does_not_skip_an_unseen_frame_at_rotation() {
        let dir = tmp("race");
        let a = dir.join(segment_name(1));
        let mut fa = preallocate(&a, 4096).unwrap();
        fa.write_all(&[record(1), record(2)].concat()).unwrap();
        let mut r = Reader::open(&dir, 0).unwrap();
        assert_eq!(r.next(10).unwrap().unwrap().0, 1);
        assert_eq!(r.next(10).unwrap().unwrap().0, 2);
        assert!(r.next(10).unwrap().is_none());
        fa.write_all(&record(3)).unwrap();
        let mut fb = preallocate(&dir.join(segment_name(4)), 4096).unwrap();
        fb.write_all(&record(4)).unwrap();
        assert_eq!(r.next(10).unwrap().unwrap().0, 3);
        assert_eq!(r.next(10).unwrap().unwrap().0, 4);
    }

    #[test]
    fn reader_crosses_a_real_gap_between_segments() {
        let dir = tmp("gap");
        let mut fa = preallocate(&dir.join(segment_name(1)), 4096).unwrap();
        fa.write_all(&[record(1), record(2)].concat()).unwrap();
        let mut fb = preallocate(&dir.join(segment_name(10)), 4096).unwrap();
        fb.write_all(&[record(10), record(11)].concat()).unwrap();
        let mut r = Reader::open(&dir, 0).unwrap();
        let seqs: Vec<u64> = std::iter::from_fn(|| r.next(u64::MAX).unwrap().map(|(s, _)| s)).collect();
        assert_eq!(seqs, vec![1, 2, 10, 11]);
        assert_eq!(recover(&dir).unwrap(), 11);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_log_resumed_later_starts_a_new_segment() {
        let dir = tmp("resume-later");
        let mut fa = preallocate(&dir.join(segment_name(1)), 4096).unwrap();
        fa.write_all(&[record(1), record(2)].concat()).unwrap();
        drop(fa);
        assert_eq!(recover(&dir).unwrap(), 2);
        let opts = Options {
            dir: dir.clone(),
            mode: WalMode::Durable,
            sync_interval: Duration::from_millis(1),
            segment_bytes: 4096,
        };
        let wal = Wal::start(opts, 9, tokio::runtime::Handle::current()).unwrap();
        append_n(&wal, 3).await;
        let firsts: Vec<u64> = segments(&dir).unwrap().into_iter().map(|(s, _)| s).collect();
        assert_eq!(firsts, vec![1, 10]);
        assert_eq!(recover(&dir).unwrap(), 12);
        let mut r = Reader::open(&dir, 0).unwrap();
        let seqs: Vec<u64> = std::iter::from_fn(|| r.next(u64::MAX).unwrap().map(|(s, _)| s)).collect();
        assert_eq!(seqs, vec![1, 2, 10, 11, 12]);
    }

    #[test]
    fn reader_sees_frames_written_after_its_buffer_filled() {
        let dir = tmp("stale");
        let mut f = preallocate(&dir.join(segment_name(1)), 4096).unwrap();
        f.write_all(&record(1)).unwrap();
        let mut r = Reader::open(&dir, 0).unwrap();
        assert_eq!(r.next(1).unwrap().unwrap().0, 1);
        f.write_all(&record(2)).unwrap();
        assert_eq!(r.next(2).unwrap().unwrap().0, 2);
    }

    #[test]
    fn a_cursor_older_than_the_log_resumes_at_its_start() {
        let dir = tmp("resume");
        let mut f = preallocate(&dir.join(segment_name(1045)), 4096).unwrap();
        f.write_all(&[record(1045), record(1046)].concat()).unwrap();
        let mut r = Reader::open(&dir, 100).unwrap();
        assert_eq!(r.next(u64::MAX).unwrap().unwrap().0, 1045);
        assert_eq!(r.next(u64::MAX).unwrap().unwrap().0, 1046);
    }
}
