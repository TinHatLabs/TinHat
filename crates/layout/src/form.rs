//! form.rs — port of `form.ts`: the desktop form's tree (LAYOUT.md §3): node kinds, keys, locations, size fields,
//! the placement rules (R1-R3 + registry), the insertion primitives shared by the operations and repair, home rules
//! and id allocation (N8). Forms are plain JSON data ([`J`]); unknown keys ride along untouched.

use std::collections::BTreeSet;
use std::rc::Rc;

use crate::json::{clamp_f, mk, J};
use crate::schema::{def, def_of, is_id_syntax, is_known_ref, is_reserved_id, is_type_syntax, kind_of, Axis, RegSizes};

// ---- known keys (LAYOUT.md §3.1) -------------------------------------------------------------------------------
pub const K_CONTAINER: [&str; 5] = ["t", "id", "c", "size", "gap"];
pub const K_STACK: [&str; 5] = ["t", "id", "c", "size", "active"];
pub const K_ISLAND: [&str; 3] = ["t", "i", "size"];
pub const K_SIZE: [&str; 9] = ["fixed", "share", "auto", "pm", "min", "max", "minPm", "maxPm", "grow"];

pub const BASES: [&str; 4] = ["fixed", "share", "auto", "pm"];
pub const SIDES: [&str; 4] = ["left", "right", "top", "bottom"];
pub const ANCHORS: [&str; 8] = ["tl", "t", "tr", "r", "br", "b", "bl", "l"];
pub const STEP_TOS: [&str; 4] = ["fold", "hide", "overlay", "drawer"];

/// `left | right | top | bottom`
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

impl Side {
    pub fn parse(s: &str) -> Option<Side> {
        match s {
            "left" => Some(Side::Left),
            "right" => Some(Side::Right),
            "top" => Some(Side::Top),
            "bottom" => Some(Side::Bottom),
            _ => None,
        }
    }
    pub fn parse_j(v: Option<&J>) -> Option<Side> {
        v.and_then(J::as_str).and_then(Side::parse)
    }
    pub fn name(self) -> &'static str {
        match self {
            Side::Left => "left",
            Side::Right => "right",
            Side::Top => "top",
            Side::Bottom => "bottom",
        }
    }
    /// left / top = before
    pub fn first(self) -> bool {
        matches!(self, Side::Left | Side::Top)
    }
}

// ---- node kinds and keys ---------------------------------------------------------------------------------------
pub fn t_of(n: &J) -> Option<Rc<str>> {
    n.get_str("t")
}
/// An object with a type name matching the node type syntax.
pub fn is_node(v: &J) -> bool {
    v.is_obj() && v.get_str("t").is_some_and(|t| is_type_syntax(&t))
}
pub fn is_container(n: &J) -> bool {
    n.is("t", "row") || n.is("t", "col")
}
pub fn is_col(n: &J) -> bool {
    n.is("t", "col")
}
pub fn is_stack(n: &J) -> bool {
    n.is("t", "stack")
}
pub fn is_island(n: &J) -> bool {
    n.is("t", "island")
}
pub fn is_known_type(n: &J) -> bool {
    is_container(n) || is_stack(n) || is_island(n)
}
pub fn is_unknown_node(n: &J) -> bool {
    is_node(n) && !is_known_type(n)
}
/// An island node whose ref this version does not render (not in the registry).
pub fn is_unknown_island(n: &J) -> bool {
    is_island(n) && n.get_str("i").is_some_and(|r| !is_known_ref(&r))
}
/// `refOf(n)`: the island's `i` (when it is a string).
pub fn ref_of(n: &J) -> Option<Rc<str>> {
    n.get_str("i")
}
/// The children of a node (references), empty when `c` is not an array.
pub fn kids_of(n: &J) -> Vec<J> {
    n.get("c").map(|c| c.items()).unwrap_or_default()
}
/// The node's `c` array itself (for in-place edits), when it is one.
pub fn kids_arr(n: &J) -> Option<J> {
    n.get("c").filter(J::is_arr)
}
/// The identity of a node in the one namespace (LAYOUT.md §2.4): island ref, or container / stack id.
pub fn key_of(n: &J) -> Option<Rc<str>> {
    if is_island(n) {
        return n.get_str("i");
    }
    if is_container(n) || is_stack(n) {
        return n.get_str("id");
    }
    None
}
pub fn axis_of_container(n: &J) -> Option<Axis> {
    if n.is("t", "row") {
        Some(Axis::W)
    } else if n.is("t", "col") {
        Some(Axis::H)
    } else {
        None
    }
}
pub fn axis_of_side(s: Side) -> Axis {
    match s {
        Side::Left | Side::Right => Axis::W,
        _ => Axis::H,
    }
}
pub fn type_of_axis(a: Axis) -> &'static str {
    match a {
        Axis::W => "row",
        Axis::H => "col",
    }
}

