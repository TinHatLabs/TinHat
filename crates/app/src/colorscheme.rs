//! Full-app theming via a generated KDE color scheme.
//!
//! Kirigami's own chrome (the global header, toolbars) follows the ACTIVE KColorScheme
//! through qqc2-desktop-style on Plasma — it ignores per-element `Kirigami.Theme`
//! overrides and the QtQuick `palette`. So to recolor the whole window (chrome
//! included) we generate one `.colors` scheme per Tin Hat theme into
//! `~/.local/share/color-schemes/` and activate it at runtime via `KColorSchemeManager`
//! (the C++ shim `tinhat_apply_color_scheme` in video_bridge.cpp). Our own drawn elements
//! (message bubbles) also bind the palette directly, so both halves stay in sync.
//!
//! themes (desktop C1): the scheme is written from the theme's resolved ROLES (theme_rt.rs → the engine):
//! surface.window, surface.island, surface.raised, text.primary, text.secondary and accent.base — the six colours the
//! 10.6 table carried. For OLED and Goose the files are byte-identical to 10.6 (a frozen copy of the old generator in
//! the tests proves it). Not written for them, as before (parity P46): ForegroundLink/Negative/Neutral/Positive and the
//! Background* accents; `[Colors:Selection] ForegroundNormal` stays white (P46 moves it to text.onAccent).
//!
//! themes (step 1b, contract HAZARDS.md H07): every OTHER theme gets the whole scheme from its roles — the same six
//! colours plus ForegroundLink/Visited = text.link, ForegroundNegative = state.dangerBright, ForegroundNeutral =
//! state.warning, ForegroundPositive = state.success in every group (KColorScheme tints the Background* accents from
//! them), and `[Colors:Selection] ForegroundNormal` = text.onAccent — so Kirigami's other colour sets (sheets, passive
//! notifications, InlineMessage) and the selection read on a light theme too. Its file is written when the theme is
//! applied, at any time: [`apply`] activates it from the file when KColorSchemeManager scanned the folder before it
//! existed (video_bridge.cpp `tinhat_apply_color_scheme_file`).

use std::path::PathBuf;

use crate::theme_rt::{self, Active};

/// The KColorScheme name for a theme (also the .colors filename stem + `[General] Name`): the 10.6 names for the
/// built-ins, `TinHat-<id>` for any other theme (a `:` of the id — allowed in theme ids, not in a Windows file name —
/// becomes `_`, which no id contains, so two ids never share a name).
pub fn scheme_name(theme: &str) -> String {
    match theme {
        "goose" => "TinHatGoose".to_string(),
        "oled" => "TinHatOLED".to_string(),
        id => format!("TinHat-{}", id.replace(':', "_")),
    }
}

/// OLED and Goose: the schemes 10.6 wrote, byte for byte.
fn is_builtin(id: &str) -> bool {
    id == "oled" || id == "goose"
}

/// A role as "r,g,b" (the KColorScheme ini format; it has no alpha).
fn rgb(a: &Active, role: &str) -> String {
    let c = a.colour(role).unwrap_or_else(|| panic!("theme '{}' has no role {role}", a.id));
    format!("{},{},{}", c.r, c.g, c.b)
}

fn schemes_dir() -> PathBuf {
    // KColorSchemeManager scans `<data base>/color-schemes` (platform-resolved base;
    // on Windows the manager is compiled out and these files are inert).
    crate::platform::data_base_dir().join("color-schemes")
}

fn colors_ini(a: &Active) -> String {
    if is_builtin(&a.id) {
        colors_ini_10_6(a)
    } else {
        colors_ini_full(a)
    }
}

