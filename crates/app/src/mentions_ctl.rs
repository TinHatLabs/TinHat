//! MentionsController — widgets W1 (2026-09-28): the Mentions widget's data (contract WIDGETS.md §13).
//!
//! * **What counts:** every event the account's push rules HIGHLIGHT, judged by the SDK after decryption — the very
//!   list `notify.rs` tallies for mention badges and desktop notifications (`SyncResponse::notifications`, 10.5.1). So
//!   encrypted rooms count (the server's `/notifications` can't see inside them). Edits (`m.replace`) don't add a row;
//!   our own events never reach this list (ruma skips the sender's own).
//! * **Where it lives:** this device only — `<data>/mentions.json` `{me, list}`, the ids of the newest 50
//!   (tinhat_widgets::mentions: normalize, add, mark seen); never an event's text. A snippet is resolved when a row
//!   shows it (`Room::event`, which decrypts) and kept in memory for the session only.
//! * **Seen:** a click on a row, "Mark all as seen", or the room being shown in this app (`notify::set_attending` →
//!   [`room_attended`]): what had arrived by then is read. A mention that arrives while its room is on screen, and every
//!   mention of this device's first sync (no list yet — they predate the widget), arrive seen.
//! * Recording runs whether or not a Mentions widget is shown (a mention that arrives while it is hidden still counts);
//!   the rows are resolved only while a widget asks (`wantRows`).

use core::pin::Pin;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;
use serde_json::{json, Value};
use tinhat_widgets::mentions::{add_mention, mark_seen, mention_summary, mentions_json, normalize_mentions, unseen_count, Mention, SeenSel};

#[cxx_qt::bridge]
pub mod qobject {
    extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(QString, rows_json)] // QML: rowsJson — the first `wantRows` entries, resolved: [{r, e, s, t, seen, room, name, avatar, kind, text}] ("" = not read yet)
        #[qproperty(i32, unseen)] //        QML: unseen — entries not opened yet
        #[qproperty(i32, total)] //         QML: total — entries listed (≤ 50)
        #[qproperty(QString, summary)] //   QML: summary — the Small line (mentions::mention_summary)
        type MentionsController = super::MentionsControllerRust;

        /// QML: start() — load this device's list and follow it (idempotent).
        #[qinvokable]
        fn start(self: Pin<&mut MentionsController>);
        /// QML: wantRows(n) — resolve the first n entries (M 5, L 12, S / collapsed 1); 0 = none (the widget went away).
        #[qinvokable]
        fn want_rows(self: Pin<&mut MentionsController>, n: i32);
        /// QML: markSeen(eventId) — a row was clicked.
        #[qinvokable]
        fn mark_seen(self: Pin<&mut MentionsController>, event_id: &QString);
        /// QML: markAllSeen() — "Mark all as seen".
        #[qinvokable]
        fn mark_all_seen(self: Pin<&mut MentionsController>);
    }

    impl cxx_qt::Threading for MentionsController {}
}

type QtThread = cxx_qt::CxxQtThread<qobject::MentionsController>;

pub struct MentionsControllerRust {
    rows_json: QString,
    unseen: i32,
    total: i32,
    summary: QString,
    want: i32,
    id: u64,
}

impl Default for MentionsControllerRust {
    fn default() -> Self {
        Self { rows_json: QString::default(), unseen: 0, total: 0, summary: QString::from("No mentions"), want: 0, id: 0 }
    }
}

// ── the store (process-wide) ───────────────────────────────────────────────────────────────────────────────────────

#[derive(Default)]
struct Store {
    loaded_for: String,
    list: Vec<Mention>,
    /// no list existed for this account on this device: the first sync's highlights arrive seen
    first_fill: bool,
    /// the views: controller id → its thread + how many rows it wants
    views: HashMap<u64, (QtThread, i32)>,
    /// event id → the resolved row parts (session memory only)
    resolved: HashMap<String, Value>,
}

