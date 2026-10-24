//! Widgets (modular UI, step 2): per-person call stats for the Call stats widget.
//!
//! Contract: `WIDGETS.md` §5–§6 + WIDGETS-UI.md "Call stats". The maths and the wire
//! format are the shared logic of `crates/widgets` (a port of the web's reference, proven by the shared vectors); this
//! module is the I/O around it. Three sources, one snapshot for the UI:
//!
//! * **incoming** — what WE receive from each remote person, measured here from libwebrtc's receiver stats
//!   (`RemoteTrack::get_stats`: the track's `inbound-rtp`, its codec, the receiving transport's candidate-pair round
//!   trip) → `quality::rx_numbers` over the last sample. Until now the app only read what it SENDS
//!   (`call::publish_stats_json`).
//! * **outgoing** — how THEY receive US: their `tinhat.rxq` report (unreliable, targeted; `rxq::decode_rxq` + the
//!   per-sender `RxqGate`). Apps without it ignore the topic; `tinhat.rx` (hear.rs) is unchanged.
//! * **connection quality** — LiveKit's own ConnectionQuality per participant (the SFU's view), the person's dot.
//!
//! Cadence (WIDGETS.md §5, GPU.md §2.8): sampled every 1 s while the app says its window is focused, every 5 s
//! otherwise (hidden too — the report to the others is their data and keeps going every 5 s); the UI snapshot is
//! handed to the app after every sample, except while the window is hidden (paused; the latest snapshot is handed over
//! the moment it shows again). Everything a peer sends is validated, limited and used for display only.
//!
//! Never in the Memories recorder (it has no UI and must not report): `call.rs` does not start this module there, and
//! the recorder's identity is never listed or reported to.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use livekit::id::ParticipantIdentity;
use livekit::track::{RemoteTrack, TrackSource};
use livekit::webrtc::stats::RtcStats;
use serde_json::{json, Value};
use tinhat_widgets::quality::{self, cq_band, rx_numbers, worst_band, Band, Kind, RxSample, TrackNumbers};
use tinhat_widgets::rxq::{self, RxqGate};

pub use tinhat_widgets::rxq::RXQ_TOPIC;

/// Incoming-bitrate samples kept per person for the sparkline.
pub const SPARK_LEN: usize = quality::SPARK_SAMPLES;
/// Sample period while the window is focused / otherwise.
pub const FOCUSED_MS: u64 = 1000;
pub const UNFOCUSED_MS: u64 = 5000;

/// The report key of a LiveKit track source (None = not reported: share audio, soundboard / music).
pub fn kind_of(source: TrackSource) -> Option<Kind> {
    match source {
        TrackSource::Microphone => Some(Kind::A),
        TrackSource::Camera => Some(Kind::V),
        TrackSource::Screenshare => Some(Kind::S),
        _ => None,
    }
}

/// LiveKit ConnectionQuality → its name (the snapshot keeps the name; `quality::cq_band` makes the dot).
pub fn cq_name(q: livekit::participant::ConnectionQuality) -> &'static str {
    use livekit::participant::ConnectionQuality as Q;
    match q {
        Q::Excellent => "excellent",
        Q::Good => "good",
        Q::Poor => "poor",
        Q::Lost => "lost",
    }
}

/// Milliseconds on a monotonic clock (the gate and the report ages).
fn now_ms() -> f64 {
    static T0: OnceLock<Instant> = OnceLock::new();
    T0.get_or_init(Instant::now).elapsed().as_secs_f64() * 1000.0
}

// ── state ───────────────────────────────────────────────────────────────────────────────────────────────────────

struct Tr {
    who: String,
    k: Kind,
    track: RemoteTrack,
    /// the latest raw sample (the display's baseline for the next one)
    prev: Option<RxSample>,
    cur: Option<TrackNumbers>,
    /// the raw sample at our last tinhat.rxq report: each report's numbers are measured from THIS baseline, every
    /// RXQ_SEND_MS, independent of the display samples and the focus (WIDGETS.md §10.2 v1.3; the web's rxqPrev)
    rep: Option<RxSample>,
}

