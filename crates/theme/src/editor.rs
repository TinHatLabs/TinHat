//! editor.rs — port of `editor.ts` (contract THEMES-UI.md §13; ENGINE.md §10): the theme editor's pure side — the
//! operations on a working theme document, the undo history, readability (contrast) issues and their fixes, the
//! island-opacity macro, the save / export helpers. The web's `src/theme/editor.ts` is the REFERENCE, ported line by
//! line: the same functions and constants (snake_case), the same order of checks (it decides which refusal wins) and
//! the same refusal texts; both run the shared vectors `testvectors/editor/` byte for byte (tests/editor.rs).
//!
//! Pure: no Qt, no clock, no I/O. Every operation returns a NEW document (never changes its input) or a refusal code;
//! a document it returns always resolves (the engine checks it), so the live preview can never be handed a theme that
//! would be refused — a malformed edit is refused, never half-applied. Documents are `serde_json` values (read them
//! with [`crate::parse_json`]: JavaScript's number semantics; JSON has no prototypes, so own-property reads are plain
//! map lookups); an op is any JSON value — anything but a well-formed op object is `bad-op`. The app's entry points
//! that take and return JSON text: [`apply_edit_json`], [`contrast_json`], [`island_opacity_json`]; the history is an
//! [`EditSession`] the app keeps ([`session_edit`] / [`session_undo`] / [`session_redo`] / [`session_reset`]).

use std::collections::HashSet;

use serde_json::{json, Value};

use crate::canon::{canon_json, js_number, pretty_json};
use crate::engine::{app_tokens, resolve_theme, Resolved, TokensSpec, Variant};
use crate::expr::{parse_expr, Expr, FnName};
use crate::import::slugify;
use crate::js::js_round;
use crate::json::{number_value, parse_json};
use crate::limits::{code_points, take_code_points};
use crate::literal::{format_colour, parse_literal, Colour};
use crate::look::{is_allowed_font, Density, FONT_SLOTS, SHADOW_BOUNDS, SIZE_BOUNDS};
use crate::maths::luminance;
use crate::ops::{cr, guard_all_c, guard_c};
use crate::roles::{guard_of, is_role, GUARDS, GUARD_SURFACES, PALETTE_SLOTS};
use crate::shape::SHAPE_BOUNDS;
use crate::sync::{unique_theme_id, JObj};

// ---- operations ---------------------------------------------------------------------------------------------------

/// The shape keys the editor sets (radius / gap / margin / border: [`crate::shape`]; shadow: [`crate::look`]).
pub const SHAPE_KEYS: [&str; 5] = ["radius", "gap", "margin", "border", "shadow"];
/// The island fills the opacity slider changes: every island except the call (its stage stays opaque under video,
/// ISLANDS-ARCH §3.4) and the widgets (clear by design: each widget draws its own card).
pub const OPACITY_TOKENS: [&str; 10] = [
    "island.bg.account",
    "island.bg.buttons",
    "island.bg.composer",
    "island.bg.dms",
    "island.bg.header",
    "island.bg.nav",
    "island.bg.panel",
    "island.bg.rooms",
    "island.bg.spaces",
    "island.bg.timeline",
];
/// The opacity slider's range in per cent (inclusive).
pub const OPACITY_BOUNDS: (i64, i64) = (40, 100);

/// A roles-only theme's surface family, re-derived from a new background (§13.2 `background`): (role, dark, light).
pub const SURFACE_FAMILY: [(&str, &str, &str); 7] = [
    ("surface.window", "mixok(surface.island, #000000, 0.25)", "mixok(surface.island, text.primary, 0.06)"),
    ("surface.raised", "mixok(surface.island, text.primary, 0.07)", "mixok(surface.island, #ffffff, 0.6)"),
    ("surface.hover", "mixok(surface.island, text.primary, 0.1)", "mixok(surface.island, text.primary, 0.1)"),
    ("surface.pressed", "mixok(surface.island, text.primary, 0.14)", "mixok(surface.island, text.primary, 0.14)"),
    ("surface.sunken", "mixok(surface.island, surface.window, 0.5)", "mixok(surface.island, surface.window, 0.5)"),
    ("border.subtle", "mixok(surface.island, text.primary, 0.13)", "mixok(surface.island, text.primary, 0.13)"),
    ("border.strong", "mixok(surface.island, text.primary, 0.24)", "mixok(surface.island, text.primary, 0.24)"),
];
/// `accent.bright` of a roles-only theme after `accent` (the engine's derivation for a literal accent, ENGINE.md §5):
/// dark themes …
pub const BRIGHT_DARK: &str = "mixok(accent.base, #ffffff, 0.3)";
/// … light themes.
pub const BRIGHT_LIGHT: &str = "mixok(accent.base, #000000, 0.2)";

/// Accent suggestions for a theme without a palette (roles-only: OLED, Goose and their copies).
pub const ACCENT_PRESETS: [&str; 12] = [
    "#1e6a76", "#2f6fd6", "#5b5bd6", "#8a4fd8", "#b0508c", "#d0485e", "#d0642c", "#c99a1e", "#4f9a3a", "#1f9d7a",
    "#0f8fb0", "#6b7280",
];

const OPTIONAL_SLOTS: [&str; 3] = ["cursor", "selectionBg", "selectionFg"];

