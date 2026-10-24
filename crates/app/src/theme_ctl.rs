//! theme_ctl.rs — ThemeController (desktop C1; docs/MIGRATING-THEME.md): the theme runtime's QML face.
//!
//! `resolve(id)` runs the engine (theme_rt.rs → crates/theme, `App::Desktop`) and hands QML one colour per property of
//! the `Th` singleton; AppCore.applyTheme() pushes them with `Th.apply(…)` at start and on every switch. The QColors
//! are built exactly like the QML the tokens replace: opaque = 8-bit channels (a `"#rrggbb"` string's QColor), with
//! alpha = `QColor::fromRgbF(r/255, g/255, b/255, a)` — `Qt.rgba(…, a)` / `Qt.alpha(c, a)`'s path, 16-bit alpha
//! identical, so a migrated site paints the same pixels.
//!
//! Qt precision: the engine works in 8-bit channels; Qt's `lighter()` / `darker()` keep 16 bits, and a TEXT (or shader)
//! colour of the old `Qt.lighter(pal.x, f)` differs from its 8-bit rounding by ±1 level on anti-aliased edges. So when
//! a colour's expression (the theme's own override, else the tokens.json default) is `lighter(X, f)` / `darker(X, f)`
//! of an opaque X, Th gets Qt's own `QColor(X).lighter(round(f·100))` — exactly what the old site built — provided it
//! rounds to the engine's 8-bit value (it always has; otherwise the engine's colour stands).
//!
//! The same for the five fills the old QML wrote as float channels OFF the 8-bit grid (`Qt.rgba(0.10, 0.10, 0.12,
//! 0.96)`, QT_FLOAT_LITERALS): Qt keeps them at 16 bits, so their 8-bit rounding blends ±1 level off. While the theme's
//! colour is still that rounding (the built-ins), Th gets the old floats; any other value is the engine's colour.

use core::pin::Pin;
use std::sync::OnceLock;

use cxx_qt_lib::{QColor, QMap, QMapPair_QString_QVariant, QString, QVariant};
use serde_json::Value;
use tinhat_theme::expr::{parse_expr, Expr, FnName};
use tinhat_theme::Colour;

