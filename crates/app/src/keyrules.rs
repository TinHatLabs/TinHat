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
//! Announce ([`key_recipients`]) — our key goes only to live, well-formed call-member devices whose user is a
//! JOINED room member, never to ourselves; the caller re-runs the check right before sending (12.0.5: the
//! desktop used to key every live membership, joined or not).
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
    /// `created_ts` of the session, else the event's `origin_server_ts` (the web's `createdTs`)
    pub created_ts: u64,
    /// the session's first livekit focus `(livekit_service_url, livekit_alias)` from `foci_preferred`, else
    /// `foci_active` (the web's `focusOf`); the alias is "" when absent (the caller uses the room id)
    pub focus: Option<(String, String)>,
}

/// The web's `focusOf`: the first `{type: livekit, livekit_service_url: <non-empty>}` of `foci_preferred` (else
/// `foci_active`).
fn focus_of(e: &serde_json::Map<String, serde_json::Value>) -> Option<(String, String)> {
    let list = e.get("foci_preferred").and_then(|v| v.as_array()).or_else(|| e.get("foci_active").and_then(|v| v.as_array()))?;
    list.iter().find_map(|f| {
        let s = |k: &str| f.get(k).and_then(|v| v.as_str()).unwrap_or("");
        (s("type") == "livekit" && !s("livekit_service_url").is_empty()).then(|| (s("livekit_service_url").to_string(), s("livekit_alias").to_string()))
    })
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
            let created_ts = e.get("created_ts").and_then(|v| v.as_f64()).filter(|f| f.is_finite() && *f >= 0.0).map(|f| f as u64).unwrap_or(origin);
            out.push(LiveSession {
                sender: sender.to_string(),
                device_id: dev.to_string(),
                state_key: state_key.to_string(),
                membership_id: s("membershipID").unwrap_or("").to_string(),
                well_formed: s("application") == Some("m.call") && s("scope") == Some("m.room") && s("call_id") == Some("") && expires > 0.0,
                created_ts,
                focus: focus_of(e),
            });
        }
    }
    out
}

/// The (user, device) pairs whose keys may be installed: live AND well-formed sessions.
pub fn live_key_members(sessions: &[LiveSession]) -> HashSet<(String, String)> {
    sessions.iter().filter(|s| s.well_formed).map(|s| (s.sender.clone(), s.device_id.clone())).collect()
}

