//! Same room (Tin Hat 11.3) — two call members in one physical room "link up": they do not play each other's
//! audio, they exchange RAW mic levels 20× a second and run the room-mic election ([`elect`]) so exactly one
//! of them sends at a time, and they tell the call about the link through `tinhat.rx`. The web client
//! (`src/features/calls/sameroom/link.ts`) is byte-compatible. Wire:
//!   tinhat.rx (existing, lossy, 3 s): + "cap":["sr"], + "sr":{"g","m":[…],"mic","spk"} while linked
//!   tinhat.sr.link  (reliable, to one peer): {"v":1,"op":"ask"|"yes"|"no"|"unlink","g","m":[…],"auto"?:true}
//!   tinhat.sr.lvl   (lossy, to the group):   {"s":<snr dB int>,"t":<sender ms>}
//! Remembered pairs (`<app data dir>/sameroom_pairs.json`, by bare mxid) link again by themselves next time
//! (the app's `sameroom-auto` pref, default on); only the lower identity of a remembered pair asks.
//!
//! Threads: the state sits behind one mutex. The GStreamer capture thread feeds [`note_raw`]; packets go
//! through an outbox a 50 ms tokio task ([`spawn_sender`]) publishes (never a LiveKit call under the lock);
//! GStreamer mute elements are touched only after the lock is released (`call::reconcile_same_room`).

pub mod elect;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use elect::*;
use livekit::id::ParticipantIdentity;

pub const LINK_TOPIC: &str = "tinhat.sr.link";
pub const LVL_TOPIC: &str = "tinhat.sr.lvl";
const ASK_TTL: Duration = Duration::from_secs(45); // 11.3 QOL: 45 s (was 20) — the card waits while someone looks away
const PEER_SILENT: Duration = Duration::from_secs(12);
const FRAME: usize = 480; // 10 ms at 48 kHz
/// 1 s of windows after linking before the election counts (the floors need a moment; the rig flipped once at link time).
const WARM_WINDOWS: u32 = 20;
const WIN_FRAMES: usize = 5; // 50 ms

struct Peer {
    at: Instant,
    cap: bool,
    m: Vec<String>,
    mic: String,
    spk: bool,
}

#[derive(Default)]
struct State {
    on: bool,
    me: String,
    present: BTreeSet<String>,      // remote identities in the call (not the recorder)
    joined_at: HashMap<String, Instant>,
    auto_tried: HashSet<String>,
    group: BTreeSet<String>,        // peers (not me)
    elect: ElectState,
    levels: BTreeMap<String, Level>, // per peer: the max of its packets from the last HOLD_MS (t = the latest)
    recent: HashMap<String, Vec<Level>>,
    mine_recent: Vec<Level>,
    warm: u32,
    mine: Level,
    floor: FloorTracker,
    sumsq: f64,
    nsamp: usize,
    frames: Vec<f32>,
    raw_seen: Option<Instant>,
    raw_ok: bool,
    peers: HashMap<String, Peer>,
    ask: Option<(String, Instant, Vec<String>)>,
    pending: Option<(String, Instant)>,
    spk: bool,
    last_tick: Option<Instant>,
    last_holder_poke: Option<Instant>,
    outbox: VecDeque<livekit::DataPacket>,
    snapshot: String,
    pairs: Option<HashMap<String, (bool, u64)>>,
    store: Option<PathBuf>,
    auto: bool,
    muted: bool, // the user's own mute (the gate yields to it)
    started: Option<Instant>,
    test_link: Option<String>, // TINHAT_SR_LINK=<identity|mxid>: ask them 8 s into the call (the rig)
    test_link_done: bool,
    test_accept: bool, // TINHAT_SR_AUTOACCEPT=1: say yes to any ask (the rig)
}

static ON: AtomicBool = AtomicBool::new(false);
static SESSION: AtomicU64 = AtomicU64::new(0);
static RX_POKE: AtomicBool = AtomicBool::new(false);
static TX_LIVE: AtomicBool = AtomicBool::new(true);
static LINKED: AtomicBool = AtomicBool::new(false);
static SPK_OFF: AtomicBool = AtomicBool::new(false);

fn st() -> &'static Mutex<State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(State { spk: true, auto: true, ..Default::default() }))
}

