//! GuestsController — the guest room on the desktop (contract: docs/GUESTS.md in the server repo; the desktop half:
//! docs/GUESTS.md here). Tin Hat Web twin: `src/features/guests/` — store.ts (the API), watch.ts (noticing),
//! GuestsBar.tsx (the bar), GuestsPane.tsx (Settings → Guests); the rules are guests.rs (model.ts).
//!
//! * **The doc** (GET /api/guests: the room, the link, this user's roles) is read at sign-in, when Settings → Guests or
//!   the Guests room's bar shows, when the room's `dev.tinhat.guests` changes, and after every action of ours — never
//!   on a timer: on/off and who waits come through sync. One read at a time; "Copy guest link" reads it first unless
//!   it is under 30 s old (a new link writes no room state, so another manager's renewal shows only there).
//! * **THE Guests room** is followed through sync ([`on_sync`], one line in bridge.rs's sync loop): its on/off, who
//!   waits, its power levels and my membership are re-read from the store whenever one of them changes there (the
//!   first pass of a session looks at every room: watch.ts checkAll).
//! * **Noticing** (watch.ts): a new id in `dev.tinhat.guests.waiting` while this user may let people in → "Ana is
//!   waiting to come in as a guest": the in-app toast with Open and the OS notification (Desktop notifications on;
//!   click → the room), unless the room is the open room in a focused window — and the notify chime either way. Each
//!   id once per device (the device pref `ui-guests-seen`, the web's `guests.seen`); the OS notification closes when
//!   that guest leaves the list (let in or turned away by anyone — Linux; a WinRT toast can't be withdrawn and
//!   expires by itself).
//! * **QML** reads one JSON (`stateJson`, `rev` bumps on a change) with everything it draws already decided — the
//!   words included ([`view_json`]); one-shot answers come as signals (toast, copyReady, openReady).
//!
//! The desktop is a self-hosted build only (docs/SELFHOST.md): the guest room is there whenever the server's offer
//! doesn't say `guests: false` (`server_offer::feature_on_global(Guests)`).

use core::pin::Pin;
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use cxx_qt::Threading;
use cxx_qt_lib::QString;
use serde_json::{json, Value};

use crate::guests::{
    arrivals, bar_plan, copy_offered, departures, is_guest_account, is_guests_room, is_waiter_id, local_role, merge_role, notice_text, pane_plan,
    parse_doc, parse_guest_state, parse_waiting, waiting_rows, waiting_text, BarButton, GuestRoomState, GuestsDoc, PaneButton, Waiter,
    EXPLAIN, GUESTS_TYPE, WAITING_TYPE, WORDS,
};
use crate::server_offer::ServerFeature;

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
        #[qproperty(i32, rev)] //            QML: rev — bumps whenever stateJson changes
        #[qproperty(QString, state_json)] // QML: stateJson — the guest room as the app draws it (guests_ctl.rs view_json)
        type GuestsController = super::GuestsControllerRust;

        /// QML: start() — signed in: follow the Guests room, read the doc.
        #[qinvokable]
        fn start(self: Pin<&mut GuestsController>);
        /// QML: reset() — signed out: forget everything, withdraw the notices.
        #[qinvokable]
        fn reset(self: Pin<&mut GuestsController>);
        /// QML: refresh() — read the doc again (Settings → Guests or the Guests room's bar shows).
        #[qinvokable]
        fn refresh(self: Pin<&mut GuestsController>);
        /// QML: offerChanged() — the server's offer changed: look at every room again (the guest room may be on now).
        #[qinvokable]
        fn offer_changed(self: Pin<&mut GuestsController>);
        /// QML: turnOn() — "Turn on the guest room" / "Turn on" (managers).
        #[qinvokable]
        fn turn_on(self: Pin<&mut GuestsController>);
        /// QML: turnOff() — "Turn off" (managers).
        #[qinvokable]
        fn turn_off(self: Pin<&mut GuestsController>);
        /// QML: newLink() — "New link" (managers, while on).
        #[qinvokable]
        fn new_link(self: Pin<&mut GuestsController>);
        /// QML: copyLink() — "Copy guest link": copyReady(link, toast), or a toast saying why there is none.
        #[qinvokable]
        fn copy_link(self: Pin<&mut GuestsController>);
        /// QML: letIn(id) — "Let in" on a waiting row.
        #[qinvokable]
        fn let_in(self: Pin<&mut GuestsController>, id: &QString);
        /// QML: turnAway(id) — "Turn away" on a waiting row.
        #[qinvokable]
        fn turn_away(self: Pin<&mut GuestsController>, id: &QString);
        /// QML: openRoom() — "Open the Guests room": joins it first when this user isn't in it, then openReady(roomId).
        #[qinvokable]
        fn open_room(self: Pin<&mut GuestsController>);
        /// QML: isGuestAccount(mxid) — one of the server's temporary guest accounts (`@guest-…`).
        #[qinvokable]
        fn is_guest_account(self: &GuestsController, mxid: &QString) -> bool;

        /// A toast for the person (Main.qml puts it where they are looking).
        #[qsignal]
        fn toast(self: Pin<&mut GuestsController>, text: QString, timeout_ms: i32);
        /// The guest link for the clipboard, and the toast that says it was copied.
        #[qsignal]
        fn copy_ready(self: Pin<&mut GuestsController>, link: QString, text: QString);
        /// The Guests room to open now (Settings closes).
        #[qsignal]
        fn open_ready(self: Pin<&mut GuestsController>, room_id: QString);
    }

    impl cxx_qt::Threading for GuestsController {}
}

#[derive(Default)]
pub struct GuestsControllerRust {
    rev: i32,
    state_json: QString,
}

type QtThread = cxx_qt::CxxQtThread<qobject::GuestsController>;

// ---- words of the API side (store.ts, byte for byte) ------------------------------------------------------------

