//! look.rs — port of `look.ts` (contract ENGINE.md §10, step 5 — the theme editor): the non-colour look of a theme
//! beyond `shape.radius/gap/margin/border` ([`crate::shape`]): the island shadow, the density and the type block (fonts
//! from an allowed list, a text size). The web's `src/theme/look.ts` is the REFERENCE (the same rules, the same names);
//! both apps run `testvectors/editor/look-*.json` (tests/editor.rs).
//!
//! Like shape.rs: a theme without a field gets the default; a value that is not exactly what the rule allows is ignored
//! (the default stays), never clamped or guessed — so a shared theme can only name fonts from the lists below and can
//! never produce a size or density no other client would draw. Keys this reader does not know stay in the document.
//! JSON has no prototypes: a key named like a JavaScript `Object.prototype` member (`constructor`, `toString`) is an
//! ordinary key here, exactly as the reference's own-property reads treat it.

use serde_json::{json, Value};

use crate::canon::push_js_string;

/// The default density of the layout (`shape.density`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Density {
    Compact,
    Normal,
    Touch,
}

impl Density {
    pub fn as_str(self) -> &'static str {
        match self {
            Density::Compact => "compact",
            Density::Normal => "normal",
            Density::Touch => "touch",
        }
    }

    /// `compact` / `normal` / `touch` (exact, case-sensitive); anything else none.
    pub fn parse(s: &str) -> Option<Density> {
        DENSITIES.into_iter().find(|d| d.as_str() == s)
    }
}

/// Every density, in the reference's order.
pub const DENSITIES: [Density; 3] = [Density::Compact, Density::Normal, Density::Touch];

/// The theme's font families: a family from the allowed lists, or `None` = the app's own font for that slot.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Fonts {
    pub ui: Option<String>,
    pub chat: Option<String>,
    pub code: Option<String>,
}

/// The look of a theme document ([`read_look`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Look {
    /// Island shadow blur radius in layout px (0 = none), painted in the `island.shadow` colour.
    pub shadow: i64,
    pub density: Density,
    /// Font family names from the allowed lists; `None` = the app's own font for that slot.
    pub fonts: Fonts,
    /// Text size in per cent of the app's normal size.
    pub size: i64,
}

pub const LOOK_DEFAULTS: Look =
    Look { shadow: 0, density: Density::Normal, fonts: Fonts { ui: None, chat: None, code: None }, size: 100 };

/// `shape.shadow`: integer px, inclusive.
pub const SHADOW_BOUNDS: (i64, i64) = (0, 24);
/// `type.size`: integer per cent, inclusive.
pub const SIZE_BOUNDS: (i64, i64) = (80, 150);

/// Interface and chat fonts (ENGINE.md §10): sans-serif families, then serif families. Exact, case-sensitive names.
pub const FONTS_SANS: [&str; 18] = [
    "Noto Sans",
    "Inter",
    "Roboto",
    "Open Sans",
    "Lato",
    "Source Sans 3",
    "IBM Plex Sans",
    "Fira Sans",
    "Ubuntu",
    "Cantarell",
    "DejaVu Sans",
    "Liberation Sans",
    "Atkinson Hyperlegible",
    "Lexend",
    "Nunito",
    "Segoe UI",
    "Arial",
    "Verdana",
];
pub const FONTS_SERIF: [&str; 8] = [
    "Noto Serif",
    "Source Serif 4",
    "IBM Plex Serif",
    "Merriweather",
    "Literata",
    "DejaVu Serif",
    "Liberation Serif",
    "Georgia",
];
/// Code fonts.
pub const FONTS_MONO: [&str; 16] = [
    "Noto Sans Mono",
    "JetBrains Mono",
    "Fira Code",
    "Fira Mono",
    "Source Code Pro",
    "IBM Plex Mono",
    "Cascadia Code",
    "Cascadia Mono",
    "Hack",
    "Iosevka",
    "Inconsolata",
    "Ubuntu Mono",
    "DejaVu Sans Mono",
    "Liberation Mono",
    "Consolas",
    "Menlo",
];
/// The font slots of the `type` block.
pub const FONT_SLOTS: [&str; 3] = ["ui", "chat", "code"];

