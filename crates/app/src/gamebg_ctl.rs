//! gamebg_ctl.rs — "Match the game" chat backgrounds (lane sun-gamebg, 2026-11-14), the desktop's runtime: the twin of
//! Tin Hat Web's `src/features/chat/gamebg/{runtime.ts,GameArtLayer.tsx}`; the rules are the shared pure model
//! (`tinhat_widgets::gamebg`, vectors `testvectors/gamebg/vectors.json`).
//!
//! QML `GameArtController` (one per chat view): `open(roomId)`, `setBackground(contentJson)` (the room's own
//! `dev.tinhat.chat.background` content), `setPrefs(backgrounds, gameArt)`, `setInk(bg, textsJson, mode)` (the timeline
//! background + every text colour drawn straight on it, people's name colours included), `setHidden(hidden)`; out:
//! `gameId`, `artUrl` (a `file://` of the cached art, "" = draw nothing), `artWidth`, `artHeight`, `veil`.
//!
//! Inputs are only what this device already has: the Reach statuses of the room's joined members who SHARE them (the
//! presence service lists nobody else), the room's Now playing (`dev.tinhat.activity`) and who is in its call. A status
//! counts only when its text names a game in the person's OWN server's catalogue (`POST <catalog>/resolve`, a local
//! lookup there). The art comes from the person's own server (`<catalog>/art/<id>/background`): downloaded once
//! (streamed, ≤ 8 MB), its size read from the header before decoding (≤ 40 MP), decoded and measured at FULL size into
//! bound cells, and only then written to the cache. No work while hidden: the loop sleeps until something changes.
use core::pin::Pin;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use cxx_qt::{CxxQtThread, CxxQtType, Threading};
use cxx_qt_lib::QString;
use serde_json::{json, Value};
use tinhat_widgets::gamebg as model;

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
        #[qproperty(QString, room_id)] //   QML: roomId
        #[qproperty(QString, game_id)] //   QML: gameId — the shown game's catalogue id ("" = none)
        #[qproperty(QString, art_url)] //   QML: artUrl — file:// of the cached art ("" = draw nothing)
        #[qproperty(i32, art_width)] //     QML: artWidth / artHeight — the art's own size (px)
        #[qproperty(i32, art_height)]
        #[qproperty(f64, veil)] //          QML: veil — the theme veil for THIS art in this theme
        #[qproperty(QString, art_json)] //  QML: artJson — {id,url,w,h,veil} set LAST, in one go ("" = nothing): the layer
        //                                   reads this one, so an image and its veil can never arrive apart
        type GameArtController = super::GameArtControllerRust;

        /// QML: open(roomId) — follow a room ("" = none).
        #[qinvokable]
        fn open(self: Pin<&mut GameArtController>, room_id: &QString);
        /// QML: setBackground(contentJson) — the room's own dev.tinhat.chat.background content ("" = none set).
        #[qinvokable]
        fn set_background(self: Pin<&mut GameArtController>, content_json: &QString);
        /// QML: setPrefs(backgrounds, gameArt) — "Show chat backgrounds" + "Game art backgrounds".
        #[qinvokable]
        fn set_prefs(self: Pin<&mut GameArtController>, backgrounds: bool, game_art: bool);
        /// QML: setInk(bg, textsJson, mode) — the timeline background + its on-background text colours ("#rrggbb").
        #[qinvokable]
        fn set_ink(self: Pin<&mut GameArtController>, bg: &QString, texts_json: &QString, mode: &QString);
        /// QML: setRoleColours(json) — the room's role name colours as drawn ("#rrggbb"); the veil measures them too (3-2).
        #[qinvokable]
        fn set_role_colours(self: Pin<&mut GameArtController>, json: &QString);
        /// QML: setHidden(hidden) — the view or the window is hidden: no lookups, no downloads, no timers.
        #[qinvokable]
        fn set_hidden(self: Pin<&mut GameArtController>, hidden: bool);
        /// QML: preview(filePath) — the gallery: show a local art file as the shown game (measured like downloaded art).
        #[qinvokable]
        fn preview(self: Pin<&mut GameArtController>, file_path: &QString);
        /// QML: matchGamePlanJson(bgKind, artShowing) — the sheet's "Match the game": {plan, content} (model::match_game_plan).
        #[qinvokable]
        fn match_game_plan_json(self: &GameArtController, bg_kind: &QString, art_showing: bool) -> QString;
        /// QML: wordsJson() — the feature's visible strings (testvectors/gamebg/words.json, the web's words.ts).
        #[qinvokable]
        fn words_json(self: &GameArtController) -> QString;
        /// QML: fitJson(artW, artH, areaW, areaH, dpr) — model::fit_art as {w,h,x,y,fadeBottom,fadeSides}.
        #[qinvokable]
        fn fit_json(self: &GameArtController, art_w: f64, art_h: f64, area_w: f64, area_h: f64, dpr: f64) -> QString;
    }

    impl cxx_qt::Threading for GameArtController {}
}

/// The visible strings, byte for byte the shared file (the web's words.ts is tested against it).
pub const WORDS: &str = include_str!("../../../testvectors/gamebg/words.json");