/// A 404 from the API: a server without the guest room (store.ts errorText).
pub const NOT_HERE: &str = "This server doesn't have the guest room yet.";
/// No answer at all (store.ts errorText, status 0).
pub const UNREACHABLE: &str = "Couldn't reach the server — try again in a moment.";
/// A 200 that isn't a doc (store.ts refreshGuests).
pub const UNEXPECTED: &str = "The server sent something unexpected.";
/// Settings → Guests before the first answer (GuestsPane.tsx).
pub const CHECKING: &str = "Checking…";
/// Settings → Guests: the section and the link row (GuestsPane.tsx).
pub const SECTION: &str = "Guest room";
pub const LINK_LABEL: &str = "Guest link";
/// "Copy guest link" (store.ts copyGuestLink).
pub const COPIED: &str = "Guest link copied — whoever opens it waits at the door until someone lets them in";
pub const OFF_NO_LINK: &str = "The guest room is off — there's no guest link right now.";
pub const NO_LINK: &str = "There's no guest link for you to copy.";
/// The in-app toast's button on a notice (watch.ts announce).
pub const OPEN: &str = "Open";

/// a doc younger than this is fresh enough to copy its link at once (store.ts FRESH_MS)
const FRESH: Duration = Duration::from_secs(30);
/// GET /guests (the web's apiJson default) and the actions (store.ts: 20 s)
const READ_TIMEOUT: Duration = Duration::from_secs(12);
const ACT_TIMEOUT: Duration = Duration::from_secs(20);

/// The device pref (`<config>/tinhat/ui-guests-seen`): the ids this device announced, a JSON array (the web's device
/// pref `guests.seen`). A device setting: it stays across sign-outs, as the web's does.
pub const SEEN_PREF: &str = "ui-guests-seen";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Manage {
    On,
    Off,
    Link,
}

impl Manage {
    fn path(self) -> &'static str {
        match self {
            Manage::On => "/guests/on",
            Manage::Off => "/guests/off",
            Manage::Link => "/guests/link",
        }
    }
    /// store.ts MANAGE_DONE
    fn done(self) -> &'static str {
        match self {
            Manage::On => "The guest room is on — copy the guest link to send it to someone",
            Manage::Off => "The guest room is off — whoever waited was turned away and the guests are gone",
            Manage::Link => "New guest link — the old one stopped working",
        }
    }
    /// store.ts MANAGE_FAIL (": <the error>" follows)
    fn fail(self) -> &'static str {
        match self {
            Manage::On => "Couldn't turn the guest room on",
            Manage::Off => "Couldn't turn the guest room off",
            Manage::Link => "Couldn't make a new link",
        }
    }
}

/// store.ts decide: the toast after Let in / Turn away.
fn decided_text(admit: bool, name: &str) -> String {
    if admit {
        format!("{name} is coming in")
    } else {
        format!("{name} was turned away")
    }
}

/// store.ts decide: the failure (": <the error>" follows).
fn decide_failed_text(admit: bool, name: &str) -> String {
    if admit {
        format!("Couldn't let {name} in")
    } else {
        format!("Couldn't turn {name} away")
    }
}

/// An API call that didn't come back 2xx.
#[derive(Clone, Debug, PartialEq)]
enum Fail {
    /// no answer (network, timeout) — the web's status 0
    Net,
    /// signed out: no API to ask
    NoServer,
    /// the answer's status and body
    Status(u16, Value),
}

/// store.ts errorText: 404 → "This server doesn't have the guest room yet."; no answer → "Couldn't reach the server — try
/// again in a moment."; else the answer's `error`, or "The server said <status>" (api.ts apiJson).
fn error_text(f: &Fail) -> String {
    match f {
        Fail::Status(404, _) => NOT_HERE.to_string(),
        Fail::Status(s, body) => match body.get("error") {
            Some(Value::String(e)) if !e.is_empty() => e.clone(),
            _ => format!("The server said {s}"),
        },
        Fail::Net => UNREACHABLE.to_string(),
        Fail::NoServer => crate::proxy::NO_SERVER.to_string(),
    }
}

/// `<b>Ana</b> is waiting` for a StyledText label (the name escaped; waiting_text's words).
fn waiting_html(name: &str) -> String {
    let esc = name.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;");
    waiting_text(&format!("<b>{esc}</b>"))
}

// ---- the process-wide store (store.ts's zustand store + watch.ts's `shown`) -------------------------------------

/// THE Guests room as sync has it.
#[derive(Clone, Debug, Default, PartialEq)]
struct RoomView {
    room_id: String,
    state: Option<GuestRoomState>,
    waiting: Vec<Waiter>,
    /// "join" / "leave" / "invite" / "knock" / "ban"
    membership: Option<String>,
    power_levels: Value,
}

#[derive(Default)]
struct Store {
    /// the session generation this store belongs to (a new session starts from nothing)
    gen: u64,
    /// the controller's thread (QML's one GuestsController)
    sink: Option<QtThread>,
    /// the first sync pass of this session looked at every room
    started: bool,
    doc: Option<GuestsDoc>,
    doc_at: Option<Instant>,
    /// the first answer (or failure) is in
    loaded: bool,
    /// why there is no doc (the server's words, or ours)
    error: String,
    /// a turn on / off / new link is on its way
    busy: bool,
    /// waiting ids whose Let in / Turn away is on its way
    pending: Vec<String>,
    view: Option<RoomView>,
    /// ids announced this session whose notices may still be up (watch.ts `shown`)
    shown: Vec<String>,
    /// waiting id → its OS notification (to withdraw it)
    notes: HashMap<String, u32>,
}

fn store() -> &'static Mutex<Store> {
    static S: OnceLock<Mutex<Store>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(Store { gen: crate::session::generation(), ..Store::default() }))
}

/// The store, for the current session: a store left from an ended session is emptied first (its notices withdrawn).
fn lock() -> MutexGuard<'static, Store> {
    let mut g = store().lock().unwrap_or_else(|e| e.into_inner());
    let gen = crate::session::generation();
    if g.gen != gen {
        let sink = g.sink.take();
        let notes: Vec<u32> = g.notes.drain().map(|(_, n)| n).collect();
        *g = Store { gen, sink, ..Store::default() };
        close_notes(notes);
    }
    g
}

fn close_notes(notes: Vec<u32>) {
    if notes.is_empty() {
        return;
    }
    crate::bridge::runtime().spawn(async move {
        for n in notes {
            crate::notify::close_notification(n).await;
        }
    });
}

