//! The reference's unit tests, mirrored test by test (web `src/theme/engine.test.ts` and `import.test.ts`, same order,
//! same names), plus the reference's exact outputs on edge inputs (`tests/data/reference-edges.json`, written by
//! `tests/data/gen-reference-edges.mjs` from the web's src/theme + src/ui/color.ts; doubles compared as IEEE-754 bits):
//! cbrt, OKLab both ways, sRGB encode on the thresholds, contrast, mix/mixok/lighter/darker/over/guard/best, the slugify
//! fold, ui/color.ts itself (lighter/darker: Qt 6's 16 → 8-bit step since PORT-QUESTIONS Q5) and `Math.round`
//! (ENGINE-QUESTIONS A4).
//!
//! The vectors themselves (engine, library digests, app, import) and the contract copies: tests/vectors.rs. The
//! document parsers are private: their mirror is `parsers::tests` in src/parsers.rs (incl. the §9 Q1/Q2 parser
//! tests); the JSON rules: `json::tests`.

mod common;

use common::{crate_dir, read_json, sha256_hex, sorted_files, vec_dir, vector_spec, Mulberry32};
use serde_json::{json, Value};
use tinhat_theme::expr::parse_expr;
use tinhat_theme::import::{clean_author, clean_name, import_colour, name_from_file, slugify};
use tinhat_theme::limits::is_mxc_uri;
use tinhat_theme::literal::parse_decimal4;
use tinhat_theme::maths::{
    cbrt, chroma, contrast_of_y, enc_mid_table, from_oklab, linear_to_srgb, luminance, mixok, srgb_to_lin_table,
    srgb_to_linear, tables_sha256_hex, to_oklab,
};
use tinhat_theme::ops::{best_c, cr, darker_c, guard_c, lighter_c, mix_c, over_c, qt_div_257};
use tinhat_theme::roles::{GUARDS, ROLE_IDS};
use tinhat_theme::{
    app_result_to_json, builtin_id, builtin_theme, canon_json, format_colour, import_theme, js_round, parse_json,
    parse_literal, parse_theme_json, resolve_for_app, resolve_for_app_json, resolve_theme, resolve_theme_json,
    result_to_json, App, Colour, TokensSpec,
};

fn lit(s: &str) -> Colour {
    parse_literal(s).unwrap_or_else(|| panic!("literal {s:?}"))
}

fn edges() -> Value {
    read_json(&crate_dir().join("tests/data/reference-edges.json"))
}

/// An IEEE-754 double from the generator's big-endian hex ("0x3ff0000000000000").
fn f(bits: &Value) -> f64 {
    let s = bits.as_str().expect("bits");
    f64::from_bits(u64::from_str_radix(s.trim_start_matches("0x"), 16).expect("hex bits"))
}

fn hex(x: f64) -> String {
    format!("0x{:016x}", x.to_bits())
}

fn library() -> Value {
    read_json(&crate_dir().join("themes-library.json"))
}

fn lib_theme(lib: &Value, id: &str) -> Value {
    lib["themes"].as_array().unwrap().iter().find(|t| t["id"] == id).unwrap_or_else(|| panic!("no {id}")).clone()
}

/// The tokens spec as JSON (the reference's `SPEC`), and parsed.
fn spec_json() -> Value {
    read_json(&vec_dir().join("engine-tokens.json"))
}

fn code(err: &str) -> &str {
    &err[..err.find(':').unwrap_or(err.len())]
}

/// JavaScript truthiness of a JSON value.
fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|x| x != 0.0 && !x.is_nan()),
        Some(Value::String(s)) => !s.is_empty(),
        Some(_) => true,
    }
}

// ---- engine.test.ts ---------------------------------------------------------------------------------------------------

#[test]
fn tables_sizes_order_and_the_f64_checksum_the_ports_verify() {
    let (lin, mid) = (srgb_to_lin_table(), enc_mid_table());
    assert_eq!((lin.len(), mid.len()), (256, 255));
    let mut buf = Vec::with_capacity(8 * 511);
    for x in lin.iter().chain(mid.iter()) {
        buf.extend_from_slice(&x.to_le_bytes());
    }
    assert_eq!(sha256_hex(&buf), tables_sha256_hex());
    for i in 0..255 {
        assert!(lin[i] < mid[i] && mid[i] < lin[i + 1], "table order at {i}");
    }
}

#[test]
fn srgb_encode_decode_exact_round_trip_thresholds_count_entries_below_x() {
    for v in 0..=255u8 {
        assert_eq!(linear_to_srgb(srgb_to_linear(v)), v);
    }
    let mid = enc_mid_table();
    for (i, &m) in mid.iter().enumerate() {
        assert_eq!(linear_to_srgb(m) as usize, i, "a value ON a threshold is not < x");
        assert_eq!(linear_to_srgb(m * (1.0 + 1e-15)) as usize, i + 1);
    }
    assert_eq!(linear_to_srgb(-0.5), 0);
    assert_eq!(linear_to_srgb(7.0), 255);
}

#[test]
fn cbrt_64_newton_steps_are_accurate_and_odd() {
    let mut r = Mulberry32::new(1);
    for _ in 0..2000 {
        let x = r.next() * 2.0;
        let y = cbrt(x);
        assert!((y * y * y - x).abs() <= 1e-15 * x, "cbrt({x})");
    }
    assert_eq!(cbrt(0.0), 0.0);
    assert_eq!(cbrt(-0.125), -cbrt(0.125));
}

fn pick_rgb(r: &mut Mulberry32) -> Colour {
    let (red, green, blue) = (r.below(256) as u8, r.below(256) as u8, r.below(256) as u8);
    Colour::rgb(red, green, blue)
}

#[test]
fn oklab_mixok_x_y_0_is_x_and_mixok_x_y_1_is_y_for_every_grey_and_3000_random_pairs() {
    let mut r = Mulberry32::new(2);
    for v in 0..=255u8 {
        let g = Colour::rgb(v, v, v);
        assert_eq!(mixok(g, Colour::rgb(255, 0, 0), 0.0), g);
    }
    for _ in 0..3000 {
        let x = pick_rgb(&mut r);
        let y = pick_rgb(&mut r);
        assert_eq!(format_colour(mixok(x, y, 0.0)), format_colour(x));
        assert_eq!(format_colour(mixok(x, y, 1.0)), format_colour(y));
    }
    // Ottosson's 10-digit matrices: white is L = 0.9999999935, B = 3.7e-8 (not exactly 1, 0) — fine, it round-trips
    let w = to_oklab(Colour::WHITE);
    assert!((w.l - 1.0).abs() < 1e-7 && w.a.abs() < 1e-7 && w.b.abs() < 1e-7);
}

