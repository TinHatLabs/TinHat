//! "Is it just me?" — port of the web's `src/widgets/justme.ts` (widget `justMe`, contract WIDGETS.md §20, vectors
//! `testvectors/widgets/justme/*`): what one check of the server's services, measured from THIS device, says — a green /
//! amber / red light per service with a one-line reason, your ping, the Small line and the friendly sentence shown when
//! something is red; plus the Large size's one-minute sparkline slots. Pure: the app runs the requests and hands the
//! numbers in (as JSON, the shape the vectors hold).

use serde_json::{json, Value};

use crate::quality::r;
use crate::{finite, js_number};

/// A probe that answered in this many ms or more is slow (amber).
pub const SLOW_MS: f64 = 1000.0;
/// A probe with no answer after this long is not reachable (red) — the app aborts the request here.
pub const TIMEOUT_MS: u64 = 8000;
/// No successful sync for this long = messages are late (amber).
pub const SYNC_LATE_MS: f64 = 60_000.0;
/// No successful sync for this long = messages aren't arriving (red).
pub const SYNC_STUCK_MS: f64 = 120_000.0;
/// Your ping's bands = Call stats' round-trip bands: under 150 ms good, under 400 ms ok, else poor.
pub const PING_OK_MS: f64 = 150.0;
pub const PING_POOR_MS: f64 = 400.0;
/// The sparkline: this many one-minute slots, the newest ending at the latest check.
pub const SPARK_SLOTS: usize = 10;
pub const SLOT_MS: f64 = 60_000.0;

/// The services, in row order.
pub const SERVICES: [&str; 4] = ["chat", "calls", "media", "tinhat"];

fn service_name(id: &str) -> &'static str {
    match id {
        "chat" => "Chat",
        "calls" => "Calls",
        "media" => "Media",
        "tinhat" => "Extras",
        "ping" => "Your ping",
        _ => "",
    }
}

fn noun(id: &str) -> &'static str {
    match id {
        "chat" => "chat",
        "calls" => "calls",
        "media" => "media",
        "tinhat" => "the extras",
        _ => "",
    }
}

/// One request's outcome (status null = no answer; ms = how long it took).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Probe {
    pub status: Option<f64>,
    pub ms: Option<f64>,
}

impl Probe {
    pub fn from_json(v: &Value) -> Probe {
        Probe { status: finite(v.get("status")), ms: finite(v.get("ms")) }
    }
    pub fn to_json(&self) -> Value {
        json!({ "status": self.status.map(crate::num).unwrap_or(Value::Null), "ms": self.ms.map(crate::num).unwrap_or(Value::Null) })
    }
    /// Answered: a status 100–599 and a time ≥ 0.
    pub fn answered(&self) -> bool {
        matches!(self.status, Some(s) if (100.0..=599.0).contains(&s)) && matches!(self.ms, Some(m) if m >= 0.0)
    }
}

/// One light.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub id: &'static str,
    pub band: &'static str,
    pub cause: &'static str,
    pub ms: Option<f64>,
    pub reason: String,
}

impl Row {
    fn new(id: &'static str, band: &'static str, cause: &'static str, ms: Option<f64>, reason: impl Into<String>) -> Row {
        Row { id, band, cause, ms, reason: reason.into() }
    }
    pub fn name(&self) -> &'static str {
        service_name(self.id)
    }
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id, "name": self.name(), "band": self.band, "cause": self.cause,
            "ms": self.ms.map(crate::num).unwrap_or(Value::Null), "reason": self.reason,
        })
    }
}

/// The whole check.
#[derive(Clone, Debug, PartialEq)]
pub struct Check {
    pub rows: Vec<Row>,
    pub ping: Row,
    pub tone: &'static str,
    pub verdict: String,
    pub note: String,
}

impl Check {
    pub fn to_json(&self) -> Value {
        json!({
            "rows": self.rows.iter().map(Row::to_json).collect::<Vec<_>>(),
            "ping": self.ping.to_json(),
            "verdict": { "tone": self.tone, "text": self.verdict },
            "note": self.note,
        })
    }
}

/// "42 ms", "1.2 s", "12 s".
pub fn fmt_ms(ms: f64) -> String {
    let m = r(ms);
    if m < 1000.0 {
        return format!("{} ms", js_number(m));
    }
    let t = ((m + 50.0) / 100.0).floor();
    if t < 100.0 {
        return format!("{}.{} s", js_number((t / 10.0).floor()), js_number(t % 10.0));
    }
    format!("{} s", js_number(((m + 500.0) / 1000.0).floor()))
}

/// The median of the pings' times, a failed ping counting as endless (index floor(n / 2) of the sorted list).
pub fn ping_of(pings: &[Probe]) -> Option<f64> {
    if pings.is_empty() {
        return None;
    }
    let mut t: Vec<f64> = pings.iter().map(|p| if p.answered() { r(p.ms.unwrap_or(0.0)) } else { f64::INFINITY }).collect();
    t.sort_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal));
    let m = t[t.len() / 2];
    if m.is_infinite() {
        None
    } else {
        Some(m)
    }
}

