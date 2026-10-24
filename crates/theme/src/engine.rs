//! engine.rs — port of `engine.ts`: `resolve_theme(theme, tokens_spec)` — validation (§4), role derivation + guards
//! (§5), tokens, result (§6). Pure data in, pure data out; a malformed theme is refused as a whole, never half-applied.
//! Every check, its order and its message text follow the reference (README "How resolveTheme works").

use crate::canon::{json_len, sorted_keys};
use crate::expr::{parse_expr, Expr, FnName};
use crate::limits::{
    code_points, contains_url, is_key_id, is_mxc_uri, is_theme_id, path_seg, BACKDROP_IMAGE_PATH, MAX_BYTES, MAX_DEPTH,
    MAX_OVERRIDES, MAX_STRING,
};
use crate::literal::{format_colour, parse_literal, Colour};
use crate::maths::{luminance, mixok};
use crate::ops::{best_c, darker_c, guard_all_c, guard_c, lighter_c, mix_c, over_c, set_alpha};
use crate::roles::{
    accent_slot, derive_role_exprs, guard_of, is_palette_slot, is_role, palette_slot, Palette, GUARD_SURFACES, ROLE_IDS,
};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap};

/// A refusal: `"<code>: <detail>"` (codes: README "Refusals"; the detail texts are the reference's).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub code: &'static str,
    pub detail: String,
}

impl Refusal {
    fn new(code: &'static str, detail: impl Into<String>) -> Refusal {
        Refusal { code, detail: detail.into() }
    }

    /// The reference's `{ ok: false, error: "<code>: <detail>" }`.
    pub fn to_json(&self) -> Value {
        json!({ "ok": false, "error": self.to_string() })
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.detail)
    }
}

impl std::error::Error for Refusal {}

type R<T> = Result<T, Refusal>;

fn refuse<T>(code: &'static str, detail: impl Into<String>) -> R<T> {
    Err(Refusal::new(code, detail))
}

/// `dark` / `light`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variant {
    Dark,
    Light,
}

impl Variant {
    pub fn as_str(self) -> &'static str {
        match self {
            Variant::Dark => "dark",
            Variant::Light => "light",
        }
    }
}

/// How display names are coloured: from the palette (`name.1`…`name.12`) or by the hash-HSL algorithm.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NamesMode {
    Palette,
    HashHsl,
}

impl NamesMode {
    pub fn as_str(self) -> &'static str {
        match self {
            NamesMode::Palette => "palette",
            NamesMode::HashHsl => "hash-hsl",
        }
    }
}

/// A resolved theme (§6): every role and every token of the spec, canonical colours, sorted by id.
#[derive(Clone, Debug, PartialEq)]
pub struct Resolved {
    pub variant: Variant,
    pub names: NamesMode,
    /// Every role id → canonical colour (`#rrggbb` or `#rrggbb/A`).
    pub roles: BTreeMap<String, String>,
    /// Every token of the spec → canonical colour.
    pub tokens: BTreeMap<String, String>,
    /// Guarded roles the readability guard changed, sorted.
    pub adjusted: Vec<String>,
}

impl Resolved {
    /// The colour of a role or token as r, g, b + alpha (for the QML theme singleton).
    pub fn colour(&self, id: &str) -> Option<Colour> {
        self.roles.get(id).or_else(|| self.tokens.get(id)).and_then(|s| parse_literal(s))
    }

    /// The reference's `{ ok: true, variant, names, roles, tokens, adjusted }`.
    pub fn to_json(&self) -> Value {
        let map = |m: &BTreeMap<String, String>| {
            Value::Object(m.iter().map(|(k, v)| (k.clone(), Value::String(v.clone()))).collect::<Map<_, _>>())
        };
        json!({
            "ok": true,
            "variant": self.variant.as_str(),
            "names": self.names.as_str(),
            "roles": map(&self.roles),
            "tokens": map(&self.tokens),
            "adjusted": self.adjusted,
        })
    }
}

