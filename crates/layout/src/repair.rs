//! repair.rs — port of `repair.ts`: LAYOUT.md §5: any JSON text or value → a valid projection + notes (never
//! panics), and validate() = the notes repair would produce (minus the informational ones). The raw value is never
//! modified.

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use crate::form::{
    all_locs, detach, find_key, fix_size, floats_of, group_edge_col, hidden_index, hidden_of, insert_beside, is_container,
    is_island, is_known_type, is_node, is_stack, islands_in, key_of, kids_arr, kids_of, loc_of, place_by_home, r1ok, r2ok,
    ref_of, room_locs, t_of, timeline_col, Ids, Loc, Side, ANCHORS, STEP_TOS,
};
use crate::json::{canon_bytes, clamp_f, cp_len, is_json_value, js_str, mk, read_json, J};
use crate::normalize::normalize_in_place;
use crate::schema::{
    classic_desktop, classic_phone, def, def_of, is_id_syntax, is_id_syntax_j, is_known_ref, is_ref_syntax,
    is_ref_syntax_j, is_reserved_id, is_step_ref_syntax, is_main, kind_of, schema, REGION_ORDER,
};

/// `{form, notes}`
#[derive(Debug, Clone)]
pub struct RepairResult {
    pub form: J,
    pub notes: Vec<String>,
}

impl RepairResult {
    pub fn to_json(&self) -> J {
        mk(vec![("form", self.form.clone()), ("notes", J::strs(&self.notes))])
    }
}

/// Notes that report what was kept, not a change: they never make a form invalid.
pub const INFO_NOTES: [&str; 2] = ["unknown-island-kept", "unknown-node-kept"];
fn code_of(note: &str) -> &str {
    match note.find(' ') {
        Some(i) => &note[..i],
        None => note,
    }
}
fn change_notes(notes: Vec<String>) -> Vec<String> {
    notes.into_iter().filter(|n| !INFO_NOTES.contains(&code_of(n))).collect()
}

// ---- limits (LAYOUT.md §3.10) -----------------------------------------------------------------------------------
/// The first limit the value exceeds, in a fixed order; None when within every limit.
pub fn limit_violation(v: &J) -> Option<&'static str> {
    let lim = &schema().lim;
    if canon_bytes(v) > lim.form_bytes {
        return Some("too-large");
    }
    struct W {
        nodes: usize,
        depth: usize,
        children: bool,
        tabs: bool,
    }
    fn visit(n: Option<&J>, d: usize, w: &mut W) {
        let Some(n) = n.filter(|n| n.is_obj()) else { return };
        let lim = &schema().lim;
        w.nodes += 1;
        if d > w.depth {
            w.depth = d;
        }
        let Some(c) = n.get("c").filter(J::is_arr) else { return };
        let items = c.items();
        if items.len() > lim.children {
            w.children = true;
        }
        if n.is("t", "stack") && items.len() > lim.stack_tabs {
            w.tabs = true;
        }
        for k in &items {
            visit(Some(k), d + 1, w);
        }
    }
    let mut w = W { nodes: 0, depth: 0, children: false, tabs: false };
    visit(v.get("root").as_ref(), 1, &mut w);
    let fl = v.get("floats").filter(J::is_arr);
    if let Some(fl) = &fl {
        for x in fl.items() {
            if x.is_obj() {
                visit(x.get("node").as_ref(), 1, &mut w);
            }
        }
    }
    if w.depth > lim.depth {
        return Some("too-deep");
    }
    if w.nodes > lim.nodes {
        return Some("too-many-nodes");
    }
    if w.children {
        return Some("over-limit-children");
    }
    if w.tabs {
        return Some("over-limit-tabs");
    }
    if fl.as_ref().is_some_and(|fl| fl.len() > lim.floats) {
        return Some("over-limit-floats");
    }
    let mut buttons: BTreeSet<Rc<str>> = BTreeSet::new();
    fn add_ref(r: Option<J>, b: &mut BTreeSet<Rc<str>>) {
        if let Some(s) = r.and_then(|r| r.as_rc()) {
            if s.starts_with("buttons:") {
                b.insert(s);
            }
        }
    }
    fn walk_refs(n: Option<&J>, b: &mut BTreeSet<Rc<str>>) {
        let Some(n) = n.filter(|n| n.is_obj()) else { return };
        add_ref(n.get("i"), b);
        if let Some(c) = n.get("c").filter(J::is_arr) {
            for k in c.items() {
                walk_refs(Some(&k), b);
            }
        }
    }
    walk_refs(v.get("root").as_ref(), &mut buttons);
    if let Some(fl) = &fl {
        for x in fl.items() {
            if x.is_obj() {
                walk_refs(x.get("node").as_ref(), &mut buttons);
            }
        }
    }
    let hid = v.get("hidden").filter(J::is_arr);
    if let Some(hid) = &hid {
        for h in hid.items() {
            add_ref(if h.is_obj() { h.get("i") } else { Some(h.clone()) }, &mut buttons);
        }
    }
    if buttons.len() > lim.buttons_islands {
        return Some("over-limit-buttons");
    }
    if hid.as_ref().is_some_and(|h| h.len() > lim.hidden) {
        return Some("over-limit-hidden");
    }
    if v.get("collapse").filter(J::is_arr).is_some_and(|c| c.len() > lim.collapse) {
        return Some("over-limit-collapse");
    }
    None
}

