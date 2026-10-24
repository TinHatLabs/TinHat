//! The `tinhat.rxq` LiveKit data message — port of the web's `src/widgets/rxq.ts` (contract WIDGETS.md §6; vectors
//! `testvectors/widgets/rxq/*`): every 5 s in a call each app tells EACH remote participant (targeted, unreliable) how
//! it receives THAT participant's tracks. The receiver shows it as that person's outgoing lane ("how they receive
//! you"). Display only — never trusted for anything else. Pure: the codec, the validation and the per-sender rate
//! limit; the call engine (streamer `callstats.rs`) does the I/O.
//!
//! ```text
//! {"v":1,"q":"good","t":[{"k":"a","kbps":48,"loss":2,"jit":12,"rtt":40},{"k":"v","kbps":1450,"loss":0,"jit":8,
//!  "w":1280,"h":720,"fps":30,"c":"AV1"}]}
//! ```
//!
//! Older apps ignore the topic; `tinhat.rx` (hear.rs / hear.ts) is unchanged.

use std::collections::HashMap;

use serde_json::{Map, Value};

use crate::quality::{clamp_numbers, worst_band, Band, NumKey, TrackNumbers, NUM_KEYS};
use crate::{js_number, num};

pub const RXQ_TOPIC: &str = "tinhat.rxq";
pub const RXQ_VERSION: u32 = 1;
pub const RXQ_MAX_BYTES: usize = 600;
pub const RXQ_MAX_ENTRIES: usize = 8;
/// The sender's period.
pub const RXQ_SEND_MS: u64 = 5000;
/// The receiver accepts at most one message per sender per 2 s.
pub const RXQ_MIN_GAP_MS: f64 = 2000.0;
/// A lane with no message for longer than this shows "—" (stale / an older app that doesn't send).
pub const RXQ_STALE_MS: f64 = 12_000.0;
/// Messages a sender may make us READ per 2 s window (valid or not) — bounds the work a flood can cause.
pub const RXQ_MAX_ATTEMPTS: u32 = 4;
/// The strict reader's nesting limit for rxq texts (the message itself needs 3 levels).
pub const RXQ_MAX_DEPTH: usize = 16;

/// A decoded message.
#[derive(Clone, Debug, PartialEq)]
pub struct Rxq {
    pub q: Band,
    pub t: Vec<TrackNumbers>,
}

/// Why a message was refused whole.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RxqError {
    TooLarge,
    NotJson,
    NotObject,
    BadV,
    BadT,
    TooMany,
}

impl RxqError {
    pub fn as_str(self) -> &'static str {
        match self {
            RxqError::TooLarge => "too-large",
            RxqError::NotJson => "not-json",
            RxqError::NotObject => "not-object",
            RxqError::BadV => "bad-v",
            RxqError::BadT => "bad-t",
            RxqError::TooMany => "too-many",
        }
    }
}

/// The reference's `RxqDecoded` as JSON: `{ok: true, q, t}` / `{ok: false, error}` (the vectors' shape).
pub fn decoded_json(d: &Result<Rxq, RxqError>) -> Value {
    let mut o = Map::new();
    match d {
        Ok(x) => {
            o.insert("ok".into(), Value::Bool(true));
            o.insert("q".into(), Value::String(x.q.as_str().into()));
            o.insert("t".into(), Value::Array(x.t.iter().map(TrackNumbers::to_json).collect()));
        }
        Err(e) => {
            o.insert("ok".into(), Value::Bool(false));
            o.insert("error".into(), Value::String(e.as_str().into()));
        }
    }
    Value::Object(o)
}

/// KEY_ORDER: k, kbps, loss, jit, rtt, w, h, fps, c — only the fields that are set, no spaces.
fn entry_json(e: &TrackNumbers) -> String {
    let mut parts: Vec<String> = vec![format!("\"k\":\"{}\"", e.k.as_str())];
    for key in NUM_KEYS {
        let v = match key {
            NumKey::Kbps => e.kbps,
            NumKey::Loss => e.loss,
            NumKey::Jit => e.jit,
            NumKey::Rtt => e.rtt,
            NumKey::W => e.w,
            NumKey::H => e.h,
            NumKey::Fps => e.fps,
        };
        if let Some(v) = v {
            parts.push(format!("\"{}\":{}", key.name(), js_number(v)));
        }
    }
    if let Some(c) = &e.c {
        let mut s = String::from("\"c\":");
        tinhat_theme::canon::push_js_string(&mut s, c);
        parts.push(s);
    }
    format!("{{{}}}", parts.join(","))
}

