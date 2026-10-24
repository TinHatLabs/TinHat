//! actions_ctl.rs — ActionsController (11.0 step 3, buttons): the actions engine's QML face (ACTIONS.md §11, §14.3).
//!
//! The engine is `crates/actions` (tinhat-actions), a byte-identical Rust port of Tin Hat Web's TypeScript reference
//! (`src/actions/`), proven by the shared vectors. Every method takes JSON text and answers the canonical compact JSON
//! the reference computes, so QML does `JSON.parse(actionsCtl.row(kind, JSON.stringify(value), snapshotJson, "null"))`
//! the way the web's ButtonRow calls evalRow. The QML side: ActionButton.qml / ButtonRow.qml (rows), QuickSwitcher.qml
//! (the Ctrl+K palette), ActionsHost.qml (the live snapshot, the run path and the id → command map).
//!
//! Every QML-facing function is `#[qinvokable]` (a `#[cxx_name]` alone compiles and is invisible to QML;
//! scripts/qml-boundary.py checks it). A panic inside the engine — a bug; the engine answers every input with a value —
//! answers `{"error":"internal: <method>"}` instead of unwinding into Qt.

use cxx_qt_lib::QString;

#[cxx_qt::bridge]
pub mod qobject {
    extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[auto_cxx_name] // snake_case Rust -> camelCase QML
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        type ActionsController = super::ActionsControllerRust;

        /// QML: actionsCtl.call(kind, inputJson) — any entry point of the engine by name (evaluate, row, menu, place,
        /// target, palette, choices, fold, score, drawer, summary), one JSON value in, one out (ACTIONS.md §11).
        #[qinvokable]
        fn call(self: &ActionsController, kind: &QString, input_json: &QString) -> QString;

        /// QML: actionsCtl.row(rowKind, valueJson, snapshotJson, contextJson) — a layout row value resolved for drawing
        /// (§9.4): `{row, look, scale, style, editable, items: [ItemOut]}`; `contextJson` "null" = none.
        #[qinvokable]
        fn row(self: &ActionsController, row_kind: &QString, value_json: &QString, snapshot_json: &QString, context_json: &QString) -> QString;

        /// QML: actionsCtl.present(action, snapshotJson, targetJson, contextJson, transient) — the presentation of one
        /// action (§7.5): what the run path checks before running it (enabled, pick, confirm, subject, params).
        #[qinvokable]
        fn present(self: &ActionsController, action: &QString, snapshot_json: &QString, target_json: &QString, context_json: &QString, transient: bool) -> QString;

        /// QML: actionsCtl.palette(snapshotJson, query, stage) — the Ctrl+K palette (§10): `{mode, query, stage, total,
        /// items}`; `stage` "" = everything, else the action whose target is being picked.
        #[qinvokable]
        fn palette(self: &ActionsController, snapshot_json: &QString, query: &QString, stage: &QString) -> QString;

        /// QML: actionsCtl.drawer(rowKind, snapshotJson) — the edit-mode drawer of a row kind (§9.6): `{groups}`.
        #[qinvokable]
        fn drawer(self: &ActionsController, row_kind: &QString, snapshot_json: &QString) -> QString;

        /// QML: actionsCtl.place(rowKind, itemJson) — may this item go into the row (§9.3): `{ok, item}` | `{ok, error, key?}`.
        #[qinvokable]
        fn place(self: &ActionsController, row_kind: &QString, item_json: &QString) -> QString;

        /// QML: actionsCtl.defaultRow(island, row, form, preset) — the preset's default row value (§9.8; default-rows.json),
        /// "null" when the preset has none (a `buttons` island starts empty). Unknown preset → classic.
        #[qinvokable]
        fn default_row(self: &ActionsController, island: &QString, row: &QString, form: &QString, preset: &QString) -> QString;

        /// QML: actionsCtl.tableJson() — the table (actions.json, the contract copy): labels, groups, the palette's texts.
        #[qinvokable]
        fn table_json(self: &ActionsController) -> QString;