fn offered() -> bool {
    crate::server_offer::feature_on_global(ServerFeature::Guests)
}

fn me_of(client: &matrix_sdk::Client) -> String {
    client.user_id().map(|u| u.to_string()).unwrap_or_default()
}

/// Redraw: QML's stateJson follows the store.
fn publish() {
    let sink = lock().sink.clone();
    if let Some(qt) = sink {
        let _ = qt.queue(|o| o.apply_view());
    }
}

fn toast(text: String, ms: i32) {
    let sink = lock().sink.clone();
    if let Some(qt) = sink {
        let _ = qt.queue(move |o| o.toast(QString::from(&text), ms));
    }
}

/// THE Guests room's id: the doc's, else the room sync recognised ("" = none known).
fn current_room(st: &Store) -> Option<String> {
    st.doc.as_ref().and_then(|d| d.room.clone()).or_else(|| st.view.as_ref().map(|v| v.room_id.clone()))
}

// ---- what QML draws -----------------------------------------------------------------------------------------------

/// The guest room as the app draws it (GuestsBar.tsx's Bar + GuestsPane.tsx, decided here):
/// `{offered, guest, room, on, bar: {text, buttons: [{id, label, highlighted, flat, enabled}]} | null,
///   rows: [{id, name, html, enabled}], pane: {state, text, buttons: [...], showLink} | null, link,
///   note, noteError, busy, words: {pane, section, explain, letIn, turnAway, link}}`.
fn view_json(st: &Store, me: &str, offered: bool) -> Value {
    let guest = is_guest_account(me);
    let doc = st.doc.as_ref();
    let doc_room = doc.and_then(|d| d.room.clone());
    // the sync view counts while it is the doc's room (or the doc names none yet)
    let view = st.view.as_ref().filter(|v| doc_room.as_deref().map_or(true, |d| d == v.room_id));
    let room = doc_room.clone().or_else(|| view.map(|v| v.room_id.clone())).unwrap_or_default();

    // the bar + its rows: the Guests room's own state through sync, the doc's roles when it names this room
    let (on, local, waiting, doc_role) = match view {
        Some(v) => (
            v.state.map(|s| s.on).or(doc.map(|d| d.on)).unwrap_or(false),
            local_role(me, v.membership.as_deref(), &v.power_levels),
            v.waiting.clone(),
            doc.filter(|d| d.room.as_deref() == Some(v.room_id.as_str())).map(GuestsDoc::role),
        ),
        None => (
            doc.map(|d| d.on).unwrap_or(false),
            local_role(me, None, &Value::Null),
            doc.map(|d| d.waiting.clone()).unwrap_or_default(),
            doc.filter(|d| d.room.is_some()).map(GuestsDoc::role),
        ),
    };
    let role = merge_role(local, doc_role);
    let bar = if offered && !room.is_empty() { bar_plan(on, role) } else { None };
    let rows = if offered && !room.is_empty() { waiting_rows(on, role, &waiting) } else { Vec::new() };

    // Settings → Guests: the doc, with on/off as sync has it (the doc may be a moment older)
    let pane = doc.map(|d| {
        let v = st.view.as_ref().filter(|v| d.room.as_deref() == Some(v.room_id.as_str()));
        let mut now = d.clone();
        if let Some(s) = v.and_then(|v| v.state) {
            now.on = s.on;
        }
        let local = match v {
            Some(v) => local_role(me, v.membership.as_deref(), &v.power_levels),
            None => local_role(me, None, &Value::Null),
        };
        pane_plan(&now, merge_role(local, Some(d.role())))
    });

    let busy = st.busy;
    let bar_json = bar.map(|b| {
        let buttons: Vec<Value> = b
            .buttons
            .iter()
            .map(|x| {
                json!({ "id": x.id(), "label": x.label(), "highlighted": *x == BarButton::TurnOn, "flat": *x != BarButton::TurnOn,
                        "enabled": !(busy && *x != BarButton::Copy) })
            })
            .collect();
        json!({ "text": b.text, "buttons": buttons })
    });
    let pane_json = pane.map(|p| {
        let buttons: Vec<Value> = p
            .buttons
            .iter()
            .map(|x| {
                json!({ "id": x.id(), "label": x.label(), "highlighted": *x == PaneButton::TurnOn,
                        "flat": matches!(x, PaneButton::TurnOff | PaneButton::NewLink),
                        "enabled": !(busy && !matches!(x, PaneButton::Copy | PaneButton::Open)) })
            })
            .collect();
        json!({ "state": p.state.name(), "text": p.text, "buttons": buttons, "showLink": p.show_link })
    });
    let rows_json: Vec<Value> =
        rows.iter().map(|w| json!({ "id": w.id, "name": w.name, "html": waiting_html(&w.name), "enabled": !st.pending.contains(&w.id) })).collect();
    let note = if doc.is_some() {
        String::new()
    } else if !st.loaded {
        CHECKING.to_string()
    } else if !st.error.is_empty() {
        st.error.clone()
    } else {
        WORDS.unavailable.to_string()
    };
    json!({
        "offered": offered,
        "guest": guest,
        "room": room,
        "on": on,
        "bar": bar_json,
        // the room menu's + header's "Copy guest link" (the web's CopyLink.tsx): where the bar offers Copy
        "copyOffered": offered && !room.is_empty() && copy_offered(on, role),
        "rows": rows_json,
        "pane": pane_json,
        "link": doc.and_then(|d| d.link.clone()).unwrap_or_default(),
        "note": note,
        "noteError": doc.is_none() && st.loaded && !st.error.is_empty(),
        "busy": busy,
        "words": { "copy": WORDS.copy, "pane": WORDS.pane, "section": SECTION, "explain": EXPLAIN, "letIn": WORDS.let_in, "turnAway": WORDS.turn_away, "link": LINK_LABEL },
    })
}

// ---- noticing (watch.ts check) ------------------------------------------------------------------------------------

