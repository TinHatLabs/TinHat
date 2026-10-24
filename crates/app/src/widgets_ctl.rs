//! WidgetsController — the widgets' data that is not the live call (modular UI step 2, contract WIDGETS.md §4, §7):
//!
//! * **call history** (Call stats outside a call; WIDGETS.md §10.1 rule 10): per joined room the call-membership
//!   state events from `/messages` (dir=b, a type filter) from the live end backwards, page by page, until a WHOLE
//!   page is older than now − 8 days − 1 h, the room's start, or 20 pages — kept in TIMELINE order (pages reversed;
//!   on the next start stitched to the cache by event id), extended from every sync (`note_sync`, one hook line in
//!   bridge.rs's sync loop; a `limited` sync timeline = a gap → that room is read again), cached locally
//!   (`<data>/widgets-history.json`, per account) and pruned to 8 days except, per slot, the newest older event.
//!   `tinhat_widgets::history::compute_history` (the web's algorithm, proven by the shared vectors) makes the calls,
//!   the week per friend and my last call; names and avatars ride along (`people`). Recomputed on a change, at most
//!   every 5 s focused / 60 s otherwise; nothing is fetched while no Call stats widget is shown (`wantHistory`).
//! * **Up next**: the calendar events (`dev.tinhat.calendar.event` + `.rsvp` + `.game`) of every joined space (and
//!   of a room that is its own calendar, not in a space) — read-only from the local store (no power-level writes; the
//!   RSVP buttons write through the calendar's own `do_rsvp`). QML asks `upNext(days, max, nowMs)` for the selection
//!   (`time::select_up_next`).
//! * **the shared formatters** QML needs, so both apps print the same text: `fmtUntil`, `fmtAgo`, `fmtDur`,
//!   `fmtRate`, `fmtLoss`, `ring`, `sparkPath`; the registry and the widget-list repair.
//!
//! Low GPU: data reaches QML only when it changed — at once while the window is focused, at most once a minute
//! otherwise (widgets.json `refresh`), never while hidden (the latest lands on show).

use core::pin::Pin;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;
use matrix_sdk::ruma::RoomId;
use matrix_sdk::Client;
use serde_json::{json, Map, Value};
use tinhat_widgets::history::{self, MemberEvent};

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
        #[qproperty(QString, history_json)] // QML: historyJson — {calls, week, last, people:{mxid:{name,avatar}}, rooms:{id:name}, at} ("" = not read yet)
        #[qproperty(QString, events_json)] //  QML: eventsJson — every calendar event of every container [{container, containerLabel, id, title, kind, …}] ("" = not read yet)
        #[qproperty(QString, around_json)] //  QML: aroundJson — Who's around: {people:{mxid:{name,avatar,dm}}, rooms:{id:{name,game,emoji,cover,count}}}
        #[qproperty(QString, window_state)] // QML: windowState — the state last passed to setWindow
        #[qproperty(i32, rev)] //              QML: rev — bumps whenever historyJson / eventsJson change
        type WidgetsController = super::WidgetsControllerRust;

        /// QML: start() — load the cache, fetch what is missing, follow the sync (idempotent).
        #[qinvokable]
        fn start(self: Pin<&mut WidgetsController>);
        /// QML: setWindow(state) — "focused" | "unfocused" | "hidden" (the app-wide window state, GPU.md §1).
        #[qinvokable]
        fn set_window(self: Pin<&mut WidgetsController>, state: &QString);
        /// QML: wantHistory(on) — a Call stats widget appeared (true) / went away (false): the call history is read only
        /// while at least one is shown (counted).
        #[qinvokable]
        fn want_history(self: Pin<&mut WidgetsController>, on: bool);
        /// QML: setPresence(usersJson) — the presence map (PresenceController.usersJson) changed: resolve names,
        /// avatars, DM rooms and the call rooms' activity (game, emoji, cover) for Who's around.
        #[qinvokable]
        fn set_presence(self: Pin<&mut WidgetsController>, users_json: &QString);
        /// QML: refreshEvents() — re-read the calendars now (after a write of ours).
        #[qinvokable]
        fn refresh_events(self: Pin<&mut WidgetsController>);
        /// QML: upNext(days, max, nowMs) — the events Up next lists (time::select_up_next), as JSON.
        #[qinvokable]
        fn up_next(self: &WidgetsController, days: i32, max: i32, now_ms: f64) -> QString;
        /// QML: rsvp(container, id, status, optionsJson) — my answer (yes|maybe|no|"" to clear), through the calendar.
        #[qinvokable]
        fn rsvp(self: Pin<&mut WidgetsController>, container: &QString, id: &QString, status: &QString, options_json: &QString);
        /// QML: ring(start, end, nowMs) — the countdown ring {phase, minutesLeft, fill, nextAt} (time::ring_of).
        #[qinvokable]
        fn ring(self: &WidgetsController, start: f64, end: f64, now_ms: f64) -> QString;
        /// QML: fmtUntil(ms) / fmtAgo(ms) / fmtDur(ms) — "in 2 h 10 m", "3 h ago", "45 m".
        #[qinvokable]
        fn fmt_until(self: &WidgetsController, ms: f64) -> QString;
        #[qinvokable]
        fn fmt_ago(self: &WidgetsController, ms: f64) -> QString;
        #[qinvokable]
        fn fmt_dur(self: &WidgetsController, ms: f64) -> QString;
        /// QML: fmtRate(kbps) / fmtLoss(perMille) — "1.4 Mb/s", "2.4 %".
        #[qinvokable]
        fn fmt_rate(self: &WidgetsController, kbps: f64) -> QString;
        #[qinvokable]
        fn fmt_loss(self: &WidgetsController, per_mille: f64) -> QString;
        /// QML: fmtRatePair(down, up) — "↓ 2.4 ↑ 1.1 Mb/s" (people::fmt_rate_pair, WIDGETS.md §10.6).
        #[qinvokable]
        fn fmt_rate_pair(self: &WidgetsController, down: f64, up: f64) -> QString;
        /// QML: callPeople(peopleJson) — the Call stats rows: `{order, bands, summary}` (people::call_people; the input
        /// `[{id, name, cq, inBand, outBand}]`, WIDGETS.md §10.6).
        #[qinvokable]
        fn call_people(self: &WidgetsController, people_json: &QString) -> QString;
        /// QML: groupAround(inputJson) — Who's around's groups `{playing, inCall, callElsewhere, online, away, total,
        /// summary}` of ids (people::group_around; input `{me, people: [{id, name, state, room}], activities, showAway}`).
        #[qinvokable]
        fn group_around(self: &WidgetsController, input_json: &QString) -> QString;
        /// QML: friendsOrder(inputJson) — the Call stats "This week" rows' order (people::friends_by_calls; input
        /// `[{user, calls, name}]`): a JSON array of the users, by calls then name (2026-09-27: no time shown).
        #[qinvokable]
        fn friends_order(self: &WidgetsController, input_json: &QString) -> QString;
        /// QML: fillPm(kbps) — a lane bar's fill in per-mille (quality::rate_fill_pm; the demo rows).
        #[qinvokable]
        fn fill_pm(self: &WidgetsController, kbps: f64) -> f64;
        /// QML: sparkPath(valuesJson, w, h) — the sparkline's points as "x,y x,y …" in px (quality::spark_points / 100).
        #[qinvokable]
        fn spark_path(self: &WidgetsController, values_json: &QString, w: f64, h: f64) -> QString;
        /// QML: registryJson() — widgets.json (the contract copy).
        #[qinvokable]
        fn registry_json(self: &WidgetsController) -> QString;
        /// QML: normalizeItems(json) — a stored widget list repaired ({items, sizes, settings}; items null = none stored).
        #[qinvokable]
        fn normalize_items(self: &WidgetsController, json: &QString) -> QString;
        /// QML: defaultItems() — the list a new user starts with.
        #[qinvokable]
        fn default_items(self: &WidgetsController) -> QString;
        /// QML: envItems() — the widget list pinned by the environment (`TINHAT_WIDGETS`, JSON; "" = none): the gates run
        /// every pre-widget state with `TINHAT_WIDGETS=[]` without writing to the gate device's prefs; while pinned the
        /// list is never saved.
        #[qinvokable]
        fn env_items(self: &WidgetsController) -> QString;
        /// QML: demoMs() — `TINHAT_WIDGETS_DEMO_MS` (the GPU gate's hook, 0 = off): Call stats shows two sample people
        /// whose numbers change every that many ms (focused or not; never while hidden) — the real motion paths
        /// (laneEase, sparkStep, lossFlash) without a call, so the gate can time "one widget update".
        #[qinvokable]
        fn demo_ms(self: &WidgetsController) -> i32;
        /// QML: scenarioOf(id) — widgets W1's gate / review switch: `TINHAT_WIDGETS_SCENARIO` = "id=key[,id=key…]" (e.g.
        /// "pinned=data") makes widget `id` show its sample state `key` instead of live data (the web's
        /// `__twWidgets.scenario(id, key)`; the same sample data in both apps); "" = live. Never saved.
        #[qinvokable]
        fn scenario_of(self: &WidgetsController, id: &QString) -> QString;
        /// QML: pinSummary(count, latest) — Pinned's Small line ("3 pins · latest: …", recent::pin_summary).
        #[qinvokable]
        fn pin_summary(self: &WidgetsController, count: f64, latest: &QString) -> QString;
        /// QML: whenLabel(ts, nowMs, offsetMin, clock) — a past row time: `clock` (the app's clock-mode time) today,
        /// else "Yesterday", "Sat", "Sep 3", "Sep 3, 2025" (recent::when_of in the viewer's zone, offsetMin = minutes
        /// ahead of UTC).
        #[qinvokable]
        fn when_label(self: &WidgetsController, ts: f64, now_ms: f64, offset_min: f64, clock: &QString) -> QString;
        /// QML: setZones(zonesJson) — Friends' clocks (W1): the zones (PresenceController.zonesJson) changed; their
        /// people's names and avatars are resolved into aroundJson.people like Who's around's.
        #[qinvokable]
        fn set_zones(self: Pin<&mut WidgetsController>, zones_json: &QString);
        /// QML: clockRows(zonesJson, myTz, nowMs) — Friends' clocks' rows + header spread (clocks::clock_rows /
        /// spread_label), the offsets from QTimeZone at `nowMs`: {rows: [{id,tz,label,offsetMin,minutes,night,me,day,
        /// diff}], spread, myOffset}. `myTz` = the zone I share (else this device's).
        #[qinvokable]
        fn clock_rows(self: &WidgetsController, zones_json: &QString, my_tz: &QString, now_ms: f64) -> QString;
        /// QML: sampleClockRows(key) — Friends' clocks' sample states (the web's samples: a fixed moment, fixed
        /// offsets): data, many, alone, hidden, empty, loading → the same JSON as clockRows.
        #[qinvokable]
        fn sample_clock_rows(self: &WidgetsController, key: &QString) -> QString;
    }

    impl cxx_qt::Threading for WidgetsController {}
}

