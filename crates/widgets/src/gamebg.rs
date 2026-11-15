//! gamebg — "Match the game" chat backgrounds (lane sun-gamebg, 2026-11-14), the pure half: a line-for-line port of
//! Tin Hat Web's `src/features/chat/gamebg/model.ts`, proven identical by `testvectors/gamebg/vectors.json` (byte copy;
//! the same case names). The server's `api/games/background.mjs` replays the `gameKey` cases too.
//!
//! - [`game_key`]: the comparable form of a status text / game name ("Playing Valheim" → "valheim").
//! - [`pick_game`]: which game a room shows: the shared statuses of its joined members + the room's Now playing; call
//!   players first; ties → the most recent, then the smaller id; nobody → None.
//! - [`step_stable`]: at most one change per [`HOLD_MS`], a candidate must last [`SETTLE_MS`], the first look at once.
//! - [`veil_for`]: the smallest theme-colour veil keeping the timeline's text colours at the target contrast over every
//!   measured art cell; [`bound_pixels`] gives the cells that bound every pixel of an art.
//! - [`art_mode`]: whether game art applies (automatic where nothing is set; `kind:"game"` asks; any wallpaper wins).
//! - [`fit_art`]: cover, top-anchored, never past one art pixel per device pixel.
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};

pub const PREF_GAME_ART: &str = "chat-game-art";
pub const SETTLE_MS: f64 = 60_000.0;
pub const HOLD_MS: f64 = 180_000.0;
/// Game art's size caps, shared with the web (model.ts; vectors `constants` + `art_size`): at most 16 MP (a 3840 × 1240
/// Steam hero is 4.8 MP) and at most 8192 px on a side, below common GPU texture maxima (R5).
pub const ART_MAX_PIXELS: u64 = 16_000_000;
pub const ART_MAX_SIDE: u32 = 8192;
/// Game art of this size may be decoded and shown (the one rule both apps share; the header's size is checked first).
pub fn art_dims_ok(w: u32, h: u32) -> bool {
    w > 0 && h > 0 && w <= ART_MAX_SIDE && h <= ART_MAX_SIDE && (w as u64) * (h as u64) <= ART_MAX_PIXELS
}
pub const FADE_MS: u32 = 2400;
pub const VEIL_FLOOR_DARK: f64 = 0.4;
pub const VEIL_FLOOR_OLED: f64 = 0.5;
pub const VEIL_FLOOR_LIGHT: f64 = 0.45;
/// the most veil per theme kind (web model.ts VEIL_CAP): light themes go further, hero art is mostly dark
pub const VEIL_CAP_DARK: f64 = 0.92;
pub const VEIL_CAP_OLED: f64 = 0.92;
pub const VEIL_CAP_LIGHT: f64 = 0.96;
pub const MIN_CONTRAST: f64 = 4.5;
/// the art is measured at FULL size into bound cells; the veil is fitted to FIT_CONTRAST (a little over the 4.5:1
/// required: resampling can overshoot the source pixels)
pub const FIT_CONTRAST: f64 = 4.6;
const NAME_MAX: usize = 64;

// ---- names ---------------------------------------------------------------------------------------------------------
fn is_ws(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{0c}' | '\u{0b}' | '\u{a0}')
}
/// The comparable form of a game name or a status text (vectors "gameKey"). Not a string → "" (callers pass `&str`;
/// [`game_key_value`] takes JSON).
pub fn game_key(s: &str) -> String {
    let mut out = String::new();
    let mut pend = false;
    for ch in s.chars() {
        if matches!(ch, '\u{2122}' | '\u{ae}' | '\u{a9}') {
            continue;
        }
        if is_ws(ch) {
            pend = !out.is_empty();
            continue;
        }
        if pend {
            out.push(' ');
            pend = false;
        }
        out.push(if ch.is_ascii_uppercase() { ch.to_ascii_lowercase() } else { ch });
    }
    for p in ["now playing", "playing"] {
        if let Some(rest) = out.strip_prefix(p) {
            // /^(?: *[:-] *| +)/
            let b = rest.as_bytes();
            let mut i = 0;
            while i < b.len() && b[i] == b' ' {
                i += 1;
            }
            let used = if i < b.len() && (b[i] == b':' || b[i] == b'-') {
                i += 1;
                while i < b.len() && b[i] == b' ' {
                    i += 1;
                }
                i
            } else {
                i
            };
            if used > 0 && rest.len() > used {
                out = rest[used..].to_string();
                break;
            }
        }
    }
    let t = out.trim_end_matches(['.', '!']).trim_end_matches(' ');
    if t.chars().count() > NAME_MAX {
        String::new()
    } else {
        t.to_string()
    }
}
pub fn game_key_value(v: &Value) -> String {
    v.as_str().map(game_key).unwrap_or_default()
}