use crate::theme_gen::{self, Entry};
use crate::theme_rt::{self, Active};

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
        #[qproperty(QString, theme_id)] //   QML: themeId — the theme resolve() applied last (after the OLED fallback)
        #[qproperty(QString, theme_name)] // QML: themeName — its display name ("OLED")
        #[qproperty(QString, variant)] //    QML: variant — "dark" | "light"
        #[qproperty(QString, names)] //      QML: names — "hash-hsl" | "palette" (how people's names are coloured)
        #[qproperty(QString, author)] //     QML: author — the applied theme's author ("" = none; THEMES-UI §3.2)
        #[qproperty(i32, adjusted)] //       QML: adjusted — how many of its colours the readability guard moved
        #[qproperty(QString, shape)] //      QML: shape — its `shape` block as JSON {border, gap, margin, radius} (11.0 islands)
        #[qproperty(QString, look)] //       QML: look — its look (ENGINE.md §10: fonts, size, density, shadow) as JSON (11.0 step 5)
        // themes step 1b (theme_store.rs, contract THEMES-SYNC.md / THEMES-UI.md):
        #[qproperty(QString, effective_theme)] // QML: effectiveTheme — the theme this device renders (AppCore applies it)
        #[qproperty(QString, effective_key)] //   QML: effectiveKey — "<id>|<hash of its own document, 0 = the library's>"
        #[qproperty(QString, sound_pack)] //      QML: soundPack — "default" | "goose" (THEMES-UI §10.4)
        #[qproperty(QString, icon_style)] //      QML: iconStyle — "duotone" | "mono" (THEMES-UI §10.3)
        #[qproperty(bool, override_on)] //        QML: overrideOn — "Use a different theme on this device"
        #[qproperty(bool, syncing)] //            QML: syncing — THEMES-SYNC on (false = gate mode TINHAT_GUI_THEMESYNC=0)
        #[qproperty(QString, state_json)] //      QML: stateJson — theme_store.rs ui_state (notices' data, My themes, the ☰ menu's rows, …)
        #[qproperty(i32, state_rev)] //           QML: stateRev — bumps with stateJson
        #[qproperty(i32, applied_rev)] //         QML: appliedRev — bumps after every apply of a person's on this device
        //                                        (a real apply ends a share card's Try without going back, THEMES-UI §14.4)
        type ThemeController = super::ThemeControllerRust;

        /// QML: themeCtl.resolve(id) — resolve a built-in or `<data>/themes/<id>.json` (anything unknown or refused →
        /// OLED), make it the process-wide theme, set themeId/themeName/variant/names and return
        /// `{ <Th property>: color }` for Th.apply().
        #[qinvokable]
        fn resolve(self: Pin<&mut ThemeController>, id: &QString) -> QMap_QString_QVariant;

        /// QML: themeCtl.known(id) — would resolve(id) apply this very id (no fallback)?
        #[qinvokable]
        fn known(self: &ThemeController, id: &QString) -> bool;

    }

    // ---- themes step 1b (theme_store.rs, contract THEMES-SYNC.md / THEMES-UI.md): what the device renders, the
    // person's picks, the gallery, import, My themes.
    #[auto_cxx_name]
    extern "RustQt" {
        /// A notice for a toast with an action (THEMES-UI §7, §10.6): kind "changed" (payload = the content shown before,
        /// for keepHere) or "offer" (payload = the sound pack, for applyPack), the text, the action's label.
        #[qsignal]
        fn notice(self: Pin<&mut ThemeController>, kind: QString, text: QString, action: QString, payload: QString);

        /// QML: themeCtl.start() — attach to the theme store and publish what the device renders (AppCore, once).
        #[qinvokable]
        fn start(self: Pin<&mut ThemeController>);
        /// QML: themeCtl.applyTheme(id, inSettings) — a person's pick (a card, the ☰ menu): "" or the failure in plain
        /// words. `inSettings`: a sound offer shows in the settings instead of a toast (THEMES-UI §10.6).
        #[qinvokable]
        fn apply_theme(self: Pin<&mut ThemeController>, id: &QString, in_settings: bool) -> QString;
        /// QML: themeCtl.undoTheme() — re-apply what was shown before the last apply (THEMES-UI §3.2).
        #[qinvokable]
        fn undo_theme(self: Pin<&mut ThemeController>) -> QString;
        /// QML: themeCtl.endSession() — the Settings sheet opened or closed: Undo and an unanswered offer end.
        #[qinvokable]
        fn end_session(self: Pin<&mut ThemeController>);
        /// QML: themeCtl.applyPack("sounds"|"icons", value) — the sound pack / icon style.
        #[qinvokable]
        fn apply_pack(self: Pin<&mut ThemeController>, key: &QString, value: &QString) -> QString;
        /// QML: themeCtl.setDeviceOverride(on) — "Use a different theme on this device".
        #[qinvokable]
        fn set_device_override(self: Pin<&mut ThemeController>, on: bool);
        /// QML: themeCtl.keepHere(payload) — the "changed on another device" toast's "Keep ‹old› here".
        #[qinvokable]
        fn keep_here(self: Pin<&mut ThemeController>, payload: &QString);
        /// QML: themeCtl.acceptOffer() / dismissOffer() — "Use Goose sounds too?" (THEMES-UI §10.6).
        #[qinvokable]
        fn accept_offer(self: Pin<&mut ThemeController>);
        #[qinvokable]
        fn dismiss_offer(self: Pin<&mut ThemeController>);
        /// QML: themeCtl.gateTheme(id) — gate mode: the pick QML already applied the 10.6 way (login.setTheme).
        #[qinvokable]
        fn gate_theme(self: Pin<&mut ThemeController>, id: &QString);
        /// QML: themeCtl.galleryJson(query, filter) — `{favourites, mine, all}` of `{id, name, author, variant}`
        /// (THEMES-UI §4: search words in name / author / id, filter "all" | "dark" | "light"; All sorted by name).
        #[qinvokable]
        fn gallery_json(self: &ThemeController, query: &QString, filter: &QString) -> QString;
        /// QML: themeCtl.card(id) — a card's colours (THEMES-UI §5.1: the card theme's roles, Qt colour strings) +
        /// name, author, variant, adjusted, usable.
        #[qinvokable]
        fn card(self: &ThemeController, id: &QString) -> QMap_QString_QVariant;
        /// QML: themeCtl.cardForDoc(json) — the same for a theme document (the import preview).
        #[qinvokable]
        fn card_for_doc(self: &ThemeController, doc_json: &QString) -> QMap_QString_QVariant;
        /// QML: themeCtl.importText(text, fileName) — THEMES-UI §6: `{ok, format, formatName, name, variant,
        /// adjusted, doc}` or `{ok: false, code, text}` (plain words, §6.1).
        #[qinvokable]
        fn import_text(self: &ThemeController, text: &QString, file_name: &QString) -> QString;
        /// QML: themeCtl.readFile(url) — a chosen file's text (≤ 256 KB): `{ok, text, name}` or `{ok: false, text}`.
        #[qinvokable]
        fn read_file(self: &ThemeController, url: &QString) -> QString;
        /// QML: themeCtl.saveImported(docJson, name, useIt) — to My themes (+ apply): `{ok, id}` or `{ok: false, text}`.
        #[qinvokable]
        fn save_imported(self: Pin<&mut ThemeController>, doc_json: &QString, name: &QString, use_it: bool) -> QString;
        /// QML: themeCtl.renameMine(id, name) / removeMine(id) (→ the removed document's JSON for Undo, "" = none) /
        /// restoreMine(docJson).
        #[qinvokable]
        fn rename_mine(self: Pin<&mut ThemeController>, id: &QString, name: &QString) -> QString;
        #[qinvokable]
        fn remove_mine(self: Pin<&mut ThemeController>, id: &QString) -> QString;
        #[qinvokable]
        fn restore_mine(self: Pin<&mut ThemeController>, doc_json: &QString) -> QString;
    }

    impl cxx_qt::Threading for ThemeController {}
}

#[derive(Default)]
pub struct ThemeControllerRust {
    theme_id: QString,
    theme_name: QString,
    variant: QString,
    names: QString,
    author: QString,
    adjusted: i32,
    shape: QString,
    look: QString,
    effective_theme: QString,
    effective_key: QString,
    sound_pack: QString,
    icon_style: QString,
    override_on: bool,
    syncing: bool,
    state_json: QString,
    state_rev: i32,
    applied_rev: i32,
}