#[test]
fn contrast_wcag_values() {
    assert_eq!(contrast_of_y(luminance(lit("#ffffff")), luminance(lit("#000000"))), 21.0);
    assert_eq!(cr(lit("#777777"), lit("#777777")), 1.0);
    // a translucent colour is composited over the background first; the background's own alpha is ignored
    assert_eq!(cr(lit("#ffffff/0"), lit("#000000")), 1.0);
    assert_eq!(cr(lit("#ffffff"), lit("#000000/0.5")), 21.0);
}

#[test]
fn literals_forms_alpha_rules_and_the_canonical_text() {
    for (s, want) in [
        ("#ABC", "#aabbcc"),
        ("#1E6A76", "#1e6a76"),
        ("#1e6a76/0.18", "#1e6a76/0.18"),
        ("#000000/.5", "#000000/0.5"),
        ("#000000/1", "#000000"),
        ("#000000/0", "#000000/0"),
        ("#000000/0.0001", "#000000/0.0001"),
        ("#000000/0.1000", "#000000/0.1"),
    ] {
        assert_eq!(format_colour(lit(s)), want, "{s}");
    }
    let reference = edges();
    let alpha_aa = reference["alphaAA"].as_array().expect("alphaAA");
    for aa in 0..256u32 {
        let c = lit(&format!("#102030{aa:02x}"));
        // Math.round((aa / 255) * 10000), and the reference's own alphaK of the same literal
        assert_eq!(c.k as f64, js_round((aa as f64 / 255.0) * 10000.0), "aa={aa}");
        assert_eq!(c.k as i64, alpha_aa[aa as usize].as_i64().unwrap(), "aa={aa}");
    }
    for bad in [
        "", "#", "#12", "#1234", "#12345", "#1234567", "#123456789", "#gggggg", "#123456/", "#123456/1.5", "#123456/0.12345",
        "#123/0.5", "123456", " #123456", "#123456 ", "#12345678/0.5",
    ] {
        assert_eq!(parse_literal(bad), None, "{bad:?}");
    }
    for k in 0..=10000u16 {
        let c = Colour { r: 1, g: 2, b: 3, k };
        assert_eq!(lit(&format_colour(c)).k, k);
    }
    assert_eq!(parse_decimal4("12345"), -1);
    assert_eq!(parse_decimal4("9999.9999"), 99999999);
    assert_eq!(parse_decimal4(".0001"), 1);
    assert_eq!(parse_decimal4("1."), -1);
}

#[test]
fn expressions_grammar_and_the_fixed_error_texts() {
    for s in [
        "bg", "#fff", "#ffffff/0.5", "alpha(accent.base, .18)", " mix ( a , b , 1 ) ", "best(a, b)", "best(a,b,c,d,e)",
        "guard(a, b, 21)", "guard(a, b, 1)", "lighter(x, 9999.9999)", "over(alpha(a, 0.5), mixok(b, c, 0))", "name.12", "x:y-z",
    ] {
        assert!(parse_expr(s).is_ok(), "{s:?}");
    }
    let deep = |n: usize| "alpha(".repeat(n) + "a" + &", 1)".repeat(n);
    for (s, why) in [
        ("".to_string(), "empty expression"),
        ("   ".into(), "empty expression"),
        ("a b".into(), "unexpected 'b'"),
        ("mix(a, b)".into(), "mix() takes 3 arguments"),
        ("best(a)".into(), "best() takes at least 2 arguments"),
        ("alpha(a, b)".into(), "alpha() argument 2 must be a number"),
        ("alpha(0.5, a)".into(), "alpha() argument 1 must be a colour"),
        ("alpha(a, 1.0001)".into(), "alpha() argument 2 out of range"),
        ("guard(a, b, 0.9999)".into(), "guard() argument 3 out of range"),
        ("guard(a, b, 21.0001)".into(), "guard() argument 3 out of range"),
        ("alpha(a, 0.12345)".into(), "bad number '0.12345'"),
        ("alpha(a, 1e2)".into(), "bad number '1e2'"),
        ("alpha(a, -1)".into(), "unexpected '-'"),
        ("#12".into(), "bad colour literal '#12'"),
        ("blend(a, b)".into(), "unknown function 'blend'"),
        ("mix(a, b, 0.5".into(), "unexpected end"),
        ("mix(a, b, 0.5))".into(), "unexpected ')'"),
        ("mix(a,, b, 0.5)".into(), "unexpected ','"),
        ("0.5".into(), "unexpected '0'"),
        ("é".into(), "unexpected 'é'"),
        ("🎨".into(), "unexpected '🎨'"),
        (deep(33), "too deeply nested"),
    ] {
        assert_eq!(parse_expr(&s), Err(why.to_string()), "{s:?}");
    }
    assert!(parse_expr(&deep(32)).is_ok());
}

