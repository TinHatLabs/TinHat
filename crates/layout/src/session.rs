//! session.rs — the edit session (EDITMODE-UI.md §1, §4.3, §8, §9): the desktop twin of the web's
//! `src/features/editmode/session.ts` — a pure state machine over the layout engine (no Qt, no clock, no I/O), so the
//! unit tests below mirror the web's `session.test.ts` case for case and both apps choose the same operations.
//!
//! A session edits ONE form (the desktop editor the desktop form, the phone editor the phone form); the other rides
//! along unchanged. Every change is an engine operation (or, for the phone form, a repaired phone form) that becomes the
//! new working copy and one undo step; Undo / Redo walk whole forms; Cancel throws everything away; Done answers the
//! edit ONCE (`finish`) — or None when nothing changed (the engine's `save` would answer noop).
//!
//! Host logic that CHOOSES operations (both apps must choose the same ones, EDITMODE-UI §4.3 / §8):
//!   pick_preset   applyPreset, then carry every placed island's button rows (opts.<ref>.rows), then the user's own
//!                 button islands and drawers (carry_own, 12.1.8)
//!   float_anchor  the host corner nearest to an island's centre
//!   dock_float    the island's registry home list as dock placements, first success wins
//!   reset_island  hide → strip size / from, from = the preset's parent + index → restore
//!
//! Not part of the engine port (it is host logic): the controller (crates/app `layout_ctl.rs`) holds one session per
//! window and hands QML its state as JSON.

use crate::form::{all_locs, find_key, floats_of, hidden_of, is_island, key_of};
use crate::json::{cmp_cp, mk, J};
use crate::ops::{apply_op, OpResult};
use crate::repair::{repair_phone, validate_desktop};
use crate::schema::{def, kind_of, preset_desktop, preset_phone};

/// The undo stack keeps at most this many steps (the oldest go first).
pub const UNDO_CAP: usize = 100;

/// Which form a session edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormKind {
    Desktop,
    Phone,
}

/// What Done hands to the save (engine `save` edit: {name, desktop, phone}).
#[derive(Clone, Debug)]
pub struct SessionResult {
    pub id: String,
    pub name: String,
    pub desktop: J,
    pub phone: J,
}

/// A step's answer: Ok, or the engine's error text.
pub type Step = Result<(), String>;

fn op(pairs: Vec<(&str, J)>) -> J {
    mk(pairs)
}

fn same(a: &J, b: &J) -> bool {
    a.canon() == b.canon()
}

#[derive(Clone, Debug)]
pub struct EditSession {
    pub kind: FormKind,
    pub id: String,
    pub start_name: String,
    /// the edited form as the session started (Previous, dirty)
    pub start: J,
    /// the form this session does not edit
    pub other: J,
    pub name: String,
    pub form: J,
    undos: Vec<J>,
    redos: Vec<J>,
    /// the session ended (Done or Cancel): every further call is refused
    pub closed: bool,
}

impl EditSession {
    pub fn new(id: &str, name: &str, desktop: &J, phone: &J, kind: FormKind) -> EditSession {
        let (start, other) = match kind {
            FormKind::Desktop => (desktop.deep(), phone.deep()),
            FormKind::Phone => (phone.deep(), desktop.deep()),
        };
        EditSession {
            kind,
            id: id.to_string(),
            start_name: name.to_string(),
            name: name.to_string(),
            form: start.deep(),
            start,
            other,
            undos: Vec::new(),
            redos: Vec::new(),
            closed: false,
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.closed && !self.undos.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.closed && !self.redos.is_empty()
    }
    pub fn undo_depth(&self) -> usize {
        self.undos.len()
    }
    pub fn redo_depth(&self) -> usize {
        self.redos.len()
    }
    /// Anything to save: the working copy or the name differs from the start (canonically).
    pub fn dirty(&self) -> bool {
        !same(&self.form, &self.start) || self.name != self.start_name
    }

    /// A new working copy (already validated by the engine) as one undo step; an equal form is not a step.
    pub fn commit(&mut self, next: &J) -> bool {
        if self.closed || same(next, &self.form) {
            return false;
        }
        self.undos.push(std::mem::take(&mut self.form));
        if self.undos.len() > UNDO_CAP {
            self.undos.remove(0);
        }
        self.redos.clear();
        self.form = next.deep();
        true
    }

    /// One engine operation on the desktop working copy (LAYOUT §8) — the input is never changed on an error.
    pub fn apply(&mut self, o: &J, ctx: Option<&J>) -> Step {
        if self.closed {
            return Err("closed".into());
        }
        if self.kind != FormKind::Desktop {
            return Err("bad-op: bad request".into());
        }
        match apply_op(&self.form, Some(o), ctx) {
            OpResult::Err(e) => Err(e),
            OpResult::Ok { form, .. } => {
                self.commit(&form);
                Ok(())
            }
        }
    }

    /// A new phone form (the phone form has no operations): accepted only when repair leaves it as it is.
    pub fn set_phone(&mut self, next: &J) -> Step {
        if self.closed {
            return Err("closed".into());
        }
        if self.kind != FormKind::Phone {
            return Err("bad-op: bad request".into());
        }
        let r = repair_phone(Some(next));
        if !same(&r.form, next) {
            return Err("bad-op: bad request".into());
        }
        self.commit(&r.form);
        Ok(())
    }

    pub fn undo(&mut self) -> bool {
        if !self.can_undo() {
            return false;
        }
        let prev = self.undos.pop().expect("an undo step");
        self.redos.push(std::mem::replace(&mut self.form, prev));
        true
    }
    pub fn redo(&mut self) -> bool {
        if !self.can_redo() {
            return false;
        }
        let next = self.redos.pop().expect("a redo step");
        self.undos.push(std::mem::replace(&mut self.form, next));
        true
    }

    /// A preset (EDITMODE-UI §8; the phone editor: the preset's phone form) as one undo step.
    pub fn preset(&mut self, name: &str) -> Step {
        if self.closed {
            return Err("closed".into());
        }
        if self.kind == FormKind::Phone {
            let Some(p) = preset_phone(name) else { return Err("bad-op: bad request".into()) };
            self.commit(&p);
            return Ok(());
        }
        let f = pick_preset(&self.form, name)?;
        self.commit(&f);
        Ok(())
    }

    /// Reset (in edit mode, EDITMODE-UI §9) = the Classic preset, staged like every change.
    pub fn reset(&mut self) -> Step {
        self.preset("classic")
    }

    /// Previous (EDITMODE-UI §8) = the start form, as one new undo step.
    pub fn previous(&mut self) -> Step {
        if self.closed {
            return Err("closed".into());
        }
        let s = self.start.deep();
        self.commit(&s);
        Ok(())
    }

    /// A new name: trimmed, at most 40 code points; an empty name keeps the old one.
    pub fn rename(&mut self, name: &str) {
        if self.closed {
            return;
        }
        let n: String = name.trim().chars().take(40).collect();
        if !n.is_empty() {
            self.name = n;
        }
    }

    /// Done: the edit to save, once — None when nothing changed (no save at all).
    pub fn finish(&mut self) -> Option<SessionResult> {
        if self.closed {
            return None;
        }
        self.closed = true;
        if !self.dirty() {
            return None;
        }
        let (desktop, phone) = match self.kind {
            FormKind::Desktop => (self.form.deep(), self.other.deep()),
            FormKind::Phone => (self.other.deep(), self.form.deep()),
        };
        Some(SessionResult { id: self.id.clone(), name: self.name.clone(), desktop, phone })
    }

    /// Cancel: nothing is saved; the start is what renders again.
    pub fn cancel(&mut self) {
        self.closed = true;
    }

    /// The save of a Done failed (too large, …): the session goes on exactly as it was — start, history, dirty.
    pub fn reopen(&mut self) {
        self.closed = false;
    }
}

// ---- host logic that chooses operations (EDITMODE-UI §4.3, §8) ------------------------------------------------------

/// Every island ref placed in the root tree or a float (known node types), document order.
pub fn placed_refs(form: &J) -> Vec<String> {
    let mut out = Vec::new();
    for l in all_locs(form) {
        if is_island(&l.node) {
            if let Some(i) = l.node.get("i").and_then(|i| i.as_str().map(str::to_string)) {
                out.push(i);
            }
        }
    }
    out
}

/// The preset pick (§8): applyPreset, then setOpt(ref, "rows", …) for every placed island that had button rows.
pub fn pick_preset(form: &J, name: &str) -> Result<J, String> {
    let mut next = match apply_op(form, Some(&op(vec![("op", J::s("applyPreset")), ("preset", J::s(name))])), None) {
        OpResult::Err(e) => return Err(e),
        OpResult::Ok { form, .. } => form,
    };
    if let Some(opts) = form.get("opts").filter(J::is_obj) {
        let placed = placed_refs(&next);
        let mut refs = opts.keys();
        refs.sort_by(|a, b| cmp_cp(a, b));
        for r in refs {
            let Some(o) = opts.get(&r).filter(J::is_obj) else { continue };
            if !o.has("rows") || !placed.contains(&r) {
                continue;
            }
            let rows = o.get("rows").unwrap_or_default().deep();
            let set = op(vec![("op", J::s("setOpt")), ("ref", J::s(&r)), ("key", J::s("rows")), ("value", rows)]);
            if let OpResult::Ok { form, .. } = apply_op(&next, Some(&set), None) {
                next = form;
            }
        }
    }
    Ok(carry_own(form, next))
}

/// The pick's second half (12.1.8, EDITMODE-UI §8 "your own button islands come along"): the user's own button islands
/// (`buttons:*`) and drawers of the form before the pick, carried into the preset. A preset has no button islands, so
/// without this a pick dropped every Add button / Add drawer result. Three passes; a step the engine refuses, or one
/// whose form would not validate, is skipped (the form stays as it was before that step):
///   1. a HIDDEN own button island the new form lacks: hidden again, with its rows (a drawer's island, or one hidden by
///      hand — still restorable from Add island);
///   2. every drawer reachable from a PLACED own button island — its drawers, and the drawers of an island (with rows) in
///      one of them, and so on (breadth first from the placed ones in document order; row keys in code-point order; items in
///      order; each island once): the island it opens is hidden in the new form (the preset placed it) or put back into
///      `hidden` (it was hidden and the preset lacks it), and keeps its rows;
///   3. a PLACED own button island the new form doesn't place: back at its registry home (hidden + restore, like New
///      button island), with its rows.
/// The layout format is unchanged: everything here is an ordinary hidden list entry, placement or `rows` option.
fn carry_own(before: &J, mut next: J) -> J {
    let valid = |f: &J| validate_desktop(Some(f)).is_empty();
    let rows_of = |r: &str| before.get("opts").and_then(|o| o.get(r)).filter(J::is_obj).and_then(|o| o.get("rows"));
    let in_form = |f: &J, r: &str| placed_refs(f).iter().any(|x| x == r) || hidden_refs(f).iter().any(|x| x == r);
    let push_hidden = |f: &J, r: &str| -> Option<J> {
        let g = f.deep();
        let mut hid = hidden_of(&g);
        hid.push(mk(vec![("t", J::s("island")), ("i", J::s(r))]));
        g.set("hidden", J::arr(hid));
        if valid(&g) {
            Some(g)
        } else {
            None
        }
    };
    let set_rows = |f: J, r: &str| -> J {
        let Some(rows) = rows_of(r) else { return f };
        let set = op(vec![("op", J::s("setOpt")), ("ref", J::s(r)), ("key", J::s("rows")), ("value", rows.deep())]);
        match apply_op(&f, Some(&set), None) {
            OpResult::Ok { form, .. } if valid(&form) => form,
            _ => f,
        }
    };
    // 1. hidden own button islands
    for r in hidden_refs(before) {
        if r.starts_with("buttons:") && !in_form(&next, &r) {
            if let Some(g) = push_hidden(&next, &r) {
                next = set_rows(g, &r);
            }
        }
    }
    // 2. the islands the drawers open, breadth first from the placed own button islands
    let placed_own: Vec<String> = placed_refs(before).into_iter().filter(|r| r.starts_with("buttons:")).collect();
    let mut queue: Vec<String> = placed_own.clone();
    let mut seen: Vec<String> = Vec::new();
    let mut targets: Vec<String> = Vec::new();
    let mut qi = 0;
    while qi < queue.len() {
        let b = queue[qi].clone();
        qi += 1;
        if seen.contains(&b) {
            continue;
        }
        seen.push(b.clone());
        let Some(rows) = rows_of(&b).filter(J::is_obj) else { continue };
        let mut keys = rows.keys();
        keys.sort_by(|x, y| cmp_cp(x, y));
        for k in keys {
            let Some(items) = rows.get(&k).and_then(|row| row.get("items")).filter(J::is_arr) else { continue };
            for it in items.items() {
                if !(it.is_obj() && it.is("a", DRAWER_ACTION)) {
                    continue;
                }
                let t = it.get("target").and_then(|t| t.get("island")).and_then(|i| i.as_str().map(str::to_string));
                if let Some(t) = t.filter(|t| !t.is_empty() && !targets.contains(t)) {
                    queue.push(t.clone()); // any island with rows may hold a drawer of its own
                    targets.push(t);
                }
            }
        }
    }
    for t in &targets {
        if placed_refs(&next).iter().any(|x| x == t) {
            if let OpResult::Ok { form, .. } = apply_op(&next, Some(&op(vec![("op", J::s("hide")), ("ref", J::s(t))])), None) {
                if valid(&form) {
                    next = form;
                }
            }
        } else if !hidden_refs(&next).iter().any(|x| x == t) && hidden_refs(before).iter().any(|x| x == t) {
            if let Some(g) = push_hidden(&next, t) {
                next = g;
            }
        }
        if hidden_refs(&next).iter().any(|x| x == t) {
            next = set_rows(next, t);
        }
    }
    // 3. placed own button islands, back at their registry home
    for r in &placed_own {
        if in_form(&next, r) {
            continue;
        }
        let g = next.deep();
        let mut hid = hidden_of(&g);
        hid.push(mk(vec![("t", J::s("island")), ("i", J::s(r))]));
        g.set("hidden", J::arr(hid));
        if let OpResult::Ok { form, .. } = apply_op(&g, Some(&op(vec![("op", J::s("restore")), ("ref", J::s(r))])), None) {
            if valid(&form) {
                next = set_rows(form, r);
            }
        }
    }
    next
}

/// A rect in host pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
}

