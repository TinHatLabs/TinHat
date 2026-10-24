//! api.rs — port of `api.ts`: the actions engine as JSON in / JSON out (ACTIONS.md §11): the functions the shared
//! vectors call, and the shape the desktop's `ActionsController` answers with. Every function takes one JSON value and
//! returns one; none panics; a malformed input answers `{"error": "<code>"}` — the reference's codes.

use std::sync::OnceLock;

use serde_json::Value;

use crate::eval::{default_keys, drawer as drawer_of, eval_menu, eval_row, present, validate_target, EvalInput, TargetCheck};
use crate::js::{is_int, is_obj, is_str, is_true, num, obj, os, s, JObj};
use crate::palette as pal;
use crate::table::{action_by_id, table, App};
use crate::{bindings, keys};

/// Every entry point by name (the vector files' `kind`), in the reference's `API` order.
pub const KINDS: [&str; 17] = [
    "sequence", "checkBinding", "effective", "matchKey", "setBinding", "shortcuts", "evaluate", "row", "menu", "place", "target",
    "palette", "choices", "fold", "score", "drawer", "summary",
];

/// The entry point named `kind` on `input`, or None when there is no such entry point.
pub fn dispatch(kind: &str, input: &Value) -> Option<Value> {
    Some(match kind {
        "sequence" => sequence(input),
        "checkBinding" => check_binding(input),
        "effective" => effective(input),
        "matchKey" => match_key(input),
        "setBinding" => set_binding(input),
        "shortcuts" => shortcuts(input),
        "evaluate" => evaluate(input),
        "row" => row(input),
        "menu" => menu(input),
        "place" => place(input),
        "target" => target(input),
        "palette" => palette(input),
        "choices" => choices(input),
        "fold" => fold(input),
        "score" => score(input),
        "drawer" => drawer(input),
        "summary" => summary(input),
        _ => return None,
    })
}

fn err(code: &str) -> Value {
    let mut o = obj();
    o.insert("error".into(), s(code));
    Value::Object(o)
}

fn as_app(v: Option<&Value>) -> Option<App> {
    is_str(v).and_then(App::parse)
}

fn as_form(v: Option<&Value>) -> Option<&str> {
    is_str(v).filter(|f| *f == "desktop" || *f == "phone")
}

fn empty() -> &'static JObj {
    static E: OnceLock<JObj> = OnceLock::new();
    E.get_or_init(JObj::new)
}

/// `isObj(v) ? v : obj()`.
fn as_snapshot(v: Option<&Value>) -> &JObj {
    is_obj(v).unwrap_or_else(|| empty())
}

/// evaluate {app, action, snapshot?, target?, context?, params?, row?, menu?, item?, form?, transient?} → the
/// Presentation (ACTIONS.md §7). `menu` + `item` (an index) take the action, target, label, when and forms from that
/// menu item.
pub fn evaluate(input: &Value) -> Value {
    let Some(input) = input.as_object() else { return err("bad-input") };
    let Some(app) = as_app(input.get("app")) else { return err("bad-app") };
    let mut ev = EvalInput::new(app, "", as_snapshot(input.get("snapshot")));
    if let Some(menu_id) = input.get("menu") {
        let Value::String(menu_id) = menu_id else { return err("bad-menu") };
        let Some(md) = table().menus.get(menu_id) else { return err("unknown-menu") };
        let Some(idx) = is_int(input.get("item")).filter(|&i| i >= 0.0 && i < md.items.len() as f64) else {
            return err("bad-item");
        };
        let it = &md.items[idx as usize];
        let Some(a) = is_str(it.raw.get("a")) else { return err("not-an-action") };
        ev.action = a;
        ev.menu = Some(menu_id);
        ev.item = Some(&it.raw);
        if let Some(t) = it.raw.get("target") {
            ev.target = Some(t);
        }
        if let Some(form) = as_form(input.get("form")) {
            ev.form = Some(form);
        }
    } else {
        let Some(a) = is_str(input.get("action")) else { return err("bad-action") };
        ev.action = a;
        if let Some(t) = input.get("target") {
            ev.target = Some(t);
        }
        if let Some(row) = input.get("row") {
            match row {
                Value::String(r) if table().rows.contains_key(r) => ev.row = Some(r),
                _ => return err("unknown-row"),
            }
        }
        if is_true(input.get("transient")) {
            ev.transient = true;
        }
    }
    if let Some(ctx) = is_obj(input.get("context")) {
        ev.context = Some(ctx);
    }
    if let Some(params) = is_obj(input.get("params")) {
        ev.params = Some(params);
    }
    present(&ev).to_json()
}

