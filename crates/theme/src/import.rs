//! import.rs — port of `import.ts` (ENGINE.md §7, B13): terminal theme files → `tinhat-theme` v1 documents, format
//! detected from content. Pure; reproduces `testvectors/import/<format>/*.expect.json` byte for byte.
//! Formats: iTerm2 .itermcolors, kitty, Alacritty TOML + legacy YAML, Ghostty, WezTerm TOML, Windows Terminal JSON,
//! Xresources (incl. #define and the iTerm2 collection's xrdb files), base16/base24 YAML (tinted-theming terminal
//! mapping), and our own tinhat-theme JSON (returned unchanged).
//!
//! Every regex of the reference is written out by hand below (`re_*`); the comment above each one is the regex it
//! implements, and why the hand-written form matches exactly the same lines (no backtracking can succeed otherwise).

use crate::js::{is_js_line_terminator, is_js_space, js_dot_star_to_end, js_trim};
use crate::limits::{contains_url, take_code_points, MAX_STRING};
use crate::literal::push_hex2;
use crate::parsers::{parse_jsonc, parse_plist, parse_toml, parse_yaml, Doc, Node, NodeId, ParseError};
use serde_json::{json, Map, Value};
use std::collections::HashMap;

/// The formats `import_theme` recognises.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ImportFormat {
    Tinhat,
    Iterm2,
    Kitty,
    AlacrittyToml,
    AlacrittyYaml,
    Ghostty,
    Wezterm,
    WindowsTerminal,
    Xresources,
    Base16,
    Base24,
}

impl ImportFormat {
    /// Every format, sorted by name (the reference's `IMPORT_FORMATS`).
    pub const ALL: [ImportFormat; 11] = [
        ImportFormat::AlacrittyToml,
        ImportFormat::AlacrittyYaml,
        ImportFormat::Base16,
        ImportFormat::Base24,
        ImportFormat::Ghostty,
        ImportFormat::Iterm2,
        ImportFormat::Kitty,
        ImportFormat::Tinhat,
        ImportFormat::Wezterm,
        ImportFormat::WindowsTerminal,
        ImportFormat::Xresources,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ImportFormat::Tinhat => "tinhat",
            ImportFormat::Iterm2 => "iterm2",
            ImportFormat::Kitty => "kitty",
            ImportFormat::AlacrittyToml => "alacritty-toml",
            ImportFormat::AlacrittyYaml => "alacritty-yaml",
            ImportFormat::Ghostty => "ghostty",
            ImportFormat::Wezterm => "wezterm",
            ImportFormat::WindowsTerminal => "windows-terminal",
            ImportFormat::Xresources => "xresources",
            ImportFormat::Base16 => "base16",
            ImportFormat::Base24 => "base24",
        }
    }
}

/// A successful import: the detected format and the theme document (for `tinhat`, the file itself).
#[derive(Clone, Debug, PartialEq)]
pub struct ImportResult {
    pub format: ImportFormat,
    pub theme: Value,
}

/// An import failure: `"<code>: <detail>"` (`import-empty`, `import-too-large`, `unknown-format`, `bad-json`,
/// `bad-plist`, `bad-toml`, `bad-yaml`, `import-incomplete`, `import-bad-colour`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportError {
    pub code: &'static str,
    pub detail: String,
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.detail)
    }
}

impl std::error::Error for ImportError {}

impl From<ParseError> for ImportError {
    fn from(e: ParseError) -> ImportError {
        ImportError { code: e.code, detail: e.detail }
    }
}

/// `{ok: true, format, theme}` / `{ok: false, error}` — the `.expect.json` of an import vector.
pub fn import_result_to_json(r: &Result<ImportResult, ImportError>) -> Value {
    match r {
        Ok(v) => json!({ "ok": true, "format": v.format.as_str(), "theme": v.theme }),
        Err(e) => json!({ "ok": false, "error": e.to_string() }),
    }
}

pub const MAX_IMPORT_BYTES: usize = 262144;

type R<T> = Result<T, ImportError>;

fn fail<T>(code: &'static str, detail: impl Into<String>) -> R<T> {
    Err(ImportError { code, detail: detail.into() })
}

fn unknown<T>() -> R<T> {
    fail("unknown-format", "not a theme file this app understands")
}

// ---- colours ----------------------------------------------------------------------------------------------------

fn is_hex(s: &[u8]) -> bool {
    !s.is_empty() && s.iter().all(u8::is_ascii_hexdigit)
}

/// A colour as terminal theme files write it → `#rrggbb` (lower case), or None: `#rgb`, `#rrggbb`, bare `rrggbb`,
/// `0xrrggbb`, X11 `rgb:h/h/h` (1–4 hex digits per channel, scaled with round-half-up in integers). Surrounding
/// spaces and one pair of double quotes are ignored. Anything else (names, `none`, alpha forms) → None.
/// (Byte-wise: any non-ASCII character makes every branch fail, as in the reference.)
pub fn import_colour(raw: &str) -> Option<String> {
    let mut s = js_trim(raw);
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        s = js_trim(&s[1..s.len() - 1]);
    }
    let b = s.as_bytes();
    let h: &[u8] = if b.first() == Some(&b'#') {
        &b[1..]
    } else if b.len() == 8 && (b.starts_with(b"0x") || b.starts_with(b"0X")) {
        &b[2..]
    } else if b.len() == 6 {
        b
    } else if b.len() >= 4 && b[..4].eq_ignore_ascii_case(b"rgb:") {
        let parts: Vec<&[u8]> = b[4..].split(|&c| c == b'/').collect();
        if parts.len() != 3 || parts.iter().any(|p| p.is_empty() || p.len() > 4 || !is_hex(p)) {
            return None;
        }
        let mut out = String::from("#");
        for p in parts {
            let max = 16u32.pow(p.len() as u32) - 1;
            let v = u32::from_str_radix(std::str::from_utf8(p).ok()?, 16).ok()?;
            // Math.floor((v * 510 + max) / (2 * max)): exact integers far below 2^53, so integer division is equal
            push_hex2(&mut out, ((v * 510 + max) / (2 * max)) as u8);
        }
        return Some(out);
    } else {
        return None;
    };
    if !is_hex(h) {
        return None;
    }
    let h = h.to_ascii_lowercase();
    match h.len() {
        3 => {
            let d = |i: usize| h[i] as char;
            Some(format!("#{}{}{}{}{}{}", d(0), d(0), d(1), d(1), d(2), d(2)))
        }
        6 => Some(format!("#{}", std::str::from_utf8(&h).ok()?)),
        _ => None,
    }
}

