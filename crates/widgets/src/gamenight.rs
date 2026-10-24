//! Game night — port of the web's `src/widgets/gamenight.ts` (widget `gameNight`, contract WIDGETS.md §23, vectors
//! `testvectors/widgets/gamenight/*`): which calendar events are game nights (the Game Reel's sessions, kind `session`),
//! the countdown line, the Small line and the credit line. Pure.

use serde_json::{json, Value};

use crate::time::fmt_until;

pub const LOOK_AHEAD_MS: f64 = 14.0 * 86_400_000.0;
pub const DAY_MS: f64 = 86_400_000.0;

#[derive(Clone, Debug, PartialEq)]
pub struct NightItem {
    pub container: String,
    pub id: String,
    pub start: f64,
    pub end: f64,
}

impl NightItem {
    pub fn to_json(&self) -> Value {
        json!({ "container": self.container, "id": self.id, "kind": "session", "start": crate::num(self.start), "end": crate::num(self.end), "canceled": false })
    }
}

/// The calendar's kind: a non-empty string, else "session".
pub fn kind_of(kind: Option<&Value>) -> String {
    match kind.and_then(Value::as_str) {
        Some(k) if !k.is_empty() => k.to_string(),
        _ => "session".to_string(),
    }
}

/// The game nights to show, soonest first (see gamenight.ts `selectNights`).
pub fn select_nights(items: &Value, now: f64, max: usize) -> Vec<NightItem> {
    let mut out = Vec::new();
    for o in items.as_array().map(Vec::as_slice).unwrap_or(&[]) {
        if !o.is_object() {
            continue;
        }
        if kind_of(o.get("kind")) != "session" || o.get("canceled") == Some(&Value::Bool(true)) {
            continue;
        }
        let (Some(start), Some(end)) = (crate::finite(o.get("start")), crate::finite(o.get("end"))) else { continue };
        if start <= 0.0 || end <= now || start >= now + LOOK_AHEAD_MS {
            continue;
        }
        let s = |k: &str| o.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        out.push(NightItem { container: s("container"), id: s("id"), start, end });
    }
    out.sort_by(|a, b| {
        a.start.partial_cmp(&b.start).unwrap_or(core::cmp::Ordering::Equal)
            .then(a.end.partial_cmp(&b.end).unwrap_or(core::cmp::Ordering::Equal))
            .then_with(|| a.container.cmp(&b.container))
            .then_with(|| a.id.cmp(&b.id))
    });
    if max > 0 {
        out.truncate(max);
    }
    out
}

/// The countdown line.
pub fn night_when(start: f64, end: f64, now: f64, day_time: &str) -> String {
    if now >= start && now < end {
        return "now".into();
    }
    if start - now < DAY_MS {
        return fmt_until(start - now);
    }
    day_time.to_string()
}

/// The Small line after the header's title ("Valheim · in 2 h").
pub fn night_summary(has: bool, game: &str, when: &str) -> String {
    if !has {
        return "No game night planned".into();
    }
    let g = game.trim_matches(|c: char| c == ' ' || ('\t'..='\r').contains(&c));
    format!("{} · {when}", if g.is_empty() { "Game TBD" } else { g })
}

/// The credit line.
pub fn credit_text(credit: Option<f64>, mode: &str) -> String {
    if mode == "free" {
        return "Credits are off tonight".into();
    }
    let Some(c) = credit.filter(|c| c.is_finite()) else { return String::new() };
    let n = c.floor().max(0.0);
    if n > 0.0 {
        format!("Your credit: +{}", crate::js_number(n))
    } else {
        "Your credit: 0".into()
    }
}
