//! table.rs — port of `table.ts`: the actions contract table (`actions.json` v1, ACTIONS.md §2–§9), read once. The
//! JSON file is a byte copy of the contract's (`sync-contract.sh`; `tests/vectors.rs` checks the copy), embedded at
//! build time. Everything here is pure data access: no I/O, no clock.
//!
//! Iteration order is never the JSON key order: registries are maps looked up by key (`BTreeMap`: code-point order
//! wherever the reference sorts keys), the `actions` and `groups` arrays keep their order.
//!
//! The reference holds the table as plain JSON typed by `table.ts`'s interfaces and relies on those types (`a.target
//! .kind` is a string, `kind.key` a string or absent, …). The typed structs below are exactly those interfaces, and
//! [`load`] refuses a table that is not (a field of another type, a target kind that is not declared, …), so the port
//! can never meet a table the reference would read differently; the parts the reference reads as JSON data (faces,
//! `when` rules, menu items, row overrides, split targets, label maps) stay `serde_json` values.

use std::collections::{BTreeMap, HashMap};
use std::sync::OnceLock;

use serde_json::Value;

use crate::js::JObj;

/// The contract's table (a byte copy; tests/vectors.rs checks it against the contract).
pub const TABLE_JSON: &str = include_str!("../actions.json");

/// The app evaluating (`App = "desktop" | "web"`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum App {
    Desktop,
    Web,
}

impl App {
    pub fn as_str(self) -> &'static str {
        match self {
            App::Desktop => "desktop",
            App::Web => "web",
        }
    }

    /// `APPS.includes(s)`.
    pub fn parse(s: &str) -> Option<App> {
        match s {
            "desktop" => Some(App::Desktop),
            "web" => Some(App::Web),
            _ => None,
        }
    }
}

pub const APPS: [App; 2] = [App::Desktop, App::Web];
/// `Look = "icon" | "iconLabel" | "label"`.
pub const LOOKS: [&str; 3] = ["icon", "iconLabel", "label"];
/// `Scale = "S" | "M"`.
pub const SCALES: [&str; 2] = ["S", "M"];

/// `{id, label}` of `groups` (palette / drawer order).
#[derive(Clone, Debug)]
pub struct Group {
    pub id: String,
    pub label: String,
}

/// A predicate (`PredicateDef`) or a state (`StateDef`): `{type, scope, values?, source?}` (ACTIONS.md §4.2).
#[derive(Clone, Debug)]
pub struct ValueDef {
    /// `type`: bool | enum (predicates) · bool | enum | text | list (states)
    pub ty: String,
    pub scope: String,
    pub values: Option<Vec<String>>,
    /// "app": derived from the evaluating app (platform.web), never read from the snapshot.
    pub source: Option<String>,
}

/// `ValueType` (ACTIONS.md §5.4).
#[derive(Clone, Debug)]
pub struct ValueType {
    pub values: Option<Vec<String>>,
    pub or: Option<String>,
    pub prefix: Option<Vec<String>>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    /// `token: true`
    pub token: bool,
    /// `server: true`
    pub server: bool,
    pub charset: Option<String>,
}

/// `TargetKind` (ACTIONS.md §5.1).
#[derive(Clone, Debug)]
pub struct TargetKind {
    /// the row-item target key a stored kind's value sits under
    pub key: Option<String>,
    /// its value type (valueTypes)
    pub value: Option<String>,
    /// the subject kind the value names (`snapshot.subjects.<subject>`)
    pub subject: Option<String>,
    pub stored: bool,
    pub portable: bool,
    pub picker: Option<String>,
    pub fallback_name: String,
}

/// `ParamSpec` (ACTIONS.md §5.5).
#[derive(Clone, Debug)]
pub struct ParamSpec {
    /// `type`: enum | text | emoji | colour | int
    pub ty: String,
    pub values: Option<Vec<String>>,
    /// `Record<string, string>` maps, read like the reference (`reg(labels, v)` then `isStr`)
    pub labels: Option<JObj>,
    pub short_labels: Option<JObj>,
    pub icons: Option<JObj>,
    /// `required: true`
    pub required: bool,
    pub max: Option<f64>,
    pub min: Option<f64>,
}