// ---- opts (desktop and phone; LAYOUT.md §3.9, §5.5) -------------------------------------------------------------
/// `^[a-zA-Z][a-zA-Z0-9]{0,31}$`
fn widget_id(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty() && b.len() <= 32 && b[0].is_ascii_alphabetic() && b[1..].iter().all(|c| c.is_ascii_alphanumeric())
}
/// `^[a-z][a-zA-Z0-9.]{0,63}$`
fn action_id(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty() && b.len() <= 64 && b[0].is_ascii_lowercase() && b[1..].iter().all(|&c| c.is_ascii_alphanumeric() || c == b'.')
}
fn keys_beyond(o: &J, n: usize) -> Vec<String> {
    o.keys().into_iter().skip(n).collect()
}
fn is_one_of(v: Option<&J>, vals: &[&str]) -> bool {
    v.and_then(J::as_str).is_some_and(|s| vals.contains(&s))
}

/// Sanitize one island's options object in place; true when something changed.
fn fix_island_opts(v: &J) -> bool {
    let mut ch = false;
    for k in ["compact", "composerTop", "headerBottom"] {
        if v.has(k) && !v.get(k).is_some_and(|x| x.is_bool()) {
            v.del(k);
            ch = true;
        }
    }
    if v.has("orient") && !is_one_of(v.get("orient").as_ref(), &["v", "h"]) {
        v.del("orient");
        ch = true;
    }
    if v.has("widgets") {
        let w = v.get("widgets").unwrap_or_default();
        if !w.is_arr() {
            v.del("widgets");
            ch = true;
        } else {
            let mut kept = Vec::new();
            for it in w.items() {
                if !it.is_obj() || !it.get_str("w").is_some_and(|x| widget_id(&x)) {
                    ch = true;
                    continue;
                }
                if it.has("size") && !is_one_of(it.get("size").as_ref(), &["S", "M", "L"]) {
                    it.del("size");
                    ch = true;
                }
                if it.has("settings") {
                    let st = it.get("settings").unwrap_or_default();
                    if !st.is_obj() || st.nkeys() > 16 {
                        it.del("settings");
                        ch = true;
                    }
                }
                kept.push(it);
            }
            if kept.len() > 12 {
                kept.truncate(12);
                ch = true;
            }
            v.set("widgets", J::arr(kept));
        }
    }
    if v.has("rows") {
        let r = v.get("rows").unwrap_or_default();
        if !r.is_obj() {
            v.del("rows");
            ch = true;
        } else {
            for rk in r.keys() {
                let row = r.get(&rk).unwrap_or_default();
                if !row.is_obj() {
                    r.del(&rk);
                    ch = true;
                    continue;
                }
                if row.has("look") && !is_one_of(row.get("look").as_ref(), &["icon", "iconLabel", "label"]) {
                    row.del("look");
                    ch = true;
                }
                if row.has("scale") && !is_one_of(row.get("scale").as_ref(), &["S", "M"]) {
                    row.del("scale");
                    ch = true;
                }
                if row.has("items") {
                    let items = row.get("items").unwrap_or_default();
                    if !items.is_arr() {
                        row.del("items");
                        ch = true;
                    } else {
                        let mut kept = Vec::new();
                        for it in items.items() {
                            if !it.is_obj() || !it.get_str("a").is_some_and(|a| action_id(&a)) {
                                ch = true;
                                continue;
                            }
                            if it.has("target") {
                                let tg = it.get("target").unwrap_or_default();
                                if !tg.is_obj() || tg.nkeys() > 4 {
                                    it.del("target");
                                    ch = true;
                                }
                            }
                            if it.has("hideWhenOff") && !it.get("hideWhenOff").is_some_and(|x| x.is_bool()) {
                                it.del("hideWhenOff");
                                ch = true;
                            }
                            kept.push(it);
                        }
                        if kept.len() > 24 {
                            kept.truncate(24);
                            ch = true;
                        }
                        row.set("items", J::arr(kept));
                    }
                }
            }
            for rk in keys_beyond(&r, 8) {
                r.del(&rk);
                ch = true;
            }
        }
    }
    // at most 32 keys: the derived flags (composerTop, headerBottom) always fit — the other keys are cut to 30 first
    let others: Vec<String> = v.keys().into_iter().filter(|k| k != "composerTop" && k != "headerBottom").collect();
    for k in others.iter().skip(30) {
        v.del(k);
        ch = true;
    }
    ch
}