/// Th's colours in file order (the roles, then every token) — the list build.rs generated qml/Th.qml from.
pub fn th_entries() -> &'static [Entry] {
    static ENTRIES: OnceLock<Vec<Entry>> = OnceLock::new();
    ENTRIES.get_or_init(|| {
        let v = tinhat_theme::parse_json(tinhat_theme::engine::TOKENS_JSON).expect("tokens.json is JSON");
        theme_gen::entries(&v).expect("tokens.json makes a Th singleton (build.rs checked it)")
    })
}

/// The QColor QML would have built for `c` (see the module doc).
pub fn qcolor(c: &Colour) -> QColor {
    if c.is_opaque() {
        QColor::from_rgb(c.r as i32, c.g as i32, c.b as i32)
    } else {
        let f = |v: u8| (v as f64 / 255.0) as f32;
        QColor::from_rgba_f(f(c.r), f(c.g), f(c.b), (c.k as f64 / 10000.0) as f32)
    }
}

/// The expression that paints `e` in theme `a`: the document's own `tokens` (or, for a role, `roles`) entry, else a
/// token's tokens.json default. `None` for a role the engine derives (a palette theme).
fn effective_expr<'a>(a: &'a Active, e: &'a Entry) -> Option<&'a str> {
    let group = if e.role { "roles" } else { "tokens" };
    if let Some(s) = a.doc.get(group).and_then(|g| g.get(&e.id)).and_then(Value::as_str) {
        return Some(s);
    }
    (!e.role).then_some(e.default.as_str())
}

/// Qt's own 16-bit `lighter()` / `darker()` of an opaque colour for a `lighter(X, f)` / `darker(X, f)` expression
/// (see the module doc), or `None` when the expression is anything else.
fn qt_precise(a: &Active, src: &str) -> Option<QColor> {
    let head = src.trim_start();
    if !head.starts_with("lighter") && !head.starts_with("darker") {
        return None; // the common case: no parse per colour and switch
    }
    let Ok(Expr::Fn { f, args, k }) = parse_expr(src) else { return None };
    if !matches!(f, FnName::Lighter | FnName::Darker) || args.len() != 1 {
        return None;
    }
    // The base at 8 bits (every palette string was): a base that is itself a lighter()/darker() would be 16-bit in
    // the old QML — no token or built-in role nests them today; the rounding check below guards it anyway.
    let base = match &args[0] {
        Expr::Lit(c) => *c,
        Expr::Ref(id) => a.colour(id)?,
        Expr::Fn { .. } => return None,
    };
    if !base.is_opaque() {
        return None;
    }
    let factor = tinhat_theme::js_round(k as f64 / 100.0) as i32; // QQuickColorProvider: int(qRound(factor * 100))
    let q = qcolor(&base);
    Some(if f == FnName::Lighter { q.lighter(factor) } else { q.darker(factor) })
}

/// The fills the old QML built from float channels off the 8-bit grid (the whole list: a scan of every
/// `Qt.rgba(…)` literal in base/11.0), with the token that paints each today.
const QT_FLOAT_LITERALS: [(&str, [f32; 4]); 5] = [
    ("callbar.chevron.bg", [0.10, 0.10, 0.12, 0.96]),       // Main.qml — the call bar ▴ disc at rest
    ("reply.bg", [0.5, 0.5, 0.5, 0.10]),                     // ConversationView.qml — the reply / edit bar
    ("overlay.pill.bg", [0.04, 0.04, 0.05, 0.82]),           // OverlayWindow.qml — the voice HUD pill
    ("overlay.pill.bg.error", [0.35, 0.08, 0.08, 0.92]),     //   … muted while talking
    ("overlay.pill.bg.speaking", [0.04, 0.09, 0.08, 0.92]),  //   … speaking
];

/// Does `q` round to the engine colour `c` on every 8-bit channel?
fn rounds_to(q: &QColor, c: &Colour) -> bool {
    (q.red(), q.green(), q.blue(), q.alpha()) == (c.r as i32, c.g as i32, c.b as i32, c.alpha_u8() as i32)
}

/// The QColor Th gets for `e`: the engine's colour (qcolor), refined to Qt precision where it applies.
pub fn entry_qcolor(a: &Active, e: &Entry) -> QColor {
    let c = a.colour(&e.id).unwrap_or_else(|| panic!("theme '{}' resolves no colour for {}", a.id, e.id));
    if let Some((_, [r, g, b, al])) = QT_FLOAT_LITERALS.iter().find(|(id, _)| *id == e.id) {
        let q = QColor::from_rgba_f(*r, *g, *b, *al);
        if rounds_to(&q, &c) {
            return q;
        }
    }
    if let Some(q) = effective_expr(a, e).and_then(|src| qt_precise(a, src)) {
        if rounds_to(&q, &c) {
            return q;
        }
        eprintln!("[theme] {}: Qt's {} rounds away from the engine's {} — using the engine's", e.id, q.name(cxx_qt_lib::QColorNameFormat::HexArgb), c);
    }
    qcolor(&c)
}

