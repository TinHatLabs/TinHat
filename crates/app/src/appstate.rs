//! Shared read/write helpers for Tin Hat custom room-STATE events
//! (`dev.tinhat.activity` / `.userstatus` / `.watchparty*`, `dev.tinhat.soundboard`).
//!
//! Raw JSON access + best-effort power-level lowering. The Tin Hat web fork lowers
//! these event types' required PL to 0 so any member can use the feature; on the live
//! server an admin already did that via the web app, so the native side just ATTEMPTS
//! writes and tolerates `M_FORBIDDEN`. Factoring this here keeps every feature
//! controller (activity/status/watch-party/soundboard) off its own copy of the logic.

use matrix_sdk::deserialized_responses::RawAnySyncOrStrippedState;
use matrix_sdk::ruma::events::StateEventType;
use matrix_sdk::Room;
use serde_json::Value;

/// The store returns a Sync/Stripped enum; reach the inner `Raw` and deserialize the
/// whole event (type/state_key/sender/content) as generic JSON.
pub(crate) fn raw_to_value(raw: &RawAnySyncOrStrippedState) -> Option<Value> {
    match raw {
        RawAnySyncOrStrippedState::Sync(r) => r.deserialize_as::<Value>().ok(),
        RawAnySyncOrStrippedState::Stripped(r) => r.deserialize_as::<Value>().ok(),
    }
}

/// Current epoch milliseconds (the `ts` / `expiresAt` fields the fork writes).
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The `content` object of one custom state event (by type + state_key), or `None`
/// if absent. An empty `{}` content (the fork's "cleared" marker) is returned as-is —
/// callers decide what counts as cleared.
pub async fn get_state(room: &Room, event_type: &str, state_key: &str) -> Option<Value> {
    let raw = room
        .get_state_event(StateEventType::from(event_type), state_key)
        .await
        .ok()??;
    let v = raw_to_value(&raw)?;
    v.get("content").cloned()
}

/// The sender of one custom state event (by type + state_key), or `None` if absent — 11.3 QOL: the Media Party's
/// "{name} paused" OSD needs who wrote the playback clock (read-only; nothing is ever written with it).
pub async fn get_state_sender(room: &Room, event_type: &str, state_key: &str) -> Option<String> {
    let raw = room
        .get_state_event(StateEventType::from(event_type), state_key)
        .await
        .ok()??;
    let v = raw_to_value(&raw)?;
    v.get("sender").and_then(|s| s.as_str()).map(str::to_string)
}

/// All state events of a type as `(state_key, sender, content)`.
pub async fn get_state_all(room: &Room, event_type: &str) -> Vec<(String, String, Value)> {
    let mut out = Vec::new();
    let Ok(raws) = room.get_state_events(StateEventType::from(event_type)).await else {
        return out;
    };
    for raw in raws {
        let Some(v) = raw_to_value(&raw) else { continue };
        let sk = v.get("state_key").and_then(|s| s.as_str()).unwrap_or_default().to_string();
        let sender = v.get("sender").and_then(|s| s.as_str()).unwrap_or_default().to_string();
        if let Some(content) = v.get("content").cloned() {
            out.push((sk, sender, content));
        }
    }
    out
}

/// Write a raw custom state event's content.
pub async fn send_state(
    room: &Room,
    event_type: &str,
    state_key: &str,
    content: Value,
) -> matrix_sdk::Result<()> {
    room.send_state_event_raw(event_type, state_key, content).await?;
    Ok(())
}

/// Best-effort: lower the required power level of each custom `event_type` to 0 so any
/// member can use it. Only an admin (who can edit `m.room.power_levels`) succeeds;
/// `M_FORBIDDEN` for everyone else is swallowed (the write that follows is attempted
/// regardless — on the live deployment the PL is already 0).
pub async fn ensure_writable(room: &Room, event_types: &[&str]) {
    // RBAC (docs/RBAC.md §4): once the room's container has roles active, the reconciler owns
    // every `events[<type>]` it derives from the roles (`dev.tinhat.soundboard`,
    // `dev.tinhat.record`, …). Lowering those to 0 here would fight it on every room
    // open — skip them; the rest of the member-state template is still applied.
    let managed = crate::roles_model::managed_event_types();
    let mut event_types: Vec<&str> = event_types.to_vec();
    if event_types.iter().any(|t| managed.contains(t)) {
        let container = crate::roles::container_of(room.room_id().as_str()).await;
        if crate::roles::load_state(&container).await.active() {
            event_types.retain(|t| !managed.contains(t));
        }
    }
    if event_types.is_empty() {
        return;
    }
    let Some(mut pl) = get_state(room, "m.room.power_levels", "").await else {
        return;
    };
    let all_zero = event_types.iter().all(|t| {
        pl.get("events").and_then(|e| e.get(*t)).and_then(|v| v.as_i64()) == Some(0)
    });
    if all_zero {
        return;
    }
    // Only an admin can edit `m.room.power_levels`. A standard (PL-0) member would just get a
    // guaranteed 403 + a noisy error log on every attempt, so skip the write silently for
    // them — they rely on an admin having provisioned the room (see `provision_room`).
    if let Ok(typed) = room.power_levels().await {
        if !typed.user_can_send_state(room.own_user_id(), StateEventType::RoomPowerLevels) {
            return;
        }
    }
    if !pl.get("events").map(Value::is_object).unwrap_or(false) {
        pl["events"] = serde_json::json!({});
    }
    if let Some(ev) = pl.get_mut("events").and_then(Value::as_object_mut) {
        for t in &event_types {
            ev.insert((*t).to_string(), Value::from(0));
        }
    }
    let _ = room.send_state_event_raw("m.room.power_levels", "", pl).await;
}

