//! json.rs — port of `json.ts`: JSON data for the layout engine (LAYOUT.md §1).
//!
//! The TypeScript reference edits forms as shared, mutable JavaScript objects: nodes are found by identity
//! (`indexOf(node)`, `ancestors.includes(parent)`), detached and inserted in place, and one object can be reachable from
//! two places for a moment (a wrapper that takes the target's size object). [`J`] reproduces exactly those semantics:
//! objects and arrays are reference-counted cells — `J::clone()` is a second reference (a JavaScript assignment),
//! [`J::deep`] is the reference's `clone()` (a deep copy) — so every function ports line for line.
//!
//! - Objects are key-sorted maps (code-point order = UTF-8 byte order = `str` order). Nothing in the engine depends on
//!   insertion order: every ordered walk is explicit (region order, code-point order, tree order) and the canonical
//!   form sorts keys. Keys are own keys only (`__proto__` is an ordinary key, as in the reference's prototype-less
//!   objects).
//! - Numbers are doubles, as in JavaScript. An **integer** is a finite double with no fraction (`Number.isInteger`).
//!   Non-finite numbers never occur: the reader turns an overflowing number into `null` (tinhat-theme `json.rs`).
//! - Reading = tinhat-theme's `parse_json` (strict JSON, nesting ≤ 128, no lone surrogate from a `\u` escape, the last
//!   duplicate key wins); printing = its ECMAScript number and `JSON.stringify` string printers (`canon.rs`).

use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::rc::Rc;

use serde_json::Value;
use tinhat_theme::canon::{js_number, push_js_string};

/// An object's entries.
pub type Map = BTreeMap<String, J>;

/// A JavaScript value holding JSON data (see the module doc). `Option<J>::None` plays `undefined`.
#[derive(Clone, Debug)]
pub enum J {
    Null,
    Bool(bool),
    Num(f64),
    Str(Rc<str>),
    Arr(Rc<RefCell<Vec<J>>>),
    Obj(Rc<RefCell<Map>>),
}

impl Default for J {
    fn default() -> J {
        J::Null
    }
}

impl J {
    /// A new empty object (`obj()`).
    pub fn obj() -> J {
        J::Obj(Rc::new(RefCell::new(Map::new())))
    }
    /// A new array holding `v`.
    pub fn arr(v: Vec<J>) -> J {
        J::Arr(Rc::new(RefCell::new(v)))
    }
    pub fn s(s: &str) -> J {
        J::Str(Rc::from(s))
    }
    pub fn int(x: i64) -> J {
        J::Num(x as f64)
    }
    pub fn strs<S: AsRef<str>>(v: &[S]) -> J {
        J::arr(v.iter().map(|s| J::s(s.as_ref())).collect())
    }

    pub fn is_obj(&self) -> bool {
        matches!(self, J::Obj(_))
    }
    pub fn is_arr(&self) -> bool {
        matches!(self, J::Arr(_))
    }
    pub fn is_str(&self) -> bool {
        matches!(self, J::Str(_))
    }
    pub fn is_num(&self) -> bool {
        matches!(self, J::Num(_))
    }
    pub fn is_bool(&self) -> bool {
        matches!(self, J::Bool(_))
    }
    pub fn is_null(&self) -> bool {
        matches!(self, J::Null)
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            J::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_rc(&self) -> Option<Rc<str>> {
        match self {
            J::Str(s) => Some(s.clone()),
            _ => None,
        }
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            J::Num(x) => Some(*x),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            J::Bool(b) => Some(*b),
            _ => None,
        }
    }
    /// `Number.isInteger(v)`: a JSON number whose value is integral (`1.0`, `1e2`, `-0` are; `1.5` is not).
    pub fn is_int(&self) -> bool {
        matches!(self, J::Num(x) if x.is_finite() && x.fract() == 0.0)
    }
    /// The integer value (saturating at the i64 range, which no clamped input reaches); `None` for a non-integer.
    pub fn int_val(&self) -> Option<i64> {
        match self {
            J::Num(x) if x.is_finite() && x.fract() == 0.0 => Some(*x as i64),
            _ => None,
        }
    }
    /// `=== true`
    pub fn is_true(&self) -> bool {
        matches!(self, J::Bool(true))
    }
    /// The value is exactly the string `s` (`v === "s"`).
    pub fn eq_str(&self, s: &str) -> bool {
        matches!(self, J::Str(x) if &**x == s)
    }

