//! theme_editor.rs — ThemeEditorController (11.0 step 5; contract THEMES-UI.md §13–§15): the QML face of the theme
//! editor, the theme share cards and the Pick tool — tinhat_theme::{editor, share, look, labels, probe} with the app's
//! token list, the theme store (theme_store.rs) for Save / Keep, and the room for Share to a room. The web's twin is
//! `src/features/settings/themes/{editor,share}/` (the same rules; the pure parts are shared vectors).
//!
//! The editor session lives here (undo / redo / gestures: tinhat_theme::editor); QML reads `stateJson` (below) and
//! paints the working document through a PREVIEW: `previewDoc(owner, docJson)` resolves it and returns the Th colours
//! for `Th.apply(…)`, with the meta in `previewVariant` / `previewNames` / `previewShape` / `previewLook`; nothing is
//! saved; AppCore leaves the effective theme alone while `previewOwner` is set and applies it again when it ends.
//!
//! `stateJson` = `{open, doc, base, target: "mine"|"new", baseName, canUndo, canRedo, dirty, hasPalette, variant,
//! adjusted, roles, tokens (canonical colours), look, shape, opacity, accentNow, accentChips: [{value, colour, label}],
//! issues: [{fg, bg, ratio, need, line}], fixes: [{op, line}]}` (issues = what the edits broke, §13.5; fixes: one per
//! colour, roles first).

use core::pin::Pin;
use std::collections::BTreeMap;
use std::sync::OnceLock;

use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::{QColor, QMap, QMapPair_QString_QVariant, QString, QVariant};
use serde_json::{json, Value};
use tinhat_theme::editor::{self as ed, ContrastIssue, EditSession, SaveTarget};
use tinhat_theme::labels;
use tinhat_theme::share;
use tinhat_theme::sync::JObj;
use tinhat_theme::{app_tokens, canon_json, parse_json, parse_theme_json, resolve_for_app, App, AppResolved};

use crate::theme_ctl::{colour_map, th_entries};
use crate::theme_rt::Active;

