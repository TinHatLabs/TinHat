//! layout_ctl.rs — LayoutController (islands C1, desktop): the layout engine's QML face.
//!
//! The engine is `crates/layout` (tinhat-layout), a byte-identical Rust port of Tin Hat Web's TypeScript reference
//! (`src/layout/`, contract LAYOUT.md). Every method takes JSON text and answers the canonical compact JSON of an object
//! — exactly the value the reference computes, so QML does `JSON.parse(layoutCtl.solve(JSON.stringify(form),
//! JSON.stringify(ctx)))` the way the web host calls the TypeScript engine. An empty argument is absent (JavaScript's
//! `undefined`); argument semantics per method: `tinhat_layout::api`.
//!
//! A panic inside the engine — a bug; the engine answers every input with a value — answers
//! `{"ok":false,"error":"internal: <method>"}` instead of unwinding into Qt.
//!
//! 11.0 edit mode (EDITMODE-UI.md): the controller also holds the window's ONE edit session (`edit*` —
//! `tinhat_layout::session`, the web's session.ts: undo / redo over whole forms, Cancel, Done once, the host logic that
//! chooses operations) and is the QML face of the synced layouts (`store*` — layout_store.rs: which layout renders, the
//! saves, the conflicts; `storeRev` bumps whenever QML should re-read `storeView`).

use core::pin::Pin;