/// `opts` of a form: keys are island refs, values objects (≤ 32 entries). Notes `bad-opts-fixed <ref | opts>`.
fn fix_opts(f: &J, notes: &mut Vec<String>) {
    if !f.has("opts") {
        return;
    }
    let o = f.get("opts").unwrap_or_default();
    if !o.is_obj() {
        f.del("opts");
        notes.push("bad-opts-fixed opts".into());
        return;
    }
    for k in o.keys() {
        if !is_ref_syntax(&k) {
            o.del(&k);
            notes.push("bad-opts-fixed opts".into());
            continue;
        }
        let v = o.get(&k).unwrap_or_default();
        if !v.is_obj() {
            o.del(&k);
            notes.push(format!("bad-opts-fixed {k}"));
            continue;
        }
        if fix_island_opts(&v) {
            notes.push(format!("bad-opts-fixed {k}"));
        }
    }
    // at most 32 entries: `composer` and `header` (where N9 records the room order) always fit — the others are cut to 30
    let beyond: Vec<String> = o.keys().into_iter().filter(|k| k != "composer" && k != "header").skip(30).collect();
    for k in &beyond {
        o.del(k);
    }
    if !beyond.is_empty() {
        notes.push("bad-opts-fixed opts".into());
    }
}

// ---- float sizes (step 6) ------------------------------------------------------------------------------------
/// A float's w / h: invalid (not an object, `share`, two bases, a wrong-typed known field) → None; else clamped.
fn fix_float_size(v: &J) -> (Option<J>, bool) {
    if !v.is_obj() || v.has("share") {
        return (None, true);
    }
    if ["fixed", "auto", "pm"].iter().filter(|b| v.has(b)).count() > 1 {
        return (None, true);
    }
    if v.has("auto") && !v.get("auto").is_some_and(|a| a.is_true()) {
        return (None, true);
    }
    for k in ["fixed", "pm", "min", "max", "minPm", "maxPm"] {
        if v.has(k) && !v.get(k).is_some_and(|x| x.is_int()) {
            return (None, true);
        }
    }
    fix_size(Some(v))
}

// ---- the desktop form ------------------------------------------------------------------------------------------
fn classic(why: &str) -> RepairResult {
    RepairResult { form: classic_desktop(), notes: vec![format!("replaced-by-classic {why}")] }
}

/// Repair desktop-form JSON text (LAYOUT.md §5.1): not JSON data → Classic.
pub fn repair_desktop_text(text: &str) -> RepairResult {
    match read_json(text) {
        Some(v) => repair_desktop(Some(&v)),
        None => classic("not-json"),
    }
}

/// LAYOUT.md §5.1-§5.3: a valid projection of any value + the notes, in the order the steps produce them.
pub fn repair_desktop(value: Option<&J>) -> RepairResult {
    // §1.4: not JSON data (an absent value included) → Classic, `not-json`
    if !is_json_value(value) {
        return classic("not-json");
    }
    let Some(value) = value.filter(|v| v.is_obj()) else { return classic("not-object") };
    let root = value.get("root");
    if !root.as_ref().is_some_and(is_node) {
        return classic("no-root");
    }
    if let Some(lim) = limit_violation(value) {
        return classic(lim);
    }
    let f = value.deep();
    let mut notes = Vec::new();
    repair_in_place(&f, &mut notes);
    if let Some(lim) = limit_violation(&f) {
        return classic(lim);
    }
    RepairResult { form: f, notes }
}

/// validate(form) → the violations = the change notes repair would produce (LAYOUT.md §5.4).
pub fn validate_desktop(value: Option<&J>) -> Vec<String> {
    change_notes(repair_desktop(value).notes)
}

fn fix_int_field(o: &J, k: &str, lo: f64, hi: f64) -> bool {
    let Some(v) = o.get(k) else { return false };
    if !v.is_int() {
        o.del(k);
        return true;
    }
    let x = v.as_f64().unwrap_or(0.0);
    let c = clamp_f(x, lo, hi);
    if c != x {
        o.set(k, J::Num(c));
        return true;
    }
    false
}

struct Walk<'a> {
    notes: &'a mut Vec<String>,
    seen: BTreeSet<String>,
    seen_ids: BTreeSet<String>,
    all_refs: BTreeSet<String>,
    ids: Ids,
}

