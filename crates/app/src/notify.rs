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
/// single "N new messages" notification instead of one-per-message spam. A rise inside the
/// window is HELD, not dropped (A03.0): the scan re-runs when the window ends and the held
/// messages get one trailing notification (shared vectors testvectors/notify-burst.json).
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
/// Bumped whenever org.freedesktop.Notifications changes hands: an id the OLD server answered after the change must
/// never land in the map (the new server may reuse it for another app's notification — Astra review, notif-actions).
#[cfg(target_os = "linux")]
static OWNER_EPOCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
#[cfg(target_os = "linux")]
fn owner_epoch() -> u64 {
    OWNER_EPOCH.load(std::sync::atomic::Ordering::SeqCst)
}
/// Remember a posted notification's action under the server that issued its id (the Notify reply's sender, a unique
/// bus name; 6-4): an id is only meaningful to that server, so a click is looked up by its signal's SENDER — a server
/// that took the name over (and counts its own ids) can never trigger it. An entry of a server that went away is
/// dropped at the next owner change; the map is bounded for a rare late reply after that.
#[cfg(target_os = "linux")]
fn remember(issuer: &str, id: u32, action: String) {
    const MAX: usize = 512;
    let mut g = pending_actions().lock().unwrap();
    if g.len() >= MAX {
        let keep = known_owner().lock().unwrap().clone();
        // a soft bound: the current owner's and this issuer's entries stay (the listener may be behind a takeover)
        g.retain(|(o, _), _| keep.as_deref() == Some(o.as_str()) || o == issuer);
    }
    g.insert((issuer.to_string(), id), action);
}

/// The server that answered a Notify (its unique name), else the destination it was sent to.
#[cfg(target_os = "linux")]
fn issuer_of(reply: &zbus::Message, dest: &str) -> String {
    reply.header().sender().map(|s| s.to_string()).unwrap_or_else(|| dest.to_string())
}
/// The posted notifications' actions, by (issuing server's unique name, notification id).
#[cfg(target_os = "linux")]
fn pending_actions() -> &'static Mutex<HashMap<(String, u32), String>> {
    static P: OnceLock<Mutex<HashMap<(String, u32), String>>> = OnceLock::new();
    P.get_or_init(|| Mutex::new(HashMap::new()))
}
/// The owner of org.freedesktop.Notifications as the action listener last saw it (None = not known yet).
#[cfg(target_os = "linux")]
fn known_owner() -> &'static Mutex<Option<String>> {
    static O: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    O.get_or_init(|| Mutex::new(None))
}
/// The name changed hands (or its first owner became known): only the new owner's actions stay, the epoch moves on
/// and its capabilities are asked again. A post that started the service registers under the new owner before or
/// after this runs and keeps its action either way (6-4).
#[cfg(target_os = "linux")]
fn owner_changed(new_owner: Option<String>) {
    let mut g = pending_actions().lock().unwrap();
    OWNER_EPOCH.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    g.retain(|(o, _), _| new_owner.as_deref() == Some(o.as_str()));
    *known_owner().lock().unwrap() = new_owner;
    *caps_cell().lock().unwrap() = None; // the new server may do different things
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

/// A view (`RoomController::open`) now shows `room_id`'s `thread` ("" = its main timeline). If the view is on screen,
/// the notifier follows it. 1-3: the room and the thread change under the attendance lock together (a reader never sees
/// the new room with the old view's thread, or the reverse), and its mentions are then seen by what it shows.
pub fn update_view(token: u64, room_id: &str, thread: &str) {
    let mut attending = false;
    if let Ok(mut g) = attended().lock() {
        if let Ok(mut v) = view_threads().lock() {
            if thread.is_empty() {
                v.remove(&token);
            } else {
                v.insert(token, thread.to_string());
            }
        }
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

/// threads (docs/THREADS.md): the thread each view shows (its RoomController's threadRoot; absent = the main timeline).
fn view_threads() -> &'static Mutex<HashMap<u64, String>> {
    static R: OnceLock<Mutex<HashMap<u64, String>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The views showing `room_id` and the thread each shows, read under the attendance lock (lock order: attended, then
/// view_threads — [`update_view`] writes in the same order).
fn room_views(room_id: &str) -> (Vec<u64>, HashMap<u64, String>) {
    let Ok(g) = attended().lock() else { return (Vec::new(), HashMap::new()) };
    let tokens: Vec<u64> = g.iter().filter(|(_, r)| r.as_str() == room_id).map(|(t, _)| *t).collect();
    let views = view_threads().lock().map(|v| tokens.iter().filter_map(|t| v.get(t).map(|r| (*t, r.clone()))).collect()).unwrap_or_default();
    (tokens, views)
}

/// threads (review #15): is the room on screen for its newest message? A thread reply is only seen when a view shows
/// that very thread (thread replies don't show in the main timeline), so a room open on its main timeline still
/// notifies for one.
fn room_attended_for_latest(room_id: &str) -> bool {
    // review #5: the thread of the newest event that NOTIFIED (record_sync), not of the room's latest event
    room_attended_for_thread(room_id, &note_thread(room_id))
}

/// threads: is `thread` ("" = the main timeline) of `room_id` on screen in this app? The room open in a view, and —
/// for a thread — a view showing that very thread (a thread reply never shows in the main timeline).
pub(crate) fn room_attended_for_thread(room_id: &str, thread: &str) -> bool {
    let (tokens, views) = room_views(room_id);
    attended_for_thread_in(thread, &tokens, &views)
}

/// 1-3: the timelines of `room_id` on screen in this app ("" = its main timeline, else a thread's root): what a room
/// coming on screen marks seen in the Mentions list (threads::seen_on_attend).
pub(crate) fn shown_threads(room_id: &str) -> Vec<String> {
    let (tokens, views) = room_views(room_id);
    let mut out: Vec<String> = tokens.iter().map(|t| views.get(t).cloned().unwrap_or_default()).collect();
    out.sort();
    out.dedup();
    out
}

/// The pure rule of [`room_attended_for_thread`]: `tokens` = the views showing the room, `views` = the thread each view
/// shows (absent = its main timeline). (1-1: closing a toast uses it too — the room open is not enough for a thread's.)
pub(crate) fn attended_for_thread_in(thread: &str, tokens: &[u64], views: &HashMap<u64, String>) -> bool {
    if tokens.is_empty() {
        return false;
    }
    // review (1-1/1-3): the main timeline too needs a view that shows it (a thread's view alone doesn't)
    tokens.iter().any(|t| views.get(t).map(String::as_str).unwrap_or("") == thread)
}

/// 1-1: per room, the thread of the message toast now on screen ("" = the main timeline): what closing it waits for.
fn posted_threads() -> &'static Mutex<HashMap<String, String>> {
    static P: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    P.get_or_init(|| Mutex::new(HashMap::new()))
}
/// R1-2: a message toast for `room` about `thread` was posted (`posted`) or failed: only a posted one moves the room's
/// toast thread (a failed replacement leaves the previous toast on screen, so closing must still wait for ITS thread).
pub(crate) fn note_posted(map: &mut HashMap<String, String>, room: &str, thread: &str, posted: bool) {
    if posted {
        map.insert(room.to_string(), thread.to_string());
    }
}
fn posted_thread(room_id: &str) -> String {
    posted_threads().lock().ok().and_then(|g| g.get(room_id).cloned()).unwrap_or_else(|| note_thread(room_id))
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
        let here = unseen_mentions_here(room).await;
        c.highlight_count = badge_highlight(true, c.notification_count, c.highlight_count, here);
    }
    c
}

/// 1-2 (threads): this device's unseen mentions in `room` that are newer than this account's read point IN THEIR OWN
/// TIMELINE — the unthreaded receipt (Mark as read, older apps) and the "main" one for a main-timeline mention, the
/// unthreaded one and that thread's for a thread mention ([`count_unseen_by_thread`]). Reading the main timeline (both apps
/// send "main" receipts now) clears main mentions; it never clears a thread's.
async fn unseen_mentions_here(room: &matrix_sdk::Room) -> u64 {
    use matrix_sdk::ruma::events::receipt::ReceiptThread;
    let list = crate::mentions_ctl::unseen_list(room.room_id().as_str());
    if list.is_empty() {
        return 0;
    }
    // review: a mention whose thread isn't known yet (recorded in an earlier run) is neither main nor a thread's: it
    // keeps today's rule (only the unthreaded receipt reads it) while it is looked up in the background — no badge
    // waits on the network
    let known = crate::mentions_ctl::mention_threads().lock().map(|g| g.clone()).unwrap_or_default();
    let items: Vec<(f64, Option<String>)> = list.iter().map(|(e, t)| (*t, known.get(e).cloned())).collect();
    let unknown: Vec<String> = list.iter().filter(|(e, _)| !known.contains_key(e)).map(|(e, _)| e.clone()).collect();
    if !unknown.is_empty() {
        crate::mentions_ctl::resolve_threads_later(room.room_id().as_str(), unknown);
    }
    let unthreaded = own_read_ts(room, ReceiptThread::Unthreaded).await;
    let main = own_read_ts(room, ReceiptThread::Main).await;
    let mut threads: HashMap<String, u64> = HashMap::new();
    // review: the threads' receipts are looked up within one short deadline (a thread not answered by then keeps its
    // mention highlighted: no read point known) — the badge never waits minutes on a cold cache
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
    for th in items.iter().filter_map(|(_, th)| th.as_ref()) {
        if th.is_empty() || threads.contains_key(th) {
            continue;
        }
        let Ok(root) = matrix_sdk::ruma::OwnedEventId::try_from(th.as_str()) else { continue };
        let fut: std::pin::Pin<Box<dyn std::future::Future<Output = u64> + Send + '_>> = Box::pin(own_read_ts(room, ReceiptThread::Thread(root)));
        match tokio::time::timeout_at(deadline, fut).await {
            Ok(ts) => {
                threads.insert(th.clone(), ts);
            }
            Err(_) => break,
        }
    }
    count_unseen_by_thread(&items, unthreaded, main, &threads)
}

/// 1-2: the unseen mentions `(ts, thread)` newer than their own read point: max(unthreaded, main) for a main-timeline
/// mention, max(unthreaded, that thread's) for a thread mention, the unthreaded one alone for a mention whose thread isn't
/// known (None). Never a maximum over all threads (that would clear a thread's mention by reading the main timeline).
pub(crate) fn count_unseen_by_thread(mentions: &[(f64, Option<String>)], unthreaded: u64, main: u64, threads: &HashMap<String, u64>) -> u64 {
    mentions
        .iter()
        .filter(|(t, th)| {
            let own = match th.as_deref() {
                Some("") => main,
                Some(th) => threads.get(th).copied().unwrap_or(0),
                None => 0,
            };
            *t > unthreaded.max(own) as f64
        })
        .count() as u64
}

/// When this account last read `room` in one receipt thread (unthreaded, "main" or a thread's root), on any device, in
/// ms; 0 = none known. A mention older than that was read. Uses the send time of the event each own read receipt
/// (public or private) points at, not when the receipt was sent: marking an older message read later on must leave a
/// newer mention highlighted. See [`read_point`] for an event that can't be looked up.
async fn own_read_ts(room: &matrix_sdk::Room, thread: matrix_sdk::ruma::events::receipt::ReceiptThread) -> u64 {
    use matrix_sdk::ruma::events::receipt::ReceiptType;
    let mut seen = Vec::with_capacity(2);
    for ty in [ReceiptType::Read, ReceiptType::ReadPrivate] {
        if let Ok(Some((event_id, r))) = room.load_user_receipt(ty, &thread, room.own_user_id()).await {
            let sent = r.ts.map_or(0, |ts| u64::from(ts.get()));
            seen.push((receipt_event_ts(room, &event_id).await, sent));
        }
    }
    read_point(&seen)
}

/// The room's read point from its receipts: `(the target event's send time when known, the receipt's own time)`.
/// A target that can't be looked up counts at the receipt's time, which can hide a still-unread mention from the badge
/// (the room still shows unread, and its toast already fired); erring that way never brings back mentions read elsewhere.
pub(crate) fn read_point(receipts: &[(Option<u64>, u64)]) -> u64 {
    receipts.iter().map(|(event, sent)| event.unwrap_or(*sent)).max().unwrap_or(0)
}

/// Send times of receipt targets: an event's never changes, and badges are recounted on every room-list refresh.
/// A lookup that failed waits [`RECEIPT_EVENT_RETRY`] before the next try, so a missing event costs no request per refresh.
#[derive(Default)]
struct ReceiptEventTimes {
    known: HashMap<String, u64>,
    failed: HashMap<String, Instant>,
}

const RECEIPT_EVENT_RETRY: Duration = Duration::from_secs(300);
const RECEIPT_EVENT_CACHE_MAX: usize = 2048;

fn receipt_event_times() -> &'static Mutex<ReceiptEventTimes> {
    static T: OnceLock<Mutex<ReceiptEventTimes>> = OnceLock::new();
    T.get_or_init(|| Mutex::new(ReceiptEventTimes::default()))
}

async fn receipt_event_ts(room: &matrix_sdk::Room, event_id: &matrix_sdk::ruma::EventId) -> Option<u64> {
    let key = event_id.as_str();
    let mut cooling = false;
    if let Ok(t) = receipt_event_times().lock() {
        if let Some(ts) = t.known.get(key) {
            return Some(*ts);
        }
        cooling = t.failed.get(key).is_some_and(|at| at.elapsed() < RECEIPT_EVENT_RETRY);
    }
    let event = if cooling {
        // no request while cooling down, but the event may have arrived locally since (sync, scrollback)
        let (cache, _handles) = room.event_cache().await.ok()?;
        Some(cache.find_event(event_id).await.ok()??)
    } else {
        let config = matrix_sdk::config::RequestConfig::short_retry().timeout(Duration::from_secs(5));
        room.load_or_fetch_event(event_id, Some(config)).await.ok()
    };
    let found = event.and_then(|e| e.timestamp()).map(|ts| u64::from(ts.get()));
    if let Ok(mut t) = receipt_event_times().lock() {
        if t.known.len() >= RECEIPT_EVENT_CACHE_MAX {
            t.known.clear();
        }
        if t.failed.len() >= RECEIPT_EVENT_CACHE_MAX {
            t.failed.clear();
        }
        match found {
            Some(ts) => {
                t.failed.remove(key);
                t.known.insert(key.to_string(), ts);
            }
            None => {
                t.failed.insert(key.to_string(), Instant::now());
            }
        }
    }
    found
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
    /// A03.0: set by each plan — the earliest moment a rise held back by the per-room window or
    /// the global cap may fire (the scan must run again then); None = nothing held.
    wake_at: Option<Instant>,
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
        // docs/NOTIF-ACTIONS.md: the newest event that notified = what Mark as read targets (the SDK's latest event is
        // not known in an encrypted room; the event id is outside the encryption)
        // Messages-style: its sender + text too — the SDK decrypted it before the push rules ran, and the room's cached
        // "latest event" (matrix-sdk 0.19) stays empty unless the room is subscribed to, so this is the toast's preview
        let newest = list.iter().rev().filter(|n| n.actions.iter().any(|a| a.should_notify())).find_map(|n| match &n.event {
            matrix_sdk_base::deserialized_responses::RawAnySyncOrStrippedTimelineEvent::Sync(raw) => {
                // threads: the thread it is in (the relation is outside the encryption)
                raw.deserialize_as::<serde_json::Value>().ok().and_then(|v| {
                    let thread = tinhat_widgets::threads::thread_of(v.get("content").unwrap_or(&serde_json::Value::Null));
                    notified_event(&v).map(|n| (n, thread))
                })
            }
            _ => None,
        });
        if let Some((n, thread)) = newest {
            // review #5/#6: what notified decides "is it on screen?" and where a click goes, not the room's latest event.
            // 12.1.16 review: the event and its thread change together (under last_notified's lock; a scan reads the pair
            // the same way), so a toast never gets one event's text with another's thread
            if let Ok(mut m) = last_notified().lock() {
                m.insert(room_id.to_string(), n);
                note_thread_set(room_id.as_str(), &thread);
            }
        }
    }
}

