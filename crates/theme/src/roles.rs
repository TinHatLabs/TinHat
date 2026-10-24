//! roles.rs — port of `roles.ts`: the v1 role ids (NAMING.md §2), the palette slots, the readability guards and the
//! derivation of every role from a terminal palette (ENGINE.md §5), written as expressions of the §3 language.
//! All of it is data: a threshold or a derivation changes here and nowhere else.

use crate::literal::{format_colour, parse_literal, Colour};
use crate::maths::chroma;
use crate::ops::cr;

/// The palette slots that `name.1` … `name.12` take (in order).
pub const NAME_ROLE_SLOTS: [&str; 12] = ["c1", "c2", "c3", "c4", "c5", "c6", "c9", "c10", "c11", "c12", "c13", "c14"];

/// Every v1 role id, sorted by id (code-point order; a unit test checks the order).
pub const ROLE_IDS: [&str; 52] = [
    "accent.base",
    "accent.bright",
    "accent.dim",
    "accent.glow",
    "badge.bg",
    "badge.mention",
    "border.focus",
    "border.strong",
    "border.subtle",
    "call.muted",
    "call.speaking",
    "mention.bg",
    "mention.edge",
    "name.1",
    "name.10",
    "name.11",
    "name.12",
    "name.2",
    "name.3",
    "name.4",
    "name.5",
    "name.6",
    "name.7",
    "name.8",
    "name.9",
    "presence.away",
    "presence.busy",
    "presence.incall",
    "presence.offline",
    "presence.online",
    "scrim",
    "shadow",
    "state.aside",
    "state.danger",
    "state.dangerBright",
    "state.info",
    "state.success",
    "state.warning",
    "surface.hover",
    "surface.island",
    "surface.overlay",
    "surface.pressed",
    "surface.raised",
    "surface.selected",
    "surface.sunken",
    "surface.window",
    "text.disabled",
    "text.link",
    "text.muted",
    "text.onAccent",
    "text.primary",
    "text.secondary",
];

/// Is `id` a v1 role?
pub fn is_role(id: &str) -> bool {
    ROLE_IDS.binary_search(&id).is_ok()
}

/// Palette slots: the only refs besides roles inside role expressions of a palette theme.
pub const PALETTE_SLOTS: [&str; 21] = [
    "bg", "fg", "cursor", "selectionBg", "selectionFg", "c0", "c1", "c2", "c3", "c4", "c5", "c6", "c7", "c8", "c9",
    "c10", "c11", "c12", "c13", "c14", "c15",
];

pub fn is_palette_slot(id: &str) -> bool {
    PALETTE_SLOTS.contains(&id)
}

/// The surfaces a guarded role must be readable on (§9 amendment 2026-09-27); the first one sets the direction.
pub const GUARD_SURFACES: [&str; 4] = ["surface.island", "surface.window", "surface.raised", "surface.hover"];

/// The readability guards of §5 (+ §9 C1): role → minimum contrast against every GUARD_SURFACES surface.
/// Body text 4.5; everything else 3.0, including the bold `name.*` labels.
pub const GUARDS: [(&str, f64); 32] = [
    ("text.primary", 4.5),
    ("text.secondary", 4.5),
    ("text.muted", 3.0),
    ("accent.base", 3.0),
    ("accent.bright", 3.0),
    ("text.link", 4.5),
    ("state.danger", 3.0),
    ("state.dangerBright", 3.0),
    ("state.warning", 3.0),
    ("state.success", 3.0),
    ("state.info", 3.0),
    ("state.aside", 3.0),
    ("mention.edge", 3.0),
    ("presence.online", 3.0),
    ("presence.away", 3.0),
    ("presence.busy", 3.0),
    ("presence.incall", 3.0),
    ("presence.offline", 3.0),
    ("call.speaking", 3.0),
    ("call.muted", 3.0),
    // name colours are bold labels: 3.0 (ENGINE.md §9 C1); body text stays 4.5
    ("name.1", 3.0),
    ("name.2", 3.0),
    ("name.3", 3.0),
    ("name.4", 3.0),
    ("name.5", 3.0),
    ("name.6", 3.0),
    ("name.7", 3.0),
    ("name.8", 3.0),
    ("name.9", 3.0),
    ("name.10", 3.0),
    ("name.11", 3.0),
    ("name.12", 3.0),
];

