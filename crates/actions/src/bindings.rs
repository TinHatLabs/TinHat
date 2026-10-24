//! bindings.rs — port of `bindings.ts`: the key bindings in effect (ACTIONS.md §8.2–§8.8) — the table's defaults for
//! one app, which actions a person may give a key, the device-local user document (`tinhat-shortcuts`), the checks a
//! user binding must pass, the bindings in effect with the one each key press runs, the clashes Settings → Shortcuts
//! flags, the Settings list, and the edits it makes. Line for line; the shared vectors prove it.

use serde_json::Value;

use crate::js::{is_arr, is_obj, num, obj, s, sorted_keys, JObj};
use crate::keys::{is_function_key, parse_sequence, parse_str, reserved, web_class};
use crate::palette::fold;
use crate::table::{action_by_id, scope_chain, scopes_overlap, table, Action, App};

/// `Binding`.
#[derive(Clone, Debug)]
pub struct Binding {
    pub action: String,
    pub key: String,
    pub scope: String,
    pub typing: bool,
    pub fixed: bool,
    pub rank: f64,
    pub hold: bool,
    /// "default" | "user"
    pub source: &'static str,
    pub params: JObj,
    pub wins: bool,
}

impl Binding {
    pub fn to_json(&self) -> Value {
        let mut o = obj();
        o.insert("action".into(), s(&self.action));
        o.insert("key".into(), s(&self.key));
        o.insert("scope".into(), s(&self.scope));
        o.insert("typing".into(), Value::Bool(self.typing));
        o.insert("fixed".into(), Value::Bool(self.fixed));
        o.insert("rank".into(), num(self.rank));
        o.insert("hold".into(), Value::Bool(self.hold));
        o.insert("source".into(), s(self.source));
        o.insert("params".into(), Value::Object(self.params.clone()));
        o.insert("wins".into(), Value::Bool(self.wins));
        Value::Object(o)
    }
}

/// `Conflict`.
#[derive(Clone, Debug)]
pub struct Conflict {
    pub key: String,
    pub actions: [String; 2],
    pub scopes: [String; 2],
}

impl Conflict {
    pub fn to_json(&self) -> Value {
        let mut o = obj();
        o.insert("key".into(), s(&self.key));
        o.insert("actions".into(), Value::Array(self.actions.iter().map(|x| s(x)).collect()));
        o.insert("scopes".into(), Value::Array(self.scopes.iter().map(|x| s(x)).collect()));
        Value::Object(o)
    }
}

fn in_app(a: &Action, app: App) -> bool {
    a.apps.iter().any(|x| x == app.as_str())
}

fn limit(name: &str, def: f64) -> f64 {
    table().limits.get(name).copied().unwrap_or(def)
}

/// A shortcut spec's raw field.
fn spec_scope(raw: &Value) -> String {
    raw.get("scope").and_then(Value::as_str).unwrap_or("").to_string()
}
fn spec_flag(raw: &Value, k: &str) -> bool {
    matches!(raw.get(k), Some(Value::Bool(true)))
}

/// The table's default bindings for one app, in table order (action id, then shortcut spec, then key).
pub fn default_bindings(app: App) -> Vec<Binding> {
    let mut out = Vec::new();
    for a in &table().actions {
        if !in_app(a, app) {
            continue;
        }
        let Some(specs) = &a.shortcut else { continue };
        for sp in specs {
            let own = match app {
                App::Web => sp.web.as_ref(),
                App::Desktop => sp.desktop.as_ref(),
            };
            for k in own.unwrap_or(&sp.keys) {
                let Ok(p) = parse_str(k) else { continue };
                let mut params = obj();
                if let Some(ps) = is_obj(sp.raw.get("params")) {
                    for pk in sorted_keys(ps) {
                        params.insert(pk.clone(), ps[pk].clone());
                    }
                }
                out.push(Binding {
                    action: a.id.clone(),
                    key: p.text,
                    scope: spec_scope(&sp.raw),
                    typing: spec_flag(&sp.raw, "typing"),
                    fixed: spec_flag(&sp.raw, "fixed"),
                    rank: sp.raw.get("rank").and_then(Value::as_f64).unwrap_or(0.0),
                    hold: a.hold,
                    source: "default",
                    params,
                    wins: false,
                });
            }
        }
    }
    out
}

