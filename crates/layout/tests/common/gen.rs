//! gen.rs — the reference's seeded generators (`tools/layout/gen-lib.mjs`) ported number for number: xorshift32,
//! randomContent / randomView / randomCtx, randomOp. Every call consumes the PRNG in the reference's order (object
//! literals left to right, `&&` short-circuits), so a seed gives the reference's sequence.
#![allow(dead_code)]

use tinhat_layout::form::{all_locs, floats_of, hidden_of, is_island, is_known_type, key_of};
use tinhat_layout::json::{mk, read_json, J};
use tinhat_layout::solve;

/// xorshift32 (Marsaglia 13/17/5); seed 0 → 1.
pub struct Rnd(u32);

impl Rnd {
    pub fn new(seed: u32) -> Rnd {
        Rnd(if seed == 0 { 1 } else { seed })
    }
    pub fn next(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }
    /// 0..n-1
    pub fn int(&mut self, n: u32) -> u32 {
        self.next() % n
    }
    pub fn pick<T: Clone>(&mut self, a: &[T]) -> T {
        let i = self.next() as usize % a.len();
        a[i].clone()
    }
    pub fn chance(&mut self, pm: u32) -> bool {
        self.next() % 1000 < pm
    }
    pub fn range(&mut self, lo: i64, hi: i64) -> i64 {
        lo + (self.next() as i64 % (hi - lo + 1))
    }
}

fn o(text: &str) -> J {
    read_json(text).expect("literal JSON")
}

