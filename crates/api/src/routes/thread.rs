use crate::db::{self, has_role};
use crate::entity;
use crate::error::{ApiError, ApiResult};
use crate::http::{JsonBody, Query, Reply, empty, js_number, js_string, js_to_number, reply};
use crate::routes::counter::thread_visible_to;
use crate::routes::user::truthy;
use crate::session::{Authed, MaybeUser};
use crate::state::AppState;
use crate::statsread::coverage;
use crate::statsread::owners::{self, Unit};
use crate::statsread::store::{self, ThreadStatsOpts};
use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use chrono::{Duration as ChronoDuration, NaiveDate, TimeZone};
use chrono_tz::America::New_York;
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/thread/all", get(all_threads))
        .route("/thread/leanStats", get(lean_stats))
        .route("/thread/msCoverage", get(ms_coverage))
        .route("/thread/countingTime", get(counting_time))
        .route("/thread/csCoverage", get(cs_coverage))
        .route("/thread/csCoverage/claims", get(cs_claims))
        .route("/thread/csCoverage/hour/{hour}", get(cs_hour))
        .route("/thread/csCoverage/hour/{hour}/second/{second}/owners", get(cs_owners))
        .route("/thread/csCoverage/hour/{hour}/second/{second}", get(cs_second))
        .route("/thread/msCoverage/counter/{uuid}", get(ms_counter))
        .route("/thread/msCoverage/hour/{hour}", get(ms_hour))
        .route("/thread/msCoverage/hour/{hour}/second/{second}", get(ms_second))
        .route("/thread/msCoverage/hour/{hour}/second/{second}/owners", get(ms_owners))
        .route("/thread/msClaims", get(ms_claims))
        .route("/thread/latestCounts", get(latest_counts))
        .route("/thread/stats/threadStats", post(thread_stats))
        .route("/thread/stats/threadRankList", post(thread_rank_list))
        .route("/thread/stats/threadStatsDetails", post(thread_stats_details))
        .route("/thread/stats/threadGraph", post(thread_graph))
        .route("/thread/stats/robs", post(rob_stats))
        .route(
            "/thread/stats/threadDailyCountLeaderboard",
            post(daily_count_leaderboard),
        )
        .route("/thread/stats/markSplitFake", post(mark_split_fake))
        .route("/thread/loadOlderCounts", get(load_older))
        .route("/thread/loadNewerCounts", get(load_newer))
        .route("/thread/robs", get(robs))
        .route("/thread/blogs/{page}", get(blogs))
        .route("/thread/allBlogs", get(all_blogs))
        .route("/thread/blog/{key}", get(blog))
        .route("/thread/blogReaction", post(blog_reaction))
        .route("/thread/shuffle/{seed}", get(shuffle))
        .route("/thread/{name}", get(thread_by_name))
        .route("/thread/updateCommunityNotes", post(update_notes))
        .route("/thread/create", post(create_thread))
        .route("/thread/getRecent", post(get_recent))
        .route("/thread/recentOnLoadFull/{thread_name}", get(recent_on_load_full))
        .route("/thread/getCountByUuid", post(count_by_uuid))
        .route("/thread/findPostByThreadAndRawCount", post(by_raw_count))
        .route("/thread/findPostByThreadAndNumber", post(by_number))
        .route("/thread/findPostByThreadAndComment", post(by_comment))
        .route("/thread/modToggleThreadLock", post(toggle_lock))
        .route("/thread/modToggleSilentThreadLock", post(toggle_silent_lock))
}

fn bare(s: StatusCode) -> ApiError {
    ApiError::bare(s)
}