#[derive(Default)]
struct Peer {
    cq: Option<&'static str>,
    /// (received at, their report)
    rxq: Option<(f64, rxq::Rxq)>,
    /// any report accepted from them in this call (never = an older app, after a while)
    out_ever: bool,
    /// when they appeared in the call (monotonic ms)
    since: f64,
    spark: VecDeque<f64>,
}

fn peer<'a>(s: &'a mut St, who: &str) -> &'a mut Peer {
    s.peers.entry(who.to_string()).or_insert_with(|| Peer { since: now_ms(), ..Peer::default() })
}

#[derive(Default)]
struct St {
    on: bool,
    me: String,
    my_cq: Option<&'static str>,
    present: HashSet<String>,
    tracks: HashMap<String, Tr>,
    peers: BTreeMap<String, Peer>,
    gate: RxqGate,
    /// Sample counter (the UI steps the sparkline once per sample).
    seq: u64,
    /// Period of the last sample, ms.
    period: u64,
    snapshot: String,
}

fn st() -> &'static Mutex<St> {
    static S: OnceLock<Mutex<St>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(St::default()))
}

static SESSION: AtomicU64 = AtomicU64::new(0);
/// 0 hidden, 1 unfocused, 2 focused (the app's window state).
static VIEW: AtomicU8 = AtomicU8::new(2);
static TASK: OnceLock<Mutex<Option<tokio::task::JoinHandle<()>>>> = OnceLock::new();

type Hook = Arc<dyn Fn() + Send + Sync>;
fn hook_cell() -> &'static Mutex<Option<Hook>> {
    static H: OnceLock<Mutex<Option<Hook>>> = OnceLock::new();
    H.get_or_init(|| Mutex::new(None))
}

/// The app installs this: called (on a runtime thread) whenever a new snapshot is ready for the UI.
pub fn set_snapshot_hook(hook: Arc<dyn Fn() + Send + Sync>) {
    *hook_cell().lock().unwrap() = Some(hook);
}

fn notify() {
    let h = hook_cell().lock().unwrap().clone();
    if let Some(h) = h {
        h();
    }
}

fn is_recorder(identity: &str) -> bool {
    crate::hear::is_recorder(identity)
}

/// The app's window state: "focused" | "unfocused" | "hidden". Hidden pauses the UI snapshot (the report to the
/// others keeps going); showing again hands over the latest snapshot at once; focus samples every second.
pub fn set_view(state: &str) {
    let v = match state {
        "hidden" => 0,
        "unfocused" => 1,
        _ => 2,
    };
    let was = VIEW.swap(v, Ordering::SeqCst);
    if was == 0 && v != 0 {
        notify();
    }
}

fn view() -> u8 {
    VIEW.load(Ordering::SeqCst)
}

/// The latest snapshot for the UI ("" outside a call). Cheap.
pub fn snapshot_json() -> String {
    st().lock().unwrap().snapshot.clone()
}

/// TINHAT_CALLSTATS_LOG=1: every sample's snapshot goes to the log (the interop gate reads it).
fn log_snapshots() -> bool {
    static L: OnceLock<bool> = OnceLock::new();
    *L.get_or_init(|| std::env::var("TINHAT_CALLSTATS_LOG").map(|v| v == "1").unwrap_or(false))
}

// ── hooks from call.rs ──────────────────────────────────────────────────────────────────────────────────────

pub(crate) fn session_start(me: &str) {
    SESSION.fetch_add(1, Ordering::SeqCst);
    let mut s = st().lock().unwrap();
    *s = St { on: true, me: me.to_string(), period: FOCUSED_MS, ..St::default() };
    s.snapshot = build_snapshot(&s, now_ms());
}

pub(crate) fn session_end() {
    SESSION.fetch_add(1, Ordering::SeqCst);
    if let Some(t) = TASK.get_or_init(|| Mutex::new(None)).lock().unwrap().take() {
        t.abort();
    }
    let was_on = {
        let mut s = st().lock().unwrap();
        let on = s.on;
        *s = St::default();
        on
    };
    if was_on {
        notify();
    }
}

pub(crate) fn participant_present(identity: &str) {
    let mut s = st().lock().unwrap();
    if !s.on || is_recorder(identity) {
        return;
    }
    s.present.insert(identity.to_string());
    peer(&mut s, identity);
}

