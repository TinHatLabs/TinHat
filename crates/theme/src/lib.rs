//! # tinhat-theme — the Tin Hat theme engine (desktop)
//!
//! Rust port of the TypeScript reference (Tin Hat Web, `src/theme/`), contract `ENGINE.md` v1 + `ENGINE-QUESTIONS.md`
//! (binding) + `NAMING.md`. Pure: no Qt, no I/O, no clock, no randomness — data in, data out. **The app calls
//! [`resolve_for_app`]`(theme, `[`app_tokens`]`(), App::Desktop)`** (the embedded `tokens.json`; built-ins:
//! [`builtin_theme`]; theme JSON text: [`parse_theme_json`]); a refusal is `"<code>: <detail>"`, exactly the web's
//! text.
//!
//! Tests (`cargo test -p tinhat-theme`): `tests/vectors.rs` — every shared vector byte for byte (engine incl. the
//! library's SHA-256 token digests, app, import; the copies equal the contract); `tests/unit.rs` — the reference's
//! unit tests mirrored test by test, plus its exact outputs on edge inputs (`tests/data/reference-edges.json`, bit for
//! bit); `tests/reference.rs` — differential cases generated from the reference (`tests/data/reference-fuzz.json`);
//! `tests/editor.rs` — the theme editor + share-card vectors (`testvectors/editor/`, `share/`) byte for byte and the
//! reference's `editor.test.ts`.
//! The data files are byte copies of the contract (`sync-vectors.sh`) or generated from the web reference by the
//! scripts in `tests/data/` (see their headers). Where the port cannot or should not be identical: contract
//! `PORT-QUESTIONS.md`.
//!
//! Module ↔ reference file (ported line by line, same names where Rust allows):
//!
//! | module | reference | what |
//! | --- | --- | --- |
//! | [`literal`] | `literal.ts` | [`Colour`], literals, canonical text form (§1) |
//! | [`maths`] | `maths.ts` | sRGB tables, cbrt, OKLab, luminance, contrast, `mixok` (§2) |
//! | [`expr`] | `expr.ts` | expression parser, fixed error texts (§3) |
//! | [`ops`] | `ops.ts` + `ui/color.ts` | `alpha mix lighter darker over best guard`, `cr` |
//! | [`roles`] | `roles.ts` | role ids, palette slots, guard table, §5 derivation (data) |
//! | [`limits`] | `limits.ts` | size / string / depth / URL / mxc / id rules |
//! | [`engine`] | `engine.ts` | [`resolve_theme`] (§4–6) |
//! | [`import`] | `import.ts` | [`import_theme`] (§7) |
//! | `parsers` | `parsers.ts` | JSONC, plist, TOML, YAML subsets |
//! | [`canon`] | `canon.ts` | canonical JSON (vector format), ECMAScript number/string printing |
//! | [`json`] | `JSON.parse` | strict JSON → `serde_json::Value` with JavaScript's number semantics |
//! | [`labels`] | `labels.ts` | the theme editor's labels, groups and search (`editor-labels.json`, THEMES-UI.md §13.8) |
//! | [`shape`] | `shape.ts` | a theme's `shape` block: island radius, gap, margin, border (step 1b) |
//! | [`sync`] | `sync.ts` | THEMES-SYNC.md: `dev.tinhat.theme` / `dev.tinhat.themes` — read, save, merge, the device protocol |
//! | [`editor`] | `editor.ts` | THEMES-UI.md §13: the theme editor — edits, history, readability, save / export |
//! | [`look`] | `look.ts` | ENGINE.md §10: the look beyond colours — island shadow, density, fonts, text size |
//! | [`share`] | `share.ts` | THEMES-UI.md §14: theme share cards — the message, reading one back, Keep |
//! | [`probe`] | `probe.ts` | THEMES-UI.md §15: the Pick tool's probe colours and the Pick list |
//! | [`recent`] | `recent.ts` | THEMES-UI.md §12: the ☰ Theme menu's recent themes (per device) — read, record a pick, the menu's rows |
//!
//! ```
//! use tinhat_theme::{import_theme, resolve_theme, Colour, TokensSpec};
//! let file = "background #1e1e2e\nforeground #cdd6f4\ncolor0 #45475a\ncolor1 #f38ba8\ncolor2 #a6e3a1\n\
//!             color3 #f9e2af\ncolor4 #89b4fa\ncolor5 #f5c2e7\ncolor6 #94e2d5\ncolor7 #bac2de\ncolor8 #585b70\n\
//!             color9 #f38ba8\ncolor10 #a6e3a1\ncolor11 #f9e2af\ncolor12 #89b4fa\ncolor13 #f5c2e7\n\
//!             color14 #94e2d5\ncolor15 #a6adc8\n";
//! let imported = import_theme(file, Some("Mocha.conf")).unwrap();
//! let resolved = resolve_theme(&imported.theme, &TokensSpec::empty()).unwrap();
//! let accent: Colour = resolved.colour("accent.base").unwrap();
//! let (r, g, b, a) = accent.rgba(); // for Qt.rgba(r / 255, g / 255, b / 255, a)
//! assert_eq!((r, g, b, a), (0x89, 0xb4, 0xfa, 1.0));
//! ```

pub mod canon;
pub mod editor;
pub mod engine;
pub mod expr;
pub mod import;
mod js;
pub mod json;
pub mod labels;
pub mod limits;
pub mod literal;
pub mod look;
pub mod maths;
pub mod ops;
mod parsers;
pub mod probe;
pub mod recent;
pub mod roles;
pub mod shape;
pub mod share;
pub mod sync;

/// SHA-256 for the unit tests (the differential corpus digests), shared with the integration tests.
#[cfg(test)]
#[path = "../tests/common/sha256.rs"]
mod test_sha256;

pub use canon::{canon_json, pretty_json};
pub use engine::{
    app_result_to_json, app_tokens, builtin_id, builtin_theme, parse_theme_json, resolve_for_app, resolve_for_app_json,
    resolve_theme, resolve_theme_json, result_to_json, App, AppResolved, NamesMode, Refusal, Resolved, TokensSpec,
    Variant,
};
pub use import::{import_result_to_json, import_theme, ImportError, ImportFormat, ImportResult};
pub use json::parse_json;
pub use literal::{format_colour, parse_literal, Colour};

/// `Math.round` exactly (ENGINE-QUESTIONS A4), exposed for the port's own tests and for callers that must round like
/// the reference.
pub fn js_round(x: f64) -> f64 {
    js::js_round(x)
}
