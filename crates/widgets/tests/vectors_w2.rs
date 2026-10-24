//! Widgets W2's shared vectors (contract `testvectors/widgets/{justme,…}/**`, byte-equal copies in
//! `testvectors/widgets/`) against the port — the same dispatch as the web's `src/widgets/widgets-w2.test.ts`. The
//! contract copies (these files included) are checked by tests/vectors.rs.

use std::path::{Path, PathBuf};

use serde_json::Value;
use tinhat_theme::canon::canon_json;
use tinhat_theme::json::parse_json;
use tinhat_widgets::justme as jm;
use tinhat_widgets::memories as mem;
use tinhat_widgets::soundpad as pad;
use tinhat_widgets::gamenight as gn;

/// The folders this file owns (tests/vectors.rs skips them).
const W2_DIRS: [&str; 4] = ["justme", "memories", "soundpad", "gamenight"];

fn vec_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testvectors").join("widgets")
}

fn walk(d: &Path, base: &Path, out: &mut Vec<String>) {
    let Ok(rd) = std::fs::read_dir(d) else { return };
    for e in rd {
        let p = e.unwrap().path();
        if p.is_dir() {
            walk(&p, base, out);
        } else {
            out.push(p.strip_prefix(base).unwrap().to_string_lossy().to_string());
        }
    }
}

fn read(rel: &str) -> Value {
    let text = std::fs::read_to_string(vec_dir().join(rel)).unwrap();
    parse_json(&text).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

struct Checker {
    cases: usize,
    fails: Vec<String>,
}

impl Checker {
    fn check(&mut self, file: &str, i: &str, got: Value, expect: &Value) {
        self.cases += 1;
        let (g, e) = (canon_json(&got), canon_json(expect));
        if g != e {
            self.fails.push(format!("{file} #{i}\n  got    {}\n  expect {}", g.chars().take(400).collect::<String>(), e.chars().take(400).collect::<String>()));
        }
    }
}

fn list<'a>(v: &'a Value, k: &str) -> &'a Vec<Value> {
    v.get(k).and_then(Value::as_array).unwrap_or_else(|| panic!("no list {k}"))
}

fn probes(v: &Value) -> Vec<jm::Probe> {
    v.as_array().map(|a| a.iter().map(jm::Probe::from_json).collect()).unwrap_or_default()
}