// ---- names, authors, ids ----------------------------------------------------------------------------------------

fn is_ascii_ws(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{c}' | '\u{b}')
}

fn controls_to_space(s: &str) -> String {
    s.chars().map(|c| if (c as u32) < 0x20 || c as u32 == 0x7f { ' ' } else { c }).collect()
}

fn words(s: &str) -> Vec<&str> {
    s.split(is_ascii_ws).filter(|w| !w.is_empty()).collect()
}

fn trim_chars<'a>(s: &'a str, set: &str) -> &'a str {
    s.trim_matches(|c| set.contains(c))
}

/// A display name: control characters → space, words with a URL dropped, spaces collapsed, ≤ 80 code points.
pub fn clean_name(s: &str) -> String {
    if s.is_empty() {
        return String::new();
    }
    let c = controls_to_space(s);
    let w: Vec<&str> = words(&c).into_iter().filter(|x| !contains_url(x)).collect();
    let joined = w.join(" ");
    trim_chars(take_code_points(&joined, MAX_STRING), " ").to_string()
}

fn linkish(w: &str) -> bool {
    contains_url(w) || w.contains('@')
}

/// An author line without links or e-mail addresses:
///  1. control characters → space; 2. `<…>` spans removed (to the next ">", or the end);
///  3. `(…)` spans removed when their text holds a URL or "@", or has no whitespace but a "." (a bare domain);
///  4. split at ","; a part with a word holding a URL or "@" is dropped whole; the others have their whitespace
///     collapsed and are trimmed; empty parts dropped; joined with ", ";
///  5. " ,;:" trimmed at both ends; at most 80 code points (then trimmed again).
pub fn clean_author(s: &str) -> String {
    if s.is_empty() {
        return String::new();
    }
    let t = controls_to_space(s);
    let b = t.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'<' {
            i = match crate::js::find_byte(b, b'>', i + 1) {
                Some(e) => e + 1,
                None => b.len(),
            };
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    let t = crate::js::bytes_to_string(out);
    let b = t.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'(' {
            let e = crate::js::find_byte(b, b')', i + 1);
            let inner = match e {
                Some(e) => &t[i + 1..e],
                None => &t[i + 1..],
            };
            let bare_domain = words(inner).len() == 1 && inner.contains('.') && !inner.chars().any(is_ascii_ws);
            if linkish(inner) || bare_domain {
                i = match e {
                    Some(e) => e + 1,
                    None => b.len(),
                };
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    let out = crate::js::bytes_to_string(out);
    let mut parts: Vec<String> = Vec::new();
    for part in out.split(',') {
        let w = words(part);
        if w.is_empty() || w.iter().any(|x| linkish(x)) {
            continue;
        }
        parts.push(w.join(" "));
    }
    let joined = parts.join(", ");
    let r = trim_chars(&joined, " ,;:");
    trim_chars(take_code_points(r, MAX_STRING), " ,;:").to_string()
}

/// Latin-1 Supplement + Latin Extended-A letters → ASCII (for ids). Index = code point - 0xC0; "_" = drop.
const FOLD_C0: &str = concat!(
    "a a a a a a ae c e e e e i i i i d n o o o o o _ o u u u u y th ss ", // U+00C0–00DF
    "a a a a a a ae c e e e e i i i i d n o o o o o _ o u u u u y th y ", // U+00E0–00FF
    "a a a a a a c c c c c c c c d d d d e e e e e e e e e e g g g g g g g g h h h h i i i i i i i i i i ij ij j j k k k ", // U+0100–0138
    "l l l l l l l l l l n n n n n n n n n o o o o o o oe oe r r r r r r s s s s s s s s t t t t t t u u u u u u u u u u u u w w y y y z z z z z z s" // U+0139–017F
);

fn fold_char(cp: u32) -> &'static str {
    if (0xc0..=0x17f).contains(&cp) {
        let f = FOLD_C0.split(' ').nth((cp - 0xc0) as usize).unwrap_or("");
        return if f == "_" { "" } else { f };
    }
    ""
}

/// Theme id from a name: ASCII letters lower-cased and digits kept; Latin-1/Latin Extended-A letters folded to ASCII
/// (é → e, ß → ss); "+" becomes the word "plus"; every other character separates words; words joined with "-";
/// at most 64 characters (trailing "-" removed); "imported" when nothing is left.
pub fn slugify(name: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut w = String::new();
    fn flush(parts: &mut Vec<String>, w: &mut String) {
        if !w.is_empty() {
            parts.push(std::mem::take(w));
        }
    }
    for ch in name.chars() {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            w.push(ch);
        } else if ch.is_ascii_uppercase() {
            w.push(ch.to_ascii_lowercase());
        } else if ch == '+' {
            flush(&mut parts, &mut w);
            parts.push("plus".into());
        } else {
            let f = fold_char(ch as u32);
            if f.is_empty() {
                flush(&mut parts, &mut w);
            } else {
                w.push_str(f);
            }
        }
    }
    flush(&mut parts, &mut w);
    let id = parts.join("-");
    let mut id: &str = &id[..id.len().min(64)]; // all ASCII
    while id.ends_with('-') {
        id = &id[..id.len() - 1];
    }
    if id.is_empty() {
        "imported".into()
    } else {
        id.to_string()
    }
}

const KNOWN_EXT: [&str; 11] =
    [".itermcolors", ".conf", ".toml", ".yml", ".yaml", ".json", ".jsonc", ".xresources", ".xrdb", ".theme", ".txt"];

/// Display name from a file name: the base name, a known extension removed, "_" → space, then clean_name.
pub fn name_from_file(hint: &str) -> String {
    if hint.is_empty() {
        return String::new();
    }
    let slash = hint.rfind(['/', '\\']).map_or(0, |p| p + 1);
    let mut base = &hint[slash..];
    let lower = base.to_ascii_lowercase();
    for e in KNOWN_EXT {
        if lower.ends_with(e) && base.len() > e.len() {
            base = &base[..base.len() - e.len()];
            break;
        }
    }
    clean_name(&base.replace('_', " "))
}

// ---- collecting a palette ---------------------------------------------------------------------------------------

#[derive(Default)]
struct Collect {
    v: HashMap<String, String>, // slot ("bg", "c3", …) → raw text, last assignment wins
    name: String,
    author: String,
}

impl Collect {
    fn set(&mut self, slot: &str, raw: Option<&str>) {
        if let Some(t) = raw {
            self.v.insert(slot.to_string(), t.to_string());
        }
    }
}

const ANSI_NAMES: [&str; 8] = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"];

fn build(format: ImportFormat, col: &Collect, hint: Option<&str>) -> R<Value> {
    let req = |slot: &str| -> R<String> {
        let Some(raw) = col.v.get(slot) else { return fail("import-incomplete", format!("missing {slot}")) };
        match import_colour(raw) {
            Some(c) => Ok(c),
            None => fail("import-bad-colour", format!("{slot} '{}'", take_code_points(js_trim(raw), 40))),
        }
    };
    let bg = req("bg")?;
    let fg = req("fg")?;
    let mut ansi = Vec::with_capacity(16);
    for i in 0..16 {
        ansi.push(Value::String(req(&format!("c{i}"))?));
    }
    let mut palette = Map::new();
    palette.insert("bg".into(), Value::String(bg));
    palette.insert("fg".into(), Value::String(fg));
    for s in ["cursor", "selectionBg", "selectionFg"] {
        if let Some(c) = col.v.get(s).and_then(|raw| import_colour(raw)) {
            palette.insert(s.into(), Value::String(c));
        }
    }
    palette.insert("ansi".into(), Value::Array(ansi));
    let mut name = clean_name(&col.name);
    if name.is_empty() {
        name = name_from_file(hint.unwrap_or(""));
    }
    if name.is_empty() {
        name = "Imported theme".into();
    }
    let author = clean_author(&col.author);
    let mut theme = Map::new();
    theme.insert("format".into(), json!("tinhat-theme"));
    theme.insert("v".into(), json!(1));
    theme.insert("id".into(), Value::String(slugify(&name)));
    theme.insert("name".into(), Value::String(name));
    if !author.is_empty() {
        theme.insert("author".into(), Value::String(author));
    }
    theme.insert("source".into(), Value::String(format!("import:{}", format.as_str())));
    theme.insert("palette".into(), Value::Object(palette));
    Ok(Value::Object(theme))
}

// ---- formats ----------------------------------------------------------------------------------------------------

/// `/^[+-]?([0-9]+(\.[0-9]*)?|\.[0-9]+)([eE][+-]?[0-9]+)?$/`
fn is_plist_number(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    if matches!(b.first(), Some(b'+' | b'-')) {
        i += 1;
    }
    let int_start = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    let int_digits = i - int_start;
    if int_digits > 0 {
        if b.get(i) == Some(&b'.') {
            i += 1;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
        }
    } else {
        if b.get(i) != Some(&b'.') {
            return false;
        }
        i += 1;
        let f = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == f {
            return false;
        }
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(b.get(i), Some(b'+' | b'-')) {
            i += 1;
        }
        let e = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == e {
            return false;
        }
    }
    i == b.len()
}

fn from_iterm(doc: &Doc, col: &mut Collect) -> R<()> {
    let root = doc.root;
    if !doc.is_obj(root) {
        return fail("bad-plist", "not a property list");
    }
    // colour(d): undefined when d is not a dict; "invalid" when a component is missing or not a number
    let colour = |d: NodeId| -> Option<String> {
        doc.obj(d)?;
        let ch = |k: &str| -> i32 {
            match doc.get(d, k).and_then(|id| match doc.node(id) {
                Node::Str(s) => Some(s.as_str()),
                _ => None,
            }) {
                Some(v) if is_plist_number(v) => {
                    // Math.min(255, Math.max(0, Math.round(Number(v) * 255))); Number(v) is correctly rounded
                    let x: f64 = v.parse().unwrap_or(f64::NAN);
                    let r = crate::js::js_round(x * 255.0);
                    if r > 255.0 {
                        255
                    } else if r > 0.0 {
                        r as i32
                    } else {
                        0
                    }
                }
                _ => -1,
            }
        };
        let (r, g, b) = (ch("Red Component"), ch("Green Component"), ch("Blue Component"));
        if r < 0 || g < 0 || b < 0 {
            return Some("invalid".into());
        }
        let mut s = String::from("#");
        push_hex2(&mut s, r as u8);
        push_hex2(&mut s, g as u8);
        push_hex2(&mut s, b as u8);
        Some(s)
    };
    let mut keys: Vec<(String, String)> = vec![
        ("bg".into(), "Background Color".into()),
        ("fg".into(), "Foreground Color".into()),
        ("cursor".into(), "Cursor Color".into()),
        ("selectionBg".into(), "Selection Color".into()),
        ("selectionFg".into(), "Selected Text Color".into()),
    ];
    for i in 0..16 {
        keys.push((format!("c{i}"), format!("Ansi {i} Color")));
    }
    for (slot, key) in keys {
        if let Some(d) = doc.get(root, &key) {
            let c = colour(d);
            col.set(&slot, c.as_deref());
        }
    }
    Ok(())
}

/// `/^##[ \t]*(name|author)[ \t]*:(.*)$/i` (ASCII-only case folding, as JavaScript's non-unicode `i` flag does for
/// these letters) → (is "name", rest).
fn re_kitty_meta(s: &str) -> Option<(bool, &str)> {
    let b = s.as_bytes();
    if !b.starts_with(b"##") {
        return None;
    }
    let mut i = skip_ws(b, 2);
    let is_name = if b.len() >= i + 4 && b[i..i + 4].eq_ignore_ascii_case(b"name") {
        i += 4;
        true
    } else if b.len() >= i + 6 && b[i..i + 6].eq_ignore_ascii_case(b"author") {
        i += 6;
        false
    } else {
        return None;
    };
    i = skip_ws(b, i);
    if b.get(i) != Some(&b':') {
        return None;
    }
    let rest = &s[i + 1..];
    js_dot_star_to_end(rest).then_some((is_name, rest))
}

/// `/^color([0-9]{1,3})$/` → the number.
fn re_color_n(s: &str) -> Option<u32> {
    let d = s.strip_prefix("color")?;
    if d.is_empty() || d.len() > 3 || !d.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    d.parse().ok()
}

fn from_kitty(t: &str, col: &mut Collect) {
    let map = |k: &str| -> Option<&'static str> {
        Some(match k {
            "foreground" => "fg",
            "background" => "bg",
            "cursor" => "cursor",
            "selection_background" => "selectionBg",
            "selection_foreground" => "selectionFg",
            _ => return None,
        })
    };
    for line in t.split('\n') {
        let s = js_trim(line);
        if s.starts_with('#') {
            if let Some((is_name, rest)) = re_kitty_meta(s) {
                if is_name {
                    col.name = js_trim(rest).to_string();
                } else {
                    col.author = js_trim(rest).to_string();
                }
            }
            continue;
        }
        let w = words(s);
        if w.len() < 2 {
            continue;
        }
        if let Some(n) = re_color_n(w[0]) {
            if n < 16 {
                col.set(&format!("c{n}"), Some(w[1]));
            }
        } else if let Some(slot) = map(w[0]) {
            col.set(slot, Some(w[1]));
        }
    }
}

