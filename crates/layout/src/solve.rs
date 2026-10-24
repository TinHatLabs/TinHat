//! solve.rs — port of `solve.ts`: LAYOUT.md §7: solve(form, size, unitPm, shape, content, view) → integer rects for
//! every island. Pure, integer-only (i64 everywhere; every quotient is [`fdiv`] = floor with a positive divisor), no
//! floating point, no hash-order iteration. Inputs are read leniently and clamped to `x-tinhat.bounds` first (§7.1).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::rc::Rc;

use crate::form::{fix_size, floats_of, is_container, is_island, is_stack, kids_of, reg_sizes};
use crate::json::{clamp_f, fdiv, is_json_value, js_str, mk, J};
use crate::schema::{def, def_of, is_compat_ref, is_known_ref, is_main, schema, Axis, Presence, Sz, SHAPE_KEYS};

/// An unbounded maximum (LAYOUT.md §7).
const INF: i64 = 1 << 40;
/// §7.1 input caps: keys of view.sizes / active / popouts and content refs read (code-point order), forced steps.
pub const VIEW_KEY_CAP: usize = 256;
pub const FORCE_CAP: usize = 64;

/// The first `n` keys of an object in code-point order.
fn first_keys(o: &J, n: usize) -> Vec<String> {
    o.keys().into_iter().take(n).collect()
}
/// An input that is JSON data (§1.4), else nothing (`dataOrNothing`).
fn data_or_nothing(v: Option<&J>) -> Option<&J> {
    v.filter(|v| is_json_value(Some(v)))
}

const GAP: usize = 0;
const MARGIN: usize = 1;
const TAB: usize = 2;
const HANDLE: usize = 3;
const DRAWER_W: usize = 4;
const DRAWER_PAD: usize = 5;

/// sc(v) = sign(v) · fdiv(|v| · unitPm + 500, 1000): layout units → px, half away from zero.
pub fn sc(v: i64, unit: i64) -> i64 {
    if v < 0 {
        return -sc(-v, unit);
    }
    fdiv(v * unit + 500, 1000)
}
/// unscale(px) = fdiv(px · 1000 + fdiv(unitPm, 2), unitPm), px ≥ 0.
pub fn unscale(px: i64, unit: i64) -> i64 {
    fdiv(px * 1000 + fdiv(unit, 2), unit)
}

fn clamp(v: i64, lo: i64, hi: i64) -> i64 {
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}
/// An integer input clamped to a range (`isInt(x) ? clamp(x, lo, hi)`).
fn int_in(v: Option<&J>, lo: i64, hi: i64) -> Option<i64> {
    let v = v?;
    if !v.is_int() {
        return None;
    }
    Some(clamp_f(v.as_f64()?, lo as f64, hi as f64) as i64)
}

// ---- inputs ----------------------------------------------------------------------------------------------------
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
}

