use crate::state::AppState;
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

pub const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(5);
const CHECK_EVERY: Duration = Duration::from_secs(5);
const OLDEST_SHOWN: usize = 10;

#[derive(Default)]
pub struct InFlight {
    next: AtomicU64,
    open: Mutex<HashMap<u64, (String, Instant)>>,
}

pub struct Ticket<'a> {
    owner: &'a InFlight,
    id: u64,
}

impl Drop for Ticket<'_> {
    fn drop(&mut self) {
        self.owner.open.lock().unwrap().remove(&self.id);
    }
}

impl InFlight {
    pub fn enter(&self, what: String) -> Ticket<'_> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        self.open.lock().unwrap().insert(id, (what, Instant::now()));
        Ticket { owner: self, id }
    }

    fn oldest(&self) -> String {
        let open = self.open.lock().unwrap();
        let mut all: Vec<_> = open.values().map(|(what, at)| (at.elapsed(), what.as_str())).collect();
        all.sort_by_key(|entry| std::cmp::Reverse(entry.0));
        let shown: Vec<String> = all
            .iter()
            .take(OLDEST_SHOWN)
            .map(|(age, what)| format!("{what} ({}s)", age.as_secs()))
            .collect();
        format!("{} in flight; oldest: [{}]", all.len(), shown.join(", "))
    }
}

fn starved(state: &AppState) -> bool {
    state.pool.size() >= state.pool.options().get_max_connections() && state.pool.num_idle() == 0
}

pub fn start(state: &AppState) {
    let s = state.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(CHECK_EVERY);
        let mut since: Option<Instant> = None;
        loop {
            tick.tick().await;
            if !starved(&s) {
                if let Some(at) = since.take() {
                    tracing::info!("mysql pool recovered after {}s", at.elapsed().as_secs());
                }
                continue;
            }
            if since.is_none() {
                tracing::warn!("mysql pool exhausted: {}", s.in_flight.oldest());
                since = Some(Instant::now());
            }
        }
    });
}
