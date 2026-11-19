pub mod derive;
#[cfg(feature = "mysql")]
pub mod mysql;
pub mod plain;
pub mod row;
pub mod tsv;

pub use plain::Plain;
pub use row::{Flag, Overrides, Row};

pub const ENC_VERSION: u8 = 1;

#[derive(Debug, Clone, Default)]
pub struct Context {
    pub last_post_ts: Option<String>,
    pub last_valid_ts: Option<String>,
    pub pos: i64,
    pub streak: i32,
}

impl Context {
    pub fn advance(&mut self, p: &Plain) {
        self.last_post_ts = Some(p.timestamp.clone());
        if p.is_valid() {
            self.last_valid_ts = Some(p.timestamp.clone());
            if let Some(n) = p.valid_count_number {
                self.pos = n;
            }
            self.streak = self.streak.max(0) + 1;
        } else if p.is_count {
            self.streak = self.streak.min(0) - 1;
        }
    }
}

fn shifted(v: Option<u32>) -> u32 {
    v.map_or(0, |x| x + 1)
}

fn unshifted(v: u32) -> Option<u32> {
    v.checked_sub(1)
}

fn derived_or_raw(original: Option<&str>, derived: &str, null: Flag, raw: Flag, row: &mut Row) -> String {
    match original {
        None => {
            row.set(null, true);
            String::new()
        }
        Some(s) if s == derived => String::new(),
        Some(s) => {
            row.set(raw, true);
            s.to_owned()
        }
    }
}

fn restored(row: &Row, derived: impl FnOnce() -> String, stored: &str, null: Flag, raw: Flag) -> Option<String> {
    if row.has(null) {
        None
    } else if row.has(raw) {
        Some(stored.to_owned())
    } else {
        Some(derived())
    }
}

pub fn encode(ctx: &Context, p: &Plain) -> Row {
    let mut row = Row {
        thread_id: p.thread_id,
        id: p.id,
        ts_us: derive::ts_us_lossy(&p.timestamp),
        valid_count_number: p.valid_count_number.unwrap_or(0),
        thread: p.thread.clone(),
        author_id: p.author_id,
        author_uuid: p.author_uuid.clone(),
        macro_preset_id: shifted(p.macro_preset_id),
        macro_preset_version_id: shifted(p.macro_preset_version_id),
        enc_version: ENC_VERSION,
        ..Row::default()
    };
    row.set(Flag::IsCount, p.is_count);
    row.set(Flag::ValidKnown, p.is_valid_count.is_some());
    row.set(Flag::Valid, p.is_valid());
    row.set(Flag::Stricken, p.stricken);
    row.set(Flag::HasComment, p.has_comment);
    row.set(Flag::HasThreeCharComment, p.has_three_char_comment);
    row.set(Flag::Deleted, p.is_deleted);
    row.set(Flag::CommentDeleted, p.is_comment_deleted);
    row.set(Flag::CountContentPresent, p.count_content.is_some());
    row.set(Flag::CommentPresent, p.comment.is_some());
    row.set(Flag::ValidCountNumberPresent, p.valid_count_number.is_some());
    row.count_content = p.count_content.clone().unwrap_or_default();
    row.comment = p.comment.clone().unwrap_or_default();

    if derive::timestamp_of(row.ts_us) != p.timestamp {
        row.set(Flag::TsRaw, true);
        row.overrides.ts = p.timestamp.clone();
    }
    match derive::uuid_rest(&p.uuid, row.ts_us).filter(|_| !row.has(Flag::TsRaw)) {
        Some(rest) => row.uuid_rest = rest,
        None => {
            row.set(Flag::UuidRaw, true);
            row.overrides.uuid = p.uuid.clone();
        }
    }

    row.set(Flag::GapPostNull, p.time_since_last_post.is_none());
    row.set(Flag::GapCountNull, p.time_since_last_count.is_none());

    if derive::raw_text(p.count_content.as_deref(), p.comment.as_deref()) != p.raw_text {
        row.set(Flag::RawTextRaw, true);
        row.overrides.raw_text = p.raw_text.clone();
    }
    let raw_count_derived = p.count_content.clone().unwrap_or_default();
    row.overrides.raw_count = derived_or_raw(
        p.raw_count.as_deref(),
        &raw_count_derived,
        Flag::RawCountNull,
        Flag::RawCountRaw,
        &mut row,
    );

    let mut next = ctx.clone();
    next.advance(p);
    row.pos = next.pos;
    row.streak = next.streak;
    row
}

