//! gameservers — the Game servers widget's shared logic (contract docs/GAMESERVERS.md §3; vectors
//! testvectors/widgets/gameservers/*.json): reading the Tin Hat server's /api/gameservers/status answer, the state chip,
//! the one-line texts, the CPU / RAM bars, which confirmation a power action needs (the server's own rules, previewed),
//! the announcement text and the calendar link. A line-for-line port of the web's `src/widgets/gameservers.ts`
//! (11.1, 2026-09-28): numbers are doubles evaluated in the order written, `r(x)` = floor(x + 0.5), strings compare in
//! code-point order, trims and case changes are ASCII-only. Pure: no Qt, no I/O, no clock.

use std::cmp::Ordering;

use serde_json::{json, Map, Value};

use crate::num;
use crate::people::{ascii_lower, ascii_trim};
use crate::time::fmt_dur;

/// floor(x + 0.5) — the contract's rounding for this module (every input here is ≥ 0).
fn r(x: f64) -> f64 {
    (x + 0.5).floor()
}

fn cps(s: &str) -> usize {
    s.chars().count()
}

fn fin(v: Option<&Value>) -> Option<f64> {
    v.and_then(Value::as_f64).filter(|x| x.is_finite())
}

fn str_max(v: Option<&Value>, max: usize) -> Option<String> {
    v.and_then(Value::as_str).filter(|s| cps(s) <= max).map(str::to_string)
}

fn non_neg(v: Option<&Value>) -> f64 {
    match fin(v) {
        Some(x) if x >= 0.0 => r(x),
        _ => 0.0,
    }
}

/// An integer-valued double (JSON 3 or 3.0 alike, like `Number.isInteger`).
fn int_of(v: Option<&Value>) -> Option<f64> {
    fin(v).filter(|x| x.fract() == 0.0)
}

