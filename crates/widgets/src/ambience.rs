//! Ambience — shared chat backgrounds + particle effects: the pure rules both apps share, a line-for-line port of
//! Tin Hat Web's `src/features/chat/ambience/core.ts` (same names in snake_case). Both replay
//! `testvectors/ambience/vectors.json` (a byte-identical copy in both repos); the tests below carry the same case names
//! as `core.test.ts`. Design + privacy: the web repo's `docs/AMBIENCE.md`.
//!
//! ```text
//!   state     the room state `dev.tinhat.chat.background` (key ""): {v: 1, preset, effect}; parse_background /
//!             background_content
//!   who       can_change_background: the room's power levels (events[type] ?? state_default ?? 50) — moderators;
//!             both people in a Tin Hat DM (trusted_private_chat makes both admins)
//!   settings  effects_state: run | paused (keep the particles, stop the clock) | off (drop them)
//!   looks     PRESETS × theme mode (light | dark | oled, theme_mode from the timeline background) + scrim_for (text
//!             on the background itself keeps ≥ 4.5:1)
//!   sim       AmbienceSim: one deterministic simulation (seeded RNG, + − × ÷ √ floor only — no sin / exp / pow — so
//!             both languages compute the same bits) and its mesh: triangles with premultiplied per-vertex colour,
//!             never inside a collider (a message bubble or a panel): nothing is ever drawn over or behind text.
//! ```
//!
//! Numbers are IEEE doubles evaluated in the order the reference writes them; the mesh buffer holds f32 like the web's
//! `Float32Array`. Pure: no Qt, no I/O.

use std::collections::{HashMap, HashSet};

use serde_json::{json, Map, Value};

use crate::num;

/// The window state the effects follow (the low-GPU contract's: "focused" | "unfocused" | "hidden").
pub use crate::lowgpu::WinState;

pub const BACKGROUND_STATE: &str = "dev.tinhat.chat.background";

// ---- prefs (device prefs, "1" / "0"; same keys as the web's) -------------------------------------------------------
pub const PREF_BACKGROUNDS: &str = "chat-backgrounds";
pub const PREF_EFFECTS: &str = "chat-effects";
pub const PREF_PAUSE_UNFOCUSED: &str = "chat-effects-pause-unfocused";

/// A stored switch: "1"/"true" on, "0"/"false" off, anything else (never touched, `None`) = the default.
pub fn pref_on(raw: Option<&str>, dflt: bool) -> bool {
    match raw {
        Some("1") | Some("true") => true,
        Some("0") | Some("false") => false,
        _ => dflt,
    }
}

pub const PREF_DEFAULTS: [(&str, bool); 3] =
    [(PREF_BACKGROUNDS, true), (PREF_EFFECTS, true), (PREF_PAUSE_UNFOCUSED, true)];

// ---- presets -------------------------------------------------------------------------------------------------------
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Effect {
    Rain,
    Embers,
    Snow,
    Fireflies,
    None,
}

impl Effect {
    pub fn as_str(self) -> &'static str {
        match self {
            Effect::Rain => "rain",
            Effect::Embers => "embers",
            Effect::Snow => "snow",
            Effect::Fireflies => "fireflies",
            Effect::None => "none",
        }
    }
    /// One of [`EFFECTS`]' names (exact, case matters); anything else = `None` (not an effect).
    pub fn parse(s: &str) -> Option<Effect> {
        match s {
            "rain" => Some(Effect::Rain),
            "embers" => Some(Effect::Embers),
            "snow" => Some(Effect::Snow),
            "fireflies" => Some(Effect::Fireflies),
            "none" => Some(Effect::None),
            _ => None,
        }
    }
    /// EFFECT_NAMES[effect]
    pub fn name(self) -> &'static str {
        match self {
            Effect::Rain => "Rain",
            Effect::Embers => "Embers",
            Effect::Snow => "Snow",
            Effect::Fireflies => "Fireflies",
            Effect::None => "None",
        }
    }
    /// EFFECT_FPS[effect]: frames per second the hosts draw at: 30 for all (rain draws streaks as long as a frame's fall).
    pub fn fps(self) -> u32 {
        match self {
            Effect::Rain => 30,
            Effect::Embers => 30,
            Effect::Snow => 30,
            Effect::Fireflies => 30,
            Effect::None => 0,
        }
    }
}

pub const EFFECTS: [Effect; 5] = [Effect::Rain, Effect::Embers, Effect::Snow, Effect::Fireflies, Effect::None];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mode {
    Light,
    Dark,
    Oled,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Light => "light",
            Mode::Dark => "dark",
            Mode::Oled => "oled",
        }
    }
    pub fn parse(s: &str) -> Option<Mode> {
        match s {
            "light" => Some(Mode::Light),
            "dark" => Some(Mode::Dark),
            "oled" => Some(Mode::Oled),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glow {
    /// centre as a fraction of the view (y may sit just outside: a glow from below / above)
    pub x: f64,
    pub y: f64,
    /// radii as fractions of the view's width / height
    pub rx: f64,
    pub ry: f64,
    pub color: &'static str,
    /// the glow's opacity at its centre (it fades linearly to 0 at the radii)
    pub a: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    /// a vertical gradient, top (0) to bottom (1)
    pub stops: &'static [(f64, &'static str)],
    pub glow: Glow,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Preset {
    pub id: &'static str,
    pub name: &'static str,
    pub effect: Effect,
    pub dark: Look,
    pub oled: Look,
    pub light: Look,
}

#[allow(clippy::too_many_arguments)]
const fn l(s: &'static [(f64, &'static str)], x: f64, y: f64, rx: f64, ry: f64, color: &'static str, a: f64) -> Look {
    Look { stops: s, glow: Glow { x, y, rx, ry, color, a } }
}

pub static PRESETS: [Preset; 6] = [
    Preset {
        id: "rain",
        name: "Rainy evening",
        effect: Effect::Rain,
        dark: l(&[(0.0, "#141b26"), (0.65, "#1a2230"), (1.0, "#232a35")], 0.35, 1.02, 0.7, 0.35, "#c08a4a", 0.18),
        oled: l(&[(0.0, "#000000"), (0.6, "#000000"), (1.0, "#05070b")], 0.35, 1.05, 0.6, 0.3, "#8a6234", 0.16),
        light: l(&[(0.0, "#f4f7fa"), (0.65, "#eef2f7"), (1.0, "#e8eef5")], 0.35, 1.02, 0.7, 0.35, "#f6dcb8", 0.4),
    },
    Preset {
        id: "hearth",
        name: "Hearth",
        effect: Effect::Embers,
        dark: l(&[(0.0, "#1a1210"), (0.6, "#221611"), (1.0, "#2e1a10")], 0.5, 1.08, 0.8, 0.4, "#d2601e", 0.3),
        oled: l(&[(0.0, "#000000"), (0.6, "#000000"), (1.0, "#0c0503")], 0.5, 1.08, 0.8, 0.4, "#b44a12", 0.22),
        light: l(&[(0.0, "#fbf8f5"), (0.6, "#faf4ee"), (1.0, "#f8eee6")], 0.5, 1.08, 0.8, 0.4, "#f8d6b8", 0.3),
    },
    Preset {
        id: "snow",
        name: "First snow",
        effect: Effect::Snow,
        dark: l(&[(0.0, "#18202a"), (0.6, "#1f2934"), (1.0, "#2a3440")], 0.6, -0.05, 0.8, 0.4, "#9fb6cc", 0.14),
        oled: l(&[(0.0, "#000000"), (0.6, "#030507"), (1.0, "#070a0e")], 0.6, -0.05, 0.8, 0.4, "#6a7f94", 0.12),
        light: l(&[(0.0, "#f6f8fb"), (0.6, "#f0f4f8"), (1.0, "#eaf0f6")], 0.6, -0.05, 0.8, 0.4, "#ffffff", 0.6),
    },
    Preset {
        id: "summer",
        name: "Summer night",
        effect: Effect::Fireflies,
        dark: l(&[(0.0, "#0f1a17"), (0.6, "#13221d"), (1.0, "#182a22")], 0.8, -0.05, 0.5, 0.35, "#a7c4a0", 0.1),
        oled: l(&[(0.0, "#000000"), (0.6, "#000000"), (1.0, "#03100a")], 0.8, -0.05, 0.5, 0.35, "#5c7a58", 0.1),
        light: l(&[(0.0, "#f4f8f3"), (0.6, "#eef5ee"), (1.0, "#e8f1e8")], 0.8, -0.05, 0.5, 0.35, "#fff8e0", 0.5),
    },
    Preset {
        id: "dusk",
        name: "Dusk",
        effect: Effect::None,
        dark: l(&[(0.0, "#1b1626"), (0.6, "#251a2c"), (1.0, "#3a2028")], 0.5, 1.05, 1.0, 0.35, "#d0704a", 0.22),
        oled: l(&[(0.0, "#000000"), (0.6, "#05030a"), (1.0, "#120608")], 0.5, 1.05, 1.0, 0.35, "#9a4a30", 0.16),
        light: l(&[(0.0, "#f7f4fa"), (0.6, "#f8f1f4"), (1.0, "#faede6")], 0.5, 1.05, 1.0, 0.35, "#f9d4bf", 0.4),
    },
    Preset {
        id: "deep",
        name: "Deep water",
        effect: Effect::None,
        dark: l(&[(0.0, "#0d1826"), (0.6, "#0f2030"), (1.0, "#10303a")], 0.3, 1.05, 0.8, 0.4, "#2a7a8a", 0.16),
        oled: l(&[(0.0, "#000000"), (0.6, "#00040a"), (1.0, "#001014")], 0.3, 1.05, 0.8, 0.4, "#13505a", 0.14),
        light: l(&[(0.0, "#f3f8fb"), (0.6, "#edf5f8"), (1.0, "#e5f1f4")], 0.3, 1.05, 0.8, 0.4, "#cfeaf0", 0.45),
    },
];

pub fn preset_by_id(id: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.id == id)
}

// ---- the state event -----------------------------------------------------------------------------------------------
/// What a room's background is:
///   unset   no state (absent, or emptied `{}`): nothing chosen — a later kind (game art) may show by itself
///   none    the room chose the plain theme: nothing shows, not even game art
///   preset  a built-in scene (no upload at all)
///   image   a member's own image: a normal (encrypted) m.image chat message, referenced by event id; `preset` is the
///           fallback for anyone who can't show it (no key, joined later, redacted, someone else's image)
///   game    the game being played (game art; this build only parses it): `preset` is the fallback
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BackgroundKind {
    Unset,
    None,
    Preset,
    Image,
    Game,
}

impl BackgroundKind {
    pub fn as_str(self) -> &'static str {
        match self {
            BackgroundKind::Unset => "unset",
            BackgroundKind::None => "none",
            BackgroundKind::Preset => "preset",
            BackgroundKind::Image => "image",
            BackgroundKind::Game => "game",
        }
    }
    pub fn parse(s: &str) -> Option<BackgroundKind> {
        match s {
            "unset" => Some(BackgroundKind::Unset),
            "none" => Some(BackgroundKind::None),
            "preset" => Some(BackgroundKind::Preset),
            "image" => Some(BackgroundKind::Image),
            "game" => Some(BackgroundKind::Game),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Background {
    pub kind: BackgroundKind,
    /// the scene to draw: the preset itself, or the fallback of image / game; None = the plain theme
    pub preset: Option<String>,
    /// the effect to run (None when there is no scene)
    pub effect: Effect,
    /// kind image: the image message's event id
    pub image: Option<String>,
    /// who set it (the state event's sender; image: must be the image's sender)
    pub by: String,
}

impl Background {
    /// The same JSON object the web holds: `{kind, preset, effect, image, by}` (null for None).
    pub fn to_json(&self) -> Value {
        json!({
            "kind": self.kind.as_str(),
            "preset": self.preset,
            "effect": self.effect.as_str(),
            "image": self.image,
            "by": self.by,
        })
    }
}

pub const NO_BACKGROUND: Background =
    Background { kind: BackgroundKind::Unset, preset: None, effect: Effect::None, image: None, by: String::new() };

fn plain(sender: &str) -> Background {
    Background { kind: BackgroundKind::None, preset: None, effect: Effect::None, image: None, by: sender.to_string() }
}

/// ECMAScript `\s` (WhiteSpace + LineTerminator; also what `String.prototype.trim` strips).
fn js_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n'
            | '\u{0B}'
            | '\u{0C}'
            | '\r'
            | ' '
            | '\u{A0}'
            | '\u{1680}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202F}'
            | '\u{205F}'
            | '\u{3000}'
            | '\u{FEFF}'
    ) || ('\u{2000}'..='\u{200A}').contains(&c)
}

/// `s.trim()` (JavaScript's whitespace set).
fn js_trim(s: &str) -> &str {
    s.trim_matches(js_space)
}

/// An event id as Matrix writes it ($ + 1..255 printable non-space characters; the count in UTF-16 units like the
/// web's regex).
pub fn is_event_id(v: &Value) -> bool {
    v.as_str().is_some_and(is_event_id_str)
}

/// [`is_event_id`] of a string.
pub fn is_event_id_str(s: &str) -> bool {
    let Some(rest) = s.strip_prefix('$') else { return false };
    let n = rest.encode_utf16().count();
    (1..=255).contains(&n) && !rest.chars().any(js_space)
}