/// The families a slot may name: `code` → mono; `ui` / `chat` (any other slot, like the reference) → sans then serif.
pub fn fonts_for(slot: &str) -> Vec<&'static str> {
    if slot == "code" {
        FONTS_MONO.to_vec()
    } else {
        [FONTS_SANS.as_slice(), FONTS_SERIF.as_slice()].concat()
    }
}

/// `family` (a string; `None` = not a string) is on the list of `slot` (`ui`, `chat` or `code`; any other slot: false).
pub fn is_allowed_font(slot: &str, family: Option<&str>) -> bool {
    if slot != "ui" && slot != "chat" && slot != "code" {
        return false;
    }
    family.is_some_and(|f| fonts_for(slot).contains(&f))
}

/// `typeof v === "number" && Number.isInteger(v) && lo <= v <= hi` → the integer.
fn int_in(v: Option<&Value>, (lo, hi): (i64, i64)) -> Option<i64> {
    let x = v?.as_number()?.as_f64()?;
    (x.is_finite() && x.fract() == 0.0 && x >= lo as f64 && x <= hi as f64).then_some(x as i64)
}

/// The look of a theme document (any value; never panics).
pub fn read_look(theme: &Value) -> Look {
    let mut out = LOOK_DEFAULTS;
    let Some(theme) = theme.as_object() else { return out };
    if let Some(Value::Object(shape)) = theme.get("shape") {
        if let Some(sh) = int_in(shape.get("shadow"), SHADOW_BOUNDS) {
            out.shadow = sh;
        }
        if let Some(d) = shape.get("density").and_then(Value::as_str).and_then(Density::parse) {
            out.density = d;
        }
    }
    if let Some(Value::Object(ty)) = theme.get("type") {
        for slot in FONT_SLOTS {
            let f = ty.get(slot).and_then(Value::as_str);
            if is_allowed_font(slot, f) {
                let f = f.map(str::to_string);
                match slot {
                    "ui" => out.fonts.ui = f,
                    "chat" => out.fonts.chat = f,
                    _ => out.fonts.code = f,
                }
            }
        }
        if let Some(size) = int_in(ty.get("size"), SIZE_BOUNDS) {
            out.size = size;
        }
    }
    out
}

impl Look {
    /// The look as a JSON value `{shadow, density, fonts: {ui, chat, code}, size}` (the vectors' `expect`; its
    /// canonical text is [`look_json`]).
    pub fn to_value(&self) -> Value {
        json!({
            "shadow": self.shadow,
            "density": self.density.as_str(),
            "fonts": { "ui": self.fonts.ui, "chat": self.fonts.chat, "code": self.fonts.code },
            "size": self.size,
        })
    }
}

/// `{"density":…,"fonts":{"chat":…,"code":…,"ui":…},"shadow":…,"size":…}` (keys sorted) — what the desktop hands to
/// QML; the same text as `canon_json(&l.to_value())`.
pub fn look_json(l: &Look) -> String {
    let q = |out: &mut String, v: &Option<String>| match v {
        None => out.push_str("null"),
        Some(s) => push_js_string(out, s),
    };
    let mut out = String::from("{\"density\":");
    push_js_string(&mut out, l.density.as_str());
    out.push_str(",\"fonts\":{\"chat\":");
    q(&mut out, &l.fonts.chat);
    out.push_str(",\"code\":");
    q(&mut out, &l.fonts.code);
    out.push_str(",\"ui\":");
    q(&mut out, &l.fonts.ui);
    out.push_str(&format!("}},\"shadow\":{},\"size\":{}}}", l.shadow, l.size));
    out
}

/// [`look_json`] of the look of a theme document given as JSON text (text that is not JSON: the defaults).
pub fn look_json_of(doc_json: &str) -> String {
    let doc = crate::json::parse_json(doc_json).unwrap_or(Value::Null);
    look_json(&read_look(&doc))
}