fn is_id(s: &str) -> bool {
    s.len() == 8 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[derive(Clone, Debug, PartialEq)]
pub struct Players {
    pub count: f64,
    pub names: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Pending {
    pub action: String,
    pub by: String,
    pub since: f64,
    pub queued: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GameServer {
    pub id: String,
    pub name: String,
    pub game: String,
    pub steam_app_id: Option<f64>,
    pub emoji: String,
    pub state: String,
    pub unavailable: Option<String>,
    pub uptime_ms: f64,
    pub cpu_used: f64,
    pub cpu_limit: f64,
    pub mem_used: f64,
    pub mem_limit: f64,
    pub players: Option<Players>,
    pub address: String,
    pub password: Option<String>,
    pub info: String,
    pub outside: bool,
    pub calendar: bool,
    pub pending: Option<Pending>,
    /// (what: "backup" | "restart", at)
    pub next: Option<(String, f64)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SwitchJob {
    pub from: String,
    pub to: String,
    pub by: String,
    pub since: f64,
    pub step: String,
    pub ok: Option<bool>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GameStatus {
    pub at: f64,
    pub space: String,
    pub announce_room: Option<String>,
    pub can_manage: bool,
    pub cap: f64,
    pub running: f64,
    pub memory_budget_mb: f64,
    pub memory_used_mb: f64,
    pub stale: bool,
    pub servers: Vec<GameServer>,
    pub job: Option<SwitchJob>,
}

// ---- 3.1 reading the answer -----------------------------------------------------------------------------------------

fn read_server(v: &Map<String, Value>, id: &str) -> GameServer {
    let name_t = v.get("name").and_then(Value::as_str).map(ascii_trim).unwrap_or("");
    let name = if (1..=64).contains(&cps(name_t)) { name_t.to_string() } else { id.to_string() };
    let app = int_of(v.get("steamAppId")).filter(|x| *x > 0.0 && *x < 1e9);
    let state = v
        .get("state")
        .and_then(Value::as_str)
        .filter(|s| matches!(*s, "running" | "starting" | "stopping" | "offline"))
        .unwrap_or("offline")
        .to_string();
    let empty = Map::new();
    let cpu = v.get("cpu").and_then(Value::as_object).unwrap_or(&empty);
    let cpu_used = match fin(cpu.get("used")) {
        Some(x) if x >= 0.0 => r(x * 10.0) / 10.0,
        _ => 0.0,
    };
    let cpu_limit = match fin(cpu.get("limit")) {
        Some(x) if x > 0.0 => r(x),
        _ => 100.0,
    };
    let mem = v.get("memory").and_then(Value::as_object).unwrap_or(&empty);
    let players = v.get("players").and_then(Value::as_object).and_then(|p| {
        let count = int_of(p.get("count")).filter(|c| *c >= 0.0)?;
        let mut names = Vec::new();
        if let Some(a) = p.get("names").and_then(Value::as_array) {
            for n in a {
                let Some(s) = n.as_str() else { continue };
                let t = ascii_trim(s);
                if (1..=32).contains(&cps(t)) && names.len() < 16 {
                    names.push(t.to_string());
                }
            }
        }
        Some(Players { count, names })
    });
    let join = v.get("join").and_then(Value::as_object).unwrap_or(&empty);
    let password = join.get("password").and_then(Value::as_str).filter(|s| (1..=128).contains(&cps(s))).map(str::to_string);
    let pending = v.get("pending").and_then(Value::as_object).and_then(|p| {
        let action = p.get("action").and_then(Value::as_str).filter(|a| matches!(*a, "start" | "stop" | "restart"))?;
        Some(Pending {
            action: action.to_string(),
            by: str_max(p.get("by"), 255).unwrap_or_default(),
            since: fin(p.get("since")).unwrap_or(0.0),
            queued: p.get("queued") == Some(&Value::Bool(true)),
        })
    });
    let next = v.get("next").and_then(Value::as_object).and_then(|n| {
        let what = n.get("what").and_then(Value::as_str).filter(|w| matches!(*w, "backup" | "restart"))?;
        let at = fin(n.get("at"))?;
        Some((what.to_string(), at))
    });
    let unavailable = str_max(v.get("unavailable"), 40).filter(|s| !s.is_empty());
    GameServer {
        id: id.to_string(),
        name,
        game: str_max(v.get("game"), 64).unwrap_or_default(),
        steam_app_id: app,
        emoji: str_max(v.get("emoji"), 16).unwrap_or_default(),
        state,
        unavailable,
        uptime_ms: non_neg(v.get("uptimeMs")),
        cpu_used,
        cpu_limit,
        mem_used: non_neg(mem.get("usedMB")),
        mem_limit: non_neg(mem.get("limitMB")),
        players,
        address: str_max(join.get("address"), 300).unwrap_or_default(),
        password,
        info: str_max(v.get("info"), 280).unwrap_or_default(),
        outside: v.get("outside") == Some(&Value::Bool(true)),
        calendar: v.get("calendar") != Some(&Value::Bool(false)),
        pending,
        next,
    }
}

/// The /status answer as the widget uses it (§3.1), or None when it is not one.
pub fn read_status(value: &Value) -> Option<GameStatus> {
    let o = value.as_object()?;
    if int_of(o.get("v")) != Some(1.0) {
        return None;
    }
    let list = o.get("servers")?.as_array()?;
    let mut servers: Vec<GameServer> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for item in list {
        if servers.len() >= 32 {
            break;
        }
        let Some(obj) = item.as_object() else { continue };
        let Some(id) = obj.get("id").and_then(Value::as_str) else { continue };
        if !is_id(id) || !seen.insert(id.to_string()) {
            continue;
        }
        servers.push(read_server(obj, id));
    }
    let cap = int_of(o.get("cap")).filter(|c| (1.0..=8.0).contains(c)).unwrap_or(2.0);
    let job = o.get("job").and_then(Value::as_object).and_then(|j| {
        let from = j.get("from").and_then(Value::as_str).filter(|s| is_id(s))?;
        let to = j.get("to").and_then(Value::as_str).filter(|s| is_id(s))?;
        let step = j.get("step").and_then(Value::as_str).filter(|s| matches!(*s, "stopping" | "starting" | "done" | "failed"))?;
        Some(SwitchJob {
            from: from.to_string(),
            to: to.to_string(),
            by: j.get("by").and_then(Value::as_str).unwrap_or("").to_string(),
            since: fin(j.get("since")).unwrap_or(0.0),
            step: step.to_string(),
            ok: match j.get("ok") {
                Some(Value::Bool(b)) => Some(*b),
                _ => None,
            },
            error: str_max(j.get("error"), 300),
        })
    });
    Some(GameStatus {
        at: fin(o.get("at")).unwrap_or(0.0),
        space: o.get("space").and_then(Value::as_str).unwrap_or("").to_string(),
        announce_room: o.get("announceRoom").and_then(Value::as_str).map(str::to_string),
        can_manage: o.get("canManage") == Some(&Value::Bool(true)),
        cap,
        running: non_neg(o.get("running")),
        memory_budget_mb: non_neg(o.get("memoryBudgetMB")),
        memory_used_mb: non_neg(o.get("memoryUsedMB")),
        stale: o.get("stale") == Some(&Value::Bool(true)),
        servers,
        job,
    })
}

fn opt_str(s: &Option<String>) -> Value {
    s.as_ref().map(|x| Value::String(x.clone())).unwrap_or(Value::Null)
}

impl GameServer {
    /// The JSON the reference's `readStatus` holds (the vectors' form).
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "game": self.game,
            "steamAppId": self.steam_app_id.map(num).unwrap_or(Value::Null),
            "emoji": self.emoji,
            "state": self.state,
            "unavailable": opt_str(&self.unavailable),
            "uptimeMs": num(self.uptime_ms),
            "cpu": {"used": num(self.cpu_used), "limit": num(self.cpu_limit)},
            "memory": {"usedMB": num(self.mem_used), "limitMB": num(self.mem_limit)},
            "players": self.players.as_ref().map(|p| json!({"count": num(p.count), "names": p.names})).unwrap_or(Value::Null),
            "join": {"address": self.address, "password": opt_str(&self.password)},
            "info": self.info,
            "outside": self.outside,
            "calendar": self.calendar,
            "pending": self.pending.as_ref().map(|p| json!({"action": p.action, "by": p.by, "since": num(p.since), "queued": p.queued})).unwrap_or(Value::Null),
            "next": self.next.as_ref().map(|(w, at)| json!({"what": w, "at": num(*at)})).unwrap_or(Value::Null),
        })
    }
}

impl GameStatus {
    pub fn to_json(&self) -> Value {
        json!({
            "at": num(self.at),
            "space": self.space,
            "announceRoom": opt_str(&self.announce_room),
            "canManage": self.can_manage,
            "cap": num(self.cap),
            "running": num(self.running),
            "memoryBudgetMB": num(self.memory_budget_mb),
            "memoryUsedMB": num(self.memory_used_mb),
            "stale": self.stale,
            "servers": self.servers.iter().map(GameServer::to_json).collect::<Vec<_>>(),
            "job": self.job.as_ref().map(|j| json!({
                "from": j.from, "to": j.to, "by": j.by, "since": num(j.since), "step": j.step,
                "ok": j.ok.map(Value::Bool).unwrap_or(Value::Null), "error": opt_str(&j.error),
            })).unwrap_or(Value::Null),
        })
    }

    pub fn server(&self, id: &str) -> Option<&GameServer> {
        self.servers.iter().find(|s| s.id == id)
    }
}

// ---- 3.2 the state chip ---------------------------------------------------------------------------------------------

/// "running" in the widget's sense: not offline, or a start / restart on its way (queued included).
pub fn is_up(s: &GameServer) -> bool {
    s.state != "offline" || s.pending.as_ref().is_some_and(|p| p.action == "start" || p.action == "restart")
}

/// (tone: "online" | "busy" | "offline", text)
pub fn chip_of(s: &GameServer) -> (&'static str, String) {
    if let Some(p) = &s.pending {
        if p.queued {
            return ("busy", "Queued".into());
        }
        let t = match p.action.as_str() {
            "start" => "Starting…",
            "stop" => "Stopping…",
            _ => "Restarting…",
        };
        return ("busy", t.into());
    }
    match s.state.as_str() {
        "running" => {
            let t = match &s.players {
                None => "Online".to_string(),
                Some(p) if p.count > 0.0 => format!("Online · {} on", js(p.count)),
                Some(_) => "Online · nobody on".to_string(),
            };
            ("online", t)
        }
        "starting" => ("busy", "Starting…".into()),
        "stopping" => ("busy", "Stopping…".into()),
        _ => ("offline", match &s.unavailable {
            Some(u) => format!("Offline · {u}"),
            None => "Offline".into(),
        }),
    }
}

pub fn chip_json(s: &GameServer) -> Value {
    let (tone, text) = chip_of(s);
    json!({"tone": tone, "text": text})
}

/// A double the way JS prints it in a template string.
fn js(x: f64) -> String {
    crate::js_number(x)
}

// ---- 3.3 lines ------------------------------------------------------------------------------------------------------

fn lower_first(t: &str) -> String {
    let mut c = t.chars();
    match c.next() {
        Some(f) if f.is_ascii_uppercase() => f.to_ascii_lowercase().to_string() + c.as_str(),
        _ => t.to_string(),
    }
}

/// The S / collapsed line.
pub fn summary_of(st: &GameStatus) -> String {
    let up: Vec<&GameServer> = st.servers.iter().filter(|s| is_up(s)).collect();
    match up.len() {
        0 => "No game running".into(),
        1 => format!("{} · {}", up[0].name, lower_first(&chip_of(up[0]).1)),
        n => {
            let mut text = format!("{} + {}", up[0].name, up[1].name);
            if n > 2 {
                text += &format!(" + {} more", n - 2);
            }
            let known: Vec<&&GameServer> = up.iter().filter(|s| s.players.is_some() && s.state == "running").collect();
            if !known.is_empty() {
                let sum: f64 = known.iter().map(|s| s.players.as_ref().map_or(0.0, |p| p.count)).sum();
                text += &format!(" · {} on", js(sum));
            }
            text
        }
    }
}

pub fn players_line(s: &GameServer) -> String {
    match &s.players {
        None => "—".into(),
        Some(p) if p.count == 0.0 => "Nobody on".into(),
        Some(p) if p.names.is_empty() => format!("{} on", js(p.count)),
        Some(p) => {
            let extra = p.count - p.names.len() as f64;
            let mut t = p.names.join(", ");
            if extra > 0.0 {
                t += &format!(" +{}", js(extra));
            }
            t
        }
    }
}

/// The card's "who's on" line, "" = no line: the names when known; "N on" only while the chip doesn't carry the count
/// (starting, stopping, pending); nothing for unknown or nobody (the chip says it).
pub fn who_line(s: &GameServer) -> String {
    match &s.players {
        None => String::new(),
        Some(p) if p.count == 0.0 => String::new(),
        Some(p) if p.names.is_empty() && s.state == "running" && s.pending.is_none() => String::new(),
        Some(_) => players_line(s),
    }
}

pub fn uptime_text(ms: f64) -> String {
    format!("up {}", fmt_dur(ms))
}

pub fn cpu_pm(used: f64, limit: f64) -> f64 {
    if limit > 0.0 {
        crate::js_min(1000.0, r(used / limit * 1000.0))
    } else {
        0.0
    }
}

pub fn cpu_text(used: f64, limit: f64) -> String {
    format!("{} %", js(if limit > 0.0 { r(used / limit * 100.0) } else { 0.0 }))
}

pub fn mem_pm(used: f64, limit: f64) -> f64 {
    if limit > 0.0 {
        crate::js_min(1000.0, r(used / limit * 1000.0))
    } else {
        0.0
    }
}

/// GB with one decimal, or whole when it is whole ("8", "12.5").
fn gb_short(mb: f64) -> String {
    let t = r(mb / 1024.0 * 10.0) / 10.0;
    if t.fract() == 0.0 {
        js(t)
    } else {
        format!("{t:.1}")
    }
}

pub fn mem_text(used: f64, limit: f64) -> String {
    let a = format!("{:.1}", r(used / 1024.0 * 10.0) / 10.0);
    if limit > 0.0 {
        format!("{a} / {} GB", gb_short(limit))
    } else {
        format!("{a} GB")
    }
}

pub fn size_text(mb: f64) -> String {
    format!("{} GB", gb_short(mb))
}

// ---- 3.4 which Start (the server's rules, previewed) ---------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub enum StartPlan {
    Ok,
    Unknown,
    Refused(String),
    Cap(Vec<String>),
    Memory { need_mb: f64, free_mb: f64, text: String },
}

impl StartPlan {
    pub fn to_json(&self) -> Value {
        match self {
            StartPlan::Ok => json!({"kind": "ok"}),
            StartPlan::Unknown => json!({"kind": "unknown"}),
            StartPlan::Refused(t) => json!({"kind": "refused", "text": t}),
            StartPlan::Cap(ids) => json!({"kind": "cap", "running": ids}),
            StartPlan::Memory { need_mb, free_mb, text } => json!({"kind": "memory", "needMB": num(*need_mb), "freeMB": num(*free_mb), "text": text}),
        }
    }
}

fn plan_with(st: &GameStatus, target: &str, first_id: Option<&str>) -> StartPlan {
    let Some(t) = st.server(target) else { return StartPlan::Unknown };
    if t.pending.is_some() {
        return StartPlan::Refused(format!("{} is busy — try again in a moment", t.name));
    }
    if let Some(u) = &t.unavailable {
        return StartPlan::Refused(format!("{} can't start right now ({u})", t.name));
    }
    if t.state != "offline" {
        return StartPlan::Refused(if t.state == "stopping" {
            format!("{} is still stopping — try again in a moment", t.name)
        } else {
            format!("{} is already running", t.name)
        });
    }
    let mut first: Option<&GameServer> = None;
    if let Some(fid) = first_id {
        let Some(f) = st.server(fid) else { return StartPlan::Unknown };
        if f.id == t.id {
            return StartPlan::Refused("stopFirst must be another server".into());
        }
        if f.pending.is_some() {
            return StartPlan::Refused(format!("{} is busy — try again in a moment", f.name));
        }
        if f.state == "offline" {
            return StartPlan::Refused(format!("{} isn't running", f.name));
        }
        first = Some(f);
    }
    let active: Vec<&GameServer> =
        st.servers.iter().filter(|s| s.id != t.id && first.map_or(true, |f| s.id != f.id) && is_up(s)).collect();
    if active.len() as f64 >= st.cap {
        return StartPlan::Cap(active.iter().map(|s| s.id.clone()).collect());
    }
    let budget = st.memory_budget_mb;
    let lim = |s: &GameServer| if s.mem_limit > 0.0 { s.mem_limit } else { budget };
    let used: f64 = active.iter().fold(0.0, |a, s| a + lim(s));
    let free = crate::js_max(0.0, budget - used);
    if used + lim(t) > budget {
        return StartPlan::Memory {
            need_mb: lim(t),
            free_mb: free,
            text: format!("{} needs {} GB and only {} GB is free — stop a game first", t.name, gb_short(lim(t)), gb_short(free)),
        };
    }
    StartPlan::Ok
}

/// Which confirmation a Start of `target` needs (§3.4).
pub fn plan_start(st: &GameStatus, target: &str) -> StartPlan {
    plan_with(st, target, None)
}

/// The same with `first` stopped first (the switch).
pub fn plan_switch(st: &GameStatus, target: &str, first: &str) -> StartPlan {
    plan_with(st, target, Some(first))
}

// ---- 3.5 confirmations ----------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct Confirm {
    pub title: String,
    pub body: String,
    pub confirm: String,
    pub destructive: bool,
}