#[derive(Clone, Default)]
struct Inputs {
    room: String,
    bg: Option<Value>,
    backgrounds: bool,
    game_art: bool,
    hidden: bool,
    /// bumps on every change: a pass computed for older inputs is never applied
    gen: u64,
}

#[derive(Clone, Default)]
struct Ink {
    bg: String,
    texts: Vec<String>,
    mode: String,
}

/// The ink the veil measures: the theme's texts + the room's role name colours (3-2; model::ink_texts).
fn ink_of(bg: &str, texts: &[String], roles: &[String], mode: &str) -> Ink {
    Ink { bg: bg.to_string(), texts: model::ink_texts(texts, &[], roles), mode: mode.to_string() }
}

fn json_strings(json: &str) -> Vec<String> {
    serde_json::from_str::<Value>(json)
        .ok()
        .and_then(|v| v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()))
        .unwrap_or_default()
}

pub struct GameArtControllerRust {
    room_id: QString,
    game_id: QString,
    art_url: QString,
    art_width: i32,
    art_height: i32,
    veil: f64,
    art_json: QString,
    /// bumps on every preview() / open(): an older preview job never draws over a newer one or a room
    preview_gen: Arc<AtomicU64>,
    inputs: Arc<Mutex<Inputs>>,
    wake: Arc<tokio::sync::Notify>,
    ink: Ink,
    /// setInk's inputs and the room's role colours: ink = ink_of(them)
    theme_ink: Ink,
    roles: Vec<String>,
    /// the game this view should show (what `apply` was last asked for), kept apart from what it DRAWS: a moment when
    /// no veil fits (a theme half-applied) must not forget the game
    wanted: Option<String>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl Default for GameArtControllerRust {
    fn default() -> Self {
        Self {
            room_id: QString::default(),
            game_id: QString::default(),
            art_url: QString::default(),
            art_width: 0,
            art_height: 0,
            veil: 0.0,
            art_json: QString::default(),
            preview_gen: Arc::new(AtomicU64::new(0)),
            inputs: Arc::new(Mutex::new(Inputs { backgrounds: true, game_art: true, ..Default::default() })),
            wake: Arc::new(tokio::sync::Notify::new()),
            ink: Ink::default(),
            theme_ink: Ink::default(),
            roles: Vec::new(),
            wanted: None,
            task: None,
        }
    }
}

impl Drop for GameArtControllerRust {
    fn drop(&mut self) {
        if let Some(t) = self.task.take() {
            t.abort();
        }
    }
}

// ---- process-wide caches (shared by every chat view; scoped to the account + catalogue; bounded) ----------------------
struct Resolved {
    id: Option<String>,
    at: Instant,
    ttl: Duration,
}
const HIT_TTL: Duration = Duration::from_secs(6 * 3600);
const MISS_TTL: Duration = Duration::from_secs(600);
const FAIL_RETRY: Duration = Duration::from_secs(60);
const READY_WAIT: Duration = Duration::from_secs(8);
const POLL: Duration = Duration::from_secs(5);
const MAX_DOWNLOAD: usize = 8 * 1024 * 1024;
const DISK_KEEP: usize = 40;

#[derive(Clone)]
struct Art {
    path: PathBuf,
    w: i32,
    h: i32,
    cells: Arc<Vec<model::Rgb>>,
}
#[derive(Default)]
struct Caches {
    scope: String,
    resolved: HashMap<String, Resolved>,
    /// user → (current status text, when this device first saw THAT text)
    current: HashMap<String, (String, f64)>,
    stable: HashMap<String, model::Stable>,
    opened: HashMap<String, Instant>,
    arts: HashMap<String, Art>,
    art_order: VecDeque<String>,
    art_miss: HashMap<String, Instant>,
    in_flight: HashSet<String>,
    veils: HashMap<(String, String), Option<f64>>,
    /// bumps when the session changes: work started for an older session never lands
    epoch: u64,
    stable_order: VecDeque<String>,
    current_order: VecDeque<String>,
}
static EPOCH: AtomicU64 = AtomicU64::new(0);
fn caches() -> &'static Mutex<Caches> {
    static C: OnceLock<Mutex<Caches>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(Caches::default()))
}
/// A poisoned lock (a panic elsewhere) still gives the data: the caches are rebuildable, never worth a cascade.
fn lock() -> std::sync::MutexGuard<'static, Caches> {
    caches().lock().unwrap_or_else(|p| p.into_inner())
}
fn now_ms() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as f64).unwrap_or(0.0)
}
fn fresh(c: &Caches, k: &str) -> bool {
    c.resolved.get(k).map(|r| r.at.elapsed() < r.ttl).unwrap_or(false)
}
/// A new account or catalogue starts every cache empty (nothing of one session leaks into the next).
fn check_scope(c: &mut Caches, scope: &str) {
    if c.scope != scope {
        *c = Caches { scope: scope.to_string(), epoch: EPOCH.fetch_add(1, Ordering::Relaxed) + 1, ..Default::default() };
    }
}
/// Drop the oldest half of a map when it passes `max` (a hard bound, whatever the entries' ages).
fn bound<K: Clone + std::hash::Hash + Eq, V>(m: &mut HashMap<K, V>, max: usize, age: impl Fn(&V) -> Instant) {
    if m.len() <= max {
        return;
    }
    let mut v: Vec<(K, Instant)> = m.iter().map(|(k, x)| (k.clone(), age(x))).collect();
    v.sort_by_key(|(_, t)| *t);
    for (k, _) in v.into_iter().take(m.len() - max / 2) {
        m.remove(&k);
    }
}
/// Removes a game's in-flight mark however its download ends (finished, failed, or cancelled with its view).
struct InFlight(String, u64);
impl Drop for InFlight {
    fn drop(&mut self) {
        let mut c = lock();
        if c.epoch == self.1 {
            c.in_flight.remove(&self.0);
        }
    }
}
/// At most one full-size decode + scan at a time, process-wide; the permit travels INTO the blocking job.
fn measuring() -> Arc<tokio::sync::Semaphore> {
    static S: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();
    S.get_or_init(|| Arc::new(tokio::sync::Semaphore::new(1))).clone()
}
async fn measure_one(bytes: Vec<u8>) -> Option<((i32, i32, Vec<model::Rgb>), Vec<u8>)> {
    let permit = measuring().acquire_owned().await.ok()?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        measure(&bytes).map(|m| (m, bytes))
    })
    .await
    .ok()
    .flatten()
}
/// A file's bytes, never reading more than `max` (a bigger or growing file reads as absent).
fn read_capped(path: &std::path::Path, max: usize) -> Option<Vec<u8>> {
    use std::io::Read;
    let f = std::fs::File::open(path).ok()?;
    let mut out = Vec::new();
    f.take(max as u64 + 1).read_to_end(&mut out).ok()?;
    (!out.is_empty() && out.len() <= max).then_some(out)
}
/// `$XDG_CACHE_HOME/TinHat/gamebg` (next to the link previews' cache).
fn cache_dir() -> PathBuf {
    crate::linkpreview::cache_dir().with_file_name("gamebg")
}
/// One file per (catalogue, id): two FNV-1a passes over the whole key (custom:a-b and custom:a_b never share a file,
/// and nothing depends on the file system's case rules).
fn cache_file(scope_base: &str, id: &str) -> PathBuf {
    let key = format!("{scope_base}\n{id}");
    let mut a: u64 = 0xcbf29ce484222325;
    let mut b: u64 = 0x84222325cbf29ce4;
    for byte in key.bytes() {
        a = (a ^ byte as u64).wrapping_mul(0x100000001b3);
        b = (b ^ (byte as u64).rotate_left(3)).wrapping_mul(0x100000001b3);
    }
    cache_dir().join(format!("bg-{a:016x}{b:016x}.art"))
}

