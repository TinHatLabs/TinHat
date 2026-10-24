//! literal.rs — port of `literal.ts` (ENGINE.md §1): the colour value, colour literals and the canonical text form.
//!
//! A colour is `{r, g, b}` 8-bit channels plus the alpha carried with at most 4 decimals. The reference holds the
//! alpha as the double `k / 10000`; the port stores the integer `k` (0..=10000) itself — every place the reference
//! computes with `a` uses exactly `k as f64 / 10000.0`, the same double.

/// An engine colour: 8-bit sRGB channels and alpha `k / 10000` (k = 0..=10000).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Colour {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    /// Alpha in 1/10000 units: 10000 = opaque, 0 = fully transparent.
    pub k: u16,
}

impl Colour {
    pub const WHITE: Colour = Colour { r: 255, g: 255, b: 255, k: 10000 };
    pub const BLACK: Colour = Colour { r: 0, g: 0, b: 0, k: 10000 };

    /// An opaque colour.
    pub const fn rgb(r: u8, g: u8, b: u8) -> Colour {
        Colour { r, g, b, k: 10000 }
    }

    /// Alpha as the reference holds it (`k / 10000`), for arithmetic and for QML (`Qt.rgba(r/255, g/255, b/255, a)`).
    pub fn alpha(&self) -> f64 {
        self.k as f64 / 10000.0
    }

    /// Alpha as an 8-bit value, rounded half up (for `QColor::fromRgb(r, g, b, a)`).
    pub fn alpha_u8(&self) -> u8 {
        ((self.k as u32 * 255 + 5000) / 10000) as u8
    }

    pub fn is_opaque(&self) -> bool {
        self.k >= 10000
    }

    /// Same RGB, alpha 1.
    pub fn opaque(&self) -> Colour {
        Colour { k: 10000, ..*self }
    }

    /// `(r, g, b, alpha)` — alpha 0.0..=1.0 with at most 4 decimals.
    pub fn rgba(&self) -> (u8, u8, u8, f64) {
        (self.r, self.g, self.b, self.alpha())
    }

    /// Any §1 literal (`#rgb`, `#rrggbb`, `#rrggbbaa`, canonical `#rrggbb/A`); `None` for anything else.
    pub fn parse(s: &str) -> Option<Colour> {
        parse_literal(s)
    }

    /// The canonical text form (`#rrggbb` or `#rrggbb/A`).
    pub fn to_canonical(&self) -> String {
        format_colour(*self)
    }

    /// `#rrggbb` (lower case), alpha ignored.
    pub fn hex6(&self) -> String {
        hex6(*self)
    }

    /// Qt's `#AARRGGBB` colour string (alpha FIRST, as QML's `color` type parses it), alpha rounded to 8 bits.
    pub fn to_qt_argb(&self) -> String {
        format!("#{:02x}{:02x}{:02x}{:02x}", self.alpha_u8(), self.r, self.g, self.b)
    }
}

impl std::fmt::Display for Colour {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&format_colour(*self))
    }
}

fn hex_val(ch: u8) -> Option<u8> {
    match ch {
        b'0'..=b'9' => Some(ch - b'0'),
        b'a'..=b'f' => Some(ch - b'a' + 10),
        b'A'..=b'F' => Some(ch - b'A' + 10),
        _ => None,
    }
}

/// The number grammar of the expression language and of the canonical alpha: `D{1,4}`, `D{1,4}.D{1,4}` or `.D{1,4}`
/// (ASCII digits). Returns k = value × 10000 as an exact integer, or -1 when the text is not such a number.
pub fn parse_decimal4(s: &str) -> i64 {
    let b = s.as_bytes();
    let n = b.len();
    let mut i = 0;
    let mut ip: i64 = 0;
    let mut idigits = 0;
    while i < n && b[i].is_ascii_digit() {
        idigits += 1;
        if idigits > 4 {
            return -1;
        }
        ip = ip * 10 + (b[i] - b'0') as i64;
        i += 1;
    }
    let mut fp: i64 = 0;
    let mut fdigits = 0;
    if i < n && b[i] == b'.' {
        i += 1;
        while i < n && b[i].is_ascii_digit() {
            fdigits += 1;
            if fdigits > 4 {
                return -1;
            }
            fp = fp * 10 + (b[i] - b'0') as i64;
            i += 1;
        }
        if fdigits == 0 {
            return -1; // "1." is not a number
        }
    }
    if i != n || idigits + fdigits == 0 {
        return -1;
    }
    for _ in fdigits..4 {
        fp *= 10;
    }
    ip * 10000 + fp
}