impl Confirm {
    pub fn to_json(&self) -> Value {
        json!({"title": self.title, "body": self.body, "confirm": self.confirm, "destructive": self.destructive})
    }
}

const LAN: &str = "Ports closed — LAN only: people outside this network can't join (ask the server's owner to open them).";

fn on_line(n: f64, name: &str) -> String {
    format!("{} {} on {name}", js(n), if n == 1.0 { "person is" } else { "people are" })
}

fn playing(s: &GameServer) -> f64 {
    match (&s.players, s.state.as_str()) {
        (Some(p), "running") => p.count,
        _ => 0.0,
    }
}

/// The dialog a power action needs, or None (§3.5).
pub fn confirm_of(st: &GameStatus, action: &str, id: &str, first_id: Option<&str>) -> Option<Confirm> {
    let s = st.server(id)?;
    if let Some(fid) = first_id {
        let f = st.server(fid)?;
        if action != "start" {
            return None;
        }
        let mut body = format!("Two games are already running. Stop {} and start {}?", f.name, s.name);
        let n = playing(f);
        if n > 0.0 {
            body += &format!(" {}.", on_line(n, &f.name));
        }
        if !s.outside {
            body += &format!(" {LAN}");
        }
        return Some(Confirm {
            title: format!("Start {}?", s.name),
            body,
            confirm: format!("Stop {} and start {}", f.name, s.name),
            destructive: true,
        });
    }
    let n = playing(s);
    match action {
        "stop" => (n > 0.0).then(|| Confirm {
            title: format!("Stop {}?", s.name),
            body: format!("{} — stop anyway?", on_line(n, &s.name)),
            confirm: "Stop".into(),
            destructive: true,
        }),
        "restart" => (n > 0.0).then(|| Confirm {
            title: format!("Restart {}?", s.name),
            body: format!("{} — restart anyway?", on_line(n, &s.name)),
            confirm: "Restart".into(),
            destructive: true,
        }),
        _ => (!s.outside).then(|| Confirm { title: format!("Start {}?", s.name), body: LAN.into(), confirm: "Start".into(), destructive: false }),
    }
}