#[test]
fn backdrop_image_is_mxc_uri_is_exactly_the_c2_pattern_the_only_url_a_theme_may_hold() {
    // the JavaScript regex's answers (reference-edges.json "mxc"): the web test's fixed strings ...
    let reference = edges();
    let mxc = &reference["mxc"];
    for case in mxc["fixed"].as_array().unwrap() {
        let s = case[0].as_str().unwrap();
        assert_eq!(is_mxc_uri(s), case[1].as_bool().unwrap(), "{s:?}");
    }
    // ... and its 20,000 strings from mulberry32(4)
    let alpha: Vec<String> = mxc["alphabet"].as_array().unwrap().iter().map(|a| a.as_str().unwrap().to_string()).collect();
    let answers = mxc["random"].as_str().unwrap().as_bytes();
    let mut r = Mulberry32::new(4);
    for (i, &want) in answers.iter().enumerate() {
        let mut s = if r.next() < 0.7 { String::from("mxc://") } else { String::new() };
        let n = r.below(12);
        for _ in 0..n {
            s.push_str(&alpha[r.below(alpha.len())]);
        }
        assert_eq!(is_mxc_uri(&s), want == b'1', "random string {i}: {s:?}");
    }
    assert_eq!(answers.len(), 20000);
    let spec = vector_spec();
    let base = lib_theme(&library(), "dracula");
    let at = |extra: Value| {
        let mut t = base.clone();
        for (k, v) in extra.as_object().unwrap() {
            t[k.as_str()] = v.clone();
        }
        resolve_theme(&t, &spec)
    };
    assert!(at(json!({ "backdrop": { "image": "mxc://example.com/AbC", "fit": "cover" } })).is_ok());
    assert_eq!(at(json!({ "backdrop": { "image": "https://x.org/a.png" } })).unwrap_err().to_string(), "url: /backdrop/image");
    assert_eq!(at(json!({ "name": "mxc://example.com/AbC" })).unwrap_err().to_string(), "url: /name");
    assert_eq!(at(json!({ "backdrop": { "image": "sunset.png" } })).unwrap_err().to_string(), "bad-field: /backdrop/image");
    assert_eq!(at(json!({ "backdrop": null })).unwrap_err().to_string(), "bad-field: backdrop");
}

#[test]
fn oled_and_goose_keep_todays_colours_exactly_and_are_never_guarded() {
    let today = [
        ("oled", ["#000000", "#0a0a0a", "#1a1a1a", "#e0e0e0", "#aaaaaa", "#222222", "#1e6a76", "#3cbad0", "#ffffff", "#c74848", "#ff6b6b", "#e0a020", "#3ad07a", "#4da3ff", "#e8b923"]),
        ("goose", ["#1a0a1a", "#2a1030", "#351540", "#f0e0f0", "#c8a0c8", "#4a2050", "#b0508c", "#e87cc0", "#ffffff", "#d1506e", "#ff7c9c", "#dfa44c", "#58c88a", "#8fa6ff", "#e8b923"]),
    ];
    let ids = [
        "surface.window", "surface.island", "surface.raised", "text.primary", "text.secondary", "border.subtle", "accent.base",
        "accent.bright", "text.onAccent", "state.danger", "state.dangerBright", "state.warning", "state.success", "state.info",
        "state.aside",
    ];
    let spec_doc = spec_json();
    let spec = vector_spec();
    let reference = edges();
    for (name, p) in today {
        let t = read_json(&crate_dir().join("builtin").join(format!("{name}.json")));
        let mut roles: Vec<&str> = t["roles"].as_object().unwrap().keys().map(String::as_str).collect();
        roles.sort();
        assert_eq!(roles, ROLE_IDS.to_vec(), "{name}: every role defined");
        let real: Vec<&str> = spec_doc["tokens"].as_array().unwrap().iter().map(|x| x["id"].as_str().unwrap()).collect();
        for k in t.get("tokens").and_then(Value::as_object).into_iter().flat_map(|o| o.keys()) {
            assert!(real.contains(&k.as_str()), "{name}: token override {k} is a real token");
        }
        let r = resolve_theme(&t, &spec).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(r.adjusted.is_empty());
        assert_eq!(r.variant.as_str(), "dark");
        assert_eq!(r.names.as_str(), "hash-hsl");
        assert_eq!(r.tokens.len(), real.len());
        for (i, id) in ids.iter().enumerate() {
            assert_eq!(r.roles[*id], p[i], "{name} {id}");
        }
        // derived today by src/ui/themes.ts with color.ts (Qt.lighter / Qt.rgba): the reference's color.ts values
        assert_eq!(r.roles["surface.hover"], reference["builtinHover"][name].as_str().unwrap());
        assert_eq!(r.roles["accent.dim"], format!("{}/0.18", p[6]));
        assert_eq!(r.roles["accent.glow"], format!("{}/0.35", p[6]));
        assert_eq!(r.roles["text.link"], p[7]);
        for (k, v) in t.get("tokens").and_then(Value::as_object).into_iter().flatten() {
            assert_eq!(r.tokens[k], format_colour(lit(v.as_str().unwrap())), "{name} pinned {k}");
        }
    }
}

#[test]
fn tokens_json_its_52_roles_are_the_engines_token_ids_sorted() {
    let tokens = read_json(&crate_dir().join("tokens.json"));
    assert_eq!(tokens["format"], "tinhat-tokens");
    let roles: Vec<&str> = tokens["roles"].as_array().unwrap().iter().map(|r| r["id"].as_str().unwrap()).collect();
    assert_eq!(roles, ROLE_IDS.to_vec());
    let ids: Vec<&str> = tokens["tokens"].as_array().unwrap().iter().map(|t| t["id"].as_str().unwrap()).collect();
    let mut sorted = ids.clone();
    sorted.sort(); // byte order = code-point order
    assert_eq!(ids, sorted);
    assert!(ids.len() > 1000);
}