#[cxx_qt::bridge]
pub mod qobject {
    extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
        include!("cxx-qt-lib/qmap.h");
        type QMap_QString_QVariant = cxx_qt_lib::QMap<cxx_qt_lib::QMapPair_QString_QVariant>;
    }

    #[auto_cxx_name] // snake_case Rust -> camelCase QML
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(bool, open)] //               QML: open — an editor session exists (the panel shows)
        #[qproperty(QString, state_json)] //      QML: stateJson — the session for the panel (module doc)
        #[qproperty(i32, state_rev)] //           QML: stateRev — bumps with stateJson
        #[qproperty(QString, preview_owner)] //   QML: previewOwner — "" | "editor" | "try"
        #[qproperty(QString, preview_id)] //      QML: previewId / previewName / previewVariant / previewNames /
        #[qproperty(QString, preview_name)] //    previewShape / previewLook — Th.apply's meta for the preview
        #[qproperty(QString, preview_variant)]
        #[qproperty(QString, preview_names)]
        #[qproperty(QString, preview_shape)]
        #[qproperty(QString, preview_look)]
        type ThemeEditorController = super::ThemeEditorRust;

        /// QML: editorCtl.openEditor(docJson, mineId, baseName, force) — THEMES-UI §13.1: `mineId` "" = a new theme
        /// from `doc` ("‹name› (edited)"), else edit that My theme. "" or the refusal in plain words (another session
        /// with changes: "Finish editing ‹name› first — save it or cancel.").
        #[qinvokable]
        fn open_editor(self: Pin<&mut ThemeEditorController>, doc_json: &QString, mine_id: &QString, base_name: &QString, force: bool) -> QString;
        /// QML: editorCtl.edit(opJson, gesture) — one edit (§13.2); gesture 0 = none. "" or the refusal code.
        #[qinvokable]
        fn edit(self: Pin<&mut ThemeEditorController>, op_json: &QString, gesture: i32) -> QString;
        /// QML: editorCtl.newGesture() — a new gesture id (one drag / one focus = one undo step, §13.3).
        #[qinvokable]
        fn new_gesture(self: Pin<&mut ThemeEditorController>) -> i32;
        #[qinvokable]
        fn undo(self: Pin<&mut ThemeEditorController>);
        #[qinvokable]
        fn redo(self: Pin<&mut ThemeEditorController>);
        #[qinvokable]
        fn reset_all(self: Pin<&mut ThemeEditorController>);
        /// QML: editorCtl.save(asName) — §13.4: My themes + applied; "" = Save, a name = Save as a new theme.
        /// `{ok, name}` or `{ok: false, text}`. The session closes on success.
        #[qinvokable]
        fn save(self: Pin<&mut ThemeEditorController>, as_name: &QString) -> QString;
        /// QML: editorCtl.close() — end the session (Cancel after the confirmation; QML ends the preview).
        #[qinvokable]
        fn close(self: Pin<&mut ThemeEditorController>);
        /// QML: editorCtl.workingDoc() — the working document as JSON ("" without a session).
        #[qinvokable]
        fn working_doc(self: &ThemeEditorController) -> QString;
        /// QML: editorCtl.exportName() / exportTo(fileUrl) — §13.4 Export: `<slug>.tinhat-theme.json`; "" or the error.
        #[qinvokable]
        fn export_name(self: &ThemeEditorController) -> QString;
        #[qinvokable]
        fn export_to(self: &ThemeEditorController, url: &QString) -> QString;
        /// QML: editorCtl.exportUrl() — the Save dialog's starting file: `<Downloads>/<slug>.tinhat-theme.json`.
        #[qinvokable]
        fn export_url(self: &ThemeEditorController) -> QString;
        /// QML: editorCtl.docJson(id) — a theme's document by id (built-in, library, one of My themes, the rendered
        /// one) as JSON text, "" when there is none (Customize… / the ⋯ menus open the editor with it).
        #[qinvokable]
        fn doc_json(self: &ThemeEditorController, id: &QString) -> QString;
        /// QML: editorCtl.groupsJson(hasPalette) — the Advanced mode's groups + token descriptions (§13.8):
        /// `{groups: [{id, section, label, rows: [{kind, id, label}]}], descriptions: {id: text}}`.
        #[qinvokable]
        fn groups_json(self: &ThemeEditorController, has_palette: bool) -> QString;
        /// QML: editorCtl.searchJson(hasPalette, query) — the groups a search keeps (§13.8 `searchGroups`, the shared
        /// rule): `[{id, section, label, rows}]`.
        #[qinvokable]
        fn search_json(self: &ThemeEditorController, has_palette: bool, query: &QString) -> QString;
        /// QML: editorCtl.label(kind, id) — a palette slot's / role's / token's label (editor-labels.json, §13.8).
        #[qinvokable]
        fn label(self: &ThemeEditorController, kind: &QString, id: &QString) -> QString;
        /// QML: editorCtl.tokenDefault(id) — a token's default expression (the popover's "default: …" line).
        #[qinvokable]
        fn token_default(self: &ThemeEditorController, id: &QString) -> QString;
        /// QML: editorCtl.canonColour(text) — a colour literal the engine reads (`#rgb`, `#rrggbb`, `#rrggbbaa`,
        /// `#rrggbb/A`) in canonical form, "" for anything else (the popover's Hex field).
        #[qinvokable]
        fn canon_colour(self: &ThemeEditorController, text: &QString) -> QString;
        /// QML: editorCtl.copyName(name) — "‹name› (edited)" cut to 80 characters (Save as's prefill, §13.4).
        #[qinvokable]
        fn copy_name(self: &ThemeEditorController, name: &QString) -> QString;
        /// QML: editorCtl.fixOp(fg, bg) — §13.9: the Fix of one shown readability issue (`fix_contrast`: a guarded role
        /// moved over every guard surface, anything else guarded against that background) as an op's JSON, "" = none.
        #[qinvokable]
        fn fix_op(self: &ThemeEditorController, fg: &QString, bg: &QString) -> QString;
        /// QML: editorCtl.fontsJson() — ENGINE.md §10's allowed families: `{ui: [sans…, serif…], code: [mono…]}` (the
        /// Simple mode's font lists; chat = ui).
        #[qinvokable]
        fn fonts_json(self: &ThemeEditorController) -> QString;

        /// QML: editorCtl.previewDoc(owner, docJson) — paint a document without saving (§13.4 / §14.4): the Th
        /// colours for Th.apply (empty on a refusal) + the preview* meta; `owner` "editor" | "try" (another owner's
        /// preview → empty).
        #[qinvokable]
        fn preview_doc(self: Pin<&mut ThemeEditorController>, owner: &QString, doc_json: &QString) -> QMap_QString_QVariant;
        /// QML: editorCtl.endPreview(owner) — true when that preview ended (QML applies the effective theme again).
        #[qinvokable]
        fn end_preview(self: Pin<&mut ThemeEditorController>, owner: &QString) -> bool;

        /// QML: editorCtl.shareToRoom(roomId, docJson) — §14.5: the card as an m.room.message; `shared(ok, text)`
        /// follows. "" or the refusal ("This theme can't be shared (‹code›).").
        #[qinvokable]
        fn share_to_room(self: Pin<&mut ThemeEditorController>, room_id: &QString, doc_json: &QString) -> QString;
        #[qsignal]
        fn shared(self: Pin<&mut ThemeEditorController>, ok: bool, text: QString);
        /// QML: editorCtl.keep(themeJson) — §14.4 Keep: `{ok, toast}` / `{ok: false, text}`.
        #[qinvokable]
        fn keep(self: Pin<&mut ThemeEditorController>, theme_json: &QString) -> QString;
        /// QML: editorCtl.keepState(themeJson, activeId) — "keep" | "use" | "inUse" (the Keep button's label).
        #[qinvokable]
        fn keep_state(self: &ThemeEditorController, theme_json: &QString, active_id: &QString) -> QString;
        /// QML: editorCtl.shareRowJson(contentJson) — a message content read as a card (§14.2) the way the timeline
        /// rows carry it (`share_row`): "null" for a message that is not a card.
        #[qinvokable]
        fn share_row_json(self: &ThemeEditorController, content_json: &QString) -> QString;
        /// QML: editorCtl.shareContentJson(docJson) — the content Share to a room sends (§14.1), "" when refused (the
        /// review harness builds its cards from it).
        #[qinvokable]
        fn share_content_json(self: &ThemeEditorController, doc_json: &QString) -> QString;

        /// QML: editorCtl.grabWindow(path, title) — the review harness (ThemeEditorGallery.qml): the visible Quick window
        /// with that title, popups and dialogs included, saved as a PNG (theme_bridge.cpp). true = saved.
        #[qinvokable]
        fn grab_window(self: &ThemeEditorController, path: &QString, title: &QString) -> bool;
        /// QML: editorCtl.harnessScheme(id) — the review harness under Breeze (org.kde.desktop): activate the theme's KDE
        /// colour scheme as the app does for the effective theme (colorscheme.rs), so the native controls are painted
        /// like on Plasma. The app itself never calls it (a preview keeps the effective theme's scheme).
        #[qinvokable]
        fn harness_scheme(self: &ThemeEditorController, id: &QString);
        /// QML: editorCtl.probeMap() — §15.1: `{ <Th property>: probe colour }` for every role and token.
        #[qinvokable]
        fn probe_map(self: &ThemeEditorController) -> QMap_QString_QVariant;
        /// QML: editorCtl.pickList(layersJson) — §15.2: `[{element, props: [{kind, real, probe}]}]` (colours as
        /// "#rrggbb" or "#aarrggbb") → the Pick list `[{id, kind}]` (a property counts when its probe value differs
        /// from its real value and decodes).
        #[qinvokable]
        fn pick_list(self: &ThemeEditorController, layers_json: &QString) -> QString;
    }

    impl cxx_qt::Threading for ThemeEditorController {}
}