// ---- 3.6 the announcement -------------------------------------------------------------------------------------------

pub fn notice_text(actor: &str, action: &str, name: &str, first_name: Option<&str>) -> String {
    if let (Some(f), "start") = (first_name, action) {
        return format!("{actor} stopped {f} and started {name}");
    }
    let verb = match action {
        "start" => "started",
        "stop" => "stopped",
        _ => "restarted",
    };
    format!("{actor} {verb} {name}")
}

// ---- 3.7 the calendar link ------------------------------------------------------------------------------------------

/// "steam:<appid>" (1–9 digits, no leading zero) → the app id.
fn steam_app(game_id: &str) -> Option<f64> {
    let d = game_id.strip_prefix("steam:")?;
    if d.is_empty() || d.len() > 9 || !d.bytes().all(|b| b.is_ascii_digit()) || d.starts_with('0') {
        return None;
    }
    d.parse::<f64>().ok()
}

/// The server a calendar event (`{game?, gameId?}`) is for, or None (§3.7).
pub fn link_server<'a>(ev: &Value, st: &'a GameStatus) -> Option<&'a GameServer> {
    let app = ev.get("gameId").and_then(Value::as_str).and_then(steam_app);
    let g = ev.get("game").and_then(Value::as_str).map(|s| ascii_lower(ascii_trim(s))).unwrap_or_default();
    let cands: Vec<&GameServer> = st
        .servers
        .iter()
        .filter(|s| {
            if !s.calendar {
                return false;
            }
            if app.is_some() && s.steam_app_id == app {
                return true;
            }
            if g.is_empty() {
                return false;
            }
            let sg = ascii_lower(&s.game);
            let sn = ascii_lower(&s.name);
            (!sg.is_empty() && sg == g) || (!sn.is_empty() && sn == g)
        })
        .collect();
    cands.iter().find(|s| is_up(s)).or(cands.first()).copied()
}

