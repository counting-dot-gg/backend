use crate::js::{self, is_space, num_to_string, parse_int};

fn digits_at(s: &str, i: usize) -> usize {
    s.as_bytes()[i.min(s.len())..]
        .iter()
        .take_while(|c| c.is_ascii_digit())
        .count()
}

pub fn extract_wave_number(s: &str) -> Option<&str> {
    wave_parts(s).map(|(m, _, _)| m)
}

fn wave_parts(s: &str) -> Option<(&str, &str, &str)> {
    let b = s.as_bytes();
    let mut i = usize::from(b.first() == Some(&b'-'));
    let d = digits_at(s, i);
    if d == 0 {
        return None;
    }
    i += d;
    let a = &s[..i];
    i += s[i..]
        .chars()
        .take_while(|&c| is_space(c))
        .map(char::len_utf8)
        .sum::<usize>();
    if b.get(i) != Some(&b'(') {
        return None;
    }
    i += 1;
    let gs = i;
    if b.get(i) == Some(&b'-') {
        i += 1;
    }
    let d = digits_at(s, i);
    if d == 0 {
        return None;
    }
    i += d;
    if b.get(i) != Some(&b')') {
        return None;
    }
    Some((&s[..i + 1], a, &s[gs..i]))
}

pub fn extract_time(s: &str) -> Option<&str> {
    let rest = |start: usize| -> Option<usize> {
        let b = s.as_bytes();
        let mut i = start;
        for _ in 0..2 {
            if b.get(i) != Some(&b':') || digits_at(s, i + 1) < 2 {
                return None;
            }
            i += 3;
        }
        let c = s.get(i..)?.chars().next()?;
        if !is_space(c) {
            return None;
        }
        i += c.len_utf8();
        let ap = b.get(i)?.to_ascii_uppercase();
        let m = b.get(i + 1)?.to_ascii_uppercase();
        ((ap == b'A' || ap == b'P') && m == b'M').then_some(i + 2)
    };
    let b = s.as_bytes();
    if b.first().is_some_and(|c| (b'1'..=b'9').contains(c))
        && b.get(1).is_some_and(u8::is_ascii_digit)
        && let Some(e) = rest(2)
    {
        return Some(&s[..e]);
    }
    if b.first().is_some_and(u8::is_ascii_digit)
        && let Some(e) = rest(1)
    {
        return Some(&s[..e]);
    }
    None
}

pub fn extract_military_time(s: &str) -> Option<&str> {
    let b = s.as_bytes();
    let ok = b.len() >= 8
        && b[0].is_ascii_digit()
        && b[1].is_ascii_digit()
        && b[2] == b':'
        && b[3].is_ascii_digit()
        && b[4].is_ascii_digit()
        && b[5] == b':'
        && b[6].is_ascii_digit()
        && b[7].is_ascii_digit();
    ok.then(|| &s[..8])
}

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

pub fn extract_date(s: &str) -> Option<&str> {
    let b = s.as_bytes();
    let m = MONTHS
        .iter()
        .find(|m| b.len() >= m.len() && b[..m.len()].eq_ignore_ascii_case(m.as_bytes()))?;
    let mut i = m.len();
    let c = s[i..].chars().next()?;
    if !is_space(c) {
        return None;
    }
    i += c.len_utf8();
    let d = digits_at(s, i);
    if d == 0 {
        return None;
    }
    i += d.min(2);
    if b.get(i) != Some(&b',') {
        return None;
    }
    i += 1;
    let c = s[i..].chars().next()?;
    if !is_space(c) {
        return None;
    }
    i += c.len_utf8();
    let d = digits_at(s, i);
    if d == 0 {
        return None;
    }
    Some(&s[..i + d.min(5)])
}

pub fn extract_rational(s: &str) -> Option<&str> {
    rational_parts(s).map(|(m, _, _)| m)
}

fn rational_parts(s: &str) -> Option<(&str, &str, &str)> {
    let b = s.as_bytes();
    if !b.first().is_some_and(|c| (b'1'..=b'9').contains(c)) {
        return None;
    }
    let n = digits_at(s, 0);
    if b.get(n) != Some(&b'/') || !b.get(n + 1).is_some_and(|c| (b'1'..=b'9').contains(c)) {
        return None;
    }
    let d = digits_at(s, n + 1);
    Some((&s[..n + 1 + d], &s[..n], &s[n + 1..n + 1 + d]))
}

