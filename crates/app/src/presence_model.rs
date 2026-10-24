//! Opt-in presence — the pure rules (docs/REACH.md §2, §3.4 and amendments A2.2 / A2.12 /
//! A2.13 / A2.15). No Qt, no network, no clock: `presence.rs` feeds these the facts (idle
//! sample, call, preference, server answers) and acts on what they decide. The web mirrors the
//! same rules in `features/presence/model.ts` with the same test names.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{json, Value};

/// Global account data: the consent. `{"share": bool, "v": 1}` — written ONLY by a person.
pub const PREF_TYPE: &str = "dev.tinhat.presence";
/// Global account data: the one-time question was answered. `{"asked": true}` (A2.2).
pub const ASKED_TYPE: &str = "dev.tinhat.presence_asked";
/// Global account data: the custom status (A4.1) —
/// `{"id", "text", "emoji", "color", "until": ms|null, "v": 1}`; `{}` or absent = none.
pub const STATUS_TYPE: &str = "dev.tinhat.presence_status";

/// §2.6 dots — STATE KEYS (themes, desktop C2; contract MERGE-NOTES decision 5): the view names the token of a presence
/// dot and QML paints the ACTIVE theme's colour for it (`Th.colourRef(dot)`), so a theme switch recolours every dot at
/// once and nothing here knows a colour. The built-ins paint the fixed §2.6 colours (#3ba55c online and in a call,
/// #faa81a away: `builtin_themes_keep_the_reach_dots`). A custom status colour (A4.3) is content and stays its hex.
pub const ONLINE_DOT: &str = "presence.online.dot";
pub const AWAY_DOT: &str = "presence.away.dot";
pub const INCALL_DOT: &str = "presence.incall.dot";

/// A4.3: the only colours a status can have (fixed hex, every theme), in the picker's order.
pub const PALETTE: [(&str, &str); 12] = [
    ("blurple", "#5865F2"),
    ("teal", "#1ABC9C"),
    ("purple", "#9B59B6"),
    ("orange", "#E67E22"),
    ("red", "#ED4245"),
    ("blue", "#3498DB"),
    ("grey", "#95A5A6"),
    ("pink", "#FF4FA3"),
    ("yellow", "#F1C40F"),
    ("lime", "#A3E635"),
    ("cyan", "#22D3EE"),
    ("white", "#F2F3F5"),
];

/// A4.2: the presets `(id, emoji, text, colour key)`, the same order everywhere.
pub const PRESETS: [(&str, &str, &str, &str); 8] = [
    ("lfg", "🎯", "Looking for group", "blurple"),
    ("down", "🎮", "Down to play", "teal"),
    ("queue", "⏳", "In queue", "purple"),
    ("grind", "⚔️", "Grinding", "orange"),
    ("ranked", "🔥", "Sweating ranked", "red"),
    ("chill", "🛋️", "Just chilling", "blue"),
    ("brb", "🍕", "BRB", "grey"),
    ("tilted", "😤", "Tilted", "pink"),
];

/// A4.1: a status text is 1–40 characters after trimming.
pub const STATUS_TEXT_MAX: usize = 40;
/// A4.6: picking a preset clears it after 4 hours unless told otherwise.
pub const PRESET_CLEAR_AFTER_MS: u64 = 4 * 60 * 60 * 1000;