/// The host corner nearest to a rect's centre (ties: tl, tr, bl, br).
pub fn float_anchor(r: Rect, host_w: i64, host_h: i64) -> &'static str {
    let cx = 2 * r.x + r.w; // doubled centre: integer maths
    let cy = 2 * r.y + r.h;
    let right = cx > host_w; // strictly past the middle → right (a tie stays left)
    let bottom = cy > host_h;
    match (bottom, right) {
        (true, true) => "br",
        (true, false) => "bl",
        (false, true) => "tr",
        (false, false) => "tl",
    }
}

/// The island's registry home list as placements (§4.3), in order.
pub fn home_placements(form: &J, r: &str) -> Vec<J> {
    let Some(d) = def(kind_of(r)) else { return Vec::new() };
    let mut out = Vec::new();
    for h in &d.home {
        let beside = h.get("beside").and_then(|v| v.as_str());
        let side = h.get("side").and_then(|v| v.as_str());
        let edge = h.get("edge").and_then(|v| v.as_str());
        if let (Some(beside), Some(side)) = (beside, side) {
            if h.get("container").and_then(|v| v.as_bool()) == Some(true) {
                let Some(l) = find_key(form, beside) else { continue };
                let Some(parent) = l.parent.as_ref() else { continue };
                if l.float >= 0 {
                    continue;
                }
                let Some(pk) = key_of(parent) else { continue };
                if form.get("root").is_some_and(|root| root.same(parent)) {
                    continue;
                }
                out.push(op(vec![("op", J::s("dockBeside")), ("target", J::s(&pk)), ("side", J::s(side))]));
            } else {
                out.push(op(vec![("op", J::s("dockBeside")), ("target", J::s(beside)), ("side", J::s(side))]));
            }
        } else if let Some(edge) = edge {
            out.push(op(vec![("op", J::s("edge")), ("side", J::s(edge))]));
        }
    }
    out
}

/// The float id holding `ref` (its node or inside it), or None.
pub fn float_of(form: &J, r: &str) -> Option<String> {
    let fl = floats_of(form);
    for l in all_locs(form) {
        if l.float < 0 || !is_island(&l.node) || !l.node.is("i", r) {
            continue;
        }
        let f = fl.get(l.float as usize)?;
        return f.get("id").and_then(|i| i.as_str().map(str::to_string));
    }
    None
}

/// Dock a floating island (§4.3): dock(float, to) for each home placement; the first success wins.
pub fn dock_float(form: &J, r: &str) -> Result<J, String> {
    let Some(f) = float_of(form, r) else { return Err("bad-op: island not placed".into()) };
    let mut last = "bad-op: target not found".to_string();
    for to in home_placements(form, r) {
        match apply_op(form, Some(&op(vec![("op", J::s("dock")), ("ref", J::s(&f)), ("to", to)])), None) {
            OpResult::Ok { form, .. } => return Ok(form),
            OpResult::Err(e) => last = e,
        }
    }
    Err(last)
}

/// Float a docked island at the host corner nearest to its centre, 16 px in (§4.1).
pub fn float_island(form: &J, r: &str, rect: Rect, host_w: i64, host_h: i64) -> Result<J, String> {
    let o = op(vec![
        ("op", J::s("float")),
        ("ref", J::s(r)),
        ("anchor", J::s(float_anchor(rect, host_w, host_h))),
        ("dx", J::int(16)),
        ("dy", J::int(16)),
    ]);
    match apply_op(form, Some(&o), None) {
        OpResult::Ok { form, .. } => Ok(form),
        OpResult::Err(e) => Err(e),
    }
}

/// Where the working copy's preset (else Classic) puts `ref`: its parent key and index, or None.
fn preset_spot(form: &J, r: &str) -> Option<(String, i64)> {
    let p = form
        .get("preset")
        .and_then(|n| n.as_str().map(str::to_string))
        .and_then(|n| preset_desktop(&n))
        .or_else(|| preset_desktop("classic"))?;
    for l in all_locs(&p) {
        if !is_island(&l.node) || !l.node.is("i", r) {
            continue;
        }
        let Some(parent) = l.parent.as_ref() else { continue };
        return key_of(parent).map(|k| (k.to_string(), l.index));
    }
    None
}

