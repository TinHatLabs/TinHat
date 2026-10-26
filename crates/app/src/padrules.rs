//! padrules.rs — Pad (your phone as a remote for your call): the PURE rules, the same rules as Tin Hat Web's
//! `src/features/calls/pad/rules.ts` (one product: the same verdict, the same codes and the same texts for the same
//! input). No Qt, no network, no clock (every time is passed in): discovery ([`pad_targets`]), the trust + schema +
//! session checks every Pad message passes ([`check_pad_message`] on the receiver, [`accept_state`] on the
//! controller), the receiver's session table ([`PadSessions`]), the sound rule ([`sound_allowed`]), the state snapshot
//! and its caps ([`build_state`]), the rate bucket ([`RateBucket`]), the stale maths ([`stale_check`] /
//! [`open_stale`]) and the words ([`why_text`]). Both apps replay the shared vectors `testvectors/pad.json`
//! (byte-identical to the web's `src/features/calls/pad/vectors.json`); the normative contract is docs/PAD.md (the
//! same text in both repos).
//!
//! Wire (v1, Olm-encrypted to-device between my own cross-signing-verified devices, never a room event):
//!   dev.tinhat.pad.cmd   controller → receiver  {v:1, sid, seq, ts, room, gen (not on open), op, …op fields}
//!   dev.tinhat.pad.state receiver → controller  {v:1, sid, rseq, rt, re, ok, why, ops, call}
//! Every op is a SET (never a toggle), so a duplicate can't flip anything twice. The trust facts (decrypted? Olm
//! sender, bound device, verified both ways) come from the glue; everything decided from them lives here.
//!
//! JSON numbers: an "integer" field is a JSON number with an integral value of at most 2^53 − 1 in size (so `1.0`
//! is 1, as in JavaScript); lengths are counted in Unicode code points (`chars()`, the web's `for…of`).
#![allow(dead_code)] // phase 1 (rules + vectors): the transport, executor and UI that call these come next

use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};

pub const PAD_V: i64 = 1;
pub const LEASE_MS: i64 = 45_000;
pub const MAX_SESSIONS: usize = 4;
pub const STALE_MS: i64 = 8_000;
pub const OPEN_SKEW_MS: i64 = 300_000;
pub const RATE_PER_SEC: f64 = 8.0;
pub const RATE_BURST: f64 = 16.0;
pub const BUSY_EVERY_MS: i64 = 1_000;
pub const PUSH_EVERY_MS: i64 = 250;
pub const TALK_PUSH_EVERY_MS: i64 = 500;
pub const MAX_PEOPLE: usize = 64;
pub const MAX_RECENT: usize = 8;
pub const MAX_STATE_BYTES: usize = 16_384;
pub const MAX_URL: usize = 2_048;
pub const MAX_NAME: usize = 64;
pub const MAX_EMOJI: usize = 16;
pub const MAX_HINT: usize = 200;
pub const MAX_BUCKETS: usize = 16;
/// A bucket idle this long is full again (16 tokens at 8/s, busy allowed): forgetting it changes nothing.
pub const BUCKET_IDLE_MS: i64 = 2_000;
/// A sid that left the table is remembered this long (2 × OPEN_SKEW_MS: no replay of its open can be fresh enough).
pub const SPENT_MS: i64 = 600_000;
/// Spent sids + sessions stay at most this many: when full, new opens wait (`busy`) — nothing unexpired is forgotten.
pub const MAX_SPENT: usize = 64;
/// Nesting depth of a wire content (the content itself = 1).
pub const MAX_DEPTH: usize = 32;
/// Default membership lifetime when `expires` is absent (as [`crate::keyrules::MEMBERSHIP_EXPIRES_MS`]).
pub const MEMBERSHIP_EXPIRES_MS: i64 = 14_400_000;

pub const PAD_CAP: &str = "pad1";
pub const PAD_CMD: &str = "dev.tinhat.pad.cmd";
pub const PAD_STATE: &str = "dev.tinhat.pad.state";
/// The ops a v1 receiver runs (the state's `ops`; the controller shows only listed controls).
pub const PAD_OPS: [&str; 8] = ["sound.play", "sound.stop", "mic", "deafen", "brb", "vol", "vol.reset", "leave"];
pub const PAD_CATS: [&str; 3] = ["mic", "screenshare", "music"];
pub const PAD_MIC: [&str; 4] = ["on", "muted", "server", "ptt"];

const PROTO_KEYS: [&str; 3] = ["__proto__", "constructor", "prototype"];
const MAX_SAFE: f64 = 9_007_199_254_740_991.0;

// ---- shapes ------------------------------------------------------------------------------------------------

/// A JSON number that is an integer of at most 2^53 − 1 in size (`1.0` counts; `"1"`, `1.5` don't).
fn int(v: Option<&Value>) -> Option<i64> {
    let f = v?.as_f64()?;
    (f.fract() == 0.0 && f.abs() <= MAX_SAFE).then_some(f as i64)
}
fn uint(v: Option<&Value>) -> Option<i64> {
    int(v).filter(|n| *n >= 0)
}
fn level(v: Option<&Value>) -> Option<f64> {
    v?.as_f64().filter(|x| x.is_finite() && *x >= 0.0 && *x <= 2.0)
}
fn cp_len(s: &str) -> usize {
    s.chars().count()
}
fn cp_trunc(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}
fn str_of(v: Option<&Value>) -> &str {
    v.and_then(|x| x.as_str()).unwrap_or("")
}
fn is_true(v: Option<&Value>) -> bool {
    v.and_then(|x| x.as_bool()) == Some(true)
}
fn obj(v: Option<&Value>) -> Option<&Map<String, Value>> {
    v.and_then(|x| x.as_object())
}

/// The wire hygiene both apps share: objects / arrays nested at most 32 deep (the content itself = 1). (The web also
/// refuses lone UTF-16 surrogates; a Rust string can't hold one — serde_json already refused such a message.)
pub fn wire_ok(v: &Value) -> bool {
    let mut stack: Vec<(&Value, usize)> = vec![(v, 1)];
    while let Some((x, d)) = stack.pop() {
        match x {
            Value::Array(a) => {
                if d > MAX_DEPTH {
                    return false;
                }
                stack.extend(a.iter().map(|e| (e, d + 1)));
            }
            Value::Object(m) => {
                if d > MAX_DEPTH {
                    return false;
                }
                stack.extend(m.values().map(|e| (e, d + 1)));
            }
            _ => {}
        }
    }
    true
}

/// A short wire id (`gen`): 1–64 of [A-Za-z0-9_-], never `__proto__` & co (as the web's `validCornerId`).
pub fn valid_id(s: &str) -> bool {
    (1..=64).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') && !PROTO_KEYS.contains(&s)
}
/// A session id: exactly 16 base64url characters.
pub fn valid_sid(s: &str) -> bool {
    s.len() == 16 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
/// A room id: "!" + 1–254 printable ASCII characters.
pub fn valid_room(s: &str) -> bool {
    let b = s.as_bytes();
    (2..=255).contains(&b.len()) && b[0] == b'!' && b[1..].iter().all(|c| (0x21..=0x7e).contains(c))
}
/// A refusal code: 0–32 of [a-z-].
pub fn valid_why(s: &str) -> bool {
    s.len() <= 32 && s.bytes().all(|b| b.is_ascii_lowercase() || b == b'-')
}
/// A user id: "@local:server", printable ASCII, no ":" in the localpart, at most 255 characters.
pub fn valid_user(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() > 255 || b.first() != Some(&b'@') || !b.iter().all(|c| (0x21..=0x7e).contains(c)) {
        return false;
    }
    match s.find(':') {
        Some(i) => i > 1 && i + 1 < b.len(),
        None => false,
    }
}

// ---- trust (security rules 1–4) ----------------------------------------------------------------------------

/// What the glue knows about one received Pad message (looked up per message, cached ≤ 60 s, dropped on any
/// device-list / trust change).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PadFacts {
    /// rule 1: arrived as an Olm-decrypted to-device event (not plaintext, not UTD, not a room event)
    pub decrypted: bool,
    /// the user the Olm session authenticates
    pub olm_sender: String,
    /// the to-device event's `sender`
    pub event_sender: String,
    /// the device the Olm sender key belongs to ("" = not bound to a known device)
    pub sender_device: String,
    /// rule 3: the Olm Curve25519 key equals the device record of `sender_device`
    pub curve_matches: bool,
    pub my_user_id: String,
    pub my_device_id: String,
    /// rule 4a: THIS device is verified (all three cross-signing private keys cached)
    pub self_verified: bool,
    /// rule 4b: `sender_device` is cross-signing trusted (`Device::is_cross_signing_trusted`)
    pub peer_trusted: bool,
}

/// Rules 1–4, in this order. Every failure is a SILENT drop (no reply: no oracle, no state to an untrusted device).
pub fn check_trust(f: &PadFacts) -> Result<(), &'static str> {
    if !f.decrypted {
        return Err("not-olm");
    }
    if f.olm_sender.is_empty() || f.olm_sender != f.event_sender {
        return Err("sender-mismatch");
    }
    if f.my_user_id.is_empty() || f.olm_sender != f.my_user_id {
        return Err("other-user");
    }
    if f.sender_device.is_empty() || !f.curve_matches {
        return Err("unbound");
    }
    if f.sender_device == f.my_device_id {
        return Err("own-device");
    }
    if !f.self_verified {
        return Err("self-unverified");
    }
    if !f.peer_trusted {
        return Err("peer-untrusted");
    }
    Ok(())
}

