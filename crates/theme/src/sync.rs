//! sync.rs — port of `sync.ts` (contract THEMES-SYNC.md): the two account-data documents `dev.tinhat.theme` (the
//! choice) and `dev.tinhat.themes` (My themes) — reading, saving, merging — and the device protocol as a
//! deterministic state machine. The web's `src/theme/sync.ts` is the REFERENCE; this port reproduces the shared
//! vectors (`testvectors/themes-sync/`) byte for byte (`tests/sync.rs`).
//!
//! Pure: no Qt, no clock, no I/O — `now` and the device id are arguments. Values are `serde_json::Value`s read with
//! the crate's JSON reader (`parse_json`: JavaScript's number semantics, own keys only); canonical forms and sizes are
//! [`canon_json`] (the reference's `canonJson`). Where the reference relies on a JavaScript detail the port spells it
//! out: `===` on JSON values ([`strict_eq`]: objects never equal), `Number.isInteger`, `Math.max`/`Math.min` with NaN
//! (a missing `now` makes a NaN stamp, printed `null`), string lengths in UTF-16 code units.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::canon::canon_json;
use crate::json::{number_value, within_nesting, MAX_NESTING};
use crate::limits::is_theme_id;

pub const CHOICE_TYPE: &str = "dev.tinhat.theme";
pub const THEMES_TYPE: &str = "dev.tinhat.themes";
pub const CHOICE_FORMAT: &str = "tinhat-theme-choice";
pub const THEMES_FORMAT: &str = "tinhat-my-themes";
pub const MAX_THEMES: usize = 50;
pub const MAX_TOMBSTONES: usize = 64;
pub const MAX_DOC_BYTES: usize = 65_536;
pub const MAX_THEME_BYTES: usize = 8_192;
pub const MAX_REV: f64 = 2_147_483_647.0;
pub const MAX_SAVED_AT: f64 = 9_007_199_254_740_991.0;
pub const SOUND_PACKS: [&str; 2] = ["default", "goose"];
pub const ICON_STYLES: [&str; 2] = ["duotone", "mono"];
/// The content keys a save sets (THEMES-SYNC §3.1); every other content key is an unknown one and is carried.
pub const PATCH_KEYS: [&str; 5] = ["theme", "doc", "overrides", "sounds", "icons"];
const CHOICE_META: [&str; 6] = ["format", "v", "savedAt", "rev", "by", "previous"];
const ENTRY_KNOWN_KEYS: [&str; 7] = ["id", "savedAt", "rev", "by", "theme", "deleted", "previous"];

/// A JSON object.
pub type JObj = Map<String, Value>;

// ---- JSON data --------------------------------------------------------------------------------------------------

pub fn is_obj(v: Option<&Value>) -> bool {
    matches!(v, Some(Value::Object(_)))
}

fn obj_of(v: Option<&Value>) -> Option<&JObj> {
    match v {
        Some(Value::Object(o)) => Some(o),
        _ => None,
    }
}

/// `typeof v === "number"` → the double.
fn num(v: Option<&Value>) -> Option<f64> {
    match v {
        Some(Value::Number(n)) => n.as_f64(),
        _ => None,
    }
}

/// `Number.isInteger(v)` → the double.
fn int_of(v: Option<&Value>) -> Option<f64> {
    num(v).filter(|x| x.is_finite() && x.fract() == 0.0)
}

/// JavaScript `===` of two JSON values (`None` = undefined): objects and arrays are never equal (they are distinct
/// objects in the reference: a patch and a document are always separate copies).
pub fn strict_eq(a: Option<&Value>, b: Option<&Value>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(Value::Null), Some(Value::Null)) => true,
        (Some(Value::Bool(x)), Some(Value::Bool(y))) => x == y,
        (Some(Value::Number(x)), Some(Value::Number(y))) => x.as_f64() == y.as_f64(),
        (Some(Value::String(x)), Some(Value::String(y))) => x == y,
        _ => false,
    }
}

/// `Math.max` / `Math.min` (NaN in → NaN out; Rust's `f64::max` ignores a NaN).
fn js_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else if a > b {
        a
    } else {
        b
    }
}
fn js_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else if a < b {
        a
    } else {
        b
    }
}

/// Account-data content as the contract reads it (ENGINE.md §9): JSON data only (nesting ≤ 128; a Rust string has no
/// lone surrogate). `None` = undefined.
pub fn as_data(v: Option<&Value>) -> Option<Value> {
    let v = v?;
    within_nesting(v, MAX_NESTING).then(|| v.clone())
}

/// The canonical text (`""` for undefined, like the reference's `canon`).
pub fn canon(v: Option<&Value>) -> String {
    v.map(canon_json).unwrap_or_default()
}

pub fn same_json(a: Option<&Value>, b: Option<&Value>) -> bool {
    canon(a) == canon(b)
}

fn canon_obj(o: &JObj) -> String {
    canon_json(&Value::Object(o.clone()))
}

/// UTF-8 bytes of the canonical form.
pub fn bytes_of(v: &Value) -> usize {
    canon_json(v).len()
}

fn obj_bytes(o: &JObj) -> usize {
    canon_obj(o).len()
}

/// `^[a-z0-9-]{1,32}$`
pub fn is_pack_id(v: Option<&Value>) -> bool {
    match v {
        Some(Value::String(s)) => {
            !s.is_empty() && s.len() <= 32 && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        }
        _ => false,
    }
}

fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

// ---- the order key (§2) -----------------------------------------------------------------------------------------

/// `(savedAt, rev, by)`; an invalid field reads as 0 / 0 / "".
#[derive(Clone, Debug, PartialEq)]
pub struct OrderKey {
    pub saved_at: f64,
    pub rev: f64,
    pub by: String,
}

pub fn order_key(e: &JObj) -> OrderKey {
    let at = int_of(e.get("savedAt")).filter(|x| *x >= 0.0 && *x <= MAX_SAVED_AT).unwrap_or(0.0);
    let rev = int_of(e.get("rev")).filter(|x| *x >= 0.0 && *x <= MAX_REV).unwrap_or(0.0);
    let by = match e.get("by") {
        Some(Value::String(s)) if utf16_len(s) <= 64 => s.clone(),
        _ => String::new(),
    };
    OrderKey { saved_at: at, rev, by }
}

fn cmp_f64(a: f64, b: f64) -> Ordering {
    a.partial_cmp(&b).unwrap_or(Ordering::Equal)
}

pub fn cmp_key(a: &JObj, b: &JObj) -> Ordering {
    let (x, y) = (order_key(a), order_key(b));
    cmp_f64(x.saved_at, y.saved_at).then(cmp_f64(x.rev, y.rev)).then_with(|| x.by.as_str().cmp(y.by.as_str()))
}

/// The total order: the key, then the canonical text (code-point order = UTF-8 byte order).
pub fn cmp_doc(a: &JObj, b: &JObj) -> Ordering {
    cmp_key(a, b).then_with(|| canon_obj(a).cmp(&canon_obj(b)))
}

/// The higher of two documents (`a` when they are equal).
pub fn higher(a: Option<JObj>, b: Option<JObj>) -> Option<JObj> {
    match (a, b) {
        (None, b) => b,
        (a, None) => a,
        (Some(a), Some(b)) => Some(if cmp_doc(&a, &b) != Ordering::Less { a } else { b }),
    }
}

/// The stamps of a save based on `base` (§2): `(savedAt, rev, by)` as JSON values (a NaN `now` → `null`).
pub fn stamps(base: Option<&JObj>, now: f64, by: &str) -> (Value, Value, Value) {
    let (at, rev) = match base {
        None => (js_min(js_max(0.0, now), MAX_SAVED_AT), 1.0),
        Some(b) => {
            let k = order_key(b);
            (js_min(js_max(now, k.saved_at + 1.0), MAX_SAVED_AT), js_min(k.rev + 1.0, MAX_REV))
        }
    };
    (number_value(at), number_value(rev), Value::String(by.to_string()))
}

fn set_stamps(o: &mut JObj, s: (Value, Value, Value)) {
    o.insert("savedAt".into(), s.0);
    o.insert("rev".into(), s.1);
    o.insert("by".into(), s.2);
}

fn fill_key(o: &mut JObj) {
    let k = order_key(o);
    o.insert("savedAt".into(), number_value(k.saved_at));
    o.insert("rev".into(), number_value(k.rev));
    o.insert("by".into(), Value::String(k.by));
}

/// `ignore` | `newer` (render best-effort, never write) | `ok`.
#[derive(Clone, Debug, PartialEq)]
pub enum Read {
    Ignore,
    Newer(JObj),
    Ok(JObj),
}

impl Read {
    pub fn kind(&self) -> &'static str {
        match self {
            Read::Ignore => "ignore",
            Read::Newer(_) => "newer",
            Read::Ok(_) => "ok",
        }
    }
    pub fn doc(&self) -> Option<&JObj> {
        match self {
            Read::Ignore => None,
            Read::Newer(d) | Read::Ok(d) => Some(d),
        }
    }
    fn ok_doc(self) -> Option<JObj> {
        match self {
            Read::Ok(d) => Some(d),
            _ => None,
        }
    }
}

// ---- dev.tinhat.theme (§3) -------------------------------------------------------------------------------------

/// §3.2: ignore | newer | ok — the document is a defaults-filled copy.
pub fn read_choice(value: Option<&Value>) -> Read {
    let Some(Value::Object(v)) = as_data(value) else { return Read::Ignore };
    if v.get("format").and_then(Value::as_str) != Some(CHOICE_FORMAT) {
        return Read::Ignore;
    }
    let Some(ver) = num(v.get("v")) else { return Read::Ignore };
    if ver > 1.0 {
        return Read::Newer(fill_choice(&v));
    }
    if ver != 1.0 {
        return Read::Ignore;
    }
    match v.get("theme") {
        Some(Value::String(t)) if is_theme_id(t) => Read::Ok(fill_choice(&v)),
        _ => Read::Ignore,
    }
}

fn fill_choice(v: &JObj) -> JObj {
    let mut d = v.clone();
    fill_key(&mut d);
    if d.get("previous").is_some_and(|p| !p.is_object()) {
        d.remove("previous");
    }
    d
}

