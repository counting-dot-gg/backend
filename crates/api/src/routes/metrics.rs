use crate::db::{self, has_role};
use crate::entity::js_number as num_json;
use crate::error::{ApiError, ApiResult};
use crate::http::{Query, Reply, js_to_number, reply};
use crate::session::Authed;
use crate::state::AppState;
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use cgg_web::now_ms;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

const CLIENT_STREAM: &str = "metrics:client_latency";
const HOST_STREAM: &str = "metrics:host";

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/metrics/public", get(public_status))
        .route("/metrics/client-health", get(client_health))
        .route("/metrics/latency", get(latency))
}

type Row = Map<String, Value>;

async fn read_stream(state: &AppState, stream: &str, since_ms: i64, limit: usize) -> ApiResult<Vec<Row>> {
    let from = now_ms() - since_ms;
    let mut conn = state.redis.clone();
    let raw: redis::Value = redis::cmd("XRANGE")
        .arg(stream)
        .arg(from.to_string())
        .arg("+")
        .arg("COUNT")
        .arg(limit)
        .query_async(&mut conn)
        .await
        .map_err(|e| tracing::warn!("XRANGE {stream}: {e}"))
        .unwrap_or(redis::Value::Nil);
    let text = |v: &redis::Value| -> Option<String> {
        match v {
            redis::Value::Data(b) => Some(String::from_utf8_lossy(b).into_owned()),
            redis::Value::Status(s) => Some(s.clone()),
            _ => None,
        }
    };
    let redis::Value::Bulk(entries) = raw else {
        return Ok(Vec::new());
    };
    Ok(entries
        .iter()
        .filter_map(|e| {
            let redis::Value::Bulk(parts) = e else { return None };
            let redis::Value::Bulk(pairs) = parts.get(1)? else {
                return None;
            };
            let mut out = Map::new();
            for kv in pairs.chunks(2) {
                if let [k, v] = kv
                    && let (Some(k), Some(v)) = (text(k), text(v))
                {
                    out.insert(k, Value::String(v));
                }
            }
            Some(out)
        })
        .collect())
}

fn js_round(x: f64) -> f64 {
    (x + 0.5).floor()
}

fn or0(x: f64) -> f64 {
    if x.is_nan() || x == 0.0 { 0.0 } else { x }
}

fn field(r: &Row, k: &str) -> f64 {
    r.get(k).and_then(Value::as_str).map_or(f64::NAN, js_to_number)
}

fn triple(v: Option<&Value>) -> [f64; 3] {
    let s = v.and_then(Value::as_str).unwrap_or("0,0,0");
    let parts: Vec<f64> = s.split(',').map(js_to_number).collect();
    let g = |i: usize| or0(parts.get(i).copied().unwrap_or(f64::NAN));
    [g(0), g(1), g(2)]
}

fn sorted(xs: &[f64]) -> Vec<f64> {
    let mut s = xs.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    s
}

fn mid(xs: &[f64]) -> f64 {
    if xs.is_empty() {
        return 0.0;
    }
    let s = sorted(xs);
    s[s.len() / 2]
}

fn worst(xs: &[f64]) -> f64 {
    xs.iter()
        .copied()
        .fold(None, |m: Option<f64>, x| Some(m.map_or(x, |m| m.max(x))))
        .unwrap_or(0.0)
}

fn minutes(q: &Query, d: f64, lo: f64, hi: f64) -> f64 {
    let n = or0(q.str("minutes").map_or(f64::NAN, js_to_number));
    let n = if n == 0.0 { d } else { n };
    n.max(lo).min(hi)
}

fn minute_of(t: f64) -> i64 {
    ((t / 60_000.0).floor() * 60_000.0) as i64
}

#[derive(Default)]
struct ClientMinute {
    n: f64,
    rt: Vec<f64>,
    unacc: Vec<f64>,
    proc: Vec<f64>,
    polling: i64,
    reports: i64,
}

