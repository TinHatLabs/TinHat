//! theme_rt.rs — the theme runtime's pure half (desktop C1; docs/MIGRATING-THEME.md): which themes exist, resolving
//! one through the engine (crates/theme, `App::Desktop`) and the process-wide ACTIVE theme. No Qt: ThemeController
//! (theme_ctl.rs, GUI thread) activates a theme on every switch; Rust painters read it (presence dots,
//! presence_model.rs; the KDE colour scheme, colorscheme.rs).
//!
//! A theme id is a built-in (`oled`, `goose`), one of your own or kept themes (theme_store.rs: the account's
//! `dev.tinhat.themes`), a library theme (theme_lib.rs, themes step 1b) or a theme document at
//! `<data dir>/themes/<id>.json` whose own `id` is that id — looked up in that order. Anything unknown, unreadable or
//! refused by the engine resolves as OLED (the reason is logged once per resolve) — a bad theme can never leave the
//! app half-painted.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, RwLock};

use serde_json::Value;
use tinhat_theme::{app_tokens, builtin_theme, resolve_for_app, App, AppResolved, Colour};

/// The theme everything falls back to.
pub const FALLBACK: &str = "oled";

/// A resolved theme: its id, display name, every role + token colour, and the document it came from.
#[derive(Debug)]
pub struct Active {
    pub id: String,
    pub name: String,
    pub resolved: AppResolved,
    /// The theme document (its own `roles` / `tokens` expressions; theme_ctl.rs reads them for Qt precision).
    pub doc: Value,
}

impl Active {
    /// A role's or token's colour (`None` for an unknown id).
    pub fn colour(&self, id: &str) -> Option<Colour> {
        self.resolved.colour(id)
    }
}

/// `<data dir>/themes` — where a theme that is not built in lives, as `<id>.json`.
pub fn themes_dir() -> PathBuf {
    crate::platform::app_data_dir().join("themes")
}

/// The theme document of `id`: a built-in, else `<dir>/<id>.json` when `id` is a valid theme id, the file parses and
/// names itself `id`.
pub fn load_doc(id: &str, dir: &Path) -> Result<Value, String> {
    if let Some(v) = builtin_theme(id) {
        return Ok(v.clone());
    }
    if !tinhat_theme::limits::is_theme_id(id) {
        return Err(format!("'{id}' is not a theme id"));
    }
    if let Some(v) = crate::theme_store::doc(id) {
        return Ok(v);
    }
    if let Some(v) = crate::theme_lib::doc(id) {
        return Ok(v.clone());
    }
    load_file_doc(id, dir)
}

/// `<dir>/<id>.json` when `id` is a valid theme id, the file parses and names itself `id` (a theme file on disk —
/// the store never, so theme_store.rs may call this under its own lock).
pub fn load_file_doc(id: &str, dir: &Path) -> Result<Value, String> {
    if !tinhat_theme::limits::is_theme_id(id) {
        return Err(format!("'{id}' is not a theme id"));
    }
    let path = dir.join(format!("{id}.json"));
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    if text.len() > tinhat_theme::limits::MAX_BYTES * 4 {
        return Err(format!("{}: too large ({} bytes)", path.display(), text.len()));
    }
    // The engine's entry point for theme JSON text (the web's parseThemeJson: strict JSON, ≤ 128 levels, no lone
    // surrogates) — a file is judged exactly like a theme arriving in account data or a shared card.
    let doc = tinhat_theme::parse_theme_json(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    match doc.get("id").and_then(Value::as_str) {
        Some(own) if own == id => Ok(doc),
        other => Err(format!("{}: its id is {other:?}, not '{id}'", path.display())),
    }
}

fn resolve_doc(id: &str, doc: &Value) -> Result<Active, String> {
    let resolved = resolve_for_app(doc, app_tokens(), App::Desktop).map_err(|e| format!("theme '{id}': {e}"))?;
    let name = doc.get("name").and_then(Value::as_str).unwrap_or(id).to_string();
    Ok(Active { id: id.to_string(), name, resolved, doc: doc.clone() })
}

/// A built-in, resolved once per process (they never change).
fn builtin(id: &str) -> Option<Arc<Active>> {
    static CACHE: OnceLock<std::sync::Mutex<Vec<Arc<Active>>>> = OnceLock::new();
    let doc = builtin_theme(id)?;
    let cache = CACHE.get_or_init(Default::default);
    let mut list = cache.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(a) = list.iter().find(|a| a.id == id) {
        return Some(a.clone());
    }
    let a = Arc::new(resolve_doc(id, doc).expect("a built-in theme resolves"));
    list.push(a.clone());
    Some(a)
}

/// Resolve `id` from the built-ins or `dir` — `Err` (why) when it is unknown or the engine refuses it. The theme the
/// device renders from its own document (theme_store.rs: a My theme, a kept theme, overrides laid over — also over a
/// built-in) is that document.
pub fn try_resolve_in(id: &str, dir: &Path) -> Result<Arc<Active>, String> {
    if let Some(doc) = crate::theme_store::rendered_doc(id) {
        return resolve_doc(id, &doc).map(Arc::new);
    }
    if let Some(a) = builtin(id) {
        return Ok(a);
    }
    let doc = load_doc(id, dir)?;
    resolve_doc(id, &doc).map(Arc::new)
}

/// Resolve `id`, falling back to OLED (the reason goes to the log).
pub fn resolve_in(id: &str, dir: &Path) -> Arc<Active> {
    match try_resolve_in(id, dir) {
        Ok(a) => a,
        Err(why) => {
            eprintln!("[theme] {why} — using {FALLBACK}");
            try_resolve_in(FALLBACK, dir).expect("the built-in OLED theme resolves")
        }
    }
}

/// Would `id` resolve as itself (no fallback)?
pub fn known(id: &str) -> bool {
    try_resolve_in(id, &themes_dir()).is_ok()
}

fn active_cell() -> &'static RwLock<Arc<Active>> {
    static CELL: OnceLock<RwLock<Arc<Active>>> = OnceLock::new();
    CELL.get_or_init(|| RwLock::new(resolve_in(FALLBACK, Path::new(""))))
}