fn epoch() -> Instant {
    static E: OnceLock<Instant> = OnceLock::new();
    *E.get_or_init(Instant::now)
}
fn now_ms() -> u64 {
    epoch().elapsed().as_millis() as u64
}
fn mxid_of(identity: &str) -> &str {
    // "@u:server:DEVICE" → "@u:server"
    match identity.rfind(':') {
        Some(i) if identity[..i].contains(':') => &identity[..i],
        _ => identity,
    }
}
fn is_recorder(identity: &str) -> bool {
    crate::hear::is_recorder(identity) // the recorder the user's server announces (hear::set_recorder)
}
fn members(s: &State) -> Vec<String> {
    let mut m: Vec<String> = s.group.iter().cloned().collect();
    m.push(s.me.clone());
    m.sort();
    m
}

// ── packets ──────────────────────────────────────────────────────────────────────────────────

fn packet(topic: &str, payload: String, reliable: bool, to: Vec<String>) -> livekit::DataPacket {
    livekit::DataPacket {
        payload: payload.into_bytes(),
        topic: Some(topic.to_string()),
        reliable,
        destination_identities: to.into_iter().map(ParticipantIdentity).collect(),
    }
}
fn link_packet(op: &str, g: &str, m: &[String], auto: bool, to: &str) -> livekit::DataPacket {
    let mut v = serde_json::json!({ "v": 1, "op": op, "g": g, "m": m });
    if auto {
        v["auto"] = serde_json::Value::Bool(true);
    }
    packet(LINK_TOPIC, v.to_string(), true, vec![to.to_string()])
}

// ── lifecycle (call.rs) ──────────────────────────────────────────────────────────────────────

pub(crate) fn session_start(me: &str) {
    let mut s = st().lock().unwrap();
    let (store, auto, pairs) = (s.store.take(), s.auto, s.pairs.take());
    *s = State { on: true, me: me.to_string(), spk: true, store, auto, pairs, ..Default::default() };
    s.started = Some(Instant::now());
    s.test_link = std::env::var("TINHAT_SR_LINK").ok().filter(|v| !v.is_empty());
    s.test_accept = std::env::var("TINHAT_SR_AUTOACCEPT").map(|v| v == "1").unwrap_or(false);
    if s.test_link.is_some() || s.test_accept {
        eprintln!("[sr] TEST HOOKS: link={:?} autoaccept={}", s.test_link, s.test_accept);
    }
    SESSION.fetch_add(1, Ordering::SeqCst);
    ON.store(true, Ordering::Relaxed);
    TX_LIVE.store(true, Ordering::Relaxed);
    LINKED.store(false, Ordering::Relaxed);
    SPK_OFF.store(false, Ordering::Relaxed);
    s.snapshot = String::new();
    refresh_snapshot(&mut s);
}

pub(crate) fn session_end() {
    SESSION.fetch_add(1, Ordering::SeqCst);
    ON.store(false, Ordering::Relaxed);
    TX_LIVE.store(true, Ordering::Relaxed);
    LINKED.store(false, Ordering::Relaxed);
    SPK_OFF.store(false, Ordering::Relaxed);
    let mut s = st().lock().unwrap();
    let (store, auto, pairs) = (s.store.take(), s.auto, s.pairs.take());
    *s = State { spk: true, store, auto, pairs, ..Default::default() };
    refresh_snapshot(&mut s);
}

pub(crate) fn participant_joined(identity: &str) {
    if is_recorder(identity) {
        return;
    }
    let mut s = st().lock().unwrap();
    if !s.on {
        return;
    }
    s.present.insert(identity.to_string());
    s.joined_at.insert(identity.to_string(), Instant::now());
}

pub(crate) fn participant_left(identity: &str) {
    let mut s = st().lock().unwrap();
    if !s.on {
        return;
    }
    s.present.remove(identity);
    s.joined_at.remove(identity);
    s.peers.remove(identity);
    s.levels.remove(identity);
    s.recent.remove(identity);
    if s.ask.as_ref().map(|a| a.0 == identity).unwrap_or(false) {
        s.ask = None;
    }
    if s.pending.as_ref().map(|p| p.0 == identity).unwrap_or(false) {
        s.pending = None;
    }
    let was = s.group.remove(identity);
    if was && s.group.is_empty() {
        let ids = leave_group_locked(&mut s, &format!("{identity} left the call"));
        drop(s);
        crate::call::reconcile_same_room(&ids);
        crate::call::reconcile_auto_mute();
        return;
    }
    if was {
        eprintln!("[sr] {identity} left the call — still linked with {}", s.group.iter().cloned().collect::<Vec<_>>().join(", "));
    }
    refresh_snapshot(&mut s);
    if was {
        drop(s);
        crate::call::reconcile_same_room(&[identity.to_string()]);
    }
}