async fn public_status(State(state): State<AppState>, q: Query) -> ApiResult<Reply> {
    let mins = minutes(&q, 60.0, 1.0, 180.0);
    let since = (mins * 60_000.0) as i64;
    let (clients, hosts) = (
        read_stream(&state, CLIENT_STREAM, since, 20_000).await?,
        read_stream(&state, HOST_STREAM, since, 20_000).await?,
    );
    let mut by_minute: BTreeMap<i64, ClientMinute> = BTreeMap::new();
    for r in &clients {
        let b = by_minute.entry(minute_of(field(r, "t"))).or_default();
        b.n += or0(field(r, "n"));
        b.reports += 1;
        b.rt.push(triple(r.get("rt"))[1]);
        b.unacc.push(triple(r.get("unacc"))[1]);
        b.proc.push(triple(r.get("proc"))[1]);
        if r.get("transport").and_then(Value::as_str) != Some("websocket") {
            b.polling += 1;
        }
    }
    let mut host_by: BTreeMap<i64, [Vec<f64>; 3]> = BTreeMap::new();
    for r in &hosts {
        let b = host_by.entry(minute_of(field(r, "t"))).or_default();
        b[0].push(or0(field(r, "cpuPct")));
        b[1].push(or0(field(r, "loopP99")));
        b[2].push(or0(field(r, "load")));
    }
    let mut keys: Vec<i64> = by_minute.keys().chain(host_by.keys()).copied().collect();
    keys.sort_unstable();
    keys.dedup();
    let timeline: Vec<Value> = keys
        .iter()
        .map(|t| {
            let c = by_minute.get(t);
            let h = host_by.get(t);
            json!({
                "t": t,
                "counts": c.map_or(json!(0), |c| num_json(c.n)),
                "reports": c.map_or(0, |c| c.reports),
                "roundTripMs": c.map_or(Value::Null, |c| num_json(mid(&c.rt))),
                "processingMs": c.map_or(Value::Null, |c| num_json(mid(&c.proc))),
                "unaccountedMs": c.map_or(Value::Null, |c| num_json(mid(&c.unacc))),
                "onPolling": c.map_or(0, |c| c.polling),
                "cpuPct": h.map_or(Value::Null, |h| num_json(js_round(mid(&h[0])))),
                "loopMs": h.map_or(Value::Null, |h| num_json(js_round(mid(&h[1])))),
                "load": h.map_or(Value::Null, |h| num_json((mid(&h[2]) * 100.0).round() / 100.0)),
            })
        })
        .collect();
    let recent: Vec<&Value> = timeline
        .iter()
        .rev()
        .take(5)
        .rev()
        .filter(|r| !r["roundTripMs"].is_null())
        .collect();
    let pick = |k: &str| -> Vec<f64> { recent.iter().filter_map(|r| r[k].as_f64()).collect() };
    let current = if recent.is_empty() {
        json!({ "roundTripMs": null, "processingMs": null, "unaccountedMs": null, "countsPerMinute": 0 })
    } else {
        json!({
            "roundTripMs": num_json(mid(&pick("roundTripMs"))),
            "processingMs": num_json(mid(&pick("processingMs"))),
            "unaccountedMs": num_json(mid(&pick("unaccountedMs"))),
            "countsPerMinute": num_json(js_round(mid(&pick("counts")))),
        })
    };
    Ok(reply(
        json!({ "minutes": num_json(mins), "now": now_ms(), "current": current, "timeline": timeline }),
    ))
}

async fn require_role(state: &AppState, uuid: &str, staff: bool) -> ApiResult<()> {
    match db::find_counter(state, uuid).await? {
        Some(c) if has_role(&c, "admin") || (staff && has_role(&c, "mod")) => Ok(()),
        _ => Err(ApiError::bare(StatusCode::FORBIDDEN)),
    }
}

fn finite(v: &Value) -> Option<f64> {
    v.as_f64().filter(|f| f.is_finite())
}

fn quant(xs: &[Option<f64>], p: f64) -> Value {
    let v = sorted(&xs.iter().flatten().copied().collect::<Vec<_>>());
    if v.is_empty() {
        return Value::Null;
    }
    let i = ((p * v.len() as f64).floor() as usize).min(v.len() - 1);
    num_json(js_round(v[i]))
}