/// Who may receive our media key (the web's `keyRecipients`): the devices of live, well-formed call sessions whose
/// user is a JOINED member of the room (`membership_of` = "join"; `None` = not loaded = no key), never our own
/// device (our OTHER devices are included); deduped, in session order. The caller re-runs it right before sending.
pub fn key_recipients(
    live: &[LiveSession],
    membership_of: &dyn Fn(&str) -> Option<String>,
    own_user_id: &str,
    own_device_id: &str,
) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut seen: HashSet<(String, String)> = HashSet::new();
    for m in live {
        if m.sender.is_empty() || m.device_id.is_empty() || !m.well_formed {
            continue;
        }
        if m.sender == own_user_id && m.device_id == own_device_id {
            continue;
        }
        if membership_of(&m.sender).as_deref() != Some("join") {
            continue;
        }
        let k = (m.sender.clone(), m.device_id.clone());
        if !seen.insert(k.clone()) {
            continue;
        }
        out.push(k);
    }
    out
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
    // ---- announce recipients (web keyrules.test.ts "announce recipients", the same cases) ----
    fn session(sender: &str, dev: &str, well_formed: bool) -> LiveSession {
        LiveSession {
            sender: sender.into(),
            device_id: dev.into(),
            state_key: format!("_{sender}_{dev}_m.call"),
            membership_id: String::new(),
            well_formed,
            created_ts: 0,
            focus: None,
        }
    }
    #[test]
    fn announce_joined_live_members_only_deduped_never_self() {
        let live = vec![
            session(ALICE, "ADEV1", true),
            session(ALICE, "ADEV1", true),
            session(ME, "MEDEV", true),
            session(ME, "OTHER", true),
            session("@kicked:example.com", "K1", true),
            session("@unknown:x", "U1", true),
            session(MALLORY, "", true),
        ];
        // (unsorted: session order, as the web's loop)
        let ids: Vec<String> = key_recipients(&live, &membership, ME, "MEDEV").into_iter().map(|(u, d)| format!("{u}|{d}")).collect();
        assert_eq!(ids, vec![format!("{ALICE}|ADEV1"), format!("{ME}|OTHER")]);
    }
    #[test]
    fn announce_skips_memberships_that_fail_the_strict_schema() {
        assert!(key_recipients(&[session(ALICE, "ADEV1", false)], &membership, ME, "MEDEV").is_empty());
    }
    /// 12.0.5: kicked / banned / left / invited / knocking members get no key; the membership is read per call, so
    /// a leave that lands between scheduling and sending drops the device on the re-check.
    #[test]
    fn announce_only_joined_never_left_kicked_banned_or_unknown() {
        let live = vec![session(ALICE, "ADEV1", true), session("@bob:example.com", "BDEV", true), session(ME, "PHONE", true)];
        for (state, expect_bob) in [("join", true), ("leave", false), ("ban", false), ("invite", false), ("knock", false)] {
            let ms = |u: &str| if u == "@bob:example.com" { Some(state.to_string()) } else { membership(u) };
            let r = key_recipients(&live, &ms, ME, "MEDEV");
            assert_eq!(r.iter().any(|(u, _)| u == "@bob:example.com"), expect_bob, "bob {state}");
            assert!(r.contains(&(ALICE.to_string(), "ADEV1".to_string())) && r.contains(&(ME.to_string(), "PHONE".to_string())), "{state}");
        }
        // not loaded = no key (never sent on trust)
        let none = |_: &str| None;
        assert!(key_recipients(&live, &none, ME, "MEDEV").is_empty());
        // the re-check before sending: bob was joined when the send was scheduled, left before it went out
        let left = std::cell::Cell::new(false);
        let ms = |u: &str| if u == "@bob:example.com" && left.get() { Some("leave".into()) } else { membership(u).or(Some("join".into())) };
        let scheduled = key_recipients(&live, &ms, ME, "MEDEV");
        assert_eq!(scheduled.len(), 3);
        left.set(true);
        let still: HashSet<(String, String)> = key_recipients(&live, &ms, ME, "MEDEV").into_iter().collect();
        let sent: Vec<_> = scheduled.into_iter().filter(|r| still.contains(r)).collect();
        assert!(!sent.iter().any(|(u, _)| u == "@bob:example.com") && sent.len() == 2);
    }
    #[test]
    fn live_sessions_carry_created_ts_and_focus() {
        let mut a = good("ADEV1");
        a["foci_preferred"] = json!([{ "type": "nextgen", "livekit_service_url": "https://x.test" },
            { "type": "livekit", "livekit_service_url": "" }, { "type": "livekit", "livekit_service_url": "https://a.test/livekit/jwt", "livekit_alias": "!r:a.test" }]);
        let mut b = good("ADEV2");
        b.as_object_mut().unwrap().remove("created_ts");
        b["foci_active"] = json!([{ "type": "livekit", "livekit_service_url": "https://b.test/livekit/jwt" }]);
        let mut e2 = mem(ALICE, &format!("_{ALICE}_ADEV2_m.call"), b);
        e2["origin_server_ts"] = json!(1_500u64);
        let s = live_sessions_from(&[mem(ALICE, &format!("_{ALICE}_ADEV1_m.call"), a), e2], 2_000);
        assert_eq!((s[0].created_ts, s[0].focus.clone()), (1_000, Some(("https://a.test/livekit/jwt".into(), "!r:a.test".into()))));
        assert_eq!((s[1].created_ts, s[1].focus.clone()), (1_500, Some(("https://b.test/livekit/jwt".into(), String::new()))));
    }

    // ---- shared calls-round vectors (share calls-round/calls.vectors.draft.json, copied verbatim): section
    // `recipients`. Cases 15–17 are today's web rule (joined + well-formed + not self) and must pass; 18–20 add the
    // 12.1.0 clauses (known device list, call-server departures) that neither the web nor 12.0.5 has yet.
    fn merge_patch(target: &mut serde_json::Value, patch: &serde_json::Value) {
        let (Some(t), Some(p)) = (target.as_object_mut(), patch.as_object()) else { return };
        for (k, v) in p {
            if v.is_null() {
                t.remove(k);
            } else if v.is_object() && t.get(k).map(|x| x.is_object()).unwrap_or(false) {
                merge_patch(t.get_mut(k).unwrap(), v);
            } else {
                t.insert(k.clone(), v.clone());
            }
        }
    }
    fn expand_member(m: &serde_json::Value, room: &str) -> serde_json::Value {
        let g = |k: &str| m.get(k).and_then(|v| v.as_str()).map(str::to_string);
        let (s, d) = (g("s").unwrap(), g("d").unwrap());
        let c = m["c"].as_u64().unwrap();
        let mut content = json!({
            "application": "m.call", "call_id": "", "scope": "m.room", "device_id": d,
            "focus_active": { "type": "livekit", "focus_selection": "oldest_membership" },
            "foci_preferred": [{ "type": "livekit",
                "livekit_service_url": g("f").unwrap_or_else(|| "https://a.test/livekit/jwt".into()),
                "livekit_alias": g("a").unwrap_or_else(|| room.to_string()) }],
            "created_ts": c, "expires": 14_400_000u64, "membershipID": format!("{s}:{d}"),
        });
        if let Some(p) = m.get("patch") {
            merge_patch(&mut content, p);
        }
        json!({ "type": "org.matrix.msc3401.call.member", "sender": s,
                "state_key": g("sk").unwrap_or_else(|| format!("_{s}_{d}_m.call")), "origin_server_ts": c, "content": content })
    }
    #[test]
    fn calls_vectors_recipients() {
        let v: serde_json::Value = serde_json::from_str(include_str!("testvectors/calls-draft.json")).unwrap();
        let defs = &v["defaults"];
        let cases = v["recipients"].as_array().unwrap();
        assert_eq!(cases.len(), 6);
        for (i, case) in cases.iter().enumerate() {
            let num = 15 + i; // the vectors' global numbering (focus 1–13, sfuUrl 14)
            let field = |k: &str| case.get(k).unwrap_or(&defs[k]).clone();
            let room = field("room").as_str().unwrap().to_string();
            let now = field("now").as_u64().unwrap();
            let me = field("me").as_str().unwrap().to_string();
            let (me_user, me_dev) = me.split_once('|').unwrap();
            let joined = field("joined");
            let events: Vec<serde_json::Value> = case["members"].as_array().unwrap().iter().map(|m| expand_member(m, &room)).collect();
            let sessions = live_sessions_from(&events, now);
            let ms = |u: &str| Some(joined.get(u).and_then(|x| x.as_str()).unwrap_or("join").to_string());
            let mut got: Vec<String> = key_recipients(&sessions, &ms, me_user, me_dev).into_iter().map(|(u, d)| format!("{u}|{d}")).collect();
            got.sort();
            let expect: Vec<String> = case["expect"].as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect();
            let name = case["name"].as_str().unwrap();
            match num {
                15..=17 | 20 => assert_eq!(got, expect, "#{num} {name}"),
                // 12.1.0 clauses (removed device, call-server departure) are not in today's rule: it still keys the
                // device those cases exclude, and nothing else changes
                18 | 19 => {
                    assert_ne!(got, expect, "#{num} {name}: passes already — promote it to the asserted set");
                    assert_eq!(got, vec!["@alice:a.test|ADEV".to_string(), "@bob:b.test|BDEV".to_string()], "#{num} {name}");
                }
                _ => unreachable!(),
            }
        }
    }
}