/// The room's background from the state content (absent = `Value::Null`) and its sender. Never fails: anything unknown
/// degrades to what can still be shown (a kind from a newer app shows its preset; nothing valid = the plain theme).
pub fn parse_background(content: &Value, sender: &str) -> Background {
    if content.is_null() {
        return NO_BACKGROUND;
    }
    let Some(c) = content.as_object() else { return plain(sender) };
    if c.is_empty() {
        return NO_BACKGROUND;
    }
    if c.get("v").and_then(Value::as_f64) != Some(1.0) {
        return plain(sender);
    }
    let kind = c.get("kind");
    if kind.and_then(Value::as_str) == Some("none") {
        return plain(sender);
    }
    if kind.and_then(Value::as_str) == Some("game") {
        // "Match the game" is its own kind (3-1): the preset is only a fallback scene and may be absent; reading it as
        // plain made "None" a no-op while the game art kept showing
        let gp = c.get("preset").and_then(Value::as_str).and_then(preset_by_id);
        let effect = c.get("effect").and_then(Value::as_str).and_then(Effect::parse).unwrap_or(gp.map(|p| p.effect).unwrap_or(Effect::None));
        return Background { kind: BackgroundKind::Game, preset: gp.map(|p| p.id.to_string()), effect, image: None, by: sender.to_string() };
    }
    let p = c.get("preset").and_then(Value::as_str).and_then(preset_by_id);
    let Some(p) = p else { return plain(sender) };
    let effect = c.get("effect").and_then(Value::as_str).and_then(Effect::parse).unwrap_or(p.effect);
    // undefined (None) unless `image` is an object; its event_id may itself be absent (None) or any value
    let img = c.get("image").and_then(Value::as_object).and_then(|o| o.get("event_id"));
    let kind_s = kind.and_then(Value::as_str);
    if kind_s == Some("image") || (kind.is_none() && img.is_some()) {
        // a broken reference still shows the fallback scene
        return match img {
            Some(id) if is_event_id(id) => Background {
                kind: BackgroundKind::Image,
                preset: Some(p.id.to_string()),
                effect,
                image: id.as_str().map(str::to_string),
                by: sender.to_string(),
            },
            _ => Background {
                kind: BackgroundKind::Preset,
                preset: Some(p.id.to_string()),
                effect,
                image: None,
                by: sender.to_string(),
            },
        };
    }
    // "preset", absent, or a kind from a newer app: the preset
    Background {
        kind: BackgroundKind::Preset,
        preset: Some(p.id.to_string()),
        effect,
        image: None,
        by: sender.to_string(),
    }
}

/// What the picker writes: kind None = plain on purpose; a preset; or an image (with its preset fallback). (The picker
/// only writes None / Preset / Image; any other kind writes like Preset, as the web's would.)
pub fn background_content(
    kind: BackgroundKind,
    preset: Option<&str>,
    effect: Option<Effect>,
    image: Option<&str>,
) -> Value {
    if kind == BackgroundKind::None {
        return json!({"v": 1, "kind": "none"});
    }
    let p = match preset {
        Some(s) if !s.is_empty() => preset_by_id(s),
        _ => None,
    };
    let fallback = p.unwrap_or(&PRESETS[0]);
    let eff = effect.unwrap_or(fallback.effect);
    if kind == BackgroundKind::Image {
        let Some(image) = image.filter(|i| is_event_id_str(i)) else {
            return json!({"v": 1, "kind": "none"});
        };
        return json!({"v": 1, "kind": "image", "preset": fallback.id, "effect": eff.as_str(), "image": {"event_id": image}});
    }
    let Some(p) = p else { return json!({"v": 1, "kind": "none"}) };
    json!({"v": 1, "kind": "preset", "preset": p.id, "effect": eff.as_str()})
}

/// What the picker's Apply does with a choice: skip (nothing changed: write nothing), blocked (keeping an image that
/// someone ELSE posted: writing it under my name would make image_shown refuse it for everyone), or write.
#[derive(Clone, Debug, PartialEq)]
pub struct SheetChoice {
    pub kind: BackgroundKind,
    /// the scene, or the image's fallback scene
    pub preset: Option<String>,
    /// None = the scene's own
    pub effect: Option<Effect>,
    /// a newly picked image is posted first (then it is mine)
    pub new_image: bool,
}

pub fn apply_plan(bg: &Background, c: &SheetChoice, me: &str) -> &'static str {
    if !c.new_image {
        let p = c.preset.as_deref().and_then(preset_by_id);
        let eff = c.effect.unwrap_or(match p {
            Some(p) => p.effect,
            None => Effect::None,
        });
        if c.kind == BackgroundKind::None && bg.kind == BackgroundKind::None {
            return "skip";
        }
        if c.kind == BackgroundKind::Preset && bg.kind == BackgroundKind::Preset && bg.preset == c.preset && bg.effect == eff {
            return "skip";
        }
        if c.kind == BackgroundKind::Image && bg.kind == BackgroundKind::Image && bg.preset == c.preset && bg.effect == eff {
            return "skip";
        }
        if c.kind == BackgroundKind::Image && (bg.kind != BackgroundKind::Image || bg.image.is_none() || bg.by != me || me.is_empty()) {
            return "blocked";
        }
    }
    "write"
}

// ---- the ☰ Particles menu (round 4, P-A7): the same rows in both apps (web core.ts particlesMenu) --------------------
/// The effect a background shows (nothing chosen / plain = none).
pub fn shown_effect(bg: &Background) -> Effect {
    match bg.kind {
        BackgroundKind::Unset | BackgroundKind::None => Effect::None,
        _ => bg.effect,
    }
}

/// Picking an effect with no scene yet (nothing chosen, plain, or game art) sets the matching scene.
pub fn scene_for(e: Effect) -> Option<&'static str> {
    match e {
        Effect::Rain => Some("rain"),
        Effect::Embers => Some("hearth"),
        Effect::Snow => Some("snow"),
        Effect::Fireflies => Some("summer"),
        Effect::None => None,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParticlesPick {
    /// "skip" | "blocked" | "write" (apply_plan's words)
    pub plan: &'static str,
    /// the state content to write when plan = "write"
    pub content: Option<Value>,
}

/// What picking an effect in the ☰ Particles menu does: the Chat background sheet's own write (apply_plan +
/// background_content) with only the effect changed. A scene's own effect is written as None ("the background's own").
pub fn particles_pick(bg: &Background, e: Effect, me: &str) -> ParticlesPick {
    let skip = ParticlesPick { plan: "skip", content: None };
    if shown_effect(bg) == e {
        return skip;
    }
    if matches!(bg.kind, BackgroundKind::Preset | BackgroundKind::Image) {
        let own = bg.preset.as_deref().and_then(preset_by_id).map(|p| p.effect);
        let effect = if Some(e) == own { None } else { Some(e) };
        let c = SheetChoice { kind: bg.kind, preset: bg.preset.clone(), effect, new_image: false };
        let plan = apply_plan(bg, &c, me);
        let content = (plan == "write").then(|| background_content(bg.kind, bg.preset.as_deref(), effect, bg.image.as_deref()));
        return ParticlesPick { plan, content };
    }
    // game art with an effect: "None" keeps the art and drops the effect = the sheet's own "Match the game" write;
    // nothing chosen, plain or game art: the matching scene
    match scene_for(e) {
        None if bg.kind == BackgroundKind::Game => ParticlesPick { plan: "write", content: Some(json!({"v": 1, "kind": "game"})) },
        None => skip,
        Some(id) => ParticlesPick { plan: "write", content: Some(background_content(BackgroundKind::Preset, Some(id), None, None)) },
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParticlesMenuIn {
    /// a room is open (the home view: only the personal rows)
    pub room: bool,
    pub bg: Background,
    /// can_change_background for this room and person
    pub can_change: bool,
    pub me: String,
    /// Settings → Appearance → Performance, the same stored prefs
    pub backgrounds: bool,
    pub effects: bool,
    pub pause_unfocused: bool,
    pub saver: bool,
}

/// A row: id = effect:<e> | background | effects | pause | settings | note:<word key> (a disabled line); checked None =
/// not a checkable row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParticlesRow {
    pub id: String,
    pub checked: Option<bool>,
    pub enabled: bool,
    pub sep: bool,
}

impl ParticlesRow {
    pub fn to_json(&self) -> Value {
        json!({"id": self.id, "checked": self.checked, "enabled": self.enabled, "sep": self.sep})
    }
}

pub fn particles_menu(p: &ParticlesMenuIn) -> Vec<ParticlesRow> {
    let row = |id: &str, checked: Option<bool>, enabled: bool, sep: bool| ParticlesRow { id: id.to_string(), checked, enabled, sep };
    let mut rows = Vec::new();
    if p.room {
        let shown = shown_effect(&p.bg);
        let mut other = false;
        for e in EFFECTS {
            let plan = particles_pick(&p.bg, e, &p.me).plan;
            if plan == "blocked" {
                other = true;
            }
            let enabled = p.can_change && plan != "blocked" && !(plan == "skip" && shown != e);
            rows.push(row(&format!("effect:{}", e.as_str()), Some(shown == e), enabled, false));
        }
        if !p.can_change {
            rows.push(row("note:readOnly", None, false, false));
        } else if other {
            rows.push(row("note:otherImage", None, false, false));
        }
        rows.push(row("background", None, true, false));
    }
    rows.push(row("effects", Some(p.backgrounds && p.effects), p.backgrounds, p.room));
    if !p.backgrounds {
        rows.push(row("note:offHere", None, false, false));
    }
    let pr = pause_row(p.pause_unfocused, p.saver);
    rows.push(row("pause", Some(pr.checked), pr.enabled, false));
    if !pr.enabled {
        rows.push(row("note:pauseSaver", None, false, false));
    }
    rows.push(row("settings", None, true, false));
    rows
}

/// What a ☰ Particles effect pick writes RIGHT NOW (both apps run every pick through this): None = NOTHING is sent (no
/// room, not allowed to change the background, the row is greyed, or nothing would change). The web's particlesWrite.
pub fn particles_write(p: &ParticlesMenuIn, e: Effect) -> Option<Value> {
    let id = format!("effect:{}", e.as_str());
    let enabled = particles_menu(p).iter().any(|r| r.id == id && r.enabled);
    if !p.room || !p.can_change || !enabled {
        return None;
    }
    let pick = particles_pick(&p.bg, e, &p.me);
    if pick.plan == "write" {
        pick.content
    } else {
        None
    }
}

/// What an image background's message turned out to be, as the viewer's app sees it (`None` at the call site = not
/// fetchable here: history visibility, gone). `decrypted` = the image itself decrypted on this device (false: no key
/// yet).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageEvent {
    pub r#type: String,
    pub sender: String,
    pub room: String,
    pub msgtype: String,
    pub redacted: bool,
    pub decrypted: bool,
}

/// Show the custom image? Only the setter's OWN image message in THIS room, not redacted, that this device could
/// decrypt. Anything else shows the preset fallback — never a broken tile.
pub fn image_shown(bg: &Background, room: &str, ev: Option<&ImageEvent>) -> bool {
    let has_image = bg.image.as_deref().is_some_and(|i| !i.is_empty());
    let Some(ev) = ev else { return false };
    if bg.kind != BackgroundKind::Image || !has_image {
        return false;
    }
    ev.r#type == "m.room.message"
        && ev.msgtype == "m.image"
        && ev.room == room
        && !ev.redacted
        && ev.decrypted
        && ev.sender == bg.by
        && !bg.by.is_empty()
}

/// Custom image limits: re-encoded as JPEG (no metadata can survive), ≤ 2560 px on the long side, ≤ 1.5 MB.
pub const IMAGE_MAX_SIDE: f64 = 2560.0;
pub const IMAGE_MAX_BYTES: u64 = 1_500_000;
/// JPEG qualities tried in turn; after the last, the image is scaled by IMAGE_SHRINK and the ladder starts again.
pub const IMAGE_QUALITIES: [f64; 3] = [0.85, 0.75, 0.65];
pub const IMAGE_SHRINK: f64 = 0.8;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImageSize {
    pub w: f64,
    pub h: f64,
}

/// The size an image is first re-encoded at (the long side capped at `max_side` — [`IMAGE_MAX_SIDE`] — aspect kept,
/// whole pixels, ≥ 1).
#[allow(clippy::neg_cmp_op_on_partial_ord)] // !(w > 0): NaN too, like the reference
pub fn fit_image(w: f64, h: f64, max_side: f64) -> ImageSize {
    if !(w > 0.0) || !(h > 0.0) {
        return ImageSize { w: 0.0, h: 0.0 };
    }
    let k = mmin(1.0, max_side / mmax(w, h));
    ImageSize { w: mmax(1.0, js_round(w * k)), h: mmax(1.0, js_round(h * k)) }
}

// ---- who can change it ---------------------------------------------------------------------------------------------
fn pl_num(v: Option<&Value>, dflt: f64) -> f64 {
    match v {
        Some(Value::Number(n)) => match n.as_f64() {
            Some(x) if x.is_finite() => x.trunc(),
            _ => dflt,
        },
        Some(Value::String(s)) => {
            let t = js_trim(s);
            let digits = t.strip_prefix('-').unwrap_or(t);
            if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
                t.parse::<f64>().unwrap_or(dflt)
            } else {
                dflt
            }
        }
        _ => dflt,
    }
}

/// `v[key]` when `v` is a JSON object (anything else has no such key).
fn field<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    v.as_object().and_then(|o| o.get(key))
}

/// The power level a member needs to set the background: events[type] ?? state_default ?? 50.
pub fn background_level(pl: &Value) -> f64 {
    let ev = field(pl, "events");
    pl_num(ev.and_then(|e| field(e, BACKGROUND_STATE)), pl_num(field(pl, "state_default"), 50.0))
}

/// Can `user` change the room's background? `creator` = the user is a room-v12 creator (unlimited power). No power
/// levels event at all (`Value::Null`) → only a creator (Matrix: the creator has 100 then; the apps pass creator = true
/// for them).
pub fn can_change_background(pl: &Value, user: &str, creator: bool) -> bool {
    if creator {
        return true;
    }
    if !(pl.is_object() || pl.is_array()) {
        return false;
    }
    let users = field(pl, "users");
    let mine = pl_num(users.and_then(|u| field(u, user)), pl_num(field(pl, "users_default"), 0.0));
    mine >= background_level(pl)
}

