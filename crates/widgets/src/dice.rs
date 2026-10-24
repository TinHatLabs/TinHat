//! dice — the Dice roller's shared logic (contract WIDGETS.md §16 Dice, vectors testvectors/widgets/dice/*). A
//! line-for-line port of the web's `src/widgets/dice.ts` (widgets W3, 2026-09-28): the expression grammar and its
//! friendly errors, raw die values → a result (keep highest / lowest, the total, the natural 20 / 1, the Advantage /
//! Disadvantage label), the unbiased mapping of a random 32-bit word onto a die (rejection sampling — the app draws the
//! words from the OS CSPRNG), the room message (plain + HTML body + the structured `dev.tinhat.dice` field), reading the
//! field back (a card only when everything re-adds up), the private-roll list, the history order and the Small line.
//! Strings are cut and compared in code points (Rust's `str` order), whitespace is ASCII only, numbers are doubles.

use std::collections::HashSet;

use serde_json::{json, Map, Value};

/// An expression longer than this many code points is refused (E_LONG).
pub const DICE_EXPR_MAX: usize = 100;
/// At most this many dice in one roll.
pub const DICE_MAX: u32 = 100;
/// The most sides a die may have.
pub const SIDES_MAX: u32 = 1000;
/// A constant term's largest value.
pub const NUMBER_MAX: u32 = 10000;
/// At most this many terms.
pub const TERMS_MAX: usize = 20;
/// The private-roll list keeps the newest this many.
pub const PRIVATE_MAX: usize = 50;
/// The wire field (inside the room message's content).
pub const DICE_FIELD: &str = "dev.tinhat.dice";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keep {
    All,
    Kh,
    Kl,
}

impl Keep {
    pub fn as_str(self) -> &'static str {
        match self {
            Keep::All => "all",
            Keep::Kh => "kh",
            Keep::Kl => "kl",
        }
    }
}

/// A term: dice (`count` d `sides`, keeping `n`) or a number (`value`); `sign` is 1 or -1.
#[derive(Clone, Debug, PartialEq)]
pub struct Term {
    pub dice: bool,
    pub sign: i64,
    pub count: u32,
    pub sides: u32,
    pub keep: Keep,
    pub n: u32,
    pub value: u32,
}