/// `{ <Th property>: color }` of a resolved theme.
pub fn colour_map(a: &Active) -> QMap<QMapPair_QString_QVariant> {
    let mut m = QMap::<QMapPair_QString_QVariant>::default();
    for e in th_entries() {
        m.insert(QString::from(e.name.as_str()), QVariant::from(&entry_qcolor(a, e)));
    }
    m
}

impl qobject::ThemeController {
    pub fn resolve(mut self: Pin<&mut Self>, id: &QString) -> QMap<QMapPair_QString_QVariant> {
        let a = theme_rt::activate(&id.to_string());
        self.as_mut().set_theme_id(QString::from(a.id.as_str()));
        self.as_mut().set_theme_name(QString::from(a.name.as_str()));
        self.as_mut().set_variant(QString::from(a.resolved.variant.as_str()));
        self.as_mut().set_names(QString::from(a.resolved.names.as_str()));
        let author = a.doc.get("author").and_then(Value::as_str).unwrap_or("").to_string();
        self.as_mut().set_author(QString::from(&author));
        self.as_mut().set_adjusted(a.resolved.adjusted.len() as i32);
        // 11.0 (ISLANDS-ARCH §3.4): the island geometry of the theme (defaults for a theme without a `shape` block)
        self.as_mut().set_shape(QString::from(tinhat_theme::shape::read_shape(&a.doc).to_json().as_str()));
        // 11.0 step 5 (ENGINE.md §10): fonts from the allowed lists, the text size, density, the island shadow
        self.as_mut().set_look(QString::from(tinhat_theme::look::look_json(&tinhat_theme::look::read_look(&a.doc)).as_str()));
        colour_map(&a)
    }

    pub fn known(&self, id: &QString) -> bool {
        theme_rt::known(&id.to_string())
    }

    // ---- themes step 1b -----------------------------------------------------------------------------------------

    pub fn start(mut self: Pin<&mut Self>) {
        use cxx_qt::Threading;
        crate::theme_store::attach_sink(self.qt_thread());
        let on = crate::theme_store::sync_enabled();
        self.as_mut().set_syncing(on);
        refresh(self);
    }

    pub fn apply_theme(mut self: Pin<&mut Self>, id: &QString, in_settings: bool) -> QString {
        let r = crate::theme_store::apply_theme(&id.to_string(), in_settings);
        refresh(self.as_mut());
        QString::from(&r.error())
    }

    pub fn undo_theme(mut self: Pin<&mut Self>) -> QString {
        let r = crate::theme_store::undo();
        refresh(self.as_mut());
        QString::from(&r.error())
    }

    pub fn end_session(self: Pin<&mut Self>) {
        crate::theme_store::end_session();
        refresh(self);
    }

    pub fn apply_pack(mut self: Pin<&mut Self>, key: &QString, value: &QString) -> QString {
        let r = crate::theme_store::apply_pack(&key.to_string(), &value.to_string());
        refresh(self.as_mut());
        QString::from(&r.error())
    }

    pub fn set_device_override(self: Pin<&mut Self>, on: bool) {
        crate::theme_store::set_override(on);
        refresh(self);
    }

    pub fn keep_here(self: Pin<&mut Self>, payload: &QString) {
        if let Ok(v) = tinhat_theme::parse_json(&payload.to_string()) {
            crate::theme_store::keep_here(&v);
        }
        refresh(self);
    }

    pub fn accept_offer(self: Pin<&mut Self>) {
        let _ = crate::theme_store::accept_offer();
        refresh(self);
    }

    pub fn dismiss_offer(self: Pin<&mut Self>) {
        crate::theme_store::dismiss_offer();
        refresh(self);
    }

    pub fn gate_theme(self: Pin<&mut Self>, id: &QString) {
        let _ = crate::theme_store::apply_theme(&id.to_string(), false);
        refresh(self);
    }

    pub fn gallery_json(&self, query: &QString, filter: &QString) -> QString {
        QString::from(&gallery(&query.to_string(), &filter.to_string()).to_string())
    }

    pub fn card(&self, id: &QString) -> QMap<QMapPair_QString_QVariant> {
        let id = id.to_string();
        if let Some(p) = crate::theme_lib::preview(&id) {
            let e = crate::theme_lib::library().get(&id);
            let name = e.map(|e| e.name.as_str()).unwrap_or(id.as_str());
            let author = e.map(|e| e.author.as_str()).unwrap_or("");
            return card_map(Ok(&p), name, author);
        }
        match crate::theme_store::my_doc(&id) {
            Some(d) => card_of_doc(&Value::Object(d)),
            None => card_map(Err("unknown"), &id, ""),
        }
    }

    pub fn card_for_doc(&self, doc_json: &QString) -> QMap<QMapPair_QString_QVariant> {
        match tinhat_theme::parse_theme_json(&doc_json.to_string()) {
            Ok(d) => card_of_doc(&d),
            Err(e) => card_map(Err(&e.to_string()), "", ""),
        }
    }

    pub fn import_text(&self, text: &QString, file_name: &QString) -> QString {
        let name = file_name.to_string();
        QString::from(&import_json(&text.to_string(), (!name.is_empty()).then_some(name.as_str())).to_string())
    }

    pub fn read_file(&self, url: &QString) -> QString {
        QString::from(&read_theme_file(&url.to_string()).to_string())
    }

