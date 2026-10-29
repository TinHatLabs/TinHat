//! #7 call-extra: AFK / BRB presence over MATRIX (LiveKit data channels/attributes do
//! NOT propagate on this SFU). A user marks themselves "away" while in a call; peers see
//! their tile dimmed with a 💤 glyph.
//!
//! TRANSPORT (primary, authoritative): a per-user custom STATE event
//! `dev.tinhat.afk` (state_key = userId, content `{away:bool}`) via `appstate`.
//! Every peer reads it straight from their LOCAL state store — no polling, no relations,
//! no per-peer requests. Requires the room to grant PL-0 members that event type, which
//! `appstate::provision_room` does on room open for an admin.
//!
//! TRANSPORT (legacy, best-effort): an `m.reaction` (key `💤`) ANNOTATING the user's OWN
//! `org.matrix.msc3401.call.member` state event, read back with
//! `room.relations(member_event_id, Annotation)`. Kept ONLY so peers still running older
//! builds can see our away state. The READ tolerates BOTH paths, so a peer using either
//! is detected.
//!
//! ⚠️ HISTORY — why the order above is not an accident. The reaction used to be primary
//! and `set_own_afk` returned early the moment the reaction POST succeeded, so the state
//! event was never written when the write landed. But some homeservers ACCEPT a reaction
//! annotating a state event while REFUSING to serve its relations (403 "You don't have
//! permission to access that event."). On such a server the write "succeeded", every
//! reader's 403 was swallowed as "not away", and BRB was **100% invisible to all peers** —
//! undetectable locally, because `local_afk` is pinned to the optimistic `AFK_INTENT`
//! atomic rather than any server read-back, so the mute/camera/privacy side effects all
//! fired normally. It also cost 40 wasted requests/min forever (see the latch below).
//! Do not reintroduce a reaction-first ordering, and do not gate the state write on it.
//!
//! All Matrix-events — no audio path — so it's fully headless-verifiable (TINHAT_AFK_TEST).

use core::pin::Pin;

use cxx_qt::Threading;
use cxx_qt_lib::QString;
use matrix_sdk::room::{IncludeRelations, RelationsOptions};
use matrix_sdk::ruma::api::Direction;
use matrix_sdk::ruma::events::relation::RelationType;
use matrix_sdk::ruma::{EventId, OwnedEventId, RoomId};
use matrix_sdk::Room;

/// The reaction key (and the custom-state marker) that means "I'm away / BRB".
const ZZZ: &str = "💤";
/// EC membership state-event type our reaction annotates (matches `rtc.rs`).
const MEMBER_EVENT_TYPE: &str = "org.matrix.msc3401.call.member";
/// Per-user fallback custom state event (state_key = userId), `{away:bool}`.
const AFK_STATE_TYPE: &str = "dev.tinhat.afk";

/// Our AUTHORITATIVE BRB intent — what the user last asked for via the button. The async
/// read-back in `set_afk`/`refresh` must never lower `local_afk` below this: otherwise a
/// slow/racey relation read (our 💤 marker not yet observed by sync) reverts the optimistic
/// flip and the BRB button looks like it "did nothing". When intent is true the UI stays
/// away; when false it reflects the real read. Reconciled up by the periodic refresh.
static AFK_INTENT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// ── Relations-transport viability latch ────────────────────────────────────────────────
///
/// Some deployed homeservers refuse to SERVE the relations of a STATE event: every read
/// answers `403 "You don't have permission to access that event."`. One user's 42-minute
/// diagnostic captured **1688** of them — a rock-steady 40/min, one per in-call member
/// event per 3s poll, for the entire 5-hour call. Nothing damped it: matrix-sdk classifies
/// 403 as `RetryKind::Permanent` (so the SDK does not retry, but it does not remember
/// either), and BOTH read sites simply swallowed the error and asked again 3 seconds later.
///
/// Latch on the first DEFINITE 403 and stop asking. Re-probe after a while rather than
/// latching forever, so a server-side fix — or a room whose parent events the server will
/// serve — recovers on its own without a restart. Only a real 403 latches: a network blip,
/// timeout or 5xx must keep retrying, since those are exactly the cases where the next
/// attempt may succeed.
static RELATIONS_403_UNTIL: std::sync::Mutex<Option<std::time::Instant>> =
    std::sync::Mutex::new(None);
