use crate::model::Post;
use crate::wal::{self, Committed, Reader, WalRecord};
use anyhow::{Context, Result};
use sqlx::{MySql, MySqlPool, QueryBuilder};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct PersistStats {
    pub checkpoint: AtomicU64,
    pub batches: AtomicU64,
    pub rows: AtomicU64,
    pub failures: AtomicU64,
    pub last_commit_us: AtomicU64,
}

pub async fn ensure_checkpoint_table(pool: &MySqlPool) -> Result<u64> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS cgg_wal_checkpoint (id TINYINT PRIMARY KEY, seq BIGINT UNSIGNED NOT NULL, \
         updatedAt TIMESTAMP(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3) ON UPDATE CURRENT_TIMESTAMP(3))",
    )
    .execute(pool)
    .await?;
    sqlx::query("INSERT IGNORE INTO cgg_wal_checkpoint (id, seq) VALUES (1, 0)")
        .execute(pool)
        .await?;
    let seq: u64 = sqlx::query_scalar("SELECT seq FROM cgg_wal_checkpoint WHERE id = 1")
        .fetch_one(pool)
        .await?;
    Ok(seq)
}

const INSERT_COLS: &str = "INSERT INTO post (id, uuid, timestamp, timeSinceLastCount, timeSinceLastPost, rawText, \
    isCount, isValidCount, validCountNumber, countContent, rawCount, stricken, thread, threadId, hasComment, \
    hasThreeCharComment, comment, authorUUID, authorId, isDeleted, isCommentDeleted, reactions, macroPresetId, \
    macroPresetVersionId) ";

async fn insert_posts(tx: &mut sqlx::Transaction<'_, MySql>, posts: &[Post]) -> Result<()> {
    for chunk in posts.chunks(500) {
        let mut q: QueryBuilder<MySql> = QueryBuilder::new(INSERT_COLS);
        q.push_values(chunk, |mut b, p| {
            b.push_bind(p.id)
                .push_bind(&p.uuid)
                .push_bind(&p.timestamp)
                .push_bind(&p.time_since_last_count)
                .push_bind(&p.time_since_last_post)
                .push_bind(&p.raw_text)
                .push_bind(p.is_count)
                .push_bind(p.is_valid_count)
                .push_bind(p.valid_count_number)
                .push_bind(&p.count_content)
                .push_bind(&p.raw_count)
                .push_bind(p.stricken)
                .push_bind(&p.thread)
                .push_bind(p.thread_id)
                .push_bind(p.has_comment)
                .push_bind(p.has_three_char_comment)
                .push_bind(&p.comment)
                .push_bind(&p.author_uuid)
                .push_bind(p.author_id)
                .push_bind(p.is_deleted)
                .push_bind(p.is_comment_deleted)
                .push_bind(p.reactions.to_string())
                .push_bind(p.macro_preset_id)
                .push_bind(p.macro_preset_version_id);
        });
        q.build().execute(&mut **tx).await?;
    }
    Ok(())
}

fn bingo_key(r: &WalRecord) -> Option<(u8, i64, &str)> {
    match r {
        WalRecord::BingoGame(g) => Some((0, g.id, "")),
        WalRecord::BingoMember(m) => Some((1, m.id, "")),
        WalRecord::BingoSquare(s) => Some((2, s.id, "")),
        WalRecord::BingoProgress(p) => Some((3, p.square_id, p.user.as_str())),
        _ => None,
    }
}