pub fn base_content() -> J {
    o(r#"{"rooms":{"h":300},"nav":{"h":228},"dms":{"h":0,"empty":true},"widgets":{"h":0,"empty":true},"account":{"h":46},"header":{"h":58},"composer":{"h":48}}"#)
}
pub fn rich_content() -> J {
    o(r#"{"rooms":{"h":900},"nav":{"h":228},"dms":{"h":160},"widgets":{"h":400,"minH":108},"account":{"h":120},"header":{"h":110},"composer":{"h":180},"call":{"h":1400,"collapsedH":104},"buttons:a1":{"w":120,"h":40}}"#)
}

/// A random content set: every known island may get natural / minimum sizes, the call its collapsed / bar heights.
pub fn random_content(rnd: &mut Rnd) -> J {
    let c = J::obj();
    let h = rnd.range(0, 1200);
    let rooms = mk(vec![("h", J::int(h))]);
    if rnd.chance(300) {
        rooms.set("minH", J::int(rnd.range(0, 300)));
    }
    c.set("rooms", rooms);
    let nav = mk(vec![("h", J::int(rnd.range(0, 400)))]);
    if rnd.chance(200) {
        nav.set("w", J::int(rnd.range(40, 400)));
    }
    c.set("nav", nav);
    let dms = if rnd.chance(500) { mk(vec![("empty", J::Bool(true))]) } else { mk(vec![("h", J::int(rnd.range(0, 400)))]) };
    c.set("dms", dms);
    let widgets = if rnd.chance(400) {
        mk(vec![("empty", J::Bool(true))])
    } else {
        let h = rnd.range(0, 800);
        mk(vec![("h", J::int(h)), ("minH", J::int(rnd.range(0, 200)))])
    };
    c.set("widgets", widgets);
    c.set("account", mk(vec![("h", J::int(rnd.range(0, 200)))]));
    let header = mk(vec![("h", J::int(rnd.range(0, 200)))]);
    if rnd.chance(200) {
        header.set("minW", J::int(rnd.range(0, 600)));
    }
    c.set("header", header);
    c.set("composer", mk(vec![("h", J::int(rnd.range(0, 300)))]));
    let h = rnd.range(0, 2000);
    let ch = rnd.range(0, 400);
    let call = mk(vec![("h", J::int(h)), ("collapsedH", J::int(ch))]);
    if rnd.chance(500) {
        call.set("barH", J::int(rnd.range(0, 120)));
    }
    c.set("call", call);
    if rnd.chance(300) {
        c.set("panel", mk(vec![("minW", J::int(rnd.range(0, 500)))]));
    }
    if rnd.chance(300) {
        c.set("timeline", mk(vec![("minH", J::int(rnd.range(0, 400)))]));
    }
    for k in ["a1", "b1", "b2", "b3"] {
        if rnd.chance(500) {
            let w = rnd.range(0, 400);
            c.set(&format!("buttons:{k}"), mk(vec![("w", J::int(w)), ("h", J::int(rnd.range(0, 120)))]));
        }
    }
    c
}

const VIEWS: [&str; 7] = [
    r#"{"roomOpen":true}"#,
    r#"{"roomOpen":true,"panelOpen":true}"#,
    r#"{"roomOpen":true,"callShown":true}"#,
    r#"{"roomOpen":true,"callShown":true,"panelOpen":true}"#,
    r#"{"roomOpen":true,"callShown":true,"sizes":{"call":{"pm":700}}}"#,
    r#"{"roomOpen":false}"#,
    r#"{"roomOpen":true,"callShown":true,"maximized":"call"}"#,
];

fn root_keys(form: &J) -> Vec<String> {
    all_locs(form).into_iter().filter(|l| l.float < 0 && is_known_type(&l.node)).filter_map(|l| key_of(&l.node)).map(|k| k.to_string()).filter(|k| !k.is_empty()).collect()
}

/// A random view state for a form: presence switches, seam overrides, forced collapse, one open drawer, tabs.
pub fn random_view(rnd: &mut Rnd, form: &J) -> J {
    let v = o(rnd.pick(&VIEWS));
    let keys = root_keys(form);
    let steps: Vec<String> = form.get("collapse").filter(J::is_arr).map(|c| c.items().iter().map(|s| s.get_str("ref").map(|r| r.to_string()).unwrap_or_default()).collect()).unwrap_or_default();
    if rnd.chance(200) && (!steps.is_empty() || !keys.is_empty()) {
        let from_steps = rnd.chance(700) && !steps.is_empty();
        let r = if from_steps { rnd.pick(&steps) } else { rnd.pick(&keys) };
        v.set("forceCollapse", J::strs(&[r]));
    } else if rnd.chance(120) && !keys.is_empty() {
        let n = rnd.range(2, 8);
        let mut fc = Vec::new();
        for _ in 0..n {
            fc.push(rnd.pick(&keys));
        }
        v.set("forceCollapse", J::strs(&fc));
    }
    if rnd.chance(150) {
        let s = v.get("sizes").filter(J::is_obj).unwrap_or_else(J::obj);
        s.set("call", mk(vec![("pm", J::int(rnd.range(200, 850)))]));
        v.set("sizes", s);
    }
    if rnd.chance(80) && !keys.is_empty() {
        let k = rnd.pick(&keys);
        let choices = [
            mk(vec![("fixed", J::int(rnd.range(0, 900)))]),
            mk(vec![("share", J::int(rnd.range(1, 5)))]),
            mk(vec![("pm", J::int(rnd.range(0, 1000)))]),
            mk(vec![("min", J::int(rnd.range(0, 500)))]),
        ];
        let val = rnd.pick(&choices);
        let s = v.get("sizes").filter(J::is_obj).unwrap_or_else(J::obj);
        s.set(&k, val);
        v.set("sizes", s);
    }
    let mut drawer_ids: Vec<String> = steps.clone();
    if let Some(fc) = v.get("forceCollapse") {
        drawer_ids.extend(fc.items().iter().filter_map(|x| x.as_str().map(str::to_string)));
    }
    drawer_ids.push("emergency".into());
    drawer_ids.extend(floats_of(form).iter().map(|f| f.get_str("id").map(|s| s.to_string()).unwrap_or_default()));
    drawer_ids.extend(keys.iter().map(|k| format!("overflow-{k}")));
    if rnd.chance(400) {
        let open = rnd.pick(&drawer_ids);
        let pinned = rnd.chance(300);
        v.set("drawer", mk(vec![("open", J::s(&open)), ("pinned", J::Bool(pinned))]));
    }
    let stacks: Vec<J> = all_locs(form).into_iter().filter(|l| l.node.is("t", "stack")).map(|l| l.node).collect();
    if !stacks.is_empty() && rnd.chance(400) {
        let s = rnd.pick(&stacks);
        let mut tabs: Vec<J> = s.get("c").map(|c| c.items()).unwrap_or_default().iter().map(|c| c.get("i").unwrap_or_default()).collect();
        tabs.push(J::s("nav"));
        let t = rnd.pick(&tabs);
        let a = J::obj();
        a.set(&s.get_str("id").map(|x| x.to_string()).unwrap_or_default(), t);
        v.set("active", a);
    }
    if rnd.chance(40) {
        let k = rnd.pick(&["rooms", "widgets", "panel"]);
        v.set("popouts", mk(vec![(k, o(r#"{"x":0,"y":0,"w":300,"h":400}"#))]));
    }
    v
}

const UNITS: [i64; 10] = [1000, 1000, 1000, 1000, 500, 889, 1111, 1500, 2000, 4000];

/// A random solve context `{size, unitPm, shape, content, view}`.
pub fn random_ctx(rnd: &mut Rnd, form: &J) -> J {
    let ww = if rnd.chance(700) {
        rnd.pick(&[480i64, 640, 700, 720, 800, 900, 1024, 1100, 1280, 1366, 1560, 1920, 2560, 3840]) + rnd.range(-40, 40)
    } else {
        rnd.range(1, 3840)
    };
    let hh = if rnd.chance(700) { rnd.pick(&[120i64, 200, 340, 520, 600, 720, 800, 960, 1080, 1440, 2160]) + rnd.range(-40, 40) } else { rnd.range(1, 2160) };
    let shape = J::obj();
    if rnd.chance(300) {
        shape.set("gap", J::int(rnd.range(0, 32)));
    }
    if rnd.chance(300) {
        shape.set("margin", J::int(rnd.pick(&[0i64, 0, 6, 32, 64, 200])));
    }
    if rnd.chance(100) {
        shape.set("tab", J::int(rnd.range(0, 60)));
    }
    if rnd.chance(100) {
        shape.set("handle", J::int(rnd.range(0, 120)));
    }
    if rnd.chance(100) {
        shape.set("drawerW", J::int(rnd.range(0, 600)));
    }
    if rnd.chance(100) {
        shape.set("drawerPad", J::int(rnd.range(0, 40)));
    }
    let size = mk(vec![("w", J::int(ww.max(0))), ("h", J::int(hh.max(0)))]);
    let unit = rnd.pick(&UNITS);
    let content = if rnd.chance(200) {
        if rnd.chance(500) {
            base_content()
        } else {
            rich_content()
        }
    } else {
        random_content(rnd)
    };
    let view = random_view(rnd, form);
    mk(vec![("size", size), ("unitPm", J::int(unit)), ("shape", shape), ("content", content), ("view", view)])
}

const SIDES: [&str; 4] = ["left", "right", "top", "bottom"];
const ANCHORS: [&str; 8] = ["tl", "t", "tr", "r", "br", "b", "bl", "l"];
const PRESETS: [&str; 5] = ["classic", "compact", "callFirst", "sideMonitor", "handheld"];

/// A random operation for `form` (valid or not — the engine decides).
pub fn random_op(rnd: &mut Rnd, form: &J, ctx: &J) -> J {
    let locs = all_locs(form);
    let islands: Vec<String> = locs.iter().filter(|l| is_island(&l.node)).map(|l| l.node.get_str("i").map(|s| s.to_string()).unwrap_or_default()).collect();
    let groups: Vec<String> = locs.iter().filter(|l| is_known_type(&l.node) && !is_island(&l.node)).map(|l| key_of(&l.node).map(|k| k.to_string()).unwrap_or_default()).collect();
    let hidden: Vec<String> = hidden_of(form).iter().map(|h| h.get_str("i").map(|s| s.to_string()).unwrap_or_default()).collect();
    let fids: Vec<String> = floats_of(form).iter().map(|f| f.get_str("id").map(|s| s.to_string()).unwrap_or_default()).collect();
    let mut any = islands.clone();
    any.extend(groups.iter().cloned());
    let movable = if rnd.chance(800) { islands.clone() } else { any.clone() };
    let pick_ref = |rnd: &mut Rnd| if movable.is_empty() { "rooms".to_string() } else { rnd.pick(&movable) };
    let tgt = |rnd: &mut Rnd| if any.is_empty() { "timeline".to_string() } else { rnd.pick(&any) };
    let r = rnd.int(100);
    if r < 28 {
        let (a, t, s) = (pick_ref(rnd), tgt(rnd), rnd.pick(&SIDES));
        return mk(vec![("op", J::s("dockBeside")), ("ref", J::s(&a)), ("target", J::s(&t)), ("side", J::s(s))]);
    }
    if r < 36 {
        let (a, t) = (pick_ref(rnd), tgt(rnd));
        return mk(vec![("op", J::s("stackAsTab")), ("ref", J::s(&a)), ("target", J::s(&t))]);
    }
    if r < 44 {
        let (a, s) = (pick_ref(rnd), rnd.pick(&SIDES));
        return mk(vec![("op", J::s("edge")), ("ref", J::s(&a)), ("side", J::s(s))]);
    }
    if r < 52 {
        let a = pick_ref(rnd);
        let an = rnd.pick(&ANCHORS);
        let dx = rnd.range(-40, 200);
        let dy = rnd.range(-40, 200);
        let op = mk(vec![("op", J::s("float")), ("ref", J::s(&a)), ("anchor", J::s(an)), ("dx", J::int(dx)), ("dy", J::int(dy))]);
        if rnd.chance(300) {
            op.set("reveal", J::s("hover"));
        }
        return op;
    }
    if r < 58 && (!fids.is_empty() || !islands.is_empty()) {
        let a = if !fids.is_empty() && rnd.chance(600) { rnd.pick(&fids) } else { rnd.pick(&islands) };
        let to = if rnd.chance(500) {
            let (t, s) = (tgt(rnd), rnd.pick(&SIDES));
            mk(vec![("op", J::s("dockBeside")), ("target", J::s(&t)), ("side", J::s(s))])
        } else if rnd.chance(500) {
            mk(vec![("op", J::s("edge")), ("side", J::s(rnd.pick(&SIDES)))])
        } else {
            mk(vec![("op", J::s("stackAsTab")), ("target", J::s(&tgt(rnd)))])
        };
        return mk(vec![("op", J::s("dock")), ("ref", J::s(&a)), ("to", to)]);
    }
    if r < 68 {
        return mk(vec![("op", J::s("hide")), ("ref", J::s(&pick_ref(rnd)))]);
    }
    if r < 76 && !hidden.is_empty() {
        let a = rnd.pick(&hidden);
        if rnd.chance(300) {
            let to = if rnd.chance(600) {
                let (t, s) = (tgt(rnd), rnd.pick(&SIDES));
                mk(vec![("op", J::s("dockBeside")), ("target", J::s(&t)), ("side", J::s(s))])
            } else {
                mk(vec![("op", J::s("edge")), ("side", J::s(rnd.pick(&SIDES)))])
            };
            return mk(vec![("op", J::s("restore")), ("ref", J::s(&a)), ("to", to)]);
        }
        return mk(vec![("op", J::s("restore")), ("ref", J::s(&a))]);
    }
    if r < 88 {
        let out = solve(form, ctx.get("size").as_ref(), ctx.get("unitPm").as_ref(), ctx.get("shape").as_ref(), ctx.get("content").as_ref(), ctx.get("view").as_ref());
        if !out.gaps.is_empty() {
            let g = rnd.pick(&out.gaps);
            if rnd.chance(200) {
                return mk(vec![("op", J::s("resetSizes")), ("a", J::s(&g.a)), ("b", J::s(&g.b))]);
            }
            return mk(vec![("op", J::s("resize")), ("a", J::s(&g.a)), ("b", J::s(&g.b)), ("delta", J::int(rnd.range(-400, 400)))]);
        }
        let (a, b) = (tgt(rnd), tgt(rnd));
        return mk(vec![("op", J::s("resize")), ("a", J::s(&a)), ("b", J::s(&b)), ("delta", J::int(rnd.range(-100, 100)))]);
    }
    if r < 96 {
        let k = rnd.pick(&["compact", "orient", "orient", "note"]);
        let rf = rnd.pick(&["rooms", "nav", "dms", "spaces", "widgets", "buttons:a1"]);
        if k == "compact" {
            return mk(vec![("op", J::s("setOpt")), ("ref", J::s(rf)), ("key", J::s("compact")), ("value", J::Bool(rnd.chance(500)))]);
        }
        if k == "orient" {
            return mk(vec![("op", J::s("setOpt")), ("ref", J::s(rf)), ("key", J::s("orient")), ("value", J::s(rnd.pick(&["h", "v"])))]);
        }
        let val = rnd.pick(&[J::int(1), J::s("x"), J::Null, o("[1,2]")]);
        return mk(vec![("op", J::s("setOpt")), ("ref", J::s(rf)), ("key", J::s("note")), ("value", val)]);
    }
    if r < 98 {
        return mk(vec![("op", J::s("applyPreset")), ("preset", J::s(rnd.pick(&PRESETS)))]);
    }
    o(rnd.pick(&[
        r#"{"op":"nonsense"}"#,
        r#"{"op":"dockBeside","ref":5}"#,
        r#"{"op":"edge","ref":"rooms","side":"middle"}"#,
        r#"{"op":"float","ref":"rooms","anchor":"zz","dx":0,"dy":0}"#,
    ]))
}
