use crate::model::{Counter, Post, ThreadMeta};
use anyhow::Result;
use serde_json::{Map, Value, json};
use sqlx::mysql::MySqlRow;
use sqlx::{MySqlPool, Row};

fn json_col(r: &MySqlRow, c: &str) -> Value {
    r.try_get::<Option<String>, _>(c)
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Value::Null)
}

fn str_list(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect())
        .unwrap_or_default()
}

const THREAD_COLS: &str = "id, uuid, name, title, description, rules, firstValidCount, validationType, \
    resetOnMistakes, allowDoublePosts, CAST(visibleTo AS CHAR) AS visibleTo, CAST(updatableBy AS CHAR) AS updatableBy, \
    locked, autoValidated, CAST(moderators AS CHAR) AS moderators, CAST(verifiers AS CHAR) AS verifiers, \
    CAST(countBans AS CHAR) AS countBans, CAST(postBans AS CHAR) AS postBans, shortDescription, color1, color2, \
    category, countsPerSplit, splitsPerGet, splitOffset, serverId, serverDiscordID, threadOfTheDay, type";

pub async fn load_threads(pool: &MySqlPool) -> Result<Vec<ThreadMeta>> {
    let rows = sqlx::query(&format!("SELECT {THREAD_COLS} FROM thread ORDER BY id"))
        .fetch_all(pool)
        .await?;
    let mut out = Vec::new();
    for r in rows {
        let b = |c: &str| r.try_get::<i8, _>(c).unwrap_or(0) != 0;
        let s = |c: &str| r.try_get::<Option<String>, _>(c).ok().flatten();
        let n = |c: &str| r.try_get::<Option<i32>, _>(c).ok().flatten();
        let mut row = Map::new();
        row.insert("id".into(), json!(n("id")));
        for c in [
            "uuid",
            "name",
            "title",
            "description",
            "rules",
            "firstValidCount",
            "validationType",
        ] {
            row.insert(c.into(), json!(s(c)));
        }
        row.insert("resetOnMistakes".into(), json!(b("resetOnMistakes")));
        row.insert("allowDoublePosts".into(), json!(b("allowDoublePosts")));
        row.insert("visibleTo".into(), json_col(&r, "visibleTo"));
        row.insert("updatableBy".into(), json_col(&r, "updatableBy"));
        row.insert("locked".into(), json!(b("locked")));
        row.insert("autoValidated".into(), json!(b("autoValidated")));
        for c in ["moderators", "verifiers", "countBans", "postBans"] {
            row.insert(c.into(), json_col(&r, c));
        }
        for c in ["shortDescription", "color1", "color2", "category"] {
            row.insert(c.into(), json!(s(c)));
        }
        for c in ["countsPerSplit", "splitsPerGet", "splitOffset", "serverId"] {
            row.insert(c.into(), json!(n(c)));
        }
        row.insert("serverDiscordID".into(), json!(s("serverDiscordID")));
        row.insert("threadOfTheDay".into(), json!(b("threadOfTheDay")));
        row.insert("type".into(), json!(s("type")));
        out.push(ThreadMeta {
            id: n("id").unwrap_or(0) as u32,
            uuid: s("uuid").unwrap_or_default(),
            name: s("name").unwrap_or_default(),
            validation_type: s("validationType").unwrap_or_default(),
            first_valid_count: s("firstValidCount").unwrap_or_default(),
            reset_on_mistakes: b("resetOnMistakes"),
            allow_double_posts: b("allowDoublePosts"),
            auto_validated: b("autoValidated"),
            locked: b("locked"),
            visible_to: str_list(&row["visibleTo"]),
            updatable_by: str_list(&row["updatableBy"]),
            count_bans: str_list(&row["countBans"]),
            post_bans: str_list(&row["postBans"]),
            row,
        });
    }
    Ok(out)
}

const COUNTER_COLS: &str = "id, uuid, discordId, name, username, color, CAST(roles AS CHAR) AS roles, avatar, \
    cardStyle, cardBorderStyle, CAST(pronouns AS CHAR) AS pronouns, title, CAST(xp AS CHAR) AS xp, \
    CAST(xp2 AS CHAR) AS xp2, rainbow, lastRob, emoji";