/// `{ok: true, …}` or `{ok: false, error}` — the `expect` of an engine test vector.
pub fn result_to_json(r: &Result<Resolved, Refusal>) -> Value {
    match r {
        Ok(v) => v.to_json(),
        Err(e) => e.to_json(),
    }
}

// ---- tokens spec ------------------------------------------------------------------------------------------------

/// A parsed element-token spec (`tokens.json`, or a vector's `tokensSpec`): ids + default expressions (+ the web's
/// step-1a parity pins). Other per-token fields (`area`, `description`, `apps`, `sites`, `notes`) are ignored.
#[derive(Clone, Debug, Default)]
pub struct TokensSpec {
    ids: Vec<String>, // sorted
    exprs: HashMap<String, Expr>,
    /// token id → built-in theme id → the web's step-1a parity pin (tokens.json `webPin`).
    web_pins: HashMap<String, BTreeMap<String, Colour>>,
}

impl TokensSpec {
    /// An empty spec (roles only).
    pub fn empty() -> TokensSpec {
        TokensSpec::default()
    }

    /// The token ids, sorted.
    pub fn ids(&self) -> &[String] {
        &self.ids
    }

    /// The web's step-1a pin of a token for a built-in theme, if any.
    pub fn web_pin(&self, token: &str, builtin: &str) -> Option<Colour> {
        self.web_pins.get(token)?.get(builtin).copied()
    }

    /// The parsed default expression of a token (`None`: not a token of this spec) — the reference's
    /// `spec.tokens[…].default`, for the theme editor (editor.rs: the opacity op and `island_opacity`).
    pub fn default_expr(&self, id: &str) -> Option<&Expr> {
        self.exprs.get(id)
    }

    /// Parse `{ "tokens": [ { "id", "default", "webPin"? }, … ] }`; refusals are `bad-tokens: …` (checked in array
    /// order; a token's `webPin` right after its default).
    pub fn from_json(spec: &Value) -> Result<TokensSpec, Refusal> {
        let Some(list) = spec.as_object().and_then(|o| o.get("tokens")).and_then(Value::as_array) else {
            return refuse("bad-tokens", "not a tokens spec");
        };
        let mut exprs = HashMap::new();
        let mut web_pins = HashMap::new();
        for (i, t) in list.iter().enumerate() {
            let (Some(id), Some(default)) = (
                t.as_object().and_then(|o| o.get("id")).and_then(Value::as_str),
                t.as_object().and_then(|o| o.get("default")).and_then(Value::as_str),
            ) else {
                return refuse("bad-tokens", format!("item {i} needs a string id and default"));
            };
            if !is_key_id(id) {
                return refuse("bad-tokens", format!("item {i}: bad id"));
            }
            if is_role(id) || is_palette_slot(id) {
                return refuse("bad-tokens", format!("{id}: clashes with a role or palette slot"));
            }
            if exprs.contains_key(id) {
                return refuse("bad-tokens", format!("{id}: duplicate"));
            }
            match parse_expr(default) {
                Ok(e) => {
                    exprs.insert(id.to_string(), e);
                }
                Err(why) => return refuse("bad-tokens", format!("{id}: {why}")),
            }
            if let Some(w) = t.as_object().and_then(|o| o.get("webPin")) {
                let Some(w) = w.as_object() else { return refuse("bad-tokens", format!("{id}: bad webPin")) };
                let mut pins = BTreeMap::new();
                for k in sorted_keys(w) {
                    let Some(c) = w[k.as_str()].as_str().and_then(parse_literal) else {
                        return refuse("bad-tokens", format!("{id}: bad webPin"));
                    };
                    pins.insert(k.clone(), c);
                }
                web_pins.insert(id.to_string(), pins);
            }
        }
        let mut ids: Vec<String> = exprs.keys().cloned().collect();
        ids.sort();
        Ok(TokensSpec { ids, exprs, web_pins })
    }
}

// ---- theme structure + limits (§4) ------------------------------------------------------------------------------

