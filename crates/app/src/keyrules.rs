//! keyrules.rs — the pure media-key trust rules, the same rules as Tin Hat Web's
//! `src/features/calls/keyrules.ts` (one product: the same verdict for the same event, the same
//! reason texts; the tests below mirror `keyrules.test.ts` case by case).
//!
//! Intake ([`check_key_event`]) — a peer's `io.element.call.encryption_keys` is installed only when ALL hold:
//!   1. it arrived Olm-encrypted to-device (the SDK's Olm encryption info is present) — never plaintext,
//!      never a room event (the room-event intake is removed);
//!   2. the Olm sender is the event sender, and the SDK bound the Curve25519 sender key to a KNOWN device
//!      of that user (`sender_device`); the payload's claimed device must be that device;
//!   3. `room_id` is the call we are in / joining;
//!   4. the sender is a joined member of that room and (sender, device) holds a live, well-formed call
//!      membership there. A key that is otherwise valid but whose membership has not synced yet is
//!      "pending" (held briefly, re-checked), never installed on trust;
//!   5. it is not under our own identity.
//! Then: at most [`MAX_ENTRIES_PER_EVENT`] key entries per event, indices 0..=254, and at most
//! [`MAX_INDICES_PER_DEVICE`] indices held per device (oldest evicted) — no sender can fill the registry.
//!
//! Without these rules any user able to reach our device over to-device (no room or call in common
//! needed) could fill the peer-key registry with a handful of events and make the native refuse every
//! real peer's key for the rest of the call (found by the rc keys-security gate, 2026-10-24).

use std::collections::HashSet;

/// What the SDK tells us about an Olm-decrypted to-device event (matrix-sdk `EncryptionInfo`).
#[derive(Clone, Debug, PartialEq)]
pub struct OlmInfo {
    pub sender: String,
    /// The device the SDK bound the Olm sender key to (`None` = not bound to a known device).
    pub sender_device: Option<String>,
    /// The Curve25519 key the message was encrypted with (base64). The glue checks it against the
    /// device record (the web's `bindDevice`); the pure rules don't read it.
    pub sender_curve: Option<String>,
}

/// The membership facts of the call room right now.
pub struct IntakeCtx<'a> {
    /// the call room we are in / joining ("" = none)
    pub call_room_id: &'a str,
    pub own_user_id: &'a str,
    pub own_device_id: &'a str,
    /// (user, device) of every live, well-formed call membership in `call_room_id`
    pub live_members: &'a HashSet<(String, String)>,
    /// membership of a user in `call_room_id`: "join" | "leave" | … | None (not loaded / unknown)
    pub membership_of: &'a dyn Fn(&str) -> Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Verdict {
    Ok { sender: String, device: String, room_id: String },
    /// Not installable yet (membership / device binding not synced): hold briefly and re-check.
    Pending(String),
    Reject(String),
}

impl Verdict {
    pub fn is_ok(&self) -> bool {
        matches!(self, Verdict::Ok { .. })
    }
    pub fn is_pending(&self) -> bool {
        matches!(self, Verdict::Pending(_))
    }
}

/// Per-device cap on distinct key indices held (oldest evicted) — no member can exhaust the registry.
pub const MAX_INDICES_PER_DEVICE: usize = 8;
/// Key entries read from one event (the web: `keys.slice(0, 4)`).
pub const MAX_ENTRIES_PER_EVENT: usize = 4;
/// How long a pending key is held for its membership to sync.
pub const PENDING_TTL_MS: u64 = 30_000;
/// At most this many pending keys (oldest dropped).
pub const PENDING_MAX: usize = 32;

/// The device the payload claims (`member.claimed_device_id`, else `device_id`). `content` is the event's
/// content (unwrapped once by the caller, as the web).
pub fn claimed_device(content: &serde_json::Value) -> String {
    let c = content;
    if let Some(d) = c.get("member").and_then(|m| m.get("claimed_device_id")).and_then(|d| d.as_str()) {
        if !d.is_empty() {
            return d.to_string();
        }
    }
    c.get("device_id").and_then(|d| d.as_str()).unwrap_or("").to_string()
}

