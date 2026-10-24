//! Squad calendar (0.8.26, 2026-09-08): game nights, free play, scheduled progression
//! ("boss run — join if you can"), time PROPOSALS with votes, standing per-game rules, and a
//! who-will-be-there-at-a-glance RSVP list — all living in the SPACE so every room in it shares
//! one calendar.
//!
//! Storage = three custom room-STATE events in the CONTAINER room (the room's parent space,
//! or the room itself when it is not in a space). State, not messages, on purpose: state is
//! unencrypted and readable with one store query, and every client (including the web fork
//! later) sees the same calendar without crypto plumbing.
//!   • `dev.tinhat.calendar.event`  state_key = event id (`ev-<epoch ms>-<hex>`)
//!       { title, game, kind: session|freeplay|milestone|proposal|cooloff, start, end (epoch ms;
//!         0 = open-ended), allDay, notes, rules, freePlay, joinIfYouCan, options:[{start,end}],
//!         chosen (-1 = not locked), createdBy, createdAt, updatedAt, canceled }
//!       `{}` = deleted.
//!   • `dev.tinhat.calendar.rsvp`   state_key = MY user id (Matrix only lets a sender write
//!       a state_key that starts with `@` for itself — which is exactly the guarantee we want:
//!       nobody can RSVP for you). Content = { <eventId>: { status: yes|maybe|no, options:[idx],
//!       at } … }; entries for events that ended >30 days ago are pruned on write.
//!   • `dev.tinhat.calendar.game`   state_key = slug: { name, color, rules, server }.
//! All three are in `appstate::TINHAT_MEMBER_STATE_EVENTS`, so an admin opening the calendar
//! lowers their required power level to 0 once and every member can schedule and RSVP.
//!
//! Reminders: a 60 s loop over every joined room fires a desktop notification for events
//! created by someone else in the last 24 h ("New on the calendar") and 30 min before an
//! event the user is going to / maybe / created ("Starting in 25 min"). Fired keys persist in
//! `<data>/calendar-seen.json` so restarts never re-toast.
//!
//! Times cross the boundary as epoch MILLISECONDS (integers) and are rendered in the user's
//! local zone by QML; the Rust side never formats wall-clock time (no TZ database in-tree).

use core::pin::Pin;
use std::collections::{HashMap, HashSet};

use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;
use matrix_sdk::ruma::events::room::message::RoomMessageEventContent;
use matrix_sdk::ruma::events::StateEventType;
use matrix_sdk::ruma::{RoomId, UserId};
use matrix_sdk::{Client, Room};
use serde_json::{json, Value};

pub const EVENT_TYPE: &str = "dev.tinhat.calendar.event";
pub const RSVP_TYPE: &str = "dev.tinhat.calendar.rsvp";
pub const GAME_TYPE: &str = "dev.tinhat.calendar.game";

/// "Starting soon" reminder lead time.
const REMIND_BEFORE_MS: u64 = 30 * 60_000;
/// An event created by someone else within this window (and still in the future) toasts once.
const NEW_EVENT_WINDOW_MS: u64 = 24 * 3600_000;
/// The header chip / upcoming strip looks this far ahead.
const UPCOMING_WINDOW_MS: u64 = 14 * 24 * 3600_000;
/// An event with no end is assumed to run this long (for "still on" / pruning).
const DEFAULT_DURATION_MS: u64 = 3 * 3600_000;
/// RSVP entries for events that ended this long ago are dropped on the next write.
const RSVP_PRUNE_MS: u64 = 30 * 24 * 3600_000;

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
        #[qproperty(QString, room_id)] //          QML: roomId — the room the calendar was opened from
        #[qproperty(QString, container_id)] //     QML: containerId — space (or orphan room) whose calendar is shown
        #[qproperty(QString, container_label)] //  QML: containerLabel — its display name
        #[qproperty(QString, events_json)] //      QML: eventsJson — [{id,title,game,…,rsvp:{yes,maybe,no},my:{status,options}}] sorted by start
        #[qproperty(QString, games_json)] //       QML: gamesJson — [{slug,name,color,rules,server}]
        #[qproperty(QString, upcoming_json)] //    QML: upcomingJson — next events of THIS container (header chip)
        #[qproperty(QString, containers_json)] //  QML: containersJson — [{id,label,avatar}] every space + the current container
        #[qproperty(bool, can_edit)] //            QML: canEdit — I may write events here (after PL provisioning)
        #[qproperty(bool, is_mod)] //              QML: isMod — my power level ≥ 50 (edit/delete anyone's event)
        #[qproperty(QString, my_id)] //            QML: myId — my user id
        #[qproperty(QString, last_error)] //       QML: lastError — friendly message on a failed write ("" = ok)
        #[qproperty(bool, busy)] //                QML: busy — a refresh is in flight
        #[qproperty(bool, reminders)] //           QML: reminders — desktop reminders on/off (persisted)
        #[qproperty(QString, focus_event)] //      QML: focusEvent — event id the UI should open next ("" = none)
        #[qproperty(bool, auto_open)] //           QML: autoOpen — TINHAT_GUI_CALENDAR=1: open the window once a container is known (gates/screenshots)
        #[qproperty(QString, missing_json)] //     QML: missingJson — [{mxid,name}] members of the room I opened from who are NOT in the container space (0.8.27)
        #[qproperty(bool, can_invite)] //          QML: canInvite — I may invite people into the container (0.8.27)
        #[qproperty(QString, unjoined_parent)] //  QML: unjoinedParent — the room's parent space I have not joined ("" = none) (0.8.27)
        #[qproperty(QString, source_label)] //     QML: sourceLabel — name of the room the calendar was opened from (0.8.27)
        type CalendarController = super::CalendarControllerRust;

        /// QML: start() — begin the reminder loop (once; safe to call repeatedly).
        #[qinvokable]
        fn start(self: Pin<&mut CalendarController>);
        /// QML: setRoom(roomId) — the open room changed: resolve its container + refresh.
        #[qinvokable]
        fn set_room(self: Pin<&mut CalendarController>, room_id: &QString);
        /// QML: openContainer(id) — show another space's calendar (the window's space picker).
        #[qinvokable]
        fn open_container(self: Pin<&mut CalendarController>, container_id: &QString);
        /// QML: refresh() — re-read everything from the local store.
        #[qinvokable]
        fn refresh(self: Pin<&mut CalendarController>);
        /// QML: saveEvent(json) — create (empty id) or update an event. Optional `announceRoom`
        /// + `announceWhen` post a chat summary on CREATE.
        #[qinvokable]
        fn save_event(self: Pin<&mut CalendarController>, json: &QString);
        /// QML: deleteEvent(id, when) — remove an event (empty content). `when` = the pre-formatted
        /// time line, so the posted chat card can be edited to say CANCELLED (0.8.33).
        #[qinvokable]
        fn delete_event(self: Pin<&mut CalendarController>, id: &QString, when: &QString);
        /// QML: rsvp(id, status, optionsJson) — my answer: yes|maybe|no|"" (clear) + option indexes.
        #[qinvokable]
        fn rsvp(self: Pin<&mut CalendarController>, id: &QString, status: &QString, options_json: &QString);
        /// QML: lockOption(id, index, when) — the organiser picks a proposed time (index < 0
        /// unlocks); `when` = the chosen time pre-formatted for the chat card (0.8.33).
        #[qinvokable]
        fn lock_option(self: Pin<&mut CalendarController>, id: &QString, index: i32, when: &QString);
        /// QML: openEvent(container, id) — 0.8.33: a chat card was clicked: show that container's
        /// calendar (switching if needed) and focus the event once its snapshot is in.
        #[qinvokable]
        fn open_event(self: Pin<&mut CalendarController>, container: &QString, id: &QString);
        /// QML: linkLegacyCards(json, hintRoom) — 0.8.35: events from before 0.8.33 have a chat card
        /// but no ref to it; for these [{id, when}] find MY earlier cards and connect them.
        #[qinvokable]
        fn link_legacy_cards(self: Pin<&mut CalendarController>, json: &QString, hint_room: &QString);
        /// QML: saveGame(json) — {name,color,rules,server}; slug derived from the name.
        #[qinvokable]
        fn save_game(self: Pin<&mut CalendarController>, json: &QString);
        /// QML: deleteGame(slug)
        #[qinvokable]
        fn delete_game(self: Pin<&mut CalendarController>, slug: &QString);
        /// QML: applyReminders(on) — persist the reminder toggle.
        #[qinvokable]
        fn apply_reminders(self: Pin<&mut CalendarController>, on: bool);
        /// QML: clearFocus() — the UI consumed focusEvent.
        #[qinvokable]
        fn clear_focus(self: Pin<&mut CalendarController>);
        /// QML: inviteMissing() — invite everyone in missingJson into the container space (0.8.27).
        #[qinvokable]
        fn invite_missing(self: Pin<&mut CalendarController>);
    }

    impl cxx_qt::Threading for CalendarController {}
}

type QtThread = cxx_qt::CxxQtThread<qobject::CalendarController>;

pub struct CalendarControllerRust {
    room_id: QString,
    container_id: QString,
    container_label: QString,
    events_json: QString,
    games_json: QString,
    upcoming_json: QString,
    containers_json: QString,
    can_edit: bool,
    is_mod: bool,
    my_id: QString,
    last_error: QString,
    busy: bool,
    reminders: bool,
    focus_event: QString,
    auto_open: bool,
    missing_json: QString,
    can_invite: bool,
    unjoined_parent: QString,
    source_label: QString,
    started: bool,
    generation: u64,
    /// 0.8.33: (container, event) to focus once that container's snapshot lands (openEvent).
    pending_focus: Option<(String, String)>,
}

impl Default for CalendarControllerRust {
    fn default() -> Self {
        Self {
            room_id: QString::default(),
            container_id: QString::default(),
            container_label: QString::default(),
            events_json: QString::from("[]"),
            games_json: QString::from("[]"),
            upcoming_json: QString::from("[]"),
            containers_json: QString::from("[]"),
            can_edit: false,
            is_mod: false,
            my_id: QString::default(),
            last_error: QString::default(),
            busy: false,
            reminders: reminders_enabled(),
            focus_event: QString::default(),
            auto_open: std::env::var("TINHAT_GUI_CALENDAR").map(|v| v == "1").unwrap_or(false),
            missing_json: QString::from("[]"),
            can_invite: false,
            unjoined_parent: QString::default(),
            source_label: QString::default(),
            started: false,
            generation: 0,
            pending_focus: None,
        }
    }
}