impl Term {
    fn to_json(&self) -> Value {
        if self.dice {
            json!({ "kind": "dice", "sign": self.sign, "count": self.count, "sides": self.sides, "keep": self.keep.as_str(), "n": self.n })
        } else {
            json!({ "kind": "num", "sign": self.sign, "value": self.value })
        }
    }
    fn text(&self) -> String {
        if !self.dice {
            return self.value.to_string();
        }
        let k = match self.keep {
            Keep::Kh => format!("kh{}", self.n),
            Keep::Kl => format!("kl{}", self.n),
            Keep::All => String::new(),
        };
        format!("{}d{}{}", self.count, self.sides, k)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Parsed {
    pub expr: String,
    pub terms: Vec<Term>,
    pub dice: u32,
    pub label: &'static str,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DiceError {
    pub code: &'static str,
    pub message: String,
}

/// The friendly text of an error (E_KEEP names the term's dice count).
pub fn error_text(code: &str, count: u32) -> String {
    match code {
        "E_EMPTY" => "Type a roll, like 2d6+3".into(),
        "E_LONG" => "That's too long — 100 characters at most".into(),
        "E_SYNTAX" => "Can't read that — try 2d6+3, 4d6kh3 or 2d20kh1".into(),
        "E_COUNT" => "Roll at least 1 die".into(),
        "E_TOO_MANY" => "That's too many dice — 100 at most".into(),
        "E_SIDES_FEW" => "A die needs at least 2 sides".into(),
        "E_SIDES_MANY" => "Dice go up to 1000 sides".into(),
        "E_KEEP" => {
            if count == 1 {
                "You can only keep 1 die".into()
            } else {
                format!("Keep between 1 and {count} dice")
            }
        }
        "E_NUMBER" => "Numbers go up to 10000".into(),
        "E_TERMS" => "That's too many parts — 20 at most".into(),
        _ => "Add a die, like 1d20".into(),
    }
}

fn fail(code: &'static str, count: u32) -> Result<Parsed, DiceError> {
    Err(DiceError { code, message: error_text(code, count) })
}

/// The canonical expression of a term list.
pub fn canonical(terms: &[Term]) -> String {
    let mut s = String::new();
    for (i, t) in terms.iter().enumerate() {
        if i == 0 {
            if t.sign < 0 {
                s.push('-');
            }
        } else {
            s.push(if t.sign < 0 { '-' } else { '+' });
        }
        s.push_str(&t.text());
    }
    s
}

fn label_of(terms: &[Term]) -> &'static str {
    let d20: Vec<&Term> = terms.iter().filter(|t| t.dice && t.sides == 20).collect();
    if d20.len() != 1 {
        return "";
    }
    let t = d20[0];
    if t.sign != 1 || t.count != 2 || t.n != 1 {
        return "";
    }
    match t.keep {
        Keep::Kh => "Advantage",
        Keep::Kl => "Disadvantage",
        Keep::All => "",
    }
}

fn is_ws(c: char) -> bool {
    c == ' ' || ('\t'..='\r').contains(&c)
}

/// Parse a roll expression (the web's `parseDice`; grammar and check order in WIDGETS.md §16.1).
pub fn parse_dice(input: &str) -> Result<Parsed, DiceError> {
    let cs: Vec<char> = input.chars().collect();
    if cs.len() > DICE_EXPR_MAX {
        return fail("E_LONG", 0);
    }
    if cs.iter().all(|c| is_ws(*c)) {
        return fail("E_EMPTY", 0);
    }
    let mut i = 0usize;
    let skip_ws = |i: &mut usize| {
        while *i < cs.len() && is_ws(cs[*i]) {
            *i += 1;
        }
    };
    let digits = |i: &mut usize| -> Option<u32> {
        if *i >= cs.len() || !cs[*i].is_ascii_digit() {
            return None;
        }
        let mut v: u64 = 0;
        while *i < cs.len() && cs[*i].is_ascii_digit() {
            v = (v * 10 + (cs[*i] as u64 - 48)).min(1_000_000_000);
            *i += 1;
        }
        Some(v as u32)
    };
    let sign = |i: &mut usize| -> i64 {
        match cs.get(*i) {
            Some('+') => {
                *i += 1;
                1
            }
            Some('-') | Some('\u{2212}') => {
                *i += 1;
                -1
            }
            _ => 0,
        }
    };
    let mut terms: Vec<Term> = Vec::new();
    let mut dice: u32 = 0;
    skip_ws(&mut i);
    let mut sg: i64 = 1;
    let first = sign(&mut i);
    if first != 0 {
        sg = first;
        skip_ws(&mut i);
    }
    loop {
        let lead = digits(&mut i);
        match cs.get(i) {
            Some('d') | Some('D') => {
                i += 1;
                let sides = if cs.get(i) == Some(&'%') {
                    i += 1;
                    Some(100)
                } else {
                    digits(&mut i)
                };
                let Some(sides) = sides else { return fail("E_SYNTAX", 0) };
                let mut keep = Keep::All;
                let mut n: i64 = -1;
                if matches!(cs.get(i), Some('k') | Some('K')) {
                    i += 1;
                    match cs.get(i) {
                        Some('h') | Some('H') => {
                            i += 1;
                            keep = Keep::Kh;
                        }
                        Some('l') | Some('L') => {
                            i += 1;
                            keep = Keep::Kl;
                        }
                        _ => keep = Keep::Kh,
                    }
                    let Some(kn) = digits(&mut i) else { return fail("E_SYNTAX", 0) };
                    n = kn as i64;
                }
                let count = lead.unwrap_or(1);
                if count < 1 {
                    return fail("E_COUNT", 0);
                }
                if count > DICE_MAX {
                    return fail("E_TOO_MANY", 0);
                }
                if sides < 2 {
                    return fail("E_SIDES_FEW", 0);
                }
                if sides > SIDES_MAX {
                    return fail("E_SIDES_MANY", 0);
                }
                if keep == Keep::All {
                    n = count as i64;
                } else if n < 1 || n > count as i64 {
                    return fail("E_KEEP", count);
                }
                terms.push(Term { dice: true, sign: sg, count, sides, keep, n: n as u32, value: 0 });
                dice += count;
                if dice > DICE_MAX {
                    return fail("E_TOO_MANY", 0);
                }
            }
            _ => {
                let Some(v) = lead else { return fail("E_SYNTAX", 0) };
                if v > NUMBER_MAX {
                    return fail("E_NUMBER", 0);
                }
                terms.push(Term { dice: false, sign: sg, count: 0, sides: 0, keep: Keep::All, n: 0, value: v });
            }
        }
        if terms.len() > TERMS_MAX {
            return fail("E_TERMS", 0);
        }
        skip_ws(&mut i);
        if i >= cs.len() {
            break;
        }
        let s = sign(&mut i);
        if s == 0 {
            return fail("E_SYNTAX", 0);
        }
        sg = s;
        skip_ws(&mut i);
        if i >= cs.len() {
            return fail("E_SYNTAX", 0);
        }
    }
    if dice == 0 {
        return fail("E_NO_DICE", 0);
    }
    let label = label_of(&terms);
    Ok(Parsed { expr: canonical(&terms), terms, dice, label })
}

/// A parse as the vectors' JSON: {ok: true, expr, terms, dice, label} | {ok: false, code, message}.
pub fn parse_json(r: &Result<Parsed, DiceError>) -> Value {
    match r {
        Ok(p) => json!({ "ok": true, "expr": p.expr, "terms": p.terms.iter().map(Term::to_json).collect::<Vec<_>>(), "dice": p.dice, "label": p.label }),
        Err(e) => json!({ "ok": false, "code": e.code, "message": e.message }),
    }
}

// ---- randomness ---------------------------------------------------------------------------------------------------------

const TWO32: u64 = 1 << 32;

/// An unbiased die from one uniformly random 32-bit word: words at or above the largest multiple of `sides` that fits are
/// rejected (None — draw another word); otherwise (u mod sides) + 1.
pub fn die_from_u32(u: u64, sides: u32) -> Option<u32> {
    let s = sides as u64;
    let limit = TWO32 - (TWO32 % s);
    if u < limit {
        Some((u % s) as u32 + 1)
    } else {
        None
    }
}

/// The raw die values of a parsed roll from a stream of 32-bit words (dice in term order); None when the stream runs
/// out first.
pub fn raw_from_stream(p: &Parsed, stream: &[u64]) -> Option<Vec<u32>> {
    let mut out = Vec::new();
    let mut i = 0usize;
    for t in p.terms.iter().filter(|t| t.dice) {
        for _ in 0..t.count {
            loop {
                let u = *stream.get(i)?;
                i += 1;
                if let Some(v) = die_from_u32(u, t.sides) {
                    out.push(v);
                    break;
                }
            }
        }
    }
    Some(out)
}

/// Roll a parsed expression with `next` (a source of uniformly random 32-bit words — the app passes the OS CSPRNG).
pub fn roll_with(p: &Parsed, mut next: impl FnMut() -> u32) -> DiceResult {
    let mut raw: Vec<Value> = Vec::new();
    for t in p.terms.iter().filter(|t| t.dice) {
        for _ in 0..t.count {
            let v = loop {
                if let Some(v) = die_from_u32(next() as u64, t.sides) {
                    break v;
                }
            };
            raw.push(json!(v));
        }
    }
    evaluate_dice(p, &raw).expect("rolled values always fit")
}

// ---- evaluation ---------------------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub dice: bool,
    pub sign: i64,
    pub count: u32,
    pub sides: u32,
    pub keep: Keep,
    pub n: u32,
    pub rolls: Vec<u32>,
    pub kept: Vec<bool>,
    pub sum: i64,
    pub value: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DiceResult {
    pub expr: String,
    pub label: &'static str,
    pub total: i64,
    pub groups: Vec<Group>,
    pub rolls: Vec<u32>,
    pub kept: Vec<bool>,
    pub natural: &'static str,
}

impl DiceResult {
    pub fn to_json(&self) -> Value {
        let groups: Vec<Value> = self
            .groups
            .iter()
            .map(|g| {
                if g.dice {
                    json!({ "kind": "dice", "sign": g.sign, "count": g.count, "sides": g.sides, "keep": g.keep.as_str(), "n": g.n,
                            "rolls": g.rolls, "kept": g.kept, "sum": g.sum })
                } else {
                    json!({ "kind": "num", "sign": g.sign, "value": g.value })
                }
            })
            .collect();
        json!({ "expr": self.expr, "label": self.label, "total": self.total, "groups": groups, "rolls": self.rolls,
                "kept": self.kept, "natural": self.natural })
    }
}

fn kept_of(rolls: &[u32], keep: Keep, n: u32) -> Vec<bool> {
    if keep == Keep::All {
        return vec![true; rolls.len()];
    }
    let mut idx: Vec<usize> = (0..rolls.len()).collect();
    idx.sort_by(|&a, &b| {
        let by = if keep == Keep::Kh { rolls[b].cmp(&rolls[a]) } else { rolls[a].cmp(&rolls[b]) };
        by.then(a.cmp(&b))
    });
    let mut kept = vec![false; rolls.len()];
    for j in idx.iter().take(n as usize) {
        kept[*j] = true;
    }
    kept
}

/// An integer die value (a JSON number that is integral, 1..=sides).
fn die_value(v: &Value, sides: u32) -> Option<u32> {
    let x = v.as_f64()?;
    if !v.is_number() || x.fract() != 0.0 || x < 1.0 || x > sides as f64 {
        return None;
    }
    Some(x as u32)
}

/// A parsed roll + its raw die values (JSON numbers) → the result; None when the values don't fit.
pub fn evaluate_dice(p: &Parsed, raw: &[Value]) -> Option<DiceResult> {
    if raw.len() != p.dice as usize {
        return None;
    }
    let mut groups = Vec::new();
    let mut rolls = Vec::new();
    let mut kept = Vec::new();
    let mut total: i64 = 0;
    let mut pos = 0usize;
    for t in &p.terms {
        if !t.dice {
            groups.push(Group { dice: false, sign: t.sign, count: 0, sides: 0, keep: Keep::All, n: 0, rolls: vec![], kept: vec![], sum: 0, value: t.value });
            total += t.sign * t.value as i64;
            continue;
        }
        let mut rs = Vec::new();
        for _ in 0..t.count {
            rs.push(die_value(&raw[pos], t.sides)?);
            pos += 1;
        }
        let ks = kept_of(&rs, t.keep, t.n);
        let sum: i64 = rs.iter().zip(&ks).filter(|(_, k)| **k).map(|(v, _)| *v as i64).sum();
        total += t.sign * sum;
        rolls.extend(rs.iter().copied());
        kept.extend(ks.iter().copied());
        groups.push(Group { dice: true, sign: t.sign, count: t.count, sides: t.sides, keep: t.keep, n: t.n, rolls: rs, kept: ks, sum, value: 0 });
    }
    let d20: Vec<&Group> = groups.iter().filter(|g| g.dice && g.sides == 20).collect();
    let mut natural = "";
    if d20.len() == 1 && d20[0].sign == 1 && d20[0].n == 1 {
        let g = d20[0];
        let i = g.kept.iter().position(|k| *k).unwrap_or(0);
        natural = match g.rolls[i] {
            20 => "nat20",
            1 => "nat1",
            _ => "",
        };
    }
    Some(DiceResult { expr: p.expr.clone(), label: p.label, total, groups, rolls, kept, natural })
}

// ---- the room message -----------------------------------------------------------------------------------------------------

fn parts_of(r: &DiceResult) -> (Vec<(i64, i64)>, Vec<u32>) {
    let mut parts = Vec::new();
    let mut dropped = Vec::new();
    for g in &r.groups {
        if !g.dice {
            parts.push((g.sign, g.value as i64));
        } else {
            for (v, k) in g.rolls.iter().zip(&g.kept) {
                if *k {
                    parts.push((g.sign, *v as i64));
                } else {
                    dropped.push(*v);
                }
            }
        }
    }
    (parts, dropped)
}

fn sum_text(parts: &[(i64, i64)]) -> String {
    let mut s = String::new();
    for (i, (sign, v)) in parts.iter().enumerate() {
        if i == 0 {
            s.push_str(&if *sign < 0 { format!("-{v}") } else { v.to_string() });
        } else {
            s.push_str(if *sign < 0 { " - " } else { " + " });
            s.push_str(&v.to_string());
        }
    }
    s
}

fn label_part(r: &DiceResult) -> String {
    if r.label.is_empty() {
        String::new()
    } else {
        format!(" ({})", r.label.to_ascii_lowercase())
    }
}

/// The plain body every client shows (the web's `diceBody`).
pub fn dice_body(r: &DiceResult) -> String {
    let (parts, dropped) = parts_of(r);
    let bd = if parts.len() > 1 || !dropped.is_empty() {
        let d = if dropped.is_empty() {
            String::new()
        } else {
            format!(", dropped {}", dropped.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(", "))
        };
        format!(" ({}{})", sum_text(&parts), d)
    } else {
        String::new()
    };
    let nat = match r.natural {
        "nat20" => " — natural 20!",
        "nat1" => " — natural 1",
        _ => "",
    };
    format!("🎲 rolled {}{} → {}{}{}", r.expr, label_part(r), r.total, bd, nat)
}

/// The same line as HTML (the web's `diceHtml`).
pub fn dice_html(r: &DiceResult) -> String {
    let (parts, dropped) = parts_of(r);
    let bd = if parts.len() > 1 || !dropped.is_empty() {
        let d = if dropped.is_empty() {
            String::new()
        } else {
            format!(", dropped {}", dropped.iter().map(|v| format!("<del>{v}</del>")).collect::<Vec<_>>().join(", "))
        };
        format!(" ({}{})", sum_text(&parts), d)
    } else {
        String::new()
    };
    let nat = match r.natural {
        "nat20" => " — <strong>natural 20!</strong>",
        "nat1" => " — <strong>natural 1</strong>",
        _ => "",
    };
    format!("🎲 rolled <code>{}</code>{} → <strong>{}</strong>{}{}", r.expr, label_part(r), r.total, bd, nat)
}

/// The structured field {v: 1, expr, rolls, kept, total}.
pub fn dice_field(r: &DiceResult) -> Value {
    json!({ "v": 1, "expr": r.expr, "rolls": r.rolls, "kept": r.kept, "total": r.total })
}

/// The room message's content (m.notice + bodies + `m.mentions: {}` + the field; the web's `diceContent`).
pub fn dice_content(r: &DiceResult) -> Value {
    let mut m = Map::new();
    m.insert("msgtype".into(), json!("m.notice"));
    m.insert("body".into(), json!(dice_body(r)));
    m.insert("format".into(), json!("org.matrix.custom.html"));
    m.insert("formatted_body".into(), json!(dice_html(r)));
    m.insert("m.mentions".into(), json!({}));
    m.insert(DICE_FIELD.into(), dice_field(r));
    Value::Object(m)
}

/// A message content → its dice result, or None (the web's `readDice`: v === 1, a parsing expression, exactly that many
/// in-range integer rolls, the total they add up to, `kept` (when present) equal to what they give).
pub fn read_dice(content: &Value) -> Option<DiceResult> {
    let d = content.as_object()?.get(DICE_FIELD)?.as_object()?;
    if !d.get("v").is_some_and(|v| v.is_number() && v.as_f64() == Some(1.0)) {
        return None;
    }
    let expr = d.get("expr")?.as_str()?;
    let rolls = d.get("rolls")?.as_array()?;
    let p = parse_dice(expr).ok()?;
    let r = evaluate_dice(&p, rolls)?;
    let t = d.get("total")?;
    if !t.is_number() || t.as_f64() != Some(r.total as f64) {
        return None;
    }
    if let Some(k) = d.get("kept") {
        let k = k.as_array()?;
        if k.len() != r.kept.len() || k.iter().zip(&r.kept).any(|(x, y)| x != &Value::Bool(*y)) {
            return None;
        }
    }
    Some(r)
}

/// Advantage / disadvantage of an expression (the web's `withAdvantage`).
pub fn with_advantage(expr: &str, adv: bool) -> String {
    let plain = if adv { "2d20kh1" } else { "2d20kl1" }.to_string();
    let Ok(p) = parse_dice(expr) else { return plain };
    let d20: Vec<usize> = p.terms.iter().enumerate().filter(|(_, t)| t.dice && t.sides == 20).map(|(i, _)| i).collect();
    if d20.len() != 1 {
        return plain;
    }
    let t = &p.terms[d20[0]];
    if t.sign != 1 {
        return plain;
    }
    let ok = (t.count == 1 && t.keep == Keep::All) || (t.count == 2 && t.n == 1);
    if !ok || p.dice - t.count + 2 > DICE_MAX {
        return plain;
    }
    let mut terms = p.terms.clone();
    terms[d20[0]] = Term { dice: true, sign: 1, count: 2, sides: 20, keep: if adv { Keep::Kh } else { Keep::Kl }, n: 1, value: 0 };
    canonical(&terms)
}

// ---- private rolls + history ------------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct PrivateRoll {
    pub id: String,
    pub ts: f64,
    pub expr: String,
    pub rolls: Vec<u32>,
    pub total: i64,
}

