//! Differential cases against the TypeScript reference: `tests/data/reference-fuzz.json`, written by
//! `tests/data/gen-reference-fuzz.mjs` from the web's `src/theme` (deterministic pseudo-random inputs + the reference's
//! exact outputs; how to regenerate: the generator's header). Every case must match exactly: expressions, literals,
//! numbers, mutated and overridden themes (replayed from the same PRNG where the file only records the seed),
//! `resolve_for_app` on edited built-ins, generated files of every import format, and theme JSON text
//! (`parse_theme_json`). Keys named like `Object.prototype` members, 125–131 nesting levels and lone-surrogate escapes
//! run through all of them (ENGINE.md §9). The document parsers are private; their cases are replayed by
//! `parsers::tests::reference_fuzz`.
//!
//! `THEME_REFERENCE_FUZZ=<file>` replays another corpus (e.g. a big one-off run with `--scale 20`).

mod common;

use common::{crate_dir, sha256_hex, Mulberry32};
use serde_json::{json, Map, Value};
use std::path::PathBuf;
use tinhat_theme::canon::js_number;
use tinhat_theme::expr::{parse_expr, Expr, FnName};
use tinhat_theme::literal::parse_decimal4;
use tinhat_theme::roles::ROLE_IDS;
use tinhat_theme::{
    app_result_to_json, builtin_theme, canon_json, format_colour, import_result_to_json, import_theme, parse_json,
    parse_literal, parse_theme_json, resolve_for_app, resolve_theme, result_to_json, App, TokensSpec,
};

fn corpus() -> Value {
    let path = std::env::var("THEME_REFERENCE_FUZZ")
        .map(PathBuf::from)
        .unwrap_or_else(|_| crate_dir().join("tests/data/reference-fuzz.json"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    parse_json(&text).expect("reference-fuzz.json is JSON")
}

fn cases<'a>(doc: &'a Value, section: &str) -> &'a Vec<Value> {
    doc[section].as_array().unwrap_or_else(|| panic!("reference-fuzz.json: no section {section}"))
}

/// JavaScript `s.length` (UTF-16 code units).
fn js_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// The generator's `out`: canonical compact JSON, or "sha256:" + 24 hex digits when longer than 160 UTF-16 units.
fn out(v: &Value) -> String {
    let s = canon_json(v);
    if js_len(&s) > 160 {
        format!("sha256:{}", &sha256_hex(s.as_bytes())[..24])
    } else {
        s
    }
}

/// The library's themes in the reference's order (`LIB.themes`, sorted by id).
fn library() -> Vec<Value> {
    let text = std::fs::read_to_string(crate_dir().join("themes-library.json")).expect("themes-library.json");
    let lib = parse_json(&text).expect("library is JSON");
    lib["themes"].as_array().expect("themes").clone()
}

/// Collects mismatches; fails with the first few once the section is done.
struct Check {
    section: &'static str,
    n: usize,
    bad: Vec<String>,
}

impl Check {
    fn new(section: &'static str) -> Check {
        Check { section, n: 0, bad: Vec::new() }
    }
    fn eq(&mut self, what: impl FnOnce() -> String, got: &str, want: &str) {
        self.n += 1;
        if got != want {
            self.bad.push(format!("{}\n    want {want}\n    got  {got}", what()));
        }
    }
    fn done(self) {
        let shown: Vec<&String> = self.bad.iter().take(12).collect();
        assert!(
            self.bad.is_empty(),
            "{}: {} of {} cases differ from the reference:\n  {}",
            self.section,
            self.bad.len(),
            self.n,
            shown.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n  ")
        );
        eprintln!("{}: {}/{} identical to the reference", self.section, self.n, self.n);
    }
}

/// `exprText(e, formatColour)` of the reference: the parsed form, numbers printed like JavaScript.
fn expr_text(e: &Expr) -> String {
    match e {
        Expr::Lit(c) => format_colour(*c),
        Expr::Ref(id) => id.clone(),
        Expr::Fn { f, args, k } => {
            let mut parts: Vec<String> = args.iter().map(expr_text).collect();
            let num = match f {
                FnName::Alpha | FnName::Lighter | FnName::Darker => Some(1),
                FnName::Mix | FnName::Mixok | FnName::Guard => Some(2),
                FnName::Over | FnName::Best => None,
            };
            if let Some(i) = num {
                parts.insert(i, js_number(*k as f64 / 10000.0));
            }
            format!("{}({})", f.as_str(), parts.join(", "))
        }
    }
}

