//! parsers.rs — port of `parsers.ts`: the small, dependency-free document parsers the importers need (ENGINE.md §7):
//! JSON with comments, the plist XML of .itermcolors, TOML and a YAML subset, line by line. Scalars stay strings
//! (no number/bool typing) except JSON. Errors carry the reference's fixed texts (line numbers count "\n").
//!
//! Port notes:
//! - documents live in an arena ([`Doc`]): TOML tables keep JavaScript's reference semantics (`current` table,
//!   a header that replaces a scalar), and a document of any depth is dropped without recursion;
//! - objects keep first-insertion key order ([`ObjMap`]), which the base16 importer's key loop depends on;
//! - scanning is byte-wise: every delimiter the reference compares with is ASCII, and a non-ASCII character is
//!   always copied whole, so positions and results equal the reference's UTF-16 ones.
//!
//! Rules shared with the reference (ENGINE.md §9, PORT-QUESTIONS Q1/Q2 — resolved: the reference adopted them):
//! - every key is an ordinary own key: `__proto__`, `constructor`, `toString` … are data (the reference builds
//!   null-prototype objects);
//! - at most [`crate::json::MAX_NESTING`] (128) nested collections — JSON values, TOML arrays + inline tables, YAML
//!   block + flow collections, plist dict/array — the 129th level is the parser's usual error, at the same line.
//!
//! Tests: the reference's parser unit test, mirrored, and the differential cases of tests/data/reference-fuzz.json
//! ("parsers": the reference parser's exact result for generated documents).

use crate::js::{bytes_to_string, find_byte, find_from, is_js_space, js_trim};
use crate::json::{parse_json_with_limit, MAX_NESTING};
use serde_json::Value;
use std::collections::HashMap;

/// A parser failure: `"<code>: <detail>"` (`bad-json: not valid JSON`, `bad-plist: not a property list`,
/// `bad-toml: line N`, `bad-yaml: line N`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub code: &'static str,
    pub detail: String,
}

impl ParseError {
    fn new(code: &'static str, detail: impl Into<String>) -> ParseError {
        ParseError { code, detail: detail.into() }
    }
}

// ---- documents ------------------------------------------------------------------------------------------------

pub(crate) type NodeId = usize;

/// A node of a parsed document. `Raw` = a TOML value that is not a string (number, boolean, date): kept verbatim.
#[derive(Clone, Debug)]
pub(crate) enum Node {
    Null,
    Str(String),
    Raw(String),
    Arr(Vec<NodeId>),
    Obj(ObjMap),
}

/// An object: keys in first-insertion order (JavaScript's `Object.keys` order for non-index keys); assigning an
/// existing key replaces the value in place.
#[derive(Clone, Debug, Default)]
pub(crate) struct ObjMap {
    entries: Vec<(String, NodeId)>,
    index: HashMap<String, usize>,
}

impl ObjMap {
    pub fn get(&self, k: &str) -> Option<NodeId> {
        self.index.get(k).map(|&i| self.entries[i].1)
    }
    pub fn set(&mut self, k: &str, v: NodeId) {
        match self.index.get(k) {
            Some(&i) => self.entries[i].1 = v,
            None => {
                self.index.insert(k.to_string(), self.entries.len());
                self.entries.push((k.to_string(), v));
            }
        }
    }
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|(k, _)| k.as_str())
    }
}

/// A parsed document (arena of nodes + the root).
#[derive(Clone, Debug)]
pub(crate) struct Doc {
    pub nodes: Vec<Node>,
    pub root: NodeId,
}

impl Doc {
    fn new() -> Doc {
        Doc { nodes: Vec::new(), root: 0 }
    }
    pub(crate) fn add(&mut self, n: Node) -> NodeId {
        self.nodes.push(n);
        self.nodes.len() - 1
    }
    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id]
    }
    pub fn obj(&self, id: NodeId) -> Option<&ObjMap> {
        match &self.nodes[id] {
            Node::Obj(o) => Some(o),
            _ => None,
        }
    }
    fn obj_mut(&mut self, id: NodeId) -> &mut ObjMap {
        match &mut self.nodes[id] {
            Node::Obj(o) => o,
            _ => unreachable!("not an object node"),
        }
    }
    /// import.ts `isObj`: an object (not null, not an array, not a raw token).
    pub fn is_obj(&self, id: NodeId) -> bool {
        matches!(self.nodes[id], Node::Obj(_))
    }
    /// An own property.
    pub fn get(&self, id: NodeId, key: &str) -> Option<NodeId> {
        self.obj(id)?.get(key)
    }
    /// import.ts `text`: strings as they are, TOML bare tokens by their text, anything else none.
    pub fn text(&self, id: NodeId) -> Option<&str> {
        match &self.nodes[id] {
            Node::Str(s) | Node::Raw(s) => Some(s),
            _ => None,
        }
    }
    /// JavaScript `String(v ?? "")` of a YAML value: arrays join their elements with "," (null elements → ""),
    /// objects are "[object Object]" — the reference's `jsString` (web d18d688: `String()` itself throws on its
    /// null-prototype documents).
    pub fn js_string(&self, id: NodeId) -> String {
        match &self.nodes[id] {
            Node::Null => String::new(),
            Node::Str(s) | Node::Raw(s) => s.clone(),
            Node::Arr(a) => a.iter().map(|&x| self.js_string(x)).collect::<Vec<_>>().join(","),
            Node::Obj(_) => "[object Object]".into(),
        }
    }
    /// A JSON view (tests only; recursive).
    #[cfg(test)]
    pub fn to_json(&self, id: NodeId) -> Value {
        match &self.nodes[id] {
            Node::Null => Value::Null,
            Node::Str(s) => Value::String(s.clone()),
            Node::Raw(s) => serde_json::json!({ "raw": s }),
            Node::Arr(a) => Value::Array(a.iter().map(|&x| self.to_json(x)).collect()),
            Node::Obj(o) => Value::Object(o.entries.iter().map(|(k, v)| (k.clone(), self.to_json(*v))).collect()),
        }
    }
}

// ---- JSON with comments ---------------------------------------------------------------------------------------

/// JSON that may carry comments (`//` and `#` to end of line, `/* … */`) outside strings and trailing commas —
/// Windows Terminal settings and pasted snippets. Comments and trailing commas are removed, then strict JSON.
pub(crate) fn parse_jsonc(text: &str) -> Result<Value, ParseError> {
    let b = text.as_bytes();
    let n = b.len();
    let mut out: Vec<u8> = Vec::with_capacity(n);
    let mut i = 0;
    while i < n {
        let ch = b[i];
        if ch == b'"' {
            let mut j = i + 1;
            while j < n && b[j] != b'"' {
                j += if b[j] == b'\\' { 2 } else { 1 };
            }
            out.extend_from_slice(&b[i..(j + 1).min(n)]);
            i = j + 1;
        } else if ch == b'#' || (ch == b'/' && b.get(i + 1) == Some(&b'/')) {
            while i < n && b[i] != b'\n' {
                i += 1;
            }
        } else if ch == b'/' && b.get(i + 1) == Some(&b'*') {
            i = match find_from(b, b"*/", i + 2) {
                Some(e) => e + 2,
                None => n,
            };
        } else {
            out.push(ch);
            i += 1;
        }
    }
    // trailing commas: a ',' whose next non-space character is '}' or ']'
    let mut clean: Vec<u8> = Vec::with_capacity(out.len());
    let mut k = 0;
    while k < out.len() {
        let ch = out[k];
        if ch == b'"' {
            let mut j = k + 1;
            while j < out.len() && out[j] != b'"' {
                j += if out[j] == b'\\' { 2 } else { 1 };
            }
            clean.extend_from_slice(&out[k..(j + 1).min(out.len())]);
            k = j + 1;
            continue;
        }
        if ch == b',' {
            let mut j = k + 1;
            while j < out.len() && matches!(out[j], b' ' | b'\t' | b'\n' | b'\r') {
                j += 1;
            }
            if matches!(out.get(j), Some(b'}' | b']')) {
                k += 1;
                continue;
            }
        }
        clean.push(ch);
        k += 1;
    }
    let clean = bytes_to_string(clean);
    parse_json_with_limit(&clean, MAX_NESTING).map_err(|_| ParseError::new("bad-json", "not valid JSON"))
}