/// `/^([0-9]{1,3})[ \t]*=(.*)$/` → (number, rest).
fn re_ghostty_palette(val: &str) -> Option<(u32, &str)> {
    let b = val.as_bytes();
    let d = b.iter().take_while(|c| c.is_ascii_digit()).count();
    if !(1..=3).contains(&d) {
        return None; // a 4th digit can be neither [ \t] nor '='
    }
    let i = skip_ws(b, d);
    if b.get(i) != Some(&b'=') {
        return None;
    }
    let rest = &val[i + 1..];
    js_dot_star_to_end(rest).then(|| (val[..d].parse().unwrap_or(999), rest))
}

fn from_ghostty(t: &str, col: &mut Collect) {
    let map = |k: &str| -> Option<&'static str> {
        Some(match k {
            "background" => "bg",
            "foreground" => "fg",
            "cursor-color" => "cursor",
            "selection-background" => "selectionBg",
            "selection-foreground" => "selectionFg",
            _ => return None,
        })
    };
    for line in t.split('\n') {
        let s = js_trim(line);
        if s.is_empty() || s.starts_with('#') {
            continue;
        }
        let Some(eq) = s.find('=') else { continue };
        let key = js_trim(&s[..eq]);
        let val = js_trim(&s[eq + 1..]);
        if key == "palette" {
            if let Some((n, rest)) = re_ghostty_palette(val) {
                if n < 16 {
                    col.set(&format!("c{n}"), Some(js_trim(rest)));
                }
            }
        } else if let Some(slot) = map(key) {
            col.set(slot, Some(val));
        }
    }
}

