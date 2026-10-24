//! tinhat-widgets — the widgets' shared logic (modular UI step 2, contract `WIDGETS.md`).
//!
//! A line-for-line port of Tin Hat Web's TypeScript reference — `src/widgets/{quality,rxq,time,registry,history}.ts`
//! — proven identical by the shared vectors (`testvectors/widgets/**`, byte-equal copies of the contract's, refreshed
//! by `sync-vectors.sh`; `tests/vectors.rs` runs every case). Pure: no Qt, no I/O. The streamer uses [`quality`] and
//! [`rxq`] (receive stats, the `tinhat.rxq` report both ways); the app uses [`history`], [`time`] and [`registry`].
//!
//! Also the low-GPU contract's pure rules (GPU.md §1–§2, modular UI step 3): [`lowgpu`] (window state, the motion switch,
//! the duration token, timers, the ring, the busy mark, GIF runs, level bars, the unfocused call-video policy) and
//! [`animimg`] (a GIF / WebP's frames, loop length and loop count) — ports of the web's `src/core/{lowgpu,animimg}.ts`
//! with the same test case names.
//!
//! Numbers follow the reference: IEEE doubles evaluated in the order written (both languages round + − × ÷ the same
//! way), integers rounded with [`quality::r`] = `x ≤ 0 ? 0 : floor(x + 0.5)`, printed like ECMAScript's
//! `Number::toString` ([`js_number`]). Strings compare in code-point order (= Rust's `str` order).

pub mod animimg;
pub mod clocks;
pub mod dice;
pub mod gamenight; // widgets W2: Game night (WIDGETS.md §23)
pub mod gameservers;
pub mod history;
pub mod json;
pub mod justme; // widgets W2: "Is it just me?" (WIDGETS.md §20)
pub mod lowgpu;
pub mod memories; // widgets W2: Memories (WIDGETS.md §21)
pub mod mentions;
pub mod note;
pub mod people;
pub mod polls;
pub mod quality;
pub mod recent;
pub mod registry;
pub mod rxq;
pub mod soundpad; // widgets W2: the Soundboard pad (WIDGETS.md §22)
pub mod time;

pub use tinhat_theme::canon::js_number;

/// `Math.min(a, b)` (NaN wins, unlike `f64::min`).
pub(crate) fn js_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else if a < b {
        a
    } else {
        b
    }
}

/// `Math.max(a, b)` (NaN wins, unlike `f64::max`).
pub(crate) fn js_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else if a > b {
        a
    } else {
        b
    }
}

/// A JSON number the way the reference holds it (integral doubles as integers — the canonical JSON is the same).
pub fn num(x: f64) -> serde_json::Value {
    tinhat_theme::json::number_value(x)
}

/// `typeof v === "number" && Number.isFinite(v)` → the double.
pub(crate) fn finite(v: Option<&serde_json::Value>) -> Option<f64> {
    v.and_then(serde_json::Value::as_f64).filter(|x| x.is_finite())
}
