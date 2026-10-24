//! 10.5 — "can they hear me?": honest, per-direction call health.
//!
//! Everything here is MEASURED, never assumed. The audio review of 2026-09-21 found that the call
//! UI could show moving meters and "Looking good" while nobody heard the user, and the field logs
//! showed the 0.8.25 "mic hears nothing" chip blinking on every pause of a gated mic (373 times in
//! one field call — it judged the PROCESSED audio, which a noise gate or the echo canceller
//! turns into digital silence between words). The facts this module keeps, each with its owner and
//! its freshness, and the one question each answers:
//!
//! * **mic check** — is the capture device producing anything? Judged on the RAW capture, before
//!   noise suppression and the canceller: a device that has produced real signal in this call is
//!   alive for the rest of it (a gate closing between words is normal); one that has produced
//!   nothing but a flat line (digital zero, or one constant value — the Legion's dead DMIC sent a
//!   constant −1.0, which a plain level reads as LOUD) gets a neutral hint at 15 s and a red one
//!   after a minute unmuted. Plus the system's own mute / 0 % volume on the input (Linux, pactl).
//! * **talking while muted** — the capture keeps running while muted, so we can hear it.
//! * **not coming through** — you are unmuted and talking loudly enough for the SFU's speech
//!   detector (LiveKit default: −35 dBov), and the SFU never lists you as speaking; or nothing
//!   leaves the computer at all (the mic's bytes-sent stopped).
//! * **receipts** — every client tells the call, every 3 s, what it RECEIVES from each person
//!   (`tinhat.rx`), so "heard by 3 of 3" is reported by the listeners' apps, not guessed.
//! * **can't hear X** — the SFU says X is speaking, our decoder yields silence, or nothing arrives,
//!   or the key is missing, or our output for X is stalled.
//! * **echo** — X's audio carries OUR voice back at one fixed delay: X's speakers into X's mic.
//!   10 ms loudness envelopes, the method validated offline on 8 recorded game nights
//!   (share: claude/tinhat-hear-me/). X is told (`tinhat.echo`) and gets one pill with the fix.
//! * **local echo** — OUR mic carries what OUR speakers play: the call (known exactly, it is what
//!   we decode) or system audio (sniffed from the output's monitor). The pill that recommends echo
//!   cancellation before anyone has to complain.
//!
//! Threading: the hot paths (`note_sent` from the mic feeder, `note_raw` on the capture's
//! streaming thread, `note_recv` from each playback feeder, the sniffer's appsink) take one
//! uncontended mutex for a few hundred nanoseconds. The analyzer task copies what it needs out
//! under the lock and does the correlation work without it.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use gstreamer as gst;
use gstreamer::prelude::*;
use livekit::id::ParticipantIdentity;

/// Receive report: `{"v":1,"deaf":bool,"rx":{"<identity>":"<state>",…}}`, lossy, every 3 s.
pub const RX_TOPIC: &str = "tinhat.rx";
/// Echo report to the echoing person: `{"v":1,"on":bool,"lag":ms,"db":level}`, reliable.
pub const ECHO_TOPIC: &str = "tinhat.echo";

const RING: usize = 6000; // 60 s of 10 ms frames
const FLOOR_DB: f32 = -65.0; // envelope floor (the offline method's)
const WIN: usize = 1500; // 15 s analysis window
const ECHO_LMAX: usize = 150; // 0–1500 ms: our send → SFU → their speakers → their mic → back
const LOCAL_LMAX: usize = 50; // 0–500 ms: our speakers → our mic (output + capture latency)
/// 0–300 ms the OTHER way: our mic hearing a sound before our speakers play it (a talker in our room).
const LOCAL_LEAD: usize = 30;
/// AC RMS (full scale 1.0) at or above which a raw capture buffer is REAL signal. A working
/// analog path never sits below ≈ −100 dBFS (16-bit: ±1 LSB ≈ −90 dBFS); a dead, hardware-muted
/// or gated source is exactly flat.
const ALIVE_AC: f32 = 3.0e-6;
/// Non-flat buffers before a device counts as alive (a start-up click is not a microphone).
const ALIVE_BUFFERS: u32 = 20;
const HINT_AFTER: Duration = Duration::from_secs(15);
const DEAD_AFTER_UNMUTED: Duration = Duration::from_secs(60);
/// A frame this loud is well inside the SFU's speech detector (LiveKit default active level
/// −35 dBov, 40th percentile of the packets in 400 ms) — only such bursts may say "not coming
/// through"; quieter ones say "your mic is very quiet".
const TX_LOUD_DB: f32 = -30.0;
/// An "echo" faster than this is not a loudspeaker: our voice cannot travel to them, be played and
/// come back that quickly (on the rig a real speaker echo sits at 220–310 ms; the SFU round trip plus
/// their playout alone is ≥ 100 ms). It is their mic hearing US directly — two people in one room
/// (recorded calls show such a pair at 40 ms). Everyone else hears that person twice;
/// echo cancellation cannot remove it (it is not in their speakers), so the advice differs.
const ROOM_LAG_MS: u32 = 120;
const TX_QUIET_DB: f32 = -38.0;

fn t0() -> Instant {
    static T: OnceLock<Instant> = OnceLock::new();
    *T.get_or_init(Instant::now)
}
fn now_tick() -> u64 {
    (t0().elapsed().as_millis() / 10) as u64
}

/// Loudness of one 10 ms frame in dBFS (full scale = 32768), floored.
pub(crate) fn db_of_i16(frame: &[i16]) -> f32 {
    if frame.is_empty() {
        return FLOOR_DB;
    }
    let e: f64 = frame.iter().map(|&s| (s as f64 / 32768.0).powi(2)).sum::<f64>() / frame.len() as f64;
    ((10.0 * (e + 1e-10).log10()) as f32).max(FLOOR_DB)
}
fn db_of_f32(frame: &[f32]) -> f32 {
    if frame.is_empty() {
        return FLOOR_DB;
    }
    let e: f64 = frame.iter().map(|&s| (s as f64).powi(2)).sum::<f64>() / frame.len() as f64;
    ((10.0 * (e + 1e-10).log10()) as f32).max(FLOOR_DB)
}

// ── envelope ring ──────────────────────────────────────────────────────────────────────────

/// 60 s of 10 ms loudness, indexed by a clock tick. Frames are numbered sequentially while they
/// keep pace with the wall clock (a burst after a stall keeps its order instead of piling onto
/// one tick); a stream that drifts more than 50 ms re-anchors.
struct Env {
    db: Vec<f32>,
    at: Vec<u64>,
    flag: Vec<u8>, // bit0 = our mic was muted when this frame was fed
    next: u64,
}
impl Env {
    fn new() -> Self {
        Env { db: vec![FLOOR_DB; RING], at: vec![u64::MAX; RING], flag: vec![0; RING], next: 0 }
    }
    fn push(&mut self, db: f32, flag: u8) {
        let now = now_tick();
        let t = if self.next == 0 || self.next + 5 < now || self.next > now + 5 { now } else { self.next };
        let i = (t % RING as u64) as usize;
        self.db[i] = db;
        self.at[i] = t;
        self.flag[i] = flag;
        self.next = t + 1;
    }
    fn get(&self, t: u64) -> Option<(f32, u8)> {
        let i = (t % RING as u64) as usize;
        (self.at[i] == t).then(|| (self.db[i], self.flag[i]))
    }
    /// `len` frames from tick `from`: missing frames → `None`.
    fn slice(&self, from: u64, len: usize) -> Vec<Option<(f32, u8)>> {
        (0..len as u64).map(|k| self.get(from + k)).collect()
    }
    fn max_since(&self, ticks: u64) -> Option<f32> {
        let now = now_tick();
        let mut m: Option<f32> = None;
        for t in now.saturating_sub(ticks)..=now {
            if let Some((d, _)) = self.get(t) {
                m = Some(m.map_or(d, |x: f32| x.max(d)));
            }
        }
        m
    }
}

// ── the correlation core (pure; the offline method of claude/tinhat-hear-me/scripts) ────────

/// Is `a` a stretch of real talking (not silence, not a steady drone)? ≥ 15 % of frames above
/// −45 dBFS and ≥ 6 dB of variation.
pub(crate) fn active(a: &[f32]) -> bool {
    if a.is_empty() {
        return false;
    }
    let n = a.len() as f32;
    let loud = a.iter().filter(|&&x| x > -45.0).count() as f32 / n;
    let mean = a.iter().sum::<f32>() / n;
    let sd = (a.iter().map(|&x| (x - mean).powi(2)).sum::<f32>() / n).sqrt();
    loud >= 0.15 && sd >= 6.0
}

/// Normalised cross-correlation of `a` (len W) with `b` (len ≥ W + lmax) at lags 0..=lmax (b
/// later than a). A flat stretch of `b` (gated, absent) is no evidence either way → 0.
pub(crate) fn ncc_curve(a: &[f32], b: &[f32], lmax: usize) -> Vec<f32> {
    let w = a.len();
    let mut out = vec![0.0f32; lmax + 1];
    if w == 0 || b.len() < w + lmax {
        return out;
    }
    let am = a.iter().map(|&x| x as f64).sum::<f64>() / w as f64;
    let a0: Vec<f64> = a.iter().map(|&x| x as f64 - am).collect();
    let an = a0.iter().map(|x| x * x).sum::<f64>().sqrt();
    if an < 1e-9 {
        return out;
    }
    for (k, o) in out.iter_mut().enumerate() {
        let seg = &b[k..k + w];
        let bm = seg.iter().map(|&x| x as f64).sum::<f64>() / w as f64;
        let mut dot = 0.0f64;
        let mut bn = 0.0f64;
        for (i, &x) in seg.iter().enumerate() {
            let v = x as f64 - bm;
            dot += v * a0[i];
            bn += v * v;
        }
        let bn = bn.sqrt();
        *o = if bn < 1e-3 * (w as f64).sqrt() { 0.0 } else { (dot / (bn * an)) as f32 };
    }
    out
}

fn percentile(v: &[f32], p: f32) -> f32 {
    if v.is_empty() {
        return FLOOR_DB;
    }
    let mut s = v.to_vec();
    s.sort_by(|x, y| x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal));
    let i = ((p / 100.0) * (s.len() - 1) as f32).round() as usize;
    s[i.min(s.len() - 1)]
}
fn median(v: &[f32]) -> f32 {
    percentile(v, 50.0)
}