/// `/^#[ \t]*define[ \t]+(\S+)[ \t]+(.*)$/` → (name, value text).
fn re_define(s: &str) -> Option<(&str, &str)> {
    let b = s.as_bytes();
    if b.first() != Some(&b'#') {
        return None;
    }
    let mut i = skip_ws(b, 1);
    if !b[i..].starts_with(b"define") {
        return None;
    }
    i += 6;
    let j = skip_ws(b, i);
    if j == i {
        return None;
    }
    // (\S+): the maximal run of non-whitespace; it must be followed by at least one space/tab
    let name_end = s[j..].find(is_js_space).map_or(s.len(), |p| j + p);
    if name_end == j {
        return None;
    }
    let k = skip_ws(b, name_end);
    if k == name_end {
        return None;
    }
    let rest = &s[k..];
    js_dot_star_to_end(rest).then_some((&s[j..name_end], rest))
}

fn from_xresources(t: &str, col: &mut Collect) {
    let mut defines: HashMap<String, String> = HashMap::new();
    let mut res: Vec<(String, String)> = Vec::new(); // insertion-ordered Map
    let names = |k: &str| -> Option<&'static str> {
        Some(match k {
            "foreground" => "fg",
            "background" => "bg",
            "cursorColor" => "cursor",
            "highlightColor" => "selectionBg",
            "highlightTextColor" => "selectionFg",
            _ => return None,
        })
    };
    for line in t.split('\n') {
        let s = js_trim(line);
        if s.is_empty() || s.starts_with('!') {
            continue;
        }
        if s.starts_with('#') {
            if let Some((name, value)) = re_define(s) {
                defines.insert(name.to_string(), js_trim(value).to_string());
            }
            continue;
        }
        let Some(colon) = s.find(':') else { continue };
        let spec = js_trim(&s[..colon]);
        let name = &spec[spec.rfind(['*', '.']).map_or(0, |p| p + 1)..];
        let slot = match re_color_n(name) {
            Some(n) if n < 16 => format!("c{n}"),
            Some(_) => String::new(),
            None => names(name).unwrap_or("").to_string(),
        };
        if !slot.is_empty() {
            let v = js_trim(&s[colon + 1..]).to_string();
            match res.iter_mut().find(|(k, _)| *k == slot) {
                Some(e) => e.1 = v,
                None => res.push((slot, v)),
            }
        }
    }
    let expand = |v: &str| -> String {
        let mut x = v.to_string();
        let mut k = 0;
        while k < 8 {
            match defines.get(&x) {
                Some(d) => x = d.clone(),
                None => break,
            }
            k += 1;
        }
        x
    };
    if !res.is_empty() {
        for (slot, v) in &res {
            col.set(slot, Some(&expand(v)));
        }
        return;
    }
    // the iTerm2 collection's xrdb files: only #defines with fixed names
    let mut xrdb: Vec<(String, String)> = vec![
        ("bg".into(), "Background_Color".into()),
        ("fg".into(), "Foreground_Color".into()),
        ("cursor".into(), "Cursor_Color".into()),
        ("selectionBg".into(), "Selection_Color".into()),
        ("selectionFg".into(), "Selected_Text_Color".into()),
    ];
    for i in 0..16 {
        xrdb.push((format!("c{i}"), format!("Ansi_{i}_Color")));
    }
    for (slot, d) in xrdb {
        if defines.contains_key(&d) {
            col.set(&slot, Some(&expand(&d)));
        }
    }
}

