//! The Memories widget — port of the web's `src/widgets/memories.ts` (widget `memories`, contract WIDGETS.md §21,
//! vectors `testvectors/widgets/memories/*`): the latest recordings of the rooms you're in (the `/api/vods/sessions`
//! answers the Memories window reads), newest first; the row texts; the Small line; the poster frame (the Memories
//! window's own rule). Pure: the app fetches the sessions and builds the signed thumbnail URL from the track this names.

use serde_json::{json, Value};

use crate::time::{fmt_ago, fmt_dur};

/// "New" = recorded within this many ms (the Small line's count).
pub const NEW_MS: f64 = 7.0 * 86_400_000.0;

#[derive(Clone, Debug, PartialEq)]
pub struct MemItem {
    pub room: String,
    pub id: String,
    pub title: String,
    pub start: f64,
    pub end: Option<f64>,
    pub state: &'static str,
    pub dur_ms: Option<f64>,
    pub poster: Option<(String, f64)>,
    pub people: Vec<String>,
}

impl MemItem {
    pub fn to_json(&self) -> Value {
        json!({
            "room": self.room, "id": self.id, "title": self.title, "start": crate::num(self.start),
            "end": self.end.map(crate::num).unwrap_or(Value::Null), "state": self.state,
            "durMs": self.dur_ms.map(crate::num).unwrap_or(Value::Null),
            "poster": self.poster.as_ref().map(|(f, t)| json!({ "file": f, "t": crate::num(*t) })).unwrap_or(Value::Null),
            "people": self.people,
        })
    }
    pub fn from_json(v: &Value) -> Option<MemItem> {
        let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let num = |k: &str| crate::finite(v.get(k));
        Some(MemItem {
            room: s("room"),
            id: s("id"),
            title: s("title"),
            start: num("start")?,
            end: num("end"),
            state: match v.get("state").and_then(Value::as_str) {
                Some("live") => "live",
                Some("processing") => "processing",
                _ => "ready",
            },
            dur_ms: num("durMs"),
            poster: v.get("poster").and_then(|p| Some((p.get("file")?.as_str()?.to_string(), crate::finite(p.get("t"))?))),
            people: v.get("people").and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default(),
        })
    }
}

struct Track {
    file: String,
    identity: String,
    kind: String,
    start: f64,
}

fn tracks_of(s: &Value) -> Vec<Track> {
    let mut out = Vec::new();
    for t in s.get("tracks").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]) {
        if !t.is_object() {
            continue;
        }
        let file = t.get("file").and_then(Value::as_str).unwrap_or("");
        if file.is_empty() {
            continue;
        }
        out.push(Track {
            file: file.to_string(),
            identity: t.get("identity").and_then(Value::as_str).unwrap_or("").to_string(),
            kind: t.get("kind").and_then(Value::as_str).unwrap_or("").to_string(),
            start: crate::finite(t.get("start_ms")).unwrap_or(0.0),
        });
    }
    out
}

fn is_video(t: &Track) -> bool {
    t.kind == "share" || t.kind == "cam"
}

/// The Memories window's poster rule (see memories.ts `posterAt`).
pub fn poster_at(session: &Value) -> Option<(String, f64)> {
    if !session.is_object() {
        return None;
    }
    let end = crate::finite(session.get("end_ms"))?;
    let start = crate::finite(session.get("start_ms"))?;
    let mut vids: Vec<Track> = tracks_of(session).into_iter().filter(is_video).collect();
    vids.sort_by(|a, b| {
        let ka = if a.kind == "share" { 0 } else { 1 };
        let kb = if b.kind == "share" { 0 } else { 1 };
        ka.cmp(&kb).then_with(|| a.file.cmp(&b.file))
    });
    let mut best: Option<&Track> = None;
    for t in &vids {
        match best {
            None => best = Some(t),
            Some(b) => {
                let d = t.start - b.start;
                if d < -500.0 || (d.abs() <= 500.0 && t.kind == "share" && b.kind != "share") {
                    best = Some(t);
                }
            }
        }
    }
    let best = best?;
    let dur = crate::js_max(1.0, (end - start) / 1000.0);
    let off = crate::js_max(0.0, (best.start - start) / 1000.0);
    let t0 = crate::js_min(dur - 1.0, crate::js_max(off + 1.0, crate::js_min(off + 8.0, dur / 3.0))).floor();
    Some((best.file.clone(), crate::js_max(0.0, t0 - off.floor())))
}