pub fn decode(ctx: &Context, row: &Row) -> Plain {
    let timestamp = if row.has(Flag::TsRaw) {
        row.overrides.ts.clone()
    } else {
        derive::timestamp_of(row.ts_us)
    };
    let uuid = if row.has(Flag::UuidRaw) {
        row.overrides.uuid.clone()
    } else {
        derive::uuid_from_rest(row.ts_us, row.uuid_rest)
    };
    let count_content = row.has(Flag::CountContentPresent).then(|| row.count_content.clone());
    let comment = row.has(Flag::CommentPresent).then(|| row.comment.clone());
    let raw_text = if row.has(Flag::RawTextRaw) {
        row.overrides.raw_text.clone()
    } else {
        derive::raw_text(count_content.as_deref(), comment.as_deref())
    };
    let time_since_last_post =
        (!row.has(Flag::GapPostNull)).then(|| derive::gap(&timestamp, ctx.last_post_ts.as_deref()));
    let time_since_last_count =
        (!row.has(Flag::GapCountNull)).then(|| derive::gap(&timestamp, ctx.last_valid_ts.as_deref()));
    let raw_count = restored(
        row,
        || count_content.clone().unwrap_or_default(),
        &row.overrides.raw_count,
        Flag::RawCountNull,
        Flag::RawCountRaw,
    );
    Plain {
        id: row.id,
        uuid,
        timestamp,
        time_since_last_count,
        time_since_last_post,
        raw_text,
        is_count: row.has(Flag::IsCount),
        is_valid_count: row.has(Flag::ValidKnown).then(|| row.has(Flag::Valid)),
        count_content,
        raw_count,
        stricken: row.has(Flag::Stricken),
        thread: row.thread.clone(),
        thread_id: row.thread_id,
        has_comment: row.has(Flag::HasComment),
        has_three_char_comment: row.has(Flag::HasThreeCharComment),
        comment,
        author_uuid: row.author_uuid.clone(),
        author_id: row.author_id,
        is_deleted: row.has(Flag::Deleted),
        is_comment_deleted: row.has(Flag::CommentDeleted),
        valid_count_number: row.has(Flag::ValidCountNumberPresent).then_some(row.valid_count_number),
        macro_preset_id: unshifted(row.macro_preset_id),
        macro_preset_version_id: unshifted(row.macro_preset_version_id),
    }
}

pub const GAP_FIELDS: [&str; 2] = ["timeSinceLastPost", "timeSinceLastCount"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repair {
    pub id: u64,
    pub field: &'static str,
    pub old: Option<String>,
    pub new: Option<String>,
}

impl Repair {
    pub fn is_float_noise(&self) -> bool {
        let parse = |v: &Option<String>| v.as_deref().and_then(|s| s.parse::<f64>().ok());
        matches!((parse(&self.old), parse(&self.new)), (Some(a), Some(b)) if (a - b).abs() < 0.001)
    }
}

pub struct Checked {
    pub row: Row,
    pub repairs: Vec<Repair>,
    pub mismatches: Vec<&'static str>,
}

pub fn encode_checked(ctx: &Context, p: &Plain) -> Checked {
    let row = encode(ctx, p);
    let back = decode(ctx, &row);
    let repairs = [
        (GAP_FIELDS[0], &p.time_since_last_post, &back.time_since_last_post),
        (GAP_FIELDS[1], &p.time_since_last_count, &back.time_since_last_count),
    ]
    .into_iter()
    .filter(|(_, old, new)| old != new)
    .map(|(field, old, new)| Repair {
        id: p.id,
        field,
        old: old.clone(),
        new: new.clone(),
    })
    .collect();
    let mismatches = p
        .differences(&back)
        .into_iter()
        .filter(|f| !GAP_FIELDS.contains(f))
        .collect();
    Checked {
        row,
        repairs,
        mismatches,
    }
}

#[cfg(test)]
mod tests {
    use super::Repair;

    fn repair(old: &str, new: &str) -> Repair {
        Repair {
            id: 1,
            field: GAP_FIELD,
            old: Some(old.into()),
            new: Some(new.into()),
        }
    }

    const GAP_FIELD: &str = "timeSinceLastPost";

    #[test]
    fn last_digit_differences_are_float_noise() {
        assert!(repair("3598517.0688476562", "3598517.0688476563").is_float_noise());
        assert!(!repair("137335", "137336.01586914063").is_float_noise());
    }
}
