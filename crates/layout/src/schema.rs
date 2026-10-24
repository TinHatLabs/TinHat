//! schema.rs — port of `schema.ts`: the contract data of `layout.schema.json` (`x-tinhat`): the island registry,
//! limits, bounds, shape defaults, presets, region order, identifier syntax (LAYOUT.md §2). The schema file is a byte
//! copy of the contract's (`sync-contract.sh`), embedded at build time.
//!
//! Registry iteration order is never the JSON key order: anything ordered uses [`REGION_ORDER`].

use std::sync::OnceLock;

use serde_json::Value;

use crate::json::J;

/// The contract's schema file (a byte copy; tests/vectors.rs checks it against the contract).
pub const SCHEMA_TEXT: &str = include_str!("../layout.schema.json");

/// An axis: `w` (a row's) or `h` (a col's).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Axis {
    W,
    H,
}

impl Axis {
    pub fn idx(self) -> usize {
        match self {
            Axis::W => 0,
            Axis::H => 1,
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            Axis::W => "w",
            Axis::H => "h",
        }
    }
    pub fn cross(self) -> Axis {
        match self {
            Axis::W => Axis::H,
            Axis::H => Axis::W,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Presence {
    Always,
    Content,
    Room,
    Open,
    Call,
}

/// A size as solve reads it (`Sz`): the known fields that hold integers, `auto` when it is `true`.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Sz {
    pub fixed: Option<i64>,
    pub share: Option<i64>,
    pub auto: bool,
    pub pm: Option<i64>,
    pub min: Option<i64>,
    pub max: Option<i64>,
    pub min_pm: Option<i64>,
    pub max_pm: Option<i64>,
    pub grow: Option<i64>,
}

/// Integers as solve reads them: a valid form holds only in-range values; anything larger is capped far beyond every
/// contract bound so the i64 arithmetic can never overflow (the reference would lose precision there instead).
fn sz_int(v: Option<J>) -> Option<i64> {
    v.and_then(|x| x.int_val()).map(|n| n.clamp(-(1 << 40), 1 << 40))
}

impl Sz {
    /// `toSz(o)`: the integer fields of a size object, `auto` when it is `true` (anything else: an empty size).
    pub fn of(o: Option<&J>) -> Sz {
        let Some(o) = o.filter(|o| o.is_obj()) else { return Sz::default() };
        Sz {
            fixed: sz_int(o.get("fixed")),
            share: sz_int(o.get("share")),
            auto: o.get("auto").is_some_and(|a| a.is_true()),
            pm: sz_int(o.get("pm")),
            min: sz_int(o.get("min")),
            max: sz_int(o.get("max")),
            min_pm: sz_int(o.get("minPm")),
            max_pm: sz_int(o.get("maxPm")),
            grow: sz_int(o.get("grow")),
        }
    }
    pub fn has_basis(&self) -> bool {
        self.fixed.is_some() || self.share.is_some() || self.auto || self.pm.is_some()
    }
}

/// Registry sizes for one orientation: the pref size objects (w, h) and the minima.
#[derive(Debug)]
pub struct RegSizes {
    pub pref: [Value; 2],
    pub pref_sz: [Sz; 2],
    pub min: [i64; 2],
}

impl RegSizes {
    /// The pref size object for an axis, as fresh data.
    pub fn pref_j(&self, a: Axis) -> J {
        J::from_value(&self.pref[a.idx()])
    }
}

#[derive(Debug)]
pub struct IslandDef {
    pub id: String,
    pub presence: Presence,
    pub hideable: bool,
    pub required: bool,
    pub stackable: bool,
    pub floatable: bool,
    pub popout: bool,
    /// view.maximized honours it (only the call)
    pub maximizable: bool,
    pub multi: bool,
    /// `rail` | `list`, or none (the room group, panel and call are no phone sections)
    pub phone: Option<String>,
    pub sizes: RegSizes,
    pub orient_h: Option<RegSizes>,
    /// the registry float size (w, h), when the island can float
    pub float: Option<[Value; 2]>,
    pub collapse: Vec<String>,
    pub home: Vec<Value>,
    /// the call: the usable minimum of its control bar (layout units), 0 elsewhere
    pub bar: i64,
}

impl IslandDef {
    pub fn collapse_allows(&self, to: &str) -> bool {
        self.collapse.iter().any(|c| c == to)
    }
}

#[derive(Debug)]
pub struct Limits {
    pub doc_bytes: usize,
    pub layouts: usize,
    pub tombstones: usize,
    pub form_bytes: usize,
    pub nodes: usize,
    pub depth: usize,
    pub children: usize,
    pub stack_tabs: usize,
    pub floats: usize,
    pub buttons_islands: usize,
    pub hidden: usize,
    pub collapse: usize,
    pub vv: usize,
}

#[derive(Debug)]
pub struct Bounds {
    pub host_px: (i64, i64),
    pub unit_pm: (i64, i64),
    pub content_px: (i64, i64),
    pub layout_units: (i64, i64),
}

/// The six shape keys (layout units).
pub const SHAPE_KEYS: [&str; 6] = ["gap", "margin", "tab", "handle", "drawerW", "drawerPad"];

#[derive(Debug)]
pub struct Schema {
    /// the registry in region order
    pub islands: Vec<IslandDef>,
    pub region_order: Vec<String>,
    pub lim: Limits,
    pub bounds: Bounds,
    /// SHAPE_KEYS order
    pub shape_defaults: [i64; 6],
    /// x-tinhat.bounds.shapeMin (SHAPE_KEYS order): usable chrome — a theme's shape values are raised to these
    pub shape_min: [i64; 6],
    pub errors: Vec<String>,
    pub notes: Vec<String>,
    presets: Value,
    internal: Value,
}

fn num(v: &Value, k: &str) -> i64 {
    v.get(k).and_then(Value::as_f64).unwrap_or_else(|| panic!("layout.schema.json: {k} is a number")) as i64
}
fn pair(v: &Value, k: &str) -> (i64, i64) {
    let a = v.get(k).and_then(Value::as_array).unwrap_or_else(|| panic!("layout.schema.json: bounds.{k}"));
    (a[0].as_f64().unwrap() as i64, a[1].as_f64().unwrap() as i64)
}
fn sizes(o: &Value) -> RegSizes {
    let pref = o.get("pref").expect("pref");
    let min = o.get("min").expect("min");
    let (pw, ph) = (pref.get("w").expect("pref.w").clone(), pref.get("h").expect("pref.h").clone());
    let pref_sz = [Sz::of(Some(&J::from_value(&pw))), Sz::of(Some(&J::from_value(&ph)))];
    RegSizes { pref: [pw, ph], pref_sz, min: [num(min, "w"), num(min, "h")] }
}
fn strings(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

fn read_def(id: &str, d: &Value) -> IslandDef {
    let flag = |k: &str| d.get(k) == Some(&Value::Bool(true));
    let presence = match d.get("presence").and_then(Value::as_str) {
        Some("always") => Presence::Always,
        Some("content") => Presence::Content,
        Some("room") => Presence::Room,
        Some("open") => Presence::Open,
        Some("call") => Presence::Call,
        p => panic!("layout.schema.json: island {id} presence {p:?}"),
    };
    let phone = match d.get("phone").and_then(Value::as_str) {
        Some(p @ ("rail" | "list")) => Some(p.to_string()),
        _ => None,
    };
    let orient_h = d.get("orientH").filter(|o| o.is_object()).map(sizes);
    let float = d.get("float").filter(|o| o.is_object()).map(|f| [f["w"].clone(), f["h"].clone()]);
    IslandDef {
        id: id.to_string(),
        presence,
        hideable: flag("hideable"),
        required: flag("required"),
        stackable: flag("stackable"),
        floatable: flag("floatable"),
        popout: flag("popout"),
        maximizable: flag("maximizable"),
        multi: flag("multi"),
        phone,
        sizes: sizes(d),
        orient_h,
        float,
        collapse: strings(d.get("collapse")),
        home: d.get("home").and_then(Value::as_array).cloned().unwrap_or_default(),
        bar: d.get("bar").and_then(Value::as_f64).map(|b| b as i64).unwrap_or(0),
    }
}

fn load() -> Schema {
    let v = tinhat_theme::json::parse_json(SCHEMA_TEXT).expect("layout.schema.json is JSON");
    let x = v.get("x-tinhat").expect("x-tinhat");
    let region_order = strings(x.get("regionOrder").and_then(|r| r.get("order")));
    let isl = x.get("islands").expect("islands");
    let islands = region_order.iter().map(|id| read_def(id, isl.get(id).expect("a registry island per region"))).collect();
    let l = x.get("limits").expect("limits");
    let lim = Limits {
        doc_bytes: num(l, "docBytes") as usize,
        layouts: num(l, "layouts") as usize,
        tombstones: num(l, "tombstones") as usize,
        form_bytes: num(l, "formBytes") as usize,
        nodes: num(l, "nodes") as usize,
        depth: num(l, "depth") as usize,
        children: num(l, "children") as usize,
        stack_tabs: num(l, "stackTabs") as usize,
        floats: num(l, "floats") as usize,
        buttons_islands: num(l, "buttonsIslands") as usize,
        hidden: num(l, "hidden") as usize,
        collapse: num(l, "collapse") as usize,
        vv: num(l, "vv") as usize,
    };
    let b = x.get("bounds").expect("bounds");
    let bounds = Bounds {
        host_px: pair(b, "hostPx"),
        unit_pm: pair(b, "unitPm"),
        content_px: pair(b, "contentPx"),
        layout_units: pair(b, "layoutUnits"),
    };
    let sd = x.get("shapeDefaults").expect("shapeDefaults");
    let mut shape_defaults = [0i64; 6];
    for (i, k) in SHAPE_KEYS.iter().enumerate() {
        shape_defaults[i] = num(sd, k);
    }
    let sm = b.get("shapeMin").expect("bounds.shapeMin");
    let mut shape_min = [0i64; 6];
    for (i, k) in SHAPE_KEYS.iter().enumerate() {
        shape_min[i] = num(sm, k);
    }
    Schema {
        islands,
        region_order,
        lim,
        bounds,
        shape_defaults,
        shape_min,
        errors: strings(x.get("errors")),
        notes: strings(x.get("repairNotes")),
        presets: x.get("presets").cloned().expect("presets"),
        internal: x.get("internalPresets").cloned().expect("internalPresets"),
    }
}

/// The contract data (parsed once).
pub fn schema() -> &'static Schema {
    static S: OnceLock<Schema> = OnceLock::new();
    S.get_or_init(load)
}

/// Registry kinds in the fixed logical region order (x-tinhat.regionOrder; LAYOUT.md §2.1).
pub const REGION_ORDER: [&str; 12] =
    ["nav", "dms", "spaces", "rooms", "widgets", "account", "header", "call", "timeline", "composer", "panel", "buttons"];

/// The registry entry of a kind (`REGISTRY[kind]`).
pub fn def(kind: &str) -> Option<&'static IslandDef> {
    schema().islands.iter().find(|d| d.id == kind)
}

/// Preset names in the schema's documented order.
pub const PRESET_NAMES: [&str; 5] = ["classic", "compact", "callFirst", "sideMonitor", "handheld"];

fn preset_form(root: &Value, name: &str, which: &str) -> Option<J> {
    let p = root.get(name)?;
    if !p.is_object() {
        return None;
    }
    p.get(which).map(J::from_value)
}
/// A fresh copy of preset `name`'s desktop form (None for an unknown name).
pub fn preset_desktop(name: &str) -> Option<J> {
    preset_form(&schema().presets, name, "desktop")
}
pub fn preset_phone(name: &str) -> Option<J> {
    preset_form(&schema().presets, name, "phone")
}
/// The internal migration preset (classic-flush): solve only, never stored, never validated.
pub fn internal_desktop(name: &str) -> Option<J> {
    preset_form(&schema().internal, name, "desktop")
}
pub fn classic_desktop() -> J {
    preset_desktop("classic").expect("the classic preset")
}
pub fn classic_phone() -> J {
    preset_phone("classic").expect("the classic preset")
}

// ---- identifiers (LAYOUT.md §2.2) ----------------------------------------------------------------------------------
fn lower(c: u8) -> bool {
    c.is_ascii_lowercase()
}
fn upper(c: u8) -> bool {
    c.is_ascii_uppercase()
}
fn digit(c: u8) -> bool {
    c.is_ascii_digit()
}

/// `^[a-z][a-z0-9-]{0,31}$` — layout, container, stack and float ids.
pub fn is_id_syntax(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty() && b.len() <= 32 && lower(b[0]) && b[1..].iter().all(|&c| lower(c) || digit(c) || c == b'-')
}
pub fn is_id_syntax_j(v: Option<&J>) -> bool {
    v.and_then(J::as_str).is_some_and(is_id_syntax)
}

fn instance_ok(inst: &[u8]) -> bool {
    !inst.is_empty() && inst.len() <= 12 && inst.iter().all(|&c| lower(c) || digit(c))
}

/// `^[a-z][a-zA-Z0-9]{0,23}(:[a-z0-9]{1,12})?$` — island refs (kind, optionally `:instance`).
pub fn is_ref_syntax(s: &str) -> bool {
    let b = s.as_bytes();
    let colon = b.iter().position(|&c| c == b':');
    let kind = match colon {
        Some(i) => &b[..i],
        None => b,
    };
    if kind.is_empty() || kind.len() > 24 || !lower(kind[0]) || !kind[1..].iter().all(|&c| lower(c) || upper(c) || digit(c)) {
        return false;
    }
    match colon {
        None => true,
        Some(i) => instance_ok(&b[i + 1..]),
    }
}
pub fn is_ref_syntax_j(v: Option<&J>) -> bool {
    v.and_then(J::as_str).is_some_and(is_ref_syntax)
}

/// `^[a-z][a-zA-Z0-9-]{0,31}(:[a-z0-9]{1,12})?$` — a collapse step's ref (a node id or an island ref).
pub fn is_step_ref_syntax(s: &str) -> bool {
    let b = s.as_bytes();
    let colon = b.iter().position(|&c| c == b':');
    let head = match colon {
        Some(i) => &b[..i],
        None => b,
    };
    if head.is_empty()
        || head.len() > 32
        || !lower(head[0])
        || !head[1..].iter().all(|&c| lower(c) || upper(c) || digit(c) || c == b'-')
    {
        return false;
    }
    match colon {
        None => true,
        Some(i) => instance_ok(&b[i + 1..]),
    }
}

/// `^[a-z]{1,16}$` — a node type name.
pub fn is_type_syntax(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty() && b.len() <= 16 && b.iter().all(|&c| lower(c))
}

/// The kind of a ref (the part before the first `:`).
pub fn kind_of(r: &str) -> &str {
    match r.find(':') {
        Some(i) => &r[..i],
        None => r,
    }
}
/// A ref this version renders: a non-multi registry kind, or `<multi kind>:<instance>` (Q-KNOWN).
pub fn is_known_ref(r: &str) -> bool {
    def(kind_of(r)).is_some_and(|d| d.multi == r.contains(':'))
}
/// The registry entry of a known ref.
pub fn def_of(r: &str) -> Option<&'static IslandDef> {
    if is_known_ref(r) {
        def(kind_of(r))
    } else {
        None
    }
}
/// Compat islands of the internal migration preset (`_nav`, `_room`): solve only.
pub fn is_compat_ref(r: &str) -> bool {
    r.len() > 1 && r.as_bytes()[0] == b'_'
}
/// Reserved node ids: a registry kind, `emergency`, or `overflow-*` (one namespace with refs and drawer ids).
pub fn is_reserved_id(id: &str) -> bool {
    def(id).is_some() || id == "emergency" || id.starts_with("overflow-")
}

