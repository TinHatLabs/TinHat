//! The Simple palette (palette.rs): the reference's palette.test.ts mirrored test by test (same names, snake_case) —
//! properties over every built-in and every library theme. The shared vectors (`testvectors/editor/palette-*`,
//! `edit-swatch-*`, `edit-group-*`, `session-drag-*`) are checked by tests/editor.rs.
mod common;

use std::collections::HashSet;

use common::{crate_dir, read_json, vec_dir};

use serde_json::{json, Value};
use tinhat_theme::canon::canon_json;
use tinhat_theme::editor::apply_edit;
use tinhat_theme::engine::{resolve_theme, TokensSpec};
use tinhat_theme::literal::{format_colour, parse_literal, Colour};
use tinhat_theme::palette::{palette_view, shift_colour, source_value, PALETTE_GROUPS};

fn spec() -> TokensSpec {
    TokensSpec::from_json(&read_json(&vec_dir().join("editor-tokens.json"))).expect("spec")
}
fn all() -> Vec<Value> {
    let lib = read_json(&crate_dir().join("themes-library.json"));
    let mut out: Vec<Value> = ["oled", "goose"]
        .iter()
        .map(|id| tinhat_theme::engine::builtin_theme(id).expect("builtin").clone())
        .collect();
    out.extend(lib["themes"].as_array().expect("themes").iter().cloned());
    out
}
fn opaque_text(c: &str) -> String {
    let c = parse_literal(c).expect("colour");
    format_colour(Colour { k: 10000, ..c })
}

#[test]
fn every_theme_the_view_reads_only_every_swatch_shows_its_sources_stored_colour_sources_are_unique() {
    let spec = spec();
    for t in all() {
        let before = canon_json(&t);
        let d = t.as_object().expect("object");
        let v = palette_view(d, &spec);
        assert_eq!(canon_json(&t), before);
        let mut seen = HashSet::new();
        for g in &v.groups {
            for sw in &g.swatches {
                assert_eq!(sw.id, sw.sources[0]);
                for s in &sw.sources {
                    assert!(seen.insert(s.clone()), "{}: {s} twice", t["id"]);
                    let sv = source_value(d, s).expect("stored");
                    assert_eq!(format_colour(parse_literal(&sv).expect("colour")), sw.colour, "{}: {s}", t["id"]);
                }
            }
        }
    }
}

#[test]
fn every_theme_setting_a_swatch_or_a_group_to_its_own_colour_changes_nothing_nothing_is_migrated() {
    let spec = spec();
    for t in all() {
        let before = canon_json(&t);
        let has_variant = t.get("variant").is_some();
        for g in palette_view(t.as_object().expect("object"), &spec).groups {
            if g.swatches.is_empty() {
                continue;
            }
            let lead = &g.swatches[0];
            let r = apply_edit(&t, &json!({ "op": "group", "id": g.id, "value": opaque_text(&lead.colour) }), &spec);
            let r = r.unwrap_or_else(|e| panic!("{} {}: {e}", t["id"], g.id));
            if !has_variant {
                assert_eq!(canon_json(&Value::Object(r.doc)), before, "{} {}", t["id"], g.id);
            }
            for sw in &g.swatches {
                let s = apply_edit(&t, &json!({ "op": "swatch", "id": sw.id, "value": opaque_text(&sw.colour) }), &spec);
                let s = s.unwrap_or_else(|e| panic!("{} {}: {e}", t["id"], sw.id));
                if !has_variant {
                    assert_eq!(canon_json(&Value::Object(s.doc)), before, "{} {}", t["id"], sw.id);
                }
            }
        }
    }
}

#[test]
fn every_theme_a_group_move_always_resolves_and_moves_every_swatch_of_the_group() {
    let spec = spec();
    for t in all().into_iter().take(200) {
        for g in &PALETTE_GROUPS {
            match apply_edit(&t, &json!({ "op": "group", "id": g.id, "value": "#336699" }), &spec) {
                Err(e) => assert_eq!(e, "empty-group", "{} {}", t["id"], g.id),
                Ok(r) => {
                    assert!(resolve_theme(&Value::Object(r.doc.clone()), &spec).is_ok());
                    let view = palette_view(t.as_object().expect("object"), &spec);
                    let lead = &view.groups.iter().find(|x| x.id == g.id).expect("group").swatches[0];
                    assert_eq!(&source_value(&r.doc, &lead.id).expect("stored")[..7], "#336699", "{} {}", t["id"], g.id);
                }
            }
        }
    }
}

#[test]
fn shift_colour_a_zero_offset_is_the_colour_itself_l_clamps_alpha_is_kept() {
    let c = Colour { r: 12, g: 34, b: 56, k: 8000 };
    let grey = Colour { r: 128, g: 128, b: 128, k: 8000 };
    assert_eq!(shift_colour(c, 0.0, 0.0, 0.0), c);
    assert_eq!(shift_colour(grey, 2.0, 0.0, 0.0), Colour { r: 255, g: 255, b: 255, k: 8000 });
    assert_eq!(shift_colour(grey, -2.0, 0.0, 0.0), Colour { r: 0, g: 0, b: 0, k: 8000 });
}

#[test]
fn groups_the_table_owns_each_role_at_most_once_and_never_scrim_or_shadow() {
    let ids: Vec<&str> = PALETTE_GROUPS.iter().flat_map(|g| g.roles.iter().copied()).collect();
    assert_eq!(ids.iter().collect::<HashSet<_>>().len(), ids.len());
    assert!(!ids.contains(&"scrim") && !ids.contains(&"shadow"));
}

#[test]
fn json_entry_points_refuse_documents_the_engine_refuses_never_panic() {
    use tinhat_theme::palette::{palette_view_json, usage_counts_json};
    let mut nord = all().into_iter().find(|t| t["id"] == "nord").expect("nord");
    nord["accent"] = json!("bogus");
    let text = nord.to_string();
    assert_eq!(palette_view_json(&text), "{}");
    assert_eq!(usage_counts_json(&text), "{}");
    assert_eq!(palette_view_json("[1]"), "{}");
    assert_eq!(palette_view_json("{"), "{}");
    let goose = tinhat_theme::engine::builtin_theme("goose").expect("goose").to_string();
    assert!(palette_view_json(&goose).contains("\"backgrounds\""));
}