impl qobject::CalendarController {
    pub fn start(mut self: Pin<&mut Self>) {
        if self.rust().started {
            return;
        }
        self.as_mut().rust_mut().started = true;
        let on = reminders_enabled();
        self.as_mut().set_reminders(on);
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            reminder_loop(qt).await;
        });
    }

    pub fn set_room(mut self: Pin<&mut Self>, room_id: &QString) {
        let rid = room_id.to_string();
        if rid == self.room_id().to_string() {
            return;
        }
        self.as_mut().set_room_id(QString::from(&rid));
        if rid.is_empty() {
            return;
        }
        self.as_mut().set_unjoined_parent(QString::default());
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let Some(client) = crate::session::client() else { return };
            let Some(cid) = container_for_room(&client, &rid).await else { return };
            // 0.8.27: the room points at a space I am not in → I only see the room's own
            // calendar; tell the window so it can say why (a field report).
            let unjoined = if cid == rid { unjoined_parent_space(&client, &rid).await.unwrap_or_default() } else { String::new() };
            let _ = qt.queue(move |mut o| {
                if o.room_id().to_string() != rid {
                    return; // the user moved on; a later set_room owns the container now
                }
                o.as_mut().set_unjoined_parent(QString::from(&unjoined));
                o.as_mut().open_container(&QString::from(&cid));
            });
        });
    }

    pub fn open_container(mut self: Pin<&mut Self>, container_id: &QString) {
        let cid = container_id.to_string();
        if cid.is_empty() {
            return;
        }
        if cid != self.container_id().to_string() {
            self.as_mut().set_container_id(QString::from(&cid));
            // Don't flash another space's events while the new snapshot builds.
            self.as_mut().set_events_json(QString::from("[]"));
            self.as_mut().set_upcoming_json(QString::from("[]"));
        }
        self.refresh();
    }

    pub fn refresh(mut self: Pin<&mut Self>) {
        let cid = self.container_id().to_string();
        let source = self.room_id().to_string();
        if cid.is_empty() {
            return;
        }
        let gen = {
            let mut r = self.as_mut().rust_mut();
            r.generation += 1;
            r.generation
        };
        self.as_mut().set_busy(true);
        let qt = self.qt_thread();
        let cid_for_focus = cid.clone();
        crate::bridge::runtime().spawn(async move {
            let Some(client) = crate::session::client() else { return };
            let snap = build_snapshot(&client, &cid, &source).await;
            let _ = qt.queue(move |mut o| {
                if o.rust().generation != gen {
                    return; // superseded by a newer refresh / container switch
                }
                o.as_mut().set_container_label(QString::from(&snap.label));
                o.as_mut().set_events_json(QString::from(&snap.events_json));
                // 0.8.33: a chat card asked for an event in THIS container — focus it now.
                if let Some((c, id)) = o.as_mut().rust_mut().pending_focus.take() {
                    if c == cid_for_focus {
                        o.as_mut().set_focus_event(QString::from(&id));
                    } else {
                        o.as_mut().rust_mut().pending_focus = Some((c, id));
                    }
                }
                o.as_mut().set_games_json(QString::from(&snap.games_json));
                o.as_mut().set_upcoming_json(QString::from(&snap.upcoming_json));
                o.as_mut().set_containers_json(QString::from(&snap.containers_json));
                o.as_mut().set_can_edit(snap.can_edit);
                o.as_mut().set_is_mod(snap.is_mod);
                o.as_mut().set_my_id(QString::from(&snap.my_id));
                o.as_mut().set_missing_json(QString::from(&snap.missing_json));
                o.as_mut().set_can_invite(snap.can_invite);
                o.as_mut().set_source_label(QString::from(&snap.source_label));
                o.as_mut().set_busy(false);
            });
        });
    }

    pub fn save_event(mut self: Pin<&mut Self>, json: &QString) {
        let cid = self.container_id().to_string();
        let raw = json.to_string();
        if cid.is_empty() {
            return;
        }
        self.as_mut().set_last_error(QString::default());
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            match do_save_event(&cid, &raw).await {
                Ok(id) => {
                    let _ = qt.queue(move |mut o| {
                        o.as_mut().set_focus_event(QString::from(&id));
                        o.as_mut().refresh();
                    });
                }
                Err(msg) => {
                    eprintln!("[calendar] save failed: {msg}");
                    let _ = qt.queue(move |mut o| o.as_mut().set_last_error(QString::from(&msg)));
                }
            }
        });
    }

    /// 0.8.35: connect the cards of events from before 0.8.33 (legacy cards must connect
    /// correctly too). QML sends every event without a card ref + its
    /// pre-formatted `when`; only my own events can be connected (the card must be mine to edit):
    /// the card is edited into a live one and the state event remembers it. One search pass.
    pub fn link_legacy_cards(mut self: Pin<&mut Self>, json: &QString, hint_room: &QString) {
        let cid = self.container_id().to_string();
        let hint = hint_room.to_string();
        let wanted: Vec<(String, String)> = serde_json::from_str::<Vec<Value>>(&json.to_string())
            .unwrap_or_default()
            .iter()
            .filter_map(|e| Some((e["id"].as_str()?.to_string(), e["when"].as_str().unwrap_or("").to_string())))
            .collect();
        if cid.is_empty() || wanted.is_empty() {
            return;
        }
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let (Some(client), Some(room)) = (crate::session::client(), container_room(&cid)) else { return };
            let me = client.user_id().map(|u| u.to_string()).unwrap_or_default();
            let mut mine: Vec<(String, String, Value)> = Vec::new();
            for (id, when) in wanted {
                if let Some(c) = crate::appstate::get_state(&room, EVENT_TYPE, &id).await {
                    let has = c.get("announce").map(|a| a.is_object()).unwrap_or(false);
                    let by_me = c.get("createdBy").and_then(Value::as_str) == Some(me.as_str());
                    let titled = c.get("title").and_then(Value::as_str).map(|t| !t.is_empty()).unwrap_or(false);
                    if !has && by_me && titled {
                        mine.push((id, when, c));
                    }
                }
            }
            if mine.is_empty() {
                return;
            }
            let keys: Vec<(String, String)> = mine.iter().map(|(id, _, c)| (id.clone(), c["title"].as_str().unwrap_or("").to_string())).collect();
            let found = adopt_cards(&client, &cid, &keys, &[hint]).await;
            let mut linked = 0;
            for (id, when, mut c) in mine {
                let Some(a) = found.get(&id) else { continue };
                let new_ref = sync_announcement(&client, &cid, &id, a, &c, &when).await.unwrap_or_else(|| a.clone());
                c["announce"] = new_ref;
                if crate::appstate::send_state(&room, EVENT_TYPE, &id, c).await.is_ok() {
                    linked += 1;
                }
            }
            eprintln!("[calendar] legacy cards connected: {linked} of {} in {cid}", keys.len());
            if linked > 0 {
                let _ = qt.queue(|mut o| o.as_mut().refresh());
            }
        });
    }

    pub fn delete_event(mut self: Pin<&mut Self>, id: &QString, when: &QString) {
        let cid = self.container_id().to_string();
        let id = id.to_string();
        let when = when.to_string();
        if cid.is_empty() || id.is_empty() {
            return;
        }
        self.as_mut().set_last_error(QString::default());
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let res = async {
                let room = container_room(&cid).ok_or("Not signed in")?;
                // 0.8.33: the chat card (if one was posted) says CANCELLED before the event goes.
                if let (Some(client), Some(existing)) = (crate::session::client(), crate::appstate::get_state(&room, EVENT_TYPE, &id).await) {
                    // 0.8.35: no ref (an event from before 0.8.33) → adopt my earlier card first.
                    let aref = match existing.get("announce").filter(|a| a.is_object()) {
                        Some(a) => Some(a.clone()),
                        None => adopt_card(&client, &cid, &id, existing.get("title").and_then(Value::as_str).unwrap_or(""), &[]).await,
                    };
                    if let Some(a) = aref {
                        let mut c = existing.clone();
                        c["canceled"] = json!(true);
                        let _ = sync_announcement(&client, &cid, &id, &a, &c, &when).await;
                    }
                }
                crate::appstate::send_state(&room, EVENT_TYPE, &id, json!({}))
                    .await
                    .map_err(|e| friendly(&e.to_string()))
            }
            .await;
            match res {
                Ok(()) => {
                    let _ = qt.queue(move |mut o| o.as_mut().refresh());
                }
                Err(msg) => {
                    let _ = qt.queue(move |mut o| o.as_mut().set_last_error(QString::from(&msg)));
                }
            }
        });
    }

    pub fn rsvp(mut self: Pin<&mut Self>, id: &QString, status: &QString, options_json: &QString) {
        let cid = self.container_id().to_string();
        let id = id.to_string();
        let status = status.to_string();
        let options: Vec<i64> = serde_json::from_str(&options_json.to_string()).unwrap_or_default();
        if cid.is_empty() || id.is_empty() {
            return;
        }
        self.as_mut().set_last_error(QString::default());
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            match do_rsvp(&cid, &id, &status, &options).await {
                Ok(()) => {
                    let _ = qt.queue(move |mut o| o.as_mut().refresh());
                }
                Err(msg) => {
                    eprintln!("[calendar] rsvp failed: {msg}");
                    let _ = qt.queue(move |mut o| o.as_mut().set_last_error(QString::from(&msg)));
                }
            }
        });
    }

    pub fn lock_option(mut self: Pin<&mut Self>, id: &QString, index: i32, when: &QString) {
        let cid = self.container_id().to_string();
        let id = id.to_string();
        let when = when.to_string();
        if cid.is_empty() || id.is_empty() {
            return;
        }
        self.as_mut().set_last_error(QString::default());
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            match do_lock_option(&cid, &id, index).await {
                Ok(content) => {
                    // 0.8.33: the chat card shows the locked-in time (0.8.35: adopting an old event's card first).
                    if let Some(client) = crate::session::client() {
                        let aref = match content.get("announce").filter(|a| a.is_object()) {
                            Some(a) => Some(a.clone()),
                            None => adopt_card(&client, &cid, &id, content.get("title").and_then(Value::as_str).unwrap_or(""), &[]).await,
                        };
                        if let Some(a) = aref {
                            let new_ref = sync_announcement(&client, &cid, &id, &a, &content, &when).await.unwrap_or(a);
                            if content.get("announce") != Some(&new_ref) {
                                if let Some(room) = container_room(&cid) {
                                    let mut c = content.clone();
                                    c["announce"] = new_ref;
                                    let _ = crate::appstate::send_state(&room, EVENT_TYPE, &id, c).await;
                                }
                            }
                        }
                    }
                    let _ = qt.queue(move |mut o| o.as_mut().refresh());
                }
                Err(msg) => {
                    let _ = qt.queue(move |mut o| o.as_mut().set_last_error(QString::from(&msg)));
                }
            }
        });
    }

    pub fn save_game(mut self: Pin<&mut Self>, json: &QString) {
        let cid = self.container_id().to_string();
        let raw = json.to_string();
        if cid.is_empty() {
            return;
        }
        self.as_mut().set_last_error(QString::default());
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let res = async {
                let v: Value = serde_json::from_str(&raw).map_err(|_| "Bad game data".to_string())?;
                let name = v.get("name").and_then(Value::as_str).unwrap_or("").trim().to_string();
                if name.is_empty() {
                    return Err("Give the game a name".to_string());
                }
                let room = container_room(&cid).ok_or("Not signed in")?;
                crate::appstate::ensure_writable(&room, &[EVENT_TYPE, RSVP_TYPE, GAME_TYPE]).await;
                let slug = v
                    .get("slug")
                    .and_then(Value::as_str)
                    .filter(|s| !s.trim().is_empty())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| slugify(&name));
                let content = json!({
                    "name": name,
                    "color": v.get("color").and_then(Value::as_str).unwrap_or(""),
                    "rules": v.get("rules").and_then(Value::as_str).unwrap_or(""),
                    "server": v.get("server").and_then(Value::as_str).unwrap_or(""),
                    "updatedAt": crate::appstate::now_ms(),
                });
                crate::appstate::send_state(&room, GAME_TYPE, &slug, content)
                    .await
                    .map_err(|e| friendly(&e.to_string()))
            }
            .await;
            match res {
                Ok(()) => {
                    let _ = qt.queue(move |mut o| o.as_mut().refresh());
                }
                Err(msg) => {
                    let _ = qt.queue(move |mut o| o.as_mut().set_last_error(QString::from(&msg)));
                }
            }
        });
    }

    pub fn delete_game(mut self: Pin<&mut Self>, slug: &QString) {
        let cid = self.container_id().to_string();
        let slug = slug.to_string();
        if cid.is_empty() || slug.is_empty() {
            return;
        }
        self.as_mut().set_last_error(QString::default());
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let res = async {
                let room = container_room(&cid).ok_or("Not signed in")?;
                crate::appstate::send_state(&room, GAME_TYPE, &slug, json!({}))
                    .await
                    .map_err(|e| friendly(&e.to_string()))
            }
            .await;
            match res {
                Ok(()) => {
                    let _ = qt.queue(move |mut o| o.as_mut().refresh());
                }
                Err(msg) => {
                    let _ = qt.queue(move |mut o| o.as_mut().set_last_error(QString::from(&msg)));
                }
            }
        });
    }

    pub fn apply_reminders(mut self: Pin<&mut Self>, on: bool) {
        crate::settings::overlay_pref_write("calendar-reminders", if on { "1" } else { "0" });
        self.as_mut().set_reminders(on);
    }

    pub fn clear_focus(mut self: Pin<&mut Self>) {
        self.as_mut().set_focus_event(QString::default());
    }

    /// 0.8.33: a chat card was clicked. Same container → focus now (the window's poll opens
    /// it); another container → switch, and focus when its snapshot arrives (refresh()).
    pub fn open_event(mut self: Pin<&mut Self>, container: &QString, id: &QString) {
        let cid = container.to_string();
        let id = id.to_string();
        if id.is_empty() {
            return;
        }
        if cid.is_empty() || cid == self.container_id().to_string() {
            self.as_mut().set_focus_event(QString::from(&id));
            return;
        }
        self.as_mut().rust_mut().pending_focus = Some((cid.clone(), id));
        self.open_container(&QString::from(&cid));
    }

    /// Invite every member of the source room who is not yet in the container space
    /// (0.8.27). One space invite each; failures are counted, not fatal.
    pub fn invite_missing(mut self: Pin<&mut Self>) {
        let cid = self.container_id().to_string();
        let list: Vec<Value> = serde_json::from_str(&self.missing_json().to_string()).unwrap_or_default();
        if cid.is_empty() || list.is_empty() {
            return;
        }
        self.as_mut().set_last_error(QString::default());
        self.as_mut().set_busy(true);
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let mut ok = 0usize;
            let mut failed: Vec<String> = Vec::new();
            if let Some(room) = container_room(&cid) {
                for p in &list {
                    let Some(mxid) = p.get("mxid").and_then(Value::as_str) else { continue };
                    match UserId::parse(mxid) {
                        Ok(uid) => match room.invite_user_by_id(&uid).await {
                            Ok(()) => ok += 1,
                            Err(e) => {
                                eprintln!("[calendar] space invite {mxid} failed: {e}");
                                failed.push(mxid.to_string());
                            }
                        },
                        Err(_) => failed.push(mxid.to_string()),
                    }
                }
            }
            eprintln!("[calendar] invited {ok} into {cid}, {} failed", failed.len());
            let msg = if failed.is_empty() {
                String::new()
            } else {
                format!("Invited {ok}; couldn't invite {}", failed.join(", "))
            };
            let _ = qt.queue(move |mut o| {
                if !msg.is_empty() {
                    o.as_mut().set_last_error(QString::from(&msg));
                }
                o.as_mut().refresh();
            });
        });
    }
}

