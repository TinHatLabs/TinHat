//! share_carry.rs — D02.2 (12.1.16): a screen share survives an auto-rejoin.
//!
//! A dropped call that heals itself (call_ctl's auto-rejoin: a fresh join to the SAME room) used to come back without
//! the share: `CallSession::close` NULLed the capture AND closed the cast (the portal session on Linux; the window /
//! monitor handle on Windows), so the only way back was to open the picker again. Now the app ARMS the carry for the
//! room just before that teardown; `close` hands the cast in instead of closing it (its pipeline is still NULLed:
//! nothing is captured or sent while the call is down); the rejoined call of the same room takes it and publishes it
//! again — the same source, no picker. Anything else (a Leave, a join of another room, no rejoin, or a carry older than
//! [`MAX_AGE`]) closes it. Tin Hat Web can't do this (a browser capture can't outlive its call without a new gesture):
//! it shows a "Share again" chip instead (docs/WEB-EXCEPTIONS.md).
//!
//! fix1216 (review 5-1 / 5-6):
//! - EPOCH: a Leave (or sign-out) cancels every resume at once, synchronously on the Qt thread ([`cancel_resume`]): an
//!   arm, a carried cast and a resume already on its way all belong to the epoch they started in, and a resume checks
//!   [`resume_still_wanted`] after it gets the call (and the publish's own capture lease is ended by the Leave too), so
//!   no capture starts in a call that was left.
//! - ORPHAN: a share RESTART (resize / Custom quality) whose re-publish failed because the call dropped hands its cast
//!   here ([`keep_after_lost`]) instead of closing it; the drop's arm adopts it.
//! - WANTED: the arm records that a share was live, cast or not; the rejoin then always ends in the share back or the
//!   "Share again" notice (`Resume::Lost`), never in silence.
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// How long a carried share may wait for its rejoin (the one auto-rejoin attempt, 5-7, and its pause fit well inside).
pub const MAX_AGE: Duration = Duration::from_secs(90);

/// What the rejoined call does about the share that was live at the drop.
#[derive(Debug, PartialEq)]
pub enum Resume<T> {
    /// publish it again (same source); the epoch it belongs to
    Back(T, u64),
    /// a share was live but its source is gone: show "Share again"
    Lost,
    /// no share was live (or a Leave / another room cancelled it): nothing to do or say
    Nothing,
}

/// The carry's rules, generic over what is carried (a `ScreenCast` in the app; plain values in the tests). Every entry
/// carries the epoch it was made in; a later epoch (a Leave) voids it.
#[derive(Debug)]
pub struct Carry<T> {
    armed: Option<(String, u64)>,
    /// (room — "" = an orphan from a failed restart, not adopted yet —, when, the cast, epoch)
    held: Option<(String, Instant, T, u64)>,
    /// a share was live at the drop of this room (epoch)
    wanted: Option<(String, u64)>,
}

impl<T> Default for Carry<T> {
    fn default() -> Self {
        Self { armed: None, held: None, wanted: None }
    }
}

