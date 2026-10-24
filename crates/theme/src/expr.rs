//! expr.rs — port of `expr.ts` (ENGINE.md §3): the expression language, parser only (evaluation: engine.rs).
//!
//! ```text
//!   expr   := literal | ref | fn '(' item (',' item)* ')'
//!   item   := expr | number
//!   ref    := [A-Za-z][A-Za-z0-9.:-]*            (role id, token id or palette slot)
//!   number := D{1,4} | D{1,4}.D{1,4} | .D{1,4}      (exact: kept as k = value × 10000)
//!   whitespace (space, tab, CR, LF) is allowed between tokens.
//! ```
//! Signatures: alpha(e, n) mix(e, e, n) lighter(e, n) darker(e, n) over(e, e) mixok(e, e, n) best(e, e, …)
//! guard(e, e, n); n ranges: alpha/mix/mixok 0..1, guard 1..21, lighter/darker 0..9999.9999.
//! Errors are the reference's fixed message set (README "Expression errors"), byte for byte.

use crate::literal::{parse_decimal4, parse_literal, Colour};

/// The functions of the language.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FnName {
    Alpha,
    Mix,
    Lighter,
    Darker,
    Over,
    Mixok,
    Best,
    Guard,
}

impl FnName {
    pub fn as_str(self) -> &'static str {
        match self {
            FnName::Alpha => "alpha",
            FnName::Mix => "mix",
            FnName::Lighter => "lighter",
            FnName::Darker => "darker",
            FnName::Over => "over",
            FnName::Mixok => "mixok",
            FnName::Best => "best",
            FnName::Guard => "guard",
        }
    }

    fn from_name(s: &str) -> Option<FnName> {
        Some(match s {
            "alpha" => FnName::Alpha,
            "mix" => FnName::Mix,
            "lighter" => FnName::Lighter,
            "darker" => FnName::Darker,
            "over" => FnName::Over,
            "mixok" => FnName::Mixok,
            "best" => FnName::Best,
            "guard" => FnName::Guard,
            _ => return None,
        })
    }

    /// (arity (-1 = variadic, at least 2 colours), index of the numeric argument (-1 = none), allowed k range)
    fn sig(self) -> (i32, i32, i64, i64) {
        match self {
            FnName::Alpha => (2, 1, 0, 10000),
            FnName::Mix => (3, 2, 0, 10000),
            FnName::Lighter => (2, 1, 0, 99999999),
            FnName::Darker => (2, 1, 0, 99999999),
            FnName::Over => (2, -1, 0, 0),
            FnName::Mixok => (3, 2, 0, 10000),
            FnName::Best => (-1, -1, 0, 0),
            FnName::Guard => (3, 2, 10000, 210000),
        }
    }
}

/// A parsed expression.
#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Lit(Colour),
    Ref(String),
    /// `k` = the numeric argument × 10000 (0 if the function has none); the reference's `n` is `k / 10000`.
    Fn { f: FnName, args: Vec<Expr>, k: i64 },
}

/// Maximum nesting of function calls in one expression.
pub const MAX_DEPTH: usize = 32;

enum Item {
    Num(i64),
    Expr(Expr),
}

fn is_ws(ch: u8) -> bool {
    matches!(ch, b' ' | b'\t' | b'\n' | b'\r')
}
fn is_letter(ch: u8) -> bool {
    ch.is_ascii_alphabetic()
}
fn is_id_char(ch: u8) -> bool {
    ch.is_ascii_alphanumeric() || ch == b'.' || ch == b':' || ch == b'-'
}
/// literal/number token characters
fn is_word_char(ch: u8) -> bool {
    ch.is_ascii_alphanumeric() || ch == b'.' || ch == b'/'
}

struct Parser<'a> {
    s: &'a str,
    b: &'a [u8],
    i: usize,
}

type Fail = String;

impl<'a> Parser<'a> {
    fn ws(&mut self) {
        while self.i < self.b.len() && is_ws(self.b[self.i]) {
            self.i += 1;
        }
    }

    /// "unexpected 'X'" for the code point at i, or "unexpected end". (i is always on a char boundary: the parser
    /// only ever steps over ASCII.)
    fn unexpected(&self) -> Fail {
        match self.s[self.i..].chars().next() {
            None => "unexpected end".into(),
            Some(c) => format!("unexpected '{c}'"),
        }
    }

