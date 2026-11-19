#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plain {
    pub id: u64,
    pub uuid: String,
    pub timestamp: String,
    pub time_since_last_count: Option<String>,
    pub time_since_last_post: Option<String>,
    pub raw_text: String,
    pub is_count: bool,
    pub is_valid_count: Option<bool>,
    pub count_content: Option<String>,
    pub raw_count: Option<String>,
    pub stricken: bool,
    pub thread: String,
    pub thread_id: u16,
    pub has_comment: bool,
    pub has_three_char_comment: bool,
    pub comment: Option<String>,
    pub author_uuid: String,
    pub author_id: u32,
    pub is_deleted: bool,
    pub is_comment_deleted: bool,
    pub valid_count_number: Option<i64>,
    pub macro_preset_id: Option<u32>,
    pub macro_preset_version_id: Option<u32>,
}

impl Plain {
    pub fn differences(&self, b: &Plain) -> Vec<&'static str> {
        let a = self;
        let checks: [(&'static str, bool); 23] = [
            ("id", a.id == b.id),
            ("uuid", a.uuid == b.uuid),
            ("timestamp", a.timestamp == b.timestamp),
            ("timeSinceLastCount", a.time_since_last_count == b.time_since_last_count),
            ("timeSinceLastPost", a.time_since_last_post == b.time_since_last_post),
            ("rawText", a.raw_text == b.raw_text),
            ("isCount", a.is_count == b.is_count),
            ("isValidCount", a.is_valid_count == b.is_valid_count),
            ("countContent", a.count_content == b.count_content),
            ("rawCount", a.raw_count == b.raw_count),
            ("stricken", a.stricken == b.stricken),
            ("thread", a.thread == b.thread),
            ("threadId", a.thread_id == b.thread_id),
            ("hasComment", a.has_comment == b.has_comment),
            (
                "hasThreeCharComment",
                a.has_three_char_comment == b.has_three_char_comment,
            ),
            ("comment", a.comment == b.comment),
            ("authorUUID", a.author_uuid == b.author_uuid),
            ("authorId", a.author_id == b.author_id),
            ("isDeleted", a.is_deleted == b.is_deleted),
            ("isCommentDeleted", a.is_comment_deleted == b.is_comment_deleted),
            ("validCountNumber", a.valid_count_number == b.valid_count_number),
            ("macroPresetId", a.macro_preset_id == b.macro_preset_id),
            (
                "macroPresetVersionId",
                a.macro_preset_version_id == b.macro_preset_version_id,
            ),
        ];
        checks.iter().filter(|c| !c.1).map(|c| c.0).collect()
    }

    pub fn is_valid(&self) -> bool {
        self.is_valid_count == Some(true)
    }
}
