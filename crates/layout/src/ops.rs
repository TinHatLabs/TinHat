//! ops.rs — port of `ops.ts`: LAYOUT.md §8: transactional operations. `apply_op(form, op, ctx)` → `{ok, form, remap}`
//! | `{ok: false, error}`; the input form is never modified. Steps: shape → resolve → target → read-only → root →
//! capabilities → mutate a copy without normalizing → placement rules → normalize once (collect the remap) →
//! validate completely (limits included) → result.

use std::collections::BTreeMap;

use crate::form::{
    axis_of_container, basis_of, detach, find_float, find_key, floats_of, hidden_index, insert_beside,
    insert_edge, is_container, is_island, is_stack, is_unknown_island, is_unknown_node, islands_in, key_of, kids_arr,
    kids_of, loc_of, nodes_in, place_by_home, placement_error, ref_of, reg_sizes, share_one_keeping, size_for_axis,
    Ids, Loc, Side, ANCHORS,
};
use crate::json::{clamp_f, fdiv, is_json_value, mk, J};
use crate::normalize::normalize_in_place;
use crate::repair::validate_desktop;
use crate::schema::{def_of, is_known_ref, is_ref_syntax, preset_desktop, Axis, PRESET_NAMES};
use crate::solve::{solve_detail, unscale, SolveDetail};

/// An operation's answer.
#[derive(Debug, Clone)]
pub enum OpResult {
    Ok { form: J, remap: BTreeMap<String, String> },
    Err(String),
}

impl OpResult {
    pub fn is_ok(&self) -> bool {
        matches!(self, OpResult::Ok { .. })
    }
    /// The vector / controller form: `{form, remap}` or `{error}` (the reference's `plain({form, remap})`).
    pub fn to_json(&self) -> J {
        match self {
            OpResult::Ok { form, remap } => {
                let r = J::obj();
                for (k, v) in remap {
                    r.set(k, J::s(v));
                }
                mk(vec![("form", form.clone()), ("remap", r)])
            }
            OpResult::Err(e) => mk(vec![("error", J::s(e))]),
        }
    }
}

fn fail(e: &str) -> OpResult {
    OpResult::Err(e.to_string())
}
const BAD: &str = "bad-op: bad request";

/// The solve context of resize / resetSizes / dropZones: `{size, unitPm, shape, content, view}`.
pub fn ctx_solve(form: &J, ctx: Option<&J>) -> SolveDetail {
    let c = ctx.filter(|c| c.is_obj()).cloned().unwrap_or_else(J::obj);
    solve_detail(form, c.get("size").as_ref(), c.get("unitPm").as_ref(), c.get("shape").as_ref(), c.get("content").as_ref(), c.get("view").as_ref())
}
fn ctx_unit(ctx: Option<&J>) -> i64 {
    let u = ctx.filter(|c| c.is_obj()).and_then(|c| c.get("unitPm"));
    match u {
        Some(u) if u.is_int() => clamp_f(u.as_f64().unwrap_or(1000.0), 500.0, 4000.0) as i64,
        _ => 1000,
    }
}

// ---- step 2: resolve ------------------------------------------------------------------------------------------
fn resolve_moved(f: &J, r: Option<&J>) -> Result<(Loc, bool), &'static str> {
    let Some(r) = r.and_then(J::as_str) else { return Err(BAD) };
    let Some(l) = find_key(f, r) else { return Err("bad-op: island not placed") };
    if is_island(&l.node) {
        if !is_known_ref(r) {
            return Err("bad-op: unknown island");
        }
        return Ok((l, false));
    }
    Ok((l, true))
}
/// Step 4's read-only rule: the node is inside an unknown node, or a group holding unknown nodes / islands.
fn read_only(l: &Loc) -> bool {
    if l.in_unknown {
        return true;
    }
    nodes_in(&l.node).iter().any(|n| is_unknown_node(n) || is_unknown_island(n))
}
fn old_axis_of(l: &Loc) -> Option<Axis> {
    l.parent.as_ref().and_then(axis_of_container)
}