type QtThread = cxx_qt::CxxQtThread<qobject::WidgetsController>;

pub struct WidgetsControllerRust {
    history_json: QString,
    events_json: QString,
    around_json: QString,
    window_state: QString,
    rev: i32,
    started: bool,
    /// the session the engine was started for (crate::session::generation)
    started_gen: u64,
}

impl Default for WidgetsControllerRust {
    fn default() -> Self {
        Self {
            history_json: QString::default(),
            events_json: QString::default(), // "" = not read yet (Up next: "Catching up…")
            around_json: QString::default(),
            window_state: QString::from("focused"),
            rev: 0,
            started: false,
            started_gen: 0,
        }
    }
}

// ── the engine (process-wide; the controller is its view) ───────────────────────────────────────────────────────

/// 0 hidden, 1 unfocused, 2 focused.
static VIEW: AtomicU8 = AtomicU8::new(2);
static HIST_DIRTY: AtomicBool = AtomicBool::new(false);
/// The session generation the engine loop runs for (one loop per signed-in session).
static LOOP_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static EVENTS_DIRTY: AtomicBool = AtomicBool::new(false);
static AROUND_DIRTY: AtomicBool = AtomicBool::new(false);
/// Call stats widgets shown (the history is read only while > 0).
static HISTORY_WANTED: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

#[derive(Default)]
struct Engine {
    qt: Option<QtThread>,
    me: String,
    /// room → its membership events (timeline order)
    cache: BTreeMap<String, RoomCache>,
    /// rooms whose sync timeline was limited (a gap to fill)
    gaps: HashSet<String>,
    /// the calendar events of every container (the last read)
    events: Vec<Value>,
    /// my RSVP answers written but not read back yet: (container, event id) → (status, written at ms) — Up next shows
    /// them at once (the calendar's pending answers); a read that agrees, a failed write or 60 s clear them
    pending: HashMap<(String, String), (String, u64)>,
    /// Who's around: the presence map last handed over, the people / rooms resolved for it
    presence: Value,
    /// Friends' clocks: mxid → zone (the people resolved with Who's around's)
    zones: BTreeMap<String, String>,
    people: BTreeMap<String, (Value, u64)>,
    covers: HashMap<String, String>,
    /// what QML has / should get
    history_out: String,
    events_out: String,
    around_out: String,
    pushed_history: String,
    pushed_events: String,
    pushed_around: String,
    last_push_ms: u64,
}

#[derive(Default, Clone)]
struct RoomCache {
    /// timeline order
    events: Vec<MemberEvent>,
    /// the ids that came from the disk cache (the only valid stitch points: a sync-appended event is newer than the
    /// gap an absence leaves)
    disk_ids: HashSet<String>,
    /// read from the server in this session (stitched or re-read; also after an error — retried next session)
    fetched: bool,
}

fn engine() -> &'static Mutex<Engine> {
    static E: OnceLock<Mutex<Engine>> = OnceLock::new();
    E.get_or_init(|| Mutex::new(Engine::default()))
}

/// Sign-out (signout.rs, which removes the history file): the engine forgets the account (its history, events,
/// people); the next session's start() builds it again for its account.
pub(crate) fn forget_session() {
    *engine().lock().unwrap_or_else(|p| p.into_inner()) = Engine::default();
    HIST_DIRTY.store(false, Ordering::SeqCst);
    EVENTS_DIRTY.store(false, Ordering::SeqCst);
    AROUND_DIRTY.store(false, Ordering::SeqCst);
}

fn wake() -> &'static Arc<tokio::sync::Notify> {
    static N: OnceLock<Arc<tokio::sync::Notify>> = OnceLock::new();
    N.get_or_init(|| Arc::new(tokio::sync::Notify::new()))
}

fn now_ms() -> u64 {
    crate::appstate::now_ms()
}

pub(crate) fn cache_path() -> std::path::PathBuf {
    crate::bridge::data_dir().join("widgets-history.json")
}

const CALENDAR_TYPES: [&str; 3] = [crate::calendar::EVENT_TYPE, crate::calendar::RSVP_TYPE, crate::calendar::GAME_TYPE];