/// Keys of `o` outside `known`.
pub fn unknown_keys(o: &J, known: &[&str]) -> Vec<String> {
    o.keys().into_iter().filter(|k| !known.contains(&k.as_str())).collect()
}
pub fn size_has_unknown(s: Option<&J>) -> bool {
    s.is_some_and(|s| s.is_obj() && !unknown_keys(s, &K_SIZE).is_empty())
}
/// The node carries keys this version does not know — on itself or inside its `size` (LAYOUT.md §4).
pub fn carries_unknown(n: &J) -> bool {
    if !is_known_type(n) {
        return true;
    }
    let known: &[&str] = if is_container(n) {
        &K_CONTAINER
    } else if is_stack(n) {
        &K_STACK
    } else {
        &K_ISLAND
    };
    !unknown_keys(n, known).is_empty() || size_has_unknown(n.get("size").as_ref())
}

// ---- locations -------------------------------------------------------------------------------------------------
/// Where a node sits: `parent` holds it in `c` (None for a tree top); `float` = index in `floats` (-1 = root tree).
#[derive(Clone, Debug)]
pub struct Loc {
    pub node: J,
    pub parent: Option<J>,
    pub index: i64,
    pub float: i64,
    pub depth: usize,
    pub in_unknown: bool,
    pub ancestors: Vec<J>,
}

impl Loc {
    pub fn parent_is(&self, pred: impl Fn(&J) -> bool) -> bool {
        self.parent.as_ref().is_some_and(pred)
    }
}

pub fn floats_of(f: &J) -> Vec<J> {
    f.get("floats").map(|v| v.items()).unwrap_or_default()
}
pub fn hidden_of(f: &J) -> Vec<J> {
    f.get("hidden").map(|v| v.items()).unwrap_or_default()
}
pub fn collapse_of(f: &J) -> Vec<J> {
    f.get("collapse").map(|v| v.items()).unwrap_or_default()
}

fn walk_tree(top: &J, float: i64, out: &mut Vec<Loc>) {
    fn rec(node: &J, parent: Option<&J>, index: i64, float: i64, depth: usize, anc: &[J], in_u: bool, out: &mut Vec<Loc>) {
        out.push(Loc {
            node: node.clone(),
            parent: parent.cloned(),
            index,
            float,
            depth,
            in_unknown: in_u,
            ancestors: anc.to_vec(),
        });
        if is_island(node) {
            return;
        }
        let kids = kids_of(node);
        let mut anc2 = anc.to_vec();
        anc2.push(node.clone());
        let in_u2 = in_u || is_unknown_node(node);
        for (i, k) in kids.iter().enumerate() {
            if k.is_obj() {
                rec(k, Some(node), i as i64, float, depth + 1, &anc2, in_u2, out);
            }
        }
    }
    rec(top, None, if float < 0 { -1 } else { float }, float, 1, &[], false, out);
}

/// Every node of the root tree, then of each float's node, depth-first pre-order.
pub fn all_locs(f: &J) -> Vec<Loc> {
    let mut out = Vec::new();
    if let Some(root) = f.get("root").filter(J::is_obj) {
        walk_tree(&root, -1, &mut out);
    }
    for (i, fl) in floats_of(f).iter().enumerate() {
        if let Some(n) = fl.get("node").filter(J::is_obj) {
            walk_tree(&n, i as i64, &mut out);
        }
    }
    out
}
pub fn root_locs(f: &J) -> Vec<Loc> {
    all_locs(f).into_iter().filter(|l| l.float < 0).collect()
}