/// May a person give this action a key (ACTIONS.md §8.3)?
pub fn assignable(a: &Action, app: App) -> bool {
    if !in_app(a, app) || a.contexts.as_ref().is_some_and(|c| !c.is_empty()) || a.pending.is_some() {
        return false;
    }
    if a.shortcut.iter().flatten().any(|sp| spec_flag(&sp.raw, "fixed")) {
        return false;
    }
    a.target.as_ref().is_none_or(|t| t.absent.is_some())
}

/// `BindingCheck`: Ok((key, scope, typing)) or the error code.
pub type BindingCheck = Result<(String, String, bool), &'static str>;

/// May `text` become a user binding of `action_id` in `app` (ACTIONS.md §8.4)?
pub fn check_user_binding(app: App, action_id: &str, text: Option<&Value>) -> BindingCheck {
    let Some(a) = action_by_id(action_id) else { return Err("unknown-action") };
    if !in_app(a, app) {
        return Err("other-app");
    }
    if !assignable(a, app) {
        return Err("not-assignable");
    }
    let Ok(p) = parse_sequence(text) else { return Err("bad-sequence") };
    if reserved().contains(&p.text) {
        return Err("reserved");
    }
    let plain_f = is_function_key(&p.key) && !p.ctrl && !p.alt && !p.shift && !p.meta;
    if a.hold {
        if !p.bare && !plain_f {
            return Err("hold-key");
        }
    } else if p.bare {
        return Err("hold-only");
    }
    let first = a.shortcut.as_ref().and_then(|v| v.first());
    let scope = first.map(|sp| spec_scope(&sp.raw)).unwrap_or_else(|| "app".into());
    if !a.hold && !scope.starts_with("window:") && !p.ctrl && !p.alt && !p.meta && !is_function_key(&p.key) {
        return Err("needs-modifier");
    }
    if app == App::Web && web_class(&p.text) == "reserved" {
        return Err("browser-reserved");
    }
    for b in default_bindings(app) {
        if b.fixed && b.key == p.text && scopes_overlap(&scope, &b.scope) {
            return Err("fixed-taken");
        }
    }
    let typing = first.map(|sp| spec_flag(&sp.raw, "typing")).unwrap_or(true);
    Ok((p.text, scope, typing))
}

fn check_json(r: &BindingCheck) -> Value {
    let mut o = obj();
    match r {
        Ok((key, scope, typing)) => {
            o.insert("ok".into(), Value::Bool(true));
            o.insert("key".into(), s(key));
            o.insert("scope".into(), s(scope));
            o.insert("typing".into(), Value::Bool(*typing));
        }
        Err(e) => {
            o.insert("ok".into(), Value::Bool(false));
            o.insert("error".into(), s(e));
        }
    }
    Value::Object(o)
}

/// `checkUserBinding` as the reference's result object.
pub fn check_binding_json(app: App, action_id: &str, text: Option<&Value>) -> Value {
    check_json(&check_user_binding(app, action_id, text))
}

/// `UserBindings`.
#[derive(Clone, Debug, Default)]
pub struct UserBindings {
    /// (action id, canonical sequences) in code-point order of the ids
    pub entries: Vec<(String, Vec<String>)>,
    pub unknown: JObj,
    pub notes: Vec<String>,
}

impl UserBindings {
    fn has(&self, id: &str) -> bool {
        self.entries.iter().any(|(x, _)| x == id)
    }
}