/// §2.4: `away` after this much system idle.
pub const AWAY_AFTER_MS: u64 = 10 * 60 * 1000;
/// §3.4: the computer counts as in use while system idle is below this.
pub const ACTIVE_IDLE_MS: u64 = 3 * 60 * 1000;
/// §2.4 cadence.
pub const BEAT_EVERY: Duration = Duration::from_secs(30);
pub const BEAT_DEBOUNCE: Duration = Duration::from_secs(2);
pub const IDLE_PROBE_EVERY: Duration = Duration::from_secs(15);
/// §2.5 cadence: visible / hidden. The server allows one GET per 5 s per token (A2.4).
pub const READ_VISIBLE: Duration = Duration::from_secs(20);
pub const READ_HIDDEN: Duration = Duration::from_secs(60);
pub const READ_MIN_GAP: Duration = Duration::from_millis(5500);
/// §2.5: on errors the last map is kept this long after the last good answer, then cleared.
pub const MAP_KEEP: Duration = Duration::from_secs(60);
/// §3.4 / A2.12 cadence.
pub const ACTIVE_EVERY: Duration = Duration::from_secs(15);
pub const QUIET_WANTED_EVERY: Duration = Duration::from_secs(10 * 60);
/// A2.12: pings only while the last successful sync is younger than this.
pub const SYNC_HEALTHY: Duration = Duration::from_secs(60);
/// A2.2: the off switch's DELETE is retried with backoff until it answers 204.
pub const DELETE_BACKOFF_FIRST: Duration = Duration::from_secs(2);
pub const DELETE_BACKOFF_MAX: Duration = Duration::from_secs(60);

/// What the idle probe saw: system idle in ms and whether the screen is locked.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IdleSample {
    pub idle_ms: u64,
    pub locked: bool,
}

/// §2.4: the state a beat reports — `away` when idle ≥ 10 min or the screen is locked,
/// otherwise `online`. (The server turns any call into `call` itself.)
pub fn beat_state(s: IdleSample) -> &'static str {
    if s.locked || s.idle_ms >= AWAY_AFTER_MS {
        "away"
    } else {
        "online"
    }
}

/// The `PUT /api/presence` body (§2.3). `call` = the room of the Tin Hat call we are
/// connected to, else null. `refresh` (A4.4): the beat right after a change of share or status
/// asks the server to re-read them instead of trusting its 20 s cache.
pub fn beat_body(state: &str, call: Option<&str>, refresh: bool) -> Value {
    let mut b = json!({ "state": state, "call": call, "client": "desktop" });
    if refresh {
        b["refresh"] = Value::Bool(true);
    }
    b
}

/// What a beat's answer means for the sender (§2.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BeatOutcome {
    /// 2xx — keep the cadence.
    Ok,
    /// 409 `not_sharing` — stop beating until the account data says `share: true` again.
    NotSharing,
    /// Anything else (network, 401, 5xx, 429) — keep the normal cadence, no retry storm.
    Retry,
}

pub fn beat_outcome(status: u16, body: &Value) -> BeatOutcome {
    match status {
        200..=299 => BeatOutcome::Ok,
        409 if body.get("error").and_then(Value::as_str) == Some("not_sharing") => BeatOutcome::NotSharing,
        _ => BeatOutcome::Retry,
    }
}

/// A change of this device's view of the consent flag, and what it asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShareChange {
    /// Sharing starts (or restarts): beat at once.
    Start,
    /// Sharing stopped: stop at once and send `DELETE /api/presence` (until it answers 204).
    StopAndDelete,
    /// Nothing to do.
    Same,
}

/// `before` is None while the preference was not known yet (before the first sync). Learning
/// "not sharing" at start-up sends nothing at all: nothing about you was ever sent.
pub fn share_change(before: Option<bool>, now: bool) -> ShareChange {
    match (before, now) {
        (Some(true), false) => ShareChange::StopAndDelete,
        (Some(true), true) | (Some(false), false) | (None, false) => ShareChange::Same,
        (None, true) | (Some(false), true) => ShareChange::Start,
    }
}

/// `dev.tinhat.presence` content → sharing? Absent, malformed or `share !== true` = no.
pub fn parse_share(content: Option<&Value>) -> bool {
    content.and_then(|c| c.get("share")).and_then(Value::as_bool) == Some(true)
}

/// `dev.tinhat.presence_asked` content → answered? (A2.2)
pub fn parse_asked(content: Option<&Value>) -> bool {
    content.and_then(|c| c.get("asked")).and_then(Value::as_bool) == Some(true)
}

