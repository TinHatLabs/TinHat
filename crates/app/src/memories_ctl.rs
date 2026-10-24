//! MemoriesController — the Memories widget's data (widgets W2, contract WIDGETS.md §21; the web's
//! features/widgets/memories/store.ts). ONE reader for the whole app, running while a widget shows it (QML `acquire()`
//! / `release()`). The rooms = the joined rooms that have a recording marker (`dev.tinhat.record`, any content —
//! only they can hold memories), at most `MAX_ROOMS`, read one after another with the Memories window's own request
//! (`GET <api>/vods/sessions?room=…`, the bearer; links re-homed onto our API base); the list is
//! `tinhat_widgets::memories::memories_of` (the web's memories.ts). Cadence = the widget's registry refresh: on show, then
//! every 5 min focused / 15 min unfocused (the RENDER window state), never while hidden; `refresh()` asks now.
//! Posters: each memory's frame (the window's own rule, `poster_at`) is fetched ONCE into `<data>/memories-thumbs/`
//! (one request at a time, ≤ 3 tries 2.5 s apart — the server renders it with ffmpeg and answers 503 while busy); a
//! list refresh or a remount never asks again. Sign-out (`reset()`) forgets the list and deletes the files.

use core::pin::Pin;
use std::collections::HashMap;
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use cxx_qt::{CxxQtThread, Threading};
use cxx_qt_lib::QString;
use serde_json::{json, Value};
use tinhat_widgets::memories as mem;

const FOCUSED_MS: u64 = 300_000;
const UNFOCUSED_MS: u64 = 900_000;
const MAX_ROOMS: usize = 12;
const RECORD_TYPE: &str = "dev.tinhat.record";

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
        #[qproperty(QString, items_json)] // QML: itemsJson — [MemItem + posterPath ("" = none yet)], newest first
        #[qproperty(bool, loading)] //       QML: loading — nothing read yet
        #[qproperty(bool, failed)] //        QML: failed — the last read failed for every room and nothing is kept
        #[qproperty(f64, read_at)] //        QML: readAt — when the last read finished (ms; the rows' clock)
        #[qproperty(i32, rev)] //            QML: rev — bumps on every change
        type MemoriesController = super::MemoriesControllerRust;

        /// QML: acquire() / release() — a widget shows the list (the reader runs while any holds it).
        #[qinvokable]
        fn acquire(self: Pin<&mut MemoriesController>);
        #[qinvokable]
        fn release(self: Pin<&mut MemoriesController>);
        /// QML: refresh() — read now ("Retry").
        #[qinvokable]
        fn refresh(self: Pin<&mut MemoriesController>);
        /// QML: setWindow("focused" | "unfocused" | "hidden") — the RENDER window state (GPU.md).
        #[qinvokable]
        fn set_window(self: Pin<&mut MemoriesController>, state: &QString);
        /// QML: reset() — sign-out: forget everything (the poster files too).
        #[qinvokable]
        fn reset(self: Pin<&mut MemoriesController>);
        /// QML: line(itemJson, nowMs) — a row's second line ("2 h ago · 1 h 20 m", "Recording now", "Processing…").
        #[qinvokable]
        fn line(self: &MemoriesController, item_json: &QString, now_ms: f64) -> QString;
        /// QML: summary(itemsJson, nowMs) — the Small line ("3 new memories · latest 2 h ago").
        #[qinvokable]
        fn summary(self: &MemoriesController, items_json: &QString, now_ms: f64) -> QString;
    }

    impl cxx_qt::Threading for MemoriesController {}
}

pub struct MemoriesControllerRust {
    items_json: QString,
    loading: bool,
    failed: bool,
    read_at: f64,
    rev: i32,
}

impl Default for MemoriesControllerRust {
    fn default() -> Self {
        Self { items_json: QString::from("[]"), loading: true, failed: false, read_at: 0.0, rev: 0 }
    }
}

#[derive(Default)]
struct Engine {
    qt: Option<CxxQtThread<qobject::MemoriesController>>,
    /// room → its last good sessions answer (+ the room's name then)
    rooms: HashMap<String, (String, Value)>,
    /// poster key → the local file ("" = failed this session)
    posters: HashMap<String, String>,
    loop_gen: u64,
    loop_running: bool,
    running: bool,
    read_at: u64,
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
static DUE_AT: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn cadence() -> u64 {
    if VIEW.load(Ordering::SeqCst) == 2 {
        FOCUSED_MS
    } else {
        UNFOCUSED_MS
    }
}

pub(crate) fn thumbs_dir() -> std::path::PathBuf {
    crate::bridge::data_dir().join("memories-thumbs")
}

fn enc(s: &str) -> String {
    s.replace('%', "%25").replace('!', "%21").replace(':', "%3A").replace('#', "%23").replace('&', "%26").replace('?', "%3F")
}

/// The joined rooms with a recording marker: (id, name), id order, at most MAX_ROOMS.
async fn record_rooms(client: &matrix_sdk::Client) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for room in client.joined_rooms() {
        if crate::appstate::get_state(&room, RECORD_TYPE, "").await.is_none() {
            continue;
        }
        out.push((room.room_id().to_string(), room.name().unwrap_or_default()));
    }
    out.sort();
    out.truncate(MAX_ROOMS);
    out
}