/// The content of a choice (§3.1): every key but format, v, savedAt, rev, by, previous.
pub fn choice_content(doc: &JObj) -> JObj {
    doc.iter().filter(|(k, _)| !CHOICE_META.contains(&k.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect()
}

/// `{entry}` | `{noop: true}` | `{error}`.
#[derive(Clone, Debug, PartialEq)]
pub enum SaveChoice {
    Entry(JObj),
    Noop,
    Error(&'static str),
}

impl SaveChoice {
    /// The reference's result object (the `expect` of a choice-save vector).
    pub fn to_json(&self) -> Value {
        let mut o = JObj::new();
        match self {
            SaveChoice::Entry(e) => o.insert("entry".into(), Value::Object(e.clone())),
            SaveChoice::Noop => o.insert("noop".into(), Value::Bool(true)),
            SaveChoice::Error(e) => o.insert("error".into(), Value::String((*e).into())),
        };
        Value::Object(o)
    }
}

/// §3.3 — a new choice document from `patch` over the current one (or over `fallback` = what the device renders).
pub fn save_choice(
    raw: Option<&Value>,
    known: Option<&JObj>,
    patch: Option<&Value>,
    fallback: Option<&Value>,
    now: f64,
    by: &str,
) -> SaveChoice {
    let r = read_choice(raw);
    if matches!(r, Read::Newer(_)) {
        return SaveChoice::Error("newer-format");
    }
    let k = match known {
        Some(k) => read_choice(Some(&Value::Object(k.clone()))),
        None => Read::Ignore,
    };
    let base = higher(k.ok_doc(), r.ok_doc());
    let p = match as_data(patch) {
        Some(Value::Object(p)) if p.keys().all(|key| PATCH_KEYS.contains(&key.as_str())) => p,
        _ => return SaveChoice::Error("bad-request"),
    };
    let start: JObj = match (&base, obj_of(fallback)) {
        (Some(b), _) => choice_content(b),
        (None, Some(f)) => choice_content(f),
        (None, None) => JObj::new(),
    };
    let mut next = start.clone();
    if p.contains_key("theme") && !strict_eq(p.get("theme"), start.get("theme")) {
        if !p.contains_key("doc") {
            next.remove("doc");
        }
        if !p.contains_key("overrides") {
            next.remove("overrides");
        }
    }
    for key in PATCH_KEYS {
        match p.get(key) {
            None => {}
            Some(Value::Null) => {
                next.remove(key);
            }
            Some(v) => {
                next.insert(key.into(), v.clone());
            }
        }
    }
    if let Some(bad) = check_content(&next) {
        return SaveChoice::Error(bad);
    }
    if canon_obj(&next) == canon_obj(&start) {
        return SaveChoice::Noop;
    }
    let mut entry = next;
    entry.insert("format".into(), Value::String(CHOICE_FORMAT.into()));
    entry.insert("v".into(), Value::from(1));
    set_stamps(&mut entry, stamps(base.as_ref(), now, by));
    if let Some(b) = &base {
        let mut prev = b.clone();
        prev.remove("previous");
        prev.remove("format");
        prev.remove("v");
        entry.insert("previous".into(), Value::Object(prev));
    }
    if obj_bytes(&entry) > MAX_DOC_BYTES {
        entry.remove("previous");
        if obj_bytes(&entry) > MAX_DOC_BYTES {
            return SaveChoice::Error("too-large");
        }
    }
    SaveChoice::Entry(entry)
}

fn check_content(c: &JObj) -> Option<&'static str> {
    let theme = match c.get("theme") {
        Some(Value::String(t)) if is_theme_id(t) => t,
        _ => return Some("bad-request"),
    };
    let doc = c.get("doc");
    if let Some(d) = doc {
        match d {
            Value::Object(o) if o.get("id").and_then(Value::as_str) == Some(theme.as_str()) => {}
            _ => return Some("bad-request"),
        }
    }
    if let Some(ov) = c.get("overrides") {
        let Value::Object(ov) = ov else { return Some("bad-request") };
        for part in ["roles", "tokens"] {
            match ov.get(part) {
                None => {}
                Some(Value::Object(m)) if m.values().all(Value::is_string) => {}
                Some(_) => return Some("bad-request"),
            }
        }
    }
    for key in ["sounds", "icons"] {
        if let Some(v) = c.get(key) {
            if !is_pack_id(Some(v)) {
                return Some("bad-request");
            }
        }
    }
    if doc.is_some_and(|d| bytes_of(d) > MAX_THEME_BYTES) {
        return Some("theme-too-large");
    }
    None
}

/// Where the rendered theme came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChoiceSource {
    Doc,
    Library,
    None,
}

impl ChoiceSource {
    pub fn as_str(self) -> &'static str {
        match self {
            ChoiceSource::Doc => "doc",
            ChoiceSource::Library => "library",
            ChoiceSource::None => "none",
        }
    }
}

/// What happened to the choice's overrides.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Overrides {
    Applied,
    None,
    Refused,
}

impl Overrides {
    pub fn as_str(self) -> &'static str {
        match self {
            Overrides::Applied => "applied",
            Overrides::None => "none",
            Overrides::Refused => "refused",
        }
    }
}

/// §3.4 — what a device renders.
#[derive(Clone, Debug, PartialEq)]
pub struct ChoiceProjection {
    pub source: ChoiceSource,
    /// the theme document to resolve (overrides laid over when `overrides` = applied); None when source = none
    pub theme: Option<JObj>,
    pub theme_id: Option<String>,
    pub overrides: Overrides,
    /// `default` | `goose`
    pub sounds: &'static str,
    /// `duotone` | `mono`
    pub icons: &'static str,
    /// the pack ids as stored (an unknown id is kept on the next save and rendered as the default)
    pub sounds_raw: Option<String>,
    pub icons_raw: Option<String>,
}

