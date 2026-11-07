use crate::bench::Metrics;

pub struct Rule {
    pub metric: &'static str,
    pub higher_is_better: bool,
    pub max_regression_pct: f64,
    pub ignore_below: f64,
}

pub const SOCKET_RULES: [Rule; 6] = [
    Rule {
        metric: "single.p50_ms",
        higher_is_better: false,
        max_regression_pct: 20.0,
        ignore_below: 0.1,
    },
    Rule {
        metric: "single.p99_ms",
        higher_is_better: false,
        max_regression_pct: 50.0,
        ignore_below: 0.5,
    },
    Rule {
        metric: "parallel.posts_per_s",
        higher_is_better: true,
        max_regression_pct: 20.0,
        ignore_below: 50.0,
    },
    Rule {
        metric: "parallel.p50_ms",
        higher_is_better: false,
        max_regression_pct: 20.0,
        ignore_below: 0.1,
    },
    Rule {
        metric: "parallel.p99_ms",
        higher_is_better: false,
        max_regression_pct: 50.0,
        ignore_below: 0.5,
    },
    Rule {
        metric: "parallel.persist_drain_ms",
        higher_is_better: false,
        max_regression_pct: 100.0,
        ignore_below: 250.0,
    },
];

pub const API_RULES: [Rule; 6] = [
    Rule {
        metric: "single.p50_ms",
        higher_is_better: false,
        max_regression_pct: 25.0,
        ignore_below: 0.2,
    },
    Rule {
        metric: "single.p99_ms",
        higher_is_better: false,
        max_regression_pct: 50.0,
        ignore_below: 1.0,
    },
    Rule {
        metric: "parallel.req_per_s",
        higher_is_better: true,
        max_regression_pct: 20.0,
        ignore_below: 200.0,
    },
    Rule {
        metric: "parallel.p50_ms",
        higher_is_better: false,
        max_regression_pct: 25.0,
        ignore_below: 0.5,
    },
    Rule {
        metric: "parallel.p99_ms",
        higher_is_better: false,
        max_regression_pct: 50.0,
        ignore_below: 2.0,
    },
    Rule {
        metric: "rss_mb",
        higher_is_better: false,
        max_regression_pct: 50.0,
        ignore_below: 20.0,
    },
];

pub const BOT_RULES: [Rule; 6] = [
    Rule {
        metric: "single.p50_ms",
        higher_is_better: false,
        max_regression_pct: 25.0,
        ignore_below: 0.2,
    },
    Rule {
        metric: "single.p99_ms",
        higher_is_better: false,
        max_regression_pct: 50.0,
        ignore_below: 1.0,
    },
    Rule {
        metric: "burst.msgs_per_s",
        higher_is_better: true,
        max_regression_pct: 35.0,
        ignore_below: 2000.0,
    },
    Rule {
        metric: "burst.p99_ms",
        higher_is_better: false,
        max_regression_pct: 75.0,
        ignore_below: 25.0,
    },
    Rule {
        metric: "mirror.drain_ms",
        higher_is_better: false,
        max_regression_pct: 50.0,
        ignore_below: 100.0,
    },
    Rule {
        metric: "rss_mb",
        higher_is_better: false,
        max_regression_pct: 50.0,
        ignore_below: 20.0,
    },
];

pub struct Row {
    pub metric: &'static str,
    pub base: f64,
    pub head: f64,
    pub change_pct: f64,
    pub regressed: bool,
}

pub fn compare(rules: &[Rule], base: &Metrics, head: &Metrics) -> Vec<Row> {
    rules
        .iter()
        .filter_map(|rule| {
            let (b, h) = (*base.get(rule.metric)?, *head.get(rule.metric)?);
            let change_pct = if b == 0.0 { 0.0 } else { (h - b) / b * 100.0 };
            let worse_by = if rule.higher_is_better { b - h } else { h - b };
            let worse_pct = if rule.higher_is_better { -change_pct } else { change_pct };
            let regressed = worse_by > rule.ignore_below && worse_pct > rule.max_regression_pct;
            Some(Row {
                metric: rule.metric,
                base: b,
                head: h,
                change_pct,
                regressed,
            })
        })
        .collect()
}

pub fn markdown(rows: &[Row]) -> String {
    let mut s = String::from("| metric | base | head | change | |\n|---|---:|---:|---:|---|\n");
    for r in rows {
        let mark = if r.regressed { "regressed" } else { "" };
        s.push_str(&format!(
            "| `{}` | {:.2} | {:.2} | {:+.1}% | {mark} |\n",
            r.metric, r.base, r.head, r.change_pct
        ));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(pairs: &[(&str, f64)]) -> Metrics {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    #[test]
    fn slower_latency_beyond_threshold_regresses() {
        let rows = compare(
            &SOCKET_RULES,
            &m(&[("single.p50_ms", 2.0)]),
            &m(&[("single.p50_ms", 3.0)]),
        );
        assert!(rows[0].regressed);
    }

    #[test]
    fn tiny_absolute_changes_never_regress() {
        let rows = compare(
            &SOCKET_RULES,
            &m(&[("single.p50_ms", 0.05)]),
            &m(&[("single.p50_ms", 0.1)]),
        );
        assert!(!rows[0].regressed);
    }

    #[test]
    fn lower_throughput_regresses_and_higher_does_not() {
        let base = m(&[("parallel.posts_per_s", 1000.0)]);
        assert!(compare(&SOCKET_RULES, &base, &m(&[("parallel.posts_per_s", 700.0)]))[0].regressed);
        assert!(!compare(&SOCKET_RULES, &base, &m(&[("parallel.posts_per_s", 1500.0)]))[0].regressed);
    }
}