/// `TargetSpec` (ACTIONS.md §5.3).
#[derive(Clone, Debug)]
pub struct TargetSpec {
    /// a declared target kind ([`load`] checks it)
    pub kind: String,
    /// currentRoom | activeSpace | pick | none
    pub absent: Option<String>,
    pub picker: Option<String>,
    pub values: Option<Vec<String>>,
    pub labels: Option<JObj>,
    pub short_labels: Option<JObj>,
    pub icons: Option<JObj>,
    /// per stored value: an emoji drawn as the glyph (status presets)
    pub emojis: Option<JObj>,
    /// param name → spec; a `BTreeMap` iterates in code-point order (`sortedKeys(spec.params)`)
    pub params: Option<BTreeMap<String, ParamSpec>>,
    pub pickable: Option<bool>,
}

/// `ToggleSpec` (ACTIONS.md §6).
#[derive(Clone, Debug)]
pub struct ToggleSpec {
    pub state: String,
    /// the faces (JSON objects: label / shortLabel / icon / tip / emoji)
    pub on: Option<Value>,
    pub off: Option<Value>,
    /// a string, `{param: <name>}` or `{subject: true}`
    pub active_when: Option<Value>,
}

/// `ShortcutSpec` (ACTIONS.md §8): what the evaluator reads (the default keys); the whole spec stays in `raw`.
#[derive(Clone, Debug)]
pub struct ShortcutSpec {
    pub keys: Vec<String>,
    pub web: Option<Vec<String>>,
    pub desktop: Option<Vec<String>>,
    pub raw: Value,
}

/// One of `confirm.variants`.
#[derive(Clone, Debug)]
pub struct ConfirmVariant {
    pub when: Option<Value>,
    pub title: Option<String>,
    pub text: Option<String>,
    pub confirm_label: Option<String>,
}

/// `ConfirmSpec` (ACTIONS.md §7.4).
#[derive(Clone, Debug)]
pub struct ConfirmSpec {
    /// destructive | consent
    pub kind: String,
    pub title: String,
    pub text: String,
    pub confirm_label: String,
    /// `askReason: true`
    pub ask_reason: bool,
    pub only_when: Option<Value>,
    /// `variants ?? []`
    pub variants: Vec<ConfirmVariant>,
}

/// `split: {a, target?}` — the ▴ part of a split button.
#[derive(Clone, Debug)]
pub struct Split {
    pub a: String,
    pub target: Option<Value>,
}

/// `Action` (ACTIONS.md §3.1).
#[derive(Clone, Debug)]
pub struct Action {
    pub id: String,
    pub group: String,
    pub label: String,
    pub short_label: Option<String>,
    pub tip: Option<String>,
    pub keywords: Option<Vec<String>>,
    /// `icon: string | null` (the key itself is required)
    pub icon: Option<String>,
    /// `Partial<Record<App, string>>`
    pub icon_by_app: Option<JObj>,
    pub apps: Vec<String>,
    /// `When` rules, evaluated like the reference (absent → true, anything malformed → false)
    pub requires: Option<Value>,
    pub when: Option<Value>,
    pub target: Option<TargetSpec>,
    pub toggle: Option<ToggleSpec>,
    /// `Variant[]`: faces with a `when` (JSON objects)
    pub variants: Option<Vec<Value>>,
    pub shortcut: Option<Vec<ShortcutSpec>>,
    /// `hold: true`
    pub hold: bool,
    pub confirm: Option<ConfirmSpec>,
    /// `danger: true`
    pub danger: bool,
    /// `primary: true` — the surface's one primary call to action (the composer's Send)
    pub primary: bool,
    /// context-only actions (`contexts` present, even when empty, means context-only — `if (a.contexts)`)
    pub contexts: Option<Vec<String>>,
    pub split: Option<Split>,
    /// the face of a button whose target is STORED in the item ("Join {name}", "{name}"; ACTIONS.md §7.2) — a JSON
    /// face object (label / shortLabel / icon / tip / emoji)
    pub target_face: Option<Value>,
    pub pending: Option<String>,
    /// `palette: false` → never listed in the Ctrl+K palette (ACTIONS.md §10.2)
    pub palette: Option<bool>,
    /// the action's JSON object as the table holds it (since, notes, … for later steps)
    pub raw: Value,
}