#[derive(Default)]
pub struct ThemeEditorRust {
    open: bool,
    state_json: QString,
    state_rev: i32,
    preview_owner: QString,
    preview_id: QString,
    preview_name: QString,
    preview_variant: QString,
    preview_names: QString,
    preview_shape: QString,
    preview_look: QString,
    // the session (not properties)
    session: Option<EditSession>,
    mine_id: Option<String>,
    base_name: String,
    base_issues: Vec<ContrastIssue>,
    gesture_seq: i32,
}

fn resolve_doc(doc: &Value) -> Result<AppResolved, String> {
    resolve_for_app(doc, app_tokens(), App::Desktop).map_err(|e| e.to_string())
}

/// "Text — 1.5:1 on Islands, needs 4.5:1" (§13.5): a role by its label, a token as "‹area label›: ‹token label›".
pub fn colour_name(id: &str) -> String {
    let l = labels::labels();
    if tinhat_theme::roles::ROLE_IDS.contains(&id) {
        return labels::role_label(id, l);
    }
    let area = labels::app_token_info().iter().find(|t| t.id == id).map(|t| t.area.as_str()).unwrap_or_else(|| id.split('.').next().unwrap_or(id));
    format!("{}: {}", labels::area_label(area, l), labels::token_label(id, l))
}

pub fn issue_line(i: &ContrastIssue) -> String {
    let ratio = (i.ratio * 10.0).floor() / 10.0;
    let need = if i.need == 3.0 { "3".to_string() } else { format!("{}", i.need) };
    format!("{} — {:.1}:1 on {}, needs {}:1", colour_name(&i.fg), ratio, colour_name(&i.bg), need)
}

/// The accent chips (§13.7): palette themes — cursor (when it is not the text colour), c1–c6, c9–c14, repeats left
/// out; roles-only themes — ACCENT_PRESETS.
fn accent_chips(doc: &JObj) -> Vec<Value> {
    let Some(p) = doc.get("palette").and_then(Value::as_object) else {
        return ed::ACCENT_PRESETS.iter().map(|c| json!({ "value": c, "colour": c, "label": c })).collect();
    };
    let l = labels::labels();
    let ansi: Vec<&str> = p.get("ansi").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    let mut seen: Vec<String> = Vec::new();
    let mut out = Vec::new();
    let mut add = |value: String, colour: Option<&str>, label: String| {
        let Some(c) = colour else { return };
        let k = c.to_ascii_lowercase();
        if seen.contains(&k) {
            return;
        }
        seen.push(k);
        out.push(json!({ "value": value, "colour": c, "label": label }));
    };
    let fg = p.get("fg").and_then(Value::as_str).unwrap_or("").to_ascii_lowercase();
    if let Some(cur) = p.get("cursor").and_then(Value::as_str) {
        if cur.to_ascii_lowercase() != fg {
            add("cursor".into(), Some(cur), "Cursor".into());
        }
    }
    for n in [1usize, 2, 3, 4, 5, 6, 9, 10, 11, 12, 13, 14] {
        let slot = format!("c{n}");
        let label = labels::palette_label(&slot, l);
        add(slot, ansi.get(n).copied(), label);
    }
    out
}

