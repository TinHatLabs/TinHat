//! OS desktop notifications (the fork behaviour: a real freedesktop notification
//! when a room you are NOT looking at gains a new HIGHLIGHT — a mention/DM).
//!
//! This is pure-Rust, no C++/QML shim: `zbus` (already in the tree via ashpd's
//! portal) talks straight to `org.freedesktop.Notifications` on the session bus.
//! One cached `Connection` is reused for every notification (no per-notification
//! reconnect). All work runs on [`crate::bridge::runtime`].
//!
//! Detection is **edge-triggered**: we keep a process-side `last_highlight` map of a
//! per-room trigger count and only notify on a RISE, for a room that is NOT the one
//! currently open. The very first scan after launch SEEDS the map silently (so we don't
//! dump a notification for every already-unread room).
//!
//! 10.5.1: the trigger is OUR OWN tally of the events each sync delivered that notify
//! under the account's push rules, evaluated by the SDK AFTER decryption
//! (`SyncResponse::notifications`, see [`record_sync`]) — no longer Synapse's
//! `unread_notification_counts()`. The server cannot see inside `m.room.encrypted`, so in
//! an encrypted room set to All messages it counted every Element Call media-key event
//! (`io.element.call.encryption_keys` on the wire as `m.room.encrypted`) as a new message:
//! a field report of notifications every few minutes during calls (in one busy room nearly all of the
//! count-raising events during calls were ~400-byte key events, only a handful were messages). Decrypted, those events hit the 10.3 quiet rule and never
//! notify. Mentions in encrypted rooms now notify too (the server never saw them).
//! A rise still only fires while the SERVER has the room unread ([`unread_on_server`]): a room
//! read on another device while we were offline comes back in the reconnect sync with its
//! messages (they notify under the push rules, receipts are not consulted) but with a zero
//! server count — no chime for things already read (Astra review 2026-09-23). The one blind
//! spot is a mentions-only ENCRYPTED room, where the server never counts anything: ungated.
//!
//! Safety rails (this codebase has a real over-notify / plasmashell FD-exhaustion
//! risk class):
//!   • per-room debounce (≤1 notification per room per [`ROOM_DEBOUNCE`]),
//!     collapsing a burst into a single "N new messages" notification;
//!   • a global rate cap ([`GLOBAL_CAP`] within [`GLOBAL_WINDOW`]);
//!   • the currently-open room is suppressed (the user is looking at it);
//!   • NO read receipt / mark_read side effects — this path only READS counts and
//!     the cached latest event;
//!   • if the session bus / Notifications service is absent it fails silently
//!     (logs once), never panics.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use matrix_sdk::Client;
use matrix_sdk_base::notification_settings::RoomNotificationMode;

/// At most one notification per room within this window — a burst collapses into a
/// single "N new messages" notification instead of one-per-message spam.
const ROOM_DEBOUNCE: Duration = Duration::from_secs(5);
/// Global rate cap: never more than [`GLOBAL_CAP`] notifications per [`GLOBAL_WINDOW`].
const GLOBAL_CAP: usize = 8;
const GLOBAL_WINDOW: Duration = Duration::from_secs(10);
/// Notifications auto-expire after this (ms). -1 = let the server decide; we pick a
/// finite value so a stack of mentions doesn't pin the user's notification area.
/// (Transport-specific: only the freedesktop Notify call takes it today.)
#[cfg(target_os = "linux")]
const EXPIRE_MS: i32 = 8000;
/// An incoming-call notification lives as long as the ring (the toast in the app bounds it).
#[cfg(target_os = "linux")]
const CALL_EXPIRE_MS: i32 = 25000;

// ---- 10.0: actions on notifications route back to QML through the LoginController.
fn login_sink() -> &'static Mutex<Option<cxx_qt::CxxQtThread<crate::bridge::qobject::LoginController>>> {
    static S: OnceLock<Mutex<Option<cxx_qt::CxxQtThread<crate::bridge::qobject::LoginController>>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(None))
}
pub fn set_login_sink(t: cxx_qt::CxxQtThread<crate::bridge::qobject::LoginController>) {
    *login_sink().lock().unwrap() = Some(t);
}
/// Deliver a notification action ("join:<roomId>") to QML (LoginController.toastAction).
pub fn toast_action(action: String) {
    let Some(t) = login_sink().lock().unwrap().clone() else { return };
    eprintln!("[notify] toast action: {action}");
    let _ = t.queue(move |mut o| o.as_mut().toast_action(cxx_qt_lib::QString::from(&action)));
}
#[cfg(target_os = "linux")]
fn pending_actions() -> &'static Mutex<HashMap<u32, String>> {
    static P: OnceLock<Mutex<HashMap<u32, String>>> = OnceLock::new();
    P.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Rooms someone can actually SEE right now (10.1, TH-NOTIF-01 / TL-03): one entry per attended
/// view (the main window's page, each floating bubble), keyed by the view's token, so a view that
/// closes, hides behind a maximised call or loses window focus withdraws its claim. A rise in an
/// attended room's highlight count is suppressed — they are looking at it. The old design was a
/// single string that was set on every open and never cleared: navigate away from Room A and it
/// never notified again for the rest of the session.
fn attended() -> &'static Mutex<HashMap<u64, String>> {
    static R: OnceLock<Mutex<HashMap<u64, String>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A view (token) is now looking at `room_id` (Some) or at nothing (None). Receipt-free.
pub fn set_attending(token: u64, room_id: Option<String>) {
    let shown = room_id.clone();
    if let Ok(mut g) = attended().lock() {
        match room_id {
            Some(r) => {
                g.insert(token, r);
            }
            None => {
                g.remove(&token);
            }
        }
    }
    // widgets W1: a room on screen has its mentions read (the Mentions widget's list)
    if let Some(r) = shown {
        crate::mentions_ctl::room_attended(&r);
    }
}

/// An attended view re-targeted to another room (`RoomController::open` while on screen).
pub fn update_view_room(token: u64, room_id: &str) {
    let mut attending = false;
    if let Ok(mut g) = attended().lock() {
        if let Some(r) = g.get_mut(&token) {
            *r = room_id.to_string();
            attending = true;
        }
    }
    if attending {
        crate::mentions_ctl::room_attended(room_id); // widgets W1
    }
}

pub(crate) fn is_open_room(room_id: &str) -> bool {
    attended().lock().map(|g| g.values().any(|r| r == room_id)).unwrap_or(false)
}

/// One room's notification-relevant state for a single scan.
///
/// `trigger` is the count that MATTERS FOR THIS ROOM'S MODE — highlight_count for a
/// mentions-only room, notification_count for an all-messages room — so the edge
/// detector below stays mode-agnostic.
pub(crate) struct Observed {
    pub id: String,
    pub trigger: u64,
    /// Muted rooms still record their level (so unmuting can't dump a backlog) but
    /// never fire.
    pub muted: bool,
    pub is_dm: bool,
    /// Mode discriminant, only used to re-seed silently when the mode CHANGES: the
    /// trigger switches metric (mentions ↔ all), and a bare rise check would read
    /// that jump as "37 new messages".
    pub mode_tag: u8,
    /// 10.5.1: the server still counts this room as unread (see [`unread_on_server`]). A rise
    /// while this is false records the level but never fires: it was read somewhere else.
    pub unread_on_server: bool,
}

/// 10.5.1 (Astra review): may a tally rise in this room fire? Only while the server still has
/// the room unread — reading it on another device zeroes both counts, and the reconnect sync
/// would otherwise chime for messages already read. A mentions-only ENCRYPTED room is the
/// exception: the server cannot see mentions inside `m.room.encrypted` (and its room rule keeps
/// the plain count at 0), so there the server says nothing and the tally alone decides.
pub(crate) fn unread_on_server(mentions_only: bool, encrypted: bool, notification_count: u64, highlight_count: u64) -> bool {
    (mentions_only && encrypted) || notification_count > 0 || highlight_count > 0
}