// ---- rate bucket + stale maths -----------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Ok,
    Busy,
    Drop,
}
impl Hit {
    pub fn as_str(self) -> &'static str {
        match self {
            Hit::Ok => "ok",
            Hit::Busy => "busy",
            Hit::Drop => "drop",
        }
    }
}

/// Per controller device: 8 commands/s, burst 16; over budget → at most one `busy` reply per second, the rest are
/// dropped (no amplification). Pure on the timestamps it is handed; a clock that goes back refills nothing.
#[derive(Clone, Debug)]
pub struct RateBucket {
    pub tokens: f64,
    pub last: Option<i64>,
    pub last_busy: Option<i64>,
}
impl Default for RateBucket {
    fn default() -> Self {
        RateBucket { tokens: RATE_BURST, last: None, last_busy: None }
    }
}
impl RateBucket {
    pub fn hit(&mut self, now: i64) -> Hit {
        match self.last {
            None => self.last = Some(now),
            Some(last) if now > last => {
                self.tokens = RATE_BURST.min(self.tokens + ((now - last) as f64 * RATE_PER_SEC) / 1000.0);
                self.last = Some(now);
            }
            _ => {}
        }
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            return Hit::Ok;
        }
        if self.last_busy.map(|b| now - b >= BUSY_EVERY_MS).unwrap_or(true) {
            self.last_busy = Some(now);
            return Hit::Busy;
        }
        Hit::Drop
    }
}

/// An `open` whose `ts` is more than 5 min off the receiver's clock is refused (`stale`).
pub fn open_stale(ts: i64, now: i64) -> bool {
    (now - ts).abs() > OPEN_SKEW_MS
}
/// A later command is stale when it arrived more than 8 s after it was sent, measured with the clock offset
/// recorded at `open` (offset = receiver now − controller ts): 8.000 s is fine, 8.001 s is stale.
pub fn stale_check(offset: i64, ts: i64, now: i64) -> bool {
    now - (ts + offset) > STALE_MS
}

// ---- the receiver's session table --------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct PadSession {
    pub sid: String,
    /// the controller device that opened it — only that device may use the sid
    pub device: String,
    pub last_seq: i64,
    /// receiver now − controller ts, at open
    pub offset: i64,
    /// the last accepted command (the lease runs 45 s from here)
    pub last_at: i64,
    pub opened_at: i64,
    /// false after `ping {vis:false}`: no unsolicited state pushes
    pub vis: bool,
    /// the controller asked for talking rings (`open {want:["talk"]}`)
    pub talk: bool,
    /// the last state sent to it (reply or push), None = none yet
    pub pushed_at: Option<i64>,
}

#[derive(Debug, Default)]
pub struct PadSessions {
    /// in open order (oldest first)
    pub sessions: Vec<PadSession>,
    /// devices whose Pad was ended with × on the receiver: refused (`closed`) until the call changes
    pub refused: HashSet<String>,
    /// per controller device, in first-seen order (at most [`MAX_BUCKETS`]; only idle ones are forgotten)
    buckets: Vec<(String, RateBucket)>,
    /// sids that left the table (closed, replaced, lapsed, refused, cleared) → when, oldest first: never opened again
    /// for [`SPENT_MS`]
    spent_at: Vec<(String, i64)>,
}

impl PadSessions {
    fn purge_spent(&mut self, now: i64) {
        self.spent_at.retain(|(_, at)| now - at <= SPENT_MS);
    }
    fn spend(&mut self, sids: &[String], now: i64) {
        self.purge_spent(now);
        for sid in sids {
            self.spent_at.retain(|(s, _)| s != sid);
            self.spent_at.push((sid.clone(), now));
        }
    }
    /// Room for one more session: spent sids (unexpired) + sessions < 64. A spent sid is never forgotten early (that
    /// would make its replay possible again), so when full a new open waits (`busy`) until the oldest expire.
    pub fn room_to_open(&mut self, now: i64) -> bool {
        self.purge_spent(now);
        self.spent_at.len() + self.sessions.len() < MAX_SPENT
    }
    /// A sid that left the table within the last [`SPENT_MS`] (a lapsed one counts at once).
    pub fn is_spent(&self, sid: &str, now: i64) -> bool {
        if self.sessions.iter().any(|s| s.sid == sid && !Self::alive(s, now)) {
            return true;
        }
        self.spent_at.iter().any(|(s, at)| s == sid && now - at <= SPENT_MS)
    }
    pub fn spent_count(&self) -> usize {
        self.spent_at.len()
    }
    fn alive(s: &PadSession, now: i64) -> bool {
        now - s.last_at <= LEASE_MS
    }
    /// The live session with this sid (a lapsed one is gone even before `sweep`).
    pub fn get(&self, sid: &str, now: i64) -> Option<&PadSession> {
        self.sessions.iter().find(|s| s.sid == sid && Self::alive(s, now))
    }
    fn get_mut(&mut self, sid: &str, now: i64) -> Option<&mut PadSession> {
        self.sessions.iter_mut().find(|s| s.sid == sid && Self::alive(s, now))
    }
    pub fn live(&self, now: i64) -> Vec<&PadSession> {
        self.sessions.iter().filter(|s| Self::alive(s, now)).collect()
    }
    /// Forget lapsed sessions; returns their sids (their controllers are gone: no final state).
    pub fn sweep(&mut self, now: i64) -> Vec<String> {
        let gone: Vec<String> = self.sessions.iter().filter(|s| !Self::alive(s, now)).map(|s| s.sid.clone()).collect();
        self.sessions.retain(|s| Self::alive(s, now));
        self.spend(&gone, now);
        gone
    }
    /// A new session (the sid is neither live nor spent). One per controller device (its older one is replaced) and at most four
    /// (the oldest is evicted). Returns the replaced sids: each gets a final state `why: "replaced"`.
    pub fn open(&mut self, sid: &str, device: &str, seq: i64, ts: i64, talk: bool, now: i64) -> Vec<String> {
        self.sweep(now);
        let mut evicted: Vec<String> = self.sessions.iter().filter(|s| s.device == device || s.sid == sid).map(|s| s.sid.clone()).collect();
        self.sessions.retain(|s| s.device != device && s.sid != sid);
        while self.sessions.len() >= MAX_SESSIONS {
            evicted.push(self.sessions.remove(0).sid);
        }
        self.spend(&evicted, now);
        self.sessions.push(PadSession {
            sid: sid.to_string(),
            device: device.to_string(),
            last_seq: seq,
            offset: now - ts,
            last_at: now,
            opened_at: now,
            vis: true,
            talk,
            pushed_at: None,
        });
        evicted
    }
    pub fn close(&mut self, sid: &str, now: i64) -> bool {
        let n = self.sessions.len();
        self.sessions.retain(|s| s.sid != sid);
        if self.sessions.len() == n {
            return false;
        }
        self.spend(&[sid.to_string()], now);
        true
    }
    /// × on the receiver: end that device's sessions and refuse it until `clear_refused` (the call changed).
    pub fn refuse(&mut self, device: &str, now: i64) -> Vec<String> {
        let gone: Vec<String> = self.sessions.iter().filter(|s| s.device == device).map(|s| s.sid.clone()).collect();
        self.sessions.retain(|s| s.device != device);
        self.refused.insert(device.to_string());
        self.spend(&gone, now);
        gone
    }
    pub fn clear_refused(&mut self) {
        self.refused.clear();
    }
    /// Pad switched off: every session ends (each gets a final state `why: "off"`).
    pub fn clear(&mut self, now: i64) -> Vec<String> {
        let gone: Vec<String> = self.sessions.iter().map(|s| s.sid.clone()).collect();
        self.sessions.clear();
        self.spend(&gone, now);
        gone
    }
    /// The rate bucket of one controller device. At most 16 are kept; to make room only buckets idle for ≥ 2 s are
    /// forgotten (they are full again, so nothing is lost). None = no room: the message is dropped (`rate`).
    pub fn bucket(&mut self, device: &str, now: i64) -> Option<&mut RateBucket> {
        if let Some(i) = self.buckets.iter().position(|(d, _)| d == device) {
            return Some(&mut self.buckets[i].1);
        }
        if self.buckets.len() >= MAX_BUCKETS {
            self.buckets.retain(|(_, b)| !b.last.map(|l| now - l >= BUCKET_IDLE_MS).unwrap_or(false));
            if self.buckets.len() >= MAX_BUCKETS {
                return None;
            }
        }
        self.buckets.push((device.to_string(), RateBucket::default()));
        self.buckets.last_mut().map(|(_, b)| b)
    }
    pub fn bucket_count(&self) -> usize {
        self.buckets.len()
    }
    /// How long an unsolicited state push to `sid` must wait: 0 = now, n = ms, −1 = not at all (no session,
    /// `vis:false`, or a talking-only change nobody asked for). Pushes ≤ 4/s, talking-only changes ≤ 2/s.
    pub fn push_wait(&self, sid: &str, now: i64, talk_only: bool) -> i64 {
        let Some(s) = self.get(sid, now) else { return -1 };
        if !s.vis || (talk_only && !s.talk) {
            return -1;
        }
        let Some(at) = s.pushed_at else { return 0 };
        let gap = if talk_only { TALK_PUSH_EVERY_MS } else { PUSH_EVERY_MS };
        let since = now - at;
        if since >= gap {
            0
        } else {
            gap - since
        }
    }
    /// A state (reply or push) went to `sid`.
    pub fn mark_pushed(&mut self, sid: &str, now: i64) {
        if let Some(s) = self.sessions.iter_mut().find(|s| s.sid == sid) {
            s.pushed_at = Some(now);
        }
    }
}

