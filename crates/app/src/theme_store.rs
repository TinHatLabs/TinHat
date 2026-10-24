//! theme_store.rs — your theme across your devices (themes step 1b; contract THEMES-SYNC.md, THEMES-UI.md): the
//! account's choice `dev.tinhat.theme`, My themes `dev.tinhat.themes`, this device's own choice ("Use a different
//! theme on this device") and what the device renders from them. The rules are the engine crate's port of the web
//! reference (`tinhat_theme::sync`, 137 shared vectors); this file is the plumbing around them: the per-account state
//! on disk, the account-data reads at the first sync and the events of every later sync (bridge.rs → `on_sync`), the
//! PUTs in the background with retries, the notices, and the view ThemeController (theme_ctl.rs) hands QML.
//!
//! What a device renders (§8): this run's test override (`TINHAT_THEME`) → the local choice (override on) → the
//! account's choice → the legacy choice (the pre-11.0 `theme` file: Goose → Goose + its sounds, else OLED). A choice
//! that names a theme this version cannot render (§3.4 `none`) keeps what is rendered now. Nothing is written to the
//! account until the person applies something (a theme, a sound pack, an icon style, a My-themes change).
//!
//! Gate mode (`TINHAT_GUI_THEMESYNC=0`, §9 — the gates run on a test account, which must never get these keys): nothing
//! is read or written; a theme pick changes only the legacy `theme` file (bridge.rs `set_theme`, as in 10.6, called by
//! QML) and accepts any theme id; sounds, icon style and My themes live for the run only.
//!
//! The ☰ Theme menu's recent themes (THEMES-UI §12, `tinhat_theme::recent`): per device, the ui pref file
//! `ui-theme-recent` (the web's pref `theme-recent`), updated by a person's action that changes the theme on screen —
//! `apply_theme` (the ☰ rows, the cards, the gallery, My themes → Use, import "Save and use"), `undo`, `keep_here`; a
//! theme another device chose is shown by the menu (`recent_rows`) but never recorded. The runs whose theme is for the
//! run only (gate mode, `TINHAT_THEME`) keep their recents in memory, so every such launch starts from the seed.
//!
//! Locking: one engine mutex. Nothing called under it reaches theme_rt.rs (which calls `doc()` here) — the projection
//! uses the library and the engine crate directly.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use matrix_sdk::ruma::events::{AnyGlobalAccountDataEvent, GlobalAccountDataEventType};
use matrix_sdk::ruma::serde::Raw;
use matrix_sdk::Client;
use serde_json::{json, Value};
use tinhat_theme::sync::{self as ts, ChoiceState, JObj, ThemesState};

/// Every account-data round trip is bounded.
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
/// THEMES-SYNC §6: a PUT still pending this long shows "Saved on this device — it syncs when you're back online."
const PENDING_LINE_AFTER: Duration = Duration::from_secs(10);

/// Sync on (the default) or gate mode (`TINHAT_GUI_THEMESYNC=0`).
pub fn sync_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("TINHAT_GUI_THEMESYNC").map(|v| v.trim() != "0").unwrap_or(true))
}

/// `TINHAT_THEME`: this run renders that theme (screenshots, gates) — never saved, never synced.
fn env_theme() -> Option<String> {
    std::env::var("TINHAT_THEME").ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn now_ms() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as f64).unwrap_or(0.0)
}

// ---- the engine ---------------------------------------------------------------------------------------------------

/// Where the wanted content comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// `TINHAT_THEME`, gate mode or signed out: this run only
    Session,
    /// the device override (or an apply while the account choice is read-only)
    Local,
    /// the account's choice
    Account,
    /// nothing chosen yet: the pre-11.0 setting
    Legacy,
}

impl Origin {
    fn as_str(self) -> &'static str {
        match self {
            Origin::Session => "session",
            Origin::Local => "local",
            Origin::Account => "account",
            Origin::Legacy => "legacy",
        }
    }
}

struct Engine {
    /// session generation this state belongs to (crate::session::generation)
    gen: u64,
    user: String,
    device: String,
    /// the first sync of this session read both documents
    synced: bool,
    choice: ChoiceState,
    themes: ThemesState,
    override_on: bool,
    /// the local choice content (§7); also where applies go while the account choice is read-only
    local: Option<JObj>,
    /// the sound-pack offer shown inline in the settings (THEMES-UI §10.6), until answered or the sheet closes
    offer: Option<Offer>,
    /// the device had theme state when this session attached (its sync state, a last-shown theme, a 10.x theme): the
    /// first read may announce another device's change; a fresh sign-in just takes the account's theme (§3.5)
    had_state: bool,
    /// this run's own content (TINHAT_THEME, gate mode, signed out)
    session: Option<JObj>,
    /// the content rendered now (after every fallback) and its projection
    rendered: JObj,
    proj: ts::ChoiceProjection,
    /// the content shown before the last apply (theme, sounds, icon style), for Undo (THEMES-UI §3.2); cleared by an
    /// Undo, the device override and the Settings sheet opening or closing (end_session)
    undo: Option<JObj>,
    /// when a save started waiting for its PUT (per key)
    choice_pending: Option<Instant>,
    themes_pending: Option<Instant>,
    /// the next PUT is a re-assert: it waits 0–2 s by the device id (§6)
    choice_reassert: bool,
    themes_reassert: bool,
    /// retry attempt per key (§6)
    choice_attempt: i64,
    themes_attempt: i64,
    /// a PUT worker is running (per key)
    choice_worker: bool,
    themes_worker: bool,
    /// notices for QML: (kind, text, action text, action payload)
    notices: Vec<(String, String, String, String)>,
    /// bumps on every change QML should re-read
    rev: u64,
    /// bumps on every apply of a person's on this device (apply_action / undo — the web's appliedListeners): a real
    /// apply ends a share card's Try without going back (THEMES-UI §14.4)
    applied: u64,
}

impl Engine {
    fn fresh(gen: u64) -> Engine {
        let session = if sync_enabled() { env_theme().map(|id| content_for(&id)) } else { Some(gate_content()) };
        let rendered = session.clone().unwrap_or_else(legacy_content);
        let proj = project(&rendered);
        Engine {
            gen,
            user: String::new(),
            device: String::new(),
            synced: false,
            choice: ChoiceState::default(),
            themes: ThemesState::default(),
            override_on: false,
            local: None,
            offer: None,
            had_state: false,
            session,
            rendered,
            proj,
            undo: None,
            choice_pending: None,
            themes_pending: None,
            choice_reassert: false,
            themes_reassert: false,
            choice_attempt: 0,
            themes_attempt: 0,
            choice_worker: false,
            themes_worker: false,
            notices: Vec::new(),
            rev: 0,
            applied: 0,
        }
    }