fn store() -> &'static Mutex<Store> {
    static S: OnceLock<Mutex<Store>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(Store::default()))
}

pub(crate) fn file() -> std::path::PathBuf {
    crate::bridge::data_dir().join("mentions.json")
}

/// Sign-out (signout.rs, which removes the file): this account's list and resolved rows go; the widgets' views stay
/// registered and the next account's list loads into them.
pub(crate) fn forget_session() {
    let mut s = store().lock().unwrap_or_else(|p| p.into_inner());
    let views = std::mem::take(&mut s.views);
    *s = Store { views, ..Store::default() };
}

fn me() -> String {
    crate::session::client().and_then(|c| c.user_id().map(|u| u.to_string())).unwrap_or_default()
}

/// Load this account's list once per account (another account's file is ignored and replaced on the next write).
fn ensure_loaded(st: &mut Store) {
    let me = me();
    if me.is_empty() || st.loaded_for == me {
        return;
    }
    st.loaded_for = me.clone();
    st.list.clear();
    st.resolved.clear();
    let saved: Option<Value> = std::fs::read_to_string(file()).ok().and_then(|t| serde_json::from_str(&t).ok());
    match saved {
        Some(v) if v.get("me").and_then(Value::as_str) == Some(me.as_str()) => {
            st.list = normalize_mentions(v.get("list").unwrap_or(&Value::Null));
            st.first_fill = false;
        }
        _ => st.first_fill = true,
    }
}

fn persist(st: &Store) {
    if st.loaded_for.is_empty() {
        return;
    }
    let v = json!({ "me": st.loaded_for, "list": mentions_json(&st.list) });
    let p = file();
    let tmp = p.with_extension("json.tmp");
    if std::fs::write(&tmp, v.to_string()).is_ok() {
        let _ = std::fs::rename(&tmp, &p);
    }
}

fn localpart(mxid: &str) -> String {
    mxid.trim_start_matches('@').split(':').next().unwrap_or(mxid).to_string()
}

pub(crate) fn now_ms() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as f64).unwrap_or(0.0)
}

/// Called from the sync loop with every `sync_once` response's notifications (next to `notify::record_sync`).
pub fn record_sync(notifications: &std::collections::BTreeMap<matrix_sdk::ruma::OwnedRoomId, Vec<matrix_sdk::sync::Notification>>) {
    let Ok(mut st) = store().lock() else { return };
    ensure_loaded(&mut st);
    if st.loaded_for.is_empty() {
        return;
    }
    let first = st.first_fill;
    let mut changed = false;
    for (room_id, list) in notifications {
        let room = room_id.as_str();
        for n in list {
            if !n.actions.iter().any(|a| a.is_highlight()) {
                continue;
            }
            let matrix_sdk::deserialized_responses::RawAnySyncOrStrippedTimelineEvent::Sync(raw) = &n.event else { continue };
            let Ok(v) = raw.deserialize_as::<Value>() else { continue };
            let is_edit = v.get("content").and_then(|c| c.get("m.relates_to")).and_then(|r| r.get("rel_type")).and_then(Value::as_str) == Some("m.replace");
            if is_edit {
                continue;
            }
            let (Some(e), Some(s)) = (v.get("event_id").and_then(Value::as_str), v.get("sender").and_then(Value::as_str)) else { continue };
            let t = v.get("origin_server_ts").and_then(Value::as_f64).unwrap_or(0.0);
            // 1-2 (threads): the thread it is in (the relation is outside the encryption), for its badge read point
            let th = tinhat_widgets::threads::thread_of(v.get("content").unwrap_or(&Value::Null));
            if let Ok(mut g) = mention_threads().lock() {
                g.insert(e.to_string(), th.clone());
            }
            // 1-3: on screen = its own timeline is (a thread's mention isn't seen because the room's main timeline is)
            let shown = !first && crate::notify::room_attended_for_thread(room, &th);
            let mut m = Mention::new(room, e, s, t);
            if first || shown {
                m.seen = true;
            }
            let next = add_mention(&st.list, m);
            if next != st.list {
                st.list = next;
                changed = true;
            }
        }
    }
    st.first_fill = false;
    // the first sync of a device writes the list even when it brought nothing: from now on, new is new
    if changed || first {
        persist(&st);
    }
    if changed {
        push(&st);
    }
}