/// An applied edit (the reference's `{ok: true, doc, resolved}`): the new working document and its resolve.
#[derive(Clone, Debug, PartialEq)]
pub struct Edited {
    pub doc: JObj,
    pub resolved: Resolved,
}

/// A refusal code (the reference's `Fail`).
type Fail = String;

fn fail<T>(code: impl Into<String>) -> Result<T, Fail> {
    Err(code.into())
}

/// `value === null`
fn is_null(v: Option<&Value>) -> bool {
    matches!(v, Some(Value::Null))
}

/// `typeof v === "number" && Number.isInteger(v)` → the double.
fn int_value(v: Option<&Value>) -> Option<f64> {
    match v {
        Some(Value::Number(n)) => n.as_f64().filter(|x| x.is_finite() && x.fract() == 0.0),
        _ => None,
    }
}

fn is_ascii_ws(ch: char) -> bool {
    matches!(ch, ' ' | '\t' | '\n' | '\r')
}

/// Strip space / tab / CR / LF from both ends (the expression grammar's whitespace; the same in the reference) —
/// nothing else: no other Unicode space.
pub fn trim_ascii(s: &str) -> &str {
    s.trim_matches(is_ascii_ws)
}

/// An opaque colour literal (any accepted form), or none.
fn opaque_literal(v: Option<&Value>) -> Option<Colour> {
    match v {
        Some(Value::String(s)) => parse_literal(s).filter(Colour::is_opaque),
        _ => None,
    }
}

/// An expression value as stored: a colour literal in canonical form, else the expression text (trimmed). `v` =
/// the op's value (`None`: absent); a non-string is `bad-op`, an expression that does not parse `bad-expr: <why>`.
pub fn expr_value(v: Option<&Value>) -> Result<String, String> {
    let Some(Value::String(v)) = v else { return fail("bad-op") };
    let t = trim_ascii(v);
    if let Some(c) = parse_literal(t) {
        return Ok(format_colour(c));
    }
    if let Err(why) = parse_expr(t) {
        return fail(format!("bad-expr: {why}"));
    }
    Ok(t.to_string())
}

/// k / 10000 printed with integer maths: "0.85", "1", "1.18", "0.0001" (ENGINE.md §1's alpha digits).
pub fn num_text(k: i64) -> String {
    let ip = k.div_euclid(10000); // Math.floor(k / 10000)
    let fp = k.rem_euclid(10000); // k - ip * 10000
    if fp == 0 {
        return ip.to_string();
    }
    let d = format!("{fp:04}");
    format!("{ip}.{}", d.trim_end_matches('0'))
}

/// Where the number goes in each function's argument list (expr.ts SIGS); -1 = none.
fn num_at(f: FnName) -> i32 {
    match f {
        FnName::Alpha | FnName::Lighter | FnName::Darker => 1,
        FnName::Mix | FnName::Mixok | FnName::Guard => 2,
        FnName::Over | FnName::Best => -1,
    }
}

/// Canonical text of a parsed expression: literals canonical, `f(a, b, n)` with ", " and [`num_text`].
pub fn expr_canon(e: &Expr) -> String {
    match e {
        Expr::Lit(c) => format_colour(*c),
        Expr::Ref(id) => id.clone(),
        Expr::Fn { f, args, k } => {
            let mut parts: Vec<String> = args.iter().map(expr_canon).collect();
            let at = num_at(*f);
            if at >= 0 {
                let n = *k as f64 / 10000.0; // the reference's e.n
                parts.insert((at as usize).min(parts.len()), num_text(js_round(n * 10000.0) as i64));
            }
            format!("{}({})", f.as_str(), parts.join(", "))
        }
    }
}

/// The object at d[key], created (replacing a non-object) when missing.
fn ensure<'a>(d: &'a mut JObj, key: &str) -> &'a mut JObj {
    let v = d.entry(key).or_insert(Value::Null);
    if !v.is_object() {
        *v = Value::Object(JObj::new());
    }
    v.as_object_mut().expect("just made an object")
}

/// Remove d[key][field]; drop d[key] when that leaves it empty.
fn remove_in(d: &mut JObj, key: &str, field: &str) {
    let Some(Value::Object(v)) = d.get_mut(key) else { return };
    if v.remove(field).is_none() {
        return;
    }
    if v.is_empty() {
        d.remove(key);
    }
}

fn has_palette(d: &JObj) -> bool {
    d.contains_key("palette")
}

fn resolve_or_fail(d: &JObj, spec: &TokensSpec) -> Result<Resolved, Fail> {
    resolve_theme(&Value::Object(d.clone()), spec).map_err(|e| format!("refused: {e}"))
}

/// A role of a resolve as a colour (every role of a resolve is a canonical colour).
fn role_colour(r: &Resolved, id: &str) -> Colour {
    r.roles.get(id).and_then(|s| parse_literal(s)).expect("a resolved role is a canonical colour")
}