/// 2026-10-23 (rooms are encrypted by default) — the highlight count a room's badge, red flag and rollups use. The
/// server can't see a mention inside an ENCRYPTED event, so its highlight count stays 0 there; this device's own record
/// of mentions (`unseen_here`: the Mentions list — every event the account's push rules HIGHLIGHT, judged by the SDK
/// after decryption — not yet seen here and newer than this account's own read receipt, mentions_ctl) fills in, but
/// only while the server still counts the room as unread (`.m.rule.encrypted` counts every message of a room without a
/// room rule), so reading it on ANY device clears the badge — and a later plain message doesn't bring back mentions
/// that were read elsewhere (the receipt is newer than them). A room with its own "Mentions" rule has a server count of
/// 0 and keeps today's badge (no number). The web's js-sdk recomputes its counts after decryption, so the web needs
/// nothing of the sort.
pub(crate) fn badge_highlight(encrypted: bool, notification_count: u64, highlight_count: u64, unseen_here: u64) -> u64 {
    if encrypted && notification_count > 0 {
        highlight_count.max(unseen_here)
    } else {
        highlight_count
    }
}

/// A room's unread counts for its badge: the server's, with [`badge_highlight`]'s highlight count.
pub(crate) async fn badge_counts(room: &matrix_sdk::Room) -> matrix_sdk_base::sync::UnreadNotificationsCount {
    let mut c = room.unread_notification_counts();
    if c.notification_count > 0 && room.encryption_state().is_encrypted() {
        let read = own_read_ts(room).await;
        let here = crate::mentions_ctl::unseen_in(room.room_id().as_str(), read);
        c.highlight_count = badge_highlight(true, c.notification_count, c.highlight_count, here);
    }
    c
}

/// When this account last read `room`, on any device: the time on its own read receipt (public or private,
/// unthreaded — both apps send unthreaded `m.read`), in ms; 0 = none known. A mention older than that was read.
/// An approximation (Astra 2026-10-23): the receipt's time, not its event's position — a receipt sent later for an
/// OLDER event (scrolled up elsewhere), or another server's skewed clock, can hide a still-unread mention from the
/// badge (the room still shows unread, and its toast already fired); erring that way never brings back mentions read
/// elsewhere. Exact ordering would need the event cache or an /event fetch per receipt.
async fn own_read_ts(room: &matrix_sdk::Room) -> u64 {
    use matrix_sdk::ruma::events::receipt::{ReceiptThread, ReceiptType};
    let mut best = 0u64;
    for ty in [ReceiptType::Read, ReceiptType::ReadPrivate] {
        if let Ok(Some((_, r))) = room.load_user_receipt(ty, &ReceiptThread::Unthreaded, room.own_user_id()).await {
            if let Some(ts) = r.ts {
                best = best.max(u64::from(ts.get()));
            }
        }
    }
    best
}

/// Edge-trigger + debounce state, all behind one lock.
#[derive(Default)]
struct DetectState {
    /// Have we seeded yet? First scan seeds silently (no startup notification storm).
    seeded: bool,
    /// room_id -> last trigger count we observed (metric depends on the room's mode).
    last_highlight: HashMap<String, u64>,
    /// room_id -> the mode its last recorded trigger was measured under.
    last_mode: HashMap<String, u8>,
    /// room_id -> when we last fired a notification for it (per-room debounce).
    last_fired: HashMap<String, Instant>,
    /// Timestamps of recent global fires (for the global rate cap).
    recent_fires: Vec<Instant>,
}

fn detect() -> &'static Mutex<DetectState> {
    static D: OnceLock<Mutex<DetectState>> = OnceLock::new();
    D.get_or_init(|| Mutex::new(DetectState::default()))
}

/// 10.5.1: per room, how many events that NOTIFY / HIGHLIGHT for us the syncs of this
/// session have delivered — cumulative, only ever rises. Fed by [`record_sync`] from the
/// SDK's own push-rule evaluation of each timeline event after decryption, so an encrypted
/// call-key event counts as what it really is (quiet) instead of as a message.
#[derive(Default, Clone, Copy, PartialEq, Debug)]
pub(crate) struct Tally {
    pub notify: u64,
    pub highlight: u64,
}

fn tallies() -> &'static Mutex<HashMap<String, Tally>> {
    static T: OnceLock<Mutex<HashMap<String, Tally>>> = OnceLock::new();
    T.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Add one sync's decryption-aware notifications to the per-room tallies. `actions` per
/// event are what the account's push rules said about the (decrypted) event; the SDK only
/// lists events that notify, and never our own (ruma skips the sender's own events).
pub(crate) fn tally_actions<'a>(room_id: &str, events: impl IntoIterator<Item = &'a [matrix_sdk::ruma::push::Action]>) {
    let Ok(mut t) = tallies().lock() else { return };
    let e = t.entry(room_id.to_string()).or_default();
    for actions in events {
        if actions.iter().any(|a| a.should_notify()) {
            e.notify += 1;
        }
        if actions.iter().any(|a| a.is_highlight()) {
            e.highlight += 1;
        }
    }
}

/// Called from the sync loop with every `sync_once` response, BEFORE [`scan_highlights`].
pub fn record_sync(notifications: &std::collections::BTreeMap<matrix_sdk::ruma::OwnedRoomId, Vec<matrix_sdk::sync::Notification>>) {
    for (room_id, list) in notifications {
        tally_actions(room_id.as_str(), list.iter().map(|n| n.actions.as_slice()));
    }
}

fn tally_of(room_id: &str) -> Tally {
    tallies().lock().ok().and_then(|t| t.get(room_id).copied()).unwrap_or_default()
}

/// Keep tallies only for rooms we are joined to. The SDK also lists an INVITE (stripped
/// `m.room.member`, `.m.rule.invite_for_me` notifies) and events of rooms we just left; kept,
/// those would surface as a phantom "New message" the moment we accept the invite / rejoin the
/// room (its first observation measures the stale tally against 0).
pub(crate) fn retain_joined(t: &mut HashMap<String, Tally>, joined: &std::collections::HashSet<&str>) {
    t.retain(|k, _| joined.contains(k.as_str()));
}

/// The cached session-bus connection. `None` until first use; an inner `None` after a
/// failed connect means "bus unavailable" — we log that once and stop trying noisily.
#[cfg(target_os = "linux")]
fn conn_cell() -> &'static tokio::sync::Mutex<Option<Option<zbus::Connection>>> {
    static C: OnceLock<tokio::sync::Mutex<Option<Option<zbus::Connection>>>> = OnceLock::new();
    C.get_or_init(|| tokio::sync::Mutex::new(None))
}

/// Emit the "session bus unavailable" log line exactly once.
#[cfg(target_os = "linux")]
fn log_bus_unavailable_once(err: &dyn std::fmt::Display) {
    static ONCE: OnceLock<()> = OnceLock::new();
    if ONCE.set(()).is_ok() {
        eprintln!("[notify] desktop notifications unavailable (session bus): {err}; disabling (silent from here)");
    }
}

/// Get (and lazily open + cache) the session-bus connection. Returns `None` if the bus
/// is unavailable — caller then fails silently.
/// 10.6: also the presence idle probe's bus (presence.rs) — one cached session connection.
#[cfg(target_os = "linux")]
pub(crate) async fn connection() -> Option<zbus::Connection> {
    let mut guard = conn_cell().lock().await;
    if let Some(cached) = guard.as_ref() {
        return cached.clone();
    }
    match zbus::Connection::session().await {
        Ok(c) => {
            *guard = Some(Some(c.clone()));
            Some(c)
        }
        Err(e) => {
            log_bus_unavailable_once(&e);
            *guard = Some(None); // remember the failure: don't retry-spam every sync
            None
        }
    }
}

/// Windows transport: a WinRT toast via tauri-winrt-notification (PLATFORM_AUDIT §6).
/// Toasts attribute to OUR AppUserModelID (polish §8.2): platform::register_aumid()
/// writes the HKCU\Software\Classes\AppUserModelId key at startup — the documented
/// unpackaged-app registration, no Start-menu shortcut required — so toasts show
/// "Tin Hat" + our icon instead of "Windows PowerShell". Toasts from an
/// UNregistered AUMID are silently dropped, which is why registration runs before
/// any toast can fire. Module contract preserved: best-effort, log-once, never
/// panics. COM work runs on a blocking thread, off the sync loop.
/// 10.0 (Windows): incoming-call toast with a Join button; the button's argument comes back
/// through on_activated and is routed to QML like the Linux action.
#[cfg(windows)]
pub async fn notify_call(summary: &str, body: &str, room_id: &str) {
    let summary = summary.to_string();
    let body = body.to_string();
    let action = format!("join:{room_id}");
    let _ = tokio::task::spawn_blocking(move || {
        use tauri_winrt_notification::{Duration as ToastDuration, Toast};
        let toast = Toast::new(crate::platform::AUMID)
            .title(&summary)
            .text1(&body)
            .duration(ToastDuration::Long)
            .add_button("Join", &action)
            .on_activated(move |arg| {
                if let Some(a) = arg {
                    if a.starts_with("join:") {
                        toast_action(a);
                    }
                }
                Ok(())
            });
        if let Err(e) = toast.show() {
            eprintln!("[notify] WinRT call toast failed: {e}");
        }
    })
    .await;
}