impl PrivateRoll {
    pub fn to_json(&self) -> Value {
        json!({ "id": self.id, "ts": crate::num(self.ts), "expr": self.expr, "rolls": self.rolls, "total": self.total })
    }
    fn result(&self) -> Option<DiceResult> {
        read_dice(&json!({ DICE_FIELD: { "v": 1, "expr": self.expr, "rolls": self.rolls, "total": self.total } }))
    }
}

fn field_of(expr: Option<&Value>, rolls: Option<&Value>, total: Option<&Value>) -> Value {
    let mut d = Map::new();
    d.insert("v".into(), json!(1));
    if let Some(x) = expr {
        d.insert("expr".into(), x.clone());
    }
    if let Some(x) = rolls {
        d.insert("rolls".into(), x.clone());
    }
    if let Some(x) = total {
        d.insert("total".into(), x.clone());
    }
    let mut c = Map::new();
    c.insert(DICE_FIELD.into(), Value::Object(d));
    Value::Object(c)
}

fn order_by(a_ts: f64, a_id: &str, b_ts: f64, b_id: &str) -> std::cmp::Ordering {
    b_ts.partial_cmp(&a_ts).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a_id.cmp(b_id))
}

/// A stored private list → the list (the web's `normalizePrivate`).
pub fn normalize_private(v: &Value) -> Vec<PrivateRoll> {
    let Some(list) = v.as_array() else { return Vec::new() };
    let mut out: Vec<PrivateRoll> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for it in list {
        let Some(o) = it.as_object() else { continue };
        let Some(id) = o.get("id").and_then(Value::as_str) else { continue };
        let len = id.chars().count();
        if !(1..=64).contains(&len) || seen.contains(id) {
            continue;
        }
        let Some(ts) = o.get("ts").filter(|t| t.is_number()).and_then(Value::as_f64).filter(|t| t.is_finite() && *t >= 0.0) else { continue };
        let Some(r) = read_dice(&field_of(o.get("expr"), o.get("rolls"), o.get("total"))) else { continue };
        seen.insert(id.to_string());
        out.push(PrivateRoll { id: id.to_string(), ts: ts.floor(), expr: r.expr.clone(), rolls: r.rolls.clone(), total: r.total });
    }
    out.sort_by(|a, b| order_by(a.ts, &a.id, b.ts, &b.id));
    out.truncate(PRIVATE_MAX);
    out
}

