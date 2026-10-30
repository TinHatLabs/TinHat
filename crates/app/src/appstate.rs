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

type Sets = std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>>;
/// `generation|user/device|room` → the types already lowered this session: the "one-time" of the must-fix round.
static WRITABLE_DONE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, Vec<String>>>> = std::sync::OnceLock::new();
/// Rooms on another server already logged as left alone (one line per room per session).
static FOREIGN_NOTED: Sets = std::sync::OnceLock::new();
/// Rooms provisioned on open this session (once per room).
static OPENED_ONCE: Sets = std::sync::OnceLock::new();
/// One local power-level write per room at a time ([`power_level_lock`]).
static PL_LOCKS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<tokio::sync::Mutex<()>>>>> =
    std::sync::OnceLock::new();

/// true when `key` was new (and is now in the set)
fn set_insert(set: &Sets, key: String) -> bool {
    set.get_or_init(Default::default).lock().map(|mut s| s.insert(key)).unwrap_or(false)
}

fn session_key(room: &Room) -> String {
    let device = room.client().device_id().map(|d| d.to_string()).unwrap_or_default();
    format!("{}|{}/{device}|{}", crate::session::generation(), room.own_user_id(), room.room_id())
}

/// The room's power-level lock: every local `m.room.power_levels` writer (ensure_writable, the roles reconciler, a
/// moderator's change) holds it across its read and its write, so none undoes another (the web's withPowerLevelLock).
pub fn power_level_lock(room_id: &str) -> std::sync::Arc<tokio::sync::Mutex<()>> {
    PL_LOCKS
        .get_or_init(Default::default)
        .lock()
        .map(|mut m| m.entry(room_id.to_string()).or_default().clone())
        .unwrap_or_default()
}

/// Per room: the id of our latest power-level write and how long we wait for it. Until the store shows that event (or
/// the wait runs out), the store may be missing it. Set only after the server accepted the write (the web's pendingEcho).
static PENDING_ECHO: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, (String, std::time::Instant)>>> =
    std::sync::OnceLock::new();
const ECHO_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// The room's power-level event as (event id, content), read once (one snapshot).
async fn pl_event(room: &Room) -> Option<(String, Value)> {
    let raw = room.get_state_event(StateEventType::RoomPowerLevels, "").await.ok()??;
    let v = raw_to_value(&raw)?;
    let id = v.get("event_id").and_then(Value::as_str).unwrap_or_default().to_string();
    Some((id, v.get("content")?.clone()))
}

fn pending_get(rid: &str) -> Option<(String, std::time::Instant)> {
    PENDING_ECHO.get_or_init(Default::default).lock().ok().and_then(|m| m.get(rid).cloned())
}
fn pending_clear(rid: &str) {
    if let Ok(mut m) = PENDING_ECHO.get_or_init(Default::default).lock() {
        m.remove(rid);
    }
}