/// The member.id a key may be installed under (hashed MSC4143 form): a derived form, or the exact
/// `membershipID` of that device's live membership — anything else falls back to `sender:device`, so a
/// member can't mint unlimited identities by varying member.id.
pub fn bound_member_id(member_id_in: Option<&str>, sender: &str, device: &str, live_membership_id: &str) -> String {
    let colon = format!("{sender}:{device}");
    let m = member_id_in.unwrap_or("");
    if !m.is_empty() && (m == colon || m == format!("_{sender}_{device}") || (!live_membership_id.is_empty() && m == live_membership_id)) {
        return m.to_string();
    }
    colon
}

pub fn check_key_event(enc: Option<&OlmInfo>, event_sender: &str, content: &serde_json::Value, ctx: &IntakeCtx) -> Verdict {
    let reject = |r: String| Verdict::Reject(r);
    let Some(enc) = enc else {
        return reject("plaintext key event — only Olm-encrypted to-device keys are trusted".into());
    };
    let sender = enc.sender.as_str();
    if sender.is_empty() || (!event_sender.is_empty() && event_sender != sender) {
        return reject(format!(
            "Olm sender {} ≠ event sender {}",
            if sender.is_empty() { "?" } else { sender },
            if event_sender.is_empty() { "?" } else { event_sender }
        ));
    }
    let device = enc.sender_device.as_deref().unwrap_or("");
    let claimed = claimed_device(content);
    if claimed.is_empty() {
        return reject("no claimed device id".into());
    }
    if device.is_empty() {
        return Verdict::Pending(format!("the sending device of {sender} is not bound to a known device yet (claims {claimed})"));
    }
    if claimed != device {
        return reject(format!("claimed device {claimed} ≠ Olm-authenticated device {device}"));
    }
    let room_id = content.get("room_id").and_then(|r| r.as_str()).unwrap_or("");
    if room_id.is_empty() {
        return reject("no room_id".into());
    }
    if ctx.call_room_id.is_empty() || room_id != ctx.call_room_id {
        return reject(format!(
            "key for {room_id}, not our call ({})",
            if ctx.call_room_id.is_empty() { "none" } else { ctx.call_room_id }
        ));
    }
    if sender == ctx.own_user_id && device == ctx.own_device_id {
        return reject("a key from our own device".into());
    }
    match (ctx.membership_of)(sender) {
        None => return Verdict::Pending(format!("room membership of {sender} not loaded yet")),
        Some(ms) if ms != "join" => return reject(format!("{sender} is not a joined member of the call room ({ms})")),
        Some(_) => {}
    }
    if !ctx.live_members.contains(&(sender.to_string(), device.to_string())) {
        return Verdict::Pending(format!("{sender}/{device} holds no live call membership (yet)"));
    }
    Verdict::Ok { sender: sender.to_string(), device: device.to_string(), room_id: room_id.to_string() }
}

/// Is `state_key` a membership state key owned by (sender, device)? (session form, EC legacy, array form)
pub fn state_key_owned(state_key: &str, sender: &str, device: &str, array_form: bool) -> bool {
    if sender.is_empty() {
        return false;
    }
    if array_form {
        return state_key == sender; // legacy memberships[] live under the user's own key
    }
    if device.is_empty() {
        return false;
    }
    state_key == format!("_{sender}_{device}_m.call")
        || state_key == format!("_{sender}_{device}")
        || state_key == format!("{sender}_{device}")
        || state_key == format!("{sender}_{device}_m.call")
}

/// Valid own/peer key index: an integer 0..=254 (255 is refused: one past the keyring). A JSON number with
/// an integral value (7, 7.0, -0 — as JavaScript's Number.isInteger) or a numeric string ("7").
pub fn valid_index(v: &serde_json::Value) -> Option<u8> {
    let n: Option<u64> = if let Some(u) = v.as_u64() {
        Some(u)
    } else if let Some(f) = v.as_f64() {
        if f.is_finite() && f.fract() == 0.0 && (0.0..=254.0).contains(&f) { Some(f as u64) } else { None }
    } else if let Some(s) = v.as_str() {
        let t = s.trim();
        if !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()) {
            t.parse::<u64>().ok()
        } else {
            None
        }
    } else {
        None
    };
    n.filter(|&n| n <= 254).map(|n| n as u8)
}