/// The guard threshold of a role, if it is guarded.
pub fn guard_of(id: &str) -> Option<f64> {
    GUARDS.iter().find(|(r, _)| *r == id).map(|&(_, n)| n)
}

/// A parsed terminal palette (all opaque).
#[derive(Clone, Debug, PartialEq)]
pub struct Palette {
    pub bg: Colour,
    pub fg: Colour,
    pub cursor: Option<Colour>,
    pub selection_bg: Option<Colour>,
    pub selection_fg: Option<Colour>,
    pub ansi: [Colour; 16],
}

/// The colour of a palette slot (`bg`, `fg`, `cursor`, `selectionBg`, `selectionFg`, `c0` … `c15`).
pub fn palette_slot(p: &Palette, id: &str) -> Option<Colour> {
    match id {
        "bg" => return Some(p.bg),
        "fg" => return Some(p.fg),
        "cursor" => return p.cursor,
        "selectionBg" => return p.selection_bg,
        "selectionFg" => return p.selection_fg,
        _ => {}
    }
    ansi_slot_number(id).map(|n| p.ansi[n])
}

/// `c0` → 0, `c1`…`c15` → 1…15 (`/^c[1-9][0-9]?$/`, below 16); anything else none.
fn ansi_slot_number(id: &str) -> Option<usize> {
    let b = id.as_bytes();
    if b.len() < 2 || b.len() > 3 || b[0] != b'c' {
        return None;
    }
    if id == "c0" {
        return Some(0);
    }
    if !(b'1'..=b'9').contains(&b[1]) || (b.len() == 3 && !b[2].is_ascii_digit()) {
        return None;
    }
    let n: usize = id[1..].parse().ok()?;
    (n < 16).then_some(n)
}

/// The accent field: "cursor", "c1".."c15" or a literal. Returns the slot number for cN, 0 for the others, -1 when
/// invalid.
pub fn accent_slot(accent: &str) -> i32 {
    if accent == "cursor" {
        return 0;
    }
    // /^c([1-9]|1[0-5])$/
    let b = accent.as_bytes();
    let is_slot = match b {
        [b'c', d] => (b'1'..=b'9').contains(d),
        [b'c', b'1', d] => (b'0'..=b'5').contains(d),
        _ => false,
    };
    if is_slot {
        return accent[1..].parse().unwrap_or(-1);
    }
    if parse_literal(accent).is_some() {
        0
    } else {
        -1
    }
}

