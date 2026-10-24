//! mentions — the Mentions widget's list (contract WIDGETS.md §13, vectors testvectors/widgets/mentions/*): each app
//! keeps, on this device only, the recent events its own push-rule evaluation highlighted (after decryption), as ids —
//! never their text. A line-for-line port of the web's `src/widgets/mentions.ts` (widgets W1, 2026-09-28): strings in
//! code-point order (Rust's `str` order), numbers as doubles floored like `Math.floor`.

use std::collections::HashSet;

use serde_json::{json, Value};

/// The list keeps the newest this many.
pub const MENTIONS_MAX: usize = 50;

#[derive(Clone, Debug, PartialEq)]
pub struct Mention {
    /// room id
    pub r: String,
    /// event id
    pub e: String,
    /// sender
    pub s: String,
    /// origin_server_ts (ms)
    pub t: f64,
    /// opened (clicked, or its room shown in this app since)
    pub seen: bool,
}

impl Mention {
    pub fn new(r: &str, e: &str, s: &str, t: f64) -> Self {
        Mention { r: r.to_string(), e: e.to_string(), s: s.to_string(), t, seen: false }
    }
    pub fn to_json(&self) -> Value {
        let mut o = json!({ "r": self.r, "e": self.e, "s": self.s, "t": crate::num(self.t) });
        if self.seen {
            o["seen"] = Value::Bool(true);
        }
        o
    }
}

/// Newest first: t descending, then e in code-point order.
fn order(a: &Mention, b: &Mention) -> std::cmp::Ordering {
    b.t.partial_cmp(&a.t).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.e.cmp(&b.e))
}

fn finish(mut out: Vec<Mention>) -> Vec<Mention> {
    out.sort_by(order);
    out.truncate(MENTIONS_MAX);
    out
}

/// A stored value → a clean list (the web's `normalizeMentions`): objects whose `r` starts with "!", `e` with "$", `s`
/// with "@" and whose `t` is a finite number ≥ 0 (floored); `seen` only when `true`; each event once (the first wins);
/// newest first; at most [`MENTIONS_MAX`]. Not an array → [].
pub fn normalize_mentions(v: &Value) -> Vec<Mention> {
    let Some(list) = v.as_array() else { return Vec::new() };
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for x in list {
        let Some(o) = x.as_object() else { continue };
        let (Some(r), Some(e), Some(s)) = (o.get("r").and_then(Value::as_str), o.get("e").and_then(Value::as_str), o.get("s").and_then(Value::as_str)) else {
            continue;
        };
        if !r.starts_with('!') || !e.starts_with('$') || !s.starts_with('@') {
            continue;
        }
        let Some(t) = o.get("t").and_then(Value::as_f64).filter(|t| t.is_finite() && *t >= 0.0) else { continue };
        if !o.get("t").map(Value::is_number).unwrap_or(false) || seen.contains(e) {
            continue;
        }
        seen.insert(e.to_string());
        out.push(Mention { r: r.to_string(), e: e.to_string(), s: s.to_string(), t: t.floor(), seen: o.get("seen") == Some(&Value::Bool(true)) });
    }
    finish(out)
}

/// The list as its JSON (the stored form and the vectors' form).
pub fn mentions_json(list: &[Mention]) -> Value {
    Value::Array(list.iter().map(Mention::to_json).collect())
}

/// A new highlight: added unless the event is already listed (then only re-normalized — its seen flag stays).
pub fn add_mention(list: &[Mention], m: Mention) -> Vec<Mention> {
    let mut v: Vec<Mention> = list.to_vec();
    if !v.iter().any(|x| x.e == m.e) {
        v.push(m);
    }
    renormalize(v)
}

/// normalize() over an in-memory list (the same rules as the stored form).
fn renormalize(v: Vec<Mention>) -> Vec<Mention> {
    normalize_mentions(&mentions_json(&v))
}

/// Which entries a mark-seen touches: one event, every entry of a room up to a time, or all.
#[derive(Clone, Debug, Default)]
pub struct SeenSel {
    pub e: Option<String>,
    pub r: Option<String>,
    pub up_to: Option<f64>,
}

impl SeenSel {
    pub fn from_json(v: &Value) -> Self {
        SeenSel {
            e: v.get("e").and_then(Value::as_str).map(String::from),
            r: v.get("r").and_then(Value::as_str).map(String::from),
            up_to: v.get("upTo").and_then(Value::as_f64),
        }
    }
}

/// Mark entries seen (the web's `markSeen`): `e` = that event; `r` = every entry of that room with t ≤ `up_to` (all of
/// the room's when absent); neither = every entry.
pub fn mark_seen(list: &[Mention], sel: &SeenSel) -> Vec<Mention> {
    let v: Vec<Mention> = list
        .iter()
        .map(|m| {
            let hit = match (&sel.e, &sel.r) {
                (Some(e), _) => &m.e == e,
                (None, Some(r)) => &m.r == r && sel.up_to.map_or(true, |u| m.t <= u),
                (None, None) => true,
            };
            let mut m = m.clone();
            if hit {
                m.seen = true;
            }
            m
        })
        .collect();
    renormalize(v)
}

pub fn unseen_count(list: &[Mention]) -> usize {
    list.iter().filter(|m| !m.seen).count()
}

/// The Small / collapsed line (the web's `mentionSummary`).
pub fn mention_summary(list: &[Mention], room_name: impl Fn(&str) -> String) -> String {
    let all = renormalize(list.to_vec());
    if all.is_empty() {
        return "No mentions".to_string();
    }
    let unseen: Vec<&Mention> = all.iter().filter(|m| !m.seen).collect();
    if unseen.is_empty() {
        return "No new mentions".to_string();
    }
    let head = if unseen.len() == 1 { "1 new mention".to_string() } else { format!("{} new mentions", unseen.len()) };
    let name = room_name(&unseen[0].r);
    if name.is_empty() {
        head
    } else {
        format!("{head} · {name}")
    }
}
