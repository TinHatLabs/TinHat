//! clocks — Friends' clocks' rules (contract WIDGETS.md §15, vectors testvectors/widgets/clocks/*): the rows (who, in
//! which order, whose row is yours, day or night, another day), the header's spread ("PT · ET +3 h"), the zone labels
//! and the automatic time-zone writes (opt-out since 2026-09-28: sharing starts ON). A line-for-line port of the
//! web's `src/widgets/clocks.ts` (widgets W1, 2026-09-28). Pure: the app turns an IANA zone into its current UTC offset
//! with QTimeZone and formats a time with its clock-mode formatter; everything else is here.

use std::collections::HashSet;

use serde_json::{json, Value};

use crate::num;
use crate::people::cmp_people;

pub const TZ_TYPE: &str = "dev.tinhat.presence_tz";
const MINUTE_MS: f64 = 60_000.0;
const DAY_MIN: f64 = 1440.0;
/// An automatic zone write at most this often per device.
pub const TZ_WRITE_GAP_MS: f64 = 10.0 * MINUTE_MS;

/// The short labels of the zones friends most often share; any other zone reads as its city ("Buenos Aires").
pub const ZONE_LABELS: &[(&str, &str)] = &[
    ("Pacific/Honolulu", "HST"),
    ("America/Anchorage", "AKT"),
    ("America/Los_Angeles", "PT"),
    ("America/Vancouver", "PT"),
    ("America/Tijuana", "PT"),
    ("America/Denver", "MT"),
    ("America/Edmonton", "MT"),
    ("America/Phoenix", "MST"),
    ("America/Chicago", "CT"),
    ("America/Winnipeg", "CT"),
    ("America/Mexico_City", "CST"),
    ("America/New_York", "ET"),
    ("America/Toronto", "ET"),
    ("America/Detroit", "ET"),
    ("America/Halifax", "AT"),
    ("America/Sao_Paulo", "BRT"),
    ("Europe/London", "UK"),
    ("Europe/Dublin", "IE"),
    ("Europe/Lisbon", "WET"),
    ("Europe/Paris", "CET"),
    ("Europe/Berlin", "CET"),
    ("Europe/Madrid", "CET"),
    ("Europe/Rome", "CET"),
    ("Europe/Amsterdam", "CET"),
    ("Europe/Stockholm", "CET"),
    ("Europe/Warsaw", "CET"),
    ("Europe/Athens", "EET"),
    ("Europe/Helsinki", "EET"),
    ("Europe/Moscow", "MSK"),
    ("Asia/Dubai", "GST"),
    ("Asia/Kolkata", "IST"),
    ("Asia/Singapore", "SGT"),
    ("Asia/Manila", "PHT"),
    ("Asia/Shanghai", "CST"),
    ("Asia/Tokyo", "JST"),
    ("Asia/Seoul", "KST"),
    ("Australia/Perth", "AWST"),
    ("Australia/Sydney", "AET"),
    ("Australia/Melbourne", "AET"),
    ("Pacific/Auckland", "NZT"),
    ("UTC", "UTC"),
    ("Etc/UTC", "UTC"),
];

/// The zones the manual picker offers (both apps, this order; "Automatic" = the device's zone comes first).
pub fn zone_choices() -> Vec<&'static str> {
    ZONE_LABELS.iter().map(|(z, _)| *z).filter(|z| *z != "Etc/UTC").collect()
}

fn zone_part(p: &str, first: bool) -> bool {
    let b = p.as_bytes();
    let ok = |c: &u8| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'+' | b'-');
    if first {
        !b.is_empty() && b.len() <= 31 && b[0].is_ascii_alphabetic() && b.iter().all(ok)
    } else {
        !b.is_empty() && b.len() <= 30 && b.iter().all(ok)
    }
}

/// An IANA-shaped zone name the server accepts (presence.mjs TZ_RE, ≤ 64 characters): "UTC", "Area/City",
/// "Area/Sub/City" — ASCII letters, digits, "_", "+", "-".
pub fn valid_zone(tz: &str) -> bool {
    if tz.len() > 64 {
        return false;
    }
    if tz == "UTC" {
        return true;
    }
    let parts: Vec<&str> = tz.split('/').collect();
    (2..=3).contains(&parts.len()) && zone_part(parts[0], true) && parts[1..].iter().all(|p| zone_part(p, false))
}

/// `GET /api/presence?tz=1`'s answer → mxid → zone for every row with a valid `tz` (a row may carry a zone and no
/// state). Other rows and malformed input → nothing.
pub fn zones_of(body: &Value) -> std::collections::BTreeMap<String, String> {
    let mut out = std::collections::BTreeMap::new();
    let Some(users) = body.get("users").and_then(Value::as_object) else { return out };
    for (mxid, row) in users {
        if !mxid.starts_with('@') || !mxid.contains(':') || !row.is_object() {
            continue;
        }
        if let Some(tz) = row.get("tz").and_then(Value::as_str).filter(|t| valid_zone(t)) {
            out.insert(mxid.clone(), tz.to_string());
        }
    }
    out
}