/// The user's own mute (publication mute / push-to-talk): the gate never un-mutes a muted mic.
pub(crate) fn set_mic_muted(muted: bool) {
    let mut s = st().lock().unwrap();
    s.muted = muted;
    apply_gate(&s);
}

// ── what call.rs / hear.rs read ──────────────────────────────────────────────────────────────

/// The mic feeder: send real frames (true) or silence (false) — a linked peer holds the room mic.
pub(crate) fn tx_live() -> bool {
    TX_LIVE.load(Ordering::Relaxed)
}
/// A linked peer: we hear them in person, so Tin Hat must not play them (any category).
pub fn is_linked_peer(identity: &str) -> bool {
    if !LINKED.load(Ordering::Relaxed) {
        return false;
    }
    st().lock().map(|s| s.group.contains(identity)).unwrap_or(false)
}
/// "Hear the call through <peer>'s speakers": my playback is cut (mic stays live, receipts unaffected).
pub fn speakers_off() -> bool {
    SPK_OFF.load(Ordering::Relaxed)
}
/// The JSON fragment appended to our `tinhat.rx` (`,"cap":["sr"]` + `,"sr":{…}` while linked).
pub(crate) fn rx_extra() -> String {
    let s = st().lock().unwrap();
    if !s.on || s.group.is_empty() {
        return ",\"cap\":[\"sr\"]".to_string();
    }
    let m = members(&s);
    let v = serde_json::json!({ "g": group_id_of(&m), "m": m, "mic": s.elect.holder, "spk": s.spk });
    format!(",\"cap\":[\"sr\"],\"sr\":{v}")
}
/// hear.rs: a link / speakers change wants a receipt now (≤ 250 ms) instead of at the 3 s cadence.
pub(crate) fn take_rx_poke() -> bool {
    RX_POKE.swap(false, Ordering::Relaxed)
}
/// The UI snapshot (call_ctl mirrors it): `{"linked":[…],"holder":"","live":true,"ask":{from,at}|null,
/// "pending":"","spk":true,"groups":{id:{m:[…],mic}},"rawOk":bool}` ("{}" outside a call).
pub fn state_json() -> String {
    st().lock().map(|s| s.snapshot.clone()).unwrap_or_else(|_| "{}".to_string())
}

fn refresh_snapshot(s: &mut State) {
    if !s.on {
        s.snapshot = "{}".to_string();
        return;
    }
    let mut groups = serde_json::Map::new();
    for (id, p) in &s.peers {
        if p.m.len() >= 2 {
            groups.insert(id.clone(), serde_json::json!({ "m": p.m, "mic": p.mic }));
        }
    }
    if !s.group.is_empty() {
        let m = members(s);
        for id in &m {
            groups.insert(id.clone(), serde_json::json!({ "m": m, "mic": s.elect.holder }));
        }
    }
    let linked: Vec<&String> = s.group.iter().collect();
    let v = serde_json::json!({
        "linked": linked,
        "holder": if s.group.is_empty() { "" } else { s.elect.holder.as_str() },
        "live": s.group.is_empty() || s.elect.holder == s.me,
        "ask": s.ask.as_ref().map(|a| serde_json::json!({ "from": a.0, "at": a.1.elapsed().as_millis() as u64 })),
        "pending": s.pending.as_ref().map(|p| p.0.clone()).unwrap_or_default(),
        "spk": s.spk,
        "groups": groups,
        "rawOk": s.raw_ok,
    });
    s.snapshot = v.to_string();
}

// ── peers' tinhat.rx ───────────────────────────────────────────────────────────────────────────

pub(crate) fn note_peer_rx(who: &str, text: &str) {
    if !ON.load(Ordering::Relaxed) || who.is_empty() || is_recorder(who) {
        return;
    }
    let v: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => return,
    };
    let cap = v["cap"].as_array().map(|a| a.iter().any(|x| x.as_str() == Some("sr"))).unwrap_or(false);
    let sr = &v["sr"];
    let m: Vec<String> = sr["m"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).filter(|x| x.len() <= 256).take(8).map(String::from).collect()).unwrap_or_default();
    let p = Peer { at: Instant::now(), cap, m, mic: sr["mic"].as_str().unwrap_or("").to_string(), spk: sr["spk"].as_bool().unwrap_or(true) };
    let mut s = st().lock().unwrap();
    s.present.insert(who.to_string());
    s.peers.insert(who.to_string(), p);
    refresh_snapshot(&mut s);
}

fn peer_supports(s: &State, identity: &str) -> Option<bool> {
    s.peers.get(identity).map(|p| p.cap)
}

// ── linking ──────────────────────────────────────────────────────────────────────────────────

