//! The shared layout vectors (contract LAYOUT.md §10), byte for byte: every vector file must reproduce exactly — the
//! harness rebuilds each file from the port's result (`prettyJson({...v, expect: computeExpect(v)})`, the reference's
//! `tools/layout/run-vectors.mjs`) and compares the bytes. Nothing is skipped or filtered. Also: every file sits in its
//! kind's directory, the crate's copies equal the contract (LAYOUT_CONTRACT_DIR: the contract directory
//! or a tinhat-web tree; skipped when unset), and the independent checker (`tools/layout/invariants.mjs`, run by node) passes the
//! port's own output of every solve vector, twice (I6).

mod common;

use common::*;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use tinhat_layout::json::{mk, J};
use tinhat_layout::solve;

const KINDS: [&str; 9] = ["solve", "op", "dropzones", "repair", "validate", "save", "merge", "sync", "migrate"];

fn all_vectors() -> Vec<PathBuf> {
    let mut files = Vec::new();
    walk(&vec_dir(), &mut files);
    files
}

#[test]
fn every_vector_reproduces_byte_for_byte() {
    let mut counts: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut fails: Vec<String> = Vec::new();
    for f in all_vectors() {
        let text = fs::read_to_string(&f).expect("read");
        let v = read_vector(&text);
        let kind = v.get_str("kind").map(|k| k.to_string()).unwrap_or_default();
        let got = rebuild(&v);
        let c = counts.entry(kind).or_default();
        if got == text {
            c.0 += 1;
        } else {
            c.1 += 1;
            fails.push(format!("{}: {}", f.strip_prefix(vec_dir()).unwrap().display(), first_diff(&text, &got)));
        }
    }
    let summary: Vec<String> = counts.iter().map(|(k, (p, f))| format!("{k} {p}/{}", p + f)).collect();
    eprintln!("layout vectors: {}", summary.join(", "));
    for f in fails.iter().take(20) {
        eprintln!("DIFF {f}");
    }
    assert!(fails.is_empty(), "{} of the vectors differ (first: {})", fails.len(), fails[0]);
    let total: usize = counts.values().map(|(p, _)| p).sum();
    assert!(total > 1000, "only {total} vectors");
    for k in KINDS {
        assert!(counts.get(k).is_some_and(|c| c.0 > 0), "no {k} vectors");
    }
}

#[test]
fn every_file_is_a_vector_of_its_directory() {
    for f in all_vectors() {
        let rel = f.strip_prefix(vec_dir()).unwrap().to_path_buf();
        let parts: Vec<String> = rel.iter().map(|p| p.to_string_lossy().to_string()).collect();
        assert!(parts.len() == 2 && parts[1].ends_with(".json"), "stray file {}", rel.display());
        assert!(KINDS.contains(&parts[0].as_str()), "unknown kind directory {}", rel.display());
        let v = read_vector(&fs::read_to_string(&f).unwrap());
        assert!(v.is("kind", &parts[0]), "{} is not a {} vector", rel.display(), parts[0]);
    }
}

fn contract_dir() -> PathBuf {
    PathBuf::from(std::env::var("LAYOUT_CONTRACT_DIR").unwrap_or_default())
}

fn same_tree(a: &Path, b: &Path) -> Result<usize, String> {
    let (mut fa, mut fb) = (Vec::new(), Vec::new());
    walk(a, &mut fa);
    walk(b, &mut fb);
    let ra: Vec<PathBuf> = fa.iter().map(|p| p.strip_prefix(a).unwrap().to_path_buf()).collect();
    let rb: Vec<PathBuf> = fb.iter().map(|p| p.strip_prefix(b).unwrap().to_path_buf()).collect();
    if ra != rb {
        return Err(format!("the file lists differ ({} vs {})", ra.len(), rb.len()));
    }
    for r in &ra {
        if fs::read(a.join(r)).unwrap() != fs::read(b.join(r)).unwrap() {
            return Err(format!("{} differs", r.display()));
        }
    }
    Ok(ra.len())
}

/// The crate's copies equal the contract's files byte for byte (sync-contract.sh keeps them so). The contract may be
/// the contract directory itself or a tinhat-web tree (src/layout/…).
#[test]
fn copies_equal_the_contract() {
    let c = contract_dir();
    if c.as_os_str().is_empty() {
        eprintln!("SKIPPED: LAYOUT_CONTRACT_DIR is not set (the shared contract directory)");
        return;
    }
    let (schema, md, vectors, checker) = if c.join("testvectors/layout").is_dir() {
        (c.join("layout.schema.json"), c.join("LAYOUT.md"), c.join("testvectors/layout"), c.join("tools/layout/invariants.mjs"))
    } else if c.join("src/layout/testvectors/layout").is_dir() {
        (
            c.join("src/layout/layout.schema.json"),
            c.join("src/layout/LAYOUT.md"),
            c.join("src/layout/testvectors/layout"),
            c.join("tools/layout/invariants.mjs"),
        )
    } else {
        eprintln!("SKIPPED: no layout contract at {}", c.display());
        return;
    };
    let pairs = [
        (schema, crate_dir().join("layout.schema.json")),
        (md, crate_dir().join("LAYOUT.md")),
        (checker, repo_dir().join("tools/layout/invariants.mjs")),
    ];
    for (theirs, mine) in &pairs {
        assert!(fs::read(theirs).unwrap() == fs::read(mine).unwrap(), "{} differs from the contract's {}", mine.display(), theirs.display());
    }
    let n = same_tree(&vectors, &vec_dir()).unwrap_or_else(|e| panic!("testvectors/layout vs {}: {e}", vectors.display()));
    eprintln!("contract copies: schema, LAYOUT.md, invariants.mjs and {n} vector files identical to {}", c.display());
}

/// The independent checker over the port's own output of every solve vector, computed twice (I6).
#[test]
fn solve_vectors_pass_the_independent_checker() {
    let Some(node) = node_bin() else {
        skip_or_fail("no node for tools/layout/invariants.mjs");
        return;
    };
    let mut ck = Checker::start(&node);
    let mut n = 0;
    for f in all_vectors() {
        let v = read_vector(&fs::read_to_string(&f).unwrap());
        if !v.is("kind", "solve") {
            continue;
        }
        let form = form_of(&v);
        let run = || solve(&form, v.get("size").as_ref(), v.get("unitPm").as_ref(), v.get("shape").as_ref(), v.get("content").as_ref(), v.get("view").as_ref()).to_json();
        let input = mk(vec![("form", form.deep())]);
        for k in ["size", "unitPm", "shape", "content", "view"] {
            if let Some(x) = v.get(k) {
                input.set(k, x);
            }
        }
        let req = mk(vec![("kind", J::s("solve")), ("input", input), ("out", run()), ("out2", run())]);
        let errs = ck.check(&req);
        assert!(errs.is_empty(), "{}: {}", f.display(), errs.join("; "));
        n += 1;
    }
    eprintln!("independent checker: {n} solve vectors (the port's own output, two runs) — 0 violations");
    assert!(n > 500);
}