/// B's level relative to A on A's loud frames (A's top 20 %), B read at `lag`.
fn rel_db(a: &[f32], b: &[f32], lag: usize) -> f32 {
    let p80 = percentile(a, 80.0);
    let d: Vec<f32> = a.iter().enumerate().filter(|(_, &x)| x > p80).map(|(i, &x)| b[i + lag] - x).collect();
    if d.is_empty() {
        0.0
    } else {
        median(&d)
    }
}
/// B's absolute level on A's loud frames, B read at `lag`.
fn level_at(a: &[f32], b: &[f32], lag: usize) -> f32 {
    let p80 = percentile(a, 80.0);
    let d: Vec<f32> = a.iter().enumerate().filter(|(_, &x)| x > p80).map(|(i, _)| b[i + lag]).collect();
    if d.is_empty() {
        FLOOR_DB
    } else {
        median(&d)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Win {
    /// A is not talking in this window — nothing to learn.
    Idle,
    /// B carries nothing of A.
    Clean,
    /// B IS A (≤ 60 ms, within 4 dB, near-identical): the same stream twice, not a room.
    Dup { lag: usize, ncc: f32, rel: f32 },
    /// A's voice comes back in B, later and quieter.
    Echo { lag: usize, ncc: f32, rel: f32 },
}

/// One window of the receiver-side check: A = what WE sent, B = what we receive from one person.
pub(crate) fn classify_echo(a: &[f32], b: &[f32], lmax: usize) -> Win {
    if !active(a) || b.len() < a.len() + lmax {
        return Win::Idle;
    }
    let c = ncc_curve(a, b, lmax);
    let k0 = argmax(&c, 0);
    let r0 = rel_db(a, b, k0);
    if c[k0] >= 0.5 && k0 <= 6 && r0.abs() <= 4.0 {
        return Win::Dup { lag: k0, ncc: c[k0], rel: r0 };
    }
    let k = argmax(&c, 4); // acoustic echo through someone else's room needs ≥ 40 ms
    if c[k] >= 0.3 {
        let r = rel_db(a, b, k);
        if r <= -6.0 {
            return Win::Echo { lag: k, ncc: c[k], rel: r };
        }
    }
    Win::Clean
}

/// One window of the LOCAL check: `r` = what our speakers play (the call, or the output's monitor),
/// `m` = our own mic after processing (what the call gets), both from the same tick; the window is
/// `w` long. No "quieter" rule — loud speakers can put the echo at the reference's own level — but
/// it must be audible (≥ −60 dBFS). Speakers → mic makes `m` LATER than `r` (lags 0..=`lmax`). When
/// `m` matches `r` better EARLIER (up to `lead`) our mic heard it before our speakers played it: the
/// talker is in our room — their voice reaches the mic through the air first and through the call
/// later. That is no speaker echo (no canceller or volume removes it), so it is not reported here;
/// the talker's own client sees it and tells us (a "same room" report).
pub(crate) fn classify_local(r: &[f32], m: &[f32], w: usize, lmax: usize, lead: usize) -> Win {
    if r.len() < w + lead || m.len() < w + lmax || !active(&r[..w]) {
        return Win::Idle;
    }
    let c = ncc_curve(&r[..w], &m[..w + lmax], lmax);
    let k = argmax(&c, 0);
    if c[k] >= 0.35 && level_at(&r[..w], m, k) >= -60.0 {
        if lead > 0 {
            let e = ncc_curve(&m[..w], &r[..w + lead], lead);
            let j = argmax(&e, 1);
            if e[j] > c[k] + 0.05 {
                return Win::Clean;
            }
        }
        return Win::Echo { lag: k, ncc: c[k], rel: rel_db(&r[..w], m, k) };
    }
    Win::Clean
}

fn argmax(c: &[f32], from: usize) -> usize {
    let mut k = from.min(c.len().saturating_sub(1));
    for i in from..c.len() {
        if c[i] > c[k] {
            k = i;
        }
    }
    k
}

/// Evidence over time for one echo path: a physical path has ONE delay, so the verdict needs
/// many windows at the same lag; conversation (turn-taking) scatters.
#[derive(Default)]
pub(crate) struct EchoTrack {
    hits: VecDeque<(Instant, usize, f32, f32)>, // when, lag (10 ms), ncc, rel dB
    analysed: VecDeque<(Instant, bool)>,
    pub(crate) on: Option<(u32, f32)>, // (lag ms, level dB)
    sent_at: Option<Instant>,
}
impl EchoTrack {
    fn note(&mut self, now: Instant, w: Win) {
        match w {
            Win::Idle => {}
            Win::Echo { lag, ncc, rel } => {
                self.hits.push_back((now, lag, ncc, rel));
                self.analysed.push_back((now, true));
            }
            _ => self.analysed.push_back((now, false)),
        }
        while self.hits.front().map(|h| now.duration_since(h.0) > Duration::from_secs(180)).unwrap_or(false) {
            self.hits.pop_front();
        }
        while self.analysed.front().map(|h| now.duration_since(h.0) > Duration::from_secs(180)).unwrap_or(false) {
            self.analysed.pop_front();
        }
    }
    /// `(lag ms, level dB, windows)` when `need` windows within `span` sit at one lag (±`tol`),
    /// they are ≥ 60 % of all hits there, and the level is at least `min_db`.
    fn verdict(&self, now: Instant, span: Duration, need: usize, tol: usize, min_db: f32) -> Option<(u32, f32, usize)> {
        let recent: Vec<&(Instant, usize, f32, f32)> = self.hits.iter().filter(|h| now.duration_since(h.0) <= span).collect();
        if recent.len() < need {
            return None;
        }
        let mut best = (0usize, 0usize);
        for h in &recent {
            let n = recent.iter().filter(|o| o.1.abs_diff(h.1) <= tol).count();
            if n > best.1 {
                best = (h.1, n);
            }
        }
        if best.1 < need || (best.1 as f32) < 0.6 * recent.len() as f32 {
            return None;
        }
        let at: Vec<f32> = recent.iter().filter(|o| o.1.abs_diff(best.0) <= tol).map(|o| o.3).collect();
        let lvl = median(&at);
        (lvl >= min_db).then_some(((best.0 * 10) as u32, lvl, best.1))
    }
    /// Cleared: A talked in ≥ `need` windows within `span` and none of them echoed.
    fn clear(&self, now: Instant, span: Duration, need: usize) -> bool {
        let recent: Vec<&(Instant, bool)> = self.analysed.iter().filter(|h| now.duration_since(h.0) <= span).collect();
        recent.len() >= need && !recent.iter().any(|h| h.1)
    }
}

// ── voice activity (muted talk, not coming through) ──────────────────────────────────────────

/// The last two seconds of our processed mic, frame by frame, and our own speech level learned
/// from the moments the SFU confirms we are speaking (a fixed bar is wrong for half the group:
/// AGC off + a gated source speaks at −26 dBFS, AGC on at −12).
#[derive(Default)]
pub(crate) struct Vad {
    frames: VecDeque<(f32, bool)>, // (dB, muted)
    speech_ref: Option<f32>,
}
impl Vad {
    fn threshold(&self) -> f32 {
        match self.speech_ref {
            Some(r) => (r - 12.0).clamp(-48.0, -28.0),
            None => -38.0,
        }
    }
    fn push(&mut self, db: f32, muted: bool, confirmed_speaking: bool) {
        self.frames.push_back((db, muted));
        while self.frames.len() > 200 {
            self.frames.pop_front();
        }
        if !muted && confirmed_speaking && db > -50.0 {
            self.speech_ref = Some(match self.speech_ref {
                Some(r) => r * 0.98 + db * 0.02,
                None => db,
            });
        }
    }
    /// Frames in the last `n` above the talk threshold, counting only muted (`muted`=true) or
    /// only live frames.
    fn talk_count(&self, n: usize, muted: bool) -> usize {
        let thr = self.threshold();
        self.frames.iter().rev().take(n).filter(|(d, m)| *m == muted && *d > thr).count()
    }
    fn loud_count(&self, n: usize) -> usize {
        let thr = self.threshold().max(TX_LOUD_DB);
        self.frames.iter().rev().take(n).filter(|(d, m)| !*m && *d > thr).count()
    }
    fn talk_median(&self, n: usize) -> f32 {
        let thr = self.threshold();
        let v: Vec<f32> = self.frames.iter().rev().take(n).filter(|(d, m)| !*m && *d > thr).map(|x| x.0).collect();
        median(&v)
    }
}

/// Talking into a muted mic: ≥ 0.45 s of speech-level frames in the last second.
pub(crate) fn muted_talking(v: &Vad) -> bool {
    v.talk_count(100, true) >= 45
}

// ── mic check ────────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum MicCheck {
    /// Nothing to say (alive, or no capture).
    Ok,
    /// A fresh capture, still inside its first 15 s.
    Wait,
    /// Neutral: "No sound from your mic yet — say something to check".
    Hint,
    /// Red: a minute unmuted and still a flat line.
    Dead,
    /// Red: muted in the system's sound settings.
    OsMuted,
    /// Red: the system input volume is 0 %.
    OsVol0,
}
impl MicCheck {
    fn as_str(self) -> &'static str {
        match self {
            MicCheck::Ok => "ok",
            MicCheck::Wait => "wait",
            MicCheck::Hint => "hint",
            MicCheck::Dead => "dead",
            MicCheck::OsMuted => "osmuted",
            MicCheck::OsVol0 => "osvol0",
        }
    }
}

#[derive(Default)]
pub(crate) struct MicState {
    device: String, // mic target ("" = system default)
    cap_started: Option<Instant>,
    nonflat: u32,
    alive: HashSet<String>,
    unmuted_in_hint: Duration,
    last_step: Option<Instant>,
    pub(crate) os_muted: Option<bool>,
    pub(crate) os_vol0: Option<bool>,
    check: Option<MicCheck>,
}
impl MicState {
    fn committed(&mut self, device: &str, now: Instant) {
        self.device = device.to_string();
        self.cap_started = Some(now);
        self.nonflat = 0;
        self.unmuted_in_hint = Duration::ZERO;
    }
    /// One raw buffer's AC RMS. Returns true the moment the device becomes alive.
    fn raw(&mut self, ac: f32) -> bool {
        if ac >= ALIVE_AC {
            self.nonflat += 1;
            if self.nonflat == ALIVE_BUFFERS {
                return self.alive.insert(self.device.clone());
            }
        }
        false
    }
    fn step(&mut self, now: Instant, muted: bool) -> MicCheck {
        let dt = self.last_step.map(|t| now.duration_since(t)).unwrap_or(Duration::ZERO);
        self.last_step = Some(now);
        let c = if self.os_muted == Some(true) {
            MicCheck::OsMuted
        } else if self.os_vol0 == Some(true) {
            MicCheck::OsVol0
        } else if self.cap_started.is_none() || self.alive.contains(&self.device) {
            MicCheck::Ok
        } else if now.duration_since(self.cap_started.unwrap()) < HINT_AFTER {
            MicCheck::Wait
        } else {
            if !muted {
                self.unmuted_in_hint += dt;
            }
            if self.unmuted_in_hint >= DEAD_AFTER_UNMUTED {
                MicCheck::Dead
            } else {
                MicCheck::Hint
            }
        };
        self.check = Some(c);
        c
    }
}

// ── not coming through ───────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) enum TxState {
    #[default]
    Ok,
    /// Nothing leaves the computer, or the SFU never hears loud speech.
    NotSent,
    /// Speech too quiet for the SFU to count it.
    Quiet,
}