// ---- plist XML (.itermcolors) -----------------------------------------------------------------------------------

struct XNode {
    tag: String,
    kids: Vec<usize>,
    text: String,
}

fn bad_plist() -> ParseError {
    ParseError::new("bad-plist", "not a property list")
}

/// `&(#x[0-9a-fA-F]+|#[0-9]+|lt|gt|amp|quot|apos);` → the character (numeric references outside 1..=0x10FFFF or in
/// the surrogate range → nothing); anything else is left as it is. One left-to-right pass.
fn xml_unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut last = 0;
    let mut i = 0;
    while let Some(p) = find_byte(b, b'&', i) {
        match entity_at(b, p + 1) {
            Some((end, repl)) => {
                out.push_str(&s[last..p]);
                out.push_str(&repl);
                last = end;
                i = end;
            }
            None => i = p + 1,
        }
    }
    out.push_str(&s[last..]);
    out
}

/// The entity after a '&' at `at`: (end index after ';', replacement).
fn entity_at(b: &[u8], at: usize) -> Option<(usize, String)> {
    let rest = &b[at..];
    let numeric = |start: usize, radix: u32| -> Option<(usize, String)> {
        let mut j = start;
        let mut cp: u64 = 0;
        while j < b.len() && (b[j] as char).is_digit(radix) {
            cp = (cp * radix as u64 + (b[j] as char).to_digit(radix).unwrap() as u64).min(0x7fff_ffff);
            j += 1;
        }
        if j == start || b.get(j) != Some(&b';') {
            return None;
        }
        let ch = if cp > 0 && cp <= 0x10ffff && !(0xd800..=0xdfff).contains(&cp) {
            char::from_u32(cp as u32).map(String::from).unwrap_or_default()
        } else {
            String::new()
        };
        Some((j + 1, ch))
    };
    if rest.starts_with(b"#x") {
        return numeric(at + 2, 16);
    }
    if rest.starts_with(b"#") {
        return numeric(at + 1, 10);
    }
    for (name, repl) in [("lt;", "<"), ("gt;", ">"), ("amp;", "&"), ("quot;", "\""), ("apos;", "'")] {
        if rest.starts_with(name.as_bytes()) {
            return Some((at + name.len(), repl.to_string()));
        }
    }
    None
}

/// The element tree (iterative; node 0 = the document root).
fn parse_xml(text: &str) -> Result<Vec<XNode>, ParseError> {
    let b = text.as_bytes();
    let n = b.len();
    let mut nodes = vec![XNode { tag: String::new(), kids: Vec::new(), text: String::new() }];
    let mut stack: Vec<usize> = vec![0];
    let mut i = 0;
    while i < n {
        let lt = find_byte(b, b'<', i);
        let chunk = &text[i..lt.unwrap_or(n)];
        if !chunk.is_empty() {
            let top = *stack.last().unwrap();
            let t = xml_unescape(chunk);
            nodes[top].text.push_str(&t);
        }
        let Some(lt) = lt else { break };
        if b[lt..].starts_with(b"<!--") {
            let e = find_from(b, b"-->", lt + 4).ok_or_else(bad_plist)?;
            i = e + 3;
        } else if b[lt..].starts_with(b"<![CDATA[") {
            let e = find_from(b, b"]]>", lt + 9).ok_or_else(bad_plist)?;
            let top = *stack.last().unwrap();
            nodes[top].text.push_str(&text[lt + 9..e]);
            i = e + 3;
        } else if b[lt..].starts_with(b"<?") {
            let e = find_from(b, b"?>", lt + 2).ok_or_else(bad_plist)?;
            i = e + 2;
        } else if b[lt..].starts_with(b"<!") {
            // <!DOCTYPE …> with an optional [internal subset]
            let mut j = lt + 2;
            let mut depth: i64 = 0;
            while j < n && (b[j] != b'>' || depth > 0) {
                if b[j] == b'[' {
                    depth += 1;
                } else if b[j] == b']' {
                    depth -= 1;
                }
                j += 1;
            }
            if j >= n {
                return Err(bad_plist());
            }
            i = j + 1;
        } else {
            let e = find_byte(b, b'>', lt + 1).ok_or_else(bad_plist)?;
            let body = js_trim(&text[lt + 1..e]);
            i = e + 1;
            if let Some(close) = body.strip_prefix('/') {
                let tag = js_trim(close);
                let top = stack.pop();
                match top {
                    Some(t) if nodes[t].tag == tag && !stack.is_empty() => {}
                    _ => return Err(bad_plist()),
                }
            } else {
                let self_close = body.ends_with('/');
                let inner = if self_close { &body[..body.len() - 1] } else { body };
                let inner = js_trim(inner);
                let tag = match inner.find(is_js_space) {
                    Some(p) => &inner[..p],
                    None => inner,
                };
                if tag.is_empty() {
                    return Err(bad_plist());
                }
                nodes.push(XNode { tag: tag.to_string(), kids: Vec::new(), text: String::new() });
                let id = nodes.len() - 1;
                let top = *stack.last().unwrap();
                nodes[top].kids.push(id);
                if !self_close {
                    stack.push(id);
                }
            }
        }
    }
    if stack.len() != 1 {
        return Err(bad_plist());
    }
    Ok(nodes)
}

fn plist_value(x: &[XNode], id: usize, doc: &mut Doc, depth: usize) -> Result<NodeId, ParseError> {
    let node = &x[id];
    match node.tag.as_str() {
        "dict" => {
            if depth > MAX_NESTING {
                return Err(bad_plist());
            }
            let mut o = ObjMap::default();
            let mut k = 0;
            while k + 1 < node.kids.len() {
                let key = &x[node.kids[k]];
                if key.tag != "key" {
                    return Err(bad_plist());
                }
                let v = plist_value(x, node.kids[k + 1], doc, depth + 1)?;
                o.set(&key.text, v);
                k += 2;
            }
            Ok(doc.add(Node::Obj(o)))
        }
        "array" => {
            if depth > MAX_NESTING {
                return Err(bad_plist());
            }
            let mut a = Vec::with_capacity(node.kids.len());
            for &kid in &node.kids {
                a.push(plist_value(x, kid, doc, depth + 1)?);
            }
            Ok(doc.add(Node::Arr(a)))
        }
        "true" => Ok(doc.add(Node::Str("true".into()))),
        "false" => Ok(doc.add(Node::Str("false".into()))),
        _ => Ok(doc.add(Node::Str(js_trim(&node.text).to_string()))), // real, integer, string, date, data: text
    }
}