/// Every custom room-STATE event type Tin Hat writes that a standard (power-level 0)
/// member must be able to send. Message events (`m.reaction`, chat, `dev.tinhat.game.code`,
/// `io.element.call.encryption_keys`) are already PL-0 via `events_default` and are
/// intentionally absent. Keep this in sync when a new state-backed feature is added.
pub const TINHAT_MEMBER_STATE_EVENTS: &[&str] = &[
    "dev.tinhat.afk",                 // BRB / away presence (afk_ctl)
    "dev.tinhat.userstatus",          // "looking to play" status (status_ctl)
    "dev.tinhat.activity",            // activity presence (activity)
    "dev.tinhat.watchparty",          // watch-party session (watchparty)
    "dev.tinhat.watchparty.playback", // watch-party play/pause/seek (watchparty)
    "dev.tinhat.watchparty.viewer",   // watch-party per-viewer state (watchparty)
    "dev.tinhat.mediaparty.history",  // Media Party 11.4: the room's last 50 plays (CONTRACT §2), the advancer writes it
    "dev.tinhat.soundboard",          // shared room soundboard (soundboard)
    "dev.tinhat.clip",                // DVR clip bookmarks — any member may clip (0.8.8)
    "dev.tinhat.record",              // start/stop call recording — any member (0.8.11, a deliberate choice)
    // Squad calendar (0.8.26): events + per-user RSVP (state_key = own user id) + game rules.
    // Written in the SPACE room (or an orphan room); any member schedules and answers.
    "dev.tinhat.calendar.event",
    "dev.tinhat.calendar.rsvp",
    "dev.tinhat.calendar.game",
    // Game Reel (10.1, TH-REEL-11): prefs are keyed to the sender's own mxid (Matrix enforces
    // that), and "anyone present may spin" means a member writes the spin record. Without these
    // a fresh space's first game night was five people getting M_FORBIDDEN on Save.
    "dev.tinhat.games.prefs",
    "dev.tinhat.games.spin",
    // widgets W1: the space's Sticky note — the friend group's (with roles active the reconciler owns it:
    // `edit_space_note`, RBAC §2)
    "dev.tinhat.note",
    // Call membership. A room whose state_default is 50 (the Matrix default for a
    // hand-made private room) silently blocks members from publishing these, and the
    // symptom is not "you cannot call" — it is peers reporting "can't key us" mid-call,
    // because the membership that carries the device list never lands. Both spellings:
    // this client writes the msc3401 type, Element Call and newer clients write m.call.
    "org.matrix.msc3401.call.member",
    "m.call.member",
];

/// Call-recording marker: `{active: true, by: "@user:server"}` while Memories
/// recording is on; empty content = off. Since 0.8.11 it IS in
/// TINHAT_MEMBER_STATE_EVENTS (a deliberate choice: members in general get access) — any member may
/// start/stop; the visible 🔴 REC cue + per-person "Never record me" opt-out carry
/// the consent story. (0.8.7–0.8.10 gated this at moderator level.)
pub const RECORD_TYPE: &str = "dev.tinhat.record";

/// Clip bookmark (0.8.8): state_key = the epoch-ms stamp (unique per clip), content
/// `{at: <ms>}`. State (not a message) on purpose — state is unencrypted, so the
/// Memories player can list every clip with one state read, no crypto plumbing.
pub const CLIP_TYPE: &str = "dev.tinhat.clip";

/// Some(content) while recording is enabled for this room.
pub async fn record_enabled(room: &Room) -> Option<Value> {
    let v = get_state(room, RECORD_TYPE, "").await?;
    if v.get("active").and_then(|a| a.as_bool()) == Some(true) {
        Some(v)
    } else {
        None
    }
}

/// Admin-only, idempotent room provisioning: ensure EVERY Tin Hat member-state event
/// type is sendable by power-level-0 members, in a single `m.room.power_levels` write. Call
/// this on room open — it is a no-op for non-admins and for already-provisioned rooms. This
/// is what makes all the "extras" (BRB, status, activity, watch party, soundboard) usable by
/// standard members instead of only the room admin.
pub async fn provision_room(room: &Room) {
    ensure_writable(room, TINHAT_MEMBER_STATE_EVENTS).await;
}

/// Provision a room we JUST created.
///
/// [`provision_room`] reads `m.room.power_levels` from the LOCAL store, and a room that
/// was created a moment ago is not in it yet — the background sync has not delivered its
/// state. Calling provision immediately therefore does NOTHING, silently, and the new
/// room ships with the very state_default=50 trap this template exists to prevent
/// (members cannot publish call membership → peers report "can't key us" mid-call).
/// Caught by the Phase-2 gate, which asserted the permissions rather than the API call.
///
/// So wait for the state to land first. Deliberately a poll rather than a `sync_once`:
/// the app runs exactly ONE sync loop on purpose (a second one contends on the
/// crypto-store lock), so this waits for that loop to catch up instead of racing it.
pub async fn provision_new_room(room: &Room) {
    for attempt in 0..20 {
        if get_state(room, "m.room.power_levels", "").await.is_some() {
            provision_room(room).await;
            return;
        }
        if attempt == 0 {
            eprintln!("[appstate] waiting for {} power levels to sync…", room.room_id());
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    eprintln!(
        "[appstate] {} never synced power levels; members may lack call permission",
        room.room_id()
    );
}
