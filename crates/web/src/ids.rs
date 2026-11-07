use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub struct Clock {
    base_ns: u64,
    start: Instant,
    last: AtomicU64,
}

impl Default for Clock {
    fn default() -> Self {
        let base_ns = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos() as u64;
        Clock {
            base_ns,
            start: Instant::now(),
            last: AtomicU64::new(0),
        }
    }
}

impl Clock {
    pub fn now_ns(&self) -> u64 {
        let t = self.base_ns + self.start.elapsed().as_nanos() as u64;
        let mut prev = self.last.load(Ordering::Relaxed);
        loop {
            let next = t.max(prev + 1);
            match self
                .last
                .compare_exchange_weak(prev, next, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => return next,
                Err(p) => prev = p,
            }
        }
    }

    pub fn now_ms(&self) -> f64 {
        self.now_ns() as f64 / 1e6
    }
}

pub fn uuid_for(ns: u64) -> String {
    let r = fastrand::f64();
    let s = format!("{ns:019}{}", (r * 9e4) as u32 + 10000);
    let v = ['8', '9', 'a'][(r * 3.0) as usize];
    format!(
        "{}-{}-4{}-{}{}-{}{}",
        &s[0..8],
        &s[8..12],
        &s[12..15],
        v,
        &s[15..18],
        &s[18..24],
        &s[12..18]
    )
}

pub fn timestamp_for(ns: u64) -> String {
    counting::js::num_to_string((ns / 1000) as f64 / 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shape() {
        let u = uuid_for(1791321226163219123);
        assert_eq!(u.len(), 36);
        assert!(u.starts_with("17913212-2616-4321-"));
        assert_eq!(&u[30..36], format!("{}{}", &u[15..18], &u[20..23]));
        assert!(matches!(&u[19..20], "8" | "9" | "a"));
        assert_eq!(timestamp_for(1791321226163219123), "1791321226163.219");
        assert_eq!(timestamp_for(1791321226163200000), "1791321226163.2");
    }
}
