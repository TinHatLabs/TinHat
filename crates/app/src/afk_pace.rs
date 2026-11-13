//! Pacing for the legacy 💤 read of a call's member events (`afk_ctl`).
//!
//! The read is one `/relations` request per member event, and the away poll runs every 3 s
//! for the whole call. Almost always the answer is "nothing there": current builds carry
//! BRB on the `dev.tinhat.afk` state event, read locally. So a member event that shows no
//! 💤 is asked again later and later, up to `MAX_WAIT`; a brand-new member event (someone
//! joined or rejoined) is read at once. An event that does show a 💤 is rechecked at
//! `FOUND_RECHECK`, so a legacy peer's "I'm back" is still seen soon.
//!
//! Between reads the last answer stands, so a skipped poll can't flicker a tile. Pure (the
//! clock is passed in), so the schedule is unit-tested without a server.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

pub const FIRST_WAIT: Duration = Duration::from_secs(30);
pub const MAX_WAIT: Duration = Duration::from_secs(5 * 60);
pub const FOUND_RECHECK: Duration = Duration::from_secs(15);

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Pace {
    /// Spend a request, then hand this ticket and the outcome to `settle`.
    Read(u64),
    /// Don't ask: this was the last answer.
    Known(bool),
}

struct Entry {
    next_at: Instant,
    wait: Duration,
    away: bool,
    /// The read this entry is waiting for; an older read's outcome is ignored.
    ticket: u64,
}

#[derive(Default)]
pub struct RelationsPace {
    entries: HashMap<String, Entry>,
    last_ticket: u64,
}

impl RelationsPace {
    /// Should this poll read `event_id`? A `Read` claims the slot: overlapping polls get the
    /// last answer, and a read that never settles frees the slot after `MAX_WAIT`.
    pub fn decide(&mut self, event_id: &str, now: Instant) -> Pace {
        if let Some(e) = self.entries.get(event_id) {
            if now < e.next_at {
                return Pace::Known(e.away);
            }
        }
        self.last_ticket += 1;
        let ticket = self.last_ticket;
        let e = self.entries.entry(event_id.to_string()).or_insert(Entry {
            next_at: now,
            wait: Duration::ZERO,
            away: false,
            ticket,
        });
        e.next_at = now + MAX_WAIT;
        e.ticket = ticket;
        Pace::Read(ticket)
    }

    /// The outcome of a `Read`: `Some(found)`, or `None` if the request failed. Returns the
    /// answer that now stands. No 💤 backs off, because the state event still carries BRB
    /// meanwhile. A failure says nothing, so the last answer stands: a confirmed 💤 is asked
    /// again at `FOUND_RECHECK`, anything else backs off as if empty.
    pub fn settle(&mut self, event_id: &str, ticket: u64, outcome: Option<bool>, now: Instant) -> bool {
        let Some(e) = self.entries.get_mut(event_id) else { return false }; // forgotten or pruned
        if e.ticket != ticket {
            return e.away; // overtaken by a newer read (or our own 💤 changed meanwhile)
        }
        e.away = outcome.unwrap_or(e.away);
        if e.away {
            if outcome.is_some() {
                e.wait = Duration::ZERO;
            }
            e.next_at = now + FOUND_RECHECK;
        } else {
            e.wait = if e.wait.is_zero() { FIRST_WAIT } else { (e.wait * 2).min(MAX_WAIT) };
            e.next_at = now + e.wait;
        }
        e.away
    }

    /// Our own 💤 just changed: the next poll must look.
    pub fn forget(&mut self, event_id: &str) {
        self.entries.remove(event_id);
    }