/// "Reset this island" (§4.3): hide → the leaf loses size / from (from = the preset's spot when it exists) → restore.
pub fn reset_island(form: &J, r: &str) -> Result<J, String> {
    let ok = def(kind_of(r)).is_some_and(|d| d.hideable && !d.required);
    if !ok {
        return Err("bad-op: not hideable".into());
    }
    let f = match apply_op(form, Some(&op(vec![("op", J::s("hide")), ("ref", J::s(r))])), None) {
        OpResult::Err(e) => return Err(e),
        OpResult::Ok { form, .. } => form,
    };
    let hid = hidden_of(&f);
    let Some(i) = hid.iter().position(|x| x.is_obj() && x.is("i", r)) else {
        return Err("bad-op: island not hidden".into());
    };
    let leaf = hid[i].deep();
    leaf.del("size");
    leaf.del("from");
    if let Some((parent, index)) = preset_spot(form, r) {
        if find_key(&f, &parent).is_some() {
            leaf.set("from", mk(vec![("parent", J::s(&parent)), ("index", J::int(index))]));
        }
    }
    let mut list = hid.clone();
    list[i] = leaf;
    f.set("hidden", J::arr(list));
    match apply_op(&f, Some(&op(vec![("op", J::s("restore")), ("ref", J::s(r))])), None) {
        OpResult::Ok { form, .. } => Ok(form),
        OpResult::Err(e) => Err(e),
    }
}

/// The hidden islands of a desktop form, in stored order (the Add island drawer, §7).
pub fn hidden_refs(form: &J) -> Vec<String> {
    hidden_of(form).iter().filter(|x| x.is_obj()).filter_map(|x| x.get("i").and_then(|i| i.as_str().map(str::to_string))).collect()
}

/// Phone form helpers (§12): move a section one step within its column. None when it can't move.
pub fn phone_move(form: &J, r: &str, dir: i64) -> Option<J> {
    for col in ["rail", "list"] {
        let Some(a) = form.get(col).filter(J::is_arr) else { continue };
        let items = a.items();
        let Some(i) = items.iter().position(|x| x.eq_str(r)) else { continue };
        let j = i as i64 + dir;
        if j < 0 || j >= items.len() as i64 {
            return None;
        }
        let mut b: Vec<J> = items.iter().map(J::deep).collect();
        b.swap(i, j as usize);
        let f = form.deep();
        f.set(col, J::arr(b));
        return Some(f);
    }
    None
}

/// Phone: hide = the section leaves its column for the end of `hidden`; show = it leaves `hidden` for its registry column
/// — back where it was hidden in this session (`at`), else at the end. None for an unknown ref.
pub fn phone_toggle(form: &J, r: &str, at: Option<i64>) -> Option<J> {
    let d = def(kind_of(r))?;
    let col_of = d.phone.clone()?;
    let f = form.deep();
    let arr = |k: &str| -> Vec<J> { f.get(k).filter(J::is_arr).map(|a| a.items()).unwrap_or_default() };
    let mut hid = arr("hidden");
    if let Some(i) = hid.iter().position(|x| x.eq_str(r)) {
        hid.remove(i);
        let mut col = arr(&col_of);
        match at.filter(|a| *a >= 0 && (*a as usize) < col.len()) {
            Some(a) => col.insert(a as usize, J::s(r)),
            None => col.push(J::s(r)),
        }
        f.set(&col_of, J::arr(col));
    } else {
        let mut found = false;
        for k in ["rail", "list"] {
            let mut col = arr(k);
            let Some(j) = col.iter().position(|x| x.eq_str(r)) else { continue };
            col.remove(j);
            f.set(k, J::arr(col));
            found = true;
        }
        if !found {
            return None;
        }
        hid.push(J::s(r));
    }
    f.set("hidden", J::arr(hid));
    Some(f)
}

// ---- your own button islands in a session (EDITMODE-UI §7; the layout rules of the buttons step) -------------------

fn btn_refs_in(form: &J) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for r in placed_refs(form).into_iter().chain(hidden_refs(form)) {
        if r.starts_with("buttons:") && !out.contains(&r) {
            out.push(r);
        }
    }
    if let Some(opts) = form.get("opts").filter(J::is_obj) {
        for k in opts.keys() {
            if k.starts_with("buttons:") && !out.contains(&k) {
                out.push(k);
            }
        }
    }
    out
}

/// A new empty button island `buttons:b<n>` (the smallest free n), placed by its registry home (the bottom edge), with
/// A new empty button island `buttons:b<n>` (the smallest free n), placed by its registry home (the bottom edge).
pub fn new_button_island(form: &J) -> Result<(J, String), String> {
    new_button_island_with(form, mk(vec![("items", J::arr(vec![]))]))
}

/// A new button island whose `main` row is `row` (new_button_island, Add button, Add drawer).
fn new_button_island_with(form: &J, row: J) -> Result<(J, String), String> {
    if placed_refs(form).iter().filter(|r| r.starts_with("buttons:")).count() >= 8 {
        return Err("too-many".into());
    }
    let used = btn_refs_in(form);
    let mut n = 1;
    while used.contains(&format!("buttons:b{n}")) {
        n += 1;
    }
    let r = format!("buttons:b{n}");
    let f = form.deep();
    let mut hid = hidden_of(&f);
    hid.push(mk(vec![("t", J::s("island")), ("i", J::s(&r))]));
    f.set("hidden", J::arr(hid));
    let restored = match apply_op(&f, Some(&op(vec![("op", J::s("restore")), ("ref", J::s(&r))])), None) {
        OpResult::Ok { form, .. } => form,
        OpResult::Err(e) => return Err(e),
    };
    let rows = mk(vec![("main", row)]);
    match apply_op(&restored, Some(&op(vec![("op", J::s("setOpt")), ("ref", J::s(&r)), ("key", J::s("rows")), ("value", rows)])), None) {
        OpResult::Ok { form, .. } => Ok((form, r)),
        OpResult::Err(e) => Err(e),
    }
}

// ---- buttons and drawers (EDITMODE-UI §7.2, §7.3; feat/editmode-buttons-drawers; the web's session.ts, line for line) --
// Add button = a new button island holding ONE action from the catalogue. Add drawer = the island goes into `hidden` and
// a new one-button island runs `layout.drawer.toggle` on it; opening the drawer renders drawer_form (the island in a
// float under its button) — a transient form, never saved. The layout format is unchanged: a Tin Hat that does not know
// the action draws the button as "Needs a newer Tin Hat" and lists the island among the hidden ones.

/// The action a drawer's button runs (ACTIONS.md: target {island, icon?}).
pub const DRAWER_ACTION: &str = "layout.drawer.toggle";
/// The drawer button's icons (the action's `icon` param values, code-point order).
pub const DRAWER_ICONS: [&str; 10] = ["calendar", "chat", "drawer", "games", "grid", "info", "music", "people", "pin", "star"];
/// Kinds an island in a drawer may have: hideable, floatable, and the same anywhere (not the room group, the call or
/// room info).
pub const DRAWER_KINDS: [&str; 7] = ["nav", "dms", "spaces", "rooms", "widgets", "account", "buttons"];
/// The gap between a drawer's button and its panel (layout units).
pub const DRAWER_GAP: i64 = 4;
/// drawer_form's numbers are clamped to ±DRAWER_SPAN first (the same domain in TypeScript, which also truncates).
pub const DRAWER_SPAN: i64 = 65536;

fn placed_buttons(form: &J) -> usize {
    placed_refs(form).iter().filter(|r| r.starts_with("buttons:")).count()
}
const LOOKS: [&str; 3] = ["icon", "iconLabel", "label"];

fn item_id_ok(a: &str) -> bool {
    let b = a.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && b[0].is_ascii_lowercase()
        && b[1..].iter().all(|c| c.is_ascii_alphanumeric() || *c == b'.')
}

/// A new button island holding one action: `item` = a row item ({a, target?}), `look` icon | iconLabel | label.
pub fn new_action_button(form: &J, item: &J, look: &str) -> Result<(J, String), String> {
    let ok = item.is_obj() && item.get("a").and_then(|a| a.as_str().map(|s| item_id_ok(&s))).unwrap_or(false);
    if !ok {
        return Err("bad-item".into());
    }
    if !LOOKS.contains(&look) {
        return Err("bad-look".into());
    }
    new_button_island_with(form, mk(vec![("look", J::s(look)), ("items", J::arr(vec![item.deep()]))]))
}

/// Whether a button island holds a drawer's button (any row): it can't go into a drawer (it would lock itself away).
fn holds_drawer_button(form: &J, r: &str) -> bool {
    let Some(rows) = form.get("opts").and_then(|o| o.get(r)).and_then(|o| o.get("rows")).filter(J::is_obj) else { return false };
    rows.keys().iter().any(|k| {
        rows.get(k).and_then(|row| row.get("items")).is_some_and(|items| {
            items.items().iter().any(|it| it.is_obj() && it.is("a", DRAWER_ACTION))
        })
    })
}

/// The islands that may go into a drawer: placed or hidden, of a DRAWER_KINDS kind, not a drawer's own button island;
/// kind order, then code-point order.
pub fn drawer_targets(form: &J) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for r in placed_refs(form).into_iter().chain(hidden_refs(form)) {
        if !seen.contains(&r) {
            seen.push(r);
        }
    }
    let rank = |r: &str| DRAWER_KINDS.iter().position(|k| *k == kind_of(r));
    let mut out: Vec<String> =
        seen.into_iter().filter(|r| rank(r).is_some() && (r.starts_with("buttons:") || r.as_str() == kind_of(r)) && !holds_drawer_button(form, r)).collect();
    out.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| cmp_cp(a, b)));
    out
}

