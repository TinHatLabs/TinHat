//! The theme editor's Simple palette (THEME-SIMPLE design, 2026-10-31) — port of the reference `palette.ts` (Tin Hat
//! Web `src/theme/palette.ts`), line by line: the few base colours a theme really stores, grouped by what they paint
//! (Backgrounds, Text, Accent, Status, People), each with every role and token that follows it; and the two Simple
//! edits (editor.rs `swatch` / `group`) — one swatch, or a whole group moved by one OKLab offset. Shared vectors:
//! `testvectors/editor/palette-*`, `edit-swatch-*`, `edit-group-*`, `session-drag-*` (tests/editor.rs) and the
//! reference's palette.test.ts mirrored in tests/palette.rs.
//!
//! No format change: the view only reads the document; the edits write literal colours into fields every app since
//! 11.0 reads (palette slots, `accent`, role values, token overrides, `variant`). Pure: no Qt, no I/O.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use serde_json::{json, Value};

use crate::engine::TokensSpec;
use crate::expr::{parse_expr, Expr};
use crate::labels::{labels, palette_label, role_label};
use crate::literal::{format_colour, parse_literal, Colour};
use crate::maths::{chroma, from_oklab, luminance, to_oklab};
use crate::roles::{derive_role_exprs, is_palette_slot, is_role, Palette, PALETTE_SLOTS, ROLE_IDS};
use crate::sync::JObj;

/// One Simple group: id, label, the roles it owns (lead order).
pub struct PaletteGroupDef {
    pub id: &'static str,
    pub label: &'static str,
    pub roles: &'static [&'static str],
}

/// The Simple groups, in order. Roles in no group (scrim, shadow) are never swatches; `name.N` roles join People only
/// when the theme's names come from the palette.
pub const PALETTE_GROUPS: [PaletteGroupDef; 5] = [
    PaletteGroupDef {
        id: "backgrounds",
        label: "Backgrounds",
        roles: &[
            "surface.island", "surface.window", "surface.raised", "surface.hover", "surface.pressed", "surface.sunken",
            "surface.overlay", "surface.selected", "border.subtle", "border.strong",
        ],
    },
    PaletteGroupDef {
        id: "text",
        label: "Text",
        roles: &["text.primary", "text.secondary", "text.muted", "text.disabled", "text.onAccent"],
    },
    PaletteGroupDef {
        id: "accent",
        label: "Accent",
        roles: &["accent.base", "accent.bright", "accent.dim", "accent.glow", "text.link", "border.focus"],
    },
    PaletteGroupDef {
        id: "status",
        label: "Status",
        roles: &[
            "state.danger", "state.dangerBright", "state.warning", "state.success", "state.info", "state.aside",
            "mention.edge", "mention.bg", "badge.bg", "badge.mention", "call.speaking", "call.muted",
        ],
    },
    PaletteGroupDef {
        id: "people",
        label: "People",
        roles: &[
            "presence.online", "presence.away", "presence.busy", "presence.incall", "presence.offline", "name.1",
            "name.2", "name.3", "name.4", "name.5", "name.6", "name.7", "name.8", "name.9", "name.10", "name.11",
            "name.12",
        ],
    },
];

/// Hand-set token overrides of exactly a swatch's colour follow its edits only when that colour is chromatic (white,
/// black and greys are never dragged along): OKLab chroma ≥ this (the engine's accent threshold, ENGINE.md §5).
pub const FOLLOW_CHROMA: f64 = 0.04;