// ---------------------------------------------------------------------------------------------
// Data layer
// ---------------------------------------------------------------------------------------------

fn reminders_enabled() -> bool {
    crate::settings::overlay_pref_read("calendar-reminders")
        .map(|s| s != "0")
        .unwrap_or(true)
}

fn container_room(cid: &str) -> Option<Room> {
    let client = crate::session::client()?;
    let rid = RoomId::parse(cid).ok()?;
    client.get_room(&rid)
}

/// Turn an SDK error into something a person can act on.
fn friendly(e: &str) -> String {
    if e.contains("M_FORBIDDEN") || e.contains("403") {
        "You can't write to this calendar yet — ask a space admin to open the calendar once (that unlocks it for everyone)".to_string()
    } else if e.contains("M_LIMIT_EXCEEDED") || e.contains("429") {
        "Slow down a little — the server is rate-limiting".to_string()
    } else {
        format!("Couldn't save: {e}")
    }
}

pub(crate) fn slugify(name: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in name.trim().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "game".to_string()
    } else {
        out
    }
}

/// A deterministic pleasant colour for a game with none chosen (hue from the slug).
pub(crate) fn auto_color(slug: &str) -> String {
    const PALETTE: &[&str] = &[
        "#7cc4ff", "#ffb347", "#8ee08e", "#ff7c9c", "#c39bff", "#ffd75e", "#5ee0d0", "#ff9e6b",
    ];
    let h = slug.bytes().fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32));
    PALETTE[(h as usize) % PALETTE.len()].to_string()
}

/// The calendar container for a room: the room itself if it is a space; else its parent
/// space (`m.space.parent`, then a scan of every joined space's children); else the room.
pub(crate) async fn container_for_room(client: &Client, room_id: &str) -> Option<String> {
    let rid = RoomId::parse(room_id).ok()?;
    let room = client.get_room(&rid)?;
    if room.is_space() {
        return Some(room_id.to_string());
    }
    for (sk, _sender, content) in crate::appstate::get_state_all(&room, "m.space.parent").await {
        if content.get("via").is_none() {
            continue; // an empty content = the parent link was removed
        }
        if let Ok(pid) = RoomId::parse(&sk) {
            // Only a space I have JOINED can be the container — an invited/left space is in
            // the store too, but its calendar state is not readable.
            if client
                .get_room(&pid)
                .map(|r| r.is_space() && r.state() == matrix_sdk::RoomState::Joined)
                .unwrap_or(false)
            {
                return Some(sk);
            }
        }
    }
    for space in client.joined_rooms().into_iter().filter(|r| r.is_space()) {
        if crate::nav::space_child_ids(&space).await.contains(room_id) {
            return Some(space.room_id().to_string());
        }
    }
    Some(room_id.to_string())
}

/// The room's parent space that I am NOT joined to (first `m.space.parent` with a `via`),
/// so the UI can explain why the calendar shown is the room's own (0.8.27).
async fn unjoined_parent_space(client: &Client, room_id: &str) -> Option<String> {
    let rid = RoomId::parse(room_id).ok()?;
    let room = client.get_room(&rid)?;
    if room.is_space() {
        return None;
    }
    for (sk, _sender, content) in crate::appstate::get_state_all(&room, "m.space.parent").await {
        if content.get("via").is_none() {
            continue;
        }
        let Ok(pid) = RoomId::parse(&sk) else { continue };
        let joined = client
            .get_room(&pid)
            .map(|r| r.state() == matrix_sdk::RoomState::Joined)
            .unwrap_or(false);
        if !joined {
            return Some(sk);
        }
    }
    None
}

pub(crate) fn room_label(room: &Room) -> String {
    room.name()
        .or_else(|| room.canonical_alias().map(|a| a.to_string()))
        .unwrap_or_else(|| room.room_id().to_string())
}

/// The moment an event effectively starts: its `start`, or (an unlocked proposal) the
/// earliest proposed option. 0 when nothing is known.
pub(crate) fn effective_start(content: &Value) -> u64 {
    let start = content.get("start").and_then(Value::as_u64).unwrap_or(0);
    if start > 0 {
        return start;
    }
    content
        .get("options")
        .and_then(Value::as_array)
        .map(|opts| {
            opts.iter()
                .filter_map(|o| o.get("start").and_then(Value::as_u64))
                .filter(|s| *s > 0)
                .min()
                .unwrap_or(0)
        })
        .unwrap_or(0)
}

pub(crate) fn effective_end(content: &Value) -> u64 {
    let end = content.get("end").and_then(Value::as_u64).unwrap_or(0);
    if end > 0 {
        return end;
    }
    let start = effective_start(content);
    if start == 0 {
        0
    } else {
        start + DEFAULT_DURATION_MS
    }
}

pub(crate) fn is_canceled(content: &Value) -> bool {
    content.get("canceled").and_then(Value::as_bool).unwrap_or(false)
}

/// Relative phrasing for notifications ("in 25 min", "in 3 h", "in 2 days") — no TZ needed.
fn rel_when(start: u64, now: u64) -> String {
    if start <= now {
        return "now".to_string();
    }
    let mins = (start - now) / 60_000;
    if mins < 60 {
        format!("in {} min", mins.max(1))
    } else if mins < 36 * 60 {
        let h = mins / 60;
        let m = mins % 60;
        if m == 0 {
            format!("in {h} h")
        } else {
            format!("in {h} h {m} min")
        }
    } else {
        format!("in {} days", (mins + 12 * 60) / (24 * 60))
    }
}