    /// A fresh state for another session, keeping what the device renders now (no flash while the state loads).
    fn reset_for(&mut self, gen: u64) {
        let (rendered, proj, session, applied) = (self.rendered.clone(), self.proj.clone(), self.session.clone(), self.applied);
        *self = Engine { rendered, proj, session, applied, ..Engine::fresh(gen) };
    }
}

fn engine() -> &'static Mutex<Engine> {
    static E: OnceLock<Mutex<Engine>> = OnceLock::new();
    E.get_or_init(|| {
        let mut e = Engine::fresh(crate::session::generation());
        if e.session.is_none() {
            if let Some(boot) = read_boot() {
                e.proj = project(&boot);
                e.rendered = boot;
            }
        }
        Mutex::new(e)
    })
}

fn lock() -> std::sync::MutexGuard<'static, Engine> {
    engine().lock().unwrap_or_else(|p| p.into_inner())
}

/// A theme id as a content: the id, Goose's sounds for Goose, the default icon style (THEMES-SYNC §8's legacy mapping,
/// for any id).
fn content_for(id: &str) -> JObj {
    let mut c = JObj::new();
    c.insert("theme".into(), Value::String(id.to_string()));
    c.insert("sounds".into(), Value::String(if id == "goose" { "goose" } else { "default" }.into()));
    c.insert("icons".into(), Value::String("duotone".into()));
    c
}

fn saved_theme_file() -> Option<String> {
    if cfg!(test) {
        return None; // unit tests never read the builder's own data dir
    }
    std::fs::read_to_string(crate::platform::app_data_dir().join("theme")).map(|s| s.trim().to_string()).ok()
}

/// The legacy choice (§8): the pre-11.0 `theme` file — Goose → Goose + its sounds, anything else → OLED.
fn legacy_content() -> JObj {
    ts::legacy_choice(saved_theme_file().as_deref())
}

/// Gate mode (and the screenshot hook): `TINHAT_THEME`, else the saved file naming ANY theme the runtime can render
/// without this store (built-in, library, `<data>/themes/<id>.json`), else OLED.
fn gate_content() -> JObj {
    let id = env_theme()
        .or_else(saved_theme_file)
        .filter(|id| {
            tinhat_theme::builtin_theme(id).is_some()
                || crate::theme_lib::doc(id).is_some()
                || crate::theme_rt::load_file_doc(id, &crate::theme_rt::themes_dir()).is_ok()
        })
        .unwrap_or_else(|| crate::theme_rt::FALLBACK.to_string());
    content_for(&id)
}

// ---- rendering ----------------------------------------------------------------------------------------------------

/// A library theme's document (OLED and Goose included).
fn library(id: &str) -> Option<&'static Value> {
    crate::theme_lib::doc(id)
}

fn resolves(t: &JObj) -> bool {
    tinhat_theme::resolve_theme(&Value::Object(t.clone()), tinhat_theme::app_tokens()).is_ok()
}

/// THEMES-SYNC §3.4 for this app. A content naming a theme that is neither in the library nor carried as `doc` may
/// still be a gate-mode file theme (`<data>/themes/<id>.json`): rendered from the file.
pub fn project(content: &JObj) -> ts::ChoiceProjection {
    let mut p = ts::project_choice(content, library, resolves);
    if p.source == ts::ChoiceSource::None && !sync_enabled() {
        if let Some(id) = p.theme_id.clone() {
            if let Ok(Value::Object(d)) = crate::theme_rt::load_file_doc(&id, &crate::theme_rt::themes_dir()) {
                if resolves(&d) {
                    p.source = ts::ChoiceSource::Doc;
                    p.theme = Some(d);
                }
            }
        }
    }
    p
}

/// The content the device should render now, and where it comes from (before the projection's fallback).
fn wanted(e: &Engine) -> (JObj, Origin) {
    if let Some(s) = &e.session {
        return (s.clone(), Origin::Session);
    }
    if let Some(l) = &e.local {
        if e.override_on || e.choice.read_only {
            return (l.clone(), Origin::Local);
        }
    }
    if e.choice.read_only {
        if let Some(Value::Object(raw)) = &e.choice.raw {
            return (ts::choice_content(raw), Origin::Account);
        }
    }
    if let Some(k) = &e.choice.known {
        return (ts::choice_content(k), Origin::Account);
    }
    (legacy_content(), Origin::Legacy)
}

fn canon(c: &JObj) -> String {
    tinhat_theme::canon_json(&Value::Object(c.clone()))
}

/// Recompute what is rendered: the wanted content when it renders, else what is rendered now (§3.4 `none`).
fn rerender(e: &mut Engine) -> bool {
    let (want, _) = wanted(e);
    if canon(&want) == canon(&e.rendered) {
        return false;
    }
    let p = project(&want);
    if p.source == ts::ChoiceSource::None {
        return false; // keep rendering what is shown; ui_state says why
    }
    e.rendered = want;
    e.proj = p;
    e.rev += 1;
    if sync_enabled() && e.session.is_none() {
        write_boot(&e.rendered);
    }
    true
}

/// The rendered view.
#[derive(Clone, Debug)]
pub struct View {
    /// the theme id to render
    pub theme: String,
    /// its document when it is not the plain library / built-in one (a My theme, a kept theme, overrides laid over)
    pub doc: Option<JObj>,
    /// `default` | `goose`
    pub sounds: &'static str,
    /// `duotone` | `mono`
    pub icons: &'static str,
}

fn view_of(p: &ts::ChoiceProjection) -> View {
    let theme = p.theme_id.clone().unwrap_or_else(|| crate::theme_rt::FALLBACK.to_string());
    let plain = p.source == ts::ChoiceSource::Library && p.overrides != ts::Overrides::Applied;
    View { theme, doc: if plain { None } else { p.theme.clone() }, sounds: p.sounds, icons: p.icons }
}

/// What the device renders now.
pub fn view() -> View {
    view_of(&lock().proj)
}

/// The rendered theme's own document when `id` is the rendered theme and it is not the plain library / built-in
/// document (theme_rt.rs resolves it before anything else).
pub fn rendered_doc(id: &str) -> Option<Value> {
    let e = lock();
    let v = view_of(&e.proj);
    (v.theme == id).then_some(v.doc).flatten().map(Value::Object)
}

/// A theme document for the runtime (theme_rt.rs `load_doc`): the rendered theme's own document when it is not a
/// library one, else one of My themes by id.
pub fn doc(id: &str) -> Option<Value> {
    if let Some(d) = rendered_doc(id) {
        return Some(d);
    }
    let e = lock();
    my_theme_doc(&e, id).map(Value::Object)
}

fn my_themes_doc(e: &Engine) -> JObj {
    ts::merge_themes(e.themes.raw.as_ref(), Some(&e.themes.known), None).0
}

/// My themes as the UI lists them (THEMES-SYNC §4.3: newest first).
fn my_list(e: &Engine) -> Vec<JObj> {
    let d = Value::Object(my_themes_doc(e));
    ts::list_themes(Some(&d))
}

