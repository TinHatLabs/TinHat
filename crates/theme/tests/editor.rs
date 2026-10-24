//! THEMES-UI.md §13–§15 + ENGINE.md §10: every shared editor / share-card vector (`testvectors/editor/`, `share/`,
//! resolved with the frozen token list `testvectors/editor-tokens.json`) rebuilds byte for byte from the port — the
//! reference's `rebuild` (web `src/theme/editor.test.ts`, generator `tools/theme-lib/gen-editor-vectors.mjs`), kind for
//! kind — plus the reference's other tests of editor.test.ts (same names, snake_case) and the JSON-text entry points
//! the app calls. The copies equal the contract's: tests/vectors.rs (`copies_equal_the_contract`).

#[allow(unused_imports)] // (common's sha256_hex is for the other test files)
mod common;

use common::{crate_dir, read_json, sorted_files, vec_dir};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use tinhat_theme::editor::{
    apply_edit, apply_edit_json, contrast_issues, contrast_json, contrast_pairs, copy_name, expr_canon,
    export_file_name, export_text, fixes_for, island_opacity, island_opacity_json, is_dirty, new_issues, num_text,
    open_session, op_key, save_doc, session_edit, session_redo, session_reset, session_undo, trim_ascii,
    ContrastIssue, SaveTarget, OPACITY_TOKENS,
};
use tinhat_theme::expr::parse_expr;
use tinhat_theme::labels::{advanced_groups, labels, search_groups, token_label, EditorGroup, TokenInfo};
use tinhat_theme::look::{look_json, look_json_of, read_look, FONTS_MONO, FONTS_SANS, FONTS_SERIF};
use tinhat_theme::probe::{
    pick_list, probe_colour, probe_ids, probe_index, probe_rgb, PickHit, PickKind, PickLayer, PICK_MAX_LAYERS,
};
use tinhat_theme::share::{
    keep_plan, read_share, read_share_json, share_content, share_content_json, SHARE_KEY, SWATCH_ROLES,
};
use tinhat_theme::sync::JObj;
use tinhat_theme::{app_tokens, canon_json, parse_json, pretty_json, resolve_theme, Resolved, TokensSpec};

