//! Games round 2: a Hold'em hand evaluator on the CLIENT (F5's "you have this" + F6's odds). Pure; best five of 5–7
//! cards; the same keys as the shared vector `testvectors/games/holdem-hand-ranking.json` (a byte copy of the
//! spec's `vectors/holdem-hand-ranking.json`): `[category, tie-break ranks…]`, compared left to right, ace = 14
//! except in a wheel (the straight's key is 5). The server stays the only judge of a hand; this only explains one.

/// The category ids, weakest first (words.json `table.categories` keys; the key's first number is the index).
pub const CATEGORIES: [&str; 9] = ["high-card", "pair", "two-pair", "trips", "straight", "flush", "full-house", "quads", "straight-flush"];

/// "As" → (14, 's'); None for anything else (ranks 23456789TJQKA, suits cdhs).
pub fn parse(c: &str) -> Option<(u8, char)> {
    let mut it = c.chars();
    let (r, s, rest) = (it.next()?, it.next()?, it.next());
    if rest.is_some() || !"cdhs".contains(s) {
        return None;
    }
    let r = "23456789TJQKA".find(r)? as u8 + 2;
    Some((r, s))
}

/// The key of exactly five cards.
fn key5(c: &[(u8, char); 5]) -> Vec<u8> {
    let mut r: Vec<u8> = c.iter().map(|x| x.0).collect();
    r.sort_unstable_by(|a, b| b.cmp(a));
    let flush = c.iter().all(|x| x.1 == c[0].1);
    let mut uniq = r.clone();
    uniq.dedup();
    let straight_hi = if uniq.len() == 5 && r[0] - r[4] == 4 {
        Some(r[0])
    } else if uniq == [14, 5, 4, 3, 2] {
        Some(5)
    } else {
        None
    };
    // groups: (count, rank), biggest count first, then the higher rank
    let mut groups: Vec<(u8, u8)> = uniq.iter().map(|&v| (r.iter().filter(|&&x| x == v).count() as u8, v)).collect();
    groups.sort_unstable_by(|a, b| b.cmp(a));
    let ranks_by_group: Vec<u8> = groups.iter().map(|g| g.1).collect();
    match (straight_hi, flush, groups[0].0, groups.get(1).map(|g| g.0)) {
        (Some(h), true, _, _) => vec![8, h],
        (_, _, 4, _) => [vec![7], ranks_by_group].concat(),
        (_, _, 3, Some(2)) => [vec![6], ranks_by_group].concat(),
        (_, true, _, _) => [vec![5], r].concat(),
        (Some(h), false, _, _) => vec![4, h],
        (_, _, 3, _) => [vec![3], ranks_by_group].concat(),
        (_, _, 2, Some(2)) => [vec![2], ranks_by_group].concat(),
        (_, _, 2, _) => [vec![1], ranks_by_group].concat(),
        _ => [vec![0], r].concat(),
    }
}

/// The best key of 5–7 cards (None if a card doesn't parse, repeats, or there are fewer than 5 / more than 7).
pub fn best_key(cards: &[&str]) -> Option<Vec<u8>> {
    if !(5..=7).contains(&cards.len()) {
        return None;
    }
    let p: Vec<(u8, char)> = cards.iter().map(|c| parse(c)).collect::<Option<_>>()?;
    for i in 0..p.len() {
        if p[i + 1..].contains(&p[i]) {
            return None;
        }
    }
    best_key_parsed(&p)
}

pub(crate) fn best_key_parsed(p: &[(u8, char)]) -> Option<Vec<u8>> {
    let n = p.len();
    let mut best: Option<Vec<u8>> = None;
    // every 5-card subset (at most C(7,5) = 21)
    for skip_a in 0..n {
        for skip_b in skip_a..n {
            if n == 5 && (skip_a, skip_b) != (0, 0) {
                continue;
            }
            if n == 6 && skip_a != skip_b {
                continue;
            }
            if n == 7 && skip_a == skip_b {
                continue;
            }
            let five: Vec<(u8, char)> = (0..n).filter(|&i| n == 5 || (i != skip_a && i != skip_b)).map(|i| p[i]).collect();
            let five: [(u8, char); 5] = five.try_into().ok()?;
            let k = key5(&five);
            if best.as_ref().is_none_or(|b| k > *b) {
                best = Some(k);
            }
        }
    }
    best
}