/// One live MatrixRTC session of a room (the web's `LiveSession`, the fields the key rules use).
#[derive(Clone, Debug, PartialEq)]
pub struct LiveSession {
    pub sender: String,
    pub device_id: String,
    pub state_key: String,
    /// `membershipID` of the session ("" = none) — the only non-derived member.id a key may carry
    pub membership_id: String,
    /// strict schema (application m.call, scope m.room, call_id "", finite expires > 0): required for keys
    pub well_formed: bool,
}

/// Default membership lifetime when `expires` is absent (MSC4143 / the web's MEMBERSHIP_EXPIRES_MS).
pub const MEMBERSHIP_EXPIRES_MS: u64 = 14_400_000;

fn entry_live(c: &serde_json::Map<String, serde_json::Value>, origin_ts: u64, now: u64) -> bool {
    if c.is_empty() {
        return false;
    }
    let num = |k: &str| c.get(k).and_then(|v| v.as_f64()).filter(|f| f.is_finite());
    let created = num("created_ts").or(if origin_ts > 0 { Some(origin_ts as f64) } else { None });
    let Some(created) = created else { return true };
    let expires = num("expires").unwrap_or(MEMBERSHIP_EXPIRES_MS as f64);
    (now as f64) <= created + expires
}

/// Live msc3401 sessions among raw `org.matrix.msc3401.call.member` state events (`{sender, state_key,
/// origin_server_ts, content}`; typed session format + legacy array), one per (user, device) — the web's
/// `liveSessions`: a device id is required and the state key must belong to the sender and that device.
pub fn live_sessions_from(events: &[serde_json::Value], now: u64) -> Vec<LiveSession> {
    let mut out: Vec<LiveSession> = Vec::new();
    let mut seen: HashSet<(String, String)> = HashSet::new();
    for ev in events {
        let sender = ev.get("sender").and_then(|s| s.as_str()).unwrap_or("");
        let Some(c) = ev.get("content").and_then(|c| c.as_object()) else { continue };
        let origin = ev.get("origin_server_ts").and_then(|t| t.as_u64()).unwrap_or(0);
        if sender.is_empty() || c.is_empty() {
            continue;
        }
        let array = c.get("memberships").and_then(|m| m.as_array());
        let entries: Vec<&serde_json::Map<String, serde_json::Value>> = match array {
            Some(a) => a.iter().filter_map(|m| m.as_object()).filter(|m| entry_live(m, origin, now)).collect(),
            None => {
                if entry_live(c, origin, now) {
                    vec![c]
                } else {
                    vec![]
                }
            }
        };
        let state_key = ev.get("state_key").and_then(|s| s.as_str()).unwrap_or("");
        for e in entries {
            let s = |k: &str| e.get(k).and_then(|v| v.as_str());
            if e.contains_key("application") && s("application") != Some("m.call") {
                continue;
            }
            if e.contains_key("scope") && s("scope") != Some("m.room") {
                continue;
            }
            let dev = s("device_id").unwrap_or("");
            if dev.is_empty() || !state_key_owned(state_key, sender, dev, array.is_some()) {
                continue;
            }
            if !seen.insert((sender.to_string(), dev.to_string())) {
                continue;
            }
            let expires = e.get("expires").and_then(|v| v.as_f64()).filter(|f| f.is_finite()).unwrap_or(0.0);
            out.push(LiveSession {
                sender: sender.to_string(),
                device_id: dev.to_string(),
                state_key: state_key.to_string(),
                membership_id: s("membershipID").unwrap_or("").to_string(),
                well_formed: s("application") == Some("m.call") && s("scope") == Some("m.room") && s("call_id") == Some("") && expires > 0.0,
            });
        }
    }
    out
}

/// The (user, device) pairs whose keys may be installed: live AND well-formed sessions.
pub fn live_key_members(sessions: &[LiveSession]) -> HashSet<(String, String)> {
    sessions.iter().filter(|s| s.well_formed).map(|s| (s.sender.clone(), s.device_id.clone())).collect()
}

/// The key entries of one event (`content` = the event's content): the first [`MAX_ENTRIES_PER_EVENT`] objects
/// or arrays of a `keys` array (as the web: `filter(x && typeof x === "object").slice(0, 4)` — an array entry
/// counts and then carries no usable index), or the one `keys` object.
pub fn key_entries(content: &serde_json::Value) -> Vec<&serde_json::Value> {
    match content.get("keys") {
        Some(k) if k.is_array() => k.as_array().unwrap().iter().filter(|e| e.is_object() || e.is_array()).take(MAX_ENTRIES_PER_EVENT).collect(),
        Some(k) if k.is_object() => vec![k],
        _ => vec![],
    }
}