/// Tokens whose override is a literal equal to an accent colour that changed follow it as a reference (§13.2).
fn follow_accent(d: &mut JObj, before: &Resolved, after: &Resolved) {
    let Some(Value::Object(t)) = d.get_mut("tokens") else { return };
    let old_bright = role_colour(before, "accent.bright");
    let old_base = role_colour(before, "accent.base");
    let bright_moved = before.roles.get("accent.bright") != after.roles.get("accent.bright");
    let base_moved = before.roles.get("accent.base") != after.roles.get("accent.base");
    // (each key on its own: the reference's code-point order changes nothing)
    for v in t.values_mut() {
        let c = match v {
            Value::String(s) => parse_literal(s),
            _ => None,
        };
        let Some(c) = c else { continue };
        if bright_moved && c == old_bright {
            *v = Value::String("accent.bright".into());
        } else if base_moved && c == old_base {
            *v = Value::String("accent.base".into());
        }
    }
}

/// The expression of a token without an outer alpha() (canonical text) + that alpha's k, or none if it does not parse.
fn unwrap_alpha(src: &str) -> Option<(String, Option<i64>)> {
    let e = parse_expr(src).ok()?;
    if let Expr::Fn { f: FnName::Alpha, args, k } = &e {
        let n = *k as f64 / 10000.0; // the reference's e.n
        return Some((expr_canon(&args[0]), Some(js_round(n * 10000.0) as i64)));
    }
    Some((expr_canon(&e), None))
}

/// `/^c([1-9]|1[0-5])$/`: an accent slot name.
fn is_accent_slot(v: &str) -> bool {
    matches!(v.as_bytes(), [b'c', b'1'..=b'9'] | [b'c', b'1', b'0'..=b'5'])
}

/// `/^c[0-9]+$/`
fn is_c_number(v: &str) -> bool {
    v.len() >= 2 && v.starts_with('c') && v[1..].bytes().all(|b| b.is_ascii_digit())
}

/// `/^c([0-9]|1[0-5])$/` → the ansi index.
fn ansi_index(slot: &str) -> Option<usize> {
    match slot.as_bytes() {
        [b'c', d @ b'0'..=b'9'] => Some((d - b'0') as usize),
        [b'c', b'1', d @ b'0'..=b'5'] => Some(10 + (d - b'0') as usize),
        _ => None,
    }
}

