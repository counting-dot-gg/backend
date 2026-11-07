use serde_json::Value;
use std::sync::Mutex;

#[derive(Default)]
pub struct Caches {
    pub pref_stats: Mutex<Option<Value>>,
    pub counter_role_total: std::sync::atomic::AtomicI64,
}