fn veil_of(art: &Art, id: &str, ink: &Ink) -> Option<f64> {
    let key = (id.to_string(), format!("{}|{}|{}", ink.bg, ink.mode, ink.texts.join(",")));
    if let Some(v) = lock().veils.get(&key) {
        return *v;
    }
    let (floor, cap) = match ink.mode.as_str() {
        "light" => (model::VEIL_FLOOR_LIGHT, model::VEIL_CAP_LIGHT),
        "oled" => (model::VEIL_FLOOR_OLED, model::VEIL_CAP_OLED),
        _ => (model::VEIL_FLOOR_DARK, model::VEIL_CAP_DARK),
    };
    let v = model::veil_for(
        &model::VeilTheme { bg: &ink.bg, texts: &ink.texts, need: model::MIN_CONTRAST, target: model::FIT_CONTRAST, floor, cap },
        &art.cells,
    );
    let mut c = lock();
    if c.veils.len() > 256 {
        c.veils.clear();
    }
    c.veils.insert(key, v);
    v
}

impl qobject::GameArtController {
    fn changed(&self) {
        {
            let mut i = self.rust().inputs.lock().unwrap_or_else(|p| p.into_inner());
            i.gen += 1;
        }
        self.rust().wake.notify_one();
    }
    pub fn open(mut self: Pin<&mut Self>, room_id: &QString) {
        self.rust().preview_gen.fetch_add(1, Ordering::Relaxed);
        let room = room_id.to_string();
        self.as_mut().set_room_id(room_id.clone());
        self.rust().inputs.lock().unwrap_or_else(|p| p.into_inner()).room = room;
        self.as_mut().apply(None); // never show the previous room's art for a moment; the loop answers at once
        self.as_mut().ensure_task();
        self.changed();
    }
    pub fn set_background(self: Pin<&mut Self>, content_json: &QString) {
        let raw = content_json.to_string();
        let v = if raw.trim().is_empty() { None } else { serde_json::from_str::<Value>(&raw).ok() };
        self.rust().inputs.lock().unwrap_or_else(|p| p.into_inner()).bg = v;
        self.changed();
        self.clear_if_off();
    }
    /// Game art just stopped applying (a switch, the room's own background): clear at once, not after the next pass.
    fn clear_if_off(mut self: Pin<&mut Self>) {
        let i = self.rust().inputs.lock().unwrap_or_else(|p| p.into_inner()).clone();
        if model::art_mode(i.bg.as_ref(), None, i.backgrounds, i.game_art) != "game" && self.rust().wanted.is_some() {
            self.as_mut().apply(None);
        }
    }
    pub fn set_prefs(self: Pin<&mut Self>, backgrounds: bool, game_art: bool) {
        {
            let mut i = self.rust().inputs.lock().unwrap_or_else(|p| p.into_inner());
            i.backgrounds = backgrounds;
            i.game_art = game_art;
        }
        self.changed();
        self.clear_if_off();
    }
    pub fn set_ink(mut self: Pin<&mut Self>, bg: &QString, texts_json: &QString, mode: &QString) {
        let texts = json_strings(&texts_json.to_string());
        self.as_mut().rust_mut().theme_ink = Ink { bg: bg.to_string(), texts, mode: mode.to_string() };
        self.as_mut().reink();
    }
    pub fn set_role_colours(mut self: Pin<&mut Self>, json: &QString) {
        let roles = json_strings(&json.to_string());
        if roles == self.rust().roles {
            return;
        }
        self.as_mut().rust_mut().roles = roles;
        self.as_mut().reink();
    }
    fn reink(mut self: Pin<&mut Self>) {
        let t = &self.rust().theme_ink;
        let ink = ink_of(&t.bg, &t.texts, &self.rust().roles, &t.mode);
        self.as_mut().rust_mut().ink = ink;
        let wanted = self.rust().wanted.clone();
        self.as_mut().apply(wanted);
    }
    pub fn set_hidden(self: Pin<&mut Self>, hidden: bool) {
        self.rust().inputs.lock().unwrap_or_else(|p| p.into_inner()).hidden = hidden;
        self.changed();
    }
    pub fn preview(mut self: Pin<&mut Self>, file_path: &QString) {
        // a preview follows no room: the room loop must not clear what it shows
        if let Some(t) = self.as_mut().rust_mut().task.take() {
            t.abort();
        }
        let path = PathBuf::from(file_path.to_string());
        let id = format!("preview:{}", path.display());
        let qt = self.qt_thread();
        let gen = self.rust().preview_gen.fetch_add(1, Ordering::Relaxed) + 1;
        let gens = self.rust().preview_gen.clone();
        crate::bridge::runtime().spawn(async move {
            let bytes = read_capped(&path, MAX_DOWNLOAD);
            let ok_dims = bytes.as_deref().and_then(image_dims).map(|(w, h)| model::art_dims_ok(w, h)).unwrap_or(false);
            let measured = match bytes {
                Some(b) if ok_dims => measure_one(b).await.map(|(m, _)| m),
                _ => None,
            };
            if let Some((w, h, cells)) = measured {
                {
                    let mut c = lock();
                    c.arts.insert(id.clone(), Art { path, w, h, cells: Arc::new(cells) });
                    c.art_order.retain(|x| x != &id);
                    c.art_order.push_back(id.clone());
                    while c.art_order.len() > 32 {
                        if let Some(old) = c.art_order.pop_front() {
                            c.arts.remove(&old);
                        }
                    }
                }
                let _ = qt.queue(move |o| {
                    if gens.load(Ordering::Relaxed) == gen {
                        o.apply(Some(id)); // only the newest preview, and never over a room opened since
                    }
                });
            } else {
                eprintln!("[gamebg] preview: not an image I can measure: {}", path.display());
            }
        });
    }
    pub fn match_game_plan_json(&self, bg_kind: &QString, art_showing: bool) -> QString {
        let (plan, content) = model::match_game_plan(&bg_kind.to_string(), art_showing);
        QString::from(&json!({"plan": plan, "content": content}).to_string())
    }
    pub fn words_json(&self) -> QString {
        QString::from(WORDS)
    }
    pub fn fit_json(&self, art_w: f64, art_h: f64, area_w: f64, area_h: f64, dpr: f64) -> QString {
        let f = model::fit_art(art_w, art_h, area_w, area_h, dpr);
        QString::from(&json!({"w": f.w, "h": f.h, "x": f.x, "y": f.y, "fadeBottom": f.fade_bottom, "fadeSides": f.fade_sides}).to_string())
    }

