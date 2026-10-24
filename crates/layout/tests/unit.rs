//! The reference's unit tests (web `src/layout/layout.test.ts`), mirrored test by test — the vector and contract-copy
//! tests live in tests/vectors.rs, the large random run in tests/differential.rs. The seeded tests use the reference's
//! own generators ported number for number (tests/common/gen.rs), so they walk the same random cases.

mod common;

use common::gen::{random_ctx, random_op, Rnd};
use common::{node_bin, Checker};
use tinhat_layout::json::{fdiv, mk, read_json, J};
use tinhat_layout::sync::{known_of, merge, project, save, Device, SaveReq, SaveResult};
use tinhat_layout::{
    apply_op, classic_desktop, internal_desktop, migrate, normalize, preset_desktop, preset_phone, repair_desktop, repair_desktop_text,
    repair_phone, sc, seam_pm, solve, unscale, validate_desktop, validate_phone, OpResult, PRESET_NAMES, REGION_ORDER,
};

fn o(text: &str) -> J {
    read_json(text).expect("literal JSON")
}
const C: &str = r#"{"rooms":{"h":300},"nav":{"h":228},"dms":{"h":0,"empty":true},"widgets":{"h":0,"empty":true},"account":{"h":46},"header":{"h":58},"composer":{"h":48}}"#;
fn content_with(extra: &str) -> J {
    let c = o(C);
    let e = o(extra);
    for k in e.keys() {
        c.set(&k, e.get(&k).unwrap());
    }
    c
}
fn run(form: &J, w: i64, h: i64, shape: Option<&J>, content: &J, view: &str) -> J {
    solve(form, Some(&o(&format!(r#"{{"w":{w},"h":{h}}}"#))), Some(&J::int(1000)), shape, Some(content), Some(&o(view))).to_json()
}
fn rect(out: &J, r: &str) -> [i64; 4] {
    let i = out.get("islands").unwrap().get(r).unwrap_or_else(|| panic!("island {r}"));
    ["x", "y", "w", "h"].map(|k| i.get(k).unwrap().int_val().unwrap())
}
fn num(v: &J, path: &[&str]) -> i64 {
    let mut x = v.clone();
    for p in path {
        x = x.get(p).unwrap_or_else(|| panic!("{p}"));
    }
    x.int_val().unwrap()
}

#[test]
fn presets_validate_cleanly_and_repair_to_themselves() {
    for p in PRESET_NAMES {
        let d = preset_desktop(p).unwrap();
        let ph = preset_phone(p).unwrap();
        assert!(validate_desktop(Some(&d)).is_empty(), "{p}");
        assert!(validate_phone(Some(&ph)).is_empty(), "{p}");
        assert_eq!(repair_desktop(Some(&d)).form.canon(), d.canon(), "{p}");
        assert_eq!(repair_phone(Some(&ph)).form.canon(), ph.canon(), "{p}");
    }
    assert!(internal_desktop("classic-flush").is_some());
    assert_eq!(REGION_ORDER, ["nav", "dms", "spaces", "rooms", "widgets", "account", "header", "call", "timeline", "composer", "panel", "buttons"]);
}

#[test]
fn json_reader_follows_the_colour_contract() {
    let r = read_json(r#"{"__proto__": {"a": 1}, "constructor": 2, "a": 1, "a": 3, "big": 1e400, "neg": -1e400, "z": -0}"#).unwrap();
    assert_eq!(r.canon(), r#"{"__proto__":{"a":1},"a":3,"big":null,"constructor":2,"neg":null,"z":0}"#);
    assert!(read_json(&("[".repeat(128) + &"]".repeat(128))).is_some());
    assert!(read_json(&("[".repeat(129) + &"]".repeat(129))).is_none());
    assert!(read_json(r#""a\ud800b""#).is_none());
    assert!(read_json(r#""\udc00""#).is_none());
    // (a raw lone surrogate cannot reach Rust: any UTF-8 transfer makes it U+FFFD — the repair vector
    // raw-lone-surrogate-becomes-fffd pins what the reference then does)
    for bad in ["", "{", "[1,]", "01", "NaN", "{a:1}", "'a'", "1 2"] {
        assert!(read_json(bad).is_none(), "{bad}");
    }
    // a form with __proto__ keys: ordinary keys, preserved
    let text = classic_desktop().canon().replace(r#""floats":[]"#, r#""floats":[],"__proto__":{"root":1}"#);
    let rep = repair_desktop_text(&text);
    assert!(rep.notes.is_empty(), "{:?}", rep.notes);
    assert_eq!(rep.form.get("__proto__").unwrap().canon(), r#"{"root":1}"#);
}

#[test]
fn arithmetic_sc_half_away_from_zero_unscale_floor_division() {
    assert_eq!(sc(360, 1000), 360);
    for gu in 9..=30i64 {
        let unit = ((gu * 1000) as f64 / 18.0).round() as i64;
        assert_eq!(sc(360, unit), 20 * gu, "gridUnit {gu}");
    }
    for u in [500, 889, 1000, 1111, 4000] {
        for v in [1, 5, 17, 68, 4095] {
            assert_eq!(sc(-v, u), -sc(v, u));
        }
    }
    assert_eq!(sc(0, 1000), 0);
    assert_eq!(sc(1, 1500), 2); // 1.5 → 2
    assert_eq!(sc(-1, 1500), -2);
    assert_eq!(fdiv(-7, 2), -4);
    assert_eq!(fdiv(7, 2), 3);
    for u in [500, 889, 1000, 1111, 4000] {
        let mut px = 0;
        while px < 3000 {
            assert!((sc(unscale(px, u), u) - px).abs() <= (u + 999) / 1000, "{px} @ {u}");
            px += 7;
        }
    }
}

#[test]
fn solve_the_worked_examples_of_islands_arch_3_2() {
    let classic = classic_desktop();
    let c = o(C);
    let out = run(&classic, 1560, 960, None, &c, r#"{"roomOpen":true}"#);
    assert_eq!(rect(&out, "nav"), [6, 6, 68, 228]);
    assert_eq!(rect(&out, "spaces"), [6, 240, 68, 714]);
    assert_eq!(rect(&out, "rooms"), [80, 6, 292, 896]);
    assert_eq!(rect(&out, "account"), [80, 908, 292, 46]);
    assert_eq!(rect(&out, "header"), [378, 6, 1176, 58]);
    assert_eq!(rect(&out, "timeline"), [378, 70, 1176, 830]);
    assert_eq!(rect(&out, "composer"), [378, 906, 1176, 48]);
    let p = run(&classic, 1560, 960, None, &c, r#"{"roomOpen":true,"panelOpen":true}"#);
    assert_eq!(rect(&p, "panel"), [1230, 70, 324, 884]);
    assert_eq!(rect(&p, "timeline")[2], 846);
    let c8 = run(&classic, 1560, 960, None, &content_with(r#"{"call":{"h":800}}"#), r#"{"roomOpen":true,"callShown":true}"#);
    assert_eq!(rect(&c8, "call")[3], 644);
    let seam = run(&classic, 1560, 960, None, &content_with(r#"{"call":{"h":800}}"#), r#"{"roomOpen":true,"callShown":true,"sizes":{"call":{"pm":420}}}"#);
    assert_eq!(rect(&seam, "call")[3], 393);
    let n = run(&classic, 1100, 800, None, &c, r#"{"roomOpen":true,"panelOpen":true}"#);
    assert_eq!([rect(&n, "panel")[0], rect(&n, "panel")[1], num(&n, &["islands", "panel", "z"])], [770, 70, 2]);
    let d = run(&classic, 700, 800, None, &c, r#"{"roomOpen":true}"#);
    let dr = d.get("drawers").unwrap().at(0).unwrap();
    let r4 = |x: J| ["x", "y", "w", "h"].map(|k| x.get(k).unwrap().int_val().unwrap());
    assert_eq!(r4(dr.get("handle").unwrap()), [6, 6, 40, 40]);
    assert_eq!(r4(dr.get("panel").unwrap()), [6, 50, 340, 656]);
    assert_eq!(num(&d, &["islands", "header", "inset", "l"]), 42);
    let m = run(&classic, 1560, 960, None, &content_with(r#"{"call":{"h":600}}"#), r#"{"roomOpen":true,"callShown":true,"maximized":"call"}"#);
    assert_eq!(m.get("islands").unwrap().keys(), vec!["call".to_string()]);
    assert_eq!(rect(&m, "call"), [6, 6, 1548, 948]);
    let mw = run(&classic, 480, 520, None, &content_with(r#"{"call":{"h":600}}"#), r#"{"roomOpen":true,"callShown":true,"panelOpen":true}"#);
    assert_eq!(["header", "call", "timeline", "composer"].map(|r| rect(&mw, r)[3]), [58, 264, 120, 48]);
    let f = run(
        &internal_desktop("classic-flush").unwrap(),
        1560,
        960,
        Some(&o(r#"{"gap":0,"margin":0}"#)),
        &J::obj(),
        r#"{"roomOpen":true,"forceCollapse":["_nav"],"drawer":{"open":"_nav"}}"#,
    );
    assert_eq!(rect(&f, "_room"), [0, 0, 1560, 960]);
    assert_eq!(num(&f, &["islands", "_room", "inset", "l"]), 46);
    assert_eq!(rect(&f, "_nav"), [12, 56, 324, 664]);
}

#[test]
fn solve_the_emergency_levels_of_a_short_wide_window() {
    let cf = preset_desktop("callFirst").unwrap();
    let content = content_with(r#"{"call":{"h":600,"collapsedH":104,"barH":48}}"#);
    let at = |h: i64| run(&cf, 1200, h, None, &content, r#"{"roomOpen":true,"callShown":true}"#);
    let l3 = at(240);
    assert!(l3.get("emergency").unwrap().is_true() && l3.get("tooSmall").is_none());
    assert_eq!([rect(&l3, "call")[3], rect(&l3, "composer")[3]], [174, 48]);
    assert_eq!([rect(&at(170), "call")[3], rect(&at(170), "composer")[3]], [104, 48]);
    assert_eq!([rect(&at(130), "call")[3], rect(&at(130), "composer")[3]], [64, 48]);
    assert_eq!(rect(&at(100), "call")[3], 88);
    assert!(!at(100).get("islands").unwrap().get("composer").unwrap().get("vis").unwrap().is_true());
    assert!(at(50).get("tooSmall").is_some_and(|t| t.is_true()));
    // the desktop's minimum window always reaches level 3 or better
    let big = content_with(r#"{"header":{"h":110},"composer":{"h":180},"call":{"h":900,"collapsedH":104}}"#);
    for p in PRESET_NAMES {
        let out = run(&preset_desktop(p).unwrap(), 480, 520, None, &big, r#"{"roomOpen":true,"callShown":true,"panelOpen":true}"#);
        let isl = out.get("islands").unwrap();
        let (call, comp) = (isl.get("call").unwrap(), isl.get("composer").unwrap());
        assert!(
            out.get("tooSmall").is_none()
                && call.get("vis").unwrap().is_true()
                && call.get("h").unwrap().int_val().unwrap() >= 104
                && comp.get("vis").unwrap().is_true()
                && comp.get("h").unwrap().int_val().unwrap() >= 48,
            "{p}"
        );
    }
}

fn op(f: &J, text: &str) -> OpResult {
    apply_op(f, Some(&o(text)), None)
}

#[test]
fn operations_never_mutate_the_input_and_follow_the_id_and_size_rules() {
    let f = classic_desktop();
    let before = f.canon();
    let bad = op(&f, r#"{"op":"hide","ref":"timeline"}"#);
    assert_eq!(bad.to_json().canon(), r#"{"error":"bad-op: required island"}"#);
    assert!(op(&f, r#"{"op":"dockBeside","ref":"widgets","target":"panel","side":"bottom"}"#).is_ok());
    assert_eq!(f.canon(), before);
    assert_eq!(op(&f, r#"{"op":"dockBeside","ref":"nav","target":"timeline","side":"left"}"#).to_json().canon(), r#"{"error":"bad-op: locked group"}"#); // R1
    let w = op(&f, r#"{"op":"dockBeside","ref":"nav","target":"rooms","side":"left"}"#);
    let OpResult::Ok { form: wf, .. } = &w else { panic!("ok") };
    assert!(wf.canon().contains(r#""id":"n1""#)); // N8: the smallest free n<k>
    let hid = op(&f, r#"{"op":"hide","ref":"rooms"}"#);
    let OpResult::Ok { form: hf, .. } = &hid else { panic!("ok") };
    assert_eq!(
        hf.get("hidden").unwrap().canon(),
        r#"[{"from":{"index":0,"parent":"roomscol"},"i":"rooms","size":{"auto":true,"grow":1,"min":160},"t":"island"}]"#
    );
    let OpResult::Ok { form: back, .. } = op(hf, r#"{"op":"restore","ref":"rooms"}"#) else { panic!("ok") };
    assert_eq!(back.canon(), before);
}

#[test]
fn normalize_and_repair_are_idempotent_and_a_projection_repairs_to_itself() {
    let mut rnd = Rnd::new(99);
    let mut form = classic_desktop();
    for _ in 0..60 {
        let ctx = random_ctx(&mut rnd, &form);
        let the_op = random_op(&mut rnd, &form, &ctx);
        let OpResult::Ok { form: next, .. } = apply_op(&form, Some(&the_op), Some(&ctx)) else { continue };
        form = next;
        let n = normalize(&form);
        assert_eq!(n.form.canon(), form.canon());
        assert!(n.notes.is_empty(), "{:?}", n.notes);
        assert!(validate_desktop(Some(&form)).is_empty());
    }
    let once = repair_desktop_text(r#"{"root":{"t":"island","i":"timeline"},"hidden":["nav",5]}"#).form;
    let twice = repair_desktop(Some(&once));
    assert_eq!(twice.form.canon(), once.canon());
    assert!(twice.notes.is_empty());
}

#[test]
fn sync_merge_laws_on_generated_documents() {
    let mut rnd = Rnd::new(5);
    let ent = |rnd: &mut Rnd| -> J {
        let id = rnd.pick(&["main", "side", "x"]);
        let by = rnd.pick(&["a", "b", "c"]);
        let rev = rnd.range(0, 3);
        let saved_at = rnd.range(0, 9);
        let t = rnd.int(3) as i64;
        let vv = mk(vec![(by, J::int(rev + 1))]);
        if rnd.chance(200) {
            return mk(vec![("id", J::s(id)), ("rev", J::int(rev)), ("savedAt", J::int(saved_at)), ("by", J::s(by)), ("vv", vv), ("deleted", J::Bool(true))]);
        }
        mk(vec![
            ("id", J::s(id)),
            ("rev", J::int(rev)),
            ("savedAt", J::int(saved_at)),
            ("by", J::s(by)),
            ("vv", vv),
            ("name", J::s("N")),
            ("desktop", mk(vec![("t", J::int(t))])),
            ("phone", J::obj()),
        ])
    };
    let doc_of = |rnd: &mut Rnd, n: i64| -> J {
        let layouts: Vec<J> = (0..n).map(|_| ent(rnd)).collect();
        mk(vec![("format", J::s("tinhat-layouts")), ("v", J::int(1)), ("layouts", J::arr(layouts))])
    };
    // knownOf(d) = Object.fromEntries(d.layouts.map((e) => [e.id, e])) — the last entry per id wins
    let known_of_doc = |d: &J| {
        let k = J::obj();
        for e in d.get("layouts").unwrap().items() {
            k.set(&e.get_str("id").unwrap(), e);
        }
        known_of(Some(&k))
    };
    let m = |raw: &J, known: &J| merge(Some(raw), &known_of_doc(known), None).0;
    let solo = |d: &J| merge(Some(d), &Default::default(), None).0;
    for _ in 0..300 {
        let na = rnd.range(0, 4);
        let a = doc_of(&mut rnd, na);
        let nb = rnd.range(0, 4);
        let b = doc_of(&mut rnd, nb);
        let nc = rnd.range(0, 4);
        let c = doc_of(&mut rnd, nc);
        let ab = m(&a, &solo(&b));
        let ba = m(&b, &solo(&a));
        assert_eq!(ab.get("layouts").unwrap().canon(), ba.get("layouts").unwrap().canon(), "commutative");
        let idem = m(&ab, &ab);
        assert_eq!(idem.canon(), ab.canon(), "idempotent");
        let left = m(&m(&a, &solo(&b)), &solo(&c));
        let right = m(&a, &m(&b, &solo(&c)));
        assert_eq!(left.get("layouts").unwrap().canon(), right.get("layouts").unwrap().canon(), "associative");
    }
}

fn req(id: &str, edit: &J, start: Option<&J>, now: i64, by: &str, seq: i64) -> SaveReq {
    SaveReq {
        id: Some(J::s(id)),
        edit: Some(edit.clone()),
        start: start.cloned(),
        also: None,
        now: Some(J::int(now)),
        by: Some(J::s(by)),
        seq: Some(J::int(seq)),
        key: None,
    }
}

#[test]
fn sync_save_refuses_newer_a_noop_saves_nothing_the_vv_cap_keeps_the_saver() {
    let e = mk(vec![("name", J::s("Main")), ("desktop", classic_desktop()), ("phone", preset_phone("classic").unwrap())]);
    let newer = o(r#"{"format":"tinhat-layouts","v":3,"layouts":[]}"#);
    let none = Default::default();
    assert_eq!(save(Some(&newer), &none, &req("main", &e, None, 1, "a", 1)).to_json().canon(), r#"{"error":"bad-save: newer format"}"#);
    let SaveResult::Saved { entry: first, doc } = save(None, &none, &req("main", &e, None, 1, "a", 1)) else { panic!("saved") };
    assert!(matches!(save(Some(&doc), &none, &req("main", &e, Some(&first), 2, "a", 2)), SaveResult::Noop));
    let vv = J::obj();
    for i in 0..20 {
        vv.set(&format!("d{i}"), J::int(100 + i));
    }
    let start = first.deep();
    start.set("vv", vv);
    let d2 = classic_desktop();
    d2.set("gap", J::int(4));
    let e2 = e.deep();
    e2.set("desktop", d2);
    let SaveResult::Saved { entry: big, .. } = save(None, &none, &req("main", &e2, Some(&start), 3, "zz", 1)) else { panic!("saved") };
    assert_eq!(big.get("vv").unwrap().nkeys(), 16);
    assert_eq!(num(&big, &["vv", "zz"]), 1);
    assert_eq!(project(Some(&doc), Some("nope")).active, "main");
    assert!(project(None, None).layouts.is_empty());
    let mut d = Device::new("a");
    assert_eq!(d.receive(Some(&o(r#"{"format":"tinhat-layouts","v":2,"layouts":[]}"#))), "read-only");
    assert_eq!(d.save(&Some("main".into()), Some(e.clone()), Some(J::int(1))), "read-only");
}

#[test]
fn migrate_nav_mode_and_the_seam_conversion() {
    assert_eq!(seam_pm(200, 1440, 1416, 0), Some(204)); // Astra's case: T 288, pm 204 gives 288
    assert_eq!(fdiv(204 * 1416, 1000), 288);
    let mut l = 1;
    while l <= 1000 {
        for fm in [200, 333, 420, 850] {
            let t = 120.max(fdiv(fm * 960, 1000));
            let pm = seam_pm(fm, 960, l, 0).unwrap();
            let h = fdiv(pm * l, 1000);
            if t >= fdiv(200 * l, 1000) && t <= fdiv(850 * l, 1000) {
                assert_eq!(h, t, "L {l} fm {fm}");
            }
        }
        l += 13;
    }
    assert_eq!(migrate(Some(&o(r#"{"nav_mode":"corner","call-pane-frac":0.42}"#)), None, None, None).canon(), r#"{"forceCollapse":["navcol"],"legacyCallPaneFm":420}"#);
    assert_eq!(migrate(Some(&o(r#"{"call-pane-frac":0.42}"#)), Some(&J::int(960)), Some(&J::int(842)), Some(&J::int(104))).canon(), r#"{"sizes":{"call":{"pm":479}}}"#);
}

/// 200 seeded cases through operations, repair and solve (the reference's own generators, same seeds): every result
/// validates, the island multiset holds (checked by the independent checker when node is there), solves pass I1-I17.
#[test]
fn property_run_200_seeded_cases() {
    let mut ck = node_bin().map(|n| Checker::start(&n));
    let mut solved = 0;
    for c in 0..200u32 {
        let mut rnd = Rnd::new(1000 + c);
        let mut form = preset_desktop(PRESET_NAMES[c as usize % PRESET_NAMES.len()]).unwrap();
        for _ in 0..6 {
            let ctx = random_ctx(&mut rnd, &form);
            let the_op = random_op(&mut rnd, &form, &ctx);
            let res = apply_op(&form, Some(&the_op), Some(&ctx));
            if let Some(ck) = ck.as_mut() {
                let r = res.to_json();
                r.set("ok", J::Bool(res.is_ok()));
                let errs = ck.check(&mk(vec![("kind", J::s("op")), ("before", form.deep()), ("op", the_op.clone()), ("res", r)]));
                assert!(errs.is_empty(), "case {c}: {errs:?}");
            }
            if let OpResult::Ok { form: next, .. } = res {
                assert!(validate_desktop(Some(&next)).is_empty(), "case {c}");
                form = next;
            }
        }
        let ctx = random_ctx(&mut rnd, &form);
        let out = solve(&form, ctx.get("size").as_ref(), ctx.get("unitPm").as_ref(), ctx.get("shape").as_ref(), ctx.get("content").as_ref(), ctx.get("view").as_ref()).to_json();
        if let Some(ck) = ck.as_mut() {
            let input = ctx.deep();
            input.set("form", form.deep());
            let errs = ck.check(&mk(vec![("kind", J::s("solve")), ("input", input), ("out", out)]));
            assert!(errs.is_empty(), "case {c}: {errs:?}");
            solved += 1;
        }
    }
    if ck.is_none() {
        eprintln!("property run: no node — the independent checker was not run (validation only)");
    } else {
        eprintln!("property run: 200 cases, {solved} solves + every operation through the independent checker — 0 violations");
    }
}