/// The derivation table of §5 for one palette theme, as expressions (in the reference's table order). `dark` is the
/// theme's variant. accent.base: the theme's `accent` (cursor | cN | literal); absent, or "cursor" without a cursor in
/// the palette → the default rule (cursor if present, ≠ fg and OKLab chroma ≥ 0.04, else c4). The accent SLOT (for
/// accent.bright) is N when accent.base is cN — including the c4 fallback — else none.
pub fn derive_role_exprs(p: &Palette, dark: bool, accent: Option<&str>) -> Vec<(&'static str, String)> {
    let accent_expr: String;
    let mut slot = 0;
    if accent.is_none() || (accent == Some("cursor") && p.cursor.is_none()) {
        match p.cursor {
            Some(c) if c != p.fg && chroma(c) >= 0.04 => accent_expr = "cursor".into(),
            _ => {
                accent_expr = "c4".into();
                slot = 4;
            }
        }
    } else {
        let a = accent.unwrap_or_default();
        if a == "cursor" {
            accent_expr = "cursor".into();
        } else if let Some(n) = a.strip_prefix('c') {
            accent_expr = a.to_string();
            slot = n.parse().unwrap_or(0);
        } else {
            accent_expr = format_colour(parse_literal(a).expect("accent validated by accent_slot"));
        }
    }
    let sel_ok = match p.selection_bg {
        Some(sel) => cr(p.fg, sel) >= 3.0,
        None => false,
    };
    let pick = |d: &str, l: &str| if dark { d.to_string() } else { l.to_string() };
    let mut d: Vec<(&'static str, String)> = vec![
        ("surface.island", "bg".into()),
        ("surface.window", pick("mixok(bg, #000000, 0.25)", "mixok(bg, fg, 0.06)")),
        ("surface.raised", pick("mixok(bg, fg, 0.07)", "mixok(bg, #ffffff, 0.6)")),
        ("surface.hover", "mixok(bg, fg, 0.10)".into()),
        ("surface.pressed", "mixok(bg, fg, 0.14)".into()),
        ("surface.selected", if sel_ok { "selectionBg".into() } else { "mixok(bg, accent.base, 0.25)".into() }),
        ("surface.sunken", "mixok(bg, surface.window, 0.5)".into()),
        ("surface.overlay", "surface.raised".into()),
        ("scrim", pick("alpha(#000000, 0.5)", "alpha(#000000, 0.3)")),
        ("border.subtle", "mixok(bg, fg, 0.13)".into()),
        ("border.strong", "mixok(bg, fg, 0.24)".into()),
        ("border.focus", "accent.bright".into()),
        ("text.primary", "fg".into()),
        ("text.secondary", "mixok(fg, bg, 0.28)".into()),
        ("text.muted", "mixok(fg, bg, 0.45)".into()),
        ("text.disabled", "mixok(fg, bg, 0.60)".into()),
        ("accent.base", accent_expr),
        (
            "accent.bright",
            if (1..=6).contains(&slot) {
                format!("c{}", slot + 8)
            } else {
                pick("mixok(accent.base, #ffffff, 0.3)", "mixok(accent.base, #000000, 0.2)")
            },
        ),
        ("accent.dim", "alpha(accent.base, 0.18)".into()),
        ("accent.glow", "alpha(accent.base, 0.35)".into()),
        ("text.link", "accent.bright".into()),
        ("text.onAccent", "best(accent.base, #ffffff, bg, fg, #000000)".into()),
        ("state.danger", "c1".into()),
        ("state.dangerBright", "c9".into()),
        ("state.warning", "c3".into()),
        ("state.success", "c2".into()),
        ("state.info", "c6".into()),
        ("state.aside", "c11".into()),
        ("mention.bg", "alpha(state.warning, 0.10)".into()),
        ("mention.edge", "state.warning".into()),
        ("badge.bg", "accent.base".into()),
        ("badge.mention", "state.danger".into()),
        ("presence.online", "c2".into()),
        ("presence.away", "c3".into()),
        ("presence.busy", "c1".into()),
        ("presence.incall", "accent.bright".into()),
        ("presence.offline", "text.muted".into()),
        ("call.speaking", "state.success".into()),
        ("call.muted", "state.dangerBright".into()),
        ("shadow", "#000000".into()), // opaque; every shadow token applies its own alpha (ENGINE.md §5, 2026-09-26)
    ];
    const NAME_IDS: [&str; 12] =
        ["name.1", "name.2", "name.3", "name.4", "name.5", "name.6", "name.7", "name.8", "name.9", "name.10", "name.11", "name.12"];
    for (i, s) in NAME_ROLE_SLOTS.iter().enumerate() {
        d.push((NAME_IDS[i], s.to_string()));
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_ids_are_sorted_unique_and_complete() {
        let mut sorted = ROLE_IDS.to_vec();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted, ROLE_IDS.to_vec());
        for (id, _) in GUARDS {
            assert!(is_role(id), "{id}");
        }
        let p = Palette {
            bg: Colour::BLACK,
            fg: Colour::WHITE,
            cursor: None,
            selection_bg: None,
            selection_fg: None,
            ansi: [Colour::WHITE; 16],
        };
        let mut derived: Vec<&str> = derive_role_exprs(&p, true, None).iter().map(|(id, _)| *id).collect();
        derived.sort();
        assert_eq!(derived, ROLE_IDS.to_vec());
    }

    #[test]
    fn slots() {
        for (s, want) in [("c0", Some(0)), ("c9", Some(9)), ("c15", Some(15)), ("c16", None), ("c00", None), ("c05", None), ("c", None), ("x1", None)] {
            assert_eq!(ansi_slot_number(s), want, "{s}");
        }
        for (s, want) in [("cursor", 0), ("c1", 1), ("c15", 15), ("c0", -1), ("c16", -1), ("c01", -1), ("#fff", 0), ("#ffffff/0.5", 0), ("blue", -1), ("", -1)] {
            assert_eq!(accent_slot(s), want, "{s}");
        }
    }
}