// ---- steps 7-10 -------------------------------------------------------------------------------------------------
fn finish(f: &J) -> OpResult {
    if let Some(pe) = placement_error(f) {
        return fail(pe);
    }
    let n = normalize_in_place(f);
    let v = validate_desktop(Some(&n.form));
    if let Some(first) = v.first() {
        let (code, subject) = match first.find(' ') {
            Some(i) => (&first[..i], &first[i + 1..]),
            None => (first.as_str(), ""),
        };
        if code == "replaced-by-classic" {
            return fail(match subject {
                "too-large" => "bad-layout: too large",
                "too-deep" => "bad-layout: too deep",
                "too-many-nodes" => "bad-layout: too many nodes",
                _ => "bad-layout: over a limit",
            });
        }
        if code == "collapse-unsafe-dropped" {
            return fail("bad-op: required island");
        }
        if code == "bad-opts-fixed" {
            return fail(BAD);
        }
        return fail("bad-op: locked group");
    }
    OpResult::Ok { form: n.form, remap: n.remap }
}

// ---- the operations ----------------------------------------------------------------------------------------------
/// Give an inserted node its size for the new parent (LAYOUT.md §8.2).
fn size_after_move(f: &J, n: &J, old_parent: Option<&J>, old_axis: Option<Axis>, new_parent: &J, keep: bool) {
    if keep {
        return;
    }
    let ax = axis_of_container(new_parent).unwrap_or(Axis::H);
    if is_island(n) {
        if old_parent.is_some_and(|p| p.same(new_parent)) {
            return; // a reorder inside the same container keeps its size
        }
        n.set("size", size_for_axis(f, n, ax));
    } else if old_axis != axis_of_container(new_parent) {
        n.set("size", share_one_keeping(n));
    }
}

#[allow(clippy::too_many_arguments)]
fn do_beside(f: &J, moved: &J, old_parent: Option<&J>, old_axis: Option<Axis>, target_key: Option<&J>, side: Option<&J>, keep: bool, ids: &mut Ids) -> Option<&'static str> {
    let (Some(tk), Some(side)) = (target_key.and_then(J::as_str), Side::parse_j(side)) else { return Some(BAD) };
    let Some(t) = find_key(f, tk) else { return Some("bad-op: target not found") };
    if t.in_unknown || is_unknown_island(&t.node) {
        return Some("bad-op: read-only layout");
    }
    let np = insert_beside(f, moved, &t, side, ids);
    size_after_move(f, moved, old_parent, old_axis, &np, keep);
    None
}
fn do_edge(f: &J, moved: &J, old_parent: Option<&J>, old_axis: Option<Axis>, side: Option<&J>, keep: bool, ids: &mut Ids) -> Option<&'static str> {
    let Some(side) = Side::parse_j(side) else { return Some(BAD) };
    let np = insert_edge(f, moved, side, ids);
    size_after_move(f, moved, old_parent, old_axis, &np, keep);
    None
}
fn do_tab(f: &J, moved: &J, target_key: Option<&J>, ids: &mut Ids) -> Option<&'static str> {
    let Some(tk) = target_key.and_then(J::as_str) else { return Some(BAD) };
    if !is_island(moved) {
        return Some("bad-op: not stackable");
    }
    let Some(t) = find_key(f, tk) else { return Some("bad-op: target not found") };
    if t.in_unknown || is_unknown_island(&t.node) {
        return Some("bad-op: read-only layout");
    }
    let r = moved.get("i").unwrap_or_default();
    if is_stack(&t.node) {
        if let Some(c) = kids_arr(&t.node) {
            c.push(moved.clone());
        }
        t.node.set("active", r);
        return None;
    }
    if !is_island(&t.node) || !ref_of(&t.node).and_then(|x| def_of(&x)).is_some_and(|d| d.stackable) {
        return Some("bad-op: not stackable");
    }
    if let Some(p) = t.parent.as_ref().filter(|p| is_stack(p)) {
        if let Some(c) = kids_arr(p) {
            let i = c.index_of(&t.node).map_or(0, |i| i + 1);
            c.insert_at(i, moved.clone());
        }
        p.set("active", r);
        return None;
    }
    let s = mk(vec![("t", J::s("stack")), ("id", J::s(&ids.next("s")))]);
    if let Some(sz) = t.node.get("size") {
        s.set("size", sz.deep());
    }
    s.set("active", r);
    s.set("c", J::arr(vec![t.node.clone(), moved.clone()]));
    let Some(tl) = loc_of(f, &t.node) else { return Some("bad-op: not stackable") };
    if let Some(p) = &tl.parent {
        if let Some(c) = kids_arr(p) {
            if let Some(i) = c.index_of(&t.node) {
                c.set_at(i, s);
            }
        }
    } else if tl.float >= 0 {
        if let Some(x) = floats_of(f).get(tl.float as usize) {
            x.set("node", s);
        }
    } else {
        return Some("bad-op: not stackable");
    }
    None
}
fn do_float(f: &J, moved: &J, to: &J, ids: &mut Ids) -> Option<&'static str> {
    let fid = ids.next("f");
    let fl = mk(vec![
        ("id", J::s(&fid)),
        ("anchor", to.get("anchor").unwrap_or_default()),
        ("dx", to.get("dx").unwrap_or_default()),
        ("dy", to.get("dy").unwrap_or_default()),
    ]);
    if is_island(moved) {
        if let Some(fs) = ref_of(moved).and_then(|r| def_of(&r)).and_then(|d| d.float.as_ref()) {
            fl.set("w", J::from_value(&fs[0]));
            fl.set("h", J::from_value(&fs[1]));
        }
    }
    fl.set("reveal", to.get("reveal").unwrap_or_else(|| J::s("always")));
    fl.set("node", moved.clone());
    if !f.has("floats") {
        f.set("floats", J::arr(vec![]));
    }
    if let Some(a) = f.get("floats").filter(J::is_arr) {
        a.push(fl);
    }
    None
}