/// One look at the Guests room's waiting list (watch.ts `check`, after the room was recognised).
#[derive(Debug, PartialEq)]
struct NoticePlan {
    /// ids whose notices close (they left the list)
    close: Vec<String>,
    /// the ids still announced and waiting, then the new ones
    shown: Vec<String>,
    /// the device's announced ids after this look
    seen: Vec<String>,
    /// who to announce now
    announce: Vec<Waiter>,
}

/// `eligible` = the server offers the guest room ∧ it is on ∧ this user is joined to it ∧ isn't a guest account.
fn notice_plan(shown: &[String], seen: &[String], view: &RoomView, doc_on: Option<bool>, me: &str, offered: bool, now: f64) -> NoticePlan {
    let close = departures(shown, &view.waiting);
    let mut still: Vec<String> = shown.iter().filter(|id| view.waiting.iter().any(|w| &w.id == *id)).cloned().collect();
    let on = view.state.map(|s| s.on).or(doc_on).unwrap_or(false);
    let eligible = offered && on && view.membership.as_deref() == Some("join") && !is_guest_account(me);
    let r = arrivals(seen, &view.waiting, now, eligible);
    for w in &r.notify {
        still.push(w.id.clone());
    }
    NoticePlan { close, shown: still, seen: r.seen, announce: r.notify }
}

fn read_seen() -> Vec<String> {
    crate::settings::overlay_pref_read(SEEN_PREF)
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()))
        .unwrap_or_default()
}

fn write_seen(seen: &[String]) {
    crate::settings::overlay_pref_write(SEEN_PREF, &serde_json::to_string(seen).unwrap_or_else(|_| "[]".into()));
}

/// "Ana is waiting to come in as a guest": the chime always; the in-app toast (with Open) and the OS notification
/// (Desktop notifications on; click → the room) unless the room is the open room in a focused window — its row is
/// right there. Returns the OS notification's id, to withdraw it.
async fn announce(room_id: &str, w: &Waiter) -> Option<u32> {
    let text = notice_text(&w.name);
    crate::notify::toast_action("sound:notify".to_string());
    if crate::notify::is_open_room(room_id) {
        return None;
    }
    crate::notify::toast_action(format!("guest-waiting:{room_id}\t{text}"));
    if !crate::notify::enabled() {
        return None;
    }
    crate::notify::notify_action_id(&text, "", None, &format!("open:{room_id}")).await
}

// ---- following the Guests room through sync ---------------------------------------------------------------------

fn membership_name(s: matrix_sdk::RoomState) -> &'static str {
    match s {
        matrix_sdk::RoomState::Joined => "join",
        matrix_sdk::RoomState::Left => "leave",
        matrix_sdk::RoomState::Invited => "invite",
        matrix_sdk::RoomState::Knocked => "knock",
        matrix_sdk::RoomState::Banned => "ban",
    }
}

/// Look at one room: is it THE Guests room (guests.rs is_guests_room)? Then keep what sync has of it, notice new
/// knocks, close the notices of those who left the list, and (`guests_changed`: its on/off changed) read the doc.
async fn check_room(client: &matrix_sdk::Client, rid: &str, guests_changed: bool) {
    let gen = crate::session::generation();
    let Ok(id) = matrix_sdk::ruma::RoomId::parse(rid) else { return };
    let Some(room) = client.get_room(&id) else { return };
    let me = me_of(client);
    let server = client.user_id().map(|u| u.server_name().to_string()).unwrap_or_default();
    let guests = crate::appstate::get_state(&room, GUESTS_TYPE, "").await;
    let alias = room.canonical_alias().map(|a| a.to_string());
    let doc_room = lock().doc.as_ref().and_then(|d| d.room.clone());
    if !is_guests_room(rid, doc_room.as_deref(), guests.is_some(), alias.as_deref(), &server) {
        let mut st = lock();
        if st.view.as_ref().is_some_and(|v| v.room_id == rid) {
            st.view = None;
            drop(st);
            publish();
        }
        return;
    }
    let waiting = crate::appstate::get_state(&room, WAITING_TYPE, "").await.map(|c| parse_waiting(&c)).unwrap_or_default();
    let power_levels = crate::appstate::get_state(&room, "m.room.power_levels", "").await.unwrap_or(Value::Null);
    let view = RoomView {
        room_id: rid.to_string(),
        state: guests.as_ref().and_then(parse_guest_state),
        waiting,
        membership: Some(membership_name(room.state()).to_string()),
        power_levels,
    };
    if crate::session::generation() != gen {
        return;
    }
    let ok = offered();
    let now = crate::appstate::now_ms() as f64;
    let (plan, close) = {
        let mut st = lock();
        let doc_on = st.doc.as_ref().map(|d| d.on);
        let before = read_seen();
        let plan = notice_plan(&st.shown, &before, &view, doc_on, &me, ok, now);
        if plan.seen != before {
            write_seen(&plan.seen);
        }
        let close: Vec<u32> = plan.close.iter().filter_map(|id| st.notes.remove(id)).collect();
        st.shown = plan.shown.clone();
        st.view = Some(view);
        (plan, close)
    };
    publish();
    close_notes(close);
    for w in &plan.announce {
        eprintln!("[guests] {} knocked", w.id);
        if let Some(n) = announce(rid, w).await {
            let mut st = lock();
            if st.shown.contains(&w.id) {
                st.notes.insert(w.id.clone(), n);
            } else {
                drop(st); // let in / turned away meanwhile
                close_notes(vec![n]);
            }
        }
    }
    if guests_changed {
        // turned on / off: the link and the roles may have changed (spawned, not awaited: read_doc looks at a newly
        // named room through check_room, and the two futures may not contain each other)
        spawn_read();
    }
}

/// Every joined room that carries the guest room's state (and the doc's room): the start of a session, a new offer.
async fn check_all(client: &matrix_sdk::Client) {
    let doc_room = lock().doc.as_ref().and_then(|d| d.room.clone());
    for room in client.joined_rooms() {
        let rid = room.room_id().to_string();
        let has = crate::appstate::get_state(&room, GUESTS_TYPE, "").await.is_some()
            || crate::appstate::get_state(&room, WAITING_TYPE, "").await.is_some();
        if has || doc_room.as_deref() == Some(rid.as_str()) {
            check_room(client, &rid, false).await;
        }
    }
}