/// A plist document → its top value (a dict becomes an object whose values are objects/arrays/strings).
pub(crate) fn parse_plist(text: &str) -> Result<Doc, ParseError> {
    let x = parse_xml(text)?;
    let plist = x[0].kids.iter().copied().find(|&k| x[k].tag == "plist").ok_or_else(bad_plist)?;
    let Some(&first) = x[plist].kids.first() else { return Err(bad_plist()) };
    let mut doc = Doc::new();
    doc.root = plist_value(&x, first, &mut doc, 1)?;
    Ok(doc)
}

// ---- TOML -----------------------------------------------------------------------------------------------------

struct Toml<'a> {
    s: &'a str,
    b: &'a [u8],
    i: usize,
    line: usize,
    doc: Doc,
    depth: usize,
}

fn is_bare_key_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'-'
}

impl<'a> Toml<'a> {
    fn fail<T>(&self) -> Result<T, ParseError> {
        Err(ParseError::new("bad-toml", format!("line {}", self.line)))
    }
    fn peek(&self, k: usize) -> Option<u8> {
        self.b.get(self.i + k).copied()
    }
    fn starts_with(&self, pat: &[u8]) -> bool {
        self.b[self.i.min(self.b.len())..].starts_with(pat)
    }
    /// spaces and tabs
    fn ws(&mut self) {
        while matches!(self.peek(0), Some(b' ' | b'\t')) {
            self.i += 1;
        }
    }
    /// spaces, tabs, newlines and comments
    fn wsnl(&mut self) {
        loop {
            match self.peek(0) {
                Some(b' ' | b'\t' | b'\r') => self.i += 1,
                Some(b'\n') => {
                    self.i += 1;
                    self.line += 1;
                }
                Some(b'#') => {
                    while self.i < self.b.len() && self.peek(0) != Some(b'\n') {
                        self.i += 1;
                    }
                }
                _ => return,
            }
        }
    }
    /// after a statement: optional spaces, optional comment, then newline or end
    fn eol(&mut self) -> Result<(), ParseError> {
        self.ws();
        if self.peek(0) == Some(b'#') {
            while self.i < self.b.len() && self.peek(0) != Some(b'\n') {
                self.i += 1;
            }
        }
        if self.peek(0) == Some(b'\r') {
            self.i += 1;
        }
        if self.i >= self.b.len() {
            return Ok(());
        }
        if self.peek(0) != Some(b'\n') {
            return self.fail();
        }
        self.i += 1;
        self.line += 1;
        Ok(())
    }
    fn key(&mut self) -> Result<Vec<String>, ParseError> {
        let mut parts = Vec::new();
        loop {
            self.ws();
            match self.peek(0) {
                Some(b'"') => parts.push(self.basic()?),
                Some(b'\'') => parts.push(self.literal()?),
                _ => {
                    let start = self.i;
                    while self.peek(0).is_some_and(is_bare_key_char) {
                        self.i += 1;
                    }
                    if self.i == start {
                        return self.fail();
                    }
                    parts.push(self.s[start..self.i].to_string());
                }
            }
            self.ws();
            if self.peek(0) != Some(b'.') {
                return Ok(parts);
            }
            self.i += 1;
        }
    }
    fn basic(&mut self) -> Result<String, ParseError> {
        self.i += 1; // "
        let mut out: Vec<u8> = Vec::new();
        loop {
            let c = match self.peek(0) {
                None | Some(b'\n') => return self.fail(),
                Some(c) => c,
            };
            self.i += 1;
            if c == b'"' {
                return Ok(bytes_to_string(out));
            }
            if c == b'\\' {
                out.extend_from_slice(self.escape()?.as_bytes());
            } else {
                out.push(c);
            }
        }
    }
    fn escape(&mut self) -> Result<String, ParseError> {
        let e = self.peek(0);
        self.i += 1;
        let simple = match e {
            Some(b'b') => Some("\u{8}"),
            Some(b't') => Some("\t"),
            Some(b'n') => Some("\n"),
            Some(b'f') => Some("\u{c}"),
            Some(b'r') => Some("\r"),
            Some(b'e') => Some("\u{1b}"),
            Some(b'"') => Some("\""),
            Some(b'\\') => Some("\\"),
            _ => None,
        };
        if let Some(s) = simple {
            return Ok(s.to_string());
        }
        let len = match e {
            Some(b'u') => 4,
            Some(b'U') => 8,
            Some(b'x') => 2,
            _ => return self.fail(),
        };
        let end = (self.i + len).min(self.b.len());
        let hex = &self.b[self.i.min(end)..end];
        if hex.is_empty() || !hex.iter().all(u8::is_ascii_hexdigit) || hex.len() != len {
            return self.fail();
        }
        self.i += len;
        let cp = u32::from_str_radix(std::str::from_utf8(hex).unwrap_or("0"), 16).unwrap_or(u32::MAX);
        if cp > 0x10ffff || (0xd800..=0xdfff).contains(&cp) {
            return self.fail();
        }
        match char::from_u32(cp) {
            Some(c) => Ok(c.to_string()),
            None => self.fail(),
        }
    }
    fn literal(&mut self) -> Result<String, ParseError> {
        let end = find_byte(self.b, b'\'', self.i + 1);
        let nl = find_byte(self.b, b'\n', self.i + 1);
        let Some(end) = end else { return self.fail() };
        if nl.is_some_and(|nl| nl < end) {
            return self.fail();
        }
        let v = self.s[self.i + 1..end].to_string();
        self.i = end + 1;
        Ok(v)
    }
    fn multiline(&mut self, q: u8) -> Result<String, ParseError> {
        let qqq = [q, q, q];
        self.i += 3;
        if self.peek(0) == Some(b'\r') && self.peek(1) == Some(b'\n') {
            self.i += 1;
        }
        if self.peek(0) == Some(b'\n') {
            self.i += 1;
            self.line += 1;
        }
        let mut out: Vec<u8> = Vec::new();
        loop {
            if self.i >= self.b.len() {
                return self.fail();
            }
            if self.starts_with(&qqq) {
                self.i += 3;
                while self.peek(0) == Some(q) && !self.starts_with(&qqq) {
                    out.push(q);
                    self.i += 1;
                }
                return Ok(bytes_to_string(out));
            }
            let c = self.b[self.i];
            self.i += 1;
            if c == b'\n' {
                self.line += 1;
            }
            if q == b'"' && c == b'\\' {
                // line-ending backslash: trim whitespace and newlines up to the next content
                if matches!(self.peek(0), Some(b' ' | b'\t' | b'\r' | b'\n')) {
                    let mut j = self.i;
                    while matches!(self.b.get(j), Some(b' ' | b'\t' | b'\r')) {
                        j += 1;
                    }
                    if self.b.get(j) == Some(&b'\n') {
                        while let Some(&c) = self.b.get(j) {
                            if !matches!(c, b' ' | b'\t' | b'\r' | b'\n') {
                                break;
                            }
                            if c == b'\n' {
                                self.line += 1;
                            }
                            j += 1;
                        }
                        self.i = j;
                        continue;
                    }
                }
                out.extend_from_slice(self.escape()?.as_bytes());
            } else {
                out.push(c);
            }
        }
    }
    fn nest(&mut self) -> Result<(), ParseError> {
        self.depth += 1;
        if self.depth > MAX_NESTING {
            return self.fail();
        }
        Ok(())
    }
    fn value(&mut self) -> Result<NodeId, ParseError> {
        match self.peek(0) {
            Some(b'"') => {
                let s = if self.starts_with(b"\"\"\"") { self.multiline(b'"')? } else { self.basic()? };
                Ok(self.doc.add(Node::Str(s)))
            }
            Some(b'\'') => {
                let s = if self.starts_with(b"'''") { self.multiline(b'\'')? } else { self.literal()? };
                Ok(self.doc.add(Node::Str(s)))
            }
            Some(b'[') => {
                self.nest()?;
                self.i += 1;
                let mut arr = Vec::new();
                loop {
                    self.wsnl();
                    if self.peek(0) == Some(b']') {
                        self.i += 1;
                        self.depth -= 1;
                        return Ok(self.doc.add(Node::Arr(arr)));
                    }
                    arr.push(self.value()?);
                    self.wsnl();
                    if self.peek(0) == Some(b',') {
                        self.i += 1;
                        continue;
                    }
                    if self.peek(0) == Some(b']') {
                        self.i += 1;
                        self.depth -= 1;
                        return Ok(self.doc.add(Node::Arr(arr)));
                    }
                    return self.fail();
                }
            }
            Some(b'{') => {
                self.nest()?;
                self.i += 1;
                let t = self.doc.add(Node::Obj(ObjMap::default()));
                self.ws();
                if self.peek(0) == Some(b'}') {
                    self.i += 1;
                    self.depth -= 1;
                    return Ok(t);
                }
                loop {
                    let k = self.key()?;
                    if self.peek(0) != Some(b'=') {
                        return self.fail();
                    }
                    self.i += 1;
                    self.ws();
                    let v = self.value()?;
                    self.assign(t, &k, v);
                    self.ws();
                    if self.peek(0) == Some(b',') {
                        self.i += 1;
                        self.ws();
                        continue;
                    }
                    if self.peek(0) == Some(b'}') {
                        self.i += 1;
                        self.depth -= 1;
                        return Ok(t);
                    }
                    return self.fail();
                }
            }
            _ => {
                let start = self.i;
                while self.i < self.b.len()
                    && !matches!(self.b[self.i], b' ' | b'\t' | b'\r' | b'\n' | b',' | b']' | b'}' | b'#')
                {
                    self.i += 1;
                }
                if self.i == start {
                    return self.fail();
                }
                let raw = self.s[start..self.i].to_string();
                Ok(self.doc.add(Node::Raw(raw)))
            }
        }
    }
    /// The table at `path` below `from` (arrays of tables: their last table), creating (or replacing non-tables
    /// with) empty tables on the way.
    fn table(&mut self, path: &[String], from: NodeId) -> NodeId {
        let mut t = from;
        for p in path {
            let mut next = self.doc.get(t, p);
            if let Some(id) = next {
                if let Node::Arr(a) = self.doc.node(id) {
                    next = a.last().copied();
                }
            }
            let next = match next {
                Some(id) if self.doc.is_obj(id) => id,
                _ => {
                    let id = self.doc.add(Node::Obj(ObjMap::default()));
                    self.doc.obj_mut(t).set(p, id);
                    id
                }
            };
            t = next;
        }
        t
    }
    fn assign(&mut self, t: NodeId, path: &[String], v: NodeId) {
        let owner = self.table(&path[..path.len() - 1], t);
        self.doc.obj_mut(owner).set(&path[path.len() - 1], v);
    }
    fn parse(mut self) -> Result<Doc, ParseError> {
        let root = self.doc.root;
        let mut current = root;
        loop {
            self.wsnl();
            if self.i >= self.b.len() {
                return Ok(self.doc);
            }
            if self.peek(0) == Some(b'[') {
                let array_table = self.peek(1) == Some(b'[');
                self.i += if array_table { 2 } else { 1 };
                let path = self.key()?;
                if self.peek(0) != Some(b']') || (array_table && self.peek(1) != Some(b']')) {
                    return self.fail();
                }
                self.i += if array_table { 2 } else { 1 };
                if array_table {
                    let owner = self.table(&path[..path.len() - 1], root);
                    let last = &path[path.len() - 1];
                    let arr = match self.doc.get(owner, last) {
                        Some(id) if matches!(self.doc.node(id), Node::Arr(_)) => id,
                        _ => self.doc.add(Node::Arr(Vec::new())),
                    };
                    self.doc.obj_mut(owner).set(last, arr);
                    current = self.doc.add(Node::Obj(ObjMap::default()));
                    if let Node::Arr(a) = &mut self.doc.nodes[arr] {
                        a.push(current);
                    }
                } else {
                    current = self.table(&path, root);
                }
                self.eol()?;
            } else {
                let k = self.key()?;
                if self.peek(0) != Some(b'=') {
                    return self.fail();
                }
                self.i += 1;
                self.ws();
                let v = self.value()?;
                self.assign(current, &k, v);
                self.eol()?;
            }
        }
    }
}

