//! App size (2026-10-31, docs/UI-ZOOM.md): the whole desktop app scaled 70–200 % in 10 % steps on top of the OS
//! scaling — Ctrl+= / Ctrl++ / Ctrl+- / Ctrl+0 (the actions `app.zoom.in/out/reset`, ACTIONS.md example 15) and the
//! slider in Settings → Appearance → Size. Kept per device in the pref file `ui-scale` (never synced, kept on sign-out).
//!
//! How: Qt's own high-DPI scale factor, the one `QT_SCALE_FACTOR` sets, changed while the app runs
//! (ui_scale_bridge.cpp). Every window, popup and menu then renders at the new device pixel ratio — text, SVG icons and
//! layers are drawn at the new resolution (crisp, never a stretched picture) and every layout re-flows; windows keep
//! their size on screen. The pure rules live here (unit-tested); the bridge does the Qt part.
use std::ffi::c_int;
use std::sync::atomic::{AtomicI32, Ordering};

/// The smallest, largest and default size, in percent, and one step.
pub const MIN: i32 = 70;
pub const MAX: i32 = 200;
pub const DEFAULT: i32 = 100;
pub const STEP: i32 = 10;
/// The device pref file (settings.rs overlay prefs): "120" = 120 %. Absent / unreadable = 100.
pub const PREF: &str = "ui-scale";
/// 12.1.9: Settings → Appearance → "Change size instantly (experimental)": "1" = the live change also where it isn't
/// proven yet (Wayland), i.e. what TINHAT_UI_SCALE_LIVE=1 does, kept per device. Off by default; restart-only stays the
/// default on Wayland. The DPR walk guard (ui_scale_bridge.cpp) covers the live path wherever it runs (not `windows`).
pub const LIVE_PREF: &str = "ui-scale-live";

/// Any value → a size the app uses: clamped to MIN..=MAX and snapped to the nearest step (a half step rounds up).
pub fn normalize(p: i32) -> i32 {
    let c = p.clamp(MIN, MAX);
    let snapped = ((c - MIN) + STEP / 2) / STEP * STEP + MIN;
    snapped.min(MAX)
}

/// The stored text → the size: a whole number of percent (spaces and one trailing `%` allowed), else the default.
pub fn parse(s: &str) -> i32 {
    let t = s.trim();
    let t = t.strip_suffix('%').unwrap_or(t).trim();
    match t.parse::<i32>() {
        Ok(v) => normalize(v),
        Err(_) => DEFAULT,
    }
}

/// One key press: `dir` > 0 bigger, < 0 smaller, 0 back to the default. Stays inside the range.
pub fn step(cur: i32, dir: i32) -> i32 {
    let c = normalize(cur);
    match dir.signum() {
        1 => normalize(c + STEP),
        -1 => normalize(c - STEP),
        _ => DEFAULT,
    }
}

/// The scale factor Qt gets: the size × the factor the person's environment already asked for (`QT_SCALE_FACTOR`).
pub fn factor(p: i32, env_factor: f64) -> f64 {
    let base = if env_factor.is_finite() && env_factor > 0.0 { env_factor } else { 1.0 };
    base * normalize(p) as f64 / 100.0
}

/// The toast after a change (Ctrl+= / Ctrl+- / Ctrl+0, never the slider): the size, and why nothing changed at an end
/// of the range, or — where the size can't change live — that it applies at the next start (`pending`: the size asked
/// for is not the one this run started with).
pub fn toast(p: i32, prev: i32, dir: i32, pending: bool) -> String {
    let p = normalize(p);
    let mut t = format!("App size {p}%");
    if p == normalize(prev) && dir > 0 && p == MAX {
        t.push_str(" (the largest)");
    } else if p == normalize(prev) && dir < 0 && p == MIN {
        t.push_str(" (the smallest)");
    }
    if pending {
        t.push_str(" when Tin Hat restarts");
    }
    if p != DEFAULT {
        t.push_str(" · Ctrl+0 resets");
    }
    t
}

