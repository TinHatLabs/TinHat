//! PlayController — Tin Hat mini games v1 for QML (GAMES-V1-SPEC.md, rev 2): the call's tables, the open table's view,
//! my chips in the call room's SPACE, the one-time opt-in line, game cards in chat and Settings → Games. ONE controller
//! for the app (AppCore.qml).
//!
//! Where a room's games run (§1.1, rev 2): chips belong to a Matrix SPACE. The room's game space is decided by
//! `game_space` on the state this device holds (the room's `m.space.parent` links, and for each linked space I'm joined
//! to: its create event + its `m.space.child` link back). The space's OWNER = the server part of the space's create-event
//! SENDER, never a `via` and never anything another server says. The owner's Tin Hat API is found through the owner's
//! own well-known (`https://<owner>/.well-known/matrix/client` → `dev.tinhat.server.api`, kept on the owner's host by
//! `server_offer::api_allowed`, fetched WITHOUT following redirects), and only there is an OpenID token exchanged.
//! `features.play === true` there or no games (`owner-unsupported`); unreachable = `owner-unavailable`. Results are
//! cached 5 minutes per room (10 per server for discovery).
//!
//! Flow: QML tells us the call room (`setCallRoom`) → the space → discovery → a play session (play_net::sign_in, kept in
//! memory per owner) → the WebSocket `sub`s the room → `room` frames fill `tablesJson` → `openTable` → `view` frames
//! (play_wire::check_view, newest vseq only) fill `viewJson` → QML calls `act({"do": …})`, which names the turn of the
//! view it was made on (play_wire::act_frame); `sit`/`join` and new tables carry the `space` the person was shown, so the
//! server refuses `409 space-changed` before any chip moves. Refused acks become a sentence in `notice` (words.json
//! `codes`). `post` frames (§6.6) send the card into the room as an ordinary message whose body carries no amounts.
//!
//! The words both apps show: `testvectors/games/words.json` (byte-identical in the web): `words()`.
//!
//! Gate hooks (compiled out of release builds, cfg tinhat_env_selftests): `TINHAT_PLAY_BASE` pins the play API base
//! for every room (a lab or S1's mock; the room's own create sender's server stands in for the owner when the room is
//! in no space), `TINHAT_PLAY_SESSION` skips the OpenID exchange (the mock's fixed session), `TINHAT_PLAY_FIXTURE` (a
//! file of `{"tables": […], "view": {…}, "wallet": {…}, "space": {…}}`) draws a fixed table with no server.

use core::pin::Pin;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use cxx_qt::{CxxQtThread, Threading};
use cxx_qt_lib::QString;
use serde_json::{json, Value};

use crate::play_net::{self, ApiError, Session, Sock, SockEvent, Wanted};
use crate::play_wire;

/// The words both apps show (byte-identical in the web).
pub const WORDS: &str = include_str!("testvectors/games/words.json");
/// The UI derivations + the game art both apps draw (byte-identical in the web).
pub const UI: &str = include_str!("testvectors/games/ui.json");

const DISCOVERY_TTL: Duration = Duration::from_secs(600);
const SPACE_TTL: Duration = Duration::from_secs(300);

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
        #[qproperty(QString, state)] //          "" (no call) | "checking" | "nospace" | "signin" | "connecting" | "online" | "retrying" | "off" | "error"
        #[qproperty(bool, available)] //         the call room's game space's owner runs games and I can talk to it
        #[qproperty(bool, rig_offered)] //       gameVisible("rig") for the call room
        #[qproperty(QString, server)] //         the space's owner (for the words: "<space> · <owner>")
        #[qproperty(QString, space_json)] //     {space, name, owner, why, link: [{space, name}]} — why "" = found
        #[qproperty(bool, needs_opt_in)] //      signed in to play, but no wallet in this space yet (the one-time line)
        #[qproperty(QString, wallet_json)] //    GET /me?space= (+ `wallet` pushes): {balance, holdings, rank, cap, …}; "" = none
        #[qproperty(QString, tables_json)] //    the call room's tables (visible games only)
        #[qproperty(QString, table_id)] //       the table I have open ("" = none)
        #[qproperty(QString, view_json)] //      that table's newest checked view ("" = not yet)
        #[qproperty(i32, live_dial)] //          The Rig's live bet dial (§6.3 `live`), -1 = none
        #[qproperty(QString, notice)] //         a sentence for the user (a refused move, a restart, …)
        #[qproperty(i32, notice_rev)]
        #[qproperty(i32, cards_rev)] //          bumps when a chat card's record arrived (QML re-reads cardView)
        #[qproperty(QString, settings_json)] //  Settings → Games: {spaces: [{space, name, owner, wallet}], current: {…}|null}
        type PlayController = super::PlayControllerRust;

        /// QML: setCallRoom(roomId) — the room of my current call ("" = not in a call).
        #[qinvokable]
        fn set_call_room(self: Pin<&mut PlayController>, room: &QString);
        /// QML: recheck() — decide the call room's space again (after a link change, "Use … here").
        #[qinvokable]
        fn recheck(self: Pin<&mut PlayController>);
        /// QML: optIn() — the one-time line's OK: create my wallet in the call room's space.
        #[qinvokable]
        fn opt_in(self: Pin<&mut PlayController>);
        /// QML: useSpaceHere(spaceId) — a room admin's "Use <space>'s chips here": write this room's m.space.parent
        /// for a space that already lists it (§1.1).
        #[qinvokable]
        fn use_space_here(self: Pin<&mut PlayController>, space: &QString);
        /// QML: createTable(game, optsJson) — open a new table in the call room (and open it here).
        #[qinvokable]
        fn create_table(self: Pin<&mut PlayController>, game: &QString, opts_json: &QString);
        /// QML: openTable(tableId) — watch / play this table (the game panel shows it).
        #[qinvokable]
        fn open_table(self: Pin<&mut PlayController>, table: &QString);
        /// QML: closeTable() — stop showing the open table (seats are left with act({"do":"leave"})).
        #[qinvokable]
        fn close_table(self: Pin<&mut PlayController>);
        /// QML: act(actionJson) — a move on the open table, e.g. {"do":"raise","to":18}; → its op id ("" = refused here).
        #[qinvokable]
        fn act(self: Pin<&mut PlayController>, action_json: &QString) -> QString;
        /// QML: dial(bet) — The Rig's live dial (no ack; QML throttles to ≤ 10 a second).
        #[qinvokable]
        fn dial(self: Pin<&mut PlayController>, bet: i32);
        /// QML: refreshWallet() — GET /me again (also the daily top-up check).
        #[qinvokable]
        fn refresh_wallet(self: Pin<&mut PlayController>);
        /// QML: openSettings() — Settings → Games: my wallets in the spaces I know of.
        #[qinvokable]
        fn open_settings(self: Pin<&mut PlayController>);
        /// QML: settingsOptIn() — Settings → Games "Start playing" in the call room's space.
        #[qinvokable]
        fn settings_opt_in(self: Pin<&mut PlayController>);
        /// QML: deleteChips(spaceId) — "Delete my chips in <space>" (after the confirmation).
        #[qinvokable]
        fn delete_chips(self: Pin<&mut PlayController>, space: &QString);
        /// QML: cardView(roomId, blockJson) — a chat card's record ({state: loading|ok|gone|failed|hidden, …}).
        #[qinvokable]
        fn card_view(self: Pin<&mut PlayController>, room: &QString, card_json: &QString) -> QString;
        /// QML: words() — words.json (both apps' words).
        #[qinvokable]
        fn words(self: &PlayController) -> QString;
        /// QML: art() — ui.json's game art colours, QML-ready ({feltCentre: "#1d5a4a", …, sym_hat: …}).
        #[qinvokable]
        fn art(self: &PlayController) -> QString;
        /// QML: cardFace("Td") → {"rank":"10","suit":"♦","red":true} ("" = not a card).
        #[qinvokable]
        fn card_face(self: &PlayController, card: &QString) -> QString;
        /// QML: raisePreset(viewJson, f) — the raise bar's quick amount (f = 0.5 | 1 | -1 = all-in); -1 = none (ui.json).
        #[qinvokable]
        fn raise_preset(self: &PlayController, view_json: &QString, f: f64) -> i32;
        /// QML: seatSlot(seat, mySeat, seats) — where a Hold'em seat is drawn (ui.json `seat_slot`).
        #[qinvokable]
        fn seat_slot(self: &PlayController, seat: i32, my_seat: i32, seats: i32) -> i32;
        /// QML: whisperAlone(viewJson, me) — warn before a Whisper: I'm the only connected House member (client-rules).
        #[qinvokable]
        fn whisper_alone(self: &PlayController, view_json: &QString, me: &QString) -> bool;
        /// QML: chipLabel(tableJson) — the call strip's chip text (ui.json `chip_label`).
        #[qinvokable]
        fn chip_label(self: &PlayController, table_json: &QString) -> QString;
        /// QML: newTableAfterClose() — the host's New table at a table end: close the ended table (the view on screen),
        /// then open a fresh one in this room with its options once the close is acked. false = nothing sent.
        #[qinvokable]
        fn new_table_after_close(self: Pin<&mut PlayController>) -> bool;
        /// QML: cardAction(kind, status, inCall) — a chat card's action: "open" | "join_call" | "" (play_wire::card_action;
        /// status "" = the table isn't listed any more).
        #[qinvokable]
        fn card_action(self: &PlayController, kind: &QString, status: &QString, in_call: bool) -> QString;
        /// QML: reset() — sign-out: forget sessions, sockets, cards.
        #[qinvokable]
        fn reset(self: Pin<&mut PlayController>);
    }

    impl cxx_qt::Threading for PlayController {}
}

#[derive(Default)]
pub struct PlayControllerRust {
    state: QString,
    available: bool,
    rig_offered: bool,
    server: QString,
    space_json: QString,
    needs_opt_in: bool,
    wallet_json: QString,
    tables_json: QString,
    table_id: QString,
    view_json: QString,
    live_dial: i32,
    notice: QString,
    notice_rev: i32,
    cards_rev: i32,
    settings_json: QString,
}

// ── gate hooks (not in release builds) ──────────────────────────────────────────────────────────────────────────

