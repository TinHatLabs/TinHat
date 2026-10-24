//! ops.rs — port of `ops.ts`: the colour functions of the expression language (ENGINE.md §3) on engine colours,
//! plus the parts of the web's `src/ui/color.ts` they use (Qt's colour arithmetic, bit for bit), line by line:
//! `mix` (Kirigami linearInterpolation), `qtLighter` / `qtDarker` (QColor::lighter/darker, 16-bit HSV) and `over`.
//! lighter/darker turn each 16-bit channel into 8 bits with Qt 6's `qt_div_257` (round to nearest), as color.ts does
//! since PORT-QUESTIONS Q5 (Qt 5's formula before).
//!
//! The reference feeds color.ts the opaque `#rrggbb` form and carries the 4-decimal alpha itself (A3): mix
//! interpolates it (lerp_alpha), lighter/darker keep it, over is opaque. color.ts' `formatColor` (3 alpha decimals)
//! is NOT ported: for opaque colours it only rounds and clamps each channel (`hex2`), which is what happens here.

use crate::js::js_round;
use crate::literal::Colour;
use crate::maths::{contrast_of_y, lerp_alpha, luminance, mixok};

const USHRT_MAX: f64 = 65535.0;

/// color.ts `hex2`: `Math.max(0, Math.min(255, Math.round(n)))`.
fn channel(n: f64) -> u8 {
    let r = js_round(n);
    if r > 255.0 {
        255
    } else if r > 0.0 {
        r as u8
    } else {
        0
    }
}

/// color.ts `qtDiv257` (re-exported by ops.ts): Qt 6's `qt_div_257` (qdrawhelper_p.h, every Qt 6 release) — a 16-bit
/// channel (0..65535) → 8 bits rounded to nearest, `qt_div_257_floor(x + 128)` = ⌊(x + 128) / 257⌋ = round(x / 257)
/// (PORT-QUESTIONS Q5; Qt 5's `(x - (x >> 8) + 0x80) >> 8`, color.ts's until then, rounds x = 257k + 128, k < 128, up).
pub fn qt_div_257(x: u32) -> u8 {
    let y = x + 128;
    ((y - (y >> 8)) >> 8) as u8
}

/// color.ts `toHsv16`: QColor's 16-bit HSV. h = degrees × 100 (or USHRT_MAX when achromatic), s and v 0..65535.
fn to_hsv16(c: Colour) -> (f64, f64, f64) {
    let r = (c.r as f64 * 257.0) / USHRT_MAX;
    let g = (c.g as f64 * 257.0) / USHRT_MAX;
    let b = (c.b as f64 * 257.0) / USHRT_MAX;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let v = js_round(max * USHRT_MAX);
    if delta.abs() < 1e-6 {
        return (USHRT_MAX, 0.0, v);
    }
    let mut hue = if r == max {
        (g - b) / delta
    } else if g == max {
        2.0 + (b - r) / delta
    } else {
        4.0 + (r - g) / delta
    };
    hue *= 60.0;
    if hue < 0.0 {
        hue += 360.0;
    }
    (js_round(hue * 100.0), js_round((delta / max) * USHRT_MAX), v)
}

/// color.ts `fromHsv16` (alpha handled by the caller), with Qt 6's 16 → 8-bit step (`qt_div_257`).
fn from_hsv16(h: f64, s: f64, v: f64) -> (u8, u8, u8) {
    let (rr, gg, bb);
    if s == 0.0 || h == USHRT_MAX {
        rr = v;
        gg = v;
        bb = v;
    } else {
        let h = if h == 36000.0 { 0.0 } else { h / 6000.0 };
        let s = s / USHRT_MAX;
        let v = v / USHRT_MAX;
        let i = h.floor();
        let f = h - i;
        let p = v * (1.0 - s);
        let ii = i as i64;
        let (x, y, z) = if ii & 1 == 1 {
            let q = v * (1.0 - s * f);
            match ii {
                1 => (q, v, p),
                3 => (p, q, v),
                _ => (v, p, q),
            }
        } else {
            let t = v * (1.0 - s * (1.0 - f));
            match ii {
                0 => (v, t, p),
                2 => (p, v, t),
                _ => (t, p, v),
            }
        };
        rr = js_round(x * USHRT_MAX);
        gg = js_round(y * USHRT_MAX);
        bb = js_round(z * USHRT_MAX);
    }
    (qt_div_257(rr as u32), qt_div_257(gg as u32), qt_div_257(bb as u32))
}

/// color.ts `qtLighterInt(color, factor)`.
fn qt_lighter_int(c: Colour, factor: f64) -> (u8, u8, u8) {
    let (h, s0, v0) = to_hsv16(c);
    let mut s = s0;
    let mut v = ((factor * v0) / 100.0).trunc();
    if v > USHRT_MAX {
        s -= v - USHRT_MAX;
        if s < 0.0 {
            s = 0.0;
        }
        v = USHRT_MAX;
    }
    from_hsv16(h, s, v)
}

/// color.ts `qtDarkerInt(color, factor)`.
fn qt_darker_int(c: Colour, factor: f64) -> (u8, u8, u8) {
    let (h, s, v0) = to_hsv16(c);
    let v = ((v0 * 100.0) / factor).trunc();
    from_hsv16(h, s, v)
}

/// color.ts `qtLighter(color, factor)` = QColor::lighter(int(round(factor*100))) on the RGB.
fn qt_lighter(c: Colour, factor: f64) -> (u8, u8, u8) {
    let f = js_round(factor * 100.0);
    if f <= 0.0 {
        return (c.r, c.g, c.b);
    }
    if f < 100.0 {
        return qt_darker_int(c, (10000.0 / f).trunc());
    }
    qt_lighter_int(c, f)
}

