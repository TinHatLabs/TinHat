//! The widget registry (contract `widgets.json`, byte-identical in both apps; WIDGETS.md §2) and the widget list a
//! layout holds (LAYOUT.md §3.9 opts.widgets: `{w, size?, settings?}` + `collapsed?`; the local pref until the dock
//! host's synced layout document carries it) — port of the web's `src/widgets/registry.ts` (vectors
//! `testvectors/widgets/registry/*`).

use std::sync::OnceLock;

use serde_json::{Map, Value};

use crate::num;

/// The contract copy (refreshed by sync-vectors.sh; tests/vectors.rs proves it byte-equal to the contract's).
pub const REGISTRY_JSON: &str = include_str!("../widgets.json");

pub const SIZES: [&str; 3] = ["S", "M", "L"];
pub const MAX_WIDGETS: usize = 12;

/// The registry as JSON (parsed once).
pub fn registry() -> &'static Value {
    static R: OnceLock<Value> = OnceLock::new();
    R.get_or_init(|| tinhat_theme::json::parse_json(REGISTRY_JSON).expect("widgets.json is JSON"))
}

/// A widget's definition (the registry entry), if this app knows the id.
pub fn widget_def(id: &str) -> Option<&'static Value> {
    registry().get("widgets")?.as_array()?.iter().find(|w| w.get("id").and_then(Value::as_str) == Some(id))
}

/// A refresh value in ms (0 = on data change only): "5s" → 5000.
pub fn refresh_ms(v: &str) -> u64 {
    let Some(d) = v.strip_suffix('s') else { return 0 };
    if d.is_empty() || !d.bytes().all(|b| b.is_ascii_digit()) {
        return 0;
    }
    d.parse::<u64>().map(|n| n.saturating_mul(1000)).unwrap_or(0)
}

fn valid_w(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty() && b.len() <= 32 && b[0].is_ascii_alphabetic() && b.iter().all(|c| c.is_ascii_alphanumeric())
}

/// A stored widget list, repaired like LAYOUT.md §5.5 (opts.widgets). Returns None when `value` is not an array (the
/// caller decides: no stored list yet → the defaults). Items that are not objects with a valid `w` are dropped; a bad
/// `size` / `settings` (not an object, or > 16 keys) / `collapsed` (not a boolean) is dropped from its item; a second
/// item of the same `w` is dropped (one instance per widget in v1); at most 12 items. Unknown keys stay.
pub fn normalize_items(value: &Value) -> Option<Vec<Map<String, Value>>> {
    let arr = value.as_array()?;
    let mut out: Vec<Map<String, Value>> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for v in arr {
        let Some(o) = v.as_object() else { continue };
        let Some(w) = o.get("w").and_then(Value::as_str) else { continue };
        if !valid_w(w) || seen.iter().any(|s| s == w) {
            continue;
        }
        let mut it = o.clone();
        if let Some(sz) = it.get("size") {
            if !sz.as_str().is_some_and(|s| SIZES.contains(&s)) {
                it.remove("size");
            }
        }
        if let Some(st) = it.get("settings") {
            if !st.as_object().is_some_and(|m| m.len() <= 16) {
                it.remove("settings");
            }
        }
        if let Some(c) = it.get("collapsed") {
            if !c.is_boolean() {
                it.remove("collapsed");
            }
        }
        seen.push(w.to_string());
        out.push(it);
        if out.len() >= MAX_WIDGETS {
            break;
        }
    }
    Some(out)
}

/// The list a new user starts with: every default widget at its default size.
pub fn default_items() -> Vec<Map<String, Value>> {
    let defs = registry().get("defaults").and_then(Value::as_array).cloned().unwrap_or_default();
    defs.iter()
        .filter_map(Value::as_str)
        .filter(|id| widget_def(id).is_some())
        .map(|id| {
            let mut m = Map::new();
            m.insert("w".into(), Value::String(id.to_string()));
            m
        })
        .collect()
}

/// The size an item renders at: its own when the widget offers it, else the widget's default ("M" for unknown ids).
pub fn size_of(item: &Map<String, Value>) -> String {
    let w = item.get("w").and_then(Value::as_str).unwrap_or("");
    let Some(def) = widget_def(w) else { return "M".into() };
    let sizes: Vec<&str> = def.get("sizes").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    match item.get("size").and_then(Value::as_str) {
        Some(s) if !s.is_empty() && sizes.contains(&s) => s.to_string(),
        _ => def.get("default").and_then(Value::as_str).unwrap_or("M").to_string(),
    }
}

/// The settings an item runs with: every declared setting, its stored value when valid (ints floored + clamped to
/// [min, max], bools as booleans), else the default.
pub fn settings_of(item: &Map<String, Value>) -> Map<String, Value> {
    let mut out = Map::new();
    let w = item.get("w").and_then(Value::as_str).unwrap_or("");
    let Some(def) = widget_def(w) else { return out };
    let empty = Map::new();
    let stored = item.get("settings").and_then(Value::as_object).unwrap_or(&empty);
    for s in def.get("settings").and_then(Value::as_array).into_iter().flatten() {
        let Some(id) = s.get("id").and_then(Value::as_str) else { continue };
        let v = stored.get(id);
        match s.get("type").and_then(Value::as_str) {
            Some("int") => {
                let min = s.get("min").and_then(Value::as_f64).unwrap_or(f64::NAN);
                let max = s.get("max").and_then(Value::as_f64).unwrap_or(f64::NAN);
                let val = match v.and_then(Value::as_f64).filter(|x| x.is_finite()) {
                    Some(x) => num(crate::js_max(min, crate::js_min(max, x.floor()))),
                    None => s.get("default").cloned().unwrap_or(Value::Null),
                };
                out.insert(id.to_string(), val);
            }
            _ => {
                let val = match v {
                    Some(Value::Bool(b)) => Value::Bool(*b),
                    _ => s.get("default").cloned().unwrap_or(Value::Null),
                };
                out.insert(id.to_string(), val);
            }
        }
    }
    out
}

/// A setting as an integer (the widget's own reading of `settings_of`).
pub fn int_setting(item: &Map<String, Value>, id: &str) -> Option<i64> {
    settings_of(item).get(id).and_then(Value::as_f64).map(|x| x as i64)
}
