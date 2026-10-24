//! The strict JSON reader of peer-sent widget data (`tinhat.rxq`; WIDGETS.md §10.2) — port of the web's
//! `src/widgets/json.ts parseStrictJson`. Both apps read these texts with the same small RFC 8259 grammar (serde_json
//! alone does not match it: it keeps the last duplicate key):
//!
//! - whitespace: only space, tab, LF, CR (a BOM or any other character outside a string refuses the text)
//! - strings: no raw control characters (< U+0020); escapes `\" \\ \/ \b \f \n \r \t \uXXXX`; a `\u` high surrogate
//!   must be followed by a `\u` low surrogate (a pair), a lone one refuses the text. (A raw lone surrogate cannot be in
//!   a Rust string: the app decodes the payload as strict UTF-8 first — [`parse_strict_json_bytes`].)
//! - numbers: `-?(0|[1-9][0-9]*)(.[0-9]+)?([eE][+-]?[0-9]+)?`, read as the correctly rounded binary64 value
//!   (`str::parse::<f64>`, JS `Number()`); a value that is not finite (1e400) refuses the text
//! - objects: a key twice (after unescaping) refuses the text
//! - depth: the root value is level 1, a value inside an array/object one level deeper; a container at a level
//!   > max_depth refuses the text

use serde_json::{Map, Value};

struct P<'a> {
    b: &'a [u8],
    s: &'a str,
    i: usize,
    max: usize,
}

type R<T> = Result<T, ()>;

impl<'a> P<'a> {
    fn ws(&mut self) {
        while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }

    fn hex4(&mut self) -> R<u32> {
        if self.i + 4 > self.b.len() {
            return Err(());
        }
        let h = &self.b[self.i..self.i + 4];
        if !h.iter().all(u8::is_ascii_hexdigit) {
            return Err(());
        }
        self.i += 4;
        u32::from_str_radix(std::str::from_utf8(h).map_err(|_| ())?, 16).map_err(|_| ())
    }

    fn string(&mut self) -> R<String> {
        self.i += 1; // the opening quote
        let mut out = String::new();
        loop {
            let Some(&c) = self.b.get(self.i) else { return Err(()) };
            if c == b'"' {
                self.i += 1;
                return Ok(out);
            }
            if c < 0x20 {
                return Err(());
            }
            if c == b'\\' {
                let e = self.b.get(self.i + 1).copied();
                self.i += 2;
                match e {
                    Some(b'"') => out.push('"'),
                    Some(b'\\') => out.push('\\'),
                    Some(b'/') => out.push('/'),
                    Some(b'b') => out.push('\u{8}'),
                    Some(b'f') => out.push('\u{c}'),
                    Some(b'n') => out.push('\n'),
                    Some(b'r') => out.push('\r'),
                    Some(b't') => out.push('\t'),
                    Some(b'u') => {
                        let u = self.hex4()?;
                        if (0xdc00..=0xdfff).contains(&u) {
                            return Err(()); // a lone low surrogate
                        }
                        if (0xd800..=0xdbff).contains(&u) {
                            if self.b.get(self.i) != Some(&b'\\') || self.b.get(self.i + 1) != Some(&b'u') {
                                return Err(());
                            }
                            self.i += 2;
                            let lo = self.hex4()?;
                            if !(0xdc00..=0xdfff).contains(&lo) {
                                return Err(());
                            }
                            let cp = 0x10000 + ((u - 0xd800) << 10) + (lo - 0xdc00);
                            out.push(char::from_u32(cp).ok_or(())?);
                        } else {
                            out.push(char::from_u32(u).ok_or(())?);
                        }
                    }
                    _ => return Err(()),
                }
                continue;
            }
            // one whole UTF-8 character (the text is a &str: never a lone surrogate)
            let ch = self.s[self.i..].chars().next().ok_or(())?;
            out.push(ch);
            self.i += ch.len_utf8();
        }
    }