pub(crate) struct Person {
    name: String,
    avatar: String,
}

/// Member name + avatar (cache-first; a missing avatar downloads once in the background).
pub(crate) async fn resolve_person(
    client: &Client,
    room: &Room,
    mxid: &str,
    cache: &mut HashMap<String, Person>,
) -> Value {
    if !cache.contains_key(mxid) {
        let mut name = mxid.trim_start_matches('@').split(':').next().unwrap_or(mxid).to_string();
        let mut avatar = String::new();
        if let Ok(parsed) = UserId::parse(mxid) {
            if let Ok(Some(member)) = room.get_member_no_sync(&parsed).await {
                if let Some(dn) = member.display_name() {
                    name = dn.to_string();
                }
                if let Some(mxc) = member.avatar_url() {
                    avatar = crate::bridge::resolve_avatar(client, mxc).await.unwrap_or_default();
                }
            }
        }
        cache.insert(mxid.to_string(), Person { name, avatar });
    }
    let p = &cache[mxid];
    json!({ "mxid": mxid, "name": p.name, "avatar": p.avatar })
}

struct Snapshot {
    label: String,
    missing_json: String,
    can_invite: bool,
    source_label: String,
    events_json: String,
    games_json: String,
    upcoming_json: String,
    containers_json: String,
    can_edit: bool,
    is_mod: bool,
    my_id: String,
}

