//! MatrixRTC (MSC3401) helpers: detect an active call in a room, resolve the
//! LiveKit focus the web/phone clients use, and publish/withdraw our own
//! membership so Element Call shows us as a participant.
//!
//! All verified against the live deployment (2026-06-18): every member advertises
//! `foci_preferred[0] = { type: livekit, livekit_service_url: <broker>,
//! livekit_alias: <room_id> }`, and the JWT broker maps that alias verbatim to the
//! LiveKit room. So `alias == room_id` here; we keep `room_id` as a safe fallback.

use matrix_sdk::ruma::events::call::member::CallMemberEventContent;
use matrix_sdk::{Room, RoomState};

/// Element Call membership state-event type (MSC3401/MSC4143 session format).
pub(crate) const MEMBER_EVENT_TYPE: &str = "org.matrix.msc3401.call.member";
/// Default membership lifetime (ms) — must refresh before this elapses (EC = 4h).
const MEMBER_EXPIRES_MS: u64 = 14_400_000;

/// Wall-clock ms (the clock membership liveness and the key planner run on).
pub(crate) fn now_ms_pub() -> u64 {
    now_ms()
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Where to send media for a room's active MatrixRTC call.
#[derive(Clone, Debug)]
pub struct CallFocus {
    /// lk-jwt-service broker base, e.g. `https://<server>/livekit/jwt`.
    /// POST `{service_url}/sfu/get` to mint a publish token.
    pub service_url: String,
    /// LiveKit room name to request — the Matrix room id verbatim on this server.
    pub alias: String,
}

/// Cheap, in-memory: is there an active room call right now? (Drives the UI badge.)
pub fn is_call_active(room: &Room) -> bool {
    room.has_active_room_call()
}

/// The room's raw `org.matrix.msc3401.call.member` state events (local store), as the pure rules read them.
async fn member_events(room: &Room) -> matrix_sdk::Result<Vec<serde_json::Value>> {
    let raws = room.get_state_events_static::<CallMemberEventContent>().await?;
    Ok(raws.iter().filter_map(|r| serde_json::to_value(r).ok()).collect())
}

/// Calls round M1 (DESIGN §2): which call service a join in `room` uses, and whether the user must be asked first.
/// The running call's service is the oldest live membership's (by created_ts, sender, device: a total order, the
/// same pick as the web and the phones); our own services are the server's well-known focus and
/// `<homeserver>/livekit/jwt`; the allow-list is account data `dev.tinhat.call_hosts`. No call running = start on
/// our own service. Nothing here contacts the call service.
pub async fn focus_decision_for(client: &matrix_sdk::Client, room: &Room) -> matrix_sdk::Result<crate::callrules::FocusDecision> {
    let events = member_events(room).await?;
    let (start, own) = own_foci(client).await;
    let allow = call_hosts(client).await;
    Ok(crate::callrules::focus_decision(room.room_id().as_str(), now_ms(), &events, &own, &start, &allow))
}

/// Our own call services `(start, set)` (callrules::own_foci over the server's well-known focus and the homeserver).
pub async fn own_foci(client: &matrix_sdk::Client) -> (String, Vec<String>) {
    let origin = crate::session::origin_of(client.homeserver().as_str());
    let wk_url = focus_service_url(client).await; // the well-known's focus, else the homeserver fallback
    let wk = serde_json::json!({ "org.matrix.msc4143.rtc_foci": [{ "type": "livekit", "livekit_service_url": wk_url }] });
    crate::callrules::own_foci(&wk, &origin)
}

/// Account data type of the call hosts the user chose "Always trust" for (`{"v":1,"allow":["host[:port]", …]}`),
/// synced across the user's devices and read by web, desktop and phone alike.
pub const CALL_HOSTS_TYPE: &str = "dev.tinhat.call_hosts";

async fn call_hosts_content(client: &matrix_sdk::Client) -> serde_json::Value {
    use matrix_sdk::ruma::events::GlobalAccountDataEventType;
    let raw = client.account().account_data_raw(GlobalAccountDataEventType::from(CALL_HOSTS_TYPE)).await.ok().flatten();
    raw.and_then(|r| serde_json::from_str::<serde_json::Value>(r.json().get()).ok()).unwrap_or(serde_json::Value::Null)
}

/// The hosts the user trusts for calls (empty when none or unreadable).
pub async fn call_hosts(client: &matrix_sdk::Client) -> Vec<String> {
    crate::callrules::allow_list_from(&call_hosts_content(client).await)
}

/// Add (`trust`) or remove a host from the trusted call hosts (account data, read-modify-write).
pub async fn set_call_host(client: &matrix_sdk::Client, host: &str, trust: bool) -> Result<(), String> {
    use matrix_sdk::ruma::events::GlobalAccountDataEventType;
    let next = crate::callrules::allow_list_with(&call_hosts_content(client).await, host, trust);
    let raw_value = serde_json::value::to_raw_value(&next).map_err(|e| e.to_string())?;
    let raw: matrix_sdk::ruma::serde::Raw<matrix_sdk::ruma::events::AnyGlobalAccountDataEventContent> =
        matrix_sdk::ruma::serde::Raw::from_json(raw_value);
    client
        .account()
        .set_account_data_raw(GlobalAccountDataEventType::from(CALL_HOSTS_TYPE), raw)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// The focus of the oldest live session that names one (callrules::running_session: created_ts, sender, device); an
/// empty alias means the room id. No host policy: the join path uses [`focus_decision_for`].
pub(crate) fn oldest_focus(sessions: &[crate::keyrules::LiveSession], room_id: &str) -> Option<CallFocus> {
    let (service_url, alias) = crate::callrules::running_session(sessions)?.focus.clone()?;
    let alias = if alias.is_empty() { room_id.to_string() } else { alias };
    Some(CallFocus { service_url, alias })
}

/// User-ids currently joined to the room's MatrixRTC call (active room-scoped
/// `m.call.member` memberships). Drives the Discord-style "who's in the call" avatar
/// bubbles under a room in the nav. Deduped; reads the local store only.
pub async fn call_participants(room: &Room) -> Vec<String> {
    // 0.8.23: read BOTH spellings raw. The typed ruma fetch resolved to the
    // msc3401 type only, so web / Element X peers (who write `m.call.member`) never showed
    // in the strip; and the typed path applied no ghost filter, so unclean exits lingered.
    let mut events: Vec<serde_json::Value> = Vec::new();
    for t in [MEMBER_EVENT_TYPE, "m.call.member", "m.rtc.member"] {
        let Ok(raws) = room
            .get_state_events(matrix_sdk::ruma::events::StateEventType::from(t))
            .await
        else {
            continue;
        };
        for raw in &raws {
            if let Some(v) = crate::appstate::raw_to_value(raw) {
                events.push(v);
            }
        }
    }
    call_member_users_from(&events, now_ms())
}

/// The users with a LIVE call membership among raw membership state events (either
/// spelling, session or legacy `memberships[]` format). Tombstones (empty content) and
/// expired memberships (`created_ts`, else `origin_server_ts`, + `expires`) are skipped —
/// the same liveness rule as [`call_member_devices`]. Deduped, first-seen order.
pub(crate) fn call_member_users_from(events: &[serde_json::Value], now: u64) -> Vec<String> {
    let mut users: Vec<String> = Vec::new();
    for v in events {
        let sender = v.get("sender").and_then(|s| s.as_str()).unwrap_or("");
        if sender.is_empty() {
            continue;
        }
        let Some(content) = v.get("content").and_then(|c| c.as_object()) else { continue };
        let origin_ts = v.get("origin_server_ts").and_then(|t| t.as_u64());
        let live_at = |obj: &serde_json::Map<String, serde_json::Value>| -> bool {
            let exp = obj.get("expires").and_then(|e| e.as_u64()).unwrap_or(MEMBER_EXPIRES_MS);
            let created = obj.get("created_ts").and_then(|t| t.as_u64()).or(origin_ts);
            // No usable timestamp at all: keep (better a stray bubble than a hidden peer).
            created.map(|c| now <= c.saturating_add(exp)).unwrap_or(true)
        };
        let live = if let Some(ms) = content.get("memberships").and_then(|m| m.as_array()) {
            ms.iter().filter_map(|m| m.as_object()).any(live_at) // legacy array format
        } else if content.is_empty() {
            false // tombstone = left
        } else {
            live_at(content)
        };
        if live && !users.iter().any(|u| u == sender) {
            users.push(sender.to_string());
        }
    }
    users
}

/// `(user_id, device_id)` for every LIVE MatrixRTC call membership in `room` (room-scoped
/// `m.call` sessions). NOT a key-recipient list (no joined / well-formed / ownership check — 12.0.5
/// moved key sends to [`key_recipient_devices`]); used for head counts. Skips tombstoned memberships
/// (empty content = a member who left) and reads the local store only. Deduped.
///
/// EXPIRED memberships are skipped too: an unclean exit (SIGKILL, crash, dead battery)
/// withdraws nothing, so its membership lingers until the 4h expiry — and every key
/// announce would otherwise resolve that dead device, forcing a /keys/query round-trip
/// per ghost, on every announce, for the whole call. Expiry = `created_ts` (falling back
/// to the event's `origin_server_ts`) + `expires`, defaulting to [`MEMBER_EXPIRES_MS`].
pub async fn call_member_devices(room: &Room) -> Vec<(String, String)> {
    let Ok(raws) = room.get_state_events_static::<CallMemberEventContent>().await else {
        return Vec::new();
    };
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let mut out: Vec<(String, String)> = Vec::new();
    for raw in &raws {
        // The raw state event serializes back to its on-wire JSON (sender/content/state_key).
        let Ok(v) = serde_json::to_value(raw) else {
            continue;
        };
        let sender = v.get("sender").and_then(|s| s.as_str()).unwrap_or("");
        let content = v.get("content");
        // Empty content = a member who left (tombstone) — skip.
        let tombstone = content
            .and_then(|c| c.as_object())
            .map(|o| o.is_empty())
            .unwrap_or(true);
        if sender.is_empty() || tombstone {
            continue;
        }
        let device = content
            .and_then(|c| c.get("device_id"))
            .and_then(|d| d.as_str())
            .unwrap_or("");
        if device.is_empty() {
            continue;
        }
        // Drop ghosts left behind by unclean exits (see the doc comment). A membership
        // with no usable timestamp at all is kept — better a stray announce than a
        // silenced peer.
        let created = content
            .and_then(|c| c.get("created_ts"))
            .and_then(|t| t.as_u64())
            .or_else(|| v.get("origin_server_ts").and_then(|t| t.as_u64()));
        let expires = content
            .and_then(|c| c.get("expires"))
            .and_then(|t| t.as_u64())
            .unwrap_or(MEMBER_EXPIRES_MS);
        if let Some(created) = created {
            if now_ms > created.saturating_add(expires) {
                continue;
            }
        }
        if !out.iter().any(|(u, d)| u == sender && d == device) {
            out.push((sender.to_string(), device.to_string()));
        }
    }
    out
}

/// What the media-key recipient rule saw (calls round, recipient rule v2).
pub struct KeyRecipients {
    /// `user|device`, sorted (callrules::key_recipients_v2)
    pub ids: Vec<String>,
    /// a candidate's room membership isn't in the store yet (lazy-loaded member list): it gets no key now; load the
    /// members and look again (12.0.5 review item 3: unknown members never delay the known ones)
    pub unknown_members: bool,
    /// a device list couldn't be read: that user got nothing this round (fail closed) — look again in ~2 s
    pub retry_soon: bool,
}

/// Who may receive our media key right now (recipient rule v2, DESIGN §3.1): the web's rule (live, well-formed
/// session; the user JOINED to `room`; not our own device) + the device is in its user's device list when the crypto
/// store knows that list (a removed / logged-out device gets nothing) + it hasn't left the call server in this session
/// (`sfu_gone`). Local store reads only, no network: callers run it right before every send.
pub async fn key_recipients_now(room: &Room, own_user: &str, own_device: &str, sfu_gone: &[(String, u64)]) -> KeyRecipients {
    let sessions = live_sessions(room).await;
    let cands = candidates(&sessions, own_user, own_device);
    let mut ms: std::collections::HashMap<String, Option<String>> = std::collections::HashMap::new();
    let mut devices: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
    let client = room.client();
    let mut retry_soon = false;
    let tracked = match client.encryption().tracked_users().await {
        Ok(t) => Some(t),
        Err(e) => {
            eprintln!("[send-key-td] tracked users unreadable: {e} — no key sends this round");
            retry_soon = true;
            None
        }
    };
    let rev_at: std::collections::HashMap<String, u64> = cands.iter().map(|u| (u.clone(), device_rev(u))).collect();
    for u in &cands {
        ms.insert(u.clone(), membership_in(room, u).await);
        match tracked.as_ref() {
            // a failed read fails CLOSED: an empty list for every candidate this round, never a stale one
            None => {
                devices.insert(u.clone(), Vec::new());
            }
            Some(t) => match known_devices(&client, u, t).await {
                Ok(Some(list)) => {
                    devices.insert(u.clone(), list);
                }
                Ok(None) => {}
                Err(e) => {
                    eprintln!("[send-key-td] device list of {u} unreadable: {e} — not keyed this round");
                    devices.insert(u.clone(), Vec::new());
                    retry_soon = true;
                }
            },
        }
    }
    // a device update for a user while we read: that user's list is void this round (closed), read again in 2 s
    for u in &cands {
        if device_rev(u) != rev_at.get(u).copied().unwrap_or(0) {
            eprintln!("[send-key-td] device list of {u} changed during the read — not keyed this round");
            devices.insert(u.clone(), Vec::new());
            retry_soon = true;
        }
    }
    let unknown_members = ms.values().any(|m| m.is_none());
    let of = |u: &str| ms.get(u).cloned().flatten();
    KeyRecipients { ids: crate::callrules::key_recipients_v2(&sessions, &of, own_user, own_device, &devices, sfu_gone), unknown_members, retry_soon }
}

/// Load the room's member list when it isn't (the SDK dedups concurrent loads); true when it loaded something new.
pub async fn load_members(room: &Room) -> bool {
    if room.are_members_synced() {
        return false;
    }
    match room.sync_members().await {
        Ok(()) => true,
        Err(e) => {
            eprintln!("[send-key-td] member list of {} not loaded: {e}", room.room_id());
            false
        }
    }
}

/// `user_id`'s devices as the crypto store knows them (deleted ones left out). The list is known when the crypto store
/// tracks the user's devices — then an empty list is a real "no devices left" (the SQLite store drops deleted devices,
/// so the last device's removal leaves nothing behind); `None` = untracked, unknown, excludes no one (and the
/// announce's key query then fetches it).
/// Err = the crypto store couldn't be read (the caller fails closed).
async fn known_devices(client: &matrix_sdk::Client, user_id: &str, tracked: &std::collections::HashSet<matrix_sdk::ruma::OwnedUserId>) -> Result<Option<Vec<String>>, String> {
    let Ok(uid) = matrix_sdk::ruma::UserId::parse(user_id) else { return Ok(None) };
    let devs = client.encryption().get_user_devices(&uid).await.map_err(|e| e.to_string())?;
    let mut any = false;
    let mut out = Vec::new();
    for d in devs.devices() {
        any = true;
        if !d.is_deleted() {
            out.push(d.device_id().to_string());
        }
    }
    // tracked alone doesn't mean fetched (the first query may still be pending): an empty list is authoritative only
    // for a user whose devices this process has seen before (their last device was removed since)
    let mut seen = DEVICES_SEEN.lock().unwrap();
    if any {
        seen.insert(user_id.to_string());
    }
    Ok((any || (tracked.contains(&uid) && seen.contains(user_id))).then_some(out))
}

/// Per-user device-list revision, bumped by the crypto store's device updates (call_ctl's devices stream). A read whose
/// user's revision moved meanwhile is void for that user (fail closed, read again) — never a stale list.
static DEVICE_REV: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<String, u64>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

pub(crate) fn bump_device_revs(users: &[String]) {
    let mut m = DEVICE_REV.lock().unwrap();
    for u in users {
        *m.entry(u.clone()).or_insert(0) += 1;
    }
}

fn device_rev(u: &str) -> u64 {
    DEVICE_REV.lock().unwrap().get(u).copied().unwrap_or(0)
}

/// Users whose device list the crypto store has held at least one device for, this run (see [`known_devices`]).
static DEVICES_SEEN: std::sync::LazyLock<std::sync::Mutex<std::collections::HashSet<String>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashSet::new()));

