//! eval.rs — port of `evaluate.ts`: availability, targets and presentation of an action (ACTIONS.md §4–§7, §9). Pure:
//! a snapshot of the app's state in, JSON data out; every result is the reference's, byte for byte.
//!
//! Rules that keep the two implementations identical: own keys only, no locale functions (ASCII case only), no
//! regular expressions, strings measured in code points, map iteration by code-point order, arrays keep their order.
//! Where the reference measures a string that must be ASCII to pass (`isItemId`, `isLocalId`, a colour) in UTF-16 code
//! units, the port measures bytes: any non-ASCII input fails both ways.

use std::borrow::Cow;
use std::collections::BTreeMap;

use serde_json::Value;

use crate::js::{is_int, is_obj, is_str, is_true, num, obj, os, prop_key, s, sorted_keys, str_at, strict_eq, JObj};
use crate::table::{action_by_id, table, Action, App, ConfirmSpec, ParamSpec, TargetKind, TargetSpec, LOOKS, SCALES};

// ---- the snapshot ------------------------------------------------------------------------------------------------

/// The app's state as the evaluator sees it (ACTIONS.md §4.3):
///   values:   global predicate / state values by name
///   subjects: kind → id → { name?, values? } (scoped values; also the existence and names of targets)
///   current:  { room?, activeSpace? } — the defaults of `absent` targets
///   local:    this device's table of device-local targets (local id → `{<kind key>: value, label?}`)
///   lists:    subject kind / picker → ids in the app's order (the palette)
pub type Snapshot = JObj;

/// Subjects of one evaluation: subject kind → id.
pub type Subjects = BTreeMap<String, String>;

pub struct Env<'a> {
    pub app: App,
    pub snap: &'a Snapshot,
    pub subj: &'a Subjects,
    pub params: &'a JObj,
}

/// The subject entry `snapshot.subjects[kind][id]`, if the snapshot lists it.
pub fn subject_entry<'a>(snap: &'a Snapshot, kind: &str, id: &str) -> Option<&'a JObj> {
    is_obj(is_obj(is_obj(snap.get("subjects"))?.get(kind))?.get(id))
}

/// The value of a declared predicate or state (global, or the scoped subject's), or None (ACTIONS.md §4.2).
pub fn read_value<'a>(name: &str, env: &Env<'a>) -> Option<Cow<'a, Value>> {
    let t = table();
    let def = t.predicates.get(name).or_else(|| t.states.get(name))?;
    if def.source.as_deref() == Some("app") {
        return if name == "platform.web" { Some(Cow::Owned(Value::Bool(env.app == App::Web))) } else { None };
    }
    if def.scope == "global" {
        let vals = is_obj(env.snap.get("values"))?;
        return vals.get(name).map(Cow::Borrowed);
    }
    let id = env.subj.get(&def.scope)?;
    let e = subject_entry(env.snap, &def.scope, id)?;
    let vals = is_obj(e.get("values"))?;
    vals.get(name).map(Cow::Borrowed)
}

/// A `when` / `requires` rule (ACTIONS.md §4.1). Absent → true. Anything malformed → false.
pub fn eval_when(w: Option<&Value>, env: &Env) -> bool {
    let Some(w) = w else { return true };
    if let Value::String(name) = w {
        return matches!(read_value(name, env).as_deref(), Some(Value::Bool(true)));
    }
    let Value::Object(o) = w else { return false };
    if o.len() != 1 {
        return false;
    }
    let Some((k, v)) = o.iter().next() else { return false };
    match (k.as_str(), v) {
        ("not", _) => !eval_when(Some(v), env),
        ("all", Value::Array(xs)) => {
            let mut r = true;
            for x in xs {
                if !eval_when(Some(x), env) {
                    r = false;
                }
            }
            r
        }
        ("any", Value::Array(xs)) => {
            let mut r = false;
            for x in xs {
                if eval_when(Some(x), env) {
                    r = true;
                }
            }
            r
        }
        ("eq", Value::Array(xs)) => match xs.as_slice() {
            [Value::String(name), Value::String(want)] => {
                matches!(read_value(name, env).as_deref(), Some(Value::String(got)) if got == want)
            }
            _ => false,
        },
        ("param", Value::Array(xs)) => match xs.as_slice() {
            [Value::String(p), Value::String(want)] => matches!(env.params.get(p), Some(Value::String(got)) if got == want),
            _ => false,
        },
        _ => false,
    }
}

// ---- values, params, targets --------------------------------------------------------------------------------------

/// The code points of a string.
pub fn code_points(s: &str) -> Vec<u32> {
    s.chars().map(|c| c as u32).collect()
}

/// A lone surrogate (a JavaScript string can hold one; a Rust string cannot — kept for the rules' sake).
fn lone(c: u32) -> bool {
    (0xd800..=0xdfff).contains(&c)
}

/// Not allowed in a token (ids, keys, references, emoji): space and every control / line separator.
pub fn bad_token(c: u32) -> bool {
    c <= 0x20 || (0x7f..=0xa0).contains(&c) || c == 0x2028 || c == 0x2029 || lone(c)
}

/// Not allowed in text: controls and line separators.
pub fn bad_text(c: u32) -> bool {
    c < 0x20 || (0x7f..=0x9f).contains(&c) || c == 0x2028 || c == 0x2029 || lone(c)
}

/// A value of a declared value type (ACTIONS.md §5.4); `enum_values` for the type "enum".
pub fn check_value(ty: &str, v: Option<&Value>, enum_values: Option<&[String]>) -> bool {
    match v {
        Some(Value::String(v)) => check_str(ty, v, enum_values),
        _ => false,
    }
}

fn check_str(ty: &str, v: &str, enum_values: Option<&[String]>) -> bool {
    if ty == "enum" {
        return enum_values.is_some_and(|e| e.iter().any(|x| x == v));
    }
    let Some(t) = table().value_types.get(ty) else { return false };
    if let Some(values) = &t.values {
        if values.iter().any(|x| x == v) {
            return true;
        }
        return match &t.or {
            Some(or) => check_str(or, v, None),
            None => false,
        };
    }
    let cps = code_points(v);
    for &c in &cps {
        if if t.token { bad_token(c) } else { bad_text(c) } {
            return false;
        }
    }
    if let Some(prefix) = &t.prefix {
        if !prefix.iter().any(|p| v.starts_with(p.as_str())) {
            return false;
        }
    }
    if let Some(min) = t.min {
        if (cps.len() as f64) < min {
            return false;
        }
    }
    if let Some(max) = t.max {
        if (cps.len() as f64) > max {
            return false;
        }
    }
    if t.server {
        // "@local:server": a ':' after a non-empty localpart, and a non-empty server after it
        let colon = cps.iter().enumerate().skip(2).find(|(_, &c)| c == 0x3a).map(|(i, _)| i);
        match colon {
            None => return false,
            Some(i) if i == cps.len() - 1 => return false,
            Some(_) => {}
        }
    }
    if let Some(charset) = &t.charset {
        let allowed = code_points(charset);
        for c in &cps {
            if !allowed.contains(c) {
                return false;
            }
        }
    }
    true
}