#[test]
fn resolve_for_app_web_pins_apply_to_the_shipped_builtins_only_desktop_never() {
    let tokens = read_json(&crate_dir().join("tokens.json"));
    let spec = vector_spec();
    let list = tokens["tokens"].as_array().unwrap();
    assert!(list.iter().any(|t| t.get("webPin").is_some()));
    for id in ["oled", "goose"] {
        let b = builtin_theme(id).unwrap();
        assert_eq!(builtin_id(b), Some(id));
        assert_eq!(builtin_id(&b.clone()), Some(id)); // a copy is still the built-in
        let base = resolve_theme(b, &spec).unwrap();
        let desk = resolve_for_app(b, &spec, App::Desktop).unwrap();
        let web = resolve_for_app(b, &spec, App::Web).unwrap();
        let mut want_desk = result_to_json(&Ok(base.clone()));
        want_desk["pinned"] = json!([]);
        assert_eq!(app_result_to_json(&Ok(desk)), want_desk);
        assert_eq!(web.roles, base.roles);
        let pinned: Vec<String> =
            list.iter().filter(|t| t["webPin"].get(id).is_some()).map(|t| t["id"].as_str().unwrap().to_string()).collect();
        assert_eq!(web.pinned, pinned);
        for t in list {
            let tid = t["id"].as_str().unwrap();
            let want = match t["webPin"].get(id).and_then(Value::as_str) {
                Some(pin) => format_colour(lit(pin)),
                None => base.tokens[tid].clone(),
            };
            assert_eq!(web.tokens[tid], want, "{id} {tid}");
        }
        // not the shipped document → never pinned
        let mut changed = b.clone();
        changed["name"] = json!(format!("{} copy", b["name"].as_str().unwrap()));
        assert_eq!(builtin_id(&changed), None);
        assert!(resolve_for_app(&changed, &spec, App::Web).unwrap().pinned.is_empty());
    }
    let dracula = lib_theme(&library(), "dracula");
    let mut want = result_to_json(&resolve_theme(&dracula, &spec));
    want["pinned"] = json!([]);
    assert_eq!(app_result_to_json(&resolve_for_app(&dracula, &spec, App::Web)), want);
    assert_eq!(
        app_result_to_json(&resolve_for_app_json(builtin_theme("oled").unwrap(), &spec_json(), &json!("tv"))),
        json!({ "ok": false, "error": "bad-app: must be desktop or web" })
    );
    let bad_pin = json!({ "tokens": [{ "id": "a.b", "default": "#fff", "webPin": { "oled": "red" } }] });
    assert_eq!(
        result_to_json(&resolve_theme_json(builtin_theme("oled").unwrap(), &bad_pin)),
        json!({ "ok": false, "error": "bad-tokens: a.b: bad webPin" })
    );
}

#[test]
fn mix_is_ui_color_ts_lighter_darker_its_qt_hsv_alpha_kept_to_4_decimals() {
    let r = resolve_theme_json(
        &json!({ "format": "tinhat-theme", "v": 1, "id": "t", "name": "t",
                 "palette": { "bg": "#202020", "fg": "#e0e0e0", "ansi": vec!["#808080"; 16] } }),
        &json!({ "tokens": [{ "id": "a.b", "default": "mix(#1e6a76, #ffffff, 0.3)" },
                            { "id": "a.c", "default": "lighter(alpha(#0a0a0a, 0.1234), 1.18)" }] }),
    )
    .unwrap();
    let reference = edges();
    let color_ts = &reference["colorTsEngine"]; // color.ts mix("#1e6a76", "#ffffff", 0.3), qtLighter("#0a0a0a", 1.18)
    assert_eq!(r.tokens["a.b"], color_ts["mix"].as_str().unwrap());
    assert_eq!(r.tokens["a.c"], format!("{}/0.1234", color_ts["lighter"].as_str().unwrap()));
    assert_eq!(format_colour(lighter_c(lit("#0a0a0a/0.1234"), 1.18)), format!("{}/0.1234", color_ts["lighter"].as_str().unwrap()));
}

#[test]
fn lighter_darker_round_16_to_8_bits_like_qt_6_port_questions_q5() {
    // Qt 6's qt_div_257 (every Qt 6 release) = round(x / 257) for every 16-bit value; Qt 5's formula (color.ts before
    // Q5) rounds x = 257k + 128 (k < 128) up
    let qt5 = |x: u32| (x - (x >> 8) + 0x80) >> 8;
    let mut differs = Vec::new();
    for x in 0..=65535u32 {
        assert_eq!(qt_div_257(x) as f64, js_round(x as f64 / 257.0), "x = {x}");
        if qt5(x) != qt_div_257(x) as u32 {
            differs.push(x);
        }
    }
    assert_eq!(differs, (0..128u32).map(|k| 257 * k + 128).collect::<Vec<_>>());
    // measured on Qt 6.11.2: QColor(c).lighter/darker(round(f × 100)).name() — what the desktop paints
    let qt = [
        ("lighter", "#1e6a76", 1.15, "#227a88"), // OLED memories.play.bg.hover, reel.button.primary.bg.hover (16-bit 8866, 31328, 34874)
        ("lighter", "#00005a", 1.15, "#000067"),
        ("lighter", "#00005a", 1.35, "#000079"),
        ("lighter", "#071487", 2.2, "#374bff"), // V overflows: saturation drained
        ("lighter", "#000006", 1.25, "#000007"),
        ("darker", "#000009", 1.2, "#000007"),
        ("darker", "#00007e", 1.12, "#000070"),
        ("lighter", "#0a0a0a", 1.18, "#0c0c0c"), // OLED surface.hover
        ("lighter", "#2a1030", 1.18, "#321339"), // Goose surface.hover
    ];
    for (func, c, factor, want) in qt {
        let got = if func == "lighter" { lighter_c(lit(c), factor) } else { darker_c(lit(c), factor) };
        assert_eq!(format_colour(got), want, "{func}({c}, {factor})");
    }
    // the engine's lighter/darker ARE ui/color.ts's: the web's run-time colours (avatars, hash-HSL names) round like
    // Qt 6 too (PARITY P67, done with Q5) — the reference's color.ts values for the first and the sixth case
    assert_eq!(edges()["q5"]["colorTs"], json!(["#227a88", "#000007"]));
    // the accepted residual (PORT-QUESTIONS N5): Qt 6.11 computes the 16-bit HSV in integers + float32 where the
    // engine keeps Qt 5's algorithm in doubles (ENGINE.md §3) — Qt 6.11.2 paints #189896 here (16-bit blue 38422, the
    // engine 38421)
    assert_eq!(format_colour(lighter_c(lit("#158482"), 1.15)), "#189895");
}

#[test]
fn library_every_theme_resolves_ids_unique_and_sorted_favourites_present() {
    let lib = library();
    assert_eq!(lib["format"], "tinhat-themes-library");
    let themes = lib["themes"].as_array().unwrap();
    assert_eq!(lib["count"].as_u64().unwrap() as usize, themes.len());
    let ids: Vec<&str> = themes.iter().map(|t| t["id"].as_str().unwrap()).collect();
    let mut sorted = ids.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted, ids, "ids unique and sorted (code-point order)");
    for f in lib["favourites"].as_array().unwrap() {
        assert!(ids.contains(&f.as_str().unwrap()), "{f}");
    }
    let spec = vector_spec();
    for t in themes {
        if let Err(e) = resolve_theme(t, &spec) {
            panic!("{}: {e}", t["id"]);
        }
    }
    for b in ["oled", "goose"] {
        let file = read_json(&crate_dir().join("builtin").join(format!("{b}.json")));
        assert_eq!(canon_json(&lib_theme(&lib, b)), canon_json(&file));
    }
    // port addition: the library copy and the vectors agree (every library theme is its lib-<id> vector's theme)
    for t in themes {
        let v = read_json(&vec_dir().join("engine").join(format!("lib-{}.json", t["id"].as_str().unwrap())));
        assert_eq!(canon_json(&v["theme"]), canon_json(t), "{}", t["id"]);
    }
}