fn people_of(s: &Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut add = |u: &str| {
        if !u.is_empty() && !out.iter().any(|x| x == u) {
            out.push(u.to_string());
        }
    };
    if let Some(list) = s.get("people").and_then(Value::as_array) {
        for p in list {
            add(p.as_str().unwrap_or(""));
        }
    } else {
        for t in tracks_of(s) {
            // /^(@[^:]+:[^:]+)/
            let id = t.identity.as_str();
            if let Some(rest) = id.strip_prefix('@') {
                let mut parts = rest.splitn(3, ':');
                if let (Some(a), Some(b)) = (parts.next(), parts.next()) {
                    if !a.is_empty() && !b.is_empty() {
                        add(&format!("@{a}:{b}"));
                    }
                }
            }
        }
    }
    out.truncate(5);
    out
}

/// Every room's sessions → the memories, newest first (see memories.ts `memoriesOf`). `rooms` = `[{room, name,
/// sessions}]`.
pub fn memories_of(rooms: &Value, max: usize) -> Vec<MemItem> {
    let mut out = Vec::new();
    let mut seen_room: Vec<String> = Vec::new();
    for r in rooms.as_array().map(Vec::as_slice).unwrap_or(&[]) {
        let room = r.get("room").and_then(Value::as_str).unwrap_or("");
        if !r.is_object() || room.is_empty() || seen_room.iter().any(|x| x == room) {
            continue;
        }
        seen_room.push(room.to_string());
        let name = r.get("name").and_then(Value::as_str).unwrap_or("");
        let mut seen: Vec<String> = Vec::new();
        for s in r.get("sessions").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]) {
            let id = s.get("id").and_then(Value::as_str).unwrap_or("");
            let Some(start) = crate::finite(s.get("start_ms")) else { continue };
            if !s.is_object() || id.is_empty() || seen.iter().any(|x| x == id) {
                continue;
            }
            seen.push(id.to_string());
            if s.get("empty") == Some(&Value::Bool(true)) || tracks_of(s).is_empty() {
                continue;
            }
            let end = crate::finite(s.get("end_ms"));
            let state = if end.is_none() {
                "live"
            } else if s.get("ready") == Some(&Value::Bool(false)) {
                "processing"
            } else {
                "ready"
            };
            let rn = s.get("room_name").and_then(Value::as_str).unwrap_or("");
            let title = if !rn.is_empty() { rn } else if !name.is_empty() { name } else { "Memory" };
            out.push(MemItem {
                room: room.to_string(),
                id: id.to_string(),
                title: title.to_string(),
                start,
                end,
                state,
                dur_ms: end.map(|e| crate::js_max(0.0, e - start)),
                poster: poster_at(s),
                people: people_of(s),
            });
        }
    }
    out.sort_by(|a, b| {
        b.start.partial_cmp(&a.start).unwrap_or(core::cmp::Ordering::Equal).then_with(|| a.room.cmp(&b.room)).then_with(|| a.id.cmp(&b.id))
    });
    if max > 0 {
        out.truncate(max);
    }
    out
}

/// A row's second line: "Recording now", "Processing…", or "<ago> · <length>".
pub fn mem_line(m: &MemItem, now: f64) -> String {
    match m.state {
        "live" => "Recording now".into(),
        "processing" => "Processing…".into(),
        _ => format!("{} · {}", fmt_ago(now - m.end.unwrap_or(m.start)), fmt_dur(m.dur_ms.unwrap_or(0.0))),
    }
}

/// The Small line.
pub fn mem_summary(items: &[MemItem], now: f64) -> String {
    let Some(first) = items.first() else { return "No memories yet".into() };
    if first.state == "live" {
        return "Recording now".into();
    }
    let n = items.iter().filter(|m| m.start >= now - NEW_MS).count();
    let ago = fmt_ago(now - first.end.unwrap_or(first.start));
    if n > 0 {
        format!("{n} new {} · latest {ago}", if n == 1 { "memory" } else { "memories" })
    } else {
        format!("Latest {ago}")
    }
}
