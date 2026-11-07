use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub const LAGGY_ROUND_TRIP_MS: f64 = 300.0;
pub const LAGGY_PING_MS: f64 = 400.0;
const SLOW_LEG_MS: f64 = 200.0;
const SLOW_SERVER_MS: f64 = 200.0;
const BUSY_MAIN_THREAD_MS: f64 = 200.0;
const BUSY_BLOCKED_PER_WINDOW_MS: f64 = 2000.0;
const BUSY_FRAMES_PER_MINUTE_MS: f64 = 8000.0;
const SLOW_EDGE_MS: f64 = 800.0;
const FAST_EDGE_MS: f64 = 250.0;
const MIN_OTHERS: usize = 3;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, PartialOrd, Ord)]
pub enum Cause {
    Ok,
    Background,
    Server,
    Everyone,
    Dropped,
    Device,
    Internet,
    Route,
    Upload,
    Download,
    Connection,
    Polling,
    Unknown,
}

impl Cause {
    pub const ALL: [Cause; 13] = [
        Cause::Ok,
        Cause::Background,
        Cause::Server,
        Cause::Everyone,
        Cause::Dropped,
        Cause::Device,
        Cause::Internet,
        Cause::Route,
        Cause::Upload,
        Cause::Download,
        Cause::Connection,
        Cause::Polling,
        Cause::Unknown,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Cause::Ok => "ok",
            Cause::Background => "background",
            Cause::Server => "server",
            Cause::Everyone => "everyone",
            Cause::Dropped => "dropped",
            Cause::Device => "device",
            Cause::Internet => "internet",
            Cause::Route => "route",
            Cause::Upload => "upload",
            Cause::Download => "download",
            Cause::Connection => "connection",
            Cause::Polling => "polling",
            Cause::Unknown => "unknown",
        }
    }

    pub fn index(self) -> usize {
        Cause::ALL.iter().position(|c| *c == self).unwrap_or(0)
    }

    pub fn is_ours(self) -> bool {
        matches!(self, Cause::Server | Cause::Everyone | Cause::Route)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Drop {
    pub at: i64,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub down_ms: Option<f64>,
    pub side: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Minute {
    pub t: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rt: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proc_: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub up: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub down: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ping: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_lag: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frames_blocked: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edge: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub colo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vis: Option<f64>,
    #[serde(default)]
    pub pings_sent: f64,
    #[serde(default)]
    pub pings_back: f64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub drops: Vec<Drop>,
    #[serde(default)]
    pub offline_ms: f64,
    #[serde(default)]
    pub polling: bool,
    #[serde(default)]
    pub counts: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub net: Option<String>,
}

fn worst(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.max(y)),
        (x, None) => x,
        (None, y) => y,
    }
}

fn least(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (x, None) => x,
        (None, y) => y,
    }
}

pub fn minute_of(t: i64) -> i64 {
    t.div_euclid(60_000) * 60_000
}

impl Minute {
    pub fn absorb(&mut self, o: &Minute) {
        self.rt = worst(self.rt, o.rt);
        self.proc_ = worst(self.proc_, o.proc_);
        self.up = worst(self.up, o.up);
        self.down = worst(self.down, o.down);
        self.ping = worst(self.ping, o.ping);
        self.loop_lag = worst(self.loop_lag, o.loop_lag);
        self.blocked = worst(self.blocked, o.blocked);
        self.frames_blocked = worst(self.frames_blocked, o.frames_blocked);
        self.edge = worst(self.edge, o.edge);
        if o.colo.is_some() {
            self.colo.clone_from(&o.colo);
        }
        self.vis = least(self.vis, o.vis);
        self.pings_sent += o.pings_sent;
        self.pings_back += o.pings_back;
        self.drops.extend(o.drops.iter().cloned());
        self.offline_ms += o.offline_ms;
        self.polling |= o.polling;
        self.counts += o.counts;
        if o.net.is_some() {
            self.net.clone_from(&o.net);
        }
    }