impl Rect {
    fn len(&self, a: Axis) -> i64 {
        match a {
            Axis::W => self.w,
            Axis::H => self.h,
        }
    }
    pub fn to_json(&self) -> J {
        mk(vec![("x", J::int(self.x)), ("y", J::int(self.y)), ("w", J::int(self.w)), ("h", J::int(self.h))])
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Content {
    w: Option<i64>,
    h: Option<i64>,
    min_w: Option<i64>,
    min_h: Option<i64>,
    collapsed_h: Option<i64>,
    bar_h: Option<i64>,
    empty: bool,
}

#[derive(Debug, Default)]
struct View {
    room_open: bool,
    call_shown: bool,
    panel_open: bool,
    maximized: Option<String>,
    force_collapse: Vec<String>,
    sizes: BTreeMap<String, Sz>,
    active: BTreeMap<String, String>,
    open: Option<String>,
    popouts: BTreeSet<String>,
}

/// §7.1: the view state as solve reads it (defaults: roomOpen true, everything else off). Capabilities are enforced
/// here: `popouts` only names islands whose registry allows pop-out, `maximized` only a `maximizable` island (the
/// call). At most 256 keys of sizes / active / popouts (code-point order) and 64 forced steps.
fn read_view(v: Option<&J>) -> View {
    let o = data_or_nothing(v).filter(|v| v.is_obj()).cloned().unwrap_or_else(J::obj);
    let mut sizes = BTreeMap::new();
    if let Some(sv) = o.get("sizes").filter(J::is_obj) {
        for k in first_keys(&sv, VIEW_KEY_CAP) {
            let x = sv.get(&k).unwrap_or_default();
            if x.is_obj() {
                sizes.insert(k, Sz::of(fix_size(Some(&x)).0.as_ref()));
            }
        }
    }
    let mut active = BTreeMap::new();
    if let Some(av) = o.get("active").filter(J::is_obj) {
        for k in first_keys(&av, VIEW_KEY_CAP) {
            if let Some(s) = av.get_str(&k) {
                active.insert(k, s.to_string());
            }
        }
    }
    let open = o.get("drawer").filter(J::is_obj).and_then(|d| d.get_str("open")).map(|s| s.to_string());
    // the strings, repeats removed, the first 64 (the reference's `[...new Set(…)].slice(0, 64)`; stopping at the 64th
    // distinct one gives the same list and keeps the walk linear)
    let mut force_collapse: Vec<String> = Vec::new();
    if let Some(fc) = o.get("forceCollapse").filter(J::is_arr) {
        for x in fc.items() {
            if force_collapse.len() == FORCE_CAP {
                break;
            }
            if let Some(s) = x.as_str() {
                if !force_collapse.iter().any(|y| y == s) {
                    force_collapse.push(s.to_string());
                }
            }
        }
    }
    let mut popouts = BTreeSet::new();
    if let Some(po) = o.get("popouts").filter(J::is_obj) {
        for k in first_keys(&po, VIEW_KEY_CAP) {
            if is_known_ref(&k) && def_of(&k).is_some_and(|d| d.popout) {
                popouts.insert(k);
            }
        }
    }
    let maximized = o.get_str("maximized").filter(|m| is_known_ref(m) && def_of(m).is_some_and(|d| d.maximizable)).map(|s| s.to_string());
    View {
        room_open: o.get("roomOpen").and_then(|r| r.as_bool()).unwrap_or(true),
        call_shown: o.get("callShown").is_some_and(|x| x.is_true()),
        panel_open: o.get("panelOpen").is_some_and(|x| x.is_true()),
        maximized,
        force_collapse,
        sizes,
        active,
        open,
        popouts,
    }
}

fn read_content(v: Option<&J>) -> HashMap<String, Content> {
    let mut out = HashMap::new();
    let Some(v) = data_or_nothing(v).filter(|v| v.is_obj()) else { return out };
    let (lo, hi) = schema().bounds.content_px;
    for r in first_keys(v, VIEW_KEY_CAP) {
        let c = v.get(&r).unwrap_or_default();
        if !c.is_obj() {
            continue;
        }
        let g = |k: &str| int_in(c.get(k).as_ref(), lo, hi);
        out.insert(
            r,
            Content {
                w: g("w"),
                h: g("h"),
                min_w: g("minW"),
                min_h: g("minH"),
                collapsed_h: g("collapsedH"),
                bar_h: g("barH"),
                empty: c.get("empty").is_some_and(|e| e.is_true()),
            },
        );
    }
    out
}

fn read_shape(v: Option<&J>, form: &J) -> [i64; 6] {
    let v = data_or_nothing(v);
    let (lo, hi) = schema().bounds.layout_units;
    let mut s = [0i64; 6];
    for (i, k) in SHAPE_KEYS.iter().enumerate() {
        let x = v.filter(|v| v.is_obj()).and_then(|v| v.get(k));
        s[i] = int_in(x.as_ref(), lo, hi).unwrap_or(schema().shape_defaults[i]);
    }
    // the form's gap / margin override the theme's
    for (i, k) in [(GAP, "gap"), (MARGIN, "margin")] {
        if let Some(x) = int_in(form.get(k).as_ref(), lo, hi) {
            s[i] = x;
        }
    }
    // usable chrome (§7.1): handles, tab strips and drawer panels never shrink below their minimums
    for (i, m) in schema().shape_min.iter().enumerate() {
        if s[i] < *m {
            s[i] = *m;
        }
    }
    s
}

/// §7.8 cut: r keeps out of q (with the gap g) — when they intersect, r becomes the largest of its parts above, below,
/// left of and right of q (ties: that order), or an empty rect at its corner when none is left.
pub fn cut_around(r: Rect, q: Rect, g: i64) -> Rect {
    if !(r.x < q.x + q.w && q.x < r.x + r.w && r.y < q.y + q.h && q.y < r.y + r.h) {
        return r;
    }
    let cands = [
        Rect { x: r.x, y: r.y, w: r.w, h: q.y - g - r.y },
        Rect { x: r.x, y: q.y + q.h + g, w: r.w, h: r.y + r.h - (q.y + q.h + g) },
        Rect { x: r.x, y: r.y, w: q.x - g - r.x, h: r.h },
        Rect { x: q.x + q.w + g, y: r.y, w: r.x + r.w - (q.x + q.w + g), h: r.h },
    ];
    let mut best: Option<Rect> = None;
    for c in cands {
        if c.w > 0 && c.h > 0 && best.is_none_or(|b| c.w * c.h > b.w * b.h) {
            best = Some(c);
        }
    }
    best.unwrap_or(Rect { x: r.x, y: r.y, w: 0, h: 0 })
}

// ---- the solve tree --------------------------------------------------------------------------------------------
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Row,
    Col,
    Stack,
    Island,
}

#[derive(Debug)]
struct SN {
    kind: Kind,
    key: Rc<str>,
    rf: Rc<str>,
    orig: J,
    unknown: bool,
    kids: Vec<N>,
    /// a column the solver makes (the emergency drawer's, a merged drawer's, an overflow drawer's): not in `containers`
    synthetic: bool,
}
type N = Rc<SN>;

fn is_row_col(n: &SN) -> bool {
    matches!(n.kind, Kind::Row | Kind::Col)
}
fn axis_of(n: &SN) -> Axis {
    if n.kind == Kind::Row {
        Axis::W
    } else {
        Axis::H
    }
}
fn leaf(r: &str) -> N {
    let r: Rc<str> = Rc::from(r);
    Rc::new(SN { kind: Kind::Island, key: r.clone(), rf: r, orig: J::obj(), unknown: false, kids: vec![], synthetic: false })
}
fn synthetic_col(key: String, kids: Vec<N>) -> N {
    Rc::new(SN { kind: Kind::Col, key: Rc::from(key.as_str()), rf: Rc::from(""), orig: J::obj(), unknown: true, kids, synthetic: true })
}

/// Build a solve tree; unknown node types become cols keyed `~<preorder index>` (`~f<i>.<index>` in float i).
fn build(n: &J, prefix: &str, counter: &mut usize) -> N {
    let idx = *counter;
    *counter += 1;
    if is_island(n) {
        let r: Rc<str> = Rc::from(js_str(n.get("i").as_ref()).as_str());
        return Rc::new(SN { kind: Kind::Island, key: r.clone(), rf: r, orig: n.clone(), unknown: false, kids: vec![], synthetic: false });
    }
    let known = is_container(n) || is_stack(n);
    let kind = if !known {
        Kind::Col
    } else if n.is("t", "row") {
        Kind::Row
    } else if n.is("t", "col") {
        Kind::Col
    } else {
        Kind::Stack
    };
    let key: Rc<str> = if known { Rc::from(js_str(n.get("id").as_ref()).as_str()) } else { Rc::from(format!("~{prefix}{idx}").as_str()) };
    let mut kids = Vec::new();
    for k in kids_of(n) {
        if k.is_obj() {
            kids.push(build(&k, prefix, counter));
        }
    }
    Rc::new(SN { kind, key, rf: Rc::from(""), orig: n.clone(), unknown: !known, kids, synthetic: false })
}

// ---- output ------------------------------------------------------------------------------------------------------
#[derive(Clone, Debug)]
pub struct IslandOut {
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
    pub z: i64,
    pub vis: bool,
    pub inset: Option<(Option<i64>, Option<i64>)>,
}
#[derive(Clone, Debug)]
pub struct StackOut {
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
    pub z: i64,
    pub vis: bool,
    pub tabs: Vec<Rc<str>>,
    pub active: Rc<str>,
    pub tab: i64,
}
#[derive(Clone, Debug)]
pub struct ContainerOut {
    pub r: Rect,
    pub z: i64,
}
#[derive(Clone, Debug)]
pub struct DrawerOut {
    pub id: String,
    pub side: String,
    pub handle: Rect,
    pub panel: Rect,
    pub open: bool,
    pub scroll: Option<i64>,
    pub merged: Option<Vec<String>>,
    pub count: Option<i64>,
    pub vis: Option<bool>,
    pub z: Option<i64>,
}
#[derive(Clone, Debug)]
pub struct GapOut {
    pub id: String,
    pub axis: &'static str,
    pub r: Rect,
    pub a: String,
    pub b: String,
}
#[derive(Clone, Debug, Default)]
pub struct SolveOut {
    pub islands: BTreeMap<String, IslandOut>,
    pub stacks: BTreeMap<String, StackOut>,
    pub containers: BTreeMap<String, ContainerOut>,
    pub drawers: Vec<DrawerOut>,
    pub gaps: Vec<GapOut>,
    pub collapsed: Vec<String>,
    pub overflow: Vec<String>,
    pub squeezed: Vec<String>,
    pub emergency: bool,
    pub too_small: bool,
}

impl SolveOut {
    /// The §7.13 output as JSON data (`inset` / `scroll` / `merged` / `count` / `vis` / `tooSmall` only when present).
    pub fn to_json(&self) -> J {
        let islands = J::obj();
        for (k, r) in &self.islands {
            let o = mk(vec![
                ("x", J::int(r.x)),
                ("y", J::int(r.y)),
                ("w", J::int(r.w)),
                ("h", J::int(r.h)),
                ("z", J::int(r.z)),
                ("vis", J::Bool(r.vis)),
            ]);
            if let Some((l, rr)) = r.inset {
                let ins = J::obj();
                if let Some(l) = l {
                    ins.set("l", J::int(l));
                }
                if let Some(rr) = rr {
                    ins.set("r", J::int(rr));
                }
                o.set("inset", ins);
            }
            islands.set(k, o);
        }
        let stacks = J::obj();
        for (k, s) in &self.stacks {
            stacks.set(
                k,
                mk(vec![
                    ("x", J::int(s.x)),
                    ("y", J::int(s.y)),
                    ("w", J::int(s.w)),
                    ("h", J::int(s.h)),
                    ("z", J::int(s.z)),
                    ("vis", J::Bool(s.vis)),
                    ("tabs", J::arr(s.tabs.iter().map(|t| J::Str(t.clone())).collect())),
                    ("active", J::Str(s.active.clone())),
                    ("tab", J::int(s.tab)),
                ]),
            );
        }
        let containers = J::obj();
        for (k, c) in &self.containers {
            let o = c.r.to_json();
            o.set("z", J::int(c.z));
            containers.set(k, o);
        }
        let drawers = J::arr(
            self.drawers
                .iter()
                .map(|d| {
                    let o = mk(vec![
                        ("id", J::s(&d.id)),
                        ("side", J::s(&d.side)),
                        ("handle", d.handle.to_json()),
                        ("panel", d.panel.to_json()),
                        ("open", J::Bool(d.open)),
                    ]);
                    if let Some(s) = d.scroll {
                        o.set("scroll", J::int(s));
                    }
                    if let Some(m) = &d.merged {
                        o.set("merged", J::strs(m));
                    }
                    if let Some(c) = d.count {
                        o.set("count", J::int(c));
                    }
                    if let Some(v) = d.vis {
                        o.set("vis", J::Bool(v));
                    }
                    if let Some(z) = d.z {
                        o.set("z", J::int(z));
                    }
                    o
                })
                .collect(),
        );
        let gaps = J::arr(
            self.gaps
                .iter()
                .map(|g| {
                    let o = g.r.to_json();
                    o.set("id", J::s(&g.id));
                    o.set("axis", J::s(g.axis));
                    o.set("a", J::s(&g.a));
                    o.set("b", J::s(&g.b));
                    o
                })
                .collect(),
        );
        let out = mk(vec![
            ("islands", islands),
            ("stacks", stacks),
            ("containers", containers),
            ("drawers", drawers),
            ("gaps", gaps),
            ("collapsed", J::strs(&self.collapsed)),
            ("overflow", J::strs(&self.overflow)),
            ("squeezed", J::strs(&self.squeezed)),
            ("emergency", J::Bool(self.emergency)),
        ]);
        if self.too_small {
            out.set("tooSmall", J::Bool(true));
        }
        out
    }
}

#[derive(Clone, Debug)]
struct Chip {
    key: String,
    axis: &'static str,
    rect: Rect,
    refs: Vec<String>,
    z: i64,
    layer_open: bool,
}

/// One child of a docked container's final distribution.
#[derive(Clone, Debug)]
pub struct DistKid {
    pub key: Rc<str>,
    pub len: i64,
    pub min: i64,
    pub max: i64,
}
/// Per docked container: its distribution (the operations read lengths and bounds from it; LAYOUT.md §8.2).
#[derive(Clone, Debug)]
pub struct DistRec {
    pub key: Rc<str>,
    pub axis: Axis,
    pub inner: i64,
    pub kids: Vec<DistKid>,
}

#[derive(Default)]
struct Rec {
    islands: BTreeMap<String, IslandOut>,
    stacks: BTreeMap<String, StackOut>,
    containers: BTreeMap<String, ContainerOut>,
    rects: HashMap<String, Rect>,
    inner: HashMap<String, i64>,
    gaps: Vec<GapOut>,
    overflow: Vec<String>,
    breach: BTreeSet<String>,
    squeezed: Vec<String>,
    chips: Vec<Chip>,
    /// insertion-ordered (a JavaScript Map)
    dist: Vec<DistRec>,
    tree: Option<N>,
}

#[derive(Clone, Debug)]
struct Step {
    rf: String,
    to: String,
    below: Option<i64>,
    w: Option<i64>,
    forced: bool,
}

/// solve + the docked distribution records the operations read (resize, resetSizes, dropZones).
pub struct SolveDetail {
    pub out: SolveOut,
    pub dist: Vec<DistRec>,
    pub rects: HashMap<String, Rect>,
    pub host: Rect,
}

struct Dist {
    base: Vec<i64>,
    mins: Vec<i64>,
    maxs: Vec<i64>,
    g: i64,
    l: i64,
    over: bool,
    breach: bool,
    squeezed: Vec<usize>,
}

struct DrawerItem {
    rf: String,
    w: Option<i64>,
    node: N,
    merged: Option<Vec<String>>,
}

// ---- the solver --------------------------------------------------------------------------------------------------
struct Solver<'a> {
    form: &'a J,
    ww: i64,
    hh: i64,
    unit: i64,
    shape: [i64; 6],
    content: HashMap<String, Content>,
    view: View,
    gap: i64,
    margin: i64,
    host: Rect,
    folded: BTreeSet<String>,
    emergency_mode: bool,
    root: Option<N>,
    floats: Vec<J>,
    float_trees: Vec<Option<N>>,
    /// protected controls: overlays and always-floats are cut around these (§7.8, §7.10); `true` marks the floating
    /// call's own rect (the reference compares that object by identity)
    guard: Vec<(Rect, bool)>,
}