#[test]
fn guard_property_every_guarded_role_of_every_palette_theme_meets_its_contrast_or_is_the_target() {
    let lib = library();
    let mut themes: Vec<Value> = lib["themes"].as_array().unwrap().clone();
    for f in sorted_files(&vec_dir().join("engine")) {
        if f.file_name().unwrap().to_string_lossy().starts_with("edge-") {
            themes.push(read_json(&f)["theme"].clone());
        }
    }
    let none = TokensSpec::empty();
    let mut checked = 0;
    for t in &themes {
        if !t.is_object() || !truthy(t.get("palette")) {
            continue;
        }
        let Ok(r) = resolve_theme(t, &none) else { continue };
        let island = lit(&r.roles["surface.island"]);
        let target = if luminance(island) < 0.18 { "#ffffff" } else { "#000000" };
        for (id, n) in GUARDS {
            let c = lit(&r.roles[id]);
            assert!(cr(c, island) >= n || r.roles[id] == target, "{} {id} {} on {}", t["id"], r.roles[id], r.roles["surface.island"]);
            checked += 1;
        }
    }
    assert!(checked > 20000, "{checked}");
}

#[test]
fn purity_same_input_twice_same_output_input_never_touched() {
    let lib = library();
    let spec = vector_spec();
    for t in lib["themes"].as_array().unwrap().iter().step_by(7) {
        let before = canon_json(t);
        let a = canon_json(&result_to_json(&resolve_theme(t, &spec)));
        let b = canon_json(&result_to_json(&resolve_theme(&parse_json(&before).unwrap(), &spec)));
        assert_eq!(a, b);
        assert_eq!(canon_json(t), before);
    }
}

/// The web test's procedure, with value semantics: the reference re-uses the junk objects across iterations (an
/// object once stored in a theme can be mutated by a later iteration); a JSON value cannot alias, so the Rust test
/// clones. The property is the same: ok or a known refusal code, never a panic. (Exact results for a value-semantics
/// variant of this procedure are replayed against the reference in tests/reference.rs, "mutate".)
#[test]
fn never_panics_4000_mutated_themes_give_ok_or_a_known_refusal_code() {
    let codes = [
        "bad-theme", "too-large", "too-deep", "string-too-long", "url", "bad-format", "bad-version", "bad-id", "bad-name",
        "bad-field", "bad-accent", "bad-key", "too-many-overrides", "bad-palette", "no-colours", "missing-role", "bad-expr",
        "undefined-ref", "cycle", "bad-tokens",
    ];
    let mut r = Mulberry32::new(3);
    let junk = [
        json!(null), json!(1), json!(-2.5), json!(true), json!(""), json!("#fff"), json!("x"), json!("c3"), json!("cursor"),
        json!([]), json!({}), json!(["#000"]), json!({ "a": 1 }), json!("mix(bg, fg, 0.5)"), json!("https://x.y"),
    ];
    let keys = ["format", "v", "id", "name", "palette", "roles", "tokens", "accent", "variant", "names", "author"];
    let lib = library();
    let base = &lib["themes"][123];
    // the synthetic spec of the edge vectors (tools/theme-lib/synthetic-tokens.mjs, copied by the generator)
    let syn_doc = edges()["syn"].clone();
    let syn = TokensSpec::from_json(&syn_doc).unwrap();
    let syn_list: Vec<String> =
        syn_doc["tokens"].as_array().unwrap().iter().map(|t| t["id"].as_str().unwrap().to_string()).collect();
    for _ in 0..4000 {
        let mut t = base.clone();
        let n = 1 + r.below(3);
        for _ in 0..n {
            let k = keys[r.below(keys.len())];
            let v = junk[r.below(junk.len())].clone();
            // `t.palette && typeof t.palette === "object"`: arrays count (a named slot set on an array is invisible)
            let palette_is_js_object = truthy(t.get("palette")) && (t["palette"].is_object() || t["palette"].is_array());
            if k == "palette" && palette_is_js_object && r.next() < 0.5 {
                let slot = ["bg", "fg", "cursor", "ansi"][r.below(4)];
                if t["palette"].is_object() {
                    t["palette"][slot] = v;
                }
            } else if (k == "roles" || k == "tokens") && r.next() < 0.7 {
                let id = if k == "roles" { ROLE_IDS[r.below(ROLE_IDS.len())].to_string() } else { syn_list[r.below(syn_list.len())].clone() };
                t[k] = json!({ id: if v.is_string() { v } else { json!("#fff") } });
            } else {
                t[k] = v;
            }
        }
        if let Err(e) = resolve_theme(&t, &syn) {
            let s = e.to_string();
            assert!(codes.contains(&code(&s)), "{s}");
        }
    }
}

// ---- import.test.ts ---------------------------------------------------------------------------------------------------
// ("import vectors: every sample reproduces its .expect.json byte for byte" and "at least 3 real samples per format;
// detection matches the directory; imports resolve": tests/vectors.rs; "parsers: JSONC, TOML, YAML subsets":
// parsers::tests in src/parsers.rs.)

#[test]
fn import_colour_the_accepted_spellings() {
    for (i, o) in [
        ("#ABC", Some("#aabbcc")),
        ("#A1B2C3", Some("#a1b2c3")),
        ("a1b2c3", Some("#a1b2c3")),
        ("0xA1B2C3", Some("#a1b2c3")),
        (" \"#a1b2c3\" ", Some("#a1b2c3")),
        ("rgb:ff/80/00", Some("#ff8000")),
        ("rgb:f/8/0", Some("#ff8800")),
        ("rgb:ffff/8000/0000", Some("#ff8000")),
        ("rgb:fff/800/000", Some("#ff8000")),
        ("none", None),
        ("red", None),
        ("#a1b2c3d4", None),
        ("#a1b2c", None),
        ("rgb:ff/80", None),
        ("rgb:fffff/0/0", None),
        ("", None),
        ("CellForeground", None),
        ("0xa1b2c3d4", None),
    ] {
        assert_eq!(import_colour(i).as_deref(), o, "{i:?}");
    }
}

