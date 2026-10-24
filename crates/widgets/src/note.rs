//! note — the Sticky note's rules (contract WIDGETS.md §14, vectors testvectors/widgets/note/*): one plain-text note per
//! space in the space's `dev.tinhat.note` state (key ""), `{text, v: 1}`, at most 500 code points, http / https links
//! clickable. A line-for-line port of the web's `src/widgets/note.ts` (widgets W1, 2026-09-28): code points, ASCII rules.

use serde_json::{json, Map, Value};

pub const NOTE_TYPE: &str = "dev.tinhat.note";
pub const NOTE_MAX: usize = 500;
pub const NOTE_NEAR: usize = 450;

fn is_ws(c: char) -> bool {
    c == ' ' || ('\u{9}'..='\u{d}').contains(&c)
}

/// CR LF and lone CR → LF; ASCII whitespace trimmed from both ends of the whole text.
pub fn normalize_note(s: &str) -> String {
    let t = s.replace("\r\n", "\n").replace('\r', "\n");
    t.trim_matches(is_ws).to_string()
}

/// A space's note content → the text shown, or None (no note); over [`NOTE_MAX`] → cut.
pub fn note_of(content: &Value) -> Option<String> {
    let t = content.as_object()?.get("text")?.as_str()?;
    let t = normalize_note(t);
    if t.is_empty() {
        return None;
    }
    if t.chars().count() > NOTE_MAX {
        return Some(t.chars().take(NOTE_MAX).collect());
    }
    Some(t)
}

/// What Save writes: `Ok(content)` (`{}` clears) or `Err("tooLong")`.
pub fn note_to_save(input: &str) -> Result<Value, &'static str> {
    let t = normalize_note(input);
    if t.is_empty() {
        return Ok(Value::Object(Map::new()));
    }
    if t.chars().count() > NOTE_MAX {
        return Err("tooLong");
    }
    Ok(json!({ "text": t, "v": 1 }))
}

/// The web's `noteToSave` result as JSON (`{ok, content}` / `{ok: false, reason}`).
pub fn note_to_save_json(input: &str) -> Value {
    match note_to_save(input) {
        Ok(content) => json!({ "ok": true, "content": content }),
        Err(reason) => json!({ "ok": false, "reason": reason }),
    }
}

/// The editor's counter: `(count, near, over)`.
pub fn note_counter(input: &str) -> (usize, bool, bool) {
    let n = normalize_note(input).chars().count();
    (n, n >= NOTE_NEAR, n > NOTE_MAX)
}

pub fn note_counter_json(input: &str) -> Value {
    let (count, near, over) = note_counter(input);
    json!({ "count": count, "max": NOTE_MAX, "near": near, "over": over })
}

/// One segment of the note: plain text, or a link (`url`).
#[derive(Clone, Debug, PartialEq)]
pub struct NoteSeg {
    pub text: String,
    pub url: Option<String>,
}

fn stops(c: char) -> bool {
    is_ws(c) || c == '<' || c == '>' || c == '"' || c < ' '
}

/// The note as plain and link segments (the web's `noteSegments`).
pub fn note_segments(text: &str) -> Vec<NoteSeg> {
    let cps: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut plain = String::new();
    let at = |k: usize, s: &str| -> bool {
        let want: Vec<char> = s.chars().collect();
        k + want.len() <= cps.len() && want.iter().enumerate().all(|(n, ch)| cps[k + n].to_ascii_lowercase() == *ch)
    };
    let mut i = 0;
    while i < cps.len() {
        let scheme = if at(i, "https://") {
            8
        } else if at(i, "http://") {
            7
        } else {
            0
        };
        if scheme > 0 && (i == 0 || !cps[i - 1].is_ascii_alphanumeric()) {
            let mut j = i + scheme;
            while j < cps.len() && !stops(cps[j]) {
                j += 1;
            }
            let mut end = j;
            loop {
                if end <= i + scheme {
                    break;
                }
                let c = cps[end - 1];
                if matches!(c, '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '"') {
                    end -= 1;
                    continue;
                }
                let open = match c {
                    ')' => Some('('),
                    ']' => Some('['),
                    '}' => Some('{'),
                    _ => None,
                };
                if let Some(open) = open {
                    let inner = &cps[i..end - 1];
                    let opens = inner.iter().filter(|x| **x == open).count();
                    let closes = inner.iter().filter(|x| **x == c).count();
                    if opens <= closes {
                        end -= 1;
                        continue;
                    }
                }
                break;
            }
            if end > i + scheme {
                if !plain.is_empty() {
                    out.push(NoteSeg { text: std::mem::take(&mut plain), url: None });
                }
                let url: String = cps[i..end].iter().collect();
                out.push(NoteSeg { text: url.clone(), url: Some(url) });
                i = end;
                continue;
            }
        }
        plain.push(cps[i]);
        i += 1;
    }
    if !plain.is_empty() {
        out.push(NoteSeg { text: plain, url: None });
    }
    out
}

pub fn note_segments_json(text: &str) -> Value {
    Value::Array(
        note_segments(text)
            .into_iter()
            .map(|s| match s.url {
                Some(u) => json!({ "text": s.text, "url": u }),
                None => json!({ "text": s.text }),
            })
            .collect(),
    )
}

/// The Small line: the first non-blank line, ASCII whitespace collapsed, ≤ 140 code points (+ "…"); none → "".
pub fn note_first_line(text: Option<&str>) -> String {
    let Some(text) = text else { return String::new() };
    for line in text.split('\n') {
        let mut out = String::new();
        let mut pend = false;
        for ch in line.chars() {
            if is_ws(ch) {
                pend = !out.is_empty();
                continue;
            }
            if pend {
                out.push(' ');
            }
            pend = false;
            out.push(ch);
        }
        if out.is_empty() {
            continue;
        }
        if out.chars().count() <= 140 {
            return out;
        }
        let mut cut: String = out.chars().take(140).collect();
        if cut.ends_with(' ') {
            cut.pop();
        }
        cut.push('…');
        return cut;
    }
    String::new()
}