/// The sync loop's hook (bridge.rs): membership events extend the history cache, calendar events mark Up next stale,
/// a limited timeline marks a gap to re-read. Cheap: one JSON look at each timeline/state event of each joined room.
pub fn note_sync(response: &matrix_sdk::sync::SyncResponse) {
    let mut hist = false;
    let mut cal = false;
    let mut added: Vec<(String, MemberEvent)> = Vec::new();
    let mut gaps: Vec<String> = Vec::new();
    for (rid, upd) in &response.rooms.joined {
        let room = rid.to_string();
        let mut look = |v: &Value| {
            let ty = v.get("type").and_then(Value::as_str).unwrap_or("");
            if history::MEMBER_TYPES.contains(&ty) {
                if let Some(ev) = MemberEvent::from_matrix(v) {
                    added.push((room.clone(), ev));
                    hist = true;
                }
                cal = true; // Up next's "Join call" follows the rooms' calls
            } else if CALENDAR_TYPES.contains(&ty) {
                cal = true;
            } else if ty == "dev.tinhat.activity" {
                AROUND_DIRTY.store(true, Ordering::SeqCst);
            }
        };
        for ev in &upd.timeline.events {
            if let Ok(mut v) = ev.raw().deserialize_as::<Value>() {
                crate::moved::normalize(&mut v); // a moved server: today's ids
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
        if upd.timeline.limited {
            gaps.push(room.clone());
        }
    }
    if added.is_empty() && !cal && gaps.is_empty() {
        return;
    }
    {
        let mut e = engine().lock().unwrap();
        if e.qt.is_none() {
            return; // the widgets have not started (nothing to keep up to date yet)
        }
        for (room, ev) in added {
            let rc = e.cache.entry(room).or_default();
            if ev.id.as_ref().is_some_and(|id| rc.events.iter().any(|x| x.id.as_ref() == Some(id))) {
                continue;
            }
            rc.events.push(ev);
        }
        for g in gaps {
            e.gaps.insert(g);
        }
    }
    if hist {
        HIST_DIRTY.store(true, Ordering::SeqCst);
    }
    if cal {
        EVENTS_DIRTY.store(true, Ordering::SeqCst);
    }
    wake().notify_one();
}

// ── history: cache, fetch, compute ────────────────────────────────────────────────────────────────────────────

fn load_cache(me: &str) -> BTreeMap<String, RoomCache> {
    let Ok(text) = std::fs::read_to_string(cache_path()) else { return BTreeMap::new() };
    let Ok(v) = serde_json::from_str::<Value>(&text) else { return BTreeMap::new() };
    if v.get("v").and_then(Value::as_u64) != Some(2) || v.get("me").and_then(Value::as_str) != Some(me) {
        return BTreeMap::new(); // another account's (or an older format's) cache
    }
    let mut out = BTreeMap::new();
    for (room, r) in v.get("rooms").and_then(Value::as_object).into_iter().flatten() {
        let events: Vec<MemberEvent> = r.as_array().map(|a| a.iter().filter_map(MemberEvent::from_json).collect()).unwrap_or_default();
        let disk_ids = events.iter().filter_map(|e| e.id.clone()).collect();
        out.insert(room.clone(), RoomCache { events, disk_ids, fetched: false });
    }
    out
}

/// §10.1 rule 10: events older than now − 8 days are dropped EXCEPT, per slot, the newest one of them (the slot's state
/// at the cut). Timeline order kept.
fn prune(events: &[MemberEvent], now: f64) -> Vec<MemberEvent> {
    let floor = now - history::FETCH_MS;
    let mut last_old: HashMap<(String, String), usize> = HashMap::new();
    for (i, e) in events.iter().enumerate() {
        if e.ts < floor {
            last_old.insert((e.ty.clone(), e.key.clone()), i);
        }
    }
    let keep_old: HashSet<usize> = last_old.into_values().collect();
    events.iter().enumerate().filter(|(i, e)| e.ts >= floor || keep_old.contains(i)).map(|(_, e)| e.clone()).collect()
}

fn save_cache(me: &str, cache: &BTreeMap<String, RoomCache>) {
    let now = now_ms() as f64;
    let mut rooms = Map::new();
    for (room, rc) in cache {
        let evs: Vec<Value> = prune(&rc.events, now).iter().map(MemberEvent::to_json).collect();
        if !evs.is_empty() {
            rooms.insert(room.clone(), Value::Array(evs));
        }
    }
    let doc = json!({ "v": 2, "me": me, "rooms": rooms });
    let tmp = cache_path().with_extension("json.tmp");
    if std::fs::write(&tmp, doc.to_string()).is_ok() {
        let _ = std::fs::rename(&tmp, cache_path());
    }
}

/// One room's membership events from the live end backwards (§10.1 rule 10): page by page (100 events), until a WHOLE
/// page is older than `floor`, the room's start, 20 pages — or, when `known` holds ids, the first known id (the stitch
/// point). Returns (the new events in timeline order, stitched?).
async fn fetch_room(room: &matrix_sdk::Room, floor: f64, known: &HashSet<String>) -> Result<(Vec<MemberEvent>, bool), String> {
    use matrix_sdk::room::MessagesOptions;
    let mut newest_first: Vec<MemberEvent> = Vec::new();
    let mut from: Option<String> = None;
    let mut stitched = false;
    'pages: for _ in 0..20 {
        let mut opts = MessagesOptions::backward();
        opts.from = from.clone();
        opts.limit = matrix_sdk::ruma::UInt::from(100u16);
        opts.filter.types = Some(history::MEMBER_TYPES.iter().map(|s| s.to_string()).collect());
        let resp = room.messages(opts).await.map_err(|e| e.to_string())?;
        let mut all_old = !resp.chunk.is_empty();
        for ev in &resp.chunk {
            let Ok(v) = ev.raw().deserialize_as::<Value>() else { continue };
            let Some(me) = MemberEvent::from_matrix(&v) else { continue };
            if me.id.as_ref().is_some_and(|id| known.contains(id)) {
                stitched = true;
                break 'pages;
            }
            if !(me.ts < floor) {
                all_old = false;
            }
            newest_first.push(me);
        }
        if all_old || resp.chunk.is_empty() || resp.end.is_none() || resp.end == from {
            break;
        }
        from = resp.end.clone();
    }
    newest_first.reverse();
    Ok((newest_first, stitched))
}

/// Read every room that has not been read this session (and every gap room again): stitched onto its cache by event
/// id, or — no known id reached (first run, a long absence, a limited sync) — the room's events are the ones read.
async fn fill_gaps(client: &Client) {
    let now = now_ms() as f64;
    let floor = now - history::FETCH_MS - 3_600_000.0;
    let rooms: Vec<matrix_sdk::Room> = client.joined_rooms().into_iter().filter(|r| !r.is_space()).collect();
    let mut changed = false;
    for room in rooms {
        if HISTORY_WANTED.load(Ordering::SeqCst) <= 0 {
            break; // the last Call stats widget went away: stop reading (the rest on the next show)
        }
        let rid = room.room_id().to_string();
        let (need, known) = {
            let e = engine().lock().unwrap();
            let gap = e.gaps.contains(&rid);
            match e.cache.get(&rid) {
                Some(rc) if rc.fetched && !gap => (false, HashSet::new()),
                Some(rc) if !gap => (true, rc.disk_ids.clone()),
                _ => (true, HashSet::new()),
            }
        };
        if !need {
            continue;
        }
        match fetch_room(&room, floor, &known).await {
            Ok((evs, stitched)) => {
                let mut e = engine().lock().unwrap();
                e.gaps.remove(&rid);
                let rc = e.cache.entry(rid.clone()).or_default();
                // stitched: the disk cache's events + what was read after them; else: what was read. Events the sync
                // appended that the read did not return (they arrived meanwhile) stay after them, in arrival order.
                let read: HashSet<String> = evs.iter().filter_map(|x| x.id.clone()).collect();
                let mut list: Vec<MemberEvent> = if stitched {
                    rc.events.iter().filter(|x| x.id.as_ref().is_some_and(|id| rc.disk_ids.contains(id))).cloned().collect()
                } else {
                    Vec::new()
                };
                let base: HashSet<String> = list.iter().filter_map(|x| x.id.clone()).collect();
                list.extend(evs.into_iter().filter(|x| !x.id.as_ref().is_some_and(|id| base.contains(id))));
                for x in &rc.events {
                    let id = x.id.clone().unwrap_or_default();
                    if !id.is_empty() && !read.contains(&id) && !base.contains(&id) {
                        list.push(x.clone());
                    }
                }
                changed |= list.len() != rc.events.len() || list.iter().zip(&rc.events).any(|(a, b)| a.id != b.id);
                rc.events = list;
                rc.fetched = true;
            }
            Err(err) => {
                eprintln!("[widgets] call history: {rid}: {err}");
                engine().lock().unwrap().cache.entry(rid.clone()).or_default().fetched = true; // not again this session
            }
        }
        tokio::time::sleep(Duration::from_millis(150)).await; // gentle on the homeserver
    }
    if changed {
        HIST_DIRTY.store(true, Ordering::SeqCst);
    }
    let (me, cache) = {
        let e = engine().lock().unwrap();
        (e.me.clone(), e.cache.clone())
    };
    save_cache(&me, &cache);
}

async fn compute_history_json(client: &Client) -> String {
    let (me, cache) = {
        let e = engine().lock().unwrap();
        (e.me.clone(), e.cache.clone())
    };
    let now = now_ms() as f64;
    let input = history::HistoryInput {
        me: me.clone(),
        now,
        rooms: cache.iter().map(|(room, rc)| history::RoomEvents { room: room.clone(), events: prune(&rc.events, now) }).collect(),
        // selfhost: the recorder the user's server announces (none = nobody is left out)
        recorder: Some(crate::server_offer::recorder()),
    };
    let h = history::compute_history(&input);
    // names + avatars of everyone the widget can show (the room they were seen in), and the room names
    let mut people_rooms: Vec<(String, String)> = Vec::new();
    for c in &h.calls {
        for m in &c.members {
            people_rooms.push((m.user.clone(), c.room.clone()));
        }
    }
    let mut cache_people: HashMap<String, crate::calendar::Person> = HashMap::new();
    let mut people = Map::new();
    let mut rooms = Map::new();
    for (user, room) in people_rooms {
        let Ok(rid) = RoomId::parse(&room) else { continue };
        let Some(r) = client.get_room(&rid) else { continue };
        rooms.entry(room.clone()).or_insert_with(|| json!(crate::calendar::room_label(&r)));
        if people.contains_key(&user) {
            continue;
        }
        let p = crate::calendar::resolve_person(client, &r, &user, &mut cache_people).await;
        people.insert(user, p);
    }
    let mut v = h.to_json();
    v["people"] = Value::Object(people);
    v["rooms"] = Value::Object(rooms);
    v["me"] = json!(me);
    v["at"] = tinhat_widgets::num(input.now);
    v.to_string()
}

// ── Up next: every container's calendar, read-only ────────────────────────────────────────────────────────────

async fn read_events(client: &Client) -> Vec<Value> {
    use crate::calendar::{auto_color, effective_end, effective_start, is_canceled, resolve_person, room_label, slugify};
    let me = client.user_id().map(|u| u.to_string()).unwrap_or_default();
    let mut out: Vec<Value> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for room in client.joined_rooms() {
        let rid = room.room_id().to_string();
        // every joined room that holds calendar events is a container (a space, or a room outside any space — the
        // web's rule: the room's own calendar state, whatever its parent)
        if !seen.insert(rid.clone()) {
            continue;
        }
        let evs = crate::appstate::get_state_all(&room, crate::calendar::EVENT_TYPE).await;
        if evs.is_empty() {
            continue;
        }
        let label = room_label(&room);
        // RSVP buttons are live only where I may write the calendar's state (the calendar's canEdit)
        let can_write = match (room.power_levels().await, matrix_sdk::ruma::UserId::parse(&me)) {
            (Ok(pl), Ok(uid)) => pl.user_can_send_state(&uid, matrix_sdk::ruma::events::StateEventType::from(crate::calendar::EVENT_TYPE)),
            _ => false,
        };
        let mut games: HashMap<String, String> = HashMap::new();
        for (slug, _s, c) in crate::appstate::get_state_all(&room, crate::calendar::GAME_TYPE).await {
            let name = c.get("name").and_then(Value::as_str).unwrap_or("").trim().to_string();
            if name.is_empty() {
                continue;
            }
            let color = c.get("color").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string).unwrap_or_else(|| auto_color(&slug));
            games.insert(slug.clone(), color.clone());
            games.insert(name.to_lowercase(), color);
        }
        let mut answers: HashMap<String, Vec<(String, String, Vec<i64>)>> = HashMap::new();
        for (user, _s, c) in crate::appstate::get_state_all(&room, crate::calendar::RSVP_TYPE).await {
            for (eid, entry) in c.as_object().into_iter().flatten() {
                let status = entry.get("status").and_then(Value::as_str).unwrap_or("").to_string();
                let opts: Vec<i64> = entry.get("options").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_i64).collect()).unwrap_or_default();
                answers.entry(eid.clone()).or_default().push((user.clone(), status, opts));
            }
        }
        let mut people: HashMap<String, crate::calendar::Person> = HashMap::new();
        // the rooms' live calls (participants, the recorder excluded), read once per room
        let mut counts: HashMap<String, usize> = HashMap::new();
        let mut space_call: Option<(String, usize)> = None;
        let mut space_call_done = false;
        let catalog = crate::games::default_catalog_base();
        for (id, _s, content) in evs {
            let title = content.get("title").and_then(Value::as_str).unwrap_or("").trim().to_string();
            if title.is_empty() {
                continue; // deleted
            }
            let game = content.get("game").and_then(Value::as_str).unwrap_or("").trim().to_string();
            let game_color = if game.is_empty() {
                String::new()
            } else {
                games.get(&slugify(&game)).or_else(|| games.get(&game.to_lowercase())).cloned().unwrap_or_else(|| auto_color(&slugify(&game)))
            };
            let (mut yes, mut maybe, mut no) = (Vec::new(), Vec::new(), Vec::new());
            let mut my_status = String::new();
            let mut my_opts: Vec<i64> = Vec::new();
            for (user, status, opts) in answers.get(&id).into_iter().flatten() {
                let p = resolve_person(client, &room, user, &mut people).await;
                match status.as_str() {
                    "yes" => yes.push(p),
                    "maybe" => maybe.push(p),
                    "no" => no.push(p),
                    _ => {}
                }
                if *user == me {
                    my_status = status.clone();
                    my_opts = opts.clone();
                }
            }
            let created_by = content.get("createdBy").and_then(Value::as_str).unwrap_or("").to_string();
            let announce_room = content.get("announce").and_then(|a| a.get("room")).and_then(Value::as_str).unwrap_or("").to_string();
            // WIDGETS-UI.md §4: the call room = the room the event's card was posted in, else the container when it is
            // a room, else the space's child room with an active call (most participants; ties: room id order)
            let (call_room, call_count) = if !announce_room.is_empty() {
                let n = live_calls(client, &announce_room, &mut counts).await;
                (announce_room.clone(), n)
            } else if !room.is_space() {
                let n = live_calls(client, &rid, &mut counts).await;
                (rid.clone(), n)
            } else {
                if !space_call_done {
                    space_call_done = true;
                    let mut kids: Vec<String> = crate::nav::space_child_ids(&room).await.into_iter().collect();
                    kids.sort();
                    for k in kids {
                        let n = live_calls(client, &k, &mut counts).await;
                        if n > 0 && space_call.as_ref().map_or(true, |(_, best)| n > *best) {
                            space_call = Some((k, n));
                        }
                    }
                }
                space_call.clone().unwrap_or_default()
            };
            let game_id = content.get("gameId").and_then(Value::as_str).unwrap_or("").to_string();
            let cover = if game_id.is_empty() { String::new() } else { crate::games::art_url(&catalog, &game_id, "portrait") };
            out.push(json!({
                "container": rid,
                "containerLabel": label,
                "id": id,
                "title": title,
                "kind": content.get("kind").and_then(Value::as_str).unwrap_or("session"),
                "game": game,
                "gameColor": game_color,
                "start": effective_start(&content),
                "end": effective_end(&content),
                "allDay": content.get("allDay").and_then(Value::as_bool).unwrap_or(false),
                "canceled": is_canceled(&content),
                "createdBy": created_by,
                "announceRoom": announce_room,
                "callRoom": call_room,
                "callCount": call_count,
                "gameId": game_id,
                "cover": cover,
                "rsvp": { "yes": yes, "maybe": maybe, "no": no },
                "my": { "status": my_status, "options": my_opts },
                "canWrite": can_write,
                "containerIsSpace": room.is_space(),
            }));
        }
    }
    out.sort_by(|a, b| {
        let k = |v: &Value| (v["start"].as_u64().unwrap_or(0), v["container"].as_str().unwrap_or("").to_string(), v["id"].as_str().unwrap_or("").to_string());
        k(a).cmp(&k(b))
    });
    out
}

