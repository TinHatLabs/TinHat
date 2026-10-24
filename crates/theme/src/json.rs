//! json.rs — `JSON.parse` for the port: the strict ECMA-404 grammar, producing a `serde_json::Value` that behaves
//! like the JavaScript value the reference would hold.
//!
//! - numbers are doubles, parsed correctly rounded (`str::parse::<f64>`; serde_json's default float parser can be
//!   1 ulp off). Integral values in the i64 range are stored as integers (same double, friendlier for serde users);
//!   a number that overflows to ±Infinity becomes `null` — exactly what `JSON.stringify` prints for it, and
//!   equivalent in every check the engine makes (it is neither a string, nor 1, nor a container);
//! - duplicate keys: the last value wins (like `JSON.parse`); `__proto__` is an ordinary key (like `JSON.parse`);
//! - rules shared with the reference (ENGINE.md §9, PORT-QUESTIONS Q2/Q3 — resolved: the reference adopted them):
//!   nesting deeper than the given limit is refused ([`MAX_NESTING`] = 128 everywhere), and so is a `\u` escape that
//!   leaves a lone UTF-16 surrogate (a Rust string cannot hold one; the reference checks its strings for them).
//!
//! Used by the JSON importer (Windows Terminal / tinhat-theme files), the tables and the vector tests; the app may
//! use it to read theme documents so that numbers mean the same thing on both sides.

use serde_json::{Map, Number, Value};

/// Deepest array/object nesting of any JSON value or parsed document (ENGINE.md §9; serde_json's own default).
pub const MAX_NESTING: usize = 128;

/// The input is not JSON (the only failure `JSON.parse` has).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonError {
    /// Byte offset where parsing stopped (diagnostics only; never part of a contract message).
    pub at: usize,
}

impl std::fmt::Display for JsonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "not valid JSON (at byte {})", self.at)
    }
}

impl std::error::Error for JsonError {}

/// `JSON.parse(text)` with nesting limited to [`MAX_NESTING`].
pub fn parse_json(text: &str) -> Result<Value, JsonError> {
    parse_json_with_limit(text, MAX_NESTING)
}

/// `JSON.parse(text)` with an explicit nesting limit (arrays + objects).
pub fn parse_json_with_limit(text: &str, max_nesting: usize) -> Result<Value, JsonError> {
    let mut p = Parser { b: text.as_bytes(), s: text, i: 0, depth: 0, max: max_nesting };
    p.ws();
    let v = p.value()?;
    p.ws();
    if p.i != p.b.len() {
        return Err(p.err());
    }
    Ok(v)
}

struct Parser<'a> {
    b: &'a [u8],
    s: &'a str,
    i: usize,
    depth: usize,
    max: usize,
}

impl<'a> Parser<'a> {
    fn err(&self) -> JsonError {
        JsonError { at: self.i }
    }

    fn ws(&mut self) {
        while let Some(b' ' | b'\t' | b'\n' | b'\r') = self.b.get(self.i) {
            self.i += 1;
        }
    }

    fn lit(&mut self, word: &[u8], v: Value) -> Result<Value, JsonError> {
        if self.b[self.i..].starts_with(word) {
            self.i += word.len();
            Ok(v)
        } else {
            Err(self.err())
        }
    }

    fn value(&mut self) -> Result<Value, JsonError> {
        match self.b.get(self.i) {
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => Ok(Value::String(self.string()?)),
            Some(b't') => self.lit(b"true", Value::Bool(true)),
            Some(b'f') => self.lit(b"false", Value::Bool(false)),
            Some(b'n') => self.lit(b"null", Value::Null),
            Some(b'-' | b'0'..=b'9') => self.number(),
            _ => Err(self.err()),
        }
    }

    fn enter(&mut self) -> Result<(), JsonError> {
        self.depth += 1;
        if self.depth > self.max {
            return Err(self.err());
        }
        Ok(())
    }