    // ---- objects (own keys only) ----
    /// `get(o, k)`: an own property of an object (`None` for a missing key or a non-object).
    pub fn get(&self, k: &str) -> Option<J> {
        match self {
            J::Obj(o) => o.borrow().get(k).cloned(),
            _ => None,
        }
    }
    /// `has(o, k)`
    pub fn has(&self, k: &str) -> bool {
        match self {
            J::Obj(o) => o.borrow().contains_key(k),
            _ => false,
        }
    }
    /// `o[k] = v` (objects only).
    pub fn set(&self, k: &str, v: J) {
        if let J::Obj(o) = self {
            o.borrow_mut().insert(k.to_string(), v);
        }
    }
    /// `delete o[k]`
    pub fn del(&self, k: &str) {
        if let J::Obj(o) = self {
            o.borrow_mut().remove(k);
        }
    }
    /// The object's keys in code-point order (empty for anything else).
    pub fn keys(&self) -> Vec<String> {
        match self {
            J::Obj(o) => o.borrow().keys().cloned().collect(),
            _ => Vec::new(),
        }
    }
    /// `Object.keys(o).length`
    pub fn nkeys(&self) -> usize {
        match self {
            J::Obj(o) => o.borrow().len(),
            _ => 0,
        }
    }
    /// The key `k` holds exactly the string `s`.
    pub fn is(&self, k: &str, s: &str) -> bool {
        match self {
            J::Obj(o) => matches!(o.borrow().get(k), Some(J::Str(x)) if &**x == s),
            _ => false,
        }
    }
    /// The string held by key `k`, if it holds one.
    pub fn get_str(&self, k: &str) -> Option<Rc<str>> {
        match self {
            J::Obj(o) => match o.borrow().get(k) {
                Some(J::Str(s)) => Some(s.clone()),
                _ => None,
            },
            _ => None,
        }
    }
    /// Replace every entry of this object by `other`'s (the same values, not copies).
    pub fn replace_entries(&self, other: &J) {
        if let (J::Obj(a), J::Obj(b)) = (self, other) {
            if Rc::ptr_eq(a, b) {
                return;
            }
            let m = b.borrow().clone();
            *a.borrow_mut() = m;
        }
    }