/// The distinct senders whose membership decides the recipients: those of well-formed sessions other than our own
/// device (the only ones `key_recipients` asks about). Deduped through a set (12.0.5 review: no quadratic scan).
fn candidates(sessions: &[crate::keyrules::LiveSession], own_user: &str, own_device: &str) -> Vec<String> {
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut out: Vec<String> = Vec::new();
    for s in sessions {
        if s.well_formed && !s.device_id.is_empty() && !(s.sender == own_user && s.device_id == own_device) && seen.insert(s.sender.as_str()) {
            out.push(s.sender.clone());
        }
    }
    out
}

/// `user_id`'s room membership ("join", "leave", "ban", "invite", "knock"), `None` when the store doesn't know it.
async fn membership_in(room: &Room, user_id: &str) -> Option<String> {
    let uid = matrix_sdk::ruma::UserId::parse(user_id).ok()?;
    let m = room.get_member_no_sync(&uid).await.ok().flatten()?;
    Some(m.membership().as_str().to_string())
}

/// The room's live MatrixRTC sessions as the media-key rules see them (schema, state-key ownership,
/// liveness: [`crate::keyrules::live_sessions_from`], the web's `liveSessions`). Local store only.
pub async fn live_sessions(room: &Room) -> Vec<crate::keyrules::LiveSession> {
    let Ok(raws) = room.get_state_events_static::<CallMemberEventContent>().await else {
        return Vec::new();
    };
    let events: Vec<serde_json::Value> = raws.iter().filter_map(|r| serde_json::to_value(r).ok()).collect();
    crate::keyrules::live_sessions_from(&events, now_ms())
}

