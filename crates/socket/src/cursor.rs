use crate::wal::{Committed, Reader, WalRecord};
use anyhow::{Context, Result};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Position {
    pub seq: u64,
    pub post_id: u64,
}

pub struct CursorFile {
    path: PathBuf,
}

impl CursorFile {
    pub fn new(dir: &Path, name: &str) -> CursorFile {
        CursorFile {
            path: dir.join(format!("{name}.cursor")),
        }
    }

    pub fn load(&self) -> Option<Position> {
        std::fs::read(&self.path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
    }

    pub fn store(&self, p: Position) -> Result<()> {
        let tmp = self.path.with_extension("cursor.tmp");
        std::fs::write(&tmp, serde_json::to_vec(&p)?)?;
        std::fs::rename(&tmp, &self.path).with_context(|| self.path.display().to_string())?;
        Ok(())
    }
}

#[derive(Default)]
pub struct Checkpoints {
    list: Mutex<Vec<(String, Arc<AtomicU64>)>>,
}

impl Checkpoints {
    pub fn register(&self, name: &str, at: u64) -> Arc<AtomicU64> {
        let a = Arc::new(AtomicU64::new(at));
        self.list.lock().push((name.to_owned(), a.clone()));
        a
    }

    pub fn min(&self) -> u64 {
        self.list
            .lock()
            .iter()
            .map(|(_, a)| a.load(Ordering::Relaxed))
            .min()
            .unwrap_or(u64::MAX)
    }

    pub fn snapshot(&self) -> Vec<(String, u64)> {
        self.list
            .lock()
            .iter()
            .map(|(n, a)| (n.clone(), a.load(Ordering::Relaxed)))
            .collect()
    }
}

const SAVE_EVERY: Duration = Duration::from_millis(20);

fn save(file: &CursorFile, pos: Position, checkpoint: &AtomicU64, name: &str) {
    if let Err(e) = file.store(pos) {
        tracing::error!("{name}: cursor save failed: {e:#}");
    }
    checkpoint.store(pos.seq, Ordering::Relaxed);
}

pub async fn follow<F, Fut>(
    dir: PathBuf,
    name: &'static str,
    committed: Arc<Committed>,
    start: Position,
    checkpoint: Arc<AtomicU64>,
    mut handle: F,
) -> Result<()>
where
    F: FnMut(u64, WalRecord) -> Fut,
    Fut: Future<Output = ()>,
{
    let file = CursorFile::new(&dir, name);
    let mut reader = Reader::open(&dir, start.seq)?;
    let mut pos = start;
    let mut last_save = std::time::Instant::now();
    loop {
        let notified = committed.notify.notified();
        let upto = committed.get();
        let mut n = 0;
        while n < 256 {
            let Some((seq, payload)) = reader.next(upto)? else {
                break;
            };
            n += 1;
            match serde_json::from_slice::<WalRecord>(&payload) {
                Ok(rec) => {
                    if let WalRecord::Post { post, .. } = &rec {
                        pos.post_id = pos.post_id.max(post.id);
                    }
                    handle(seq, rec).await;
                }
                Err(e) => tracing::error!("{name}: WAL record {seq} unreadable: {e}"),
            }
            pos.seq = seq;
            if last_save.elapsed() >= SAVE_EVERY {
                save(&file, pos, &checkpoint, name);
                last_save = std::time::Instant::now();
            }
        }
        if n > 0 {
            save(&file, pos, &checkpoint, name);
            last_save = std::time::Instant::now();
            continue;
        }
        let _ = tokio::time::timeout(Duration::from_millis(250), notified).await;
    }
}