/// §3.4 — what a device renders for a choice (ok or newer) or any content object (a local choice, the legacy choice).
/// `library(id)` = the built-in library's document; `resolves(theme)` = the engine accepts it.
pub fn project_choice<'a>(
    content: &JObj,
    library: impl Fn(&str) -> Option<&'a Value>,
    resolves: impl Fn(&JObj) -> bool,
) -> ChoiceProjection {
    let theme_id = match content.get("theme") {
        Some(Value::String(t)) if is_theme_id(t) => Some(t.clone()),
        _ => None,
    };
    let pack = |k: &str| if is_pack_id(content.get(k)) { content.get(k).and_then(Value::as_str).map(str::to_string) } else { None };
    let sounds_raw = pack("sounds");
    let icons_raw = pack("icons");
    let mut out = ChoiceProjection {
        source: ChoiceSource::None,
        theme: None,
        theme_id: theme_id.clone(),
        overrides: Overrides::None,
        sounds: if sounds_raw.as_deref() == Some("goose") { "goose" } else { "default" },
        icons: if icons_raw.as_deref() == Some("mono") { "mono" } else { "duotone" },
        sounds_raw,
        icons_raw,
    };
    let base: Option<JObj> = match content.get("doc") {
        Some(Value::Object(d)) => {
            out.source = ChoiceSource::Doc;
            Some(d.clone())
        }
        _ => match theme_id.as_deref().and_then(&library) {
            Some(Value::Object(d)) => {
                out.source = ChoiceSource::Library;
                Some(d.clone())
            }
            _ => None,
        },
    };
    let Some(base) = base else { return out };
    let layered = match content.get("overrides") {
        Some(Value::Object(ov)) => lay_overrides(&base, ov),
        _ => None,
    };
    if let Some(l) = &layered {
        if resolves(l) {
            out.theme = layered;
            out.overrides = Overrides::Applied;
            return out;
        }
    }
    let refused = if layered.is_some() { Overrides::Refused } else { Overrides::None };
    if resolves(&base) {
        out.theme = Some(base);
        out.overrides = refused;
        return out;
    }
    out.source = ChoiceSource::None;
    out.overrides = refused;
    out
}

/// The theme with `overrides.roles` / `.tokens` string values laid over its own (None when there is nothing to lay).
pub fn lay_overrides(theme: &JObj, ov: &JObj) -> Option<JObj> {
    let mut any = false;
    let mut t = theme.clone();
    for part in ["roles", "tokens"] {
        let Some(Value::Object(m)) = ov.get(part) else { continue };
        let mut into = match t.get(part) {
            Some(Value::Object(own)) => own.clone(),
            _ => JObj::new(),
        };
        for (k, v) in m {
            if let Value::String(_) = v {
                into.insert(k.clone(), v.clone());
                any = true;
            }
        }
        t.insert(part.into(), Value::Object(into));
    }
    any.then_some(t)
}

// ---- dev.tinhat.themes (§4) ------------------------------------------------------------------------------------

pub fn is_live(e: &JObj) -> bool {
    e.get("deleted") != Some(&Value::Bool(true))
}

pub fn entry_id(e: Option<&Value>) -> Option<String> {
    match obj_of(e)?.get("id") {
        Some(Value::String(id)) if is_theme_id(id) => Some(id.clone()),
        _ => None,
    }
}

/// §4.2 defaults filled (a copy).
pub fn fill_entry(e: &JObj) -> JObj {
    let mut d = e.clone();
    fill_key(&mut d);
    if d.get("deleted") == Some(&Value::Bool(true)) {
        d.remove("theme");
        d.remove("previous");
        return d;
    }
    d.remove("deleted");
    let drop = match d.get_mut("previous") {
        None => false,
        Some(Value::Object(prev)) if prev.get("theme").is_some_and(Value::is_object) => {
            prev.remove("previous");
            false
        }
        Some(_) => true,
    };
    if drop {
        d.remove("previous");
    }
    d
}

/// §4.3: ignore | newer | ok (the document itself, not filled).
pub fn read_themes(value: Option<&Value>) -> Read {
    let Some(Value::Object(v)) = as_data(value) else { return Read::Ignore };
    if v.get("format").and_then(Value::as_str) != Some(THEMES_FORMAT) {
        return Read::Ignore;
    }
    let Some(ver) = num(v.get("v")) else { return Read::Ignore };
    if ver > 1.0 {
        return Read::Newer(v);
    }
    if ver != 1.0 {
        return Read::Ignore;
    }
    Read::Ok(v)
}

/// The entries of a document (any object's `themes` array): valid ids, defaults filled, the higher per repeated id.
pub fn entries_of(doc: Option<&Value>) -> BTreeMap<String, JObj> {
    let mut out: BTreeMap<String, JObj> = BTreeMap::new();
    let Some(Value::Array(arr)) = obj_of(doc).and_then(|d| d.get("themes")) else { return out };
    for e in arr {
        let Some(id) = entry_id(Some(e)) else { continue };
        let filled = fill_entry(e.as_object().expect("entry_id checked it"));
        let best = higher(out.remove(&id), Some(filled)).expect("one side is set");
        out.insert(id, best);
    }
    out
}