const DAY_MS: f64 = 86_400_000.0;

/// The next calendar event for `server_id` among Up next's items (§3.7): the index into `events`, or None.
pub fn next_event_for(server_id: &str, events: &[Value], now: f64, st: &GameStatus) -> Option<usize> {
    let f = |e: &Value, k: &str| e.get(k).and_then(Value::as_f64).unwrap_or(f64::NAN);
    let s = |e: &Value, k: &str| e.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let mut idx: Vec<usize> = (0..events.len())
        .filter(|&i| {
            let e = &events[i];
            e.get("canceled") != Some(&Value::Bool(true))
                && e.get("kind").and_then(Value::as_str) != Some("focus")
                && f(e, "end") > now
                && f(e, "start") < now + 14.0 * DAY_MS
                && link_server(e, st).is_some_and(|x| x.id == server_id)
        })
        .collect();
    let cmp_f = |a: f64, b: f64| a.partial_cmp(&b).unwrap_or(Ordering::Equal);
    idx.sort_by(|&a, &b| {
        let (x, y) = (&events[a], &events[b]);
        cmp_f(f(x, "start"), f(y, "start"))
            .then_with(|| cmp_f(f(x, "end"), f(y, "end")))
            .then_with(|| s(x, "container").cmp(&s(y, "container")))
            .then_with(|| s(x, "id").cmp(&s(y, "id")))
    });
    idx.first().copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(servers: Value) -> GameStatus {
        read_status(&json!({"v": 1, "cap": 2, "memoryBudgetMB": 30720, "servers": servers})).unwrap()
    }

    #[test]
    fn chip_players_bars_and_the_switch() {
        let s = st(json!([
            {"id": "8e6c2f05", "name": "Valheim Vanilla", "game": "Valheim", "state": "running", "players": {"count": 3, "names": ["Darrow", "Sable"]}, "memory": {"usedMB": 2048, "limitMB": 8192}, "cpu": {"used": 18, "limit": 300}, "outside": true},
            {"id": "6c3f9a02", "name": "PZ", "game": "Project Zomboid", "state": "running", "memory": {"limitMB": 14336}},
            {"id": "a1c0de77", "name": "Abiotic Factor", "game": "Abiotic Factor", "state": "offline", "memory": {"limitMB": 6144}}
        ]));
        let v = &s.servers[0];
        assert_eq!(chip_of(v), ("online", "Online · 3 on".to_string()));
        assert_eq!(players_line(v), "Darrow, Sable +1");
        assert_eq!(who_line(v), "Darrow, Sable +1");
        assert_eq!(who_line(&s.servers[1]), "");
        let who = |players: Value, state: &str| {
            let one = st(json!([{"id": "a1c0de77", "name": "X", "state": state, "players": players, "memory": {"limitMB": 1024}}]));
            who_line(&one.servers[0])
        };
        assert_eq!(who(json!({"count": 0, "names": []}), "running"), "");
        assert_eq!(who(json!({"count": 3, "names": []}), "running"), "");
        assert_eq!(who(json!({"count": 3, "names": []}), "stopping"), "3 on");
        assert_eq!(cpu_text(v.cpu_used, v.cpu_limit), "6 %");
        assert_eq!(mem_text(v.mem_used, v.mem_limit), "2.0 / 8 GB");
        assert_eq!(plan_start(&s, "a1c0de77"), StartPlan::Cap(vec!["8e6c2f05".into(), "6c3f9a02".into()]));
        assert_eq!(plan_switch(&s, "a1c0de77", "8e6c2f05"), StartPlan::Ok);
        assert_eq!(confirm_of(&s, "stop", "8e6c2f05", None).unwrap().body, "3 people are on Valheim Vanilla — stop anyway?");
        assert_eq!(summary_of(&s), "Valheim Vanilla + PZ · 3 on");
        assert_eq!(link_server(&json!({"game": " valheim "}), &s).map(|x| x.id.as_str()), Some("8e6c2f05"));
        assert_eq!(notice_text("Blake", "restart", "Valheim Vanilla", None), "Blake restarted Valheim Vanilla");
    }

    #[test]
    fn the_next_event_of_a_linked_server() {
        let s = st(json!([
            {"id": "8e6c2f05", "name": "Valheim Vanilla", "game": "Valheim", "steamAppId": 892970, "state": "running", "calendar": true}
        ]));
        let now = 1_790_000_000_000.0;
        let evs = vec![json!({"container": "!l:x", "id": "ev-valheim", "title": "Valheim night", "kind": "session",
            "start": now + 2.0 * DAY_MS, "end": now + 2.0 * DAY_MS + 3.0 * 3_600_000.0, "canceled": false,
            "game": "Valheim", "gameId": "steam:892970"})];
        assert_eq!(next_event_for("8e6c2f05", &evs, now, &s), Some(0));
    }
}