/// A param value → its normalized form, or None when invalid (ACTIONS.md §5.5).
pub fn check_param(spec: &ParamSpec, v: Option<&Value>) -> Option<Value> {
    match spec.ty.as_str() {
        "enum" => match v {
            Some(Value::String(x)) if spec.values.as_ref().is_some_and(|vs| vs.contains(x)) => Some(Value::String(x.clone())),
            _ => None,
        },
        "text" => {
            let x = is_str(v)?;
            let cps = code_points(x);
            if cps.is_empty() || cps.len() as f64 > spec.max.unwrap_or(80.0) {
                return None;
            }
            if cps.iter().any(|&c| bad_text(c)) {
                return None;
            }
            Some(s(x))
        }
        "emoji" => {
            let x = is_str(v)?;
            let cps = code_points(x);
            if cps.is_empty() || cps.len() > 16 {
                return None;
            }
            if cps.iter().any(|&c| bad_token(c)) {
                return None;
            }
            Some(s(x))
        }
        "colour" => {
            // `v.length !== 7 || v[0] !== "#"` then six hex digits (upper case lowered): only ASCII passes, so bytes
            // measure it exactly
            let b = is_str(v)?.as_bytes();
            if b.len() != 7 || b[0] != b'#' {
                return None;
            }
            let mut out = String::from("#");
            for &c in &b[1..7] {
                match c {
                    b'0'..=b'9' | b'a'..=b'f' => out.push(c as char),
                    b'A'..=b'F' => out.push((c + 32) as char),
                    _ => return None,
                }
            }
            Some(Value::String(out))
        }
        "int" => {
            let x = is_int(v)?;
            if spec.min.is_some_and(|min| x < min) {
                return None;
            }
            if spec.max.is_some_and(|max| x > max) {
                return None;
            }
            Some(num(if x == 0.0 { 0.0 } else { x })) // -0 → 0
        }
        _ => None,
    }
}