/// Add drawer: `r` is hidden (when placed) and a new one-button island opens it; answers (form, button island ref).
pub fn new_drawer(form: &J, r: &str, icon: &str) -> Result<(J, String), String> {
    if !drawer_targets(form).iter().any(|x| x == r) {
        return Err("not-drawer-island".into());
    }
    if !icon.is_empty() && !DRAWER_ICONS.contains(&icon) {
        return Err("bad-icon".into());
    }
    // it must be able to open: a free float slot, and a button island in it must not make a ninth placed one
    // a button island in it: after hiding it, its launcher + itself opened must still fit in 8 placed
    if r.starts_with("buttons:") {
        let after_hide = placed_buttons(form) - usize::from(placed_refs(form).iter().any(|x| x == r));
        if after_hide >= 7 {
            return Err("too-many".into());
        }
    }
    let f = if placed_refs(form).iter().any(|x| x == r) {
        match apply_op(form, Some(&op(vec![("op", J::s("hide")), ("ref", J::s(r))])), None) {
            OpResult::Ok { form, .. } => form,
            OpResult::Err(e) => return Err(e),
        }
    } else {
        form.deep()
    };
    // a free float slot once it is hidden (hiding an island that floats alone frees its slot)
    if floats_of(&f).len() >= 8 {
        return Err("too-many-floats".into());
    }
    let target = if icon.is_empty() {
        mk(vec![("island", J::s(r))])
    } else {
        mk(vec![("icon", J::s(icon)), ("island", J::s(r))])
    };
    let item = mk(vec![("a", J::s(DRAWER_ACTION)), ("target", target)]);
    new_button_island_with(&f, mk(vec![("look", J::s("icon")), ("items", J::arr(vec![item]))]))
}

/// The form a host renders while `r`'s drawer is open: the hidden island in a float beside its button `btn` (layout
/// units, host coordinates, integers): under it (over it in the lower half), left-aligned with it (right-aligned in the
/// right half), DRAWER_GAP away; the float's size is the island's registry float size. Never saved. Answers (form, id).
pub fn drawer_form(form: &J, r: &str, btn: Rect, host_w: i64, host_h: i64) -> Result<(J, String), String> {
    if !hidden_refs(form).iter().any(|x| x == r) {
        return Err("not-hidden".into());
    }
    if !drawer_targets(form).iter().any(|x| x == r) {
        return Err("not-drawer-island".into()); // whatever a layout says
    }
    let floats = floats_of(form);
    if floats.len() >= 8 {
        return Err("too-many-floats".into());
    }
    if r.starts_with("buttons:") && placed_buttons(form) >= 8 {
        return Err("too-many".into());
    }
    // the input domain: integers in [-DRAWER_SPAN, DRAWER_SPAN] (clamped first) — exact in both languages
    let n = |v: i64| v.clamp(-DRAWER_SPAN, DRAWER_SPAN);
    let btn = Rect { x: n(btn.x), y: n(btn.y), w: n(btn.w), h: n(btn.h) };
    let (host_w, host_h) = (n(host_w), n(host_h));
    let mut taken: Vec<String> = placed_refs(form).into_iter().chain(hidden_refs(form)).collect();
    for fl in &floats {
        if let Some(i) = fl.get("id").and_then(|i| i.as_str().map(str::to_string)) {
            taken.push(i);
        }
    }
    for l in all_locs(form) {
        if let Some(k) = key_of(&l.node) {
            taken.push(k.to_string());
        }
    }
    let mut id = "drawer".to_string();
    let mut n = 2;
    while taken.contains(&id) {
        id = format!("drawer-{n}");
        n += 1;
    }
    let right = 2 * btn.x + btn.w > host_w; // doubled centre: integer maths (float_anchor's rule)
    let bottom = 2 * btn.y + btn.h > host_h;
    let clamp = |v: i64| v.clamp(-4096, 4096);
    let dx = clamp(if right { host_w - (btn.x + btn.w) } else { btn.x });
    let dy = clamp(if bottom { host_h - btn.y + DRAWER_GAP } else { btn.y + btn.h + DRAWER_GAP });
    let anchor = format!("{}{}", if bottom { "b" } else { "t" }, if right { "r" } else { "l" });
    let f = form.deep();
    let kept: Vec<J> = hidden_of(&f).into_iter().filter(|x| !(x.is_obj() && x.is("i", r))).collect();
    f.set("hidden", J::arr(kept));
    let mut fl = floats_of(&f);
    fl.push(mk(vec![
        ("id", J::s(&id)),
        ("anchor", J::s(&anchor)),
        ("dx", J::int(dx)),
        ("dy", J::int(dy)),
        ("node", mk(vec![("t", J::s("island")), ("i", J::s(r))])),
    ]));
    f.set("floats", J::arr(fl));
    Ok((f, id))
}

/// Remove a button island: hidden, then its leaf and its options dropped. None when it can't be hidden.
pub fn remove_button_island_from(form: &J, r: &str) -> Option<J> {
    let f = if placed_refs(form).iter().any(|x| x == r) {
        match apply_op(form, Some(&op(vec![("op", J::s("hide")), ("ref", J::s(r))])), None) {
            OpResult::Ok { form, .. } => form,
            OpResult::Err(_) => return None,
        }
    } else {
        form.deep()
    };
    let kept: Vec<J> = hidden_of(&f).into_iter().filter(|x| !(x.is_obj() && x.is("i", r))).collect();
    f.set("hidden", J::arr(kept));
    if let Some(opts) = f.get("opts").filter(J::is_obj) {
        opts.del(r);
    }
    Some(f)
}

// ---- the phone form's home sections (EDITMODE-UI §12 rendering rules; the web's features/nav/sections.ts) ------------
// The desktop renders no phone form in 11.0 (its handheld windows use the dock editor), but the rules are shared.

/// The phone form's sections: `rail` (nav · dms · spaces) and `list` (rooms · widgets · account), each in its order.
pub const DEFAULT_RAIL: [&str; 3] = ["nav", "dms", "spaces"];
pub const DEFAULT_LIST: [&str; 3] = ["rooms", "widgets", "account"];

/// The known rail sections in phone-form order (unknown refs skipped); None = the default order.
pub fn rail_order(rail: Option<&[&str]>) -> Vec<String> {
    rail.unwrap_or(&DEFAULT_RAIL).iter().filter(|r| matches!(**r, "nav" | "dms" | "spaces")).map(|r| r.to_string()).collect()
}

/// Where the list column's parts go: rooms shown, widgets before / after the rows (or hidden), the account card at the
/// top / bottom (or hidden). None = the default order.
#[derive(Debug, PartialEq, Eq)]
pub struct ListPlan {
    pub rooms: bool,
    pub widgets: Option<&'static str>,
    pub account: Option<&'static str>,
}
pub fn list_plan(list: Option<&[&str]>) -> ListPlan {
    let l = list.unwrap_or(&DEFAULT_LIST);
    let i = |r: &str| l.iter().position(|x| *x == r).map(|p| p as i64).unwrap_or(-1);
    let rooms = i("rooms") >= 0;
    let widgets = if i("widgets") < 0 {
        None
    } else if rooms && i("widgets") < i("rooms") {
        Some("before")
    } else {
        Some("after")
    };
    let account = if i("account") < 0 {
        None
    } else if rooms && i("account") < i("rooms") {
        Some("top")
    } else if !rooms && i("widgets") >= 0 && i("account") < i("widgets") {
        Some("top")
    } else {
        Some("bottom")
    };
    ListPlan { rooms, widgets, account }
}