/// Live call participants of a room (the recorder excluded), memoised for one read.
async fn live_calls(client: &Client, room_id: &str, memo: &mut HashMap<String, usize>) -> usize {
    if let Some(n) = memo.get(room_id) {
        return *n;
    }
    let n = match RoomId::parse(room_id).ok().and_then(|r| client.get_room(&r)) {
        Some(r) if r.state() == matrix_sdk::RoomState::Joined => {
            let rec = crate::server_offer::recorder();
            crate::rtc::call_participants(&r).await.iter().filter(|u| !history::is_recorder_user(u, Some(rec.as_deref()))).count()
        }
        _ => 0,
    };
    memo.insert(room_id.to_string(), n);
    n
}

// ── Who's around: names, avatars, DM rooms; the call rooms' activity + cover ────────────────────────────────────

/// A game's portrait by name (the catalogue's search: the item whose name equals it, case-insensitively), cached per
/// name for the session ("" = none).
async fn cover_for(name: &str) -> String {
    let key = name.to_lowercase();
    if let Some(c) = engine().lock().unwrap().covers.get(&key) {
        return c.clone();
    }
    let base = crate::games::default_catalog_base();
    let token = crate::session::client().and_then(|c| c.access_token());
    let path = format!("/search?q={}", crate::games::enc(name));
    let found = match tokio::time::timeout(Duration::from_secs(8), crate::games::catalog_get_auth(&base, &path, token.as_deref())).await {
        Ok(Ok(v)) => {
            let list = v.get("items").or_else(|| v.get("results")).cloned().unwrap_or(v);
            list.as_array()
                .and_then(|a| a.iter().find(|g| g.get("name").and_then(Value::as_str).is_some_and(|n| n.to_lowercase() == key)).cloned())
                .and_then(|g| g.get("id").and_then(Value::as_str).map(|id| crate::games::art_url(&base, id, "portrait")))
                .unwrap_or_default()
        }
        _ => return String::new(), // not cached: tried again on the next change
    };
    engine().lock().unwrap().covers.insert(key, found.clone());
    found
}