fn check_string(s: &str, path: &str) -> R<()> {
    if code_points(s) > MAX_STRING {
        return refuse("string-too-long", path);
    }
    if contains_url(s) {
        return refuse("url", path);
    }
    Ok(())
}

/// Depth-first over every value (object keys in id order, arrays in index order): depth, string length, URLs.
/// The one exception (§9 C2): the string at /backdrop/image is exempt from both string checks when it is an mxc URI.
/// Recursion stops at depth MAX_DEPTH + 1 (refused before descending).
fn scan(v: &Value, path: &str, depth: usize) -> R<()> {
    match v {
        Value::String(s) => {
            if path == BACKDROP_IMAGE_PATH && is_mxc_uri(s) {
                return Ok(());
            }
            check_string(s, path)
        }
        Value::Array(a) => {
            if depth > MAX_DEPTH {
                return refuse("too-deep", path);
            }
            for (i, x) in a.iter().enumerate() {
                scan(x, &format!("{path}/{i}"), depth + 1)?;
            }
            Ok(())
        }
        Value::Object(o) => {
            if depth > MAX_DEPTH {
                return refuse("too-deep", path);
            }
            for k in sorted_keys(o) {
                let p = format!("{path}/{}", path_seg(k));
                check_string(k, &p)?;
                scan(&o[k.as_str()], &p, depth + 1)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn check_overrides<'a>(t: &'a Map<String, Value>, field: &'static str) -> R<Option<&'a Map<String, Value>>> {
    let Some(o) = t.get(field) else { return Ok(None) };
    let Some(o) = o.as_object() else { return refuse("bad-field", field) };
    for k in sorted_keys(o) {
        if !is_key_id(k) {
            return refuse("bad-key", format!("/{field}/{}", path_seg(k)));
        }
        if !o[k.as_str()].is_string() {
            return refuse("bad-field", format!("/{field}/{k}"));
        }
    }
    Ok(Some(o))
}

fn palette_colour(name: &str, v: Option<&Value>) -> R<Colour> {
    match v.and_then(Value::as_str).and_then(parse_literal) {
        Some(c) if c.is_opaque() => Ok(c),
        _ => refuse("bad-palette", format!("{name} must be an opaque colour")),
    }
}

fn parse_palette(p: &Value) -> R<Palette> {
    let Some(p) = p.as_object() else { return refuse("bad-palette", "not an object") };
    let bg = palette_colour("bg", p.get("bg"))?;
    let fg = palette_colour("fg", p.get("fg"))?;
    let Some(list) = p.get("ansi").and_then(Value::as_array).filter(|a| a.len() == 16) else {
        return refuse("bad-palette", "ansi must be 16 colours");
    };
    let mut ansi = [Colour::BLACK; 16];
    for (i, v) in list.iter().enumerate() {
        ansi[i] = palette_colour(&format!("ansi[{i}]"), Some(v))?;
    }
    let opt = |name: &str| -> R<Option<Colour>> {
        match p.get(name) {
            Some(v) => palette_colour(name, Some(v)).map(Some),
            None => Ok(None),
        }
    };
    let cursor = opt("cursor")?;
    let selection_bg = opt("selectionBg")?;
    let selection_fg = opt("selectionFg")?;
    Ok(Palette { bg, fg, cursor, selection_bg, selection_fg, ansi })
}

// ---- evaluation (§3 resolution order, §5 guards) ----------------------------------------------------------------

struct Exprs<'a> {
    palette: Option<&'a Palette>,
    roles: HashMap<&'static str, Expr>,
    spec: &'a TokensSpec,
    token_overrides: HashMap<String, Expr>,
    guarded: bool,
}

impl<'a> Exprs<'a> {
    fn role(&self, id: &str) -> &Expr {
        self.roles.get(id).expect("every role has an expression")
    }
    fn token(&self, id: &str) -> Option<&Expr> {
        self.token_overrides.get(id).or_else(|| self.spec.exprs.get(id))
    }
}

struct Evaluator<'a> {
    x: &'a Exprs<'a>,
    role_val: HashMap<String, Colour>,
    token_val: HashMap<String, Colour>,
    stack: Vec<String>,
    adjusted: Vec<String>,
}

impl<'a> Evaluator<'a> {
    fn enter(&mut self, id: &str) -> R<()> {
        if let Some(i) = self.stack.iter().position(|s| s == id) {
            let mut chain: Vec<&str> = self.stack[i..].iter().map(String::as_str).collect();
            chain.push(id);
            return refuse("cycle", chain.join(" -> "));
        }
        self.stack.push(id.to_string());
        Ok(())
    }

    fn role(&mut self, id: &str) -> R<Colour> {
        if let Some(c) = self.role_val.get(id) {
            return Ok(*c);
        }
        self.enter(id)?;
        let x = self.x;
        let mut c = self.eval(x.role(id), id, true)?;
        let n = if x.guarded { guard_of(id) } else { None };
        if let Some(n) = n {
            // §9 (2026-09-27): against every neutral surface, island first (it picks the direction).
            let mut bgs = Vec::with_capacity(GUARD_SURFACES.len());
            for s in GUARD_SURFACES {
                bgs.push(self.role(s)?);
            }
            let g = guard_all_c(c, &bgs, n);
            if g != c {
                self.adjusted.push(id.to_string());
            }
            c = g;
        }
        self.stack.pop();
        self.role_val.insert(id.to_string(), c);
        Ok(c)
    }

    fn token(&mut self, id: &str) -> R<Colour> {
        if let Some(c) = self.token_val.get(id) {
            return Ok(*c);
        }
        self.enter(id)?;
        let x = self.x;
        let e = x.token(id).expect("token exists");
        let c = self.eval(e, id, false)?;
        self.stack.pop();
        self.token_val.insert(id.to_string(), c);
        Ok(c)
    }

    fn reference(&mut self, id: &str, owner: &str, in_role: bool) -> R<Colour> {
        if is_role(id) {
            return self.role(id);
        }
        if in_role {
            if let Some(p) = self.x.palette {
                if is_palette_slot(id) {
                    if let Some(c) = palette_slot(p, id) {
                        return Ok(c);
                    }
                }
            }
        } else if self.x.token(id).is_some() {
            return self.token(id);
        }
        refuse("undefined-ref", format!("{owner} -> {id}"))
    }

    fn eval(&mut self, e: &'a Expr, owner: &str, in_role: bool) -> R<Colour> {
        match e {
            Expr::Lit(c) => Ok(*c),
            Expr::Ref(id) => self.reference(id, owner, in_role),
            Expr::Fn { f, args, k } => {
                let mut a = Vec::with_capacity(args.len());
                for x in args {
                    a.push(self.eval(x, owner, in_role)?);
                }
                let n = *k as f64 / 10000.0; // the reference's e.n
                Ok(match f {
                    FnName::Alpha => set_alpha(a[0], *k as u16),
                    FnName::Mix => mix_c(a[0], a[1], n),
                    FnName::Lighter => lighter_c(a[0], n),
                    FnName::Darker => darker_c(a[0], n),
                    FnName::Over => over_c(a[0], a[1]),
                    FnName::Mixok => mixok(a[0], a[1], n),
                    FnName::Best => best_c(a[0], &a[1..]),
                    FnName::Guard => guard_c(a[0], a[1], n),
                })
            }
        }
    }
}

fn parse_or_refuse(src: &str, where_: &str) -> R<Expr> {
    parse_expr(src).or_else(|why| refuse("bad-expr", format!("{where_}: {why}")))
}

/// ENGINE.md §6: resolve a theme document against a token spec. Never panics on bad input; a refusal is
/// `"<code>: <detail>"` exactly as the reference words it.
pub fn resolve_theme(theme: &Value, spec: &TokensSpec) -> Result<Resolved, Refusal> {
    // 1. shape, JSON data, size, strings, URLs
    let Some(t) = theme.as_object() else { return refuse("bad-theme", "not an object") };
    // what the port can parse (§9, PORT-QUESTIONS Q2/Q3; a Rust string never holds a lone surrogate): nesting ≤ 128
    if !crate::json::within_nesting(theme, crate::json::MAX_NESTING) {
        return refuse("bad-theme", "not JSON data");
    }
    let bytes = json_len(theme);
    if bytes > MAX_BYTES {
        return refuse("too-large", format!("{bytes} bytes (max {MAX_BYTES})"));
    }
    scan(theme, "", 1)?;

    // 2. header fields
    if t.get("format").and_then(Value::as_str) != Some("tinhat-theme") {
        return refuse("bad-format", "expected tinhat-theme");
    }
    if t.get("v").and_then(Value::as_number).and_then(|n| n.as_f64()) != Some(1.0) {
        return refuse("bad-version", "expected 1");
    }
    if !t.get("id").and_then(Value::as_str).is_some_and(is_theme_id) {
        return refuse("bad-id", "must be 1-64 characters of a-z 0-9 . : -");
    }
    if !t.get("name").and_then(Value::as_str).is_some_and(|s| s.chars().any(|c| !matches!(c, ' ' | '\t' | '\n' | '\r'))) {
        return refuse("bad-name", "must be a non-empty string");
    }
    for f in ["author", "source"] {
        if t.get(f).is_some_and(|v| !v.is_string()) {
            return refuse("bad-field", f);
        }
    }
    if t.get("variant").is_some_and(|v| v.as_str() != Some("dark") && v.as_str() != Some("light")) {
        return refuse("bad-field", "variant");
    }
    if t.get("names").is_some_and(|v| v.as_str() != Some("palette") && v.as_str() != Some("hash-hsl")) {
        return refuse("bad-field", "names");
    }
    if t.get("accent").is_some_and(|v| v.as_str().is_none_or(|a| accent_slot(a) < 0)) {
        return refuse("bad-accent", "must be cursor, c1-c15 or a colour");
    }
    if let Some(b) = t.get("backdrop") {
        // step 1b: only `image` is validated for now; other backdrop keys are kept like unknown keys
        let Some(b) = b.as_object() else { return refuse("bad-field", "backdrop") };
        if b.get("image").is_some_and(|v| !v.as_str().is_some_and(is_mxc_uri)) {
            return refuse("bad-field", BACKDROP_IMAGE_PATH);
        }
    }
    let role_ov = check_overrides(t, "roles")?;
    let token_ov = check_overrides(t, "tokens")?;
    let n_ov = role_ov.map_or(0, Map::len) + token_ov.map_or(0, Map::len);
    if n_ov > MAX_OVERRIDES {
        return refuse("too-many-overrides", format!("{n_ov} (max {MAX_OVERRIDES})"));
    }

    // 3. colours: a palette (derivation + overrides + guards) or every role directly
    let palette = match t.get("palette") {
        Some(p) => Some(parse_palette(p)?),
        None => None,
    };
    if palette.is_none() && role_ov.is_none() {
        return refuse("no-colours", "needs a palette or every role");
    }
    if palette.is_none() {
        let ov = role_ov.expect("checked above");
        for id in ROLE_IDS {
            if !ov.contains_key(id) {
                return refuse("missing-role", id);
            }
        }
    }

    let mut variant = match t.get("variant").and_then(Value::as_str) {
        Some("dark") => Some(Variant::Dark),
        Some("light") => Some(Variant::Light),
        _ => None,
    };
    let mut roles: HashMap<&'static str, Expr> = HashMap::new();
    if let Some(p) = &palette {
        let v = *variant.get_or_insert(if luminance(p.bg) < luminance(p.fg) { Variant::Dark } else { Variant::Light });
        for (id, src) in derive_role_exprs(p, v == Variant::Dark, t.get("accent").and_then(Value::as_str)) {
            let e = parse_expr(&src).unwrap_or_else(|why| panic!("theme engine: bad derivation for {id}: {why}"));
            roles.insert(id, e);
        }
    }
    if let Some(ov) = role_ov {
        for k in sorted_keys(ov) {
            if let Ok(i) = ROLE_IDS.binary_search(&k.as_str()) {
                let e = parse_or_refuse(ov[k.as_str()].as_str().unwrap_or_default(), &format!("/roles/{k}"))?;
                roles.insert(ROLE_IDS[i], e);
            }
        }
    }
    let mut token_overrides = HashMap::new();
    if let Some(ov) = token_ov {
        for k in sorted_keys(ov) {
            if spec.exprs.contains_key(k.as_str()) {
                let e = parse_or_refuse(ov[k.as_str()].as_str().unwrap_or_default(), &format!("/tokens/{k}"))?;
                token_overrides.insert(k.clone(), e);
            }
        }
    }

    // 4. evaluate: roles in id order, then tokens in id order
    let exprs = Exprs { palette: palette.as_ref(), roles, spec, token_overrides, guarded: palette.is_some() };
    let mut ev = Evaluator {
        x: &exprs,
        role_val: HashMap::new(),
        token_val: HashMap::new(),
        stack: Vec::new(),
        adjusted: Vec::new(),
    };
    for id in ROLE_IDS {
        ev.role(id)?;
    }
    for id in &spec.ids {
        ev.token(id)?;
    }
    let variant = match variant {
        Some(v) => v,
        None => {
            let island = ev.role_val["surface.island"];
            let primary = ev.role_val["text.primary"];
            if luminance(island) < luminance(primary) {
                Variant::Dark
            } else {
                Variant::Light
            }
        }
    };
    let roles = ROLE_IDS.iter().map(|id| (id.to_string(), format_colour(ev.role_val[*id]))).collect();
    let tokens = spec.ids.iter().map(|id| (id.clone(), format_colour(ev.token_val[id]))).collect();
    let mut adjusted = ev.adjusted;
    adjusted.sort();
    let names = match t.get("names").and_then(Value::as_str) {
        Some("hash-hsl") => NamesMode::HashHsl,
        _ => NamesMode::Palette,
    };
    Ok(Resolved { variant, names, roles, tokens, adjusted })
}

/// A theme document from JSON text (account data, a shared card, a file) — the reference's `parseThemeJson`: strict
/// JSON (no BOM, no comments), arrays/objects nested at most 128 deep, no lone surrogate made by a `\u` escape.
/// Anything else is `bad-theme: not JSON data`. Hand the value to [`resolve_for_app`] / [`resolve_theme`].
pub fn parse_theme_json(text: &str) -> Result<Value, Refusal> {
    crate::json::parse_json(text).map_err(|_| Refusal::new("bad-theme", "not JSON data"))
}

/// The reference's signature: `resolveTheme(theme, tokensSpec)` with the spec as JSON (checked first).
pub fn resolve_theme_json(theme: &Value, tokens_spec: &Value) -> Result<Resolved, Refusal> {
    let spec = TokensSpec::from_json(tokens_spec)?;
    resolve_theme(theme, &spec)
}

// ---- the apps' entry point ----------------------------------------------------------------------------------------

/// Which app resolves the theme.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum App {
    Desktop,
    Web,
}

impl App {
    /// `"desktop"` / `"web"`; anything else is the reference's `bad-app: must be desktop or web`.
    pub fn parse(s: &str) -> Result<App, Refusal> {
        match s {
            "desktop" => Ok(App::Desktop),
            "web" => Ok(App::Web),
            _ => refuse("bad-app", "must be desktop or web"),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            App::Desktop => "desktop",
            App::Web => "web",
        }
    }
}

/// `resolve_for_app`'s result: the resolved theme + the tokens the web's step-1a pins replaced (sorted; always empty
/// on the desktop).
#[derive(Clone, Debug, PartialEq)]
pub struct AppResolved {
    pub resolved: Resolved,
    pub pinned: Vec<String>,
}

impl std::ops::Deref for AppResolved {
    type Target = Resolved;
    fn deref(&self) -> &Resolved {
        &self.resolved
    }
}

impl AppResolved {
    /// The reference's `{ ok: true, variant, names, roles, tokens, adjusted, pinned }`.
    pub fn to_json(&self) -> Value {
        let mut v = self.resolved.to_json();
        v["pinned"] = json!(self.pinned);
        v
    }
}

/// `{ok: true, …, pinned}` or `{ok: false, error}` — the `expect` of an app vector.
pub fn app_result_to_json(r: &Result<AppResolved, Refusal>) -> Value {
    match r {
        Ok(v) => v.to_json(),
        Err(e) => e.to_json(),
    }
}

/// The contract's token list (`tokens.json`, a byte-identical copy next to Cargo.toml): what the app resolves with.
pub const TOKENS_JSON: &str = include_str!("../tokens.json");

/// The shipped built-in themes (contract `builtin/*.json`, byte-identical copies), by id.
pub const BUILTIN_JSON: [(&str, &str); 2] =
    [("goose", include_str!("../builtin/goose.json")), ("oled", include_str!("../builtin/oled.json"))];

/// `tokens.json`, parsed once.
pub fn app_tokens() -> &'static TokensSpec {
    static SPEC: std::sync::OnceLock<TokensSpec> = std::sync::OnceLock::new();
    SPEC.get_or_init(|| {
        let v = crate::json::parse_json(TOKENS_JSON).expect("tokens.json is JSON");
        TokensSpec::from_json(&v).expect("tokens.json is a valid token spec")
    })
}