/// Where a size may change while Tin Hat runs (2026-11-01): only the platform plugins the live change was proven on —
/// `xcb` (X11, XWayland) and `windows`. On `wayland` pressing Ctrl+- crashed the published 12.1.6 Flatpak on KDE Plasma
/// (docs/UI-ZOOM.md "Where it changes live"), so there — and on any other plugin — a change is stored and applies at
/// the next start (the size at start is set before the first window, the moment Qt supports). `force_live`
/// (TINHAT_UI_SCALE_LIVE=1) lifts the list, for proving another platform; it never overrides a missing Qt call.
pub fn live_allowed_on(platform: &str, force_live: bool) -> bool {
    force_live || matches!(platform, "xcb" | "windows")
}

// ---- the Qt part (ui_scale_bridge.cpp) ---------------------------------------------------------------------------
extern "C" {
    fn tinhat_ui_scale_preinit(factor: f64, force_restart: c_int);
    fn tinhat_ui_scale_apply(factor: f64) -> c_int;
    fn tinhat_ui_scale_apply_startup(factor: f64) -> c_int;
    fn tinhat_ui_scale_live_capable() -> c_int;
    fn tinhat_ui_scale_why() -> c_int;
    fn tinhat_ui_scale_platform(buf: *mut std::ffi::c_char, cap: c_int) -> c_int;
}

/// The running Qt platform plugin ("" before QGuiApplication).
fn platform() -> String {
    let mut buf = [0 as std::ffi::c_char; 64];
    let n = unsafe { tinhat_ui_scale_platform(buf.as_mut_ptr(), buf.len() as c_int) };
    let n = (n.max(0) as usize).min(buf.len() - 1);
    String::from_utf8_lossy(&buf[..n].iter().map(|&c| c as u8).collect::<Vec<u8>>()).into_owned()
}

/// TINHAT_UI_SCALE_LIVE=1 (testing), or the Settings opt-in (LIVE_PREF): allow the live change on a platform not in
/// the list.
fn force_live() -> bool {
    std::env::var_os("TINHAT_UI_SCALE_LIVE").is_some_and(|v| v == "1") || live_opt_in()
}

/// The Settings opt-in "Change size instantly (experimental)" of this device.
pub fn live_opt_in() -> bool {
    crate::settings::overlay_pref_read(LIVE_PREF).map(|s| s.trim() == "1").unwrap_or(false)
}

/// Store the opt-in. Answers whether the size now changes live.
pub fn set_live_opt_in(on: bool) -> bool {
    crate::settings::overlay_pref_write(LIVE_PREF, if on { "1" } else { "0" });
    live_capable()
}

/// Is the opt-in worth offering here? Only where the live change is held back by the platform list alone: Qt has the
/// calls, TINHAT_UI_SCALE_RESTART isn't set, and the plugin is not one of the proven ones (in practice Wayland).
pub fn live_opt_in_offered() -> bool {
    offered_on(&platform(), !restart_only() && unsafe { tinhat_ui_scale_live_capable() } == 1)
}

fn offered_on(platform: &str, qt_capable: bool) -> bool {
    qt_capable && !live_allowed_on(platform, false)
}

/// The size Qt shows now: what this run started with (live or through QT_SCALE_FACTOR), then each live change (12.1.9:
/// with the opt-in a run can change live and then go back to restart-only, so "pending" compares with what is shown).
static STARTUP: AtomicI32 = AtomicI32::new(DEFAULT);

/// TINHAT_UI_SCALE_RESTART=1: never change the size live (it then applies at the next start) — the safety valve if a
/// platform ever misdraws a live change.
fn restart_only() -> bool {
    std::env::var_os("TINHAT_UI_SCALE_RESTART").is_some_and(|v| v == "1")
}

fn env_factor() -> f64 {
    std::env::var("QT_SCALE_FACTOR").ok().and_then(|v| v.trim().parse::<f64>().ok()).unwrap_or(1.0)
}

/// The stored size of this device.
pub fn stored() -> i32 {
    crate::settings::overlay_pref_read(PREF).map(|s| parse(&s)).unwrap_or(DEFAULT)
}