#[cfg(tinhat_env_selftests)]
fn pinned_base() -> Option<String> {
    std::env::var("TINHAT_PLAY_BASE").ok().map(|s| s.trim().trim_end_matches('/').to_string()).filter(|s| !s.is_empty())
}
#[cfg(not(tinhat_env_selftests))]
fn pinned_base() -> Option<String> {
    None
}

#[cfg(tinhat_env_selftests)]
fn pinned_session() -> Option<String> {
    std::env::var("TINHAT_PLAY_SESSION").ok().filter(|s| !s.is_empty())
}
#[cfg(not(tinhat_env_selftests))]
fn pinned_session() -> Option<String> {
    None
}

#[cfg(tinhat_env_selftests)]
fn fixture() -> Option<Value> {
    let p = std::env::var("TINHAT_PLAY_FIXTURE").ok()?;
    serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok()
}
#[cfg(not(tinhat_env_selftests))]
fn fixture() -> Option<Value> {
    None
}

// ── discovery (the owner's own well-known, no redirects) ────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct Discovered {
    /// `<api>/play/v1`; "" = this server runs no games (or couldn't be reached: `reachable`)
    base: String,
    /// gameVisible's ctx `{play, phone_app, rig_phone_apps}` as the server said them
    flags: Value,
    reachable: bool,
}

/// The play part of a well-known (`dev.tinhat.server`): base + flags. Pure (tested).
fn discovered_from(well_known: &Value, server: &str) -> Discovered {
    let t = well_known.get(crate::server_offer::DISCOVERY_KEY);
    let f = t.and_then(|t| t.get("features"));
    let play = f.and_then(|f| f.get("play")) == Some(&Value::Bool(true));
    let rig = f.and_then(|f| f.get("play_rig_phone_apps")) == Some(&Value::Bool(true));
    let flags = json!({"play": play, "phone_app": false, "rig_phone_apps": rig});
    let api = t.and_then(|t| t.get("api")).and_then(Value::as_str).map(|s| s.trim().trim_end_matches('/').to_string()).unwrap_or_default();
    // the announced api must sit on that server's own host (as for my own server's: server_offer::api_allowed)
    let api = if !api.is_empty() && crate::server_offer::api_allowed(&api, &format!("https://{server}"), server) {
        api
    } else {
        format!("https://{server}/api")
    };
    Discovered { base: if play { play_net::play_base(&api) } else { String::new() }, flags, reachable: true }
}

fn no_redirects() -> Option<matrix_sdk::reqwest::Client> {
    use matrix_sdk::reqwest;
    static C: OnceLock<Option<reqwest::Client>> = OnceLock::new();
    // never a fallback client: a default one follows redirects
    C.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(crate::proxy::user_agent())
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10))
            .build()
            .ok()
    })
    .clone()
}

/// A server name we may ask: a host[:port], nothing that changes the URL's meaning.
fn server_name_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 255 && s.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-' | b':' | b'[' | b']'))
}

async fn discover(server: &str) -> Discovered {
    if let Some(b) = pinned_base() {
        return Discovered { base: b, flags: json!({"play": true, "phone_app": false, "rig_phone_apps": false}), reachable: true };
    }
    if !server_name_ok(server) {
        return Discovered { base: String::new(), flags: json!({}), reachable: false };
    }
    if let Some((at, d)) = engine().lock().unwrap().discovery.get(server).cloned() {
        if at.elapsed() < DISCOVERY_TTL {
            return d;
        }
    }
    let url = format!("https://{server}/.well-known/matrix/client");
    let Some(http) = no_redirects() else {
        return Discovered { base: String::new(), flags: json!({}), reachable: false };
    };
    let d = match http.get(&url).send().await {
        Ok(r) if r.status().is_success() => {
            let mut v = r.text().await.ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).unwrap_or(Value::Null);
            play_net::well_known_seen(&mut v);
            discovered_from(&v, server)
        }
        Ok(r) if r.status().as_u16() == 404 => discovered_from(&Value::Null, server),
        _ => Discovered { base: String::new(), flags: json!({}), reachable: false },
    };
    if d.reachable {
        engine().lock().unwrap().discovery.insert(server.to_string(), (Instant::now(), d.clone()));
    }
    d
}

// ── the room's game space (§1.1) ────────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
struct SpaceInfo {
    /// "" = found; else is-space | no-space | unavailable | not-a-room
    why: String,
    space: String,
    owner: String,
    name: String,
    /// spaces I'm in that list this room while it doesn't name them (the "Use <space>'s chips here" offer)
    link: Vec<(String, String)>,
}

impl SpaceInfo {
    fn to_json(&self) -> Value {
        json!({"space": self.space, "name": self.name, "owner": self.owner, "why": self.why,
               "link": self.link.iter().map(|(s, n)| json!({"space": s, "name": n})).collect::<Vec<_>>()})
    }
}


fn room_name(room: &matrix_sdk::Room) -> String {
    room.name().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| room.room_id().to_string())
}

/// game_space's app-mode ctx from the state this device holds (pure shape: client-rules.json `game_space`).
async fn space_ctx(client: &matrix_sdk::Client, room: &matrix_sdk::Room) -> Value {
    let rid = room.room_id().to_string();
    let create = crate::foreign_room::create_event(room).await.unwrap_or(Value::Null);
    let create_type = create.pointer("/content/type").cloned().unwrap_or(Value::Null);
    let mut parents = Vec::new();
    let mut spaces = serde_json::Map::new();
    for (space, _sender, content) in crate::appstate::get_state_all(room, "m.space.parent").await {
        parents.push(json!({"space": space, "content": content}));
        let Ok(sid) = matrix_sdk::ruma::RoomId::parse(&space) else { continue };
        let Some(sp) = client.get_room(&sid) else { continue };
        if sp.state() != matrix_sdk::RoomState::Joined {
            continue; // not held: game_space says unavailable for it
        }
        let sc = crate::foreign_room::create_event(&sp).await.unwrap_or(Value::Null);
        let children: Vec<Value> = crate::appstate::get_state_all(&sp, "m.space.child")
            .await
            .into_iter()
            .filter(|(k, _, _)| *k == rid)
            .map(|(k, _, c)| json!({"room": k, "content": c}))
            .collect();
        spaces.insert(
            space.clone(),
            json!({"create_sender": sc.get("sender").cloned().unwrap_or(Value::Null),
                   "create_type": sc.pointer("/content/type").cloned().unwrap_or(Value::Null),
                   "children": children}),
        );
    }
    let mut r = json!({"id": rid, "parents": parents});
    if !create_type.is_null() {
        r["create_type"] = create_type;
    }
    json!({"mode": "app", "room": r, "spaces": spaces})
}

/// Spaces I'm joined to that list `room_id` (a valid `via`) — the "Use <space>'s chips here" candidates.
async fn listing_spaces(client: &matrix_sdk::Client, room_id: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for sp in client.joined_rooms() {
        if !sp.is_space() {
            continue;
        }
        if let Some(c) = crate::appstate::get_state(&sp, "m.space.child", room_id).await {
            if crate::play_rules::via_ok(&c) {
                out.push((sp.room_id().to_string(), room_name(&sp)));
            }
        }
    }
    out.sort();
    out
}

async fn resolve_space(room_id: &str) -> SpaceInfo {
    if let Some((at, s)) = engine().lock().unwrap().spaces.get(room_id).cloned() {
        if at.elapsed() < SPACE_TTL {
            return s;
        }
    }
    // gate hook: with a pinned lab base the lab answers GET /rooms/{room}/space itself (connect_call takes its answer)
    if pinned_base().is_some() {
        let owner = room_id.split_once(':').map(|(_, s)| s.to_string()).unwrap_or_else(|| "lab.test".into());
        return SpaceInfo { space: room_id.to_string(), owner, name: room_id.to_string(), ..Default::default() };
    }
    let not_room = SpaceInfo { why: "not-a-room".into(), ..Default::default() };
    let Some(client) = crate::session::client() else { return not_room };
    let Ok(rid) = matrix_sdk::ruma::RoomId::parse(room_id) else { return not_room };
    let Some(room) = client.get_room(&rid) else { return not_room };
    let ctx = space_ctx(&client, &room).await;
    let r = crate::play_rules::game_space(&ctx);
    let why = r.get("why").and_then(Value::as_str).unwrap_or("unavailable").to_string();
    let mut info = SpaceInfo {
        why: why.clone(),
        space: r.get("space").and_then(Value::as_str).unwrap_or("").to_string(),
        owner: r.get("server").and_then(Value::as_str).unwrap_or("").to_string(),
        ..Default::default()
    };
    if !info.space.is_empty() {
        if let Some(sp) = matrix_sdk::ruma::RoomId::parse(&info.space).ok().and_then(|s| client.get_room(&s)) {
            info.name = room_name(&sp);
        }
    }
    if why == "no-space" {
        info.link = listing_spaces(&client, room_id).await;
    }
    engine().lock().unwrap().spaces.insert(room_id.to_string(), (Instant::now(), info.clone()));
    info
}

// ── the engine (process-wide) ───────────────────────────────────────────────────────────────────────────────────

#[derive(Default)]
struct Engine {
    qt: Option<CxxQtThread<qobject::PlayController>>,
    /// The Rig's New table at the table end (spec line 687): the close's op, then the game + options to create once
    /// that close is acked (the server counts an `end` table as the room's table until it's closed: one-per-room)
    after_close: Option<(String, String, Value, Instant)>,
    discovery: HashMap<String, (Instant, Discovered)>,
    spaces: HashMap<String, (Instant, SpaceInfo)>,
    /// play sessions per play base (of the account of `acct_gen`)
    sessions: HashMap<String, Session>,
    /// session::generation() the sessions belong to (a sign-out drops them; late answers of the old account are dropped)
    acct_gen: u64,
    /// bumps on every call-room change / recheck: async work of an earlier call never lands in this one
    call_gen: u64,
    /// bumps on every socket start: callbacks of an earlier socket are ignored
    sock_gen: u64,
    /// table → game (from `room` frames and created tables; a `post` names a table)
    table_game: HashMap<String, String>,
    /// the call room, its game space, the owner's base + flags
    call_room: String,
    call_space: SpaceInfo,
    call_base: String,
    call_flags: Value,
    sock: Option<Sock>,
    sock_base: String,
    /// the socket's restarts after a refused session (at most one a minute)
    last_resign: Option<Instant>,
    open_table: Option<String>,
    view: Option<Value>,
    newest: play_wire::Newest,
    /// table → room (from `room` frames; a `post` names a table)
    table_room: HashMap<String, String>,
    /// chat cards' records: key → view JSON
    cards: HashMap<String, Value>,
    /// owner base → the spaces I opened there (Settings)
    known: HashMap<String, Vec<(String, String, String)>>,
}