async fn build_snapshot(client: &Client, cid: &str, source_room: &str) -> Snapshot {
    let me = client.user_id().map(|u| u.to_string()).unwrap_or_default();
    let mut snap = Snapshot {
        label: String::new(),
        missing_json: "[]".into(),
        can_invite: false,
        source_label: String::new(),
        events_json: "[]".into(),
        games_json: "[]".into(),
        upcoming_json: "[]".into(),
        containers_json: "[]".into(),
        can_edit: false,
        is_mod: false,
        my_id: me.clone(),
    };
    let Ok(rid) = RoomId::parse(cid) else { return snap };
    let Some(room) = client.get_room(&rid) else { return snap };
    snap.label = room_label(&room);

    // Admin-only, idempotent: unlock the three calendar types for PL-0 members. A member
    // gets an early return inside (no 403 noise).
    crate::appstate::ensure_writable(&room, &[EVENT_TYPE, RSVP_TYPE, GAME_TYPE]).await;
    if let Ok(pl) = room.power_levels().await {
        use matrix_sdk::ruma::events::room::power_levels::UserPowerLevel;
        if let Ok(uid) = UserId::parse(&me) {
            snap.can_invite = pl.user_can_invite(&uid);
            snap.can_edit = pl.user_can_send_state(&uid, StateEventType::from(EVENT_TYPE));
            snap.is_mod = match pl.for_user(&uid) {
                UserPowerLevel::Infinite => true,
                UserPowerLevel::Int(i) => i64::from(i) >= 50,
                _ => false,
            };
        }
    }

    // ---- 0.8.27: people in the room I opened from who are not in this space (they see an
    // empty room calendar instead of this one). Admins get a one-click "invite them".
    if !source_room.is_empty() && source_room != cid {
        if let Ok(srid) = RoomId::parse(source_room) {
            if let Some(src) = client.get_room(&srid) {
                snap.source_label = room_label(&src);
                let in_space: std::collections::HashSet<String> = match room.members_no_sync(matrix_sdk::RoomMemberships::JOIN | matrix_sdk::RoomMemberships::INVITE).await {
                    Ok(list) => list.into_iter().map(|m| m.user_id().to_string()).collect(),
                    Err(_) => std::collections::HashSet::new(),
                };
                let mut missing: Vec<Value> = Vec::new();
                if let Ok(list) = src.members_no_sync(matrix_sdk::RoomMemberships::JOIN).await {
                    for m in list {
                        let id = m.user_id().to_string();
                        if in_space.contains(&id) || id == me {
                            continue;
                        }
                        let name = m.display_name().map(|s| s.to_string()).unwrap_or_else(|| id.trim_start_matches('@').split(':').next().unwrap_or(&id).to_string());
                        missing.push(json!({ "mxid": id, "name": name }));
                    }
                }
                missing.sort_by(|a, b| a["name"].as_str().unwrap_or("").to_lowercase().cmp(&b["name"].as_str().unwrap_or("").to_lowercase()));
                snap.missing_json = serde_json::to_string(&missing).unwrap_or_else(|_| "[]".into());
            }
        }
    }

    // ---- games ----
    let mut games: Vec<Value> = Vec::new();
    let mut game_colors: HashMap<String, String> = HashMap::new();
    for (slug, _sender, content) in crate::appstate::get_state_all(&room, GAME_TYPE).await {
        let name = content.get("name").and_then(Value::as_str).unwrap_or("").trim().to_string();
        if name.is_empty() {
            continue; // deleted
        }
        let mut color = content.get("color").and_then(Value::as_str).unwrap_or("").to_string();
        if color.is_empty() {
            color = auto_color(&slug);
        }
        game_colors.insert(slug.clone(), color.clone());
        game_colors.insert(name.to_lowercase(), color.clone());
        games.push(json!({
            "slug": slug,
            "name": name,
            "color": color,
            "rules": content.get("rules").and_then(Value::as_str).unwrap_or(""),
            "server": content.get("server").and_then(Value::as_str).unwrap_or(""),
        }));
    }
    games.sort_by(|a, b| {
        a["name"].as_str().unwrap_or("").to_lowercase().cmp(&b["name"].as_str().unwrap_or("").to_lowercase())
    });
    snap.games_json = serde_json::to_string(&games).unwrap_or_else(|_| "[]".into());

    // ---- RSVPs, aggregated per event ----
    // event id -> (status -> [mxid]), event id -> (mxid -> [option idx])
    let mut answers: HashMap<String, Vec<(String, String, Vec<i64>)>> = HashMap::new();
    for (user, _sender, content) in crate::appstate::get_state_all(&room, RSVP_TYPE).await {
        let Some(map) = content.as_object() else { continue };
        for (eid, entry) in map {
            let status = entry.get("status").and_then(Value::as_str).unwrap_or("").to_string();
            let opts: Vec<i64> = entry
                .get("options")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_i64).collect())
                .unwrap_or_default();
            if status.is_empty() && opts.is_empty() {
                continue;
            }
            answers.entry(eid.clone()).or_default().push((user.clone(), status, opts));
        }
    }

    // ---- events ----
    let now = crate::appstate::now_ms();
    let mut people: HashMap<String, Person> = HashMap::new();
    let mut events: Vec<(u64, Value)> = Vec::new();
    for (id, _sender, content) in crate::appstate::get_state_all(&room, EVENT_TYPE).await {
        let title = content.get("title").and_then(Value::as_str).unwrap_or("").trim().to_string();
        if title.is_empty() {
            continue; // deleted
        }
        let game = content.get("game").and_then(Value::as_str).unwrap_or("").trim().to_string();
        let game_color = if game.is_empty() {
            String::new()
        } else {
            game_colors
                .get(&slugify(&game))
                .or_else(|| game_colors.get(&game.to_lowercase()))
                .cloned()
                .unwrap_or_else(|| auto_color(&slugify(&game)))
        };
        let created_by = content.get("createdBy").and_then(Value::as_str).unwrap_or("").to_string();
        let creator = resolve_person(client, &room, &created_by, &mut people).await;
        let start = effective_start(&content);
        let end = effective_end(&content);
        let mut yes = Vec::new();
        let mut maybe = Vec::new();
        let mut no = Vec::new();
        let mut my_status = String::new();
        let mut my_options: Vec<i64> = Vec::new();
        let n_opts = content.get("options").and_then(Value::as_array).map(|a| a.len()).unwrap_or(0);
        let mut votes: Vec<Vec<Value>> = vec![Vec::new(); n_opts];
        if let Some(list) = answers.get(&id) {
            for (user, status, opts) in list {
                let person = resolve_person(client, &room, user, &mut people).await;
                match status.as_str() {
                    "yes" => yes.push(person.clone()),
                    "maybe" => maybe.push(person.clone()),
                    "no" => no.push(person.clone()),
                    _ => {}
                }
                for idx in opts {
                    if *idx >= 0 && (*idx as usize) < n_opts {
                        votes[*idx as usize].push(person.clone());
                    }
                }
                if user == &me {
                    my_status = status.clone();
                    my_options = opts.clone();
                }
            }
        }
        let options: Vec<Value> = content
            .get("options")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .enumerate()
                    .map(|(i, o)| {
                        json!({
                            "start": o.get("start").and_then(Value::as_u64).unwrap_or(0),
                            "end": o.get("end").and_then(Value::as_u64).unwrap_or(0),
                            "votes": votes.get(i).cloned().unwrap_or_default(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let chosen = content.get("chosen").and_then(Value::as_i64).unwrap_or(-1);
        let canceled = is_canceled(&content);
        let v = json!({
            "id": id,
            "title": title,
            "game": game,
            // 11.1: the Reel's catalogue id ("steam:892970") — the Game servers link (docs/GAMESERVERS.md §5)
            "gameId": content.get("gameId").and_then(Value::as_str).unwrap_or(""),
            "gameColor": game_color,
            "kind": content.get("kind").and_then(Value::as_str).unwrap_or("session"),
            "start": content.get("start").and_then(Value::as_u64).unwrap_or(0),
            "end": content.get("end").and_then(Value::as_u64).unwrap_or(0),
            "effStart": start,
            "effEnd": end,
            "allDay": content.get("allDay").and_then(Value::as_bool).unwrap_or(false),
            "notes": content.get("notes").and_then(Value::as_str).unwrap_or(""),
            "rules": content.get("rules").and_then(Value::as_str).unwrap_or(""),
            "freePlay": content.get("freePlay").and_then(Value::as_bool).unwrap_or(false),
            "joinIfYouCan": content.get("joinIfYouCan").and_then(Value::as_bool).unwrap_or(false),
            "options": options,
            "chosen": chosen,
            "createdBy": created_by,
            "creator": creator,
            "createdAt": content.get("createdAt").and_then(Value::as_u64).unwrap_or(0),
            "updatedAt": content.get("updatedAt").and_then(Value::as_u64).unwrap_or(0),
            "canceled": canceled,
            "hasCard": content.get("announce").map(|a| a.is_object()).unwrap_or(false),
            "past": end > 0 && end < now,
            "canEdit": snap.is_mod || (!me.is_empty() && created_by == me),
            "rsvp": { "yes": yes, "maybe": maybe, "no": no },
            "my": { "status": my_status, "options": my_options },
        });
        events.push((start, v));
    }
    events.sort_by_key(|(s, v)| (if *s == 0 { u64::MAX } else { *s }, v["title"].as_str().unwrap_or("").to_string()));
    let list: Vec<Value> = events.iter().map(|(_, v)| v.clone()).collect();
    snap.events_json = serde_json::to_string(&list).unwrap_or_else(|_| "[]".into());

    // ---- upcoming (this container): live or ahead within the window ----
    let upcoming: Vec<Value> = list
        .iter()
        .filter(|v| !v["canceled"].as_bool().unwrap_or(false))
        .filter(|v| v["kind"].as_str() != Some("focus")) // main-game ranges are backdrop, not appointments
        .filter(|v| {
            let s = v["effStart"].as_u64().unwrap_or(0);
            let e = v["effEnd"].as_u64().unwrap_or(0);
            s > 0 && e >= now && s <= now + UPCOMING_WINDOW_MS
        })
        .take(5)
        .cloned()
        .collect();
    snap.upcoming_json = serde_json::to_string(&upcoming).unwrap_or_else(|_| "[]".into());

    // ---- containers: every joined space, plus this one if it is a plain room ----
    let mut containers: Vec<Value> = Vec::new();
    let mut seen_current = false;
    for r in client.joined_rooms() {
        if !r.is_space() {
            continue;
        }
        let id = r.room_id().to_string();
        if id == cid {
            seen_current = true;
        }
        let avatar = match r.avatar_url() {
            Some(mxc) => crate::bridge::resolve_avatar(client, &mxc).await.unwrap_or_default(),
            None => String::new(),
        };
        containers.push(json!({ "id": id, "label": room_label(&r), "avatar": avatar, "space": true }));
    }
    if !seen_current {
        let avatar = match room.avatar_url() {
            Some(mxc) => crate::bridge::resolve_avatar(client, &mxc).await.unwrap_or_default(),
            None => String::new(),
        };
        containers.insert(0, json!({ "id": cid, "label": snap.label, "avatar": avatar, "space": false }));
    }
    snap.containers_json = serde_json::to_string(&containers).unwrap_or_else(|_| "[]".into());
    snap
}

/// Create or update an event from the editor's JSON. Returns the event id.
async fn do_save_event(cid: &str, raw: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(raw).map_err(|_| "Bad event data".to_string())?;
    let title = v.get("title").and_then(Value::as_str).unwrap_or("").trim().to_string();
    if title.is_empty() {
        return Err("Give it a title".to_string());
    }
    let kind = v.get("kind").and_then(Value::as_str).unwrap_or("session").to_string();
    let start = v.get("start").and_then(Value::as_u64).unwrap_or(0);
    let end = v.get("end").and_then(Value::as_u64).unwrap_or(0);
    let options: Vec<Value> = v
        .get("options")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|o| {
                    let s = o.get("start").and_then(Value::as_u64).unwrap_or(0);
                    if s == 0 {
                        return None;
                    }
                    Some(json!({ "start": s, "end": o.get("end").and_then(Value::as_u64).unwrap_or(0) }))
                })
                .collect()
        })
        .unwrap_or_default();
    if kind == "proposal" {
        if options.is_empty() && start == 0 {
            return Err("Add at least one proposed time".to_string());
        }
    } else if kind == "focus" {
        // 0.8.27: a "main game" range — whole days, colours the grid; needs both ends.
        if start == 0 || end < start {
            return Err("Pick the first and the last day".to_string());
        }
    } else if start == 0 {
        return Err("Pick a date and time".to_string());
    }
    let client = crate::session::client().ok_or("Not signed in")?;
    let me = client.user_id().map(|u| u.to_string()).unwrap_or_default();
    let room = container_room(cid).ok_or("Not signed in")?;
    crate::appstate::ensure_writable(&room, &[EVENT_TYPE, RSVP_TYPE, GAME_TYPE]).await;
    let now = crate::appstate::now_ms();
    let given_id = v.get("id").and_then(Value::as_str).unwrap_or("").trim().to_string();
    let is_new = given_id.is_empty();
    let id = if is_new { format!("ev-{}-{:04x}", now, rand::random::<u16>()) } else { given_id };
    let existing = if is_new { None } else { crate::appstate::get_state(&room, EVENT_TYPE, &id).await };
    let created_by = existing
        .as_ref()
        .and_then(|e| e.get("createdBy").and_then(Value::as_str))
        .map(|s| s.to_string())
        .unwrap_or_else(|| me.clone());
    let created_at = existing
        .as_ref()
        .and_then(|e| e.get("createdAt").and_then(Value::as_u64))
        .unwrap_or(now);
    let chosen = v
        .get("chosen")
        .and_then(Value::as_i64)
        .or_else(|| existing.as_ref().and_then(|e| e.get("chosen").and_then(Value::as_i64)))
        .unwrap_or(-1);
    let game = v.get("game").and_then(Value::as_str).unwrap_or("").trim().to_string();
    // 0.8.33: which chat message is this event's card ({room, event, by}) rides along.
    let announce_ref = existing
        .as_ref()
        .and_then(|e| e.get("announce"))
        .filter(|a| a.is_object())
        .cloned();
    let mut content = json!({
        "title": title,
        "game": game,
        "kind": kind,
        "start": start,
        "end": end,
        "allDay": kind == "focus" || v.get("allDay").and_then(Value::as_bool).unwrap_or(false),
        "notes": v.get("notes").and_then(Value::as_str).unwrap_or("").trim(),
        "rules": v.get("rules").and_then(Value::as_str).unwrap_or("").trim(),
        "freePlay": v.get("freePlay").and_then(Value::as_bool).unwrap_or(false),
        "joinIfYouCan": v.get("joinIfYouCan").and_then(Value::as_bool).unwrap_or(false),
        "options": options,
        "chosen": chosen,
        "createdBy": created_by,
        "createdAt": created_at,
        "updatedAt": now,
        // 10.1 (TH-CAL-05): a save that does not mention `canceled` keeps the existing value
        // (like `chosen`); the editor never sends it, the "Call it off" action does.
        "canceled": v
            .get("canceled")
            .and_then(Value::as_bool)
            .or_else(|| existing.as_ref().and_then(|e| e.get("canceled").and_then(Value::as_bool)))
            .unwrap_or(false),
    });
    if let Some(a) = &announce_ref {
        content["announce"] = a.clone();
    }
    crate::appstate::send_state(&room, EVENT_TYPE, &id, content.clone())
        .await
        .map_err(|e| friendly(&e.to_string()))?;
    eprintln!("[calendar] saved {id} '{title}' kind={kind} start={start} new={is_new}");

    // A game mentioned for the first time joins the "Games & rules" list automatically.
    if !game.is_empty() {
        let slug = slugify(&game);
        let known = crate::appstate::get_state(&room, GAME_TYPE, &slug)
            .await
            .map(|g| g.get("name").and_then(Value::as_str).map(|n| !n.is_empty()).unwrap_or(false))
            .unwrap_or(false);
        if !known {
            let _ = crate::appstate::send_state(
                &room,
                GAME_TYPE,
                &slug,
                json!({ "name": game, "color": auto_color(&slug), "rules": "", "server": "", "updatedAt": now }),
            )
            .await;
        }
    }

    // The organiser is going, unless they said otherwise (a main-game range has no RSVP).
    if is_new && kind != "focus" {
        let _ = do_rsvp(cid, &id, "yes", &[]).await;
    }

    // Chat announcement. The editor pre-formats the local time (`announceWhen`) so everyone
    // reads the organiser's wall clock + zone; Rust never formats time. On CREATE (with a room
    // chosen) the card is posted and the state event remembers it; on every later save the
    // card is edited to match — or re-posted as a reply when it is someone else's (0.8.33).
    // 0.8.35: an event with NO ref (everything from before 0.8.33) first looks for my earlier
    // card and adopts it; the editor's "Post a fresh card in …" posts a new one instead.
    let when = v.get("announceWhen").and_then(Value::as_str).unwrap_or("").trim().to_string();
    let want = v.get("announceRoom").and_then(Value::as_str).unwrap_or("").trim().to_string();
    let hint = v.get("announceHint").and_then(Value::as_str).unwrap_or("").trim().to_string();
    if is_new {
        if !want.is_empty() {
            post_card(&client, &room, cid, &id, &want, &mut content, &when).await;
        }
    } else if !want.is_empty() {
        post_card(&client, &room, cid, &id, &want, &mut content, &when).await;
    } else {
        let aref = match &announce_ref {
            Some(a) => Some(a.clone()),
            None => {
                let old_title = existing.as_ref().and_then(|e| e.get("title")).and_then(Value::as_str).unwrap_or(&title).to_string();
                adopt_card(&client, cid, &id, &old_title, &[hint]).await
            }
        };
        if let Some(a) = aref {
            let new_ref = sync_announcement(&client, cid, &id, &a, &content, &when).await.unwrap_or(a);
            if announce_ref.as_ref() != Some(&new_ref) {
                content["announce"] = new_ref;
                let _ = crate::appstate::send_state(&room, EVENT_TYPE, &id, content.clone()).await;
            }
        }
    }
    Ok(id)
}

/// Post the chat card for an event into `announce` and record `{room, event, by}` on the state event.
async fn post_card(client: &Client, room: &Room, cid: &str, id: &str, announce: &str, content: &mut Value, when: &str) {
    let me = client.user_id().map(|u| u.to_string()).unwrap_or_default();
    let Ok(arid) = RoomId::parse(announce) else { return };
    let Some(target) = client.get_room(&arid) else { return };
    let text = announce_text(content, when);
    match target.send_raw("m.room.message", announce_content(&text, cid, id)).await {
        Ok(r) => {
            content["announce"] = json!({ "room": announce, "event": r.response.event_id.to_string(), "by": me });
            if let Err(e) = crate::appstate::send_state(room, EVENT_TYPE, id, content.clone()).await {
                eprintln!("[calendar] could not record the announcement on {id}: {e}");
            }
        }
        Err(e) => eprintln!("[calendar] announce failed: {e}"),
    }
}

/// 0.8.35: find MY earlier card for an event whose state has no `announce` ref — every event
/// from before 0.8.33, or one whose card was never recorded (2026-09-10: a recent change never
/// updated the calendar message). See `adopt_cards`.
async fn adopt_card(client: &Client, cid: &str, id: &str, old_title: &str, hints: &[String]) -> Option<Value> {
    adopt_cards(client, cid, &[(id.to_string(), old_title.to_string())], hints).await.remove(id)
}

/// The search behind `adopt_card` / `linkLegacyCards`, one fetch pass for many events: hinted
/// rooms first (the room the calendar was opened from), then every joined room of this
/// container, newest messages first — a message by me whose `dev.tinhat.calendar` marker names
/// the event, or (pre-0.8.33 cards carry no marker) whose body opens with the card's title line
/// ("📅 **Title**"). An edit of the card is followed back to the original message. The rooms
/// are encrypted, so this reads the client's decrypted timeline; the server holds ciphertext.
async fn adopt_cards(client: &Client, cid: &str, wanted: &[(String, String)], hints: &[String]) -> HashMap<String, Value> {
    let mut found: HashMap<String, Value> = HashMap::new();
    let me = client.user_id().map(|u| u.to_string()).unwrap_or_default();
    if me.is_empty() || wanted.is_empty() {
        return found;
    }
    let mut rooms: Vec<String> = Vec::new();
    for h in hints {
        if !h.is_empty() && !rooms.contains(h) {
            rooms.push(h.clone());
        }
    }
    for r in client.joined_rooms() {
        let rid = r.room_id().to_string();
        if rooms.len() >= 12 || rooms.contains(&rid) {
            continue;
        }
        if container_for_room(client, &rid).await.as_deref() == Some(cid) {
            rooms.push(rid);
        }
    }
    'rooms: for rid in rooms {
        let Ok(r) = RoomId::parse(&rid) else { continue };
        let Some(room) = client.get_room(&r) else { continue };
        for ev in recent_events(&room, 150).await {
            if found.len() == wanted.len() {
                break 'rooms;
            }
            if ev["sender"] != me.as_str() || ev["type"] != "m.room.message" {
                continue;
            }
            let c = &ev["content"];
            let is_edit = c["m.relates_to"]["rel_type"] == "m.replace";
            let src = if is_edit { &c["m.new_content"] } else { c };
            let body = src["body"].as_str().unwrap_or("");
            let body = body.strip_prefix("* ").unwrap_or(body);
            for (id, title) in wanted {
                if title.is_empty() || found.contains_key(id) {
                    continue;
                }
                let marker = src["dev.tinhat.calendar"]["event"] == id.as_str();
                let head_live = format!("📅 **{title}**");
                let head_cancelled = format!("📅 ~~{title}~~");
                if !(marker || body.starts_with(&head_live) || body.starts_with(&head_cancelled)) {
                    continue;
                }
                let target = if is_edit { c["m.relates_to"]["event_id"].as_str().unwrap_or("") } else { ev["event_id"].as_str().unwrap_or("") };
                if target.is_empty() {
                    continue;
                }
                eprintln!("[calendar] adopted my earlier card {target} in {rid} for {id}");
                found.insert(id.clone(), json!({ "room": rid, "event": target, "by": me }));
                break;
            }
        }
    }
    for (id, _) in wanted {
        if !found.contains_key(id) {
            eprintln!("[calendar] no earlier card of mine found for {id} — 'Post a fresh card' in the editor posts one");
        }
    }
    found
}

