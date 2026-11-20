//! P22 (12.1.17): the personal text size + density layered over the theme — every shared vector in
//! `testvectors/personal/` (byte-identical with the web's src/theme/testvectors/personal/, run there by
//! personal.test.ts).

use std::{fs, path::PathBuf};

use serde_json::Value;
use tinhat_theme::look::{look_json, Density, DENSITIES};
use tinhat_theme::personal::{
    density_label, effective_density, effective_look, gap, line, migrate, read_personal, row, snap_text_size, DEFAULT_DENSITY, DEFAULT_TEXT_SIZE,
    PREF_DENSITY, PREF_MIGRATED, PREF_TEXT_SIZE, TEXT_SIZES,
};

fn vec_file(name: &str) -> Value {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testvectors/personal").join(name);
    serde_json::from_str(&fs::read_to_string(&p).expect("read vector")).expect("vector json")
}

fn opt(v: &Value) -> Option<&str> {
    v.as_str()
}

#[test]
fn personal_steps_words_and_keys() {
    let v = vec_file("steps.json");
    let sizes: Vec<i64> = v["sizes"].as_array().unwrap().iter().map(|x| x.as_i64().unwrap()).collect();
    assert_eq!(sizes, TEXT_SIZES.to_vec());
    assert_eq!(v["defaultSize"].as_i64(), Some(DEFAULT_TEXT_SIZE));
    assert_eq!(v["defaultDensity"].as_str(), Some(DEFAULT_DENSITY.as_str()));
    let ds = v["densities"].as_array().unwrap();
    assert_eq!(ds.len(), DENSITIES.len());
    for (d, e) in DENSITIES.iter().zip(ds) {
        assert_eq!(e["id"].as_str(), Some(d.as_str()));
        assert_eq!(e["label"].as_str(), Some(density_label(*d)));
    }
    assert_eq!(v["prefs"]["textSize"].as_str(), Some(PREF_TEXT_SIZE));
    assert_eq!(v["prefs"]["density"].as_str(), Some(PREF_DENSITY));
    assert_eq!(v["prefs"]["migrated"].as_str(), Some(PREF_MIGRATED));
}

#[test]
fn personal_parse() {
    let v = vec_file("parse.json");
    for c in v["cases"].as_array().unwrap() {
        let p = read_personal(opt(&c["textSize"]), opt(&c["density"]));
        assert_eq!(p.size, c["expect"]["size"].as_i64().unwrap(), "{c}");
        assert_eq!(p.density.as_str(), c["expect"]["density"].as_str().unwrap(), "{c}");
    }
}

#[test]
fn personal_snap() {
    for c in vec_file("snap.json")["cases"].as_array().unwrap() {
        assert_eq!(snap_text_size(c[0].as_i64().unwrap()), c[1].as_i64().unwrap(), "{c}");
    }
}

#[test]
fn personal_migrate() {
    let v = vec_file("migrate.json");
    let cases = v["cases"].as_array().unwrap();
    assert!(cases.len() >= 20);
    for c in cases {
        let got: Vec<Value> = migrate(opt(&c["migrated"]), opt(&c["textSize"]), opt(&c["density"]), &c["theme"])
            .into_iter()
            .map(|(k, v)| Value::from(vec![Value::from(k), Value::from(v)]))
            .collect();
        assert_eq!(Value::from(got), c["expect"], "{}", c["name"]);
    }
}

#[test]
fn personal_metrics() {
    let v = vec_file("metrics.json");
    for c in v["line"].as_array().unwrap() {
        assert_eq!(line(c[0].as_i64().unwrap(), c[1].as_i64().unwrap()), c[2].as_i64().unwrap(), "line {c}");
    }
    for c in v["row"].as_array().unwrap() {
        let d = Density::parse(c[2].as_str().unwrap()).unwrap();
        assert_eq!(row(c[0].as_i64().unwrap(), c[1].as_i64().unwrap(), d), c[3].as_i64().unwrap(), "row {c}");
    }
    for c in v["gap"].as_array().unwrap() {
        let d = Density::parse(c[1].as_str().unwrap()).unwrap();
        assert_eq!(gap(c[0].as_i64().unwrap(), d), c[2].as_i64().unwrap(), "gap {c}");
    }
}

/// The P22 promise: the theme never decides the size or the density — two themes, one person, one size.
#[test]
fn personal_effective_look() {
    let v = vec_file("effective.json");
    for c in v["cases"].as_array().unwrap() {
        let p = read_personal(opt(&c["textSize"]), opt(&c["density"]));
        assert_eq!(look_json(&effective_look(&c["theme"], p)), c["expect"].as_str().unwrap(), "{c}");
    }
}

#[test]
fn personal_layout_density() {
    for c in vec_file("layout.json")["cases"].as_array().unwrap() {
        let p = Density::parse(c["person"].as_str().unwrap()).unwrap();
        let l = if c["layout"].is_null() { None } else { Some(&c["layout"]) };
        assert_eq!(effective_density(p, l).as_str(), c["expect"].as_str().unwrap(), "{c}");
    }
}

#[test]
fn the_default_look_is_unchanged_at_100_cozy() {
    // every row base the apps use at 100 % / Cozy keeps its old height (the theme gate's "nothing visible changed")
    for base in 28..=200 {
        assert_eq!(row(base, 100, Density::Normal), base);
    }
}