/// The per-device index book-keeping: record `index` for `owner` and return the indices to evict
/// (oldest first) so at most [`MAX_INDICES_PER_DEVICE`] stay held. A re-sent index keeps its place (the web's
/// `installSlot`: insertion order, no recency refresh).
pub fn note_index(held: &mut Vec<u8>, index: u8) -> Vec<u8> {
    if held.contains(&index) {
        return Vec::new();
    }
    held.push(index);
    let mut out = Vec::new();
    while held.len() > MAX_INDICES_PER_DEVICE {
        out.push(held.remove(0));
    }
    out
}

#[cfg(test)]
mod tests {
    // Mirrors Tin Hat Web src/features/calls/keyrules.test.ts (same cases, same order), plus the native's
    // liveness/entries/eviction helpers.
    use super::*;
    use serde_json::json;

    const ROOM: &str = "!call:example.com";
    const ALICE: &str = "@alice:example.com";
    const MALLORY: &str = "@mallory:example.com";
    const ME: &str = "@me:example.com";

    fn membership(u: &str) -> Option<String> {
        if [ALICE, ME, MALLORY].contains(&u) {
            Some("join".into())
        } else if u == "@kicked:example.com" {
            Some("leave".into())
        } else {
            None
        }
    }
    fn live_default() -> HashSet<(String, String)> {
        [(ALICE.to_string(), "ADEV1".to_string()), (ME.to_string(), "MEDEV".to_string())].into_iter().collect()
    }
    fn check(enc: Option<OlmInfo>, ev_sender: &str, content: serde_json::Value, call_room: &str, live: &HashSet<(String, String)>) -> Verdict {
        let ctx = IntakeCtx { call_room_id: call_room, own_user_id: ME, own_device_id: "MEDEV", live_members: live, membership_of: &membership };
        check_key_event(enc.as_ref(), ev_sender, &content, &ctx)
    }
    fn content(dev: &str) -> serde_json::Value {
        content_in(dev, ROOM, json!(3))
    }
    fn content_in(dev: &str, room: &str, idx: serde_json::Value) -> serde_json::Value {
        json!({ "keys": { "index": idx, "key": "AAAAAAAAAAAAAAAAAAAAAA==" }, "room_id": room,
                "member": { "claimed_device_id": dev, "id": format!("{ALICE}:{dev}") } })
    }
    fn enc(sender: &str, dev: Option<&str>) -> Option<OlmInfo> {
        Some(OlmInfo { sender: sender.into(), sender_device: dev.map(str::to_string), sender_curve: Some("curve".into()) })
    }