/// From bridge.rs's sync loop, after every successful pass: a room whose guest state, waiting list or canonical alias
/// changed (or THE Guests room's power levels, or my own membership anywhere) is looked at again — local store reads
/// only, off the sync loop. The first pass of a session looks at every room.
pub fn on_sync(client: &matrix_sdk::Client, response: &matrix_sdk::sync::SyncResponse) {
    let (first, known) = {
        let mut st = lock();
        let first = !st.started;
        st.started = true;
        (first, current_room(&st))
    };
    if first {
        let c = client.clone();
        crate::bridge::runtime().spawn(async move { check_all(&c).await });
        return;
    }
    let me = me_of(client);
    let mut todo: Vec<(String, bool)> = Vec::new();
    for (rid, upd) in &response.rooms.joined {
        let room = rid.as_str();
        let (mut watched, mut guests_changed) = (false, false);
        let mut look = |v: &Value| {
            let Some(key) = v.get("state_key").and_then(Value::as_str) else { return };
            match v.get("type").and_then(Value::as_str).unwrap_or("") {
                GUESTS_TYPE => {
                    watched = true;
                    guests_changed = true;
                }
                WAITING_TYPE | "m.room.canonical_alias" => watched = true,
                "m.room.power_levels" if known.as_deref() == Some(room) => watched = true,
                "m.room.member" if key == me => watched = true, // joined it just now (or left it)
                _ => {}
            }
        };
        for ev in &upd.timeline.events {
            if let Ok(v) = ev.raw().deserialize_as::<Value>() {
                look(&v);
            }
        }
        let state = match &upd.state {
            matrix_sdk::sync::State::Before(v) | matrix_sdk::sync::State::After(v) => v,
        };
        for raw in state {
            if let Ok(v) = raw.deserialize_as::<Value>() {
                look(&v);
            }
        }
        if watched {
            todo.push((room.to_string(), guests_changed));
        }
    }
    for rid in response.rooms.left.keys() {
        if known.as_deref() == Some(rid.as_str()) {
            todo.push((rid.to_string(), false));
        }
    }
    for (rid, changed) in todo {
        let c = client.clone();
        crate::bridge::runtime().spawn(async move { check_room(&c, &rid, changed).await });
    }
}

// ---- the API (store.ts) -------------------------------------------------------------------------------------------

/// One request to `{proxy::base()}{path}` with the session's Matrix access token → the answer's JSON (null when
/// empty, the text when it isn't JSON — api.ts apiJson).
async fn call(method: &str, path: &str, body: Option<Value>, timeout: Duration) -> Result<Value, Fail> {
    let Some(token) = crate::session::client().and_then(|c| c.access_token()) else { return Err(Fail::NoServer) };
    let Ok(url) = crate::proxy::url(path) else { return Err(Fail::NoServer) };
    let c = crate::proxy::http(Some(timeout));
    let mut req = if method == "POST" { c.post(&url) } else { c.get(&url) };
    req = req.header("Authorization", format!("Bearer {token}"));
    if let Some(b) = body {
        req = req.header("Content-Type", "application/json").body(b.to_string());
    }
    let resp = req.send().await.map_err(|e| {
        eprintln!("[guests] {method} {path}: {e}");
        Fail::Net
    })?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    let v = if text.is_empty() { Value::Null } else { serde_json::from_str::<Value>(&text).unwrap_or(Value::String(text)) };
    if status.is_success() {
        Ok(v)
    } else {
        Err(Fail::Status(status.as_u16(), v))
    }
}

/// One read at a time: a second caller waits for the read in flight and takes its answer.
fn read_gate() -> &'static tokio::sync::Mutex<()> {
    static G: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    G.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// Read the doc again (store.ts refreshGuests). A doc that names another room than before looks at that room.
async fn read_doc() {
    if !offered() || crate::session::client().is_none() {
        return;
    }
    let gen = crate::session::generation();
    let asked = Instant::now();
    let _one = read_gate().lock().await;
    if lock().doc_at.is_some_and(|t| t >= asked) {
        return; // read while we waited
    }
    let r = call("GET", "/guests", None, READ_TIMEOUT).await;
    if crate::session::generation() != gen {
        return;
    }
    let (before, after) = {
        let mut st = lock();
        let before = current_room(&st);
        match r {
            Ok(v) => {
                let doc = parse_doc(&v);
                st.error = if doc.is_some() { String::new() } else { UNEXPECTED.to_string() };
                st.doc = doc;
                st.loaded = true;
                st.doc_at = Some(Instant::now());
            }
            Err(f) => {
                let e = error_text(&f);
                eprintln!("[guests] GET /guests: {e}");
                st.loaded = true;
                st.error = e;
            }
        }
        (before, st.doc.as_ref().and_then(|d| d.room.clone()))
    };
    publish();
    // the doc names the room (or another one) after the first look: look at it now
    if let (Some(r), Some(c)) = (after.filter(|a| before.as_deref() != Some(a.as_str())), crate::session::client()) {
        crate::bridge::runtime().spawn(async move { check_room(&c, &r, false).await });
    }
}

fn spawn_read() {
    crate::bridge::runtime().spawn(read_doc());
}

/// Turn on / off, new link (store.ts manage): one at a time; the answer is the new doc.
fn manage(what: Manage) {
    {
        let mut st = lock();
        if st.busy {
            return;
        }
        st.busy = true;
    }
    publish();
    let gen = crate::session::generation();
    crate::bridge::runtime().spawn(async move {
        let r = call("POST", what.path(), Some(json!({})), ACT_TIMEOUT).await;
        if crate::session::generation() != gen {
            return;
        }
        let mut room_now = None;
        match r {
            Ok(v) => {
                if let Some(doc) = parse_doc(&v) {
                    let mut st = lock();
                    let before = current_room(&st);
                    room_now = doc.room.clone().filter(|r| before.as_deref() != Some(r.as_str()));
                    st.doc = Some(doc);
                    st.loaded = true;
                    st.doc_at = Some(Instant::now());
                    st.error.clear();
                }
                eprintln!("[guests] {} ok", what.path());
                toast(what.done().to_string(), 5000);
            }
            Err(f) => toast(format!("{}: {}", what.fail(), error_text(&f)), 6000),
        }
        lock().busy = false;
        publish();
        // the first "Turn on" made the room (and joined us): look at it as soon as it is here
        if let (Some(r), Some(c)) = (room_now, crate::session::client()) {
            check_room(&c, &r, false).await;
        }
    });
}

