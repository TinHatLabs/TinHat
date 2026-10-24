//! Startup window state (0.8.24): what the app is doing before the main window can show.
//!
//! A cold launch used to paint NOTHING for several seconds (GStreamer registry scan, the
//! 7.7k-line Main.qml load, session restore) — people assumed the app was broken. Now
//! main.rs shows `StartupWindow.qml` the moment Qt can paint, pushes a stage before each
//! blocking step and pumps the event loop so the window repaints; bridge.rs pushes the
//! session-restore / room-load stages; Main.qml keeps its own window hidden until the
//! rooms are in (or there is no session) and then calls `finish()`.
//!
//! One process-global state (`stage`, `detail`, `percent`, `finished`) + the qt-thread
//! handle of whichever `BootController` instance called `attach()` (the startup window's).
//! Every change is queued to that instance as a property update, so the window needs no
//! polling timer — which could not fire anyway while the GUI thread is blocked.
use core::pin::Pin;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use cxx_qt::{CxxQtThread, Threading};
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
        #[qproperty(QString, stage)] //       QML: stage — "Loading the interface", … (user-facing)
        #[qproperty(QString, detail)] //      QML: detail — second line (e.g. the account being restored)
        #[qproperty(i32, percent)] //         QML: percent — 0..100 for the bar
        #[qproperty(bool, finished)] //       QML: finished — main window revealed; fade out and hide
        #[qproperty(QString, version)] //     QML: version — "U0.1.0 · build 0.8.24"
        #[qproperty(QString, theme)] //       QML: theme — the persisted theme name (oled/goose/furry)
        #[qproperty(QString, assets_path)] // QML: assetsPath — bundled assets dir (app-icon layers, chimes)
        #[qproperty(bool, slow)] //           QML: slow — TINHAT_BOOT_SLOW=1 holds every stage (screenshots)
        #[qproperty(bool, safe_mode)] //      QML: safeMode — this start runs with software rendering (0.8.25)
        #[qproperty(QString, safe_reason)] // QML: safeReason — why (one line, user-facing)
        type BootController = super::BootControllerRust;

        /// QML (StartupWindow, Component.onCompleted): become the instance stage pushes go to.
        #[qinvokable]
        fn attach(self: Pin<&mut BootController>);
        /// QML (Main.qml): the main window is on screen — close the startup window.
        #[qinvokable]
        fn finish(self: Pin<&mut BootController>);
        /// QML (safe-mode banner): the user wants normal mode again next start.
        #[qinvokable]
        fn clear_safe_mode(self: Pin<&mut BootController>);
    }

    impl cxx_qt::Threading for BootController {}
}

pub struct BootControllerRust {
    stage: QString,
    detail: QString,
    percent: i32,
    finished: bool,
    version: QString,
    theme: QString,
    assets_path: QString,
    slow: bool,
    safe_mode: bool,
    safe_reason: QString,
}

impl Default for BootControllerRust {
    fn default() -> Self {
        let s = state().lock().unwrap().clone();
        Self {
            stage: QString::from(s.stage.as_str()),
            detail: QString::from(s.detail.as_str()),
            percent: s.percent,
            finished: s.finished,
            version: QString::from(
                format!("{} · build {}", crate::update::UNIVERSAL_VERSION, env!("CARGO_PKG_VERSION")).as_str(),
            ),
            // Same precedence as Main.qml: TINHAT_THEME (screenshot hook) > persisted choice.
            theme: QString::from(
                std::env::var("TINHAT_THEME").ok().filter(|t| !t.is_empty())
                    .unwrap_or_else(crate::bridge::read_saved_theme).as_str(),
            ),
            assets_path: QString::from(
                crate::platform::qml_file_prefix_path(&crate::assets::assets_dir()).as_str(),
            ),
            slow: slow_mode(),
            safe_mode: safe_mode(),
            safe_reason: QString::from(safe_reason().as_str()),
        }
    }
}