fn apply_op(d: &mut JObj, op: &JObj, spec: &TokensSpec) -> Result<(), Fail> {
    let no_tokens = TokensSpec::empty();
    let value = op.get("value");
    let kind = match op.get("op") {
        Some(Value::String(k)) => k.as_str(),
        _ => return fail("bad-op"),
    };
    match kind {
        "name" => {
            let Some(Value::String(v)) = value else { return fail("bad-op") };
            let n = trim_ascii(v);
            if n.is_empty() || code_points(n) > 80 {
                return fail("bad-value");
            }
            d.insert("name".into(), Value::String(n.to_string()));
            Ok(())
        }
        "author" => {
            let a = match value {
                Some(Value::Null) => "",
                Some(Value::String(v)) => trim_ascii(v),
                _ => return fail("bad-op"),
            };
            if code_points(a) > 80 {
                return fail("bad-value");
            }
            if a.is_empty() {
                d.remove("author");
            } else {
                d.insert("author".into(), Value::String(a.to_string()));
            }
            Ok(())
        }
        "background" => {
            let c = opaque_literal(value);
            if !matches!(value, Some(Value::String(_))) {
                return fail("bad-op");
            }
            let Some(c) = c else { return fail("bad-colour") };
            if has_palette(d) {
                ensure(d, "palette").insert("bg".into(), Value::String(format_colour(c)));
                return Ok(());
            }
            let before = resolve_or_fail(d, &no_tokens)?;
            let dark = luminance(c) < luminance(role_colour(&before, "text.primary"));
            let roles = ensure(d, "roles");
            roles.insert("surface.island".into(), Value::String(format_colour(c)));
            for (id, dark_e, light_e) in SURFACE_FAMILY {
                roles.insert(id.into(), Value::String(if dark { dark_e } else { light_e }.into()));
            }
            if d.contains_key("variant") {
                d.insert("variant".into(), Value::String(if dark { "dark" } else { "light" }.into()));
            }
            Ok(())
        }
        "accent" => {
            let Some(Value::String(v)) = value else { return fail("bad-op") };
            let before = resolve_or_fail(d, &no_tokens)?;
            if has_palette(d) {
                let pal = ensure(d, "palette");
                let a = if v == "cursor" {
                    if !pal.contains_key("cursor") {
                        return fail("bad-value");
                    }
                    v.clone()
                } else if is_accent_slot(v) {
                    v.clone()
                } else {
                    let Some(c) = opaque_literal(value) else { return fail("bad-colour") };
                    format_colour(c)
                };
                d.insert("accent".into(), Value::String(a));
                remove_in(d, "roles", "accent.base");
                remove_in(d, "roles", "accent.bright");
            } else {
                if v == "cursor" || is_c_number(v) {
                    return fail("no-palette");
                }
                let Some(c) = opaque_literal(value) else { return fail("bad-colour") };
                let roles = ensure(d, "roles");
                roles.insert("accent.base".into(), Value::String(format_colour(c)));
                let bright = if before.variant == Variant::Dark { BRIGHT_DARK } else { BRIGHT_LIGHT };
                roles.insert("accent.bright".into(), Value::String(bright.into()));
            }
            let after = resolve_or_fail(d, &no_tokens)?;
            follow_accent(d, &before, &after);
            Ok(())
        }
        "palette" => {
            let (Some(Value::String(slot)), Some(Value::Null | Value::String(_))) = (op.get("slot"), value) else {
                return fail("bad-op");
            };
            if !has_palette(d) {
                return fail("no-palette");
            }
            if !PALETTE_SLOTS.contains(&slot.as_str()) {
                return fail("unknown-slot");
            }
            let pal = ensure(d, "palette");
            if is_null(value) {
                if !OPTIONAL_SLOTS.contains(&slot.as_str()) {
                    return fail("required");
                }
                pal.remove(slot.as_str());
                return Ok(());
            }
            let Some(c) = opaque_literal(value) else { return fail("bad-colour") };
            if let Some(i) = ansi_index(slot) {
                match pal.get_mut("ansi") {
                    Some(Value::Array(ansi)) if ansi.len() == 16 => ansi[i] = Value::String(format_colour(c)),
                    _ => return fail("refused: bad-palette: ansi must be 16 colours"),
                }
            } else {
                pal.insert(slot.clone(), Value::String(format_colour(c)));
            }
            Ok(())
        }
        "role" => {
            let Some(Value::String(id)) = op.get("id") else { return fail("bad-op") };
            if !is_role(id) {
                return fail("unknown-role");
            }
            if is_null(value) {
                if !has_palette(d) {
                    return fail("required");
                }
                remove_in(d, "roles", id);
                return Ok(());
            }
            let e = expr_value(value)?;
            ensure(d, "roles").insert(id.clone(), Value::String(e));
            Ok(())
        }
        "token" => {
            let Some(Value::String(id)) = op.get("id") else { return fail("bad-op") };
            if spec.default_expr(id).is_none() {
                return fail("unknown-token");
            }
            if is_null(value) {
                remove_in(d, "tokens", id);
                return Ok(());
            }
            let e = expr_value(value)?;
            ensure(d, "tokens").insert(id.clone(), Value::String(e));
            Ok(())
        }
        "shape" => {
            let (Some(Value::String(key)), Some(Value::Null | Value::Number(_))) = (op.get("key"), value) else {
                return fail("bad-op");
            };
            if !SHAPE_KEYS.contains(&key.as_str()) {
                return fail("unknown-key");
            }
            if is_null(value) {
                remove_in(d, "shape", key);
                return Ok(());
            }
            let (lo, hi) = if key == "shadow" {
                SHADOW_BOUNDS
            } else {
                let &(_, lo, hi) = SHAPE_BOUNDS.iter().find(|(k, _, _)| k == key).expect("a shape.rs key");
                (lo, hi)
            };
            let Some(n) = int_value(value).filter(|&n| n >= lo as f64 && n <= hi as f64) else {
                return fail("out-of-range");
            };
            ensure(d, "shape").insert(key.clone(), number_value(n));
            Ok(())
        }
        "density" => match value {
            Some(Value::Null) => {
                remove_in(d, "shape", "density");
                Ok(())
            }
            Some(Value::String(v)) => {
                if Density::parse(v).is_none() {
                    return fail("bad-value");
                }
                ensure(d, "shape").insert("density".into(), Value::String(v.clone()));
                Ok(())
            }
            _ => fail("bad-op"),
        },
        "font" => {
            let (Some(Value::String(slot)), Some(Value::Null | Value::String(_))) = (op.get("slot"), value) else {
                return fail("bad-op");
            };
            if !FONT_SLOTS.contains(&slot.as_str()) {
                return fail("unknown-slot");
            }
            let Some(Value::String(family)) = value else {
                remove_in(d, "type", slot); // value === null
                return Ok(());
            };
            if !is_allowed_font(slot, Some(family)) {
                return fail("font-not-allowed");
            }
            ensure(d, "type").insert(slot.clone(), Value::String(family.clone()));
            Ok(())
        }
        "size" => match value {
            Some(Value::Null) => {
                remove_in(d, "type", "size");
                Ok(())
            }
            Some(Value::Number(_)) => {
                let Some(n) = int_value(value).filter(|&n| n >= SIZE_BOUNDS.0 as f64 && n <= SIZE_BOUNDS.1 as f64)
                else {
                    return fail("out-of-range");
                };
                ensure(d, "type").insert("size".into(), number_value(n));
                Ok(())
            }
            _ => fail("bad-op"),
        },
        "opacity" => {
            if !matches!(value, Some(Value::Number(_))) {
                return fail("bad-op");
            }
            let Some(v) =
                int_value(value).filter(|&n| n >= OPACITY_BOUNDS.0 as f64 && n <= OPACITY_BOUNDS.1 as f64)
            else {
                return fail("out-of-range");
            };
            let v = v as i64;
            for id in OPACITY_TOKENS {
                // an older token list without the island fills
                let Some(dflt) = spec.default_expr(id) else { continue };
                let over = match d.get("tokens") {
                    Some(Value::Object(t)) => match t.get(id) {
                        Some(Value::String(s)) => Some(s.as_str()),
                        _ => None,
                    },
                    _ => None,
                };
                // canonExpr(default): the spec parsed every default already, so this is never null here
                let dflt_canon = Some(expr_canon(dflt));
                // the fill without the alpha this slider set (an override is unwrapped once; a default never is)
                let u = over.and_then(unwrap_alpha);
                let base = match u {
                    Some((b, _)) => Some(b),
                    None => dflt_canon.clone(),
                };
                let Some(base) = base else { return fail(format!("bad-expr: {id}")) };
                if v == 100 {
                    if Some(&base) == dflt_canon.as_ref() {
                        remove_in(d, "tokens", id);
                    } else {
                        ensure(d, "tokens").insert(id.into(), Value::String(base));
                    }
                } else {
                    let e = format!("alpha({base}, {})", num_text(v * 100));
                    ensure(d, "tokens").insert(id.into(), Value::String(e));
                }
            }
            Ok(())
        }
        _ => fail("bad-op"),
    }
}

