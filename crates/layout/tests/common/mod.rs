//! Shared test support: the vector evaluator (a port of the reference's `tools/layout/vector-run.mjs` computeExpect),
//! the vector files, the node / TypeScript-reference locations, and the independent checker bridge.
#![allow(dead_code)]

pub mod gen;

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};

use tinhat_layout::json::{mk, read_json, J};
use tinhat_layout::schema::{internal_desktop, preset_desktop};
use tinhat_layout::sync::{known_of, merge, project, run_sync, save, SaveReq};
use tinhat_layout::{apply_op, drop_zones, migrate, repair_desktop_text, repair_phone_text, solve, validate_desktop, validate_phone};

pub fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}
pub fn repo_dir() -> PathBuf {
    crate_dir().join("../..")
}
pub fn vec_dir() -> PathBuf {
    crate_dir().join("testvectors/layout")
}

pub fn sorted_files(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(dir).expect("dir").map(|e| e.expect("entry").path()).collect();
    v.sort();
    v
}
/// Every file under `dir`, recursively, sorted per directory (the reference runner's order).
pub fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    for p in sorted_files(dir) {
        if p.is_dir() {
            walk(&p, out);
        } else {
            out.push(p);
        }
    }
}

// ---- vector files ------------------------------------------------------------------------------------------------
// A vector file is `prettyJson` of a JavaScript value: it may nest deeper than the engine's 128 levels (a repair
// vector's `expect` holds the 128-level form of its text), and `JSON.stringify` writes a lone UTF-16 surrogate inside a
// string as a `\udxxx` escape (the vector of a text holding a raw lone surrogate). A Rust string cannot hold a lone
// surrogate, so the harness reads such an escape as a sentinel character (U+10F800 + (s − 0xD800), private use,
// never in a vector otherwise), prints it back as the same escape, and hands the ENGINE the text a UTF-8 transfer
// makes of it: U+FFFD (LAYOUT.md §1.1 "a raw lone surrogate in the text reads as U+FFFD").

const SENTINEL: u32 = 0x10F800;