    // ---- arrays ----
    /// The elements of an array (references), empty for anything else.
    pub fn items(&self) -> Vec<J> {
        match self {
            J::Arr(a) => a.borrow().clone(),
            _ => Vec::new(),
        }
    }
    /// An array's length (0 for anything else).
    pub fn len(&self) -> usize {
        match self {
            J::Arr(a) => a.borrow().len(),
            _ => 0,
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn at(&self, i: usize) -> Option<J> {
        match self {
            J::Arr(a) => a.borrow().get(i).cloned(),
            _ => None,
        }
    }
    pub fn push(&self, v: J) {
        if let J::Arr(a) = self {
            a.borrow_mut().push(v);
        }
    }
    pub fn unshift(&self, v: J) {
        if let J::Arr(a) = self {
            a.borrow_mut().insert(0, v);
        }
    }
    /// `splice(i, 0, v)` (an index past the end appends).
    pub fn insert_at(&self, i: usize, v: J) {
        if let J::Arr(a) = self {
            let mut a = a.borrow_mut();
            let i = i.min(a.len());
            a.insert(i, v);
        }
    }
    /// `splice(i, 0, ...vs)`
    pub fn insert_all(&self, i: usize, vs: Vec<J>) {
        if let J::Arr(a) = self {
            let mut a = a.borrow_mut();
            let i = i.min(a.len());
            let tail = a.split_off(i);
            a.extend(vs);
            a.extend(tail);
        }
    }
    /// `splice(i, 1)[0]`
    pub fn remove_at(&self, i: usize) -> Option<J> {
        match self {
            J::Arr(a) => {
                let mut a = a.borrow_mut();
                (i < a.len()).then(|| a.remove(i))
            }
            _ => None,
        }
    }
    /// `arr[i] = v` for an existing index.
    pub fn set_at(&self, i: usize, v: J) {
        if let J::Arr(a) = self {
            if let Some(slot) = a.borrow_mut().get_mut(i) {
                *slot = v;
            }
        }
    }
    /// `arr.length = n` (shrinking only).
    pub fn truncate(&self, n: usize) {
        if let J::Arr(a) = self {
            a.borrow_mut().truncate(n);
        }
    }
    /// `indexOf(x)` by identity (the reference only ever searches for objects).
    pub fn index_of(&self, x: &J) -> Option<usize> {
        match self {
            J::Arr(a) => a.borrow().iter().position(|y| y.same(x)),
            _ => None,
        }
    }

    /// Identity (`===` on objects / arrays); primitives are never "the same object".
    pub fn same(&self, other: &J) -> bool {
        match (self, other) {
            (J::Obj(a), J::Obj(b)) => Rc::ptr_eq(a, b),
            (J::Arr(a), J::Arr(b)) => Rc::ptr_eq(a, b),
            _ => false,
        }
    }

    /// The reference's `clone()`: a deep copy (a non-finite number becomes null).
    pub fn deep(&self) -> J {
        match self {
            J::Arr(a) => J::arr(a.borrow().iter().map(J::deep).collect()),
            J::Obj(o) => J::Obj(Rc::new(RefCell::new(o.borrow().iter().map(|(k, v)| (k.clone(), v.deep())).collect()))),
            J::Num(x) if !x.is_finite() => J::Null,
            other => other.clone(),
        }
    }

    // ---- conversions ----
    /// A serde_json value (from the theme reader) as a fresh [`J`].
    pub fn from_value(v: &Value) -> J {
        match v {
            Value::Null => J::Null,
            Value::Bool(b) => J::Bool(*b),
            Value::Number(n) => n.as_f64().map(J::Num).unwrap_or(J::Null),
            Value::String(s) => J::s(s),
            Value::Array(a) => J::arr(a.iter().map(J::from_value).collect()),
            Value::Object(o) => J::Obj(Rc::new(RefCell::new(o.iter().map(|(k, v)| (k.clone(), J::from_value(v))).collect()))),
        }
    }
    /// This value as serde_json data (numbers as the reader stores them).
    pub fn to_value(&self) -> Value {
        match self {
            J::Null => Value::Null,
            J::Bool(b) => Value::Bool(*b),
            J::Num(x) => tinhat_theme::json::number_value(*x),
            J::Str(s) => Value::String(s.to_string()),
            J::Arr(a) => Value::Array(a.borrow().iter().map(J::to_value).collect()),
            J::Obj(o) => Value::Object(o.borrow().iter().map(|(k, v)| (k.clone(), v.to_value())).collect()),
        }
    }

    /// The canonical compact form (`canonJson`): keys in code-point order, ECMAScript numbers, no whitespace.
    pub fn canon(&self) -> String {
        let mut s = String::new();
        self.write_canon(&mut s);
        s
    }
    fn write_canon(&self, out: &mut String) {
        match self {
            J::Arr(a) => {
                out.push('[');
                for (i, x) in a.borrow().iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    x.write_canon(out);
                }
                out.push(']');
            }
            J::Obj(o) => {
                out.push('{');
                for (i, (k, v)) in o.borrow().iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    push_js_string(out, k);
                    out.push(':');
                    v.write_canon(out);
                }
                out.push('}');
            }
            scalar => write_scalar(scalar, out),
        }
    }
    /// The canonical pretty form + final newline (`prettyJson`): the test-vector file format.
    pub fn pretty(&self) -> String {
        let mut s = String::new();
        self.write_pretty(&mut s, "");
        s.push('\n');
        s
    }
    fn write_pretty(&self, out: &mut String, ind: &str) {
        let inner = format!("{ind}  ");
        match self {
            J::Arr(a) => {
                let a = a.borrow();
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
                    x.write_pretty(out, &inner);
                }
                out.push('\n');
                out.push_str(ind);
                out.push(']');
            }
            J::Obj(o) => {
                let o = o.borrow();
                if o.is_empty() {
                    out.push_str("{}");
                    return;
                }
                out.push_str("{\n");
                for (i, (k, v)) in o.iter().enumerate() {
                    if i > 0 {
                        out.push_str(",\n");
                    }
                    out.push_str(&inner);
                    push_js_string(out, k);
                    out.push_str(": ");
                    v.write_pretty(out, &inner);
                }
                out.push('\n');
                out.push_str(ind);
                out.push('}');
            }
            scalar => write_scalar(scalar, out),
        }
    }
}