impl<T> Carry<T> {
    /// The next close of this room's call keeps its share; the rejoin owes the share or "Share again". An orphan of a
    /// failed restart is adopted for this room. Returns anything to close (an older carry, or one of a voided epoch).
    pub fn arm(&mut self, room: &str, epoch: u64, current: u64) -> Option<T> {
        if epoch != current {
            // a Leave came first: arm nothing, keep nothing
            self.armed = None;
            self.wanted = None;
            return self.held.take().map(|h| h.2);
        }
        self.armed = Some((room.to_string(), epoch));
        self.wanted = Some((room.to_string(), epoch));
        match self.held.take() {
            Some((r, at, t, e)) if r.is_empty() && e == epoch => {
                self.held = Some((room.to_string(), at, t, epoch));
                None
            }
            Some(h) => Some(h.2),
            None => None,
        }
    }
    /// The call is closing with `cast` (or no share): `Some` = close it now; `None` = carried for the armed room (the
    /// app arms right before it starts that call's teardown; one call at a time). Always disarms.
    pub fn closing(&mut self, cast: Option<T>, now: Instant) -> Option<T> {
        let armed = self.armed.take();
        let cast = cast?;
        match armed {
            Some((room, e)) => self.held.replace((room, now, cast, e)).map(|h| h.2),
            None => Some(cast),
        }
    }
    /// A restart's re-publish failed because the call dropped: keep the cast for the drop's arm (armed already: held for
    /// that room). `epoch` = the one the restart started in: a Leave since then (a later epoch) gets the cast back, to
    /// close. Returns what to close.
    pub fn keep_after_lost(&mut self, cast: T, now: Instant, epoch: u64, current: u64) -> Option<T> {
        if epoch != current {
            return Some(cast);
        }
        let room = self.armed.as_ref().filter(|a| a.1 == current).map(|a| a.0.clone()).unwrap_or_default();
        self.held.replace((room, now, cast, current)).map(|h| h.2)
    }
    /// The rejoined call of `room` is up: the carried share for it (fresh, this epoch), "Lost" when a share was live but
    /// nothing came through, else Nothing. The second value is anything to close.
    pub fn take(&mut self, room: &str, now: Instant, current: u64) -> (Resume<T>, Option<T>) {
        self.armed = None;
        let wanted = self.wanted.take().is_some_and(|(r, e)| r == room && e == current);
        match self.held.take() {
            Some((r, at, t, e)) if r == room && e == current && now.duration_since(at) <= MAX_AGE => (Resume::Back(t, e), None),
            Some((_, _, t, _)) => (if wanted { Resume::Lost } else { Resume::Nothing }, Some(t)),
            None => (if wanted { Resume::Lost } else { Resume::Nothing }, None),
        }
    }
    /// No rejoin after all (Leave, gave up, another room): whatever is carried, to close.
    pub fn drop_all(&mut self) -> Option<T> {
        self.armed = None;
        self.wanted = None;
        self.held.take().map(|h| h.2)
    }
    /// The carry's own timer: a share carried longer than [`MAX_AGE`] (no rejoin came), to close.
    pub fn expire(&mut self, now: Instant) -> Option<T> {
        match &self.held {
            Some((_, at, _, _)) if now.duration_since(*at) >= MAX_AGE => self.held.take().map(|h| h.2),
            _ => None,
        }
    }
    pub fn is_armed(&self) -> bool {
        self.armed.is_some()
    }
    pub fn holds(&self) -> bool {
        self.held.is_some()
    }
    /// an orphan of a failed restart is waiting for the drop's arm
    pub fn has_orphan(&self) -> bool {
        self.held.as_ref().is_some_and(|h| h.0.is_empty())
    }
}

fn carry() -> &'static std::sync::Mutex<Carry<crate::ScreenCast>> {
    static C: std::sync::OnceLock<std::sync::Mutex<Carry<crate::ScreenCast>>> = std::sync::OnceLock::new();
    C.get_or_init(|| std::sync::Mutex::new(Carry::default()))
}

fn epoch_cell() -> &'static AtomicU64 {
    static E: AtomicU64 = AtomicU64::new(1);
    &E
}

/// The current resume epoch (the app reads it on the Qt thread when it decides to arm).
pub fn resume_epoch() -> u64 {
    epoch_cell().load(Ordering::SeqCst)
}

/// Leave / sign-out (the Qt thread, synchronously): every arm, carry and resume of earlier epochs is void from now on,
/// and what is carried is closed (on the app's runtime: `rt`).
pub fn cancel_resume(rt: &tokio::runtime::Handle) {
    epoch_cell().fetch_add(1, Ordering::SeqCst);
    let old = carry().lock().unwrap().drop_all();
    if let Some(c) = old {
        eprintln!("[call] carried screen-share closed (left the call)");
        rt.spawn(c.close());
    }
}

/// A resume of `epoch` may still go on (no Leave since).
pub fn resume_still_wanted(epoch: u64) -> bool {
    resume_epoch() == epoch
}

/// an orphan of a failed restart is waiting (the drop treats the call as sharing)
pub fn has_orphan() -> bool {
    carry().lock().unwrap().has_orphan()
}

fn watch_expiry() {
    // a rejoin that never comes (it failed, or parked on the Rejoin banner) must not keep the source open. The cast is
    // stored later than the arm (after the teardown's unpublish), so the watch runs until nothing is armed or carried
    tokio::spawn(async {
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            let stale = {
                let mut c = carry().lock().unwrap();
                if !c.is_armed() && !c.holds() {
                    return;
                }
                c.expire(Instant::now())
            };
            if let Some(c) = stale {
                eprintln!("[call] carried screen-share closed (no rejoin within {} s)", MAX_AGE.as_secs());
                c.close().await;
                return;
            }
        }
    });
}

