//! AudioPrefsController — per-(category, user) call-audio volume/mute prefs (#5).
//!
//! Persists `~/.local/share/Tin Hat/audio_prefs.json`, keyed `"<cat>_<userId>"` ->
//! `{ volume: f64 (1.0 default), muted: bool }`. On a `set*` it BOTH persists the pref
//! AND pushes it live to the call's per-track `volume` element via
//! `tinhat_streamer::set_track_gain` / `set_track_muted` (NO-OP at unity / when the
//! track is absent — see `crates/streamer/src/call.rs`). On call JOIN the app calls
//! `reapplyAll()` so a remembered volume re-sticks onto the freshly-subscribed tracks.
//!
//! `userId` here is a Matrix mxid (`@u:server`) as carried by the participant bubbles;
//! the streamer matches it against the full LiveKit identity (`@u:server:DEVICE`) by
//! prefix, so a remembered pref applies regardless of which device that user joins from.

use core::pin::Pin;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use cxx_qt::CxxQtType;
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
        type AudioPrefsController = super::AudioPrefsControllerRust;

        /// QML: volumeFor(userId, category) -> "1.0" .. "2.0" as a STRING (dodges the
        /// f64 QML-binding awkwardness; QML does parseFloat). Default "1.0" (unity).
        #[qinvokable]
        fn volume_for(self: Pin<&mut AudioPrefsController>, user_id: &QString, category: &QString) -> QString;
        /// QML: mutedFor(userId, category) -> bool. Default false.
        #[qinvokable]
        fn muted_for(self: Pin<&mut AudioPrefsController>, user_id: &QString, category: &QString) -> bool;
        /// QML: setVolume(userId, category, vol) — persist + push live (clamped 0–2).
        #[qinvokable]
        fn set_volume(self: Pin<&mut AudioPrefsController>, user_id: &QString, category: &QString, vol: f64);
        /// QML: pushVolume(userId, category, vol) — push live ONLY (no disk write). Used
        /// during a slider DRAG so we don't rewrite+chown the JSON on every pixel; the
        /// final value is committed with setVolume on release.
        #[qinvokable]
        fn push_volume(self: Pin<&mut AudioPrefsController>, user_id: &QString, category: &QString, vol: f64);
        /// QML: setMuted(userId, category, muted) — persist + push live.
        #[qinvokable]
        fn set_muted(self: Pin<&mut AudioPrefsController>, user_id: &QString, category: &QString, muted: bool);
        /// QML: reapplyAll() — push every saved pref onto the live call's tracks (call on
        /// JOIN so remembered volumes stick across rejoin). NO-OP for absent tracks.
        #[qinvokable]
        fn reapply_all(self: Pin<&mut AudioPrefsController>);
    }
}

/// One participant/category pref. `volume` is a linear factor (1.0 = unity).
#[derive(Clone, Copy)]
struct Pref {
    volume: f64,
    muted: bool,
}

impl Default for Pref {
    fn default() -> Self {
        Pref { volume: 1.0, muted: false }
    }
}

pub struct AudioPrefsControllerRust {
    // In-memory prefs, keyed (userId, category). Loaded from disk on first construction.
    prefs: Mutex<HashMap<(String, String), Pref>>,
}

impl Default for AudioPrefsControllerRust {
    fn default() -> Self {
        Self { prefs: Mutex::new(load_prefs()) }
    }
}

impl qobject::AudioPrefsController {
    pub fn volume_for(self: Pin<&mut Self>, user_id: &QString, category: &QString) -> QString {
        let key = (user_id.to_string(), category.to_string());
        let v = self
            .rust()
            .prefs
            .lock()
            .unwrap()
            .get(&key)
            .map(|p| p.volume)
            .unwrap_or(1.0);
        QString::from(&format!("{v}"))
    }

    pub fn muted_for(self: Pin<&mut Self>, user_id: &QString, category: &QString) -> bool {
        let key = (user_id.to_string(), category.to_string());
        self.rust()
            .prefs
            .lock()
            .unwrap()
            .get(&key)
            .map(|p| p.muted)
            .unwrap_or(false)
    }

    pub fn set_volume(self: Pin<&mut Self>, user_id: &QString, category: &QString, vol: f64) {
        let user = user_id.to_string();
        let cat = category.to_string();
        // NaN-safe (f64::clamp passes NaN through); fall back to unity on a non-finite vol.
        let v = if vol.is_finite() { vol.clamp(0.0, 2.0) } else { 1.0 };
        {
            let mut g = self.rust().prefs.lock().unwrap();
            let p = g.entry((user.clone(), cat.clone())).or_default();
            p.volume = v;
            save_prefs(&g);
        }
        // Push live (NO-OP at unity / when the track is absent).
        tinhat_streamer::set_track_gain(&user, &cat, v);
    }

