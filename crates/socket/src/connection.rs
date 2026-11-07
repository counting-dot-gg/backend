use crate::app::{App, now_ms};
use crate::diagnosis::{self, Cause, Drop, Minute, Others, minute_of};
use crate::gateway::user_of;
use dashmap::DashMap;
use parking_lot::Mutex;
use serde_json::{Map, Value, json};
use socketioxide::extract::SocketRef;
use socketioxide::socket::DisconnectReason;
use std::collections::{BTreeMap, HashSet};
use std::sync::LazyLock;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

const PER_COUNTER_MAXLEN: usize = 3000;
const PER_COUNTER_TTL_SECS: i64 = 3 * 24 * 3600;
const BASELINE_STREAM: &str = "metrics:client_minute";
const BASELINE_MAXLEN: usize = 6000;
const LEGACY_LATENCY: &str = "metrics:client_latency";
const LEGACY_HEALTH: &str = "metrics:client_health";
const LEGACY_SCAN_CAP: usize = 120_000;
const CHECK_EVERY: Duration = Duration::from_secs(5);

fn per_counter(uuid: &str) -> String {
    format!("metrics:conn:{uuid}")
}

fn num(v: &Value) -> Option<f64> {
    v.as_f64().filter(|n| n.is_finite() && *n >= 0.0)
}

fn p95(v: &Value) -> Option<f64> {
    num(&v["p95"])
}

fn spread_p95(s: &str) -> Option<f64> {
    s.split(',')
        .nth(1)
        .and_then(|x| x.parse::<f64>().ok())
        .filter(|n| n.is_finite())
}

fn text(v: &Value, max: usize) -> Option<String> {
    v.as_str()
        .filter(|t| !t.is_empty() && t.encode_utf16().count() <= max)
        .map(str::to_owned)
}

