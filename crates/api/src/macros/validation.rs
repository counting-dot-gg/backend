use crate::error::{ApiError, ApiResult};
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};

const MAX_TRIGGER_LENGTH: usize = 64;
const DEFAULT_MAX_ENTRIES: usize = 100;
const DEFAULT_REPEAT_CAP: f64 = 256.0;
const MAX_TRIGGERS_PER_CHAR_INSERT: usize = 2;
const MAX_SUBMIT_MACROS: usize = 10;
const MAX_PAD_ROWS: i64 = 9;
const MAX_PAD_COLS: i64 = 8;
const MAX_LABEL_LENGTH: usize = 8;

pub const ENTRY_TYPES: [&str; 6] = ["CHAR_INSERT", "ACTION", "SUBMIT", "SUBMIT_ACTION", "TOGGLE", "COMBO"];
const ACTIONS: [&str; 12] = [
    "BACKSPACE",
    "DELETE",
    "CTRL_BACKSPACE",
    "LEFT",
    "RIGHT",
    "UP",
    "DOWN",
    "HOME",
    "END",
    "SELECT_ALL",
    "COPY",
    "PASTE",
];
const COMBOS: [&str; 2] = ["SELECT_ALL_COPY", "SELECT_ALL_PASTE"];
const NAMED_KEYS: [&str; 25] = [
    "backspace",
    "tab",
    "enter",
    "shift",
    "control",
    "alt",
    "pause",
    "capslock",
    "escape",
    "space",
    "pageup",
    "pagedown",
    "end",
    "home",
    "arrowleft",
    "arrowup",
    "arrowright",
    "arrowdown",
    "printscreen",
    "insert",
    "delete",
    "meta",
    "contextmenu",
    "numlock",
    "scrolllock",
];
const FIXED_KINDS: [&str; 5] = ["backspace", "enter", "space", "clear", "letters"];

fn bad(msg: impl Into<String>) -> ApiError {
    ApiError::bad_request(msg.into())
}

fn chars(s: &str) -> usize {
    s.chars().count()
}

pub fn normalize_trigger(key: &str) -> String {
    let raw = key.to_lowercase();
    if raw == " " || raw == "spacebar" {
        return "space".into();
    }
    let n = counting::js::trim(&raw).to_owned();
    let alias = match n.as_str() {
        "win" | "windows" | "cmd" | "command" | "os" => "meta",
        "ctrl" => "control",
        "option" => "alt",
        "esc" => "escape",
        "return" => "enter",
        "up" => "arrowup",
        "down" => "arrowdown",
        "left" => "arrowleft",
        "right" => "arrowright",
        _ => return n,
    };
    alias.into()
}

fn one_of(s: &str, options: &[&str]) -> Option<String> {
    options
        .iter()
        .find(|o| s.starts_with(**o))
        .map(|o| s[o.len()..].to_owned())
}

fn code_style(k: &str) -> bool {
    let one = |prefix: &str, f: fn(&u8) -> bool| {
        k.strip_prefix(prefix)
            .is_some_and(|s| s.len() == 1 && f(&s.as_bytes()[0]))
    };
    one("key", u8::is_ascii_lowercase)
        || one("digit", u8::is_ascii_digit)
        || one("numpad", u8::is_ascii_digit)
        || k.strip_prefix('f').is_some_and(|s| {
            !s.is_empty()
                && s.len() <= 2
                && !s.starts_with('0')
                && s.parse::<u32>().is_ok_and(|n| (1..=24).contains(&n))
                && s.bytes().all(|b| b.is_ascii_digit())
        })
        || one_of(k, &["shift", "control", "alt", "meta"]).is_some_and(|t| t == "left" || t == "right")
        || [
            "backquote",
            "minus",
            "equal",
            "bracketleft",
            "bracketright",
            "backslash",
            "semicolon",
            "quote",
            "comma",
            "period",
            "slash",
        ]
        .contains(&k)
        || k.strip_prefix("numpad").is_some_and(|t| {
            [
                "add",
                "subtract",
                "multiply",
                "divide",
                "decimal",
                "enter",
                "equal",
                "comma",
                "parenleft",
                "parenright",
            ]
            .contains(&t)
        })
        || k.strip_prefix("intl")
            .is_some_and(|t| ["backslash", "ro", "yen"].contains(&t))
}