fn engine() -> &'static Mutex<Engine> {
    static E: OnceLock<Mutex<Engine>> = OnceLock::new();
    E.get_or_init(|| Mutex::new(Engine::default()))
}

fn rt() -> tokio::runtime::Handle {
    crate::bridge::runtime().handle().clone()
}

fn qt() -> Option<CxxQtThread<qobject::PlayController>> {
    engine().lock().unwrap().qt.clone()
}

/// Run `f` on the Qt thread with the controller.
fn ui(f: impl FnOnce(Pin<&mut qobject::PlayController>) + Send + 'static) {
    if let Some(q) = qt() {
        let _ = q.queue(move |o| f(o));
    }
}

fn words() -> &'static Value {
    static W: OnceLock<Value> = OnceLock::new();
    W.get_or_init(|| serde_json::from_str(WORDS).unwrap_or(Value::Null))
}

/// The sentence for a contract code (words.json `codes`; anything unknown = "other").
pub fn sentence_for(code: &str) -> String {
    let c = &words()["codes"];
    c.get(code).or_else(|| c.get("other")).and_then(Value::as_str).unwrap_or("").to_string()
}

fn say(text: String) {
    if text.is_empty() {
        return;
    }
    ui(move |mut o| {
        o.as_mut().set_notice(QString::from(&text));
        let r = o.notice_rev().wrapping_add(1);
        o.as_mut().set_notice_rev(r);
    });
}

fn set_state(s: &'static str) {
    ui(move |mut o| o.as_mut().set_state(QString::from(s)));
}

/// A play session for `base` (an existing one while it's valid, else a fresh OpenID exchange).
async fn session_for(base: &str) -> Result<Session, ApiError> {
    if let Some(s) = engine().lock().unwrap().sessions.get(base).cloned() {
        if s.valid(play_net::now_ms()) {
            return Ok(s);
        }
    }
    let g = crate::session::generation();
    let s = if let Some(token) = pinned_session() {
        Session { token, user: String::new(), expires_ms: play_net::now_ms() + 8 * 3600 * 1000, wallets: Vec::new(), rig_hidden: false }
    } else {
        let client = crate::session::client().ok_or(ApiError { status: 0, code: "signed-out".into() })?;
        play_net::sign_in(&client, base).await?
    };
    let mut e = engine().lock().unwrap();
    if crate::session::generation() != g || e.acct_gen != g {
        // signed out (or another account) while the exchange ran: never keep or use this session
        return Err(ApiError { status: 0, code: "signed-out".into() });
    }
    e.sessions.insert(base.to_string(), s.clone());
    let owners = owners_of(&e, base);
    drop(e);
    // the prestige store reads ranks with this session (no exchange of its own)
    for o in owners {
        crate::prestige::share_session(&o, &s.token, s.expires_ms);
    }
    Ok(s)
}

/// The servers whose well-known named this play base (normally one: the space owner).
fn owners_of(e: &Engine, base: &str) -> Vec<String> {
    e.discovery.iter().filter(|(_, (_, d))| !base.is_empty() && d.base == base).map(|(o, _)| o.clone()).collect()
}

fn mark_wallet(base: &str, space: &str, has: bool) {
    let mut e = engine().lock().unwrap();
    if let Some(s) = e.sessions.get_mut(base) {
        s.wallets.retain(|x| x != space);
        if has {
            s.wallets.push(space.to_string());
        }
    }
}

/// GET /me?space= for the call's space: the header's chips, the opt-in line.
async fn load_wallet(base: String, space: String) {
    let gen = engine().lock().unwrap().call_gen;
    let Ok(s) = session_for(&base).await else { return };
    let path = format!("/me?space={}", play_net::enc(&space));
    let answer = play_net::call(&base, &s.token, "GET", &path, None).await;
    {
        let e = engine().lock().unwrap();
        if e.call_gen != gen || e.call_space.space != space {
            return; // the call moved on: this answer is about another space
        }
    }
    match answer {
        Ok(me) => {
            mark_wallet(&base, &space, true);
            eprintln!(
                "[play] wallet: loaded (balance {}, earned today {})",
                me.get("balance").and_then(Value::as_i64).unwrap_or(-1),
                me.get("earned_today").and_then(Value::as_i64).unwrap_or(-1)
            );
            let text = me.to_string();
            ui(move |mut o| {
                o.as_mut().set_wallet_json(QString::from(&text));
                o.as_mut().set_needs_opt_in(false);
            });
        }
        Err(e) if e.code == "no-wallet" || e.status == 404 => {
            mark_wallet(&base, &space, false);
            ui(|mut o| {
                o.as_mut().set_wallet_json(QString::default());
                o.as_mut().set_needs_opt_in(true);
            });
        }
        Err(e) => eprintln!("[play] /me: {e}"),
    }
}

fn call_target() -> (String, String, String) {
    let e = engine().lock().unwrap();
    (e.call_base.clone(), e.call_room.clone(), e.call_space.space.clone())
}

// ── the socket ──────────────────────────────────────────────────────────────────────────────────────────────────

fn on_sock(base: String, sg: u64) -> Arc<dyn Fn(SockEvent) + Send + Sync> {
    Arc::new(move |ev| {
        if engine().lock().unwrap().sock_gen != sg {
            return; // an earlier socket's last words
        }
        match ev {
        SockEvent::Status(s) => {
            let st: &'static str = match s.as_str() {
                "connecting" => "connecting",
                "online" => "online",
                "retrying" => "retrying",
                "stopped:off" => "off",
                "stopped:closed" => return,
                _ => "error",
            };
            if st == "online" {
                // a new connection: the server sends the open table's snapshot afresh (its vseq may have restarted)
                let mut e = engine().lock().unwrap();
                if let Some(t) = e.open_table.clone() {
                    e.newest.forget(&t);
                }
            }
            if st == "off" {
                say(sentence_for("off"));
            }
            set_state(st);
        }
        SockEvent::SessionExpired => {
            let again = {
                let mut e = engine().lock().unwrap();
                e.sessions.remove(&base);
                e.sock = None;
                let ok = e.last_resign.is_none_or(|t| t.elapsed() > Duration::from_secs(60));
                if ok {
                    e.last_resign = Some(Instant::now());
                }
                ok
            };
            if again {
                rt().spawn(ensure_socket(base.clone()));
            } else {
                set_state("error");
                say(sentence_for("session"));
            }
        }
        SockEvent::Frame(f) => on_frame(&base, f),
        }
    })
}

async fn ensure_socket(base: String) {
    {
        let e = engine().lock().unwrap();
        if e.sock.is_some() && e.sock_base == base {
            return;
        }
    }
    set_state("signin");
    let s = match session_for(&base).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[play] sign-in refused: {e}");
            set_state("error");
            say(sentence_for(&e.code));
            return;
        }
    };
    let space = {
        let mut e = engine().lock().unwrap();
        if e.call_base != base {
            return; // the call moved on while we signed in
        }
        let wanted = Wanted { room: Some(e.call_room.clone()).filter(|r| !r.is_empty()), table: e.open_table.clone() };
        e.sock_gen += 1;
        let sg = e.sock_gen;
        e.sock = play_net::start(&rt(), &base, s.token.clone(), wanted, on_sock(base.clone(), sg));
        e.sock_base = base.clone();
        e.call_space.space.clone()
    };
    let opted = s.wallets.iter().any(|w| *w == space);
    ui(move |mut o| o.as_mut().set_needs_opt_in(!opted));
    rt().spawn(load_wallet(base, space));
}

fn send(frame: Value) -> bool {
    engine().lock().unwrap().sock.as_ref().is_some_and(|s| s.send(frame))
}

fn visible_tables(tables: &Value, flags: &Value) -> Value {
    let list = tables.as_array().cloned().unwrap_or_default();
    Value::Array(
        list.into_iter()
            .filter(|t| t.get("game").and_then(Value::as_str).is_some_and(|g| crate::play_rules::game_visible(g, flags)))
            .filter(|t| t.get("table").and_then(Value::as_str).is_some_and(play_wire::table_id_ok))
            .collect(),
    )
}