#[cfg(not(any(windows, target_os = "linux")))]
pub async fn notify_call(_summary: &str, _body: &str, _room_id: &str) {}

/// A plain notification (no click action). See `notify_action` for one that opens something.
pub async fn notify(summary: &str, body: &str, icon: Option<String>) {
    notify_action(summary, body, icon, "").await
}

/// guests: elsewhere a posted toast can't be withdrawn (a WinRT toast expires by itself): no id.
#[cfg(not(target_os = "linux"))]
pub async fn notify_action_id(summary: &str, body: &str, icon: Option<String>, action: &str) -> Option<u32> {
    notify_action(summary, body, icon, action).await;
    None
}

/// guests: nothing to withdraw where [`notify_action_id`] hands back no id.
#[cfg(not(target_os = "linux"))]
pub async fn close_notification(_id: u32) {}

/// 10.1 (TH-GAP-06 / TH-CAL-11): a notification whose CLICK routes `action` back to QML through
/// `toast_action` ("open:<roomId>", "calendar:<container>/<eventId>"), exactly like the incoming-
/// call toast's Join button. `action == ""` = the old plain toast.
#[cfg(not(target_os = "linux"))]
pub async fn notify_action(summary: &str, body: &str, icon: Option<String>, action: &str) {
    let summary = summary.to_string();
    let body = body.to_string();
    let action = action.to_string();
    let icon = icon.map(|u| crate::platform::path_from_file_url(&u));
    let _ = tokio::task::spawn_blocking(move || {
        use tauri_winrt_notification::{Duration as ToastDuration, IconCrop, Toast};
        let mut toast = Toast::new(crate::platform::AUMID)
            .title(&summary)
            .text1(&body)
            .duration(ToastDuration::Short);
        if !action.is_empty() {
            let a = action.clone();
            toast = toast.on_activated(move |_arg| {
                toast_action(a.clone());
                Ok(())
            });
        }
        // The sender's photo, round-cropped like every other chat app (QOL 1.5.0).
        // Skipped when the file is missing: WinRT drops the WHOLE toast on a bad image
        // path, so a stale avatar cache would silently cost the notification itself.
        if let Some(p) = icon.as_deref().filter(|p| !p.is_empty()) {
            let path = std::path::Path::new(p);
            if path.exists() {
                toast = toast.icon(path, IconCrop::Circular, "");
            }
        }
        let r = toast.show();
        if let Err(e) = r {
            static ONCE: OnceLock<()> = OnceLock::new();
            if ONCE.set(()).is_ok() {
                eprintln!("[notify] WinRT toast failed: {e}; silent from here");
            }
        }
    })
    .await;
}

/// Security review 2026-10-26 (M7's notification twin): a body is someone's text (a message, a name), and a
/// notification server that advertises `body-markup` (Plasma, GNOME, mako, dunst) renders `<b>`, `<a href>` and — with
/// `body-images` — `<img>` in it. The spec says escape the text for such a server (and only for such a server: one
/// without markup shows "&lt;" as it is). Pure; the caller asks the server once.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn markup_escape(body: &str) -> String {
    body.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// The body as this notification server needs it: escaped when it renders markup (its capabilities, asked once).
#[cfg(target_os = "linux")]
async fn body_for_server(conn: &zbus::Connection, body: &str) -> String {
    static MARKUP: tokio::sync::OnceCell<bool> = tokio::sync::OnceCell::const_new();
    let markup = *MARKUP
        .get_or_init(|| async {
            match conn
                .call_method(Some("org.freedesktop.Notifications"), "/org/freedesktop/Notifications", Some("org.freedesktop.Notifications"), "GetCapabilities", &())
                .await
            {
                Ok(reply) => reply.body().deserialize::<Vec<String>>().map(|c| c.iter().any(|x| x == "body-markup")).unwrap_or(true),
                // can't tell: escape (a stray "&lt;" beats a rendered <img>)
                Err(_) => true,
            }
        })
        .await;
    if markup { markup_escape(body) } else { body.to_string() }
}

/// Fire ONE notification via `org.freedesktop.Notifications.Notify`. Best-effort:
/// any error (bus gone mid-session, service crashed) is logged at most once and
/// swallowed — never panics, never blocks the sync loop meaningfully.
/// 10.0: incoming-call notification — a Join button that switches/joins, 25 s lifetime.
#[cfg(target_os = "linux")]
pub async fn notify_call(summary: &str, body: &str, room_id: &str) {
    let Some(conn) = connection().await else { return };
    ensure_action_listener(conn.clone()).await;
    let mut hints: HashMap<&str, zbus::zvariant::Value<'_>> = HashMap::new();
    hints.insert("desktop-entry", zbus::zvariant::Value::from("dev.tinhat.TinHat"));
    hints.insert("urgency", zbus::zvariant::Value::from(2u8));
    hints.insert("category", zbus::zvariant::Value::from("call.incoming"));
    let actions: Vec<&str> = vec!["default", "Join", "join", "Join"];
    let body = body_for_server(&conn, body).await;
    let body_tuple = ("Tin Hat", 0u32, "dev.tinhat.TinHat", summary, body.as_str(), actions, hints, CALL_EXPIRE_MS);
    match conn
        .call_method(Some("org.freedesktop.Notifications"), "/org/freedesktop/Notifications", Some("org.freedesktop.Notifications"), "Notify", &body_tuple)
        .await
    {
        Ok(reply) => {
            if let Ok(id) = reply.body().deserialize::<u32>() {
                pending_actions().lock().unwrap().insert(id, format!("join:{room_id}"));
            }
        }
        Err(e) => log_bus_unavailable_once(&e),
    }
}

/// Listen once for ActionInvoked on the session bus; map the notification id back to its action.
#[cfg(target_os = "linux")]
async fn ensure_action_listener(conn: zbus::Connection) {
    static STARTED: OnceLock<()> = OnceLock::new();
    if STARTED.set(()).is_err() {
        return;
    }
    crate::bridge::runtime().spawn(async move {
        use futures_util::StreamExt;
        const SVC: &str = "org.freedesktop.Notifications";
        let proxy = match zbus::Proxy::new(&conn, SVC, "/org/freedesktop/Notifications", SVC).await {
            Ok(p) => p,
            Err(e) => { eprintln!("[notify] action listener: proxy failed: {e}"); return; }
        };
        let mut stream = match proxy.receive_signal("ActionInvoked").await {
            Ok(s) => s,
            Err(e) => { eprintln!("[notify] action listener: signal stream failed: {e}"); return; }
        };
        // 10.6 (REACH A5.6, audit S2): a notification id is only meaningful to the service that
        // issued it. Drop an id's action when that notification closes, forget every id when the
        // service changes hands (a restarted service counts from 1 again), and ignore signals from
        // anyone but the current owner — else a click on ANOTHER app's notification that reused an
        // old id could run a stale "join:<room>".
        let mut closed = proxy.receive_signal("NotificationClosed").await.ok();
        let dbus = zbus::fdo::DBusProxy::new(&conn).await.ok();
        // Subscribe to owner changes BEFORE reading the owner: a restart in between is then a
        // queued change, not a missed one (Astra review).
        let mut owner_changes = match &dbus {
            Some(d) => d.receive_name_owner_changed().await.ok(),
            None => None,
        };
        let mut owner: Option<String> = match (&dbus, zbus::names::BusName::try_from(SVC)) {
            (Some(d), Ok(n)) => d.get_name_owner(n).await.ok().map(|o| o.to_string()),
            _ => None,
        };
        let from_owner = |msg: &zbus::Message, owner: &Option<String>| -> bool {
            match (msg.header().sender(), owner) {
                (Some(s), Some(o)) => s.as_str() == o,
                _ => true, // no owner known (no DBus proxy): keep the old behaviour
            }
        };
        loop {
            tokio::select! {
                // ActionInvoked precedes its NotificationClosed on the bus; polled first, so a
                // click is never eaten by the close that follows it (Astra review).
                biased;
                m = stream.next() => {
                    let Some(msg) = m else { break };
                    if !from_owner(&msg, &owner) {
                        continue;
                    }
                    if let Ok((id, key)) = msg.body().deserialize::<(u32, String)>() {
                        let action = pending_actions().lock().unwrap().remove(&id);
                        if let Some(a) = action {
                            if key == "join" || key == "default" {
                                toast_action(a);
                            }
                        }
                    }
                }
                m = async { match closed.as_mut() { Some(s) => s.next().await, None => std::future::pending().await } } => {
                    let Some(msg) = m else { closed = None; continue };
                    if !from_owner(&msg, &owner) {
                        continue;
                    }
                    if let Ok((id, _reason)) = msg.body().deserialize::<(u32, u32)>() {
                        // …and the entry outlives the close by a moment, for a click still in flight.
                        crate::bridge::runtime().spawn(async move {
                            tokio::time::sleep(Duration::from_secs(2)).await;
                            pending_actions().lock().unwrap().remove(&id);
                        });
                    }
                }
                c = async { match owner_changes.as_mut() { Some(s) => s.next().await, None => std::future::pending().await } } => {
                    let Some(sig) = c else { owner_changes = None; continue };
                    let Ok(args) = sig.args() else { continue };
                    if args.name().as_str() != SVC {
                        continue;
                    }
                    owner = args.new_owner().as_ref().map(|u| u.to_string());
                    pending_actions().lock().unwrap().clear();
                    eprintln!("[notify] notification service changed hands — pending actions cleared");
                }
            }
        }
    });
}