/// A device-local id (the `local` key of a row item target): 1–12 of a–z, 0–9.
pub fn is_local_id(v: Option<&Value>) -> bool {
    let Some(x) = is_str(v) else { return false };
    let b = x.as_bytes();
    !b.is_empty() && b.len() <= 12 && b.iter().all(|&c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// The answer of [`validate_target`]: `{ok: true, target}` or `{ok: false, error, key?}`.
#[derive(Clone, Debug, PartialEq)]
pub enum TargetCheck {
    Ok(JObj),
    Err { error: &'static str, key: Option<String> },
}

impl TargetCheck {
    pub fn to_json(&self) -> Value {
        let mut o = obj();
        match self {
            TargetCheck::Ok(t) => {
                o.insert("ok".into(), Value::Bool(true));
                o.insert("target".into(), Value::Object(t.clone()));
            }
            TargetCheck::Err { error, key } => {
                o.insert("ok".into(), Value::Bool(false));
                o.insert("error".into(), s(error));
                if let Some(k) = key {
                    o.insert("key".into(), s(k));
                }
            }
        }
        Value::Object(o)
    }
}

fn fail(error: &'static str, key: Option<&str>) -> TargetCheck {
    TargetCheck::Err { error, key: key.map(str::to_string) }
}

/// Validate a row item's `target` object for an action (ACTIONS.md §5.6). None (no target) is valid. The first
/// failure, in this order: not-object, too-many-keys, not-targetable, then each key in code-point order:
/// not-pickable / not-portable / bad-value (the subject key), bad-local (`local`), bad-value (a param), unknown-key.
/// The result holds the normalized values (colour in lower case, -0 as 0). `transient`: the target is run, never
/// stored (a palette choice) — a device-local kind's own key is then allowed (no not-portable).
pub fn validate_target(a: &Action, t: Option<&Value>, transient: bool) -> TargetCheck {
    let mut out = obj();
    let Some(t) = t else { return TargetCheck::Ok(out) };
    let Value::Object(t) = t else { return fail("not-object", None) };
    let tbl = table();
    let keys = sorted_keys(t);
    if keys.len() as f64 > tbl.limits.get("targetKeys").copied().unwrap_or(4.0) {
        return fail("too-many-keys", None);
    }
    if keys.is_empty() {
        return TargetCheck::Ok(out);
    }
    let Some(spec) = &a.target else { return fail("not-targetable", None) };
    let Some(kind) = tbl.target_kinds.get(&spec.kind) else { return fail("not-targetable", None) };
    for k in keys {
        let v = t.get(k.as_str());
        if kind.stored && kind.key.as_deref() == Some(k.as_str()) {
            if spec.pickable == Some(false) {
                return fail("not-pickable", Some(k));
            }
            if !kind.portable && !transient {
                return fail("not-portable", Some(k));
            }
            // `kind.value as string`: an absent value type names no declared type (reg(…, undefined) → "undefined")
            if !check_value(kind.value.as_deref().unwrap_or("undefined"), v, spec.values.as_deref()) {
                return fail("bad-value", Some(k));
            }
            out.insert(k.clone(), v.cloned().unwrap_or(Value::Null));
            continue;
        }
        if k == "local" && kind.stored && !kind.portable {
            if spec.pickable == Some(false) {
                return fail("not-pickable", Some(k));
            }
            if !is_local_id(v) {
                return fail("bad-local", Some(k));
            }
            out.insert(k.clone(), v.cloned().unwrap_or(Value::Null));
            continue;
        }
        if let Some(ps) = spec.params.as_ref().and_then(|p| p.get(k.as_str())) {
            let Some(n) = check_param(ps, v) else { return fail("bad-value", Some(k)) };
            out.insert(k.clone(), n);
            continue;
        }
        return fail("unknown-key", Some(k));
    }
    TargetCheck::Ok(out)
}

/// `Resolution` (ACTIONS.md §5.7).
#[derive(Clone, Debug, Default)]
pub struct Resolution {
    pub subj: Subjects,
    pub params: JObj,
    pub pick: bool,
    /// the value came from the stored target (its key or a device-local id), not the absent rule or a context
    pub stored: bool,
    /// None | target-invalid | target-missing | target-unknown
    pub error: Option<&'static str>,
    /// the validation error code when error = target-invalid
    pub invalid: Option<&'static str>,
    /// the label of a device-local target (its entry in snapshot.local)
    pub local_label: Option<String>,
}

/// `EvalInput`: what [`present`] evaluates.
#[derive(Clone, Copy)]
pub struct EvalInput<'a> {
    pub app: App,
    pub action: &'a str,
    pub snapshot: &'a Snapshot,
    /// a row item's / menu item's target object (None: no target)
    pub target: Option<&'a Value>,
    /// the target is run, never stored (a palette choice; §5.6)
    pub transient: bool,
    /// subjects the surrounding context supplies: kind → id
    pub context: Option<&'a JObj>,
    /// invocation params (a key binding's, a host's) — validated like the target's
    pub params: Option<&'a JObj>,
    /// the row kind the item sits in ("header.main")
    pub row: Option<&'a str>,
    /// the menu the item sits in, its item (label / when / forms), and the form
    pub menu: Option<&'a str>,
    pub item: Option<&'a Value>,
    pub form: Option<&'a str>,
}

impl<'a> EvalInput<'a> {
    /// `{app, action, snapshot}`, everything else absent.
    pub fn new(app: App, action: &'a str, snapshot: &'a Snapshot) -> EvalInput<'a> {
        EvalInput { app, action, snapshot, target: None, transient: false, context: None, params: None, row: None, menu: None, item: None, form: None }
    }
}

/// Resolve an action's target and subjects (ACTIONS.md §5.7).
pub fn resolve_target(a: &Action, input: &EvalInput) -> Resolution {
    let mut r = Resolution::default();
    if let Some(ctx) = input.context {
        for k in sorted_keys(ctx) {
            if let Some(Value::String(id)) = ctx.get(k.as_str()) {
                r.subj.insert(k.clone(), id.clone());
            }
        }
    }
    let spec: Option<&TargetSpec> = a.target.as_ref();
    // an invalid target or invocation param stops here: the action is unavailable, nothing is resolved from it
    let t = match validate_target(a, input.target, input.transient) {
        TargetCheck::Ok(t) => t,
        TargetCheck::Err { error, .. } => {
            r.error = Some("target-invalid");
            r.invalid = Some(error);
            return r;
        }
    };
    let mut inv = obj();
    if let Some(ip) = input.params {
        for p in sorted_keys(ip) {
            let ps = spec.and_then(|sp| sp.params.as_ref()).and_then(|m| m.get(p.as_str()));
            let Some(n) = ps.and_then(|ps| check_param(ps, ip.get(p.as_str()))) else {
                r.error = Some("target-invalid");
                r.invalid = Some("bad-param");
                return r;
            };
            inv.insert(p.clone(), n);
        }
    }
    let Some(spec) = spec else { return r };
    // `reg(TABLE.targetKinds, spec.kind) as TargetKind` — declared (table::load checks every spec's kind)
    let Some(kind) = table().target_kinds.get(&spec.kind) else { return r };
    // params: the item's, then the invocation's (the invocation's win)
    if let Some(sp) = &spec.params {
        for p in sp.keys() {
            if let Some(v) = t.get(p) {
                r.params.insert(p.clone(), v.clone());
            }
        }
    }
    for p in sorted_keys(&inv) {
        r.params.insert(p.clone(), inv[p.as_str()].clone());
    }
    let mut from_item = false;
    let mut value: Option<String> = None;
    if let Some(key) = kind.key.as_deref().filter(|k| t.contains_key(*k)) {
        // the kind key's value was validated as a string of the kind's value type (only stored kinds have a key)
        value = is_str(t.get(key)).map(str::to_string);
        from_item = true;
        r.stored = true;
    } else if let Some(local) = t.get("local") {
        let e = prop_key(local).and_then(|lk| is_obj(is_obj(input.snapshot.get("local")).and_then(|l| l.get(&lk))));
        let lv = match (e, &kind.key) {
            (Some(e), Some(key)) => e.get(key),
            _ => None,
        };
        match lv {
            Some(Value::String(lv)) if check_str(kind.value.as_deref().unwrap_or("undefined"), lv, None) => {
                value = Some(lv.clone());
                from_item = true;
                r.stored = true;
                if let Some(lab) = e.and_then(|e| is_str(e.get("label"))) {
                    r.local_label = Some(lab.to_string());
                }
            }
            _ => r.pick = true,
        }
    }
    if let Some(value) = value {
        let vt = kind.value.as_ref().and_then(|v| table().value_types.get(v));
        let listed = vt.and_then(|vt| vt.values.as_ref()).is_some_and(|vs| vs.contains(&value));
        match (&kind.subject, listed) {
            (Some(subject), false) => {
                r.subj.insert(subject.clone(), value);
            }
            _ => {
                // `params[kind.key as string]` — a value always comes with the kind's key
                r.params.insert(kind.key.clone().unwrap_or_else(|| "undefined".into()), Value::String(value));
            }
        }
    } else if !r.pick {
        let in_ctx = kind.subject.as_ref().is_some_and(|sj| r.subj.contains_key(sj));
        if !in_ctx {
            match spec.absent.as_deref() {
                Some(rule @ ("currentRoom" | "activeSpace")) => {
                    let id = str_at(input.snapshot.get("current"), if rule == "currentRoom" { "room" } else { "activeSpace" });
                    match id {
                        None => {
                            if r.error.is_none() {
                                r.error = Some("target-missing");
                            }
                        }
                        Some(id) => {
                            r.subj.insert(kind.subject.clone().unwrap_or_else(|| "undefined".into()), id.to_string());
                            from_item = true; // a default subject must exist like a stored one
                        }
                    }
                }
                Some("pick") => r.pick = true,
                None => {
                    if r.error.is_none() {
                        r.error = Some("target-missing");
                    }
                }
                Some(_) => {} // "none"
            }
        }
    }
    if let Some(sp) = &spec.params {
        for (p, ps) in sp {
            if ps.required && !r.params.contains_key(p) {
                r.pick = true;
            }
        }
    }
    if from_item && r.error.is_none() {
        if let Some(subject) = &kind.subject {
            if let Some(id) = r.subj.get(subject) {
                if subject_entry(input.snapshot, subject, id).is_none() {
                    r.error = Some("target-unknown");
                }
            }
        }
    }
    r
}

// ---- presentation ------------------------------------------------------------------------------------------------

/// `ConfirmOut` (ACTIONS.md §7.4).
#[derive(Clone, Debug, PartialEq)]
pub struct ConfirmOut {
    pub kind: String,
    pub title: String,
    pub text: String,
    pub confirm_label: String,
    pub ask_reason: bool,
}

impl ConfirmOut {
    pub fn to_json(&self) -> Value {
        let mut o = obj();
        o.insert("kind".into(), s(&self.kind));
        o.insert("title".into(), s(&self.title));
        o.insert("text".into(), s(&self.text));
        o.insert("confirmLabel".into(), s(&self.confirm_label));
        o.insert("askReason".into(), Value::Bool(self.ask_reason));
        Value::Object(o)
    }
}

/// The split (▴) part: the action it runs and that action's target (ACTIONS.md §3).
#[derive(Clone, Debug, PartialEq)]
pub struct SplitOut {
    pub a: String,
    pub target: JObj,
}

impl SplitOut {
    pub fn to_json(&self) -> Value {
        let mut o = obj();
        o.insert("a".into(), s(&self.a));
        o.insert("target".into(), Value::Object(self.target.clone()));
        Value::Object(o)
    }
}

/// Everything a host needs to draw and run one action in one place (ACTIONS.md §7).
#[derive(Clone, Debug, PartialEq)]
pub struct Presentation {
    pub exists: bool,
    pub visible: bool,
    pub enabled: bool,
    /// the first failing stage: unknown | app | context | form | target | requires | when (None = available)
    pub why: Option<&'static str>,
    /// the target error when why = target: `target-invalid:<code>` | `target-missing` | `target-unknown`
    pub why_target: Option<String>,
    pub pick: bool,
    pub active: Option<bool>,
    pub label: String,
    pub short_label: Option<String>,
    pub icon: Option<String>,
    pub tip: Option<String>,
    pub look: Option<String>,
    pub danger: bool,
    /// the surface's one primary call to action (the composer's Send)
    pub primary: bool,
    pub hold: bool,
    pub subject: Option<String>,
    pub params: JObj,
    pub target_name: Option<String>,
    pub target_icon: Option<String>,
    /// an emoji to draw as the glyph (a target value's), else None
    pub emoji: Option<String>,
    pub confirm: Option<ConfirmOut>,
    pub split: Option<SplitOut>,
}

impl Presentation {
    pub fn to_json(&self) -> Value {
        let mut o = obj();
        o.insert("exists".into(), Value::Bool(self.exists));
        o.insert("visible".into(), Value::Bool(self.visible));
        o.insert("enabled".into(), Value::Bool(self.enabled));
        o.insert("why".into(), os(self.why));
        o.insert("whyTarget".into(), os(self.why_target.as_deref()));
        o.insert("pick".into(), Value::Bool(self.pick));
        o.insert("active".into(), self.active.map(Value::Bool).unwrap_or(Value::Null));
        o.insert("label".into(), s(&self.label));
        o.insert("shortLabel".into(), os(self.short_label.as_deref()));
        o.insert("icon".into(), os(self.icon.as_deref()));
        o.insert("tip".into(), os(self.tip.as_deref()));
        o.insert("look".into(), os(self.look.as_deref()));
        o.insert("danger".into(), Value::Bool(self.danger));
        o.insert("primary".into(), Value::Bool(self.primary));
        o.insert("hold".into(), Value::Bool(self.hold));
        o.insert("subject".into(), os(self.subject.as_deref()));
        o.insert("params".into(), Value::Object(self.params.clone()));
        o.insert("targetName".into(), os(self.target_name.as_deref()));
        o.insert("targetIcon".into(), os(self.target_icon.as_deref()));
        o.insert("emoji".into(), os(self.emoji.as_deref()));
        o.insert("confirm".into(), self.confirm.as_ref().map(ConfirmOut::to_json).unwrap_or(Value::Null));
        o.insert("split".into(), self.split.as_ref().map(SplitOut::to_json).unwrap_or(Value::Null));
        Value::Object(o)
    }
}

/// The enumerated value faces of the target: the stored key's value, then each enum param (code-point order).
fn value_faces(spec: Option<&TargetSpec>, kind: Option<&TargetKind>, r: &Resolution) -> Vec<JObj> {
    let mut out: Vec<JObj> = Vec::new();
    let (Some(spec), Some(kind)) = (spec, kind) else { return out };
    let face = |labels: Option<&JObj>, shorts: Option<&JObj>, icons: Option<&JObj>, v: Option<&Value>, emojis: Option<&JObj>, out: &mut Vec<JObj>| {
        let Some(Value::String(v)) = v else { return };
        let mut f = obj();
        if let Some(Value::String(l)) = labels.and_then(|m| m.get(v)) {
            f.insert("label".into(), s(l));
        }
        if let Some(Value::String(sl)) = shorts.and_then(|m| m.get(v)) {
            f.insert("shortLabel".into(), s(sl));
        }
        if let Some(Value::String(i)) = icons.and_then(|m| m.get(v)) {
            f.insert("icon".into(), s(i));
        }
        if let Some(Value::String(e)) = emojis.and_then(|m| m.get(v)) {
            f.insert("emoji".into(), s(e));
        }
        if !f.is_empty() {
            out.push(f);
        }
    };
    if let Some(key) = &kind.key {
        if let Some(v) = r.params.get(key) {
            face(spec.labels.as_ref(), spec.short_labels.as_ref(), spec.icons.as_ref(), Some(v), spec.emojis.as_ref(), &mut out);
        }
    }
    if let Some(sp) = &spec.params {
        for (p, ps) in sp {
            if ps.ty == "enum" {
                if let Some(v) = r.params.get(p) {
                    face(ps.labels.as_ref(), ps.short_labels.as_ref(), ps.icons.as_ref(), Some(v), None, &mut out);
                }
            }
        }
    }
    out
}

fn kind_of(a: &Action) -> Option<&'static TargetKind> {
    a.target.as_ref().and_then(|t| table().target_kinds.get(&t.kind))
}

