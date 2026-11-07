use crate::ValidationType as V;
use crate::helpers as h;
use crate::js::{len16, num_to_string, parse_int};
use crate::seedrandom::SeedRandom;
use chrono::{TimeZone, Timelike};
use chrono_tz::Tz;

#[derive(Debug, Clone, Default)]
pub struct PrevCount {
    pub raw_count: Option<String>,
    pub author: u64,
    pub valid_count_number: Option<i64>,
    pub timestamp: f64,
    pub time_since_last_count: f64,
}

#[derive(Debug, Clone, Copy)]
pub struct ThreadRules<'a> {
    pub vt: V,
    pub first_valid_count: &'a str,
    pub allow_double_posts: bool,
    pub reset_on_mistakes: bool,
}

pub struct Env<'a> {
    pub now_ms: f64,
    pub server_tz: Tz,
    pub random: &'a mut dyn FnMut() -> f64,
    pub secret: Option<&'a str>,
}

pub struct Input<'a> {
    pub author: u64,
    pub is_bot: bool,
    pub username_len16: usize,
    pub count_banned: bool,
    pub raw_count: Option<&'a str>,
    pub last_valid: Option<&'a PrevCount>,
    pub recent_valid: [Option<&'a PrevCount>; 2],
    pub post_timestamp: f64,
    pub time_since_last_count: f64,
    pub author_counted_within_hour: bool,
    pub newest_valid_count_number: Option<Option<i64>>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Verdict {
    pub is_valid: bool,
    pub valid_count_number: Option<i64>,
    pub roll: Option<(f64, f64)>,
}

fn pi(s: Option<&str>) -> f64 {
    s.map_or(f64::NAN, |s| parse_int(s, None))
}

fn starts0(s: Option<&str>) -> bool {
    s.is_some_and(|s| s.starts_with('0'))
}

fn vcn(p: &PrevCount) -> f64 {
    p.valid_count_number.unwrap_or(0) as f64
}

fn eq(raw: Option<&str>, expected: Option<&str>) -> bool {
    matches!((raw, expected), (Some(a), Some(b)) if a == b)
}

fn coords(s: &str) -> Option<(f64, f64)> {
    let (_, a, b) = crate::parse::match_coordinates(s)?;
    Some((parse_int(a, None), parse_int(b, None)))
}

fn eastern(ms: f64) -> chrono::DateTime<Tz> {
    chrono_tz::America::New_York
        .timestamp_millis_opt(ms.floor() as i64)
        .single()
        .expect("valid instant")
}

pub fn day_seed(prefix: &str, day: &str, secret: Option<&str>) -> String {
    match secret {
        Some(s) => format!("{prefix}{day}:{s}"),
        None => format!("{prefix}{day}"),
    }
}

pub fn random_hour(ms: f64, secret: Option<&str>) -> u32 {
    let day = eastern(ms).format("%Y-%m-%d").to_string();
    (SeedRandom::new(&day_seed("", &day, secret)).next_f64() * 24.0).floor() as u32
}

pub fn random_minute(ms: f64, secret: Option<&str>) -> i64 {
    let day = eastern(ms).format("%Y-%m-%d").to_string();
    let seed = day_seed("random_minute_", &day, secret);
    (SeedRandom::new(&seed).next_f64() * 1440.0).floor() as i64
}

fn eastern_minute_of_day(ms: f64) -> i64 {
    let t = eastern(ms);
    let start = chrono_tz::America::New_York
        .from_local_datetime(&t.date_naive().and_hms_opt(0, 0, 0).unwrap())
        .earliest()
        .expect("midnight exists in New York");
    let diff = ms - start.timestamp_millis() as f64;
    (diff / 60000.0).trunc() as i64
}

fn local_hour(ms: f64, tz: Tz) -> u32 {
    tz.timestamp_millis_opt(ms.floor() as i64)
        .single()
        .expect("valid instant")
        .hour()
}

pub fn validate(t: &ThreadRules, i: &Input, env: &mut Env) -> Verdict {
    let (is_valid, roll) = validate_count(t, i, env);
    let is_valid = is_valid && !i.count_banned;
    let valid_count_number = if !is_valid {
        None
    } else if let Some(last) = i.last_valid {
        Some(last.valid_count_number.unwrap_or(0) + 1)
    } else if i.raw_count == Some(t.first_valid_count) {
        if t.reset_on_mistakes {
            match i.newest_valid_count_number {
                Some(Some(n)) => Some(n + 1),
                Some(None) => Some(1),
                None => Some(1),
            }
        } else {
            Some(1)
        }
    } else {
        None
    };
    Verdict {
        is_valid,
        valid_count_number,
        roll,
    }
}

fn validate_count(t: &ThreadRules, i: &Input, env: &mut Env) -> (bool, Option<(f64, f64)>) {
    if i.is_bot {
        return (false, None);
    }
    let raw = i.raw_count;
    let Some(last) = i.last_valid else {
        return (raw == Some(t.first_valid_count), None);
    };
    if last.author == i.author && !t.allow_double_posts {
        return (false, None);
    }
    let prev_raw = last.raw_count.as_deref();
    let prev = pi(prev_raw);
    let cur = pi(raw);
    let plus = |k: f64| prev + k == cur && !starts0(raw);
    let base_n = |radix: u32| {
        let p = h::base_n_to_decimal(prev_raw.unwrap_or(""), radix);
        h::base_n_to_decimal(raw.unwrap_or(""), radix) == p + 1.0 && !starts0(raw)
    };
    let ok = match t.vt {
        V::Main | V::Yoco | V::Roulette => plus(1.0),
        V::Parity => plus(2.0),
        V::By3 => plus(3.0),
        V::Slow => {
            if i.author_counted_within_hour {
                return (false, None);
            }
            plus(1.0)
        }
        V::Bars => {
            prev + 1.0 == cur
                && (last.timestamp / 3600000.0).floor() < (i.post_timestamp / 3600000.0).floor()
                && !starts0(raw)
        }
        V::Letters => h::letters_to_decimal(raw.unwrap_or("")) == h::letters_to_decimal(prev_raw.unwrap_or("")) + 1.0,
        V::Binary | V::Ternary | V::Quaternary | V::Quinary | V::Senary | V::Septenary | V::Octonary | V::Nonary => {
            base_n(t.vt.base_n_radix().unwrap())
        }
        V::NoRepdigits => match h::no_repdig_get_count(vcn(last) + 1.0) {
            Some(next) => cur == parse_int(&next, None) && !starts0(raw),
            None => false,
        },
        V::Repdigits => h::find_next_repdigit(prev) == Some(cur) && !cur.is_nan() && !starts0(raw),
        V::RepeatingDigits => {
            let next =
                h::find_next_count_by_predicate(prev_raw.unwrap_or(""), h::has_all_digits_repeated_at_least_twice);
            next.is_some() && raw.map_or(f64::NAN, |r| parse_int(r, Some(10))) == next.unwrap() && !starts0(raw)
        }
        V::MostlyRepeatingDigits => {
            let next = h::find_next_count_by_predicate(prev_raw.unwrap_or(""), h::has_any_repeated_digit);
            next.is_some() && raw.map_or(f64::NAN, |r| parse_int(r, Some(10))) == next.unwrap() && !starts0(raw)
        }
        V::Hexadecimal => {
            h::radix_to_decimal(raw.unwrap_or(""), 16) == h::radix_to_decimal(prev_raw.unwrap_or(""), 16) + 1.0
        }
        V::Alphanumerics => {
            h::radix_to_decimal(raw.unwrap_or(""), 36) == h::radix_to_decimal(prev_raw.unwrap_or(""), 36) + 1.0
        }
        V::Base62 => eq(raw, Some(&h::decimal_to_base62(vcn(last) + 1.0))),
        V::Base64 => eq(raw, Some(&h::decimal_to_base64(vcn(last) + 1.0))),
        V::Coordinates | V::CoordinatesTow => match (prev_raw.and_then(coords), raw.and_then(coords)) {
            (Some((px, py)), Some((cx, cy))) => {
                if t.vt == V::Coordinates {
                    (px == cx && py + 1.0 == cy) || (px + 1.0 == cx && py == cy)
                } else {
                    (px == cx && (py - cy).abs() == 1.0) || ((px - cx).abs() == 1.0 && py == cy)
                }
            }
            _ => false,
        },
        V::Wave => eq(raw, prev_raw.and_then(h::next_wave_number).as_deref()),
        V::Mayan => eq(raw, Some(&h::decimal_to_mayan(vcn(last) + 1.0))),
        V::Permutations => eq(raw, Some(&h::next_permutation(prev_raw.unwrap_or("")))),
        V::Dates => eq(raw, h::days_to_date(vcn(last) + 1.0).as_deref()),
        V::Military => eq(raw, h::seconds_to_military_time_string(vcn(last) + 1.0).as_deref()),
        V::Time => eq(raw, h::seconds_to_time_string(vcn(last) + 1.0).as_deref()),
        V::Rationals => eq(raw, Some(&h::next_rational(prev_raw.unwrap_or("")))),
        V::Factoradic => eq(raw, h::decimal_to_factoradic(vcn(last) + 1.0).as_deref()),
        V::FastTslc => {
            cur == prev + 1.0
                && (if prev >= 2.0 {
                    i.time_since_last_count < last.time_since_last_count
                } else {
                    true
                })
                && !starts0(raw)
        }
        V::Tslc => cur == prev + 1.0 && i.time_since_last_count > last.time_since_last_count && !starts0(raw),
        V::Username => cur == prev + i.username_len16 as f64,
        V::Wait2 => match i.recent_valid {
            [Some(p1), two] => {
                cur == pi(p1.raw_count.as_deref()) + 1.0
                    && i.author != p1.author
                    && two.is_none_or(|p2| i.author != p2.author)
                    && !starts0(raw)
            }
            _ => false,
        },
        V::IncrementalOdds => {
            let Some(p1) = i.recent_valid[0] else {
                return (false, None);
            };
            let pd = pi(p1.raw_count.as_deref());
            let diff = cur - pd;
            let need = match i.recent_valid[1] {
                Some(p2) => pd - pi(p2.raw_count.as_deref()),
                None => 1.0,
            };
            if diff >= need {
                let chance = 1.0 / diff;
                let roll = (env.random)();
                let ok = cur.fract() == 0.0 && cur.is_finite() && roll <= chance && !starts0(raw);
                return (ok, Some((roll, chance)));
            }
            return (false, None);
        }
        V::Roman => h::roman_to_decimal(raw.unwrap_or("")) == h::roman_to_decimal(prev_raw.unwrap_or("")) + 1.0,
        V::Words => raw.is_some_and(|r| !r.is_empty()) && eq(raw, h::number_to_words(vcn(last) + 1.0).as_deref()),
        V::Ordinals => {
            raw.is_some_and(|r| !r.is_empty()) && eq(raw, h::number_to_ordinal_words(vcn(last) + 1.0).as_deref())
        }
        V::Tugofwar => {
            (cur == prev + 1.0 || cur == prev - 1.0)
                && raw != Some("-0")
                && !raw.is_some_and(|r| len16(r) > 1 && r.starts_with('0'))
                && !raw.is_some_and(|r| len16(r) > 2 && r.starts_with("-0"))
        }
        V::Randomhour => {
            cur == prev + 1.0
                && local_hour(env.now_ms, env.server_tz) == random_hour(env.now_ms, env.secret)
                && !starts0(raw)
        }
        V::Randomminute => {
            cur == prev + 1.0
                && random_minute(env.now_ms, env.secret) == eastern_minute_of_day(env.now_ms)
                && !starts0(raw)
        }
        V::Waitx => cur == prev + 1.0 && cur < (env.now_ms - last.timestamp) / 1000.0 && !starts0(raw),
        V::OneInX => {
            if cur == prev + 1.0 {
                let p = if prev.is_nan() || prev == 0.0 { 0.0 } else { prev };
                let chance = 1.0 / (p + 1.0);
                let roll = (env.random)();
                return (roll <= chance && !starts0(raw), Some((roll, chance)));
            }
            return (false, None);
        }
        V::Countdown => cur == prev - 1.0 && cur > -1.0 && !raw.is_some_and(|r| len16(r) > 1 && r.starts_with('0')),
        V::G2048 => eq(raw, prev_raw.map(h::next_2048).as_deref()),
        V::Other => false,
    };
    (ok, None)
}

pub fn timestamp_minus_ms(ts: f64, ms: f64) -> f64 {
    ((ts * 1000.0).round() - (ms * 1000.0).round()) / 1000.0
}

pub fn ts_string(x: f64) -> String {
    num_to_string(x)
}

#[cfg(test)]
mod secret_tests {
    use super::*;

    const NOON: f64 = 1_735_747_200_000.0;

    #[test]
    fn without_a_secret_the_random_hour_follows_the_date() {
        let day = eastern(NOON).format("%Y-%m-%d").to_string();
        let expected = (SeedRandom::new(&day).next_f64() * 24.0).floor() as u32;
        assert_eq!(random_hour(NOON, None), expected);
    }

    #[test]
    fn a_secret_changes_the_random_minute() {
        assert_ne!(random_minute(NOON, Some("a")), random_minute(NOON, Some("b")));
        let day = eastern(NOON).format("%Y-%m-%d").to_string();
        let plain = (SeedRandom::new(&format!("random_minute_{day}")).next_f64() * 1440.0).floor() as i64;
        assert_eq!(random_minute(NOON, None), plain);
    }

    #[test]
    fn a_secret_changes_the_random_hour_on_some_days() {
        let differs = (0..30).any(|d| {
            let ms = NOON + d as f64 * 86_400_000.0;
            random_hour(ms, Some("a")) != random_hour(ms, Some("b"))
        });
        assert!(differs);
    }
}
