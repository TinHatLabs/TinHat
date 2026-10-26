//! Discord-style navigation backend — Spaces + rooms, ported from the earlier web
//! client's corner docks + `SpaceStore` meta-spaces. This is the data layer the QML nav
//! (`ThSpaceRail` + `ThRoomList`, composed in `ThNav`) binds to, in BOTH the
//! persistent-sidebar and the corner-tucked view modes.
//!
//! It exposes, for the active "space":
//!   • `spacesJson`  — the space rail: meta-spaces (Home / Favourites / People /
//!     Rooms) + any real Matrix spaces the user has joined `[{key,label,kind,
//!     avatar,unread,highlight}]`.
//!   • `activeSpace` / `activeLabel` — the selected space key + its display label.
//!   • `roomsJson`   — the rooms inside the active space, recency-sorted, a shape
//!     identical to `LoginController.roomsJson` BUT a **distinct model** so the
//!     dock and the legacy room-list pillar never fight over one list.
//!   • `globalBadge` — total unread-notification count across all rooms (badge text;
//!     "" when zero).
//!
//! Meta-space membership mirrors the fork / element-web `MetaSpace`:
//!   home → every joined non-space room · favourites → `is_favourite()` ·
//!   people → DMs (`is_direct()`) · orphans("Rooms") → non-DM non-space rooms.
//! A real space's rooms are its `m.space.child` state events ∩ joined rooms.

use core::pin::Pin;

use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;
use matrix_sdk::{Client, Room};
use serde_json::{json, Value};

/// Where the user's custom real-space order is persisted (a JSON array of room-id keys).
fn space_order_file() -> std::path::PathBuf {
    crate::bridge::data_dir().join("space_order")
}

/// The saved real-space order (room-id keys, top→bottom). Empty = default order.
fn read_space_order() -> Vec<String> {
    std::fs::read_to_string(space_order_file())
        .ok()
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .unwrap_or_default()
}

/// Persist the real-space order (a drag-reorder commits the visible key sequence).
fn write_space_order(keys: &[String]) {
    let _ = std::fs::create_dir_all(crate::bridge::data_dir());
    if let Ok(json) = serde_json::to_string(keys) {
        let _ = std::fs::write(space_order_file(), json);
    }
}

/// Fixed meta-spaces, in the fork's display order `(key, label)`.
const META_SPACES: &[(&str, &str)] = &[
    ("home", "Home"),
    ("favourites", "Favourites"),
    ("people", "People"),
    ("orphans", "Rooms"),
];

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
        #[qproperty(QString, spaces_json)] //  QML: spacesJson  — [{key,label,kind,avatar,unread,highlight,membership:"join"|"invite",inviter,inviterMxid,memberCount}]
        #[qproperty(QString, active_space)] // QML: activeSpace — selected space key ("home" default)
        #[qproperty(QString, active_label)] // QML: activeLabel — display label of the active space
        #[qproperty(QString, rooms_json)] //   QML: roomsJson   — rooms in the active space (distinct model)
        #[qproperty(QString, global_badge)] // QML: globalBadge — total unread count ("" when zero)
        #[qproperty(QString, dm_strip_json)] // QML: dmStripJson — 10.6 (REACH §1): {"entries":[{id,name,avatar,peer,unread,highlight,invite,recency}],"overflow":N}
        type NavController = super::NavControllerRust;

        /// QML: refresh() — recompute spaces + the active space's rooms + badge.
        #[qinvokable]
        fn refresh(self: Pin<&mut NavController>);
        /// QML: refreshBadge() — cheap unread-only recompute for the always-visible
        /// handle badge (no per-room display_name/avatar/space walk). Safe to call on
        /// every sync; the full refresh() is reserved for when the panel is revealed.
        #[qinvokable]
        fn refresh_badge(self: Pin<&mut NavController>);
        /// QML: refreshParticipants() — recompute ONLY the "who's in the call" strips of
        /// the rooms already in the list (no name/avatar/space walk). Before 0.8.23 the full
        /// refresh() only ran when the room-list JSON changed, and that JSON carries no
        /// participant data — so mid-call joins/leaves never refreshed the strip. The
        /// in-call 3 s ticker and the join/leave edges call this instead.
        #[qinvokable]
        fn refresh_participants(self: Pin<&mut NavController>);
        /// QML: selectSpace(key) — drill into a space and recompute its rooms.
        /// (Named `select_space`, not `set_active_space`, to avoid colliding with the
        /// `active_space` qproperty's auto-generated setter.)
        #[qinvokable]
        fn select_space(self: Pin<&mut NavController>, key: &QString);
        /// QML: setSpaceOrder(keysJson) — persist a user-chosen order of the REAL spaces
        /// (a JSON array of room-id keys, top→bottom). Meta-spaces stay pinned on top.
        #[qinvokable]
        fn set_space_order(self: Pin<&mut NavController>, keys_json: &QString);
        /// QML: markRoomRead(roomId) — 10.1 (TH-NAV-03): a public read receipt + fully-read
        /// marker on the room's newest event WITHOUT opening it (the list's "Mark as read").
        #[qinvokable]
        #[cxx_name = "markRoomRead"]
        fn mark_room_read(self: Pin<&mut NavController>, room_id: &QString);
        /// QML: markSpaceRead(key) — 10.1 (TH-NAV-03): the same for every unread room the rail
        /// key owns (a real space's children; "home" = orphans + DMs; "favourites").
        #[qinvokable]
        #[cxx_name = "markSpaceRead"]
        fn mark_space_read(self: Pin<&mut NavController>, key: &QString);
        /// QML: spaceOf(roomId) — 10.1 (TH-NAV-02): the rail key that owns a room ("home" for
        /// orphans and DMs), so opening a room from Ctrl+K / a toast can switch the rail to it.
        #[qinvokable]
        #[cxx_name = "spaceOf"]
        fn space_of(self: Pin<&mut NavController>, room_id: &QString) -> QString;
    }

    impl cxx_qt::Threading for NavController {}
}

pub struct NavControllerRust {
    spaces_json: QString,
    active_space: QString,
    active_label: QString,
    rooms_json: QString,
    global_badge: QString,
    dm_strip_json: QString,
    /// 10.1: room id → owning space id, from the last snapshot (for `space_of`).
    room_space: std::sync::Mutex<std::collections::HashMap<String, String>>,
}

impl Default for NavControllerRust {
    fn default() -> Self {
        Self {
            spaces_json: QString::default(),
            active_space: QString::from("home"),
            active_label: QString::from("Home"),
            rooms_json: QString::default(),
            global_badge: QString::default(),
            dm_strip_json: QString::default(),
            room_space: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }
}

impl qobject::NavController {
    pub fn space_of(self: Pin<&mut Self>, room_id: &QString) -> QString {
        let id = room_id.to_string();
        let key = self
            .rust()
            .room_space
            .lock()
            .ok()
            .and_then(|m| m.get(&id).cloned())
            .unwrap_or_else(|| "home".to_string());
        QString::from(&key)
    }

    /// 10.1 (TH-NAV-03): mark one room read from the list (no need to open it).
    pub fn mark_room_read(self: Pin<&mut Self>, room_id: &QString) {
        let id = room_id.to_string();
        if id.is_empty() {
            return;
        }
        crate::bridge::runtime().spawn(async move {
            let Some(client) = crate::session::client() else { return };
            mark_rooms_read(&client, vec![id]).await;
        });
    }

    /// 10.1 (TH-NAV-03): mark every unread room a rail key owns read.
    pub fn mark_space_read(self: Pin<&mut Self>, key: &QString) {
        let key = key.to_string();
        if key.is_empty() {
            return;
        }
        let owner: std::collections::HashMap<String, String> =
            self.rust().room_space.lock().map(|m| m.clone()).unwrap_or_default();
        crate::bridge::runtime().spawn(async move {
            let Some(client) = crate::session::client() else { return };
            let mut ids = Vec::new();
            for room in client.joined_rooms() {
                if room.unread_notification_counts().notification_count == 0 {
                    continue;
                }
                let rid = room.room_id().to_string();
                let owned = match key.as_str() {
                    "favourites" => room.is_favourite(),
                    "home" => owner.get(&rid).map(|k| k == "home").unwrap_or(true),
                    k => owner.get(&rid).map(|o| o == k).unwrap_or(false),
                };
                if owned {
                    ids.push(rid);
                }
            }
            mark_rooms_read(&client, ids).await;
        });
    }