/// `RowKind` (ACTIONS.md §9.1).
#[derive(Clone, Debug)]
pub struct RowKind {
    pub contexts: Vec<String>,
    pub forms: Vec<String>,
    /// the row's defaults when the stored row has none (ACTIONS.md §9.4)
    pub look: String,
    pub scale: String,
    /// header | tool | compact | call | rail | hover | tile (informative)
    pub style: String,
    pub editable: bool,
    /// action id → `RowOverride` (a JSON face + look + requires + when)
    pub overrides: Option<JObj>,
}

/// One of `menus.<id>.items`: `{menu}` (a submenu) or a row item + section / label / when / forms / sub (JSON).
#[derive(Clone, Debug)]
pub struct MenuItem {
    pub menu: Option<String>,
    pub raw: Value,
}

/// `MenuDef` (ACTIONS.md §9.7).
#[derive(Clone, Debug)]
pub struct MenuDef {
    pub context: Option<String>,
    pub label: Option<String>,
    pub items: Vec<MenuItem>,
}

/// `palette.tiers` (ACTIONS.md §10.4).
#[derive(Clone, Debug)]
pub struct Tiers {
    pub prefix: f64,
    pub word_start: f64,
    pub words: f64,
    pub substring: f64,
    pub alias: f64,
    pub subsequence: f64,
    pub keyword_prefix: f64,
    pub keyword_word_start: f64,
    pub keyword_substring: f64,
}

/// `palette.boosts`.
#[derive(Clone, Debug)]
pub struct Boosts {
    pub unread: f64,
    pub dm: f64,
}

/// `PaletteSpec` (ACTIONS.md §10).
#[derive(Clone, Debug)]
pub struct PaletteSpec {
    pub max_results: usize,
    pub prefixes: BTreeMap<String, String>,
    pub kind_order: Vec<String>,
    pub tiers: Tiers,
    pub boosts: Boosts,
    /// inclusive code-point ranges that separate words
    pub separators: Vec<(f64, f64)>,
    pub texts: Value,
}

/// `placeholder`: what a row draws for an action id this version does not know (ACTIONS.md §9.5).
#[derive(Clone, Debug)]
pub struct Placeholder {
    pub label: String,
    pub icon: String,
    pub tip: String,
}

/// `scopes.<scope>`: `{parent?}`.
#[derive(Clone, Debug)]
pub struct ScopeDef {
    pub parent: Option<String>,
}

/// The table (`Table`).
#[derive(Clone, Debug)]
pub struct Table {
    /// the whole table as JSON (keys, webKeys, contexts, paramTypes, docs … for later steps)
    pub json: Value,
    pub groups: Vec<Group>,
    pub predicates: BTreeMap<String, ValueDef>,
    pub states: BTreeMap<String, ValueDef>,
    pub value_types: BTreeMap<String, ValueType>,
    pub target_kinds: BTreeMap<String, TargetKind>,
    pub scopes: BTreeMap<String, ScopeDef>,
    pub limits: BTreeMap<String, f64>,
    pub placeholder: Placeholder,
    pub palette: PaletteSpec,
    pub rows: BTreeMap<String, RowKind>,
    pub menus: BTreeMap<String, MenuDef>,
    /// in the table's order (sorted by id)
    pub actions: Vec<Action>,
    /// id → index in `actions` (`new Map(actions.map(a => [a.id, a]))`: a repeated id → the last one)
    by_id: HashMap<String, usize>,
}

static OVERLAY: OnceLock<fn(&mut Value)> = OnceLock::new();
static BUILT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// An app's overlay over the table JSON before it is read once (the self-hosted server gates: `server.<feature>`
/// predicates ANDed into the `requires` of the actions a feature carries — the desktop's server_offer.rs, the web's
/// applyActionGates). The contract file itself is never edited. Install it before the first [`table`]; false when
/// the table was already built without it. A second install keeps the first.
pub fn install_overlay(f: fn(&mut Value)) -> bool {
    if BUILT.load(std::sync::atomic::Ordering::SeqCst) {
        return false;
    }
    let _ = OVERLAY.set(f);
    true
}

