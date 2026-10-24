//! Same room (Tin Hat 11.3) — the room-mic election and the raw-level tracker. PURE: no GStreamer, no
//! LiveKit, no Qt — the web runs the same rules (`src/features/calls/sameroom/elect.ts`) and both replay
//! `vectors.json` (byte-identical in both repos). See docs/SAME-ROOM.md.
//!
//! Linked members (two laptops in one room) each measure their RAW mic (before echo cancellation / noise
//! suppression / gain) as "signal above the mic's own noise floor" (SNR dB) and exchange it 20× a second
//! (`tinhat.sr.lvl`, lossy, group only). Every tick each member runs [`elect_tick`] on its own fresh level +
//! the peers' latest ones; exactly one member — the holder — sends its mic, the others send silence:
//!   • no holder (or the holder left the group) → the lowest identity holds (deterministic, no negotiation);
//!   • a member takes the mic when it is speaking (SNR ≥ `SPEECH_DB`) AND beats the holder by ≥ `MARGIN_DB`
//!     for `WIN_TICKS` consecutive ticks (hysteresis: a chair creak, a 2 dB wobble or one quiet packet still in
//!     flight at a speech onset never flips it); callers pass each peer's level as the max of its packets from
//!     the last `HOLD_MS`;
//!   • a peer level older than `STALE_MS` counts as silent (−∞): packet loss or a frozen peer cannot hold
//!     the mic against someone who is talking; my own level is always fresh;
//!   • ties go to the lower identity, so two members comparing the same numbers agree.
//! Why RAW levels: a laptop's own speakers are loudest at its own mic, so it keeps the mic for the sound
//! they make and its own canceller removes them — the far end never leaks back through the other laptop.

use std::collections::BTreeMap;