// ---- picking -------------------------------------------------------------------------------------------------------
#[derive(Clone, Debug, PartialEq)]
pub struct Claim {
    pub user: String,
    pub text: String,
    pub at: f64,
}
#[derive(Clone, Debug, Default)]
pub struct PickIn {
    pub members: Vec<String>,
    pub callers: Vec<String>,
    pub claims: Vec<Claim>,
    pub activity: Option<(String, f64)>,
    /// gameKey → catalogue id (None = not a game)
    pub ids: BTreeMap<String, Option<String>>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickOut {
    pub game: Option<String>,
    pub why: &'static str,
}
/// JavaScript's string `<` (UTF-16 code units), for byte-identical tie-breaks with the web.
fn utf16_lt(a: &str, b: &str) -> bool {
    a.encode_utf16().lt(b.encode_utf16())
}
fn winner(votes: &[(String, f64)]) -> String {
    let mut g: Vec<(String, u32, f64)> = Vec::new();
    for (id, at) in votes {
        if let Some(e) = g.iter_mut().find(|e| &e.0 == id) {
            e.1 += 1;
            e.2 = e.2.max(*at);
        } else {
            g.push((id.clone(), 1, *at));
        }
    }
    let mut best = g[0].clone();
    for e in g.into_iter().skip(1) {
        if e.1 > best.1 || (e.1 == best.1 && (e.2 > best.2 || (e.2 == best.2 && utf16_lt(&e.0, &best.0)))) {
            best = e;
        }
    }
    best.0
}
pub fn pick_game(p: &PickIn) -> PickOut {
    let members: HashSet<&str> = p.members.iter().map(String::as_str).collect();
    let id_of = |text: &str| -> Option<String> {
        let k = game_key(text);
        if k.is_empty() {
            return None;
        }
        p.ids.get(&k).cloned().flatten().filter(|s| !s.is_empty())
    };
    let mut own: Vec<(String, (String, f64))> = Vec::new();
    for c in &p.claims {
        if !members.contains(c.user.as_str()) {
            continue;
        }
        let Some(id) = id_of(&c.text) else { continue };
        match own.iter_mut().find(|(u, _)| u == &c.user) {
            Some((_, cur)) => {
                if c.at > cur.1 || (c.at == cur.1 && utf16_lt(&id, &cur.0)) {
                    *cur = (id, c.at);
                }
            }
            None => own.push((c.user.clone(), (id, c.at))),
        }
    }
    let act = p.activity.as_ref().and_then(|(t, at)| id_of(t).map(|id| (id, *at)));
    let mut callers: Vec<&str> = Vec::new();
    for u in &p.callers {
        if members.contains(u.as_str()) && !callers.contains(&u.as_str()) {
            callers.push(u);
        }
    }
    if !callers.is_empty() {
        let votes: Vec<(String, f64)> =
            callers.iter().filter_map(|u| own.iter().find(|(o, _)| o == u).map(|(_, v)| v.clone()).or_else(|| act.clone())).collect();
        if !votes.is_empty() {
            return PickOut { game: Some(winner(&votes)), why: "call" };
        }
    }
    let mut votes: Vec<(String, f64)> = own.into_iter().map(|(_, v)| v).collect();
    if let Some(a) = act {
        votes.push(a);
    }
    if !votes.is_empty() {
        return PickOut { game: Some(winner(&votes)), why: "room" };
    }
    PickOut { game: None, why: "none" }
}

// ---- no flicker ----------------------------------------------------------------------------------------------------
#[derive(Clone, Debug, PartialEq)]
pub struct Stable {
    pub shown: Option<String>,
    pub since: f64,
    pub cand: Option<String>,
    pub cand_since: f64,
}
pub fn step_stable(st: Option<&Stable>, cand: Option<&str>, now: f64) -> Stable {
    let Some(st) = st else {
        // a first look of nobody starts no hold: the first game that appears shows after the settle
        let since = if cand.is_some() { now } else { f64::NEG_INFINITY };
        return Stable { shown: cand.map(str::to_string), since, cand: cand.map(str::to_string), cand_since: now };
    };
    let mut s = st.clone();
    if cand != s.cand.as_deref() {
        s.cand = cand.map(str::to_string);
        s.cand_since = now;
    }
    if s.cand != s.shown && now - s.cand_since >= SETTLE_MS && now - s.since >= HOLD_MS {
        s.shown = s.cand.clone();
        s.since = now;
    }
    s
}
pub fn next_step_in(st: Option<&Stable>, now: f64) -> Option<f64> {
    let st = st?;
    if st.cand == st.shown {
        return None;
    }
    Some(0f64.max(st.cand_since + SETTLE_MS - now).max(st.since + HOLD_MS - now))
}

// ---- contrast ------------------------------------------------------------------------------------------------------
pub type Rgb = [f64; 3];
pub fn parse_hex(s: &str) -> Option<Rgb> {
    // ASCII-only trimming, as the web's model.ts (String.prototype.trim would also take U+FEFF etc.)
    let s = s.trim_matches(|c| matches!(c, ' ' | '\t' | '\n' | '\r'));
    let h = s.strip_prefix('#')?;
    if h.len() != 6 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let n = u32::from_str_radix(h, 16).ok()?;
    Some([((n >> 16) & 255) as f64, ((n >> 8) & 255) as f64, (n & 255) as f64])
}
fn lin(c: f64) -> f64 {
    let x = c / 255.0;
    if x <= 0.04045 {
        x / 12.92
    } else {
        ((x + 0.055) / 1.055).powf(2.4)
    }
}
pub fn luminance(c: Rgb) -> f64 {
    0.2126 * lin(c[0]) + 0.7152 * lin(c[1]) + 0.0722 * lin(c[2])
}
pub fn contrast(a: Rgb, b: Rgb) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}
pub struct VeilTheme<'a> {
    pub bg: &'a str,
    /// the text colours drawn straight on the background (timeline texts + people's name colours)
    pub texts: &'a [String],
    /// the contrast every text must keep (4.5); a text counts when it already has it on the plain theme
    pub need: f64,
    /// the contrast the veil is fitted to (resampling headroom); falls back to `need`
    pub target: f64,
    pub floor: f64,
    pub cap: f64,
}
fn contrast_l(a: f64, b: f64) -> f64 {
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}
/// The veil's text list (3-2): every colour drawn straight on the background — the timeline's own texts, the theme's
/// name colours AND the room's role name colours — as unique lower-case "#rrggbb" in order; anything else is dropped
/// (callers resolve colours to sRGB hex first). Shared with the web (model.ts inkTexts; vectors "ink").
pub fn ink_texts(own: &[String], names: &[String], roles: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for x in own.iter().chain(names).chain(roles) {
        let ok = x.len() == 7 && x.starts_with('#') && x[1..].chars().all(|c| c.is_ascii_hexdigit());
        let h = x.to_ascii_lowercase();
        if ok && !out.contains(&h) {
            out.push(h);
        }
    }
    out
}
/// The smallest veil (steps of 0.01 in [floor, cap]) with every counted text at ≥ target over every cell mixed toward
/// the theme background; else the smallest reaching `need`; else None = draw no art. (web model.ts veilFor)
pub fn veil_for(t: &VeilTheme, cells: &[Rgb]) -> Option<f64> {
    let lo = js_round(t.floor * 100.0);
    let hi = js_round(t.cap * 100.0);
    let bg = parse_hex(t.bg)?;
    let mut tl: Vec<f64> = Vec::new();
    for x in t.texts {
        if let Some(c) = parse_hex(x) {
            if contrast(c, bg) >= t.need {
                let l = luminance(c);
                if !tl.contains(&l) {
                    tl.push(l);
                }
            }
        }
    }
    tl.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    if tl.is_empty() {
        return Some(lo as f64 / 100.0);
    }
    let ok = |a: f64, want: f64| -> bool {
        cells.iter().all(|c| {
            let l = luminance([a * bg[0] + (1.0 - a) * c[0], a * bg[1] + (1.0 - a) * c[1], a * bg[2] + (1.0 - a) * c[2]]);
            let i = tl.iter().take_while(|&&x| x < l).count(); // bisect_left
            !(i > 0 && contrast_l(tl[i - 1], l) < want) && !(i < tl.len() && contrast_l(tl[i], l) < want)
        })
    };
    // a veil counts only if every STRONGER veil up to fully opaque is safe too (a fade through the theme)
    for want in [t.target, t.need] {
        let good: Vec<bool> = (lo..=100).map(|k| ok(k as f64 / 100.0, want)).collect();
        for k in lo..=hi {
            if good[(k - lo) as usize..].iter().all(|&g| g) {
                return Some(k as f64 / 100.0);
            }
        }
    }
    None
}
/// ECMAScript Math.round (half toward +∞) for the floor/cap steps.
fn js_round(x: f64) -> i64 {
    (x + 0.5).floor() as i64
}
/// The cells veil_for measures for an art `width` px wide (web model.ts boundPixels): every pixel's 5×5 neighbourhood
/// (clamped) gives its per-channel min and max colour; per occupied (r, g) bucket of 4 levels, the upper corner with the
/// highest blue, then (all buckets again) the lower corner with the lowest. Bounds every pixel and every nearby average.
pub fn bound_pixels(rgba: &[u8], width: usize) -> Vec<Rgb> {
    let w = width;
    let h = if w > 0 { (rgba.len() / 4) / w } else { 0 };
    let mut mx = vec![-1i16; 4096];
    let mut mn = vec![256i16; 4096];
    let mut put = |r: u8, g: u8, b: u8| {
        let k = (r as usize >> 2) * 64 + (g as usize >> 2);
        mx[k] = mx[k].max(b as i16);
        mn[k] = mn[k].min(b as i16);
    };
    if w > 0 && h > 0 {
        let mut hlo = vec![0u8; w * h * 3];
        let mut hhi = vec![0u8; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                for c in 0..3 {
                    let (mut lo, mut hi) = (255u8, 0u8);
                    for xx in x.saturating_sub(2)..=(x + 2).min(w - 1) {
                        let v = rgba[(y * w + xx) * 4 + c];
                        lo = lo.min(v);
                        hi = hi.max(v);
                    }
                    hlo[(y * w + x) * 3 + c] = lo;
                    hhi[(y * w + x) * 3 + c] = hi;
                }
            }
        }
        for y in 0..h {
            for x in 0..w {
                let (mut lo, mut hi) = ([255u8; 3], [0u8; 3]);
                for c in 0..3 {
                    for yy in y.saturating_sub(2)..=(y + 2).min(h - 1) {
                        let i = (yy * w + x) * 3 + c;
                        lo[c] = lo[c].min(hlo[i]);
                        hi[c] = hi[c].max(hhi[i]);
                    }
                }
                put(lo[0], lo[1], lo[2]);
                put(hi[0], hi[1], hi[2]);
            }
        }
    }
    let mut up = Vec::new();
    let mut dn = Vec::new();
    for k in 0..4096usize {
        if mx[k] < 0 {
            continue;
        }
        up.push([((k >> 6) * 4 + 3) as f64, ((k & 63) * 4 + 3) as f64, mx[k] as f64]);
        dn.push([((k >> 6) * 4) as f64, ((k & 63) * 4) as f64, mn[k] as f64]);
    }
    up.extend(dn);
    up
}
/// The distinct pixels of an RGBA buffer, in first-seen order (small inputs / tests).
pub fn pixels_of(rgba: &[u8]) -> Vec<Rgb> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for p in rgba.chunks_exact(4) {
        let key = (p[0] as u32) << 16 | (p[1] as u32) << 8 | p[2] as u32;
        if seen.insert(key) {
            out.push([p[0] as f64, p[1] as f64, p[2] as f64]);
        }
    }
    out
}