#[cfg(test)]
mod tests {
    //! The web's session.test.ts, case for case.
    use super::*;
    use crate::json::read_json;
    use crate::repair::{repair_desktop, validate_desktop};
    use crate::schema::{classic_desktop, classic_phone};
    /// presets_carry_your_buttons_and_drawers: the Compact pick of its start form (the web asserts the same string)
    /// presets_carry_nested_drawers_and_target_rows: the Call-first pick (the web asserts the same string)
    const CARRY_NESTED: &str = r##"{"collapse":[{"below":1300,"ref":"panel","to":"overlay"},{"below":1180,"ref":"navcol","to":"drawer"}],"floats":[],"hidden":[{"i":"buttons:b1","t":"island"},{"from":{"index":0,"parent":"roomscol"},"i":"rooms","size":{"auto":true,"grow":1,"min":160},"t":"island"},{"from":{"index":0,"parent":"roomscol"},"i":"widgets","size":{"auto":true},"t":"island"}],"opts":{"buttons:b1":{"rows":{"main":{"items":[{"a":"layout.drawer.toggle","target":{"icon":"star","island":"rooms"}}],"look":"icon"}}},"buttons:b2":{"rows":{"main":{"items":[{"a":"layout.drawer.toggle","target":{"island":"buttons:b1"}},{"a":"layout.drawer.toggle","target":{"island":"Bad Ref"}}],"look":"icon"}}},"rooms":{"rows":{"main":{"items":[{"a":"app.settings.open"},{"a":"layout.drawer.toggle","target":{"island":"widgets"}}]}}}},"preset":"callFirst","root":{"c":[{"c":[{"c":[{"c":[{"i":"nav","size":{"auto":true},"t":"island"},{"i":"dms","size":{"auto":true},"t":"island"},{"i":"spaces","size":{"min":112,"share":1},"t":"island"}],"id":"rail","size":{"fixed":68},"t":"col"},{"i":"account","size":{"fixed":260},"t":"island"}],"id":"navcol","size":{"auto":true},"t":"row"},{"c":[{"i":"header","size":{"auto":true},"t":"island"},{"c":[{"i":"call","size":{"min":480,"share":2},"t":"island"},{"c":[{"i":"timeline","size":{"min":120,"share":1},"t":"island"},{"i":"composer","size":{"auto":true},"t":"island"}],"id":"roomcol","size":{"min":340,"share":1},"t":"col"},{"i":"panel","size":{"fixed":324},"t":"island"}],"id":"stage","size":{"share":1},"t":"row"}],"id":"main","size":{"min":360,"share":1},"t":"col"}],"id":"body","size":{"share":1},"t":"row"},{"i":"buttons:b2","t":"island"}],"id":"n1","t":"col"}}"##;
    const CARRY_COMPACT: &str = r##"{"collapse":[{"below":820,"ref":"panel","to":"overlay"},{"below":680,"ref":"navcol","to":"drawer"}],"floats":[],"gap":4,"hidden":[{"i":"buttons:b3","t":"island"},{"i":"buttons:b5","t":"island"},{"from":{"index":0,"parent":"roomscol"},"i":"rooms","size":{"auto":true,"grow":1,"min":160},"t":"island"}],"margin":4,"opts":{"buttons:b1":{"rows":{"main":{"items":[{"a":"call.deafen"}],"look":"icon"}}},"buttons:b2":{"rows":{"main":{"items":[{"a":"layout.drawer.toggle","target":{"icon":"star","island":"rooms"}}],"look":"icon"}}},"buttons:b3":{"rows":{"main":{"items":[{"a":"app.settings.open"}]}}},"buttons:b4":{"rows":{"main":{"items":[{"a":"layout.drawer.toggle","target":{"island":"buttons:b3"}}],"look":"icon"}}},"buttons:b5":{"rows":{"main":{"items":[]}}},"rooms":{"compact":true}},"preset":"compact","root":{"c":[{"c":[{"c":[{"c":[{"i":"nav","size":{"auto":true},"t":"island"},{"i":"dms","size":{"auto":true},"t":"island"},{"i":"spaces","size":{"min":112,"share":1},"t":"island"}],"id":"rail","size":{"fixed":64},"t":"col"},{"c":[{"i":"widgets","size":{"auto":true},"t":"island"},{"i":"account","size":{"auto":true},"t":"island"}],"id":"roomscol","size":{"fixed":248},"t":"col"}],"id":"navcol","size":{"auto":true},"t":"row"},{"c":[{"i":"header","size":{"auto":true},"t":"island"},{"i":"call","size":{"auto":true,"min":252},"t":"island"},{"c":[{"c":[{"i":"timeline","size":{"min":120,"share":1},"t":"island"},{"i":"composer","size":{"auto":true},"t":"island"}],"id":"roomcol","size":{"min":360,"share":1},"t":"col"},{"i":"panel","size":{"fixed":300},"t":"island"}],"id":"convrow","size":{"min":160,"minPm":250,"share":1},"t":"row"}],"id":"main","size":{"min":360,"share":1},"t":"col"}],"id":"body","size":{"share":1},"t":"row"},{"i":"buttons:b1","t":"island"},{"i":"buttons:b2","t":"island"},{"i":"buttons:b4","t":"island"}],"id":"n1","t":"col"}}"##;

    fn j(s: &str) -> J {
        read_json(s).expect("json")
    }
    fn classic() -> J {
        classic_desktop()
    }
    fn start(desktop: &J) -> EditSession {
        EditSession::new("main", "Main", desktop, &classic_phone(), FormKind::Desktop)
    }
    fn apply(f: &J, o: &str) -> J {
        match apply_op(f, Some(&j(o)), None) {
            OpResult::Ok { form, .. } => form,
            OpResult::Err(e) => panic!("{o}: {e}"),
        }
    }
    /// (parent key, index, size) of an island.
    fn parent_of(form: &J, r: &str) -> Option<(Option<String>, i64, Option<J>)> {
        for l in all_locs(form) {
            if is_island(&l.node) && l.node.is("i", r) {
                let p = l.parent.as_ref().and_then(|p| p.get("id")).and_then(|i| i.as_str().map(str::to_string));
                return Some((p, if l.parent.is_some() { l.index } else { -1 }, l.node.get("size")));
            }
        }
        None
    }

    #[test]
    fn fresh_session_is_clean_and_saves_nothing() {
        let mut s = start(&classic());
        assert!(!s.dirty());
        assert!(!s.can_undo());
        assert!(s.finish().is_none(), "nothing changed → no save");
        assert!(s.closed);
    }