async fn resolve_around(client: &Client) -> String {
    let (presence, me, zones) = {
        let e = engine().lock().unwrap();
        (e.presence.clone(), e.me.clone(), e.zones.keys().cloned().collect::<Vec<_>>())
    };
    let now = now_ms();
    let mut people = Map::new();
    let mut rooms = Map::new();
    let empty = Map::new();
    let map = presence.as_object().unwrap_or(&empty);
    for (mxid, entry) in map {
        if *mxid == me {
            continue;
        }
        let cached = engine().lock().unwrap().people.get(mxid).cloned();
        let p = match cached {
            Some((v, at)) if now.saturating_sub(at) < 10 * 60_000 => v,
            _ => {
                let v = resolve_member(client, mxid).await;
                engine().lock().unwrap().people.insert(mxid.clone(), (v.clone(), now));
                v
            }
        };
        people.insert(mxid.clone(), p);
        // the call room (only when we share it — the service says so)
        if entry.get("state").and_then(Value::as_str) != Some("call") {
            continue;
        }
        let Some(rid) = entry.get("room").and_then(Value::as_str) else { continue };
        if rooms.contains_key(rid) {
            continue;
        }
        let Some(room) = RoomId::parse(rid).ok().and_then(|r| client.get_room(&r)) else { continue };
        let act = crate::appstate::get_state(&room, "dev.tinhat.activity", "").await.unwrap_or(Value::Null);
        let game = act.get("game").and_then(Value::as_str).unwrap_or("").trim().to_string();
        let emoji = act.get("emoji").and_then(Value::as_str).unwrap_or("").to_string();
        let cover = if game.is_empty() { String::new() } else { cover_for(&game).await };
        rooms.insert(
            rid.to_string(),
            json!({ "name": crate::calendar::room_label(&room), "game": game, "emoji": emoji, "cover": cover }),
        );
    }
    // Friends' clocks (W1): everyone listed with a zone — me included (my own row)
    for mxid in zones {
        if people.contains_key(&mxid) {
            continue;
        }
        let cached = engine().lock().unwrap().people.get(&mxid).cloned();
        let p = match cached {
            Some((v, at)) if now.saturating_sub(at) < 10 * 60_000 => v,
            _ => {
                let v = resolve_member(client, &mxid).await;
                engine().lock().unwrap().people.insert(mxid.clone(), (v.clone(), now));
                v
            }
        };
        people.insert(mxid, p);
    }
    json!({ "people": people, "rooms": rooms }).to_string()
}

/// A person's display name + avatar (their DM room first, else the first joined room they are in) and their DM room.
async fn resolve_member(client: &Client, mxid: &str) -> Value {
    let fallback = mxid.trim_start_matches('@').split(':').next().unwrap_or(mxid).to_string();
    let Ok(uid) = matrix_sdk::ruma::UserId::parse(mxid) else { return json!({ "name": fallback, "avatar": "", "dm": "" }) };
    let dm = client.get_dm_room(&uid);
    let mut rooms: Vec<matrix_sdk::Room> = dm.iter().cloned().collect();
    rooms.extend(client.joined_rooms().into_iter().filter(|r| !r.is_space()).take(40));
    for r in rooms {
        if let Ok(Some(m)) = r.get_member_no_sync(&uid).await {
            let name = m.display_name().map(str::to_string).unwrap_or_else(|| fallback.clone());
            let avatar = match m.avatar_url() {
                Some(mxc) => crate::bridge::resolve_avatar(client, mxc).await.unwrap_or_default(),
                None => String::new(),
            };
            return json!({ "name": name, "avatar": avatar, "dm": dm.as_ref().map(|d| d.room_id().to_string()).unwrap_or_default() });
        }
    }
    json!({ "name": fallback, "avatar": "", "dm": dm.as_ref().map(|d| d.room_id().to_string()).unwrap_or_default() })
}

// ── the loop: recompute what is stale, push what changed (at the window's pace) ─────────────────────────────────

