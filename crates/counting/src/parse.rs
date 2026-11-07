use crate::ValidationType as V;
use crate::helpers;
use crate::js::{self, is_space};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Parsed {
    pub is_count: bool,
    pub count_content: Option<String>,
    pub raw_count: Option<String>,
    pub comment: Option<String>,
}

impl Parsed {
    fn comment_only(s: &str) -> Self {
        Parsed {
            comment: Some(s.to_owned()),
            ..Default::default()
        }
    }
}

pub fn parse_spaces(input: &str) -> &str {
    js::trim(input)
}

fn char_off(s: &str, n: usize) -> usize {
    s.char_indices().nth(n).map_or(s.len(), |(i, _)| i)
}

fn strip_spaces(s: &str) -> &str {
    s.trim_start_matches(' ')
}

fn strip_ws_keep_newlines(s: &str) -> &str {
    s.trim_start_matches(|c: char| is_space(c) && c != '\r' && c != '\n')
}

fn is_sep(c: char) -> bool {
    c == ',' || c == '.' || is_space(c)
}

fn strip_seps(s: &str) -> String {
    s.chars().filter(|&c| !is_sep(c)).collect()
}

fn first_double_sep(s: &str) -> Option<usize> {
    let cs: Vec<char> = s.chars().collect();
    (0..cs.len().saturating_sub(1)).find(|&i| is_sep(cs[i]) && is_sep(cs[i + 1]))
}

fn split_count(trimmed: &str, matched: &str) -> Parsed {
    if let Some(idx) = first_double_sep(matched) {
        let before = &matched[..char_off(matched, idx)];
        let count_content = js::trim(before).to_owned();
        let comment = strip_spaces(&trimmed[char_off(trimmed, idx)..]).to_owned();
        let raw = strip_seps(&count_content);
        Parsed {
            is_count: true,
            count_content: Some(count_content),
            raw_count: Some(raw),
            comment: Some(comment),
        }
    } else {
        let count_content = js::trim(matched).to_owned();
        let raw = strip_seps(&count_content);
        let comment = strip_ws_keep_newlines(&trimmed[count_content.len()..]).to_owned();
        Parsed {
            is_count: true,
            count_content: Some(count_content),
            raw_count: Some(raw),
            comment: Some(comment),
        }
    }
}

fn match_numeric(s: &str) -> Option<&str> {
    let b = s.as_bytes();
    let start = usize::from(b.first() == Some(&b'-'));
    if !b.get(start).is_some_and(u8::is_ascii_digit) {
        return None;
    }
    let body = start + 1;
    let mut last_ok = body;
    for (i, c) in s[body..].char_indices() {
        if c.is_ascii_digit() || is_space(c) {
            last_ok = body + i + c.len_utf8();
        } else if c != ',' && c != '.' {
            break;
        }
    }
    Some(&s[..last_ok])
}

fn match_digits_ws(s: &str, max_digit: u8) -> Option<&str> {
    let ok = |c: char| c.is_ascii_digit() && (c as u8 - b'0') <= max_digit;
    let mut it = s.char_indices();
    match it.next() {
        Some((_, c)) if ok(c) => {}
        _ => return None,
    }
    let end = it.find(|&(_, c)| !(ok(c) || is_space(c))).map_or(s.len(), |(i, _)| i);
    Some(&s[..end])
}

fn match_while(s: &str, f: impl Fn(char) -> bool) -> Option<&str> {
    let end = s.char_indices().find(|&(_, c)| !f(c)).map_or(s.len(), |(i, _)| i);
    (end > 0).then(|| &s[..end])
}

fn match_coord_num(s: &str, i: usize) -> Option<usize> {
    let b = s.as_bytes();
    let mut j = i;
    if b.get(j) == Some(&b'-') {
        j += 1;
    }
    if !b.get(j).is_some_and(u8::is_ascii_digit) {
        return None;
    }
    j += 1;
    loop {
        if b.get(j).is_some_and(u8::is_ascii_digit) {
            j += 1;
            continue;
        }
        match s[j..].chars().next() {
            Some(c) if is_sep(c) => {
                let k = j + c.len_utf8();
                if b.get(k).is_some_and(u8::is_ascii_digit) {
                    j = k + 1;
                    continue;
                }
                return Some(j);
            }
            _ => return Some(j),
        }
    }
}