    pub fn refresh(self: Pin<&mut Self>) {
        let active = self.active_space().to_string();
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let Some(client) = crate::session::client() else {
                return;
            };
            let snap = build_nav(&client, &active).await;
            let built_for = active.clone();
            let _ = qt.queue(move |mut o| {
                // spaces rail + global badge are independent of the active space — always safe.
                o.as_mut().set_spaces_json(QString::from(&snap.spaces_json));
                o.as_mut().set_global_badge(QString::from(&snap.global_badge));
                // 10.6 (REACH §1.4): the DM strip is built from ALL rooms, never the active space.
                if o.dm_strip_json().to_string() != snap.dm_strip_json {
                    o.as_mut().set_dm_strip_json(QString::from(&snap.dm_strip_json));
                }
                // The room list is space-specific: if the user has since drilled into a
                // different space, this snapshot is stale — drop it so a slow refresh for
                // the previous space can't overwrite the freshly-selected one.
                if o.active_space().to_string() == built_for {
                    o.as_mut().set_rooms_json(QString::from(&snap.rooms_json));
                    o.as_mut().set_active_label(QString::from(&snap.active_label));
                }
                if let Ok(mut m) = o.rust().room_space.lock() {
                    *m = snap.room_space;
                }
            });
        });
    }

    pub fn refresh_participants(self: Pin<&mut Self>) {
        let current = self.rooms_json().to_string();
        if current.is_empty() {
            return;
        }
        let active = self.active_space().to_string();
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let Some(client) = crate::session::client() else { return };
            let Ok(mut rooms) = serde_json::from_str::<Vec<Value>>(&current) else { return };
            let mut changed = false;
            for r in rooms.iter_mut() {
                let Some(id) = r.get("id").and_then(|i| i.as_str()).map(|s| s.to_string()) else { continue };
                let Ok(rid) = matrix_sdk::ruma::RoomId::parse(&id) else { continue };
                let Some(room) = client.get_room(&rid) else { continue };
                let call = room.has_active_room_call();
                let parts = if call { resolve_call_participants(&client, &room).await } else { Vec::new() };
                let parts_v = Value::Array(parts);
                let same = r.get("participants").map(|p| p == &parts_v).unwrap_or(parts_v.as_array().map(|a| a.is_empty()).unwrap_or(true))
                    && r.get("call").and_then(|c| c.as_bool()) == Some(call);
                if !same {
                    r["participants"] = parts_v;
                    r["call"] = Value::Bool(call);
                    changed = true;
                }
            }
            if !changed {
                return;
            }
            let Ok(json_out) = serde_json::to_string(&rooms) else { return };
            let _ = qt.queue(move |mut o| {
                if o.active_space().to_string() == active {
                    o.as_mut().set_rooms_json(QString::from(&json_out));
                }
            });
        });
    }

    pub fn refresh_badge(self: Pin<&mut Self>) {
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let Some(client) = crate::session::client() else {
                return;
            };
            let badge = global_badge_text(&client).await;
            let _ = qt.queue(move |mut o| o.as_mut().set_global_badge(QString::from(&badge)));
        });
    }

    pub fn select_space(mut self: Pin<&mut Self>, key: &QString) {
        let key = key.to_string();
        if key.is_empty() {
            return;
        }
        // Set the selection synchronously so the dock highlights instantly, then
        // recompute that space's rooms off-thread.
        self.as_mut().set_active_space(QString::from(&key));
        self.refresh();
    }

    pub fn set_space_order(mut self: Pin<&mut Self>, keys_json: &QString) {
        if let Ok(keys) = serde_json::from_str::<Vec<String>>(&keys_json.to_string()) {
            write_space_order(&keys);
        }
        // Re-emit the rail in the new order (build_nav reads the saved order).
        self.as_mut().refresh();
    }
}

/// A computed nav snapshot (all the qproperties recomputed together in one pass so
/// the dock never shows a half-updated state).
struct NavSnapshot {
    spaces_json: String,
    rooms_json: String,
    active_label: String,
    global_badge: String,
    /// 10.6 (REACH §1): the rail's DM strip, `dm_strip_json()` of every joined + invited room.
    dm_strip_json: String,
    /// 10.1 (TH-NAV-02): room id → owning space id.
    room_space: std::collections::HashMap<String, String>,
}

/// One room-list entry `(recency, json)` for the nav room panel. `membership` is
/// "join" or "invite" (invites render Accept/Decline). For a room with a live call we
/// also resolve the connected users (Discord-style "who's in the call" bubbles).
async fn nav_room_entry(
    client: &Client,
    settings: &matrix_sdk::notification_settings::NotificationSettings,
    room: &Room,
    membership: &str,
) -> (Option<u64>, Value) {
    let name = match room.display_name().await {
        Ok(d) => d.to_string(),
        Err(_) => room
            .name()
            .or_else(|| room.canonical_alias().map(|a| a.to_string()))
            .unwrap_or_else(|| room.room_id().to_string()),
    };
    let recency = room.recency_stamp().map(u64::from);
    // 2026-10-23: mentions in an encrypted room counted by this device (notify::badge_counts)
    let unread = crate::notify::badge_counts(room).await;
    // 10.6 (REACH §1.1): an INVITE is a DM when our own m.room.member event says is_direct —
    // m.direct only learns about the room after we join.
    let is_dm = if membership == "invite" {
        invite_is_direct(room).await
    } else {
        room.is_direct().await.unwrap_or(false)
    };
    let call = room.has_active_room_call();
    let notify_mode = crate::notify::mode_in(settings, room, is_dm).await;
    let muted = matches!(
        notify_mode,
        matrix_sdk_base::notification_settings::RoomNotificationMode::Mute
    );
    // 10.6 (REACH §1.4 / A2.14): a DM row knows its one other person (the presence dot) and
    // falls back to their member avatar when the room has none.
    let (peer, peer_avatar) = if is_dm { dm_peer(room, membership).await } else { (String::new(), None) };
    let avatar = match room.avatar_url() {
        Some(mxc) => crate::bridge::resolve_avatar(client, &mxc).await.unwrap_or_default(),
        None => match peer_avatar {
            Some(mxc) => crate::bridge::resolve_avatar(client, &mxc).await.unwrap_or_default(),
            None => String::new(),
        },
    };
    let participants = if call {
        resolve_call_participants(client, room).await
    } else {
        Vec::new()
    };
    // "What we're playing" + per-person statuses — cheap LOCAL reads off already-synced
    // room state (no network). DMs stay clean: the fork gates both to non-DM rooms.
    let (activity, statuses) = if is_dm {
        (Value::Null, Vec::new())
    } else {
        (
            nav_activity_value(room).await,
            nav_status_values(room).await,
        )
    };
    (
        recency,
        json!({
            "id": room.room_id().to_string(),
            "name": name,
            "avatar": avatar,
            // 10.1 (TH-NOTIF-03): the badge counts what would NOTIFY — nothing for a muted room,
            // mentions only for a mentions-only room — so muting visibly does something and the
            // number does not overstate what needs you.
            "unread": if muted {
                0
            } else if matches!(notify_mode, matrix_sdk_base::notification_settings::RoomNotificationMode::MentionsAndKeywordsOnly) {
                unread.highlight_count
            } else {
                unread.notification_count
            },
            "highlight": unread.highlight_count > 0,
            "dm": is_dm,
            "peer": peer,
            "favourite": room.is_favourite(),
            "call": call,
            "membership": membership,
            "notifyMode": crate::notify::mode_name(notify_mode),
            "muted": muted,
            "participants": participants,
            "activity": activity,
            "statuses": statuses,
            // 10.1 (TH-GAP-05): the sidebar's context menu builds "Copy invite link" from THIS row.
            // Without these two the link was always the raw !id and the toast always said "they
            // will still need an invite", even for a public room with an alias.
            "alias": room.canonical_alias().map(|a| a.to_string()).unwrap_or_default(),
            // Whether a link is enough on its own, or the person also needs an invite.
            // Told to the user rather than discovered when their friend cannot get in.
            "publicJoin": matches!(
                room.join_rule(),
                Some(
                    matrix_sdk::ruma::events::room::join_rules::JoinRule::Public
                        | matrix_sdk::ruma::events::room::join_rules::JoinRule::Knock
                )
            ),
        }),
    )
}

/// The room's current now-playing as `{emoji,game,by,code}` (empty `by`/`emoji`/`code`
/// tolerated), or `Null` when none. Reuses `activity::read_activity`'s `(game,emoji,set_by,code)`.
async fn nav_activity_value(room: &Room) -> Value {
    match crate::activity::read_activity(room).await {
        Some((game, emoji, set_by, code)) => json!({ "emoji": emoji, "game": game, "by": set_by, "code": code }),
        None => Value::Null,
    }
}

