//! labels.rs — port of `labels.ts` (contract THEMES-UI.md §13.8): the words of the theme editor's Advanced mode —
//! palette-slot and role labels, token labels built from the id (NAMING.md §4), the groups (the palette, the role
//! groups, then every token area under its section) and the search. The labels and the grouping come from
//! `editor-labels.json`, a byte copy of the contract's (the web loads the same file), so both apps show the same words
//! in the same order; the functions are pinned by `testvectors/editor/labels-*.json` (tests/editor.rs).
//! Pure; ASCII-only case folding (both languages agree on every input).

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde_json::{json, Value};

use crate::roles::PALETTE_SLOTS;

/// The contract's `editor-labels.json` (sync-vectors.sh copies it byte for byte).
pub const LABELS_JSON: &str = include_str!("../editor-labels.json");

/// The parsed label data.
#[derive(Clone, Debug)]
pub struct EditorLabels {
    pub palette: BTreeMap<String, String>,
    pub roles: BTreeMap<String, String>,
    /// (label, role ids) in order
    pub role_groups: Vec<(String, Vec<String>)>,
    /// (label, areas) in order
    pub sections: Vec<(String, Vec<String>)>,
    pub areas: BTreeMap<String, String>,
    pub words: BTreeMap<String, String>,
    pub properties: Vec<String>,
    pub states: Vec<String>,
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default()
}
fn string_map(v: &Value) -> BTreeMap<String, String> {
    v.as_object()
        .map(|o| o.iter().filter_map(|(k, x)| x.as_str().map(|s| (k.clone(), s.to_string()))).collect())
        .unwrap_or_default()
}

impl EditorLabels {
    pub fn from_json(v: &Value) -> EditorLabels {
        let pairs = |key: &str, list: &str| -> Vec<(String, Vec<String>)> {
            v[key]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|g| (g["label"].as_str().unwrap_or("").to_string(), strings(&g[list])))
                        .collect()
                })
                .unwrap_or_default()
        };
        EditorLabels {
            palette: string_map(&v["palette"]),
            roles: string_map(&v["roles"]),
            role_groups: pairs("roleGroups", "ids"),
            sections: pairs("sections", "areas"),
            areas: string_map(&v["areas"]),
            words: string_map(&v["words"]),
            properties: strings(&v["properties"]),
            states: strings(&v["states"]),
        }
    }
}

/// The embedded labels (parsed once).
pub fn labels() -> &'static EditorLabels {
    static L: OnceLock<EditorLabels> = OnceLock::new();
    L.get_or_init(|| {
        let v: Value = crate::json::parse_json(LABELS_JSON).expect("editor-labels.json is JSON");
        EditorLabels::from_json(&v)
    })
}

/// ASCII lower case (A–Z only).
pub fn lower_ascii(s: &str) -> String {
    s.chars().map(|c| c.to_ascii_lowercase()).collect()
}

/// First character upper-cased when it is an ASCII letter.
fn capital(s: &str) -> String {
    let mut it = s.chars();
    match it.next() {
        Some(c) if c.is_ascii_lowercase() => c.to_ascii_uppercase().to_string() + it.as_str(),
        _ => s.to_string(),
    }
}

/// "afkBanner" → ["afk", "banner"]: split before each ASCII capital, lower-cased.
fn camel_words(seg: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut w = String::new();
    for c in seg.chars() {
        if c.is_ascii_uppercase() {
            if !w.is_empty() {
                out.push(std::mem::take(&mut w));
            }
            w.push(c.to_ascii_lowercase());
        } else {
            w.push(c);
        }
    }
    if !w.is_empty() {
        out.push(w);
    }
    out
}

fn word<'a>(w: &'a str, l: &'a EditorLabels) -> &'a str {
    l.words.get(w).map(String::as_str).unwrap_or(w)
}

/// A token's label from its id (§13.8): the area segment dropped; a trailing state after a property becomes
/// " (<state>)"; every remaining segment split into words (camelCase), each word through the word table; joined with
/// spaces, first letter capital. A one-segment id is its own words.
pub fn token_label(id: &str, l: &EditorLabels) -> String {
    let segs: Vec<&str> = id.split('.').collect();
    let mut rest: Vec<&str> = if segs.len() > 1 { segs[1..].to_vec() } else { segs.clone() };
    let mut state: Option<&str> = None;
    if rest.len() >= 2
        && l.states.iter().any(|s| s == rest[rest.len() - 1])
        && l.properties.iter().any(|p| p == rest[rest.len() - 2])
    {
        state = rest.pop();
    }
    let mut words: Vec<String> = Vec::new();
    for s in &rest {
        for w in camel_words(s) {
            words.push(word(&w, l).to_string());
        }
    }
    let label = capital(&words.join(" "));
    match state {
        None => label,
        Some(s) => format!("{label} ({})", word(s, l)),
    }
}

pub fn role_label(id: &str, l: &EditorLabels) -> String {
    l.roles.get(id).cloned().unwrap_or_else(|| id.to_string())
}
pub fn palette_label(slot: &str, l: &EditorLabels) -> String {
    l.palette.get(slot).cloned().unwrap_or_else(|| slot.to_string())
}
pub fn area_label(area: &str, l: &EditorLabels) -> String {
    l.areas.get(area).cloned().unwrap_or_else(|| area.to_string())
}

