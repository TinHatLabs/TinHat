//! DM ringing, the desktop's half of the client glue (share `claude/tinhat-android/RINGING.md` §5): the callee's device
//! sends the room event `dev.tinhat.call.decline` `{ring}` (encrypted in an encrypted DM; the mobile core's
//! `send_call_decline`, ring = the caller's call.member event id the server rang with). The caller's call shows
//! "<name> declined" and plays the end tone; the callee's OTHER devices drop their incoming card.
//!
//! = the web's `src/features/calls/ringglue.ts` (`declineOutcome` / `declinedText`): the same rule, the same words and
//! the same vectors (`testvectors/ringdecline.json`, owned by the web, copied byte for byte). The desktop gets no rings
//! from push (RINGING.md §4): its in-app ringing is unchanged.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use serde_json::Value;

pub const CALL_DECLINE_TYPE: &str = "dev.tinhat.call.decline";
/// A decline counts only this close to now, either way (the server keeps a ring declinable for 60 s).
pub const DECLINE_FRESH_MS: i64 = 60_000;
/// How long "<name> declined" stays on screen (ms; the web's ringglue.ts DECLINED_SHOWN_MS, the vectors'
/// `declinedShownMs`). Transient for people; gates read the `[ringglue] ring in … declined by …` stderr line.
pub const DECLINED_SHOWN_MS: i32 = 6000;

/// The core's `call_decline_content` rule: "$" + 1..254 more UTF-8 bytes, no control or white-space characters.
pub fn ring_id_ok(ring: &str) -> bool {
    ring.starts_with('$') && ring.len() >= 2 && ring.len() <= 255 && !ring.chars().any(|c| c.is_control() || c.is_whitespace())
}

/// What the device knows about the event's room right now.
pub struct DeclineCtx<'a> {
    pub me: &'a str,
    pub now_ms: i64,
    /// the event's room is a two-person room (the server rings DMs only; nobody else can speak for "the callee")
    pub dm: bool,
    /// the decliner's room membership is `join` (someone who has left can't decline; fix/left-member's join-only rule)
    pub sender_joined: bool,
    /// I am in, or joining, the call of the event's room
    pub in_call_here: bool,
    /// my incoming-call card for the event's room is up
    pub card_up: bool,
    /// `ring_sets().0`: MY live call.member event ids in the room (the ring I may have rung someone with)
    pub own_rings: &'a [String],
    /// `ring_sets().1`: the joined others' live call.member event ids (the ring I'm being rung with)
    pub live_rings: &'a [String],
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum DeclineOutcome {
    /// show `declined_text` + the end tone
    Declined,
    /// my other device declined: the card and the ring go
    Dismiss,
    None,
}

/// What a room event means to this device (the vectors' table).
pub fn decline_outcome(ev: &Value, ctx: &DeclineCtx) -> DeclineOutcome {
    if ev.get("type").and_then(Value::as_str) != Some(CALL_DECLINE_TYPE) {
        return DeclineOutcome::None;
    }
    let Some(sender) = ev.get("sender").and_then(Value::as_str).filter(|s| !s.is_empty()) else { return DeclineOutcome::None };
    // a JSON number of any kind, as the web's `typeof ts === "number"` (an f64 compare keeps 1.5e12 working too)
    let Some(ts) = ev.get("origin_server_ts").and_then(Value::as_f64).filter(|t| t.is_finite()) else { return DeclineOutcome::None };
    if (ctx.now_ms as f64 - ts).abs() > DECLINE_FRESH_MS as f64 {
        return DeclineOutcome::None;
    }
    let Some(content) = ev.get("content").and_then(Value::as_object) else { return DeclineOutcome::None };
    let Some(ring) = content.get("ring").and_then(Value::as_str).filter(|r| ring_id_ok(r)) else { return DeclineOutcome::None };
    if !ctx.dm || !ctx.sender_joined {
        return DeclineOutcome::None;
    }
    let has = |list: &[String]| list.iter().any(|x| x == ring);
    if sender == ctx.me {
        return if ctx.card_up && has(ctx.live_rings) { DeclineOutcome::Dismiss } else { DeclineOutcome::None };
    }
    if ctx.in_call_here && has(ctx.own_rings) {
        DeclineOutcome::Declined
    } else {
        DeclineOutcome::None
    }
}