/// LIVE call members that are NOT Tin Hat native clients — browser Element Call,
/// Element X, or a native build older than 0.8.5 (which predates the marker and is
/// treated as a browser: the safe direction, since H.264 plays everywhere). Excludes
/// our own device. Same liveness rules as [`call_member_devices`]. Returned as
/// `user:device` strings, sorted and deduped, so callers can log and compare them.
pub async fn non_native_call_members(room: &Room) -> Vec<String> {
    let Ok(raws) = room.get_state_events_static::<CallMemberEventContent>().await else {
        return Vec::new();
    };
    let own_user = room.own_user_id().to_string();
    let own_device = room.client().device_id().map(|d| d.to_string()).unwrap_or_default();
    let now = now_ms();
    let mut out: Vec<String> = Vec::new();
    for raw in &raws {
        let Ok(v) = serde_json::to_value(raw) else { continue };
        let sender = v.get("sender").and_then(|s| s.as_str()).unwrap_or("");
        let Some(content) = v.get("content").and_then(|c| c.as_object()) else { continue };
        if sender.is_empty() || content.is_empty() {
            continue; // tombstone = left
        }
        let device = content.get("device_id").and_then(|d| d.as_str()).unwrap_or("");
        if device.is_empty() || (sender == own_user && device == own_device) {
            continue;
        }
        let created = content
            .get("created_ts")
            .and_then(|t| t.as_u64())
            .or_else(|| v.get("origin_server_ts").and_then(|t| t.as_u64()));
        let expires = content.get("expires").and_then(|t| t.as_u64()).unwrap_or(MEMBER_EXPIRES_MS);
        if let Some(created) = created {
            if now > created.saturating_add(expires) {
                continue; // ghost of an unclean exit
            }
        }
        let native = content
            .get("dev.tinhat.client")
            .and_then(|c| c.get("kind"))
            .and_then(|k| k.as_str())
            == Some("native");
        if !native {
            out.push(format!("{sender}:{device}"));
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Bare mxids of LIVE call members advertising `record_optout` ("Never record me") in
/// their `dev.tinhat.client` marker (0.8.7). ANY opted-out device excludes that
/// user's EVERY track — the conservative direction for a privacy promise. Same
/// liveness rules as [`call_member_devices`].
pub async fn record_optout_mxids(room: &Room) -> Vec<String> {
    let Ok(raws) = room.get_state_events_static::<CallMemberEventContent>().await else {
        return Vec::new();
    };
    let now = now_ms();
    let mut out: Vec<String> = Vec::new();
    for raw in &raws {
        let Ok(v) = serde_json::to_value(raw) else { continue };
        let sender = v.get("sender").and_then(|s| s.as_str()).unwrap_or("");
        let Some(content) = v.get("content").and_then(|c| c.as_object()) else { continue };
        if sender.is_empty() || content.is_empty() {
            continue; // tombstone = left
        }
        let created = content
            .get("created_ts")
            .and_then(|t| t.as_u64())
            .or_else(|| v.get("origin_server_ts").and_then(|t| t.as_u64()));
        let expires = content.get("expires").and_then(|t| t.as_u64()).unwrap_or(MEMBER_EXPIRES_MS);
        if let Some(created) = created {
            if now > created.saturating_add(expires) {
                continue; // ghost of an unclean exit
            }
        }
        let optout = content
            .get("dev.tinhat.client")
            .and_then(|c| c.get("record_optout"))
            .and_then(|k| k.as_bool())
            == Some(true);
        if optout && !out.iter().any(|u| u == sender) {
            out.push(sender.to_string());
        }
    }
    out
}

/// The call service when the user's server names none in its well-known: `<homeserver origin>/livekit/jwt`, where a
/// Tin Hat Server serves it. The app has no call service of its own (selfhost, 2026-10-21).
pub fn fallback_focus_url(origin: &str) -> String {
    format!("{}/livekit/jwt", origin.trim_end_matches('/'))
}

/// The call JWT service a `.well-known/matrix/client` names: its first `org.matrix.msc4143.rtc_foci` entry of type
/// livekit with a `livekit_service_url` (Tin Hat Web's `focusFromWellKnown` in calls/rtc.ts, the same cases tested).
pub fn focus_from_wellknown(wk: &serde_json::Value) -> Option<String> {
    wk.get("org.matrix.msc4143.rtc_foci")?.as_array()?.iter().find_map(|f| {
        if f.get("type").and_then(|t| t.as_str()) != Some("livekit") {
            return None;
        }
        f.get("livekit_service_url").and_then(|u| u.as_str()).filter(|u| !u.is_empty()).map(str::to_string)
    })
}

/// What the last read of a server's well-known said (server name, its call service), kept for the run as the web
/// keeps it for the page's life. A read that failed (no answer, not JSON) isn't kept: the next call tries again.
static SERVER_FOCUS: std::sync::Mutex<Option<(String, Option<String>)>> = std::sync::Mutex::new(None);

/// The call JWT service new calls use, and the connection checks probe: the one the user's server names (a Tin Hat
/// Server names its own), else `<homeserver origin>/livekit/jwt` (fallback_focus_url). Tin Hat Web's
/// `focusServiceUrl`. The offer store read the server's well-known at sign-in (server_offer.rs): its answer is the
/// truth for the run; a read of its own only when the store has none for this server.
pub async fn focus_service_url(client: &matrix_sdk::Client) -> String {
    let fallback = fallback_focus_url(&crate::session::origin_of(client.homeserver().as_str()));
    let Some(server) = client.user_id().map(|u| u.server_name().to_string()) else { return fallback };
    if let Some(known) = crate::server_offer::focus_url_for(&server) {
        return known.unwrap_or(fallback);
    }
    if let Some((s, url)) = SERVER_FOCUS.lock().unwrap().clone() {
        if s == server {
            return url.unwrap_or(fallback);
        }
    }
    let read = async {
        let resp = matrix_sdk::reqwest::Client::new()
            .get(format!("https://{server}/.well-known/matrix/client"))
            .timeout(std::time::Duration::from_secs(8))
            .send()
            .await
            .ok()?;
        if !resp.status().is_success() {
            return Some(None); // answered without a well-known: names none
        }
        let text = resp.text().await.ok()?;
        let wk = serde_json::from_str::<serde_json::Value>(&text).ok()?;
        Some(focus_from_wellknown(&wk))
    };
    match read.await {
        Some(url) => {
            *SERVER_FOCUS.lock().unwrap() = Some((server, url.clone()));
            url.unwrap_or(fallback)
        }
        None => fallback,
    }
}

/// The EC session-membership state key: `_@user:server_DEVICEID_m.call`.
fn membership_state_key(user_id: &str, device_id: &str) -> String {
    format!("_{user_id}_{device_id}_m.call")
}

/// Publish our membership in the **session format** Element Call's membership
/// manager parses (sent as a RAW state event so the JSON matches byte-for-byte —
/// peers pick to-device key recipients straight from these memberships, so an
/// unrecognized membership means we never receive anyone's media key). The room
/// must be joined; call again on a timer to refresh before [`MEMBER_EXPIRES_MS`].
pub async fn join_rtc_session(room: &Room, service_url: &str) -> matrix_sdk::Result<()> {
    if room.state() != RoomState::Joined {
        return Ok(());
    }
    let user_id = room.own_user_id().to_string();
    let Some(device_id) = room.client().device_id().map(|d| d.to_string()) else {
        return Ok(());
    };
    let room_id = room.room_id().to_string();
    let state_key = membership_state_key(&user_id, &device_id);
    // REFRESH must keep the ORIGINAL created_ts: matrix-js-sdk's RTCEncryptionManager
    // treats a membership whose created_ts changed as leave+rejoin and ROTATES EVERY
    // WEB PEER'S media key — with the 30-min refresh timer that meant a synchronized
    // key-rotation storm (and MissingKey flap on every native receiver) twice an hour
    // for the whole call (a field log: one peer's refresh pairs each followed by two
    // other peers' rotations at identical timestamps). Only a genuinely NEW
    // session (no live membership of ours in the room state) mints a fresh created_ts.
    let created_ts = live_own_membership_created_ts(room, &state_key).await.unwrap_or_else(now_ms);
    // Refresh semantics: `expires` is a DURATION FROM created_ts, so extending the
    // session means GROWING expires past (now - created), not re-sending the original
    // 4h (which would leave the membership already-expired deep into a long call —
    // the exact bug class fixed in the web fork's updateCallIntent on 2026-08-17).
    let expires_ms = now_ms().saturating_sub(created_ts).saturating_add(MEMBER_EXPIRES_MS);
    let content = serde_json::json!({
        "application": "m.call",
        "call_id": "",
        "scope": "m.room",
        "device_id": device_id,
        "focus_active": { "type": "livekit", "focus_selection": "oldest_membership" },
        "foci_preferred": [ {
            "type": "livekit",
            "livekit_service_url": service_url,
            "livekit_alias": room_id,
        } ],
        "created_ts": created_ts,
        "expires": expires_ms,
        "membershipID": format!("{user_id}:{device_id}"),
        // Say what kind of client this is. Peers use it to tell a Tin Hat
        // native client (patched libwebrtc, decodes encrypted AV1) from a browser /
        // Element X (whose LiveKit JS SDK cannot decrypt AV1 at all) and pick a share
        // codec everyone can actually watch. matrix-js-sdk ignores unknown keys.
        "dev.tinhat.client": {
            "kind": "native",
            "version": env!("CARGO_PKG_VERSION"),
            // 0.8.7 "never record me": the Memories recorder skips every track of a
            // member whose LIVE membership carries this flag, and drops them from
            // the mixed audio. Advertised on every (re)publish, so a mid-call flip
            // lands at the next membership refresh.
            "record_optout": crate::settings::record_optout(),
            // Pad (docs/PAD.md): "pad1" = my other verified devices may drive this call from their Pad (Settings →
            // Calls → Pad, default on). Nothing else reads `caps` today; older clients ignore it.
            "caps": if crate::pad_ctl::pad_enabled() { serde_json::json!([crate::padrules::PAD_CAP]) } else { serde_json::json!([]) },
        },
    });
    room.send_state_event_raw(MEMBER_EVENT_TYPE, &state_key, content).await?;
    Ok(())
}

/// Our own LIVE membership's `created_ts` (None when absent/tombstoned/expired), read
/// via the same raw-JSON path as `call_member_devices`. Used so a REFRESH republish
/// keeps the original stamp — matrix-js-sdk treats a changed created_ts as
/// leave+rejoin and rotates every web peer's media key (30-min rotation storms in
/// field logs).
async fn live_own_membership_created_ts(room: &Room, state_key: &str) -> Option<u64> {
    let raws = room.get_state_events_static::<CallMemberEventContent>().await.ok()?;
    for raw in &raws {
        let Ok(v) = serde_json::to_value(raw) else { continue };
        if v.get("state_key").and_then(|k| k.as_str()) != Some(state_key) {
            continue;
        }
        let content = v.get("content")?;
        if content.as_object().map(|o| o.is_empty()).unwrap_or(true) {
            return None; // tombstone — a genuinely new session
        }
        let created = content
            .get("created_ts")
            .and_then(|t| t.as_u64())
            .or_else(|| v.get("origin_server_ts").and_then(|t| t.as_u64()))?;
        let expires = content.get("expires").and_then(|t| t.as_u64()).unwrap_or(MEMBER_EXPIRES_MS);
        if now_ms() <= created.saturating_add(expires) {
            return Some(created);
        }
        return None; // expired — treat as new
    }
    None
}

/// Withdraw our membership (empty content = left) so peers stop enumerating us — Element
/// Call derives the visible participant list from these `m.call.member` state events, NOT the
/// SFU, so a dropped withdrawal leaves a GHOST participant (and can block our own rejoin).
/// Retries on error (an `Ok` already proves the homeserver persisted the blank — it returns
/// the event id), since a single best-effort write that times out is the ghost-leave bug.
pub async fn leave_rtc_session(room: &Room) -> matrix_sdk::Result<()> {
    if room.state() != RoomState::Joined {
        return Ok(());
    }
    let user_id = room.own_user_id().to_string();
    let Some(device_id) = room.client().device_id().map(|d| d.to_string()) else {
        return Ok(());
    };
    let state_key = membership_state_key(&user_id, &device_id);
    let mut last_err = None;
    for attempt in 0..3 {
        match room
            .send_state_event_raw(MEMBER_EVENT_TYPE, &state_key, serde_json::json!({}))
            .await
        {
            Ok(_) => return Ok(()),
            Err(e) => {
                eprintln!("[rtc] membership withdrawal attempt {} failed: {e}", attempt + 1);
                last_err = Some(e);
                tokio::time::sleep(std::time::Duration::from_millis(300 * (attempt + 1))).await;
            }
        }
    }
    Err(last_err.expect("loop ran at least once"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 12.0.5: the focus of the OLDEST live session (the web's resolveFocus), not the first in store order; expired,
    /// not-owned and focus-less sessions never pick it; an empty alias is the room id; a created_ts tie goes to the
    /// lower sender (calls round: a total order).
    #[test]
    fn resolve_focus_takes_the_oldest_live_session() {
        let room = "!call:a.test";
        let now = 1_761_400_000_000u64;
        let m = |s: &str, d: &str, c: u64, url: &str, alias: &str, sk: Option<&str>| {
            let mut content = json!({"application":"m.call","call_id":"","scope":"m.room","device_id":d,
                "created_ts":c,"expires":14_400_000u64,"membershipID":format!("{s}:{d}")});
            if !url.is_empty() {
                content["foci_preferred"] = json!([{"type":"livekit","livekit_service_url":url,"livekit_alias":alias}]);
            }
            json!({"type":"org.matrix.msc3401.call.member","sender":s,"origin_server_ts":c,
                   "state_key": sk.map(str::to_string).unwrap_or_else(|| format!("_{s}_{d}_m.call")),"content":content})
        };
        let evs = vec![
            m("@new:a.test", "N", now - 1_000, "https://new.test/livekit/jwt", room, None),
            m("@ghost:b.test", "G", now - 20_000_000, "https://ghost.test/livekit/jwt", room, None), // expired
            m("@mallory:b.test", "M", now - 9_000, "https://evil.test/livekit/jwt", room, Some("_@victim:a.test_V_m.call")), // not owned
            m("@nofocus:a.test", "X", now - 8_000, "", "", None),
            m("@old:a.test", "O", now - 5_000, "https://old.test/livekit/jwt", "", None),
            m("@tie:a.test", "T", now - 5_000, "https://tie.test/livekit/jwt", room, None),
        ];
        let f = oldest_focus(&crate::keyrules::live_sessions_from(&evs, now), room).unwrap();
        assert_eq!((f.service_url.as_str(), f.alias.as_str()), ("https://old.test/livekit/jwt", room));
        assert!(oldest_focus(&crate::keyrules::live_sessions_from(&evs[1..4], now), room).is_none());
    }

    /// The strip must count BOTH membership spellings, both content formats, skip
    /// tombstones and expired ghosts, and dedupe multi-device users.
    #[test]
    fn call_member_users_from_covers_both_spellings_and_drops_ghosts() {
        let now: u64 = 1_800_000_000_000;
        let h = 3_600_000u64;
        let events = vec![
            // session format, msc3401 spelling, live (created 1h ago, 4h expiry)
            json!({"type":"org.matrix.msc3401.call.member","sender":"@native:example.com",
                   "origin_server_ts": now - h,
                   "content":{"application":"m.call","device_id":"D1","created_ts": now - h,"expires": 4*h}}),
            // stable spelling written by a web client — the one the old code never saw
            json!({"type":"m.call.member","sender":"@web:example.com",
                   "origin_server_ts": now - 10_000,
                   "content":{"application":"m.call","device_id":"WEB","expires": 4*h}}),
            // ghost of an unclean exit: created 9h ago, 4h expiry
            json!({"type":"org.matrix.msc3401.call.member","sender":"@ghost:example.com",
                   "origin_server_ts": now - 9*h,
                   "content":{"application":"m.call","device_id":"G","created_ts": now - 9*h,"expires": 4*h}}),
            // tombstone (left)
            json!({"type":"m.call.member","sender":"@left:example.com","origin_server_ts": now - 1000,"content":{}}),
            // legacy array format: one expired entry + one live entry
            json!({"type":"org.matrix.msc3401.call.member","sender":"@legacy:example.com",
                   "origin_server_ts": now - 2*h,
                   "content":{"memberships":[{"expires": h},{"expires": 3*h}]}}),
            // legacy array, all expired
            json!({"type":"org.matrix.msc3401.call.member","sender":"@legacyghost:example.com",
                   "origin_server_ts": now - 5*h,
                   "content":{"memberships":[{"expires": h}]}}),
            // second device of the native user: deduped
            json!({"type":"m.call.member","sender":"@native:example.com",
                   "origin_server_ts": now - 100,
                   "content":{"application":"m.call","device_id":"D2","expires": 4*h}}),
        ];
        let users = call_member_users_from(&events, now);
        assert_eq!(users, vec!["@native:example.com", "@web:example.com", "@legacy:example.com"]);
    }

    /// The call service a well-known names (Tin Hat Web's focusFromWellKnown, the same cases): the first livekit focus
    /// with a service url; nothing usable → None (new calls then use DEFAULT_FOCUS_URL).
    #[test]
    fn focus_from_wellknown_takes_the_first_livekit_focus() {
        let tinhat = json!({"m.homeserver": {"base_url": "https://chat.crew.test"},
            "org.matrix.msc4143.rtc_foci": [{"type": "livekit", "livekit_service_url": "https://chat.crew.test/livekit/jwt"}]});
        assert_eq!(focus_from_wellknown(&tinhat).as_deref(), Some("https://chat.crew.test/livekit/jwt"));
        let mixed = json!({"org.matrix.msc4143.rtc_foci": [{"type": "nextgen", "livekit_service_url": "https://a.example/jwt"},
            {"type": "livekit", "livekit_service_url": ""}, {"type": "livekit", "livekit_service_url": "https://b.example/jwt"},
            {"type": "livekit", "livekit_service_url": "https://c.example/jwt"}]});
        assert_eq!(focus_from_wellknown(&mixed).as_deref(), Some("https://b.example/jwt"));
        for none in [
            json!({"m.homeserver": {"base_url": "https://matrix.example"}}),
            json!({"org.matrix.msc4143.rtc_foci": "https://a.example/jwt"}),
            json!({"org.matrix.msc4143.rtc_foci": [{"type": "livekit"}]}),
            json!({"org.matrix.msc4143.rtc_foci": [{"type": "livekit", "livekit_service_url": 7}]}),
            json!(null),
        ] {
            assert_eq!(focus_from_wellknown(&none), None, "{none}");
        }
    }
}