fn check_float_args(op: &J) -> Option<&'static str> {
    if !op.get("anchor").as_ref().and_then(J::as_str).is_some_and(|a| ANCHORS.contains(&a)) {
        return Some(BAD);
    }
    let in_range = |k: &str| op.get(k).is_some_and(|v| v.is_int() && (-4096.0..=4096.0).contains(&v.as_f64().unwrap_or(0.0)));
    if !in_range("dx") || !in_range("dy") {
        return Some(BAD);
    }
    match op.get("reveal") {
        None => {}
        Some(r) if r.eq_str("always") || r.eq_str("hover") => {}
        Some(_) => return Some(BAD),
    }
    None
}
/// Step 5 for a placement (`to`) of `n`: floatable / stackable / required (hover).
fn check_placement(n: &J, to: &J) -> Option<&'static str> {
    let kind = to.get_str("op");
    match kind.as_deref() {
        Some("float") => {
            if let Some(e) = check_float_args(to) {
                return Some(e);
            }
            for r in islands_in(n) {
                let d = def_of(&r);
                if d.is_some_and(|d| !d.floatable) {
                    return Some("bad-op: not floatable");
                }
                if d.is_some_and(|d| d.required) && to.is("reveal", "hover") {
                    return Some("bad-op: required island");
                }
            }
            None
        }
        Some("stackAsTab") => {
            if !is_island(n) || !ref_of(n).and_then(|r| def_of(&r)).is_some_and(|d| d.stackable) {
                return Some("bad-op: not stackable");
            }
            None
        }
        Some("dockBeside") | Some("edge") => None,
        _ => Some(BAD),
    }
}
/// Insert a detached node per a placement object (dockBeside / stackAsTab / edge / float). `ids` = N8 over the form as
/// it was BEFORE the node was detached (so a moved group's own ids are never handed out again).
fn place(f: &J, n: &J, to: &J, old_parent: Option<&J>, old_axis: Option<Axis>, keep: bool, ids: &mut Ids) -> Option<&'static str> {
    match to.get_str("op").as_deref() {
        Some("dockBeside") => do_beside(f, n, old_parent, old_axis, to.get("target").as_ref(), to.get("side").as_ref(), keep, ids),
        Some("edge") => do_edge(f, n, old_parent, old_axis, to.get("side").as_ref(), keep, ids),
        Some("stackAsTab") => do_tab(f, n, to.get("target").as_ref(), ids),
        Some("float") => do_float(f, n, to, ids),
        _ => Some(BAD),
    }
}

