//! Bundled app assets — the app-icon layers and the themed call chimes — written to the
//! runtime asset dir on first launch of each build (see `ensure_assets`).

use std::path::PathBuf;

macro_rules! sound {
    ($p:expr) => {
        ($p, include_bytes!(concat!("../assets/sounds/", $p)) as &[u8])
    };
}

/// Themed sounds, written to `$dir/sounds/<theme>/<slot>.mp3` (QML plays them locally via file://).
/// Slots: join/leave (call chimes — remote peers AND your own transitions), send/receive (the open
/// room), notify + notify-2..4 (desktop-notification sound, user-picked), ring + ring-2 (incoming
/// call), muted (10.5: the talking-while-muted cue). 10.0: the OLED set is a royalty-free pack (2026-09-16); Goose keeps its own
/// join/leave and takes the OLED files for every other slot ("use the gaps from the new OLED one").
const SOUNDS: &[(&str, &[u8])] = &[
    sound!("oled/join.mp3"), sound!("oled/leave.mp3"),
    sound!("oled/send.mp3"), sound!("oled/receive.mp3"),
    sound!("oled/notify.mp3"), sound!("oled/notify-2.mp3"), sound!("oled/notify-3.mp3"), sound!("oled/notify-4.mp3"),
    sound!("oled/ring.mp3"), sound!("oled/ring-2.mp3"),
    // 10.5: the optional "you're talking while muted" cue (Settings › Calls; off by default) — a soft
    // two-tone down-step, 0.26 s, generated (sine 740 → 494 Hz), so no licence to track.
    sound!("oled/muted.mp3"),
    ("goose/muted.mp3", include_bytes!("../assets/sounds/oled/muted.mp3") as &[u8]),
    sound!("goose/join.mp3"), sound!("goose/leave.mp3"),
    ("goose/send.mp3", include_bytes!("../assets/sounds/oled/send.mp3") as &[u8]),
    ("goose/receive.mp3", include_bytes!("../assets/sounds/oled/receive.mp3") as &[u8]),
    ("goose/notify.mp3", include_bytes!("../assets/sounds/oled/notify.mp3") as &[u8]),
    ("goose/notify-2.mp3", include_bytes!("../assets/sounds/oled/notify-2.mp3") as &[u8]),
    ("goose/notify-3.mp3", include_bytes!("../assets/sounds/oled/notify-3.mp3") as &[u8]),
    ("goose/notify-4.mp3", include_bytes!("../assets/sounds/oled/notify-4.mp3") as &[u8]),
    ("goose/ring.mp3", include_bytes!("../assets/sounds/oled/ring.mp3") as &[u8]),
    ("goose/ring-2.mp3", include_bytes!("../assets/sounds/oled/ring-2.mp3") as &[u8]),
];

/// The app icon (10.0: the Tin Hat mark): one 256 px PNG bundled for the sign-in card, the
/// startup window, the window/taskbar icon and (Windows) toasts. The loader is pure vector
/// (TinHatLoader.qml) so no layer images ship any more.
const APP_ASSETS: &[(&str, &[u8])] = &[
    ("tinhat-256.png", include_bytes!("../assets/appicon/tinhat-256.png") as &[u8]),
];

/// Runtime asset dir: `<app data dir>/assets` (Linux: `$XDG_DATA_HOME/Tin Hat/assets`
/// default `~/.local/share/...`; Windows: `%LOCALAPPDATA%\TinHatApp\assets`).
pub fn assets_dir() -> PathBuf {
    crate::platform::app_data_dir().join("assets")
}

/// Write the bundled assets to the runtime dir (idempotent; rewrites so updates land).
pub fn ensure_assets() {
    let dir = assets_dir();
    // Build-hash sentinel: skip the ~62 synchronous file writes on every launch AFTER the first
    // for a given build. We rewrite only when the baked build hash changes — i.e. after an update,
    // which is exactly when the bundled assets could differ. `option_env!` reads the hash baked by
    // build.rs (cargo:rustc-env=TINHAT_BUILD_HASH); on a hashless dev build it's None → we never skip
    // (preserving the old always-rewrite behavior). The sentinel lives INSIDE `dir`, so if the
    // assets dir is wiped the sentinel goes with it → mismatch → full rewrite.
    let sentinel = dir.join(".asset-build");
    if let Some(hash) = option_env!("TINHAT_BUILD_HASH") {
        if std::fs::read_to_string(&sentinel).map(|s| s == hash).unwrap_or(false) {
            return;
        }
    }
    // Track whether every write succeeded; only stamp the sentinel if so (below), so a
    // partial/failed pass is retried next launch instead of being cached as "current".
    let mut all_ok = true;
    for (rel, bytes) in APP_ASSETS {
        let path = dir.join("appicon").join(rel);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        all_ok &= std::fs::write(&path, bytes).is_ok();
    }
    // Themed sounds -> $dir/sounds/<theme>/<slot>.mp3 (QML plays via file://).
    for (rel, bytes) in SOUNDS {
        let path = dir.join("sounds").join(rel);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        all_ok &= std::fs::write(&path, bytes).is_ok();
    }
    // Stamp the sentinel only after EVERY write in this pass succeeded — so a crash mid-write
    // OR a silently-failed write never marks an incomplete asset set as current; it's retried
    // next launch (preserves the old always-rewrite self-heal). No-op on hashless dev builds.
    if all_ok {
        if let Some(hash) = option_env!("TINHAT_BUILD_HASH") {
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(&sentinel, hash);
        }
    }
}
