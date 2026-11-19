use crate::plain::Plain;
use crate::row::Row;

pub fn unescape(field: &str) -> Option<String> {
    if field == "\\N" {
        return None;
    }
    let mut out = String::with_capacity(field.len());
    let mut chars = field.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('0') => out.push('\0'),
            Some('b') => out.push('\u{8}'),
            Some('f') => out.push('\u{c}'),
            Some('a') => out.push('\u{7}'),
            Some('v') => out.push('\u{b}'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    Some(out)
}

pub fn escape(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\0' => out.push_str("\\0"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            _ => out.push(c),
        }
    }
}

pub const PLAIN_COLUMNS: &str = "id, uuid, timestamp, timeSinceLastCount, timeSinceLastPost, rawText, isCount, \
    isValidCount, countContent, rawCount, stricken, thread, threadId, hasComment, hasThreeCharComment, comment, \
    authorUUID, authorId, isDeleted, isCommentDeleted, validCountNumber, macroPresetId, macroPresetVersionId";

#[derive(Debug)]
pub struct BadLine(pub String);

struct Fields<'a> {
    it: std::str::Split<'a, char>,
    line: &'a str,
}

impl Fields<'_> {
    fn opt(&mut self) -> Result<Option<String>, BadLine> {
        self.it
            .next()
            .map(unescape)
            .ok_or_else(|| BadLine(self.line.chars().take(200).collect()))
    }

    fn text(&mut self) -> Result<String, BadLine> {
        Ok(self.opt()?.unwrap_or_default())
    }

    fn num<T: std::str::FromStr>(&mut self) -> Result<Option<T>, BadLine> {
        let line = self.line;
        self.opt()?
            .map(|s| s.parse().map_err(|_| BadLine(line.chars().take(200).collect())))
            .transpose()
    }

    fn flag(&mut self) -> Result<bool, BadLine> {
        Ok(self.num::<u8>()?.unwrap_or(0) != 0)
    }
}

pub fn parse_plain(line: &str) -> Result<Plain, BadLine> {
    let mut f = Fields {
        it: line.split('\t'),
        line,
    };
    Ok(Plain {
        id: f.num()?.unwrap_or(0),
        uuid: f.text()?,
        timestamp: f.text()?,
        time_since_last_count: f.opt()?,
        time_since_last_post: f.opt()?,
        raw_text: f.text()?,
        is_count: f.flag()?,
        is_valid_count: f.num::<u8>()?.map(|v| v != 0),
        count_content: f.opt()?,
        raw_count: f.opt()?,
        stricken: f.flag()?,
        thread: f.text()?,
        thread_id: f.num()?.unwrap_or(0),
        has_comment: f.flag()?,
        has_three_char_comment: f.flag()?,
        comment: f.opt()?,
        author_uuid: f.text()?,
        author_id: f.num()?.unwrap_or(0),
        is_deleted: f.flag()?,
        is_comment_deleted: f.flag()?,
        valid_count_number: f.num()?,
        macro_preset_id: f.num()?,
        macro_preset_version_id: f.num()?,
    })
}

pub const ROW_COLUMNS: &str = "threadId, id, tsUs, uuidRest, flags, validCountNumber, pos, streak, countContent, \
    comment, thread, authorId, authorUUID, macroPresetId, macroPresetVersionId, tsRaw, uuidRaw, \
    rawTextRaw, rawCountRaw, encVersion";

pub fn write_row(r: &Row, out: &mut String) {
    let nums = [
        r.thread_id.to_string(),
        r.id.to_string(),
        r.ts_us.to_string(),
        r.uuid_rest.to_string(),
        r.flags.to_string(),
        r.valid_count_number.to_string(),
        r.pos.to_string(),
        r.streak.to_string(),
    ];
    out.push_str(&nums.join("\t"));
    for s in [&r.count_content, &r.comment, &r.thread] {
        out.push('\t');
        escape(s, out);
    }
    out.push('\t');
    out.push_str(&r.author_id.to_string());
    out.push('\t');
    escape(&r.author_uuid, out);
    let tail = [r.macro_preset_id.to_string(), r.macro_preset_version_id.to_string()];
    for t in tail {
        out.push('\t');
        out.push_str(&t);
    }
    let o = &r.overrides;
    for s in [&o.ts, &o.uuid, &o.raw_text, &o.raw_count] {
        out.push('\t');
        escape(s, out);
    }
    out.push('\t');
    out.push_str(&r.enc_version.to_string());
}