#[derive(Default)]
pub(crate) struct Tx {
    pub(crate) state: TxState,
    pub(crate) why: String,
    burst: Option<Burst>,
    unconfirmed: VecDeque<Instant>,
    quiet: VecDeque<Instant>,
    bytes: u64,
    bytes_moved_at: Option<Instant>,
}
struct Burst {
    start: Instant,
    last_talk: Instant,
    loud: bool,
    confirmed: bool,
    levels: Vec<f32>,
}
impl Tx {
    fn reset(&mut self) {
        self.burst = None;
        self.unconfirmed.clear();
        self.quiet.clear();
        self.state = TxState::Ok;
        self.why.clear();
    }
    /// One 250 ms step. `talk`/`loud`: ≥ 1 s of talk / 0.8 s of loud talk in the last 2 s;
    /// `level` = median talk level; `spoke_at` = the SFU's last "you are speaking".
    #[allow(clippy::too_many_arguments)]
    fn step(&mut self, now: Instant, eligible: bool, talk: bool, loud: bool, level: f32, spoke_at: Option<Instant>) -> TxState {
        if !eligible {
            self.reset();
            self.bytes_moved_at = None;
            return self.state;
        }
        let heard_since = |t: Instant| spoke_at.map(|s| s + Duration::from_millis(500) >= t).unwrap_or(false);
        if talk {
            let b = self.burst.get_or_insert(Burst {
                start: now.checked_sub(Duration::from_secs(2)).unwrap_or(now),
                last_talk: now,
                loud: false,
                confirmed: false,
                levels: Vec::new(),
            });
            b.last_talk = now;
            b.loud |= loud;
            b.levels.push(level);
            if heard_since(b.start) {
                b.confirmed = true;
            }
        } else if let Some(b) = self.burst.as_ref() {
            if now.duration_since(b.last_talk) > Duration::from_millis(1500) {
                let b = self.burst.take().unwrap();
                if b.confirmed || heard_since(b.start) {
                    self.unconfirmed.clear();
                    self.quiet.clear();
                } else if b.loud {
                    self.unconfirmed.push_back(now);
                } else if median(&b.levels) < TX_QUIET_DB {
                    self.quiet.push_back(now);
                }
            }
        }
        while self.unconfirmed.front().map(|t| now.duration_since(*t) > Duration::from_secs(60)).unwrap_or(false) {
            self.unconfirmed.pop_front();
        }
        while self.quiet.front().map(|t| now.duration_since(*t) > Duration::from_secs(120)).unwrap_or(false) {
            self.quiet.pop_front();
        }
        let recently_heard = spoke_at.map(|s| now.duration_since(s) < Duration::from_secs(5)).unwrap_or(false);
        let stalled = self.bytes_moved_at.map(|t| now.duration_since(t) > Duration::from_secs(8)).unwrap_or(false);
        let (st, why) = if stalled {
            (TxState::NotSent, "nothing has left this computer for 8 s".to_string())
        } else if recently_heard {
            // The SFU lists us as speaking: whatever older bursts said, we are coming through now.
            self.unconfirmed.clear();
            self.quiet.clear();
            (TxState::Ok, String::new())
        } else if self.unconfirmed.len() >= 2 {
            (TxState::NotSent, "you were talking, but the call server didn't receive your voice".to_string())
        } else if self.quiet.len() >= 3 {
            (TxState::Quiet, "your voice reaches the call too quietly to count as speech".to_string())
        } else if self.state == TxState::NotSent && self.unconfirmed.is_empty() {
            (TxState::Ok, String::new())
        } else {
            (self.state, self.why.clone())
        };
        self.state = st;
        self.why = why;
        st
    }
    fn bytes(&mut self, total: u64, now: Instant) {
        // ANY change is progress: a re-published mic (an AGC/NS toggle) is a new track whose counter
        // restarts at 0 — "only increases count" read that as 8 s of nothing leaving the computer.
        if total != self.bytes || self.bytes_moved_at.is_none() {
            self.bytes_moved_at = Some(now);
        }
        self.bytes = total;
    }
}

// ── receive side ─────────────────────────────────────────────────────────────────────────────

struct Recv {
    identity: String,
    category: String,
    env: Env,
    added: Instant,
    last_frame: Option<Instant>,
    frames: u64,
    muted: bool,
    stalled: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct PeerReport {
    at: Instant,
    pub(crate) deaf: bool,
    pub(crate) away: bool,
    pub(crate) rx: HashMap<String, String>,
}

// ── the session ──────────────────────────────────────────────────────────────────────────────

#[derive(Default)]
struct Hear {
    on: bool,
    me: String,
    mic_muted: bool,
    unmuted_at: Option<Instant>, // peers' reports lag an unmute by up to one 3 s period
    reconnecting: bool,
    ec_engaged: bool,
    sent: Option<Env>,
    vad: Vad,
    muted_talk_until: Option<Instant>,
    muted_talk_logged: bool,
    last_loud_rx: Option<Instant>,
    last_loud_sys: Option<Instant>, // system audio (the sniffed output) was loud
    mic: MicState,
    out_muted: Option<bool>,
    out_vol0: Option<bool>,
    out_label: String,
    mic_label: String,
    tx: Tx,
    recv: HashMap<String, Recv>, // by track sid
    present: HashSet<String>,
    speaking_now: HashSet<String>,
    self_spoke_at: Option<Instant>,
    spk_hist: HashMap<String, VecDeque<bool>>,
    reports: HashMap<String, PeerReport>,
    echo: HashMap<String, EchoTrack>,             // they send MY voice back
    echo_me: HashMap<String, (Instant, u32, f32, bool)>, // they hear themselves through ME (lag, dB, same room)
    local_call: EchoTrack,
    local_sys: EchoTrack,
    sys: Option<Env>,
    sniff_out: Option<String>,
    sniff_try: Option<(String, u32, Instant)>, // a monitor that would not start: (output, failures, next try)
    dup_logged: HashSet<(String, String)>,
    ec_fix: (String, String, String), // (kind, output node, label)
    heal_requests: HashSet<String>,   // identities the user asked to repair
    last_states: HashMap<String, String>,
    last_heard: String,  // the "heard by" line as last logged
    last_fix: String,    // the echo fix as last logged
    snapshot: String,
}

static ON: AtomicBool = AtomicBool::new(false);
/// Bumped by every session start/end: an analyzer whose session is gone (a join abandoned after
/// connect, a CallSession dropped without close()) exits instead of running on beside the next one.
static SESSION: AtomicU64 = AtomicU64::new(0);

fn hear() -> &'static Mutex<Hear> {
    static H: OnceLock<Mutex<Hear>> = OnceLock::new();
    H.get_or_init(|| Mutex::new(Hear::default()))
}

/// Start a call's bookkeeping (not for the recorder — it has no mic and plays nothing).
pub(crate) fn session_start(me: &str) {
    let mut h = hear().lock().unwrap();
    // Evidence is per call (Astra 2026-09-22 #11): a mic that worked last call may be hardware-muted
    // now. (A gated mic therefore gets one neutral hint per call until its first word — by design.)
    *h = Hear::default();
    h.on = true;
    h.me = me.to_string();
    h.sent = Some(Env::new());
    h.snapshot = "{\"v\":1,\"on\":true}".to_string();
    SESSION.fetch_add(1, Ordering::SeqCst);
    ON.store(true, Ordering::Relaxed);
    eprintln!("[hear] session started as {me}");
}

/// End it: the UI shows nothing outside a call.
pub(crate) fn session_end() {
    SESSION.fetch_add(1, Ordering::SeqCst);
    ON.store(false, Ordering::Relaxed);
    sniffer_stop();
    let mut h = hear().lock().unwrap();
    *h = Hear::default();
}

/// Is the call's link to the SFU down right now? (The mic healer does not try to publish into it.)
pub fn reconnecting() -> bool {
    hear().lock().map(|h| h.reconnecting).unwrap_or(false)
}
/// A call session is live (the Settings preview meter must not drive the call's meter).
pub(crate) fn session_active() -> bool {
    ON.load(Ordering::Relaxed)
}

/// The UI's view (see the module doc), `{}` outside a call. Cached; rebuilt 4×/s.
pub fn state_json() -> String {
    if !ON.load(Ordering::Relaxed) {
        return "{}".to_string();
    }
    hear().lock().unwrap().snapshot.clone()
}

/// The mic check's red states — what the app's old `micSilent` property now means.
pub(crate) fn mic_needs_attention() -> bool {
    if !ON.load(Ordering::Relaxed) {
        return false;
    }
    matches!(hear().lock().unwrap().mic.check, Some(MicCheck::Dead | MicCheck::OsMuted | MicCheck::OsVol0))
}

pub(crate) fn set_mic_muted(muted: bool) {
    if let Ok(mut h) = hear().lock() {
        if h.mic_muted != muted {
            h.mic_muted = muted;
            if !muted {
                h.muted_talk_until = None;
                h.unmuted_at = Some(Instant::now());
            }
        }
    }
}
/// The user's current mute intent as the streamer last applied it (ensure_mic republishes with it).
pub fn mic_muted_now() -> bool {
    hear().lock().map(|h| h.mic_muted).unwrap_or(false)
}
pub(crate) fn set_reconnecting(on: bool) {
    hear().lock().unwrap().reconnecting = on;
}

/// A new capture is live (after PLAYING — the Train A commit point). `device` = the mic target.
pub(crate) fn capture_committed(device: &str) {
    if !ON.load(Ordering::Relaxed) {
        return;
    }
    let mut h = hear().lock().unwrap();
    h.mic.committed(device, Instant::now());
}

/// One RAW capture buffer (mono F32 48 k, before any processing) — the capture's streaming thread.
pub(crate) fn note_raw(buf: &[f32]) {
    if !ON.load(Ordering::Relaxed) || buf.is_empty() {
        return;
    }
    let n = buf.len() as f64;
    let mean = buf.iter().map(|&x| x as f64).sum::<f64>() / n;
    let var = buf.iter().map(|&x| (x as f64 - mean).powi(2)).sum::<f64>() / n;
    let ac = var.sqrt() as f32;
    let mut h = hear().lock().unwrap();
    if h.mic.raw(ac) {
        let d = if h.mic.device.is_empty() { "the system default input".to_string() } else { h.mic.device.clone() };
        eprintln!("[hear] mic alive: real signal from {d}");
    }
}

/// One frame the call is fed (after all processing) — the mic feeder, every 10 ms.
pub(crate) fn note_sent(frame: &[i16]) {
    if !ON.load(Ordering::Relaxed) {
        return;
    }
    let db = db_of_i16(frame);
    let now = Instant::now();
    let mut h = hear().lock().unwrap();
    let muted = h.mic_muted;
    if let Some(e) = h.sent.as_mut() {
        e.push(db, muted as u8);
    }
    let confirmed = h.self_spoke_at.map(|t| now.duration_since(t) < Duration::from_millis(600)).unwrap_or(false);
    // Others' voices — or a game — through the speakers must not read as us talking: without the
    // canceller judge only while the call is quiet, and never while the sniffed output (an output the
    // canceller does not cover, or any output when it is off) plays loud. Such frames are not even
    // EVIDENCE: counted, they tripped the warning in the silence right after the loud part (#7).
    let recent = |t: Option<Instant>| t.map(|t| now.duration_since(t) < Duration::from_millis(400)).unwrap_or(false);
    let echo_risk = (!h.ec_engaged && recent(h.last_loud_rx)) || recent(h.last_loud_sys);
    h.vad.push(if muted && echo_risk { FLOOR_DB } else { db }, muted, confirmed);
    if muted {
        if !echo_risk && muted_talking(&h.vad) {
            h.muted_talk_until = Some(now + Duration::from_millis(1500));
            if !h.muted_talk_logged {
                h.muted_talk_logged = true;
                eprintln!("[hear] muted talk: talking into a muted mic (threshold {:.0} dBFS)", h.vad.threshold());
            }
        }
    }
}

/// A remote audio track is subscribed.
pub(crate) fn recv_track_added(identity: &str, category: &str, sid: &str) {
    if !ON.load(Ordering::Relaxed) {
        return;
    }
    let mut h = hear().lock().unwrap();
    h.present.insert(identity.to_string());
    h.recv.insert(
        sid.to_string(),
        Recv {
            identity: identity.to_string(),
            category: category.to_string(),
            env: Env::new(),
            added: Instant::now(),
            last_frame: None,
            frames: 0,
            muted: false,
            stalled: false,
        },
    );
}
pub(crate) fn recv_track_removed(sid: &str) {
    if let Ok(mut h) = hear().lock() {
        h.recv.remove(sid);
    }
}
pub(crate) fn participant_present(identity: &str) {
    if ON.load(Ordering::Relaxed) {
        hear().lock().unwrap().present.insert(identity.to_string());
    }
}
pub(crate) fn participant_left(identity: &str) {
    if let Ok(mut h) = hear().lock() {
        h.present.remove(identity);
        h.recv.retain(|_, r| r.identity != identity);
        h.reports.remove(identity);
        h.echo.remove(identity);
        h.echo_me.remove(identity);
        h.spk_hist.remove(identity);
    }
}
pub(crate) fn note_track_muted(identity: &str, category: &str, muted: bool) {
    if let Ok(mut h) = hear().lock() {
        for r in h.recv.values_mut() {
            if r.identity == identity && r.category == category {
                r.muted = muted;
            }
        }
    }
}
/// The playback healer's view of one track's output.
pub(crate) fn note_play_stall(sid: &str, stalled: bool) {
    if let Ok(mut h) = hear().lock() {
        if let Some(r) = h.recv.get_mut(sid) {
            r.stalled = stalled;
        }
    }
}