fn reason_key(s: &str) -> String {
    s.chars()
        .take(32)
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

pub fn from_latency_report(r: &Value, t: i64) -> Minute {
    let blocked = (r["longtask"].as_bool() == Some(true))
        .then(|| num(&r["blockedMs"]))
        .flatten();
    Minute {
        t,
        rt: p95(&r["roundTrip"]),
        proc_: p95(&r["processing"]),
        up: p95(&r["uplink"]),
        down: p95(&r["downlink"]),
        ping: p95(&r["ping"]).filter(|p| *p > 0.0),
        loop_lag: p95(&r["loopLag"]),
        blocked,
        polling: r["transport"].as_str().is_some_and(|t| t != "websocket"),
        counts: num(&r["n"]).unwrap_or(0.0),
        ..Default::default()
    }
}

fn from_latency_row(r: &Map<String, Value>) -> Option<(String, Minute)> {
    let f = |k: &str| r.get(k).and_then(Value::as_str);
    let t = f("t")?.parse::<i64>().ok()?;
    let counter = f("counter")?.to_owned();
    let leg = |k: &str| f(k).and_then(spread_p95).filter(|v| *v > 0.0);
    let m = Minute {
        t,
        rt: f("rt").and_then(spread_p95),
        proc_: f("proc").and_then(spread_p95),
        up: leg("up"),
        down: leg("down"),
        ping: leg("ping"),
        loop_lag: leg("loop"),
        blocked: f("blocked").and_then(|b| b.parse::<f64>().ok()).filter(|b| *b > 0.0),
        polling: f("transport").is_some_and(|t| t != "websocket"),
        counts: f("n").and_then(|n| n.parse::<f64>().ok()).unwrap_or(0.0),
        ..Default::default()
    };
    Some((counter, m))
}

pub fn from_health(j: &Value, t: i64) -> Minute {
    let conn = &j["conn"];
    let drops = conn["drops"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|d| {
            Some(Drop {
                at: d[0].as_f64()? as i64,
                reason: reason_key(d[1].as_str()?),
                down_ms: num(&d[2]),
                side: "client".into(),
            })
        })
        .collect();
    let ping_back = j["ping"].is_object().then(|| num(&j["pings"]["back"])).flatten();
    Minute {
        t,
        rt: p95(&j["own"]["rt"]),
        proc_: p95(&j["own"]["proc"]),
        ping: p95(&j["ping"]).filter(|p| *p > 0.0),
        loop_lag: p95(&j["lag"]),
        frames_blocked: num(&j["loaf"]["block"]),
        edge: num(&conn["edge"]["ms"]),
        colo: text(&conn["edge"]["colo"], 8),
        vis: num(&j["vis"]).map(|v| v.min(1.0)),
        pings_sent: num(&j["pings"]["sent"]).unwrap_or(0.0),
        pings_back: ping_back.unwrap_or(0.0),
        drops,
        offline_ms: num(&conn["offlineMs"]).unwrap_or(0.0),
        polling: j["tr"].as_str().is_some_and(|t| t != "websocket" && t != "unknown"),
        counts: num(&j["own"]["n"]).unwrap_or(0.0),
        net: text(&j["dev"]["net"], 8),
        ..Default::default()
    }
}

struct Baseline {
    t: i64,
    current: Others,
    clients: HashSet<String>,
    last: Option<Others>,
}

static BASELINE: LazyLock<Mutex<Baseline>> = LazyLock::new(|| {
    Mutex::new(Baseline {
        t: minute_of(now_ms()),
        current: Others::default(),
        clients: HashSet::new(),
        last: None,
    })
});

fn add_to_baseline(who: &str, m: &Minute) -> Option<Others> {
    let mut b = BASELINE.lock();
    let t = minute_of(m.t);
    let mut finished = None;
    if t > b.t {
        let mut done = std::mem::take(&mut b.current);
        done.t = b.t;
        done.clients = b.clients.len();
        b.clients.clear();
        b.t = t;
        b.last = Some(done.clone());
        finished = Some(done);
    }
    if t == b.t {
        b.current.add(m);
        b.clients.insert(who.to_owned());
    }
    finished
}

fn last_minute_baseline() -> Option<Others> {
    BASELINE.lock().last.clone()
}

fn live_baseline() -> Vec<Others> {
    let b = BASELINE.lock();
    let mut current = b.current.clone();
    current.t = b.t;
    current.clients = b.clients.len();
    b.last.iter().cloned().chain(std::iter::once(current)).collect()
}

async fn write_baseline(app: &App, o: &Others) {
    let Ok(j) = serde_json::to_string(o) else { return };
    let mut redis = app.redis.clone();
    let _: Result<String, _> = redis::cmd("XADD")
        .arg(BASELINE_STREAM)
        .arg("MAXLEN")
        .arg("~")
        .arg(BASELINE_MAXLEN)
        .arg("*")
        .arg("t")
        .arg(o.t)
        .arg("j")
        .arg(j)
        .query_async(&mut redis)
        .await;
}

async fn append(app: &App, uuid: &str, kind: &str, m: &Minute) {
    let Ok(j) = serde_json::to_string(m) else { return };
    let key = per_counter(uuid);
    let mut redis = app.redis.clone();
    let _: Result<(), _> = redis::pipe()
        .cmd("XADD")
        .arg(&key)
        .arg("MAXLEN")
        .arg("~")
        .arg(PER_COUNTER_MAXLEN)
        .arg("*")
        .arg("k")
        .arg(kind)
        .arg("j")
        .arg(j)
        .ignore()
        .cmd("EXPIRE")
        .arg(&key)
        .arg(PER_COUNTER_TTL_SECS)
        .ignore()
        .query_async(&mut redis)
        .await;
}

pub async fn record(app: &App, s: &SocketRef, kind: &str, m: Minute) {
    let user = user_of(s);
    let who = user.clone().unwrap_or_else(|| format!("sock:{}", s.id));
    if let Some(done) = add_to_baseline(&who, &m) {
        write_baseline(app, &done).await;
    }
    if m.is_laggy() {
        let crowd = last_minute_baseline()
            .unwrap_or_default()
            .without(std::slice::from_ref(&m));
        let cause = diagnosis::classify(&m, &crowd, None);
        app.metrics.slow_reports[cause.index()].fetch_add(1, Ordering::Relaxed);
    }
    if let Some(u) = user {
        append(app, &u, kind, &m).await;
    }
}

fn disconnect_key(r: DisconnectReason) -> &'static str {
    match r {
        DisconnectReason::TransportClose => "transport_close",
        DisconnectReason::MultipleHttpPollingError => "multiple_polling",
        DisconnectReason::PacketParsingError => "packet_parsing",
        DisconnectReason::TransportError => "transport_error",
        DisconnectReason::HeartbeatTimeout => "heartbeat_timeout",
        DisconnectReason::ClientNSDisconnect => "client_disconnect",
        DisconnectReason::ServerNSDisconnect => "server_disconnect",
        DisconnectReason::ClosingServer => "closing_server",
    }
}

