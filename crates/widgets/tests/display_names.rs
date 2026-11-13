//! The "people, not friends" copy pass (2026-11-12): display text only. A widget list saved before the rename still loads
//! as it was saved (same ids, sizes, settings, collapsed) and resolves to the renamed titles; no widget name or
//! description says "friends". The web mirror is src/widgets/displayNames.test.ts.

use serde_json::{json, Value};
use tinhat_widgets::registry::{normalize_items, widget_def};

fn saved_before_rename() -> Value {
    json!([
        {"w": "whosAround", "size": "M", "settings": {"away": true}},
        {"w": "clocks", "size": "L", "collapsed": true},
        {"w": "justMe"}
    ])
}

#[test]
fn a_layout_saved_before_the_rename_loads_unchanged() {
    let saved = saved_before_rename();
    let items = normalize_items(&saved).expect("a list");
    assert_eq!(Value::Array(items.into_iter().map(Value::Object).collect()), saved);
    for w in ["whosAround", "clocks", "justMe"] {
        assert!(widget_def(w).is_some(), "{w} still in the registry under its old id");
    }
    assert_eq!(widget_def("clocks").unwrap()["title"], "People's clocks");
    assert!(widget_def("whosAround").unwrap()["description"].as_str().unwrap().starts_with("People who share their status"));
}

#[test]
fn no_widget_name_or_description_says_friends() {
    let reg: Value = serde_json::from_str(include_str!("../widgets.json")).unwrap();
    for w in reg["widgets"].as_array().unwrap() {
        for k in ["title", "description"] {
            let t = w[k].as_str().unwrap_or("").to_lowercase();
            assert!(!t.contains(concat!("fri", "end")), "{} {k}: {t}", w["id"]);
        }
    }
}