/// Read the device-local user document (ACTIONS.md §8.5). Never panics.
pub fn read_user_bindings(app: App, doc: Option<&Value>) -> UserBindings {
    let mut res = UserBindings::default();
    let doc = match doc {
        None | Some(Value::Null) => return res,
        Some(d) => d,
    };
    let Some(d) = doc.as_object() else {
        res.notes.push("not-object".into());
        return res;
    };
    if tinhat_theme::canon::canon_json(doc).len() as f64 > limit("bindingsBytes", 16384.0) {
        res.notes.push("too-large".into());
        return res;
    }
    if d.get("format").and_then(Value::as_str) != Some("tinhat-shortcuts") {
        res.notes.push("bad-format".into());
        return res;
    }
    if !matches!(d.get("version"), Some(Value::Number(n)) if n.as_f64() == Some(1.0)) {
        res.notes.push("bad-version".into());
        return res;
    }
    let Some(bs) = is_obj(d.get("bindings")) else {
        res.notes.push("bad-field bindings".into());
        return res;
    };
    let mut ids: Vec<&String> = sorted_keys(bs);
    let max_actions = limit("bindingsActions", 256.0) as usize;
    if ids.len() > max_actions {
        ids.truncate(max_actions);
        res.notes.push("too-many-actions".into());
    }
    let per = limit("bindingsPerAction", 4.0) as usize;
    for id in ids {
        let v = &bs[id];
        let a = action_by_id(id);
        let Some(a) = a.filter(|a| in_app(a, app)) else {
            res.unknown.insert(id.clone(), v.clone());
            res.notes.push(format!("{} {id}", if a.is_some() { "other-app" } else { "unknown-action" }));
            continue;
        };
        let _ = a;
        let Some(list) = is_arr(Some(v)) else {
            res.notes.push(format!("bad-entry {id}"));
            continue;
        };
        let mut list: &[Value] = list;
        if list.len() > per {
            list = &list[..per];
            res.notes.push(format!("too-many {id}"));
        }
        let mut keys: Vec<String> = Vec::new();
        for (i, x) in list.iter().enumerate() {
            let p = match parse_sequence(Some(x)) {
                Ok(p) => p,
                Err(_) => {
                    res.notes.push(format!("bad-sequence {id} {i}"));
                    continue;
                }
            };
            if keys.contains(&p.text) {
                res.notes.push(format!("duplicate {id} {}", p.text));
                continue;
            }
            if let Err(e) = check_user_binding(app, id, Some(&Value::String(p.text.clone()))) {
                res.notes.push(format!("{e} {id} {}", p.text));
                continue;
            }
            keys.push(p.text);
        }
        // a list whose every key failed is not a choice of "no key": the entry is dropped and the defaults stay
        if !list.is_empty() && keys.is_empty() {
            res.notes.push(format!("no-usable-key {id}"));
            continue;
        }
        res.entries.push((id.clone(), keys));
    }
    res
}

/// `Effective`.
#[derive(Clone, Debug, Default)]
pub struct Effective {
    pub bindings: Vec<Binding>,
    pub conflicts: Vec<Conflict>,
    pub notes: Vec<String>,
    pub unknown: JObj,
}

impl Effective {
    pub fn to_json(&self) -> Value {
        let mut o = obj();
        o.insert("bindings".into(), Value::Array(self.bindings.iter().map(Binding::to_json).collect()));
        o.insert("conflicts".into(), Value::Array(self.conflicts.iter().map(Conflict::to_json).collect()));
        o.insert("notes".into(), Value::Array(self.notes.iter().map(|n| s(n)).collect()));
        o.insert("unknown".into(), Value::Object(self.unknown.clone()));
        Value::Object(o)
    }
}

/// The dispatch order of two bindings that share a key (§8.6): rank, a person's own before a default, action id, then
/// list order.
fn before(x: &Binding, xi: usize, y: &Binding, yi: usize) -> bool {
    if x.rank != y.rank {
        return x.rank < y.rank;
    }
    if x.source != y.source {
        return x.source == "user";
    }
    if x.action != y.action {
        return x.action < y.action;
    }
    xi < yi
}