pub(crate) fn participant_left(identity: &str) {
    let mut s = st().lock().unwrap();
    s.present.remove(identity);
    s.peers.remove(identity);
    s.gate.forget(identity);
    s.tracks.retain(|_, t| t.who != identity);
}

pub(crate) fn track_added(identity: &str, sid: &str, source: TrackSource, track: RemoteTrack) {
    let Some(k) = kind_of(source) else { return };
    let mut s = st().lock().unwrap();
    if !s.on || is_recorder(identity) {
        return;
    }
    s.present.insert(identity.to_string());
    peer(&mut s, identity);
    s.tracks.insert(sid.to_string(), Tr { who: identity.to_string(), k, track, prev: None, cur: None, rep: None });
}

pub(crate) fn track_removed(sid: &str) {
    st().lock().unwrap().tracks.remove(sid);
}

pub(crate) fn connection_quality(identity: &str, local: bool, q: livekit::participant::ConnectionQuality) {
    let mut s = st().lock().unwrap();
    if !s.on {
        return;
    }
    let name = cq_name(q);
    if local {
        s.my_cq = Some(name);
    } else if !is_recorder(identity) {
        peer(&mut s, identity).cq = Some(name);
    }
}

/// A `tinhat.rxq` report from `who` (WIDGETS.md §10.2, in this order): (1) only from a current remote participant that
/// is not the recorder, payload ≤ 600 bytes — else dropped unread; (2) the per-sender gate (≤ 4 reads per 2 s window,
/// one accepted per 2 s); (3) strict UTF-8; (4) `decode_rxq`; (5) valid → accepted and shown.
pub(crate) fn handle_rxq(who: &str, payload: &[u8]) {
    handle_rxq_at(who, payload, now_ms());
}

fn handle_rxq_at(who: &str, payload: &[u8], now: f64) -> bool {
    let mut s = st().lock().unwrap();
    if !s.on || who.is_empty() || !s.present.contains(who) || is_recorder(who) || payload.len() > rxq::RXQ_MAX_BYTES {
        return false;
    }
    match rxq::receive_rxq_bytes(&mut s.gate, who, now, payload) {
        rxq::Verdict::Accept(r) => {
            let p = peer(&mut s, who);
            p.rxq = Some((now, r));
            p.out_ever = true;
            true
        }
        _ => false,
    }
}

// ── the sampler ─────────────────────────────────────────────────────────────────────────────────────────────

/// One receiver's stats → the reference's `RxSample`: the track's `inbound-rtp` (bytes, packets, lost, jitter; video
/// size and rate), its codec's mimeType and the round trip of the transport it arrives on (the selected candidate
/// pair's currentRoundTripTime — the nominated pair when the report names none). Times in ms (libwebrtc's µs / 1000).
pub(crate) fn read_sample(k: Kind, stats: &[RtcStats]) -> Option<RxSample> {
    let mut sample: Option<RxSample> = None;
    let mut codec_id = String::new();
    let mut transport_id = String::new();
    for s in stats {
        if let RtcStats::InboundRtp(i) = s {
            let fps = i.inbound.frames_per_second;
            let video = k != Kind::A;
            sample = Some(RxSample {
                k,
                at: i.rtc.timestamp as f64 / 1000.0,
                bytes: i.inbound.bytes_received as f64,
                packets: i.received.packets_received as f64,
                lost: i.received.packets_lost as f64,
                jitter: Some(i.received.jitter).filter(|x| x.is_finite()),
                rtt: None,
                fps: (video && fps.is_finite()).then_some(fps),
                w: video.then_some(i.inbound.frame_width as f64),
                h: video.then_some(i.inbound.frame_height as f64),
                codec: None,
            });
            codec_id = i.stream.codec_id.clone();
            transport_id = i.stream.transport_id.clone();
            break;
        }
    }
    let mut sample = sample?;
    let mut pair_id = String::new();
    for s in stats {
        match s {
            RtcStats::Codec(c) if !codec_id.is_empty() && c.rtc.id == codec_id => sample.codec = Some(c.codec.mime_type.clone()),
            RtcStats::Transport(t) if pair_id.is_empty() && (transport_id.is_empty() || t.rtc.id == transport_id) => {
                pair_id = t.transport.selected_candidate_pair_id.clone();
            }
            _ => {}
        }
    }
    for s in stats {
        if let RtcStats::CandidatePair(p) = s {
            let cp = &p.candidate_pair;
            let chosen = if pair_id.is_empty() { cp.nominated } else { p.rtc.id == pair_id };
            if chosen && cp.current_round_trip_time > 0.0 {
                sample.rtt = Some(cp.current_round_trip_time);
                break;
            }
        }
    }
    Some(sample)
}