pub(crate) fn match_coordinates(s: &str) -> Option<(&str, &str, &str)> {
    if !s.starts_with('(') {
        return None;
    }
    let a_end = match_coord_num(s, 1)?;
    if !s[a_end..].starts_with(", ") {
        return None;
    }
    let b_start = a_end + 2;
    let b_end = match_coord_num(s, b_start)?;
    if !s[b_end..].starts_with(')') {
        return None;
    }
    Some((&s[..b_end + 1], &s[1..a_end], &s[b_start..b_end]))
}

fn match_2048(s: &str) -> Option<&str> {
    let b = s.as_bytes();
    let mut end = b.iter().take_while(|c| c.is_ascii_digit()).count();
    if end == 0 {
        return None;
    }
    loop {
        let rest = &s[end..];
        let ws: usize = rest.chars().take_while(|&c| is_space(c)).map(char::len_utf8).sum();
        if ws == 0 {
            break;
        }
        let d = rest[ws..].bytes().take_while(u8::is_ascii_digit).count();
        if d == 0 {
            break;
        }
        end += ws + d;
    }
    Some(&s[..end])
}

fn is_roman_char(c: char) -> bool {
    matches!(c.to_ascii_lowercase(), 'i' | 'v' | 'x' | 'l' | 'c' | 'd' | 'm') || c == '\u{305}'
}

fn simple(trimmed: &str, m: Option<&str>, raw: impl Fn(&str) -> String) -> Parsed {
    match m {
        Some(m) if !m.is_empty() => {
            let cc = js::trim(m).to_owned();
            Parsed {
                is_count: true,
                raw_count: Some(raw(&cc)),
                count_content: Some(cc),
                comment: Some(strip_spaces(&trimmed[m.len()..]).to_owned()),
            }
        }
        _ => Parsed::comment_only(trimmed),
    }
}

fn words(trimmed: &str, is_word: impl Fn(&str) -> bool) -> Parsed {
    let (count, comment) = helpers::split_words(trimmed, is_word);
    match count {
        Some(c) if !js::trim(&c).is_empty() => {
            let cc = js::trim(&c).to_owned();
            Parsed {
                is_count: true,
                raw_count: Some(js::capitalize_first_letter(&cc)),
                count_content: Some(cc),
                comment: comment.map(|c| js::trim(&c).to_owned()),
            }
        }
        _ => Parsed::comment_only(trimmed),
    }
}

