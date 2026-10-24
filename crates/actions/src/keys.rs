//! keys.rs — port of `keys.ts`: key sequences (ACTIONS.md §8.1) — parsing a sequence into its canonical text, and the
//! web's key classes. ASCII case folding only; lengths the reference measures in UTF-16 code units are measured the
//! same way here (`utf16_len`), in code points where it counts code points.

use std::sync::OnceLock;

use serde_json::Value;

use crate::js::{is_arr, is_obj};
use crate::table::table;

/// `Sequence`: a parsed key sequence.
#[derive(Clone, Debug, PartialEq)]
pub struct Sequence {
    /// canonical text: Ctrl, Alt, Shift, Meta (in this order) then the key, joined by "+"; a bare modifier alone
    pub text: String,
    /// A–Z, 0–9, F1–F24, a named key, a punctuation character — or the modifier of a bare-modifier sequence
    pub key: String,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub meta: bool,
    /// a modifier held on its own (push to talk)
    pub bare: bool,
}

pub const MODIFIERS: [&str; 4] = ["Ctrl", "Alt", "Shift", "Meta"];

fn is_modifier_name(s: &str) -> bool {
    MODIFIERS.contains(&s)
}

/// The table's key registries (`keys`, `webKeys`), read once.
struct Keys {
    named: Vec<String>,
    function: u32,
    punctuation: Vec<String>,
    aliases: Vec<(String, String)>,
    reserved: Vec<String>,
    web_reserved: Vec<String>,
    web_browser: Vec<String>,
}

fn strs(v: Option<&Value>) -> Vec<String> {
    is_arr(v).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

fn keys() -> &'static Keys {
    static K: OnceLock<Keys> = OnceLock::new();
    K.get_or_init(|| {
        let j = &table().json;
        let k = j.get("keys");
        let w = j.get("webKeys");
        let aliases = is_obj(k.and_then(|k| k.get("aliases")))
            .map(|o| o.iter().filter_map(|(a, b)| b.as_str().map(|b| (a.clone(), b.to_string()))).collect())
            .unwrap_or_default();
        Keys {
            named: strs(k.and_then(|k| k.get("named"))),
            function: k.and_then(|k| k.get("function")).and_then(Value::as_f64).unwrap_or(24.0) as u32,
            punctuation: strs(k.and_then(|k| k.get("punctuation"))),
            aliases,
            reserved: strs(k.and_then(|k| k.get("reserved"))),
            web_reserved: strs(w.and_then(|w| w.get("reserved"))),
            web_browser: strs(w.and_then(|w| w.get("browser"))),
        }
    })
}

/// `keys.reserved` (never user-bindable).
pub fn reserved() -> &'static [String] {
    &keys().reserved
}

/// ASCII-only lower case.
pub fn ascii_lower(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_uppercase() { c.to_ascii_lowercase() } else { c }).collect()
}

fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

fn alias(l: &str) -> Option<&'static str> {
    keys().aliases.iter().find(|(a, _)| a == l).map(|(_, b)| b.as_str())
}

/// A token as a modifier (canonical name) or None.
fn as_modifier(tok: &str) -> Option<&'static str> {
    let l = ascii_lower(tok);
    for m in MODIFIERS {
        if ascii_lower(m) == l {
            return Some(m);
        }
    }
    let al = alias(&l)?;
    MODIFIERS.iter().copied().find(|m| *m == al)
}

/// "f1".."f24" (ASCII case-insensitive, no leading zero) → "F1".."F24".
fn as_function_key(l: &str) -> Option<String> {
    let units: Vec<u16> = l.encode_utf16().collect();
    if units.len() < 2 || units.len() > 3 || units[0] != 0x66 {
        return None;
    }
    let mut n: u32 = 0;
    for (i, &d) in units.iter().enumerate().skip(1) {
        if !(0x30..=0x39).contains(&d) {
            return None;
        }
        if i == 1 && d == 0x30 {
            return None;
        }
        n = n * 10 + (d - 0x30) as u32;
    }
    (n >= 1 && n <= keys().function).then(|| format!("F{n}"))
}

/// A token as a key (canonical name) or None.
fn as_key(tok: &str) -> Option<String> {
    if utf16_len(tok) == 1 {
        let c = tok.chars().next()?;
        if c.is_ascii_lowercase() {
            return Some(c.to_ascii_uppercase().to_string());
        }
        if c.is_ascii_uppercase() || c.is_ascii_digit() {
            return Some(tok.to_string());
        }
        if keys().punctuation.iter().any(|p| p == tok) {
            return Some(tok.to_string());
        }
        return None;
    }
    let l = ascii_lower(tok);
    if let Some(f) = as_function_key(&l) {
        return Some(f);
    }
    if let Some(k) = keys().named.iter().find(|k| ascii_lower(k) == l) {
        return Some(k.clone());
    }
    let al = alias(&l)?;
    (!is_modifier_name(al)).then(|| al.to_string())
}

