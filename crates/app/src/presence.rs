//! PresenceController — opt-in presence (Online / Away / In a call) and the desktop half of
//! "don't buzz the phone while you're at a computer" (10.6, docs/REACH.md §2, §3.4, A1, A2).
//!
//! Nothing is sent about you unless the account says `dev.tinhat.presence.share == true`, and
//! that flag is written ONLY by you: the Settings switch, the footer popover, or "Turn on" on the
//! one-time card (A2.2). The rules live in `presence_model.rs` (pure, unit-tested); this file is
//! the engine that feeds them facts and talks to the user's server's API (`proxy::base()`):
//!
//!   • beats    `PUT /api/presence` {state, call, client:"desktop"} at once, every 30 s and 2 s after
//!              a change; a 409 `not_sharing` stops them until the account data says share:true again.
//!   • off      share:false is written FIRST, then `DELETE /api/presence` until it answers 204 (the
//!              switch says "Turning off…" meanwhile). Sign-out: `DELETE ?device=this` first.
//!   • reader   `GET /api/presence` every 20 s while the window is shown, 60 s otherwise, at once on
//!              focus — always, sharing or not (non-reciprocal); an error keeps the map ≤ 60 s.
//!   • activity §3.4 `POST /api/push/active` {pushkey:null, visible, instance} every 15 s while the
//!              computer is in use, one visible:false on the way out — only with consent
//!              (`GET /api/push/quiet-wanted`), a healthy sync and desktop notifications on (A2.12).
//!   • idle     every 15 s on a tokio worker (never the GUI thread), each D-Bus call bounded at 1 s:
//!              freedesktop ScreenSaver.GetSessionIdleTime (KDE answers in ms) → Mutter IdleMonitor →
//!              Tin Hat's own input clock (video_bridge.cpp); Windows: GetLastInputInfo. Lock state:
//!              ScreenSaver.GetActive (Linux only — A2.13).
//!
//! Never in the recorder or a headless harness: the engine starts only when the QML controller
//! calls `start()`, and refuses under `TINHAT_VOD_SERVICE`. Logs carry counts and our OWN state
//! transitions only — never anybody else's status (the shipped logs reach the server admin).

use core::pin::Pin;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use cxx_qt::{CxxQtThread, Threading};
use cxx_qt_lib::QString;
use matrix_sdk::ruma::events::{AnyGlobalAccountDataEvent, GlobalAccountDataEventType};
use matrix_sdk::ruma::serde::Raw;
use matrix_sdk::Client;
use serde_json::Value;

use crate::presence_model::{self as model, IdleSample, Ping, ShareChange};

/// Every API round trip is bounded.
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);

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
        #[qproperty(bool, known)] //         QML: known — the account's preference has been read (after the first sync)
        #[qproperty(bool, sharing)] //       QML: sharing — dev.tinhat.presence share === true (and not being turned off)
        #[qproperty(bool, asked)] //         QML: asked — the one-time question was answered (dev.tinhat.presence_asked)
        #[qproperty(bool, turning_off)] //   QML: turningOff — share:false written / being written, DELETE not yet 204
        #[qproperty(bool, busy)] //          QML: busy — a preference write is in flight
        #[qproperty(QString, my_state)] //   QML: myState — what others see: "online" | "away" | "call" | "" (A2.15)
        #[qproperty(QString, my_room)] //    QML: myRoom — the call room in our own entry ("" unless in a call)
        #[qproperty(QString, users_json)] // QML: usersJson — {"@a:hs": {"state": "online"|"away"|"call", "room"?: "!r"}}
        #[qproperty(i32, rev)] //            QML: rev — bumps whenever usersJson / myState / myRoom change
        #[qproperty(QString, my_status_json)] // QML: myStatusJson — A4: my own custom status as set ({id,text,emoji,color,until}) or "" (the picker)
        #[qproperty(QString, presets_json)] //   QML: presetsJson — A4.2 presets [{id,emoji,text,color,hex}] (constant)
        #[qproperty(QString, palette_json)] //   QML: paletteJson — A4.3 palette [{key,hex}] (constant)
        #[qproperty(QString, last_error)] // QML: lastError — a failed preference write, user-facing
        #[qproperty(i32, error_rev)] //      QML: errorRev — bumps with every lastError
        #[qproperty(QString, zones_json)] // QML: zonesJson — A7: {"@a:hs": "America/New_York"} (only while a Friends' clocks widget wants it)
        #[qproperty(QString, tz_json)] //    QML: tzJson — A7: {known, share (null = absent), tz, auto, device, busy, error}
        type PresenceController = super::PresenceControllerRust;

        /// QML: start() — attach this controller to the engine and start the engine (once per
        /// process; never in the recorder). Idempotent.
        #[qinvokable]
        fn start(self: Pin<&mut PresenceController>);
        /// QML: chooseSharing(on) — the switch (Settings → Privacy, the footer popover): a person's
        /// explicit choice, the ONLY way `share` is ever written (A2.2).
        #[qinvokable]
        fn choose_sharing(self: Pin<&mut PresenceController>, on: bool);
        /// QML: answerAsk(turnOn) — the one-time card: [Turn on] (true) or [No thanks] (false).
        #[qinvokable]
        fn answer_ask(self: Pin<&mut PresenceController>, turn_on: bool);
        /// QML: setCallRoom(roomId) — the room of the Tin Hat call we are connected to ("" = none).
        #[qinvokable]
        fn set_call_room(self: Pin<&mut PresenceController>, room_id: &QString);
        /// QML: setWindowState(visible, focused) — the main window is shown / has focus (§2.5 cadence).
        #[qinvokable]
        fn set_window_state(self: Pin<&mut PresenceController>, visible: bool, focused: bool);
        /// QML: setStatus(id, text, emoji, color, untilMs, turnOnSharing) — A4.6: a preset or a custom
        /// status (untilMs 0 = don't clear). With turnOnSharing (the "This turns on sharing" answer),
        /// `share: true` is written first — the user's explicit choice — then the status.
        #[qinvokable]
        fn set_status(
            self: Pin<&mut PresenceController>,
            id: &QString,
            text: &QString,
            emoji: &QString,
            color: &QString,
            until_ms: f64,
            turn_on_sharing: bool,
        );
        /// QML: clearStatus() — A4.6 "Clear status" (writes `{}`).
        #[qinvokable]
        fn clear_status(self: Pin<&mut PresenceController>);
        /// QML: wantZones(on) — A7: a Friends' clocks widget is shown (on) / gone (off); counted. While any wants
        /// them, the reader asks `GET /presence?tz=1` and zonesJson carries the zones.
        #[qinvokable]
        fn want_zones(self: Pin<&mut PresenceController>, on: bool);
        /// QML: tzShare(on) — A7: "Share my time zone" (Settings → Privacy, the widget's "Share again"). Off writes
        /// share:false first, then `DELETE /presence?what=tz`.
        #[qinvokable]
        fn tz_share(self: Pin<&mut PresenceController>, on: bool);
        /// QML: tzPick(zone) — A7: the zone picker; "" = Automatic (this device's zone, kept in step), else by hand.
        #[qinvokable]
        fn tz_pick(self: Pin<&mut PresenceController>, zone: &QString);
        /// QML: tzChoices() — A7: the picker's zones as [{tz, text}] ("JST · Asia/Tokyo"; the shared list).
        #[qinvokable]
        fn tz_choices(self: &PresenceController) -> QString;
        /// QML: tzChoiceText(tz) — "JST · Asia/Tokyo" for any zone (the Automatic entry names the device's).
        #[qinvokable]
        fn tz_choice_text(self: &PresenceController, tz: &QString) -> QString;
    }

    impl cxx_qt::Threading for PresenceController {}
}

pub struct PresenceControllerRust {
    known: bool,
    sharing: bool,
    asked: bool,
    turning_off: bool,
    busy: bool,
    my_state: QString,
    my_room: QString,
    users_json: QString,
    rev: i32,
    my_status_json: QString,
    presets_json: QString,
    palette_json: QString,
    last_error: QString,
    error_rev: i32,
    zones_json: QString,
    tz_json: QString,
}

impl Default for PresenceControllerRust {
    fn default() -> Self {
        let presets: Vec<Value> = model::PRESETS
            .iter()
            .map(|(id, emoji, text, color)| {
                serde_json::json!({ "id": id, "emoji": emoji, "text": text, "color": color, "hex": model::status_color(color) })
            })
            .collect();
        let palette: Vec<Value> =
            model::PALETTE.iter().map(|(k, h)| serde_json::json!({ "key": k, "hex": h })).collect();
        Self {
            known: false,
            sharing: false,
            asked: false,
            turning_off: false,
            busy: false,
            my_state: QString::default(),
            my_room: QString::default(),
            users_json: QString::default(),
            rev: 0,
            my_status_json: QString::default(),
            presets_json: QString::from(&Value::Array(presets).to_string()),
            palette_json: QString::from(&Value::Array(palette).to_string()),
            last_error: QString::default(),
            error_rev: 0,
            zones_json: QString::from("{}"),
            tz_json: QString::default(),
        }
    }
}

// ───────────────────────────────────── engine state ─────────────────────────────────────