/// A room's newest event that notified: its id (what Mark as read targets), sender and text (the toast's preview).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Notified {
    pub id: String,
    pub sender: Option<String>,
    pub body: Option<String>,
}

/// Messages-style: what a notification needs from a (decrypted) timeline event — its id, sender and, for an event with
/// a text body, that text trimmed (None for an empty body or an event without one). None without an event id.
pub(crate) fn notified_event(ev: &serde_json::Value) -> Option<Notified> {
    let id = ev.get("event_id")?.as_str()?.to_string();
    let sender = ev.get("sender").and_then(|s| s.as_str()).map(str::to_owned);
    let body = ev
        .get("content")
        .and_then(|c| c.get("body"))
        .and_then(|b| b.as_str())
        .filter(|b| preview_text(b, None).is_some())
        .map(str::to_owned);
    Some(Notified { id, sender, body })
}

/// Each room's newest event that notified under the push rules (from [`record_sync`]); cleared with the session.
fn last_notified() -> &'static Mutex<HashMap<String, Notified>> {
    static L: OnceLock<Mutex<HashMap<String, Notified>>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(HashMap::new()))
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

/// The body as the notification server needs it, and WHERE to post it: the owner whose ids the action map keys by
/// (6-4). The name's current owner (its unique bus name) is asked whether it renders markup, and the post goes to that
/// same owner — never to a server that took the name over in between (a cached answer once outlived a switch from a
/// plain server to a markup one, whose `<img src=file://…>` in someone's text then became markup). Asked at every post;
/// can't tell = escape. Nobody owns the name: one GetCapabilities to the well-known name starts the service (D-Bus
/// activation, as the first capability question always did) and its owner is read again.
#[cfg(target_os = "linux")]
async fn body_for_server(conn: &zbus::Connection, body: &str) -> (String, String) {
    const SVC: &str = "org.freedesktop.Notifications";
    const PATH: &str = "/org/freedesktop/Notifications";
    let dbus = zbus::fdo::DBusProxy::new(conn).await.ok();
    let owner_now = || async {
        match (&dbus, zbus::names::BusName::try_from(SVC)) {
            (Some(d), Ok(name)) => d.get_name_owner(name).await.ok().map(|o| o.to_string()),
            _ => None,
        }
    };
    let mut owner = owner_now().await;
    if owner.is_none() {
        let _ = conn.call_method(Some(SVC), PATH, Some(SVC), "GetCapabilities", &()).await;
        owner = owner_now().await;
    }
    let Some(dest) = owner else { return (markup_escape(body), SVC.to_string()) };
    let markup = match conn.call_method(Some(dest.as_str()), PATH, Some(SVC), "GetCapabilities", &()).await {
        Ok(reply) => reply.body().deserialize::<Vec<String>>().map(|c| c.iter().any(|x| x == "body-markup")).unwrap_or(true),
        Err(_) => true,
    };
    (if markup { markup_escape(body) } else { body.to_string() }, dest)
}