/// §4.3 the list the UI shows: live entries whose theme object carries the entry's id, newest first.
pub fn list_themes(doc: Option<&Value>) -> Vec<JObj> {
    let mut live: Vec<JObj> = entries_of(doc)
        .into_values()
        .filter(|e| is_live(e) && obj_of(e.get("theme")).is_some_and(|t| strict_eq(t.get("id"), e.get("id"))))
        .collect();
    live.sort_by(|a, b| {
        cmp_f64(order_key(b).saved_at, order_key(a).saved_at)
            .then_with(|| a.get("id").and_then(Value::as_str).cmp(&b.get("id").and_then(Value::as_str)))
    });
    live
}

/// id → entry: this device's best entries (the reference's `KnownThemes`; any JSON value per key, only objects
/// with a valid `id` count, under that id).
pub type KnownThemes = BTreeMap<String, Value>;

fn known_entries(known: Option<&KnownThemes>) -> Vec<JObj> {
    let Some(known) = known else { return Vec::new() };
    known
        .values()
        .filter(|e| entry_id(Some(e)).is_some())
        .map(|e| fill_entry(e.as_object().expect("entry_id checked it")))
        .collect()
}

fn id_of(e: &JObj) -> String {
    e.get("id").and_then(Value::as_str).unwrap_or_default().to_string()
}

/// §4.5 — the join of raw's entries and known's, raw's unknown top-level keys kept, tombstones ≤ 64, size rule.
/// Returns `(doc, tooLarge)`.
pub fn merge_themes(raw: Option<&Value>, known: Option<&KnownThemes>, just_saved: Option<&str>) -> (JObj, bool) {
    let r = as_data(raw);
    let mut by_id = entries_of(r.as_ref());
    for e in known_entries(known) {
        let id = id_of(&e);
        let best = higher(by_id.remove(&id), Some(e)).expect("one side is set");
        by_id.insert(id, best);
    }
    let mut tombs: Vec<&JObj> = by_id.values().filter(|e| !is_live(e)).collect();
    tombs.sort_by(|a, b| cmp_doc(a, b));
    let cut: Vec<String> = tombs.iter().take(tombs.len().saturating_sub(MAX_TOMBSTONES)).map(|e| id_of(e)).collect();
    for id in cut {
        by_id.remove(&id);
    }
    let mut doc = JObj::new();
    if let Some(Value::Object(r)) = &r {
        for (k, v) in r {
            if k != "format" && k != "v" && k != "themes" {
                doc.insert(k.clone(), v.clone());
            }
        }
    }
    doc.insert("format".into(), Value::String(THEMES_FORMAT.into()));
    doc.insert("v".into(), Value::from(1));
    // BTreeMap order = code-point order of the ids
    let mut entries: Vec<JObj> = by_id.into_values().collect();
    let with = |doc: &JObj, entries: &[JObj]| {
        let mut d = doc.clone();
        d.insert("themes".into(), Value::Array(entries.iter().cloned().map(Value::Object).collect()));
        d
    };
    let full = with(&doc, &entries);
    if obj_bytes(&full) <= MAX_DOC_BYTES {
        return (full, false);
    }
    let mut victims: Vec<usize> = (0..entries.len()).filter(|&i| is_live(&entries[i]) && entries[i].contains_key("previous")).collect();
    victims.sort_by(|&a, &b| {
        let ja = (entries[a].get("id").and_then(Value::as_str) == just_saved) as u8;
        let jb = (entries[b].get("id").and_then(Value::as_str) == just_saved) as u8;
        ja.cmp(&jb).then_with(|| cmp_doc(&entries[a], &entries[b]))
    });
    for i in victims {
        entries[i].remove("previous");
        let d = with(&doc, &entries);
        if obj_bytes(&d) <= MAX_DOC_BYTES {
            return (d, false);
        }
    }
    (with(&doc, &entries), true)
}

