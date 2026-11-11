use cgg_decide::model::{BroadcastPost, Counter, Post, PostExtras, ThreadMeta};
use cgg_decide::{Book, Dice, Job, apply, judge};
use serde::Deserialize;
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Default)]
pub struct Engine {
    meta: Option<ThreadMeta>,
    book: Book,
    counters: HashMap<String, Arc<Counter>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PostAsk {
    author: String,
    text: String,
    ns: u64,
    now_ms: f64,
    #[serde(default)]
    random: Vec<f64>,
    hash: Option<Value>,
    macro_preset_id: Option<i64>,
    macro_preset_version_id: Option<i64>,
}

fn counter_from(v: Value) -> Option<Counter> {
    match v {
        Value::Object(fields) => Some(Counter::from_fields(fields)),
        _ => None,
    }
}

fn failure(error: &str) -> Value {
    json!({ "error": error })
}

impl Engine {
    pub fn handle(&mut self, ask: Value) -> Value {
        match ask.get("op").and_then(Value::as_str).unwrap_or("") {
            "load" => self.load(ask),
            "meta" => self.set_meta(ask),
            "counter" => self.add_counter(ask),
            "knows" => json!({ "known": ask["uuid"].as_str().is_some_and(|u| self.counters.contains_key(u)) }),
            "book" => json!({
                "meta": self.meta.as_ref().map(ThreadMeta::to_json),
                "book": &self.book,
                "counters": self.counters.values().map(|c| &c.fields).collect::<Vec<_>>(),
            }),
            "post" => match serde_json::from_value::<PostAsk>(ask) {
                Ok(p) => self.post(p),
                Err(e) => failure(&e.to_string()),
            },
            _ => failure("unknown op"),
        }
    }

    fn load(&mut self, mut ask: Value) -> Value {
        let Some(meta) = ThreadMeta::from_json(&ask["meta"]) else {
            return failure("bad meta");
        };
        let book = match serde_json::from_value::<Book>(ask["book"].take()) {
            Ok(b) => b,
            Err(e) => return failure(&e.to_string()),
        };
        self.meta = Some(meta);
        self.book = book;
        if let Value::Array(counters) = ask["counters"].take() {
            for c in counters.into_iter().filter_map(counter_from) {
                self.counters.insert(c.uuid.clone(), Arc::new(c));
            }
        }
        json!({ "ok": true })
    }

    fn set_meta(&mut self, ask: Value) -> Value {
        match ThreadMeta::from_json(&ask["meta"]) {
            Some(meta) => {
                self.meta = Some(meta);
                json!({ "ok": true })
            }
            None => failure("bad meta"),
        }
    }

    fn add_counter(&mut self, mut ask: Value) -> Value {
        match counter_from(ask["counter"].take()) {
            Some(c) if !c.uuid.is_empty() => {
                self.counters.insert(c.uuid.clone(), Arc::new(c));
                json!({ "ok": true })
            }
            _ => failure("bad counter"),
        }
    }