/// Before QGuiApplication: when the live path is unavailable, hand the stored size to Qt through QT_SCALE_FACTOR.
pub fn preinit() {
    let p = stored();
    STARTUP.store(p, Ordering::Relaxed);
    unsafe { tinhat_ui_scale_preinit(factor(p, 1.0), restart_only() as c_int) }
}

/// Right after QGuiApplication, before the first window: the stored size (the supported moment to set Qt's factor).
pub fn apply_startup() {
    let p = stored();
    // one line in every log: which path this build/run has (a restart-only build must be noticed — launch-gate 10-31)
    let plat = platform();
    let why = match unsafe { tinhat_ui_scale_why() } {
        0 if live_allowed_on(&plat, false) => "changes live".to_string(),
        0 if live_opt_in() => format!("changes live on '{plat}': the experimental setting is on"),
        0 if force_live() => format!("changes live on '{plat}': TINHAT_UI_SCALE_LIVE=1"),
        0 => format!("restart-only: a live change is not proven on '{plat}' yet"),
        1 => "restart-only: built without Qt's private headers".to_string(),
        2 => "restart-only: this Qt lacks the scale calls".to_string(),
        _ => "restart-only: TINHAT_UI_SCALE_RESTART=1".to_string(),
    };
    eprintln!("[ui-scale] App size {p}% at start; {why}");
    // the size at start goes in before the first window on EVERY platform with the calls (Qt's supported moment, the
    // same factor QT_SCALE_FACTOR sets); only the live change of a running app is limited to live_allowed_on
    if p != DEFAULT && unsafe { tinhat_ui_scale_apply_startup(factor(p, env_factor())) } == 0 && unsafe { tinhat_ui_scale_why() } == 0 {
        eprintln!("[ui-scale] the size at start was not set: a window already existed");
    }
}

/// Can a size change while Tin Hat runs here? (false: Qt lacks the calls, TINHAT_UI_SCALE_RESTART=1, or the platform
/// plugin is not one the live change is proven on — live_allowed_on)
pub fn live_capable() -> bool {
    !restart_only() && unsafe { tinhat_ui_scale_live_capable() } == 1 && live_allowed_on(&platform(), force_live())
}

/// Is `p` waiting for a restart (it can't change live, and it isn't the size this run started with)?
pub fn pending(p: i32) -> bool {
    !live_capable() && normalize(p) != STARTUP.load(Ordering::Relaxed)
}