/// The signed thumbnail URL of a poster (the Memories window's thumb rule: file → thumb, `&t=`, `&r=` on retries).
fn thumb_url(track_url: &str, t: f64, retry: u32) -> String {
    let base = crate::proxy::rebase_url(track_url).replace("/api/vods/file/", "/api/vods/thumb/");
    format!("{base}&t={}{}", t.max(0.0).floor() as i64, if retry > 0 { format!("&r={retry}") } else { String::new() })
}

fn poster_key(it: &mem::MemItem) -> Option<String> {
    let (file, t) = it.poster.as_ref()?;
    Some(format!("{}\u{0}{}\u{0}{}\u{0}{}", it.room, it.id, file, t))
}

fn file_name(key: &str) -> String {
    // a stable, filesystem-safe name (FNV-1a 64 of the key)
    let mut h: u64 = 0xcbf29ce484222325;
    for b in key.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}.jpg")
}

/// Fetch one poster into the thumbs dir (≤ 3 tries, 2.5 s apart on 503 / 429 / network errors).
async fn fetch_poster(track_url: &str, t: f64, path: &std::path::Path) -> bool {
    let c = crate::proxy::http(Some(Duration::from_secs(15)));
    for attempt in 0..3u32 {
        match c.get(thumb_url(track_url, t, attempt)).send().await {
            Ok(r) if r.status().is_success() => {
                if let Ok(bytes) = r.bytes().await {
                    if !bytes.is_empty() && std::fs::create_dir_all(thumbs_dir()).is_ok() && std::fs::write(path, &bytes).is_ok() {
                        return true;
                    }
                }
                return false;
            }
            Ok(r) if r.status().as_u16() != 503 && r.status().as_u16() != 429 => return false,
            _ => {}
        }
        tokio::time::sleep(Duration::from_millis(2500)).await;
    }
    false
}

fn rooms_value(e: &Engine, order: &[(String, String)]) -> Value {
    Value::Array(
        order
            .iter()
            .filter_map(|(id, name)| e.rooms.get(id).map(|(_, sessions)| json!({ "room": id, "name": name, "sessions": sessions })))
            .collect(),
    )
}

/// The items QML shows: the shared list + each poster's local file.
fn items_view(items: &[mem::MemItem], posters: &HashMap<String, String>) -> String {
    Value::Array(
        items
            .iter()
            .map(|it| {
                let mut v = it.to_json();
                let p = poster_key(it).and_then(|k| posters.get(&k).cloned()).unwrap_or_default();
                if let Some(o) = v.as_object_mut() {
                    o.insert("posterPath".into(), Value::String(if p.is_empty() { p } else { format!("file://{p}") }));
                }
                v
            })
            .collect(),
    )
    .to_string()
}

fn deliver(items: String, failed: bool, at: u64) {
    let qt = engine().lock().unwrap().qt.clone();
    if let Some(qt) = qt {
        let _ = qt.queue(move |mut o| {
            o.as_mut().set_items_json(QString::from(&items));
            o.as_mut().set_loading(false);
            o.as_mut().set_failed(failed);
            o.as_mut().set_read_at(at as f64);
            let r = o.rev().wrapping_add(1);
            o.as_mut().set_rev(r);
        });
    }
}

