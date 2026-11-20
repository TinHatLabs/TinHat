//! AmbienceController — chat backgrounds + particle effects' rules for QML (docs/AMBIENCE.md).
//!
//! Every rule lives in `tinhat_widgets::ambience` (the web's `src/features/chat/ambience/core.ts`, proven identical
//! by `testvectors/ambience/vectors.json`); this object only carries them across the QML boundary — the state parser
//! and writer, the scenes, the contrast scrim, when effects run, the Settings rows — plus the shared words
//! (`testvectors/ambience/words.json`, the same file the web checks its words against).

use cxx_qt_lib::QString;
use serde_json::{json, Value};
use tinhat_widgets::ambience::{self as amb, BackgroundKind, Effect, EffectsIn, EffectsState, LiveIn, Mode, WinState};

#[cxx_qt::bridge]
pub mod qobject {
    extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        type AmbienceController = super::AmbienceControllerRust;

        /// QML: parse(contentJson, sender) → {kind, preset, effect, image, by} as JSON ("" content = absent).
        #[qinvokable]
        fn parse(self: &AmbienceController, content: &QString, sender: &QString) -> QString;
        /// QML: content(kind, preset, effect, image) → the state content the picker writes (JSON).
        /// kind "none" | "preset" | "image"; effect "" = the preset's own.
        #[qinvokable]
        fn content(self: &AmbienceController, kind: &QString, preset: &QString, effect: &QString, image: &QString) -> QString;
        /// QML: presetsJson() → [{id, name, effect}] in the picker's order.
        #[qinvokable]
        fn presets_json(self: &AmbienceController) -> QString;
        /// QML: effectsJson() → [{id, name}] (rain, embers, snow, fireflies, none).
        #[qinvokable]
        fn effects_json(self: &AmbienceController) -> QString;
        /// QML: themeMode(timelineBg) → "light" | "dark" | "oled".
        #[qinvokable]
        fn theme_mode(self: &AmbienceController, bg: &QString) -> QString;
        /// QML: lookJson(preset, mode) → {stops: [[at, "#hex"]…], glow: {x, y, rx, ry, color, a}} ("" = unknown preset).
        #[qinvokable]
        fn look_json(self: &AmbienceController, preset: &QString, mode: &QString) -> QString;
        /// QML: scrimFor(preset, mode, timelineBg, fgsJson) → the theme veil (0..0.9) that keeps text ≥ 4.5:1.
        #[qinvokable]
        fn scrim_for(self: &AmbienceController, preset: &QString, mode: &QString, bg: &QString, fgs: &QString) -> f64;
        /// QML: scrimForImage(rgbaJson, w, h, timelineBg, fgsJson) → the veil a custom image needs: its pixels (an RGBA
        /// byte array as JSON, w × h) as an 8 × 8 grid of colour boxes (ambience::image_cells, a true bound).
        #[qinvokable]
        fn scrim_for_image(self: &AmbienceController, rgba: &QString, w: i32, h: i32, bg: &QString, fgs: &QString) -> f64;
        /// QML: scrimForCells(cellsJson, timelineBg, fgsJson) → the veil for colour cells [[[r,g,b],[r,g,b]]…] (0..1).
        #[qinvokable]
        fn scrim_for_cells(self: &AmbienceController, cells: &QString, bg: &QString, fgs: &QString) -> f64;
        /// QML: applyPlan(backgroundJson, kind, preset, effect, newImage, me) → "skip" | "blocked" | "write": what the
        /// sheet's Apply does (never republish someone else's image under my name; nothing changed = write nothing).
        #[qinvokable]
        fn apply_plan(self: &AmbienceController, background: &QString, kind: &QString, preset: &QString, effect: &QString, new_image: bool, me: &QString) -> QString;
        /// QML: scrimForUnknownImage(timelineBg, fgsJson) → the veil before an image is read: as if any colour.
        #[qinvokable]
        fn scrim_for_unknown_image(self: &AmbienceController, bg: &QString, fgs: &QString) -> f64;
        /// QML: liveWinState(hidden, focused) → the window state the effects follow.
        #[qinvokable]
        fn live_win_state(self: &AmbienceController, hidden: bool, focused: bool) -> QString;
        /// QML: effectsState(backgrounds, effects, pauseUnfocused, saver, reduced, win, gpu, effect, inCall) → "run" | "paused" | "off".
        #[qinvokable]
        fn effects_state(self: &AmbienceController, backgrounds: bool, effects: bool, pause_unfocused: bool, saver: bool, reduced: bool, win: &QString, gpu: bool, effect: &QString, in_call: bool) -> QString;
        /// QML: effectFps(effect) → the effect's frame rate (30 / 60).
        #[qinvokable]
        fn effect_fps(self: &AmbienceController, effect: &QString) -> i32;
        /// QML: particlesMenuJson(room, background, canChange, me, backgrounds, effects, pauseUnfocused, saver) → the ☰
        /// Particles rows [{id, label, checked (null = not checkable), enabled, sep}] (the shared particles_menu).
        #[qinvokable]
        fn particles_menu_json(self: &AmbienceController, room: bool, background: &QString, can_change: bool, me: &QString, backgrounds: bool, effects: bool, pause_unfocused: bool, saver: bool) -> QString;
        /// QML: particlesWriteJson(room, background, canChange, me, effect) → the state content a pick sends now, or ""
        /// = NOTHING is sent (the shared particles_write: no room, no permission, a greyed row, or no change).
        #[qinvokable]
        fn particles_write_json(self: &AmbienceController, room: bool, background: &QString, can_change: bool, me: &QString, effect: &QString) -> QString;
        /// QML: particlesPickJson(background, effect, me) → {plan: "skip" | "blocked" | "write", content} (what the Chat
        /// background sheet would write with only the effect changed: the shared particles_pick).
        #[qinvokable]
        fn particles_pick_json(self: &AmbienceController, background: &QString, effect: &QString, me: &QString) -> QString;
        /// QML: labAutoshare() → the measurement rigs asked for a screen share to start by itself (lab builds only;
        /// always false in a release).
        #[qinvokable]
        fn lab_autoshare(self: &AmbienceController) -> bool;
        /// QML: pauseRowChecked / pauseRowEnabled(pause, saver) — the Settings row (Save GPU on → on and greyed).
        #[qinvokable]
        fn pause_row_checked(self: &AmbienceController, pause_unfocused: bool, saver: bool) -> bool;
        #[qinvokable]
        fn pause_row_enabled(self: &AmbienceController, pause_unfocused: bool, saver: bool) -> bool;
        /// QML: prefOn(raw, dflt) — a stored switch ("1"/"0"; anything else = the default).
        #[qinvokable]
        fn pref_on(self: &AmbienceController, raw: &QString, dflt: bool) -> bool;
        /// QML: prefKey(name) — "backgrounds" | "effects" | "pause" → the ui pref's key.
        #[qinvokable]
        fn pref_key(self: &AmbienceController, name: &QString) -> QString;
        /// QML: word(key) — a visible string (testvectors/ambience/words.json).
        #[qinvokable]
        fn word(self: &AmbienceController, key: &QString) -> QString;
    }
}

