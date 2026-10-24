//! theme_lib.rs — the built-in theme library (themes, step 1b; share DESIGN.md "The library"): the contract's
//! `themes-library.json` — 1,190 themes from iTerm2-Color-Schemes and tinted-theming base16/base24 at pinned commits,
//! plus OLED and Goose — embedded in the binary. The copy is `crates/theme/themes-library.json`, byte-identical to the
//! contract and to the web's `src/theme/themes-library.json` (crates/theme/sync-vectors.sh copies and checks it).
//! Pure: no Qt, no I/O.
//!
//! What a gallery needs for 1,190 cards without resolving 1,190 themes: the id, name, author, dark or light (ENGINE.md
//! §4: the document's `variant`, else dark ⇔ Y(bg) < Y(fg) — exactly what the engine computes) and the favourite rank.
//! The colours of a card come from [`preview`] (the ROLES only, resolved when the card is first drawn, then cached).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::Value;
use tinhat_theme::maths::luminance;
use tinhat_theme::{parse_literal, resolve_theme, TokensSpec, Variant};

/// The library file (`format: "tinhat-themes-library"`, `v: 1`).
pub const LIBRARY_JSON: &str = include_str!("../../theme/themes-library.json");

/// One library theme.
#[derive(Debug)]
pub struct Entry {
    pub id: String,
    pub name: String,
    /// `author` when the document has one, else "".
    pub author: String,
    pub variant: Variant,
    /// Its place in the library's `favourites` list (0 = first), if it is a favourite.
    pub fav: Option<usize>,
    /// The theme document (`tinhat-theme` v1).
    pub doc: Value,
}

/// The parsed library.
#[derive(Debug)]
pub struct Library {
    /// In file order (by id).
    pub entries: Vec<Entry>,
    /// The favourites, in the library's order (ids).
    pub favourites: Vec<String>,
    by_id: HashMap<String, usize>,
}

impl Library {
    pub fn get(&self, id: &str) -> Option<&Entry> {
        self.by_id.get(id).map(|&i| &self.entries[i])
    }
}

/// dark/light of a theme document without resolving it (ENGINE.md §4): `variant` when given; for a palette theme
/// dark ⇔ Y(bg) < Y(fg); otherwise (a roles-only theme without `variant`) the engine decides from its resolved roles.
pub fn doc_variant(doc: &Value) -> Option<Variant> {
    match doc.get("variant").and_then(Value::as_str) {
        Some("dark") => return Some(Variant::Dark),
        Some("light") => return Some(Variant::Light),
        _ => {}
    }
    if let Some(p) = doc.get("palette") {
        let bg = p.get("bg").and_then(Value::as_str).and_then(parse_literal)?;
        let fg = p.get("fg").and_then(Value::as_str).and_then(parse_literal)?;
        return Some(if luminance(bg) < luminance(fg) { Variant::Dark } else { Variant::Light });
    }
    resolve_theme(doc, &TokensSpec::empty()).ok().map(|r| r.variant)
}

fn parse_library(text: &str) -> Result<Library, String> {
    let v = tinhat_theme::parse_json(text).map_err(|e| format!("themes-library.json: {e:?}"))?;
    if v.get("format").and_then(Value::as_str) != Some("tinhat-themes-library") {
        return Err("themes-library.json: not a tinhat-themes-library".into());
    }
    let favourites: Vec<String> = v
        .get("favourites")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    let list = v.get("themes").and_then(Value::as_array).ok_or("themes-library.json: no themes")?;
    let mut entries = Vec::with_capacity(list.len());
    let mut by_id = HashMap::with_capacity(list.len());
    for doc in list {
        let id = doc.get("id").and_then(Value::as_str).ok_or("themes-library.json: a theme without an id")?;
        let name = doc.get("name").and_then(Value::as_str).unwrap_or(id).to_string();
        let author = doc.get("author").and_then(Value::as_str).unwrap_or("").to_string();
        let variant = doc_variant(doc).ok_or_else(|| format!("themes-library.json: {id}: no variant"))?;
        let fav = favourites.iter().position(|f| f == id);
        if by_id.insert(id.to_string(), entries.len()).is_some() {
            return Err(format!("themes-library.json: {id} twice"));
        }
        entries.push(Entry { id: id.to_string(), name, author, variant, fav, doc: doc.clone() });
    }
    Ok(Library { entries, favourites, by_id })
}