        /// QML: actionsCtl.envShortcuts() — the TINHAT_SHORTCUTS environment variable (a user key document, ACTIONS.md §8.5,
        /// as JSON text, or `@<file>`; "" when unset): the review captures pin this device's keys with it — shown, never saved (like
        /// TINHAT_LAYOUT / TINHAT_WIDGETS).
        #[qinvokable]
        fn env_shortcuts(self: &ActionsController) -> QString;
    }
}

#[derive(Default)]
pub struct ActionsControllerRust {}

const TABLE_JSON: &str = include_str!("../../actions/actions.json");
const DEFAULT_ROWS_JSON: &str = include_str!("../../actions/default-rows.json");

/// Run an engine call; a panic (a bug) becomes an error answer instead of unwinding into Qt.
fn guarded(method: &str, f: impl FnOnce() -> String + std::panic::UnwindSafe) -> QString {
    // selfhost: the server gates are laid over the table before the engine's first use (server_offer.rs)
    crate::server_offer::ensure_actions_overlay();
    let s = std::panic::catch_unwind(f).unwrap_or_else(|e| {
        let why = e.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| e.downcast_ref::<String>().cloned()).unwrap_or_default();
        eprintln!("[actions] {method} panicked: {why}");
        format!(r#"{{"error":"internal: {method}"}}"#)
    });
    QString::from(s.as_str())
}

/// A JSON string literal of `s` (for the engine inputs this controller assembles).
fn jstr(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}
/// Raw JSON text from QML, or `null` when empty (JavaScript's undefined).
fn raw(s: &str) -> &str {
    if s.trim().is_empty() {
        "null"
    } else {
        s
    }
}