fn write_scalar(v: &J, out: &mut String) {
    match v {
        J::Null => out.push_str("null"),
        J::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        J::Num(x) => push_num(out, *x),
        J::Str(s) => push_js_string(out, s),
        J::Arr(_) | J::Obj(_) => unreachable!("containers are printed by the callers"),
    }
}

/// `JSON.stringify(x)` for a number: ECMAScript `Number::toString`, `null` when not finite.
fn push_num(out: &mut String, x: f64) {
    if !x.is_finite() {
        out.push_str("null");
    } else if x.fract() == 0.0 && x.abs() < 1e15 {
        // integers below 10^15 print as plain digits (-0 as 0), exactly what js_number gives, without its formatting
        let n = x as i64;
        out.push_str(&n.to_string());
    } else {
        out.push_str(&js_number(x));
    }
}

/// Build an object from `(key, value)` pairs (`mk`).
pub fn mk(pairs: Vec<(&str, J)>) -> J {
    let o = J::obj();
    for (k, v) in pairs {
        o.set(k, v);
    }
    o
}

/// `readJson(text)`: JSON text → data with the colour contract's reader (LAYOUT.md §1.1); `None` = not JSON data.
pub fn read_json(text: &str) -> Option<J> {
    tinhat_theme::json::parse_json(text).ok().map(|v| J::from_value(&v))
}

/// LAYOUT.md §1.4: the most values a JSON-value check visits (a value reached twice counts twice).
pub const MAX_VALUES: usize = 1 << 20;

/// LAYOUT.md §1.4 — the input domain (`isJsonValue`): a value JSON text could have produced — finite numbers, arrays and
/// objects nested at most 128 deep (the root is level 1), at most 2^20 values visited. `None` (the reference's
/// `undefined`) is not JSON data. A Rust value cannot hold a lone surrogate, a hole, a function or a class instance, so
/// what is left to check is the numbers, the depth and the visit budget — which is what makes a JSON tree of more than
/// 2^20 values "not JSON data" here exactly as in the reference. Iterative (the reference's visit order), never panics.
pub fn is_json_value(v: Option<&J>) -> bool {
    let Some(v) = v else { return false };
    let max_depth = tinhat_theme::json::MAX_NESTING;
    let mut stack: Vec<(J, usize)> = vec![(v.clone(), 1)];
    let mut visits = 0usize;
    while let Some((x, depth)) = stack.pop() {
        visits += 1;
        if visits > MAX_VALUES {
            return false;
        }
        match &x {
            J::Num(n) => {
                if !n.is_finite() {
                    return false;
                }
            }
            J::Arr(a) => {
                if depth > max_depth {
                    return false;
                }
                let a = a.borrow();
                if visits + stack.len() + a.len() > MAX_VALUES {
                    return false;
                }
                stack.extend(a.iter().map(|y| (y.clone(), depth + 1)));
            }
            J::Obj(o) => {
                if depth > max_depth {
                    return false;
                }
                let o = o.borrow();
                if visits + stack.len() + o.len() > MAX_VALUES {
                    return false;
                }
                stack.extend(o.values().map(|y| (y.clone(), depth + 1)));
            }
            _ => {}
        }
    }
    true
}