use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;
use tinhat_layout::api;
use tinhat_layout::json::{mk, read_json, J};
use tinhat_layout::session::{self as es, EditSession, FormKind, Rect};

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
        #[qproperty(i32, store_rev)] // QML: storeRev — bumps whenever the synced layouts changed (re-read storeView)
        #[qproperty(i32, edit_rev)] //  QML: editRev — bumps on every change of the edit session (re-read editState)
        type LayoutController = super::LayoutControllerRust;

        /// QML: layoutCtl.load(docJson, activeId) — the projection of the account-data document
        /// `dev.tinhat.layouts` (LAYOUT.md §9.4): `{read, layouts, active, name, desktop, phone, notes: {desktop,
        /// phone}, conflicts, quarantine}` (the active layout's concurrent losers and whether it keeps an original this
        /// version could not read — the host shows both); not JSON data / no document → the built-in Classic.
        #[qinvokable]
        fn load(self: &LayoutController, doc_json: &QString, active_id: &QString) -> QString;

        /// QML: layoutCtl.repair(target, text) — `desktop` | `phone`: a valid projection of any JSON text,
        /// `{form, notes}` (§5); `document`: as load(text, "").
        #[qinvokable]
        fn repair(self: &LayoutController, target: &QString, text: &QString) -> QString;

        /// QML: layoutCtl.validate(target, formJson) — `{violations}` (§5.4; empty = valid).
        #[qinvokable]
        fn validate(self: &LayoutController, target: &QString, form_json: &QString) -> QString;

        /// QML: layoutCtl.solve(formJson, ctxJson) — ctx `{size, unitPm, shape, content, view}`; the §7.13 output
        /// (every island's rect, stacks, containers, drawers, gap handles, collapsed / overflow / squeezed).
        #[qinvokable]
        fn solve(self: &LayoutController, form_json: &QString, ctx_json: &QString) -> QString;

        /// QML: layoutCtl.preset(name) — `{form}`: a built-in desktop form by name, or the internal `classic-flush`
        /// (the dock host's form until layouts are stored, ISLANDS-ARCH C4/C5); `{form: null}` for an unknown name.
        #[qinvokable]
        fn preset(self: &LayoutController, name: &QString) -> QString;

        /// QML: layoutCtl.applyOp(formJson, opJson, ctxJson) — one transactional operation (§8):
        /// `{ok: true, form, remap}` | `{ok: false, error}`; ctx (resize / resetSizes) as for solve.
        #[qinvokable]
        fn apply_op(self: &LayoutController, form_json: &QString, op_json: &QString, ctx_json: &QString) -> QString;

        /// QML: layoutCtl.dropZones(formJson, ctxJson, dragged) — `{hit, keys}` (§8.3): the zones of a drag in hit-test
        /// and keyboard order, each `{kind, rect, op}` with an operation that succeeds.
        #[qinvokable]
        fn drop_zones(self: &LayoutController, form_json: &QString, ctx_json: &QString, dragged: &QString) -> QString;

        /// QML: layoutCtl.save(rawJson, knownJson, reqJson) — a new entry that wins its own merge + the document to
        /// put (§9.5), req `{id, edit, start?, also?, now, by, seq, key?}`: `{ok: true, entry, doc}` |
        /// `{ok: true, noop: true}` | `{ok: false, error}`.
        #[qinvokable]
        fn save(self: &LayoutController, raw_json: &QString, known_json: &QString, req_json: &QString) -> QString;

        /// QML: layoutCtl.merge(rawJson, knownJson, justSaved) — the join to put (§9.6): `{doc, tooLarge}`.
        #[qinvokable]
        fn merge(self: &LayoutController, raw_json: &QString, known_json: &QString, just_saved: &QString) -> QString;

        /// QML: layoutCtl.migrate(argsJson) — 10.6 prefs → the view state (§6.3), args `{prefs, H, L, chromeH}`:
        /// `{view}`.
        #[qinvokable]
        fn migrate(self: &LayoutController, args_json: &QString) -> QString;

        /// QML: layoutCtl.envForm() — the TINHAT_LAYOUT environment variable (a desktop form as JSON text, or `@<file>` = the
        /// file's text; "" when unset):
        /// the review captures pin the device's layout with it (LayoutStore.qml: shown, never saved — like TINHAT_WIDGETS).
        #[qinvokable]
        fn env_form(self: &LayoutController) -> QString;

        // ---- 11.0 edit mode: the synced layouts (layout_store.rs) ----
        /// QML: layoutCtl.startSync() — this controller hears every later change of the synced layouts (storeRev).
        #[qinvokable]
        fn start_sync(self: Pin<&mut LayoutController>);
        /// QML: layoutCtl.storeView(activeId) — what this device renders from its synced state: `{signedIn, sync,
        /// synced, rev, id, name, desktop, phone, source: "doc" | "none", readOnly, layouts: [{id, name}], hasPrevious,
        /// previous, conflict, tuple, lost}`.
        #[qinvokable]
        fn store_view(self: &LayoutController, active_id: &QString) -> QString;
        /// QML: layoutCtl.storeSave(id, editJson) — one save (§16): "saved" | "noop" | "read-only" | "busy" (an edit
        /// session owns that layout) | "signed-out" | error.
        #[qinvokable]
        fn store_save(self: &LayoutController, id: &QString, edit_json: &QString) -> QString;
        /// QML: layoutCtl.storeSaveDone(id, editJson) — the Done of the edit session that owns layout `id`.
        #[qinvokable]
        fn store_save_done(self: &LayoutController, id: &QString, edit_json: &QString) -> QString;
        /// QML: layoutCtl.storeBegin(id) — an edit session opens on layout `id` (LAYOUT §9.7: the sync device's begin).
        #[qinvokable]
        fn store_begin(self: &LayoutController, id: &QString) -> QString;
        /// QML: layoutCtl.storeEnd(id) — the session on `id` ended without a save.
        #[qinvokable]
        fn store_end(self: &LayoutController, id: &QString);
        /// QML: layoutCtl.storeResolve(id, take) — Restore mine (take) / Dismiss of the layout's first conflict (§13).
        #[qinvokable]
        fn store_resolve(self: &LayoutController, id: &QString, take: bool) -> QString;
        /// QML: layoutCtl.storeTakeLost() — the layout ids whose save was replaced since the last call (JSON array).
        #[qinvokable]
        fn store_take_lost(self: &LayoutController) -> QString;
        /// QML: layoutCtl.storeDebug() — the device protocol's state (gates / the sync check).
        #[qinvokable]
        fn store_debug(self: &LayoutController) -> QString;
        /// QML: layoutCtl.syncEnabled() — false in gate mode (TINHAT_GUI_LAYOUTSYNC=0 / TINHAT_GUI_THEMESYNC=0).
        #[qinvokable]
        fn sync_enabled(self: &LayoutController) -> bool;

        // ---- 11.0 edit mode: the session (tinhat_layout::session) ----
        /// QML: layoutCtl.editOpen(startJson, kind) — start = `{id, name, desktop, phone}`, kind "desktop" | "phone";
        /// answers editState(). An open session is replaced.
        #[qinvokable]
        fn edit_open(self: Pin<&mut LayoutController>, start_json: &QString, kind: &QString) -> QString;
        /// QML: layoutCtl.editState() — `{open, kind, id, name, dirty, undo, redo, form}` (`{open: false}` when none).
        #[qinvokable]
        fn edit_state(self: &LayoutController) -> QString;
        /// QML: layoutCtl.editApply(opJson, ctxJson) — one engine operation as one undo step: `{ok}` | `{ok: false, error}`.
        #[qinvokable]
        fn edit_apply(self: Pin<&mut LayoutController>, op_json: &QString, ctx_json: &QString) -> QString;
        /// QML: layoutCtl.editCommit(formJson) — a form computed elsewhere (a gap drag's last result) as one undo step.
        #[qinvokable]
        fn edit_commit(self: Pin<&mut LayoutController>, form_json: &QString) -> bool;
        #[qinvokable]
        fn edit_undo(self: Pin<&mut LayoutController>) -> bool;
        #[qinvokable]
        fn edit_redo(self: Pin<&mut LayoutController>) -> bool;
        /// QML: layoutCtl.editPreset(name) — a preset pick (rows carried), "previous" = the start, "reset" = Classic.
        #[qinvokable]
        fn edit_preset(self: Pin<&mut LayoutController>, name: &QString) -> QString;
        /// QML: layoutCtl.editDock(ref) — a floating island back to its home (§4.3).
        #[qinvokable]
        fn edit_dock(self: Pin<&mut LayoutController>, island: &QString) -> QString;
        /// QML: layoutCtl.editFloat(ref, rectJson, hostW, hostH) — float at the nearest host corner, 16 px in (§4.1).
        #[qinvokable]
        fn edit_float(self: Pin<&mut LayoutController>, island: &QString, rect_json: &QString, host_w: i32, host_h: i32) -> QString;
        /// QML: layoutCtl.editResetIsland(ref) — "Reset this island" (§4.3).
        #[qinvokable]
        fn edit_reset_island(self: Pin<&mut LayoutController>, island: &QString) -> QString;
        #[qinvokable]
        fn edit_rename(self: Pin<&mut LayoutController>, name: &QString);
        /// QML: layoutCtl.editFinish() — Done: `{id, name, desktop, phone}` to save, or `null` when nothing changed.
        #[qinvokable]
        fn edit_finish(self: Pin<&mut LayoutController>) -> QString;
        /// QML: layoutCtl.editReopen() — the save of a Done failed: the session goes on exactly as it was.
        #[qinvokable]
        fn edit_reopen(self: Pin<&mut LayoutController>);
        /// QML: layoutCtl.editNewButtonIsland() — a new empty button island at its home (§7), one step: `{ok, ref}` |
        /// `{ok: false, error}` ("too-many" at 8).
        #[qinvokable]
        fn edit_new_button_island(self: Pin<&mut LayoutController>) -> QString;
        /// QML: layoutCtl.editRemoveButtonIsland(ref) — remove a button island (leaf + options), one step.
        #[qinvokable]
        fn edit_remove_button_island(self: Pin<&mut LayoutController>, island: &QString) -> QString;
        #[qinvokable]
        fn edit_cancel(self: Pin<&mut LayoutController>);
        /// QML: layoutCtl.hiddenRefs(formJson) — the hidden islands in stored order (JSON array).
        #[qinvokable]
        fn hidden_refs(self: &LayoutController, form_json: &QString) -> QString;
        /// QML: layoutCtl.pickPreset(formJson, name) — `{ok, form}` | `{ok: false, error}` (the preset menu's thumbnails).
        #[qinvokable]
        fn pick_preset(self: &LayoutController, form_json: &QString, name: &QString) -> QString;
    }

    impl cxx_qt::Threading for LayoutController {}
}