fn my_theme_doc(e: &Engine, id: &str) -> Option<JObj> {
    my_list(e).into_iter().find(|x| x.get("id").and_then(Value::as_str) == Some(id)).and_then(|x| x.get("theme").and_then(Value::as_object).cloned())
}

/// The theme this device starts with (colorscheme.rs writes its KDE scheme before KColorSchemeManager starts).
pub fn start_theme() -> String {
    view().theme
}

// ---- persistence (per account; cleared at sign-out) ---------------------------------------------------------------

fn dir() -> PathBuf {
    crate::platform::app_data_dir().join("themesync")
}

fn state_file(user: &str) -> PathBuf {
    let safe: String = user.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '_' }).collect();
    dir().join(format!("{safe}.json"))
}

fn state_json(e: &Engine) -> Value {
    json!({
        "v": 1,
        "user": e.user,
        "choice": {
            "known": e.choice.known.clone().map(Value::Object),
            "raw": e.choice.raw.clone(),
            "readOnly": e.choice.read_only,
            "wantPut": e.choice.want_put,
        },
        "themes": {
            "known": Value::Object(e.themes.known.iter().map(|(k, v)| (k.clone(), v.clone())).collect()),
            "raw": e.themes.raw.clone(),
            "readOnly": e.themes.read_only,
            "wantPut": e.themes.want_put,
            "justSaved": e.themes.just_saved.clone(),
        },
        "override": { "on": e.override_on, "local": e.local.clone().map(Value::Object) },
    })
}

fn save_state(e: &Engine) {
    if !sync_enabled() || e.user.is_empty() {
        return;
    }
    let _ = std::fs::create_dir_all(dir());
    let path = state_file(&e.user);
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, state_json(e).to_string()).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

/// This account's state from disk; true when there was one.
fn load_state(e: &mut Engine) -> bool {
    let Ok(text) = std::fs::read_to_string(state_file(&e.user)) else { return false };
    let Ok(v) = tinhat_theme::parse_json(&text) else { return false };
    if v.get("user").and_then(Value::as_str) != Some(e.user.as_str()) {
        return false;
    }
    let c = &v["choice"];
    e.choice.known = c.get("known").and_then(Value::as_object).cloned();
    e.choice.raw = c.get("raw").filter(|r| !r.is_null()).cloned();
    e.choice.read_only = c["readOnly"].as_bool().unwrap_or(false);
    e.choice.want_put = c["wantPut"].as_bool().unwrap_or(false);
    let t = &v["themes"];
    e.themes.known =
        t.get("known").and_then(Value::as_object).map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect()).unwrap_or_default();
    e.themes.raw = t.get("raw").filter(|r| !r.is_null()).cloned();
    e.themes.read_only = t["readOnly"].as_bool().unwrap_or(false);
    e.themes.want_put = t["wantPut"].as_bool().unwrap_or(false);
    e.themes.just_saved = t.get("justSaved").and_then(Value::as_str).map(str::to_string);
    e.override_on = v["override"]["on"].as_bool().unwrap_or(false);
    e.local = v["override"].get("local").and_then(Value::as_object).cloned();
    if e.choice.want_put {
        e.choice_pending = Some(Instant::now());
    }
    if e.themes.want_put {
        e.themes_pending = Some(Instant::now());
    }
    true
}

// ---- the sound-pack offers this device has shown (per device, like the web's pref `theme-offers`) ------------------

/// THEMES-UI §10.6: the offer made when a theme suggests another sound pack. Shown once per theme id and pack per
/// device: `<data>/themesync/offers.json` (not synced, not cleared at sign-out; the last 200).
#[derive(Clone, Debug)]
pub struct Offer {
    pub pack: &'static str,
    pub theme_id: String,
    pub theme_name: String,
}

impl Offer {
    fn key(&self) -> String {
        format!("{}:sounds:{}", self.theme_id, self.pack)
    }
}

fn offers_file() -> PathBuf {
    dir().join("offers.json")
}

fn offers_seen() -> Vec<String> {
    if cfg!(test) {
        return Vec::new();
    }
    let text = std::fs::read_to_string(offers_file()).unwrap_or_default();
    tinhat_theme::parse_json(&text)
        .ok()
        .and_then(|v| v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()))
        .unwrap_or_default()
}

fn mark_offered(o: &Offer) {
    if cfg!(test) {
        return;
    }
    let mut seen = offers_seen();
    let k = o.key();
    if !seen.contains(&k) {
        seen.push(k);
        let from = seen.len().saturating_sub(200);
        let _ = std::fs::create_dir_all(dir());
        let _ = std::fs::write(offers_file(), Value::from(seen[from..].to_vec()).to_string());
    }
}

/// The last rendered content (device-wide), so the next start paints it before the session is back.
fn boot_file() -> PathBuf {
    dir().join("boot.json")
}

fn write_boot(c: &JObj) {
    let _ = std::fs::create_dir_all(dir());
    let _ = std::fs::write(boot_file(), Value::Object(c.clone()).to_string());
}

fn read_boot() -> Option<JObj> {
    if !sync_enabled() || cfg!(test) {
        return None;
    }
    let text = std::fs::read_to_string(boot_file()).ok()?;
    let v = tinhat_theme::parse_json(&text).ok()?;
    let c = v.as_object()?.clone();
    (project(&c).source != ts::ChoiceSource::None).then_some(c)
}

/// The signed-in session, attached to the engine (a new account or a new session starts from that account's state on
/// disk). Returns false when nobody is signed in.
fn ensure_session(e: &mut Engine) -> bool {
    let Some(client) = crate::session::client() else { return false };
    let me = client.user_id().map(|u| u.to_string()).unwrap_or_default();
    let gen = crate::session::generation();
    if me.is_empty() {
        return false;
    }
    if e.user != me || e.gen != gen {
        e.reset_for(gen);
        e.user = me;
        e.device = client.device_id().map(|d| d.to_string()).unwrap_or_default();
        let had_state = sync_enabled() && load_state(e);
        e.had_state = had_state || boot_file().exists() || saved_theme_file().is_some();
        rerender(e);
        e.rev += 1;
    }
    true
}

// ---- QML plumbing -------------------------------------------------------------------------------------------------

type Sink = cxx_qt::CxxQtThread<crate::theme_ctl::qobject::ThemeController>;

fn sink() -> &'static Mutex<Option<Sink>> {
    static S: OnceLock<Mutex<Option<Sink>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(None))
}

/// ThemeController.start(): QML receives every later change through `publish`.
pub fn attach_sink(s: Sink) {
    *sink().lock().unwrap_or_else(|p| p.into_inner()) = Some(s);
    let _ = engine(); // the start content is read now, before the first frame
}

/// Push the engine's view to QML (queued onto the GUI thread).
pub fn publish() {
    let Some(qt) = sink().lock().ok().and_then(|s| s.clone()) else { return };
    let _ = qt.queue(|o| crate::theme_ctl::refresh(o));
}