impl Walk<'_> {
    fn note(&mut self, n: &str, s: &str) {
        self.notes.push(format!("{n} {s}"));
    }
    fn fix_id(&mut self, n: &J, prefix: &str) {
        let id = n.get("id");
        let ids = id.as_ref().and_then(J::as_str).map(str::to_string);
        let why = match &ids {
            None => Some("id-reassigned"),
            Some(s) if !is_id_syntax(s) => Some("id-reassigned"),
            Some(s) if is_reserved_id(s) => Some("renamed-reserved-id"),
            Some(s) if self.seen_ids.contains(s) || self.all_refs.contains(s) => Some("id-reassigned"),
            Some(_) => None,
        };
        let Some(why) = why else {
            self.seen_ids.insert(ids.expect("valid"));
            return;
        };
        let nid = self.ids.next(prefix);
        self.seen_ids.insert(nid.clone());
        n.set("id", J::s(&nid));
        let subj = if why == "renamed-reserved-id" { js_str(id.as_ref()) } else { nid };
        self.note(why, &subj);
    }
    fn fix_node_size(&mut self, n: &J, subject: &str) {
        if !n.has("size") {
            return;
        }
        let (size, changed) = fix_size(n.get("size").as_ref());
        if !changed {
            return;
        }
        match size {
            Some(s) => n.set("size", s),
            None => n.del("size"),
        }
        self.note("bad-size-fixed", subject);
    }
    fn walk_kids(&mut self, n: &J, subject: &str) {
        let mut kept = Vec::new();
        for k in kids_of(n) {
            if let Some(r) = self.walk(&k, subject) {
                kept.push(r);
            }
        }
        n.set("c", J::arr(kept));
    }
    fn walk(&mut self, n: &J, parent_subject: &str) -> Option<J> {
        if !is_node(n) {
            self.note("dropped-invalid", parent_subject);
            return None;
        }
        let t = t_of(n).expect("a node");
        if &*t == "island" {
            let Some(r) = n.get_str("i").filter(|r| is_ref_syntax(r)) else {
                self.note("dropped-invalid", parent_subject);
                return None;
            };
            if self.seen.contains(&*r) {
                self.note("dropped-duplicate", &r);
                return None;
            }
            self.seen.insert(r.to_string());
            if !is_known_ref(&r) {
                self.note("unknown-island-kept", &r);
            } else {
                self.fix_node_size(n, &r);
            }
            return Some(n.clone());
        }
        if matches!(&*t, "row" | "col" | "stack") {
            self.fix_id(n, if &*t == "stack" { "s" } else { "n" });
            let id = js_str(n.get("id").as_ref());
            if &*t != "stack" && fix_int_field(n, "gap", 0.0, 32.0) {
                self.note("bad-field-fixed", &id);
            }
            self.fix_node_size(n, &id);
            if !n.get("c").is_some_and(|c| c.is_arr()) {
                n.set("c", J::arr(vec![]));
                self.note("bad-field-fixed", &id);
            }
            self.walk_kids(n, &id);
            return Some(n.clone());
        }
        self.note("unknown-node-kept", &t);
        if n.get("c").is_some_and(|c| c.is_arr()) {
            self.walk_kids(n, &t);
        }
        Some(n.clone())
    }
}

fn take_hidden(f: &J, hi: usize) -> J {
    let leaf = f.get("hidden").and_then(|h| h.remove_at(hi)).unwrap_or_default();
    leaf.del("from");
    leaf
}