/// A custom status that can be shown now (A4.1 / A4.4 validation).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CustomStatus {
    pub id: String,
    pub text: String,
    pub emoji: String,
    /// palette key (an unknown key renders grey)
    pub color: String,
    /// epoch ms; None = don't clear (the server strips it from GET rows)
    pub until: Option<u64>,
}

/// A palette key → hex; an unknown key renders grey (A4.3).
pub fn status_color(key: &str) -> &'static str {
    PALETTE.iter().find(|(k, _)| *k == key).map(|(_, h)| *h).unwrap_or("#95A5A6")
}

/// The status if it can be shown at `now_ms` (text 1–40 after trim, not expired), else None.
pub fn live_status(v: Option<&Value>, now_ms: u64) -> Option<CustomStatus> {
    let o = v?.as_object()?;
    let text = o.get("text").and_then(Value::as_str).unwrap_or("").trim().to_string();
    let n = text.chars().count();
    if n == 0 || n > STATUS_TEXT_MAX {
        return None;
    }
    let until = o.get("until").and_then(Value::as_u64);
    if matches!(until, Some(u) if u <= now_ms) {
        return None;
    }
    Some(CustomStatus {
        id: o.get("id").and_then(Value::as_str).unwrap_or("").to_string(),
        text,
        emoji: o.get("emoji").and_then(Value::as_str).unwrap_or("").trim().to_string(),
        color: o.get("color").and_then(Value::as_str).unwrap_or("grey").to_string(),
        until,
    })
}

/// A4.6 expiry: a status whose `until` has passed must be cleared (`{}` written once).
pub fn status_expired(v: Option<&Value>, now_ms: u64) -> bool {
    let Some(o) = v.and_then(Value::as_object) else { return false };
    matches!(o.get("until").and_then(Value::as_u64), Some(u) if u <= now_ms)
}

/// The account data a pick writes (A4.1).
pub fn status_content(id: &str, text: &str, emoji: &str, color: &str, until: Option<u64>) -> Value {
    json!({ "id": id, "text": text.trim(), "emoji": emoji.trim(), "color": color, "until": until, "v": 1 })
}

pub fn state_word(state: &str) -> &'static str {
    match state {
        "away" => "Away",
        "call" => "In a call",
        _ => "Online",
    }
}

/// "<emoji> <text>", or "<text>" without an emoji.
pub fn status_text(s: &CustomStatus) -> String {
    if s.emoji.is_empty() {
        s.text.clone()
    } else {
        format!("{} {}", s.emoji, s.text)
    }
}

/// What a person's dot and label read (A4.5). `dot` = a presence token id (ONLINE_DOT / AWAY_DOT / INCALL_DOT, a state
/// key) or a status colour hex; `chip` = the status part (text, colour hex).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusView {
    pub dot: String,
    pub label: String,
    pub chip: Option<(String, String)>,
}

/// A4.5 — away wins the dot; a call keeps the status colour (and its headset glyph).
pub fn status_view(state: &str, status: Option<&Value>, now_ms: u64) -> StatusView {
    let Some(s) = live_status(status, now_ms) else {
        let dot = match state {
            "away" => AWAY_DOT,
            "call" => INCALL_DOT,
            _ => ONLINE_DOT,
        };
        return StatusView { dot: dot.into(), label: state_word(state).into(), chip: None };
    };
    let color = status_color(&s.color).to_string();
    let text = status_text(&s);
    let chip = Some((text.clone(), color.clone()));
    match state {
        "away" => StatusView { dot: AWAY_DOT.into(), label: format!("{text} · Away"), chip },
        "call" => StatusView { dot: color, label: format!("{text} · In a call"), chip },
        _ => StatusView { dot: color, label: text, chip },
    }
}

/// The whole consent object, as the switch / the card writes it (A2.2 — never from a cache).
pub fn pref_content(share: bool) -> Value {
    json!({ "share": share, "v": 1 })
}