impl ThemeEditorRust {
    /// The panel's state (module doc).
    fn state(&self) -> Value {
        let Some(s) = &self.session else { return json!({ "open": false }) };
        let doc = Value::Object(s.doc.clone());
        let Ok(r) = resolve_doc(&doc) else { return json!({ "open": false }) };
        let issues = ed::new_issues(&ed::contrast_issues(&r), &self.base_issues);
        let mut fixes: Vec<(bool, Value)> = ed::fixes_for(&issues, &r)
            .into_iter()
            .map(|op| {
                let fg = op.get("id").and_then(Value::as_str).unwrap_or("").to_string();
                let line = issues.iter().find(|i| i.fg == fg).map(issue_line).unwrap_or_default();
                (op.get("op").and_then(Value::as_str) == Some("role"), json!({ "op": op, "line": line }))
            })
            .collect();
        fixes.sort_by_key(|(role, _)| !*role); // roles first (stable: id order within)
        let has_palette = s.doc.get("palette").is_some_and(Value::is_object);
        let accent_now = if has_palette {
            s.doc.get("accent").and_then(Value::as_str).map(str::to_string)
        } else {
            s.doc.get("roles").and_then(|r| r.get("accent.base")).and_then(Value::as_str).map(str::to_string)
        };
        let look = tinhat_theme::look::read_look(&doc);
        json!({
            "open": true,
            "doc": doc,
            "base": Value::Object(s.base.clone()),
            "target": if self.mine_id.is_some() { "mine" } else { "new" },
            "baseName": self.base_name,
            "canUndo": !s.past.is_empty(),
            "canRedo": !s.future.is_empty(),
            "dirty": ed::is_dirty(s),
            "hasPalette": has_palette,
            "variant": r.variant.as_str(),
            "adjusted": r.adjusted.len(),
            "roles": r.roles,
            "tokens": r.tokens,
            "look": parse_json(&tinhat_theme::look::look_json(&look)).unwrap_or(Value::Null),
            "shape": parse_json(&tinhat_theme::shape::read_shape(&doc).to_json()).unwrap_or(Value::Null),
            "opacity": ed::island_opacity(&doc, app_tokens()),
            "accentNow": accent_now,
            "accentChips": accent_chips(&s.doc),
            "issues": issues.iter().map(|i| { let mut v = i.to_json(); v["line"] = json!(issue_line(i)); v }).collect::<Vec<_>>(),
            "fixes": fixes.into_iter().map(|(_, v)| v).collect::<Vec<_>>(),
        })
    }
}

fn my_theme_docs() -> Vec<JObj> {
    crate::theme_store::my_ids().iter().filter_map(|id| crate::theme_store::my_doc(id)).collect()
}

fn plain(v: Value) -> QString {
    QString::from(canon_json(&v).as_str())
}

/// The roles a share card paints (THEMES-UI §14.3: the card in the SHARED theme's own roles), under the keys
/// ThemeShareCard.qml reads.
const CARD_ROLES: [(&str, &str); 10] = [
    ("bg", "surface.island"),
    ("raised", "surface.raised"),
    ("hover", "surface.hover"),
    ("border", "border.subtle"),
    ("focus", "border.focus"),
    ("fg", "text.primary"),
    ("fg2", "text.secondary"),
    ("on", "text.onAccent"),
    ("accent", "accent.base"),
    ("bright", "accent.bright"),
];

fn qt_of(canonical: &str) -> String {
    tinhat_theme::parse_literal(canonical).map(|c| crate::theme_rt::qt_string(&c)).unwrap_or_else(|| "transparent".to_string())
}

/// THEMES-UI §14.2: a message content read as a share card, the way a timeline row carries it — `null` when it is not
/// a card, else `read_share(…).to_json()` + (ok) `themeJson` (the theme's canonical JSON text, for Try / Keep),
/// `colours` (the card's roles as Qt colour strings: the theme resolved with the roles only, like the web's card) and
/// `swatchColours` (the swatches as Qt colour strings). The receiver computes all of it.
pub fn share_row(content: &Value) -> Value {
    if content.get(share::SHARE_KEY).is_none() {
        return Value::Null;
    }
    let r = share::read_share(content, app_tokens());
    if r.kind() == "none" {
        return Value::Null;
    }
    let mut v = r.to_json();
    if let share::ShareRead::Ok { theme, swatches, .. } = &r {
        let t = Value::Object(theme.clone());
        v["themeJson"] = Value::String(canon_json(&t));
        let mut colours = serde_json::Map::new();
        if let Ok(res) = tinhat_theme::resolve_theme(&t, &tinhat_theme::TokensSpec::empty()) {
            for (key, role) in CARD_ROLES {
                colours.insert(key.to_string(), Value::String(res.roles.get(role).map(|c| qt_of(c)).unwrap_or_else(|| "transparent".into())));
            }
        }
        v["colours"] = Value::Object(colours);
        v["swatchColours"] = Value::Array(swatches.iter().map(|c| Value::String(qt_of(c))).collect());
    }
    v
}

/// The file path of a `file://` URL (percent-decoded; Windows `file:///C:/…` → `C:/…`).
fn path_of_url(u: &str) -> String {
    let p = match u.strip_prefix("file://") {
        Some(rest) => percent_decode(rest),
        None => u.to_string(),
    };
    if cfg!(windows) {
        p.trim_start_matches('/').to_string()
    } else {
        p
    }
}

impl qobject::ThemeEditorController {
    fn publish(mut self: Pin<&mut Self>) {
        let st = self.rust().state();
        let open = st.get("open").and_then(Value::as_bool).unwrap_or(false);
        self.as_mut().set_state_json(QString::from(canon_json(&st).as_str()));
        let rev = *self.state_rev() + 1;
        self.as_mut().set_state_rev(rev);
        self.as_mut().set_open(open);
    }