pub fn ping_band(ms: Option<f64>) -> &'static str {
    match ms {
        None => "poor",
        Some(m) if m < PING_OK_MS => "good",
        Some(m) if m < PING_POOR_MS => "ok",
        Some(_) => "poor",
    }
}

/// One service's light from its probe.
pub fn probe_row(id: &'static str, p: &Probe) -> Row {
    if !p.answered() {
        return Row::new(id, "poor", "down", None, "not reachable");
    }
    let ms = r(p.ms.unwrap_or(0.0));
    if p.status.unwrap_or(0.0) >= 500.0 {
        return Row::new(id, "poor", "error", Some(ms), "having problems");
    }
    if ms >= SLOW_MS {
        return Row::new(id, "ok", "slow", Some(ms), format!("slow · {}", fmt_ms(ms)));
    }
    Row::new(id, "good", "ok", Some(ms), format!("reachable · {}", fmt_ms(ms)))
}

/// Chat = the pings to the homeserver and whether sync is flowing.
pub fn chat_row(pings: &[Probe], sync_ago_ms: Option<f64>) -> Row {
    let got: Vec<&Probe> = pings.iter().filter(|p| p.answered()).collect();
    let base = if got.is_empty() {
        Row::new("chat", "poor", "down", None, "not reachable")
    } else if got.iter().any(|p| p.status.unwrap_or(0.0) >= 500.0) {
        Row::new("chat", "poor", "error", ping_of(pings), "having problems")
    } else {
        let ms = ping_of(pings);
        if got.len() < pings.len() {
            match ms {
                None => Row::new("chat", "ok", "unsteady", None, "unsteady"),
                Some(m) => Row::new("chat", "ok", "unsteady", ms, format!("unsteady · {}", fmt_ms(m))),
            }
        } else {
            let m = ms.unwrap_or(0.0);
            if m >= SLOW_MS {
                Row::new("chat", "ok", "slow", ms, format!("slow · {}", fmt_ms(m)))
            } else {
                Row::new("chat", "good", "ok", ms, format!("reachable · {}", fmt_ms(m)))
            }
        }
    };
    let Some(ago) = sync_ago_ms else { return base };
    if base.band == "poor" {
        return base;
    }
    if ago >= SYNC_STUCK_MS {
        return Row::new("chat", "poor", "stuck", base.ms, "messages aren't arriving");
    }
    if ago >= SYNC_LATE_MS {
        return Row::new("chat", "ok", "late", base.ms, "messages are late");
    }
    base
}

/// Your ping row.
pub fn ping_row(pings: &[Probe]) -> Row {
    match ping_of(pings) {
        None => Row::new("ping", "poor", "down", None, "no answer"),
        Some(m) => {
            let band = ping_band(Some(m));
            Row::new("ping", band, if band == "good" { "ok" } else { "slow" }, Some(m), fmt_ms(m))
        }
    }
}

fn cap(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_ascii_uppercase().to_string() + c.as_str(),
        None => String::new(),
    }
}

/// "Calls", "Calls and media", "Chat, calls and media".
pub fn names_of(ids: &[&str]) -> String {
    let n: Vec<&str> = ids.iter().map(|i| noun(i)).collect();
    if n.len() <= 1 {
        return cap(n.first().copied().unwrap_or(""));
    }
    cap(&format!("{} and {}", n[..n.len() - 1].join(", "), n[n.len() - 1]))
}

fn plural(ids: &[&str]) -> bool {
    ids.len() > 1 || ids.first() == Some(&"calls") || ids.first() == Some(&"tinhat")
}

fn note_down(id: &str) -> &'static str {
    match id {
        "chat" => "Chat isn't reachable from here. If friends can chat, it's your connection.",
        "calls" => "Calls aren't reachable from here. If friends can call, it's your connection.",
        "media" => "Pictures and files aren't loading from here. If friends can see them, it's your connection.",
        "tinhat" => "The extras (sounds, GIFs, Memories) aren't reachable from here. If they work for friends, it's your connection.",
        _ => "",
    }
}
const NOTE_STUCK: &str = "New messages aren't arriving here. If friends can chat, it's your connection.";
const NOTE_ALL: &str = "The server isn't reachable from here. If other websites work, the server itself may be down.";
const NOTE_SLOW: &str = "Your connection to the server is slow right now, so calls may stutter.";

/// The check's input as the vectors hold it: `{pings: [Probe…], syncAgoMs, calls, media, tinhat}`.
pub struct CheckInput {
    pub pings: Vec<Probe>,
    pub sync_ago_ms: Option<f64>,
    pub calls: Probe,
    pub media: Probe,
    pub tinhat: Probe,
}

impl CheckInput {
    pub fn from_json(v: &Value) -> CheckInput {
        let probe = |k: &str| v.get(k).map(Probe::from_json).unwrap_or_default();
        CheckInput {
            pings: v.get("pings").and_then(Value::as_array).map(|a| a.iter().map(Probe::from_json).collect()).unwrap_or_default(),
            sync_ago_ms: finite(v.get("syncAgoMs")),
            calls: probe("calls"),
            media: probe("media"),
            tinhat: probe("tinhat"),
        }
    }
}