fn on_frame(base: &str, f: Value) {
    match f.get("t").and_then(Value::as_str).unwrap_or("") {
        "room" => {
            let room = f.get("room").and_then(Value::as_str).unwrap_or("").to_string();
            let (flags, is_call) = {
                let mut e = engine().lock().unwrap();
                for t in f.get("tables").and_then(Value::as_array).into_iter().flatten() {
                    if let Some(id) = t.get("table").and_then(Value::as_str) {
                        e.table_room.insert(id.to_string(), room.clone());
                        if let Some(g) = t.get("game").and_then(Value::as_str).filter(|g| play_wire::GAMES.contains(g)) {
                            e.table_game.insert(id.to_string(), g.to_string());
                        }
                    }
                }
                (e.call_flags.clone(), e.call_room == room)
            };
            if is_call {
                let tables = visible_tables(f.get("tables").unwrap_or(&Value::Null), &flags);
                ui(move |mut o| o.as_mut().set_tables_json(QString::from(&tables.to_string())));
            }
        }
        "view" => {
            let Some(view) = f.get("view") else { return };
            let (game, table, vseq) = match play_wire::check_view(view) {
                Ok(x) => x,
                Err(why) => {
                    eprintln!("[play] dropped a malformed view: {why}");
                    return;
                }
            };
            let accept = {
                let mut e = engine().lock().unwrap();
                let open = e.open_table.as_deref() == Some(table.as_str());
                let visible = crate::play_rules::game_visible(&game, &e.call_flags);
                let ours = view.get("space").and_then(Value::as_str) == Some(e.call_space.space.as_str());
                if open && visible && ours && e.newest.accept(&table, vseq) {
                    e.view = Some(view.clone());
                    true
                } else {
                    false
                }
            };
            if accept {
                let text = view.to_string();
                let t = table.clone();
                ui(move |mut o| {
                    if o.table_id().to_string() == t {
                        o.as_mut().set_view_json(QString::from(&text));
                        o.as_mut().set_live_dial(-1); // a new snapshot carries its own dial
                    }
                });
            }
        }
        "live" => {
            // a `live` dial belongs to the view of its vseq: applied only to that exact view (contract README)
            let table = f.get("table").and_then(Value::as_str).unwrap_or("");
            let open = {
                let e = engine().lock().unwrap();
                e.open_table.as_deref() == Some(table)
                    && e.view.as_ref().and_then(|v| v.get("vseq")).and_then(Value::as_u64) == f.get("vseq").and_then(Value::as_u64)
            };
            if let (true, Some(d)) = (open, f.get("dial").and_then(Value::as_i64)) {
                let d = d.clamp(0, 3) as i32;
                ui(move |mut o| o.as_mut().set_live_dial(d));
            }
        }
        "ack" => {
            // the New table step 2: the close we sent is done (ok, or the table is already closed / gone)
            let pending = {
                let mut e = engine().lock().unwrap();
                match &e.after_close {
                    Some((op, ..)) if f.get("op").and_then(Value::as_str) == Some(op.as_str()) => e.after_close.take(),
                    Some((.., at)) if at.elapsed() > Duration::from_secs(30) => {
                        e.after_close = None;
                        None
                    }
                    _ => None,
                }
            };
            if let Some((_, game, opts, _)) = pending {
                let code = f.get("code").and_then(Value::as_str).unwrap_or("");
                if f.get("ok") == Some(&Value::Bool(true)) || matches!(code, "closed" | "no-table") {
                    let opts = opts.to_string();
                    ui(move |o| o.create_table(&QString::from(&game), &QString::from(&opts)));
                    return;
                }
            }
            if f.get("ok") == Some(&Value::Bool(false)) && f.get("dup") != Some(&Value::Bool(true)) {
                let code = f.get("code").and_then(Value::as_str).unwrap_or("other").to_string();
                if code == "no-wallet" {
                    ui(|mut o| o.as_mut().set_needs_opt_in(true));
                }
                if code == "space-changed" {
                    // the server's space differs from the one shown: decide it again (nothing moved)
                    ui(|o| o.recheck());
                }
                say(sentence_for(&code));
            }
        }
        "wallet" => {
            let (b, space) = {
                let e = engine().lock().unwrap();
                (e.call_base.clone(), e.call_space.space.clone())
            };
            let fs = f.get("space").and_then(Value::as_str).unwrap_or(&space);
            if b == base && fs == space {
                rt().spawn(load_wallet(b, space));
            }
        }
        "error" => {
            // the answer to a socket request without an op (open of a table I can't see, a malformed frame, …)
            let code = f.get("code").and_then(Value::as_str).unwrap_or("other").to_string();
            eprintln!("[play] socket error frame: {code}");
            if code == "no-table" || code == "rig-hidden" {
                ui(|o| o.close_table());
            }
            say(sentence_for(&code));
        }
        "post" => post_card(&f),
        "rank" => {
            // the prestige store: a rank frame of this owner's socket can only change that owner's spaces
            let owners = owners_of(&engine().lock().unwrap(), base);
            for o in owners {
                crate::prestige::apply_push(&o, &f);
            }
        }
        _ => {}
    }
}

/// §6.6: the server asked this app to post a card. One per (table, kind, round): readers ignore repeats anyway.
fn post_card(f: &Value) {
    let kind = f.get("kind").and_then(Value::as_str).unwrap_or("").to_string();
    let table = f.get("table").and_then(Value::as_str).unwrap_or("").to_string();
    let round = f.get("round").and_then(Value::as_u64);
    if !matches!(kind.as_str(), "invite" | "result" | "summary") || !play_wire::table_id_ok(&table) {
        return;
    }
    let (room, owner, space, game) = {
        let e = engine().lock().unwrap();
        let room = e.table_room.get(&table).cloned().unwrap_or_default();
        let game = e.table_game.get(&table).cloned();
        (room, e.call_space.owner.clone(), e.call_space.space.clone(), game)
    };
    if room.is_empty() || owner.is_empty() || game.is_none() || room != engine().lock().unwrap().call_room {
        return;
    }
    rt().spawn(async move {
        let Some(client) = crate::session::client() else { return };
        let Ok(rid) = matrix_sdk::ruma::RoomId::parse(&room) else { return };
        let Some(r) = client.get_room(&rid) else { return };
        let Some(game) = game else { return }; // never guess a card's game
        let who = match client.account().get_display_name().await {
            Ok(Some(n)) => n,
            _ => client.user_id().map(|u| u.localpart().to_string()).unwrap_or_default(),
        };
        let content = play_wire::card_content(&kind, &game, &owner, &space, &table, round, &who);
        if let Err(e) = r.send_raw("m.room.message", content).await {
            eprintln!("[play] posting a {kind} card failed: {e}");
        }
    });
}

/// Decide the call room's space, discover its owner, sign in, open the socket.
async fn connect_call(room: String, gen: u64) {
    let info = resolve_space(&room).await;
    {
        let mut e = engine().lock().unwrap();
        if e.call_gen != gen {
            return;
        }
        e.call_space = info.clone();
    }
    let sj = info.to_json().to_string();
    ui(move |mut o| o.as_mut().set_space_json(QString::from(&sj)));
    if !info.why.is_empty() {
        set_state("nospace");
        return;
    }
    let d = discover(&info.owner).await;
    {
        let mut e = engine().lock().unwrap();
        if e.call_gen != gen {
            return;
        }
        e.call_base = d.base.clone();
        e.call_flags = d.flags.clone();
    }
    if d.base.is_empty() {
        let why = if d.reachable { "owner-unsupported" } else { "owner-unavailable" };
        let mut i = info.clone();
        i.why = why.to_string();
        engine().lock().unwrap().call_space.why = why.to_string();
        let sj = i.to_json().to_string();
        ui(move |mut o| o.as_mut().set_space_json(QString::from(&sj)));
        set_state("nospace");
        return;
    }
    // confirm with the owner (§6.2 GET /rooms/{room}/space): the same space or nothing; `wrong-server` is shown, never
    // followed. (Gate hook: with a pinned lab base the lab's answer IS the space; the mock has its own space ids.)
    let mut info = info;
    match session_for(&d.base).await {
        Err(e) => {
            eprintln!("[play] sign-in refused: {e}");
            set_state("error");
            say(sentence_for(&e.code));
            return;
        }
        Ok(s) => match play_net::call(&d.base, &s.token, "GET", &format!("/rooms/{}/space", play_net::enc(&room)), None).await {
            Ok(v) => {
                let sp = v.get("space").and_then(Value::as_str).unwrap_or("").to_string();
                let srv = v.get("server").and_then(Value::as_str).unwrap_or("").to_string();
                if pinned_base().is_some() && !sp.is_empty() {
                    info.space = sp;
                    info.owner = srv;
                } else if sp != info.space || srv != info.owner {
                    info.why = "wrong-server".into();
                }
            }
            Err(e) => {
                info.why = match e.code.as_str() {
                    "wrong-server" | "no-space" | "unavailable" | "not-member" => e.code.clone(),
                    _ => "owner-unavailable".into(),
                };
            }
        },
    }
    {
        let mut e = engine().lock().unwrap();
        if e.call_gen != gen {
            return;
        }
        e.call_space = info.clone();
    }
    let sj = info.to_json().to_string();
    ui(move |mut o| o.as_mut().set_space_json(QString::from(&sj)));
    if !info.why.is_empty() {
        set_state("nospace");
        return;
    }
    {
        let mut e = engine().lock().unwrap();
        let k = e.known.entry(d.base.clone()).or_default();
        if !k.iter().any(|(s, _, _)| *s == info.space) {
            k.push((info.space.clone(), info.name.clone(), info.owner.clone()));
        }
    }
    let rig = crate::play_rules::game_visible("rig", &d.flags);
    let owner = info.owner.clone();
    ui(move |mut o| {
        o.as_mut().set_available(true);
        o.as_mut().set_rig_offered(rig);
        o.as_mut().set_server(QString::from(&owner));
    });
    ensure_socket(d.base).await;
}

// ── QML ─────────────────────────────────────────────────────────────────────────────────────────────────────────

impl qobject::PlayController {
    fn bind(self: Pin<&mut Self>) {
        let q = self.qt_thread();
        let mut e = engine().lock().unwrap();
        let g = crate::session::generation();
        if e.acct_gen != g {
            // another sign-in since the sessions were made: forget them all
            e.sessions.clear();
            e.known.clear();
            e.cards.clear();
            e.acct_gen = g;
        }
        e.qt = Some(q);
    }

    fn clear_call(mut self: Pin<&mut Self>) {
        engine().lock().unwrap().after_close = None; // a New table pending in another call never lands here
        // "" = this call's room listing hasn't arrived yet ("[]" = it arrived, empty): the chat card tells
        // "loading" from "closed" by it
        self.as_mut().set_tables_json(QString::default());
        self.as_mut().set_table_id(QString::default());
        self.as_mut().set_view_json(QString::default());
        self.as_mut().set_wallet_json(QString::default());
        self.as_mut().set_space_json(QString::default());
        self.as_mut().set_available(false);
        self.as_mut().set_rig_offered(false);
        self.as_mut().set_needs_opt_in(false);
        self.as_mut().set_live_dial(-1);
    }

    pub fn set_call_room(mut self: Pin<&mut Self>, room: &QString) {
        self.as_mut().bind();
        let room = room.to_string();
        if let Some(fx) = fixture() {
            self.as_mut().apply_fixture(&room, fx);
            return;
        }
        {
            let mut e = engine().lock().unwrap();
            if e.call_room == room {
                return;
            }
            e.call_room = room.clone();
            e.call_gen += 1;
            e.call_space = SpaceInfo::default();
            e.call_base.clear();
            e.call_flags = Value::Null;
            e.sock = None;
            e.sock_gen += 1; // the dropped socket's last words are ignored
            e.sock_base.clear();
            e.open_table = None;
            e.view = None;
            e.newest.clear();
        }
        self.as_mut().clear_call();
        if room.is_empty() {
            self.as_mut().set_state(QString::default());
            return;
        }
        self.as_mut().set_state(QString::from("checking"));
        let gen = engine().lock().unwrap().call_gen;
        rt().spawn(connect_call(room, gen));
    }

