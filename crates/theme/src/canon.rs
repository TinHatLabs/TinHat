//! canon.rs — port of `canon.ts`: the byte-stable JSON forms of the theme contract files (test vectors, the
//! library, share payloads), printed exactly like the reference's `JSON.stringify`.
//!
//! - object keys in code-point order at every level (= UTF-8 byte order = Rust `str` Ord);
//! - strings escaped like `JSON.stringify` (`\"` `\\` `\b` `\f` `\n` `\r` `\t`, other controls `\u00xx`, the rest
//!   as UTF-8, U+2028/U+2029 unescaped);
//! - numbers printed like ECMAScript `Number::toString` (JSON numbers are doubles: `1.0` prints `1`, `1e21` prints
//!   `1e+21`), never with Rust's float printer.
//!
//! [`pretty_json`] is the test-vector file format: 2-space indent, `": "`, one element per line, `[]` / `{}` when
//! empty, final newline.

use serde_json::{Map, Value};
use std::fmt::Write as _;

/// Keys of an object in code-point order (independent of serde_json's map implementation).
pub fn sorted_keys(o: &Map<String, Value>) -> Vec<&String> {
    let mut keys: Vec<&String> = o.keys().collect();
    keys.sort();
    keys
}

/// ECMAScript `Number::toString(10)` of a double — what `JSON.stringify` prints for a finite number.
/// Non-finite numbers print `null` in JSON (serde_json cannot hold them; see `push_number`).
pub fn js_number(x: f64) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x == 0.0 {
        return "0".into(); // +0 and -0
    }
    if x.is_infinite() {
        return if x > 0.0 { "Infinity".into() } else { "-Infinity".into() };
    }
    if x < 0.0 {
        return format!("-{}", js_number(-x));
    }
    // Rust's `{:e}` prints the shortest digit string that round-trips (closest to x), like ECMAScript's
    // Number::toString: "d[.ddd]e<exp>". k = number of digits, n = exponent + 1 (x = 0.d1d2… × 10^n).
    let e = format!("{:e}", x);
    let (mant, exp) = e.split_once('e').expect("LowerExp prints an exponent");
    let exp: i32 = exp.parse().expect("LowerExp exponent");
    let digits: String = mant.chars().filter(|c| *c != '.').collect();
    let k = digits.len() as i32;
    let n = exp + 1;
    if k <= n && n <= 21 {
        let mut s = digits;
        for _ in 0..(n - k) {
            s.push('0');
        }
        s
    } else if 0 < n && n <= 21 {
        format!("{}.{}", &digits[..n as usize], &digits[n as usize..])
    } else if -6 < n && n <= 0 {
        let mut s = String::from("0.");
        for _ in 0..(-n) {
            s.push('0');
        }
        s.push_str(&digits);
        s
    } else {
        let e1 = n - 1;
        let sign = if e1 >= 0 { '+' } else { '-' };
        if k == 1 {
            format!("{}e{}{}", digits, sign, e1.abs())
        } else {
            format!("{}.{}e{}{}", &digits[..1], &digits[1..], sign, e1.abs())
        }
    }
}

/// `JSON.stringify(s)` for a string, appended to `out`.
pub fn push_js_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// A JSON number as ECMAScript prints it (the value is the double JavaScript would hold).
fn push_number(out: &mut String, n: &serde_json::Number) {
    match n.as_f64() {
        Some(x) if x.is_finite() => out.push_str(&js_number(x)),
        _ => out.push_str("null"),
    }
}

fn push_scalar(out: &mut String, v: &Value) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => push_number(out, n),
        Value::String(s) => push_js_string(out, s),
        Value::Array(_) | Value::Object(_) => unreachable!("containers are printed by the callers"),
    }
}