/// Any theme but the built-ins (HAZARDS.md H07): the 10.6 layout + the accent foregrounds + the selection ink.
fn colors_ini_full(a: &Active) -> String {
    let name = scheme_name(&a.id);
    let (bg, panel, panel_hi, text, dim, accent) = (
        rgb(a, "surface.window"),
        rgb(a, "surface.island"),
        rgb(a, "surface.raised"),
        rgb(a, "text.primary"),
        rgb(a, "text.secondary"),
        rgb(a, "accent.base"),
    );
    let (link, negative, neutral, positive, on_accent) = (
        rgb(a, "text.link"),
        rgb(a, "state.dangerBright"),
        rgb(a, "state.warning"),
        rgb(a, "state.success"),
        rgb(a, "text.onAccent"),
    );
    let group = |bgc: &str, fgc: &str| {
        format!(
            "BackgroundNormal={bgc}\nBackgroundAlternate={panel_hi}\nForegroundNormal={fgc}\nForegroundInactive={dim}\n\
             ForegroundActive={accent}\nForegroundLink={link}\nForegroundVisited={link}\nForegroundNegative={negative}\n\
             ForegroundNeutral={neutral}\nForegroundPositive={positive}\nDecorationFocus={accent}\nDecorationHover={accent}\n"
        )
    };
    format!(
        "[General]\nName={name}\nColorScheme={name}\nshadeSortColumn=true\n\n\
         [Colors:Window]\n{win}\n\
         [Colors:View]\n{view}\n\
         [Colors:Button]\n{btn}\n\
         [Colors:Selection]\n{sel}\n\
         [Colors:Tooltip]\n{tip}\n\
         [Colors:Complementary]\n{comp}\n\
         [Colors:Header]\n{hdr}\n\
         [WM]\nactiveBackground={panel}\nactiveForeground={text}\ninactiveBackground={bg}\ninactiveForeground={dim}\n",
        win = group(&bg, &text),
        view = group(&bg, &text),
        btn = group(&panel, &text),
        sel = group(&accent, &on_accent),
        tip = group(&panel_hi, &text),
        comp = group(&bg, &text),
        hdr = group(&panel, &text),
    )
}

/// OLED and Goose: 10.6's six-colour scheme (frozen: builtin_schemes_are_byte_identical_to_10_6).
fn colors_ini_10_6(a: &Active) -> String {
    let name = scheme_name(&a.id);
    let (bg, panel, panel_hi, text, dim, accent) = (
        rgb(a, "surface.window"),
        rgb(a, "surface.island"),
        rgb(a, "surface.raised"),
        rgb(a, "text.primary"),
        rgb(a, "text.secondary"),
        rgb(a, "accent.base"),
    );
    // Set every group qqc2-desktop-style consults so no surface keeps the Breeze default.
    let group = |bgc: &str, fgc: &str| {
        format!(
            "BackgroundNormal={bgc}\nBackgroundAlternate={panel_hi}\nForegroundNormal={fgc}\nForegroundInactive={dim}\nForegroundActive={accent}\nDecorationFocus={accent}\nDecorationHover={accent}\n"
        )
    };
    format!(
        "[General]\nName={name}\nColorScheme={name}\nshadeSortColumn=true\n\n\
         [Colors:Window]\n{win}\n\
         [Colors:View]\n{view}\n\
         [Colors:Button]\n{btn}\n\
         [Colors:Selection]\nBackgroundNormal={accent}\nForegroundNormal=255,255,255\nDecorationFocus={accent}\nDecorationHover={accent}\n\n\
         [Colors:Tooltip]\nBackgroundNormal={panel_hi}\nForegroundNormal={text}\n\n\
         [Colors:Complementary]\n{comp}\n\
         [Colors:Header]\n{hdr}\n\
         [WM]\nactiveBackground={panel}\nactiveForeground={text}\ninactiveBackground={bg}\ninactiveForeground={dim}\n",
        name = name,
        accent = accent,
        text = text,
        panel = panel,
        bg = bg,
        dim = dim,
        win = group(&bg, &text),
        view = group(&bg, &text),
        btn = group(&panel, &text),
        comp = group(&bg, &text),
        hdr = group(&panel, &text),
    )
}