    #[test]
    fn plaintext_rejected() {
        assert!(!check(None, ALICE, content("ADEV1"), ROOM, &live_default()).is_ok());
    }
    #[test]
    fn valid_member_device_accepted() {
        assert!(check(enc(ALICE, Some("ADEV1")), ALICE, content("ADEV1"), ROOM, &live_default()).is_ok());
    }
    #[test]
    fn other_device_claiming_adev1_rejected_not_pending() {
        let v = check(enc(ALICE, Some("ADEV2")), ALICE, content("ADEV1"), ROOM, &live_default());
        assert!(!v.is_ok() && !v.is_pending(), "{v:?}");
        assert_eq!(v, Verdict::Reject("claimed device ADEV1 ≠ Olm-authenticated device ADEV2".into()));
    }
    #[test]
    fn second_device_without_membership_held() {
        let v = check(enc(ALICE, Some("ADEV2")), ALICE, content("ADEV2"), ROOM, &live_default());
        assert!(!v.is_ok() && v.is_pending(), "{v:?}");
    }
    #[test]
    fn unbound_sender_device_not_installed() {
        let v = check(enc(ALICE, None), ALICE, content("ADEV1"), ROOM, &live_default());
        assert!(!v.is_ok() && v.is_pending(), "{v:?}");
    }
    #[test]
    fn olm_sender_not_event_sender_rejected() {
        assert!(!check(enc(MALLORY, Some("M1")), ALICE, content("M1"), ROOM, &live_default()).is_ok());
    }
    #[test]
    fn key_for_another_room_rejected() {
        assert!(!check(enc(ALICE, Some("ADEV1")), ALICE, content_in("ADEV1", "!other:x", json!(3)), ROOM, &live_default()).is_ok());
    }
    #[test]
    fn no_call_rejected() {
        let v = check(enc(ALICE, Some("ADEV1")), ALICE, content("ADEV1"), "", &live_default());
        assert!(!v.is_ok() && !v.is_pending(), "{v:?}");
    }
    #[test]
    fn joined_member_outside_call_never_installed() {
        assert!(!check(enc(MALLORY, Some("M1")), MALLORY, content("M1"), ROOM, &live_default()).is_ok());
    }
    #[test]
    fn departed_user_with_leftover_membership_rejected() {
        let live: HashSet<(String, String)> = [("@kicked:example.com".to_string(), "K1".to_string())].into_iter().collect();
        let v = check(enc("@kicked:example.com", Some("K1")), "@kicked:example.com", content("K1"), ROOM, &live);
        assert!(!v.is_ok() && !v.is_pending(), "{v:?}");
    }
    #[test]
    fn federated_non_member_not_installed() {
        assert!(!check(enc("@fed:other.org", Some("F1")), "@fed:other.org", content("F1"), ROOM, &live_default()).is_ok());
    }
    #[test]
    fn own_device_rejected() {
        assert!(!check(enc(ME, Some("MEDEV")), ME, content("MEDEV"), ROOM, &live_default()).is_ok());
    }
    #[test]
    fn claimed_device_prefers_member_claimed_device_id() {
        assert_eq!(claimed_device(&json!({ "member": { "claimed_device_id": "X" }, "device_id": "Y" })), "X");
        assert_eq!(claimed_device(&json!({ "device_id": "Y" })), "Y");
        // the content is read as given (unwrapped once by the caller): a nested "content" is just another field
        assert_eq!(claimed_device(&json!({ "content": { "member": { "claimed_device_id": "X" } } })), "");
    }
    #[test]
    fn no_claimed_device_rejected() {
        assert!(!check(enc(ALICE, Some("ADEV1")), ALICE, json!({ "keys": {}, "room_id": ROOM }), ROOM, &live_default()).is_ok());
    }
    #[test]
    fn indices() {
        assert_eq!(valid_index(&json!(255)), None, "255 refused");
        assert_eq!(valid_index(&json!(254)), Some(254));
        assert_eq!(valid_index(&json!("7")), Some(7));
        assert_eq!(valid_index(&json!(-1)), None);
        assert_eq!(valid_index(&json!(1.5)), None);
        assert_eq!(valid_index(&json!(" 9 ")), Some(9));
        assert_eq!(valid_index(&json!("x1")), None);
        assert_eq!(valid_index(&json!(7.0)), Some(7), "an integral float, as Number.isInteger");
        assert_eq!(valid_index(&json!(-0.0)), Some(0));
        assert_eq!(valid_index(&json!(254.0)), Some(254));
        assert_eq!(valid_index(&json!(255.0)), None);
    }
    #[test]
    fn membership_ownership() {
        assert!(state_key_owned(&format!("_{ALICE}_ADEV1_m.call"), ALICE, "ADEV1", false), "session state key owned");
        assert!(!state_key_owned(&format!("_{MALLORY}_ADEV1_m.call"), ALICE, "ADEV1", false), "another user's key");
        assert!(!state_key_owned(&format!("_{ALICE}_ADEV2_m.call"), ALICE, "ADEV1", false), "another device's key");
        assert!(state_key_owned(ALICE, ALICE, "ADEV1", true) && !state_key_owned(MALLORY, ALICE, "ADEV1", true), "legacy array form");
    }
    #[test]
    fn member_id_binding() {
        assert_eq!(bound_member_id(Some(&format!("{ALICE}:ADEV1")), ALICE, "ADEV1", ""), format!("{ALICE}:ADEV1"));
        assert_eq!(bound_member_id(Some("uuid-123"), ALICE, "ADEV1", "uuid-123"), "uuid-123");
        assert_eq!(bound_member_id(Some("spam-42"), ALICE, "ADEV1", "uuid-123"), format!("{ALICE}:ADEV1"));
        assert_eq!(bound_member_id(Some(&format!("{MALLORY}:M1")), ALICE, "ADEV1", ""), format!("{ALICE}:ADEV1"));
        assert_eq!(bound_member_id(None, ALICE, "ADEV1", "uuid-123"), format!("{ALICE}:ADEV1"));
    }