    pub fn save_imported(mut self: Pin<&mut Self>, doc_json: &QString, name: &QString, use_it: bool) -> QString {
        let out = match tinhat_theme::parse_theme_json(&doc_json.to_string()) {
            Ok(Value::Object(d)) => match crate::theme_store::save_new(&d, name.to_string().trim(), use_it) {
                Ok(id) => serde_json::json!({ "ok": true, "id": id }),
                Err(why) => serde_json::json!({ "ok": false, "text": why }),
            },
            _ => serde_json::json!({ "ok": false, "text": "This isn't a theme." }),
        };
        refresh(self.as_mut());
        QString::from(&out.to_string())
    }

    pub fn rename_mine(mut self: Pin<&mut Self>, id: &QString, name: &QString) -> QString {
        let r = crate::theme_store::rename_mine(&id.to_string(), name.to_string().trim());
        refresh(self.as_mut());
        QString::from(&r.error())
    }

    pub fn remove_mine(mut self: Pin<&mut Self>, id: &QString) -> QString {
        let r = crate::theme_store::remove_mine(&id.to_string());
        refresh(self.as_mut());
        QString::from(&r.map(|d| Value::Object(d).to_string()).unwrap_or_default())
    }

    pub fn restore_mine(mut self: Pin<&mut Self>, doc_json: &QString) -> QString {
        let r = match tinhat_theme::parse_theme_json(&doc_json.to_string()) {
            Ok(Value::Object(d)) => crate::theme_store::restore_mine(&d),
            _ => crate::theme_store::Outcome::Nothing,
        };
        refresh(self.as_mut());
        QString::from(&r.error())
    }
}

/// Publish the theme store's view to the controller (GUI thread): the rendered theme / sound pack / icon style (QML
/// applies them on change), the switch, the UI state and the queued notices.
pub fn refresh(mut o: Pin<&mut qobject::ThemeController>) {
    let s = crate::theme_store::ui_state();
    let v = &s.view;
    if o.effective_theme().to_string() != v.theme {
        o.as_mut().set_effective_theme(QString::from(v.theme.as_str()));
    }
    // the key changes with the id and with the rendered document (a renamed My theme keeps its id)
    let doc_hash = match &v.doc {
        Some(d) => {
            let text = tinhat_theme::canon_json(&Value::Object(d.clone()));
            let mut h: u32 = 0x811c_9dc5;
            for b in text.bytes() {
                h ^= b as u32;
                h = h.wrapping_mul(0x0100_0193);
            }
            format!("{h:08x}")
        }
        None => "0".to_string(),
    };
    let key = format!("{}|{doc_hash}", v.theme);
    if o.effective_key().to_string() != key {
        o.as_mut().set_effective_key(QString::from(&key));
    }
    if o.sound_pack().to_string() != v.sounds {
        o.as_mut().set_sound_pack(QString::from(v.sounds));
    }
    if o.icon_style().to_string() != v.icons {
        o.as_mut().set_icon_style(QString::from(v.icons));
    }
    if *o.override_on() != s.override_on {
        o.as_mut().set_override_on(s.override_on);
    }
    let j = s.json.to_string();
    if o.state_json().to_string() != j {
        o.as_mut().set_state_json(QString::from(&j));
        let r = *o.state_rev() + 1;
        o.as_mut().set_state_rev(r);
    }
    let applied = (s.applied & 0x3fff_ffff) as i32;
    if *o.applied_rev() != applied {
        o.as_mut().set_applied_rev(applied);
    }
    for (kind, text, action, payload) in s.notices {
        o.as_mut().notice(QString::from(&kind), QString::from(&text), QString::from(&action), QString::from(&payload));
    }
}

// ---- the gallery (THEMES-UI §4) -----------------------------------------------------------------------------------

/// Search: the query lower-cased and split on whitespace; a theme matches when every word occurs in its name, author
/// or id (lower-cased). Filter: "dark" / "light" = the resolved variant.
pub fn matches(words: &[String], filter: &str, name: &str, author: &str, id: &str, variant: tinhat_theme::Variant) -> bool {
    if (filter == "dark" && variant != tinhat_theme::Variant::Dark) || (filter == "light" && variant != tinhat_theme::Variant::Light) {
        return false;
    }
    let (n, a) = (name.to_lowercase(), author.to_lowercase());
    words.iter().all(|w| n.contains(w.as_str()) || a.contains(w.as_str()) || id.contains(w.as_str()))
}

pub fn query_words(q: &str) -> Vec<String> {
    q.to_lowercase().split_whitespace().map(str::to_string).collect()
}