pub fn private_json(list: &[PrivateRoll]) -> Value {
    Value::Array(list.iter().map(PrivateRoll::to_json).collect())
}

/// Add a private roll (newest), normalized.
pub fn add_private(list: &[PrivateRoll], roll: &PrivateRoll) -> Vec<PrivateRoll> {
    let mut v = vec![roll.to_json()];
    v.extend(list.iter().map(PrivateRoll::to_json));
    normalize_private(&Value::Array(v))
}

#[derive(Clone, Debug, PartialEq)]
pub struct DiceEntry {
    pub id: String,
    pub ts: f64,
    pub sender: String,
    pub private: bool,
    pub result: DiceResult,
}

impl DiceEntry {
    pub fn to_json(&self) -> Value {
        json!({ "id": self.id, "ts": crate::num(self.ts), "sender": self.sender, "private": self.private, "result": self.result.to_json() })
    }
}

/// The widget's history (the web's `diceHistory`): the room's dice messages ({id, sender, ts, content}) and the private
/// rolls, newest first, each id once, at most `limit` (1–50).
pub fn dice_history(events: &[Value], private: &[PrivateRoll], limit: f64) -> Vec<DiceEntry> {
    let mut out: Vec<DiceEntry> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for e in events {
        let Some(o) = e.as_object() else { continue };
        let (Some(id), Some(sender)) = (o.get("id").and_then(Value::as_str), o.get("sender").and_then(Value::as_str)) else { continue };
        let Some(ts) = o.get("ts").filter(|t| t.is_number()).and_then(Value::as_f64).filter(|t| t.is_finite()) else { continue };
        if id.is_empty() || seen.contains(id) {
            continue;
        }
        let Some(r) = read_dice(o.get("content").unwrap_or(&Value::Null)) else { continue };
        seen.insert(id.to_string());
        out.push(DiceEntry { id: id.to_string(), ts, sender: sender.to_string(), private: false, result: r });
    }
    let priv_json = private_json(private);
    for p in normalize_private(&priv_json) {
        if seen.contains(&p.id) {
            continue;
        }
        let Some(r) = p.result() else { continue };
        seen.insert(p.id.clone());
        out.push(DiceEntry { id: p.id.clone(), ts: p.ts, sender: String::new(), private: true, result: r });
    }
    out.sort_by(|a, b| order_by(a.ts, &a.id, b.ts, &b.id));
    let n = if limit.is_finite() { limit.floor().clamp(1.0, 50.0) as usize } else { 1 };
    out.truncate(n);
    out
}