    fn word(&mut self, start: usize) -> &'a str {
        while self.i < self.b.len() && is_word_char(self.b[self.i]) {
            self.i += 1;
        }
        &self.s[start..self.i]
    }

    fn expr(&mut self, depth: usize) -> Result<Expr, Fail> {
        self.ws();
        if self.i >= self.b.len() {
            return Err(self.unexpected());
        }
        let ch = self.b[self.i];
        if ch == b'#' {
            let start = self.i;
            self.i += 1;
            self.word(self.i); // hex digits, letters, '/', '.' — the whole token is judged by parse_literal
            let tok = &self.s[start..self.i];
            return match parse_literal(tok) {
                Some(c) => Ok(Expr::Lit(c)),
                None => Err(format!("bad colour literal '{tok}'")),
            };
        }
        if !is_letter(ch) {
            return Err(self.unexpected());
        }
        let start = self.i;
        while self.i < self.b.len() && is_id_char(self.b[self.i]) {
            self.i += 1;
        }
        let name = &self.s[start..self.i];
        self.ws();
        if self.i < self.b.len() && self.b[self.i] == b'(' {
            let Some(f) = FnName::from_name(name) else { return Err(format!("unknown function '{name}'")) };
            if depth + 1 > MAX_DEPTH {
                return Err("too deeply nested".into());
            }
            self.i += 1;
            return self.call(f, depth + 1);
        }
        Ok(Expr::Ref(name.to_string()))
    }

    fn item(&mut self, depth: usize) -> Result<Item, Fail> {
        self.ws();
        if self.i >= self.b.len() {
            return Err(self.unexpected());
        }
        let ch = self.b[self.i];
        if ch.is_ascii_digit() || ch == b'.' {
            let start = self.i;
            let tok = self.word(start);
            let k = parse_decimal4(tok);
            if k < 0 {
                return Err(format!("bad number '{tok}'"));
            }
            return Ok(Item::Num(k));
        }
        Ok(Item::Expr(self.expr(depth)?))
    }

    fn call(&mut self, f: FnName, depth: usize) -> Result<Expr, Fail> {
        let mut items: Vec<Item> = Vec::new();
        self.ws();
        if self.i < self.b.len() && self.b[self.i] == b')' {
            self.i += 1;
        } else {
            loop {
                items.push(self.item(depth)?);
                self.ws();
                if self.i >= self.b.len() {
                    return Err(self.unexpected());
                }
                match self.b[self.i] {
                    b',' => {
                        self.i += 1;
                        continue;
                    }
                    b')' => {
                        self.i += 1;
                        break;
                    }
                    _ => return Err(self.unexpected()),
                }
            }
        }
        let name = f.as_str();
        let (arity, num, lo, hi) = f.sig();
        if arity < 0 {
            if items.len() < 2 {
                return Err(format!("{name}() takes at least 2 arguments"));
            }
        } else if items.len() != arity as usize {
            return Err(format!("{name}() takes {arity} arguments"));
        }
        let mut args = Vec::with_capacity(items.len());
        let mut k = 0;
        for (j, it) in items.into_iter().enumerate() {
            if j as i32 == num {
                let Item::Num(v) = it else { return Err(format!("{name}() argument {} must be a number", j + 1)) };
                if v < lo || v > hi {
                    return Err(format!("{name}() argument {} out of range", j + 1));
                }
                k = v;
            } else {
                match it {
                    Item::Num(_) => return Err(format!("{name}() argument {} must be a colour", j + 1)),
                    Item::Expr(e) => args.push(e),
                }
            }
        }
        Ok(Expr::Fn { f, args, k })
    }
}

/// Parse one expression. `Err` carries the reference's fixed message ("unexpected end", "bad number '…'", …).
pub fn parse_expr(src: &str) -> Result<Expr, String> {
    let mut p = Parser { s: src, b: src.as_bytes(), i: 0 };
    p.ws();
    if p.i >= src.len() {
        return Err("empty expression".into());
    }
    let e = p.expr(0)?;
    p.ws();
    if p.i < src.len() {
        return Err(p.unexpected());
    }
    Ok(e)
}