    pub fn recheck(mut self: Pin<&mut Self>) {
        let room = {
            let mut e = engine().lock().unwrap();
            let r = e.call_room.clone();
            e.spaces.remove(&r);
            e.call_room.clear();
            r
        };
        self.as_mut().set_call_room(&QString::from(&room));
    }

    fn apply_fixture(mut self: Pin<&mut Self>, room: &str, fx: Value) {
        let on = !room.is_empty();
        self.as_mut().set_available(on);
        self.as_mut().set_rig_offered(on);
        self.as_mut().set_server(QString::from(fx.pointer("/space/owner").and_then(Value::as_str).unwrap_or("lab.test")));
        self.as_mut().set_space_json(QString::from(&fx.get("space").map(Value::to_string).unwrap_or_default()));
        self.as_mut().set_state(QString::from(if on { "online" } else { "" }));
        self.as_mut().set_needs_opt_in(fx.get("needs_opt_in") == Some(&Value::Bool(true)));
        let t = |k: &str| fx.get(k).map(Value::to_string).unwrap_or_default();
        self.as_mut().set_tables_json(QString::from(&t("tables")));
        self.as_mut().set_wallet_json(QString::from(&t("wallet")));
        self.as_mut().set_settings_json(QString::from(&t("settings")));
        if let Some(v) = fx.get("view").filter(|v| play_wire::check_view(v).is_ok()) {
            let id = v.get("table").and_then(Value::as_str).unwrap_or("").to_string();
            engine().lock().unwrap().view = Some(v.clone());
            self.as_mut().set_table_id(QString::from(&id));
            self.as_mut().set_view_json(QString::from(&v.to_string()));
        } else {
            self.as_mut().set_table_id(QString::default());
            self.as_mut().set_view_json(QString::default());
        }
        self.as_mut().set_live_dial(fx.get("dial").and_then(Value::as_i64).map(|d| d as i32).unwrap_or(-1));
    }

    pub fn opt_in(mut self: Pin<&mut Self>) {
        self.as_mut().bind();
        let (base, _, space) = call_target();
        if !base.is_empty() && !space.is_empty() {
            rt().spawn(opt_in_at(base, space));
        }
    }

    pub fn settings_opt_in(mut self: Pin<&mut Self>) {
        self.as_mut().bind();
        let (base, _, space) = call_target();
        if base.is_empty() || space.is_empty() {
            return;
        }
        rt().spawn(async move {
            opt_in_at(base, space).await;
            settings_load().await;
        });
    }

    pub fn use_space_here(mut self: Pin<&mut Self>, space: &QString) {
        self.as_mut().bind();
        let space = space.to_string();
        let room = engine().lock().unwrap().call_room.clone();
        if room.is_empty() || matrix_sdk::ruma::RoomId::parse(&space).is_err() {
            return;
        }
        rt().spawn(async move {
            let Some(client) = crate::session::client() else { return };
            let (Ok(rid), Ok(sid)) = (matrix_sdk::ruma::RoomId::parse(&room), matrix_sdk::ruma::RoomId::parse(&space)) else { return };
            let (Some(r), Some(sp)) = (client.get_room(&rid), client.get_room(&sid)) else { return };
            // only a space that already lists this room (the offer's own check, again)
            if !crate::appstate::get_state(&sp, "m.space.child", &room).await.is_some_and(|c| crate::play_rules::via_ok(&c)) {
                return;
            }
            let owner = crate::foreign_room::create_event(&sp)
                .await
                .and_then(|c| c.get("sender").and_then(Value::as_str).and_then(crate::play_rules::server_of_user_id).map(str::to_string));
            let Some(owner) = owner else { return };
            match crate::appstate::send_state(&r, "m.space.parent", &space, json!({"via": [owner], "canonical": true})).await {
                Ok(()) => {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    ui(|o| o.recheck());
                }
                Err(e) => {
                    eprintln!("[play] linking the room to its space failed: {e}");
                    say("You can't change this room's settings.".to_string());
                }
            }
        });
    }

    pub fn create_table(mut self: Pin<&mut Self>, game: &QString, opts_json: &QString) {
        self.as_mut().bind();
        let game = game.to_string();
        let opts: Value = serde_json::from_str(&opts_json.to_string()).unwrap_or(json!({}));
        let (base, room, space) = call_target();
        let flags = engine().lock().unwrap().call_flags.clone();
        if base.is_empty() || room.is_empty() || !crate::play_rules::game_visible(&game, &flags) {
            return;
        }
        let gen = engine().lock().unwrap().call_gen;
        rt().spawn(async move {
            let Ok(s) = session_for(&base).await else { return };
            let path = format!("/rooms/{}/tables", play_net::enc(&room));
            let body = json!({"game": &game, "opts": opts, "space": space});
            match play_net::call(&base, &s.token, "POST", &path, Some(&body)).await {
                Ok(v) => {
                    if let Some(t) = v.get("table").and_then(Value::as_str).filter(|t| play_wire::table_id_ok(t)) {
                        let t = t.to_string();
                        {
                            let mut e = engine().lock().unwrap();
                            if e.call_gen != gen {
                                return; // another call by now: don't open this one here
                            }
                            e.table_room.insert(t.clone(), room.clone());
                            e.table_game.insert(t.clone(), game.clone());
                        }
                        ui(move |o| o.open_table(&QString::from(&t)));
                    }
                }
                Err(e) => {
                    if e.code == "space-changed" {
                        ui(|o| o.recheck());
                    }
                    say(sentence_for(&e.code));
                }
            }
        });
    }

    pub fn open_table(mut self: Pin<&mut Self>, table: &QString) {
        self.as_mut().bind();
        let table = table.to_string();
        if !play_wire::table_id_ok(&table) || engine().lock().unwrap().open_table.as_deref() == Some(table.as_str()) {
            return;
        }
        let prev = {
            let mut e = engine().lock().unwrap();
            let prev = e.open_table.replace(table.clone());
            if let Some(s) = &e.sock {
                if let Ok(mut w) = s.wanted.lock() {
                    w.table = Some(table.clone());
                }
            }
            if prev.as_deref() != Some(table.as_str()) {
                e.view = None;
                e.newest.forget(&table);
            }
            prev
        };
        if let Some(p) = prev.filter(|p| *p != table) {
            send(json!({"t": "close", "table": p}));
        }
        send(json!({"t": "open", "table": table}));
        self.as_mut().set_table_id(QString::from(&table));
        self.as_mut().set_view_json(QString::default());
        self.as_mut().set_live_dial(-1);
    }

    pub fn close_table(mut self: Pin<&mut Self>) {
        let prev = {
            let mut e = engine().lock().unwrap();
            if let Some(s) = &e.sock {
                if let Ok(mut w) = s.wanted.lock() {
                    w.table = None;
                }
            }
            e.view = None;
            e.open_table.take()
        };
        if let Some(p) = prev {
            send(json!({"t": "close", "table": p}));
        }
        self.as_mut().set_table_id(QString::default());
        self.as_mut().set_view_json(QString::default());
    }

    pub fn act(self: Pin<&mut Self>, action_json: &QString) -> QString {
        let Ok(mut a) = serde_json::from_str::<Value>(&action_json.to_string()) else { return QString::default() };
        let Some(action) = a.get("do").and_then(Value::as_str).map(str::to_string) else { return QString::default() };
        // the move names the turn of the view ON SCREEN (viewJson), never a newer one that arrived meanwhile
        let Ok(view) = serde_json::from_str::<Value>(&self.view_json().to_string()) else { return QString::default() };
        // §1.1: sit / join carry the space the person was shown (the displayed view's; 409 space-changed before any
        // chip moves)
        if matches!(action.as_str(), "sit" | "join") {
            let Some(space) = view.get("space").and_then(Value::as_str) else { return QString::default() };
            a["space"] = json!(space);
        }
        let op = play_wire::new_op_id();
        let Some(frame) = play_wire::act_frame(&view, &op, &a) else { return QString::default() };
        if !send(frame) {
            say(sentence_for("network"));
            return QString::default();
        }
        QString::from(&op)
    }