impl<'a> Solver<'a> {
    fn new(form: &'a J, size: Option<&J>, unit_pm: Option<&J>, shape: Option<&J>, content: Option<&J>, view: Option<&J>) -> Solver<'a> {
        let b = &schema().bounds;
        let (hlo, hhi) = b.host_px;
        let size = data_or_nothing(size).filter(|s| s.is_obj());
        let ww = int_in(size.and_then(|s| s.get("w")).as_ref(), hlo, hhi).unwrap_or(0);
        let hh = int_in(size.and_then(|s| s.get("h")).as_ref(), hlo, hhi).unwrap_or(0);
        let (ulo, uhi) = b.unit_pm;
        let unit = int_in(unit_pm, ulo, uhi).unwrap_or(1000);
        let shape = read_shape(shape, form);
        let content = read_content(content);
        let view = read_view(view);
        let gap = sc(shape[GAP], unit);
        let margin = sc(shape[MARGIN], unit).min(fdiv(ww, 8)).min(fdiv(hh, 8));
        let m = margin;
        let host = Rect { x: m, y: m, w: (ww - 2 * m).max(0), h: (hh - 2 * m).max(0) };
        let root = form.get("root").filter(J::is_obj).map(|r| build(&r, "", &mut 0));
        let floats = floats_of(form);
        let float_trees = floats
            .iter()
            .enumerate()
            .map(|(i, f)| f.get("node").filter(J::is_obj).map(|n| build(&n, &format!("f{i}."), &mut 0)))
            .collect();
        Solver {
            form,
            ww,
            hh,
            unit,
            shape,
            content,
            view,
            gap,
            margin,
            host,
            folded: BTreeSet::new(),
            emergency_mode: false,
            root,
            floats,
            float_trees,
            guard: Vec::new(),
        }
    }

    fn sc(&self, v: i64) -> i64 {
        sc(v, self.unit)
    }

    // ---- presence (§7.3) ----
    fn present(&self, r: &str) -> bool {
        if is_compat_ref(r) {
            return true;
        }
        let Some(d) = def_of(r) else { return false };
        if self.view.popouts.contains(r) {
            return false;
        }
        let v = &self.view;
        if !v.room_open && matches!(r, "header" | "composer" | "panel") {
            return false; // the call: callShown alone
        }
        match d.presence {
            Presence::Room => v.room_open,
            Presence::Call => v.call_shown,
            Presence::Open => v.panel_open,
            Presence::Content => !self.content.get(r).is_some_and(|c| c.empty),
            Presence::Always => true,
        }
    }
    fn prune(&self, n: &N, removed: &BTreeSet<String>) -> Option<N> {
        if removed.contains(&*n.key) {
            return None;
        }
        if n.kind == Kind::Island {
            return self.present(&n.rf).then(|| n.clone());
        }
        let kids: Vec<N> = n.kids.iter().filter_map(|k| self.prune(k, removed)).collect();
        if kids.is_empty() {
            return None;
        }
        Some(Rc::new(SN {
            kind: n.kind,
            key: n.key.clone(),
            rf: n.rf.clone(),
            orig: n.orig.clone(),
            unknown: n.unknown,
            kids,
            synthetic: n.synthetic,
        }))
    }

    // ---- sizes (§7.4) ----
    fn reg_min(&self, n: &SN, a: Axis) -> i64 {
        if n.kind != Kind::Island {
            return 0;
        }
        let c = self.content.get(&*n.rf);
        let cm = c.and_then(|c| if a == Axis::W { c.min_w } else { c.min_h });
        let rs = if is_compat_ref(&n.rf) { None } else { reg_sizes(Some(self.form), &n.rf) };
        let reg = rs.map_or(0, |rs| self.sc(rs.min[a.idx()]));
        reg.max(cm.unwrap_or(0))
    }
    fn size_of(&self, n: &SN, a: Axis) -> Sz {
        let mut s: Sz;
        if n.unknown {
            s = Sz { share: Some(1), ..Sz::default() };
        } else {
            let stored = Sz::of(n.orig.get("size").filter(J::is_obj).as_ref());
            if stored.has_basis() {
                s = stored;
            } else {
                let rs = if n.kind == Kind::Island && !is_compat_ref(&n.rf) { reg_sizes(Some(self.form), &n.rf) } else { None };
                s = match rs {
                    Some(rs) => rs.pref_sz[a.idx()],
                    None => Sz { share: Some(1), ..Sz::default() },
                };
                if stored.min.is_some() {
                    s.min = stored.min;
                }
                if stored.max.is_some() {
                    s.max = stored.max;
                }
                if stored.min_pm.is_some() {
                    s.min_pm = stored.min_pm;
                }
                if stored.max_pm.is_some() {
                    s.max_pm = stored.max_pm;
                }
            }
        }
        if !self.emergency_mode {
            if let Some(ov) = self.view.sizes.get(&*n.key) {
                if ov.has_basis() {
                    s.fixed = ov.fixed;
                    s.share = ov.share;
                    s.auto = ov.auto;
                    s.pm = ov.pm;
                    s.grow = ov.grow;
                }
                if ov.min.is_some() {
                    s.min = ov.min;
                }
                if ov.max.is_some() {
                    s.max = ov.max;
                }
                if ov.min_pm.is_some() {
                    s.min_pm = ov.min_pm;
                }
                if ov.max_pm.is_some() {
                    s.max_pm = ov.max_pm;
                }
            }
        }
        s
    }
    fn floor_len(&self, n: &SN, a: Axis) -> i64 {
        match n.kind {
            Kind::Island => {
                if def_of(&n.rf).is_some_and(|d| d.required) {
                    self.reg_min(n, a)
                } else {
                    0
                }
            }
            Kind::Stack => 0,
            _ => {
                let fl = n.kids.iter().map(|c| self.floor_len(c, a));
                if a == axis_of(n) {
                    fl.sum()
                } else {
                    fl.max().unwrap_or(0)
                }
            }
        }
    }
    fn tab_px(&self) -> i64 {
        self.sc(self.shape[TAB])
    }
    fn cgap(&self, n: &SN) -> i64 {
        match n.orig.get("gap") {
            Some(g) if !n.unknown && g.is_int() => self.sc(g.int_val().unwrap_or(0).clamp(-(1 << 40), 1 << 40)),
            _ => self.gap,
        }
    }
    fn natural(&self, n: &SN, a: Axis) -> i64 {
        match n.kind {
            Kind::Island => {
                if self.folded.contains(&*n.rf) {
                    return self.reg_min(n, a);
                }
                if let Some(c) = self.content.get(&*n.rf) {
                    if let Some(v) = if a == Axis::W { c.w } else { c.h } {
                        return v;
                    }
                }
                if is_compat_ref(&n.rf) {
                    return 0;
                }
                let pref = reg_sizes(Some(self.form), &n.rf).map(|rs| rs.pref_sz[a.idx()]).unwrap_or_default();
                if let Some(f) = pref.fixed {
                    return self.sc(f);
                }
                self.reg_min(n, a)
            }
            Kind::Stack => {
                let m = n.kids.iter().map(|c| self.natural(c, a)).max().unwrap_or(-INF);
                m + if a == Axis::H { self.tab_px() } else { 0 }
            }
            _ => {
                if a == axis_of(n) {
                    let s: i64 = n.kids.iter().map(|c| self.base_len(c, a, None)).sum();
                    s + self.cgap(n) * (n.kids.len() as i64 - 1)
                } else {
                    n.kids.iter().map(|c| self.natural(c, a)).max().unwrap_or(-INF)
                }
            }
        }
    }
    fn min_len(&self, n: &SN, a: Axis, l: Option<i64>, along: bool) -> i64 {
        let mut m = self.reg_min(n, a);
        if along {
            let s = self.size_of(n, a);
            m = m.max(self.sc(s.min.unwrap_or(0)));
            if let (Some(l), Some(mp)) = (l, s.min_pm) {
                m = m.max(fdiv(mp * l, 1000));
            }
        }
        if is_row_col(n) {
            let inner = if a == axis_of(n) {
                let s: i64 = n.kids.iter().map(|c| self.min_len(c, a, None, true)).sum();
                s + self.cgap(n) * (n.kids.len() as i64 - 1)
            } else {
                n.kids.iter().map(|c| self.min_len(c, a, None, false)).max().unwrap_or(-INF)
            };
            m = m.max(inner);
        } else if n.kind == Kind::Stack {
            let k = n.kids.iter().map(|c| self.min_len(c, a, None, false)).max().unwrap_or(-INF);
            m = m.max(k + if a == Axis::H { self.tab_px() } else { 0 });
        }
        m
    }
    fn max_len(&self, n: &SN, a: Axis, l: Option<i64>) -> i64 {
        let s = self.size_of(n, a);
        let mut mx = INF;
        if let Some(x) = s.max {
            mx = mx.min(self.sc(x));
        }
        if let (Some(mp), Some(l)) = (s.max_pm, l) {
            mx = mx.min(fdiv(mp * l, 1000));
        }
        mx
    }
    fn base_len(&self, n: &SN, a: Axis, l: Option<i64>) -> i64 {
        let s = self.size_of(n, a);
        let mn = self.min_len(n, a, l, true);
        let mx = mn.max(self.max_len(n, a, l));
        let b = if let Some(f) = s.fixed {
            self.sc(f)
        } else if s.auto {
            self.natural(n, a)
        } else if let (Some(pm), Some(l)) = (s.pm, l) {
            fdiv(pm * l, 1000)
        } else {
            mn
        };
        clamp(b, mn, mx)
    }
    fn weight(&self, n: &SN, a: Axis) -> i64 {
        let s = self.size_of(n, a);
        if s.fixed.is_some() || s.auto || s.pm.is_some() {
            return s.grow.unwrap_or(0);
        }
        s.share.unwrap_or(1)
    }

    // ---- docked distribution (§7.6) ----
    fn distribute(&self, n: &SN, kids: &[N], length: i64, a: Axis, g_fixed: Option<i64>) -> Dist {
        let k = kids.len() as i64;
        let g = match g_fixed {
            Some(g) => g,
            None => {
                let mut g = self.cgap(n);
                if k > 1 && g * (k - 1) > fdiv(length, 4) {
                    g = fdiv(fdiv(length, 4), k - 1);
                }
                g
            }
        };
        let l = (length - g * (k - 1).max(0)).max(0);
        let mut base: Vec<i64> = kids.iter().map(|c| self.base_len(c, a, Some(l))).collect();
        let mins: Vec<i64> = kids.iter().map(|c| self.min_len(c, a, Some(l), true)).collect();
        let maxs: Vec<i64> = kids.iter().enumerate().map(|(i, c)| mins[i].max(self.max_len(c, a, Some(l)))).collect();
        let floors: Vec<i64> = kids.iter().map(|c| self.floor_len(c, a)).collect();
        let wts: Vec<i64> = kids.iter().map(|c| self.weight(c, a)).collect();
        let mut r = l - base.iter().sum::<i64>();
        let mut over = false;
        let mut breach = false;
        let mut squeezed: Vec<usize> = Vec::new();
        let ku = kids.len();
        if r > 0 && wts.iter().any(|&w| w > 0) {
            let mut capped = vec![false; ku];
            while r > 0 {
                let act: Vec<usize> = (0..ku).filter(|&i| wts[i] > 0 && !capped[i]).collect();
                let s: i64 = act.iter().map(|&i| wts[i]).sum();
                if s == 0 {
                    break;
                }
                let mut gave = 0;
                let mut newcap = false;
                let mut rems: Vec<(i64, usize)> = Vec::new();
                for &i in &act {
                    let mut add = fdiv(r * wts[i], s);
                    let rem = r * wts[i] - add * s;
                    let room = maxs[i] - base[i];
                    if add >= room {
                        add = room;
                        capped[i] = true;
                        newcap = true;
                    }
                    base[i] += add;
                    gave += add;
                    rems.push((rem, i));
                }
                r -= gave;
                if !newcap {
                    rems.sort_by(|p, q| q.0.cmp(&p.0).then(p.1.cmp(&q.1)));
                    for &(_, i) in &rems {
                        if r <= 0 {
                            break;
                        }
                        if base[i] >= maxs[i] {
                            continue;
                        }
                        base[i] += 1;
                        r -= 1;
                    }
                    break;
                }
            }
        } else if r < 0 {
            over = true;
            let mut need = -r;
            for phase in [1, 2, 3] {
                for i in (0..ku).rev() {
                    if need <= 0 {
                        break;
                    }
                    let s = self.size_of(&kids[i], a);
                    if phase == 1 && !(s.auto || s.pm.is_some()) {
                        continue;
                    }
                    if phase == 2 && s.fixed.is_none() {
                        continue;
                    }
                    if phase == 3 && floors[i] == 0 {
                        continue;
                    }
                    let lo = if phase == 3 { floors[i] } else { mins[i] };
                    let take = need.min((base[i] - lo).max(0));
                    base[i] -= take;
                    need -= take;
                }
            }
            for i in (0..ku).rev() {
                if need <= 0 {
                    break;
                }
                if floors[i] == 0 && base[i] > 0 {
                    need -= base[i] + if k > 1 { g } else { 0 };
                    base[i] = 0;
                    squeezed.push(i);
                }
            }
            if need > 0 {
                breach = true;
            }
        }
        Dist { base, mins, maxs, g, l, over, breach, squeezed }
    }

    fn islands_in(n: &SN) -> Vec<String> {
        if n.kind == Kind::Island {
            return vec![n.rf.to_string()];
        }
        n.kids.iter().flat_map(|c| Solver::islands_in(c)).collect()
    }

    fn solve_node(&self, n: &N, r: Rect, z: i64, rec: &mut Rec, docked: bool) {
        rec.rects.insert(n.key.to_string(), r);
        if n.kind == Kind::Island {
            rec.islands.insert(n.rf.to_string(), IslandOut { x: r.x, y: r.y, w: r.w, h: r.h, z, vis: r.w > 0 && r.h > 0, inset: None });
            return;
        }
        if n.kind == Kind::Stack {
            let t = self.tab_px().min(r.h);
            let refs: Vec<Rc<str>> = n.kids.iter().map(|c| c.rf.clone()).collect();
            let has = |x: &str| refs.iter().any(|r| &**r == x);
            let mut act: Option<Rc<str>> = self.view.active.get(&*n.key).map(|s| Rc::from(s.as_str()));
            if !act.as_ref().is_some_and(|a| has(a)) {
                act = n.orig.get_str("active");
            }
            if !act.as_ref().is_some_and(|a| has(a)) {
                act = refs.first().cloned();
            }
            let act = act.unwrap_or_else(|| Rc::from(""));
            rec.stacks.insert(
                n.key.to_string(),
                StackOut { x: r.x, y: r.y, w: r.w, h: r.h, z, vis: r.w > 0 && r.h > 0, tabs: refs.clone(), active: act.clone(), tab: t },
            );
            for c in &n.kids {
                self.solve_node(c, Rect { x: r.x, y: r.y + t, w: r.w, h: r.h - t }, z, rec, false);
                if let Some(o) = rec.islands.get_mut(&*c.rf) {
                    o.vis = o.vis && *c.rf == *act;
                }
            }
            return;
        }
        if !n.synthetic {
            rec.containers.insert(n.key.to_string(), ContainerOut { r, z });
        }
        let a = axis_of(n);
        let ca = a.cross();
        let length = r.len(a);
        let clen = r.len(ca);
        let mut kidx: Vec<usize> = (0..n.kids.len()).collect();
        let kids_of_idx = |ix: &[usize]| -> Vec<N> { ix.iter().map(|&i| n.kids[i].clone()).collect() };
        let first = self.distribute(n, &n.kids, length, a, None);
        let first_g = first.g;
        let first_l = first.l;
        let mut over = first.over;
        let mut breach = first.breach;
        let mut cur = first;
        let mut squeezed_idx: Vec<usize> = Vec::new();
        let chip = self.sc(self.shape[HANDLE]).min(length);
        while !cur.squeezed.is_empty() {
            let gone: BTreeSet<usize> = cur.squeezed.iter().map(|&i| kidx[i]).collect();
            squeezed_idx.extend(kidx.iter().copied().filter(|i| gone.contains(i)));
            kidx.retain(|i| !gone.contains(i));
            let ks = kids_of_idx(&kidx);
            cur = self.distribute(n, &ks, (length - (chip + first_g)).max(0), a, Some(first_g));
            over = over || cur.over;
            breach = breach || cur.breach;
        }
        rec.inner.insert(n.key.to_string(), first_l);
        if !squeezed_idx.is_empty() {
            let cr = if a == Axis::W {
                Rect { x: r.x + r.w - chip, y: r.y, w: chip, h: r.h }
            } else {
                Rect { x: r.x, y: r.y + r.h - chip, w: r.w, h: chip }
            };
            let mut order = squeezed_idx.clone();
            order.sort();
            let refs: Vec<String> = order.iter().flat_map(|&i| Solver::islands_in(&n.kids[i])).collect();
            for rf in &refs {
                rec.islands.insert(rf.clone(), IslandOut { x: cr.x, y: cr.y, w: 0, h: 0, z, vis: false, inset: None });
                rec.squeezed.push(rf.clone());
            }
            rec.chips.push(Chip { key: n.key.to_string(), axis: if a == Axis::W { "x" } else { "y" }, rect: cr, refs, z, layer_open: true });
        }
        if over {
            rec.overflow.push(n.key.to_string());
        }
        if breach {
            rec.breach.insert(n.key.to_string());
        }
        let kids = kids_of_idx(&kidx);
        for c in &kids {
            if self.min_len(c, ca, None, false) > clen {
                if !rec.overflow.iter().any(|o| **o == *n.key) {
                    rec.overflow.push(n.key.to_string());
                }
                if self.floor_len(c, ca) > clen {
                    rec.breach.insert(n.key.to_string());
                }
            }
        }
        if docked {
            let d = DistRec {
                key: n.key.clone(),
                axis: a,
                inner: cur.l,
                kids: kids.iter().enumerate().map(|(i, c)| DistKid { key: c.key.clone(), len: cur.base[i], min: cur.mins[i], max: cur.maxs[i] }).collect(),
            };
            match rec.dist.iter_mut().find(|x| x.key == d.key) {
                Some(slot) => *slot = d,
                None => rec.dist.push(d),
            }
        }
        let mut pos = if a == Axis::W { r.x } else { r.y };
        let g = cur.g;
        let mut prev: Option<Rc<str>> = None;
        for (i, c) in kids.iter().enumerate() {
            let ln = cur.base[i];
            let cr = if a == Axis::W { Rect { x: pos, y: r.y, w: ln, h: r.h } } else { Rect { x: r.x, y: pos, w: r.w, h: ln } };
            if let Some(p) = &prev {
                if z == 0 {
                    let gr = if a == Axis::W { Rect { x: pos - g, y: r.y, w: g, h: r.h } } else { Rect { x: r.x, y: pos - g, w: r.w, h: g } };
                    rec.gaps.push(GapOut {
                        id: format!("{}/{}", n.key, i - 1),
                        axis: if a == Axis::W { "x" } else { "y" },
                        r: gr,
                        a: p.to_string(),
                        b: c.key.to_string(),
                    });
                }
            }
            self.solve_node(c, cr, z, rec, docked);
            prev = Some(c.key.clone());
            pos += ln + g;
        }
    }

    // ---- trials, collapse (§7.5) ----
    fn find(n: Option<&N>, key: &str) -> Option<N> {
        let n = n?;
        if &*n.key == key {
            return Some(n.clone());
        }
        n.kids.iter().find_map(|c| Solver::find(Some(c), key))
    }
    fn parent_of(n: &N, key: &str) -> Option<N> {
        for c in &n.kids {
            if &*c.key == key {
                return Some(n.clone());
            }
            if let Some(p) = Solver::parent_of(c, key) {
                return Some(p);
            }
        }
        None
    }
    fn descendants(n: &N, out: &mut Vec<N>) {
        out.push(n.clone());
        for c in &n.kids {
            Solver::descendants(c, out);
        }
    }
    fn trial(&self, removed: &BTreeSet<String>, full: &N) -> Rec {
        let tree = self.prune(full, removed);
        let mut rec = Rec::default();
        if let Some(t) = &tree {
            self.solve_node(t, self.host, 0, &mut rec, true);
        }
        rec.tree = tree;
        rec
    }
    fn below_floor(&self, rec: &Rec, under: Option<&BTreeSet<String>>) -> bool {
        for (r, o) in &rec.islands {
            if !def_of(r).is_some_and(|d| d.required) || o.z != 0 {
                continue;
            }
            if under.is_some_and(|u| !u.contains(r)) {
                continue;
            }
            let node = leaf(r);
            if o.w < self.reg_min(&node, Axis::W) || o.h < self.reg_min(&node, Axis::H) {
                return true;
            }
        }
        false
    }
    fn breached(&self, rec: &Rec) -> bool {
        !rec.breach.is_empty() || self.below_floor(rec, None)
    }
    /// A step's subtree in the form holds a room-group island or the call (§3.5 safety).
    fn unsafe_step(&self, key: &str) -> bool {
        match Solver::find(self.root.as_ref(), key) {
            Some(n) => Solver::islands_in(&n).iter().any(|r| is_main(r)),
            None => false,
        }
    }
    fn can_fold(key: &str, node: &SN) -> bool {
        node.kind == Kind::Island && def_of(key).is_some_and(|d| d.collapse_allows("fold"))
    }
    fn condition(&self, st: &Step, rec: &Rec) -> bool {
        if st.forced {
            return true;
        }
        let Some(tree) = &rec.tree else { return false };
        let par = Solver::parent_of(tree, &st.rf);
        let p = par.clone().unwrap_or_else(|| tree.clone());
        let prect = match &par {
            Some(par) => rec.rects.get(&*par.key).copied(),
            None => Some(self.host),
        };
        // a parent squeezed out of its own container is not laid out: length 0, and it counts as overflowed
        let Some(prect) = prect else { return true };
        let plen = if p.kind == Kind::Row { prect.w } else { prect.h };
        if let Some(b) = st.below {
            return plen < self.sc(b);
        }
        let mut sub = Vec::new();
        Solver::descendants(&p, &mut sub);
        if sub.iter().any(|d| rec.overflow.iter().any(|o| **o == *d.key)) {
            return true;
        }
        let under: BTreeSet<String> = Solver::islands_in(&p).into_iter().collect();
        self.below_floor(rec, Some(&under))
    }
    /// A step takes part in a trial when its node is in the working tree, it is safe, and a fold can fold.
    fn eligible(&self, st: &Step, rec: &Rec) -> bool {
        let Some(node) = Solver::find(rec.tree.as_ref(), &st.rf) else { return false };
        if self.unsafe_step(&st.rf) {
            return false;
        }
        if st.to == "fold" && !Solver::can_fold(&st.rf, &node) {
            return false;
        }
        true
    }
    fn effects(steps: &[Step], applied: &[usize]) -> BTreeSet<String> {
        applied.iter().filter(|&&i| steps[i].to != "fold").map(|&i| steps[i].rf.clone()).collect()
    }

    fn run(mut self) -> SolveDetail {
        let mut out = SolveOut::default();
        let mut res_dist: Vec<DistRec> = Vec::new();
        let mut res_rects: HashMap<String, Rect> = HashMap::new();
        let host = self.host;
        if let Some(mx) = self.view.maximized.clone() {
            if self.placed_present(&mx) {
                let h = self.host;
                out.islands.insert(mx, IslandOut { x: h.x, y: h.y, w: h.w, h: h.h, z: 0, vis: h.w > 0 && h.h > 0, inset: None });
                return SolveDetail { out, dist: res_dist, rects: res_rects, host };
            }
        }
        let full = self.root.as_ref().and_then(|r| self.prune(r, &BTreeSet::new()));
        let Some(full) = full else {
            let mut chips = Vec::new();
            self.set_guard(None);
            self.place_floats(&mut out, &mut chips);
            self.place_chips(&chips, &mut out);
            return SolveDetail { out, dist: res_dist, rects: res_rects, host };
        };
        // steps: forced (view.forceCollapse) first, then the form's
        let mut form_steps: Vec<Step> = Vec::new();
        if let Some(col) = self.form.get("collapse").filter(J::is_arr) {
            for s in col.items() {
                form_steps.push(Step {
                    rf: js_str(s.get("ref").as_ref()),
                    to: js_str(s.get("to").as_ref()),
                    below: s.get("below").filter(|b| b.is_int()).and_then(|b| b.int_val()).map(|b| b.clamp(-(1 << 40), 1 << 40)),
                    w: s.get("w").filter(|b| b.is_int()).and_then(|b| b.int_val()).map(|b| b.clamp(-(1 << 40), 1 << 40)),
                    forced: false,
                });
            }
        }
        let mut steps: Vec<Step> = Vec::new();
        for r in &self.view.force_collapse {
            let node = Solver::find(Some(&full), r);
            let fs = form_steps.iter().find(|s| s.rf == *r);
            let to = match fs {
                Some(fs) => fs.to.clone(),
                None => {
                    if node.is_some_and(|n| n.kind != Kind::Island) {
                        "drawer".into()
                    } else {
                        "hide".into()
                    }
                }
            };
            steps.push(Step { rf: r.clone(), to, below: fs.and_then(|f| f.below), w: fs.and_then(|f| f.w), forced: true });
        }
        for s in &form_steps {
            if !self.view.force_collapse.contains(&s.rf) {
                steps.push(s.clone());
            }
        }
        let mut applied: Vec<usize> = Vec::new();
        for (si, st) in steps.iter().enumerate() {
            let rec = self.trial(&Solver::effects(&steps, &applied), &full);
            if !self.eligible(st, &rec) {
                continue;
            }
            if self.condition(st, &rec) {
                applied.push(si);
                if st.to == "fold" {
                    self.folded.insert(st.rf.clone());
                }
            }
        }
        // fixpoint (Q-FIXPOINT): a later step can shrink an earlier step's parent (a share container follows its
        // content's minimum), so apply the first not-applied eligible step whose condition now holds, until none does
        loop {
            let rec = self.trial(&Solver::effects(&steps, &applied), &full);
            let next = (0..steps.len()).find(|&si| !applied.contains(&si) && self.eligible(&steps[si], &rec) && self.condition(&steps[si], &rec));
            let Some(si) = next else { break };
            applied.push(si);
            if steps[si].to == "fold" {
                self.folded.insert(steps[si].rf.clone());
            }
        }
        let snapshot = applied.clone();
        for &si in snapshot.iter().rev() {
            let st = &steps[si];
            if st.forced {
                continue;
            }
            let rest: Vec<usize> = applied.iter().copied().filter(|&x| x != si).collect();
            let saved = self.folded.clone();
            if st.to == "fold" {
                self.folded.remove(&st.rf);
            }
            let rec = self.trial(&Solver::effects(&steps, &rest), &full);
            let mut ok = !self.breached(&rec);
            if ok {
                for (oi, other) in steps.iter().enumerate() {
                    if rest.contains(&oi) {
                        continue;
                    }
                    if !self.eligible(other, &rec) {
                        continue;
                    }
                    if self.condition(other, &rec) {
                        ok = false;
                        break;
                    }
                }
            }
            if ok {
                applied = rest;
            } else {
                self.folded = saved;
            }
        }
        out.collapsed = applied.iter().map(|&i| format!("{}:{}", steps[i].rf, steps[i].to)).collect();
        let eff = Solver::effects(&steps, &applied);
        let fin = self.trial(&eff, &full);
        self.set_guard(Some(&fin));
        if self.breached(&fin) || self.float_call_breach() {
            self.emergency(&full, &mut out);
            return SolveDetail { out, dist: res_dist, rects: res_rects, host };
        }
        for (k, v) in &fin.islands {
            out.islands.insert(k.clone(), v.clone());
        }
        for (k, v) in &fin.stacks {
            out.stacks.insert(k.clone(), v.clone());
        }
        for (k, v) in &fin.containers {
            out.containers.insert(k.clone(), v.clone());
        }
        out.gaps.extend(fin.gaps.iter().cloned());
        out.overflow.extend(fin.overflow.iter().cloned());
        out.squeezed.extend(fin.squeezed.iter().cloned());
        let mut chips: Vec<Chip> = fin.chips.clone();
        res_dist = fin.dist.clone();
        res_rects = fin.rects.clone();
        for &si in &applied {
            if steps[si].to == "overlay" {
                self.place_overlay(&steps[si], &full, &fin, &eff, &mut out, &mut chips);
            }
        }
        self.place_floats(&mut out, &mut chips);
        let drawers: Vec<DrawerItem> = applied
            .iter()
            .filter(|&&si| steps[si].to == "drawer")
            .filter_map(|&si| {
                let s = &steps[si];
                self.prune_for(&full, &s.rf, &eff).map(|node| DrawerItem { rf: s.rf.clone(), w: s.w, node, merged: None })
            })
            .collect();
        self.place_drawers(drawers, &full, &mut out, &mut chips, None);
        self.place_chips(&chips, &mut out);
        SolveDetail { out, dist: res_dist, rects: res_rects, host }
    }

    /// The step's node from the uncollapsed tree, without the nodes other applied steps removed.
    fn prune_for(&self, full: &N, key: &str, eff: &BTreeSet<String>) -> Option<N> {
        let node = Solver::find(Some(full), key)?;
        let mut others = eff.clone();
        others.remove(key);
        self.prune(&node, &others)
    }

    fn placed_present(&self, r: &str) -> bool {
        if !self.present(r) {
            return false;
        }
        fn in_tree(n: &N, r: &str) -> bool {
            (n.kind == Kind::Island && &*n.rf == r) || n.kids.iter().any(|c| in_tree(c, r))
        }
        self.root.as_ref().is_some_and(|n| in_tree(n, r)) || self.float_trees.iter().flatten().any(|n| in_tree(n, r))
    }

    // ---- overlays (§7.8) ----
    fn place_overlay(&self, st: &Step, full: &N, fin: &Rec, eff: &BTreeSet<String>, out: &mut SolveOut, chips: &mut Vec<Chip>) {
        let par = Solver::parent_of(full, &st.rf);
        let node = self.prune_for(full, &st.rf, eff);
        let (Some(par), Some(node)) = (par, node) else { return };
        let a = if par.kind == Kind::Row { Axis::W } else { Axis::H };
        let ca = a.cross();
        let prect = fin.rects.get(&*par.key).copied().unwrap_or(self.host);
        let plen = prect.len(a);
        let inner = fin.inner.get(&*par.key).copied().unwrap_or(plen);
        let ln = self.base_len(&node, a, Some(inner)).min(plen);
        let fin_par = Solver::find(fin.tree.as_ref(), &par.key);
        let remaining: BTreeSet<String> = fin_par.map(|p| p.kids.iter().map(|c| c.key.to_string()).collect()).unwrap_or_default();
        let idx = par.kids.iter().position(|c| *c.key == *st.rf).map_or(0, |i| i + 1);
        let last = par.kids[idx.min(par.kids.len())..].iter().all(|c| !remaining.contains(&*c.key));
        let r = if a == Axis::W {
            Rect { x: if last { prect.x + prect.w - ln } else { prect.x }, y: prect.y, w: ln, h: prect.len(ca) }
        } else {
            Rect { x: prect.x, y: if last { prect.y + prect.h - ln } else { prect.y }, w: prect.len(ca), h: ln }
        };
        let mut rr = r;
        for (q, _) in &self.guard {
            rr = cut_around(rr, *q, self.gap); // never over the composer or the call
        }
        let mut rec = Rec::default();
        self.solve_node(&node, rr, 2, &mut rec, false);
        for (k, v) in rec.islands {
            out.islands.insert(k, v);
        }
        for (k, v) in rec.stacks {
            out.stacks.insert(k, v);
        }
        for (k, v) in rec.containers {
            out.containers.insert(k, v);
        }
        chips.extend(rec.chips);
        out.squeezed.extend(rec.squeezed);
    }

    // ---- drawers (§7.9) ----
    fn hx(&self) -> i64 {
        self.margin.max(4).min(fdiv(self.ww, 8)).min(fdiv(self.hh, 8))
    }
    /// the handle size: at most half of what is between the two handle margins, so the sides never meet (§7.9)
    fn hs(&self) -> i64 {
        let hx = self.hx();
        0.max(self.sc(self.shape[HANDLE]).min(fdiv(self.ww - 2 * hx - 4, 2)).min(self.hh - 2 * hx))
    }
    fn place_drawers(&mut self, items: Vec<DrawerItem>, full: &N, out: &mut SolveOut, chips: &mut Vec<Chip>, side_rect: Option<Rect>) {
        if items.is_empty() {
            return;
        }
        let hx = self.hx();
        let hs = self.hs();
        let pad = self.sc(self.shape[DRAWER_PAD]);
        // the side of a drawer: its node's rect in the uncollapsed trial (every present node, no steps, no folds)
        let side_rec = if side_rect.is_none() {
            let saved = std::mem::take(&mut self.folded);
            let r = self.trial(&BTreeSet::new(), full);
            self.folded = saved;
            Some(r)
        } else {
            None
        };
        let kmax = 1.max(fdiv(self.ww - 2 * hx + 4, 2 * (hs + 4)));
        let mut groups: [Vec<DrawerItem>; 2] = [Vec::new(), Vec::new()];
        for d in items {
            let nr = match side_rect {
                Some(r) => r,
                None => side_rec.as_ref().and_then(|s| s.rects.get(&d.rf).copied()).unwrap_or(self.host),
            };
            let right = 2 * nr.x + nr.w > 2 * self.host.x + self.host.w;
            groups[usize::from(right)].push(d);
        }
        let open = self.view.open.clone();
        for (gi, side) in ["l", "r"].iter().enumerate() {
            let mut g = std::mem::take(&mut groups[gi]);
            if g.len() as i64 > kmax {
                let tail = g.split_off((kmax - 1) as usize);
                let first = tail[0].rf.clone();
                let merged: Vec<String> = tail.iter().map(|t| t.rf.clone()).collect();
                let node = synthetic_col(format!("~m.{first}"), tail.into_iter().map(|t| t.node).collect());
                g.push(DrawerItem { rf: first, w: None, node, merged: Some(merged) });
            }
            for (k, d) in g.iter().enumerate() {
                let hxk = hx + k as i64 * (hs + 4);
                let handle = Rect { x: if *side == "l" { hxk } else { self.ww - hxk - hs }, y: hx, w: hs, h: hs };
                let mut pw = match d.w {
                    // a stored drawer width counts, but never below the usable minimum (x-tinhat.bounds.shapeMin.drawerW)
                    Some(w) => self.sc(w.max(schema().shape_min[DRAWER_W])),
                    None => {
                        let nat = self.natural(&d.node, Axis::W);
                        if nat > 0 {
                            self.sc(self.shape[DRAWER_W]).min(nat + 2 * pad)
                        } else {
                            self.sc(self.shape[DRAWER_W])
                        }
                    }
                };
                pw = pw.min((self.ww - 2 * hx).max(0));
                let py = (hx + hs + 4).min((self.hh - hx).max(0));
                let mut ph = 200.max(self.sc(680).min(fdiv(self.hh * 820, 1000)).min(self.hh - 94));
                ph = 0.max(ph.min(self.hh - hx - py));
                let px = if *side == "l" { hx } else { self.ww - hx - pw };
                let pd = pad.min(fdiv(pw, 4)).min(fdiv(ph, 4));
                let iw = (pw - 2 * pd).max(0);
                let ih = (ph - 2 * pd).max(0);
                let ch = ih.max(self.min_len(&d.node, Axis::H, None, false));
                let mut rec = Rec::default();
                self.solve_node(&d.node, Rect { x: px + pd, y: py + pd, w: iw, h: ch }, 20, &mut rec, false);
                let is_open = match &open {
                    Some(o) => match &d.merged {
                        Some(m) => m.contains(o),
                        None => *o == d.rf,
                    },
                    None => false,
                };
                Solver::merge_layer(rec, out, chips, is_open);
                out.drawers.push(DrawerOut {
                    id: d.rf.clone(),
                    side: side.to_string(),
                    handle,
                    panel: Rect { x: px, y: py, w: pw, h: ph },
                    open: is_open,
                    scroll: (ch > ih).then_some(ch),
                    merged: d.merged.clone(),
                    count: None,
                    vis: None,
                    z: None,
                });
                for r in out.islands.values_mut() {
                    if r.z != 0 || r.w <= 0 || r.h <= 0 {
                        continue;
                    }
                    if r.x < handle.x + hs && handle.x < r.x + r.w && r.y < handle.y + hs && handle.y < r.y + r.h {
                        let (mut l, mut rr) = r.inset.unwrap_or((None, None));
                        if *side == "l" {
                            l = Some(l.unwrap_or(0).max(handle.x + hs + 2 - r.x));
                        } else {
                            rr = Some(rr.unwrap_or(0).max(r.x + r.w - handle.x + 2));
                        }
                        r.inset = Some((l, rr));
                    }
                }
            }
        }
    }
    /// Copy a layer's islands / stacks / containers into the output; `open` = the layer is shown.
    fn merge_layer(rec: Rec, out: &mut SolveOut, chips: &mut Vec<Chip>, open: bool) {
        for (k, mut r) in rec.islands {
            r.vis = r.vis && open;
            out.islands.insert(k, r);
        }
        for (k, mut s) in rec.stacks {
            s.vis = s.vis && open;
            out.stacks.insert(k, s);
        }
        for (k, c) in rec.containers {
            out.containers.insert(k, c);
        }
        for mut c in rec.chips {
            c.layer_open = c.layer_open && open;
            chips.push(c);
        }
        out.squeezed.extend(rec.squeezed);
    }

    // ---- overflow chips (§7.6, §7.9) ----
    fn place_chips(&self, chips: &[Chip], out: &mut SolveOut) {
        let hx = self.hx();
        let pad = self.sc(self.shape[DRAWER_PAD]);
        let open = self.view.open.clone();
        for c in chips {
            let Rect { x: cx, y: cy, w: cw, h: chh } = c.rect;
            // the panel is a plain scrolling list (§7.9): each island at its natural height, at least its registry
            // minimum, the form gap between them — no size constraints, no view overrides, no distribution, so nothing can
            // be squeezed a second time
            let kids: Vec<N> = c.refs.iter().map(|r| leaf(r)).collect();
            let hts: Vec<i64> = kids.iter().map(|k| self.natural(k, Axis::H).max(self.reg_min(k, Axis::H))).collect();
            let list_h = hts.iter().sum::<i64>() + self.gap * (kids.len() as i64 - 1).max(0);
            let widest = kids.iter().map(|k| self.natural(k, Axis::W)).max().unwrap_or(-INF);
            let mut pw = self.sc(self.shape[DRAWER_W]).min(widest + 2 * pad).min((self.ww - 2 * hx).max(0));
            let want = list_h + 2 * pad;
            let room = |s: &str| -> i64 {
                match s {
                    "below" => self.hh - hx - (cy + chh + 4),
                    "above" => cy - 4 - hx,
                    "right" => self.ww - hx - (cx + cw + 4),
                    _ => cx - 4 - hx,
                }
            };
            let prefs: [&str; 4] = if c.axis == "x" { ["below", "above", "right", "left"] } else { ["right", "left", "below", "above"] };
            let need = |s: &str| -> i64 {
                if s == "below" || s == "above" {
                    want.min(200)
                } else {
                    pw
                }
            };
            let side = match prefs.iter().find(|&&k| room(k) >= need(k)) {
                Some(s) => *s,
                None => {
                    let mut side = prefs[0];
                    for k in prefs {
                        if room(k) > room(side) {
                            side = k;
                        }
                    }
                    side
                }
            };
            let (mut px, mut py, mut ph);
            if side == "below" || side == "above" {
                ph = 0.max(want.min(room(side)));
                py = if side == "below" { cy + chh + 4 } else { cy - 4 - ph };
                px = cx + cw - pw;
            } else {
                pw = 0.max(pw.min(room(side)));
                ph = 0.max(want.min(self.hh - 2 * hx));
                px = if side == "right" { cx + cw + 4 } else { cx - 4 - pw };
                py = cy + chh - ph;
            }
            px = clamp(px, hx, hx.max(self.ww - hx - pw));
            py = clamp(py, hx, hx.max(self.hh - hx - ph));
            pw = 0.max(pw.min(self.ww - hx - px));
            ph = 0.max(ph.min(self.hh - hx - py));
            let pd = pad.min(fdiv(pw, 4)).min(fdiv(ph, 4));
            let iw = (pw - 2 * pd).max(0);
            let ih = (ph - 2 * pd).max(0);
            let chn = ih.max(list_h);
            let id = format!("overflow-{}", c.key);
            let is_open = open.as_deref() == Some(id.as_str());
            let mut y = py + pd;
            for (i, k) in kids.iter().enumerate() {
                out.islands.insert(k.rf.to_string(), IslandOut { x: px + pd, y, w: iw, h: hts[i], z: 20, vis: iw > 0 && hts[i] > 0 && is_open, inset: None });
                y += hts[i] + self.gap;
            }
            out.drawers.push(DrawerOut {
                id,
                side: "chip".into(),
                handle: c.rect,
                panel: Rect { x: px, y: py, w: pw, h: ph },
                open: is_open,
                scroll: (chn > ih).then_some(chn),
                merged: None,
                count: Some(c.refs.len() as i64),
                vis: Some(c.layer_open),
                z: Some(c.z),
            });
        }
    }

    // ---- floats (§7.10) ----
    fn float_len(&self, spec: &Sz, node: &SN, a: Axis, host_len: i64, hover: bool) -> i64 {
        let mut v = if let Some(f) = spec.fixed {
            self.sc(f)
        } else if let Some(pm) = spec.pm {
            fdiv(pm * host_len, 1000)
        } else {
            self.natural(node, a)
        };
        let lo = self.sc(spec.min.unwrap_or(0)).max(fdiv(spec.min_pm.unwrap_or(0) * host_len, 1000));
        let mut hi = INF;
        if let Some(mx) = spec.max {
            hi = hi.min(self.sc(mx));
        }
        if let Some(mp) = spec.max_pm {
            hi = hi.min(fdiv(mp * host_len, 1000));
        }
        v = clamp(v, lo, lo.max(hi));
        if hover {
            v = v.max(self.hs().min(host_len));
        }
        clamp(v, 0, host_len)
    }
    fn float_geom(&self, f: &J, node: &SN) -> Rect {
        let h = self.host;
        let d = if node.kind == Kind::Island { def_of(&node.rf) } else { None };
        let spec = |k: &str, i: usize| -> Sz {
            if let Some(v) = f.get(k).filter(J::is_obj) {
                return Sz::of(Some(&v));
            }
            if let Some(fs) = d.and_then(|d| d.float.as_ref()) {
                return Sz::of(Some(&J::from_value(&fs[i])));
            }
            Sz { auto: true, ..Sz::default() }
        };
        let hover = f.is("reveal", "hover");
        let fw = self.float_len(&spec("w", 0), node, Axis::W, h.w, hover);
        let fh = self.float_len(&spec("h", 1), node, Axis::H, h.h, hover);
        let off = |k: &str| f.get(k).filter(|v| v.is_int()).and_then(|v| v.int_val()).unwrap_or(0).clamp(-(1 << 40), 1 << 40);
        let dx = self.sc(off("dx"));
        let dy = self.sc(off("dy"));
        let an = f.get_str("anchor").map(|s| s.to_string()).unwrap_or_default();
        let fx = if an.contains('l') {
            h.x + dx
        } else if an.contains('r') {
            h.x + h.w - fw - dx
        } else {
            h.x + fdiv(h.w - fw, 2) + dx
        };
        let fy = if an.starts_with('t') {
            h.y + dy
        } else if an.starts_with('b') {
            h.y + h.h - fh - dy
        } else {
            h.y + fdiv(h.h - fh, 2) + dy
        };
        let mut r = Rect { x: clamp(fx, h.x, h.x + h.w - fw), y: clamp(fy, h.y, h.y + h.h - fh), w: fw, h: fh };
        if hover {
            return r; // a hover float opens on demand, like a drawer
        }
        let own = Solver::islands_in(node).iter().any(|x| x == "call");
        for (q, is_call_float) in &self.guard {
            if !(own && *is_call_float) {
                r = cut_around(r, *q, self.gap);
            }
        }
        r
    }
    /// The protected controls (§7.8, §7.10): the visible docked composer and call, then a floating call's rect.
    fn set_guard(&mut self, fin: Option<&Rec>) {
        self.guard.clear();
        if let Some(fin) = fin {
            for r in ["composer", "call"] {
                if let Some(o) = fin.islands.get(r) {
                    if o.z == 0 && o.vis {
                        self.guard.push((Rect { x: o.x, y: o.y, w: o.w, h: o.h }, false));
                    }
                }
            }
        }
        let floats = self.floats.clone();
        for (i, f) in floats.iter().enumerate() {
            let node = self.float_trees.get(i).cloned().flatten().and_then(|t| self.prune(&t, &BTreeSet::new()));
            let Some(node) = node else { continue };
            if !Solver::islands_in(&node).iter().any(|r| r == "call") {
                continue;
            }
            let mut rec = Rec::default();
            let g = self.float_geom(f, &node);
            self.solve_node(&node, g, 10 + i as i64, &mut rec, false);
            if let Some(c) = rec.islands.get("call") {
                if c.vis {
                    self.guard.push((Rect { x: c.x, y: c.y, w: c.w, h: c.h }, true));
                }
            }
            break;
        }
    }
    fn float_call_breach(&self) -> bool {
        for (i, f) in self.floats.iter().enumerate() {
            let node = self.float_trees.get(i).cloned().flatten().and_then(|t| self.prune(&t, &BTreeSet::new()));
            let Some(node) = node else { continue };
            if !Solver::islands_in(&node).iter().any(|r| r == "call") {
                continue;
            }
            let mut rec = Rec::default();
            self.solve_node(&node, self.float_geom(f, &node), 10 + i as i64, &mut rec, false);
            let cn = leaf("call");
            match rec.islands.get("call") {
                None => return true,
                Some(c) => {
                    if c.w < self.reg_min(&cn, Axis::W) || c.h < self.reg_min(&cn, Axis::H) || !rec.breach.is_empty() {
                        return true;
                    }
                }
            }
        }
        false
    }
    fn place_floats(&self, out: &mut SolveOut, chips: &mut Vec<Chip>) {
        let hs = self.hs();
        for (i, f) in self.floats.iter().enumerate() {
            let node = self.float_trees.get(i).cloned().flatten().and_then(|t| self.prune(&t, &BTreeSet::new()));
            let Some(node) = node else { continue };
            let g = self.float_geom(f, &node);
            let mut rec = Rec::default();
            self.solve_node(&node, g, 10 + i as i64, &mut rec, false);
            let hover = f.is("reveal", "hover");
            let fid = f.get("id");
            let is_open = !hover || matches!((&self.view.open, fid.as_ref().and_then(J::as_str)), (Some(o), Some(id)) if o == id);
            Solver::merge_layer(rec, out, chips, is_open);
            if hover {
                let an = f.get_str("anchor").map(|s| s.to_string()).unwrap_or_default();
                let mut hxp = if an.contains('l') {
                    g.x
                } else if an.contains('r') {
                    g.x + g.w - hs
                } else {
                    g.x + fdiv(g.w - hs, 2)
                };
                let mut hyp = if an.starts_with('t') {
                    g.y
                } else if an.starts_with('b') {
                    g.y + g.h - hs
                } else {
                    g.y + fdiv(g.h - hs, 2)
                };
                hxp = clamp(hxp, 0, (self.ww - hs).max(0));
                hyp = clamp(hyp, 0, (self.hh - hs).max(0));
                out.drawers.push(DrawerOut {
                    id: js_str(fid.as_ref()),
                    side: an,
                    handle: Rect { x: hxp, y: hyp, w: hs, h: hs },
                    panel: g,
                    open: is_open,
                    scroll: None,
                    merged: None,
                    count: None,
                    vis: None,
                    z: None,
                });
            }
        }
    }

    // ---- the emergency arrangement (§7.12) ----
    fn emergency(&mut self, full: &N, out: &mut SolveOut) {
        self.emergency_mode = true;
        self.folded = BTreeSet::new();
        *out = SolveOut { emergency: true, collapsed: vec!["emergency".into()], ..SolveOut::default() };
        let g = self.gap;
        let h = self.host;
        let mut subtrees: Vec<N> = Vec::new();
        let mut loose: Vec<N> = Vec::new();
        let is_main_or_panel = |r: &str| is_main(r) || r == "panel";
        fn walk(n: &N, subtrees: &mut Vec<N>, pred: &dyn Fn(&str) -> bool) {
            let refs = Solver::islands_in(n);
            if !refs.iter().any(|r| pred(r)) {
                subtrees.push(n.clone());
                return;
            }
            if n.kind == Kind::Island {
                return;
            }
            for c in &n.kids {
                walk(c, subtrees, pred);
            }
        }
        walk(full, &mut subtrees, &is_main_or_panel);
        let mut refs = Solver::islands_in(full);
        for (i, _) in self.floats.iter().enumerate() {
            let node = self.float_trees.get(i).cloned().flatten().and_then(|t| self.prune(&t, &BTreeSet::new()));
            let Some(node) = node else { continue };
            for r in Solver::islands_in(&node) {
                if r == "call" || r == "panel" {
                    refs.push(r);
                } else if !is_main_or_panel(&r) {
                    loose.push(leaf(&r));
                }
            }
        }
        let opts = self.form.get("opts");
        let flag = |r: &str, k: &str| -> bool {
            let o = opts.as_ref().filter(|o| o.is_obj()).and_then(|o| o.get(r));
            o.is_some_and(|o| o.is_obj() && o.get(k).is_some_and(|v| v.is_true()))
        };
        let room: [&str; 2] = if flag("composer", "composerTop") { ["composer", "timeline"] } else { ["timeline", "composer"] };
        let hb = flag("header", "headerBottom");
        let mut order0: Vec<&str> = Vec::new();
        if !hb {
            order0.push("header");
        }
        order0.push("call");
        order0.extend(room);
        if hb {
            order0.push("header");
        }
        order0.retain(|r| refs.iter().any(|x| x == r));
        let nat = |r: &str| self.natural(&leaf(r), Axis::H);
        let fl2 = |r: &str| self.reg_min(&leaf(r), Axis::H);
        let has2 = |r: &str| order0.contains(&r);
        let comp = if has2("composer") { nat("composer").max(fl2("composer")) } else { 0 };
        let cc = self.content.get("call").copied();
        // the call's usable minimum: its control bar, never below the registry's `bar` (a measurement cannot hide it)
        let reg_bar = def("call").map_or(0, |d| d.bar);
        let call_b = if has2("call") { self.sc(reg_bar).max(cc.and_then(|c| c.bar_h).unwrap_or(0)) } else { 0 };
        let call_c = if has2("call") { cc.and_then(|c| c.collapsed_h).unwrap_or_else(|| fl2("call")).max(call_b) } else { 0 };
        let call_n = if has2("call") { nat("call").max(call_c) } else { 0 };
        let head = if has2("header") { nat("header") } else { 0 };
        let tlf = if has2("timeline") { fl2("timeline") } else { 0 };
        let call_on = has2("call");
        let avail = |o: &[&str]| h.h - g * (o.len() as i64 - 1).max(0);
        let mut lens: Option<BTreeMap<&str, i64>> = None;
        let mut order: Vec<&str> = Vec::new();
        let mut too_small = false;
        let drops: [&[&str]; 3] = [&[], &["header"], &["header", "timeline"]];
        for drop in drops {
            let o: Vec<&str> = order0.iter().copied().filter(|r| !drop.contains(r)).collect();
            if !o.contains(&"call") && !o.contains(&"composer") && !o.contains(&"timeline") {
                continue;
            }
            let a = avail(&o);
            let hd = if o.contains(&"header") { head } else { 0 };
            let tf = if o.contains(&"timeline") { tlf } else { 0 };
            let call = if call_on { clamp(a - comp - hd - tf, call_c, call_n) } else { 0 };
            let rest = a - comp - hd - call;
            if rest >= tf && (!o.contains(&"timeline") || rest > 0) {
                lens = Some(BTreeMap::from([("header", hd), ("call", call), ("composer", comp), ("timeline", rest)]));
                order = o;
                break;
            }
        }
        if lens.is_none() {
            let o: Vec<&str> = order0.iter().copied().filter(|r| *r == "call" || *r == "composer").collect();
            let a = avail(&o);
            if call_on && o.contains(&"composer") && a >= call_b + comp {
                lens = Some(BTreeMap::from([("call", call_c.min(a - comp)), ("composer", comp)]));
                order = o;
            } else if call_on && h.h >= call_b {
                lens = Some(BTreeMap::from([("call", call_n.min(h.h))]));
                order = vec!["call"];
            } else if !call_on && o.contains(&"composer") && h.h >= comp {
                lens = Some(BTreeMap::from([("composer", h.h)]));
                order = vec!["composer"];
            } else {
                let keep = if call_on {
                    Some("call")
                } else if o.contains(&"composer") {
                    Some("composer")
                } else if has2("timeline") {
                    Some("timeline")
                } else {
                    None
                };
                lens = Some(keep.map(|k| BTreeMap::from([(k, h.h)])).unwrap_or_default());
                order = keep.into_iter().collect();
                too_small = true;
            }
        }
        let lens = lens.unwrap_or_default();
        let mut y = h.y;
        for r in &order {
            let ln = lens.get(r).copied().unwrap_or(0);
            out.islands.insert(r.to_string(), IslandOut { x: h.x, y, w: h.w, h: ln, z: 0, vis: ln > 0 && h.w > 0, inset: None });
            y += ln + g;
        }
        for r in crate::schema::MAIN {
            if refs.iter().any(|x| x == r) && !order.contains(&r) {
                out.islands.insert(r.to_string(), IslandOut { x: h.x, y: h.y, w: 0, h: 0, z: 0, vis: false, inset: None });
            }
        }
        if too_small {
            out.too_small = true;
        }
        if refs.iter().any(|x| x == "panel") {
            let t = out.islands.get("timeline").cloned();
            let pw = self.sc(324).min(h.w);
            let p = match t {
                Some(t) if t.vis => IslandOut { x: h.x + h.w - pw, y: t.y, w: pw, h: t.h, z: 2, vis: pw > 0, inset: None },
                _ => IslandOut { x: h.x, y: h.y, w: 0, h: 0, z: 2, vis: false, inset: None },
            };
            out.islands.insert("panel".into(), p);
        }
        let mut kids = subtrees;
        kids.extend(loose);
        if !kids.is_empty() {
            let node = if kids.len() == 1 { kids.remove(0) } else { synthetic_col("emergency".into(), kids) };
            let mut chips = Vec::new();
            self.place_drawers(vec![DrawerItem { rf: "emergency".into(), w: None, node, merged: None }], full, out, &mut chips, Some(Rect { x: h.x, y: h.y, w: 0, h: h.h }));
            self.place_chips(&chips, out);
        }
    }
}

/// LAYOUT.md §7: the solver's entry point. `form` must be a valid (repaired) desktop form or an internal preset.
pub fn solve(form: &J, size: Option<&J>, unit_pm: Option<&J>, shape: Option<&J>, content: Option<&J>, view: Option<&J>) -> SolveOut {
    solve_detail(form, size, unit_pm, shape, content, view).out
}
/// solve + the docked distribution records the operations read (resize, resetSizes, dropZones).
pub fn solve_detail(form: &J, size: Option<&J>, unit_pm: Option<&J>, shape: Option<&J>, content: Option<&J>, view: Option<&J>) -> SolveDetail {
    // §1.4: a form that is not a JSON object is solved as an empty form (no root: floats only, i.e. nothing)
    let empty = J::obj();
    let f = if form.is_obj() && is_json_value(Some(form)) { form } else { &empty };
    Solver::new(f, size, unit_pm, shape, content, view).run()
}