/// The bindings in effect (ACTIONS.md §8.6).
pub fn effective_bindings(app: App, doc: Option<&Value>) -> Effective {
    let user = read_user_bindings(app, doc);
    let mut out: Vec<Binding> = Vec::new();
    for b in default_bindings(app) {
        if !b.fixed && user.has(&b.action) {
            continue;
        }
        out.push(b);
    }
    for (id, keys) in &user.entries {
        let Some(a) = action_by_id(id) else { continue };
        for k in keys {
            let Ok((key, scope, typing)) = check_user_binding(app, id, Some(&Value::String(k.clone()))) else { continue };
            out.push(Binding { action: id.clone(), key, scope, typing, fixed: false, rank: 0.0, hold: a.hold, source: "user", params: obj(), wins: false });
        }
    }
    let mut conflicts = Vec::new();
    let n = out.len();
    for i in 0..n {
        let mut wins = true;
        for j in 0..n {
            if i == j {
                continue;
            }
            let (x, y) = (&out[i], &out[j]);
            if x.key != y.key || x.action == y.action || !scopes_overlap(&x.scope, &y.scope) {
                continue;
            }
            if before(y, j, x, i) {
                wins = false;
            }
            if j > i && !(x.fixed && y.fixed) {
                conflicts.push(Conflict { key: x.key.clone(), actions: [x.action.clone(), y.action.clone()], scopes: [x.scope.clone(), y.scope.clone()] });
            }
        }
        out[i].wins = wins;
    }
    Effective { bindings: out, conflicts, notes: user.notes, unknown: user.unknown }
}