fn counter_from_row(r: &MySqlRow) -> Counter {
    let s = |c: &str| r.try_get::<Option<String>, _>(c).ok().flatten();
    let id = r.try_get::<i32, _>("id").unwrap_or(0);
    let Value::Object(m) = json!({
        "id": id,
        "uuid": s("uuid"),
        "discordId": s("discordId"),
        "name": s("name"),
        "username": s("username"),
        "color": s("color"),
        "roles": json_col(r, "roles"),
        "avatar": s("avatar"),
        "cardStyle": s("cardStyle"),
        "cardBorderStyle": s("cardBorderStyle"),
        "pronouns": json_col(r, "pronouns"),
        "title": s("title"),
        "xp": s("xp"),
        "xp2": s("xp2"),
        "rainbow": r.try_get::<Option<i32>, _>("rainbow").ok().flatten(),
        "lastRob": s("lastRob"),
        "emoji": s("emoji"),
    }) else {
        unreachable!()
    };
    Counter::from_fields(m)
}

pub async fn counter_by_uuid(pool: &MySqlPool, uuid: &str) -> Result<Option<Counter>> {
    let r = sqlx::query(&format!("SELECT {COUNTER_COLS} FROM counters WHERE uuid = ?"))
        .bind(uuid)
        .fetch_optional(pool)
        .await?;
    Ok(r.as_ref().map(counter_from_row))
}

pub async fn counters_by_uuids(pool: &MySqlPool, uuids: &[String]) -> Result<Vec<Counter>> {
    if uuids.is_empty() {
        return Ok(vec![]);
    }
    let marks = vec!["?"; uuids.len()].join(",");
    let sql = format!("SELECT {COUNTER_COLS} FROM counters WHERE uuid IN ({marks})");
    let mut q = sqlx::query(&sql);
    for u in uuids {
        q = q.bind(u);
    }
    Ok(q.fetch_all(pool).await?.iter().map(counter_from_row).collect())
}

pub const POST_COLS: &str = "id, uuid, timestamp, timeSinceLastCount, timeSinceLastPost, rawText, isCount, \
    isValidCount, validCountNumber, countContent, rawCount, stricken, thread, threadId, hasComment, \
    hasThreeCharComment, comment, authorUUID, authorId, isDeleted, isCommentDeleted, \
    CAST(reactions AS CHAR) AS reactions, macroPresetId, macroPresetVersionId";

pub fn post_from_row(r: &MySqlRow) -> Post {
    let s = |c: &str| r.try_get::<Option<String>, _>(c).ok().flatten();
    let b = |c: &str| r.try_get::<Option<i8>, _>(c).ok().flatten().unwrap_or(0) != 0;
    Post {
        uuid: s("uuid").unwrap_or_default(),
        timestamp: s("timestamp").unwrap_or_default(),
        time_since_last_count: s("timeSinceLastCount").unwrap_or_default(),
        time_since_last_post: s("timeSinceLastPost").unwrap_or_default(),
        raw_text: s("rawText").unwrap_or_default(),
        is_count: b("isCount"),
        is_valid_count: b("isValidCount"),
        valid_count_number: r
            .try_get::<Option<i32>, _>("validCountNumber")
            .ok()
            .flatten()
            .map(i64::from),
        count_content: s("countContent"),
        raw_count: s("rawCount"),
        stricken: b("stricken"),
        thread: s("thread").unwrap_or_default(),
        thread_id: r.try_get::<Option<u16>, _>("threadId").ok().flatten().unwrap_or(0) as u32,
        has_comment: b("hasComment"),
        has_three_char_comment: b("hasThreeCharComment"),
        comment: s("comment"),
        author_uuid: s("authorUUID").unwrap_or_default(),
        author_id: r.try_get::<Option<u32>, _>("authorId").ok().flatten(),
        is_deleted: b("isDeleted"),
        is_comment_deleted: b("isCommentDeleted"),
        reactions: json_col(r, "reactions"),
        macro_preset_id: r
            .try_get::<Option<i32>, _>("macroPresetId")
            .ok()
            .flatten()
            .map(i64::from),
        macro_preset_version_id: r
            .try_get::<Option<i32>, _>("macroPresetVersionId")
            .ok()
            .flatten()
            .map(i64::from),
        id: r.try_get::<i32, _>("id").unwrap_or(0) as u64,
    }
}

