//! GameServersController — the Game servers widget's data (11.1, docs/GAMESERVERS.md §4–§6; the web's
//! features/widgets/gameservers/store.ts). ONE poller of the Tin Hat server's `/api/gameservers/status` for the whole
//! app, running while something shows it (the widget, an Up next event whose game has a server, a visible calendar
//! card): QML calls `acquire()` / `release()`. Cadence = the widget's registry refresh (focused 15 s, unfocused 60 s,
//! the window state from `setWindow`), 3 s while a power action is pending (≤ 3 min), nothing while hidden (one refresh
//! when shown). A 403 (not a member of the governing space) rests the poller for 30 minutes. `power()` POSTs
//! `/api/gameservers/power` and, once the server accepted it, posts the announcement notice (§6).
//!
//! The shared logic is `tinhat_widgets::gameservers` (the web's src/widgets/gameservers.ts, proven identical by the
//! vectors); QML gets the status as a VIEW (`statusJson`: the read status plus, per server, the chip, the players
//! line, uptime, the bars and the size text) and calls the pure invokables with the view it holds (`planStart`,
//! `confirmFor`, `linkServer`, `nextEvent`, `summary`, `view` for the samples).
//!
//! Gate / review mode: `TINHAT_GAMESERVERS_DEMO=<sample>` (`demoKey()`): nothing is fetched; the widget shows its QML
//! sample of that name (member, admin, two, empty, empty-member, members-only, loading, error).

use core::pin::Pin;
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use cxx_qt::{CxxQtThread, Threading};
use cxx_qt_lib::QString;
use serde_json::{json, Value};
use tinhat_widgets::gameservers as gs;

const TIMEOUT: Duration = Duration::from_secs(20);
const FOCUSED_MS: u64 = 15_000;
const UNFOCUSED_MS: u64 = 60_000;
const PENDING_MS: u64 = 3_000;
const PENDING_MAX_MS: u64 = 3 * 60_000;
const NOT_MEMBER_REST_MS: u64 = 30 * 60_000;

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
        #[qproperty(QString, status_json)] // QML: statusJson — the status VIEW ("" = none yet / not a member)
        #[qproperty(QString, error)] //       QML: error — "" | "not-member" | "network" | "off"
        #[qproperty(bool, loading)] //        QML: loading — no answer yet
        #[qproperty(i32, rev)] //             QML: rev — bumps on every change
        #[qproperty(QString, toast)] //       QML: toast — a message for the user (a refused / failed power request)
        #[qproperty(i32, toast_rev)] //       QML: toastRev — bumps on every toast
        type GameServersController = super::GameServersControllerRust;

        /// QML: acquire() / release() — something shows the status (the poller runs while any holds it).
        #[qinvokable]
        fn acquire(self: Pin<&mut GameServersController>);
        #[qinvokable]
        fn release(self: Pin<&mut GameServersController>);
        /// QML: refresh() — ask now.
        #[qinvokable]
        fn refresh(self: Pin<&mut GameServersController>);
        /// QML: setWindow("focused" | "unfocused" | "hidden") — the RENDER window state (GPU.md).
        #[qinvokable]
        fn set_window(self: Pin<&mut GameServersController>, state: &QString);
        /// QML: power(action, id, first) — start / stop / restart (first = stop that one first; "" = none).
        #[qinvokable]
        fn power(self: Pin<&mut GameServersController>, action: &QString, id: &QString, first: &QString);
        /// QML: reset() — sign-out: forget everything.
        #[qinvokable]
        fn reset(self: Pin<&mut GameServersController>);

        /// QML: demoKey() — TINHAT_GAMESERVERS_DEMO ("" = live).
        #[qinvokable]
        fn demo_key(self: &GameServersController) -> QString;
        /// QML: view(statusJson) — a raw status (a QML sample) as the VIEW statusJson carries ("" = not a status).
        #[qinvokable]
        fn view(self: &GameServersController, status_json: &QString) -> QString;
        /// QML: summary(view) — the S / collapsed line (§3.3).
        #[qinvokable]
        fn summary(self: &GameServersController, view_json: &QString) -> QString;
        /// QML: planStart(view, id) — {kind: ok|unknown|refused|cap|memory, …} (§3.4).
        #[qinvokable]
        fn plan_start(self: &GameServersController, view_json: &QString, id: &QString) -> QString;
        /// QML: confirmFor(view, action, id, first) — {title, body, confirm, destructive} or "" (§3.5).
        #[qinvokable]
        fn confirm_for(self: &GameServersController, view_json: &QString, action: &QString, id: &QString, first: &QString) -> QString;
        /// QML: linkServer(view, eventJson) — the id of the server a calendar event is for, or "" (§3.7).
        #[qinvokable]
        fn link_server(self: &GameServersController, view_json: &QString, event_json: &QString) -> QString;
        /// QML: nextEvent(view, serverId, eventsJson, nowMs) — the index of the server's next event in eventsJson, -1 (§3.7).
        #[qinvokable]
        fn next_event(self: &GameServersController, view_json: &QString, server_id: &QString, events_json: &QString, now_ms: f64) -> i32;
    }

    impl cxx_qt::Threading for GameServersController {}
}

