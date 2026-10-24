//! share.rs — port of `share.ts` (contract THEMES-UI.md §14): theme share cards — the `m.room.message` a "Share to a
//! room…" sends, reading one back (a card, a refusal, a newer card, or not a card at all), and what Keep does. The
//! web's `src/theme/share.ts` is the REFERENCE, ported line by line; both run the shared vectors `testvectors/share/`
//! byte for byte (tests/editor.rs).
//!
//! Pure: no Qt, no I/O. A card carries DATA ONLY — one `tinhat-theme` v1 document, which the receiver checks with the
//! engine exactly like a theme file (size, strings, no URLs except an mxc backdrop, expressions, fonts from the allowed
//! list); the receiver computes the swatches and the variant itself and never trusts anything else the sender wrote.
//! Other apps (Element X, Element Web) show `body`: one plain line of text. The app's entry points that take and return
//! JSON text: [`share_content_json`], [`read_share_json`]; Keep: [`keep_plan`] with the app's library lookup.

use std::collections::HashSet;

use serde_json::{json, Value};

use crate::canon::canon_json;
use crate::editor::trim_ascii;
use crate::engine::{app_tokens, parse_theme_json, resolve_theme, TokensSpec, Variant};
use crate::import::slugify;
use crate::json::{parse_json, within_nesting, MAX_NESTING};
use crate::limits::{code_points, contains_url, is_theme_id};
use crate::sync::{unique_theme_id, JObj};

/// The content key that makes an `m.room.message` a theme card.
pub const SHARE_KEY: &str = "dev.tinhat.theme.share";
/// The share object's `format`.
pub const SHARE_FORMAT: &str = "tinhat-theme-share";
/// The share object's version (`v`): a higher one is a newer card, anything else not 1 a bad one.
pub const SHARE_V: i64 = 1;
/// Canonical UTF-8 size of the share object (a theme is ≤ 8,192 bytes, ENGINE.md §4; the rest is its envelope).
pub const SHARE_MAX_BYTES: usize = 12_288;
/// The roles the card's swatch strip shows, left to right.
pub const SWATCH_ROLES: [&str; 10] = [
    "surface.window",
    "surface.island",
    "surface.raised",
    "text.secondary",
    "text.primary",
    "accent.base",
    "accent.bright",
    "state.success",
    "state.warning",
    "state.danger",
];
/// How long Try shows a shared theme before switching back (ms).
pub const TRY_MS: u64 = 30_000;

/// A display string from a theme field: a string of 1–80 code points (trimmed of space / tab / CR / LF) with no URL,
/// else none.
fn shown_text(v: Option<&Value>) -> Option<String> {
    let Some(Value::String(s)) = v else { return None };
    let t = trim_ascii(s);
    (!t.is_empty() && code_points(t) <= 80 && !contains_url(t)).then(|| t.to_string())
}

/// The plain line other apps show: `🎨 Nord by Arctic Ice Studio — a Tin Hat theme. Open it in Tin Hat to try it.`
pub fn share_body(name: &str, author: Option<&str>) -> String {
    let by = match author {
        Some(a) if !a.is_empty() => format!(" by {a}"),
        _ => String::new(),
    };
    format!("🎨 {name}{by} — a Tin Hat theme. Open it in Tin Hat to try it.")
}

/// The message content that shares `theme` (§14.1): `{msgtype: "m.text", body, "m.mentions": {}, [SHARE_KEY]:
/// {format, v, theme}}`. Refused when the theme does not resolve (`refused: <engine error>`) or the share object is
/// larger than [`SHARE_MAX_BYTES`] (`too-large`). The theme travels as it is (its own id and name).
pub fn share_content(theme: &Value, spec: &TokensSpec) -> Result<JObj, String> {
    if let Err(e) = resolve_theme(theme, spec) {
        return Err(format!("refused: {e}"));
    }
    let t = theme.clone();
    let mut share = JObj::new();
    share.insert("format".into(), json!(SHARE_FORMAT));
    share.insert("v".into(), json!(SHARE_V));
    share.insert("theme".into(), t.clone());
    let share = Value::Object(share);
    if canon_json(&share).len() > SHARE_MAX_BYTES {
        return Err("too-large".into());
    }
    let name = shown_text(t.get("name")).unwrap_or_else(|| "A theme".into());
    let author = shown_text(t.get("author"));
    let mut content = JObj::new();
    content.insert("msgtype".into(), json!("m.text"));
    content.insert("body".into(), Value::String(share_body(&name, author.as_deref())));
    content.insert("m.mentions".into(), Value::Object(JObj::new()));
    content.insert(SHARE_KEY.into(), share);
    Ok(content)
}

