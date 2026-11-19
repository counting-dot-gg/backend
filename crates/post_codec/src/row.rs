#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flag {
    IsCount,
    ValidKnown,
    Valid,
    Stricken,
    HasComment,
    HasThreeCharComment,
    Deleted,
    CommentDeleted,
    CountContentPresent,
    CommentPresent,
    ValidCountNumberPresent,
    TsRaw,
    UuidRaw,
    GapCountNull,
    GapPostNull,
    RawTextRaw,
    RawCountNull,
    RawCountRaw,
}

impl Flag {
    pub const ALL: [Flag; 18] = [
        Flag::IsCount,
        Flag::ValidKnown,
        Flag::Valid,
        Flag::Stricken,
        Flag::HasComment,
        Flag::HasThreeCharComment,
        Flag::Deleted,
        Flag::CommentDeleted,
        Flag::CountContentPresent,
        Flag::CommentPresent,
        Flag::ValidCountNumberPresent,
        Flag::TsRaw,
        Flag::UuidRaw,
        Flag::GapCountNull,
        Flag::GapPostNull,
        Flag::RawTextRaw,
        Flag::RawCountNull,
        Flag::RawCountRaw,
    ];

    pub fn bit(self) -> u32 {
        1 << self as u32
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Row {
    pub thread_id: u16,
    pub id: u64,
    pub ts_us: u64,
    pub uuid_rest: u32,
    pub flags: u32,
    pub valid_count_number: i64,
    pub pos: i64,
    pub streak: i32,
    pub count_content: String,
    pub comment: String,
    pub thread: String,
    pub author_id: u32,
    pub author_uuid: String,
    pub macro_preset_id: u32,
    pub macro_preset_version_id: u32,
    pub overrides: Overrides,
    pub enc_version: u8,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Overrides {
    pub ts: String,
    pub uuid: String,
    pub raw_text: String,
    pub raw_count: String,
}

impl Row {
    pub fn has(&self, f: Flag) -> bool {
        self.flags & f.bit() != 0
    }

    pub fn set(&mut self, f: Flag, on: bool) {
        if on {
            self.flags |= f.bit();
        }
    }
}