// ── Safe mode (0.8.25) ────────────────────────────────────────────────────────
//
// "It just doesn't launch" on a machine we cannot see (an old AMD GPU on a laptop, a
// driver that dies creating the GL context, a compositor quirk) used to be a dead end:
// the crash left no window and the next start died the same way. Now every start writes
// a marker BEFORE anything risky and clears it when the main window is on screen. A
// start that finds the marker still armed knows the previous one never got that far —
// and runs in SAFE MODE: software scene graph, no GL probe, GPU off for the web view.
// The following start goes back to normal (a one-off crash must not trap anyone), so a
// machine that only works in safe mode alternates until the cause is fixed — and every
// safe-mode start ships the failed start's log, which is the evidence we were missing.
//
// Env: TINHAT_SAFE_MODE=1 forces it, TINHAT_SAFE_MODE=0 disables the automatic decision.
//
// Where the marker lives: `<config home>/tinhat/boot-state` (settings::overlay_pref_path).
// History: on Windows the config home used to be a `.config` RELATIVE to the launch folder
// when this decision ran (HOME is unset there and XDG_CONFIG_HOME was exported only later),
// while the "ok" was written after the export — so the "starting" line was never cleared
// and every second start from the same place ran in safe mode. platform::init_dirs() now
// fixes the config home as main()'s first line.

const MARKER: &str = "boot-state";

struct SafeDecision {
    on: bool,
    reason: String,
    /// The marker line the previous start left ("" = none) — for the Windows log summary.
    #[cfg_attr(not(windows), allow(dead_code))]
    prev: String,
}

/// What the previous start's marker line means for this one (pure — unit-tested).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Verdict {
    /// Run this start in safe mode.
    on: bool,
    /// One user-facing line explaining why ("" when not in safe mode).
    reason: String,
    /// The previous start WAS safe mode and failed too (this one tries normal mode again).
    safe_also_failed: bool,
}

/// Pure: the safe-mode verdict for version `ver` from the previous start's marker line
/// `prev` ("<phase> <version> <safe 0|1> <unix seconds>", "" = none) and the
/// TINHAT_SAFE_MODE override `forced`.
fn decide(prev: &str, ver: &str, forced: Option<&str>) -> Verdict {
    let f: Vec<&str> = prev.split_whitespace().collect();
    let prev_phase = f.first().copied().unwrap_or("");
    let prev_ver = f.get(1).copied().unwrap_or("");
    let prev_safe = f.get(2).copied().unwrap_or("0") == "1";
    let (on, reason, safe_also_failed) = match forced {
        Some("1") => (true, "TINHAT_SAFE_MODE=1".to_string(), false),
        Some("0") => (false, String::new(), false),
        _ => {
            if prev_phase == "starting" && prev_ver == ver && !prev_safe {
                (true, "the last start of this version didn't reach the main window".to_string(), false)
            } else if prev_phase == "starting" && prev_ver == ver && prev_safe {
                (false, String::new(), true)
            } else {
                (false, String::new(), false)
            }
        }
    };
    Verdict { on, reason, safe_also_failed }
}

/// The marker line a start writes BEFORE anything risky.
fn starting_line(ver: &str, on: bool, now: u64) -> String {
    format!("starting {ver} {} {now}", if on { 1 } else { 0 })
}

/// The marker line written once the main window is on screen (or on a clean early exit).
fn ok_line(ver: &str, on: bool, now: u64) -> String {
    format!("ok {ver} {} {now}", if on { 1 } else { 0 })
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn safe_decision() -> &'static SafeDecision {
    static D: OnceLock<SafeDecision> = OnceLock::new();
    D.get_or_init(|| {
        let ver = env!("CARGO_PKG_VERSION");
        let prev = crate::settings::overlay_pref_read(MARKER).unwrap_or_default();
        let forced = std::env::var("TINHAT_SAFE_MODE").ok();
        let v = decide(&prev, ver, forced.as_deref());
        if v.safe_also_failed {
            eprintln!("[boot] the previous SAFE-MODE start also failed — trying normal mode again");
        }
        crate::settings::overlay_pref_write(MARKER, &starting_line(ver, v.on, unix_now()));
        let f: Vec<&str> = prev.split_whitespace().collect();
        let prev_phase = f.first().copied().unwrap_or("");
        let prev_ver = f.get(1).copied().unwrap_or("");
        if v.on {
            eprintln!("[boot] SAFE MODE: {} — software rendering, no GL probe, GPU off for the web view", v.reason);
        } else if prev_phase == "starting" && !prev_ver.is_empty() && prev_ver != ver {
            eprintln!("[boot] previous start ({prev_ver}) never reached the main window; this is {ver}, trying normally");
        }
        SafeDecision { on: v.on, reason: v.reason, prev }
    })
}

/// Is this start running in safe mode? Decided once, on the first call (main() calls it
/// first thing, before QGuiApplication, so the env it sets is seen by Qt).
pub fn safe_mode() -> bool {
    safe_decision().on
}