/// The name of the target: the subject's name, a device-local label, the kind's fallback — or the label (else the
/// value) of a stored value that names no subject. None when the action has no target or nothing resolved.
fn target_name(a: &Action, r: &Resolution, snap: &Snapshot) -> Option<String> {
    let spec = a.target.as_ref()?;
    let kind = kind_of(a)?;
    if let Some(subject) = &kind.subject {
        if let Some(id) = r.subj.get(subject) {
            let n = subject_entry(snap, subject, id).and_then(|e| is_str(e.get("name")));
            if let Some(n) = n {
                return Some(n.to_string());
            }
            if let Some(l) = &r.local_label {
                return Some(l.clone());
            }
            return Some(kind.fallback_name.clone());
        }
    }
    let v = kind.key.as_ref().and_then(|k| r.params.get(k));
    if let Some(Value::String(v)) = v {
        return match spec.labels.as_ref().and_then(|m| m.get(v)) {
            Some(Value::String(l)) => Some(l.clone()),
            _ => Some(v.clone()),
        };
    }
    None
}

fn target_icon(a: &Action, r: &Resolution) -> Option<String> {
    let spec = a.target.as_ref()?;
    let kind = kind_of(a);
    for f in value_faces(Some(spec), kind, r) {
        if let Some(Value::String(i)) = f.get("icon") {
            return Some(i.clone());
        }
    }
    None
}

