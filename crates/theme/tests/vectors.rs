//! The shared test vectors (contract ENGINE.md §8, ENGINE-QUESTIONS D2), byte for byte: every engine vector and every
//! import vector must reproduce its file exactly — the harness rebuilds each file from the port's result
//! (`canon::pretty_json` = the reference's `prettyJson`) and compares the bytes; a library vector stored as
//! `tokensSha256` is compared through the SHA-256 of the port's own token map. Nothing is skipped or filtered: every
//! file under testvectors/ must belong to a family (engine `lib-` / `edge-` / `app-`, an import sample with its
//! `.expect.json`, `engine-tokens.json`), or the test fails. The vectors are the crate's copy of the contract's
//! `testvectors/` (`sync-vectors.sh`); the last test checks the copies against the contract when it exists
//! (THEME_CONTRACT_DIR; skipped when unset).

mod common;

use common::{crate_dir, sha256_hex, sorted_files, vec_dir, vector_spec};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use tinhat_theme::{
    app_result_to_json, canon_json, import_result_to_json, import_theme, parse_json, parse_theme_json, pretty_json,
    resolve_for_app, resolve_theme, result_to_json, App, ImportFormat, TokensSpec,
};

/// The first line where two texts differ (for readable failures).
fn first_diff(want: &str, got: &str) -> String {
    for (i, (a, b)) in want.lines().zip(got.lines()).enumerate() {
        if a != b {
            return format!("line {}: want {a:?}, got {b:?}", i + 1);
        }
    }
    format!("lengths differ: want {} lines, got {}", want.lines().count(), got.lines().count())
}

/// vector-form.mjs `tokensSha256`: SHA-256 (hex) of the compact canonical JSON of a token map.
fn tokens_sha256(tokens: &Value) -> String {
    sha256_hex(canon_json(tokens).as_bytes())
}

/// vector-form.mjs `vectorExpect`: the result unchanged when stored `full` or refused; else `tokens` replaced by
/// `tokensSha256`.
fn vector_expect(mut result: Value, full: bool) -> Value {
    if full || result["ok"] != json!(true) {
        return result;
    }
    let o = result.as_object_mut().expect("result object");
    let tokens = o.remove("tokens").expect("tokens");
    o.insert("tokensSha256".into(), Value::String(tokens_sha256(&tokens)));
    result
}

/// One engine vector rebuilt from the port's result: `{ ...v, expect: vectorExpect(result, full) }` where the result
/// is `resolveForApp(theme, spec, app)` when the vector has an `app`, else `resolveTheme(theme, spec)`, and the spec is
/// the vector's own `tokensSpec` or the real token list. A vector holds the theme as data (`theme`) or as JSON TEXT
/// (`themeJson`): that is read with `parse_theme_json` first, and a refusal there is the result.
fn engine_vector(text: &str, real: &TokensSpec) -> String {
    let mut v = parse_json(text).expect("vector is JSON");
    let input = match v.get("themeJson") {
        Some(t) => parse_theme_json(t.as_str().expect("themeJson is text")),
        None => Ok(v["theme"].clone()),
    };
    let own = v.get("tokensSpec").filter(|s| !s.is_null()).cloned();
    let spec = match &own {
        Some(s) => TokensSpec::from_json(s),
        None => Ok(real.clone()),
    };
    let result = match (input, v.get("app")) {
        (Err(e), _) => e.to_json(),
        (Ok(theme), Some(app)) => {
            // resolveForApp checks the app first, then the spec, then the theme
            let r = App::parse(app.as_str().unwrap_or("")).and_then(|a| spec.and_then(|s| resolve_for_app(&theme, &s, a)));
            app_result_to_json(&r)
        }
        (Ok(theme), None) => result_to_json(&spec.and_then(|s| resolve_theme(&theme, &s))),
    };
    let full = !v.get("expect").is_some_and(|e| e.get("tokensSha256").is_some());
    v["expect"] = vector_expect(result, full);
    pretty_json(&v)
}

fn walk(dir: &Path, base: &Path, out: &mut Vec<PathBuf>) {
    for p in sorted_files(dir) {
        if p.is_dir() {
            walk(&p, base, out);
        } else {
            out.push(p.strip_prefix(base).unwrap().to_path_buf());
        }
    }
}