struct Engine {
    /// Bumps on sign-out: a task that started for the previous session drops its result.
    gen: u64,
    user: String,
    // sync facts
    synced: bool,
    last_sync_ok: Option<Instant>,
    // this device's view of the account data
    share: Option<bool>,
    asked: bool,
    /// A4.1: my custom status as the account data holds it (None / `{}` = none).
    my_status: Option<Value>,
    /// A4.6: the `until` we already wrote `{}` for (the clean-up runs once per status).
    status_cleared_for: Option<u64>,
    /// A4.4: the next beat carries `refresh: true` (right after a share / status change).
    want_refresh: bool,
    /// 409 not_sharing — no beats until the account data says share:true again.
    blocked: bool,
    // the off switch (A2.2)
    flow: u64,
    turning_off: bool,
    stop_local: bool,
    delete_due: Option<Instant>,
    delete_backoff: Option<Duration>,
    delete_in_flight: bool,
    writing: bool,
    // inputs
    call_room: Option<String>,
    visible: bool,
    focused: bool,
    idle: IdleSample,
    probe_due: Option<Instant>,
    probe_in_flight: bool,
    // beats
    beat_due: Option<Instant>,
    last_state: Option<(String, Option<String>)>,
    logged_state: Option<(String, Option<String>)>,
    beat_in_flight: bool,
    beat_failing: bool,
    // reader
    read_due: Option<Instant>,
    read_last_at: Option<Instant>,
    read_last_ok: Option<Instant>,
    read_in_flight: bool,
    read_failing: bool,
    map: BTreeMap<String, model::Seen>,
    /// A7: mxid → zone from the last `?tz=1` read (empty while no Friends' clocks widget wants it)
    zones: BTreeMap<String, String>,
    // A7: the time zone's account data (`dev.tinhat.presence_tz`) as this device knows it
    tz_known: bool,
    /// the stored content (None = absent)
    tz_pref: Option<Value>,
    tz_busy: bool,
    tz_inflight: bool,
    tz_error: String,
    // §3.4
    instance: String,
    quiet_wanted: bool,
    quiet_due: Option<Instant>,
    quiet_in_flight: bool,
    active_sent: bool,
    ping_last_active: Option<Instant>,
    ping_in_flight: bool,
    ping_failing: bool,
    // a failed preference write, for the next publish
    error: Option<String>,
    /// The newest explicit choice (the switch / the card) not yet confirmed on the server. ONE
    /// writer task writes the newest choice at a time, so an older write can never land last.
    desired_share: Option<bool>,
    consent_writer: bool,
    /// The newest status pick (`{}` = clear) not yet written — one writer, newest wins.
    desired_status: Option<Value>,
    status_writer: bool,
    /// A clean quit is saying goodbye: nothing new is sent from here on.
    stopping: bool,
}

impl Engine {
    fn fresh(gen: u64, instance: String, visible: bool, focused: bool, idle: IdleSample) -> Self {
        Engine {
            gen,
            user: String::new(),
            synced: false,
            last_sync_ok: None,
            share: None,
            asked: false,
            my_status: None,
            status_cleared_for: None,
            want_refresh: false,
            blocked: false,
            flow: 0,
            turning_off: false,
            stop_local: false,
            delete_due: None,
            delete_backoff: None,
            delete_in_flight: false,
            writing: false,
            call_room: None,
            visible,
            focused,
            idle,
            probe_due: None,
            probe_in_flight: false,
            beat_due: None,
            last_state: None,
            logged_state: None,
            beat_in_flight: false,
            beat_failing: false,
            read_due: None,
            read_last_at: None,
            read_last_ok: None,
            read_in_flight: false,
            read_failing: false,
            map: BTreeMap::new(),
            zones: BTreeMap::new(),
            tz_known: false,
            tz_pref: None,
            tz_busy: false,
            tz_inflight: false,
            tz_error: String::new(),
            instance,
            quiet_wanted: false,
            quiet_due: None,
            quiet_in_flight: false,
            active_sent: false,
            ping_last_active: None,
            ping_in_flight: false,
            ping_failing: false,
            error: None,
            desired_share: None,
            consent_writer: false,
            desired_status: None,
            status_writer: false,
            stopping: false,
        }
    }
}

/// `instance` (A2.12): a random id per app start, so a `visible:false` clears only this copy.
fn new_instance() -> String {
    use rand::RngCore;
    let mut b = [0u8; 8];
    rand::thread_rng().fill_bytes(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn engine() -> &'static Mutex<Engine> {
    static E: OnceLock<Mutex<Engine>> = OnceLock::new();
    E.get_or_init(|| Mutex::new(Engine::fresh(1, new_instance(), true, true, IdleSample::default())))
}

fn lock() -> std::sync::MutexGuard<'static, Engine> {
    engine().lock().unwrap_or_else(|p| p.into_inner())
}

fn sink() -> &'static Mutex<Option<CxxQtThread<qobject::PresenceController>>> {
    static S: OnceLock<Mutex<Option<CxxQtThread<qobject::PresenceController>>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(None))
}

fn wake() -> &'static tokio::sync::Notify {
    static W: OnceLock<tokio::sync::Notify> = OnceLock::new();
    W.get_or_init(tokio::sync::Notify::new)
}

static STARTED: AtomicBool = AtomicBool::new(false);

/// Beats and activity pings on the wire right now (any session). A goodbye (sign-out, quit)
/// waits for them first, so a late PUT can never recreate the entry the DELETE just removed.
static INFLIGHT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

struct Inflight;
impl Inflight {
    fn new() -> Self {
        INFLIGHT.fetch_add(1, Ordering::SeqCst);
        Inflight
    }
}
impl Drop for Inflight {
    fn drop(&mut self) {
        INFLIGHT.fetch_sub(1, Ordering::SeqCst);
    }
}

/// 10.6.0 review: `dev.tinhat.presence` writes on the wire (the consent writer). A goodbye waits for
/// them too, so an older write can't land after the goodbye's own share:false.
static CONSENT_INFLIGHT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

