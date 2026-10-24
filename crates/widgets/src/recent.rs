//! recent — the rules the Pinned and Mentions widgets share (contract WIDGETS.md §11–§12, vectors
//! testvectors/widgets/recent/*): which pins show and in which order, the one-line snippet of a message, the Small
//! summaries, and which form a row's time takes. A line-for-line port of the web's `src/widgets/recent.ts` (widgets W1,
//! 2026-09-28): strings are cut in code points, whitespace is ASCII only (U+0009–U+000D, U+0020), numbers are doubles
//! floored like `Math.floor`. Pure: the apps render a "clock" form with their own clock-mode formatter.

use std::collections::HashSet;

use serde_json::{json, Map, Value};

/// At most this many pin ids are read (a room with more shows the newest).
pub const PIN_MAX: usize = 100;
/// A snippet is cut to this many code points ("…" appended when cut).
pub const SNIPPET_MAX: usize = 140;

const DAY_MS: f64 = 86_400_000.0;
const MINUTE_MS: f64 = 60_000.0;

fn is_ws(c: char) -> bool {
    c == ' ' || ('\u{9}'..='\u{d}').contains(&c)
}

fn str_of(v: Option<&Value>) -> &str {
    v.and_then(Value::as_str).unwrap_or("")
}

/// `m.room.pinned_events` content → the pinned event ids, the most recently pinned first. The list's END is the
/// newest pin, so it is read backwards; an id that appears twice keeps its later (newer) place; only strings starting
/// with "$" count; at most [`PIN_MAX`] ids.
pub fn pin_order(content: &Value) -> Vec<String> {
    let list = content.as_object().and_then(|o| o.get("pinned")).and_then(Value::as_array);
    let mut out = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    if let Some(list) = list {
        for id in list.iter().rev() {
            if out.len() >= PIN_MAX {
                break;
            }
            let Some(id) = id.as_str() else { continue };
            if !id.starts_with('$') || seen.contains(id) {
                continue;
            }
            seen.insert(id);
            out.push(id.to_string());
        }
    }
    out
}

/// A row's snippet: `kind` (text | image | video | audio | voice | file | sticker | location | poll | encrypted |
/// deleted | other) and one line of `text`.
#[derive(Clone, Debug, PartialEq)]
pub struct Snippet {
    pub kind: &'static str,
    pub text: String,
}

impl Snippet {
    fn new(kind: &'static str, text: impl Into<String>) -> Self {
        Snippet { kind, text: text.into() }
    }
    pub fn to_json(&self) -> Value {
        json!({ "kind": self.kind, "text": self.text })
    }
}

/// The reply fallback a reply's `body` starts with is dropped: the leading lines that start with "> " (or are
/// exactly ">") go, and — when at least one went — the next line too when it holds only ASCII whitespace. Lines are
/// split on "\n" only.
pub fn strip_reply_fallback(s: &str) -> String {
    let lines: Vec<&str> = s.split('\n').collect();
    let mut i = 0;
    while i < lines.len() && (lines[i].starts_with("> ") || lines[i] == ">") {
        i += 1;
    }
    if i > 0 && i < lines.len() && lines[i].chars().all(is_ws) {
        i += 1;
    }
    lines[i..].join("\n")
}

/// ASCII whitespace runs → one space, trimmed, cut to `max` code points (a space left at the cut goes; + "…" when
/// cut).
pub fn one_line(s: &str, max: usize) -> String {
    let mut out = String::new();
    let mut pending_space = false;
    for ch in s.chars() {
        if is_ws(ch) {
            pending_space = !out.is_empty();
            continue;
        }
        if pending_space {
            out.push(' ');
        }
        pending_space = false;
        out.push(ch);
    }
    let n = out.chars().count();
    if n <= max {
        return out;
    }
    let mut cut: String = out.chars().take(max).collect();
    if cut.ends_with(' ') {
        cut.pop();
    }
    cut.push('…');
    cut
}