/// Start the sampler for this call (called once per session by call.rs, never for the recorder).
pub(crate) fn spawn(lp: livekit::participant::LocalParticipant) {
    let my = SESSION.load(Ordering::SeqCst);
    let task = tokio::spawn(async move {
        let mut iv = tokio::time::interval(Duration::from_millis(250));
        iv.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut last_sample: Option<Instant> = None;
        let mut last_send: Option<Instant> = None;
        let sending = Arc::new(AtomicBool::new(false));
        let err_logged = Arc::new(AtomicBool::new(false));
        let slack = Duration::from_millis(125);
        loop {
            iv.tick().await;
            if SESSION.load(Ordering::SeqCst) != my || !st().lock().unwrap().on {
                break;
            }
            let period = if view() == 2 { FOCUSED_MS } else { UNFOCUSED_MS };
            let due = last_sample.map_or(true, |t| t.elapsed() + slack >= Duration::from_millis(period));
            let send_due = last_send.map_or(true, |t| t.elapsed() + slack >= Duration::from_millis(rxq::RXQ_SEND_MS));
            if !due && !send_due {
                continue;
            }
            last_sample = Some(Instant::now());
            sample_once(my, period).await;
            if SESSION.load(Ordering::SeqCst) != my {
                break;
            }
            if send_due && !sending.load(Ordering::Relaxed) {
                last_send = Some(Instant::now());
                let pkts = rxq_packets();
                if !pkts.is_empty() {
                    sending.store(true, Ordering::Relaxed);
                    let (lp2, busy, logged) = (lp.clone(), sending.clone(), err_logged.clone());
                    tokio::spawn(async move {
                        for pkt in pkts {
                            match tokio::time::timeout(Duration::from_secs(2), lp2.publish_data(pkt)).await {
                                Ok(Ok(())) => logged.store(false, Ordering::Relaxed),
                                Ok(Err(e)) => {
                                    if !logged.swap(true, Ordering::Relaxed) {
                                        eprintln!("[callstats] report not sent: {e}");
                                    }
                                }
                                Err(_) => {
                                    if !logged.swap(true, Ordering::Relaxed) {
                                        eprintln!("[callstats] report not sent within 2 s — skipping");
                                    }
                                    break;
                                }
                            }
                        }
                        busy.store(false, Ordering::Relaxed);
                    });
                }
            }
            if log_snapshots() {
                eprintln!("[callstats] {}", snapshot_json());
            }
            if view() != 0 {
                notify();
            }
        }
    });
    if let Some(old) = TASK.get_or_init(|| Mutex::new(None)).lock().unwrap().replace(task) {
        old.abort();
    }
}

async fn sample_once(my: u64, period: u64) {
    let list: Vec<(String, RemoteTrack)> =
        st().lock().unwrap().tracks.iter().map(|(sid, t)| (sid.clone(), t.track.clone())).collect();
    let reads = futures_util::future::join_all(list.into_iter().map(|(sid, track)| async move {
        let r = tokio::time::timeout(Duration::from_secs(2), track.get_stats()).await;
        (sid, r)
    }))
    .await;
    let now = now_ms();
    let mut s = st().lock().unwrap();
    if SESSION.load(Ordering::SeqCst) != my || !s.on {
        return;
    }
    for (sid, r) in reads {
        let Ok(Ok(stats)) = r else { continue };
        let Some(t) = s.tracks.get_mut(&sid) else { continue };
        let Some(cur) = read_sample(t.k, &stats) else { continue };
        if let Some(n) = rx_numbers(t.prev.as_ref(), &cur) {
            t.cur = Some(n);
        }
        t.prev = Some(cur);
    }
    // per-person sparkline: this sample's incoming kbps (all of their reported tracks)
    let mut sums: HashMap<String, f64> = HashMap::new();
    for t in s.tracks.values() {
        let e = sums.entry(t.who.clone()).or_insert(0.0);
        if let Some(c) = &t.cur {
            *e += c.kbps.unwrap_or(0.0);
        }
    }
    for (who, p) in s.peers.iter_mut() {
        p.spark.push_back(sums.get(who).copied().unwrap_or(0.0));
        while p.spark.len() > SPARK_LEN {
            p.spark.pop_front();
        }
    }
    s.seq += 1;
    s.period = period;
    s.snapshot = build_snapshot(&s, now);
}