/// How long a 403 suppresses the reaction transport before one probe request is allowed.
const RELATIONS_REPROBE: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// May we spend a relations request right now?
fn relations_usable() -> bool {
    match *RELATIONS_403_UNTIL.lock().unwrap() {
        Some(until) => std::time::Instant::now() >= until,
        None => true,
    }
}

/// Record a definite 403 from a relations read and suppress the transport for a while.
fn note_relations_forbidden() {
    let now = std::time::Instant::now();
    let mut guard = RELATIONS_403_UNTIL.lock().unwrap();
    let prev: Option<std::time::Instant> = *guard;
    *guard = Some(now + RELATIONS_REPROBE);
    drop(guard); // never hold a std Mutex across an await
    // Log only on the transition into suppression, not on every re-probe failure.
    if prev.is_none_or(|until| now >= until) {
        eprintln!(
            "[afk] homeserver refuses relations on call.member state events (403) — \
             suppressing the reaction transport for {}min; BRB rides the \
             dev.tinhat.afk state event, which every peer reads locally",
            RELATIONS_REPROBE.as_secs() / 60
        );
    }
}

/// `true` ONLY for a definite HTTP 403 / `M_FORBIDDEN`. `false` for network errors,
/// timeouts, 429, 5xx and deserialization failures — anything that could succeed on retry
/// and so must NOT latch the transport off.
fn is_forbidden(e: &matrix_sdk::Error) -> bool {
    use matrix_sdk::ruma::api::error::ErrorKind;
    use matrix_sdk::HttpError;
    fn http_forbidden(h: &HttpError) -> bool {
        // `Cached` wraps another HttpError and `as_client_api_error` does not see through it.
        if let HttpError::Cached(inner) = h {
            return http_forbidden(inner);
        }
        match h.as_client_api_error() {
            // status_code covers a bare 403 with a non-Matrix body (proxy/HTML);
            // error_kind covers M_FORBIDDEN carried on any status.
            Some(api) => {
                api.status_code.as_u16() == 403
                    || matches!(api.error_kind(), Some(ErrorKind::Forbidden))
            }
            None => false,
        }
    }
    match e {
        matrix_sdk::Error::Http(h) => http_forbidden(h),
        _ => false,
    }
}

/// Clear the BRB intent — call on EVERY call end (leave / terminal disconnect) and on a fresh
/// join, so a call NEVER inherits stale away-state. Without this, BRB stays sticky across a
/// teardown and re-arms the next (re)join: the rising edge re-mutes the mic, unpublishes the
/// camera, and cuts incoming audio, so a rejoined call comes back dead ("can't recover").
pub fn clear_intent() {
    AFK_INTENT.store(false, std::sync::atomic::Ordering::Relaxed);
}

/// Clear a STALE `away:true` left on the server, before a fresh call starts.
///
/// `clear_intent` only resets the local atomic — nothing ever cleared the server side. That
/// was invisible while the state transport was effectively dead (`set_own_afk`'s old
/// reaction-first early return meant it was rarely written at all), but now that the state
/// event is authoritative a BRB that is never cleared — leave while away, quit while away,
/// crash while away — would make us show up AWAY to every peer the moment we rejoin, with
/// the local UI showing "back" because `AFK_INTENT` starts false. Clearing on JOIN rather
/// than on leave is deliberate: it is the one hook that also survives a crash or a restart.
///
/// Cheap and quiet: `away_via_state` is a LOCAL store read, so in the overwhelmingly common
/// case (not away) this costs no network and writes no event.
pub async fn clear_stale_away(room: &Room) {
    let me = room.own_user_id().to_string();
    if !away_via_state(room).await.contains(&me) {
        return;
    }
    eprintln!("[afk] clearing stale away state from a previous call");
    if let Err(e) = write_afk_state(room, false).await {
        eprintln!("[afk] failed to clear stale away state: {e}");
    }
}

#[cxx_qt::bridge]
pub mod qobject {
    extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[auto_cxx_name] // snake_case Rust -> camelCase QML
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(QString, room_id)] //   QML: roomId — the call's room this controller tracks
        #[qproperty(bool, local_afk)] //    QML: localAfk — whether WE are currently marked away
        #[qproperty(QString, away_json)] // QML: awayJson — JSON [userId,…] of away users in the call
        type AfkController = super::AfkControllerRust;

        /// QML: setRoom(roomId) — track a room's call + read who's away.
        #[qinvokable]
        fn set_room(self: Pin<&mut AfkController>, room_id: &QString);
        /// QML: refresh() — re-read the away set for the tracked room.
        #[qinvokable]
        fn refresh(self: Pin<&mut AfkController>);
        /// QML: setAfk(on) — mark ourselves away (true) / back (false); updates Matrix.
        #[qinvokable]
        fn set_afk(self: Pin<&mut AfkController>, on: bool);
    }

    impl cxx_qt::Threading for AfkController {}
}

