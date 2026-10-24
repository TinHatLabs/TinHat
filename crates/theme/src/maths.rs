//! maths.rs — port of `maths.ts` (ENGINE.md §2): the deterministic colour maths (sRGB tables, cbrt, OKLab,
//! luminance, contrast).
//!
//! Only IEEE-754 double `+ - * /`, comparisons, `floor` and `sqrt`; every formula is written in the order of the
//! reference (`a*x + b*y + c*z` = `((a*x) + (b*y)) + (c*z)`, `l_*l_*l_` = `(l_*l_)*l_`). No `powf`/`cbrt`/`exp`, no
//! `mul_add` (Rust never contracts into FMA on its own). Same expressions, same order, same bits.

use crate::literal::Colour;
use std::sync::OnceLock;

/// The contract's `engine-tables.json` (a byte-identical copy lives next to Cargo.toml).
pub const ENGINE_TABLES_JSON: &str = include_str!("../engine-tables.json");

struct Tables {
    /// SRGB_TO_LIN[v] = sRGB decode of v/255.
    lin: [f64; 256],
    /// ENC_MID[i] = sRGB decode of (i + 0.5)/255: the encode thresholds.
    mid: [f64; 255],
}

static TABLES: OnceLock<Tables> = OnceLock::new();

fn tables() -> &'static Tables {
    TABLES.get_or_init(|| {
        // Correctly rounded decimal → double (str::parse via crate::json), as the README requires.
        let v = crate::json::parse_json(ENGINE_TABLES_JSON).expect("engine-tables.json is JSON");
        let read = |name: &str, n: usize| -> Vec<f64> {
            let a = v[name].as_array().unwrap_or_else(|| panic!("engine-tables.json: no {name}"));
            assert_eq!(a.len(), n, "engine-tables.json: bad table size for {name}");
            a.iter().map(|x| x.as_f64().expect("engine-tables.json: number")).collect()
        };
        let lin = read("SRGB_TO_LIN", 256);
        let mid = read("ENC_MID", 255);
        Tables { lin: lin.try_into().unwrap(), mid: mid.try_into().unwrap() }
    })
}

/// The `SRGB_TO_LIN` table (256 entries).
pub fn srgb_to_lin_table() -> &'static [f64; 256] {
    &tables().lin
}

/// The `ENC_MID` table (255 entries).
pub fn enc_mid_table() -> &'static [f64; 255] {
    &tables().mid
}

/// `f64_sha256` as recorded in engine-tables.json (the tests recompute it from the parsed doubles).
pub fn tables_sha256_hex() -> String {
    let v = crate::json::parse_json(ENGINE_TABLES_JSON).expect("engine-tables.json is JSON");
    v["f64_sha256"].as_str().unwrap_or_default().to_string()
}

/// sRGB decode of an 8-bit channel (table lookup).
pub fn srgb_to_linear(v: u8) -> f64 {
    tables().lin[v as usize]
}

/// sRGB encode of a linear value: clamp to `[0, 1]`, then the number of ENC_MID entries that are < x
/// (a value exactly on a threshold rounds down). Binary search.
pub fn linear_to_srgb(x: f64) -> u8 {
    if x.is_nan() || x <= 0.0 {
        return 0; // the reference's `!(x > 0)`: no threshold is < 0
    }
    let x = if x > 1.0 { 1.0 } else { x };
    let mid = &tables().mid;
    let (mut lo, mut hi) = (0usize, 255usize);
    while lo < hi {
        let m = (lo + hi) >> 1;
        if mid[m] < x {
            lo = m + 1;
        } else {
            hi = m;
        }
    }
    lo as u8
}

/// cbrt by exactly 64 Newton steps: y0 = 1 if x < 1 else x; y = (2*y + x/(y*y)) / 3. cbrt(0) = 0, cbrt(-x) = -cbrt(x).
pub fn cbrt(x: f64) -> f64 {
    if x == 0.0 {
        return 0.0;
    }
    if x < 0.0 {
        return -cbrt(-x);
    }
    let mut y = if x < 1.0 { 1.0 } else { x };
    for _ in 0..64 {
        y = (2.0 * y + x / (y * y)) / 3.0;
    }
    y
}