/// TOML → nested tables; strings stay strings, other scalars become `Raw`.
pub(crate) fn parse_toml(text: &str) -> Result<Doc, ParseError> {
    let mut doc = Doc::new();
    doc.root = doc.add(Node::Obj(ObjMap::default()));
    Toml { s: text, b: text.as_bytes(), i: 0, line: 1, doc, depth: 0 }.parse()
}

// ---- YAML subset ----------------------------------------------------------------------------------------------
// Block mappings and sequences by indentation (spaces), plain / 'single' / "double" quoted scalars, one-line flow
// collections ({…} / […]), comments, block scalars (| >, kept as text). No anchors/aliases/tags semantics, no
// multi-line flow collections or multi-line quoted scalars (a line error). Every scalar is a string (null = empty).

#[derive(Clone)]
struct YLine {
    n: usize,      // 1-based line number
    indent: usize, // leading spaces
    text: String,  // content after the indentation, trailing spaces removed
}

struct Yaml {
    lines: Vec<YLine>,
    doc: Doc,
}

/// `/^[ \t]+/` removed.
fn strip_leading_ws(s: &str) -> &str {
    s.trim_start_matches([' ', '\t'])
}
/// `/[ \t]+$/` removed.
fn strip_trailing_ws(s: &str) -> &str {
    s.trim_end_matches([' ', '\t'])
}

impl Yaml {
    fn new(text: &str) -> Result<Yaml, ParseError> {
        let mut lines = Vec::new();
        for (k, raw) in text.split('\n').enumerate() {
            let l = raw.trim_end_matches([' ', '\t', '\r']);
            let ind = l.bytes().take_while(|&c| c == b' ').count();
            let t = &l[ind..];
            if t.is_empty() || t.starts_with('#') {
                continue;
            }
            if ind == 0 && (t == "---" || t.starts_with("--- ") || t == "..." || t.starts_with('%')) {
                continue;
            }
            if t.starts_with('\t') {
                return Err(ParseError::new("bad-yaml", format!("line {}", k + 1)));
            }
            lines.push(YLine { n: k + 1, indent: ind, text: t.to_string() });
        }
        Ok(Yaml { lines, doc: Doc::new() })
    }