/// The Small / collapsed line (the web's `diceSummary`).
pub fn dice_summary(e: Option<&DiceEntry>, who: &str) -> String {
    let Some(e) = e else { return "No rolls yet".into() };
    let w = if e.private { "Private" } else { who };
    let nat = match e.result.natural {
        "nat20" => " · nat 20",
        "nat1" => " · nat 1",
        _ => "",
    };
    format!("{} · {} → {}{}", w, e.result.expr, e.result.total, nat)
}

/// A card's dice faces in order: [{v, sides, kept, sign}].
pub fn faces_of(r: &DiceResult) -> Value {
    let mut out = Vec::new();
    for g in r.groups.iter().filter(|g| g.dice) {
        for (v, k) in g.rolls.iter().zip(&g.kept) {
            out.push(json!({ "v": v, "sides": g.sides, "kept": k, "sign": g.sign }));
        }
    }
    Value::Array(out)
}

/// The modifier text after the faces ("+3", "-1", "").
pub fn modifier_of(r: &DiceResult) -> String {
    let nums: Vec<&Group> = r.groups.iter().filter(|g| !g.dice).collect();
    let m: i64 = nums.iter().map(|g| g.sign * g.value as i64).sum();
    if nums.is_empty() || m == 0 {
        String::new()
    } else if m > 0 {
        format!("+{m}")
    } else {
        m.to_string()
    }
}