pub fn pad_only_trigger(k: &str) -> bool {
    k.strip_prefix("pad:").is_some_and(|d| {
        let b = d.as_bytes();
        (b.len() == 1 || b.len() == 2) && (b'1'..=b'9').contains(&b[0]) && b.iter().all(u8::is_ascii_digit)
    })
}

fn valid_trigger(k: &str) -> bool {
    if k.is_empty() || k.encode_utf16().count() > MAX_TRIGGER_LENGTH {
        return false;
    }
    chars(k) == 1 || NAMED_KEYS.contains(&k) || pad_only_trigger(k) || code_style(k)
}

fn is_int(v: &Value) -> bool {
    v.as_f64().is_some_and(|f| f.is_finite() && f.fract() == 0.0)
}

fn has(p: &Map<String, Value>, k: &str) -> bool {
    p.get(k).is_some()
}

pub fn validate_entries(entries: &Value) -> ApiResult<()> {
    let Some(entries) = entries.as_array() else {
        return Err(bad("entries must be an array"));
    };
    if entries.len() > DEFAULT_MAX_ENTRIES {
        return Err(bad(format!(
            "Too many macro entries. Max allowed is {DEFAULT_MAX_ENTRIES}."
        )));
    }
    let mut triggers: HashSet<String> = HashSet::new();
    let mut char_counts: HashMap<String, usize> = HashMap::new();
    let mut submits = 0;
    for (i, entry) in entries.iter().enumerate() {
        let prefix = format!("entries[{i}]");
        let Some(e) = entry.as_object() else {
            return Err(bad(format!("{prefix} must be an object")));
        };
        let trigger = match e.get("triggerKey").and_then(Value::as_str) {
            Some(t) if !counting::js::trim(t).is_empty() => t,
            _ => return Err(bad(format!("{prefix}.triggerKey must be a non-empty string"))),
        };
        if trigger.encode_utf16().count() > MAX_TRIGGER_LENGTH {
            return Err(bad(format!(
                "{prefix}.triggerKey exceeds max length {MAX_TRIGGER_LENGTH}"
            )));
        }
        let norm = normalize_trigger(trigger);
        if !valid_trigger(&norm) {
            return Err(bad(format!("{prefix}.triggerKey is not a supported key/code")));
        }
        if !triggers.insert(norm) {
            return Err(bad(format!("Duplicate triggerKey \"{trigger}\" in macro payload")));
        }
        let ty = e.get("macroType").and_then(Value::as_str).unwrap_or("");
        if !ENTRY_TYPES.contains(&ty) {
            return Err(bad(format!("{prefix}.macroType is invalid")));
        }
        let Some(p) = e.get("payloadJson").and_then(Value::as_object) else {
            return Err(bad(format!("{prefix}.payloadJson must be an object")));
        };
        let action_ok = |p: &Map<String, Value>| -> ApiResult<()> {
            if !p
                .get("action")
                .and_then(Value::as_str)
                .is_some_and(|a| ACTIONS.contains(&a))
            {
                return Err(bad(format!("{prefix}.payloadJson.action is not allowlisted")));
            }
            match p.get("repeat") {
                None | Some(Value::Null) => Ok(()),
                Some(r) if !is_int(r) || r.as_f64().unwrap_or(0.0) < 1.0 => {
                    Err(bad(format!("{prefix}.payloadJson.repeat must be an integer >= 1")))
                }
                Some(r) if r.as_f64().unwrap_or(0.0) > DEFAULT_REPEAT_CAP => Err(bad(format!(
                    "{prefix}.payloadJson.repeat exceeds max allowed repeat {DEFAULT_REPEAT_CAP}"
                ))),
                Some(_) => Ok(()),
            }
        };
        match ty {
            "CHAR_INSERT" => {
                let c = match p.get("char").and_then(Value::as_str) {
                    Some(c) if chars(c) == 1 => c.to_owned(),
                    _ => return Err(bad(format!("{prefix}.payloadJson.char must be exactly one character"))),
                };
                let n = char_counts.entry(c.clone()).or_default();
                if *n >= MAX_TRIGGERS_PER_CHAR_INSERT {
                    return Err(bad(format!(
                        "At most {MAX_TRIGGERS_PER_CHAR_INSERT} trigger keys may map to \"{c}\""
                    )));
                }
                *n += 1;
            }
            "ACTION" => action_ok(p)?,
            "SUBMIT" | "TOGGLE" => {
                if has(p, "action") || has(p, "repeat") || has(p, "submit") {
                    return Err(bad(format!("{prefix}.payloadJson must be empty for {ty}")));
                }
                if ty == "SUBMIT" {
                    submits += 1;
                }
            }
            "SUBMIT_ACTION" => {
                action_ok(p)?;
                submits += 1;
            }
            _ => {
                if !p
                    .get("comboId")
                    .and_then(Value::as_str)
                    .is_some_and(|c| COMBOS.contains(&c))
                {
                    return Err(bad(format!("{prefix}.payloadJson.comboId is not allowlisted")));
                }
            }
        }
    }
    if submits > MAX_SUBMIT_MACROS {
        return Err(bad(format!(
            "At most {MAX_SUBMIT_MACROS} submit-related macros are allowed per version"
        )));
    }
    Ok(())
}