    fn number(&mut self) -> R<Value> {
        let b = self.b;
        let s = self.i;
        let mut i = self.i;
        if b.get(i) == Some(&b'-') {
            i += 1;
        }
        match b.get(i) {
            Some(b'0') => i += 1,
            Some(b'1'..=b'9') => {
                i += 1;
                while matches!(b.get(i), Some(b'0'..=b'9')) {
                    i += 1;
                }
            }
            _ => return Err(()),
        }
        // (.[0-9]+)? — only when a digit follows the dot
        if b.get(i) == Some(&b'.') && matches!(b.get(i + 1), Some(b'0'..=b'9')) {
            i += 2;
            while matches!(b.get(i), Some(b'0'..=b'9')) {
                i += 1;
            }
        }
        // ([eE][+-]?[0-9]+)? — only when at least one digit follows
        if matches!(b.get(i), Some(b'e' | b'E')) {
            let mut j = i + 1;
            if matches!(b.get(j), Some(b'+' | b'-')) {
                j += 1;
            }
            if matches!(b.get(j), Some(b'0'..=b'9')) {
                while matches!(b.get(j), Some(b'0'..=b'9')) {
                    j += 1;
                }
                i = j;
            }
        }
        let x: f64 = self.s[s..i].parse().map_err(|_| ())?;
        if !x.is_finite() {
            return Err(());
        }
        self.i = i;
        Ok(crate::num(x))
    }

    fn lit(&mut self, word: &str, v: Value) -> R<Value> {
        if self.b[self.i..].starts_with(word.as_bytes()) {
            self.i += word.len();
            Ok(v)
        } else {
            Err(())
        }
    }

    fn value(&mut self, level: usize) -> R<Value> {
        self.ws();
        let Some(&c) = self.b.get(self.i) else { return Err(()) };
        match c {
            b'{' => {
                if level > self.max {
                    return Err(());
                }
                self.i += 1;
                let mut o = Map::new();
                self.ws();
                if self.b.get(self.i) == Some(&b'}') {
                    self.i += 1;
                    return Ok(Value::Object(o));
                }
                loop {
                    self.ws();
                    if self.b.get(self.i) != Some(&b'"') {
                        return Err(());
                    }
                    let k = self.string()?;
                    if o.contains_key(&k) {
                        return Err(());
                    }
                    self.ws();
                    if self.b.get(self.i) != Some(&b':') {
                        return Err(());
                    }
                    self.i += 1;
                    let v = self.value(level + 1)?;
                    o.insert(k, v);
                    self.ws();
                    match self.b.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b'}') => {
                            self.i += 1;
                            return Ok(Value::Object(o));
                        }
                        _ => return Err(()),
                    }
                }
            }
            b'[' => {
                if level > self.max {
                    return Err(());
                }
                self.i += 1;
                let mut a = Vec::new();
                self.ws();
                if self.b.get(self.i) == Some(&b']') {
                    self.i += 1;
                    return Ok(Value::Array(a));
                }
                loop {
                    a.push(self.value(level + 1)?);
                    self.ws();
                    match self.b.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b']') => {
                            self.i += 1;
                            return Ok(Value::Array(a));
                        }
                        _ => return Err(()),
                    }
                }
            }
            b'"' => self.string().map(Value::String),
            b't' => self.lit("true", Value::Bool(true)),
            b'f' => self.lit("false", Value::Bool(false)),
            b'n' => self.lit("null", Value::Null),
            _ => self.number(),
        }
    }
}

/// `parseStrictJson(text, maxDepth)`: the value, or None when the grammar above refuses the text.
pub fn parse_strict_json(text: &str, max_depth: usize) -> Option<Value> {
    let mut p = P { b: text.as_bytes(), s: text, i: 0, max: max_depth };
    let v = p.value(1).ok()?;
    p.ws();
    (p.i == p.b.len()).then_some(v)
}

/// The app's path for received bytes: strict UTF-8 (malformed → refused; a BOM stays and is refused by the grammar),
/// then [`parse_strict_json`].
pub fn parse_strict_json_bytes(bytes: &[u8], max_depth: usize) -> Option<Value> {
    parse_strict_json(std::str::from_utf8(bytes).ok()?, max_depth)
}