/// Move a placed node (island or group) per `to`: resolve, check, detach, insert, finish.
fn move_op(form: &J, r: Option<&J>, to: &J) -> OpResult {
    let f = form.deep();
    let (loc, _group) = match resolve_moved(&f, r) {
        Ok(m) => m,
        Err(e) => return fail(e),
    };
    // target checks that do not need the mutation: existence, same node, inside the moved group
    if to.is("op", "dockBeside") || to.is("op", "stackAsTab") {
        let Some(tk) = to.get_str("target") else { return fail(BAD) };
        let Some(t) = find_key(&f, &tk) else { return fail("bad-op: target not found") };
        if t.node.same(&loc.node) {
            return fail("bad-op: same island");
        }
        if nodes_in(&loc.node).iter().any(|n| n.same(&t.node)) {
            return fail("bad-op: target inside moved group");
        }
        if t.in_unknown || is_unknown_island(&t.node) {
            return fail("bad-op: read-only layout");
        }
    }
    if read_only(&loc) {
        return fail("bad-op: read-only layout");
    }
    if loc.parent.is_none() && loc.float < 0 {
        return fail("bad-op: locked group"); // the root does not move
    }
    if let Some(e) = check_placement(&loc.node, to) {
        return fail(e);
    }
    let old_parent = loc.parent.clone();
    let old_axis = old_axis_of(&loc);
    let mut ids = Ids::new(&f);
    detach(&f, &loc);
    if let Some(e) = place(&f, &loc.node, to, old_parent.as_ref(), old_axis, false, &mut ids) {
        return fail(e);
    }
    finish(&f)
}

/// The stored size as solve reads it on `axis`: with a basis as stored; else the registry pref (islands) or share 1,
/// with the stored constraints and unknown keys on top.
fn effective_size(f: &J, n: &J, axis: Axis) -> J {
    let stored = n.get("size").filter(J::is_obj).unwrap_or_else(J::obj);
    if basis_of(Some(&stored)).is_some() {
        return stored.deep();
    }
    let out = J::obj();
    let rs = if is_island(n) { ref_of(n).and_then(|r| reg_sizes(Some(f), &r)) } else { None };
    let src = match rs {
        Some(rs) => rs.pref_j(axis),
        None => mk(vec![("share", J::int(1))]),
    };
    for k in src.keys() {
        out.set(&k, src.get(&k).map(|v| v.deep()).unwrap_or_default());
    }
    for k in stored.keys() {
        out.set(&k, stored.get(&k).map(|v| v.deep()).unwrap_or_default());
    }
    out
}