#[derive(Default)]
pub struct AfkControllerRust {
    room_id: QString,
    local_afk: bool,
    away_json: QString,
}

impl qobject::AfkController {
    pub fn set_room(mut self: Pin<&mut Self>, room_id: &QString) {
        let rid = room_id.to_string();
        if rid == self.room_id().to_string() {
            self.refresh();
            return;
        }
        self.as_mut().set_room_id(QString::from(&rid));
        self.as_mut().set_away_json(QString::from("[]"));
        self.as_mut().set_local_afk(false);
        AFK_INTENT.store(false, std::sync::atomic::Ordering::Relaxed); // new room = not away
        self.refresh();
    }

    pub fn refresh(self: Pin<&mut Self>) {
        let room_id = self.room_id().to_string();
        if room_id.is_empty() {
            return;
        }
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let (json, _own_away) = load_away(&room_id).await;
            let started = room_id.clone();
            let _ = qt.queue(move |mut o| {
                if o.room_id().to_string() == started {
                    o.as_mut().set_away_json(QString::from(&json));
                    // OUR OWN flag follows AFK_INTENT (what the user asked for) — NOT the
                    // server read-back: a lagging read (our 💤 not yet observed, or a stale
                    // own_away=true right after "I'm back") would otherwise flip local_afk and
                    // re-fire the destructive rising edge (re-mute mic / kill camera / cut
                    // INCOMING audio). away_json (others) still comes from the read.
                    o.as_mut()
                        .set_local_afk(AFK_INTENT.load(std::sync::atomic::Ordering::Relaxed));
                }
            });
        });
    }

    pub fn set_afk(mut self: Pin<&mut Self>, on: bool) {
        let room_id = self.room_id().to_string();
        if room_id.is_empty() {
            return;
        }
        // Record intent FIRST, then reflect the toggle immediately (optimistic). The async
        // reconcile below + the periodic refresh both honor AFK_INTENT, so a slow read-back
        // can never silently revert this — that was the "BRB button does nothing" bug.
        AFK_INTENT.store(on, std::sync::atomic::Ordering::Relaxed);
        self.as_mut().set_local_afk(on);
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let Some(room) = room_by_id(&room_id).await else { return };
            if let Err(e) = set_own_afk(&room, on).await {
                // This is now the AUTHORITATIVE transport, so a failure here means peers
                // will not see the change at all — even though the local UI, mic, camera
                // and audio-cut have already acted on it. The overwhelmingly likely cause
                // is the room not granting PL-0 members `dev.tinhat.afk`; an admin
                // opening the room runs `appstate::provision_room`, which fixes it.
                eprintln!(
                    "[afk] set {on} FAILED to publish ({e}) — peers will not see this. \
                     If this is M_FORBIDDEN, the room needs provisioning: have an admin \
                     open it so PL-0 members may send dev.tinhat.afk"
                );
            }
            // Re-read so the away SET (others' tiles) settles on the real state; our OWN flag
            // stays pinned to AFK_INTENT so a lagging relation read can't revert it (or, after
            // "I'm back", spuriously raise it and re-fire the rising-edge mute/cam/cut).
            let (json, _own_away) = load_away(&room_id).await;
            let started = room_id.clone();
            let _ = qt.queue(move |mut o| {
                if o.room_id().to_string() == started {
                    o.as_mut().set_away_json(QString::from(&json));
                    o.as_mut()
                        .set_local_afk(AFK_INTENT.load(std::sync::atomic::Ordering::Relaxed));
                }
            });
        });
    }
}

async fn room_by_id(room_id: &str) -> Option<Room> {
    let client = crate::session::client()?;
    let rid = RoomId::parse(room_id).ok()?;
    client.get_room(&rid)
}

