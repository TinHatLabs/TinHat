//! theme_gen.rs — the shape of the `Th` QML singleton (desktop C1; docs/MIGRATING-THEME.md), shared by `build.rs`,
//! which writes `qml/Th.qml` from `crates/theme/tokens.json` and the built-in OLED theme, and by the app
//! (`theme_ctl.rs`), which fills Th with the active theme at run time. Pure: std + serde_json + tinhat_theme.
//!
//! Th carries one `property color` per ROLE and per element TOKEN of tokens.json (every one — see `entries`), named in
//! lowerCamel after the id (`composer.border.focus` → `composerBorderFocus`, `state.dangerBright` →
//! `stateDangerBright`). Two ids that land on one name, a name Th itself uses, or a malformed id fail the build.

use serde_json::Value;
use tinhat_theme::{Colour, Resolved};

/// Where the generated singleton lives (relative to crates/app; registered as `singleton Th` in build.rs).
pub const TH_FILE: &str = "qml/Th.qml";

/// Names the singleton itself uses — never a colour's name.
pub const RESERVED: [&str; 24] = [
    "themeId", "themeName", "variant", "names", "revision", "apply", "byId", "colourRef",
    "objectName", "th", "toString", "shapeRadius", "shapeGap", "shapeMargin", "shapeBorder",
    "fontUi", "fontChat", "fontCode", "textSize", "shapeShadow", "shapeDensity", "applyColours",
    "textScale", "fontMono",
];

/// One colour of Th.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Role or token id (`composer.border.focus`).
    pub id: String,
    /// Th property name (`composerBorderFocus`).
    pub name: String,
    /// A role (true) or an element token (false).
    pub role: bool,
    /// The token's default expression from tokens.json ("" for a role: roles come from the theme).
    pub default: String,
}

/// `composer.border.focus` → `composerBorderFocus`: the first segment as is, every later one with its first letter
/// upper-cased (segments are lowerCamel ASCII, NAMING.md §4).
pub fn qml_name(id: &str) -> String {
    let mut out = String::with_capacity(id.len());
    for (i, seg) in id.split('.').enumerate() {
        if i == 0 {
            out.push_str(seg);
            continue;
        }
        let mut cs = seg.chars();
        if let Some(c) = cs.next() {
            out.push(c.to_ascii_uppercase());
            out.push_str(cs.as_str());
        }
    }
    out
}

/// NAMING.md §4 id grammar as far as Th needs it: `[a-z][A-Za-z0-9]*` then `.`-separated `[A-Za-z0-9]+` segments.
pub fn valid_id(id: &str) -> bool {
    let mut segs = id.split('.');
    let Some(first) = segs.next() else { return false };
    let mut fc = first.chars();
    if !matches!(fc.next(), Some('a'..='z')) || !fc.all(|c| c.is_ascii_alphanumeric()) {
        return false;
    }
    segs.all(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric()))
}

/// Every colour of Th in file order — the roles, then the element tokens, each sorted by id — or why the token list
/// cannot become a QML singleton (a malformed id, two ids on one name, a reserved name, a `…Changed` name that would
/// shadow a change signal). ALL tokens, not only those the audit found a desktop site for (`apps`): the audit
/// missed some desktop sites (GlowButton's rest fill is `button.cta.bg`, marked web-only), and parity fixes give
/// the desktop elements only the web paints today — every catalogue name must be usable on both sides.
pub fn entries(tokens_json: &Value) -> Result<Vec<Entry>, String> {
    let roles = tokens_json.get("roles").and_then(Value::as_array).ok_or("tokens.json has no roles array")?;
    let tokens = tokens_json.get("tokens").and_then(Value::as_array).ok_or("tokens.json has no tokens array")?;
    let mut role_ids: Vec<String> = Vec::new();
    for r in roles {
        role_ids.push(r.get("id").and_then(Value::as_str).ok_or("a role without an id")?.to_string());
    }
    role_ids.sort();
    let mut toks: Vec<(String, String)> = Vec::new();
    for t in tokens {
        let id = t.get("id").and_then(Value::as_str).ok_or("a token without an id")?;
        let default = t.get("default").and_then(Value::as_str).ok_or_else(|| format!("{id}: no default"))?;
        toks.push((id.to_string(), default.to_string()));
    }
    toks.sort();
    let mut out = Vec::with_capacity(role_ids.len() + toks.len());
    for id in role_ids {
        out.push(Entry { name: qml_name(&id), id, role: true, default: String::new() });
    }
    for (id, default) in toks {
        out.push(Entry { name: qml_name(&id), id, role: false, default });
    }
    let mut seen: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
    for e in &out {
        if !valid_id(&e.id) {
            return Err(format!("{}: not a NAMING.md §4 id", e.id));
        }
        if RESERVED.contains(&e.name.as_str()) {
            return Err(format!("{}: its name `{}` is one Th itself uses", e.id, e.name));
        }
        if e.name.ends_with("Changed") {
            return Err(format!("{}: `{}` would shadow a QML change signal", e.id, e.name));
        }
        if let Some(other) = seen.insert(e.name.as_str(), e.id.as_str()) {
            return Err(format!("{} and {} both become `{}`", other, e.id, e.name));
        }
    }
    Ok(out)
}