    fn fail<T>(n: usize) -> Result<T, ParseError> {
        Err(ParseError::new("bad-yaml", format!("line {n}")))
    }

    fn is_seq(t: &str) -> bool {
        t == "-" || t.starts_with("- ") || t.starts_with("-\t")
    }

    /// "key: rest" → (key, rest) (key unquoted), or None when the text is not a mapping entry.
    fn split_key(t: &str) -> Option<(String, String)> {
        let b = t.as_bytes();
        if b.first().is_some_and(|&c| c == b'{' || c == b'[') {
            return None;
        }
        if let Some(&q) = b.first().filter(|&&c| c == b'"' || c == b'\'') {
            let mut j = 1;
            let mut key: Vec<u8> = Vec::new();
            while j < b.len() {
                if b[j] == b'\\' && q == b'"' {
                    if let Some(&c) = b.get(j + 1) {
                        key.push(c);
                    }
                    j += 1;
                } else if b[j] == q {
                    if q == b'\'' && b.get(j + 1) == Some(&b'\'') {
                        key.push(b'\'');
                        j += 1;
                    } else {
                        break;
                    }
                } else {
                    key.push(b[j]);
                }
                j += 1;
            }
            if j >= b.len() {
                return None;
            }
            let mut k = j + 1;
            while b.get(k) == Some(&b' ') {
                k += 1;
            }
            if b.get(k) != Some(&b':') || (k + 1 < b.len() && b[k + 1] != b' ' && b[k + 1] != b'\t') {
                return None;
            }
            return Some((bytes_to_string(key), t[k + 1..].to_string()));
        }
        for j in 0..b.len() {
            if b[j] == b'#' && j > 0 && (b[j - 1] == b' ' || b[j - 1] == b'\t') {
                return None;
            }
            if b[j] == b':' && (j + 1 == b.len() || b[j + 1] == b' ' || b[j + 1] == b'\t') {
                let key = strip_trailing_ws(&t[..j]);
                return if key.is_empty() { None } else { Some((key.to_string(), t[j + 1..].to_string())) };
            }
        }
        None
    }

    /// A scalar or flow collection on one line; the rest of the line may only be a comment. None = null.
    fn inline(&mut self, src: &str, n: usize, depth: usize) -> Result<NodeId, ParseError> {
        let s = strip_leading_ws(src);
        if s.is_empty() || s.starts_with('#') {
            return Ok(self.doc.add(Node::Null));
        }
        if s.starts_with(['"', '\'', '{', '[']) {
            let mut i = 0;
            let v = self.flow_value(s, &mut i, n, false, depth)?;
            let rest = strip_leading_ws(&s[i..]);
            if !rest.is_empty() && !rest.starts_with('#') {
                return Self::fail(n);
            }
            return Ok(v);
        }
        // plain scalar: up to a " #" comment
        let b = s.as_bytes();
        let cut = (1..b.len()).find(|&j| b[j] == b'#' && (b[j - 1] == b' ' || b[j - 1] == b'\t'));
        let s = match cut {
            Some(j) => &s[..j - 1],
            None => s,
        };
        Ok(self.doc.add(Node::Str(strip_trailing_ws(s).to_string())))
    }

    fn quoted(&mut self, s: &str, i: &mut usize, n: usize) -> Result<String, ParseError> {
        let b = s.as_bytes();
        let q = b[*i];
        *i += 1;
        let mut out: Vec<u8> = Vec::new();
        loop {
            if *i >= b.len() {
                return Self::fail(n);
            }
            let c = b[*i];
            *i += 1;
            if c == q {
                if q == b'\'' && b.get(*i) == Some(&b'\'') {
                    out.push(b'\'');
                    *i += 1;
                    continue;
                }
                return Ok(bytes_to_string(out));
            }
            if q == b'"' && c == b'\\' {
                let e = b.get(*i).copied();
                *i += 1;
                let simple = match e {
                    Some(b'n') => Some(b'\n'),
                    Some(b't') => Some(b'\t'),
                    Some(b'r') => Some(b'\r'),
                    Some(b'0') => Some(0u8),
                    Some(b'"') => Some(b'"'),
                    Some(b'\\') => Some(b'\\'),
                    Some(b'/') => Some(b'/'),
                    Some(b' ') => Some(b' '),
                    _ => None,
                };
                if let Some(c) = simple {
                    out.push(c);
                } else if let Some(e @ (b'x' | b'u' | b'U')) = e {
                    let len = match e {
                        b'x' => 2,
                        b'u' => 4,
                        _ => 8,
                    };
                    let hex = b.get(*i..*i + len);
                    let Some(hex) = hex.filter(|h| h.iter().all(u8::is_ascii_hexdigit)) else { return Self::fail(n) };
                    let cp = u32::from_str_radix(std::str::from_utf8(hex).unwrap_or("0"), 16).unwrap_or(u32::MAX);
                    if cp > 0x10ffff || (0xd800..=0xdfff).contains(&cp) {
                        return Self::fail(n);
                    }
                    let Some(ch) = char::from_u32(cp) else { return Self::fail(n) };
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                    *i += len;
                } else {
                    return Self::fail(n);
                }
            } else {
                out.push(c);
            }
        }
    }

    fn flow_value(&mut self, s: &str, i: &mut usize, n: usize, in_flow: bool, depth: usize) -> Result<NodeId, ParseError> {
        let b = s.as_bytes();
        while matches!(b.get(*i), Some(b' ' | b'\t')) {
            *i += 1;
        }
        let c = b.get(*i).copied();
        if matches!(c, Some(b'"' | b'\'')) {
            let v = self.quoted(s, i, n)?;
            return Ok(self.doc.add(Node::Str(v)));
        }
        if let Some(open @ (b'[' | b'{')) = c {
            if depth + 1 > MAX_NESTING {
                return Self::fail(n);
            }
            let close = if open == b'[' { b']' } else { b'}' };
            *i += 1;
            let mut arr: Vec<NodeId> = Vec::new();
            let mut obj = ObjMap::default();
            loop {
                while matches!(b.get(*i), Some(b' ' | b'\t')) {
                    *i += 1;
                }
                if *i >= b.len() {
                    return Self::fail(n);
                }
                if b[*i] == close {
                    *i += 1;
                    return Ok(if open == b'[' { self.doc.add(Node::Arr(arr)) } else { self.doc.add(Node::Obj(obj)) });
                }
                let k = self.flow_value(s, i, n, true, depth + 1)?;
                while matches!(b.get(*i), Some(b' ' | b'\t')) {
                    *i += 1;
                }
                if open == b'{' {
                    if b.get(*i) != Some(&b':') {
                        return Self::fail(n);
                    }
                    *i += 1;
                    let key = self.doc.js_string(k);
                    let v = self.flow_value(s, i, n, true, depth + 1)?;
                    obj.set(&key, v);
                } else {
                    arr.push(k);
                }
                while matches!(b.get(*i), Some(b' ' | b'\t')) {
                    *i += 1;
                }
                if b.get(*i) == Some(&b',') {
                    *i += 1;
                } else if b.get(*i) != Some(&close) {
                    return Self::fail(n);
                }
            }
        }
        let start = *i;
        while *i < b.len() {
            let ch = b[*i];
            if in_flow && (ch == b',' || ch == b']' || ch == b'}') {
                break;
            }
            if in_flow && ch == b':' && matches!(b.get(*i + 1), Some(b' ' | b'\t')) {
                break;
            }
            if (ch == b' ' || ch == b'\t') && b.get(*i + 1) == Some(&b'#') {
                break;
            }
            *i += 1;
        }
        let v = strip_trailing_ws(&s[start..*i]);
        Ok(if v.is_empty() { self.doc.add(Node::Null) } else { self.doc.add(Node::Str(v.to_string())) })
    }