fn from_alacritty(doc: &Doc, col: &mut Collect) {
    let c = doc.get(doc.root, "colors");
    let at = |path: &[&str]| -> Option<&str> {
        let mut v = c?;
        for p in path {
            v = doc.get(v, p)?;
        }
        doc.text(v)
    };
    col.set("bg", at(&["primary", "background"]));
    col.set("fg", at(&["primary", "foreground"]));
    col.set("cursor", at(&["cursor", "cursor"]));
    col.set("selectionBg", at(&["selection", "background"]));
    col.set("selectionFg", at(&["selection", "text"]));
    for (i, n) in ANSI_NAMES.iter().enumerate() {
        col.set(&format!("c{i}"), at(&["normal", n]));
        col.set(&format!("c{}", i + 8), at(&["bright", n]));
    }
}

fn from_wezterm(doc: &Doc, col: &mut Collect) {
    let c = doc.get(doc.root, "colors");
    let at = |k: &str| -> Option<&str> { doc.text(doc.get(c?, k)?) };
    col.set("bg", at("background"));
    col.set("fg", at("foreground"));
    col.set("cursor", at("cursor_bg"));
    col.set("selectionBg", at("selection_bg"));
    col.set("selectionFg", at("selection_fg"));
    let arr = |k: &str| -> Option<&Vec<NodeId>> {
        match doc.node(doc.get(c?, k)?) {
            Node::Arr(a) => Some(a),
            _ => None,
        }
    };
    let (ansi, brights) = (arr("ansi"), arr("brights"));
    for i in 0..8 {
        if let Some(a) = ansi {
            col.set(&format!("c{i}"), a.get(i).and_then(|&x| doc.text(x)));
        }
        if let Some(a) = brights {
            col.set(&format!("c{}", i + 8), a.get(i).and_then(|&x| doc.text(x)));
        }
    }
    let meta = |k: &str| -> Option<&str> { doc.text(doc.get(doc.get(doc.root, "metadata")?, k)?) };
    col.name = meta("name").unwrap_or("").to_string();
    col.author = meta("author").unwrap_or("").to_string();
}

fn from_windows_terminal(o: &Map<String, Value>, col: &mut Collect) {
    let mut keys: Vec<(String, String)> = vec![
        ("bg".into(), "background".into()),
        ("fg".into(), "foreground".into()),
        ("cursor".into(), "cursorColor".into()),
        ("selectionBg".into(), "selectionBackground".into()),
    ];
    let wt = ["black", "red", "green", "yellow", "blue", "purple", "cyan", "white"];
    for (i, n) in wt.iter().enumerate() {
        keys.push((format!("c{i}"), n.to_string()));
        keys.push((format!("c{}", i + 8), format!("bright{}{}", n[..1].to_ascii_uppercase(), &n[1..])));
    }
    for (slot, k) in keys {
        col.set(&slot, o.get(&k).and_then(Value::as_str));
    }
    col.name = o.get("name").and_then(Value::as_str).unwrap_or("").to_string();
}

const B16: [&str; 16] = ["00", "08", "0B", "0A", "0D", "0E", "0C", "05", "03", "08", "0B", "0A", "0D", "0E", "0C", "07"];
const B24: [&str; 16] = ["00", "08", "0B", "0A", "0D", "0E", "0C", "05", "03", "12", "14", "13", "16", "17", "15", "07"];

/// `/^base[0-9A-Fa-f]{2}$/`
fn is_base_key(k: &str) -> bool {
    let b = k.as_bytes();
    b.len() == 6 && b.starts_with(b"base") && b[4].is_ascii_hexdigit() && b[5].is_ascii_hexdigit()
}

fn from_base(doc: &Doc) -> R<(ImportFormat, Collect)> {
    let root = doc.root;
    if !doc.is_obj(root) {
        return unknown();
    }
    let pal = match doc.get(root, "palette") {
        Some(p) if doc.is_obj(p) => p,
        _ => root,
    };
    let mut base: HashMap<String, NodeId> = HashMap::new();
    for k in doc.obj(pal).map(|o| o.keys().collect::<Vec<_>>()).unwrap_or_default() {
        if is_base_key(k) {
            base.insert(format!("base{}", k[4..].to_ascii_uppercase()), doc.get(pal, k).expect("own key"));
        }
    }
    let system = doc.get(root, "system").and_then(|id| doc.text(id));
    let format = match system {
        Some("base24") => ImportFormat::Base24,
        Some("base16") => ImportFormat::Base16,
        Some(_) => return unknown(),
        None => {
            if ["10", "11", "12", "13", "14", "15", "16", "17"].iter().all(|x| base.contains_key(&format!("base{x}"))) {
                ImportFormat::Base24
            } else {
                ImportFormat::Base16
            }
        }
    };
    let map = if format == ImportFormat::Base24 { &B24 } else { &B16 };
    let mut col = Collect::default();
    let at = |k: &str| base.get(k).and_then(|&id| doc.text(id));
    col.set("bg", at("base00"));
    col.set("fg", at("base05"));
    col.set("cursor", at("base05"));
    col.set("selectionBg", at("base02"));
    for (i, b) in map.iter().enumerate() {
        col.set(&format!("c{i}"), at(&format!("base{b}")));
    }
    let text = |k: &str| doc.get(root, k).and_then(|id| doc.text(id));
    col.name = text("name").or_else(|| text("scheme")).unwrap_or("").to_string();
    col.author = text("author").unwrap_or("").to_string();
    Ok((format, col))
}