/// One decoded frame from a remote track — each playback feeder, every 10 ms.
pub(crate) fn note_recv(sid: &str, frame: &[i16]) {
    if !ON.load(Ordering::Relaxed) {
        return;
    }
    let db = db_of_i16(frame);
    let now = Instant::now();
    let mut h = hear().lock().unwrap();
    if db > -40.0 {
        h.last_loud_rx = Some(now);
    }
    if let Some(r) = h.recv.get_mut(sid) {
        r.env.push(db, 0);
        r.last_frame = Some(now);
        r.frames += 1;
    }
}

/// The SFU's active-speaker list changed.
pub(crate) fn note_speakers(ids: &[String]) {
    if !ON.load(Ordering::Relaxed) {
        return;
    }
    let mut h = hear().lock().unwrap();
    h.speaking_now = ids.iter().cloned().collect();
    if ids.iter().any(|i| *i == h.me) {
        h.self_spoke_at = Some(Instant::now());
    }
}

/// The mic's cumulative bytes-sent (the stats task, every 3 s).
pub(crate) fn note_mic_bytes(total: u64) {
    if ON.load(Ordering::Relaxed) {
        hear().lock().unwrap().tx.bytes(total, Instant::now());
    }
}

/// The user asked to repair what they hear from `identity` (the tile's Fix).
pub fn request_heal(identity: &str) {
    if let Ok(mut h) = hear().lock() {
        h.heal_requests.insert(identity.to_string());
    }
}
/// The playback feeder asks: should I rebuild my output now? (consumes the request)
pub(crate) fn take_heal_request(identity: &str) -> bool {
    hear().lock().map(|mut h| h.heal_requests.remove(identity)).unwrap_or(false)
}

// ── receipts ─────────────────────────────────────────────────────────────────────────────────

/// The Memories recorder's account (`@user:server`) as the user's server announces it (the app's server_offer.rs
/// sets it); None = this server has no recorder, and nobody is filtered.
static RECORDER: Mutex<Option<String>> = Mutex::new(None);

pub fn set_recorder(mxid: Option<String>) {
    *RECORDER.lock().unwrap_or_else(|e| e.into_inner()) = mxid.filter(|m| !m.is_empty());
}

/// Is this the Memories recorder? It records, it does not listen, it sends no receipts.
pub(crate) fn is_recorder(identity: &str) -> bool {
    let r = RECORDER.lock().unwrap_or_else(|e| e.into_inner()).clone();
    is_recorder_with(identity, r.as_deref())
}

/// `identity` = "@u:server:DEVICE" (a LiveKit identity) or "@u:server"; the recorder is named by account (the web's
/// offer.ts isRecorderId: the account itself, or the account followed by ":<device>").
pub(crate) fn is_recorder_with(identity: &str, recorder: Option<&str>) -> bool {
    let Some(r) = recorder.filter(|r| !r.is_empty()) else { return false };
    identity == r || identity.strip_prefix(r).is_some_and(|rest| rest.starts_with(':'))
}

pub(crate) fn build_rx_report(deaf: bool, away: bool, rx: &[(String, String)]) -> String {
    build_rx_report_with(deaf, away, rx, "")
}

/// `extra` = a JSON fragment appended after `rx` (11.3: `,"cap":["sr"],"sr":{…}` from sameroom::rx_extra).
pub(crate) fn build_rx_report_with(deaf: bool, away: bool, rx: &[(String, String)], extra: &str) -> String {
    let mut s = format!("{{\"v\":1,\"deaf\":{deaf},\"away\":{away},\"rx\":{{");
    for (i, (id, st)) in rx.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push('"');
        s.push_str(&json_esc(id));
        s.push_str("\":\"");
        s.push_str(&json_esc(st));
        s.push('"');
    }
    s.push('}');
    s.push_str(extra);
    s.push('}');
    s
}

/// `{"v":1,"deaf":…,"rx":{…}}` → (deaf, rx). Hand-parsed like the other `tinhat.*` packets (the
/// streamer has no serde): identities and states never contain quotes or backslashes.
pub(crate) fn parse_rx_report(text: &str) -> Option<(bool, bool, HashMap<String, String>)> {
    let t: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    if !t.starts_with('{') {
        return None;
    }
    let deaf = t.contains("\"deaf\":true");
    let away = t.contains("\"away\":true");
    let body = t.split("\"rx\":{").nth(1)?;
    let body = &body[..body.find('}')?];
    let mut rx = HashMap::new();
    for pair in body.split("\",\"") {
        let pair = pair.trim_matches('"');
        if let Some((k, v)) = pair.split_once("\":\"") {
            if !k.is_empty() {
                rx.insert(k.to_string(), v.to_string());
            }
        }
    }
    Some((deaf, away, rx))
}

pub(crate) fn handle_rx(who: &str, text: &str) {
    if !ON.load(Ordering::Relaxed) || who.is_empty() || is_recorder(who) {
        return;
    }
    if let Some((deaf, away, rx)) = parse_rx_report(text) {
        let mut h = hear().lock().unwrap();
        h.present.insert(who.to_string());
        h.reports.insert(who.to_string(), PeerReport { at: Instant::now(), deaf, away, rx });
    }
    crate::sameroom::note_peer_rx(who, text); // 11.3: capability + link state ride the receipt
}

pub(crate) fn handle_echo(who: &str, text: &str) {
    if !ON.load(Ordering::Relaxed) || who.is_empty() {
        return;
    }
    let t: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    let num = |key: &str| -> Option<f32> {
        let rest = t.split(&format!("\"{key}\":")).nth(1)?;
        let end = rest.find([',', '}']).unwrap_or(rest.len());
        rest[..end].parse::<f32>().ok()
    };
    let mut h = hear().lock().unwrap();
    if t.contains("\"on\":true") {
        // Validated (#12): `"db":inf` parsed fine and printed as a bare `inf`, which made the whole UI
        // snapshot invalid JSON — one packet hid every indicator for everyone who received it.
        let lag = num("lag").filter(|v| v.is_finite()).unwrap_or(0.0).clamp(0.0, 5000.0) as u32;
        let db = num("db").filter(|v| v.is_finite()).unwrap_or(-30.0).clamp(-120.0, 20.0);
        let room = t.contains("\"room\":true");
        if !h.echo_me.contains_key(who) {
            if room {
                eprintln!("[hear] echo report: our mic picks up {who} in the room ({lag} ms, {db:.0} dB) — others hear them twice");
            } else {
                eprintln!("[hear] echo report: {who} hears themselves through our mic ({lag} ms later, {db:.0} dB)");
            }
        }
        h.echo_me.insert(who.to_string(), (Instant::now(), lag, db, room));
    } else if h.echo_me.remove(who).is_some() {
        eprintln!("[hear] echo report: {who} no longer hears an echo from us");
    }
}

/// A float as JSON (never `inf`/`NaN`, which would make the snapshot invalid).
fn jnum(x: f32) -> String {
    if x.is_finite() {
        format!("{x:.0}")
    } else {
        "0".to_string()
    }
}

fn json_esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o
}

// ── the system-audio sniffer (local echo from games/music on an output) ──────────────────────

fn sniffer_cell() -> &'static Mutex<Option<gst::Pipeline>> {
    static S: OnceLock<Mutex<Option<gst::Pipeline>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(None))
}

fn sniffer_stop() {
    sniffer_stop_with(false)
}
/// `wait`: stop the old monitor before returning (bounded) — a start runs on a blocking thread and
/// replaces the old one in order rather than beside a teardown.
fn sniffer_stop_with(wait: bool) {
    let old = sniffer_cell().lock().unwrap().take();
    if let Some(p) = old {
        if wait {
            crate::call::set_state_bounded(p.upcast_ref(), gst::State::Null, 1500, "hear sniffer");
        } else {
            std::thread::spawn(move || {
                crate::call::set_state_bounded(p.upcast_ref(), gst::State::Null, 1500, "hear sniffer");
            });
        }
    }
    if let Ok(mut h) = hear().lock() {
        h.sys = None;
        h.sniff_out = None;
        // Its evidence and verdict go with it (#9): echo cancellation switched on / the output added
        // stops the sniffer, and nothing would ever clear a verdict that is no longer measured.
        h.local_sys = EchoTrack::default();
    }
}

/// Listen to what `out` plays (its monitor; "" = the default output) at 8 kHz, as loudness only.
fn sniffer_start(out: &str) -> bool {
    sniffer_stop_with(true);
    // One session's sniffer (#4): a start still running when the call ends must not install a
    // capture afterwards, and its callbacks must not feed the next call.
    let my_session = SESSION.load(Ordering::SeqCst);
    if !ON.load(Ordering::Relaxed) {
        return false;
    }
    // Linux: ask PipeWire for the graph's own rate and resample here. Left alone, the 8 kHz caps below
    // reach PipeWire as an 8 kHz mono stream — on the rig one such stream made pw-play a 100 ms-quantum
    // driver; a sniffer must never shape the graph it listens to.
    let native = if cfg!(windows) { "" } else { " ! audio/x-raw,rate=48000" };
    let desc = format!(
        "{}{native} ! queue max-size-time=200000000 max-size-buffers=0 max-size-bytes=0 leaky=downstream ! \
         audioconvert ! audioresample ! audio/x-raw,format=F32LE,rate=8000,channels=1,layout=interleaved ! \
         appsink name=hearsnk sync=false max-buffers=20 drop=true",
        crate::call::monitor_head(out, "hearsniff")
    );
    let pipeline = match gst::parse::launch(&desc).ok().and_then(|p| p.downcast::<gst::Pipeline>().ok()) {
        Some(p) => p,
        None => {
            eprintln!("[hear] system-audio sniffer: could not build a monitor of {out:?}");
            return false;
        }
    };
    let Some(sink) = pipeline.by_name("hearsnk").and_then(|e| e.downcast::<gstreamer_app::AppSink>().ok()) else {
        return false;
    };
    let mut rest: Vec<f32> = Vec::with_capacity(160);
    sink.set_callbacks(
        gstreamer_app::AppSinkCallbacks::builder()
            .new_sample(move |s| {
                let sample = s.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                let buf = sample.buffer().ok_or(gst::FlowError::Error)?;
                let map = buf.map_readable().map_err(|_| gst::FlowError::Error)?;
                rest.extend(map.as_slice().chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])));
                let mut dbs = Vec::new();
                while rest.len() >= 80 {
                    dbs.push(db_of_f32(&rest[..80]));
                    rest.drain(..80);
                }
                if SESSION.load(Ordering::SeqCst) != my_session {
                    return Ok(gst::FlowSuccess::Ok);
                }
                if let Ok(mut h) = hear().lock() {
                    if dbs.iter().any(|&d| d > -40.0) {
                        h.last_loud_sys = Some(Instant::now());
                    }
                    if let Some(e) = h.sys.as_mut() {
                        for d in dbs {
                            e.push(d, 0);
                        }
                    }
                }
                Ok(gst::FlowSuccess::Ok)
            })
            .build(),
    );
    {
        let mut h = hear().lock().unwrap();
        h.sys = Some(Env::new());
        h.sniff_out = Some(out.to_string());
    }
    if !crate::call::set_state_bounded(pipeline.upcast_ref(), gst::State::Playing, 4000, "hear sniffer start") {
        crate::call::set_state_bounded(pipeline.upcast_ref(), gst::State::Null, 1500, "hear sniffer");
        let mut h = hear().lock().unwrap();
        h.sys = None;
        h.sniff_out = None;
        eprintln!("[hear] system-audio sniffer: the monitor of {out:?} would not start");
        return false;
    }
    if !ON.load(Ordering::Relaxed) || SESSION.load(Ordering::SeqCst) != my_session {
        crate::call::set_state_bounded(pipeline.upcast_ref(), gst::State::Null, 1500, "hear sniffer (call ended)");
        return false;
    }
    eprintln!(
        "[hear] system-audio sniffer listening to {}",
        if out.is_empty() { "the default output" } else { out }
    );
    *sniffer_cell().lock().unwrap() = Some(pipeline);
    true
}