async fn apply_bingo(tx: &mut sqlx::Transaction<'_, MySql>, r: &WalRecord) -> Result<()> {
    match r {
        WalRecord::BingoGame(g) => {
            sqlx::query(
                "UPDATE bingogame SET status = ?, mode = ?, `rank` = ?, allowJoin = ?, joinMode = ?, teamSwapMode = ?, joinCode = ?, \
                 timeLimitSeconds = ?, startedAt = ?, endsAt = ?, graceEndsAt = ?, finishedAt = ?, winnerUserUuid = ?, \
                 winnerTeamId = ?, winnerAt = ?, winnerTeamSize = ?, boardSeed = ?, minTimeSeconds = ?, lineAt = ? WHERE id = ?",
            )
            .bind(&g.status)
            .bind(&g.mode)
            .bind(&g.rank)
            .bind(g.allow_join)
            .bind(&g.join_mode)
            .bind(&g.team_swap_mode)
            .bind(&g.join_code)
            .bind(g.time_limit_seconds)
            .bind(g.started_at)
            .bind(g.ends_at)
            .bind(g.grace_ends_at)
            .bind(g.finished_at)
            .bind(&g.winner_user)
            .bind(g.winner_team)
            .bind(g.winner_at)
            .bind(g.winner_team_size)
            .bind(&g.board_seed)
            .bind(g.min_time_seconds)
            .bind(g.line_at)
            .bind(g.id)
            .execute(&mut **tx)
            .await?;
        }
        WalRecord::BingoMember(m) => {
            sqlx::query("UPDATE bingogamemember SET teamId = ?, role = ?, status = ?, leftAt = ? WHERE id = ?")
                .bind(m.team_id)
                .bind(&m.role)
                .bind(&m.status)
                .bind(m.left_at)
                .bind(m.id)
                .execute(&mut **tx)
                .await?;
        }
        WalRecord::BingoSquare(s) => {
            sqlx::query(
                "UPDATE bingogamesquare SET params = ?, progressValue = ?, isUnlocked = ?, mostRecentUnlockerUserUuid = ?, \
                 mostRecentUnlockerTeamId = ?, mostRecentUnlockAt = ?, lastProgressAt = ? WHERE id = ?",
            )
            .bind(&s.params)
            .bind(s.progress_value)
            .bind(s.unlocked)
            .bind(&s.unlocker_user)
            .bind(s.unlocker_team)
            .bind(s.unlock_at)
            .bind(s.last_progress_at)
            .bind(s.id)
            .execute(&mut **tx)
            .await?;
        }
        WalRecord::BingoProgress(p) => {
            let id: Option<i32> = sqlx::query_scalar(
                "SELECT id FROM bingosquareuserprogress WHERE squareId = ? AND userUuid = ? LIMIT 1",
            )
            .bind(p.square_id)
            .bind(&p.user)
            .fetch_optional(&mut **tx)
            .await?;
            match id {
                Some(id) => {
                    sqlx::query(
                        "UPDATE bingosquareuserprogress SET progressValue = ?, lastProgressAt = ? WHERE id = ?",
                    )
                    .bind(p.value)
                    .bind(p.at)
                    .bind(id)
                    .execute(&mut **tx)
                    .await?;
                }
                None => {
                    sqlx::query("INSERT INTO bingosquareuserprogress (squareId, userUuid, progressValue, lastProgressAt) VALUES (?, ?, ?, ?)")
                        .bind(p.square_id)
                        .bind(&p.user)
                        .bind(p.value)
                        .bind(p.at)
                        .execute(&mut **tx)
                        .await?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

async fn apply(pool: &MySqlPool, records: &[(u64, WalRecord)]) -> Result<()> {
    let mut tx = pool.begin().await?;
    let mut posts: Vec<Post> = Vec::new();
    let mut last: std::collections::HashMap<(u8, i64, &str), usize> = std::collections::HashMap::new();
    for (i, (_, r)) in records.iter().enumerate() {
        if let Some(k) = bingo_key(r) {
            last.insert(k, i);
        }
    }
    for (i, (_, r)) in records.iter().enumerate() {
        match r {
            WalRecord::Post { post, .. } => posts.push(post.clone()),
            WalRecord::BingoGame(_)
            | WalRecord::BingoMember(_)
            | WalRecord::BingoSquare(_)
            | WalRecord::BingoProgress(_) => {
                if bingo_key(r).and_then(|k| last.get(&k)) == Some(&i) {
                    apply_bingo(&mut tx, r).await?;
                }
            }
            other => {
                insert_posts(&mut tx, &posts).await?;
                posts.clear();
                match other {
                    WalRecord::ThreadBans {
                        thread_id,
                        count_bans,
                        post_bans,
                    } => {
                        sqlx::query("UPDATE thread SET countBans = ?, postBans = ? WHERE id = ?")
                            .bind(serde_json::to_string(count_bans)?)
                            .bind(serde_json::to_string(post_bans)?)
                            .bind(thread_id)
                            .execute(&mut *tx)
                            .await?;
                    }
                    WalRecord::PostUpdate {
                        uuid,
                        id,
                        reactions,
                        deleted_comment,
                    } => {
                        let id = match id {
                            Some(id) => Some(*id as i64),
                            None => sqlx::query_scalar::<_, i32>("SELECT id FROM post WHERE uuid = ? LIMIT 1")
                                .bind(uuid)
                                .fetch_optional(&mut *tx)
                                .await?
                                .map(i64::from),
                        };
                        if let Some(id) = id {
                            if let Some(r) = reactions {
                                sqlx::query("UPDATE post SET reactions = ? WHERE id = ?")
                                    .bind(r.to_string())
                                    .bind(id)
                                    .execute(&mut *tx)
                                    .await?;
                            }
                            if let Some(d) = deleted_comment {
                                sqlx::query(
                                    "UPDATE post SET hasComment = 0, isCommentDeleted = 1, comment = NULL, rawText = ? WHERE id = ?",
                                )
                                .bind(&d.raw_text)
                                .bind(id)
                                .execute(&mut *tx)
                                .await?;
                            }
                        }
                    }
                    WalRecord::ThreadFlags {
                        thread_id,
                        thread_of_the_day,
                    } => {
                        sqlx::query("UPDATE thread SET threadOfTheDay = ? WHERE id = ?")
                            .bind(thread_of_the_day)
                            .bind(thread_id)
                            .execute(&mut *tx)
                            .await?;
                    }
                    WalRecord::Post { .. }
                    | WalRecord::BingoGame(_)
                    | WalRecord::BingoMember(_)
                    | WalRecord::BingoSquare(_)
                    | WalRecord::BingoProgress(_) => unreachable!(),
                }
            }
        }
    }
    insert_posts(&mut tx, &posts).await?;
    let last = records.last().map(|r| r.0).unwrap_or(0);
    sqlx::query("UPDATE cgg_wal_checkpoint SET seq = ? WHERE id = 1")
        .bind(last)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

pub struct Persister {
    pub pool: MySqlPool,
    pub dir: PathBuf,
    pub batch: usize,
    pub stats: Arc<PersistStats>,
    pub persisted: Arc<Committed>,
}

impl Persister {
    fn read_batch(&self, reader: &mut Reader, upto: u64) -> Result<Vec<(u64, WalRecord)>> {
        let mut out = Vec::new();
        while out.len() < self.batch {
            match reader.next(upto)? {
                Some((seq, payload)) => {
                    let rec: WalRecord =
                        serde_json::from_slice(&payload).with_context(|| format!("wal record {seq} does not parse"))?;
                    out.push((seq, rec));
                }
                None => break,
            }
        }
        Ok(out)
    }

    pub async fn drain_backlog(&self, checkpoint: u64, last_on_disk: u64) -> Result<u64> {
        let mut reader = Reader::open(&self.dir, checkpoint)?;
        let mut cp = checkpoint;
        loop {
            let batch = self.read_batch(&mut reader, last_on_disk)?;
            if batch.is_empty() {
                break;
            }
            self.apply_with_retry(&batch).await;
            cp = batch.last().unwrap().0;
        }
        if cp > checkpoint {
            tracing::info!("persister: replayed WAL {}..={cp} into MySQL", checkpoint + 1);
        }
        Ok(cp)
    }

    async fn apply_with_retry(&self, batch: &[(u64, WalRecord)]) {
        let mut backoff = Duration::from_millis(50);
        loop {
            let t0 = Instant::now();
            match apply(&self.pool, batch).await {
                Ok(()) => {
                    let last = batch.last().unwrap().0;
                    self.stats.checkpoint.store(last, Ordering::Relaxed);
                    self.persisted.set(last);
                    let mut top: std::collections::HashMap<u32, i64> = std::collections::HashMap::new();
                    for (_, r) in batch {
                        if let WalRecord::Post { post, .. } = r
                            && let (true, Some(n)) = (post.is_valid_count, post.valid_count_number)
                        {
                            let e = top.entry(post.thread_id).or_insert(n);
                            *e = (*e).max(n);
                        }
                    }
                    for (thread_id, n) in top {
                        crate::persisted::published(thread_id, n);
                    }
                    self.stats.batches.fetch_add(1, Ordering::Relaxed);
                    self.stats.rows.fetch_add(batch.len() as u64, Ordering::Relaxed);
                    self.stats
                        .last_commit_us
                        .store(t0.elapsed().as_micros() as u64, Ordering::Relaxed);
                    return;
                }
                Err(e) => {
                    self.stats.failures.fetch_add(1, Ordering::Relaxed);
                    tracing::error!(
                        "persister: batch {}..={} failed: {e:#}; retrying in {backoff:?}",
                        batch[0].0,
                        batch.last().unwrap().0
                    );
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(Duration::from_secs(10));
                }
            }
        }
    }

    pub async fn run(
        self,
        committed: Arc<Committed>,
        checkpoint: u64,
        others: Arc<crate::cursor::Checkpoints>,
    ) -> Result<()> {
        let mut reader = Reader::open(&self.dir, checkpoint)?;
        let mut last_gc = Instant::now();
        loop {
            let notified = committed.notify.notified();
            let upto = committed.get();
            let batch = self.read_batch(&mut reader, upto)?;
            if batch.is_empty() {
                let _ = tokio::time::timeout(Duration::from_millis(100), notified).await;
                continue;
            }
            if let Some(ms) = std::env::var("PERSIST_DELAY_MS")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
            {
                tokio::time::sleep(Duration::from_millis(ms)).await;
            }
            self.apply_with_retry(&batch).await;
            if last_gc.elapsed() > Duration::from_secs(10) {
                let cp = self.stats.checkpoint.load(Ordering::Relaxed).min(others.min());
                if let Ok(n) = wal::gc(&self.dir, cp)
                    && n > 0
                {
                    tracing::info!("wal: removed {n} applied segment(s)");
                }
                last_gc = Instant::now();
            }
        }
    }
}