    /// Show `game` (None = nothing): its measured art with its veil for this theme; art that can't be made readable here
    /// (no veil reaches 4.5:1) is not drawn.
    fn apply(mut self: Pin<&mut Self>, game: Option<String>) {
        self.as_mut().rust_mut().wanted = game.clone();
        let art = game.as_ref().and_then(|g| lock().arts.get(g).cloned());
        let veil = match (&game, &art) {
            (Some(g), Some(a)) => veil_of(a, g, &self.rust().ink),
            _ => None,
        };
        match (game, art, veil) {
            (Some(g), Some(a), Some(v)) => {
                self.as_mut().set_veil(v);
                self.as_mut().set_art_width(a.w);
                self.as_mut().set_art_height(a.h);
                let url = crate::platform::file_url(&a.path);
                self.as_mut().set_game_id(QString::from(&g));
                self.as_mut().set_art_url(QString::from(&url));
                self.as_mut().set_art_json(QString::from(&json!({"id": g, "url": url, "w": a.w, "h": a.h, "veil": v}).to_string()));
            }
            _ => {
                self.as_mut().set_art_url(QString::default());
                self.as_mut().set_game_id(QString::default());
                self.as_mut().set_art_json(QString::default());
            }
        }
    }

    fn ensure_task(mut self: Pin<&mut Self>) {
        if self.rust().task.as_ref().map(|t| !t.is_finished()).unwrap_or(false) {
            return;
        }
        let inputs = self.rust().inputs.clone();
        let wake = self.rust().wake.clone();
        let qt = self.qt_thread();
        let h = crate::bridge::runtime().spawn(run(inputs, wake, qt));
        self.as_mut().rust_mut().task = Some(h);
    }
}