async fn engine_loop(client: Client) {
    let gen = crate::session::generation();
    HIST_DIRTY.store(true, Ordering::SeqCst);
    EVENTS_DIRTY.store(true, Ordering::SeqCst);
    let mut shown_cache = false;
    let mut computed_at: u64 = 0;
    loop {
        if crate::session::generation() != gen {
            return;
        }
        if EVENTS_DIRTY.swap(false, Ordering::SeqCst) {
            let evs = read_events(&client).await;
            let text = serde_json::to_string(&evs).unwrap_or_else(|_| "[]".into());
            let mut e = engine().lock().unwrap();
            let now = now_ms();
            let read: HashMap<(String, String), String> = evs
                .iter()
                .map(|v| {
                    let k = (v["container"].as_str().unwrap_or("").to_string(), v["id"].as_str().unwrap_or("").to_string());
                    (k, v["my"]["status"].as_str().unwrap_or("").to_string())
                })
                .collect();
            e.pending.retain(|k, (st, at)| now.saturating_sub(*at) < 60_000 && read.get(k) != Some(st));
            e.events = evs;
            e.events_out = text;
        }
        if AROUND_DIRTY.swap(false, Ordering::SeqCst) {
            let a = resolve_around(&client).await;
            engine().lock().unwrap().around_out = a;
        }
        if HISTORY_WANTED.load(Ordering::SeqCst) > 0 {
            if !shown_cache {
                // the cached history at once, before the (slow) reads
                shown_cache = true;
                let h = compute_history_json(&client).await;
                engine().lock().unwrap().history_out = h;
                computed_at = now_ms();
                push(false);
            }
            let pending = {
                let e = engine().lock().unwrap();
                !e.gaps.is_empty() || e.cache.values().all(|rc| !rc.fetched)
                    || client.joined_rooms().iter().any(|r| !r.is_space() && !e.cache.get(r.room_id().as_str()).is_some_and(|rc| rc.fetched))
            };
            if pending {
                fill_gaps(&client).await;
            }
            // a change → recompute, at most every 5 s focused / 60 s otherwise; the week slides: every 5 minutes anyway
            let now = now_ms();
            let min_gap = if VIEW.load(Ordering::SeqCst) == 2 { 5_000 } else { 60_000 };
            let due_change = HIST_DIRTY.load(Ordering::SeqCst) && now.saturating_sub(computed_at) >= min_gap;
            if due_change || now.saturating_sub(computed_at) > 5 * 60_000 {
                HIST_DIRTY.store(false, Ordering::SeqCst);
                let h = compute_history_json(&client).await;
                engine().lock().unwrap().history_out = h;
                computed_at = now_ms();
            }
        }
        push(false);
        // sleep until a sync brings something, or 5 s (a deferred recompute / the unfocused pace)
        let _ = tokio::time::timeout(Duration::from_secs(5), wake().notified()).await;
        tokio::time::sleep(Duration::from_millis(300)).await; // coalesce a burst of sync events
    }
}

/// Hand QML what changed: focused → at once; unfocused → Who's around at once (widgets.json: event), history and Up
/// next at most once a minute; hidden → nothing (the latest on show). `force` = the window just showed / got focus.
fn push(force: bool) {
    let view = VIEW.load(Ordering::SeqCst);
    let (qt, h, ev, ar) = {
        let mut e = engine().lock().unwrap();
        if view == 0 {
            return;
        }
        let now = now_ms();
        let slow_ok = view == 2 || force || now.saturating_sub(e.last_push_ms) >= 60_000;
        let mut h = None;
        let mut ev = None;
        let mut ar = None;
        if slow_ok && e.history_out != e.pushed_history {
            e.pushed_history = e.history_out.clone();
            h = Some(e.history_out.clone());
        }
        if slow_ok && e.events_out != e.pushed_events {
            e.pushed_events = e.events_out.clone();
            ev = Some(e.events_out.clone());
        }
        if e.around_out != e.pushed_around {
            e.pushed_around = e.around_out.clone();
            ar = Some(e.around_out.clone());
        }
        if h.is_none() && ev.is_none() && ar.is_none() {
            return;
        }
        if h.is_some() || ev.is_some() {
            e.last_push_ms = now;
        }
        (e.qt.clone(), h, ev, ar)
    };
    if let Some(qt) = qt {
        let _ = qt.queue(move |mut o| {
            if let Some(h) = h {
                o.as_mut().set_history_json(QString::from(&h));
            }
            if let Some(ev) = ev {
                o.as_mut().set_events_json(QString::from(&ev));
            }
            if let Some(ar) = ar {
                o.as_mut().set_around_json(QString::from(&ar));
            }
            let r = o.rev().wrapping_add(1);
            o.as_mut().set_rev(r);
        });
    }
}

/// Bump `rev` on the GUI thread (a selection input changed without new JSON, e.g. a failed RSVP write).
fn push_rev() {
    let qt = engine().lock().unwrap().qt.clone();
    if let Some(qt) = qt {
        let _ = qt.queue(|mut o| {
            let r = o.rev().wrapping_add(1);
            o.as_mut().set_rev(r);
        });
    }
}

// ── the controller ─────────────────────────────────────────────────────────────────────────────────────────────

impl qobject::WidgetsController {
    pub fn start(mut self: Pin<&mut Self>) {
        // once per session: the controller lives for the process, the engine's loop for a session (it ends with it),
        // so the next session's start runs again (it used to return: the widgets froze after a sign-out + sign-in)
        if self.rust().started && self.rust().started_gen == crate::session::generation() {
            return;
        }
        let Some(client) = crate::session::client() else { return };
        let me = client.user_id().map(|u| u.to_string()).unwrap_or_default();
        if me.is_empty() || std::env::var("TINHAT_VOD_SERVICE").is_ok() {
            return;
        }
        self.as_mut().rust_mut().started = true;
        self.as_mut().rust_mut().started_gen = crate::session::generation();
        let qt = self.qt_thread();
        {
            let mut e = engine().lock().unwrap();
            if e.me != me {
                *e = Engine { me: me.clone(), cache: load_cache(&me), ..Engine::default() };
            }
            e.qt = Some(qt);
            e.pushed_history.clear();
            e.pushed_events.clear();
            e.pushed_around.clear();
        }
        let gen = crate::session::generation();
        if LOOP_GEN.swap(gen, Ordering::SeqCst) != gen {
            crate::bridge::runtime().spawn(engine_loop(client));
        } else {
            push(true); // a second view of the same engine: hand it the latest
        }
    }

    pub fn set_window(mut self: Pin<&mut Self>, state: &QString) {
        let s = state.to_string();
        let v = match s.as_str() {
            "hidden" => 0,
            "unfocused" => 1,
            _ => 2,
        };
        if self.window_state().to_string() != s {
            self.as_mut().set_window_state(QString::from(&s));
        }
        let was = VIEW.swap(v, Ordering::SeqCst);
        if v != 0 && (was == 0 || (was == 1 && v == 2)) {
            push(true); // shown again / focused: the latest at once
        }
    }

    pub fn want_history(self: Pin<&mut Self>, on: bool) {
        let n = if on { HISTORY_WANTED.fetch_add(1, Ordering::SeqCst) + 1 } else { HISTORY_WANTED.fetch_sub(1, Ordering::SeqCst) - 1 };
        if n < 0 {
            HISTORY_WANTED.store(0, Ordering::SeqCst);
        }
        if on {
            wake().notify_one();
        }
    }

    pub fn set_presence(self: Pin<&mut Self>, users_json: &QString) {
        let v: Value = serde_json::from_str(&users_json.to_string()).unwrap_or(Value::Null);
        engine().lock().unwrap().presence = v;
        AROUND_DIRTY.store(true, Ordering::SeqCst);
        wake().notify_one();
    }

    pub fn refresh_events(self: Pin<&mut Self>) {
        EVENTS_DIRTY.store(true, Ordering::SeqCst);
        wake().notify_one();
    }