/// `{entry, doc}` | `{noop: true}` | `{error}`.
#[derive(Clone, Debug, PartialEq)]
pub enum SaveTheme {
    Entry { entry: JObj, doc: JObj },
    Noop,
    Error(&'static str),
}

impl SaveTheme {
    pub fn to_json(&self) -> Value {
        let mut o = JObj::new();
        match self {
            SaveTheme::Entry { entry, doc } => {
                o.insert("entry".into(), Value::Object(entry.clone()));
                o.insert("doc".into(), Value::Object(doc.clone()));
            }
            SaveTheme::Noop => {
                o.insert("noop".into(), Value::Bool(true));
            }
            SaveTheme::Error(e) => {
                o.insert("error".into(), Value::String((*e).into()));
            }
        }
        Value::Object(o)
    }
}

/// §4.4 — a new entry (a save or a delete) and the document to put. `req` = `{id, theme? | deleted?, now, by}`.
pub fn save_theme(raw: Option<&Value>, known: Option<&KnownThemes>, req: &JObj) -> SaveTheme {
    let r = read_themes(raw);
    if matches!(r, Read::Newer(_)) {
        return SaveTheme::Error("newer-format");
    }
    let id = match req.get("id") {
        Some(Value::String(id)) if is_theme_id(id) => id.clone(),
        _ => return SaveTheme::Error("bad-request"),
    };
    let del = req.get("deleted") == Some(&Value::Bool(true));
    let theme = as_data(req.get("theme"));
    if !del {
        match &theme {
            Some(Value::Object(t)) if t.get("id").and_then(Value::as_str) == Some(id.as_str()) => {}
            _ => return SaveTheme::Error("bad-request"),
        }
        if bytes_of(theme.as_ref().expect("checked")) > MAX_THEME_BYTES {
            return SaveTheme::Error("theme-too-large");
        }
    }
    let now = num(req.get("now")).unwrap_or(f64::NAN);
    let by = req.get("by").and_then(Value::as_str).unwrap_or_default();
    let known_e = known_entries(known).into_iter().filter(|e| id_of(e) == id).fold(None, |a, e| higher(a, Some(e)));
    let raw_e = match &r {
        Read::Ok(d) => entries_of(Some(&Value::Object(d.clone()))).remove(&id),
        _ => None,
    };
    let base = higher(known_e, raw_e);
    let unknown_keys = |e: Option<&JObj>| -> JObj {
        e.map(|e| {
            e.iter().filter(|(k, _)| !ENTRY_KNOWN_KEYS.contains(&k.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect()
        })
        .unwrap_or_default()
    };
    let mut entry;
    if del {
        let Some(b) = base.as_ref().filter(|b| is_live(b)) else { return SaveTheme::Noop };
        entry = unknown_keys(Some(b));
        entry.insert("id".into(), Value::String(id.clone()));
        set_stamps(&mut entry, stamps(Some(b), now, by));
        entry.insert("deleted".into(), Value::Bool(true));
    } else {
        let theme = theme.expect("checked");
        if let Some(b) = base.as_ref().filter(|b| is_live(b)) {
            if same_json(b.get("theme"), Some(&theme)) {
                return SaveTheme::Noop;
            }
        }
        if base.as_ref().is_none_or(|b| !is_live(b)) {
            let (merged, _) = merge_themes(raw, known, None);
            let live = entries_of(Some(&Value::Object(merged))).iter().filter(|(k, e)| **k != id && is_live(e)).count();
            if live >= MAX_THEMES {
                return SaveTheme::Error("too-many");
            }
        }
        entry = unknown_keys(base.as_ref());
        entry.insert("id".into(), Value::String(id.clone()));
        set_stamps(&mut entry, stamps(base.as_ref(), now, by));
        entry.insert("theme".into(), theme);
        if let Some(b) = base.as_ref().filter(|b| is_live(b)) {
            if let Some(bt @ Value::Object(_)) = b.get("theme") {
                let k = order_key(b);
                let mut prev = JObj::new();
                prev.insert("savedAt".into(), number_value(k.saved_at));
                prev.insert("rev".into(), number_value(k.rev));
                prev.insert("by".into(), Value::String(k.by));
                prev.insert("theme".into(), bt.clone());
                entry.insert("previous".into(), Value::Object(prev));
            }
        }
    }
    let mut next_known: KnownThemes = BTreeMap::new();
    for e in known_entries(known) {
        let eid = id_of(&e);
        let prev = next_known.remove(&eid).and_then(|v| v.as_object().cloned());
        let best = higher(prev, Some(e)).expect("one side is set");
        next_known.insert(eid, Value::Object(best));
    }
    next_known.insert(id.clone(), Value::Object(entry.clone()));
    let (doc, too_large) = merge_themes(raw, Some(&next_known), Some(&id));
    if too_large {
        return SaveTheme::Error("too-large");
    }
    SaveTheme::Entry { entry, doc }
}

/// A My-themes id for a new theme: `base` (a slug) made unique against the library and the ids in use (§4.1).
pub fn unique_theme_id(base: &str, taken: impl Fn(&str) -> bool) -> String {
    let b = if is_theme_id(base) { base } else { "theme" };
    if !taken(b) {
        return b.to_string();
    }
    let mut n: u64 = 2;
    loop {
        let tail = format!("-{n}");
        // ids are ASCII: a slice by UTF-16 units is a slice by bytes
        let keep = 64usize.saturating_sub(tail.len()).min(b.len());
        let id = format!("{}{tail}", b[..keep].trim_end_matches('-'));
        if is_theme_id(&id) && !taken(&id) {
            return id;
        }
        n += 1;
    }
}

// ---- the device protocol (§3.5, §4.6) ---------------------------------------------------------------------------

/// The choice's device state.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChoiceState {
    pub known: Option<JObj>,
    /// the last document received (ok or newer); None = null
    pub raw: Option<Value>,
    pub read_only: bool,
    pub want_put: bool,
}

/// My themes' device state.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ThemesState {
    pub known: KnownThemes,
    pub raw: Option<Value>,
    pub read_only: bool,
    pub want_put: bool,
    pub just_saved: Option<String>,
}

fn raw_of(v: Option<&Value>) -> Option<Value> {
    as_data(v).filter(|v| !v.is_null())
}

/// apply(patch): `saved` | `noop` | `read-only` | an error code. Mutates `s`.
pub fn choice_apply(s: &mut ChoiceState, patch: Option<&Value>, fallback: Option<&Value>, now: f64, by: &str) -> &'static str {
    if s.read_only {
        return "read-only";
    }
    match save_choice(s.raw.as_ref(), s.known.as_ref(), patch, fallback, now, by) {
        SaveChoice::Error(e) => e,
        SaveChoice::Noop => "noop",
        SaveChoice::Entry(e) => {
            s.known = Some(e);
            s.want_put = true;
            "saved"
        }
    }
}

/// put: the document to PUT (and `wantPut` cleared), or None (`idle`).
pub fn choice_put(s: &mut ChoiceState) -> Option<JObj> {
    if !s.want_put || s.read_only {
        return None;
    }
    let k = s.known.clone()?;
    s.want_put = false;
    Some(k)
}