/// First node (root tree, then floats) whose key is `key`; known node types only.
pub fn find_key(f: &J, key: &str) -> Option<Loc> {
    all_locs(f).into_iter().find(|l| is_known_type(&l.node) && key_of(&l.node).as_deref() == Some(key))
}
pub fn find_float(f: &J, id: &str) -> Option<usize> {
    floats_of(f).iter().position(|x| x.is_obj() && x.is("id", id))
}
pub fn hidden_index(f: &J, r: &str) -> Option<usize> {
    hidden_of(f).iter().position(|h| h.is_obj() && h.is("i", r))
}

/// Island refs of a subtree, depth-first.
pub fn islands_in(n: &J) -> Vec<Rc<str>> {
    if is_island(n) {
        return n.get_str("i").into_iter().collect();
    }
    let mut out = Vec::new();
    for k in kids_of(n) {
        if k.is_obj() {
            out.extend(islands_in(&k));
        }
    }
    out
}
/// Every node of a subtree (itself included), depth-first pre-order.
pub fn nodes_in(n: &J) -> Vec<J> {
    let mut out = vec![n.clone()];
    if !is_island(n) {
        for k in kids_of(n) {
            if k.is_obj() {
                out.extend(nodes_in(&k));
            }
        }
    }
    out
}

/// Remove the located node from its slot. A float's node leaves the float without a node (normalization drops it).
pub fn detach(f: &J, l: &Loc) {
    if let Some(p) = &l.parent {
        if let Some(c) = kids_arr(p) {
            // splice(indexOf(node), 1): a node that is not there removes the last element (-1), as the reference would
            match c.index_of(&l.node) {
                Some(i) => {
                    c.remove_at(i);
                }
                None => {
                    let n = c.len();
                    if n > 0 {
                        c.remove_at(n - 1);
                    }
                }
            }
        }
    } else if l.float >= 0 {
        if let Some(fl) = floats_of(f).get(l.float as usize) {
            fl.del("node");
        }
    } else {
        debug_assert!(false, "layout: the root cannot be detached");
    }
}
/// Put `repl` where the located node is.
pub fn replace_at(f: &J, l: &Loc, repl: J) {
    if let Some(p) = &l.parent {
        if let Some(c) = kids_arr(p) {
            if let Some(i) = c.index_of(&l.node) {
                c.set_at(i, repl);
            }
        }
    } else if l.float >= 0 {
        if let Some(fl) = floats_of(f).get(l.float as usize) {
            fl.set("node", repl);
        }
    } else {
        f.set("root", repl);
    }
}

// ---- ids (N8) --------------------------------------------------------------------------------------------------
/// Every id and island ref in the form: node / float ids (any node type) and `i` values in root, floats and hidden.
pub fn used_names(f: &J) -> BTreeSet<String> {
    let mut s = BTreeSet::new();
    for l in all_locs(f) {
        if let Some(id) = l.node.get_str("id") {
            s.insert(id.to_string());
        }
        if let Some(i) = l.node.get_str("i") {
            s.insert(i.to_string());
        }
    }
    for fl in floats_of(f) {
        if let Some(id) = fl.get_str("id") {
            s.insert(id.to_string());
        }
    }
    for h in hidden_of(f) {
        if let Some(i) = h.get_str("i") {
            s.insert(i.to_string());
        }
        if let Some(x) = h.as_str() {
            s.insert(x.to_string());
        }
    }
    s
}

/// N8: `<prefix><k>` with the smallest positive k not used by any id or ref of the form.
#[derive(Clone, Debug)]
pub struct Ids {
    pub used: BTreeSet<String>,
}

impl Ids {
    pub fn new(f: &J) -> Ids {
        Ids { used: used_names(f) }
    }
    pub fn next(&mut self, prefix: &str) -> String {
        let mut k = 1u64;
        loop {
            let id = format!("{prefix}{k}");
            if !self.used.contains(&id) {
                self.used.insert(id.clone());
                return id;
            }
            k += 1;
        }
    }
    /// N5: `body` if free, else `body1`, `body2`, …
    pub fn body(&mut self) -> String {
        if !self.used.contains("body") {
            self.used.insert("body".into());
            return "body".into();
        }
        self.next("body")
    }
}