    pub fn up_next(&self, days: i32, max: i32, now_ms: f64) -> QString {
        let (events, pending) = {
            let e = engine().lock().unwrap();
            (e.events.clone(), e.pending.clone())
        };
        let items: Vec<tinhat_widgets::time::UpNextItem> = events
            .iter()
            .map(|v| tinhat_widgets::time::UpNextItem {
                container: v["container"].as_str().unwrap_or("").to_string(),
                id: v["id"].as_str().unwrap_or("").to_string(),
                kind: v["kind"].as_str().unwrap_or("").to_string(),
                start: v["start"].as_f64().unwrap_or(0.0),
                end: v["end"].as_f64().unwrap_or(0.0),
                canceled: v["canceled"].as_bool().unwrap_or(false),
            })
            .collect();
        let sel = tinhat_widgets::time::select_up_next(&items, now_ms, days as f64, max as f64);
        let out: Vec<Value> = sel
            .iter()
            .filter_map(|s| events.iter().find(|v| v["container"].as_str() == Some(s.container.as_str()) && v["id"].as_str() == Some(s.id.as_str())).cloned())
            .map(|mut v| {
                let k = (v["container"].as_str().unwrap_or("").to_string(), v["id"].as_str().unwrap_or("").to_string());
                if let Some((st, _)) = pending.get(&k) {
                    v["my"]["status"] = json!(st); // the answer just given (not read back yet)
                }
                v
            })
            .collect();
        QString::from(&serde_json::to_string(&out).unwrap_or_else(|_| "[]".into()))
    }

    pub fn rsvp(mut self: Pin<&mut Self>, container: &QString, id: &QString, status: &QString, options_json: &QString) {
        let (cid, id, status) = (container.to_string(), id.to_string(), status.to_string());
        let opts: Vec<i64> = serde_json::from_str::<Vec<i64>>(&options_json.to_string()).unwrap_or_default();
        engine().lock().unwrap().pending.insert((cid.clone(), id.clone()), (status.clone(), now_ms()));
        let r = self.rev().wrapping_add(1);
        self.as_mut().set_rev(r); // Up next re-reads its selection with the pending answer at once
        crate::bridge::runtime().spawn(async move {
            if let Err(e) = crate::calendar::do_rsvp(&cid, &id, &status, &opts).await {
                eprintln!("[widgets] rsvp failed: {e}");
                engine().lock().unwrap().pending.remove(&(cid.clone(), id.clone()));
                push_rev();
            }
            EVENTS_DIRTY.store(true, Ordering::SeqCst);
            wake().notify_one();
        });
    }