#[cfg(target_os = "linux")]
pub async fn notify_action(summary: &str, body: &str, icon: Option<String>, action: &str) {
    let _ = notify_action_id(summary, body, icon, action).await;
}

/// guests (docs/GUESTS.md): [`notify_action`] that hands back the notification's id, so it can be withdrawn later
/// ([`close_notification`] — "Ana is waiting to come in as a guest" closes once Ana was let in or turned away). None
/// when nothing was posted.
#[cfg(target_os = "linux")]
pub async fn notify_action_id(summary: &str, body: &str, icon: Option<String>, action: &str) -> Option<u32> {
    let conn = connection().await?;
    if !action.is_empty() {
        ensure_action_listener(conn.clone()).await;
    }

    // hints: tie the notification to our desktop entry so the shell can group it and
    // use our app icon even when app_icon resolution is fuzzy.
    let mut hints: HashMap<&str, zbus::zvariant::Value<'_>> = HashMap::new();
    hints.insert("desktop-entry", zbus::zvariant::Value::from("dev.tinhat.TinHat"));
    // The SENDER's photo, not our app icon — a toast that shows who is talking is the
    // whole point (QOL 1.5.0). image-path wins over app_icon in every shell that
    // implements it; where it doesn't, the app icon still shows.
    let icon_path = icon.map(|u| crate::platform::path_from_file_url(&u));
    if let Some(p) = icon_path.as_deref() {
        if !p.is_empty() {
            hints.insert("image-path", zbus::zvariant::Value::from(p));
        }
    }

    // Notify(app_name, replaces_id, app_icon, summary, body, actions, hints, expire_timeout)
    // "default" = the click on the toast itself (no button drawn); the listener maps the id back.
    let actions: Vec<&str> = if action.is_empty() { Vec::new() } else { vec!["default", "Open"] };
    let body = body_for_server(&conn, body).await;
    let body_tuple = (
        "Tin Hat", // app_name
        0u32,          // replaces_id (0 = new notification each time)
        "dev.tinhat.TinHat", // app_icon (10.1, TH-COPY-20: the rename missed this one)
        summary,
        body.as_str(),
        actions,
        hints,
        EXPIRE_MS, // expire_timeout (ms)
    );

    match conn
        .call_method(
            Some("org.freedesktop.Notifications"),
            "/org/freedesktop/Notifications",
            Some("org.freedesktop.Notifications"),
            "Notify",
            &body_tuple,
        )
        .await
    {
        Ok(reply) => {
            let id = reply.body().deserialize::<u32>().ok();
            if let Some(id) = id.filter(|_| !action.is_empty()) {
                pending_actions().lock().unwrap().insert(id, action.to_string());
            }
            id
        }
        Err(e) => {
            // Drop the cached connection so a transient bus blip can re-open next time,
            // but log only once so a permanently-broken service can't spam the log.
            log_bus_unavailable_once(&e);
            if let Ok(mut g) = conn_cell().try_lock() {
                *g = None;
            }
            None
        }
    }
}

/// guests: withdraw a notification [`notify_action_id`] posted (`CloseNotification`; one that already went is a no-op).
#[cfg(target_os = "linux")]
pub async fn close_notification(id: u32) {
    let Some(conn) = connection().await else { return };
    if let Err(e) = conn
        .call_method(
            Some("org.freedesktop.Notifications"),
            "/org/freedesktop/Notifications",
            Some("org.freedesktop.Notifications"),
            "CloseNotification",
            &(id,),
        )
        .await
    {
        eprintln!("[notify] close {id}: {e}");
    }
}

/// Decide what (if anything) to fire for this scan, under one lock, WITHOUT doing any
/// async / network work while holding it. Returns `Vec<(room_id, rise_amount)>` of the
/// rooms that crossed an edge and passed debounce + the global cap. The first scan
/// after launch seeds silently and returns empty.
fn plan_fires(observed: &[Observed], gen: u64) -> Vec<(String, u64, bool)> {
    let mut st = match detect().lock() {
        Ok(g) => g,
        Err(_) => return Vec::new(),
    };
    // 10.6: a scan that began for a session that has since ended never touches the state (the
    // next account's first scan must seed) and never fires.
    if crate::session::generation() != gen {
        return Vec::new();
    }
    plan_fires_in(&mut st, observed, Instant::now(), is_open_room)
}

/// The fire decision over its state (unit-tested). The very first scan seeds silently; a room
/// that appears LATER (a DM invite just accepted, a new room) starts from 0, so its first
/// notifying message fires like any other rise.
fn plan_fires_in(
    st: &mut DetectState,
    observed: &[Observed],
    now: Instant,
    is_open: impl Fn(&str) -> bool,
) -> Vec<(String, u64, bool)> {
    // First scan: SEED last-seen counts from the current values and notify for NONE
    // (else every already-unread room would fire at startup).
    if !st.seeded {
        for o in observed {
            st.last_highlight.insert(o.id.clone(), o.trigger);
            st.last_mode.insert(o.id.clone(), o.mode_tag);
        }
        st.seeded = true;
        return Vec::new();
    }

    // Prune the global rate window first so the cap reflects the last GLOBAL_WINDOW.
    st.recent_fires.retain(|t| now.duration_since(*t) < GLOBAL_WINDOW);

    let mut to_fire: Vec<(String, u64, bool)> = Vec::new();
    for o in observed {
        let id = &o.id;
        let prev = st.last_highlight.get(id).copied().unwrap_or(0);
        let prev_mode = st.last_mode.get(id).copied();
        // EDGE-TRIGGERED: only a RISE counts. Always record the new level so a later
        // rise edge is measured against the latest, never re-fires on a plateau.
        st.last_highlight.insert(id.clone(), o.trigger);
        st.last_mode.insert(id.clone(), o.mode_tag);
        // The room's mode just changed, so `trigger` switched metric: re-seed silently
        // rather than reporting the metric change as a burst of new messages.
        if prev_mode.is_some_and(|m| m != o.mode_tag) {
            continue;
        }
        if o.muted || o.trigger <= prev {
            continue; // muted, or no rise (or a clear) — nothing to notify
        }
        if !o.unread_on_server {
            continue; // read on another device already (the level is recorded above)
        }
        // Suppress the room the user is actively viewing.
        if is_open(id) {
            continue;
        }
        // Per-room debounce: collapse a burst into one notification per window.
        if let Some(t) = st.last_fired.get(id) {
            if now.duration_since(*t) < ROOM_DEBOUNCE {
                continue;
            }
        }
        // Global rate cap.
        if st.recent_fires.len() + to_fire.len() >= GLOBAL_CAP {
            break;
        }
        let rise = o.trigger.saturating_sub(prev);
        st.last_fired.insert(id.clone(), now);
        st.recent_fires.push(now);
        to_fire.push((id.clone(), rise, o.is_dm));
    }
    // Drop entries for rooms that vanished (left/forgotten) so the maps don't grow
    // unbounded over a long session.
    if observed.len() != st.last_highlight.len() {
        let live: std::collections::HashSet<&str> =
            observed.iter().map(|o| o.id.as_str()).collect();
        st.last_highlight.retain(|k, _| live.contains(k.as_str()));
        st.last_mode.retain(|k, _| live.contains(k.as_str()));
        st.last_fired.retain(|k, _| live.contains(k.as_str()));
    }
    to_fire
}