/// A zone's short label: the table's; "Etc/GMT+10" (POSIX sign, 10 h BEHIND) → "UTC−10"; else its last part with "_"
/// as spaces, runs of spaces as one, no space at either end ("America/Argentina/Buenos_Aires" → "Buenos Aires"); else
/// the zone as written.
pub fn zone_label(tz: &str) -> String {
    if let Some((_, l)) = ZONE_LABELS.iter().find(|(z, _)| *z == tz) {
        return (*l).to_string();
    }
    if let Some(rest) = tz.strip_prefix("Etc/GMT") {
        let mut cs = rest.chars();
        if let Some(sign @ ('+' | '-')) = cs.next() {
            let digits: &str = cs.as_str();
            if (1..=2).contains(&digits.len()) && digits.bytes().all(|b| b.is_ascii_digit()) {
                let n: u32 = digits.parse().unwrap_or(0);
                if digits == "0" {
                    return "UTC".into();
                }
                return format!("UTC{}{}", if sign == '+' { '\u{2212}' } else { '+' }, n);
            }
        }
    }
    let last = tz.rsplit('/').next().unwrap_or("").replace('_', " ");
    let mut collapsed = String::with_capacity(last.len());
    let mut prev_space = false;
    for c in last.chars() {
        if c == ' ' {
            if !prev_space {
                collapsed.push(' ');
            }
            prev_space = true;
        } else {
            collapsed.push(c);
            prev_space = false;
        }
    }
    let t = collapsed.strip_prefix(' ').unwrap_or(&collapsed);
    let t = t.strip_suffix(' ').unwrap_or(t);
    if t.is_empty() {
        tz.to_string()
    } else {
        t.to_string()
    }
}

/// A picker entry: "PT · America/Los_Angeles" (a label equal to the zone shows once: "UTC").
pub fn zone_choice_text(tz: &str) -> String {
    let l = zone_label(tz);
    if l == tz {
        tz.to_string()
    } else {
        format!("{l} · {tz}")
    }
}

/// A difference in minutes as "+6 h", "−3 h", "+5 h 30 m", "+45 m", "±0 h" (U+2212 for minus).
pub fn fmt_offset(diff_min: f64) -> String {
    let d = diff_min.trunc() as i64;
    if d == 0 {
        return "±0 h".into();
    }
    let sign = if d > 0 { "+" } else { "\u{2212}" };
    let a = d.abs();
    let (h, m) = (a / 60, a % 60);
    if h == 0 {
        return format!("{sign}{m} m");
    }
    if m != 0 {
        format!("{sign}{h} h {m} m")
    } else {
        format!("{sign}{h} h")
    }
}

/// Local minutes of the day (0…1439) of `now` in a zone `offset_min` minutes ahead of UTC.
pub fn local_minutes(now: f64, offset_min: f64) -> f64 {
    let m = ((now + offset_min * MINUTE_MS) / MINUTE_MS).floor();
    ((m % DAY_MIN) + DAY_MIN) % DAY_MIN
}

/// Night = 22:00 to 07:00 local (a moon), day otherwise (a sun).
pub fn is_night(local_min: f64) -> bool {
    local_min >= 22.0 * 60.0 || local_min < 7.0 * 60.0
}

fn local_day(now: f64, offset_min: f64) -> f64 {
    ((now + offset_min * MINUTE_MS) / (DAY_MIN * MINUTE_MS)).floor()
}

pub const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

#[derive(Clone, Debug)]
pub struct ClockPerson {
    pub id: String,
    pub name: String,
    pub tz: String,
    /// the zone's current offset from UTC in minutes; None = the app doesn't know the zone
    pub offset_min: Option<f64>,
}

impl ClockPerson {
    /// `{id, name, tz, offsetMin}` (the web's ClockPerson; offsetMin a finite number or not).
    pub fn from_json(v: &Value) -> ClockPerson {
        let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        ClockPerson { id: s("id"), name: s("name"), tz: s("tz"), offset_min: crate::finite(v.get("offsetMin")) }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClockRow {
    pub id: String,
    pub tz: String,
    pub label: String,
    pub offset_min: f64,
    /// HH·60 + MM local
    pub minutes: f64,
    pub night: bool,
    pub me: bool,
    /// the weekday ("Tue") when the local date is not the viewer's, else ""
    pub day: String,
    /// the offset from the viewer's zone ("+6 h"; "" on the viewer's own row)
    pub diff: String,
}

impl ClockRow {
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id, "tz": self.tz, "label": self.label, "offsetMin": num(self.offset_min),
            "minutes": num(self.minutes), "night": self.night, "me": self.me, "day": self.day, "diff": self.diff,
        })
    }
}