/// row {app, snapshot?, row, value, context?} → the RowOut (ACTIONS.md §9.4): a layout row value resolved for drawing.
pub fn row(input: &Value) -> Value {
    let Some(input) = input.as_object() else { return err("bad-input") };
    let Some(app) = as_app(input.get("app")) else { return err("bad-app") };
    let Some(rk) = is_str(input.get("row")).filter(|r| table().rows.contains_key(*r)) else { return err("unknown-row") };
    let value = input.get("value").unwrap_or(&Value::Null);
    match eval_row(app, as_snapshot(input.get("snapshot")), rk, value, is_obj(input.get("context"))) {
        Some(r) => r.to_json(),
        None => err("unknown-row"),
    }
}

/// menu {app, snapshot?, menu, form, context?} → the MenuOut (ACTIONS.md §9.7).
pub fn menu(input: &Value) -> Value {
    let Some(input) = input.as_object() else { return err("bad-input") };
    let Some(app) = as_app(input.get("app")) else { return err("bad-app") };
    let Some(m) = is_str(input.get("menu")).filter(|m| table().menus.contains_key(*m)) else { return err("unknown-menu") };
    let Some(form) = as_form(input.get("form")) else { return err("bad-form") };
    eval_menu(app, as_snapshot(input.get("snapshot")), m, form, is_obj(input.get("context")), 0).to_json()
}

/// place {row, item} → {ok: true, item} | {ok: false, error, key?} — may this item be put into a row of this kind
/// (edit mode; ACTIONS.md §9.3)? The first failure: unknown-row, bad-item, unknown-action, context (a contextOnly
/// action in a row that provides none of its contexts), bad-flag (hideWhenOff / sepBefore not boolean), then the
/// target's (validate_target). The answer's item is the normalized item: {a, target? (only when not empty),
/// hideWhenOff? / sepBefore? (only when true)}.
pub fn place(input: &Value) -> Value {
    let Some(input) = input.as_object() else { return err("bad-input") };
    let fail = |error: &str, key: Option<&str>| -> Value {
        let mut o = obj();
        o.insert("ok".into(), Value::Bool(false));
        o.insert("error".into(), s(error));
        if let Some(k) = key {
            o.insert("key".into(), s(k));
        }
        Value::Object(o)
    };
    let Some(row_def) = is_str(input.get("row")).and_then(|r| table().rows.get(r)) else { return fail("unknown-row", None) };
    let Some(item) = is_obj(input.get("item")) else { return fail("bad-item", None) };
    let Some(id) = is_str(item.get("a")) else { return fail("bad-item", None) };
    let Some(a) = action_by_id(id) else { return fail("unknown-action", None) };
    if let Some(ctxs) = &a.contexts {
        if !ctxs.iter().any(|c| row_def.contexts.contains(c)) {
            return fail("context", None);
        }
    }
    for f in ["hideWhenOff", "sepBefore"] {
        if item.get(f).is_some_and(|v| !v.is_boolean()) {
            return fail("bad-flag", Some(f));
        }
    }
    let t = match validate_target(a, item.get("target"), false) {
        TargetCheck::Ok(t) => t,
        TargetCheck::Err { error, key } => return fail(error, key.as_deref()),
    };
    let mut norm = obj();
    norm.insert("a".into(), s(&a.id));
    if !t.is_empty() {
        norm.insert("target".into(), Value::Object(t));
    }
    for f in ["hideWhenOff", "sepBefore"] {
        if is_true(item.get(f)) {
            norm.insert(f.into(), Value::Bool(true));
        }
    }
    let mut o = obj();
    o.insert("ok".into(), Value::Bool(true));
    o.insert("item".into(), Value::Object(norm));
    Value::Object(o)
}

/// target {action, target, transient?} → validate_target's answer (ACTIONS.md §5.6).
pub fn target(input: &Value) -> Value {
    let Some(input) = input.as_object() else { return err("bad-input") };
    let Some(a) = is_str(input.get("action")).and_then(action_by_id) else { return err("unknown-action") };
    validate_target(a, input.get("target"), is_true(input.get("transient"))).to_json()
}