/// `canon(v)` of an optional value (`undefined` prints as the empty string, like the reference's helper).
pub fn canon_opt(v: Option<&J>) -> String {
    v.map(J::canon).unwrap_or_default()
}

/// UTF-8 bytes of the canonical form.
pub fn canon_bytes(v: &J) -> usize {
    v.canon().len()
}

/// Code-point order of two strings (= Rust `str` order).
pub fn cmp_cp(a: &str, b: &str) -> Ordering {
    a.cmp(b)
}

/// Floor division with a positive divisor (LAYOUT.md §7.2; the reference's `fdiv`, which throws for b ≤ 0 — no caller
/// ever passes one; the port answers 0 instead of panicking).
pub fn fdiv(a: i64, b: i64) -> i64 {
    if b <= 0 {
        debug_assert!(b > 0, "layout: fdiv by {b}");
        return 0;
    }
    a.div_euclid(b)
}

/// `clampInt` on doubles (the reference clamps JSON numbers before converting nothing).
pub fn clamp_f(v: f64, lo: f64, hi: f64) -> f64 {
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

/// `clamp` on integers (`v < lo ? lo : v > hi ? hi : v` — `lo` wins when the range is empty).
pub fn clamp_i(v: i64, lo: i64, hi: i64) -> i64 {
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

/// Unicode code points (`Array.from(s).length`).
pub fn cp_len(s: &str) -> usize {
    s.chars().count()
}

/// UTF-16 code units (`s.length`).
pub fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

/// What a JavaScript template literal prints for a value (`${v}`): `undefined`, `null`, booleans, ECMAScript
/// numbers, strings as they are, arrays joined by `,` (null / undefined elements empty), `[object Object]`.
pub fn js_str(v: Option<&J>) -> String {
    match v {
        None => "undefined".into(),
        Some(J::Null) => "null".into(),
        Some(J::Bool(b)) => b.to_string(),
        Some(J::Num(x)) => {
            if x.is_nan() {
                "NaN".into()
            } else {
                js_number(*x)
            }
        }
        Some(J::Str(s)) => s.to_string(),
        Some(J::Arr(a)) => a
            .borrow()
            .iter()
            .map(|x| if matches!(x, J::Null) { String::new() } else { js_str(Some(x)) })
            .collect::<Vec<_>>()
            .join(","),
        Some(J::Obj(_)) => "[object Object]".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_and_prints_like_the_reference() {
        let v = read_json(r#"{"__proto__": {"a": 1}, "constructor": 2, "a": 1, "a": 3, "big": 1e400, "neg": -1e400, "z": -0}"#)
            .unwrap();
        assert_eq!(v.canon(), r#"{"__proto__":{"a":1},"a":3,"big":null,"constructor":2,"neg":null,"z":0}"#);
        assert!(read_json(&("[".repeat(128) + &"]".repeat(128))).is_some());
        assert!(read_json(&("[".repeat(129) + &"]".repeat(129))).is_none());
        assert!(read_json(r#""a\ud800b""#).is_none());
        assert!(read_json(r#""\udc00""#).is_none());
        for bad in ["", "{", "[1,]", "01", "NaN", "{a:1}", "'a'", "1 2"] {
            assert!(read_json(bad).is_none(), "{bad}");
        }
        let n = read_json("[1.5, 1e21, 100, -3, 0.1]").unwrap();
        assert_eq!(n.canon(), "[1.5,1e+21,100,-3,0.1]");
        assert_eq!(n.pretty(), "[\n  1.5,\n  1e+21,\n  100,\n  -3,\n  0.1\n]\n");
        assert_eq!(J::obj().pretty(), "{}\n");
    }

    /// §1.4 the input domain, at its edges — every expectation checked against the TypeScript reference (web 90bfebbb,
    /// `isJsonValue` / `repairDesktopText` / `repairPhoneText` run in node on the same values).
    #[test]
    fn input_domain_like_the_reference() {
        let zeros = |n: usize| J::arr(vec![J::int(0); n]);
        assert!(!is_json_value(None), "undefined is not JSON data");
        assert!(is_json_value(Some(&J::Null)));
        assert!(is_json_value(Some(&zeros(MAX_VALUES - 1))), "2^20 values in all");
        assert!(!is_json_value(Some(&zeros(MAX_VALUES))), "2^20 + 1 values");
        let nest = |n: usize, arr: bool| {
            let mut d = if arr { J::int(1) } else { J::s("x") };
            for _ in 0..n {
                d = if arr { J::arr(vec![d]) } else { mk(vec![("k", d)]) };
            }
            d
        };
        for arr in [true, false] {
            assert!(is_json_value(Some(&nest(128, arr))), "128 containers deep");
            assert!(!is_json_value(Some(&nest(129, arr))), "129 containers deep");
        }
        assert!(!is_json_value(Some(&J::Num(f64::NAN))) && !is_json_value(Some(&J::Num(f64::INFINITY))));
        let notes = |r: crate::repair::RepairResult| r.notes.join(",");
        let body = r#"{"root":{"t":"row","id":"body","c":[]},"x":["#;
        let text = |n: usize| format!("{body}{}]}}", vec!["0"; n].join(","));
        assert_eq!(notes(crate::repair::repair_desktop_text(&text(MAX_VALUES))), "replaced-by-classic not-json");
        assert_eq!(notes(crate::repair::repair_desktop_text(&text(MAX_VALUES - 10))), "replaced-by-classic too-large");
        let phone = format!(r#"{{"rail":[{}]}}"#, vec!["0"; MAX_VALUES].join(","));
        assert_eq!(notes(crate::repair::repair_phone_text(&phone)), "replaced-by-classic not-json");
        assert_eq!(notes(crate::repair::repair_desktop(None)), "replaced-by-classic not-json");
        assert_eq!(notes(crate::repair::repair_desktop(Some(&J::Null))), "replaced-by-classic not-object");
    }

    #[test]
    fn identity_and_copies() {
        let a = mk(vec![("x", J::int(1))]);
        let arr = J::arr(vec![a.clone(), J::int(2)]);
        assert_eq!(arr.index_of(&a), Some(0));
        let d = arr.deep();
        assert_eq!(d.canon(), arr.canon());
        assert_eq!(d.index_of(&a), None);
        a.set("x", J::int(5));
        assert_eq!(arr.canon(), r#"[{"x":5},2]"#);
        assert_eq!(d.canon(), r#"[{"x":1},2]"#);
    }

    #[test]
    fn arithmetic_helpers() {
        assert_eq!(fdiv(-7, 2), -4);
        assert_eq!(fdiv(7, 2), 3);
        assert_eq!(js_str(None), "undefined");
        assert_eq!(js_str(Some(&J::Num(1.5))), "1.5");
        assert_eq!(js_str(read_json("[1,null,\"a\"]").as_ref()), "1,,a");
        assert_eq!(utf16_len("a🎨"), 3);
        assert_eq!(cp_len("a🎨"), 2);
    }
}