/// Per-person, expiry-filtered statuses for the room as `[{userId,name,text}]`. Reuses
/// `status_ctl::render_statuses` (already drops expired/cleared) and resolves a member
/// display name (cache-only, best-effort → bare mxid).
async fn nav_status_values(room: &Room) -> Vec<Value> {
    let (json_str, _mine) = crate::status_ctl::render_statuses(room).await;
    let parsed: Vec<Value> = serde_json::from_str(&json_str).unwrap_or_default();
    let mut out = Vec::with_capacity(parsed.len());
    for entry in parsed {
        let user_id = entry.get("userId").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let text = entry.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if user_id.is_empty() || text.is_empty() {
            continue;
        }
        let mut name = user_id.clone();
        if let Ok(parsed_id) = matrix_sdk::ruma::UserId::parse(&user_id) {
            if let Ok(Some(member)) = room.get_member_no_sync(&parsed_id).await {
                if let Some(dn) = member.display_name() {
                    name = dn.to_string();
                }
            }
        }
        out.push(json!({ "userId": user_id, "name": name, "text": text }));
    }
    out
}

/// Resolve a live call's participants to `[{mxid,name,avatar}]` (member display name +
/// avatar, best-effort, cache-only — falls back to the bare mxid).
/// 10.1 (TH-NAV-03): public read receipt + fully-read marker on each room's newest event.
/// The server recomputes the unread counts and the next sync redraws the badges.
pub(crate) async fn mark_rooms_read(client: &Client, ids: Vec<String>) {
    use matrix_sdk::room::{MessagesOptions, Receipts};
    for id in ids {
        let Ok(rid) = matrix_sdk::ruma::RoomId::parse(&id) else { continue };
        let Some(room) = client.get_room(&rid) else { continue };
        let mut opts = MessagesOptions::backward();
        opts.limit = matrix_sdk::ruma::UInt::new(1).unwrap_or_default();
        let Ok(resp) = room.messages(opts).await else { continue };
        let Some(ev) = resp.chunk.first() else { continue };
        let Ok(v) = ev.raw().deserialize_as::<Value>() else { continue };
        let Some(eid) = v.get("event_id").and_then(|e| e.as_str()) else { continue };
        let Ok(eid) = matrix_sdk::ruma::OwnedEventId::try_from(eid) else { continue };
        let receipts = Receipts::new().fully_read_marker(eid.clone()).public_read_receipt(eid);
        if let Err(e) = room.send_multiple_receipts(receipts).await {
            eprintln!("[nav] mark read failed for {id}: {e}");
        }
    }
}

pub(crate) async fn resolve_call_participants(client: &Client, room: &Room) -> Vec<Value> {
    let ids = crate::rtc::call_participants(room).await;
    let mut out = Vec::with_capacity(ids.len());
    for uid in ids {
        let mut name = uid.clone();
        let mut avatar = String::new();
        if let Ok(parsed) = matrix_sdk::ruma::UserId::parse(&uid) {
            if let Ok(Some(member)) = room.get_member_no_sync(&parsed).await {
                if let Some(dn) = member.display_name() {
                    name = dn.to_string();
                }
                if let Some(mxc) = member.avatar_url() {
                    avatar = crate::bridge::resolve_avatar(client, mxc).await.unwrap_or_default();
                }
            }
        }
        out.push(json!({ "mxid": uid, "name": name, "avatar": avatar }));
    }
    out
}

// ───────────────────────── 10.6: the DM strip (docs/REACH.md §1) ─────────────────────────

/// How many DMs the strip shows at most, and how many of those may be invites (A2.14).
pub const DM_STRIP_MAX: usize = 5;
pub const DM_STRIP_MAX_INVITES: usize = 2;

/// One room as the DM strip sees it (REACH §1.1). `unread` is the room list's badge number
/// (0 for a muted room, the highlight count for a mentions-only room); `recency` the newest
/// activity in ms since the epoch (0 = unknown; for an invite, the invite time); `peer` the
/// other person's mxid ("" when there is not exactly one).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DmCandidate {
    pub id: String,
    pub dm: bool,
    pub membership: String,
    pub unread: u64,
    pub highlight: bool,
    pub recency: u64,
    pub name: String,
    pub avatar: String,
    pub peer: String,
}

/// The strip itself: the bubbles to draw and how many qualifying DMs did not fit ("+N").
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DmStrip {
    pub entries: Vec<DmCandidate>,
    pub overflow: usize,
}

/// REACH §1.1 + A2.14 — pure, and mirrored by the web's `features/nav/dmstrip.ts` (same test
/// names). A room qualifies when it is a DM and either joined with something unread or an
/// invite. Newest first; ties by the lowercased name compared by code point (Rust `str`
/// order), then by id. The first five are kept, at most two of them invites; every other
/// qualifying room counts toward `overflow`.
pub fn dm_strip(rooms: &[DmCandidate]) -> DmStrip {
    let mut q: Vec<&DmCandidate> = rooms
        .iter()
        .filter(|r| r.dm && ((r.membership == "join" && r.unread > 0) || r.membership == "invite"))
        .collect();
    q.sort_by(|a, b| {
        b.recency
            .cmp(&a.recency)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.id.cmp(&b.id))
    });
    let mut entries: Vec<DmCandidate> = Vec::new();
    let mut invites = 0usize;
    for r in &q {
        if entries.len() >= DM_STRIP_MAX {
            break;
        }
        let invite = r.membership == "invite";
        if invite && invites >= DM_STRIP_MAX_INVITES {
            continue;
        }
        if invite {
            invites += 1;
        }
        entries.push((*r).clone());
    }
    let overflow = q.len() - entries.len();
    DmStrip { entries, overflow }
}

/// The `dmStripJson` wire shape (REACH §1.4).
pub fn dm_strip_json(strip: &DmStrip) -> String {
    let entries: Vec<Value> = strip
        .entries
        .iter()
        .map(|e| {
            json!({
                "id": e.id, "name": e.name, "avatar": e.avatar, "peer": e.peer,
                "unread": e.unread, "highlight": e.highlight,
                "invite": e.membership == "invite", "recency": e.recency,
            })
        })
        .collect();
    json!({ "entries": entries, "overflow": strip.overflow }).to_string()
}

// ───────────────────────── 11.0.2: Ctrl+K's pending invites (ACTIONS.md §4.3) ─────────────────────────

/// One pending invite as Ctrl+K orders it: the room id, the room list's name for it, and the invite time in ms since
/// the epoch (our own `m.room.member` event, `invite_recency_ms`; 0 = unknown).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PaletteInvite {
    pub id: String,
    pub name: String,
    pub at: u64,
}

/// ACTIONS.md §4.3 `lists.invite` (2026-09-28: Ctrl+K lists pending invites again) — pure, and mirrored by the
/// web's `features/nav/palettelists.ts` `paletteInviteOrder` (same test names): newest first; ties by the lowercased
/// name compared by code point (Rust `str` order), then by id — the DM strip's rule. The room ids in that order.
pub fn palette_invite_order(invites: &[PaletteInvite]) -> Vec<String> {
    let mut v: Vec<&PaletteInvite> = invites.iter().collect();
    v.sort_by(|a, b| {
        b.at.cmp(&a.at)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.id.cmp(&b.id))
    });
    v.into_iter().map(|i| i.id.clone()).collect()
}

/// REACH §1.1: an invite is a DM invite when OUR OWN `m.room.member` event carries
/// `is_direct: true` (m.direct only knows the room once we have joined).
pub(crate) async fn invite_is_direct(room: &Room) -> bool {
    use matrix_sdk::deserialized_responses::MemberEvent;
    let Ok(Some(m)) = room.get_member_no_sync(room.own_user_id()).await else { return false };
    let flag = match m.event().as_ref() {
        MemberEvent::Stripped(ev) => ev.content.is_direct,
        MemberEvent::Sync(ev) => ev.as_original().and_then(|o| o.content.is_direct),
    };
    flag == Some(true)
}