/// The embedded table, read once (with the installed overlay, if any).
pub fn table() -> &'static Table {
    static T: OnceLock<Table> = OnceLock::new();
    T.get_or_init(|| {
        BUILT.store(true, std::sync::atomic::Ordering::SeqCst);
        let mut v = tinhat_theme::json::parse_json(TABLE_JSON).expect("actions.json is JSON");
        if let Some(f) = OVERLAY.get() {
            f(&mut v);
        }
        load(&v).unwrap_or_else(|e| panic!("actions.json is not the table the reference reads: {e}"))
    })
}

/// The action with this id, or None (`actionById`).
pub fn action_by_id(id: &str) -> Option<&'static Action> {
    let t = table();
    t.by_id.get(id).and_then(|&i| t.actions.get(i))
}

/// The scope chain of a shortcut scope, innermost first; an unknown scope is a chain of its own (`[scope]`).
pub fn scope_chain(scope: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut s: Option<String> = Some(scope.to_string());
    while let Some(cur) = s {
        if out.len() >= 16 || out.contains(&cur) {
            break;
        }
        s = table().scopes.get(&cur).and_then(|d| d.parent.clone());
        out.push(cur);
    }
    out
}

/// True when one scope is the other or an ancestor of it (bindings in them can meet one key press).
pub fn scopes_overlap(a: &str, b: &str) -> bool {
    scope_chain(a).iter().any(|x| x == b) || scope_chain(b).iter().any(|x| x == a)
}

// ---- reading the table ----------------------------------------------------------------------------------------------

type R<T> = Result<T, String>;

fn o_of<'a>(v: &'a Value, at: &str) -> R<&'a JObj> {
    v.as_object().ok_or_else(|| format!("{at}: not an object"))
}

fn field<'a>(o: &'a JObj, k: &str, at: &str) -> R<&'a Value> {
    o.get(k).ok_or_else(|| format!("{at}.{k}: missing"))
}

fn str_req(o: &JObj, k: &str, at: &str) -> R<String> {
    match o.get(k) {
        Some(Value::String(s)) => Ok(s.clone()),
        _ => Err(format!("{at}.{k}: not a string")),
    }
}

fn str_opt(o: &JObj, k: &str, at: &str) -> R<Option<String>> {
    match o.get(k) {
        None => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(format!("{at}.{k}: not a string")),
    }
}

fn bool_req(o: &JObj, k: &str, at: &str) -> R<bool> {
    match o.get(k) {
        Some(Value::Bool(b)) => Ok(*b),
        _ => Err(format!("{at}.{k}: not a boolean")),
    }
}

fn bool_opt(o: &JObj, k: &str, at: &str) -> R<Option<bool>> {
    match o.get(k) {
        None => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(format!("{at}.{k}: not a boolean")),
    }
}

fn num_of(v: &Value, at: &str) -> R<f64> {
    v.as_f64().filter(|x| x.is_finite()).ok_or_else(|| format!("{at}: not a number"))
}

fn num_opt(o: &JObj, k: &str, at: &str) -> R<Option<f64>> {
    match o.get(k) {
        None => Ok(None),
        Some(v) => num_of(v, &format!("{at}.{k}")).map(Some),
    }
}

fn strs_of(v: &Value, at: &str) -> R<Vec<String>> {
    let a = v.as_array().ok_or_else(|| format!("{at}: not a list"))?;
    a.iter()
        .enumerate()
        .map(|(i, x)| x.as_str().map(str::to_string).ok_or_else(|| format!("{at}[{i}]: not a string")))
        .collect()
}

fn strs_opt(o: &JObj, k: &str, at: &str) -> R<Option<Vec<String>>> {
    match o.get(k) {
        None => Ok(None),
        Some(v) => strs_of(v, &format!("{at}.{k}")).map(Some),
    }
}

fn obj_opt(o: &JObj, k: &str, at: &str) -> R<Option<JObj>> {
    match o.get(k) {
        None => Ok(None),
        Some(Value::Object(m)) => Ok(Some(m.clone())),
        Some(_) => Err(format!("{at}.{k}: not an object")),
    }
}