/// The emergency column's islands (LAYOUT.md §7.12).
pub const MAIN: [&str; 4] = ["header", "call", "timeline", "composer"];
pub fn is_main(r: &str) -> bool {
    MAIN.contains(&r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_and_syntax() {
        let s = schema();
        assert_eq!(s.region_order, REGION_ORDER.iter().map(|x| x.to_string()).collect::<Vec<_>>());
        assert!(def("timeline").unwrap().required && !def("timeline").unwrap().hideable);
        assert!(is_known_ref("rooms") && is_known_ref("buttons:a1"));
        assert!(!is_known_ref("buttons") && !is_known_ref("rooms:x") && !is_known_ref("zz"));
        assert!(is_ref_syntax("buttons:a1") && !is_ref_syntax("Bad Ref") && !is_ref_syntax("a:") && !is_ref_syntax("a:B"));
        assert!(is_id_syntax("n1") && !is_id_syntax("N1") && !is_id_syntax(&"a".repeat(33)));
        assert!(is_step_ref_syntax("side-col") && is_step_ref_syntax("buttons:a1"));
        assert!(is_reserved_id("buttons") && is_reserved_id("overflow-x") && !is_reserved_id("body"));
        assert!(is_compat_ref("_nav") && !is_compat_ref("_"));
        assert_eq!(s.lim.form_bytes, 8192);
        assert_eq!(s.shape_defaults, [6, 6, 28, 40, 340, 8]);
        for p in PRESET_NAMES {
            assert!(preset_desktop(p).is_some() && preset_phone(p).is_some(), "{p}");
        }
        assert!(internal_desktop("classic-flush").is_some());
    }
}