fn resize_op(form: &J, op: &J, ctx: Option<&J>, reset: bool) -> OpResult {
    let (a, b) = (op.get_str("a"), op.get_str("b"));
    let delta = op.get("delta");
    let delta_ok = delta.as_ref().is_some_and(|d| d.is_int() && (-16384.0..=16384.0).contains(&d.as_f64().unwrap_or(0.0)));
    // v1.2: `into` is optional; present, it must be a boolean (the reference's shapeOk)
    let into_ok = op.get("into").map_or(true, |v| v.is_bool());
    let (Some(a), Some(b)) = (a, b) else { return fail(BAD) };
    if !reset && (!delta_ok || !into_ok) {
        return fail(BAD);
    }
    let det = ctx_solve(form, ctx);
    let mut rec: Option<(usize, usize)> = None;
    for (di, d) in det.dist.iter().enumerate() {
        for i in 0..d.kids.len().saturating_sub(1) {
            if *d.kids[i].key == *a && *d.kids[i + 1].key == *b {
                rec = Some((di, i));
            }
        }
    }
    let Some((di, i)) = rec else { return fail("bad-op: target not found") };
    let d = &det.dist[di];
    let f = form.deep();
    let (Some(la), Some(lb)) = (find_key(&f, &a), find_key(&f, &b)) else { return fail("bad-op: target not found") };
    if read_only(&la) || read_only(&lb) || is_unknown_node(&la.node) || is_unknown_node(&lb.node) {
        return fail("bad-op: read-only layout");
    }
    let axis = d.axis;
    if reset {
        for l in [&la, &lb] {
            let s = if is_island(&l.node) { size_for_axis(&f, &l.node, axis) } else { share_one_keeping(&l.node) };
            l.node.set("size", s);
        }
        return finish(&f);
    }
    let ka = &d.kids[i];
    let kb = &d.kids[i + 1];
    let (la_len, lb_len) = (ka.len, kb.len);
    let delta = delta.and_then(|d| d.int_val()).unwrap_or(0);
    let t = 4 * fdiv(la_len + delta + 2, 4);
    let mut lo = ka.min.max(la_len + lb_len - kb.max);
    let mut hi = ka.max.min(la_len + lb_len - kb.min);
    // v1.2 `into: true` (§8.2, Q-RESIZEINTO): a side that is a row / col with C's axis and an effective `auto` basis passes
    // the change on to its child next to the gap (a: its last laid-out child, b: its first — repeated while that child is
    // again such a container), unless that child's basis is share; the container keeps its auto size (it follows its
    // content). The child's own bounds then limit the move too.
    struct RSide {
        node: J,
        eff: J,
        len: i64,
        min: i64,
        max: i64,
        inner: i64,
    }
    let into = op.get("into").is_some_and(|v| v.is_true());
    let side = |l: &Loc, k: &crate::solve::DistKid, first: bool| -> RSide {
        let mut cur = RSide { node: l.node.clone(), eff: effective_size(&f, &l.node, axis), len: k.len, min: k.min, max: k.max, inner: d.inner };
        if !into {
            return cur;
        }
        for _guard in 0..64 {
            let n = cur.node.clone();
            if !is_container(&n) || axis_of_container(&n) != Some(axis) || basis_of(Some(&cur.eff)) != Some("auto") {
                break;
            }
            let Some(key) = key_of(&n) else { break };
            let Some(dd) = det.dist.iter().rev().find(|x| *x.key == *key) else { break };
            if dd.axis != axis || dd.kids.is_empty() {
                break;
            }
            let kid = if first { &dd.kids[0] } else { &dd.kids[dd.kids.len() - 1] };
            let Some(kl) = find_key(&f, &kid.key) else { break };
            if read_only(&kl) || is_unknown_node(&kl.node) {
                break;
            }
            let keff = effective_size(&f, &kl.node, axis);
            if basis_of(Some(&keff)) == Some("share") {
                break;
            }
            cur = RSide { node: kl.node.clone(), eff: keff, len: kid.len, min: kid.min, max: kid.max, inner: dd.inner };
        }
        cur
    };
    let sa = side(&la, ka, false);
    let sb = side(&lb, kb, true);
    if !sa.node.same(&la.node) {
        lo = lo.max(la_len - (sa.len - sa.min));
        hi = hi.min(la_len + (sa.max - sa.len));
    }
    if !sb.node.same(&lb.node) {
        lo = lo.max(la_len + sb.len - sb.max);
        hi = hi.min(la_len + sb.len - sb.min);
    }
    if lo > hi {
        return fail("bad-op: no room");
    }
    let n_a = if t < lo {
        lo
    } else if t > hi {
        hi
    } else {
        t
    };
    let d_a = n_a - la_len;
    let unit = ctx_unit(ctx);
    let share_a = basis_of(Some(&sa.eff)) == Some("share");
    let share_b = basis_of(Some(&sb.eff)) == Some("share");
    let write = |sd: &RSide, len: i64, as_pm: bool| -> Option<&'static str> {
        let s = sd.eff.deep();
        let basis = basis_of(Some(&s));
        let lc = sd.inner;
        for k in ["fixed", "share", "auto", "pm"] {
            s.del(k);
        }
        if as_pm || basis == Some("pm") {
            if lc <= 0 {
                return Some("bad-op: no room");
            }
            s.set("pm", J::int(fdiv(len * 1000 + fdiv(lc, 2), lc).clamp(0, 1000)));
            if basis == Some("share") {
                s.del("grow");
            }
        } else {
            s.set("fixed", J::int(unscale(len, unit).clamp(0, 8192)));
        }
        sd.node.set("size", s);
        None
    };
    let len_a = sa.len + d_a;
    let len_b = sb.len - d_a;
    let e = if share_a && share_b {
        write(&sa, len_a, true)
    } else if share_a {
        write(&sb, len_b, false)
    } else if share_b {
        write(&sa, len_a, false)
    } else {
        write(&sa, len_a, false).or_else(|| write(&sb, len_b, false))
    };
    if let Some(e) = e {
        return fail(e);
    }
    finish(&f)
}