/// Fill `{name}` (the target's name) and `{<text state>}` placeholders (a global one, or one of the subject's scope:
/// `{room.pinnedCount}`), left to right (ACTIONS.md §7.3); an absent value fills "".
pub fn fill(s: &str, name: Option<&str>, env: &Env) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'{' {
            // `s.indexOf("}", i + 1)`: '{' and '}' are ASCII, so byte offsets find the same braces
            if let Some(off) = s[i + 1..].find('}') {
                let j = i + 1 + off;
                let inner = &s[i + 1..j];
                if is_placeholder_name(inner) {
                    if inner == "name" {
                        out.push_str(name.unwrap_or(""));
                        i = j + 1;
                        continue;
                    }
                    if table().states.get(inner).is_some_and(|d| d.ty == "text") {
                        if let Some(Value::String(v)) = read_value(inner, env).as_deref() {
                            out.push_str(v);
                        }
                        i = j + 1;
                        continue;
                    }
                }
            }
            out.push('{');
            i += 1;
            continue;
        }
        // copy one whole character (i is always on a character boundary here)
        let ch = s[i..].chars().next().unwrap_or('\u{fffd}');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// `[a-zA-Z][a-zA-Z0-9.]*`
fn is_placeholder_name(s: &str) -> bool {
    let b = s.as_bytes();
    if b.is_empty() {
        return false;
    }
    for (i, &c) in b.iter().enumerate() {
        let alpha = c.is_ascii_alphabetic();
        if if i == 0 { !alpha } else { !(alpha || c.is_ascii_digit() || c == b'.') } {
            return false;
        }
    }
    true
}

fn toggle_active(a: &Action, r: &Resolution, env: &Env) -> Option<bool> {
    let tg = a.toggle.as_ref()?;
    let v = read_value(&tg.state, env);
    let want: Option<Value> = match &tg.active_when {
        None => Some(Value::Bool(true)),
        Some(Value::String(aw)) => Some(s(aw)),
        Some(Value::Object(aw)) if aw.contains_key("param") => {
            // `activeWhen.param` is a string (table::load checks it)
            is_str(aw.get("param")).and_then(|p| r.params.get(p)).cloned()
        }
        Some(Value::Object(aw)) if aw.contains_key("subject") => {
            kind_of(a).and_then(|k| k.subject.as_ref()).and_then(|sj| r.subj.get(sj)).map(|id| s(id))
        }
        Some(_) => None,
    };
    let Some(want) = want.filter(|w| !w.is_null()) else { return Some(false) };
    Some(match v.as_deref() {
        Some(Value::Array(list)) => matches!(&want, Value::String(w) if list.iter().any(|x| x.as_str() == Some(w.as_str()))),
        Some(x) if !x.is_null() => strict_eq(x, &want),
        _ => false,
    })
}

fn confirm_out(c: Option<&ConfirmSpec>, name: Option<&str>, env: &Env) -> Option<ConfirmOut> {
    let c = c?;
    if let Some(ow) = &c.only_when {
        if !eval_when(Some(ow), env) {
            return None;
        }
    }
    let mut title = &c.title;
    let mut text = &c.text;
    let mut confirm_label = &c.confirm_label;
    for v in &c.variants {
        if eval_when(v.when.as_ref(), env) {
            if let Some(x) = &v.title {
                title = x;
            }
            if let Some(x) = &v.text {
                text = x;
            }
            if let Some(x) = &v.confirm_label {
                confirm_label = x;
            }
            break;
        }
    }
    Some(ConfirmOut {
        kind: c.kind.clone(),
        title: fill(title, name, env),
        text: fill(text, name, env),
        confirm_label: fill(confirm_label, name, env),
        ask_reason: c.ask_reason,
    })
}

/// The face layers of a presentation (label / shortLabel / icon / tip / emoji).
struct Face {
    label: String,
    short_label: Option<String>,
    icon: Option<String>,
    tip: Option<String>,
    emoji: Option<String>,
}

impl Face {
    fn apply(&mut self, f: Option<&JObj>) {
        let Some(f) = f else { return };
        if let Some(Value::String(x)) = f.get("label") {
            self.label = x.clone();
        }
        if let Some(Value::String(x)) = f.get("shortLabel") {
            self.short_label = Some(x.clone());
        }
        if let Some(Value::String(x)) = f.get("icon") {
            self.icon = Some(x.clone());
        }
        if let Some(Value::String(x)) = f.get("tip") {
            self.tip = Some(x.clone());
        }
        if let Some(Value::String(x)) = f.get("emoji") {
            self.emoji = Some(x.clone());
        }
    }
}