// ---- commands ----------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub enum PadCmd {
    Open { talk: bool },
    Ping { vis: bool },
    Close,
    SoundPlay { url: String },
    SoundStop,
    Mic { muted: bool },
    Deafen { on: bool },
    Brb { on: bool },
    Vol { user: String, cat: &'static str, level: Option<f64>, muted: Option<bool>, live: bool },
    VolReset { user: String },
    Leave,
}

impl PadCmd {
    pub fn op(&self) -> &'static str {
        match self {
            PadCmd::Open { .. } => "open",
            PadCmd::Ping { .. } => "ping",
            PadCmd::Close => "close",
            PadCmd::SoundPlay { .. } => "sound.play",
            PadCmd::SoundStop => "sound.stop",
            PadCmd::Mic { .. } => "mic",
            PadCmd::Deafen { .. } => "deafen",
            PadCmd::Brb { .. } => "brb",
            PadCmd::Vol { .. } => "vol",
            PadCmd::VolReset { .. } => "vol.reset",
            PadCmd::Leave => "leave",
        }
    }
    /// The parsed command as JSON (`{op, …fields}`, the web's PadCmd object) — what the executor hands to QML.
    pub fn to_json(&self) -> Value {
        let mut m = Map::new();
        m.insert("op".into(), Value::from(self.op()));
        match self {
            PadCmd::Open { talk } => {
                m.insert("talk".into(), Value::from(*talk));
            }
            PadCmd::Ping { vis } => {
                m.insert("vis".into(), Value::from(*vis));
            }
            PadCmd::SoundPlay { url } => {
                m.insert("url".into(), Value::from(url.as_str()));
            }
            PadCmd::Mic { muted } => {
                m.insert("muted".into(), Value::from(*muted));
            }
            PadCmd::Deafen { on } | PadCmd::Brb { on } => {
                m.insert("on".into(), Value::from(*on));
            }
            PadCmd::Vol { user, cat, level, muted, live } => {
                m.insert("user".into(), Value::from(user.as_str()));
                m.insert("cat".into(), Value::from(*cat));
                if let Some(l) = level {
                    m.insert("level".into(), num(*l));
                }
                if let Some(x) = muted {
                    m.insert("muted".into(), Value::from(*x));
                }
                m.insert("live".into(), Value::from(*live));
            }
            PadCmd::VolReset { user } => {
                m.insert("user".into(), Value::from(user.as_str()));
            }
            PadCmd::Close | PadCmd::SoundStop | PadCmd::Leave => {}
        }
        Value::Object(m)
    }
}

/// A float as JSON the way JavaScript writes it: an integral value as an integer (2, not 2.0).
fn num(x: f64) -> Value {
    if x.fract() == 0.0 && x.abs() <= MAX_SAFE {
        Value::from(x as i64)
    } else {
        serde_json::Number::from_f64(x).map(Value::Number).unwrap_or(Value::Null)
    }
}

/// Rule 6 for one command (after its sid/seq/v): `ts`, `room`, `gen` (not on open), a known op and its fields with
/// their exact types. Unknown fields are ignored. None = `bad`.
fn parse_cmd(c: &Map<String, Value>) -> Option<PadCmd> {
    uint(c.get("ts"))?;
    if !valid_room(c.get("room")?.as_str()?) {
        return None;
    }
    let op = c.get("op")?.as_str()?;
    if op != "open" && !valid_id(c.get("gen")?.as_str()?) {
        return None;
    }
    let boolean = |k: &str| c.get(k).and_then(|v| v.as_bool());
    match op {
        "open" => match c.get("want") {
            None => Some(PadCmd::Open { talk: false }),
            Some(w) => {
                let a = w.as_array()?;
                if !a.iter().all(|x| x.is_string()) {
                    return None;
                }
                Some(PadCmd::Open { talk: a.iter().any(|x| x.as_str() == Some("talk")) })
            }
        },
        "ping" => Some(PadCmd::Ping { vis: boolean("vis")? }),
        "close" => Some(PadCmd::Close),
        "sound.stop" => Some(PadCmd::SoundStop),
        "leave" => Some(PadCmd::Leave),
        "sound.play" => {
            let url = c.get("url")?.as_str()?;
            (!url.is_empty() && cp_len(url) <= MAX_URL).then(|| PadCmd::SoundPlay { url: url.to_string() })
        }
        "mic" => Some(PadCmd::Mic { muted: boolean("muted")? }),
        "deafen" => Some(PadCmd::Deafen { on: boolean("on")? }),
        "brb" => Some(PadCmd::Brb { on: boolean("on")? }),
        "vol" => {
            let user = c.get("user")?.as_str()?;
            if !valid_user(user) {
                return None;
            }
            let cat_in = c.get("cat")?.as_str()?;
            let cat = *PAD_CATS.iter().find(|k| **k == cat_in)?;
            let (has_level, has_muted) = (c.contains_key("level"), c.contains_key("muted"));
            if !has_level && !has_muted {
                return None;
            }
            let lv = if has_level { Some(level(c.get("level"))?) } else { None };
            let muted = if has_muted { Some(boolean("muted")?) } else { None };
            let live = match c.get("live") {
                None => false,
                Some(v) => v.as_bool()?,
            };
            // a live slider push carries only the level (no disk write); mute changes are never "live"
            if live && (!has_level || has_muted) {
                return None;
            }
            Some(PadCmd::Vol { user: user.to_string(), cat, level: lv, muted, live })
        }
        "vol.reset" => {
            let user = c.get("user")?.as_str()?;
            valid_user(user).then(|| PadCmd::VolReset { user: user.to_string() })
        }
        _ => None,
    }
}