// ---- does game art apply -------------------------------------------------------------------------------------------
pub fn nothing_set(c: Option<&Value>) -> bool {
    match c {
        Some(Value::Object(m)) => m.is_empty(),
        _ => true,
    }
}
/// "game" | "off" (vectors "mode"). `room` / `space` = the `dev.tinhat.chat.background` contents (None = none).
pub fn art_mode(room: Option<&Value>, space: Option<&Value>, backgrounds: bool, game_art: bool) -> &'static str {
    if !backgrounds || !game_art {
        return "off";
    }
    // THE one parse rule (ambience parse_background, 3-1): game art iff the background is unset or "game"; the room's
    // own state decides first, the space's only when the room has nothing set
    use crate::ambience::{parse_background, BackgroundKind};
    let kind = |c: Option<&Value>| parse_background(c.unwrap_or(&Value::Null), "").kind;
    let k = if kind(room) == BackgroundKind::Unset { kind(space) } else { kind(room) };
    if k == BackgroundKind::Unset || k == BackgroundKind::Game {
        "game"
    } else {
        "off"
    }
}
pub fn fallback_preset(c: Option<&Value>) -> Option<String> {
    if nothing_set(c) {
        return None;
    }
    let c = c?;
    let v1 = c.get("v").and_then(Value::as_f64) == Some(1.0);
    if v1 && c.get("kind").and_then(Value::as_str) == Some("game") {
        c.get("preset").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string)
    } else {
        None
    }
}
/// The background sheet's "Match the game" (web model.ts matchGamePlan, vectors "sheet"): ("skip", None) where game art
/// already shows (the room asks for it, or nothing is set and art is showing), else ("write", {v:1, kind:"game"}).
pub fn match_game_plan(bg_kind: &str, art_showing: bool) -> (&'static str, Option<Value>) {
    if bg_kind == "game" || (bg_kind == "unset" && art_showing) {
        ("skip", None)
    } else {
        ("write", Some(serde_json::json!({"v": 1, "kind": "game"})))
    }
}
/// The art url on the person's own server (`encodeURIComponent` of the id).
pub fn background_url(catalog_base: &str, id: &str) -> String {
    let mut enc = String::new();
    for b in id.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            enc.push(b as char);
        } else {
            enc.push_str(&format!("%{b:02X}"));
        }
    }
    format!("{catalog_base}/art/{enc}/background")
}