/// Colour literal (§1): `#rgb`, `#rrggbb`, `#rrggbbaa` (CSS order, alpha LAST: k = round(aa × 10000 / 255), computed
/// exactly as ⌊(aa × 20000 + 255) / 510⌋) or the canonical `#rrggbb/A` (A = 0..1, ≤ 4 decimals). Hex is
/// case-insensitive. `None` for anything else (no surrounding whitespace allowed).
pub fn parse_literal(s: &str) -> Option<Colour> {
    let b = s.as_bytes();
    // (UTF-16 vs UTF-8 length differ only for non-ASCII text, which can never be a literal.)
    if b.len() < 4 || b[0] != b'#' {
        return None;
    }
    let slash = b.iter().position(|&c| c == b'/');
    let hex_end = slash.unwrap_or(b.len());
    let mut hex = Vec::with_capacity(8);
    for &c in &b[1..hex_end] {
        hex.push(hex_val(c)?);
    }
    let byte = |i: usize| hex[i] * 16 + hex[i + 1];
    if let Some(sl) = slash {
        if hex.len() != 6 {
            return None;
        }
        let k = parse_decimal4(&s[sl + 1..]);
        if !(0..=10000).contains(&k) {
            return None;
        }
        return Some(Colour { r: byte(0), g: byte(2), b: byte(4), k: k as u16 });
    }
    match hex.len() {
        3 => Some(Colour::rgb(hex[0] * 17, hex[1] * 17, hex[2] * 17)),
        6 => Some(Colour::rgb(byte(0), byte(2), byte(4))),
        8 => {
            let aa = byte(6) as u32;
            let k = (aa * 20000 + 255) / 510;
            Some(Colour { r: byte(0), g: byte(2), b: byte(4), k: k as u16 })
        }
        _ => None,
    }
}

const HEX: &[u8; 16] = b"0123456789abcdef";

pub(crate) fn push_hex2(out: &mut String, v: u8) {
    out.push(HEX[(v >> 4) as usize] as char);
    out.push(HEX[(v & 15) as usize] as char);
}

/// `#rrggbb` (lower case) of the RGB channels, alpha ignored.
pub fn hex6(c: Colour) -> String {
    let mut s = String::with_capacity(7);
    s.push('#');
    push_hex2(&mut s, c.r);
    push_hex2(&mut s, c.g);
    push_hex2(&mut s, c.b);
    s
}

/// Canonical text form (§1, A2): `#rrggbb` when k = 10000, `#rrggbb/0` when k = 0, else `#rrggbb/0.dddd` with the 4
/// zero-padded digits of k and trailing zeros stripped (`#1e6a76/0.18`, `#000000/0.5`, `#ffffff/0.0001`).
/// Integer maths only.
pub fn format_colour(c: Colour) -> String {
    let h = hex6(c);
    if c.k >= 10000 {
        return h;
    }
    if c.k == 0 {
        return h + "/0";
    }
    let d = format!("{:04}", c.k); // integer printing, 1..9999
    let d = d.trim_end_matches('0');
    format!("{h}/0.{d}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qml_helpers() {
        let c = parse_literal("#1e6a76/0.18").unwrap();
        assert_eq!(c.rgba(), (0x1e, 0x6a, 0x76, 0.18));
        assert_eq!(c.alpha_u8(), 46); // 0.18 × 255 = 45.9
        assert_eq!(c.to_qt_argb(), "#2e1e6a76");
        assert_eq!(Colour::WHITE.to_qt_argb(), "#ffffffff");
        assert_eq!(parse_literal("#000000/0.5").unwrap().alpha_u8(), 128); // 127.5 rounds half up
        assert_eq!(parse_literal("#000000/0").unwrap().alpha_u8(), 0);
        assert_eq!(format!("{}", parse_literal("#ABC").unwrap()), "#aabbcc");
    }
}