/// REACH §1.4 / A2.14: the other person of a DM — their mxid only when the room has exactly ONE
/// other member (joined or invited); for an invite, whoever invited us. Returns (mxid or "",
/// their member avatar). Local store reads only.
pub(crate) async fn dm_peer(room: &Room, membership: &str) -> (String, Option<matrix_sdk::ruma::OwnedMxcUri>) {
    use matrix_sdk::ruma::OwnedUserId;
    if membership == "invite" {
        // A2.14 for invites too: the inviter is the peer only when the invite's own member state
        // shows exactly one other person — and that person is the inviter (Astra review).
        let Ok(inv) = room.invite_details().await else { return (String::new(), None) };
        let own = room.own_user_id().to_owned();
        let others: Vec<OwnedUserId> = room
            .members_no_sync(matrix_sdk::RoomMemberships::JOIN | matrix_sdk::RoomMemberships::INVITE)
            .await
            .map(|ms| ms.iter().map(|m| m.user_id().to_owned()).filter(|u| *u != own).collect())
            .unwrap_or_default();
        if others.len() > 1 || others.iter().any(|u| *u != inv.inviter_id) {
            return (String::new(), None);
        }
        let avatar = inv.inviter.as_ref().and_then(|m| m.avatar_url().map(|u| u.to_owned()));
        return (inv.inviter_id.to_string(), avatar);
    }
    let own = room.own_user_id().to_owned();
    // The local member store is complete here (Tin Hat does not lazy-load members); the sync
    // summary's counts and heroes only arrive WITH lazy loading, so they are the fallback.
    let stored: Vec<OwnedUserId> = room
        .members_no_sync(matrix_sdk::RoomMemberships::JOIN | matrix_sdk::RoomMemberships::INVITE)
        .await
        .map(|ms| ms.iter().map(|m| m.user_id().to_owned()).collect())
        .unwrap_or_default();
    let counted = room.joined_members_count() + room.invited_members_count();
    let total = if counted > 0 { counted } else { stored.len() as u64 };
    if total != 2 {
        return (String::new(), None);
    }
    let heroes = room.heroes();
    let mut others: Vec<OwnedUserId> = stored.into_iter().filter(|u| *u != own).collect();
    if others.len() != 1 {
        others = heroes.iter().map(|h| h.user_id.clone()).filter(|u| *u != own).collect();
    }
    if others.len() != 1 {
        return (String::new(), None);
    }
    let peer = others.remove(0);
    let avatar = match room.get_member_no_sync(&peer).await {
        Ok(Some(m)) => m.avatar_url().map(|u| u.to_owned()),
        _ => None,
    }
    .or_else(|| heroes.iter().find(|h| h.user_id == peer).and_then(|h| h.avatar_url.clone()));
    (peer.to_string(), avatar)
}

/// Newest timeline activity per joined room seen by the live sync (ms since the epoch). The
/// SDK's `recency_stamp` is not a time, and its latest-event cache is not always filled under
/// our manual `sync_once` loop, so the strip keeps its own clock. Fed by `note_sync`.
/// Persisted (`<data>/room_recency.json`, room id → ms) so an unread DM from before this run
/// still sorts by its real time after a restart.
fn room_ts() -> &'static std::sync::Mutex<std::collections::HashMap<String, u64>> {
    static T: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, u64>>> =
        std::sync::OnceLock::new();
    T.get_or_init(|| {
        let saved = std::fs::read_to_string(room_ts_file())
            .ok()
            .and_then(|s| serde_json::from_str::<std::collections::HashMap<String, u64>>(&s).ok())
            .unwrap_or_default();
        std::sync::Mutex::new(saved)
    })
}

fn room_ts_file() -> std::path::PathBuf {
    crate::bridge::data_dir().join("room_recency.json")
}