fn trim_spaces(s: &str) -> &str {
    s.trim_matches(' ')
}

/// Parse a key sequence (ACTIONS.md §8.1). The first failure: not-string, empty, too-long (more than
/// limits.sequenceChars code points), then per token left to right: empty-token, key-not-last, duplicate-modifier,
/// two-keys, unknown-key; finally no-key.
pub fn parse_sequence(text: Option<&Value>) -> Result<Sequence, &'static str> {
    let Some(Value::String(text)) = text else { return Err("not-string") };
    parse_str(text)
}

/// `parseSequence` of a string.
pub fn parse_str(text: &str) -> Result<Sequence, &'static str> {
    if text.is_empty() {
        return Err("empty");
    }
    let limit = table().limits.get("sequenceChars").copied().unwrap_or(64.0);
    if text.chars().count() as f64 > limit {
        return Err("too-long");
    }
    // spaces around the whole text go first, so a trailing "+" key is still seen ("Ctrl++ ")
    let t = trim_spaces(text);
    let toks: Vec<&str> = if t == "+" {
        vec!["+"]
    } else if let Some(head) = t.strip_suffix("++") {
        let mut v: Vec<&str> = head.split('+').collect();
        v.push("+");
        v
    } else {
        t.split('+').collect()
    };
    let (mut ctrl, mut alt, mut shift, mut meta) = (false, false, false, false);
    let mut key: Option<String> = None;
    let mut n_mods = 0;
    for raw in &toks {
        let tok = trim_spaces(raw);
        if tok.is_empty() {
            return Err("empty-token");
        }
        if let Some(m) = as_modifier(tok) {
            if key.is_some() {
                return Err("key-not-last");
            }
            let flag = match m {
                "Ctrl" => &mut ctrl,
                "Alt" => &mut alt,
                "Shift" => &mut shift,
                _ => &mut meta,
            };
            if *flag {
                return Err("duplicate-modifier");
            }
            *flag = true;
            n_mods += 1;
            continue;
        }
        let k = as_key(tok);
        if key.is_some() {
            return Err("two-keys");
        }
        let Some(k) = k else { return Err("unknown-key") };
        key = Some(k);
    }
    let Some(key) = key else {
        if n_mods == 1 && toks.len() == 1 {
            let m = if ctrl {
                "Ctrl"
            } else if alt {
                "Alt"
            } else if shift {
                "Shift"
            } else {
                "Meta"
            };
            return Ok(Sequence { text: m.into(), key: m.into(), ctrl: false, alt: false, shift: false, meta: false, bare: true });
        }
        return Err("no-key");
    };
    Ok(Sequence { text: chord_text(&key, ctrl, alt, shift, meta), key, ctrl, alt, shift, meta, bare: false })
}

/// The canonical text of a key press: the flags + the key (the key already canonical).
pub fn chord_text(key: &str, ctrl: bool, alt: bool, shift: bool, meta: bool) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if ctrl {
        parts.push("Ctrl");
    }
    if alt {
        parts.push("Alt");
    }
    if shift {
        parts.push("Shift");
    }
    if meta {
        parts.push("Meta");
    }
    parts.push(key);
    parts.join("+")
}

/// How browsers treat a canonical sequence: "reserved", "browser" or "free".
pub fn web_class(canonical: &str) -> &'static str {
    if keys().web_reserved.iter().any(|k| k == canonical) {
        "reserved"
    } else if keys().web_browser.iter().any(|k| k == canonical) {
        "browser"
    } else {
        "free"
    }
}

/// F1–F24.
pub fn is_function_key(key: &str) -> bool {
    utf16_len(key) >= 2 && key.starts_with('F') && as_function_key(&ascii_lower(key)).as_deref() == Some(key)
}

/// The sequence as the reference's result object (`{ok: true, text, key, ctrl, alt, shift, meta, bare}`).
pub fn sequence_json(r: &Result<Sequence, &'static str>) -> Value {
    let mut o = crate::js::obj();
    match r {
        Ok(q) => {
            o.insert("ok".into(), Value::Bool(true));
            o.insert("text".into(), Value::String(q.text.clone()));
            o.insert("key".into(), Value::String(q.key.clone()));
            o.insert("ctrl".into(), Value::Bool(q.ctrl));
            o.insert("alt".into(), Value::Bool(q.alt));
            o.insert("shift".into(), Value::Bool(q.shift));
            o.insert("meta".into(), Value::Bool(q.meta));
            o.insert("bare".into(), Value::Bool(q.bare));
        }
        Err(e) => {
            o.insert("ok".into(), Value::Bool(false));
            o.insert("error".into(), Value::String((*e).into()));
        }
    }
    Value::Object(o)
}