fn resolve(s: &State, target: &str) -> Option<String> {
    if s.present.contains(target) {
        return Some(target.to_string());
    }
    s.present.iter().find(|i| mxid_of(i) == target).cloned()
}

/// Ask `target` (identity or mxid) to link up. Ok, or the reason.
pub fn link_with(target: &str) -> Result<(), String> {
    link_with_inner(target, false)
}
fn link_with_inner(target: &str, auto: bool) -> Result<(), String> {
    let mut s = st().lock().unwrap();
    eprintln!("[sr] link_with({target}) auto={auto} on={} present={}", s.on, s.present.len());
    if !s.on {
        return Err("You're not in a call".to_string());
    }
    let id = resolve(&s, target).ok_or_else(|| "They're not in this call".to_string())?;
    if s.group.contains(&id) {
        return Ok(());
    }
    if peer_supports(&s, &id) == Some(false) {
        return Err("They need to update Tin Hat to link up".to_string());
    }
    let mut m = members(&s);
    m.push(id.clone());
    let g = group_id_of(&m);
    let mine = members(&s);
    s.pending = Some((id.clone(), Instant::now()));
    s.outbox.push_back(link_packet("ask", &g, &mine, auto, &id));
    eprintln!("[sr] asked {id} to link up{}", if auto { " (remembered pair)" } else { "" });
    refresh_snapshot(&mut s);
    Ok(())
}

/// Join `peer_ids` into my group (the caller publishes nothing here). Returns the ids whose playback
/// must be reconciled by call.rs (after the lock is released).
fn join_locked(s: &mut State, peer_ids: &[String]) -> Vec<String> {
    for id in peer_ids {
        if *id != s.me && s.present.contains(id) {
            s.group.insert(id.clone());
        }
    }
    if s.group.is_empty() {
        return Vec::new();
    }
    s.elect = ElectState::default();
    s.levels.clear();
    s.recent.clear();
    s.mine_recent.clear();
    s.warm = 0;
    s.floor.reset();
    s.frames.clear();
    s.sumsq = 0.0;
    s.nsamp = 0;
    let now = now_ms();
    let mut pairs = load_pairs(s);
    let group: Vec<String> = s.group.iter().cloned().collect();
    for id in &group {
        let spk = pairs.get(mxid_of(id)).map(|p| p.0).unwrap_or(true);
        pairs.insert(mxid_of(id).to_string(), (spk, now));
    }
    // the remembered speakers choice: off only if every linked peer was saved that way
    s.spk = !group.iter().all(|id| pairs.get(mxid_of(id)).map(|p| !p.0).unwrap_or(false));
    save_pairs(s, pairs);
    LINKED.store(true, Ordering::Relaxed);
    SPK_OFF.store(!s.spk, Ordering::Relaxed);
    RX_POKE.store(true, Ordering::Relaxed);
    eprintln!("[sr] linked with {} — room mic election on", group.join(", "));
    run_election(s);
    refresh_snapshot(s);
    group
}

fn leave_group_locked(s: &mut State, why: &str) -> Vec<String> {
    let ids: Vec<String> = s.group.iter().cloned().collect();
    s.group.clear();
    s.elect = ElectState::default();
    s.levels.clear();
    s.spk = true;
    s.raw_ok = false;
    LINKED.store(false, Ordering::Relaxed);
    SPK_OFF.store(false, Ordering::Relaxed);
    TX_LIVE.store(true, Ordering::Relaxed);
    RX_POKE.store(true, Ordering::Relaxed);
    eprintln!("[sr] unlinked ({why})");
    refresh_snapshot(s);
    ids
}

pub fn answer_ask(yes: bool) {
    let mut s = st().lock().unwrap();
    let a = match s.ask.take() {
        Some(a) => a,
        None => {
            refresh_snapshot(&mut s);
            return;
        }
    };
    if !s.on {
        return;
    }
    let mut all = members(&s);
    all.extend(a.2.iter().cloned());
    all.push(a.0.clone());
    let g = group_id_of(&all);
    let mine = members(&s);
    s.outbox.push_back(link_packet(if yes { "yes" } else { "no" }, &g, &mine, false, &a.0));
    if yes {
        let mut ids = vec![a.0.clone()];
        ids.extend(a.2.iter().cloned());
        let touched = join_locked(&mut s, &ids);
        drop(s);
        crate::call::reconcile_same_room(&touched);
        crate::call::reconcile_auto_mute();
    } else {
        refresh_snapshot(&mut s);
    }
}

