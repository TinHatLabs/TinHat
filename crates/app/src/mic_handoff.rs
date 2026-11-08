//! mic_handoff.rs — PAD-PHONE Step B: the computer's side of "my phone has the mic" (docs/PAD.md "Phone as Mic /
//! Camera"; the web's engine.ts takeover / maybeRestore / takenBack / autoTakeUntil). Pure: call_ctl.rs feeds it the
//! mic's flags, what the call server says about my phone's Pad Mic connections and the clock, and carries out the
//! actions it returns.
//!
//! The rules (clients review rounds 1 + 2):
//! - A HOLD is a mute REASON of its own: a Pad session took the mic (`takeover.mic`), or "auto": a Pad Mic connection of
//!   my person was transmitting when this call started / during its first 15 s. Taking it mutes; `prior` = the mic was
//!   muted, or the user asked for a mute meanwhile (any mute request counts, even when the mic already reads muted).
//! - Never both live: the local mic comes on only once no Pad Mic connection of my person transmits — every known one
//!   unpublished or muted, none of unknown state.
//! - EVERY unmute request (the mic button, push-to-talk, the Pad's `mic {muted:false}`, "Take the mic back", BRB's
//!   return) while a hold or a pending restore exists is the user taking the mic back: ONE deferred handoff — the hold
//!   ends, every Pad Mic connection of mine is remembered (never auto-taken again), the Pad sessions learn take:false,
//!   and the mic comes on once the phone stopped. A mute request cancels a pending restore.
//! - The release of a hold (the phone gave it back, its session ended, the auto connection left) restores the mic only
//!   if it wasn't muted by `prior`, and never over a moderator's mute, push-to-talk or BRB.
//! - Clients round 3: the BARRIER (a released / relinquished phone may still be sending: set by every release and every
//!   unmute request while held, cleared only once no Pad Mic of mine publishes an unmuted mic) is separate from the WANT
//!   (the mic should come back). A mute clears only the want — every later unmute request while the barrier is up waits
//!   again. No automatic acquisition while the barrier is up, and an automatic hold keeps a pending want. A want blocked
//!   by a moderator's mute (or, for a release, push-to-talk / BRB) is kept, not dropped: push-to-talk's key-down
//!   completes once safe, its key-up (a mute request) cancels it.
//! - Clients round 4: fulfilling ANY want (a release's, a user's, the explicit unmute while a want exists) re-checks
//!   right then that no Pad Mic of mine publishes an unmuted mic (unknown = not stopped) — a phone live again puts the
//!   barrier back up and the want waits. Push-to-talk with the key up blocks every want (a user's too).
//! - Clients round 5: an unmute request is unconditional ONLY with no hold, no barrier and no pending want; with a want
//!   pending every request goes through the phone check and the blockers. The blockers are the same for every want and
//!   for the web: a moderator's mute, push-to-talk with the key up, BRB, a watch call.
use std::collections::HashSet;