pub const DISCONNECT_KEYS: [&str; 8] = [
    "transport_close",
    "multiple_polling",
    "packet_parsing",
    "transport_error",
    "heartbeat_timeout",
    "client_disconnect",
    "server_disconnect",
    "closing_server",
];

pub async fn on_disconnect(app: &App, s: &SocketRef, reason: DisconnectReason) {
    let key = disconnect_key(reason);
    if let Some(i) = DISCONNECT_KEYS.iter().position(|k| *k == key) {
        app.metrics.disconnects[i].fetch_add(1, Ordering::Relaxed);
    }
    CHECKED.remove(&s.id.to_string());
    let Some(u) = user_of(s) else { return };
    let t = now_ms();
    let m = Minute {
        t,
        drops: vec![Drop {
            at: t,
            reason: key.into(),
            down_ms: None,
            side: "server".into(),
        }],
        ..Default::default()
    };
    append(app, &u, "d", &m).await;
}

type Row = (i64, Map<String, Value>);

async fn xrange(app: &App, key: &str, from_ms: i64, count: usize) -> Vec<Row> {
    let mut conn = app.redis.clone();
    let raw: redis::Value = redis::cmd("XRANGE")
        .arg(key)
        .arg(from_ms.max(0).to_string())
        .arg("+")
        .arg("COUNT")
        .arg(count)
        .query_async(&mut conn)
        .await
        .unwrap_or(redis::Value::Nil);
    let as_text = |v: &redis::Value| -> Option<String> {
        match v {
            redis::Value::Data(b) => Some(String::from_utf8_lossy(b).into_owned()),
            redis::Value::Status(s) => Some(s.clone()),
            _ => None,
        }
    };
    let redis::Value::Bulk(entries) = raw else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|e| {
            let redis::Value::Bulk(parts) = e else { return None };
            let id = as_text(parts.first()?)?;
            let ms = id.split('-').next()?.parse::<i64>().ok()?;
            let redis::Value::Bulk(pairs) = parts.get(1)? else {
                return None;
            };
            let mut out = Map::new();
            for kv in pairs.chunks(2) {
                if let [k, v] = kv
                    && let (Some(k), Some(v)) = (as_text(k), as_text(v))
                {
                    out.insert(k, Value::String(v));
                }
            }
            Some((ms, out))
        })
        .collect()
}

fn parse_minute(r: &Map<String, Value>) -> Option<Minute> {
    serde_json::from_str(r.get("j")?.as_str()?).ok()
}

fn legacy_target(latency: &[Row], health: &[Row], uuid: &str, before: i64) -> Vec<Minute> {
    let mut out: Vec<Minute> = latency
        .iter()
        .filter(|(ms, _)| *ms < before)
        .filter_map(|(_, r)| from_latency_row(r))
        .filter(|(c, _)| c == uuid)
        .map(|(_, m)| m)
        .collect();
    for (ms, r) in health.iter().filter(|(ms, _)| *ms < before) {
        if r.get("c").and_then(Value::as_str) != Some(uuid) {
            continue;
        }
        let Some(j) = r
            .get("j")
            .and_then(Value::as_str)
            .and_then(|j| serde_json::from_str::<Value>(j).ok())
        else {
            continue;
        };
        out.push(from_health(&j, *ms));
    }
    out
}

fn legacy_baseline(latency: &[Row]) -> BTreeMap<i64, Others> {
    let mut out: BTreeMap<i64, (Others, HashSet<String>)> = BTreeMap::new();
    for (c, m) in latency.iter().filter_map(|(_, r)| from_latency_row(r)) {
        let t = minute_of(m.t);
        let e = out.entry(t).or_insert_with(|| {
            (
                Others {
                    t,
                    ..Default::default()
                },
                HashSet::new(),
            )
        });
        e.0.add(&m);
        e.1.insert(c);
    }
    out.into_iter()
        .map(|(t, (mut o, set))| {
            o.clients = set.len();
            (t, o)
        })
        .collect()
}