/// Everything a host needs to draw and run one action in one place (ACTIONS.md §7).
pub fn present(input: &EvalInput) -> Presentation {
    let Some(a) = action_by_id(input.action) else {
        return Presentation {
            exists: false,
            visible: false,
            enabled: false,
            why: Some("unknown"),
            why_target: None,
            pick: false,
            active: None,
            label: input.action.to_string(),
            short_label: None,
            icon: None,
            tip: None,
            look: None,
            danger: false,
            primary: false,
            hold: false,
            subject: None,
            params: obj(),
            target_name: None,
            target_icon: None,
            emoji: None,
            confirm: None,
            split: None,
        };
    };
    let tbl = table();
    let exists = a.apps.iter().any(|x| x == input.app.as_str());
    // placement: a contextOnly action needs a row / menu that provides one of its contexts
    let mut placed = true;
    let mut row_when: Option<&Value> = None;
    let mut row_requires: Option<&Value> = None;
    let mut over: Option<&JObj> = None;
    if let Some(row) = input.row {
        let rk = tbl.rows.get(row);
        let provided: &[String] = rk.map(|r| r.contexts.as_slice()).unwrap_or(&[]);
        if let Some(ctxs) = &a.contexts {
            if !ctxs.iter().any(|c| provided.contains(c)) {
                placed = false;
            }
        }
        if let Some(ov) = is_obj(rk.and_then(|r| r.overrides.as_ref()).and_then(|o| o.get(&a.id))) {
            over = Some(ov);
            row_when = ov.get("when");
            row_requires = ov.get("requires");
        }
    } else if let Some(menu) = input.menu {
        let provided: Option<&String> = tbl.menus.get(menu).and_then(|md| md.context.as_ref());
        if let Some(ctxs) = &a.contexts {
            if !ctxs.iter().any(|c| provided == Some(c)) {
                placed = false;
            }
        }
    }
    let r = resolve_target(a, input);
    let env = Env { app: input.app, snap: input.snapshot, subj: &r.subj, params: &r.params };
    let item = is_obj(input.item);
    let mut form_ok = true;
    if let Some(Value::Array(forms)) = item.and_then(|i| i.get("forms")) {
        form_ok = input.form.is_some_and(|f| forms.iter().any(|x| x.as_str() == Some(f)));
    }
    // the stages in order (ACTIONS.md §7.1): a bad target dims before `requires` is asked (it names no subject to ask)
    let mut why: Option<&'static str> = None;
    if !exists {
        why = Some("app");
    } else if !placed {
        why = Some("context");
    } else if !form_ok {
        why = Some("form");
    } else if r.error.is_some() {
        why = Some("target");
    } else if !eval_when(a.requires.as_ref(), &env) || !eval_when(row_requires, &env) {
        why = Some("requires");
    } else {
        let mut when_ok = eval_when(a.when.as_ref(), &env);
        if !eval_when(row_when, &env) {
            when_ok = false;
        }
        if let Some(item) = item {
            if item.contains_key("when") && !eval_when(item.get("when"), &env) {
                when_ok = false;
            }
        }
        if !when_ok {
            why = Some("when");
        }
    }
    let visible = !matches!(why, Some("app" | "context" | "form" | "requires"));
    let enabled = why.is_none();

    let kind = kind_of(a);
    let active = toggle_active(a, &r, &env);
    let mut face = Face { label: a.label.clone(), short_label: a.short_label.clone(), icon: a.icon.clone(), tip: a.tip.clone(), emoji: None };
    if let Some(Value::String(by_app)) = a.icon_by_app.as_ref().and_then(|m| m.get(input.app.as_str())) {
        face.icon = Some(by_app.clone());
    }
    // the face layers (ACTIONS.md §7.2): value faces → toggle → first variant → the stored target's face → row override
    // → menu item label
    for f in value_faces(a.target.as_ref(), kind, &r) {
        face.apply(Some(&f));
    }
    if let Some(tg) = &a.toggle {
        face.apply(is_obj(if active == Some(true) { tg.on.as_ref() } else { tg.off.as_ref() }));
    }
    for v in a.variants.iter().flatten() {
        if eval_when(v.get("when"), &env) {
            face.apply(v.as_object());
            break;
        }
    }
    if let Some(tf) = a.target_face.as_ref().filter(|_| r.stored) {
        face.apply(tf.as_object());
    }
    let mut look: Option<String> = None;
    if let Some(ov) = over {
        face.apply(Some(ov));
        if let Some(Value::String(l)) = ov.get("look") {
            look = Some(l.clone());
        }
    }
    if let Some(Value::String(l)) = item.and_then(|i| i.get("label")) {
        face.label = l.clone();
    }
    let t_name = target_name(a, &r, input.snapshot);
    let subject = kind.and_then(|k| k.subject.as_ref()).and_then(|sj| r.subj.get(sj)).cloned();
    let name_for_text: Option<String> = t_name.clone().or_else(|| kind.map(|k| k.fallback_name.clone()));
    let nt = name_for_text.as_deref();
    let why_target = if why == Some("target") {
        if r.error == Some("target-invalid") {
            Some(format!("target-invalid:{}", r.invalid.unwrap_or("undefined")))
        } else {
            r.error.map(str::to_string)
        }
    } else {
        None
    };
    let label = fill(&face.label, nt, &env);
    let short_label = face.short_label.as_deref().map(|x| fill(x, nt, &env));
    let tip = face.tip.as_deref().map(|x| fill(x, nt, &env));
    let confirm = confirm_out(a.confirm.as_ref(), nt, &env);
    let target_icon = target_icon(a, &r);
    Presentation {
        exists,
        visible,
        enabled,
        why,
        why_target,
        pick: r.pick,
        active,
        label,
        short_label,
        icon: face.icon,
        tip,
        look,
        danger: a.danger,
        primary: a.primary,
        hold: a.hold,
        subject,
        params: r.params,
        target_name: t_name,
        target_icon,
        emoji: face.emoji,
        confirm,
        split: split_out(a),
    }
}

fn split_out(a: &Action) -> Option<SplitOut> {
    let sp = a.split.as_ref()?;
    let mut t = obj();
    if let Some(Value::Object(st)) = &sp.target {
        for k in sorted_keys(st) {
            t.insert(k.clone(), st[k.as_str()].clone());
        }
    }
    Some(SplitOut { a: sp.a.clone(), target: t })
}

// ---- rows and menus ------------------------------------------------------------------------------------------------

/// One button of a row (or item of a menu) as the host draws it (ACTIONS.md §9.4).
#[derive(Clone, Debug, PartialEq)]
pub struct ItemOut {
    /// the stored action id ("" when the item has none)
    pub a: String,
    /// the id is in this version's table (false → the "needs a newer Tin Hat" placeholder, §9.5)
    pub known: bool,
    /// shown | dimmed | hidden
    pub state: &'static str,
    /// the first failing stage: bad | unknown | app | context | form | target | requires | when (None = available)
    pub why: Option<&'static str>,
    pub label: String,
    pub short_label: Option<String>,
    pub icon: Option<String>,
    pub tip: Option<String>,
    /// how this button draws: the row's look, or the row kind's override for this action
    pub look: String,
    pub active: Option<bool>,
    pub danger: bool,
    pub primary: bool,
    /// running it opens its target picker (no stored target, absent = pick)
    pub pick: bool,
    pub hold: bool,
    /// the resolved subject (a room, person, sound … id) and its kind — hosts draw avatars / emoji from it
    pub subject: Option<String>,
    pub subject_kind: Option<String>,
    pub target_name: Option<String>,
    pub target_icon: Option<String>,
    /// an emoji to draw as the glyph (hosts prefer it to the icon)
    pub emoji: Option<String>,
    /// the item's target, normalized (None: none, or invalid)
    pub target: Option<JObj>,
    pub split: Option<SplitOut>,
    pub sep_before: bool,
    pub hide_when_off: bool,
}

impl ItemOut {
    pub fn to_json(&self) -> Value {
        let mut o = obj();
        o.insert("a".into(), s(&self.a));
        o.insert("known".into(), Value::Bool(self.known));
        o.insert("state".into(), s(self.state));
        o.insert("why".into(), os(self.why));
        o.insert("label".into(), s(&self.label));
        o.insert("shortLabel".into(), os(self.short_label.as_deref()));
        o.insert("icon".into(), os(self.icon.as_deref()));
        o.insert("tip".into(), os(self.tip.as_deref()));
        o.insert("look".into(), s(&self.look));
        o.insert("active".into(), self.active.map(Value::Bool).unwrap_or(Value::Null));
        o.insert("danger".into(), Value::Bool(self.danger));
        o.insert("primary".into(), Value::Bool(self.primary));
        o.insert("pick".into(), Value::Bool(self.pick));
        o.insert("hold".into(), Value::Bool(self.hold));
        o.insert("subject".into(), os(self.subject.as_deref()));
        o.insert("subjectKind".into(), os(self.subject_kind.as_deref()));
        o.insert("targetName".into(), os(self.target_name.as_deref()));
        o.insert("targetIcon".into(), os(self.target_icon.as_deref()));
        o.insert("emoji".into(), os(self.emoji.as_deref()));
        o.insert("target".into(), self.target.clone().map(Value::Object).unwrap_or(Value::Null));
        o.insert("split".into(), self.split.as_ref().map(SplitOut::to_json).unwrap_or(Value::Null));
        o.insert("sepBefore".into(), Value::Bool(self.sep_before));
        o.insert("hideWhenOff".into(), Value::Bool(self.hide_when_off));
        Value::Object(o)
    }
}