    pub fn ring(&self, start: f64, end: f64, now_ms: f64) -> QString {
        QString::from(&tinhat_widgets::time::ring_of(start, end, now_ms).to_json().to_string())
    }
    pub fn fmt_until(&self, ms: f64) -> QString {
        QString::from(&tinhat_widgets::time::fmt_until(ms))
    }
    pub fn fmt_ago(&self, ms: f64) -> QString {
        QString::from(&tinhat_widgets::time::fmt_ago(ms))
    }
    pub fn fmt_dur(&self, ms: f64) -> QString {
        QString::from(&tinhat_widgets::time::fmt_dur(ms))
    }
    pub fn fmt_rate(&self, kbps: f64) -> QString {
        QString::from(&tinhat_widgets::quality::fmt_rate(kbps))
    }
    pub fn fill_pm(&self, kbps: f64) -> f64 {
        tinhat_widgets::quality::rate_fill_pm(kbps)
    }
    pub fn fmt_loss(&self, per_mille: f64) -> QString {
        QString::from(&tinhat_widgets::quality::fmt_loss(per_mille))
    }
    pub fn fmt_rate_pair(&self, down: f64, up: f64) -> QString {
        QString::from(&tinhat_widgets::people::fmt_rate_pair(down, up))
    }
    pub fn call_people(&self, people_json: &QString) -> QString {
        let v: Value = serde_json::from_str(&people_json.to_string()).unwrap_or(Value::Null);
        let people: Vec<tinhat_widgets::people::CallPerson> = v
            .as_array()
            .map(|a| a.iter().map(tinhat_widgets::people::CallPerson::from_json).collect())
            .unwrap_or_default();
        QString::from(&tinhat_widgets::people::call_people(&people).to_json().to_string())
    }
    pub fn group_around(&self, input_json: &QString) -> QString {
        let mut v: Value = serde_json::from_str(&input_json.to_string()).unwrap_or(Value::Null);
        // selfhost: the recorder the user's server announces (null = none, nobody is left out) unless QML named one
        if let Some(o) = v.as_object_mut() {
            o.entry("recorder").or_insert_with(|| crate::server_offer::recorder().map(Value::String).unwrap_or(Value::Null));
        }
        QString::from(&tinhat_widgets::people::group_around(&v).to_json().to_string())
    }
    pub fn friends_order(&self, input_json: &QString) -> QString {
        let v: Value = serde_json::from_str(&input_json.to_string()).unwrap_or(Value::Null);
        let items: Vec<(String, i64, String)> = v
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|x| {
                        (
                            x.get("user").and_then(Value::as_str).unwrap_or("").to_string(),
                            x.get("calls").and_then(Value::as_i64).unwrap_or(0),
                            x.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        QString::from(&Value::from(tinhat_widgets::people::friends_by_calls(&items)).to_string())
    }
    pub fn spark_path(&self, values_json: &QString, w: f64, h: f64) -> QString {
        let vals: Vec<Option<f64>> = serde_json::from_str::<Vec<Value>>(&values_json.to_string())
            .unwrap_or_default()
            .iter()
            .map(Value::as_f64)
            .collect();
        let pts = tinhat_widgets::quality::spark_points(&vals, w, h);
        let s: Vec<String> = pts.iter().map(|(x, y)| format!("{},{}", x / 100.0, y / 100.0)).collect();
        QString::from(&s.join(" "))
    }
    pub fn registry_json(&self) -> QString {
        // selfhost: a widget of a feature the server doesn't offer leaves the registry QML sees — the gallery, the
        // stack and the defaults (server_offer.rs WIDGET_GATES); the contract copy itself is untouched
        let mut v: Value = serde_json::from_str(tinhat_widgets::registry::REGISTRY_JSON).unwrap_or(Value::Null);
        let off = |id: &str| crate::server_offer::gate_of_widget(id).is_some_and(|f| !crate::server_offer::feature_on_global(f));
        if let Some(ws) = v.get_mut("widgets").and_then(Value::as_array_mut) {
            ws.retain(|w| !w.get("id").and_then(Value::as_str).is_some_and(|id| off(id)));
        }
        if let Some(ds) = v.get_mut("defaults").and_then(Value::as_array_mut) {
            ds.retain(|d| !d.as_str().is_some_and(|id| off(id)));
        }
        QString::from(&v.to_string())
    }
    pub fn normalize_items(&self, json: &QString) -> QString {
        use tinhat_widgets::registry::{normalize_items, settings_of, size_of};
        let v: Value = serde_json::from_str(&json.to_string()).unwrap_or(Value::Null);
        let out = match normalize_items(&v) {
            None => json!({ "items": null, "sizes": null, "settings": null }),
            Some(list) => json!({
                "items": list.iter().cloned().map(Value::Object).collect::<Vec<_>>(),
                "sizes": list.iter().map(size_of).collect::<Vec<_>>(),
                "settings": list.iter().map(|it| Value::Object(settings_of(it))).collect::<Vec<_>>(),
            }),
        };
        QString::from(&out.to_string())
    }
    pub fn default_items(&self) -> QString {
        let items: Vec<Value> = tinhat_widgets::registry::default_items().into_iter().map(Value::Object).collect();
        QString::from(&serde_json::to_string(&items).unwrap_or_else(|_| "[]".into()))
    }
    pub fn env_items(&self) -> QString {
        QString::from(std::env::var("TINHAT_WIDGETS").unwrap_or_default().trim())
    }
    pub fn demo_ms(&self) -> i32 {
        std::env::var("TINHAT_WIDGETS_DEMO_MS").ok().and_then(|v| v.trim().parse::<i32>().ok()).filter(|v| *v >= 250).unwrap_or(0)
    }
    pub fn scenario_of(&self, id: &QString) -> QString {
        QString::from(&scenario_in(&std::env::var("TINHAT_WIDGETS_SCENARIO").unwrap_or_default(), &id.to_string()))
    }
    pub fn pin_summary(&self, count: f64, latest: &QString) -> QString {
        QString::from(&tinhat_widgets::recent::pin_summary(count, &latest.to_string()))
    }
    pub fn when_label(&self, ts: f64, now_ms: f64, offset_min: f64, clock: &QString) -> QString {
        let w = tinhat_widgets::recent::when_of(ts, now_ms, offset_min);
        QString::from(&tinhat_widgets::recent::when_label(&w, &clock.to_string()))
    }
    pub fn set_zones(self: Pin<&mut Self>, zones_json: &QString) {
        let v: Value = serde_json::from_str(&zones_json.to_string()).unwrap_or(Value::Null);
        let zones: BTreeMap<String, String> = v
            .as_object()
            .map(|o| o.iter().filter_map(|(k, z)| z.as_str().map(|z| (k.clone(), z.to_string()))).collect())
            .unwrap_or_default();
        let mut e = engine().lock().unwrap();
        if e.zones != zones {
            e.zones = zones;
            drop(e);
            AROUND_DIRTY.store(true, Ordering::SeqCst);
            wake().notify_one();
        }
    }
    pub fn clock_rows(&self, zones_json: &QString, my_tz: &QString, now_ms: f64) -> QString {
        let v: Value = serde_json::from_str(&zones_json.to_string()).unwrap_or(Value::Null);
        let me = engine().lock().unwrap().me.clone();
        let people: Vec<tinhat_widgets::clocks::ClockPerson> = v
            .as_object()
            .map(|o| {
                o.iter()
                    .filter_map(|(id, z)| z.as_str().map(|z| (id, z)))
                    .map(|(id, z)| tinhat_widgets::clocks::ClockPerson {
                        id: id.clone(),
                        name: engine().lock().unwrap().people.get(id).and_then(|(p, _)| p.get("name").and_then(Value::as_str).map(str::to_string)).unwrap_or_else(|| id.trim_start_matches('@').split(':').next().unwrap_or(id).to_string()),
                        tz: z.to_string(),
                        offset_min: zone_offset_min(z, now_ms),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let my_tz = my_tz.to_string();
        let my_off = zone_offset_min(&my_tz, now_ms).or_else(|| system_offset_min(now_ms)).unwrap_or(0.0);
        let rows = tinhat_widgets::clocks::clock_rows(&people, &me, my_off, now_ms);
        let spread = tinhat_widgets::clocks::spread_label(&rows, &my_tz, my_off);
        QString::from(&json!({ "rows": rows.iter().map(|r| r.to_json()).collect::<Vec<_>>(), "spread": spread, "myOffset": my_off }).to_string())
    }
    pub fn sample_clock_rows(&self, key: &QString) -> QString {
        QString::from(&clocks_sample(&key.to_string()).to_string())
    }
}

/// A zone's offset from UTC in minutes at `now_ms` (QTimeZone; None = Qt doesn't know the zone).
fn zone_offset_min(tz: &str, now_ms: f64) -> Option<f64> {
    if !tinhat_widgets::clocks::valid_zone(tz) {
        return None;
    }
    let z = cxx_qt_lib::QTimeZone::from_iana(&cxx_qt_lib::QByteArray::from(tz));
    if z.is_null() || !z.is_valid() {
        return None;
    }
    let t = (now_ms / 60_000.0).floor() * 60_000.0;
    let at = cxx_qt_lib::QDateTime::from_msecs_since_epoch(t as i64, &cxx_qt_lib::QTimeZone::utc());
    Some(f64::from(z.offset_from_utc(&at)) / 60.0)
}

/// This computer's own offset (the fallback when my zone is unknown).
fn system_offset_min(now_ms: f64) -> Option<f64> {
    let z = cxx_qt_lib::QTimeZone::system_time_zone();
    if z.is_null() || !z.is_valid() {
        return None;
    }
    let at = cxx_qt_lib::QDateTime::from_msecs_since_epoch(now_ms as i64, &cxx_qt_lib::QTimeZone::utc());
    Some(f64::from(z.offset_from_utc(&at)) / 60.0)
}

/// Friends' clocks' sample states — the web's `sample()` in features/widgets/clocks/Clocks.tsx: 14:30 in Denver on
/// 2026-09-28, fixed offsets, the sample people (names from the gallery's sample table).
pub(crate) fn clocks_sample(key: &str) -> Value {
    use tinhat_widgets::clocks::{clock_rows, spread_label, ClockPerson};
    let sp = |id: &str| format!("@{id}:sample.invalid");
    let now = 1_790_627_400_000.0; // Date.UTC(2026, 8, 28, 20, 30)
    let me = sp("me");
    let p = |id: String, tz: &str, off: f64| {
        let name = sample_name(&id_local(&id));
        ClockPerson { id, name, tz: tz.into(), offset_min: Some(off) }
    };
    let mut people: Vec<ClockPerson> = if key == "empty" {
        vec![]
    } else {
        let mut v = vec![];
        if key != "hidden" {
            v.push(p(me.clone(), "America/Denver", -360.0));
        }
        v.extend([
            p(sp("ana"), "America/New_York", -240.0),
            p(sp("bo"), "America/Los_Angeles", -420.0),
            p(sp("cy"), "Europe/London", 60.0),
            p(sp("dee"), "America/New_York", -240.0),
        ]);
        v
    };
    if key == "alone" {
        people = vec![p(me.clone(), "America/Denver", -360.0)];
    }
    if key == "many" {
        people.extend([
            p(sp("eli"), "Asia/Tokyo", 540.0),
            p(sp("fay"), "Australia/Sydney", 600.0),
            p(sp("gus"), "America/Chicago", -300.0),
            p(sp("hal"), "Europe/Berlin", 120.0),
            p(sp("ivy"), "Asia/Kolkata", 330.0),
        ]);
    }
    let rows = clock_rows(&people, &me, -360.0, now);
    let spread = spread_label(&rows, "America/Denver", -360.0);
    json!({
        "rows": rows.iter().map(|r| r.to_json()).collect::<Vec<_>>(),
        "spread": spread,
        "myOffset": -600,
        "ready": key != "loading",
        "hidden": key == "hidden",
    })
}

fn id_local(mxid: &str) -> String {
    mxid.trim_start_matches('@').split(':').next().unwrap_or(mxid).to_string()
}

/// The gallery's sample people (the web's names.ts SAMPLE_NAMES).
fn sample_name(local: &str) -> String {
    match local {
        "me" => "You".into(),
        l => {
            let mut c = l.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        }
    }
}

/// `TINHAT_WIDGETS_SCENARIO` ("id=key[,id=key…]", widgets W1) → the key for widget `id` ("" = none / live).
pub(crate) fn scenario_in(spec: &str, id: &str) -> String {
    spec.split(',')
        .filter_map(|p| p.split_once('='))
        .find(|(k, _)| k.trim() == id)
        .map(|(_, v)| v.trim().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod w1_tests {
    use super::scenario_in;

    #[test]
    fn scenario_switch() {
        assert_eq!(scenario_in("pinned=data", "pinned"), "data");
        assert_eq!(scenario_in(" upNext=tbd , pinned = many ", "pinned"), "many");
        assert_eq!(scenario_in("pinned=data", "mentions"), "");
        assert_eq!(scenario_in("", "pinned"), "");
        assert_eq!(scenario_in("garbage,pinned", "pinned"), "");
    }
}