/// Apply one edit to a working document (§13.2). Returns the new document and its resolve, or the refusal:
/// `bad-op`, `bad-colour`, `bad-value`, `no-palette`, `required`, `unknown-role`, `unknown-token`, `unknown-slot`,
/// `unknown-key`, `bad-expr: <why>`, `out-of-range`, `font-not-allowed`, or `refused: <engine error>` when the result
/// would not resolve. Never panics; never changes `doc`.
pub fn apply_edit(doc: &Value, op: &Value, spec: &TokensSpec) -> Result<Edited, String> {
    let (Some(doc), Some(op)) = (doc.as_object(), op.as_object()) else { return fail("bad-op") };
    let mut d = doc.clone();
    apply_op(&mut d, op, spec)?;
    let resolved = resolve_or_fail(&d, spec)?;
    Ok(Edited { doc: d, resolved })
}

/// `{variant, names, adjusted, roles, tokens}` of a resolve (the vectors' `resolved`).
fn resolved_out(r: &Resolved) -> Value {
    let mut v = r.to_json();
    if let Some(o) = v.as_object_mut() {
        o.remove("ok");
    }
    v
}

/// `{"ok": true, "doc": …, "resolved": {variant, names, adjusted, roles, tokens}}` or `{"ok": false, "error": …}`.
pub fn edit_result_to_json(r: &Result<Edited, String>) -> Value {
    match r {
        Ok(e) => json!({ "ok": true, "doc": Value::Object(e.doc.clone()), "resolved": resolved_out(&e.resolved) }),
        Err(error) => json!({ "ok": false, "error": error }),
    }
}

/// JavaScript `String(v)` of a JSON value (`None` = undefined).
fn js_string(v: Option<&Value>) -> String {
    match v {
        None => "undefined".into(),
        Some(v) => js_string_of(v),
    }
}

fn js_string_of(v: &Value) -> String {
    match v {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => js_number(n.as_f64().unwrap_or(f64::NAN)),
        Value::String(s) => s.clone(),
        // Array.prototype.join: null elements print as ""
        Value::Array(a) => {
            a.iter().map(|x| if x.is_null() { String::new() } else { js_string_of(x) }).collect::<Vec<_>>().join(",")
        }
        Value::Object(_) => "[object Object]".into(),
    }
}

/// The history key of an op (`<op>:<slot | id | key>`): edits of the same key in one gesture (a drag) are one undo
/// step.
pub fn op_key(op: &JObj) -> String {
    // `slot ?? id ?? key ?? ""`: the first one that is neither absent nor null
    let target = ["slot", "id", "key"].iter().find_map(|k| op.get(*k).filter(|v| !v.is_null()));
    format!("{}:{}", js_string(op.get("op")), target.map(js_string_of).unwrap_or_default())
}

/// Island opacity (per cent) as the slider shows it: 100 when no [`OPACITY_TOKENS`] fill is overridden with an
/// alpha(), the shared value when every fill is `alpha(x, n)` with the same whole-per-cent n (or 100), else `None`
/// (mixed).
pub fn island_opacity(doc: &Value, spec: &TokensSpec) -> Option<i64> {
    let t = doc.as_object().and_then(|d| d.get("tokens"));
    let mut shared: Option<i64> = None;
    for id in OPACITY_TOKENS {
        if spec.default_expr(id).is_none() {
            continue;
        }
        let mut pct = 100;
        if let Some(Value::Object(t)) = t {
            if let Some(Value::String(s)) = t.get(id) {
                let (_, k) = unwrap_alpha(s)?;
                if let Some(k) = k {
                    if k % 100 != 0 {
                        return None;
                    }
                    pct = k / 100;
                }
            }
        }
        match shared {
            None => shared = Some(pct),
            Some(s) if s != pct => return None,
            Some(_) => {}
        }
    }
    Some(shared.unwrap_or(100))
}

// ---- history (§13.3) ----------------------------------------------------------------------------------------------

/// The undo stack's size (the oldest step is dropped beyond it).
pub const HISTORY_MAX: usize = 100;

/// The editor's session: the base, the working document and the undo / redo stacks.
#[derive(Clone, Debug, PartialEq)]
pub struct EditSession {
    /// The document the editor opened with (Reset / Cancel / dirty).
    pub base: JObj,
    /// The working document (always resolves).
    pub doc: JObj,
    /// Undo stack, oldest first (≤ [`HISTORY_MAX`]).
    pub past: Vec<JObj>,
    /// Redo stack, the most recently undone last.
    pub future: Vec<JObj>,
    /// The gesture of the last edit (edits of one gesture on the same key coalesce); `None` after undo / redo.
    pub gesture: Option<i64>,
    pub last_key: Option<String>,
}