/// The name a waiting id goes by (for the toasts), "A guest" when it already left the list.
fn name_of(st: &Store, id: &str) -> String {
    let from = |ws: &[Waiter]| ws.iter().find(|w| w.id == id).map(|w| w.name.clone());
    st.view
        .as_ref()
        .and_then(|v| from(&v.waiting))
        .or_else(|| st.doc.as_ref().and_then(|d| from(&d.waiting)))
        .unwrap_or_else(|| WORDS.someone.to_string())
}

/// Let in / Turn away (store.ts decide): one request per id at a time; its row's buttons wait meanwhile.
fn decide(admit: bool, id: String) {
    if !is_waiter_id(&id) {
        return;
    }
    let name = {
        let mut st = lock();
        if st.pending.contains(&id) {
            return;
        }
        st.pending.push(id.clone());
        name_of(&st, &id)
    };
    publish();
    let gen = crate::session::generation();
    crate::bridge::runtime().spawn(async move {
        let path = if admit { "/guests/admit" } else { "/guests/turn-away" };
        let r = call("POST", path, Some(json!({ "id": id })), ACT_TIMEOUT).await;
        if crate::session::generation() != gen {
            return;
        }
        match r {
            Ok(_) => toast(decided_text(admit, &name), 4000),
            Err(f) => toast(format!("{}: {}", decide_failed_text(admit, &name), error_text(&f)), 6000),
        }
        lock().pending.retain(|x| x != &id);
        publish();
    });
}

/// The text of a failed join (the Matrix error's sentence when the server sent one).
fn join_error_text(e: &matrix_sdk::Error) -> String {
    use matrix_sdk::ruma::api::error::ErrorBody;
    if let Some(api) = e.as_client_api_error() {
        if let ErrorBody::Standard(b) = &api.body {
            if !b.message.is_empty() {
                return b.message.clone();
            }
        }
    }
    e.to_string()
}

impl qobject::GuestsController {
    /// QML's stateJson follows the store (the Qt thread).
    fn apply_view(mut self: Pin<&mut Self>) {
        let me = crate::session::client().map(|c| me_of(&c)).unwrap_or_default();
        let json = view_json(&lock(), &me, offered()).to_string();
        if self.state_json().to_string() != json {
            self.as_mut().set_state_json(QString::from(&json));
            let r = *self.rev() + 1;
            self.as_mut().set_rev(r);
        }
    }

    pub fn start(mut self: Pin<&mut Self>) {
        let qt = self.qt_thread();
        lock().sink = Some(qt);
        self.as_mut().apply_view();
        let me = crate::session::client().map(|c| me_of(&c)).unwrap_or_default();
        if !is_guest_account(&me) {
            spawn_read();
        }
    }

    pub fn reset(mut self: Pin<&mut Self>) {
        {
            let mut st = lock();
            let notes: Vec<u32> = st.notes.drain().map(|(_, n)| n).collect();
            let (gen, sink) = (st.gen, st.sink.take());
            *st = Store { gen, sink, ..Store::default() };
            close_notes(notes);
        }
        self.as_mut().apply_view();
    }

    pub fn refresh(self: Pin<&mut Self>) {
        spawn_read();
    }

    pub fn offer_changed(self: Pin<&mut Self>) {
        let Some(c) = crate::session::client() else { return };
        publish();
        crate::bridge::runtime().spawn(async move {
            if !offered() {
                return;
            }
            if lock().doc.is_none() && !is_guest_account(&me_of(&c)) {
                read_doc().await;
            }
            check_all(&c).await;
        });
    }

    pub fn turn_on(self: Pin<&mut Self>) {
        manage(Manage::On);
    }

    pub fn turn_off(self: Pin<&mut Self>) {
        manage(Manage::Off);
    }

    pub fn new_link(self: Pin<&mut Self>) {
        manage(Manage::Link);
    }

    pub fn copy_link(self: Pin<&mut Self>) {
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let stale = {
                let st = lock();
                st.doc.as_ref().and_then(|d| d.link.as_ref()).is_none() || st.doc_at.map_or(true, |t| t.elapsed() > FRESH)
            };
            if stale {
                read_doc().await;
            }
            let (link, why) = {
                let st = lock();
                let link = st.doc.as_ref().and_then(|d| d.link.clone());
                let why = if !st.error.is_empty() {
                    st.error.clone()
                } else if st.doc.as_ref().is_some_and(|d| !d.on) {
                    OFF_NO_LINK.to_string()
                } else {
                    NO_LINK.to_string()
                };
                (link, why)
            };
            let _ = qt.queue(move |o| match link {
                Some(l) => o.copy_ready(QString::from(&l), QString::from(COPIED)),
                None => o.toast(QString::from(&why), 5000),
            });
        });
    }

    pub fn let_in(self: Pin<&mut Self>, id: &QString) {
        decide(true, id.to_string());
    }

    pub fn turn_away(self: Pin<&mut Self>, id: &QString) {
        decide(false, id.to_string());
    }

    pub fn open_room(self: Pin<&mut Self>) {
        let Some(room) = lock().doc.as_ref().and_then(|d| d.room.clone()) else { return };
        let Some(client) = crate::session::client() else { return };
        let qt = self.qt_thread();
        let gen = crate::session::generation();
        crate::bridge::runtime().spawn(async move {
            let Ok(id) = matrix_sdk::ruma::RoomId::parse(&room) else { return };
            let joined = client.get_room(&id).is_some_and(|r| r.state() == matrix_sdk::RoomState::Joined);
            if !joined {
                // its join rule is public: any account on the server may join it
                if let Err(e) = client.join_room_by_id(&id).await {
                    eprintln!("[guests] join {room}: {e}");
                    if crate::session::generation() == gen {
                        toast(format!("Couldn't open the Guests room: {}", join_error_text(&e)), 6000);
                    }
                    return;
                }
                eprintln!("[guests] joined the Guests room");
                spawn_read();
                let (c, r) = (client.clone(), room.clone());
                crate::bridge::runtime().spawn(async move { check_room(&c, &r, false).await });
            }
            if crate::session::generation() == gen {
                let _ = qt.queue(move |o| o.open_ready(QString::from(&room)));
            }
        });
    }

    pub fn is_guest_account(&self, mxid: &QString) -> bool {
        is_guest_account(&mxid.to_string())
    }
}

