mod defs;

pub use defs::*;

use chrono::{Local, NaiveDateTime, TimeZone, Utc};
use rust_decimal::Decimal;
use serde_json::{Map, Value};
use sqlx::mysql::MySqlRow;
use sqlx::{Column as _, Row, TypeInfo, ValueRef};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Str,
    Num,
    BigStr,
    DecStr,
    Bool,
    Json,
    Date,
}

pub struct Column {
    pub prop: &'static str,
    pub column: &'static str,
    pub kind: Kind,
    pub hidden: bool,
}

pub struct Entity {
    pub columns: &'static [Column],
}

impl Entity {
    pub fn json(&self, row: &MySqlRow) -> Value {
        self.json_prefixed(row, "")
    }

    pub fn json_prefixed(&self, row: &MySqlRow, prefix: &str) -> Value {
        let mut out = Map::new();
        for c in self.columns.iter().filter(|c| !c.hidden) {
            let name = format!("{prefix}{}", c.column);
            if row.columns().iter().any(|rc| rc.name() == name) {
                out.insert(c.prop.to_owned(), cell(row, &name, c.kind));
            }
        }
        Value::Object(out)
    }

    pub fn select(&self, alias: &str) -> String {
        self.columns
            .iter()
            .filter(|c| !c.hidden)
            .map(|c| format!("{alias}.`{0}` AS `{alias}_{0}`", c.column))
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub fn rows(&self, rows: &[MySqlRow]) -> Value {
        Value::Array(rows.iter().map(|r| self.json(r)).collect())
    }
}

pub fn cell(row: &MySqlRow, name: &str, kind: Kind) -> Value {
    let Ok(raw) = row.try_get_raw(name) else {
        return Value::Null;
    };
    if raw.is_null() {
        return Value::Null;
    }
    let ty = raw.type_info().name().to_owned();
    match kind {
        Kind::Bool => int(row, name).map_or(Value::Null, |n| Value::Bool(n != 0)),
        Kind::BigStr => match ty.as_str() {
            "DECIMAL" => decimal(row, name),
            _ => int(row, name).map_or_else(|| text_value(row, name), |n| Value::String(n.to_string())),
        },
        Kind::DecStr => decimal(row, name),
        Kind::Json => json_cell(row, name),
        Kind::Date => date(row, name),
        Kind::Num | Kind::Str => natural(row, name, &ty),
    }
}

pub fn natural(row: &MySqlRow, name: &str, ty: &str) -> Value {
    match ty {
        "BOOLEAN" | "TINYINT" | "SMALLINT" | "MEDIUMINT" | "INT" | "TINYINT UNSIGNED" | "SMALLINT UNSIGNED"
        | "MEDIUMINT UNSIGNED" | "INT UNSIGNED" | "YEAR" => int(row, name).map_or(Value::Null, Value::from),
        "BIGINT" | "BIGINT UNSIGNED" => int(row, name).map_or(Value::Null, |n| Value::String(n.to_string())),
        "DECIMAL" => decimal(row, name),
        "FLOAT" | "DOUBLE" => row
            .try_get::<f64, _>(name)
            .ok()
            .or_else(|| row.try_get::<f32, _>(name).ok().map(f64::from))
            .map_or(Value::Null, js_number),
        "JSON" => json_cell(row, name),
        "DATETIME" | "TIMESTAMP" | "DATE" => date(row, name),
        _ => text_value(row, name),
    }
}

pub fn js_number(f: f64) -> Value {
    if f.fract() == 0.0 && f.abs() < 9.007_199_254_740_992e15 {
        Value::from(f as i64)
    } else {
        serde_json::Number::from_f64(f).map_or(Value::Null, Value::Number)
    }
}

fn int(row: &MySqlRow, name: &str) -> Option<i64> {
    row.try_get::<i64, _>(name)
        .ok()
        .or_else(|| row.try_get::<u64, _>(name).ok().map(|n| n as i64))
        .or_else(|| row.try_get::<i32, _>(name).ok().map(i64::from))
        .or_else(|| row.try_get::<u32, _>(name).ok().map(i64::from))
        .or_else(|| row.try_get::<i16, _>(name).ok().map(i64::from))
        .or_else(|| row.try_get::<u16, _>(name).ok().map(i64::from))
        .or_else(|| row.try_get::<i8, _>(name).ok().map(i64::from))
        .or_else(|| row.try_get::<u8, _>(name).ok().map(i64::from))
        .or_else(|| row.try_get::<bool, _>(name).ok().map(i64::from))
}

fn decimal(row: &MySqlRow, name: &str) -> Value {
    row.try_get::<Decimal, _>(name)
        .map(|d| Value::String(d.to_string()))
        .unwrap_or_else(|_| text_value(row, name))
}

fn text_value(row: &MySqlRow, name: &str) -> Value {
    row.try_get::<String, _>(name)
        .map(Value::String)
        .or_else(|_| {
            row.try_get::<Vec<u8>, _>(name)
                .map(|b| Value::String(String::from_utf8_lossy(&b).into_owned()))
        })
        .unwrap_or(Value::Null)
}

fn json_cell(row: &MySqlRow, name: &str) -> Value {
    if let Ok(v) = row.try_get::<sqlx::types::Json<Value>, _>(name) {
        return v.0;
    }
    match text_value(row, name) {
        Value::String(s) => serde_json::from_str(&s).unwrap_or(Value::String(s)),
        other => other,
    }
}

fn date(row: &MySqlRow, name: &str) -> Value {
    let naive = row.try_get::<NaiveDateTime, _>(name).ok().or_else(|| {
        row.try_get::<chrono::DateTime<Utc>, _>(name)
            .ok()
            .map(|d| d.naive_utc())
    });
    naive.map_or(Value::Null, |n| Value::String(iso_local(n)))
}

pub fn iso_local(n: NaiveDateTime) -> String {
    let utc = Local
        .from_local_datetime(&n)
        .earliest()
        .map_or_else(|| Utc.from_utc_datetime(&n), |l| l.with_timezone(&Utc));
    js_iso(utc)
}

pub fn js_iso(t: chrono::DateTime<Utc>) -> String {
    t.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

pub fn row_json(row: &MySqlRow) -> Value {
    let mut out = Map::new();
    for c in row.columns() {
        let name = c.name();
        let ty = c.type_info().name().to_owned();
        let v = match row.try_get_raw(name) {
            Ok(r) if r.is_null() => Value::Null,
            Ok(_) => natural(row, name, &ty),
            Err(_) => Value::Null,
        };
        out.insert(name.to_owned(), v);
    }
    Value::Object(out)
}