/// The receiver's own state the gates read (the same flags its own buttons see).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PadMe {
    pub mic_muted: bool,
    pub ptt: bool,
    /// a moderator's server mute lock
    pub server_muted: bool,
    pub deaf: bool,
    pub brb: bool,
    /// `pad_gate` / `use_soundboard`: this call's roles let me play sounds
    pub sound_role: bool,
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PadCall {
    pub room: String,
    /// the receiver's call generation
    pub gen: String,
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PadSounds {
    /// my favourites (account data dev.tinhat.soundboard_personal)
    pub favourites: Vec<String>,
    /// room id → that room's sounds (dev.tinhat.soundboard state); only the call room's count
    pub room_sounds: HashMap<String, Vec<String>>,
    /// this device's Recent list
    pub recent: Vec<String>,
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PadCtx {
    /// Settings → Calls → Pad on this receiver
    pub pad_on: bool,
    /// the call this device is in, None = not in a call
    pub call: Option<PadCall>,
    /// bare user ids of the people in the call with me (not me)
    pub participants: Vec<String>,
    pub me: PadMe,
    pub sounds: PadSounds,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PadVerdict {
    /// run it (when `run`) and reply ok
    pub ok: bool,
    /// the refusal code to reply with ("" with ok:false = a silent drop)
    pub why: &'static str,
    /// for the log: "" when ok, the refusal code, or the drop reason
    pub reason: &'static str,
    /// answer with one state `re: <seq>` (never for a `vol` with `live: true`, never for a drop)
    pub reply: bool,
    pub re: i64,
    pub sid: String,
    /// call the receiver's own command for this op (false: nothing to change — every op is a set)
    pub run: bool,
    pub cmd: Option<PadCmd>,
    /// sids an `open` replaced (send each a final state `why: "replaced"`)
    pub evicted: Vec<String>,
}

fn drop_v(reason: &'static str) -> PadVerdict {
    PadVerdict { ok: false, why: "", reason, reply: false, re: 0, sid: String::new(), run: false, cmd: None, evicted: Vec::new() }
}

/// A sound the receiver plays: in its favourites, the CALL room's sounds or its Recent list (exact url).
pub fn sound_allowed(url: &str, room: &str, s: &PadSounds) -> bool {
    if url.is_empty() {
        return false;
    }
    if s.favourites.iter().any(|u| u == url) || s.recent.iter().any(|u| u == url) {
        return true;
    }
    s.room_sounds.get(room).map(|l| l.iter().any(|u| u == url)).unwrap_or(false)
}

/// The receiver's whole decision about one `dev.tinhat.pad.cmd` (docs/PAD.md "Receiving a command" — the order is
/// normative). Commits to the session table and rate bucket it is handed (opening / renewing / closing a session,
/// accepting a seq); deterministic: the same table + message + facts + now give the same verdict.
pub fn check_pad_message(t: &mut PadSessions, typ: &str, content: &Value, f: &PadFacts, ctx: &PadCtx, now: i64) -> PadVerdict {
    if typ != PAD_CMD {
        return drop_v("wrong-type");
    }
    if let Err(reason) = check_trust(f) {
        return drop_v(reason);
    }
    let Some(c) = content.as_object().filter(|_| wire_ok(content)) else { return drop_v("bad-envelope") };
    let sid = match c.get("sid").and_then(|s| s.as_str()) {
        Some(s) if valid_sid(s) => s.to_string(),
        _ => return drop_v("bad-envelope"),
    };
    let Some(seq) = int(c.get("seq")).filter(|s| *s >= 1) else { return drop_v("bad-envelope") };
    // a message that says it is a live slider push is never answered (ok or refused)
    let reply = !(c.get("op").and_then(|o| o.as_str()) == Some("vol") && c.get("live").and_then(|l| l.as_bool()) == Some(true));
    let refuse = |why: &'static str| PadVerdict { ok: false, why, reason: why, reply, re: seq, sid: sid.clone(), run: false, cmd: None, evicted: Vec::new() };

    let Some(bucket) = t.bucket(&f.sender_device, now) else { return drop_v("rate") };
    match bucket.hit(now) {
        Hit::Busy => return refuse("busy"),
        Hit::Drop => return drop_v("rate"),
        Hit::Ok => {}
    }
    match int(c.get("v")) {
        Some(v) if v >= 1 => {
            if v > PAD_V {
                return refuse("version");
            }
        }
        _ => return refuse("bad"),
    }
    if !ctx.pad_on {
        return refuse("off");
    }
    if t.refused.contains(&f.sender_device) {
        return refuse("closed");
    }
    let Some(cmd) = parse_cmd(c) else { return refuse("bad") };
    let ts = int(c.get("ts")).unwrap_or(0);
    let room = str_of(c.get("room"));
    let ok = |run: bool, cmd: PadCmd, evicted: Vec<String>| PadVerdict { ok: true, why: "", reason: "", reply, re: seq, sid: sid.clone(), run, cmd: Some(cmd), evicted };

    if let PadCmd::Open { talk } = cmd {
        let mut existed = false;
        if let Some(cur) = t.get_mut(&sid, now) {
            if cur.device != f.sender_device {
                return refuse("no-session");
            }
            if seq <= cur.last_seq {
                return drop_v("old-seq");
            }
            cur.last_seq = seq;
            cur.last_at = now;
            existed = true;
        } else if t.is_spent(&sid, now) {
            return refuse("no-session"); // a session that ended is never opened again
        }
        if open_stale(ts, now) {
            return refuse("stale");
        }
        let Some(call) = &ctx.call else { return refuse("not-in-call") };
        if room != call.room {
            return refuse("other-call");
        }
        if existed {
            // re-open of a live session: the clock offset can only tighten (a held re-open must not widen it)
            if let Some(cur) = t.get_mut(&sid, now) {
                cur.offset = cur.offset.min(now - ts);
                cur.vis = true;
                cur.talk = talk;
            }
            return ok(false, cmd, Vec::new());
        }
        if !t.room_to_open(now) {
            return refuse("busy");
        }
        let evicted = t.open(&sid, &f.sender_device, seq, ts, talk, now);
        return ok(false, cmd, evicted);
    }

    let Some(s) = t.get_mut(&sid, now).filter(|s| s.device == f.sender_device) else { return refuse("no-session") };
    if seq <= s.last_seq {
        return drop_v("old-seq");
    }
    s.last_seq = seq;
    s.last_at = now;
    if stale_check(s.offset, ts, now) {
        return refuse("stale");
    }
    match cmd {
        PadCmd::Ping { vis } => {
            s.vis = vis;
            return ok(false, cmd, Vec::new());
        }
        PadCmd::Close => {
            t.close(&sid, now);
            return ok(false, cmd, Vec::new());
        }
        _ => {}
    }
    let Some(call) = &ctx.call else { return refuse("not-in-call") };
    if room != call.room || str_of(c.get("gen")) != call.gen {
        return refuse("other-call");
    }
    let me = &ctx.me;
    match &cmd {
        PadCmd::SoundPlay { url } => {
            if !me.sound_role {
                return refuse("off-role");
            }
            if me.server_muted {
                return refuse("server-muted");
            }
            if !sound_allowed(url, &call.room, &ctx.sounds) {
                return refuse("unknown-sound");
            }
            ok(true, cmd, Vec::new())
        }
        PadCmd::Mic { muted: true } => ok(!me.mic_muted, cmd, Vec::new()), // muting is always allowed
        PadCmd::Mic { muted: false } => {
            if me.server_muted {
                return refuse("server-muted");
            }
            if me.ptt {
                return refuse("ptt");
            }
            ok(me.mic_muted, cmd, Vec::new())
        }
        PadCmd::Deafen { on } => {
            let run = *on != me.deaf;
            ok(run, cmd, Vec::new())
        }
        PadCmd::Brb { on } => {
            let run = *on != me.brb;
            ok(run, cmd, Vec::new())
        }
        PadCmd::Vol { user, .. } | PadCmd::VolReset { user } => {
            if !ctx.participants.iter().any(|p| p == user) {
                return refuse("not-here");
            }
            ok(true, cmd, Vec::new())
        }
        PadCmd::SoundStop | PadCmd::Leave => ok(true, cmd, Vec::new()),
        PadCmd::Open { .. } | PadCmd::Ping { .. } | PadCmd::Close => unreachable!("handled above"),
    }
}

// ---- the state --------------------------------------------------------------------------------------------

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}
fn json_len(v: &Value) -> usize {
    serde_json::to_string(v).map(|s| s.len()).unwrap_or(usize::MAX)
}

/// The `dev.tinhat.pad.state` content: a full snapshot, from the receiver's own stores (`input` = the web's
/// PadStateIn as JSON: `{sid, rseq, rt, re, ok, why, call: null | {room, gen, since, me: {mic, deaf, brb, talk,
/// sound: url|"", gate: {allowed, hint}}, people: [{u, talk, micOff, away, vol: {cat: n}, muted: {cat: bool}}],
/// recent: [{name, url, emoji}]}}`). Per person only non-default volumes (≠ 1 after rounding to 0.01, clamped to
/// 0–2) and the muted categories; `vol` / `mute` are left out when empty. `people` ≤ 64 (the rest counted in `more`),
/// `recent` ≤ 8, and the whole content ≤ 16 KiB of UTF-8 JSON (if not: recent entries are dropped from the end, then
/// people, counted in `more`). `sid` / `why` / `room` / `gen` that aren't valid ids are sent as "" (the controller then
/// drops the state), so what is left without people and recent is bounded (the worst case, a 2048-character sound url
/// and a 200-character hint of control characters, is under 15 KiB). Returns the content and its size in bytes (the
/// same as the web's: key order differs, lengths don't).
pub fn build_state(input: &Value) -> (Value, usize) {
    let empty = Map::new();
    let inp = input.as_object().unwrap_or(&empty);
    let mut out = Map::new();
    out.insert("v".into(), Value::from(PAD_V));
    let sid = str_of(inp.get("sid"));
    out.insert("sid".into(), Value::from(if valid_sid(sid) { sid } else { "" }));
    for k in ["rseq", "rt", "re"] {
        out.insert(k.into(), Value::from(uint(inp.get(k)).unwrap_or(0)));
    }
    out.insert("ok".into(), Value::from(is_true(inp.get("ok"))));
    let why = inp.get("why").and_then(|w| w.as_str()).filter(|w| valid_why(w)).unwrap_or("");
    out.insert("why".into(), Value::from(why));
    out.insert("ops".into(), Value::from(PAD_OPS.to_vec()));
    let Some(ci) = obj(inp.get("call")) else {
        out.insert("call".into(), Value::Null);
        let v = Value::Object(out);
        let n = json_len(&v);
        return (v, n);
    };
    let mi = obj(ci.get("me")).unwrap_or(&empty);
    let gi = obj(mi.get("gate")).unwrap_or(&empty);
    let mut people: Vec<Value> = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    let mut valid = 0usize;
    for p in ci.get("people").and_then(|x| x.as_array()).map(|a| a.as_slice()).unwrap_or(&[]) {
        let Some(p) = p.as_object() else { continue };
        let Some(u) = p.get("u").and_then(|x| x.as_str()) else { continue };
        if !valid_user(u) || !seen.insert(u) {
            continue;
        }
        valid += 1;
        if people.len() >= MAX_PEOPLE {
            continue;
        }
        let mut e = Map::new();
        e.insert("u".into(), Value::from(u));
        e.insert("talk".into(), Value::from(is_true(p.get("talk"))));
        e.insert("micOff".into(), Value::from(is_true(p.get("micOff"))));
        e.insert("away".into(), Value::from(is_true(p.get("away"))));
        let vi = obj(p.get("vol")).unwrap_or(&empty);
        let mu = obj(p.get("muted")).unwrap_or(&empty);
        let mut vol = Map::new();
        let mut mute: Vec<Value> = Vec::new();
        for cat in PAD_CATS {
            if let Some(x) = vi.get(cat).and_then(|x| x.as_f64()).filter(|x| x.is_finite()) {
                let r = round2(x.clamp(0.0, 2.0));
                if r != 1.0 {
                    vol.insert(cat.into(), num(r));
                }
            }
            if is_true(mu.get(cat)) {
                mute.push(Value::from(cat));
            }
        }
        if !vol.is_empty() {
            e.insert("vol".into(), Value::Object(vol));
        }
        if !mute.is_empty() {
            e.insert("mute".into(), Value::Array(mute));
        }
        people.push(Value::Object(e));
    }
    let mut recent: Vec<Value> = Vec::new();
    for r in ci.get("recent").and_then(|x| x.as_array()).map(|a| a.as_slice()).unwrap_or(&[]) {
        if recent.len() >= MAX_RECENT {
            break;
        }
        let Some(r) = r.as_object() else { continue };
        let url = str_of(r.get("url"));
        if url.is_empty() || cp_len(url) > MAX_URL {
            continue;
        }
        let mut e = Map::new();
        e.insert("name".into(), Value::from(cp_trunc(str_of(r.get("name")), MAX_NAME)));
        e.insert("url".into(), Value::from(url));
        e.insert("emoji".into(), Value::from(cp_trunc(str_of(r.get("emoji")), MAX_EMOJI)));
        recent.push(Value::Object(e));
    }
    let mic = str_of(mi.get("mic"));
    let sound = str_of(mi.get("sound"));
    let mut me = Map::new();
    me.insert("mic".into(), Value::from(if PAD_MIC.contains(&mic) { mic } else { "on" }));
    me.insert("deaf".into(), Value::from(is_true(mi.get("deaf"))));
    me.insert("brb".into(), Value::from(is_true(mi.get("brb"))));
    me.insert("talk".into(), Value::from(is_true(mi.get("talk"))));
    let snd = if !sound.is_empty() && cp_len(sound) <= MAX_URL {
        let mut s = Map::new();
        s.insert("url".into(), Value::from(sound));
        Value::Object(s)
    } else {
        Value::Null
    };
    me.insert("sound".into(), snd);
    let mut gate = Map::new();
    gate.insert("allowed".into(), Value::from(is_true(gi.get("allowed"))));
    gate.insert("hint".into(), Value::from(cp_trunc(str_of(gi.get("hint")), MAX_HINT)));
    me.insert("gate".into(), Value::Object(gate));
    let mut more = valid - people.len();

    let assemble = |people: &[Value], recent: &[Value], more: usize| -> Value {
        let mut call = Map::new();
        let (room, gen) = (str_of(ci.get("room")), str_of(ci.get("gen")));
        call.insert("room".into(), Value::from(if valid_room(room) { room } else { "" }));
        call.insert("gen".into(), Value::from(if valid_id(gen) { gen } else { "" }));
        call.insert("since".into(), Value::from(uint(ci.get("since")).unwrap_or(0)));
        call.insert("me".into(), Value::Object(me.clone()));
        call.insert("people".into(), Value::Array(people.to_vec()));
        call.insert("more".into(), Value::from(more));
        call.insert("recent".into(), Value::Array(recent.to_vec()));
        let mut o = out.clone();
        o.insert("call".into(), Value::Object(call));
        Value::Object(o)
    };
    let mut v = assemble(&people, &recent, more);
    let mut bytes = json_len(&v);
    while bytes > MAX_STATE_BYTES && !recent.is_empty() {
        recent.pop();
        v = assemble(&people, &recent, more);
        bytes = json_len(&v);
    }
    while bytes > MAX_STATE_BYTES && !people.is_empty() {
        people.pop();
        more += 1;
        v = assemble(&people, &recent, more);
        bytes = json_len(&v);
    }
    (v, bytes)
}

/// The controller's side of one session: the receiver device it opened, and the newest state it accepted.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PadCtrlSession {
    pub sid: String,
    pub device: String,
    pub last_rseq: i64,
}

fn valid_state_body(c: &Map<String, Value>) -> bool {
    let is_bool = |m: &Map<String, Value>, k: &str| m.get(k).map(|v| v.is_boolean()).unwrap_or(false);
    let str_max = |v: Option<&Value>, max: usize, nonempty: bool| v.and_then(|x| x.as_str()).map(|s| cp_len(s) <= max && (!nonempty || !s.is_empty())).unwrap_or(false);
    if uint(c.get("rt")).is_none() || uint(c.get("re")).is_none() || !is_bool(c, "ok") || !c.get("why").and_then(|w| w.as_str()).map(valid_why).unwrap_or(false) {
        return false;
    }
    match c.get("ops").and_then(|o| o.as_array()) {
        Some(ops) if ops.len() <= 64 && ops.iter().all(|o| str_max(Some(o), 64, false)) => {}
        _ => return false,
    }
    let call = match c.get("call") {
        None => return false,
        Some(Value::Null) => return true,
        Some(Value::Object(m)) => m,
        Some(_) => return false,
    };
    if !call.get("room").and_then(|x| x.as_str()).map(valid_room).unwrap_or(false)
        || !call.get("gen").and_then(|x| x.as_str()).map(valid_id).unwrap_or(false)
        || uint(call.get("since")).is_none()
        || uint(call.get("more")).is_none()
    {
        return false;
    }
    let Some(me) = obj(call.get("me")) else { return false };
    if !me.get("mic").and_then(|x| x.as_str()).map(|m| PAD_MIC.contains(&m)).unwrap_or(false) || !["deaf", "brb", "talk"].iter().all(|k| is_bool(me, k)) {
        return false;
    }
    match me.get("sound") {
        Some(Value::Null) => {}
        Some(Value::Object(s)) if str_max(s.get("url"), MAX_URL, true) => {}
        _ => return false,
    }
    match obj(me.get("gate")) {
        Some(g) if is_bool(g, "allowed") && str_max(g.get("hint"), MAX_HINT, false) => {}
        _ => return false,
    }
    let Some(people) = call.get("people").and_then(|x| x.as_array()) else { return false };
    if people.len() > MAX_PEOPLE {
        return false;
    }
    for p in people {
        let Some(p) = p.as_object() else { return false };
        if !p.get("u").and_then(|x| x.as_str()).map(valid_user).unwrap_or(false) || !["talk", "micOff", "away"].iter().all(|k| is_bool(p, k)) {
            return false;
        }
        if let Some(vol) = p.get("vol") {
            let Some(vol) = vol.as_object() else { return false };
            if !vol.iter().all(|(k, v)| PAD_CATS.contains(&k.as_str()) && level(Some(v)).is_some()) {
                return false;
            }
        }
        if let Some(mute) = p.get("mute") {
            let Some(mute) = mute.as_array() else { return false };
            if !mute.iter().all(|m| m.as_str().map(|s| PAD_CATS.contains(&s)).unwrap_or(false)) {
                return false;
            }
        }
    }
    let Some(recent) = call.get("recent").and_then(|x| x.as_array()) else { return false };
    if recent.len() > MAX_RECENT {
        return false;
    }
    recent.iter().all(|r| {
        r.as_object()
            .map(|r| str_max(r.get("name"), MAX_NAME, false) && str_max(r.get("url"), MAX_URL, true) && str_max(r.get("emoji"), MAX_EMOJI, false))
            .unwrap_or(false)
    })
}

/// The controller's decision about one `dev.tinhat.pad.state`: trust rules 1–4, from the device this session was
/// opened with, our sid, a newer `rseq` than the last accepted one (highest wins; older or equal ones are ignored)
/// and a well-formed body (unknown fields ignored). Accepting updates `s.last_rseq`; the state is `content`
/// itself. Every refusal is silent (Err = the reason for the log).
pub fn accept_state(s: &mut PadCtrlSession, typ: &str, content: &Value, f: &PadFacts) -> Result<(), &'static str> {
    if typ != PAD_STATE {
        return Err("wrong-type");
    }
    check_trust(f)?;
    if f.sender_device != s.device {
        return Err("other-device");
    }
    let Some(c) = content.as_object().filter(|_| wire_ok(content)) else { return Err("bad") };
    match int(c.get("v")) {
        Some(v) if v >= 1 => {
            if v > PAD_V {
                return Err("version");
            }
        }
        _ => return Err("bad"),
    }
    if c.get("sid").and_then(|x| x.as_str()) != Some(s.sid.as_str()) {
        return Err("other-session");
    }
    let Some(rseq) = int(c.get("rseq")).filter(|r| *r >= 1) else { return Err("bad") };
    if rseq <= s.last_rseq {
        return Err("old-rseq");
    }
    if !valid_state_body(c) {
        return Err("bad");
    }
    s.last_rseq = rseq;
    Ok(())
}

