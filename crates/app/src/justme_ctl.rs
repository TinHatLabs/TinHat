//! JustMeController — "Is it just me?" (widgets W2, contract WIDGETS.md §20; the web's features/widgets/justme/store.ts).
//! ONE checker for the whole app, running while a widget shows it (QML `acquire()` / `release()`). A check = the
//! homeserver's `/_matrix/client/versions` three times in a row (Chat's answer + your ping, the median), then at once:
//! the call JWT service's `/healthz` (the focus new calls use, `rtc::focus_service_url`), the media repo's
//! `/_matrix/client/v1/media/config` (with the bearer) and the Tin Hat server's `<proxy base>/presence` (no bearer: its
//! 401 is an answer) — each aborted after `TIMEOUT_MS`; plus the time since the last sync that answered (`note_sync_ok`,
//! one hook line in bridge.rs's sync loop). The verdict is `tinhat_widgets::justme` (the web's justme.ts, the shared
//! vectors). One HTTP client for the process, so its connections stay warm between checks like a browser's.
//!
//! Cadence = the widget's registry refresh: every 60 s while the window is focused, every 5 min unfocused (the RENDER
//! window state from `setWindow`), NEVER while hidden; on show and when a widget appears, one check after 3 s;
//! `checkNow()` = "Check again". The
//! last 10 minutes per row stay in memory for the Large sparkline.
//!
//! Test hook (end to end only, never saved): `TINHAT_JUSTME_URLS="calls=http://127.0.0.1:9/,media=…"` points a probe at
//! another URL (an unreachable one = that service down); ids chat / calls / media / tinhat.

use core::pin::Pin;
use std::collections::HashMap;
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use cxx_qt::{CxxQtThread, Threading};
use cxx_qt_lib::QString;
use serde_json::{json, Value};
use tinhat_widgets::justme as jm;

const FOCUSED_MS: u64 = 60_000;
const UNFOCUSED_MS: u64 = 300_000;
const ON_SHOW_MS: u64 = 3_000;

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
        #[qproperty(QString, check_json)] //  QML: checkJson — the last check (jm::Check JSON) + "at" ("" = none yet)
        #[qproperty(QString, history_json)] // QML: historyJson — {chat, calls, media, tinhat, ping: [{at, band, ms}]}
        #[qproperty(bool, running)] //         QML: running — a check is out
        #[qproperty(i32, rev)] //              QML: rev — bumps on every change
        type JustMeController = super::JustMeControllerRust;

        /// QML: acquire() / release() — a widget shows the checker's answers (it runs while any holds it).
        #[qinvokable]
        fn acquire(self: Pin<&mut JustMeController>);
        #[qinvokable]
        fn release(self: Pin<&mut JustMeController>);
        /// QML: checkNow() — "Check again".
        #[qinvokable]
        fn check_now(self: Pin<&mut JustMeController>);
        /// QML: setWindow("focused" | "unfocused" | "hidden") — the RENDER window state (GPU.md).
        #[qinvokable]
        fn set_window(self: Pin<&mut JustMeController>, state: &QString);
        /// QML: reset() — sign-out: forget everything.
        #[qinvokable]
        fn reset(self: Pin<&mut JustMeController>);
        /// QML: checkOf(inputJson) — the shared verdict of a check input (the samples use it: the same text as live).
        #[qinvokable]
        fn check_of(self: &JustMeController, input_json: &QString) -> QString;
        /// QML: sparkSlots(samplesJson, at) — the Large sparkline's ten one-minute slots ([{band, h} | null]).
        #[qinvokable]
        fn spark_slots(self: &JustMeController, samples_json: &QString, at: f64) -> QString;
    }

    impl cxx_qt::Threading for JustMeController {}
}

#[derive(Default)]
pub struct JustMeControllerRust {
    check_json: QString,
    history_json: QString,
    running: bool,
    rev: i32,
}

// ── the engine (process-wide) ───────────────────────────────────────────────────────────────────────────────────

#[derive(Default)]
struct Engine {
    qt: Option<CxxQtThread<qobject::JustMeController>>,
    history: HashMap<&'static str, Vec<Value>>,
    loop_gen: u64,
    loop_running: bool,
    running: bool,
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
/** the next check is due at (ms since the epoch; 0 = now) */
static DUE_AT: AtomicU64 = AtomicU64::new(0);
/** the last sync that answered (ms), and when this session's sync began */
static LAST_SYNC_OK: AtomicU64 = AtomicU64::new(0);
static SYNC_FROM: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// bridge.rs, the sync loop: this session's sync begins (the age counts from here until the first answer).
pub fn note_sync_start() {
    SYNC_FROM.store(now_ms(), Ordering::SeqCst);
    LAST_SYNC_OK.store(0, Ordering::SeqCst);
}

/// bridge.rs, the sync loop: a sync answered.
pub fn note_sync_ok() {
    LAST_SYNC_OK.store(now_ms(), Ordering::SeqCst);
}

fn sync_age_ms() -> Option<f64> {
    let from = SYNC_FROM.load(Ordering::SeqCst);
    if from == 0 {
        return None;
    }
    let last = LAST_SYNC_OK.load(Ordering::SeqCst).max(from);
    Some(now_ms().saturating_sub(last) as f64)
}

fn http() -> &'static matrix_sdk::reqwest::Client {
    static C: OnceLock<matrix_sdk::reqwest::Client> = OnceLock::new();
    C.get_or_init(|| crate::proxy::http(Some(Duration::from_millis(jm::TIMEOUT_MS))))
}

