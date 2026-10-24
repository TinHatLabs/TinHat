//! js.rs — the JSON helpers the reference imports from `layout/json.ts` (`get` / `has` / `isObj` / `isStr` / `isArr` /
//! `isInt` / `obj`) and `theme/canon.ts` (`cmpCodePoint`), over `serde_json` values.
//!
//! The reference reads own keys only (`get` / `has`): a `serde_json` map holds nothing else, so `Map::get` is `get`.
//! Code-point order of two strings (`cmpCodePoint`) is Rust's `str` order (UTF-8 byte order). JSON numbers are doubles:
//! every number is read through `as_f64` and written back with [`num`] (the theme crate's `number_value`: integral
//! values as integers, `-0` as `0` — the same canonical text `JSON.stringify` prints).

use serde_json::{Map, Value};

/// A JSON object (`JObj`): own keys only.
pub type JObj = Map<String, Value>;

/// `obj()`: a new empty object.
pub fn obj() -> JObj {
    Map::new()
}

/// `isObj(v) ? v : undefined`.
pub fn is_obj(v: Option<&Value>) -> Option<&JObj> {
    match v {
        Some(Value::Object(o)) => Some(o),
        _ => None,
    }
}

/// `isStr(v) ? v : undefined`.
pub fn is_str(v: Option<&Value>) -> Option<&str> {
    match v {
        Some(Value::String(s)) => Some(s.as_str()),
        _ => None,
    }
}

/// `isArr(v) ? v : undefined`.
pub fn is_arr(v: Option<&Value>) -> Option<&Vec<Value>> {
    match v {
        Some(Value::Array(a)) => Some(a),
        _ => None,
    }
}

/// `isInt(v)`: a JSON number whose value is integral (1.0 and 1e2 are integers, 1.5 is not; -0 is 0) → the double.
pub fn is_int(v: Option<&Value>) -> Option<f64> {
    match v {
        Some(Value::Number(n)) => n.as_f64().filter(|x| x.is_finite() && x.trunc() == *x),
        _ => None,
    }
}

/// `v === true`.
pub fn is_true(v: Option<&Value>) -> bool {
    matches!(v, Some(Value::Bool(true)))
}

/// `objAt(o, k)`: `o[k]` when `o` is an object and `o[k]` an object.
pub fn obj_at<'a>(o: Option<&'a Value>, k: &str) -> Option<&'a JObj> {
    is_obj(is_obj(o)?.get(k))
}

/// `strAt(o, k)`: `o[k]` when `o` is an object and `o[k]` a string.
pub fn str_at<'a>(o: Option<&'a Value>, k: &str) -> Option<&'a str> {
    is_str(is_obj(o)?.get(k))
}

/// The keys of an object in code-point order (`Object.keys(o).sort(cmpCodePoint)`), never the map's own order.
pub fn sorted_keys(o: &JObj) -> Vec<&String> {
    tinhat_theme::canon::sorted_keys(o)
}

/// A JSON number the way the reference holds it (a double; integral values print without a fraction, -0 as 0).
pub fn num(x: f64) -> Value {
    tinhat_theme::json::number_value(x)
}

/// `a === b` for the values the engine compares: primitives by value (a number is a double: 0 === -0), containers
/// never (the reference compares a snapshot value with a param / subject / literal, which is never the same object).
pub fn strict_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Null, Value::Null) => true,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        (Value::String(x), Value::String(y)) => x == y,
        _ => false,
    }
}

/// JavaScript's property key of a primitive value (`o[v]` / `hasOwnProperty(v)` convert it with ToPropertyKey): a
/// string is itself, a number its ECMAScript text, `true` / `false` / `null` their names. Containers: None (the
/// engine only ever looks up with strings and normalized param values, which are strings or numbers).
pub fn prop_key(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => n.as_f64().map(tinhat_theme::canon::js_number),
        Value::Bool(b) => Some(if *b { "true".into() } else { "false".into() }),
        Value::Null => Some("null".into()),
        Value::Array(_) | Value::Object(_) => None,
    }
}

/// A JSON string value.
pub(crate) fn s(x: &str) -> Value {
    Value::String(x.to_string())
}

/// A string or null.
pub(crate) fn os(x: Option<&str>) -> Value {
    x.map(s).unwrap_or(Value::Null)
}
