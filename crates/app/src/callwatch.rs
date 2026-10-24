//! Incoming-call watcher (10.0). On every sync pass, compare each joined room's active-call flag
//! with the last pass. A rising edge in a room whose call we are not in (and did not start) is an
//! INCOMING call: the LoginController raises `incomingCall(roomId, roomName, who)` — QML shows the
//! toast, rings, and posts a desktop notification with a Join action. The falling edge raises
//! `incomingCallEnded(roomId)` so the toast and the ring stop by themselves. The first pass only
//! seeds the table (a call already running when we sign in is not "incoming").
//!
//! Muted rooms never ring (same rule as desktop notifications). Rooms we are joining or in are
//! skipped via the call controller's target room.
//!
//! 10.5.1: a group room whose call keeps starting and stopping alerts ONCE per [`REANNOUNCE`]
//! (DMs ring again after [`DM_REANNOUNCE`], 10.6 A5.7), and a call whose only live member is the recorder never alerts — until a
//! person joins it: a call the recorder started alone (or was alone in when we started) is
//! re-checked every pass, and the first human in it is the incoming-call edge (Astra review
//! 2026-09-23: before, no edge ever came). Parking depends only on who is in the call, never on
//! the room's mute state. Why: a test rig that started and ended a call every few minutes in a room
//! a user belonged to, while that user was in another call, produced a notify blip, a desktop
//! notification and a "you're in another call" card for every one of them.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use matrix_sdk::Client;

/// A room that alerted is not announced again for this long: a flapping call (a test rig, a
/// client that keeps reconnecting, a recorder retrying its join) rings once, not every cycle.
/// The room list still shows the live call, so nothing is hidden.
pub const REANNOUNCE: Duration = Duration::from_secs(10 * 60);
/// 10.6 (REACH A5.7): a DM call rings again only after this gap — a caller whose phone keeps
/// dropping and reconnecting (iOS suspends it) rings once, while a real callback a minute later
/// still rings.
pub const DM_REANNOUNCE: Duration = Duration::from_secs(45);

#[derive(Default)]
struct State {
    seeded: bool,
    active: HashMap<String, bool>,
    /// room_id -> when we last raised an incoming-call alert for it.
    announced: HashMap<String, Instant>,
    /// Rooms whose call is live with nobody but the recorder in it: re-checked every pass, since
    /// the first person to join is the real start (no rising edge will come for it).
    recorder_alone: std::collections::HashSet<String>,
}

/// What one sync pass means for one room (unit-tested).
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub(crate) enum Pass {
    /// the call just started (or we saw it for the first time after the seeding pass)
    Rising,
    /// the call just ended
    Falling,
    /// still live, and last time only the recorder was in it: look again for a person
    Recheck,
    Nothing,
}

pub(crate) fn classify(prev: Option<bool>, now: bool, recorder_alone: bool) -> Pass {
    match (prev, now) {
        (Some(false) | None, true) => Pass::Rising,
        (Some(true), false) => Pass::Falling,
        (Some(true), true) if recorder_alone => Pass::Recheck,
        _ => Pass::Nothing,
    }
}

/// Every live participant we can see is the recorder (an empty list is NOT: membership may not
/// have synced yet, and that case keeps the old behaviour — alert). `recorder` is the account the user's server
/// announces (room.rs recorder_mxid); with none, nothing is filtered.
pub(crate) fn is_recorder_only(participants: &[String], recorder: Option<&str>) -> bool {
    let Some(r) = recorder else { return false };
    !participants.is_empty() && participants.iter().all(|p| p == r)
}

/// Pure decision for a rising edge (unit-tested): alert unless every live participant we can
/// see is the recorder, or this GROUP room alerted less than [`REANNOUNCE`] ago. A DM call is
/// aimed at you, so a DM rings again after only [`DM_REANNOUNCE`]: someone calling back a minute
/// after a missed or dropped call must still reach you (the flapping calls that needed the long
/// window were all in group rooms: test rigs, reconnecting clients, recorder retries), but a
/// call that reconnects every few seconds rings once (A5.7).
pub(crate) fn should_announce(last: Option<Instant>, now: Instant, participants: &[String], is_dm: bool, recorder: Option<&str>) -> bool {
    if is_recorder_only(participants, recorder) {
        return false;
    }
    let gap = if is_dm { DM_REANNOUNCE } else { REANNOUNCE };
    !last.is_some_and(|t| now.saturating_duration_since(t) < gap)
}

fn state() -> &'static Mutex<State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(State::default()))
}

/// 10.6: sign-out (or a switch of account) — the next account's first pass seeds again, and no
/// call state of the old session can ring for the next one. Call after `session::clear_client()`.
pub fn forget_session() {
    if let Ok(mut st) = state().lock() {
        *st = State::default();
    }
}