/// `TINHAT_JUSTME_URLS` (tests): id → URL.
fn url_override(id: &str) -> Option<String> {
    let v = std::env::var("TINHAT_JUSTME_URLS").ok()?;
    v.split(',').find_map(|kv| kv.split_once('=').filter(|(k, _)| k.trim() == id).map(|(_, u)| u.trim().to_string()))
}

/// One request: its status (none = no answer) and the round trip to the response headers.
async fn probe(url: &str, bearer: Option<&str>) -> jm::Probe {
    let mut req = http().get(url).header("Cache-Control", "no-store");
    if let Some(t) = bearer {
        req = req.header("Authorization", format!("Bearer {t}"));
    }
    let t0 = Instant::now();
    match tokio::time::timeout(Duration::from_millis(jm::TIMEOUT_MS), req.send()).await {
        Ok(Ok(resp)) => {
            let ms = t0.elapsed().as_secs_f64() * 1000.0;
            let status = resp.status().as_u16() as f64;
            drop(resp);
            jm::Probe { status: Some(status), ms: Some(ms) }
        }
        _ => jm::Probe { status: None, ms: None },
    }
}

fn push(list: &mut Vec<Value>, at: u64, band: &str, ms: Option<f64>) {
    let from = at as f64 - jm::SPARK_SLOTS as f64 * jm::SLOT_MS;
    list.retain(|x| x.get("at").and_then(Value::as_f64).is_some_and(|t| t > from));
    list.push(json!({ "at": at, "band": band, "ms": ms.map(tinhat_widgets::num).unwrap_or(Value::Null) }));
    let n = list.len();
    if n > 2 * jm::SPARK_SLOTS {
        list.drain(..n - 2 * jm::SPARK_SLOTS);
    }
}

fn history_json(h: &HashMap<&'static str, Vec<Value>>) -> String {
    let mut o = serde_json::Map::new();
    for k in ["chat", "calls", "media", "tinhat", "ping"] {
        o.insert(k.into(), Value::Array(h.get(k).cloned().unwrap_or_default()));
    }
    Value::Object(o).to_string()
}

fn set_running(on: bool) {
    let qt = {
        let mut e = engine().lock().unwrap();
        e.running = on;
        e.qt.clone()
    };
    if let Some(qt) = qt {
        let _ = qt.queue(move |mut o| {
            o.as_mut().set_running(on);
            let r = o.rev().wrapping_add(1);
            o.as_mut().set_rev(r);
        });
    }
}

async fn run_check() {
    let Some(client) = crate::session::client() else { return };
    if engine().lock().unwrap().running {
        return;
    }
    set_running(true);
    let hs = client.homeserver().to_string().trim_end_matches('/').to_string();
    let versions = url_override("chat").unwrap_or_else(|| format!("{hs}/_matrix/client/versions"));
    let mut pings = Vec::with_capacity(3);
    for _ in 0..3 {
        pings.push(probe(&versions, None).await);
    }
    let token = client.access_token();
    let calls_url = match url_override("calls") {
        Some(u) => u,
        None => format!("{}/healthz", crate::rtc::focus_service_url(&client).await.trim_end_matches('/')),
    };
    let media_url = url_override("media").unwrap_or_else(|| format!("{hs}/_matrix/client/v1/media/config"));
    let tinhat_url = url_override("tinhat").unwrap_or_else(|| format!("{}/presence", crate::proxy::base()));
    let (calls, media, tinhat) = tokio::join!(probe(&calls_url, None), probe(&media_url, token.as_deref()), probe(&tinhat_url, None));
    let input = jm::CheckInput { pings, sync_ago_ms: sync_age_ms(), calls, media, tinhat };
    let check = jm::check_of(&input);
    let at = now_ms();
    let mut out = check.to_json();
    if let Some(o) = out.as_object_mut() {
        o.insert("at".into(), Value::from(at));
    }
    if check.tone == "poor" {
        eprintln!("[justme] check: {}", check.verdict);
    }
    let (qt, hist) = {
        let mut e = engine().lock().unwrap();
        for row in &check.rows {
            push(e.history.entry(row.id).or_default(), at, row.band, row.ms);
        }
        push(e.history.entry("ping").or_default(), at, check.ping.band, check.ping.ms);
        e.running = false;
        (e.qt.clone(), history_json(&e.history))
    };
    let text = out.to_string();
    if let Some(qt) = qt {
        let _ = qt.queue(move |mut o| {
            o.as_mut().set_check_json(QString::from(&text));
            o.as_mut().set_history_json(QString::from(&hist));
            o.as_mut().set_running(false);
            let r = o.rev().wrapping_add(1);
            o.as_mut().set_rev(r);
        });
    }
}