const NUMBER_WORDS: [&str; 32] = [
    "zero",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "ten",
    "eleven",
    "twelve",
    "thirteen",
    "fourteen",
    "fifteen",
    "sixteen",
    "seventeen",
    "eighteen",
    "nineteen",
    "twenty",
    "thirty",
    "forty",
    "fifty",
    "sixty",
    "seventy",
    "eighty",
    "ninety",
    "hundred",
    "thousand",
    "million",
    "billion",
];

const ORDINAL_WORDS: [&str; 33] = [
    "first",
    "second",
    "third",
    "fourth",
    "fifth",
    "sixth",
    "seventh",
    "eighth",
    "ninth",
    "tenth",
    "eleventh",
    "twelfth",
    "thirteenth",
    "fourteenth",
    "fifteenth",
    "sixteenth",
    "seventeenth",
    "eighteenth",
    "nineteenth",
    "twentieth",
    "thirtieth",
    "fortieth",
    "fiftieth",
    "sixtieth",
    "seventieth",
    "eightieth",
    "ninetieth",
    "hundredth",
    "thousandth",
    "millionth",
    "billionth",
    "and",
    "zero",
];

pub fn is_number_word(w: &str) -> bool {
    let l = w.to_lowercase();
    NUMBER_WORDS.contains(&l.as_str())
}

pub fn is_ordinal_word(w: &str) -> bool {
    let l = w.to_lowercase();
    NUMBER_WORDS.contains(&l.as_str()) || ORDINAL_WORDS.contains(&l.as_str())
}

pub fn split_words(input: &str, is_word: impl Fn(&str) -> bool) -> (Option<String>, Option<String>) {
    let words: Vec<&str> = input.split(' ').collect();
    let mut count = String::new();
    let mut comment = String::new();
    for (i, w) in words.iter().enumerate() {
        if is_word(w) {
            count.push_str(w);
            count.push(' ');
        } else {
            comment = words[i..].join(" ");
            break;
        }
    }
    let count = js::trim(&count).to_owned();
    (
        (!count.is_empty()).then_some(count),
        (!comment.is_empty()).then_some(comment),
    )
}