/// The alpha of a canonical colour as the engine writes it (`0.18`, `0.5`, `0`), for a QML/JS number literal: it
/// parses to exactly the double `k / 10000` the app hands Qt.
pub fn alpha_text(c: &Colour) -> String {
    let canon = tinhat_theme::format_colour(*c);
    match canon.split_once('/') {
        Some((_, a)) => a.to_string(),
        None => "1".to_string(),
    }
}

/// A QML expression that builds the very QColor the app pushes for `c` (theme_ctl.rs `qcolor`): `"#rrggbb"` for an
/// opaque colour (8-bit channels, like every palette string), else `Qt.rgba(r/255, g/255, b/255, a)` — the
/// fromRgbF path the old `Qt.rgba(…)` / `Qt.alpha(…)` sites took, so the 16-bit alpha is identical.
pub fn qml_colour(c: &Colour) -> String {
    if c.is_opaque() {
        format!("\"{}\"", c.hex6())
    } else {
        format!("Qt.rgba({}/255, {}/255, {}/255, {})", c.r, c.g, c.b, alpha_text(c))
    }
}

fn js_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The whole text of `qml/Th.qml`: every entry with its value in `default_theme` (the built-in OLED) as default.
/// `meta` = (id, display name, variant, names mode) of that theme.
pub fn th_qml(entries: &[Entry], default_theme: &Resolved, meta: (&str, &str, &str, &str)) -> String {
    let (theme_id, theme_name, variant, names) = meta;
    let n_roles = entries.iter().filter(|e| e.role).count();
    let n_tokens = entries.len() - n_roles;
    let colour_of = |id: &str| -> Colour {
        default_theme.colour(id).unwrap_or_else(|| panic!("the default theme resolves no colour for {id}"))
    };
    let mut s = String::with_capacity(160 * entries.len());
    s.push_str(
        "// Th.qml — GENERATED by crates/app/build.rs (src/theme_gen.rs) from crates/theme/tokens.json and the built-in\n\
         // OLED theme. DO NOT EDIT: every build rewrites this file when it is stale — commit the result with the change\n\
         // that caused it (scripts/theme-lint.py fails on an uncommitted Th.qml).\n\
         //\n",
    );
    s.push_str(&format!(
        "// The ACTIVE theme: one `color` per role ({n_roles}) and per element token ({n_tokens}), named in lowerCamel\n\
         // after the id — `composer.border.focus` → Th.composerBorderFocus. Paint with the token of the element (NAMING.md;\n\
         // the site → token map is audit/merged/sitemap-desktop.jsonl); a role only where the sitemap says so (plumbing).\n\
         // Defaults = the built-in OLED. AppCore.applyTheme() fills everything from ThemeController (the Rust engine,\n\
         // crates/theme) at start and on every switch; nothing else may assign these. Guide: docs/MIGRATING-THEME.md.\n"
    ));
    s.push_str("pragma Singleton\nimport QtQuick\n\nQtObject {\n    id: th\n\n");
    s.push_str("    // ---- the theme itself (not colours)\n");
    s.push_str(&format!(
        "    property string themeId: {}      // the applied theme (an unknown or refused id falls back to OLED)\n",
        js_string(theme_id)
    ));
    s.push_str(&format!("    property string themeName: {}    // its display name\n", js_string(theme_name)));
    s.push_str(&format!("    property string variant: {}      // \"dark\" | \"light\"\n", js_string(variant)));
    s.push_str(&format!(
        "    property string names: {}    // people's name colours: \"hash-hsl\" (the algorithm) | \"palette\" (name.1…12)\n",
        js_string(names)
    ));
    s.push_str(
        "    property int revision: 0             // +1 after every apply(): repaint imperative painters (Canvas.requestPaint)\n",
    );
    s.push_str(
        "    // the theme's shape block (ENGINE.md §4 step 1b, ISLANDS-ARCH §3.4): island geometry in layout px (× unitPm)\n\
         \x20   property int shapeRadius: 12\n\
         \x20   property int shapeGap: 6\n\
         \x20   property int shapeMargin: 6\n\
         \x20   property int shapeBorder: 1\n\
         \x20   // the theme's look (ENGINE.md §10, THEMES-UI.md §13): fonts from the allowed lists (\"\" = the app's own; the chat\n\
         \x20   // font is type.chat, else type.ui), the text size in per cent, the island shadow (px, 0 = none) and the density.\n\
         \x20   // Main.qml puts fontUi + textSize on the window's font (every control inherits it); the house text components\n\
         \x20   // scale their sizes by textScale; fontMono is the code font (type.code, else the platform's monospace)\n\
         \x20   property string fontUi: \"\"\n\
         \x20   property string fontChat: \"\"\n\
         \x20   property string fontCode: \"\"\n\
         \x20   property int textSize: 100\n\
         \x20   property real textScale: textSize / 100\n\
         \x20   property string fontMono: fontCode.length > 0 ? fontCode : \"monospace\"\n\
         \x20   property int shapeShadow: 0\n\
         \x20   property string shapeDensity: \"normal\"\n\n",
    );
    s.push_str(
        "    // Fill every colour from ThemeController.resolve() ({ <property>: color }) + the theme's meta and shape\n\
         \x20   // (ThemeController.shape, JSON) + its look (ThemeController.look, JSON). AppCore only.\n\
         \x20   function apply(colours, id, name, variantName, namesMode, shapeJson, lookJson) {\n\
         \x20       var n = 0\n\
         \x20       for (var k in colours) {\n\
         \x20           if (k in th) { th[k] = colours[k]; n++ }\n\
         \x20           else console.warn(\"[theme] Th has no colour '\" + k + \"' (stale qml/Th.qml?)\")\n\
         \x20       }\n\
         \x20       th.themeId = id; th.themeName = name; th.variant = variantName; th.names = namesMode\n\
         \x20       if (shapeJson) {\n\
         \x20           var sh = JSON.parse(shapeJson)\n\
         \x20           th.shapeRadius = sh.radius; th.shapeGap = sh.gap; th.shapeMargin = sh.margin; th.shapeBorder = sh.border\n\
         \x20       }\n\
         \x20       if (lookJson) {\n\
         \x20           var lk = JSON.parse(lookJson), f = lk.fonts || {}\n\
         \x20           th.fontUi = f.ui || \"\"; th.fontChat = f.chat || f.ui || \"\"; th.fontCode = f.code || \"\"\n\
         \x20           th.textSize = lk.size; th.shapeShadow = lk.shadow; th.shapeDensity = lk.density\n\
         \x20       }\n\
         \x20       th.revision++\n\
         \x20       return n\n\
         \x20   }\n\
         \x20   // The Pick tool's probe pass (THEMES-UI.md §15.2): the colours only — no meta, no revision (nothing repaints for\n\
         \x20   // it; the real colours come back in the same call).\n\
         \x20   function applyColours(colours) {\n\
         \x20       for (var k in colours) if (k in th) th[k] = colours[k]\n\
         \x20   }\n\
         \x20   // A role's or token's colour by id: Th.byId(\"composer.border.focus\") === Th.composerBorderFocus.\n\
         \x20   function byId(id) {\n\
         \x20       var s = String(id).split(\".\"), n = s[0]\n\
         \x20       for (var i = 1; i < s.length; i++) n += s[i].charAt(0).toUpperCase() + s[i].slice(1)\n\
         \x20       return th[n]\n\
         \x20   }\n\
         \x20   // A colour REFERENCE as a model hands it to QML: a role or token id — a state key, e.g. the presence\n\
         \x20   // model's \"presence.away.dot\" — is the active theme's colour (a binding re-reads it on a switch); a\n\
         \x20   // colour string (\"#5865F2\": content, e.g. a custom status colour) is itself.\n\
         \x20   function colourRef(ref) {\n\
         \x20       var s = String(ref)\n\
         \x20       return s.charAt(0) === \"#\" ? s : th.byId(s)\n\
         \x20   }\n",
    );
    let line = |s: &mut String, e: &Entry| {
        let decl = format!("    property color {}: {}", e.name, qml_colour(&colour_of(&e.id)));
        let note = if e.role { e.id.clone() } else { format!("{} = {}", e.id, e.default) };
        let pad = if decl.len() < 76 { 76 - decl.len() } else { 1 };
        s.push_str(&decl);
        s.push_str(&" ".repeat(pad));
        s.push_str("// ");
        s.push_str(&note);
        s.push('\n');
    };
    s.push_str(&format!("\n    // ---- roles ({n_roles})\n"));
    for e in entries.iter().filter(|e| e.role) {
        line(&mut s, e);
    }
    s.push_str(&format!("\n    // ---- element tokens ({n_tokens}): <id> = <default expression over roles>\n"));
    for e in entries.iter().filter(|e| !e.role) {
        line(&mut s, e);
    }
    s.push_str("}\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(qml_name("composer.border.focus"), "composerBorderFocus");
        assert_eq!(qml_name("state.dangerBright"), "stateDangerBright");
        assert_eq!(qml_name("name.1"), "name1");
        assert_eq!(qml_name("scrim"), "scrim");
        assert!(valid_id("a.b1.cD") && valid_id("shadow"));
        assert!(!valid_id("A.b") && !valid_id("a..b") && !valid_id("a.b-c") && !valid_id(""));
    }

    #[test]
    fn collisions_and_reserved_names_are_refused() {
        let spec = |ids: &[&str]| {
            serde_json::json!({
                "roles": [{"id": "accent.base"}],
                "tokens": ids.iter().map(|i| serde_json::json!({"id": i, "default": "accent.base", "apps": ["desktop"]})).collect::<Vec<_>>()
            })
        };
        assert!(entries(&spec(&["a.bC", "a.b.c"])).unwrap_err().contains("both become `aBC`"));
        assert!(entries(&spec(&["theme.id"])).unwrap_err().contains("Th itself uses"));
        assert!(entries(&spec(&["x.y.changed"])).unwrap_err().contains("change signal"));
        assert_eq!(entries(&spec(&["a.b"])).unwrap().len(), 2);
    }

    #[test]
    fn every_token_roles_first() {
        let v = serde_json::json!({
            "roles": [{"id": "surface.window"}],
            "tokens": [
                {"id": "z.web", "default": "surface.window", "apps": ["web"]},
                {"id": "a.both", "default": "surface.window", "apps": ["desktop", "web"]}
            ]
        });
        let e = entries(&v).unwrap();
        assert_eq!(e.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["surface.window", "a.both", "z.web"]);
    }

    #[test]
    fn colour_literals() {
        let c = tinhat_theme::parse_literal("#1e6a76/0.18").unwrap();
        assert_eq!(qml_colour(&c), "Qt.rgba(30/255, 106/255, 118/255, 0.18)");
        assert_eq!(qml_colour(&tinhat_theme::parse_literal("#1E6A76").unwrap()), "\"#1e6a76\"");
        assert_eq!(alpha_text(&tinhat_theme::parse_literal("#000000/0").unwrap()), "0");
    }

    /// The real catalogue: no collision, and every desktop site token of the audit has its property.
    #[test]
    fn the_real_token_list_makes_a_singleton() {
        let v = tinhat_theme::parse_json(tinhat_theme::engine::TOKENS_JSON).unwrap();
        let e = entries(&v).unwrap();
        assert!(e.iter().filter(|e| e.role).count() >= 52);
        assert!(e.iter().any(|e| e.name == "composerBorderFocus"));
    }
}