/// The embedded library, parsed once.
pub fn library() -> &'static Library {
    static LIB: OnceLock<Library> = OnceLock::new();
    LIB.get_or_init(|| parse_library(LIBRARY_JSON).expect("the embedded themes-library.json is a valid library"))
}

/// A library theme's document.
pub fn doc(id: &str) -> Option<&'static Value> {
    library().get(id).map(|e| &e.doc)
}

/// The colours a gallery card paints: the theme's resolved ROLES (no element tokens — cheap), its variant and how
/// many guarded roles the readability guard adjusted ("N colours adjusted").
#[derive(Debug)]
pub struct Preview {
    pub variant: Variant,
    pub roles: std::collections::BTreeMap<String, String>,
    pub adjusted: Vec<String>,
}

/// Resolve the roles of a theme document for its card (`Err` = the engine's refusal text).
pub fn preview_doc(doc: &Value) -> Result<Preview, String> {
    let r = resolve_theme(doc, &TokensSpec::empty()).map_err(|e| e.to_string())?;
    Ok(Preview { variant: r.variant, roles: r.roles, adjusted: r.adjusted })
}

/// [`preview_doc`] of a library theme, cached for the life of the process (library themes never change).
pub fn preview(id: &str) -> Option<Arc<Preview>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<Preview>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(p) = cache.lock().unwrap_or_else(|p| p.into_inner()).get(id) {
        return Some(p.clone());
    }
    let p = Arc::new(preview_doc(doc(id)?).ok()?);
    cache.lock().unwrap_or_else(|p| p.into_inner()).insert(id.to_string(), p.clone());
    Some(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_library_parses() {
        let lib = library();
        assert_eq!(lib.entries.len(), 1190);
        assert_eq!(lib.favourites.first().map(String::as_str), Some("oled"));
        for f in &lib.favourites {
            assert!(lib.get(f).is_some(), "favourite {f} is in the library");
        }
        let latte = lib.get("catppuccin-latte").unwrap();
        assert_eq!((latte.name.as_str(), latte.variant), ("Catppuccin Latte", Variant::Light));
        assert_eq!(lib.get("dracula").unwrap().variant, Variant::Dark);
        assert_eq!(lib.get("oled").unwrap().fav, Some(0));
    }

    /// The library's OLED and Goose are the shipped built-ins (so a library card and the built-in agree).
    #[test]
    fn library_builtins_are_the_shipped_ones() {
        for id in ["oled", "goose"] {
            assert_eq!(tinhat_theme::builtin_id(doc(id).unwrap()), Some(id), "{id}");
        }
    }

    /// Every library theme resolves (roles only), and the variant the gallery filters on is the engine's.
    #[test]
    fn every_library_theme_resolves_with_the_engines_variant() {
        for e in &library().entries {
            let p = preview_doc(&e.doc).unwrap_or_else(|why| panic!("{}: {why}", e.id));
            assert_eq!(p.variant, e.variant, "{}", e.id);
        }
    }

    #[test]
    fn previews_are_cached() {
        let a = preview("nord").unwrap();
        let b = preview("nord").unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        assert!(preview("no-such-theme").is_none());
        assert_eq!(a.roles["surface.island"], "#2e3440");
    }
}

#[cfg(test)]
mod bench {
    /// Timing only (`cargo test --release -p tinhat-app theme_lib::bench -- --ignored --nocapture`).
    #[test]
    #[ignore]
    fn resolve_timings() {
        let lib = super::library();
        let t = std::time::Instant::now();
        for e in &lib.entries {
            let _ = super::preview_doc(&e.doc);
        }
        eprintln!("roles only: {} themes in {:?}", lib.entries.len(), t.elapsed());
        let t = std::time::Instant::now();
        let mut n = 0;
        for e in lib.entries.iter().take(100) {
            let _ = tinhat_theme::resolve_for_app(&e.doc, tinhat_theme::app_tokens(), tinhat_theme::App::Desktop);
            n += 1;
        }
        eprintln!("full: {n} themes in {:?}", t.elapsed());
    }
}
