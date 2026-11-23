//! Games round 2, F6: your odds at Hold'em (GAMES-V1.1-ADDENDUM §3). Pure and deterministic: the same integers as
//! Tin Hat Web for every input (`testvectors/games/odds-v1.json`, a byte copy of the spec's `vectors/odds-v1.json`).
//!
//! Inputs are ONLY what the player can see: their own two hole cards, the board, and the number of live opponents
//! (`games_rules::live_opponents` over the view's seats). Nothing else reaches this module (launch-gate ruling 7).
//!
//! - **The evaluator** is a direct 7-card one (rank counts + suit masks, no allocation); its packed score orders hands
//!   exactly as `holdem-hand-ranking.json`'s keys (`[category, tie-break ranks…]`, ace 14, a wheel's straight = 5).
//!   `play_hand::best_key` (21 five-card subsets, slow) is the independent reference the tests compare it with.
//! - **Exact** enumeration when `E = C(|D|,k) × Π C(|D|−k−2i, 2)` ≤ `ODDS_EXACT_MAX` (saturating), else a seeded
//!   Monte Carlo of exactly `ODDS_SAMPLES` samples (FNV-1a 32 seed, mulberry32, unbiased `below` by rejection, a
//!   partial Fisher–Yates of a fresh copy of D per sample, the runout first, then the opponents in order).
//! - **Outs** (flop + turn): each unseen card that lifts my category above both my current one and what the board plus
//!   that card makes alone, counted under the category reached.

use std::collections::BTreeMap;

pub const ODDS_EXACT_MAX: u64 = 50_000;
pub const ODDS_SAMPLES: u32 = 10_000;

/// A card as 0..52 = suit × 13 + rank (suits c d h s, ranks 2..A → 0..12): the vectors' deck order.
pub type Card = u8;

pub fn card(s: &str) -> Option<Card> {
    let (r, su) = crate::play_hand::parse(s)?;
    Some("cdhs".find(su)? as u8 * 13 + (r - 2))
}

pub fn card_str(c: Card) -> String {
    let r = b"23456789TJQKA"[(c % 13) as usize] as char;
    let s = b"cdhs"[(c / 13) as usize] as char;
    format!("{r}{s}")
}

/// The packed score: category << 20 | k1 << 16 | … | k5 (each k a rank value 2..14, trailing zeros); comparing two
/// scores = comparing the vectors' keys left to right (within a category every key has the same length).
pub fn score(cards: &[Card]) -> u32 {
    let mut cnt = [0u8; 13];
    let mut sm = [0u16; 4];
    for &c in cards {
        cnt[(c % 13) as usize] += 1;
        sm[(c / 13) as usize] |= 1 << (c % 13);
    }
    let all: u16 = sm[0] | sm[1] | sm[2] | sm[3];
    let pack = |cat: u32, ks: &[u32]| -> u32 {
        let mut v = cat << 20;
        for (i, k) in ks.iter().take(5).enumerate() {
            v |= k << (16 - 4 * i as u32);
        }
        v
    };
    let top = |mask: u16, n: usize, skip: &[usize]| -> Vec<u32> {
        (0..13usize).rev().filter(|r| mask & (1 << r) != 0 && !skip.contains(r)).take(n).map(|r| r as u32 + 2).collect()
    };
    fn straight_hi(m: u16) -> Option<u32> {
        for hi in (4..13).rev() {
            if (m >> (hi - 4)) & 0x1F == 0x1F {
                return Some(hi as u32 + 2);
            }
        }
        if m & 0x100F == 0x100F {
            return Some(5); // A 2 3 4 5
        }
        None
    }
    let flush = sm.iter().position(|m| m.count_ones() >= 5);
    if let Some(f) = flush {
        if let Some(h) = straight_hi(sm[f]) {
            return pack(8, &[h]);
        }
    }
    let ranks_with = |n: u8| -> Vec<usize> { (0..13usize).rev().filter(|&r| cnt[r] >= n).collect() };
    let quads = ranks_with(4);
    if let Some(&q) = quads.first() {
        let k = top(all, 1, &[q]);
        return pack(7, &[&[q as u32 + 2][..], &k].concat());
    }
    let trips = ranks_with(3);
    let pairs = ranks_with(2);
    if let Some(&t) = trips.first() {
        if let Some(&p) = pairs.iter().find(|&&p| p != t) {
            return pack(6, &[t as u32 + 2, p as u32 + 2]);
        }
    }
    if let Some(f) = flush {
        return pack(5, &top(sm[f], 5, &[]));
    }
    if let Some(h) = straight_hi(all) {
        return pack(4, &[h]);
    }
    if let Some(&t) = trips.first() {
        let k = top(all, 2, &[t]);
        return pack(3, &[&[t as u32 + 2][..], &k].concat());
    }
    if pairs.len() >= 2 {
        let (a, b) = (pairs[0], pairs[1]);
        let k = top(all, 1, &[a, b]);
        return pack(2, &[&[a as u32 + 2, b as u32 + 2][..], &k].concat());
    }
    if let Some(&p) = pairs.first() {
        let k = top(all, 3, &[p]);
        return pack(1, &[&[p as u32 + 2][..], &k].concat());
    }
    pack(0, &top(all, 5, &[]))
}

