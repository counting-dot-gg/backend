use crate::app::{App, now_ms};
use crate::daily::eastern_day_key;
use serde_json::{Value, json};
use socketioxide::extract::SocketRef;

#[derive(Default)]
pub struct Misc {
    pub rock: Value,
    pub sky: Value,
    loaded: bool,
}

fn today() -> String {
    eastern_day_key(now_ms())
}

fn roll_daily(v: &mut Value) -> i64 {
    if !v.is_object() {
        return 0;
    }
    let t = today();
    if v["day"].as_str() != Some(t.as_str()) {
        v["day"] = json!(t);
        v["count"] = json!(0);
    }
    v["count"].as_i64().unwrap_or(0)
}

async fn load_misc(app: &App, name: &str, default: Value) -> Value {
    let v: Option<String> = sqlx::query_scalar("SELECT CAST(val AS CHAR) FROM misc WHERE name = ?")
        .bind(name)
        .fetch_optional(&app.pool)
        .await
        .ok()
        .flatten();
    match v.and_then(|s| serde_json::from_str(&s).ok()) {
        Some(v) => v,
        None => {
            let _ = sqlx::query("INSERT IGNORE INTO misc (name, val) VALUES (?, ?)")
                .bind(name)
                .bind(default.to_string())
                .execute(&app.pool)
                .await;
            default
        }
    }
}

pub async fn init(app: std::sync::Arc<App>) {
    let rock = load_misc(&app, "rock", json!({ "count": 0 })).await;
    let sky = load_misc(&app, "sky_counter", json!({ "count": 0, "day": today() })).await;
    {
        let mut t = app.misc.lock();
        t.rock = rock;
        t.sky = sky;
        t.loaded = true;
    }
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(60));
        tick.tick().await;
        loop {
            tick.tick().await;
            flush(&app).await;
        }
    });
}

pub async fn flush(app: &App) {
    let (rock, sky) = {
        let t = app.misc.lock();
        if !t.loaded {
            return;
        }
        (t.rock.to_string(), t.sky.to_string())
    };
    for (name, val) in [("rock", rock), ("sky_counter", sky)] {
        if let Err(e) = sqlx::query("UPDATE misc SET val = ? WHERE name = ?")
            .bind(val)
            .bind(name)
            .execute(&app.pool)
            .await
        {
            tracing::warn!("misc flush {name}: {e}");
        }
    }
}

pub async fn watch_rock(app: &App, s: &SocketRef) {
    s.join("the_rock");
    let n = app.io.within("the_rock").sockets().len();
    app.to("the_rock", "watcher_count", &n).await;
    let count = app.misc.lock().rock["count"].clone();
    if !count.is_null() {
        app.to("the_rock", "rock_count", &count).await;
    }
}

pub async fn rock(app: &App) {
    let count = {
        let mut t = app.misc.lock();
        match t.rock["count"].as_i64() {
            Some(c) => {
                t.rock["count"] = json!(c + 1);
                c + 1
            }
            None => return,
        }
    };
    app.to("the_rock", "rock_count", &count).await;
}

pub async fn watch_sky(app: &App, s: &SocketRef) {
    s.join("sky_counter");
    let n = roll_daily(&mut app.misc.lock().sky);
    s.emit("sky_count", &n).ok();
}

pub async fn sky_click(app: &App, data: &Value) {
    let dir = match data["dir"].as_f64() {
        Some(-1.0) => -1,
        Some(1.0) => 1,
        _ => return,
    };
    let total = {
        let mut t = app.misc.lock();
        if !t.sky.is_object() {
            return;
        }
        roll_daily(&mut t.sky);
        let total = t.sky["count"].as_i64().unwrap_or(0) + dir;
        t.sky["count"] = json!(total);
        total
    };
    app.to("sky_counter", "sky_count", &total).await;
}