    fn object(&mut self) -> Result<Value, JsonError> {
        self.enter()?;
        self.i += 1; // {
        let mut m = Map::new();
        self.ws();
        if self.b.get(self.i) == Some(&b'}') {
            self.i += 1;
            self.depth -= 1;
            return Ok(Value::Object(m));
        }
        loop {
            self.ws();
            if self.b.get(self.i) != Some(&b'"') {
                return Err(self.err());
            }
            let k = self.string()?;
            self.ws();
            if self.b.get(self.i) != Some(&b':') {
                return Err(self.err());
            }
            self.i += 1;
            self.ws();
            let v = self.value()?;
            m.insert(k, v); // last one wins
            self.ws();
            match self.b.get(self.i) {
                Some(b',') => self.i += 1,
                Some(b'}') => {
                    self.i += 1;
                    self.depth -= 1;
                    return Ok(Value::Object(m));
                }
                _ => return Err(self.err()),
            }
        }
    }

    fn array(&mut self) -> Result<Value, JsonError> {
        self.enter()?;
        self.i += 1; // [
        let mut a = Vec::new();
        self.ws();
        if self.b.get(self.i) == Some(&b']') {
            self.i += 1;
            self.depth -= 1;
            return Ok(Value::Array(a));
        }
        loop {
            self.ws();
            a.push(self.value()?);
            self.ws();
            match self.b.get(self.i) {
                Some(b',') => self.i += 1,
                Some(b']') => {
                    self.i += 1;
                    self.depth -= 1;
                    return Ok(Value::Array(a));
                }
                _ => return Err(self.err()),
            }
        }
    }

    fn hex4(&mut self) -> Result<u32, JsonError> {
        let h = self.b.get(self.i..self.i + 4).ok_or_else(|| self.err())?;
        let mut v = 0u32;
        for &c in h {
            let d = (c as char).to_digit(16).ok_or_else(|| self.err())?;
            v = v * 16 + d;
        }
        self.i += 4;
        Ok(v)
    }

    fn string(&mut self) -> Result<String, JsonError> {
        self.i += 1; // opening quote
        let mut out = String::new();
        let mut run = self.i; // start of the pending unescaped run
        loop {
            let Some(&c) = self.b.get(self.i) else { return Err(self.err()) };
            match c {
                b'"' => {
                    out.push_str(&self.s[run..self.i]);
                    self.i += 1;
                    return Ok(out);
                }
                b'\\' => {
                    out.push_str(&self.s[run..self.i]);
                    self.i += 1;
                    let Some(&e) = self.b.get(self.i) else { return Err(self.err()) };
                    self.i += 1;
                    match e {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let u = self.hex4()?;
                            let cp = if (0xd800..0xdc00).contains(&u) {
                                // a high surrogate must be followed by \u + a low surrogate
                                if self.b.get(self.i..self.i + 2) != Some(b"\\u") {
                                    return Err(self.err());
                                }
                                self.i += 2;
                                let lo = self.hex4()?;
                                if !(0xdc00..0xe000).contains(&lo) {
                                    return Err(self.err());
                                }
                                0x10000 + ((u - 0xd800) << 10) + (lo - 0xdc00)
                            } else if (0xdc00..0xe000).contains(&u) {
                                return Err(self.err());
                            } else {
                                u
                            };
                            out.push(char::from_u32(cp).ok_or_else(|| self.err())?);
                        }
                        _ => return Err(self.err()),
                    }
                    run = self.i;
                }
                0x00..=0x1f => return Err(self.err()),
                _ => self.i += 1, // ASCII or any byte of a multi-byte character: copied with the run
            }
        }
    }

    fn digits(&mut self) -> usize {
        let start = self.i;
        while let Some(b'0'..=b'9') = self.b.get(self.i) {
            self.i += 1;
        }
        self.i - start
    }

    fn number(&mut self) -> Result<Value, JsonError> {
        let start = self.i;
        if self.b.get(self.i) == Some(&b'-') {
            self.i += 1;
        }
        match self.b.get(self.i) {
            Some(b'0') => self.i += 1,
            Some(b'1'..=b'9') => {
                self.digits();
            }
            _ => return Err(self.err()),
        }
        if self.b.get(self.i) == Some(&b'.') {
            self.i += 1;
            if self.digits() == 0 {
                return Err(self.err());
            }
        }
        if let Some(b'e' | b'E') = self.b.get(self.i) {
            self.i += 1;
            if let Some(b'+' | b'-') = self.b.get(self.i) {
                self.i += 1;
            }
            if self.digits() == 0 {
                return Err(self.err());
            }
        }
        let x: f64 = self.s[start..self.i].parse().map_err(|_| JsonError { at: start })?;
        Ok(number_value(x))
    }
}