pub const SPEECH_DB: i32 = 6;
pub const MARGIN_DB: i32 = 3;
/// 200 ms: longer than a level packet's flight, so an onset never flips the mic (rig 2026-10-11).
pub const WIN_TICKS: u32 = 4;
pub const STALE_MS: u64 = 400;
/// Callers feed a peer's level as the max of its packets from this window.
pub const HOLD_MS: u64 = 150;
/// 0.5 dB/s at 20 windows/s: speech pauses pull the floor back down at once.
pub const FLOOR_RISE_DB_PER_WIN: f64 = 0.025;
pub const FLOOR_MIN_DB: f64 = -90.0;
pub const FLOOR_MAX_DB: f64 = -20.0;
pub const SNR_MAX_DB: i32 = 60;

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Level {
    /// SNR dB (an integer on the wire).
    pub s: i32,
    /// ms — the receiver's clock when it arrived (mine: any value, always fresh).
    pub t: u64,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Wins {
    pub who: String,
    pub n: u32,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct ElectState {
    pub holder: String,
    pub wins: Wins,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ElectResult {
    pub holder: String,
    pub wins: Wins,
    /// holder == me → my mic sends.
    pub live: bool,
}

/// One election tick. `members` = the group (including `me`), any order; `levels` = the latest level per
/// identity; `now` ms on the same clock as the peers' `t`.
pub fn elect_tick(st: &ElectState, me: &str, members: &[String], levels: &BTreeMap<String, Level>, now: u64) -> ElectResult {
    let mut members: Vec<&str> = members.iter().map(|s| s.as_str()).collect();
    members.sort_unstable();
    members.dedup();
    if members.is_empty() {
        return ElectResult { holder: String::new(), wins: Wins::default(), live: false };
    }
    let lvl = |id: &str| -> i64 {
        match levels.get(id) {
            None => i64::MIN,
            Some(l) if id == me => l.s as i64,
            Some(l) => {
                if now.saturating_sub(l.t) <= STALE_MS {
                    l.s as i64
                } else {
                    i64::MIN
                }
            }
        }
    };
    let mut holder: &str = if members.contains(&st.holder.as_str()) { st.holder.as_str() } else { members[0] };
    let mut wins = st.wins.clone();
    let mut best: Option<(&str, i64)> = None;
    for id in &members {
        if *id == holder {
            continue;
        }
        let v = lvl(id);
        if v < SPEECH_DB as i64 {
            continue;
        }
        match best {
            None => best = Some((id, v)),
            Some((bid, bv)) => {
                if v > bv || (v == bv && *id < bid) {
                    best = Some((id, v));
                }
            }
        }
    }
    let hold = lvl(holder);
    match best {
        Some((bid, bv)) if bv >= hold.saturating_add(MARGIN_DB as i64) => {
            wins = if wins.who == bid { Wins { who: bid.to_string(), n: wins.n + 1 } } else { Wins { who: bid.to_string(), n: 1 } };
            if wins.n >= WIN_TICKS {
                holder = bid;
                wins = Wins::default();
            }
        }
        _ => {
            if wins.n != 0 || !wins.who.is_empty() {
                wins = Wins::default();
            }
        }
    }
    ElectResult { holder: holder.to_string(), wins, live: holder == me }
}

/// The raw mic's noise floor + SNR per 50 ms window (identical on both apps — vectors.json "floor"): the floor
/// starts at the first window's level (clamped), every later window below it becomes the floor, otherwise the
/// floor rises by `FLOOR_RISE_DB_PER_WIN`; SNR = round(window − floor) clamped to 0..=`SNR_MAX_DB`.
#[derive(Debug, Default)]
pub struct FloorTracker {
    pub floor: Option<f64>,
}

impl FloorTracker {
    /// `db` = the window's level in dBFS. Returns the integer SNR dB to put on the wire.
    pub fn push(&mut self, db: f64) -> i32 {
        let d = if db.is_finite() { db } else { FLOOR_MIN_DB };
        let f = match self.floor {
            Some(f) if d >= f => (f + FLOOR_RISE_DB_PER_WIN).min(FLOOR_MAX_DB),
            _ => d.clamp(FLOOR_MIN_DB, FLOOR_MAX_DB),
        };
        self.floor = Some(f);
        let snr = (d - f).round() as i64;
        snr.clamp(0, SNR_MAX_DB as i64) as i32
    }
    pub fn reset(&mut self) {
        self.floor = None;
    }
}

/// dBFS of a window of linear RMS frames (the loudest 10 ms frame), floor −100.
pub fn window_db(frames: &[f32]) -> f64 {
    let m = frames.iter().cloned().fold(0.0f32, f32::max);
    if m > 0.0 {
        (20.0 * (m as f64).log10()).max(-100.0)
    } else {
        -100.0
    }
}

/// The group id both sides compute without negotiation.
pub fn group_id_of(members: &[String]) -> String {
    let mut m: Vec<&str> = members.iter().map(|s| s.as_str()).collect();
    m.sort_unstable();
    m.join("+")
}

#[cfg(test)]
mod tests {
    use super::*;

    const VECTORS: &str = include_str!("vectors.json");

    #[test]
    fn shared_vectors_replay() {
        let v: serde_json::Value = serde_json::from_str(VECTORS).expect("vectors.json parses");
        let c = &v["const"];
        assert_eq!(c["speechDb"].as_i64().unwrap() as i32, SPEECH_DB);
        assert_eq!(c["marginDb"].as_i64().unwrap() as i32, MARGIN_DB);
        assert_eq!(c["winTicks"].as_u64().unwrap() as u32, WIN_TICKS);
        assert_eq!(c["staleMs"].as_u64().unwrap(), STALE_MS);
        assert_eq!(c["holdMs"].as_u64().unwrap(), HOLD_MS);
        assert_eq!(c["floorRiseDbPerWin"].as_f64().unwrap(), FLOOR_RISE_DB_PER_WIN);
        assert_eq!(c["floorMinDb"].as_f64().unwrap(), FLOOR_MIN_DB);
        assert_eq!(c["floorMaxDb"].as_f64().unwrap(), FLOOR_MAX_DB);
        assert_eq!(c["snrMaxDb"].as_i64().unwrap() as i32, SNR_MAX_DB);
        let mut cases = 0;
        for case in v["elect"].as_array().unwrap() {
            cases += 1;
            let name = case["name"].as_str().unwrap();
            let me = case["me"].as_str().unwrap();
            let members: Vec<String> = case["members"].as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect();
            let mut st = ElectState::default();
            if let Some(s0) = case.get("state") {
                st.holder = s0["holder"].as_str().unwrap_or("").to_string();
                st.wins = Wins { who: s0["wins"]["who"].as_str().unwrap_or("").to_string(), n: s0["wins"]["n"].as_u64().unwrap_or(0) as u32 };
            }
            for (i, step) in case["steps"].as_array().unwrap().iter().enumerate() {
                let now = step["now"].as_u64().unwrap();
                let mut levels = BTreeMap::new();
                for (id, l) in step["levels"].as_object().unwrap() {
                    levels.insert(id.clone(), Level { s: l["s"].as_i64().unwrap() as i32, t: l["t"].as_u64().unwrap() });
                }
                let r = elect_tick(&st, me, &members, &levels, now);
                assert_eq!(r.holder, step["expect"]["holder"].as_str().unwrap(), "{name}: step {} holder", i + 1);
                assert_eq!(r.live, step["expect"]["live"].as_bool().unwrap(), "{name}: step {} live", i + 1);
                st = ElectState { holder: r.holder, wins: r.wins };
            }
        }
        for case in v["floor"].as_array().unwrap() {
            cases += 1;
            let name = case["name"].as_str().unwrap();
            let mut f = FloorTracker::default();
            let got: Vec<i64> = case["windowsDb"].as_array().unwrap().iter().map(|d| f.push(d.as_f64().unwrap()) as i64).collect();
            let want: Vec<i64> = case["expectSnr"].as_array().unwrap().iter().map(|d| d.as_i64().unwrap()).collect();
            assert_eq!(got, want, "{name}");
        }
        assert!(cases >= 10, "the vector file is not empty");
    }

    #[test]
    fn invariants() {
        let none = BTreeMap::new();
        assert!(!elect_tick(&ElectState::default(), "@a:x", &[], &none, 0).live);
        let mut one = BTreeMap::new();
        one.insert("@a:x".to_string(), Level { s: 0, t: 0 });
        assert!(elect_tick(&ElectState::default(), "@a:x", &["@a:x".to_string()], &one, 0).live);
        assert_eq!(group_id_of(&["@b:x".to_string(), "@a:x".to_string()]), "@a:x+@b:x");
        assert_eq!(window_db(&[0.0, 0.0]), -100.0);
        assert!((window_db(&[0.01, 0.1, 0.05]) + 20.0).abs() < 1e-5); // f32 0.1 → −19.9999999
        assert_eq!(FloorTracker::default().push(f64::NAN), 0);
    }
}