/// Store `p` and apply it now. Answers the size stored and whether it changed live (false: at the next start).
pub fn set(p: i32) -> (i32, bool) {
    let p = normalize(p);
    crate::settings::overlay_pref_write(PREF, &p.to_string());
    // never the live call where it isn't allowed: the pref above is the change, the next start applies it
    let live = live_capable() && unsafe { tinhat_ui_scale_apply(factor(p, env_factor())) } == 1;
    if live {
        // the size now shown = the LAST size asked for: a handler running inside the change may ask for another one,
        // which the bridge coalesces and shows; that request wrote the pref after ours (review a045 area 3)
        STARTUP.store(stored(), Ordering::Relaxed);
    }
    (p, live)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_clamps_and_snaps() {
        assert_eq!(normalize(100), 100);
        assert_eq!(normalize(0), MIN);
        assert_eq!(normalize(-50), MIN);
        assert_eq!(normalize(69), MIN);
        assert_eq!(normalize(201), MAX);
        assert_eq!(normalize(i32::MAX), MAX);
        assert_eq!(normalize(i32::MIN), MIN);
        assert_eq!(normalize(124), 120);
        assert_eq!(normalize(125), 130); // a half step rounds up
        assert_eq!(normalize(196), 200);
        assert_eq!(normalize(74), 70);
        assert_eq!(normalize(75), 80);
    }

    #[test]
    fn live_only_where_proven() {
        // 2026-11-01: Ctrl+- crashed 12.1.6 on KDE Plasma Wayland — live only on the proven plugins
        assert!(live_allowed_on("xcb", false));
        assert!(live_allowed_on("windows", false));
        for p in ["wayland", "wayland-egl", "eglfs", "offscreen", "minimal", "cocoa", "", "XCB"] {
            assert!(!live_allowed_on(p, false), "{p}");
            assert!(live_allowed_on(p, true), "{p} with TINHAT_UI_SCALE_LIVE=1");
        }
    }

    #[test]
    fn opt_in_offered_only_where_the_list_holds_it_back() {
        // 12.1.9: the Settings opt-in shows on Wayland (and other unproven plugins), never where it changes live anyway,
        // and never where Qt can't change it live at all (a restart-only build / TINHAT_UI_SCALE_RESTART=1)
        for p in ["wayland", "wayland-egl", "eglfs"] {
            assert!(offered_on(p, true), "{p}");
            assert!(!offered_on(p, false), "{p} without the Qt calls");
        }
        for p in ["xcb", "windows"] {
            assert!(!offered_on(p, true), "{p}");
        }
    }

    #[test]
    fn every_value_in_range_is_a_step() {
        for v in -500..600 {
            let n = normalize(v);
            assert!((MIN..=MAX).contains(&n), "{v} → {n}");
            assert_eq!((n - MIN) % STEP, 0, "{v} → {n}");
            assert_eq!(normalize(n), n, "idempotent at {v}");
        }
    }

    #[test]
    fn parse_reads_the_pref() {
        assert_eq!(parse("120"), 120);
        assert_eq!(parse(" 150 \n"), 150);
        assert_eq!(parse("130%"), 130);
        assert_eq!(parse("130 %"), 130);
        assert_eq!(parse(""), DEFAULT);
        assert_eq!(parse("big"), DEFAULT);
        assert_eq!(parse("1.5"), DEFAULT);
        assert_eq!(parse("999"), MAX);
        assert_eq!(parse("10"), MIN);
        assert_eq!(parse("-120"), MIN);
    }

    #[test]
    fn step_walks_the_range() {
        assert_eq!(step(100, 1), 110);
        assert_eq!(step(100, -1), 90);
        assert_eq!(step(100, 7), 110); // only the sign counts
        assert_eq!(step(150, 0), DEFAULT);
        assert_eq!(step(MAX, 1), MAX);
        assert_eq!(step(MIN, -1), MIN);
        assert_eq!(step(123, 1), 130); // an off-step value snaps first
        // all the way up from the smallest: 14 presses, every step 10
        let mut p = MIN;
        let mut seen = vec![p];
        while p < MAX {
            p = step(p, 1);
            seen.push(p);
        }
        assert_eq!(seen.len(), 14);
        assert_eq!(seen, (0..14).map(|i| MIN + i * STEP).collect::<Vec<_>>());
    }

    #[test]
    fn factor_multiplies_the_environment() {
        assert_eq!(factor(100, 1.0), 1.0);
        assert_eq!(factor(150, 1.0), 1.5);
        assert_eq!(factor(150, 2.0), 3.0);
        assert_eq!(factor(70, 1.0), 0.7);
        assert_eq!(factor(120, 0.0), 1.2); // a nonsense environment counts as 1
        assert_eq!(factor(120, f64::NAN), 1.2);
        assert_eq!(factor(120, -2.0), 1.2);
        assert_eq!(factor(500, 1.0), 2.0); // clamped
    }

    #[test]
    fn toast_words() {
        assert_eq!(toast(110, 100, 1, false), "App size 110% · Ctrl+0 resets");
        assert_eq!(toast(100, 110, -1, false), "App size 100%");
        assert_eq!(toast(100, 150, 0, false), "App size 100%");
        assert_eq!(toast(200, 200, 1, false), "App size 200% (the largest) · Ctrl+0 resets");
        assert_eq!(toast(70, 70, -1, false), "App size 70% (the smallest) · Ctrl+0 resets");
        assert_eq!(toast(120, 110, 1, true), "App size 120% when Tin Hat restarts · Ctrl+0 resets");
        assert_eq!(toast(100, 100, 0, false), "App size 100%");
        // restart-only: still pending at an end of the range, or after a repeated press (review 2026-10-31)
        assert_eq!(toast(200, 200, 1, true), "App size 200% (the largest) when Tin Hat restarts · Ctrl+0 resets");
        assert_eq!(toast(130, 130, 0, true), "App size 130% when Tin Hat restarts · Ctrl+0 resets");
    }
}