fn repair_in_place(f: &J, notes: &mut Vec<String>) {
    let note = |notes: &mut Vec<String>, n: &str, s: &str| notes.push(format!("{n} {s}"));
    // ---- step 2a: form-level fields (preset, gap, margin, density, floats, hidden, collapse, opts)
    if f.has("preset") && !f.get_str("preset").is_some_and(|p| cp_len(&p) <= 32) {
        f.del("preset");
        note(notes, "bad-field-fixed", "preset");
    }
    if fix_int_field(f, "gap", 0.0, 32.0) {
        note(notes, "bad-field-fixed", "gap");
    }
    if fix_int_field(f, "margin", 0.0, 64.0) {
        note(notes, "bad-field-fixed", "margin");
    }
    if f.has("density") && !is_one_of(f.get("density").as_ref(), &["compact", "normal", "touch"]) {
        f.del("density");
        note(notes, "bad-field-fixed", "density");
    }
    for k in ["floats", "hidden", "collapse"] {
        if f.has(k) && !f.get(k).is_some_and(|v| v.is_arr()) {
            f.del(k);
            note(notes, "bad-field-fixed", k);
        }
    }
    fix_opts(f, notes);

    // ---- step 2b: the root is a row or col (N5)
    let ids = Ids::new(f);
    let mut all_refs = BTreeSet::new();
    for l in all_locs(f) {
        if let Some(i) = l.node.get_str("i") {
            all_refs.insert(i.to_string());
        }
    }
    for h in hidden_of(f) {
        if let Some(s) = h.as_str() {
            all_refs.insert(s.to_string());
        } else if let Some(i) = h.get_str("i") {
            all_refs.insert(i.to_string());
        }
    }
    let mut w = Walk { notes, seen: BTreeSet::new(), seen_ids: BTreeSet::new(), all_refs, ids };
    let mut root = f.get("root").unwrap_or_default();
    if !is_container(&root) {
        let id = w.ids.body();
        root = mk(vec![("t", J::s("row")), ("id", J::s(&id)), ("c", J::arr(vec![root]))]);
        f.set("root", root.clone());
        w.note("root-wrapped", &id);
    }

    // ---- step 2c: walk the root depth-first, then the floats, then hidden
    w.fix_id(&root, "n");
    let rid = js_str(root.get("id").as_ref());
    if fix_int_field(&root, "gap", 0.0, 32.0) {
        w.note("bad-field-fixed", &rid);
    }
    w.fix_node_size(&root, &rid);
    if !root.get("c").is_some_and(|c| c.is_arr()) {
        root.set("c", J::arr(vec![]));
        w.note("bad-field-fixed", &rid);
    }
    w.walk_kids(&root, &rid);

    if f.has("floats") {
        let fl = f.get("floats").unwrap_or_default();
        let mut kept = Vec::new();
        for x in fl.items() {
            if !x.is_obj() {
                w.note("dropped-invalid", "floats");
                continue;
            }
            w.fix_id(&x, "f");
            let id = js_str(x.get("id").as_ref());
            let node = x.get("node");
            let r = match &node {
                Some(n) if is_node(n) => w.walk(n, &id),
                _ => None,
            };
            let Some(r) = r else {
                w.note("bad-float-fixed", &id);
                continue;
            };
            x.set("node", r);
            kept.push(x);
        }
        f.set("floats", J::arr(kept));
    }
    if f.has("hidden") {
        let hid = f.get("hidden").unwrap_or_default();
        let mut kept = Vec::new();
        for h in hid.items() {
            if let Some(hs) = h.as_str() {
                if !is_ref_syntax(hs) {
                    w.note("dropped-invalid", "hidden");
                    continue;
                }
                if w.seen.contains(hs) {
                    w.note("dropped-duplicate", hs);
                    continue;
                }
                w.seen.insert(hs.to_string());
                w.note("hidden-upgraded", hs);
                if !is_known_ref(hs) {
                    w.note("unknown-island-kept", hs);
                }
                kept.push(mk(vec![("t", J::s("island")), ("i", J::s(hs))]));
                continue;
            }
            if !h.is_obj() || !h.is("t", "island") || !is_ref_syntax_j(h.get("i").as_ref()) {
                w.note("dropped-invalid", "hidden");
                continue;
            }
            let r = h.get_str("i").expect("checked").to_string();
            if w.seen.contains(&r) {
                w.note("dropped-duplicate", &r);
                continue;
            }
            w.seen.insert(r.clone());
            if !is_known_ref(&r) {
                w.note("unknown-island-kept", &r);
            } else {
                w.fix_node_size(&h, &r);
            }
            if h.has("from") {
                let fr = h.get("from").unwrap_or_default();
                if !fr.is_obj() || !is_id_syntax_j(fr.get("parent").as_ref()) || !fr.get("index").is_some_and(|i| i.is_int()) {
                    h.del("from");
                    w.note("bad-field-fixed", &r);
                } else if fix_int_field(&fr, "index", 0.0, 15.0) {
                    w.note("bad-field-fixed", &r);
                }
            }
            kept.push(h);
        }
        f.set("hidden", J::arr(kept));
    }
    let Walk { notes, mut ids, .. } = w;

    // ---- step 3: stacks hold stackable islands only; the rest moves out, right after the stack
    let stacks: Vec<J> = all_locs(f).into_iter().filter(|l| is_stack(&l.node)).map(|l| l.node).collect();
    for s in &stacks {
        let mut keep = Vec::new();
        let mut out = Vec::new();
        for k in kids_of(s) {
            if is_island(&k) {
                let d = ref_of(&k).and_then(|r| def_of(&r));
                if d.is_none_or(|d| d.stackable) {
                    keep.push(k);
                    continue;
                }
            }
            let subj = key_of(&k).map(|x| x.to_string()).unwrap_or_else(|| js_str(t_of(&k).map(J::Str).as_ref()));
            note(notes, "unstacked", &subj);
            out.push(k);
        }
        if out.is_empty() {
            continue;
        }
        s.set("c", J::arr(keep));
        let Some(l) = loc_of(f, s) else { continue };
        if let Some(p) = &l.parent {
            if let Some(c) = kids_arr(p) {
                let i = c.index_of(s).map_or(0, |i| i + 1);
                c.insert_all(i, out);
            }
        } else if l.float >= 0 {
            if let Some(x) = floats_of(f).get(l.float as usize) {
                let mut kids = vec![s.clone()];
                kids.extend(out);
                x.set("node", mk(vec![("t", J::s("col")), ("id", J::s(&ids.next("n"))), ("c", J::arr(kids))]));
            }
        }
    }

    // ---- step 4: the room group (R1-R3)
    let partial = |g: &J| partial_ok(g);
    // 4a. the timeline: docked in the root tree
    {
        let hi = hidden_index(f, "timeline");
        let tl = find_key(f, "timeline");
        if let Some(hi) = hi {
            let leaf = take_hidden(f, hi);
            note(notes, "unhid-required", "timeline");
            place_by_home(f, &leaf, &mut ids, &partial);
        } else if let Some(tl) = tl.as_ref().filter(|t| t.float >= 0) {
            detach(f, tl);
            note(notes, "regrouped-room", "timeline");
            place_by_home(f, &tl.node, &mut ids, &partial);
        } else if tl.is_none() {
            note(notes, "restored-required", "timeline");
            place_by_home(f, &mk(vec![("t", J::s("island")), ("i", J::s("timeline"))]), &mut ids, &partial);
        }
    }
    // 4b. the composer: directly after the timeline in its col (a col is created around the timeline if needed)
    {
        let room = room_locs(f);
        let hi = hidden_index(f, "composer");
        let mut leaf: Option<J> = None;
        if let Some(hi) = hi {
            leaf = Some(take_hidden(f, hi));
            note(notes, "unhid-required", "composer");
        } else if room.composer.is_none() {
            leaf = Some(mk(vec![("t", J::s("island")), ("i", J::s("composer"))]));
            note(notes, "restored-required", "composer");
        } else if !r1ok(&room) {
            let c = room.composer.as_ref().expect("placed");
            detach(f, c);
            leaf = Some(c.node.clone());
            note(notes, "regrouped-room", "composer");
        }
        if let Some(leaf) = leaf {
            if let Some(tl) = find_key(f, "timeline") {
                insert_beside(f, &leaf, &tl, Side::Bottom, &mut ids);
            }
        }
    }
    // 4c. the header (when placed): first child of the nearest col containing the timeline's col (groupEdge top)
    {
        let room = room_locs(f);
        if let Some(h) = &room.header {
            if h.float >= 0 || !r2ok(&room) {
                detach(f, h);
                note(notes, "regrouped-room", "header");
                if let Some(col) = group_edge_col(&room_locs(f)) {
                    if let Some(c) = kids_arr(&col) {
                        c.unshift(h.node.clone());
                    }
                }
            }
        }
    }

    // ---- step 5: the call (required) and every other missing singleton island, by their home rules
    {
        if let Some(hi) = hidden_index(f, "call") {
            let leaf = take_hidden(f, hi);
            note(notes, "unhid-required", "call");
            place_by_home(f, &leaf, &mut ids, &partial);
        } else if find_key(f, "call").is_none() {
            note(notes, "restored-required", "call");
            place_by_home(f, &mk(vec![("t", J::s("island")), ("i", J::s("call"))]), &mut ids, &partial);
        }
        for k in REGION_ORDER {
            let d = def(k).expect("registry");
            if d.multi || d.required {
                continue;
            }
            if find_key(f, k).is_some() || hidden_index(f, k).is_some() {
                continue;
            }
            note(notes, "restored-missing", k);
            place_by_home(f, &mk(vec![("t", J::s("island")), ("i", J::s(k))]), &mut ids, &partial);
        }
    }

    // ---- step 6: floats' fields
    for x in floats_of(f) {
        let id = js_str(x.get("id").as_ref());
        let mut bad = false;
        if !is_one_of(x.get("anchor").as_ref(), &ANCHORS) {
            x.set("anchor", J::s("tl"));
            bad = true;
        }
        for k in ["dx", "dy"] {
            if fix_int_field(&x, k, -4096.0, 4096.0) {
                bad = true;
            }
        }
        for k in ["w", "h"] {
            let Some(v) = x.get(k) else { continue };
            let (size, changed) = fix_float_size(&v);
            match size {
                None => {
                    x.del(k);
                    bad = true;
                }
                Some(s) if changed => {
                    x.set(k, s);
                    bad = true;
                }
                Some(_) => {}
            }
        }
        if x.has("reveal") && !is_one_of(x.get("reveal").as_ref(), &["always", "hover"]) {
            x.set("reveal", J::s("always"));
            bad = true;
        }
        if bad {
            note(notes, "bad-float-fixed", &id);
        }
        if x.is("reveal", "hover") {
            if let Some(node) = x.get("node").filter(J::is_obj) {
                if islands_in(&node).iter().any(|r| def_of(r).is_some_and(|d| d.required)) {
                    x.set("reveal", J::s("always"));
                    note(notes, "reveal-fixed", &id);
                }
            }
        }
    }

    // ---- step 7: collapse steps
    if f.has("collapse") {
        let col = f.get("collapse").unwrap_or_default();
        let mut kept = Vec::new();
        let mut root_keys: BTreeMap<String, Loc> = BTreeMap::new();
        for l in all_locs(f) {
            if l.float < 0 && is_known_type(&l.node) {
                if let Some(k) = key_of(&l.node) {
                    root_keys.entry(k.to_string()).or_insert(l);
                }
            }
        }
        let mut seen_steps: BTreeSet<String> = BTreeSet::new();
        for st in col.items() {
            let r = if st.is_obj() { st.get_str("ref") } else { None };
            let valid_ref = r.as_ref().filter(|r| is_step_ref_syntax(r));
            let subj = valid_ref.map(|r| r.to_string()).unwrap_or_else(|| "collapse".into());
            if !st.is_obj() || valid_ref.is_none() || !is_one_of(st.get("to").as_ref(), &STEP_TOS) {
                note(notes, "bad-collapse-dropped", &subj);
                continue;
            }
            let r = valid_ref.expect("checked").to_string();
            let to = st.get_str("to").expect("checked");
            let Some(l) = root_keys.get(&r) else {
                note(notes, "bad-collapse-dropped", &r);
                continue;
            };
            if is_island(&l.node) {
                if def_of(&r).is_some_and(|d| !d.collapse_allows(&to)) {
                    note(notes, "bad-collapse-dropped", &r);
                    continue;
                }
            } else if &*to == "fold" {
                note(notes, "bad-collapse-dropped", &r);
                continue;
            }
            if islands_in(&l.node).iter().any(|x| is_main(x)) {
                note(notes, "collapse-unsafe-dropped", &r);
                continue;
            }
            if seen_steps.contains(&r) {
                note(notes, "dropped-duplicate", &r);
                continue;
            }
            seen_steps.insert(r.clone());
            let mut bad = false;
            for k in ["below", "w"] {
                if fix_int_field(&st, k, 0.0, 8192.0) {
                    bad = true;
                }
            }
            if bad {
                note(notes, "bad-field-fixed", &r);
            }
            kept.push(st);
        }
        f.set("collapse", J::arr(kept));
    }

    // ---- step 8: normalize
    let n = normalize_in_place(f);
    notes.extend(n.notes);
}