/// Write a theme's `.colors` file; its path (None when the folder cannot be written).
fn write_scheme(a: &Active) -> Option<PathBuf> {
    let dir = schemes_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!("{}.colors", scheme_name(&a.id)));
    // unchanged content is not rewritten (the file's mtime stays; nothing re-reads it for nothing)
    let text = colors_ini(a);
    if std::fs::read_to_string(&path).map(|old| old == text).unwrap_or(false) {
        return Some(path);
    }
    std::fs::write(&path, text).ok().map(|_| path)
}

/// Write the `.colors` files (idempotent): both built-ins, as 10.6 did, plus the theme the app starts with when it is
/// not built in (KColorSchemeManager scans the dir once, so that scheme is in its model from the start). Must run
/// BEFORE the first `KColorSchemeManager::instance()`.
pub fn ensure_color_schemes() {
    if std::fs::create_dir_all(schemes_dir()).is_err() {
        return;
    }
    let mut ids = vec!["oled".to_string(), "goose".to_string()];
    let start = crate::theme_store::start_theme();
    if !ids.contains(&start) && !cfg!(windows) {
        ids.push(start);
    }
    for id in ids {
        let a = theme_rt::resolve_in(&id, &theme_rt::themes_dir());
        let _ = write_scheme(&a);
    }
}