async fn run_read() {
    let Some(client) = crate::session::client() else { return };
    {
        let mut e = engine().lock().unwrap();
        if e.running {
            return;
        }
        e.running = true;
    }
    let rooms = record_rooms(&client).await;
    let token = client.access_token().unwrap_or_default();
    let mut failed = 0usize;
    for (id, name) in &rooms {
        match crate::proxy::get_json_auth(&format!("/vods/sessions?room={}", enc(id)), &token).await {
            Ok(mut v) => {
                crate::proxy::rebase_urls_in_json(&mut v);
                let sessions = v.get("sessions").cloned().unwrap_or(Value::Array(vec![]));
                engine().lock().unwrap().rooms.insert(id.clone(), (name.clone(), sessions));
            }
            Err(err) => {
                failed += 1;
                eprintln!("[memories-widget] sessions of {id}: {err}");
            }
        }
    }
    let at = now_ms();
    let (items, all_failed) = {
        let mut e = engine().lock().unwrap();
        e.rooms.retain(|k, _| rooms.iter().any(|(id, _)| id == k));
        let items = mem::memories_of(&rooms_value(&e, &rooms), 8);
        e.read_at = at;
        let all_failed = !rooms.is_empty() && failed == rooms.len() && e.rooms.is_empty();
        (items, all_failed)
    };
    // the list first (posters that are already here), then each missing poster once
    let posters = engine().lock().unwrap().posters.clone();
    deliver(items_view(&items, &posters), all_failed, at);
    for it in &items {
        let Some(key) = poster_key(it) else { continue };
        if engine().lock().unwrap().posters.contains_key(&key) {
            continue;
        }
        let (file, t) = it.poster.clone().unwrap_or_default();
        let track_url = {
            let e = engine().lock().unwrap();
            e.rooms.get(&it.room).and_then(|(_, ss)| {
                ss.as_array()?.iter().find(|s| s.get("id").and_then(Value::as_str) == Some(it.id.as_str()))?.get("tracks")?.as_array()?.iter()
                    .find(|x| x.get("file").and_then(Value::as_str) == Some(file.as_str()))?.get("url")?.as_str().map(str::to_string)
            })
        };
        let Some(track_url) = track_url else { continue };
        let path = thumbs_dir().join(file_name(&key));
        let ok = path.exists() || fetch_poster(&track_url, t, &path).await;
        let posters = {
            let mut e = engine().lock().unwrap();
            e.posters.insert(key, if ok { path.to_string_lossy().to_string() } else { String::new() });
            e.posters.clone()
        };
        if ok {
            deliver(items_view(&items, &posters), all_failed, at);
        }
    }
    engine().lock().unwrap().running = false;
}

async fn read_loop(gen: u64) {
    loop {
        if crate::session::generation() != gen {
            engine().lock().unwrap().loop_running = false;
            return;
        }
        let want = CONSUMERS.load(Ordering::SeqCst) > 0 && VIEW.load(Ordering::SeqCst) != 0;
        let mut sleep_ms = 3_600_000;
        if want {
            let due = DUE_AT.load(Ordering::SeqCst);
            let now = now_ms();
            if now >= due {
                run_read().await;
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
        crate::bridge::runtime().spawn(read_loop(gen));
    }
}

impl qobject::MemoriesController {
    fn bind(self: Pin<&mut Self>) {
        let qt = self.qt_thread();
        engine().lock().unwrap().qt = Some(qt);
    }

    pub fn acquire(mut self: Pin<&mut Self>) {
        let first = CONSUMERS.fetch_add(1, Ordering::SeqCst) == 0;
        self.as_mut().bind();
        if first {
            let last = engine().lock().unwrap().read_at;
            if last == 0 || now_ms().saturating_sub(last) >= FOCUSED_MS {
                DUE_AT.store(0, Ordering::SeqCst);
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

    pub fn refresh(mut self: Pin<&mut Self>) {
        if crate::session::client().is_none() || VIEW.load(Ordering::SeqCst) == 0 {
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
            // back on screen: read now when the last read is older than the cadence
            let last = engine().lock().unwrap().read_at;
            if last == 0 || now_ms().saturating_sub(last) >= cadence() {
                DUE_AT.store(0, Ordering::SeqCst);
            }
        }
        if CONSUMERS.load(Ordering::SeqCst) > 0 && crate::session::client().is_some() {
            self.as_mut().bind();
            ensure_loop();
        }
        wake().notify_one();
    }

    pub fn reset(mut self: Pin<&mut Self>) {
        {
            let mut e = engine().lock().unwrap();
            e.rooms.clear();
            e.posters.clear();
            e.read_at = 0;
        }
        let _ = std::fs::remove_dir_all(thumbs_dir());
        DUE_AT.store(0, Ordering::SeqCst);
        self.as_mut().set_items_json(QString::from("[]"));
        self.as_mut().set_loading(true);
        self.as_mut().set_failed(false);
        let r = self.rev().wrapping_add(1);
        self.as_mut().set_rev(r);
    }

    pub fn line(&self, item_json: &QString, now_ms: f64) -> QString {
        let v = tinhat_theme::json::parse_json(&item_json.to_string()).unwrap_or(Value::Null);
        mem::MemItem::from_json(&v).map(|it| QString::from(&mem::mem_line(&it, now_ms))).unwrap_or_default()
    }

    pub fn summary(&self, items_json: &QString, now_ms: f64) -> QString {
        let v = tinhat_theme::json::parse_json(&items_json.to_string()).unwrap_or(Value::Null);
        let items: Vec<mem::MemItem> = v.as_array().map(|a| a.iter().filter_map(mem::MemItem::from_json).collect()).unwrap_or_default();
        QString::from(&mem::mem_summary(&items, now_ms))
    }
}