#[derive(Default)]
pub struct LayoutControllerRust {
    store_rev: i32,
    edit_rev: i32,
    /// the window's edit session (EDITMODE-UI §1: one working copy)
    session: Option<EditSession>,
}

/// layout_store.rs → the GUI thread: the synced layouts changed.
pub fn bump(o: Pin<&mut qobject::LayoutController>) {
    let r = *o.store_rev();
    o.set_store_rev(r.wrapping_add(1));
}

fn step_json(r: Result<(), String>) -> String {
    match r {
        Ok(()) => r#"{"ok":true}"#.to_string(),
        Err(e) => mk(vec![("ok", J::Bool(false)), ("error", J::s(&e))]).canon(),
    }
}

/// The session's state for QML (`{open: false}` when none or closed).
pub fn session_state(s: Option<&EditSession>) -> String {
    match s.filter(|s| !s.closed) {
        None => r#"{"open":false}"#.to_string(),
        Some(s) => mk(vec![
            ("open", J::Bool(true)),
            ("kind", J::s(if s.kind == FormKind::Phone { "phone" } else { "desktop" })),
            ("id", J::s(&s.id)),
            ("name", J::s(&s.name)),
            ("dirty", J::Bool(s.dirty())),
            ("undo", J::int(s.undo_depth() as i64)),
            ("redo", J::int(s.redo_depth() as i64)),
            ("form", s.form.clone()),
            ("start", s.start.clone()),
        ])
        .canon(),
    }
}