#[derive(Default)]
pub struct GameServersControllerRust {
    status_json: QString,
    error: QString,
    loading: bool,
    rev: i32,
    toast: QString,
    toast_rev: i32,
}

// ── the engine (process-wide: one poller whatever QML holds) ─────────────────────────────────────────────────────

#[derive(Default)]
struct Engine {
    qt: Option<CxxQtThread<qobject::GameServersController>>,
    /// the raw last good status (for the notice's names)
    last: Option<gs::GameStatus>,
    loop_gen: u64,
    loop_running: bool,
}

fn engine() -> &'static Mutex<Engine> {
    static E: OnceLock<Mutex<Engine>> = OnceLock::new();
    E.get_or_init(|| Mutex::new(Engine::default()))
}
fn wake() -> &'static tokio::sync::Notify {
    static W: OnceLock<tokio::sync::Notify> = OnceLock::new();
    W.get_or_init(tokio::sync::Notify::new)
}
static CONSUMERS: AtomicI32 = AtomicI32::new(0);
static VIEW: AtomicI32 = AtomicI32::new(2); // 0 hidden, 1 unfocused, 2 focused
static FAST_UNTIL: AtomicU64 = AtomicU64::new(0);
static REST_UNTIL: AtomicU64 = AtomicU64::new(0);
static PENDING: AtomicI32 = AtomicI32::new(0);

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn demo() -> Option<String> {
    std::env::var("TINHAT_GAMESERVERS_DEMO").ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// The VIEW of a read status: the status JSON plus, per server, what QML shows (§3.2–§3.3).
pub fn view_of(st: &gs::GameStatus) -> Value {
    let mut v = st.to_json();
    if let Some(list) = v.get_mut("servers").and_then(Value::as_array_mut) {
        for (i, sv) in st.servers.iter().enumerate() {
            let (tone, text) = gs::chip_of(sv);
            if let Some(o) = list.get_mut(i).and_then(Value::as_object_mut) {
                o.insert("up".into(), Value::Bool(gs::is_up(sv)));
                o.insert("chip".into(), json!({"tone": tone, "text": text}));
                o.insert("playersLine".into(), Value::String(gs::players_line(sv)));
                o.insert("whoLine".into(), Value::String(gs::who_line(sv)));
                o.insert("uptime".into(), Value::String(gs::uptime_text(sv.uptime_ms)));
                o.insert("cpuPm".into(), tinhat_widgets::num(gs::cpu_pm(sv.cpu_used, sv.cpu_limit)));
                o.insert("cpuText".into(), Value::String(gs::cpu_text(sv.cpu_used, sv.cpu_limit)));
                o.insert("memPm".into(), tinhat_widgets::num(gs::mem_pm(sv.mem_used, sv.mem_limit)));
                o.insert("memText".into(), Value::String(gs::mem_text(sv.mem_used, sv.mem_limit)));
                o.insert("size".into(), Value::String(gs::size_text(sv.mem_limit)));
                // the games catalogue's portrait (Up next's covers; "" = none: the emoji tile)
                let cover = sv.steam_app_id.map(|a| crate::games::art_url(&crate::games::default_catalog_base(), &format!("steam:{a}"), "portrait"));
                o.insert("cover".into(), Value::String(cover.unwrap_or_default()));
            }
        }
    }
    if let Some(o) = v.as_object_mut() {
        // the view is handed back to the pure invokables, which read it as a status again: it carries the format too
        o.insert("v".into(), Value::from(1));
        o.insert("summary".into(), Value::String(gs::summary_of(st)));
        o.insert("upCount".into(), Value::from(st.servers.iter().filter(|s| gs::is_up(s)).count() as i64));
    }
    v
}

fn read(text: &str) -> Option<gs::GameStatus> {
    tinhat_theme::json::parse_json(text).ok().as_ref().and_then(gs::read_status)
}

fn bearer() -> Option<String> {
    crate::session::client().and_then(|c| c.access_token())
}

/// One request to `{proxy::base()}/gameservers/…`: (status, body).
async fn call(method: &str, path: &str, body: Option<Value>) -> Result<(u16, Value), String> {
    let Some(token) = bearer() else { return Err("Not signed in".into()) };
    let url = format!("{}{}", crate::proxy::base(), path);
    let c = crate::proxy::http(Some(TIMEOUT));
    let mut req = if method == "POST" { c.post(&url) } else { c.get(&url) };
    req = req.header("Authorization", format!("Bearer {token}"));
    if let Some(b) = body {
        req = req.header("Content-Type", "application/json").body(b.to_string());
    }
    let resp = req.send().await.map_err(|e| if e.is_timeout() { "The server didn't answer in time".to_string() } else { format!("Couldn't reach the server: {e}") })?;
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    Ok((status, tinhat_theme::json::parse_json(&text).unwrap_or(Value::Null)))
}

enum Fetched {
    Status(gs::GameStatus),
    NotMember,
    Off,
    Failed,
}

async fn fetch() -> Fetched {
    match call("GET", "/gameservers/status", None).await {
        Ok((200, v)) => match gs::read_status(&v) {
            Some(st) => Fetched::Status(st),
            None => Fetched::Failed,
        },
        Ok((403, _)) => Fetched::NotMember,
        Ok((503, _)) => Fetched::Off,
        Ok((s, _)) => {
            eprintln!("[gameservers] status said {s}");
            Fetched::Failed
        }
        Err(e) => {
            eprintln!("[gameservers] status: {e}");
            Fetched::Failed
        }
    }
}

fn has_pending(st: &gs::GameStatus) -> bool {
    st.servers.iter().any(|s| s.pending.is_some() || s.state == "starting" || s.state == "stopping")
        || st.job.as_ref().is_some_and(|j| j.step == "stopping" || j.step == "starting")
}

/// Apply an answer on the Qt thread.
fn deliver(f: Fetched) {
    let qt = {
        let mut e = engine().lock().unwrap();
        if let Fetched::Status(st) = &f {
            e.last = Some(st.clone());
            PENDING.store(i32::from(has_pending(st)), Ordering::SeqCst);
        }
        if matches!(f, Fetched::NotMember) {
            e.last = None;
        }
        e.qt.clone()
    };
    let Some(qt) = qt else { return };
    let _ = qt.queue(move |mut o| {
        match f {
            Fetched::Status(st) => {
                o.as_mut().set_status_json(QString::from(&view_of(&st).to_string()));
                o.as_mut().set_error(QString::default());
            }
            Fetched::NotMember => {
                o.as_mut().set_status_json(QString::default());
                o.as_mut().set_error(QString::from("not-member"));
            }
            Fetched::Off => o.as_mut().set_error(QString::from("off")),
            Fetched::Failed => o.as_mut().set_error(QString::from("network")),
        }
        o.as_mut().set_loading(false);
        let r = o.rev().wrapping_add(1);
        o.as_mut().set_rev(r);
    });
}

async fn poll_loop(gen: u64) {
    loop {
        if crate::session::generation() != gen {
            engine().lock().unwrap().loop_running = false;
            return;
        }
        let view = VIEW.load(Ordering::SeqCst);
        let now = now_ms();
        let want = CONSUMERS.load(Ordering::SeqCst) > 0 && view != 0;
        if want && now >= REST_UNTIL.load(Ordering::SeqCst) {
            let f = fetch().await;
            if matches!(f, Fetched::NotMember) {
                REST_UNTIL.store(now_ms() + NOT_MEMBER_REST_MS, Ordering::SeqCst);
            } else if matches!(f, Fetched::Status(_)) {
                REST_UNTIL.store(0, Ordering::SeqCst);
            }
            deliver(f);
        }
        let mut ms = if VIEW.load(Ordering::SeqCst) == 2 { FOCUSED_MS } else { UNFOCUSED_MS };
        if PENDING.load(Ordering::SeqCst) != 0 && now_ms() < FAST_UNTIL.load(Ordering::SeqCst) {
            ms = PENDING_MS;
        }
        let rest = REST_UNTIL.load(Ordering::SeqCst);
        if rest > now_ms() {
            ms = ms.max(rest - now_ms());
        }
        if CONSUMERS.load(Ordering::SeqCst) <= 0 || VIEW.load(Ordering::SeqCst) == 0 {
            ms = 3_600_000; // asleep until something wants it (acquire / setWindow / refresh wake us)
        }
        let _ = tokio::time::timeout(Duration::from_millis(ms), wake().notified()).await;
    }
}

/// The actor's name for the notice: their display name in the room, else their global one, else the localpart.
async fn actor_name(room: &matrix_sdk::Room, me: &matrix_sdk::ruma::UserId) -> String {
    if let Ok(Some(m)) = room.get_member_no_sync(me).await {
        if let Some(n) = m.display_name().map(str::trim).filter(|n| !n.is_empty()) {
            return n.to_string();
        }
    }
    me.localpart().to_string()
}

/// Post the announcement (§6) in the configured room when this account is joined there; failures are logged only.
async fn announce(st: gs::GameStatus, action: String, id: String, first: Option<String>) {
    let Some(room_id) = st.announce_room.clone() else { return };
    let Some(client) = crate::session::client() else { return };
    let Ok(rid) = matrix_sdk::ruma::RoomId::parse(&room_id) else { return };
    let Some(room) = client.get_room(&rid) else { return };
    if room.state() != matrix_sdk::RoomState::Joined {
        return;
    }
    let Some(me) = client.user_id().map(|u| u.to_owned()) else { return };
    let name = |x: &str| st.server(x).map(|s| s.name.clone()).unwrap_or_else(|| x.to_string());
    let body = gs::notice_text(&actor_name(&room, &me).await, &action, &name(&id), first.as_deref().map(name).as_deref());
    if let Err(e) = room.send_raw("m.room.message", json!({ "msgtype": "m.notice", "body": body })).await {
        eprintln!("[gameservers] the notice was not posted: {e}");
    }
}

fn ensure_loop() {
    let gen = crate::session::generation();
    let start = {
        let mut e = engine().lock().unwrap();
        if e.loop_running && e.loop_gen == gen {
            false
        } else {
            e.loop_running = true;
            e.loop_gen = gen;
            true
        }
    };
    if start {
        crate::bridge::runtime().spawn(poll_loop(gen));
    }
}

impl qobject::GameServersController {
    fn bind(self: Pin<&mut Self>) {
        let qt = self.qt_thread();
        engine().lock().unwrap().qt = Some(qt);
    }

    fn say(mut self: Pin<&mut Self>, text: &str) {
        self.as_mut().set_toast(QString::from(text));
        let r = self.toast_rev().wrapping_add(1);
        self.as_mut().set_toast_rev(r);
    }

    pub fn acquire(mut self: Pin<&mut Self>) {
        if demo().is_some() {
            return;
        }
        if CONSUMERS.fetch_add(1, Ordering::SeqCst) == 0 && self.status_json().is_empty() && self.error().is_empty() {
            self.as_mut().set_loading(true);
        }
        self.as_mut().bind();
        if crate::session::client().is_some() {
            ensure_loop();
            wake().notify_one();
        }
    }

    pub fn release(self: Pin<&mut Self>) {
        if demo().is_some() {
            return;
        }
        if CONSUMERS.fetch_sub(1, Ordering::SeqCst) <= 0 {
            CONSUMERS.store(0, Ordering::SeqCst);
        }
    }

    pub fn refresh(mut self: Pin<&mut Self>) {
        // selfhost: a server without game servers (gameservers: false) is never polled; the widget is hidden
        if demo().is_some() || crate::session::client().is_none() || !crate::server_offer::feature_on_global(crate::server_offer::ServerFeature::Gameservers) {
            return;
        }
        self.as_mut().bind();
        if self.error().to_string() == "not-member" {
            return; // resting (30 min); a new session resets it
        }
        ensure_loop();
        wake().notify_one();
    }

    pub fn set_window(mut self: Pin<&mut Self>, state: &QString) {
        let v = match state.to_string().as_str() {
            "hidden" => 0,
            "unfocused" => 1,
            _ => 2,
        };
        let was = VIEW.swap(v, Ordering::SeqCst);
        if v != 0 && was == 0 {
            // shown again: ask at once (and start the poller if a widget acquired it before the session was restored)
            if CONSUMERS.load(Ordering::SeqCst) > 0 && demo().is_none() && crate::session::client().is_some() {
                self.as_mut().bind();
                ensure_loop();
            }
            wake().notify_one();
        }
    }

    pub fn power(mut self: Pin<&mut Self>, action: &QString, id: &QString, first: &QString) {
        let (action, id, first) = (action.to_string(), id.to_string(), first.to_string());
        if !matches!(action.as_str(), "start" | "stop" | "restart") || id.len() != 8 || demo().is_some() {
            return;
        }
        let first = (!first.is_empty()).then_some(first);
        self.as_mut().bind();
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let body = match &first {
                Some(f) => json!({ "server": id, "action": action, "stopFirst": f }),
                None => json!({ "server": id, "action": action }),
            };
            let before = engine().lock().unwrap().last.clone();
            let answer = call("POST", "/gameservers/power", Some(body)).await;
            let refused = match answer {
                Ok((200 | 202, _)) => {
                    FAST_UNTIL.store(now_ms() + PENDING_MAX_MS, Ordering::SeqCst);
                    PENDING.store(1, Ordering::SeqCst);
                    wake().notify_one();
                    if let Some(st) = before {
                        announce(st, action.clone(), id.clone(), first.clone()).await;
                    }
                    None
                }
                Ok((s, v)) => Some(v.get("error").and_then(Value::as_str).filter(|x| !x.is_empty()).map(str::to_string).unwrap_or_else(|| format!("The server said {s}"))),
                Err(e) => Some(e),
            };
            if let Some(text) = refused {
                let _ = qt.queue(move |o| o.say(&text));
            }
        });
    }

    pub fn reset(mut self: Pin<&mut Self>) {
        REST_UNTIL.store(0, Ordering::SeqCst);
        engine().lock().unwrap().last = None;
        self.as_mut().set_status_json(QString::default());
        self.as_mut().set_error(QString::default());
        self.as_mut().set_loading(true);
        let r = self.rev().wrapping_add(1);
        self.as_mut().set_rev(r);
    }

    pub fn demo_key(&self) -> QString {
        QString::from(&demo().unwrap_or_default())
    }

    pub fn view(&self, status_json: &QString) -> QString {
        match read(&status_json.to_string()) {
            Some(st) => QString::from(&view_of(&st).to_string()),
            None => QString::default(),
        }
    }

    pub fn summary(&self, view_json: &QString) -> QString {
        read(&view_json.to_string()).map(|st| QString::from(&gs::summary_of(&st))).unwrap_or_default()
    }

    pub fn plan_start(&self, view_json: &QString, id: &QString) -> QString {
        match read(&view_json.to_string()) {
            Some(st) => QString::from(&gs::plan_start(&st, &id.to_string()).to_json().to_string()),
            None => QString::from("{\"kind\":\"unknown\"}"),
        }
    }

    pub fn confirm_for(&self, view_json: &QString, action: &QString, id: &QString, first: &QString) -> QString {
        let Some(st) = read(&view_json.to_string()) else { return QString::default() };
        let f = first.to_string();
        let c = gs::confirm_of(&st, &action.to_string(), &id.to_string(), (!f.is_empty()).then_some(f.as_str()));
        c.map(|c| QString::from(&c.to_json().to_string())).unwrap_or_default()
    }

    pub fn link_server(&self, view_json: &QString, event_json: &QString) -> QString {
        let Some(st) = read(&view_json.to_string()) else { return QString::default() };
        let ev = tinhat_theme::json::parse_json(&event_json.to_string()).unwrap_or(Value::Null);
        gs::link_server(&ev, &st).map(|s| QString::from(&s.id)).unwrap_or_default()
    }

    pub fn next_event(&self, view_json: &QString, server_id: &QString, events_json: &QString, now_ms: f64) -> i32 {
        let Some(st) = read(&view_json.to_string()) else { return -1 };
        let evs = tinhat_theme::json::parse_json(&events_json.to_string()).ok().and_then(|v| v.as_array().cloned()).unwrap_or_default();
        gs::next_event_for(&server_id.to_string(), &evs, now_ms, &st).map(|i| i as i32).unwrap_or(-1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_view_carries_what_qml_shows() {
        let st = gs::read_status(&json!({"v": 1, "cap": 2, "memoryBudgetMB": 30720, "servers": [
            {"id": "8e6c2f05", "name": "Valheim Vanilla", "state": "running", "uptimeMs": 248491769, "players": {"count": 3, "names": ["Darrow", "Sable"]},
             "cpu": {"used": 17.6, "limit": 300}, "memory": {"usedMB": 2028, "limitMB": 8192}},
            {"id": "a1c0de77", "name": "Abiotic Factor", "state": "offline", "memory": {"limitMB": 6144}}
        ]})).unwrap();
        let v = view_of(&st);
        let s0 = &v["servers"][0];
        assert_eq!(s0["chip"], json!({"tone": "online", "text": "Online · 3 on"}));
        assert_eq!(s0["playersLine"], "Darrow, Sable +1");
        assert_eq!(s0["whoLine"], "Darrow, Sable +1");
        assert_eq!(s0["uptime"], "up 2 d 21 h");
        assert_eq!(s0["cpuText"], "6 %");
        assert_eq!(s0["memText"], "2.0 / 8 GB");
        assert_eq!(v["servers"][1]["size"], "6 GB");
        assert_eq!(v["summary"], "Valheim Vanilla · online · 3 on");
        assert_eq!(v["upCount"], 1);
        // the view reads back as the same status (QML hands it to the pure invokables)
        assert_eq!(gs::read_status(&v), Some(st));
    }
}