    fn post(&mut self, ask: PostAsk) -> Value {
        let (Some(meta), Some(counter)) = (self.meta.as_mut(), self.counters.get(&ask.author)) else {
            return failure("not_ready");
        };
        let mut dice_left = ask.random.into_iter();
        let mut random = move || dice_left.next().unwrap_or(0.5);
        let mut dice = Dice {
            now_ms: ask.now_ms,
            server_tz: chrono_tz::America::New_York,
            secret: None,
            random: &mut random,
        };
        let job = Job {
            counter,
            text: &ask.text,
            system: false,
            ns: cgg_decide::ids::ns_after(self.book.last_post.as_ref().map(|p| p.timestamp.as_str()), ask.ns),
            macro_preset_id: ask.macro_preset_id,
            macro_preset_version_id: ask.macro_preset_version_id,
        };
        let decision = match judge(meta, &self.book, &job, &mut dice) {
            Ok(d) => d,
            Err(reason) => return json!({ "rejected": reason }),
        };
        let post: Arc<Post> = Arc::new(decision.post.clone());
        let bans_changed = apply(meta, &mut self.book, &post, &decision.bans);
        let extras = PostExtras {
            post_hash: ask.hash,
            roll: decision.roll,
            chance: decision.chance,
            server_at: ask.now_ms,
            ..Default::default()
        };
        let message = json!({
            "post": BroadcastPost { post: &post, extras: &extras },
            "counter": &counter.json,
            "thread_streak": self.book.streak,
        });
        let update = bans_changed.then(|| json!({ "threadInfo": meta.to_json() }));
        json!({ "decision": decision, "message": message, "threadUpdate": update })
    }
}

thread_local! {
    static ENGINE: RefCell<Engine> = RefCell::new(Engine::default());
    static OUT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

#[unsafe(no_mangle)]
pub extern "C" fn input(len: usize) -> *mut u8 {
    let mut buf = vec![0u8; len];
    let ptr = buf.as_mut_ptr();
    std::mem::forget(buf);
    ptr
}

#[unsafe(no_mangle)]
pub extern "C" fn output() -> *const u8 {
    OUT.with(|o| o.borrow().as_ptr())
}

#[unsafe(no_mangle)]
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub extern "C" fn call(ptr: *mut u8, len: usize) -> usize {
    let bytes = unsafe { Vec::from_raw_parts(ptr, len, len) };
    let reply = match serde_json::from_slice::<Value>(&bytes) {
        Ok(ask) => ENGINE.with(|e| e.borrow_mut().handle(ask)),
        Err(e) => failure(&e.to_string()),
    };
    let encoded = serde_json::to_vec(&reply).unwrap_or_default();
    let n = encoded.len();
    OUT.with(|o| *o.borrow_mut() = encoded);
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(extra: Value) -> Value {
        let mut m = json!({
            "id": 7, "uuid": "t-uuid", "name": "main", "validationType": "main", "firstValidCount": "1",
            "resetOnMistakes": false, "allowDoublePosts": false, "autoValidated": true, "locked": false,
            "visibleTo": ["all"], "updatableBy": ["counter"], "countBans": [], "postBans": []
        });
        for (k, v) in extra.as_object().unwrap() {
            m[k] = v.clone();
        }
        m
    }

    fn engine(extra: Value) -> Engine {
        let mut e = Engine::default();
        assert_eq!(
            e.handle(json!({ "op": "load", "meta": meta(extra), "book": {} }))["ok"],
            true
        );
        for (uuid, roles) in [
            ("a", json!(["counter"])),
            ("b", json!(["counter"])),
            ("m", json!(["counter", "muted"])),
        ] {
            let c = json!({ "uuid": uuid, "username": uuid, "id": 1, "roles": roles });
            assert_eq!(e.handle(json!({ "op": "counter", "counter": c }))["ok"], true);
        }
        e
    }

    fn post(e: &mut Engine, author: &str, text: &str, ns: u64) -> Value {
        e.handle(json!({ "op": "post", "author": author, "text": text, "ns": ns, "nowMs": ns as f64 / 1e6, "random": [0.1] }))
    }

    #[test]
    fn counts_follow_the_rules_and_the_book_carries_them() {
        let mut e = engine(json!({}));
        let first = post(&mut e, "a", "1", 1_000_000_000);
        assert_eq!(first["decision"]["post"]["isValidCount"], true);
        assert_eq!(first["message"]["post"]["validCountNumber"], 1);
        assert_eq!(first["message"]["counter"]["uuid"], "a");
        assert_eq!(
            post(&mut e, "a", "2", 2_000_000_000)["decision"]["post"]["isValidCount"],
            false
        );
        let second = post(&mut e, "b", "2", 3_000_000_000);
        assert_eq!(second["decision"]["post"]["isValidCount"], true);
        assert_eq!(second["message"]["thread_streak"], 1);
        let saved = e.handle(json!({ "op": "book" }));
        assert_eq!(saved["book"]["lastValid"]["rawCount"], "2");
        let mut woken = Engine::default();
        let mut reload = saved.clone();
        reload["op"] = json!("load");
        assert_eq!(woken.handle(reload)["ok"], true);
        assert_eq!(
            post(&mut woken, "a", "3", 4_000_000_000)["decision"]["post"]["isValidCount"],
            true
        );
    }

    #[test]
    fn locks_bans_and_mutes_apply_in_the_region() {
        let mut e = engine(json!({ "postBans": ["b"] }));
        assert_eq!(post(&mut e, "b", "1", 1_000)["rejected"], "banned");
        let muted = post(&mut e, "m", "1 hello", 2_000);
        assert_eq!(muted["decision"]["post"]["isCommentDeleted"], true);
        assert_eq!(
            e.handle(json!({ "op": "meta", "meta": meta(json!({ "locked": true })) }))["ok"],
            true
        );
        assert_eq!(post(&mut e, "a", "2", 3_000)["rejected"], "locked");
    }

    #[test]
    fn an_unknown_author_is_not_ready_rather_than_judged() {
        let mut e = engine(json!({}));
        assert_eq!(post(&mut e, "nobody", "1", 1_000)["error"], "not_ready");
        assert_eq!(e.handle(json!({ "op": "knows", "uuid": "a" }))["known"], true);
    }

    #[test]
    fn yoco_bans_after_a_valid_count_and_says_so() {
        let mut e = engine(json!({ "name": "yoco", "validationType": "yoco" }));
        let r = post(&mut e, "a", "1", 1_000);
        assert_eq!(r["decision"]["bans"][0]["uuid"], "a");
        assert_eq!(r["threadUpdate"]["threadInfo"]["postBans"][0], "a");
        assert_eq!(post(&mut e, "a", "2", 2_000)["rejected"], "banned");
    }
}