    pub fn ping_loss(&self) -> Option<f64> {
        (self.pings_sent >= 4.0).then(|| ((self.pings_sent - self.pings_back) / self.pings_sent).clamp(0.0, 1.0))
    }

    pub fn is_laggy(&self) -> bool {
        self.rt.is_some_and(|v| v >= LAGGY_ROUND_TRIP_MS)
            || self.ping.is_some_and(|v| v >= LAGGY_PING_MS)
            || self.ping_loss().is_some_and(|l| l >= 0.25)
            || self.drops.iter().any(Drop::is_network)
            || self.offline_ms > 0.0
    }
}

impl Drop {
    pub fn is_network(&self) -> bool {
        match self.side.as_str() {
            "server" => matches!(self.reason.as_str(), "heartbeat_timeout" | "transport_error"),
            _ => matches!(
                self.reason.as_str(),
                "ping_timeout" | "transport_close" | "transport_error"
            ),
        }
    }
}

pub fn merge_by_minute(samples: impl IntoIterator<Item = Minute>) -> BTreeMap<i64, Minute> {
    let mut out: BTreeMap<i64, Minute> = BTreeMap::new();
    for mut s in samples {
        let t = minute_of(s.t);
        s.t = t;
        match out.get_mut(&t) {
            Some(m) => m.absorb(&s),
            None => {
                out.insert(t, s);
            }
        }
    }
    out
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Others {
    pub t: i64,
    #[serde(default)]
    pub rt: Vec<f64>,
    #[serde(default)]
    pub proc_: Vec<f64>,
    #[serde(default)]
    pub up: Vec<f64>,
    #[serde(default)]
    pub down: Vec<f64>,
    #[serde(default)]
    pub ping: Vec<f64>,
    #[serde(default)]
    pub edge: Vec<f64>,
    #[serde(default)]
    pub clients: usize,
}

const OTHERS_CAP: usize = 400;

fn push_capped(v: &mut Vec<f64>, x: Option<f64>) {
    if let Some(x) = x.filter(|x| x.is_finite())
        && v.len() < OTHERS_CAP
    {
        v.push(x.round());
    }
}

fn remove_one(v: &mut Vec<f64>, x: Option<f64>) {
    if let Some(x) = x
        && let Some(i) = v.iter().position(|y| *y == x.round())
    {
        v.swap_remove(i);
    }
}

pub fn median(v: &[f64]) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    Some(s[s.len() / 2])
}

impl Others {
    pub fn add(&mut self, m: &Minute) {
        push_capped(&mut self.rt, m.rt);
        push_capped(&mut self.proc_, m.proc_);
        push_capped(&mut self.up, m.up);
        push_capped(&mut self.down, m.down);
        push_capped(&mut self.ping, m.ping);
        push_capped(&mut self.edge, m.edge);
    }

    pub fn without(&self, own: &[Minute]) -> Others {
        let mut o = self.clone();
        for m in own {
            remove_one(&mut o.rt, m.rt);
            remove_one(&mut o.proc_, m.proc_);
            remove_one(&mut o.up, m.up);
            remove_one(&mut o.down, m.down);
            remove_one(&mut o.ping, m.ping);
            remove_one(&mut o.edge, m.edge);
        }
        o.clients = o.clients.saturating_sub(usize::from(!own.is_empty()));
        o
    }

    fn sampled(&self) -> usize {
        self.clients.max(self.rt.len().max(self.ping.len()))
    }