// ── the system's own mute / volume (Linux) ───────────────────────────────────────────────────

/// `pactl <args>` with a 2 s bound (a wedged sound server must not stall the analyzer).
fn pactl(args: &[&str]) -> Option<String> {
    if cfg!(windows) {
        return None;
    }
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let out = std::process::Command::new("pactl")
            .args(&args)
            .env("LC_ALL", "C")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string());
        let _ = tx.send(out);
    });
    rx.recv_timeout(Duration::from_secs(2)).ok().flatten()
}

/// (muted, volume is 0 %) of a source ("source") or sink ("sink"); `None` = unknown.
pub(crate) fn parse_mute(text: &str) -> Option<bool> {
    let l = text.lines().find(|l| l.trim_start().starts_with("Mute:"))?;
    Some(l.contains("yes"))
}
pub(crate) fn parse_vol0(text: &str) -> Option<bool> {
    let pcts: Vec<u32> = text
        .split_whitespace()
        .filter_map(|w| w.strip_suffix('%'))
        .filter_map(|n| n.parse::<u32>().ok())
        .collect();
    if pcts.is_empty() {
        None
    } else {
        Some(pcts.iter().all(|&p| p == 0))
    }
}
fn os_state(kind: &str, node: &str) -> (Option<bool>, Option<bool>) {
    let target = if node.is_empty() {
        if kind == "source" { "@DEFAULT_SOURCE@" } else { "@DEFAULT_SINK@" }
    } else {
        node
    };
    let m = pactl(&[&format!("get-{kind}-mute"), target]).and_then(|t| parse_mute(&t));
    let v = pactl(&[&format!("get-{kind}-volume"), target]).and_then(|t| parse_vol0(&t));
    (m, v)
}

/// Unmute and (if at 0 %) raise the system input or output — the pills' one-click fix.
pub fn fix_system_level(output: bool) -> bool {
    if cfg!(windows) {
        return false;
    }
    let (kind, node) = if output {
        ("sink", crate::call::playback_target_node().unwrap_or_default())
    } else {
        ("source", crate::call::mic_target_node().unwrap_or_default())
    };
    let target = if node.is_empty() {
        if output { "@DEFAULT_SINK@".to_string() } else { "@DEFAULT_SOURCE@".to_string() }
    } else {
        node
    };
    let a = pactl(&[&format!("set-{kind}-mute"), &target, "0"]).is_some();
    let (_, vol0) = os_state(kind, if target.starts_with('@') { "" } else { &target });
    let b = if vol0 == Some(true) { pactl(&[&format!("set-{kind}-volume"), &target, "100%"]).is_some() } else { true };
    eprintln!("[hear] system {kind} {target}: unmuted{} (ok={})", if vol0 == Some(true) { " + volume 100 %" } else { "" }, a && b);
    if let Ok(mut h) = hear().lock() {
        if output {
            h.out_muted = Some(false);
            h.out_vol0 = Some(false);
        } else {
            h.mic.os_muted = Some(false);
            h.mic.os_vol0 = Some(false);
        }
    }
    a && b
}

// ── the analyzer ─────────────────────────────────────────────────────────────────────────────

/// The call's analyzer: 4 Hz state, receipts every 3 s, echo every 5 s. Aborted by close().
pub(crate) fn spawn_analyzer(lp: livekit::participant::LocalParticipant) -> tokio::task::JoinHandle<()> {
    let my_session = SESSION.load(Ordering::SeqCst);
    tokio::spawn(async move {
        let mut tick: u64 = 0;
        let rx_err_logged = std::sync::Arc::new(AtomicBool::new(false));
        let rx_in_flight = std::sync::Arc::new(AtomicBool::new(false));
        let mut iv = tokio::time::interval(Duration::from_millis(250));
        iv.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            iv.tick().await;
            if !ON.load(Ordering::Relaxed) || SESSION.load(Ordering::SeqCst) != my_session {
                break;
            }
            tick += 1;
            let now = Instant::now();
            // Every 5 s: the system's own mute/volume + the echo-cancellation facts (pactl, off-thread).
            if tick % 20 == 1 {
                let r = tokio::time::timeout(
                    Duration::from_secs(6),
                    tokio::task::spawn_blocking(|| {
                        let mic = crate::call::mic_target_node().unwrap_or_default();
                        let out = crate::call::playback_target_node().unwrap_or_default();
                        let (im, iv0) = os_state("source", &mic);
                        let (om, ov0) = os_state("sink", &out);
                        (im, iv0, om, ov0, crate::call::ec_facts())
                    }),
                )
                .await;
                if let Ok(Ok((im, iv0, om, ov0, facts))) = r {
                    let want = {
                        let mut h = hear().lock().unwrap();
                        for (was, new, what) in [(h.mic.os_muted, im, "input muted"), (h.mic.os_vol0, iv0, "input volume 0 %")] {
                            if was != new && new.is_some() && (was.is_some() || new == Some(true)) {
                                eprintln!("[hear] system {what}: {}", if new == Some(true) { "yes" } else { "no" });
                            }
                        }
                        for (was, new, what) in [(h.out_muted, om, "output muted"), (h.out_vol0, ov0, "output volume 0 %")] {
                            if was != new && new.is_some() && (was.is_some() || new == Some(true)) {
                                eprintln!("[hear] system {what}: {}", if new == Some(true) { "yes" } else { "no" });
                            }
                        }
                        h.mic.os_muted = im;
                        h.mic.os_vol0 = iv0;
                        h.out_muted = om;
                        h.out_vol0 = ov0;
                        h.ec_engaged = facts.engaged;
                        h.mic_label = facts.mic_label.clone();
                        h.out_label = facts.out_label.clone();
                        h.ec_fix = ec_fix(&facts, &h);
                        sniff_wanted(&facts)
                    };
                    let cur = hear().lock().unwrap().sniff_out.clone();
                    if want != cur {
                        match want {
                            Some(o) => sniff_kick(o, now),
                            None => {
                                if !SNIFF_BUSY.load(Ordering::SeqCst) {
                                    sniffer_stop();
                                }
                            }
                        }
                    }
                }
            }
            state_step(now);
            if tick % 4 == 0 {
                recv_step(now);
            }
            // #15: sends run beside the loop, bounded — a data channel waiting on a reconnect must not
            // freeze the snapshot (a stale "all good", a warning that never expires). Receipts coalesce.
            // 11.3: a same-room link / speakers change goes out with the next step, not the 3 s cadence
            if (tick % 12 == 0 || crate::sameroom::take_rx_poke()) && !rx_in_flight.load(Ordering::Relaxed) {
                if let Some(pkt) = rx_packet() {
                    rx_in_flight.store(true, Ordering::Relaxed);
                    let (lp2, busy, logged) = (lp.clone(), rx_in_flight.clone(), rx_err_logged.clone());
                    tokio::spawn(async move {
                        match tokio::time::timeout(Duration::from_secs(2), lp2.publish_data(pkt)).await {
                            Ok(Ok(())) => logged.store(false, Ordering::Relaxed),
                            Ok(Err(e)) => {
                                if !logged.swap(true, Ordering::Relaxed) {
                                    eprintln!("[hear] receive report not sent: {e}");
                                }
                            }
                            Err(_) => {
                                if !logged.swap(true, Ordering::Relaxed) {
                                    eprintln!("[hear] receive report not sent within 2 s — skipping");
                                }
                            }
                        }
                        busy.store(false, Ordering::Relaxed);
                    });
                }
            }
            if tick % 20 == 10 {
                for pkt in echo_step(now) {
                    let lp2 = lp.clone();
                    tokio::spawn(async move {
                        let _ = tokio::time::timeout(Duration::from_secs(5), lp2.publish_data(pkt)).await;
                    });
                }
            }
            snapshot();
        }
    })
}

/// A sniffer start is running (on a blocking thread, never awaited by the analyzer).
static SNIFF_BUSY: AtomicBool = AtomicBool::new(false);

/// Start the sniffer on `out` beside the analyzer. On the rig a monitor that would not start held the
/// analyzer 5.5 s per attempt, every 5 s — the snapshot and receipts starved ("heard by" flickered) and
/// each attempt left a stuck state change behind. Now: one start at a time; a failed output waits
/// 30 s, 60 s, then is left alone for the rest of the call (after its third failure).
fn sniff_kick(out: String, now: Instant) {
    {
        let h = hear().lock().unwrap();
        if let Some((o, fails, next)) = h.sniff_try.as_ref() {
            if *o == out && (*fails >= 3 || now < *next) {
                return;
            }
        }
    }
    if SNIFF_BUSY.swap(true, Ordering::SeqCst) {
        return;
    }
    let session = SESSION.load(Ordering::SeqCst);
    tokio::task::spawn_blocking(move || {
        let ok = sniffer_start(&out);
        if SESSION.load(Ordering::SeqCst) == session {
            if let Ok(mut h) = hear().lock() {
                if ok {
                    h.sniff_try = None;
                } else {
                    let fails = match h.sniff_try.as_ref() {
                        Some((o, f, _)) if *o == out => f + 1,
                        _ => 1,
                    };
                    if fails >= 3 {
                        eprintln!("[hear] system-audio sniffer: giving up on the monitor of {out:?} for this call");
                    }
                    h.sniff_try = Some((out.clone(), fails, Instant::now() + Duration::from_secs(30 << (fails - 1).min(1))));
                }
            }
        }
        SNIFF_BUSY.store(false, Ordering::SeqCst);
    });
}

fn sniff_wanted(f: &crate::call::EcFacts) -> Option<String> {
    // Echo cancellation off (or skipped): listen where system audio plays (the default output —
    // the call itself is known exactly from what we decode). On: only an output the canceller
    // does NOT reference can still put echo into the call.
    let default_out = f.default_sink.clone();
    if !f.enabled || !f.engaged {
        return Some(default_out); // "" when it cannot be named = the default output all the same
    }
    let covered = f.ref_outputs.iter().any(|o| o.is_empty() || *o == default_out);
    if covered || default_out.is_empty() {
        None
    } else {
        Some(default_out)
    }
}

fn ec_fix(f: &crate::call::EcFacts, h: &Hear) -> (String, String, String) {
    let k = |s: &str| s.to_string();
    if !f.enabled {
        return (k("turn_on"), String::new(), String::new());
    }
    if !f.dsp_tier {
        return (k("volume"), String::new(), String::new());
    }
    if !f.skip.is_empty() {
        // The automatic headset skip decided "headphones"; echo says otherwise → make it explicit.
        return (k("add_output"), f.call_output_resolved.clone(), f.out_label.clone());
    }
    if f.degraded || !f.engaged {
        return (k("retry"), String::new(), String::new());
    }
    if let (Some(out), Some(_)) = (h.sniff_out.as_ref(), h.local_sys.on) {
        if !out.is_empty() && !f.ref_outputs.iter().any(|o| o == out) {
            return (k("add_output"), out.clone(), f.default_sink_label.clone());
        }
    }
    (k("volume"), String::new(), String::new())
}