fn maxv(xs: &[Option<f64>]) -> Value {
    let v: Vec<f64> = xs.iter().flatten().copied().collect();
    if v.is_empty() { Value::Null } else { num_json(worst(&v)) }
}

struct Rep {
    t: f64,
    c: String,
    j: Value,
}

impl Rep {
    fn at(&self, path: &[&str]) -> Option<f64> {
        let mut v = &self.j;
        for p in path {
            v = v.get(*p)?;
        }
        finite(v)
    }

    fn s(&self, k: &str) -> Value {
        self.j.get(k).cloned().unwrap_or(Value::Null)
    }
}

fn dev_key(dev: &Value) -> String {
    let s = |k: &str| dev.get(k).and_then(Value::as_str).unwrap_or("?").to_owned();
    let m = dev.get("m").is_some_and(crate::routes::user::truthy);
    format!("{} · {}{}", s("b"), s("os"), if m { " · mobile" } else { "" })
}

fn summary(g: &[&Rep]) -> Map<String, Value> {
    let with_own: Vec<&&Rep> = g
        .iter()
        .filter(|r| r.j.get("own").is_some_and(crate::routes::user::truthy))
        .collect();
    let col = |reps: &[&&Rep], path: &[&str]| -> Vec<Option<f64>> { reps.iter().map(|r| r.at(path)).collect() };
    let all: Vec<&&Rep> = g.iter().collect();
    let rt50 = quant(&col(&with_own, &["own", "rt", "p50"]), 0.5);
    let proc50 = quant(&col(&with_own, &["own", "proc", "p50"]), 0.5);
    let ping50 = quant(&col(&all, &["ping", "p50"]), 0.5);
    let kb: Vec<Option<f64>> = g
        .iter()
        .map(|r| Some(r.at(&["msg", "bytes"]).unwrap_or(0.0) / 1024.0))
        .collect();
    let mut m = Map::new();
    m.insert("reports".into(), json!(g.len()));
    m.insert(
        "counts".into(),
        num_json(with_own.iter().map(|r| r.at(&["own", "n"]).unwrap_or(0.0)).sum()),
    );
    m.insert("rtP50".into(), rt50.clone());
    m.insert("rtP95".into(), quant(&col(&with_own, &["own", "rt", "p95"]), 0.95));
    m.insert("procP50".into(), proc50.clone());
    m.insert("pingP50".into(), ping50.clone());
    m.insert("pingP95".into(), quant(&col(&all, &["ping", "p95"]), 0.95));
    m.insert("lagP50".into(), quant(&col(&all, &["lag", "p50"]), 0.5));
    m.insert("lagP95".into(), quant(&col(&all, &["lag", "p95"]), 0.95));
    m.insert("lagMax".into(), maxv(&col(&all, &["lag", "max"])));
    m.insert("longTaskMs".into(), quant(&col(&all, &["lt", "ms"]), 0.5));
    m.insert("longTaskMsP95".into(), quant(&col(&all, &["lt", "ms"]), 0.95));
    m.insert("jank".into(), quant(&col(&all, &["fr", "jank"]), 0.5));
    m.insert("msgs".into(), quant(&col(&all, &["msg", "total"]), 0.5));
    m.insert("kb".into(), quant(&kb, 0.5));
    m.insert(
        "polling".into(),
        json!(
            g.iter()
                .filter(|r| r.j.get("tr").and_then(Value::as_str).is_some_and(|t| t != "websocket"))
                .count()
        ),
    );
    m.insert("blockMs".into(), quant(&col(&all, &["loaf", "block"]), 0.5));
    m.insert("blockMsP95".into(), quant(&col(&all, &["loaf", "block"]), 0.95));
    m.insert("styleMs".into(), quant(&col(&all, &["loaf", "style"]), 0.5));
    m.insert("renderMs".into(), quant(&col(&all, &["loaf", "render"]), 0.5));
    m.insert("heapMb".into(), quant(&col(&all, &["mem"]), 0.5));
    m.insert("heapMbMax".into(), maxv(&col(&all, &["mem"])));
    m.insert("dom".into(), quant(&col(&all, &["dom"]), 0.5));
    m.insert("domMax".into(), maxv(&col(&all, &["dom"])));
    m.insert("ageMax".into(), maxv(&col(&all, &["age"])));
    let split = match rt50.as_f64() {
        None => Value::Null,
        Some(rt) => {
            let p = proc50.as_f64().unwrap_or(0.0);
            let ping = ping50.as_f64().unwrap_or(0.0);
            json!({
                "server": num_json(p),
                "network": num_json(ping.min((rt - p).max(0.0))),
                "rest": num_json((rt - p - ping).max(0.0)),
            })
        }
    };
    m.insert("split".into(), split);
    m
}