/// A usable container / stack / float id: the syntax, not reserved.
pub fn id_usable(id: &str) -> bool {
    is_id_syntax(id) && !is_reserved_id(id)
}

// ---- sizes (N6, LAYOUT.md §3.3) ---------------------------------------------------------------------------------
fn range_of(k: &str) -> (f64, f64) {
    match k {
        "fixed" | "min" | "max" => (0.0, 8192.0),
        "share" | "grow" => (1.0, 10000.0),
        _ => (0.0, 1000.0), // pm, minPm, maxPm
    }
}

/// N6 on a size object (a copy is returned; unknown keys kept): a known field of the wrong type is dropped (never
/// rounded), integers are clamped to their ranges, several bases keep the first of fixed, share, auto, pm, `grow`
/// needs an explicit non-share basis, max ≥ min and maxPm ≥ minPm. Not an object → None, changed.
pub fn fix_size(v: Option<&J>) -> (Option<J>, bool) {
    let Some(v) = v.filter(|v| v.is_obj()) else { return (None, true) };
    let s = v.deep();
    let mut changed = false;
    for k in ["fixed", "share", "pm", "min", "max", "minPm", "maxPm", "grow"] {
        let Some(x) = s.get(k) else { continue };
        if !x.is_int() {
            s.del(k);
            changed = true;
            continue;
        }
        let xv = x.as_f64().unwrap_or(0.0);
        let (lo, hi) = range_of(k);
        let c = clamp_f(xv, lo, hi);
        if c != xv {
            s.set(k, J::Num(c));
            changed = true;
        }
    }
    if s.has("auto") && !s.get("auto").is_some_and(|a| a.is_true()) {
        s.del("auto");
        changed = true;
    }
    let bases: Vec<&str> = BASES.iter().copied().filter(|b| s.has(b)).collect();
    for b in bases.iter().skip(1) {
        s.del(b);
        changed = true;
    }
    if s.has("grow") && (bases.is_empty() || bases[0] == "share") {
        s.del("grow");
        changed = true;
    }
    let num = |k: &str| s.get(k).and_then(|x| x.as_f64()).unwrap_or(0.0);
    if s.has("min") && s.has("max") && num("max") < num("min") {
        s.set("max", s.get("min").unwrap_or_default());
        changed = true;
    }
    if s.has("minPm") && s.has("maxPm") && num("maxPm") < num("minPm") {
        s.set("maxPm", s.get("minPm").unwrap_or_default());
        changed = true;
    }
    (Some(s), changed)
}
pub fn basis_of(s: Option<&J>) -> Option<&'static str> {
    let s = s?;
    BASES.iter().copied().find(|b| s.has(b))
}

/// Registry sizes of an island for the form's orientation option (`opts.<ref>.orient = "h"` → orientH).
pub fn reg_sizes(f: Option<&J>, r: &str) -> Option<&'static RegSizes> {
    let d = def_of(r)?;
    if let (Some(f), Some(oh)) = (f, d.orient_h.as_ref()) {
        let o = f.get("opts");
        let oo = o.filter(J::is_obj).and_then(|o| o.get(r));
        if oo.as_ref().is_some_and(|oo| oo.is_obj() && oo.is("orient", "h")) {
            return Some(oh);
        }
    }
    Some(&d.sizes)
}

/// A size for `n` on the axis of a new parent (LAYOUT.md §8.2): an island gets its registry pref for that axis, a
/// group `{share: 1}`; the unknown keys of the node's old size ride along.
pub fn size_for_axis(f: &J, n: &J, axis: Axis) -> J {
    let out = J::obj();
    if let Some(old) = n.get("size").filter(J::is_obj) {
        for k in unknown_keys(&old, &K_SIZE) {
            out.set(&k, old.get(&k).map(|v| v.deep()).unwrap_or_default());
        }
    }
    let rs = if is_island(n) { ref_of(n).and_then(|r| reg_sizes(Some(f), &r)) } else { None };
    let src = match rs {
        Some(rs) => rs.pref_j(axis),
        None => mk(vec![("share", J::int(1))]),
    };
    for k in src.keys() {
        out.set(&k, src.get(&k).map(|v| v.deep()).unwrap_or_default());
    }
    out
}
/// `{share: 1}` plus the unknown keys of the node's old size.
pub fn share_one_keeping(n: &J) -> J {
    let out = J::obj();
    if let Some(old) = n.get("size").filter(J::is_obj) {
        for k in unknown_keys(&old, &K_SIZE) {
            out.set(&k, old.get(&k).map(|v| v.deep()).unwrap_or_default());
        }
    }
    out.set("share", J::int(1));
    out
}