/// The rows: every person whose zone the app knows, each id once (the first), sorted by offset (west first), then
/// `cmp_people`; the viewer's own row is marked; a row's weekday shows when its local date differs from the viewer's.
pub fn clock_rows(people: &[ClockPerson], me: &str, my_offset_min: f64, now: f64) -> Vec<ClockRow> {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut keep: Vec<(&ClockPerson, f64)> = Vec::new();
    for p in people {
        let Some(off) = p.offset_min.filter(|x| x.is_finite()) else { continue };
        if !seen.insert(p.id.as_str()) {
            continue;
        }
        keep.push((p, off));
    }
    keep.sort_by(|a, b| {
        let d = a.1 - b.1;
        if d != 0.0 {
            if d < 0.0 {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Greater
            }
        } else {
            cmp_people(&a.0.id, &a.0.name, &b.0.id, &b.0.name)
        }
    });
    let my_day = local_day(now, my_offset_min);
    keep.into_iter()
        .map(|(p, o)| {
            let off = o.trunc();
            let minutes = local_minutes(now, off);
            let day = local_day(now, off);
            let wd = ((((day + 4.0) % 7.0) + 7.0) % 7.0) as usize;
            ClockRow {
                id: p.id.clone(),
                tz: p.tz.clone(),
                label: zone_label(&p.tz),
                offset_min: off,
                minutes,
                night: is_night(minutes),
                me: p.id == me,
                day: if day == my_day { String::new() } else { WEEKDAYS[wd].to_string() },
                diff: if p.id == me { String::new() } else { fmt_offset(off - my_offset_min) },
            }
        })
        .collect()
}

/// The header's spread: the viewer's zone label, then the zone furthest from it (largest |difference|; ties: the later
/// row) with the difference — "PT · ET +3 h". Nobody elsewhere → "<label> · everyone here"; no rows → the label alone.
pub fn spread_label(rows: &[ClockRow], my_tz: &str, my_offset_min: f64) -> String {
    let mine = zone_label(my_tz);
    let mut best: Option<&ClockRow> = None;
    let mut best_abs = 0.0;
    for r in rows {
        let a = (r.offset_min - my_offset_min).abs();
        if a > 0.0 && a >= best_abs {
            best = Some(r);
            best_abs = a;
        }
    }
    if let Some(b) = best {
        return format!("{mine} · {} {}", b.label, fmt_offset(b.offset_min - my_offset_min));
    }
    if rows.iter().any(|r| !r.me) {
        format!("{mine} · everyone here")
    } else {
        mine
    }
}

// ---- the account data (opt-out: sharing starts on) ---------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct TzPref {
    pub share: bool,
    pub tz: String,
    /// follow the device's zone (never picked by hand)
    pub auto: bool,
}

impl TzPref {
    pub fn to_json(&self) -> Value {
        json!({ "share": self.share, "tz": self.tz, "auto": self.auto, "v": 1 })
    }
}

/// The stored `dev.tinhat.presence_tz` → a clean pref, or None when absent / not an object / no string zone.
pub fn read_tz_pref(v: Option<&Value>) -> Option<TzPref> {
    let o = v?.as_object()?;
    let tz = o.get("tz")?.as_str()?;
    if tz.is_empty() {
        return None;
    }
    Some(TzPref {
        share: o.get("share") == Some(&Value::Bool(true)),
        tz: tz.to_string(),
        auto: o.get("auto") != Some(&Value::Bool(false)),
    })
}

/// What this device writes when it starts or comes back to the foreground (see the web's `tzAutoWrite`): nothing
/// stored (absent, null or `{}`) → sharing ON with the device's zone; stored, sharing, automatic, another zone and this
/// device's last automatic write ≥ 10 min ago → the device's zone (the device used last wins); else None.
pub fn tz_auto_write(stored: Option<&Value>, device_tz: &str, now: f64, last_write_at: Option<f64>) -> Option<TzPref> {
    if !valid_zone(device_tz) {
        return None;
    }
    let absent = match stored {
        None | Some(Value::Null) => true,
        Some(Value::Object(m)) => m.is_empty(),
        _ => false,
    };
    let on = TzPref { share: true, tz: device_tz.to_string(), auto: true };
    if absent {
        return Some(on);
    }
    let p = read_tz_pref(stored)?;
    if !p.share || !p.auto || p.tz == device_tz {
        return None;
    }
    if let Some(t) = last_write_at {
        if now - t < TZ_WRITE_GAP_MS {
            return None;
        }
    }
    Some(on)
}
