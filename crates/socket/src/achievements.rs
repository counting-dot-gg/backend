use serde_json::json;
use socketioxide::SocketIo;
use sqlx::{MySqlPool, Row};
use stats::achievements::{AchievementRef, Award, LiveEvent};
use std::collections::HashMap;
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub enum Ach {
    Id(i32),
    Name(String),
}

enum Cmd {
    Award {
        counter: String,
        ach: Ach,
    },
    Progress {
        counter: String,
        ach: Ach,
        add: i64,
        overwrite: bool,
    },
}

#[derive(Clone)]
pub struct Achievements {
    tx: mpsc::UnboundedSender<Cmd>,
}

impl Achievements {
    pub fn spawn(pool: MySqlPool, io: SocketIo) -> Achievements {
        let (tx, mut rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let mut w = Worker {
                pool,
                io,
                defs: HashMap::new(),
                by_name: HashMap::new(),
            };
            while let Some(cmd) = rx.recv().await {
                let r = match cmd {
                    Cmd::Award { counter, ach } => w.award(&counter, &ach).await,
                    Cmd::Progress {
                        counter,
                        ach,
                        add,
                        overwrite,
                    } => w.progress(&counter, &ach, add, overwrite).await,
                };
                if let Err(e) = r {
                    tracing::warn!("achievement: {e:#}");
                }
            }
        });
        Achievements { tx }
    }

    pub fn record(&self, counter: &str, event: LiveEvent) {
        for award in progression::live_awards(&event) {
            let cmd = match award {
                Award::Earn(r) => Cmd::Award {
                    counter: counter.into(),
                    ach: r.into(),
                },
                Award::Progress(r, add) => Cmd::Progress {
                    counter: counter.into(),
                    ach: r.into(),
                    add,
                    overwrite: false,
                },
            };
            let _ = self.tx.send(cmd);
        }
    }
}

impl From<AchievementRef> for Ach {
    fn from(r: AchievementRef) -> Ach {
        match r {
            AchievementRef::Id(id) => Ach::Id(id),
            AchievementRef::Name(name) => Ach::Name(name.into()),
        }
    }
}

#[derive(Clone)]
struct Def {
    id: i32,
    max_progress: i64,
}

struct Worker {
    pool: MySqlPool,
    io: SocketIo,
    defs: HashMap<i32, Def>,
    by_name: HashMap<String, i32>,
}

impl Worker {
    async fn def(&mut self, ach: &Ach) -> anyhow::Result<Def> {
        let id = match ach {
            Ach::Id(i) => Some(*i),
            Ach::Name(n) => self.by_name.get(n).copied(),
        };
        if let Some(d) = id.and_then(|i| self.defs.get(&i)) {
            return Ok(d.clone());
        }
        let row = match ach {
            Ach::Id(i) => {
                sqlx::query("SELECT id, name, maxProgress FROM achievements WHERE id = ?")
                    .bind(i)
                    .fetch_optional(&self.pool)
                    .await?
            }
            Ach::Name(n) => {
                sqlx::query("SELECT id, name, maxProgress FROM achievements WHERE name = ? LIMIT 1")
                    .bind(n)
                    .fetch_optional(&self.pool)
                    .await?
            }
        };
        let Some(r) = row else {
            anyhow::bail!("achievement {ach:?} not found")
        };
        let d = Def {
            id: r.get("id"),
            max_progress: r.get::<i32, _>("maxProgress") as i64,
        };
        self.by_name.insert(r.get("name"), d.id);
        self.defs.insert(d.id, d.clone());
        Ok(d)
    }

    async fn award(&mut self, counter: &str, ach: &Ach) -> anyhow::Result<()> {
        let d = self.def(ach).await?;
        let existing: Option<(i32, i8)> = sqlx::query_as(
            "SELECT id, isComplete FROM counter_achievements WHERE counterUUID = ? AND achievementId = ? LIMIT 1",
        )
        .bind(counter)
        .bind(d.id)
        .fetch_optional(&self.pool)
        .await?;
        let now = crate::app::now_ms().to_string();
        match existing {
            Some((_, done)) if done != 0 => return Ok(()),
            Some((row, _)) => {
                sqlx::query(
                    "UPDATE counter_achievements SET isComplete = 1, lastChecked = 1, progress = ?, timestamp = ? WHERE id = ?",
                )
                .bind(d.max_progress)
                .bind(&now)
                .bind(row)
                .execute(&self.pool)
                .await?;
            }
            None => {
                sqlx::query(
                    "INSERT INTO counter_achievements (counterUUID, achievementId, isComplete, lastChecked, progress, timestamp) \
                     VALUES (?, ?, 1, 1, ?, ?)",
                )
                .bind(counter)
                .bind(d.id)
                .bind(d.max_progress)
                .bind(&now)
                .execute(&self.pool)
                .await?;
            }
        }
        sqlx::query("UPDATE achievements SET countersEarned = countersEarned + 1 WHERE id = ?")
            .bind(d.id)
            .execute(&self.pool)
            .await?;
        if let Some(r) = sqlx::query(
            "SELECT id, name, icon, isPublic, description, countersEarned, maxProgress FROM achievements WHERE id = ?",
        )
        .bind(d.id)
        .fetch_optional(&self.pool)
        .await?
        {
            let v = json!({
                "id": r.get::<i32, _>("id"),
                "name": r.get::<String, _>("name"),
                "icon": r.get::<Option<String>, _>("icon"),
                "isPublic": r.get::<i8, _>("isPublic") != 0,
                "description": r.get::<String, _>("description"),
                "countersEarned": r.get::<i32, _>("countersEarned"),
                "maxProgress": r.get::<i32, _>("maxProgress"),
            });
            let _ = self.io.to(format!("counter_{counter}")).emit("achievement", &v).await;
        }
        Ok(())
    }

    async fn progress(&mut self, counter: &str, ach: &Ach, add: i64, overwrite: bool) -> anyhow::Result<()> {
        let d = self.def(ach).await?;
        let existing: Option<(i32, i8, i32)> = sqlx::query_as(
            "SELECT id, isComplete, progress FROM counter_achievements WHERE counterUUID = ? AND achievementId = ? LIMIT 1",
        )
        .bind(counter)
        .bind(d.id)
        .fetch_optional(&self.pool)
        .await?;
        match existing {
            Some((_, done, _)) if done != 0 => Ok(()),
            Some((row, _, p)) => {
                let progress = if overwrite { add } else { p as i64 + add };
                if progress >= d.max_progress {
                    return Box::pin(self.award(counter, &Ach::Id(d.id))).await;
                }
                sqlx::query("UPDATE counter_achievements SET lastChecked = 1, progress = ? WHERE id = ?")
                    .bind(progress)
                    .bind(row)
                    .execute(&self.pool)
                    .await?;
                Ok(())
            }
            None => {
                if add >= d.max_progress {
                    return Box::pin(self.award(counter, &Ach::Id(d.id))).await;
                }
                sqlx::query(
                    "INSERT INTO counter_achievements (counterUUID, achievementId, isComplete, lastChecked, progress) \
                     VALUES (?, ?, 0, 1, ?)",
                )
                .bind(counter)
                .bind(d.id)
                .bind(add)
                .execute(&self.pool)
                .await?;
                Ok(())
            }
        }
    }
}
