use counting::js::{num_to_string, parse_float};

pub fn ts_us_of(timestamp: &str) -> Option<u64> {
    let (ms, frac) = timestamp.split_once('.').unwrap_or((timestamp, ""));
    if ms.is_empty() || frac.len() > 3 || !ms.bytes().chain(frac.bytes()).all(|b| b.is_ascii_digit()) {
        return None;
    }
    let frac: String = frac.chars().chain(std::iter::repeat('0')).take(3).collect();
    Some(ms.parse::<u64>().ok()? * 1000 + frac.parse::<u64>().ok()?)
}

pub fn ts_us_lossy(timestamp: &str) -> u64 {
    ts_us_of(timestamp).unwrap_or_else(|| {
        let ms = parse_float(timestamp);
        if ms.is_finite() && ms > 0.0 {
            (ms * 1000.0).round() as u64
        } else {
            0
        }
    })
}

pub fn timestamp_of(ts_us: u64) -> String {
    num_to_string(ts_us as f64 / 1000.0)
}

const RAND_SPAN: u32 = 90_000;
const RAND_BASE: u32 = 10_000;

pub fn uuid_of(ns: u64, rand5: u32) -> String {
    let s = format!("{ns:019}{rand5}");
    let v = ['8', '9', 'a'][((rand5 - RAND_BASE) / 30_000).min(2) as usize];
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

pub fn uuid_parts(uuid: &str) -> Option<(u64, u32)> {
    let b = uuid.as_bytes();
    if b.len() != 36 || [8, 13, 18, 23].iter().any(|&i| b[i] != b'-') || b[14] != b'4' {
        return None;
    }
    let digits = [&uuid[0..8], &uuid[9..13], &uuid[15..18], &uuid[20..23], &uuid[24..30]].concat();
    if digits.len() != 24 || !digits.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let ns = digits[0..19].parse().ok()?;
    let rand5: u32 = digits[19..24].parse().ok()?;
    (RAND_BASE..RAND_BASE + RAND_SPAN)
        .contains(&rand5)
        .then_some((ns, rand5))
}

pub fn uuid_rest(uuid: &str, ts_us: u64) -> Option<u32> {
    let (ns, rand5) = uuid_parts(uuid)?;
    if ns / 1000 != ts_us || uuid_of(ns, rand5) != uuid {
        return None;
    }
    Some((ns % 1000) as u32 * RAND_SPAN + (rand5 - RAND_BASE))
}

pub fn uuid_from_rest(ts_us: u64, rest: u32) -> String {
    uuid_of(ts_us * 1000 + u64::from(rest / RAND_SPAN), rest % RAND_SPAN + RAND_BASE)
}

pub fn gap(timestamp: &str, prev: Option<&str>) -> String {
    let t = parse_float(timestamp);
    num_to_string(prev.map_or(0.0, |p| t - parse_float(p)))
}

pub fn raw_text(count_content: Option<&str>, comment: Option<&str>) -> String {
    [count_content.unwrap_or(""), comment.unwrap_or("")].concat()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuid_round_trip() {
        let u = uuid_of(1791321226163219123, 54321);
        let ts = ts_us_of("1791321226163.219").unwrap();
        let rest = uuid_rest(&u, ts).unwrap();
        assert_eq!(uuid_from_rest(ts, rest), u);
    }

    #[test]
    fn timestamps_drop_trailing_zeros() {
        assert_eq!(timestamp_of(ts_us_of("1791502647977.17").unwrap()), "1791502647977.17");
        assert_eq!(timestamp_of(ts_us_of("1794321976968").unwrap()), "1794321976968");
        assert_eq!(ts_us_of("1.2345"), None);
    }
}