/// Run an engine call; a panic (a bug) becomes an error answer instead of unwinding into Qt.
fn guarded(method: &str, f: impl FnOnce() -> String + std::panic::UnwindSafe) -> QString {
    let s = std::panic::catch_unwind(f).unwrap_or_else(|e| {
        let why = e.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| e.downcast_ref::<String>().cloned()).unwrap_or_default();
        eprintln!("[layout] {method} panicked: {why}");
        format!(r#"{{"error":"internal: {method}","ok":false}}"#)
    });
    QString::from(s.as_str())
}

// The invokables as plain functions of QStrings (the unit tests call these; the QObject methods forward to them).
pub fn load(doc_json: &QString, active_id: &QString) -> QString {
    let (d, a) = (doc_json.to_string(), active_id.to_string());
    guarded("load", move || api::load(&d, &a))
}
pub fn repair(target: &QString, text: &QString) -> QString {
    let (t, x) = (target.to_string(), text.to_string());
    guarded("repair", move || api::repair(&t, &x))
}
pub fn validate(target: &QString, form_json: &QString) -> QString {
    let (t, f) = (target.to_string(), form_json.to_string());
    guarded("validate", move || api::validate(&t, &f))
}
pub fn solve(form_json: &QString, ctx_json: &QString) -> QString {
    let (f, c) = (form_json.to_string(), ctx_json.to_string());
    guarded("solve", move || api::solve(&f, &c))
}
pub fn apply_op(form_json: &QString, op_json: &QString, ctx_json: &QString) -> QString {
    let (f, o, c) = (form_json.to_string(), op_json.to_string(), ctx_json.to_string());
    guarded("applyOp", move || api::apply_op(&f, &o, &c))
}
pub fn drop_zones(form_json: &QString, ctx_json: &QString, dragged: &QString) -> QString {
    let (f, c, d) = (form_json.to_string(), ctx_json.to_string(), dragged.to_string());
    guarded("dropZones", move || api::drop_zones(&f, &c, &d))
}
pub fn save(raw_json: &QString, known_json: &QString, req_json: &QString) -> QString {
    let (r, k, q) = (raw_json.to_string(), known_json.to_string(), req_json.to_string());
    guarded("save", move || api::save(&r, &k, &q))
}
pub fn merge(raw_json: &QString, known_json: &QString, just_saved: &QString) -> QString {
    let (r, k, j) = (raw_json.to_string(), known_json.to_string(), just_saved.to_string());
    guarded("merge", move || api::merge(&r, &k, &j))
}
pub fn migrate(args_json: &QString) -> QString {
    let a = args_json.to_string();
    guarded("migrate", move || api::migrate(&a))
}