fn per(x: f64, n: usize) -> Value {
    num_json(js_round((x / n.max(1) as f64) * 10.0) / 10.0)
}

fn scripts_of(g: &[&Rep], n: usize) -> Vec<Value> {
    let mut m: Vec<(String, [f64; 3])> = Vec::new();
    for r in g {
        for s in
            r.j.pointer("/loaf/scripts")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        {
            let Some(a) = s.as_array() else { continue };
            let k = crate::http::js_string(a.first().unwrap_or(&Value::Null));
            let f = |i: usize| a.get(i).and_then(Value::as_f64).unwrap_or(0.0);
            match m.iter_mut().find(|x| x.0 == k) {
                Some(slot) => {
                    slot.1[0] += f(1);
                    slot.1[1] += f(2);
                    slot.1[2] += f(3);
                }
                None => m.push((k, [f(1), f(2), f(3)])),
            }
        }
    }
    m.sort_by(|a, b| b.1[0].partial_cmp(&a.1[0]).unwrap_or(std::cmp::Ordering::Equal));
    m.into_iter()
        .take(n)
        .map(|(script, v)| {
            json!({
                "script": script,
                "msPerTabMinute": per(v[0], g.len()),
                "count": num_json(v[1]),
                "forcedLayoutMs": num_json(js_round(v[2])),
            })
        })
        .collect()
}

fn group_by(reps: &[Rep], key: impl Fn(&Rep) -> String) -> Vec<(String, Vec<&Rep>)> {
    let mut out: Vec<(String, Vec<&Rep>)> = Vec::new();
    for r in reps {
        let k = key(r);
        match out.iter_mut().find(|x| x.0 == k) {
            Some(slot) => slot.1.push(r),
            None => out.push((k, vec![r])),
        }
    }
    out
}

fn null_or(v: Option<f64>) -> Value {
    v.map_or(Value::Null, num_json)
}