// ---- discovery ---------------------------------------------------------------------------------------------

/// One of my other devices in the call. `why`: "" (ready), "unverified" (THIS device isn't verified), "untrusted"
/// (that device isn't cross-signing trusted) or "version" (its membership has no `pad1` cap: an older Tin Hat,
/// another client, or Pad turned off there).
#[derive(Clone, Debug, PartialEq)]
pub struct PadTarget {
    pub device: String,
    pub created_ts: f64,
    pub ready: bool,
    pub why: &'static str,
}

fn finite(v: Option<&Value>) -> Option<f64> {
    v?.as_f64().filter(|x| x.is_finite())
}
fn entry_live(c: &Map<String, Value>, origin: f64, now: i64) -> bool {
    if c.is_empty() {
        return false;
    }
    let created = finite(c.get("created_ts")).or(if origin > 0.0 { Some(origin) } else { None });
    let Some(created) = created else { return true };
    let expires = finite(c.get("expires")).unwrap_or(MEMBERSHIP_EXPIRES_MS as f64);
    (now as f64) <= created + expires
}

/// Pad targets in one call room, newest membership (`created_ts`) first (ties: device id): every live, well-formed
/// MatrixRTC session (the same liveness / schema / state-key ownership as [`crate::keyrules::live_sessions_from`])
/// whose sender is me and whose device isn't this one. `events` = the room's raw `org.matrix.msc3401.call.member`
/// state events (`{sender, state_key, origin_server_ts, content}`, session form or the legacy `memberships[]` form);
/// `trusted` = my device ids that are cross-signing trusted. The default target is the first ready one.
pub fn pad_targets(me: &str, my_device: &str, now: i64, self_verified: bool, trusted: &[String], events: &[Value]) -> Vec<PadTarget> {
    let mut out: Vec<PadTarget> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for ev in events {
        let Some(ev) = ev.as_object() else { continue };
        let sender = str_of(ev.get("sender"));
        let Some(c) = obj(ev.get("content")) else { continue };
        if sender.is_empty() || sender != me || c.is_empty() {
            continue;
        }
        let origin = uint(ev.get("origin_server_ts")).unwrap_or(0) as f64;
        let array = c.get("memberships").and_then(|m| m.as_array());
        let entries: Vec<&Map<String, Value>> = match array {
            Some(a) => a.iter().filter_map(|m| m.as_object()).filter(|m| entry_live(m, origin, now)).collect(),
            None if entry_live(c, origin, now) => vec![c],
            None => vec![],
        };
        let state_key = str_of(ev.get("state_key"));
        for e in entries {
            let s = |k: &str| e.get(k).and_then(|v| v.as_str());
            if e.contains_key("application") && s("application") != Some("m.call") {
                continue;
            }
            if e.contains_key("scope") && s("scope") != Some("m.room") {
                continue;
            }
            let dev = s("device_id").unwrap_or("");
            if dev.is_empty() || !crate::keyrules::state_key_owned(state_key, sender, dev, array.is_some()) {
                continue;
            }
            let well_formed = s("application") == Some("m.call") && s("scope") == Some("m.room") && s("call_id") == Some("") && finite(e.get("expires")).unwrap_or(0.0) > 0.0;
            // only a well-formed entry claims its device (a malformed one can't hide a good one)
            if !well_formed || !seen.insert(dev.to_string()) || dev == my_device {
                continue;
            }
            let marker = obj(e.get("dev.tinhat.client")).or_else(|| obj(c.get("dev.tinhat.client")));
            let capped = marker.and_then(|m| m.get("caps")).and_then(|x| x.as_array()).map(|a| a.iter().any(|x| x.as_str() == Some(PAD_CAP))).unwrap_or(false);
            let why = if !self_verified {
                "unverified"
            } else if !trusted.iter().any(|t| t == dev) {
                "untrusted"
            } else if !capped {
                "version"
            } else {
                ""
            };
            out.push(PadTarget { device: dev.to_string(), created_ts: finite(e.get("created_ts")).unwrap_or(origin), ready: why.is_empty(), why });
        }
    }
    out.sort_by(|a, b| b.created_ts.partial_cmp(&a.created_ts).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.device.cmp(&b.device)));
    out
}