/// The raw `(event_id, sender)` of each `m.call.member` state event in the room.
/// (`sender` = the member; `event_id` is what a reaction annotates / relations reads.)
async fn member_event_ids(room: &Room) -> Vec<(OwnedEventId, String)> {
    use matrix_sdk::ruma::events::StateEventType;
    let mut out = Vec::new();
    let Ok(raws) = room.get_state_events(StateEventType::from(MEMBER_EVENT_TYPE)).await else {
        return out;
    };
    for raw in raws {
        let v = match &raw {
            matrix_sdk::deserialized_responses::RawAnySyncOrStrippedState::Sync(r) => {
                r.deserialize_as::<serde_json::Value>().ok()
            }
            matrix_sdk::deserialized_responses::RawAnySyncOrStrippedState::Stripped(r) => {
                r.deserialize_as::<serde_json::Value>().ok()
            }
        };
        let Some(v) = v else { continue };
        let sender = v.get("sender").and_then(|s| s.as_str()).unwrap_or("").to_string();
        let Some(eid_str) = v.get("event_id").and_then(|e| e.as_str()) else { continue };
        if let Ok(eid) = EventId::parse(eid_str) {
            out.push((eid, sender));
        }
    }
    out
}

/// Our OWN call.member state event id (the one our `join_rtc_session` published, matched
/// by sender == our user id). The most-recent (last) wins if multiple devices are present.
async fn own_member_event_id(room: &Room) -> Option<OwnedEventId> {
    let me = room.own_user_id().to_string();
    member_event_ids(room)
        .await
        .into_iter()
        .filter(|(_, sender)| *sender == me)
        .last()
        .map(|(eid, _)| eid)
}

/// Is `sender` away, per a 💤 reaction annotating their `member_eid`? Reads relations of
/// type Annotation and looks for the 💤 key from that same sender. Tolerant: returns
/// false on any error so a HS that rejects state-annotations just yields "not away here".
async fn away_via_reaction(room: &Room, member_eid: &OwnedEventId, sender: &str) -> bool {
    // This is the ONLY network request in the 3s AFK poll (call_participants,
    // away_via_state and member_event_ids are all local state-store reads), so the latch
    // here is what turns 40 wasted requests/min into two per re-probe window.
    if !relations_usable() {
        return false;
    }
    let opts = RelationsOptions {
        from: None,
        dir: Direction::Backward,
        limit: matrix_sdk::ruma::UInt::new(100),
        include_relations: IncludeRelations::RelationsOfType(RelationType::Annotation),
        recurse: false,
    };
    let relations = match room.relations(member_eid.clone(), opts).await {
        Ok(r) => r,
        Err(e) => {
            if is_forbidden(&e) {
                note_relations_forbidden();
            }
            return false;
        }
    };
    for ev in &relations.chunk {
        let Ok(mut v) = ev.raw().deserialize_as::<serde_json::Value>() else { continue };
        crate::moved::normalize(&mut v); // a moved server: today's ids
        if v.get("type").and_then(|t| t.as_str()) != Some("m.reaction") {
            continue;
        }
        if v.get("sender").and_then(|s| s.as_str()) != Some(sender) {
            continue;
        }
        let key = v
            .get("content")
            .and_then(|c| c.get("m.relates_to"))
            .and_then(|r| r.get("key"))
            .and_then(|k| k.as_str());
        if key == Some(ZZZ) {
            return true;
        }
    }
    false
}

/// Away set for the FALLBACK path: userIds whose `dev.tinhat.afk` state has away=true.
async fn away_via_state(room: &Room) -> std::collections::HashSet<String> {
    let mut set = std::collections::HashSet::new();
    for (state_key, _sender, content) in crate::appstate::get_state_all(room, AFK_STATE_TYPE).await {
        if content.get("away").and_then(|a| a.as_bool()) == Some(true) && !state_key.is_empty() {
            set.insert(state_key);
        }
    }
    set
}

/// Compute `(away_json, own_away)` for a room: union of the reaction path and the custom
/// -state fallback, intersected with who's actually IN the call (via `rtc::call_participants`).
async fn load_away(room_id: &str) -> (String, bool) {
    let Some(room) = room_by_id(room_id).await else {
        return ("[]".to_string(), false);
    };
    let in_call = crate::rtc::call_participants(&room).await;
    let me = room.own_user_id().to_string();
    let state_away = away_via_state(&room).await;
    let members = member_event_ids(&room).await;

    let mut away: Vec<String> = Vec::new();
    for uid in &in_call {
        let mut is_away = state_away.contains(uid);
        if !is_away {
            // Reaction path: check EVERY member event this user has (one per device — a
            // stale second-device membership is common). The writer annotates ITS OWN
            // (last-published) event, so reading only the FIRST match missed the 💤
            // whenever an older device's event sorted first, and peers never saw AFK.
            for (eid, _) in members.iter().filter(|(_, s)| s == uid) {
                if away_via_reaction(&room, eid, uid).await {
                    is_away = true;
                    break;
                }
            }
        }
        if is_away && !away.iter().any(|u| u == uid) {
            away.push(uid.clone());
        }
    }
    let own_away = away.iter().any(|u| u == &me);
    let json = serde_json::to_string(&away).unwrap_or_else(|_| "[]".to_string());
    (json, own_away)
}