// ---- placement rules (LAYOUT.md §3.7) ------------------------------------------------------------------------------
#[derive(Default, Debug)]
pub struct RoomLocs {
    pub timeline: Option<Loc>,
    pub composer: Option<Loc>,
    pub header: Option<Loc>,
}

pub fn room_locs(f: &J) -> RoomLocs {
    let mut r = RoomLocs::default();
    for l in all_locs(f) {
        if !is_island(&l.node) {
            continue;
        }
        let Some(rf) = ref_of(&l.node) else { continue };
        match &*rf {
            "timeline" if r.timeline.is_none() => r.timeline = Some(l),
            "composer" if r.composer.is_none() => r.composer = Some(l),
            "header" if r.header.is_none() => r.header = Some(l),
            _ => {}
        }
    }
    r
}

fn is_col_opt(n: Option<&J>) -> bool {
    n.is_some_and(is_col)
}

/// R1: timeline and composer are adjacent children of one col (root tree).
pub fn r1ok(r: &RoomLocs) -> bool {
    let (Some(t), Some(c)) = (&r.timeline, &r.composer) else { return false };
    if t.float >= 0 || c.float >= 0 || !is_col_opt(t.parent.as_ref()) {
        return false;
    }
    let (Some(tp), Some(cp)) = (&t.parent, &c.parent) else { return false };
    if !tp.same(cp) {
        return false;
    }
    let k = J::arr(kids_of(tp));
    let ti = k.index_of(&t.node).map_or(-1, |i| i as i64);
    let ci = k.index_of(&c.node).map_or(-1, |i| i as i64);
    (ti - ci).abs() == 1
}
/// The col R2 measures from: the timeline's col (after R1).
pub fn timeline_col(r: &RoomLocs) -> Option<J> {
    let t = r.timeline.as_ref()?;
    t.parent.clone().filter(is_col)
}
/// R2 (header placed): first or last child of the timeline's col or of an ancestor col of it.
pub fn r2ok(r: &RoomLocs) -> bool {
    let Some(h) = &r.header else { return true };
    let tc = timeline_col(r);
    let (Some(tc), Some(t)) = (tc, &r.timeline) else { return false };
    if h.float >= 0 || !is_col_opt(h.parent.as_ref()) {
        return false;
    }
    let hp = h.parent.as_ref().expect("checked");
    let anc = &t.ancestors; // ancestors of the timeline: …, tc
    let proper = &anc[..anc.len().saturating_sub(1)];
    if !hp.same(&tc) && !proper.iter().any(|a| a.same(hp)) {
        return false;
    }
    let k = kids_of(hp);
    let i = k.iter().position(|x| x.same(&h.node));
    match i {
        Some(i) => i == 0 || i + 1 == k.len(),
        None => k.is_empty(), // indexOf -1: -1 === k.length - 1 only for an empty list
    }
}
/// groupEdge target: the nearest col that contains the timeline's col, else the timeline's col itself.
pub fn group_edge_col(r: &RoomLocs) -> Option<J> {
    let tc = timeline_col(r)?;
    let t = r.timeline.as_ref()?;
    let anc = &t.ancestors[..t.ancestors.len().saturating_sub(1)];
    for a in anc.iter().rev() {
        if is_col(a) {
            return Some(a.clone());
        }
    }
    Some(tc)
}