/// The placement rules a repair / home insertion must keep (the ones whose islands are placed).
pub fn partial_ok(f: &J) -> bool {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for l in all_locs(f) {
        if is_stack(&l.node) {
            for k in kids_of(&l.node) {
                if !is_island(&k) || ref_of(&k).and_then(|r| def_of(&r)).is_some_and(|d| !d.stackable) {
                    return false;
                }
            }
        }
        if !is_island(&l.node) {
            continue;
        }
        let r = js_str(l.node.get("i").as_ref());
        if seen.contains(&r) {
            return false;
        }
        seen.insert(r.clone());
        if l.float >= 0 && def_of(&r).is_some_and(|d| !d.floatable) {
            return false;
        }
    }
    let room = room_locs(f);
    if room.timeline.is_some() && room.composer.is_some() && !r1ok(&room) {
        return false;
    }
    if room.header.is_some() && timeline_col(&room).is_some() && !r2ok(&room) {
        return false;
    }
    true
}

// ---- the phone form (LAYOUT.md §5.6) ------------------------------------------------------------------------------
pub fn repair_phone_text(text: &str) -> RepairResult {
    match read_json(text) {
        Some(v) => repair_phone(Some(&v)),
        None => RepairResult { form: classic_phone(), notes: vec!["replaced-by-classic not-json".into()] },
    }
}

