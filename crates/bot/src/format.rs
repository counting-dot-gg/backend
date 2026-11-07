pub fn fancy_time(ms: i64) -> String {
    let hours = ms.div_euclid(3_600_000);
    let ms = ms.rem_euclid(3_600_000);
    let minutes = ms / 60_000;
    let ms = ms % 60_000;
    let seconds = ms / 1000;
    let millis = ms % 1000;
    let minutes_s = if hours > 0 {
        format!("{minutes:02}")
    } else {
        minutes.to_string()
    };
    let seconds_s = if hours > 0 || minutes > 0 {
        format!("{seconds:02}")
    } else {
        seconds.to_string()
    };
    let h = if hours > 0 { format!("{hours}:") } else { String::new() };
    let m = if hours > 0 || minutes > 0 {
        format!("{minutes_s}:")
    } else {
        String::new()
    };
    format!("{h}{m}{seconds_s}.{millis:03}")
}

pub fn reply_speed(ms: f64) -> String {
    let abs = ms.abs().round() as i64;
    let (h, m, s, rest) = (
        abs / 3_600_000,
        (abs % 3_600_000) / 60_000,
        (abs % 60_000) / 1000,
        abs % 1000,
    );
    if h > 0 {
        format!("{h}h {m}m {s}s")
    } else if m > 0 {
        format!("{m}m {s}s")
    } else if s > 0 {
        format!("{s}s {rest:03}ms")
    } else {
        format!("{rest}ms")
    }
}

pub fn thousands(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 { format!("-{out}") } else { out }
}

pub fn clamp(v: Option<&str>, n: usize) -> String {
    match v.filter(|s| !s.is_empty()) {
        None => "unknown".to_owned(),
        Some(s) => {
            let units: Vec<u16> = s.encode_utf16().collect();
            if units.len() > n {
                format!("{}\u{2026}", String::from_utf16_lossy(&units[..n - 1]))
            } else {
                s.to_owned()
            }
        }
    }
}

pub fn utf16_chunks(s: &str, size: usize) -> Vec<String> {
    let units: Vec<u16> = s.encode_utf16().collect();
    units.chunks(size).map(String::from_utf16_lossy).collect()
}

pub fn hex_color(s: &str) -> Option<u32> {
    let t = s.trim_start_matches('#');
    let end = t.find(|c: char| !c.is_ascii_hexdigit()).unwrap_or(t.len());
    u32::from_str_radix(&t[..end], 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fancy_time_matches_node() {
        assert_eq!(fancy_time(5_042), "5.042");
        assert_eq!(fancy_time(65_042), "1:05.042");
        assert_eq!(fancy_time(3_725_007), "1:02:05.007");
        assert_eq!(fancy_time(0), "0.000");
    }

    #[test]
    fn reply_speed_units() {
        assert_eq!(reply_speed(812.0), "812ms");
        assert_eq!(reply_speed(4_005.0), "4s 005ms");
        assert_eq!(reply_speed(61_000.0), "1m 1s");
        assert_eq!(reply_speed(3_661_000.0), "1h 1m 1s");
    }

    #[test]
    fn thousands_separators() {
        assert_eq!(thousands(1_234_567), "1,234,567");
        assert_eq!(thousands(999), "999");
    }

    #[test]
    fn clamp_marks_cuts() {
        assert_eq!(clamp(Some("abcdef"), 4), "abc\u{2026}");
        assert_eq!(clamp(None, 4), "unknown");
        assert_eq!(clamp(Some(""), 4), "unknown");
    }

    #[test]
    fn colors_parse_like_parse_int() {
        assert_eq!(hex_color("#ff0000"), Some(0xff0000));
        assert_eq!(hex_color("zz"), None);
    }
}
