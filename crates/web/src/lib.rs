pub mod ids;
mod levels;
pub mod nest;
pub mod reactions;
pub mod session;

pub use levels::level_of;

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}