fn hide_op(form: &J, r: Option<&J>) -> OpResult {
    let f = form.deep();
    let (loc, group) = match resolve_moved(&f, r) {
        Ok(m) => m,
        Err(e) => return fail(e),
    };
    if read_only(&loc) {
        return fail("bad-op: read-only layout");
    }
    if loc.parent.is_none() && loc.float < 0 {
        return fail("bad-op: locked group"); // the root
    }
    if group {
        return fail("bad-op: not hideable");
    }
    let d = ref_of(&loc.node).and_then(|r| def_of(&r));
    if d.is_some_and(|d| d.required) {
        return fail("bad-op: required island");
    }
    if d.is_some_and(|d| !d.hideable) {
        return fail("bad-op: not hideable");
    }
    // (mk skips an undefined value: a parent without a key gives no `parent`)
    let from = J::obj();
    match &loc.parent {
        Some(p) => {
            let idx = kids_of(p).iter().position(|k| k.same(&loc.node)).map_or(-1, |i| i as i64);
            if let Some(k) = key_of(p) {
                from.set("parent", J::Str(k));
            }
            from.set("index", J::int(idx));
        }
        None => {
            if let Some(fid) = floats_of(&f).get(loc.float as usize).and_then(|x| x.get("id")) {
                from.set("parent", fid);
            }
            from.set("index", J::int(0));
        }
    }
    detach(&f, &loc);
    let leaf = loc.node.clone();
    leaf.set("from", from);
    if !f.has("hidden") {
        f.set("hidden", J::arr(vec![]));
    }
    if let Some(h) = f.get("hidden").filter(J::is_arr) {
        h.push(leaf);
    }
    finish(&f)
}