/// Take back my "link up?" ask (the pending pill's Cancel): the peer's card goes away ("no" from the asker).
pub fn cancel_ask() {
    let mut s = st().lock().unwrap();
    let who = match s.pending.take() {
        Some(p) => p.0,
        None => return,
    };
    if s.on {
        s.outbox.push_back(link_packet("no", "", &[], false, &who));
    }
    eprintln!("[sr] cancelled the ask to {who}");
    refresh_snapshot(&mut s);
}

pub fn unlink() {
    let mut s = st().lock().unwrap();
    if !s.on || s.group.is_empty() {
        return;
    }
    for id in s.group.clone() {
        s.outbox.push_back(link_packet("unlink", "", &[], false, &id));
    }
    let ids = leave_group_locked(&mut s, "you unlinked");
    drop(s);
    crate::call::reconcile_same_room(&ids);
    crate::call::reconcile_auto_mute();
}

/// Hear the call on this device (true) or through a linked peer's speakers (false). Remembered per pair.
pub fn set_hear_through(on_this_device: bool) {
    let mut s = st().lock().unwrap();
    if !s.on || s.group.is_empty() {
        return;
    }
    s.spk = on_this_device;
    SPK_OFF.store(!on_this_device, Ordering::Relaxed);
    RX_POKE.store(true, Ordering::Relaxed);
    let mut pairs = load_pairs(&s);
    let now = now_ms();
    for id in s.group.clone() {
        pairs.insert(mxid_of(&id).to_string(), (on_this_device, now));
    }
    save_pairs(&mut s, pairs);
    refresh_snapshot(&mut s);
    drop(s);
    crate::call::reconcile_auto_mute();
}

pub(crate) fn handle_link(who: &str, text: &str) {
    if !ON.load(Ordering::Relaxed) || who.is_empty() {
        return;
    }
    let v: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => return,
    };
    let op = v["op"].as_str().unwrap_or("");
    let m: Vec<String> = v["m"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).filter(|x| x.len() <= 256).take(8).map(String::from).collect()).unwrap_or_default();
    let mut s = st().lock().unwrap();
    match op {
        "ask" => {
            if s.group.contains(who) {
                let g = group_id_of(&members(&s));
                let mine = members(&s);
                s.outbox.push_back(link_packet("yes", &g, &mine, false, who));
                return;
            }
            let remembered = load_pairs(&s).contains_key(mxid_of(who));
            if (v["auto"].as_bool() == Some(true) && remembered && s.auto) || s.test_accept {
                let mut all = members(&s);
                all.push(who.to_string());
                all.extend(m.iter().cloned());
                let g = group_id_of(&all);
                let mine = members(&s);
                s.outbox.push_back(link_packet("yes", &g, &mine, false, who));
                let mut ids = vec![who.to_string()];
                ids.extend(m);
                let touched = join_locked(&mut s, &ids);
                drop(s);
                crate::call::reconcile_same_room(&touched);
                crate::call::reconcile_auto_mute();
                return;
            }
            s.ask = Some((who.to_string(), Instant::now(), m));
            eprintln!("[sr] {who} asks to link up (same room)");
            refresh_snapshot(&mut s);
        }
        "yes" => {
            let pending = s.pending.as_ref().map(|p| p.0 == who).unwrap_or(false);
            if !pending && s.group.is_empty() {
                // a late yes (I cancelled or it expired): tell them, so they don't stay linked on their own
                s.outbox.push_back(link_packet("unlink", "", &[], false, who));
                return;
            }
            if pending {
                s.pending = None;
            }
            let mut ids = vec![who.to_string()];
            ids.extend(m);
            let touched = join_locked(&mut s, &ids);
            drop(s);
            crate::call::reconcile_same_room(&touched);
            crate::call::reconcile_auto_mute();
        }
        "no" => {
            if s.pending.as_ref().map(|p| p.0 == who).unwrap_or(false) {
                s.pending = None;
                eprintln!("[sr] {who} didn't link up");
                refresh_snapshot(&mut s);
            }
            // the asker took it back (their Cancel): the card goes away
            if s.ask.as_ref().map(|a| a.0 == who).unwrap_or(false) {
                s.ask = None;
                eprintln!("[sr] {who} took back the ask");
                refresh_snapshot(&mut s);
            }
        }
        "unlink" => {
            if !s.group.remove(who) {
                return;
            }
            s.levels.remove(who);
            if s.group.is_empty() {
                let mut ids = leave_group_locked(&mut s, &format!("{who} unlinked"));
                ids.push(who.to_string()); // their playback comes back too
                drop(s);
                crate::call::reconcile_same_room(&ids);
                crate::call::reconcile_auto_mute();
            } else {
                refresh_snapshot(&mut s);
                drop(s);
                crate::call::reconcile_same_room(&[who.to_string()]);
            }
        }
        _ => {}
    }
}

