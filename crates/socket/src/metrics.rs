use std::fmt::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

const BUCKETS: usize = 4 * 27;

pub struct Histogram {
    counts: [AtomicU64; BUCKETS],
    sum_ns: AtomicU64,
    n: AtomicU64,
    max_ns: AtomicU64,
    recent: [(AtomicU64, AtomicU64); SLOTS],
}

const SLOTS: usize = 6;
const SLOT_SECS: u64 = 10;

fn epoch_now() -> u64 {
    static START: std::sync::LazyLock<std::time::Instant> = std::sync::LazyLock::new(std::time::Instant::now);
    START.elapsed().as_secs() / SLOT_SECS + 1
}

impl Default for Histogram {
    fn default() -> Self {
        Histogram {
            counts: std::array::from_fn(|_| AtomicU64::new(0)),
            sum_ns: AtomicU64::new(0),
            n: AtomicU64::new(0),
            max_ns: AtomicU64::new(0),
            recent: std::array::from_fn(|_| (AtomicU64::new(0), AtomicU64::new(0))),
        }
    }
}

fn bucket_of(ns: u64) -> usize {
    let us = (ns / 1000).max(1);
    let lz = 63 - us.leading_zeros() as usize;
    let frac = if lz >= 2 {
        ((us >> (lz - 2)) & 3) as usize
    } else {
        ((us << (2 - lz)) & 3) as usize
    };
    (lz * 4 + frac).min(BUCKETS - 1)
}

fn bucket_upper_us(b: usize) -> f64 {
    let (p, f) = (b / 4, b % 4);
    (1u64 << p) as f64 * (1.0 + (f as f64 + 1.0) / 4.0)
}

impl Histogram {
    pub fn record(&self, d: Duration) {
        let ns = d.as_nanos() as u64;
        self.counts[bucket_of(ns)].fetch_add(1, Ordering::Relaxed);
        self.sum_ns.fetch_add(ns, Ordering::Relaxed);
        self.n.fetch_add(1, Ordering::Relaxed);
        self.max_ns.fetch_max(ns, Ordering::Relaxed);
        let e = epoch_now();
        let (slot_epoch, slot_max) = &self.recent[(e % SLOTS as u64) as usize];
        if slot_epoch.swap(e, Ordering::Relaxed) != e {
            slot_max.store(0, Ordering::Relaxed);
        }
        slot_max.fetch_max(ns, Ordering::Relaxed);
    }

    pub fn max_last_minute_ns(&self) -> u64 {
        let e = epoch_now();
        self.recent
            .iter()
            .filter(|(se, _)| {
                let se = se.load(Ordering::Relaxed);
                se + SLOTS as u64 > e
            })
            .map(|(_, m)| m.load(Ordering::Relaxed))
            .max()
            .unwrap_or(0)
    }

    pub fn quantile_us(&self, q: f64) -> f64 {
        let n = self.n.load(Ordering::Relaxed);
        if n == 0 {
            return 0.0;
        }
        let target = ((n as f64) * q).ceil() as u64;
        let mut acc = 0;
        for (i, c) in self.counts.iter().enumerate() {
            acc += c.load(Ordering::Relaxed);
            if acc >= target {
                return bucket_upper_us(i);
            }
        }
        bucket_upper_us(BUCKETS - 1)
    }

    pub fn write(&self, out: &mut String, name: &str, help: &str) {
        let _ = writeln!(out, "# HELP {name} {help}\n# TYPE {name} histogram");
        let mut acc = 0;
        for (i, c) in self.counts.iter().enumerate() {
            acc += c.load(Ordering::Relaxed);
            let _ = writeln!(out, "{name}_bucket{{le=\"{}\"}} {acc}", bucket_upper_us(i) / 1e6);
        }
        let _ = writeln!(out, "{name}_bucket{{le=\"+Inf\"}} {acc}");
        let _ = writeln!(out, "{name}_sum {}", self.sum_ns.load(Ordering::Relaxed) as f64 / 1e9);
        let _ = writeln!(out, "{name}_count {acc}");
        let _ = writeln!(
            out,
            "# TYPE {name}_max_1m gauge\n{name}_max_1m {}",
            self.max_last_minute_ns() as f64 / 1e9
        );
        let _ = writeln!(out, "# TYPE {name}_boot gauge");
        for q in [0.5, 0.9, 0.99, 0.999, 0.9999] {
            let _ = writeln!(out, "{name}_boot{{quantile=\"{q}\"}} {}", self.quantile_us(q) / 1e6);
        }
        let _ = writeln!(
            out,
            "# TYPE {name}_boot_max gauge\n{name}_boot_max {}",
            self.max_ns.load(Ordering::Relaxed) as f64 / 1e9
        );
    }
}