// ---- the words ---------------------------------------------------------------------------------------------

const WHY_TEXT: [(&str, &str); 16] = [
    ("off", "Pad is turned off on {device} (Settings → Calls)."),
    ("not-in-call", "{device} isn't in a call any more."),
    ("other-call", "{device} switched calls."),
    ("unknown-sound", "{device} doesn't have that sound yet — try again."),
    ("off-role", "The soundboard is off for your role in this call."),
    ("server-muted", "A moderator muted you for everyone — sounds are off until you're unmuted"),
    ("ptt", "Push to talk is on at {device} — use the key there."),
    ("stale", "That took too long to reach {device} — try again."),
    ("busy", "Slow down a little."),
    ("version", "Update Tin Hat on {device} to use Pad."),
    ("ended", "{device} left the call."),
    ("replaced", "Another of your devices took over Pad on {device}."),
    ("closed", "Pad was closed on {device} for this call."),
    ("unverified", "Verify this device to use Pad."),
    ("untrusted", "Verify {device} to use Pad."),
    ("unreachable", "Not reaching {device}"),
];
/// For `mic`, `server-muted` is the call's existing lock notice.
const MIC_SERVER_MUTED: &str = "A moderator muted your mic for everyone — ask them to unmute you";

/// The one text per code (both apps). `device` = the receiver's name (Settings → Devices & security); `op` = the
/// refused command's op. `bad` / `not-here` / `no-session` and unknown codes have no text ("").
pub fn why_text(code: &str, device: &str, op: &str) -> String {
    if code == "server-muted" && op == "mic" {
        return MIC_SERVER_MUTED.to_string();
    }
    WHY_TEXT.iter().find(|(k, _)| *k == code).map(|(_, t)| t.replace("{device}", device)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const VECTORS: &str = include_str!("testvectors/pad.json");

    fn vectors() -> Value {
        serde_json::from_str(VECTORS).expect("pad.json parses")
    }
    /// objects merge, arrays and scalars replace (the web harness's `merge`)
    fn merge(base: &Value, over: Option<&Value>) -> Value {
        match (base, over) {
            (_, None) => base.clone(),
            (Value::Object(b), Some(Value::Object(o))) => {
                let mut out = b.clone();
                for (k, v) in o {
                    let merged = merge(out.get(k).unwrap_or(&Value::Null), Some(v));
                    out.insert(k.clone(), merged);
                }
                Value::Object(out)
            }
            (_, Some(o)) => o.clone(),
        }
    }
    fn without(mut v: Value, keys: Option<&Value>) -> Value {
        if let (Some(m), Some(ks)) = (v.as_object_mut(), keys.and_then(|k| k.as_array())) {
            for k in ks {
                m.remove(k.as_str().unwrap());
            }
        }
        v
    }
    fn facts(v: &Value) -> PadFacts {
        serde_json::from_value(v.clone()).expect("facts")
    }
    fn ctx(v: &Value) -> PadCtx {
        serde_json::from_value(v.clone()).expect("ctx")
    }
    fn live(t: &PadSessions, now: i64) -> Value {
        Value::from(t.live(now).iter().map(|s| s.sid.clone()).collect::<Vec<_>>())
    }

    /// One message against a receiver table; "" or what differed.
    fn check_step(label: &str, t: &mut PadSessions, typ: &str, content: &Value, f: &PadFacts, c: &PadCtx, now: i64, e: &Value) -> String {
        let v = check_pad_message(t, typ, content, f, c, now);
        let mut bad: Vec<String> = Vec::new();
        let got = json!({"ok": v.ok, "why": v.why, "reason": v.reason, "reply": v.reply, "run": v.run});
        for k in ["ok", "why", "reason", "reply", "run"] {
            if got[k] != e[k] {
                bad.push(format!("{k}={} want {}", got[k], e[k]));
            }
        }
        let want_ev = e.get("evicted").cloned().unwrap_or(json!([]));
        if Value::from(v.evicted.clone()) != want_ev {
            bad.push(format!("evicted={:?} want {want_ev}", v.evicted));
        }
        if let Some(want) = e.get("cmd") {
            let have = v.cmd.as_ref().map(|c| c.to_json()).unwrap_or(Value::Null);
            if &have != want {
                bad.push(format!("cmd={have} want {want}"));
            }
        }
        if let Some(want) = e.get("live") {
            let have = live(t, now);
            if &have != want {
                bad.push(format!("live={have} want {want}"));
            }
        }
        if v.ok || !v.why.is_empty() {
            if Some(v.re) != content.get("seq").and_then(|s| s.as_i64()) {
                bad.push(format!("re={}", v.re));
            }
        } else if v.re != 0 {
            bad.push(format!("re={} on a drop", v.re));
        }
        if bad.is_empty() {
            String::new()
        } else {
            format!("{label}: {}", bad.join(", "))
        }
    }

    #[test]
    fn padrules_constants() {
        let v = vectors();
        let c = &v["const"];
        let ints: [(&str, i64); 19] = [
            ("v", PAD_V),
            ("leaseMs", LEASE_MS),
            ("maxSessions", MAX_SESSIONS as i64),
            ("staleMs", STALE_MS),
            ("openSkewMs", OPEN_SKEW_MS),
            ("busyEveryMs", BUSY_EVERY_MS),
            ("pushEveryMs", PUSH_EVERY_MS),
            ("talkPushEveryMs", TALK_PUSH_EVERY_MS),
            ("maxPeople", MAX_PEOPLE as i64),
            ("maxRecent", MAX_RECENT as i64),
            ("maxStateBytes", MAX_STATE_BYTES as i64),
            ("maxUrl", MAX_URL as i64),
            ("maxName", MAX_NAME as i64),
            ("maxEmoji", MAX_EMOJI as i64),
            ("maxHint", MAX_HINT as i64),
            ("bucketIdleMs", BUCKET_IDLE_MS),
            ("spentMs", SPENT_MS),
            ("maxSpent", MAX_SPENT as i64),
            ("maxDepth", MAX_DEPTH as i64),
        ];
        for (k, want) in ints {
            assert_eq!(c[k].as_i64(), Some(want), "const {k}");
        }
        assert_eq!(c["ratePerSec"].as_f64(), Some(RATE_PER_SEC));
        assert_eq!(c["rateBurst"].as_f64(), Some(RATE_BURST));
        assert_eq!(c["maxBuckets"].as_i64(), Some(MAX_BUCKETS as i64));
        assert_eq!(c["membershipExpiresMs"].as_i64(), Some(MEMBERSHIP_EXPIRES_MS));
        assert_eq!(MEMBERSHIP_EXPIRES_MS as u64, crate::keyrules::MEMBERSHIP_EXPIRES_MS);
        assert_eq!(c.as_object().unwrap().len(), 23, "every constant is checked");
        assert_eq!(v["names"], json!({"cap": PAD_CAP, "cmd": PAD_CMD, "state": PAD_STATE}));
        assert_eq!(v["ops"], json!(PAD_OPS));
        assert_eq!(v["cats"], json!(PAD_CATS));
    }

    #[test]
    fn padrules_trust_matrix() {
        let v = vectors();
        let d = &v["defaults"];
        let cases = v["trust"].as_array().unwrap();
        assert!(cases.len() >= 10);
        for c in cases {
            let name = c["name"].as_str().unwrap();
            let want = c["expect"].as_str().unwrap();
            let f = facts(&merge(&d["receiverFacts"], c.get("facts")));
            assert_eq!(check_trust(&f).err().unwrap_or(""), want, "trust: {name}");
            let mut t = PadSessions::default();
            let open = json!({"v": 1, "sid": "PhoneSession0001", "seq": 1, "ts": 1000, "room": d["ctx"]["call"]["room"], "op": "open"});
            let e = if want.is_empty() {
                json!({"ok": true, "why": "", "reason": "", "reply": true, "run": false})
            } else {
                json!({"ok": false, "why": "", "reason": want, "reply": false, "run": false})
            };
            let r = check_step("open", &mut t, PAD_CMD, &open, &f, &ctx(&d["ctx"]), 1000, &e);
            assert!(r.is_empty(), "trust via check_pad_message: {name}: {r}");
            if !want.is_empty() {
                assert!(t.live(1000).is_empty(), "a dropped open opens nothing: {name}");
            }
            let mut s = PadCtrlSession { sid: "PhoneSession0001".into(), device: f.sender_device.clone(), last_rseq: 0 };
            assert_eq!(accept_state(&mut s, PAD_STATE, &d["state"], &f).err().unwrap_or(""), want, "trust via accept_state: {name}");
            assert_eq!(s.last_rseq, if want.is_empty() { 1 } else { 0 }, "{name}");
        }
    }

    #[test]
    fn padrules_commands() {
        let v = vectors();
        let d = &v["defaults"];
        let cases = v["cmds"].as_array().unwrap();
        assert!(cases.len() >= 100);
        let mut fails: Vec<String> = Vec::new();
        for c in cases {
            let name = c["name"].as_str().unwrap();
            let mut t = PadSessions::default();
            let pre = json!({"v": 1, "sid": "PhoneSession0001", "seq": 1, "ts": 1000, "room": d["ctx"]["call"]["room"], "op": "open", "want": ["talk"]});
            assert!(check_pad_message(&mut t, PAD_CMD, &pre, &facts(&d["receiverFacts"]), &ctx(&d["ctx"]), 1000).ok, "{name}: the implicit open");
            let env = json!({"v": 1, "sid": "PhoneSession0001", "seq": 2, "ts": 1500, "room": d["ctx"]["call"]["room"], "gen": "g1"});
            let content = match c.get("raw") {
                Some(raw) => raw.clone(),
                None => without(merge(&env, c.get("msg")), c.get("drop")),
            };
            let typ = c.get("type").and_then(|x| x.as_str()).unwrap_or(PAD_CMD);
            let now = c.get("now").and_then(|x| x.as_i64()).unwrap_or(1500);
            let r = check_step("cmd", &mut t, typ, &content, &facts(&merge(&d["receiverFacts"], c.get("facts"))), &ctx(&merge(&d["ctx"], c.get("ctx"))), now, &c["expect"]);
            if !r.is_empty() {
                fails.push(format!("{name}: {r}"));
            }
        }
        assert!(fails.is_empty(), "{} failing:\n{}", fails.len(), fails.join("\n"));
    }

    #[test]
    fn padrules_sessions() {
        let v = vectors();
        let d = &v["defaults"];
        let mut fails: Vec<String> = Vec::new();
        for sc in v["sessions"].as_array().unwrap() {
            let name = sc["name"].as_str().unwrap();
            let mut t = PadSessions::default();
            for (i, st) in sc["steps"].as_array().unwrap().iter().enumerate() {
                let label = format!("{name}: step {}", i + 1);
                let now = st["now"].as_i64().unwrap();
                let e = &st["expect"];
                if let Some(op) = st.get("do").and_then(|x| x.as_str()) {
                    let sid = st.get("sid").and_then(|x| x.as_str()).unwrap_or("");
                    let mut sids: Option<Vec<String>> = None;
                    let mut wait: Option<i64> = None;
                    match op {
                        "refuse" => sids = Some(t.refuse(st["device"].as_str().unwrap(), now)),
                        "clearRefused" => t.clear_refused(),
                        "clear" => sids = Some(t.clear(now)),
                        "sweep" => sids = Some(t.sweep(now)),
                        "pushed" => t.mark_pushed(sid, now),
                        "pushWait" => wait = Some(t.push_wait(sid, now, st["talkOnly"].as_bool().unwrap())),
                        other => fails.push(format!("{label}: unknown do {other}")),
                    }
                    if let Some(want) = e.get("sids") {
                        if Value::from(sids.clone().unwrap_or_default()) != *want {
                            fails.push(format!("{label}: sids={sids:?} want {want}"));
                        }
                    }
                    if let Some(want) = e.get("wait") {
                        if wait != want.as_i64() {
                            fails.push(format!("{label}: wait={wait:?} want {want}"));
                        }
                    }
                    if let Some(want) = e.get("live") {
                        if live(&t, now) != *want {
                            fails.push(format!("{label}: live={}", live(&t, now)));
                        }
                    }
                    continue;
                }
                let f = facts(&merge(&merge(&d["receiverFacts"], sc.get("facts")), st.get("facts")));
                let cx = ctx(&merge(&merge(&d["ctx"], sc.get("ctx")), st.get("ctx")));
                let reps = st.get("repeat").and_then(|x| x.as_u64()).unwrap_or(1);
                for k in 0..reps {
                    let env = json!({"v": 1, "sid": "PhoneSession0001", "ts": now, "room": d["ctx"]["call"]["room"], "gen": "g1"});
                    let mut content = without(merge(&env, st.get("msg")), st.get("drop"));
                    if let Some(seq) = content.get("seq").and_then(|s| s.as_i64()) {
                        content["seq"] = json!(seq + k as i64);
                    }
                    let typ = st.get("type").and_then(|x| x.as_str()).unwrap_or(PAD_CMD);
                    let r = check_step(&format!("{label}.{}", k + 1), &mut t, typ, &content, &f, &cx, now, e);
                    if !r.is_empty() {
                        fails.push(r);
                    }
                }
            }
        }
        assert!(fails.is_empty(), "{} failing:\n{}", fails.len(), fails.join("\n"));
    }

    #[test]
    fn padrules_sound_allowed() {
        let v = vectors();
        for c in v["sounds"].as_array().unwrap() {
            let s: PadSounds = serde_json::from_value(merge(&v["defaults"]["ctx"]["sounds"], c.get("sounds"))).unwrap();
            assert_eq!(sound_allowed(c["url"].as_str().unwrap(), c["room"].as_str().unwrap(), &s), c["expect"].as_bool().unwrap(), "sound: {}", c["name"]);
        }
    }

    fn expand(input: &Value) -> Value {
        let mut inp = input.clone();
        if let Some(c) = inp.get_mut("call").and_then(|c| c.as_object_mut()) {
            if let Some(g) = c.remove("genPeople") {
                let n = g["n"].as_u64().unwrap();
                let len = g.get("len").and_then(|l| l.as_u64()).unwrap_or(0) as usize;
                let people = c.entry("people").or_insert(json!([])).as_array_mut().unwrap();
                for i in 1..=n {
                    let base = format!("@p{i}:example.org");
                    let u = if len == 0 || len <= base.len() { base.clone() } else { format!("@p{i}{}:example.org", "x".repeat(len - base.len())) };
                    people.push(json!({"u": u, "talk": false, "micOff": false, "away": false}));
                }
            }
            if let Some(g) = c.remove("genRecent") {
                let n = g["n"].as_u64().unwrap();
                let (nl, ul) = (g["nameLen"].as_u64().unwrap() as usize, g["urlLen"].as_u64().unwrap() as usize);
                let recent = c.entry("recent").or_insert(json!([])).as_array_mut().unwrap();
                for i in 1..=n {
                    let n0 = format!("Sound {i} ");
                    let u0 = format!("https://sounds.example.org/{i}/");
                    recent.push(json!({"name": format!("{n0}{}", "y".repeat(nl - n0.chars().count())), "url": format!("{u0}{}", "z".repeat(ul - u0.len())), "emoji": "\u{1F50A}"}));
                }
            }
        }
        inp
    }

    #[test]
    fn padrules_build_state() {
        let v = vectors();
        let mut cases = 0;
        for c in v["state"].as_array().unwrap() {
            cases += 1;
            let name = c["name"].as_str().unwrap();
            let (state, bytes) = build_state(&expand(&c["input"]));
            assert_eq!(bytes, serde_json::to_string(&state).unwrap().len(), "{name}: the byte count is the content's");
            assert!(bytes <= MAX_STATE_BYTES, "{name}: {bytes} bytes");
            if let Some(sum) = c.get("summary") {
                let call = &state["call"];
                let people = call["people"].as_array().unwrap();
                let got = json!({"people": people.len(), "more": call["more"], "recent": call["recent"].as_array().unwrap().len(),
                    "lastU": people.last().map(|p| p["u"].clone()).unwrap_or(json!("")), "bytes": bytes});
                assert_eq!(&got, sum, "state: {name}");
            } else {
                assert_eq!(state, c["expect"], "state: {name}");
                assert_eq!(Some(bytes as u64), c["bytes"].as_u64(), "state: {name}: bytes");
            }
        }
        assert!(cases >= 10);
    }

    #[test]
    fn padrules_accept_state() {
        let v = vectors();
        let d = &v["defaults"];
        for c in v["accept"].as_array().unwrap() {
            let name = c["name"].as_str().unwrap();
            let mut s: PadCtrlSession = serde_json::from_value(merge(&json!({"sid": "PhoneSession0001", "device": d["controllerFacts"]["senderDevice"], "lastRseq": 0}), c.get("session"))).unwrap();
            for (i, st) in c["steps"].as_array().unwrap().iter().enumerate() {
                let before = s.last_rseq;
                let content = match st.get("raw") {
                    Some(raw) => raw.clone(),
                    None => without(merge(&d["state"], st.get("msg")), st.get("drop")),
                };
                let typ = st.get("type").and_then(|x| x.as_str()).unwrap_or(PAD_STATE);
                let r = accept_state(&mut s, typ, &content, &facts(&merge(&d["controllerFacts"], st.get("facts"))));
                assert_eq!(r.err().unwrap_or(""), st["expect"].as_str().unwrap(), "accept: {name}: step {}", i + 1);
                if r.is_ok() {
                    assert_eq!(Some(s.last_rseq), content["rseq"].as_i64(), "{name}: step {}", i + 1);
                } else {
                    assert_eq!(s.last_rseq, before, "{name}: step {}: a refused state moved lastRseq", i + 1);
                }
            }
        }
    }

    #[test]
    fn padrules_targets() {
        let v = vectors();
        let d = &v["defaults"]["targets"];
        for c in v["targets"].as_array().unwrap() {
            let name = c["name"].as_str().unwrap();
            let pick = |k: &str| c.get(k).unwrap_or(&d[k]).clone();
            let trusted: Vec<String> = serde_json::from_value(pick("trusted")).unwrap();
            let events = c["events"].as_array().unwrap();
            let got = pad_targets(pick("me").as_str().unwrap(), pick("myDevice").as_str().unwrap(), pick("now").as_i64().unwrap(), pick("selfVerified").as_bool().unwrap(), &trusted, events);
            let got: Vec<Value> = got.iter().map(|t| json!({"device": t.device, "createdTs": num(t.created_ts), "ready": t.ready, "why": t.why})).collect();
            assert_eq!(Value::from(got), c["expect"], "targets: {name}");
        }
    }

    #[test]
    fn padrules_rate_stale_words() {
        let v = vectors();
        for c in v["rate"].as_array().unwrap() {
            let mut b = RateBucket::default();
            for (i, s) in c["steps"].as_array().unwrap().iter().enumerate() {
                for k in 0..s["n"].as_u64().unwrap() {
                    assert_eq!(b.hit(s["now"].as_i64().unwrap()).as_str(), s["expect"].as_str().unwrap(), "rate: {}: step {}.{}", c["name"], i + 1, k + 1);
                }
            }
        }
        for c in v["stale"].as_array().unwrap() {
            assert_eq!(stale_check(c["offset"].as_i64().unwrap(), c["ts"].as_i64().unwrap(), c["now"].as_i64().unwrap()), c["expect"].as_bool().unwrap(), "stale: {}", c["name"]);
        }
        for c in v["openStale"].as_array().unwrap() {
            assert_eq!(open_stale(c["ts"].as_i64().unwrap(), c["now"].as_i64().unwrap()), c["expect"].as_bool().unwrap(), "openStale: {}", c["name"]);
        }
        for c in v["why"].as_array().unwrap() {
            let op = c.get("op").and_then(|x| x.as_str()).unwrap_or("");
            assert_eq!(why_text(c["code"].as_str().unwrap(), c["device"].as_str().unwrap(), op), c["expect"].as_str().unwrap(), "why: {c}");
        }
    }

    /// What JSON can't carry, and the Rust side of the number rules.
    #[test]
    fn padrules_hand_written() {
        let v = vectors();
        let d = &v["defaults"];
        let (f, cx) = (facts(&d["receiverFacts"]), ctx(&d["ctx"]));
        let vol = |lv: Value| {
            let mut t = PadSessions::default();
            check_pad_message(&mut t, PAD_CMD, &json!({"v": 1, "sid": "PhoneSession0001", "seq": 1, "ts": 1000, "room": "!call:example.org", "op": "open"}), &f, &cx, 1000);
            check_pad_message(&mut t, PAD_CMD, &json!({"v": 1, "sid": "PhoneSession0001", "seq": 2, "ts": 1000, "room": "!call:example.org", "gen": "g1", "op": "vol", "user": "@bob:example.org", "cat": "mic", "level": lv}), &f, &cx, 1000)
        };
        // serde_json can't hold NaN / Infinity: json!(f64::NAN) is null, which is refused like the web's NaN
        assert_eq!(json!(f64::NAN), Value::Null);
        assert_eq!(vol(json!(f64::NAN)).why, "bad");
        assert_eq!(vol(json!(f64::INFINITY)).why, "bad");
        assert!(vol(json!(-0.0)).ok, "-0 is 0");
        // integers: 1.0 is 1; 2^53 is not a safe integer (the web's Number.isSafeInteger)
        assert_eq!(int(Some(&json!(1.0))), Some(1));
        assert_eq!(int(Some(&json!(9_007_199_254_740_991u64))), Some(9_007_199_254_740_991));
        assert_eq!(int(Some(&json!(9_007_199_254_740_992u64))), None);
        assert_eq!(int(Some(&json!(u64::MAX))), None);
        assert_eq!(int(Some(&json!("1"))), None);
        let mut t = PadSessions::default();
        let big = check_pad_message(&mut t, PAD_CMD, &json!({"v": 1, "sid": "PhoneSession0001", "seq": 9_007_199_254_740_992u64, "ts": 1000, "room": "!call:example.org", "op": "open"}), &f, &cx, 1000);
        assert_eq!(big.reason, "bad-envelope");
        // the bucket table never grows past 16 controller devices, and a bucket in use is never forgotten
        let mut t = PadSessions::default();
        let mut refused = 0;
        for i in 0..40 {
            match t.bucket(&format!("DEV{i}"), 0) {
                Some(b) => {
                    b.hit(0);
                }
                None => refused += 1,
            }
        }
        assert_eq!((t.bucket_count(), refused), (MAX_BUCKETS, 24));
        for _ in 0..16 {
            t.bucket("DEV0", 1999).unwrap().hit(1999);
        }
        assert!(t.bucket("DEV99", 2000).is_some());
        assert_eq!(t.bucket_count(), 2);
        assert!(t.bucket("DEV0", 2000).unwrap().tokens < 1.0, "only idle buckets are forgotten");
        // nesting: 32 deep is fine, 33 isn't; scalars don't count
        let mut deep = json!([]);
        for _ in 0..31 {
            deep = json!([deep]);
        }
        assert!(wire_ok(&deep) && !wire_ok(&json!([deep])) && wire_ok(&json!(1)));
        // float parsing is correctly rounded (serde_json "float_roundtrip", as JavaScript's JSON.parse)
        for lit in ["2.0000000000000002220446049250313", "2.0000000000000002220446049250314", "0.1000000000000000055511151231257827", "9007199254740991.0", "1.00000000000000011102230246251565404236316680908203125", "8.988465674311580536566680e307", "0.30000000000000004440892098500626161694526672363281"] {
            let ours: f64 = serde_json::from_str(lit).unwrap();
            assert_eq!(ours.to_bits(), lit.parse::<f64>().unwrap().to_bits(), "{lit}");
        }
        // floats as JavaScript writes them (the byte count depends on it)
        assert_eq!(serde_json::to_string(&num(2.0)).unwrap(), "2");
        assert_eq!(serde_json::to_string(&num(0.07)).unwrap(), "0.07");
        assert_eq!(serde_json::to_string(&num(1.01)).unwrap(), "1.01");
        assert_eq!(serde_json::to_string(&num(round2(0.333333))).unwrap(), "0.33");
        // a validated command round-trips through its JSON
        let ok = vol(json!(0.25));
        assert_eq!(ok.cmd.unwrap().to_json(), json!({"op": "vol", "user": "@bob:example.org", "cat": "mic", "level": 0.25, "live": false}));
        // ids
        assert!(valid_user("@a:b") && !valid_user("@:b") && !valid_user("@a:") && !valid_user("a:b") && !valid_user("@ä:b"));
        assert!(valid_room("!a") && !valid_room("!") && !valid_room("!a b"));
        assert!(!valid_id("__proto__") && !valid_id("prototype") && valid_id("k3f9a1"));
    }

    /// The byte copy equals the web's vectors (`TINHAT_WEB_DIR` = a tinhat-web tree, else `$HOME/tinhat-web`;
    /// skipped when neither has the file).
    #[test]
    fn padrules_vectors_are_the_webs() {
        let dir = std::env::var_os("TINHAT_WEB_DIR").map(std::path::PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join("tinhat-web")));
        let Some(dir) = dir else { return };
        let path = dir.join("src/features/calls/pad/vectors.json");
        match std::fs::read_to_string(&path) {
            Ok(theirs) => {
                assert!(VECTORS == theirs, "testvectors/pad.json differs from {}", path.display());
                eprintln!("pad vectors: byte-identical to {}", path.display());
            }
            Err(_) => eprintln!("SKIPPED: no web vectors at {}", path.display()),
        }
    }
}