static CHECKED: LazyLock<DashMap<String, Instant>> = LazyLock::new(DashMap::new);

fn too_soon(s: &SocketRef) -> bool {
    let id = s.id.to_string();
    if CHECKED.get(&id).is_some_and(|t| t.elapsed() < CHECK_EVERY) {
        return true;
    }
    CHECKED.insert(id, Instant::now());
    false
}

async fn uuid_of_username(app: &App, name: &str) -> Option<(String, String)> {
    sqlx::query_as::<_, (String, String)>("SELECT uuid, username FROM counters WHERE username = ? LIMIT 1")
        .bind(name)
        .fetch_optional(&app.pool)
        .await
        .ok()
        .flatten()
}

fn minute_json(m: &Minute, c: Cause, ev: &[diagnosis::Evidence]) -> Value {
    let r = |v: Option<f64>| v.map(f64::round);
    let mut o = json!({
        "t": m.t, "cause": c.key(), "rt": r(m.rt), "ping": r(m.ping), "up": r(m.up), "down": r(m.down),
        "server": r(m.proc_), "edge": r(m.edge), "counts": m.counts,
    });
    if c != Cause::Ok {
        o["evidence"] = json!(ev.iter().map(diagnosis::Evidence::json).collect::<Vec<_>>());
    }
    if let Some(l) = m.ping_loss().filter(|l| *l > 0.0) {
        o["loss"] = json!((l * 100.0).round());
    }
    if m.vis.is_some_and(|v| v < 1.0) {
        o["vis"] = json!(m.vis.map(|v| (v * 100.0).round()));
    }
    if !m.drops.is_empty() {
        o["drops"] = json!(
            m.drops
                .iter()
                .map(|d| json!({ "at": d.at, "reason": d.reason, "side": d.side, "downMs": d.down_ms }))
                .collect::<Vec<_>>()
        );
    }
    o
}