/// receive(D): `ignored` | `read-only` | `new` | `changed` | `reassert` | `ok`.
pub fn choice_receive(s: &mut ChoiceState, value: Option<&Value>) -> &'static str {
    let d = match read_choice(value) {
        Read::Ignore => return "ignored",
        Read::Newer(_) => {
            s.read_only = true;
            s.raw = raw_of(value);
            return "read-only";
        }
        Read::Ok(d) => d,
    };
    s.read_only = false;
    s.raw = raw_of(value);
    let known = match &s.known {
        Some(k) => read_choice(Some(&Value::Object(k.clone()))).ok_doc(),
        None => None,
    };
    let Some(known) = known else {
        s.known = Some(d);
        return "new";
    };
    if cmp_doc(&d, &known) == Ordering::Greater {
        let changed = canon_obj(&choice_content(&d)) != canon_obj(&choice_content(&known));
        s.known = Some(d);
        return if changed { "changed" } else { "ok" };
    }
    if cmp_doc(&known, &d) == Ordering::Greater {
        s.want_put = true;
        return "reassert";
    }
    "ok"
}

/// save / delete of one of My themes: `saved` | `noop` | `read-only` | an error code. Mutates `s`.
pub fn themes_save(s: &mut ThemesState, req: &JObj) -> &'static str {
    if s.read_only {
        return "read-only";
    }
    match save_theme(s.raw.as_ref(), Some(&s.known), req) {
        SaveTheme::Error(e) => e,
        SaveTheme::Noop => "noop",
        SaveTheme::Entry { entry, .. } => {
            let id = req.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
            s.known.insert(id.clone(), Value::Object(entry));
            s.want_put = true;
            s.just_saved = Some(id);
            "saved"
        }
    }
}

pub fn themes_put(s: &mut ThemesState) -> Option<JObj> {
    if !s.want_put || s.read_only {
        return None;
    }
    let (doc, _) = merge_themes(s.raw.as_ref(), Some(&s.known), s.just_saved.as_deref());
    s.want_put = false;
    s.just_saved = None;
    Some(doc)
}

pub fn themes_receive(s: &mut ThemesState, value: Option<&Value>) -> &'static str {
    let d = match read_themes(value) {
        Read::Ignore => return "ignored",
        Read::Newer(_) => {
            s.read_only = true;
            s.raw = raw_of(value);
            return "read-only";
        }
        Read::Ok(d) => d,
    };
    s.read_only = false;
    let dv = Value::Object(d);
    s.raw = Some(dv.clone());
    for (id, e) in entries_of(Some(&dv)) {
        let mine = match s.known.get(&id) {
            Some(Value::Object(k)) => Some(fill_entry(k)),
            _ => None,
        };
        let best = higher(mine, Some(e)).expect("one side is set");
        s.known.insert(id, Value::Object(best));
    }
    let (merged, _) = merge_themes(Some(&dv), Some(&s.known), None);
    if canon_obj(&merged) != canon_json(&dv) {
        s.want_put = true;
        return "reassert";
    }
    "ok"
}

/// The re-assert delay (§6): 0–2000 ms from a hash of the device id (FNV-1a over UTF-16 code units).
pub fn reassert_delay_ms(device_id: &str) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for u in device_id.encode_utf16() {
        h ^= u as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h % 2001
}

/// The retry schedule (§6): 1, 2, 4 … s, capped at 5 min.
pub fn retry_delay_ms(attempt: i64) -> u64 {
    let e = attempt.clamp(0, 20) as u32;
    (1000u64 << e).min(300_000)
}

// ---- the legacy choice (§8) -------------------------------------------------------------------------------------

/// The device's pre-11.0 theme setting as a choice content: "goose" → Goose + its sounds, anything else → OLED.
pub fn legacy_choice(saved: Option<&str>) -> JObj {
    let goose = saved == Some("goose");
    let mut c = JObj::new();
    c.insert("theme".into(), Value::String(if goose { "goose" } else { "oled" }.into()));
    c.insert("sounds".into(), Value::String(if goose { "goose" } else { "default" }.into()));
    c.insert("icons".into(), Value::String("duotone".into()));
    c
}

// ---- the vector simulator (THEMES-SYNC §10 "sync") ---------------------------------------------------------------

struct SimDevice {
    choice: ChoiceState,
    themes: ThemesState,
    legacy: JObj,
}