/// 1-2 (threads): this device's unseen mentions in `room` — (event id, ts) — for notify::badge_counts, which reads each
/// one against its own timeline's receipts.
pub(crate) fn unseen_list(room: &str) -> Vec<(String, f64)> {
    let Ok(mut st) = store().lock() else { return Vec::new() };
    ensure_loaded(&mut st);
    st.list.iter().filter(|m| m.r == room && !m.seen).map(|m| (m.e.clone(), m.t)).collect()
}

/// 1-2: the thread each recorded mention is in ("" = the main timeline), as far as this run has seen it (record_sync;
/// [`resolve_threads_later`] looks the rest up). Absent = not known yet: neither main nor a thread.
pub(crate) fn mention_threads() -> &'static Mutex<std::collections::HashMap<String, String>> {
    static T: std::sync::OnceLock<Mutex<std::collections::HashMap<String, String>>> = std::sync::OnceLock::new();
    T.get_or_init(Default::default)
}

/// 2026-10-23's single-read-point rule (one unthreaded receipt for the room) — 1-2 replaced it in the badge with
/// notify::count_unseen_by_thread; kept for its tests.
#[cfg(test)]
fn count_unseen_after(list: &[Mention], room: &str, read_ts: u64) -> u64 {
    list.iter().filter(|m| m.r == room && !m.seen && m.t > read_ts as f64).count() as u64
}

/// A room is on screen in this app (`notify::set_attending` / `update_view`): its mentions so far are read —
/// 1-3: those of the timelines on screen (threads::seen_on_attend), not a closed thread's. A mention whose thread isn't
/// known yet (recorded in an earlier run) waits for its lookup, then this runs again.
pub fn room_attended(room: &str) {
    let shown = crate::notify::shown_threads(room);
    let Ok(mut st) = store().lock() else { return };
    ensure_loaded(&mut st);
    let threads = mention_threads().lock().map(|g| g.clone()).unwrap_or_default();
    let ids = attended_seen_ids(&st.list, room, &shown, now_ms(), &threads);
    let unknown: Vec<String> = st.list.iter().filter(|m| m.r == room && !m.seen && !threads.contains_key(&m.e)).map(|m| m.e.clone()).collect();
    if !unknown.is_empty() {
        resolve_threads_later(room, unknown);
    }
    if ids.is_empty() {
        return;
    }
    let mut list = st.list.clone();
    for e in &ids {
        list = mark_seen(&list, &SeenSel { e: Some(e.clone()), r: None, up_to: None });
    }
    st.list = list;
    persist(&st);
    push(&st);
}

/// 1-3: the unseen mentions of `room` a room coming on screen marks seen: by each mention's thread (`threads`: event →
/// thread, as recorded or looked up). A mention whose thread isn't known is not marked (it isn't known to be on screen).
pub(crate) fn attended_seen_ids(list: &[Mention], room: &str, shown: &[String], up_to: f64, threads: &HashMap<String, String>) -> Vec<String> {
    let ms: Vec<(String, f64, String)> = list
        .iter()
        .filter(|m| m.r == room && !m.seen)
        .filter_map(|m| threads.get(&m.e).map(|th| (m.e.clone(), m.t, th.clone())))
        .collect();
    tinhat_widgets::threads::seen_on_attend(&ms, shown, up_to)
}

/// A mention's thread lookup answered (`content` = the event's content) or failed (None): only an answer is recorded —
/// a failed lookup leaves the thread unknown, so the mention stays unseen until its timeline is known to be on screen
/// (Mark as read and "Mark all as seen" still clear it). True = recorded.
pub(crate) fn record_lookup(threads: &mut HashMap<String, String>, event: &str, content: Option<&Value>) -> bool {
    let Some(c) = content else { return false };
    threads.insert(event.to_string(), tinhat_widgets::threads::thread_of(c));
    true
}