/// What a session step did (the reference's `result`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepResult {
    Edited,
    Noop,
    Undone,
    Redone,
    Reset,
}

impl StepResult {
    pub fn as_str(self) -> &'static str {
        match self {
            StepResult::Edited => "edited",
            StepResult::Noop => "noop",
            StepResult::Undone => "undone",
            StepResult::Redone => "redone",
            StepResult::Reset => "reset",
        }
    }
}

fn canon_obj(o: &JObj) -> String {
    canon_json(&Value::Object(o.clone()))
}

/// `[...past, doc].slice(-HISTORY_MAX)`
fn push_capped(past: &mut Vec<JObj>, doc: JObj) {
    past.push(doc);
    if past.len() > HISTORY_MAX {
        past.drain(..past.len() - HISTORY_MAX);
    }
}

/// A session on `doc`: base = working document = `doc`, empty history.
pub fn open_session(doc: &JObj) -> EditSession {
    let (past, future) = (Vec::new(), Vec::new());
    EditSession { base: doc.clone(), doc: doc.clone(), past, future, gesture: None, last_key: None }
}

/// An edit in the session: refused → `Err` and the session is unchanged; equal to the working document → `Noop`
/// (unchanged); else `Edited`: the old document goes on the undo stack — unless this edit continues the previous one's
/// gesture on the same key — and the redo stack is cleared. `gesture`: `None` = not part of a gesture.
pub fn session_edit(
    s: &mut EditSession,
    op: &Value,
    spec: &TokensSpec,
    gesture: Option<i64>,
) -> Result<StepResult, String> {
    let r = apply_edit(&Value::Object(s.doc.clone()), op, spec)?;
    let key = op.as_object().map(op_key).unwrap_or_default();
    if canon_obj(&r.doc) == canon_obj(&s.doc) {
        return Ok(StepResult::Noop);
    }
    let coalesce =
        gesture.is_some() && s.gesture == gesture && s.last_key.as_deref() == Some(key.as_str()) && !s.past.is_empty();
    let old = std::mem::replace(&mut s.doc, r.doc);
    if !coalesce {
        push_capped(&mut s.past, old);
    }
    s.future.clear();
    s.gesture = gesture;
    s.last_key = Some(key);
    Ok(StepResult::Edited)
}

/// Undo: `Undone`, or `Noop` with nothing to undo.
pub fn session_undo(s: &mut EditSession) -> StepResult {
    let Some(doc) = s.past.pop() else { return StepResult::Noop };
    let old = std::mem::replace(&mut s.doc, doc);
    s.future.push(old);
    s.gesture = None;
    s.last_key = None;
    StepResult::Undone
}

/// Redo: `Redone`, or `Noop` with nothing to redo.
pub fn session_redo(s: &mut EditSession) -> StepResult {
    let Some(doc) = s.future.pop() else { return StepResult::Noop };
    let old = std::mem::replace(&mut s.doc, doc);
    push_capped(&mut s.past, old);
    s.gesture = None;
    s.last_key = None;
    StepResult::Redone
}

/// Reset all: back to the base document as one more undoable step (`Noop` when the document is the base).
pub fn session_reset(s: &mut EditSession) -> StepResult {
    if canon_obj(&s.doc) == canon_obj(&s.base) {
        return StepResult::Noop;
    }
    let old = std::mem::replace(&mut s.doc, s.base.clone());
    push_capped(&mut s.past, old);
    s.future.clear();
    s.gesture = None;
    s.last_key = None;
    StepResult::Reset
}

/// The working document differs from the base.
pub fn is_dirty(s: &EditSession) -> bool {
    canon_obj(&s.doc) != canon_obj(&s.base)
}

// ---- readability (§13.5) ------------------------------------------------------------------------------------------

/// A pair below its minimum contrast.
#[derive(Clone, Debug, PartialEq)]
pub struct ContrastIssue {
    pub fg: String,
    pub bg: String,
    /// cr(fg, bg), floored to 2 decimals
    pub ratio: f64,
    pub need: f64,
}

impl ContrastIssue {
    /// `{"fg": …, "bg": …, "ratio": …, "need": …}`
    pub fn to_json(&self) -> Value {
        json!({ "fg": self.fg, "bg": self.bg, "ratio": self.ratio, "need": self.need })
    }
}

/// `/^(.+)\.(fg|icon)(\.[a-z][A-Za-z0-9]*)?$/` with the reference's backtracking (the longest group 1 wins) →
/// (group 1, "fg" | "icon", group 3 or "").
fn fg_icon_match(id: &str) -> Option<(&str, &str, &str)> {
    // group 1 (`.+`) cannot hold a line terminator (JavaScript's `.` without the s flag)
    let limit = id.find(['\n', '\r', '\u{2028}', '\u{2029}']).unwrap_or(id.len());
    let b = id.as_bytes();
    for i in (1..limit).rev() {
        if b[i] != b'.' {
            continue;
        }
        let rest = &id[i + 1..];
        let (kind, after) = if let Some(a) = rest.strip_prefix("fg") {
            ("fg", a)
        } else if let Some(a) = rest.strip_prefix("icon") {
            ("icon", a)
        } else {
            continue;
        };
        let ab = after.as_bytes();
        let state_ok = ab.len() >= 2
            && ab[0] == b'.'
            && ab[1].is_ascii_lowercase()
            && ab[2..].iter().all(|c| c.is_ascii_alphanumeric());
        if after.is_empty() || state_ok {
            return Some((&id[..i], kind, after));
        }
    }
    None
}