/// palette {app, snapshot?, query?, stage?} → the PaletteOut (ACTIONS.md §10).
pub fn palette(input: &Value) -> Value {
    let Some(input) = input.as_object() else { return err("bad-input") };
    let Some(app) = as_app(input.get("app")) else { return err("bad-app") };
    let q = input.get("query");
    if q.is_some_and(|q| !q.is_string()) {
        return err("bad-query");
    }
    let st = input.get("stage");
    if st.is_some_and(|x| !x.is_string()) {
        return err("bad-stage");
    }
    pal::palette(app, as_snapshot(input.get("snapshot")), is_str(q).unwrap_or(""), is_str(st)).to_json()
}

/// choices {app, snapshot?, action} → {items: [...]} | {items: null} — stage 2's entries before ranking (§10.6).
pub fn choices(input: &Value) -> Value {
    let Some(input) = input.as_object() else { return err("bad-input") };
    let Some(app) = as_app(input.get("app")) else { return err("bad-app") };
    let Some(id) = is_str(input.get("action")) else { return err("bad-action") };
    let ch = pal::choice_entries(app, as_snapshot(input.get("snapshot")), id);
    let mut o = obj();
    o.insert("items".into(), ch.map(|c| Value::Array(pal::entries_out(&c))).unwrap_or(Value::Null));
    Value::Object(o)
}

/// fold {text} → {folded} (ACTIONS.md §10.3).
pub fn fold(input: &Value) -> Value {
    let Some(text) = input.as_object().and_then(|o| is_str(o.get("text"))) else { return err("bad-input") };
    let mut o = obj();
    o.insert("folded".into(), s(&pal::fold(text)));
    Value::Object(o)
}

/// score {text, query, as: name|keyword|alias} → {tier} (the ranking's parts, §10.4).
pub fn score(input: &Value) -> Value {
    let Some(input) = input.as_object() else { return err("bad-input") };
    let (Some(text), Some(query)) = (is_str(input.get("text")), is_str(input.get("query"))) else { return err("bad-input") };
    let t = pal::fold(text);
    let q = pal::fold(query);
    let tier = match is_str(input.get("as")) {
        Some("name") => pal::name_tier(&t, &q, true),
        Some("keyword") => pal::keyword_tier(&t, &q),
        Some("alias") => pal::alias_tier(&t, &q),
        _ => return err("bad-as"),
    };
    let mut o = obj();
    o.insert("tier".into(), num(tier));
    Value::Object(o)
}

/// drawer {app, snapshot?, row} → {groups: [...]} — the edit-mode drawer of a row kind (ACTIONS.md §9.6).
pub fn drawer(input: &Value) -> Value {
    let Some(input) = input.as_object() else { return err("bad-input") };
    let Some(app) = as_app(input.get("app")) else { return err("bad-app") };
    let Some(rk) = is_str(input.get("row")).filter(|r| table().rows.contains_key(*r)) else { return err("unknown-row") };
    let Some(groups) = drawer_of(app, as_snapshot(input.get("snapshot")), rk, &pal::fold) else { return err("unknown-row") };
    let mut o = obj();
    o.insert("groups".into(), Value::Array(groups.iter().map(|g| g.to_json()).collect()));
    Value::Object(o)
}