/// Every file of testvectors/, relative, sorted.
fn all_vector_files() -> Vec<PathBuf> {
    let mut v = Vec::new();
    walk(&vec_dir(), &vec_dir(), &mut v);
    v
}

#[test]
fn every_vector_file_belongs_to_a_family() {
    let mut families: BTreeMap<&str, usize> = BTreeMap::new();
    let mut stray = Vec::new();
    let files = all_vector_files();
    for f in &files {
        let s = f.to_string_lossy().to_string();
        let parts: Vec<&str> = s.split('/').collect();
        let family = match parts.as_slice() {
            ["engine-tokens.json"] => "engine-tokens",
            ["engine", name] if name.ends_with(".json") && name.starts_with("lib-") => "engine lib",
            ["engine", name] if name.ends_with(".json") && name.starts_with("edge-") => "engine edge",
            ["engine", name] if name.ends_with(".json") && name.starts_with("app-") => "engine app",
            ["import", _, name] if name.ends_with(".expect.json") => {
                let sample = vec_dir().join(&s[..s.len() - ".expect.json".len()]);
                if !sample.is_file() {
                    stray.push(format!("{s} (no sample)"));
                }
                "import expect"
            }
            ["import", _, _] => {
                if !vec_dir().join(format!("{s}.expect.json")).is_file() {
                    stray.push(format!("{s} (no .expect.json)"));
                }
                "import sample"
            }
            // THEMES-SYNC.md §10 (tests/sync.rs)
            ["themes-sync", name] if name.ends_with(".json") => "themes-sync",
            // THEMES-UI.md §12-§14 + ENGINE.md §10 (tests/editor.rs): the editor and the share cards
            ["editor-tokens.json"] => "editor-tokens",
            ["editor", name] if name.ends_with(".json") => "editor",
            ["share", name] if name.ends_with(".json") => "share",
            _ => {
                stray.push(s.clone());
                "stray"
            }
        };
        *families.entry(family).or_default() += 1;
    }
    assert!(stray.is_empty(), "files outside every vector family: {stray:?}");
    assert_eq!(families["import sample"], families["import expect"]);
    assert_eq!(families.values().sum::<usize>(), files.len());
    eprintln!("testvectors/: {} files: {families:?}", files.len());
}

#[test]
fn engine_vectors_byte_for_byte() {
    let real = vector_spec();
    let files: Vec<PathBuf> = sorted_files(&vec_dir().join("engine"));
    let (mut lib, mut lib_digest, mut edge, mut edge_json, mut app) = (0, 0, 0, 0, 0);
    let mut bad = Vec::new();
    for f in &files {
        let name = f.file_name().unwrap().to_string_lossy().to_string();
        let text = fs::read_to_string(f).expect("read vector");
        let digest = text.contains("\"tokensSha256\"");
        match &name[..name.find('-').unwrap_or(0)] {
            "lib" => {
                lib += 1;
                lib_digest += usize::from(digest);
            }
            "edge" => {
                edge += 1;
                edge_json += usize::from(name.starts_with("edge-json-"));
            }
            "app" => app += 1,
            _ => panic!("{name}: not an engine vector family (lib-/edge-/app-)"),
        }
        let again = engine_vector(&text, &real);
        if again != text {
            bad.push(format!("{name}: {}", first_diff(&text, &again)));
        }
    }
    assert!(bad.is_empty(), "{} of {} engine vectors differ:\n{}", bad.len(), files.len(), bad.join("\n"));
    assert_eq!(lib + edge + app, files.len());
    assert!(lib >= 1190 && lib_digest > 1000 && edge >= 100 && app >= 8, "lib {lib} ({lib_digest} digests), edge {edge}, app {app}");
    // the digest does not depend on the order the map was built in
    assert_eq!(
        tokens_sha256(&json!({ "a.b": "#000000", "a.a": "#ffffff/0.5" })),
        tokens_sha256(&json!({ "a.a": "#ffffff/0.5", "a.b": "#000000" }))
    );
    eprintln!(
        "engine vectors: {}/{} byte-identical — library {lib} ({lib_digest} through the SHA-256 token digest, {} with full \
         tokens), edge {edge} ({edge_json} as JSON text), app {app}",
        files.len(),
        files.len(),
        lib - lib_digest
    );
}

