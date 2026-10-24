//! The Up next widget's clock maths and the widgets' time labels — port of the web's `src/widgets/time.ts` (contract
//! WIDGETS.md §4 + WIDGETS-UI.md; vectors `testvectors/widgets/time/*`): the countdown ring, the event phases, which
//! events show, and the duration / "in …" / "… ago" labels every widget uses. Integer maths on doubles, no locale
//! (wall-clock times like "7:30 PM" are the apps' clock-mode formatters and are not in the vectors).

use serde_json::{Map, Value};

use crate::{js_max, js_min, js_number, num};

pub const MINUTE_MS: f64 = 60_000.0;
pub const DAY_MINUTES: f64 = 1440.0;
/// The ring fills over the last 24 h before the start.
pub const RING_MINUTES: f64 = DAY_MINUTES;
/// "Starting soon" (accent edge + Join) from 10 minutes before the start.
pub const SOON_MINUTES: f64 = 10.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Later,
    Today,
    Soon,
    Live,
    Over,
}

impl Phase {
    pub fn as_str(self) -> &'static str {
        match self {
            Phase::Later => "later",
            Phase::Today => "today",
            Phase::Soon => "soon",
            Phase::Live => "live",
            Phase::Over => "over",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Ring {
    pub phase: Phase,
    /// whole minutes to the start, rounded up (0 once it has started)
    pub minutes_left: f64,
    /// the ring's filled part in minutes of RING_MINUTES (0 … 1440): the angle is fill / 1440 × 360°, from 12 o'clock
    /// clockwise
    pub fill: f64,
    /// when the ring / phase / label next change (the one timer to arm), None when nothing will
    pub next_at: Option<f64>,
}

impl Ring {
    pub fn to_json(&self) -> Value {
        let mut o = Map::new();
        o.insert("phase".into(), Value::String(self.phase.as_str().into()));
        o.insert("minutesLeft".into(), num(self.minutes_left));
        o.insert("fill".into(), num(self.fill));
        o.insert("nextAt".into(), self.next_at.map(num).unwrap_or(Value::Null));
        Value::Object(o)
    }
}

/// The countdown of an event that runs [start, end): later (> 24 h ahead) → today (≤ 24 h: the ring fills a minute at a
/// time) → soon (≤ 10 min) → live (started) → over (ended). The ring ticks once a minute: next_at is the next minute
/// boundary before the start (start − (minutesLeft − 1) min), then the end.
pub fn ring_of(start: f64, end: f64, now: f64) -> Ring {
    if now >= end {
        return Ring { phase: Phase::Over, minutes_left: 0.0, fill: RING_MINUTES, next_at: None };
    }
    if now >= start {
        return Ring { phase: Phase::Live, minutes_left: 0.0, fill: RING_MINUTES, next_at: Some(end) };
    }
    let m = ((start - now) / MINUTE_MS).ceil();
    let fill = RING_MINUTES - js_min(m, RING_MINUTES);
    let phase = if m <= SOON_MINUTES {
        Phase::Soon
    } else if m <= RING_MINUTES {
        Phase::Today
    } else {
        Phase::Later
    };
    Ring { phase, minutes_left: m, fill, next_at: Some(start - (m - 1.0) * MINUTE_MS) }
}

// ---- labels ---------------------------------------------------------------------------------------------------------

/// "< 1 m", "45 m", "2 h", "2 h 10 m", "1 d", "1 d 4 h" (floored to the minute).
pub fn fmt_dur(ms: f64) -> String {
    let m = (js_max(0.0, ms) / MINUTE_MS).floor();
    if m < 1.0 {
        return "< 1 m".into();
    }
    spell(m)
}

fn spell(m: f64) -> String {
    if m < 60.0 {
        return format!("{} m", js_number(m));
    }
    let h = (m / 60.0).floor();
    let mm = m % 60.0;
    if h < 24.0 {
        return if mm != 0.0 { format!("{} h {} m", js_number(h), js_number(mm)) } else { format!("{} h", js_number(h)) };
    }
    let d = (h / 24.0).floor();
    let hh = h % 24.0;
    if hh != 0.0 {
        format!("{} d {} h", js_number(d), js_number(hh))
    } else {
        format!("{} d", js_number(d))
    }
}

/// Time until something: "now" (≤ 0), "in 1 m" … "in 59 m", "in 2 h 10 m", "in 1 d 4 h" (minutes rounded UP, so the
/// label never says "in 0 m" before the start).
pub fn fmt_until(ms: f64) -> String {
    if ms <= 0.0 {
        return "now".into();
    }
    format!("in {}", spell((ms / MINUTE_MS).ceil()))
}

/// Time since something: "just now" (< 1 min), "12 m ago", "3 h ago", "2 d ago", "3 w ago" (floored).
pub fn fmt_ago(ms: f64) -> String {
    let m = (js_max(0.0, ms) / MINUTE_MS).floor();
    if m < 1.0 {
        return "just now".into();
    }
    if m < 60.0 {
        return format!("{} m ago", js_number(m));
    }
    let h = (m / 60.0).floor();
    if h < 24.0 {
        return format!("{} h ago", js_number(h));
    }
    let d = (h / 24.0).floor();
    if d < 7.0 {
        return format!("{} d ago", js_number(d));
    }
    format!("{} w ago", js_number((d / 7.0).floor()))
}

// ---- which events Up next shows -------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct UpNextItem {
    /// calendar container (space or room id)
    pub container: String,
    /// event id (state key)
    pub id: String,
    pub kind: String,
    /// effective start / end (the calendar's effectiveStart / effectiveEnd: end = start + 3 h when unset)
    pub start: f64,
    pub end: f64,
    pub canceled: bool,
}

impl UpNextItem {
    /// Read a reference-shaped item (missing numbers = NaN — they fail every comparison, like undefined does).
    pub fn from_json(v: &Value) -> UpNextItem {
        let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let n = |k: &str| v.get(k).and_then(Value::as_f64).unwrap_or(f64::NAN);
        let truthy = match v.get("canceled") {
            None | Some(Value::Null) => false,
            Some(Value::Bool(b)) => *b,
            Some(Value::Number(x)) => x.as_f64().map(|f| f != 0.0 && !f.is_nan()).unwrap_or(true),
            Some(Value::String(t)) => !t.is_empty(),
            Some(_) => true,
        };
        UpNextItem { container: s("container"), id: s("id"), kind: s("kind"), start: n("start"), end: n("end"), canceled: truthy }
    }
}

/// The events Up next lists, in order: not cancelled, not the "focus" kind (Main game), start > 0, not over (end > now),
/// starting before now + `days` days; sorted by start, end, container, id (code-point order); the first `max` (0 = all).
/// M shows 3, S the first, L all of them (the widget's `days` setting, 1–14, default 7).
pub fn select_up_next(items: &[UpNextItem], now: f64, days: f64, max: f64) -> Vec<UpNextItem> {
    let horizon = now + js_max(1.0, js_min(14.0, days.floor())) * DAY_MINUTES * MINUTE_MS;
    let mut keep: Vec<UpNextItem> = items
        .iter()
        .filter(|e| !e.canceled && e.kind != "focus" && e.start > 0.0 && e.end > now && e.start < horizon)
        .cloned()
        .collect();
    keep.sort_by(|a, b| {
        let d = a.start - b.start;
        if d != 0.0 && !d.is_nan() {
            return if d < 0.0 { std::cmp::Ordering::Less } else { std::cmp::Ordering::Greater };
        }
        let d = a.end - b.end;
        if d != 0.0 && !d.is_nan() {
            return if d < 0.0 { std::cmp::Ordering::Less } else { std::cmp::Ordering::Greater };
        }
        a.container.cmp(&b.container).then_with(|| a.id.cmp(&b.id))
    });
    if max > 0.0 {
        let n = if max.is_infinite() { keep.len() } else { max.trunc() as usize };
        keep.truncate(n);
    }
    keep
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hand_checks_of_the_reference() {
        const T0: f64 = 1_790_000_000_000.0;
        const MIN: f64 = 60_000.0;
        const H: f64 = 3_600_000.0;
        let r = ring_of(T0 + 130.0 * MIN, T0 + 4.0 * H, T0);
        assert_eq!(r, Ring { phase: Phase::Today, minutes_left: 130.0, fill: 1310.0, next_at: Some(T0 + MIN) });
        assert_eq!(fmt_until(130.0 * MIN), "in 2 h 10 m");
        assert_eq!(fmt_dur(25.0 * MIN), "25 m");
        assert_eq!(fmt_ago(3.0 * H), "3 h ago");
    }
}