pub(crate) fn handle_lvl(who: &str, text: &str) {
    if !ON.load(Ordering::Relaxed) || who.is_empty() {
        return;
    }
    let v: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => return,
    };
    let snr = v["s"].as_f64().filter(|x| x.is_finite()).map(|x| x.round().clamp(0.0, SNR_MAX_DB as f64) as i32).unwrap_or(0);
    let mut s = st().lock().unwrap();
    if !s.group.contains(who) {
        return;
    }
    let now = now_ms();
    let rec = s.recent.entry(who.to_string()).or_default();
    rec.retain(|l| now.saturating_sub(l.t) <= HOLD_MS);
    rec.push(Level { s: snr, t: now });
    let held = rec.iter().map(|l| l.s).max().unwrap_or(snr);
    s.levels.insert(who.to_string(), Level { s: held, t: now });
    // a peer's level alone can move the election (we may be between our own windows)
    if s.last_tick.map(|t| t.elapsed() > Duration::from_millis(60)).unwrap_or(true) {
        run_election(&mut s);
    }
}

// ── the raw mic + the election ───────────────────────────────────────────────────────────────

/// One RAW capture buffer (F32 48 k mono, before any processing) — the capture's streaming thread.
pub(crate) fn note_raw(buf: &[f32]) {
    if !LINKED.load(Ordering::Relaxed) || buf.is_empty() {
        return;
    }
    let mut s = st().lock().unwrap();
    if s.group.is_empty() {
        return;
    }
    s.raw_seen = Some(Instant::now());
    s.raw_ok = true;
    for &x in buf {
        s.sumsq += (x as f64) * (x as f64);
        s.nsamp += 1;
        if s.nsamp >= FRAME {
            let rms = (s.sumsq / s.nsamp as f64).sqrt() as f32;
            s.sumsq = 0.0;
            s.nsamp = 0;
            s.frames.push(rms);
            if s.frames.len() >= WIN_FRAMES {
                on_window(&mut s);
            }
        }
    }
}

/// One frame the call is fed (after all processing), every 10 ms — the election's fallback level when the raw
/// probe is silent (the PipeWire echo-cancel tier captures a processed source).
pub(crate) fn note_sent_rms(rms: f32) {
    if !LINKED.load(Ordering::Relaxed) {
        return;
    }
    let mut s = st().lock().unwrap();
    if s.group.is_empty() || s.raw_seen.map(|t| t.elapsed() < Duration::from_millis(500)).unwrap_or(false) {
        return;
    }
    s.raw_ok = false;
    s.frames.push(rms);
    if s.frames.len() >= WIN_FRAMES {
        on_window(&mut s);
    }
}

fn on_window(s: &mut State) {
    let db = window_db(&s.frames);
    s.frames.clear();
    let snr = s.floor.push(db);
    let t = now_ms();
    if s.warm < WARM_WINDOWS {
        s.warm += 1;
    }
    // my own level gets the same HOLD_MS max as the peers' (symmetric decay at the end of a sentence — without
    // it the rig showed the holder flipping to the peer in every speech gap)
    s.mine_recent.retain(|l| t.saturating_sub(l.t) <= HOLD_MS);
    s.mine_recent.push(Level { s: snr, t });
    let held = s.mine_recent.iter().map(|l| l.s).max().unwrap_or(snr);
    s.mine = Level { s: held, t };
    let to: Vec<String> = s.group.iter().cloned().collect();
    s.outbox.push_back(packet(LVL_TOPIC, format!("{{\"s\":{snr},\"t\":{t}}}"), false, to));
    run_election(s);
}

fn run_election(s: &mut State) {
    s.last_tick = Some(Instant::now());
    // before the warm-up nobody can challenge: the default holder (lowest identity) keeps the mic
    let mut levels = if s.warm >= WARM_WINDOWS { s.levels.clone() } else { BTreeMap::new() };
    if s.warm >= WARM_WINDOWS {
        levels.insert(s.me.clone(), s.mine);
    }
    let r = elect_tick(&s.elect, &s.me, &members(s), &levels, now_ms());
    let changed = r.holder != s.elect.holder;
    s.elect = ElectState { holder: r.holder, wins: r.wins };
    if changed {
        eprintln!("[sr] room mic → {}", if s.elect.holder == s.me { "me".to_string() } else { s.elect.holder.clone() });
        let poke = s.last_holder_poke.map(|t| t.elapsed() > Duration::from_millis(1500)).unwrap_or(true);
        if poke {
            s.last_holder_poke = Some(Instant::now());
            RX_POKE.store(true, Ordering::Relaxed);
        }
        refresh_snapshot(s);
    }
    apply_gate(s);
}