/// What we receive from each person (their tracks' latest numbers, a/v/s order).
fn incoming(s: &St) -> BTreeMap<String, Vec<TrackNumbers>> {
    let mut by: BTreeMap<String, Vec<TrackNumbers>> = BTreeMap::new();
    for t in s.tracks.values() {
        let e = by.entry(t.who.clone()).or_default();
        if let Some(c) = &t.cur {
            e.push(c.clone());
        }
    }
    for v in by.values_mut() {
        v.sort_by_key(|x| x.k.rank());
    }
    by
}

/// Our reports: one per person we receive something from. Each track's numbers cover the time since the previous
/// report (its `rep` baseline, then moved to the latest sample): the first call only takes the baselines (nothing to
/// report yet — an empty report is not sent), like the web's first rxq tick.
fn rxq_packets() -> Vec<livekit::DataPacket> {
    let mut guard = st().lock().unwrap();
    let s = &mut *guard;
    let mut by: BTreeMap<String, Vec<TrackNumbers>> = BTreeMap::new();
    for t in s.tracks.values_mut() {
        let Some(latest) = t.prev.clone() else { continue };
        let n = rx_numbers(t.rep.as_ref(), &latest);
        t.rep = Some(latest);
        if let Some(n) = n {
            by.entry(t.who.clone()).or_default().push(n);
        }
    }
    for v in by.values_mut() {
        v.sort_by_key(|x| x.k.rank());
    }
    by.into_iter()
        .filter(|(who, _)| s.present.contains(who) && !is_recorder(who))
        .filter_map(|(who, t)| {
            let text = rxq::encode_rxq(&t);
            (!text.is_empty()).then(|| livekit::DataPacket {
                payload: text.into_bytes(),
                topic: Some(RXQ_TOPIC.to_string()),
                reliable: false,
                destination_identities: vec![ParticipantIdentity(who)],
            })
        })
        .collect()
}

fn band_str(b: Option<Band>) -> Value {
    b.map(|x| Value::String(x.as_str().into())).unwrap_or(Value::Null)
}

/// A lane for the UI: its tracks (the reference's objects + their band, labels), its band (the given one, else the
/// worst track's), the summed bitrate with its label and bar fill (per-mille).
fn lane_json(t: &[TrackNumbers], band: Option<Band>) -> Value {
    let kbps: f64 = t.iter().map(|x| x.kbps.unwrap_or(0.0)).sum();
    let tracks: Vec<Value> = t
        .iter()
        .map(|x| {
            let mut o = x.to_json();
            o["q"] = band_str(x.band());
            o["rate"] = json!(quality::fmt_rate(x.kbps.unwrap_or(0.0)));
            if let Some(l) = x.loss {
                o["lossText"] = json!(quality::fmt_loss(l));
            }
            o["res"] = json!(quality::res_chip(x.w, x.h, x.fps));
            o
        })
        .collect();
    json!({
        "q": band_str(band.or_else(|| worst_band(t.iter().map(TrackNumbers::band)))),
        "kbps": tinhat_widgets::num(kbps),
        "rate": quality::fmt_rate(kbps),
        "fill": tinhat_widgets::num(quality::rate_fill_pm(kbps)),
        "t": tracks,
    })
}

