//! personal.rs — port of `personal.ts` (P22, 12.1.17): the text size and the density are PERSONAL settings layered over
//! the theme, never stored in it. Before 12.1.17 they lived in the theme document (`type.size`, `shape.density`), so
//! switching theme changed your size. Now:
//!
//! * the person's own prefs `text-size` (one of [`TEXT_SIZES`]) and `density` (one of the [`Density`] ids) decide the
//!   size and the density on every theme ([`effective_look`]); a theme's own `type.size` / `shape.density` are ignored;
//! * once, the first time an app with this module starts ([`migrate`]), the ACTIVE theme's old size and density become
//!   the person's default (the size snapped to the nearest step), so nobody's text jumps on the update;
//! * density is drawn: [`row`] is the height of a list row (room list rows, menu items) for a base height, the text
//!   size and the density; [`line`] is a line height scaled by the text size. Integer maths only, so both apps get the
//!   same pixels.
//!
//! The web's `src/theme/personal.ts` is the REFERENCE (the same names, the same rules); both apps run
//! `testvectors/personal/*.json` (tests/personal.rs; web personal.test.ts), byte-identical in both repos.

use serde_json::Value;

use crate::look::{read_look, Density, Look};

/// The text sizes a person can pick, in per cent of the app's normal size.
pub const TEXT_SIZES: [i64; 5] = [90, 100, 110, 125, 150];
pub const DEFAULT_TEXT_SIZE: i64 = 100;
pub const DEFAULT_DENSITY: Density = Density::Normal;

/// The pref keys (desktop: the `ui-<key>` files; web: `tinhat.pref.<key>`).
pub const PREF_TEXT_SIZE: &str = "text-size";
pub const PREF_DENSITY: &str = "density";
/// Set to "1" once [`migrate`] has run on this device.
pub const PREF_MIGRATED: &str = "look-personal";

/// The words on screen for each density (both apps show exactly these).
pub fn density_label(d: Density) -> &'static str {
    match d {
        Density::Compact => "Compact",
        Density::Normal => "Cozy",
        Density::Touch => "Comfortable",
    }
}

/// A stored `text-size` pref: exactly the decimal text of one of [`TEXT_SIZES`] ("125"); anything else → none.
pub fn parse_text_size(pref: Option<&str>) -> Option<i64> {
    let s = pref?;
    TEXT_SIZES.into_iter().find(|n| n.to_string() == s)
}

/// A stored `density` pref: exactly `compact` / `normal` / `touch`; anything else → none.
pub fn parse_density(pref: Option<&str>) -> Option<Density> {
    Density::parse(pref?)
}

/// The step nearest to a per-cent size (a theme's old `type.size`, 80–150); a tie goes to the LARGER step (readability
/// first). Below the smallest step → the smallest; above the largest → the largest.
pub fn snap_text_size(n: i64) -> i64 {
    let mut best = TEXT_SIZES[0];
    for s in TEXT_SIZES {
        let (d, bd) = ((s - n).abs(), (best - n).abs());
        if d < bd || (d == bd && s > best) {
            best = s;
        }
    }
    best
}

/// The person's size and density (from the prefs; defaults for a missing or bad pref).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Personal {
    pub size: i64,
    pub density: Density,
}

pub fn read_personal(text_size: Option<&str>, density: Option<&str>) -> Personal {
    Personal {
        size: parse_text_size(text_size).unwrap_or(DEFAULT_TEXT_SIZE),
        density: parse_density(density).unwrap_or(DEFAULT_DENSITY),
    }
}

/// The look an app draws: the theme's fonts and shadow, the PERSON's size and density.
pub fn effective_look(theme: &Value, p: Personal) -> Look {
    let mut l = read_look(theme);
    l.size = p.size;
    l.density = p.density;
    l
}

/// What [`migrate`] writes: `(key, value)` pairs, in this order; empty = nothing to do.
pub type PrefWrites = Vec<(&'static str, String)>;

/// The one-time move of a theme's size and density into the person's prefs. `migrated` / `text_size` / `density` are
/// the stored prefs (None = absent), `active_theme` the document of the theme in use when this runs.
///
/// Already migrated → nothing. Otherwise: a missing or bad `text-size` pref takes the theme's old size, snapped to a
/// step, and a missing or bad `density` pref takes the theme's old density — each written only when it is not the
/// default — and the marker is written last. A valid pref the person already has is never overwritten.
pub fn migrate(migrated: Option<&str>, text_size: Option<&str>, density: Option<&str>, active_theme: &Value) -> PrefWrites {
    if migrated == Some("1") {
        return Vec::new();
    }
    let old = read_look(active_theme);
    let mut out: PrefWrites = Vec::new();
    if parse_text_size(text_size).is_none() {
        let s = snap_text_size(old.size);
        if s != DEFAULT_TEXT_SIZE {
            out.push((PREF_TEXT_SIZE, s.to_string()));
        }
    }
    if parse_density(density).is_none() && old.density != DEFAULT_DENSITY {
        out.push((PREF_DENSITY, old.density.as_str().to_string()));
    }
    out.push((PREF_MIGRATED, "1".to_string()));
    out
}

/// `round(n × num / den)` for non-negative integers, halves up (the same integer on both apps).
fn mul_div_round(n: i64, num: i64, den: i64) -> i64 {
    (2 * n * num + den) / (2 * den)
}

/// A line height (or any type-driven length) of `base` px at the text size `size` (per cent), rounded to a whole px.
pub fn line(base: i64, size: i64) -> i64 {
    mul_div_round(base, size, 100)
}

/// How much a density adds to a row (layout px) and the least a row may be.
pub fn density_delta(d: Density) -> i64 {
    match d {
        Density::Compact => -8,
        Density::Normal => 0,
        Density::Touch => 8,
    }
}
pub fn density_min_row(d: Density) -> i64 {
    match d {
        Density::Compact => 24,
        Density::Normal => 28,
        Density::Touch => 44,
    }
}

/// The height of a list row whose design height is `base` px at 100 % / Cozy: grown with the text size, then the
/// density's step, never under the density's minimum (Comfortable keeps a 44 px touch target). At 100 % and Cozy it is
/// `base` itself for every base ≥ 28, so the default look is pixel-identical to before.
pub fn row(base: i64, size: i64, d: Density) -> i64 {
    (line(base, size) + density_delta(d)).max(density_min_row(d))
}

/// The space between groups (chat messages from different senders) whose design value is `base` px at Cozy:
/// Compact halves it (rounded down), Comfortable doubles it.
pub fn gap(base: i64, d: Density) -> i64 {
    match d {
        Density::Compact => base / 2,
        Density::Normal => base,
        Density::Touch => base * 2,
    }
}

/// The density drawn: the layout form's own `density` when it names one (the Handheld preset sets `touch`; LAYOUT.md
/// form field, any non-string or unknown value = none), else the person's.
pub fn effective_density(person: Density, layout: Option<&Value>) -> Density {
    layout.and_then(Value::as_str).and_then(Density::parse).unwrap_or(person)
}

/// The look's `{"density":…,"size":…}` part for QML / CSS consumers that only need the personal half.
pub fn personal_json(p: Personal) -> String {
    format!("{{\"density\":\"{}\",\"size\":{}}}", p.density.as_str(), p.size)
}