impl qobject::LayoutController {
    pub fn load(&self, doc_json: &QString, active_id: &QString) -> QString {
        load(doc_json, active_id)
    }
    pub fn repair(&self, target: &QString, text: &QString) -> QString {
        repair(target, text)
    }
    pub fn validate(&self, target: &QString, form_json: &QString) -> QString {
        validate(target, form_json)
    }
    pub fn solve(&self, form_json: &QString, ctx_json: &QString) -> QString {
        solve(form_json, ctx_json)
    }
    pub fn preset(&self, name: &QString) -> QString {
        let n = name.to_string();
        guarded("preset", move || api::preset(&n))
    }
    pub fn apply_op(&self, form_json: &QString, op_json: &QString, ctx_json: &QString) -> QString {
        apply_op(form_json, op_json, ctx_json)
    }
    pub fn drop_zones(&self, form_json: &QString, ctx_json: &QString, dragged: &QString) -> QString {
        drop_zones(form_json, ctx_json, dragged)
    }
    pub fn save(&self, raw_json: &QString, known_json: &QString, req_json: &QString) -> QString {
        save(raw_json, known_json, req_json)
    }
    pub fn merge(&self, raw_json: &QString, known_json: &QString, just_saved: &QString) -> QString {
        merge(raw_json, known_json, just_saved)
    }
    pub fn migrate(&self, args_json: &QString) -> QString {
        migrate(args_json)
    }
    pub fn env_form(&self) -> QString {
        QString::from(env_text("TINHAT_LAYOUT").as_str())
    }

    // ---- the synced layouts ----
    pub fn start_sync(self: Pin<&mut Self>) {
        crate::layout_store::attach_sink(self.qt_thread());
    }
    pub fn store_view(&self, active_id: &QString) -> QString {
        let a = active_id.to_string();
        guarded("storeView", move || crate::layout_store::view(&a))
    }
    pub fn store_save(&self, id: &QString, edit_json: &QString) -> QString {
        let (i, e) = (id.to_string(), edit_json.to_string());
        guarded("storeSave", move || crate::layout_store::save(&i, &e, false))
    }
    pub fn store_save_done(&self, id: &QString, edit_json: &QString) -> QString {
        let (i, e) = (id.to_string(), edit_json.to_string());
        guarded("storeSaveDone", move || crate::layout_store::save(&i, &e, true))
    }
    pub fn store_begin(&self, id: &QString) -> QString {
        let i = id.to_string();
        guarded("storeBegin", move || crate::layout_store::begin(&i))
    }
    pub fn store_end(&self, id: &QString) {
        crate::layout_store::end(&id.to_string());
    }
    pub fn store_resolve(&self, id: &QString, take: bool) -> QString {
        let i = id.to_string();
        guarded("storeResolve", move || crate::layout_store::resolve(&i, take))
    }
    pub fn store_take_lost(&self) -> QString {
        let v = crate::layout_store::take_lost();
        QString::from(J::arr(v.iter().map(|s| J::s(s)).collect()).canon().as_str())
    }
    pub fn store_debug(&self) -> QString {
        QString::from(crate::layout_store::debug_state().as_str())
    }
    pub fn sync_enabled(&self) -> bool {
        crate::layout_store::sync_enabled()
    }