/// A message as the timeline holds it after decryption (`{type?, content?, redacted?}`) → its row snippet (the web's
/// `snippetOf`, WIDGETS.md §11.2).
pub fn snippet_of(ev: &Value) -> Snippet {
    let ty = str_of(ev.get("type"));
    let empty = Map::new();
    let c = ev.get("content").and_then(Value::as_object).unwrap_or(&empty);
    if ev.get("redacted") == Some(&Value::Bool(true)) {
        return Snippet::new("deleted", "Message deleted");
    }
    if ty == "m.room.encrypted" {
        return Snippet::new("encrypted", "Encrypted message");
    }
    let body = one_line(str_of(c.get("body")), SNIPPET_MAX);
    if ty == "m.sticker" {
        return Snippet::new("sticker", if body.is_empty() { "Sticker".to_string() } else { body });
    }
    if ty == "m.poll.start" || ty == "org.matrix.msc3381.poll.start" {
        return Snippet::new("poll", "Poll");
    }
    if ty != "m.room.message" {
        return if body.is_empty() { Snippet::new("other", "Message") } else { Snippet::new("other", body) };
    }
    let fname = one_line(str_of(c.get("filename")), SNIPPET_MAX);
    let file = if fname.is_empty() { body } else { fname };
    let or = |s: String, d: &str| if s.is_empty() { d.to_string() } else { s };
    match str_of(c.get("msgtype")) {
        "m.image" => Snippet::new("image", or(file, "Image")),
        "m.video" => Snippet::new("video", or(file, "Video")),
        "m.file" => Snippet::new("file", or(file, "File")),
        "m.audio" => {
            if c.contains_key("org.matrix.msc3245.voice") {
                Snippet::new("voice", "Voice message")
            } else {
                Snippet::new("audio", or(file, "Audio"))
            }
        }
        "m.location" => Snippet::new("location", "Location"),
        _ => {
            let t = one_line(&strip_reply_fallback(str_of(c.get("body"))), SNIPPET_MAX);
            if t.is_empty() {
                Snippet::new("other", "Message")
            } else {
                Snippet::new("text", t)
            }
        }
    }
}

/// Pinned, Small / collapsed: "No pins" · "1 pin · <latest>" · "3 pins · latest: <latest>" ("" latest → the count).
pub fn pin_summary(count: f64, latest: &str) -> String {
    if count <= 0.0 {
        return "No pins".to_string();
    }
    let n = if count == 1.0 { "1 pin".to_string() } else { format!("{} pins", crate::js_number(count)) };
    if latest.is_empty() {
        return n;
    }
    if count == 1.0 {
        format!("{n} · {latest}")
    } else {
        format!("{n} · latest: {latest}")
    }
}

/// The form a past row time takes (the web's `whenOf`).
#[derive(Clone, Debug, PartialEq)]
pub struct When {
    /// clock | yesterday | weekday | date | dateYear
    pub form: &'static str,
    /// the local calendar fields of the time (weekday 0 = Sunday, month 0 = January)
    pub weekday: i64,
    pub day: i64,
    pub month: i64,
    pub year: i64,
}

struct Civil {
    days: i64,
    weekday: i64,
    day: i64,
    month: i64,
    year: i64,
}

/// Local calendar fields of `ms` in a zone `offset_min` minutes ahead of UTC (proleptic Gregorian; H. Hinnant's
/// civil_from_days with floored division, as the reference's Math.floor).
fn civil(ms: f64, offset_min: f64) -> Civil {
    let days = ((ms + offset_min * MINUTE_MS) / DAY_MS).floor() as i64;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 2 } else { mp - 10 };
    let year = yoe + era * 400 + if month <= 1 { 1 } else { 0 };
    let weekday = (days + 4).rem_euclid(7);
    Civil { days, weekday, day, month, year }
}

/// Which form a past row time takes in the viewer's zone: same local day → clock; the day before → yesterday; 2–6
/// days before → weekday; earlier this year → date; another year → dateYear. A time after `now` counts as today.
pub fn when_of(ts: f64, now: f64, offset_min: f64) -> When {
    let a = civil(ts, offset_min);
    let b = civil(now, offset_min);
    let ago = b.days - a.days;
    let form = if ago <= 0 {
        "clock"
    } else if ago == 1 {
        "yesterday"
    } else if ago < 7 {
        "weekday"
    } else if a.year == b.year {
        "date"
    } else {
        "dateYear"
    };
    When { form, weekday: a.weekday, day: a.day, month: a.month, year: a.year }
}

pub const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
pub const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// The label of a [`When`]; `clock` = the app's clock-mode text of the time (only used by the clock form).
pub fn when_label(w: &When, clock: &str) -> String {
    match w.form {
        "clock" => clock.to_string(),
        "yesterday" => "Yesterday".to_string(),
        "weekday" => WEEKDAYS[w.weekday as usize].to_string(),
        "date" => format!("{} {}", MONTHS[w.month as usize], w.day),
        _ => format!("{} {}, {}", MONTHS[w.month as usize], w.day, w.year),
    }
}

impl When {
    pub fn to_json(&self) -> Value {
        json!({ "form": self.form, "weekday": self.weekday, "day": self.day, "month": self.month, "year": self.year })
    }
}