/// A token as the Advanced mode needs it (tokens.json: id, area, description).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TokenInfo {
    pub id: String,
    pub area: String,
    pub description: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    Palette,
    Role,
    Token,
}
impl RowKind {
    pub fn as_str(self) -> &'static str {
        match self {
            RowKind::Palette => "palette",
            RowKind::Role => "role",
            RowKind::Token => "token",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditorRow {
    pub kind: RowKind,
    pub id: String,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditorGroup {
    /// "palette", "roles:<n>", "area:<area>"
    pub id: String,
    pub section: String,
    pub label: String,
    pub rows: Vec<EditorRow>,
}

impl EditorGroup {
    /// `{id, label, rows: [{id, kind, label}], section}` — the reference's object.
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "section": self.section,
            "label": self.label,
            "rows": self.rows.iter().map(|r| json!({"kind": r.kind.as_str(), "id": r.id, "label": r.label})).collect::<Vec<_>>(),
        })
    }
}

/// The Advanced mode's groups in order (§13.8): the palette (palette themes only), the role groups, then for every
/// section every area of it that has tokens, then the areas no section lists under "Other", by area id. Token rows in
/// id order (code points).
pub fn advanced_groups(has_palette: bool, tokens: &[TokenInfo], l: &EditorLabels) -> Vec<EditorGroup> {
    let mut out = Vec::new();
    if has_palette {
        out.push(EditorGroup {
            id: "palette".into(),
            section: "Palette".into(),
            label: "Palette".into(),
            rows: PALETTE_SLOTS
                .iter()
                .map(|s| EditorRow { kind: RowKind::Palette, id: s.to_string(), label: palette_label(s, l) })
                .collect(),
        });
    }
    for (i, (label, ids)) in l.role_groups.iter().enumerate() {
        out.push(EditorGroup {
            id: format!("roles:{i}"),
            section: "Roles".into(),
            label: label.clone(),
            rows: ids.iter().map(|id| EditorRow { kind: RowKind::Role, id: id.clone(), label: role_label(id, l) }).collect(),
        });
    }
    let mut by_area: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for t in tokens {
        by_area.entry(t.area.as_str()).or_default().push(t.id.as_str());
    }
    let rows_of = |area: &str| -> Vec<EditorRow> {
        let mut ids: Vec<&str> = by_area.get(area).cloned().unwrap_or_default();
        ids.sort();
        ids.into_iter().map(|id| EditorRow { kind: RowKind::Token, id: id.to_string(), label: token_label(id, l) }).collect()
    };
    let mut listed: Vec<&str> = Vec::new();
    for (section, areas) in &l.sections {
        for a in areas {
            listed.push(a.as_str());
            if !by_area.contains_key(a.as_str()) {
                continue;
            }
            out.push(EditorGroup { id: format!("area:{a}"), section: section.clone(), label: area_label(a, l), rows: rows_of(a) });
        }
    }
    for a in by_area.keys().filter(|a| !listed.contains(a)) {
        out.push(EditorGroup { id: format!("area:{a}"), section: "Other".into(), label: a.to_string(), rows: rows_of(a) });
    }
    out
}

/// The search words of a query: ASCII-lower-cased, split on space / tab / CR / LF, empty ones dropped.
pub fn search_words(query: &str) -> Vec<String> {
    lower_ascii(query).split([' ', '\t', '\r', '\n']).filter(|w| !w.is_empty()).map(str::to_string).collect()
}

/// The groups a search keeps (§13.8): a row matches when every word occurs in its text — label, id, group label,
/// section and (tokens) the description — ASCII-lower-cased; groups with no matching row are left out.
pub fn search_groups(groups: &[EditorGroup], query: &str, describe: impl Fn(&str) -> String) -> Vec<EditorGroup> {
    let words = search_words(query);
    if words.is_empty() {
        return groups.to_vec();
    }
    let mut out = Vec::new();
    for g in groups {
        let rows: Vec<EditorRow> = g
            .rows
            .iter()
            .filter(|r| {
                let desc = if r.kind == RowKind::Token { describe(&r.id) } else { String::new() };
                let hay = lower_ascii(&format!("{} {} {} {} {}", r.label, r.id, g.label, g.section, desc));
                words.iter().all(|w| hay.contains(w.as_str()))
            })
            .cloned()
            .collect();
        if !rows.is_empty() {
            out.push(EditorGroup { rows, ..g.clone() });
        }
    }
    out
}

/// The app's token infos (the embedded tokens.json: id, area, description), in list order.
pub fn app_token_info() -> &'static [TokenInfo] {
    static T: OnceLock<Vec<TokenInfo>> = OnceLock::new();
    T.get_or_init(|| {
        let v: Value = crate::json::parse_json(crate::engine::TOKENS_JSON).expect("tokens.json is JSON");
        v["tokens"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|t| TokenInfo {
                        id: t["id"].as_str().unwrap_or("").to_string(),
                        area: t["area"].as_str().unwrap_or("").to_string(),
                        description: t["description"].as_str().unwrap_or("").to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    })
}

/// For the app (QML): every group of the Advanced mode with the app's tokens, as JSON text
/// `[{id, section, label, rows: [{kind, id, label}]}]`, plus each token's description in `descriptions`.
pub fn advanced_groups_json(has_palette: bool) -> String {
    let groups = advanced_groups(has_palette, app_token_info(), labels());
    let descriptions: BTreeMap<&str, &str> = app_token_info().iter().map(|t| (t.id.as_str(), t.description.as_str())).collect();
    crate::canon::canon_json(&json!({
        "groups": groups.iter().map(EditorGroup::to_json).collect::<Vec<_>>(),
        "descriptions": descriptions,
    }))
}