/// Send silence while a linked peer holds the room mic. The user's own mute always wins (the track is
/// disabled then anyway); unlinked = always live.
fn apply_gate(s: &State) {
    let live = s.group.is_empty() || s.elect.holder == s.me;
    TX_LIVE.store(live, Ordering::Relaxed);
}

// ── remembered pairs (the app sets the store path + the auto pref) ───────────────────────────

pub fn set_store_path(p: PathBuf) {
    let mut s = st().lock().unwrap();
    s.store = Some(p);
    s.pairs = None;
}
pub fn set_auto(on: bool) {
    st().lock().unwrap().auto = on;
}
pub fn auto_link() -> bool {
    st().lock().map(|s| s.auto).unwrap_or(true)
}
fn load_pairs(s: &State) -> HashMap<String, (bool, u64)> {
    if let Some(p) = &s.pairs {
        return p.clone();
    }
    let mut out = HashMap::new();
    if let Some(path) = &s.store {
        if let Ok(text) = std::fs::read_to_string(path) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                if let Some(o) = v.as_object() {
                    for (k, e) in o {
                        if k.starts_with('@') && k.len() <= 256 {
                            out.insert(k.clone(), (e["spk"].as_bool().unwrap_or(true), e["at"].as_u64().unwrap_or(0)));
                        }
                    }
                }
            }
        }
    }
    out
}
fn save_pairs(s: &mut State, pairs: HashMap<String, (bool, u64)>) {
    if let Some(path) = &s.store {
        let mut o = serde_json::Map::new();
        for (k, (spk, at)) in &pairs {
            o.insert(k.clone(), serde_json::json!({ "spk": spk, "at": at }));
        }
        if let Some(d) = path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if let Err(e) = std::fs::write(path, serde_json::Value::Object(o).to_string()) {
            eprintln!("[sr] remembered pairs not saved: {e}");
        }
    }
    s.pairs = Some(pairs);
}
/// `{"<mxid>":{"spk":bool,"at":ms},…}` — Settings lists them.
pub fn pairs_json() -> String {
    let s = st().lock().unwrap();
    let pairs = load_pairs(&s);
    let mut o = serde_json::Map::new();
    for (k, (spk, at)) in &pairs {
        o.insert(k.clone(), serde_json::json!({ "spk": spk, "at": at }));
    }
    serde_json::Value::Object(o).to_string()
}
pub fn forget_pair(mxid: &str) {
    let mut s = st().lock().unwrap();
    let mut pairs = load_pairs(&s);
    if pairs.remove(mxid).is_some() {
        save_pairs(&mut s, pairs);
    }
}

// ── the 50 ms sender task (beside hear's analyzer; exits with the session) ───────────────────