/// call_ctl, at an auto-rejoin drop of a call that was sharing: keep this room's share through the teardown. `epoch` =
/// [`resume_epoch`] read when the drop was decided (a Leave since then voids it).
pub async fn arm_share_carry(room: &str, epoch: u64) {
    let old = carry().lock().unwrap().arm(room, epoch, resume_epoch());
    if let Some(c) = old {
        c.close().await;
    }
    watch_expiry();
}

/// `CallSession::close`: the share's cast, or `None` when it was carried.
pub(crate) fn share_closing(cast: Option<crate::ScreenCast>) -> Option<crate::ScreenCast> {
    let out = carry().lock().unwrap().closing(cast, Instant::now());
    if out.is_none() && carry().lock().unwrap().holds() {
        eprintln!("[call] screen-share kept for the rejoin (capture stopped; same source on the way back)");
    }
    out
}

/// 5-6: a share restart whose re-publish failed because the call dropped: its cast waits for the drop's arm.
pub(crate) async fn keep_after_lost(cast: crate::ScreenCast, epoch: u64) {
    let old = carry().lock().unwrap().keep_after_lost(cast, Instant::now(), epoch, resume_epoch());
    if old.is_some() && epoch != resume_epoch() {
        eprintln!("[call] screen-share restart lost the call after a Leave: its source is closed");
    } else {
        eprintln!("[call] screen-share restart lost the call: the source waits for the rejoin");
    }
    if let Some(c) = old {
        c.close().await;
    }
    watch_expiry();
}

/// The rejoined call of `room`: what to do about the share that was live at the drop (a stale or other-room cast is
/// closed here).
pub async fn take_carried_share(room: &str) -> Resume<crate::ScreenCast> {
    let (r, stale) = carry().lock().unwrap().take(room, Instant::now(), resume_epoch());
    if let Some(c) = stale {
        c.close().await;
    }
    r
}

