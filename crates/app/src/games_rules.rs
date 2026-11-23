//! Games round 2: the table's pure UI rules (GAMES-V1.1-ADDENDUM §1.4, §3.2, §4.2), byte-identical in meaning with
//! Tin Hat Web's `features/games/cardsFace.ts` & co. The cases are `testvectors/games/table-ui-v1.json` (a byte copy
//! of the spec's `vectors/table-ui-v1.json`).
//!
//! - `cards_face` (B1/B2): where my hole cards are face up. The desktop is never a Pad (`on_pad` = false here).
//! - `show_here_next`: "Show here" holds until a Pad attaches AGAIN or the table closes.
//! - `live_opponents`, `odds_visible` (F6), `odds_inputs`: the ONLY inputs of the odds (my cards, the board, a count).
//! - `ReactBucket` (F7): the client mirror of the server's per-person reaction bucket (greys the tray).

use serde_json::Value;

/// none | up | down | peek
pub fn cards_face(has_cards: bool, sharing: bool, on_pad: bool, here: bool, screens: &[&str]) -> &'static str {
    if !has_cards {
        "none"
    } else if sharing {
        "down"
    } else if on_pad || here {
        "up"
    } else if screens.contains(&"pad") {
        "peek"
    } else {
        "up"
    }
}

pub fn show_here_next(here: bool, prev_screens: &[&str], screens: &[&str], table_closed: bool) -> bool {
    if table_closed {
        return false;
    }
    if screens.contains(&"pad") && !prev_screens.contains(&"pad") {
        return false;
    }
    here
}

/// Seats in the hand that can still win it, other than me (`you` = my seat, None = a spectator): active, all-in, or
/// leaving (still in the hand until their in-turn fold; plays out if all-in).
pub fn live_opponents(you: Option<i64>, seats: &[Value]) -> u32 {
    seats
        .iter()
        .filter(|s| matches!(s["state"].as_str(), Some("active") | Some("allin") | Some("leaving")))
        .filter(|s| you.is_none_or(|y| s["seat"].as_i64() != Some(y)))
        .count() as u32
}

/// `face` = cards_face's answer, or "peeking" while I hold to peek.
pub fn odds_visible(setting: &Value, you_state: &str, street: &str, result: &Value, face: &str, opponents: u32) -> bool {
    setting == &Value::Bool(true)
        && matches!(you_state, "active" | "allin")
        && matches!(street, "preflop" | "flop" | "turn" | "river")
        && result.is_null()
        && matches!(face, "up" | "peeking")
        && opponents >= 1
}

/// The odds' inputs from a view: (my hole cards, the board, live opponents). Nothing else of the view is read:
/// another seat's `shown` / `cards`, the result, the pots never reach the odds (launch-gate ruling 7).
pub fn odds_inputs(view: &Value) -> Option<(Vec<String>, Vec<String>, u32)> {
    let you = view.get("you")?;
    let hole: Vec<String> = you.get("cards")?.as_array()?.iter().map(|c| c.as_str().map(str::to_string)).collect::<Option<_>>()?;
    let board: Vec<String> = match view.get("hand").and_then(|h| h.get("board")) {
        Some(b) => b.as_array()?.iter().map(|c| c.as_str().map(str::to_string)).collect::<Option<_>>()?,
        None => vec![],
    };
    let seats = view.get("seats").and_then(Value::as_array).cloned().unwrap_or_default();
    let n = live_opponents(you.get("seat").and_then(Value::as_i64), &seats);
    Some((hole, board, n))
}