fn restore_op(form: &J, r: Option<&J>, to: Option<&J>) -> OpResult {
    let Some(r) = r.and_then(J::as_str) else { return fail(BAD) };
    if to.is_some_and(|t| !t.is_obj()) {
        return fail(BAD);
    }
    let f = form.deep();
    let Some(hi) = hidden_index(&f, r) else { return fail("bad-op: island not hidden") };
    if !is_known_ref(r) {
        return fail("bad-op: unknown island");
    }
    let mut ids = Ids::new(&f);
    let leaf = f.get("hidden").and_then(|h| h.remove_at(hi)).unwrap_or_default();
    let from = leaf.get("from");
    leaf.del("from");
    if let Some(to) = to.filter(|t| t.is_obj()) {
        if to.is("op", "dockBeside") || to.is("op", "stackAsTab") {
            let Some(tk) = to.get_str("target") else { return fail(BAD) };
            let Some(t) = find_key(&f, &tk) else { return fail("bad-op: target not found") };
            if t.in_unknown || is_unknown_island(&t.node) {
                return fail("bad-op: read-only layout");
            }
        }
        if let Some(e) = check_placement(&leaf, to) {
            return fail(e);
        }
        if let Some(e) = place(&f, &leaf, to, None, None, true, &mut ids) {
            return fail(e);
        }
        return finish(&f);
    }
    // at `from` when that node exists and accepts it, else the registry home rules
    if let Some(from) = from.filter(|x| x.is_obj()) {
        if let (Some(pk), Some(idx)) = (from.get_str("parent"), from.get("index").filter(|i| i.is_int())) {
            let trial = f.deep();
            let lf = leaf.deep();
            let idx = idx.as_f64().unwrap_or(0.0);
            let mut slot: Option<J> = None;
            let pl = find_key(&trial, &pk);
            if let Some(pl) = pl.filter(|pl| !pl.in_unknown && (is_container(&pl.node) || is_stack(&pl.node))) {
                slot = Some(pl.node);
            } else if let Some(fi) = find_float(&trial, &pk) {
                let node = floats_of(&trial).get(fi).and_then(|x| x.get("node"));
                if let Some(node) = node.filter(|n| n.is_obj() && is_container(n)) {
                    slot = Some(node);
                }
            }
            if let Some(slot) = slot {
                let stack_refuses = is_stack(&slot) && !def_of(r).is_some_and(|d| d.stackable);
                if !stack_refuses {
                    if let Some(c) = kids_arr(&slot) {
                        // splice(Math.min(idx, c.length), 0, lf) — a negative index counts from the end
                        let n = c.len() as f64;
                        let at = idx.min(n);
                        let at = if at < 0.0 { (n + at).max(0.0) } else { at };
                        c.insert_at(at as usize, lf);
                    }
                    if placement_error(&trial).is_none() {
                        return finish(&trial);
                    }
                }
            }
        }
    }
    place_by_home(&f, &leaf, &mut ids, &|g: &J| placement_error(g).is_none());
    finish(&f)
}

fn set_opt_op(form: &J, op: &J) -> OpResult {
    let r = op.get_str("ref");
    let key = op.get_str("key");
    let (Some(r), Some(key)) = (r.filter(|r| is_ref_syntax(r)), key.filter(|k| !k.is_empty())) else { return fail(BAD) };
    if &*key == "composerTop" || &*key == "headerBottom" {
        return fail(BAD); // the tree records the order (N9)
    }
    let f = form.deep();
    let opts = match f.get("opts").filter(J::is_obj) {
        Some(o) => o,
        None => {
            let o = J::obj();
            f.set("opts", o.clone());
            o
        }
    };
    let o = match opts.get(&r).filter(J::is_obj) {
        Some(o) => o,
        None => {
            let o = J::obj();
            opts.set(&r, o.clone());
            o
        }
    };
    if op.has("value") {
        o.set(&key, op.get("value").map(|v| v.deep()).unwrap_or_default());
    } else {
        o.del(&key);
        if o.nkeys() == 0 {
            opts.del(&r);
        }
    }
    finish(&f)
}

fn is_side(v: Option<J>) -> bool {
    Side::parse_j(v.as_ref()).is_some()
}
/// A placement object (`to` of dock / restore): dockBeside {target, side}, stackAsTab {target}, edge {side}, float.
fn placement_shape_ok(to: Option<J>, allow_float: bool) -> bool {
    let Some(to) = to.filter(J::is_obj) else { return false };
    match to.get_str("op").as_deref() {
        Some("dockBeside") => to.get("target").is_some_and(|t| t.is_str()) && is_side(to.get("side")),
        Some("stackAsTab") => to.get("target").is_some_and(|t| t.is_str()),
        Some("edge") => is_side(to.get("side")),
        Some("float") => allow_float && check_float_args(&to).is_none(),
        _ => false,
    }
}
/// §8.1 step 1: the op object's shape (every field an operation reads, typed).
fn shape_ok(op: &J) -> bool {
    let s = |k: &str| op.get(k).is_some_and(|v| v.is_str());
    match op.get_str("op").as_deref() {
        Some("dockBeside") => s("ref") && s("target") && is_side(op.get("side")),
        Some("stackAsTab") => s("ref") && s("target"),
        Some("edge") => s("ref") && is_side(op.get("side")),
        Some("float") => s("ref") && check_float_args(op).is_none(),
        Some("dock") => s("ref") && placement_shape_ok(op.get("to"), false),
        Some("resize") => {
            let d = op.get("delta");
            s("a") && s("b") && d.is_some_and(|d| d.is_int() && (-16384.0..=16384.0).contains(&d.as_f64().unwrap_or(0.0)))
        }
        Some("resetSizes") => s("a") && s("b"),
        Some("hide") => s("ref"),
        Some("restore") => s("ref") && (!op.has("to") || placement_shape_ok(op.get("to"), true)),
        Some("setOpt") => {
            let k = op.get_str("key");
            op.get_str("ref").is_some_and(|r| is_ref_syntax(&r))
                && k.is_some_and(|k| !k.is_empty() && &*k != "composerTop" && &*k != "headerBottom")
        }
        Some("applyPreset") => op.get_str("preset").is_some_and(|p| PRESET_NAMES.contains(&&*p)),
        Some("reset") => true,
        _ => false,
    }
}