/// Resolve `id` (OLED fallback) and make it the process-wide active theme.
pub fn activate(id: &str) -> Arc<Active> {
    let a = resolve_in(id, &themes_dir());
    *active_cell().write().unwrap_or_else(|p| p.into_inner()) = a.clone();
    a
}

/// The active theme (OLED until the first `activate`).
pub fn current() -> Arc<Active> {
    active_cell().read().unwrap_or_else(|p| p.into_inner()).clone()
}

/// A colour as a Qt colour string: `#rrggbb`, or `#aarrggbb` (alpha first, 8-bit) when not opaque.
pub fn qt_string(c: &Colour) -> String {
    if c.is_opaque() {
        c.hex6()
    } else {
        c.to_qt_argb()
    }
}

/// A role or token of the ACTIVE theme as a Qt colour string (see `qt_string`); a wrong id is a bug → panic.
pub fn qt_colour(id: &str) -> String {
    let a = current();
    let c = a.colour(id).unwrap_or_else(|| panic!("theme runtime: no role or token '{id}'"));
    qt_string(&c)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("tinhat-theme-rt-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn builtins_resolve_as_themselves() {
        for (id, name, bg) in [("oled", "OLED", "#000000"), ("goose", "Goose", "#1a0a1a")] {
            let a = try_resolve_in(id, Path::new("/nonexistent")).unwrap();
            assert_eq!((a.id.as_str(), a.name.as_str()), (id, name));
            assert_eq!(qt_string(&a.colour("surface.window").unwrap()), bg);
        }
    }

    #[test]
    fn unknown_or_bad_themes_fall_back_to_oled() {
        let d = tmp_dir("bad");
        assert_eq!(resolve_in("nope", &d).id, "oled");
        assert_eq!(resolve_in("../etc/passwd", &d).id, "oled");
        std::fs::write(d.join("broken.json"), "{ not json").unwrap();
        assert_eq!(resolve_in("broken", &d).id, "oled");
        // A document that calls itself something else is not that theme.
        let mut doc = builtin_theme("goose").unwrap().clone();
        doc["id"] = Value::String("other".into());
        std::fs::write(d.join("mine.json"), doc.to_string()).unwrap();
        assert!(try_resolve_in("mine", &d).unwrap_err().contains("its id is"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_theme_file_resolves() {
        let d = tmp_dir("file");
        let mut doc = builtin_theme("goose").unwrap().clone();
        doc["id"] = Value::String("mygoose".into());
        doc["name"] = Value::String("My Goose".into());
        std::fs::write(d.join("mygoose.json"), doc.to_string()).unwrap();
        let a = resolve_in("mygoose", &d);
        assert_eq!((a.id.as_str(), a.name.as_str()), ("mygoose", "My Goose"));
        assert_eq!(qt_string(&a.colour("accent.base").unwrap()), "#b0508c");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn qt_strings() {
        assert_eq!(qt_string(&tinhat_theme::parse_literal("#1e6a76/0.18").unwrap()), "#2e1e6a76");
        assert_eq!(qt_string(&tinhat_theme::parse_literal("#3BA55C").unwrap()), "#3ba55c");
    }
}