/// What QML shows (THEMES-UI.md), plus the notices queued since the last call.
pub struct UiState {
    pub view: View,
    pub override_on: bool,
    pub json: Value,
    pub notices: Vec<(String, String, String, String)>,
    /// how many applies of a person's this run has seen (Engine::applied)
    pub applied: u64,
}

pub fn ui_state() -> UiState {
    let mut e = lock();
    let _ = ensure_session(&mut e);
    let v = view_of(&e.proj);
    let (want, origin) = wanted(&e);
    let wp = project(&want);
    let unknown = if wp.source == ts::ChoiceSource::None { wp.theme_id.clone() } else { None };
    let account_name = e.choice.known.as_ref().map(|k| name_of_content(&e, &ts::choice_content(k)));
    let pending = [e.choice_pending, e.themes_pending].iter().flatten().any(|t| t.elapsed() >= PENDING_LINE_AFTER);
    let mine = mine_of(&e);
    let json = json!({
        "sync": sync_enabled(),
        "signedIn": !e.user.is_empty(),
        "synced": e.synced,
        "origin": origin.as_str(),
        "readOnly": e.choice.read_only,
        "themesReadOnly": e.themes.read_only,
        "unknownTheme": unknown,
        "overridesRefused": wp.overrides == ts::Overrides::Refused,
        "pending": pending,
        "accountThemeName": account_name,
        "canUndo": e.undo.is_some(),
        "offer": e.offer.as_ref().map(|o| json!({ "pack": o.pack, "themeId": o.theme_id, "themeName": o.theme_name })),
        "mine": mine,
        "recent": recent_rows(&e),
        "rev": e.rev,
    });
    let notices = std::mem::take(&mut e.notices);
    UiState { view: v, override_on: e.override_on, json, notices, applied: e.applied }
}

/// My themes as the UI shows them: `{id, name, author, variant, usable}`, newest first.
fn mine_of(e: &Engine) -> Vec<Value> {
    my_list(e)
        .into_iter()
        .map(|entry| {
            let t = entry.get("theme").and_then(Value::as_object).cloned().unwrap_or_default();
            let usable = resolves(&t);
            let variant = crate::theme_lib::doc_variant(&Value::Object(t.clone())).map(|v| v.as_str()).unwrap_or("dark");
            json!({
                "id": entry.get("id"),
                "name": t.get("name").and_then(Value::as_str).unwrap_or(""),
                "author": t.get("author").and_then(Value::as_str).unwrap_or(""),
                "variant": variant,
                "usable": usable,
            })
        })
        .collect()
}

/// My themes for the gallery.
pub fn mine_items() -> Vec<Value> {
    mine_of(&lock())
}

/// A content's theme name (its own doc, the library, My themes), for notices and the hint.
fn name_of_content(e: &Engine, c: &JObj) -> String {
    let id = c.get("theme").and_then(Value::as_str).unwrap_or("");
    if let Some(Value::Object(d)) = c.get("doc") {
        if let Some(n) = d.get("name").and_then(Value::as_str) {
            return n.to_string();
        }
    }
    if let Some(d) = library(id) {
        return d.get("name").and_then(Value::as_str).unwrap_or(id).to_string();
    }
    if let Some(d) = my_theme_doc(e, id) {
        return d.get("name").and_then(Value::as_str).unwrap_or(id).to_string();
    }
    id.to_string()
}

// ---- applies (a person's actions; GUI thread) --------------------------------------------------------------------

/// The result of a person's action, in plain words when it failed (THEMES-UI.md §6.1 / §7).
pub enum Outcome {
    Done,
    Nothing,
    Failed(String),
}

impl Outcome {
    /// "" or the plain-words failure.
    pub fn error(&self) -> String {
        match self {
            Outcome::Failed(w) => w.clone(),
            _ => String::new(),
        }
    }
}

fn plain(code: &str) -> String {
    match code {
        "too-many" => "You already have 50 themes. Remove one to make room.".into(),
        "too-large" => "Your saved themes are full. Remove a theme to make room.".into(),
        "newer-format" => "A newer Tin Hat manages your themes. Update Tin Hat to save new ones here.".into(),
        "theme-too-large" => "This theme is larger than 8 KB.".into(),
        c => format!("This Tin Hat theme can't be used ({c})."),   // THEMES-UI §6.1 "any other refusal"
    }
}

/// Apply `patch` (`{theme?, doc?, sounds?, icons?}`, THEMES-SYNC §3.3) the way the device is set up: to this run in
/// gate mode / signed out / under TINHAT_THEME, to the local choice with the override on (or while the account's is
/// read-only), else to the account.
fn apply_patch(e: &mut Engine, patch: &Value) -> Outcome {
    let now = now_ms();
    let signed_in = ensure_session(e);
    if !sync_enabled() || e.session.is_some() || !signed_in {
        let start = e.session.clone().unwrap_or_else(|| e.rendered.clone());
        return match ts::save_choice(None, None, Some(patch), Some(&Value::Object(start)), now, "") {
            ts::SaveChoice::Entry(entry) => {
                e.session = Some(ts::choice_content(&entry));
                Outcome::Done
            }
            ts::SaveChoice::Noop => Outcome::Nothing,
            ts::SaveChoice::Error(c) => Outcome::Failed(plain(c)),
        };
    }
    if e.override_on || e.choice.read_only {
        let start = e.local.clone().unwrap_or_else(|| e.rendered.clone());
        return match ts::save_choice(None, None, Some(patch), Some(&Value::Object(start)), now, &e.device) {
            ts::SaveChoice::Entry(entry) => {
                e.local = Some(ts::choice_content(&entry));
                Outcome::Done
            }
            ts::SaveChoice::Noop => Outcome::Nothing,
            ts::SaveChoice::Error(c) => Outcome::Failed(plain(c)),
        };
    }
    // §3.3 fallback = what the device renders now (the legacy choice before the first save) — never an override
    let fallback = Value::Object(wanted(e).0);
    let by = e.device.clone();
    match ts::choice_apply(&mut e.choice, Some(patch), Some(&fallback), now, &by) {
        "saved" => {
            e.choice_pending.get_or_insert_with(Instant::now);
            Outcome::Done
        }
        "noop" => Outcome::Nothing,
        "read-only" => Outcome::Failed(plain("newer-format")),
        c => Outcome::Failed(plain(c)),
    }
}

fn after_change(e: &mut Engine) {
    rerender(e);
    e.rev += 1;
    save_state(e);
}

/// A person's action (THEMES-UI §3.2): `patch` applied the way the device is set up; on success the content shown
/// before it becomes the Undo, and a theme pick may offer the theme's sound pack (§10.6). The web's applyChoice.
fn apply_action(e: &mut Engine, patch: &Value, in_settings: bool) -> Outcome {
    let before = e.rendered.clone();
    let r = apply_patch(e, patch);
    if matches!(r, Outcome::Done) {
        e.undo = Some(before);
        e.applied += 1;
        after_change(e);
        if patch.get("theme").is_some() {
            maybe_offer(e, in_settings);
        }
    }
    r
}