/// LAYOUT.md §8: apply one operation. Never panics; the input is never mutated.
pub fn apply_op(form: &J, op: Option<&J>, ctx: Option<&J>) -> OpResult {
    // §1.4: the input domain first (an absent ctx is fine; an absent op is not JSON data)
    if !is_json_value(Some(form)) || !form.is_obj() || !is_json_value(op) || ctx.is_some_and(|c| !is_json_value(Some(c))) {
        return fail(BAD);
    }
    let Some(op) = op.filter(|o| o.is_obj()) else { return fail(BAD) };
    if !op.get("op").is_some_and(|o| o.is_str()) || !shape_ok(op) {
        return fail(BAD);
    }
    let kind = op.get_str("op").expect("checked");
    // resize and resetSizes read lengths from a solve: they need ctx.size {w, h} (integers)
    if &*kind == "resize" || &*kind == "resetSizes" {
        let sz = ctx.filter(|c| c.is_obj()).and_then(|c| c.get("size"));
        let ok = sz.as_ref().is_some_and(|s| s.is_obj() && s.get("w").is_some_and(|w| w.is_int()) && s.get("h").is_some_and(|h| h.is_int()));
        if !ok {
            return fail(BAD);
        }
    }
    match &*kind {
        "dockBeside" | "stackAsTab" | "edge" => move_op(form, op.get("ref").as_ref(), op),
        "float" => {
            if let Some(e) = check_float_args(op) {
                return fail(e);
            }
            move_op(form, op.get("ref").as_ref(), op)
        }
        "dock" => {
            let to = op.get("to");
            let r = op.get_str("ref");
            let (Some(to), Some(r)) = (to.filter(|t| t.is_obj()), r) else { return fail(BAD) };
            if !to.get_str("op").is_some_and(|o| matches!(&*o, "dockBeside" | "stackAsTab" | "edge")) {
                return fail(BAD);
            }
            let key: J;
            if let Some(fi) = find_float(form, &r) {
                let node = floats_of(form).get(fi).and_then(|x| x.get("node"));
                match node.filter(J::is_obj).and_then(|n| key_of(&n)) {
                    Some(k) => key = J::Str(k),
                    None => return fail("bad-op: read-only layout"),
                }
            } else {
                let Some(l) = find_key(form, &r) else { return fail("bad-op: island not placed") };
                if l.float < 0 {
                    return fail("bad-op: island not placed");
                }
                key = J::Str(r);
            }
            move_op(form, Some(&key), &to)
        }
        "resize" => resize_op(form, op, ctx, false),
        "resetSizes" => resize_op(form, op, ctx, true),
        "hide" => hide_op(form, op.get("ref").as_ref()),
        "restore" => restore_op(form, op.get("ref").as_ref(), op.get("to").as_ref()),
        "applyPreset" | "reset" => {
            let name = if &*kind == "reset" { Some("classic".into()) } else { op.get_str("preset").map(|p| p.to_string()) };
            let Some(name) = name.filter(|n| PRESET_NAMES.contains(&n.as_str())) else { return fail(BAD) };
            match preset_desktop(&name) {
                Some(p) => finish(&p),
                None => fail(BAD),
            }
        }
        "setOpt" => set_opt_op(form, op),
        _ => fail(BAD),
    }
}