/// The per-view loop: a pass on every change and then every POLL (or sooner for a stabiliser step); while hidden it
/// sleeps until something changes.
async fn run(inputs: Arc<Mutex<Inputs>>, wake: Arc<tokio::sync::Notify>, qt: CxxQtThread<qobject::GameArtController>) {
    loop {
        let inp = inputs.lock().unwrap_or_else(|p| p.into_inner()).clone();
        if inp.hidden || inp.room.is_empty() {
            wake.notified().await; // hidden, or following no room (a closed sheet, a preview): asleep until a change
            continue;
        }
        let mut wait = POLL;
        match step(&inp).await {
            // the epoch the pass ran under (taken right after its session check): a pass that straddles a newer
            // session never lands
            Ok((shown, next, epoch)) => {
                if let Some(n) = next {
                    wait = wait.min(n + Duration::from_millis(50));
                }
                let (room, gen) = (inp.room.clone(), inp.gen);
                let cur = inputs.clone();
                let ok = qt.queue(move |o| {
                    let now = cur.lock().unwrap_or_else(|p| p.into_inner()).clone();
                    if lock().epoch != epoch {
                        return;
                    }
                    let differs = o.rust().wanted != shown || (shown.is_some() && o.art_url().is_empty());
                    if now.gen == gen && !now.hidden && o.room_id().to_string() == room && differs {
                        o.apply(shown);
                    }
                });
                if ok.is_err() {
                    return; // the view is gone
                }
            }
            Err(e) => eprintln!("[gamebg] {e}"),
        }
        let _ = tokio::time::timeout(wait, wake.notified()).await;
    }
}

