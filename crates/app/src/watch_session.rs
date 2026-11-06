//! Pad Watch (12.1.7, share claude/tinhat-pad/WATCH-1217/STATE-TABLE.md): the ONE place that decides whether a call
//! join WATCHES (no microphone, nothing captured or published until this device's own "Talk here").
//! Byte-compatible with the web's src/features/calls/watchsession.ts; both run the same case table
//! (watchsession.cases.json, sha256 pinned in both tests).
//!
//! The rule. A Watch press starts a WATCH EPISODE; every other press (Join, a switch, Try again, sign-in's auto-join)
//! starts a NORMAL one; Leave / sign-out / a give-up end the episode. Internal joins never choose a mode: a recovery
//! after a drop holds the TICKET (the episode id) it was given and joins only while that episode is still the current
//! one, in that episode's mode; a join queued behind a teardown is the episode that queued it. So a press, a Leave or a
//! sign-out during a teardown voids the recovery instead of letting it rejoin normally.
//! The capture BAR (tinhat_streamer::capture_barred) is set from the episode when its join starts and cleared only by
//! a NORMAL join's start or Talk here — never by a leave (fail closed: anything still in flight from a watch call stays
//! refused until the next normal join).

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Connecting,
    Live,
    Tearing,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Episode {
    pub id: u64,
    pub room: String,
    pub watch: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    None(&'static str),
    Join { id: u64, room: String, watch: bool },
    Queue { id: u64, room: String, watch: bool },
    LeaveThenJoin { id: u64, room: String, watch: bool },
    Leave,
    Abort(&'static str),
}

impl Decision {
    pub fn kind(&self) -> &'static str {
        match self {
            Decision::None(_) => "none",
            Decision::Join { .. } => "join",
            Decision::Queue { .. } => "queue",
            Decision::LeaveThenJoin { .. } => "leave-then-join",
            Decision::Leave => "leave",
            Decision::Abort(_) => "abort",
        }
    }
}

#[derive(Debug, Default)]
pub struct WatchLedger {
    seq: u64,
    pub ep: Option<Episode>,
    /// the episode queued behind a teardown (joined by teardown_done)
    pub pending: Option<u64>,
    pub bar: bool,
}

impl WatchLedger {
    pub const fn new() -> Self {
        WatchLedger { seq: 0, ep: None, pending: None, bar: false }
    }
    fn fresh(&mut self, room: &str, watch: bool) -> Episode {
        self.seq += 1;
        let e = Episode { id: self.seq, room: room.to_string(), watch };
        self.ep = Some(e.clone());
        e
    }
    fn join(e: &Episode) -> Decision {
        Decision::Join { id: e.id, room: e.room.clone(), watch: e.watch }
    }
    fn queue(e: &Episode) -> Decision {
        Decision::Queue { id: e.id, room: e.room.clone(), watch: e.watch }
    }
    fn current(&self, id: u64) -> bool {
        self.ep.as_ref().is_some_and(|e| e.id == id)
    }

    /// a Join press (call buttons, palette, Pad join, Answer, Try again, sign-in's auto-join): always a normal join
    pub fn press_join(&mut self, room: &str, phase: Phase) -> Decision {
        if room.is_empty() {
            return Decision::None("no room");
        }
        if matches!(phase, Phase::Live | Phase::Connecting) {
            return Decision::None("in a call");
        }
        let e = self.fresh(room, false);
        if phase == Phase::Tearing {
            self.pending = Some(e.id);
            return Self::queue(&e);
        }
        self.pending = None;
        Self::join(&e)
    }

    /// the Watch press: a watch join; refused while a call is up or connecting
    pub fn press_watch(&mut self, room: &str, phase: Phase) -> Decision {
        if room.is_empty() {
            return Decision::None("no room");
        }
        if matches!(phase, Phase::Live | Phase::Connecting) {
            return Decision::None("in a call");
        }
        let e = self.fresh(room, true);
        if phase == Phase::Tearing {
            self.pending = Some(e.id);
            return Self::queue(&e);
        }
        self.pending = None;
        Self::join(&e)
    }

    /// a switch press: the call already up (or joining) in that room stays as it is; any other room is a normal join
    pub fn press_switch(&mut self, room: &str, phase: Phase, cur: &str) -> Decision {
        if room.is_empty() {
            return Decision::None("no room");
        }
        if matches!(phase, Phase::Live | Phase::Connecting) && cur == room {
            return Decision::None("already there");
        }
        let e = self.fresh(room, false);
        if phase == Phase::Idle {
            self.pending = None;
            return Self::join(&e);
        }
        self.pending = Some(e.id);
        if phase == Phase::Tearing {
            Self::queue(&e)
        } else {
            Decision::LeaveThenJoin { id: e.id, room: e.room, watch: e.watch }
        }
    }

    /// Leave / Stop watching: ends the episode — a recovery or a queued join of it never runs (the bar stays as is)
    pub fn press_leave(&mut self) -> Decision {
        self.ep = None;
        self.pending = None;
        Decision::Leave
    }

    /// Talk here: the current watch episode becomes a normal one (its recoveries are normal joins) and capture opens
    pub fn talk(&mut self) -> bool {
        if self.pending.is_some() {
            return false;
        }
        match self.ep.as_mut() {
            Some(e) if e.watch => {
                e.watch = false;
                self.bar = false;
                true
            }
            _ => false,
        }
    }

    /// the join of episode `id` really starts: the bar follows that episode; None = not the current episode, don't join
    pub fn start_join(&mut self, id: u64) -> Option<bool> {
        let watch = self.ep.as_ref().filter(|e| e.id == id)?.watch;
        self.bar = watch;
        Some(watch)
    }

    /// the call in `room` dropped. Recoverable → the ticket a recovery must present (None: nothing to recover);
    /// not recoverable (kicked, the call ended) → the episode ends. A drop of a call already being left changes nothing.
    pub fn dropped(&mut self, room: &str, recoverable: bool) -> Option<u64> {
        let id = self.ep.as_ref().filter(|e| e.room == room)?.id;
        if self.pending.is_some() {
            return None;
        }
        if !recoverable {
            self.ep = None;
            return None;
        }
        Some(id)
    }

    /// a recovery (the auto-rejoin after a drop) holding `ticket`: joins only the episode that dropped, in its mode
    pub fn recover(&mut self, ticket: Option<u64>) -> Decision {
        let Some(t) = ticket else { return Decision::Abort("no ticket") };
        if !self.current(t) || self.pending.is_some() {
            return Decision::Abort("episode replaced or ended");
        }
        Self::join(self.ep.as_ref().expect("current"))
    }

    /// an internal go-ahead for the current call's join (the call-host sheet): the current episode, in its mode
    pub fn resume(&mut self, room: &str) -> Decision {
        match self.ep.as_ref() {
            Some(e) if e.room == room && self.pending.is_none() => Self::join(e),
            _ => Decision::Abort("not the current episode"),
        }
    }

    /// every teardown finished: the queued episode joins (still current), else nothing
    pub fn teardown_done(&mut self) -> Decision {
        let Some(id) = self.pending.take() else { return Decision::None("nothing queued") };
        if !self.current(id) {
            return Decision::Abort("queued episode replaced");
        }
        Self::join(self.ep.as_ref().expect("current"))
    }

    /// the join failed / was given up / the recovery budget ran out: the episode ends (the next join is a press)
    pub fn gave_up(&mut self) {
        if self.pending.is_none() {
            self.ep = None;
        }
    }

    /// the restart rejoin (the one deliberate carry-over): a new episode in the mode it was left in
    pub fn reload_rejoin(&mut self, room: &str, watching: bool, phase: Phase) -> Decision {
        if room.is_empty() || phase != Phase::Idle {
            return Decision::None("busy");
        }
        let e = self.fresh(room, watching);
        self.pending = None;
        Self::join(&e)
    }

    /// sign-out / session loss: no episode, nothing queued (the bar stays as is until a normal join)
    pub fn session_lost(&mut self) {
        self.ep = None;
        self.pending = None;
    }

    /// every teardown is done and no episode is current (left / given up / signed out): the bar may open — nothing of
    /// a watch call is running any more. While an episode is current (even a dropped one recovering) it changes nothing.
    pub fn settled(&mut self) -> bool {
        if self.ep.is_some() || self.pending.is_some() {
            return false;
        }
        self.bar = false;
        true
    }

    pub fn watching(&self) -> bool {
        self.ep.as_ref().is_some_and(|e| e.watch)
    }
}

/// The process-wide ledger (the Qt thread decides; the streamer's bar is pushed from start_join / talk).
pub fn ledger() -> std::sync::MutexGuard<'static, WatchLedger> {
    static L: std::sync::Mutex<WatchLedger> = std::sync::Mutex::new(WatchLedger::new());
    // a panic while holding it must not open anything: the state is still well-formed (every method is total)
    L.lock().unwrap_or_else(|p| p.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::collections::HashMap;

    const CASES: &str = include_str!("watchsession.cases.json");
    /// = the web's watchsession.test.ts CASES_SHA256 (the same bytes on both sides)
    const CASES_SHA256: &str = "9b9fefc3b45cbc77b57dda1d54ee9e8e9b27613c9eeb92a7cd881fe92d7dab7e";

    #[test]
    fn the_case_table_is_the_shared_one() {
        use sha2::Digest;
        let h = sha2::Sha256::digest(CASES.as_bytes());
        let hex: String = h.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, CASES_SHA256);
    }

    fn phase(v: &Value) -> Phase {
        match v.as_str().unwrap() {
            "idle" => Phase::Idle,
            "connecting" => Phase::Connecting,
            "live" => Phase::Live,
            "tearing" => Phase::Tearing,
            p => panic!("phase {p}"),
        }
    }

    fn run(name: &str, steps: &[Value]) {
        let mut l = WatchLedger::new();
        let mut saved: HashMap<String, Option<u64>> = HashMap::new();
        let refv = |saved: &HashMap<String, Option<u64>>, v: &Value| -> Option<u64> {
            match v.as_str() {
                None => None,
                Some(k) => *saved.get(k).unwrap_or_else(|| panic!("{name}: no saved {k}")),
            }
        };
        for (i, s) in steps.iter().enumerate() {
            let op = s["do"].as_str().unwrap();
            let at = format!("{name}: step {} {op}", i + 1);
            let st = |k: &str| s[k].as_str().unwrap_or_default().to_string();
            let d: Option<Decision> = match op {
                "pressJoin" => Some(l.press_join(&st("room"), phase(&s["phase"]))),
                "pressWatch" => Some(l.press_watch(&st("room"), phase(&s["phase"]))),
                "pressSwitch" => Some(l.press_switch(&st("room"), phase(&s["phase"]), &st("cur"))),
                "pressLeave" => Some(l.press_leave()),
                "recover" => Some(l.recover(refv(&saved, &s["ticket"]))),
                "resume" => Some(l.resume(&st("room"))),
                "teardownDone" => Some(l.teardown_done()),
                "reloadRejoin" => Some(l.reload_rejoin(&st("room"), s["watching"].as_bool().unwrap(), phase(&s["phase"]))),
                "gaveUp" => {
                    l.gave_up();
                    None
                }
                "settled" => {
                    assert_eq!(l.settled(), s["want"].as_bool().unwrap(), "{at}");
                    None
                }
                "sessionLost" => {
                    l.session_lost();
                    None
                }
                "talk" => {
                    assert_eq!(l.talk(), s["want"].as_bool().unwrap(), "{at}");
                    None
                }
                "startJoin" => {
                    let got = l.start_join(refv(&saved, &s["of"]).unwrap_or(u64::MAX));
                    let want = s["want"].get("watch").and_then(Value::as_bool);
                    assert_eq!(got, want, "{at}");
                    None
                }
                "dropped" => {
                    let t = l.dropped(&st("room"), s["recoverable"].as_bool().unwrap());
                    assert_eq!(t.is_some(), s["ticket"].as_bool().unwrap(), "{at}");
                    if let Some(k) = s["save"].as_str() {
                        saved.insert(k.to_string(), t);
                    }
                    None
                }
                o => panic!("unknown op {o}"),
            };
            if let Some(d) = d {
                let w = &s["want"];
                assert_eq!(d.kind(), w["kind"].as_str().unwrap(), "{at}: {d:?}");
                if let Decision::Join { id, room, watch } | Decision::Queue { id, room, watch } | Decision::LeaveThenJoin { id, room, watch } = &d {
                    if let Some(r) = w.get("room") {
                        assert_eq!(room, r.as_str().unwrap(), "{at}");
                    }
                    if let Some(x) = w.get("watch") {
                        assert_eq!(*watch, x.as_bool().unwrap(), "{at}");
                    }
                    if let Some(k) = s["save"].as_str() {
                        saved.insert(k.to_string(), Some(*id));
                    }
                }
            }
            if let Some(k) = s.get("epIs").and_then(Value::as_str) {
                let want = *saved.get(k).unwrap_or_else(|| panic!("{at}: no saved {k}"));
                assert_eq!(l.ep.as_ref().map(|e| e.id), want, "{at}: still the episode saved as {k}");
            }
            if let Some(c) = s.get("check") {
                assert_eq!(l.bar, c["bar"].as_bool().unwrap(), "{at}: bar");
                assert_eq!(l.watching(), c["watching"].as_bool().unwrap(), "{at}: watching");
            }
        }
    }

    fn row(i: usize) {
        let v: Value = serde_json::from_str(CASES).unwrap();
        let cases = v["cases"].as_array().unwrap();
        assert_eq!(cases.len(), 44, "a row was added: add its test below too");
        let c = &cases[i];
        let name = c["name"].as_str().unwrap();
        assert!(name.starts_with(&format!("R{:02} ", i + 1)), "{name}");
        run(name, c["steps"].as_array().unwrap());
    }

    /// one test per row of the state table (the web runs one per row too)
    macro_rules! rows {
        ($($n:ident = $i:expr,)*) => { $( #[test] fn $n() { row($i) } )* };
    }
    rows! {
        r01 = 0,
        r02 = 1,
        r03 = 2,
        r04 = 3,
        r05 = 4,
        r06 = 5,
        r07 = 6,
        r08 = 7,
        r09 = 8,
        r10 = 9,
        r11 = 10,
        r12 = 11,
        r13 = 12,
        r14 = 13,
        r15 = 14,
        r16 = 15,
        r17 = 16,
        r18 = 17,
        r19 = 18,
        r20 = 19,
        r21 = 20,
        r22 = 21,
        r23 = 22,
        r24 = 23,
        r25 = 24,
        r26 = 25,
        r27 = 26,
        r28 = 27,
        r29 = 28,
        r30 = 29,
        r31 = 30,
        r32 = 31,
        r33 = 32,
        r34 = 33,
        r35 = 34,
        r36 = 35,
        r37 = 36,
        r38 = 37,
        r39 = 38,
        r40 = 39,
        r41 = 40,
        r42 = 41,
        r43 = 42,
        r44 = 43,
    }
}