/// The tray's mirror of the server's bucket: starts full; +1 per refill (fractional), capped at burst; a send costs 1.
pub struct ReactBucket {
    burst: f64,
    refill_ms: f64,
    tokens: f64,
    at: Option<f64>,
}
impl ReactBucket {
    pub fn new(burst: u32, refill_ms: u32) -> Self {
        Self { burst: burst as f64, refill_ms: refill_ms as f64, tokens: burst as f64, at: None }
    }
    fn level(&self, now_ms: f64) -> f64 {
        match self.at {
            Some(t) => (self.tokens + (now_ms - t).max(0.0) / self.refill_ms).min(self.burst),
            None => self.tokens,
        }
    }
    /// Would a send now pass (no token taken).
    pub fn peek(&self, now_ms: f64) -> bool {
        self.level(now_ms) >= 1.0
    }
    pub fn try_send(&mut self, now_ms: f64) -> bool {
        if let Some(t) = self.at {
            self.tokens = (self.tokens + (now_ms - t).max(0.0) / self.refill_ms).min(self.burst);
        }
        self.at = Some(now_ms);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    pub const TABLE_UI: &str = include_str!("testvectors/games/table-ui-v1.json");

    fn strs(v: &Value) -> Vec<&str> {
        v.as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default()
    }

    /// 12.2.0's rule (HoldemTable.qml: hidden = sharing || screens has pad; no Show here): must FAIL the vectors.
    fn rule_1220(has_cards: bool, sharing: bool, _on_pad: bool, _here: bool, screens: &[&str]) -> &'static str {
        if !has_cards {
            "none"
        } else if sharing || screens.contains(&"pad") {
            "peek"
        } else {
            "up"
        }
    }

