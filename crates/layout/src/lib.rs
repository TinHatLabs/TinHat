//! # tinhat-layout — the Tin Hat layout engine (desktop)
//!
//! Rust port of the TypeScript reference (Tin Hat Web, `src/layout/`, commits 197b1fa / 92970b7 / 91c694c), contract
//! `LAYOUT.md` v1 + `LAYOUT-QUESTIONS.md` (islands plan C1). Pure: no Qt, no I/O, no clock, no randomness — data in,
//! data out; no floating point in solve (i64 everywhere). **The app calls it through [`api`]** (JSON text in / JSON
//! text out — `crates/app/src/layout_ctl.rs`, the `LayoutController` QML type).
//!
//! Tests (`cargo test -p tinhat-layout`): `tests/vectors.rs` — every shared vector byte for byte (1,135: solve, op,
//! dropzones, repair, validate, save, merge, sync, migrate) + the copies equal the contract + the independent checker
//! (`tools/layout/invariants.mjs`, run by node) over the port's own solve outputs; `tests/unit.rs` — the reference's
//! unit tests mirrored; `tests/differential.rs` — a seeded random run through a node bridge to the TypeScript engine
//! (`tools/layout/rust-bridge/`), every result compared canonically and checked by the independent checker. Where
//! the port cannot or should not be identical: contract `LAYOUT-QUESTIONS.md`.
//!
//! Module ↔ reference file (ported line by line, same names where Rust allows):
//!
//! | module | reference | what |
//! | --- | --- | --- |
//! | [`json`] | `json.ts` | the JavaScript-like value [`J`] (shared mutable objects, deep copies), reading, canonical printing |
//! | [`schema`] | `schema.ts` | registry, limits, bounds, presets, identifier syntax (§2) |
//! | [`form`] | `form.ts` | node kinds, locations, sizes, placement rules, insertion, home rules, ids (§3) |
//! | [`normalize`] | `normalize.ts` | N1-N9 (§4) |
//! | [`repair`] | `repair.ts` | repair + validate, desktop and phone (§5) |
//! | [`ops`] | `ops.ts` | transactional operations (§8) |
//! | [`solve`] | `solve.ts` | the integer solver incl. collapse, drawers, floats, the emergency arrangement (§7) |
//! | [`dropzones`] | `dropzones.ts` | drop zones (§8.3) |
//! | [`sync`] | `sync.ts` | the layouts document: tuples, vv, save, merge, the device protocol (§9) |
//! | [`migrate`] | `migrate.ts` | 10.6 prefs → view state, the seam conversion (§6.3) |
//! | [`session`] | `features/editmode/session.ts` | the edit session + the host logic that chooses operations (EDITMODE-UI) |
//! | [`api`] | — | JSON text in / out for the LayoutController |

pub mod api;
pub mod dropzones;
pub mod form;
pub mod json;
pub mod migrate;
pub mod normalize;
pub mod ops;
pub mod repair;
pub mod schema;
pub mod session;
pub mod solve;
pub mod sync;

pub use dropzones::{drop_zones, Zone, Zones};
pub use json::{read_json, J};
pub use migrate::{migrate, seam_pm};
pub use normalize::normalize;
pub use ops::{apply_op, OpResult};
pub use repair::{
    repair_desktop, repair_desktop_text, repair_phone, repair_phone_text, validate_desktop, validate_phone, RepairResult,
    INFO_NOTES,
};
pub use schema::{classic_desktop, classic_phone, internal_desktop, preset_desktop, preset_phone, PRESET_NAMES, REGION_ORDER};
pub use solve::{sc, solve, solve_detail, unscale, SolveOut};
pub use sync::{known_of, merge, project, run_sync, save, Device, Projection, SaveReq, SaveResult};