#[derive(Default)]
pub struct AmbienceControllerRust;

const WORDS: &str = include_str!("../../../testvectors/ambience/words.json");

fn words() -> &'static Value {
    static W: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    W.get_or_init(|| serde_json::from_str::<Value>(WORDS).ok().and_then(|v| v.get("words").cloned()).unwrap_or(Value::Null))
}

fn strs(json: &QString) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(&json.to_string()).unwrap_or_default()
}

impl qobject::AmbienceController {
    fn parse(&self, content: &QString, sender: &QString) -> QString {
        let c = content.to_string();
        let v = if c.trim().is_empty() { Value::Null } else { serde_json::from_str::<Value>(&c).unwrap_or(Value::String(c)) };
        QString::from(&amb::parse_background(&v, &sender.to_string()).to_json().to_string())
    }
    fn content(&self, kind: &QString, preset: &QString, effect: &QString, image: &QString) -> QString {
        let k = BackgroundKind::parse(&kind.to_string()).unwrap_or(BackgroundKind::None);
        let p = preset.to_string();
        let e = Effect::parse(&effect.to_string());
        let i = image.to_string();
        let v = amb::background_content(k, (!p.is_empty()).then_some(p.as_str()), e, (!i.is_empty()).then_some(i.as_str()));
        QString::from(&v.to_string())
    }
    fn presets_json(&self) -> QString {
        let v: Vec<Value> = amb::PRESETS.iter().map(|p| json!({"id": p.id, "name": p.name, "effect": p.effect.as_str()})).collect();
        QString::from(&Value::Array(v).to_string())
    }
    fn effects_json(&self) -> QString {
        let v: Vec<Value> = amb::EFFECTS.iter().map(|e| json!({"id": e.as_str(), "name": e.name()})).collect();
        QString::from(&Value::Array(v).to_string())
    }
    fn theme_mode(&self, bg: &QString) -> QString {
        QString::from(amb::theme_mode(&bg.to_string()).as_str())
    }
    fn look_json(&self, preset: &QString, mode: &QString) -> QString {
        let (Some(p), Some(m)) = (amb::preset_by_id(&preset.to_string()), Mode::parse(&mode.to_string())) else { return QString::default() };
        let l = amb::look_for(p, m);
        let stops: Vec<Value> = l.stops.iter().map(|(at, c)| json!([at, c])).collect();
        let g = &l.glow;
        QString::from(&json!({"stops": stops, "glow": {"x": g.x, "y": g.y, "rx": g.rx, "ry": g.ry, "color": g.color, "a": g.a}}).to_string())
    }
    fn scrim_for(&self, preset: &QString, mode: &QString, bg: &QString, fgs: &QString) -> f64 {
        let (Some(p), Some(m)) = (amb::preset_by_id(&preset.to_string()), Mode::parse(&mode.to_string())) else { return 0.0 };
        let f: Vec<String> = strs(fgs).iter().map(|s| qhex(s)).collect();
        let fr: Vec<&str> = f.iter().map(String::as_str).collect();
        amb::scrim_for(amb::look_for(p, m), &qhex(&bg.to_string()), &fr)
    }
    fn scrim_for_image(&self, rgba: &QString, w: i32, h: i32, bg: &QString, fgs: &QString) -> f64 {
        let px: Vec<u8> = serde_json::from_str::<Vec<u8>>(&rgba.to_string()).unwrap_or_default();
        let (w, h) = (w.max(0) as usize, h.max(0) as usize);
        if px.len() < w * h * 4 || w == 0 || h == 0 {
            return self.scrim_for_unknown_image(bg, fgs);
        }
        let f: Vec<String> = strs(fgs).iter().map(|s| qhex(s)).collect();
        let fr: Vec<&str> = f.iter().map(String::as_str).collect();
        amb::scrim_for_cells(&amb::image_cells(&px, w, h, 8), &qhex(&bg.to_string()), &fr)
    }
    fn scrim_for_cells(&self, cells: &QString, bg: &QString, fgs: &QString) -> f64 {
        let parsed: Vec<[[f64; 3]; 2]> = serde_json::from_str(&cells.to_string()).unwrap_or_default();
        if parsed.is_empty() {
            return self.scrim_for_unknown_image(bg, fgs);
        }
        let f: Vec<String> = strs(fgs).iter().map(|s| qhex(s)).collect();
        let fr: Vec<&str> = f.iter().map(String::as_str).collect();
        amb::scrim_for_cells(&parsed, &qhex(&bg.to_string()), &fr)
    }
    fn apply_plan(&self, background: &QString, kind: &QString, preset: &QString, effect: &QString, new_image: bool, me: &QString) -> QString {
        let bg = bg_from_json(&background.to_string());
        let p = preset.to_string();
        let choice = amb::SheetChoice {
            kind: BackgroundKind::parse(&kind.to_string()).unwrap_or(BackgroundKind::None),
            preset: (!p.is_empty()).then_some(p),
            effect: Effect::parse(&effect.to_string()),
            new_image,
        };
        QString::from(amb::apply_plan(&bg, &choice, &me.to_string()))
    }
    fn scrim_for_unknown_image(&self, bg: &QString, fgs: &QString) -> f64 {
        let f: Vec<String> = strs(fgs).iter().map(|s| qhex(s)).collect();
        let fr: Vec<&str> = f.iter().map(String::as_str).collect();
        amb::scrim_for_cells(&[[[0.0, 0.0, 0.0], [1.0, 1.0, 1.0]]], &qhex(&bg.to_string()), &fr)
    }
    fn live_win_state(&self, hidden: bool, focused: bool) -> QString {
        let w = amb::live_win_state(&LiveIn { hidden, app_visible: None, covered: None, app_focused: None, phone_shell: false, has_focus: focused, native_call: false });
        QString::from(w.as_str())
    }
    #[allow(clippy::too_many_arguments)]
    fn effects_state(&self, backgrounds: bool, effects: bool, pause_unfocused: bool, saver: bool, reduced: bool, win: &QString, gpu: bool, effect: &QString, in_call: bool) -> QString {
        let win = WinState::parse(&win.to_string());
        let effect = Effect::parse(&effect.to_string()).unwrap_or(Effect::None);
        let shared = amb::effects_state(&EffectsIn { backgrounds, effects, pause_unfocused, saver, reduced, win, gpu, effect });
        QString::from(call_pause(shared, in_call).as_str())
    }
    fn effect_fps(&self, effect: &QString) -> i32 {
        Effect::parse(&effect.to_string()).map(|e| e.fps() as i32).unwrap_or(30).max(1)
    }
    #[allow(clippy::too_many_arguments)]
    fn particles_menu_json(&self, room: bool, background: &QString, can_change: bool, me: &QString, backgrounds: bool, effects: bool, pause_unfocused: bool, saver: bool) -> QString {
        let input = amb::ParticlesMenuIn { room, bg: bg_from_json(&background.to_string()), can_change, me: me.to_string(), backgrounds, effects, pause_unfocused, saver };
        QString::from(&particles_rows_json(&input).to_string())
    }
    fn particles_write_json(&self, room: bool, background: &QString, can_change: bool, me: &QString, effect: &QString) -> QString {
        QString::from(&particles_write_str(room, &background.to_string(), can_change, &me.to_string(), &effect.to_string()))
    }
    fn particles_pick_json(&self, background: &QString, effect: &QString, me: &QString) -> QString {
        let Some(e) = Effect::parse(&effect.to_string()) else { return QString::from(r#"{"plan":"skip","content":null}"#) };
        let r = amb::particles_pick(&bg_from_json(&background.to_string()), e, &me.to_string());
        QString::from(&json!({"plan": r.plan, "content": r.content}).to_string())
    }
    /// The rigs' screen-share autostart (headless weston + Xwayland has no pointer, so a synthetic click on "Start
    /// sharing" never lands). Compiled only outside a release (cfg tinhat_env_selftests, build.rs): a release build
    /// neither names the flag nor accepts it, so no shortcut or other program can start a share nobody chose, and
    /// scripts/release-binary-check.sh refuses a binary that names it.
    fn lab_autoshare(&self) -> bool {
        #[cfg(tinhat_env_selftests)]
        {
            // black_box keeps the flag's bytes in a lab binary (the optimiser would turn the compare into integer
            // compares and drop the text), so release-binary-check.sh can tell a lab binary that slipped into a release
            let flag: &str = std::hint::black_box("--tinhat-autoshare");
            std::env::args().any(|a| a == flag)
        }
        #[cfg(not(tinhat_env_selftests))]
        {
            false
        }
    }
    fn pause_row_checked(&self, pause_unfocused: bool, saver: bool) -> bool {
        amb::pause_row(pause_unfocused, saver).checked
    }
    fn pause_row_enabled(&self, pause_unfocused: bool, saver: bool) -> bool {
        amb::pause_row(pause_unfocused, saver).enabled
    }
    fn pref_on(&self, raw: &QString, dflt: bool) -> bool {
        let r = raw.to_string();
        amb::pref_on(Some(r.as_str()), dflt)
    }
    fn pref_key(&self, name: &QString) -> QString {
        QString::from(match name.to_string().as_str() {
            "backgrounds" => amb::PREF_BACKGROUNDS,
            "effects" => amb::PREF_EFFECTS,
            "pause" => amb::PREF_PAUSE_UNFOCUSED,
            _ => "",
        })
    }
    fn word(&self, key: &QString) -> QString {
        QString::from(words().get(key.to_string()).and_then(|v| v.as_str()).unwrap_or(""))
    }
}

/// A QML colour's text ("#aarrggbb" when translucent, "#rrggbb") as "#rrggbb" (the alpha is dropped: the theme
/// background is opaque; a translucent text colour is judged at full strength, the stricter reading).
fn qhex(s: &str) -> String {
    let t = s.trim();
    if t.len() == 9 && t.starts_with('#') { format!("#{}", &t[3..]) } else { t.to_string() }
}

/// Both apps (the web's callpause.ts, same rule, same cases): while you're in a call (or joining one) the particles
/// stop and are dropped. The call's video already keeps the window drawing, and particles on top of it cost an Intel
/// iGPU about 1.5 points of render time in a call with a screen share; even frozen ones were still drawn with every
/// video frame (+0.5). Off stays off; after the call the scene starts fresh.
pub fn call_pause(s: EffectsState, in_call: bool) -> EffectsState {
    if in_call {
        EffectsState::Off
    } else {
        s
    }
}

/// A parsed background as QML holds it (AmbienceController.parse's JSON: {kind, preset, effect, image, by}).
fn bg_from_json(raw: &str) -> amb::Background {
    let v: Value = serde_json::from_str(raw).unwrap_or(Value::Null);
    amb::Background {
        kind: v.get("kind").and_then(|k| k.as_str()).and_then(BackgroundKind::parse).unwrap_or(BackgroundKind::Unset),
        preset: v.get("preset").and_then(|p| p.as_str()).map(str::to_string),
        effect: v.get("effect").and_then(|e| e.as_str()).and_then(Effect::parse).unwrap_or(Effect::None),
        image: v.get("image").and_then(|i| i.as_str()).map(str::to_string),
        by: v.get("by").and_then(|b| b.as_str()).unwrap_or("").to_string(),
    }
}

/// A ☰ Particles row's visible words (the shared words; effects by their shared names) — the web's particlesLabel.
fn particles_label(id: &str) -> String {
    if let Some(e) = id.strip_prefix("effect:").and_then(Effect::parse) {
        return e.name().to_string();
    }
    let key = id.strip_prefix("note:").unwrap_or(match id {
        "background" => "menu",
        "effects" => "effects",
        "pause" => "pause",
        "settings" => "effectsSettings",
        _ => "",
    });
    words().get(key).and_then(|v| v.as_str()).unwrap_or("").to_string()
}

/// particlesWriteJson's answer: the content to send, or "" (nothing is sent). The personal prefs don't touch the effect
/// rows, so they're fixed here.
fn particles_write_str(room: bool, background: &str, can_change: bool, me: &str, effect: &str) -> String {
    let Some(e) = Effect::parse(effect) else { return String::new() };
    let input = amb::ParticlesMenuIn { room, bg: bg_from_json(background), can_change, me: me.to_string(), backgrounds: true, effects: true, pause_unfocused: true, saver: false };
    amb::particles_write(&input, e).map(|v| v.to_string()).unwrap_or_default()
}

fn particles_rows_json(input: &amb::ParticlesMenuIn) -> Value {
    Value::Array(
        amb::particles_menu(input)
            .iter()
            .map(|r| {
                let mut v = r.to_json();
                v["label"] = Value::from(particles_label(&r.id));
                v
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ambience_particles_only_in_the_main_view() {
        // 3-7: a thread panel (ThreadsPane's ConversationView: host null, not primary) runs no particle sim of its own;
        // only the room's main view does, as on the web (ConversationView.tsx: effects only when primary)
        let cv = include_str!("../qml/ConversationView.qml");
        let gate = cv.lines().find(|l| l.trim_start().starts_with("particles:")).expect("the AmbienceLayer particles gate");
        assert!(gate.contains("conv.primary"), "particles gate must need the main view: {gate}");
        assert!(gate.contains("conv.host === null") && gate.contains("!conv.hidden"), "{gate}");
        let threads = include_str!("../qml/ThreadsPane.qml");
        let at = threads.find("ConversationView {").expect("the thread pane's conversation");
        let block = &threads[at..at + threads[at..].find('}').unwrap()];
        assert!(!block.contains("primary: true"), "a thread panel is never the primary view");
    }
    #[test]
    fn surfaces_only_the_cards() {
        // 12.1.17 (the web's layer.wiring "surfaces"): every ambRects hands the particle layer the visible cards only,
        // each id ending in its shared kind (amb::SURFACE_KINDS); never the NEW line, a row or a container
        let mut seen = std::collections::BTreeSet::new();
        for (name, qml) in [
            ("ChatBubble.qml", include_str!("../qml/ChatBubble.qml")),
            ("ConversationView.qml", include_str!("../qml/ConversationView.qml")),
            ("AmbienceGallery.qml", include_str!("../qml/AmbienceGallery.qml")),
            ("GameArtGallery.qml", include_str!("../qml/GameArtGallery.qml")),
        ] {
            let at = qml.find("function ambRects(").unwrap_or_else(|| panic!("{name}: no ambRects"));
            // the function's body: up to the line that closes it at its own indent
            let indent = qml[..at].rsplit('\n').next().unwrap().len();
            let close = format!("\n{}}}", " ".repeat(indent));
            let body = &qml[at..at + qml[at..].find(&close).unwrap_or_else(|| panic!("{name}: ambRects not closed"))];
            assert!(!body.contains("NEW") && !body.to_lowercase().contains("newline") && !body.contains("unread"), "{name}: the NEW line is no surface");
            let mut kinds = 0;
            for part in body.split('"').skip(1).step_by(2) {
                if let Some(k) = part.strip_prefix(':') {
                    assert!(amb::SURFACE_KINDS.contains(&k), "{name}: collider kind \":{k}\" is not a card");
                    seen.insert(k.to_string());
                    kinds += 1;
                }
            }
            assert!(kinds > 0, "{name}: ambRects names no kind");
        }
        let all: Vec<String> = amb::SURFACE_KINDS.iter().map(|k| k.to_string()).collect();
        assert_eq!(seen.into_iter().collect::<Vec<_>>(), { let mut a = all; a.sort(); a });
    }
    #[test]
    fn image_pick_is_shared() {
        // 12.1.17 (the web's image_pick + BackgroundSheet): "Your own image…" offers the shared formats, HEIC / HEIF too
        // (12.1.16's filter hid them without a word), and a picture this device can't read says so (customFormat)
        let v: Value = serde_json::from_str(include_str!("../../../testvectors/ambience/vectors.json")).expect("vectors");
        let exts: Vec<String> = v["image_pick"]["exts"].as_array().expect("image_pick.exts").iter().map(|e| format!("*.{}", e.as_str().unwrap())).collect();
        assert!(exts.iter().any(|e| e == "*.heic") && exts.iter().any(|e| e == "*.heif"));
        let filter = format!("nameFilters: [\"Images ({})\"]", exts.join(" "));
        let sheet = include_str!("../qml/BackgroundSheet.qml");
        assert!(sheet.contains(&filter), "the file dialog filter must be {filter}");
        assert!(sheet.contains("Image.Error") && sheet.contains("dlg.w(\"customFormat\")"), "an unreadable picture must say so");
        // review round: a pick still being read never lands in a reopened sheet or over a scene picked meanwhile, and Apply
        // waits for it
        assert!(sheet.lines().any(|l| l.trim_start().starts_with("onClosed:") && l.contains("pickProbe.source = \"\"")));
        assert!(sheet.contains("onChosenChanged: if (chosen !== \"image\") { pickProbe.source = \"\""));
        assert!(sheet.contains("pickProbe.status !== Image.Loading"));
        assert!(words().get("customFormat").and_then(|v| v.as_str()).is_some_and(|s| s.contains("can't be used as a background")));
    }
    #[test]
    fn words_file_has_every_key() {
        for k in ["backgrounds", "effects", "pause", "pauseHelp", "pauseSaver", "menu", "title", "customPosted", "readOnly"] {
            assert!(words().get(k).and_then(|v| v.as_str()).is_some_and(|s| !s.is_empty()), "{k}");
        }
    }
    #[test]
    fn qml_colour_text() {
        assert_eq!(qhex("#ff1b1e20"), "#1b1e20");
        assert_eq!(qhex("#1b1e20"), "#1b1e20");
    }
    #[test]
    fn effects_pause_in_a_call() {
        assert_eq!(call_pause(EffectsState::Run, true), EffectsState::Off);
        assert_eq!(call_pause(EffectsState::Paused, true), EffectsState::Off);
        assert_eq!(call_pause(EffectsState::Off, true), EffectsState::Off);
        assert_eq!(call_pause(EffectsState::Run, false), EffectsState::Run);
        assert_eq!(call_pause(EffectsState::Paused, false), EffectsState::Paused);
        assert_eq!(call_pause(EffectsState::Off, false), EffectsState::Off);
    }
    #[test]
    fn autoshare_is_a_lab_only_switch() {
        // round 3: the rigs' screen-share autostart must never ride in a release. QML can't be compiled out, so no QML
        // file may name the flag; only lab_autoshare (cfg tinhat_env_selftests) reads it, and the release check refuses
        // a binary that names it.
        let flag = concat!("--tinhat-", "autoshare");
        for (name, qml) in [("Main.qml", include_str!("../qml/Main.qml")), ("AppCore.qml", include_str!("../qml/AppCore.qml"))] {
            assert!(!qml.contains(&flag[2..]), "{name} names the autoshare flag: only Rust under the lab cfg may");
        }
        assert!(include_str!("../../../scripts/release-binary-check.sh").contains(flag), "release-binary-check.sh must refuse it");
    }
    #[test]
    fn particles_labels() {
        // the same table as the web's particles.test.ts particles_labels
        for (id, want) in [
            ("effect:rain", "Rain"), ("effect:embers", "Embers"), ("effect:snow", "Snow"), ("effect:fireflies", "Fireflies"), ("effect:none", "None"),
            ("note:readOnly", "Only moderators can change this room's background."),
            ("note:otherImage", "This image was posted by someone else. To change the background, pick a scene or your own image."),
            ("background", "Chat background…"), ("effects", "Particle effects"),
            ("note:offHere", "Chat backgrounds are off on this device (Settings → Appearance)."),
            ("pause", "Pause effects when Tin Hat isn't focused"),
            ("note:pauseSaver", "Save GPU is on, so effects always pause when Tin Hat isn't focused."),
            ("settings", "Effects settings…"),
        ] {
            assert_eq!(particles_label(id), want, "{id}");
        }
    }
    #[test]
    fn particles_after_theme() {
        // round 4: the ☰ app menu has Particles ▸ right after Theme, with a row for everything particles_menu can show
        let main = include_str!("../qml/Main.qml");
        let theme = main.find("id: themeMenu").expect("the Theme submenu");
        let particles = main.find("ParticlesMenu {").expect("the Particles submenu");
        let gallery = main.find("text: \"Icon gallery\"").expect("the next row");
        assert!(theme < particles && particles < gallery, "Particles sits right after Theme");
        let block = &main[particles..gallery];
        assert!(block.contains("app: root") && block.contains("roomCtl: room") && block.contains("onOpenSheet: backgroundSheet.open()"));
        let menu = include_str!("../qml/ParticlesMenu.qml");
        assert!(menu.contains("title: LinkGate.plain(app ? String(app.ambience.word(\"particles\")) : \"\")"));
        for id in ["effect:rain", "effect:embers", "effect:snow", "effect:fireflies", "effect:none", "note:readOnly", "note:otherImage", "background", "effects", "note:offHere", "pause", "note:pauseSaver", "settings"] {
            assert!(menu.contains(&format!("{{ rid: \"{id}\" }}")), "row {id}");
        }
    }
    #[test]
    fn particles_game_art_none() {
        // None on game art with an effect keeps the art and drops the effect (the sheet's Match the game); the web's
        // particles_game_art_none
        let game = amb::parse_background(&json!({"v": 1, "kind": "game", "preset": "dusk", "effect": "snow"}), "@them:x");
        let r = amb::particles_pick(&game, Effect::None, "@me:x");
        assert_eq!((r.plan, r.content.clone()), ("write", Some(json!({"v": 1, "kind": "game"}))));
        // the art stays after None: what was written reads back as game art with no effect (launch-gate 18:27)
        let after = amb::parse_background(&r.content.unwrap(), "@me:x");
        assert_eq!((after.kind, amb::shown_effect(&after)), (BackgroundKind::Game, Effect::None));
        // the same permission as the sheet: without it, nothing is written
        let bgj = json!({"kind": "game", "preset": "dusk", "effect": "snow", "image": null, "by": "@them:x"}).to_string();
        assert_eq!(particles_write_str(true, &bgj, false, "@me:x", "none"), "");
        assert_eq!(amb::particles_pick(&game, Effect::Rain, "@me:x").content, Some(json!({"v": 1, "kind": "preset", "preset": "rain", "effect": "rain"})));
        let input = amb::ParticlesMenuIn { room: true, bg: game, can_change: true, me: "@me:x".into(), backgrounds: true, effects: true, pause_unfocused: true, saver: false };
        let rows = particles_rows_json(&input);
        assert!(rows.as_array().unwrap().iter().any(|r| r["id"] == "effect:none" && r["enabled"] == true));
    }
    #[test]
    fn particles_no_permission_no_write() {
        // without permission a pick sends NOTHING, and the sheet's words show (the web's particles_no_permission_no_write)
        let bgs = [
            json!({"kind": "preset", "preset": "rain", "effect": "rain", "image": null, "by": "@mod:x"}),
            json!({"kind": "unset", "preset": null, "effect": "none", "image": null, "by": ""}),
            json!({"kind": "none", "preset": null, "effect": "none", "image": null, "by": "@mod:x"}),
            json!({"kind": "game", "preset": "dusk", "effect": "snow", "image": null, "by": "@mod:x"}),
            json!({"kind": "image", "preset": "rain", "effect": "rain", "image": "$mine", "by": "@me:x"}),
        ];
        let mut writes = 0;
        for bg in &bgs {
            for e in amb::EFFECTS {
                if !particles_write_str(true, &bg.to_string(), false, "@me:x", e.as_str()).is_empty() {
                    writes += 1;
                }
                if !particles_write_str(false, &bg.to_string(), true, "@me:x", e.as_str()).is_empty() {
                    writes += 1;
                }
            }
            let input = amb::ParticlesMenuIn { room: true, bg: bg_from_json(&bg.to_string()), can_change: false, me: "@me:x".into(), backgrounds: true, effects: true, pause_unfocused: true, saver: false };
            let rows = particles_rows_json(&input);
            assert!(rows.as_array().unwrap().iter().any(|r| r["id"] == "note:readOnly" && r["label"] == "Only moderators can change this room's background."));
        }
        assert_eq!(writes, 0, "zero writes");
        // the control: WITH permission the same pick does write (what the sheet writes); the checked one sends nothing
        let rain = bgs[0].to_string();
        assert_eq!(particles_write_str(true, &rain, true, "@me:x", "snow"), json!({"v": 1, "kind": "preset", "preset": "rain", "effect": "snow"}).to_string());
        assert_eq!(particles_write_str(true, &rain, true, "@me:x", "rain"), "");
        // the menu's ONE background write, and only what particlesWriteJson allows
        let menu = include_str!("../qml/ParticlesMenu.qml");
        assert_eq!(menu.matches("setBackground(").count(), 1);
        assert!(menu.contains("var c = String(app.ambience.particlesWriteJson(roomOpen, bgJson, roomOpen && !!roomCtl && roomCtl.canSetBackground === true, me, id.slice(7)))"));
        assert!(menu.contains("if (c.length > 0) roomCtl.setBackground(c)"));
    }
    #[test]
    fn particles_permission() {
        let input = amb::ParticlesMenuIn {
            room: true,
            bg: amb::parse_background(&json!({"v": 1, "kind": "preset", "preset": "rain"}), "@mod:x"),
            can_change: false,
            me: "@me:x".into(),
            backgrounds: true,
            effects: true,
            pause_unfocused: true,
            saver: false,
        };
        let rows = particles_rows_json(&input);
        for r in rows.as_array().unwrap().iter().filter(|r| r["id"].as_str().unwrap().starts_with("effect:")) {
            assert_eq!(r["enabled"], false, "{}", r["id"]);
        }
        let note = rows.as_array().unwrap().iter().find(|r| r["id"] == "note:readOnly").expect("the sheet's words");
        assert_eq!(note["label"], "Only moderators can change this room's background.");
        assert!(rows.as_array().unwrap().iter().any(|r| r["id"] == "background" && r["enabled"] == true), "the sheet still opens");
    }
    #[test]
    fn particles_switches() {
        // the menu's two switches are the Performance section's own stored settings (the same prefKey, the same props)
        let menu = include_str!("../qml/ParticlesMenu.qml");
        let main = include_str!("../qml/Main.qml");
        let pane = include_str!("../qml/AppearancePane.qml");
        assert!(menu.contains("app.mChatEffects = !r.checked; app.settings.applyUiPref(app.ambience.prefKey(\"effects\"), r.checked ? \"0\" : \"1\")"));
        assert!(menu.contains("app.mChatEffectsPause = !r.checked; app.settings.applyUiPref(app.ambience.prefKey(\"pause\"), r.checked ? \"0\" : \"1\")"));
        assert!(pane.contains("pane.app.mChatEffects = checked; pane.app.settings.applyUiPref(pane.app.ambience.prefKey(\"effects\")"));
        assert!(pane.contains("pane.app.mChatEffectsPause = checked; pane.app.settings.applyUiPref(pane.app.ambience.prefKey(\"pause\")"));
        // the effect rows write what the sheet writes (particles_pick), Effects settings… = the Performance row
        assert!(main.contains("onOpenSettings: { settingsWindow.openAt(\"appearance\"); settingsWindow.jumpTo(\"appearance.gpu-saver\") }"));
        assert_eq!(crate::settings_search::pane_of("appearance.gpu-saver"), "appearance");
    }
}
