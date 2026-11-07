use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Post {
    pub uuid: String,
    pub timestamp: String,
    pub time_since_last_count: String,
    pub time_since_last_post: String,
    pub raw_text: String,
    pub is_count: bool,
    pub is_valid_count: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub valid_count_number: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub count_content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub raw_count: Option<String>,
    pub stricken: bool,
    pub thread: String,
    pub thread_id: u32,
    pub has_comment: bool,
    pub has_three_char_comment: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub comment: Option<String>,
    #[serde(rename = "authorUUID")]
    pub author_uuid: String,
    pub author_id: Option<u32>,
    pub is_deleted: bool,
    pub is_comment_deleted: bool,
    pub reactions: serde_json::Value,
    pub macro_preset_id: Option<i64>,
    pub macro_preset_version_id: Option<i64>,
    pub id: u64,
}

impl Post {
    pub fn json_with_nulls(&self) -> serde_json::Value {
        let mut v = serde_json::to_value(self).unwrap_or(serde_json::Value::Null);
        if let Some(o) = v.as_object_mut() {
            let mut out = serde_json::Map::new();
            for k in [
                "uuid",
                "timestamp",
                "timeSinceLastCount",
                "timeSinceLastPost",
                "rawText",
                "isCount",
                "isValidCount",
                "validCountNumber",
                "countContent",
                "rawCount",
                "stricken",
                "thread",
                "threadId",
                "hasComment",
                "hasThreeCharComment",
                "comment",
                "authorUUID",
                "authorId",
                "isDeleted",
                "isCommentDeleted",
                "reactions",
                "macroPresetId",
                "macroPresetVersionId",
                "id",
            ] {
                out.insert(k.into(), o.remove(k).unwrap_or(serde_json::Value::Null));
            }
            return serde_json::Value::Object(out);
        }
        v
    }

    pub fn as_prev(&self, author_key: u64) -> counting::PrevCount {
        counting::PrevCount {
            raw_count: self.raw_count.clone(),
            author: author_key,
            valid_count_number: self.valid_count_number,
            timestamp: counting::js::parse_float(&self.timestamp),
            time_since_last_count: counting::js::parse_float(&self.time_since_last_count),
        }
    }
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PostExtras {
    #[serde(rename = "post_hash", skip_serializing_if = "Option::is_none")]
    pub post_hash: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chance: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub roll: Option<f64>,
    pub scheduling_ms: f64,
    pub db_read_ms: f64,
    pub db_write_ms: f64,
    pub pre_broadcast_ms: f64,
    pub processing_latency: f64,
    pub server_at: f64,
    pub other_ms: f64,
}

#[derive(Serialize)]
pub struct BroadcastPost<'a> {
    #[serde(flatten)]
    pub post: &'a Post,
    #[serde(flatten)]
    pub extras: &'a PostExtras,
}

#[derive(Debug)]
pub struct Counter {
    pub id: Option<u32>,
    pub uuid: String,
    pub username: String,
    pub roles: Vec<String>,
    pub fields: serde_json::Map<String, serde_json::Value>,
    pub json: Box<RawValue>,
}