/// Apply a theme by id (a card, the ☰ menu): a library theme by its id, one of My themes with its document.
/// `in_settings`: the pick came from the Settings sheet (an offer shows there inline; elsewhere as a toast).
pub fn apply_theme(id: &str, in_settings: bool) -> Outcome {
    let mut e = lock();
    let mut patch = json!({ "theme": id });
    if library(id).is_none() {
        match my_theme_doc(&e, id) {
            Some(d) => patch["doc"] = Value::Object(d),
            None => {
                if !sync_enabled() && crate::theme_rt::load_file_doc(id, &crate::theme_rt::themes_dir()).is_ok() {
                    // gate mode: a file theme by its id (the screenshot hook)
                } else if view_of(&e.proj).theme == id {
                    return Outcome::Nothing;
                } else {
                    return Outcome::Failed(format!("The theme “{id}” isn't available here."));
                }
            }
        }
    }
    let shown = view_of(&e.proj).theme;
    let r = apply_action(&mut e, &patch, in_settings);
    // THEMES-UI §12: a pick that changes the theme goes first in the ☰ menu's recents
    if matches!(r, Outcome::Done) && shown != id {
        note_pick(&e, &shown, id);
    }
    drop(e);
    kick_puts();
    publish();
    r
}

/// Undo (THEMES-UI §3.2): the content shown before the last apply — theme, sounds, icon style — applied as a new apply
/// (no Undo of the Undo).
pub fn undo() -> Outcome {
    let mut e = lock();
    let Some(u) = e.undo.take() else { return Outcome::Nothing };
    let shown = view_of(&e.proj).theme;
    let back_to = u.get("theme").and_then(Value::as_str).map(str::to_string);
    let obj_or_null = |k: &str| u.get(k).filter(|v| v.is_object()).cloned().unwrap_or(Value::Null);
    let mut patch = json!({ "theme": u.get("theme").cloned().unwrap_or(Value::Null), "doc": obj_or_null("doc"), "overrides": obj_or_null("overrides") });
    for k in ["sounds", "icons"] {
        if let Some(v @ Value::String(_)) = u.get(k) {
            patch[k] = v.clone();
        }
    }
    let r = apply_patch(&mut e, &patch);
    e.undo = None;
    e.offer = None;
    if matches!(r, Outcome::Done) {
        e.applied += 1;
    }
    after_change(&mut e);
    // THEMES-UI §12: the theme an Undo goes back to is a pick too (the web's undoLast → applyChoice)
    if let Some(t) = back_to.filter(|t| matches!(r, Outcome::Done) && *t != shown) {
        note_pick(&e, &shown, &t);
    }
    drop(e);
    kick_puts();
    publish();
    r
}

/// The sound pack (`default` | `goose`) or icon style (`duotone` | `mono`) — Settings → Appearance (§11).
pub fn apply_pack(key: &str, value: &str) -> Outcome {
    if key != "sounds" && key != "icons" {
        return Outcome::Nothing;
    }
    let mut e = lock();
    let r = apply_action(&mut e, &json!({ key: value }), true);
    drop(e);
    kick_puts();
    publish();
    r
}

/// §10.6 (the web's maybeOffer): the rendered theme's document suggests a sound pack (`sounds`) other than the one in
/// use → offer it once per theme id and pack per device — inline in the settings, else as a toast with the action.
/// Nothing switches by itself; gate mode offers nothing.
fn maybe_offer(e: &mut Engine, in_settings: bool) {
    if !sync_enabled() || e.session.is_some() {
        return;
    }
    let suggested = e.proj.theme.as_ref().and_then(|t| t.get("sounds")).cloned();
    let sounds_now = e.proj.sounds;
    let Some(pack) = suggested.as_ref().filter(|v| ts::is_pack_id(Some(v))).and_then(Value::as_str).filter(|p| *p != sounds_now) else {
        e.offer = None;
        return;
    };
    let pack: &'static str = match pack {
        "default" => "default",
        "goose" => "goose",
        _ => return, // a pack this version doesn't have: never offered
    };
    let v = view_of(&e.proj);
    let name = e.proj.theme.as_ref().and_then(|t| t.get("name")).and_then(Value::as_str).unwrap_or(&v.theme).to_string();
    let o = Offer { pack, theme_id: v.theme.clone(), theme_name: name };
    if offers_seen().contains(&o.key()) {
        return;
    }
    if in_settings {
        e.offer = Some(o);
        e.rev += 1;
    } else {
        mark_offered(&o);
        let pack_name = pack_name(pack);
        e.notices.push((
            "offer".into(),
            format!("{} comes with its own sounds. Use {pack_name} sounds too?", o.theme_name),
            format!("Use {pack_name} sounds"),
            pack.to_string(),
        ));
    }
}

fn pack_name(pack: &str) -> &'static str {
    if pack == "goose" {
        "Goose"
    } else {
        "Tin Hat"
    }
}

/// "Use ‹pack› sounds" of the inline offer: the pack, applied like a pick in the settings.
pub fn accept_offer() -> Outcome {
    let mut e = lock();
    let Some(o) = e.offer.take() else { return Outcome::Nothing };
    mark_offered(&o);
    let r = apply_action(&mut e, &json!({ "sounds": o.pack }), true);
    drop(e);
    kick_puts();
    publish();
    r
}

/// "No thanks": the offer goes and is not made again for this theme and pack.
pub fn dismiss_offer() {
    let mut e = lock();
    if let Some(o) = e.offer.take() {
        mark_offered(&o);
    }
    e.rev += 1;
    drop(e);
    publish();
}

/// The Settings sheet opened or closed (THEMES-UI §3.2, §10.6): Undo and an unanswered offer end with it (the offer
/// counts as shown).
pub fn end_session() {
    let mut e = lock();
    if e.undo.is_none() && e.offer.is_none() {
        return;
    }
    if let Some(o) = e.offer.take() {
        mark_offered(&o);
    }
    e.undo = None;
    e.rev += 1;
    drop(e);
    publish();
}

/// "Use a different theme on this device" (§7): on copies what is rendered now into the local choice (no visible
/// change); off forgets it and the account's choice renders again.
pub fn set_override(on: bool) {
    let mut e = lock();
    if !sync_enabled() || !ensure_session(&mut e) || on == e.override_on {
        return;
    }
    e.override_on = on;
    e.local = if on { Some(e.rendered.clone()) } else { None };
    e.undo = None;
    e.offer = None;
    after_change(&mut e);
    drop(e);
    publish();
}