// The invokables as plain functions of QStrings (the unit tests call these; the QObject methods forward to them).
pub fn call(kind: &QString, input_json: &QString) -> QString {
    let (k, i) = (kind.to_string(), input_json.to_string());
    guarded("call", move || tinhat_actions::call(&k, &i))
}
pub fn row(row_kind: &QString, value_json: &QString, snapshot_json: &QString, context_json: &QString) -> QString {
    let input = format!(
        r#"{{"app":"desktop","row":{},"value":{},"snapshot":{},"context":{}}}"#,
        jstr(&row_kind.to_string()),
        raw(&value_json.to_string()),
        raw(&snapshot_json.to_string()),
        raw(&context_json.to_string())
    );
    guarded("row", move || tinhat_actions::call("row", &input))
}
pub fn present(action: &QString, snapshot_json: &QString, target_json: &QString, context_json: &QString, transient: bool) -> QString {
    let t = target_json.to_string();
    let c = context_json.to_string();
    let mut input = format!(r#"{{"app":"desktop","action":{},"snapshot":{}"#, jstr(&action.to_string()), raw(&snapshot_json.to_string()));
    // absent target / context = not given (the engine's undefined), not null
    if raw(&t) != "null" {
        input.push_str(&format!(r#","target":{t}"#));
    }
    if raw(&c) != "null" {
        input.push_str(&format!(r#","context":{c}"#));
    }
    if transient {
        input.push_str(r#","transient":true"#);
    }
    input.push('}');
    guarded("present", move || tinhat_actions::call("evaluate", &input))
}
pub fn palette(snapshot_json: &QString, query: &QString, stage: &QString) -> QString {
    let st = stage.to_string();
    let mut input = format!(r#"{{"app":"desktop","snapshot":{},"query":{}"#, raw(&snapshot_json.to_string()), jstr(&query.to_string()));
    if !st.is_empty() {
        input.push_str(&format!(r#","stage":{}"#, jstr(&st)));
    }
    input.push('}');
    guarded("palette", move || tinhat_actions::call("palette", &input))
}
pub fn drawer(row_kind: &QString, snapshot_json: &QString) -> QString {
    let input = format!(r#"{{"app":"desktop","row":{},"snapshot":{}}}"#, jstr(&row_kind.to_string()), raw(&snapshot_json.to_string()));
    guarded("drawer", move || tinhat_actions::call("drawer", &input))
}
pub fn place(row_kind: &QString, item_json: &QString) -> QString {
    let input = format!(r#"{{"row":{},"item":{}}}"#, jstr(&row_kind.to_string()), raw(&item_json.to_string()));
    guarded("place", move || tinhat_actions::call("place", &input))
}
pub fn default_row(island: &QString, row: &QString, form: &QString, preset: &QString) -> QString {
    let (isl, r, f, p) = (island.to_string(), row.to_string(), form.to_string(), preset.to_string());
    guarded("defaultRow", move || {
        let doc: serde_json::Value = serde_json::from_str(DEFAULT_ROWS_JSON).unwrap_or(serde_json::Value::Null);
        let presets = &doc["presets"];
        let preset = if presets.get(&p).is_some() { &presets[&p] } else { &presets["classic"] };
        let form = if f == "phone" { "phone" } else { "desktop" };
        let kind = isl.split(':').next().unwrap_or("");
        let v = &preset[form]["opts"][kind]["rows"][&r];
        if v.is_null() {
            "null".into()
        } else {
            serde_json::to_string(v).unwrap_or_else(|_| "null".into())
        }
    })
}
pub fn table_json() -> QString {
    QString::from(TABLE_JSON)
}

impl qobject::ActionsController {
    pub fn call(&self, kind: &QString, input_json: &QString) -> QString {
        call(kind, input_json)
    }
    pub fn row(&self, row_kind: &QString, value_json: &QString, snapshot_json: &QString, context_json: &QString) -> QString {
        row(row_kind, value_json, snapshot_json, context_json)
    }
    pub fn present(&self, action: &QString, snapshot_json: &QString, target_json: &QString, context_json: &QString, transient: bool) -> QString {
        present(action, snapshot_json, target_json, context_json, transient)
    }
    pub fn palette(&self, snapshot_json: &QString, query: &QString, stage: &QString) -> QString {
        palette(snapshot_json, query, stage)
    }
    pub fn drawer(&self, row_kind: &QString, snapshot_json: &QString) -> QString {
        drawer(row_kind, snapshot_json)
    }
    pub fn place(&self, row_kind: &QString, item_json: &QString) -> QString {
        place(row_kind, item_json)
    }
    pub fn default_row(&self, island: &QString, row: &QString, form: &QString, preset: &QString) -> QString {
        default_row(island, row, form, preset)
    }
    pub fn table_json(&self) -> QString {
        table_json()
    }
    pub fn env_shortcuts(&self) -> QString {
        QString::from(crate::layout_ctl::env_text("TINHAT_SHORTCUTS").trim())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(s: &str) -> QString {
        QString::from(s)
    }

    #[test]
    fn a_row_resolves_through_the_engine() {
        let snap = r#"{"values":{"call.in":true,"call.micMuted":true}}"#;
        let out = row(&q("buttons.main"), &q(r#"{"items":[{"a":"call.mic.toggle"},{"a":"call.teleport"}]}"#), &q(snap), &q(""));
        let v: serde_json::Value = serde_json::from_str(&out.to_string()).unwrap();
        assert_eq!(v["items"][0]["label"], "Unmute");
        assert_eq!(v["items"][0]["state"], "shown");
        assert_eq!(v["items"][1]["known"], false);
        assert_eq!(v["items"][1]["state"], "dimmed");
    }

    #[test]
    fn present_passes_target_and_context_only_when_given() {
        let snap = r#"{"current":{"room":"!r:x"},"subjects":{"room":{"!r:x":{"name":"R","values":{"room.joined":true}}}}}"#;
        let a: serde_json::Value = serde_json::from_str(&present(&q("room.leave"), &q(snap), &q(""), &q("null"), false).to_string()).unwrap();
        assert_eq!(a["subject"], "!r:x");
        assert_eq!(a["confirm"]["title"], "Leave room?");
    }

    #[test]
    fn the_palette_and_default_rows() {
        let p: serde_json::Value = serde_json::from_str(&palette(&q("{}"), &q(">mute"), &q("")).to_string()).unwrap();
        assert_eq!(p["mode"], "action");
        let d: serde_json::Value = serde_json::from_str(&default_row(&q("header"), &q("main"), &q("desktop"), &q("classic")).to_string()).unwrap();
        assert_eq!(d["items"][0]["a"], "reel.open");
        assert_eq!(default_row(&q("buttons:a1"), &q("main"), &q("desktop"), &q("nope")).to_string(), "null");
        assert!(table_json().to_string().contains("\"tinhat-actions\""));
    }
}