/// Called from the sync loop after every successful pass: remember each joined room's newest
/// event time (saved at most every 30 s, and only when something moved).
pub fn note_sync(response: &matrix_sdk::sync::SyncResponse) {
    static SAVED_AT: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
    // An update inside the 30 s window is remembered and flushed by a LATER pass, even a quiet
    // one — it used to wait for the next activity, which might never come (Astra review).
    static DIRTY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    let Ok(mut m) = room_ts().lock() else { return };
    let mut moved = DIRTY.load(std::sync::atomic::Ordering::Relaxed);
    for (rid, update) in &response.rooms.joined {
        let newest = update
            .timeline
            .events
            .iter()
            .filter_map(|e| e.timestamp().map(|t| u64::from(t.0)))
            .max();
        if let Some(ts) = newest {
            let slot = m.entry(rid.to_string()).or_insert(0);
            if ts > *slot {
                *slot = ts;
                moved = true;
            }
        }
    }
    if !moved {
        return;
    }
    DIRTY.store(true, std::sync::atomic::Ordering::Relaxed);
    let Ok(mut at) = SAVED_AT.lock() else { return };
    if at.map_or(true, |t| t.elapsed() >= std::time::Duration::from_secs(30)) {
        *at = Some(std::time::Instant::now());
        if let Ok(json) = serde_json::to_string(&*m) {
            let _ = std::fs::create_dir_all(crate::bridge::data_dir());
            if std::fs::write(room_ts_file(), json).is_ok() {
                DIRTY.store(false, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }
}

/// Sign-out: the next account must not inherit this one's room clock.
pub fn forget_recency() {
    if let Ok(mut m) = room_ts().lock() {
        m.clear();
    }
    let _ = std::fs::remove_file(room_ts_file());
}

/// Newest activity of a joined room in ms (0 = unknown).
fn joined_recency_ms(room: &Room) -> u64 {
    let latest = room.latest_event().timestamp().map(|t| u64::from(t.0)).unwrap_or(0);
    let seen = room_ts().lock().ok().and_then(|m| m.get(room.room_id().as_str()).copied()).unwrap_or(0);
    latest.max(seen)
}

/// When we were invited, in ms (0 = unknown): our own member event's timestamp (Synapse sends
/// the invite event itself with the stripped state), else the SDK's latest-event invite time.
pub(crate) async fn invite_recency_ms(room: &Room) -> u64 {
    use matrix_sdk::deserialized_responses::RawSyncOrStrippedState as R;
    use matrix_sdk::ruma::events::room::member::RoomMemberEventContent;
    let own = room.own_user_id().to_owned();
    let from_event = match room.get_state_event_static_for_key::<RoomMemberEventContent, _>(&own).await {
        Ok(Some(R::Sync(raw))) => raw.get_field::<u64>("origin_server_ts").ok().flatten(),
        Ok(Some(R::Stripped(raw))) => raw.get_field::<u64>("origin_server_ts").ok().flatten(),
        _ => None,
    };
    from_event.unwrap_or_else(|| room.latest_event().timestamp().map(|t| u64::from(t.0)).unwrap_or(0))
}

/// The room list's badge number for a room (TH-NOTIF-03): 0 when muted, the highlight count
/// for a mentions-only room, else the notification count — the same rule as `nav_room_entry`.
async fn badge_unread(settings: &matrix_sdk::notification_settings::NotificationSettings, room: &Room, is_dm: bool) -> u64 {
    use matrix_sdk_base::notification_settings::RoomNotificationMode as M;
    let counts = crate::notify::badge_counts(room).await;
    match crate::notify::mode_in(settings, room, is_dm).await {
        M::Mute => 0,
        M::MentionsAndKeywordsOnly => counts.highlight_count,
        _ => counts.notification_count,
    }
}

async fn room_name(room: &Room) -> String {
    match room.display_name().await {
        Ok(d) => d.to_string(),
        Err(_) => room
            .name()
            .or_else(|| room.canonical_alias().map(|a| a.to_string()))
            .unwrap_or_else(|| room.room_id().to_string()),
    }
}

/// Build the strip from EVERY joined and invited room (never just the active space). Cheap
/// facts first; the peer and the picture are resolved only for the bubbles actually drawn.
async fn build_dm_strip(client: &Client, settings: &matrix_sdk::notification_settings::NotificationSettings) -> DmStrip {
    let mut rooms: Vec<(DmCandidate, Room)> = Vec::new();
    for room in client.joined_rooms() {
        if room.is_space() || !room.is_direct().await.unwrap_or(false) {
            continue;
        }
        let unread = badge_unread(settings, &room, true).await;
        if unread == 0 {
            continue; // cannot qualify — skip the name/recency reads
        }
        let c = DmCandidate {
            id: room.room_id().to_string(),
            dm: true,
            membership: "join".into(),
            unread,
            highlight: crate::notify::badge_counts(&room).await.highlight_count > 0,
            recency: joined_recency_ms(&room),
            name: room_name(&room).await,
            ..Default::default()
        };
        rooms.push((c, room));
    }
    for room in client.invited_rooms() {
        if room.is_space() || !invite_is_direct(&room).await {
            continue;
        }
        // An invite's stripped state rarely names the room: say who is asking instead.
        let name = match room.display_name().await {
            Ok(matrix_sdk::RoomDisplayName::Empty) | Ok(matrix_sdk::RoomDisplayName::EmptyWas(_)) | Err(_) => {
                match room.invite_details().await {
                    Ok(inv) => inv
                        .inviter
                        .as_ref()
                        .map(|m| m.name().to_string())
                        .unwrap_or_else(|| inv.inviter_id.localpart().to_string()),
                    Err(_) => room_name(&room).await,
                }
            }
            Ok(d) => d.to_string(),
        };
        let c = DmCandidate {
            id: room.room_id().to_string(),
            dm: true,
            membership: "invite".into(),
            unread: 0,
            highlight: false,
            recency: invite_recency_ms(&room).await,
            name,
            ..Default::default()
        };
        rooms.push((c, room));
    }
    let cands: Vec<DmCandidate> = rooms.iter().map(|(c, _)| c.clone()).collect();
    let mut strip = dm_strip(&cands);
    for e in strip.entries.iter_mut() {
        let Some((_, room)) = rooms.iter().find(|(c, _)| c.id == e.id) else { continue };
        let (peer, peer_avatar) = dm_peer(room, &e.membership).await;
        e.peer = peer;
        let mxc = room.avatar_url().or(peer_avatar);
        if let Some(mxc) = mxc {
            e.avatar = crate::bridge::resolve_avatar(client, &mxc).await.unwrap_or_default();
        }
    }
    strip
}

/// 10.6 (REACH A5.3): the number a room's ROW shows — 0 for a muted room, the highlights for a
/// mentions-only room, the notification count for an all-messages room or a DM — and a space
/// room counts nothing. Every rollup (meta + space bubbles, the dock/corner badge, the global
/// badge) is the sum of these, so a bubble never says "99+" over rows that show nothing.
pub(crate) fn adjusted_unread(
    mode: matrix_sdk_base::notification_settings::RoomNotificationMode,
    notification: u64,
    highlight: u64,
    is_space: bool,
) -> u64 {
    use matrix_sdk_base::notification_settings::RoomNotificationMode as M;
    if is_space {
        return 0;
    }
    match mode {
        M::Mute => 0,
        M::MentionsAndKeywordsOnly => highlight,
        M::AllMessages => notification,
    }
}

/// A5.3: the red flag = any unmuted (non-space) row with highlights.
pub(crate) fn adjusted_highlight(
    mode: matrix_sdk_base::notification_settings::RoomNotificationMode,
    highlight: u64,
    is_space: bool,
) -> bool {
    !is_space && highlight > 0 && !matches!(mode, matrix_sdk_base::notification_settings::RoomNotificationMode::Mute)
}

/// A5.3: a rollup over rows `(mode, notification, highlight, is_space)` → (total, red).
pub(crate) fn rollup(
    rows: &[(matrix_sdk_base::notification_settings::RoomNotificationMode, u64, u64, bool)],
) -> (u64, bool) {
    rows.iter().fold((0, false), |(n, red), (m, notif, hl, space)| {
        (n + adjusted_unread(*m, *notif, *hl, *space), red || adjusted_highlight(*m, *hl, *space))
    })
}

fn badge_text(total: u64) -> String {
    if total == 0 {
        String::new()
    } else if total > 99 {
        "99+".to_string()
    } else {
        total.to_string()
    }
}

/// A5.3: once per session, after the first live sync — a read receipt on the newest event of
/// every joined SPACE whose server count is above 0. Nothing reads a space, so its original
/// invite stayed "unread" forever: the permanent 1 on the space (and on phones' badges).
pub async fn clear_space_receipts(client: &Client) {
    let ids: Vec<String> = client
        .joined_rooms()
        .into_iter()
        .filter(|r| r.is_space() && r.unread_notification_counts().notification_count > 0)
        .map(|r| r.room_id().to_string())
        .collect();
    if ids.is_empty() {
        return;
    }
    eprintln!("[nav] {} space(s) still counted as unread — sending their receipts", ids.len());
    mark_rooms_read(client, ids).await;
}

/// Precomputed facts about one joined room (computed once per refresh so meta-space
/// membership is a pure predicate instead of N×4 repeated async store reads).
struct RoomFact {
    id: String,
    is_space: bool,
    is_dm: bool,
    favourite: bool,
    parented: bool, // a child of at least one real Matrix space
    unread: u64,
    highlight: bool,
}

/// Does a (non-space) room belong in the given meta-space? Mirrors element-web's
/// SpaceStore `showInHomeSpace` with the fork default `allRoomsInHome=false`: a room
/// that lives inside a real space shows ONLY in that space, not in Home/Rooms.
fn meta_member(f: &RoomFact, key: &str) -> bool {
    if f.is_space {
        return false; // spaces are rail entries, never room rows
    }
    match key {
        // Home: orphan rooms (no parent space) + DMs.
        "home" => !f.parented || f.is_dm,
        "favourites" => f.favourite,
        "people" => f.is_dm,
        // "Rooms" (orphans): non-DM rooms with no parent space.
        "orphans" => !f.parented && !f.is_dm,
        _ => false,
    }
}

/// The global / dock badge (10.6, A5.3): the sum of what the rows show, as badge text ("" at 0).
async fn global_badge_text(client: &Client) -> String {
    let mut total: u64 = 0;
    let settings = client.notification_settings().await; // once per pass (notify::mode_in)
    for r in client.joined_rooms() {
        if r.is_space() {
            continue;
        }
        let is_dm = r.is_direct().await.unwrap_or(false);
        let mode = crate::notify::mode_in(&settings, &r, is_dm).await;
        let u = crate::notify::badge_counts(&r).await;
        total += adjusted_unread(mode, u.notification_count, u.highlight_count, false);
    }
    badge_text(total)
}

/// Rooms in `space_id` that we have NOT joined and WOULD be allowed to join.
///
/// Uses the space hierarchy API, which reports each child's join_rule without us being a
/// member. Only `public` and `restricted` are listed: an invite-only child stays hidden
/// unless we actually hold an invite, so a space is never a directory of private rooms.
/// Child SPACES are skipped — they belong on the space rail, not in a room list.
async fn space_unjoined_children(
    client: &Client,
    space_id: &str,
    joined_ids: &std::collections::HashSet<String>,
) -> Vec<Value> {
    // 10.1 (TH-NAV-18): every nav refresh inside a space used to fire a /hierarchy request
    // (a refresh runs on each room-list change). The joinable-children answer changes rarely,
    // so it is cached per space for a minute; the joined filter is applied on every read so a
    // room you just joined drops out of "not joined" immediately.
    let mut cached = hierarchy_cached(space_id).await;
    if cached.is_none() {
        let fresh = fetch_hierarchy_children(client, space_id).await;
        hierarchy_store(space_id, &fresh).await;
        cached = Some(fresh);
    }
    let mut out = cached.unwrap_or_default();
    out.retain(|v| v.get("id").and_then(|i| i.as_str()).map(|i| !joined_ids.contains(i)).unwrap_or(false));
    out
}

type HierCache = std::collections::HashMap<String, (std::time::Instant, Vec<Value>)>;
static HIER_CACHE: std::sync::OnceLock<tokio::sync::Mutex<HierCache>> = std::sync::OnceLock::new();
const HIER_TTL: std::time::Duration = std::time::Duration::from_secs(60);

async fn hierarchy_cached(space_id: &str) -> Option<Vec<Value>> {
    let m = HIER_CACHE.get_or_init(|| tokio::sync::Mutex::new(HierCache::new())).lock().await;
    m.get(space_id).filter(|(at, _)| at.elapsed() < HIER_TTL).map(|(_, v)| v.clone())
}

async fn hierarchy_store(space_id: &str, rooms: &[Value]) {
    let mut m = HIER_CACHE.get_or_init(|| tokio::sync::Mutex::new(HierCache::new())).lock().await;
    m.insert(space_id.to_string(), (std::time::Instant::now(), rooms.to_vec()));
}

/// The raw /hierarchy answer for one space, privacy-filtered (public / restricted rooms only,
/// child spaces skipped) but NOT filtered by membership — see space_unjoined_children.
async fn fetch_hierarchy_children(client: &Client, space_id: &str) -> Vec<Value> {
    use matrix_sdk::ruma::api::client::space::get_hierarchy;
    let Ok(sid) = matrix_sdk::ruma::RoomId::parse(space_id) else {
        return Vec::new();
    };
    let mut req = get_hierarchy::v1::Request::new(sid.to_owned());
    // One level: the rooms directly in this space. Deeper nesting is its own space.
    req.max_depth = Some(1u32.into());
    req.limit = Some(100u32.into());
    let resp = match client.send(req).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[nav] hierarchy for {space_id} failed: {e}");
            return Vec::new();
        }
    };
    let mut out = Vec::new();
    for child in resp.rooms {
        let id = child.summary.room_id.to_string();
        if id == space_id {
            continue;
        }
        if child.summary.room_type.is_some() {
            continue; // a child space, not a room
        }
        // THE PRIVACY FILTER. Anything we could not actually enter must not be listed.
        let rule = match &child.summary.join_rule {
            matrix_sdk::ruma::room::JoinRuleSummary::Public => "public",
            matrix_sdk::ruma::room::JoinRuleSummary::Restricted(_) => "restricted",
            _ => continue,
        };
        let avatar = match &child.summary.avatar_url {
            Some(mxc) => crate::bridge::resolve_avatar(client, mxc).await.unwrap_or_default(),
            None => String::new(),
        };
        out.push(json!({
            "id": id,
            "name": child.summary.name.clone().unwrap_or_else(|| child.summary.room_id.to_string()),
            "membership": "none",          // the room list draws this as "not joined"
            "joinRule": rule,
            "memberCount": child.summary.num_joined_members,
            "topic": child.summary.topic.clone().unwrap_or_default(),
            "avatar": avatar,
            "unread": 0, "highlight": false, "muted": false, "isDm": false,
            "call": false, "favourite": false, "participants": [], "statuses": [],
            "activity": Value::Null, "notifyMode": "all", "alias": "", "publicJoin": rule == "public",
        }));
    }
    eprintln!("[nav] {space_id}: {} joinable child room(s) (cached 60 s)", out.len());
    out
}

/// Child room ids of a real space (its active `m.space.child` state events).
pub(crate) async fn space_child_ids(space: &Room) -> std::collections::HashSet<String> {
    let mut ids = std::collections::HashSet::new();
    for (child_id, _sender, content) in crate::appstate::get_state_all(space, "m.space.child").await
    {
        // A child is "active" when it still advertises at least one `via` server;
        // an empty/absent `via` is a removed child (element-web's isActiveChild).
        let active = content
            .get("via")
            .and_then(|v| v.as_array())
            .map(|a| !a.is_empty())
            .unwrap_or(false);
        if active {
            ids.insert(child_id);
        }
    }
    ids
}

/// One space we are INVITED to, for the rail (RBAC §5.3 / §7.3).
struct InvitedSpace {
    id: String,
    label: String,
    avatar: String,
    inviter: String,
    inviter_mxid: String,
    member_count: u64,
}

async fn invited_space_entry(client: &Client, space: &Room) -> InvitedSpace {
    let id = space.room_id().to_string();
    let label = space
        .name()
        .or_else(|| space.canonical_alias().map(|a| a.to_string()))
        .unwrap_or_else(|| id.clone());
    let avatar = match space.avatar_url() {
        Some(mxc) => crate::bridge::resolve_avatar(client, &mxc).await.unwrap_or_default(),
        None => String::new(),
    };
    // The inviter is the sender of our own stripped m.room.member event; the SDK also hands
    // us their member row (display name) when the invite state carried it.
    let (inviter, inviter_mxid) = match space.invite_details().await {
        Ok(inv) => {
            let name = inv
                .inviter
                .as_ref()
                .map(|m| m.name().to_string())
                .unwrap_or_else(|| inv.inviter_id.localpart().to_string());
            (name, inv.inviter_id.to_string())
        }
        Err(e) => {
            eprintln!("[nav] invite details for {id} unavailable: {e}");
            (String::new(), String::new())
        }
    };
    let member_count = invited_space_member_count(client, space).await;
    InvitedSpace { id, label, avatar, inviter, inviter_mxid, member_count }
}

/// Member count of a space we have not joined. The invite's stripped state carries no
/// summary, so ask the server once (room preview: MSC3266 summary, else /hierarchy — Synapse
/// answers both for an invited user) and cache it for a while; on failure fall back to the
/// joined members visible in the stripped state (at least the inviter). Cached either way so
/// the rail's frequent rebuilds never repeat the network call.
static INVITE_COUNT_CACHE: std::sync::OnceLock<
    tokio::sync::Mutex<std::collections::HashMap<String, (std::time::Instant, u64)>>,
> = std::sync::OnceLock::new();
const INVITE_COUNT_TTL: std::time::Duration = std::time::Duration::from_secs(600);

async fn invited_space_member_count(client: &Client, space: &Room) -> u64 {
    let id = space.room_id().to_string();
    let cache = INVITE_COUNT_CACHE.get_or_init(|| tokio::sync::Mutex::new(std::collections::HashMap::new()));
    if let Some((at, n)) = cache.lock().await.get(&id).copied() {
        if at.elapsed() < INVITE_COUNT_TTL {
            return n;
        }
    }
    let via: Vec<matrix_sdk::ruma::OwnedServerName> = id
        .rsplit_once(':')
        .and_then(|(_, s)| matrix_sdk::ruma::ServerName::parse(s).ok())
        .into_iter()
        .collect();
    let target: &matrix_sdk::ruma::RoomOrAliasId = space.room_id().into();
    let n = match client.get_room_preview(target, via).await {
        Ok(p) => p.num_joined_members,
        Err(e) => {
            eprintln!("[nav] preview for invited space {id} failed: {e}");
            let stripped = space
                .members_no_sync(matrix_sdk::RoomMemberships::JOIN)
                .await
                .map(|m| m.len() as u64)
                .unwrap_or(0);
            stripped.max(space.joined_members_count())
        }
    };
    cache.lock().await.insert(id, (std::time::Instant::now(), n));
    n
}

/// Sort: pending invites, then DMs with something new, then favourites (10.1, TH-NAV-07: the
/// tier bridge::rooms_json always had and this list silently lacked — "Favourite" did nothing
/// visible), then active call, then anything else unread, then SDK recency, then name.
fn sort_rooms(rooms: &mut [(Option<u64>, Value)]) {
    let name_of = |v: &Value| v["name"].as_str().unwrap_or("").to_lowercase();
    let is_invite = |v: &Value| v["membership"].as_str() == Some("invite");
    let fav_of = |v: &Value| v["favourite"].as_bool().unwrap_or(false);
    let unread_of =
        |v: &Value| !v["muted"].as_bool().unwrap_or(false) && v["unread"].as_u64().unwrap_or(0) > 0;
    let unread_dm_of = |v: &Value| unread_of(v) && v["dm"].as_bool().unwrap_or(false);
    rooms.sort_by(|a, b| {
        let a_call = a.1["call"].as_bool().unwrap_or(false);
        let b_call = b.1["call"].as_bool().unwrap_or(false);
        is_invite(&b.1)
            .cmp(&is_invite(&a.1)) // pending invites to the very top
            .then_with(|| unread_dm_of(&b.1).cmp(&unread_dm_of(&a.1))) // then new DMs
            .then_with(|| fav_of(&b.1).cmp(&fav_of(&a.1))) // then favourites
            .then_with(|| b_call.cmp(&a_call))
            .then_with(|| unread_of(&b.1).cmp(&unread_of(&a.1))) // then anything unread
            .then_with(|| match (a.0, b.0) {
                (Some(x), Some(y)) => y.cmp(&x),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            })
            .then_with(|| name_of(&a.1).cmp(&name_of(&b.1)))
    });
}

/// Compute the full nav snapshot for `active_space` from the local store.
async fn build_nav(client: &Client, active_space: &str) -> NavSnapshot {
    use std::collections::HashMap;
    let joined = client.joined_rooms();
    // the push rules, read + parsed ONCE for the whole snapshot (rows, rollups, DM strip) — see notify::mode_in
    let settings = client.notification_settings().await;

    // ---- resolve every real space's children once: a parent index (which rooms live
    // inside *some* space) + per-space child sets (for the rail's per-space unread). ----
    struct SpaceEntry {
        id: String,
        label: String,
        avatar: String,
        children: std::collections::HashSet<String>,
    }
    let mut space_entries: Vec<SpaceEntry> = Vec::new();
    let mut parented: std::collections::HashSet<String> = std::collections::HashSet::new();
    for space in joined.iter().filter(|r| r.is_space()) {
        let children = space_child_ids(space).await;
        for c in &children {
            parented.insert(c.clone());
        }
        let label = space
            .name()
            .or_else(|| space.canonical_alias().map(|a| a.to_string()))
            .unwrap_or_else(|| space.room_id().to_string());
        let avatar = match space.avatar_url() {
            Some(mxc) => crate::bridge::resolve_avatar(client, &mxc).await.unwrap_or_default(),
            None => String::new(),
        };
        space_entries.push(SpaceEntry { id: space.room_id().to_string(), label, avatar, children });
    }
    // Apply the user's drag-chosen order: keys in the saved order come first (in that
    // order); any new/unsaved space keeps its default position at the end (stable).
    let order = read_space_order();
    if !order.is_empty() {
        space_entries.sort_by_key(|se| order.iter().position(|k| k == &se.id).unwrap_or(usize::MAX));
    }
    // 10.1 (TH-NAV-02): which space owns each room, for `space_of`.
    let mut room_space: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for se in &space_entries {
        for c in &se.children {
            room_space.entry(c.clone()).or_insert_with(|| se.id.clone());
        }
    }
    // RBAC §5.3 / §7.3: spaces we are INVITED to ride the rail too (dashed ring, "Invited
    // by …"), after the joined ones. They own no rooms (`room_space` / `spaceOf` never see
    // them) and, when selected, yield an empty room list — the children come after joining.
    let mut invited_spaces: Vec<InvitedSpace> = Vec::new();
    for space in client.invited_rooms().iter().filter(|r| r.is_space()) {
        invited_spaces.push(invited_space_entry(client, space).await);
    }
    invited_spaces.sort_by(|a, b| a.label.to_lowercase().cmp(&b.label.to_lowercase()));

    // ---- precompute per-room facts once (is_direct is async → do it a single time). ----
    let mut facts: Vec<RoomFact> = Vec::with_capacity(joined.len());
    let mut unread_by_id: HashMap<String, (u64, bool)> = HashMap::new();
    let mut total: u64 = 0;
    for room in &joined {
        let id = room.room_id().to_string();
        let u = crate::notify::badge_counts(room).await;
        let is_space = room.is_space();
        let is_dm = room.is_direct().await.unwrap_or(false);
        // 10.6 (A5.3): the rollups add up what the ROWS show, not the server's raw counts.
        let mode = crate::notify::mode_in(&settings, room, is_dm).await;
        let unread = adjusted_unread(mode, u.notification_count, u.highlight_count, is_space);
        let highlight = adjusted_highlight(mode, u.highlight_count, is_space);
        total += unread;
        unread_by_id.insert(id.clone(), (unread, highlight));
        facts.push(RoomFact {
            parented: parented.contains(&id),
            is_space,
            is_dm,
            favourite: room.is_favourite(),
            unread,
            highlight,
            id,
        });
    }
    let global_badge = badge_text(total);
    // A5.1: a pending DM invite adds 1 to the People bubble (it is waiting on you).
    let mut dm_invites: u64 = 0;
    for room in client.invited_rooms() {
        if !room.is_space() && invite_is_direct(&room).await {
            dm_invites += 1;
        }
    }

    // ---- the space rail: meta-spaces (with rollups) + real spaces (per-space rollups). ----
    let mut spaces: Vec<Value> = Vec::new();
    for (key, label) in META_SPACES {
        let mut count: u64 = 0;
        let mut hl = false;
        for f in &facts {
            if meta_member(f, key) {
                count += f.unread;
                hl |= f.highlight;
            }
        }
        if *key == "people" {
            count += dm_invites;
        }
        spaces.push(json!({
            "key": key, "label": label, "kind": "meta",
            "avatar": "", "unread": count, "highlight": hl,
        }));
    }
    let mut active_label = META_SPACES
        .iter()
        .find(|(k, _)| *k == active_space)
        .map(|(_, l)| l.to_string())
        .unwrap_or_default();
    for se in &space_entries {
        // Per-space unread = sum over this space's child rooms we've joined.
        let (mut count, mut hl) = (0u64, false);
        for cid in &se.children {
            if let Some((c, h)) = unread_by_id.get(cid) {
                count += *c;
                hl |= *h;
            }
        }
        if se.id == active_space {
            active_label = se.label.clone();
        }
        spaces.push(json!({
            "key": se.id, "label": se.label, "kind": "space",
            "avatar": se.avatar, "unread": count, "highlight": hl,
            "membership": "join",
        }));
    }
    for inv in &invited_spaces {
        if inv.id == active_space {
            active_label = inv.label.clone();
        }
        spaces.push(json!({
            "key": inv.id, "label": inv.label, "kind": "space",
            "avatar": inv.avatar, "unread": 0, "highlight": false,
            "membership": "invite", "inviter": inv.inviter, "inviterMxid": inv.inviter_mxid,
            "memberCount": inv.member_count,
        }));
    }

    // ---- rooms inside the active space ----
    // A real-space key is a room id ("!..."); use its resolved child set, else the
    // meta-space predicate.
    let active_children: Option<&std::collections::HashSet<String>> = if active_space.starts_with('!')
    {
        Some(
            space_entries
                .iter()
                .find(|se| se.id == active_space)
                .map(|se| &se.children)
                .unwrap_or(EMPTY_SET.get_or_init(std::collections::HashSet::new)),
        )
    } else {
        None
    };

    let mut rooms: Vec<(Option<u64>, Value)> = Vec::new();
    for (room, f) in joined.iter().zip(facts.iter()) {
        let include = match active_children {
            Some(children) => !f.is_space && children.contains(&f.id),
            None => meta_member(f, active_space),
        };
        if include {
            rooms.push(nav_room_entry(client, &settings, room, "join").await);
        }
    }
    // Pending invites are otherwise invisible — surface them in Home (sorted to top) and,
    // 10.1 (TH-NAV-01), inside the real space whose child they are, so an invite to a room
    // in the space you are looking at does not hide behind the Home tab.
    for room in client.invited_rooms() {
        let rid = room.room_id().to_string();
        let show = match active_children {
            Some(children) => children.contains(&rid),
            // 10.6 (REACH §1.3): a DM invite also waits in People — the strip's invite bubble
            // selects People and the Accept / Decline row has to be there.
            None => active_space == "home" || (active_space == "people" && invite_is_direct(&room).await),
        };
        if show {
            rooms.push(nav_room_entry(client, &settings, &room, "invite").await);
        }
    }
    // Inside a real space, also list the rooms there we have NOT joined but could —
    // Discord's "you can see the channels in this server". Private children are
    // filtered out by the hierarchy call, not here.
    // Only for a space we have JOINED: an invited space lists nothing until we join (RBAC §5.3).
    if active_space.starts_with('!') && space_entries.iter().any(|se| se.id == active_space) {
        let joined_ids: std::collections::HashSet<String> =
            facts.iter().map(|f| f.id.clone()).collect();
        for entry in space_unjoined_children(client, &active_space, &joined_ids).await {
            rooms.push((None, entry));
        }
    }
    sort_rooms(&mut rooms);
    let rooms_arr: Vec<Value> = rooms.into_iter().map(|(_, v)| v).collect();
    let strip = build_dm_strip(client, &settings).await;

    NavSnapshot {
        spaces_json: serde_json::to_string(&spaces).unwrap_or_else(|_| "[]".into()),
        rooms_json: serde_json::to_string(&rooms_arr).unwrap_or_else(|_| "[]".into()),
        active_label,
        global_badge,
        dm_strip_json: dm_strip_json(&strip),
        room_space,
    }
}

/// Shared empty set for the "real space with no resolvable children" fast path.
static EMPTY_SET: std::sync::OnceLock<std::collections::HashSet<String>> = std::sync::OnceLock::new();

/// Headless dump (no GUI): print the computed space rail + the active space's rooms +
/// global badge, so the dock's data layer can be proven without rendering.
///   TINHAT_NAV_DUMP=1 [TINHAT_NAV_SPACE=people] ./tinhat
pub async fn headless_dump(active: &str) {
    let Some(client) = crate::session::client() else {
        eprintln!("[nav] not signed in");
        return;
    };
    let snap = build_nav(&client, active).await;
    eprintln!("[nav] activeSpace={active} activeLabel={}", snap.active_label);
    eprintln!("[nav] globalBadge={:?}", snap.global_badge);
    let spaces: Value = serde_json::from_str(&snap.spaces_json).unwrap_or(Value::Null);
    let rooms: Value = serde_json::from_str(&snap.rooms_json).unwrap_or(Value::Null);
    let n_spaces = spaces.as_array().map(|a| a.len()).unwrap_or(0);
    let n_rooms = rooms.as_array().map(|a| a.len()).unwrap_or(0);
    eprintln!("[nav] spaces={n_spaces}: {}", snap.spaces_json);
    eprintln!("[nav] rooms({n_rooms}): {}", snap.rooms_json);
    eprintln!("[nav] dmStripJson={}", snap.dm_strip_json);
    eprintln!(
        "[nav] VERDICT: {}",
        if n_spaces >= META_SPACES.len() { "PASS" } else { "FAIL" }
    );
}

/// REACH §1.1 + A2.14 — the web runs the SAME 10 cases with the same fixtures in
/// `features/nav/dmstrip.test.ts` (tinhat-web): a DM defaults to joined, unread 1, recency 1000,
/// name = the id without its "!".
#[cfg(test)]
mod dm_strip_tests {
    use super::{dm_strip, dm_strip_json, DmCandidate, DmStrip, DM_STRIP_MAX};

    fn dm(id: &str) -> DmCandidate {
        let bare = id.trim_start_matches('!');
        DmCandidate {
            id: id.into(),
            dm: true,
            membership: "join".into(),
            unread: 1,
            highlight: false,
            recency: 1000,
            name: bare.into(),
            avatar: String::new(),
            peer: format!("@{bare}:example.com"),
        }
    }
    fn invite(id: &str, recency: u64) -> DmCandidate {
        DmCandidate { membership: "invite".into(), unread: 0, recency, ..dm(id) }
    }
    fn ids(s: &DmStrip) -> String {
        s.entries.iter().map(|e| e.id.as_str()).collect::<Vec<_>>().join(",")
    }
    fn json_of(s: &DmStrip) -> serde_json::Value {
        serde_json::from_str(&dm_strip_json(s)).unwrap()
    }

    #[test]
    fn empty() {
        let s = dm_strip(&[]);
        assert!(s.entries.is_empty() && s.overflow == 0);
        assert_eq!(json_of(&s), serde_json::json!({"entries": [], "overflow": 0}));
    }

    #[test]
    fn read_dm_hidden() {
        let s = dm_strip(&[DmCandidate { unread: 0, ..dm("!read") }]);
        assert!(s.entries.is_empty() && s.overflow == 0);
    }

    #[test]
    fn muted_dm_hidden() {
        // a muted DM's badge is 0 by construction (the server may still count messages)
        let s = dm_strip(&[DmCandidate { unread: 0, highlight: false, ..dm("!muted") }]);
        assert!(s.entries.is_empty() && s.overflow == 0);
    }

    #[test]
    fn group_excluded() {
        let s = dm_strip(&[DmCandidate { dm: false, unread: 5, highlight: true, ..dm("!group") }]);
        assert!(s.entries.is_empty() && s.overflow == 0);
    }

    #[test]
    fn invite_included() {
        let s = dm_strip(&[invite("!inv", 5)]);
        assert_eq!(ids(&s), "!inv");
        assert_eq!((s.entries[0].unread, s.overflow), (0, 0));
        let v = json_of(&s);
        assert_eq!(v["entries"][0]["invite"], true);
        assert_eq!(v["entries"][0]["peer"], "@inv:example.com");
    }

    #[test]
    fn order() {
        // newest first; equal recency → name case-insensitively ("adam" before "Zoe"), then id
        let s = dm_strip(&[
            DmCandidate { name: "Al".into(), recency: 100, ..dm("!r6") },
            DmCandidate { name: "Zoe".into(), recency: 200, ..dm("!r2") },
            DmCandidate { name: "Bea".into(), recency: 200, ..dm("!r5") },
            DmCandidate { name: "adam".into(), recency: 200, ..dm("!r3") },
            DmCandidate { name: "bea".into(), recency: 200, ..dm("!r4") },
        ]);
        assert_eq!(ids(&s), "!r3,!r4,!r5,!r2,!r6");
        assert_eq!(s.overflow, 0);
    }

    #[test]
    fn cap_overflow() {
        let rooms: Vec<DmCandidate> =
            (1..=7u64).map(|i| DmCandidate { recency: i * 10, ..dm(&format!("!c{i}")) }).collect();
        let s = dm_strip(&rooms);
        assert_eq!(DM_STRIP_MAX, 5);
        assert_eq!(ids(&s), "!c7,!c6,!c5,!c4,!c3");
        assert_eq!(s.overflow, 2);
        assert_eq!(json_of(&s)["overflow"], 2);
    }

    #[test]
    fn mentions_count() {
        // a DM in mentions mode carries its highlight count as the badge
        let s = dm_strip(&[DmCandidate { unread: 2, highlight: true, ..dm("!mention") }]);
        assert_eq!(ids(&s), "!mention");
        assert_eq!((s.entries[0].unread, s.entries[0].highlight), (2, true));
        assert_eq!(json_of(&s)["entries"][0]["invite"], false);
    }

    #[test]
    fn invite_cap() {
        // 4 invites (recency 10, 20, 30, 40) + 1 unread DM (recency 5) → invites 40, 30 + the DM; overflow 2
        let s = dm_strip(&[
            invite("!i10", 10),
            invite("!i20", 20),
            invite("!i30", 30),
            invite("!i40", 40),
            DmCandidate { unread: 1, recency: 5, ..dm("!dm5") },
        ]);
        assert_eq!(ids(&s), "!i40,!i30,!dm5");
        assert_eq!(s.overflow, 2);
    }

    #[test]
    fn unicode_order() {
        // equal recency: code point order of the lowercased names
        let names = ["éclair", "Zed", "apple", "😀 smile"];
        let rooms: Vec<DmCandidate> = names
            .iter()
            .enumerate()
            .map(|(i, n)| DmCandidate { name: n.to_string(), recency: 100, ..dm(&format!("!u{i}")) })
            .collect();
        let s = dm_strip(&rooms);
        let got: Vec<&str> = s.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(got.join("|"), "apple|Zed|éclair|😀 smile");
        assert_eq!(s.overflow, 0);
    }
}

#[cfg(test)]
mod palette_invite_tests {
    // the web's src/features/nav/palettelists.test.ts runs the same five cases under the same names
    use super::{palette_invite_order, PaletteInvite};

    fn inv(id: &str, name: &str, at: u64) -> PaletteInvite {
        PaletteInvite { id: id.into(), name: name.into(), at }
    }
    fn order(v: &[PaletteInvite]) -> String {
        palette_invite_order(v).join(",")
    }

    #[test]
    fn invite_order_empty() {
        assert!(palette_invite_order(&[]).is_empty());
    }

    #[test]
    fn invite_order_newest_first() {
        let v = [inv("!a", "Movie night", 100), inv("!b", "Zane", 300), inv("!c", "Simon", 200)];
        assert_eq!(order(&v), "!b,!c,!a");
    }

    #[test]
    fn invite_order_ties() {
        // the same time: the name case-insensitively ("adam" before "Bea"), then the id
        let v = [inv("!r2", "Zoe", 500), inv("!r5", "bea", 500), inv("!r3", "adam", 500), inv("!r4", "Bea", 500)];
        assert_eq!(order(&v), "!r3,!r4,!r5,!r2");
    }

    #[test]
    fn invite_order_unknown_time_last() {
        // 0 = the invite time is unknown: after every known one
        let v = [inv("!old", "Old", 0), inv("!new", "New", 20), inv("!mid", "Mid", 10)];
        assert_eq!(order(&v), "!new,!mid,!old");
    }

    #[test]
    fn invite_order_unicode() {
        // the same time: code point order of the lowercased names — U+FF5A before U+1F600 (UTF-16 units would put the
        // emoji's surrogate first); never a locale
        let v = [inv("!u0", "éclair", 7), inv("!u1", "Zed", 7), inv("!u2", "apple", 7), inv("!u3", "😀 smile", 7), inv("!u4", "\u{ff5a}", 7)];
        assert_eq!(order(&v), "!u2,!u1,!u0,!u4,!u3");
    }
}

/// REACH A5.3 — the web mirrors this name (`rollup_adjusted`) with the same rows.
#[cfg(test)]
mod rollup_tests {
    use super::{adjusted_highlight, badge_text, rollup};
    use matrix_sdk_base::notification_settings::RoomNotificationMode as M;

    #[test]
    fn rollup_adjusted() {
        let rows = [
            (M::Mute, 5, 0, false),                     // muted room with 5 → 0
            (M::MentionsAndKeywordsOnly, 10, 1, false), // mentions-only, notif 10 / highlight 1 → 1
            (M::AllMessages, 3, 0, false),              // a DM (all messages) with 3 → 3
            (M::MentionsAndKeywordsOnly, 1, 1, true),   // a space room with 1 → skipped
        ];
        let (total, red) = rollup(&rows);
        assert_eq!(total, 4);
        assert!(red, "the mentions-only row has a highlight");
        // red only for an unmuted, non-space row with highlights
        assert!(!rollup(&[(M::Mute, 9, 9, false), (M::AllMessages, 2, 0, false)]).1);
        assert!(!adjusted_highlight(M::AllMessages, 3, true));
        assert_eq!(badge_text(0), "");
        assert_eq!(badge_text(4), "4");
        assert_eq!(badge_text(446), "99+");
    }
}
