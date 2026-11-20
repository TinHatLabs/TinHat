//! The prestige rank cache (GAMES-V1-SPEC rev 2 §3.3 / §3.4), pure and sans-IO: the engine (prestige.rs) feeds it the
//! clock and the answers, and asks it what to fetch when. Ranks are per SPACE: everything is keyed by a scope = (owner
//! server, space id), the owner being the server of the space's create-event sender (play_rules::game_space), never a via. Tin Hat Web's `src/features/prestige/rankCache.ts` is the same
//! machine (same constants; both replay `testvectors/prestige-wire.json`, byte copies; the machine tests below mirror
//! the web's by name).
//!
//!   get(scope, mxid, now)    the rank to draw now: 0 = no token (no scope, unknown, the owner has no games, older than
//!                            1 h); asking also queues a read when the answer is missing or older than 10 min
//!   push(owner, frame, now)  a {"t":"rank","space":…,"u":…,"rank":…} frame from that owner's play WebSocket (it can
//!                            only ever change that owner's scopes)
//!   due(now)                 what is due ([`Step::Base`] = read an owner's well-known first, [`Step::Ranks`] = GET
//!                            <base>/ranks?space=… for these ids), then `finish_*` with what came back
//!
//! Reads: ≤ 100 ids, one read per space in flight and at most one every 2 s (a 401's retry included: the session is
//! renewed for the next read, once; a second 401 in a row waits a minute). An owner whose well-known doesn't say
//! features.play === true (or that answers 503) draws no tokens and is asked again after 10 min, its names kept queued.
//! A failed read keeps what was known (≤ 1 h old) and is retried after a minute. A push newer than a read's start wins
//! over that read's answer. Names a label shows are `hold`-ed: they are re-read when their answer turns 10 min old and
//! re-queued after a failure; released names leave the queue; ranks older than 1 h are dropped (bounded memory).
//! forget_person(space, mxid) = a member left that space.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde_json::Value;

use crate::play_rules::valid_rank;

pub const BATCH_MAX: usize = 100;
pub const MIN_GAP_MS: u64 = 2_000;
/// the first read of a burst waits this long so one frame's names go out together
pub const GATHER_MS: u64 = 60;
pub const FRESH_MS: u64 = 10 * 60_000;
pub const MAX_AGE_MS: u64 = 60 * 60_000;
pub const SERVER_RECHECK_MS: u64 = 10 * 60_000;
pub const RETRY_MS: u64 = 60_000;

/// A user id the cache will ask about (and accept from a server): `@local:server`, no whitespace, ≤ 255 bytes.
pub fn valid_mxid(u: &str) -> bool {
    if u.len() > 255 || u.chars().any(char::is_whitespace) {
        return false;
    }
    let Some(rest) = u.strip_prefix('@') else { return false };
    match rest.find(':') {
        Some(i) => i > 0 && i + 1 < rest.len(),
        None => false,
    }
}

/// A space id as the cache takes it: a room id ("!…", ≤ 255 bytes, no whitespace).
pub fn valid_space(s: &str) -> bool {
    s.len() >= 2 && s.len() <= 255 && s.starts_with('!') && !s.chars().any(char::is_whitespace)
}

/// The ranks answer (§3.4) {"v":1,"space":"!S","ranks":{mxid: rank>0}}: for each asked id its rank (0 = absent / not a
/// rank); ids that weren't asked are ignored. Anything that isn't a version-1 answer for the asked space → None.
pub fn parse_ranks_answer(body: &Value, space: &str, asked: &[String]) -> Option<BTreeMap<String, i64>> {
    let o = body.as_object()?;
    if o.get("v").and_then(Value::as_f64) != Some(1.0) || o.get("space").and_then(Value::as_str) != Some(space) {
        return None;
    }
    let ranks = o.get("ranks")?.as_object()?;
    Some(asked.iter().map(|id| (id.clone(), ranks.get(id).map(valid_rank).unwrap_or(0))).collect())
}

/// A play-socket frame (§6.3) {"t":"rank","space":"!S","u":mxid,"rank":n ≥ 0} → (space, u, rank); else None (ignored).
pub fn parse_rank_push(frame: &Value) -> Option<(String, String, i64)> {
    let o = frame.as_object()?;
    if o.get("t")?.as_str()? != "rank" {
        return None;
    }
    let space = o.get("space")?.as_str()?;
    let u = o.get("u")?.as_str()?;
    if !valid_space(space) || !valid_mxid(u) {
        return None;
    }
    // a JSON number that is a whole number (2 and 2.0 alike: the web's Number.isSafeInteger), ≥ 0
    let n = match o.get("rank")? {
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i
            } else {
                let f = n.as_f64()?;
                if f.fract() != 0.0 || f.abs() > 9_007_199_254_740_991.0 {
                    return None;
                }
                f as i64
            }
        }
        _ => return None,
    };
    if !(0..=9_007_199_254_740_991).contains(&n) {
        return None;
    }
    Some((space.to_string(), u.to_string(), n))
}