/// A `Record<string, string>` (labels, icons, prefixes …).
fn str_map_opt(o: &JObj, k: &str, at: &str) -> R<Option<JObj>> {
    let m = obj_opt(o, k, at)?;
    if let Some(m) = &m {
        for (key, v) in m {
            if !v.is_string() {
                return Err(format!("{at}.{k}.{key}: not a string"));
            }
        }
    }
    Ok(m)
}

fn objs_opt(o: &JObj, k: &str, at: &str) -> R<Option<Vec<Value>>> {
    match o.get(k) {
        None => Ok(None),
        Some(Value::Array(a)) => {
            for (i, x) in a.iter().enumerate() {
                if !x.is_object() {
                    return Err(format!("{at}.{k}[{i}]: not an object"));
                }
            }
            Ok(Some(a.clone()))
        }
        Some(_) => Err(format!("{at}.{k}: not a list")),
    }
}

fn registry<'a>(t: &'a JObj, k: &str) -> R<&'a JObj> {
    o_of(field(t, k, "table")?, k)
}

fn value_def(v: &Value, at: &str) -> R<ValueDef> {
    let o = o_of(v, at)?;
    Ok(ValueDef {
        ty: str_req(o, "type", at)?,
        scope: str_req(o, "scope", at)?,
        values: strs_opt(o, "values", at)?,
        source: str_opt(o, "source", at)?,
    })
}

fn value_type(v: &Value, at: &str) -> R<ValueType> {
    let o = o_of(v, at)?;
    Ok(ValueType {
        values: strs_opt(o, "values", at)?,
        or: str_opt(o, "or", at)?,
        prefix: strs_opt(o, "prefix", at)?,
        min: num_opt(o, "min", at)?,
        max: num_opt(o, "max", at)?,
        token: bool_opt(o, "token", at)? == Some(true),
        server: bool_opt(o, "server", at)? == Some(true),
        charset: str_opt(o, "charset", at)?,
    })
}

fn target_kind(v: &Value, at: &str) -> R<TargetKind> {
    let o = o_of(v, at)?;
    let k = TargetKind {
        key: str_opt(o, "key", at)?,
        value: str_opt(o, "value", at)?,
        subject: str_opt(o, "subject", at)?,
        stored: bool_req(o, "stored", at)?,
        portable: bool_req(o, "portable", at)?,
        picker: str_opt(o, "picker", at)?,
        fallback_name: str_req(o, "fallbackName", at)?,
    };
    // the reference reads the kind key's value of a target as the validated string (resolveTarget `t[kind.key] as
    // string`): true when only stored kinds have a key (validateTarget checks the key of a stored kind)
    if k.key.is_some() && !k.stored {
        return Err(format!("{at}: a key on a kind that is not stored"));
    }
    Ok(k)
}

fn param_spec(v: &Value, at: &str) -> R<ParamSpec> {
    let o = o_of(v, at)?;
    Ok(ParamSpec {
        ty: str_req(o, "type", at)?,
        values: strs_opt(o, "values", at)?,
        labels: str_map_opt(o, "labels", at)?,
        short_labels: str_map_opt(o, "shortLabels", at)?,
        icons: str_map_opt(o, "icons", at)?,
        required: bool_opt(o, "required", at)? == Some(true),
        max: num_opt(o, "max", at)?,
        min: num_opt(o, "min", at)?,
    })
}

fn target_spec(v: &Value, at: &str, kinds: &BTreeMap<String, TargetKind>) -> R<TargetSpec> {
    let o = o_of(v, at)?;
    let kind = str_req(o, "kind", at)?;
    if !kinds.contains_key(&kind) {
        return Err(format!("{at}.kind: {kind} is not a declared target kind"));
    }
    let params = match o.get("params") {
        None => None,
        Some(p) => {
            let po = o_of(p, &format!("{at}.params"))?;
            let mut m = BTreeMap::new();
            for (k, ps) in po {
                m.insert(k.clone(), param_spec(ps, &format!("{at}.params.{k}"))?);
            }
            Some(m)
        }
    };
    Ok(TargetSpec {
        kind,
        absent: str_opt(o, "absent", at)?,
        picker: str_opt(o, "picker", at)?,
        values: strs_opt(o, "values", at)?,
        labels: str_map_opt(o, "labels", at)?,
        short_labels: str_map_opt(o, "shortLabels", at)?,
        icons: str_map_opt(o, "icons", at)?,
        emojis: str_map_opt(o, "emojis", at)?,
        params,
        pickable: bool_opt(o, "pickable", at)?,
    })
}