fn int_in(v: Option<&Value>, min: i64, max: i64) -> Option<i64> {
    let v = v?;
    if !is_int(v) {
        return None;
    }
    let n = v.as_f64()? as i64;
    (min..=max).contains(&n).then_some(n)
}

fn label_of(prefix: &str, v: Option<&Value>, required: bool) -> ApiResult<Option<String>> {
    match v {
        None | Some(Value::Null) => {
            if required {
                return Err(bad(format!("{prefix}.label is required")));
            }
            Ok(None)
        }
        Some(Value::String(s)) if s.is_empty() => {
            if required {
                return Err(bad(format!("{prefix}.label is required")));
            }
            Ok(None)
        }
        Some(Value::String(s)) => {
            let label = counting::js::trim(s).to_owned();
            if label.is_empty() && required {
                return Err(bad(format!("{prefix}.label is required")));
            }
            if chars(&label) > MAX_LABEL_LENGTH {
                return Err(bad(format!("{prefix}.label exceeds {MAX_LABEL_LENGTH} characters")));
            }
            Ok((!label.is_empty()).then_some(label))
        }
        Some(_) => Err(bad(format!("{prefix}.label must be a string"))),
    }
}

pub fn normalize_layout(input: &Value, trigger_keys: &[String]) -> ApiResult<Value> {
    if input.is_null() {
        return Ok(Value::Null);
    }
    let Some(raw) = input.as_object() else {
        return Err(bad("keyboardLayout must be an object"));
    };
    let (Some(rows), Some(cols)) = (
        int_in(raw.get("rows"), 1, MAX_PAD_ROWS),
        int_in(raw.get("cols"), 1, MAX_PAD_COLS),
    ) else {
        return Err(bad(format!(
            "keyboardLayout needs 1-{MAX_PAD_ROWS} rows and 1-{MAX_PAD_COLS} columns"
        )));
    };
    let Some(cells_in) = raw.get("cells").and_then(Value::as_array) else {
        return Err(bad("keyboardLayout.cells must be an array"));
    };
    if cells_in.len() as i64 > rows * cols {
        return Err(bad("keyboardLayout has more keys than the grid has cells"));
    }
    let triggers: HashSet<String> = trigger_keys.iter().map(|t| normalize_trigger(t)).collect();
    let mut occupied: HashSet<(i64, i64)> = HashSet::new();
    let mut cells = Vec::new();
    for (i, c) in cells_in.iter().enumerate() {
        let prefix = format!("keyboardLayout.cells[{i}]");
        let Some(cell) = c.as_object() else {
            return Err(bad(format!("{prefix} must be an object")));
        };
        let row = int_in(cell.get("row"), 0, rows - 1);
        let col = int_in(cell.get("col"), 0, cols - 1);
        let w = int_in(cell.get("w"), 1, cols);
        let h = int_in(cell.get("h"), 1, rows);
        let (Some(row), Some(col), Some(w), Some(h)) = (row, col, w, h) else {
            return Err(bad(format!("{prefix} does not fit inside the grid")));
        };
        if col + w > cols || row + h > rows {
            return Err(bad(format!("{prefix} does not fit inside the grid")));
        }
        for r in row..row + h {
            for k in col..col + w {
                if !occupied.insert((r, k)) {
                    return Err(bad(format!("{prefix} overlaps another key")));
                }
            }
        }
        let Some(key) = cell.get("key").and_then(Value::as_object) else {
            return Err(bad(format!("{prefix}.key must be an object")));
        };
        let kp = format!("{prefix}.key");
        let kind = key.get("kind").and_then(Value::as_str);
        let mut out = Map::new();
        match kind {
            Some("char") => {
                let value = match key.get("value").and_then(Value::as_str) {
                    Some(v) if chars(v) == 1 => v.to_owned(),
                    _ => return Err(bad(format!("{prefix}.key.value must be exactly one character"))),
                };
                out.insert("kind".into(), json!("char"));
                out.insert("value".into(), json!(value));
                if let Some(l) = label_of(&kp, key.get("label"), false)? {
                    out.insert("label".into(), json!(l));
                }
            }
            Some(k) if FIXED_KINDS.contains(&k) => {
                out.insert("kind".into(), json!(k));
                if let Some(l) = label_of(&kp, key.get("label"), false)? {
                    out.insert("label".into(), json!(l));
                }
            }
            Some("macro") => {
                let t = match key.get("triggerKey").and_then(Value::as_str) {
                    Some(t) if triggers.contains(&normalize_trigger(t)) => t.to_owned(),
                    _ => {
                        return Err(bad(format!(
                            "{prefix}.key must name one of this preset's macro triggers"
                        )));
                    }
                };
                let label = label_of(&kp, key.get("label"), true)?;
                out.insert("kind".into(), json!("macro"));
                out.insert("triggerKey".into(), json!(t));
                out.insert("label".into(), json!(label));
            }
            _ => return Err(bad(format!("{prefix}.key.kind is not allowlisted"))),
        }
        cells.push(json!({ "row": row, "col": col, "w": w, "h": h, "key": out }));
    }
    cells.sort_by_key(|c| (c["row"].as_i64(), c["col"].as_i64()));
    Ok(json!({ "rows": rows, "cols": cols, "cells": cells }))
}

