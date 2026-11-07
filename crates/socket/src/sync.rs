use crate::app::App;
use crate::db;
use crate::model::ThreadMeta;
use crate::thread::{self, Msg};
use std::sync::Arc;
use std::time::Duration;

pub const INTERVAL: Duration = Duration::from_secs(2);

pub async fn run(app: Arc<App>, ctx: Arc<thread::Ctx>) {
    let mut tick = tokio::time::interval(INTERVAL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        if let Err(e) = threads(&app, &ctx).await {
            tracing::warn!("thread sync: {e:#}");
        }
        if let Err(e) = counters(&app).await {
            tracing::warn!("counter sync: {e:#}");
        }
    }
}

fn same(a: &ThreadMeta, b: &ThreadMeta) -> bool {
    a.to_json() == b.to_json()
}

async fn threads(app: &Arc<App>, ctx: &Arc<thread::Ctx>) -> anyhow::Result<()> {
    for meta in db::load_threads(&app.pool).await? {
        match app.thread_by_uuid(&meta.uuid) {
            Some(h) => {
                let current = h.meta.read().clone();
                if !same(&current, &meta) {
                    let _ = h.tx.send(Msg::Sync(Box::new(meta))).await;
                }
            }
            None => {
                tracing::info!("new thread {}: starting its actor", meta.name);
                let boot = db::thread_boot(&app.pool, &meta, ctx.clock.now_ms()).await?;
                let lv = match boot.recent_valid.first() {
                    Some(p) => app.counter(&p.author_uuid).await,
                    None => None,
                };
                let st = thread::boot_state(&meta, boot, lv);
                let (name, uuid, id) = (meta.name.clone(), meta.uuid.clone(), meta.id);
                let h = thread::spawn(meta, st, ctx.clone());
                app.threads.insert(name, h.clone());
                app.threads_by_uuid.insert(uuid.clone(), h.clone());
                app.threads_by_id.insert(id, h);
                app.daily.lock().threads.entry(uuid).or_default();
            }
        }
    }
    Ok(())
}

async fn counters(app: &App) -> anyhow::Result<()> {
    let cached: Vec<String> = app.counters.iter().map(|e| e.key().clone()).collect();
    for chunk in cached.chunks(500) {
        for c in db::counters_by_uuids(&app.pool, chunk).await? {
            let changed = app
                .counters
                .get(&c.uuid)
                .is_none_or(|old| old.json.get() != c.json.get());
            if changed {
                app.counters.insert(c.uuid.clone(), Arc::new(c));
            }
        }
    }
    Ok(())
}