    /// Drop everything that is no longer a member event of the call.
    pub fn retain(&mut self, event_ids: &HashSet<&str>) {
        self.entries.retain(|id, _| event_ids.contains(id.as_str()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Poll like the app (every 3 s) for `secs`; `found` answers each read. Returns reads.
    fn poll(p: &mut RelationsPace, ids: &[String], start: Instant, secs: u64, found: bool) -> usize {
        let mut reads = 0;
        for t in (0..secs).step_by(3) {
            let now = start + Duration::from_secs(t);
            for id in ids {
                if let Pace::Read(t) = p.decide(id, now) {
                    reads += 1;
                    p.settle(id, t, Some(found), now);
                }
            }
        }
        reads
    }

    fn read(p: &mut RelationsPace, id: &str, now: Instant) -> u64 {
        match p.decide(id, now) {
            Pace::Read(t) => t,
            other => panic!("expected a read of {id}, got {other:?}"),
        }
    }

    fn ids(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("$event{i}")).collect()
    }

    #[test]
    fn empty_answers_end_the_poll_flood() {
        // The field report: 17 member events, 8 minutes, every answer empty = 2497 requests.
        let reads = poll(&mut RelationsPace::default(), &ids(17), Instant::now(), 480, false);
        assert_eq!(reads, 17 * 5, "0, 30, 90, 210, 450 s per event");
    }

    #[test]
    fn a_long_call_stays_bounded_by_the_cap() {
        let reads = poll(&mut RelationsPace::default(), &ids(1), Instant::now(), 3600, false);
        assert!(reads <= 15, "an hour of empty answers took {reads} reads");
    }

    #[test]
    fn someone_new_is_read_at_once() {
        let (mut p, t0) = (RelationsPace::default(), Instant::now());
        poll(&mut p, &ids(3), t0, 300, false);
        read(&mut p, "$rejoined", t0 + Duration::from_secs(300));
    }

    #[test]
    fn a_found_marker_stands_between_reads_and_is_rechecked() {
        let (mut p, t0) = (RelationsPace::default(), Instant::now());
        let t = read(&mut p, "$a", t0);
        assert!(p.settle("$a", t, Some(true), t0));
        assert_eq!(p.decide("$a", t0 + Duration::from_secs(3)), Pace::Known(true));
        let t = read(&mut p, "$a", t0 + FOUND_RECHECK);
        // The peer came back: the tile clears on that read, and backing off starts over.
        assert!(!p.settle("$a", t, Some(false), t0 + FOUND_RECHECK));
        assert_eq!(p.decide("$a", t0 + FOUND_RECHECK + Duration::from_secs(3)), Pace::Known(false));
        read(&mut p, "$a", t0 + FOUND_RECHECK + FIRST_WAIT);
    }

    #[test]
    fn overlapping_polls_share_one_read_and_a_lost_read_recovers() {
        let (mut p, t0) = (RelationsPace::default(), Instant::now());
        read(&mut p, "$a", t0);
        assert_eq!(p.decide("$a", t0 + Duration::from_secs(3)), Pace::Known(false));
        read(&mut p, "$a", t0 + MAX_WAIT); // never settled
    }

    #[test]
    fn a_failed_read_does_not_erase_a_confirmed_marker() {
        let (mut p, t0) = (RelationsPace::default(), Instant::now());
        let t = read(&mut p, "$a", t0);
        p.settle("$a", t, Some(true), t0);
        let at = t0 + FOUND_RECHECK;
        let t = read(&mut p, "$a", at);
        assert!(p.settle("$a", t, None, at), "a timeout says nothing about the 💤");
        assert_eq!(p.decide("$a", at + Duration::from_secs(3)), Pace::Known(true));
        read(&mut p, "$a", at + FOUND_RECHECK); // and it is asked again soon
    }

    #[test]
    fn a_read_overtaken_by_our_own_change_is_ignored() {
        let (mut p, t0) = (RelationsPace::default(), Instant::now());
        let slow = read(&mut p, "$own", t0); // sees the 💤 just before it is redacted
        p.forget("$own");
        let fresh = read(&mut p, "$own", t0);
        assert!(!p.settle("$own", fresh, Some(false), t0));
        assert!(!p.settle("$own", slow, Some(true), t0), "the older read must not win");
        assert_eq!(p.decide("$own", t0 + Duration::from_secs(3)), Pace::Known(false));
    }

    #[test]
    fn forget_and_retain() {
        let (mut p, t0) = (RelationsPace::default(), Instant::now());
        poll(&mut p, &ids(2), t0, 3, false);
        let now = t0 + Duration::from_secs(3);
        p.forget("$event0");
        read(&mut p, "$event0", now);
        p.retain(&HashSet::from(["$event0"]));
        assert_eq!(p.entries.len(), 1);
        assert!(!p.settle("$gone", 1, Some(true), now), "an outdated read reports nothing");
        assert_eq!(p.entries.len(), 1);
    }
}