#[test]
fn names_authors_ids() {
    assert_eq!(clean_name("  Tokyo\tNight  "), "Tokyo Night");
    assert_eq!(clean_name("Dracula https://draculatheme.com"), "Dracula");
    assert_eq!(clean_name(&"x".repeat(90)), "x".repeat(80));
    assert_eq!(
        clean_author("Tinted Theming (https://github.com/tinted-theming), Ayu Theme (https://github.com/ayu-theme)"),
        "Tinted Theming, Ayu Theme"
    );
    assert_eq!(clean_author("Patel, Nimai <nimai.m.patel@gmail.com>, colors from www.github.com/rexim/gruber-darker-theme"), "Patel, Nimai");
    assert_eq!(
        clean_author("Stephan Boyer (https://github.com/stepchowfun) and Esther Wang (https://github.com/ewang12)"),
        "Stephan Boyer and Esther Wang"
    );
    assert_eq!(clean_author("TheNeverMan (github.com/TheNeverMan)"), "TheNeverMan");
    assert_eq!(clean_author("Wenhan Zhu (Cosmos)"), "Wenhan Zhu (Cosmos)");
    assert_eq!(clean_author("mailto:x@y.z"), "");
    assert_eq!(slugify("Rosé Pine Moon"), "rose-pine-moon");
    assert_eq!(slugify("Dracula+"), "dracula-plus");
    assert_eq!(slugify("iTerm2 Solarized Dark"), "iterm2-solarized-dark");
    assert_eq!(slugify("日本"), "imported");
    assert_eq!(slugify(&("a".repeat(63) + " b")), "a".repeat(63));
    assert_eq!(name_from_file("/x/y/tokyonight_night.Xresources"), "tokyonight night");
    assert_eq!(name_from_file("C:\\themes\\Hopscotch.256"), "Hopscotch.256");
    assert_eq!(name_from_file(".conf"), ".conf");
}

#[test]
fn slugify_folds_latin_1_supplement_and_latin_extended_a_like_nfd_plus_the_special_letters() {
    // want = the reference test's rule (NFD, combining marks removed, lower-cased; special letters), computed by the
    // generator with JavaScript's normalize()
    let reference = edges();
    let fold = reference["fold"].as_array().unwrap();
    assert_eq!(fold.len(), 0x180 - 0xc0);
    for (i, want) in fold.iter().enumerate() {
        let cp = 0xc0 + i as u32;
        let ch = char::from_u32(cp).unwrap();
        let want = want.as_str().unwrap();
        let expect = if want.is_empty() { "x-y".to_string() } else { format!("x{want}y") };
        assert_eq!(slugify(&format!("x{ch}y")), expect, "U+{cp:04x}");
    }
}

#[test]
fn import_theme_never_throws_and_keeps_its_error_codes() {
    let codes = [
        "import-empty", "import-too-large", "unknown-format", "bad-json", "bad-plist", "bad-toml", "bad-yaml", "import-incomplete",
        "import-bad-colour",
    ];
    let big = "x".repeat(262145);
    for s in [
        "", "{", "[", "<plist>", "<?xml?><plist><dict><key>a</key></dict>", "colors:\n  primary: [", "base00: x", "[colors]\nansi = [",
        "palette = 0=#000", "color0 #000", "*color0: #000", big.as_str(), "{\"schemes\": [1]}", "\u{feff}\r\n\r\n",
        "background = \"#000\"\n[colors]\n",
    ] {
        if let Err(e) = import_theme(s, Some("f.txt")) {
            let t = e.to_string();
            assert!(codes.contains(&code(&t)), "{t}");
        }
    }
    // port addition: the size limit is exact
    assert!(import_theme(&"x".repeat(262144), None).unwrap_err().to_string().starts_with("unknown-format"));
    assert_eq!(import_theme(&big, None).unwrap_err().to_string(), "import-too-large: 262145 bytes (max 262144)");
}

// ---- reference-edges.json: the reference's exact outputs on edge inputs, bit for bit ------------------------------------

#[test]
fn reference_edges_cbrt() {
    for case in edges()["cbrt"].as_array().unwrap() {
        assert_eq!(hex(cbrt(f(&case[0]))), case[1].as_str().unwrap(), "cbrt({})", f(&case[0]));
    }
}

#[test]
fn reference_edges_oklab_chroma_luminance() {
    for case in edges()["oklab"].as_array().unwrap() {
        let c = lit(case[0].as_str().unwrap());
        let l = to_oklab(c);
        let got = [hex(l.l), hex(l.a), hex(l.b), hex(chroma(c)), hex(luminance(c))];
        let want: Vec<&str> = case.as_array().unwrap()[1..].iter().map(|x| x.as_str().unwrap()).collect();
        assert_eq!(got.to_vec(), want, "{}", case[0]);
    }
    for case in edges()["fromOklab"].as_array().unwrap() {
        let (r, g, b) = from_oklab(f(&case[0]), f(&case[1]), f(&case[2]));
        assert_eq!(json!([r, g, b]), case[3], "{case}");
    }
}

#[test]
fn reference_edges_srgb_encode() {
    for case in edges()["encode"].as_array().unwrap() {
        assert_eq!(linear_to_srgb(f(&case[0])) as i64, case[1].as_i64().unwrap(), "encode({})", f(&case[0]));
    }
}

#[test]
fn reference_edges_mix_mixok() {
    let e = edges();
    for (section, op) in [("mixok", mixok as fn(Colour, Colour, f64) -> Colour), ("mix", mix_c)] {
        for case in e[section].as_array().unwrap() {
            let got = format_colour(op(lit(case[0].as_str().unwrap()), lit(case[1].as_str().unwrap()), f(&case[2])));
            assert_eq!(got, case[3].as_str().unwrap(), "{section} {case}");
        }
    }
}