// ---- detection --------------------------------------------------------------------------------------------------

/// `[ \t]*` from i.
fn skip_ws(b: &[u8], mut i: usize) -> usize {
    while matches!(b.get(i), Some(b' ' | b'\t')) {
        i += 1;
    }
    i
}

/// `(#.*)?$` at i: the end, or "#" and no line terminator after it.
fn comment_to_end(s: &str, i: usize) -> bool {
    i == s.len() || (s.as_bytes()[i] == b'#' && !s[i + 1..].chars().any(is_js_line_terminator))
}

/// RE_TOML_HEADER `/^[ \t]*\[\[?[ \t]*[A-Za-z0-9_."' -]+[ \t]*\]\]?[ \t]*(#.*)?$/`. The class holds no `]`, so the
/// content is exactly what lies before the first `]`; a second `[` / `]` must be taken (nothing else can match
/// there). The content matches `[ \t]*C+[ \t]*` iff, with its leading and trailing spaces/tabs removed, it is a
/// non-empty run of class characters (spaces allowed, tabs not) — or it is all spaces/tabs with at least one space.
fn re_toml_header(l: &str) -> bool {
    let b = l.as_bytes();
    let mut i = skip_ws(b, 0);
    if b.get(i) != Some(&b'[') {
        return false;
    }
    i += 1;
    if b.get(i) == Some(&b'[') {
        i += 1;
    }
    let Some(close) = b[i..].iter().position(|&c| c == b']').map(|p| p + i) else { return false };
    let seg = &b[i..close];
    let is_class = |c: u8| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'"' | b'\'' | b' ' | b'-');
    if !seg.iter().all(|&c| is_class(c) || c == b'\t') {
        return false;
    }
    let lead = seg.iter().take_while(|&&c| c == b' ' || c == b'\t').count();
    let trail = seg[lead..].iter().rev().take_while(|&&c| c == b' ' || c == b'\t').count();
    let core = &seg[lead..seg.len() - trail];
    let ok = if core.is_empty() { seg.contains(&b' ') } else { !core.contains(&b'\t') };
    if !ok {
        return false;
    }
    i = close + 1;
    if b.get(i) == Some(&b']') {
        i += 1;
    }
    comment_to_end(l, skip_ws(b, i))
}

/// RE_TOML_COLORS `/^[ \t]*colors\.[A-Za-z0-9_.]+[ \t]*=/`
fn re_toml_colors(l: &str) -> bool {
    let b = l.as_bytes();
    let mut i = skip_ws(b, 0);
    if !b[i..].starts_with(b"colors.") {
        return false;
    }
    i += 7;
    let start = i;
    while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'.') {
        i += 1;
    }
    i > start && b.get(skip_ws(b, i)) == Some(&b'=')
}

/// RE_BASE `/^[ \t]*["']?base0[0-9A-Fa-f]["']?[ \t]*:/`
fn re_base(l: &str) -> bool {
    let b = l.as_bytes();
    let mut i = skip_ws(b, 0);
    if matches!(b.get(i), Some(b'"' | b'\'')) {
        i += 1;
    }
    if !b[i..].starts_with(b"base0") || !b.get(i + 5).is_some_and(u8::is_ascii_hexdigit) {
        return false;
    }
    i += 6;
    if matches!(b.get(i), Some(b'"' | b'\'')) {
        i += 1;
    }
    b.get(skip_ws(b, i)) == Some(&b':')
}

/// RE_ALACRITTY_YAML `/^colors:[ \t]*(#.*)?$/`
fn re_alacritty_yaml(l: &str) -> bool {
    l.starts_with("colors:") && comment_to_end(l, skip_ws(l.as_bytes(), 7))
}

/// RE_GHOSTTY `/^[ \t]*(palette|background|foreground|cursor-color|selection-background|selection-foreground)[ \t]*=/`
fn re_ghostty(l: &str) -> bool {
    let b = l.as_bytes();
    let i = skip_ws(b, 0);
    ["palette", "background", "foreground", "cursor-color", "selection-background", "selection-foreground"]
        .iter()
        .any(|w| b[i..].starts_with(w.as_bytes()) && b.get(skip_ws(b, i + w.len())) == Some(&b'='))
}

/// RE_KITTY `/^[ \t]*(color[0-9]{1,3}|foreground|background|cursor|selection_background|selection_foreground)[ \t]+\S/`
/// (`color` takes its whole digit run: a shorter one leaves a digit where a space must be).
fn re_kitty(l: &str) -> bool {
    let b = l.as_bytes();
    let i = skip_ws(b, 0);
    let ws_then_non_space = |j: usize| -> bool {
        let k = skip_ws(b, j);
        k > j && l[k..].chars().next().is_some_and(|c| !is_js_space(c))
    };
    if b[i..].starts_with(b"color") {
        let d = b[i + 5..].iter().take_while(|c| c.is_ascii_digit()).count();
        if (1..=3).contains(&d) && ws_then_non_space(i + 5 + d) {
            return true;
        }
    }
    ["foreground", "background", "cursor", "selection_background", "selection_foreground"]
        .iter()
        .any(|w| b[i..].starts_with(w.as_bytes()) && ws_then_non_space(i + w.len()))
}