    pub fn open_editor(mut self: Pin<&mut Self>, doc_json: &QString, mine_id: &QString, base_name: &QString, force: bool) -> QString {
        if let Some(s) = &self.rust().session {
            if ed::is_dirty(s) && !force {
                let name = s.doc.get("name").and_then(Value::as_str).unwrap_or("").to_string();
                return QString::from(format!("Finish editing {name} first — save it or cancel.").as_str());
            }
        }
        let Ok(Value::Object(doc)) = parse_theme_json(&doc_json.to_string()) else {
            return QString::from(format!("{} can't be edited by this version of Tin Hat.", base_name).as_str());
        };
        let mine = mine_id.to_string();
        let start = if mine.is_empty() { ed::start_copy(&doc) } else { doc };
        let Ok(r) = resolve_doc(&Value::Object(start.clone())) else {
            return QString::from(format!("{} can't be edited by this version of Tin Hat.", base_name).as_str());
        };
        {
            let mut rs = self.as_mut().rust_mut();
            rs.base_issues = ed::contrast_issues(&r);
            rs.session = Some(ed::open_session(&start));
            rs.mine_id = (!mine.is_empty()).then_some(mine);
            rs.base_name = base_name.to_string();
        }
        self.publish();
        QString::default()
    }

    pub fn edit(mut self: Pin<&mut Self>, op_json: &QString, gesture: i32) -> QString {
        let op = parse_json(&op_json.to_string()).unwrap_or(Value::Null);
        let r = {
            let mut rs = self.as_mut().rust_mut();
            let Some(s) = rs.session.as_mut() else { return QString::from("closed") };
            ed::session_edit(s, &op, app_tokens(), (gesture > 0).then_some(gesture as i64))
        };
        match r {
            Ok(ed::StepResult::Noop) => QString::default(),
            Ok(_) => {
                self.publish();
                QString::default()
            }
            Err(code) => QString::from(code.as_str()),
        }
    }

    pub fn new_gesture(mut self: Pin<&mut Self>) -> i32 {
        let mut rs = self.as_mut().rust_mut();
        rs.gesture_seq += 1;
        rs.gesture_seq
    }

    pub fn undo(mut self: Pin<&mut Self>) {
        let changed = self.as_mut().rust_mut().session.as_mut().map(|s| ed::session_undo(s) != ed::StepResult::Noop).unwrap_or(false);
        if changed {
            self.publish();
        }
    }

    pub fn redo(mut self: Pin<&mut Self>) {
        let changed = self.as_mut().rust_mut().session.as_mut().map(|s| ed::session_redo(s) != ed::StepResult::Noop).unwrap_or(false);
        if changed {
            self.publish();
        }
    }

    pub fn reset_all(mut self: Pin<&mut Self>) {
        let changed = self.as_mut().rust_mut().session.as_mut().map(|s| ed::session_reset(s) != ed::StepResult::Noop).unwrap_or(false);
        if changed {
            self.publish();
        }
    }

    pub fn save(mut self: Pin<&mut Self>, as_name: &QString) -> QString {
        let (mut doc, mine): (JObj, Option<String>) = match &self.rust().session {
            Some(s) => (s.doc.clone(), self.rust().mine_id.clone()),
            None => return plain(json!({ "ok": false, "text": "Nothing to save." })),
        };
        let as_name = as_name.to_string();
        let as_new = !as_name.trim().is_empty();
        if as_new {
            match ed::apply_edit(&Value::Object(doc.clone()), &json!({ "op": "name", "value": as_name }), app_tokens()) {
                Ok(e) => doc = e.doc,
                Err(_) => return plain(json!({ "ok": false, "text": "A theme's name is 1 to 80 characters." })),
            }
        }
        let name = doc.get("name").and_then(Value::as_str).unwrap_or("").to_string();
        let result: Result<(), String> = match mine {
            Some(id) if !as_new => {
                let d = ed::save_doc(&doc, SaveTarget::Mine(&id), |_| false);
                match crate::theme_store::restore_mine(&d) {
                    crate::theme_store::Outcome::Failed(w) => Err(w),
                    _ => match crate::theme_store::apply_theme(&id, false) {
                        crate::theme_store::Outcome::Failed(w) => Err(w),
                        _ => Ok(()),
                    },
                }
            }
            // a new theme: its id = slugify(name) made unique against the library and My themes (save_new); the
            // editor is not the Settings sheet, so a sound offer comes as a toast (like the web's chooseTheme)
            _ => crate::theme_store::save_new_opts(&doc, &name, true, false).map(|_| ()),
        };
        match result {
            Ok(()) => {
                self.as_mut().close();
                plain(json!({ "ok": true, "name": name }))
            }
            Err(text) => plain(json!({ "ok": false, "text": text })),
        }
    }

    pub fn close(mut self: Pin<&mut Self>) {
        {
            let mut rs = self.as_mut().rust_mut();
            rs.session = None;
            rs.mine_id = None;
            rs.base_issues.clear();
        }
        self.publish();
    }

    pub fn working_doc(&self) -> QString {
        match &self.rust().session {
            Some(s) => QString::from(canon_json(&Value::Object(s.doc.clone())).as_str()),
            None => QString::default(),
        }
    }

    pub fn export_name(&self) -> QString {
        match &self.rust().session {
            Some(s) => QString::from(ed::export_file_name(&s.doc).as_str()),
            None => QString::default(),
        }
    }