/// `{favourites, mine, all}`: the library's favourites in library order, My themes newest first, every library theme
/// sorted by the lower-cased name (code-point order) then id — each `{id, name, author, variant}`, filtered.
pub fn gallery(query: &str, filter: &str) -> Value {
    let lib = crate::theme_lib::library();
    let words = query_words(query);
    let item = |e: &crate::theme_lib::Entry| serde_json::json!({ "id": e.id, "name": e.name, "author": e.author, "variant": e.variant.as_str() });
    let favourites: Vec<Value> = lib
        .favourites
        .iter()
        .filter_map(|id| lib.get(id))
        .filter(|e| matches(&words, filter, &e.name, &e.author, &e.id, e.variant))
        .map(item)
        .collect();
    let mut all: Vec<&crate::theme_lib::Entry> =
        lib.entries.iter().filter(|e| matches(&words, filter, &e.name, &e.author, &e.id, e.variant)).collect();
    all.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then_with(|| a.id.cmp(&b.id)));
    let mine: Vec<Value> = crate::theme_store::mine_items()
        .into_iter()
        .filter(|m| {
            let v = if m["variant"] == "light" { tinhat_theme::Variant::Light } else { tinhat_theme::Variant::Dark };
            matches(&words, filter, m["name"].as_str().unwrap_or(""), m["author"].as_str().unwrap_or(""), m["id"].as_str().unwrap_or(""), v)
        })
        .collect();
    serde_json::json!({ "favourites": favourites, "mine": mine, "all": all.into_iter().map(item).collect::<Vec<_>>() })
}

// ---- cards (THEMES-UI §5.1) -----------------------------------------------------------------------------------------

/// The roles a card paints (THEMES-UI §5.1), under the QML names ThemeCard.qml reads.
const CARD_ROLES: [(&str, &str); 15] = [
    ("window", "surface.window"),
    ("island", "surface.island"),
    ("raised", "surface.raised"),
    ("selected", "surface.selected"),
    ("text", "text.primary"),
    ("textSecondary", "text.secondary"),
    ("accent", "accent.base"),
    ("accentBright", "accent.bright"),
    ("onAccent", "text.onAccent"),
    ("border", "border.subtle"),
    ("name1", "name.1"),
    ("name2", "name.2"),
    ("name3", "name.3"),
    ("name4", "name.4"),
    ("name5", "name.5"),
];

fn card_map(p: Result<&crate::theme_lib::Preview, &str>, name: &str, author: &str) -> QMap<QMapPair_QString_QVariant> {
    let mut m = QMap::<QMapPair_QString_QVariant>::default();
    let s = |x: &str| QVariant::from(&QString::from(x));
    m.insert(QString::from("name"), s(name));
    m.insert(QString::from("author"), s(author));
    match p {
        Ok(p) => {
            m.insert(QString::from("ok"), QVariant::from(&true));
            m.insert(QString::from("variant"), s(p.variant.as_str()));
            m.insert(QString::from("adjusted"), QVariant::from(&(p.adjusted.len() as i32)));
            for (key, role) in CARD_ROLES {
                let c = p.roles.get(role).and_then(|x| tinhat_theme::parse_literal(x)).unwrap_or(Colour::BLACK);
                m.insert(QString::from(key), s(&theme_rt::qt_string(&c)));
            }
        }
        Err(why) => {
            m.insert(QString::from("ok"), QVariant::from(&false));
            m.insert(QString::from("error"), s(why));
            m.insert(QString::from("variant"), s("dark"));
            m.insert(QString::from("adjusted"), QVariant::from(&0i32));
        }
    }
    m
}

fn card_of_doc(d: &Value) -> QMap<QMapPair_QString_QVariant> {
    let name = d.get("name").and_then(Value::as_str).unwrap_or("");
    let author = d.get("author").and_then(Value::as_str).unwrap_or("");
    // a card judges the theme like the app would apply it: the engine with the real token list
    if let Err(e) = tinhat_theme::resolve_theme(d, tinhat_theme::app_tokens()) {
        return card_map(Err(&e.to_string()), name, author);
    }
    match crate::theme_lib::preview_doc(d) {
        Ok(p) => card_map(Ok(&p), name, author),
        Err(e) => card_map(Err(&e), name, author),
    }
}

// ---- import (THEMES-UI §6) ------------------------------------------------------------------------------------------

fn format_name(f: tinhat_theme::ImportFormat) -> &'static str {
    use tinhat_theme::ImportFormat as F;
    match f {
        F::Tinhat => "Tin Hat",
        F::Iterm2 => "iTerm2",
        F::Kitty => "kitty",
        F::AlacrittyToml | F::AlacrittyYaml => "Alacritty",
        F::Ghostty => "Ghostty",
        F::Wezterm => "WezTerm",
        F::WindowsTerminal => "Windows Terminal",
        F::Xresources => "Xresources",
        F::Base16 => "base16",
        F::Base24 => "base24",
    }
}

/// THEMES-UI §6.1: a palette slot's plain name.
fn slot_name(slot: &str) -> String {
    const HUES: [&str; 8] = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"];
    match slot {
        "bg" => "background".into(),
        "fg" => "text".into(),
        "cursor" => "cursor".into(),
        "selectionBg" | "selectionFg" => "selection".into(),
        s => match s.strip_prefix('c').and_then(|n| n.parse::<usize>().ok()) {
            Some(n) if n < 8 => HUES[n].into(),
            Some(n) if n < 16 => format!("bright {}", HUES[n - 8]),
            _ => s.to_string(),
        },
    }
}