/// The onboarding flag object (A2.2).
pub fn asked_content() -> Value {
    json!({ "asked": true })
}

/// One person in `GET /api/presence`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Seen {
    pub state: String,
    pub room: String,
    /// A4.4: `{"text", "emoji", "color"}` when they have a valid status (the server validates).
    pub status: Option<Value>,
}

/// §2.5: the GET body → mxid → state. Unknown states and malformed rows are dropped (no dot
/// beats a wrong dot); `room` is kept only for `call`.
pub fn parse_users(body: &Value) -> BTreeMap<String, Seen> {
    let mut out = BTreeMap::new();
    let Some(users) = body.get("users").and_then(Value::as_object) else { return out };
    for (mxid, v) in users {
        if !mxid.starts_with('@') || !mxid.contains(':') {
            continue;
        }
        let Some(state) = v.get("state").and_then(Value::as_str) else { continue };
        if !matches!(state, "online" | "away" | "call") {
            continue;
        }
        let room = if state == "call" {
            v.get("room").and_then(Value::as_str).filter(|r| r.starts_with('!')).unwrap_or("").to_string()
        } else {
            String::new()
        };
        // Re-validated here too: a row that fails `live_status` shows as no status at all.
        let status = v.get("status").filter(|s| live_status(Some(s), 0).is_some()).cloned();
        out.insert(mxid.clone(), Seen { state: state.to_string(), room, status });
    }
    out
}

/// The map as QML reads it, with the A4.5 view precomputed so QML never re-implements it:
/// `{"@a:hs": {"state": "call", "room": "!r:hs", "dot": "#5865F2", "label": "🎯 … · In a call",
///             "chip": "🎯 Looking for group", "chipColor": "#5865F2"},
///  "@b:hs": {"state": "away", "dot": "presence.away.dot", "label": "Away"}}` (`room`/`chip*` only when set; `dot` is
/// a status colour or a presence token id — QML paints `Th.colourRef(dot)`).
pub fn users_json(map: &BTreeMap<String, Seen>, now_ms: u64) -> String {
    let mut o = serde_json::Map::new();
    for (k, s) in map {
        let view = status_view(&s.state, s.status.as_ref(), now_ms);
        let mut e = serde_json::Map::new();
        e.insert("state".into(), Value::String(s.state.clone()));
        if !s.room.is_empty() {
            e.insert("room".into(), Value::String(s.room.clone()));
        }
        e.insert("dot".into(), Value::String(view.dot));
        e.insert("label".into(), Value::String(view.label));
        if let Some((text, color)) = view.chip {
            e.insert("chip".into(), Value::String(text));
            e.insert("chipColor".into(), Value::String(color));
        }
        o.insert(k.clone(), Value::Object(e));
    }
    Value::Object(o).to_string()
}

/// A2.15: "Others see you as …" comes from the server's view of you — your own entry.
pub fn my_view(map: &BTreeMap<String, Seen>, me: &str) -> (String, String) {
    map.get(me).map(|s| (s.state.clone(), s.room.clone())).unwrap_or_default()
}

/// §2.5: keep the last good map on errors, but not longer than a minute.
pub fn keep_map(since_last_good: Option<Duration>) -> bool {
    matches!(since_last_good, Some(d) if d <= MAP_KEEP)
}

/// §2.5: the reader's cadence.
pub fn read_interval(visible: bool) -> Duration {
    if visible {
        READ_VISIBLE
    } else {
        READ_HIDDEN
    }
}

/// §3.4 / A2.12: this computer is "in use" — you are in a call, or the screen is unlocked and
/// the system has seen input within 3 minutes.
pub fn computer_active(s: IdleSample, in_call: bool) -> bool {
    in_call || (!s.locked && s.idle_ms < ACTIVE_IDLE_MS)
}