/// No rejoin after all: close whatever is carried.
pub async fn drop_carried_share() {
    let old = carry().lock().unwrap().drop_all();
    if let Some(c) = old {
        eprintln!("[call] carried screen-share closed (no rejoin)");
        c.close().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const E: u64 = 5;

    #[test]
    fn an_armed_close_of_the_same_room_carries_the_share_and_the_rejoin_takes_it() {
        let mut c = Carry::default();
        let t0 = Instant::now();
        assert_eq!(c.arm("!a", E, E), None);
        assert_eq!(c.closing(Some(7), t0), None, "carried, not closed");
        assert!(!c.is_armed() && c.holds());
        assert_eq!(c.take("!a", t0 + Duration::from_secs(5), E), (Resume::Back(7, E), None));
        assert!(!c.holds());
    }

    #[test]
    fn an_unarmed_close_closes_the_share() {
        let mut c: Carry<i32> = Carry::default();
        assert_eq!(c.closing(Some(7), Instant::now()), Some(7));
        assert!(!c.holds());
    }

    #[test]
    fn a_close_without_a_share_disarms() {
        let mut c: Carry<i32> = Carry::default();
        c.arm("!a", E, E);
        assert_eq!(c.closing(None, Instant::now()), None);
        assert!(!c.is_armed(), "a later close (a Leave) must not keep anything");
        assert_eq!(c.closing(Some(9), Instant::now()), Some(9));
    }

    #[test]
    fn a_join_of_another_room_or_a_late_one_closes_the_carried_share() {
        let mut c = Carry::default();
        let t0 = Instant::now();
        c.arm("!a", E, E);
        c.closing(Some(7), t0);
        assert_eq!(c.take("!b", t0, E), (Resume::Nothing, Some(7)));
        c.arm("!a", E, E);
        c.closing(Some(8), t0);
        assert_eq!(c.take("!a", t0 + MAX_AGE + Duration::from_secs(1), E), (Resume::Lost, Some(8)), "too late: Share again");
    }

    #[test]
    fn the_timer_closes_a_share_no_rejoin_took() {
        let mut c = Carry::default();
        let t0 = Instant::now();
        c.arm("!a", E, E);
        c.closing(Some(7), t0);
        assert_eq!(c.expire(t0 + Duration::from_secs(10)), None, "still waiting for its rejoin");
        assert_eq!(c.expire(t0 + MAX_AGE), Some(7));
        assert!(!c.holds());
    }

    #[test]
    fn leave_drops_it_and_rearming_closes_an_older_one() {
        let mut c = Carry::default();
        let t0 = Instant::now();
        c.arm("!a", E, E);
        c.closing(Some(7), t0);
        assert_eq!(c.arm("!a", E, E), Some(7), "never two carried shares");
        c.closing(Some(8), t0);
        assert_eq!(c.drop_all(), Some(8));
        assert_eq!(c.take("!a", t0, E), (Resume::Nothing, None), "a Leave owes nothing");
    }

    // 5-1: a Leave (a later epoch) voids every arm, carry and resume made before it
    #[test]
    fn a_leave_voids_the_resume_wherever_it_is() {
        let t0 = Instant::now();
        // the Leave lands before the arm runs: nothing is armed or kept
        let mut c: Carry<i32> = Carry::default();
        assert_eq!(c.arm("!a", E, E + 1), None);
        assert!(!c.is_armed());
        assert_eq!(c.closing(Some(7), t0), Some(7), "the teardown closes the share");
        assert_eq!(c.take("!a", t0, E + 1), (Resume::Nothing, None));
        // the Leave lands after the carry: the rejoin of the old epoch gets nothing back, and closes the cast
        let mut c = Carry::default();
        c.arm("!a", E, E);
        c.closing(Some(7), t0);
        assert_eq!(c.take("!a", t0, E + 1), (Resume::Nothing, Some(7)));
    }

    #[test]
    fn the_resume_check_follows_the_epoch() {
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let e = resume_epoch();
        assert!(resume_still_wanted(e));
        cancel_resume(rt.handle());
        assert!(!resume_still_wanted(e), "a resume taken before a Leave must stand down");
        assert!(resume_still_wanted(resume_epoch()));
    }

    // 5-6: a restart that lost the call keeps its cast for the rejoin, whichever comes first, the arm or the failure
    #[test]
    fn a_restart_that_lost_the_call_hands_its_source_to_the_rejoin() {
        let t0 = Instant::now();
        // the restart fails first (an orphan), then the drop arms: adopted
        let mut c = Carry::default();
        assert_eq!(c.keep_after_lost(7, t0, E, E), None);
        assert!(c.has_orphan());
        assert_eq!(c.arm("!a", E, E), None, "the orphan is adopted, not closed");
        assert_eq!(c.closing(None, t0), None, "the teardown finds no share of its own");
        assert_eq!(c.take("!a", t0, E), (Resume::Back(7, E), None));
        // the drop arms first, then the restart fails: held for that room
        let mut c = Carry::default();
        c.arm("!a", E, E);
        assert_eq!(c.keep_after_lost(8, t0, E, E), None);
        assert!(!c.has_orphan());
        c.closing(None, t0);
        assert_eq!(c.take("!a", t0, E), (Resume::Back(8, E), None));
    }

    // review r1: a Leave during the restart's re-publish, then its NOT_CONNECTED failure: closed, never carried
    #[test]
    fn a_restart_failing_after_a_leave_is_closed_not_carried() {
        let t0 = Instant::now();
        let mut c = Carry::default();
        assert_eq!(c.keep_after_lost(7, t0, E, E + 1), Some(7), "the restart's epoch is over");
        assert!(!c.holds() && !c.has_orphan());
        assert_eq!(c.arm("!a", E + 1, E + 1), None);
        c.closing(None, t0);
        assert_eq!(c.take("!a", t0, E + 1), (Resume::Lost, None), "a later drop of another call keeps nothing of it");
    }

    #[test]
    fn a_share_live_at_the_drop_always_ends_in_back_or_share_again() {
        let t0 = Instant::now();
        // armed (a share was live) but its source never reached the carry: Lost = "Share again"
        let mut c: Carry<i32> = Carry::default();
        c.arm("!a", E, E);
        c.closing(None, t0);
        assert_eq!(c.take("!a", t0, E), (Resume::Lost, None));
        // no share at the drop: nothing to say
        let mut c: Carry<i32> = Carry::default();
        assert_eq!(c.take("!a", t0, E), (Resume::Nothing, None));
    }
}