/// Every import sample `(format dir, file)`, sorted like the reference's test.
fn samples() -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    for d in sorted_files(&vec_dir().join("import")) {
        let dir = d.file_name().unwrap().to_string_lossy().to_string();
        for f in sorted_files(&d) {
            if !f.to_string_lossy().ends_with(".expect.json") {
                out.push((dir.clone(), f));
            }
        }
    }
    out
}

/// Node's `readFileSync(f, "utf8")`: invalid UTF-8 → U+FFFD (maximal subparts, like from_utf8_lossy).
fn read_text(p: &Path) -> String {
    String::from_utf8_lossy(&fs::read(p).expect("read sample")).into_owned()
}

#[test]
fn import_vectors_byte_for_byte() {
    let all = samples();
    assert!(all.len() >= 50, "only {} import samples", all.len());
    let mut bad = Vec::new();
    for (d, f) in &all {
        let name = f.file_name().unwrap().to_string_lossy().to_string();
        let text = read_text(f);
        let want = fs::read_to_string(format!("{}.expect.json", f.display())).expect("expect file");
        let got = pretty_json(&import_result_to_json(&import_theme(&text, Some(&name))));
        if got != want {
            bad.push(format!("{d}/{name}: {}", first_diff(&want, &got)));
        }
    }
    assert!(bad.is_empty(), "{} of {} import vectors differ:\n{}", bad.len(), all.len(), bad.join("\n"));
    eprintln!("import vectors: {}/{} byte-identical", all.len(), all.len());
}

#[test]
fn import_vectors_detect_their_directory_and_resolve() {
    let spec = vector_spec();
    let mut per_format: std::collections::HashMap<String, usize> = Default::default();
    for (d, f) in samples() {
        let name = f.file_name().unwrap().to_string_lossy().to_string();
        if !name.starts_with("handmade-") {
            *per_format.entry(d.clone()).or_default() += 1;
        }
        let Ok(r) = import_theme(&read_text(&f), Some(&name)) else { continue };
        assert_eq!(r.format.as_str(), d, "{d}/{name}");
        if let Err(e) = resolve_theme(&r.theme, &spec) {
            panic!("{d}/{name}: {e}");
        }
    }
    for fmt in ImportFormat::ALL {
        if fmt != ImportFormat::Tinhat {
            let n = per_format.get(fmt.as_str()).copied().unwrap_or(0);
            assert!(n >= 3, "{}: {n} real samples", fmt.as_str());
        }
    }
}

fn same_bytes(a: &Path, b: &Path) {
    assert!(fs::read(a).expect("read") == fs::read(b).expect("read"), "{} ≠ {} (run sync-vectors.sh)", a.display(), b.display());
}

/// The theme's vector families in the contract's testvectors/ (sync-vectors.sh THEME_VECTORS): the rest of that folder
/// belongs to other crates (layout/, since the layout contract C0 of 2026-09-27).
const THEME_VECTOR_FAMILIES: [&str; 7] =
    ["engine", "import", "themes-sync", "engine-tokens.json", "editor", "share", "editor-tokens.json"];

#[test]
fn copies_equal_the_contract() {
    let contract = PathBuf::from(std::env::var("THEME_CONTRACT_DIR").unwrap_or_default());
    // the embedded token list is the vectors' list in every case
    same_bytes(&crate_dir().join("tokens.json"), &vec_dir().join("engine-tokens.json"));
    if contract.as_os_str().is_empty() {
        eprintln!("skipped: THEME_CONTRACT_DIR is not set (the shared contract directory)");
        return;
    }
    if !contract.join("testvectors").is_dir() {
        eprintln!("skipped: no contract at {}", contract.display());
        return;
    }
    for f in ["engine-tables.json", "tokens.json", "themes-library.json", "editor-labels.json", "builtin/oled.json", "builtin/goose.json"] {
        same_bytes(&crate_dir().join(f), &contract.join(f));
    }
    let mut theirs = Vec::new();
    walk(&contract.join("testvectors"), &contract.join("testvectors"), &mut theirs);
    // the contract's testvectors/ also holds other crates' vectors (layout/ = LAYOUT.md's): compare the theme's own
    theirs.retain(|f| THEME_VECTOR_FAMILIES.iter().any(|t| f.starts_with(t)));
    let mine = all_vector_files();
    assert_eq!(mine, theirs, "the vector file lists differ (run sync-vectors.sh)");
    for f in &mine {
        same_bytes(&vec_dir().join(f), &contract.join("testvectors").join(f));
    }
}