/// A2.12: pings are allowed at all only while signed in, the sync is healthy (< 60 s since
/// the last good pass), desktop notifications are on, and the account asked for them (a push
/// registration with `quietWhileActive`).
pub fn may_ping(signed_in: bool, since_sync: Option<Duration>, notifications_on: bool, quiet_wanted: bool) -> bool {
    signed_in && notifications_on && quiet_wanted && matches!(since_sync, Some(d) if d < SYNC_HEALTHY)
}

/// One step of the §3.4 sender.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ping {
    /// `{"pushkey": null, "visible": true, "instance": …}`
    Active,
    /// `{"pushkey": null, "visible": false, "instance": …}` — once, on the way out.
    Inactive,
    None,
}

/// `active_sent` = the server was last told `visible: true`; `due` = at least 15 s since the
/// last `visible: true` ATTEMPT (answered or not — a failing server never gets a storm).
pub fn ping_step(allowed_and_active: bool, active_sent: bool, due: bool) -> Ping {
    match (allowed_and_active, active_sent) {
        (true, _) if due => Ping::Active,
        (false, true) => Ping::Inactive,
        _ => Ping::None,
    }
}

/// The `POST /api/push/active` body (§3.4, A2.12 identity).
pub fn ping_body(visible: bool, instance: &str) -> Value {
    json!({ "pushkey": Value::Null, "visible": visible, "instance": instance })
}

/// `GET /api/push/quiet-wanted` → wanted? Anything but an explicit `true` is no (consent first).
pub fn parse_quiet_wanted(body: &Value) -> bool {
    body.get("wanted").and_then(Value::as_bool) == Some(true)
}

/// A2.2: the off switch's DELETE backoff: 2 s, 4 s, 8 s … capped at a minute.
pub fn next_backoff(prev: Option<Duration>) -> Duration {
    match prev {
        None => DELETE_BACKOFF_FIRST,
        Some(d) => (d * 2).min(DELETE_BACKOFF_MAX),
    }
}