/// THEMES-UI §6.1: an import or engine refusal in plain words.
pub fn plain_import_error(code: &str, detail: &str) -> String {
    match code {
        "import-empty" => "Paste a theme or choose a file first.".into(),
        "import-too-large" => "That file is too big to be a theme (the limit is 256 KB).".into(),
        "unknown-format" => "Tin Hat doesn't recognise this format. It reads kitty, Alacritty, Ghostty, iTerm2, WezTerm, \
                             Windows Terminal, Xresources and base16 / base24 themes."
            .into(),
        "import-incomplete" => {
            let slot = detail.strip_prefix("missing ").unwrap_or(detail);
            format!("This theme has no {} colour, so Tin Hat can't use it.", slot_name(slot))
        }
        "import-bad-colour" => {
            // detail: "<slot> '<value>'"
            let (slot, v) = detail.split_once(' ').unwrap_or((detail, ""));
            let v = v.trim().trim_start_matches('\'').trim_end_matches('\'');
            format!("The {} colour (“{v}”) isn't a colour Tin Hat can read.", slot_name(slot))
        }
        "bad-json" => "This isn't valid JSON.".into(),
        "bad-toml" | "bad-yaml" => {
            let kind = if code == "bad-toml" { "TOML" } else { "YAML" };
            match detail.strip_prefix("line ").and_then(|n| n.split(|c: char| !c.is_ascii_digit()).next()) {
                Some(n) if !n.is_empty() => format!("There's a mistake on line {n} of this {kind} file."),
                _ => format!("There's a mistake in this {kind} file."),
            }
        }
        "bad-plist" => "This isn't a valid iTerm2 colours file.".into(),
        "too-large" => "This theme is larger than 8 KB.".into(),
        "url" => "Themes can't contain web links.".into(),
        "too-many-overrides" => "This theme changes more than 400 colours.".into(),
        c => format!("This Tin Hat theme can't be used ({c})."),
    }
}

/// `importTheme(text, fileName)` + the engine's verdict, for the import dialog.
pub fn import_json(text: &str, file_name: Option<&str>) -> Value {
    match tinhat_theme::import_theme(text, file_name) {
        Err(e) => serde_json::json!({ "ok": false, "code": e.code, "text": plain_import_error(e.code, &e.detail) }),
        Ok(r) => match tinhat_theme::resolve_theme(&r.theme, tinhat_theme::app_tokens()) {
            Err(e) => serde_json::json!({ "ok": false, "code": e.code, "text": plain_import_error(e.code, &e.detail) }),
            Ok(res) => serde_json::json!({
                "ok": true,
                "format": r.format.as_str(),
                "formatName": format_name(r.format),
                "name": r.theme.get("name").and_then(Value::as_str).unwrap_or(""),
                "variant": res.variant.as_str(),
                "adjusted": res.adjusted.len(),
                "doc": r.theme,
            }),
        },
    }
}