pub(crate) fn spawn_sender(lp: livekit::participant::LocalParticipant) -> tokio::task::JoinHandle<()> {
    let my_session = SESSION.load(Ordering::SeqCst);
    eprintln!("[sr] sender task on (session {my_session})");
    tokio::spawn(async move {
        let mut iv = tokio::time::interval(Duration::from_millis(50));
        iv.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut n: u64 = 0;
        loop {
            iv.tick().await;
            if !ON.load(Ordering::Relaxed) || SESSION.load(Ordering::SeqCst) != my_session {
                eprintln!("[sr] sender task off (on={} session {} vs {my_session})", ON.load(Ordering::Relaxed), SESSION.load(Ordering::SeqCst));
                break;
            }
            n += 1;
            let mut touched: Vec<String> = Vec::new();
            let mut reconcile_cut = false;
            let pkts: Vec<livekit::DataPacket> = {
                let mut s = st().lock().unwrap();
                let now = Instant::now();
                if n % 20 == 0 {
                    // 1 s: ask/pending expiry, a linked peer whose receipts stopped listing us, auto-link
                    if s.ask.as_ref().map(|a| now.duration_since(a.1) > ASK_TTL).unwrap_or(false) {
                        s.ask = None;
                        refresh_snapshot(&mut s);
                    }
                    if s.pending.as_ref().map(|p| now.duration_since(p.1) > ASK_TTL).unwrap_or(false) {
                        let who = s.pending.take().map(|p| p.0).unwrap_or_default();
                        eprintln!("[sr] {who} didn't answer");
                        refresh_snapshot(&mut s);
                    }
                    let me = s.me.clone();
                    let gone: Vec<String> = s
                        .group
                        .iter()
                        .filter(|id| {
                            s.peers
                                .get(*id)
                                .map(|p| now.duration_since(p.at) < PEER_SILENT && now.duration_since(p.at) > Duration::from_secs(6) && !p.m.is_empty() && !p.m.contains(&me))
                                .unwrap_or(false)
                        })
                        .cloned()
                        .collect();
                    for id in &gone {
                        s.group.remove(id);
                        eprintln!("[sr] {id} no longer lists us as linked");
                        touched.push(id.clone());
                    }
                    if !gone.is_empty() && s.group.is_empty() {
                        touched.extend(leave_group_locked(&mut s, "no peers left"));
                        reconcile_cut = true;
                    } else if !gone.is_empty() {
                        refresh_snapshot(&mut s);
                    }
                    // the rig: TINHAT_SR_LINK asks once, 8 s into the call
                    if !s.test_link_done && s.pending.is_none() && s.group.is_empty() {
                        if let Some(t) = s.test_link.clone() {
                            if s.started.map(|t0| now.duration_since(t0) > Duration::from_secs(8)).unwrap_or(false) {
                                s.test_link_done = true;
                                drop(s);
                                if let Err(e) = link_with_inner(&t, false) {
                                    eprintln!("[sr] TEST link_with({t}) failed: {e}");
                                }
                                s = st().lock().unwrap();
                            }
                        }
                    }
                    if n % 100 == 0 && !s.group.is_empty() {
                        // 5 s: what each linked peer's mic track delivers here (the rig's "gated = silent" oracle)
                        let lv: Vec<String> = s.group.iter().map(|id| format!("{id}={:.3}", crate::call::track_level_of(id, "mic"))).collect();
                        eprintln!("[sr] linked peer levels: {} (room mic: {})", lv.join(" "), if s.elect.holder == s.me { "me".to_string() } else { s.elect.holder.clone() });
                    }
                    if s.auto && s.pending.is_none() {
                        let me = s.me.clone();
                        let pairs = load_pairs(&s);
                        let cands: Vec<String> = s
                            .present
                            .iter()
                            .filter(|id| !s.group.contains(*id) && !s.auto_tried.contains(*id) && me.as_str() < id.as_str())
                            .filter(|id| pairs.contains_key(mxid_of(id)))
                            .filter(|id| s.joined_at.get(*id).map(|t| now.duration_since(*t) > Duration::from_millis(3500)).unwrap_or(true))
                            .filter(|id| peer_supports(&s, id) != Some(false))
                            .cloned()
                            .collect();
                        if let Some(id) = cands.into_iter().next() {
                            s.auto_tried.insert(id.clone());
                            drop(s);
                            let _ = link_with_inner(&id, true);
                            s = st().lock().unwrap();
                        }
                    }
                }
                // the election must keep running while our own tap is quiet (a peer may be speaking)
                if !s.group.is_empty() && s.last_tick.map(|t| now.duration_since(t) > Duration::from_millis(150)).unwrap_or(true) {
                    run_election(&mut s);
                }
                s.outbox.drain(..).collect()
            };
            if !touched.is_empty() {
                crate::call::reconcile_same_room(&touched);
            }
            if reconcile_cut {
                crate::call::reconcile_auto_mute();
            }
            for pkt in pkts {
                let lp2 = lp.clone();
                tokio::spawn(async move {
                    let _ = tokio::time::timeout(Duration::from_secs(2), lp2.publish_data(pkt)).await;
                });
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mxid_of_strips_the_device() {
        assert_eq!(mxid_of("@a:example.com:DEV"), "@a:example.com");
        assert_eq!(mxid_of("@a:example.com"), "@a:example.com");
    }

    #[test]
    fn link_packets_are_the_web_shape() {
        let p = link_packet("ask", "@a:x+@b:x", &["@a:x".to_string()], true, "@b:x");
        let v: serde_json::Value = serde_json::from_slice(&p.payload).unwrap();
        assert_eq!(v["v"], 1);
        assert_eq!(v["op"], "ask");
        assert_eq!(v["g"], "@a:x+@b:x");
        assert_eq!(v["auto"], true);
        assert!(p.reliable);
        assert_eq!(p.topic.as_deref(), Some(LINK_TOPIC));
    }

    #[test]
    fn rx_extra_outside_a_call_is_the_capability_only() {
        assert_eq!(rx_extra(), ",\"cap\":[\"sr\"]");
    }
}