/// summary {app} → the per-app view of the table (ACTIONS.md §11): every action of the app in id order with its
/// group, placeable / contexts, target kind, toggle state, flags, icon, label, default keys and whether the palette
/// lists it.
pub fn summary(input: &Value) -> Value {
    let Some(input) = input.as_object() else { return err("bad-input") };
    let Some(app) = as_app(input.get("app")) else { return err("bad-app") };
    let mut actions: Vec<Value> = Vec::new();
    for a in &table().actions {
        if !a.apps.iter().any(|x| x == app.as_str()) {
            continue;
        }
        let by_app = is_str(a.icon_by_app.as_ref().and_then(|m| m.get(app.as_str())));
        let mut e = obj();
        e.insert("id".into(), s(&a.id));
        e.insert("group".into(), s(&a.group));
        e.insert("placeable".into(), Value::Bool(a.contexts.is_none()));
        e.insert("contexts".into(), Value::Array(a.contexts.iter().flatten().map(|c| s(c)).collect()));
        e.insert("target".into(), os(a.target.as_ref().map(|t| t.kind.as_str())));
        e.insert("toggle".into(), os(a.toggle.as_ref().map(|t| t.state.as_str())));
        e.insert("hold".into(), Value::Bool(a.hold));
        e.insert("danger".into(), Value::Bool(a.danger));
        e.insert("confirm".into(), os(a.confirm.as_ref().map(|c| c.kind.as_str())));
        e.insert("icon".into(), os(by_app.or(a.icon.as_deref())));
        e.insert("label".into(), s(&a.label));
        e.insert("keys".into(), Value::Array(default_keys(a, app).iter().map(|k| s(k)).collect()));
        e.insert("palette".into(), Value::Bool(a.contexts.is_none() && a.palette != Some(false) && !a.hold && a.pending.is_none()));
        e.insert("pending".into(), os(a.pending.as_deref()));
        actions.push(Value::Object(e));
    }
    let mut o = obj();
    o.insert("app".into(), s(app.as_str()));
    o.insert("actions".into(), Value::Array(actions));
    Value::Object(o)
}

// ---- key bindings (ACTIONS.md §8) -------------------------------------------------------------------------------

/// sequence {text} → {ok: true, text, key, ctrl, alt, shift, meta, bare} | {ok: false, error} (§8.1).
pub fn sequence(input: &Value) -> Value {
    let Some(input) = input.as_object() else { return err("bad-input") };
    keys::sequence_json(&keys::parse_sequence(input.get("text")))
}

/// checkBinding {app, action, key} → {ok: true, key, scope, typing} | {ok: false, error} (§8.4).
pub fn check_binding(input: &Value) -> Value {
    let Some(input) = input.as_object() else { return err("bad-input") };
    let Some(app) = as_app(input.get("app")) else { return err("bad-app") };
    let Some(a) = is_str(input.get("action")) else { return err("bad-action") };
    bindings::check_binding_json(app, a, input.get("key"))
}

/// effective {app, doc?} → {bindings, conflicts, notes, unknown} (§8.5, §8.6).
pub fn effective(input: &Value) -> Value {
    let Some(input) = input.as_object() else { return err("bad-input") };
    let Some(app) = as_app(input.get("app")) else { return err("bad-app") };
    bindings::effective_bindings(app, input.get("doc")).to_json()
}

/// matchKey {app, doc?, key, scope?, typing?} → {actions: [ids in dispatch order]} (§8.7); scope default main.
pub fn match_key(input: &Value) -> Value {
    let Some(input) = input.as_object() else { return err("bad-input") };
    let Some(app) = as_app(input.get("app")) else { return err("bad-app") };
    let Some(k) = is_str(input.get("key")) else { return err("bad-key") };
    let scope = match input.get("scope") {
        None => "main",
        Some(Value::String(sc)) => sc.as_str(),
        Some(_) => return err("bad-scope"),
    };
    let eff = bindings::effective_bindings(app, input.get("doc"));
    let mut o = obj();
    o.insert("actions".into(), Value::Array(bindings::match_key(&eff.bindings, k, scope, is_true(input.get("typing"))).iter().map(|x| s(x)).collect()));
    Value::Object(o)
}

/// setBinding {app, doc?, action, keys: [..] | null} → {doc: <the new document> | null} (§8.8).
pub fn set_binding(input: &Value) -> Value {
    let Some(input) = input.as_object() else { return err("bad-input") };
    let Some(app) = as_app(input.get("app")) else { return err("bad-app") };
    let Some(a) = is_str(input.get("action")) else { return err("bad-action") };
    let ks: Option<&[Value]> = match input.get("keys") {
        Some(Value::Null) => None,
        Some(Value::Array(v)) => Some(v.as_slice()),
        _ => return err("bad-keys"),
    };
    let mut o = obj();
    o.insert("doc".into(), bindings::set_user_binding(app, input.get("doc"), a, ks).unwrap_or(Value::Null));
    Value::Object(o)
}

/// shortcuts {app, doc?} → {groups: [{group, label, items}], conflicts, notes} — Settings → Shortcuts (§8.8).
pub fn shortcuts(input: &Value) -> Value {
    let Some(input) = input.as_object() else { return err("bad-input") };
    let Some(app) = as_app(input.get("app")) else { return err("bad-app") };
    bindings::shortcuts_view(app, input.get("doc"))
}