/// Mark ourselves away (`on`) or back. The `dev.tinhat.afk` STATE event is the
/// authoritative transport; the 💤 reaction is a best-effort legacy mirror.
///
/// This used to be reaction-FIRST with an early `return Ok(())` the instant the reaction
/// POST succeeded — so `write_afk_state` was unreachable whenever the write landed. On a
/// homeserver that ACCEPTS the reaction write but 403s the relations READ (exactly
/// what one deployed server did), that meant the state event was never written at all, every peer's
/// `away_via_reaction` swallowed its 403 and reported "not away", and **BRB was 100%
/// invisible to everyone** while still looking fine locally — `local_afk` is driven by the
/// optimistic `AFK_INTENT` atomic, not by any server read-back, so the mute/camera/privacy
/// side effects all fired and nothing looked broken from the inside.
///
/// So: always write the state event (it is a local-store read for every peer — no polling,
/// no relations, no permission surprises beyond the PL-0 provisioning `appstate` already
/// handles), and only touch the reaction transport when relations are known to work.
async fn set_own_afk(room: &Room, on: bool) -> matrix_sdk::Result<()> {
    // AUTHORITATIVE, always — never gated on the reaction transport's success.
    let state_result = write_afk_state(room, on).await;

    // Legacy mirror for peers on older builds, which read AFK only via relations. Skipped
    // entirely once the latch is set: that both ends the request flood and — importantly —
    // stops us stranding 💤 reactions we could never find again to redact. A stranded 💤
    // is not harmless: the reader ORs the two transports and `state:false` does NOT veto
    // `reaction:true`, so if relations ever START working, everyone who pressed BRB once
    // would show permanently away with no way to clear it.
    if relations_usable() {
        if let Some(eid) = own_member_event_id(room).await {
            if on {
                // Re-check AFTER the read: own_reaction_id may have just latched the
                // transport off, and we must never write a 💤 the clear path can't reach.
                let already = own_reaction_id(room, &eid).await.is_some();
                if !already && relations_usable() {
                    if let Err(e) = send_reaction(room, &eid, ZZZ).await {
                        eprintln!("[afk] legacy reaction annotate rejected ({e}) — harmless, the state transport carries it");
                    }
                }
            } else if let Some(rid) = own_reaction_id(room, &eid).await {
                let _ = room.redact(&rid, None, None).await;
            }
        }
    }
    state_result
}

/// Send an `m.reaction` annotating `target` with `key` (raw, so it can annotate a STATE
/// event — `room.send(ReactionEventContent)` would also work but raw keeps it explicit).
async fn send_reaction(room: &Room, target: &OwnedEventId, key: &str) -> matrix_sdk::Result<()> {
    let content = serde_json::json!({
        "m.relates_to": { "rel_type": "m.annotation", "event_id": target, "key": key }
    });
    room.send_raw("m.reaction", content).await?;
    Ok(())
}

/// Our own 💤-reaction event id annotating `member_eid`, for redaction. None if absent.
async fn own_reaction_id(room: &Room, member_eid: &OwnedEventId) -> Option<OwnedEventId> {
    if !relations_usable() {
        return None;
    }
    let me = room.own_user_id().to_string();
    let opts = RelationsOptions {
        from: None,
        dir: Direction::Backward,
        limit: matrix_sdk::ruma::UInt::new(100),
        include_relations: IncludeRelations::RelationsOfType(RelationType::Annotation),
        recurse: false,
    };
    let relations = match room.relations(member_eid.clone(), opts).await {
        Ok(r) => r,
        Err(e) => {
            if is_forbidden(&e) {
                note_relations_forbidden();
            }
            return None;
        }
    };
    for ev in &relations.chunk {
        let Ok(mut v) = ev.raw().deserialize_as::<serde_json::Value>() else { continue };
        crate::moved::normalize(&mut v); // a moved server: today's ids
        if v.get("type").and_then(|t| t.as_str()) != Some("m.reaction") {
            continue;
        }
        if v.get("sender").and_then(|s| s.as_str()) != Some(me.as_str()) {
            continue;
        }
        let key = v
            .get("content")
            .and_then(|c| c.get("m.relates_to"))
            .and_then(|r| r.get("key"))
            .and_then(|k| k.as_str());
        if key == Some(ZZZ) {
            return ev.event_id().map(ToOwned::to_owned); // matrix-sdk 0.19: borrowed
        }
    }
    None
}