fn state_step(now: Instant) {
    let primed = crate::call::mic_capture_primed();
    let server_muted = crate::call::i_am_server_muted();
    let mut h = hear().lock().unwrap();
    let muted = h.mic_muted;
    // speaking history for "can't hear X"
    let speaking: Vec<String> = h.speaking_now.iter().cloned().collect();
    let present: Vec<String> = h.present.iter().cloned().collect();
    for id in present {
        let on = speaking.contains(&id);
        let q = h.spk_hist.entry(id).or_default();
        q.push_back(on);
        while q.len() > 12 {
            q.pop_front();
        }
    }
    // mic check
    let before = h.mic.check;
    let c = h.mic.step(now, muted);
    if before != Some(c) {
        let dev = if h.mic.device.is_empty() { "the system default input".to_string() } else { h.mic.device.clone() };
        match c {
            MicCheck::Hint => eprintln!("[hear] mic check: hint — no sound from {dev} yet"),
            MicCheck::Dead => eprintln!("[hear] mic check: dead — {dev} has sent a flat line for a minute unmuted"),
            MicCheck::OsMuted => eprintln!("[hear] mic check: the input is muted in the system settings"),
            MicCheck::OsVol0 => eprintln!("[hear] mic check: the input volume is 0 % in the system settings"),
            MicCheck::Ok if before.is_some() && before != Some(MicCheck::Wait) => eprintln!("[hear] mic check: ok"),
            _ => {}
        }
    }
    // muted talk ends by itself
    if h.muted_talk_until.map(|t| now >= t).unwrap_or(false) || !muted {
        if h.muted_talk_until.is_some() && h.muted_talk_logged {
            eprintln!("[hear] muted talk: stopped");
        }
        h.muted_talk_until = None;
        h.muted_talk_logged = false;
    }
    // not coming through
    let eligible = !muted && !server_muted && primed && !h.reconnecting;
    let talk = h.vad.talk_count(200, false) >= 100;
    let loud = h.vad.loud_count(200) >= 80;
    let level = h.vad.talk_median(200);
    let spoke = h.self_spoke_at;
    let before = h.tx.state;
    let st = h.tx.step(now, eligible, talk, loud, level, spoke);
    if st != before {
        match st {
            TxState::Ok => eprintln!("[hear] tx: ok — the call server hears you again"),
            TxState::NotSent => eprintln!("[hear] tx: NOT COMING THROUGH — {}", h.tx.why),
            TxState::Quiet => eprintln!("[hear] tx: quiet — {}", h.tx.why),
        }
    }
}

/// What we receive from one person, as a state code (see [`recv_state`]).
pub(crate) fn recv_state(
    muted: bool,
    stalled: bool,
    nokey: bool,
    since_added: Duration,
    frames: u64,
    since_frame: Option<Duration>,
    speaking_samples: usize,
    recent_max_db: Option<f32>,
) -> &'static str {
    if muted {
        "muted"
    } else if frames == 0 && since_added < Duration::from_secs(4) {
        "waiting"
    } else if stalled {
        "stalled"
    } else if nokey {
        "nokey"
    } else if since_frame.map(|d| d > Duration::from_secs(2)).unwrap_or(true) {
        "noaudio"
    } else if speaking_samples >= 6 && recent_max_db.map(|d| d < -60.0).unwrap_or(true) {
        "silent"
    } else {
        "ok"
    }
}

fn i_hear(h: &Hear, identity: &str, now: Instant) -> Option<&'static str> {
    let r = h.recv.values().filter(|r| r.identity == identity && r.category == "mic").max_by_key(|r| r.added)?;
    let spk = h.spk_hist.get(identity).map(|q| q.iter().filter(|&&b| b).count()).unwrap_or(0);
    Some(recv_state(
        r.muted,
        r.stalled,
        crate::call::missing_key_for(identity, 3),
        now.duration_since(r.added),
        r.frames,
        r.last_frame.map(|t| now.duration_since(t)),
        spk,
        r.env.max_since(300),
    ))
}

fn recv_step(now: Instant) {
    let mut h = hear().lock().unwrap();
    let ids: Vec<String> = h.present.iter().filter(|i| !is_recorder(i)).cloned().collect();
    let mut changes = Vec::new();
    for id in ids {
        let st = i_hear(&h, &id, now).unwrap_or("nomic").to_string();
        if h.last_states.get(&id) != Some(&st) {
            changes.push((id.clone(), st.clone()));
            h.last_states.insert(id, st);
        }
    }
    for (id, st) in changes {
        eprintln!("[hear] receiving {id}: {st}");
    }
    // stale reports expire
    h.reports.retain(|_, r| now.duration_since(r.at) < Duration::from_secs(20));
    h.echo_me.retain(|_, r| now.duration_since(r.0) < Duration::from_secs(75));
}

fn rx_packet() -> Option<livekit::DataPacket> {
    let h = hear().lock().unwrap();
    if !h.on {
        return None;
    }
    let now = Instant::now();
    let mut rx: Vec<(String, String)> = Vec::new();
    for id in h.present.iter().filter(|i| !is_recorder(i)) {
        if let Some(st) = i_hear(&h, id, now) {
            rx.push((id.clone(), st.to_string()));
        }
    }
    rx.sort();
    let deaf = crate::call::deafened_active();
    let away = crate::call::away_privacy_active(); // BRB cuts incoming call audio too
    let extra = crate::sameroom::rx_extra(); // 11.3: "cap":["sr"] (+ the link while linked)
    Some(livekit::DataPacket {
        payload: build_rx_report_with(deaf, away, &rx, &extra).into_bytes(),
        topic: Some(RX_TOPIC.to_string()),
        reliable: false,
        destination_identities: Vec::new(),
    })
}

/// Pull `len` frames from tick `from` out of an envelope: missing frames as `fill`, muted frames
/// (flag bit0) as `muted_as` (None = keep). Returns the frames and how many were missing.
fn pull(e: &Env, from: u64, len: usize, fill: f32, muted_as: Option<f32>) -> (Vec<f32>, usize) {
    let mut missing = 0;
    let v = e
        .slice(from, len)
        .into_iter()
        .map(|x| match x {
            Some((d, f)) => {
                if f & 1 != 0 {
                    muted_as.unwrap_or(d)
                } else {
                    d
                }
            }
            None => {
                missing += 1;
                fill
            }
        })
        .collect();
    (v, missing)
}

fn echo_step(now: Instant) -> Vec<livekit::DataPacket> {
    let t = now_tick().saturating_sub(20);
    // Copy out under the lock, correlate without it.
    let (sent_live, sent_all, peers, call_mix, sys) = {
        let h = hear().lock().unwrap();
        let Some(sent) = h.sent.as_ref() else { return Vec::new() };
        let from_e = t.saturating_sub((ECHO_LMAX + WIN) as u64);
        let (a_live, miss) = pull(sent, from_e, WIN, FLOOR_DB, Some(FLOOR_DB));
        let from_l = t.saturating_sub((LOCAL_LMAX + WIN) as u64);
        let (b_all, miss_l) = pull(sent, from_l, WIN + LOCAL_LMAX, FLOOR_DB, None);
        let mut newest: HashMap<&str, &Recv> = HashMap::new();
        for r in h.recv.values().filter(|r| r.category == "mic" && !is_recorder(&r.identity)) {
            let keep = newest.get(r.identity.as_str()).map(|o| o.added < r.added).unwrap_or(true);
            if keep {
                newest.insert(r.identity.as_str(), r);
            }
        }
        let peers: Vec<(String, Vec<f32>)> = newest
            .values()
            .map(|r| (r.identity.clone(), pull(&r.env, from_e, WIN + ECHO_LMAX, FLOOR_DB, None).0))
            .collect();
        let mut mix = vec![0.0f64; WIN + LOCAL_LEAD];
        let mut any_rx = false;
        for r in h.recv.values() {
            if is_recorder(&r.identity) {
                continue;
            }
            // everything we play from the call, summed in energy: the local reference
            let (m, _) = pull(&r.env, from_l, WIN + LOCAL_LEAD, FLOOR_DB, None);
            for (i, d) in m.iter().enumerate() {
                mix[i] += 10f64.powf(*d as f64 / 10.0);
            }
            any_rx = true;
        }
        let call_mix: Option<Vec<f32>> = any_rx.then(|| mix.iter().map(|&e| ((10.0 * e.log10()) as f32).max(FLOOR_DB)).collect());
        let sys = h.sys.as_ref().map(|e| pull(e, from_l, WIN + LOCAL_LEAD, FLOOR_DB, None).0);
        let live = (miss as f32) < 0.05 * WIN as f32;
        let all = (miss_l as f32) < 0.05 * (WIN + LOCAL_LMAX) as f32;
        (live.then_some(a_live), all.then_some(b_all), peers, call_mix, sys)
    };
    let mut results: Vec<(String, Win)> = Vec::new();
    if let Some(a) = sent_live.as_ref() {
        for (id, b) in &peers {
            results.push((id.clone(), classify_echo(a, b, ECHO_LMAX)));
        }
    }
    let local_call = match (call_mix.as_ref(), sent_all.as_ref()) {
        (Some(r), Some(m)) => Some(classify_local(r, m, WIN, LOCAL_LMAX, LOCAL_LEAD)),
        _ => None,
    };
    let local_sys = match (sys.as_ref(), sent_all.as_ref()) {
        (Some(r), Some(m)) => Some(classify_local(r, m, WIN, LOCAL_LMAX, LOCAL_LEAD)),
        _ => None,
    };
    // remote ↔ remote duplicates (the recorder-type mix-up, if it ever reaches a listener)
    let mut dups: Vec<(String, String)> = Vec::new();
    for i in 0..peers.len() {
        for j in (i + 1)..peers.len() {
            if peers[i].0 == peers[j].0 {
                continue;
            }
            let a = &peers[i].1[..WIN];
            if !active(a) {
                continue;
            }
            let c = ncc_curve(a, &peers[j].1[..WIN + 6], 6);
            if c.iter().cloned().fold(f32::MIN, f32::max) >= 0.9 {
                dups.push((peers[i].0.clone(), peers[j].0.clone()));
            }
        }
    }

    let mut out = Vec::new();
    let mut guard = hear().lock().unwrap();
    let h: &mut Hear = &mut guard;
    let me = h.me.clone();
    for (id, w) in results {
        if let Win::Dup { lag, .. } = w {
            if h.dup_logged.insert((me.clone(), id.clone())) {
                eprintln!("[hear] {id}'s audio is a copy of ours ({} ms) — the same microphone on two clients?", lag * 10);
            }
        }
        let tr = h.echo.entry(id.clone()).or_default();
        tr.note(now, w);
        let v = tr.verdict(now, Duration::from_secs(180), 6, 2, -40.0);
        if let Some((lag, db, n)) = v {
            let first = tr.on.is_none();
            tr.on = Some((lag, db));
            let due = tr.sent_at.map(|s| now.duration_since(s) >= Duration::from_secs(30)).unwrap_or(true);
            if first {
                if lag < ROOM_LAG_MS {
                    eprintln!("[hear] echo: {id}'s mic picks up our voice in the room ({lag} ms, {db:.0} dB, {n} windows)");
                } else {
                    eprintln!("[hear] echo: {id} sends our voice back {lag} ms later at {db:.0} dB ({n} windows)");
                }
            }
            if first || due {
                tr.sent_at = Some(now);
                out.push(echo_packet(&id, true, lag, db));
            }
        } else if tr.on.is_some() && tr.clear(now, Duration::from_secs(120), 6) {
            tr.on = None;
            tr.sent_at = None;
            eprintln!("[hear] echo: {id} no longer sends our voice back");
            out.push(echo_packet(&id, false, 0, 0.0));
        }
    }
    for (label, w, which) in [("the call", local_call, 0), ("system audio", local_sys, 1)] {
        let Some(w) = w else { continue };
        let tr = if which == 0 { &mut h.local_call } else { &mut h.local_sys };
        tr.note(now, w);
        match tr.verdict(now, Duration::from_secs(120), 5, 3, -60.0) {
            Some((lag, db, n)) => {
                if tr.on.is_none() {
                    eprintln!("[hear] local echo: our mic picks up {label} from the speakers ({lag} ms, {db:+.0} dB vs the source, {n} windows)");
                }
                tr.on = Some((lag, db));
            }
            None => {
                if tr.on.is_some() && tr.clear(now, Duration::from_secs(90), 4) {
                    tr.on = None;
                    eprintln!("[hear] local echo: {label} no longer reaches our mic");
                }
            }
        }
    }
    for (a, b) in dups {
        if h.dup_logged.insert((a.clone(), b.clone())) {
            eprintln!("[hear] DUPLICATE STREAM: what we receive as {b} is a copy of {a}");
        }
    }
    out
}