#[cfg(test)]
mod tests {
    //! The desktop's own glue around guests.rs's shared rules: what QML is handed (the bar, the rows, the pane — the
    //! web's GuestsBar / GuestsPane decisions), the noticing step (watch.ts check) and the API's error words (store.ts).
    use super::*;

    const ME: &str = "@ana:maple.example";
    const ROOM: &str = "!guests:maple.example";

    fn doc(on: bool, can_manage: bool) -> GuestsDoc {
        GuestsDoc {
            available: true,
            room: Some(ROOM.into()),
            alias: "#guests:maple.example".into(),
            on,
            since: 1.0,
            link: if on { Some("https://maple.example/call/?g=abcdefghijklmnopqrstuvwx".into()) } else { None },
            waiting: Vec::new(),
            can_manage,
            can_admit: true,
            member: true,
        }
    }
    fn waiter(id: &str, name: &str, at: f64) -> Waiter {
        Waiter { id: id.into(), name: name.into(), at }
    }
    fn view(on: bool, membership: &str, level: i64, waiting: Vec<Waiter>) -> RoomView {
        RoomView {
            room_id: ROOM.into(),
            state: Some(GuestRoomState { on, since: Some(1.0) }),
            waiting,
            membership: Some(membership.into()),
            power_levels: json!({ "users": { ME: level }, "users_default": 0, "invite": 0 }),
        }
    }
    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }
    fn labels(v: &Value) -> Vec<String> {
        v.as_array().unwrap().iter().map(|b| b["label"].as_str().unwrap().to_string()).collect()
    }

    #[test]
    fn the_api_side_words_byte_for_byte() {
        assert_eq!(error_text(&Fail::Status(404, json!({ "error": "x" }))), "This server doesn't have the guest room yet.");
        assert_eq!(error_text(&Fail::Status(403, json!({ "error": "Only a manager can do that" }))), "Only a manager can do that");
        assert_eq!(error_text(&Fail::Status(500, json!({ "error": "" }))), "The server said 500");
        assert_eq!(error_text(&Fail::Status(502, Value::String("<html>".into()))), "The server said 502");
        assert_eq!(error_text(&Fail::Net), "Couldn't reach the server — try again in a moment.");
        assert_eq!(Manage::On.done(), "The guest room is on — copy the guest link to send it to someone");
        assert_eq!(Manage::Off.done(), "The guest room is off — whoever waited was turned away and the guests are gone");
        assert_eq!(Manage::Link.done(), "New guest link — the old one stopped working");
        assert_eq!(Manage::On.fail(), "Couldn't turn the guest room on");
        assert_eq!(Manage::Off.fail(), "Couldn't turn the guest room off");
        assert_eq!(Manage::Link.fail(), "Couldn't make a new link");
        assert_eq!([Manage::On.path(), Manage::Off.path(), Manage::Link.path()], ["/guests/on", "/guests/off", "/guests/link"]);
        assert_eq!(decided_text(true, "Ana"), "Ana is coming in");
        assert_eq!(decided_text(false, "Ana"), "Ana was turned away");
        assert_eq!(decide_failed_text(true, "Ana"), "Couldn't let Ana in");
        assert_eq!(decide_failed_text(false, "Ana"), "Couldn't turn Ana away");
        assert_eq!(COPIED, "Guest link copied — whoever opens it waits at the door until someone lets them in");
        assert_eq!(OFF_NO_LINK, "The guest room is off — there's no guest link right now.");
        assert_eq!(NO_LINK, "There's no guest link for you to copy.");
        assert_eq!(UNEXPECTED, "The server sent something unexpected.");
        assert_eq!(waiting_html("Ana"), "<b>Ana</b> is waiting");
        assert_eq!(waiting_html("<i>Bo & Cy</i>"), "<b>&lt;i&gt;Bo &amp; Cy&lt;/i&gt;</b> is waiting", "a name is text, never markup");
        assert_eq!(SEEN_PREF, "ui-guests-seen");
    }

    #[test]
    fn what_qml_draws_the_bar_the_rows_the_pane() {
        let ana = waiter("abcd1234", "Ana", 1.0);
        // a manager in the room while it is on, Ana waiting, Ana's admit on its way
        let st = Store { doc: Some(doc(true, false)), loaded: true, view: Some(view(true, "join", 50, vec![ana.clone()])), pending: ids(&["abcd1234"]), ..Store::default() };
        let v = view_json(&st, ME, true);
        assert_eq!(v["room"], ROOM);
        assert_eq!(v["bar"]["text"], "Guest room is on");
        assert_eq!(labels(&v["bar"]["buttons"]), ["Copy guest link", "New link", "Turn off"]);
        assert_eq!(v["bar"]["buttons"][0]["flat"], true);
        assert_eq!(v["rows"], json!([{ "id": "abcd1234", "name": "Ana", "html": "<b>Ana</b> is waiting", "enabled": false }]));
        assert_eq!(v["pane"]["state"], "on");
        assert_eq!(labels(&v["pane"]["buttons"]), ["Copy guest link", "New link", "Turn off", "Open the Guests room"]);
        assert_eq!(v["pane"]["showLink"], true);
        assert_eq!(v["link"], "https://maple.example/call/?g=abcdefghijklmnopqrstuvwx");
        assert_eq!(v["note"], "");
        assert_eq!(v["words"]["letIn"], "Let in");
        assert_eq!(v["words"]["turnAway"], "Turn away");
        assert_eq!(v["words"]["section"], "Guest room");
        assert_eq!(v["words"]["link"], "Guest link");
        assert_eq!(v["words"]["explain"], EXPLAIN);
        // sync says off before the doc does: the bar and the pane follow sync
        let st = Store { doc: Some(doc(true, false)), loaded: true, view: Some(view(false, "join", 50, vec![])), busy: true, ..Store::default() };
        let v = view_json(&st, ME, true);
        assert_eq!(v["bar"]["text"], "Guest room is off");
        assert_eq!(labels(&v["bar"]["buttons"]), ["Turn on"]);
        assert_eq!(v["bar"]["buttons"][0]["highlighted"], true);
        assert_eq!(v["bar"]["buttons"][0]["enabled"], false, "a turn on / off is on its way");
        assert_eq!(v["pane"]["state"], "off");
        assert_eq!(labels(&v["pane"]["buttons"]), ["Turn on the guest room", "Open the Guests room"]);
        assert_eq!(v["pane"]["buttons"][1]["enabled"], true, "Open stays usable");
        assert_eq!(v["rows"], json!([]));
        // a member (level 0): Copy only; the server says no: nothing at all
        let st = Store { doc: Some(doc(true, false)), loaded: true, view: Some(view(true, "join", 0, vec![ana.clone()])), ..Store::default() };
        let v = view_json(&st, "@bo:maple.example", true);
        assert_eq!(labels(&v["bar"]["buttons"]), ["Copy guest link"]);
        assert_eq!(v["rows"][0]["enabled"], true);
        let v = view_json(&st, "@bo:maple.example", false);
        assert_eq!(v["bar"], Value::Null);
        assert_eq!(v["rows"], json!([]));
        // a guest account: no bar, no rows (the pane is hidden by QML)
        let v = view_json(&st, "@guest-abc:maple.example", true);
        assert_eq!(v["guest"], true);
        assert_eq!(v["bar"], Value::Null);
        assert_eq!(v["rows"], json!([]));
        // a server admin outside the room: the doc's roles, no rows until they join
        let st = Store { doc: Some(GuestsDoc { member: false, can_admit: false, ..doc(true, true) }), loaded: true, ..Store::default() };
        let v = view_json(&st, "@admin:maple.example", true);
        assert_eq!(labels(&v["bar"]["buttons"]), ["Copy guest link", "New link", "Turn off"]);
        assert_eq!(labels(&v["pane"]["buttons"]), ["Copy guest link", "New link", "Turn off", "Open the Guests room"]);
        // no doc yet: "Checking…", then the server's words
        let v = view_json(&Store::default(), ME, true);
        assert_eq!((v["note"].as_str(), v["noteError"].as_bool(), v["pane"].is_null(), v["room"].as_str()), (Some("Checking…"), Some(false), true, Some("")));
        let st = Store { loaded: true, error: NOT_HERE.into(), ..Store::default() };
        let v = view_json(&st, ME, true);
        assert_eq!((v["note"].as_str(), v["noteError"].as_bool()), (Some(NOT_HERE), Some(true)));
        // a room sync recognised before the doc named it (the server's alias): its bar shows
        let st = Store { loaded: true, error: UNREACHABLE.into(), view: Some(view(true, "join", 0, vec![ana])), ..Store::default() };
        let v = view_json(&st, "@bo:maple.example", true);
        assert_eq!(v["room"], ROOM);
        assert_eq!(v["bar"]["text"], "Guest room is on");
        // the doc names another room: the old view no longer counts
        let st = Store { doc: Some(GuestsDoc { room: Some("!other:maple.example".into()), ..doc(true, false) }), loaded: true, view: Some(view(false, "join", 50, vec![])), ..Store::default() };
        let v = view_json(&st, ME, true);
        assert_eq!(v["room"], "!other:maple.example");
        assert_eq!(v["bar"]["text"], "Guest room is on", "the doc's on, the doc's roles");
    }

    #[test]
    fn noticing_new_knocks_once_departures_close_only_while_eligible() {
        let now = 10_000_000.0;
        let ana = waiter("abcd1234", "Ana", now - 1000.0);
        let bo = waiter("efgh5678", "Bo", now - 10.0);
        let v = view(true, "join", 0, vec![ana.clone()]);
        let p = notice_plan(&[], &[], &v, None, ME, true, now);
        assert_eq!(p, NoticePlan { close: vec![], shown: ids(&["abcd1234"]), seen: ids(&["abcd1234"]), announce: vec![ana.clone()] });
        // the same list again: nothing
        let p = notice_plan(&p.shown, &p.seen, &v, None, ME, true, now + 5.0);
        assert!(p.announce.is_empty());
        // Ana let in, Bo knocks: Ana's notice closes, Bo is announced
        let v2 = view(true, "join", 0, vec![bo.clone()]);
        let p = notice_plan(&p.shown, &p.seen, &v2, None, ME, true, now + 10.0);
        assert_eq!(p.close, ids(&["abcd1234"]));
        assert_eq!(p.shown, ids(&["efgh5678"]));
        assert_eq!(p.announce, vec![bo.clone()]);
        assert_eq!(p.seen, ids(&["abcd1234", "efgh5678"]));
        // not eligible: off (sync, else the doc), not joined, a guest account, the server says no
        for (v, doc_on, me, offered) in [
            (view(false, "join", 0, vec![ana.clone()]), Some(true), ME, true),
            (RoomView { state: None, ..view(true, "join", 0, vec![ana.clone()]) }, Some(false), ME, true),
            (RoomView { state: None, ..view(true, "join", 0, vec![ana.clone()]) }, None, ME, true),
            (view(true, "leave", 0, vec![ana.clone()]), None, ME, true),
            (view(true, "join", 0, vec![ana.clone()]), None, "@guest-abc:maple.example", true),
            (view(true, "join", 0, vec![ana.clone()]), None, ME, false),
        ] {
            let p = notice_plan(&[], &[], &v, doc_on, me, offered, now);
            assert!(p.announce.is_empty() && p.seen.is_empty(), "{me} {offered} {doc_on:?} {:?}", v.membership);
        }
        // sync says nothing yet, the doc says on: eligible
        let p = notice_plan(&[], &[], &RoomView { state: None, ..view(true, "join", 0, vec![ana.clone()]) }, Some(true), ME, true, now);
        assert_eq!(p.announce, vec![ana]);
    }
}