    /// Push a gain live to the call WITHOUT persisting (slider-drag path). The final
    /// value lands on disk via [`Self::set_volume`] when the drag ends.
    pub fn push_volume(self: Pin<&mut Self>, user_id: &QString, category: &QString, vol: f64) {
        let v = if vol.is_finite() { vol.clamp(0.0, 2.0) } else { 1.0 };
        tinhat_streamer::set_track_gain(&user_id.to_string(), &category.to_string(), v);
    }

    pub fn set_muted(self: Pin<&mut Self>, user_id: &QString, category: &QString, muted: bool) {
        let user = user_id.to_string();
        let cat = category.to_string();
        {
            let mut g = self.rust().prefs.lock().unwrap();
            let p = g.entry((user.clone(), cat.clone())).or_default();
            p.muted = muted;
            save_prefs(&g);
        }
        tinhat_streamer::set_track_muted(&user, &cat, muted);
    }

    pub fn reapply_all(self: Pin<&mut Self>) {
        let g = self.rust().prefs.lock().unwrap();
        for ((user, cat), p) in g.iter() {
            // Only touch non-unity prefs (unity is already the track default, so skipping
            // keeps existing audio byte-identical for the common case).
            if (p.volume - 1.0).abs() > f64::EPSILON {
                tinhat_streamer::set_track_gain(user, cat, p.volume);
            }
            if p.muted {
                tinhat_streamer::set_track_muted(user, cat, true);
            }
        }
    }
}

/// `<app data dir>/audio_prefs.json` (Linux: `~/.local/share/Tin Hat`, honours
/// `XDG_DATA_HOME`; Windows: `%LOCALAPPDATA%\TinHatApp`).
fn prefs_path() -> PathBuf {
    crate::platform::app_data_dir().join("audio_prefs.json")
}

/// On-disk shape: `{ "<cat>_<userId>": { "volume": f64, "muted": bool }, … }`.
fn load_prefs() -> HashMap<(String, String), Pref> {
    let mut out = HashMap::new();
    let path = prefs_path();
    let Ok(text) = std::fs::read_to_string(&path) else {
        return out;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return out;
    };
    let Some(map) = v.as_object() else {
        return out;
    };
    for (k, entry) in map {
        // Split "<cat>_<userId>" at the FIRST '_' (category has no '_'; a userId/mxid may).
        let Some((cat, user)) = k.split_once('_') else {
            continue;
        };
        let volume = entry.get("volume").and_then(|x| x.as_f64()).unwrap_or(1.0).clamp(0.0, 2.0);
        let muted = entry.get("muted").and_then(|x| x.as_bool()).unwrap_or(false);
        out.insert((user.to_string(), cat.to_string()), Pref { volume, muted });
    }
    out
}

/// Persist the prefs map (best-effort; written by this process, so owned by the current user).
fn save_prefs(prefs: &HashMap<(String, String), Pref>) {
    let mut map = serde_json::Map::new();
    for ((user, cat), p) in prefs {
        map.insert(
            format!("{cat}_{user}"),
            serde_json::json!({ "volume": p.volume, "muted": p.muted }),
        );
    }
    let json = serde_json::Value::Object(map);
    let path = prefs_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string_pretty(&json) {
        let _ = std::fs::write(&path, text);
    }
}

/// Headless round-trip (no GUI): write a pref, reload it from disk, print a VERDICT.
/// Proves the JSON persistence path. Run via `TINHAT_AUDIOPREFS_TEST=1`.
pub fn headless_verify() {
    let key = ("@verify:example.com".to_string(), "mic".to_string());
    let mut m = load_prefs();
    m.insert(key.clone(), Pref { volume: 0.5, muted: true });
    save_prefs(&m);
    let reread = load_prefs();
    let p = reread.get(&key).copied().unwrap_or_default();
    let pass = (p.volume - 0.5).abs() < 1e-9 && p.muted;
    eprintln!(
        "[audioprefs] wrote vol=0.5 muted=true; reread vol={} muted={} -> {}",
        p.volume,
        p.muted,
        if pass { "PASS" } else { "FAIL" }
    );
    // Clean up the sentinel so the user's real prefs file isn't polluted.
    let mut m2 = load_prefs();
    m2.remove(&key);
    save_prefs(&m2);
    eprintln!("[audioprefs] path = {}", prefs_path().display());
}
