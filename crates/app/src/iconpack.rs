//! Icon pack (0.8.26): a uniform, ANIMATED icon set replaces Breeze name-by-name,
//! incrementally — any name the pack lacks still comes from Breeze, so the pack can land in
//! pieces. Layout of a pack directory:
//!
//!   manifest.json            { "name": "...", "version": 1,
//!                              "icons": { "<icon-name>": { "anim": "anim/<file>.webp",
//!                                                          "trigger": "hover|active|loop|once",
//!                                                          "variants": { "goose": "anim/…" } } } }
//!   theme/<theme>/index.theme + <size>/<context>/<name>.svg   — the STATIC glyphs, registered
//!                              as the QIcon theme (video_bridge.cpp reads TINHAT_ICON_THEME_DIR /
//!                              TINHAT_ICON_THEME_NAME) with Breeze as the inheritance fallback.
//!   anim/*.webp | *.gif      — animated frames, played by ThIcon.qml's AnimatedImage. Neither
//!                              platform ships Qt Lottie; both decode animated WebP and GIF.
//!
//! Discovery order (first hit wins):
//!   1. $TINHAT_ICON_PACK                       — explicit (gates, experiments)
//!   2. <app data>/iconpack                   — a LIVE override: drop a pack there, restart, see it
//!                                              (how a pack is reviewed in the real app)
//!   3. <exe dir>/iconpack                    — the Windows bundle (package.ps1 stages it)
//!   4. /app/share/tinhat/iconpack, /usr/share/tinhat/iconpack — the Flatpak / the package
//!   5. <repo>/crates/app/assets/iconpack     — dev builds straight from the tree
//! A directory counts as a pack when it holds `manifest.json` or a `theme/` dir.

use core::pin::Pin;
use std::path::{Path, PathBuf};

use cxx_qt_lib::QString;

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
        #[qproperty(QString, pack_json)] //   QML: packJson — {"dir": <file:// url>, "path", "name", "version", "icons": {…}} or ""
        #[qproperty(QString, pack_dir)] //    QML: packDir — filesystem path of the pack ("" = none, Breeze only)
        #[qproperty(QString, theme_name)] //  QML: themeName — the static icon theme registered from the pack ("" = none)
        #[qproperty(bool, gallery_requested)] // QML: galleryRequested — TINHAT_GUI_ICONGALLERY=1 opens the gallery at start
        type IconPackController = super::IconPackControllerRust;

        /// QML: reload() — re-scan the pack locations + re-read the manifest (no restart needed
        /// for animated icons; the STATIC theme is registered once at start).
        #[qinvokable]
        fn reload(self: Pin<&mut IconPackController>);
    }
}

pub struct IconPackControllerRust {
    pack_json: QString,
    pack_dir: QString,
    theme_name: QString,
    gallery_requested: bool,
}

impl Default for IconPackControllerRust {
    fn default() -> Self {
        let loaded = load();
        Self {
            pack_json: QString::from(&loaded.json),
            pack_dir: QString::from(&loaded.dir),
            theme_name: QString::from(&loaded.theme),
            gallery_requested: std::env::var("TINHAT_GUI_ICONGALLERY").map(|v| v == "1").unwrap_or(false),
        }
    }
}

impl qobject::IconPackController {
    pub fn reload(mut self: Pin<&mut Self>) {
        let loaded = load();
        self.as_mut().set_pack_json(QString::from(&loaded.json));
        self.as_mut().set_pack_dir(QString::from(&loaded.dir));
        self.as_mut().set_theme_name(QString::from(&loaded.theme));
    }
}

struct Loaded {
    json: String,
    dir: String,
    theme: String,
}

fn is_pack(dir: &Path) -> bool {
    dir.join("manifest.json").is_file() || dir.join("theme").is_dir()
}

/// The pack directory in use, if any (see the module doc for the search order).
pub(crate) fn pack_dir() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(p) = std::env::var_os("TINHAT_ICON_PACK") {
        if !p.is_empty() {
            candidates.push(PathBuf::from(p));
        }
    }
    candidates.push(crate::platform::app_data_dir().join("iconpack"));
    if let Ok(exe) = std::env::current_exe() {
        if let Some(d) = exe.parent() {
            candidates.push(d.join("iconpack"));
        }
    }
    #[cfg(not(windows))]
    candidates.push(PathBuf::from("/app/share/tinhat/iconpack")); // Flatpak
    candidates.push(PathBuf::from("/usr/share/tinhat/iconpack"));
    // the checkout's own pack: dev runs and tests only (a release binary must not carry the build machine's path)
    #[cfg(any(debug_assertions, test))]
    candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets").join("iconpack"));
    candidates.into_iter().find(|d| is_pack(d))
}

/// `(theme search path, theme name)` for the pack's static theme: `<pack>/theme/<name>/index.theme`.
pub(crate) fn theme_dir() -> Option<(PathBuf, String)> {
    let base = pack_dir()?.join("theme");
    let entries = std::fs::read_dir(&base).ok()?;
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() && p.join("index.theme").is_file() {
            let name = p.file_name()?.to_string_lossy().to_string();
            return Some((base, name));
        }
    }
    None
}

/// Publish the static theme location for the C++ icon-theme registration (called BEFORE
/// `video::icon_theme()`; a missing pack leaves the variables unset = Breeze as today).
pub(crate) fn export_theme_env() {
    if let Some((base, name)) = theme_dir() {
        std::env::set_var("TINHAT_ICON_THEME_DIR", &base);
        std::env::set_var("TINHAT_ICON_THEME_NAME", &name);
        eprintln!("[iconpack] static theme '{name}' from {}", base.display());
    }
}

fn load() -> Loaded {
    let Some(dir) = pack_dir() else {
        return Loaded { json: String::new(), dir: String::new(), theme: String::new() };
    };
    let theme = theme_dir().map(|(_, n)| n).unwrap_or_default();
    let manifest: serde_json::Value = std::fs::read_to_string(dir.join("manifest.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    let icons = manifest.get("icons").cloned().unwrap_or_else(|| serde_json::json!({}));
    // The current pack: every catalogue entry is a vector hover story (`canonical` names the
    // drawing); the earlier webp format used `anim`. Count either as animated.
    let n_anim = icons
        .as_object()
        .map(|o| o.values().filter(|v| v.get("anim").is_some() || v.get("canonical").is_some()).count())
        .unwrap_or(0);
    // The motion engine (motion/ThIcon.qml + IconManifest.js + drawings/) is loaded by URL
    // from the pack dir by qml/ThIcon.qml when present.
    let motion = dir.join("motion").join("ThIcon.qml").is_file() && dir.join("motion").join("IconManifest.js").is_file();
    let out = serde_json::json!({
        "dir": crate::platform::file_url(&dir),
        "path": dir.to_string_lossy(),
        "name": manifest.get("name").and_then(|v| v.as_str()).unwrap_or(""),
        "version": manifest.get("version").cloned().unwrap_or(serde_json::Value::Null),
        "theme": theme,
        "animated": n_anim,
        "motion": motion,
        "icons": icons,
    });
    eprintln!(
        "[iconpack] pack at {} — {} animated icon(s), static theme '{}', motion engine {}",
        dir.display(),
        n_anim,
        if theme.is_empty() { "-" } else { &theme },
        if motion { "yes" } else { "no" }
    );
    Loaded { json: out.to_string(), dir: dir.to_string_lossy().to_string(), theme }
}