/// RE_XRES `/^[A-Za-z0-9_.*-]*(color[0-9]{1,3}|foreground|background|cursorColor)[ \t]*:/`. Prefix and alternative
/// are both made of class characters and must be followed by a space, tab or ":", so the alternative ends exactly
/// where the leading run of class characters ends.
fn re_xres(l: &str) -> bool {
    let b = l.as_bytes();
    let run = b.iter().take_while(|&&c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'*' | b'-')).count();
    let r = &b[..run];
    let color_n = (1..=3).any(|d| {
        r.len() >= 5 + d && r[r.len() - d..].iter().all(u8::is_ascii_digit) && &r[r.len() - d - 5..r.len() - d] == b"color"
    });
    let word = [&b"foreground"[..], b"background", b"cursorColor"].iter().any(|w| r.ends_with(w));
    (color_n || word) && b.get(skip_ws(b, run)) == Some(&b':')
}

/// RE_XRDB `/^[ \t]*#[ \t]*define[ \t]+(Ansi_[0-9]+_Color|Background_Color|Foreground_Color)[ \t]/`
fn re_xrdb(l: &str) -> bool {
    let b = l.as_bytes();
    let mut i = skip_ws(b, 0);
    if b.get(i) != Some(&b'#') {
        return false;
    }
    i = skip_ws(b, i + 1);
    if !b[i..].starts_with(b"define") {
        return false;
    }
    let j = skip_ws(b, i + 6);
    if j == i + 6 {
        return false;
    }
    let rest = &b[j..];
    let end = if let Some(r) = rest.strip_prefix(b"Ansi_") {
        let d = r.iter().take_while(|c| c.is_ascii_digit()).count();
        if d == 0 || !r[d..].starts_with(b"_Color") {
            return false;
        }
        j + 5 + d + 6
    } else if rest.starts_with(b"Background_Color") || rest.starts_with(b"Foreground_Color") {
        j + 16
    } else {
        return false;
    };
    matches!(b.get(end), Some(b' ' | b'\t'))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Plist,
    Json,
    Toml,
    Base,
    AlacrittyYaml,
    Ghostty,
    Kitty,
    Xresources,
}

/// `/<plist[\s>]/` anywhere in the text.
fn has_plist_tag(t: &str) -> bool {
    let mut from = 0;
    while let Some(p) = t[from..].find("<plist") {
        let at = from + p + 6;
        if t[at..].chars().next().is_some_and(|c| c == '>' || is_js_space(c)) {
            return true;
        }
        from = from + p + 1;
    }
    false
}

/// Content-based detection, first match wins in this order: plist, JSON, TOML, base16/24, Alacritty YAML, Ghostty,
/// kitty, Xresources.
fn detect_kind(t: &str) -> Option<Kind> {
    let ls: Vec<&str> = t.split('\n').collect();
    if let Some(first) = ls.iter().find(|l| !js_trim(l).is_empty()) {
        if js_trim(first).starts_with('<') {
            return has_plist_tag(t).then_some(Kind::Plist);
        }
    }
    for l in &ls {
        let x = js_trim(l);
        if x.is_empty() || x.starts_with('#') || x.starts_with("//") {
            continue;
        }
        if x.starts_with('{') {
            return Some(Kind::Json);
        }
        break;
    }
    if ls.iter().any(|l| re_toml_header(l) || re_toml_colors(l)) {
        return Some(Kind::Toml);
    }
    if ls.iter().any(|l| re_base(l)) {
        return Some(Kind::Base);
    }
    if ls.iter().any(|l| re_alacritty_yaml(l)) {
        return Some(Kind::AlacrittyYaml);
    }
    if ls.iter().any(|l| re_ghostty(l)) {
        return Some(Kind::Ghostty);
    }
    if ls.iter().any(|l| re_kitty(l)) {
        return Some(Kind::Kitty);
    }
    if ls.iter().any(|l| re_xres(l) || re_xrdb(l)) {
        return Some(Kind::Xresources);
    }
    None
}