/// What Mark as read clears in the Mentions list (D03.4): the room's mentions up to `up_to`, whatever their thread.
fn read_selection(room: &str, up_to: f64) -> SeenSel {
    SeenSel { e: None, r: Some(room.to_string()), up_to: Some(up_to) }
}
/// What "Mark all as seen" clears: everything.
fn all_selection() -> SeenSel {
    SeenSel::default()
}

/// When each mention's thread lookup was last tried (a failed one waits [`RESOLVE_RETRY`] before the next try).
fn resolve_tries() -> &'static Mutex<HashMap<String, std::time::Instant>> {
    static R: OnceLock<Mutex<HashMap<String, std::time::Instant>>> = OnceLock::new();
    R.get_or_init(Default::default)
}
const RESOLVE_RETRY: std::time::Duration = std::time::Duration::from_secs(120);

/// 1-2 / 1-3: look up, in the background, the thread of mentions recorded in an earlier run (the relation is outside the
/// encryption; one event at a time, 5 s each, a failure retried after [`RESOLVE_RETRY`]). When something resolved and
/// the room is on screen, [`room_attended`] runs again.
pub(crate) fn resolve_threads_later(room: &str, events: Vec<String>) {
    let now = std::time::Instant::now();
    let todo: Vec<String> = match resolve_tries().lock() {
        Ok(mut g) => {
            let due: Vec<String> = events.into_iter().filter(|e| g.get(e).is_none_or(|t| now.duration_since(*t) >= RESOLVE_RETRY)).collect();
            for e in &due {
                g.insert(e.clone(), now);
            }
            due
        }
        Err(_) => return,
    };
    if todo.is_empty() {
        return;
    }
    let gen = crate::session::generation();
    let room = room.to_string();
    crate::bridge::runtime().spawn(async move {
        let Some(client) = crate::session::client() else { return };
        let Ok(rid) = matrix_sdk::ruma::RoomId::parse(&room) else { return };
        let Some(r) = client.get_room(&rid) else { return };
        let mut any = false;
        for e in todo {
            let Ok(eid) = matrix_sdk::ruma::OwnedEventId::try_from(e.as_str()) else { continue };
            let fut: std::pin::Pin<Box<dyn std::future::Future<Output = _> + Send + '_>> = Box::pin(r.load_or_fetch_event(&eid, None));
            let v = match tokio::time::timeout(std::time::Duration::from_secs(5), fut).await {
                Ok(Ok(ev)) => Some(ev.raw().deserialize_as::<Value>().unwrap_or_default()),
                _ => None, // failed / timed out: tried again after RESOLVE_RETRY
            };
            if crate::session::generation() != gen {
                return;
            }
            if let Ok(mut g) = mention_threads().lock() {
                any |= record_lookup(&mut g, &e, v.as_ref().map(|v| v.get("content").unwrap_or(&Value::Null)));
            }
        }
        if any && crate::notify::is_open_room(&room) {
            room_attended(&room);
        }
    });
}

/// D03.4: a room marked read — its mentions up to `up_to` (ms; when its newest event was looked up) are read.
pub fn room_seen_up_to(room: &str, up_to: f64) {
    let Ok(mut st) = store().lock() else { return };
    ensure_loaded(&mut st);
    if !st.list.iter().any(|m| m.r == room && !m.seen) {
        return;
    }
    st.list = mark_seen(&st.list, &read_selection(room, up_to));
    persist(&st);
    push(&st);
}