impl Counter {
    pub fn from_fields(fields: serde_json::Map<String, serde_json::Value>) -> Counter {
        let s = |k: &str| fields.get(k).and_then(|v| v.as_str()).unwrap_or("").to_owned();
        let roles = fields
            .get("roles")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|r| r.as_str().map(str::to_owned)).collect())
            .unwrap_or_default();
        let id = fields
            .get("id")
            .and_then(|v| v.as_i64())
            .filter(|i| *i >= 0)
            .map(|i| i as u32);
        let json = serde_json::value::to_raw_value(&fields).expect("json");
        Counter {
            id,
            uuid: s("uuid"),
            username: s("username"),
            roles,
            fields,
            json,
        }
    }

    pub fn has_role(&self, r: &str) -> bool {
        self.roles.iter().any(|x| x == r)
    }

    pub fn xp(&self) -> f64 {
        match self.fields.get("xp") {
            Some(serde_json::Value::String(s)) => s.parse().unwrap_or(0.0),
            Some(v) => v.as_f64().unwrap_or(0.0),
            None => 0.0,
        }
    }

    pub fn with(&self, key: &str, v: serde_json::Value) -> Counter {
        let mut f = self.fields.clone();
        f.insert(key.into(), v);
        Counter::from_fields(f)
    }

    pub fn pronoun(&self, i: usize) -> String {
        self.fields
            .get("pronouns")
            .and_then(|p| p.get(i))
            .and_then(|v| v.as_str())
            .unwrap_or("their")
            .to_owned()
    }

    pub fn system() -> Counter {
        let v = serde_json::json!({
            "id": -1, "uuid": "[SYSTEM]", "name": "[SYSTEM]", "username": "[SYSTEM]", "roles": ["bot"],
            "title": "SYSTEM", "discordId": "0", "pronouns": ["IT", "IT", "IT", "IT"], "avatar": "0",
            "cardStyle": "card_default", "cardBorderStyle": "no_border_circle", "xp": 0, "xp2": 0,
            "rainbow": 1, "color": "#069420", "emoji": "\u{1F916}", "blogs": [], "blogLikes": []
        });
        let serde_json::Value::Object(m) = v else {
            unreachable!()
        };
        Counter::from_fields(m)
    }
}

#[derive(Debug, Clone)]
pub struct ThreadMeta {
    pub id: u32,
    pub uuid: String,
    pub name: String,
    pub validation_type: String,
    pub first_valid_count: String,
    pub reset_on_mistakes: bool,
    pub allow_double_posts: bool,
    pub auto_validated: bool,
    pub locked: bool,
    pub visible_to: Vec<String>,
    pub updatable_by: Vec<String>,
    pub count_bans: Vec<String>,
    pub post_bans: Vec<String>,
    pub row: serde_json::Map<String, serde_json::Value>,
}

impl ThreadMeta {
    pub fn visible_to_counter(&self, c: Option<&Counter>) -> bool {
        match c {
            Some(c) if c.has_role("admin") => true,
            Some(c) => self.visible_to.iter().any(|r| r == "all" || c.has_role(r)),
            None => self.visible_to.iter().any(|r| r == "all"),
        }
    }

    pub fn may_post(&self, c: &Counter) -> bool {
        let privileged = c.has_role("admin") || c.has_role("bot");
        let updatable = privileged || self.updatable_by.iter().any(|r| c.has_role(r));
        let visible = privileged || self.visible_to.iter().any(|r| r == "all" || c.has_role(r));
        updatable && visible && !c.has_role("banned") && !self.locked
    }

    pub fn to_json(&self) -> serde_json::Value {
        let mut m = self.row.clone();
        m.insert("countBans".into(), self.count_bans.clone().into());
        m.insert("postBans".into(), self.post_bans.clone().into());
        m.insert("locked".into(), self.locked.into());
        serde_json::Value::Object(m)
    }
}

pub type CounterRef = Arc<Counter>;

impl ThreadMeta {
    pub fn from_json(v: &serde_json::Value) -> Option<ThreadMeta> {
        let o = v.as_object()?;
        let s = |k: &str| o.get(k).and_then(|x| x.as_str()).unwrap_or("").to_owned();
        let b = |k: &str| match o.get(k) {
            Some(serde_json::Value::Bool(x)) => *x,
            Some(serde_json::Value::Number(n)) => n.as_i64() != Some(0),
            _ => false,
        };
        let list = |k: &str| -> Vec<String> {
            o.get(k)
                .and_then(|x| x.as_array())
                .map(|a| a.iter().filter_map(|i| i.as_str().map(str::to_owned)).collect())
                .unwrap_or_default()
        };
        Some(ThreadMeta {
            id: o.get("id").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
            uuid: s("uuid"),
            name: s("name"),
            validation_type: s("validationType"),
            first_valid_count: s("firstValidCount"),
            reset_on_mistakes: b("resetOnMistakes"),
            allow_double_posts: b("allowDoublePosts"),
            auto_validated: b("autoValidated"),
            locked: b("locked"),
            visible_to: list("visibleTo"),
            updatable_by: list("updatableBy"),
            count_bans: list("countBans"),
            post_bans: list("postBans"),
            row: o.clone(),
        })
    }
}