    // ---- the edit session ----
    fn touched(mut self: Pin<&mut Self>) {
        let r = *self.edit_rev();
        self.as_mut().set_edit_rev(r.wrapping_add(1));
    }
    pub fn edit_open(mut self: Pin<&mut Self>, start_json: &QString, kind: &QString) -> QString {
        let st = read_json(&start_json.to_string()).filter(J::is_obj);
        let kind = if kind.to_string() == "phone" { FormKind::Phone } else { FormKind::Desktop };
        let s = st.map(|st| {
            let txt = |k: &str| st.get(k).and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
            let (id, name) = (txt("id"), txt("name"));
            let desktop = st.get("desktop").filter(J::is_obj).unwrap_or_else(tinhat_layout::classic_desktop);
            let phone = st.get("phone").filter(J::is_obj).unwrap_or_else(tinhat_layout::classic_phone);
            EditSession::new(if id.is_empty() { "main" } else { &id }, if name.is_empty() { "Main" } else { &name }, &desktop, &phone, kind)
        });
        self.as_mut().rust_mut().session = s;
        self.as_mut().touched();
        QString::from(session_state(self.session.as_ref()).as_str())
    }
    pub fn edit_state(&self) -> QString {
        QString::from(session_state(self.session.as_ref()).as_str())
    }
    fn with_session(mut self: Pin<&mut Self>, f: impl FnOnce(&mut EditSession) -> Result<(), String>) -> QString {
        let r = match self.as_mut().rust_mut().session.as_mut() {
            None => Err("closed".to_string()),
            Some(s) => {
                let before = (s.undo_depth(), s.redo_depth(), s.form.canon());
                let r = f(s);
                let changed = before != (s.undo_depth(), s.redo_depth(), s.form.canon());
                if changed {
                    self.as_mut().touched();
                }
                r
            }
        };
        QString::from(step_json(r).as_str())
    }
    pub fn edit_apply(self: Pin<&mut Self>, op_json: &QString, ctx_json: &QString) -> QString {
        let o = read_json(&op_json.to_string());
        let c = api::arg(&ctx_json.to_string());
        self.with_session(move |s| match o {
            None => Err("bad-op: bad request".into()),
            Some(o) => s.apply(&o, c.as_ref()),
        })
    }
    pub fn edit_commit(mut self: Pin<&mut Self>, form_json: &QString) -> bool {
        let Some(f) = read_json(&form_json.to_string()).filter(J::is_obj) else { return false };
        let ok = self.as_mut().rust_mut().session.as_mut().is_some_and(|s| s.commit(&f));
        if ok {
            self.touched();
        }
        ok
    }
    pub fn edit_undo(mut self: Pin<&mut Self>) -> bool {
        let ok = self.as_mut().rust_mut().session.as_mut().is_some_and(EditSession::undo);
        if ok {
            self.touched();
        }
        ok
    }
    pub fn edit_redo(mut self: Pin<&mut Self>) -> bool {
        let ok = self.as_mut().rust_mut().session.as_mut().is_some_and(EditSession::redo);
        if ok {
            self.touched();
        }
        ok
    }
    pub fn edit_preset(self: Pin<&mut Self>, name: &QString) -> QString {
        let n = name.to_string();
        self.with_session(move |s| match n.as_str() {
            "previous" => s.previous(),
            "reset" => s.reset(),
            _ => s.preset(&n),
        })
    }
    pub fn edit_dock(self: Pin<&mut Self>, island: &QString) -> QString {
        let r = island.to_string();
        self.with_session(move |s| {
            let f = es::dock_float(&s.form, &r)?;
            s.commit(&f);
            Ok(())
        })
    }
    pub fn edit_float(self: Pin<&mut Self>, island: &QString, rect_json: &QString, host_w: i32, host_h: i32) -> QString {
        let r = island.to_string();
        let rect = read_json(&rect_json.to_string()).map(|v| {
            let n = |k: &str| v.get(k).and_then(|x| x.as_f64()).map(|x| x.round() as i64).unwrap_or(0);
            Rect { x: n("x"), y: n("y"), w: n("w"), h: n("h") }
        });
        self.with_session(move |s| {
            let f = es::float_island(&s.form, &r, rect.unwrap_or_default(), host_w as i64, host_h as i64)?;
            s.commit(&f);
            Ok(())
        })
    }
    pub fn edit_reset_island(self: Pin<&mut Self>, island: &QString) -> QString {
        let r = island.to_string();
        self.with_session(move |s| {
            let f = es::reset_island(&s.form, &r)?;
            s.commit(&f);
            Ok(())
        })
    }
    pub fn edit_rename(mut self: Pin<&mut Self>, name: &QString) {
        if let Some(s) = self.as_mut().rust_mut().session.as_mut() {
            s.rename(&name.to_string());
        }
        self.touched();
    }
    pub fn edit_finish(mut self: Pin<&mut Self>) -> QString {
        let r = self.as_mut().rust_mut().session.as_mut().and_then(|s| s.finish());
        self.as_mut().touched();
        let out = match r {
            None => "null".to_string(),
            Some(r) => mk(vec![("id", J::s(&r.id)), ("name", J::s(&r.name)), ("desktop", r.desktop), ("phone", r.phone)]).canon(),
        };
        QString::from(out.as_str())
    }
    pub fn edit_reopen(mut self: Pin<&mut Self>) {
        if let Some(s) = self.as_mut().rust_mut().session.as_mut() {
            s.reopen();
        }
        self.touched();
    }
    pub fn edit_new_button_island(mut self: Pin<&mut Self>) -> QString {
        let out = match self.as_mut().rust_mut().session.as_mut().filter(|s| !s.closed) {
            None => step_json(Err("closed".into())),
            Some(s) => match es::new_button_island(&s.form) {
                Ok((f, r)) => {
                    s.commit(&f);
                    mk(vec![("ok", J::Bool(true)), ("ref", J::s(&r))]).canon()
                }
                Err(e) => step_json(Err(e)),
            },
        };
        self.touched();
        QString::from(out.as_str())
    }
    pub fn edit_remove_button_island(self: Pin<&mut Self>, island: &QString) -> QString {
        let r = island.to_string();
        self.with_session(move |s| match es::remove_button_island_from(&s.form, &r) {
            None => Err("bad-op: bad request".into()),
            Some(f) => {
                s.commit(&f);
                Ok(())
            }
        })
    }
    pub fn edit_cancel(mut self: Pin<&mut Self>) {
        if let Some(mut s) = self.as_mut().rust_mut().session.take() {
            s.cancel();
        }
        self.touched();
    }
    pub fn hidden_refs(&self, form_json: &QString) -> QString {
        let f = form_json.to_string();
        guarded("hiddenRefs", move || {
            let v = read_json(&f).map(|f| es::hidden_refs(&f)).unwrap_or_default();
            J::arr(v.iter().map(|s| J::s(s)).collect()).canon()
        })
    }
    pub fn pick_preset(&self, form_json: &QString, name: &QString) -> QString {
        let (f, n) = (form_json.to_string(), name.to_string());
        guarded("pickPreset", move || match read_json(&f).filter(J::is_obj) {
            None => step_json(Err("bad-layout: not JSON data".into())),
            Some(form) => match es::pick_preset(&form, &n) {
                Ok(form) => mk(vec![("ok", J::Bool(true)), ("form", form)]).canon(),
                Err(e) => step_json(Err(e)),
            },
        })
    }
}