// ---- the chat samples (the gates' timeline cards; both apps render the same rows) ------------------------------------

/// A sample dice message a gate / review build appends to the open conversation (desktop `TINHAT_CHAT_SAMPLES=dice`, web
/// `?chatsamples=dice` with the debug flag): a local row only, never sent. `mine` = the viewer's own; `at` = today's local
/// time (h, m). The web's `DICE_CHAT_SAMPLES` holds the same list (vector dice/samples.json).
pub struct ChatSample {
    pub id: &'static str,
    pub sender: &'static str,
    pub mine: bool,
    pub at: (u32, u32),
    pub expr: &'static str,
    pub rolls: &'static [u32],
}

pub const DICE_CHAT_SAMPLES: &[ChatSample] = &[
    ChatSample { id: "$w3-sample-dice-1", sender: "@Ana:sample.invalid", mine: false, at: (20, 42), expr: "2d20kh1+5", rolls: &[7, 20] },
    ChatSample { id: "$w3-sample-dice-2", sender: "", mine: true, at: (20, 43), expr: "4d6kh3", rolls: &[3, 6, 3, 2] },
    ChatSample { id: "$w3-sample-dice-3", sender: "@Bo:sample.invalid", mine: false, at: (20, 44), expr: "1d20-1", rolls: &[1] },
    ChatSample {
        id: "$w3-sample-dice-4",
        sender: "",
        mine: true,
        at: (20, 45),
        expr: "20d6+2",
        rolls: &[6, 1, 3, 4, 5, 2, 6, 6, 1, 2, 3, 4, 5, 6, 1, 2, 3, 4, 5, 6],
    },
];

/// The samples as the vector's JSON.
pub fn chat_samples_json() -> Value {
    Value::Array(
        DICE_CHAT_SAMPLES
            .iter()
            .map(|s| json!({ "id": s.id, "sender": s.sender, "mine": s.mine, "at": [s.at.0, s.at.1], "expr": s.expr, "rolls": s.rolls }))
            .collect(),
    )
}

/// A sample's message content (the shared diceContent of its roll).
pub fn sample_content(s: &ChatSample) -> Option<Value> {
    let p = parse_dice(s.expr).ok()?;
    let raw: Vec<Value> = s.rolls.iter().map(|v| json!(v)).collect();
    evaluate_dice(&p, &raw).map(|r| dice_content(&r))
}