    pub fn export_to(&self, url: &QString) -> QString {
        let Some(s) = &self.rust().session else { return QString::from("Nothing to export.") };
        let path = path_of_url(&url.to_string());
        match std::fs::write(&path, ed::export_text(&s.doc)) {
            Ok(()) => QString::default(),
            Err(e) => QString::from(format!("Couldn't write {path}: {e}").as_str()),
        }
    }

    pub fn export_url(&self) -> QString {
        let name = match &self.rust().session {
            Some(s) => ed::export_file_name(&s.doc),
            None => "theme.tinhat-theme.json".to_string(),
        };
        let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(std::path::PathBuf::from).unwrap_or_default();
        let downloads = home.join("Downloads");
        let dir = if downloads.is_dir() { downloads } else { home };
        QString::from(crate::platform::file_url(&dir.join(name)).as_str())
    }

    pub fn doc_json(&self, id: &QString) -> QString {
        match crate::theme_rt::load_doc(&id.to_string(), &crate::theme_rt::themes_dir()) {
            Ok(d) => QString::from(canon_json(&d).as_str()),
            Err(_) => QString::default(),
        }
    }

    pub fn groups_json(&self, has_palette: bool) -> QString {
        static CACHE: OnceLock<[String; 2]> = OnceLock::new();
        let c = CACHE.get_or_init(|| [labels::advanced_groups_json(false), labels::advanced_groups_json(true)]);
        QString::from(c[has_palette as usize].as_str())
    }

    pub fn search_json(&self, has_palette: bool, query: &QString) -> QString {
        static GROUPS: OnceLock<[Vec<labels::EditorGroup>; 2]> = OnceLock::new();
        let g = GROUPS.get_or_init(|| {
            let l = labels::labels();
            [labels::advanced_groups(false, labels::app_token_info(), l), labels::advanced_groups(true, labels::app_token_info(), l)]
        });
        let descriptions = descriptions();
        let kept = labels::search_groups(&g[has_palette as usize], &query.to_string(), |id| descriptions.get(id).cloned().unwrap_or_default());
        QString::from(canon_json(&Value::Array(kept.iter().map(labels::EditorGroup::to_json).collect())).as_str())
    }

    pub fn label(&self, kind: &QString, id: &QString) -> QString {
        let l = labels::labels();
        let id = id.to_string();
        let s = match kind.to_string().as_str() {
            "palette" => labels::palette_label(&id, l),
            "role" => labels::role_label(&id, l),
            "area" => labels::area_label(&id, l),
            "colour" => colour_name(&id),
            _ => labels::token_label(&id, l),
        };
        QString::from(s.as_str())
    }

    pub fn token_default(&self, id: &QString) -> QString {
        QString::from(token_defaults().get(&id.to_string()).map(String::as_str).unwrap_or(""))
    }

    pub fn canon_colour(&self, text: &QString) -> QString {
        match tinhat_theme::parse_literal(&text.to_string()) {
            Some(c) => QString::from(tinhat_theme::format_colour(c).as_str()),
            None => QString::default(),
        }
    }

    pub fn copy_name(&self, name: &QString) -> QString {
        QString::from(ed::copy_name(&name.to_string()).as_str())
    }

    pub fn fix_op(&self, fg: &QString, bg: &QString) -> QString {
        let Some(s) = &self.rust().session else { return QString::default() };
        let Ok(r) = resolve_doc(&Value::Object(s.doc.clone())) else { return QString::default() };
        let (fg, bg) = (fg.to_string(), bg.to_string());
        let issues = ed::new_issues(&ed::contrast_issues(&r), &self.rust().base_issues);
        match issues.iter().find(|i| i.fg == fg && i.bg == bg).and_then(|i| ed::fix_contrast(i, &r)) {
            Some(op) => QString::from(canon_json(&op).as_str()),
            None => QString::default(),
        }
    }

    pub fn fonts_json(&self) -> QString {
        let v = json!({ "ui": tinhat_theme::look::fonts_for("ui"), "code": tinhat_theme::look::fonts_for("code") });
        QString::from(canon_json(&v).as_str())
    }

    pub fn preview_doc(mut self: Pin<&mut Self>, owner: &QString, doc_json: &QString) -> QMap<QMapPair_QString_QVariant> {
        let owner = owner.to_string();
        let cur = self.preview_owner().to_string();
        if !cur.is_empty() && cur != owner {
            return QMap::default();
        }
        let Ok(doc) = parse_theme_json(&doc_json.to_string()) else { return QMap::default() };
        let Ok(resolved) = resolve_doc(&doc) else { return QMap::default() };
        let id = doc.get("id").and_then(Value::as_str).unwrap_or("preview").to_string();
        let name = doc.get("name").and_then(Value::as_str).unwrap_or(&id).to_string();
        let a = Active { id: id.clone(), name: name.clone(), resolved, doc: doc.clone() };
        self.as_mut().set_preview_id(QString::from(id.as_str()));
        self.as_mut().set_preview_name(QString::from(name.as_str()));
        self.as_mut().set_preview_variant(QString::from(a.resolved.variant.as_str()));
        self.as_mut().set_preview_names(QString::from(a.resolved.names.as_str()));
        self.as_mut().set_preview_shape(QString::from(tinhat_theme::shape::read_shape(&doc).to_json().as_str()));
        self.as_mut().set_preview_look(QString::from(tinhat_theme::look::look_json(&tinhat_theme::look::read_look(&doc)).as_str()));
        self.as_mut().set_preview_owner(QString::from(owner.as_str()));
        colour_map(&a)
    }