/// The UI snapshot:
/// `{"on":true,"seq":N,"period":ms,"me":{"id","cq","dot"},"down":kbps,"downRate":"…","worst":id|"",
///   "peers":[{"id","cq","dot","in":lane,"out":lane|null,"outAge":s|null,"outEver":bool,"inCallS":s,"spark":[kbps…],
///   "worst":band}]}`
/// (lane: see [`lane_json`]; `dot` = the ConnectionQuality band, `worst` = the worst of dot, in and out).
fn build_snapshot(s: &St, now: f64) -> String {
    if !s.on {
        return String::new();
    }
    let inc = incoming(s);
    let mut down = 0.0;
    let mut peers = Vec::new();
    let mut worst: Option<(Band, String)> = None;
    for (who, p) in s.peers.iter() {
        if !s.present.contains(who) {
            continue;
        }
        let inn = inc.get(who).cloned().unwrap_or_default();
        down += inn.iter().map(|x| x.kbps.unwrap_or(0.0)).sum::<f64>();
        let in_lane = lane_json(&inn, None);
        let live = p.rxq.as_ref().filter(|(at, _)| !rxq::rxq_stale(Some(*at), now));
        let (out, age, out_band) = match live {
            Some((at, r)) => (lane_json(&r.t, Some(r.q)), json!(((now - at) / 1000.0).floor()), Some(r.q)),
            None => (Value::Null, Value::Null, None),
        };
        let dot = cq_band(p.cq);
        let in_band = worst_band(inn.iter().map(TrackNumbers::band));
        let pw = worst_band([dot, in_band, out_band]);
        if let Some(b) = pw {
            if worst.as_ref().map_or(true, |(wb, _)| b > *wb) {
                worst = Some((b, who.clone()));
            }
        }
        peers.push(json!({
            "id": who,
            "cq": p.cq.unwrap_or(""),
            "dot": band_str(dot),
            "in": in_lane,
            "out": out,
            "outAge": age,
            "outEver": p.out_ever,
            "inCallS": ((now - p.since) / 1000.0).floor().max(0.0),
            "spark": p.spark.iter().map(|x| tinhat_widgets::num(*x)).collect::<Vec<_>>(),
            "worst": band_str(pw),
        }));
    }
    json!({
        "on": true,
        "seq": s.seq,
        "period": s.period,
        "me": { "id": s.me, "cq": s.my_cq.unwrap_or(""), "dot": band_str(cq_band(s.my_cq)) },
        "down": tinhat_widgets::num(down),
        "downRate": quality::fmt_rate(down),
        "worst": worst.map(|(_, w)| w).unwrap_or_default(),
        "peers": peers,
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_only_from_the_call_and_at_most_one_per_two_seconds() {
        session_start("@me:hs:DEV");
        participant_present("@a:hs:A");
        let p = br#"{"v":1,"q":"good","t":[{"k":"a","kbps":40,"loss":0,"jit":3}]}"#;
        let t0 = 1_000_000.0;
        assert!(!handle_rxq_at("@stranger:hs:X", p, t0)); // not in the call
        assert!(handle_rxq_at("@a:hs:A", p, t0));
        assert!(!handle_rxq_at("@a:hs:A", p, t0 + 1999.0));
        assert!(handle_rxq_at("@a:hs:A", p, t0 + 2000.0));
        assert!(!handle_rxq_at("@a:hs:A", b"junk", t0 + 5000.0)); // read (an attempt), refused as not-json
        assert!(!handle_rxq_at("@a:hs:A", b"\xed\xa0\x80", t0 + 5001.0)); // malformed UTF-8: refused
        {
            let s = st().lock().unwrap();
            let snap: Value = serde_json::from_str(&build_snapshot(&s, t0 + 3000.0)).unwrap();
            assert_eq!(snap["peers"][0]["out"]["q"], "good");
            assert_eq!(snap["peers"][0]["out"]["t"][0]["kbps"], 40);
            // stale after 12 s without a report
            let snap: Value = serde_json::from_str(&build_snapshot(&s, t0 + 2000.0 + 12_001.0)).unwrap();
            assert!(snap["peers"][0]["out"].is_null());
        }
        participant_left("@a:hs:A");
        assert!(!handle_rxq_at("@a:hs:A", p, t0 + 9000.0));
        crate::hear::set_recorder(Some("@recorder:hs".into())); // the recorder this server announces
        participant_present("@recorder:hs:REC");
        assert!(!handle_rxq_at("@recorder:hs:REC", p, t0 + 9000.0));
        session_end();
        assert_eq!(snapshot_json(), "");
    }
}