/// color.ts `qtDarker(color, factor)` = QColor::darker(int(round(factor*100))) on the RGB.
fn qt_darker(c: Colour, factor: f64) -> (u8, u8, u8) {
    let f = js_round(factor * 100.0);
    if f <= 0.0 {
        return (c.r, c.g, c.b);
    }
    if f < 100.0 {
        return qt_lighter_int(c, (10000.0 / f).trunc());
    }
    qt_darker_int(c, f)
}

/// alpha(x, n): same colour, alpha SET to n (n = k / 10000).
pub fn set_alpha(x: Colour, k: u16) -> Colour {
    Colour { k, ..x }
}

/// mix(x, y, t): Kirigami linearInterpolation in 8-bit sRGB (color.ts mix), alpha lerp to 4 decimals.
pub fn mix_c(x: Colour, y: Colour, t: f64) -> Colour {
    let ch = |a: u8, b: u8| channel(a as f64 + (b as f64 - a as f64) * t);
    Colour { r: ch(x.r, y.r), g: ch(x.g, y.g), b: ch(x.b, y.b), k: lerp_alpha(x.k, y.k, t) }
}

/// lighter(x, f): QColor::lighter(int(round(f*100))) (color.ts qtLighter), alpha kept.
pub fn lighter_c(x: Colour, f: f64) -> Colour {
    let (r, g, b) = qt_lighter(x, f);
    Colour { r, g, b, k: x.k }
}

/// darker(x, f): QColor::darker(int(round(f*100))) (color.ts qtDarker), alpha kept.
pub fn darker_c(x: Colour, f: f64) -> Colour {
    let (r, g, b) = qt_darker(x, f);
    Colour { r, g, b, k: x.k }
}

/// over(top, under): top composited with its alpha over under (under's alpha ignored) → opaque (color.ts over:
/// `t.r * t.a + u.r * (1 - t.a)` per channel, rounded and clamped).
pub fn over_c(top: Colour, under: Colour) -> Colour {
    let a = top.alpha();
    let ch = |t: u8, u: u8| channel(t as f64 * a + u as f64 * (1.0 - a));
    Colour::rgb(ch(top.r, under.r), ch(top.g, under.g), ch(top.b, under.b))
}

/// cr(fg, bg): WCAG contrast. bg counts as opaque (its alpha is ignored); a translucent fg is composited over it
/// first (over), as the text would be painted.
pub fn cr(fg: Colour, bg: Colour) -> f64 {
    let b = bg.opaque();
    let f = if fg.is_opaque() { fg } else { over_c(fg, b) };
    contrast_of_y(luminance(f), luminance(b))
}

/// best(bg, c1, c2, …): the first candidate with cr ≥ 4.5 against bg, else the first one with the highest cr.
pub fn best_c(bg: Colour, cands: &[Colour]) -> Colour {
    let mut pick = cands[0];
    let mut pick_cr = -1.0;
    for &c in cands {
        let r = cr(c, bg);
        if r >= 4.5 {
            return c;
        }
        if r > pick_cr {
            pick_cr = r;
            pick = c;
        }
    }
    pick
}

/// guardAll(fg, bgs, n) (§9 amendment 2026-09-27): fg must reach n against EVERY surface in bgs; T from bgs[0]
/// (surface.island); `passes(c)` = every cr(c, bg) ≥ n (a NaN fails, like the reference's `>=`); the same 24-step
/// bisection as guard_c. The reference: web src/theme/ops.ts guardAllC.
pub fn guard_all_c(fg: Colour, bgs: &[Colour], n: f64) -> Colour {
    let passes = |c: Colour| bgs.iter().all(|&bg| cr(c, bg) >= n);
    if passes(fg) {
        return fg;
    }
    let target = if luminance(bgs[0]) < 0.18 { Colour::WHITE } else { Colour::BLACK };
    if !passes(mixok(fg, target, 1.0)) {
        return target;
    }
    let mut lo = 0.0_f64;
    let mut hi = 1.0_f64;
    for _ in 0..24 {
        let mid = (lo + hi) / 2.0;
        if passes(mixok(fg, target, mid)) {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    mixok(fg, target, hi)
}

/// guard(fg, bg, n) (§5): fg if cr(fg, bg) ≥ n. Otherwise T = #ffffff when Y(bg) < 0.18 else #000000;
/// c(t) = mixok(fg, T, t). If c(1) fails too, T. Else bisection lo = 0, hi = 1, exactly 24 steps of
/// mid = (lo + hi) / 2 (pass → hi = mid, fail → lo = mid); the answer is c(hi), the smallest passing t found.
pub fn guard_c(fg: Colour, bg: Colour, n: f64) -> Colour {
    if cr(fg, bg) >= n {
        return fg;
    }
    let target = if luminance(bg) < 0.18 { Colour::WHITE } else { Colour::BLACK };
    let reach = cr(mixok(fg, target, 1.0), bg);
    if reach.is_nan() || reach < n {
        return target; // the reference's `!(cr(mixok(fg, T, 1), bg) >= n)`: even the target colour itself fails
    }
    let mut lo = 0.0_f64;
    let mut hi = 1.0_f64;
    for _ in 0..24 {
        let mid = (lo + hi) / 2.0;
        if cr(mixok(fg, target, mid), bg) >= n {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    mixok(fg, target, hi)
}
