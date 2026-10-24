//! The randomized differential run against the TypeScript reference (LAYOUT.md §10.4): `tools/layout/rust-bridge/
//! diff-cases.mjs` (node) generates seeded cases of every vector kind — operation sequences from every preset (each
//! operation a case), solves in random contexts, drop zones, corrupted forms through repair and validate, phone and
//! document repair, save, merge, scripted sync runs, migrate — computes each with the reference and streams them as
//! vector lines; the port recomputes every `expect` and must agree canonically, byte for byte. On top, the port's own
//! results go through the independent checks: `tools/layout/invariants.mjs` (via node) for every solve (two runs, I1-I17)
//! and every operation (the island multiset), plus the reference differential run's properties (O2 input untouched, O3
//! results validate, R1-R3 repair projection valid / idempotent / identity on valid forms).
//!
//! Env: LAYOUT_DIFF_CASES (default 3000 seeds ≈ 44k records), LAYOUT_DIFF_SEED (default 20260927), LAYOUT_DIFF_FIRST
//! (the first case number, default 0: shards of one large run, see tools/layout/rust-bridge/diff-shards.sh),
//! LAYOUT_TS_REF (the reference tree), LAYOUT_NODE; without node or
//! the reference the test is skipped with a loud line unless LAYOUT_REQUIRE_NODE=1.

mod common;

use common::*;
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::time::Instant;
use tinhat_layout::json::{mk, J};
use tinhat_layout::{repair_desktop, validate_desktop};

fn env_num(k: &str, d: u64) -> u64 {
    std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d)
}

#[test]
fn differential_against_the_typescript_reference() {
    let Some(node) = node_bin() else {
        skip_or_fail("no node for the differential run");
        return;
    };
    let Some(reference) = ts_ref() else {
        skip_or_fail("no TypeScript reference tree (LAYOUT_TS_REF)");
        return;
    };
    let cases = env_num("LAYOUT_DIFF_CASES", 3000);
    let seed = env_num("LAYOUT_DIFF_SEED", 20260927);
    let first = env_num("LAYOUT_DIFF_FIRST", 0);
    let t0 = Instant::now();
    let mut child = Command::new(&node)
        .arg("--no-warnings")
        .arg("--import")
        .arg(reference.join("tools/theme-lib/register.mjs"))
        .arg(repo_dir().join("tools/layout/rust-bridge/diff-cases.mjs"))
        .arg(&reference)
        .arg(cases.to_string())
        .arg(seed.to_string())
        .arg(first.to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("start the case generator");
    let lines = BufReader::with_capacity(1 << 20, child.stdout.take().expect("stdout")).lines();
    let mut ck = Checker::start(&node);

    let mut per_kind: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    let mut firsts: Vec<String> = Vec::new();
    let (mut checked_solves, mut checked_ops, mut repairs) = (0u64, 0u64, 0u64);
    let mut violations: Vec<String> = Vec::new();
    let note = |v: &mut Vec<String>, s: String| {
        if v.len() < 20 {
            v.push(s);
        }
    };
    for (n, line) in lines.enumerate() {
        let line = line.expect("read a case");
        let v = read_vector(&line);
        let kind = v.get_str("kind").map(|k| k.to_string()).unwrap_or_default();
        let want = v.get("expect").map(|e| e.canon()).unwrap_or_default();
        let input_before = v.canon();
        let got = compute_expect(&v);
        let c = per_kind.entry(kind.clone()).or_default();
        if got.canon() == want {
            c.0 += 1;
        } else {
            c.1 += 1;
            if firsts.len() < 10 {
                let mut short = line.clone();
                short.truncate(4000);
                firsts.push(format!("#{n} {kind}\n  want {}\n  got  {}\n  case {short}", trunc(&want), trunc(&got.canon())));
            }
            continue;
        }
        if v.canon() != input_before {
            note(&mut violations, format!("#{n} {kind}: O2 the port modified its input"));
        }
        match kind.as_str() {
            "solve" => {
                let input = J::obj();
                input.set("form", v.get("form").unwrap_or_default());
                for k in ["size", "unitPm", "shape", "content", "view"] {
                    if let Some(x) = v.get(k) {
                        input.set(k, x);
                    }
                }
                let again = compute_expect(&v);
                let errs = ck.check(&mk(vec![("kind", J::s("solve")), ("input", input), ("out", got), ("out2", again)]));
                if !errs.is_empty() {
                    note(&mut violations, format!("#{n} solve: {}", errs.join("; ")));
                }
                checked_solves += 1;
            }
            "op" => {
                let res = got.deep();
                res.set("ok", J::Bool(got.get("form").is_some()));
                let before = v.get("form").unwrap_or_default();
                let errs = ck.check(&mk(vec![("kind", J::s("op")), ("before", before), ("op", v.get("op").unwrap_or_default()), ("res", res)]));
                if !errs.is_empty() {
                    note(&mut violations, format!("#{n} op: {}", errs.join("; ")));
                }
                if let Some(f) = got.get("form") {
                    let viol = validate_desktop(Some(&f));
                    if !viol.is_empty() {
                        note(&mut violations, format!("#{n} op: O3 the result does not validate: {}", viol.join(", ")));
                    }
                }
                checked_ops += 1;
            }
            "repair" if v.is("target", "desktop") => {
                let form = got.get("form").unwrap_or_default();
                let viol = validate_desktop(Some(&form));
                if !viol.is_empty() {
                    note(&mut violations, format!("#{n} repair: R1 the projection does not validate: {}", viol.join(", ")));
                }
                let again = repair_desktop(Some(&form));
                if again.form.canon() != form.canon() {
                    note(&mut violations, format!("#{n} repair: R2 repair is not idempotent"));
                }
                repairs += 1;
            }
            _ => {}
        }
    }
    let status = child.wait().expect("the case generator exits");
    let total: u64 = per_kind.values().map(|(p, f)| p + f).sum();
    let bad: u64 = per_kind.values().map(|(_, f)| f).sum();
    let summary: Vec<String> = per_kind.iter().map(|(k, (p, f))| format!("{k} {p}/{}", p + f)).collect();
    eprintln!(
        "differential vs the TypeScript reference ({}): {cases} cases (#{first}…) from seed {seed} → {total} records, {bad} different — {}; \
         independent checks: {checked_solves} solves (I1-I17, two runs), {checked_ops} operations (multiset + validation), \
         {repairs} desktop repairs (R1/R2) — {} violations; {:.1} s",
        reference.display(),
        summary.join(", "),
        violations.len(),
        t0.elapsed().as_secs_f64()
    );
    for f in &firsts {
        eprintln!("DIFF {f}");
    }
    for v in &violations {
        eprintln!("VIOLATION {v}");
    }
    assert!(status.success(), "the case generator failed: {status}");
    assert!(total > 0, "no cases");
    assert_eq!(bad, 0, "{bad} of {total} records differ from the reference");
    assert!(violations.is_empty(), "{} independent-check violations", violations.len());
    // (a valid form repairs to itself — R3 — is every op / solve record's form: covered by validate in O3)
}

fn trunc(s: &str) -> String {
    if s.len() <= 1500 {
        return s.to_string();
    }
    let mut end = 1500;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}