// ---- drawing without upscaling --------------------------------------------------------------------------------------
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fit {
    pub w: f64,
    pub h: f64,
    pub x: f64,
    pub y: f64,
    pub fade_bottom: bool,
    pub fade_sides: bool,
}
pub fn fit_art(art_w: f64, art_h: f64, area_w: f64, area_h: f64, dpr: f64) -> Fit {
    if !(art_w > 0.0 && art_h > 0.0 && area_w > 0.0 && area_h > 0.0) {
        return Fit { w: 0.0, h: 0.0, x: 0.0, y: 0.0, fade_bottom: false, fade_sides: false };
    }
    let d = if dpr > 0.0 { dpr } else { 1.0 };
    let cover = (area_w / art_w).max(area_h / art_h);
    let s = cover.min(1.0 / d);
    let (w, h) = (art_w * s, art_h * s);
    Fit { w, h, x: (area_w - w) / 2.0, y: 0.0, fade_bottom: h < area_h - 0.5, fade_sides: w < area_w - 0.5 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vectors() -> Value {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../testvectors/gamebg/vectors.json");
        serde_json::from_str(&std::fs::read_to_string(path).expect("vectors")).expect("json")
    }
    fn strs(v: &Value) -> Vec<String> {
        v.as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect()
    }
    fn pick_in(c: &Value) -> PickIn {
        PickIn {
            members: strs(&c["members"]),
            callers: strs(&c["callers"]),
            claims: c["claims"]
                .as_array()
                .unwrap()
                .iter()
                .map(|x| Claim { user: x["user"].as_str().unwrap().into(), text: x["text"].as_str().unwrap().into(), at: x["at"].as_f64().unwrap() })
                .collect(),
            activity: c["activity"].as_object().map(|a| (a["text"].as_str().unwrap().to_string(), a["at"].as_f64().unwrap())),
            ids: c["ids"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), v.as_str().map(str::to_string))).collect(),
        }
    }

    #[test]
    fn gamebg_constants() {
        let v = vectors();
        assert_eq!(SETTLE_MS, v["constants"]["SETTLE_MS"].as_f64().unwrap());
        assert_eq!(HOLD_MS, v["constants"]["HOLD_MS"].as_f64().unwrap());
        assert_eq!(ART_MAX_PIXELS, v["constants"]["ART_MAX_PIXELS"].as_u64().unwrap());
        assert_eq!(ART_MAX_SIDE as u64, v["constants"]["ART_MAX_SIDE"].as_u64().unwrap());
        assert_eq!(PREF_GAME_ART, "chat-game-art");
    }
    #[test]
    fn gamebg_art_size() {
        // R5: game art's size rule (both apps, from the header's size): ≤ 8192 px a side AND ≤ 16 MP
        let v = vectors();
        let cases = v["art_size"].as_array().unwrap();
        assert!(cases.len() >= 12);
        for c in cases {
            let (w, h) = (c["w"].as_u64().unwrap() as u32, c["h"].as_u64().unwrap() as u32);
            assert_eq!(art_dims_ok(w, h), c["ok"].as_bool().unwrap(), "{}", c["name"]);
        }
        assert!(!art_dims_ok(8193, 100), "an 8193 × 100 header: no art");
    }
    #[test]
    fn gamebg_game_key() {
        let v = vectors();
        let cases = v["gameKey"].as_array().unwrap();
        assert!(cases.len() >= 20);
        for c in cases {
            assert_eq!(game_key_value(&c["in"]), c["out"].as_str().unwrap(), "{}", c["in"]);
        }
    }
    #[test]
    fn gamebg_pick() {
        let v = vectors();
        let cases = v["pick"].as_array().unwrap();
        assert!(cases.len() >= 15);
        for c in cases {
            let got = pick_game(&pick_in(c));
            assert_eq!(got.game.as_deref(), c["out"]["game"].as_str(), "{}", c["name"]);
            assert_eq!(got.why, c["out"]["why"].as_str().unwrap(), "{}", c["name"]);
        }
    }
    #[test]
    fn gamebg_stabilise() {
        let v = vectors();
        for c in v["stabilise"].as_array().unwrap() {
            let mut st: Option<Stable> = None;
            for s in c["steps"].as_array().unwrap() {
                let next = step_stable(st.as_ref(), s["cand"].as_str(), s["now"].as_f64().unwrap());
                assert_eq!(next.shown.as_deref(), s["shown"].as_str(), "{} @{}", c["name"], s["now"]);
                st = Some(next);
            }
        }
    }
    #[test]
    fn gamebg_next_step_in() {
        let st = step_stable(None, Some("A"), 0.0);
        assert_eq!(next_step_in(Some(&st), 0.0), None);
        let st = step_stable(Some(&st), Some("B"), 10_000.0);
        assert_eq!(next_step_in(Some(&st), 10_000.0), Some(HOLD_MS - 10_000.0));
        let st = step_stable(Some(&st), Some("B"), 200_000.0);
        assert_eq!(st.shown.as_deref(), Some("B"));
        assert_eq!(next_step_in(Some(&st), 200_000.0), None);
    }
    #[test]
    fn gamebg_ink_measures_role_names() {
        // 3-2: a role (custom) name colour is drawn on the background too, so the veil measures it. The shared case: a
        // #808080 role name on white art under OLED needs 0.92; without it the veil settles at 0.90 (≈ 4.4:1 for that name).
        let v = vectors();
        let cases = v["ink"].as_array().unwrap();
        assert!(cases.len() >= 4);
        for c in cases {
            let name = c["name"].as_str().unwrap();
            let raw = |k: &str| -> Vec<String> { c[k].as_array().unwrap().iter().map(|x| x.as_str().unwrap_or("").to_string()).collect() };
            let texts = ink_texts(&raw("own"), &raw("names"), &raw("roles"));
            assert_eq!(texts, strs(&c["out"]["texts"]), "{name}");
            let t = &c["theme"];
            let theme = VeilTheme {
                bg: t["bg"].as_str().unwrap(),
                texts: &texts,
                need: t["need"].as_f64().unwrap(),
                target: t["target"].as_f64().unwrap(),
                floor: t["floor"].as_f64().unwrap(),
                cap: t["cap"].as_f64().unwrap(),
            };
            let cells: Vec<Rgb> = strs(&c["cells"]).iter().map(|x| parse_hex(x).unwrap()).collect();
            let got = veil_for(&theme, &cells);
            assert_eq!(got, c["out"]["veil"].as_f64(), "{name}");
            let (Some(a), Some(bg)) = (got, parse_hex(theme.bg)) else { continue };
            for r in raw("roles") {
                let Some(tx) = parse_hex(&r) else { continue };
                if contrast(tx, bg) < theme.need {
                    continue;
                }
                for cell in &cells {
                    let m: Rgb = [0, 1, 2].map(|i| a * bg[i] + (1.0 - a) * cell[i]);
                    assert!(contrast(tx, m) >= theme.need, "{name}: role {r} at veil {a} = {}", contrast(tx, m));
                }
            }
        }
    }
    #[test]
    fn gamebg_veil() {
        let v = vectors();
        for c in v["veil"].as_array().unwrap() {
            let t = &c["theme"];
            let texts = strs(&t["texts"]);
            let theme = VeilTheme {
                bg: t["bg"].as_str().unwrap(),
                texts: &texts,
                need: t["need"].as_f64().unwrap(),
                target: t["target"].as_f64().unwrap(),
                floor: t["floor"].as_f64().unwrap(),
                cap: t["cap"].as_f64().unwrap(),
            };
            let cells: Vec<Rgb> = match c.get("cells") {
                Some(cs) => strs(cs).iter().map(|h| parse_hex(h).unwrap()).collect(),
                None => bound_pixels(&c["rgba"].as_array().unwrap().iter().map(|x| x.as_u64().unwrap() as u8).collect::<Vec<u8>>(), c["width"].as_u64().unwrap() as usize),
            };
            assert_eq!(veil_for(&theme, &cells), c["out"].as_f64(), "{}", c["name"]);
        }
    }
    #[test]
    fn gamebg_bound() {
        let v = vectors();
        for c in v["bound"].as_array().unwrap() {
            let rgba: Vec<u8> = c["rgba"].as_array().unwrap().iter().map(|x| x.as_u64().unwrap() as u8).collect();
            let want: Vec<Rgb> = c["out"].as_array().unwrap().iter().map(|p| {
                let a = p.as_array().unwrap();
                [a[0].as_f64().unwrap(), a[1].as_f64().unwrap(), a[2].as_f64().unwrap()]
            }).collect();
            assert_eq!(bound_pixels(&rgba, c["width"].as_u64().unwrap() as usize), want, "{}", c["name"]);
        }
    }
    #[test]
    fn gamebg_bound_holds_for_every_pixel() {
        let mut seed: u32 = 7;
        let mut rnd = || -> u32 {
            seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
            (seed >> 8) % 256
        };
        for run in 0..40 {
            let mut rgba = Vec::new();
            for _ in 0..400 {
                let (r, g, b) = (rnd(), rnd(), rnd());
                rgba.extend_from_slice(&[r as u8, g as u8, b as u8, 255]);
            }
            let hex = |r: u32, g: u32, b: u32| format!("#{r:02x}{g:02x}{b:02x}");
            let dark = run % 2 == 0;
            let bg = if dark { hex(rnd() >> 3, rnd() >> 3, rnd() >> 2) } else { hex(255 - (rnd() >> 4), 255 - (rnd() >> 4), 255 - (rnd() >> 4)) };
            let tx = if dark { hex(150 + (rnd() >> 2), 150 + (rnd() >> 2), 150 + (rnd() >> 2)) } else { hex(rnd() >> 2, rnd() >> 2, rnd() >> 2) };
            let texts = vec![tx.clone()];
            let theme = VeilTheme { bg: &bg, texts: &texts, need: 4.5, target: 4.6, floor: 0.4, cap: 0.97 };
            let Some(v) = veil_for(&theme, &bound_pixels(&rgba, 20)) else { continue };
            let (bgc, txc) = (parse_hex(&bg).unwrap(), parse_hex(&tx).unwrap());
            if contrast(txc, bgc) < 4.5 {
                continue;
            }
            // every pixel, and every average of two horizontal neighbours (what a 2:1 downscale draws)
            let px: Vec<Rgb> = rgba.chunks_exact(4).map(|p| [p[0] as f64, p[1] as f64, p[2] as f64]).collect();
            let avg: Vec<Rgb> = (0..399).filter(|i| (i + 1) % 20 != 0).map(|i| [0, 1, 2].map(|c| (px[i][c] + px[i + 1][c]) / 2.0)).collect();
            for p in px.iter().chain(avg.iter()) {
                let m = [v * bgc[0] + (1.0 - v) * p[0], v * bgc[1] + (1.0 - v) * p[1], v * bgc[2] + (1.0 - v) * p[2]];
                assert!(contrast(txc, m) >= 4.5, "run {run} colour {p:?} veil {v}");
            }
        }
    }
    #[test]
    fn gamebg_parse_one_rule() {
        // 3-1: the gamebg vectors pin the ONE parse rule both apps (and the server's copy) share
        let v = vectors();
        let cases = v["parse"].as_array().unwrap();
        assert!(cases.len() >= 20);
        for c in cases {
            let got = crate::ambience::parse_background(c.get("content").unwrap_or(&Value::Null), "@a:x").kind;
            assert_eq!(got.as_str(), c["kind"].as_str().unwrap(), "{}", c["name"]);
        }
    }
    #[test]
    fn gamebg_mode() {
        let v = vectors();
        for c in v["mode"].as_array().unwrap() {
            let p = &c["prefs"];
            let got = art_mode(c.get("room"), c.get("space"), p["backgrounds"].as_bool().unwrap(), p["gameArt"].as_bool().unwrap());
            assert_eq!(got, c["out"].as_str().unwrap(), "{}", c["name"]);
        }
        assert_eq!(fallback_preset(Some(&serde_json::json!({"v": 1, "kind": "game", "preset": "rain"}))).as_deref(), Some("rain"));
        assert_eq!(fallback_preset(Some(&serde_json::json!({"v": 1, "preset": "rain"}))), None);
        assert_eq!(fallback_preset(None), None);
    }
    #[test]
    fn gamebg_fit() {
        let v = vectors();
        for c in v["fit"].as_array().unwrap() {
            let a: Vec<f64> = c["in"].as_array().unwrap().iter().map(|x| x.as_f64().unwrap()).collect();
            let got = fit_art(a[0], a[1], a[2], a[3], a[4]);
            let o = &c["out"];
            for (k, g) in [("w", got.w), ("h", got.h), ("x", got.x), ("y", got.y)] {
                assert!((g - o[k].as_f64().unwrap()).abs() < 1e-6, "{} {k}", c["name"]);
            }
            assert_eq!(got.fade_bottom, o["fadeBottom"].as_bool().unwrap(), "{}", c["name"]);
            assert_eq!(got.fade_sides, o["fadeSides"].as_bool().unwrap(), "{}", c["name"]);
            if got.w > 0.0 {
                assert!(got.w * a[4] / a[0] <= 1.0 + 1e-9, "{}: no upscaling", c["name"]);
            }
        }
    }
    #[test]
    fn gamebg_words_are_the_shared_words() {
        // the app reads the same file with include_str! (gamebg_ctl.rs WORDS); the web's words.ts is tested against it
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../testvectors/gamebg/words.json");
        let w: Value = serde_json::from_str(&std::fs::read_to_string(path).expect("words")).expect("json");
        for k in ["gameArt", "gameArtHelp", "matchGame", "matchGameHelp"] {
            assert!(!w[k].as_str().unwrap_or("").is_empty(), "{k}");
        }
        assert_eq!(w["gameArt"], "Game art backgrounds");
        assert_eq!(w["matchGame"], "Match the game");
    }
    #[test]
    fn gamebg_sheet_match_game() {
        let v = vectors();
        let cases = v["sheet"].as_array().unwrap();
        assert!(cases.len() >= 7);
        for c in cases {
            let (plan, content) = match_game_plan(c["bgKind"].as_str().unwrap(), c["artShowing"].as_bool().unwrap());
            assert_eq!(plan, c["out"]["plan"].as_str().unwrap(), "{}", c["name"]);
            assert_eq!(content.unwrap_or(Value::Null), c["out"]["content"], "{}", c["name"]);
        }
    }
    #[test]
    fn gamebg_pixels_and_url() {
        assert_eq!(pixels_of(&[255, 0, 0, 255, 255, 0, 0, 255, 0, 0, 255, 128]), vec![[255.0, 0.0, 0.0], [0.0, 0.0, 255.0]]);
        assert_eq!(background_url("https://h.example/api/games", "steam:892970"), "https://h.example/api/games/art/steam%3A892970/background");
    }
}