/// The category id of 5–7 cards ("flush"), "" when they don't make a hand.
pub fn category(cards: &[&str]) -> &'static str {
    best_key(cards).map(|k| CATEGORIES[k[0] as usize]).unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    const RANKING: &str = include_str!("testvectors/games/holdem-hand-ranking.json");

    fn strs(v: &Value) -> Vec<&str> {
        v.as_array().unwrap().iter().map(|c| c.as_str().unwrap()).collect()
    }

    #[test]
    fn hand_vectors_seven() {
        let v: Value = serde_json::from_str(RANKING).unwrap();
        let seven = v["seven"].as_array().unwrap();
        assert!(seven.len() >= 10, "too few vectors");
        for t in seven {
            let cards = strs(&t["cards"]);
            let k = best_key(&cards).unwrap_or_else(|| panic!("{}: no key", t["id"]));
            let want: Vec<u8> = t["expect"]["key"].as_array().unwrap().iter().map(|x| x.as_u64().unwrap() as u8).collect();
            assert_eq!(k, want, "{}", t["id"]);
            assert_eq!(category(&cards), t["expect"]["category"].as_str().unwrap(), "{}", t["id"]);
        }
    }

    #[test]
    fn hand_vectors_showdown() {
        let v: Value = serde_json::from_str(RANKING).unwrap();
        for t in v["showdown"].as_array().unwrap() {
            let board = strs(&t["board"]);
            let hole = t["hole"].as_object().unwrap();
            for (seat, want) in t["expect"]["keys"].as_object().unwrap() {
                let mut c = board.clone();
                c.extend(strs(&hole[seat]));
                let want: Vec<u8> = want.as_array().unwrap().iter().map(|x| x.as_u64().unwrap() as u8).collect();
                assert_eq!(best_key(&c).unwrap(), want, "{} seat {seat}", t["id"]);
            }
        }
    }

    #[test]
    fn hand_refuses_bad_input() {
        for bad in [vec!["As", "Ks", "Qs", "Js"], vec!["As", "As", "Qs", "Js", "Ts"], vec!["1s", "Ks", "Qs", "Js", "Ts"], vec!["As", "Kx", "Qs", "Js", "Ts"]] {
            assert!(best_key(&bad).is_none(), "{bad:?}");
        }
    }

    /// F5: every example hand in the rankings sheet IS the hand it illustrates; one row per category, strongest first
    /// (the words' order and HandRankings.qml's `order` both); every line and word is there.
    #[test]
    fn rankings_sheet_examples_are_true() {
        let w: Value = serde_json::from_str(crate::play_ctl::WORDS).unwrap();
        let t = &w["table"];
        let mut best_first: Vec<&str> = CATEGORIES.to_vec();
        best_first.reverse();
        let lines: Vec<&str> = t["rank_lines"].as_object().expect("table.rank_lines").keys().map(String::as_str).collect();
        let mut sorted_lines = lines.clone();
        sorted_lines.sort();
        let mut sorted_best = best_first.clone();
        sorted_best.sort();
        assert_eq!(sorted_lines, sorted_best, "a line per category");
        let qml = include_str!("../qml/HandRankings.qml");
        let order_line = qml.lines().find(|l| l.contains("readonly property var order:")).expect("HandRankings.qml order");
        let order: Vec<&str> = order_line.split('"').skip(1).step_by(2).collect();
        assert_eq!(order, best_first, "HandRankings.qml draws strongest first");
        for id in &best_first {
            let eg = strs(&t["rank_eg"][id]);
            assert_eq!(eg.len(), 5, "{id}: five example cards");
            assert_eq!(category(&eg), *id, "the {id} row's example");
            assert!(t["categories"][id].as_str().is_some_and(|s| !s.is_empty()), "{id} has a name");
            assert!(t["rank_lines"][id].as_str().is_some_and(|s| !s.is_empty()), "{id} has a line");
        }
        for k in ["hands_button", "hands_title", "hands_foot", "hands_you", "hands_close", "cards_on_phone", "show_here", "hidden_sharing", "odds_win", "odds_tie", "odds_improve", "odds_improve_one", "odds_info", "odds_toggle"] {
            assert!(t["r2"][k].as_str().is_some_and(|s| !s.is_empty()), "table.r2.{k}");
        }
        // the check can fail: a mislabelled example is caught
        assert_ne!(category(&["Ac", "Jc", "8c", "6c", "2d"]), "flush");
    }
}
