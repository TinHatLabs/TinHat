//! # tinhat-actions — the Tin Hat actions engine (desktop)
//!
//! A line-for-line Rust port of Tin Hat Web's TypeScript reference (`src/actions/{table,evaluate,palette,api}.ts`),
//! contract `ACTIONS.md` v1 (byte copy in this crate): what every action is, whether
//! it applies and how it looks right now (availability, targets, presentation), the button rows and menus, the
//! edit-mode drawer and the Ctrl+K palette. Pure: no Qt, no I/O (the table `actions.json` is embedded), no clock, no
//! randomness — JSON data in, JSON data out. **The app calls [`call`]`(kind, json)`** (JSON text in, canonical JSON
//! text out; ACTIONS.md §11), or the typed entry points ([`evaluate`], [`row`], [`menu`], [`place`], [`target`],
//! [`palette()`], [`choices`], [`fold()`], [`score`], [`drawer`], [`summary`]: `&serde_json::Value` in, `Value` out).
//!
//! Tests (`cargo test -p tinhat-actions`): `tests/vectors.rs` — every shared vector byte for byte (canonical JSON; the
//! copies equal the contract); `tests/unit.rs` — the reference's hand-written tests mirrored (the worked examples of
//! ACTIONS.md §13, the table's invariants, robustness). Contract copies: `sync-contract.sh`.
//!
//! Module ↔ reference file (ported line by line, same names where Rust allows):
//!
//! | module | reference | what |
//! | --- | --- | --- |
//! | [`table`](mod@table) | `table.ts` | the table (embedded, read once), typed like `table.ts`'s interfaces |
//! | [`eval`] | `evaluate.ts` | rules, values, params, targets, presentation, rows, menus, the drawer |
//! | [`palette`](mod@palette) | `palette.ts` | folding, tiers, entries, ranking, stage 2 |
//! | [`keys`](mod@keys) | `keys.ts` | key sequences: parsing, canonical text, the web's key classes (§8.1) |
//! | [`bindings`](mod@bindings) | `bindings.ts` | default and user key bindings, checks, clashes, matching, Settings → Shortcuts (§8) |
//! | [`api`] | `api.ts` | JSON in / JSON out, `{"error": "<code>"}` for a malformed input |
//! | [`js`] | `layout/json.ts`, `theme/canon.ts` | own-key JSON access, `===`, code-point order |
//!
//! Numbers are doubles (JSON numbers), printed like `JSON.stringify` by the theme crate's canonical printer; strings
//! are measured in code points; every ordered walk is explicit (code-point order of keys, or an array's own order) —
//! never a map's iteration order.

pub mod api;
pub mod bindings;
pub mod eval;
pub mod js;
pub mod keys;
pub mod palette;
pub mod table;

pub use api::{choices, dispatch, drawer, evaluate, fold, menu, palette, place, row, score, summary, target, KINDS};
pub use table::{action_by_id, table, Action, App, Table};

use serde_json::Value;

/// ACTIONS.md §11: the entry point `kind` (one of [`KINDS`]) on the JSON text `input`, answered as canonical compact
/// JSON (keys in code-point order, numbers like `JSON.stringify`). Text that is not JSON data (empty, malformed,
/// nested deeper than 128) reads as an absent value, which every entry point answers `{"error":"bad-input"}`; an
/// unknown `kind` answers `{"error":"unknown-kind"}`. Never panics.
pub fn call(kind: &str, input: &str) -> String {
    let v = tinhat_theme::json::parse_json(input).unwrap_or(Value::Null);
    let out = match api::dispatch(kind, &v) {
        Some(out) => out,
        None => {
            let mut o = js::obj();
            o.insert("error".into(), Value::String("unknown-kind".into()));
            Value::Object(o)
        }
    };
    tinhat_theme::canon::canon_json(&out)
}