/// The whole check: the rows, your ping, the verdict and the note.
pub fn check_of(input: &CheckInput) -> Check {
    let rows = vec![
        chat_row(&input.pings, input.sync_ago_ms),
        probe_row("calls", &input.calls),
        probe_row("media", &input.media),
        probe_row("tinhat", &input.tinhat),
    ];
    let ping = ping_row(&input.pings);
    let red: Vec<&Row> = rows.iter().filter(|x| x.band == "poor").collect();
    let red_ids: Vec<&str> = red.iter().map(|x| x.id).collect();
    let (tone, verdict, note): (&'static str, String, String) = if red.len() == rows.len() {
        ("poor", "Can't reach the server".into(), NOTE_ALL.into())
    } else if !red.is_empty() {
        let v = format!("{} {} down", names_of(&red_ids), if plural(&red_ids) { "look" } else { "looks" });
        let n = if red.len() == 1 {
            let x = red[0];
            if x.cause == "error" {
                format!("{} {} having problems on the server's side. It's not just you.", names_of(&red_ids), if plural(&red_ids) { "are" } else { "is" })
            } else if x.cause == "stuck" {
                NOTE_STUCK.to_string()
            } else {
                note_down(x.id).to_string()
            }
        } else if red.iter().all(|x| x.cause == "error") {
            format!("{} are having problems on the server's side. It's not just you.", names_of(&red_ids))
        } else {
            format!("{} aren't reachable from here. If they work for friends, it's your connection.", names_of(&red_ids))
        };
        ("poor", v, n)
    } else if ping.ms.is_none() {
        ("ok", "Unsteady connection".into(), String::new())
    } else if ping.band == "poor" {
        ("ok", format!("Slow connection · {}", fmt_ms(ping.ms.unwrap_or(0.0))), NOTE_SLOW.into())
    } else if ping.band == "ok" || rows.iter().any(|x| x.band == "ok") {
        ("ok", format!("A bit slow · {}", fmt_ms(ping.ms.unwrap_or(0.0))), String::new())
    } else {
        ("good", format!("All good · {}", fmt_ms(ping.ms.unwrap_or(0.0))), String::new())
    };
    Check { rows, ping, tone, verdict, note }
}

// ---- the sparkline (Large) ----------------------------------------------------------------------------------------

/// A bar's height (per-mille): red → full; else by the round trip (under 50 / 150 / 400 / 1000 ms, then full).
pub fn bar_pm(band: &str, ms: Option<f64>) -> f64 {
    let Some(ms) = ms.filter(|m| m.is_finite()) else { return 1000.0 };
    if band == "poor" {
        return 1000.0;
    }
    let m = r(ms);
    if m < 50.0 {
        250.0
    } else if m < 150.0 {
        450.0
    } else if m < 400.0 {
        650.0
    } else if m < 1000.0 {
        850.0
    } else {
        1000.0
    }
}

/// The last 10 minutes as one-minute slots ending at `at` (the latest check): `[{band, h} | null; 10]` (JSON).
pub fn spark_slots(samples: &Value, at: Option<f64>) -> Value {
    let mut best: Vec<Option<(f64, &'static str, Option<f64>)>> = vec![None; SPARK_SLOTS];
    if let (Some(at), Some(list)) = (at, samples.as_array()) {
        for s in list {
            let Some(t) = finite(s.get("at")) else { continue };
            if t > at || t <= at - SPARK_SLOTS as f64 * SLOT_MS {
                continue;
            }
            let band: &'static str = match s.get("band").and_then(Value::as_str) {
                Some("good") => "good",
                Some("ok") => "ok",
                Some("poor") => "poor",
                _ => continue,
            };
            let k = SPARK_SLOTS as f64 - 1.0 - ((at - t) / SLOT_MS).floor();
            let k = k as usize;
            if best[k].map_or(true, |b| t >= b.0) {
                best[k] = Some((t, band, finite(s.get("ms"))));
            }
        }
    }
    Value::Array(
        best.into_iter()
            .map(|b| match b {
                Some((_, band, ms)) => json!({ "band": band, "h": crate::num(bar_pm(band, ms)) }),
                None => Value::Null,
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_task_example_and_the_friendly_sentence() {
        let p = |s: Option<f64>, m: Option<f64>| Probe { status: s, ms: m };
        let good = CheckInput {
            pings: vec![p(Some(200.0), Some(40.0)), p(Some(200.0), Some(42.0)), p(Some(200.0), Some(45.0))],
            sync_ago_ms: Some(5000.0),
            calls: p(Some(200.0), Some(84.0)),
            media: p(Some(200.0), Some(51.0)),
            tinhat: p(Some(401.0), Some(30.0)),
        };
        let c = check_of(&good);
        assert_eq!(c.verdict, "All good · 42 ms");
        assert_eq!(c.rows[1].reason, "reachable · 84 ms");
        let down = check_of(&CheckInput { calls: p(None, None), ..good });
        assert_eq!(down.verdict, "Calls look down");
        assert_eq!(down.note, "Calls aren't reachable from here. If friends can call, it's your connection.");
    }
}