/// Lone-surrogate escapes inside JSON strings → the sentinel character (everything else unchanged).
fn unlone(text: &str) -> String {
    let b = text.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let hex = |i: usize| -> Option<u32> { text.get(i..i + 4).and_then(|h| u32::from_str_radix(h, 16).ok()) };
    let (mut i, mut in_str) = (0usize, false);
    while i < b.len() {
        let c = b[i];
        if !in_str {
            in_str = c == b'"';
            out.push(c);
            i += 1;
            continue;
        }
        match c {
            b'"' => {
                in_str = false;
                out.push(c);
                i += 1;
            }
            b'\\' if b.get(i + 1) == Some(&b'u') => {
                let v = hex(i + 2);
                let pair = v.is_some_and(|v| (0xD800..0xDC00).contains(&v))
                    && b.get(i + 6) == Some(&b'\\')
                    && b.get(i + 7) == Some(&b'u')
                    && hex(i + 8).is_some_and(|w| (0xDC00..0xE000).contains(&w));
                match v {
                    Some(v) if (0xD800..0xE000).contains(&v) && !pair => {
                        let ch = char::from_u32(SENTINEL + (v - 0xD800)).expect("sentinel");
                        let mut buf = [0u8; 4];
                        out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                        i += 6;
                    }
                    _ => {
                        let n = if pair { 12 } else { 6 };
                        out.extend_from_slice(&b[i..(i + n).min(b.len())]);
                        i += n;
                    }
                }
            }
            b'\\' => {
                out.extend_from_slice(&b[i..(i + 2).min(b.len())]);
                i += 2;
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8(out).expect("UTF-8")
}

/// Read a vector file (or a bridge line): JSON.parse semantics for what a test file can hold (see above).
pub fn read_vector(text: &str) -> J {
    let v = tinhat_theme::json::parse_json_with_limit(&unlone(text), 1 << 12).unwrap_or_else(|e| panic!("a vector is JSON: {e}"));
    J::from_value(&v)
}

/// The sentinel characters back as the escapes JSON.stringify writes.
fn relone(s: String) -> String {
    if !s.chars().any(|c| (c as u32) >= SENTINEL) {
        return s;
    }
    s.chars()
        .map(|c| if (c as u32) >= SENTINEL { format!("\\u{:04x}", 0xD800 + (c as u32 - SENTINEL)) } else { c.to_string() })
        .collect()
}

/// The text the engine receives: a lone surrogate (the sentinel) became U+FFFD on the way.
pub fn engine_text(s: &str) -> String {
    s.chars().map(|c| if (c as u32) >= SENTINEL { '\u{fffd}' } else { c }).collect()
}

/// The vector's form: `form`, or the named preset (`preset`: a preset name or "classic-flush").
pub fn form_of(v: &J) -> J {
    if let Some(f) = v.get("form") {
        return f.deep();
    }
    let p = v.get_str("preset").map(|p| p.to_string()).unwrap_or_default();
    preset_desktop(&p).or_else(|| internal_desktop(&p)).unwrap_or_default()
}

fn opt_deep(v: Option<J>) -> Option<J> {
    v.map(|x| x.deep())
}

/// `computeExpect(v)` of vector-run.mjs, with the port.
pub fn compute_expect(v: &J) -> J {
    let kind = v.get_str("kind").map(|k| k.to_string()).unwrap_or_default();
    match kind.as_str() {
        "solve" => solve(&form_of(v), v.get("size").as_ref(), v.get("unitPm").as_ref(), v.get("shape").as_ref(), v.get("content").as_ref(), v.get("view").as_ref()).to_json(),
        "op" => apply_op(&form_of(v), opt_deep(v.get("op")).as_ref(), opt_deep(v.get("ctx")).as_ref()).to_json(),
        "dropzones" => drop_zones(&form_of(v), opt_deep(v.get("ctx")).as_ref(), v.get("dragged").as_ref()).to_json(),
        "repair" => {
            let text = v.get_str("text").map(|t| engine_text(&t));
            let target = v.get_str("target").map(|t| t.to_string()).unwrap_or_default();
            if target == "document" {
                let val = text.as_deref().and_then(read_json);
                let active = v.get_str("active").map(|a| a.to_string());
                return project(val.as_ref(), active.as_deref()).to_json();
            }
            // text that is not a string is not JSON data (readJson refuses anything but a string)
            let t = text.unwrap_or_else(|| "\u{0}not a string".into());
            if target == "phone" {
                repair_phone_text(&t).to_json()
            } else {
                repair_desktop_text(&t).to_json()
            }
        }
        "validate" => {
            let f = opt_deep(v.get("form"));
            let viol = if v.is("target", "phone") { validate_phone(f.as_ref()) } else { validate_desktop(f.as_ref()) };
            mk(vec![("violations", J::strs(&viol))])
        }
        "save" => {
            let s = v.get("save").unwrap_or_else(J::obj);
            let known = known_of(opt_deep(v.get("known")).as_ref());
            let raw = opt_deep(v.get("raw"));
            let req = SaveReq {
                id: s.get("id"),
                edit: opt_deep(s.get("edit")),
                start: opt_deep(s.get("start")),
                also: opt_deep(s.get("also")),
                now: s.get("now"),
                by: s.get("by"),
                seq: s.get("seq"),
                key: s.get("key"), // vector-run.mjs passes `key` when the vector has one (else the save counts under `by`)
            };
            save(raw.as_ref(), &known, &req).to_json()
        }
        "merge" => {
            let known = known_of(opt_deep(v.get("known")).as_ref());
            let raw = opt_deep(v.get("raw"));
            let js = v.get_str("justSaved").map(|s| s.to_string());
            let (doc, too_large) = merge(raw.as_ref(), &known, js.as_deref());
            mk(vec![("doc", doc), ("tooLarge", J::Bool(too_large))])
        }
        "sync" => run_sync(&v.deep()),
        "migrate" => mk(vec![("view", migrate(v.get("prefs").as_ref(), v.get("H").as_ref(), v.get("L").as_ref(), v.get("chromeH").as_ref()))]),
        other => panic!("unknown vector kind {other:?}"),
    }
}

/// The vector file rebuilt from the port's result: `prettyJson({...v, expect: computeExpect(v)})`.
pub fn rebuild(v: &J) -> String {
    let out = v.deep();
    out.set("expect", compute_expect(v));
    relone(out.pretty())
}

/// The first line where two texts differ (for readable failures).
pub fn first_diff(want: &str, got: &str) -> String {
    for (i, (a, b)) in want.lines().zip(got.lines()).enumerate() {
        if a != b {
            return format!("line {}: want {a:?}, got {b:?}", i + 1);
        }
    }
    format!("lengths differ: want {} lines, got {}", want.lines().count(), got.lines().count())
}

// ---- node and the TypeScript reference ---------------------------------------------------------------------------
/// node (LAYOUT_NODE, else /opt/node22/bin/node, else `node` on PATH).
pub fn node_bin() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("LAYOUT_NODE") {
        return Some(PathBuf::from(p));
    }
    let p = PathBuf::from("/opt/node22/bin/node");
    if p.is_file() {
        return Some(p);
    }
    Command::new("node").arg("--version").output().ok().filter(|o| o.status.success()).map(|_| PathBuf::from("node"))
}

/// The TypeScript reference tree the differential run imports (LAYOUT_TS_REF): a tinhat-web checkout or a pinned
/// `git archive` of one (src/layout, src/theme/{canon,limits}.ts, tools/layout, tools/theme-lib, package.json).
pub fn ts_ref() -> Option<PathBuf> {
    let p = PathBuf::from(std::env::var_os("LAYOUT_TS_REF")?);
    p.join("src/layout/solve.ts").is_file().then_some(p)
}

/// Skip a node-backed test with a loud line unless LAYOUT_REQUIRE_NODE=1 (then it fails).
pub fn skip_or_fail(why: &str) {
    if std::env::var("LAYOUT_REQUIRE_NODE").as_deref() == Ok("1") {
        panic!("{why} (LAYOUT_REQUIRE_NODE=1)");
    }
    eprintln!("SKIPPED: {why} (set LAYOUT_NODE / LAYOUT_TS_REF; LAYOUT_REQUIRE_NODE=1 makes this a failure)");
}

/// The independent checker (tools/layout/invariants.mjs) as a line-oriented child process: one JSON request per line
/// `{kind: "solve", input, out, out2?}` | `{kind: "op", before, op, res}`, one answer line each (`[]` = pass).
pub struct Checker {
    child: Child,
    stdin: ChildStdin,
    lines: std::io::Lines<BufReader<std::process::ChildStdout>>,
}

impl Checker {
    pub fn start(node: &Path) -> Checker {
        let script = repo_dir().join("tools/layout/rust-bridge/check.mjs");
        let mut child = Command::new(node)
            .arg(&script)
            .env("LAYOUT_SCHEMA", crate_dir().join("layout.schema.json"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("start the checker");
        let stdin = child.stdin.take().expect("stdin");
        let lines = BufReader::new(child.stdout.take().expect("stdout")).lines();
        Checker { child, stdin, lines }
    }
    /// Check one request; the violations (empty = pass).
    pub fn check(&mut self, req: &J) -> Vec<String> {
        writeln!(self.stdin, "{}", req.canon()).expect("write to the checker");
        self.stdin.flush().expect("flush");
        let line = self.lines.next().expect("a checker answer").expect("read");
        let v = read_json(&line).expect("checker answer is JSON");
        v.items().iter().map(|x| x.as_str().unwrap_or("?").to_string()).collect()
    }
}

impl Drop for Checker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