/// One user-facing line explaining safe mode ("" when not in safe mode).
pub fn safe_reason() -> String {
    safe_decision().reason.clone()
}

/// Export safe mode into the process environment BEFORE Qt / WebEngine / GStreamer init.
pub fn apply_safe_mode_env() {
    if !safe_mode() {
        return;
    }
    // Qt Quick: the software scene graph (video_bridge switches the RHI to Software too).
    std::env::set_var("TINHAT_SAFE_MODE_ACTIVE", "1");
    std::env::set_var("QT_QUICK_BACKEND", "software");
    // Any remaining GL user (GStreamer glupload, the web view) gets Mesa's software path.
    std::env::set_var("LIBGL_ALWAYS_SOFTWARE", "1");
    let flags = std::env::var("QTWEBENGINE_CHROMIUM_FLAGS").unwrap_or_default();
    std::env::set_var("QTWEBENGINE_CHROMIUM_FLAGS", format!("{flags} --disable-gpu").trim().to_string());
}

/// The main window reached the screen: disarm the marker (the next start is normal).
fn mark_boot_ok() {
    crate::settings::overlay_pref_write(MARKER, &ok_line(env!("CARGO_PKG_VERSION"), safe_mode(), unix_now()));
}

/// Windows, after the log redirect (main.rs): one line with the marker path, what the
/// previous start left and this start's mode — the decision itself runs before stderr reaches
/// the log file, so its own lines are lost on a shortcut or Explorer start. The copies
/// older builds left next to their launch folder are deliberately NOT touched: nothing
/// reads them any more (Setup deletes the data-root one on Install/Repair), and deleting files
/// outside the live marker's folder is not worth its risks (junctions, races — review of the
/// first build with the fix, 2026-09-26).
#[cfg(windows)]
pub fn report_start_marker() {
    let d = safe_decision();
    let live = crate::settings::overlay_pref_path(MARKER);
    let mode = if d.on {
        format!("SAFE MODE ({})", d.reason)
    } else if std::env::var("TINHAT_SAFE_MODE").as_deref() == Ok("0") {
        "normal start (TINHAT_SAFE_MODE=0)".to_string()
    } else {
        "normal start".to_string()
    };
    let prev = if d.prev.is_empty() { "none".to_string() } else { format!("\"{}\"", d.prev) };
    eprintln!("[boot] start marker {}: previous {prev} -> {mode}", live.display());
}

/// 0.8.28: exit a headless gate / early-exit run WITHOUT leaving the marker armed.
/// `safe_decision()` writes "starting" first thing, so any process that exits before the main
/// window — a headless test, a rejected second instance — used to make the NEXT real start run
/// in safe mode (a field report: relaunched while the old process was still closing, got
/// SAFE MODE for no reason). The second-instance case is now also ordered after the lock.
pub fn exit_early(code: i32) -> ! {
    mark_boot_ok();
    std::process::exit(code)
}

#[derive(Clone)]
struct BootState {
    stage: String,
    detail: String,
    percent: i32,
    finished: bool,
}

fn state() -> &'static Mutex<BootState> {
    static S: OnceLock<Mutex<BootState>> = OnceLock::new();
    S.get_or_init(|| {
        Mutex::new(BootState { stage: "Starting up".into(), detail: String::new(), percent: 5, finished: false })
    })
}

fn sink() -> &'static Mutex<Option<CxxQtThread<qobject::BootController>>> {
    static S: OnceLock<Mutex<Option<CxxQtThread<qobject::BootController>>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(None))
}

fn started() -> Instant {
    static T: OnceLock<Instant> = OnceLock::new();
    *T.get_or_init(Instant::now)
}

/// TINHAT_BOOT_SLOW=1: hold every stage ~1.5 s (screenshots / eyeballing the window).
pub fn slow_mode() -> bool {
    std::env::var_os("TINHAT_BOOT_SLOW").is_some()
}

/// Call once, first thing in main(): anchors the "+N ms" in the [boot] log lines.
pub fn mark_start() {
    let _ = started();
}

fn elapsed_ms() -> u128 {
    started().elapsed().as_millis()
}

fn push() {
    let Some(t) = sink().lock().unwrap().clone() else { return };
    let s = state().lock().unwrap().clone();
    let _ = t.queue(move |mut o| {
        o.as_mut().set_stage(QString::from(s.stage.as_str()));
        o.as_mut().set_detail(QString::from(s.detail.as_str()));
        o.as_mut().set_percent(s.percent);
        o.as_mut().set_finished(s.finished);
    });
}

