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

// ---- the Qt part (ui_scale_bridge.cpp) ---------------------------------------------------------------------------
extern "C" {
    fn tinhat_ui_scale_preinit(factor: f64, force_restart: c_int);
    fn tinhat_ui_scale_apply(factor: f64) -> c_int;
    fn tinhat_ui_scale_live_capable() -> c_int;
    fn tinhat_ui_scale_why() -> c_int;
}

/// The size this run started with (what Qt got at start, live or through QT_SCALE_FACTOR).
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
    let why = match unsafe { tinhat_ui_scale_why() } {
        0 => "changes live",
        1 => "restart-only: built without Qt's private headers",
        2 => "restart-only: this Qt lacks the scale calls",
        _ => "restart-only: TINHAT_UI_SCALE_RESTART=1",
    };
    eprintln!("[ui-scale] App size {p}% at start; {why}");
    if p != DEFAULT {
        unsafe {
            tinhat_ui_scale_apply(factor(p, env_factor()));
        }
    }
}

/// Can a size change while Tin Hat runs here? (false: Qt lacks the calls, or TINHAT_UI_SCALE_RESTART=1)
pub fn live_capable() -> bool {
    !restart_only() && unsafe { tinhat_ui_scale_live_capable() } == 1
}

/// Is `p` waiting for a restart (it can't change live, and it isn't the size this run started with)?
pub fn pending(p: i32) -> bool {
    !live_capable() && normalize(p) != STARTUP.load(Ordering::Relaxed)
}

/// Store `p` and apply it now. Answers the size stored and whether it changed live (false: at the next start).
pub fn set(p: i32) -> (i32, bool) {
    let p = normalize(p);
    crate::settings::overlay_pref_write(PREF, &p.to_string());
    let live = !restart_only() && unsafe { tinhat_ui_scale_apply(factor(p, env_factor())) } == 1;
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
