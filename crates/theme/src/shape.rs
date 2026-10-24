//! A theme's `shape` block (ENGINE.md §4, step 1b; ISLANDS-ARCH §3.4): the island geometry in layout px — corner
//! radius, the gap between islands, the margin around them, the frame's border width. Port of the web's
//! `src/theme/shape.ts` (the same rules, the same test names in `tests/shape.rs`). A theme without the block (every
//! library theme, OLED and Goose) gets the defaults; a value that is not an integer inside its bounds is ignored (the
//! default stays), never clamped. Keys this reader does not know stay in the theme document.
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shape {
    pub radius: i64,
    pub gap: i64,
    pub margin: i64,
    pub border: i64,
}

pub const SHAPE_DEFAULTS: Shape = Shape { radius: 12, gap: 6, margin: 6, border: 1 };
/// (key, low, high) — inclusive bounds
pub const SHAPE_BOUNDS: [(&str, i64, i64); 4] = [("radius", 0, 24), ("gap", 0, 24), ("margin", 0, 24), ("border", 0, 2)];

pub fn read_shape(theme: &Value) -> Shape {
    let mut out = SHAPE_DEFAULTS;
    let Some(s) = theme.as_object().and_then(|t| t.get("shape")).and_then(Value::as_object) else {
        return out;
    };
    for (k, lo, hi) in SHAPE_BOUNDS {
        let Some(v) = s.get(k).and_then(Value::as_f64) else { continue };
        if v.fract() != 0.0 || v < lo as f64 || v > hi as f64 {
            continue;
        }
        let n = v as i64;
        match k {
            "radius" => out.radius = n,
            "gap" => out.gap = n,
            "margin" => out.margin = n,
            _ => out.border = n,
        }
    }
    out
}

impl Shape {
    /// `{"border":…,"gap":…,"margin":…,"radius":…}` (keys sorted) — what the app hands to QML.
    pub fn to_json(&self) -> String {
        format!(r#"{{"border":{},"gap":{},"margin":{},"radius":{}}}"#, self.border, self.gap, self.margin, self.radius)
    }
}
