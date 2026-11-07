use crate::app::{App, now_ms};
use chrono::{Datelike, TimeZone};
use serde_json::{Value, json};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

const DEFAULT_DAYS: [u32; 1] = [5];

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Config {
    pub days: Vec<u32>,
    pub over: Option<(bool, Option<i64>)>,
}

impl Config {
    pub fn parse(raw: Option<&str>) -> Config {
        let v: Value = raw.and_then(|r| serde_json::from_str(r).ok()).unwrap_or(Value::Null);
        let days = match v.get("days").and_then(|d| d.as_array()) {
            Some(a) => a
                .iter()
                .filter_map(|d| d.as_u64())
                .filter(|d| *d < 7)
                .map(|d| d as u32)
                .collect(),
            None => DEFAULT_DAYS.to_vec(),
        };
        let over = v.get("override").filter(|o| o.is_object()).map(|o| {
            (
                o["open"].as_bool().unwrap_or(false),
                o["until"].as_f64().map(|u| u as i64),
            )
        });
        Config { days, over }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct State {
    pub open: bool,
    pub next: Option<i64>,
}

fn day_at(ms: i64) -> (i64, u32, i64) {
    let (start, end) = crate::daily::eastern_day_bounds(ms);
    let wd = chrono_tz::America::New_York
        .timestamp_millis_opt(ms)
        .unwrap()
        .weekday()
        .num_days_from_sunday();
    (start, wd, end)
}

fn scheduled(days: &[u32], now: i64) -> State {
    let open_on = |ms: i64| days.contains(&day_at(ms).1);
    let open = open_on(now);
    if days.is_empty() || days.len() == 7 {
        return State { open, next: None };
    }
    let mut t = day_at(now).2;
    for _ in 0..8 {
        if open_on(t) != open {
            return State { open, next: Some(t) };
        }
        t = day_at(t).2;
    }
    State { open, next: None }
}

pub fn state(c: &Config, now: i64) -> State {
    match c.over {
        Some((open, until)) if until.is_none_or(|u| u > now) => {
            let next = until.and_then(|u| {
                let after = scheduled(&c.days, u);
                if after.open != open { Some(u) } else { after.next }
            });
            State { open, next }
        }
        _ => scheduled(&c.days, now),
    }
}

static CACHE: LazyLock<parking_lot::Mutex<(Config, Option<Instant>)>> =
    LazyLock::new(|| parking_lot::Mutex::new((Config::parse(None), None)));

pub async fn config(app: &App) -> Config {
    let stale = CACHE.lock().1.is_none_or(|t| t.elapsed() > Duration::from_secs(5));
    if stale {
        let raw: Option<Option<String>> =
            sqlx::query_scalar("SELECT config FROM feature_flag WHERE flagKey = 'bingo' LIMIT 1")
                .fetch_optional(&app.pool)
                .await
                .ok()
                .flatten();
        *CACHE.lock() = (Config::parse(raw.flatten().as_deref()), Some(Instant::now()));
    }
    CACHE.lock().0.clone()
}

pub async fn now_state(app: &App) -> State {
    state(&config(app).await, now_ms())
}

pub fn json(s: &State) -> Value {
    json!({ "open": s.open, "next": s.next })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn et(y: i32, mo: u32, d: u32, h: u32) -> i64 {
        chrono_tz::America::New_York
            .with_ymd_and_hms(y, mo, d, h, 0, 0)
            .unwrap()
            .timestamp_millis()
    }

    #[test]
    fn fridays_by_default() {
        let c = Config::parse(None);
        assert_eq!(c.days, vec![5]);
        assert_eq!(
            state(&c, et(2026, 10, 13, 12)),
            State {
                open: false,
                next: Some(et(2026, 10, 16, 0))
            }
        );
        assert_eq!(
            state(&c, et(2026, 10, 16, 20)),
            State {
                open: true,
                next: Some(et(2026, 10, 17, 0))
            }
        );
    }

    #[test]
    fn overrides_open_and_close_until_a_time() {
        let tue = et(2026, 10, 13, 12);
        let open = Config::parse(Some(r#"{"days":[5],"override":{"open":true,"until":null}}"#));
        assert_eq!(state(&open, tue), State { open: true, next: None });
        let until = et(2026, 10, 14, 18);
        let c = Config {
            days: vec![5],
            over: Some((true, Some(until))),
        };
        assert_eq!(
            state(&c, tue),
            State {
                open: true,
                next: Some(until)
            }
        );
        assert_eq!(
            state(&c, until + 1),
            State {
                open: false,
                next: Some(et(2026, 10, 16, 0))
            }
        );
        let c = Config {
            days: vec![5],
            over: Some((true, Some(et(2026, 10, 16, 3)))),
        };
        assert_eq!(state(&c, tue).next, Some(et(2026, 10, 17, 0)));
        let c = Config {
            days: vec![5],
            over: Some((false, Some(et(2026, 10, 16, 18)))),
        };
        assert_eq!(
            state(&c, et(2026, 10, 16, 9)),
            State {
                open: false,
                next: Some(et(2026, 10, 16, 18))
            }
        );
    }

    #[test]
    fn a_friday_lobby_was_made_in_the_window() {
        let c = Config::parse(None);
        assert!(state(&c, et(2026, 10, 16, 23)).open);
        assert!(!state(&c, et(2026, 10, 17, 0)).open);
    }

    #[test]
    fn neighbouring_days_run_together() {
        let c = Config {
            days: vec![5, 6, 0],
            over: None,
        };
        assert_eq!(state(&c, et(2026, 10, 16, 9)).next, Some(et(2026, 10, 19, 0)));
        assert_eq!(
            state(
                &Config {
                    days: vec![],
                    over: None
                },
                et(2026, 10, 16, 9)
            ),
            State {
                open: false,
                next: None
            }
        );
    }
}