#[test]
fn reference_edges_qt_lighter_darker() {
    for case in edges()["qt"].as_array().unwrap() {
        let (c, factor) = (lit(case[0].as_str().unwrap()), f(&case[1]));
        assert_eq!(format_colour(lighter_c(c, factor)), case[2].as_str().unwrap(), "lighter {case}");
        assert_eq!(format_colour(darker_c(c, factor)), case[3].as_str().unwrap(), "darker {case}");
    }
}

#[test]
fn reference_edges_over_cr_guard_best_contrast() {
    let e = edges();
    for case in e["over"].as_array().unwrap() {
        let (a, b) = (lit(case[0].as_str().unwrap()), lit(case[1].as_str().unwrap()));
        assert_eq!(format_colour(over_c(a, b)), case[2].as_str().unwrap(), "over {case}");
        assert_eq!(hex(cr(a, b)), case[3].as_str().unwrap(), "cr {case}");
    }
    for case in e["guard"].as_array().unwrap() {
        let got = guard_c(lit(case[0].as_str().unwrap()), lit(case[1].as_str().unwrap()), f(&case[2]));
        assert_eq!(format_colour(got), case[3].as_str().unwrap(), "guard {case}");
    }
    for case in e["best"].as_array().unwrap() {
        let cands: Vec<Colour> = case[1].as_array().unwrap().iter().map(|c| lit(c.as_str().unwrap())).collect();
        assert_eq!(format_colour(best_c(lit(case[0].as_str().unwrap()), &cands)), case[2].as_str().unwrap(), "best {case}");
    }
    for case in e["contrast"].as_array().unwrap() {
        let got = contrast_of_y(luminance(lit(case[0].as_str().unwrap())), luminance(lit(case[1].as_str().unwrap())));
        assert_eq!(hex(got), case[2].as_str().unwrap(), "contrast {case}");
    }
}

#[test]
fn reference_edges_ui_color_ts() {
    // color.ts on opaque colours: mix(c, #ffffff, min(f, 1)), qtLighter(c, f), qtDarker(c, f) — the engine's mix /
    // lighter / darker must be exactly these pixels (color.ts rounds 16 → 8 bits like Qt 6 since PORT-QUESTIONS Q5)
    for case in edges()["colorTs"].as_array().unwrap() {
        let (c, factor) = (lit(case[0].as_str().unwrap()), f(&case[1]));
        assert_eq!(format_colour(mix_c(c, Colour::WHITE, factor.min(1.0))), case[2].as_str().unwrap(), "mix {case}");
        assert_eq!(format_colour(lighter_c(c, factor)), case[3].as_str().unwrap(), "qtLighter {case}");
        assert_eq!(format_colour(darker_c(c, factor)), case[4].as_str().unwrap(), "qtDarker {case}");
    }
}

#[test]
fn reference_edges_math_round_a4() {
    // A4: for x >= 0 the bits equal Math.round's; for negatives the number (JavaScript gives -0 where the port gives
    // +0; every caller clamps at 0)
    for case in edges()["round"].as_array().unwrap() {
        let (x, want) = (f(&case[0]), f(&case[1]));
        let got = js_round(x);
        if x.is_sign_positive() {
            assert_eq!(hex(got), hex(want), "round({x})");
        } else {
            assert_eq!(got, want, "round({x})");
        }
    }
    // the one value where (x + 0.5).floor() is wrong
    let x = 0.49999999999999994_f64;
    assert_eq!((x + 0.5).floor(), 1.0);
    assert_eq!(js_round(x), 0.0);
}

// ---- rules shared with the reference (ENGINE.md §9, PORT-QUESTIONS Q1–Q3; its tests of the same names) -------------
// The reference's tests also cover what a Rust value cannot hold — a lone surrogate inside a string, a cycle, an
// inherited property of a polluted Object.prototype — and check that it refuses or ignores them with the same texts.

fn nest_arrays(n: usize) -> Value {
    let mut v = json!("x");
    for _ in 0..n {
        v = json!([v]);
    }
    v
}

#[test]
fn not_json_data_nesting_deeper_than_128() {
    let spec = vector_spec();
    let dracula = lib_theme(&library(), "dracula");
    let at = |extra: Value| {
        let mut t = dracula.clone();
        for (k, v) in extra.as_object().unwrap() {
            t[k.as_str()] = v.clone();
        }
        resolve_theme(&t, &spec)
    };
    // the theme object is level 1: 127 arrays under a key reach level 128 (then the §4 limit: too-deep), 128 are 129
    assert_eq!(at(json!({ "nest": nest_arrays(127) })).unwrap_err().to_string(), format!("too-deep: /nest{}", "/0".repeat(15)));
    assert_eq!(at(json!({ "nest": nest_arrays(128) })).unwrap_err().to_string(), "bad-theme: not JSON data");
    assert!(at(json!({ "name": "Dracula 🎨" })).is_ok());
    assert_eq!(resolve_theme(&json!("x"), &spec).unwrap_err().to_string(), "bad-theme: not an object");
}

#[test]
fn parse_theme_json_reads_theme_text_exactly_like_the_reference() {
    let spec = vector_spec();
    let dracula = lib_theme(&library(), "dracula");
    let text = canon_json(&dracula);
    assert_eq!(parse_theme_json(&text).unwrap(), dracula);
    let deep_objects = "{\"a\":".repeat(129) + "1" + &"}".repeat(129);
    for bad in [
        format!("\u{feff}{text}"),
        text[..text.len() - 1].to_string(),
        format!("{text} x"),
        "{} // c".to_string(),
        String::new(),
        "{'a': 1}".to_string(),
        r#"{"a": "\ud800"}"#.to_string(),
        r#"{"\udc00": 1}"#.to_string(),
        r#"["\udc00\ud800"]"#.to_string(),
        "[".repeat(129) + &"]".repeat(129),
        deep_objects,
    ] {
        assert_eq!(parse_theme_json(&bad).unwrap_err().to_string(), "bad-theme: not JSON data", "{}", &bad[..bad.len().min(40)]);
    }
    let root = parse_theme_json(&("[".repeat(128) + &"]".repeat(128))).unwrap();
    assert_eq!(resolve_theme(&root, &spec).unwrap_err().to_string(), "bad-theme: not an object");
    // 1e400: null here, Infinity in the reference — the same result either way
    let v = parse_theme_json(&text.replace("\"v\":1", "\"v\":1e400")).unwrap();
    assert_eq!(resolve_theme(&v, &spec).unwrap_err().to_string(), "bad-version: expected 1");
    let v = parse_theme_json(&text.replace("\"v\":1", "\"v\":1,\"x\":1e400")).unwrap();
    assert_eq!(result_to_json(&resolve_theme(&v, &spec)), result_to_json(&resolve_theme(&dracula, &spec)));
}