/// An environment variable's text for the review-capture hooks: the value, or — `@<path>` — that file's text ("" when
/// unset or unreadable). (TINHAT_LAYOUT here, TINHAT_SHORTCUTS in actions_ctl.rs.)
pub fn env_text(name: &str) -> String {
    let v = std::env::var(name).unwrap_or_default();
    match v.strip_prefix('@') {
        Some(path) => std::fs::read_to_string(path.trim()).unwrap_or_default(),
        None => v,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tinhat_layout::json::{read_json, J};

    fn q(s: &str) -> QString {
        QString::from(s)
    }
    fn parse(s: &QString) -> J {
        read_json(&s.to_string()).expect("the controller answers JSON")
    }

    /// Every invokable through QStrings, the way QML calls them: a load → solve → op → solve round, drop zones, a
    /// save + merge of the result, migrate — and the answers are the engine's (tinhat_layout::api) byte for byte.
    #[test]
    fn layout_controller_round_trip() {
        // load: no document → the built-in Classic, active main
        let p = parse(&load(&q(""), &q("")));
        assert_eq!(p.get("active").and_then(|a| a.as_str().map(str::to_string)).as_deref(), Some("main"));
        assert_eq!(p.get("read").and_then(|a| a.as_str().map(str::to_string)).as_deref(), Some("ignore"));
        let form = p.get("desktop").unwrap();
        let form_json = form.canon();
        assert_eq!(parse(&validate(&q("desktop"), &q(&form_json))).canon(), r#"{"violations":[]}"#);
        // solve at the ISLANDS-ARCH §3.2 worked example (1560 × 960)
        let ctx = r#"{"size":{"w":1560,"h":960},"unitPm":1000,"content":{"rooms":{"h":300},"nav":{"h":228},"dms":{"h":0,"empty":true},"widgets":{"h":0,"empty":true},"account":{"h":46},"header":{"h":58},"composer":{"h":48}},"view":{"roomOpen":true}}"#;
        let o = parse(&solve(&q(&form_json), &q(ctx)));
        let t = o.get("islands").unwrap().get("timeline").unwrap();
        assert_eq!(t.canon(), r#"{"h":830,"vis":true,"w":1176,"x":378,"y":70,"z":0}"#);
        assert_eq!(solve(&q(&form_json), &q(ctx)).to_string(), api::solve(&form_json, ctx));
        // an operation: hide rooms (ok), hide the timeline (refused, the input untouched)
        let h = parse(&apply_op(&q(&form_json), &q(r#"{"op":"hide","ref":"rooms"}"#), &q("")));
        assert!(h.get("ok").unwrap().is_true());
        assert_eq!(h.get("form").unwrap().get("hidden").unwrap().len(), 1);
        let bad = parse(&apply_op(&q(&form_json), &q(r#"{"op":"hide","ref":"timeline"}"#), &q("")));
        assert_eq!(bad.canon(), r#"{"error":"bad-op: required island","ok":false}"#);
        // drop zones of dragging rooms
        let z = parse(&drop_zones(&q(&form_json), &q(ctx), &q("rooms")));
        assert!(!z.get("hit").unwrap().is_empty() && z.get("hit").unwrap().len() == z.get("keys").unwrap().len());
        // repair of text that is not JSON
        let r = parse(&repair(&q("desktop"), &q("{nope")));
        assert_eq!(r.get("notes").unwrap().canon(), r#"["replaced-by-classic not-json"]"#);
        // save the hidden-rooms form as layout main, then merge the document with nothing known
        let phone = p.get("phone").unwrap().canon();
        let req = format!(r#"{{"id":"main","edit":{{"name":"Main","desktop":{},"phone":{phone}}},"now":1000,"by":"dev-a","seq":1}}"#, h.get("form").unwrap().canon());
        let s = parse(&save(&q(""), &q("{}"), &q(&req)));
        assert!(s.get("ok").unwrap().is_true(), "{}", s.canon());
        let doc = s.get("doc").unwrap().canon();
        let m = parse(&merge(&q(&doc), &q("{}"), &q("")));
        assert_eq!(m.get("doc").unwrap().canon(), doc);
        assert!(!m.get("tooLarge").unwrap().is_true());
        // the saved document loads back with the same desktop form
        let back = parse(&load(&q(&doc), &q("main")));
        assert_eq!(back.get("desktop").unwrap().canon(), h.get("form").unwrap().canon());
        // migrate
        assert_eq!(migrate(&q(r#"{"prefs":{"nav_mode":"corner"}}"#)).to_string(), r#"{"view":{"forceCollapse":["navcol"]}}"#);
        // an argument that is not JSON data
        assert_eq!(parse(&solve(&q("garbage"), &q(""))).get("ok").map(|o| o.is_true()), Some(false));
    }
}