/// Arrays/objects nested at most `max` deep (the root container is level 1) — what [`parse_json`] accepts, checked
/// on a value built some other way. Iterative.
pub fn within_nesting(v: &Value, max: usize) -> bool {
    let mut stack: Vec<(&Value, usize)> = vec![(v, 1)];
    while let Some((x, depth)) = stack.pop() {
        let kids: Box<dyn Iterator<Item = &Value>> = match x {
            Value::Array(a) => Box::new(a.iter()),
            Value::Object(o) => Box::new(o.values()),
            _ => continue,
        };
        if depth > max {
            return false;
        }
        stack.extend(kids.map(|k| (k, depth + 1)));
    }
    true
}

/// The JSON value of a double as JavaScript holds it: ±Infinity → null (what JSON.stringify prints), integral values
/// in the i64 range → an integer Number (the same double), everything else → an f64 Number.
pub fn number_value(x: f64) -> Value {
    if !x.is_finite() {
        return Value::Null;
    }
    if x == x.trunc() && x.abs() < 9.2e18 {
        return Value::Number(Number::from(x as i64));
    }
    Number::from_f64(x).map(Value::Number).unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_what_json_parse_accepts() {
        let v = parse_json(" {\"a\": [1, -0, 1.0, 1e2, 2.5E-1, \"\\u00e9\\ud83c\\udfa8\\n\"], \"a\": true, \"__proto__\": null} ").unwrap();
        assert_eq!(v, serde_json::json!({"a": true, "__proto__": null}));
        let v = parse_json("[1, -0, 1.0, 1e2, 2.5E-1, \"\\u00e9\\ud83c\\udfa8\\n\\/\"]").unwrap();
        assert_eq!(v, serde_json::json!([1, 0, 1, 100, 0.25, "é🎨\n/"]));
        assert_eq!(parse_json("1e400").unwrap(), Value::Null);
        assert_eq!(parse_json("-1e400").unwrap(), Value::Null);
        assert_eq!(parse_json("1e-400").unwrap(), serde_json::json!(0));
        assert_eq!(parse_json("\"\"").unwrap(), serde_json::json!(""));
        // correctly rounded (serde_json's default parser is not for every input)
        let text = "0.00030352698354883749";
        assert_eq!(parse_json(text).unwrap().as_f64(), Some(text.parse::<f64>().unwrap()));
    }

    #[test]
    fn refuses_what_json_parse_refuses() {
        for bad in ["", " ", "{", "[1,]", "{\"a\":1,}", "01", "1.", ".5", "+1", "-", "1e", "tru", "nul", "'a'", "\"a", "\"\t\"",
            "\"\\x\"", "\"\\u12\"", "[1] 2", "{a:1}", "NaN", "Infinity", "[", "{\"a\" 1}"]
        {
            assert!(parse_json(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn lone_surrogates_and_deep_nesting_are_refused() {
        // lone surrogates (a Rust String cannot hold one; the reference refuses them too) — ENGINE.md §9, Q3
        for bad in ["\"\\ud800\"", "\"\\udc00\"", "\"\\ud800\\u0041\"", "\"\\ud800x\""] {
            assert!(parse_json(bad).is_err(), "{bad:?}");
        }
        // nesting limit (Q2): the root container is level 1
        let deep = |n: usize| "[".repeat(n) + &"]".repeat(n);
        assert!(parse_json(&deep(MAX_NESTING)).is_ok());
        assert!(parse_json(&deep(MAX_NESTING + 1)).is_err());
        let v = parse_json(&deep(MAX_NESTING)).unwrap();
        assert!(within_nesting(&v, MAX_NESTING) && !within_nesting(&v, MAX_NESTING - 1));
        assert!(within_nesting(&Value::Null, 0) && within_nesting(&serde_json::json!({"a": [1]}), 2));
        assert!(!within_nesting(&serde_json::json!({"a": [1]}), 1));
    }
}