/// The frozen token list every editor / share vector resolves with (the reference's `SPEC`).
fn spec() -> TokensSpec {
    TokensSpec::from_json(&read_json(&vec_dir().join("editor-tokens.json"))).expect("editor-tokens.json")
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

fn obj(v: &Value) -> &JObj {
    v.as_object().expect("an object")
}

/// `resolvedOut(r)`: `{variant, names, adjusted, roles, tokens}`.
fn resolved_out(r: &Resolved) -> Value {
    json!({
        "variant": r.variant.as_str(),
        "names": r.names.as_str(),
        "adjusted": r.adjusted,
        "roles": r.roles,
        "tokens": r.tokens,
    })
}

fn issues_json(v: &[ContrastIssue]) -> Value {
    Value::Array(v.iter().map(ContrastIssue::to_json).collect())
}

fn layers_of(v: &Value) -> Vec<PickLayer> {
    v.as_array()
        .expect("layers")
        .iter()
        .map(|l| PickLayer {
            element: l["element"].as_str().expect("element").to_string(),
            hits: l["hits"]
                .as_array()
                .expect("hits")
                .iter()
                .map(|h| PickHit {
                    id: h["id"].as_str().expect("id").to_string(),
                    kind: PickKind::parse(h["kind"].as_str().expect("kind")).expect("a pick kind"),
                })
                .collect(),
        })
        .collect()
}

fn hits_json(v: &[PickHit]) -> Value {
    Value::Array(v.iter().map(PickHit::to_json).collect())
}

/// The vector with its `expect` recomputed by the port (the reference's `rebuild`, kind for kind).
fn rebuild(v: &Value, spec: &TokensSpec) -> Value {
    let mut out = v.clone();
    let expect = match v["kind"].as_str().expect("kind") {
        "edit" => match apply_edit(&v["doc"], &v["op"], spec) {
            Ok(r) => json!({ "ok": true, "doc": Value::Object(r.doc), "resolved": resolved_out(&r.resolved) }),
            Err(error) => json!({ "ok": false, "error": error }),
        },
        "opacity" => json!({ "opacity": island_opacity(&v["doc"], spec) }),
        "session" => {
            let mut s = open_session(obj(&v["doc"]));
            let mut log: Vec<String> = Vec::new();
            for st in v["steps"].as_array().expect("steps") {
                let (undo, redo, reset) = (truthy(st.get("undo")), truthy(st.get("redo")), truthy(st.get("reset")));
                if undo || redo || reset {
                    let r = if undo {
                        session_undo(&mut s)
                    } else if redo {
                        session_redo(&mut s)
                    } else {
                        session_reset(&mut s)
                    };
                    log.push(r.as_str().into());
                } else {
                    // `st.gesture ?? null`
                    let gesture = st.get("gesture").filter(|g| !g.is_null()).map(|g| g.as_i64().expect("an integer"));
                    match session_edit(&mut s, &st["edit"], spec, gesture) {
                        Ok(r) => log.push(r.as_str().into()),
                        Err(e) => log.push(e),
                    }
                }
            }
            json!({
                "log": log,
                "doc": Value::Object(s.doc.clone()),
                "past": s.past.len(),
                "future": s.future.len(),
                "dirty": is_dirty(&s),
            })
        }
        "contrast" => {
            let r = resolve_theme(&v["doc"], spec).expect("the doc resolves");
            let issues = contrast_issues(&r);
            let base = if truthy(v.get("base")) {
                contrast_issues(&resolve_theme(&v["base"], spec).expect("the base resolves"))
            } else {
                Vec::new()
            };
            let fresh = new_issues(&issues, &base);
            json!({ "issues": issues_json(&issues), "new": issues_json(&fresh), "fixes": fixes_for(&fresh, &r) })
        }
        "pairs" => {
            let ids: Vec<&str> = v["tokens"].as_array().expect("tokens").iter().map(|t| t.as_str().unwrap()).collect();
            let pairs: Vec<Value> = contrast_pairs(&ids).into_iter().map(|(f, b, need)| json!([f, b, need])).collect();
            json!({ "pairs": pairs })
        }
        "look" => read_look(&v["theme"]).to_value(),
        "save" => {
            let doc = obj(&v["doc"]);
            let target = match v["target"]["kind"].as_str() {
                Some("mine") => SaveTarget::Mine(v["target"]["id"].as_str().expect("target id")),
                _ => SaveTarget::New,
            };
            let taken: Vec<&str> = v["taken"].as_array().expect("taken").iter().map(|t| t.as_str().unwrap()).collect();
            json!({
                "doc": Value::Object(save_doc(doc, target, |id| taken.contains(&id))),
                "fileName": export_file_name(doc),
                "exportText": export_text(doc),
                "copyName": copy_name(doc["name"].as_str().expect("name")),
            })
        }
        "probe" => {
            let ids = probe_ids(spec).expect("probe ids");
            let decode: Vec<Value> = v["expect"]["decode"]
                .as_array()
                .expect("decode")
                .iter()
                .map(|d| {
                    let c = |i: usize| u8::try_from(d["rgb"][i].as_u64().expect("channel")).expect("a byte");
                    let index = probe_index(c(0), c(1), c(2), ids.len()).map_or(-1, |i| i as i64);
                    json!({ "rgb": d["rgb"], "expect": index })
                })
                .collect();
            let colours: Vec<String> = (0..ids.len()).map(probe_colour).collect();
            json!({ "ids": ids, "colours": colours, "decode": decode })
        }
        "picklist" => {
            let layers = layers_of(&v["layers"]);
            let max = v["max"].as_u64().expect("max") as usize;
            // `pickList(v.layers, v.max)` and `pickList(v.layers)` (the default max)
            let all = pick_list(&layers, PICK_MAX_LAYERS);
            json!({ "list": hits_json(&pick_list(&layers, max)), "all": hits_json(&all) })
        }
        "labels" => {
            // labels.ts: `labels` (tokenLabel per id), `groups` (advancedGroups), or a search (searchGroups → ids)
            let l = labels();
            if let Some(ids) = v.get("ids") {
                let out: Vec<String> = ids.as_array().expect("ids").iter().map(|x| token_label(x.as_str().expect("id"), l)).collect();
                json!({ "labels": out })
            } else {
                let infos: Vec<TokenInfo> = v["tokens"]
                    .as_array()
                    .expect("tokens")
                    .iter()
                    .map(|t| TokenInfo {
                        id: t["id"].as_str().expect("id").to_string(),
                        area: t["area"].as_str().expect("area").to_string(),
                        description: t["description"].as_str().unwrap_or("").to_string(),
                    })
                    .collect();
                let groups = advanced_groups(v["hasPalette"].as_bool().expect("hasPalette"), &infos, l);
                match v.get("query") {
                    None => json!({ "groups": groups.iter().map(EditorGroup::to_json).collect::<Vec<_>>() }),
                    Some(q) => {
                        let desc: HashMap<&str, &str> = infos.iter().map(|t| (t.id.as_str(), t.description.as_str())).collect();
                        let kept = search_groups(&groups, q.as_str().expect("query"), |id| desc.get(id).map(|d| d.to_string()).unwrap_or_default());
                        json!({ "groups": kept.iter().map(|g| json!({ "id": g.id, "rows": g.rows.iter().map(|r| r.id.clone()).collect::<Vec<_>>() })).collect::<Vec<_>>() })
                    }
                }
            }
        }
        "encode" => match share_content(&v["theme"], spec) {
            Ok(content) => json!({ "ok": true, "content": Value::Object(content) }),
            Err(error) => json!({ "ok": false, "error": error }),
        },
        "decode" => read_share(&v["content"], spec).to_json(),
        "keep" => {
            // `new Map(v.library.map((t) => [t.id, t]))`: a later entry with the same id wins
            let lib: HashMap<String, Value> = v["library"]
                .as_array()
                .expect("library")
                .iter()
                .map(|t| (t["id"].as_str().expect("library id").to_string(), t.clone()))
                .collect();
            let mine: Vec<JObj> = v["mine"].as_array().expect("mine").iter().map(|m| obj(m).clone()).collect();
            keep_plan(obj(&v["theme"]), |id| lib.get(id), &mine).to_json()
        }
        k => panic!("unknown vector kind {k}"),
    };
    out["expect"] = expect;
    out
}

/// The first line where two texts differ (for readable failures).
fn first_diff(want: &str, got: &str) -> String {
    for (i, (a, b)) in want.lines().zip(got.lines()).enumerate() {
        if a != b {
            return format!("line {}: want {a:?}, got {b:?}", i + 1);
        }
    }
    format!("lengths differ: want {} lines, got {}", want.lines().count(), got.lines().count())
}

/// `${family}: ${files.length} vectors rebuild byte for byte`
fn vectors_rebuild_byte_for_byte(family: &str) -> usize {
    let spec = spec();
    let files: Vec<_> = sorted_files(&vec_dir().join(family))
        .into_iter()
        .filter(|f| f.extension().is_some_and(|e| e == "json"))
        .collect();
    assert!(files.len() > 20, "vectors present");
    let mut bad = Vec::new();
    for f in &files {
        let name = f.file_name().unwrap().to_string_lossy().to_string();
        let text = fs::read_to_string(f).expect("read vector");
        let v = parse_json(&text).expect("vector is JSON");
        let kind = v["kind"].as_str().expect("kind");
        assert!(name.starts_with(&format!("{kind}-")), "{name}: kind prefix");
        let again = pretty_json(&rebuild(&v, &spec));
        if again != text {
            bad.push(format!("{family}/{name}: {}", first_diff(&text, &again)));
        }
    }
    assert!(bad.is_empty(), "{} of {} {family} vectors differ:\n{}", bad.len(), files.len(), bad.join("\n"));
    eprintln!("{family}: {} vectors rebuild byte for byte", files.len());
    files.len()
}

#[test]
fn editor_vectors_rebuild_byte_for_byte() {
    vectors_rebuild_byte_for_byte("editor");
}

#[test]
fn share_vectors_rebuild_byte_for_byte() {
    vectors_rebuild_byte_for_byte("share");
}

#[test]
fn apply_edit_never_mutates_its_input_and_never_throws_on_junk() {
    let spec = spec();
    let doc = read_json(&vec_dir().join("editor/edit-accent-oled.json"))["doc"].clone();
    let before = canon_json(&doc);
    let ops = [
        json!({ "op": "accent", "value": "#123456" }),
        json!({ "op": "background", "value": "#abcdef" }),
        json!({ "op": "opacity", "value": 50 }),
        json!({ "op": "token", "id": "composer.bg", "value": "#010203" }),
        json!({ "op": "shape", "key": "radius", "value": 3 }),
        json!(null),
        json!([]),
        json!({ "op": "role" }),
        json!({ "op": "token", "id": 5, "value": "#000" }),
        json!({ "op": "font", "slot": "ui", "value": 3 }),
        json!({ "op": "opacity", "value": "50" }),
    ];
    for op in &ops {
        // a result or a refusal, never a panic
        let r = apply_edit(&doc, op, &spec);
        assert!(r.is_ok() || r.is_err());
    }
    assert_eq!(canon_json(&doc), before);
    // (the junk ones are refusals, the first five apply)
    let oks: Vec<bool> = ops.iter().map(|op| apply_edit(&doc, op, &spec).is_ok()).collect();
    assert_eq!(oks, [true, true, true, true, true, false, false, false, false, false, false]);
}

#[test]
fn every_document_apply_edit_returns_resolves() {
    let spec = spec();
    let mut n = 0;
    for f in sorted_files(&vec_dir().join("editor")) {
        let name = f.file_name().unwrap().to_string_lossy().to_string();
        if !name.starts_with("edit-") {
            continue;
        }
        let v = read_json(&f);
        if v["expect"]["ok"] == json!(true) {
            assert!(resolve_theme(&v["expect"]["doc"], &spec).is_ok(), "{name}");
            // and the port's own result is that document
            let r = apply_edit(&v["doc"], &v["op"], &spec).expect(&name);
            assert!(resolve_theme(&Value::Object(r.doc), &spec).is_ok(), "{name}");
            n += 1;
        }
    }
    assert!(n > 40, "{n} successful edits");
}

#[test]
fn expr_canon_num_text_print_like_the_contract() {
    assert_eq!(num_text(8500), "0.85");
    assert_eq!(num_text(10000), "1");
    assert_eq!(num_text(11800), "1.18");
    assert_eq!(num_text(1), "0.0001");
    assert_eq!(num_text(99999999), "9999.9999");
    assert_eq!(trim_ascii(" \t\r\n x y \n"), "x y");
    assert_eq!(trim_ascii("\u{a0}x\u{a0}"), "\u{a0}x\u{a0}", "only ASCII whitespace is trimmed");
    // exprCanon: literals canonical, ", " between arguments, the number through numText
    let canon = |s: &str| expr_canon(&parse_expr(s).expect(s));
    assert_eq!(canon(" alpha( rail.bg ,0.850) "), "alpha(rail.bg, 0.85)");
    assert_eq!(canon("mix(#FFF,c1,.5)"), "mix(#ffffff, c1, 0.5)");
    assert_eq!(canon("guard(text.primary, surface.island, 21)"), "guard(text.primary, surface.island, 21)");
    assert_eq!(canon("best(a,b,#000000FF)"), "best(a, b, #000000)");
    assert_eq!(canon("lighter(darker(x, 1.0), 9999.9999)"), "lighter(darker(x, 1), 9999.9999)");
    assert_eq!(canon("#11223380"), "#112233/0.502");
}

#[test]
fn look_the_allowed_font_lists_are_disjoint_and_look_json_is_canonical() {
    let all: Vec<&str> = FONTS_SANS.iter().chain(FONTS_SERIF.iter()).chain(FONTS_MONO.iter()).copied().collect();
    let set: std::collections::HashSet<&str> = all.iter().copied().collect();
    assert_eq!(set.len(), all.len());
    let l = read_look(&json!({ "type": { "ui": "Inter", "code": "Hack" }, "shape": { "shadow": 4 } }));
    assert_eq!(look_json(&l), canon_json(&l.to_value()));
    let want = r#"{"density":"normal","fonts":{"chat":null,"code":"Hack","ui":"Inter"},"shadow":4,"size":100}"#;
    assert_eq!(look_json(&l), want);
}

#[test]
fn share_a_card_of_every_library_sized_theme_round_trips_swatches_are_the_listed_roles() {
    let spec = spec();
    let lib = read_json(&crate_dir().join("themes-library.json"));
    let mut n = 0;
    for t in lib["themes"].as_array().expect("themes").iter().take(120) {
        let id = t["id"].as_str().unwrap_or_default();
        let c = share_content(t, &spec).unwrap_or_else(|e| panic!("{id}: {e}"));
        let body = c["body"].as_str().expect("body");
        assert!(!(body.contains("http:") || body.contains("https:") || body.contains("www.")), "no URL in the body");
        let r = read_share(&parse_json(&canon_json(&Value::Object(c))).unwrap(), &spec);
        assert_eq!(r.kind(), "ok", "{id}");
        let tinhat_theme::share::ShareRead::Ok { theme, swatches, .. } = r else { unreachable!() };
        assert_eq!(canon_json(&Value::Object(theme)), canon_json(t));
        assert_eq!(swatches.len(), SWATCH_ROLES.len());
        n += 1;
    }
    assert!(n > 100);
    assert_eq!(read_share(&json!({ "body": "x", SHARE_KEY: null }), &spec).kind(), "none");
}

#[test]
fn probe_every_index_round_trips_a_probe_colour_never_decodes_to_another_index() {
    let n = probe_ids(&spec()).expect("probe ids").len();
    for i in 0..4095 {
        let (r, g, b) = probe_rgb(i);
        assert_eq!(probe_index(r, g, b, 4095), Some(i));
        assert_eq!(probe_index(r, g, b, n), if i < n { Some(i) } else { None });
    }
}

// ---- the port's own: history keys and the JSON-text entry points the app calls ------------------------------------

#[test]
fn op_keys_join_the_op_and_its_target() {
    let k = |v: Value| op_key(obj(&v));
    assert_eq!(k(json!({ "op": "palette", "slot": "bg", "value": "#000000" })), "palette:bg");
    assert_eq!(k(json!({ "op": "role", "id": "text.primary", "value": null })), "role:text.primary");
    assert_eq!(k(json!({ "op": "shape", "key": "radius", "value": 4 })), "shape:radius");
    assert_eq!(k(json!({ "op": "opacity", "value": 80 })), "opacity:");
    // `slot ?? id ?? key ?? ""` and JavaScript's String()
    assert_eq!(k(json!({ "op": "x", "slot": null, "id": 1.5 })), "x:1.5");
    assert_eq!(k(json!({ "slot": [1, null, "a"] })), "undefined:1,,a");
    assert_eq!(k(json!({ "op": null, "key": {} })), "null:[object Object]");
}

#[test]
fn json_entry_points_use_the_app_token_list() {
    let lib = read_json(&crate_dir().join("themes-library.json"));
    let nord = lib["themes"].as_array().unwrap().iter().find(|t| t["id"] == "nord").unwrap().clone();
    let nord_text = canon_json(&nord);
    let op = r#"{"op":"opacity","value":85}"#;
    let r = parse_json(&apply_edit_json(&nord_text, op)).unwrap();
    let want = apply_edit(&nord, &parse_json(op).unwrap(), app_tokens()).unwrap();
    assert_eq!(r["ok"], json!(true));
    assert_eq!(canon_json(&r["doc"]), canon_json(&Value::Object(want.doc.clone())));
    assert_eq!(r["resolved"]["tokens"].as_object().unwrap().len(), app_tokens().ids().len());
    assert!(r.get("resolved").and_then(|x| x.get("ok")).is_none());
    assert_eq!(island_opacity_json(&canon_json(&r["doc"])), r#"{"opacity":85}"#);
    assert_eq!(island_opacity_json(&nord_text), r#"{"opacity":100}"#);
    for id in OPACITY_TOKENS {
        assert!(app_tokens().ids().iter().any(|t| t == id), "{id} is an app token");
    }
    assert_eq!(apply_edit_json("not json", op), r#"{"error":"bad-op","ok":false}"#);
    assert_eq!(apply_edit_json(&nord_text, r#"{"op":"size","value":79}"#), r#"{"error":"out-of-range","ok":false}"#);

    // readability: an edit that breaks a pair shows up as new, with its fix (a palette theme's roles are guarded by
    // the engine; a token override is not)
    let broken = apply_edit(&nord, &json!({ "op": "token", "id": "toast.fg", "value": "#3b4252" }), app_tokens())
        .unwrap()
        .doc;
    let c = parse_json(&contrast_json(&canon_json(&Value::Object(broken)), &nord_text)).unwrap();
    assert!(c["new"].as_array().unwrap().iter().any(|i| i["fg"] == "toast.fg" && i["bg"] == "toast.bg"));
    assert!(c["fixes"].as_array().unwrap().iter().any(|f| f["id"] == "toast.fg" && f["op"] == "token"));
    let same = parse_json(&contrast_json(&nord_text, &nord_text)).unwrap();
    assert_eq!(same["new"], json!([]));
    assert_eq!(same["fixes"], json!([]));
    assert_eq!(parse_json(&contrast_json(&nord_text, "")).unwrap()["issues"], same["issues"]);
    assert!(parse_json(&contrast_json("[]", "")).unwrap()["error"].as_str().unwrap().starts_with("refused: "));

    // share cards
    let card = share_content_json(&nord_text);
    let content = parse_json(&card).unwrap()["content"].clone();
    let read = parse_json(&read_share_json(&canon_json(&content))).unwrap();
    assert_eq!(read["kind"], json!("ok"));
    assert_eq!(read["name"], json!("Nord"));
    assert_eq!(read["swatches"].as_array().unwrap().len(), SWATCH_ROLES.len());
    assert_eq!(read_share_json("{"), r#"{"kind":"none"}"#);
    assert_eq!(share_content_json("{"), r#"{"error":"refused: bad-theme: not JSON data","ok":false}"#);

    // the look
    assert_eq!(look_json_of(r#"{"type":{"size":120}}"#), look_json(&read_look(&json!({ "type": { "size": 120 } }))));
    assert_eq!(look_json_of("nope"), look_json(&read_look(&Value::Null)));
}

#[test]
fn a_session_edit_that_is_refused_changes_nothing() {
    let spec = spec();
    let doc = read_json(&vec_dir().join("editor/edit-accent-oled.json"))["doc"].clone();
    let mut s = open_session(obj(&doc));
    let before = s.clone();
    assert_eq!(session_edit(&mut s, &json!({ "op": "size", "value": 1 }), &spec, Some(1)), Err("out-of-range".into()));
    assert_eq!(s, before);
    assert_eq!(session_undo(&mut s).as_str(), "noop");
    assert_eq!(session_redo(&mut s).as_str(), "noop");
    assert_eq!(session_reset(&mut s).as_str(), "noop");
    assert_eq!(s, before);
}

#[test]
fn labels_every_area_of_tokens_json_is_in_a_section_every_role_has_a_label() {
    let l = labels();
    let tokens = read_json(&crate_dir().join("tokens.json"));
    let listed: Vec<&str> = l.sections.iter().flat_map(|(_, a)| a.iter().map(String::as_str)).collect();
    let infos: Vec<TokenInfo> = tokens["tokens"]
        .as_array()
        .expect("tokens")
        .iter()
        .map(|t| TokenInfo {
            id: t["id"].as_str().expect("id").to_string(),
            area: t["area"].as_str().expect("area").to_string(),
            description: String::new(),
        })
        .collect();
    for t in &infos {
        assert!(listed.contains(&t.area.as_str()), "area {} of {} is in no section", t.area, t.id);
    }
    for r in tokens["roles"].as_array().expect("roles") {
        assert!(l.roles.contains_key(r["id"].as_str().expect("role id")), "{r}");
    }
    let groups = advanced_groups(true, &infos, l);
    let rows: usize = groups.iter().map(|g| g.rows.len()).sum();
    assert_eq!(rows, 21 + tokens["roles"].as_array().expect("roles").len() + infos.len());
    assert!(groups.iter().all(|g| g.section != "Other"));
}