/// How long after joining the auto rule applies (ms).
pub const AUTO_WINDOW_MS: u64 = 15_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HoldBy {
    /// a Pad session (its sid) took the mic
    Pad(String),
    /// a Pad Mic connection (its identity) was transmitting as this call started
    Auto(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hold {
    pub by: HoldBy,
    /// the mic stays off when the hold ends
    pub prior_muted: bool,
}

/// What the caller does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Act {
    /// mute the local mic (the hold's own mute: never counted as the user's)
    Mute,
    /// unmute the local mic now
    Unmute,
    /// tell the Pad sessions take:false (the receiver's takeover record ends, every session gets a fresh state)
    CancelSession,
    Status(&'static str),
}

pub const TAKING_BACK: &str = "Taking the mic back from your phone…";
pub const MUTE_ON_PHONE: &str = "Your phone is still sending — mute it on your phone (This phone · Stop)";
pub const BACK_ON: &str = "Your mic is back on";
pub const PHONE_HAS_IT: &str = "Your phone has the mic — muted here";

/// The local mic's flags.
#[derive(Clone, Copy, Debug, Default)]
pub struct Mic {
    pub muted: bool,
    pub server_muted: bool,
    /// push-to-talk is on and the key is up
    pub ptt: bool,
    pub afk: bool,
    /// this device only watches the call (no capture)
    pub watching: bool,
}
impl Mic {
    /// Nothing may turn the mic on now (the same for every want).
    fn blocked(&self) -> bool {
        self.server_muted || self.ptt || self.afk || self.watching
    }
}

/// My person's Pad Mic connections in the call, as the call server says (publications, not subscriptions).
#[derive(Clone, Debug, Default)]
pub struct Phone {
    /// present, with an unmuted mic publication
    pub live: Vec<String>,
    /// present, mic state not known yet
    pub unknown: Vec<String>,
    /// every Pad Mic connection of mine present
    pub present: Vec<String>,
}
impl Phone {
    pub fn stopped(&self) -> bool {
        self.live.is_empty() && self.unknown.is_empty()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Restore {
    /// the user asked for the mic (only a moderator's mute stops it)
    User,
    /// a hold ended (prior-mute / moderator / push-to-talk / BRB all keep it off)
    Release,
}

#[derive(Debug, Default)]
pub struct MicHandoff {
    pub hold: Option<Hold>,
    /// the WANT: the mic should come back (and why)
    restore: Option<Restore>,
    /// the BARRIER: a released / relinquished phone may still be sending — nothing unmutes until it stopped
    barrier: bool,
    /// Pad Mic identities the user took the mic back from: never auto-taken again (this call)
    pub taken_back: HashSet<String>,
    auto_until: u64,
}

impl MicHandoff {
    pub fn held(&self) -> bool {
        self.hold.is_some()
    }
    pub fn restore_pending(&self) -> bool {
        self.restore.is_some()
    }
    pub fn barrier(&self) -> bool {
        self.barrier
    }
    /// The Pad session holding the mic ("" = none / auto).
    pub fn holder_sid(&self) -> &str {
        match &self.hold {
            Some(Hold { by: HoldBy::Pad(s), .. }) => s,
            _ => "",
        }
    }

    /// The call is up and its roster known (before any unmuted mic of mine is published): a new call's state; a Pad Mic
    /// of mine transmitting in it holds the mic at once (the muted join).
    pub fn joined(&mut self, now: u64, mic: Mic, phone: &Phone) -> Vec<Act> {
        *self = MicHandoff { auto_until: now + AUTO_WINDOW_MS, ..MicHandoff::default() };
        self.auto(now, mic, phone)
    }
    /// The call ended.
    pub fn left(&mut self) {
        *self = MicHandoff::default();
    }

    /// `takeover.mic {on:true}` ran for Pad session `sid`.
    pub fn pad_take(&mut self, sid: &str, mic: Mic) -> Vec<Act> {
        match &mut self.hold {
            Some(h) => {
                h.by = HoldBy::Pad(sid.to_string());
                Vec::new()
            }
            None => {
                // (round 3) a pending want survives the new hold: the release decides with it
                self.hold = Some(Hold { by: HoldBy::Pad(sid.to_string()), prior_muted: mic.muted });
                vec![Act::Mute, Act::Status("Your phone has the mic")]
            }
        }
    }
    /// The hold of Pad session `sid` ended (it gave the mic back, or its session ended): only that session's.
    pub fn pad_release(&mut self, sid: &str, mic: Mic, phone: &Phone) -> Vec<Act> {
        if self.holder_sid() != sid || sid.is_empty() {
            return Vec::new();
        }
        self.release(mic, phone)
    }
    fn release(&mut self, mic: Mic, phone: &Phone) -> Vec<Act> {
        let Some(h) = self.hold.take() else { return Vec::new() };
        self.barrier = true; // the phone may still be sending
        if !h.prior_muted && self.restore.is_none() {
            self.restore = Some(Restore::Release);
        }
        self.try_restore(mic, phone)
    }

    /// ANY unmute request. Nothing held or pending: unmute now. Otherwise the one deferred handoff.
    /// `has_session`: a Pad session of the holding phone is open here (it can hear take:false).
    pub fn user_unmute(&mut self, mic: Mic, phone: &Phone, has_session: bool) -> Vec<Act> {
        // (round 5/6) the ONLY early return: nothing held, no barrier, no pending want
        if self.hold.is_none() && !self.barrier && self.restore.is_none() {
            return vec![Act::Unmute];
        }
        // every other request: the common bookkeeping — the Pad Mic connections of mine TRANSMITTING now (round 7: not
        // silent or merely present ones; an unknown state still blocks the restore, in its own check) and a relinquished
        // automatic holder are remembered as taken back (never auto-taken again), the hold ends, and the request is a
        // want through the restore check
        let was = self.hold.take();
        let relinquish = was.is_some() || self.barrier;
        for id in &phone.live {
            self.taken_back.insert(id.clone());
        }
        let auto_only = matches!(was, Some(Hold { by: HoldBy::Auto(_), .. }));
        if let Some(Hold { by: HoldBy::Auto(id), .. }) = &was {
            self.taken_back.insert(id.clone());
        }
        self.restore = Some(Restore::User);
        if was.is_some() {
            self.barrier = true; // relinquished: the phone may still be sending
        }
        let mut acts = if relinquish { vec![Act::CancelSession] } else { Vec::new() };
        let now = self.try_restore(mic, phone);
        if now.is_empty() && self.barrier {
            acts.push(Act::Status(if auto_only && !has_session { MUTE_ON_PHONE } else { TAKING_BACK }));
        }
        acts.extend(now);
        acts
    }

    /// ANY mute request by the user (the hold's own mute never comes here): the mic stays off when a hold ends, and a
    /// pending restore is cancelled.
    pub fn user_mute(&mut self) {
        if let Some(h) = &mut self.hold {
            h.prior_muted = true;
        }
        self.restore = None; // the want only: the barrier stays (round 3 #1)
    }

    /// The clock (≈ 500 ms) and every roster / publication change.
    pub fn tick(&mut self, now: u64, mic: Mic, phone: &Phone) -> Vec<Act> {
        let mut acts = Vec::new();
        if let Some(Hold { by: HoldBy::Auto(id), .. }) = &self.hold {
            if !phone.present.contains(id) {
                acts.extend(self.release(mic, phone));
            }
        }
        acts.extend(self.try_restore(mic, phone));
        acts.extend(self.auto(now, mic, phone));
        acts
    }

    fn try_restore(&mut self, mic: Mic, phone: &Phone) -> Vec<Act> {
        if self.hold.is_some() {
            return Vec::new(); // a hold decides at its release (the want is kept)
        }
        if self.barrier && phone.stopped() {
            self.barrier = false;
        }
        if self.barrier {
            return Vec::new();
        }
        let Some(k) = self.restore else { return Vec::new() };
        if !mic.muted {
            self.restore = None; // already on
            return Vec::new();
        }
        // (round 4 #1) fulfilling re-checks the phone right now: live again (or unknown) = the barrier goes back up
        if !phone.stopped() {
            self.barrier = true;
            return Vec::new();
        }
        // blocked (a moderator's mute, push-to-talk's key up, BRB, a watch call — every want alike): the want waits,
        // it is not dropped
        let _ = k;
        if mic.blocked() {
            return Vec::new();
        }
        self.restore = None;
        vec![Act::Unmute, Act::Status(BACK_ON)]
    }

    fn auto(&mut self, now: u64, mic: Mic, phone: &Phone) -> Vec<Act> {
        // (round 3 #2) never while a release is unresolved (the barrier)
        if self.hold.is_some() || self.barrier || now > self.auto_until {
            return Vec::new();
        }
        let Some(id) = phone.live.iter().find(|x| !self.taken_back.contains(*x)) else { return Vec::new() };
        // a pending want is kept (the release decides with it)
        self.hold = Some(Hold { by: HoldBy::Auto(id.clone()), prior_muted: mic.muted && self.restore.is_none() });
        vec![Act::Mute, Act::Status(PHONE_HAS_IT)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: &str = "@me:h.test:PHONE~pad-mic-0123456789abcdef";
    fn on() -> Mic {
        Mic { muted: false, ..Mic::default() }
    }
    fn muted() -> Mic {
        Mic { muted: true, ..Mic::default() }
    }
    fn live() -> Phone {
        Phone { live: vec![P.into()], unknown: vec![], present: vec![P.into()] }
    }
    fn stopped() -> Phone {
        Phone { live: vec![], unknown: vec![], present: vec![P.into()] }
    }
    fn unknown() -> Phone {
        Phone { live: vec![], unknown: vec![P.into()], present: vec![P.into()] }
    }
    fn gone() -> Phone {
        Phone::default()
    }

    #[test]
    fn every_unmute_while_held_is_one_deferred_handoff() {
        // (round 2 #1) the mic button / PTT / the Pad's unmute while the phone transmits: no unmute yet
        let mut h = MicHandoff::default();
        h.joined(0, on(), &gone());
        assert_eq!(h.pad_take("S1", on()), vec![Act::Mute, Act::Status("Your phone has the mic")]);
        let acts = h.user_unmute(muted(), &live(), true);
        assert_eq!(acts, vec![Act::CancelSession, Act::Status(TAKING_BACK)]);
        assert!(!h.held() && h.restore_pending());
        assert!(h.taken_back.contains(P), "the relinquished Pad Mic is remembered");
        // still transmitting / unknown: nothing
        assert!(h.tick(100, muted(), &live()).is_empty());
        assert!(h.tick(200, muted(), &unknown()).is_empty(), "an unknown state never authorises the restore");
        // never auto re-taken within the joining window
        assert!(!h.held());
        // the phone stopped: now the mic comes on
        assert_eq!(h.tick(300, muted(), &stopped()), vec![Act::Unmute, Act::Status(BACK_ON)]);
        assert!(!h.restore_pending());
    }

    /// clients round 3 #1: unmute → mute → unmute while the phone still transmits: nothing until it stopped
    #[test]
    fn unmute_mute_unmute_while_the_phone_is_live_waits_for_it() {
        let mut h = MicHandoff::default();
        h.joined(0, on(), &gone());
        h.pad_take("S1", on());
        assert!(!h.user_unmute(muted(), &live(), true).contains(&Act::Unmute));
        h.user_mute();
        let again = h.user_unmute(muted(), &live(), true);
        assert!(!again.contains(&Act::Unmute), "the barrier holds: {again:?}");
        assert!(h.tick(100, muted(), &live()).is_empty());
        assert_eq!(h.tick(200, muted(), &stopped()), vec![Act::Unmute, Act::Status(BACK_ON)]);
        // and after unmute → mute, the phone stopping restores nothing (the want was cancelled)
        let mut h = MicHandoff::default();
        h.joined(0, on(), &gone());
        h.pad_take("S1", on());
        h.user_unmute(muted(), &live(), true);
        h.user_mute();
        assert!(h.tick(100, muted(), &stopped()).is_empty());
        assert_eq!(h.user_unmute(muted(), &stopped(), true), vec![Act::Unmute], "then an unmute is immediate");
    }

    /// clients round 3 #2: a release still unresolved (the phone appears live) is never swallowed by an auto hold
    #[test]
    fn no_auto_hold_during_an_unresolved_release_and_an_auto_hold_keeps_the_want() {
        let mut h = MicHandoff::default();
        h.joined(0, on(), &gone());
        h.pad_take("S1", on());
        assert!(h.pad_release("S1", muted(), &live()).is_empty(), "waits for the phone");
        assert!(h.tick(1_000, muted(), &live()).is_empty(), "no automatic re-take while the release is unresolved");
        assert!(!h.held());
        assert_eq!(h.tick(2_000, muted(), &stopped()), vec![Act::Unmute, Act::Status(BACK_ON)], "the original intent wins");
        // an auto hold that replaces a want blocked by a moderator's mute keeps that want
        let mut h = MicHandoff::default();
        h.joined(0, on(), &gone());
        h.pad_take("S1", on());
        h.user_unmute(muted(), &live(), true);
        let sm = Mic { muted: true, server_muted: true, ..Mic::default() };
        assert!(h.tick(100, sm, &stopped()).is_empty(), "blocked: kept");
        assert!(h.restore_pending());
        let other = Phone { live: vec!["@me:h.test:TAB~pad-mic-fedcba9876543210".into()], unknown: vec![], present: vec!["@me:h.test:TAB~pad-mic-fedcba9876543210".into()] };
        // (round 4 #1) a phone of mine live again while that want waits = the barrier is back up: no auto hold, and the
        // want survives until the phone is silent and nothing blocks it
        assert!(h.tick(200, sm, &other).is_empty());
        assert!(!h.held());
        assert!(h.tick(250, muted(), &other).is_empty(), "the phone is live: still waiting");
        assert_eq!(h.tick(300, muted(), &gone()), vec![Act::Unmute, Act::Status(BACK_ON)], "the user's want survived");
        // a Pad session's hold replacing a pending want keeps it too
        let mut h = MicHandoff::default();
        h.joined(0, on(), &gone());
        h.pad_take("S1", on());
        h.user_unmute(muted(), &live(), true);
        assert!(h.tick(100, sm, &stopped()).is_empty());
        h.pad_take("S2", muted());
        assert!(h.pad_release("S2", muted(), &stopped()).contains(&Act::Unmute), "the want decides at the release");
    }

    /// clients round 3 #3 (kept): push-to-talk's key-down completes once safe; its key-up cancels the want
    #[test]
    fn ptt_key_down_completes_once_safe_and_key_up_cancels() {
        let down = Mic { muted: true, ..Mic::default() }; // the key is held: not ptt-blocked
        let mut h = MicHandoff::default();
        h.joined(0, on(), &gone());
        h.pad_take("S1", on());
        assert!(!h.user_unmute(down, &live(), true).contains(&Act::Unmute));
        assert_eq!(h.tick(100, down, &stopped()), vec![Act::Unmute, Act::Status(BACK_ON)]);
        let mut h = MicHandoff::default();
        h.joined(0, on(), &gone());
        h.pad_take("S1", on());
        h.user_unmute(down, &live(), true);
        h.user_mute(); // key-up
        assert!(h.tick(100, Mic { muted: true, ptt: true, ..Mic::default() }, &stopped()).is_empty());
    }

    /// clients round 4 #1: a want blocked earlier is fulfilled only while my phone is silent RIGHT THEN — on the clock
    /// and on the explicit unmute's fast path
    #[test]
    fn a_want_is_fulfilled_only_while_the_phone_is_silent_right_then() {
        let sm = Mic { muted: true, server_muted: true, ..Mic::default() };
        // the clock: blocked by a moderator while the phone was silent; unblocked when the phone is live again
        let mut h = MicHandoff::default();
        h.joined(0, on(), &gone());
        h.pad_take("S1", on());
        h.user_unmute(muted(), &live(), true);
        assert!(h.tick(100, sm, &stopped()).is_empty());
        assert!(h.tick(200, muted(), &live()).is_empty(), "the phone is live again: no unmute");
        assert_eq!(h.tick(300, muted(), &stopped()), vec![Act::Unmute, Act::Status(BACK_ON)]);
        // the explicit unmute while that want exists
        let mut h = MicHandoff::default();
        h.joined(0, on(), &gone());
        h.pad_take("S1", on());
        h.user_unmute(muted(), &live(), true);
        assert!(h.tick(100, sm, &stopped()).is_empty());
        let fast = h.user_unmute(muted(), &live(), true);
        assert!(!fast.contains(&Act::Unmute), "{fast:?}");
        assert!(h.tick(200, muted(), &unknown()).is_empty(), "unknown is not silent");
        assert_eq!(h.tick(300, muted(), &stopped()), vec![Act::Unmute, Act::Status(BACK_ON)]);
    }

    /// clients round 4 #2: push-to-talk's key up blocks a user's want (it waits for key-down; key-up's mute cancels)
    #[test]
    fn ptt_key_up_blocks_a_users_want() {
        let up = Mic { muted: true, ptt: true, ..Mic::default() };
        let mut h = MicHandoff::default();
        h.joined(0, on(), &gone());
        h.pad_take("S1", on());
        h.user_unmute(up, &live(), true);
        assert!(h.tick(100, up, &stopped()).is_empty(), "key up: waits");
        assert_eq!(h.tick(200, muted(), &stopped()), vec![Act::Unmute, Act::Status(BACK_ON)], "key down: on");
    }

    /// clients round 5 #1: with a want pending, a second unmute request is no bypass — it meets the blockers too
    #[test]
    fn a_second_unmute_request_with_a_want_pending_still_meets_the_blockers() {
        let up = Mic { muted: true, ptt: true, ..Mic::default() };
        let mut h = MicHandoff::default();
        h.joined(0, on(), &gone());
        h.pad_take("S1", on());
        assert!(!h.user_unmute(up, &stopped(), true).contains(&Act::Unmute));
        assert!(!h.user_unmute(up, &stopped(), true).contains(&Act::Unmute), "the second request: key still up");
        assert_eq!(h.tick(100, muted(), &stopped()), vec![Act::Unmute, Act::Status(BACK_ON)], "key-down");
        // variant: key-up cancels the want
        let mut h = MicHandoff::default();
        h.joined(0, on(), &gone());
        h.pad_take("S1", on());
        h.user_unmute(up, &stopped(), true);
        h.user_unmute(up, &stopped(), true);
        h.user_mute();
        assert!(h.tick(100, muted(), &stopped()).is_empty());
    }

    /// clients round 5 #2 (web parity): BRB blocks a user's want too, and so does a watch call
    #[test]
    fn brb_and_watching_block_a_users_want() {
        for blocker in [Mic { muted: true, afk: true, ..Mic::default() }, Mic { muted: true, watching: true, ..Mic::default() }] {
            let mut h = MicHandoff::default();
            h.joined(0, on(), &gone());
            h.pad_take("S1", on());
            assert!(!h.user_unmute(blocker, &stopped(), true).contains(&Act::Unmute), "{blocker:?}");
            assert!(h.tick(100, blocker, &stopped()).is_empty(), "{blocker:?}");
            assert!(!h.user_unmute(blocker, &stopped(), true).contains(&Act::Unmute), "{blocker:?}");
            assert_eq!(h.tick(200, muted(), &stopped()), vec![Act::Unmute, Act::Status(BACK_ON)], "back from it: {blocker:?}");
        }
    }

    /// clients round 6: an unmute request with a want pending records my live Pad Mic as taken back too, so the same
    /// connection publishing again inside the joining window is never auto-taken after the restore
    #[test]
    fn a_pending_want_request_remembers_the_phone_as_taken_back() {
        let sm = Mic { muted: true, server_muted: true, ..Mic::default() };
        let mut h = MicHandoff::default();
        h.joined(0, on(), &gone());
        h.pad_take("S1", on());
        assert!(h.pad_release("S1", sm, &live()).is_empty(), "released while blocked, the phone still sending");
        assert!(h.tick(1_000, sm, &stopped()).is_empty(), "the phone stopped: the barrier clears, the want stays");
        assert!(h.restore_pending());
        // the phone publishes again; the user asks for the mic before the next tick
        assert!(!h.user_unmute(muted(), &live(), true).contains(&Act::Unmute), "waits for the phone");
        assert_eq!(h.tick(2_000, muted(), &stopped()), vec![Act::Unmute, Act::Status(BACK_ON)]);
        // the same Pad Mic connection publishes again inside the 15 s window: not auto-taken
        assert!(h.tick(3_000, on(), &live()).is_empty());
        assert!(!h.held());
    }

    /// clients round 7: only the TRANSMITTING phone is remembered as taken back — a second phone of mine that was silent
    /// then may still be auto-taken when it starts transmitting inside the joining window
    #[test]
    fn only_the_transmitting_phone_is_taken_back() {
        let a = "@me:h.test:PHONEA~pad-mic-0123456789abcdef".to_string();
        let b = "@me:h.test:PHONEB~pad-mic-fedcba9876543210".to_string();
        let ph = |live: &[&String]| Phone { live: live.iter().map(|x| (*x).clone()).collect(), unknown: vec![], present: vec![a.clone(), b.clone()] };
        let mut h = MicHandoff::default();
        assert_eq!(h.joined(0, on(), &ph(&[&a])), vec![Act::Mute, Act::Status(PHONE_HAS_IT)], "A transmits: auto hold");
        assert!(!h.user_unmute(muted(), &ph(&[&a]), true).contains(&Act::Unmute), "take A back: waits for A");
        assert_eq!(h.tick(1_000, muted(), &ph(&[])), vec![Act::Unmute, Act::Status(BACK_ON)], "A stopped: restored");
        h.user_mute(); // the computer is muted again
        assert_eq!(h.tick(5_000, muted(), &ph(&[&b])), vec![Act::Mute, Act::Status(PHONE_HAS_IT)], "B starts: auto-taken");
        assert!(h.held());
        assert!(!h.user_unmute(muted(), &ph(&[&b]), true).contains(&Act::Unmute), "a later unmute is deferred");
        // A transmitting again is still never auto-taken
        let mut h = MicHandoff::default();
        h.joined(0, on(), &ph(&[&a]));
        h.user_unmute(muted(), &ph(&[&a]), true);
        h.tick(1_000, muted(), &ph(&[]));
        h.user_mute();
        assert!(h.tick(5_000, muted(), &ph(&[&a])).is_empty());
    }

    #[test]
    fn nothing_held_unmutes_at_once() {
        let mut h = MicHandoff::default();
        h.joined(0, muted(), &gone());
        assert_eq!(h.user_unmute(muted(), &gone(), false), vec![Act::Unmute]);
    }

    #[test]
    fn a_muted_join_when_my_phone_is_in_the_roster() {
        // (round 2 #2) the roster (publications, regardless of subscription) is known before the mic publishes
        let mut h = MicHandoff::default();
        assert_eq!(h.joined(0, on(), &live()), vec![Act::Mute, Act::Status(PHONE_HAS_IT)]);
        assert!(h.held());
        // the phone muted itself later: the hold stays until it leaves (never both live on a re-unmute)
        assert!(h.tick(1_000, muted(), &stopped()).is_empty());
        assert_eq!(h.tick(2_000, muted(), &gone()), vec![Act::Unmute, Act::Status(BACK_ON)], "the auto connection left: back on");
    }

    #[test]
    fn a_mute_request_is_kept_even_when_the_mic_already_reads_muted() {
        // (round 2 #3) the user mutes while the takeover's mute already holds: the release must not unmute
        let mut h = MicHandoff::default();
        h.joined(0, on(), &gone());
        h.pad_take("S1", on());
        h.user_mute();
        assert!(h.pad_release("S1", muted(), &stopped()).is_empty());
        // and a mute while a restore is pending cancels it
        let mut h = MicHandoff::default();
        h.joined(0, on(), &gone());
        h.pad_take("S1", on());
        assert!(h.pad_release("S1", muted(), &live()).is_empty(), "waits for the phone");
        assert!(h.restore_pending());
        h.user_mute();
        assert!(!h.restore_pending());
        assert!(h.tick(100, muted(), &stopped()).is_empty());
    }

    #[test]
    fn take_back_of_an_auto_hold_without_a_session_says_mute_it_on_the_phone_and_keeps_waiting() {
        // (round 2 #6) no Pad session = no take:false the phone could hear: say so, keep deferring
        let mut h = MicHandoff::default();
        h.joined(0, on(), &live());
        let acts = h.user_unmute(muted(), &live(), false);
        assert_eq!(acts, vec![Act::CancelSession, Act::Status(MUTE_ON_PHONE)]);
        assert!(h.tick(20_000, muted(), &live()).is_empty());
        assert!(!h.held(), "never auto re-taken");
        assert_eq!(h.tick(30_000, muted(), &stopped()), vec![Act::Unmute, Act::Status(BACK_ON)]);
        // with a session from that phone: the ordinary message
        let mut h = MicHandoff::default();
        h.joined(0, on(), &live());
        assert_eq!(h.user_unmute(muted(), &live(), true), vec![Act::CancelSession, Act::Status(TAKING_BACK)]);
    }

    #[test]
    fn release_respects_prior_moderator_ptt_and_brb_but_a_user_request_only_the_moderator() {
        let mut h = MicHandoff::default();
        h.joined(0, muted(), &gone());
        h.pad_take("S1", muted());
        assert!(h.pad_release("S1", muted(), &stopped()).is_empty(), "was muted when taken");
        for m in [Mic { muted: true, server_muted: true, ..Mic::default() }, Mic { muted: true, ptt: true, ..Mic::default() }, Mic { muted: true, afk: true, ..Mic::default() }] {
            let mut h = MicHandoff::default();
            h.joined(0, on(), &gone());
            h.pad_take("S1", on());
            assert!(h.pad_release("S1", m, &stopped()).is_empty(), "{m:?}");
        }
        // (round 4 #2) push-to-talk with the key up blocks a user's want too: it waits for key-down
        let mut h = MicHandoff::default();
        h.joined(0, on(), &gone());
        h.pad_take("S1", on());
        let up = Mic { muted: true, ptt: true, ..Mic::default() };
        assert!(!h.user_unmute(up, &stopped(), true).contains(&Act::Unmute));
        assert!(h.tick(100, up, &stopped()).is_empty());
        assert_eq!(h.tick(200, muted(), &stopped()), vec![Act::Unmute, Act::Status(BACK_ON)], "key-down");
    }

    #[test]
    fn only_the_holding_session_releases_and_the_auto_rule_is_a_joining_rule() {
        let mut h = MicHandoff::default();
        h.joined(0, on(), &gone());
        h.pad_take("S1", on());
        assert!(h.pad_release("S2", muted(), &stopped()).is_empty());
        assert!(h.held());
        let mut h = MicHandoff::default();
        h.joined(0, on(), &gone());
        assert!(h.tick(AUTO_WINDOW_MS + 1, on(), &live()).is_empty(), "after 15 s a phone mic is not auto-taken");
        let mut h = MicHandoff::default();
        h.joined(0, on(), &gone());
        assert_eq!(h.tick(5_000, on(), &live()), vec![Act::Mute, Act::Status(PHONE_HAS_IT)], "within 15 s it is (continuous)");
    }
}