#[test]
fn prng_is_the_references() {
    let doc = corpus();
    let mut r = Mulberry32::new(1);
    for (i, want) in cases(&doc, "rng").iter().enumerate() {
        assert_eq!(r.next().to_bits(), want.as_f64().expect("number").to_bits(), "mulberry32(1) value {i}");
    }
}

#[test]
fn expressions() {
    let doc = corpus();
    let mut c = Check::new("expr");
    for case in cases(&doc, "expr") {
        let src = case[0].as_str().expect("source");
        let got = match parse_expr(src) {
            Ok(e) => format!("ok:{}", expr_text(&e)),
            Err(why) => format!("err:{why}"),
        };
        c.eq(|| format!("{src:?}"), &got, case[1].as_str().expect("result"));
    }
    c.done();
}

#[test]
fn literals_and_numbers() {
    let doc = corpus();
    let mut c = Check::new("literal");
    for case in cases(&doc, "literal") {
        let src = case[0].as_str().expect("source");
        let got = parse_literal(src).map(format_colour);
        let want = case[1].as_str().map(str::to_string);
        c.eq(|| format!("{src:?}"), &format!("{got:?}"), &format!("{want:?}"));
    }
    c.done();
    let mut c = Check::new("decimal");
    for case in cases(&doc, "decimal") {
        let src = case[0].as_str().expect("source");
        let want = case[1].as_i64().expect("k");
        c.eq(|| format!("{src:?}"), &parse_decimal4(src).to_string(), &want.to_string());
    }
    c.done();
}

/// One case of the generator's `mutateCase` (the web unit test's procedure, value semantics), from the same PRNG.
fn mutate_case(r: &mut Mulberry32, lib: &[Value], keys: &[String], junk: &[Value], syn_ids: &[String]) -> Value {
    let mut t = lib[r.below(lib.len())].clone();
    let n = 1 + r.below(3);
    for _ in 0..n {
        let k = keys[r.below(keys.len())].as_str();
        let v = junk[r.below(junk.len())].clone();
        let palette_is_object = t.get("palette").is_some_and(Value::is_object);
        if k == "palette" && palette_is_object && r.next() < 0.5 {
            let slot = ["bg", "fg", "cursor", "ansi", "selectionBg"][r.below(5)];
            t["palette"][slot] = v;
        } else if (k == "roles" || k == "tokens") && r.next() < 0.7 {
            let id = if k == "roles" {
                ROLE_IDS[r.below(ROLE_IDS.len())].to_string()
            } else {
                syn_ids[r.below(syn_ids.len())].clone()
            };
            let mut m = Map::new();
            m.insert(id, if v.is_string() { v } else { json!("#fff") });
            t[k] = Value::Object(m);
        } else {
            t[k] = v;
        }
    }
    t
}

#[test]
fn mutated_themes() {
    let doc = corpus();
    let lib = library();
    let syn = TokensSpec::from_json(&doc["syn"]).expect("synthetic spec");
    let syn_ids: Vec<String> = syn.ids().to_vec();
    // the generator picks by position in the spec's list; ids() is sorted — use the list order
    let syn_list: Vec<String> = doc["syn"]["tokens"]
        .as_array()
        .expect("syn tokens")
        .iter()
        .map(|t| t["id"].as_str().expect("id").to_string())
        .collect();
    assert_eq!(syn_ids.len(), syn_list.len());
    let m = &doc["mutate"];
    let keys: Vec<String> = m["keys"].as_array().expect("keys").iter().map(|k| k.as_str().expect("key").to_string()).collect();
    let junk = m["junk"].as_array().expect("junk");
    let mut r = Mulberry32::new(m["seed"].as_u64().expect("seed") as u32);
    let mut c = Check::new("mutate");
    for (i, want) in m["results"].as_array().expect("results").iter().enumerate() {
        let t = mutate_case(&mut r, &lib, &keys, junk, &syn_list);
        let got = out(&result_to_json(&resolve_theme(&t, &syn)));
        c.eq(|| format!("case {i}: {}", canon_json(&t)), &got, want.as_str().expect("result"));
    }
    c.done();
}