pub fn repair_phone(value: Option<&J>) -> RepairResult {
    if !is_json_value(value) {
        return RepairResult { form: classic_phone(), notes: vec!["replaced-by-classic not-json".into()] };
    }
    let Some(value) = value.filter(|v| v.is_obj()) else {
        return RepairResult { form: classic_phone(), notes: vec!["replaced-by-classic not-object".into()] };
    };
    if canon_bytes(value) > schema().lim.form_bytes {
        return RepairResult { form: classic_phone(), notes: vec!["replaced-by-classic too-large".into()] };
    }
    let f = value.deep();
    let mut notes: Vec<String> = Vec::new();
    let mut cols: [Vec<String>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    let mut misplaced: [Vec<String>; 2] = [Vec::new(), Vec::new()];
    let names = ["rail", "list", "hidden"];
    let col_idx = |p: &str| if p == "rail" { 0 } else { 1 };
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for (ki, k) in names.iter().enumerate() {
        let arr = f.get(k);
        let items = match &arr {
            Some(a) if a.is_arr() => a.items(),
            Some(_) => {
                notes.push(format!("bad-phone-fixed {k}"));
                Vec::new()
            }
            None => Vec::new(),
        };
        for r in items {
            let Some(r) = r.as_str().filter(|r| is_ref_syntax(r)).map(str::to_string) else {
                notes.push(format!("bad-phone-fixed {k}"));
                continue;
            };
            if seen.contains(&r) {
                notes.push(format!("dropped-duplicate {r}"));
                continue;
            }
            let d = def_of(&r);
            if !is_known_ref(&r) {
                seen.insert(r.clone());
                notes.push(format!("unknown-island-kept {r}"));
                cols[ki].push(r);
                continue;
            }
            let Some(phone) = d.and_then(|d| d.phone.as_deref()) else {
                notes.push(format!("bad-phone-fixed {r}"));
                continue;
            };
            seen.insert(r.clone());
            if *k != "hidden" && phone != *k {
                notes.push(format!("bad-phone-fixed {r}"));
                misplaced[col_idx(phone)].push(r);
                continue;
            }
            cols[ki].push(r);
        }
    }
    let [mr, ml] = misplaced;
    cols[0].extend(mr);
    cols[1].extend(ml);
    for k in REGION_ORDER {
        let d = def(k).expect("registry");
        let Some(phone) = d.phone.as_deref() else { continue };
        if d.multi || seen.contains(k) {
            continue;
        }
        cols[col_idx(phone)].push(k.to_string());
        notes.push(format!("restored-missing {k}"));
    }
    // overflow (§5.6, Q-PHONEFIX; nothing is ever dropped): a column over its limit gives up its last unknown ref, else
    // its last button section, until it fits; what it gives up moves to the end of `hidden` as one run, in the order it
    // had (one `bad-phone-fixed <ref>` each, in that order). `hidden` over its limit then → the Classic phone form
    // (`replaced-by-classic over-limit-hidden`): a save quarantines the original (§9.5), nothing is silently lost.
    let lims = [16usize, 16, 32];
    let is_btn = |r: &str| kind_of(r) == "buttons" && is_known_ref(r);
    for ki in 0..2 {
        let col = std::mem::take(&mut cols[ki]);
        let mut gone = vec![false; col.len()];
        let mut n_gone = 0usize;
        let last_free = |gone: &[bool], pred: &dyn Fn(&str) -> bool| (0..col.len()).rev().find(|&i| !gone[i] && pred(&col[i]));
        while col.len() - n_gone > lims[ki] {
            // (no candidate cannot happen: a column holds at most 3 known sections that are not button sections)
            let Some(i) = last_free(&gone, &|r| !is_known_ref(r)).or_else(|| last_free(&gone, &is_btn)) else { break };
            gone[i] = true;
            n_gone += 1;
        }
        let mut moved = Vec::new();
        for (i, r) in col.into_iter().enumerate() {
            if gone[i] {
                moved.push(r);
            } else {
                cols[ki].push(r);
            }
        }
        for r in moved {
            notes.push(format!("bad-phone-fixed {r}"));
            cols[2].push(r);
        }
    }
    if cols[2].len() > lims[2] {
        return RepairResult { form: classic_phone(), notes: vec!["replaced-by-classic over-limit-hidden".into()] };
    }
    for (ki, k) in names.iter().enumerate() {
        // a key that was absent stays absent while its column is empty
        if f.has(k) || !cols[ki].is_empty() {
            f.set(k, J::strs(&cols[ki]));
        }
    }
    fix_opts(&f, &mut notes);
    if canon_bytes(&f) > schema().lim.form_bytes {
        return RepairResult { form: classic_phone(), notes: vec!["replaced-by-classic too-large".into()] };
    }
    RepairResult { form: f, notes }
}

pub fn validate_phone(value: Option<&J>) -> Vec<String> {
    change_notes(repair_phone(value).notes)
}