/// Whose rank: the space and the server that keeps its chips (the server of the space's create-event sender).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Scope {
    pub owner: String,
    pub space: String,
}

impl Scope {
    pub fn key(&self) -> String {
        format!("{}|{}", self.owner, self.space)
    }
}

#[derive(Clone, Copy, Debug)]
struct Entry {
    rank: i64,
    at: u64,
}

#[derive(Default, Debug)]
struct OwnerState {
    /// None = not asked yet; Some(None) = no games there; Some(Some(base)) = `<api>/play/v1`
    base: Option<Option<String>>,
    base_at: u64,
    asking: bool,
    /// the last read was refused (401): the next one renews the session
    renew: bool,
}

#[derive(Default, Debug)]
struct ScopeState {
    ranks: HashMap<String, Entry>,
    queue: BTreeSet<String>,
    /// when the queue last went from empty to non-empty
    queued_at: u64,
    inflight: BTreeSet<String>,
    busy: bool,
    last_sent: Option<u64>,
    hold_until: u64,
    /// 401s in a row
    refused: u32,
    /// names a label shows (refcounts): re-read at 10 min, re-queued after a failure
    held: HashMap<String, u32>,
}

/// What the engine should do next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// read the owner's well-known (`finish_base`)
    Base { owner: String },
    /// GET <base>/ranks?space=…&u=… for `ids` (`finish_ranks` with `sent_at`); `renew` = exchange a new session first
    Ranks { scope: Scope, base: String, ids: Vec<String>, sent_at: u64, renew: bool },
}

/// How a ranks read ended.
#[derive(Clone, Debug)]
pub enum Outcome {
    /// HTTP 200 with this body
    Answer(Value),
    /// HTTP 503: games off (or draining) there
    Off,
    /// HTTP 401: the session was refused
    Refused,
    /// anything else: no answer, an error status, a refused session after the one renewal
    Failed,
}

#[derive(Default, Debug)]
pub struct RankCache {
    owners: BTreeMap<String, OwnerState>,
    scopes: BTreeMap<Scope, ScopeState>,
}