/// §2.7: the one-time question shows only when it was never answered, you are not sharing,
/// the preference is known (after the first sync) and you are not in a call.
pub fn show_question(known: bool, asked: bool, sharing: bool, in_call: bool) -> bool {
    known && !asked && !sharing && !in_call
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: u64 = 60 * 1000;

    #[test]
    fn away_after_ten_minutes_or_locked() {
        assert_eq!(beat_state(IdleSample { idle_ms: 0, locked: false }), "online");
        assert_eq!(beat_state(IdleSample { idle_ms: 10 * MIN - 1, locked: false }), "online");
        assert_eq!(beat_state(IdleSample { idle_ms: 10 * MIN, locked: false }), "away");
        assert_eq!(beat_state(IdleSample { idle_ms: 5_000, locked: true }), "away");
    }

    #[test]
    fn beat_body_shape() {
        assert_eq!(beat_body("online", None, false), json!({"state": "online", "call": null, "client": "desktop"}));
        assert_eq!(beat_body("away", Some("!r:example.com"), false)["call"], "!r:example.com");
        assert_eq!(beat_body("online", None, true)["refresh"], true, "A4.4: right after a change");
    }

    #[test]
    fn beat_answers() {
        assert_eq!(beat_outcome(200, &json!({"ok": true, "every": 30, "ttl": 90})), BeatOutcome::Ok);
        assert_eq!(beat_outcome(409, &json!({"error": "not_sharing"})), BeatOutcome::NotSharing);
        // any other failure keeps the cadence (no storms, no silent stop)
        for s in [0u16, 400, 401, 429, 500, 502, 503] {
            assert_eq!(beat_outcome(s, &Value::Null), BeatOutcome::Retry, "{s}");
        }
        assert_eq!(beat_outcome(409, &json!({"error": "other"})), BeatOutcome::Retry);
    }

    #[test]
    fn share_transitions() {
        assert_eq!(share_change(None, false), ShareChange::Same); // never shared: nothing is sent
        assert_eq!(share_change(None, true), ShareChange::Start);
        assert_eq!(share_change(Some(false), true), ShareChange::Start);
        assert_eq!(share_change(Some(true), false), ShareChange::StopAndDelete);
        assert_eq!(share_change(Some(true), true), ShareChange::Same);
        assert_eq!(share_change(Some(false), false), ShareChange::Same);
    }

    #[test]
    fn preference_parsing_is_strict() {
        assert!(!parse_share(None));
        assert!(!parse_share(Some(&json!({}))));
        assert!(!parse_share(Some(&json!({"share": "true"}))));
        assert!(!parse_share(Some(&json!({"share": 1}))));
        assert!(!parse_share(Some(&json!({"share": false, "v": 1}))));
        assert!(parse_share(Some(&json!({"share": true, "v": 1}))));
        assert!(!parse_asked(Some(&json!({"asked": false}))));
        assert!(parse_asked(Some(&json!({"asked": true}))));
        // what the switch and the card write (A2.2)
        assert_eq!(pref_content(true), json!({"share": true, "v": 1}));
        assert_eq!(pref_content(false), json!({"share": false, "v": 1}));
        assert_eq!(asked_content(), json!({"asked": true}));
    }

    #[test]
    fn reader_map() {
        let body = json!({"users": {
            "@a:example.com": {"state": "online"},
            "@b:example.com": {"state": "call", "room": "!r:example.com"},
            "@c:example.com": {"state": "away", "room": "!leak:example.com"},
            "@d:example.com": {"state": "busy"},
            "not-an-mxid": {"state": "online"},
            "@e:example.com": {"state": "call"}
        }, "every": 20});
        let m = parse_users(&body);
        assert_eq!(m.len(), 4);
        assert_eq!(m["@a:example.com"], Seen { state: "online".into(), room: String::new(), status: None });
        assert_eq!(m["@b:example.com"].room, "!r:example.com");
        assert_eq!(m["@c:example.com"].room, "", "a room is only ever shown for a call");
        assert_eq!(m["@e:example.com"], Seen { state: "call".into(), room: String::new(), status: None });
        assert!(!m.contains_key("@d:example.com"));
        assert_eq!(
            serde_json::from_str::<Value>(&users_json(&m, 0)).unwrap(),
            json!({"@a:example.com": {"state": "online", "dot": "presence.online.dot", "label": "Online"},
                   "@b:example.com": {"state": "call", "room": "!r:example.com", "dot": "presence.incall.dot", "label": "In a call"},
                   "@c:example.com": {"state": "away", "dot": "presence.away.dot", "label": "Away"},
                   "@e:example.com": {"state": "call", "dot": "presence.incall.dot", "label": "In a call"}})
        );
        assert!(parse_users(&json!({"error": "x"})).is_empty());
        assert_eq!(my_view(&m, "@b:example.com"), ("call".to_string(), "!r:example.com".to_string()));
        assert_eq!(my_view(&m, "@nobody:example.com"), (String::new(), String::new()));
    }

    #[test]
    fn map_survives_errors_for_a_minute() {
        assert!(!keep_map(None));
        assert!(keep_map(Some(Duration::from_secs(59))));
        assert!(keep_map(Some(Duration::from_secs(60))));
        assert!(!keep_map(Some(Duration::from_secs(61))));
        assert_eq!(read_interval(true), Duration::from_secs(20));
        assert_eq!(read_interval(false), Duration::from_secs(60));
        assert!(READ_MIN_GAP > Duration::from_secs(5), "the server allows one GET per 5 s");
    }

    #[test]
    fn computer_in_use() {
        assert!(computer_active(IdleSample { idle_ms: 0, locked: false }, false));
        assert!(computer_active(IdleSample { idle_ms: 3 * MIN - 1, locked: false }, false));
        assert!(!computer_active(IdleSample { idle_ms: 3 * MIN, locked: false }, false));
        assert!(!computer_active(IdleSample { idle_ms: 0, locked: true }, false));
        // a call counts as being at the computer, idle or locked
        assert!(computer_active(IdleSample { idle_ms: 20 * MIN, locked: true }, true));
    }

    #[test]
    fn ping_consent_and_health() {
        let ok = Some(Duration::from_secs(5));
        assert!(may_ping(true, ok, true, true));
        assert!(!may_ping(false, ok, true, true), "signed out");
        assert!(!may_ping(true, None, true, true), "no sync yet");
        assert!(!may_ping(true, Some(Duration::from_secs(60)), true, true), "sync unhealthy");
        assert!(!may_ping(true, ok, false, true), "desktop notifications off");
        assert!(!may_ping(true, ok, true, false), "no quietWhileActive registration = no consent");
        assert!(parse_quiet_wanted(&json!({"wanted": true})));
        assert!(!parse_quiet_wanted(&json!({"wanted": "yes"})));
        assert!(!parse_quiet_wanted(&Value::Null));
    }

    #[test]
    fn ping_steps() {
        assert_eq!(ping_step(true, false, true), Ping::Active, "becoming active pings at once");
        assert_eq!(ping_step(true, false, false), Ping::None, "a failed attempt is not retried inside 15 s");
        assert_eq!(ping_step(true, true, false), Ping::None);
        assert_eq!(ping_step(true, true, true), Ping::Active, "every 15 s");
        assert_eq!(ping_step(false, true, false), Ping::Inactive, "one visible:false on the way out");
        assert_eq!(ping_step(false, true, true), Ping::Inactive);
        assert_eq!(ping_step(false, false, true), Ping::None, "and only once");
        assert_eq!(ping_body(true, "i1"), json!({"pushkey": null, "visible": true, "instance": "i1"}));
        assert_eq!(ping_body(false, "i1")["visible"], false);
    }

    #[test]
    fn delete_backoff() {
        let a = next_backoff(None);
        let b = next_backoff(Some(a));
        assert_eq!((a, b), (Duration::from_secs(2), Duration::from_secs(4)));
        assert_eq!(next_backoff(Some(Duration::from_secs(40))), Duration::from_secs(60));
        assert_eq!(next_backoff(Some(Duration::from_secs(60))), Duration::from_secs(60));
    }

    #[test]
    fn one_time_question() {
        assert!(show_question(true, false, false, false));
        assert!(!show_question(false, false, false, false), "not before the account data is known");
        assert!(!show_question(true, true, false, false), "answered");
        assert!(!show_question(true, false, true, false), "already sharing");
        assert!(!show_question(true, false, false, true), "never while in a call");
    }

    // ---- A4.5 statusView — the web mirrors these 7 names in status.test.ts (same fixtures) ----

    const NOW: u64 = 1_790_000_000_000;
    fn lfg() -> Value {
        let (id, emoji, text, color) = PRESETS[0];
        status_content(id, text, emoji, color, Some(NOW + 3_600_000))
    }

    #[test]
    fn sv_online_preset() {
        let v = status_view("online", Some(&lfg()), NOW);
        assert_eq!((v.dot.as_str(), v.label.as_str()), ("#5865F2", "🎯 Looking for group"));
        assert_eq!(v.chip, Some(("🎯 Looking for group".to_string(), "#5865F2".to_string())));
    }

    #[test]
    fn sv_call_preset() {
        let v = status_view("call", Some(&lfg()), NOW);
        assert_eq!((v.dot.as_str(), v.label.as_str()), ("#5865F2", "🎯 Looking for group · In a call"));
    }

    #[test]
    fn sv_away_preset() {
        let v = status_view("away", Some(&lfg()), NOW);
        assert_eq!((v.dot.as_str(), v.label.as_str()), (AWAY_DOT, "🎯 Looking for group · Away"));
    }

    #[test]
    fn sv_none_online() {
        let v = status_view("online", None, NOW);
        assert_eq!((v.dot.as_str(), v.label.as_str(), v.chip), (ONLINE_DOT, "Online", None));
    }

    /// REACH §2.6: the shipped themes keep the fixed dot colours (the dots are the theme's presence tokens — the view
    /// hands QML their ids, so these ids must exist in every theme).
    #[test]
    fn builtin_themes_keep_the_reach_dots() {
        for id in ["oled", "goose"] {
            let a = crate::theme_rt::try_resolve_in(id, std::path::Path::new("/nonexistent")).unwrap();
            let hex = |t: &str| crate::theme_rt::qt_string(&a.colour(t).unwrap());
            assert_eq!(hex(ONLINE_DOT), "#3ba55c", "{id}");
            assert_eq!(hex(INCALL_DOT), "#3ba55c", "{id}");
            assert_eq!(hex(AWAY_DOT), "#faa81a", "{id}");
        }
    }

    #[test]
    fn sv_expired() {
        let mut s = lfg();
        s["until"] = json!(NOW - 1);
        let v = status_view("online", Some(&s), NOW);
        assert_eq!((v.dot.as_str(), v.label.as_str()), (ONLINE_DOT, "Online"));
        assert!(status_expired(Some(&s), NOW), "the client clears it once");
        assert!(!status_expired(Some(&lfg()), NOW));
        assert!(!status_expired(Some(&json!({"text": "x", "until": null})), NOW), "don't clear = never expires");
    }

    #[test]
    fn sv_bad_color() {
        let s = json!({"text": "Raid night", "emoji": "🗡️", "color": "chartreuse", "until": null});
        let v = status_view("online", Some(&s), NOW);
        assert_eq!((v.dot.as_str(), v.label.as_str()), ("#95A5A6", "🗡️ Raid night"));
    }

    #[test]
    fn sv_no_emoji() {
        let s = json!({"text": "Back at 9", "emoji": "", "color": "teal", "until": null});
        let v = status_view("online", Some(&s), NOW);
        assert_eq!((v.dot.as_str(), v.label.as_str()), ("#1ABC9C", "Back at 9"));
    }

    #[test]
    fn status_validation_and_rows() {
        // words without a status; 1–40 characters after trimming; the palette and presets are A4's
        assert_eq!(status_view("call", Some(&json!({})), NOW).label, "In a call");
        assert_eq!(status_view("away", None, NOW).label, "Away");
        assert!(live_status(Some(&json!({"text": "   "})), NOW).is_none());
        assert!(live_status(Some(&json!({"text": "x".repeat(41)})), NOW).is_none());
        assert!(live_status(Some(&json!({"text": "é".repeat(40)})), NOW).is_some(), "characters, not bytes");
        assert_eq!(PALETTE.len(), 12);
        assert_eq!(PRESETS.map(|p| p.0), ["lfg", "down", "queue", "grind", "ranked", "chill", "brb", "tilted"]);
        assert!(PRESETS.iter().all(|p| PALETTE.iter().any(|(k, _)| *k == p.3)));
        // a GET row with a status → the precomputed view; an invalid status is dropped
        let body = json!({"users": {
            "@a:example.com": {"state": "call", "room": "!r:example.com", "status": {"text": "Down to play", "emoji": "🎮", "color": "teal"}},
            "@b:example.com": {"state": "online", "status": {"text": "", "emoji": "🎮", "color": "teal"}}
        }});
        let m = parse_users(&body);
        assert!(m["@b:example.com"].status.is_none());
        let v: Value = serde_json::from_str(&users_json(&m, NOW)).unwrap();
        assert_eq!(v["@a:example.com"]["dot"], "#1ABC9C");
        assert_eq!(v["@a:example.com"]["label"], "🎮 Down to play · In a call");
        assert_eq!(v["@a:example.com"]["chip"], "🎮 Down to play");
        assert_eq!(v["@a:example.com"]["chipColor"], "#1ABC9C");
        assert_eq!(v["@b:example.com"]["label"], "Online");
        assert_eq!(status_content("custom", " GG ", "", "lime", None), json!({"id": "custom", "text": "GG", "emoji": "", "color": "lime", "until": null, "v": 1}));
    }
}