/// Fire ONE notification via `org.freedesktop.Notifications.Notify`. Best-effort:
/// any error (bus gone mid-session, service crashed) is logged at most once and
/// swallowed — never panics, never blocks the sync loop meaningfully.
/// 10.0: incoming-call notification — a Join button that switches/joins, 25 s lifetime.
#[cfg(target_os = "linux")]
pub async fn notify_call(summary: &str, body: &str, room_id: &str) {
    let Some(conn) = connection().await else { return };
    ensure_action_listener(conn.clone());
    let mut hints: HashMap<&str, zbus::zvariant::Value<'_>> = HashMap::new();
    hints.insert("desktop-entry", zbus::zvariant::Value::from("dev.tinhat.TinHat"));
    hints.insert("urgency", zbus::zvariant::Value::from(2u8));
    hints.insert("category", zbus::zvariant::Value::from("call.incoming"));
    let actions: Vec<&str> = vec!["default", "Join", "join", "Join"];
    let (body, dest) = body_for_server(&conn, body).await;
    let body_tuple = ("Tin Hat", 0u32, "dev.tinhat.TinHat", summary, body.as_str(), actions, hints, CALL_EXPIRE_MS);
    match conn
        .call_method(Some(dest.as_str()), "/org/freedesktop/Notifications", Some("org.freedesktop.Notifications"), "Notify", &body_tuple)
        .await
    {
        Ok(reply) => {
            if let Ok(id) = reply.body().deserialize::<u32>() {
                remember(&issuer_of(&reply, &dest), id, format!("join:{room_id}"));
            }
        }
        Err(e) => log_bus_unavailable_once(&e),
    }
}