/// A file chosen in the import dialog: its text (invalid UTF-8 → U+FFFD, like the web's file reader), ≤ 256 KB.
pub fn read_theme_file(url: &str) -> Value {
    let path = url.strip_prefix("file://").map(|p| percent_decode(p)).unwrap_or_else(|| url.to_string());
    // Windows file URLs are file:///C:/…
    let path = if cfg!(windows) { path.trim_start_matches('/').to_string() } else { path };
    let name = std::path::Path::new(&path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    match std::fs::metadata(&path) {
        Ok(m) if m.len() as usize > tinhat_theme::import::MAX_IMPORT_BYTES => {
            serde_json::json!({ "ok": false, "text": plain_import_error("import-too-large", ""), "name": name })
        }
        Ok(_) => match std::fs::read(&path) {
            Ok(b) => serde_json::json!({ "ok": true, "text": String::from_utf8_lossy(&b), "name": name }),
            Err(e) => serde_json::json!({ "ok": false, "text": format!("Tin Hat can't read that file ({e}).") }),
        },
        Err(e) => serde_json::json!({ "ok": false, "text": format!("Tin Hat can't read that file ({e}).") }),
    }
}

/// `%xx` escapes of a file URL's path.
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok());
            if let Some(v) = hex {
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

    /// The built-ins' roles are exactly the palette tables Main.qml carried before the runtime (10.6.0 `themes`).
    #[test]
    fn builtins_keep_the_10_6_palettes() {
        let roles = [
            "surface.window", "surface.island", "surface.raised", "text.primary", "text.secondary", "border.subtle",
            "accent.base", "accent.bright", "state.danger", "state.dangerBright", "state.warning", "state.success",
            "text.onAccent", "state.info", "state.aside",
        ];
        let old = [
            ("oled", "OLED", [
                "#000000", "#0a0a0a", "#1a1a1a", "#e0e0e0", "#aaaaaa", "#222222", "#1e6a76", "#3cbad0", "#c74848",
                "#ff6b6b", "#e0a020", "#3ad07a", "#ffffff", "#4da3ff", "#e8b923",
            ]),
            ("goose", "Goose", [
                "#1a0a1a", "#2a1030", "#351540", "#f0e0f0", "#c8a0c8", "#4a2050", "#b0508c", "#e87cc0", "#d1506e",
                "#ff7c9c", "#dfa44c", "#58c88a", "#ffffff", "#8fa6ff", "#e8b923",
            ]),
        ];
        for (id, name, values) in old {
            let a = theme_rt::try_resolve_in(id, std::path::Path::new("/nonexistent")).unwrap();
            assert_eq!(a.name, name);
            for (role, want) in roles.iter().zip(values) {
                let c = a.colour(role).unwrap();
                assert!(c.is_opaque(), "{id}: {role}");
                assert_eq!(c.hex6(), want, "{id}: {role}");
            }
        }
    }

    /// scripts/theme-migrate.py --verify: every role and token of the built-ins as the engine resolves them
    /// (canonical text) → $THEME_DUMP. Run: `cargo test --release -p tinhat-app dump_resolved_builtins --
    /// --ignored` with THEME_DUMP=<file>.
    #[test]
    #[ignore]
    fn dump_resolved_builtins() {
        let path = std::env::var("THEME_DUMP").expect("THEME_DUMP=<file>");
        let mut out = serde_json::Map::new();
        for id in ["oled", "goose"] {
            let a = theme_rt::try_resolve_in(id, std::path::Path::new("/nonexistent")).unwrap();
            let m: serde_json::Map<String, serde_json::Value> = a
                .resolved
                .roles
                .iter()
                .chain(a.resolved.tokens.iter())
                .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
                .collect();
            out.insert(id.to_string(), serde_json::Value::Object(m));
        }
        std::fs::write(path, serde_json::to_string_pretty(&serde_json::Value::Object(out)).unwrap()).unwrap();
    }

    /// Qt precision: a lighter()/darker() colour is Qt's own 16-bit QColor (what `Qt.lighter(pal.x, f)` built), still
    /// the engine's 8-bit value; everything else is the 8-bit engine colour.
    #[test]
    fn lighter_and_darker_keep_qt_precision() {
        let oled = theme_rt::try_resolve_in("oled", std::path::Path::new("/nonexistent")).unwrap();
        let entry = |id: &str| th_entries().iter().find(|e| e.id == id).unwrap().clone();
        // surface.hover = lighter(surface.island, 1.18) (a role of the built-in): Qt.lighter("#0a0a0a", 1.18)
        let hover = entry_qcolor(&oled, &entry("surface.hover"));
        let qt = QColor::from_rgb(10, 10, 10).lighter(118);
        assert_eq!(hover, qt);
        assert_eq!((hover.red(), hover.green(), hover.blue()), (12, 12, 12));
        assert_ne!(hover, QColor::from_rgb(12, 12, 12)); // 16-bit: not the 8-bit rounding
        // input.bg.focus = lighter(surface.raised, 1.08) (a tokens.json default)
        assert_eq!(entry_qcolor(&oled, &entry("input.bg.focus")), QColor::from_rgb(26, 26, 26).lighter(108));
        // a plain reference stays the 8-bit colour
        assert_eq!(entry_qcolor(&oled, &entry("input.bg")), QColor::from_rgb(0, 0, 0));
    }

    /// The off-grid float fills: the built-ins get the old `Qt.rgba(…)` QColor (16-bit, not the 8-bit rounding); a
    /// theme that paints the token any other colour gets the engine's.
    #[test]
    fn float_literals_keep_qt_precision() {
        let entry = |id: &str| th_entries().iter().find(|e| e.id == id).unwrap().clone();
        for id in ["oled", "goose"] {
            let a = theme_rt::try_resolve_in(id, std::path::Path::new("/nonexistent")).unwrap();
            for (tok, [r, g, b, al]) in QT_FLOAT_LITERALS {
                let got = entry_qcolor(&a, &entry(tok));
                assert_eq!(got, QColor::from_rgba_f(r, g, b, al), "{id}: {tok}");
                assert_ne!(got, qcolor(&a.colour(tok).unwrap()), "{id}: {tok} is off the grid");
            }
        }
        let dir = std::env::temp_dir().join(format!("th-float-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut doc = tinhat_theme::builtin_theme("oled").unwrap().clone();
        doc["id"] = Value::from("float-probe");
        doc["tokens"]["callbar.chevron.bg"] = Value::from("#202020/0.96");
        std::fs::write(dir.join("float-probe.json"), serde_json::to_string(&doc).unwrap()).unwrap();
        let a = theme_rt::try_resolve_in("float-probe", &dir).unwrap();
        assert_eq!(entry_qcolor(&a, &entry("callbar.chevron.bg")), qcolor(&a.colour("callbar.chevron.bg").unwrap()));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Every Th entry resolves in both built-ins, and the derived palette values the old QML computed with
    /// Qt.rgba/Qt.lighter come out of the engine with the same 8-bit value.
    #[test]
    fn builtins_fill_every_entry() {
        for id in ["oled", "goose"] {
            let a = theme_rt::try_resolve_in(id, std::path::Path::new("/nonexistent")).unwrap();
            for e in th_entries() {
                assert!(a.colour(&e.id).is_some(), "{id}: {}", e.id);
            }
        }
        let oled = theme_rt::try_resolve_in("oled", std::path::Path::new("/nonexistent")).unwrap();
        assert_eq!(oled.colour("accent.dim").unwrap().to_canonical(), "#1e6a76/0.18");
        assert_eq!(oled.colour("accent.glow").unwrap().to_canonical(), "#1e6a76/0.35");
        assert_eq!(oled.colour("surface.hover").unwrap().to_canonical(), "#0c0c0c"); // Qt.lighter(#0a0a0a, 1.18)
        let goose = theme_rt::try_resolve_in("goose", std::path::Path::new("/nonexistent")).unwrap();
        assert_eq!(goose.colour("surface.hover").unwrap().to_canonical(), "#321339"); // Qt.lighter(#2a1030, 1.18)
    }
}