/// The placement rules an operation's mutated copy must meet before normalization (LAYOUT.md §8.1 step 7), in this
/// order: one occurrence per ref; hidden islands hideable and not required; stacks hold stackable islands only;
/// floated islands floatable, no required island in a hover float; the required islands placed; R1; R2/R3.
pub fn placement_error(f: &J) -> Option<&'static str> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let locs = all_locs(f);
    for l in &locs {
        if !is_island(&l.node) {
            continue;
        }
        let r = ref_of(&l.node).map(|r| r.to_string()).unwrap_or_else(|| "\u{0}undefined".into());
        if seen.contains(&r) {
            return Some("bad-op: locked group");
        }
        seen.insert(r);
    }
    for h in hidden_of(f) {
        let r = h.get_str("i");
        let key = r.as_ref().map(|r| r.to_string()).unwrap_or_else(|| "\u{0}undefined".into());
        if seen.contains(&key) {
            return Some("bad-op: locked group");
        }
        seen.insert(key);
        let d = r.as_ref().and_then(|r| def_of(r));
        if d.is_some_and(|d| d.required) {
            return Some("bad-op: required island");
        }
        if d.is_some_and(|d| !d.hideable) {
            return Some("bad-op: not hideable");
        }
    }
    for l in &locs {
        if !is_stack(&l.node) {
            continue;
        }
        for k in kids_of(&l.node) {
            if !is_island(&k) {
                return Some("bad-op: not stackable");
            }
            if ref_of(&k).and_then(|r| def_of(&r)).is_some_and(|d| !d.stackable) {
                return Some("bad-op: not stackable");
            }
        }
    }
    for fl in floats_of(f) {
        let Some(node) = fl.get("node").filter(J::is_obj) else { continue };
        for r in islands_in(&node) {
            let d = def_of(&r);
            if d.is_some_and(|d| !d.floatable) {
                return Some("bad-op: not floatable");
            }
            if d.is_some_and(|d| d.required) && fl.is("reveal", "hover") {
                return Some("bad-op: required island");
            }
        }
    }
    for req in ["timeline", "composer", "call"] {
        if !seen.contains(req) || hidden_index(f, req).is_some() {
            return Some("bad-op: required island");
        }
    }
    let room = room_locs(f);
    if !r1ok(&room) {
        return Some("bad-op: locked group");
    }
    if let Some(h) = &room.header {
        if h.float >= 0 || !r2ok(&room) {
            return Some("bad-op: locked group");
        }
    }
    for l in [&room.timeline, &room.composer, &room.header].into_iter().flatten() {
        if l.parent.as_ref().is_some_and(is_stack) {
            return Some("bad-op: locked group");
        }
    }
    None
}

// ---- insertion primitives (LAYOUT.md §8.2) --------------------------------------------------------------------------
/// dockBeside's insertion (the moved node is already detached): target in a stack → the stack; P = its parent; P has
/// the side's axis → insert before / after; else the target is replaced by a new container of that axis holding
/// [moved, target] or [target, moved], which takes the target's size while the target gets `{share: 1}`. Returns the
/// new parent of `moved`.
pub fn insert_beside(f: &J, moved: &J, target: &Loc, side: Side, ids: &mut Ids) -> J {
    let mut t = target.clone();
    if let Some(p) = &target.parent {
        if is_stack(p) {
            if let Some(l) = loc_of(f, p) {
                t = l;
            }
        }
    }
    let ty = type_of_axis(axis_of_side(side));
    let before = side.first();
    if let Some(p) = &t.parent {
        if is_container(p) && p.is("t", ty) {
            if let Some(c) = kids_arr(p) {
                let i = c.index_of(&t.node).map_or(-1, |i| i as i64);
                let at = if before { i } else { i + 1 };
                if at < 0 {
                    // splice(-1, 0, x): before the last element
                    let n = c.len();
                    c.insert_at(n.saturating_sub(1), moved.clone());
                } else {
                    c.insert_at(at as usize, moved.clone());
                }
            }
            return p.clone();
        }
    }
    let w = mk(vec![("t", J::s(ty)), ("id", J::s(&ids.next("n")))]);
    if let Some(ts) = t.node.get("size") {
        w.set("size", ts);
    }
    w.set("c", J::arr(if before { vec![moved.clone(), t.node.clone()] } else { vec![t.node.clone(), moved.clone()] }));
    t.node.set("size", mk(vec![("share", J::int(1))]));
    replace_at(f, &t, w.clone());
    w
}
/// edge's insertion: a new first / last child of the root, or a new root wrapping the old one. Returns the new parent.
pub fn insert_edge(f: &J, moved: &J, side: Side, ids: &mut Ids) -> J {
    let root = f.get("root").unwrap_or_default();
    let ty = type_of_axis(axis_of_side(side));
    let first = side.first();
    if root.is("t", ty) {
        if let Some(c) = kids_arr(&root) {
            if first {
                c.unshift(moved.clone());
            } else {
                c.push(moved.clone());
            }
        }
        return root;
    }
    root.set("size", share_one_keeping(&root));
    let w = mk(vec![
        ("t", J::s(ty)),
        ("id", J::s(&ids.next("n"))),
        ("c", J::arr(if first { vec![moved.clone(), root.clone()] } else { vec![root.clone(), moved.clone()] })),
    ]);
    f.set("root", w.clone());
    w
}
/// The location of a node object (by identity).
pub fn loc_of(f: &J, n: &J) -> Option<Loc> {
    all_locs(f).into_iter().find(|l| l.node.same(n))
}