#[test]
fn overridden_themes() {
    let doc = corpus();
    let lib = library();
    let syn = TokensSpec::from_json(&doc["syn"]).expect("synthetic spec");
    let real = tinhat_theme::app_tokens();
    let mut c = Check::new("overrides");
    for case in cases(&doc, "overrides") {
        let mut t = lib[case[0].as_u64().expect("base") as usize].clone();
        for (k, v) in case[1].as_object().expect("patch") {
            t[k.as_str()] = v.clone();
        }
        let spec = if case[2] == "real" { real } else { &syn };
        let got = out(&result_to_json(&resolve_theme(&t, spec)));
        c.eq(|| canon_json(&t), &got, case[3].as_str().expect("result"));
    }
    c.done();
}

/// The generator's `expandPads`: `{"$pad": [unit, n, chunk]}` → an array of strings, `unit` repeated `n` times in
/// chunks of `chunk` (the corpus stays small while a theme reaches the 8 KB boundary exactly); `{"$nest": [n, leaf]}`
/// → `n` nested arrays around `leaf` (the corpus stays shallow while a theme reaches the 128-level limit).
fn expand_pads(v: &Value) -> Value {
    match v {
        Value::Array(a) => Value::Array(a.iter().map(expand_pads).collect()),
        Value::Object(o) => {
            if let Some(Value::Array(p)) = o.get("$pad") {
                let unit = p[0].as_str().expect("unit");
                let (n, chunk) = (p[1].as_u64().expect("n") as usize, p[2].as_u64().expect("chunk") as usize);
                let mut out = Vec::new();
                let mut left = n;
                while left > 0 {
                    let k = chunk.min(left);
                    out.push(Value::String(unit.repeat(k)));
                    left -= k;
                }
                return Value::Array(out);
            }
            if let Some(Value::Array(p)) = o.get("$nest") {
                let mut x = p[1].clone();
                for _ in 0..p[0].as_u64().expect("n") {
                    x = Value::Array(vec![x]);
                }
                return x;
            }
            Value::Object(o.iter().map(|(k, v)| (k.clone(), expand_pads(v))).collect())
        }
        _ => v.clone(),
    }
}

#[test]
fn limits() {
    let doc = corpus();
    let lib = library();
    let syn = TokensSpec::from_json(&doc["syn"]).expect("synthetic spec");
    let mut c = Check::new("limits");
    for case in cases(&doc, "limits") {
        let mut t = lib[case[0].as_u64().expect("base") as usize].clone();
        for (k, v) in expand_pads(&case[1]).as_object().expect("patch") {
            t[k.as_str()] = v.clone();
        }
        let got = out(&result_to_json(&resolve_theme(&t, &syn)));
        c.eq(|| canon_json(&case[1]), &got, case[2].as_str().expect("result"));
    }
    c.done();
}

#[test]
fn theme_json_text() {
    let doc = corpus();
    let syn = TokensSpec::from_json(&doc["syn"]).expect("synthetic spec");
    let mut c = Check::new("themeJson");
    for case in cases(&doc, "themeJson") {
        let text = case[0].as_str().expect("text");
        let result = match parse_theme_json(text) {
            Ok(theme) => result_to_json(&resolve_theme(&theme, &syn)),
            Err(e) => e.to_json(),
        };
        c.eq(|| format!("{:?}", &text[..text.len().min(120)]), &out(&result), case[1].as_str().expect("result"));
    }
    c.done();
}

#[test]
fn resolve_for_app_on_edited_builtins() {
    let doc = corpus();
    let mut c = Check::new("app");
    for case in cases(&doc, "app") {
        let mut t = builtin_theme(case[0].as_str().expect("id")).expect("built-in").clone();
        if let Some(p) = case[1].as_object() {
            for (k, v) in p {
                t[k.as_str()] = v.clone();
            }
        }
        let app = case[2].as_str().expect("app");
        let got = out(&app_result_to_json(&App::parse(app).and_then(|a| resolve_for_app(&t, tinhat_theme::app_tokens(), a))));
        c.eq(|| format!("{app} {}", canon_json(&t)), &got, case[3].as_str().expect("result"));
    }
    c.done();
}

#[test]
fn imports() {
    let doc = corpus();
    let mut c = Check::new("import");
    for case in cases(&doc, "import") {
        let text = case[0].as_str().expect("text");
        let hint = case[1].as_str();
        let got = out(&import_result_to_json(&import_theme(text, hint)));
        c.eq(|| format!("{text:?} (hint {hint:?})"), &got, case[2].as_str().expect("result"));
    }
    c.done();
}