pub fn category_of(score: u32) -> usize {
    (score >> 20) as usize
}

// ── the generator ──

pub fn fnv1a32(s: &str) -> u32 {
    let mut h: u32 = 0x811C_9DC5;
    for b in s.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

pub struct Mulberry32(pub u32);
impl Mulberry32 {
    pub fn next_u32(&mut self) -> u32 {
        self.0 = self.0.wrapping_add(0x6D2B_79F5);
        let mut t = self.0;
        t = (t ^ (t >> 15)).wrapping_mul(t | 1);
        t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
        t ^ (t >> 14)
    }
    /// Unbiased 0..r by rejection (lim = 2^32 − 2^32 mod r).
    pub fn below(&mut self, r: u32) -> u32 {
        below_from(r, || self.next_u32())
    }
}

/// `below` over any source of u32s (the tests inject outputs that hit the rejection).
pub fn below_from(r: u32, mut next: impl FnMut() -> u32) -> u32 {
    let lim: u64 = (1u64 << 32) - ((1u64 << 32) % r as u64);
    loop {
        let x = next() as u64;
        if x < lim {
            return (x % r as u64) as u32;
        }
    }
}

// ── odds ──

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Odds {
    pub exact: bool,
    pub samples: u64,
    pub win: u64,
    pub tie: u64,
    pub seed: Option<u32>,
    /// None preflop / on the river
    pub outs: Option<(u32, BTreeMap<String, u32>)>,
}

/// The canonical input: hole + board each sorted into deck order. None = no odds (bad input, shown as nothing).
pub fn canonical(hole: &[&str], board: &[&str], n: u32) -> Option<(Vec<Card>, Vec<Card>, u32)> {
    if hole.len() != 2 || ![0usize, 3, 4, 5].contains(&board.len()) || !(1..=7).contains(&n) {
        return None;
    }
    let mut h: Vec<Card> = hole.iter().map(|c| card(c)).collect::<Option<_>>()?;
    let mut b: Vec<Card> = board.iter().map(|c| card(c)).collect::<Option<_>>()?;
    let mut all: Vec<Card> = h.iter().chain(b.iter()).copied().collect();
    all.sort_unstable();
    all.dedup();
    if all.len() != h.len() + b.len() {
        return None;
    }
    h.sort_unstable();
    b.sort_unstable();
    Some((h, b, n))
}

/// The seed string `odds-v1|<hole>|<board>|<n>` of the canonical input.
pub fn seed_string(h: &[Card], b: &[Card], n: u32) -> String {
    let j = |v: &[Card]| v.iter().map(|&c| card_str(c)).collect::<Vec<_>>().join(",");
    format!("odds-v1|{}|{}|{}", j(h), j(b), n)
}

fn choose(n: u64, k: u64) -> u64 {
    if k > n {
        return 0;
    }
    let mut r: u64 = 1;
    for i in 0..k {
        r = r * (n - i) / (i + 1);
    }
    r
}

/// E, saturating just above `ODDS_EXACT_MAX` (a preflop n = 7 product overflows 64 bits).
pub fn enumeration_count(d: u64, k: u64, n: u64) -> u64 {
    let mut e = choose(d, k).min(ODDS_EXACT_MAX + 1);
    for i in 0..n {
        if e > ODDS_EXACT_MAX {
            return ODDS_EXACT_MAX + 1;
        }
        e = e.saturating_mul(choose(d.saturating_sub(k + 2 * i), 2));
    }
    e.min(ODDS_EXACT_MAX + 1)
}

/// Win / tie counts of one deal: my score vs every opponent's.
fn judge(me: u32, opps: impl Iterator<Item = u32>) -> (u64, u64) {
    let best = opps.max().unwrap_or(0);
    if me > best {
        (1, 0)
    } else if me == best {
        (0, 1)
    } else {
        (0, 0)
    }
}

pub fn odds(hole: &[&str], board: &[&str], n: u32) -> Option<Odds> {
    let (h, b, n) = canonical(hole, board, n)?;
    let d: Vec<Card> = (0..52u8).filter(|c| !h.contains(c) && !b.contains(c)).collect();
    let k = 5 - b.len();
    let e = enumeration_count(d.len() as u64, k as u64, n as u64);
    let known: Vec<Card> = b.clone();
    let outs = if b.len() == 3 || b.len() == 4 { Some(outs_of(&h, &b, &d)) } else { None };
    let (mut win, mut tie, mut samples) = (0u64, 0u64, 0u64);
    let mut seed = None;
    if e <= ODDS_EXACT_MAX {
        // every runout (k-combination of D) × every ordered tuple of opponent hands
        let mut runout = vec![];
        exact_runouts(&d, k, 0, &mut runout, &mut |run: &[Card]| {
            let board_full: Vec<Card> = known.iter().chain(run.iter()).copied().collect();
            let me = score(&[&h[..], &board_full].concat());
            let rest: Vec<Card> = d.iter().copied().filter(|c| !run.contains(c)).collect();
            let mut opp_scores = vec![];
            exact_opps(&rest, n as usize, &board_full, &mut opp_scores, &mut |s: &[u32]| {
                let (w, t) = judge(me, s.iter().copied());
                win += w;
                tie += t;
                samples += 1;
            });
        });
    } else {
        let s = fnv1a32(&seed_string(&h, &b, n));
        seed = Some(s);
        let mut g = Mulberry32(s);
        let draw = k + 2 * n as usize;
        let mut deck = d.clone();
        let mut cards7: Vec<Card> = Vec::with_capacity(7);
        for _ in 0..ODDS_SAMPLES {
            deck.copy_from_slice(&d);
            for i in 0..draw {
                let j = i + g.below((deck.len() - i) as u32) as usize;
                deck.swap(i, j);
            }
            cards7.clear();
            cards7.extend_from_slice(&b);
            cards7.extend_from_slice(&deck[..k]);
            let board_full = cards7.clone();
            cards7.extend_from_slice(&h);
            let me = score(&cards7);
            let (w, t) = judge(
                me,
                (0..n as usize).map(|t| {
                    let mut o = board_full.clone();
                    o.push(deck[k + 2 * t]);
                    o.push(deck[k + 2 * t + 1]);
                    score(&o)
                }),
            );
            win += w;
            tie += t;
            samples += 1;
        }
    }
    Some(Odds { exact: e <= ODDS_EXACT_MAX, samples, win, tie, seed, outs })
}

fn exact_runouts(d: &[Card], k: usize, from: usize, cur: &mut Vec<Card>, f: &mut dyn FnMut(&[Card])) {
    if cur.len() == k {
        f(cur);
        return;
    }
    for i in from..d.len() {
        cur.push(d[i]);
        exact_runouts(d, k, i + 1, cur, f);
        cur.pop();
    }
}

fn exact_opps(rest: &[Card], n: usize, board: &[Card], scores: &mut Vec<u32>, f: &mut dyn FnMut(&[u32])) {
    if scores.len() == n {
        f(scores);
        return;
    }
    for i in 0..rest.len() {
        for j in i + 1..rest.len() {
            let mut o = board.to_vec();
            o.push(rest[i]);
            o.push(rest[j]);
            scores.push(score(&o));
            let left: Vec<Card> = rest.iter().enumerate().filter(|(x, _)| *x != i && *x != j).map(|(_, &c)| c).collect();
            exact_opps(&left, n, board, scores, f);
            scores.pop();
        }
    }
}

fn outs_of(h: &[Card], b: &[Card], d: &[Card]) -> (u32, BTreeMap<String, u32>) {
    let now = category_of(score(&[h, b].concat()));
    let mut by: BTreeMap<String, u32> = BTreeMap::new();
    let mut total = 0;
    for &c in d {
        let mine = category_of(score(&[h, b, &[c]].concat()));
        let alone = category_of(score(&[b, &[c]].concat()));
        if mine > now && mine > alone {
            *by.entry(crate::play_hand::CATEGORIES[mine].to_string()).or_default() += 1;
            total += 1;
        }
    }
    (total, by)
}

/// The shown percent: "0%", "100%", "<1%" below 0.5 %, ">99%" from 99.5 % (not 100 %), else whole percent half up.
pub fn label(num: u64, den: u64) -> String {
    if den == 0 || num == 0 {
        return "0%".into();
    }
    if num >= den {
        return "100%".into();
    }
    if num * 1000 < 5 * den {
        return "<1%".into();
    }
    if num * 1000 >= 995 * den {
        return ">99%".into();
    }
    format!("{}%", (num * 200 + den) / (2 * den))
}

/// The QML answer: {tag, win, tie, win_label, tie_label, samples, exact, outs: {total, by} | null}.
pub fn odds_json(tag: &str, hole: &[&str], board: &[&str], n: u32) -> serde_json::Value {
    use serde_json::json;
    match odds(hole, board, n) {
        None => json!({"tag": tag, "none": true}),
        Some(o) => json!({
            "tag": tag, "win": o.win, "tie": o.tie, "samples": o.samples, "exact": o.exact,
            "win_label": label(o.win, o.samples), "tie_label": label(o.tie, o.samples),
            "outs": o.outs.map(|(t, by)| json!({"total": t, "by": by})),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    const ODDS: &str = include_str!("testvectors/games/odds-v1.json");
    const RANKING: &str = include_str!("testvectors/games/holdem-hand-ranking.json");

    fn strs(v: &Value) -> Vec<&str> {
        v.as_array().unwrap().iter().map(|c| c.as_str().unwrap()).collect()
    }

    #[test]
    fn odds_rng_known_answers() {
        let v: Value = serde_json::from_str(ODDS).unwrap();
        let r = &v["rng_check"];
        for (s, want) in r["fnv1a32"].as_object().unwrap() {
            assert_eq!(fnv1a32(s) as u64, want.as_u64().unwrap(), "fnv1a32({s:?})");
        }
        assert_eq!(fnv1a32(r["seed_string"].as_str().unwrap()) as u64, r["seed"].as_u64().unwrap());
        let mut g = Mulberry32(r["seed"].as_u64().unwrap() as u32);
        let got: Vec<u64> = (0..8).map(|_| g.next_u32() as u64).collect();
        let want: Vec<u64> = r["first_8_u32"].as_array().unwrap().iter().map(|x| x.as_u64().unwrap()).collect();
        assert_eq!(got, want);
        // rejection, not plain modulo: r = 3 → lim = 2^32 − 1; the output 2^32 − 1 is rejected, the next one used
        let mut outs = [u32::MAX, 7].into_iter();
        assert_eq!(below_from(3, || outs.next().unwrap()), 1);
        let mut outs = [u32::MAX - 1, 7].into_iter();
        assert_eq!(below_from(3, || outs.next().unwrap()), (u32::MAX - 1) % 3);
    }

    #[test]
    fn odds_labels() {
        let v: Value = serde_json::from_str(ODDS).unwrap();
        let ls = v["labels"].as_array().unwrap();
        assert!(ls.len() >= 10);
        for l in ls {
            let (n, d) = (l["num"].as_u64().unwrap(), l["den"].as_u64().unwrap());
            assert_eq!(label(n, d), l["expect"].as_str().unwrap(), "{n}/{d}");
        }
    }

    #[test]
    fn odds_vectors() {
        let v: Value = serde_json::from_str(ODDS).unwrap();
        assert_eq!(v["exact_max"].as_u64(), Some(ODDS_EXACT_MAX));
        assert_eq!(v["samples"].as_u64(), Some(ODDS_SAMPLES as u64));
        let cases = v["cases"].as_array().unwrap();
        assert!(cases.len() >= 15, "too few odds cases");
        for c in cases {
            let id = c["id"].as_str().unwrap();
            let x = &c["expect"];
            let o = odds(&strs(&c["hole"]), &strs(&c["board"]), c["opponents"].as_u64().unwrap() as u32).unwrap_or_else(|| panic!("{id}: no odds"));
            assert_eq!(o.exact, x["method"] == "exact", "{id} method");
            assert_eq!(o.samples, x["samples"].as_u64().unwrap(), "{id} samples");
            assert_eq!((o.win, o.tie), (x["win"].as_u64().unwrap(), x["tie"].as_u64().unwrap()), "{id} win/tie");
            if let Some(s) = x["seed"].as_u64() {
                assert_eq!(o.seed, Some(s as u32), "{id} seed");
            }
            assert_eq!(label(o.win, o.samples), x["win_label"].as_str().unwrap(), "{id}");
            assert_eq!(label(o.tie, o.samples), x["tie_label"].as_str().unwrap(), "{id}");
            match &x["outs"] {
                Value::Null => assert!(o.outs.is_none(), "{id}: outs should be null"),
                w => {
                    let (t, by) = o.outs.clone().unwrap_or_else(|| panic!("{id}: no outs"));
                    assert_eq!(t as u64, w["total"].as_u64().unwrap(), "{id} outs total");
                    let want: BTreeMap<String, u32> = w["by"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), v.as_u64().unwrap() as u32)).collect();
                    assert_eq!(by, want, "{id} outs by");
                }
            }
        }
    }

    #[test]
    fn odds_refuses_bad_input() {
        for (h, b, n) in [
            (vec!["As"], vec![], 1),
            (vec!["As", "As"], vec![], 1),
            (vec!["As", "Kd"], vec!["2c"], 1),
            (vec!["As", "Kd"], vec!["As", "2c", "3c"], 1),
            (vec!["As", "Kd"], vec![], 0),
            (vec!["As", "Kd"], vec![], 8),
            (vec!["As", "Kx"], vec![], 1),
        ] {
            assert!(odds(&h, &b, n).is_none(), "{h:?} {b:?} {n}");
        }
        assert_eq!(enumeration_count(50, 5, 7), ODDS_EXACT_MAX + 1, "saturates, never overflows");
    }

    /// The fast evaluator orders every hand exactly as the reference (21 five-card subsets) does: the vectors + 3000
    /// seeded random 7-card hands; and a planted wrong rule (a flush below a straight) is caught.
    #[test]
    fn evaluator_matches_the_reference() {
        let v: Value = serde_json::from_str(RANKING).unwrap();
        let mut hands: Vec<Vec<Card>> = v["seven"].as_array().unwrap().iter().map(|t| strs(&t["cards"]).iter().map(|c| card(c).unwrap()).collect()).collect();
        let mut g = Mulberry32(12345);
        for _ in 0..3000 {
            let mut d: Vec<Card> = (0..52).collect();
            for i in 0..7 {
                let j = i + g.below((52 - i) as u32) as usize;
                d.swap(i, j);
            }
            hands.push(d[..7].to_vec());
        }
        let refkey = |h: &[Card]| -> Vec<u8> {
            let s: Vec<String> = h.iter().map(|&c| card_str(c)).collect();
            crate::play_hand::best_key(&s.iter().map(String::as_str).collect::<Vec<_>>()).unwrap()
        };
        for a in hands.iter().take(400) {
            for b in hands.iter().skip(400).take(60) {
                assert_eq!(score(a).cmp(&score(b)), refkey(a).cmp(&refkey(b)), "{a:?} vs {b:?}");
            }
            assert_eq!(category_of(score(a)), refkey(a)[0] as usize);
        }
        let planted = |h: &[Card]| -> u32 {
            let s = score(h);
            match category_of(s) { 5 => (4 << 20) | (s & 0xFFFFF), 4 => (5 << 20) | (s & 0xFFFFF), _ => s }
        };
        let fl: Vec<Card> = ["Ah", "Jh", "8h", "6h", "2h"].iter().map(|c| card(c).unwrap()).collect();
        let st: Vec<Card> = ["8d", "7c", "6s", "5h", "4d"].iter().map(|c| card(c).unwrap()).collect();
        assert_ne!(planted(&fl).cmp(&planted(&st)), refkey(&fl).cmp(&refkey(&st)), "the comparison can fail");
    }
}