/// 0.8.33: the chat card's markdown, rebuilt from the event's content on every change so the
/// posted message can be EDITED to match (before, a changed event left the
/// card stale). `when` is pre-formatted by QML in the organiser's wall clock + zone.
fn announce_text(content: &Value, when: &str) -> String {
    let title = content.get("title").and_then(Value::as_str).unwrap_or("");
    let game = content.get("game").and_then(Value::as_str).unwrap_or("");
    let kind = content.get("kind").and_then(Value::as_str).unwrap_or("session");
    let canceled = is_canceled(content);
    let mut text = if canceled { format!("📅 ~~{title}~~ — **CANCELLED**") } else { format!("📅 **{title}**") };
    if !game.is_empty() {
        text.push_str(&format!(" — {game}"));
    }
    let locked = content.get("chosen").and_then(Value::as_i64).unwrap_or(-1) >= 0;
    let kind_label = match kind {
        "freeplay" => "Free play",
        "milestone" => "Scheduled progression — join if you can",
        "proposal" if locked => "Time locked in",
        "proposal" => "Proposal — vote for a time",
        "cooloff" => "Cool-off game",
        "focus" => "Main game",
        _ => "Group session",
    };
    text.push_str(&format!("\n{kind_label}"));
    if !when.is_empty() {
        text.push_str(&format!(" · {when}"));
    }
    if content["freePlay"].as_bool().unwrap_or(false) && kind != "freeplay" {
        text.push_str("\nFree play is allowed outside the session.");
    }
    let notes = content["notes"].as_str().unwrap_or("");
    if !notes.is_empty() {
        text.push_str("\n\n");
        text.push_str(notes);
    }
    if !canceled {
        text.push_str("\n\n_RSVP on the calendar (📅 in the room header)._");
    }
    text
}

/// The card's message content: the markdown + the `dev.tinhat.calendar` marker Tin Hat
/// turns into a live card (ChatBubble → CalendarCard). Stock clients render the markdown and
/// ignore the extra key; in an encrypted room the SDK encrypts the whole content.
fn announce_content(text: &str, cid: &str, id: &str) -> Value {
    let mut c = serde_json::to_value(RoomMessageEventContent::text_markdown(text))
        .unwrap_or_else(|_| json!({ "msgtype": "m.text", "body": text }));
    c["dev.tinhat.calendar"] = json!({ "container": cid, "event": id });
    c
}

/// Bring the posted card in line with `content`: EDIT it (m.replace) when I posted it, or —
/// Matrix lets only the sender edit — reply to it with a fresh card. Returns the new
/// `announce` ref to store when a fresh card was posted.
async fn sync_announcement(client: &Client, cid: &str, id: &str, announce: &Value, content: &Value, when: &str) -> Option<Value> {
    let room_id = announce.get("room").and_then(Value::as_str).unwrap_or("");
    let msg_id = announce.get("event").and_then(Value::as_str).unwrap_or("");
    let by = announce.get("by").and_then(Value::as_str).unwrap_or("");
    let me = client.user_id().map(|u| u.to_string()).unwrap_or_default();
    let Ok(rid) = RoomId::parse(room_id) else { return None };
    let Some(target) = client.get_room(&rid) else { return None };
    if msg_id.is_empty() {
        return None;
    }
    let text = announce_text(content, when);
    let new = announce_content(&text, cid, id);
    if by == me {
        let mut c = new.clone();
        c["body"] = json!(format!("* {text}")); // fallback for clients without edit support
        c["m.new_content"] = new;
        c["m.relates_to"] = json!({ "rel_type": "m.replace", "event_id": msg_id });
        match target.send_raw("m.room.message", c).await {
            Ok(_) => eprintln!("[calendar] chat card {msg_id} edited to match {id}"),
            Err(e) => eprintln!("[calendar] chat card edit failed: {e}"),
        }
        None
    } else {
        let mut c = new;
        c["m.relates_to"] = json!({ "m.in_reply_to": { "event_id": msg_id } });
        match target.send_raw("m.room.message", c).await {
            Ok(r) => {
                eprintln!("[calendar] chat card re-posted for {id} (the original is {by}'s)");
                Some(json!({ "room": room_id, "event": r.response.event_id.to_string(), "by": me }))
            }
            Err(e) => {
                eprintln!("[calendar] chat card re-post failed: {e}");
                None
            }
        }
    }
}

/// Write my RSVP map for the container (status "" + no options = clear my answer).
pub(crate) async fn do_rsvp(cid: &str, id: &str, status: &str, options: &[i64]) -> Result<(), String> {
    let client = crate::session::client().ok_or("Not signed in")?;
    let me = client.user_id().map(|u| u.to_string()).ok_or("Not signed in")?;
    let room = container_room(cid).ok_or("Not signed in")?;
    crate::appstate::ensure_writable(&room, &[EVENT_TYPE, RSVP_TYPE, GAME_TYPE]).await;
    let mut map = crate::appstate::get_state(&room, RSVP_TYPE, &me)
        .await
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    // Read-modify-write against the LOCAL store: a write made a moment ago is not synced back
    // yet, so two quick answers (create two events → two auto-RSVPs) used to overwrite each
    // other. Merge in what this process last wrote for the container, newest `at` wins.
    if let Ok(cache) = rsvp_cache().lock() {
        if let Some(last) = cache.get(cid).and_then(Value::as_object) {
            if let Some(obj) = map.as_object_mut() {
                for (eid, entry) in last {
                    let newer = obj
                        .get(eid)
                        .and_then(|e| e.get("at"))
                        .and_then(Value::as_u64)
                        .map(|at| entry.get("at").and_then(Value::as_u64).unwrap_or(0) > at)
                        .unwrap_or(true);
                    if newer {
                        obj.insert(eid.clone(), entry.clone());
                    }
                }
            }
        }
    }
    let now = crate::appstate::now_ms();
    // Prune answers for events that are gone or long over.
    let live: HashMap<String, Value> = crate::appstate::get_state_all(&room, EVENT_TYPE)
        .await
        .into_iter()
        .filter(|(_, _, c)| c.get("title").and_then(Value::as_str).map(|t| !t.is_empty()).unwrap_or(false))
        .map(|(k, _, c)| (k, c))
        .collect();
    if let Some(obj) = map.as_object_mut() {
        obj.retain(|eid, entry| match live.get(eid) {
            Some(c) => {
                let e = effective_end(c);
                e == 0 || e + RSVP_PRUNE_MS > now
            }
            // 0.8.35: an id the LOCAL store does not know yet is usually an event created a moment
            // ago (not synced back) — dropping its answer lost the auto-RSVP of the first of two
            // quick creates. Keep recent answers; a deleted event's answer ages out instead.
            None => entry.get("at").and_then(Value::as_u64).map(|at| at + RSVP_PRUNE_MS > now).unwrap_or(false),
        });
        if status.is_empty() && options.is_empty() {
            obj.remove(id);
        } else {
            obj.insert(id.to_string(), json!({ "status": status, "options": options, "at": now }));
        }
    }
    let res = crate::appstate::send_state(&room, RSVP_TYPE, &me, map.clone())
        .await
        .map_err(|e| friendly(&e.to_string()));
    if res.is_ok() {
        if let Ok(mut cache) = rsvp_cache().lock() {
            cache.insert(cid.to_string(), map);
        }
    }
    res
}