    fn block(&mut self, i: usize, ind: usize, depth: usize) -> Result<(NodeId, usize), ParseError> {
        if depth > MAX_NESTING {
            return Self::fail(self.lines[i].n);
        }
        if Self::is_seq(&self.lines[i].text) {
            self.seq(i, ind, depth)
        } else {
            self.map(i, ind, depth)
        }
    }

    fn value(&mut self, rest: &str, mut i: usize, ind: usize, n: usize, depth: usize) -> Result<(NodeId, usize), ParseError> {
        let r = strip_leading_ws(rest);
        let len = self.lines.len();
        if r.is_empty() || r.starts_with('#') {
            if i < len && self.lines[i].indent > ind {
                let li = self.lines[i].indent;
                return self.block(i, li, depth + 1);
            }
            if i < len && self.lines[i].indent == ind && Self::is_seq(&self.lines[i].text) {
                if depth + 1 > MAX_NESTING {
                    return Self::fail(self.lines[i].n);
                }
                return self.seq(i, ind, depth + 1);
            }
            return Ok((self.doc.add(Node::Null), i));
        }
        if r.starts_with(['|', '>']) {
            let mut parts: Vec<&str> = Vec::new();
            while i < len && self.lines[i].indent > ind {
                parts.push(&self.lines[i].text);
                i += 1;
            }
            let joined = parts.join("\n");
            return Ok((self.doc.add(Node::Str(joined)), i));
        }
        let first = self.inline(r, n, depth)?;
        let plain = !r.starts_with(['"', '\'', '{', '[']);
        let mut v: Option<String> = None; // the joined plain scalar once continuation lines are seen
        while i < len && self.lines[i].indent > ind {
            let line = self.lines[i].clone();
            if !plain || Self::is_seq(&line.text) || Self::split_key(&line.text).is_some() {
                return Self::fail(line.n);
            }
            let prev = match &v {
                Some(s) => s.clone(),
                None => self.doc.js_string(first),
            };
            let next = self.inline(&line.text, line.n, depth)?;
            let joined = format!("{} {}", prev, self.doc.js_string(next));
            v = Some(js_trim(&joined).to_string());
            i += 1;
        }
        Ok(match v {
            Some(s) => (self.doc.add(Node::Str(s)), i),
            None => (first, i),
        })
    }

    fn map(&mut self, mut i: usize, ind: usize, depth: usize) -> Result<(NodeId, usize), ParseError> {
        let mut o = ObjMap::default();
        while i < self.lines.len() && self.lines[i].indent == ind && !Self::is_seq(&self.lines[i].text) {
            let Some((key, rest)) = Self::split_key(&self.lines[i].text) else { return Self::fail(self.lines[i].n) };
            let n = self.lines[i].n;
            i += 1;
            let (v, next) = self.value(&rest, i, ind, n, depth)?;
            o.set(&key, v);
            i = next;
        }
        if i < self.lines.len() && self.lines[i].indent > ind {
            return Self::fail(self.lines[i].n);
        }
        Ok((self.doc.add(Node::Obj(o)), i))
    }

    fn seq(&mut self, mut i: usize, ind: usize, depth: usize) -> Result<(NodeId, usize), ParseError> {
        let mut arr: Vec<NodeId> = Vec::new();
        while i < self.lines.len() && self.lines[i].indent == ind && Self::is_seq(&self.lines[i].text) {
            let t = self.lines[i].text.clone();
            let after = strip_leading_ws(&t[1..]).to_string();
            let col = ind + (t.len() - after.len());
            if after.is_empty() || after.starts_with('#') {
                i += 1;
                if i < self.lines.len() && self.lines[i].indent > ind {
                    let li = self.lines[i].indent;
                    let (v, next) = self.block(i, li, depth + 1)?;
                    arr.push(v);
                    i = next;
                } else {
                    arr.push(self.doc.add(Node::Null));
                }
                continue;
            }
            if Self::is_seq(&after) || Self::split_key(&after).is_some() {
                // "- key: v" / "- - x": the item is a block starting at the item's column
                let n = self.lines[i].n;
                self.lines[i] = YLine { n, indent: col, text: after };
                let (v, next) = self.block(i, col, depth + 1)?;
                arr.push(v);
                i = next;
                continue;
            }
            let n = self.lines[i].n;
            i += 1;
            let (v, next) = self.value(&after, i, ind, n, depth)?;
            arr.push(v);
            i = next;
        }
        if i < self.lines.len() && self.lines[i].indent > ind {
            return Self::fail(self.lines[i].n);
        }
        Ok((self.doc.add(Node::Arr(arr)), i))
    }

    fn parse(mut self) -> Result<Doc, ParseError> {
        if self.lines.is_empty() {
            self.doc.root = self.doc.add(Node::Null);
            return Ok(self.doc);
        }
        let first = self.lines[0].clone();
        if !Self::is_seq(&first.text) && Self::split_key(&first.text).is_none() {
            if self.lines.len() == 1 {
                self.doc.root = self.inline(&first.text, first.n, 0)?;
                return Ok(self.doc);
            }
            return Self::fail(first.n);
        }
        let (v, i) = self.block(0, first.indent, 1)?;
        if i < self.lines.len() {
            return Self::fail(self.lines[i].n);
        }
        self.doc.root = v;
        Ok(self.doc)
    }
}