fn is_day(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

async fn viewer(state: &AppState, user: Option<&crate::session::SessionUser>) -> ApiResult<Option<Value>> {
    match user {
        Some(u) => db::find_counter(state, &u.uuid).await,
        None => Ok(None),
    }
}

pub async fn find_thread(
    state: &AppState,
    name: &str,
    counter: Option<&Value>,
    has_user: bool,
) -> ApiResult<Option<Value>> {
    let Some(t) = db::find_thread_by_name(state, name).await? else {
        return Ok(None);
    };
    let ok = match (has_user, counter) {
        (true, Some(c)) => thread_visible_to(&t, Some(c)),
        _ => thread_visible_to(&t, None),
    };
    Ok(ok.then_some(t))
}

async fn all_threads(State(state): State<AppState>, MaybeUser(user): MaybeUser) -> ApiResult<Reply> {
    let c = viewer(&state, user.as_ref()).await?;
    let rows = sqlx::query("SELECT * FROM thread").fetch_all(&state.pool).await?;
    let threads: Vec<Value> = rows
        .iter()
        .map(|r| entity::THREAD.json(r))
        .filter(|t| thread_visible_to(t, c.as_ref()))
        .collect();
    Ok(reply(threads))
}

async fn lean_stats(State(state): State<AppState>, q: Query) -> ApiResult<Reply> {
    let start = q.str("start").filter(|s| !s.is_empty());
    let end = q.str("end").filter(|s| !s.is_empty());
    if start.is_some_and(|s| !is_day(s)) || end.is_some_and(|s| !is_day(s)) {
        return Err(ApiError::bad_request("start and end must be YYYY-MM-DD"));
    }
    Ok(reply(Value::Object(
        store::lean_totals_for_range(&state, start, end).await,
    )))
}

async fn ms_coverage(State(state): State<AppState>) -> ApiResult<Reply> {
    Ok(reply(
        coverage::ms_coverage(&state)
            .await
            .map_err(|e| ApiError::Internal(e.into()))?,
    ))
}

async fn cs_coverage(State(state): State<AppState>) -> ApiResult<Reply> {
    Ok(reply(
        coverage::cs_coverage(&state)
            .await
            .map_err(|e| ApiError::Internal(e.into()))?,
    ))
}

async fn counting_time(State(state): State<AppState>) -> ApiResult<Reply> {
    Ok(reply(coverage::sitewide_counting_seconds(&state).await))
}

fn claim_limit(q: &Query) -> usize {
    let n = q.str("limit").map_or(0.0, js_to_number);
    let n = if q.str("limit").is_none() { f64::NAN } else { n };
    if n.is_finite() {
        n.trunc().clamp(1.0, 250.0) as usize
    } else {
        50
    }
}

fn claims_json(v: Vec<(String, f64)>) -> Value {
    json!(
        v.into_iter()
            .map(|(u, n)| json!({ "counterUuid": u, "claims": entity::js_number(n) }))
            .collect::<Vec<_>>()
    )
}

async fn cs_claims(State(state): State<AppState>, q: Query) -> ApiResult<Reply> {
    Ok(reply(claims_json(
        coverage::claim_leaderboard(&state, "stats:cs_claims", claim_limit(&q)).await,
    )))
}

async fn ms_claims(State(state): State<AppState>, q: Query) -> ApiResult<Reply> {
    Ok(reply(claims_json(
        coverage::claim_leaderboard(&state, "stats:ms_claims", claim_limit(&q)).await,
    )))
}

fn num_param(s: &str) -> f64 {
    js_to_number(s)
}

async fn cs_hour(State(state): State<AppState>, Path(hour): Path<String>) -> ApiResult<Reply> {
    let h = num_param(&hour);
    let counts = coverage::cs_second_counts(&state, h)
        .await
        .ok_or_else(|| ApiError::bad_request("hour must be 0-23"))?;
    Ok(reply(json!({ "hour": entity::js_number(h), "seconds": counts })))
}

async fn ms_hour(State(state): State<AppState>, Path(hour): Path<String>) -> ApiResult<Reply> {
    let h = num_param(&hour);
    let counts = coverage::second_counts(&state, h)
        .await
        .ok_or_else(|| ApiError::bad_request("hour must be 0-23"))?;
    Ok(reply(json!({ "hour": entity::js_number(h), "seconds": counts })))
}

async fn bits(state: &AppState, hour: &str, second: &str, cs: bool) -> ApiResult<Reply> {
    let (h, s) = (num_param(hour), num_param(second));
    let res = if cs {
        coverage::cs_bits(state, h, s).await
    } else {
        coverage::ms_bits(state, h, s).await
    };
    let Some(r) = res else {
        return Err(ApiError::bad_request("hour must be 0-23, second 0-3599"));
    };
    Ok(reply(
        json!({ "hour": entity::js_number(h), "second": entity::js_number(s), "blob": r["blob"], "claimed": r["claimed"] }),
    ))
}

async fn cs_second(State(state): State<AppState>, Path((hour, second)): Path<(String, String)>) -> ApiResult<Reply> {
    bits(&state, &hour, &second, true).await
}

async fn ms_second(State(state): State<AppState>, Path((hour, second)): Path<(String, String)>) -> ApiResult<Reply> {
    bits(&state, &hour, &second, false).await
}

async fn owners_of(state: &AppState, hour: &str, second: &str, unit: Unit) -> ApiResult<Reply> {
    let Some(mut res) = owners::second_owners(state, num_param(hour), num_param(second), unit).await else {
        return Ok(reply(json!({ "available": false, "owners": [], "tally": [] })));
    };
    let uuids: Vec<String> = res["tally"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| t["counterUuid"].as_str().map(str::to_owned))
        .collect();
    let counters = db::find_counters(state, &uuids).await?;
    let tally: Vec<Value> = res["tally"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|mut t| {
            let c = counters.iter().find(|c| c["uuid"] == t["counterUuid"]);
            let name = c.map_or(Value::Null, |c| {
                if truthy(&c["name"]) {
                    c["name"].clone()
                } else {
                    c["username"].clone()
                }
            });
            let color = c.map_or(Value::Null, |c| {
                if truthy(&c["color"]) {
                    c["color"].clone()
                } else {
                    Value::Null
                }
            });
            t["name"] = name;
            t["color"] = color;
            t
        })
        .collect();
    let mut out = Map::new();
    out.insert("available".into(), json!(true));
    for (k, v) in res.as_object_mut().into_iter().flatten() {
        out.insert(k.clone(), v.clone());
    }
    out.insert("tally".into(), json!(tally));
    Ok(reply(Value::Object(out)))
}

async fn cs_owners(State(state): State<AppState>, Path((hour, second)): Path<(String, String)>) -> ApiResult<Reply> {
    owners_of(&state, &hour, &second, Unit::Cs).await
}

async fn ms_owners(State(state): State<AppState>, Path((hour, second)): Path<(String, String)>) -> ApiResult<Reply> {
    owners_of(&state, &hour, &second, Unit::Ms).await
}

async fn ms_counter(State(state): State<AppState>, Path(uuid): Path<String>) -> ApiResult<Reply> {
    let Some(c) = db::find_counter(&state, &uuid).await? else {
        return Ok(reply(json!({ "uuid": null, "claims": 0, "covered": 0, "blob": null })));
    };
    let u = c["uuid"].as_str().unwrap_or_default();
    let (claims, seconds, by_hour, cs) = tokio::join!(
        coverage::counter_claims(&state, "stats:ms_claims", u),
        coverage::counter_seconds(&state, u, false),
        coverage::claims_by_hour(&state, u),
        coverage::counter_claims(&state, "stats:cs_claims", u),
    );
    Ok(reply(json!({
        "claimsByHour": by_hour.map(|v| v.into_iter().map(entity::js_number).collect::<Vec<_>>()),
        "csClaims": entity::js_number(cs),
        "uuid": u,
        "claims": entity::js_number(claims),
        "covered": seconds["covered"],
        "blob": seconds.get("blob").cloned().unwrap_or(Value::Null),
    })))
}

async fn latest_counts(State(state): State<AppState>) -> ApiResult<Reply> {
    let v = state
        .socket_calls
        .call("latestCounts", json!({}), Duration::from_secs(10))
        .await
        .ok_or_else(|| ApiError::Internal(anyhow::anyhow!("socket did not answer latestCounts")))?;
    Ok(reply(v))
}

async fn visible_or_404(state: &AppState, name: &str, user: Option<&crate::session::SessionUser>) -> ApiResult<()> {
    let c = viewer(state, user).await?;
    let t = find_thread(state, name, c.as_ref(), user.is_some()).await?;
    if t.is_none() && name != "all" {
        return Err(bare(StatusCode::NOT_FOUND));
    }
    Ok(())
}

fn date_keys(stats: &Map<String, Value>) -> Vec<String> {
    let mut k: Vec<String> = stats.keys().filter(|k| is_day(k)).cloned().collect();
    k.sort();
    k
}

pub fn sum_day_stats(days: &[&Value]) -> Value {
    const MAPS: [&str; 6] = ["leaderboard", "gets", "assists", "palindromes", "repdigits", "time"];
    let mut maps: Vec<Map<String, Value>> = vec![Map::new(); 6];
    let mut speed = Vec::new();
    let mut split = Vec::new();
    let (mut sc, mut ssc) = (0f64, 0f64);
    let (mut last, mut last_uuid) = (0f64, Value::from(""));
    for day in days {
        if !day.is_object() {
            continue;
        }
        for (i, key) in MAPS.iter().enumerate() {
            for (id, n) in day[*key].as_object().into_iter().flatten() {
                let add = {
                    let x = js_number(n);
                    if x.is_nan() { 0.0 } else { x }
                };
                let cur = maps[i].get(id).and_then(Value::as_f64).unwrap_or(0.0);
                maps[i].insert(id.clone(), entity::js_number(cur + add));
            }
        }
        if let Some(a) = day["speed"].as_array() {
            speed.extend(a.iter().cloned());
        }
        if let Some(a) = day["splitSpeed"].as_array() {
            split.extend(a.iter().cloned());
        }
        sc += day
            .get("speedCount")
            .filter(|v| !v.is_null())
            .map_or_else(|| day["speed"].as_array().map_or(0.0, |a| a.len() as f64), js_number);
        ssc += day.get("splitSpeedCount").filter(|v| !v.is_null()).map_or_else(
            || day["splitSpeed"].as_array().map_or(0.0, |a| a.len() as f64),
            js_number,
        );
        let lu = day.get("last_updated").map_or(0.0, js_number);
        if lu > last {
            last = lu;
            last_uuid = day.get("last_updated_uuid").cloned().unwrap_or(json!(""));
        }
    }
    let mut out = Map::new();
    out.insert("last_updated".into(), entity::js_number(last));
    out.insert("last_updated_uuid".into(), last_uuid);
    for (i, key) in MAPS.iter().enumerate() {
        out.insert((*key).into(), Value::Object(std::mem::take(&mut maps[i])));
    }
    out.insert("speed".into(), json!(speed));
    out.insert("splitSpeed".into(), json!(split));
    out.insert("speedCount".into(), entity::js_number(sc));
    out.insert("splitSpeedCount".into(), entity::js_number(ssc));
    Value::Object(out)
}

fn summary_for_range(
    stats: &Map<String, Value>,
    date: Option<&str>,
    start: Option<&str>,
    end: Option<&str>,
) -> Map<String, Value> {
    let mut scoped = Map::new();
    if let Some(a) = stats.get("all").filter(|a| truthy(a)) {
        scoped.insert("all".into(), a.clone());
    }
    if let Some(d) = date {
        if let Some(v) = stats.get(d).filter(|v| truthy(v)) {
            scoped.insert(d.into(), v.clone());
        }
        return scoped;
    }
    if start.is_none() && end.is_none() {
        return scoped;
    }
    let keys = date_keys(stats);
    if keys.is_empty() {
        return scoped;
    }
    let (s, e) = effective(start, end, &keys);
    let (Some(s), Some(e)) = (s, e) else { return scoped };
    for k in keys {
        if k >= s && k <= e {
            scoped.insert(k.clone(), stats[&k].clone());
        }
    }
    scoped
}

fn effective(start: Option<&str>, end: Option<&str>, keys: &[String]) -> (Option<String>, Option<String>) {
    let mut s = start.map(str::to_owned);
    let mut e = end.map(str::to_owned);
    if s.is_none() && e.is_some() {
        s = keys.first().cloned();
    }
    if s.is_some() && e.is_none() {
        e = s.clone();
    }
    (s, e)
}

fn to_summary(stats: &Map<String, Value>) -> Map<String, Value> {
    stats
        .iter()
        .map(|(k, v)| {
            let Some(o) = v.as_object() else {
                return (k.clone(), v.clone());
            };
            let count = |arr: &str, n: &str| match &v[arr] {
                Value::Array(a) => json!(a.len()),
                _ => match &v[n] {
                    Value::Number(x) => Value::Number(x.clone()),
                    _ => json!(0),
                },
            };
            let mut s = o.clone();
            s.insert("speedCount".into(), count("speed", "speedCount"));
            s.insert("splitSpeedCount".into(), count("splitSpeed", "splitSpeedCount"));
            s.insert("speed".into(), json!([]));
            s.insert("splitSpeed".into(), json!([]));
            (k.clone(), Value::Object(s))
        })
        .collect()
}

fn opt(body: &JsonBody, k: &str) -> Option<String> {
    body.get(k).as_str().filter(|s| !s.is_empty()).map(str::to_owned)
}

async fn thread_stats(State(state): State<AppState>, MaybeUser(user): MaybeUser, body: JsonBody) -> ApiResult<Reply> {
    let name = js_string(body.get("thread"));
    visible_or_404(&state, &name, user.as_ref()).await?;
    let (date, start, end) = (
        opt(&body, "dateStr"),
        opt(&body, "startDateStr"),
        opt(&body, "endDateStr"),
    );
    let details = truthy(body.get("includeDetails"));
    let stats = store::load_thread_stats(
        &state,
        &name,
        ThreadStatsOpts {
            date: date.as_deref(),
            start: start.as_deref(),
            end: end.as_deref(),
            full: details,
            ..Default::default()
        },
    )
    .await;
    if !stats.get("all").is_some_and(truthy) {
        return Err(bare(StatusCode::NOT_FOUND));
    }
    let mut scoped = summary_for_range(&stats, date.as_deref(), start.as_deref(), end.as_deref());
    if date.is_some() || start.is_some() || end.is_some() {
        let days: Vec<&Value> = scoped.iter().filter(|(k, _)| is_day(k)).map(|(_, v)| v).collect();
        let summed = sum_day_stats(&days);
        scoped.insert("all".into(), summed);
    }
    let response = if details { scoped.clone() } else { to_summary(&scoped) };
    let lb = &stats["all"]["leaderboard"];
    if truthy(lb) {
        let mut uuids: Vec<String> = lb.as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default();
        for k in scoped
            .get("all")
            .and_then(|a| a["time"].as_object())
            .map(|o| o.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default()
        {
            if !uuids.contains(&k) {
                uuids.push(k);
            }
        }
        let counters = db::find_counters(&state, &uuids).await?;
        return Ok(reply(json!({ "stats": response, "counters": counters })));
    }
    Ok(reply(json!({ "stats": response, "counters": [] })))
}

async fn thread_rank_list(
    State(state): State<AppState>,
    MaybeUser(user): MaybeUser,
    body: JsonBody,
) -> ApiResult<Reply> {
    let name = js_string(body.get("thread"));
    let metric = body
        .str("metric")
        .filter(|m| store::RANK_METRICS.contains(m))
        .unwrap_or("counts")
        .to_owned();
    if name != "all" {
        let c = viewer(&state, user.as_ref()).await?;
        if find_thread(&state, &name, c.as_ref(), user.is_some()).await?.is_none() {
            return Err(bare(StatusCode::NOT_FOUND));
        }
    }
    let list = store::rank_list(&state, &name, &metric).await;
    let Some(entries) = list["entries"].as_array() else {
        return Ok(reply(
            json!({ "metric": metric, "total": 0, "rank": null, "rows": [], "counters": [] }),
        ));
    };
    let total = entries.len();
    let around = body.get("around");
    let rank = if around.is_null() {
        None
    } else {
        entries.iter().position(|e| e.get(0) == Some(around)).map(|i| i + 1)
    };
    let mut wanted: Vec<usize> = Vec::new();
    let num_or = |k: &str, d: f64| {
        let n = js_number(body.get(k));
        if n.is_nan() || n == 0.0 { d } else { n }
    };
    if !body.get("offset").is_null() && !body.get("limit").is_null() {
        let offset = num_or("offset", 0.0).floor().max(0.0) as usize;
        let limit = num_or("limit", 1.0).floor().clamp(1.0, 100.0) as usize;
        for i in offset..(offset + limit).min(total) {
            wanted.push(i);
        }
    } else {
        let top = if body.get("top").is_null() {
            5.0
        } else {
            js_number(body.get("top"))
        };
        let span = if body.get("span").is_null() {
            1.0
        } else {
            js_number(body.get("span"))
        };
        let top = if top.is_nan() {
            0
        } else {
            top.floor().clamp(0.0, 50.0) as usize
        };
        let span = if span.is_nan() {
            0
        } else {
            span.floor().clamp(0.0, 25.0) as i64
        };
        for i in 0..top.min(total) {
            wanted.push(i);
        }
        if let Some(r) = rank {
            for i in (r as i64 - 1 - span)..=(r as i64 - 1 + span) {
                if i >= 0 && (i as usize) < total && !wanted.contains(&(i as usize)) {
                    wanted.push(i as usize);
                }
            }
        }
    }
    wanted.sort_unstable();
    wanted.dedup();
    let rows: Vec<Value> = wanted
        .iter()
        .filter_map(|i| {
            let e = entries[*i].as_array()?;
            let counter = e.first()?.as_str()?;
            let value = e
                .get(1)
                .and_then(Value::as_f64)
                .filter(|f| f.is_finite())
                .map_or(json!(0), |_| e[1].clone());
            Some(json!({ "rank": i + 1, "counter": counter, "value": value }))
        })
        .collect();
    let uuids: Vec<String> = rows
        .iter()
        .filter_map(|r| r["counter"].as_str().map(str::to_owned))
        .collect();
    let counters = if uuids.is_empty() {
        Vec::new()
    } else {
        db::find_counters(&state, &uuids).await?
    };
    Ok(reply(
        json!({ "metric": metric, "total": total, "rank": rank, "rows": rows, "counters": counters }),
    ))
}

fn details_for_range(
    stats: &Map<String, Value>,
    kind: &str,
    date: Option<&str>,
    start: Option<&str>,
    end: Option<&str>,
) -> Vec<Value> {
    let get = |k: &str| {
        stats
            .get(k)
            .and_then(|d| d[kind].as_array())
            .cloned()
            .unwrap_or_default()
    };
    if let Some(d) = date {
        return get(d);
    }
    if start.is_none() && end.is_none() {
        return get("all");
    }
    let keys = date_keys(stats);
    if keys.is_empty() {
        return Vec::new();
    }
    let (s, e) = effective(start, end, &keys);
    let (Some(s), Some(e)) = (s, e) else { return Vec::new() };
    keys.iter()
        .filter(|k| **k >= s && **k <= e)
        .flat_map(|k| get(k))
        .collect()
}

fn time_of(r: &Value) -> f64 {
    let t = js_number(&r["time"]);
    if t.is_finite() { t } else { f64::INFINITY }
}

fn quantile(v: &[f64], q: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    let pos = (v.len() as f64 - 1.0) * q;
    let base = pos.floor() as usize;
    let rest = pos - base as f64;
    match v.get(base + 1) {
        Some(next) => v[base] + rest * (next - v[base]),
        None => v[base],
    }
}

fn distribution(records: &[&Value], custom: Option<f64>) -> Vec<Value> {
    let mut by: Vec<(String, Vec<f64>)> = Vec::new();
    for r in records {
        let t = js_number(&r["time"]);
        if !t.is_finite() {
            continue;
        }
        for u in r["qualifiedCounters"].as_array().into_iter().flatten() {
            let u = js_string(u);
            match by.iter_mut().find(|x| x.0 == u) {
                Some(s) => s.1.push(t),
                None => by.push((u, vec![t])),
            }
        }
    }
    let mut out: Vec<(f64, Value)> = by
        .into_iter()
        .map(|(uuid, mut times)| {
            times.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let p75 = quantile(&times, 0.75);
            let median = quantile(&times, 0.5);
            let n = entity::js_number;
            let mut o = json!({
                "uuid": uuid, "attempts": times.len(), "min": n(times[0]), "q1": n(quantile(&times, 0.25)),
                "median": n(median), "q3": n(p75), "p99": n(quantile(&times, 0.99)), "max": n(times[times.len() - 1]),
                "plotMax": n(p75),
            });
            if let Some(f) = custom {
                o["custom"] = n(quantile(&times, f));
            }
            (median, o)
        })
        .collect();
    out.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    out.into_iter().map(|x| x.1).collect()
}

fn hall_of_speed(records: &[&Value]) -> Vec<Value> {
    let mut best: Vec<(String, &Value)> = Vec::new();
    for r in records {
        let t = js_number(&r["time"]);
        if !t.is_finite() {
            continue;
        }
        for u in r["qualifiedCounters"].as_array().into_iter().flatten() {
            let u = js_string(u);
            match best.iter_mut().find(|x| x.0 == u) {
                Some(slot) => {
                    if t < js_number(&slot.1["time"]) {
                        slot.1 = r;
                    }
                }
                None => best.push((u, r)),
            }
        }
    }
    best.sort_by(|a, b| {
        js_number(&a.1["time"])
            .partial_cmp(&js_number(&b.1["time"]))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut prev: Option<f64> = None;
    let mut prev_rank = 0usize;
    best.into_iter()
        .enumerate()
        .map(|(i, (counter, obj))| {
            let t = js_number(&obj["time"]);
            let r3 = |x: f64| (x * 1000.0).round() / 1000.0;
            let rank = match prev {
                Some(p) if t.is_finite() && p.is_finite() && r3(t) == r3(p) => prev_rank,
                _ => i + 1,
            };
            prev = Some(t);
            prev_rank = rank;
            json!({ "counter": counter, "obj": obj, "rank": rank })
        })
        .collect()
}

fn compose_split_pages(real: &[&Value], fake: &[&Value], per: usize) -> Vec<Value> {
    let mut out = Vec::new();
    let mut off = 0;
    loop {
        let r: Vec<Value> = real.iter().skip(off).take(per).map(|v| (*v).clone()).collect();
        let f: Vec<Value> = fake.iter().skip(off).take(per).map(|v| (*v).clone()).collect();
        if r.is_empty() && f.is_empty() {
            break;
        }
        out.extend(r);
        out.extend(f);
        off += per;
    }
    out
}

async fn resolve_posts(state: &AppState, thread: &str, numbers: &[i64]) -> ApiResult<HashMap<i64, (String, Value)>> {
    let mut out = HashMap::new();
    if numbers.is_empty() {
        return Ok(out);
    }
    let Some(t) = db::find_thread_by_name(state, thread).await? else {
        return Ok(out);
    };
    for chunk in numbers.chunks(500) {
        let mut args = vec![t["id"].clone()];
        args.extend(chunk.iter().map(|n| json!(n)));
        let rows = db::rows(
            state,
            &format!(
                "SELECT validCountNumber, uuid, rawCount FROM post WHERE threadId = ? AND isValidCount = 1 AND validCountNumber IN ({})",
                db::placeholders(chunk.len())
            ),
            &args,
        )
        .await?;
        for r in &rows {
            let j = entity::row_json(r);
            out.insert(
                js_number(&j["validCountNumber"]) as i64,
                (js_string(&j["uuid"]), j["rawCount"].clone()),
            );
        }
    }
    Ok(out)
}

fn count_numbers(records: &[&Value]) -> Vec<i64> {
    let mut set: Vec<i64> = Vec::new();
    for r in records {
        for k in ["startCountNumber", "endCountNumber"] {
            let n = js_number(&r[k]);
            if n.is_finite() && !set.contains(&(n as i64)) {
                set.push(n as i64);
            }
        }
    }
    set
}

fn rehydrate(r: &Value, posts: &HashMap<i64, (String, Value)>) -> Value {
    let mut o = r.clone();
    if let Some((uuid, raw)) = posts.get(&(js_number(&r["startCountNumber"]) as i64)) {
        o["start"] = json!(uuid);
        o["startCount"] = raw.clone();
    }
    if let Some((uuid, raw)) = posts.get(&(js_number(&r["endCountNumber"]) as i64)) {
        o["end"] = json!(uuid);
        o["endCount"] = raw.clone();
    }
    o
}

async fn thread_stats_details(
    State(state): State<AppState>,
    MaybeUser(user): MaybeUser,
    body: JsonBody,
) -> ApiResult<Reply> {
    let name = js_string(body.get("thread"));
    let sort = body
        .str("sortOrder")
        .filter(|s| matches!(*s, "slowest" | "newest" | "oldest"))
        .unwrap_or("fastest");
    visible_or_404(&state, &name, user.as_ref()).await?;
    let kind = body.str("type").unwrap_or("").to_owned();
    if kind != "speed" && kind != "splitSpeed" {
        return Err(bare(StatusCode::BAD_REQUEST));
    }
    let (date, start, end) = (
        opt(&body, "dateStr"),
        opt(&body, "startDateStr"),
        opt(&body, "endDateStr"),
    );
    let stats = store::load_thread_stats(
        &state,
        &name,
        ThreadStatsOpts {
            date: date.as_deref(),
            start: start.as_deref(),
            end: end.as_deref(),
            full: true,
            all_days: false,
            include_rollup: date.is_none() && start.is_none() && end.is_none(),
        },
    )
    .await;
    let records = details_for_range(&stats, &kind, date.as_deref(), start.as_deref(), end.as_deref());
    let offset = {
        let n = js_number(body.get("offset"));
        if n.is_nan() { 0.0 } else { n.max(0.0) }
    } as usize;
    let selected: Vec<String> = body
        .get("selectedUserUUIDs")
        .as_array()
        .map(|a| a.iter().filter_map(|u| u.as_str().map(str::to_owned)).collect())
        .unwrap_or_default();
    let has_sel = !selected.is_empty();
    let mut sorted: Vec<&Value> = records.iter().collect();
    sorted.sort_by(|a, b| time_of(a).partial_cmp(&time_of(b)).unwrap_or(std::cmp::Ordering::Equal));
    let filtered: Vec<&Value> = if has_sel {
        sorted
            .iter()
            .copied()
            .filter(|r| {
                r["qualifiedCounters"].as_array().is_some_and(|q| {
                    q.iter()
                        .any(|u| u.as_str().is_some_and(|u| selected.iter().any(|s| s == u)))
                })
            })
            .collect()
    } else {
        sorted.clone()
    };
    let base: &[&Value] = &filtered;
    let real: Vec<&Value> = base.iter().copied().filter(|r| !truthy(&r["isFake"])).collect();
    let fake: Vec<&Value> = base.iter().copied().filter(|r| truthy(&r["isFake"])).collect();
    let custom = body
        .get("customPercentile")
        .as_f64()
        .filter(|f| f.is_finite())
        .map(|f| f.clamp(0.0, 100.0) / 100.0);
    let dist = distribution(base, custom);
    let dist_real = distribution(&real, custom);
    let dist_fake = distribution(&fake, custom);
    let hall = hall_of_speed(base);
    let hall_real = hall_of_speed(&real);
    let hall_fake = hall_of_speed(&fake);
    let mut composed: Vec<Value> = if has_sel && kind == "splitSpeed" {
        compose_split_pages(&real, &fake, 50)
    } else {
        filtered.iter().map(|v| (*v).clone()).collect()
    };
    match sort {
        "slowest" => composed.reverse(),
        "oldest" | "newest" => {
            let n = |r: &Value| {
                let x = js_number(&r["startCountNumber"]);
                if r["startCountNumber"].is_null() { 0.0 } else { x }
            };
            composed.sort_by(|a, b| {
                let d = if sort == "oldest" { n(a) - n(b) } else { n(b) - n(a) };
                d.partial_cmp(&0.0).unwrap_or(std::cmp::Ordering::Equal)
            });
        }
        _ => {}
    }
    let fallback = if has_sel && kind == "splitSpeed" { 100.0 } else { 50.0 };
    let limit = {
        let l = js_number(body.get("limit"));
        if l.is_nan() || l == 0.0 { fallback } else { l }
    }
    .clamp(1.0, 5000.0) as usize;
    let paged: Vec<Value> = composed.iter().skip(offset).take(limit).cloned().collect();
    let has_more = offset + limit < composed.len();
    let mut need: Vec<&Value> = paged.iter().collect();
    for h in hall.iter().chain(&hall_real).chain(&hall_fake) {
        if truthy(&h["obj"]) {
            need.push(&h["obj"]);
        }
    }
    let posts = resolve_posts(&state, &name, &count_numbers(&need)).await?;
    let paged: Vec<Value> = paged.iter().map(|r| rehydrate(r, &posts)).collect();
    let hall_fix = |rows: &[Value]| -> Vec<Value> {
        rows.iter()
            .map(|r| {
                let mut o = r.clone();
                if truthy(&r["obj"]) {
                    o["obj"] = rehydrate(&r["obj"], &posts);
                }
                o
            })
            .collect()
    };
    let mut uuids: Vec<String> = Vec::new();
    for r in &paged {
        for u in r["qualifiedCounters"].as_array().into_iter().flatten() {
            if let Some(u) = u.as_str()
                && !uuids.iter().any(|x| x == u)
            {
                uuids.push(u.to_owned());
            }
        }
    }
    for d in &dist {
        if let Some(u) = d["uuid"].as_str().filter(|u| !u.is_empty())
            && !uuids.iter().any(|x| x == u)
        {
            uuids.push(u.to_owned());
        }
    }
    let counters = if uuids.is_empty() {
        Vec::new()
    } else {
        db::find_counters(&state, &uuids).await?
    };
    Ok(reply(json!({
        "records": paged, "counters": counters, "offset": offset, "limit": limit, "total": composed.len(), "hasMore": has_more,
        "distributionStats": dist, "distributionStatsRealOnly": dist_real, "distributionStatsFakeOnly": dist_fake,
        "hallOfSpeed": hall_fix(&hall), "hallOfSpeedRealOnly": hall_fix(&hall_real), "hallOfSpeedFakeOnly": hall_fix(&hall_fake),
    })))
}

fn parse_day(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
}

async fn thread_graph(State(state): State<AppState>, MaybeUser(user): MaybeUser, body: JsonBody) -> ApiResult<Reply> {
    let name = js_string(body.get("thread"));
    visible_or_404(&state, &name, user.as_ref()).await?;
    let (date, start, end) = (
        opt(&body, "dateStr"),
        opt(&body, "startDateStr"),
        opt(&body, "endDateStr"),
    );
    let stats = store::load_thread_stats(
        &state,
        &name,
        ThreadStatsOpts {
            date: date.as_deref(),
            start: start.as_deref(),
            end: end.as_deref(),
            full: false,
            all_days: true,
            include_rollup: true,
        },
    )
    .await;
    let all_lb = stats
        .get("all")
        .map(|a| a["leaderboard"].clone())
        .filter(Value::is_object)
        .unwrap_or(json!({}));
    let selected: Vec<String> = body
        .get("selectedUserUUIDs")
        .as_array()
        .map(|a| a.iter().filter_map(|u| u.as_str().map(str::to_owned)).collect())
        .unwrap_or_default();
    let top_n = body
        .get("topN")
        .as_f64()
        .filter(|f| f.is_finite())
        .map_or(0, |f| f.floor().clamp(0.0, 50.0) as usize);
    let keys = date_keys(&stats);
    if keys.is_empty() {
        return Ok(reply(json!({ "allLeaderboard": all_lb, "points": [], "topUUIDs": [] })));
    }
    let (s, e) = match &date {
        Some(d) => (Some(d.clone()), Some(d.clone())),
        None if start.is_none() && end.is_none() => (keys.first().cloned(), keys.last().cloned()),
        None => effective(start.as_deref(), end.as_deref(), &keys),
    };
    let (Some(s), Some(e)) = (s, e) else {
        return Ok(reply(json!({ "allLeaderboard": all_lb, "points": [], "topUUIDs": [] })));
    };
    let mut sel: Vec<String> = selected;
    let mut top: Vec<String> = Vec::new();
    if top_n > 0 {
        let mut totals: Vec<(String, f64)> = Vec::new();
        for d in keys.iter().filter(|d| **d >= s && **d <= e) {
            for (u, c) in stats[d]["leaderboard"].as_object().into_iter().flatten() {
                let n = {
                    let x = js_number(c);
                    if x.is_nan() { 0.0 } else { x }
                };
                match totals.iter_mut().find(|t| t.0 == *u) {
                    Some(t) => t.1 += n,
                    None => totals.push((u.clone(), n)),
                }
            }
        }
        totals.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        for (u, _) in totals.into_iter().take(top_n) {
            if !sel.contains(&u) {
                sel.push(u.clone());
            }
            top.push(u);
        }
    }
    let mut seen = HashSet::new();
    sel.retain(|u| seen.insert(u.clone()));
    let mut points = Vec::new();
    let (mut cumulative, mut other_cum) = (0f64, 0f64);
    let mut user_cum: HashMap<String, f64> = HashMap::new();
    if let (Some(mut cur), Some(last)) = (parse_day(&s), parse_day(&e)) {
        while cur <= last {
            let key = cur.format("%Y-%m-%d").to_string();
            let lb = stats
                .get(&key)
                .map(|d| d["leaderboard"].clone())
                .filter(Value::is_object)
                .unwrap_or(json!({}));
            let num = |v: &Value| {
                let x = js_number(v);
                if x.is_nan() { 0.0 } else { x }
            };
            let daily: f64 = lb.as_object().into_iter().flatten().map(|(_, v)| num(v)).sum();
            cumulative += daily;
            let mut p = Map::new();
            p.insert("date".into(), json!(key));
            p.insert("daily".into(), entity::js_number(daily));
            p.insert("cumulative".into(), entity::js_number(cumulative));
            for u in &sel {
                let d = num(&lb[u.as_str()]);
                let c = user_cum.entry(u.clone()).or_insert(0.0);
                *c += d;
                p.insert(u.clone(), entity::js_number(d));
                p.insert(format!("{u}_cum"), entity::js_number(*c));
            }
            if top_n > 0 {
                let top_daily: f64 = top.iter().map(|u| num(&lb[u.as_str()])).sum();
                let other = daily - top_daily;
                other_cum += other;
                p.insert("other".into(), entity::js_number(other));
                p.insert("other_cum".into(), entity::js_number(other_cum));
            }
            points.push(Value::Object(p));
            cur += ChronoDuration::days(1);
        }
    }
    Ok(reply(
        json!({ "allLeaderboard": all_lb, "points": points, "topUUIDs": top }),
    ))
}

fn eastern_bounds(day: &str, end_of_day: bool) -> Option<i64> {
    let d = parse_day(day)?;
    let base = New_York.from_local_datetime(&d.and_hms_opt(0, 0, 0)?).earliest()?;
    if end_of_day {
        let next = New_York
            .from_local_datetime(&(d + ChronoDuration::days(1)).and_hms_opt(0, 0, 0)?)
            .earliest()?;
        Some(next.timestamp_millis() - 1)
    } else {
        Some(base.timestamp_millis())
    }
}

pub fn rob_window(start: Option<&str>, end: Option<&str>) -> Option<(Option<i64>, Option<i64>)> {
    let s = start.filter(|d| is_day(d));
    let e = end.filter(|d| is_day(d)).or(s);
    if s.is_none() && e.is_none() {
        return None;
    }
    Some((
        s.and_then(|s| eastern_bounds(s, false)),
        e.and_then(|e| eastern_bounds(e, true)),
    ))
}

async fn all_robs(state: &AppState) -> ApiResult<Vec<(i64, String, f64, f64, String)>> {
    let rows: Vec<(i32, String, Option<String>, Option<i64>, Option<String>)> =
        sqlx::query_as("SELECT id, counterUUID, postUUID, moneyRobbed, timestamp FROM rob ORDER BY id ASC")
            .fetch_all(&state.pool)
            .await?;
    Ok(rows
        .into_iter()
        .map(|(id, c, p, m, t)| {
            let ts = t.as_deref().map(js_to_number).filter(|x| !x.is_nan()).unwrap_or(0.0);
            (i64::from(id), c, m.unwrap_or(0) as f64, ts, p.unwrap_or_default())
        })
        .collect())
}

async fn rob_stats(State(state): State<AppState>, body: JsonBody) -> ApiResult<Reply> {
    let n = |k: &str, d: f64| {
        let x = js_number(body.get(k));
        if x.is_nan() || x == 0.0 { d } else { x }
    };
    let size = n("pageSize", 25.0).floor().clamp(1.0, 100.0) as usize;
    let page = n("page", 1.0).floor().max(1.0) as usize;
    let window = rob_window(body.str("startDateStr"), body.str("endDateStr"));
    let robs: Vec<_> = all_robs(&state)
        .await?
        .into_iter()
        .filter(|r| match window {
            Some((from, to)) => {
                r.3 >= from.map_or(f64::NEG_INFINITY, |f| f as f64) && r.3 <= to.map_or(f64::INFINITY, |t| t as f64)
            }
            None => true,
        })
        .collect();
    let mut by: Vec<(String, i64, f64, f64, f64)> = Vec::new();
    for r in &robs {
        match by.iter_mut().find(|x| x.0 == r.1) {
            Some(s) => {
                s.1 += 1;
                s.2 += r.2;
                s.3 = s.3.max(r.2);
                s.4 = s.4.max(r.3);
            }
            None => by.push((r.1.clone(), 1, r.2, r.2.max(0.0), r.3.max(0.0))),
        }
    }
    by.sort_by(|a, b| {
        b.2.partial_cmp(&a.2)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.1.cmp(&a.1))
    });
    let total = robs.len();
    let start = total as i64 - (page * size) as i64;
    let end = start + size as i64;
    let (s, e) = (start.max(0) as usize, (end.max(0) as usize).min(total));
    let entries: Vec<Value> = robs[s.min(e)..e]
        .iter()
        .rev()
        .map(|r| json!({ "id": r.0, "counterUUID": r.1, "moneyRobbed": entity::js_number(r.2), "timestamp": entity::js_number(r.3), "postUUID": r.4 }))
        .collect();
    let leaderboard: Vec<Value> = by
        .iter()
        .map(|x| json!({ "counterUUID": x.0, "robs": x.1, "total": entity::js_number(x.2), "biggest": entity::js_number(x.3), "lastAt": entity::js_number(x.4) }))
        .collect();
    let mut uuids: Vec<String> = by.iter().map(|x| x.0.clone()).collect();
    for e in &entries {
        let u = js_string(&e["counterUUID"]);
        if !uuids.contains(&u) {
            uuids.push(u);
        }
    }
    let counters = if uuids.is_empty() {
        Vec::new()
    } else {
        db::find_counters(&state, &uuids).await?
    };
    Ok(reply(
        json!({ "leaderboard": leaderboard, "total": total, "page": page, "pageSize": size, "entries": entries, "counters": counters }),
    ))
}

async fn daily_count_leaderboard(
    State(state): State<AppState>,
    MaybeUser(user): MaybeUser,
    body: JsonBody,
) -> ApiResult<Reply> {
    let name = js_string(body.get("thread"));
    visible_or_404(&state, &name, user.as_ref()).await?;
    let (start, end) = (opt(&body, "startDateStr"), opt(&body, "endDateStr"));
    let stats = store::load_thread_stats(
        &state,
        &name,
        ThreadStatsOpts {
            start: start.as_deref(),
            end: end.as_deref(),
            full: false,
            all_days: true,
            ..Default::default()
        },
    )
    .await;
    let mut by: Vec<(String, Vec<(String, f64)>)> = Vec::new();
    for day in stats.keys().filter(|k| is_day(k)) {
        if start.as_deref().is_some_and(|s| day.as_str() < s) || end.as_deref().is_some_and(|e| day.as_str() > e) {
            continue;
        }
        for (u, c) in stats[day]["leaderboard"].as_object().into_iter().flatten() {
            let n = js_number(c);
            if !n.is_finite() {
                continue;
            }
            match by.iter_mut().find(|x| x.0 == *u) {
                Some(s) => s.1.push((day.clone(), n)),
                None => by.push((u.clone(), vec![(day.clone(), n)])),
            }
        }
    }
    let frac = body
        .get("customPercentile")
        .as_f64()
        .filter(|f| f.is_finite())
        .map_or(1.0, |f| f.clamp(0.0, 100.0) / 100.0);
    let mut entries: Vec<(f64, Value)> = by
        .into_iter()
        .map(|(uuid, mut days)| {
            days.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
            let idx = ((days.len() as f64 - 1.0) * frac + 0.5).floor() as usize;
            let (day, count) = days[idx.min(days.len() - 1)].clone();
            (
                count,
                json!({ "uuid": uuid, "bestDay": day, "count": entity::js_number(count), "daysCount": days.len() }),
            )
        })
        .collect();
    entries.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let entries: Vec<Value> = entries.into_iter().map(|x| x.1).collect();
    let uuids: Vec<String> = entries.iter().map(|e| js_string(&e["uuid"])).collect();
    let counters = if uuids.is_empty() {
        Vec::new()
    } else {
        db::find_counters(&state, &uuids).await?
    };
    Ok(reply(json!({ "entries": entries, "counters": counters })))
}

fn split_permission(c: Option<&Value>) -> &'static str {
    let Some(c) = c else { return "none" };
    if ["speed_verifier", "mod", "admin"].iter().any(|r| has_role(c, r)) {
        return "any";
    }
    if has_role(c, "banned") || has_role(c, "muted") {
        return "none";
    }
    if has_role(c, "counter") { "own" } else { "none" }
}

async fn boundary(state: &AppState, start: &str, end: &str) -> ApiResult<Option<(Value, Value)>> {
    let one = |u: String| {
        let s = state.clone();
        async move {
            db::rows(
                &s,
                "SELECT uuid, validCountNumber, timestamp FROM post WHERE uuid = ? LIMIT 1",
                &[json!(u)],
            )
            .await
        }
    };
    let (a, b) = tokio::join!(one(start.to_owned()), one(end.to_owned()));
    let (a, b) = (a?, b?);
    Ok(match (a.first(), b.first()) {
        (Some(a), Some(b)) => Some((entity::row_json(a), entity::row_json(b))),
        _ => None,
    })
}

async fn mark_split_fake(State(state): State<AppState>, Authed(user): Authed, body: JsonBody) -> ApiResult<Reply> {
    let (thread, start, end) = (body.str("thread"), body.str("start"), body.str("end"));
    let (Some(thread), Some(start), Some(end), Some(is_fake)) = (
        thread.filter(|s| !s.is_empty()),
        start.filter(|s| !s.is_empty()),
        end.filter(|s| !s.is_empty()),
        body.get("isFake").as_bool(),
    ) else {
        return Err(bare(StatusCode::BAD_REQUEST));
    };
    let c = db::find_counter(&state, &user.uuid).await?;
    let perm = split_permission(c.as_ref());
    if perm == "none" {
        return Err(bare(StatusCode::FORBIDDEN));
    }
    let c = c.unwrap_or(Value::Null);
    let Some((s, e)) = boundary(&state, start, end).await? else {
        return Err(bare(StatusCode::NOT_FOUND));
    };
    let (sn, en) = (js_number(&s["validCountNumber"]), js_number(&e["validCountNumber"]));
    if perm == "own" {
        if is_fake {
            let day = js_to_number(&js_string(&e["timestamp"]));
            let key = chrono::DateTime::from_timestamp_millis(day as i64)
                .map(|t| t.with_timezone(&New_York).format("%Y-%m-%d").to_string())
                .unwrap_or_default();
            let find = |v: &Value| -> Option<Value> {
                v.as_array()?
                    .iter()
                    .find(|r| js_number(&r["startCountNumber"]) == sn && js_number(&r["endCountNumber"]) == en)
                    .cloned()
            };
            let day_stats = store::load_thread_stats(
                &state,
                thread,
                ThreadStatsOpts {
                    date: Some(&key),
                    full: true,
                    include_rollup: false,
                    ..Default::default()
                },
            )
            .await;
            let mut rec = day_stats.get(&key).and_then(|d| find(&d["splitSpeed"]));
            if rec.is_none() {
                let all = store::load_thread_stats(
                    &state,
                    thread,
                    ThreadStatsOpts {
                        full: true,
                        ..Default::default()
                    },
                )
                .await;
                rec = all.get("all").and_then(|d| find(&d["splitSpeed"]));
            }
            let Some(rec) = rec else {
                return Err(bare(StatusCode::NOT_FOUND));
            };
            if !rec["qualifiedCounters"]
                .as_array()
                .is_some_and(|q| q.iter().any(|u| *u == c["uuid"]))
            {
                return Err(bare(StatusCode::FORBIDDEN));
            }
        } else {
            let m = store::split_fake_marker(&state, thread, sn, en).await;
            if !(m["isFake"] == Value::Bool(true) && m["markedBy"] == c["uuid"]) {
                return Err(bare(StatusCode::FORBIDDEN));
            }
        }
    }
    store::set_split_fake(&state, thread, sn, en, is_fake, c["uuid"].as_str())
        .await
        .map_err(|_| bare(StatusCode::NOT_FOUND))?;
    Ok(reply(json!({ "updated": true })))
}

async fn thread_id_of(state: &AppState, uuid: &str) -> ApiResult<i64> {
    db::find_thread_by_uuid(state, uuid)
        .await?
        .and_then(|t| t["id"].as_i64())
        .ok_or_else(|| ApiError::not_found(format!("Unknown thread uuid {uuid}")))
}

async fn counter_id_of(state: &AppState, uuid: &str) -> ApiResult<i64> {
    db::find_counter(state, uuid)
        .await?
        .and_then(|c| c["id"].as_i64())
        .ok_or_else(|| ApiError::not_found(format!("Unknown counter uuid {uuid}")))
}

fn posts_of(rows: &[sqlx::mysql::MySqlRow]) -> Vec<Value> {
    rows.iter().map(|r| entity::POST.json(r)).collect()
}

async fn authors(state: &AppState, posts: &[Value]) -> ApiResult<Vec<Value>> {
    let mut u: Vec<String> = Vec::new();
    for p in posts {
        let a = js_string(&p["authorUUID"]);
        if !u.contains(&a) {
            u.push(a);
        }
    }
    db::find_counters(state, &u).await
}

async fn load_window(state: &AppState, q: &Query, older: bool) -> ApiResult<Reply> {
    let thread_name = q.str("thread_name").filter(|s| !s.is_empty());
    let post_uuid = q.str("uuid").filter(|s| !s.is_empty());
    let author = q.str("author_uuid").filter(|s| !s.is_empty());
    let lim = counting::js::parse_int(q.str("limit").unwrap_or(""), Some(10));
    let take = if lim.is_finite() {
        lim.clamp(1.0, 1000.0) as i64
    } else {
        50
    };
    let comments = q.str("commentsOnly").is_some_and(|s| s.to_lowercase() == "true");
    let valid = q.str("validCountsOnly").is_some_and(|s| s.to_lowercase() == "true");
    let mut thread_id = None;
    let mut thread_uuid = None;
    if let Some(n) = thread_name {
        let Some(t) = db::find_thread_by_name(state, n).await? else {
            return Err(ApiError::bad_request(format!("Thread {n} not found.")));
        };
        thread_uuid = t["uuid"].as_str().map(str::to_owned);
        thread_id = t["id"].as_i64();
    }
    let mut anchor = None;
    if let Some(p) = post_uuid {
        let rows = match thread_id {
            Some(tid) => {
                db::rows(
                    state,
                    "SELECT id FROM post WHERE uuid = ? AND threadId = ? LIMIT 1",
                    &[json!(p), json!(tid)],
                )
                .await?
            }
            None => db::rows(state, "SELECT id FROM post WHERE uuid = ? LIMIT 1", &[json!(p)]).await?,
        };
        let Some(r) = rows.first() else {
            let suffix = thread_uuid
                .as_ref()
                .map_or_else(|| "undefined".to_owned(), |t| format!("in thread {t}"));
            return Err(ApiError::bad_request(format!("Post {p} not found {suffix}")));
        };
        anchor = Some(js_number(&entity::row_json(r)["id"]) as i64);
    }
    let index = if comments {
        "n_thread_3char_id"
    } else if valid {
        "n_thread_valid_id"
    } else {
        "n_thread_id"
    };
    let mut sql = if thread_id.is_some() {
        format!("SELECT * FROM post USE INDEX ({index}) WHERE 1=1")
    } else {
        "SELECT * FROM post WHERE 1=1".to_owned()
    };
    let mut args: Vec<Value> = Vec::new();
    if let Some(t) = thread_id {
        sql.push_str(" AND threadId = ?");
        args.push(json!(t));
    }
    if let Some(a) = author {
        sql.push_str(" AND authorId = ?");
        args.push(json!(counter_id_of(state, a).await?));
    }
    if let Some(id) = anchor {
        sql.push_str(if older { " AND id < ?" } else { " AND id > ?" });
        args.push(json!(id));
    }
    if comments {
        sql.push_str(" AND hasThreeCharComment = true");
    }
    if valid {
        sql.push_str(" AND isValidCount = true");
    }
    sql.push_str(&format!(
        " ORDER BY id {} LIMIT {}",
        if older { "DESC" } else { "ASC" },
        take + 1
    ));
    let mut posts = posts_of(&db::rows(state, &sql, &args).await?);
    let at_end = posts.len() as i64 <= take;
    if posts.len() as i64 > take {
        posts.pop();
    }
    let counts: Vec<Value> = posts.into_iter().filter(|p| p["uuid"].as_str() != post_uuid).collect();
    let counters = authors(state, &counts).await?;
    let flag = if older { "loadedOldest" } else { "loadedNewest" };
    Ok(reply(json!({ "counts": counts, "counters": counters, flag: at_end })))
}

async fn load_older(State(state): State<AppState>, q: Query) -> ApiResult<Reply> {
    load_window(&state, &q, true).await
}

async fn load_newer(State(state): State<AppState>, q: Query) -> ApiResult<Reply> {
    load_window(&state, &q, false).await
}

async fn robs(State(state): State<AppState>) -> ApiResult<Reply> {
    let rows = sqlx::query("SELECT * FROM rob ORDER BY id DESC")
        .fetch_all(&state.pool)
        .await?;
    Ok(reply(entity::ROB.rows(&rows)))
}

async fn blog_rows(state: &AppState, where_sql: &str, args: &[Value], tail: &str) -> ApiResult<Vec<Value>> {
    let rows = db::rows(state, &format!("SELECT * FROM blog {where_sql} {tail}"), args).await?;
    let mut out = Vec::new();
    for r in &rows {
        let mut b = entity::BLOG.json(r);
        let author: Option<String> = sqlx::Row::try_get(r, "authorUuid").ok().flatten();
        let id = b["uuid"].clone();
        let likes = db::rows(state, "SELECT * FROM bloglike WHERE blogUuid = ?", &[id]).await?;
        b["likes"] = entity::BLOG_LIKE.rows(&likes);
        b["author"] = match author {
            Some(a) => db::find_counter(state, &a).await?.unwrap_or(Value::Null),
            None => Value::Null,
        };
        out.push(b);
    }
    Ok(out)
}

async fn blogs(State(state): State<AppState>, Path(page): Path<String>) -> ApiResult<Reply> {
    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM blog WHERE hidden = 0")
        .fetch_one(&state.pool)
        .await?;
    let p = js_to_number(&page);
    let offset = ((p - 1.0) * 50.0) as i64;
    let list = if p.is_finite() {
        blog_rows(
            &state,
            "WHERE hidden = 0",
            &[],
            &format!("ORDER BY id DESC LIMIT 50 OFFSET {}", offset.max(0)),
        )
        .await?
    } else {
        blog_rows(&state, "WHERE hidden = 0", &[], "ORDER BY id DESC LIMIT 50").await?
    };
    Ok(reply(
        json!({ "blogs": list, "pageCount": (count as f64 / 50.0).ceil() as i64 }),
    ))
}

async fn all_blogs(State(state): State<AppState>) -> ApiResult<Reply> {
    Ok(reply(
        json!({ "blogs": blog_rows(&state, "", &[], "ORDER BY id DESC").await? }),
    ))
}

async fn blog(State(state): State<AppState>, Path(key): Path<String>) -> ApiResult<Reply> {
    let as_id = js_to_number(&key);
    let rows = if as_id.fract() == 0.0 && as_id > 0.0 {
        blog_rows(&state, "WHERE id = ?", &[json!(as_id as i64)], "LIMIT 1").await?
    } else if !key.is_empty() {
        blog_rows(&state, "WHERE uuid = ?", &[json!(key)], "LIMIT 1").await?
    } else {
        Vec::new()
    };
    rows.into_iter()
        .next()
        .map(reply)
        .ok_or_else(|| bare(StatusCode::NOT_FOUND))
}

async fn blog_reaction(State(state): State<AppState>, Authed(user): Authed, body: JsonBody) -> ApiResult<Reply> {
    let blog_uuid = body.get("blog_uuid");
    let Some(emote) = body.str("id").filter(|e| !e.is_empty()) else {
        return Err(ApiError::bad_request("blog_uuid and id are required."));
    };
    if !truthy(blog_uuid) {
        return Err(ApiError::bad_request("blog_uuid and id are required."));
    }
    let ok = db::find_counter(&state, &user.uuid)
        .await?
        .is_some_and(|c| has_role(&c, "counter") && !has_role(&c, "banned") && !has_role(&c, "muted"));
    if !ok {
        return Err(ApiError::forbidden("You cannot react."));
    }
    let known = cgg_web::reactions::builtin(emote)
        || sqlx::query("SELECT id FROM emote WHERE name = ? AND approved = 1 LIMIT 1")
            .bind(emote.to_lowercase())
            .fetch_optional(&state.pool)
            .await?
            .is_some();
    if !known {
        return Err(ApiError::bad_request(format!("Unknown emote \"{emote}\".")));
    }
    let _turn = state.jobs.lock().await;
    let bu = js_string(blog_uuid);
    let Some(b) = db::one(&state, &entity::BLOG, "SELECT * FROM blog WHERE uuid = ? LIMIT 1", &bu).await? else {
        return Err(ApiError::not_found(format!("Blog {bu} not found.")));
    };
    let mut reactions = b["reactions"].as_object().cloned().unwrap_or_default();
    match reactions.get_mut(emote).and_then(Value::as_array_mut) {
        None => {
            if reactions.len() >= 11 {
                return Err(ApiError::bad_request("This post already has 11 different reactions."));
            }
            reactions.insert(emote.to_owned(), json!([user.uuid]));
        }
        Some(list) => match list.iter().position(|u| *u == user.uuid) {
            Some(i) => {
                list.remove(i);
                if list.is_empty() {
                    reactions.remove(emote);
                }
            }
            None => list.push(json!(user.uuid)),
        },
    }
    db::exec(
        &state,
        "UPDATE blog SET reactions = ? WHERE uuid = ?",
        &[json!(Value::Object(reactions.clone()).to_string()), json!(bu)],
    )
    .await?;
    Ok(reply(json!({ "reactions": reactions })))
}

async fn shuffle(State(state): State<AppState>, Path(seed): Path<String>) -> ApiResult<Reply> {
    if seed.is_empty() || seed.encode_utf16().count() > 64 {
        return Err(bare(StatusCode::NOT_FOUND));
    }
    let find = |col: &'static str| {
        let (s, seed) = (state.clone(), seed.clone());
        async move {
            db::one(
                &s,
                &entity::NUMBER_SHUFFLE,
                &format!("SELECT * FROM numbershuffle WHERE {col} = ? ORDER BY id DESC LIMIT 1"),
                &seed,
            )
            .await
        }
    };
    let game = match find("fakeSeed").await? {
        Some(g) => g,
        None => find("seed").await?.ok_or_else(|| bare(StatusCode::NOT_FOUND))?,
    };
    let counter = match game["user_uuid"].as_str().filter(|u| !u.is_empty()) {
        Some(u) => db::find_counter(&state, u).await?,
        None => None,
    };
    let s = |c: &Value, k: &str| c[k].as_str().filter(|x| !x.is_empty()).map(str::to_owned);
    Ok(reply(json!({
        "seed": game["seed"], "size": game["size"], "score": game["score"],
        "gameStatus": game["gameStatus"], "day": game["day"],
        "counter": counter.map(|c| json!({
            "username": c["username"],
            "name": s(&c, "name").map_or(c["username"].clone(), Value::from),
            "color": s(&c, "color").unwrap_or_default(),
            "discordId": s(&c, "discordId").unwrap_or_default(),
            "avatar": s(&c, "avatar").unwrap_or_default(),
        })),
    })))
}

async fn thread_by_name(
    State(state): State<AppState>,
    MaybeUser(user): MaybeUser,
    Path(name): Path<String>,
) -> ApiResult<Reply> {
    let c = viewer(&state, user.as_ref()).await?;
    find_thread(&state, &name, c.as_ref(), user.is_some())
        .await?
        .map(reply)
        .ok_or_else(|| bare(StatusCode::NOT_FOUND))
}

const BANNED_SUBSTRINGS: [&str; 14] = [
    "kill yourself",
    "chink",
    "nigga",
    "nigger",
    "retard",
    "faggot",
    "卐",
    "卍",
    "shemale",
    "tranny",
    "trannies",
    "bimbo",
    "is cancer",
    "are cancer",
];
const BANNED_WORDS: [&str; 7] = ["niger", "nigers", "fag", "fags", "coon", "coons", "kys"];
const BANNED_ENTIRE: [&str; 5] = ["cancer", "cunt", "cunts", "regarded", "you are regarded"];

pub fn notes_ok(update: &str) -> bool {
    let lower = update.to_lowercase();
    let words: Vec<&str> = lower.split(|c: char| c.is_whitespace()).collect();
    !(update.encode_utf16().count() > 16384
        || BANNED_SUBSTRINGS.iter().any(|s| lower.contains(s))
        || words.iter().any(|w| BANNED_WORDS.contains(w))
        || BANNED_ENTIRE.contains(&lower.as_str()))
}

fn b64(s: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(s.as_bytes())
}

async fn update_notes(State(state): State<AppState>, Authed(user): Authed, body: JsonBody) -> ApiResult<Reply> {
    let c = match db::find_counter(&state, &user.uuid).await? {
        Some(c) if js_number(&c["xp"]) >= 54321.0 && !has_role(&c, "muted") && !has_role(&c, "banned") => c,
        _ => return Err(bare(StatusCode::FORBIDDEN)),
    };
    let tuuid = js_string(body.get("threadUUID"));
    let thread = db::find_thread_by_uuid(&state, &tuuid)
        .await?
        .filter(|t| thread_visible_to(t, Some(&c)));
    let Some(thread) = thread else {
        return Err(bare(StatusCode::NOT_FOUND));
    };
    let Some(update) = body.str("update") else {
        return Err(ApiError::Internal(anyhow::anyhow!("update missing")));
    };
    let old = thread["rules"].as_str().unwrap_or("").to_owned();
    if counting::js::trim(&old) == counting::js::trim(update) {
        return Ok(empty());
    }
    if !notes_ok(update) {
        return Err(bare(StatusCode::BAD_REQUEST));
    }
    db::exec(
        &state,
        "UPDATE thread SET rules = ? WHERE uuid = ?",
        &[json!(update), json!(tuuid)],
    )
    .await?;
    let updated = db::find_thread_by_uuid(&state, &tuuid).await?.unwrap_or(Value::Null);
    let diff = crate::unified_diff::two_files_patch("old.txt", "new.txt", &old, update, "old", "new");
    state.bot.emit(
        "logCustomMessageWithAttachments",
        json!({
            "message": format!(
                "Counter {} ({}) just updated thread {} ({})'s community note.",
                js_string(&c["username"]), js_string(&c["uuid"]), js_string(&thread["name"]), tuuid
            ),
            "attachments": [
                { "name": "old.txt", "base64": b64(&old) },
                { "name": "new.txt", "base64": b64(update) },
                { "name": "changes.diff", "base64": b64(&diff) },
            ],
        }),
    );
    state.socket.emit("updateThread", json!({ "thread": updated }));
    Ok(empty())
}

fn csv(v: &Value) -> Option<Vec<String>> {
    Some(counting::js::trim(v.as_str()?).split(',').map(str::to_owned).collect())
}

async fn create_thread(State(state): State<AppState>, Authed(user): Authed, body: JsonBody) -> ApiResult<Reply> {
    let Some(c) = db::find_counter(&state, &user.uuid).await? else {
        return Err(bare(StatusCode::NOT_FOUND));
    };
    if !has_role(&c, "admin") {
        return Ok(empty());
    }
    let lists: Vec<Option<Vec<String>>> = [
        "visibleTo",
        "updatableBy",
        "moderators",
        "verifiers",
        "countBans",
        "postBans",
    ]
    .iter()
    .map(|k| csv(body.get(k)))
    .collect();
    let (Some(name), Some(title)) = (body.str("name"), body.str("title")) else {
        return Err(ApiError::Internal(anyhow::anyhow!("name/title missing")));
    };
    if lists.iter().any(Option::is_none) {
        return Err(ApiError::Internal(anyhow::anyhow!("list fields missing")));
    }
    let lists: Vec<Vec<String>> = lists.into_iter().flatten().collect();
    let len = |s: &str| counting::js::trim(s).encode_utf16().count();
    let valid = !(len(name) < 1
        || len(name) > 200
        || len(title) < 1
        || len(title) > 200
        || lists[0].is_empty()
        || lists[1].is_empty()
        || !body.get("locked").is_boolean()
        || !body.get("autoValidated").is_boolean());
    if !valid {
        return Err(bare(StatusCode::BAD_REQUEST));
    }
    let j = |v: &Vec<String>| json!(json!(v).to_string());
    let f = |k: &str| body.get(k).clone();
    let uuid = body.str("uuid").filter(|u| !u.is_empty()).map(str::to_owned);
    let saved_uuid = match uuid {
        None => {
            let uuid = cgg_web::ids::uuid_for(state.clock.now_ns());
            db::exec(
                &state,
                "INSERT INTO thread (uuid, name, title, description, rules, firstValidCount, validationType, visibleTo, updatableBy, locked, autoValidated, moderators, verifiers, countBans, postBans, resetOnMistakes, allowDoublePosts, shortDescription, color1, color2, category, countsPerSplit, splitsPerGet, splitOffset) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                &[json!(uuid), json!(name), json!(title), f("description"), f("rules"), f("firstValidCount"), f("validationType"), j(&lists[0]), j(&lists[1]), f("locked"), f("autoValidated"), j(&lists[2]), j(&lists[3]), j(&lists[4]), j(&lists[5]), f("resetOnMistakes"), f("allowDoublePosts"), f("shortDescription"), f("color1"), f("color2"), f("category"), f("countsPerSplit"), f("splitsPerGet"), f("splitOffset")],
            )
            .await?;
            uuid
        }
        Some(uuid) => {
            db::exec(
                &state,
                "UPDATE thread SET title = ?, description = ?, rules = ?, firstValidCount = ?, validationType = ?, visibleTo = ?, updatableBy = ?, locked = ?, autoValidated = ?, resetOnMistakes = ?, allowDoublePosts = ?, moderators = ?, verifiers = ?, countBans = ?, postBans = ?, shortDescription = ?, color1 = ?, color2 = ?, category = ?, countsPerSplit = ?, splitsPerGet = ?, splitOffset = ? WHERE uuid = ?",
                &[json!(title), f("description"), f("rules"), f("firstValidCount"), f("validationType"), j(&lists[0]), j(&lists[1]), f("locked"), f("autoValidated"), f("resetOnMistakes"), f("allowDoublePosts"), j(&lists[2]), j(&lists[3]), j(&lists[4]), j(&lists[5]), f("shortDescription"), f("color1"), f("color2"), f("category"), f("countsPerSplit"), f("splitsPerGet"), f("splitOffset"), json!(uuid)],
            )
            .await?;
            uuid
        }
    };
    let saved = db::find_thread_by_uuid(&state, &saved_uuid)
        .await?
        .unwrap_or(Value::Null);
    state.socket.emit("updateThread", json!({ "thread": saved }));
    Ok(reply(saved))
}

async fn recent_counts(
    state: &AppState,
    thread_id: i64,
    take: i64,
    end_id: Option<i64>,
    comments: bool,
) -> ApiResult<(Vec<Value>, Vec<Value>, bool)> {
    let index = if comments { "n_thread_3char_id" } else { "n_thread_id" };
    let mut sql = format!("SELECT * FROM post USE INDEX ({index}) WHERE threadId = ?");
    let mut args = vec![json!(thread_id)];
    if let Some(e) = end_id.filter(|e| *e != 0) {
        sql.push_str(" AND id < ?");
        args.push(json!(e));
    }
    if comments {
        sql.push_str(" AND hasThreeCharComment = true");
    }
    sql.push_str(&format!(" ORDER BY id DESC LIMIT {}", take.max(0)));
    let posts = posts_of(&db::rows(state, &sql, &args).await?);
    let counters = authors(state, &posts).await?;
    let oldest = (posts.len() as i64) < take;
    Ok((posts, counters, oldest))
}

struct Window {
    posts: Vec<Value>,
    counters: Vec<Value>,
    newest: bool,
    oldest: Option<bool>,
}

async fn around(state: &AppState, thread_id: i64, context: Option<&str>, comments: bool) -> ApiResult<(Window, bool)> {
    let anchor = match context {
        Some(c) => db::rows(
            state,
            "SELECT * FROM post WHERE threadId = ? AND uuid = ? LIMIT 1",
            &[json!(thread_id), json!(c)],
        )
        .await?
        .first()
        .map(|r| entity::POST.json(r)),
        None => None,
    };
    let Some(anchor) = anchor else {
        let (posts, counters, oldest) = recent_counts(state, thread_id, 50, None, comments).await?;
        return Ok((
            Window {
                posts,
                counters,
                newest: true,
                oldest: oldest.then_some(true),
            },
            false,
        ));
    };
    let id = anchor["id"].as_i64().unwrap_or_default();
    let c3 = if comments {
        " AND hasThreeCharComment = true"
    } else {
        ""
    };
    let newer = posts_of(
        &db::rows(
            state,
            &format!("SELECT * FROM post WHERE threadId = ? AND id > ?{c3} ORDER BY id ASC LIMIT 25"),
            &[json!(thread_id), json!(id)],
        )
        .await?,
    );
    let older = if newer.len() >= 24 {
        posts_of(
            &db::rows(
                state,
                &format!("SELECT * FROM post WHERE threadId = ? AND id < ?{c3} ORDER BY id DESC LIMIT 25"),
                &[json!(thread_id), json!(id)],
            )
            .await?,
        )
    } else {
        recent_counts(state, thread_id, 50 - newer.len() as i64, Some(id), comments)
            .await?
            .0
    };
    let newest = newer.len() != 25;
    let oldest = older.len() < 25;
    let mut all: Vec<Value> = newer.into_iter().take(24).rev().collect();
    all.push(anchor);
    all.extend(older);
    let counters = authors(state, &all).await?;
    Ok((
        Window {
            posts: all,
            counters,
            newest,
            oldest: Some(oldest),
        },
        true,
    ))
}

async fn get_recent(State(state): State<AppState>, MaybeUser(user): MaybeUser, body: JsonBody) -> ApiResult<Reply> {
    let c = viewer(&state, user.as_ref()).await?;
    let name = js_string(body.get("thread_name"));
    let Some(thread) = find_thread(&state, &name, c.as_ref(), user.is_some()).await? else {
        return Err(bare(StatusCode::BAD_REQUEST));
    };
    let tid = thread["id"].as_i64().unwrap_or_default();
    let comments = truthy(body.get("commentsOnly"));
    let (w, _) = around(&state, tid, body.get("context").as_str(), comments).await?;
    let mut out = json!({ "recentCounts": w.posts, "counters": w.counters, "isNewest": w.newest, "recentSplits": [] });
    if let Some(o) = w.oldest {
        out["isOldest"] = json!(o);
    }
    Ok(reply(out))
}

fn system_counter() -> Value {
    json!({
        "uuid": "[SYSTEM]", "id": -1, "discordId": "[SYSTEM]", "name": "[SYSTEM]", "username": "[SYSTEM]", "color": "#069420",
        "roles": ["bot"], "avatar": "1", "pronouns": ["they", "them", "their", "theirs"], "title": "[SYSTEM]",
        "cardStyle": "Default", "cardBorderStyle": "Default", "xp": 100, "xp2": 100, "rainbow": 1, "blogs": [], "blogLikes": [],
    })
}

async fn last_valid(state: &AppState, thread_id: i64) -> ApiResult<Option<Value>> {
    Ok(db::rows(
        state,
        "SELECT * FROM post WHERE threadId = ? AND isValidCount = 1 ORDER BY id DESC LIMIT 1",
        &[json!(thread_id)],
    )
    .await?
    .first()
    .map(|r| entity::POST.json(r)))
}

async fn recent_on_load_full(
    State(state): State<AppState>,
    MaybeUser(user): MaybeUser,
    Path(name): Path<String>,
    q: Query,
) -> ApiResult<Reply> {
    let c = viewer(&state, user.as_ref()).await?;
    let Some(thread) = find_thread(&state, &name, c.as_ref(), user.is_some()).await? else {
        return Err(bare(StatusCode::BAD_REQUEST));
    };
    let tid = thread["id"].as_i64().unwrap_or_default();
    let comments = q.str("commentsOnly") == Some("true");
    let context = q.str("context").filter(|s| !s.is_empty());
    let (w, anchored) = around(&state, tid, context, comments).await?;
    let with_author = |p: &Value, counters: &[Value]| counters.iter().find(|c| c["uuid"] == p["authorUUID"]).cloned();
    let last_post = if anchored {
        last_valid(&state, tid).await?
    } else {
        match w.posts.iter().find(|p| p["isValidCount"] == Value::Bool(true)) {
            Some(p) => Some(p.clone()),
            None => last_valid(&state, tid).await?,
        }
    };
    let last = match last_post {
        Some(p) => {
            let author = match with_author(&p, if anchored { &[] } else { &w.counters }) {
                Some(a) => a,
                None => db::find_counter(&state, p["authorUUID"].as_str().unwrap_or(""))
                    .await?
                    .unwrap_or(Value::Null),
            };
            json!({ "post": p, "author": author })
        }
        None if !anchored => Value::Null,
        None => return Err(ApiError::Internal(anyhow::anyhow!("no valid count in thread"))),
    };
    let pwa: Vec<Value> = w
        .posts
        .iter()
        .map(|p| json!({ "post": p, "author": with_author(p, &w.counters).unwrap_or_else(system_counter) }))
        .collect();
    let mut out = json!({ "isNewest": w.newest, "postsWithAuthors": pwa });
    if let Some(o) = w.oldest {
        out["isOldest"] = json!(o);
    }
    if !last.is_null() {
        out["lastCount"] = last;
    }
    Ok(reply(out))
}

async fn count_by_uuid(State(state): State<AppState>, body: JsonBody) -> ApiResult<Reply> {
    let uuid = js_string(body.get("uuid"));
    let Some(post) = db::one(
        &state,
        &entity::POST,
        "SELECT * FROM post WHERE uuid = ? LIMIT 1",
        &uuid,
    )
    .await?
    else {
        return Err(ApiError::Internal(anyhow::anyhow!("post not found")));
    };
    let thread = db::find_thread_by_uuid_value(&state, &post["thread"])
        .await?
        .unwrap_or(Value::Null);
    let counter = db::find_counter(&state, post["authorUUID"].as_str().unwrap_or(""))
        .await?
        .unwrap_or(Value::Null);
    let mut out = json!({ "count": [post], "counters": [counter] });
    if !thread.is_null() {
        out["thread"] = thread;
    }
    Ok(reply(out))
}

async fn search_result(state: &AppState, posts: Vec<Value>) -> ApiResult<Reply> {
    let counters = authors(state, &posts).await?;
    let mut tids: Vec<String> = Vec::new();
    for p in &posts {
        let t = js_string(&p["thread"]);
        if !tids.contains(&t) {
            tids.push(t);
        }
    }
    let threads: Vec<Value> = if tids.is_empty() {
        Vec::new()
    } else {
        let a: Vec<Value> = tids.iter().map(|t| json!(t)).collect();
        db::rows(
            state,
            &format!("SELECT * FROM thread WHERE uuid IN ({})", db::placeholders(a.len())),
            &a,
        )
        .await?
        .iter()
        .map(|r| entity::THREAD.json(r))
        .collect()
    };
    Ok(reply(
        json!({ "posts": posts, "counters": counters, "threads": threads }),
    ))
}

async fn by_raw_count(State(state): State<AppState>, body: JsonBody) -> ApiResult<Reply> {
    let raw = js_string(body.get("rawCount"));
    let posts = match body.str("thread").filter(|t| !t.is_empty()) {
        Some(t) => {
            let tid = thread_id_of(&state, t).await?;
            posts_of(
                &db::rows(
                    &state,
                    "SELECT * FROM post WHERE threadId = ? AND isValidCount = 1 AND rawCount = ? ORDER BY id ASC",
                    &[json!(tid), json!(raw)],
                )
                .await?,
            )
        }
        None => posts_of(
            &db::rows(
                &state,
                "SELECT * FROM post WHERE isValidCount = 1 AND rawCount = ? ORDER BY id ASC",
                &[json!(raw)],
            )
            .await?,
        ),
    };
    search_result(&state, posts).await
}

async fn by_number(State(state): State<AppState>, body: JsonBody) -> ApiResult<Reply> {
    let number = body.get("number").clone();
    let posts = match body.str("thread").filter(|t| !t.is_empty()) {
        Some(t) => {
            let tid = thread_id_of(&state, t).await?;
            posts_of(&db::rows(&state, "SELECT * FROM post WHERE threadId = ? AND isValidCount = 1 AND validCountNumber = ? ORDER BY id ASC", &[json!(tid), number]).await?)
        }
        None => posts_of(
            &db::rows(&state, "SELECT * FROM post WHERE threadId IN (SELECT id FROM thread) AND isValidCount = 1 AND validCountNumber = ? ORDER BY id ASC", &[number]).await?,
        ),
    };
    search_result(&state, posts).await
}

async fn by_comment(State(state): State<AppState>, Authed(user): Authed, body: JsonBody) -> ApiResult<Reply> {
    match db::find_counter(&state, &user.uuid).await? {
        Some(c) if has_role(&c, "admin") || has_role(&c, "mod") => {}
        _ => return Err(bare(StatusCode::FORBIDDEN)),
    }
    let Some(comment) = body.get("comment").as_str() else {
        return Err(ApiError::Internal(anyhow::anyhow!("comment missing")));
    };
    let comment = comment.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
    let like = format!("%{comment}%");
    let posts = match body.str("thread").filter(|t| !t.is_empty()) {
        Some(t) => posts_of(
            &db::rows(
                &state,
                "SELECT * FROM post WHERE comment LIKE ? AND thread = ? LIMIT 20000",
                &[json!(like), json!(t)],
            )
            .await?,
        ),
        None => posts_of(
            &db::rows(
                &state,
                "SELECT * FROM post WHERE comment LIKE ? LIMIT 20000",
                &[json!(like)],
            )
            .await?,
        ),
    };
    search_result(&state, posts).await
}

async fn lock(state: &AppState, user: &crate::session::SessionUser, uuid: &str, silent: bool) -> ApiResult<Reply> {
    let c = match db::find_counter(state, &user.uuid).await? {
        Some(c) if has_role(&c, "mod") => c,
        _ => return Err(bare(StatusCode::NOT_FOUND)),
    };
    let Some(mut t) = db::find_thread_by_uuid(state, uuid).await? else {
        return Err(ApiError::Internal(anyhow::anyhow!("thread {uuid} not found")));
    };
    let locked = t["locked"] != Value::Bool(true);
    t["locked"] = json!(locked);
    db::exec(
        state,
        "UPDATE thread SET locked = ? WHERE uuid = ?",
        &[json!(locked), json!(uuid)],
    )
    .await?;
    if !silent {
        state.socket.emit("updateThread", json!({ "thread": t }));
    }
    state.bot.emit(
        if silent { "logSilentLock" } else { "logLock" },
        json!({ "locker": c, "thread": t }),
    );
    Ok(empty())
}

async fn toggle_lock(State(state): State<AppState>, Authed(user): Authed, body: JsonBody) -> ApiResult<Reply> {
    lock(&state, &user, &js_string(body.get("thread_uuid")), false).await
}

async fn toggle_silent_lock(State(state): State<AppState>, Authed(user): Authed, body: JsonBody) -> ApiResult<Reply> {
    lock(&state, &user, &js_string(body.get("thread_uuid")), true).await
}