/// "Keep ‹old› here" (THEMES-UI §7): the override on with the content shown before the change.
pub fn keep_here(content: &Value) {
    let Some(c) = content.as_object() else { return };
    let mut e = lock();
    if !ensure_session(&mut e) {
        return;
    }
    let shown = view_of(&e.proj).theme;
    e.override_on = true;
    e.local = Some(ts::choice_content(c));
    after_change(&mut e);
    // the person chose to keep that theme here: a pick for the ☰ menu's recents (THEMES-UI §12)
    if let Some(t) = c.get("theme").and_then(Value::as_str).filter(|t| *t != shown) {
        note_pick(&e, &shown, t);
    }
    drop(e);
    publish();
}

// ---- the ☰ Theme menu's recent themes (THEMES-UI §12) -------------------------------------------------------------

/// The ui pref file (`<config>/tinhat/ui-theme-recent`, like every `settings.uiPref`; the web's pref `theme-recent`).
const RECENT_FILE: &str = "ui-theme-recent";

/// Gate mode and `TINHAT_THEME` runs keep their recents for the run only (never read or written) — like their theme.
fn recent_run_only() -> bool {
    cfg!(test) || !sync_enabled() || env_theme().is_some()
}

fn recent_mem() -> &'static Mutex<Vec<String>> {
    static R: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(Vec::new()))
}

/// The history, most recent first.
fn recent_history() -> Vec<String> {
    if recent_run_only() {
        return recent_mem().lock().unwrap_or_else(|p| p.into_inner()).clone();
    }
    tinhat_theme::recent::read_recent(crate::settings::overlay_pref_read(RECENT_FILE).as_deref())
}

/// Can the menu apply this theme: a library theme (OLED and Goose included), one of My themes the engine accepts, or
/// — gate mode, which applies them by id — a file theme (`<data>/themes/<id>.json`).
fn menu_usable(e: &Engine) -> impl Fn(&str) -> bool {
    let mine = my_list(e);
    move |id: &str| {
        if library(id).is_some() {
            return true;
        }
        let entry = mine.iter().find(|x| x.get("id").and_then(Value::as_str) == Some(id));
        if let Some(t) = entry.and_then(|x| x.get("theme")).and_then(Value::as_object) {
            return resolves(t);
        }
        !sync_enabled() && crate::theme_rt::load_file_doc(id, &crate::theme_rt::themes_dir()).is_ok()
    }
}

/// A person's action on this device put `picked` on screen while `shown` was there.
fn note_pick(e: &Engine, shown: &str, picked: &str) {
    let next = tinhat_theme::recent::record_pick(&recent_history(), shown, picked, &menu_usable(e));
    if recent_run_only() {
        *recent_mem().lock().unwrap_or_else(|p| p.into_inner()) = next;
        return;
    }
    let text = tinhat_theme::recent::write_recent(&next);
    if crate::settings::overlay_pref_read(RECENT_FILE).as_deref() != Some(text.as_str()) {
        crate::settings::overlay_pref_write(RECENT_FILE, &text);
    }
}

/// The card roles a swatch paints, in its order: fill, border, the three dots.
const SWATCH_ROLES: [&str; 5] = ["surface.window", "border.subtle", "accent.base", "name.1", "name.2"];

fn swatch_of(p: Option<&crate::theme_lib::Preview>) -> Value {
    let Some(p) = p else { return Value::Null };
    let colours: Vec<Value> = SWATCH_ROLES
        .iter()
        .map(|role| {
            let c = p.roles.get(*role).and_then(|x| tinhat_theme::parse_literal(x)).unwrap_or(tinhat_theme::Colour::BLACK);
            Value::String(crate::theme_rt::qt_string(&c))
        })
        .collect();
    Value::Array(colours)
}

/// The ☰ Theme menu's rows, top to bottom: `{id, name, current, swatch}` — the recents for the theme on screen
/// (`tinhat_theme::recent::recent_menu`); `swatch` = the row theme's [surface.window, border.subtle, accent.base,
/// name.1, name.2] as Qt colour strings (ThemeSwatch.qml), null when its roles do not resolve.
fn recent_rows(e: &Engine) -> Vec<Value> {
    let v = view_of(&e.proj);
    let usable = menu_usable(e);
    let ids = tinhat_theme::recent::recent_menu(&recent_history(), &v.theme, &usable);
    ids.into_iter()
        .filter_map(|id| {
            let current = id == v.theme;
            let (name, swatch) = if current && v.doc.is_some() {
                // the theme on screen with its own document (a My theme, a kept theme, overrides laid over)
                let d = v.doc.clone().map(Value::Object).unwrap_or(Value::Null);
                let name = d.get("name").and_then(Value::as_str).unwrap_or(&id).to_string();
                (name, swatch_of(crate::theme_lib::preview_doc(&d).ok().as_ref()))
            } else if let Some(entry) = crate::theme_lib::library().get(&id) {
                (entry.name.clone(), swatch_of(crate::theme_lib::preview(&id).as_deref()))
            } else if let Some(t) = my_theme_doc(e, &id) {
                let name = t.get("name").and_then(Value::as_str).unwrap_or(&id).to_string();
                (name, swatch_of(crate::theme_lib::preview_doc(&Value::Object(t)).ok().as_ref()))
            } else if let Ok(d) = crate::theme_rt::load_file_doc(&id, &crate::theme_rt::themes_dir()) {
                // gate mode's file themes (the screenshot hook)
                let name = d.get("name").and_then(Value::as_str).unwrap_or(&id).to_string();
                (name, swatch_of(crate::theme_lib::preview_doc(&d).ok().as_ref()))
            } else {
                return None;
            };
            Some(json!({ "id": id, "name": name, "current": current, "swatch": swatch }))
        })
        .collect()
}

// ---- My themes ----------------------------------------------------------------------------------------------------

fn save_mine(e: &mut Engine, req: Value) -> Outcome {
    let Some(req) = req.as_object() else { return Outcome::Nothing };
    match ts::themes_save(&mut e.themes, req) {
        "saved" => {
            if sync_enabled() && !e.user.is_empty() {
                e.themes_pending.get_or_insert_with(Instant::now);
            } else {
                e.themes.want_put = false; // gate mode / signed out: this run only
            }
            Outcome::Done
        }
        "noop" => Outcome::Nothing,
        "read-only" => Outcome::Failed(plain("newer-format")),
        c => Outcome::Failed(plain(c)),
    }
}

/// Save an imported (or kept) theme to My themes under a new id (THEMES-SYNC §4.1) named `name`; `use_it` applies it
/// too (a pick in the Settings sheet: a sound offer shows there). Returns the id.
pub fn save_new(doc: &JObj, name: &str, use_it: bool) -> Result<String, String> {
    save_new_opts(doc, name, use_it, true)
}