/// The effective notification mode for a room: the user's explicit per-room choice
/// (a SERVER push rule, so native and web agree) or our default when they haven't
/// chosen — DMs notify on every message, group rooms on mentions only. That default
/// is the Discord/TeamSpeak convention and deliberately NOT Matrix's server default
/// (which notifies for every message in every room, i.e. unusable in a busy room).
pub(crate) async fn effective_mode(
    client: &Client,
    room: &matrix_sdk::Room,
    is_dm: bool,
) -> RoomNotificationMode {
    mode_in(&client.notification_settings().await, room, is_dm).await
}

/// [`effective_mode`] against a NotificationSettings the caller made ONCE for a whole pass over the rooms (10.8, the CPU
/// track): `client.notification_settings()` reads the push ruleset from the store (a SQLite query on the blocking pool),
/// parses it and registers an event handler — per call. The room list, the nav (rows, rollups, DM strip, dock badge) and
/// the highlight scan called it once PER ROOM on every sync: ~47 store reads a second in a 1-peer rig call, and the
/// steady trickle kept ~70 idle blocking-pool threads alive for the whole session.
pub(crate) async fn mode_in(
    settings: &matrix_sdk::notification_settings::NotificationSettings,
    room: &matrix_sdk::Room,
    is_dm: bool,
) -> RoomNotificationMode {
    if let Some(m) = settings.get_user_defined_room_notification_mode(room.room_id()).await {
        return m;
    }
    if is_dm {
        RoomNotificationMode::AllMessages
    } else {
        RoomNotificationMode::MentionsAndKeywordsOnly
    }
}

/// Stable discriminant for [`DetectState::last_mode`].
pub(crate) fn mode_tag(m: RoomNotificationMode) -> u8 {
    match m {
        RoomNotificationMode::AllMessages => 1,
        RoomNotificationMode::MentionsAndKeywordsOnly => 2,
        RoomNotificationMode::Mute => 3,
    }
}

/// Wire name for the room-list JSON / QML menu ("all" | "mentions" | "mute").
pub fn mode_name(m: RoomNotificationMode) -> &'static str {
    match m {
        RoomNotificationMode::AllMessages => "all",
        RoomNotificationMode::MentionsAndKeywordsOnly => "mentions",
        RoomNotificationMode::Mute => "mute",
    }
}

/// Parse the wire name back. Unknown strings mean "no explicit choice".
pub fn mode_from_name(s: &str) -> Option<RoomNotificationMode> {
    match s {
        "all" => Some(RoomNotificationMode::AllMessages),
        "mentions" => Some(RoomNotificationMode::MentionsAndKeywordsOnly),
        "mute" => Some(RoomNotificationMode::Mute),
        _ => None,
    }
}

/// Scan every joined room's highlight count and fire desktop notifications for the
/// rooms that just crossed a highlight edge (and aren't the open room). Call this once
/// per sync from the sync loop — it's cheap (local store reads only) and receipt-free.
/// Persisted on/off switch for desktop notifications (default ON). Some people play with
/// the app on a second screen and want nothing popping over a fullscreen game; that is a
/// preference, not a bug, so it gets a real toggle rather than a code default.
fn notify_path() -> std::path::PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| {
            std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
        });
    base.join("tinhat/desktop-notifications")
}
fn enabled_cell() -> &'static Mutex<bool> {
    static E: OnceLock<Mutex<bool>> = OnceLock::new();
    E.get_or_init(|| {
        Mutex::new(std::fs::read_to_string(notify_path()).map(|s| s.trim() != "0").unwrap_or(true))
    })
}
/// Are desktop notifications enabled? (Persisted; default ON.)
pub fn enabled() -> bool {
    *enabled_cell().lock().unwrap()
}
/// Set + persist the desktop-notification preference. Takes effect immediately.
pub fn set_enabled(on: bool) {
    *enabled_cell().lock().unwrap() = on;
    let p = notify_path();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(&p, if on { "1" } else { "0" });
    eprintln!("[notify] desktop notifications {}", if on { "ON" } else { "OFF" });
}

pub async fn scan_highlights(client: &Client) {
    let gen = crate::session::generation();
    // Turned off by the user: still SEED the counts below (so switching it back on doesn't
    // dump a backlog), but never raise a notification.
    let notify_on = enabled();
    // Snapshot each room's mode-appropriate trigger count (local reads only — the push
    // ruleset is an in-memory RwLock, is_direct + counts come from the store).
    let mut observed: Vec<Observed> = Vec::new();
    let settings = client.notification_settings().await; // once per pass (see mode_in)
    for r in client.joined_rooms() {
        let is_dm = r.is_direct().await.unwrap_or(false);
        let mode = mode_in(&settings, &r, is_dm).await;
        // 10.5.1: our decryption-aware tally, NOT the server's unread_notification_counts()
        // (encryption-blind: it counted Element Call key events as messages — see the
        // module doc). The room list badges still show the server counts.
        let counts = tally_of(r.room_id().as_str());
        // ONLY mentions in a mentions-only room; EVERY message in an all-messages room
        // (which is what makes DMs actually notify — the old code read highlight_count
        // for everything, and Matrix only bumps that for mentions, so plain DMs were
        // silent forever).
        let trigger = match mode {
            RoomNotificationMode::AllMessages => counts.notify,
            RoomNotificationMode::MentionsAndKeywordsOnly => counts.highlight,
            RoomNotificationMode::Mute => counts.notify,
        };
        let server = r.unread_notification_counts();
        observed.push(Observed {
            id: r.room_id().to_string(),
            trigger,
            muted: matches!(mode, RoomNotificationMode::Mute),
            is_dm,
            mode_tag: mode_tag(mode),
            unread_on_server: unread_on_server(
                matches!(mode, RoomNotificationMode::MentionsAndKeywordsOnly),
                r.encryption_state().is_encrypted(),
                server.notification_count,
                server.highlight_count,
            ),
        });
    }

    {
        let joined: std::collections::HashSet<&str> = observed.iter().map(|o| o.id.as_str()).collect();
        if let Ok(mut t) = tallies().lock() {
            retain_joined(&mut t, &joined);
        }
    }
    let fires = plan_fires(&observed, gen);
    // (computed before the early return: the levels are recorded on every pass)
    let personal = {
        let mut last = last_highlight().lock().unwrap_or_else(|p| p.into_inner());
        let any = fires.iter().any(|(id, _, is_dm)| {
            *is_dm || tally_of(id).highlight > last.get(id).copied().unwrap_or(0)
        });
        // every room's level is recorded each pass, so only a NEW mention counts next time
        for o in &observed {
            last.insert(o.id.clone(), tally_of(&o.id).highlight);
        }
        any
    };
    if fires.is_empty() {
        return;
    }
    // 10.0: the notification sound rides the same decision as the toast (per-room modes, mute,
    // debounce); it plays even when desktop toasts are switched off.
    if let Some(t) = login_sink().lock().unwrap().clone() {
        let _ = t.queue(move |mut o| o.as_mut().toast_action(cxx_qt_lib::QString::from("sound:notify")));
    }
    // 10.6 (REACH A5.6): a new DM or mention flashes the taskbar button while the window is not
    // active (QML checks that) — with desktop notifications on, like the popup itself. A mention
    // is a rise of the room's highlight tally, whatever the room's mode (All-messages rooms too).
    if personal && notify_on {
        toast_action("alert".to_string());
    }

    // Build + send each notification (async; lock already released).
    for (room_id, rise, is_dm) in fires {
        let Ok(rid) = matrix_sdk::ruma::RoomId::parse(&room_id) else { continue };
        let Some(room) = client.get_room(&rid) else { continue };

        // Summary = room display name (best-effort, cache-only fallbacks).
        let summary = match room.display_name().await {
            Ok(d) => d.to_string(),
            Err(_) => room
                .name()
                .or_else(|| room.canonical_alias().map(|a| a.to_string()))
                .unwrap_or_else(|| room_id.clone()),
        };

        // Body: the latest event's text preview (cache-only, no network, no receipt),
        // or a generic "N new messages" fallback. A burst always collapses to ONE.
        // In a GROUP room the sender is prefixed ("Alex: ..."), because "who" is the
        // first thing you want from a room toast; in a DM the summary is already the
        // person, so prefixing would just say their name twice.
        let (body, icon) = latest_preview(&room, !is_dm).await.unwrap_or_else(|| {
            (
                if rise > 1 {
                    format!("{rise} new messages")
                } else {
                    "New message".to_string()
                },
                None,
            )
        });
        let body = if rise > 1 && !body.is_empty() {
            format!("{body}  (+{} more)", rise.saturating_sub(1))
        } else {
            body
        };

        // 10.6: the preview / avatar lookups above can take a while — a session that ended
        // meanwhile never gets its notification posted over the next one.
        if notify_on && crate::session::generation() == gen {
            // 10.1 (TH-GAP-06): clicking the toast opens the room (the most-performed gesture in a chat app).
            notify_action(&summary, &body, icon, &format!("open:{room_id}")).await;
        }
    }
}