    pub fn end_preview(mut self: Pin<&mut Self>, owner: &QString) -> bool {
        if self.preview_owner().to_string() != owner.to_string() || owner.is_empty() {
            return false;
        }
        self.as_mut().set_preview_owner(QString::default());
        true
    }

    pub fn share_to_room(self: Pin<&mut Self>, room_id: &QString, doc_json: &QString) -> QString {
        let doc = parse_json(&doc_json.to_string()).unwrap_or(Value::Null);
        let content = match share::share_content(&doc, app_tokens()) {
            Ok(c) => c,
            Err(e) => {
                let code = e.split(':').nth(if e.starts_with("refused") { 1 } else { 0 }).unwrap_or(&e).trim().to_string();
                return QString::from(format!("This theme can't be shared ({code}).").as_str());
            }
        };
        let name = doc.get("name").and_then(Value::as_str).unwrap_or("The theme").to_string();
        let rid = room_id.to_string();
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let r: Result<String, String> = async {
                let client = crate::session::client().ok_or_else(|| "Not signed in".to_string())?;
                let id = matrix_sdk::ruma::RoomId::parse(&rid).map_err(|e| format!("bad room id: {e}"))?;
                let room = client.get_room(&id).ok_or_else(|| "Room not found".to_string())?;
                room.send_raw("m.room.message", Value::Object(content)).await.map_err(|e| e.to_string())?;
                Ok(room.name().unwrap_or_else(|| "the room".to_string()))
            }
            .await;
            let _ = qt.queue(move |o| {
                let (ok, text) = match r {
                    Ok(room) => (true, format!("{name} shared in {room}")),
                    Err(e) => (false, format!("Couldn't share {name}: {e}")),
                };
                o.shared(ok, QString::from(text.as_str()));
            });
        });
        QString::default()
    }

    pub fn keep(self: Pin<&mut Self>, theme_json: &QString) -> QString {
        let Ok(Value::Object(theme)) = parse_json(&theme_json.to_string()) else {
            return plain(json!({ "ok": false, "text": "This theme can't be used." }));
        };
        let mine = my_theme_docs();
        let plan = share::keep_plan(&theme, crate::theme_lib::doc, &mine);
        let out = match plan {
            share::KeepPlan::Library { id } | share::KeepPlan::Mine { id } => match crate::theme_store::apply_theme(&id, false) {
                crate::theme_store::Outcome::Failed(w) => json!({ "ok": false, "text": w }),
                _ => json!({ "ok": true, "toast": "" }),
            },
            share::KeepPlan::Save { doc } => {
                let name = doc.get("name").and_then(Value::as_str).unwrap_or("").to_string();
                match crate::theme_store::save_new_opts(&doc, &name, true, false) {
                    Ok(_) => json!({ "ok": true, "toast": format!("{name} added to My themes") }),
                    Err(w) => json!({ "ok": false, "text": w }),
                }
            }
        };
        plain(out)
    }

    pub fn keep_state(&self, theme_json: &QString, active_id: &QString) -> QString {
        let Ok(Value::Object(theme)) = parse_json(&theme_json.to_string()) else { return QString::from("keep") };
        let mine = my_theme_docs();
        let s = match share::keep_plan(&theme, crate::theme_lib::doc, &mine) {
            share::KeepPlan::Save { .. } => "keep",
            share::KeepPlan::Library { id } | share::KeepPlan::Mine { id } => {
                if id == active_id.to_string() {
                    "inUse"
                } else {
                    "use"
                }
            }
        };
        QString::from(s)
    }

    pub fn share_row_json(&self, content_json: &QString) -> QString {
        let v = parse_json(&content_json.to_string()).unwrap_or(Value::Null);
        QString::from(canon_json(&share_row(&v)).as_str())
    }

    pub fn share_content_json(&self, doc_json: &QString) -> QString {
        let doc = parse_json(&doc_json.to_string()).unwrap_or(Value::Null);
        match share::share_content(&doc, app_tokens()) {
            Ok(c) => QString::from(canon_json(&Value::Object(c)).as_str()),
            Err(_) => QString::default(),
        }
    }

    pub fn harness_scheme(&self, id: &QString) {
        crate::colorscheme::apply(&id.to_string());
    }

    pub fn grab_window(&self, path: &QString, title: &QString) -> bool {
        extern "C" {
            fn tinhat_grab_window(path: *const std::os::raw::c_char, title: *const std::os::raw::c_char) -> std::os::raw::c_int;
        }
        let (Ok(p), Ok(t)) = (std::ffi::CString::new(path.to_string()), std::ffi::CString::new(title.to_string())) else { return false };
        unsafe { tinhat_grab_window(p.as_ptr(), t.as_ptr()) == 1 }
    }

    pub fn probe_map(&self) -> QMap<QMapPair_QString_QVariant> {
        let mut m = QMap::<QMapPair_QString_QVariant>::default();
        let Ok(ids) = tinhat_theme::probe::probe_ids(app_tokens()) else { return m };
        let index: BTreeMap<&str, usize> = ids.iter().enumerate().map(|(i, id)| (id.as_str(), i)).collect();
        for e in th_entries() {
            if let Some(&i) = index.get(e.id.as_str()) {
                let (r, g, b) = tinhat_theme::probe::probe_rgb(i);
                m.insert(QString::from(e.name.as_str()), QVariant::from(&QColor::from_rgb(r as i32, g as i32, b as i32)));
            }
        }
        m
    }

    pub fn pick_list(&self, layers_json: &QString) -> QString {
        use tinhat_theme::probe::{pick_list, probe_ids, probe_index, PickHit, PickKind, PickLayer, PICK_MAX_LAYERS};
        let Ok(ids) = probe_ids(app_tokens()) else { return QString::from("[]") };
        let layers_v = parse_json(&layers_json.to_string()).unwrap_or(Value::Null);
        let mut layers: Vec<PickLayer> = Vec::new();
        for l in layers_v.as_array().into_iter().flatten() {
            let mut hits: Vec<PickHit> = Vec::new();
            for p in l.get("props").and_then(Value::as_array).into_iter().flatten() {
                let real = p.get("real").and_then(Value::as_str).unwrap_or("");
                let probe = p.get("probe").and_then(Value::as_str).unwrap_or("");
                if real == probe {
                    continue; // does not depend on the theme
                }
                let Some((r, g, b)) = rgb_of(probe) else { continue };
                let Some(k) = probe_index(r, g, b, ids.len()) else { continue };
                let kind = PickKind::parse(p.get("kind").and_then(Value::as_str).unwrap_or("other")).unwrap_or(PickKind::Other);
                if !hits.iter().any(|h| h.id == ids[k]) {
                    hits.push(PickHit { id: ids[k].clone(), kind });
                }
            }
            layers.push(PickLayer { element: l.get("element").and_then(Value::as_str).unwrap_or("").to_string(), hits });
        }
        let list = pick_list(&layers, PICK_MAX_LAYERS);
        QString::from(canon_json(&Value::Array(list.iter().map(PickHit::to_json).collect())).as_str())
    }
}