/// The call.member event ids a ring may name: the room's call.member state events (`{event_id, sender, state_key,
/// origin_server_ts, content}`) that are live sessions (`keyrules::live_sessions_from`, the shared rule) of senders whose
/// membership is exactly `join` (unknown counts as not joined here: a departed caller's ring names nobody).
/// `(own, live)` = (mine, the others'), in event order. = the web's `ringSets`.
pub fn ring_sets(events: &[Value], membership_of: impl Fn(&str) -> Option<String>, me: &str, now_ms: u64) -> (Vec<String>, Vec<String>) {
    let mut own = Vec::new();
    let mut live = Vec::new();
    for ev in events {
        let (Some(id), Some(sender)) = (ev.get("event_id").and_then(Value::as_str), ev.get("sender").and_then(Value::as_str)) else { continue };
        if id.is_empty() || membership_of(sender).as_deref() != Some("join") {
            continue;
        }
        let state_key = ev.get("state_key").and_then(Value::as_str).unwrap_or("");
        if !crate::keyrules::live_sessions_from(std::slice::from_ref(ev), now_ms).iter().any(|s| s.state_key == state_key) {
            continue;
        }
        if sender == me { own.push(id.to_string()) } else { live.push(id.to_string()) }
    }
    (own, live)
}

/// How many people are joined, for the two-person-room rule (= the web's `room.getJoinedMemberCount()`: the sync
/// summary's count when the server sent one, else the members it holds). The summary's counts only arrive WITH lazy
/// loading, which Tin Hat doesn't use, so on the desktop it read 0 and no decline ever counted (assembly-038, the phone
/// gate's item 2b: the caller never showed "<name> declined"); the local member store is complete then (nav.rs
/// `dm_peer` uses the same order).
pub fn joined_count(stored_joined: Option<usize>, summary_joined: u64) -> u64 {
    if summary_joined > 0 {
        summary_joined
    } else {
        stored_joined.unwrap_or(0) as u64
    }
}

/// The words the caller sees (the web's `declinedText`: the same).
pub fn declined_text(name: &str) -> String {
    let n = name.trim();
    format!("{} declined", if n.is_empty() { "They" } else { n })
}

// ---- the sync loop's half ------------------------------------------------------------------------------------------
// A decline is judged again on every pass until it matters or goes stale (60 s): one that arrives before its incoming
// card still takes the card down once it shows. Join-only (fix/left-member's rule): a decliner who has left is ignored,
// and a ring names only a live call.member of a joined member (a departed caller's ring names nobody). One that couldn't be decrypted yet (its room key still on the way) is
// retried each pass while it's fresh, in rooms where a decline could matter (my call, my card).

#[derive(Default)]
struct Pending {
    /// readable declines not decided yet: (room, event)
    declines: Vec<(String, Value)>,
    /// encrypted timeline events not readable yet, fresh ones only (≤ 32): (room, event)
    sealed: Vec<(String, Value)>,
    /// decided declines' event ids (≤ 200)
    done: Vec<String>,
}

fn pending() -> &'static Mutex<Pending> {
    static P: OnceLock<Mutex<Pending>> = OnceLock::new();
    P.get_or_init(|| Mutex::new(Pending::default()))
}

/// Sign-out / account switch: nothing of the old session counts for the next.
pub fn forget_session() {
    *pending().lock().unwrap() = Pending::default();
}

/// One decline that changes something on screen.
pub struct Hit {
    pub room_id: String,
    pub outcome: DeclineOutcome,
    /// `declined_text` of the decliner (Declined only)
    pub text: String,
}

fn event_id(v: &Value) -> &str {
    v.get("event_id").and_then(Value::as_str).unwrap_or("")
}

fn fresh(v: &Value, now_ms: i64) -> bool {
    v.get("origin_server_ts").and_then(Value::as_f64).is_some_and(|t| (now_ms as f64 - t).abs() <= DECLINE_FRESH_MS as f64)
}