/// The message a sender writes for one remote participant: its entries clamped, one per kind (a, v, s — the first of
/// each), in kind order; q = the worst band of the entries ("ok" when none can be banded). Keys in the fixed order
/// v, q, t and k, kbps, loss, jit, rtt, w, h, fps, c; no spaces. Entries are dropped from the end while the text is
/// over 600 bytes. Returns "" when there is nothing to report (no entries): send nothing then.
pub fn encode_rxq(entries: &[TrackNumbers]) -> String {
    let mut t: Vec<TrackNumbers> = Vec::new();
    for e in entries {
        if !t.iter().any(|x| x.k == e.k) {
            t.push(clamp_numbers(e));
        }
    }
    t.sort_by_key(|x| x.k.rank());
    while !t.is_empty() {
        let q = worst_band(t.iter().map(TrackNumbers::band)).unwrap_or(Band::Ok);
        let text = format!(
            "{{\"v\":{},\"q\":\"{}\",\"t\":[{}]}}",
            RXQ_VERSION,
            q.as_str(),
            t.iter().map(entry_json).collect::<Vec<_>>().join(",")
        );
        if text.len() <= RXQ_MAX_BYTES {
            return text;
        }
        t.pop();
    }
    String::new()
}

/// Read a received message: the payload's UTF-8 text (the app decodes the bytes strictly — malformed UTF-8 is refused
/// before this, see [`decode_rxq_bytes`]; a BOM stays in the text and is refused here). Refused whole: over 600 bytes
/// (too-large); not strict JSON by `json::parse_strict_json` (not-json: other whitespace, a duplicate key, an escaped
/// lone surrogate, a number that is not a finite double, nesting over 16); not an object (not-object); `v` not a number
/// whose value is an integer ≥ 1 (bad-v); `t` not an array (bad-t); more than 8 entries (too-many). Then per entry: not
/// an object or `k` not a/v/s → skipped; a second entry of a kind → skipped; fields that are not JSON numbers →
/// dropped, the rest r() + clamped; `c` kept only when it is a valid label. `q` = the given one when good/ok/poor, else
/// the worst band of the kept entries, else "ok". Unknown keys are ignored (a newer version may add some).
pub fn decode_rxq(text: &str) -> Result<Rxq, RxqError> {
    if text.len() > RXQ_MAX_BYTES {
        return Err(RxqError::TooLarge);
    }
    let v = crate::json::parse_strict_json(text, RXQ_MAX_DEPTH).ok_or(RxqError::NotJson)?;
    let o = v.as_object().ok_or(RxqError::NotObject)?;
    match o.get("v").and_then(Value::as_f64) {
        Some(x) if x.is_finite() && x.fract() == 0.0 && x >= 1.0 => {}
        _ => return Err(RxqError::BadV),
    }
    let tv = o.get("t").and_then(Value::as_array).ok_or(RxqError::BadT)?;
    if tv.len() > RXQ_MAX_ENTRIES {
        return Err(RxqError::TooMany);
    }
    let mut t: Vec<TrackNumbers> = Vec::new();
    for e in tv {
        let Some(n) = TrackNumbers::from_json(e) else { continue };
        if t.iter().any(|x| x.k == n.k) {
            continue;
        }
        t.push(clamp_numbers(&n));
    }
    let q = match o.get("q").and_then(Value::as_str).and_then(Band::parse) {
        Some(b) => b,
        None => worst_band(t.iter().map(TrackNumbers::band)).unwrap_or(Band::Ok),
    };
    Ok(Rxq { q, t })
}

/// [`decode_rxq`] of a payload's bytes: over 600 bytes → too-large; malformed UTF-8 → not-json (strict decoding).
pub fn decode_rxq_bytes(payload: &[u8]) -> Result<Rxq, RxqError> {
    if payload.len() > RXQ_MAX_BYTES {
        return Err(RxqError::TooLarge);
    }
    decode_rxq(std::str::from_utf8(payload).map_err(|_| RxqError::NotJson)?)
}

/// The receiver's per-sender limits, checked BEFORE a message is read (times from a monotonic clock, ms; one gate per
/// call, cleared when the call ends):
/// - `read(sender, now)`: false → drop unread: a message of this sender was accepted less than 2000 ms ago, or it
///   already made us read RXQ_MAX_ATTEMPTS messages in the current attempt window (a window opens at the first read and
///   lasts 2000 ms); a non-finite `now` → false. true → read it (counts as an attempt).
/// - `accepted(sender, now)`: it was valid and is shown: no other message of this sender is read for 2000 ms.
#[derive(Default, Debug, Clone)]
pub struct RxqGate {
    accepted_at: HashMap<String, f64>,
    window: HashMap<String, (f64, u32)>,
}

impl RxqGate {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn read(&mut self, sender: &str, now: f64) -> bool {
        if !now.is_finite() {
            return false;
        }
        if let Some(acc) = self.accepted_at.get(sender) {
            if now - acc < RXQ_MIN_GAP_MS {
                return false;
            }
        }
        let w = self.window.entry(sender.to_string()).or_insert((now, 0));
        // a new window at the first read, or once the current one is 2000 ms old (the entry just made has start = now)
        if now - w.0 >= RXQ_MIN_GAP_MS {
            *w = (now, 0);
        }
        if w.1 >= RXQ_MAX_ATTEMPTS {
            return false;
        }
        w.1 += 1;
        true
    }
    pub fn accepted(&mut self, sender: &str, now: f64) {
        if now.is_finite() {
            self.accepted_at.insert(sender.to_string(), now);
        }
    }
    /// A participant left: forget them.
    pub fn forget(&mut self, sender: &str) {
        self.accepted_at.remove(sender);
        self.window.remove(sender);
    }
    pub fn clear(&mut self) {
        self.accepted_at.clear();
        self.window.clear();
    }
}