async fn client_health(State(state): State<AppState>, Authed(user): Authed, q: Query) -> ApiResult<Reply> {
    require_role(&state, &user.uuid, true).await?;
    let mins = minutes(&q, 60.0, 5.0, 1440.0);
    let raw = read_stream(&state, "metrics:client_health", (mins * 60_000.0) as i64, 50_000).await?;
    let reps: Vec<Rep> = raw
        .iter()
        .filter_map(|r| {
            let j: Value = serde_json::from_str(r.get("j")?.as_str()?).ok()?;
            j.is_object().then(|| Rep {
                t: field(r, "t"),
                c: r.get("c").and_then(Value::as_str).unwrap_or("").to_owned(),
                j,
            })
        })
        .collect();
    let by_counter = group_by(&reps, |r| r.c.clone());
    let named: Vec<String> = by_counter.iter().map(|x| x.0.clone()).filter(|c| c != "anon").collect();
    let cards = db::find_counters(&state, &named).await?;
    let mut counters: Vec<Value> = by_counter
        .iter()
        .map(|(uuid, g)| {
            let last = g[g.len() - 1];
            let mut routes: Vec<(String, i64)> = Vec::new();
            for r in g {
                let k = crate::http::js_string(&r.s("route"));
                match routes.iter_mut().find(|x| x.0 == k) {
                    Some(s) => s.1 += 1,
                    None => routes.push((k, 1)),
                }
            }
            routes.sort_by_key(|r| std::cmp::Reverse(r.1));
            let mut m = Map::new();
            m.insert("uuid".into(), json!(uuid));
            match cards.iter().find(|c| c["uuid"] == *uuid) {
                Some(c) => {
                    m.insert("username".into(), c["username"].clone());
                    m.insert("name".into(), c["name"].clone());
                    m.insert("color".into(), c["color"].clone());
                }
                None => {
                    let short: String = if uuid == "anon" {
                        "anon".into()
                    } else {
                        uuid.chars().take(8).collect()
                    };
                    m.insert("username".into(), json!(short));
                    m.insert("name".into(), json!(""));
                    m.insert("color".into(), json!(""));
                }
            }
            m.insert("lastSeen".into(), num_json(last.t));
            m.insert("device".into(), json!(dev_key(&last.s("dev"))));
            m.insert("route".into(), routes.first().map_or(Value::Null, |r| json!(r.0)));
            m.extend(summary(g));
            Value::Object(m)
        })
        .collect();
    let k = |v: &Value, key: &str| v[key].as_f64().unwrap_or(-1.0);
    counters.sort_by(|a, b| {
        let d = k(b, "rtP95") - k(a, "rtP95");
        let d = if d == 0.0 { k(b, "lagP95") - k(a, "lagP95") } else { d };
        d.partial_cmp(&0.0).unwrap_or(std::cmp::Ordering::Equal)
    });
    let grouped = |key: &dyn Fn(&Rep) -> String, n: usize| -> Vec<Value> {
        let mut g: Vec<Map<String, Value>> = group_by(&reps, key)
            .iter()
            .map(|(k, g)| {
                let mut m = Map::new();
                m.insert("key".into(), json!(k));
                m.extend(summary(g));
                m
            })
            .collect();
        g.sort_by(|a, b| b["reports"].as_i64().cmp(&a["reports"].as_i64()));
        g.into_iter().take(n).map(Value::Object).collect()
    };
    let mut events: Vec<(String, [f64; 3])> = Vec::new();
    for r in &reps {
        for e in r.j.pointer("/msg/top").and_then(Value::as_array).into_iter().flatten() {
            let Some(a) = e.as_array() else { continue };
            let name = crate::http::js_string(a.first().unwrap_or(&Value::Null));
            let f = |i: usize| a.get(i).and_then(Value::as_f64).unwrap_or(0.0);
            match events.iter_mut().find(|x| x.0 == name) {
                Some(s) => {
                    s.1[0] += f(1);
                    s.1[1] += f(2);
                    s.1[2] += f(3);
                }
                None => events.push((name, [f(1), f(2), f(3)])),
            }
        }
    }
    events.sort_by(|a, b| {
        b.1[2]
            .partial_cmp(&a.1[2])
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.1[0].partial_cmp(&a.1[0]).unwrap_or(std::cmp::Ordering::Equal))
    });
    let messages: Vec<Value> = events
        .iter()
        .take(15)
        .map(|(event, e)| {
            json!({
                "event": event,
                "perTabMinute": per(e[0], reps.len()),
                "kbPerTabMinute": per(e[1] / 1024.0, reps.len()),
                "msPerTabMinute": per(e[2], reps.len()),
            })
        })
        .collect();
    let all: Vec<&Rep> = reps.iter().collect();
    let detail = match q.str("counter").filter(|c| !c.is_empty()) {
        None => Value::Null,
        Some(name) => {
            let uuid = db::find_counter_by_username(&state, name)
                .await?
                .and_then(|c| c["uuid"].as_str().map(str::to_owned));
            let g: Vec<&Rep> = uuid
                .as_ref()
                .and_then(|u| by_counter.iter().find(|x| &x.0 == u))
                .map(|x| x.1.clone())
                .unwrap_or_default();
            let mut d = Map::new();
            d.insert("username".into(), json!(name));
            d.insert("found".into(), json!(uuid.is_some()));
            d.extend(summary(&g));
            d.insert("scripts".into(), json!(scripts_of(&g, 15)));
            let start = g.len().saturating_sub(240);
            let minutes: Vec<Value> = g[start..]
                .iter()
                .map(|r| {
                    let top: Vec<Value> = r.j.pointer("/msg/top").and_then(Value::as_array).map(|a| a.iter().take(5).cloned().collect()).unwrap_or_default();
                    let scripts: Vec<Value> = r.j.pointer("/loaf/scripts").and_then(Value::as_array).map(|a| a.iter().take(3).cloned().collect()).unwrap_or_default();
                    json!({
                        "t": num_json(r.t), "route": r.s("route"), "tr": r.s("tr"), "vis": r.s("vis"),
                        "device": dev_key(&r.s("dev")), "dev": r.s("dev"),
                        "rtP50": null_or(r.at(&["own", "rt", "p50"])), "rtP95": null_or(r.at(&["own", "rt", "p95"])),
                        "procP50": null_or(r.at(&["own", "proc", "p50"])), "counts": num_json(r.at(&["own", "n"]).unwrap_or(0.0)),
                        "pingP50": null_or(r.at(&["ping", "p50"])), "lagP50": null_or(r.at(&["lag", "p50"])),
                        "lagP95": null_or(r.at(&["lag", "p95"])), "lagMax": null_or(r.at(&["lag", "max"])),
                        "longTaskMs": null_or(r.at(&["lt", "ms"])), "jank": null_or(r.at(&["fr", "jank"])),
                        "msgs": null_or(r.at(&["msg", "total"])), "kb": num_json(js_round(r.at(&["msg", "bytes"]).unwrap_or(0.0) / 1024.0)),
                        "top": top,
                        "blockMs": null_or(r.at(&["loaf", "block"])), "styleMs": null_or(r.at(&["loaf", "style"])),
                        "renderMs": null_or(r.at(&["loaf", "render"])), "scripts": scripts,
                        "heapMb": null_or(r.at(&["mem"])), "dom": null_or(r.at(&["dom"])), "age": null_or(r.at(&["age"])),
                    })
                })
                .collect();
            d.insert("minutes".into(), json!(minutes));
            Value::Object(d)
        }
    };
    let mut overall = summary(&all);
    overall.insert("counters".into(), json!(named.len()));
    Ok(reply(json!({
        "minutes": num_json(mins),
        "now": now_ms(),
        "overall": overall,
        "counters": counters.into_iter().take(200).collect::<Vec<_>>(),
        "pages": grouped(&|r: &Rep| crate::http::js_string(&r.s("route")), 15),
        "devices": grouped(&|r: &Rep| dev_key(&r.s("dev")), 15),
        "messages": messages,
        "scripts": scripts_of(&all, 15),
        "detail": detail,
    })))
}