/// What a message is (§14.2).
#[derive(Clone, Debug, PartialEq)]
pub enum ShareRead {
    /// Not a theme card: render the message as usual.
    None,
    /// A card of a later version (`v` > 1).
    Newer { name: Option<String> },
    /// A card that cannot be used — `bad-card`, `too-large` or the engine's refusal of the theme — shown as such,
    /// never applied.
    Refused { error: String, name: Option<String> },
    /// The theme, its name / author / variant and the swatch colours ([`SWATCH_ROLES`], canonical), all computed here.
    Ok { theme: JObj, name: String, author: Option<String>, variant: Variant, swatches: Vec<String> },
}

impl ShareRead {
    /// `none` / `newer` / `refused` / `ok`.
    pub fn kind(&self) -> &'static str {
        match self {
            ShareRead::None => "none",
            ShareRead::Newer { .. } => "newer",
            ShareRead::Refused { .. } => "refused",
            ShareRead::Ok { .. } => "ok",
        }
    }

    /// The reference's object: `{kind: "none"}`, `{kind: "newer", name}`, `{kind: "refused", error, name}`,
    /// `{kind: "ok", theme, name, author, variant, swatches}` (`null` for a missing name / author).
    pub fn to_json(&self) -> Value {
        match self {
            ShareRead::None => json!({ "kind": "none" }),
            ShareRead::Newer { name } => json!({ "kind": "newer", "name": name }),
            ShareRead::Refused { error, name } => json!({ "kind": "refused", "error": error, "name": name }),
            ShareRead::Ok { theme, name, author, variant, swatches } => json!({
                "kind": "ok",
                "theme": Value::Object(theme.clone()),
                "name": name,
                "author": author,
                "variant": variant.as_str(),
                "swatches": swatches,
            }),
        }
    }
}

/// Read a message's content (§14.2). `None`: not a theme card. `Newer`: a card of a later version (`v` > 1).
/// `Refused`: a card that cannot be used — `bad-card` (not a v1 share object, no theme object, not JSON data),
/// `too-large`, or the engine's refusal of the theme. `Ok`: the theme, its name / author / variant and the swatches.
pub fn read_share(content: &Value, spec: &TokensSpec) -> ShareRead {
    let Some(content) = content.as_object() else { return ShareRead::None };
    let Some(Value::Object(share)) = content.get(SHARE_KEY) else { return ShareRead::None };
    if share.get("format").and_then(Value::as_str) != Some(SHARE_FORMAT) {
        return ShareRead::None;
    }
    let theme = share.get("theme");
    let name = match theme {
        Some(Value::Object(t)) => shown_text(t.get("name")),
        _ => None,
    };
    let refused = |error: &str, name: Option<String>| ShareRead::Refused { error: error.into(), name };
    let Some(v) = share.get("v").and_then(Value::as_number).and_then(|n| n.as_f64()) else {
        return refused("bad-card", name);
    };
    if v > SHARE_V as f64 {
        return ShareRead::Newer { name };
    }
    if v != SHARE_V as f64 {
        return refused("bad-card", name);
    }
    let Some(Value::Object(theme)) = theme else { return refused("bad-card", name) };
    // isJsonData(share): nesting ≤ 128 (a Rust string never holds a lone surrogate)
    let share_value = Value::Object(share.clone());
    if !within_nesting(&share_value, MAX_NESTING) {
        return refused("bad-card", name);
    }
    if canon_json(&share_value).len() > SHARE_MAX_BYTES {
        return refused("too-large", name);
    }
    let r = match resolve_theme(&Value::Object(theme.clone()), spec) {
        Ok(r) => r,
        Err(e) => return refused(&e.to_string(), name),
    };
    let swatches = SWATCH_ROLES.iter().map(|id| r.roles.get(*id).cloned().unwrap_or_default()).collect();
    ShareRead::Ok {
        theme: theme.clone(),
        name: name.unwrap_or_else(|| "A theme".into()),
        author: shown_text(theme.get("author")),
        variant: r.variant,
        swatches,
    }
}