/// `save_new` with the apply's `in_settings` (THEMES-UI §10.6): the theme editor's Save and a share card's Keep are not
/// in the Settings sheet, so a sound offer comes as a toast (the web's chooseTheme without `inSettings`).
pub fn save_new_opts(doc: &JObj, name: &str, use_it: bool, in_settings: bool) -> Result<String, String> {
    let mut e = lock();
    let _ = ensure_session(&mut e);
    let taken_doc = Value::Object(my_themes_doc(&e));
    let taken: BTreeSet<String> = ts::entries_of(Some(&taken_doc)).into_keys().collect();
    let id = ts::unique_theme_id(&tinhat_theme::import::slugify(name), |x| taken.contains(x) || library(x).is_some());
    let mut t = doc.clone();
    t.insert("id".into(), Value::String(id.clone()));
    t.insert("name".into(), Value::String(name.to_string()));
    let by = e.device.clone();
    let req = json!({ "id": id, "theme": Value::Object(t), "now": now_ms(), "by": by });
    if let Outcome::Failed(why) = save_mine(&mut e, req) {
        return Err(why);
    }
    after_change(&mut e);
    drop(e);
    if use_it {
        if let Outcome::Failed(why) = apply_theme(&id, in_settings) {
            return Err(why);
        }
    } else {
        kick_puts();
        publish();
    }
    Ok(id)
}

/// Rename one of My themes (the rendered one follows).
pub fn rename_mine(id: &str, name: &str) -> Outcome {
    let mut e = lock();
    let _ = ensure_session(&mut e);
    let Some(mut t) = my_theme_doc(&e, id) else { return Outcome::Nothing };
    t.insert("name".into(), Value::String(name.to_string()));
    let by = e.device.clone();
    let req = json!({ "id": id, "theme": Value::Object(t.clone()), "now": now_ms(), "by": by });
    let r = save_mine(&mut e, req);
    if matches!(r, Outcome::Done) && view_of(&e.proj).theme == id {
        let _ = apply_patch(&mut e, &json!({ "theme": id, "doc": Value::Object(t) }));
    }
    after_change(&mut e);
    drop(e);
    kick_puts();
    publish();
    r
}

/// Remove one of My themes; returns its document for the toast's Undo (`restore_mine`).
pub fn remove_mine(id: &str) -> Option<JObj> {
    let mut e = lock();
    let _ = ensure_session(&mut e);
    let t = my_theme_doc(&e, id)?;
    let by = e.device.clone();
    let req = json!({ "id": id, "deleted": true, "now": now_ms(), "by": by });
    let r = save_mine(&mut e, req);
    after_change(&mut e);
    drop(e);
    kick_puts();
    publish();
    matches!(r, Outcome::Done).then_some(t)
}

/// The toast's Undo: the removed theme comes back under its id.
pub fn restore_mine(doc: &JObj) -> Outcome {
    let Some(id) = doc.get("id").and_then(Value::as_str).map(str::to_string) else { return Outcome::Nothing };
    let mut e = lock();
    let _ = ensure_session(&mut e);
    let by = e.device.clone();
    let req = json!({ "id": id, "theme": Value::Object(doc.clone()), "now": now_ms(), "by": by });
    let r = save_mine(&mut e, req);
    after_change(&mut e);
    drop(e);
    kick_puts();
    publish();
    r
}

/// My themes' document by id (the import preview's "is it one of mine" and the cards).
pub fn my_doc(id: &str) -> Option<JObj> {
    let e = lock();
    my_theme_doc(&e, id)
}

/// My themes' ids (newest first).
pub fn my_ids() -> Vec<String> {
    let e = lock();
    my_list(&e).iter().filter_map(|x| x.get("id").and_then(Value::as_str).map(str::to_string)).collect()
}

// ---- the session: the syncs, sign-out ------------------------------------------------------------------------------

async fn read_account_data(client: &Client, ty: &str) -> Option<Value> {
    let raw = client.account().account_data_raw(GlobalAccountDataEventType::from(ty)).await.ok().flatten()?;
    // the contract's reader (JavaScript numbers, own keys, ≤ 128 levels)
    tinhat_theme::parse_json(raw.json().get()).ok()
}