/// Listen once for ActionInvoked on the session bus; map the notification id back to its action.
#[cfg(target_os = "linux")]
// A plain fn (it only spawns): the listener's task spawns notif_actions::run, whose failure note posts through
// notify_action → here again; an async fn here made that a cycle the Send check can't resolve.
fn ensure_action_listener(conn: zbus::Connection) {
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
        let mut replied = proxy.receive_signal("NotificationReplied").await.ok();
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
        {
            let _g = pending_actions().lock().unwrap();
            *known_owner().lock().unwrap() = owner.clone();
        }
        // 6-4: an action is found by (the signal's sender, id) — the server that issued the id
        let key_of = |msg: &zbus::Message, id: u32| msg.header().sender().map(|s| (s.to_string(), id));
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
                        let action = key_of(&msg, id).and_then(|k| pending_actions().lock().unwrap().remove(&k));
                        if let Some(a) = action {
                            // docs/NOTIF-ACTIONS.md: a message notification's Reply / Mark as read / click
                            if let Some((gen, room, event)) = crate::notif_actions::parse_pending(&a) {
                                if matches!(key.as_str(), "default" | "reply" | "inline-reply" | "read") {
                                    // an inline field the server could not show arrives here without text: Compose
                                    crate::bridge::runtime().spawn(crate::notif_actions::run(gen, room, event, key.clone(), String::new()));
                                    if key == "read" {
                                        crate::bridge::runtime().spawn(close_notification(id));
                                    }
                                }
                            } else if key == "join" || key == "default" {
                                toast_action(a);
                            }
                        }
                    }
                }
                // KDE's inline reply (`inline-reply` capability): the typed text. Precedes its NotificationClosed.
                m = async { match replied.as_mut() { Some(s) => s.next().await, None => std::future::pending().await } } => {
                    let Some(msg) = m else { replied = None; continue };
                    if !from_owner(&msg, &owner) {
                        continue;
                    }
                    if let Ok((id, text)) = msg.body().deserialize::<(u32, String)>() {
                        let action = key_of(&msg, id).and_then(|k| pending_actions().lock().unwrap().remove(&k));
                        if let Some((gen, room, event)) = action.as_deref().and_then(crate::notif_actions::parse_pending) {
                            crate::bridge::runtime().spawn(crate::notif_actions::run(gen, room, event, "inline-reply".into(), text));
                        }
                    }
                }
                m = async { match closed.as_mut() { Some(s) => s.next().await, None => std::future::pending().await } } => {
                    let Some(msg) = m else { closed = None; continue };
                    if !from_owner(&msg, &owner) {
                        continue;
                    }
                    if let Some(k) = msg.body().deserialize::<(u32, u32)>().ok().and_then(|(id, _reason)| key_of(&msg, id)) {
                        // …and the entry outlives the close by a while, for a click — or a KDE inline reply, which
                        // arrives on its own signal stream and may be handled after the close — still in flight. Ids
                        // only grow within one server; a new server clears the map (owner change).
                        crate::bridge::runtime().spawn(async move {
                            tokio::time::sleep(Duration::from_secs(30)).await;
                            pending_actions().lock().unwrap().remove(&k);
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
                    owner_changed(owner.clone());
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
    notify_action_replacing(summary, body, icon, action, 0).await
}

/// [`notify_action_id`] that replaces notification `replaces` (0 = a new one): P3 a, one message notification per room.
#[cfg(target_os = "linux")]
async fn notify_action_replacing(summary: &str, body: &str, icon: Option<String>, action: &str, replaces: u32) -> Option<u32> {
    let conn = connection().await?;
    if !action.is_empty() {
        ensure_action_listener(conn.clone());
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
    let (body, dest) = body_for_server(&conn, body).await;
    let body_tuple = (
        "Tin Hat", // app_name
        replaces,      // replaces_id (0 = a new notification; P3 a: a room's previous message notification)
        "dev.tinhat.TinHat", // app_icon (10.1, TH-COPY-20: the rename missed this one)
        summary,
        body.as_str(),
        actions,
        hints,
        EXPIRE_MS, // expire_timeout (ms)
    );

    match conn
        .call_method(
            Some(dest.as_str()),
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
                remember(&issuer_of(&reply, &dest), id, action.to_string());
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

/// What the notification server can do (asked once per server: dropped when the name changes hands).
#[cfg(target_os = "linux")]
#[derive(Clone, Copy, Debug)]
struct Caps {
    actions: bool,
    inline_reply: bool,
}

#[cfg(target_os = "linux")]
fn caps_cell() -> &'static Mutex<Option<Caps>> {
    static C: OnceLock<Mutex<Option<Caps>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(None))
}

#[cfg(target_os = "linux")]
async fn caps(conn: &zbus::Connection) -> Caps {
    if let Some(c) = *caps_cell().lock().unwrap() {
        return c;
    }
    let epoch = owner_epoch();
    let c = match conn
        .call_method(Some("org.freedesktop.Notifications"), "/org/freedesktop/Notifications", Some("org.freedesktop.Notifications"), "GetCapabilities", &())
        .await
    {
        Ok(reply) => {
            let list = reply.body().deserialize::<Vec<String>>().unwrap_or_default();
            Caps { actions: list.iter().any(|x| x == "actions"), inline_reply: list.iter().any(|x| x == "inline-reply") }
        }
        // can't tell: plain notifications (as before), asked again next time
        Err(_) => return Caps { actions: false, inline_reply: false },
    };
    eprintln!("[notify] notification server: actions={} inline-reply={}", c.actions, c.inline_reply);
    // kept only if the server didn't change hands while we asked (the answer was the old server's)
    let g = pending_actions().lock().unwrap();
    if owner_epoch() == epoch {
        *caps_cell().lock().unwrap() = Some(c);
    }
    drop(g);
    c
}

/// docs/NOTIF-ACTIONS.md: a MESSAGE notification — the click opens the room; Reply (inline where the server has a
/// field and this device is trusted, else it opens the room with the composer focused); Mark as read (when the
/// notified event is known). Everything else as [`notify_action`].
#[cfg(target_os = "linux")]
pub async fn notify_message(summary: &str, body: &str, icon: Option<String>, room_id: &str, event_id: &str, thread: &str, trusted: bool, gen: u64) -> bool {
    use crate::notif_actions as na;
    let Some(conn) = connection().await else { return false };
    let caps = caps(&conn).await;
    let keys = na::offer(&na::OfferCtx { actions_cap: caps.actions, inline_field: caps.inline_reply, trusted, event: event_id });
    // a stable line for the gates (tools/README-gates.md): which actions this notification carries
    eprintln!("[notify] message notification: keys={keys:?} trusted={trusted}");
    if crate::session::generation() != gen {
        return false; // the session the scan began in ended meanwhile
    }
    // P3 a: the room's notification still on screen is replaced (a burst, then its trailing follow-up = one entry)
    let epoch = owner_epoch();
    let replaces = room_toasts().lock().ok().and_then(|m| m.get(room_id).copied()).filter(|(e, _)| *e == epoch).map_or(0, |(_, id)| id);
    if keys.len() <= 1 {
        let Some(id) = notify_action_replacing(summary, body, icon, &open_action(room_id, thread), replaces).await else { return false };
        if let Ok(mut m) = room_toasts().lock() {
            m.insert(room_id.to_string(), (epoch, id));
        }
        return true;
    }
    ensure_action_listener(conn.clone());
    let mut hints: HashMap<&str, zbus::zvariant::Value<'_>> = HashMap::new();
    hints.insert("desktop-entry", zbus::zvariant::Value::from("dev.tinhat.TinHat"));
    hints.insert("category", zbus::zvariant::Value::from("im.received"));
    let icon_path = icon.map(|u| crate::platform::path_from_file_url(&u));
    if let Some(p) = icon_path.as_deref().filter(|p| !p.is_empty()) {
        hints.insert("image-path", zbus::zvariant::Value::from(p));
    }
    if keys.contains(&"inline-reply") {
        hints.insert("x-kde-reply-placeholder-text", zbus::zvariant::Value::from(na::WORD_PLACEHOLDER));
        hints.insert("x-kde-reply-submit-button-text", zbus::zvariant::Value::from(na::WORD_SEND));
    }
    // [key, label] pairs; "default" = the click (no button drawn)
    let mut actions: Vec<&str> = Vec::new();
    for k in &keys {
        actions.push(k);
        actions.push(match *k {
            "default" => "Open",
            "read" => na::WORD_MARK_READ,
            _ => na::WORD_REPLY,
        });
    }
    let (body, dest) = body_for_server(&conn, body).await;
    // the session the scan began in (passed down, never re-read here): a sign-out meanwhile posts nothing
    if crate::session::generation() != gen {
        return false;
    }
    let body_tuple = ("Tin Hat", replaces, "dev.tinhat.TinHat", summary, body.as_str(), actions, hints, EXPIRE_MS);
    match conn
        .call_method(Some(dest.as_str()), "/org/freedesktop/Notifications", Some("org.freedesktop.Notifications"), "Notify", &body_tuple)
        .await
    {
        Ok(reply) => {
            let Ok(id) = reply.body().deserialize::<u32>() else { return false };
            remember(&issuer_of(&reply, &dest), id, na::pending_entry(gen, room_id, event_id));
            if let Ok(mut m) = room_toasts().lock() {
                m.insert(room_id.to_string(), (epoch, id));
            }
            true
        }
        Err(e) => {
            log_bus_unavailable_once(&e);
            if let Ok(mut g) = conn_cell().try_lock() {
                *g = None;
            }
            false
        }
    }
}

/// docs/NOTIF-ACTIONS.md (Windows): our own toast XML ([`crate::notif_actions::toast_xml`]) through the same WinRT
/// notifier the toast crate uses — the crate has no text box. Activated (in-process) carries the button's arguments
/// and, for Send, the typed text in `UserInput["reply"]` (Windows 10 1903+, so the box is offered only there).
#[cfg(windows)]
pub async fn notify_message(summary: &str, body: &str, icon: Option<String>, room_id: &str, event_id: &str, _thread: &str, trusted: bool, gen: u64) -> bool {
    use crate::notif_actions as na;
    let inline = windows_version::OsVersion::current().build >= 18362;
    let keys = na::offer(&na::OfferCtx { actions_cap: true, inline_field: inline, trusted, event: event_id });
    eprintln!("[notify] message notification: keys={keys:?} trusted={trusted}");
    let icon = icon.map(|u| crate::platform::path_from_file_url(&u)).filter(|p| !p.is_empty() && std::path::Path::new(p).exists());
    let xml = na::toast_xml(summary, body, icon.as_deref(), &keys);
    if crate::session::generation() != gen {
        return false; // the session the scan began in ended meanwhile
    }
    let room_id = room_id.to_string();
    let event_id = event_id.to_string();
    // R1-2: true = the toast was handed to Windows (the caller moves posted_threads only then)
    tokio::task::spawn_blocking(move || {
        use windows::core::{Interface, HSTRING};
        use windows::Data::Xml::Dom::XmlDocument;
        use windows::Foundation::{IPropertyValue, TypedEventHandler};
        use windows::UI::Notifications::{ToastActivatedEventArgs, ToastNotification, ToastNotificationManager};
        let show = || -> windows::core::Result<bool> {
            let doc = XmlDocument::new()?;
            doc.LoadXml(&HSTRING::from(xml.as_str()))?;
            let toast = ToastNotification::CreateToastNotification(&doc)?;
            // P3 a: one toast per room — a newer one with the same tag + group replaces it (and reading the room
            // removes it from the Action Center: close_room_toasts)
            let _ = toast.SetTag(&HSTRING::from(room_toast_tag(&room_id)));
            let _ = toast.SetGroup(&HSTRING::from(ROOM_TOAST_GROUP));
            if let Ok(mut m) = room_toasts().lock() {
                m.insert(room_id.clone());
            }
            let (r, e) = (room_id.clone(), event_id.clone());
            toast.Activated(&TypedEventHandler::new(move |_, insp| {
                let insp: &Option<windows::core::IInspectable> = &insp;
                let Some(args) = insp.as_ref().and_then(|i| i.cast::<ToastActivatedEventArgs>().ok()) else { return Ok(()) };
                let key = args.Arguments().map(|a| a.to_string()).unwrap_or_default();
                let text = if key == "inline-reply" {
                    args.UserInput()
                        .and_then(|set| set.Lookup(&HSTRING::from("reply")))
                        .and_then(|v| v.cast::<IPropertyValue>())
                        .and_then(|v| v.GetString())
                        .map(|h| h.to_string())
                        .unwrap_or_default()
                } else {
                    String::new()
                };
                crate::bridge::runtime().spawn(na::run(gen, r.clone(), e.clone(), key, text));
                Ok(())
            }))?;
            if crate::session::generation() != gen {
                return Ok(false); // signed out while this waited for its thread
            }
            ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(crate::platform::AUMID))?.Show(&toast)?;
            Ok(true)
        };
        match show() {
            Ok(shown) => shown,
            Err(e) => {
                static ONCE: OnceLock<()> = OnceLock::new();
                if ONCE.set(()).is_ok() {
                    eprintln!("[notify] WinRT message toast failed: {e}; silent from here");
                }
                false
            }
        }
    })
    .await
    .unwrap_or(false)
}

/// Elsewhere (macOS dev builds): the plain click-to-open notification.
#[cfg(not(any(windows, target_os = "linux")))]
pub async fn notify_message(summary: &str, body: &str, icon: Option<String>, room_id: &str, _event_id: &str, thread: &str, _trusted: bool, gen: u64) -> bool {
    if crate::session::generation() != gen {
        return false;
    }
    // no replacement here (each notification is its own) and no result from the OS: handed over = posted
    notify_action(summary, body, icon, &open_action(room_id, thread)).await;
    true
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
    plan_fires_in(&mut st, observed, Instant::now(), room_attended_for_latest)
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
    st.wake_at = None;
    let wake = |st: &mut DetectState, at: Instant| st.wake_at = Some(st.wake_at.map_or(at, |w| w.min(at)));

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
        // Per-room debounce: collapse a burst into one notification per window. A03.0: the rise
        // is HELD (the level goes back to what was last notified) and the scan wakes when the
        // window ends, so a follow-up inside it still gets its (trailing) notification.
        if let Some(t) = st.last_fired.get(id).copied() {
            if now.duration_since(t) < ROOM_DEBOUNCE {
                st.last_highlight.insert(id.clone(), prev);
                wake(st, t + ROOM_DEBOUNCE);
                continue;
            }
        }
        // Global rate cap (counted once: each fire is already in recent_fires), held the same way.
        if st.recent_fires.len() >= GLOBAL_CAP {
            st.last_highlight.insert(id.clone(), prev);
            if let Some(oldest) = st.recent_fires.iter().min().copied() {
                wake(st, oldest + GLOBAL_WINDOW);
            }
            continue;
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
    // A03.0 (review): the sync loop's scan and the trailing timer's never overlap — an older snapshot planned after a
    // newer one would put the levels back (a second notification), and a room's posts and closes stay in order
    let gen = crate::session::generation();
    let _one_at_a_time = scan_gate().lock().await;
    if crate::session::generation() != gen {
        return; // signed out (or another account) while this waited for the scan before it
    }
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
    // A03.2: a room read here or elsewhere takes its notification with it (as on the web); a room on screen here too —
    // an encrypted mentions-only room always counts as unread on the server (unread_on_server), so that is its way out
    // 1-1 (threads): "on screen" = the toast's own thread on screen — a thread reply's toast stays while only the room's
    // main timeline (or another thread) shows
    let read: Vec<String> = observed.iter().filter(|o| !o.unread_on_server || room_attended_for_thread(&o.id, &posted_thread(&o.id))).map(|o| o.id.clone()).collect();
    if !read.is_empty() {
        close_room_toasts(read).await;
    }
    let fires = plan_fires(&observed, gen);
    // review (1-1/1-5): what each notification is about — its event and that event's thread — read ONCE, now: a sync
    // landing during the awaits below rewrites the room's newest notification, and this toast must not take its words,
    // target or thread from that newer event
    let about: HashMap<String, (Option<Notified>, String)> = fires
        .iter()
        .map(|(id, _, _)| (id.clone(), last_notified().lock().map(|m| (m.get(id).cloned(), note_thread(id))).unwrap_or_default()))
        .collect();
    // A03.0: a rise held back by the per-room window / the cap gets its trailing notification even when no
    // other sync comes meanwhile — one timer runs this scan again when the earliest hold ends.
    if let Some(at) = detect().lock().ok().and_then(|d| d.wake_at) {
        arm_trailing_scan(client.clone(), at, gen);
    }
    // (computed before the early return: the levels are recorded on every pass)
    let personal = {
        let mut last = last_highlight().lock().unwrap_or_else(|p| p.into_inner());
        let any = fires.iter().any(|(id, _, is_dm)| {
            *is_dm || tally_of(id).highlight > last.get(id).copied().unwrap_or(0)
        });
        // every room's level is recorded each pass, so only a NEW mention counts next time — except a room whose rise
        // is HELD (A03.0): its trailing notification still flashes for the mention that came with it
        let held: std::collections::HashSet<&str> = detect()
            .lock()
            .map(|d| observed.iter().filter(|o| d.last_highlight.get(&o.id).is_some_and(|l| *l < o.trigger)).map(|o| o.id.as_str()).collect())
            .unwrap_or_default();
        for o in &observed {
            if !held.contains(o.id.as_str()) {
                last.insert(o.id.clone(), tally_of(&o.id).highlight);
            }
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
        // docs/NOTIF-ACTIONS.md: the event this notification is about — the room's newest event that notified (what
        // the tally rose by), else the SDK's latest event; Mark as read targets it, nothing newer
        let (notified, thread) = about.get(&room_id).cloned().unwrap_or_default();
        let event_id = notified.as_ref().map(|n| n.id.clone()).unwrap_or_else(|| latest_event_id(&room));
        // Messages-style: the preview (sender, text, the sender's picture) comes from the event that notified — the
        // one Mark as read targets; with none recorded, from the SDK's latest event when that is the same event
        let preview = match &notified {
            Some(n) => notified_preview(&room, n, !is_dm, &thread).await,
            None => {
                let latest_id = latest_event_id(&room);
                if latest_id.is_empty() || latest_id == event_id { latest_preview(&room, !is_dm).await } else { None }
            }
        };
        // threads: "Ana (in thread): …" / "In thread: …" (sender_preview); a click opens that thread (open_action)
        let (body, icon) = toast_body(preview, rise);

        // 10.6: the preview / avatar lookups above can take a while — a session that ended
        // meanwhile never gets its notification posted over the next one.
        if notify_on && crate::session::generation() == gen {
            // 10.1 (TH-GAP-06): clicking the toast opens the room (the most-performed gesture in a chat app).
            // docs/NOTIF-ACTIONS.md: plus Reply and Mark as read.
            let trusted = crate::notif_actions::own_device_trusted(client).await;
            // the scan's own generation (checked again inside after every await): never re-read after this await
            let posted = notify_message(&summary, &body, icon, &room_id, &event_id, &thread, trusted, gen).await;
            // 1-1: the thread of the toast now on screen, for closing it only when that thread is (recorded once it
            // is posted, by the same session). R1-2: only when the post went out — a failed replacement leaves the old
            // toast (and its thread) on screen
            if crate::session::generation() == gen {
                if let Ok(mut g) = posted_threads().lock() {
                    note_posted(&mut g, &room_id, &thread, posted);
                }
            }
        }
    }
}

/// A03.1: the toast text — the preview with "(+N more)" for a burst (one space, as on the web), or, with no preview,
/// "N new messages" / "New message" alone (never "3 new messages (+2 more)").
pub(crate) fn toast_body(preview: Option<(String, Option<String>)>, rise: u64) -> (String, Option<String>) {
    match preview {
        Some((body, icon)) if !body.is_empty() => {
            if rise > 1 {
                (format!("{body} (+{} more)", rise - 1), icon)
            } else {
                (body, icon)
            }
        }
        _ => (if rise > 1 { format!("{rise} new messages") } else { "New message".to_string() }, None),
    }
}

/// P3 a: the rooms whose message notification may still be on screen. Linux: room → (service epoch, id), so the next
/// one replaces it and reading the room closes it. Windows: the rooms with a tagged toast in the Action Center.
#[cfg(target_os = "linux")]
fn room_toasts() -> &'static Mutex<HashMap<String, (u64, u32)>> {
    static M: OnceLock<Mutex<HashMap<String, (u64, u32)>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(HashMap::new()))
}
#[cfg(windows)]
fn room_toasts() -> &'static Mutex<std::collections::HashSet<String>> {
    static M: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}

/// P3 a (Windows): the toast group of message notifications; the tag is [`room_toast_tag`].
#[cfg(windows)]
const ROOM_TOAST_GROUP: &str = "msg";

/// P3 a (Windows): a room's toast tag. Windows caps a tag at 64 characters and room ids can be longer, so the tag is
/// a stable 64-bit FNV-1a of the id: "r" + 16 hex digits.
#[cfg(any(windows, test))]
fn room_toast_tag(room_id: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in room_id.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("r{h:016x}")
}

/// A03.2: these rooms were read (here or on another device): their message notification goes.
pub async fn close_room_toasts(rooms: Vec<String>) {
    #[cfg(target_os = "linux")]
    {
        let epoch = owner_epoch();
        let ids: Vec<u32> = match room_toasts().lock() {
            Ok(mut m) => rooms.iter().filter_map(|r| m.remove(r)).filter(|(e, _)| *e == epoch).map(|(_, id)| id).collect(),
            Err(_) => return,
        };
        for id in ids {
            close_notification(id).await;
        }
    }
    #[cfg(windows)]
    {
        let rooms: Vec<String> = match room_toasts().lock() {
            Ok(mut m) => rooms.into_iter().filter(|r| m.remove(r)).collect(),
            Err(_) => return,
        };
        if rooms.is_empty() {
            return;
        }
        let _ = tokio::task::spawn_blocking(move || {
            use windows::core::HSTRING;
            use windows::UI::Notifications::ToastNotificationManager;
            let Ok(history) = ToastNotificationManager::History() else { return };
            let (group, app) = (HSTRING::from(ROOM_TOAST_GROUP), HSTRING::from(crate::platform::AUMID));
            for r in rooms {
                let _ = history.RemoveGroupedTagWithId(&HSTRING::from(room_toast_tag(&r)), &group, &app);
            }
        })
        .await;
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    let _ = rooms;
}

/// A03.0: one scan at a time (see [`scan_highlights`]).
fn scan_gate() -> &'static tokio::sync::Mutex<()> {
    static G: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    G.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// A03.0: the session and moment of the one armed trailing scan (None = none armed).
fn trailing_armed() -> &'static Mutex<Option<(u64, Instant)>> {
    static A: OnceLock<Mutex<Option<(u64, Instant)>>> = OnceLock::new();
    A.get_or_init(|| Mutex::new(None))
}

/// A03.0: run [`scan_highlights`] again at `at` (a little after, so the window has surely ended). An already armed
/// scan that runs no later covers it (that scan re-arms for whatever is still held). A plain fn (it only spawns): an
/// async fn spawning scan_highlights from inside scan_highlights would be a cycle the Send check can't resolve.
fn arm_trailing_scan(client: Client, at: Instant, gen: u64) {
    {
        let Ok(mut a) = trailing_armed().lock() else { return };
        // a scan of THIS session armed for no later is still to come: it covers this one (another session's never does)
        if a.is_some_and(|(g, x)| g == gen && x <= at && x > Instant::now()) {
            return;
        }
        *a = Some((gen, at));
    }
    crate::bridge::runtime().spawn(async move {
        tokio::time::sleep_until(tokio::time::Instant::from_std(at + Duration::from_millis(50))).await;
        {
            let Ok(mut a) = trailing_armed().lock() else { return };
            if *a != Some((gen, at)) {
                return; // another was armed after this one (earlier, or for a new session): it runs instead
            }
            *a = None;
        }
        if crate::session::generation() == gen {
            scan_highlights(&client).await;
        }
    });
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
    if let Ok(mut a) = trailing_armed().lock() {
        *a = None;
    }
    if let Ok(mut t) = tallies().lock() {
        t.clear();
    }
    if let Ok(mut l) = last_highlight().lock() {
        l.clear();
    }
    if let Ok(mut l) = last_notified().lock() {
        l.clear();
    }
    // threads: a signed-out account's thread routing never reaches the next one
    if let Ok(mut t) = note_threads().lock() {
        t.clear();
    }
    if let Ok(mut t) = posted_threads().lock() {
        t.clear();
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
/// The id of the event the notification's preview comes from ("" when unknown or not a server event).
fn latest_event_id(room: &matrix_sdk::Room) -> String {
    use matrix_sdk_base::latest_event::LatestEventValue;
    match room.latest_event() {
        LatestEventValue::Remote(event) => event.event_id().map(|e| e.to_string()).unwrap_or_default(),
        _ => String::new(),
    }
}

/// threads (docs/THREADS.md): per room, the thread its newest message notification is about ("" / none = the main
/// timeline). Every "open the room" action of a notification reads it through [`open_action`].
fn note_threads() -> &'static std::sync::Mutex<std::collections::HashMap<String, String>> {
    static N: OnceLock<std::sync::Mutex<std::collections::HashMap<String, String>>> = OnceLock::new();
    N.get_or_init(Default::default)
}
#[cfg(test)]
pub(crate) fn note_thread_set_for_tests(room_id: &str, root: &str) {
    note_thread_set(room_id, root);
}
fn note_thread_set(room_id: &str, root: &str) {
    if let Ok(mut g) = note_threads().lock() {
        g.insert(room_id.to_string(), root.to_string());
    }
}

/// The action a click on a room's message notification runs: "open:<room>", or — when the notification is about a
/// thread reply — "thread:<room>\t<root>" (Main.qml opens the room and the thread in the Threads panel).
/// 12.1.16 review: `root` is the thread of the event THIS notification is about (captured when it was planned), never
/// re-read from the room's newest notification.
pub(crate) fn open_action(room_id: &str, root: &str) -> String {
    if root.is_empty() {
        format!("open:{room_id}")
    } else {
        format!("thread:{room_id}\t{root}")
    }
}

/// threads: the thread of the room's newest message notification ("" = the main timeline).
fn note_thread(room_id: &str) -> String {
    note_threads().lock().ok().and_then(|g| g.get(room_id).cloned()).unwrap_or_default()
}

/// Messages-style: the toast preview of the event that notified ([`Notified`]): its text, prefixed with the sender's
/// name in a group room, and the sender's picture. None when it has no text (the generic "New message" then).
async fn notified_preview(room: &matrix_sdk::Room, n: &Notified, with_sender: bool, thread: &str) -> Option<(String, Option<String>)> {
    sender_preview(room, n.body.as_deref()?, n.sender.as_deref(), with_sender, thread).await
}

/// The longest preview, in code points (the web's PREVIEW_MAX).
pub(crate) const PREVIEW_MAX: usize = 160;

/// JavaScript's `\s` (what the web's `trim()` and `/\s+/` use): Rust's `char::is_whitespace` differs at U+0085 and
/// U+FEFF, and the preview must be the same text in both apps.
fn js_space(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}'
        | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}')
}

/// Messages-style: a notification's preview text, the same rule as the web's toastpreview.ts (shared vectors
/// testvectors/toast-preview.json): the body trimmed, every whitespace run folded to one space, cut to PREVIEW_MAX
/// code points + "…", then "Name: " in a group room. None = no preview (the generic wording).
pub(crate) fn preview_text(body: &str, name: Option<&str>) -> Option<String> {
    let folded = body.split(js_space).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ");
    if folded.is_empty() {
        return None;
    }
    let cut = if folded.chars().count() > PREVIEW_MAX {
        let mut c: String = folded.chars().take(PREVIEW_MAX).collect();
        c.push('…');
        c
    } else {
        folded
    };
    Some(match name {
        Some(n) => format!("{n}: {cut}"),
        None => cut,
    })
}

async fn latest_preview(room: &matrix_sdk::Room, with_sender: bool) -> Option<(String, Option<String>)> {
    use matrix_sdk_base::latest_event::LatestEventValue;
    let LatestEventValue::Remote(event) = room.latest_event() else {
        return None;
    };
    let json = event.raw().deserialize_as::<serde_json::Value>().ok()?;
    // raw: preview_text trims and decides emptiness with the web's whitespace set (Rust's trim() differs at U+0085)
    let body = json.get("content").and_then(|c| c.get("body")).and_then(|b| b.as_str())?;
    let thread = tinhat_widgets::threads::thread_of(json.get("content").unwrap_or(&serde_json::Value::Null));
    sender_preview(room, body, json.get("sender").and_then(|s| s.as_str()), with_sender, &thread).await
}

/// The preview of `body` ([`preview_text`]) with its sender: the sender's picture, and in a group room "Name: body".
/// threads: a reply in a thread reads "Name (in thread): body" / "In thread: body" (tinhat_widgets::threads::note_text,
/// the web's noteText); the text itself is the shared preview rule's.
async fn sender_preview(room: &matrix_sdk::Room, body: &str, sender: Option<&str>, with_sender: bool, thread: &str) -> Option<(String, Option<String>)> {
    // Sender lookup from the local member cache only — get_member_no_sync never hits the
    // network, so a cold cache degrades to the bare message instead of stalling a toast.
    // Done even for DMs (where we don't prefix the name), because we still want the face.
    let member = match sender {
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
    // 1-10: an uncached member still gets its name (the id's localpart), as on the web
    let display = member.as_ref().and_then(|(_, m)| m.display_name());
    Some((toast_note(display, sender, !with_sender, !thread.is_empty(), body)?, icon))
}

/// fix1216 1-10: a group toast's sender label, the web's `rawDisplayName || localpart`: the display name, or the id's
/// localpart when the member isn't cached or its name is empty.
pub(crate) fn sender_label(display: Option<&str>, user_id: &str) -> String {
    match display.filter(|d| !d.is_empty()) {
        Some(d) => d.to_owned(),
        None => user_id.strip_prefix('@').unwrap_or(user_id).split(':').next().unwrap_or("").to_owned(),
    }
}

/// fix1216 1-10: the whole toast line, the web's toastNote (shared vectors toast-preview.json `notes`): the cut text
/// ([`preview_text`]) with the thread wording, "Name: …" in a group room, the text alone in a DM. None = no preview.
pub(crate) fn toast_note(display: Option<&str>, sender: Option<&str>, dm: bool, thread: bool, body: &str) -> Option<String> {
    let cut = preview_text(body, None)?;
    let name = if dm { String::new() } else { sender_label(display, sender.unwrap_or("")) };
    // no name to put first (no sender at all): the nameless form, which still says it is in a thread
    Some(tinhat_widgets::threads::note_text(&name, &cut, thread, dm || name.is_empty()))
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

/// 6-4 (fix1216): the markup decision and the post both follow the server that owns the name when a notification is
/// posted. Runs a private `dbus-daemon` (skipped, with a note, where there is none) with stand-in notification servers.
#[cfg(all(test, target_os = "linux"))]
mod markup_server_tests {
    use std::sync::{Arc, Mutex};

    struct Fake {
        caps: Vec<String>,
        got: Arc<Mutex<Vec<String>>>,
    }
    #[zbus::interface(name = "org.freedesktop.Notifications")]
    impl Fake {
        async fn get_capabilities(&self) -> Vec<String> {
            self.caps.clone()
        }
        #[allow(clippy::too_many_arguments)]
        async fn notify(
            &self,
            _app: String,
            _replaces: u32,
            _icon: String,
            _summary: String,
            body: String,
            _actions: Vec<String>,
            _hints: std::collections::HashMap<String, zbus::zvariant::OwnedValue>,
            _timeout: i32,
        ) -> u32 {
            self.got.lock().unwrap().push(body);
            1
        }
    }

    async fn server(addr: &str, caps: &[&str]) -> (zbus::Connection, Arc<Mutex<Vec<String>>>) {
        let got = Arc::new(Mutex::new(Vec::new()));
        let conn = zbus::connection::Builder::address(addr)
            .unwrap()
            .serve_at("/org/freedesktop/Notifications", Fake { caps: caps.iter().map(|c| c.to_string()).collect(), got: got.clone() })
            .unwrap()
            .name("org.freedesktop.Notifications")
            .unwrap()
            .build()
            .await
            .unwrap();
        (conn, got)
    }

    async fn post(client: &zbus::Connection, dest: &str, body: &str) {
        let hints: std::collections::HashMap<&str, zbus::zvariant::Value<'_>> = std::collections::HashMap::new();
        let t = ("Tin Hat", 0u32, "dev.tinhat.TinHat", "WN", body, Vec::<&str>::new(), hints, 5000i32);
        let _ = client.call_method(Some(dest), "/org/freedesktop/Notifications", Some("org.freedesktop.Notifications"), "Notify", &t).await;
    }

    /// 6-4 (reviews 3+4): actions are kept per issuing server and found by (sender, id). A post that started the service
    /// keeps its action across the owner change it causes; a post the new server answered before the listener saw the
    /// takeover keeps its action too; a takeover drops the old owner's actions, and the new owner's same id finds
    /// nothing of the old one's.
    #[test]
    fn actions_belong_to_the_server_that_issued_the_id() {
        use super::{known_owner, owner_changed, pending_actions, remember};
        let get = |o: &str, id: u32| pending_actions().lock().unwrap().get(&(o.to_string(), id)).cloned();
        pending_actions().lock().unwrap().clear();
        // activation: the listener doesn't know the new owner yet when the post registers its id
        *known_owner().lock().unwrap() = None;
        remember(":1.50", 1, "join:!a".into());
        owner_changed(Some(":1.50".into()));
        assert_eq!(get(":1.50", 1).as_deref(), Some("join:!a"), "the started service keeps the first post's action");
        // the listener still knows :1.50 while :1.60 already answered a post
        remember(":1.60", 2, "join:!b".into());
        owner_changed(Some(":1.60".into()));
        assert_eq!(get(":1.60", 2).as_deref(), Some("join:!b"), "the new server's post keeps its action");
        assert_eq!(get(":1.50", 1), None, "the old server's actions go with it");
        assert_eq!(get(":1.60", 1), None, "the new server's id 1 finds nothing of the old server's");
        // soft bound: leftovers of servers that went away are trimmed (the current owner's and the inserting server's stay)
        for i in 0..600u32 {
            remember(&format!(":1.{}", 1000 + i), 1, "join:!x".into());
        }
        assert!(pending_actions().lock().unwrap().len() <= 512);
        assert_eq!(get(":1.60", 2).as_deref(), Some("join:!b"), "the current owner's action survives the trim");
        pending_actions().lock().unwrap().clear();
        *known_owner().lock().unwrap() = None;
    }

    /// Every Notify reply registers its actions under the server that answered (fallback posts included).
    #[test]
    fn every_post_registers_under_the_answering_server() {
        let src = include_str!("notify.rs");
        let prod = &src[..src.find("mod markup_server_tests").unwrap()];
        assert_eq!(prod.matches("remember(&issuer_of(&reply, &dest), id,").count(), 3);
    }

    /// Every production Notify goes to the owner body_for_server asked (the test above posts through its own helper).
    #[test]
    fn every_notify_goes_to_the_owner_that_was_asked() {
        let src = include_str!("notify.rs");
        let prod = &src[..src.find("mod markup_server_tests").unwrap()];
        let notifies = prod.matches("\"Notify\"").count();
        let to_owner = prod.matches("Some(dest.as_str()), \"/org/freedesktop/Notifications\", Some(\"org.freedesktop.Notifications\"), \"Notify\"").count()
            + prod.matches("Some(dest.as_str()),\n            \"/org/freedesktop/Notifications\",\n            Some(\"org.freedesktop.Notifications\"),\n            \"Notify\"").count();
        assert_eq!(notifies, 3, "three Notify posts");
        assert_eq!(to_owner, 3, "each addressed to the owner that answered the capability question");
        assert_eq!(prod.matches("body_for_server(&conn, body).await").count(), 3);
    }

    #[test]
    fn a_switch_to_a_markup_server_never_gets_raw_text() {
        use std::io::BufRead;
        let Ok(mut daemon) = std::process::Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--nopidfile", "--print-address=1"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
        else {
            eprintln!("markup_server_tests: no dbus-daemon here, skipped");
            return;
        };
        let mut addr = String::new();
        std::io::BufReader::new(daemon.stdout.take().unwrap()).read_line(&mut addr).unwrap();
        let addr = addr.trim().to_string();
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let (nobody, plain_first, raced, markup_next, plain_got, markup_got, dest_is_owner) = rt.block_on(async {
            let client = zbus::connection::Builder::address(addr.as_str()).unwrap().build().await.unwrap();
            // 0. nobody owns the name (and nothing to start): escaped, posted to the well-known name
            let nobody = super::body_for_server(&client, "<b>x</b>").await;
            let (plain, plain_got) = server(&addr, &["body", "actions"]).await;
            let plain_id = plain.unique_name().unwrap().to_string();
            // 1. a plain server: the text as it is, addressed to that server
            let (first, dest1) = super::body_for_server(&client, "<b>hi</b> & bye").await;
            // 2. prepared against the plain server, then a markup server takes the name before the post (the race)
            let (raced, dest2) = super::body_for_server(&client, "<img src=\"file:///tmp/race.png\"/>").await;
            plain.release_name("org.freedesktop.Notifications").await.unwrap();
            let (markup, markup_got) = server(&addr, &["body", "body-markup", "body-images"]).await;
            let markup_id = markup.unique_name().unwrap().to_string();
            post(&client, &dest2, &raced).await;
            // 3. the next post: asked of the markup server, escaped, addressed to it
            let (next, dest3) = super::body_for_server(&client, "<img src=\"file:///tmp/probe.png\"/> hi").await;
            post(&client, &dest3, &next).await;
            let p = plain_got.lock().unwrap().clone();
            let m = markup_got.lock().unwrap().clone();
            drop((plain, markup));
            (nobody, first, raced, next, p, m, dest1 == plain_id && dest2 == plain_id && dest3 == markup_id)
        });
        let _ = daemon.kill();
        let _ = daemon.wait();
        assert_eq!((nobody.0.as_str(), nobody.1.as_str()), ("&lt;b&gt;x&lt;/b&gt;", "org.freedesktop.Notifications"), "no owner: escaped");
        assert_eq!(plain_first, "<b>hi</b> & bye", "a server without body-markup gets the text as it is");
        assert!(dest_is_owner, "each post is addressed to the owner that answered GetCapabilities");
        assert_eq!(raced, "<img src=\"file:///tmp/race.png\"/>");
        assert_eq!(plain_got, vec![raced.clone()], "the raced post goes to the plain server that was asked");
        assert_eq!(markup_next, "&lt;img src=\"file:///tmp/probe.png\"/&gt; hi", "the markup server that took over gets escaped text");
        assert_eq!(markup_got, vec![markup_next.clone()], "the markup server never received the unescaped text");
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

    /// Messages-style (sun-winnotify): a notification's preview comes from the event that notified — id, sender, text.
    #[test]
    fn the_notified_event_gives_id_sender_and_trimmed_text() {
        let n = notified_event(&ev("@sam:example.com", "m.room.message", serde_json::json!({ "msgtype": "m.text", "body": "  hi there \n" })));
        assert_eq!(n, Some(Notified { id: "$e".into(), sender: Some("@sam:example.com".into()), body: Some("  hi there \n".into()) }));
        // still encrypted (no key yet): no text, the generic wording then — but the id stays for Mark as read
        let enc = notified_event(&ev("@sam:example.com", "m.room.encrypted", serde_json::json!({ "algorithm": "m.megolm.v1.aes-sha2", "ciphertext": "x" })));
        assert_eq!(enc.as_ref().map(|n| (n.id.as_str(), n.body.clone())), Some(("$e", None)));
        let blank = notified_event(&ev("@sam:example.com", "m.room.message", serde_json::json!({ "msgtype": "m.text", "body": "   " })));
        assert_eq!(blank.and_then(|n| n.body), None);
        assert_eq!(notified_event(&serde_json::json!({ "type": "m.room.message", "content": { "body": "x" } })), None);
    }

    /// Messages-style (sun-winnotify): record_sync keeps the NEWEST event that notified, with its sender and text (the
    /// room's cached latest event stays empty in matrix-sdk 0.19, so without this every toast said "New message").
    #[test]
    fn record_sync_keeps_the_newest_notifying_events_sender_and_text() {
        use matrix_sdk::ruma::serde::Raw;
        use matrix_sdk_base::deserialized_responses::RawAnySyncOrStrippedTimelineEvent;
        let room = "!wn-preview:example.com";
        let mk = |id: &str, body: &str, notify: bool| matrix_sdk::sync::Notification {
            actions: if notify { vec![Action::Notify] } else { vec![] },
            event: RawAnySyncOrStrippedTimelineEvent::Sync(
                Raw::from_json(serde_json::value::to_raw_value(&serde_json::json!({ "type": "m.room.message", "sender": "@sam:example.com",
                    "event_id": id, "origin_server_ts": 1u64, "content": { "msgtype": "m.text", "body": body } })).unwrap()),
            ),
        };
        let mut map = std::collections::BTreeMap::new();
        map.insert(matrix_sdk::ruma::OwnedRoomId::try_from(room).unwrap(), vec![mk("$a", "first", true), mk("$b", "second", true), mk("$c", "quiet", false)]);
        record_sync(&map);
        let got = last_notified().lock().unwrap().get(room).cloned();
        assert_eq!(got, Some(Notified { id: "$b".into(), sender: Some("@sam:example.com".into()), body: Some("second".into()) }));
    }

    /// Messages-style (sun-winnotify): the preview text on the shared vectors (byte copy of the web's
    /// src/features/settings/testvectors/toast-preview.json).
    #[test]
    fn toast_preview_vectors() {
        let v: serde_json::Value = serde_json::from_str(include_str!("testvectors/toast-preview.json")).unwrap();
        assert_eq!(v["max"].as_u64(), Some(PREVIEW_MAX as u64));
        let mut bad = Vec::new();
        for c in v["cases"].as_array().unwrap() {
            let got = preview_text(c["body"].as_str().unwrap(), c["sender"].as_str());
            if got.as_deref() != c["want"].as_str() {
                bad.push(format!("{}: got {got:?} want {}", c["name"], c["want"]));
            }
        }
        assert!(bad.is_empty(), "{bad:#?}");
    }

    /// fix1216 1-10: the whole toast line on the shared `notes` vectors (an uncached sender gets its localpart).
    #[test]
    fn toast_note_vectors() {
        let v: serde_json::Value = serde_json::from_str(include_str!("testvectors/toast-preview.json")).unwrap();
        let notes = v["notes"].as_array().expect("the notes group");
        assert!(notes.len() >= 10);
        let mut bad = Vec::new();
        for c in notes {
            let got = toast_note(c["display"].as_str(), c["sender"].as_str(), c["dm"].as_bool().unwrap(), c["thread"].as_bool().unwrap(), c["body"].as_str().unwrap());
            if got.as_deref() != c["want"].as_str() {
                bad.push(format!("{}: got {got:?} want {}", c["name"], c["want"]));
            }
        }
        assert!(bad.is_empty(), "{bad:#?}");
    }

    /// fix1216 1-10 (wiring): the toast's line is toast_note — the old inline naming can't come back unnoticed.
    #[test]
    fn sender_preview_uses_toast_note() {
        let src = include_str!("notify.rs");
        let at = src.find("async fn sender_preview(").expect("sender_preview");
        let body = &src[at..at + src[at..].find("\n}\n").expect("its end")];
        assert!(body.contains("toast_note(display, sender, !with_sender, !thread.is_empty(), body)"), "sender_preview builds its line with toast_note");
        assert!(!body.contains("localpart()"), "no separate naming rule in sender_preview");
    }

    #[test]
    fn toast_preview_vectors_match_the_web_copy() {
        let Ok(dir) = std::env::var("TINHAT_WEB_DIR") else { return };
        let theirs = std::fs::read_to_string(std::path::Path::new(&dir).join("src/features/settings/testvectors/toast-preview.json")).unwrap();
        assert!(include_str!("testvectors/toast-preview.json") == theirs, "testvectors/toast-preview.json differs from the web copy");
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

/// A03.0 (P3 a): the shared burst vectors — the web's notifyrules planScan replays the same file.
#[cfg(test)]
mod burst_tests {
    use super::{mode_tag, plan_fires_in, DetectState, Observed};
    use matrix_sdk_base::notification_settings::RoomNotificationMode as M;
    use std::time::{Duration, Instant};

    const VECTORS: &str = include_str!("testvectors/notify-burst.json");

    #[test]
    fn notify_burst_vectors() {
        let v: serde_json::Value = serde_json::from_str(VECTORS).unwrap();
        assert_eq!(v["gap_ms"].as_u64(), Some(super::ROOM_DEBOUNCE.as_millis() as u64));
        assert_eq!(v["cap"].as_u64(), Some(super::GLOBAL_CAP as u64));
        assert_eq!(v["cap_window_ms"].as_u64(), Some(super::GLOBAL_WINDOW.as_millis() as u64));
        let t0 = Instant::now();
        let mut failed = Vec::new();
        for c in v["cases"].as_array().unwrap() {
            let name = c["name"].as_str().unwrap();
            let mut st = DetectState::default();
            for (i, step) in c["steps"].as_array().unwrap().iter().enumerate() {
                let t = step["t"].as_u64().unwrap();
                let now = t0 + Duration::from_millis(t);
                let mut open = Vec::new();
                let rooms: Vec<Observed> = step["rooms"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|r| {
                        let id = r["id"].as_str().unwrap().to_string();
                        let mode = match r["mode"].as_str().unwrap_or("all") {
                            "mute" => M::Mute,
                            "mentions" => M::MentionsAndKeywordsOnly,
                            _ => M::AllMessages,
                        };
                        if r["attended"].as_bool().unwrap_or(false) {
                            open.push(id.clone());
                        }
                        Observed {
                            id,
                            trigger: r["count"].as_u64().unwrap(),
                            muted: matches!(mode, M::Mute),
                            is_dm: false,
                            mode_tag: mode_tag(mode),
                            unread_on_server: true,
                        }
                    })
                    .collect();
                let fired = plan_fires_in(&mut st, &rooms, now, |id| open.iter().any(|o| o == id));
                let got_fire: Vec<serde_json::Value> = fired.iter().map(|(id, rise, _)| serde_json::json!([id, rise])).collect();
                let got_wake = st.wake_at.map(|w| w.duration_since(t0).as_millis() as u64);
                let want_fire = step["fire"].as_array().unwrap().clone();
                let want_wake = step["wake"].as_u64();
                if got_fire != want_fire || got_wake != want_wake {
                    failed.push(format!("{name} step {i} (t={t}): got {got_fire:?} wake {got_wake:?}, want {want_fire:?} wake {want_wake:?}"));
                    break;
                }
            }
        }
        assert!(failed.is_empty(), "notify-burst vectors:\n{}", failed.join("\n"));
    }

    /// A03.1: one space before "(+N more)" (as the web's toastBody), and no "(+N more)" after the "N new messages" fallback.
    #[test]
    fn toast_body_counts_once() {
        use super::toast_body;
        assert_eq!(toast_body(Some(("Ana: hi".into(), None)), 1).0, "Ana: hi");
        assert_eq!(toast_body(Some(("Ana: hi".into(), None)), 3).0, "Ana: hi (+2 more)");
        assert_eq!(toast_body(None, 3).0, "3 new messages");
        assert_eq!(toast_body(None, 1).0, "New message");
        assert_eq!(toast_body(Some((String::new(), None)), 2).0, "2 new messages");
    }

    /// P3 a (Windows): a room's toast tag fits Windows' 64-character cap, is stable, and differs per room.
    #[test]
    fn room_toast_tag_is_short_and_stable() {
        use super::room_toast_tag;
        let long = format!("!{}:{}", "x".repeat(40), "a-very-long-homeserver-name.example.org");
        let t = room_toast_tag(&long);
        assert!(t.len() <= 64 && t.len() == 17, "{t}");
        assert_eq!(t, room_toast_tag(&long));
        assert_ne!(room_toast_tag("!a:example.org"), room_toast_tag("!b:example.org"));
        assert_eq!(room_toast_tag(""), "rcbf29ce484222325");
    }

    /// The web's copy is byte-identical (checked when the web repo is next to this one).
    #[test]
    fn notify_burst_vectors_match_the_web_copy() {
        let Ok(dir) = std::env::var("TINHAT_WEB_DIR") else { return };
        let path = std::path::Path::new(&dir).join("src/features/settings/testvectors/notify-burst.json");
        let theirs = std::fs::read_to_string(&path).unwrap();
        assert!(VECTORS == theirs, "testvectors/notify-burst.json differs from {}", path.display());
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

#[cfg(test)]
mod fix1216_thread_tests {
    use super::*;

    /// 1-1: a thread reply's toast is "on screen" only while that thread shows — the room open on its main timeline
    /// (or on another thread) is not enough; a main-timeline toast closes as before.
    #[test]
    fn a_thread_toast_closes_only_when_its_thread_shows() {
        let mut views: HashMap<u64, String> = HashMap::new();
        // view 1 = the main timeline, open
        assert!(!attended_for_thread_in("$threadA", &[1], &views), "main timeline open: thread A's toast stays");
        assert!(attended_for_thread_in("", &[1], &views), "a main-timeline toast closes as before");
        views.insert(2, "$threadB".into());
        assert!(!attended_for_thread_in("$threadA", &[1, 2], &views), "another thread open: thread A's toast stays");
        views.insert(3, "$threadA".into());
        assert!(attended_for_thread_in("$threadA", &[1, 3], &views), "thread A open: its toast closes");
        assert!(!attended_for_thread_in("", &[], &views), "the room not open: nothing closes");
        // review: only a thread's view on screen (no main view of the room): the main timeline is not on screen
        assert!(!attended_for_thread_in("", &[3], &views), "thread A's view alone doesn't show main");
        assert!(attended_for_thread_in("$threadA", &[3], &views));
    }

    /// R1-2: a failed replacement post keeps the room's toast thread where it was (the old toast is still on screen):
    /// opening the NEW thread must not close thread A's toast; a successful post moves it
    #[test]
    fn a_failed_replacement_post_keeps_the_old_toasts_thread() {
        let mut map: HashMap<String, String> = HashMap::new();
        note_posted(&mut map, "!r", "$threadA", true);
        note_posted(&mut map, "!r", "$threadB", false);
        assert_eq!(map.get("!r").map(String::as_str), Some("$threadA"), "the failed post for B leaves A's toast tracked");
        let views: HashMap<u64, String> = [(7u64, "$threadB".to_string())].into_iter().collect();
        assert!(!attended_for_thread_in(map.get("!r").unwrap(), &[7], &views), "thread B on screen doesn't close A's toast");
        note_posted(&mut map, "!r", "$threadB", true);
        assert_eq!(map.get("!r").map(String::as_str), Some("$threadB"), "a posted one moves it");
        note_posted(&mut map, "!new", "", false);
        assert!(!map.contains_key("!new"), "nothing posted: nothing tracked");
    }

    /// review (1-1): a plain toast's click (Linux single action, macOS) opens the thread captured with THIS toast's event,
    /// not the room's newest notification's
    #[test]
    fn a_plain_toast_click_uses_its_own_thread() {
        note_thread_set("!fix1216-plain:x", "$threadB");
        assert_eq!(open_action("!fix1216-plain:x", "$threadA"), "thread:!fix1216-plain:x\t$threadA");
        assert_eq!(open_action("!fix1216-plain:x", ""), "open:!fix1216-plain:x", "a main-timeline toast: the room");
    }

    /// 1-2 (launch-gate's test): a "main" receipt advances a main-timeline mention's read point — the reads both apps
    /// send now — and never a thread mention's; a thread's own receipt reads that thread's mention only.
    #[test]
    fn a_main_receipt_advances_the_mention_badge_read_point() {
        let none: HashMap<String, u64> = HashMap::new();
        let main = || Some(String::new());
        let root = || Some("$root".to_string());
        // a main mention at t=100; read on another device through a "main" receipt at 200; the unthreaded one is old
        assert_eq!(count_unseen_by_thread(&[(100.0, main())], 50, 200, &none), 0, "the main receipt read it");
        assert_eq!(count_unseen_by_thread(&[(300.0, main())], 50, 200, &none), 1, "a newer mention stays");
        // a thread mention at 100: the main receipt (200) does not read it; its thread's receipt does
        assert_eq!(count_unseen_by_thread(&[(100.0, root())], 50, 200, &none), 1, "main never clears a thread's");
        let mut th = HashMap::new();
        th.insert("$root".to_string(), 150u64);
        assert_eq!(count_unseen_by_thread(&[(100.0, root())], 50, 200, &th), 0, "its own thread receipt reads it");
        // the unthreaded receipt (Mark as read) still reads everything before it
        assert_eq!(count_unseen_by_thread(&[(100.0, root()), (90.0, main())], 120, 0, &none), 0);
        // review: a mention whose thread isn't known yet keeps today's rule — the main receipt doesn't read it, the
        // unthreaded one does
        assert_eq!(count_unseen_by_thread(&[(100.0, None)], 50, 200, &none), 1, "unknown is not main");
        assert_eq!(count_unseen_by_thread(&[(100.0, None)], 120, 0, &none), 0);
    }
}