fn iso(ms: f64) -> Value {
    chrono::DateTime::from_timestamp_millis(ms as i64).map_or(Value::Null, |t| json!(crate::entity::js_iso(t)))
}

async fn latency(State(state): State<AppState>, Authed(user): Authed, q: Query) -> ApiResult<Reply> {
    require_role(&state, &user.uuid, false).await?;
    let mins = minutes(&q, 30.0, 1.0, 240.0);
    let since = (mins * 60_000.0) as i64;
    let rows = read_stream(&state, CLIENT_STREAM, since, 20_000).await?;
    let hosts = read_stream(&state, HOST_STREAM, since, 20_000).await?;
    let thread = q.str("thread").filter(|t| !t.is_empty());
    struct C {
        t: f64,
        n: f64,
        rt: [f64; 3],
        proc: [f64; 3],
        unacc: [f64; 3],
        ping: [f64; 3],
        blocked: f64,
        polling: bool,
    }
    let clients: Vec<C> = rows
        .iter()
        .filter(|r| thread.is_none_or(|t| r.get("thread").and_then(Value::as_str) == Some(t)))
        .map(|r| C {
            t: field(r, "t"),
            n: or0(field(r, "n")),
            rt: triple(r.get("rt")),
            proc: triple(r.get("proc")),
            unacc: triple(r.get("unacc")),
            ping: triple(r.get("ping")),
            blocked: or0(field(r, "blocked")),
            polling: r.get("transport").and_then(Value::as_str) != Some("websocket"),
        })
        .collect();
    let mut by: BTreeMap<i64, (f64, [Vec<f64>; 5], i64)> = BTreeMap::new();
    for c in &clients {
        let b = by.entry(minute_of(c.t)).or_default();
        b.0 += c.n;
        b.1[0].push(c.rt[1]);
        b.1[1].push(c.unacc[1]);
        b.1[2].push(c.proc[1]);
        b.1[3].push(c.ping[1]);
        b.1[4].push(c.blocked);
        if c.polling {
            b.2 += 1;
        }
    }
    let timeline: Vec<Value> = by
        .iter()
        .map(|(t, (n, v, polling))| {
            json!({
                "t": t, "iso": iso(*t as f64), "counts": num_json(*n), "reports": v[0].len(),
                "roundTripP95": num_json(mid(&v[0])), "roundTripWorst": num_json(worst(&v[0])),
                "unaccountedP95": num_json(mid(&v[1])), "unaccountedWorst": num_json(worst(&v[1])),
                "processingP95": num_json(mid(&v[2])), "pingP95": num_json(mid(&v[3])),
                "elsewhereP95": num_json((mid(&v[0]) - mid(&v[2]) - mid(&v[3])).max(0.0)),
                "blockedMs": num_json(mid(&v[4])), "onPolling": polling,
            })
        })
        .collect();
    let host: Vec<Value> = hosts
        .iter()
        .map(|r| {
            let f = |k: &str| num_json(or0(field(r, k)));
            let disks: Map<String, Value> = r
                .iter()
                .filter_map(|(k, v)| {
                    let name = k.strip_prefix("disk_")?;
                    let p: Vec<f64> = crate::http::js_string(v).split(',').map(js_to_number).collect();
                    let g = |i: usize| num_json(or0(p.get(i).copied().unwrap_or(f64::NAN)));
                    Some((
                        name.to_owned(),
                        json!({
                            "utilPct": g(0), "readsPerSec": g(1), "writesPerSec": g(2), "readAwaitMs": g(3),
                            "writeAwaitMs": g(4), "fsyncAwaitMs": g(5), "inFlight": g(6),
                        }),
                    ))
                })
                .collect();
            json!({
                "t": num_json(field(r, "t")), "iso": iso(field(r, "t")), "cpuPct": f("cpuPct"), "load": f("load"),
                "runq": f("runq"), "memAvailMb": f("memAvailMb"), "swapUsedMb": f("swapUsedMb"), "ctxPerSec": f("ctxPerSec"),
                "loopP99": f("loopP99"), "loopMax": f("loopMax"), "redisOps": f("redisOps"), "redisSlow": f("redisSlow"),
                "retransPerSec": f("retransPerSec"), "listenDrops": f("listenDrops"),
                "diskBusiest": r.get("diskBusiest").cloned().unwrap_or(json!("-")),
                "diskBusiestUtilPct": f("diskBusiestUtilPct"), "disks": disks,
            })
        })
        .collect();
    let mx = |f: fn(&C) -> f64| num_json(worst(&clients.iter().map(f).collect::<Vec<_>>()));
    Ok(reply(json!({
        "minutes": num_json(mins),
        "summary": {
            "reports": clients.len(),
            "countsSampled": num_json(clients.iter().map(|c| c.n).sum()),
            "roundTripWorst": mx(|c| c.rt[2]),
            "processingWorst": mx(|c| c.proc[2]),
            "unaccountedWorst": mx(|c| c.unacc[2]),
            "clientsOnPolling": clients.iter().filter(|c| c.polling).count(),
        },
        "timeline": timeline,
        "host": host,
    })))
}