    fn cards_face_misses(f: fn(bool, bool, bool, bool, &[&str]) -> &'static str) -> Vec<String> {
        let v: Value = serde_json::from_str(TABLE_UI).unwrap();
        v["cards_face"]["cases"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| {
                let i = &c["in"];
                let b = |k: &str| i[k].as_bool().unwrap_or(false);
                f(b("has_cards"), b("sharing"), b("on_pad"), b("here"), &strs(&i["screens"])) != c["expect"].as_str().unwrap()
            })
            .map(|c| c["id"].as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn table_ui_cards_face() {
        assert!(cards_face_misses(cards_face).is_empty(), "{:?}", cards_face_misses(cards_face));
        // failing first: the published 12.2.0 rule is red on these vectors
        assert!(!cards_face_misses(rule_1220).is_empty());
    }

    /// Parity (PM 15:54): while THIS screen shares, my cards are face down with NO peek, whatever else is true (every
    /// combination of cards / Pad / Show here / screens); and HoldemTable.qml's hold-to-peek is enabled only for `peek`.
    #[test]
    fn sharing_is_always_down_never_peek() {
        let screens: [&[&str]; 5] = [&[], &["desktop"], &["desktop", "pad"], &["pad"], &["web", "pad", "tv"]];
        for has in [true, false] {
            for on_pad in [true, false] {
                for here in [true, false] {
                    for sc in screens {
                        let f = cards_face(has, true, on_pad, here, sc);
                        assert_eq!(f, if has { "down" } else { "none" }, "sharing, has {has} pad {on_pad} here {here} {sc:?}");
                    }
                }
            }
        }
        let qml = include_str!("../qml/HoldemTable.qml");
        let peek = qml.find("objectName: \"holdemPeek\"").expect("the hold-to-peek area");
        let block = &qml[peek..peek + 400];
        assert!(block.contains("enabled: ht.face === \"peek\""), "hold-to-peek must be enabled only for a Pad's peek");
        // what the cards draw: faces only while up or peeking (never while down)
        assert!(qml.contains("card: ht.mineVisible ? String(modelData) : \"\""));
        assert!(qml.contains("readonly property bool mineVisible: ht.seen === \"up\" || ht.seen === \"peeking\""));
        assert!(qml.contains("readonly property string seen: ht.face === \"up\" ? \"up\" : (ht.face === \"peek\" && ht.peeking ? \"peeking\" : ht.face)"));
        // the check can fail: the 12.2.0 rule gave a peek while sharing
        assert_eq!(rule_1220(true, true, false, false, &["desktop"]), "peek");
    }

    #[test]
    fn table_ui_show_here_next() {
        let v: Value = serde_json::from_str(TABLE_UI).unwrap();
        let cases = v["show_here_next"]["cases"].as_array().unwrap();
        assert!(cases.len() >= 4);
        for c in cases {
            let i = &c["in"];
            let got = show_here_next(i["here"].as_bool().unwrap(), &strs(&i["prev_screens"]), &strs(&i["screens"]), i["table_closed"].as_bool().unwrap_or(false));
            assert_eq!(got, c["expect"].as_bool().unwrap(), "{}", c["id"]);
        }
        // a rule that never resets is caught
        assert!(cases.iter().any(|c| c["in"]["here"] == json!(true) && c["expect"] == json!(false)));
    }

    #[test]
    fn table_ui_live_opponents_and_odds_visible() {
        let v: Value = serde_json::from_str(TABLE_UI).unwrap();
        for c in v["live_opponents"]["cases"].as_array().unwrap() {
            let i = &c["in"];
            assert_eq!(live_opponents(i["you"].as_i64(), i["seats"].as_array().unwrap()) as u64, c["expect"].as_u64().unwrap(), "{}", c["id"]);
        }
        let cases = v["odds_visible"]["cases"].as_array().unwrap();
        assert!(cases.len() >= 10);
        for c in cases {
            let i = &c["in"];
            let got = odds_visible(&i["setting"], i["you_state"].as_str().unwrap(), i["street"].as_str().unwrap(), &i["result"], i["face"].as_str().unwrap(), i["opponents"].as_u64().unwrap() as u32);
            assert_eq!(got, c["expect"].as_bool().unwrap(), "{}", c["id"]);
        }
    }

    #[test]
    fn table_ui_react_bucket() {
        let v: Value = serde_json::from_str(TABLE_UI).unwrap();
        let r = &v["reactions"];
        assert_eq!(strs(&r["ids"]), crate::games_rules::REACTIONS.to_vec());
        for c in r["bucket_cases"].as_array().unwrap() {
            let mut b = ReactBucket::new(r["burst"].as_u64().unwrap() as u32, r["refill_ms"].as_u64().unwrap() as u32);
            let got: Vec<bool> = c["at_ms"].as_array().unwrap().iter().map(|t| b.try_send(t.as_f64().unwrap())).collect();
            let want: Vec<bool> = c["expect"].as_array().unwrap().iter().map(|x| x.as_bool().unwrap()).collect();
            assert_eq!(got, want, "{}", c["id"]);
        }
    }

    /// Launch-gate ruling 7 (addendum §3.3 (b)): a view whose OTHER seats carry planted cards (shown, a card list
    /// where a count belongs) gives odds inputs without any of them; and a caller that read them would be caught.
    #[test]
    fn odds_inputs_carry_only_what_i_see() {
        let planted = ["Qs", "Qd", "Jc", "Jd"];
        let view = json!({
            "game": "holdem", "status": "hand",
            "seats": [
                {"seat": 1, "u": "@me:x", "state": "active", "cards": 2},
                {"seat": 3, "u": "@ana:x", "state": "active", "cards": ["Qs", "Qd"], "shown": ["Qs", "Qd"]},
                {"seat": 5, "u": "@ben:x", "state": "folded", "cards": 2, "shown": ["Jc", "Jd"]}
            ],
            "hand": {"n": 3, "street": "flop", "board": ["2c", "9d", "Ts"]},
            "result": {"shown": {"3": ["Qs", "Qd"]}},
            "you": {"seat": 1, "cards": ["As", "Kd"]}
        });
        let (hole, board, n) = odds_inputs(&view).unwrap();
        assert_eq!(hole, vec!["As", "Kd"]);
        assert_eq!(board, vec!["2c", "9d", "Ts"]);
        assert_eq!(n, 1);
        let all = format!("{hole:?}{board:?}");
        for p in planted {
            assert!(!all.contains(p), "a planted card {p} reached the odds");
        }
        // the check can fail: a leaky extraction (every string card anywhere in the view) contains them
        let leaky: String = view.to_string();
        assert!(planted.iter().any(|p| leaky.contains(p)));
        // a spectator (no `you`) gets no odds inputs at all
        assert!(odds_inputs(&json!({"seats": [], "hand": {"board": []}})).is_none());
    }
}

/// F7: the reactions, tray order (addendum §4.1; = the vectors' `reactions.ids`).
pub const REACTIONS: [&str; 6] = ["tip", "knock", "clap", "toss", "sweat", "foil"];