/// One pass: inputs → resolve → pick → stabilise → (download + measure the art). → (shown game, next timer).
async fn step(inp: &Inputs) -> Result<(Option<String>, Option<Duration>, u64), String> {
    if inp.room.is_empty()
        || model::art_mode(inp.bg.as_ref(), None, inp.backgrounds, inp.game_art) != "game"
        || !crate::server_offer::feature_on_global(crate::server_offer::ServerFeature::Games)
    {
        return Ok((None, None, lock().epoch));
    }
    let client = crate::session::client().ok_or("not signed in")?;
    let base = crate::games::default_catalog_base();
    let scope = format!(
        "{}|{}|{base}",
        client.user_id().map(|u| u.to_string()).unwrap_or_default(),
        client.device_id().map(|d| d.to_string()).unwrap_or_default()
    );
    let epoch = {
        let mut c = lock();
        check_scope(&mut c, &scope);
        c.opened.entry(inp.room.clone()).or_insert_with(Instant::now);
        bound(&mut c.opened, 500, |t| *t);
        c.epoch
    };
    let rid = matrix_sdk::ruma::RoomId::parse(&inp.room).map_err(|e| e.to_string())?;
    let room = client.get_room(&rid).ok_or("no such room")?;
    let members: Vec<String> = room
        .members_no_sync(matrix_sdk::RoomMemberships::JOIN)
        .await
        .map_err(|e| e.to_string())?
        .iter()
        .map(|m| m.user_id().to_string())
        .collect();
    let callers = crate::rtc::call_participants(&room).await;
    let act = crate::appstate::get_state(&room, "dev.tinhat.activity", "").await;
    let statuses = crate::presence::shared_status_texts();
    let presence_loaded = crate::presence::presence_loaded();
    let now = now_ms();

    let mut claims = Vec::new();
    {
        let mut c = lock();
        if c.epoch != epoch {
            return Ok((None, None, epoch)); // another session started meanwhile
        }
        for u in &members {
            let Some(text) = statuses.get(u).filter(|t| !t.trim().is_empty()) else {
                if c.current.remove(u).is_some() {
                    c.current_order.retain(|x| x != u); // a cleared status: the next one starts a new clock
                }
                continue;
            };
            let at = match c.current.get(u) {
                Some((t, at)) if t == text => *at,
                _ => {
                    c.current.insert(u.clone(), (text.clone(), now));
                    c.current_order.retain(|x| x != u);
                    c.current_order.push_back(u.clone());
                    now
                }
            };
            claims.push(model::Claim { user: u.clone(), text: text.clone(), at });
        }
        while c.current.len() > 5000 {
            // the web's Map: the oldest entry goes first, one at a time
            match c.current_order.pop_front() {
                Some(old) => {
                    c.current.remove(&old);
                }
                None => break,
            }
        }
    }
    let activity = act.as_ref().and_then(|a| {
        let g = a.get("game").and_then(Value::as_str).unwrap_or("");
        (!g.trim().is_empty()).then(|| (g.to_string(), a.get("ts").and_then(Value::as_f64).unwrap_or(0.0)))
    });
    let texts: Vec<String> = claims.iter().map(|c| c.text.clone()).chain(activity.iter().map(|a| a.0.clone())).collect();
    let keys: Vec<String> = texts.iter().map(|t| model::game_key(t)).filter(|k| !k.is_empty()).collect();

    // look up the names this device hasn't (≤ 20 a request; a local lookup on the person's own server)
    let missing: Vec<(String, String)> = {
        let c = lock();
        let mut seen = HashSet::new();
        texts
            .iter()
            .filter_map(|t| {
                let k = model::game_key(t);
                (!k.is_empty() && !fresh(&c, &k) && seen.insert(k.clone())).then(|| (k, t.clone()))
            })
            .take(20)
            .collect()
    };
    if !missing.is_empty() {
        let token = client.access_token();
        let names: Vec<&str> = missing.iter().map(|(_, t)| t.as_str()).collect();
        let res = crate::games::catalog_post(&base, "/resolve", json!({ "names": names }), token.as_deref()).await;
        let mut c = lock();
        if c.epoch == epoch {
            for (k, t) in &missing {
                let (id, ttl) = match &res {
                    Ok(v) => match v.get("ids").and_then(|m| m.get(t)).and_then(Value::as_str) {
                        Some(id) => (Some(id.to_string()), HIT_TTL),
                        None => (None, MISS_TTL),
                    },
                    Err(_) => (None, FAIL_RETRY),
                };
                c.resolved.insert(k.clone(), Resolved { id, at: Instant::now(), ttl });
            }
            c.resolved.retain(|_, r| r.at.elapsed() < r.ttl);
            bound(&mut c.resolved, 2000, |r| r.at);
        }
    }

    let st = {
        let mut c = lock();
        if c.epoch != epoch {
            return Ok((None, None, epoch));
        }
        let st0 = c.stable.get(&inp.room).cloned();
        if st0.is_none() {
            // the first look waits until the names are answered, and (a few seconds at most) for the presence list
            if keys.iter().any(|k| !c.resolved.contains_key(k)) {
                return Ok((None, Some(Duration::from_secs(1)), epoch));
            }
            let waited = c.opened.get(&inp.room).map(|o| o.elapsed() >= READY_WAIT).unwrap_or(true);
            if !presence_loaded && !waited {
                return Ok((None, Some(READY_WAIT), epoch));
            }
        }
        let ids = keys.iter().map(|k| (k.clone(), c.resolved.get(k).and_then(|r| r.id.clone()))).collect();
        let pick = model::pick_game(&model::PickIn { members, callers, claims, activity, ids });
        let st = model::step_stable(st0.as_ref(), pick.game.as_deref(), now);
        if c.stable.insert(inp.room.clone(), st.clone()).is_none() {
            c.stable_order.push_back(inp.room.clone());
        }
        while c.stable.len() > 500 {
            // the oldest room first (never the one in view)
            match c.stable_order.pop_front() {
                Some(old) if old != inp.room => {
                    c.stable.remove(&old);
                }
                Some(old) => c.stable_order.push_back(old),
                None => break,
            }
        }
        st
    };
    let next = model::next_step_in(Some(&st), now).map(|ms| Duration::from_millis(ms as u64));
    if let Some(g) = &st.shown {
        ensure_art(&base, epoch, g).await;
    }
    Ok((st.shown, next, epoch))
}