    // ── native helpers ──
    fn mem(sender: &str, sk: &str, c: serde_json::Value) -> serde_json::Value {
        json!({ "sender": sender, "state_key": sk, "origin_server_ts": 1_000u64, "content": c })
    }
    fn good(dev: &str) -> serde_json::Value {
        json!({ "application": "m.call", "scope": "m.room", "call_id": "", "device_id": dev, "expires": 14_400_000u64,
                "created_ts": 1_000u64, "membershipID": format!("mid-{dev}") })
    }
    #[test]
    fn live_sessions_schema_ownership_liveness() {
        let now = 2_000u64;
        let evs = vec![
            mem(ALICE, &format!("_{ALICE}_ADEV1_m.call"), good("ADEV1")),
            mem(MALLORY, &format!("_{ALICE}_ADEV9_m.call"), good("ADEV9")), // state key of another user
            mem(ALICE, &format!("_{ALICE}_ADEV2_m.call"), good("OTHER")),   // state key of another device
            mem(ALICE, &format!("_{ALICE}_ADEV3_m.call"), json!({})),       // tombstone
            mem(ALICE, &format!("_{ALICE}_ADEV4_m.call"), json!({ "application": "m.call", "scope": "m.room", "call_id": "", "device_id": "ADEV4", "expires": 10u64, "created_ts": 1_000u64 })), // expired
            mem(ALICE, &format!("_{ALICE}_ADEV5_m.call"), json!({ "application": "m.call", "device_id": "ADEV5", "created_ts": 1_000u64 })), // live, not well-formed
            mem(ALICE, &format!("_{ALICE}_ADEV6_m.call"), json!({ "application": "m.other", "scope": "m.room", "call_id": "", "device_id": "ADEV6", "expires": 100_000u64 })),
            mem(ME, ME, json!({ "memberships": [good("MEDEV"), { "device_id": "OLD", "expires": 1u64, "created_ts": 1u64 }] })), // legacy array
        ];
        let s = live_sessions_from(&evs, now);
        let ids: Vec<(String, bool)> = s.iter().map(|x| (format!("{}|{}", x.sender, x.device_id), x.well_formed)).collect();
        assert_eq!(
            ids,
            vec![(format!("{ALICE}|ADEV1"), true), (format!("{ALICE}|ADEV5"), false), (format!("{ME}|MEDEV"), true)]
        );
        assert_eq!(s[0].membership_id, "mid-ADEV1");
        let k = live_key_members(&s);
        assert!(k.contains(&(ALICE.to_string(), "ADEV1".to_string())) && !k.contains(&(ALICE.to_string(), "ADEV5".to_string())));
    }
    #[test]
    fn entries_capped_per_event() {
        let many: Vec<serde_json::Value> = (0..255).map(|i| json!({ "index": i, "key": "AAAAAAAAAAAAAAAAAAAAAA==" })).collect();
        assert_eq!(key_entries(&json!({ "keys": many })).len(), MAX_ENTRIES_PER_EVENT);
        assert_eq!(key_entries(&json!({ "keys": { "index": 1, "key": "x" } })).len(), 1);
        assert_eq!(key_entries(&json!({ "keys": "nope" })).len(), 0);
        // arrays count toward the 4 (as the web), then carry no index: the 5th, valid entry is never read
        let k = json!({ "keys": [[], [], [], [], { "index": 1, "key": "x" }] });
        let e = key_entries(&k);
        assert_eq!(e.len(), 4);
        assert!(e.iter().all(|x| x.get("index").is_none()));
    }
    #[test]
    fn per_device_index_cap_evicts_oldest() {
        let mut held = Vec::new();
        for i in 0..MAX_INDICES_PER_DEVICE as u8 {
            assert!(note_index(&mut held, i).is_empty());
        }
        assert!(note_index(&mut held, 0).is_empty(), "a re-sent index is not new");
        assert_eq!(note_index(&mut held, 100), vec![0u8], "…and keeps its place: the oldest goes (the web's policy)");
        assert_eq!(held.len(), MAX_INDICES_PER_DEVICE);
        assert_eq!(note_index(&mut held, 101), vec![1u8]);
    }
}