/// Compact JSON with object keys sorted at every level (`canonJson`). Iterative: never recurses, so an
/// arbitrarily deep value cannot overflow the stack.
pub fn canon_json(v: &Value) -> String {
    enum Step<'a> {
        Value(&'a Value),
        Raw(&'static str),
        Key(&'a str),
    }
    let mut out = String::new();
    let mut stack: Vec<Step> = vec![Step::Value(v)];
    while let Some(step) = stack.pop() {
        match step {
            Step::Raw(s) => out.push_str(s),
            Step::Key(k) => {
                push_js_string(&mut out, k);
                out.push(':');
            }
            Step::Value(Value::Array(a)) => {
                out.push('[');
                stack.push(Step::Raw("]"));
                for (i, x) in a.iter().enumerate().rev() {
                    stack.push(Step::Value(x));
                    if i > 0 {
                        stack.push(Step::Raw(","));
                    }
                }
            }
            Step::Value(Value::Object(o)) => {
                out.push('{');
                stack.push(Step::Raw("}"));
                let keys = sorted_keys(o);
                for (i, k) in keys.iter().enumerate().rev() {
                    stack.push(Step::Value(&o[k.as_str()]));
                    stack.push(Step::Key(k));
                    if i > 0 {
                        stack.push(Step::Raw(","));
                    }
                }
            }
            Step::Value(scalar) => push_scalar(&mut out, scalar),
        }
    }
    out
}

/// UTF-8 byte length of `JSON.stringify(v)` (key order does not change the length).
pub fn json_len(v: &Value) -> usize {
    canon_json(v).len()
}

/// Pretty JSON, keys sorted at every level, final newline (`prettyJson`): the test-vector file format.
pub fn pretty_json(v: &Value) -> String {
    let mut out = String::new();
    pretty_at(&mut out, v, "");
    out.push('\n');
    out
}

fn pretty_at(out: &mut String, v: &Value, ind: &str) {
    let inner = format!("{ind}  ");
    match v {
        Value::Array(a) => {
            if a.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push_str("[\n");
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push_str(",\n");
                }
                out.push_str(&inner);
                pretty_at(out, x, &inner);
            }
            out.push('\n');
            out.push_str(ind);
            out.push(']');
        }
        Value::Object(o) => {
            let keys = sorted_keys(o);
            if keys.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push_str("{\n");
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    out.push_str(",\n");
                }
                out.push_str(&inner);
                push_js_string(out, k);
                out.push_str(": ");
                pretty_at(out, &o[k.as_str()], &inner);
            }
            out.push('\n');
            out.push_str(ind);
            out.push('}');
        }
        scalar => push_scalar(out, scalar),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_print_like_ecmascript() {
        let cases: &[(f64, &str)] = &[
            (0.0, "0"),
            (-0.0, "0"),
            (1.0, "1"),
            (-1.5, "-1.5"),
            (0.1, "0.1"),
            (100.0, "100"),
            (123.456, "123.456"),
            (1e21, "1e+21"),
            (1e20, "100000000000000000000"),
            (123456789012345680000.0, "123456789012345680000"),
            (1.5e21, "1.5e+21"),
            (1e-6, "0.000001"),
            (1e-7, "1e-7"),
            (1.25e-7, "1.25e-7"),
            (0.000123, "0.000123"),
            (5e-324, "5e-324"),
            (1.7976931348623157e308, "1.7976931348623157e+308"),
            (0.1 + 0.2, "0.30000000000000004"),
            (9007199254740993.0, "9007199254740992"),
            (12345678901234567890.0, "12345678901234567000"),
        ];
        for &(x, want) in cases {
            assert_eq!(js_number(x), want, "{x:e}");
        }
    }

    #[test]
    fn strings_escape_like_json_stringify() {
        let mut s = String::new();
        push_js_string(&mut s, "a\"b\\c\u{8}\u{c}\n\r\t\u{1}\u{1f}\u{7f}é🎨\u{2028}/");
        assert_eq!(s, "\"a\\\"b\\\\c\\b\\f\\n\\r\\t\\u0001\\u001f\u{7f}é🎨\u{2028}/\"");
    }

    #[test]
    fn canonical_forms() {
        let v: Value = serde_json::json!({"b": [1, 2.5, {"z": null, "a": true}], "a": "x", "é": {}, "e": []});
        assert_eq!(canon_json(&v), r#"{"a":"x","b":[1,2.5,{"a":true,"z":null}],"e":[],"é":{}}"#);
        assert_eq!(
            pretty_json(&v),
            "{\n  \"a\": \"x\",\n  \"b\": [\n    1,\n    2.5,\n    {\n      \"a\": true,\n      \"z\": null\n    }\n  ],\n  \"e\": [],\n  \"é\": {}\n}\n"
        );
        assert_eq!(json_len(&v), canon_json(&v).len());
    }
}