pub fn number_to_words(n: f64) -> Option<String> {
    if n < 0.0 {
        return None;
    }
    if n == 0.0 {
        return Some("Zero".into());
    }
    const SINGLE: [&str; 10] = [
        "", "One", "Two", "Three", "Four", "Five", "Six", "Seven", "Eight", "Nine",
    ];
    const DOUBLE: [&str; 10] = [
        "Ten",
        "Eleven",
        "Twelve",
        "Thirteen",
        "Fourteen",
        "Fifteen",
        "Sixteen",
        "Seventeen",
        "Eighteen",
        "Nineteen",
    ];
    const TENS: [&str; 8] = [
        "Twenty", "Thirty", "Forty", "Fifty", "Sixty", "Seventy", "Eighty", "Ninety",
    ];
    fn idx(a: &[&'static str], i: f64) -> &'static str {
        if i >= 0.0 && (i as usize) < a.len() && i.fract() == 0.0 {
            a[i as usize]
        } else {
            "undefined"
        }
    }
    fn translate(n: f64) -> String {
        if n < 10.0 {
            format!("{} ", idx(&SINGLE, n))
        } else if n < 20.0 {
            format!("{} ", idx(&DOUBLE, n - 10.0))
        } else if n < 100.0 {
            let rem = translate(n % 10.0);
            format!("{} {}", idx(&TENS, (n - n % 10.0) / 10.0 - 2.0), rem)
        } else if n < 1000.0 {
            format!("{} Hundred {}", idx(&SINGLE, (n / 100.0).trunc()), translate(n % 100.0))
        } else if n < 1e6 {
            format!(
                "{} Thousand {}",
                js::trim(&translate((n / 1000.0).trunc())),
                translate(n % 1000.0)
            )
        } else if n < 1e9 {
            format!(
                "{} Million {}",
                js::trim(&translate((n / 1e6).trunc())),
                translate(n % 1e6)
            )
        } else {
            format!(
                "{} Billion {}",
                js::trim(&translate((n / 1e9).trunc())),
                translate(n % 1e9)
            )
        }
    }
    Some(js::trim(&js::capitalize_first_letter(&translate(n))).to_owned())
}

pub fn number_to_ordinal_words(n: f64) -> Option<String> {
    if n < 0.0 || n.fract() != 0.0 || !n.is_finite() {
        return None;
    }
    if n == 0.0 {
        return Some("Zeroth".into());
    }
    const SINGLE: [&str; 10] = [
        "", "One", "Two", "Three", "Four", "Five", "Six", "Seven", "Eight", "Nine",
    ];
    const TEENS: [&str; 10] = [
        "Ten",
        "Eleven",
        "Twelve",
        "Thirteen",
        "Fourteen",
        "Fifteen",
        "Sixteen",
        "Seventeen",
        "Eighteen",
        "Nineteen",
    ];
    const TENS: [&str; 8] = [
        "Twenty", "Thirty", "Forty", "Fifty", "Sixty", "Seventy", "Eighty", "Ninety",
    ];
    const SINGLE_ORD: [&str; 10] = [
        "", "First", "Second", "Third", "Fourth", "Fifth", "Sixth", "Seventh", "Eighth", "Ninth",
    ];
    const TEEN_ORD: [&str; 10] = [
        "Tenth",
        "Eleventh",
        "Twelfth",
        "Thirteenth",
        "Fourteenth",
        "Fifteenth",
        "Sixteenth",
        "Seventeenth",
        "Eighteenth",
        "Nineteenth",
    ];
    const TENS_ORD: [&str; 8] = [
        "Twentieth",
        "Thirtieth",
        "Fortieth",
        "Fiftieth",
        "Sixtieth",
        "Seventieth",
        "Eightieth",
        "Ninetieth",
    ];
    fn below_hundred(n: u64, ord: bool) -> String {
        let n = n as usize;
        if n < 10 {
            return (if ord { SINGLE_ORD[n] } else { SINGLE[n] }).into();
        }
        if n < 20 {
            return (if ord { TEEN_ORD[n - 10] } else { TEENS[n - 10] }).into();
        }
        let (t, r) = (n / 10, n % 10);
        if r == 0 {
            return (if ord { TENS_ORD[t - 2] } else { TENS[t - 2] }).into();
        }
        format!("{} {}", TENS[t - 2], if ord { SINGLE_ORD[r] } else { SINGLE[r] })
    }
    fn below_thousand(n: u64, ord: bool) -> String {
        if n < 100 {
            return below_hundred(n, ord);
        }
        let (h, r) = ((n / 100) as usize, n % 100);
        if r == 0 {
            return format!("{} Hundred{}", SINGLE[h], if ord { "th" } else { "" });
        }
        format!(
            "{} Hundred{}{}",
            SINGLE[h],
            if ord { " and " } else { " " },
            below_hundred(r, ord)
        )
    }
    fn convert(n: u64, ord: bool) -> String {
        if n < 1000 {
            return below_thousand(n, ord);
        }
        for (v, label) in [
            (1_000_000_000u64, "Billion"),
            (1_000_000, "Million"),
            (1_000, "Thousand"),
        ] {
            if n >= v {
                let (major, r) = (n / v, n % v);
                let mw = convert(major, false);
                if r == 0 {
                    return format!("{mw} {label}{}", if ord { "th" } else { "" });
                }
                let joiner = if ord && r < 100 { " and " } else { " " };
                return format!("{mw} {label}{joiner}{}", convert(r, ord));
            }
        }
        String::new()
    }
    Some(js::capitalize_first_letter(js::trim(&convert(n as u64, true))))
}

pub fn letters_to_decimal(s: &str) -> f64 {
    let p = parse_int(s, None);
    if s.is_empty() || (p != 0.0 && !p.is_nan()) {
        return 0.0;
    }
    s.encode_utf16().fold(0.0, |d, u| d * 26.0 + (u as f64 - 65.0 + 1.0))
}

pub fn radix_to_decimal(s: &str, radix: u32) -> f64 {
    if s.is_empty() || !s.chars().all(|c| c.is_digit(radix) && c.is_ascii()) {
        return 0.0;
    }
    s.chars()
        .fold(0.0, |d, c| d * radix as f64 + c.to_digit(radix).unwrap() as f64)
}

pub fn base_n_to_decimal(s: &str, radix: u32) -> f64 {
    if s.is_empty() || !s.chars().all(|c| c.is_ascii_digit() && c.to_digit(10).unwrap() < radix) {
        return 0.0;
    }
    parse_int(s, Some(radix))
}

fn encode_base(n: f64, chars: &[u8], zero: &str) -> String {
    if n == 0.0 {
        return zero.into();
    }
    let base = chars.len() as f64;
    let mut out = Vec::new();
    let mut r = n;
    while r > 0.0 {
        let d = r % base;
        if d.fract() == 0.0 {
            out.push(String::from(chars[d as usize] as char));
        } else {
            out.push("undefined".into());
        }
        r = (r / base).floor();
    }
    out.reverse();
    out.concat()
}

pub fn decimal_to_base62(n: f64) -> String {
    encode_base(
        n,
        b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz",
        "0",
    )
}

pub fn decimal_to_base64(n: f64) -> String {
    encode_base(
        n,
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/",
        "A",
    )
}

fn roman_value(u: u16) -> i32 {
    match u {
        0x49 => 1,
        0x56 => 5,
        0x58 => 10,
        0x4C => 50,
        0x43 => 100,
        0x44 => 500,
        0x4D => 1000,
        0x61 => 5000,
        0x62 => 10000,
        0x63 => 50000,
        0x64 => 100000,
        0x65 => 500000,
        0x66 => 1000000,
        _ => -1,
    }
}

fn highest_one_bit(mut n: i32) -> i32 {
    let mut r = 0;
    while n > 0 {
        n >>= 1;
        r += 1;
    }
    1i32.wrapping_shl((r - 1) as u32)
}

const MAX_SAFE: f64 = 9007199254740991.0;
const ROMAN_MASK: i32 = 0b00010010010001001001000100100100;

pub fn roman_to_decimal(s: &str) -> f64 {
    if s.is_empty() {
        return 0.0;
    }
    let s = s
        .replace("V\u{305}", "a")
        .replace("X\u{305}", "b")
        .replace("L\u{305}", "c")
        .replace("C\u{305}", "d")
        .replace("D\u{305}", "e")
        .replace("M\u{305}", "f");
    let u: Vec<u16> = s.encode_utf16().collect();
    let (mut result, mut reps, mut last) = (0i64, 0, -1i32);
    let mut i = 0;
    while i < u.len() {
        let s1 = roman_value(u[i]);
        if s1 == -1 || (s1 > last && last > 0) {
            return MAX_SAFE;
        }
        if i + 1 < u.len() {
            let s2 = roman_value(u[i + 1]);
            if s2 == -1 || (s2 > last && last > 0) {
                return MAX_SAFE;
            }
            if s1 >= s2 {
                result += s1 as i64;
                reps = if s1 == last { reps + 1 } else { 1 };
            } else {
                if (s2 >> 4) > s1 || (s2 == last && (highest_one_bit(s2) & ROMAN_MASK) != 0) {
                    return MAX_SAFE;
                }
                result += (s2 - s1) as i64;
                reps = 3;
                i += 1;
            }
            last = s1;
        } else {
            result += s1 as i64;
            reps = if s1 == last { reps + 1 } else { 1 };
        }
        if reps >= 4 || (reps >= 2 && (highest_one_bit(s1) & ROMAN_MASK) != 0) {
            return MAX_SAFE;
        }
        i += 1;
    }
    result as f64
}

pub fn is_repdigit(s: &str) -> bool {
    if parse_int(s, None) < 10.0 {
        return true;
    }
    let b = s.as_bytes();
    if b.len() >= 2 && b[0].is_ascii_digit() && b.iter().all(|&c| c == b[0]) {
        return true;
    }
    let len = b.len();
    for parts in 2..=4 {
        if len.is_multiple_of(parts) {
            let w = len / parts;
            if (1..parts).all(|k| b[..w] == b[k * w..(k + 1) * w]) {
                return true;
            }
        }
    }
    false
}

pub fn find_next_repdigit(cur: f64) -> Option<f64> {
    let mut n = cur + 1.0;
    for _ in 0..999_999 {
        if is_repdigit(&num_to_string(n)) {
            return Some(n);
        }
        n += 1.0;
    }
    None
}

fn digit_counts(s: &str) -> std::collections::HashMap<char, u32> {
    let mut m = std::collections::HashMap::new();
    for c in s.chars() {
        *m.entry(c).or_insert(0) += 1;
    }
    m
}

pub fn has_any_repeated_digit(s: &str) -> bool {
    digit_counts(s).values().any(|&c| c >= 2)
}

pub fn has_all_digits_repeated_at_least_twice(s: &str) -> bool {
    let m = digit_counts(s);
    !m.is_empty() && m.values().all(|&c| c >= 2)
}

pub fn find_next_count_by_predicate(prev: &str, pred: impl Fn(&str) -> bool) -> Option<f64> {
    let p = parse_int(prev, Some(10));
    if !p.is_finite() {
        return None;
    }
    let mut c = p + 1.0;
    for _ in 0..1_000_000 {
        let t = num_to_string(c);
        if !t.starts_with('0') && pred(&t) {
            return Some(c);
        }
        c += 1.0;
    }
    None
}

fn factorial(mut n: f64) -> f64 {
    let mut r = 1.0;
    while n > 0.0 {
        r *= n;
        n -= 1.0;
    }
    r
}

fn nth_permutation(set: &[char], prefix: String, k: f64, n: f64) -> String {
    if k == 0.0 {
        return prefix;
    }
    let f = factorial(set.len() as f64 - 1.0);
    let idx = (n / f).floor();
    let pick = set.get(idx as usize).map_or("undefined".to_string(), |c| c.to_string());
    let rest: Vec<char> = set
        .iter()
        .enumerate()
        .filter(|&(i, _)| (i as f64) != idx)
        .map(|(_, c)| *c)
        .collect();
    nth_permutation(&rest, prefix + &pick, k - 1.0, n % f)
}

fn nth_permutation_no_leading_zero(set: &[char], k: f64, mut n: f64) -> String {
    n *= factorial(set.len() as f64 - k + 1.0);
    let f = factorial(set.len() as f64);
    let idx = (n / f).floor();
    let pick = set.get(idx as usize).map_or("undefined".to_string(), |c| c.to_string());
    let mut chars = vec!['0'];
    chars.extend(
        set.iter()
            .enumerate()
            .filter(|&(i, _)| (i as f64) != idx)
            .map(|(_, c)| *c),
    );
    nth_permutation(&chars, pick, k - 1.0, n % f)
}

pub fn no_repdig_get_count(num: f64) -> Option<String> {
    if num <= 0.0 || num > 8877690.0 || num.is_nan() {
        return None;
    }
    let mut num = num - 1.0;
    let set: Vec<char> = "123456789".chars().collect();
    let perms = factorial(set.len() as f64);
    let mut i = 0usize;
    while i <= set.len() {
        let size = (set.len() as f64 * perms) / factorial(set.len() as f64 - i as f64);
        if num >= size {
            num -= size;
        } else {
            break;
        }
        i += 1;
    }
    Some(nth_permutation_no_leading_zero(&set, i as f64 + 1.0, num))
}

pub fn decimal_to_factoradic(mut d: f64) -> Option<String> {
    if d < 0.0 || d.fract() != 0.0 || !d.is_finite() {
        return None;
    }
    if d == 0.0 {
        return Some("0".into());
    }
    let mut parts = Vec::new();
    let mut pos = 1.0;
    while d > 0.0 {
        parts.push(num_to_string(d % pos));
        d = (d / pos).floor();
        pos += 1.0;
    }
    parts.reverse();
    let mut s = parts.concat();
    s.pop();
    Some(s)
}

fn pad2(n: f64) -> String {
    format!("{:0>2}", num_to_string(n))
}

pub fn seconds_to_time_string(s: f64) -> Option<String> {
    if s < 0.0 || s.fract() != 0.0 || !s.is_finite() {
        return None;
    }
    let total = s % 86400.0;
    let h = (total / 3600.0).floor() % 12.0;
    let m = ((total % 3600.0) / 60.0).floor();
    let sec = total % 60.0;
    let period = if (s / 43200.0).floor() % 2.0 == 0.0 { "AM" } else { "PM" };
    Some(format!(
        "{}:{}:{} {}",
        num_to_string(if h == 0.0 { 12.0 } else { h }),
        pad2(m),
        pad2(sec),
        period
    ))
}

pub fn seconds_to_military_time_string(s: f64) -> Option<String> {
    if s < 0.0 || s.fract() != 0.0 || !s.is_finite() {
        return None;
    }
    let total = s % 86400.0;
    let h = (total / 3600.0).floor();
    let m = ((total % 3600.0) / 60.0).floor();
    Some(format!("{}:{}:{}", pad2(h), pad2(m), pad2(total % 60.0)))
}

pub fn days_to_date(days: f64) -> Option<String> {
    if days < 1.0 || days.fract() != 0.0 || !days.is_finite() {
        return None;
    }
    let z = (days - 1.0) as i64 + 306;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    if y > 275760 {
        return Some("Invalid Date".into());
    }
    Some(format!("{} {}, {}", MONTHS[(m - 1) as usize], d, y))
}

pub fn next_rational(prev: &str) -> String {
    let Some((m, a, b)) = rational_parts(prev) else {
        return String::new();
    };
    if m.len() != prev.len() {
        return String::new();
    }
    let (n, d) = (parse_int(a, Some(10)), parse_int(b, Some(10)));
    if !n.is_finite() || !d.is_finite() {
        return String::new();
    }
    let diag = n + d - 1.0;
    if diag <= 0.0 {
        return String::new();
    }
    let s = num_to_string;
    if diag % 2.0 == 0.0 {
        if d < diag {
            return format!("{}/{}", s(n - 1.0), s(d + 1.0));
        }
        return format!("1/{}", s(diag + 1.0));
    }
    if n < diag {
        return format!("{}/{}", s(n + 1.0), s(d - 1.0));
    }
    format!("{}/1", s(diag + 1.0))
}

const PERM_SYMBOLS: &str = "123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

pub fn next_permutation(prev: &str) -> String {
    let rank = |u: u16| PERM_SYMBOLS.bytes().position(|b| b as u16 == u);
    let mut cs: Vec<u16> = prev.encode_utf16().collect();
    if cs.is_empty() {
        return "1".into();
    }
    let mut seen = std::collections::HashSet::new();
    if !cs.iter().all(|c| seen.insert(*c)) || cs.iter().any(|&c| rank(c).is_none()) {
        return String::new();
    }
    let r = |c: u16| rank(c).unwrap();
    let mut i = cs.len() as isize - 2;
    while i >= 0 && r(cs[i as usize]) >= r(cs[i as usize + 1]) {
        i -= 1;
    }
    if i >= 0 {
        let i = i as usize;
        let mut j = cs.len() - 1;
        while r(cs[j]) <= r(cs[i]) {
            j -= 1;
        }
        cs.swap(i, j);
        cs[i + 1..].reverse();
        String::from_utf16_lossy(&cs)
    } else {
        let next_len = cs.len() + 1;
        if next_len > PERM_SYMBOLS.len() {
            return String::new();
        }
        let mut out = String::from("1");
        out.extend(PERM_SYMBOLS.chars().filter(|&c| c != '1').take(next_len - 1));
        out
    }
}

pub fn decimal_to_mayan(n: f64) -> String {
    if n == 0.0 {
        return "~".into();
    }
    let mut chunks: Vec<String> = Vec::new();
    let mut p = 0;
    while 20f64.powi(p) <= n {
        let mut r = (n / 20f64.powi(p)).floor() % 20.0;
        let mut chunk = String::new();
        while r > 0.0 {
            if r >= 5.0 {
                chunk.insert(0, '-');
                r -= 5.0;
            } else {
                chunk.push('.');
                r -= 1.0;
            }
        }
        chunks.insert(0, chunk);
        p += 1;
    }
    chunks.join(";")
}

pub fn next_wave_number(prev: &str) -> Option<String> {
    let (_, a, g) = wave_parts(prev)?;
    let (mut cur, mut goal) = (parse_int(a, Some(10)), parse_int(g, Some(10)));
    if goal < 0.0 {
        cur -= 1.0;
        if cur <= goal {
            goal *= -1.0;
        }
    } else {
        cur += 1.0;
        if cur > goal {
            goal = -(goal + 1.0);
        }
    }
    Some(format!("{} ({})", num_to_string(cur), num_to_string(goal)))
}

pub fn next_2048(seq: &str) -> String {
    let mut out: Vec<f64> = seq.split(' ').map(|n| parse_int(n, None)).collect();
    out.push(2.0);
    while out.len() >= 2 {
        let a = out[out.len() - 1];
        let b = out[out.len() - 2];
        if a != b {
            break;
        }
        out.truncate(out.len() - 2);
        out.push(a + b);
    }
    out.iter().map(|&n| num_to_string(n)).collect::<Vec<_>>().join(" ")
}

pub fn next_2187(previous: &str) -> Option<String> {
    let mut tiles = previous
        .split(' ')
        .map(str::parse::<u64>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;

    let mut carry = 3;
    while tiles.ends_with(&[carry, carry]) {
        tiles.truncate(tiles.len() - 2);
        carry = carry.checked_mul(3)?;
    }

    tiles.push(carry);
    Some(tiles.iter().map(u64::to_string).collect::<Vec<_>>().join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generators() {
        assert_eq!(number_to_words(120.0).unwrap(), "One hundred twenty");
        assert_eq!(number_to_words(1001000.0).unwrap(), "One million one thousand");
        assert_eq!(number_to_ordinal_words(101.0).unwrap(), "One hundred and first");
        assert_eq!(number_to_ordinal_words(1000.0).unwrap(), "One thousandth");
        assert_eq!(decimal_to_base62(62.0), "10");
        assert_eq!(decimal_to_base64(64.0), "BA");
        assert_eq!(decimal_to_factoradic(5.0).unwrap(), "21");
        assert_eq!(seconds_to_time_string(43200.0).unwrap(), "12:00:00 PM");
        assert_eq!(seconds_to_time_string(3661.0).unwrap(), "1:01:01 AM");
        assert_eq!(seconds_to_military_time_string(3661.0).unwrap(), "01:01:01");
        assert_eq!(days_to_date(1.0).unwrap(), "January 1, 1");
        assert_eq!(days_to_date(738000.0).unwrap(), "July 29, 2021");
        assert_eq!(days_to_date(577738.0).unwrap(), "October 17, 1582");
        assert_eq!(next_rational("1/1"), "2/1");
        assert_eq!(next_rational("2/1"), "1/2");
        assert_eq!(next_rational("1/2"), "1/3");
        assert_eq!(next_permutation("21"), "123");
        assert_eq!(next_permutation("132"), "213");
        assert_eq!(decimal_to_mayan(21.0), ".;.");
        assert_eq!(decimal_to_mayan(7.0), "-..");
        assert_eq!(next_wave_number("3 (3)").unwrap(), "4 (-4)");
        assert_eq!(next_wave_number("-4 (-4)").unwrap(), "-5 (4)");
        assert_eq!(next_2048("2 4 2"), "2 8");
        assert_eq!(roman_to_decimal("XIV"), 14.0);
        assert_eq!(roman_to_decimal("IIII"), MAX_SAFE);
        assert_eq!(roman_to_decimal("V\u{305}"), 5000.0);
        assert_eq!(find_next_repdigit(99.0), Some(111.0));
        assert_eq!(find_next_repdigit(1212.0), Some(1313.0));
        assert_eq!(no_repdig_get_count(10.0).unwrap(), "10");
        assert_eq!(no_repdig_get_count(11.0).unwrap(), "12");
        assert_eq!(letters_to_decimal("AA"), 27.0);
        assert_eq!(extract_date("march 5, 2020 hi"), Some("march 5, 2020"));
        assert_eq!(extract_time("12:00:00 PM x"), Some("12:00:00 PM"));
    }
}