/// My last-written RSVP map per container (see `do_rsvp`).
fn rsvp_cache() -> &'static std::sync::Mutex<HashMap<String, Value>> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<HashMap<String, Value>>> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

async fn do_lock_option(cid: &str, id: &str, index: i32) -> Result<Value, String> {
    let room = container_room(cid).ok_or("Not signed in")?;
    let mut content = crate::appstate::get_state(&room, EVENT_TYPE, id).await.ok_or("Event vanished")?;
    let opts = content.get("options").and_then(Value::as_array).cloned().unwrap_or_default();
    if index >= 0 {
        let o = opts.get(index as usize).ok_or("No such option")?;
        content["start"] = Value::from(o.get("start").and_then(Value::as_u64).unwrap_or(0));
        content["end"] = Value::from(o.get("end").and_then(Value::as_u64).unwrap_or(0));
        content["chosen"] = Value::from(index as i64);
    } else {
        content["start"] = Value::from(0u64);
        content["end"] = Value::from(0u64);
        content["chosen"] = Value::from(-1i64);
    }
    content["updatedAt"] = Value::from(crate::appstate::now_ms());
    crate::appstate::send_state(&room, EVENT_TYPE, id, content.clone())
        .await
        .map_err(|e| friendly(&e.to_string()))?;
    Ok(content)
}

// ---------------------------------------------------------------------------------------------
// Reminders
// ---------------------------------------------------------------------------------------------

fn seen_path() -> std::path::PathBuf {
    crate::bridge::data_dir().join("calendar-seen.json")
}

fn load_seen() -> HashSet<String> {
    std::fs::read_to_string(seen_path())
        .ok()
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .map(|v| v.into_iter().collect())
        .unwrap_or_default()
}

fn save_seen(seen: &HashSet<String>) {
    let mut v: Vec<&String> = seen.iter().collect();
    v.sort();
    // Bounded: keys embed the event's epoch-ms id, so the oldest sort first.
    if v.len() > 600 {
        let cut = v.len() - 600;
        v.drain(..cut);
    }
    if let Ok(json) = serde_json::to_string(&v) {
        let _ = std::fs::write(seen_path(), json);
    }
}

async fn reminder_loop(qt: QtThread) {
    let mut seen = load_seen();
    let mut first = true;
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(if first { 25 } else { 60 })).await;
        first = false;
        let Some(client) = crate::session::client() else { continue };
        let me = client.user_id().map(|u| u.to_string()).unwrap_or_default();
        if me.is_empty() {
            continue;
        }
        let now = crate::appstate::now_ms();
        let mut dirty = false;
        let remind = reminders_enabled() && crate::notify::enabled();
        for room in client.joined_rooms() {
            let events = crate::appstate::get_state_all(&room, EVENT_TYPE).await;
            if events.is_empty() {
                continue;
            }
            let mine = crate::appstate::get_state(&room, RSVP_TYPE, &me).await.unwrap_or(json!({}));
            for (id, _sender, content) in events {
                let title = content.get("title").and_then(Value::as_str).unwrap_or("").trim().to_string();
                if title.is_empty() || is_canceled(&content) || content.get("kind").and_then(Value::as_str) == Some("focus") {
                    continue;
                }
                let start = effective_start(&content);
                let created_by = content.get("createdBy").and_then(Value::as_str).unwrap_or("");
                let created_at = content.get("createdAt").and_then(Value::as_u64).unwrap_or(0);
                let game = content.get("game").and_then(Value::as_str).unwrap_or("");
                let key_new = format!("new|{id}");
                if !seen.contains(&key_new) {
                    seen.insert(key_new);
                    dirty = true;
                    if remind && created_by != me && now.saturating_sub(created_at) < NEW_EVENT_WINDOW_MS && start > now {
                        let body = if game.is_empty() {
                            format!("{title} · {}", rel_when(start, now))
                        } else {
                            format!("{title} · {game} · {}", rel_when(start, now))
                        };
                        eprintln!("[calendar] new-event notice: {id}");
                        crate::notify::notify("New on the calendar", &body, None).await;
                    }
                }
                // 10.1 (TH-CAL-03): a reschedule is announced, and the reminder fires again for the
                // NEW time. The last start we saw rides in the seen-set as `start|<id>|<ms>`; the
                // reminder key carries the start too, so moving an event re-arms it.
                let key_start = format!("start|{id}|{start}");
                if !seen.contains(&key_start) {
                    let prefix = format!("start|{id}|");
                    let moved = seen.iter().any(|k| k.starts_with(&prefix));
                    seen.retain(|k| !k.starts_with(&prefix));
                    seen.insert(key_start);
                    dirty = true;
                    if moved && remind && created_by != me && start > now {
                        let body = if game.is_empty() {
                            format!("{title} is now {}", rel_when(start, now))
                        } else {
                            format!("{title} · {game} is now {}", rel_when(start, now))
                        };
                        eprintln!("[calendar] moved notice: {id}");
                        crate::notify::notify("Moved on the calendar", &body, None).await;
                    }
                }
                if start > now && start - now <= REMIND_BEFORE_MS {
                    let key = format!("soon|{id}|{start}");
                    if !seen.contains(&key) {
                        seen.insert(key);
                        dirty = true;
                        let status = mine.get(&id).and_then(|e| e.get("status")).and_then(Value::as_str).unwrap_or("");
                        let going = created_by == me || status == "yes" || status == "maybe";
                        if remind && going {
                            let mins = ((start - now) / 60_000).max(1);
                            let body = if game.is_empty() { title.clone() } else { format!("{title} · {game}") };
                            eprintln!("[calendar] reminder: {id} in {mins} min");
                            // 10.1 (TH-CAL-11): the reminder opens the event when clicked.
                            let action = format!("calendar:{}/{id}", room.room_id());
                            crate::notify::notify_action(&format!("Starting in {mins} min"), &body, None, &action).await;
                        }
                    }
                }
            }
        }
        if dirty {
            save_seen(&seen);
        }
        // Keep the open calendar / header chip live without QML polling the store.
        let _ = qt.queue(|mut o| o.as_mut().refresh());
    }
}