fn echo_packet(to: &str, on: bool, lag: u32, db: f32) -> livekit::DataPacket {
    let room = on && lag < ROOM_LAG_MS;
    livekit::DataPacket {
        payload: format!("{{\"v\":1,\"on\":{on},\"lag\":{lag},\"db\":{db:.1},\"room\":{room}}}").into_bytes(),
        topic: Some(ECHO_TOPIC.to_string()),
        reliable: true,
        destination_identities: vec![ParticipantIdentity(to.to_string())],
    }
}

// ── the UI snapshot ──────────────────────────────────────────────────────────────────────────

fn snapshot() {
    let now = Instant::now();
    let mut h = hear().lock().unwrap();
    if !h.on {
        return;
    }
    let mut s = String::from("{\"v\":1,\"on\":true,");
    s.push_str(&format!("\"me\":\"{}\",", json_esc(&h.me)));
    let mic_dev = if !h.mic_label.is_empty() {
        h.mic_label.clone()
    } else if h.mic.device.is_empty() {
        "your microphone".to_string()
    } else {
        h.mic.device.clone()
    };
    s.push_str(&format!(
        "\"mic\":{{\"check\":\"{}\",\"dev\":\"{}\"}},",
        h.mic.check.unwrap_or(MicCheck::Ok).as_str(),
        json_esc(&mic_dev)
    ));
    let out_check = if h.out_muted == Some(true) {
        "osmuted"
    } else if h.out_vol0 == Some(true) {
        "osvol0"
    } else {
        "ok"
    };
    s.push_str(&format!("\"out\":{{\"check\":\"{out_check}\",\"dev\":\"{}\"}},", json_esc(&h.out_label)));
    s.push_str(&format!("\"mutedTalk\":{},", h.muted_talk_until.is_some()));
    s.push_str(&format!(
        "\"tx\":{{\"state\":\"{}\",\"why\":\"{}\"}},",
        match h.tx.state {
            TxState::Ok => "ok",
            TxState::NotSent => "notsent",
            TxState::Quiet => "quiet",
        },
        json_esc(&h.tx.why)
    ));
    // per person
    let mut ids: Vec<String> = h.present.iter().filter(|i| !is_recorder(i) && **i != h.me).cloned().collect();
    ids.sort();
    let (mut ok, mut total) = (0, 0);
    let mut not: Vec<String> = Vec::new();
    let mut unconf: Vec<String> = Vec::new();
    let mut deaf_list: Vec<String> = Vec::new();
    let mut pending: Vec<String> = Vec::new();
    // Reports lag reality by up to one period: just after an unmute a peer still says "muted", just
    // after our mic went up it has not subscribed yet. Those are not faults — nobody gets a flash of
    // "not receiving you" for them.
    let unmute_grace = h.unmuted_at.map(|t| now.duration_since(t) < Duration::from_secs(7)).unwrap_or(false);
    let fresh_mic = h.mic.cap_started.map(|t| now.duration_since(t) < Duration::from_secs(10)).unwrap_or(true);
    s.push_str("\"peers\":{");
    for (n, id) in ids.iter().enumerate() {
        let rep = h.reports.get(id).filter(|r| now.duration_since(r.at) < Duration::from_secs(8));
        let hears_me = match rep {
            Some(r) => r.rx.get(&h.me).cloned().unwrap_or_else(|| "unknown".to_string()),
            None => "unconfirmed".to_string(),
        };
        total += 1;
        // `not_me` = the reason this person is NOT receiving us ("" = fine or unknowable) — the tile mark.
        let deafish = rep.map(|r| r.deaf || r.away).unwrap_or(false);
        let not_me: &str = match hears_me.as_str() {
            // #8: a deafened (or away) listener decodes us but hears nothing — not "heard".
            "ok" if deafish => {
                deaf_list.push(id.clone());
                ""
            }
            "ok" => {
                ok += 1;
                ""
            }
            "waiting" => {
                pending.push(id.clone()); // subscribed, first frame not yet — neither heard nor a fault
                ""
            }
            "unconfirmed" => {
                unconf.push(id.clone());
                ""
            }
            "unknown" if fresh_mic => {
                unconf.push(id.clone());
                ""
            }
            "unknown" => {
                not.push(id.clone());
                "nosub" // their app reports, but it has no track of ours: not subscribed to our mic
            }
            "muted" if h.mic_muted || unmute_grace => {
                ok += 1;
                ""
            }
            "muted" => {
                not.push(id.clone());
                "muted" // they still see us muted
            }
            other => {
                not.push(id.clone());
                match other {
                    "silent" => "silent",
                    "noaudio" => "noaudio",
                    "nokey" => "nokey",
                    "stalled" => "stalled",
                    _ => "other",
                }
            }
        };
        let deaf = rep.map(|r| r.deaf).unwrap_or(false);
        let away = rep.map(|r| r.away).unwrap_or(false);
        let ihear = h.last_states.get(id).cloned().unwrap_or_else(|| "nomic".to_string());
        let echo = match h.echo.get(id).and_then(|e| e.on) {
            Some((lag, db)) => format!("{{\"lag\":{lag},\"db\":{},\"room\":{}}}", jnum(db), lag < ROOM_LAG_MS),
            None => "null".to_string(),
        };
        if n > 0 {
            s.push(',');
        }
        s.push_str(&format!(
            "\"{}\":{{\"deaf\":{deaf},\"away\":{away},\"hearsMe\":\"{}\",\"notMe\":\"{not_me}\",\"iHear\":\"{}\",\"echo\":{echo}}}",
            json_esc(id),
            json_esc(&hears_me),
            json_esc(&ihear)
        ));
    }
    s.push_str("},");
    let list = |v: &[String]| v.iter().map(|x| format!("\"{}\"", json_esc(x))).collect::<Vec<_>>().join(",");
    s.push_str(&format!(
        "\"heard\":{{\"ok\":{ok},\"total\":{total},\"not\":[{}],\"unconf\":[{}],\"deaf\":[{}],\"pending\":[{}]}},",
        list(&not),
        list(&unconf),
        list(&deaf_list),
        list(&pending)
    ));
    // Speaker echo (they hear themselves) and same-room pickup (our mic hears them directly) are
    // listed apart: the fixes differ.
    let mut by: Vec<String> = h.echo_me.iter().filter(|(_, v)| !v.3).map(|(k, _)| k.clone()).collect();
    by.sort();
    let mut room: Vec<String> = h.echo_me.iter().filter(|(_, v)| v.3).map(|(k, _)| k.clone()).collect();
    room.sort();
    let (lag, db) = h.echo_me.values().filter(|v| !v.3).map(|v| (v.1, v.2)).fold((0u32, -99.0f32), |a, b| if b.1 > a.1 { b } else { a });
    s.push_str(&format!(
        "\"echoMe\":{{\"by\":[{}],\"room\":[{}],\"lag\":{lag},\"db\":{}}},",
        list(&by),
        list(&room),
        jnum(db)
    ));
    let local = match (h.local_call.on, h.local_sys.on) {
        (Some((lag, db)), _) => format!("{{\"src\":\"call\",\"lag\":{lag},\"db\":{}}}", jnum(db)),
        (None, Some((lag, db))) => format!("{{\"src\":\"system\",\"lag\":{lag},\"db\":{}}}", jnum(db)),
        _ => "null".to_string(),
    };
    s.push_str(&format!("\"localEcho\":{local},"));
    s.push_str(&format!(
        "\"ecFix\":{{\"kind\":\"{}\",\"out\":\"{}\",\"label\":\"{}\"}}",
        json_esc(&h.ec_fix.0),
        json_esc(&h.ec_fix.1),
        json_esc(&h.ec_fix.2)
    ));
    s.push('}');
    h.snapshot = s;
    // Field evidence (and the E2E gate's oracle): who receives us, and the echo fix, on change only.
    let heard = format!("{ok}/{total} not={not:?} unconfirmed={unconf:?}");
    if heard != h.last_heard {
        eprintln!("[hear] heard by {heard}");
        h.last_heard = heard;
    }
    let echo_on = h.echo_me.values().any(|v| !v.3) || h.local_call.on.is_some() || h.local_sys.on.is_some();
    let fix = if echo_on { format!("{} {}", h.ec_fix.0, h.ec_fix.1) } else { String::new() };
    if fix != h.last_fix {
        if !fix.is_empty() {
            eprintln!("[hear] echo fix offered: {fix}");
        }
        h.last_fix = fix;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A speech-like loudness envelope: bursts of 200–900 ms at −30…−15 dBFS with gaps.
    fn speechy(n: usize, seed: u64) -> Vec<f32> {
        speechy_gaps(n, seed, 10, 60)
    }
    /// The same with gaps of `g0`…`g0+gspan` frames (a listener who talks now and then).
    fn speechy_gaps(n: usize, seed: u64, g0: usize, gspan: usize) -> Vec<f32> {
        let mut x = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let mut rnd = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x % 10_000) as f32 / 10_000.0
        };
        let mut v = Vec::with_capacity(n);
        while v.len() < n {
            let talk = 20 + (rnd() * 70.0) as usize;
            let lvl = -30.0 + rnd() * 15.0;
            for _ in 0..talk {
                v.push(lvl + rnd() * 6.0 - 3.0);
            }
            let gap = g0 + (rnd() * gspan as f32) as usize;
            for _ in 0..gap {
                v.push(-62.0 + rnd() * 3.0);
            }
        }
        v.truncate(n);
        v
    }
    fn energy_sum(a: f32, b: f32) -> f32 {
        (10.0 * (10f32.powf(a / 10.0) + 10f32.powf(b / 10.0)).log10()).max(FLOOR_DB)
    }

    #[test]
    fn echo_is_found_at_its_delay_and_conversation_is_not() {
        let a = speechy(WIN + ECHO_LMAX, 1);
        // the other person mostly listens (a word now and then), like the recorded calls
        let other = speechy_gaps(WIN + ECHO_LMAX, 2, 150, 250);
        // B = the other person's own voice + our voice back 280 ms later, 25 dB down.
        let b: Vec<f32> = (0..WIN + ECHO_LMAX)
            .map(|i| {
                let echo = if i >= 28 { a[i - 28] - 25.0 } else { FLOOR_DB };
                energy_sum(other[i] - 6.0, echo)
            })
            .collect();
        match classify_echo(&a[..WIN], &b, ECHO_LMAX) {
            Win::Echo { lag, rel, .. } => {
                assert!((26..=30).contains(&lag), "lag {lag}");
                assert!(rel < -6.0, "rel {rel}");
            }
            w => panic!("expected echo, got {w:?}"),
        }
        // A conversation partner who does not echo: never an echo window.
        let mut echoes = 0;
        for s in 0..20u64 {
            let a = speechy(WIN, 10 + s);
            let b = speechy(WIN + ECHO_LMAX, 100 + s);
            if matches!(classify_echo(&a, &b, ECHO_LMAX), Win::Echo { .. }) {
                echoes += 1;
            }
        }
        assert!(echoes <= 2, "{echoes} false echo windows out of 20");
    }

    #[test]
    fn a_copy_of_the_stream_is_a_duplicate_not_an_echo() {
        let a = speechy(WIN + ECHO_LMAX, 3);
        let b: Vec<f32> = (0..WIN + ECHO_LMAX).map(|i| if i >= 2 { a[i - 2] } else { FLOOR_DB }).collect();
        assert!(matches!(classify_echo(&a[..WIN], &b, ECHO_LMAX), Win::Dup { .. }));
    }

    #[test]
    fn silence_teaches_nothing() {
        let a = vec![-62.0f32; WIN];
        let b = speechy(WIN + ECHO_LMAX, 4);
        assert_eq!(classify_echo(&a, &b, ECHO_LMAX), Win::Idle);
    }

    #[test]
    fn local_echo_from_the_speakers_is_found() {
        let r = speechy(WIN + LOCAL_LMAX, 5);
        // our mic: quiet room + the speakers 120 ms later, 10 dB down (loud speakers, EC off)
        let m: Vec<f32> = (0..WIN + LOCAL_LMAX)
            .map(|i| if i >= 12 { energy_sum(r[i - 12] - 10.0, -60.0) } else { -60.0 })
            .collect();
        match classify_local(&r, &m, WIN, LOCAL_LMAX, LOCAL_LEAD) {
            Win::Echo { lag, .. } => assert!((11..=13).contains(&lag), "lag {lag}"),
            w => panic!("expected local echo, got {w:?}"),
        }
        // a short path (the output's monitor → the mic, 20 ms) is still an echo with the lead search on
        let m2: Vec<f32> = (0..WIN + LOCAL_LMAX)
            .map(|i| if i >= 2 { energy_sum(r[i - 2] - 10.0, -60.0) } else { -60.0 })
            .collect();
        assert!(matches!(classify_local(&r, &m2, WIN, LOCAL_LMAX, LOCAL_LEAD), Win::Echo { .. }));
        // echo cancellation removed it: the mic only has our own voice
        let own = speechy(WIN + LOCAL_LMAX, 6);
        assert!(!matches!(classify_local(&r, &own, WIN, LOCAL_LMAX, LOCAL_LEAD), Win::Echo { .. }));
    }

    #[test]
    fn a_talker_in_our_room_is_not_a_local_echo() {
        // Two people in one room: our mic hears them through the air 60–120 ms BEFORE the call
        // plays them — no speaker echo, whatever the correlation near lag 0 says.
        for (seed, early) in [(7u64, 6usize), (8, 9), (9, 12)] {
            let r = speechy(WIN + LOCAL_LMAX + early, seed);
            let m: Vec<f32> = (0..WIN + LOCAL_LMAX).map(|i| energy_sum(r[i + early] - 12.0, -60.0)).collect();
            let w = classify_local(&r, &m, WIN, LOCAL_LMAX, LOCAL_LEAD);
            assert!(!matches!(w, Win::Echo { .. }), "early {early}: {w:?}");
        }
    }

    #[test]
    fn verdict_needs_one_delay_many_times() {
        let t = Instant::now();
        let mut tr = EchoTrack::default();
        for i in 0..5 {
            tr.note(t + Duration::from_secs(5 * i), Win::Echo { lag: 30, ncc: 0.5, rel: -25.0 });
        }
        assert!(tr.verdict(t + Duration::from_secs(25), Duration::from_secs(180), 6, 2, -40.0).is_none());
        tr.note(t + Duration::from_secs(30), Win::Echo { lag: 31, ncc: 0.5, rel: -24.0 });
        let v = tr.verdict(t + Duration::from_secs(30), Duration::from_secs(180), 6, 2, -40.0).unwrap();
        assert_eq!(v.0, 300);
        // scattered lags never make a verdict
        let mut sc = EchoTrack::default();
        for i in 0..12usize {
            sc.note(t + Duration::from_secs(5 * i as u64), Win::Echo { lag: 10 + i * 9, ncc: 0.4, rel: -20.0 });
        }
        assert!(sc.verdict(t + Duration::from_secs(60), Duration::from_secs(180), 6, 2, -40.0).is_none());
        // too faint to hear → no verdict
        let mut faint = EchoTrack::default();
        for i in 0..8 {
            faint.note(t + Duration::from_secs(5 * i), Win::Echo { lag: 30, ncc: 0.4, rel: -47.0 });
        }
        assert!(faint.verdict(t + Duration::from_secs(40), Duration::from_secs(180), 6, 2, -40.0).is_none());
        // cleared after enough clean talking
        for i in 0..6 {
            tr.note(t + Duration::from_secs(200 + 5 * i), Win::Clean);
        }
        assert!(tr.clear(t + Duration::from_secs(230), Duration::from_secs(120), 6));
    }

    #[test]
    fn mic_check_flat_hint_then_dead_and_alive_is_forever() {
        let t = Instant::now();
        let mut m = MicState::default();
        m.committed("AutoMic", t);
        assert_eq!(m.step(t + Duration::from_secs(5), false), MicCheck::Wait);
        assert_eq!(m.step(t + Duration::from_secs(16), false), MicCheck::Hint);
        // muted time does not count towards "dead"
        let mut now = t + Duration::from_secs(16);
        for _ in 0..30 {
            now += Duration::from_secs(1);
            m.step(now, true);
        }
        assert_eq!(m.step(now, true), MicCheck::Hint);
        for _ in 0..61 {
            now += Duration::from_secs(1);
            m.step(now, false);
        }
        assert_eq!(m.step(now, false), MicCheck::Dead);
        // a gated mic: flat between words, alive after the first word — then never again
        let mut g = MicState::default();
        g.committed("AutoMic", t);
        for _ in 0..ALIVE_BUFFERS {
            g.raw(1e-3);
        }
        for _ in 0..1000 {
            g.raw(0.0);
        }
        assert_eq!(g.step(t + Duration::from_secs(600), false), MicCheck::Ok);
        // a rebuild of the same device stays alive; a DC line is flat
        g.committed("AutoMic", t + Duration::from_secs(700));
        assert_eq!(g.step(t + Duration::from_secs(800), false), MicCheck::Ok);
        let mut dc = MicState::default();
        dc.committed("acp-dmic", t);
        for _ in 0..100 {
            dc.raw(0.0); // the note_raw path removes the mean: a constant −1.0 arrives as AC 0
        }
        assert_eq!(dc.step(t + Duration::from_secs(20), false), MicCheck::Hint);
        // the system says muted → red at once
        dc.os_muted = Some(true);
        assert_eq!(dc.step(t + Duration::from_secs(21), false), MicCheck::OsMuted);
    }

    #[test]
    fn muted_talk_needs_sustained_speech_and_learns_the_voice() {
        let mut v = Vad::default();
        for _ in 0..100 {
            v.push(-55.0, true, false); // a quiet room
        }
        assert!(!muted_talking(&v));
        for _ in 0..20 {
            v.push(-20.0, true, false); // a cough
        }
        assert!(!muted_talking(&v));
        for _ in 0..50 {
            v.push(-24.0, true, false);
        }
        assert!(muted_talking(&v));
        // learned level: a loud talker's threshold rises (typing at −40 no longer counts)
        let mut l = Vad::default();
        for _ in 0..400 {
            l.push(-12.0, false, true);
        }
        assert!(l.threshold() > -30.0, "{}", l.threshold());
    }

    #[test]
    fn tx_flags_two_unheard_loud_bursts_and_clears_when_heard() {
        let t = Instant::now();
        let mut tx = Tx::default();
        let mut now = t;
        let mut burst = |tx: &mut Tx, now: &mut Instant, spoke: Option<Instant>| {
            for _ in 0..8 {
                *now += Duration::from_millis(250);
                tx.step(*now, true, true, true, -20.0, spoke);
            }
            for _ in 0..8 {
                *now += Duration::from_millis(250);
                tx.step(*now, true, false, false, -60.0, spoke);
            }
        };
        burst(&mut tx, &mut now, None);
        assert_eq!(tx.state, TxState::Ok);
        burst(&mut tx, &mut now, None);
        assert_eq!(tx.state, TxState::NotSent);
        let heard = now + Duration::from_millis(300);
        burst(&mut tx, &mut now, Some(heard));
        assert_eq!(tx.state, TxState::Ok);
        // quiet talkers: three quiet unconfirmed bursts → Quiet, never NotSent
        let mut q = Tx::default();
        let mut now = t;
        for _ in 0..3 {
            for _ in 0..8 {
                now += Duration::from_millis(250);
                q.step(now, true, true, false, -44.0, None);
            }
            for _ in 0..8 {
                now += Duration::from_millis(250);
                q.step(now, true, false, false, -60.0, None);
            }
        }
        assert_eq!(q.state, TxState::Quiet);
        // not eligible (muted) → nothing to say
        assert_eq!(q.step(now, false, true, true, -20.0, None), TxState::Ok);
    }

    #[test]
    fn receive_states() {
        let s = Duration::from_secs;
        assert_eq!(recv_state(false, false, false, s(1), 0, None, 0, None), "waiting");
        assert_eq!(recv_state(false, false, false, s(10), 500, Some(s(5)), 0, Some(-20.0)), "noaudio");
        assert_eq!(recv_state(false, false, false, s(10), 500, Some(s(0)), 8, Some(-70.0)), "silent");
        assert_eq!(recv_state(false, false, false, s(10), 500, Some(s(0)), 8, Some(-25.0)), "ok");
        assert_eq!(recv_state(false, false, true, s(10), 500, Some(s(0)), 8, Some(-70.0)), "nokey");
        assert_eq!(recv_state(false, true, false, s(10), 500, Some(s(0)), 8, Some(-25.0)), "stalled");
        assert_eq!(recv_state(true, false, false, s(10), 500, Some(s(9)), 0, None), "muted");
    }

    #[test]
    fn rx_report_roundtrip() {
        let p = build_rx_report(true, false, &[("@a:x:D1".into(), "ok".into()), ("@b:x:D2".into(), "silent".into())]);
        let (deaf, away, rx) = parse_rx_report(&p).unwrap();
        assert!(deaf && !away);
        assert_eq!(rx.get("@a:x:D1").map(String::as_str), Some("ok"));
        assert_eq!(rx.get("@b:x:D2").map(String::as_str), Some("silent"));
        let (d2, a2, e) = parse_rx_report("{\"v\":1,\"deaf\":false,\"away\":true,\"rx\":{}}").unwrap();
        assert!(!d2 && a2 && e.is_empty());
        // an older 10.5 report without "away" still parses
        let (_, a3, _) = parse_rx_report("{\"v\":1,\"deaf\":false,\"rx\":{}}").unwrap();
        assert!(!a3);
        assert!(parse_rx_report("garbage").is_none());
        // the recorder is whoever the server announces (set_recorder); none announced = nobody
        assert!(is_recorder_with("@recorder:example.com:ABC", Some("@recorder:example.com")));
        assert!(is_recorder_with("@recorder:example.com", Some("@recorder:example.com")));
        assert!(!is_recorder_with("@logan:example.com:ABC", Some("@recorder:example.com")));
        assert!(!is_recorder_with("@recorder:other.test:ABC", Some("@recorder:example.com")));
        assert!(!is_recorder_with("@recorder:example.com:ABC", None));
    }

    #[test]
    fn snapshot_numbers_are_always_json() {
        // Astra 2026-09-22 #12: a report with "db":inf once made the whole snapshot invalid JSON.
        assert_eq!(jnum(f32::INFINITY), "0");
        assert_eq!(jnum(f32::NAN), "0");
        assert_eq!(jnum(-14.4), "-14");
    }

    #[test]
    fn pactl_parsing() {
        assert_eq!(parse_mute("Mute: yes\n"), Some(true));
        assert_eq!(parse_mute("Mute: no\n"), Some(false));
        assert_eq!(parse_vol0("Volume: front-left: 0 /   0% / -inf dB,   front-right: 0 /   0% / -inf dB\n        balance 0.00"), Some(true));
        assert_eq!(parse_vol0("Volume: mono: 65536 / 100% / 0.00 dB\n"), Some(false));
        assert_eq!(parse_vol0("nothing"), None);
    }
}