struct ConsentInflight;
impl ConsentInflight {
    fn new() -> Self {
        CONSENT_INFLIGHT.fetch_add(1, Ordering::SeqCst);
        ConsentInflight
    }
}
impl Drop for ConsentInflight {
    fn drop(&mut self) {
        CONSENT_INFLIGHT.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Wait (bounded) until no beat / ping / consent write is on the wire. True when it emptied (false =
/// the deadline passed with something still out there).
async fn drain(max: Duration) -> bool {
    let until = Instant::now() + max;
    loop {
        if INFLIGHT.load(Ordering::SeqCst) == 0 && CONSENT_INFLIGHT.load(Ordering::SeqCst) == 0 {
            return true;
        }
        if Instant::now() >= until {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// `TINHAT_PRESENCE_DEBUG=1`: log every request + answer (own requests only, never others' states).
fn debug() -> bool {
    static D: OnceLock<bool> = OnceLock::new();
    *D.get_or_init(|| std::env::var("TINHAT_PRESENCE_DEBUG").is_ok_and(|v| v == "1"))
}

fn spawn<F: std::future::Future<Output = ()> + Send + 'static>(f: F) {
    crate::bridge::runtime().spawn(f);
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// A4.1: my status for the picker — the live one, as `{id,text,emoji,color,until}`, else "".
fn my_status_for_qml(v: Option<&Value>) -> String {
    match model::live_status(v, now_ms()) {
        Some(s) => serde_json::json!({
            "id": s.id, "text": s.text, "emoji": s.emoji, "color": s.color, "until": s.until,
        })
        .to_string(),
        None => String::new(),
    }
}

// ───────────────────────────────────── the API ─────────────────────────────────────

/// One bearer request to `{proxy::base()}{path}` → (status, JSON body or Null).
async fn api(method: &str, path: &str, token: &str, body: Option<Value>) -> Result<(u16, Value), String> {
    // selfhost: a server without Reach (presence: false) is never asked — the same answer as a server without the API
    if !crate::server_offer::feature_on_global(crate::server_offer::ServerFeature::Presence) {
        return Ok((404, Value::Null));
    }
    let url = crate::proxy::url(path)?;
    let c = crate::proxy::http(Some(HTTP_TIMEOUT));
    let mut req = match method {
        "PUT" => c.put(&url),
        "POST" => c.post(&url),
        "DELETE" => c.delete(&url),
        _ => c.get(&url),
    };
    req = req.header("Authorization", format!("Bearer {token}"));
    if let Some(b) = body {
        req = req.header("Content-Type", "application/json").body(b.to_string());
    }
    let resp = req.send().await.map_err(|e| if e.is_timeout() { "timeout".to_string() } else { e.to_string() })?;
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    Ok((status, serde_json::from_str(&text).unwrap_or(Value::Null)))
}

// ── 10.6.0 review (Astra #3): an off that never reached the server survives quit / sign-out ──
// The switch's OFF is the person's explicit choice (A2.2). It is written ahead to
// `<data>/presence-pending-off.json` (a list of mxids) when they choose it, and cleared when a share:false
// write lands or they choose ON again. If a session ends before the write landed (offline, homeserver
// down), that account's next start finishes the off before anything is sent. Only ever an OFF.
fn pending_off_file() -> std::path::PathBuf {
    crate::bridge::data_dir().join("presence-pending-off.json")
}

fn marker_list(path: &std::path::Path) -> Vec<String> {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice::<Vec<String>>(&b).ok()).unwrap_or_default()
}

/// Change the note (true = it changed). Callers hold `note_lock()`.
fn marker_update(path: &std::path::Path, user: &str, on: bool) -> bool {
    if user.is_empty() {
        return false;
    }
    let mut v = marker_list(path);
    if v.iter().any(|u| u == user) == on {
        return false;
    }
    if on {
        v.push(user.to_string());
    } else {
        v.retain(|u| u != user);
    }
    if v.is_empty() {
        let _ = std::fs::remove_file(path);
        return true;
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_extension("json.tmp");
    let body = serde_json::to_vec(&v).unwrap_or_default();
    match std::fs::write(&tmp, body).and_then(|_| std::fs::rename(&tmp, path)) {
        Ok(()) => true,
        Err(err) => {
            eprintln!("[presence] could not {} the pending-off note: {err}", if on { "write" } else { "clear" });
            false
        }
    }
}

/// One lock for every note change (Qt thread + workers); lock order: engine -> note, never the reverse.
fn note_lock() -> std::sync::MutexGuard<'static, ()> {
    static L: Mutex<()> = Mutex::new(());
    L.lock().unwrap_or_else(|p| p.into_inner())
}

/// The note's revision: bumps on every change, so a completion can tell whether a newer choice came in.
static NOTE_REV: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn note_set_at(path: &std::path::Path, rev: &std::sync::atomic::AtomicU64, user: &str, on: bool) {
    let _g = note_lock();
    if marker_update(path, user, on) {
        rev.fetch_add(1, Ordering::SeqCst);
    }
}

/// Clear the note because an off LANDED — only if nothing changed it since `seen` (the revision the
/// write or goodbye captured when it started). True when the note is now clear for that reason.
fn note_ack_at(path: &std::path::Path, rev: &std::sync::atomic::AtomicU64, user: &str, seen: u64) -> bool {
    let _g = note_lock();
    if rev.load(Ordering::SeqCst) != seen {
        return false;
    }
    if marker_update(path, user, false) {
        rev.fetch_add(1, Ordering::SeqCst);
    }
    true
}

fn pending_off_set(user: &str, on: bool) {
    note_set_at(&pending_off_file(), &NOTE_REV, user, on);
}

fn pending_off_ack(user: &str, seen: u64) -> bool {
    note_ack_at(&pending_off_file(), &NOTE_REV, user, seen)
}

fn pending_off_rev() -> u64 {
    NOTE_REV.load(Ordering::SeqCst)
}

fn pending_off_has(user: &str) -> bool {
    let _g = note_lock();
    marker_list(&pending_off_file()).iter().any(|u| u == user)
}

/// At an account's first sync: an off it chose in an earlier session that never landed is finished now
/// (through the consent writer: share:false, then the DELETE). Returns (start the writer, clear the note).
fn reconcile_pending_off(e: &mut Engine, pending: bool, share: bool) -> (bool, bool) {
    if e.desired_share.is_some() {
        return (false, false); // a choice made in this session already: it is the newest, it wins
    }
    match (pending, share) {
        (true, true) => {
            eprintln!("[presence] finishing an off chosen before the last quit / sign-out");
            (request_consent(e, false), false)
        }
        (true, false) => (false, true), // it landed after all (or another device turned it off)
        _ => (false, false),
    }
}

/// A bounded account-data write's outcome, for the goodbye log lines.
fn written(r: Result<Result<(), String>, tokio::time::error::Elapsed>) -> String {
    match r {
        Ok(Ok(())) => "written".into(),
        Ok(Err(e)) => format!("error: {e}"),
        Err(_) => "timeout".into(),
    }
}

fn status_of(r: &Result<(u16, Value), String>) -> String {
    match r {
        Ok((s, _)) => s.to_string(),
        Err(e) => format!("error: {e}"),
    }
}

async fn write_account_data(client: &Client, ty: &str, content: Value) -> Result<(), String> {
    let raw: Raw<matrix_sdk::ruma::events::AnyGlobalAccountDataEventContent> =
        Raw::new(&content).map_err(|e| e.to_string())?.cast_unchecked();
    client
        .account()
        .set_account_data_raw(GlobalAccountDataEventType::from(ty), raw)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}

async fn read_account_data(client: &Client, ty: &str) -> Option<Value> {
    client
        .account()
        .account_data_raw(GlobalAccountDataEventType::from(ty))
        .await
        .ok()
        .flatten()
        .and_then(|raw| raw.deserialize_as_unchecked::<Value>().ok())
}

// ───────────────────────────────────── publishing ─────────────────────────────────────

/// Push the engine's view to the QML controller (only what changed; `rev` bumps with the map).
fn publish() {
    let Some(qt) = sink().lock().ok().and_then(|s| s.clone()) else { return };
    let (known, sharing, asked, turning_off, busy, my_state, my_room, users, my_status, error, zones, tz) = {
        let mut e = lock();
        let (st, room) = model::my_view(&e.map, &e.user);
        let zones = serde_json::to_string(&e.zones).unwrap_or_else(|_| "{}".into());
        let tz = tz_view(&e);
        (
            e.synced,
            e.share == Some(true) && !e.stop_local,
            e.asked,
            e.turning_off,
            e.writing,
            st,
            room,
            model::users_json(&e.map, now_ms()),
            my_status_for_qml(e.my_status.as_ref()),
            e.error.take(),
            zones,
            tz,
        )
    };
    let _ = qt.queue(move |mut o| {
        if *o.known() != known {
            o.as_mut().set_known(known);
        }
        if *o.sharing() != sharing {
            o.as_mut().set_sharing(sharing);
        }
        if *o.asked() != asked {
            o.as_mut().set_asked(asked);
        }
        if *o.turning_off() != turning_off {
            o.as_mut().set_turning_off(turning_off);
        }
        if *o.busy() != busy {
            o.as_mut().set_busy(busy);
        }
        let changed = o.users_json().to_string() != users
            || o.my_state().to_string() != my_state
            || o.my_room().to_string() != my_room;
        if changed {
            o.as_mut().set_users_json(QString::from(&users));
            o.as_mut().set_my_state(QString::from(&my_state));
            o.as_mut().set_my_room(QString::from(&my_room));
            let r = *o.rev() + 1;
            o.as_mut().set_rev(r);
        }
        if o.my_status_json().to_string() != my_status {
            o.as_mut().set_my_status_json(QString::from(&my_status));
        }
        if let Some(err) = error {
            o.as_mut().set_last_error(QString::from(&err));
            let r = *o.error_rev() + 1;
            o.as_mut().set_error_rev(r);
        }
        if o.zones_json().to_string() != zones {
            o.as_mut().set_zones_json(QString::from(&zones));
        }
        if o.tz_json().to_string() != tz {
            o.as_mut().set_tz_json(QString::from(&tz));
        }
    });
}

// ───────────────────────────────────── A7: the time zone (Friends' clocks) ─────────────────────────────────────
// 2026-09-28: the time zone is not secret, but sharing it can be opted out of. The global account data
// `dev.tinhat.presence_tz` = {share, tz, auto, v: 1}, opt-OUT: after the first sync, an absent key is written by this
// device itself (sharing on, the device's zone, automatic); while `auto` the zone follows the device (start / focus,
// at most one automatic write per device per 10 minutes, only on a real change — with two devices in two zones the
// one used last wins). A pick by hand writes auto: false. Off writes share: false first, then DELETE ?what=tz. The rules
// are the shared ones (tinhat_widgets::clocks, the web's src/widgets/clocks.ts; runtime twin: the web's presence/tz.ts).

pub const TZ_TYPE: &str = tinhat_widgets::clocks::TZ_TYPE;
static ZONES_WANTED: AtomicI32 = AtomicI32::new(0);

fn device_tz_cell() -> &'static Mutex<String> {
    static C: OnceLock<Mutex<String>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(String::new()))
}

/// This device's IANA zone as last read on the Qt thread ("" = unknown / not a zone the server accepts).
pub fn device_tz() -> String {
    device_tz_cell().lock().map(|s| s.clone()).unwrap_or_default()
}

/// Read the system zone (QTimeZone::systemTimeZoneId — Qt thread).
fn refresh_device_tz() {
    let id = cxx_qt_lib::QTimeZone::system_time_zone_id();
    let z = String::from_utf8_lossy(id.as_slice()).to_string();
    let z = if tinhat_widgets::clocks::valid_zone(&z) { z } else { String::new() };
    if let Ok(mut c) = device_tz_cell().lock() {
        *c = z;
    }
}

fn tz_sharing(e: &Engine) -> bool {
    tinhat_widgets::clocks::read_tz_pref(e.tz_pref.as_ref()).is_some_and(|p| p.share)
}

/// tzJson for QML: {known, share (null while absent), tz, auto, device, busy, error}.
fn tz_view(e: &Engine) -> String {
    let p = tinhat_widgets::clocks::read_tz_pref(e.tz_pref.as_ref());
    serde_json::json!({
        "known": e.tz_known,
        "share": p.as_ref().map(|p| Value::Bool(p.share)).unwrap_or(Value::Null),
        "tz": p.as_ref().map(|p| p.tz.clone()).unwrap_or_default(),
        "auto": p.as_ref().map(|p| p.auto).unwrap_or(true),
        "device": device_tz(),
        "busy": e.tz_busy,
        "error": e.tz_error,
    })
    .to_string()
}

pub(crate) fn tz_auto_file() -> std::path::PathBuf {
    crate::bridge::data_dir().join("clocks-tz.json")
}
/// This device's last automatic write for this account (ms since the epoch), kept across restarts.
fn tz_last_auto(me: &str) -> Option<f64> {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(tz_auto_file()).ok()?).ok()?;
    if v.get("me").and_then(Value::as_str) != Some(me) {
        return None;
    }
    v.get("autoAt").and_then(Value::as_f64)
}
fn tz_set_last_auto(me: &str, at: f64) {
    let _ = std::fs::write(tz_auto_file(), serde_json::json!({ "me": me, "autoAt": at }).to_string());
}

/// Start / focus: the first-run write (absent → sharing on) or keeping an automatic zone in step.
async fn tz_check(why: &'static str) {
    let Some(client) = crate::session::client() else { return };
    let me = client.user_id().map(|u| u.to_string()).unwrap_or_default();
    let (stored, gen) = {
        let mut e = lock();
        if !e.tz_known || e.tz_inflight || e.user != me || e.stopping {
            return;
        }
        let dev = device_tz();
        let want = tinhat_widgets::clocks::tz_auto_write(e.tz_pref.as_ref(), &dev, now_ms() as f64, tz_last_auto(&me));
        let Some(want) = want else { return };
        e.tz_inflight = true;
        (want.to_json(), e.gen)
    };
    tz_set_last_auto(&me, now_ms() as f64);
    let r = tokio::time::timeout(Duration::from_secs(20), write_account_data(&client, TZ_TYPE, stored.clone())).await;
    {
        let mut e = lock();
        if e.gen != gen {
            return;
        }
        e.tz_inflight = false;
        match r {
            Ok(Ok(())) => {
                e.tz_pref = Some(stored);
                eprintln!("[presence] time zone written automatically ({why})");
            }
            _ => eprintln!("[presence] automatic time-zone write failed ({why}): {}", written(r)),
        }
    }
    publish();
}

enum TzAction {
    Share(bool),
    Pick(String),
}

/// The switch / the picker — a person's choice. Off: share:false first, then DELETE /presence?what=tz (retried).
async fn tz_write(action: TzAction) {
    let Some(client) = crate::session::client() else { return };
    let dev = device_tz();
    let (content, gen, off) = {
        let mut e = lock();
        let cur = tinhat_widgets::clocks::read_tz_pref(e.tz_pref.as_ref());
        let (share, tz, auto) = match &action {
            TzAction::Share(on) => {
                let auto = cur.as_ref().map_or(true, |p| p.auto);
                let tz = if auto {
                    if !dev.is_empty() { dev.clone() } else { cur.as_ref().map(|p| p.tz.clone()).unwrap_or_default() }
                } else {
                    cur.as_ref().map(|p| p.tz.clone()).filter(|t| !t.is_empty()).unwrap_or_else(|| dev.clone())
                };
                (*on, tz, auto)
            }
            TzAction::Pick(z) => {
                let auto = z.is_empty();
                (cur.as_ref().map_or(true, |p| p.share), if auto { dev.clone() } else { z.clone() }, auto)
            }
        };
        if !tinhat_widgets::clocks::valid_zone(&tz) {
            e.tz_error = "Tin Hat can't tell this device's time zone — pick one.".into();
            drop(e);
            publish();
            return;
        }
        e.tz_busy = true;
        e.tz_error.clear();
        let c = tinhat_widgets::clocks::TzPref { share, tz, auto }.to_json();
        (c, e.gen, !share)
    };
    publish();
    let me = client.user_id().map(|u| u.to_string()).unwrap_or_default();
    let r = tokio::time::timeout(Duration::from_secs(20), write_account_data(&client, TZ_TYPE, content.clone())).await;
    let ok = matches!(r, Ok(Ok(())));
    {
        let mut e = lock();
        if e.gen != gen {
            return;
        }
        e.tz_busy = false;
        if ok {
            if content.get("auto") == Some(&Value::Bool(true)) {
                tz_set_last_auto(&me, now_ms() as f64);
            }
            e.tz_pref = Some(content);
            if off {
                e.zones.remove(&me);
            }
        } else {
            e.tz_error = format!("Couldn't save: {}", written(r));
        }
    }
    publish();
    if ok && off {
        let Some(token) = client.access_token() else { return };
        for wait in [0u64, 2, 5, 15] {
            if wait > 0 {
                tokio::time::sleep(Duration::from_secs(wait)).await;
            }
            if lock().gen != gen {
                return;
            }
            if let Ok((204 | 200, _)) = api("DELETE", "/presence?what=tz", &token, None).await {
                return;
            }
        }
        eprintln!("[presence] the time zone's DELETE never answered 204 (the server's 20 s cache still ends it)");
    }
}

// ───────────────────────────────────── preference ─────────────────────────────────────

/// Sharing just went off (here, or seen via sync): your own entry leaves the local map at once,
/// so the footer, the member list and the profile cards stop showing you before the next read.
fn drop_own_entry(e: &mut Engine) -> bool {
    let me = e.user.clone();
    e.map.remove(&me).is_some()
}

/// Is this device (still) sharing, as far as it knows? Only then is your own entry shown.
fn sharing_locally(e: &Engine) -> bool {
    e.share == Some(true) && !e.stop_local && e.desired_share != Some(false)
}

/// A person turned sharing (back) on: forget an off that has not finished yet.
fn cancel_off(e: &mut Engine) {
    e.stop_local = false;
    e.turning_off = false;
    e.delete_due = None;
    e.delete_backoff = None;
}

/// Apply this device's new view of `share` (from the account data, a sync, or a write we made).
fn apply_share(e: &mut Engine, share: bool, why: &str) {
    match model::share_change(e.share, share) {
        ShareChange::Start => {
            e.flow += 1; // an older DELETE / 409 answer is now stale
            e.blocked = false;
            e.stop_local = false;
            e.turning_off = false;
            e.delete_due = None;
            e.delete_backoff = None;
            e.beat_due = None; // → a beat at once
            e.want_refresh = true; // A4.4: …that makes the server re-read the account data
            e.last_state = None;
            e.logged_state = None;
            eprintln!("[presence] sharing ON ({why})");
        }
        ShareChange::StopAndDelete => {
            e.flow += 1; // an older beat's answer is now stale
            drop_own_entry(e);
            e.stop_local = true;
            e.turning_off = true;
            e.beat_due = None;
            e.last_state = None;
            e.logged_state = None;
            if e.delete_due.is_none() {
                e.delete_due = Some(Instant::now());
                e.delete_backoff = None;
            }
            eprintln!("[presence] sharing OFF ({why}) — DELETE /api/presence");
        }
        ShareChange::Same => {
            // "…until the account data says share: true again" (§2.4): any fresh assertion of
            // share:true (a sync of the event, or our own write) lifts a 409 stop.
            if share && e.blocked {
                e.blocked = false;
                e.beat_due = None;
                eprintln!("[presence] share:true again ({why}) — beats resume");
            }
        }
    }
    e.share = Some(share);
}

/// 10.6.0 review: the signed-in session is still `me`'s — false once `session::clear_client` ran
/// (a sign-out), so a pass or a step that began for that session never touches the fresh engine.
fn session_is(me: &str) -> bool {
    crate::session::client().and_then(|c| c.user_id().map(|u| u.to_string())).is_some_and(|u| u == me)
}

/// Called from the live sync loop (bridge.rs) after every successful pass.
pub async fn on_sync(client: &Client, account_data: &[Raw<AnyGlobalAccountDataEvent>]) {
    let me = client.user_id().map(|u| u.to_string()).unwrap_or_default();
    let (first, gen) = {
        let mut e = lock();
        // A sync of another session than the engine's (a sign-out raced this pass) is ignored.
        if (!e.user.is_empty() && e.user != me) || !session_is(&me) {
            return;
        }
        e.last_sync_ok = Some(Instant::now());
        (!e.synced, e.gen)
    };
    if first {
        // "Start after the first sync, once the account data is known" (§2.4).
        let share = read_account_data(client, model::PREF_TYPE).await;
        let asked = read_account_data(client, model::ASKED_TYPE).await;
        let status = read_account_data(client, model::STATUS_TYPE).await;
        let tz = read_account_data(client, TZ_TYPE).await;
        let (mut start_writer, mut clear_note) = (false, false);
        let mut e = lock();
        if e.gen != gen || (!e.user.is_empty() && e.user != me) || !session_is(&me) {
            return; // signed out (or in as someone else) while reading
        }
        if !e.synced {
            e.user = me.clone();
            let s = model::parse_share(share.as_ref());
            apply_share(&mut e, s, "account data");
            // 10.6.0 review: an off chosen before the last quit / sign-out that never landed — finished
            // here, in the same locked step that allows beats, so nothing is sent before it.
            // (the note is read under the engine lock: choose_sharing changes it under the same lock)
            let pending = pending_off_has(&me);
            (start_writer, clear_note) = reconcile_pending_off(&mut e, pending, s);
            e.asked = model::parse_asked(asked.as_ref());
            e.my_status = status;
            e.tz_pref = tz;
            e.tz_known = true;
            e.synced = true;
            eprintln!(
                "[presence] preference known: sharing {}, question {}",
                if s { "on" } else { "off" },
                if e.asked { "answered" } else { "not answered" }
            );
        }
        let gen_now = e.gen;
        drop(e);
        if clear_note && CONSENT_INFLIGHT.load(Ordering::SeqCst) == 0 {
            pending_off_set(&me, false); // already off, and no older write of ours can still land
        }
        if start_writer {
            spawn(consent_writer(gen_now, client.clone()));
        }
        spawn(tz_check("start"));
    } else {
        let mut e = lock();
        if e.gen != gen || !session_is(&me) {
            return;
        }
        for raw in account_data {
            let Ok(Some(ty)) = raw.get_field::<String>("type") else { continue };
            if ty != model::PREF_TYPE && ty != model::ASKED_TYPE && ty != model::STATUS_TYPE && ty != TZ_TYPE {
                continue;
            }
            let content: Option<Value> = raw.get_field::<Value>("content").ok().flatten();
            if ty == TZ_TYPE {
                e.tz_pref = content; // another device's switch / pick / automatic zone, or the echo of ours
            } else if ty == model::PREF_TYPE {
                if e.desired_share.is_some() {
                    continue; // a choice made here is still being written: it wins (the writer applies it)
                }
                apply_share(&mut e, model::parse_share(content.as_ref()), "sync");
            } else if ty == model::ASKED_TYPE {
                e.asked = model::parse_asked(content.as_ref());
            } else if e.my_status != content {
                // A4.4: a status set here or on another device — the next beat re-reads it at once.
                e.my_status = content;
                e.want_refresh = true;
                e.beat_due = None;
            }
        }
    }
    publish();
    wake().notify_one();
}

/// Sign-out (bridge.rs `finish_sign_out`): `DELETE /api/presence?device=this` and, when this computer
/// had told the server it was in use, one `visible:false` — sent with this session's token, which the
/// server ends only after them (finish_sign_out waits for the returned task, bounded, before its server
/// logout). The engine is reset first (nothing new is dispatched for the old session), then the goodbye
/// waits for whatever was already on the wire. Then everything is forgotten. None: nothing to say.
pub fn on_sign_out(old: Option<Client>) -> Option<tokio::task::JoinHandle<()>> {
    let token = old.as_ref().and_then(|c| c.access_token());
    let (instance, was_active, pinging, off_pending, (write_off, note_rev)) = {
        let mut e = lock();
        // An off that has not reached its 204 yet is finished with the account-wide DELETE.
        let off_pending = e.turning_off || e.delete_due.is_some() || e.desired_share == Some(false);
        // 10.6.0 review: an off whose share:false has not landed yet (still on the wire, or in its
        // retry backoff) is written here first — the account must not stay opted in (A2.2).
        let write_off = e.desired_share == Some(false);
        let out = (e.instance.clone(), e.active_sent, e.ping_in_flight, off_pending, (write_off, pending_off_rev()));
        let (inst, visible, focused, idle, gen) = (e.instance.clone(), e.visible, e.focused, e.idle, e.gen);
        *e = Engine::fresh(gen + 1, inst, visible, focused, idle);
        out
    };
    publish();
    let (Some(token), Some(client)) = (token, old) else { return None };
    Some(crate::bridge::runtime().spawn(async move {
        let drained = drain(Duration::from_secs(3)).await;
        if write_off {
            let r = tokio::time::timeout(HTTP_TIMEOUT, write_account_data(&client, model::PREF_TYPE, model::pref_content(false))).await;
            let landed = matches!(r, Ok(Ok(())));
            eprintln!("[presence] sign-out: share:false (pending off) → {}", written(r));
            // the note is cleared only when nothing older can still land after this write (the drain
            // emptied) and no newer choice changed it; else this account's next start finishes the off
            if landed && drained {
                if let Some(u) = client.user_id() {
                    pending_off_ack(u.as_str(), note_rev);
                }
            }
        }
        let path = if off_pending { "/presence" } else { "/presence?device=this" };
        let r = tokio::time::timeout(HTTP_TIMEOUT, api("DELETE", path, &token, None)).await;
        eprintln!("[presence] sign-out: DELETE {path} → {}", r.map(|x| status_of(&x)).unwrap_or_else(|_| "timeout".into()));
        if was_active || pinging {
            let body = model::ping_body(false, &instance);
            let r = tokio::time::timeout(HTTP_TIMEOUT, api("POST", "/push/active", &token, Some(body))).await;
            eprintln!("[activity] sign-out: goodbye ping → {}", r.map(|x| status_of(&x)).unwrap_or_else(|_| "timeout".into()));
        }
    }))
}

/// Clean quit (main.rs, after the Qt loop or on SIGTERM): the same goodbyes, bounded at 3 s, so
/// friends do not see a closed Tin Hat as "Online" for the server's 90 s TTL and the phone is not
/// kept quiet by a computer that no longer runs Tin Hat. Nothing new is sent once it starts.
pub async fn on_quit() {
    if !STARTED.load(Ordering::SeqCst) {
        return;
    }
    let Some(client) = crate::session::client() else { return };
    let Some(token) = client.access_token() else { return };
    let started = Instant::now();
    let (sharing, off_pending, write_off, note_rev) = {
        let mut e = lock();
        e.stopping = true; // from here the consent writer picks nothing new
        let off_pending = e.turning_off || e.delete_due.is_some() || e.desired_share == Some(false);
        // 10.6.0 review: an off whose share:false has not landed yet is written first (A2.2)
        (e.synced && e.share == Some(true), off_pending, e.desired_share == Some(false), pending_off_rev())
    };
    let drained = drain(Duration::from_millis(1500)).await;
    let (active, instance) = {
        let e = lock();
        (e.active_sent || e.ping_in_flight, e.instance.clone())
    };
    let del = async {
        if off_pending {
            if write_off {
                let w = write_account_data(&client, model::PREF_TYPE, model::pref_content(false)).await;
                if w.is_ok() && drained {
                    if let Some(u) = client.user_id() {
                        pending_off_ack(u.as_str(), note_rev);
                    }
                } // else (or past the deadline) the note stays: the next start finishes the off
                eprintln!("[presence] quit: share:false (pending off) → {}", written(Ok(w)));
            }
            // an off still waiting for its 204: finish it for every device, as the switch asked
            let r = api("DELETE", "/presence", &token, None).await;
            eprintln!("[presence] quit: DELETE (pending off) → {}", status_of(&r));
        } else if sharing {
            let r = api("DELETE", "/presence?device=this", &token, None).await;
            eprintln!("[presence] quit: DELETE ?device=this → {}", status_of(&r));
        }
    };
    let ping = async {
        if active {
            let r = api("POST", "/push/active", &token, Some(model::ping_body(false, &instance))).await;
            eprintln!("[activity] quit: goodbye ping → {}", status_of(&r));
        }
    };
    let left = Duration::from_secs(3).saturating_sub(started.elapsed()).max(Duration::from_millis(500));
    let _ = tokio::time::timeout(left, async { tokio::join!(del, ping) }).await;
}

/// The consent writer (A2.2): writes the NEWEST explicit choice, one write at a time, so an older
/// "on" can never land after a newer "off". Checked for sign-out before every dispatch and after
/// every backoff. An "off" is retried until it lands (then the DELETE follows); a failed "on" is
/// reported and dropped.
async fn consent_writer(gen: u64, client: Client) {
    enum Next {
        Stop,
        Write(bool, ConsentInflight, u64),
    }
    enum After {
        Done,
        Again,
        Retry(Duration),
    }
    let mut backoff: Option<Duration> = None;
    loop {
        // (the engine lock is only ever held inside these blocks — never across an .await)
        let next = {
            let mut e = lock();
            if e.gen != gen || e.stopping {
                Next::Stop // signed out (the next session starts clean) / quitting (the goodbye finishes it)
            } else if let Some(w) = e.desired_share {
                // registered as on the wire, with the note's revision, in the same locked step that picks it
                Next::Write(w, ConsentInflight::new(), pending_off_rev())
            } else {
                e.consent_writer = false;
                e.writing = false;
                Next::Stop
            }
        };
        let (want, wire, note_rev) = match next {
            Next::Stop => {
                publish();
                return;
            }
            Next::Write(w, wire, rev) => (w, wire, rev),
        };
        let res = write_account_data(&client, model::PREF_TYPE, model::pref_content(want)).await;
        drop(wire);
        let after = {
            let mut e = lock();
            if e.gen != gen {
                return;
            }
            match res {
                Ok(()) if e.desired_share != Some(want) => {
                    backoff = None;
                    After::Again // a newer choice arrived while this one was on the wire
                }
                Ok(()) => {
                    if !want {
                        // the newest choice (an off) landed: nothing left to finish — unless the note
                        // changed since this write was picked (then it belongs to a newer choice)
                        if let Some(u) = client.user_id() {
                            pending_off_ack(u.as_str(), note_rev);
                        }
                    }
                    e.desired_share = None;
                    e.consent_writer = false;
                    e.writing = false;
                    apply_share(&mut e, want, "you");
                    if want {
                        cancel_off(&mut e);
                    } else {
                        // apply_share schedules the DELETE only on a true → false edge; make sure
                        // it runs even when this device never saw share:true (A2.2: write, then DELETE).
                        e.turning_off = true;
                        e.stop_local = true;
                        if e.delete_due.is_none() {
                            e.delete_due = Some(Instant::now());
                            e.delete_backoff = None;
                        }
                    }
                    After::Done
                }
                Err(err) if want && e.desired_share == Some(true) => {
                    eprintln!("[presence] turning on failed: {err}");
                    e.desired_share = None;
                    e.consent_writer = false;
                    e.writing = false;
                    e.error = Some("Couldn't turn on your status — check your connection and try again.".into());
                    After::Done
                }
                Err(err) => {
                    let b = model::next_backoff(backoff);
                    backoff = Some(b);
                    eprintln!("[presence] writing the choice failed: {err} — retry in {} s", b.as_secs());
                    After::Retry(b)
                }
            }
        };
        match after {
            After::Done => {
                publish();
                wake().notify_one();
                return;
            }
            After::Again => {}
            After::Retry(b) => {
                publish();
                tokio::time::sleep(b).await; // then the newest choice (and the session) is re-read
            }
        }
    }
}

/// Ask for a consent write (the switch, the card, a status pick that turns sharing on).
fn request_consent(e: &mut Engine, on: bool) -> bool {
    e.flow += 1;
    if on {
        cancel_off(e);
    } else {
        e.stop_local = true;
        e.turning_off = true;
        e.beat_due = None;
        drop_own_entry(e);
    }
    e.desired_share = Some(on);
    e.writing = true;
    let start = !e.consent_writer;
    e.consent_writer = true;
    start
}

/// The status writer (A4.1): the newest pick wins, one write at a time. A non-empty status
/// waits for a pending "turn on" to land first (consent, then the status — A4.6). A write the
/// user has since replaced is not applied locally.
async fn status_writer(gen: u64, client: Client) {
    enum Next {
        Stop,
        Wait,
        Write(Value),
    }
    loop {
        let next = {
            let mut e = lock();
            if e.gen != gen {
                Next::Stop
            } else {
                match e.desired_status.clone() {
                    // consent first: a non-empty status waits for a pending "turn on" to land
                    Some(c) if c.as_object().is_some_and(|o| !o.is_empty()) && e.desired_share == Some(true) => Next::Wait,
                    Some(c) => Next::Write(c),
                    None => {
                        e.status_writer = false;
                        Next::Stop
                    }
                }
            }
        };
        let content = match next {
            Next::Stop => return,
            Next::Wait => {
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
            Next::Write(c) => c,
        };
        let res = write_account_data(&client, model::STATUS_TYPE, content.clone()).await;
        let done = {
            let mut e = lock();
            if e.gen != gen {
                return;
            }
            if e.desired_status.as_ref() != Some(&content) {
                false // replaced while on the wire: write the newer one
            } else {
                e.desired_status = None;
                e.status_writer = false;
                match res {
                    Ok(()) => {
                        let set = model::live_status(Some(&content), now_ms()).is_some();
                        eprintln!("[presence] status {}", if set { "set" } else { "cleared" });
                        e.my_status = Some(content);
                        e.want_refresh = true;
                        e.beat_due = None;
                    }
                    Err(err) => {
                        eprintln!("[presence] writing the status failed: {err}");
                        e.error = Some("Couldn't save your status — check your connection and try again.".into());
                    }
                }
                true
            }
        };
        if done {
            publish();
            wake().notify_one();
            return;
        }
    }
}

/// Ask for a status write; returns whether a writer has to be started.
fn request_status(e: &mut Engine, content: Value) -> bool {
    e.desired_status = Some(content);
    let start = !e.status_writer;
    e.status_writer = true;
    start
}

// ───────────────────────────────────── the loop ─────────────────────────────────────

async fn run() {
    loop {
        step();
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(1)) => {}
            _ = wake().notified() => {}
        }
    }
}

/// Decide what is due and spawn it. Never holds the lock across an await.
fn step() {
    let Some(client) = crate::session::client() else { return };
    let Some(token) = client.access_token() else { return };
    let me = client.user_id().map(|u| u.to_string()).unwrap_or_default();
    let now = Instant::now();
    let mut dirty = false;
    let mut e = lock();
    let gen = e.gen;
    if e.stopping {
        return; // a clean quit is saying goodbye
    }
    if !session_is(&me) {
        return; // 10.6.0 review: this client's session was dropped while we waited for the lock
    }
    if e.user.is_empty() {
        e.user = me;
    } else if e.user != me {
        return; // another session's client (a sign-out is resetting the engine) — never mix them
    }

    // idle probe — every 15 s
    if !e.probe_in_flight && e.probe_due.map_or(true, |d| now >= d) {
        e.probe_in_flight = true;
        e.probe_due = Some(now + model::IDLE_PROBE_EVERY);
        spawn(probe_task(gen));
    }

    // reader — always (sharing or not); never closer than the server's 5 s limit
    let gap_ok = e.read_last_at.map_or(true, |t| now.duration_since(t) >= model::READ_MIN_GAP);
    if !e.read_in_flight && gap_ok && e.read_due.map_or(true, |d| now >= d) {
        e.read_in_flight = true;
        e.read_last_at = Some(now);
        e.read_due = Some(now + model::read_interval(e.visible));
        spawn(read_task(gen, token.clone()));
    }
    if (!e.map.is_empty() || !e.zones.is_empty()) && !model::keep_map(e.read_last_ok.map(|t| now.duration_since(t))) {
        e.map.clear();
        e.zones.clear();
        dirty = true;
        eprintln!("[presence] no answer for over a minute — statuses cleared");
    }

    // the off switch — DELETE until 204 (never overlapping a beat, never before the write)
    if let Some(due) = e.delete_due {
        if now >= due && !e.delete_in_flight && !e.beat_in_flight && !e.writing {
            e.delete_in_flight = true;
            spawn(delete_task(gen, e.flow, token.clone()));
        }
    }

    // beats — only while the account says share:true (and never over a pending DELETE)
    if e.synced && e.share == Some(true) && !e.stop_local && !e.blocked && !e.delete_in_flight && e.delete_due.is_none() {
        let cur = (model::beat_state(e.idle).to_string(), e.call_room.clone());
        if e.last_state.as_ref() != Some(&cur) {
            if e.last_state.is_some() {
                e.beat_due = Some(now + model::BEAT_DEBOUNCE); // a change → 2 s debounce
            }
            e.last_state = Some(cur.clone());
        }
        if !e.beat_in_flight && e.beat_due.map_or(true, |d| now >= d) {
            e.beat_in_flight = true;
            e.beat_due = Some(now + model::BEAT_EVERY);
            let refresh = std::mem::take(&mut e.want_refresh);
            spawn(beat_task(gen, e.flow, token.clone(), cur, refresh));
        }
    }

    // A4.6 expiry: a status whose `until` passed is cleared by writing `{}` — once, and only
    // while no newer pick is waiting to be written (the writer would otherwise erase it).
    if e.synced && e.desired_status.is_none() && model::status_expired(e.my_status.as_ref(), now_ms()) {
        let until = e.my_status.as_ref().and_then(|v| v.get("until")).and_then(Value::as_u64);
        if e.status_cleared_for != until {
            e.status_cleared_for = until;
            dirty = true; // it already shows as nothing (live_status) — the picker too
            eprintln!("[presence] status expired — clearing it");
            if request_status(&mut e, serde_json::json!({})) {
                spawn(status_writer(gen, client.clone()));
            }
        }
    }

    // §3.4 consent — at start and every 10 min
    if !e.quiet_in_flight && e.quiet_due.map_or(true, |d| now >= d) {
        e.quiet_in_flight = true;
        e.quiet_due = Some(now + model::QUIET_WANTED_EVERY);
        spawn(quiet_task(gen, token.clone()));
    }

    // §3.4 pings
    if !e.ping_in_flight {
        let allowed = model::may_ping(
            true,
            e.last_sync_ok.map(|t| now.duration_since(t)),
            crate::notify::enabled(),
            e.quiet_wanted,
        );
        let active = model::computer_active(e.idle, e.call_room.is_some());
        let due = e.ping_last_active.map_or(true, |t| now.duration_since(t) >= model::ACTIVE_EVERY);
        match model::ping_step(allowed && active, e.active_sent, due) {
            Ping::Active => {
                e.ping_in_flight = true;
                e.ping_last_active = Some(now);
                spawn(ping_task(gen, token, true, e.instance.clone()));
            }
            Ping::Inactive => {
                e.ping_in_flight = true;
                e.active_sent = false;
                spawn(ping_task(gen, token, false, e.instance.clone()));
            }
            Ping::None => {}
        }
    }
    drop(e);
    if dirty {
        publish();
    }
}

// ───────────────────────────────────── tasks ─────────────────────────────────────

async fn probe_task(gen: u64) {
    let sample = probe_idle().await;
    let mut e = lock();
    if e.gen != gen {
        return;
    }
    e.probe_in_flight = false;
    e.idle = sample;
    drop(e);
    wake().notify_one();
}

async fn beat_task(gen: u64, flow: u64, token: String, cur: (String, Option<String>), refresh: bool) {
    // Re-checked right before it goes out: sharing may have been switched off, the session
    // signed out or a quit begun since this beat was scheduled.
    {
        let mut e = lock();
        if e.gen != gen || e.stopping || e.share != Some(true) || e.stop_local || e.flow != flow {
            if e.gen == gen {
                e.beat_in_flight = false;
            }
            return;
        }
    }
    let body = model::beat_body(&cur.0, cur.1.as_deref(), refresh);
    let res = {
        let _wire = Inflight::new();
        api("PUT", "/presence", &token, Some(body)).await
    };
    let (status, reply) = match &res {
        Ok((s, v)) => (*s, v.clone()),
        Err(_) => (0, Value::Null),
    };
    let mut e = lock();
    if e.gen != gen {
        return;
    }
    e.beat_in_flight = false;
    // Logs never carry the state itself (the logs can be shipped): answers only, and only on a
    // change of outcome. TINHAT_PRESENCE_DEBUG=1 (a test rig) adds the state.
    let what = if debug() {
        format!(" {}{}{}", cur.0, if cur.1.is_some() { " (in a call)" } else { "" }, if refresh { " (refresh)" } else { "" })
    } else {
        String::new()
    };
    match model::beat_outcome(status, &reply) {
        model::BeatOutcome::Ok => {
            if debug() || e.beat_failing || e.logged_state.is_none() {
                eprintln!("[presence] beat{what} → {}", status_of(&res));
                e.logged_state = Some(cur);
            }
            e.beat_failing = false;
            if refresh {
                // what others see just changed — look again soon (the GET gap still applies)
                let soon = Instant::now() + Duration::from_secs(1);
                e.read_due = Some(e.read_due.map_or(soon, |d| d.min(soon)));
            }
        }
        model::BeatOutcome::NotSharing => {
            // Only when nothing newer happened meanwhile (a "share:true again" must not be undone).
            if e.flow == flow {
                e.blocked = true;
                eprintln!("[presence] beat → 409 not_sharing — stopped until the account says share:true again");
            }
        }
        model::BeatOutcome::Retry => {
            if debug() || !e.beat_failing {
                eprintln!("[presence] beat{what} → {} (next in 30 s)", status_of(&res));
            }
            e.beat_failing = true;
            if refresh {
                e.want_refresh = true; // the change still has to reach the server
            }
        }
    }
    drop(e);
    wake().notify_one();
}

async fn delete_task(gen: u64, flow: u64, token: String) {
    let res = api("DELETE", "/presence", &token, None).await;
    let ok = matches!(res, Ok((204, _))); // A2.2: retried until it answers 204 — nothing else
    let mut e = lock();
    if e.gen != gen {
        return;
    }
    e.delete_in_flight = false;
    if e.flow != flow {
        return; // turned back on meanwhile — that choice wins
    }
    if ok {
        e.delete_due = None;
        e.delete_backoff = None;
        e.turning_off = false;
        e.stop_local = false;
        eprintln!("[presence] off: DELETE /api/presence → {}", status_of(&res));
    } else {
        let b = model::next_backoff(e.delete_backoff);
        e.delete_backoff = Some(b);
        e.delete_due = Some(Instant::now() + b);
        eprintln!("[presence] off: DELETE /api/presence → {} — retry in {} s", status_of(&res), b.as_secs());
    }
    drop(e);
    publish();
    wake().notify_one();
}

async fn read_task(gen: u64, token: String) {
    let with_tz = ZONES_WANTED.load(Ordering::SeqCst) > 0;
    let res = api("GET", if with_tz { "/presence?tz=1" } else { "/presence" }, &token, None).await;
    let mut e = lock();
    if e.gen != gen {
        return;
    }
    e.read_in_flight = false;
    let mut changed = false;
    match &res {
        Ok((200, body)) => {
            let mut map = model::parse_users(body);
            // Until the DELETE lands the server may still list you: never show that here.
            if !sharing_locally(&e) {
                map.remove(&e.user);
            }
            // A7: the zones ride on the same read; mine goes at once when I switched it off here
            let mut zones = if with_tz { tinhat_widgets::clocks::zones_of(body) } else { BTreeMap::new() };
            if !tz_sharing(&e) {
                zones.remove(&e.user);
            }
            changed = map != e.map || zones != e.zones;
            if debug() || e.read_failing || e.read_last_ok.is_none() {
                eprintln!("[presence] read → 200 ({} sharing{})", map.len(), if with_tz { format!(", {} zones", zones.len()) } else { String::new() });
            }
            e.map = map;
            e.zones = zones;
            e.read_last_ok = Some(Instant::now());
            e.read_failing = false;
        }
        _ => {
            if debug() || !e.read_failing {
                eprintln!("[presence] read → {} (keeping the last answer ≤ 60 s)", status_of(&res));
            }
            e.read_failing = true;
        }
    }
    drop(e);
    if changed {
        publish();
    }
}

async fn quiet_task(gen: u64, token: String) {
    let res = api("GET", "/push/quiet-wanted", &token, None).await;
    let mut e = lock();
    if e.gen != gen {
        return;
    }
    e.quiet_in_flight = false;
    let wanted = match &res {
        Ok((200, body)) => Some(model::parse_quiet_wanted(body)),
        // No answer is no consent (A2.12): pings pause until the server says `wanted` again —
        // asked again in a minute rather than at the next 10-minute check.
        _ => {
            e.quiet_due = Some(Instant::now() + Duration::from_secs(60));
            Some(false)
        }
    };
    if let Some(w) = wanted {
        if w != e.quiet_wanted || debug() {
            eprintln!(
                "[activity] quiet-wanted → {} ({})",
                if w { "yes — this computer will keep the phone quiet while in use" } else { "no" },
                status_of(&res)
            );
        }
        e.quiet_wanted = w;
    }
}

async fn ping_task(gen: u64, token: String, visible: bool, instance: String) {
    if visible {
        // Consent and eligibility hold for EVERY ping (A2.12): re-checked right before it goes out.
        let mut e = lock();
        let now = Instant::now();
        let eligible = model::may_ping(
            true,
            e.last_sync_ok.map(|t| now.duration_since(t)),
            crate::notify::enabled(),
            e.quiet_wanted,
        ) && model::computer_active(e.idle, e.call_room.is_some());
        if e.gen != gen || e.stopping || !eligible {
            if e.gen == gen {
                e.ping_in_flight = false;
            }
            return;
        }
    }
    let res = {
        let _wire = Inflight::new();
        api("POST", "/push/active", &token, Some(model::ping_body(visible, &instance))).await
    };
    let ok = matches!(res, Ok((200..=299, _)));
    let mut e = lock();
    if e.gen != gen {
        return;
    }
    e.ping_in_flight = false;
    // The answer carries the consent too ({ok, ttl, wanted}): a phone that just dropped its
    // quietWhileActive registration stops the pings now, not at the next 10-minute check.
    if let Ok((200..=299, body)) = &res {
        if let Some(w) = body.get("wanted").and_then(Value::as_bool) {
            if w != e.quiet_wanted {
                eprintln!("[activity] quiet-wanted → {} (from the ping answer)", if w { "yes" } else { "no" });
            }
            e.quiet_wanted = w;
        }
    }
    // 10.6.0 review: normal logs (which can be shared) never record when you were at the computer —
    // only that a ping failed; TINHAT_PRESENCE_DEBUG=1 (a test rig) logs every ping.
    if visible {
        if ok {
            if debug() {
                eprintln!("[activity] at the computer: visible:true → {}", status_of(&res));
            } else if e.ping_failing {
                eprintln!("[activity] pings work again");
            }
            e.active_sent = true;
            e.ping_failing = false;
        } else {
            if !e.ping_failing || debug() {
                eprintln!("[activity] ping → {} (next try in 15 s)", status_of(&res));
            }
            e.ping_failing = true;
        }
    } else if debug() {
        eprintln!("[activity] away from the computer: visible:false → {}", status_of(&res));
    }
}

// ───────────────────────────────────── idle ─────────────────────────────────────

/// Log a probe fallback once per reason.
fn log_once(key: &'static str, msg: String) {
    static SEEN: OnceLock<Mutex<std::collections::HashSet<&'static str>>> = OnceLock::new();
    let set = SEEN.get_or_init(|| Mutex::new(std::collections::HashSet::new()));
    if set.lock().map(|mut s| s.insert(key)).unwrap_or(false) {
        eprintln!("{msg}");
    }
}

/// Tin Hat's own input clock (any Tin Hat window) — the last fallback.
fn in_app_sample() -> IdleSample {
    IdleSample { idle_ms: crate::video::input_idle_ms().unwrap_or(0), locked: false }
}

#[cfg(target_os = "linux")]
async fn bus_call(
    conn: &zbus::Connection,
    dest: &'static str,
    path: &'static str,
    iface: &'static str,
    method: &'static str,
) -> Option<zbus::Message> {
    match tokio::time::timeout(Duration::from_secs(1), conn.call_method(Some(dest), path, Some(iface), method, &()))
        .await
    {
        Ok(Ok(m)) => Some(m),
        _ => None,
    }
}

#[cfg(target_os = "linux")]
async fn probe_idle() -> IdleSample {
    const SS: &str = "org.freedesktop.ScreenSaver";
    // 10.6.0 review: getting the (shared, cached) connection is bounded too — a stalled bus setup must
    // neither stop the 15 s probes nor keep the fallback from answering.
    let conn = match tokio::time::timeout(Duration::from_secs(2), crate::notify::connection()).await {
        Ok(Some(c)) => c,
        Ok(None) => {
            log_once("nobus", "[presence] idle: no session bus — using Tin Hat's own input clock".into());
            return in_app_sample();
        }
        Err(_) => {
            log_once("busslow", "[presence] idle: the session bus did not answer in 2 s — using Tin Hat's own input clock".into());
            return in_app_sample();
        }
    };
    // 1. freedesktop ScreenSaver (KDE answers in ms)
    let mut idle: Option<u64> = None;
    for path in ["/ScreenSaver", "/org/freedesktop/ScreenSaver"] {
        if let Some(m) = bus_call(&conn, SS, path, SS, "GetSessionIdleTime").await {
            if let Ok(v) = m.body().deserialize::<u32>() {
                idle = Some(u64::from(v));
                log_once("ss", format!("[presence] idle: {SS}.GetSessionIdleTime on {path}"));
                break;
            }
        }
    }
    // 2. Mutter's IdleMonitor (GNOME)
    if idle.is_none() {
        if let Some(m) = bus_call(
            &conn,
            "org.gnome.Mutter.IdleMonitor",
            "/org/gnome/Mutter/IdleMonitor/Core",
            "org.gnome.Mutter.IdleMonitor",
            "GetIdletime",
        )
        .await
        {
            if let Ok(v) = m.body().deserialize::<u64>() {
                idle = Some(v);
                log_once("mutter", "[presence] idle: org.gnome.Mutter.IdleMonitor.GetIdletime".into());
            }
        }
    }
    // Screen lock.
    let mut locked = false;
    for path in ["/ScreenSaver", "/org/freedesktop/ScreenSaver"] {
        if let Some(m) = bus_call(&conn, SS, path, SS, "GetActive").await {
            if let Ok(b) = m.body().deserialize::<bool>() {
                locked = b;
                break;
            }
        }
    }
    match idle {
        Some(idle_ms) => IdleSample { idle_ms, locked },
        None => {
            log_once(
                "noidle",
                "[presence] idle: neither ScreenSaver nor Mutter answered — using Tin Hat's own input clock".into(),
            );
            IdleSample { locked, ..in_app_sample() }
        }
    }
}

#[cfg(windows)]
async fn probe_idle() -> IdleSample {
    match win_idle::idle_ms() {
        Some(idle_ms) => IdleSample { idle_ms, locked: false }, // A2.13: no lock detection needed
        None => {
            log_once("win", "[presence] idle: GetLastInputInfo failed — using Tin Hat's own input clock".into());
            in_app_sample()
        }
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
async fn probe_idle() -> IdleSample {
    in_app_sample()
}

/// Windows: `GetLastInputInfo` — the session's last keyboard/mouse input (keeps counting while
/// locked, which A2.13 accepts). Hand-bound like `audioapps_win.rs` (no windows crate).
#[cfg(windows)]
mod win_idle {
    #[repr(C)]
    struct LastInputInfo {
        cb_size: u32,
        dw_time: u32,
    }
    #[link(name = "user32")]
    extern "system" {
        fn GetLastInputInfo(plii: *mut LastInputInfo) -> i32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetTickCount() -> u32;
    }
    /// Milliseconds since the last input anywhere in the session.
    pub fn idle_ms() -> Option<u64> {
        let mut lii = LastInputInfo { cb_size: std::mem::size_of::<LastInputInfo>() as u32, dw_time: 0 };
        // SAFETY: `lii` is a correctly sized, initialised LASTINPUTINFO; the call only writes dwTime.
        let ok = unsafe { GetLastInputInfo(&mut lii) };
        if ok == 0 {
            return None;
        }
        // SAFETY: no arguments, no preconditions.
        let now = unsafe { GetTickCount() };
        // Both are 32-bit tick counts that wrap every ~49.7 days; the wrapping difference is right.
        Some(u64::from(now.wrapping_sub(lii.dw_time)))
    }
}

// ───────────────────────────────────── the QML side ─────────────────────────────────────

impl qobject::PresenceController {
    pub fn start(self: Pin<&mut Self>) {
        if std::env::var_os("TINHAT_VOD_SERVICE").is_some() {
            return; // the recorder never says anything about anyone
        }
        refresh_device_tz();
        if let Ok(mut s) = sink().lock() {
            *s = Some(self.qt_thread());
        }
        if !STARTED.swap(true, Ordering::SeqCst) {
            eprintln!("[presence] engine started");
            spawn(run());
        }
        publish();
    }

    pub fn choose_sharing(self: Pin<&mut Self>, on: bool) {
        let Some(client) = crate::session::client() else { return };
        let me = client.user_id().map(|u| u.to_string()).unwrap_or_default();
        let (gen, start, ask_too) = {
            let mut e = lock();
            // 10.6.0 review: the person's OFF is written ahead (their ON clears it), so a quit / sign-out
            // before the write lands can't lose it — under the engine lock, ordered with the writer.
            pending_off_set(&me, !on);
            let start = request_consent(&mut e, on);
            let ask_too = !e.asked;
            if ask_too {
                // Flipping the switch answers the one-time question too (it would otherwise pop
                // up later for someone who already chose). Not consent data — the flag only.
                e.asked = true;
            }
            (e.gen, start, ask_too)
        };
        publish();
        if ask_too {
            let c = client.clone();
            spawn(async move {
                if let Err(err) = write_account_data(&c, model::ASKED_TYPE, model::asked_content()).await {
                    eprintln!("[presence] writing {} failed: {err}", model::ASKED_TYPE);
                }
            });
        }
        if start {
            spawn(consent_writer(gen, client));
        }
    }

    pub fn answer_ask(self: Pin<&mut Self>, turn_on: bool) {
        let Some(client) = crate::session::client() else { return };
        lock().asked = true; // the card goes away at once
        publish();
        let c = client.clone();
        spawn(async move {
            for attempt in 0..2 {
                match write_account_data(&c, model::ASKED_TYPE, model::asked_content()).await {
                    Ok(()) => {
                        eprintln!("[presence] one-time question answered ({})", if turn_on { "turn on" } else { "no thanks" });
                        return;
                    }
                    Err(err) => {
                        eprintln!("[presence] writing {} failed: {err}", model::ASKED_TYPE);
                        if attempt == 0 {
                            tokio::time::sleep(Duration::from_secs(5)).await;
                        }
                    }
                }
            }
        });
        if turn_on {
            self.choose_sharing(true);
        }
    }

    pub fn set_call_room(self: Pin<&mut Self>, room_id: &QString) {
        let r = room_id.to_string();
        let call = if r.starts_with('!') { Some(r) } else { None };
        let mut e = lock();
        if e.call_room != call {
            e.call_room = call;
            drop(e);
            wake().notify_one();
        }
    }

    pub fn set_status(
        self: Pin<&mut Self>,
        id: &QString,
        text: &QString,
        emoji: &QString,
        color: &QString,
        until_ms: f64,
        turn_on_sharing: bool,
    ) {
        let Some(client) = crate::session::client() else { return };
        let text = text.to_string().trim().to_string();
        let n = text.chars().count();
        if n == 0 || n > model::STATUS_TEXT_MAX {
            return;
        }
        let color = color.to_string();
        let color = if model::PALETTE.iter().any(|(k, _)| *k == color) { color } else { "grey".to_string() };
        let until = if until_ms.is_finite() && until_ms > 0.0 { Some(until_ms as u64) } else { None };
        // A4.4: the server ignores the WHOLE status when the emoji is over 16 UTF-8 bytes — drop
        // just the emoji instead.
        let emoji = emoji.to_string().trim().to_string();
        let emoji = if emoji.len() > 16 { String::new() } else { emoji };
        let content = model::status_content(&id.to_string(), &text, &emoji, &color, until);
        if turn_on_sharing {
            // The "Show your status to friends? This turns on sharing." answer: consent first —
            // the status writer waits for the "on" to land (A4.6).
            self.choose_sharing(true);
        }
        let (gen, start) = {
            let mut e = lock();
            (e.gen, request_status(&mut e, content))
        };
        if start {
            spawn(status_writer(gen, client));
        }
    }

    pub fn clear_status(self: Pin<&mut Self>) {
        let Some(client) = crate::session::client() else { return };
        let (gen, start) = {
            let mut e = lock();
            (e.gen, request_status(&mut e, serde_json::json!({})))
        };
        if start {
            spawn(status_writer(gen, client));
        }
    }

    pub fn set_window_state(self: Pin<&mut Self>, visible: bool, focused: bool) {
        let now = Instant::now();
        let mut e = lock();
        let mut poke = false;
        if focused && !e.focused {
            e.read_due = Some(now); // "at once on focus" (§2.5), still ≥ 5.5 s after the last GET
            poke = true;
        }
        if visible && !e.visible {
            // back from hidden: fall into the visible cadence without waiting out a 60 s wait
            let next = e.read_last_at.map(|t| t + model::READ_VISIBLE).unwrap_or(now);
            e.read_due = Some(e.read_due.map_or(next, |d| d.min(next)));
            poke = true;
        }
        let back = (focused && !e.focused) || (visible && !e.visible);
        e.visible = visible;
        e.focused = focused;
        drop(e);
        if poke {
            wake().notify_one();
        }
        if back {
            // A7: the system zone may have changed (travel, a manual change) — read it here, on the Qt thread
            refresh_device_tz();
            spawn(tz_check("focus"));
        }
    }

    pub fn want_zones(self: Pin<&mut Self>, on: bool) {
        let n = if on { ZONES_WANTED.fetch_add(1, Ordering::SeqCst) + 1 } else { (ZONES_WANTED.fetch_sub(1, Ordering::SeqCst) - 1).max(0) };
        if n <= 0 {
            ZONES_WANTED.store(0, Ordering::SeqCst);
            let mut e = lock();
            if !e.zones.is_empty() {
                e.zones.clear();
                drop(e);
                publish();
            }
            return;
        }
        if on && n == 1 {
            lock().read_due = Some(Instant::now()); // read now (still ≥ 5.5 s after the last GET)
            wake().notify_one();
        }
    }

    pub fn tz_share(self: Pin<&mut Self>, on: bool) {
        refresh_device_tz();
        spawn(tz_write(TzAction::Share(on)));
    }

    pub fn tz_pick(self: Pin<&mut Self>, zone: &QString) {
        refresh_device_tz();
        spawn(tz_write(TzAction::Pick(zone.to_string())));
    }

    pub fn tz_choices(&self) -> QString {
        let v: Vec<Value> = tinhat_widgets::clocks::zone_choices()
            .into_iter()
            .map(|z| serde_json::json!({ "tz": z, "text": tinhat_widgets::clocks::zone_choice_text(z) }))
            .collect();
        QString::from(&Value::Array(v).to_string())
    }

    pub fn tz_choice_text(&self, tz: &QString) -> QString {
        QString::from(&tinhat_widgets::clocks::zone_choice_text(&tz.to_string()))
    }
}

/// 10.6.0 review (Astra #3): the pending-off note and what the next start does with it.
#[cfg(test)]
mod pending_off_tests {
    use super::*;

    #[test]
    fn pending_off_note_roundtrip() {
        let dir = std::env::temp_dir().join(format!("tinhat-pending-off-{}", std::process::id()));
        let p = dir.join("presence-pending-off.json");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(marker_list(&p).is_empty(), "no file = nothing pending");
        assert!(marker_update(&p, "@a:example.com", true));
        assert!(!marker_update(&p, "@a:example.com", true), "idempotent: no change");
        assert!(marker_update(&p, "@b:example.com", true));
        assert_eq!(marker_list(&p), vec!["@a:example.com".to_string(), "@b:example.com".to_string()]);
        marker_update(&p, "@a:example.com", false);
        assert_eq!(marker_list(&p), vec!["@b:example.com".to_string()]);
        marker_update(&p, "@b:example.com", false);
        assert!(!p.exists(), "the last one removes the file");
        marker_update(&p, "", true);
        assert!(!p.exists(), "never an empty id");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&p, b"not json").unwrap();
        assert!(marker_list(&p).is_empty(), "a broken note reads as nothing pending");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pending_off_ack_needs_the_same_revision() {
        use std::sync::atomic::AtomicU64;
        let dir = std::env::temp_dir().join(format!("tinhat-pending-rev-{}", std::process::id()));
        let p = dir.join("presence-pending-off.json");
        let _ = std::fs::remove_dir_all(&dir);
        let rev = AtomicU64::new(0);
        let u = "@me:example.com";
        note_set_at(&p, &rev, u, true); // the person chose OFF
        let picked = rev.load(Ordering::SeqCst); // a write picks it up
        note_set_at(&p, &rev, u, false); // ... then chose ON
        note_set_at(&p, &rev, u, true); // ... and OFF again, before the first write came back
        assert!(!note_ack_at(&p, &rev, u, picked), "an old completion never clears a newer choice");
        assert_eq!(marker_list(&p), vec![u.to_string()]);
        let now = rev.load(Ordering::SeqCst);
        note_set_at(&p, &rev, u, true); // a repeated OFF changes nothing (no revision bump)
        assert_eq!(rev.load(Ordering::SeqCst), now);
        assert!(note_ack_at(&p, &rev, u, now), "the newest off landing clears it");
        assert!(marker_list(&p).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pending_off_is_finished_at_the_next_start() {
        let fresh = || {
            let mut e = Engine::fresh(3, "i".into(), true, true, IdleSample::default());
            e.user = "@me:example.com".into();
            e.synced = true;
            e
        };
        // marked + the account still says share:true -> the off is finished: writer, no beats
        let mut e = fresh();
        apply_share(&mut e, true, "account data");
        let (start, clear) = reconcile_pending_off(&mut e, true, true);
        assert!(start && !clear);
        assert_eq!(e.desired_share, Some(false));
        assert!(e.stop_local && e.turning_off && e.beat_due.is_none(), "nothing is sent before it lands");
        // marked but already off -> just forget the note
        let mut e = fresh();
        apply_share(&mut e, false, "account data");
        assert_eq!(reconcile_pending_off(&mut e, true, false), (false, true));
        assert_eq!(e.desired_share, None);
        // not marked -> nothing changes, whatever the account says
        let mut e = fresh();
        apply_share(&mut e, true, "account data");
        assert_eq!(reconcile_pending_off(&mut e, false, true), (false, false));
        assert_eq!(e.desired_share, None);
        assert!(!e.stop_local);
        // a choice already made in this session (here: ON before the first sync) is the newest: untouched
        let mut e = fresh();
        apply_share(&mut e, true, "account data");
        request_consent(&mut e, true);
        assert_eq!(reconcile_pending_off(&mut e, true, true), (false, false));
        assert_eq!(e.desired_share, Some(true));
        assert!(!e.stop_local);
    }
}

/// 10.6 (fix round): your own entry leaves the map the moment sharing goes off — here or via sync.
#[cfg(test)]
mod own_entry_tests {
    use super::*;

    fn engine_sharing() -> Engine {
        let mut e = Engine::fresh(7, "i".into(), true, true, IdleSample::default());
        e.user = "@me:example.com".into();
        e.synced = true;
        apply_share(&mut e, true, "test");
        for who in ["@me:example.com", "@friend:example.com"] {
            e.map.insert(who.into(), model::Seen { state: "online".into(), room: String::new(), status: None });
        }
        e
    }

    #[test]
    fn own_entry_goes_at_switch_off() {
        let mut e = engine_sharing();
        assert!(sharing_locally(&e));
        request_consent(&mut e, false); // the switch → off (before the write is acknowledged)
        assert!(!e.map.contains_key("@me:example.com"), "gone at once");
        assert!(e.map.contains_key("@friend:example.com"), "friends stay");
        assert!(!sharing_locally(&e) && e.turning_off && e.stop_local);
        assert!(e.delete_due.is_none(), "the DELETE waits for the write's acknowledgement");

        let mut e = engine_sharing();
        apply_share(&mut e, false, "sync"); // turned off on another device
        assert!(!e.map.contains_key("@me:example.com"));
        assert!(e.turning_off && e.delete_due.is_some(), "and the off is finished here with a DELETE");

        let mut e = engine_sharing();
        request_consent(&mut e, true); // on stays on: nothing is dropped
        assert!(e.map.contains_key("@me:example.com"));
    }
}
