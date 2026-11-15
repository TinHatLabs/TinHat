//! Auto-rejoin after a terminal drop (P14, 2026-11-13; shared vectors testvectors/rejoin.json = the web's rejoinRule.ts).
//! A healthy call (45 s+) always heals itself with a fresh join. Since P14 so does a young call whose join had
//! completed (the call server said Connected). 5-7 (launch-gate 2026-11-14): ONE automatic rejoin per run of drops — a
//! second drop shows "Connection lost" with Try again, no loop. A join that never completed is a JOIN problem and parks on
//! the Rejoin banner as before.

pub(crate) const MAX_AUTO_REJOINS: u32 = 1;

/// `attempts` = auto-rejoins already made in this run of drops (before the healthy reset).
pub(crate) fn auto_rejoin(recoverable: bool, hung_up: bool, healthy: bool, completed: bool, attempts: u32) -> bool {
    let attempts = if healthy { 0 } else { attempts };
    recoverable && !hung_up && attempts < MAX_AUTO_REJOINS && (healthy || completed)
}

#[cfg(test)]
mod tests {
    use super::*;
    const V: &str = include_str!("testvectors/rejoin.json");

    #[test]
    fn the_shared_vectors() {
        let v: serde_json::Value = serde_json::from_str(V).unwrap();
        assert_eq!(v["maxAttempts"].as_u64(), Some(MAX_AUTO_REJOINS as u64));
        let cases = v["cases"].as_array().unwrap();
        assert_eq!(cases.len(), 9);
        let b = |c: &serde_json::Value, k: &str| c[k].as_bool().unwrap();
        for c in cases {
            let got = auto_rejoin(b(c, "recoverable"), b(c, "hungUp"), b(c, "healthy"), b(c, "completed"), c["attempts"].as_u64().unwrap() as u32);
            assert_eq!(got, b(c, "auto"), "{}", c["name"]);
        }
    }

    /// 5-7 (launch-gate 2026-11-14): ONE automatic rejoin. A young drop rejoins once; the drop right after that rejoin
    /// (still young, one attempt made) shows Connection lost with Try again — no loop.
    #[test]
    fn the_second_drop_does_not_rejoin() {
        assert!(auto_rejoin(true, false, false, true, 0), "the first young drop rejoins");
        assert!(!auto_rejoin(true, false, false, true, 1), "the second drop does not");
        assert!(!auto_rejoin(true, false, false, false, 1), "nor after a rejoin that never connected");
        assert!(auto_rejoin(true, false, true, true, 1), "a call that ran 45 s since starts a new run of drops");
    }

    #[test]
    fn the_vectors_are_the_webs() {
        let Ok(dir) = std::env::var("TINHAT_WEB_DIR") else { return };
        let web = std::fs::read_to_string(format!("{dir}/src/features/calls/testvectors/rejoin.json")).unwrap();
        assert_eq!(web, V, "byte-identical with the web's copy");
    }

    /// The drop handler asks the rule (a terminal drop is only seen after the join completed: call_started is Some).
    #[test]
    fn the_drop_asks_the_rule() {
        let cc = include_str!("call_ctl.rs");
        assert!(cc.contains("crate::rejoin_rule::auto_rejoin(true, false, healthy, true, before)"), "wired");
    }
}