// ─────────────── 10.6 (REACH A5.1): invites announce themselves ───────────────

/// The highlight tally each room had when it last fired (the flash follows real mentions).
fn last_highlight() -> &'static Mutex<HashMap<String, u64>> {
    static L: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The per-session invite seen-set (like the space invites' in AppCore).
#[derive(Default)]
pub(crate) struct InviteSeen {
    seeded: bool,
    seen: std::collections::HashSet<String>,
}

/// Which invites are NEW this pass. The first pass (the first live sync) only seeds: an invite
/// that was already waiting when Tin Hat started never alerts. A new one alerts once; seeing it
/// again never re-alerts; one that went away (accepted, declined) is forgotten, so a later
/// re-invite to the same room is new again. Pure; the web mirrors it (`invite_seen_once`).
pub(crate) fn new_invites(st: &mut InviteSeen, current: &[String]) -> Vec<String> {
    let now: std::collections::HashSet<&str> = current.iter().map(String::as_str).collect();
    st.seen.retain(|id| now.contains(id.as_str()));
    let mut fresh = Vec::new();
    for id in current {
        if st.seen.insert(id.clone()) && st.seeded {
            fresh.push(id.clone());
        }
    }
    st.seeded = true;
    fresh
}

fn invite_seen() -> &'static Mutex<InviteSeen> {
    static S: OnceLock<Mutex<InviteSeen>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(InviteSeen::default()))
}

/// Sign-out: the next account seeds afresh.
pub fn forget_invites() {
    if let Ok(mut st) = invite_seen().lock() {
        *st = InviteSeen::default();
    }
}

/// 10.6: sign-out (or a switch of account). Nothing of the old session may act for the next one:
/// its levels, tallies, debounce clocks, click actions and invite seen-set go, and the next
/// account's first scan seeds silently again. Call AFTER `session::clear_client()` (which ends the
/// sync task and bumps the generation that guards any scan still finishing).
pub fn forget_session() {
    if let Ok(mut d) = detect().lock() {
        *d = DetectState::default();
    }
    if let Ok(mut t) = tallies().lock() {
        t.clear();
    }
    if let Ok(mut l) = last_highlight().lock() {
        l.clear();
    }
    #[cfg(target_os = "linux")]
    if let Ok(mut p) = pending_actions().lock() {
        p.clear();
    }
    forget_invites();
}

/// Called from the live sync loop after every pass. A NEW DM invite (is_direct): the notify
/// sound + a taskbar flash (QML) and the OS notification "<inviter> wants to chat" — a click
/// selects People, where the invite waits with Accept / Decline. A NEW room invite: the in-app
/// toast (as for space invites), the sound, and "<inviter> invited you to <room>". Spaces keep the
/// rail's own toast. Desktop notifications off → the sound only; sounds off → silent (QML).
pub async fn scan_invites(client: &Client) {
    let gen = crate::session::generation();
    let invited = client.invited_rooms();
    let ids: Vec<String> = invited.iter().map(|r| r.room_id().to_string()).collect();
    let fresh = match invite_seen().lock() {
        Ok(mut st) => new_invites(&mut st, &ids),
        Err(_) => return,
    };
    if fresh.is_empty() {
        return;
    }
    let notify_on = enabled();
    for id in fresh {
        let Some(room) = invited.iter().find(|r| r.room_id().as_str() == id) else { continue };
        if room.is_space() {
            continue;
        }
        if crate::session::generation() != gen {
            return; // signed out meanwhile: the old session's invites stay quiet
        }
        let is_dm = crate::nav::invite_is_direct(room).await;
        let (inviter, icon) = match room.invite_details().await {
            Ok(inv) => {
                let name = inv
                    .inviter
                    .as_ref()
                    .map(|m| m.name().to_string())
                    .unwrap_or_else(|| inv.inviter_id.localpart().to_string());
                let icon = match inv.inviter.as_ref().and_then(|m| m.avatar_url()) {
                    Some(mxc) => crate::bridge::resolve_avatar(client, mxc).await,
                    None => None,
                };
                (name, icon)
            }
            Err(_) => ("Someone".to_string(), None),
        };
        if crate::session::generation() != gen {
            return;
        }
        if notify_on {
            toast_action("alert".to_string()); // the taskbar flash while the window is inactive (A5.6)
        }
        if is_dm {
            eprintln!("[notify] new DM invite");
            toast_action(format!("invite-dm:{id}"));
            if notify_on {
                notify_action(&format!("{inviter} wants to chat"), "", icon, &format!("invite:people:{id}")).await;
            }
        } else {
            let room_name = match room.display_name().await {
                Ok(matrix_sdk::RoomDisplayName::Empty) | Ok(matrix_sdk::RoomDisplayName::EmptyWas(_)) | Err(_) => {
                    "a chat".to_string()
                }
                Ok(d) => d.to_string(),
            };
            let text = format!("{inviter} invited you to {room_name}");
            eprintln!("[notify] new room invite");
            toast_action(format!("invite-room:{id}\t{text}"));
            if notify_on {
                notify_action(&text, "", icon, &format!("invite:home:{id}")).await;
            }
        }
    }
}

/// A short text preview of a room's latest event, from the local cache only (NO
/// network, NO read receipt). Returns `None` when there's no usable text body.
///
/// `latest_event()` is a local read of the cached `LatestEventValue`; only the
/// `Remote` variant carries a real received event whose `content.body` we preview.
async fn latest_preview(
    room: &matrix_sdk::Room,
    with_sender: bool,
) -> Option<(String, Option<String>)> {
    use matrix_sdk_base::latest_event::LatestEventValue;
    let LatestEventValue::Remote(event) = room.latest_event() else {
        return None;
    };
    let json = event.raw().deserialize_as::<serde_json::Value>().ok()?;
    let body = json
        .get("content")
        .and_then(|c| c.get("body"))
        .and_then(|b| b.as_str())?
        .trim();
    if body.is_empty() {
        return None;
    }
    let body = truncate(body, 160);
    // Sender lookup from the local member cache only — get_member_no_sync never hits the
    // network, so a cold cache degrades to the bare message instead of stalling a toast.
    // Done even for DMs (where we don't prefix the name), because we still want the face.
    let member = match json.get("sender").and_then(|s| s.as_str()) {
        Some(s) => match matrix_sdk::ruma::UserId::parse(s) {
            Ok(uid) => room.get_member_no_sync(&uid).await.ok().flatten().map(|m| (uid, m)),
            Err(_) => None,
        },
        None => None,
    };
    let icon = match &member {
        Some((_, m)) => match m.avatar_url() {
            Some(mxc) => {
                let client = room.client();
                crate::bridge::resolve_avatar(&client, mxc).await
            }
            None => None,
        },
        None => None,
    };
    if !with_sender {
        return Some((body, icon));
    }
    let name = match &member {
        Some((uid, m)) => m
            .display_name()
            .map(str::to_owned)
            .unwrap_or_else(|| uid.localpart().to_owned()),
        None => return Some((body, icon)),
    };
    Some((format!("{name}: {body}"), icon))
}