/// Activate the KDE color scheme for `theme` (recolors the whole Kirigami app); an unknown theme is OLED, like
/// everywhere else. The built-ins go through KColorSchemeManager by name as in 10.6; any other theme's scheme is
/// (re)written first and activated from its file — also one applied long after start (themes step 1b). Must be called
/// on the GUI thread (touches the app palette).
pub fn apply(theme: &str) {
    let cur = theme_rt::current();
    let a = if cur.id == theme { cur } else { theme_rt::resolve_in(theme, &theme_rt::themes_dir()) };
    let name = scheme_name(&a.id);
    if is_builtin(&a.id) || cfg!(windows) {
        crate::video::apply_color_scheme(&name);
        return;
    }
    match write_scheme(&a) {
        Some(path) => crate::video::apply_color_scheme_file(&name, &path),
        None => eprintln!("[theme] cannot write the color scheme of '{}'", a.id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 10.6.0 generator, frozen verbatim with its palette table: the runtime must write the same bytes.
    mod legacy {
        pub struct Pal {
            bg: &'static str,
            panel: &'static str,
            panel_hi: &'static str,
            text: &'static str,
            text_dim: &'static str,
            accent: &'static str,
        }

        pub fn pal(theme: &str) -> Pal {
            match theme {
                "goose" => Pal { bg: "#1a0a1a", panel: "#2a1030", panel_hi: "#351540", text: "#f0e0f0", text_dim: "#c8a0c8", accent: "#b0508c" },
                _ /* oled */ => Pal { bg: "#000000", panel: "#0a0a0a", panel_hi: "#1a1a1a", text: "#e0e0e0", text_dim: "#aaaaaa", accent: "#1e6a76" },
            }
        }

        pub fn scheme_name(theme: &str) -> &'static str {
            match theme {
                "goose" => "TinHatGoose",
                _ => "TinHatOLED",
            }
        }

        fn rgb(hex: &str) -> String {
            let h = hex.trim_start_matches('#');
            let p = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).unwrap_or(0);
            format!("{},{},{}", p(0), p(2), p(4))
        }

        pub fn colors_ini(theme: &str, p: &Pal) -> String {
            let name = scheme_name(theme);
            let (bg, panel, panel_hi, text, dim, accent) =
                (rgb(p.bg), rgb(p.panel), rgb(p.panel_hi), rgb(p.text), rgb(p.text_dim), rgb(p.accent));
            let group = |bgc: &str, fgc: &str| {
                format!(
                    "BackgroundNormal={bgc}\nBackgroundAlternate={panel_hi}\nForegroundNormal={fgc}\nForegroundInactive={dim}\nForegroundActive={accent}\nDecorationFocus={accent}\nDecorationHover={accent}\n"
                )
            };
            format!(
                "[General]\nName={name}\nColorScheme={name}\nshadeSortColumn=true\n\n\
                 [Colors:Window]\n{win}\n\
                 [Colors:View]\n{view}\n\
                 [Colors:Button]\n{btn}\n\
                 [Colors:Selection]\nBackgroundNormal={accent}\nForegroundNormal=255,255,255\nDecorationFocus={accent}\nDecorationHover={accent}\n\n\
                 [Colors:Tooltip]\nBackgroundNormal={panel_hi}\nForegroundNormal={text}\n\n\
                 [Colors:Complementary]\n{comp}\n\
                 [Colors:Header]\n{hdr}\n\
                 [WM]\nactiveBackground={panel}\nactiveForeground={text}\ninactiveBackground={bg}\ninactiveForeground={dim}\n",
                name = name,
                accent = accent,
                text = text,
                panel = panel,
                bg = bg,
                dim = dim,
                win = group(&bg, &text),
                view = group(&bg, &text),
                btn = group(&panel, &text),
                comp = group(&bg, &text),
                hdr = group(&panel, &text),
            )
        }
    }

    #[test]
    fn builtin_schemes_are_byte_identical_to_10_6() {
        for id in ["oled", "goose"] {
            let a = theme_rt::try_resolve_in(id, std::path::Path::new("/nonexistent")).unwrap();
            assert_eq!(colors_ini(&a), legacy::colors_ini(id, &legacy::pal(id)), "{id}");
            assert_eq!(scheme_name(id), legacy::scheme_name(id));
        }
    }

    #[test]
    fn other_themes_get_their_own_scheme_name() {
        assert_eq!(scheme_name("mocha"), "TinHat-mocha");
        assert_eq!(scheme_name("my:mocha.v2"), "TinHat-my_mocha.v2");
    }

    /// HAZARDS.md H07: a theme that is not built in gets the whole scheme from its roles.
    #[test]
    fn other_themes_get_the_whole_scheme() {
        let dir = std::env::temp_dir().join(format!("tinhat-colorscheme-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut doc = crate::theme_lib::doc("catppuccin-latte").unwrap().clone();
        doc["id"] = serde_json::Value::String("latte".into());
        std::fs::write(dir.join("latte.json"), doc.to_string()).unwrap();
        let a = theme_rt::try_resolve_in("latte", &dir).unwrap();
        let ini = colors_ini(&a);
        let _ = std::fs::remove_dir_all(&dir);
        let c = |id: &str| rgb(&a, id);
        assert!(ini.starts_with("[General]\nName=TinHat-latte\nColorScheme=TinHat-latte\n"));
        for group in ["Window", "View", "Button", "Selection", "Tooltip", "Complementary", "Header"] {
            let body = ini.split(&format!("[Colors:{group}]\n")).nth(1).unwrap().split("\n\n").next().unwrap();
            for (key, id) in [
                ("ForegroundLink", "text.link"),
                ("ForegroundVisited", "text.link"),
                ("ForegroundNegative", "state.dangerBright"),
                ("ForegroundNeutral", "state.warning"),
                ("ForegroundPositive", "state.success"),
            ] {
                assert!(body.contains(&format!("\n{key}={}\n", c(id))), "{group}: {key}");
            }
            let fg = if group == "Selection" { c("text.onAccent") } else { c("text.primary") };
            assert!(body.contains(&format!("\nForegroundNormal={fg}\n")), "{group}: ForegroundNormal");
        }
        assert!(ini.contains(&format!("[Colors:Window]\nBackgroundNormal={}\n", c("surface.window"))));
        assert!(!ini.contains("255,255,255\n") || c("text.onAccent") == "255,255,255");
    }
}