/// The layout's item id syntax (LAYOUT.md §3.9: `[a-z][a-zA-Z0-9.]{0,63}`); anything else cannot be stored. (ASCII
/// only, so bytes measure it exactly.)
pub fn is_item_id(s: &str) -> bool {
    let b = s.as_bytes();
    if b.is_empty() || b.len() > 64 {
        return false;
    }
    for (i, &c) in b.iter().enumerate() {
        let lower = c.is_ascii_lowercase();
        if if i == 0 { !lower } else { !(lower || c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'.') } {
            return false;
        }
    }
    true
}

fn placeholder_item(a: &str, why: &'static str, look: &str, hide_off: bool, sep: bool) -> ItemOut {
    let ph = &table().placeholder;
    let bad = why == "bad";
    ItemOut {
        a: a.to_string(),
        known: false,
        state: if bad || hide_off { "hidden" } else { "dimmed" },
        why: Some(why),
        label: if bad { String::new() } else { ph.label.clone() },
        short_label: None,
        icon: if bad { None } else { Some(ph.icon.clone()) },
        tip: if bad { None } else { Some(ph.tip.clone()) },
        look: look.to_string(),
        active: None,
        danger: false,
        primary: false,
        pick: false,
        hold: false,
        subject: None,
        subject_kind: None,
        target_name: None,
        target_icon: None,
        emoji: None,
        target: None,
        split: None,
        sep_before: sep,
        hide_when_off: hide_off,
    }
}

/// Where an item sits: `{row?, menu?, form?, context?, look?}`.
#[derive(Clone, Copy, Default)]
pub struct Where<'a> {
    pub row: Option<&'a str>,
    pub menu: Option<&'a str>,
    pub form: Option<&'a str>,
    pub context: Option<&'a JObj>,
    pub look: Option<&'a str>,
}

/// One row / menu item's state (ACTIONS.md §9.4): not visible → hidden; unavailable → dimmed, or hidden with
/// hideWhenOff; else shown. An item that is not an object with an `a` of the item id syntax is hidden (why bad); an
/// id this version does not know is the placeholder (dimmed, or hidden with hideWhenOff). `item` is the layout row item
/// {a, target?, hideWhenOff?, sepBefore?}; `look` the row's.
pub fn eval_item(app: App, snap: &Snapshot, item: &Value, w: &Where) -> ItemOut {
    let look = w.look.unwrap_or("icon");
    let io = item.as_object();
    let hide_off = io.is_some_and(|o| is_true(o.get("hideWhenOff")));
    let sep = io.is_some_and(|o| is_true(o.get("sepBefore")));
    let a_str = io.and_then(|o| is_str(o.get("a")));
    let (Some(io), Some(id)) = (io, a_str.filter(|x| is_item_id(x))) else {
        return placeholder_item(a_str.unwrap_or(""), "bad", look, hide_off, sep);
    };
    let Some(a) = action_by_id(id) else { return placeholder_item(id, "unknown", look, hide_off, sep) };
    let p = present(&EvalInput {
        app,
        action: id,
        snapshot: snap,
        target: io.get("target"),
        transient: false,
        context: w.context,
        params: None,
        row: w.row,
        menu: w.menu,
        item: if w.menu.is_some() { Some(item) } else { None },
        form: w.form,
    });
    let state = if !p.visible {
        "hidden"
    } else if !p.enabled {
        if hide_off {
            "hidden"
        } else {
            "dimmed"
        }
    } else {
        "shown"
    };
    let chk = validate_target(a, io.get("target"), false);
    let kind = kind_of(a);
    ItemOut {
        a: id.to_string(),
        known: true,
        state,
        why: p.why,
        label: p.label,
        short_label: p.short_label,
        icon: p.icon,
        tip: p.tip,
        look: p.look.unwrap_or_else(|| look.to_string()),
        active: p.active,
        danger: p.danger,
        primary: p.primary,
        pick: p.pick,
        hold: p.hold,
        subject_kind: if p.subject.is_some() { kind.and_then(|k| k.subject.clone()) } else { None },
        subject: p.subject,
        target_name: p.target_name,
        target_icon: p.target_icon,
        emoji: p.emoji,
        target: match chk {
            TargetCheck::Ok(t) if io.contains_key("target") => Some(t),
            _ => None,
        },
        split: p.split,
        sep_before: sep,
        hide_when_off: hide_off,
    }
}

/// A resolved row (ACTIONS.md §9.4).
#[derive(Clone, Debug, PartialEq)]
pub struct RowOut {
    pub row: String,
    pub look: String,
    pub scale: String,
    pub style: String,
    pub editable: bool,
    pub items: Vec<ItemOut>,
}

impl RowOut {
    pub fn to_json(&self) -> Value {
        let mut o = obj();
        o.insert("row".into(), s(&self.row));
        o.insert("look".into(), s(&self.look));
        o.insert("scale".into(), s(&self.scale));
        o.insert("style".into(), s(&self.style));
        o.insert("editable".into(), Value::Bool(self.editable));
        o.insert("items".into(), Value::Array(self.items.iter().map(ItemOut::to_json).collect()));
        Value::Object(o)
    }
}

/// A layout row value (the layout's `opts.<island>.rows.<row>`, LAYOUT.md §3.9) resolved for drawing (ACTIONS.md §9.4):
/// `look` / `scale` as stored when valid, else the row kind's defaults; the first `limits.rowItems` items, each
/// through [`eval_item`]. Any value is accepted (not an object → no items, the defaults). The row kind must exist
/// (None otherwise; the reference's callers check it).
pub fn eval_row(app: App, snap: &Snapshot, row_kind: &str, row: &Value, context: Option<&JObj>) -> Option<RowOut> {
    let tbl = table();
    let rk = tbl.rows.get(row_kind)?;
    let ro = row.as_object();
    let sl = ro.and_then(|o| is_str(o.get("look")));
    let ss = ro.and_then(|o| is_str(o.get("scale")));
    let look = match sl {
        Some(l) if LOOKS.contains(&l) => l.to_string(),
        _ => rk.look.clone(),
    };
    let scale = match ss {
        Some(x) if SCALES.contains(&x) => x.to_string(),
        _ => rk.scale.clone(),
    };
    let raw: &[Value] = match ro.and_then(|o| o.get("items")) {
        Some(Value::Array(items)) => items,
        _ => &[],
    };
    let n = tbl.limits.get("rowItems").copied().unwrap_or(24.0) as usize;
    let w = Where { row: Some(row_kind), context, look: Some(&look), ..Where::default() };
    let items = raw.iter().take(n).map(|it| eval_item(app, snap, it, &w)).collect();
    Some(RowOut { row: row_kind.to_string(), look: look.clone(), scale, style: rk.style.clone(), editable: rk.editable, items })
}