// ---- home rules (LAYOUT.md §3.8) --------------------------------------------------------------------------------
/// Place `leaf` (not in the form) by its registry home rules, first applicable wins; a rule applies when its target
/// exists docked (outside unknown nodes) and the result meets `check`. Fallback: wrap the root on the side of the
/// island's last edge rule (left if it has none). Mutates `f`.
pub fn place_by_home(f: &J, leaf: &J, ids: &mut Ids, check: &dyn Fn(&J) -> bool) {
    let r = ref_of(leaf).map(|r| r.to_string()).unwrap_or_default();
    let rules: Vec<J> = def(kind_of(&r)).map(|d| d.home.iter().map(J::from_value).collect()).unwrap_or_default();
    let mut last_edge = Side::Left;
    for rule in &rules {
        let trial = f.deep();
        let lf = leaf.deep();
        let mut t_ids = Ids::new(&trial);
        for u in &ids.used {
            t_ids.used.insert(u.clone());
        }
        if let Some(e) = rule.get_str("edge") {
            if let Some(s) = Side::parse(&e) {
                last_edge = s;
            }
        }
        if !apply_home_rule(&trial, &lf, rule, &mut t_ids) {
            continue;
        }
        if !check(&trial) {
            continue;
        }
        f.replace_entries(&trial);
        for u in t_ids.used {
            ids.used.insert(u);
        }
        return;
    }
    // fallback: a new root around the old one
    let root = f.get("root").unwrap_or_default();
    let ty = type_of_axis(axis_of_side(last_edge));
    root.set("size", share_one_keeping(&root));
    let first = last_edge.first();
    f.set(
        "root",
        mk(vec![
            ("t", J::s(ty)),
            ("id", J::s(&ids.next("n"))),
            ("c", J::arr(if first { vec![leaf.clone(), root.clone()] } else { vec![root.clone(), leaf.clone()] })),
        ]),
    );
}

fn apply_home_rule(f: &J, leaf: &J, rule: &J, ids: &mut Ids) -> bool {
    if let Some(e) = rule.get_str("edge") {
        if let Some(side) = Side::parse(&e) {
            insert_edge(f, leaf, side, ids);
            return true;
        }
        return false;
    }
    if let Some(ge) = rule.get_str("groupEdge") {
        let Some(col) = group_edge_col(&room_locs(f)) else { return false };
        if let Some(c) = kids_arr(&col) {
            if &*ge == "top" {
                c.unshift(leaf.clone());
            } else {
                c.push(leaf.clone());
            }
        }
        return true;
    }
    let Some(beside) = rule.get_str("beside") else { return false };
    let Some(x) = find_key(f, &beside) else { return false };
    if x.float >= 0 || x.in_unknown || !is_island(&x.node) {
        return false;
    }
    let mut target = x.clone();
    if rule.get("container").is_some_and(|c| c.is_true()) {
        let Some(p) = &x.parent else { return false };
        let Some(pl) = loc_of(f, p) else { return false };
        if pl.in_unknown || is_unknown_node(&pl.node) {
            return false;
        }
        target = pl;
    }
    let Some(side) = Side::parse_j(rule.get("side").as_ref()) else { return false };
    insert_beside(f, leaf, &target, side, ids);
    true
}

/// The node types this version edits: known types outside unknown nodes.
pub fn editable(l: &Loc) -> bool {
    !l.in_unknown && is_known_type(&l.node)
}