/// An OKLab colour.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lab {
    pub l: f64,
    pub a: f64,
    pub b: f64,
}

/// OKLab of an 8-bit sRGB colour (alpha ignored).
pub fn to_oklab(c: Colour) -> Lab {
    let t = tables();
    let r = t.lin[c.r as usize];
    let g = t.lin[c.g as usize];
    let b = t.lin[c.b as usize];
    let l = 0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b;
    let m = 0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b;
    let s = 0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b;
    let l_ = cbrt(l);
    let m_ = cbrt(m);
    let s_ = cbrt(s);
    Lab {
        l: 0.2104542553 * l_ + 0.7936177850 * m_ - 0.0040720468 * s_,
        a: 1.9779984951 * l_ - 2.4285922050 * m_ + 0.4505937099 * s_,
        b: 0.0259040371 * l_ + 0.7827717662 * m_ - 0.8086757660 * s_,
    }
}

/// OKLab → 8-bit sRGB channels: inverse matrices, cube as (x*x)*x, clamp to `[0, 1]`, table encode.
pub fn from_oklab(l: f64, a: f64, b: f64) -> (u8, u8, u8) {
    let l_ = l + 0.3963377774 * a + 0.2158037573 * b;
    let m_ = l - 0.1055613458 * a - 0.0638541728 * b;
    let s_ = l - 0.0894841775 * a - 1.2914855480 * b;
    let l3 = l_ * l_ * l_;
    let m3 = m_ * m_ * m_;
    let s3 = s_ * s_ * s_;
    let r = 4.0767416621 * l3 - 3.3077115913 * m3 + 0.2309699292 * s3;
    let g = -1.2684380046 * l3 + 2.6097574011 * m3 - 0.3413193965 * s3;
    let bb = -0.0041960863 * l3 - 0.7034186147 * m3 + 1.7076147010 * s3;
    (linear_to_srgb(r), linear_to_srgb(g), linear_to_srgb(bb))
}

/// OKLab chroma sqrt(A*A + B*B).
pub fn chroma(c: Colour) -> f64 {
    let lab = to_oklab(c);
    (lab.a * lab.a + lab.b * lab.b).sqrt()
}

/// WCAG relative luminance of the RGB channels (alpha ignored).
pub fn luminance(c: Colour) -> f64 {
    let t = tables();
    0.2126 * t.lin[c.r as usize] + 0.7152 * t.lin[c.g as usize] + 0.0722 * t.lin[c.b as usize]
}

/// (max(Y1,Y2) + 0.05) / (min(Y1,Y2) + 0.05).
pub fn contrast_of_y(y1: f64, y2: f64) -> f64 {
    let hi = if y1 > y2 { y1 } else { y2 };
    let lo = if y1 > y2 { y2 } else { y1 };
    (hi + 0.05) / (lo + 0.05)
}

/// Linear interpolation of the 4-decimal alpha: k = round((a1 + (a2 - a1) * t) * 10000), clamped to 0..10000
/// (the reference's `lerpAlpha`; returns k).
pub fn lerp_alpha(k1: u16, k2: u16, t: f64) -> u16 {
    let a1 = k1 as f64 / 10000.0;
    let a2 = k2 as f64 / 10000.0;
    let k = crate::js::js_round((a1 + (a2 - a1) * t) * 10000.0);
    if k < 0.0 {
        0
    } else if k > 10000.0 {
        10000
    } else {
        k as u16
    }
}

/// mixok(x, y, t): OKLab interpolation p + (q - p) * t per component, alpha linear (lerp_alpha).
pub fn mixok(x: Colour, y: Colour, t: f64) -> Colour {
    let p = to_oklab(x);
    let q = to_oklab(y);
    let (r, g, b) = from_oklab(p.l + (q.l - p.l) * t, p.a + (q.a - p.a) * t, p.b + (q.b - p.b) * t);
    Colour { r, g, b, k: lerp_alpha(x.k, y.k, t) }
}