pub async fn check(app: &App, s: &SocketRef, d: &Value) {
    if too_soon(s) {
        return;
    }
    let reply = |v: Value| {
        let _ = s.emit("connection_report", &v);
    };
    let Some(me) = user_of(s) else {
        reply(json!({ "error": "signed_out" }));
        return;
    };
    let caller = app.counter(&me).await;
    let staff = caller
        .as_ref()
        .is_some_and(|c| c.has_role("admin") || c.has_role("mod"));
    let asked = d["username"].as_str().map(str::trim).filter(|u| !u.is_empty());
    let (uuid, username) = match asked {
        Some(name) if staff => match uuid_of_username(app, name).await {
            Some(found) => found,
            None => {
                reply(json!({ "error": "not_found", "username": name }));
                return;
            }
        },
        Some(_) => {
            reply(json!({ "error": "forbidden" }));
            return;
        }
        None => (
            me.clone(),
            caller.as_ref().map(|c| c.username.clone()).unwrap_or_default(),
        ),
    };
    let max_minutes = if staff { 24 * 60 } else { 6 * 60 };
    let minutes = d["minutes"]
        .as_f64()
        .filter(|m| m.is_finite())
        .map_or(60, |m| m.clamp(5.0, max_minutes as f64) as i64);
    let now = now_ms();
    let from = now - minutes * 60_000;
    let own_rows = xrange(app, &per_counter(&uuid), from, PER_COUNTER_MAXLEN).await;
    let mut samples: Vec<Minute> = own_rows.iter().filter_map(|(_, r)| parse_minute(r)).collect();
    let mut baseline: BTreeMap<i64, Others> = xrange(app, BASELINE_STREAM, from, BASELINE_MAXLEN)
        .await
        .iter()
        .filter_map(|(_, r)| serde_json::from_str::<Others>(r.get("j")?.as_str()?).ok())
        .map(|o| (o.t, o))
        .collect();
    for o in live_baseline() {
        if o.t >= minute_of(from) {
            baseline.entry(o.t).or_insert(o);
        }
    }
    let earliest_own = own_rows.first().map_or(i64::MAX, |r| r.0);
    let earliest_base = baseline.keys().next().copied().unwrap_or(i64::MAX);
    if earliest_own > from + 120_000 || earliest_base > from + 120_000 {
        let latency = xrange(app, LEGACY_LATENCY, from, LEGACY_SCAN_CAP).await;
        for (t, o) in legacy_baseline(&latency) {
            if t < earliest_base {
                baseline.entry(t).or_insert(o);
            }
        }
        if staff && earliest_own > from + 120_000 {
            let health = xrange(app, LEGACY_HEALTH, from, LEGACY_SCAN_CAP).await;
            let older = legacy_target(&latency, &health, &uuid, earliest_own);
            samples.extend(older);
        }
    }
    let you = diagnosis::merge_by_minute(samples);
    let dx = diagnosis::diagnose(&you, &baseline);
    let minutes_json: Vec<Value> = dx.minutes.iter().map(|(m, c, ev)| minute_json(m, *c, ev)).collect();
    let causes: Map<String, Value> = dx
        .by_cause
        .iter()
        .map(|(c, n)| (c.key().to_owned(), json!(n)))
        .collect();
    let proof = dx
        .proof
        .map(|i| minute_json(&dx.minutes[i].0, dx.minutes[i].1, &dx.minutes[i].2));
    let colo = you.values().rev().find_map(|m| m.colo.clone());
    let net = you.values().rev().find_map(|m| m.net.clone());
    reply(json!({
        "target": { "uuid": uuid, "username": username, "self": uuid == me },
        "from": from, "to": now, "window": minutes,
        "verdict": dx.verdict.key(), "ours": dx.verdict.is_ours(),
        "measured": dx.minutes.len(), "laggy": dx.laggy, "causes": causes,
        "proof": proof, "usualEdge": dx.usual_edge.map(f64::round), "colo": colo, "net": net,
        "minutes": minutes_json,
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_latency_rows_parse_into_minutes() {
        let mut r = Map::new();
        for (k, v) in [
            ("t", "1793824800123"),
            ("counter", "abc"),
            ("rt", "200,9324,9400"),
            ("proc", "4,6,9"),
            ("up", "100,9231,9300"),
            ("down", "60,250,400"),
            ("ping", "0,0,0"),
            ("transport", "websocket"),
            ("n", "12"),
        ] {
            r.insert(k.into(), Value::String(v.into()));
        }
        let (c, m) = from_latency_row(&r).unwrap();
        assert_eq!(c, "abc");
        assert_eq!(m.up, Some(9231.0));
        assert_eq!(m.ping, None);
        assert_eq!(m.counts, 12.0);
        assert!(!m.polling);
    }

    #[test]
    fn health_reports_carry_drops_probes_and_lost_pings() {
        let j = json!({
            "tr": "websocket", "vis": 1.0, "ping": { "p50": 90, "p95": 4200, "max": 8000 },
            "pings": { "sent": 20, "back": 9 }, "lag": { "p50": 2, "p95": 8, "max": 20 },
            "conn": { "drops": [[1793824800000_i64, "ping timeout", 5300]], "offlineMs": 0, "edge": { "ms": 3900, "colo": "IAD" } },
            "dev": { "net": "4g" },
        });
        let m = from_health(&j, 1793824800000);
        assert_eq!(m.ping, Some(4200.0));
        assert_eq!(m.pings_sent, 20.0);
        assert_eq!(m.pings_back, 9.0);
        assert_eq!(m.drops[0].reason, "ping_timeout");
        assert!(m.drops[0].is_network());
        assert_eq!(m.edge, Some(3900.0));
        assert_eq!(m.colo.as_deref(), Some("IAD"));
    }

    #[test]
    fn a_minute_round_trips_through_its_stored_form() {
        let m = Minute {
            t: 60_000,
            rt: Some(300.0),
            drops: vec![Drop {
                at: 1,
                reason: "x".into(),
                down_ms: Some(2.0),
                side: "client".into(),
            }],
            ..Default::default()
        };
        let back: Minute = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
        assert_eq!(back, m);
    }
}