/// The token pairs checked for readability, from the naming contract (NAMING.md §4): a token `<p>.fg[.<state>]` (4.5)
/// or `<p>.icon[.<state>]` (3.0) against `<p>.bg[.<state>]`, else `<p>.bg`, when that token exists. Sorted by fg id:
/// (fg, bg, need).
pub fn contrast_pairs(token_ids: &[&str]) -> Vec<(String, String, f64)> {
    let ids: HashSet<&str> = token_ids.iter().copied().collect();
    let mut sorted = token_ids.to_vec();
    sorted.sort(); // code-point order (= UTF-8 byte order), stable
    let mut out = Vec::new();
    for id in sorted {
        let Some((p, kind, state)) = fg_icon_match(id) else { continue };
        let with_state = format!("{p}.bg{state}");
        let plain = format!("{p}.bg");
        let bg = if ids.contains(with_state.as_str()) {
            Some(with_state)
        } else if ids.contains(plain.as_str()) {
            Some(plain)
        } else {
            None
        };
        if let Some(bg) = bg {
            out.push((id.to_string(), bg, if kind == "fg" { 4.5 } else { 3.0 }));
        }
    }
    out
}

/// `Math.floor(x * 100) / 100`
fn floor2(x: f64) -> f64 {
    (x * 100.0).floor() / 100.0
}

/// The colour of a role or a token of a resolve.
fn col(r: &Resolved, id: &str) -> Option<Colour> {
    r.roles.get(id).or_else(|| r.tokens.get(id)).and_then(|s| parse_literal(s))
}

/// Readability issues of a resolved theme (§13.5): every guarded role (ENGINE.md §5 GUARDS) against every guard
/// surface, and every [`contrast_pairs`] token pair whose background is opaque (a translucent one sits on something
/// this cannot know: a bubble, a video, a card). Only the pairs below their minimum, sorted by fg, then bg.
pub fn contrast_issues(resolved: &Resolved) -> Vec<ContrastIssue> {
    let mut out = Vec::new();
    let mut guards = GUARDS.to_vec();
    guards.sort_by(|a, b| a.0.cmp(b.0));
    for (g, need) in guards {
        let Some(fg) = col(resolved, g) else { continue };
        for s in GUARD_SURFACES {
            let Some(bg) = col(resolved, s) else { continue };
            let ratio = cr(fg, bg);
            if ratio < need {
                out.push(ContrastIssue { fg: g.into(), bg: s.into(), ratio: floor2(ratio), need });
            }
        }
    }
    let ids: Vec<&str> = resolved.tokens.keys().map(String::as_str).collect();
    for (f, b, need) in contrast_pairs(&ids) {
        // a translucent background's backdrop is not known here (a bubble, a video, a card): not checked
        let (Some(fg), Some(bg)) = (col(resolved, &f), col(resolved, &b)) else { continue };
        if !bg.is_opaque() {
            continue;
        }
        let ratio = cr(fg, bg);
        if ratio < need {
            out.push(ContrastIssue { fg: f, bg: b, ratio: floor2(ratio), need });
        }
    }
    out.sort_by(|a, b| a.fg.cmp(&b.fg).then_with(|| a.bg.cmp(&b.bg)));
    out
}

/// The issues of `now` whose pair is not an issue of `base` (the editor warns about what the edits broke, not about a
/// built-in's own choices).
pub fn new_issues(now: &[ContrastIssue], base: &[ContrastIssue]) -> Vec<ContrastIssue> {
    let seen: HashSet<String> = base.iter().map(|i| format!("{}\u{0}{}", i.fg, i.bg)).collect();
    now.iter().filter(|i| !seen.contains(&format!("{}\u{0}{}", i.fg, i.bg))).cloned().collect()
}

/// The edit that fixes an issue, as a literal on the fg id (the op object `{"op": "role" | "token", "id", "value"}`
/// that [`apply_edit`] takes): a guarded role against a guard surface is moved with guardAll over EVERY guard surface
/// (one fix for all of them, as the engine guards palette themes, ENGINE.md §9); any other pair with
/// guard(fg, bg, need). `None` when the issue's ids are not colours of `resolved` (where the reference throws).
pub fn fix_contrast(issue: &ContrastIssue, resolved: &Resolved) -> Option<Value> {
    let is_role = resolved.roles.contains_key(&issue.fg);
    let fg = parse_literal(if is_role { resolved.roles.get(&issue.fg) } else { resolved.tokens.get(&issue.fg) }?)?;
    let need = guard_of(&issue.fg);
    let fixed = match need {
        Some(n) if is_role && GUARD_SURFACES.contains(&issue.bg.as_str()) => {
            let bgs: Option<Vec<Colour>> =
                GUARD_SURFACES.iter().map(|id| resolved.roles.get(*id).and_then(|s| parse_literal(s))).collect();
            guard_all_c(fg, &bgs?, n)
        }
        _ => guard_c(fg, col(resolved, &issue.bg)?, issue.need),
    };
    let op = if is_role { "role" } else { "token" };
    Some(json!({ "op": op, "id": issue.fg, "value": format_colour(fixed) }))
}