/// Download (once, streamed with the cap) + check the header size + decode + measure (once, one at a time), and only
/// then publish into the cache. A 404 / refusal / failure is remembered for 10 min. One in flight per game.
async fn ensure_art(base: &str, epoch: u64, id: &str) {
    let _flight = {
        let mut c = lock();
        if c.epoch != epoch {
            return;
        }
        if let Some(a) = c.arts.get(id) {
            if a.path.exists() {
                return;
            }
            c.arts.remove(id); // pruned from disk meanwhile: fetch it again
        }
        if c.art_miss.get(id).map(|t| t.elapsed() < MISS_TTL).unwrap_or(false) || !c.in_flight.insert(id.to_string()) {
            return;
        }
        InFlight(id.to_string(), epoch)
    };
    let file = cache_file(base, id);
    let cached = read_capped(&file, MAX_DOWNLOAD);
    let from_cache = cached.is_some();
    let bytes = match cached {
        Some(b) => Some(b),
        None => fetch(&model::background_url(base, id)).await,
    };
    let measured = match bytes {
        Some(b) if image_dims(&b).map(|(w, h)| model::art_dims_ok(w, h)).unwrap_or(false) => measure_one(b).await,
        _ => None,
    };
    let published = match measured {
        Some(((w, h, cells), b)) => {
            let ok = from_cache || {
                let _ = std::fs::create_dir_all(cache_dir());
                let tmp = file.with_extension(format!("part-{}-{}", std::process::id(), SEQ.fetch_add(1, Ordering::Relaxed)));
                let wrote = std::fs::write(&tmp, &b).is_ok() && std::fs::rename(&tmp, &file).is_ok();
                if !wrote {
                    let _ = std::fs::remove_file(&tmp);
                }
                wrote
            };
            ok.then(|| Art { path: file.clone(), w, h, cells: Arc::new(cells) })
        }
        None => {
            if from_cache {
                let _ = std::fs::remove_file(&file); // a corrupt cache file: gone, fetched again next time
            }
            None
        }
    };
    let mut c = lock();
    if c.epoch != epoch {
        return; // another session meanwhile
    }
    match published {
        Some(a) => {
            c.arts.insert(id.to_string(), a);
            c.art_order.retain(|x| x != id);
            c.art_order.push_back(id.to_string());
            while c.art_order.len() > 32 {
                if let Some(old) = c.art_order.pop_front() {
                    c.arts.remove(&old);
                }
            }
            c.art_miss.remove(id);
            drop(c);
            prune_disk();
        }
        None => {
            c.art_miss.insert(id.to_string(), Instant::now());
            c.art_miss.retain(|_, t| t.elapsed() < MISS_TTL);
            bound(&mut c.art_miss, 2000, |t| *t);
        }
    }
}
static SEQ: AtomicU64 = AtomicU64::new(0);

/// Keep the newest DISK_KEEP art files.
fn prune_disk() {
    let Ok(rd) = std::fs::read_dir(cache_dir()) else { return };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = rd
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().ends_with(".art"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    if files.len() <= DISK_KEEP {
        return;
    }
    files.sort();
    for (_, p) in files.iter().take(files.len() - DISK_KEEP) {
        let _ = std::fs::remove_file(p);
    }
}

/// GET with the cap enforced while reading (a refused or oversized answer is dropped at once).
async fn fetch(url: &str) -> Option<Vec<u8>> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return None;
    }
    let mut r = crate::games::http().get(url).send().await.ok()?;
    if r.status().as_u16() != 200 || r.content_length().map(|n| n as usize > MAX_DOWNLOAD).unwrap_or(false) {
        return None;
    }
    let mut out = Vec::new();
    loop {
        match r.chunk().await {
            Ok(Some(chunk)) => {
                if out.len() + chunk.len() > MAX_DOWNLOAD {
                    return None;
                }
                out.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            Err(_) => return None, // a reset mid-body is not a (shorter) image
        }
    }
    Some(out)
}

/// The image's size from its header, before anything is decoded (JPEG SOFn, PNG IHDR, WebP VP8/VP8L/VP8X).
fn image_dims(b: &[u8]) -> Option<(u32, u32)> {
    if b.len() >= 24 && b[..8] == [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a] {
        return Some((u32::from_be_bytes(b[16..20].try_into().ok()?), u32::from_be_bytes(b[20..24].try_into().ok()?)));
    }
    if b.len() >= 30 && &b[..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        return match &b[12..16] {
            b"VP8X" => Some((1 + (b[24] as u32 | (b[25] as u32) << 8 | (b[26] as u32) << 16), 1 + (b[27] as u32 | (b[28] as u32) << 8 | (b[29] as u32) << 16))),
            b"VP8L" => {
                let v = u32::from_le_bytes(b[21..25].try_into().ok()?);
                Some((1 + (v & 0x3fff), 1 + ((v >> 14) & 0x3fff)))
            }
            b"VP8 " => Some(((u16::from_le_bytes(b[26..28].try_into().ok()?) & 0x3fff) as u32, (u16::from_le_bytes(b[28..30].try_into().ok()?) & 0x3fff) as u32)),
            _ => None,
        };
    }
    if b.len() >= 4 && b[0] == 0xff && b[1] == 0xd8 {
        let mut i = 2;
        while i + 9 < b.len() {
            if b[i] != 0xff {
                return None;
            }
            let m = b[i + 1];
            if m == 0xd8 || (0xd0..=0xd7).contains(&m) || m == 0x01 || m == 0xff {
                i += if m == 0xff { 1 } else { 2 };
                continue;
            }
            let len = u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize;
            if (0xc0..=0xcf).contains(&m) && m != 0xc4 && m != 0xc8 && m != 0xcc {
                return Some((u16::from_be_bytes([b[i + 7], b[i + 8]]) as u32, u16::from_be_bytes([b[i + 5], b[i + 6]]) as u32));
            }
            i += 2 + len;
        }
    }
    None
}