/// Write `m.room.power_levels` (under [`power_level_lock`]) and wait ≤ 10 s until the store shows this very event, so
/// the next writer reads it rather than the state before it. A rejected write marks nothing. Every local power-level
/// writer uses this (the web's sendPowerLevels).
pub async fn send_power_levels(room: &Room, content: Value) -> matrix_sdk::Result<()> {
    let rid = room.room_id().to_string();
    let id = room.send_state_event_raw("m.room.power_levels", "", content).await?.event_id.to_string();
    let until = std::time::Instant::now() + ECHO_WAIT;
    if let Ok(mut m) = PENDING_ECHO.get_or_init(Default::default).lock() {
        m.insert(rid.clone(), (id.clone(), until));
    }
    loop {
        let now = pl_event(room).await.map(|(i, _)| i).unwrap_or_default();
        if now == id {
            pending_clear(&rid);
            break;
        }
        if std::time::Instant::now() >= until {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    Ok(())
}

/// The room's power levels as the next write must start from: the store's — or None while our latest write isn't in
/// it yet (writing now could undo it; the caller skips this time). After the wait runs out the store is trusted. The
/// web's powerLevelsForWrite.
pub async fn power_levels_for_write(room: &Room) -> Option<Value> {
    let (id, content) = pl_event(room).await?;
    let rid = room.room_id().to_string();
    if let Some((pending, until)) = pending_get(&rid) {
        if id != pending && std::time::Instant::now() < until {
            return None;
        }
        pending_clear(&rid);
    }
    Some(content)
}

/// Best-effort: lower the required power level of each Tin Hat type to 0 so any member can use it — the one write
/// `foreign_room::pl_write_plan` (shared vectors with the web) allows. Called when a Tin Hat feature is used, on
/// joining a call, on room open ([`provision_on_open`]) and for a room we just made. The must-fix round
/// (MATRIX-FRONTEND-DESIGN.md B0/B5 rule 1; the web's ensureWritable is the same):
/// - a room on another server is never written (its power levels stay as its owners set them; the feature works only
///   as far as they allow) — logged once per room;
/// - only dev.tinhat.* types and call membership, only while above 0, only when I may edit power levels (an
///   unreadable power-level state = no);
/// - each type lowered once per room per session, logged (a refusal isn't remembered: the next use asks again);
/// - one write per room at a time, under the lock every local power-level writer shares, each waiting for its own
///   echo ([`send_power_levels`]), so none starts from state an earlier one replaced.
pub async fn ensure_writable(room: &Room, event_types: &[&str]) {
    let rid = room.room_id().to_string();
    let generation = crate::session::generation();
    if !crate::foreign_room::managed_here(room).await {
        if set_insert(&FOREIGN_NOTED, rid.clone()) {
            eprintln!("[appstate] power levels left alone in {rid}: it lives on {}", crate::foreign_room::origin_here(room).await);
        }
        return;
    }
    let lock = power_level_lock(&rid);
    let _held = lock.lock().await;
    if generation != crate::session::generation() {
        return; // queued for a session that has ended
    }
    // RBAC (docs/RBAC.md §4): once the room's container has roles active, the reconciler owns
    // every `events[<type>]` it derives from the roles (`dev.tinhat.soundboard`,
    // `dev.tinhat.record`, …). Lowering those to 0 here would fight it — skip them; the rest
    // of the member-state template is still applied.
    let managed = crate::roles_model::managed_event_types();
    let mut event_types: Vec<&str> = event_types.to_vec();
    if event_types.iter().any(|t| managed.contains(t)) {
        let container = crate::roles::container_of(room.room_id().as_str()).await;
        if crate::roles::load_state(&container).await.active() {
            event_types.retain(|t| !managed.contains(t));
        }
    }
    let create = crate::foreign_room::create_event(room).await.unwrap_or(Value::Null);
    let pl = power_levels_for_write(room).await;
    // Only an admin can edit `m.room.power_levels`; a standard (PL-0) member would just get a guaranteed 403.
    let can_edit = match room.power_levels().await {
        Ok(typed) => typed.user_can_send_state(room.own_user_id(), StateEventType::RoomPowerLevels),
        Err(_) => false,
    };
    let key = session_key(room);
    let done: Vec<String> =
        WRITABLE_DONE.get_or_init(Default::default).lock().ok().and_then(|m| m.get(&key).cloned()).unwrap_or_default();
    let own = room.own_user_id().server_name().to_string();
    let plan = crate::foreign_room::pl_write_plan(&create, &own, pl.as_ref(), &event_types, can_edit, &done);
    let (true, Some(content)) = (plan.reason == "write", plan.content) else { return };
    eprintln!("[appstate] power levels: letting members send {} in {rid}", plan.types.join(", "));
    match send_power_levels(room, content).await {
        Ok(()) => {
            if let Ok(mut m) = WRITABLE_DONE.get_or_init(Default::default).lock() {
                m.entry(key).or_default().extend(plan.types.iter().cloned());
            }
        }
        // a refusal isn't remembered: the next use asks again (the web's the same)
        Err(e) => eprintln!("[appstate] ensure_writable {rid}: {e}"),
    }
}

/// Room open (room.rs; the web's ConversationView): provision once per room per session — my own server's rooms only
/// ([`ensure_writable`] never writes a room on another server). What keeps PL-0 members able to call / use the extras
/// in a room an admin opens, including types added in a later release.
pub async fn provision_on_open(room: &Room) {
    if set_insert(&OPENED_ONCE, session_key(room)) {
        provision_room(room).await;
    }
}

/// Call membership, both spellings (the web's CALL_MEMBER_STATE_EVENTS): what joining a call asks
/// [`ensure_writable`] for.
pub const CALL_MEMBER_STATE_EVENTS: &[&str] = &["org.matrix.msc3401.call.member", "m.call.member"];

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
/// type is sendable by power-level-0 members, in a single `m.room.power_levels` write (my own
/// server's rooms only). For a room we just made ([`provision_new_room`]) and, once per room per
/// session, on room open ([`provision_on_open`]).
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