    pub fn new_table_after_close(mut self: Pin<&mut Self>) -> bool {
        let Ok(view) = serde_json::from_str::<Value>(&self.view_json().to_string()) else { return false };
        let Some(game) = view.get("game").and_then(Value::as_str).map(str::to_string) else { return false };
        let opts = fresh_opts(&view);
        let op = self.as_mut().act(&QString::from(r#"{"do":"close"}"#)).to_string();
        if op.is_empty() {
            return false;
        }
        engine().lock().unwrap().after_close = Some((op, game, opts, Instant::now()));
        true
    }

    pub fn dial(self: Pin<&mut Self>, bet: i32) {
        let Ok(view) = serde_json::from_str::<Value>(&self.view_json().to_string()) else { return };
        let Some(frame) = play_wire::act_frame(&view, &play_wire::new_op_id(), &json!({"do": "dial", "bet": bet.clamp(1, 3)})) else { return };
        send(frame);
    }

    pub fn refresh_wallet(mut self: Pin<&mut Self>) {
        self.as_mut().bind();
        let (base, _, space) = call_target();
        if !base.is_empty() && !space.is_empty() {
            rt().spawn(load_wallet(base, space));
        }
    }

    pub fn open_settings(mut self: Pin<&mut Self>) {
        self.as_mut().bind();
        if fixture().is_some() {
            return;
        }
        rt().spawn(settings_load());
    }

    pub fn delete_chips(mut self: Pin<&mut Self>, space: &QString) {
        self.as_mut().bind();
        let space = space.to_string();
        let base = {
            let e = engine().lock().unwrap();
            e.known.iter().find(|(_, v)| v.iter().any(|(s, _, _)| *s == space)).map(|(b, _)| b.clone())
        };
        let Some(base) = base else { return };
        rt().spawn(async move {
            let Ok(s) = session_for(&base).await else { return };
            match play_net::call(&base, &s.token, "DELETE", &format!("/wallet?space={}", play_net::enc(&space)), None).await {
                Ok(_) => {
                    mark_wallet(&base, &space, false);
                    let (cb, _, cs) = call_target();
                    if cb == base && cs == space {
                        ui(|mut o| {
                            o.as_mut().set_wallet_json(QString::default());
                            o.as_mut().set_needs_opt_in(true);
                        });
                    }
                    settings_load().await;
                }
                Err(e) => say(sentence_for(&e.code)),
            }
        });
    }

    pub fn card_view(mut self: Pin<&mut Self>, room: &QString, card_json: &QString) -> QString {
        self.as_mut().bind();
        let room = room.to_string();
        let Ok(content) = serde_json::from_str::<Value>(&card_json.to_string()) else { return QString::default() };
        // QML passes the `dev.tinhat.game` block itself, or the whole content
        let wrapped = if content.get(play_wire::CARD_KEY).is_some() { content } else { json!({play_wire::CARD_KEY: content}) };
        let Some(card) = play_wire::card_of(&wrapped) else {
            return QString::from(&json!({"state": "failed"}).to_string());
        };
        let key = format!("{room}|{}|{}|{}|{}|{}", card.server, card.space, card.table, card.kind, card.round.unwrap_or(0));
        if let Some(v) = engine().lock().unwrap().cards.get(&key) {
            // a transient failure (network, discovery) is asked again after a minute, and so is an invite's listing
            // snapshot (a live table closes); loading / gone / result + summary records stand
            let again = v["state"] == json!("failed") || (v["kind"] == json!("invite") && v["state"] == json!("ok"));
            let age = v["_at"].as_u64().map(|t| play_net::now_ms().saturating_sub(t));
            // a load that never finished (its future stuck or dropped) is asked again after 20 s, never left at "loading"
            let stuck = v["state"] == json!("loading") && age.is_some_and(|a| a > 20_000);
            let stale = stuck || (again && age.is_some_and(|a| a > 60_000));
            if !stale {
                return QString::from(&v.to_string());
            }
        }
        let loading = json!({"state": "loading", "kind": card.kind, "game": card.game, "table": card.table, "_at": play_net::now_ms()});
        engine().lock().unwrap().cards.insert(key.clone(), loading.clone());
        rt().spawn(load_card(key, room, card));
        QString::from(&loading.to_string())
    }

    pub fn words(&self) -> QString {
        QString::from(WORDS)
    }

    pub fn art(&self) -> QString {
        let ui: Value = serde_json::from_str(UI).unwrap_or(Value::Null);
        QString::from(&play_wire::art_for_qml(&ui).to_string())
    }

    pub fn card_face(&self, card: &QString) -> QString {
        play_wire::card_face(&card.to_string()).map(|v| QString::from(&v.to_string())).unwrap_or_default()
    }

    pub fn raise_preset(&self, view_json: &QString, f: f64) -> i32 {
        let Ok(v) = serde_json::from_str::<Value>(&view_json.to_string()) else { return -1 };
        let f = if f < 0.0 { json!("max") } else { json!(f) };
        play_wire::raise_preset_view(&v, &f).map(|n| n.min(i32::MAX as u64) as i32).unwrap_or(-1)
    }

    pub fn seat_slot(&self, seat: i32, my_seat: i32, seats: i32) -> i32 {
        play_wire::seat_slot(seat.max(0) as u32, my_seat.max(0) as u32, seats.clamp(1, 8) as u32) as i32
    }

    pub fn whisper_alone(&self, view_json: &QString, me: &QString) -> bool {
        let v: Value = serde_json::from_str(&view_json.to_string()).unwrap_or(Value::Null);
        crate::play_rules::whisper_alone_warn(&json!({"roster": v.get("players").cloned().unwrap_or(Value::Null), "whisperer": me.to_string()}))
    }

    pub fn card_action(&self, kind: &QString, status: &QString, in_call: bool) -> QString {
        let st = status.to_string();
        QString::from(play_wire::card_action(&kind.to_string(), (!st.is_empty()).then_some(st.as_str()), in_call))
    }

    pub fn chip_label(&self, table_json: &QString) -> QString {
        let t: Value = serde_json::from_str(&table_json.to_string()).unwrap_or(Value::Null);
        QString::from(&play_wire::chip_label(words(), &t))
    }

    pub fn reset(mut self: Pin<&mut Self>) {
        {
            let mut e = engine().lock().unwrap();
            let q = e.qt.take();
            *e = Engine { qt: q, acct_gen: crate::session::generation(), ..Default::default() };
        }
        self.as_mut().clear_call();
        self.as_mut().set_state(QString::default());
        self.as_mut().set_server(QString::default());
        self.as_mut().set_settings_json(QString::default());
    }
}

async fn opt_in_at(base: String, space: String) {
    let Ok(s) = session_for(&base).await else { return };
    match play_net::call(&base, &s.token, "POST", "/wallet", Some(&json!({"space": space}))).await {
        Ok(_) => {
            mark_wallet(&base, &space, true);
            ui(|mut o| o.as_mut().set_needs_opt_in(false));
            load_wallet(base, space).await;
        }
        Err(e) => say(sentence_for(&e.code)),
    }
}

/// Settings → Games: one row per space I opened games in this session (with its wallet), the call's space first.
async fn settings_load() {
    let g = crate::session::generation();
    let (known, current) = {
        let e = engine().lock().unwrap();
        (e.known.clone(), e.call_space.clone())
    };
    let mut rows = Vec::new();
    for (base, spaces) in known {
        let Ok(s) = session_for(&base).await else { continue };
        for (space, name, owner) in spaces {
            let wallet = if s.wallets.iter().any(|w| *w == space) {
                play_net::call(&base, &s.token, "GET", &format!("/me?space={}", play_net::enc(&space)), None).await.ok()
            } else {
                None
            };
            rows.push(json!({"space": space, "name": name, "owner": owner, "wallet": wallet, "current": space == current.space}));
        }
    }
    rows.sort_by_key(|r| r["current"] != json!(true));
    let out = json!({"spaces": rows, "current": if current.space.is_empty() { Value::Null } else { current.to_json() }});
    if crate::session::generation() != g {
        return; // another account by now
    }
    let text = out.to_string();
    ui(move |mut o| o.as_mut().set_settings_json(QString::from(&text)));
}

/// A fresh table's options from an ended one's view (the web's newTable: The Rig keeps preset, bankroll and patch;
/// Hold'em its seats and blinds). Only known keys, never the whole view.
fn fresh_opts(view: &Value) -> Value {
    let o = view.get("opts").cloned().unwrap_or(json!({}));
    let keys: &[&str] = if view.get("game").and_then(Value::as_str) == Some("rig") { &["preset", "bankroll", "patch"] } else { &["seats", "sb", "bb"] };
    let mut out = serde_json::Map::new();
    for k in keys {
        if let Some(v) = o.get(*k) {
            out.insert((*k).to_string(), v.clone());
        }
    }
    Value::Object(out)
}

/// An invite card's table in the room's listing (`GET /rooms/{room}/tables` = the contract's `tables`), pure: its row
/// while it's listed and not closed, else `{"closed": true}`. A listing for another room's space is never trusted for a
/// row (the rows carry no room: the request named it).
fn invite_row(listing: &Value, table: &str, _room: &str) -> Value {
    listing
        .get("tables")
        .and_then(Value::as_array)
        .and_then(|rows| {
            rows.iter().find(|t| {
                t.get("table").and_then(Value::as_str) == Some(table) && t.get("status").and_then(Value::as_str) != Some("closed")
            })
        })
        .cloned()
        .unwrap_or_else(|| json!({"closed": true}))
}

#[derive(Debug, PartialEq)]
enum RecordRoom {
    Here,
    Elsewhere,
    AskTable,
}

/// Whose room a card's record is (pure): a record naming a room (`summary`) must name this one; one naming only its
/// table (`round`) must name the card's table, and then the table's room decides (`AskTable`).
fn record_room(v: &Value, table: &str, room: &str) -> RecordRoom {
    match v.get("room").and_then(Value::as_str) {
        Some(r) if r == room && v.get("table").and_then(Value::as_str).is_none_or(|t| t == table) => RecordRoom::Here,
        Some(_) => RecordRoom::Elsewhere,
        None if v.get("table").and_then(Value::as_str) == Some(table) => RecordRoom::AskTable,
        None => RecordRoom::Elsewhere,
    }
}

/// A chat card's record (§6.6, rev 2): fetched only from the server keeping THIS room's game space (a card naming
/// another server or space is "failed" and nothing is fetched from what it names); the record's room must be this room.
async fn load_card(key: String, room: String, card: play_wire::Card) {
    let g = crate::session::generation();
    let finish = |mut v: Value| {
        if crate::session::generation() != g {
            return; // another account by now: never cache this one's records
        }
        v["_at"] = json!(play_net::now_ms());
        let mut e = engine().lock().unwrap();
        if e.cards.len() > 400 {
            e.cards.clear(); // bounded: a long game-heavy history re-asks rather than growing forever
        }
        e.cards.insert(key.clone(), v);
        drop(e);
        ui(|mut o| {
            let r = o.cards_rev().wrapping_add(1);
            o.as_mut().set_cards_rev(r);
        });
    };
    let with = |state: &str, data: Option<Value>| {
        let mut v = json!({"kind": card.kind, "game": card.game, "table": card.table, "round": card.round, "state": state});
        if let Some(d) = data {
            v["data"] = d;
        }
        v
    };
    let tag = format!("{} {} {}", card.game, card.kind, &card.table[..card.table.len().min(6)]);
    eprintln!("[play] card {tag}: load");
    let info = resolve_space(&room).await;
    eprintln!("[play] card {tag}: space resolved ({})", if info.why.is_empty() { "ok" } else { info.why.as_str() });
    if !info.why.is_empty() || info.owner != card.server || (!card.space.is_empty() && info.space != card.space) {
        let why = if !info.why.is_empty() { info.why.as_str() } else if info.owner != card.server { "other-server" } else { "other-space" };
        eprintln!("[play] card {} {}: not this room's games ({why})", card.game, card.kind);
        finish(with("failed", None));
        return;
    }
    let d = discover(&info.owner).await;
    if d.base.is_empty() {
        eprintln!("[play] card {} {}: the owner's play API isn't found", card.game, card.kind);
        finish(with("failed", None));
        return;
    }
    if !crate::play_rules::game_visible(&card.game, &d.flags) {
        finish(with("hidden", None));
        return;
    }
    eprintln!("[play] card {tag}: discovered");
    let Ok(s) = session_for(&d.base).await else {
        eprintln!("[play] card {tag}: no play session");
        finish(with("failed", None));
        return;
    };
    eprintln!("[play] card {tag}: session");
    if card.kind == "invite" {
        // an invite draws from the ROOM's live table list (the web lists every room's): in my call the strip's list is
        // fresher (the QML prefers it); outside it, GET /rooms/{room}/tables answers who opened it and how many sit.
        // The row, or {"closed": true} when the table isn't listed any more.
        let p = format!("/rooms/{}/tables", play_net::enc(&room));
        match play_net::call(&d.base, &s.token, "GET", &p, None).await {
            Ok(v) => {
                eprintln!("[play] card {tag}: listing answered");
                finish(with("ok", Some(invite_row(&v, &card.table, &room))))
            }
            Err(e) => {
                eprintln!("[play] card {} invite: the room's tables answered {} {}", card.game, e.status, e.code);
                finish(with("failed", None))
            }
        }
        return;
    }
    let path = match (card.game.as_str(), card.kind.as_str(), card.round) {
        ("rig", "result", Some(r)) => format!("/tables/{}/rounds/{r}", card.table),
        _ => format!("/tables/{}/summary", card.table),
    };
    match play_net::call(&d.base, &s.token, "GET", &path, None).await {
        Ok(v) => {
            let here = match record_room(&v, &card.table, &room) {
                RecordRoom::Here => true,
                RecordRoom::Elsewhere => false,
                // a round record (the contract's `round`) names its table, not a room: the table's room decides, from the
                // room's live table list, else from the table's summary (which names its room)
                RecordRoom::AskTable => {
                    let listed = engine().lock().unwrap().table_room.get(&card.table).cloned();
                    match listed {
                        Some(r) => r == room,
                        None => {
                            let p = format!("/tables/{}/summary", card.table);
                            play_net::call(&d.base, &s.token, "GET", &p, None).await.is_ok_and(|sv| {
                                matches!(record_room(&sv, &card.table, &room), RecordRoom::Here)
                            })
                        }
                    }
                }
            };
            if here {
                finish(with("ok", Some(v)));
            } else {
                eprintln!("[play] card {} {}: the record isn't this room's", card.game, card.kind);
                finish(with("failed", None));
            }
        }
        Err(e) if e.status == 404 => finish(with("gone", None)),
        Err(e) => {
            eprintln!("[play] card {} {}: the record answered {} {}", card.game, card.kind, e.status, e.code);
            finish(with("failed", None))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_parse_and_cover_every_code() {
        let w: Value = serde_json::from_str(WORDS).unwrap();
        // every code of the contract (§6.3 + rev 2's) has a sentence
        for c in [
            "session", "guest", "not-member", "not-in-call", "remote-refused", "rig-hidden", "no-table", "no-wallet",
            "seated-elsewhere", "table-full", "one-per-room", "seated", "open-money", "rate", "off", "db", "stale",
            "not-your-turn", "phase", "bad", "bad-action", "bad-amount", "no-chips", "limit", "locked", "window", "taken",
            "rules", "greyed", "host-only", "no-space", "wrong-server", "unavailable", "space-changed", "owner-unsupported",
            "owner-unavailable",
        ] {
            assert!(w["codes"][c].as_str().is_some_and(|s| !s.is_empty()), "no words for code {c}");
        }
        assert_eq!(sentence_for("made-up"), w["codes"]["other"].as_str().unwrap());
        // every Rig card has its words, and only those
        let cards = w["rig"]["cards"].as_object().unwrap();
        assert_eq!(cards.len(), 16);
        for id in play_wire::RIG_CARDS {
            assert!(cards[id]["name"].is_string() && cards[id]["rule"].is_string(), "{id}");
        }
        // the spec's Lite deck = the 10 ★ cards
        assert_eq!(cards.values().filter(|c| c["lite"] == json!(true)).count(), 10);
        // the visible bodies of chat cards carry no amounts
        for k in ["neutral", "loading", "gone", "failed"] {
            assert!(!w["card"][k].as_str().unwrap().chars().any(|c| c.is_ascii_digit()), "{k}");
        }
        // every words key the chat card reads (GameChatCard.qml: cw.<key>) exists
        let qml = include_str!("../qml/GameChatCard.qml");
        let mut keys: Vec<&str> = qml.match_indices("cw.").map(|(i, _)| {
            let rest = &qml[i + 3..];
            &rest[..rest.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(rest.len())]
        }).filter(|k| !k.is_empty()).collect();
        keys.sort();
        keys.dedup();
        assert!(keys.len() >= 10, "the scan found too few card keys: {keys:?}");
        // the Rig's table end (spec line 687): the host's New table + the shared Close / Back to Games words
        for (g, k) in [("rig", "new_table"), ("table", "close"), ("lobby", "closed_back"), ("card", "join_call")] {
            assert!(w[g][k].as_str().is_some_and(|s| !s.is_empty()), "{g}.{k} missing in words.json");
        }
        for k in keys {
            assert!(w["card"][k].as_str().is_some_and(|s| !s.is_empty()), "GameChatCard.qml reads card.{k}: not in words.json");
        }
    }

    #[test]
    fn discovery_reads_play_strictly() {
        let wk = |f: Value| json!({"dev.tinhat.server": {"api": "https://lab.test/api", "features": f}});
        let d = discovered_from(&wk(json!({"play": true})), "lab.test");
        assert_eq!(d.base, "https://lab.test/api/play/v1");
        assert_eq!(d.flags, json!({"play": true, "phone_app": false, "rig_phone_apps": false}));
        for v in [json!("true"), json!(1), json!(null)] {
            assert!(discovered_from(&wk(json!({"play": v})), "lab.test").base.is_empty(), "play {v}");
        }
        assert!(discovered_from(&wk(json!({})), "lab.test").base.is_empty(), "no flag = no games (older servers)");
        assert!(discovered_from(&Value::Null, "lab.test").base.is_empty());
        // an api on a third host is not followed
        let other = json!({"dev.tinhat.server": {"api": "https://evil.example/api", "features": {"play": true}}});
        assert_eq!(discovered_from(&other, "lab.test").base, "https://lab.test/api/play/v1");
        let rig = discovered_from(&wk(json!({"play": true, "play_rig_phone_apps": true})), "lab.test");
        assert_eq!(rig.flags["rig_phone_apps"], json!(true));
    }

    #[test]
    fn server_names_we_ask() {
        assert!(server_name_ok("chat.example.org") && server_name_ok("example.org:8448") && server_name_ok("[::1]:8448"));
        for bad in ["", "evil.example/x", "a@b", "x?y", "a b", "a#b"] {
            assert!(!server_name_ok(bad), "{bad}");
        }
    }

    #[test]
    fn hidden_games_leave_the_list() {
        let tables = json!([{"table": "tA", "game": "holdem"}, {"table": "tB", "game": "rig"}, {"table": "t/C", "game": "holdem"}, {"table": "tD", "game": "bingo"}]);
        let on = json!({"play": true, "phone_app": false});
        assert_eq!(visible_tables(&tables, &on).as_array().unwrap().len(), 2);
        let phone = json!({"play": true, "phone_app": true});
        assert_eq!(visible_tables(&tables, &phone), json!([{"table": "tA", "game": "holdem"}]));
        assert_eq!(visible_tables(&tables, &json!({})), json!([]));
    }

    /// The prestige hand-off: ranks and the session go only to the owners whose own well-known
    /// named that play base, never to another server's.
    #[test]
    fn prestige_goes_to_the_base_owner_only() {
        let mut e = Engine::default();
        let d = |b: &str| (Instant::now(), Discovered { base: b.into(), flags: json!({}), reachable: true });
        e.discovery.insert("a.example.org".into(), d("https://a.example.org/api/play/v1"));
        e.discovery.insert("b.example.org".into(), d("https://b.example.org/api/play/v1"));
        e.discovery.insert("nogames.example.org".into(), d(""));
        assert_eq!(owners_of(&e, "https://a.example.org/api/play/v1"), vec!["a.example.org".to_string()]);
        assert!(owners_of(&e, "https://c.example.org/api/play/v1").is_empty());
        assert!(owners_of(&e, "").is_empty(), "an empty base names nobody (servers without games)");
    }

    /// An invite card outside my call: the room's listing answers its row, or closed.
    #[test]
    fn invite_cards_read_the_rooms_listing() {
        let listing = json!({"space": "!s:example.org", "owner": "example.org", "tables": [
            {"table": "tA", "game": "holdem", "status": "hand", "host": "@ana:example.org", "seated": 3},
            {"table": "tB", "game": "rig", "status": "closed", "host": "@ben:example.org", "seated": 0}]});
        assert_eq!(invite_row(&listing, "tA", "!r:example.org")["seated"], 3);
        assert_eq!(invite_row(&listing, "tB", "!r:example.org"), json!({"closed": true}), "a closed table is no live row");
        assert_eq!(invite_row(&listing, "tZ", "!r:example.org"), json!({"closed": true}), "an unlisted table is closed");
        assert_eq!(invite_row(&json!({"tables": {"0": {}}}), "tA", "!r"), json!({"closed": true}), "a malformed listing");
    }

    /// The Rig's New table (spec line 687): the fresh table keeps the ended one's options, and only those (the contract
    /// view's own opts: rig/11-end.json).
    #[test]
    fn new_table_keeps_the_options() {
        let end: Value = serde_json::from_str(include_str!("testvectors/games/views/rig/11-end.json")).unwrap();
        let v = &end["views"]["ana"];
        assert_eq!(fresh_opts(v), json!({"preset": "full", "bankroll": 6, "patch": true}));
        assert_eq!(fresh_opts(&json!({"game": "rig", "opts": {"preset": "lite", "seed": "x"}})), json!({"preset": "lite"}));
        assert_eq!(fresh_opts(&json!({"game": "holdem", "opts": {"seats": 6, "sb": 1, "bb": 2, "x": 1}})), json!({"seats": 6, "sb": 1, "bb": 2}));
    }

    /// The lab finding (2026-11-19): a Rig result card stayed "Couldn't load" because the contract's `round` record
    /// names no room; it names its table, and the table's room decides.
    #[test]
    fn card_records_name_their_room_or_table() {
        let room = "!r:example.org";
        assert_eq!(record_room(&json!({"room": room, "table": "tA"}), "tA", room), RecordRoom::Here);
        assert_eq!(record_room(&json!({"room": "!other:example.org", "table": "tA"}), "tA", room), RecordRoom::Elsewhere);
        assert_eq!(record_room(&json!({"room": room, "table": "tB"}), "tA", room), RecordRoom::Elsewhere);
        assert_eq!(record_room(&json!({"table": "tA", "r": 1}), "tA", room), RecordRoom::AskTable);
        assert_eq!(record_room(&json!({"table": "tB", "r": 1}), "tA", room), RecordRoom::Elsewhere);
        assert_eq!(record_room(&json!({}), "tA", room), RecordRoom::Elsewhere);
    }

    /// Runs one recorded line through the app's own checks; returns what it was (for the counts) or why it failed.
    fn recorded_line(line: &str, words: &Value) -> Result<&'static str, String> {
        let d: Value = serde_json::from_str(line).map_err(|e| e.to_string())?;
        let (kind, what, body) = (d["kind"].as_str().unwrap_or(""), d["what"].as_str().unwrap_or(""), d["body"].as_str().unwrap_or(""));
        let on = json!({"play": true, "phone_app": false});
        match kind {
            "frame" => {
                let f = play_wire::frame_ok(body).ok_or(format!("{what} frame refused by frame_ok"))?;
                if !play_wire::server_frame_ok(&f) {
                    return Err(format!("{what} frame refused by server_frame_ok"));
                }
                match what {
                    "view" => {
                        play_wire::check_view(&f["view"])?;
                        Ok("view")
                    }
                    "room" => {
                        let list = visible_tables(&f["tables"], &on);
                        let list = list.as_array().cloned().unwrap_or_default();
                        // a chip exactly for the chip statuses (an ended table is listed, chipless)
                        let wrong = list.iter().any(|t| {
                            let chips = t["status"].as_str().is_some_and(|s| play_wire::CHIP_STATUSES.contains(&s));
                            play_wire::chip_label(words, t).is_empty() == chips
                        });
                        if list.is_empty() || wrong {
                            return Err("a room frame's tables got the wrong chips".into());
                        }
                        Ok("room")
                    }
                    _ => Ok("frame"),
                }
            }
            "http" => {
                let v: Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
                if what.starts_with("GET /me") {
                    // the header chip, the wallet line and the opt-in read these
                    for k in ["balance", "cap", "rank", "day_max", "earned_today", "holdings"] {
                        if !v[k].is_u64() {
                            return Err(format!("/me {k} is not a whole number"));
                        }
                    }
                    if !(v["topup"].is_object() && v["topup"]["available"].is_boolean()) {
                        return Err("/me topup.available is not a boolean".into());
                    }
                    return Ok("me");
                }
                if what.starts_with("GET /rooms/") {
                    // the room's game space confirmation
                    if !(v["server"].is_string() && v["space"].as_str().is_some_and(|s| s.starts_with('!'))) {
                        return Err("/rooms/{room}/space has no server + space".into());
                    }
                    return Ok("space");
                }
                // a chat card's record, as load_card judges it (record_room) + what the card draws
                let table = what.split('/').nth(2).unwrap_or("");
                let room = "!lmnyiDDdZWzVMvZdJk:lab.example.org";
                if what.ends_with("/summary") {
                    if record_room(&v, table, room) != RecordRoom::Here {
                        return Err("a summary that isn't this room's".into());
                    }
                    if !(v["hands"].is_u64() || v["rounds"].is_u64()) || !v["net"].is_object() || !v["leaderboard"].is_array() {
                        return Err("a summary without hands/rounds, net or leaderboard".into());
                    }
                    return Ok("summary");
                }
                if what.contains("/rounds/") {
                    if record_room(&v, table, room) != RecordRoom::AskTable || !v["r"].is_u64() || !v["reveal"].is_object() {
                        return Err("a round record without its table, r or reveal".into());
                    }
                    return Ok("round");
                }
                Err(format!("an unexpected recorded answer {what}"))
            }
            _ => Err(format!("unknown kind {kind}")),
        }
    }

    /// Launch-gate 2026-11-19 13:23 (real-shaped fixtures): what a REAL lab server sent this app (TINHAT_PLAY_RECORD,
    /// names neutralised, testvectors/games/recorded/) goes through the same checks the app runs on it.
    #[test]
    fn recorded_lab_shapes() {
        let words: Value = serde_json::from_str(WORDS).unwrap();
        let mut seen = std::collections::BTreeMap::new();
        for line in include_str!("testvectors/games/recorded/lab.jsonl").lines().filter(|l| !l.trim().is_empty()) {
            let k = recorded_line(line, &words).unwrap_or_else(|e| panic!("{e}: {}", &line[..line.len().min(160)]));
            *seen.entry(k).or_insert(0) += 1;
        }
        for k in ["view", "room", "me", "space", "summary", "round"] {
            assert!(seen.get(k).is_some_and(|n| *n > 0), "nothing recorded of kind {k}: {seen:?}");
        }
    }

    /// The control: the same lines with one shape changed the way a server regression would (a list sent as a map, a
    /// number sent as a string) are refused, so the recorded test can fail.
    #[test]
    fn recorded_lab_shapes_control() {
        let words: Value = serde_json::from_str(WORDS).unwrap();
        let rec = include_str!("testvectors/games/recorded/lab.jsonl");
        let mut broke = 0;
        for line in rec.lines().filter(|l| !l.trim().is_empty()) {
            let mut d: Value = serde_json::from_str(line).unwrap();
            let mut body: Value = match serde_json::from_str(d["body"].as_str().unwrap()) {
                Ok(b) => b,
                Err(_) => continue,
            };
            match (d["kind"].as_str().unwrap(), d["what"].as_str().unwrap()) {
                ("frame", "view") => {
                    let seats = if body["view"]["game"] == "holdem" { "seats" } else { "players" };
                    let list = body["view"][seats].take();
                    let map: serde_json::Map<String, Value> =
                        list.as_array().into_iter().flatten().enumerate().map(|(i, x)| (i.to_string(), x.clone())).collect();
                    body["view"][seats] = Value::Object(map);
                }
                ("frame", "room") => {
                    let list = body["tables"].take();
                    body["tables"] = json!({"0": list});
                }
                ("http", w) if w.starts_with("GET /me") => body["balance"] = json!(body["balance"].to_string()),
                ("http", w) if w.ends_with("/summary") => {
                    body.as_object_mut().unwrap().remove("room");
                }
                ("http", w) if w.contains("/rounds/") => body["table"] = json!("tSomeOtherTable1"),
                _ => continue,
            }
            d["body"] = json!(body.to_string());
            assert!(recorded_line(&d.to_string(), &words).is_err(), "a changed shape passed: {}", &line[..line.len().min(120)]);
            broke += 1;
        }
        assert!(broke >= 6, "the control changed only {broke} lines");
    }

    /// Every "is this room my call?" comparison of `mCallRoomId` on one QML line (`x.mCallRoomId === r`, `r === x.mCallRoomId`,
    /// `!==` / `==` alike), as "file:line". Pure, so the control below can plant one.
    fn call_room_checks_without_in_call(lines: &[(String, String)]) -> Vec<String> {
        let mut bad = Vec::new();
        for (at, l) in lines {
            if !at.contains(".qml:") || l.contains("mInCall") {
                continue;
            }
            let mut from = 0;
            while let Some(i) = l[from..].find("mCallRoomId").map(|i| i + from) {
                let after = l[i + "mCallRoomId".len()..].trim_start();
                let before = l[..i].trim_end_matches(|c: char| c.is_alphanumeric() || c == '_' || c == '.').trim_end();
                let cmp = |t: &str| t.starts_with("==") || t.starts_with("!=");
                if cmp(after) || before.ends_with("==") || before.ends_with("!=") {
                    bad.push(at.clone());
                    break;
                }
                from = i + 1;
            }
        }
        bad
    }

    /// #27 (2026-11-20): `mCallRoomId` keeps the last call's room after a hang-up (AppCore sets it on join and never
    /// clears it), so "this room is my call" must also read `mInCall`. GameChatCard's `inCall` didn't: after a hang-up
    /// every invite card stayed on the in-call path, read the emptied call listing and sat at "Loading the game…" for
    /// the rest of the session, never offering "Join the call" (seen on the 12.2.0 Flatpak LAB).
    #[test]
    fn a_room_is_my_call_only_while_the_call_is_live() {
        let bad = call_room_checks_without_in_call(&crate::wording::code_lines());
        assert!(bad.is_empty(), "\"is this room my call\" without mInCall (mCallRoomId outlives the call): {bad:?}");
        let card = include_str!("../qml/GameChatCard.qml");
        let line = card.lines().find(|l| l.contains("readonly property bool inCall:")).expect("GameChatCard inCall");
        assert!(line.contains("mInCall === true") && line.contains("mCallRoomId === gameCardRoot.roomId"), "{line}");
    }

    /// The control: the scan finds each comparison shape when planted, and leaves guarded or non-comparing uses alone.
    #[test]
    fn a_room_is_my_call_only_while_the_call_is_live_control() {
        let l = |s: &str| ("qml/X.qml:1".to_string(), s.to_string());
        for planted in ["ok: app.mCallRoomId === roomId", "ok: roomId === root.mCallRoomId", "ok: a.mCallRoomId !== r", "ok: core.mCallRoomId == r"] {
            assert_eq!(call_room_checks_without_in_call(&[l(planted)]), vec!["qml/X.qml:1".to_string()], "{planted}");
        }
        for fine in ["ok: app.mInCall && app.mCallRoomId === r", r#"roomId: String(app.mCallRoomId || "")"#, "x: mInCall ? mCallRoomId : \"\""] {
            assert!(call_room_checks_without_in_call(&[l(fine)]).is_empty(), "{fine}");
        }
        assert!(call_room_checks_without_in_call(&[("src/x.rs:1".into(), "a.mCallRoomId === r".into())]).is_empty());
    }
}