/// YAML subset → nested objects/arrays of string scalars (null for empty values).
pub(crate) fn parse_yaml(text: &str) -> Result<Doc, ParseError> {
    Yaml::new(text)?.parse()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn jsonc() {
        let v = parse_jsonc("# c\n// c\n{ \"a\": \"#x\", /* b */ \"b\": [1, 2,], }").unwrap();
        assert_eq!(v, json!({"a": "#x", "b": [1, 2]}));
        let v = parse_jsonc("{\"u\": \"https://x//y#z\", \"q\": \"a\\\"b // c\", /* x */ }").unwrap();
        assert_eq!(v, json!({"u": "https://x//y#z", "q": "a\"b // c"}));
        assert_eq!(parse_jsonc("{").unwrap_err(), ParseError::new("bad-json", "not valid JSON"));
    }

    #[test]
    fn toml() {
        let d = parse_toml("a.b = \"x\"\n[c.\"d e\"]\nf = [ \"1\",\n \"2\", ] # c\ng = { h = 'i', j.k = \"\\u00e9\" }\n[[l]]\nm = 1\n[[l]]\nm = 2\n").unwrap();
        assert_eq!(
            d.to_json(d.root),
            json!({"a": {"b": "x"}, "c": {"d e": {"f": ["1", "2"], "g": {"h": "i", "j": {"k": "é"}}}}, "l": [{"m": {"raw": "1"}}, {"m": {"raw": "2"}}]})
        );
        assert_eq!(parse_toml("a = \"x\n").unwrap_err(), ParseError::new("bad-toml", "line 1"));
        // a header that meets a scalar replaces it; `current` keeps pointing at the table it named
        let d = parse_toml("a = 1\n[a]\nb = 2\n[[t]]\nx = 1\n[t.s]\ny = 2\n").unwrap();
        assert_eq!(d.to_json(d.root), json!({"a": {"b": {"raw": "2"}}, "t": [{"x": {"raw": "1"}, "s": {"y": {"raw": "2"}}}]}));
        // multi-line strings
        let d = parse_toml("a = \"\"\"\nx \\\n   y\"\"\"\"\nb = '''\nl'''\n").unwrap();
        assert_eq!(d.to_json(d.root), json!({"a": "x y\"", "b": "l"}));
    }

    #[test]
    fn yaml() {
        let d = parse_yaml("---\na: \"x # y\"  # c\nb:\n  c: 'it''s'\n  d:\n  - 1\n  - e: f\n    g: h\nk: [a, \"b\"]\nm: { n: o }\n").unwrap();
        assert_eq!(d.to_json(d.root), json!({"a": "x # y", "b": {"c": "it's", "d": ["1", {"e": "f", "g": "h"}]}, "k": ["a", "b"], "m": {"n": "o"}}));
        assert_eq!(parse_yaml("a: b\n  c: d\n").unwrap_err(), ParseError::new("bad-yaml", "line 2"));
        // plain continuation lines join with a space; a flow value on a continuation line is String()-ed
        let d = parse_yaml("a: one\n  two\n  [x, [y, z]]\nb: |\n  l1\n  l2\n{c: 1}: 2\n").unwrap_err();
        assert_eq!(d, ParseError::new("bad-yaml", "line 7"));
        let d = parse_yaml("a: one\n  two\n  [x, [y, z]]\nb: |\n  l1\n  l2\n").unwrap();
        assert_eq!(d.to_json(d.root), json!({"a": "one two x,y,z", "b": "l1\nl2"}));
        let d = parse_yaml("{[a, [b]]: 1, {x: y}: 2, : 3}").unwrap();
        assert_eq!(d.to_json(d.root), json!({"a,b": "1", "[object Object]": "2", "": "3"}));
    }

    #[test]
    fn plist() {
        let d = parse_plist("<?xml version=\"1.0\"?><!DOCTYPE plist [<!ENTITY x \"y\">]><plist><dict><key>a&amp;b</key><real> 0.5 </real><key>t</key><true/><key>c</key><array><string>&#x41;&#66;&#0;&lt;</string><![CDATA[<z>]]></array></dict></plist>").unwrap();
        assert_eq!(d.to_json(d.root), json!({"a&b": "0.5", "t": "true", "c": ["AB<"]}));
        assert!(parse_plist("<plist><dict><key>a</key></plist>").is_err());
        assert!(parse_plist("<x/>").is_err());
    }

    const INHERITED: [&str; 12] = [
        "__proto__", "constructor", "toString", "valueOf", "hasOwnProperty", "isPrototypeOf", "propertyIsEnumerable",
        "toLocaleString", "__defineGetter__", "__defineSetter__", "__lookupGetter__", "__lookupSetter__",
    ];

    /// The reference's "parsers: Object.prototype names are ordinary own keys; no prototype is ever written"
    /// (PORT-QUESTIONS Q1). In Rust every key is ordinary by construction: this pins the documents and the key order.
    #[test]
    fn inherited_names_are_ordinary_own_keys() {
        let quote = |s: &str| crate::canon::canon_json(&Value::String(s.to_string()));
        for n in INHERITED {
            let (n2, n3, n4) = (format!("{n}2"), format!("{n}3"), format!("{n}4"));
            let (q, q2, q3, q4) = (quote(n), quote(&n2), quote(&n3), quote(&n4));
            // TOML: a key, a dotted key, a table header, a nested header, an inline table, arrays of tables
            let t = parse_toml(&format!(
                "{q} = \"v\"\nd.{q}.e = \"6\"\n[{q2}]\nx = \"1\"\n[a.{q}.b]\ny = {{ {q} = \"2\" }}\n[[{q3}]]\nz = \"3\"\n[[{q3}]]\nz = \"4\"\n[{q4}.{q}]\nw = \"5\"\n"
            ))
            .unwrap();
            let keys: Vec<&str> = t.obj(t.root).unwrap().keys().collect();
            assert_eq!(keys, [n, "d", n2.as_str(), "a", n3.as_str(), n4.as_str()]);
            let want = format!(
                "{{{q}:\"v\",\"d\":{{{q}:{{\"e\":\"6\"}}}},{q2}:{{\"x\":\"1\"}},\"a\":{{{q}:{{\"b\":{{\"y\":{{{q}:\"2\"}}}}}}}},{q3}:[{{\"z\":\"3\"}},{{\"z\":\"4\"}}],{q4}:{{{q}:{{\"w\":\"5\"}}}}}}"
            );
            assert_eq!(t.to_json(t.root), crate::json::parse_json(&want).unwrap(), "{n}");
            // YAML: a block key, a nested block, a flow mapping key, a sequence item
            let y = parse_yaml(&format!("{n}:\n  {n}: x\nk: {{{n}: y}}\ns:\n- {n}: z\n")).unwrap();
            let want = format!("{{{q}:{{{q}:\"x\"}},\"k\":{{{q}:\"y\"}},\"s\":[{{{q}:\"z\"}}]}}");
            assert_eq!(y.to_json(y.root), crate::json::parse_json(&want).unwrap(), "{n}");
            // plist dict keys
            let p = parse_plist(&format!("<plist><dict><key>{n}</key><dict><key>{n}</key><string>x</string></dict></dict></plist>")).unwrap();
            assert_eq!(p.to_json(p.root), crate::json::parse_json(&format!("{{{q}:{{{q}:\"x\"}}}}")).unwrap());
            // JSON(C)
            let j = parse_jsonc(&format!("{{{q}: {{{q}: 1}}}}")).unwrap();
            assert_eq!(j.as_object().unwrap().keys().collect::<Vec<_>>(), [n]);
        }
        assert!(parse_toml("[__proto__.__proto__]\nx = 1\n").is_ok());
    }

    /// The reference's "a YAML collection as a flow key or a continuation reads like String() of a plain object"
    /// (Q1; web d18d688): `js_string` — null → "", arrays joined with "," (null items → ""), mappings → "[object Object]".
    #[test]
    fn yaml_collection_keys_and_continuations_read_like_js_string() {
        let y = parse_yaml("k: {[a, , b]: 1, {x: y}: 2, : 3, [{p: q}, r]: 4}\nm: a\n  [b, c]\n  {d: e}\n  [{f: g}, [h, ]]\n").unwrap();
        let want = r#"{"k":{"a,,b":"1","[object Object]":"2","":"3","[object Object],r":"4"},"m":"a b,c [object Object] [object Object],h"}"#;
        assert_eq!(y.to_json(y.root), crate::json::parse_json(want).unwrap());
        assert_eq!(parse_yaml(" k: {[a, b]: c, {x: y}: z, : w}\n- - z").unwrap_err(), ParseError::new("bad-yaml", "line 2"));
    }

    /// The reference's "nesting: at most 128 levels in every parser, the 129th is the parser's own error" (Q2).
    #[test]
    fn nesting_at_most_128_levels_in_every_parser() {
        let brackets = |n: usize| "[".repeat(n) + &"]".repeat(n);
        assert!(parse_jsonc(&brackets(128)).is_ok());
        assert_eq!(parse_jsonc(&brackets(129)).unwrap_err(), ParseError::new("bad-json", "not valid JSON"));
        assert!(parse_jsonc(&("{\"a\":".repeat(128) + "1" + &"}".repeat(128))).is_ok());
        assert_eq!(parse_jsonc(&("{\"a\":".repeat(129) + "1" + &"}".repeat(129))).unwrap_err().code, "bad-json");
        assert!(parse_toml(&format!("a = {}", brackets(128))).is_ok());
        assert_eq!(parse_toml(&format!("a = {}", brackets(129))).unwrap_err(), ParseError::new("bad-toml", "line 1"));
        assert_eq!(parse_toml(&format!("x = 1\na = {}{}", "[\n".repeat(129), "]".repeat(129))).unwrap_err().detail, "line 130");
        assert!(parse_toml(&format!("a = {}1{}", "{b = ".repeat(128), "}".repeat(128))).is_ok());
        assert_eq!(parse_toml(&format!("a = {}1{}", "{b = ".repeat(129), "}".repeat(129))).unwrap_err().detail, "line 1");
        let dotted: Vec<String> = (0..300).map(|i| format!("t{i}")).collect();
        assert!(parse_toml(&format!("[{}]\nx = 1\n", dotted.join("."))).is_ok());
        let blocks = |n: usize| (0..n).map(|i| " ".repeat(i) + "a:").collect::<Vec<_>>().join("\n") + " x\n";
        assert!(parse_yaml(&blocks(128)).is_ok());
        assert_eq!(parse_yaml(&blocks(129)).unwrap_err(), ParseError::new("bad-yaml", "line 129"));
        assert!(parse_yaml(&format!("a: {}", brackets(127))).is_ok());
        assert_eq!(parse_yaml(&format!("a: {}", brackets(128))).unwrap_err(), ParseError::new("bad-yaml", "line 1"));
        assert!(parse_yaml(&brackets(128)).is_ok()); // a one-line document: the flow collection is the root
        assert_eq!(parse_yaml(&brackets(129)).unwrap_err().detail, "line 1");
        let seqs = |n: usize| "- ".repeat(n) + "x\n";
        assert!(parse_yaml(&seqs(128)).is_ok());
        assert_eq!(parse_yaml(&seqs(129)).unwrap_err().detail, "line 1");
        let arrays = |n: usize| format!("<plist>{}{}</plist>", "<array>".repeat(n), "</array>".repeat(n));
        assert!(parse_plist(&arrays(128)).is_ok());
        assert_eq!(parse_plist(&arrays(129)).unwrap_err(), ParseError::new("bad-plist", "not a property list"));
    }

    /// The reference's "lone surrogates: a JSON escape that leaves one is bad-json" (Q3) — the parser half.
    #[test]
    fn lone_surrogate_escapes_are_bad_json() {
        for s in [r#"{"a": "\ud800"}"#, r#"{"a": "x\udc00"}"#, r#"{"\ud800": 1}"#, r#"{"a": "\udc00\ud800"}"#, r#"{"a": "\ud800\u0041"}"#, r#"["\udbff"]"#] {
            assert_eq!(parse_jsonc(s).unwrap_err(), ParseError::new("bad-json", "not valid JSON"), "{s}");
        }
        assert_eq!(parse_jsonc("{\"a\": \"\\ud83c\\udfa8\"} // \\ud800").unwrap(), json!({"a": "🎨"}));
    }

    /// The generator's `jsJson`: `JSON.stringify` in JavaScript's own key order (array-index keys ascending first,
    /// then first-insertion order), TOML bare values as `{"$raw": …}`.
    fn js_json(doc: &Doc, id: NodeId, out: &mut String) {
        use crate::canon::push_js_string;
        fn is_array_index(k: &str) -> bool {
            !k.is_empty()
                && k.len() <= 10
                && k.bytes().all(|c| c.is_ascii_digit())
                && (k == "0" || !k.starts_with('0'))
                && k.parse::<u64>().is_ok_and(|n| n < 4_294_967_295)
        }
        match doc.node(id) {
            Node::Null => out.push_str("null"),
            Node::Str(s) => push_js_string(out, s),
            Node::Raw(s) => {
                out.push_str("{\"$raw\":");
                push_js_string(out, s);
                out.push('}');
            }
            Node::Arr(a) => {
                out.push('[');
                for (i, &x) in a.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    js_json(doc, x, out);
                }
                out.push(']');
            }
            Node::Obj(o) => {
                let mut index: Vec<&(String, NodeId)> = o.entries.iter().filter(|(k, _)| is_array_index(k)).collect();
                index.sort_by_key(|(k, _)| k.parse::<u64>().unwrap_or(0));
                let rest = o.entries.iter().filter(|(k, _)| !is_array_index(k));
                out.push('{');
                for (i, (k, v)) in index.into_iter().chain(rest).enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    push_js_string(out, k);
                    out.push(':');
                    js_json(doc, *v, out);
                }
                out.push('}');
            }
        }
    }

    /// The differential cases of tests/data/reference-fuzz.json ("parsers": generated TOML / YAML / JSONC / plist
    /// documents and the reference parser's exact result; see tests/data/gen-reference-fuzz.mjs).
    /// `THEME_REFERENCE_FUZZ=<file>` replays another corpus.
    #[test]
    fn reference_fuzz() {
        let path = std::env::var("THEME_REFERENCE_FUZZ")
            .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/reference-fuzz.json").to_string());
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        let corpus = crate::json::parse_json(&text).expect("reference-fuzz.json is JSON");
        let cases = corpus["parsers"].as_array().expect("parsers section");
        let cap = |s: String| -> String {
            if s.encode_utf16().count() > 200 {
                let digest = crate::test_sha256::sha256_hex(s.as_bytes());
                format!("sha256:{}", &digest[..24])
            } else {
                s
            }
        };
        let mut bad = Vec::new();
        for case in cases {
            let (which, src, want) = (case[0].as_str().unwrap(), case[1].as_str().unwrap(), case[2].as_str().unwrap());
            let doc_json = |d: Result<Doc, ParseError>| {
                d.map(|d| {
                    let mut s = String::new();
                    js_json(&d, d.root, &mut s);
                    s
                })
            };
            let got = match which {
                "jsonc" => parse_jsonc(src).map(|v| crate::canon::canon_json(&v)),
                "toml" => doc_json(parse_toml(src)),
                "yaml" => doc_json(parse_yaml(src)),
                "plist" => doc_json(parse_plist(src)),
                other => panic!("unknown parser {other}"),
            };
            let got = cap(match got {
                Ok(s) => format!("ok:{s}"),
                Err(e) => format!("err:{}: {}", e.code, e.detail),
            });
            if got != want {
                bad.push(format!("{which} {src:?}\n    want {want}\n    got  {got}"));
            }
        }
        assert!(bad.is_empty(), "{} of {} parser cases differ from the reference:\n  {}", bad.len(), cases.len(), bad[..bad.len().min(12)].join("\n  "));
        eprintln!("parsers: {}/{} identical to the reference", cases.len(), cases.len());
    }
}