/// Hand every view its counts, summary and the rows it wants (resolving what is not resolved yet in the background).
fn push(st: &Store) {
    let names = room_names(&st.list);
    let summary = mention_summary(&st.list, |r| names.get(r).cloned().unwrap_or_default());
    let unseen = unseen_count(&st.list) as i32;
    let total = st.list.len() as i32;
    for (qt, want) in st.views.values() {
        let rows = rows_of(st, *want as usize);
        let summary = summary.clone();
        let _ = qt.queue(move |mut o| {
            o.as_mut().set_unseen(unseen);
            o.as_mut().set_total(total);
            o.as_mut().set_summary(QString::from(&summary));
            o.as_mut().set_rows_json(QString::from(&rows));
        });
    }
    let missing: Vec<(String, String)> = st
        .views
        .values()
        .map(|(_, w)| *w as usize)
        .max()
        .map(|n| st.list.iter().take(n).filter(|m| !st.resolved.contains_key(&m.e)).map(|m| (m.r.clone(), m.e.clone())).collect())
        .unwrap_or_default();
    if !missing.is_empty() {
        crate::bridge::runtime().spawn(resolve(missing));
    }
}

/// Room names from the local store (the rail's names; "" while unknown).
fn room_names(list: &[Mention]) -> HashMap<String, String> {
    let mut out = HashMap::new();
    if let Some(c) = crate::session::client() {
        for m in list {
            if out.contains_key(&m.r) {
                continue;
            }
            let name = matrix_sdk::ruma::RoomId::parse(&m.r)
                .ok()
                .and_then(|id| c.get_room(&id))
                .and_then(|r| r.cached_display_name().map(|d| d.to_string()).or_else(|| r.name()))
                .unwrap_or_default();
            out.insert(m.r.clone(), name);
        }
    }
    out
}

fn rows_of(st: &Store, n: usize) -> String {
    let names = room_names(&st.list);
    let rows: Vec<Value> = st
        .list
        .iter()
        .take(n)
        .map(|m| {
            let mut o = m.to_json();
            o["seen"] = Value::Bool(m.seen);
            o["room"] = Value::String(names.get(&m.r).cloned().unwrap_or_default());
            match st.resolved.get(&m.e) {
                Some(r) => {
                    for k in ["name", "avatar", "kind", "text"] {
                        o[k] = r.get(k).cloned().unwrap_or_default();
                    }
                    o["ready"] = Value::Bool(true);
                }
                None => {
                    o["name"] = Value::String(localpart(&m.s));
                    o["avatar"] = Value::String(String::new());
                    o["kind"] = Value::String("text".into());
                    o["text"] = Value::String(String::new());
                    o["ready"] = Value::Bool(false);
                }
            }
            o
        })
        .collect();
    Value::Array(rows).to_string()
}

/// Resolve rows off the GUI thread: each event as the timeline decrypts it (`Room::event`), its sender's room name and
/// avatar, the shared snippet. Kept in memory; a failed lookup resolves to "Message not available".
async fn resolve(missing: Vec<(String, String)>) {
    let Some(client) = crate::session::client() else { return };
    let mut avatars: HashMap<String, String> = HashMap::new();
    for (r, e) in missing {
        let Ok(rid) = matrix_sdk::ruma::RoomId::parse(&r) else { continue };
        let Some(room) = client.get_room(&rid) else { continue };
        let row = match matrix_sdk::ruma::EventId::parse(&e) {
            Ok(eid) => match room.event(&eid, None).await {
                Ok(ev) => {
                    let v = ev.raw().deserialize_as::<Value>().unwrap_or(Value::Null);
                    let sender = v.get("sender").and_then(Value::as_str).unwrap_or("").to_string();
                    let sn = tinhat_widgets::recent::snippet_of(&json!({
                        "type": v.get("type").cloned().unwrap_or_default(),
                        "content": v.get("content").cloned().unwrap_or_default(),
                        "redacted": v.get("unsigned").and_then(|u| u.get("redacted_because")).is_some(),
                    }));
                    let name = crate::room::resolve_member_name(&room, &sender).await;
                    let avatar = crate::room::sender_avatar(&room, &sender, &mut avatars).await;
                    json!({ "name": name, "avatar": avatar, "kind": sn.kind, "text": sn.text })
                }
                Err(_) => json!({ "name": "", "avatar": "", "kind": "other", "text": "Message not available" }),
            },
            Err(_) => continue,
        };
        if let Ok(mut st) = store().lock() {
            // a lookup that failed keeps the sender's localpart as the name
            let mut row = row;
            if row.get("name").and_then(Value::as_str).unwrap_or("").is_empty() {
                if let Some(m) = st.list.iter().find(|m| m.e == e) {
                    row["name"] = Value::String(localpart(&m.s));
                }
            }
            st.resolved.insert(e.clone(), row);
        }
    }
    if let Ok(st) = store().lock() {
        push_rows_only(&st);
    }
}