/// The actions a key press runs, in dispatch order (§8.7).
pub fn match_key(bindings: &[Binding], key: &str, scope: &str, typing: bool) -> Vec<String> {
    let chain = scope_chain(scope);
    let mut list: Vec<(usize, usize)> = Vec::new(); // (binding index, chain index)
    for (i, b) in bindings.iter().enumerate() {
        // a clash's loser never runs (the hosts give a key to its winner only, §8.6); fixed ones stay (focus decides)
        if b.key != key || b.hold || (!b.fixed && !b.wins) {
            continue;
        }
        let Some(ci) = chain.iter().position(|c| *c == b.scope) else { continue };
        if typing && !b.typing {
            continue;
        }
        list.push((i, ci));
    }
    list.sort_by(|&(xi, xc), &(yi, yc)| {
        xc.cmp(&yc).then_with(|| {
            if before(&bindings[xi], xi, &bindings[yi], yi) {
                std::cmp::Ordering::Less
            } else if before(&bindings[yi], yi, &bindings[xi], xi) {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        })
    });
    let mut out: Vec<String> = Vec::new();
    for (i, _) in list {
        if !out.contains(&bindings[i].action) {
            out.push(bindings[i].action.clone());
        }
    }
    out
}

/// The user document after one edit (ACTIONS.md §8.8), or None when it holds no entry.
pub fn set_user_binding(app: App, doc: Option<&Value>, action_id: &str, keys: Option<&[Value]>) -> Option<Value> {
    let user = read_user_bindings(app, doc);
    // (id → value) in code-point order at the end
    let mut entries: Vec<(String, Value)> = Vec::new();
    let put = |entries: &mut Vec<(String, Value)>, id: &str, v: Value| {
        if let Some(e) = entries.iter_mut().find(|(x, _)| x == id) {
            e.1 = v;
        } else {
            entries.push((id.to_string(), v));
        }
    };
    for (id, v) in &user.unknown {
        put(&mut entries, id, v.clone());
    }
    for (id, ks) in &user.entries {
        put(&mut entries, id, Value::Array(ks.iter().map(|k| s(k)).collect()));
    }
    if let Some(a) = action_by_id(action_id).filter(|a| in_app(a, app) && assignable(a, app)) {
        let _ = a;
        match keys {
            None => entries.retain(|(x, _)| x != action_id),
            Some(ks) => {
                let per = limit("bindingsPerAction", 4.0) as usize;
                let mut list: Vec<String> = Vec::new();
                for k in ks {
                    if let Ok((key, _, _)) = check_user_binding(app, action_id, Some(k)) {
                        if !list.contains(&key) && list.len() < per {
                            list.push(key);
                        }
                    }
                }
                let defaults: Vec<String> = default_bindings(app).into_iter().filter(|b| b.action == action_id).map(|b| b.key).collect();
                if !ks.is_empty() && list.is_empty() {
                    // keys whose every one failed change nothing (only an empty list means "no key")
                } else if list == defaults {
                    entries.retain(|(x, _)| x != action_id);
                } else {
                    put(&mut entries, action_id, Value::Array(list.iter().map(|k| s(k)).collect()));
                }
            }
        }
    }
    if entries.is_empty() {
        return None;
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let n = entries.len();
    let mut bs = obj();
    for (id, v) in entries {
        bs.insert(id, v);
    }
    let mut o = obj();
    o.insert("bindings".into(), Value::Object(bs));
    o.insert("format".into(), s("tinhat-shortcuts"));
    o.insert("version".into(), num(1.0));
    let out = Value::Object(o);
    // an edit may not take the document past what a reader keeps (§8.5): it changes nothing then
    if n as f64 > limit("bindingsActions", 256.0) || tinhat_theme::canon::canon_json(&out).len() as f64 > limit("bindingsBytes", 16384.0) {
        return doc.filter(|d| d.is_object()).cloned();
    }
    Some(out)
}

/// Settings → Shortcuts (ACTIONS.md §8.8): `{groups: [{group, label, items}], conflicts, notes}`.
pub fn shortcuts_view(app: App, doc: Option<&Value>) -> Value {
    let eff = effective_bindings(app, doc);
    let user = read_user_bindings(app, doc);
    let defaults_all = default_bindings(app);
    let mut groups: Vec<Value> = Vec::new();
    for g in &table().groups {
        let mut acts: Vec<(&Action, String)> = table().actions.iter().filter(|a| a.group == g.id && assignable(a, app)).map(|a| (a, fold(&a.label))).collect();
        acts.sort_by(|x, y| x.1.cmp(&y.1).then_with(|| x.0.id.cmp(&y.0.id)));
        let mut items: Vec<Value> = Vec::new();
        for (a, _) in acts {
            let mut keys: Vec<String> = Vec::new();
            for b in &eff.bindings {
                if b.action == a.id && !keys.contains(&b.key) {
                    keys.push(b.key.clone());
                }
            }
            let mut defaults: Vec<String> = Vec::new();
            for b in &defaults_all {
                if b.action == a.id && !defaults.contains(&b.key) {
                    defaults.push(b.key.clone());
                }
            }
            let mut clashes: Vec<Value> = Vec::new();
            for k in &keys {
                let mut w: Vec<String> = Vec::new();
                for c in &eff.conflicts {
                    if c.key != *k {
                        continue;
                    }
                    let other = if c.actions[0] == a.id {
                        Some(&c.actions[1])
                    } else if c.actions[1] == a.id {
                        Some(&c.actions[0])
                    } else {
                        None
                    };
                    if let Some(o) = other {
                        if !w.contains(o) {
                            w.push(o.clone());
                        }
                    }
                }
                if !w.is_empty() {
                    let mut c = obj();
                    c.insert("key".into(), s(k));
                    c.insert("with".into(), Value::Array(w.iter().map(|x| s(x)).collect()));
                    clashes.push(Value::Object(c));
                }
            }
            let by_app = a.icon_by_app.as_ref().and_then(|m| m.get(app.as_str()));
            let icon = match by_app {
                Some(v) => v.clone(),
                None => a.icon.as_deref().map(s).unwrap_or(Value::Null),
            };
            let browser: Vec<Value> = if app == App::Web { keys.iter().filter(|k| web_class(k) == "browser").map(|k| s(k)).collect() } else { Vec::new() };
            let mut it = obj();
            it.insert("a".into(), s(&a.id));
            it.insert("label".into(), s(&a.label));
            it.insert("icon".into(), icon);
            it.insert("keys".into(), Value::Array(keys.iter().map(|k| s(k)).collect()));
            it.insert("defaults".into(), Value::Array(defaults.iter().map(|k| s(k)).collect()));
            it.insert("changed".into(), Value::Bool(user.has(&a.id)));
            it.insert("hold".into(), Value::Bool(a.hold));
            it.insert("clashes".into(), Value::Array(clashes));
            it.insert("browser".into(), Value::Array(browser));
            items.push(Value::Object(it));
        }
        if !items.is_empty() {
            let mut go = obj();
            go.insert("group".into(), s(&g.id));
            go.insert("label".into(), s(&g.label));
            go.insert("items".into(), Value::Array(items));
            groups.push(Value::Object(go));
        }
    }
    let mut o = obj();
    o.insert("groups".into(), Value::Array(groups));
    o.insert("conflicts".into(), num(eff.conflicts.len() as f64));
    o.insert("notes".into(), Value::Array(user.notes.iter().map(|n| s(n)).collect()));
    Value::Object(o)
}