/// Plays a `sync` vector: devices (their id = their name) and one server holding the two documents.
/// Returns `{log, server, devices}` as the reference's `simulate` does.
pub fn simulate(vector: &Value) -> Value {
    let empty = JObj::new();
    let v = vector.as_object().unwrap_or(&empty);
    let names: Vec<String> = match v.get("devices") {
        Some(Value::Array(a)) => a.iter().filter_map(|n| n.as_str().map(str::to_string)).collect(),
        _ => Vec::new(),
    };
    let legacy = obj_of(v.get("legacy")).cloned().unwrap_or_default();
    // a Map in insertion order; a repeated name starts over but keeps its first place
    let mut order: Vec<String> = Vec::new();
    let mut dev: BTreeMap<String, SimDevice> = BTreeMap::new();
    for n in &names {
        if !order.contains(n) {
            order.push(n.clone());
        }
        let l = legacy.get(n).and_then(Value::as_str);
        dev.insert(n.clone(), SimDevice { choice: ChoiceState::default(), themes: ThemesState::default(), legacy: legacy_choice(l) });
    }
    let srv = obj_of(v.get("server")).cloned().unwrap_or_default();
    let mut server_choice: Value = srv.get("choice").cloned().unwrap_or(Value::Null);
    let mut server_themes: Value = srv.get("themes").cloned().unwrap_or(Value::Null);
    let mut log: Vec<String> = Vec::new();
    let steps: Vec<Value> = match v.get("steps") {
        Some(Value::Array(a)) => a.clone(),
        _ => Vec::new(),
    };
    for (i, raw) in steps.iter().enumerate() {
        let st = raw.as_object().cloned().unwrap_or_default();
        let name = st.get("device").and_then(Value::as_str).map(str::to_string);
        let ev = st.get("event").and_then(Value::as_str).unwrap_or("").to_string();
        let key = match st.get("key").and_then(Value::as_str) {
            Some("choice") => Some("choice"),
            Some("themes") => Some("themes"),
            _ => None,
        };
        let who = name.clone().unwrap_or_else(|| "-".into());
        let mut line = |k: &str, result: &str| log.push(format!("{i} {who} {ev} {k}: {result}"));
        let now = num(st.get("now")).unwrap_or(f64::NAN);
        if ev == "serverPut" {
            if let Some(k) = key {
                let doc = st.get("doc").cloned().unwrap_or(Value::Null);
                if k == "choice" {
                    server_choice = doc;
                } else {
                    server_themes = doc;
                }
                line(k, "set");
                continue;
            }
        }
        let Some(d) = name.as_ref().and_then(|n| dev.get_mut(n)) else {
            line(key.unwrap_or("-"), "bad-step");
            continue;
        };
        let by = name.as_deref().unwrap_or_default();
        match ev.as_str() {
            "apply" => {
                let fallback = match st.get("fallback") {
                    None | Some(Value::Null) => Value::Object(d.legacy.clone()),
                    Some(f) => f.clone(),
                };
                let r = choice_apply(&mut d.choice, st.get("patch"), Some(&fallback), now, by);
                line("choice", r);
            }
            "saveTheme" | "deleteTheme" => {
                let mut req = JObj::new();
                if let Some(id) = st.get("id") {
                    req.insert("id".into(), id.clone());
                }
                if ev == "saveTheme" {
                    if let Some(t) = st.get("theme") {
                        req.insert("theme".into(), t.clone());
                    }
                } else {
                    req.insert("deleted".into(), Value::Bool(true));
                }
                req.insert("now".into(), number_value(now));
                req.insert("by".into(), Value::String(by.into()));
                let r = themes_save(&mut d.themes, &req);
                line("themes", r);
            }
            "put" => {
                let Some(k) = key else {
                    line("-", "bad-step");
                    continue;
                };
                let doc = if k == "choice" { choice_put(&mut d.choice) } else { themes_put(&mut d.themes) };
                match doc {
                    None => line(k, "idle"),
                    Some(doc) => {
                        if k == "choice" {
                            server_choice = Value::Object(doc);
                        } else {
                            server_themes = Value::Object(doc);
                        }
                        line(k, "put");
                    }
                }
            }
            "deliver" | "incoming" => {
                let Some(k) = key else {
                    line("-", "bad-step");
                    continue;
                };
                let doc = if ev == "deliver" {
                    if k == "choice" { server_choice.clone() } else { server_themes.clone() }
                } else {
                    st.get("doc").cloned().unwrap_or(Value::Null)
                };
                let present = ev == "deliver" || st.contains_key("doc");
                let arg = present.then_some(&doc);
                let r = if k == "choice" { choice_receive(&mut d.choice, arg) } else { themes_receive(&mut d.themes, arg) };
                line(k, r);
            }
            _ => line(key.unwrap_or("-"), "bad-step"),
        }
    }
    let mut devices = JObj::new();
    for n in &order {
        let d = &dev[n];
        let mut c = JObj::new();
        c.insert("known".into(), d.choice.known.clone().map(Value::Object).unwrap_or(Value::Null));
        c.insert("raw".into(), d.choice.raw.clone().unwrap_or(Value::Null));
        c.insert("readOnly".into(), Value::Bool(d.choice.read_only));
        c.insert("wantPut".into(), Value::Bool(d.choice.want_put));
        let mut t = JObj::new();
        t.insert("known".into(), Value::Object(d.themes.known.iter().map(|(k, v)| (k.clone(), v.clone())).collect()));
        t.insert("raw".into(), d.themes.raw.clone().unwrap_or(Value::Null));
        t.insert("readOnly".into(), Value::Bool(d.themes.read_only));
        t.insert("wantPut".into(), Value::Bool(d.themes.want_put));
        t.insert("justSaved".into(), d.themes.just_saved.clone().map(Value::String).unwrap_or(Value::Null));
        let mut o = JObj::new();
        o.insert("choice".into(), Value::Object(c));
        o.insert("themes".into(), Value::Object(t));
        devices.insert(n.clone(), Value::Object(o));
    }
    let mut server = JObj::new();
    server.insert("choice".into(), server_choice);
    server.insert("themes".into(), server_themes);
    let mut out = JObj::new();
    out.insert("log".into(), Value::Array(log.into_iter().map(Value::String).collect()));
    out.insert("server".into(), Value::Object(server));
    out.insert("devices".into(), Value::Object(devices));
    Value::Object(out)
}