/// Every sync pass (bridge.rs, after callwatch::scan): take the pass's declines, then judge every pending one.
pub async fn on_sync(client: &matrix_sdk::Client, response: &matrix_sdk::sync::SyncResponse) -> Vec<Hit> {
    let now_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
    {
        let mut p = pending().lock().unwrap();
        for (rid, upd) in &response.rooms.joined {
            let room = rid.to_string();
            for ev in &upd.timeline.events {
                let Ok(v) = ev.raw().deserialize_as::<Value>() else { continue };
                match v.get("type").and_then(Value::as_str) {
                    Some(CALL_DECLINE_TYPE) => p.declines.push((room.clone(), v)),
                    Some("m.room.encrypted") if fresh(&v, now_ms) => p.sealed.push((room.clone(), v)),
                    _ => {}
                }
            }
        }
        // fresh ones are kept (bounded) even where nothing matters yet: the card may show next pass
        p.sealed.retain(|(_, v)| fresh(v, now_ms));
        let n = p.sealed.len();
        if n > 32 {
            p.sealed.drain(..n - 32);
        }
    }
    // a sealed one whose key has arrived since: readable now (tried only where a decline could matter: my call, my card)
    let sealed: Vec<(String, Value)> = pending().lock().unwrap().sealed.clone();
    for (room_id, v) in sealed {
        if !(crate::call_ctl::is_our_call_room(&room_id) || crate::callwatch::card_up(&room_id)) {
            continue;
        }
        let Some(room) = matrix_sdk::ruma::RoomId::parse(room_id.as_str()).ok().and_then(|r| client.get_room(&r)) else { continue };
        let Ok(json) = serde_json::value::to_raw_value(&v) else { continue };
        let raw: matrix_sdk::ruma::serde::Raw<matrix_sdk::ruma::events::room::encrypted::OriginalSyncRoomEncryptedEvent> =
            matrix_sdk::ruma::serde::Raw::from_json(json);
        let Ok(ev) = room.decrypt_event(&raw, None).await else { continue };
        if !matches!(ev.kind, matrix_sdk::deserialized_responses::TimelineEventKind::Decrypted(_)) {
            continue; // still sealed: next pass
        }
        let mut p = pending().lock().unwrap();
        p.sealed.retain(|(_, s)| event_id(s) != event_id(&v));
        if let Ok(d) = ev.raw().deserialize_as::<Value>() {
            if d.get("type").and_then(Value::as_str) == Some(CALL_DECLINE_TYPE) {
                p.declines.push((room_id, d));
            }
        }
    }
    let todo: Vec<(String, Value)> = {
        let mut p = pending().lock().unwrap();
        let Pending { declines, done, .. } = &mut *p;
        declines.retain(|(_, v)| fresh(v, now_ms) && !event_id(v).is_empty() && !done.iter().any(|d| d == event_id(v)));
        let mut ids: Vec<String> = Vec::new();
        declines.retain(|(_, v)| {
            let id = event_id(v).to_string();
            let first = !ids.contains(&id);
            ids.push(id);
            first
        });
        let n = declines.len();
        if n > 50 {
            declines.drain(..n - 50);
        }
        declines.clone()
    };
    if todo.is_empty() {
        return Vec::new();
    }
    let me = client.user_id().map(|u| u.to_string()).unwrap_or_default();
    let mut hits = Vec::new();
    for (room_id, ev) in todo {
        let Some(room) = matrix_sdk::ruma::RoomId::parse(room_id.as_str()).ok().and_then(|r| client.get_room(&r)) else { continue };
        // the room's call.member state now, and the room membership of everyone it names + the decliner
        let members: Vec<Value> = {
            use matrix_sdk::ruma::events::call::member::CallMemberEventContent;
            match room.get_state_events_static::<CallMemberEventContent>().await {
                Ok(raws) => raws.iter().filter_map(|x| serde_json::to_value(x).ok()).collect(),
                Err(_) => Vec::new(),
            }
        };
        let decliner = ev.get("sender").and_then(Value::as_str).unwrap_or("").to_string();
        let mut joined: HashMap<String, Option<String>> = HashMap::new();
        for who in members.iter().filter_map(|v| v.get("sender").and_then(Value::as_str)).chain(std::iter::once(decliner.as_str())) {
            if joined.contains_key(who) {
                continue;
            }
            let ms = match matrix_sdk::ruma::UserId::parse(who) {
                Ok(u) => room.get_member_no_sync(&u).await.ok().flatten().map(|m| m.membership().to_string()),
                Err(_) => None,
            };
            joined.insert(who.to_string(), ms);
        }
        let membership_of = |u: &str| joined.get(u).cloned().flatten();
        let stored_joined = room.members_no_sync(matrix_sdk::RoomMemberships::JOIN).await.ok().map(|ms| ms.len());
        let (own, live) = ring_sets(&members, membership_of, &me, now_ms.max(0) as u64);
        let ctx = DeclineCtx {
            me: &me,
            now_ms,
            dm: joined_count(stored_joined, room.joined_members_count()) == 2,
            sender_joined: joined.get(decliner.as_str()).cloned().flatten().as_deref() == Some("join"),
            in_call_here: crate::call_ctl::is_our_call_room(&room_id),
            card_up: crate::callwatch::card_up(&room_id),
            own_rings: &own,
            live_rings: &live,
        };
        let outcome = decline_outcome(&ev, &ctx);
        if outcome == DeclineOutcome::None {
            continue; // judged again next pass while fresh
        }
        {
            let mut p = pending().lock().unwrap();
            let id = event_id(&ev).to_string();
            p.declines.retain(|(_, v)| event_id(v) != id);
            p.done.push(id);
            let n = p.done.len();
            if n > 200 {
                p.done.drain(..n - 200);
            }
        }
        let sender = ev.get("sender").and_then(Value::as_str).unwrap_or("").to_string();
        let mut name = String::new();
        if outcome == DeclineOutcome::Declined {
            if let Ok(u) = matrix_sdk::ruma::UserId::parse(sender.as_str()) {
                if let Ok(Some(m)) = room.get_member_no_sync(&u).await {
                    name = m.display_name().map(str::to_string).unwrap_or_default();
                }
            }
            if name.is_empty() {
                name = sender.trim_start_matches('@').split(':').next().unwrap_or("").to_string();
            }
        } else {
            crate::callwatch::forget_card(&room_id);
            eprintln!("[ringglue] ring in {room_id} declined on another of my devices — card dismissed");
        }
        let text = if outcome == DeclineOutcome::Declined { declined_text(&name) } else { String::new() };
        if outcome == DeclineOutcome::Declined {
            // the gates' stable line (tools/README-gates.md "Declined rings"): the notification itself is transient
            let ring = ev.get("content").and_then(|c| c.get("ring")).and_then(Value::as_str).unwrap_or("");
            eprintln!("[ringglue] ring in {room_id} declined by {sender} (ring {ring}) — notice requested: {text:?} for {DECLINED_SHOWN_MS} ms");
        }
        hits.push(Hit { room_id, outcome, text });
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) const VECTORS: &str = include_str!("testvectors/ringdecline.json");

    fn strings(v: &Value) -> Vec<String> {
        v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default()
    }

    /// The shared vectors (web ringglue.test.ts runs the same file).
    #[test]
    fn vectors() {
        let v: Value = serde_json::from_str(VECTORS).expect("vectors parse");
        assert_eq!(v["v"], 1);
        let now = v["now"].as_i64().expect("now");
        let cases = v["cases"].as_array().expect("cases");
        assert_eq!(cases.len(), 33);
        for c in cases {
            let name = c["name"].as_str().unwrap_or("?");
            let x = &c["ctx"];
            let (own, live) = (strings(&x["ownRings"]), strings(&x["liveRings"]));
            let ctx = DeclineCtx {
                me: x["me"].as_str().expect("me"),
                now_ms: now,
                dm: x["dm"].as_bool().expect("dm"),
                sender_joined: x["senderJoined"].as_bool().expect("senderJoined"),
                in_call_here: x["inCallHere"].as_bool().expect("inCallHere"),
                card_up: x["cardUp"].as_bool().expect("cardUp"),
                own_rings: &own,
                live_rings: &live,
            };
            let want = match c["want"].as_str() {
                Some("declined") => DeclineOutcome::Declined,
                Some("dismiss") => DeclineOutcome::Dismiss,
                _ => DeclineOutcome::None,
            };
            assert_eq!(decline_outcome(&c["event"], &ctx), want, "case: {name}");
        }
        let rings = v["rings"].as_array().expect("rings");
        assert_eq!(rings.len(), 16);
        for r in rings {
            let name = r["name"].as_str().unwrap_or("?");
            let ms = r["membership"].clone();
            let (own, live) = ring_sets(r["events"].as_array().expect("events"), |u| ms.get(u).and_then(Value::as_str).map(str::to_string), r["me"].as_str().unwrap(), now as u64);
            assert_eq!((own, live), (strings(&r["want"]["own"]), strings(&r["want"]["live"])), "rings: {name}");
        }
        assert_eq!(v["declinedShownMs"].as_i64(), Some(DECLINED_SHOWN_MS as i64), "the shared duration");
        assert!(DECLINED_SHOWN_MS > 0 && DECLINED_SHOWN_MS < 10_000);
        let texts = v["texts"].as_array().expect("texts");
        assert_eq!(texts.len(), 5);
        for t in texts {
            assert_eq!(declined_text(t[0].as_str().unwrap()), t[1].as_str().unwrap());
        }
    }

    /// The vectors are the web's own file, byte for byte (`TINHAT_WEB_DIR` = a tinhat-web checkout; skipped without).
    #[test]
    fn joined_count_summary_then_member_store() {
        // the desktop's case: no lazy loading, so the sync summary says 0 while the store holds both people
        assert_eq!(joined_count(Some(2), 0), 2);
        // the server sent a summary (lazy loading): it wins, as in matrix-js-sdk (the store may be partial then)
        assert_eq!(joined_count(Some(2), 3), 3);
        assert_eq!(joined_count(Some(0), 2), 2);
        assert_eq!(joined_count(None, 2), 2);
        // nothing known: not a two-person room
        assert_eq!(joined_count(None, 0), 0);
    }

    #[test]
    fn vectors_are_the_webs() {
        let Ok(dir) = std::env::var("TINHAT_WEB_DIR") else {
            eprintln!("SKIPPED: TINHAT_WEB_DIR unset");
            return;
        };
        let path = std::path::Path::new(&dir).join("src/features/calls/testvectors/ringdecline.json");
        match std::fs::read_to_string(&path) {
            Ok(theirs) => assert!(VECTORS == theirs, "testvectors/ringdecline.json differs from {}", path.display()),
            Err(e) => panic!("TINHAT_WEB_DIR set but {} unreadable: {e}", path.display()),
        }
    }

    /// Garbage never panics and changes nothing.
    #[test]
    fn garbage() {
        let rings = vec!["$r".to_string()];
        let ctx = DeclineCtx { me: "@me:x", now_ms: 1_000_000, dm: true, sender_joined: true, in_call_here: true, card_up: true, own_rings: &rings, live_rings: &rings };
        for g in [
            Value::Null,
            serde_json::json!(7),
            serde_json::json!("x"),
            serde_json::json!([]),
            serde_json::json!({"type": CALL_DECLINE_TYPE}),
            serde_json::json!({"type": CALL_DECLINE_TYPE, "sender": "@b:x", "origin_server_ts": 1_000_000, "content": null}),
            serde_json::json!({"type": CALL_DECLINE_TYPE, "sender": "@b:x", "origin_server_ts": 1_000_000, "content": []}),
        ] {
            assert_eq!(decline_outcome(&g, &ctx), DeclineOutcome::None, "{g}");
        }
        let none: (Vec<String>, Vec<String>) = (vec![], vec![]);
        assert_eq!(ring_sets(&[Value::Null, serde_json::json!({}), serde_json::json!({"event_id": 5, "sender": "@me:x"})], |_| Some("join".into()), "@me:x", 0), none);
        // the core's own test ids
        assert!(ring_id_ok("$abc:x"));
        for bad in ["", "$", "abc", "$a b", "$a\n"] {
            assert!(!ring_id_ok(bad), "{bad:?}");
        }
    }
}