fn face_opt(o: &JObj, k: &str, at: &str) -> R<Option<Value>> {
    Ok(obj_opt(o, k, at)?.map(Value::Object))
}

fn toggle_spec(v: &Value, at: &str) -> R<ToggleSpec> {
    let o = o_of(v, at)?;
    let active_when = o.get("activeWhen").cloned();
    if let Some(Value::Object(aw)) = &active_when {
        if let Some(p) = aw.get("param") {
            if !p.is_string() {
                return Err(format!("{at}.activeWhen.param: not a string"));
            }
        }
    }
    Ok(ToggleSpec { state: str_req(o, "state", at)?, on: face_opt(o, "on", at)?, off: face_opt(o, "off", at)?, active_when })
}

fn shortcut_spec(v: &Value, at: &str) -> R<ShortcutSpec> {
    let o = o_of(v, at)?;
    Ok(ShortcutSpec {
        keys: strs_of(field(o, "keys", at)?, &format!("{at}.keys"))?,
        web: strs_opt(o, "web", at)?,
        desktop: strs_opt(o, "desktop", at)?,
        raw: v.clone(),
    })
}

fn confirm_spec(v: &Value, at: &str) -> R<ConfirmSpec> {
    let o = o_of(v, at)?;
    let mut variants = Vec::new();
    for (i, cv) in objs_opt(o, "variants", at)?.unwrap_or_default().iter().enumerate() {
        let at = format!("{at}.variants[{i}]");
        let co = o_of(cv, &at)?;
        variants.push(ConfirmVariant {
            when: co.get("when").cloned(),
            title: str_opt(co, "title", &at)?,
            text: str_opt(co, "text", &at)?,
            confirm_label: str_opt(co, "confirmLabel", &at)?,
        });
    }
    Ok(ConfirmSpec {
        kind: str_req(o, "kind", at)?,
        title: str_req(o, "title", at)?,
        text: str_req(o, "text", at)?,
        confirm_label: str_req(o, "confirmLabel", at)?,
        ask_reason: bool_opt(o, "askReason", at)? == Some(true),
        only_when: o.get("onlyWhen").cloned(),
        variants,
    })
}

fn action(v: &Value, at: &str, kinds: &BTreeMap<String, TargetKind>) -> R<Action> {
    let o = o_of(v, at)?;
    let id = str_req(o, "id", at)?;
    let at = &format!("actions[{id}]");
    let icon = match field(o, "icon", at)? {
        Value::String(s) => Some(s.clone()),
        Value::Null => None,
        _ => return Err(format!("{at}.icon: not a string or null")),
    };
    let target = match o.get("target") {
        None => None,
        Some(t) => Some(target_spec(t, &format!("{at}.target"), kinds)?),
    };
    let toggle = match o.get("toggle") {
        None => None,
        Some(t) => Some(toggle_spec(t, &format!("{at}.toggle"))?),
    };
    let shortcut = match objs_opt(o, "shortcut", at)? {
        None => None,
        Some(list) => Some(list.iter().enumerate().map(|(i, s)| shortcut_spec(s, &format!("{at}.shortcut[{i}]"))).collect::<R<Vec<_>>>()?),
    };
    let confirm = match o.get("confirm") {
        None => None,
        Some(c) => Some(confirm_spec(c, &format!("{at}.confirm"))?),
    };
    let split = match o.get("split") {
        None => None,
        Some(s) => {
            let so = o_of(s, &format!("{at}.split"))?;
            Some(Split { a: str_req(so, "a", &format!("{at}.split"))?, target: face_opt(so, "target", &format!("{at}.split"))? })
        }
    };
    Ok(Action {
        group: str_req(o, "group", at)?,
        label: str_req(o, "label", at)?,
        short_label: str_opt(o, "shortLabel", at)?,
        tip: str_opt(o, "tip", at)?,
        keywords: strs_opt(o, "keywords", at)?,
        icon,
        icon_by_app: str_map_opt(o, "iconByApp", at)?,
        apps: strs_of(field(o, "apps", at)?, &format!("{at}.apps"))?,
        requires: o.get("requires").cloned(),
        when: o.get("when").cloned(),
        target,
        toggle,
        variants: objs_opt(o, "variants", at)?,
        shortcut,
        hold: bool_opt(o, "hold", at)? == Some(true),
        confirm,
        danger: bool_opt(o, "danger", at)? == Some(true),
        primary: bool_opt(o, "primary", at)? == Some(true),
        contexts: strs_opt(o, "contexts", at)?,
        split,
        target_face: face_opt(o, "targetFace", at)?,
        pending: str_opt(o, "pending", at)?,
        palette: bool_opt(o, "palette", at)?,
        raw: v.clone(),
        id,
    })
}

