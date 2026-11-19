use crate::Plain;
use sqlx::Row;
use sqlx::mysql::MySqlRow;

fn flag(r: &MySqlRow, col: &str) -> Result<bool, sqlx::Error> {
    Ok(r.try_get::<i8, _>(col)? != 0)
}

fn small(r: &MySqlRow, col: &str) -> Result<Option<u32>, sqlx::Error> {
    Ok(r.try_get::<Option<i32>, _>(col)?.and_then(|v| u32::try_from(v).ok()))
}

pub fn plain(r: &MySqlRow) -> Result<Plain, sqlx::Error> {
    Ok(Plain {
        id: u64::try_from(r.try_get::<i32, _>("id")?).map_err(|e| sqlx::Error::Decode(Box::new(e)))?,
        uuid: r.try_get("uuid")?,
        timestamp: r.try_get("timestamp")?,
        time_since_last_count: r.try_get("timeSinceLastCount")?,
        time_since_last_post: r.try_get("timeSinceLastPost")?,
        raw_text: r.try_get("rawText")?,
        is_count: flag(r, "isCount")?,
        is_valid_count: r.try_get::<Option<i8>, _>("isValidCount")?.map(|v| v != 0),
        count_content: r.try_get("countContent")?,
        raw_count: r.try_get("rawCount")?,
        stricken: flag(r, "stricken")?,
        thread: r.try_get("thread")?,
        thread_id: r.try_get::<Option<u16>, _>("threadId")?.unwrap_or(0),
        has_comment: flag(r, "hasComment")?,
        has_three_char_comment: flag(r, "hasThreeCharComment")?,
        comment: r.try_get("comment")?,
        author_uuid: r.try_get("authorUUID")?,
        author_id: r.try_get::<Option<u32>, _>("authorId")?.unwrap_or(0),
        is_deleted: flag(r, "isDeleted")?,
        is_comment_deleted: flag(r, "isCommentDeleted")?,
        valid_count_number: r.try_get::<Option<i32>, _>("validCountNumber")?.map(i64::from),
        macro_preset_id: small(r, "macroPresetId")?,
        macro_preset_version_id: small(r, "macroPresetVersionId")?,
    })
}
