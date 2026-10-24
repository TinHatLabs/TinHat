//! The Soundboard pad — port of the web's `src/widgets/soundpad.ts` (widget `soundPad`, contract WIDGETS.md §22,
//! vectors `testvectors/widgets/soundpad/*`): your soundboard favourites (the personal sounds account data) as the
//! pad's buttons, and whether a press may play into the call (the advisory `use_soundboard` permission) with the one
//! line under the pad — the soundboard panel shows the same line. Pure.

use serde_json::{json, Value};

pub const DEFAULT_EMOJI: &str = "🔊";
pub const OFF_TEXT: &str = "The soundboard is off for your role in this call.";
pub const PREVIEW_TEXT: &str = "Only you hear these here. Join a call to play them for everyone.";

#[derive(Clone, Debug, PartialEq)]
pub struct PadSound {
    pub name: String,
    pub url: String,
    pub emoji: String,
}

impl PadSound {
    pub fn to_json(&self) -> Value {
        json!({ "name": self.name, "url": self.url, "emoji": self.emoji })
    }
}

fn trim_ascii(s: &str) -> &str {
    s.trim_matches(|c: char| c == ' ' || ('\t'..='\r').contains(&c))
}

/// The favourites content → the pad's buttons (see soundpad.ts `padSounds`).
pub fn pad_sounds(content: &Value, max: usize) -> Vec<PadSound> {
    let mut out: Vec<PadSound> = Vec::new();
    let list = content.get("sounds").and_then(Value::as_array).filter(|_| content.is_object());
    for e in list.map(Vec::as_slice).unwrap_or(&[]) {
        if !e.is_object() {
            continue;
        }
        let name = e.get("name").and_then(Value::as_str).map(trim_ascii).unwrap_or("");
        let url = e.get("sourceUrl").and_then(Value::as_str).map(trim_ascii).unwrap_or("");
        if name.is_empty() || url.is_empty() || out.iter().any(|x| x.url == url) {
            continue;
        }
        let emoji = e.get("emoji").and_then(Value::as_str).map(trim_ascii).unwrap_or("");
        out.push(PadSound { name: name.into(), url: url.into(), emoji: if emoji.is_empty() { DEFAULT_EMOJI.into() } else { emoji.into() } });
        if max > 0 && out.len() >= max {
            break;
        }
    }
    out
}

/// `(allowed, hint)` (see soundpad.ts `padGate`).
pub fn pad_gate(in_call: bool, call_perms: &Value) -> (bool, &'static str) {
    if in_call {
        if call_perms.get("use_soundboard") == Some(&Value::Bool(false)) && call_perms.is_object() {
            return (false, OFF_TEXT);
        }
        return (true, "");
    }
    (true, PREVIEW_TEXT)
}