// ---------------------------------------------------------------------------------------------
// Headless round-trip (no GUI) — the gate's proof against the live server:
//   TINHAT_CALENDAR_TEST='!room:server' ./tinhat
// create an event → sync → read it back with my RSVP=yes → vote no → read → delete → read.
// ---------------------------------------------------------------------------------------------
pub async fn headless_verify(room_id: &str) {
    let Some(client) = crate::session::client() else {
        eprintln!("[caltest] not signed in");
        return;
    };
    let Some(cid) = container_for_room(&client, room_id).await else {
        let joined = client.joined_rooms();
        eprintln!(
            "[caltest] no container for {room_id} — me={:?} joined={} spaces={} first={:?}",
            client.user_id().map(|u| u.to_string()),
            joined.len(),
            joined.iter().filter(|r| r.is_space()).count(),
            joined.iter().take(3).map(|r| r.room_id().to_string()).collect::<Vec<_>>()
        );
        return;
    };
    eprintln!("[caltest] room {room_id} → container {cid}");
    let sync = || async {
        let _ = client
            .sync_once(crate::bridge::sync_settings().timeout(std::time::Duration::from_secs(10)))
            .await;
    };
    let start = crate::appstate::now_ms() + 2 * 3600_000;
    // 0.8.33: announced into the source room → the chat card (edited below, CANCELLED at the end).
    let draft = json!({
        "title": "caltest Valheim 1.0 launch night", "game": "Valheim", "kind": "session",
        "start": start, "end": start + 3 * 3600_000, "notes": "headless", "rules": "no cheese",
        "freePlay": true, "joinIfYouCan": true, "options": [], "chosen": -1,
        "announceRoom": room_id, "announceWhen": "Thu, Sep 10 · 7:00 PM – 10:00 PM UTC"
    });
    let id = match do_save_event(&cid, &draft.to_string()).await {
        Ok(id) => id,
        Err(e) => {
            eprintln!("[caltest] save FAILED: {e}");
            eprintln!("[caltest] VERDICT: FAIL");
            return;
        }
    };
    // 0.8.27: a main-game range alongside — must be in events, never in upcoming.
    let focus = json!({
        "title": "caltest Valheim season", "game": "Valheim", "kind": "focus",
        "start": start, "end": start + 20 * 24 * 3600_000, "notes": "", "rules": "", "options": [], "chosen": -1
    });
    let focus_id = do_save_event(&cid, &focus.to_string()).await.unwrap_or_default();
    // 0.8.35: an UNLINKED event with a card in the 0.8.32 format, as every pre-0.8.33 event has.
    // The GUI leg (opening the calendar window) must connect it; headless_clean reports it.
    let legacy2 = json!({
        "title": "caltest legacy card", "game": "Valheim", "kind": "session",
        "start": start + 86_400_000, "end": start + 86_400_000 + 3_600_000, "notes": "", "rules": "", "options": [], "chosen": -1
    });
    let legacy2_id = do_save_event(&cid, &legacy2.to_string()).await.unwrap_or_default();
    if let Some(sr) = RoomId::parse(room_id).ok().and_then(|r| client.get_room(&r)) {
        let text = "📅 **caltest legacy card** — Valheim\nGroup session · Sat, Sep 12 · 8:00 PM – 9:00 PM UTC\n\n_RSVP on the calendar (📅 in the room header)._";
        let content = serde_json::to_value(RoomMessageEventContent::text_markdown(text)).unwrap_or_else(|_| json!({ "msgtype": "m.text", "body": text }));
        match sr.send_raw("m.room.message", content).await {
            Ok(_) => eprintln!("[caltest] legacy event {legacy2_id} left with a 0.8.32-format card and no ref (the GUI leg must connect it)"),
            Err(e) => eprintln!("[caltest] legacy card post failed: {e}"),
        }
    }
    sync().await;
    let snap = build_snapshot(&client, &cid, room_id).await;
    let focus_ok = !focus_id.is_empty() && snap.events_json.contains(&focus_id) && !snap.upcoming_json.contains(&focus_id);
    eprintln!("[caltest] focus range: saved={} in_events_not_upcoming={focus_ok}", !focus_id.is_empty());
    eprintln!("[caltest] source={room_id} label='{}' can_invite={} missing={}", snap.source_label, snap.can_invite, snap.missing_json);
    let list: Vec<Value> = serde_json::from_str(&snap.events_json).unwrap_or_default();
    let mine = list.iter().find(|e| e["id"] == id).cloned();
    let created_ok = mine.as_ref().map(|e| e["title"] == "caltest Valheim 1.0 launch night").unwrap_or(false);
    let rsvp_ok = mine.as_ref().map(|e| e["my"]["status"] == "yes" && e["rsvp"]["yes"].as_array().map(|a| !a.is_empty()).unwrap_or(false)).unwrap_or(false);
    let game_ok = snap.games_json.contains("\"Valheim\"");
    eprintln!("[caltest] after create: found={created_ok} rsvp_yes={rsvp_ok} game_listed={game_ok} can_edit={} upcoming={}", snap.can_edit, snap.upcoming_json.len() > 2);
    let vote_ok = do_rsvp(&cid, &id, "no", &[]).await.is_ok();
    sync().await;
    let snap2 = build_snapshot(&client, &cid, room_id).await;
    let list2: Vec<Value> = serde_json::from_str(&snap2.events_json).unwrap_or_default();
    let no_ok = list2.iter().find(|e| e["id"] == id).map(|e| e["my"]["status"] == "no").unwrap_or(false);
    eprintln!("[caltest] after rsvp no: write_ok={vote_ok} read_no={no_ok}");
    // 0.8.33: the chat card. The state event must remember it, the message must carry the
    // marker + the title, and an update must land as an m.replace of that message.
    let source_room = RoomId::parse(room_id).ok().and_then(|r| client.get_room(&r));
    let ann_id = match container_room(&cid) {
        Some(r) => crate::appstate::get_state(&r, EVENT_TYPE, &id)
            .await
            .and_then(|c| c.get("announce").and_then(|a| a.get("event")).and_then(Value::as_str).map(String::from))
            .unwrap_or_default(),
        None => String::new(),
    };
    let recent = match &source_room { Some(r) => recent_events(r, 12).await, None => Vec::new() };
    let card = recent.iter().find(|e| e["event_id"] == ann_id.as_str());
    let card_ok = !ann_id.is_empty()
        && card
            .map(|e| {
                e["content"]["dev.tinhat.calendar"]["event"] == id.as_str()
                    && e["content"]["body"].as_str().map(|b| b.contains("caltest Valheim 1.0 launch night") && b.contains("7:00 PM")).unwrap_or(false)
            })
            .unwrap_or(false);
    eprintln!("[caltest] card: announce_ref={} posted_with_marker={card_ok}", !ann_id.is_empty());
    let mut upd = draft.clone();
    upd["id"] = json!(id);
    if let Some(o) = upd.as_object_mut() {
        o.remove("announceRoom"); // 0.8.35: on an existing event that means "post a FRESH card"
    }
    upd["title"] = json!("caltest Valheim 1.0 launch night (moved)");
    upd["announceWhen"] = json!("Fri, Sep 11 · 8:00 PM – 11:00 PM UTC");
    let upd_ok = do_save_event(&cid, &upd.to_string()).await.is_ok();
    sync().await;
    let recent2 = match &source_room { Some(r) => recent_events(r, 12).await, None => Vec::new() };
    let edit_ok = recent2.iter().any(|e| {
        e["content"]["m.relates_to"]["rel_type"] == "m.replace"
            && e["content"]["m.relates_to"]["event_id"] == ann_id.as_str()
            && e["content"]["m.new_content"]["body"].as_str().map(|b| b.contains("(moved)") && b.contains("8:00 PM")).unwrap_or(false)
            && e["content"]["m.new_content"]["dev.tinhat.calendar"]["event"] == id.as_str()
    });
    eprintln!("[caltest] card after update: save_ok={upd_ok} edited={edit_ok}");
    // 0.8.35: adoption. Every event from before 0.8.33 has a card but no ref to it, so a change
    // could never reach the card. Post a marker-less card in the 0.8.32 format, drop the ref from
    // the state, save a change: the save must find THAT card by its title line, edit it and
    // remember it. Then put things back (redact the stand-in, restore the ref) for the GUI leg.
    let adopt_ok = match (&source_room, container_room(&cid)) {
        (Some(sr), Some(room)) => {
            let legacy_text = "📅 **caltest Valheim 1.0 launch night (moved)**\nGroup session · Fri, Sep 11 · 8:00 PM – 11:00 PM UTC\n\n_RSVP on the calendar (📅 in the room header)._";
            let legacy_content = serde_json::to_value(RoomMessageEventContent::text_markdown(legacy_text))
                .unwrap_or_else(|_| json!({ "msgtype": "m.text", "body": legacy_text }));
            let legacy_id = match sr.send_raw("m.room.message", legacy_content).await {
                Ok(r) => r.response.event_id.to_string(),
                Err(e) => {
                    eprintln!("[caltest] legacy card post failed: {e}");
                    String::new()
                }
            };
            let before = crate::appstate::get_state(&room, EVENT_TYPE, &id).await.unwrap_or_else(|| json!({}));
            let mut stripped = before.clone();
            if let Some(o) = stripped.as_object_mut() {
                o.remove("announce");
            }
            let _ = crate::appstate::send_state(&room, EVENT_TYPE, &id, stripped).await;
            sync().await;
            let mut upd2 = upd.clone();
            upd2["announceWhen"] = json!("Sat, Sep 12 · 9:00 PM – 11:00 PM UTC");
            upd2["announceHint"] = json!(room_id);
            let saved = do_save_event(&cid, &upd2.to_string()).await.is_ok();
            sync().await;
            let recent = recent_events(sr, 12).await;
            let edited = !legacy_id.is_empty()
                && recent.iter().any(|e| {
                    e["content"]["m.relates_to"]["rel_type"] == "m.replace"
                        && e["content"]["m.relates_to"]["event_id"] == legacy_id.as_str()
                        && e["content"]["m.new_content"]["body"].as_str().map(|b| b.contains("9:00 PM")).unwrap_or(false)
                        && e["content"]["m.new_content"]["dev.tinhat.calendar"]["event"] == id.as_str()
                });
            let remembered = crate::appstate::get_state(&room, EVENT_TYPE, &id)
                .await
                .map(|c| c["announce"]["event"] == legacy_id.as_str())
                .unwrap_or(false);
            eprintln!("[caltest] adoption: legacy_card={} saved={saved} edited_that_card={edited} ref_restored={remembered}", !legacy_id.is_empty());
            if let Ok(eid) = matrix_sdk::ruma::OwnedEventId::try_from(legacy_id.as_str()) {
                let _ = sr.redact(&eid, Some("caltest stand-in"), None).await;
            }
            let _ = crate::appstate::send_state(&room, EVENT_TYPE, &id, before).await;
            sync().await;
            saved && edited && remembered
        }
        _ => false,
    };
    if std::env::var("TINHAT_CALENDAR_KEEP").map(|v| v == "1").unwrap_or(false) {
        eprintln!("[caltest] KEEP set — leaving {id} in place for the GUI shot");
        eprintln!("[caltest] VERDICT: {}", if created_ok && rsvp_ok && game_ok && vote_ok && no_ok && card_ok && edit_ok && adopt_ok { "PASS" } else { "FAIL" });
        return;
    }
    // Delete the way the window does (0.8.33): the card says CANCELLED, then the event goes.
    let del_ok = if let Some(room) = container_room(&cid) {
        if let Some(existing) = crate::appstate::get_state(&room, EVENT_TYPE, &id).await {
            if let Some(a) = existing.get("announce").filter(|a| a.is_object()) {
                let mut c = existing.clone();
                c["canceled"] = json!(true);
                let _ = sync_announcement(&client, &cid, &id, a, &c, "Fri, Sep 11 · 8:00 PM – 11:00 PM UTC").await;
            }
        }
        crate::appstate::send_state(&room, EVENT_TYPE, &id, json!({})).await.is_ok()
    } else {
        false
    };
    sync().await;
    if let Some(room) = container_room(&cid) {
        let _ = crate::appstate::send_state(&room, EVENT_TYPE, &focus_id, json!({})).await;
    }
    sync().await;
    let snap3 = build_snapshot(&client, &cid, room_id).await;
    let gone = !snap3.events_json.contains(&id);
    let recent3 = match &source_room { Some(r) => recent_events(r, 12).await, None => Vec::new() };
    let cancel_ok = recent3.iter().any(|e| {
        e["content"]["m.relates_to"]["rel_type"] == "m.replace"
            && e["content"]["m.relates_to"]["event_id"] == ann_id.as_str()
            && e["content"]["m.new_content"]["body"].as_str().map(|b| b.contains("CANCELLED")).unwrap_or(false)
    });
    eprintln!("[caltest] after delete: write_ok={del_ok} gone={gone} card_cancelled={cancel_ok}");
    let pass = created_ok && rsvp_ok && game_ok && vote_ok && no_ok && del_ok && gone && focus_ok && card_ok && edit_ok && cancel_ok && adopt_ok;
    eprintln!("[caltest] VERDICT: {}", if pass { "PASS" } else { "FAIL" });
}

/// The newest `n` timeline events of `room` as JSON (decrypted where the SDK can) — test helper.
async fn recent_events(room: &Room, n: u16) -> Vec<Value> {
    let mut opts = matrix_sdk::room::MessagesOptions::backward();
    opts.limit = matrix_sdk::ruma::UInt::from(n);
    match room.messages(opts).await {
        Ok(resp) => resp
            .chunk
            .iter()
            .filter_map(|ev| ev.raw().deserialize_as::<Value>().ok())
            .map(|mut v| {
                crate::moved::normalize(&mut v); // a moved server: today's ids
                v
            })
            .collect(),
        Err(e) => {
            eprintln!("[caltest] messages failed: {e}");
            Vec::new()
        }
    }
}

/// Delete every `caltest…` event the gate left behind (TINHAT_CALENDAR_CLEAN='!room:server').
pub async fn headless_clean(room_id: &str) {
    let Some(client) = crate::session::client() else { return };
    let Some(cid) = container_for_room(&client, room_id).await else { return };
    let Some(room) = container_room(&cid) else { return };
    let mut n = 0;
    for (id, _sender, content) in crate::appstate::get_state_all(&room, EVENT_TYPE).await {
        if content.get("title").and_then(Value::as_str).map(|t| t.starts_with("caltest")).unwrap_or(false) {
            // 0.8.35: did the GUI leg connect the legacy card? (linkLegacyCards on window open)
            eprintln!(
                "[caltest] legacy-link: '{}' linked={}",
                content.get("title").and_then(Value::as_str).unwrap_or(""),
                content.get("announce").map(|a| a.is_object()).unwrap_or(false)
            );
            if crate::appstate::send_state(&room, EVENT_TYPE, &id, json!({})).await.is_ok() {
                n += 1;
            }
        }
    }
    eprintln!("[caltest] cleaned {n} test event(s) in {cid}");
}