    pub fn site_wide_lag(&self) -> bool {
        if self.sampled() < MIN_OTHERS {
            return false;
        }
        median(&self.rt).is_some_and(|v| v >= LAGGY_ROUND_TRIP_MS)
            || median(&self.ping).is_some_and(|v| v >= LAGGY_PING_MS)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Evidence {
    pub key: &'static str,
    pub you: Option<f64>,
    pub others: Option<f64>,
}

impl Evidence {
    pub fn json(&self) -> Value {
        json!([self.key, self.you.map(f64::round), self.others.map(f64::round)])
    }
}

fn busy_device(m: &Minute) -> bool {
    m.loop_lag.is_some_and(|v| v >= BUSY_MAIN_THREAD_MS)
        || m.blocked.is_some_and(|v| v >= BUSY_BLOCKED_PER_WINDOW_MS)
        || m.frames_blocked.is_some_and(|v| v >= BUSY_FRAMES_PER_MINUTE_MS)
}

fn edge_verdict(m: &Minute, usual_edge: Option<f64>) -> Option<Cause> {
    let e = m.edge?;
    let usual_slow = usual_edge.is_some_and(|u| e >= 300.0 && e >= 4.0 * u.max(1.0));
    if e >= SLOW_EDGE_MS || usual_slow {
        return Some(Cause::Internet);
    }
    (e <= FAST_EDGE_MS).then_some(Cause::Route)
}

pub fn classify(m: &Minute, others: &Others, usual_edge: Option<f64>) -> Cause {
    if !m.is_laggy() {
        return Cause::Ok;
    }
    if m.vis.is_some_and(|v| v < 0.5) {
        return Cause::Background;
    }
    if let (Some(p), Some(rt)) = (m.proc_, m.rt)
        && p >= SLOW_SERVER_MS
        && p >= 0.4 * rt
    {
        return Cause::Server;
    }
    if others.site_wide_lag() {
        return Cause::Everyone;
    }
    if m.offline_ms > 0.0 || m.ping_loss().is_some_and(|l| l >= 0.5) || m.drops.iter().any(Drop::is_network) {
        return Cause::Dropped;
    }
    if busy_device(m) {
        return Cause::Device;
    }
    if let Some(c) = edge_verdict(m, usual_edge) {
        return c;
    }
    let up = m.up.unwrap_or(0.0);
    let down = m.down.unwrap_or(0.0);
    if up >= SLOW_LEG_MS && up >= down {
        return Cause::Upload;
    }
    if down >= SLOW_LEG_MS {
        return Cause::Download;
    }
    if m.ping.is_some_and(|v| v >= LAGGY_PING_MS) || m.ping_loss().is_some_and(|l| l >= 0.25) {
        return Cause::Connection;
    }
    if m.polling {
        return Cause::Polling;
    }
    Cause::Unknown
}

pub fn evidence(cause: Cause, m: &Minute, o: &Others) -> Vec<Evidence> {
    let e = |key: &'static str, you: Option<f64>, others: Option<f64>| Evidence { key, you, others };
    let rt = e("roundTrip", m.rt, median(&o.rt));
    let server = e("server", m.proc_, median(&o.proc_));
    let ping = e("ping", m.ping, median(&o.ping));
    let up = e("upload", m.up, median(&o.up));
    let down = e("download", m.down, median(&o.down));
    let edge = e("edge", m.edge, median(&o.edge));
    let loss = e("pingLoss", m.ping_loss().map(|l| l * 100.0), None);
    let main = e("mainThread", m.loop_lag, None);
    let blocked = e("blocked", m.blocked.or(m.frames_blocked), None);
    let vis = e("visible", m.vis.map(|v| v * 100.0), None);
    let offline = e("offline", (m.offline_ms > 0.0).then_some(m.offline_ms), None);
    let mut out = match cause {
        Cause::Ok | Cause::Unknown | Cause::Polling => vec![rt, server, ping],
        Cause::Background => vec![vis, rt, ping],
        Cause::Server | Cause::Everyone => vec![server, rt, ping],
        Cause::Dropped => vec![loss, offline, ping, rt],
        Cause::Device => vec![main, blocked, rt, server],
        Cause::Internet | Cause::Route => vec![edge, ping, up, server],
        Cause::Upload => vec![up, down, ping, server],
        Cause::Download => vec![down, up, ping, server],
        Cause::Connection => vec![ping, loss, server],
    };
    out.retain(|x| x.you.is_some() || x.others.is_some());
    out
}

pub struct Diagnosis {
    pub minutes: Vec<(Minute, Cause, Vec<Evidence>)>,
    pub verdict: Cause,
    pub laggy: usize,
    pub by_cause: BTreeMap<Cause, usize>,
    pub proof: Option<usize>,
    pub usual_edge: Option<f64>,
}

fn severity(m: &Minute) -> f64 {
    m.rt.unwrap_or(0.0).max(m.ping.unwrap_or(0.0)) + m.ping_loss().unwrap_or(0.0) * 5000.0 + m.offline_ms
}

pub fn diagnose(you: &BTreeMap<i64, Minute>, others: &BTreeMap<i64, Others>) -> Diagnosis {
    let edges: Vec<f64> = you.values().filter_map(|m| m.edge).collect();
    let usual_edge = median(&edges);
    let empty = Others::default();
    let minutes: Vec<(Minute, Cause, Vec<Evidence>)> = you
        .values()
        .map(|m| {
            let o = others
                .get(&m.t)
                .map_or_else(|| empty.clone(), |o| o.without(std::slice::from_ref(m)));
            let c = classify(m, &o, usual_edge);
            let ev = evidence(c, m, &o);
            (m.clone(), c, ev)
        })
        .collect();
    let mut by_cause: BTreeMap<Cause, usize> = BTreeMap::new();
    for (_, c, _) in &minutes {
        if *c != Cause::Ok {
            *by_cause.entry(*c).or_default() += 1;
        }
    }
    let laggy = by_cause.values().sum();
    let verdict = by_cause
        .iter()
        .max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))
        .map_or(Cause::Ok, |(c, _)| *c);
    let proof = minutes
        .iter()
        .enumerate()
        .filter(|(_, (_, c, _))| *c == verdict && verdict != Cause::Ok)
        .max_by(|a, b| {
            severity(&a.1.0)
                .partial_cmp(&severity(&b.1.0))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(i, _)| i);
    Diagnosis {
        minutes,
        verdict,
        laggy,
        by_cause,
        proof,
        usual_edge,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn crowd(t: i64, rt: f64, ping: f64, up: f64) -> Others {
        let mut o = Others {
            t,
            clients: 10,
            ..Default::default()
        };
        for _ in 0..10 {
            o.add(&Minute {
                rt: Some(rt),
                ping: Some(ping),
                up: Some(up),
                proc_: Some(5.0),
                ..Default::default()
            });
        }
        o
    }

    fn slow_upload(t: i64) -> Minute {
        Minute {
            t,
            rt: Some(4500.0),
            proc_: Some(6.0),
            up: Some(4400.0),
            down: Some(90.0),
            ping: Some(4100.0),
            vis: Some(1.0),
            ..Default::default()
        }
    }

    #[test]
    fn upload_lag_with_a_healthy_crowd_is_the_upload() {
        let o = crowd(0, 200.0, 150.0, 60.0);
        assert_eq!(classify(&slow_upload(0), &o, None), Cause::Upload);
    }

    #[test]
    fn the_same_lag_for_everyone_is_ours() {
        let o = crowd(0, 2000.0, 1500.0, 1200.0);
        assert_eq!(classify(&slow_upload(0), &o, None), Cause::Everyone);
        assert!(Cause::Everyone.is_ours());
    }

    #[test]
    fn a_slow_server_is_the_server_even_when_alone() {
        let m = Minute {
            rt: Some(900.0),
            proc_: Some(700.0),
            ..Default::default()
        };
        assert_eq!(classify(&m, &Others::default(), None), Cause::Server);
    }

    #[test]
    fn a_slow_edge_means_the_users_internet_and_a_fast_one_means_the_route() {
        let o = crowd(0, 200.0, 150.0, 60.0);
        let mut m = slow_upload(0);
        m.edge = Some(3900.0);
        assert_eq!(classify(&m, &o, None), Cause::Internet);
        m.edge = Some(40.0);
        assert_eq!(classify(&m, &o, None), Cause::Route);
    }

    #[test]
    fn background_tabs_and_busy_devices_are_named() {
        let o = crowd(0, 200.0, 150.0, 60.0);
        let mut m = slow_upload(0);
        m.vis = Some(0.1);
        assert_eq!(classify(&m, &o, None), Cause::Background);
        let busy = Minute {
            rt: Some(800.0),
            proc_: Some(5.0),
            loop_lag: Some(650.0),
            vis: Some(1.0),
            ..Default::default()
        };
        assert_eq!(classify(&busy, &o, None), Cause::Device);
    }

    #[test]
    fn lost_pings_and_dropped_sockets_are_drops() {
        let o = crowd(0, 200.0, 150.0, 60.0);
        let m = Minute {
            pings_sent: 20.0,
            pings_back: 4.0,
            vis: Some(1.0),
            ..Default::default()
        };
        assert_eq!(classify(&m, &o, None), Cause::Dropped);
        let d = Minute {
            drops: vec![Drop {
                at: 0,
                reason: "heartbeat_timeout".into(),
                down_ms: None,
                side: "server".into(),
            }],
            ..Default::default()
        };
        assert_eq!(classify(&d, &o, None), Cause::Dropped);
    }

    #[test]
    fn a_fast_minute_is_fine() {
        let m = Minute {
            rt: Some(120.0),
            ping: Some(80.0),
            ..Default::default()
        };
        assert_eq!(classify(&m, &Others::default(), None), Cause::Ok);
    }

    #[test]
    fn excluding_yourself_from_the_crowd() {
        let mut o = Others {
            clients: 2,
            ..Default::default()
        };
        let me = slow_upload(0);
        o.add(&me);
        o.add(&Minute {
            rt: Some(100.0),
            ..Default::default()
        });
        let w = o.without(std::slice::from_ref(&me));
        assert_eq!(w.rt, vec![100.0]);
        assert_eq!(w.clients, 1);
    }

    #[test]
    fn diagnosis_picks_the_commonest_cause_and_its_worst_minute_as_proof() {
        let mut you = BTreeMap::new();
        let mut others = BTreeMap::new();
        for i in 0..5 {
            let t = i * 60_000;
            let mut m = slow_upload(t);
            m.rt = Some(1000.0 + i as f64 * 100.0);
            you.insert(t, m);
            others.insert(t, crowd(t, 200.0, 150.0, 60.0));
        }
        you.insert(
            300_000,
            Minute {
                t: 300_000,
                rt: Some(100.0),
                ..Default::default()
            },
        );
        let d = diagnose(&you, &others);
        assert_eq!(d.verdict, Cause::Upload);
        assert_eq!(d.laggy, 5);
        assert_eq!(d.proof, Some(4));
        let ev = &d.minutes[4].2;
        assert_eq!(ev[0].key, "upload");
        assert_eq!(ev[0].others, Some(60.0));
    }

    #[test]
    fn merge_takes_the_worst_of_a_minute() {
        let a = Minute {
            t: 61_000,
            rt: Some(100.0),
            pings_sent: 3.0,
            ..Default::default()
        };
        let b = Minute {
            t: 119_000,
            rt: Some(900.0),
            pings_sent: 3.0,
            vis: Some(0.4),
            ..Default::default()
        };
        let m = merge_by_minute([a, b]);
        let only = m.get(&60_000).unwrap();
        assert_eq!(only.rt, Some(900.0));
        assert_eq!(only.pings_sent, 6.0);
        assert_eq!(only.vis, Some(0.4));
    }
}