pub fn parse_input(input: &str, vt: V, is_bot: bool, auto_validated: bool) -> Parsed {
    let t = js::trim(input);
    if is_bot || !auto_validated {
        return Parsed::comment_only(t);
    }
    let id = |s: &str| s.to_owned();
    match vt {
        _ if vt.is_numeric() => match match_numeric(t) {
            Some(m) => split_count(t, m),
            None => Parsed::comment_only(t),
        },
        V::Coordinates | V::CoordinatesTow => match match_coordinates(t) {
            Some((all, a, b)) => Parsed {
                is_count: true,
                count_content: Some(js::trim(all).to_owned()),
                raw_count: Some(format!("({}, {})", strip_seps(a), strip_seps(b))),
                comment: Some(strip_spaces(&t[all.len()..]).to_owned()),
            },
            None => Parsed::comment_only(t),
        },
        V::Letters => simple(t, match_while(t, |c| c.is_ascii_alphabetic()), js::upper),
        V::Binary | V::Ternary | V::Quaternary | V::Quinary | V::Senary | V::Septenary | V::Octonary | V::Nonary => {
            match match_digits_ws(t, vt.base_n_radix().unwrap() as u8 - 1) {
                Some(m) => split_count(t, m),
                None => Parsed::comment_only(t),
            }
        }
        V::Hexadecimal => simple(t, match_while(t, |c| c.is_ascii_hexdigit()), js::upper),
        V::Alphanumerics => simple(t, match_while(t, |c| c.is_ascii_alphanumeric()), js::upper),
        V::Base62 | V::Permutations => simple(t, match_while(t, |c| c.is_ascii_alphanumeric()), id),
        V::Base64 => simple(
            t,
            match_while(t, |c| c.is_ascii_alphanumeric() || c == '+' || c == '/'),
            id,
        ),
        V::Wave => simple(t, helpers::extract_wave_number(t), id),
        V::Mayan => simple(t, match_while(t, |c| matches!(c, '~' | '-' | '.' | ';')), id),
        V::Dates => simple(t, helpers::extract_date(t), id),
        V::Military => simple(t, helpers::extract_military_time(t), id),
        V::Time => simple(t, helpers::extract_time(t), js::upper),
        V::Rationals => simple(t, helpers::extract_rational(t), id),
        V::Roman => simple(t, match_while(t, is_roman_char), js::upper),
        V::Words => words(t, helpers::is_number_word),
        V::Ordinals => words(t, helpers::is_ordinal_word),
        V::G2048 => simple(t, match_2048(t), id),
        _ => Parsed::comment_only(t),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(vt: V, s: &str) -> (bool, Option<String>, Option<String>, Option<String>) {
        let r = parse_input(s, vt, false, true);
        (r.is_count, r.count_content, r.raw_count, r.comment)
    }
    fn s(x: &str) -> Option<String> {
        Some(x.to_owned())
    }

    #[test]
    fn numeric() {
        assert_eq!(p(V::Main, "1,234 hi"), (true, s("1,234"), s("1234"), s("hi")));
        assert_eq!(p(V::Main, "1,234."), (true, s("1,234"), s("1234"), s(".")));
        assert_eq!(p(V::Main, "12  34"), (true, s("12"), s("12"), s("34")));
        assert_eq!(p(V::Main, "12,, x"), (true, s("12"), s("12"), s(",, x")));
        assert_eq!(p(V::Main, "1 2 3"), (true, s("1 2 3"), s("123"), s("")));
        assert_eq!(p(V::Main, "-5 x"), (true, s("-5"), s("-5"), s("x")));
        assert_eq!(p(V::Main, "hi 5"), (false, None, None, s("hi 5")));
        assert_eq!(p(V::Main, "5\n\nhey"), (true, s("5"), s("5"), s("\n\nhey")));
        assert_eq!(p(V::Main, "5 \nhey"), (true, s("5"), s("5"), s("\nhey")));
        assert_eq!(p(V::Main, "5abc"), (true, s("5"), s("5"), s("abc")));
    }

    #[test]
    fn others() {
        assert_eq!(p(V::Binary, "101 1 x"), (true, s("101 1"), s("1011"), s("x")));
        assert_eq!(p(V::Letters, "abc d"), (true, s("abc"), s("ABC"), s("d")));
        assert_eq!(
            p(V::Coordinates, "(1,2, 3) hi"),
            (true, s("(1,2, 3)"), s("(12, 3)"), s("hi"))
        );
        assert_eq!(p(V::Coordinates, "(1, 2, 3)"), (false, None, None, s("(1, 2, 3)")));
        assert_eq!(p(V::Mayan, "hi"), (false, None, None, s("hi")));
        assert_eq!(p(V::G2048, "2 4 8  x"), (true, s("2 4 8"), s("2 4 8"), s("x")));
        assert_eq!(p(V::Roman, "xiv V̅ x"), (true, s("xiv"), s("XIV"), s("V̅ x")));
        assert_eq!(
            p(V::Words, "one hundred  two"),
            (true, s("one hundred"), s("One hundred"), s("two"))
        );
        assert_eq!(
            p(V::Time, "1:00:00 am ok"),
            (true, s("1:00:00 am"), s("1:00:00 AM"), s("ok"))
        );
    }
}