/// A shipped built-in theme document (`oled`, `goose`).
pub fn builtin_theme(id: &str) -> Option<&'static Value> {
    static DOCS: std::sync::OnceLock<Vec<(&'static str, Value)>> = std::sync::OnceLock::new();
    let docs = DOCS.get_or_init(|| {
        BUILTIN_JSON
            .iter()
            .map(|(id, text)| (*id, crate::json::parse_json(text).expect("builtin theme is JSON")))
            .collect()
    });
    docs.iter().find(|(k, _)| *k == id).map(|(_, v)| v)
}

/// Structural equality of JSON data: same keys (any order), same strings, numbers equal as doubles, same arrays.
fn same_json(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Null, Value::Null) => true,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        (Value::String(x), Value::String(y)) => x == y,
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same_json(p, q)),
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same_json(v, w)))
        }
        _ => false,
    }
}

/// A theme is a built-in when its id names one and it is structurally equal to that shipped document.
pub fn builtin_id(theme: &Value) -> Option<&'static str> {
    let id = theme.as_object()?.get("id")?.as_str()?;
    let (bid, _) = BUILTIN_JSON.iter().find(|(k, _)| *k == id)?;
    same_json(theme, builtin_theme(bid)?).then_some(*bid)
}

/// What the apps call (ENGINE.md §9, ENGINE-QUESTIONS D1): `resolve_theme`, then — for the web only, and only when
/// the theme IS a shipped built-in — every token with a `webPin` for that built-in takes the pinned colour. On the
/// desktop this is `resolve_theme` + `pinned: []`.
pub fn resolve_for_app(theme: &Value, spec: &TokensSpec, app: App) -> Result<AppResolved, Refusal> {
    let mut r = resolve_theme(theme, spec)?;
    let mut pinned = Vec::new();
    if app == App::Web {
        if let Some(bid) = builtin_id(theme) {
            for id in &spec.ids {
                if let Some(pin) = spec.web_pin(id, bid) {
                    r.tokens.insert(id.clone(), format_colour(pin));
                    pinned.push(id.clone());
                }
            }
        }
    }
    Ok(AppResolved { resolved: r, pinned })
}

/// The reference's signature: `resolveForApp(theme, tokensSpec, app)` with JSON arguments, checked in its order
/// (app, then the spec, then the theme).
pub fn resolve_for_app_json(theme: &Value, tokens_spec: &Value, app: &Value) -> Result<AppResolved, Refusal> {
    let app = App::parse(app.as_str().unwrap_or(""))?;
    let spec = TokensSpec::from_json(tokens_spec)?;
    resolve_for_app(theme, &spec, app)
}