async fn write_account_data(client: &Client, ty: &str, content: &JObj) -> Result<(), String> {
    let text = tinhat_theme::canon_json(&Value::Object(content.clone()));
    let raw_value = serde_json::value::RawValue::from_string(text).map_err(|e| e.to_string())?;
    let raw: Raw<matrix_sdk::ruma::events::AnyGlobalAccountDataEventContent> = Raw::from_json(raw_value);
    client
        .account()
        .set_account_data_raw(GlobalAccountDataEventType::from(ty), raw)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn session_is(user: &str, gen: u64) -> bool {
    crate::session::generation() == gen
        && crate::session::client().and_then(|c| c.user_id().map(|u| u.to_string())).as_deref() == Some(user)
}

/// A document of either type arrived (the first sync's read = `initial`, or a sync event): the protocol decides (§3.5,
/// §4.6). A choice that another device wrote and that changes the theme on screen (no override) is announced once it is
/// painted (THEMES-UI §7) — not on the first read of a device that had no theme state yet (a fresh sign-in just takes
/// the account's theme). The web's themeStore `receive`.
fn receive(e: &mut Engine, ty: &str, content: Option<&Value>, initial: bool) {
    if ty == ts::CHOICE_TYPE {
        let before = e.rendered.clone();
        let before_name = name_of_content(e, &before);
        let had_override = e.override_on;
        let r = ts::choice_receive(&mut e.choice, content);
        if (r == "changed" || r == "new") && !had_override && !(initial && !e.had_state) {
            let by = e.choice.known.as_ref().and_then(|k| k.get("by")).and_then(Value::as_str).unwrap_or("").to_string();
            // what the device renders after this document (rerender keeps the screen when it can't render it)
            rerender(e);
            let to = e.rendered.clone();
            if by != e.device && to.get("theme") != before.get("theme") {
                let to_name = name_of_content(e, &to);
                e.notices.push((
                    "changed".into(),
                    format!("Your theme was changed to {to_name} on another device."),
                    format!("Keep {before_name} here"),
                    Value::Object(before).to_string(),
                ));
            }
        }
        if r == "reassert" {
            e.choice_reassert = true;
            e.choice_pending.get_or_insert_with(Instant::now);
        }
        eprintln!("[themesync] receive choice: {r}");
    } else if ty == ts::THEMES_TYPE {
        let r = ts::themes_receive(&mut e.themes, content);
        if r == "reassert" {
            e.themes_reassert = true;
            e.themes_pending.get_or_insert_with(Instant::now);
        }
        eprintln!("[themesync] receive my themes: {r}");
    }
}

/// bridge.rs's sync loop, after every successful pass (like presence.rs `on_sync`).
pub async fn on_sync(client: &Client, account_data: &[Raw<AnyGlobalAccountDataEvent>]) {
    if !sync_enabled() {
        return;
    }
    let me = client.user_id().map(|u| u.to_string()).unwrap_or_default();
    let gen = crate::session::generation();
    let first = {
        let mut e = lock();
        if !ensure_session(&mut e) || e.user != me {
            return;
        }
        !e.synced
    };
    if first {
        // "known after the first sync" — both documents from the store's copy
        let choice = read_account_data(client, ts::CHOICE_TYPE).await;
        let themes = read_account_data(client, ts::THEMES_TYPE).await;
        let mut e = lock();
        if !session_is(&me, gen) || e.user != me || e.gen != gen || e.synced {
            return;
        }
        // a key the server never had reads as nothing (§3.2 / §4.3: ignored) — the device keeps what it knows
        receive(&mut e, ts::CHOICE_TYPE, choice.as_ref(), true);
        receive(&mut e, ts::THEMES_TYPE, themes.as_ref(), true);
        e.synced = true;
        eprintln!(
            "[themesync] account known: choice {}, my themes {}",
            e.choice.known.as_ref().and_then(|k| k.get("theme")).and_then(Value::as_str).unwrap_or("none"),
            e.themes.known.len()
        );
        after_change(&mut e);
    } else {
        let mut e = lock();
        if e.user != me || e.gen != gen {
            return;
        }
        let mut any = false;
        for raw in account_data {
            let Ok(Some(ty)) = raw.get_field::<String>("type") else { continue };
            if ty != ts::CHOICE_TYPE && ty != ts::THEMES_TYPE {
                continue;
            }
            let content = tinhat_theme::parse_json(raw.json().get()).ok().and_then(|v| v.get("content").cloned());
            receive(&mut e, &ty, content.as_ref(), false);
            any = true;
        }
        if !any {
            return;
        }
        after_change(&mut e);
    }
    kick_puts();
    publish();
}

/// Sign-out: this account's sync state is forgotten (THEMES-SYNC §6); the device keeps its look.
pub fn on_sign_out(user: &str) {
    if !user.is_empty() {
        let _ = std::fs::remove_file(state_file(user));
    }
    let mut e = lock();
    e.reset_for(crate::session::generation());
    drop(e);
    publish();
}

// ---- PUTs ---------------------------------------------------------------------------------------------------------

/// Start the PUT worker of each key that wants one (at most one per key; retries 1, 2, 4 … s up to 5 min, §6).
fn kick_puts() {
    if !sync_enabled() {
        return;
    }
    let (choice, themes) = {
        let mut e = lock();
        // nothing goes out before this session's first read of both keys (the web's `live`): the reads decide first
        let ready = e.synced && !e.user.is_empty();
        let c = ready && e.choice.want_put && !e.choice.read_only && !e.choice_worker;
        let t = ready && e.themes.want_put && !e.themes.read_only && !e.themes_worker;
        e.choice_worker |= c;
        e.themes_worker |= t;
        (c, t)
    };
    if choice {
        crate::bridge::runtime().spawn(put_worker(ts::CHOICE_TYPE));
    }
    if themes {
        crate::bridge::runtime().spawn(put_worker(ts::THEMES_TYPE));
    }
    if choice || themes {
        // the pending line appears 10 s after a save that is still waiting
        crate::bridge::runtime().spawn(async {
            tokio::time::sleep(PENDING_LINE_AFTER + Duration::from_millis(200)).await;
            publish();
        });
    }
}

async fn put_worker(ty: &'static str) {
    let choice = ty == ts::CHOICE_TYPE;
    loop {
        // a re-assert waits 0–2 s by the device id (two devices rarely re-put at the same moment)
        let delay = {
            let mut e = lock();
            let flag = if choice { &mut e.choice_reassert } else { &mut e.themes_reassert };
            let d = if *flag { Some(()) } else { None };
            *flag = false;
            d.map(|_| ts::reassert_delay_ms(&e.device))
        };
        if let Some(ms) = delay.filter(|ms| *ms > 0) {
            tokio::time::sleep(Duration::from_millis(ms as u64)).await;
        }
        let (client, doc, user, gen) = {
            let mut e = lock();
            let doc = if choice { ts::choice_put(&mut e.choice) } else { ts::themes_put(&mut e.themes) };
            let Some(doc) = doc else {
                if choice {
                    e.choice_worker = false;
                    e.choice_pending = None;
                } else {
                    e.themes_worker = false;
                    e.themes_pending = None;
                }
                e.rev += 1;
                save_state(&e);
                drop(e);
                publish();
                return;
            };
            save_state(&e);
            (crate::session::client(), doc, e.user.clone(), e.gen)
        };
        let ok = match client {
            Some(c) if session_is(&user, gen) => match tokio::time::timeout(HTTP_TIMEOUT, write_account_data(&c, ty, &doc)).await {
                Ok(Ok(())) => {
                    eprintln!("[themesync] PUT {ty}: ok");
                    true
                }
                Ok(Err(why)) => {
                    eprintln!("[themesync] PUT {ty} failed: {why}");
                    false
                }
                Err(_) => {
                    eprintln!("[themesync] PUT {ty} timed out");
                    false
                }
            },
            _ => false,
        };
        let wait = {
            let mut e = lock();
            if e.user != user || e.gen != gen {
                return; // signed out meanwhile: this session's state is gone
            }
            if ok {
                if choice {
                    e.choice_attempt = 0;
                } else {
                    e.themes_attempt = 0;
                }
                None // the next turn ends the worker unless another save came in meanwhile
            } else {
                // the save still has to land: want the PUT again, back off
                let attempt = if choice { &mut e.choice_attempt } else { &mut e.themes_attempt };
                let wait = ts::retry_delay_ms(*attempt);
                *attempt += 1;
                if choice {
                    e.choice.want_put = true;
                } else {
                    e.themes.want_put = true;
                }
                save_state(&e);
                Some(wait)
            }
        };
        if let Some(ms) = wait {
            publish();
            tokio::time::sleep(Duration::from_millis(ms)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contents_and_legacy() {
        assert_eq!(content_for("goose")["sounds"], "goose");
        assert_eq!(content_for("nord")["sounds"], "default");
        assert_eq!(content_for("nord")["icons"], "duotone");
    }

    #[test]
    fn projection_of_library_and_doc_themes() {
        let p = project(&content_for("nord"));
        assert_eq!(p.source, ts::ChoiceSource::Library);
        let v = view_of(&p);
        assert_eq!((v.theme.as_str(), v.doc.is_none(), v.sounds, v.icons), ("nord", true, "default", "duotone"));
        let mut mine = crate::theme_lib::doc("dracula").unwrap().as_object().unwrap().clone();
        mine.insert("id".into(), json!("my-dracula"));
        let c = json!({ "theme": "my-dracula", "doc": mine, "sounds": "goose", "icons": "mono" });
        let p = project(c.as_object().unwrap());
        let v = view_of(&p);
        assert_eq!((v.theme.as_str(), v.doc.is_some(), v.sounds, v.icons), ("my-dracula", true, "goose", "mono"));
        let p = project(json!({ "theme": "from-2027" }).as_object().unwrap());
        assert_eq!(p.source, ts::ChoiceSource::None);
    }
}