// ---- when effects run ----------------------------------------------------------------------------------------------
/// What the page / window can tell about itself. Web: `document.hidden` + the Tin Hat phone apps' bridge. Desktop: the
/// window's visibility + active state (the shell fields stay None / false).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LiveIn {
    pub hidden: bool,
    /// Android appState().visible (None = no such bridge)
    pub app_visible: Option<bool>,
    /// iOS covered() (None = no such bridge)
    pub covered: Option<bool>,
    /// Android appState().focused (None = no such bridge)
    pub app_focused: Option<bool>,
    /// the page runs in a Tin Hat PHONE app (iOS or Android): its web view's hasFocus() can be false while it is plainly
    /// in front (iOS until the first tap; older Android apps), so without a native focus report visible = focused (3-4)
    pub phone_shell: bool,
    /// document.hasFocus() / the window is active
    pub has_focus: bool,
    /// the phone app's NATIVE call is on (web native/callState); false on the desktop
    pub native_call: bool,
}

/// The REAL window state the effects follow (one rule for both apps and every shell).
pub fn live_win_state(p: &LiveIn) -> WinState {
    if p.hidden || p.app_visible == Some(false) || p.covered == Some(true) {
        return WinState::Hidden;
    }
    // 3-3: an app that reports neither covered() nor appState() (today's iOS app) can't say when its native call screen
    // hides the page, so a native call counts as covering it for as long as the call is on
    if p.covered.is_none() && p.app_visible.is_none() && p.native_call {
        return WinState::Hidden;
    }
    let focused = match p.app_focused {
        Some(f) => f,
        None => {
            if p.phone_shell {
                true
            } else {
                p.has_focus
            }
        }
    };
    if focused {
        WinState::Focused
    } else {
        WinState::Unfocused
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EffectsIn {
    /// Show chat backgrounds
    pub backgrounds: bool,
    /// Particle effects (the master switch)
    pub effects: bool,
    /// Pause effects when Tin Hat isn't focused (default ON)
    pub pause_unfocused: bool,
    /// Save GPU while Tin Hat isn't focused: effects ALWAYS pause while unfocused, whatever pause_unfocused says
    pub saver: bool,
    /// Reduce motion (the app's pref OR the OS setting): effects off
    pub reduced: bool,
    /// live_win_state
    pub win: WinState,
    /// a GPU renderer draws the app (desktop: not Qt's software renderer; web: WebGL2 is there)
    pub gpu: bool,
    /// the room background's effect
    pub effect: Effect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EffectsState {
    Run,
    Paused,
    Off,
}

impl EffectsState {
    pub fn as_str(self) -> &'static str {
        match self {
            EffectsState::Run => "run",
            EffectsState::Paused => "paused",
            EffectsState::Off => "off",
        }
    }
    pub fn parse(s: &str) -> Option<EffectsState> {
        match s {
            "run" => Some(EffectsState::Run),
            "paused" => Some(EffectsState::Paused),
            "off" => Some(EffectsState::Off),
            _ => None,
        }
    }
}

/// Off drops the particles; Paused keeps them and stops the clock (nothing is drawn again until it runs); Run steps
/// and draws. Hidden / minimised ALWAYS pauses (nobody sees it).
pub fn effects_state(p: &EffectsIn) -> EffectsState {
    if !p.backgrounds || !p.effects || p.reduced || !p.gpu || p.effect == Effect::None {
        return EffectsState::Off;
    }
    if p.win == WinState::Hidden {
        return EffectsState::Paused;
    }
    if p.win == WinState::Unfocused && (p.pause_unfocused || p.saver) {
        return EffectsState::Paused;
    }
    EffectsState::Run
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PauseRow {
    pub checked: bool,
    pub enabled: bool,
}

/// The "Pause effects when Tin Hat isn't focused" row as Settings shows it: Save GPU on → shown on and greyed.
pub fn pause_row(pause_unfocused: bool, saver: bool) -> PauseRow {
    if saver {
        PauseRow { checked: true, enabled: false }
    } else {
        PauseRow { checked: pause_unfocused, enabled: true }
    }
}

// ---- colour maths --------------------------------------------------------------------------------------------------
pub type Rgb = [f64; 3];

/// "#rrggbb" (or "#rgb", or "#aarrggbb" — alpha ignored) → 0..1 channels; anything else → None.
pub fn parse_hex(s: &str) -> Option<Rgb> {
    let t = js_trim(s);
    let m = t.strip_prefix('#')?;
    if !matches!(m.len(), 3 | 6 | 8) || !m.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut h: String = m.to_string();
    if h.len() == 3 {
        let b = h.as_bytes();
        h = [b[0], b[0], b[1], b[1], b[2], b[2]].iter().map(|&c| c as char).collect();
    }
    if h.len() == 8 {
        h = h[2..].to_string();
    }
    let ch = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).map(|v| v as f64 / 255.0).ok();
    Some([ch(0)?, ch(2)?, ch(4)?])
}

fn lin(c: f64) -> f64 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// WCAG relative luminance.
pub fn luminance(c: Rgb) -> f64 {
    0.2126 * lin(c[0]) + 0.7152 * lin(c[1]) + 0.0722 * lin(c[2])
}

/// WCAG contrast ratio (1..21).
pub fn contrast(a: Rgb, b: Rgb) -> f64 {
    let la = luminance(a);
    let lb = luminance(b);
    (mmax(la, lb) + 0.05) / (mmin(la, lb) + 0.05)
}

pub fn mix(a: Rgb, b: Rgb, t: f64) -> Rgb {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

/// The theme's mode from its timeline background: near-black = oled, dark, else light.
pub fn theme_mode(timeline_bg: &str) -> Mode {
    let Some(c) = parse_hex(timeline_bg) else { return Mode::Dark };
    let l = luminance(c);
    if l <= 0.0025 {
        return Mode::Oled;
    }
    if l < 0.3 {
        Mode::Dark
    } else {
        Mode::Light
    }
}

pub fn look_for(preset: &Preset, mode: Mode) -> &Look {
    match mode {
        Mode::Light => &preset.light,
        Mode::Oled => &preset.oled,
        Mode::Dark => &preset.dark,
    }
}

pub const MIN_CONTRAST: f64 = 4.5;
pub const SCRIM_STEP: f64 = 0.05;
pub const SCRIM_MAX: f64 = 1.0;
/// Text that is already below 4.5:1 on the theme's own background (some people's name colours) may lose at most this
/// much on a chat background.
pub const SUB_AA_SLACK: f64 = 0.1;

/// A box of colours: per channel the lowest and the highest value of a region (a stretch of the gradient, the glow, a
/// patch of an image). Blending toward the theme background is increasing in every channel and so is luminance, so the
/// region's blended luminances fill exactly [lum(blend(lo)), lum(blend(hi))] — a true bound, not a sample.
pub type Cell = [Rgb; 2];

fn cell_of(cols: &[Rgb]) -> Cell {
    let mut lo = [1.0, 1.0, 1.0];
    let mut hi = [0.0, 0.0, 0.0];
    for c in cols {
        for k in 0..3 {
            if c[k] < lo[k] {
                lo[k] = c[k];
            }
            if c[k] > hi[k] {
                hi[k] = c[k];
            }
        }
    }
    [lo, hi]
}

/// The cells of a look: each stretch between two stops (plus the glow over it, where the glow reaches).
pub fn look_cells(look: &Look) -> Vec<Cell> {
    let g = parse_hex(look.glow.color);
    let mut out = Vec::new();
    let s = look.stops;
    let mut i = 0;
    while i + 1 < s.len() {
        let (a, b) = (parse_hex(s[i].1), parse_hex(s[i + 1].1));
        if let (Some(a), Some(b)) = (a, b) {
            let mut cols: Vec<Rgb> = vec![a, b];
            let reach = look.glow.ry;
            let (y0, y1) = (s[i].0, s[i + 1].0);
            if let Some(g) = g {
                if look.glow.a > 0.0 && y1 >= look.glow.y - reach && y0 <= look.glow.y + reach {
                    cols.push(mix(a, g, look.glow.a));
                    cols.push(mix(b, g, look.glow.a));
                }
            }
            out.push(cell_of(&cols));
        }
        i += 1;
    }
    out
}

/// What each text colour must keep: 4.5:1 when it reaches that on the theme's background, else its own level minus
/// SUB_AA_SLACK.
pub fn contrast_target(fg: Rgb, bg: Rgb) -> f64 {
    let c = contrast(fg, bg);
    if c >= MIN_CONTRAST {
        MIN_CONTRAST
    } else {
        c - SUB_AA_SLACK
    }
}

/// The worst contrast a text colour can meet over a cell veiled with `a` of the theme background.
pub fn cell_worst(fg: Rgb, cell: &Cell, bg: Rgb, a: f64) -> f64 {
    let m0 = mix(cell[0], bg, a);
    let m1 = mix(cell[1], bg, a);
    let lt = luminance(fg);
    if lt >= luminance(m0) && lt <= luminance(m1) {
        return 1.0;
    }
    mmin(contrast(fg, m0), contrast(fg, m1))
}

/// The theme-coloured veil laid over a background so that every text colour drawn straight on it (`fgs`: the
/// timeline's own text and the name colours in view) keeps its target (contrast_target) over every cell: the smallest
/// multiple of 0.05 that does. 1 (the plain theme) always does, so the promise has no exceptions.
pub fn scrim_for_cells(cells: &[Cell], timeline_bg: &str, fgs: &[&str]) -> f64 {
    let Some(bg) = parse_hex(timeline_bg) else { return 0.0 };
    let mut texts: Vec<(Rgb, f64)> = Vec::new();
    for f in fgs {
        if let Some(c) = parse_hex(f) {
            texts.push((c, contrast_target(c, bg)));
        }
    }
    let mut k = 0.0_f64;
    while k * SCRIM_STEP <= SCRIM_MAX + 1e-9 {
        let a = js_round(k * SCRIM_STEP * 100.0) / 100.0;
        let mut ok = true;
        for cell in cells {
            for &(t, want) in &texts {
                if cell_worst(t, cell, bg, a) < want {
                    ok = false;
                }
            }
        }
        if ok {
            return a;
        }
        k += 1.0;
    }
    SCRIM_MAX
}

pub fn scrim_for(look: &Look, timeline_bg: &str, fgs: &[&str]) -> f64 {
    scrim_for_cells(&look_cells(look), timeline_bg, fgs)
}

/// An image's cells from its pixels (RGBA bytes, `w` × `h`): a `grid` × `grid` split, each cell the box of its pixels.
pub fn image_cells(px: &[u8], w: usize, h: usize, grid: usize) -> Vec<Cell> {
    let mut out = Vec::new();
    for gy in 0..grid {
        for gx in 0..grid {
            let x0 = (gx * w) / grid;
            let x1 = ((gx + 1) * w / grid).max(x0 + 1);
            let y0 = (gy * h) / grid;
            let y1 = ((gy + 1) * h / grid).max(y0 + 1);
            let mut lo = [1.0, 1.0, 1.0];
            let mut hi = [0.0, 0.0, 0.0];
            let mut y = y0;
            while y < y1 && y < h {
                let mut x = x0;
                while x < x1 && x < w {
                    let o = (y * w + x) * 4;
                    for k in 0..3 {
                        let v = px[o + k] as f64 / 255.0;
                        if v < lo[k] {
                            lo[k] = v;
                        }
                        if v > hi[k] {
                            hi[k] = v;
                        }
                    }
                    x += 1;
                }
                y += 1;
            }
            if lo[0] <= hi[0] {
                out.push([lo, hi]);
            }
        }
    }
    out
}

// ---- deterministic maths -------------------------------------------------------------------------------------------
/// `Math.round(x)`: the nearest integer, halves toward +∞ (exact, also where `floor(x + 0.5)` would round up).
fn js_round(x: f64) -> f64 {
    let f = x.floor();
    if x - f >= 0.5 {
        f + 1.0
    } else {
        f
    }
}

/// `Math.min(a, b)`, signed zeros included (−0 < +0).
fn mmin(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else if a < b || (a == b && a.is_sign_negative()) {
        a
    } else {
        b
    }
}

/// `Math.max(a, b)`, signed zeros included (+0 > −0).
fn mmax(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else if a > b || (a == b && a.is_sign_positive()) {
        a
    } else {
        b
    }
}

/// mulberry32 — u32 state, the same bits in both languages.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rng {
    pub s: u32,
}

impl Rng {
    pub fn new(seed: u32) -> Rng {
        Rng { s: seed }
    }
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> f64 {
        self.s = self.s.wrapping_add(0x6d2b79f5);
        let mut t = self.s;
        t = (t ^ (t >> 15)).wrapping_mul(t | 1);
        t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
        (t ^ (t >> 14)) as f64 / 4294967296.0
    }
    pub fn range(&mut self, a: f64, b: f64) -> f64 {
        a + (b - a) * self.next()
    }
}

// = 3.141592653589793 and 6.283185307179586, the reference's literals (same doubles)
const PI: f64 = std::f64::consts::PI;
const TAU: f64 = std::f64::consts::TAU;

/// sin without the platform's sin: range-reduced to [-π, π], folded to [-π/2, π/2], an odd polynomial to x^11
/// (|err| < 6e-8).
pub fn fsin(x: f64) -> f64 {
    let mut r = x - TAU * ((x + PI) / TAU).floor();
    if r > PI / 2.0 {
        r = PI - r;
    } else if r < -PI / 2.0 {
        r = -PI - r;
    }
    let r2 = r * r;
    r * (1.0
        + r2 * (-1.0 / 6.0
            + r2 * (1.0 / 120.0 + r2 * (-1.0 / 5040.0 + r2 * (1.0 / 362880.0 + r2 * (-1.0 / 39916800.0))))))
}

fn clamp(x: f64, a: f64, b: f64) -> f64 {
    if x < a {
        a
    } else if x > b {
        b
    } else {
        x
    }
}

// ---- the simulation ------------------------------------------------------------------------------------------------
#[derive(Clone, Debug, PartialEq)]
pub struct Collider {
    /// stable id (the message's event id, "composer", …): pools / snow lines follow it while it moves
    pub id: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    /// the top-left / top-right corner radii (a bubble's rounded top): pools and snow follow the curve there
    pub rl: Option<f64>,
    pub rr: Option<f64>,
}

/// The y of a collider's top edge at x (its rounded corners curve down; square = c.y).
pub fn top_at(c: &Collider, x: f64) -> f64 {
    let rl = mmin(mmin(c.rl.unwrap_or(0.0), c.w / 2.0), c.h);
    let rr = mmin(mmin(c.rr.unwrap_or(0.0), c.w / 2.0), c.h);
    if rl > 0.0 && x < c.x + rl {
        let d = mmin(rl, c.x + rl - x);
        return c.y + rl - mmax(0.0, rl * rl - d * d).sqrt();
    }
    if rr > 0.0 && x > c.x + c.w - rr {
        let d = mmin(rr, x - (c.x + c.w - rr));
        return c.y + rr - mmax(0.0, rr * rr - d * d).sqrt();
    }
    c.y
}

/// Is (x, y) inside the collider's shape (the rect minus the air of its rounded top corners)?
pub fn inside_shape(c: &Collider, x: f64, y: f64) -> bool {
    x > c.x && x < c.x + c.w && y < c.y + c.h && y > top_at(c, x)
}

/// A moving piece: k = 0 rain, 1 splash, 2 drip (falling), 3 runner (down a side), 4 ember, 5 spark, 6 flake,
/// 7 firefly.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Particle {
    pub k: u32,
    pub x: f64,
    pub y: f64,
    pub vx: f64,
    pub vy: f64,
    pub life: f64,
    pub max: f64,
    pub size: f64,
    pub seed: f64,
    /// runner: the collider id it runs down; ember: the collider it slides under ("" = none)
    pub on: String,
    /// runner / sliding ember: -1 left side, +1 right side
    pub dir: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Ledge {
    /// column heights in px (rain: water depth; snow: snow depth)
    pub h: Vec<f64>,
    /// water waiting at the left / right end (rain)
    pub end_l: f64,
    pub end_r: f64,
}

/// Fixed simulation step (s); a frame runs as many steps as its time covers (≤ MAX_STEPS).
pub const STEP: f64 = 1.0 / 60.0;
pub const MAX_STEPS: u32 = 6;
/// Pool / snow-line column width.
pub const COL_W: f64 = 6.0;
/// The deepest a pool or snow line gets (px): small, below any text.
pub const LEDGE_MAX: f64 = 3.0;
pub const PARTICLE_CAP: usize = 400;

/// Spawn rates — SUBTLE: rain drops/s per px of width (+200), embers and snowflakes per px of width, fireflies per
/// million px² of view.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rate {
    pub rain: f64,
    pub embers: f64,
    pub snow: f64,
    pub fireflies: f64,
}
pub const RATE: Rate = Rate { rain: 0.02, embers: 0.006, snow: 0.003, fireflies: 8.0 };
/// Seconds simulated at once when an effect starts (AmbienceSim::warm).
pub const WARM_S: f64 = 6.0;
/// Rain: what one drop adds to a pool (px of depth over one column), how fast a pool runs to its ends (px/s), and how
/// much gathers at an end before it lets go down the side (px²).
pub const RAIN_MASS: f64 = 0.6;
pub const POOL_SPEED: f64 = 14.0;
/// Snow: what one flake (per px of its size) adds to the line it lands on.
pub const SNOW_MASS: f64 = 0.6;
pub const RUNNER_MASS: f64 = 60.0;

#[derive(Clone, Debug)]
pub struct AmbienceSim {
    pub effect: Effect,
    pub density: f64,
    pub rng: Rng,
    pub w: f64,
    pub h: f64,
    pub t: f64,
    pub acc: f64,
    pub parts: Vec<Particle>,
    /// keyed by collider id; read in collider order only
    pub ledges: HashMap<String, Ledge>,
    pub cols: Vec<Collider>,
    pub wind: f64,
    spawn_acc: f64,
}

impl AmbienceSim {
    pub fn new(effect: Effect, seed: u32, density: f64) -> AmbienceSim {
        AmbienceSim {
            effect,
            density,
            rng: Rng::new(seed),
            w: 0.0,
            h: 0.0,
            t: 0.0,
            acc: 0.0,
            parts: Vec::new(),
            ledges: HashMap::new(),
            cols: Vec::new(),
            wind: 0.0,
            spawn_acc: 0.0,
        }
    }

    pub fn set_view(&mut self, w: f64, h: f64) {
        self.w = w;
        self.h = h;
    }

    /// The colliders this frame, top to bottom as drawn. Ledges of colliders that left are dropped; a column count that
    /// changed (a bubble resized) starts its ledge again.
    pub fn set_colliders(&mut self, cols: &[Collider]) {
        self.cols = cols.to_vec();
        let mut keep: HashSet<&str> = HashSet::new();
        for c in cols {
            keep.insert(c.id.as_str());
            let n = mmax(1.0, (c.w / COL_W).floor()) as usize;
            let fresh = match self.ledges.get(&c.id) {
                Some(l) => l.h.len() != n,
                None => true,
            };
            if fresh {
                self.ledges.insert(c.id.clone(), Ledge { h: vec![0.0; n], end_l: 0.0, end_r: 0.0 });
            }
        }
        self.ledges.retain(|id, _| keep.contains(id.as_str()));
    }

    /// The first collider with this id (its index in `cols`).
    fn col(&self, id: &str) -> Option<usize> {
        self.cols.iter().position(|c| c.id == id)
    }

    /// Run `seconds` of simulation at once (a scene that starts settled: rain already pooled, snow on the ledges,
    /// fireflies out). Call after the first set_view / set_colliders.
    pub fn warm(&mut self, seconds: f64) {
        let n = js_round(seconds / STEP);
        let mut i = 0.0;
        while i < n {
            self.step();
            i += 1.0;
        }
    }

    /// Advance by `dt` seconds of real time (fixed steps; a long stall is not caught up). Returns the steps run.
    pub fn advance(&mut self, dt: f64) -> u32 {
        self.acc += mmin(dt, STEP * MAX_STEPS as f64);
        let mut n = 0;
        while self.acc >= STEP - 1e-9 && n < MAX_STEPS {
            self.acc -= STEP;
            self.step();
            n += 1;
        }
        n
    }

    /// One fixed step.
    pub fn step(&mut self) {
        let dt = STEP;
        self.t += dt;
        let area = self.w * self.h;
        self.wind = 0.5 * fsin(self.t * 0.23) + 0.25 * fsin(self.t * 0.71 + 1.3);
        match self.effect {
            Effect::Rain => self.spawn(RATE.rain * (self.w + 200.0) * self.density, 0),
            Effect::Embers => self.spawn(RATE.embers * self.w * self.density, 4),
            Effect::Snow => self.spawn(RATE.snow * self.w * self.density, 6),
            Effect::Fireflies => {
                let want = clamp(js_round((RATE.fireflies * area * self.density) / 1e6), 3.0, 10.0);
                let have = self.parts.iter().filter(|p| p.k == 7).count() as f64;
                if have < want && self.rng.next() < 0.05 {
                    let p = self.new_firefly();
                    self.parts.push(p);
                }
            }
            Effect::None => {}
        }
        // index loop: pieces appended while moving (splashes) are moved in this same step too
        let mut i = 0;
        while i < self.parts.len() {
            let mut p = std::mem::take(&mut self.parts[i]);
            self.move_part(&mut p, dt);
            self.parts[i] = p;
            i += 1;
        }
        // drop the dead in place, order kept
        self.parts.retain(|p| p.life > 0.0);
        if self.effect == Effect::Rain {
            self.flow(dt);
        } else if self.effect == Effect::Snow {
            self.melt(dt);
        }
    }

    /// `make` = the kind spawned: 0 rain, 4 ember, 6 flake.
    fn spawn(&mut self, per_second: f64, make: u32) {
        self.spawn_acc += per_second * STEP;
        while self.spawn_acc >= 1.0 {
            self.spawn_acc -= 1.0;
            if self.parts.len() < PARTICLE_CAP {
                let p = match make {
                    0 => self.new_rain(),
                    4 => self.new_ember(),
                    _ => self.new_flake(),
                };
                self.parts.push(p);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn base(&mut self, k: u32, x: f64, y: f64, vx: f64, vy: f64, life: f64, size: f64) -> Particle {
        Particle {
            k,
            x,
            y,
            vx,
            vy,
            life,
            max: life,
            size,
            seed: self.rng.range(0.0, 60.0),
            on: String::new(),
            dir: 0.0,
        }
    }

    // The arguments are drawn into locals in the reference's (left-to-right) order: the RNG sequence is the contract.
    fn new_rain(&mut self) -> Particle {
        let vy = self.rng.range(700.0, 950.0);
        let x = self.rng.range(-100.0, self.w + 100.0);
        let y = -20.0 - self.rng.next() * 60.0;
        let vx = self.wind * 140.0 + self.rng.range(-15.0, 15.0);
        let size = self.rng.range(20.0, 28.0);
        self.base(0, x, y, vx, vy, 10.0, size)
    }

    fn new_ember(&mut self) -> Particle {
        let spread = (self.rng.next() + self.rng.next() + self.rng.next()) / 3.0 - 0.5;
        let x = self.w * (0.5 + spread * 1.6);
        let y = self.h + 6.0;
        let vx = self.rng.range(-20.0, 20.0);
        let vy = -self.rng.range(35.0, 90.0);
        let life = self.rng.range(4.0, 8.0);
        let size = self.rng.range(1.2, 2.4);
        self.base(4, x, y, vx, vy, life, size)
    }

    fn new_flake(&mut self) -> Particle {
        let x = self.rng.range(-40.0, self.w + 40.0);
        let vy = self.rng.range(18.0, 40.0);
        let size = self.rng.range(1.2, 2.6);
        self.base(6, x, -8.0, 0.0, vy, 90.0, size)
    }

    fn new_firefly(&mut self) -> Particle {
        let x = self.rng.range(0.05, 0.95) * self.w;
        let y = self.rng.range(0.1, 0.95) * self.h;
        let life = self.rng.range(14.0, 30.0);
        let size = self.rng.range(0.8, 1.2);
        self.base(7, x, y, 0.0, 0.0, life, size)
    }

    /// The collider whose top edge the segment (x, y0) → (x, y1) crosses going down, the highest one first.
    fn hit_top(&self, x: f64, y0: f64, y1: f64) -> Option<usize> {
        let mut best: Option<usize> = None;
        for (i, c) in self.cols.iter().enumerate() {
            if x <= c.x || x >= c.x + c.w {
                continue;
            }
            let top = top_at(c, x);
            if y0 <= top && y1 > top && best.is_none_or(|b| top < top_at(&self.cols[b], x)) {
                best = Some(i);
            }
        }
        best
    }

    /// The collider whose bottom edge the segment crosses going UP, the lowest one first.
    fn hit_bottom(&self, x: f64, y0: f64, y1: f64) -> Option<usize> {
        let mut best: Option<usize> = None;
        for (i, c) in self.cols.iter().enumerate() {
            if x <= c.x || x >= c.x + c.w {
                continue;
            }
            let b = c.y + c.h;
            if y0 >= b && y1 < b && best.is_none_or(|j| b > self.cols[j].y + self.cols[j].h) {
                best = Some(i);
            }
        }
        best
    }

    fn inside(&self, x: f64, y: f64, m: f64) -> Option<usize> {
        self.cols.iter().position(|c| x > c.x - m && x < c.x + c.w + m && y > c.y - m && y < c.y + c.h + m)
    }

    fn land(&mut self, ci: usize, x: f64, mass: f64) {
        let c = &self.cols[ci];
        let Some(l) = self.ledges.get_mut(&c.id) else { return };
        let n = l.h.len();
        let i = clamp(((x - c.x) / COL_W).floor(), 0.0, n as f64 - 1.0) as usize;
        let a = i.saturating_sub(1);
        let b = (i + 1).min(n - 1);
        l.h[a] += mass * 0.25;
        l.h[i] += mass * 0.5;
        l.h[b] += mass * 0.25;
    }

    fn move_part(&mut self, p: &mut Particle, dt: f64) {
        let y0 = p.y;
        if p.k == 0 {
            p.x += p.vx * dt;
            p.y += p.vy * dt;
            if let Some(ci) = self.hit_top(p.x, y0, p.y) {
                self.land(ci, p.x, RAIN_MASS);
                p.life = 0.0;
                if self.rng.next() < 0.2 && self.parts.len() < PARTICLE_CAP {
                    let y = top_at(&self.cols[ci], p.x) - 1.5;
                    let vx = self.rng.range(-60.0, 60.0);
                    let vy = -self.rng.range(50.0, 120.0);
                    let q = self.base(1, p.x, y, vx, vy, 0.35, 0.9);
                    self.parts.push(q);
                }
            } else if p.y > self.h + 30.0 {
                p.life = 0.0;
            }
            return;
        }
        if p.k == 1 || p.k == 2 || p.k == 5 {
            p.vy += (if p.k == 5 { -120.0 } else { 1400.0 }) * dt;
            p.vx *= 1.0 - 1.5 * dt;
            p.x += p.vx * dt;
            p.y += p.vy * dt;
            p.life -= if p.k == 2 { 0.0 } else { dt };
            if p.k == 2 && p.vy > 0.0 {
                if let Some(ci) = self.hit_top(p.x, y0, p.y) {
                    self.land(ci, p.x, p.size * 0.12);
                    p.life = 0.0;
                }
            }
            if p.k == 1 && p.vy > 0.0 && self.hit_top(p.x, y0, p.y).is_some() {
                p.life = 0.0;
            }
            if p.y > self.h + 20.0 || self.inside(p.x, p.y, 0.0).is_some() {
                p.life = 0.0;
            }
            return;
        }
        if p.k == 3 {
            let Some(ci) = self.col(&p.on) else {
                p.life = 0.0;
                return;
            };
            let (cx, cy, cw, ch) = (self.cols[ci].x, self.cols[ci].y, self.cols[ci].w, self.cols[ci].h);
            p.vy += (110.0 - p.vy) * mmin(1.0, dt * 3.0);
            p.y += p.vy * dt;
            p.x = if p.dir < 0.0 { cx - 2.2 } else { cx + cw + 2.2 };
            if p.y >= cy + ch {
                p.k = 2;
                p.on = String::new();
                p.vx = p.dir * self.rng.range(4.0, 18.0);
                p.vy = mmax(p.vy, 60.0);
            }
            return;
        }
        if p.k == 4 {
            p.life -= dt * (if !p.on.is_empty() { 1.6 } else { 1.0 });
            if !p.on.is_empty() {
                match self.col(&p.on) {
                    None => p.on = String::new(),
                    Some(ci) => {
                        let (cx, cy, cw, ch) = (self.cols[ci].x, self.cols[ci].y, self.cols[ci].w, self.cols[ci].h);
                        p.x += p.dir * p.vx * dt;
                        p.y = cy + ch + 3.0 + 0.8 * fsin(self.t * 9.0 + p.seed);
                        if p.x < cx - 3.0 || p.x > cx + cw + 3.0 {
                            p.on = String::new();
                            p.vy = -self.rng.range(35.0, 70.0);
                            p.vx = p.dir * self.rng.range(6.0, 20.0);
                        }
                    }
                }
            } else {
                p.vx += (fsin(p.seed + self.t * 1.1 + p.y * 0.011) * 60.0 + fsin(p.seed * 3.1 + self.t * 2.9) * 22.0
                    - p.vx * 1.6)
                    * dt;
                p.vy += (-38.0 - p.vy) * 0.45 * dt;
                p.x += p.vx * dt;
                p.y += p.vy * dt;
                if let Some(ci) = self.hit_bottom(p.x, y0, p.y) {
                    // it meets the underside of a panel: slide along it to the nearer side (sometimes the other way),
                    // then rise
                    let c = &self.cols[ci];
                    p.on = c.id.clone();
                    p.dir = if p.x - c.x < c.x + c.w - p.x { -1.0 } else { 1.0 };
                    let cyh = c.y + c.h;
                    if self.rng.next() < 0.15 {
                        p.dir = -p.dir;
                    }
                    p.vx = self.rng.range(30.0, 70.0);
                    p.y = cyh + 3.0;
                } else if let Some(si) = self.inside(p.x, p.y, 0.0) {
                    let s = &self.cols[si];
                    let left = p.x - s.x < s.x + s.w - p.x;
                    p.x = if left { s.x - 3.0 } else { s.x + s.w + 3.0 };
                    p.vx = if left { -p.vx.abs() * 0.3 } else { p.vx.abs() * 0.3 };
                }
            }
            if p.y < -40.0 || p.x < -60.0 || p.x > self.w + 60.0 {
                p.life = 0.0;
            }
            return;
        }
        if p.k == 6 {
            p.x += (self.wind * 18.0 + 14.0 * fsin(self.t * 0.9 + p.seed)) * dt;
            p.y += p.vy * dt;
            if let Some(ci) = self.hit_top(p.x, y0, p.y) {
                self.land(ci, p.x, p.size * SNOW_MASS);
                p.life = 0.0;
            } else if p.y > self.h + 10.0 {
                p.life = 0.0;
            }
            return;
        }
        if p.k == 7 {
            p.life -= dt;
            let ang = p.seed + self.t * 0.35 + 1.4 * fsin(self.t * 0.21 + p.seed * 1.7);
            let sp = 20.0 * p.size;
            let tx = sp * fsin(ang + PI / 2.0);
            let ty = sp * fsin(ang);
            p.vx += (tx - p.vx) * mmin(1.0, dt * 1.2);
            p.vy += (ty - p.vy) * mmin(1.0, dt * 1.2);
            // steer clear of the panels (14 px of air) and the edges
            if let Some(si) = self.inside(p.x, p.y, 16.0) {
                let s = &self.cols[si];
                let cx = s.x + s.w / 2.0;
                let cy = s.y + s.h / 2.0;
                let dx = (p.x - cx) / mmax(1.0, s.w / 2.0);
                let dy = (p.y - cy) / mmax(1.0, s.h / 2.0);
                if dx.abs() > dy.abs() {
                    p.vx += (if dx > 0.0 { 1.0 } else { -1.0 }) * 90.0 * dt;
                } else {
                    p.vy += (if dy > 0.0 { 1.0 } else { -1.0 }) * 90.0 * dt;
                }
            }
            if p.x < 10.0 {
                p.vx += 40.0 * dt;
            }
            if p.x > self.w - 10.0 {
                p.vx -= 40.0 * dt;
            }
            if p.y < 10.0 {
                p.vy += 40.0 * dt;
            }
            if p.y > self.h - 10.0 {
                p.vy -= 40.0 * dt;
            }
            p.x += p.vx * dt;
            p.y += p.vy * dt;
            if p.x < -30.0 || p.x > self.w + 30.0 || p.y < -30.0 || p.y > self.h + 30.0 {
                p.life = 0.0;
            }
        }
    }

    /// Rain: a pool on a bubble's top runs toward the nearer end (a bubble top is never quite level), smooths itself,
    /// and the ends drip down the sides; a dry spell slowly takes the film. Never deeper than LEDGE_MAX.
    #[allow(clippy::needless_range_loop)] // index loops with neighbours, as the reference writes them
    fn flow(&mut self, dt: f64) {
        let f = mmin(0.5, POOL_SPEED * dt / COL_W);
        let k = mmin(0.3, 12.0 * dt);
        for ci in 0..self.cols.len() {
            let Some(l) = self.ledges.get_mut(&self.cols[ci].id) else { continue };
            let h = &mut l.h;
            let n = h.len();
            let mid = (n as f64 - 1.0) / 2.0;
            // outward: the left half moves left, the right half right (from the ends inward, so water moves one column
            // a step)
            for i in 0..n {
                if (i as f64) < mid {
                    let m = h[i] * f;
                    h[i] -= m;
                    if i == 0 {
                        l.end_l += m * COL_W;
                    } else {
                        h[i - 1] += m;
                    }
                }
            }
            for i in (0..n).rev() {
                if (i as f64) > mid {
                    let m = h[i] * f;
                    h[i] -= m;
                    if i == n - 1 {
                        l.end_r += m * COL_W;
                    } else {
                        h[i + 1] += m;
                    }
                }
            }
            // level: each column moves toward its neighbours' mean (old values: `prev` keeps the left one)
            let mut prev = h[0];
            for i in 0..n {
                let cur = h[i];
                let left = if i > 0 { prev } else { cur };
                let right = if i < n - 1 { h[i + 1] } else { cur };
                h[i] = mmax(0.0, cur + (left + right - 2.0 * cur) * k * 0.5 - 0.02 * dt);
                prev = cur;
            }
            for i in 0..n {
                if h[i] > LEDGE_MAX {
                    let extra = h[i] - LEDGE_MAX;
                    h[i] = LEDGE_MAX;
                    if (i as f64) < mid {
                        l.end_l += extra * COL_W;
                    } else {
                        l.end_r += extra * COL_W;
                    }
                }
            }
            // the runners draw from the RNG left first, then right (they never touch the ledges)
            let mut run_l = None;
            let mut run_r = None;
            if l.end_l > RUNNER_MASS {
                run_l = Some(l.end_l);
                l.end_l = 0.0;
            }
            if l.end_r > RUNNER_MASS {
                run_r = Some(l.end_r);
                l.end_r = 0.0;
            }
            if let Some(m) = run_l {
                self.runner(ci, -1.0, m);
            }
            if let Some(m) = run_r {
                self.runner(ci, 1.0, m);
            }
        }
    }

    fn runner(&mut self, ci: usize, dir: f64, mass: f64) {
        if self.parts.len() >= PARTICLE_CAP {
            return;
        }
        let c = &self.cols[ci];
        let x = if dir < 0.0 { c.x - 2.2 } else { c.x + c.w + 2.2 };
        let y = c.y + 0.7 * mmin(c.h, if dir < 0.0 { c.rl.unwrap_or(0.0) } else { c.rr.unwrap_or(0.0) }) + 2.0;
        let id = c.id.clone();
        let mut p = self.base(3, x, y, 0.0, 20.0, 10.0, clamp(mass * 0.12, 3.0, 9.0));
        p.on = id;
        p.dir = dir;
        self.parts.push(p);
    }

    /// Snow: the lines melt slowly and never grow past LEDGE_MAX.
    fn melt(&mut self, dt: f64) {
        for c in &self.cols {
            let Some(l) = self.ledges.get_mut(&c.id) else { continue };
            for v in l.h.iter_mut() {
                *v = clamp(*v - 0.01 * dt, 0.0, LEDGE_MAX);
            }
        }
    }

    // ---- the mesh ------------------------------------------------------------------------------------------------
    /// Triangles, 6 floats per vertex: x, y, r, g, b, a (premultiplied, 0..1). A piece whose box touches any collider
    /// is left out (nothing is ever drawn inside a bubble or a panel). Returns the vertex count.
    pub fn mesh(&self, mode: Mode, out: &mut MeshOut) -> usize {
        out.n = 0;
        let pal = palette(mode);
        if self.effect == Effect::Rain || self.effect == Effect::Snow {
            let water = self.effect == Effect::Rain;
            let c0 = if water { pal.pool } else { pal.snow };
            let c1 = if water { pal.pool_top } else { pal.snow_top };
            for (ci, c) in self.cols.iter().enumerate() {
                let Some(l) = self.ledges.get(&c.id) else { continue };
                let n = l.h.len();
                let cw = c.w / n as f64;
                for i in 0..n {
                    let ha = if i > 0 { (l.h[i - 1] + l.h[i]) / 2.0 } else { l.h[i] };
                    let hb = if i < n - 1 { (l.h[i] + l.h[i + 1]) / 2.0 } else { l.h[i] };
                    if ha < 0.12 && hb < 0.12 {
                        continue;
                    }
                    // the pool sits on the top edge and steps down its rounded corners: each column's floor is the
                    // HIGHER of its two ends (a straight edge between two points of a convex corner would cut into it)
                    let xa = c.x + i as f64 * cw;
                    let xb = xa + cw;
                    let ba = mmin(top_at(c, xa), top_at(c, xb)) - 0.5;
                    let bb = ba;
                    let ya = ba - mmin(ha, LEDGE_MAX);
                    let yb2 = bb - mmin(hb, LEDGE_MAX);
                    if self.blocked(xa, mmin(ya, yb2), xb, mmax(ba, bb), Some(ci)) {
                        continue;
                    }
                    out.tri(xa, ba, c0, xb, bb, c0, xa, ya, c1);
                    out.tri(xb, bb, c0, xb, yb2, c1, xa, ya, c1);
                }
            }
        }
        for p in &self.parts {
            if p.k == 0 {
                let sp = (p.vx * p.vx + p.vy * p.vy).sqrt();
                let ux = p.vx / sp;
                let uy = p.vy / sp;
                let len = p.size;
                let tx = p.x - ux * len;
                let ty = p.y - uy * len;
                // a streak with soft sides (its own anti-aliasing): bright down the middle, clear at the edges and the
                // tail
                let hw = 1.0;
                if self.blocked(mmin(tx, p.x) - hw, mmin(ty, p.y), mmax(tx, p.x) + hw, mmax(ty, p.y), None) {
                    continue;
                }
                let px = -uy * hw;
                let py = ux * hw;
                out.tri(p.x + px, p.y + py, CLEAR, p.x, p.y, pal.rain, tx, ty, CLEAR);
                out.tri(p.x + px, p.y + py, CLEAR, tx, ty, CLEAR, tx + px, ty + py, CLEAR);
                out.tri(p.x, p.y, pal.rain, p.x - px, p.y - py, CLEAR, tx - px, ty - py, CLEAR);
                out.tri(p.x, p.y, pal.rain, tx - px, ty - py, CLEAR, tx, ty, CLEAR);
            } else if p.k == 1 {
                let a = clamp(p.life / p.max, 0.0, 1.0);
                self.blob(out, p.x, p.y, p.size, p.size, 0.0, 1.0, scale(pal.drop, a), CLEAR, 6);
            } else if p.k == 2 || p.k == 3 {
                let r = 0.55 * p.size.sqrt();
                let sp = (p.vx * p.vx + p.vy * p.vy).sqrt();
                let ux = if sp > 0.0 { p.vx / sp } else { 0.0 };
                let uy = if sp > 0.0 { p.vy / sp } else { 1.0 };
                let stretch = 1.0 + mmin(1.2, sp / 600.0);
                self.blob(out, p.x, p.y, r, r * stretch, ux, uy, pal.drop, pal.drop_edge, 8);
                if p.k == 3 {
                    // the wet track it leaves above itself, faint
                    if let Some(ci) = self.col(&p.on) {
                        let c = &self.cols[ci];
                        if p.y - c.y > 4.0 {
                            let hw = 0.7;
                            let top = mmax(c.y, p.y - 26.0);
                            if !self.blocked(p.x - hw, top, p.x + hw, p.y - r, None) {
                                out.tri(
                                    p.x - hw,
                                    p.y - r,
                                    pal.track,
                                    p.x + hw,
                                    p.y - r,
                                    pal.track,
                                    p.x - hw,
                                    top,
                                    CLEAR,
                                );
                                out.tri(p.x + hw, p.y - r, pal.track, p.x + hw, top, CLEAR, p.x - hw, top, CLEAR);
                            }
                        }
                    }
                }
            } else if p.k == 4 || p.k == 5 {
                let f = clamp(p.life / p.max, 0.0, 1.0);
                let flick = 0.88 + 0.12 * fsin(self.t * 7.0 + p.seed * 7.0);
                let heat = f * f.sqrt() * flick;
                let on = !p.on.is_empty();
                let vx = if on { p.dir * p.vx } else { p.vx };
                let vy = if on { 0.0 } else { p.vy };
                let sp = (vx * vx + vy * vy).sqrt();
                let ux = if sp > 0.0 { vx / sp } else { 0.0 };
                let uy = if sp > 0.0 { vy / sp } else { 1.0 };
                let hot: Rgb = [1.0, 0.25 + 0.65 * f * f, 0.04 + 0.45 * f * f * f];
                let r = p.size * 3.4;
                let light = mode == Mode::Light;
                let glow = if light { light_glow(hot, 0.4 * heat) } else { add_glow(hot, 0.38 * heat) };
                let core = if light { light_glow(hot, 0.7 * heat) } else { add_glow(hot, 0.8 * heat) };
                self.blob(out, p.x, p.y, r, r * (1.0 + mmin(1.6, sp / 160.0)), ux, uy, glow, CLEAR, 8);
                self.blob(
                    out,
                    p.x,
                    p.y,
                    p.size * 0.8,
                    p.size * 0.8 * (1.0 + mmin(1.2, sp / 220.0)),
                    ux,
                    uy,
                    core,
                    scale(core, 0.2),
                    6,
                );
            } else if p.k == 6 {
                self.blob(out, p.x, p.y, p.size, p.size, 0.0, 1.0, pal.flake, CLEAR, 6);
            } else if p.k == 7 {
                // a soft glow that never quite goes out, and a slow blink on top
                let pulse = mmax(0.0, fsin(self.t * 1.3 + p.seed * 2.3));
                let b = (0.18 + 0.82 * pulse * pulse * pulse)
                    * clamp(p.life, 0.0, 1.0)
                    * clamp((p.max - p.life) / 1.5, 0.0, 1.0);
                if b < 0.02 {
                    continue;
                }
                let light = mode == Mode::Light;
                let g = if light { light_glow(FIREFLY, 0.4 * b) } else { add_glow(FIREFLY, 0.4 * b) };
                let k = if light { light_glow(FIREFLY, 0.8 * b) } else { add_glow(FIREFLY, 0.8 * b) };
                self.blob(out, p.x, p.y, 13.0 * p.size, 13.0 * p.size, 0.0, 1.0, g, CLEAR, 8);
                self.blob(out, p.x, p.y, 2.0, 2.0, 0.0, 1.0, k, scale(k, 0.3), 6);
            }
        }
        out.n
    }

    /// Is the box (x0, y0)–(x1, y1) touching a collider (other than the one at index `skip`)?
    pub fn blocked(&self, x0: f64, y0: f64, x1: f64, y1: f64, skip: Option<usize>) -> bool {
        self.cols
            .iter()
            .enumerate()
            .any(|(i, c)| Some(i) != skip && x1 > c.x && x0 < c.x + c.w && y1 > c.y && y0 < c.y + c.h)
    }

    /// How far (x, y) is from the nearest collider, as the half-size of the largest square around it that touches
    /// none (Chebyshev distance; 0 inside one).
    pub fn clearance(&self, x: f64, y: f64) -> f64 {
        let mut best = 1e9;
        for c in &self.cols {
            let dx = mmax(mmax(c.x - x, 0.0), x - (c.x + c.w));
            let dy = mmax(mmax(c.y - y, 0.0), y - (c.y + c.h));
            let d = mmax(dx, dy);
            if d < best {
                best = d;
            }
        }
        best
    }

    /// An ellipse (a fan of `n` triangles) with its `ry` axis along (ux, uy); centre colour `ci`, rim colour `co`.
    /// Near a collider it shrinks to the air around it (a glow never reaches into a bubble; too close = not drawn).
    #[allow(clippy::too_many_arguments)]
    fn blob(
        &self,
        out: &mut MeshOut,
        x: f64,
        y: f64,
        rx0: f64,
        ry0: f64,
        ux: f64,
        uy: f64,
        ci: Rgba,
        co: Rgba,
        n: usize,
    ) {
        let ext = mmax(rx0, ry0);
        let room = self.clearance(x, y) - 0.5;
        if room <= 0.5 {
            return;
        }
        let k = if room < ext { room / ext } else { 1.0 };
        let rx = rx0 * k;
        let ry = ry0 * k;
        let ring: &[(f64, f64)] = if n == 6 { &RING6 } else { &RING8 };
        // axes: v along (ux, uy) scaled by ry, u across scaled by rx
        let ax = -uy * rx;
        let ay = ux * rx;
        let bx = ux * ry;
        let by = uy * ry;
        for i in 0..n {
            let (c0, s0) = ring[i];
            let (c1, s1) = ring[(i + 1) % n];
            out.tri(
                x,
                y,
                ci,
                x + ax * c0 + bx * s0,
                y + ay * c0 + by * s0,
                co,
                x + ax * c1 + bx * s1,
                y + ay * c1 + by * s1,
                co,
            );
        }
    }
}

pub type Rgba = [f64; 4];
const CLEAR: Rgba = [0.0, 0.0, 0.0, 0.0];
const FIREFLY: Rgb = [0.78, 1.0, 0.42];

fn scale(c: Rgba, k: f64) -> Rgba {
    [c[0] * k, c[1] * k, c[2] * k, c[3] * k]
}

/// A glow on a dark background: premultiplied with alpha = the brightest channel (reads like light added).
fn add_glow(c: Rgb, i: f64) -> Rgba {
    let m = mmax(mmax(c[0], c[1]), c[2]);
    [c[0] * i, c[1] * i, c[2] * i, m * i]
}

/// A glow on a light background: a deeper, opaque-ish tone (light added to white would be invisible).
fn light_glow(c: Rgb, i: f64) -> Rgba {
    let d: Rgb = [c[0] * 0.78, c[1] * 0.5, c[2] * 0.35];
    [d[0] * i, d[1] * i, d[2] * i, i]
}

/// Premultiplied from straight colour + alpha.
fn pm(r: f64, g: f64, b: f64, a: f64) -> Rgba {
    [r * a, g * a, b * a, a]
}

struct Palette {
    rain: Rgba,
    drop: Rgba,
    drop_edge: Rgba,
    track: Rgba,
    pool: Rgba,
    pool_top: Rgba,
    snow: Rgba,
    snow_top: Rgba,
    flake: Rgba,
}

fn palette(mode: Mode) -> Palette {
    match mode {
        Mode::Dark => Palette {
            rain: pm(0.78, 0.84, 0.92, 0.3),
            drop: pm(0.85, 0.9, 0.97, 0.7),
            drop_edge: pm(0.6, 0.68, 0.78, 0.25),
            track: pm(0.75, 0.82, 0.9, 0.16),
            pool: pm(0.55, 0.64, 0.76, 0.38),
            pool_top: pm(0.88, 0.93, 1.0, 0.75),
            snow: pm(0.86, 0.9, 0.95, 0.7),
            snow_top: pm(0.97, 0.98, 1.0, 0.9),
            flake: pm(0.94, 0.96, 1.0, 0.65),
        },
        Mode::Oled => Palette {
            rain: pm(0.7, 0.76, 0.86, 0.26),
            drop: pm(0.78, 0.84, 0.92, 0.6),
            drop_edge: pm(0.5, 0.58, 0.68, 0.2),
            track: pm(0.7, 0.76, 0.86, 0.12),
            pool: pm(0.45, 0.53, 0.64, 0.28),
            pool_top: pm(0.8, 0.86, 0.95, 0.55),
            snow: pm(0.8, 0.84, 0.9, 0.6),
            snow_top: pm(0.92, 0.94, 0.98, 0.8),
            flake: pm(0.88, 0.9, 0.96, 0.55),
        },
        Mode::Light => Palette {
            rain: pm(0.3, 0.38, 0.5, 0.26),
            drop: pm(0.35, 0.45, 0.6, 0.6),
            drop_edge: pm(0.3, 0.38, 0.5, 0.2),
            track: pm(0.35, 0.45, 0.6, 0.12),
            pool: pm(0.4, 0.52, 0.68, 0.3),
            pool_top: pm(0.3, 0.42, 0.6, 0.55),
            snow: pm(1.0, 1.0, 1.0, 0.92),
            snow_top: pm(0.78, 0.84, 0.92, 0.95),
            flake: pm(0.62, 0.7, 0.82, 0.7),
        },
    }
}

// = 0.7071067811865476, the reference's literal (same double)
const S2: f64 = std::f64::consts::FRAC_1_SQRT_2;
const S3: f64 = 0.8660254037844386;
const RING8: [(f64, f64); 8] =
    [(1.0, 0.0), (S2, S2), (0.0, 1.0), (-S2, S2), (-1.0, 0.0), (-S2, -S2), (0.0, -1.0), (S2, -S2)];
const RING6: [(f64, f64); 6] = [(1.0, 0.0), (0.5, S3), (-0.5, S3), (-1.0, 0.0), (-0.5, -S3), (0.5, -S3)];

/// A growable vertex buffer (6 floats per vertex, f32 like the web's Float32Array).
#[derive(Clone, Debug)]
pub struct MeshOut {
    pub buf: Vec<f32>,
    pub n: usize,
}

impl Default for MeshOut {
    fn default() -> Self {
        MeshOut::new(8192)
    }
}

impl MeshOut {
    /// Room for `cap` vertices to start with (it doubles as needed).
    pub fn new(cap: usize) -> MeshOut {
        MeshOut { buf: vec![0.0; cap * 6], n: 0 }
    }

    fn v(&mut self, x: f64, y: f64, c: Rgba) {
        if (self.n + 1) * 6 > self.buf.len() {
            let len = (self.buf.len() * 2).max(6);
            self.buf.resize(len, 0.0);
        }
        let o = self.n * 6;
        self.buf[o] = x as f32;
        self.buf[o + 1] = y as f32;
        self.buf[o + 2] = c[0] as f32;
        self.buf[o + 3] = c[1] as f32;
        self.buf[o + 4] = c[2] as f32;
        self.buf[o + 5] = c[3] as f32;
        self.n += 1;
    }

    #[allow(clippy::too_many_arguments)]
    pub fn tri(&mut self, x0: f64, y0: f64, c0: Rgba, x1: f64, y1: f64, c1: Rgba, x2: f64, y2: f64, c2: Rgba) {
        self.v(x0, y0, c0);
        self.v(x1, y1, c1);
        self.v(x2, y2, c2);
    }
}

/// Round to 1/1000 the same way in both languages (floor(x·1000 + 0.5) / 1000) — what the vectors compare.
pub fn r3(x: f64) -> f64 {
    (x * 1000.0 + 0.5).floor() / 1000.0
}

/// A summary of the sim the vectors pin: counts per kind, the first pieces, ledge sums, and the mesh.
pub fn sim_digest(sim: &AmbienceSim, mode: Mode) -> Value {
    let mut kinds = [0u32; 8];
    for p in &sim.parts {
        kinds[p.k as usize] += 1;
    }
    let first: Vec<Value> = sim
        .parts
        .iter()
        .take(6)
        .map(|p| json!([num(p.k as f64), num(r3(p.x)), num(r3(p.y)), num(r3(p.vx)), num(r3(p.vy))]))
        .collect();
    let ledges: Vec<Value> = sim
        .cols
        .iter()
        .map(|c| {
            let mut s = 0.0;
            if let Some(l) = sim.ledges.get(&c.id) {
                for v in &l.h {
                    s += v;
                }
            }
            json!([c.id, num(r3(s))])
        })
        .collect();
    let mut out = MeshOut::new(1024);
    let n = sim.mesh(mode, &mut out);
    let mut sx = 0.0;
    let mut sa = 0.0;
    for i in 0..n {
        sx += out.buf[i * 6] as f64 + out.buf[i * 6 + 1] as f64;
        sa += out.buf[i * 6 + 5] as f64;
    }
    let mut m = Map::new();
    m.insert("kinds".into(), Value::Array(kinds.iter().map(|&k| num(k as f64)).collect()));
    m.insert("first".into(), Value::Array(first));
    m.insert("ledges".into(), Value::Array(ledges));
    m.insert("verts".into(), num(n as f64));
    m.insert("sumxy".into(), num(r3(sx)));
    m.insert("suma".into(), num(r3(sa)));
    Value::Object(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vectors() -> Value {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../testvectors/ambience/vectors.json");
        // JSON.parse-exact numbers (correctly rounded; integral values as integers, like `num`)
        tinhat_theme::json::parse_json(&std::fs::read_to_string(path).expect("vectors")).expect("json")
    }

    fn s(v: &Value) -> &str {
        v.as_str().unwrap()
    }

    fn f(v: &Value) -> f64 {
        v.as_f64().unwrap()
    }

    fn strs(v: &Value) -> Vec<&str> {
        v.as_array().unwrap().iter().map(s).collect()
    }

    fn look_json(l: &Look) -> Value {
        json!({
            "stops": l.stops.iter().map(|&(y, c)| json!([num(y), c])).collect::<Vec<_>>(),
            "glow": {"x": num(l.glow.x), "y": num(l.glow.y), "rx": num(l.glow.rx), "ry": num(l.glow.ry),
                     "color": l.glow.color, "a": num(l.glow.a)},
        })
    }

    fn preset_json(p: &Preset) -> Value {
        json!({"id": p.id, "name": p.name, "effect": p.effect.as_str(), "dark": look_json(&p.dark),
               "oled": look_json(&p.oled), "light": look_json(&p.light)})
    }

    fn background_from(v: &Value) -> Background {
        Background {
            kind: BackgroundKind::parse(s(&v["kind"])).unwrap(),
            preset: v["preset"].as_str().map(str::to_string),
            effect: Effect::parse(s(&v["effect"])).unwrap(),
            image: v["image"].as_str().map(str::to_string),
            by: s(&v["by"]).to_string(),
        }
    }

    fn collider(v: &Value) -> Collider {
        Collider {
            id: s(&v["id"]).to_string(),
            x: f(&v["x"]),
            y: f(&v["y"]),
            w: f(&v["w"]),
            h: f(&v["h"]),
            rl: v.get("rl").and_then(Value::as_f64),
            rr: v.get("rr").and_then(Value::as_f64),
        }
    }

    fn colliders(v: &Value) -> Vec<Collider> {
        v["sim"]["colliders"].as_array().unwrap().iter().map(collider).collect()
    }

    #[test]
    fn constants_are_the_shared_constants() {
        let v = vectors();
        let c = &v["constants"];
        assert_eq!(BACKGROUND_STATE, s(&c["state"]));
        assert_eq!(PREF_BACKGROUNDS, s(&c["prefs"]["backgrounds"]));
        assert_eq!(PREF_EFFECTS, s(&c["prefs"]["effects"]));
        assert_eq!(PREF_PAUSE_UNFOCUSED, s(&c["prefs"]["pause_unfocused"]));
        let defaults: Map<String, Value> = PREF_DEFAULTS.iter().map(|&(k, b)| (k.to_string(), json!(b))).collect();
        assert_eq!(Value::Object(defaults), c["pref_defaults"]);
        assert_eq!(EFFECTS.iter().map(|e| e.as_str()).collect::<Vec<_>>(), strs(&c["effects"]));
        let names: Map<String, Value> = EFFECTS.iter().map(|e| (e.as_str().to_string(), json!(e.name()))).collect();
        assert_eq!(Value::Object(names), c["effect_names"]);
        let fps: Map<String, Value> = EFFECTS.iter().map(|e| (e.as_str().to_string(), num(e.fps() as f64))).collect();
        assert_eq!(Value::Object(fps), c["effect_fps"]);
        assert_eq!(Value::Array(PRESETS.iter().map(preset_json).collect()), c["presets"]);
        assert_eq!(num(STEP), c["step"]);
        assert_eq!(num(MAX_STEPS as f64), c["max_steps"]);
        assert_eq!(num(COL_W), c["col_w"]);
        assert_eq!(num(LEDGE_MAX), c["ledge_max"]);
        assert_eq!(num(PARTICLE_CAP as f64), c["particle_cap"]);
        assert_eq!(num(WARM_S), c["warm_s"]);
        assert_eq!(num(RAIN_MASS), c["rain_mass"]);
        assert_eq!(num(POOL_SPEED), c["pool_speed"]);
        assert_eq!(num(RUNNER_MASS), c["runner_mass"]);
        assert_eq!(num(SNOW_MASS), c["snow_mass"]);
        let rate = json!({"rain": num(RATE.rain), "embers": num(RATE.embers), "snow": num(RATE.snow),
                          "fireflies": num(RATE.fireflies)});
        assert_eq!(rate, c["rate"]);
        assert_eq!(num(MIN_CONTRAST), c["min_contrast"]);
        assert_eq!(num(SCRIM_STEP), c["scrim_step"]);
        assert_eq!(num(SCRIM_MAX), c["scrim_max"]);
        assert_eq!(num(SUB_AA_SLACK), c["sub_aa_slack"]);
    }

    #[test]
    fn pref_on() {
        let v = vectors();
        for c in v["pref_on"].as_array().unwrap() {
            assert_eq!(
                super::pref_on(c["raw"].as_str(), c["dflt"].as_bool().unwrap()),
                c["want"].as_bool().unwrap(),
                "{c}"
            );
        }
    }

    #[test]
    fn parse_background() {
        let v = vectors();
        let cases = v["parse"].as_array().unwrap();
        assert!(cases.len() >= 25);
        let mut by = std::collections::HashMap::new();
        for c in cases {
            let got = super::parse_background(&c["content"], s(&c["sender"]));
            assert_eq!(got.to_json(), c["want"], "{}", c["name"]);
            by.insert(s(&c["name"]).to_string(), s(&c["want"]["kind"]).to_string());
        }
        // the product rule: absent = nothing chosen (game art may show); an explicit none = plain on purpose
        assert_eq!(by["absent (unset)"], "unset");
        assert_eq!(by["explicit none"], "none");
        assert_eq!(by["a newer kind shows its preset"], "preset");
    }

    #[test]
    fn background_content() {
        let v = vectors();
        for c in v["content"].as_array().unwrap() {
            let kind = BackgroundKind::parse(s(&c["kind"])).unwrap();
            let effect = c["effect"].as_str().and_then(Effect::parse);
            let got = super::background_content(kind, c["preset"].as_str(), effect, c["image"].as_str());
            assert_eq!(got, c["want"], "{}/{}/{}", c["kind"], c["preset"], c["effect"]);
        }
        // what the picker writes reads back as itself
        for p in &PRESETS {
            for e in EFFECTS {
                let got = super::parse_background(
                    &super::background_content(BackgroundKind::Preset, Some(p.id), Some(e), None),
                    "@a:x",
                );
                let want = Background {
                    kind: BackgroundKind::Preset,
                    preset: Some(p.id.into()),
                    effect: e,
                    image: None,
                    by: "@a:x".into(),
                };
                assert_eq!(got, want);
                let got = super::parse_background(
                    &super::background_content(BackgroundKind::Image, Some(p.id), Some(e), Some("$i")),
                    "@a:x",
                );
                let want = Background {
                    kind: BackgroundKind::Image,
                    preset: Some(p.id.into()),
                    effect: e,
                    image: Some("$i".into()),
                    by: "@a:x".into(),
                };
                assert_eq!(got, want);
            }
        }
        let none = super::background_content(BackgroundKind::None, None, None, None);
        assert_eq!(super::parse_background(&none, "@a:x").kind, BackgroundKind::None);
    }

    #[test]
    fn image_shown_else_the_preset() {
        let v = vectors();
        let cases = v["image_shown"].as_array().unwrap();
        let mut by = std::collections::HashMap::new();
        for c in cases {
            let bg = background_from(&c["bg"]);
            let ev = c["ev"].as_object().map(|e| ImageEvent {
                r#type: s(&e["type"]).into(),
                sender: s(&e["sender"]).into(),
                room: s(&e["room"]).into(),
                msgtype: s(&e["msgtype"]).into(),
                redacted: e["redacted"].as_bool().unwrap(),
                decrypted: e["decrypted"].as_bool().unwrap(),
            });
            let want = c["want"].as_bool().unwrap();
            assert_eq!(image_shown(&bg, s(&c["room"]), ev.as_ref()), want, "{}", c["name"]);
            by.insert(s(&c["name"]).to_string(), want);
            if !want && bg.kind == BackgroundKind::Image {
                assert!(preset_by_id(bg.preset.as_deref().unwrap()).is_some(), "{}: has a fallback", c["name"]);
            }
        }
        // a member who can't decrypt the image sees the PRESET fallback, never a broken tile
        assert!(!by["no key yet (joined after it)"]);
        assert!(!by["someone else's image"]);
        assert!(by["own image, decrypted"]);
    }

    #[test]
    fn apply_plan() {
        let v = vectors();
        for c in v["apply_plan"].as_array().unwrap() {
            let b = &c["bg"];
            let bg = Background {
                kind: BackgroundKind::parse(s(&b["kind"])).unwrap(),
                preset: b["preset"].as_str().map(str::to_string),
                effect: Effect::parse(s(&b["effect"])).unwrap(),
                image: b["image"].as_str().map(str::to_string),
                by: s(&b["by"]).to_string(),
            };
            let ch = &c["choice"];
            let choice = SheetChoice {
                kind: BackgroundKind::parse(s(&ch["kind"])).unwrap(),
                preset: ch["preset"].as_str().map(str::to_string),
                effect: ch["effect"].as_str().and_then(Effect::parse),
                new_image: ch["new_image"].as_bool().unwrap(),
            };
            assert_eq!(super::apply_plan(&bg, &choice, s(&c["me"])), s(&c["want"]), "{}", c["name"]);
        }
    }

    #[test]
    fn particles_pick() {
        // the ☰ Particles menu's effect rows write what the Chat background sheet writes (round 4)
        let v = vectors();
        for c in v["particles_pick"].as_array().unwrap() {
            let r = super::particles_pick(&background_from(&c["bg"]), Effect::parse(s(&c["effect"])).unwrap(), s(&c["me"]));
            assert_eq!(r.plan, s(&c["want"]), "{}", c["name"]);
            assert_eq!(r.content.unwrap_or(Value::Null), c["content"], "{}", c["name"]);
        }
    }

    #[test]
    fn particles_menu() {
        let v = vectors();
        for c in v["particles_menu"].as_array().unwrap() {
            let i = &c["input"];
            let input = ParticlesMenuIn {
                room: i["room"].as_bool().unwrap(),
                bg: background_from(&i["bg"]),
                can_change: i["can_change"].as_bool().unwrap(),
                me: s(&i["me"]).to_string(),
                backgrounds: i["backgrounds"].as_bool().unwrap(),
                effects: i["effects"].as_bool().unwrap(),
                pause_unfocused: i["pause_unfocused"].as_bool().unwrap(),
                saver: i["saver"].as_bool().unwrap(),
            };
            let rows: Vec<Value> = super::particles_menu(&input).iter().map(ParticlesRow::to_json).collect();
            assert_eq!(Value::Array(rows), c["want"], "{}", c["name"]);
        }
    }

    #[test]
    fn image_limits() {
        let v = vectors();
        let l = &v["image_limits"];
        assert_eq!(num(IMAGE_MAX_SIDE), l["max_side"]);
        assert_eq!(num(IMAGE_MAX_BYTES as f64), l["max_bytes"]);
        assert_eq!(Value::Array(IMAGE_QUALITIES.iter().map(|&q| num(q)).collect()), l["qualities"]);
        assert_eq!(num(IMAGE_SHRINK), l["shrink"]);
        for c in v["fit_image"].as_array().unwrap() {
            let got = fit_image(f(&c["w"]), f(&c["h"]), IMAGE_MAX_SIDE);
            assert_eq!(json!({"w": num(got.w), "h": num(got.h)}), c["want"], "{}x{}", c["w"], c["h"]);
        }
    }

    #[test]
    fn can_change_background() {
        let v = vectors();
        let mut by = std::collections::HashMap::new();
        for c in v["can_change"].as_array().unwrap() {
            assert_eq!(num(background_level(&c["pl"])), c["level"], "{}", c["name"]);
            let want = c["want"].as_bool().unwrap();
            let got = super::can_change_background(&c["pl"], s(&c["user"]), c["creator"].as_bool().unwrap());
            assert_eq!(got, want, "{}", c["name"]);
            by.insert(s(&c["name"]).to_string(), want);
        }
        assert!(by["moderator, default 50"]);
        assert!(!by["member, default 50"]);
        assert!(by["DM: both admins"]);
    }

    #[test]
    fn live_window_state() {
        let v = vectors();
        let mut by = std::collections::HashMap::new();
        for c in v["live"].as_array().unwrap() {
            let i = &c["in"];
            let got = live_win_state(&LiveIn {
                hidden: i["hidden"].as_bool().unwrap(),
                app_visible: i["app_visible"].as_bool(),
                covered: i["covered"].as_bool(),
                app_focused: i["app_focused"].as_bool(),
                phone_shell: i["phone_shell"].as_bool().unwrap(),
                has_focus: i["has_focus"].as_bool().unwrap(),
                native_call: i["native_call"].as_bool().unwrap(),
            });
            assert_eq!(got.as_str(), s(&c["want"]), "{}", c["name"]);
            by.insert(s(&c["name"]).to_string(), s(&c["want"]).to_string());
        }
        assert_eq!(by["android: appState hidden while the page says visible (call + Home)"], "hidden");
        assert_eq!(by["ios: call screen covers the page"], "hidden");
        assert_eq!(by["ios: before the first tap (hasFocus false)"], "focused");
        // 3-3: today's iOS app has no covered(): its native call pauses the effects; an app that reports cover decides
        assert_eq!(by["ios today (no covered()): a native call is on (its screen may cover the page)"], "hidden");
        assert_eq!(by["ios with covered(): a native call, page not covered (Media Party)"], "focused");
        assert_eq!(by["android with appState: a native call while the app is in front"], "focused");
        // 3-4: an older Android app (no appState) in front before the first tap: focused, like the iOS app
        assert_eq!(by["android 0.2.7 (no appState), in front before the first tap (hasFocus false)"], "focused");
        assert_eq!(by["android 0.2.7 (no appState), backgrounded"], "hidden");
    }

    #[test]
    fn effects_state() {
        let v = vectors();
        let mut by = std::collections::HashMap::new();
        for c in v["effects_state"].as_array().unwrap() {
            let i = &c["in"];
            let b = |k: &str| i[k].as_bool().unwrap();
            let got = super::effects_state(&EffectsIn {
                backgrounds: b("backgrounds"),
                effects: b("effects"),
                pause_unfocused: b("pause_unfocused"),
                saver: b("saver"),
                reduced: b("reduced"),
                win: WinState::parse(s(&i["win"])),
                gpu: b("gpu"),
                effect: Effect::parse(s(&i["effect"])).unwrap(),
            });
            assert_eq!(got.as_str(), s(&c["want"]), "{}", c["name"]);
            assert_eq!(EffectsState::parse(s(&c["want"])), Some(got));
            by.insert(s(&c["name"]).to_string(), s(&c["want"]).to_string());
        }
        assert_eq!(by["hidden always pauses"], "paused");
        assert_eq!(by["defaults, unfocused (pause is on by default)"], "paused");
        assert_eq!(by["save GPU, focused = run"], "run");
        assert_eq!(by["save GPU, unfocused, pause off = paused"], "paused");
        assert_eq!(by["reduce motion"], "off");
        for c in v["pause_row"].as_array().unwrap() {
            let r = pause_row(c["pause"].as_bool().unwrap(), c["saver"].as_bool().unwrap());
            assert_eq!(json!({"checked": r.checked, "enabled": r.enabled}), c["want"]);
        }
        assert!(PREF_DEFAULTS.iter().any(|&(k, d)| k == PREF_PAUSE_UNFOCUSED && d));
    }

    #[test]
    fn theme_mode() {
        let v = vectors();
        for c in v["theme_mode"].as_array().unwrap() {
            assert_eq!(super::theme_mode(s(&c["bg"])).as_str(), s(&c["want"]), "{}", c["bg"]);
        }
    }

    fn cells_json(cells: &[Cell]) -> Value {
        Value::Array(cells.iter().map(|c| json!([c[0].iter().map(|&x| num(x)).collect::<Vec<_>>(), c[1].iter().map(|&x| num(x)).collect::<Vec<_>>()])).collect())
    }
    fn cells_from(v: &Value) -> Vec<Cell> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|c| {
                let f = |i: usize| -> Rgb {
                    let a = c[i].as_array().unwrap();
                    [a[0].as_f64().unwrap(), a[1].as_f64().unwrap(), a[2].as_f64().unwrap()]
                };
                [f(0), f(1)]
            })
            .collect()
    }
    /// The promise itself, checked by brute force: every colour inside every cell (a 5 × 5 × 5 grid of each box),
    /// veiled with the scrim, keeps every text colour at its target.
    fn holds(cells: &[Cell], bg_hex: &str, fgs: &[&str], a: f64) -> String {
        let bg = parse_hex(bg_hex).unwrap();
        for cell in cells {
            for i in 0..=4 {
                for j in 0..=4 {
                    for k in 0..=4 {
                        let n = [i, j, k];
                        let c: Rgb = [0, 1, 2].map(|ch| cell[0][ch] + ((cell[1][ch] - cell[0][ch]) * n[ch] as f64) / 4.0);
                        let m = mix(c, bg, a);
                        for f in fgs {
                            let fc = parse_hex(f).unwrap();
                            if contrast(fc, m) < contrast_target(fc, bg) - 1e-9 {
                                return format!("{f} over {c:?}");
                            }
                        }
                    }
                }
            }
        }
        String::new()
    }

    #[test]
    fn scrim_keeps_text_readable() {
        let v = vectors();
        let cases = v["scrim"].as_array().unwrap();
        assert!(cases.len() >= 30);
        for c in cases {
            let p = preset_by_id(s(&c["preset"])).unwrap();
            let look = look_for(p, Mode::parse(s(&c["mode"])).unwrap());
            assert_eq!(cells_json(&look_cells(look)), c["cells"], "{}/{} cells", c["theme"], c["preset"]);
            let fgs = strs(&c["fgs"]);
            let a = scrim_for(look, s(&c["bg"]), &fgs);
            assert_eq!(num(a), c["want"], "{}/{}", c["theme"], c["preset"]);
            assert_eq!(holds(&look_cells(look), s(&c["bg"]), &fgs, a), "", "{}/{}", c["theme"], c["preset"]);
        }
    }

    #[test]
    fn scrim_for_image_cells() {
        let v = vectors();
        let mut by: std::collections::HashMap<String, Vec<Cell>> = std::collections::HashMap::new();
        for c in v["image_cells"].as_array().unwrap() {
            let px: Vec<u8> = c["px"].as_array().unwrap().iter().map(|x| x.as_u64().unwrap() as u8).collect();
            let got = image_cells(&px, c["w"].as_u64().unwrap() as usize, c["h"].as_u64().unwrap() as usize, 4);
            assert_eq!(cells_json(&got), c["want"], "{}", c["name"]);
            by.insert(s(&c["name"]).to_string(), cells_from(&c["want"]));
        }
        let cases = v["scrim_cells"].as_array().unwrap();
        assert!(cases.len() >= 20);
        for c in cases {
            let cells = &by[s(&c["image"])];
            let fgs = strs(&c["fgs"]);
            let a = scrim_for_cells(cells, s(&c["bg"]), &fgs);
            assert_eq!(num(a), c["want"], "{}/{}", c["theme"], c["image"]);
            assert_eq!(holds(cells, s(&c["bg"]), &fgs, a), "", "{}/{}", c["theme"], c["image"]);
        }
    }

    #[test]
    fn rng_and_fsin() {
        let v = vectors();
        let mut r = Rng::new(1);
        let got: Vec<Value> = (0..5).map(|_| num(r.next())).collect();
        assert_eq!(Value::Array(got), v["rng"]["seed1"]);
        let mut r2 = Rng::new(0xdeadbeef);
        let got: Vec<Value> = (0..3).map(|_| num(r2.next())).collect();
        assert_eq!(Value::Array(got), v["rng"]["seed_deadbeef"]);
        for c in v["fsin"].as_array().unwrap() {
            let x = f(&c["x"]);
            assert_eq!(num(r3(fsin(x) * 1000.0)), c["want"], "{x}");
            assert!((fsin(x) - x.sin()).abs() < 1e-6, "fsin {x}");
        }
    }

    fn run_case(v: &Value, c: &Value, mut each: impl FnMut(&AmbienceSim, usize)) -> AmbienceSim {
        let view = &v["sim"]["view"];
        let cols = colliders(v);
        let mut sim = AmbienceSim::new(
            Effect::parse(s(&c["effect"])).unwrap(),
            c["seed"].as_u64().unwrap() as u32,
            f(&c["density"]),
        );
        sim.set_view(f(&view["w"]), f(&view["h"]));
        sim.set_colliders(&cols);
        let steps = c["steps"].as_u64().unwrap() as usize;
        let scroll = c["scroll"].as_bool().unwrap();
        for i in 0..steps {
            if scroll && i as f64 >= steps as f64 / 2.0 {
                let k = i as f64 - steps as f64 / 2.0;
                let mut moved: Vec<Collider> =
                    cols.iter().filter(|x| x.id != "$a").map(|x| Collider { y: x.y - k * 2.0, ..x.clone() }).collect();
                moved.push(Collider {
                    id: "$e".into(),
                    x: 60.0,
                    y: 560.0 - k * 2.0,
                    w: 340.0,
                    h: 60.0,
                    rl: None,
                    rr: None,
                });
                sim.set_colliders(&moved);
            }
            sim.step();
            each(&sim, i);
        }
        sim
    }

    fn mode_of(c: &Value) -> Mode {
        Mode::parse(s(&c["mode"])).unwrap()
    }

    #[test]
    fn sim_digests() {
        let v = vectors();
        let cases = v["sim"]["cases"].as_array().unwrap();
        assert!(cases.len() >= 8);
        for c in cases {
            let sim = run_case(&v, c, |_, _| {});
            assert_eq!(sim_digest(&sim, mode_of(c)), c["want"], "{}", c["name"]);
        }
    }

    #[test]
    fn sim_is_alive() {
        // each effect actually shows something: particles of its own kinds, and rain / snow gather on the bubbles
        let v = vectors();
        let cases = v["sim"]["cases"].as_array().unwrap();
        let by = |name: &str| &cases.iter().find(|c| c["name"] == name).unwrap()["want"];
        let ledge_over_1 = |w: &Value| w["ledges"].as_array().unwrap().iter().any(|l| f(&l[1]) > 1.0);
        let r = by("rain 6 s light");
        assert!(f(&r["kinds"][0]) >= 4.0, "rain falls");
        assert!(ledge_over_1(r), "rain pools on a bubble");
        {
            // over half a minute the pools run off the ends: drips run down the sides
            let mut sim = AmbienceSim::new(Effect::Rain, 11, 1.0);
            sim.set_view(f(&v["sim"]["view"]["w"]), f(&v["sim"]["view"]["h"]));
            sim.set_colliders(&colliders(&v));
            let mut drips = 0;
            for _ in 0..1800 {
                sim.step();
                drips += sim.parts.iter().filter(|p| p.k == 3).count();
            }
            assert!(drips > 0, "pools run off the ends");
        }
        assert!(f(&by("embers 4 s")["kinds"][4]) >= 5.0, "embers rise");
        assert!(ledge_over_1(by("snow 8 s oled")), "snow settles");
        assert!(f(&by("fireflies 10 s")["kinds"][7]) >= 3.0, "fireflies");
        for c in cases {
            assert!(f(&c["want"]["verts"]) > 0.0, "{} draws", c["name"]);
        }
    }

    #[test]
    fn nothing_is_drawn_inside_a_bubble() {
        // every 10th step of every case: no vertex strictly inside any collider (the mesh leaves out every piece whose
        // box touches one, and glows shrink to the air around them)
        let v = vectors();
        let mut out = MeshOut::new(1024);
        let mut checked = 0usize;
        for c in v["sim"]["cases"].as_array().unwrap() {
            let mode = mode_of(c);
            run_case(&v, c, |sim, i| {
                if i % 10 != 0 {
                    return;
                }
                let n = sim.mesh(mode, &mut out);
                // every vertex AND points inside every triangle (centroid, edge midpoints, near each corner): a straight
                // edge between two points outside a rounded corner can still cut into it
                let mut t = 0;
                while t + 2 < n {
                    let p: Vec<(f64, f64)> =
                        (0..3).map(|v| (out.buf[(t + v) * 6] as f64, out.buf[(t + v) * 6 + 1] as f64)).collect();
                    let mut pts = p.clone();
                    for [u, v, w] in [
                        [1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0],
                        [0.5, 0.5, 0.0],
                        [0.5, 0.0, 0.5],
                        [0.0, 0.5, 0.5],
                        [0.8, 0.1, 0.1],
                        [0.1, 0.8, 0.1],
                        [0.1, 0.1, 0.8],
                    ] {
                        pts.push((u * p[0].0 + v * p[1].0 + w * p[2].0, u * p[0].1 + v * p[1].1 + w * p[2].1));
                    }
                    for &(x, y) in &pts {
                        for b in &sim.cols {
                            let inner =
                                Collider { x: b.x + 1e-3, y: b.y + 1e-3, w: b.w - 2e-3, h: b.h - 2e-3, ..b.clone() };
                            assert!(
                                !inside_shape(&inner, x, y),
                                "{} step {i}: point ({x}, {y}) of a triangle inside {}",
                                c["name"],
                                b.id
                            );
                        }
                    }
                    checked += 1;
                    t += 3;
                }
            });
        }
        assert!(checked > 3000, "checked {checked}");
    }

    #[test]
    fn premultiplied_colours() {
        // every vertex colour is valid premultiplied (r, g, b ≤ a ≤ 1): the same on every blend path
        let v = vectors();
        let mut out = MeshOut::new(1024);
        for c in v["sim"]["cases"].as_array().unwrap() {
            let sim = run_case(&v, c, |_, _| {});
            let n = sim.mesh(mode_of(c), &mut out);
            for k in 0..n {
                let [r, g, b, a] = [2, 3, 4, 5].map(|j| out.buf[k * 6 + j] as f64);
                assert!(
                    a <= 1.0 + 1e-6
                        && r <= a + 1e-6
                        && g <= a + 1e-6
                        && b <= a + 1e-6
                        && r.min(g).min(b).min(a) >= -1e-6,
                    "{} vertex {k}: {r},{g},{b},{a}",
                    c["name"]
                );
            }
        }
    }

    #[test]
    fn advance_fixed_steps() {
        let v = vectors();
        let mut sim = AmbienceSim::new(Effect::Rain, 1, 1.0);
        sim.set_view(400.0, 300.0);
        sim.set_colliders(&[]);
        for c in v["advance"].as_array().unwrap() {
            assert_eq!(num(sim.advance(f(&c["dt"])) as f64), c["want"], "{}", c["dt"]);
        }
    }

    #[test]
    fn top_edge_follows_rounded_corners() {
        let v = vectors();
        let c = Collider { id: "x".into(), x: 10.0, y: 100.0, w: 200.0, h: 50.0, rl: Some(12.0), rr: Some(4.0) };
        for t in v["top_at"].as_array().unwrap() {
            assert_eq!(num(r3(top_at(&c, f(&t["x"])))), t["want"], "{}", t["x"]);
        }
        assert_eq!(top_at(&c, 110.0), 100.0);
        assert_eq!(top_at(&c, 10.0), 112.0);
        assert!(!inside_shape(&c, 11.0, 101.0) && inside_shape(&c, 30.0, 101.0) && inside_shape(&c, 11.0, 120.0));
    }

    #[test]
    fn bounded() {
        // a long heavy run never passes the particle cap, and ledges never pass LEDGE_MAX
        let v = vectors();
        let mut sim = AmbienceSim::new(Effect::Rain, 2, 3.0);
        sim.set_view(2560.0, 1400.0);
        sim.set_colliders(&colliders(&v));
        for _ in 0..3600 {
            sim.step();
        }
        assert!(sim.parts.len() <= PARTICLE_CAP);
        for l in sim.ledges.values() {
            for &h in &l.h {
                assert!(h <= LEDGE_MAX + 1e-9);
            }
        }
    }
}