#[test]
fn themes_object_prototype_names_are_ordinary_keys() {
    let spec = vector_spec();
    let dracula = lib_theme(&library(), "dracula");
    let base = result_to_json(&resolve_theme(&dracula, &spec));
    let text = canon_json(&dracula);
    let with_keys =
        parse_json(&format!("{{\"__proto__\": {{\"format\": \"no\"}}, \"constructor\": \"x\", \"toString\": \"y\", \"hasOwnProperty\": 1, \"valueOf\": [],{}", &text[1..])).unwrap();
    assert_eq!(result_to_json(&resolve_theme(&with_keys, &spec)), base); // unknown keys, kept and ignored
    let with = |k: &str, v: Value| {
        let mut t = dracula.clone();
        t[k] = v;
        t
    };
    assert_eq!(result_to_json(&resolve_theme(&with("tokens", json!({ "constructor": "#ffffff" })), &spec)), base);
    assert_eq!(result_to_json(&resolve_theme(&with("roles", json!({ "toString": "#ffffff", "valueOf": "mix(bg, fg, 0.5)" })), &spec)), base);
    assert_eq!(resolve_theme(&with("roles", json!({ "__proto__": "#ffffff" })), &spec).unwrap_err().to_string(), "bad-key: /roles/__proto__");
    assert_eq!(
        resolve_theme(&with("palette", json!({ "__proto__": { "bg": "#000000" } })), &spec).unwrap_err().to_string(),
        "bad-palette: bg must be an opaque colour"
    );
    // a token spec may name tokens like Object.prototype members: ordinary ids
    let names = TokensSpec::from_json(&json!({ "tokens": [
        { "id": "constructor", "default": "surface.raised" }, { "id": "toString", "default": "constructor" },
        { "id": "valueOf", "default": "alpha(toString, 0.5)" } ] }))
    .unwrap();
    let r = resolve_theme(&with("tokens", json!({ "constructor": "#123456" })), &names).unwrap();
    assert_eq!(result_to_json(&Ok(r))["tokens"], json!({ "constructor": "#123456", "toString": "#123456", "valueOf": "#123456/0.5" }));
}

#[test]
fn importers_read_own_keys_only_a_crafted_toml_changes_nothing() {
    // nothing in Rust can be inherited or polluted; the observable half of the reference's test: the crafted TOML is
    // refused like any unknown file, a nested __proto__ header is data, and later imports are unaffected
    let err = |t: &str, h: &str| import_theme(t, Some(h)).unwrap_err().to_string();
    assert_eq!(err("[colors]\n[__proto__]\nsystem = \"base24\"\nformat = \"tinhat-theme\"\n", "evil.toml"), "unknown-format: not a theme file this app understands");
    assert_eq!(err("[__proto__.__proto__]\nx = 1\n[colors]\nansi = []\n", "x.toml"), "import-incomplete: missing bg");
    assert_eq!(err("{}", "x.json"), "unknown-format: not a theme file this app understands");
    let base16 = std::fs::read_to_string(vec_dir().join("import/base16/dracula.yaml")).unwrap();
    let no_system: String = base16.lines().filter(|l| !l.starts_with("system:")).map(|l| format!("{l}\n")).collect();
    assert_eq!(import_theme(&no_system, Some("f")).unwrap().format.as_str(), "base16");
}

#[test]
fn yaml_collection_keys_and_continuations_an_import_with_such_junk_equals_the_one_without() {
    // the reference's String() of a null-prototype mapping threw (web d18d688 spells it out like the port's js_string)
    let dracula = std::fs::read_to_string(vec_dir().join("import/alacritty-yaml/Dracula.yml")).unwrap();
    let junk = "junk: {{x: y}: 1, [{p: q}]: 2}\nnote: a\n  {b: c}\n";
    assert_eq!(import_theme(&format!("{dracula}\n{junk}"), Some("Dracula.yml")), import_theme(&dracula, Some("Dracula.yml")));
}

#[test]
fn nesting_import_theme_never_panics_however_deep_the_input() {
    let wrap = |n: usize, o: &str, c: &str| o.repeat(n) + &c.repeat(n);
    let blocks = (0..600).map(|i| " ".repeat(i) + "a:").collect::<Vec<_>>().join("\n") + " x\n";
    for text in [
        format!("[colors]\na = {}", wrap(20000, "[", "]")),
        format!("base00: x\na: {}", wrap(20000, "[", "]")),
        format!("{blocks}base00: x\n"),
        format!("<?xml version=\"1.0\"?><plist><dict><key>a</key>{}</dict></plist>", wrap(10000, "<array>", "</array>")),
        format!("{{\"a\": {}}}", wrap(100000, "[", "]")),
    ] {
        let e = import_theme(&text, Some("deep")).unwrap_err().to_string();
        assert!(["bad-json: ", "bad-toml: ", "bad-yaml: ", "bad-plist: "].iter().any(|p| e.starts_with(p)), "{e}");
    }
}

#[test]
fn lone_surrogates_a_json_escape_that_leaves_one_is_bad_json() {
    let wt = std::fs::read_to_string(vec_dir().join("import/windows-terminal/Dracula.json")).unwrap();
    let named = |name: &str| import_theme(&wt.replace("\"name\": \"Dracula\"", &format!("\"name\": \"{name}\"")), Some("d.json"));
    assert_eq!(named("Dracula \\ud800").unwrap_err().to_string(), "bad-json: not valid JSON");
    assert_eq!(named("Dracula \\udc00\\ud800").unwrap_err().to_string(), "bad-json: not valid JSON");
    assert_eq!(named("Dracula \\ud83c\\udfa8").unwrap().theme["name"], "Dracula 🎨");
    // (raw text cannot hold a lone surrogate in Rust; the reference reads one as U+FFFD, like any UTF-8 transfer)
}