/// Write our per-user `dev.tinhat.afk` state (state_key = our userId), `{away}`.
async fn write_afk_state(room: &Room, away: bool) -> matrix_sdk::Result<()> {
    crate::appstate::ensure_writable(room, &[AFK_STATE_TYPE]).await;
    let me = room.own_user_id().to_string();
    let content = serde_json::json!({ "away": away, "ts": crate::appstate::now_ms() });
    crate::appstate::send_state(room, AFK_STATE_TYPE, &me, content).await
}

/// Headless verification (no GUI), via `TINHAT_AFK_TEST='!room:server'`:
/// set_afk(true) -> read back our own away -> set_afk(false) -> gone. Reports WHICH
/// transport path worked (reaction vs custom-state fallback). Tolerates a HS rejecting a
/// reaction on a state event (that's exactly what the fallback covers).
pub async fn headless_verify(room_id: &str) {
    let Some(room) = room_by_id(room_id).await else {
        eprintln!("[afk] room not found in store");
        return;
    };
    let client = match crate::session::client() {
        Some(c) => c,
        None => {
            eprintln!("[afk] not signed in");
            return;
        }
    };
    let me = room.own_user_id().to_string();

    // Which path will set_own_afk(true) use? Probe the reaction send directly so we can
    // REPORT it (set_own_afk swallows the distinction internally).
    let mut path = "custom-state";
    if let Some(eid) = own_member_event_id(&room).await {
        match send_reaction(&room, &eid, ZZZ).await {
            Ok(()) => {
                path = "reaction";
                // Clean that probe reaction back up; set_own_afk re-sends below.
                let _ = client
                    .sync_once(crate::bridge::sync_settings()
                        .timeout(std::time::Duration::from_secs(8)))
                    .await;
                if let Some(rid) = own_reaction_id(&room, &eid).await {
                    let _ = room.redact(&rid, None, None).await;
                }
            }
            Err(e) => eprintln!("[afk] reaction path rejected by HS ({e}) -> fallback"),
        }
    } else {
        eprintln!("[afk] NOTE: no own m.call.member event (not in a call?) -> fallback only");
    }
    eprintln!("[afk] transport path = {path}");

    // SET away.
    if let Err(e) = set_own_afk(&room, true).await {
        eprintln!("[afk] set away FAILED: {e}");
        return;
    }
    let _ = client
        .sync_once(crate::bridge::sync_settings()
            .timeout(std::time::Duration::from_secs(10)))
        .await;
    // For the read-back to find us we must look like we're "in the call". If we are not a
    // call participant, fall back to a DIRECT read of our own marker (both paths) so the
    // probe still validates set/clear without needing a live call.
    let after_set = own_away_direct(&room, &me).await;
    eprintln!("[afk] after set: own_away = {after_set}");

    // CLEAR.
    if let Err(e) = set_own_afk(&room, false).await {
        eprintln!("[afk] clear FAILED: {e}");
        return;
    }
    let _ = client
        .sync_once(crate::bridge::sync_settings()
            .timeout(std::time::Duration::from_secs(10)))
        .await;
    let after_clear = own_away_direct(&room, &me).await;
    eprintln!("[afk] after clear: own_away = {after_clear}");

    let pass = after_set && !after_clear;
    eprintln!("[afk] VERDICT: {} (path={path})", if pass { "PASS" } else { "FAIL" });
}

/// Read our own away marker DIRECTLY (both paths), independent of call membership — used by
/// the headless probe so it works even when we're not an active call participant.
async fn own_away_direct(room: &Room, me: &str) -> bool {
    if away_via_state(room).await.contains(me) {
        return true;
    }
    if let Some(eid) = own_member_event_id(room).await {
        return away_via_reaction(room, &eid, me).await;
    }
    false
}