/// The default keys of an action in an app (the table's shortcut specs; the app's own list wins), in table order.
pub fn default_keys(a: &Action, app: App) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for sc in a.shortcut.iter().flatten() {
        let own = if app == App::Web { sc.web.as_ref() } else { sc.desktop.as_ref() };
        for k in own.unwrap_or(&sc.keys) {
            if !out.contains(k) {
                out.push(k.clone());
            }
        }
    }
    out
}

/// One action of the drawer.
#[derive(Clone, Debug, PartialEq)]
pub struct DrawerItem {
    pub a: String,
    pub label: String,
    pub icon: Option<String>,
    pub pick: bool,
}

/// One group of the drawer.
#[derive(Clone, Debug, PartialEq)]
pub struct DrawerGroup {
    pub group: String,
    pub label: String,
    pub items: Vec<DrawerItem>,
}

impl DrawerGroup {
    pub fn to_json(&self) -> Value {
        let items = self
            .items
            .iter()
            .map(|it| {
                let mut o = obj();
                o.insert("a".into(), s(&it.a));
                o.insert("label".into(), s(&it.label));
                o.insert("icon".into(), os(it.icon.as_deref()));
                o.insert("pick".into(), Value::Bool(it.pick));
                Value::Object(o)
            })
            .collect();
        let mut o = obj();
        o.insert("group".into(), s(&self.group));
        o.insert("label".into(), s(&self.label));
        o.insert("items".into(), Value::Array(items));
        Value::Object(o)
    }
}

/// The edit-mode drawer for a row kind (ACTIONS.md §9.6): every action of the app that may go into the row — its
/// contexts provided by the row (or none), not hidden by `requires` in this snapshot — per group in the table's group
/// order, each group by folded label (code-point order), then id. Empty groups are left out. The row kind must exist
/// (None otherwise).
pub fn drawer(app: App, snap: &Snapshot, row_kind: &str, fold: &dyn Fn(&str) -> String) -> Option<Vec<DrawerGroup>> {
    let tbl = table();
    let rk = tbl.rows.get(row_kind)?;
    let mut out = Vec::new();
    for g in &tbl.groups {
        let mut items: Vec<(String, DrawerItem)> = Vec::new();
        for a in &tbl.actions {
            if a.group != g.id || !a.apps.iter().any(|x| x == app.as_str()) {
                continue;
            }
            if let Some(ctxs) = &a.contexts {
                if !ctxs.iter().any(|c| rk.contexts.contains(c)) {
                    continue;
                }
            }
            let p = present(&EvalInput { row: Some(row_kind), ..EvalInput::new(app, &a.id, snap) });
            if !p.visible {
                continue;
            }
            items.push((fold(&a.label), DrawerItem { a: a.id.clone(), label: a.label.clone(), icon: a.icon.clone(), pick: p.pick }));
        }
        items.sort_by(|x, y| x.0.cmp(&y.0).then_with(|| x.1.a.cmp(&y.1.a)));
        if !items.is_empty() {
            out.push(DrawerGroup { group: g.id.clone(), label: g.label.clone(), items: items.into_iter().map(|x| x.1).collect() });
        }
    }
    Some(out)
}

/// A submenu of a menu: shown when any of its items is.
#[derive(Clone, Debug, PartialEq)]
pub struct SubmenuOut {
    pub menu: String,
    /// shown | hidden
    pub state: &'static str,
    pub label: String,
    pub items: Vec<MenuEntry>,
}

/// An entry of a menu: an item (boxed: an item is much larger than a submenu), or a submenu.
#[derive(Clone, Debug, PartialEq)]
pub enum MenuEntry {
    Item(Box<ItemOut>),
    Sub(SubmenuOut),
}

impl MenuEntry {
    pub fn state(&self) -> &'static str {
        match self {
            MenuEntry::Item(i) => i.state,
            MenuEntry::Sub(sm) => sm.state,
        }
    }

    pub fn to_json(&self) -> Value {
        match self {
            MenuEntry::Item(i) => i.to_json(),
            MenuEntry::Sub(sm) => {
                let mut o = obj();
                o.insert("menu".into(), s(&sm.menu));
                o.insert("state".into(), s(sm.state));
                o.insert("label".into(), s(&sm.label));
                o.insert("items".into(), Value::Array(sm.items.iter().map(MenuEntry::to_json).collect()));
                Value::Object(o)
            }
        }
    }
}

/// A resolved menu.
#[derive(Clone, Debug, PartialEq)]
pub struct MenuOut {
    pub menu: String,
    pub items: Vec<MenuEntry>,
}

impl MenuOut {
    pub fn to_json(&self) -> Value {
        let mut o = obj();
        o.insert("menu".into(), s(&self.menu));
        o.insert("items".into(), Value::Array(self.items.iter().map(MenuEntry::to_json).collect()));
        Value::Object(o)
    }
}

/// A menu from the table (ACTIONS.md §9.7); a submenu is shown when any of its items is.
pub fn eval_menu(app: App, snap: &Snapshot, menu_id: &str, form: &str, context: Option<&JObj>, depth: usize) -> MenuOut {
    let tbl = table();
    let mut out = MenuOut { menu: menu_id.to_string(), items: Vec::new() };
    let Some(md) = tbl.menus.get(menu_id) else { return out };
    if depth > 4 {
        return out;
    }
    for it in &md.items {
        if let Some(m) = &it.menu {
            let sub = eval_menu(app, snap, m, form, context, depth + 1);
            let sd = tbl.menus.get(m);
            let shown = sub.items.iter().any(|x| matches!(x.state(), "shown" | "dimmed"));
            out.items.push(MenuEntry::Sub(SubmenuOut {
                menu: m.clone(),
                state: if shown { "shown" } else { "hidden" },
                label: sd.and_then(|d| d.label.clone()).unwrap_or_else(|| m.clone()),
                items: sub.items,
            }));
            continue;
        }
        let w = Where { menu: Some(menu_id), form: Some(form), context, ..Where::default() };
        out.items.push(MenuEntry::Item(Box::new(eval_item(app, snap, &it.raw, &w))));
    }
    out
}