#[test]
fn every_w2_vector_matches_the_reference() {
    let mut files = Vec::new();
    for d in W2_DIRS {
        walk(&vec_dir().join(d), &vec_dir(), &mut files);
    }
    files.sort();
    assert!(files.len() >= 6, "{} W2 vector files", files.len());
    let mut ck = Checker { cases: 0, fails: Vec::new() };
    for f in &files {
        let v = read(f);
        if f == "justme/check.json" {
            for (i, c) in list(&v, "cases").iter().enumerate() {
                let got = jm::check_of(&jm::CheckInput::from_json(&c["input"])).to_json();
                ck.check(f, &format!("{i} {}", c["note"].as_str().unwrap_or("")), got, &c["expect"]);
            }
        } else if f == "justme/format.json" {
            for (i, c) in list(&v, "ms").iter().enumerate() {
                ck.check(f, &format!("ms {i}"), Value::String(jm::fmt_ms(c["ms"].as_f64().unwrap())), &c["expect"]);
            }
            for (i, c) in list(&v, "ping").iter().enumerate() {
                let got = jm::ping_of(&probes(&c["pings"])).map(tinhat_widgets::num).unwrap_or(Value::Null);
                ck.check(f, &format!("ping {i}"), got, &c["expect"]);
            }
            for (i, c) in list(&v, "names").iter().enumerate() {
                let ids: Vec<&str> = c["ids"].as_array().unwrap().iter().map(|x| x.as_str().unwrap()).collect();
                ck.check(f, &format!("names {i}"), Value::String(jm::names_of(&ids)), &c["expect"]);
            }
            for (i, c) in list(&v, "bar").iter().enumerate() {
                let got = tinhat_widgets::num(jm::bar_pm(c["band"].as_str().unwrap_or(""), c["ms"].as_f64()));
                ck.check(f, &format!("bar {i}"), got, &c["expect"]);
            }
        } else if f == "justme/spark.json" {
            for (i, c) in list(&v, "cases").iter().enumerate() {
                let at = c.get("at").and_then(Value::as_f64).filter(|x| x.is_finite());
                ck.check(f, &i.to_string(), jm::spark_slots(&c["samples"], at), &c["expect"]);
            }
        } else if f == "memories/poster.json" {
            for (i, c) in list(&v, "cases").iter().enumerate() {
                let got = mem::poster_at(&c["session"]).map(|(f, t)| serde_json::json!({ "file": f, "t": tinhat_widgets::num(t) })).unwrap_or(Value::Null);
                ck.check(f, &i.to_string(), got, &c["expect"]);
            }
        } else if f == "memories/list.json" {
            for (i, c) in list(&v, "cases").iter().enumerate() {
                let max = c["max"].as_f64().unwrap_or(0.0).max(0.0) as usize;
                let got = Value::Array(mem::memories_of(&c["rooms"], max).iter().map(mem::MemItem::to_json).collect());
                ck.check(f, &i.to_string(), got, &c["expect"]);
            }
        } else if f == "memories/text.json" {
            for (i, c) in list(&v, "line").iter().enumerate() {
                let it = mem::MemItem::from_json(&c["item"]).expect("item");
                ck.check(f, &format!("line {i}"), Value::String(mem::mem_line(&it, c["now"].as_f64().unwrap())), &c["expect"]);
            }
            for (i, c) in list(&v, "summary").iter().enumerate() {
                let items: Vec<mem::MemItem> = c["items"].as_array().unwrap().iter().filter_map(mem::MemItem::from_json).collect();
                ck.check(f, &format!("summary {i}"), Value::String(mem::mem_summary(&items, c["now"].as_f64().unwrap())), &c["expect"]);
            }
        } else if f == "gamenight/select.json" {
            for (i, c) in list(&v, "cases").iter().enumerate() {
                let max = c["max"].as_f64().unwrap_or(0.0).max(0.0) as usize;
                let got = Value::Array(gn::select_nights(&c["items"], c["now"].as_f64().unwrap(), max).iter().map(gn::NightItem::to_json).collect());
                ck.check(f, &i.to_string(), got, &c["expect"]);
            }
        } else if f == "gamenight/text.json" {
            for (i, c) in list(&v, "when").iter().enumerate() {
                let got = gn::night_when(c["start"].as_f64().unwrap(), c["end"].as_f64().unwrap(), c["now"].as_f64().unwrap(), c["dayTime"].as_str().unwrap());
                ck.check(f, &format!("when {i}"), Value::String(got), &c["expect"]);
            }
            for (i, c) in list(&v, "summary").iter().enumerate() {
                let got = gn::night_summary(c["has"].as_bool().unwrap(), c["game"].as_str().unwrap(), c["when"].as_str().unwrap());
                ck.check(f, &format!("summary {i}"), Value::String(got), &c["expect"]);
            }
            for (i, c) in list(&v, "credit").iter().enumerate() {
                let got = gn::credit_text(c["credit"].as_f64(), c["mode"].as_str().unwrap_or(""));
                ck.check(f, &format!("credit {i}"), Value::String(got), &c["expect"]);
            }
        } else if f == "soundpad/pad.json" {
            for (i, c) in list(&v, "sounds").iter().enumerate() {
                let max = c["max"].as_f64().unwrap_or(0.0).max(0.0) as usize;
                let got = Value::Array(pad::pad_sounds(&c["content"], max).iter().map(pad::PadSound::to_json).collect());
                ck.check(f, &format!("sounds {i}"), got, &c["expect"]);
            }
            for (i, c) in list(&v, "gate").iter().enumerate() {
                let (allowed, hint) = pad::pad_gate(c["inCall"].as_bool().unwrap_or(false), &c["perms"]);
                ck.check(f, &format!("gate {i}"), serde_json::json!({ "allowed": allowed, "hint": hint }), &c["expect"]);
            }
        } else {
            ck.fails.push(format!("unknown W2 vector file {f}"));
        }
    }
    eprintln!("{} W2 vector files, {} cases, {} failed", files.len(), ck.cases, ck.fails.len());
    assert!(ck.fails.is_empty(), "{} of {} cases differ:\n{}", ck.fails.len(), ck.cases, ck.fails.join("\n"));
    assert!(ck.cases > 200, "{} cases", ck.cases);
}
