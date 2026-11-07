#[inline]
pub fn is_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r' | ' ' | '\u{A0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200A}' | '\u{2028}' | '\u{2029}' | '\u{202F}' | '\u{205F}' | '\u{3000}' | '\u{FEFF}'
    )
}

pub fn trim(s: &str) -> &str {
    s.trim_matches(is_space)
}

pub fn len16(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

pub fn parse_int(s: &str, radix: Option<u32>) -> f64 {
    let mut s = s.trim_start_matches(is_space);
    let mut sign = 1.0;
    if let Some(rest) = s.strip_prefix('-') {
        sign = -1.0;
        s = rest;
    } else if let Some(rest) = s.strip_prefix('+') {
        s = rest;
    }
    let mut r = radix.unwrap_or(0);
    let mut strip_prefix = true;
    if r != 0 {
        if !(2..=36).contains(&r) {
            return f64::NAN;
        }
        if r != 16 {
            strip_prefix = false;
        }
    } else {
        r = 10;
    }
    if strip_prefix && (s.starts_with("0x") || s.starts_with("0X")) {
        s = &s[2..];
        r = 16;
    }
    let end = s
        .char_indices()
        .find(|&(_, c)| c.to_digit(r).is_none() || !c.is_ascii())
        .map_or(s.len(), |(i, _)| i);
    let digits = &s[..end];
    if digits.is_empty() {
        return f64::NAN;
    }
    let v = if r == 10 {
        digits.parse::<f64>().unwrap_or(f64::NAN)
    } else {
        let mut acc: u128 = 0;
        let mut exact = true;
        let mut f = 0.0f64;
        for c in digits.chars() {
            let d = c.to_digit(r).unwrap();
            if exact {
                match acc.checked_mul(r as u128).and_then(|v| v.checked_add(d as u128)) {
                    Some(v) => acc = v,
                    None => {
                        exact = false;
                        f = acc as f64 * r as f64 + d as f64;
                    }
                }
            } else {
                f = f * r as f64 + d as f64;
            }
        }
        if exact { acc as f64 } else { f }
    };
    sign * v
}

pub fn parse_float(s: &str) -> f64 {
    let s = s.trim_start_matches(is_space);
    let b = s.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    if s[i..].starts_with("Infinity") {
        return if b.first() == Some(&b'-') {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        };
    }
    let int_start = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    let mut digits = i - int_start;
    if i < b.len() && b[i] == b'.' {
        let mut j = i + 1;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        digits += j - i - 1;
        if digits > 0 {
            i = j;
        }
    }
    if digits == 0 {
        return f64::NAN;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        let k = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j > k {
            i = j;
        }
    }
    let lit = &s[..i];
    let lit = lit.strip_suffix('.').unwrap_or(lit);
    lit.parse::<f64>().unwrap_or(f64::NAN)
}

pub fn num_to_string(x: f64) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x == 0.0 {
        return "0".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "Infinity".into() } else { "-Infinity".into() };
    }
    if x.fract() == 0.0 && x.abs() < 9007199254740992.0 {
        return (x as i64).to_string();
    }
    let e = format!("{:e}", x.abs());
    let (mant, exp) = e.split_once('e').unwrap();
    let digits: String = mant.chars().filter(|c| *c != '.').collect();
    let k = digits.len() as i32;
    let n = exp.parse::<i32>().unwrap() + 1;
    let mut out = String::new();
    if x < 0.0 {
        out.push('-');
    }
    if k <= n && n <= 21 {
        out.push_str(&digits);
        out.extend(std::iter::repeat_n('0', (n - k) as usize));
    } else if 0 < n && n <= 21 {
        out.push_str(&digits[..n as usize]);
        out.push('.');
        out.push_str(&digits[n as usize..]);
    } else if -6 < n && n <= 0 {
        out.push_str("0.");
        out.extend(std::iter::repeat_n('0', (-n) as usize));
        out.push_str(&digits);
    } else {
        out.push_str(&digits[..1]);
        if k > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push('e');
        out.push(if n > 0 { '+' } else { '-' });
        out.push_str(&(n - 1).abs().to_string());
    }
    out
}

pub fn upper(s: &str) -> String {
    s.to_uppercase()
}

pub fn capitalize_first_letter(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => {
            let mut out: String = c.to_uppercase().collect();
            out.push_str(&chars.as_str().to_lowercase());
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_int_matches_js() {
        assert_eq!(parse_int("  12abc", None), 12.0);
        assert!(parse_int("abc", None).is_nan());
        assert_eq!(parse_int("0x1A", None), 26.0);
        assert_eq!(parse_int("0x1A", Some(10)), 0.0);
        assert!(parse_int("-0", None).is_sign_negative());
        assert_eq!(parse_int("\u{FEFF}7", None), 7.0);
        assert!(parse_int("\u{85}7", None).is_nan());
        assert_eq!(parse_int("1012", Some(2)), 5.0);
        assert_eq!(parse_int("z", Some(36)), 35.0);
        assert_eq!(parse_int("99999999999999999999", None), 1e20);
    }

    #[test]
    fn parse_float_matches_js() {
        assert_eq!(parse_float("1700000000000.123"), 1700000000000.123);
        assert_eq!(parse_float("1.5e3x"), 1500.0);
        assert_eq!(parse_float("1e"), 1.0);
        assert_eq!(parse_float(".5"), 0.5);
        assert_eq!(parse_float("5."), 5.0);
        assert!(parse_float(".").is_nan());
        assert_eq!(parse_float("-Infinityx"), f64::NEG_INFINITY);
    }

    #[test]
    fn to_string_matches_js() {
        assert_eq!(num_to_string(1e21), "1e+21");
        assert_eq!(num_to_string(123456789012345680000.0), "123456789012345680000");
        assert_eq!(num_to_string(-0.0), "0");
        assert_eq!(num_to_string(0.1 + 0.2), "0.30000000000000004");
        assert_eq!(num_to_string(1700000000000.123), "1700000000000.123");
        assert_eq!(num_to_string(1e-7), "1e-7");
        assert_eq!(num_to_string(0.000001), "0.000001");
        assert_eq!(num_to_string(1.5e300), "1.5e+300");
    }
}