#[derive(Clone, Debug, PartialEq)]
pub struct Swatch {
    /// the first source: the swatch's key (a palette slot, "accent", or a role id)
    pub id: String,
    /// every source holding this colour in this group, in group order
    pub sources: Vec<String>,
    /// the stored colour, canonical
    pub colour: String,
    /// the first source's label
    pub label: String,
    /// hand-set token overrides of exactly this colour that follow it (chromatic only), id order
    pub follow: Vec<String>,
    /// every role and token that paints with it (the sources' roles included), id order
    pub used_by: Vec<String>,
    /// the other groups whose roles it reaches, group order
    pub also_in: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PaletteGroup {
    pub id: String,
    pub label: String,
    pub swatches: Vec<Swatch>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PaletteView {
    pub groups: Vec<PaletteGroup>,
    /// literal token overrides no swatch reaches, and grouped roles made only of literals (not a swatch): Advanced
    pub hand_set: Vec<String>,
}

impl PaletteView {
    /// The reference's object (`{groups: [{id, label, swatches: [{id, sources, colour, label, follow, usedBy,
    /// alsoIn}]}], handSet}`).
    pub fn to_json(&self) -> Value {
        json!({
            "groups": self.groups.iter().map(|g| json!({
                "id": g.id,
                "label": g.label,
                "swatches": g.swatches.iter().map(|s| json!({
                    "id": s.id, "sources": s.sources, "colour": s.colour, "label": s.label,
                    "follow": s.follow, "usedBy": s.used_by, "alsoIn": s.also_in,
                })).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
            "handSet": self.hand_set,
        })
    }
}

fn refs(e: &Expr, out: &mut Vec<String>) {
    match e {
        Expr::Ref(id) => out.push(id.clone()),
        Expr::Fn { args, .. } => args.iter().for_each(|a| refs(a, out)),
        Expr::Lit(_) => {}
    }
}
fn refs_of(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(e) = parse_expr(src) {
        refs(&e, &mut out);
    }
    out
}
fn str_lit(v: Option<&Value>) -> Option<Colour> {
    v.and_then(Value::as_str).and_then(parse_literal)
}
fn is_literal(v: Option<&Value>) -> bool {
    str_lit(v).is_some()
}
fn obj<'a>(d: &'a JObj, key: &str) -> Option<&'a JObj> {
    d.get(key).and_then(Value::as_object)
}

/// The document's palette (for the §5 derivation), or none (roles-only / unusable: the engine refuses those).
fn palette_of(doc: &JObj) -> Option<Palette> {
    let p = obj(doc, "palette")?;
    let ansi: Vec<Colour> = p.get("ansi").and_then(Value::as_array)?.iter().map(|v| str_lit(Some(v))).collect::<Option<_>>()?;
    let ansi: [Colour; 16] = ansi.try_into().ok()?;
    Some(Palette {
        bg: str_lit(p.get("bg"))?,
        fg: str_lit(p.get("fg"))?,
        cursor: str_lit(p.get("cursor")),
        selection_bg: str_lit(p.get("selectionBg")),
        selection_fg: str_lit(p.get("selectionFg")),
        ansi,
    })
}

fn ansi_index(id: &str) -> Option<usize> {
    let n = id.strip_prefix('c')?;
    if n.is_empty() || n.len() > 2 || !n.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    n.parse().ok()
}

/// The stored value of a source: a palette slot, "accent" (the literal `accent` field) or a role's own value.
pub fn source_value(doc: &JObj, id: &str) -> Option<String> {
    if id == "accent" {
        return doc.get("accent").and_then(Value::as_str).map(str::to_string);
    }
    if is_palette_slot(id) {
        let p = obj(doc, "palette")?;
        let v = match ansi_index(id) {
            Some(n) => p.get("ansi").and_then(Value::as_array).and_then(|a| a.get(n)),
            None => p.get(id),
        };
        return v.and_then(Value::as_str).map(str::to_string);
    }
    obj(doc, "roles").and_then(|r| r.get(id)).and_then(Value::as_str).map(str::to_string)
}

struct Graph {
    /// effective role expressions (derivation + overrides / the roles)
    role_src: HashMap<String, String>,
    /// node → the nodes that reference it directly ("p:<slot>", "r:<role>", "t:<token>")
    rev: HashMap<String, Vec<String>>,
    palette: bool,
    names: bool,
}

fn graph(doc: &JObj, spec: &TokensSpec) -> Graph {
    let pal = palette_of(doc);
    let mut role_src: HashMap<String, String> = HashMap::new();
    if let Some(p) = &pal {
        let dark = match doc.get("variant").and_then(Value::as_str) {
            Some("dark") => true,
            Some("light") => false,
            _ => luminance(p.bg) < luminance(p.fg),
        };
        for (id, src) in derive_role_exprs(p, dark, doc.get("accent").and_then(Value::as_str)) {
            role_src.insert(id.to_string(), src);
        }
    }
    if let Some(ov) = obj(doc, "roles") {
        for (id, v) in ov {
            if let (true, Some(s)) = (is_role(id), v.as_str()) {
                role_src.insert(id.clone(), s.to_string());
            }
        }
    }
    let mut rev: HashMap<String, Vec<String>> = HashMap::new();
    let mut link = |from: String, to: String| rev.entry(to).or_default().push(from);
    for (id, src) in &role_src {
        for r in refs_of(src) {
            if is_role(&r) {
                link(format!("r:{id}"), format!("r:{r}"));
            } else if pal.is_some() && is_palette_slot(&r) {
                link(format!("r:{id}"), format!("p:{r}"));
            }
        }
    }
    let tok_ov = obj(doc, "tokens");
    let token_ids: HashSet<&str> = spec.ids().iter().map(String::as_str).collect();
    for t in spec.ids() {
        let mut rs = Vec::new();
        match tok_ov.and_then(|o| o.get(t)).and_then(Value::as_str) {
            Some(s) => rs = refs_of(s),
            None => {
                if let Some(e) = spec.default_expr(t) {
                    refs(e, &mut rs);
                }
            }
        }
        for r in rs {
            if is_role(&r) {
                link(format!("t:{t}"), format!("r:{r}"));
            } else if token_ids.contains(r.as_str()) {
                link(format!("t:{t}"), format!("t:{r}"));
            }
        }
    }
    Graph { role_src, rev, palette: pal.is_some(), names: doc.get("names").and_then(Value::as_str) != Some("hash-hsl") }
}

/// Every node reachable backwards from `starts` (the starts included).
fn reach(g: &Graph, starts: Vec<String>) -> HashSet<String> {
    let mut seen = HashSet::new();
    let mut stack = starts;
    while let Some(n) = stack.pop() {
        if !seen.insert(n.clone()) {
            continue;
        }
        if let Some(ms) = g.rev.get(&n) {
            stack.extend(ms.iter().cloned());
        }
    }
    seen
}

fn group_roles(g: &Graph, roles: &'static [&'static str]) -> Vec<&'static str> {
    roles.iter().copied().filter(|r| g.names || !r.starts_with("name.")).collect()
}
fn label_of(id: &str) -> String {
    if id == "accent" {
        role_label("accent.base", labels())
    } else if is_palette_slot(id) {
        palette_label(id, labels())
    } else {
        role_label(id, labels())
    }
}
fn node_of(id: &str) -> String {
    if id == "accent" {
        "r:accent.base".into()
    } else if is_palette_slot(id) {
        format!("p:{id}")
    } else {
        format!("r:{id}")
    }
}

/// The Simple palette of a working document (which must resolve).
pub fn palette_view(doc: &JObj, spec: &TokensSpec) -> PaletteView {
    let g = graph(doc, spec);
    // 1. sources, in group order
    let mut owner: HashSet<String> = HashSet::new();
    let mut order: Vec<Vec<String>> = vec![Vec::new(); PALETTE_GROUPS.len()];
    let mut role_group: HashMap<&str, usize> = HashMap::new();
    for (gi, gr) in PALETTE_GROUPS.iter().enumerate() {
        for r in group_roles(&g, gr.roles) {
            role_group.insert(r, gi);
        }
    }
    let mut claim = |src: &str, gi: usize, order: &mut Vec<Vec<String>>| {
        if owner.insert(src.to_string()) {
            order[gi].push(src.to_string());
        }
    };
    let own = |r: &str| obj(doc, "roles").is_some_and(|o| o.contains_key(r));
    let mut made_of_literals: Vec<String> = Vec::new();
    // pass 1: a role that IS a source (a literal, the literal accent, or exactly one slot); pass 2: slots referenced
    // inside expressions — so Text owns `fg` even though the surfaces mix it in
    for (gi, gr) in PALETTE_GROUPS.iter().enumerate() {
        for r in group_roles(&g, gr.roles) {
            let Some(src) = g.role_src.get(r) else { continue };
            if parse_literal(src).is_some() {
                if own(r) {
                    claim(r, gi, &mut order);
                } else if r == "accent.base" && is_literal(doc.get("accent")) {
                    claim("accent", gi, &mut order);
                }
                continue;
            }
            if let (true, Ok(Expr::Ref(id))) = (g.palette, parse_expr(src)) {
                if is_palette_slot(&id) && !is_role(&id) {
                    claim(&id, gi, &mut order);
                }
            }
        }
    }
    for (gi, gr) in PALETTE_GROUPS.iter().enumerate() {
        for r in group_roles(&g, gr.roles) {
            let Some(src) = g.role_src.get(r) else { continue };
            if parse_literal(src).is_some() {
                continue;
            }
            let mut any = false;
            for x in refs_of(src) {
                if is_role(&x) {
                    any = true;
                } else if g.palette && is_palette_slot(&x) {
                    any = true;
                    claim(&x, gi, &mut order);
                }
            }
            if !any && own(r) {
                made_of_literals.push(r.to_string());
            }
        }
    }
    // 2. swatches: the sources of a group with the same canonical colour
    let empty = JObj::new();
    let tok_ov = obj(doc, "tokens").unwrap_or(&empty);
    let mut tok_keys: Vec<&String> = tok_ov.keys().collect();
    tok_keys.sort();
    let mut claimed_tokens: HashSet<String> = HashSet::new();
    let mut groups = Vec::new();
    for (gi, gr) in PALETTE_GROUPS.iter().enumerate() {
        let mut swatches: Vec<Swatch> = Vec::new();
        for src in &order[gi] {
            let Some(c) = source_value(doc, src).as_deref().and_then(parse_literal) else { continue };
            let colour = format_colour(c);
            if let Some(sw) = swatches.iter_mut().find(|s| s.colour == colour) {
                sw.sources.push(src.clone());
            } else {
                swatches.push(Swatch {
                    id: src.clone(),
                    sources: vec![src.clone()],
                    colour,
                    label: label_of(src),
                    follow: Vec::new(),
                    used_by: Vec::new(),
                    also_in: Vec::new(),
                });
            }
        }
        for sw in &mut swatches {
            let c = parse_literal(&sw.colour).expect("canonical");
            if chroma(c) >= FOLLOW_CHROMA {
                for t in &tok_keys {
                    if claimed_tokens.contains(*t) {
                        continue;
                    }
                    if let Some(tc) = str_lit(tok_ov.get(*t)) {
                        if format_colour(tc) == sw.colour {
                            sw.follow.push((*t).clone());
                            claimed_tokens.insert((*t).clone());
                        }
                    }
                }
            }
            let mut starts: Vec<String> = sw.sources.iter().map(|s| node_of(s)).collect();
            starts.extend(sw.follow.iter().map(|t| format!("t:{t}")));
            let seen = reach(&g, starts);
            let mut used: Vec<String> = Vec::new();
            let mut also: BTreeSet<usize> = BTreeSet::new();
            for n in &seen {
                if n.starts_with("p:") {
                    continue;
                }
                let id = &n[2..];
                used.push(id.to_string());
                if n.starts_with("r:") {
                    if let Some(&rg) = role_group.get(id) {
                        if rg != gi {
                            also.insert(rg);
                        }
                    }
                }
            }
            used.sort();
            sw.used_by = used;
            sw.also_in = also.into_iter().map(|i| PALETTE_GROUPS[i].id.to_string()).collect();
        }
        groups.push(PaletteGroup { id: gr.id.into(), label: gr.label.into(), swatches });
    }
    let mut hand_set = made_of_literals;
    for t in &tok_keys {
        if !claimed_tokens.contains(*t) && is_literal(tok_ov.get(*t)) {
            hand_set.push((*t).clone());
        }
    }
    hand_set.sort();
    PaletteView { groups, hand_set }
}

/// A colour moved by an OKLab offset (L clamped to 0..1, channels clamped by from_oklab); its alpha kept. A zero
/// offset returns the colour itself (no re-quantising).
pub fn shift_colour(c: Colour, dl: f64, da: f64, db: f64) -> Colour {
    if dl == 0.0 && da == 0.0 && db == 0.0 {
        return c;
    }
    let p = to_oklab(c);
    let mut l = p.l + dl;
    if l < 0.0 {
        l = 0.0;
    }
    if l > 1.0 {
        l = 1.0;
    }
    let (r, g, b) = from_oklab(l, p.a + da, p.b + db);
    Colour { r, g, b, k: c.k }
}

/// Advanced's "Used by ‹n›": for every role (and, on a palette theme, every palette slot) how many other roles and
/// tokens paint with it, followed through every expression. Keys in id order.
pub fn usage_counts(doc: &JObj, spec: &TokensSpec) -> BTreeMap<String, usize> {
    let g = graph(doc, spec);
    let mut out = BTreeMap::new();
    let slots: &[&str] = if g.palette { &PALETTE_SLOTS } else { &[] };
    for id in slots.iter().chain(ROLE_IDS.iter()) {
        let me = if is_palette_slot(id) { format!("p:{id}") } else { format!("r:{id}") };
        let n = reach(&g, vec![me.clone()]).iter().filter(|x| **x != me && !x.starts_with("p:")).count();
        out.insert((*id).to_string(), n);
    }
    out
}

/// `usage_counts` for the apps (JSON text in and out, the app's token list).
pub fn usage_counts_json(doc_json: &str) -> String {
    match checked_doc(doc_json) {
        Some(d) => json!(usage_counts(&d, crate::engine::app_tokens())).to_string(),
        None => "{}".into(),
    }
}

/// `palette_view` for the apps: the working document's JSON text → the view's JSON text (`{}` when it is not a
/// theme the engine resolves), with the app's token list.
pub fn palette_view_json(doc_json: &str) -> String {
    match checked_doc(doc_json) {
        Some(d) => palette_view(&d, crate::engine::app_tokens()).to_json().to_string(),
        None => "{}".into(),
    }
}

/// A theme document from JSON text that the engine resolves (the view and the counts assume one: the §5 derivation
/// trusts a validated `accent`), or none.
fn checked_doc(doc_json: &str) -> Option<JObj> {
    match crate::json::parse_json(doc_json) {
        Ok(Value::Object(d)) if crate::engine::resolve_theme(&Value::Object(d.clone()), &TokensSpec::empty()).is_ok() => Some(d),
        _ => None,
    }
}
