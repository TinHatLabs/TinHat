//! migrate.rs — port of `migrate.ts`: LAYOUT.md §6.3: 10.6's device prefs → the view state, and the exact seam
//! conversion (decision 9, Q-MIGRATE).

use crate::json::{fdiv, mk, J};

/// `Math.round` for x ≥ 0, spelled out (floor(x) + 1 when x − floor(x) ≥ 0.5) — on doubles, like the reference.
pub fn js_round(x: f64) -> f64 {
    let f = x.floor();
    if x - f >= 0.5 {
        f + 1.0
    } else {
        f
    }
}

/// The seam conversion: T = max(120 + chromeH, fdiv(fm · H, 1000)); p1 = fdiv(1000 · T, L); of p1 and p1 + 1 (each
/// clamped to 200..850) the per mille whose height fdiv(pm · L, 1000) is nearest to T, ties to the smaller. L ≤ 0 →
/// None (the conversion waits).
pub fn seam_pm(fm: i64, h: i64, l: i64, chrome_h: i64) -> Option<i64> {
    if l <= 0 {
        return None;
    }
    let t = (120 + chrome_h).max(fdiv(fm * h, 1000));
    let p1 = fdiv(1000 * t, l);
    let cands = [p1, p1 + 1].map(|p| p.clamp(200, 850));
    let mut best = cands[0];
    for p in cands {
        let d = (fdiv(p * l, 1000) - t).abs();
        let db = (fdiv(best * l, 1000) - t).abs();
        if d < db || (d == db && p < best) {
            best = p;
        }
    }
    Some(best)
}

/// migrate(prefs, H, L, chromeH) → the device's view state (LAYOUT.md §6.3).
pub fn migrate(prefs: Option<&J>, h: Option<&J>, l: Option<&J>, chrome_h: Option<&J>) -> J {
    let p = prefs.filter(|p| p.is_obj()).cloned().unwrap_or_else(J::obj);
    let view = J::obj();
    if p.is("nav_mode", "corner") {
        view.set("forceCollapse", J::strs(&["navcol"]));
    }
    let mut fm: Option<i64> = None;
    if let Some(f) = p.get("call-pane-frac").and_then(|f| f.as_f64()) {
        if f.is_finite() && f >= 0.0 {
            let r = js_round(f * 1000.0);
            fm = Some(if r < 200.0 {
                200
            } else if r > 850.0 {
                850
            } else {
                r as i64
            });
        }
    }
    if let Some(fm) = fm {
        // host bounds
        let px = |v: Option<&J>| -> Option<i64> {
            let v = v?;
            if !v.is_int() {
                return None;
            }
            let x = v.as_f64()?;
            Some(if x < 0.0 {
                0
            } else if x > 16384.0 {
                16384
            } else {
                x as i64
            })
        };
        let ch = px(chrome_h).unwrap_or(0);
        let pm = match (px(h), px(l)) {
            (Some(h), Some(l)) => seam_pm(fm, h, l, ch),
            _ => None,
        };
        match pm {
            Some(pm) => view.set("sizes", mk(vec![("call", mk(vec![("pm", J::int(pm))]))])),
            None => view.set("legacyCallPaneFm", J::int(fm)),
        }
    }
    view
}