fn row_kind(v: &Value, at: &str) -> R<RowKind> {
    let o = o_of(v, at)?;
    let overrides = obj_opt(o, "overrides", at)?;
    if let Some(ov) = &overrides {
        for (k, x) in ov {
            if !x.is_object() {
                return Err(format!("{at}.overrides.{k}: not an object"));
            }
        }
    }
    Ok(RowKind {
        contexts: strs_of(field(o, "contexts", at)?, &format!("{at}.contexts"))?,
        forms: strs_of(field(o, "forms", at)?, &format!("{at}.forms"))?,
        look: str_req(o, "look", at)?,
        scale: str_req(o, "scale", at)?,
        style: str_req(o, "style", at)?,
        editable: bool_req(o, "editable", at)?,
        overrides,
    })
}

fn menu_def(v: &Value, at: &str) -> R<MenuDef> {
    let o = o_of(v, at)?;
    let items = objs_opt(o, "items", at)?.ok_or_else(|| format!("{at}.items: missing"))?;
    let mut out = Vec::new();
    for (i, it) in items.into_iter().enumerate() {
        let io = o_of(&it, &format!("{at}.items[{i}]"))?;
        out.push(MenuItem { menu: str_opt(io, "menu", &format!("{at}.items[{i}]"))?, raw: it.clone() });
    }
    Ok(MenuDef { context: str_opt(o, "context", at)?, label: str_opt(o, "label", at)?, items: out })
}

fn palette_spec(v: &Value) -> R<PaletteSpec> {
    let at = "palette";
    let o = o_of(v, at)?;
    let max = num_of(field(o, "maxResults", at)?, "palette.maxResults")?;
    if max < 0.0 || max.trunc() != max {
        return Err("palette.maxResults: not a count".into());
    }
    let mut prefixes = BTreeMap::new();
    for (k, x) in str_map_opt(o, "prefixes", at)?.ok_or("palette.prefixes: missing")? {
        prefixes.insert(k, x.as_str().unwrap_or_default().to_string());
    }
    let t = o_of(field(o, "tiers", at)?, "palette.tiers")?;
    let tier = |k: &str| num_of(field(t, k, "palette.tiers")?, &format!("palette.tiers.{k}"));
    let tiers = Tiers {
        prefix: tier("prefix")?,
        word_start: tier("wordStart")?,
        words: tier("words")?,
        substring: tier("substring")?,
        alias: tier("alias")?,
        subsequence: tier("subsequence")?,
        keyword_prefix: tier("keywordPrefix")?,
        keyword_word_start: tier("keywordWordStart")?,
        keyword_substring: tier("keywordSubstring")?,
    };
    let b = o_of(field(o, "boosts", at)?, "palette.boosts")?;
    let boosts = Boosts {
        unread: num_of(field(b, "unread", "palette.boosts")?, "palette.boosts.unread")?,
        dm: num_of(field(b, "dm", "palette.boosts")?, "palette.boosts.dm")?,
    };
    let mut separators = Vec::new();
    for (i, r) in field(o, "separators", at)?.as_array().ok_or("palette.separators: not a list")?.iter().enumerate() {
        let at = format!("palette.separators[{i}]");
        match r.as_array().map(Vec::as_slice) {
            Some([lo, hi]) => separators.push((num_of(lo, &at)?, num_of(hi, &at)?)),
            _ => return Err(format!("{at}: not a [lo, hi] pair")),
        }
    }
    Ok(PaletteSpec {
        max_results: max as usize,
        prefixes,
        kind_order: strs_of(field(o, "kindOrder", at)?, "palette.kindOrder")?,
        tiers,
        boosts,
        separators,
        texts: o.get("texts").cloned().unwrap_or(Value::Null),
    })
}