/// What the app does with one received message (the gate + the reader): skip (not read), refuse (read, malformed),
/// accept (read, valid: show it).
#[derive(Clone, Debug, PartialEq)]
pub enum Verdict {
    Skip,
    Refuse,
    Accept(Rxq),
}

impl Verdict {
    pub fn as_str(&self) -> &'static str {
        match self {
            Verdict::Skip => "skip",
            Verdict::Refuse => "refuse",
            Verdict::Accept(_) => "accept",
        }
    }
}

/// `receiveRxq(gate, sender, now, text)`.
pub fn receive_rxq(gate: &mut RxqGate, sender: &str, now: f64, text: &str) -> Verdict {
    if !gate.read(sender, now) {
        return Verdict::Skip;
    }
    match decode_rxq(text) {
        Err(_) => Verdict::Refuse,
        Ok(m) => {
            gate.accepted(sender, now);
            Verdict::Accept(m)
        }
    }
}

/// [`receive_rxq`] of a payload's bytes (strict UTF-8 after the gate — malformed = refuse).
pub fn receive_rxq_bytes(gate: &mut RxqGate, sender: &str, now: f64, payload: &[u8]) -> Verdict {
    if !gate.read(sender, now) {
        return Verdict::Skip;
    }
    match decode_rxq_bytes(payload) {
        Err(_) => Verdict::Refuse,
        Ok(m) => {
            gate.accepted(sender, now);
            Verdict::Accept(m)
        }
    }
}

/// A lane that has had no accepted message for longer than RXQ_STALE_MS (monotonic ms).
pub fn rxq_stale(received_at: Option<f64>, now: f64) -> bool {
    match received_at {
        None => true,
        Some(at) => !(now - at <= RXQ_STALE_MS),
    }
}

/// The track list as JSON (the reference's `t`).
pub fn tracks_json(t: &[TrackNumbers]) -> Value {
    Value::Array(t.iter().map(TrackNumbers::to_json).collect())
}

/// A number as the reference holds it (re-exported for callers building snapshots).
pub fn json_num(x: f64) -> Value {
    num(x)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quality::Kind;

    const EXAMPLE: &str = r#"{"v":1,"q":"good","t":[{"k":"a","kbps":48,"loss":2,"jit":12,"rtt":40},{"k":"v","kbps":1450,"loss":0,"jit":8,"w":1280,"h":720,"fps":30,"c":"AV1"}]}"#;

    #[test]
    fn the_contract_example_round_trips() {
        let d = decode_rxq(EXAMPLE).unwrap();
        assert_eq!(d.q, Band::Good);
        assert_eq!(d.t.len(), 2);
        assert_eq!(d.t[1].c.as_deref(), Some("AV1"));
        assert_eq!(encode_rxq(&d.t), EXAMPLE);
    }

    #[test]
    fn out_of_range_numbers_are_not_json() {
        assert_eq!(decode_rxq(r#"{"v":1,"t":[{"k":"a","kbps":1e999}]}"#), Err(RxqError::NotJson));
        assert_eq!(decode_rxq(r#"{"v":1,"t":[{"k":"a","c":"1e999"}]}"#).map(|x| x.t.len()), Ok(1));
        assert_eq!(decode_rxq_bytes(b"{\"v\":1,\"t\":[{\"k\":\"a\",\"c\":\"\xed\xa0\x80\"}]}"), Err(RxqError::NotJson));
    }

    #[test]
    fn decode_of_encode_round_trips_for_random_entries() {
        // the web's property test (widgets.test.ts), same generator
        let mut a: f64 = 1234567.0;
        let mut rnd = || {
            a = (a * 1103515245.0 + 12345.0) % 2147483648.0;
            a / 2147483648.0
        };
        for _ in 0..2000 {
            let mut entries = Vec::new();
            for k in [Kind::A, Kind::V, Kind::S] {
                if rnd() < 0.7 {
                    let mut e = TrackNumbers::new(k);
                    e.kbps = Some(rnd() * 9000.0);
                    e.loss = Some(rnd() * 120.0);
                    e.jit = Some(rnd() * 100.0);
                    e.rtt = Some(rnd() * 500.0);
                    if k != Kind::A {
                        e.w = Some(1280.0);
                        e.h = Some(720.0);
                        e.fps = Some(30.0);
                        e.c = Some("VP8".into());
                    } else {
                        e.c = Some("Opus".into());
                    }
                    entries.push(e);
                }
            }
            let text = encode_rxq(&entries);
            if entries.is_empty() {
                assert_eq!(text, "");
                continue;
            }
            let d = decode_rxq(&text).unwrap();
            assert_eq!(encode_rxq(&d.t), text);
        }
    }
}