/// After a resolve: hand the views their rows again (no new lookups — avoids a loop on a failing one).
fn push_rows_only(st: &Store) {
    for (qt, want) in st.views.values() {
        let rows = rows_of(st, *want as usize);
        let _ = qt.queue(move |mut o| o.as_mut().set_rows_json(QString::from(&rows)));
    }
}

impl qobject::MentionsController {
    fn view_id(mut self: Pin<&mut Self>) -> u64 {
        if self.rust().id == 0 {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
            self.as_mut().rust_mut().id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        self.rust().id
    }

    pub fn start(mut self: Pin<&mut Self>) {
        let id = self.as_mut().view_id();
        let want = self.rust().want;
        let qt = self.qt_thread();
        if let Ok(mut st) = store().lock() {
            ensure_loaded(&mut st);
            st.views.insert(id, (qt, want));
            push(&st);
        }
    }

    pub fn want_rows(mut self: Pin<&mut Self>, n: i32) {
        let n = n.clamp(0, 50);
        self.as_mut().rust_mut().want = n;
        let id = self.as_mut().view_id();
        let qt = self.qt_thread();
        if let Ok(mut st) = store().lock() {
            if n == 0 {
                st.views.remove(&id);
                return;
            }
            ensure_loaded(&mut st);
            st.views.insert(id, (qt, n));
            push(&st);
        }
    }

    pub fn mark_seen(self: Pin<&mut Self>, event_id: &QString) {
        let e = event_id.to_string();
        if let Ok(mut st) = store().lock() {
            ensure_loaded(&mut st);
            st.list = mark_seen(&st.list, &SeenSel { e: Some(e), r: None, up_to: None });
            persist(&st);
            push(&st);
        }
    }

    pub fn mark_all_seen(self: Pin<&mut Self>) {
        if let Ok(mut st) = store().lock() {
            ensure_loaded(&mut st);
            st.list = mark_seen(&st.list, &all_selection());
            persist(&st);
            push(&st);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marking_an_older_message_read_keeps_a_newer_mentions_highlight() {
        let list = vec![Mention::new("!a:x", "$b", "@b:x", 2000.0)];
        // the receipt went out at 3000, after mention B, but it points at message A from 1000
        let read = crate::notify::read_point(&[(Some(1000), 3000)]);
        assert_eq!(read, 1000);
        assert_eq!(count_unseen_after(&list, "!a:x", read), 1, "B is newer than what was read");
    }

    #[test]
    fn a_receipt_on_a_newer_event_still_reads_the_mention() {
        let list = vec![Mention::new("!a:x", "$b", "@b:x", 2000.0)];
        let read = crate::notify::read_point(&[(Some(2500), 2600)]);
        assert_eq!(count_unseen_after(&list, "!a:x", read), 0);
    }

    #[test]
    fn the_read_point_is_the_furthest_receipt_and_falls_back_to_the_receipts_time() {
        use crate::notify::read_point;
        assert_eq!(read_point(&[]), 0);
        assert_eq!(read_point(&[(Some(1000), 9000), (Some(2500), 3000)]), 2500, "public and private: the further one");
        assert_eq!(read_point(&[(None, 3000)]), 3000, "target unknown: never brings back what was read elsewhere");
        assert_eq!(read_point(&[(None, 0), (Some(1000), 1100)]), 1000);
    }

    #[test]
    fn a_badge_counts_unseen_mentions_after_the_accounts_own_read_receipt() {
        let mut seen = Mention::new("!a:x", "$3", "@b:x", 3000.0);
        seen.seen = true;
        let list = vec![
            Mention::new("!a:x", "$1", "@b:x", 1000.0),
            Mention::new("!a:x", "$2", "@b:x", 2000.0),
            seen,
            Mention::new("!other:x", "$4", "@b:x", 4000.0),
        ];
        assert_eq!(count_unseen_after(&list, "!a:x", 0), 2, "never read: both unseen ones");
        assert_eq!(count_unseen_after(&list, "!a:x", 1500), 1, "read elsewhere between them: only the newer one");
        assert_eq!(count_unseen_after(&list, "!a:x", 2500), 0, "read elsewhere after both: none come back");
        assert_eq!(count_unseen_after(&list, "!other:x", 2500), 1, "per room");
    }
}

#[cfg(test)]
mod fix1216_seen_tests {
    use super::*;

    /// 1-3: the room's main timeline coming on screen marks the main mention seen, not the one in a closed thread;
    /// that thread's view coming on screen marks its own.
    #[test]
    fn a_closed_threads_mention_stays_unseen_when_the_room_shows() {
        let list = vec![Mention::new("!r", "$main", "@a:x", 100.0), Mention::new("!r", "$inA", "@a:x", 200.0), Mention::new("!other", "$o", "@a:x", 50.0)];
        let threads: HashMap<String, String> = [("$inA".to_string(), "$rootA".to_string()), ("$main".to_string(), String::new())].into_iter().collect();
        assert_eq!(attended_seen_ids(&list, "!r", &[String::new()], 1000.0, &threads), vec!["$main".to_string()]);
        assert_eq!(attended_seen_ids(&list, "!r", &["$rootA".to_string()], 1000.0, &threads), vec!["$inA".to_string()]);
        assert!(attended_seen_ids(&list, "!r", &[], 1000.0, &threads).is_empty(), "nothing on screen");
        // review: a mention from an earlier run whose thread isn't known yet is not marked by the main timeline showing
        let mut more = list.clone();
        more.push(Mention::new("!r", "$old", "@a:x", 150.0));
        assert_eq!(attended_seen_ids(&more, "!r", &[String::new()], 1000.0, &threads), vec!["$main".to_string()], "unknown waits for its lookup");
    }

    /// 1-3 ruling (a), launch-gate 11-14: a mention from an earlier run whose thread lookup FAILS stays unseen while its
    /// room shows; Mark as read and "Mark all as seen" still clear it.
    #[test]
    fn a_failed_thread_lookup_keeps_the_mention_unseen_but_read_and_mark_all_clear_it() {
        let list = vec![Mention::new("!r", "$old", "@a:x", 150.0)];
        let mut threads: HashMap<String, String> = HashMap::new();
        assert!(!record_lookup(&mut threads, "$old", None), "a failed lookup records nothing");
        assert!(attended_seen_ids(&list, "!r", &[String::new()], 1000.0, &threads).is_empty(), "the room on screen: still unseen");
        assert!(mark_seen(&list, &read_selection("!r", 1000.0))[0].seen, "Mark as read clears it");
        assert!(mark_seen(&list, &all_selection())[0].seen, "Mark all as seen clears it");
        assert!(!mark_seen(&list, &read_selection("!other", 1000.0))[0].seen, "another room's Mark as read doesn't");
        // and an answer later records it (main here): then the main timeline showing marks it
        assert!(record_lookup(&mut threads, "$old", Some(&json!({"body": "hi"}))));
        assert_eq!(attended_seen_ids(&list, "!r", &[String::new()], 1000.0, &threads), vec!["$old".to_string()]);
    }
}
