//! The theme's shape block (src/shape.rs) — the web reference's `src/theme/shape.test.ts`, case for case.
use serde_json::json;
use tinhat_theme::shape::{read_shape, Shape, SHAPE_DEFAULTS};
use tinhat_theme::parse_json;

const D: Shape = SHAPE_DEFAULTS;
fn s(radius: i64, gap: i64, margin: i64, border: i64) -> Shape {
    Shape { radius, gap, margin, border }
}

#[test]
fn shape_defaults_without_block() {
    assert_eq!(read_shape(&json!({"id": "x"})), D);
}
#[test]
fn shape_defaults_for_non_objects() {
    assert_eq!(read_shape(&json!(null)), D);
    assert_eq!(read_shape(&json!("x")), D);
    assert_eq!(read_shape(&json!([])), D);
}
#[test]
fn shape_block_not_object() {
    assert_eq!(read_shape(&json!({"shape": [1, 2]})), D);
    assert_eq!(read_shape(&json!({"shape": 3})), D);
    assert_eq!(read_shape(&json!({"shape": null})), D);
}
#[test]
fn shape_reads_each_key() {
    assert_eq!(read_shape(&json!({"shape": {"radius": 0, "gap": 12, "margin": 0, "border": 2}})), s(0, 12, 0, 2));
}
#[test]
fn shape_partial_keeps_defaults() {
    assert_eq!(read_shape(&json!({"shape": {"radius": 4}})), s(4, 6, 6, 1));
}
#[test]
fn shape_bounds_inclusive() {
    assert_eq!(read_shape(&json!({"shape": {"radius": 24, "gap": 24, "margin": 24, "border": 0}})), s(24, 24, 24, 0));
}
#[test]
fn shape_out_of_bounds_ignored() {
    assert_eq!(read_shape(&json!({"shape": {"radius": 25, "gap": -1, "margin": 99, "border": 3}})), D);
}
#[test]
fn shape_non_integer_ignored() {
    assert_eq!(read_shape(&json!({"shape": {"radius": 12.5, "gap": "6", "margin": true, "border": 1.0000001}})), D);
}
#[test]
fn shape_integral_float_accepted() {
    // through the crate's JSON reader (JavaScript number semantics: 8.0 is the number 8)
    let v = parse_json(r#"{"shape": {"radius": 8.0}}"#).unwrap();
    assert_eq!(read_shape(&v), s(8, 6, 6, 1));
}
#[test]
fn shape_unknown_keys_ignored() {
    assert_eq!(read_shape(&json!({"shape": {"radius": 10, "shadow": "x", "tab": 30}})), s(10, 6, 6, 1));
}
#[test]
fn shape_inherited_keys_ignored() {
    // JSON has no prototypes: the web case (an inherited `radius`) has no Rust equivalent; an empty block = defaults
    assert_eq!(read_shape(&json!({"shape": {}})), D);
}
#[test]
fn shape_json_sorted_keys() {
    assert_eq!(s(12, 6, 6, 1).to_json(), r#"{"border":1,"gap":6,"margin":6,"radius":12}"#);
}