/// A theme document without its `id` (what "the same theme" means for Keep), canonical.
fn without_id(t: &JObj) -> String {
    let mut c = t.clone();
    c.remove("id");
    canon_json(&Value::Object(c))
}

/// What Keep does with a card's theme (§14.4).
#[derive(Clone, Debug, PartialEq)]
pub enum KeepPlan {
    /// A library theme exactly as shipped: use it by id (nothing is saved).
    Library { id: String },
    /// The same theme as one of My themes: use that one.
    Mine { id: String },
    /// Save this document as a new My theme and use it.
    Save { doc: JObj },
}

impl KeepPlan {
    /// `{kind: "library", id}` / `{kind: "mine", id}` / `{kind: "save", doc}`.
    pub fn to_json(&self) -> Value {
        match self {
            KeepPlan::Library { id } => json!({ "kind": "library", "id": id }),
            KeepPlan::Mine { id } => json!({ "kind": "mine", "id": id }),
            KeepPlan::Save { doc } => json!({ "kind": "save", "doc": Value::Object(doc.clone()) }),
        }
    }
}

fn id_of(m: &JObj) -> &str {
    m.get("id").and_then(Value::as_str).unwrap_or_default()
}

/// What Keep does with a card's theme (§14.4): a library theme exactly as shipped → use it by id (nothing is saved);
/// the same theme as one of My themes (compared without `id`) → use that one; else save it as a new My theme — its id
/// = slugify(name) made unique against the library and My themes (THEMES-SYNC §4.1) — and use it. `library(id)` = the
/// built-in library's document (the app: `theme_lib::doc`); `mine` in any order (the first equal one in id order wins).
pub fn keep_plan<'a>(theme: &JObj, library: impl Fn(&str) -> Option<&'a Value>, mine: &[JObj]) -> KeepPlan {
    if let Some(Value::String(id)) = theme.get("id") {
        if is_theme_id(id) {
            if let Some(lib) = library(id) {
                if canon_json(lib) == canon_json(&Value::Object(theme.clone())) {
                    return KeepPlan::Library { id: id.clone() };
                }
            }
        }
    }
    let key = without_id(theme);
    let mut sorted: Vec<&JObj> = mine.iter().filter(|m| matches!(m.get("id"), Some(Value::String(_)))).collect();
    sorted.sort_by(|a, b| id_of(a).cmp(id_of(b))); // stable, code-point order
    for m in &sorted {
        if without_id(m) == key {
            return KeepPlan::Mine { id: id_of(m).to_string() };
        }
    }
    let taken: HashSet<&str> = sorted.iter().map(|m| id_of(m)).collect();
    let name = match theme.get("name") {
        Some(Value::String(n)) => n.as_str(),
        _ => "",
    };
    let id = unique_theme_id(&slugify(name), |x| taken.contains(x) || library(x).is_some());
    let mut doc = theme.clone();
    doc.insert("id".into(), Value::String(id));
    KeepPlan::Save { doc }
}

// ---- the app's JSON-text entry points (app tokens: the embedded tokens.json) --------------------------------------

/// [`share_content`] of a theme given as JSON text, with the app's token list: `{"ok":true,"content":{…}}` /
/// `{"ok":false,"error":…}` as canonical JSON (text that is not a JSON value: `refused: bad-theme: not JSON data`).
pub fn share_content_json(theme_json: &str) -> String {
    let r = parse_theme_json(theme_json)
        .map_err(|e| format!("refused: {e}"))
        .and_then(|theme| share_content(&theme, app_tokens()));
    canon_json(&match r {
        Ok(content) => json!({ "ok": true, "content": Value::Object(content) }),
        Err(error) => json!({ "ok": false, "error": error }),
    })
}

/// [`read_share`] of a message content given as JSON text, with the app's token list: [`ShareRead::to_json`] as
/// canonical JSON (text that is not JSON is not a card: `{"kind":"none"}`).
pub fn read_share_json(content_json: &str) -> String {
    let content = parse_json(content_json).unwrap_or(Value::Null);
    canon_json(&read_share(&content, app_tokens()).to_json())
}