/// Decode with Qt and read EVERY pixel into the bound cells (model::bound_pixels: the 5×5 neighbourhoods) → (w, h, cells).
fn measure(bytes: &[u8]) -> Option<(i32, i32, Vec<model::Rgb>)> {
    let img = cxx_qt_lib::QImage::from_data(bytes, None)?;
    let (w, h) = (img.width(), img.height());
    if w <= 0 || h <= 0 || !model::art_dims_ok(w as u32, h as u32) {
        return None;
    }
    let mut rgba = Vec::with_capacity((w as usize) * (h as usize) * 4);
    for y in 0..h {
        for x in 0..w {
            let c = img.pixel_color(x, y);
            rgba.extend_from_slice(&[(c.red() & 255) as u8, (c.green() & 255) as u8, (c.blue() & 255) as u8, 255]);
        }
    }
    Some((w, h, model::bound_pixels(&rgba, w as usize)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_app_property_the_ambience_and_game_art_qml_reads_exists_on_main() {
        // r6: these files read the app root (Main.qml) as `app` / `pane.app` / `conv.app` / `dlg.app`; a name Main.qml
        // doesn't declare is `undefined` there: a QML error at every start (empty profile included) and a setting that
        // shows the wrong state or does nothing ("Game art backgrounds"). AppCore-only properties need an alias.
        let main = include_str!("../qml/Main.qml");
        let window_builtins = ["close", "show", "raise", "requestActivate", "width", "height", "visible", "visibility", "active", "screen", "title"];
        let uses = regex::Regex::new(r"(?:^|[^\w.])(?:(?:pane|conv|dlg|gl|root|sheet|menu)\.)?app\.([A-Za-z_]\w*)").unwrap();
        let mut missing = Vec::new();
        for (file, src) in [
            ("AppearancePane.qml", include_str!("../qml/AppearancePane.qml")),
            ("BackgroundSheet.qml", include_str!("../qml/BackgroundSheet.qml")),
            ("ConversationView.qml", include_str!("../qml/ConversationView.qml")),
            ("AmbienceLayer.qml", include_str!("../qml/AmbienceLayer.qml")),
            ("GameArtLayer.qml", include_str!("../qml/GameArtLayer.qml")),
            ("ParticlesMenu.qml", include_str!("../qml/ParticlesMenu.qml")),
        ] {
            let mut names: Vec<&str> = uses.captures_iter(src).map(|c| c.get(1).unwrap().as_str()).collect();
            names.sort();
            names.dedup();
            for n in names {
                let decl = regex::Regex::new(&format!(r"(property\s+(alias|[\w.<>]+)\s+{n}\b|function\s+{n}\s*\(|signal\s+{n}\b|\bid:\s*{n}\b)")).unwrap();
                if !decl.is_match(main) && !window_builtins.contains(&n) {
                    missing.push(format!("{file}: app.{n}"));
                }
            }
        }
        assert!(missing.is_empty(), "read as app.* but not on Main.qml: {missing:?}");
    }

    #[test]
    fn gamebg_role_colours_reach_the_veil() {
        // 3-2: the controller's ink = the theme's texts + the room's role colours; a #808080 role name on white art under
        // OLED makes the veil 0.92 (without it 0.90 ≈ 4.4:1 for that name).
        let own: Vec<String> = ["#b5bac1", "#80848e", "#949ba4", "#e57373", "#64b5f6", "#81c784"].map(String::from).to_vec();
        let white = Art { path: PathBuf::new(), w: 1, h: 1, cells: Arc::new(vec![[255.0, 255.0, 255.0]]) };
        let without = ink_of("#000000", &own, &[], "oled");
        let with = ink_of("#000000", &own, &["#808080".to_string()], "oled");
        assert!(with.texts.contains(&"#808080".to_string()));
        assert_eq!(veil_of(&white, "test:3-2-a", &without), Some(0.9));
        assert_eq!(veil_of(&white, "test:3-2-b", &with), Some(0.92));
    }

    #[test]
    fn gamebg_image_dims_from_headers() {
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13, b'I', b'H', b'D', b'R'];
        png.extend_from_slice(&3840u32.to_be_bytes());
        png.extend_from_slice(&1240u32.to_be_bytes());
        assert_eq!(image_dims(&png), Some((3840, 1240)));
        // JPEG: SOI, an APP0 segment, then SOF0 with height 1240, width 3840
        let mut jpg = vec![0xff, 0xd8, 0xff, 0xe0, 0x00, 0x04, 0x00, 0x00, 0xff, 0xc0, 0x00, 0x11, 0x08];
        jpg.extend_from_slice(&1240u16.to_be_bytes());
        jpg.extend_from_slice(&3840u16.to_be_bytes());
        jpg.extend_from_slice(&[0; 12]);
        assert_eq!(image_dims(&jpg), Some((3840, 1240)));
        // a decompression bomb's header is caught before decoding (> 16 MP)
        let mut bomb = png[..16].to_vec();
        bomb.extend_from_slice(&30000u32.to_be_bytes());
        bomb.extend_from_slice(&30000u32.to_be_bytes());
        let (w, h) = image_dims(&bomb).unwrap();
        assert!(!model::art_dims_ok(w, h));
        assert_eq!(image_dims(b"<html>not an image</html> padding padding"), None);
    }

    #[test]
    fn gamebg_cache_files_never_collide() {
        let a = cache_file("https://h.example/api/games", "custom:a-b");
        let b = cache_file("https://h.example/api/games", "custom:a_b");
        let c = cache_file("https://other.example/api/games", "custom:a-b");
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_eq!(a, cache_file("https://h.example/api/games", "custom:a-b"));
    }

}