pub struct Edge {
    pub room_id: String,
    pub room_name: String,
    /// Display name of the first participant we can see (the starter, usually); "" if none yet.
    pub who: String,
    pub started: bool,
}

/// Compute this pass's edges. Cheap when nothing changed (one sync flag read per room).
pub async fn scan(client: &Client) -> Vec<Edge> {
    let gen = crate::session::generation();
    let rooms = client.joined_rooms();
    // (room, true = a real rising edge / false = a recorder-alone call re-checked)
    let mut rising: Vec<(matrix_sdk::Room, bool)> = Vec::new();
    let mut falling: Vec<matrix_sdk::Room> = Vec::new();
    // the seeding pass stays silent, but a call the recorder is alone in must still be parked
    let mut seed_active: Vec<matrix_sdk::Room> = Vec::new();
    {
        let mut st = state().lock().unwrap();
        if crate::session::generation() != gen {
            return Vec::new(); // this session ended: its rooms never touch the next one's state
        }
        let mut seen: HashMap<String, bool> = HashMap::with_capacity(rooms.len());
        for r in &rooms {
            let id = r.room_id().to_string();
            let now = r.has_active_room_call();
            let prev = st.active.get(&id).copied();
            let alone = st.recorder_alone.contains(&id);
            seen.insert(id.clone(), now);
            if !st.seeded {
                if now {
                    seed_active.push(r.clone());
                }
                continue;
            }
            match classify(prev, now, alone) {
                Pass::Rising => rising.push((r.clone(), true)),
                Pass::Recheck => rising.push((r.clone(), false)),
                Pass::Falling => {
                    st.recorder_alone.remove(&id);
                    falling.push(r.clone());
                }
                Pass::Nothing => {}
            }
        }
        st.recorder_alone.retain(|id| seen.get(id).copied().unwrap_or(false));
        st.active = seen;
        st.seeded = true;
    }
    for r in seed_active {
        if is_recorder_only(&crate::rtc::call_participants(&r).await, crate::room::recorder_mxid().as_deref()) {
            eprintln!("[callwatch] call in {} has only the recorder in it at startup; waiting for a person", r.room_id());
            state().lock().unwrap().recorder_alone.insert(r.room_id().to_string());
        }
    }
    let mut edges = Vec::new();
    for r in falling {
        edges.push(Edge { room_id: r.room_id().to_string(), room_name: String::new(), who: String::new(), started: false });
    }
    for (r, edge) in rising {
        let id = r.room_id().to_string();
        if crate::call_ctl::is_our_call_room(&id) {
            state().lock().unwrap().recorder_alone.remove(&id);
            continue; // we started it (or are joining it)
        }
        // WHO is in the call decides the parking, before (and independent of) whether we may ring:
        // a room muted for a while must not lose the first person who joins a recorder-alone call.
        let participants = crate::rtc::call_participants(&r).await;
        let recorder = crate::room::recorder_mxid();
        if is_recorder_only(&participants, recorder.as_deref()) {
            // nobody to ring for yet — look again next pass (quietly after the first time)
            if edge {
                eprintln!("[callwatch] call in {id} has only the recorder in it; waiting for a person");
            }
            state().lock().unwrap().recorder_alone.insert(id);
            continue;
        }
        // a person is in it: this pass is the call's real start, whatever we decide below
        state().lock().unwrap().recorder_alone.remove(&id);
        let is_dm = r.is_direct().await.unwrap_or(false);
        let mode = crate::notify::effective_mode(client, &r, is_dm).await;
        if matches!(mode, matrix_sdk_base::notification_settings::RoomNotificationMode::Mute) {
            continue;
        }
        {
            let now = Instant::now();
            let mut st = state().lock().unwrap();
            let last = st.announced.get(&id).copied();
            if !should_announce(last, now, &participants, is_dm, recorder.as_deref()) {
                eprintln!("[callwatch] call in {id} not announced (alerted {:?} ago, participants {participants:?})", last.map(|t| now.saturating_duration_since(t)));
                continue;
            }
            st.announced.retain(|_, t| now.saturating_duration_since(*t) < REANNOUNCE);
            st.announced.insert(id.clone(), now);
        }
        let room_name = match r.display_name().await {
            Ok(d) => d.to_string(),
            Err(_) => r.name().unwrap_or_else(|| id.clone()),
        };
        let mut who = String::new();
        if let Some(uid) = participants.iter().find(|p| Some(p.as_str()) != recorder.as_deref()).cloned() {
            who = uid.clone();
            if let Ok(u) = matrix_sdk::ruma::UserId::parse(&uid) {
                if let Ok(Some(m)) = r.get_member_no_sync(&u).await {
                    if let Some(n) = m.display_name() {
                        who = n.to_string();
                    }
                }
            }
        }
        eprintln!("[callwatch] incoming call in {room_name} ({id}) started by {who:?}");
        edges.push(Edge { room_id: id, room_name, who, started: true });
    }
    if crate::session::generation() != gen {
        return Vec::new(); // signed out while resolving names: ring nothing for the old session
    }
    edges
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The recorder a server announces (a sample account; room.rs recorder_mxid at run time).
    const REC: &str = "@recorder:example.com";

    fn p(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_flapping_call_alerts_once_per_window() {
        let t0 = Instant::now();
        let who = p(&["@tester1:example.com"]);
        assert!(should_announce(None, t0, &who, false, Some(REC)), "the first start alerts");
        // The 2026-09-23 test rig restarted its call every ~3.3 min.
        assert!(!should_announce(Some(t0), t0 + Duration::from_secs(200), &who, false, Some(REC)));
        assert!(!should_announce(Some(t0), t0 + REANNOUNCE - Duration::from_secs(1), &who, false, Some(REC)));
        assert!(should_announce(Some(t0), t0 + REANNOUNCE, &who, false, Some(REC)), "a call after the window alerts again");
    }

    #[test]
    fn a_dm_callback_always_rings() {
        let t0 = Instant::now();
        let who = p(&["@dale:example.com"]);
        // Missed / dropped, then they call again two minutes later: that one must ring too.
        assert!(should_announce(Some(t0), t0 + Duration::from_secs(120), &who, true, Some(REC)));
        // The recorder alone still never rings, DM or not.
        assert!(!should_announce(None, t0, &p(&[REC]), true, Some(REC)));
    }

    /// REACH A5.7 — the web mirrors this name in its callwatch tests.
    #[test]
    fn dm_ring_gap() {
        let t0 = Instant::now();
        let who = p(&["@dale:example.com"]);
        assert!(should_announce(None, t0, &who, true, Some(REC)), "ring at t0");
        assert!(!should_announce(Some(t0), t0 + Duration::from_secs(30), &who, true, Some(REC)), "a reconnect at t0+30 s is silent");
        assert!(should_announce(Some(t0), t0 + Duration::from_secs(60), &who, true, Some(REC)), "at t0+60 s it rings");
        assert!(!should_announce(Some(t0), t0 + Duration::from_secs(60), &who, false, Some(REC)), "group rooms keep the 10-minute window");
    }

    #[test]
    fn a_person_joining_a_call_the_recorder_started_alone_is_the_incoming_edge() {
        // pass 1: the call appears with only the recorder -> Rising, then parked as recorder-alone
        assert_eq!(classify(Some(false), true, false), Pass::Rising);
        assert!(is_recorder_only(&p(&[REC]), Some(REC)));
        // pass 2+: still live, still parked -> Recheck (a plain "still active" pass would be Nothing)
        assert_eq!(classify(Some(true), true, true), Pass::Recheck);
        assert_eq!(classify(Some(true), true, false), Pass::Nothing);
        // a person arrives: not recorder-only any more -> the normal decision rings it
        let both = p(&[REC, "@dale:example.com"]);
        assert!(!is_recorder_only(&both, Some(REC)));
        assert!(should_announce(None, Instant::now(), &both, false, Some(REC)));
        // the call ends -> Falling (the parked flag is dropped there)
        assert_eq!(classify(Some(true), false, true), Pass::Falling);
        // an empty participant list is not "recorder only" (membership not synced yet)
        assert!(!is_recorder_only(&[], Some(REC)));
        // the seeding pass / a first sight after it
        assert_eq!(classify(None, true, false), Pass::Rising);
        assert_eq!(classify(None, false, false), Pass::Nothing);
    }

    #[test]
    fn with_no_recorder_announced_nothing_is_filtered() {
        // a server without Memories names no recorder: the same account is just a participant
        let t0 = Instant::now();
        assert!(!is_recorder_only(&p(&[REC]), None));
        assert!(should_announce(None, t0, &p(&[REC]), false, None));
        assert!(should_announce(None, t0, &p(&[REC]), true, None));
    }

    #[test]
    fn a_recorder_only_call_never_alerts() {
        let t0 = Instant::now();
        assert!(!should_announce(None, t0, &p(&[REC]), false, Some(REC)));
        assert!(should_announce(None, t0, &p(&[REC, "@dale:example.com"]), false, Some(REC)));
        // Nothing visible yet (membership not synced): keep the old behaviour and alert.
        assert!(should_announce(None, t0, &[], false, Some(REC)));
    }
}