#[derive(Default)]
pub struct Metrics {
    pub queue: Histogram,
    pub process: Histogram,
    pub wal: Histogram,
    pub total: Histogram,
    pub broadcast: Histogram,
    pub effects: Histogram,
    pub client_round_trip: Histogram,
    pub client_processing: Histogram,
    pub client_unaccounted: Histogram,
    pub client_uplink: Histogram,
    pub client_downlink: Histogram,
    pub client_ping: Histogram,
    pub client_loop_lag: Histogram,
    pub client_paint: Histogram,
    pub client_blocked: Histogram,
    pub client_reports_ws: AtomicU64,
    pub client_reports_polling: AtomicU64,
    pub client_counts: AtomicU64,
    pub posts: AtomicU64,
    pub rejected: AtomicU64,
    pub rate_limited: AtomicU64,
    pub sockets: AtomicU64,
    pub slow_reports: [AtomicU64; 13],
    pub disconnects: [AtomicU64; 8],
}

impl Metrics {
    pub fn render(&self, extra: &[(&str, f64)]) -> String {
        let mut s = String::new();
        self.queue
            .write(&mut s, "cgg_post_queue_seconds", "ingress to actor pickup");
        self.process
            .write(&mut s, "cgg_post_process_seconds", "parse, validate and build");
        self.wal.write(&mut s, "cgg_post_wal_seconds", "WAL append to commit");
        self.total
            .write(&mut s, "cgg_post_total_seconds", "ingress to broadcast");
        self.broadcast
            .write(&mut s, "cgg_post_broadcast_seconds", "post emit to the thread room");
        self.effects
            .write(&mut s, "cgg_post_effects_seconds", "side effects of one post");
        self.client_round_trip.write(
            &mut s,
            "cgg_client_round_trip_seconds",
            "browser-reported p95 round trip, per report",
        );
        self.client_processing.write(
            &mut s,
            "cgg_client_processing_seconds",
            "server share of it, per report",
        );
        self.client_unaccounted.write(
            &mut s,
            "cgg_client_unaccounted_seconds",
            "the rest: network, queueing, browser",
        );
        self.client_uplink.write(
            &mut s,
            "cgg_client_uplink_seconds",
            "browser emit to server ingress, per report p95",
        );
        self.client_downlink.write(
            &mut s,
            "cgg_client_downlink_seconds",
            "server broadcast to browser handler, per report p95",
        );
        self.client_ping.write(
            &mut s,
            "cgg_client_ping_seconds",
            "no-work echo round trip, per report p95",
        );
        self.client_loop_lag.write(
            &mut s,
            "cgg_client_loop_lag_seconds",
            "browser main-thread task delay, per report p95",
        );
        self.client_paint.write(
            &mut s,
            "cgg_client_paint_seconds",
            "post handler to next frame, per report p95",
        );
        self.client_blocked.write(
            &mut s,
            "cgg_client_blocked_seconds",
            "browser long tasks per report window",
        );
        let _ = writeln!(
            s,
            "cgg_client_reports_total{{transport=\"websocket\"}} {}",
            self.client_reports_ws.load(Ordering::Relaxed)
        );
        let _ = writeln!(
            s,
            "cgg_client_reports_total{{transport=\"polling\"}} {}",
            self.client_reports_polling.load(Ordering::Relaxed)
        );
        let _ = writeln!(s, "# TYPE cgg_client_slow_reports_total counter");
        for (c, n) in crate::diagnosis::Cause::ALL.iter().zip(&self.slow_reports) {
            let _ = writeln!(
                s,
                "cgg_client_slow_reports_total{{cause=\"{}\"}} {}",
                c.key(),
                n.load(Ordering::Relaxed)
            );
        }
        let _ = writeln!(s, "# TYPE cgg_disconnects_total counter");
        for (k, n) in crate::connection::DISCONNECT_KEYS.iter().zip(&self.disconnects) {
            let _ = writeln!(
                s,
                "cgg_disconnects_total{{reason=\"{k}\"}} {}",
                n.load(Ordering::Relaxed)
            );
        }
        for (k, v) in [
            ("cgg_posts_total", self.posts.load(Ordering::Relaxed) as f64),
            ("cgg_posts_rejected_total", self.rejected.load(Ordering::Relaxed) as f64),
            (
                "cgg_rate_limited_total",
                self.rate_limited.load(Ordering::Relaxed) as f64,
            ),
            ("cgg_sockets", self.sockets.load(Ordering::Relaxed) as f64),
            (
                "cgg_client_counts_total",
                self.client_counts.load(Ordering::Relaxed) as f64,
            ),
        ]
        .iter()
        .chain(extra)
        {
            let _ = writeln!(s, "{k} {v}");
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantiles() {
        let h = Histogram::default();
        for us in 1..=1000u64 {
            h.record(Duration::from_micros(us));
        }
        let p50 = h.quantile_us(0.5);
        assert!((450.0..=650.0).contains(&p50), "{p50}");
        assert!(h.quantile_us(0.99) >= 990.0);
        assert_eq!(h.max_last_minute_ns(), 1_000_000);
        let mut out = String::new();
        h.write(&mut out, "t", "test");
        assert!(out.contains("t_bucket{le=\"+Inf\"} 1000\n"));
        assert!(out.contains("t_count 1000\n"));
        assert!(out.contains("t_max_1m 0.001\n"));
        assert!(!out.contains('\r'));
    }
}