pub struct ThreadBoot {
    pub recent_valid: Vec<Post>,
    pub last_post: Option<Post>,
    pub last_stricken_id: Option<u64>,
    pub streak: u64,
    pub slow_recent: Vec<(String, f64)>,
}

pub async fn thread_boot(pool: &MySqlPool, t: &ThreadMeta, now_ms: f64) -> Result<ThreadBoot> {
    let recent_valid: Vec<Post> = sqlx::query(&format!(
        "SELECT {POST_COLS} FROM post FORCE INDEX (n_thread_valid_id) \
         WHERE threadId = ? AND isValidCount = 1 ORDER BY id DESC LIMIT 3"
    ))
    .bind(t.id)
    .fetch_all(pool)
    .await?
    .iter()
    .map(post_from_row)
    .collect();
    let last_post = sqlx::query(&format!(
        "SELECT {POST_COLS} FROM post FORCE INDEX (n_thread_id) WHERE threadId = ? ORDER BY id DESC LIMIT 1"
    ))
    .bind(t.id)
    .fetch_optional(pool)
    .await?
    .as_ref()
    .map(post_from_row);
    let last_stricken_id: Option<u64> = if t.validation_type.is_empty() {
        None
    } else {
        sqlx::query_scalar::<_, i32>(
            "SELECT id FROM post FORCE INDEX (n_thread_valid_id) \
             WHERE threadId = ? AND isValidCount = 0 AND stricken = 1 ORDER BY id DESC LIMIT 1",
        )
        .bind(t.id)
        .fetch_optional(pool)
        .await?
        .map(|v| v as u64)
    };
    let streak: i64 = match last_stricken_id {
        Some(id) => {
            sqlx::query_scalar(
                "SELECT COUNT(*) FROM post FORCE INDEX (n_thread_valid_id) \
             WHERE threadId = ? AND isValidCount = 1 AND id > ?",
            )
            .bind(t.id)
            .bind(id as i64)
            .fetch_one(pool)
            .await?
        }
        None => 0,
    };
    let mut slow_recent = Vec::new();
    if t.validation_type == "slow" {
        let since = counting::js::num_to_string(now_ms - 3_600_000.0);
        let floor: Option<i64> = sqlx::query_scalar("SELECT CAST(MIN(id) AS SIGNED) FROM post WHERE timestamp > ?")
            .bind(&since)
            .fetch_one(pool)
            .await?;
        if let Some(floor) = floor {
            let rows: Vec<(String, String)> = sqlx::query_as(
                "SELECT authorUUID, timestamp FROM post FORCE INDEX (n_thread_valid_id) \
                 WHERE threadId = ? AND isValidCount = 1 AND id >= ? AND timestamp > ? ORDER BY id",
            )
            .bind(t.id)
            .bind(floor)
            .bind(since)
            .fetch_all(pool)
            .await?;
            slow_recent = rows
                .into_iter()
                .map(|(a, ts)| (a, counting::js::parse_float(&ts)))
                .collect();
        }
    }
    Ok(ThreadBoot {
        recent_valid,
        last_post,
        last_stricken_id,
        streak: streak as u64,
        slow_recent,
    })
}

pub async fn max_post_id(pool: &MySqlPool) -> Result<u64> {
    let v: Option<i64> = sqlx::query_scalar("SELECT CAST(MAX(id) AS SIGNED) FROM post")
        .fetch_one(pool)
        .await?;
    Ok(v.unwrap_or(0) as u64)
}

fn col_json(r: &MySqlRow, i: usize) -> Value {
    use sqlx::{Column, TypeInfo, ValueRef};
    let col = &r.columns()[i];
    if r.try_get_raw(i).map(|v| v.is_null()).unwrap_or(true) {
        return Value::Null;
    }
    match col.type_info().name() {
        "BOOLEAN" | "TINYINT" | "TINYINT UNSIGNED" => json!(r.try_get::<i8, _>(i).unwrap_or(0) != 0),
        "SMALLINT" | "SMALLINT UNSIGNED" | "MEDIUMINT" | "MEDIUMINT UNSIGNED" | "INT" | "INT UNSIGNED" => {
            json!(
                r.try_get::<i64, _>(i)
                    .or_else(|_| r.try_get::<i32, _>(i).map(i64::from))
                    .unwrap_or(0)
            )
        }
        "DOUBLE" | "FLOAT" => json!(r.try_get::<f64, _>(i).unwrap_or(0.0)),
        "JSON" => r
            .try_get::<String, _>(i)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(Value::Null),
        _ => json!(r.try_get::<String, _>(i).unwrap_or_default()),
    }
}