/// Read a table (the embedded one: [`table()`]): the typed view of every part the engine reads, or the first place where
/// the JSON is not what `table.ts`'s types say.
pub fn load(v: &Value) -> Result<Table, String> {
    let t = o_of(v, "table")?;
    let mut groups = Vec::new();
    for (i, g) in field(t, "groups", "table")?.as_array().ok_or("groups: not a list")?.iter().enumerate() {
        let at = format!("groups[{i}]");
        let go = o_of(g, &at)?;
        groups.push(Group { id: str_req(go, "id", &at)?, label: str_req(go, "label", &at)? });
    }
    let mut predicates = BTreeMap::new();
    for (k, d) in registry(t, "predicates")? {
        predicates.insert(k.clone(), value_def(d, &format!("predicates.{k}"))?);
    }
    let mut states = BTreeMap::new();
    for (k, d) in registry(t, "states")? {
        states.insert(k.clone(), value_def(d, &format!("states.{k}"))?);
    }
    let mut value_types = BTreeMap::new();
    for (k, d) in registry(t, "valueTypes")? {
        value_types.insert(k.clone(), value_type(d, &format!("valueTypes.{k}"))?);
    }
    let mut target_kinds = BTreeMap::new();
    for (k, d) in registry(t, "targetKinds")? {
        target_kinds.insert(k.clone(), target_kind(d, &format!("targetKinds.{k}"))?);
    }
    let mut scopes = BTreeMap::new();
    for (k, d) in registry(t, "scopes")? {
        let at = format!("scopes.{k}");
        scopes.insert(k.clone(), ScopeDef { parent: str_opt(o_of(d, &at)?, "parent", &at)? });
    }
    let mut limits = BTreeMap::new();
    for (k, x) in registry(t, "limits")? {
        let n = num_of(x, &format!("limits.{k}"))?;
        if n < 0.0 || n.trunc() != n {
            return Err(format!("limits.{k}: not a count"));
        }
        limits.insert(k.clone(), n);
    }
    let ph = o_of(field(t, "placeholder", "table")?, "placeholder")?;
    let placeholder = Placeholder {
        label: str_req(ph, "label", "placeholder")?,
        icon: str_req(ph, "icon", "placeholder")?,
        tip: str_req(ph, "tip", "placeholder")?,
    };
    let palette = palette_spec(field(t, "palette", "table")?)?;
    let mut rows = BTreeMap::new();
    for (k, d) in registry(t, "rows")? {
        rows.insert(k.clone(), row_kind(d, &format!("rows.{k}"))?);
    }
    let mut menus = BTreeMap::new();
    for (k, d) in registry(t, "menus")? {
        menus.insert(k.clone(), menu_def(d, &format!("menus.{k}"))?);
    }
    let mut actions = Vec::new();
    for (i, a) in field(t, "actions", "table")?.as_array().ok_or("actions: not a list")?.iter().enumerate() {
        actions.push(action(a, &format!("actions[{i}]"), &target_kinds)?);
    }
    let mut by_id = HashMap::new();
    for (i, a) in actions.iter().enumerate() {
        by_id.insert(a.id.clone(), i);
    }
    Ok(Table {
        json: v.clone(),
        groups,
        predicates,
        states,
        value_types,
        target_kinds,
        scopes,
        limits,
        placeholder,
        palette,
        rows,
        menus,
        actions,
        by_id,
    })
}