    #[test]
    fn undo_redo_walk_whole_forms() {
        let mut s = start(&classic());
        let s0 = s.form.canon();
        assert_eq!(s.apply(&j(r#"{"op":"hide","ref":"widgets"}"#), None), Ok(()));
        assert!(s.dirty() && s.can_undo() && !s.can_redo());
        assert_eq!(hidden_refs(&s.form), vec!["widgets"]);
        assert_eq!(s.apply(&j(r#"{"op":"dockBeside","ref":"panel","target":"rooms","side":"bottom"}"#), None), Ok(()));
        assert_eq!(s.undo_depth(), 2);
        assert!(s.undo());
        assert_eq!(hidden_refs(&s.form), vec!["widgets"]);
        assert!(s.undo());
        assert_eq!(s.form.canon(), s0);
        assert!(!s.dirty());
        assert!(!s.undo(), "nothing before the start");
        assert!(s.redo() && s.redo());
        assert!(!s.redo());
        assert_eq!(parent_of(&s.form, "panel").unwrap().0.as_deref(), Some("roomscol"));
        assert!(s.undo());
        assert!(s.can_redo());
        let _ = s.apply(&j(r#"{"op":"hide","ref":"account"}"#), None);
        assert!(!s.can_redo());
        assert!(validate_desktop(Some(&s.form)).is_empty());
    }

    #[test]
    fn refused_operation_changes_nothing() {
        let mut s = start(&classic());
        let before = s.form.canon();
        assert_eq!(s.apply(&j(r#"{"op":"hide","ref":"timeline"}"#), None), Err("bad-op: required island".to_string()));
        assert_eq!(s.form.canon(), before);
        assert!(!s.can_undo());
        assert_eq!(s.apply(&j(r#"{"op":"setOpt","ref":"rooms","key":"x"}"#), None), Ok(()));
        assert!(!s.can_undo());
    }

    #[test]
    fn cancel_restores() {
        let mut s = start(&classic());
        let _ = s.apply(&j(r#"{"op":"hide","ref":"widgets"}"#), None);
        s.cancel();
        assert!(s.closed);
        assert!(s.apply(&j(r#"{"op":"hide","ref":"account"}"#), None).is_err());
        assert!(s.finish().is_none(), "a cancelled session never saves");
    }

    #[test]
    fn done_answers_once_with_other_form_untouched() {
        let phone = classic_phone();
        let mut s = EditSession::new("main", "Main", &classic(), &phone, FormKind::Desktop);
        let _ = s.apply(&j(r#"{"op":"hide","ref":"widgets"}"#), None);
        let r = s.finish().expect("an edit");
        assert_eq!((r.id.as_str(), r.name.as_str()), ("main", "Main"));
        assert_eq!(hidden_refs(&r.desktop), vec!["widgets"]);
        assert!(same(&r.phone, &phone));
        assert!(s.finish().is_none(), "a second Done saves nothing");
        r.desktop.set("root", J::Null);
        assert!(!s.form.get("root").unwrap().is_null(), "the result is a copy");
    }

    #[test]
    fn rename_trims_and_caps() {
        let mut s = start(&classic());
        s.rename("  Side  ");
        assert_eq!(s.name, "Side");
        assert!(s.dirty());
        s.rename("   ");
        assert_eq!(s.name, "Side");
        s.rename(&"x".repeat(50));
        assert_eq!(s.name.chars().count(), 40);
    }

    #[test]
    fn presets_carry_rows_and_previous_is_the_start() {
        let rows = j(r#"{"main":{"items":[{"a":"room.info"}]}}"#);
        let mut f = classic();
        f = apply(&f, &format!(r#"{{"op":"setOpt","ref":"header","key":"rows","value":{}}}"#, rows.canon()));
        f = apply(&f, r#"{"op":"setOpt","ref":"rooms","key":"compact","value":true}"#);
        let mut s = start(&f);
        assert_eq!(s.preset("callFirst"), Ok(()));
        assert!(s.form.is("preset", "callFirst"));
        assert_eq!(s.form.get("opts").unwrap().get("header").unwrap().get("rows").unwrap().canon(), rows.canon(), "rows carried");
        assert!(s.form.get("opts").unwrap().get("rooms").and_then(|r| r.get("compact")).is_none(), "other options are the preset's");
        assert_eq!(s.undo_depth(), 1);
        assert!(s.preset("nope").is_err());
        assert_eq!(s.undo_depth(), 1);
        assert_eq!(s.previous(), Ok(()));
        assert!(same(&s.form, &f));
        assert_eq!(s.undo_depth(), 2, "Previous is a step of its own");
        assert!(!s.dirty());
        let _ = s.preset("handheld");
        let _ = s.reset();
        assert!(s.form.is("preset", "classic"));
        for p in ["classic", "compact", "callFirst", "sideMonitor", "handheld"] {
            let r = pick_preset(&f, p).unwrap_or_else(|e| panic!("{p}: {e}"));
            assert!(validate_desktop(Some(&r)).is_empty(), "{p}");
            assert_eq!(r.get("opts").unwrap().get("header").unwrap().get("rows").unwrap().canon(), rows.canon(), "{p}");
        }
    }

    #[test]
    fn undo_stack_is_capped() {
        let mut s = start(&classic());
        for i in 0..(UNDO_CAP + 20) {
            let _ = s.apply(&j(&format!(r#"{{"op":"setOpt","ref":"rooms","key":"n","value":{i}}}"#)), None);
        }
        assert_eq!(s.undo_depth(), UNDO_CAP);
    }

    #[test]
    fn float_picks_nearest_corner_and_dock_follows_home() {
        let r = |x, y, w, h| Rect { x, y, w, h };
        assert_eq!(float_anchor(r(0, 0, 100, 100), 1000, 800), "tl");
        assert_eq!(float_anchor(r(900, 0, 100, 100), 1000, 800), "tr");
        assert_eq!(float_anchor(r(0, 700, 100, 100), 1000, 800), "bl");
        assert_eq!(float_anchor(r(900, 700, 100, 100), 1000, 800), "br");
        assert_eq!(float_anchor(r(450, 350, 100, 100), 1000, 800), "tl", "the exact middle stays top-left");
        let f0 = classic();
        let fl = float_island(&f0, "widgets", r(900, 700, 100, 100), 1000, 800).expect("float");
        assert_eq!(fl.get("floats").unwrap().len(), 1);
        assert!(fl.get("floats").unwrap().at(0).unwrap().is("anchor", "br"));
        assert_eq!(fl.get("floats").unwrap().at(0).unwrap().get("dx").unwrap().int_val(), Some(16));
        assert!(float_of(&fl, "widgets").is_some());
        let d = dock_float(&fl, "widgets").expect("dock");
        assert_eq!(d.get("floats").unwrap().len(), 0);
        assert_eq!(parent_of(&d, "widgets").unwrap().0.as_deref(), Some("roomscol"), "home: beside rooms, bottom");
        assert!(dock_float(&f0, "widgets").is_err(), "a docked island has no float");
        let c = float_island(&f0, "call", r(0, 0, 10, 10), 1000, 800).expect("float call");
        let cd = dock_float(&c, "call").expect("dock call");
        assert_eq!(parent_of(&cd, "call").unwrap().0.as_deref(), Some("main"));
        assert_eq!(
            home_placements(&f0, "panel")[0].canon(),
            r#"{"op":"dockBeside","side":"right","target":"roomcol"}"#,
            "container: true → the target's parent"
        );
    }

    #[test]
    fn reset_island_returns_to_preset_spot() {
        let f0 = classic();
        let f = apply(&f0, r#"{"op":"edge","ref":"rooms","side":"right"}"#);
        assert_ne!(parent_of(&f, "rooms").unwrap().0.as_deref(), Some("roomscol"));
        let r = reset_island(&f, "rooms").expect("reset");
        let (p, i, size) = parent_of(&r, "rooms").unwrap();
        assert_eq!(p.as_deref(), Some("roomscol"));
        assert_eq!(i, 0);
        assert!(size.is_none(), "no stored size = the registry pref");
        assert!(validate_desktop(Some(&r)).is_empty());
        assert!(reset_island(&f0, "timeline").is_err(), "required islands have no reset");
        let fl = float_island(&f0, "account", Rect { x: 0, y: 0, w: 10, h: 10 }, 1000, 800).unwrap();
        let r2 = reset_island(&fl, "account").expect("reset a float");
        assert_eq!(parent_of(&r2, "account").unwrap().0.as_deref(), Some("roomscol"));
        assert_eq!(r2.get("floats").unwrap().len(), 0);
        assert!(placed_refs(&r2).contains(&"account".to_string()));
    }

    #[test]
    fn phone_editor_moves_and_toggles() {
        let p0 = classic_phone();
        let m = phone_move(&p0, "account", -1).unwrap();
        assert_eq!(m.get("list").unwrap().canon(), r#"["rooms","account","widgets"]"#);
        assert!(phone_move(&p0, "rooms", -1).is_none(), "already first");
        assert!(phone_move(&p0, "nope", 1).is_none());
        let h = phone_toggle(&p0, "widgets", None).unwrap();
        assert_eq!(h.get("list").unwrap().canon(), r#"["rooms","account"]"#);
        assert_eq!(h.get("hidden").unwrap().canon(), r#"["widgets"]"#);
        assert!(same(&repair_phone(Some(&h)).form, &h), "a valid phone form");
        let back = phone_toggle(&h, "widgets", None).unwrap();
        assert_eq!(back.get("list").unwrap().canon(), r#"["rooms","account","widgets"]"#);
        assert_eq!(back.get("hidden").unwrap().canon(), "[]");
        assert!(phone_toggle(&p0, "timeline", None).is_none(), "not a phone section");
        let mut s = EditSession::new("main", "Main", &classic(), &classic_phone(), FormKind::Phone);
        assert_eq!(s.set_phone(&h), Ok(()));
        assert!(s.dirty());
        assert!(s.set_phone(&j(r#"{"rail":["nav","nav"]}"#)).is_err(), "a form repair would change is refused");
        assert!(s.apply(&j(r#"{"op":"hide","ref":"rooms"}"#), None).is_err(), "no engine operations on the phone form");
        let r = s.finish().unwrap();
        assert!(same(&r.desktop, &classic()), "the desktop form rides along");
        assert_eq!(r.phone.get("hidden").unwrap().canon(), r#"["widgets"]"#);
    }

    #[test]
    fn every_preset_repairs_to_itself() {
        for p in ["classic", "compact", "callFirst", "sideMonitor", "handheld"] {
            let f = preset_desktop(p).unwrap();
            assert!(same(&repair_desktop(Some(&f)).form, &f), "{p}");
        }
    }

    #[test]
    fn button_islands_in_a_session() {
        let f0 = classic();
        let (a, ra) = new_button_island(&f0).expect("a new island");
        assert_eq!(ra, "buttons:b1");
        assert!(placed_refs(&a).contains(&"buttons:b1".to_string()));
        assert_eq!(a.get("opts").unwrap().get("buttons:b1").unwrap().canon(), r#"{"rows":{"main":{"items":[]}}}"#);
        assert!(validate_desktop(Some(&a)).is_empty());
        let (b, rb) = new_button_island(&a).unwrap();
        assert_eq!(rb, "buttons:b2");
        let r = remove_button_island_from(&b, "buttons:b1").unwrap();
        assert!(!placed_refs(&r).contains(&"buttons:b1".to_string()));
        assert!(!hidden_refs(&r).contains(&"buttons:b1".to_string()));
        assert!(r.get("opts").unwrap().get("buttons:b1").is_none());
        assert!(validate_desktop(Some(&r)).is_empty());
        assert_eq!(new_button_island(&r).unwrap().1, "buttons:b1", "the smallest free n again");
        let mut f = f0;
        for _ in 0..8 {
            f = new_button_island(&f).unwrap().0;
        }
        assert_eq!(new_button_island(&f).err().as_deref(), Some("too-many"));
    }

    #[test]
    fn presets_carry_your_buttons_and_drawers() {
        // 12.1.8 (EDITMODE-UI §8): Add button + Add drawer (Rooms, and a button island) + a hidden button island survive
        // every preset pick; the web's session.test.ts asserts the same canonical form for Compact.
        let f0 = classic();
        let (a, ra) = new_action_button(&f0, &j(r#"{"a":"call.deafen"}"#), "icon").unwrap();
        let (b, rb) = new_drawer(&a, "rooms", "star").unwrap();
        let (c, rc) = new_button_island(&b).unwrap();
        let c = apply(&c, &format!(r#"{{"op":"setOpt","ref":"{rc}","key":"rows","value":{{"main":{{"items":[{{"a":"app.settings.open"}}]}}}}}}"#));
        let (d, rd) = new_drawer(&c, &rc, "").unwrap();
        let (e, re) = new_button_island(&d).unwrap();
        let e = apply(&e, &format!(r#"{{"op":"hide","ref":"{re}"}}"#));
        assert_eq!((ra.as_str(), rb.as_str(), rc.as_str(), rd.as_str(), re.as_str()), ("buttons:b1", "buttons:b2", "buttons:b3", "buttons:b4", "buttons:b5"));
        let rows = |f: &J, r: &str| f.get("opts").and_then(|o| o.get(r)).and_then(|o| o.get("rows")).map(|x| x.canon());
        for p in ["classic", "compact", "callFirst", "sideMonitor", "handheld"] {
            let g = pick_preset(&e, p).unwrap_or_else(|x| panic!("{p}: {x}"));
            assert!(g.is("preset", p), "{p}");
            assert!(validate_desktop(Some(&g)).is_empty(), "{p}: {:?}", validate_desktop(Some(&g)));
            let placed = placed_refs(&g);
            let hidden = hidden_refs(&g);
            for r in [&ra, &rb, &rd] {
                assert!(placed.contains(r), "{p}: {r} placed");
                assert_eq!(rows(&g, r), rows(&e, r), "{p}: {r} rows");
            }
            for r in ["rooms", rc.as_str(), re.as_str()] {
                assert!(hidden.iter().any(|x| x == r) && !placed.iter().any(|x| x == r), "{p}: {r} hidden");
            }
            assert_eq!(rows(&g, &rc), rows(&e, &rc), "{p}: the drawer's button island keeps its button");
            assert!(drawer_form(&g, "rooms", Rect { x: 0, y: 0, w: 40, h: 40 }, 1600, 1000).is_ok(), "{p}: the Rooms drawer opens");
            assert!(drawer_form(&g, &rc, Rect { x: 0, y: 0, w: 40, h: 40 }, 1600, 1000).is_ok(), "{p}: the button drawer opens");
        }
        // nothing of yours: the pick is applyPreset (+ rows) as before
        assert!(same(&pick_preset(&f0, "compact").unwrap(), &preset_desktop("compact").unwrap()));
        // a session: one undo step; Previous brings back the start
        let mut s = start(&e);
        assert_eq!(s.preset("callFirst"), Ok(()));
        assert_eq!(s.undo_depth(), 1);
        assert_eq!(s.previous(), Ok(()));
        assert!(same(&s.form, &e));
        // the shared vector (session.test.ts: the same string)
        let g = pick_preset(&e, "compact").unwrap();
        if std::env::var_os("CARRY_DUMP").is_some() {
            println!("CARRY_COMPACT={}", g.canon());
            println!("CARRY_START={}", e.canon());
        }
        assert_eq!(g.canon(), CARRY_COMPACT);
    }

    #[test]
    fn presets_carry_nested_drawers_and_target_rows() {
        // the review's cases: a drawer whose island has a drawer of its own (b2 → b1 → Rooms → Widgets), the
        // Rooms island's own rows, and a malformed hidden entry ("Bad Ref") that must not come back (a well-formed unknown ref is kept: a newer Tin Hat's island)
        let f0 = classic();
        let (a, ra) = new_drawer(&f0, "rooms", "star").unwrap();
        let a = apply(&a, r#"{"op":"setOpt","ref":"rooms","key":"rows","value":{"main":{"items":[{"a":"app.settings.open"},{"a":"layout.drawer.toggle","target":{"island":"widgets"}}]}}}"#);
        // (Add drawer refuses a button island that holds a drawer button: this chain only comes from a hand-made layout)
        let a = apply(&a, &format!(r#"{{"op":"hide","ref":"{ra}"}}"#));
        let (b, rb) = new_button_island(&a).unwrap();
        assert_eq!((ra.as_str(), rb.as_str()), ("buttons:b1", "buttons:b2"));
        let b = apply(&b, r#"{"op":"setOpt","ref":"buttons:b2","key":"rows","value":{"main":{"items":[{"a":"layout.drawer.toggle","target":{"island":"buttons:b1"}},{"a":"layout.drawer.toggle","target":{"island":"Bad Ref"}}],"look":"icon"}}}"#);
        let mut hid = hidden_of(&b);
        hid.push(j(r#"{"t":"island","i":"Bad Ref"}"#));
        b.set("hidden", J::arr(hid));
        let rows = |f: &J, r: &str| f.get("opts").and_then(|o| o.get(r)).and_then(|o| o.get("rows")).map(|x| x.canon());
        for p in ["classic", "compact", "callFirst", "sideMonitor", "handheld"] {
            let g = pick_preset(&b, p).unwrap_or_else(|x| panic!("{p}: {x}"));
            assert!(validate_desktop(Some(&g)).is_empty(), "{p}: {:?}", validate_desktop(Some(&g)));
            let (placed, hidden) = (placed_refs(&g), hidden_refs(&g));
            assert!(placed.contains(&rb), "{p}: b2 placed");
            for r in ["rooms", ra.as_str(), "widgets"] {
                assert!(hidden.iter().any(|x| x == r) && !placed.iter().any(|x| x == r), "{p}: {r} hidden");
                assert_eq!(rows(&g, r), rows(&b, r), "{p}: {r} rows");
            }
            assert!(!hidden.iter().any(|x| x == "Bad Ref") && !placed.iter().any(|x| x == "Bad Ref"), "{p}: a malformed ref is refused");
            // Rooms now holds a drawer button itself, so (by design) it can't go into a drawer; its own drawer opens
            assert!(drawer_form(&g, "widgets", Rect { x: 0, y: 0, w: 40, h: 40 }, 1600, 1000).is_ok(), "{p}: Rooms' Widgets drawer opens");
        }
        let g = pick_preset(&b, "callFirst").unwrap();
        if std::env::var_os("CARRY_DUMP").is_some() {
            println!("CARRY_NESTED={}", g.canon());
        }
        assert_eq!(g.canon(), CARRY_NESTED);
    }

    #[test]
    fn failed_done_reopens_the_session_as_it_was() {
        let mut s = start(&classic());
        let _ = s.apply(&j(r#"{"op":"hide","ref":"widgets"}"#), None);
        let _ = s.apply(&j(r#"{"op":"hide","ref":"account"}"#), None);
        assert!(s.finish().is_some());
        assert!(s.closed);
        s.reopen();
        assert!(!s.closed);
        assert!(s.dirty(), "still dirty: a second Done saves again");
        assert_eq!(s.undo_depth(), 2, "history kept");
        assert!(s.undo() && s.undo());
        assert!(!s.dirty(), "undo still reaches the original start");
    }

    #[test]
    fn phone_show_returns_where_it_was_hidden() {
        let p0 = classic_phone();
        let h = phone_toggle(&p0, "dms", None).unwrap();
        assert_eq!(h.get("rail").unwrap().canon(), r#"["nav","spaces"]"#);
        assert_eq!(phone_toggle(&h, "dms", Some(1)).unwrap().get("rail").unwrap().canon(), r#"["nav","dms","spaces"]"#);
        assert_eq!(phone_toggle(&h, "dms", None).unwrap().get("rail").unwrap().canon(), r#"["nav","spaces","dms"]"#);
        assert_eq!(phone_toggle(&h, "dms", Some(9)).unwrap().get("rail").unwrap().canon(), r#"["nav","spaces","dms"]"#, "past the end appends");
    }

    // the web's features/nav/sections.test.ts, case for case
    #[test]
    fn phone_rail_order() {
        assert_eq!(rail_order(None), vec!["nav", "dms", "spaces"]);
        assert_eq!(rail_order(Some(&["spaces", "nav"])), vec!["spaces", "nav"]);
        assert_eq!(rail_order(Some(&["nav", "x:future", "dms"])), vec!["nav", "dms"]);
    }

    #[test]
    fn phone_list_plan() {
        let p = |rooms, widgets, account| ListPlan { rooms, widgets, account };
        assert_eq!(list_plan(None), p(true, Some("after"), Some("bottom")));
        assert_eq!(list_plan(Some(&DEFAULT_LIST)), p(true, Some("after"), Some("bottom")));
        assert_eq!(list_plan(Some(&["widgets", "rooms", "account"])), p(true, Some("before"), Some("bottom")));
        assert_eq!(list_plan(Some(&["account", "rooms", "widgets"])), p(true, Some("after"), Some("top")));
        assert_eq!(list_plan(Some(&["rooms", "account", "widgets"])), p(true, Some("after"), Some("bottom")));
        assert_eq!(list_plan(Some(&["rooms"])), p(true, None, None));
        // rooms hidden: the account card's place is decided against the widgets
        assert_eq!(list_plan(Some(&["account", "widgets"])), p(false, Some("after"), Some("top")));
        assert_eq!(list_plan(Some(&["widgets", "account"])), p(false, Some("after"), Some("bottom")));
        assert_eq!(list_plan(Some(&[])), p(false, None, None));
    }

    // ---- buttons and drawers (EDITMODE-UI §7.2, §7.3) — session.test.ts has the same cases, same canonical answers --
    #[test]
    fn add_button_holds_one_action_with_its_look() {
        let f0 = classic();
        let (a, ra) = new_action_button(&f0, &j(r#"{"a":"call.mic.toggle"}"#), "iconLabel").expect("a button");
        assert_eq!(ra, "buttons:b1");
        assert!(placed_refs(&a).contains(&"buttons:b1".to_string()));
        assert_eq!(
            a.get("opts").unwrap().get("buttons:b1").unwrap().canon(),
            r#"{"rows":{"main":{"items":[{"a":"call.mic.toggle"}],"look":"iconLabel"}}}"#
        );
        assert!(validate_desktop(Some(&a)).is_empty());
        let (t, rt) = new_action_button(&a, &j(r#"{"a":"nav.room.open","target":{"room":"!abc:example.org"}}"#), "icon").unwrap();
        assert_eq!(rt, "buttons:b2");
        assert_eq!(
            t.get("opts").unwrap().get("buttons:b2").unwrap().canon(),
            r#"{"rows":{"main":{"items":[{"a":"nav.room.open","target":{"room":"!abc:example.org"}}],"look":"icon"}}}"#
        );
        assert_eq!(new_action_button(&f0, &j(r#"{"a":"Bad"}"#), "icon").err().as_deref(), Some("bad-item"));
        assert_eq!(new_action_button(&f0, &j(r#"{"b":"call.mic.toggle"}"#), "icon").err().as_deref(), Some("bad-item"));
        assert_eq!(new_action_button(&f0, &j(r#""call.mic.toggle""#), "icon").err().as_deref(), Some("bad-item"));
        assert_eq!(new_action_button(&f0, &j(r#"{"a":"call.mic.toggle"}"#), "big").err().as_deref(), Some("bad-look"));
    }

    #[test]
    fn add_drawer_hides_the_island_and_adds_its_button() {
        let f0 = classic();
        assert_eq!(drawer_targets(&f0), vec!["nav", "dms", "spaces", "rooms", "widgets", "account"]);
        let (d, rd) = new_drawer(&f0, "rooms", "star").expect("a drawer");
        assert_eq!(rd, "buttons:b1");
        assert!(hidden_refs(&d).contains(&"rooms".to_string()));
        assert!(!placed_refs(&d).contains(&"rooms".to_string()));
        assert_eq!(
            d.get("opts").unwrap().get("buttons:b1").unwrap().canon(),
            r#"{"rows":{"main":{"items":[{"a":"layout.drawer.toggle","target":{"icon":"star","island":"rooms"}}],"look":"icon"}}}"#
        );
        assert!(validate_desktop(Some(&d)).is_empty());
        assert_eq!(drawer_targets(&d), vec!["nav", "dms", "spaces", "rooms", "widgets", "account"], "not its own button");
        let (p, _) = new_button_island(&d).unwrap();
        assert_eq!(drawer_targets(&p), vec!["nav", "dms", "spaces", "rooms", "widgets", "account", "buttons:b2"], "a plain button island may");
        assert_eq!(new_drawer(&d, "buttons:b1", "").err().as_deref(), Some("not-drawer-island"));
        let (e, _) = new_drawer(&d, "rooms", "").unwrap();
        assert_eq!(
            e.get("opts").unwrap().get("buttons:b2").unwrap().canon(),
            r#"{"rows":{"main":{"items":[{"a":"layout.drawer.toggle","target":{"island":"rooms"}}],"look":"icon"}}}"#
        );
        assert_eq!(new_drawer(&f0, "timeline", "star").err().as_deref(), Some("not-drawer-island"));
        assert_eq!(new_drawer(&f0, "panel", "star").err().as_deref(), Some("not-drawer-island"));
        assert_eq!(new_drawer(&f0, "rooms:x", "").err().as_deref(), Some("not-drawer-island"));
        assert_eq!(new_drawer(&f0, "rooms", "rocket").err().as_deref(), Some("bad-icon"));
    }

    #[test]
    fn drawer_form_floats_the_island_beside_its_button() {
        let (d, _) = new_drawer(&classic(), "rooms", "star").unwrap();
        let (w, h) = (1560, 960);
        let (o, id) = drawer_form(&d, "rooms", Rect { x: 100, y: 50, w: 40, h: 40 }, w, h).expect("open");
        assert_eq!(id, "drawer");
        let last = |f: &J| floats_of(f).last().unwrap().canon();
        assert_eq!(last(&o), r#"{"anchor":"tl","dx":100,"dy":94,"id":"drawer","node":{"i":"rooms","t":"island"}}"#);
        assert!(!hidden_refs(&o).contains(&"rooms".to_string()));
        assert!(placed_refs(&o).contains(&"rooms".to_string()));
        assert!(validate_desktop(Some(&o)).is_empty());
        let (br, _) = drawer_form(&d, "rooms", Rect { x: 1400, y: 900, w: 40, h: 40 }, w, h).unwrap();
        assert_eq!(last(&br), r#"{"anchor":"br","dx":120,"dy":64,"id":"drawer","node":{"i":"rooms","t":"island"}}"#);
        assert_eq!(drawer_form(&d, "widgets", Rect { x: 0, y: 0, w: 40, h: 40 }, w, h).err().as_deref(), Some("not-hidden"));
        let hw = apply(&o, r#"{"op":"hide","ref":"widgets"}"#);
        let (_, id2) = drawer_form(&hw, "widgets", Rect { x: 100, y: 50, w: 40, h: 40 }, w, h).unwrap();
        assert_eq!(id2, "drawer-2");
        assert!(hidden_refs(&d).contains(&"rooms".to_string()), "the form it opened from is untouched");
        // the input domain: clamped to ±65536 — the same numbers as TypeScript (the bridge truncates fractions)
        let (hg, _) = drawer_form(&d, "rooms", Rect { x: 9007199254740991, y: 50, w: 2, h: 40 }, 9007199254740991, 960).unwrap();
        assert_eq!(last(&hg), r#"{"anchor":"tr","dx":-2,"dy":94,"id":"drawer","node":{"i":"rooms","t":"island"}}"#);
    }

    #[test]
    fn a_drawer_can_hold_a_button_island() {
        // assembly-042: a button island tucked into a drawer opens as a placed island WITH its row, so a host that
        // renders the drawer form's islands draws its buttons (the desktop rendered only islands placed in the saved form)
        let (f, b1) = new_action_button(&classic(), &j(r#"{"a":"app.settings.open"}"#), "icon").unwrap();
        assert_eq!(b1, "buttons:b1");
        let row = f.get("opts").unwrap().get("buttons:b1").unwrap().canon();
        assert!(row.contains("app.settings.open"));
        let (d, launcher) = new_drawer(&f, "buttons:b1", "star").expect("a button island may go into a drawer");
        assert_ne!(launcher, "buttons:b1");
        assert!(hidden_refs(&d).contains(&"buttons:b1".to_string()));
        let (o, _) = drawer_form(&d, "buttons:b1", Rect { x: 100, y: 50, w: 40, h: 40 }, 1560, 960).expect("opens");
        assert!(placed_refs(&o).contains(&"buttons:b1".to_string()));
        assert_eq!(o.get("opts").unwrap().get("buttons:b1").unwrap().canon(), row, "its buttons come along");
        assert!(validate_desktop(Some(&o)).is_empty());
        // its own launcher's island can't go into a drawer
        assert_eq!(new_drawer(&d, &launcher, "").err().as_deref(), Some("not-drawer-island"));
    }

    #[test]
    fn a_drawer_must_be_able_to_open() {
        let mut f = classic();
        for _ in 0..8 {
            f = new_button_island(&f).unwrap().0;
        }
        assert_eq!(new_drawer(&f, "buttons:b1", "").err().as_deref(), Some("too-many"));
        // seven placed + one hidden: its launcher would be the eighth, opening it the ninth
        let seven = apply(&f, r#"{"op":"hide","ref":"buttons:b8"}"#);
        assert_eq!(new_drawer(&seven, "buttons:b8", "").err().as_deref(), Some("too-many"));
        // seven in all (the engine's limit counts placed + hidden: 8), one of them hidden: launcher 7th placed, opened 8th
        let mut g7 = classic();
        for _ in 0..7 {
            g7 = new_button_island(&g7).unwrap().0;
        }
        let six = apply(&g7, r#"{"op":"hide","ref":"buttons:b7"}"#);
        let r6 = new_drawer(&six, "buttons:b7", "");
        assert!(r6.is_ok(), "six placed + one hidden: fits: {:?}", r6.err());
        assert_eq!(new_drawer(&f, "rooms", "").err().as_deref(), Some("too-many"), "rooms is fine, but its button would be a ninth");
        let hidden = apply(&f, r#"{"op":"hide","ref":"buttons:b8"}"#);
        assert!(drawer_form(&hidden, "buttons:b8", Rect { x: 0, y: 0, w: 40, h: 40 }, 1560, 960).is_ok(), "eight placed again is fine");
        let mut g = classic();
        for r in ["nav", "dms", "spaces", "widgets", "account"] {
            g = apply(&g, &format!(r#"{{"op":"float","ref":"{r}","anchor":"tl","dx":16,"dy":16}}"#));
        }
        for i in 1..=3 {
            g = apply(&new_button_island(&g).unwrap().0, &format!(r#"{{"op":"float","ref":"buttons:b{i}","anchor":"br","dx":16,"dy":16}}"#));
        }
        assert_eq!(floats_of(&g).len(), 8);
        assert_eq!(new_drawer(&g, "rooms", "").err().as_deref(), Some("too-many-floats"));
        assert!(placed_refs(&g).contains(&"rooms".to_string()), "refused before anything was hidden");
        // an island that floats alone: hiding it frees its slot, so it may go into a drawer even at 8 floats
        let mut h7 = classic();
        for r in ["nav", "dms", "spaces", "widgets", "account"] {
            h7 = apply(&h7, &format!(r#"{{"op":"float","ref":"{r}","anchor":"tl","dx":16,"dy":16}}"#));
        }
        for i in 1..=2 {
            h7 = apply(&new_button_island(&h7).unwrap().0, &format!(r#"{{"op":"float","ref":"buttons:b{i}","anchor":"br","dx":16,"dy":16}}"#));
        }
        let r8 = apply(&h7, r#"{"op":"float","ref":"rooms","anchor":"tr","dx":16,"dy":16}"#);
        assert_eq!(floats_of(&r8).len(), 8);
        assert!(new_drawer(&r8, "rooms", "").is_ok(), "a floating island at the limit may go into a drawer");
        // drawer_form itself refuses what may not be in a drawer, whatever a layout says
        let hp = apply(&classic(), r#"{"op":"hide","ref":"panel"}"#);
        assert_eq!(drawer_form(&hp, "panel", Rect { x: 0, y: 0, w: 40, h: 40 }, 1560, 960).err().as_deref(), Some("not-drawer-island"));
    }

    #[test]
    fn an_older_tin_hat_keeps_a_drawer() {
        // 12.1.x runs this same engine (repair, ops) and does not know layout.drawer.toggle: what it would do to the layout
        let (d, _) = new_drawer(&classic(), "rooms", "star").unwrap();
        let opts = d.get("opts").unwrap().get("buttons:b1").unwrap().canon();
        assert_eq!(opts, r#"{"rows":{"main":{"items":[{"a":"layout.drawer.toggle","target":{"icon":"star","island":"rooms"}}],"look":"icon"}}}"#);
        assert!(same(&repair_desktop(Some(&d)).form, &d), "repair changes nothing");
        let h = apply(&d, r#"{"op":"hide","ref":"widgets"}"#);
        assert_eq!(h.get("opts").unwrap().get("buttons:b1").unwrap().canon(), opts, "the button item survives an unrelated edit");
        assert_eq!(hidden_refs(&h), vec!["rooms", "widgets"]);
    }
}