impl RankCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// The rank to draw now (0 = no token). Queues a read when missing or older than FRESH_MS.
    pub fn get(&mut self, scope: Option<&Scope>, mxid: &str, now: u64) -> i64 {
        let Some(sc) = scope.filter(|s| !s.owner.is_empty() && valid_space(&s.space)) else { return 0 };
        if !valid_mxid(mxid) {
            return 0;
        }
        self.owners.entry(sc.owner.clone()).or_default();
        let s = self.scopes.entry(sc.clone()).or_default();
        let e = s.ranks.get(mxid).copied();
        if e.is_none_or(|e| now.saturating_sub(e.at) >= FRESH_MS) && !s.inflight.contains(mxid) {
            if s.queue.is_empty() {
                s.queued_at = now;
            }
            s.queue.insert(mxid.to_string());
        }
        self.peek(sc, mxid, now)
    }

    /// The drawn value without queueing anything.
    pub fn peek(&self, sc: &Scope, mxid: &str, now: u64) -> i64 {
        if matches!(self.owners.get(&sc.owner).map(|o| &o.base), Some(Some(None))) {
            return 0;
        }
        match self.scopes.get(sc).and_then(|s| s.ranks.get(mxid)) {
            Some(e) if now.saturating_sub(e.at) < MAX_AGE_MS => e.rank,
            _ => 0,
        }
    }

    /// Every (scope key, mxid, rank > 0) drawn now.
    pub fn view(&self, now: u64) -> BTreeMap<String, BTreeMap<String, i64>> {
        let mut out = BTreeMap::new();
        for (sc, s) in &self.scopes {
            let m: BTreeMap<String, i64> = s
                .ranks
                .keys()
                .filter_map(|id| {
                    let r = self.peek(sc, id, now);
                    (r > 0).then(|| (id.clone(), r))
                })
                .collect();
            if !m.is_empty() {
                out.insert(sc.key(), m);
            }
        }
        out
    }

    /// A rank frame from `owner`'s play socket. True when it was a rank frame (applied, to that owner's space only).
    pub fn push(&mut self, owner: &str, frame: &Value, now: u64) -> bool {
        if owner.is_empty() {
            return false;
        }
        let Some((space, u, rank)) = parse_rank_push(frame) else { return false };
        self.owners.entry(owner.to_string()).or_default();
        let s = self.scopes.entry(Scope { owner: owner.to_string(), space }).or_default();
        s.ranks.insert(u, Entry { rank, at: now });
        true
    }

    /// A member left `space` (any owner): their token there goes at once (§1.1: no rank for a departed member). Recorded
    /// as rank 0 NOW, so an answer of a read that began before the departure can't bring it back.
    pub fn forget_person(&mut self, space: &str, mxid: &str, now: u64) {
        for (sc, s) in self.scopes.iter_mut() {
            if sc.space == space {
                s.queue.remove(mxid);
                s.ranks.insert(mxid.to_string(), Entry { rank: 0, at: now });
            }
        }
    }

    /// A label shows this name (idempotent; the engine keeps the per-label counts).
    pub fn hold(&mut self, scope: &Scope, mxid: &str) {
        self.scopes.entry(scope.clone()).or_default().held.insert(mxid.to_string(), 1);
    }

    /// No label shows it any more: the name leaves the queue (no retries for names nobody shows).
    pub fn release(&mut self, scope: &Scope, mxid: &str) {
        let Some(s) = self.scopes.get_mut(scope) else { return };
        s.held.remove(mxid);
        s.queue.remove(mxid);
    }

    /// Forget everything (sign-out / account switch).
    pub fn clear(&mut self) {
        self.owners.clear();
        self.scopes.clear();
    }

    fn base_known(o: &OwnerState, now: u64) -> bool {
        o.base.is_some() && now.saturating_sub(o.base_at) < SERVER_RECHECK_MS
    }

    fn due_at(o: &OwnerState, s: &ScopeState) -> Option<u64> {
        if s.queue.is_empty() || s.busy || o.asking {
            return None;
        }
        if matches!(o.base, Some(None)) {
            return Some(o.base_at + SERVER_RECHECK_MS);
        }
        let gap = s.last_sent.map(|t| t + MIN_GAP_MS).unwrap_or(0);
        Some((s.queued_at + GATHER_MS).max(gap).max(s.hold_until))
    }

    /// The next FUTURE moment a held name's answer turns 10 min old (a refresh) or any answer turns 1 h old (it
    /// disappears). Moments already past are handled by `age()` in the next `due()` (a queued name / a dropped answer).
    fn next_age_at(s: &ScopeState, now: u64) -> Option<u64> {
        let refresh = s.held.keys().filter_map(|id| s.ranks.get(id)).map(|e| e.at + FRESH_MS);
        let expiry = s.ranks.values().map(|e| e.at + MAX_AGE_MS);
        refresh.chain(expiry).filter(|t| *t > now).min()
    }

    /// When the engine must look again (a step due, a refresh, an expiry); None = nothing until another `get`.
    pub fn next_at(&self, now: u64) -> Option<u64> {
        self.scopes
            .iter()
            .flat_map(|(sc, s)| {
                let step = self.owners.get(&sc.owner).and_then(|o| Self::due_at(o, s));
                let age = Self::next_age_at(s, now);
                [step, age]
            })
            .flatten()
            .min()
    }

    /// Held names whose answer is 10 min old (or missing) go back in the queue; answers older than 1 h are dropped.
    fn age(&mut self, now: u64) {
        for s in self.scopes.values_mut() {
            s.ranks.retain(|_, e| now.saturating_sub(e.at) < MAX_AGE_MS);
            let stale: Vec<String> = s
                .held
                .keys()
                .filter(|id| !s.inflight.contains(*id) && !s.queue.contains(*id))
                .filter(|id| s.ranks.get(*id).is_none_or(|e| now.saturating_sub(e.at) >= FRESH_MS))
                .cloned()
                .collect();
            if !stale.is_empty() && s.queue.is_empty() {
                s.queued_at = now;
            }
            s.queue.extend(stale);
        }
    }

    /// The steps due now (an owner's well-known at most once at a time; each space at most one read in flight).
    pub fn due(&mut self, now: u64) -> Vec<Step> {
        self.age(now);
        let mut out = Vec::new();
        let keys: Vec<Scope> = self.scopes.keys().cloned().collect();
        for sc in keys {
            let Some(o) = self.owners.get(&sc.owner) else { continue };
            let Some(s) = self.scopes.get(&sc) else { continue };
            match Self::due_at(o, s) {
                Some(at) if at <= now => {}
                _ => continue,
            }
            if !Self::base_known(o, now) {
                if let Some(o) = self.owners.get_mut(&sc.owner) {
                    o.asking = true;
                }
                out.push(Step::Base { owner: sc.owner.clone() });
                continue;
            }
            let Some(Some(base)) = o.base.clone() else { continue };
            let renew = o.renew;
            if let Some(o) = self.owners.get_mut(&sc.owner) {
                o.renew = false;
            }
            let Some(s) = self.scopes.get_mut(&sc) else { continue };
            let ids: Vec<String> = s.queue.iter().take(BATCH_MAX).cloned().collect();
            for id in &ids {
                s.queue.remove(id);
                s.inflight.insert(id.clone());
            }
            s.busy = true;
            s.last_sent = Some(now);
            out.push(Step::Ranks { scope: sc, base, ids, sent_at: now, renew });
        }
        out
    }

    /// The owner's well-known read: `<api>/play/v1` when features.play === true, else None (no games there).
    pub fn finish_base(&mut self, owner: &str, base: Option<String>, now: u64) {
        let o = self.owners.entry(owner.to_string()).or_default();
        o.asking = false;
        o.base = Some(base);
        o.base_at = now;
    }

    /// `sent_at` = when the read itself went out (after any session exchange): the 2 s gap counts from there.
    pub fn finish_ranks(&mut self, scope: &Scope, ids: &[String], sent_at: u64, outcome: Outcome, now: u64) {
        let answer = match &outcome {
            Outcome::Answer(body) => parse_ranks_answer(body, &scope.space, ids),
            _ => None,
        };
        let s = self.scopes.entry(scope.clone()).or_default();
        s.busy = false;
        s.last_sent = Some(s.last_sent.unwrap_or(0).max(sent_at));
        for id in ids {
            s.inflight.remove(id);
        }
        if let Some(map) = answer {
            s.refused = 0;
            for (id, rank) in map {
                // a push (or a departure) that arrived once this read had begun — the same millisecond included — is
                // newer than its answer: it wins
                if s.ranks.get(&id).is_some_and(|e| e.at >= sent_at) {
                    continue;
                }
                s.ranks.insert(id, Entry { rank, at: now });
            }
            return;
        }
        // no answer: the names go back in the queue (only the ones a label still shows)
        if s.queue.is_empty() {
            s.queued_at = now;
        }
        for id in ids {
            if s.held.contains_key(id) {
                s.queue.insert(id.clone());
            }
        }
        match outcome {
            Outcome::Off => {
                let o = self.owners.entry(scope.owner.clone()).or_default();
                o.base = Some(None);
                o.base_at = now;
            }
            Outcome::Refused if s.refused == 0 => {
                // the session was refused: the next read (after the usual 2 s) renews it, once
                s.refused = 1;
                self.owners.entry(scope.owner.clone()).or_default().renew = true;
            }
            Outcome::Refused => {
                s.refused += 1;
                s.hold_until = now + RETRY_MS;
            }
            _ => {
                s.refused = 0;
                s.hold_until = now + RETRY_MS;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wire() -> Value {
        serde_json::from_str(include_str!("testvectors/prestige-wire.json")).unwrap()
    }

    fn sc(owner: &str, space: &str) -> Scope {
        Scope { owner: owner.into(), space: space.into() }
    }

    #[test]
    fn the_spec_numbers() {
        assert_eq!(BATCH_MAX, 100);
        assert_eq!(MIN_GAP_MS, 2_000);
        assert_eq!(FRESH_MS, 10 * 60_000);
        assert_eq!(MAX_AGE_MS, 60 * 60_000);
    }

    #[test]
    fn wire_the_ranks_answer() {
        let w = wire();
        assert_eq!(w["v"], 2);
        let cases = w["answers"].as_array().unwrap();
        assert!(cases.len() >= 19);
        for c in cases {
            let asked: Vec<String> = c["asked"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect();
            let got = parse_ranks_answer(&c["body"], c["space"].as_str().unwrap(), &asked).map(|m| serde_json::to_value(m).unwrap());
            let want = if c["want"].is_null() { None } else { Some(c["want"].clone()) };
            assert_eq!(got, want, "{}", c["id"]);
        }
    }

    #[test]
    fn wire_the_rank_push_frame() {
        let w = wire();
        let cases = w["pushes"].as_array().unwrap();
        assert!(cases.len() >= 18);
        for c in cases {
            let got = parse_rank_push(&c["frame"]).map(|(space, u, rank)| serde_json::json!({"space": space, "u": u, "rank": rank}));
            let want = if c["want"].is_null() { None } else { Some(c["want"].clone()) };
            assert_eq!(got, want, "{}", c["id"]);
        }
    }

    /// A fake ranks server + clock driving the cache the way prestige.rs does.
    struct Rig {
        cache: RankCache,
        now: u64,
        play: BTreeMap<String, bool>,
        ranks: BTreeMap<String, i64>,
        by_space: BTreeMap<String, BTreeMap<String, i64>>,
        answer_space: Option<String>,
        status: u16,
        refuse_next: u32,
        renewed: u32,
        fail_session: bool,
        /// (space, ids, time)
        requests: Vec<(String, usize, u64)>,
        /// a stalled well-known read (Base steps wait while true)
        base_stalled: bool,
        /// a read on the wire, answered later (finish_read)
        parked: Option<(Scope, Vec<String>, u64)>,
        park_next: bool,
        /// the first session exchange takes this long (the read goes out after it)
        session_delay: u64,
    }

    impl Rig {
        fn new(play: &[(&str, bool)], ranks: &[(&str, i64)]) -> Self {
            Rig {
                cache: RankCache::new(),
                now: 1_000_000,
                play: play.iter().map(|(s, b)| (s.to_string(), *b)).collect(),
                ranks: ranks.iter().map(|(s, r)| (s.to_string(), *r)).collect(),
                by_space: BTreeMap::new(),
                answer_space: None,
                status: 200,
                refuse_next: 0,
                renewed: 0,
                fail_session: false,
                requests: Vec::new(),
                base_stalled: false,
                parked: None,
                park_next: false,
                session_delay: 0,
            }
        }

        /// a label shows this name
        fn show(&mut self, scope: &Scope, mxid: &str) {
            self.cache.hold(scope, mxid);
        }

        /// move the clock WITHOUT running anything (the age check in get() must hold on its own)
        fn jump(&mut self, ms: u64) {
            self.now += ms;
        }

        fn finish_read(&mut self) {
            if let Some((scope, ids, sent_at)) = self.parked.take() {
                let o = self.serve(&scope.space, &ids);
                let now = self.now;
                self.cache.finish_ranks(&scope, &ids, sent_at, o, now);
            }
        }

        fn serve(&mut self, space: &str, ids: &[String]) -> Outcome {
            if self.fail_session {
                return Outcome::Failed;
            }
            self.requests.push((space.to_string(), ids.len(), self.now));
            assert!(ids.len() <= BATCH_MAX, "batch over 100");
            if self.refuse_next > 0 {
                self.refuse_next -= 1;
                return Outcome::Refused;
            }
            match self.status {
                200 => {
                    let table = self.by_space.get(space).unwrap_or(&self.ranks);
                    let mut out = serde_json::Map::new();
                    for id in ids {
                        if let Some(r) = table.get(id).filter(|r| **r > 0) {
                            out.insert(id.clone(), Value::from(*r));
                        }
                    }
                    let named = self.answer_space.clone().unwrap_or_else(|| space.to_string());
                    Outcome::Answer(serde_json::json!({"v": 1, "space": named, "ranks": out}))
                }
                503 => Outcome::Off,
                _ => Outcome::Failed,
            }
        }

        /// run every due step up to now + ms, in time order
        fn advance(&mut self, ms: u64) {
            let end = self.now + ms;
            let mut spins = 0u32;
            loop {
                // a machine that keeps producing work without the clock moving is a bug: fail, never hang
                spins += 1;
                assert!(spins < 100_000, "rig: no progress (the cache keeps asking for the same step)");
                let Some(at) = self.cache.next_at(self.now) else { break };
                if at > end {
                    break;
                }
                self.now = self.now.max(at);
                let mut progressed = false;
                for step in self.cache.due(self.now) {
                    progressed = true;
                    match step {
                        Step::Base { owner } => {
                            if self.base_stalled {
                                continue; // the read hangs: the owner stays "asking"
                            }
                            let b = self.play.get(&owner).copied().unwrap_or(false).then(|| format!("https://{owner}/api/play/v1"));
                            self.cache.finish_base(&owner, b, self.now);
                        }
                        Step::Ranks { scope, base: _, ids, sent_at, renew } => {
                            if renew {
                                self.renewed += 1;
                            }
                            if self.park_next {
                                self.park_next = false;
                                self.requests.push((scope.space.clone(), ids.len(), self.now));
                                self.parked = Some((scope, ids, sent_at));
                                continue;
                            }
                            // a slow first session exchange: the read itself goes out later
                            let sent = sent_at + std::mem::take(&mut self.session_delay);
                            self.now = self.now.max(sent);
                            let o = self.serve(&scope.space, &ids);
                            self.cache.finish_ranks(&scope, &ids, sent, o, self.now);
                        }
                    }
                }
                if !progressed && self.cache.next_at(self.now) == Some(at) && at <= self.now {
                    // an age-only wake-up (refresh / expiry) with nothing to send: move on
                    self.now += 1;
                }
            }
            self.now = end;
        }

        fn get(&mut self, scope: &Scope, mxid: &str) -> i64 {
            self.cache.get(Some(scope), mxid, self.now)
        }
    }

    #[test]
    fn machine_unknown_asked_once_then_drawn() {
        let s = sc("s.test", "!S");
        let mut r = Rig::new(&[("s.test", true)], &[("@a:s.test", 4)]);
        assert_eq!(r.get(&s, "@a:s.test"), 0);
        assert_eq!(r.get(&s, "@a:s.test"), 0);
        r.advance(100);
        assert_eq!(r.get(&s, "@a:s.test"), 4);
        assert_eq!(r.requests.len(), 1);
        assert_eq!(r.get(&s, "not-an-id"), 0);
        assert_eq!(r.cache.get(None, "@a:s.test", r.now), 0);
        assert_eq!(r.get(&sc("s.test", "S"), "@a:s.test"), 0, "not a space id");
        r.advance(5_000);
        assert_eq!(r.requests.len(), 1);
    }

    #[test]
    fn machine_250_names_in_one_render() {
        let s = sc("s.test", "!S");
        let mut r = Rig::new(&[("s.test", true)], &[]);
        for i in 0..250 {
            r.get(&s, &format!("@p{i}:s.test"));
        }
        r.advance(10_000);
        assert_eq!(r.requests.iter().map(|q| q.1).collect::<Vec<_>>(), vec![100, 100, 50]);
        for w in r.requests.windows(2) {
            assert!(w[1].2 - w[0].2 >= 2_000, "one request per space per 2 s (spec §3.4)");
        }
    }

    #[test]
    fn machine_fresh_10_min_never_older_than_1_h() {
        let s = sc("s.test", "!S");
        let mut r = Rig::new(&[("s.test", true)], &[("@a:s.test", 2)]);
        r.show(&s, "@a:s.test");
        r.get(&s, "@a:s.test");
        r.advance(100);
        assert_eq!(r.get(&s, "@a:s.test"), 2);
        r.advance(FRESH_MS - 1_000);
        r.get(&s, "@a:s.test");
        r.advance(100);
        assert_eq!(r.requests.len(), 1, "no read inside 10 min");
        r.advance(2_000);
        assert_eq!(r.requests.len(), 2, "a held name is re-read at 10 min by itself (no get needed)");
        r.advance(2_000);
        r.status = 500;
        assert_eq!(r.get(&s, "@a:s.test"), 2, "still drawn while reads fail (< 1 h)");
        r.advance(MAX_AGE_MS);
        assert_eq!(r.get(&s, "@a:s.test"), 0, "older than 1 h: no token");
        r.status = 200;
        r.advance(RETRY_MS + 3_000);
        assert_eq!(r.get(&s, "@a:s.test"), 2, "back once a read works");
    }

    #[test]
    fn machine_server_without_games() {
        let s = sc("s.test", "!S");
        let mut r = Rig::new(&[("s.test", false)], &[("@a:s.test", 5)]);
        r.get(&s, "@a:s.test");
        r.advance(5_000);
        assert_eq!(r.get(&s, "@a:s.test"), 0);
        assert_eq!(r.requests.len(), 0);
        r.play.insert("s.test".into(), true);
        r.advance(SERVER_RECHECK_MS + 3_000);
        assert_eq!(r.get(&s, "@a:s.test"), 5);
    }

    #[test]
    fn machine_refused_session_renewed_once() {
        let s = sc("s.test", "!S");
        let mut r = Rig::new(&[("s.test", true)], &[("@a:s.test", 3)]);
        r.show(&s, "@a:s.test");
        r.refuse_next = 1;
        r.get(&s, "@a:s.test");
        r.advance(3_000);
        assert_eq!(r.get(&s, "@a:s.test"), 3);
        assert_eq!(r.renewed, 1);
        assert_eq!(r.requests.len(), 2);
        assert!(r.requests[1].2 - r.requests[0].2 >= 2_000, "the retry waits for the 2 s gap");
    }

    #[test]
    fn machine_games_switched_off() {
        let s = sc("s.test", "!S");
        let mut r = Rig::new(&[("s.test", true)], &[("@a:s.test", 3)]);
        r.get(&s, "@a:s.test");
        r.advance(100);
        assert_eq!(r.get(&s, "@a:s.test"), 3);
        r.status = 503;
        r.advance(FRESH_MS + 100);
        r.get(&s, "@a:s.test");
        r.advance(100);
        assert_eq!(r.get(&s, "@a:s.test"), 0);
    }

    #[test]
    fn machine_no_session() {
        let s = sc("s.test", "!S");
        let mut r = Rig::new(&[("s.test", true)], &[("@a:s.test", 3)]);
        r.fail_session = true;
        r.get(&s, "@a:s.test");
        r.advance(RETRY_MS + 5_000);
        assert_eq!(r.get(&s, "@a:s.test"), 0);
    }

    #[test]
    fn machine_push() {
        let s = sc("s.test", "!S");
        let mut r = Rig::new(&[("s.test", true)], &[("@a:s.test", 1)]);
        r.get(&s, "@a:s.test");
        r.advance(100);
        let now = r.now;
        assert!(r.cache.push("s.test", &serde_json::json!({"t": "rank", "space": "!S", "u": "@a:s.test", "rank": 2}), now));
        assert_eq!(r.get(&s, "@a:s.test"), 2);
        assert!(!r.cache.push("s.test", &serde_json::json!({"t": "rank", "space": "!S", "u": "@a:s.test", "rank": "9"}), now));
        assert_eq!(r.get(&s, "@a:s.test"), 2);
        assert_eq!(r.get(&sc("other.test", "!S"), "@a:s.test"), 0);
    }

    #[test]
    fn machine_ranks_are_per_owner() {
        let mut r = Rig::new(&[("s.test", true), ("t.test", true)], &[("@a:s.test", 4)]);
        r.get(&sc("s.test", "!S"), "@a:s.test");
        r.get(&sc("t.test", "!S"), "@a:s.test");
        r.advance(100);
        let now = r.now;
        r.cache.push("t.test", &serde_json::json!({"t": "rank", "space": "!S", "u": "@a:s.test", "rank": 1}), now);
        assert_eq!(r.get(&sc("s.test", "!S"), "@a:s.test"), 4, "a push from one server never touches another server's space");
        assert_eq!(r.get(&sc("t.test", "!S"), "@a:s.test"), 1);
    }

    #[test]
    fn machine_clear() {
        let s = sc("s.test", "!S");
        let mut r = Rig::new(&[("s.test", true)], &[("@a:s.test", 4)]);
        r.get(&s, "@a:s.test");
        r.advance(100);
        r.cache.clear();
        assert_eq!(r.get(&s, "@a:s.test"), 0);
    }

    #[test]
    fn machine_ranks_are_per_space() {
        let (s1, s2) = (sc("s.test", "!S"), sc("s.test", "!T"));
        let mut r = Rig::new(&[("s.test", true)], &[]);
        r.by_space.insert("!S".into(), [("@a:s.test".to_string(), 4)].into_iter().collect());
        r.by_space.insert("!T".into(), [("@a:s.test".to_string(), 1)].into_iter().collect());
        r.get(&s1, "@a:s.test");
        r.get(&s2, "@a:s.test");
        r.advance(100);
        assert_eq!(r.get(&s1, "@a:s.test"), 4);
        assert_eq!(r.get(&s2, "@a:s.test"), 1);
        let mut spaces: Vec<&str> = r.requests.iter().map(|q| q.0.as_str()).collect();
        spaces.sort();
        assert_eq!(spaces, vec!["!S", "!T"]);
        let now = r.now;
        r.cache.push("s.test", &serde_json::json!({"t": "rank", "space": "!T", "u": "@a:s.test", "rank": 2}), now);
        assert_eq!(r.get(&s1, "@a:s.test"), 4, "a push names its space");
        assert_eq!(r.get(&s2, "@a:s.test"), 2);
    }

    #[test]
    fn machine_answer_naming_another_space() {
        let s = sc("s.test", "!S");
        let mut r = Rig::new(&[("s.test", true)], &[("@a:s.test", 4)]);
        r.show(&s, "@a:s.test");
        r.answer_space = Some("!X".into());
        r.get(&s, "@a:s.test");
        r.advance(100);
        assert_eq!(r.get(&s, "@a:s.test"), 0);
        r.answer_space = None;
        r.advance(RETRY_MS + 3_000);
        assert_eq!(r.get(&s, "@a:s.test"), 4, "control: the asked space's answer is drawn");
    }

    #[test]
    fn machine_expiry_at_1_h_without_a_render() {
        let s = sc("s.test", "!S");
        let mut r = Rig::new(&[("s.test", true)], &[("@a:s.test", 4)]);
        r.show(&s, "@a:s.test");
        r.get(&s, "@a:s.test");
        r.advance(100);
        assert_eq!(r.cache.peek(&s, "@a:s.test", r.now), 4);
        r.status = 500; // refreshes fail from now on
        assert!(r.cache.next_at(r.now).is_some_and(|t| t <= r.now + FRESH_MS), "the engine is woken for the refresh");
        r.advance(MAX_AGE_MS);
        assert_eq!(r.cache.peek(&s, "@a:s.test", r.now), 0, "1 h: gone, with no get() in between");
    }

    #[test]
    fn machine_one_read_per_space_while_discovery_stalls() {
        let s = sc("s.test", "!S");
        let mut r = Rig::new(&[("s.test", true)], &[]);
        r.base_stalled = true;
        for i in 0..150 {
            r.get(&s, &format!("@p{i}:s.test"));
        }
        r.advance(5_000);
        for i in 150..160 {
            r.get(&s, &format!("@p{i}:s.test"));
        }
        r.advance(5_000);
        assert_eq!(r.requests.len(), 0);
        r.base_stalled = false;
        // the stalled read finally answers
        let now = r.now;
        r.cache.finish_base("s.test", Some("https://s.test/api/play/v1".into()), now);
        r.advance(20_000);
        assert!(r.requests.iter().all(|q| q.1 <= 100));
        for w in r.requests.windows(2) {
            assert!(w[1].2 - w[0].2 >= 2_000, "never two reads of one space within 2 s");
        }
        assert_eq!(r.requests.iter().map(|q| q.1).sum::<usize>(), 160);
    }

    #[test]
    fn machine_push_newer_than_a_read_wins() {
        let s = sc("s.test", "!S");
        let mut r = Rig::new(&[("s.test", true)], &[("@a:s.test", 1)]);
        r.show(&s, "@a:s.test");
        r.park_next = true;
        r.get(&s, "@a:s.test");
        r.advance(100); // the read is on the wire
        let now = r.now;
        r.cache.push("s.test", &serde_json::json!({"t": "rank", "space": "!S", "u": "@a:s.test", "rank": 2}), now);
        r.now += 50;
        r.finish_read();
        assert_eq!(r.get(&s, "@a:s.test"), 2, "the older answer (1) doesn't undo the prestige");
    }

    #[test]
    fn machine_503_keeps_names_queued() {
        let s = sc("s.test", "!S");
        let mut r = Rig::new(&[("s.test", true)], &[("@a:s.test", 3)]);
        r.show(&s, "@a:s.test");
        r.status = 503;
        r.get(&s, "@a:s.test");
        r.advance(100);
        // (no get() here: it would queue the name again by itself)
        r.status = 200;
        r.advance(SERVER_RECHECK_MS + 3_000);
        assert_eq!(r.cache.peek(&s, "@a:s.test", r.now), 3);
    }

    #[test]
    fn machine_member_who_left_loses_the_token() {
        let (s1, s2) = (sc("s.test", "!S"), sc("s.test", "!T"));
        let mut r = Rig::new(&[("s.test", true)], &[("@a:s.test", 3), ("@b:s.test", 2)]);
        r.get(&s1, "@a:s.test");
        r.get(&s1, "@b:s.test");
        r.get(&s2, "@a:s.test");
        r.advance(3_000);
        let now = r.now;
        r.cache.forget_person("!S", "@a:s.test", now);
        assert_eq!(r.get(&s1, "@a:s.test"), 0);
        assert_eq!(r.get(&s1, "@b:s.test"), 2);
        assert_eq!(r.get(&s2, "@a:s.test"), 3, "another space is untouched");
    }

    #[test]
    fn machine_released_names_leave_the_queue() {
        let s = sc("s.test", "!S");
        let mut r = Rig::new(&[("s.test", true)], &[("@a:s.test", 3)]);
        r.show(&s, "@a:s.test");
        r.status = 500;
        r.get(&s, "@a:s.test");
        r.advance(100);
        r.cache.release(&s, "@a:s.test");
        let before = r.requests.len();
        r.advance(10 * RETRY_MS);
        assert_eq!(r.requests.len(), before);
    }

    #[test]
    fn machine_get_alone_never_draws_older_than_1_h() {
        let s = sc("s.test", "!S");
        let mut r = Rig::new(&[("s.test", true)], &[("@a:s.test", 4)]);
        r.get(&s, "@a:s.test");
        r.advance(100);
        assert_eq!(r.get(&s, "@a:s.test"), 4);
        r.jump(MAX_AGE_MS);
        assert_eq!(r.get(&s, "@a:s.test"), 0);
    }

    #[test]
    fn machine_departure_beats_an_older_answer() {
        let s = sc("s.test", "!S");
        let mut r = Rig::new(&[("s.test", true)], &[("@a:s.test", 3)]);
        r.show(&s, "@a:s.test");
        r.park_next = true;
        r.get(&s, "@a:s.test");
        r.advance(100); // the read is on the wire
        let now = r.now;
        r.cache.forget_person("!S", "@a:s.test", now);
        r.now += 50;
        r.finish_read();
        assert_eq!(r.cache.peek(&s, "@a:s.test", r.now), 0);
    }

    #[test]
    fn machine_push_in_the_same_millisecond_wins() {
        let s = sc("s.test", "!S");
        let mut r = Rig::new(&[("s.test", true)], &[("@a:s.test", 1)]);
        r.show(&s, "@a:s.test");
        r.park_next = true;
        r.get(&s, "@a:s.test");
        r.advance(60); // exactly when the read starts
        let sent = r.parked.as_ref().map(|p| p.2).unwrap();
        r.cache.push("s.test", &serde_json::json!({"t": "rank", "space": "!S", "u": "@a:s.test", "rank": 2}), sent);
        r.finish_read();
        assert_eq!(r.cache.peek(&s, "@a:s.test", r.now), 2);
    }

    #[test]
    fn machine_gap_counts_from_the_read_itself() {
        let s = sc("s.test", "!S");
        let mut r = Rig::new(&[("s.test", true)], &[]);
        r.session_delay = 8_000;
        for i in 0..150 {
            r.get(&s, &format!("@p{i}:s.test"));
        }
        r.advance(30_000);
        assert_eq!(r.requests.len(), 2);
        assert!(r.requests[1].2 - r.requests[0].2 >= 2_000, "never two reads of one space within 2 s");
    }

    #[test]
    fn view_lists_only_drawn_tokens() {
        let s = sc("s.test", "!S");
        let mut r = Rig::new(&[("s.test", true)], &[("@a:s.test", 4)]);
        r.get(&s, "@a:s.test");
        r.get(&s, "@b:s.test");
        r.advance(100);
        let v = r.cache.view(r.now);
        assert_eq!(serde_json::to_value(&v).unwrap(), serde_json::json!({"s.test|!S": {"@a:s.test": 4}}));
    }

    /// The byte copy equals Tin Hat Web's when a web checkout is named by `TINHAT_WEB_DIR` (skipped otherwise).
    #[test]
    fn wire_vectors_are_the_webs() {
        let Some(dir) = std::env::var_os("TINHAT_WEB_DIR") else { return };
        let path = std::path::Path::new(&dir).join("src/features/prestige/testvectors/prestige-wire.json");
        let theirs = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert_eq!(include_str!("testvectors/prestige-wire.json"), theirs);
    }
}