/// Trim a preview to `max` chars on a char boundary, adding an ellipsis if cut.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Headless gate for the NOTIFICATION DECISION TABLE (QOL Phase 1) — the part
/// that actually decides whether you get a toast, which no structural check covers:
///   TINHAT_NOTIFY_RULES=1 [TINHAT_USER=… TINHAT_PASS=…] ./tinhat
///
/// Part 1 is pure logic against [`plan_fires`] (no server needed). Part 2, when
/// credentials are supplied, round-trips a real per-room push rule through the
/// homeserver — proving "Mute" survives as SERVER state rather than a local flag.
pub async fn headless_rules_test() -> bool {
    let mut fails = 0;
    let mut check = |label: &str, ok: bool| {
        eprintln!("[notify-rules] {} {label}", if ok { "PASS" } else { "FAIL" });
        if !ok {
            fails += 1;
        }
    };
    let dm = "!dm:example.com";
    let grp = "!grp:example.com";
    let mut_ = "!muted:example.com";
    let o = |id: &str, trigger: u64, muted: bool, is_dm: bool, tag: u8| Observed {
        id: id.to_string(),
        trigger,
        muted,
        is_dm,
        mode_tag: tag,
        unread_on_server: true,
    };

    // 1. First scan seeds silently — no startup notification storm.
    let fires = plan_fires(&[o(dm, 0, false, true, 1), o(grp, 0, false, false, 2), o(mut_, 0, true, false, 3)], crate::session::generation());
    check("first scan seeds silently", fires.is_empty());

    // 2. A DM message raises notification_count -> toast. THE ORIGINAL REGRESSION:
    //    this used to read highlight_count, which plain DMs never bump.
    let fires = plan_fires(&[o(dm, 1, false, true, 1), o(grp, 0, false, false, 2), o(mut_, 0, true, false, 3)], crate::session::generation());
    check("plain DM message fires", fires.iter().any(|(id, _, is_dm)| id == dm && *is_dm));

    // 3. A non-mention group message leaves the mentions-only trigger flat -> silent.
    check("group chatter stays silent", !fires.iter().any(|(id, _, _)| id == grp));

    // 4. Debounce: a second DM rise inside the window collapses into the first toast.
    let fires = plan_fires(&[o(dm, 2, false, true, 1), o(grp, 0, false, false, 2), o(mut_, 0, true, false, 3)], crate::session::generation());
    check("burst debounced to one", fires.is_empty());

    // 5. A muted room never fires no matter how far its count climbs.
    let fires = plan_fires(&[o(dm, 2, false, true, 1), o(grp, 0, false, false, 2), o(mut_, 99, true, false, 3)], crate::session::generation());
    check("muted room never fires", !fires.iter().any(|(id, _, _)| id == mut_));

    // 6. Switching a room's mode changes which metric `trigger` carries; that jump must
    //    re-seed silently instead of reading as a burst of new messages.
    let fires = plan_fires(&[o(grp, 37, false, false, 1)], crate::session::generation());
    check("mode change re-seeds silently", fires.is_empty());
    let fires = plan_fires(&[o(grp, 38, false, false, 1)], crate::session::generation());
    check("fires again after re-seed", fires.iter().any(|(id, _, _)| id == grp));

    // 7. 10.5.1: a rise the server no longer counts (read on another device while we were
    //    offline) records the level silently; the next real message fires for itself alone.
    let read = "!read-elsewhere:example.com";
    let fires = plan_fires(&[Observed { unread_on_server: false, ..o(read, 3, false, false, 1) }], crate::session::generation());
    check("read elsewhere stays silent", fires.is_empty());
    let fires = plan_fires(&[o(read, 4, false, false, 1)], crate::session::generation());
    check("the next real message fires (rise 1)", fires.iter().any(|(id, rise, _)| id == read && *rise == 1));

    // Part 2 (optional): the per-room level is SERVER state, so web/mobile agree.
    match (std::env::var("TINHAT_USER"), std::env::var("TINHAT_PASS")) {
        (Ok(user), Ok(pass)) => {
            // no server of the harness's own: TINHAT_HS names it (an empty address fails the login below, plainly)
            let hs = crate::bridge::env_homeserver().unwrap_or_else(|e| {
                eprintln!("[notify] part 2: {e}");
                String::new()
            });
            match crate::bridge::headless_login(&hs, &user, &pass).await {
                Ok(client) => {
                    use matrix_sdk::ruma::api::client::room::create_room;
                    let req = create_room::v3::Request::new();
                    match client.create_room(req).await {
                        Ok(room) => {
                            let id = room.room_id().to_owned();
                            let s = client.notification_settings().await;
                            for want in [
                                RoomNotificationMode::Mute,
                                RoomNotificationMode::MentionsAndKeywordsOnly,
                                RoomNotificationMode::AllMessages,
                            ] {
                                let set = s.set_room_notification_mode(&id, want).await;
                                let got = s.get_user_defined_room_notification_mode(&id).await;
                                check(
                                    &format!("push rule round-trips as {}", mode_name(want)),
                                    set.is_ok() && got == Some(want),
                                );
                            }
                            let _ = client.get_room(&id).map(|r| async move { r.leave().await });
                        }
                        Err(e) => check(&format!("create scratch room ({e})"), false),
                    }
                }
                Err(e) => check(&format!("login ({e})"), false),
            }
        }
        _ => eprintln!("[notify-rules] (skipped live push-rule round trip: no TINHAT_USER/TINHAT_PASS)"),
    }

    eprintln!(
        "[notify-rules] VERDICT: {}",
        if fails == 0 { "PASS" } else { "FAIL" }
    );
    fails == 0
}