/// Announce the next startup stage (user-facing wording; `detail` may be empty).
pub fn set_stage(stage: &str, detail: &str, percent: i32) {
    {
        let mut s = state().lock().unwrap();
        if s.finished {
            return; // the main window is up — nothing left to narrate
        }
        s.stage = stage.to_string();
        s.detail = detail.to_string();
        s.percent = percent.clamp(0, 100);
    }
    if detail.is_empty() {
        eprintln!("[boot] +{} ms {stage}", elapsed_ms());
    } else {
        eprintln!("[boot] +{} ms {stage} — {detail}", elapsed_ms());
    }
    push();
}

/// The main window is on screen: the startup window fades out and hides.
pub fn set_done() {
    {
        let mut s = state().lock().unwrap();
        if s.finished {
            return;
        }
        s.finished = true;
        s.percent = 100;
    }
    eprintln!("[boot] +{} ms main window shown — startup window closing", elapsed_ms());
    mark_boot_ok();
    push();
}

/// Let the startup window repaint between blocking startup steps (the GUI thread is not
/// in `app.exec()` yet, so nothing else would deliver the queued stage update or the
/// expose/render events).
pub fn pump() {
    for _ in 0..3 {
        crate::video::pump_events(20);
    }
}

/// `pump()` plus, in TINHAT_BOOT_SLOW mode, ~1.5 s of holding the stage on screen.
pub fn hold() {
    pump();
    if slow_mode() {
        for _ in 0..50 {
            crate::video::pump_events(20);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}

impl qobject::BootController {
    pub fn attach(self: Pin<&mut Self>) {
        *sink().lock().unwrap() = Some(self.qt_thread());
        eprintln!("[boot] +{} ms startup window attached", elapsed_ms());
        push();
    }

    pub fn finish(self: Pin<&mut Self>) {
        set_done();
    }

    pub fn clear_safe_mode(self: Pin<&mut Self>) {
        mark_boot_ok();
        eprintln!("[boot] safe mode cleared by the user — next start runs normally");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};

    const V: &str = "10.6.1";

    /// One start against a marker store: decide from the marker, arm "starting", and write
    /// "ok" only if the main window is reached (a crash, hang or kill never gets there).
    fn start(marker: &mut String, reaches_window: bool) -> bool {
        let v = decide(marker, V, None);
        *marker = starting_line(V, v.on, 100);
        if reaches_window {
            *marker = ok_line(V, v.on, 101);
        }
        v.on
    }

    #[test]
    fn good_starts_never_run_in_safe_mode() {
        let mut m = String::new();
        for _ in 0..6 {
            assert!(!start(&mut m, true));
        }
        assert_eq!(m, "ok 10.6.1 0 101");
    }

    #[test]
    fn a_start_that_dies_makes_the_next_one_safe_and_the_one_after_normal() {
        let mut m = String::new();
        assert!(!start(&mut m, true));
        assert!(!start(&mut m, false)); // killed / crashed / hung before the main window
        assert!(start(&mut m, true)); // safe mode — and it reaches the window
        assert_eq!(m, "ok 10.6.1 1 101");
        assert!(!start(&mut m, true)); // a good start cleared it
        assert!(!start(&mut m, true));
    }

    #[test]
    fn a_safe_start_that_dies_too_goes_back_to_normal_mode() {
        let mut m = "starting 10.6.1 1 5".to_string();
        let v = decide(&m, V, None);
        assert!(!v.on && v.safe_also_failed);
        assert!(!start(&mut m, true));
    }

    #[test]
    fn an_unfinished_start_of_another_version_does_not_trigger_safe_mode() {
        for prev in ["starting 10.6.0 0 5", "starting 10.6.0 1 5", "ok 10.6.1 0 5", "ok 10.6.1 1 5", "", "garbage"] {
            let v = decide(prev, V, None);
            assert!(!v.on && !v.safe_also_failed, "{prev:?}");
        }
        let v = decide("starting 10.6.1 0 5", V, None);
        assert!(v.on);
        assert_eq!(v.reason, "the last start of this version didn't reach the main window");
    }

    #[test]
    fn tinhat_safe_mode_forces_or_disables_the_decision() {
        let armed = "starting 10.6.1 0 5";
        assert!(decide("", V, Some("1")).on);
        assert_eq!(decide("", V, Some("1")).reason, "TINHAT_SAFE_MODE=1");
        assert!(!decide(armed, V, Some("0")).on);
        assert!(decide(armed, V, Some("anything else")).on);
    }

    /// Absolute by Windows rules (the production rule; Path::is_absolute answers for Linux).
    fn windows_absolute(p: &Path) -> bool {
        crate::platform::windows_is_absolute(p.as_os_str())
    }

    /// One Windows start as data (the alternating safe-mode bug). The decision reads and arms the marker where the
    /// environment points at that moment; the "ok" goes where it points once the main window
    /// is up. `fixed` = main() calls platform::init_dirs() first; otherwise the old
    /// order: the decision saw the raw environment (HOME unset → `.config`, which the OS
    /// resolves against the launch folder `cwd`).
    fn windows_start(store: &mut HashMap<PathBuf, String>, cwd: &str, fixed: bool, reaches_window: bool) -> bool {
        use crate::platform::{resolve_dirs, DirRules};
        let temp = Path::new(r"C:\Users\alice\AppData\Local\Temp");
        let raw = |k: &str| match k {
            "LOCALAPPDATA" => Some(OsString::from(r"C:\Users\alice\AppData\Local")),
            "USERPROFILE" => Some(OsString::from(r"C:\Users\alice")),
            _ => None,
        };
        let line_one = resolve_dirs(DirRules::Windows, &raw, temp); // what init_dirs() exports
        let after = |k: &str| {
            line_one.exports.iter().find(|(n, _)| *n == k).map(|(_, v)| v.clone().into_os_string()).or_else(|| raw(k))
        };
        let file = |config_home: PathBuf| {
            let p = config_home.join("tinhat").join(MARKER);
            if windows_absolute(&p) { p } else { Path::new(cwd).join(p) }
        };
        let at_decision = if fixed {
            file(resolve_dirs(DirRules::Windows, &after, temp).config_home)
        } else {
            file(resolve_dirs(DirRules::Unix, &raw, temp).config_home) // the old HOME fallback
        };
        let at_ok = file(resolve_dirs(DirRules::Windows, &after, temp).config_home); // after init_bundle_env
        let v = decide(&store.get(&at_decision).cloned().unwrap_or_default(), V, None);
        store.insert(at_decision, starting_line(V, v.on, 100));
        if reaches_window {
            store.insert(at_ok, ok_line(V, v.on, 101));
        }
        v.on
    }

    #[test]
    fn windows_old_order_alternated_and_the_fixed_order_never_does() {
        let shortcut = r"C:\Users\alice\AppData\Local\TinHatApp"; // Start-menu shortcut: Start in = data root
        let downloads = r"C:\Users\alice\Downloads\tinhat-10.6.0-win64"; // double-clicked zip copy
        for cwd in [shortcut, downloads] {
            let mut old = HashMap::new();
            let seq: Vec<bool> = (0..4).map(|_| windows_start(&mut old, cwd, false, true)).collect();
            assert_eq!(seq, vec![false, true, false, true], "the reproduced bug ({cwd})");
            let mut new = HashMap::new();
            let seq: Vec<bool> = (0..4).map(|_| windows_start(&mut new, cwd, true, true)).collect();
            assert_eq!(seq, vec![false; 4], "fixed ({cwd})");
            assert_eq!(new.len(), 1, "one marker file");
        }
        // Mixed launch folders share the one marker too.
        let mut m = HashMap::new();
        for cwd in [shortcut, downloads, shortcut, downloads] {
            assert!(!windows_start(&mut m, cwd, true, true));
        }
    }

    #[test]
    fn windows_fixed_order_keeps_the_safety_net_and_ignores_stale_copies() {
        let cwd = r"C:\Users\alice\AppData\Local\TinHatApp";
        let mut m = HashMap::new();
        // An upgrade over a pre-fix build: its armed copy next to the launch folder is ignored.
        m.insert(Path::new(cwd).join(".config").join("tinhat").join(MARKER), "starting 10.6.1 0 1".to_string());
        assert!(!windows_start(&mut m, cwd, true, true));
        // A start killed before the main window: the next one is safe mode, then normal again.
        assert!(!windows_start(&mut m, cwd, true, false));
        assert!(windows_start(&mut m, cwd, true, true));
        assert!(!windows_start(&mut m, cwd, true, true));
    }
}