pub async fn user_json(pool: &MySqlPool, uuid: &str) -> Result<Option<Value>> {
    use sqlx::Column;
    let Some(r) = sqlx::query(
        "SELECT *, CAST(money AS CHAR) AS money_s, CAST(pref_stricken_count_opacity AS CHAR) AS opacity_s, \
         CAST(inventory AS CHAR) AS inventory_s FROM users WHERE uuid = ?",
    )
    .bind(uuid)
    .fetch_optional(pool)
    .await?
    else {
        return Ok(None);
    };
    let mut m = Map::new();
    for (i, c) in r.columns().iter().enumerate() {
        let name = c.name();
        let key = match name {
            "discord_id" => "discordId",
            "access_token" | "refresh_token" | "redditAccess" | "redditRefresh" => continue,
            "miscSettingsId" | "money_s" | "opacity_s" | "inventory_s" => continue,
            "money" => {
                m.insert("money".into(), json!(r.get::<String, _>("money_s")));
                continue;
            }
            "pref_stricken_count_opacity" => {
                m.insert(name.into(), json!(r.get::<String, _>("opacity_s")));
                continue;
            }
            "inventory" => {
                let v = r
                    .get::<Option<String>, _>("inventory_s")
                    .and_then(|s| serde_json::from_str(&s).ok());
                m.insert(name.into(), v.unwrap_or(Value::Null));
                continue;
            }
            n => n,
        };
        m.insert(key.into(), col_json(&r, i));
    }
    Ok(Some(Value::Object(m)))
}

pub async fn user_items(pool: &MySqlPool, uuid: &str) -> Result<Value> {
    use sqlx::Column;
    let rows = sqlx::query(
        "SELECT i.id, i.name, i.description, i.category, i.unlockMethod, i.unlockDescription, \
         CAST(i.price AS CHAR) AS price, i.quantity, i.levelToUnlock, i.achievementId, i.internal_name, i.uuid, i.season \
         FROM useritem ui JOIN item i ON i.id = ui.itemId WHERE ui.userUuid = ? ORDER BY ui.id",
    )
    .bind(uuid)
    .fetch_all(pool)
    .await?;
    Ok(Value::Array(
        rows.iter()
            .map(|r| {
                let mut m = Map::new();
                for (i, c) in r.columns().iter().enumerate() {
                    m.insert(c.name().into(), col_json(r, i));
                }
                Value::Object(m)
            })
            .collect(),
    ))
}

pub async fn post_by_uuid(pool: &MySqlPool, uuid: &str) -> Result<Option<Post>> {
    let r = sqlx::query(&format!("SELECT {POST_COLS} FROM post WHERE uuid = ? LIMIT 1"))
        .bind(uuid)
        .fetch_optional(pool)
        .await?;
    Ok(r.as_ref().map(post_from_row))
}

pub async fn post_id_in_thread(pool: &MySqlPool, uuid: &str, thread_id: u32) -> Result<Option<u64>> {
    let id: Option<i32> = sqlx::query_scalar("SELECT id FROM post WHERE uuid = ? AND threadId = ? LIMIT 1")
        .bind(uuid)
        .bind(thread_id)
        .fetch_optional(pool)
        .await?;
    Ok(id.map(|i| i as u64))
}

pub async fn posts_around(
    pool: &MySqlPool,
    thread_id: u32,
    anchor_id: u64,
    older: bool,
    chats: bool,
    take: usize,
) -> Result<Vec<Post>> {
    let (index, filter) = if chats {
        ("n_thread_3char_id", " AND hasThreeCharComment = 1")
    } else {
        ("n_thread_id", "")
    };
    let (cmp, order) = if older { ("<", "DESC") } else { (">", "ASC") };
    let sql = format!(
        "SELECT {POST_COLS} FROM post FORCE INDEX ({index}) WHERE threadId = ?{filter} AND id {cmp} ? ORDER BY id {order} LIMIT ?"
    );
    let rows = sqlx::query(&sql)
        .bind(thread_id)
        .bind(anchor_id as i64)
        .bind(take as i64)
        .fetch_all(pool)
        .await?;
    Ok(rows.iter().map(post_from_row).collect())
}