/// "#rrggbb" / "#aarrggbb" (Qt's colour name) → rgb.
fn rgb_of(s: &str) -> Option<(u8, u8, u8)> {
    let h = s.strip_prefix('#')?;
    let h = match h.len() {
        6 => h,
        8 => &h[2..],
        _ => return None,
    };
    let v = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    Some((v(0)?, v(2)?, v(4)?))
}

fn token_defaults() -> &'static BTreeMap<String, String> {
    static D: OnceLock<BTreeMap<String, String>> = OnceLock::new();
    D.get_or_init(|| {
        let v = parse_json(tinhat_theme::engine::TOKENS_JSON).unwrap_or(Value::Null);
        let mut m = BTreeMap::new();
        for t in v.get("tokens").and_then(Value::as_array).into_iter().flatten() {
            if let (Some(id), Some(d)) = (t.get("id").and_then(Value::as_str), t.get("default").and_then(Value::as_str)) {
                m.insert(id.to_string(), d.to_string());
            }
        }
        m
    })
}

/// Every token's description (tokens.json), for the search (§13.8: a token row also matches its description).
fn descriptions() -> &'static BTreeMap<String, String> {
    static D: OnceLock<BTreeMap<String, String>> = OnceLock::new();
    D.get_or_init(|| labels::app_token_info().iter().map(|t| (t.id.clone(), t.description.clone())).collect())
}

/// `%20` → ' ' etc. for a file URL's path.
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A shared library theme reads as an ok card with the card's colours and ten swatches as Qt colour strings.
    #[test]
    fn share_rows() {
        let nord = crate::theme_lib::doc("nord").expect("nord in the library").clone();
        let content = Value::Object(share::share_content(&nord, app_tokens()).unwrap());
        let row = share_row(&content);
        assert_eq!(row["kind"], "ok");
        assert_eq!(row["name"], "Nord");
        assert_eq!(row["swatchColours"].as_array().unwrap().len(), 10);
        assert!(row["colours"]["bg"].as_str().unwrap().starts_with('#'));
        assert!(row["themeJson"].as_str().unwrap().contains("\"nord\""));
        // not a card / a plain message
        assert_eq!(share_row(&json!({ "msgtype": "m.text", "body": "hi" })), Value::Null);
        // a newer card
        let mut newer = content.clone();
        newer[share::SHARE_KEY]["v"] = json!(2);
        assert_eq!(share_row(&newer)["kind"], "newer");
        // a theme carrying a web link is refused, never applied
        let mut bad = content.clone();
        bad[share::SHARE_KEY]["theme"]["author"] = json!("see https://example.com");
        assert_eq!(share_row(&bad)["kind"], "refused");
    }

    #[test]
    fn file_urls() {
        if !cfg!(windows) {
            assert_eq!(path_of_url("file:///home/me/My%20Theme.tinhat-theme.json"), "/home/me/My Theme.tinhat-theme.json");
        }
        assert_eq!(rgb_of("#80102030"), Some((0x10, 0x20, 0x30)));
        assert_eq!(rgb_of("#102030"), Some((0x10, 0x20, 0x30)));
        assert_eq!(rgb_of("red"), None);
    }
}