/// Headless verification hook (no live second sender needed):
///   TINHAT_NOTIFY_TEST=1 ./tinhat
/// Fires one sample desktop notification, then returns. Modeled on the other TINHAT_*
/// probes in `main()`. Reports a one-line verdict so it's testable headless.
pub async fn headless_test() {
    eprintln!("[notify] TINHAT_NOTIFY_TEST: firing one sample notification…");
    #[cfg(target_os = "linux")]
    match connection().await {
        Some(_) => {
            notify("Tin Hat test", "Desktop notifications are working ✓", None).await;
            eprintln!("[notify] VERDICT: sent (check your notification area)");
        }
        None => {
            eprintln!("[notify] VERDICT: session bus unavailable (no notification daemon?)");
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        notify("Tin Hat test", "Desktop notifications are working ✓", None).await;
        eprintln!("[notify] VERDICT: WinRT toast dispatched (check the notification area; a toast error would have logged above)");
    }
}

#[cfg(test)]
mod tests {
    //! 10.5.1: the notification trigger must judge an event by what it DECRYPTS to. These run the
    //! real ruma push-rule engine over a typical account's ruleset shape: the server defaults +
    //! the 15 quiet overrides (quietrules.rs) + an All-messages room rule on a busy group room.
    use super::*;

    /// security review 2026-10-26: a message body never reaches a markup-rendering notification server as markup
    #[test]
    fn notification_bodies_are_escaped_for_markup_servers() {
        assert_eq!(super::markup_escape("<img src=\"https://evil.example/p\">hi & bye"), "&lt;img src=\"https://evil.example/p\"&gt;hi &amp; bye");
        assert_eq!(super::markup_escape("plain"), "plain");
        assert_eq!(super::markup_escape("&lt;"), "&amp;lt;");
    }
    use matrix_sdk::ruma::push::{
        Action, EventMatchConditionData, NewConditionalPushRule, NewPushRule, NewSimplePushRule, PushCondition,
        PushConditionRoomCtx, Ruleset, Tweak,
    };
    use matrix_sdk::ruma::{owned_room_id, owned_user_id, serde::Raw, uint};

    const GC: &str = "!gamenight:example.com";

    fn ruleset() -> Ruleset {
        let me = owned_user_id!("@logan:example.com");
        let mut rs = Ruleset::server_default(&me);
        for (suffix, pattern) in crate::quietrules::QUIET_TYPES {
            let cond = PushCondition::EventMatch(EventMatchConditionData::new("type".to_owned(), (*pattern).to_owned()));
            let rule = NewConditionalPushRule::new(crate::quietrules::rule_id(suffix), vec![cond], Vec::new());
            rs.insert(NewPushRule::Override(rule), None, None).unwrap();
        }
        let room = NewSimplePushRule::new(owned_room_id!("!gamenight:example.com"), vec![
            Action::Notify,
            Action::SetTweak(Tweak::Sound("default".into())),
        ]);
        rs.insert(NewPushRule::Room(room), None, None).unwrap();
        rs
    }

    fn actions(rs: &Ruleset, ev: serde_json::Value) -> Vec<Action> {
        let ctx = PushConditionRoomCtx::new(
            owned_room_id!("!gamenight:example.com"),
            uint!(17),
            owned_user_id!("@logan:example.com"),
            "Blake".to_owned(),
        );
        let raw: Raw<serde_json::Value> = Raw::new(&ev).unwrap();
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(async { rs.get_actions(&raw, &ctx).await.to_vec() })
    }

    fn ev(sender: &str, ty: &str, content: serde_json::Value) -> serde_json::Value {
        serde_json::json!({ "type": ty, "sender": sender, "event_id": "$e", "origin_server_ts": 1u64, "room_id": GC, "content": content })
    }

    /// What the SDK hands `record_sync`: only events whose actions notify (`sync::Notification`).
    fn feed(room: &str, acts: &[Vec<Action>]) {
        let listed: Vec<&[Action]> = acts.iter().filter(|a| a.iter().any(|x| x.should_notify())).map(|a| a.as_slice()).collect();
        tally_actions(room, listed);
    }

    #[test]
    fn a_call_key_event_is_quiet_once_decrypted_but_the_server_counts_its_wrapper() {
        let rs = ruleset();
        let wrapper = ev("@simon_r:example.com", "m.room.encrypted", serde_json::json!({
            "algorithm": "m.megolm.v1.aes-sha2", "ciphertext": "AwgA", "device_id": "DVQPRMFIED",
            "sender_key": "k", "session_id": "s" }));
        let decrypted = ev("@simon_r:example.com", "io.element.call.encryption_keys", serde_json::json!({
            "keys": [{ "index": 0, "key": "a2V5" }], "device_id": "DVQPRMFIED", "call_id": "" }));
        // Synapse's view (the old trigger): the room rule matches the opaque wrapper -> notify + sound.
        assert!(actions(&rs, wrapper).iter().any(|a| a.should_notify()), "the wrapper is what chimed");
        // Our view after decryption (the new trigger): the quiet override wins -> nothing.
        assert!(actions(&rs, decrypted).is_empty(), "a decrypted media-key event must never notify");
    }

    #[test]
    fn the_tally_rises_for_messages_and_mentions_not_for_call_housekeeping() {
        let rs = ruleset();
        let room = "!tally-test:example.com";
        let key = actions(&rs, ev("@dale:example.com", "io.element.call.encryption_keys", serde_json::json!({ "keys": [] })));
        let member = actions(&rs, ev("@dale:example.com", "org.matrix.msc3401.call.member", serde_json::json!({})));
        let msg = actions(&rs, ev("@dale:example.com", "m.room.message", serde_json::json!({ "msgtype": "m.text", "body": "gg" })));
        let mention = actions(&rs, ev("@dale:example.com", "m.room.message", serde_json::json!({
            "msgtype": "m.text", "body": "logan look", "m.mentions": { "user_ids": ["@logan:example.com"] } })));
        let own = actions(&rs, ev("@logan:example.com", "m.room.message", serde_json::json!({ "msgtype": "m.text", "body": "me" })));
        feed(room, &[key, member, own]);
        assert_eq!(tally_of(room), Tally::default(), "housekeeping and our own events leave the tally flat");
        feed(room, &[msg]);
        assert_eq!(tally_of(room), Tally { notify: 1, highlight: 0 });
        feed(room, &[mention]);
        assert_eq!(tally_of(room), Tally { notify: 2, highlight: 1 }, "a mention in an encrypted room now counts");
    }

    #[test]
    fn badge_mentions_in_encrypted_rooms_come_from_this_device_while_the_server_says_unread() {
        assert_eq!(badge_highlight(false, 5, 0, 3), 0, "unencrypted: the server sees mentions itself");
        assert_eq!(badge_highlight(false, 5, 2, 0), 2);
        assert_eq!(badge_highlight(true, 5, 0, 2), 2, "encrypted: the server's 0 is blindness, this device's 2 count");
        assert_eq!(badge_highlight(true, 5, 3, 1), 3, "never below the server");
        assert_eq!(badge_highlight(true, 0, 0, 2), 0, "read on another device (the server's count went to 0): no badge");
        assert_eq!(badge_highlight(true, 5, 0, 0), 0);
    }

    #[test]
    fn a_room_read_elsewhere_never_chimes_but_encrypted_mentions_still_can() {
        // All messages: the server counts every message (and every key event) — its count is the gate.
        assert!(unread_on_server(false, true, 1, 0), "an unread message: fire");
        assert!(!unread_on_server(false, true, 0, 0), "read on the phone while we were offline: silent");
        assert!(!unread_on_server(false, false, 0, 0));
        // Mentions only, unencrypted: the server sees the mention (highlight) and plain messages.
        assert!(unread_on_server(true, false, 0, 1));
        assert!(unread_on_server(true, false, 3, 0));
        assert!(!unread_on_server(true, false, 0, 0), "read elsewhere: silent");
        // Mentions only, ENCRYPTED: the server is blind, the decryption-aware tally alone decides.
        assert!(unread_on_server(true, true, 0, 0));
    }

    #[test]
    fn an_invite_or_a_left_room_leaves_no_stale_tally() {
        // A local map: the global one is shared with the other tests running in parallel.
        let mut t: HashMap<String, Tally> = HashMap::new();
        t.insert("!joined:x".into(), Tally { notify: 3, highlight: 0 });
        t.insert("!invited:x".into(), Tally { notify: 1, highlight: 0 }); // .m.rule.invite_for_me
        t.insert("!left:x".into(), Tally { notify: 7, highlight: 1 });
        let joined: std::collections::HashSet<&str> = ["!joined:x"].into_iter().collect();
        retain_joined(&mut t, &joined);
        assert_eq!(t.len(), 1);
        assert_eq!(t.get("!joined:x"), Some(&Tally { notify: 3, highlight: 0 }));
    }
}

/// 10.6 (Astra's web review, item 16 — checked on the desktop): the baseline is taken ONCE.
#[cfg(test)]
mod baseline_tests {
    use super::{plan_fires_in, DetectState, Observed};
    use std::time::{Duration, Instant};

    fn room(id: &str, trigger: u64, is_dm: bool) -> Observed {
        Observed { id: id.into(), trigger, muted: false, is_dm, mode_tag: 1, unread_on_server: true }
    }

    #[test]
    fn a_room_that_appears_after_the_baseline_notifies_its_first_message() {
        let mut st = DetectState::default();
        let t0 = Instant::now();
        // the baseline (first live sync): everything already unread stays quiet
        assert!(plan_fires_in(&mut st, &[room("!old", 4, false)], t0, |_| false).is_empty());
        // a DM invite accepted later arrives with its first message: that fires
        let later = t0 + Duration::from_secs(30);
        let fired = plan_fires_in(&mut st, &[room("!old", 4, false), room("!newdm", 1, true)], later, |_| false);
        assert_eq!(fired, vec![("!newdm".to_string(), 1, true)]);
        // …once: the same level on the next pass is no rise
        assert!(plan_fires_in(&mut st, &[room("!old", 4, false), room("!newdm", 1, true)], later + Duration::from_secs(30), |_| false).is_empty());
        // a new session starts from a fresh state: its first scan is the baseline again
        let mut next = DetectState::default();
        assert!(plan_fires_in(&mut next, &[room("!b", 9, true)], later, |_| false).is_empty());
    }
}

/// REACH A5.1 — the web mirrors this name (`invite_seen_once`).
#[cfg(test)]
mod invite_tests {
    use super::{new_invites, InviteSeen};

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn invite_seen_once() {
        let mut st = InviteSeen::default();
        // an invite present at the seed (the first live sync) never alerts
        assert!(new_invites(&mut st, &ids(&["!old"])).is_empty());
        assert!(new_invites(&mut st, &ids(&["!old"])).is_empty());
        // a new one alerts once…
        assert_eq!(new_invites(&mut st, &ids(&["!old", "!new"])), ids(&["!new"]));
        // …and seeing it again never re-alerts
        assert!(new_invites(&mut st, &ids(&["!old", "!new"])).is_empty());
        assert!(new_invites(&mut st, &ids(&["!new", "!old"])).is_empty());
        // accepted / declined, then invited again later: that is a new invite
        assert!(new_invites(&mut st, &ids(&["!old"])).is_empty());
        assert_eq!(new_invites(&mut st, &ids(&["!old", "!new"])), ids(&["!new"]));
    }
}