pub fn validate_version(entries: &Value, layout: &Value) -> ApiResult<Value> {
    validate_entries(entries)?;
    let triggers: Vec<String> = entries
        .as_array()
        .map(|a| {
            a.iter()
                .map(|e| e["triggerKey"].as_str().unwrap_or("").to_owned())
                .collect()
        })
        .unwrap_or_default();
    let layout = normalize_layout(layout, &triggers)?;
    if triggers.is_empty() && layout.is_null() {
        return Err(bad("A preset needs at least one macro mapping or a custom keyboard"));
    }
    Ok(layout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triggers_normalize_like_the_ts_aliases() {
        assert_eq!(normalize_trigger("Ctrl"), "control");
        assert_eq!(normalize_trigger(" "), "space");
        assert!(valid_trigger("keya"));
        assert!(valid_trigger("f12"));
        assert!(!valid_trigger("f25"));
        assert!(valid_trigger("pad:12"));
        assert!(!valid_trigger("pad:0"));
        assert!(valid_trigger("numpadadd"));
        assert!(valid_trigger("é"));
    }

    #[test]
    fn duplicate_triggers_and_too_many_char_maps_are_rejected() {
        let dup = json!([
            { "triggerKey": "a", "macroType": "CHAR_INSERT", "payloadJson": { "char": "1" } },
            { "triggerKey": "A", "macroType": "CHAR_INSERT", "payloadJson": { "char": "2" } }
        ]);
        assert!(validate_entries(&dup).is_err());
        let three = json!([
            { "triggerKey": "a", "macroType": "CHAR_INSERT", "payloadJson": { "char": "1" } },
            { "triggerKey": "b", "macroType": "CHAR_INSERT", "payloadJson": { "char": "1" } },
            { "triggerKey": "c", "macroType": "CHAR_INSERT", "payloadJson": { "char": "1" } }
        ]);
        assert!(validate_entries(&three).is_err());
    }

    #[test]
    fn layouts_sort_cells_and_reject_overlaps() {
        let l = json!({ "rows": 2, "cols": 2, "cells": [
            { "row": 1, "col": 0, "w": 1, "h": 1, "key": { "kind": "enter" } },
            { "row": 0, "col": 0, "w": 2, "h": 1, "key": { "kind": "char", "value": "1", "label": "" } }
        ]});
        let n = normalize_layout(&l, &[]).unwrap();
        assert_eq!(n["cells"][0]["row"], 0);
        assert!(n["cells"][0]["key"].get("label").is_none());
        let overlap = json!({ "rows": 1, "cols": 2, "cells": [
            { "row": 0, "col": 0, "w": 2, "h": 1, "key": { "kind": "enter" } },
            { "row": 0, "col": 1, "w": 1, "h": 1, "key": { "kind": "enter" } }
        ]});
        assert!(normalize_layout(&overlap, &[]).is_err());
    }
}
