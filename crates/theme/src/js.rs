//! The few ECMAScript semantics the TypeScript reference relies on, written out once for the port.
//!
//! Everything else in the reference is plain integer / IEEE-754 double arithmetic that Rust performs identically
//! (Rust never contracts `a*b + c` into a fused multiply-add, and parses float literals correctly rounded).

/// ECMAScript `WhiteSpace` + `LineTerminator`: what `String.prototype.trim` strips and regex `\s` matches.
/// = Unicode White_Space minus U+0085, plus U+FEFF (README "Importers").
pub fn is_js_space(c: char) -> bool {
    (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}'
}

/// `String.prototype.trim`.
pub fn js_trim(s: &str) -> &str {
    s.trim_matches(is_js_space)
}

/// A regex `.` (no `s` flag) matches every code point except these.
pub fn is_js_line_terminator(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

/// `(.*)$` at the end of a regex without the `m` flag: the rest of the string holds no line terminator.
pub fn js_dot_star_to_end(s: &str) -> bool {
    !s.chars().any(is_js_line_terminator)
}

/// `Math.round` (ENGINE-QUESTIONS A4): exact for every x >= 0, including 0.49999999999999994 where
/// `(x + 0.5).floor()` is wrong (x + 0.5 rounds up to 1.0). `x - x.floor()` is exact for x >= 0.
/// (Negative inputs only occur where the result is clamped to 0 anyway; there it agrees with JS up to the sign
/// of zero.)
pub fn js_round(x: f64) -> f64 {
    let f = x.floor();
    if x - f >= 0.5 {
        f + 1.0
    } else {
        f
    }
}

/// Valid UTF-8 bytes that a byte-wise port produced (every multi-byte character is copied whole) → String.
/// Never lossy in practice; the fallback only avoids a panic path.
pub(crate) fn bytes_to_string(b: Vec<u8>) -> String {
    match String::from_utf8(b) {
        Ok(s) => s,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    }
}

/// `str.indexOf(needle, from)` on bytes (needles are ASCII, so positions are char boundaries).
pub(crate) fn find_from(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if from > hay.len() || needle.is_empty() {
        return if needle.is_empty() && from <= hay.len() { Some(from) } else { None };
    }
    hay[from..].windows(needle.len()).position(|w| w == needle).map(|p| p + from)
}

/// `str.indexOf(byte, from)`.
pub(crate) fn find_byte(hay: &[u8], byte: u8, from: usize) -> Option<usize> {
    if from >= hay.len() {
        return None;
    }
    hay[from..].iter().position(|&c| c == byte).map(|p| p + from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn js_round_is_math_round_for_non_negative_x() {
        // A4: the one value where (x + 0.5).floor() goes wrong.
        let x = 0.49999999999999994_f64;
        assert_eq!((x + 0.5).floor(), 1.0);
        assert_eq!(js_round(x), 0.0);
        let cases: &[(f64, f64)] = &[
            (0.0, 0.0),
            (0.5, 1.0),
            (1.5, 2.0),
            (2.5, 3.0),
            (1.4999999999999998, 1.0),
            (117.99999999999999, 118.0),
            (4503599627370495.5, 4503599627370496.0),
            (4503599627370497.0, 4503599627370497.0),
            (9007199254740993.0, 9007199254740992.0),
            (65534.5, 65535.0),
            (f64::INFINITY, f64::INFINITY),
        ];
        for &(x, want) in cases {
            assert_eq!(js_round(x), want, "js_round({x})");
        }
        assert!(js_round(f64::NAN).is_nan());
    }

    #[test]
    fn js_whitespace_is_ecmascript() {
        for c in ['\t', '\n', '\u{b}', '\u{c}', '\r', ' ', '\u{a0}', '\u{1680}', '\u{2000}', '\u{200a}', '\u{2028}',
            '\u{2029}', '\u{202f}', '\u{205f}', '\u{3000}', '\u{feff}']
        {
            assert!(is_js_space(c), "{:?}", c);
        }
        for c in ['\u{85}', '\u{180e}', '\u{200b}', 'a', '\u{0}'] {
            assert!(!is_js_space(c), "{:?}", c);
        }
        assert_eq!(js_trim("\u{feff} a b \u{3000}"), "a b");
        assert_eq!(js_trim("\u{85}a"), "\u{85}a");
    }
}