fn run(input: &str, hint: Option<&str>) -> R<ImportResult> {
    let bytes = input.len();
    if bytes > MAX_IMPORT_BYTES {
        return fail("import-too-large", format!("{bytes} bytes (max {MAX_IMPORT_BYTES})"));
    }
    let t = input.strip_prefix('\u{feff}').unwrap_or(input);
    let t = t.replace("\r\n", "\n").replace('\r', "\n");
    if js_trim(&t).is_empty() {
        return fail("import-empty", "nothing to import");
    }
    let mut col = Collect::default();
    let (format, theme) = match detect_kind(&t) {
        Some(Kind::Plist) => {
            from_iterm(&parse_plist(&t)?, &mut col)?;
            (ImportFormat::Iterm2, build(ImportFormat::Iterm2, &col, hint)?)
        }
        Some(Kind::Json) => {
            let doc = parse_jsonc(&t)?;
            let Some(o) = doc.as_object() else { return unknown() };
            if o.get("format").and_then(Value::as_str) == Some("tinhat-theme") {
                return Ok(ImportResult { format: ImportFormat::Tinhat, theme: doc });
            }
            let scheme = match o.get("schemes") {
                Some(Value::Array(a)) => a.iter().find_map(Value::as_object),
                _ => Some(o),
            };
            let Some(scheme) = scheme.filter(|s| s.contains_key("background") || s.contains_key("black")) else {
                return unknown();
            };
            from_windows_terminal(scheme, &mut col);
            (ImportFormat::WindowsTerminal, build(ImportFormat::WindowsTerminal, &col, hint)?)
        }
        Some(Kind::Toml) => {
            let doc = parse_toml(&t)?;
            let c = doc.get(doc.root, "colors").filter(|&c| doc.is_obj(c));
            let Some(c) = c else { return unknown() };
            let is_obj_at = |k: &str| doc.get(c, k).is_some_and(|id| doc.is_obj(id));
            if is_obj_at("primary") || is_obj_at("normal") || is_obj_at("bright") {
                from_alacritty(&doc, &mut col);
                (ImportFormat::AlacrittyToml, build(ImportFormat::AlacrittyToml, &col, hint)?)
            } else if ["ansi", "brights", "cursor_bg", "selection_bg"].iter().any(|k| doc.get(c, k).is_some()) {
                from_wezterm(&doc, &mut col);
                (ImportFormat::Wezterm, build(ImportFormat::Wezterm, &col, hint)?)
            } else {
                return unknown();
            }
        }
        Some(Kind::Base) => {
            let (format, col) = from_base(&parse_yaml(&t)?)?;
            (format, build(format, &col, hint)?)
        }
        Some(Kind::AlacrittyYaml) => {
            let doc = parse_yaml(&t)?;
            if !doc.get(doc.root, "colors").is_some_and(|c| doc.is_obj(c)) {
                return unknown();
            }
            from_alacritty(&doc, &mut col);
            (ImportFormat::AlacrittyYaml, build(ImportFormat::AlacrittyYaml, &col, hint)?)
        }
        Some(Kind::Ghostty) => {
            from_ghostty(&t, &mut col);
            (ImportFormat::Ghostty, build(ImportFormat::Ghostty, &col, hint)?)
        }
        Some(Kind::Kitty) => {
            from_kitty(&t, &mut col);
            (ImportFormat::Kitty, build(ImportFormat::Kitty, &col, hint)?)
        }
        Some(Kind::Xresources) => {
            from_xresources(&t, &mut col);
            (ImportFormat::Xresources, build(ImportFormat::Xresources, &col, hint)?)
        }
        None => return unknown(),
    };
    Ok(ImportResult { format, theme })
}

/// ENGINE.md §7: `importTheme(text, filenameHint?)`. Never panics on bad input.
pub fn import_theme(text: &str, filename_hint: Option<&str>) -> Result<ImportResult, ImportError> {
    run(text, filename_hint)
}

/// Every string of an imported theme obeys the engine's limits (the library build's self-check).
pub fn within_limits(theme: &Value) -> bool {
    ["id", "name", "author", "source"].iter().all(|k| {
        let s = theme.get(k).and_then(Value::as_str).unwrap_or("");
        s.chars().count() <= MAX_STRING && !contains_url(s)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fold_table_has_192_entries() {
        assert_eq!(FOLD_C0.split(' ').count(), 0x180 - 0xc0);
    }

    #[test]
    fn detection_regexes() {
        for (l, want) in [
            ("[colors.primary]", true),
            ("[[colors.indexed_colors]]", true),
            ("  [ \"a b\" . 'c' ]  # x", true),
            ("[  ]", true),
            ("[\t]", false),
            ("[\t a \t]", true),
            ("[a\tb]", false),
            ("[a] x", false),
            ("[a]]]", false),
            ("[[[a]", false),
            ("[a]\u{2028}", false),
            ("[a] #\u{2028}", false),
            ("[é]", false),
        ] {
            assert_eq!(re_toml_header(l), want, "{l:?}");
        }
        assert!(re_toml_colors("colors.primary.background = \"#000\""));
        assert!(!re_toml_colors("colors. = 1"));
        assert!(re_base("  'base0A' : x") && re_base("base0f:") && !re_base("base0g: x") && !re_base("base10: x"));
        assert!(re_alacritty_yaml("colors:  # x") && !re_alacritty_yaml(" colors:") && !re_alacritty_yaml("colors: x"));
        assert!(re_ghostty("palette = 0=#000") && re_ghostty("  cursor-color=#fff") && !re_ghostty("cursor = #fff"));
        for (l, want) in [
            ("color0 #000", true),
            ("color255\t#000", true),
            ("color1234 #000", false),
            ("color #000", false),
            ("cursor none", true),
            ("cursor_text_color background", false),
            ("foreground\u{a0}#000", false),
            ("foreground  \u{a0}", false),
            ("background 🎨", true),
        ] {
            assert_eq!(re_kitty(l), want, "{l:?}");
        }
        for (l, want) in [
            ("*color0: #000", true),
            ("URxvt*background:#000", true),
            ("*.cursorColor :x", true),
            ("color12:", true),
            ("xcolor1234:", false),
            ("*colorX:", false),
            ("foreground-x:", false),
            ("*color7 x:", false),
        ] {
            assert_eq!(re_xres(l), want, "{l:?}");
        }
        assert!(re_xrdb("#define Ansi_0_Color #000") && re_xrdb(" # define\tBackground_Color\t#000"));
        assert!(!re_xrdb("#define Ansi__Color #000") && !re_xrdb("#defineAnsi_0_Color #000") && !re_xrdb("#define Ansi_0_Color"));
        assert_eq!(re_define("#define  X  a b "), Some(("X", "a b ")));
        assert_eq!(re_define("# define X\u{a0}y"), None);
        assert_eq!(re_define("#define X"), None);
        assert_eq!(re_kitty_meta("## NAME : Foo"), Some((true, " Foo")));
        assert_eq!(re_kitty_meta("##author:"), Some((false, "")));
        assert_eq!(re_kitty_meta("## names: x"), None);
        assert_eq!(re_ghostty_palette("12 = #fff"), Some((12, " #fff")));
        assert_eq!(re_ghostty_palette("1234=#fff"), None);
        assert!(has_plist_tag("<plist version=\"1.0\">") && has_plist_tag("<plist>") && !has_plist_tag("<plistx>"));
        assert!(is_plist_number("1") && is_plist_number("-0.5") && is_plist_number(".5") && is_plist_number("2E-1"));
        assert!(is_plist_number("1.") && !is_plist_number(".") && !is_plist_number("1e") && !is_plist_number(" 1"));
    }
}