fn cadence() -> u64 {
    if VIEW.load(Ordering::SeqCst) == 2 {
        FOCUSED_MS
    } else {
        UNFOCUSED_MS
    }
}

async fn check_loop(gen: u64) {
    loop {
        if crate::session::generation() != gen {
            engine().lock().unwrap().loop_running = false;
            return;
        }
        let view = VIEW.load(Ordering::SeqCst);
        let want = CONSUMERS.load(Ordering::SeqCst) > 0 && view != 0;
        let mut sleep_ms = 3_600_000; // asleep until something wants it (acquire / setWindow / checkNow wake us)
        if want {
            let due = DUE_AT.load(Ordering::SeqCst);
            let now = now_ms();
            if now >= due {
                run_check().await;
                DUE_AT.store(now_ms() + cadence(), Ordering::SeqCst);
                sleep_ms = cadence();
            } else {
                sleep_ms = due - now;
            }
        }
        let _ = tokio::time::timeout(Duration::from_millis(sleep_ms), wake().notified()).await;
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
        crate::bridge::runtime().spawn(check_loop(gen));
    }
}

impl qobject::JustMeController {
    fn bind(self: Pin<&mut Self>) {
        let qt = self.qt_thread();
        engine().lock().unwrap().qt = Some(qt);
    }

    pub fn acquire(mut self: Pin<&mut Self>) {
        let first = CONSUMERS.fetch_add(1, Ordering::SeqCst) == 0;
        self.as_mut().bind();
        if first {
            // a fresh widget: check now unless the last one is recent
            let last = self.check_json().to_string();
            let recent = tinhat_theme::json::parse_json(&last).ok().and_then(|v| v.get("at").and_then(Value::as_f64)).is_some_and(|at| (now_ms() as f64) - at < FOCUSED_MS as f64);
            if !recent {
                // in 3 s: the app may still be loading (a busy start reads as a slow network)
                DUE_AT.store(now_ms() + ON_SHOW_MS, Ordering::SeqCst);
            }
        }
        if crate::session::client().is_some() {
            ensure_loop();
            wake().notify_one();
        }
    }

    pub fn release(self: Pin<&mut Self>) {
        if CONSUMERS.fetch_sub(1, Ordering::SeqCst) <= 0 {
            CONSUMERS.store(0, Ordering::SeqCst);
        }
    }

    pub fn check_now(mut self: Pin<&mut Self>) {
        if crate::session::client().is_none() || VIEW.load(Ordering::SeqCst) == 0 || *self.running() {
            return;
        }
        self.as_mut().bind();
        DUE_AT.store(0, Ordering::SeqCst);
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
        if v == was {
            return;
        }
        if v != 0 && was == 0 {
            // shown again: one check after 3 s (a resumed sync answers first)
            DUE_AT.store(now_ms() + ON_SHOW_MS, Ordering::SeqCst);
        } else if v != 0 {
            // focused ↔ unfocused: the new cadence from now
            DUE_AT.store(now_ms() + if v == 2 { FOCUSED_MS } else { UNFOCUSED_MS }, Ordering::SeqCst);
        }
        if CONSUMERS.load(Ordering::SeqCst) > 0 && crate::session::client().is_some() {
            self.as_mut().bind();
            ensure_loop();
        }
        wake().notify_one();
    }

    pub fn reset(mut self: Pin<&mut Self>) {
        engine().lock().unwrap().history.clear();
        DUE_AT.store(0, Ordering::SeqCst);
        SYNC_FROM.store(0, Ordering::SeqCst);
        self.as_mut().set_check_json(QString::default());
        self.as_mut().set_history_json(QString::default());
        self.as_mut().set_running(false);
        let r = self.rev().wrapping_add(1);
        self.as_mut().set_rev(r);
    }

    pub fn check_of(&self, input_json: &QString) -> QString {
        let v = tinhat_theme::json::parse_json(&input_json.to_string()).unwrap_or(Value::Null);
        QString::from(&jm::check_of(&jm::CheckInput::from_json(&v)).to_json().to_string())
    }

    pub fn spark_slots(&self, samples_json: &QString, at: f64) -> QString {
        let v = tinhat_theme::json::parse_json(&samples_json.to_string()).unwrap_or(Value::Null);
        QString::from(&jm::spark_slots(&v, at.is_finite().then_some(at)).to_string())
    }
}