/// One fix per fg id (the first issue of each fg, in issue order): what the "Fix" buttons apply.
pub fn fixes_for(issues: &[ContrastIssue], resolved: &Resolved) -> Vec<Value> {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut out = Vec::new();
    for i in issues {
        if !seen.insert(i.fg.as_str()) {
            continue;
        }
        out.extend(fix_contrast(i, resolved));
    }
    out
}

// ---- starting, saving, exporting (§13.1, §13.4) -------------------------------------------------------------------

/// "‹name› (edited)", cut so it stays ≤ 80 code points.
pub fn copy_name(name: &str) -> String {
    const TAIL: &str = " (edited)";
    format!("{}{TAIL}", take_code_points(trim_ascii(name), 80 - TAIL.len()))
}

/// The working document of "New theme from this": a copy with [`copy_name`]`(name)` ("Theme" when it has none).
pub fn start_copy(doc: &JObj) -> JObj {
    let mut d = doc.clone();
    let name = match d.get("name") {
        Some(Value::String(n)) => copy_name(n),
        _ => copy_name("Theme"),
    };
    d.insert("name".into(), Value::String(name));
    d
}

/// Where [`save_doc`] saves: over one of My themes (its id) or as a new one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveTarget<'a> {
    Mine(&'a str),
    New,
}

/// The document to save: `Mine` keeps its id; `New` gets slugify(name) made unique (THEMES-SYNC §4.1) against
/// `taken` (the library and My themes).
pub fn save_doc(doc: &JObj, target: SaveTarget, taken: impl Fn(&str) -> bool) -> JObj {
    let mut d = doc.clone();
    let id = match target {
        SaveTarget::Mine(id) => id.to_string(),
        SaveTarget::New => {
            let name = match d.get("name") {
                Some(Value::String(n)) => n.as_str(),
                _ => "",
            };
            unique_theme_id(&slugify(name), taken)
        }
    };
    d.insert("id".into(), Value::String(id));
    d
}

/// The export file's text: the document as pretty canonical JSON (the vector format), ending in a newline.
pub fn export_text(doc: &JObj) -> String {
    pretty_json(&Value::Object(doc.clone()))
}

/// `<slug>.tinhat-theme.json`.
pub fn export_file_name(doc: &JObj) -> String {
    let name = match doc.get("name") {
        Some(Value::String(n)) => n.as_str(),
        _ => "",
    };
    format!("{}.tinhat-theme.json", slugify(name))
}

// ---- the app's JSON-text entry points (app tokens: the embedded tokens.json) --------------------------------------

/// [`apply_edit`] on JSON text with the app's token list: `{"ok":true,"doc":…,"resolved":{…}}` /
/// `{"ok":false,"error":…}` as canonical JSON (text that is not JSON is not an object: `bad-op`).
pub fn apply_edit_json(doc_json: &str, op_json: &str) -> String {
    let doc = parse_json(doc_json).unwrap_or(Value::Null);
    let op = parse_json(op_json).unwrap_or(Value::Null);
    canon_json(&edit_result_to_json(&apply_edit(&doc, &op, app_tokens())))
}

/// [`island_opacity`] of a document given as JSON text, with the app's token list: `{"opacity":n}` or
/// `{"opacity":null}` (mixed).
pub fn island_opacity_json(doc_json: &str) -> String {
    let doc = parse_json(doc_json).unwrap_or(Value::Null);
    canon_json(&json!({ "opacity": island_opacity(&doc, app_tokens()) }))
}

/// The editor's readability state (§13.5) with the app's token list: `{"issues":[…],"new":[…],"fixes":[…]}` —
/// `issues` of the working document, `new` = those that are not issues of the base, `fixes` = [`fixes_for`]`(new)`.
/// A base that is empty, not JSON or does not resolve has no issues. A working document that does not resolve (never
/// one [`apply_edit`] returned) gives empty lists and `"error":"refused: <engine error>"`.
pub fn contrast_json(doc_json: &str, base_json: &str) -> String {
    let spec = app_tokens();
    let doc = parse_json(doc_json).unwrap_or(Value::Null);
    let r = match resolve_theme(&doc, spec) {
        Ok(r) => r,
        Err(e) => {
            return canon_json(&json!({ "error": format!("refused: {e}"), "issues": [], "new": [], "fixes": [] }));
        }
    };
    let issues = contrast_issues(&r);
    let base = parse_json(base_json)
        .ok()
        .and_then(|b| resolve_theme(&b, spec).ok())
        .map(|b| contrast_issues(&b))
        .unwrap_or_default();
    let fresh = new_issues(&issues, &base);
    let fixes = fixes_for(&fresh, &r);
    let list = |v: &[ContrastIssue]| Value::Array(v.iter().map(ContrastIssue::to_json).collect());
    canon_json(&json!({ "issues": list(&issues), "new": list(&fresh), "fixes": fixes }))
}
