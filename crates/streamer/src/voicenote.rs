//! Voice notes: the recorder (docs/VOICE-NOTES.md §4 "Desktop chain"). One recording per process, outside calls only.
//!
//!   {the app's chosen input: audio_src_fragment(mic_target)} ! queue ! audioconvert ! audioresample
//!     ! F32LE 48 kHz mono ! [audiornnoise ! F32LE, when noise suppression is on and the element exists] ! tee
//!   tee. ! queue (unbounded, leaky=no) ! appsink (drop=false, sync=false)   — frame peaks + the sample count, folded
//!                                                                              on the streaming thread (never the UI's)
//!   tee. ! queue (unbounded, leaky=no) ! audioconvert ! S16LE ! opusenc (24 kb/s VBR, voice, 20 ms) ! oggmux ! filesink
//!
//! Both branches see the same samples: a probe on the tee's sink pad counts them and cuts the stream at the cap
//! (15:00 = 43 200 000 samples), so the waveform, the duration and the file all end at the same sample. No echo
//! cancellation and no AGC (there is no far end outside a call); we never touch the call's AEC state, probe or lease.
//!
//! Ownership of the microphone (review r1 #2/#3): one process-wide [`Phase`] — Idle → Starting → Running → Stopping →
//! Idle. It is Idle only once the recorder's pipeline is in NULL (the device released). Each running recording has its
//! own WORKER thread: it watches for a stop / cancel / call request, the cap and errors, and finalizes by itself (EOS →
//! NULL) — independent of any view polling it. [`release_for_call`] (called by the call's mic start before it opens its
//! capture) asks whatever runs to stop and waits, bounded, for the confirmed release; on timeout the call capture fails
//! instead of overlapping. Results go to the app's listener ([`set_listener`]).
//!
//! Finalizing (review r1 #4): a note is sendable only when its file was finalized cleanly (EOS reached the file, no
//! error on the bus during the whole recording and its finish) or, after a failure, when an independent decode of the
//! file succeeds (the sample count then comes from that decode).
//!
//! Capture rules (capture_lint.rs): the recorder takes its own capture lease (`Kind::VoiceNote`, refused while
//! watching / capture barred and during a call) and starts only through `leased_play` (the CLI self-test through
//! `leased_start`).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

use gstreamer as gst;
use gstreamer::prelude::*;
use tinhat_widgets::voicenote::{duration_ms, waveform, PeakMeter, VOICE_FRAME, VOICE_MAX_MS, VOICE_RATE, WAVEFORM_N};

/// The cap in samples (15:00 at 48 kHz = 43 200 000), counted from what the tee passed.
pub const CAP_SAMPLES: u64 = VOICE_MAX_MS * VOICE_RATE as u64 / 1000;
/// Frame peaks kept for the live level strip (the last ~3 s).
pub const RECENT_FRAMES: usize = 300;
/// How long the call's mic start waits for the recorder's confirmed release.
pub const CALL_RELEASE_MS: u64 = 2000;

/// Why a recording did not start.
#[derive(Debug, Clone, PartialEq)]
pub enum StartError {
    /// watching (Pad Watch) / the capture bar is closed: the lease was refused
    Watching,
    /// a call session is current: voice messages are off until you leave
    InCall,
    /// a recording is already starting / running / stopping
    Busy,
    /// cancelled (or a call asked for the microphone) while the device was opening
    Cancelled,
    /// the pipeline could not be built or started (the log names the cause)
    Failed(String),
}

/// Why a recording ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// Stop / Send (the user)
    Stop,
    /// the 15-minute cap
    Cap,
    /// the device / pipeline failed
    Error,
    /// a call (or a watch) needed the microphone
    Call,
}

/// What the streaming thread folds (shared with the UI poll: atomics + short locks).
#[derive(Default)]
struct Live {
    meter: Mutex<PeakMeter>,
    samples: AtomicU64,
    level: AtomicU32,
    capped: AtomicBool,
    failed: Mutex<Option<String>>,
    recent: Mutex<std::collections::VecDeque<u32>>,
}

impl Live {
    fn fold(&self, chunk: &[f32]) {
        let mut m = self.meter.lock().unwrap_or_else(|p| p.into_inner());
        let before = m.peaks.len();
        m.push(chunk);
        self.samples.store(m.samples, Ordering::SeqCst);
        self.level.store(m.live(), Ordering::Relaxed);
        if m.peaks.len() > before {
            let mut r = self.recent.lock().unwrap_or_else(|p| p.into_inner());
            for &p in &m.peaks[before..] {
                r.push_back(p);
                while r.len() > RECENT_FRAMES {
                    r.pop_front();
                }
            }
        }
    }
    fn error(&self) -> Option<String> {
        self.failed.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
}

/// The finished note: what the event and the review need.
#[derive(Debug, Clone, PartialEq)]
pub struct Finished {
    pub id: u64,
    pub path: PathBuf,
    pub samples: u64,
    pub duration_ms: u64,
    pub waveform: Vec<u32>,
    pub size: u64,
    pub reason: Reason,
    pub capped: bool,
    /// an error seen on the bus at any point (recording or finishing)
    pub error: Option<String>,
    /// EOS reached the file and no error was seen
    pub clean: bool,
    /// not clean, but an independent decode of the file succeeded (`samples` = the decoded count)
    pub salvaged: bool,
}

impl Finished {
    /// May this note be reviewed and sent?
    pub fn sendable(&self) -> bool {
        (self.clean || self.salvaged) && self.size > 0 && self.samples > 0
    }
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "samples": self.samples, "duration_ms": self.duration_ms, "waveform": self.waveform, "size": self.size,
            "path": self.path.to_string_lossy(), "capped": self.capped, "clean": self.clean, "salvaged": self.salvaged,
            "error": self.error, "reason": format!("{:?}", self.reason),
        })
    }
}

/// The live state for the UI poll.
#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    pub id: u64,
    pub samples: u64,
    /// the live frame peak, 0..32768
    pub level: u32,
    /// the last [`RECENT_FRAMES`] frame peaks, oldest first
    pub recent: Vec<u32>,
    pub capped: bool,
    pub failed: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Req {
    Stop,
    Cancel,
    Call,
}

/// Who owns the microphone. Idle = the recorder holds no device (its pipeline reached NULL).
enum Phase {
    Idle,
    Starting { id: u64, cancel: Arc<AtomicBool> },
    Running { id: u64, live: Arc<Live>, req: Arc<Mutex<Option<Req>>> },
    Stopping { id: u64 },
}

fn phase() -> &'static (Mutex<Phase>, Condvar) {
    static P: OnceLock<(Mutex<Phase>, Condvar)> = OnceLock::new();
    P.get_or_init(|| (Mutex::new(Phase::Idle), Condvar::new()))
}
fn lock_phase() -> std::sync::MutexGuard<'static, Phase> {
    phase().0.lock().unwrap_or_else(|p| p.into_inner())
}
/// The device is released: Idle, and everyone waiting for it is told. Tests only (set the phase by hand); the code
/// releases through [`set_idle_if`].
#[cfg(test)]
fn set_idle() {
    *lock_phase() = Phase::Idle;
    phase().1.notify_all();
}
/// Release the reservation of recording `id` (review r2 #1): Idle only when the phase still belongs to `id` — a late
/// cleanup never erases a newer reservation.
fn set_idle_if(id: u64) -> bool {
    let mut p = lock_phase();
    let mine = match &*p {
        Phase::Starting { id: i, .. } | Phase::Running { id: i, .. } | Phase::Stopping { id: i } => *i == id,
        Phase::Idle => false,
    };
    if mine {
        *p = Phase::Idle;
        phase().1.notify_all();
    }
    mine
}

/// tests: make the next N NULL transitions fail (review r2 #3)
#[cfg(test)]
static FAIL_NULL: AtomicU32 = AtomicU32::new(0);
/// tests: make the next worker spawn fail (review r2 #1)
#[cfg(test)]
static FAIL_SPAWN: AtomicBool = AtomicBool::new(false);

/// review r2 #3: NULL, CONFIRMED — the state change returned Success / NoPreroll, or an Async change completed (bounded
/// wait), and the pipeline reports NULL. false = the device may still be held.
fn null_confirmed(p: &gst::Pipeline) -> bool {
    #[cfg(test)]
    if FAIL_NULL.load(Ordering::SeqCst) > 0 {
        FAIL_NULL.fetch_sub(1, Ordering::SeqCst);
        return false;
    }
    match p.set_state(gst::State::Null) {
        Ok(gst::StateChangeSuccess::Success) | Ok(gst::StateChangeSuccess::NoPreroll) => p.current_state() == gst::State::Null,
        Ok(gst::StateChangeSuccess::Async) => {
            let (r, cur, _) = p.state(gst::ClockTime::from_seconds(2));
            r.is_ok() && cur == gst::State::Null
        }
        Err(_) => false,
    }
}

/// Pipelines whose NULL is not confirmed yet, each with the recording that still owns the microphone (review r3 #1):
/// they are held HERE — never only inside a closure — until a retry confirms NULL.
fn parked() -> &'static Mutex<Vec<(u64, gst::Pipeline)>> {
    static P: OnceLock<Mutex<Vec<(u64, gst::Pipeline)>>> = OnceLock::new();
    P.get_or_init(|| Mutex::new(vec![]))
}
/// One retry pass over the parked pipelines: each confirmed NULL releases its recording's reservation; the others stay.
fn retry_parked() {
    let items = std::mem::take(&mut *parked().lock().unwrap_or_else(|p| p.into_inner()));
    if items.is_empty() {
        return;
    }
    let mut left = vec![];
    for (id, p) in items {
        if null_confirmed(&p) {
            eprintln!("[voicenote] recording {id}: NULL confirmed on retry — released");
            set_idle_if(id);
        } else {
            left.push((id, p));
        }
    }
    parked().lock().unwrap_or_else(|p| p.into_inner()).extend(left);
}
/// The one cleanup worker (started on first need): retries the parked pipelines every 500 ms. false = it could not be
/// created — then every release retries synchronously (bounded) and every call wait retries too.
fn cleanup_worker() -> bool {
    static W: OnceLock<bool> = OnceLock::new();
    *W.get_or_init(|| {
        std::thread::Builder::new()
            .name("voicenote-cleanup".into())
            .spawn(|| loop {
                std::thread::sleep(Duration::from_millis(500));
                retry_parked();
            })
            .is_ok()
    })
}

/// Take the pipeline of recording `id` to NULL and only then release `id`'s reservation. true = released now. A failed
/// NULL keeps the ownership AND the pipeline (parked, retried by the cleanup worker — or, without one, here, bounded,
/// and by every later call wait): meanwhile the call's release wait runs into its bound and that capture fails.
fn release(id: u64, pipeline: gst::Pipeline) -> bool {
    release_with(id, pipeline, cleanup_worker())
}
fn release_with(id: u64, pipeline: gst::Pipeline, worker: bool) -> bool {
    if null_confirmed(&pipeline) {
        set_idle_if(id);
        return true;
    }
    eprintln!("[voicenote] recording {id}: NULL not confirmed — keeping the microphone's ownership and the pipeline");
    parked().lock().unwrap_or_else(|p| p.into_inner()).push((id, pipeline));
    if !worker {
        for _ in 0..5 {
            std::thread::sleep(Duration::from_millis(200));
            retry_parked();
            if !owned_by(id) {
                return true;
            }
        }
        eprintln!("[voicenote] recording {id}: no cleanup worker — the pipeline stays parked, retried on every call wait");
    }
    false
}
/// Does recording `id` still hold the microphone (its reservation not released)?
fn owned_by(id: u64) -> bool {
    match &*lock_phase() {
        Phase::Idle => false,
        Phase::Starting { id: i, .. } | Phase::Running { id: i, .. } | Phase::Stopping { id: i } => *i == id,
    }
}
/// Wait (bounded) until recording `id` released the microphone. true = released.
fn wait_released(id: u64, ms: u64) -> bool {
    let t0 = std::time::Instant::now();
    while owned_by(id) {
        if t0.elapsed() >= Duration::from_millis(ms) {
            return false;
        }
        retry_parked();
        let (m, cv) = phase();
        let g = m.lock().unwrap_or_else(|p| p.into_inner());
        let _ = cv.wait_timeout(g, Duration::from_millis(200));
    }
    true
}

type Listener = Box<dyn Fn(&Finished) + Send + Sync>;
fn listener() -> &'static Mutex<Option<Listener>> {
    static L: OnceLock<Mutex<Option<Listener>>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(None))
}
/// Results nobody listens for (the CLI self-test, the tests): picked up by [`wait_result`].
fn results() -> &'static (Mutex<Vec<Finished>>, Condvar) {
    static R: OnceLock<(Mutex<Vec<Finished>>, Condvar)> = OnceLock::new();
    R.get_or_init(|| (Mutex::new(vec![]), Condvar::new()))
}
/// The app's listener: every finished recording (stopped, capped, failed, stopped for a call) — on the worker thread.
pub fn set_listener(f: impl Fn(&Finished) + Send + Sync + 'static) {
    *listener().lock().unwrap_or_else(|p| p.into_inner()) = Some(Box::new(f));
}
fn deliver(f: Finished) {
    let l = listener().lock().unwrap_or_else(|p| p.into_inner());
    match l.as_ref() {
        Some(cb) => cb(&f),
        None => {
            results().0.lock().unwrap_or_else(|p| p.into_inner()).push(f);
            results().1.notify_all();
        }
    }
}
/// Wait (bounded) for recording `id`'s result when no listener is set.
pub fn wait_result(id: u64, ms: u64) -> Option<Finished> {
    let (m, cv) = results();
    let g = m.lock().unwrap_or_else(|p| p.into_inner());
    let (mut g, _) = cv
        .wait_timeout_while(g, Duration::from_millis(ms), |v| !v.iter().any(|f| f.id == id))
        .unwrap_or_else(|p| p.into_inner());
    let i = g.iter().position(|f| f.id == id)?;
    Some(g.remove(i))
}

/// Is the RNNoise stage wanted? The user's noise suppression is on (any mode) and the element exists.
fn rnnoise_wanted() -> bool {
    crate::call::mic_ns_mode() != crate::call::NsMode::Off && gst::ElementFactory::find("audiornnoise").is_some()
}

/// The recorder's launch line for a capture head (pure: the unit test reads it).
pub(crate) fn recorder_desc(src: &str, rnnoise: bool) -> String {
    const F32: &str = "audio/x-raw,format=F32LE,rate=48000,channels=1,layout=interleaved";
    let denoise = if rnnoise { format!("audiornnoise ! {F32} ! ") } else { String::new() };
    format!(
        "{src} ! queue name=vnq max-size-time=200000000 max-size-buffers=0 max-size-bytes=0 leaky=no ! \
         audioconvert ! audioresample ! {F32} ! {denoise}tee name=vnt \
         vnt. ! queue max-size-time=0 max-size-buffers=0 max-size-bytes=0 leaky=no ! \
           appsink name=vnmeter emit-signals=false drop=false sync=false max-buffers=0 \
         vnt. ! queue max-size-time=0 max-size-buffers=0 max-size-bytes=0 leaky=no ! audioconvert ! \
           audio/x-raw,format=S16LE,rate=48000,channels=1,layout=interleaved ! opusenc name=vnenc ! oggmux ! \
           filesink name=vnfile sync=false async=false"
    )
}

/// The encoder settings, each only when this GStreamer's opusenc has it (older runtimes name some differently).
fn configure_encoder(enc: &gst::Element) {
    enc.set_property("bitrate", 24_000i32);
    for (k, v) in [("bitrate-type", "vbr"), ("audio-type", "voice"), ("frame-size", "20")] {
        if enc.find_property(k).is_some() {
            enc.set_property_from_str(k, v);
        } else {
            eprintln!("[voicenote] opusenc has no `{k}` here — left at its default");
        }
    }
}

/// The production instrumentation on a recorder launch line: the encoder settings, the file, the cap probe, the meter
/// sink, the bus handler. Shared by the app, the CLI self-test and the unit tests (which give a synthetic head).
fn instrument(desc: &str, path: &Path, cap: u64) -> Result<(gst::Pipeline, Arc<Live>), String> {
    let pipeline = gst::parse::launch(desc)
        .map_err(|e| format!("build: {e}"))?
        .downcast::<gst::Pipeline>()
        .map_err(|_| "build: not a pipeline".to_string())?;
    let enc = pipeline.by_name("vnenc").ok_or("no encoder")?;
    configure_encoder(&enc);
    let file = pipeline.by_name("vnfile").ok_or("no file sink")?;
    file.set_property("location", path.to_string_lossy().to_string());
    let live = Arc::new(Live::default());
    // the cap, at the one point both branches share: the tee's sink pad. At the cap the rest is dropped and the worker
    // (which sees `capped`) finalizes and releases the device.
    let tee = pipeline.by_name("vnt").ok_or("no tee")?;
    let pad = tee.static_pad("sink").ok_or("no tee pad")?;
    let counted = Arc::new(AtomicU64::new(0));
    let l2 = live.clone();
    pad.add_probe(gst::PadProbeType::BUFFER, move |_, info| {
        let Some(gst::PadProbeData::Buffer(buf)) = &mut info.data else { return gst::PadProbeReturn::Ok };
        let n = (buf.size() / 4) as u64;
        let done = counted.load(Ordering::SeqCst);
        if done >= cap {
            l2.capped.store(true, Ordering::SeqCst);
            return gst::PadProbeReturn::Drop;
        }
        if done + n >= cap {
            let keep = (cap - done) as usize;
            if (keep as u64) < n {
                buf.make_mut().set_size(keep * 4);
            }
            counted.store(cap, Ordering::SeqCst);
            l2.capped.store(true, Ordering::SeqCst);
        } else {
            counted.store(done + n, Ordering::SeqCst);
        }
        gst::PadProbeReturn::Ok
    });
    let sink = pipeline
        .by_name("vnmeter")
        .and_then(|e| e.downcast::<gstreamer_app::AppSink>().ok())
        .ok_or("no meter sink")?;
    let l3 = live.clone();
    sink.set_callbacks(
        gstreamer_app::AppSinkCallbacks::builder()
            .new_sample(move |s| {
                let sample = s.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                let buffer = sample.buffer().ok_or(gst::FlowError::Error)?;
                let map = buffer.map_readable().map_err(|_| gst::FlowError::Error)?;
                let chunk: Vec<f32> =
                    map.as_slice().chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
                l3.fold(&chunk);
                Ok(gst::FlowSuccess::Ok)
            })
            .build(),
    );
    // errors (recorded for the whole life of the pipeline, its finish included) and EOS; everything else is dropped so
    // the bus never grows
    if let Some(bus) = pipeline.bus() {
        let l4 = live.clone();
        bus.set_sync_handler(move |_, msg| match msg.view() {
            gst::MessageView::Error(e) => {
                let who = msg.src().map(|s| s.path_string().to_string()).unwrap_or_default();
                eprintln!("[voicenote] ERROR from {who}: {} ({})", e.error(), e.debug().map(|d| d.to_string()).unwrap_or_default());
                let mut f = l4.failed.lock().unwrap_or_else(|p| p.into_inner());
                if f.is_none() {
                    *f = Some(e.error().to_string());
                }
                gst::BusSyncReply::Pass
            }
            gst::MessageView::Eos(_) => gst::BusSyncReply::Pass,
            _ => gst::BusSyncReply::Drop,
        });
    }
    Ok((pipeline, live))
}

/// Build the recorder for the app's chosen input, writing `path`. Not started: [`start`] (leased_play) or the
/// self-test (leased_start) starts it.
fn build_recorder(path: &Path) -> Result<(gst::Pipeline, Arc<Live>), String> {
    crate::ensure_init().map_err(|e| format!("gstreamer: {e}"))?;
    let node = crate::call::mic_target_node();
    let src = crate::call::audio_src_fragment(node.as_deref());
    let rn = rnnoise_wanted();
    let r = instrument(&recorder_desc(&src, rn), path, CAP_SAMPLES)?;
    eprintln!(
        "[voicenote] recorder built: input {} {}",
        node.as_deref().unwrap_or("(system default)"),
        if rn { "+ RNNoise" } else { "(no noise suppression)" }
    );
    Ok(r)
}

/// Reserve the microphone for recording `id` (Idle → Starting). None = busy.
fn reserve(id: u64) -> Option<Arc<AtomicBool>> {
    let mut p = lock_phase();
    if !matches!(*p, Phase::Idle) {
        return None;
    }
    let cancel = Arc::new(AtomicBool::new(false));
    *p = Phase::Starting { id, cancel: cancel.clone() };
    Some(cancel)
}

/// Undo a start: the file goes, the pipeline (if any) to a confirmed NULL, then `id`'s reservation is released.
fn abort_start(id: u64, pipeline: Option<gst::Pipeline>, path: &Path) {
    let _ = std::fs::remove_file(path);
    match pipeline {
        Some(p) => {
            release(id, p);
        }
        None => {
            set_idle_if(id);
        }
    }
}

/// Starting → Running: the worker takes the pipeline. Err = cancelled / a call meanwhile / no worker thread: the
/// pipeline has then been shut down here (review r2 #1: the REAL pipeline, kept until the worker exists) and only `id`'s
/// reservation released; the caller deletes the file.
fn adopt(id: u64, cancel: &AtomicBool, pipeline: gst::Pipeline, path: PathBuf, live: Arc<Live>) -> Result<(), ()> {
    let mut p = lock_phase();
    if cancel.load(Ordering::SeqCst) || !matches!(*p, Phase::Starting { id: i, .. } if i == id) {
        drop(p);
        release(id, pipeline);
        return Err(());
    }
    let req = Arc::new(Mutex::new(None));
    *p = Phase::Running { id, live: live.clone(), req: req.clone() };
    drop(p);
    // the worker is created first and receives the pipeline only once it exists: a failed spawn leaves it with us
    let (tx, rx) = std::sync::mpsc::channel::<(gst::Pipeline, PathBuf, Arc<Live>, Arc<Mutex<Option<Req>>>)>();
    #[cfg(test)]
    let fail = FAIL_SPAWN.swap(false, Ordering::SeqCst);
    #[cfg(not(test))]
    let fail = false;
    let spawned = if fail {
        Err(std::io::Error::other("test: spawn refused"))
    } else {
        std::thread::Builder::new().name(format!("voicenote-{id}")).spawn(move || {
            if let Ok((pl, pa, li, rq)) = rx.recv() {
                worker(id, pl, pa, li, rq);
            }
        })
    };
    match spawned {
        Ok(_) => {
            let _ = tx.send((pipeline, path, live, req));
            Ok(())
        }
        Err(e) => {
            eprintln!("[voicenote] recording {id}: no worker thread ({e}) — shutting the capture down");
            release(id, pipeline);
            Err(())
        }
    }
}

/// Start recording into `path` as recording `id` (the app's draft id). Only through `leased_play`, under a
/// `Kind::VoiceNote` lease: refused while watching / capture barred, and during a call. Blocks for the device's
/// bounded start (call it off the GUI thread).
pub fn start(path: &Path, id: u64) -> Result<(), StartError> {
    if crate::call::capture_barred() {
        return Err(StartError::Watching);
    }
    if crate::capture_lease::session().is_some() {
        return Err(StartError::InCall);
    }
    let Some(lease) = crate::capture_lease::take(crate::capture_lease::Kind::VoiceNote, None) else {
        eprintln!("[voicenote] watching — the recorder opens no microphone");
        return Err(StartError::Watching);
    };
    let Some(cancel) = reserve(id) else { return Err(StartError::Busy) };
    let (pipeline, live) = match build_recorder(path) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[voicenote] {e}");
            abort_start(id, None, path);
            return Err(StartError::Failed(e));
        }
    };
    if !crate::call::leased_play(&lease, &pipeline, 6000, "voice note start") {
        abort_start(id, Some(pipeline), path);
        return Err(if crate::call::capture_barred() { StartError::Watching } else { StartError::Failed("start".into()) });
    }
    if crate::call::capture_barred() || !lease.commit() {
        eprintln!("[voicenote] watching began while starting — stopped");
        abort_start(id, Some(pipeline), path);
        return Err(StartError::Watching);
    }
    if crate::capture_lease::session().is_some() {
        eprintln!("[voicenote] a call began while the recorder was starting — stopped");
        abort_start(id, Some(pipeline), path);
        return Err(StartError::InCall);
    }
    match adopt(id, &cancel, pipeline, path.to_path_buf(), live) {
        Ok(()) => {
            eprintln!("[voicenote] recording {id} started");
            Ok(())
        }
        Err(()) => {
            eprintln!("[voicenote] recording {id} was cancelled while the microphone opened (or got no worker)");
            let _ = std::fs::remove_file(path);
            Err(StartError::Cancelled)
        }
    }
}

/// The recording's own thread: waits for a request, the cap or an error, then finalizes (or cancels) and releases.
fn worker(id: u64, pipeline: gst::Pipeline, path: PathBuf, live: Arc<Live>, req: Arc<Mutex<Option<Req>>>) {
    let why = loop {
        if let Some(r) = req.lock().unwrap_or_else(|p| p.into_inner()).take() {
            break r;
        }
        if live.capped.load(Ordering::SeqCst) {
            break Req::Stop;
        }
        if live.error().is_some() {
            break Req::Stop;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    *lock_phase() = Phase::Stopping { id };
    let reason = match why {
        Req::Call => Reason::Call,
        _ if live.capped.load(Ordering::SeqCst) => Reason::Cap,
        _ if live.error().is_some() => Reason::Error,
        _ => Reason::Stop,
    };
    if why == Req::Cancel {
        // Windows refuses to delete a file filesink still holds: NULL first, then the draft
        release(id, pipeline); // review r2 #3: Idle only after a confirmed NULL
        delete_draft(&path);
        eprintln!("[voicenote] recording {id} cancelled");
        return;
    }
    // a call waits CALL_RELEASE_MS in all: its EOS waits are inside that, the NULL right after
    let (eos_ms, again_ms) = if reason == Reason::Call { (900, 400) } else { (3000, 1000) };
    let f = finish(id, &pipeline, &path, &live, reason, eos_ms, again_ms);
    deliver(f);
}

/// Removes `path`, retrying while something still has the file open (Windows: a sharing violation from a player, a probe or
/// a virus scanner). NotFound counts as done. false = still there after every try: the caller logs it, the next start sweeps it.
pub fn remove_retrying(path: &Path, tries: u32, wait: Duration, rm: &mut dyn FnMut(&Path) -> std::io::Result<()>) -> bool {
    for attempt in 0..tries.max(1) {
        match rm(path) {
            Ok(()) => return true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return true,
            Err(_) if attempt + 1 < tries => std::thread::sleep(wait),
            Err(_) => {}
        }
    }
    false
}

/// Deletes a cancelled draft, a few tries (a virus scanner can hold a new file for a moment on Windows).
fn delete_draft(path: &Path) {
    if !remove_retrying(path, 10, Duration::from_millis(100), &mut |p| std::fs::remove_file(p)) {
        eprintln!("[voicenote] a cancelled draft could not be deleted (the startup sweep removes it)");
    }
}

/// EOS → wait for it on the bus (bounded) → NULL on this thread (the device is released when it returns: then Idle) →
/// the numbers, validated.
fn finish(id: u64, pipeline: &gst::Pipeline, path: &Path, live: &Live, reason: Reason, eos_ms: u64, again_ms: u64) -> Finished {
    let bus = pipeline.bus();
    let wait = |ms: u64| -> bool {
        let Some(bus) = &bus else { return false };
        matches!(
            bus.timed_pop_filtered(gst::ClockTime::from_mseconds(ms), &[gst::MessageType::Eos, gst::MessageType::Error]),
            Some(m) if m.type_() == gst::MessageType::Eos
        )
    };
    let mut eos = false;
    if live.error().is_none() {
        pipeline.send_event(gst::event::Eos::new());
        eos = wait(eos_ms);
    }
    if !eos {
        // the source is dead (or never answered): close it and push EOS into the queue behind it, so the encoder and the
        // muxer can still finish the file with what was captured
        for e in pipeline.iterate_elements().into_iter().flatten() {
            if e.sink_pads().is_empty() {
                let _ = e.set_state(gst::State::Null);
            }
        }
        if let Some(pad) = pipeline.by_name("vnq").and_then(|q| q.static_pad("sink")) {
            pad.send_event(gst::event::Eos::new());
            eos = wait(again_ms);
        }
    }
    // review r2 #3: NULL, confirmed, and only then `id`'s reservation goes (else ownership + the pipeline are kept);
    // before the file's validation, so a call never waits on a decode. Review r3 #2: nothing is validated or published
    // while that teardown is still pending — the file could still change — and a teardown that never completes leaves
    // the note unsendable.
    let torn_down = release(id, pipeline.clone()) || wait_released(id, 10_000);
    let error = if torn_down { live.error() } else { Some(live.error().unwrap_or_else(|| "the recorder did not stop".into())) };
    let (peaks, samples) = {
        let mut m = live.meter.lock().unwrap_or_else(|p| p.into_inner());
        (m.finish().to_vec(), m.samples)
    };
    let size = std::fs::metadata(path).map(|x| x.len()).unwrap_or(0);
    let clean = torn_down && eos && error.is_none() && size > 0;
    let (samples, peaks, salvaged) = if !torn_down {
        (samples, peaks, false)
    } else if clean {
        (samples, peaks, false)
    } else {
        // salvage only what an independent decode of the file proves is there
        match decode_ogg(path).ok().filter(|&d| d > 0) {
            Some(d) => {
                let n = d.min(samples);
                let frames = (n.div_ceil(VOICE_FRAME as u64) as usize).min(peaks.len());
                (n, peaks[..frames].to_vec(), true)
            }
            None => (samples, peaks, false),
        }
    };
    let f = Finished {
        id,
        path: path.to_path_buf(),
        samples,
        duration_ms: duration_ms(samples, VOICE_RATE),
        waveform: waveform(&peaks, WAVEFORM_N),
        size,
        reason,
        capped: live.capped.load(Ordering::SeqCst),
        error,
        clean,
        salvaged,
    };
    eprintln!(
        "[voicenote] recording {id} finished ({reason:?}): {} samples, {} ms, {} bytes, {}",
        f.samples,
        f.duration_ms,
        f.size,
        if f.clean { "clean" } else if f.salvaged { "salvaged (decoded)" } else { "NOT sendable" }
    );
    f
}

/// Decode an Ogg/Opus file independently of the recorder's own count: the number of 48 kHz mono samples it yields.
pub fn decode_ogg(path: &Path) -> Result<u64, String> {
    decode_with(
        "filesrc name=f ! oggdemux ! opusdec ! audioconvert ! audioresample ! \
         audio/x-raw,format=S16LE,rate=48000,channels=1 ! appsink name=s sync=false emit-signals=false",
        path,
    )
}
/// review r2 #2: every error the decode posts is LATCHED (a sync handler, for the whole decode) and checked before an
/// EOS is accepted — GStreamer may post an error and then EOS; a decode that failed part-way never counts as one.
fn decode_with(desc: &str, path: &Path) -> Result<u64, String> {
    let _ = crate::ensure_init();
    let p = gst::parse::launch(desc).map_err(|e| e.to_string())?.downcast::<gst::Pipeline>().map_err(|_| "pipeline")?;
    p.by_name("f").ok_or("filesrc")?.set_property("location", path.to_string_lossy().to_string());
    let sink = p.by_name("s").and_then(|e| e.downcast::<gstreamer_app::AppSink>().ok()).ok_or("appsink")?;
    let latched: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    if let Some(bus) = p.bus() {
        let l = latched.clone();
        bus.set_sync_handler(move |_, msg| {
            if let gst::MessageView::Error(e) = msg.view() {
                let mut g = l.lock().unwrap_or_else(|p| p.into_inner());
                if g.is_none() {
                    *g = Some(e.error().to_string());
                }
            }
            gst::BusSyncReply::Drop
        });
    }
    let failed = || latched.lock().unwrap_or_else(|p| p.into_inner()).clone();
    if let Err(e) = p.set_state(gst::State::Playing) {
        let _ = p.set_state(gst::State::Null);
        return Err(e.to_string());
    }
    let mut n = 0u64;
    let t0 = std::time::Instant::now();
    let r = loop {
        if t0.elapsed() > Duration::from_secs(20) {
            break Err("decode timed out".to_string());
        }
        match sink.try_pull_sample(gst::ClockTime::from_mseconds(500)) {
            Some(s) => n += s.buffer().map(|b| b.size() as u64 / 2).unwrap_or(0),
            None => {
                if let Some(e) = failed() {
                    break Err(format!("decode: {e}"));
                }
                if sink.is_eos() {
                    break Ok(n);
                }
            }
        }
    };
    let _ = p.set_state(gst::State::Null);
    // an error latched at any point (also one posted after the last sample, next to the EOS) fails the decode
    match (r, failed()) {
        (_, Some(e)) => Err(format!("decode: {e}")),
        (r, None) => r,
    }
}

fn request(id: u64, r: Req) {
    let p = lock_phase();
    match &*p {
        Phase::Starting { id: i, cancel } if *i == id => cancel.store(true, Ordering::SeqCst),
        Phase::Running { id: i, req, .. } if *i == id => {
            let mut q = req.lock().unwrap_or_else(|p| p.into_inner());
            if *q != Some(Req::Cancel) {
                *q = Some(r);
            }
        }
        _ => {}
    }
}
/// Stop recording `id` into review (the result comes to the listener). A start still in progress is cancelled.
pub fn request_stop(id: u64) {
    request(id, Req::Stop);
}
/// Cancel recording `id`: NULL, the file is deleted, no result. Idempotent.
pub fn request_cancel(id: u64) {
    request(id, Req::Cancel);
}

/// The call's mic start calls this FIRST: whatever the recorder holds is asked to stop (a pending start is cancelled,
/// a running recording finalizes into review: Reason::Call) and this waits, at most `ms`, for the CONFIRMED release
/// (the recorder's pipeline in NULL). true = the microphone is free; false = not in time: the call must not open its
/// capture now (it fails / retries instead of overlapping).
pub fn release_within(ms: u64) -> bool {
    let (m, cv) = phase();
    let g = m.lock().unwrap_or_else(|p| p.into_inner());
    match &*g {
        Phase::Idle => return true,
        Phase::Starting { cancel, .. } => cancel.store(true, Ordering::SeqCst),
        Phase::Running { id, req, .. } => {
            eprintln!("[voicenote] a call needs the microphone — stopping recording {id} into review");
            let mut q = req.lock().unwrap_or_else(|p| p.into_inner());
            if *q != Some(Req::Cancel) {
                *q = Some(Req::Call);
            }
        }
        Phase::Stopping { .. } => {}
    }
    drop(g);
    // waits in slices, retrying a parked teardown each time (review r3 #1: also when no cleanup worker exists)
    let t0 = std::time::Instant::now();
    loop {
        retry_parked();
        let g = m.lock().unwrap_or_else(|p| p.into_inner());
        let left = Duration::from_millis(ms).saturating_sub(t0.elapsed());
        if matches!(*g, Phase::Idle) || left.is_zero() {
            break;
        }
        let _ = cv.wait_timeout_while(g, left.min(Duration::from_millis(200)), |p| !matches!(p, Phase::Idle));
    }
    let free = matches!(*lock_phase(), Phase::Idle);
    if !free {
        eprintln!("[voicenote] the recorder did not release the microphone within {ms} ms");
    }
    free
}
/// [`release_within`] with the call's bound.
pub fn release_for_call() -> bool {
    release_within(CALL_RELEASE_MS)
}

/// A watch join / Watch press closed the capture bar: the recording stops into review (its gate closed at the revoke).
pub fn retire_if_barred() {
    if crate::call::capture_barred() {
        let _ = release_within(CALL_RELEASE_MS);
    }
}

/// The running recording's live state (None = none running).
pub fn status() -> Option<Status> {
    let (id, live) = match &*lock_phase() {
        Phase::Running { id, live, .. } => (*id, live.clone()),
        _ => return None,
    };
    let recent: Vec<u32> = live.recent.lock().unwrap_or_else(|p| p.into_inner()).iter().copied().collect();
    Some(Status {
        id,
        samples: live.samples.load(Ordering::SeqCst),
        level: live.level.load(Ordering::Relaxed),
        recent,
        capped: live.capped.load(Ordering::SeqCst),
        failed: live.error(),
    })
}

/// Does the recorder hold (or is it opening / releasing) the microphone for recording `id`?
pub fn holds(id: u64) -> bool {
    match &*lock_phase() {
        Phase::Idle => false,
        Phase::Starting { id: i, .. } | Phase::Running { id: i, .. } | Phase::Stopping { id: i } => *i == id,
    }
}

/// Headless self-test (CLI): record `TINHAT_VOICENOTE_TEST` seconds from the app's chosen input (the real capture head)
/// into `TINHAT_VOICENOTE_OUT` (default /tmp/voicenote-selftest.ogg), started through `leased_start`, and print the
/// finished note as one JSON line on stdout. Exit 0 = a sendable note was written.
pub fn headless_voicenote_test() -> ! {
    let secs: f64 = std::env::var("TINHAT_VOICENOTE_TEST").ok().and_then(|v| v.parse().ok()).unwrap_or(5.0);
    let out = std::env::var("TINHAT_VOICENOTE_OUT").unwrap_or_else(|_| "/tmp/voicenote-selftest.ogg".into());
    let path = PathBuf::from(&out);
    let Some(cancel) = reserve(1) else { std::process::exit(2) };
    let (pipeline, live) = match build_recorder(&path) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[voicenote-test] build FAILED: {e}");
            std::process::exit(2);
        }
    };
    if !crate::call::leased_start(crate::capture_lease::Kind::VoiceNote, &pipeline, 6000, "voice note self-test") {
        eprintln!("[voicenote-test] start FAILED");
        std::process::exit(2);
    }
    if adopt(1, &cancel, pipeline, path, live).is_err() {
        std::process::exit(2);
    }
    let t0 = std::time::Instant::now();
    while t0.elapsed().as_secs_f64() < secs && status().is_some() {
        std::thread::sleep(Duration::from_millis(50));
    }
    request_stop(1);
    let Some(f) = wait_result(1, 10_000) else {
        eprintln!("[voicenote-test] no result");
        std::process::exit(1);
    };
    println!("{}", f.to_json());
    std::process::exit(if f.sendable() { 0 } else { 1 });
}

/// Ogg page reader (RFC 3533) for the tests' independent checks: CRC, lacing, flags, granules.
#[cfg(test)]
pub(crate) mod ogg {
    #[derive(Debug, Clone)]
    pub struct Page {
        pub flags: u8,
        pub granule: i64,
        pub serial: u32,
        pub seq: u32,
        pub packets_done: usize,
        pub body: Vec<u8>,
        pub lacing: Vec<u8>,
    }
    fn crc(data: &[u8]) -> u32 {
        let mut c: u32 = 0;
        for &b in data {
            c ^= (b as u32) << 24;
            for _ in 0..8 {
                c = if c & 0x8000_0000 != 0 { (c << 1) ^ 0x04C1_1DB7 } else { c << 1 };
            }
        }
        c
    }
    /// Every page of `data`; Err on a bad capture pattern, a bad CRC or a truncated page.
    pub fn pages(data: &[u8]) -> Result<Vec<Page>, String> {
        let mut out = vec![];
        let mut i = 0;
        while i < data.len() {
            if data.len() < i + 27 || &data[i..i + 4] != b"OggS" {
                return Err(format!("no page at {i}"));
            }
            let nseg = data[i + 26] as usize;
            let lacing = data.get(i + 27..i + 27 + nseg).ok_or("truncated lacing")?.to_vec();
            let blen: usize = lacing.iter().map(|&x| x as usize).sum();
            let end = i + 27 + nseg + blen;
            let page = data.get(i..end).ok_or("truncated body")?;
            let mut z = page.to_vec();
            z[22..26].copy_from_slice(&[0, 0, 0, 0]);
            let want = u32::from_le_bytes([page[22], page[23], page[24], page[25]]);
            if crc(&z) != want {
                return Err(format!("bad CRC on the page at {i}"));
            }
            out.push(Page {
                flags: page[5],
                granule: i64::from_le_bytes(page[6..14].try_into().unwrap()),
                serial: u32::from_le_bytes(page[14..18].try_into().unwrap()),
                seq: u32::from_le_bytes(page[18..22].try_into().unwrap()),
                packets_done: lacing.iter().filter(|&&x| x < 255).count(),
                body: page[27 + nseg..].to_vec(),
                lacing,
            });
            i = end;
        }
        Ok(out)
    }
    #[test]
    fn crc_matches_an_independent_value() {
        // RFC 3533's CRC (poly 0x04C11DB7, no reflection, init 0): the CRC-32/MPEG-2-like check value without the
        // final XOR / init inversion is 0x89A1897F for "123456789" (CRC-32/BZIP2 = 0xFC891918 has init+xorout).
        assert_eq!(crc(b"123456789"), 0x89A1_897F);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_delete_retries_through_a_sharing_violation_and_gives_up_visibly() {
        let p = Path::new("voice-x.ogg");
        let mut left = 3;
        let mut busy = |_: &Path| -> std::io::Result<()> {
            if left > 0 { left -= 1; Err(std::io::Error::from_raw_os_error(32)) } else { Ok(()) }
        };
        assert!(remove_retrying(p, 10, Duration::from_millis(1), &mut busy), "gone once the handle closes");
        let mut stuck = |_: &Path| -> std::io::Result<()> { Err(std::io::Error::from_raw_os_error(32)) };
        assert!(!remove_retrying(p, 4, Duration::from_millis(1), &mut stuck), "reports that it stayed");
        let mut gone = |_: &Path| -> std::io::Result<()> { Err(std::io::ErrorKind::NotFound.into()) };
        assert!(remove_retrying(p, 4, Duration::from_millis(1), &mut gone), "already gone counts as done");
    }

    /// launch-gate 2026-11-07 (review item 6): Windows refuses to delete a file `filesink` still holds, so a cancelled
    /// recording releases the pipeline first and only then deletes the draft.
    #[test]
    fn a_cancelled_draft_is_deleted_after_the_pipeline_lets_go() {
        let src = include_str!("voicenote.rs");
        let at = src.find("if why == Req::Cancel {").unwrap();
        let b = &src[at..at + 500];
        let rel = b.find("release(id, pipeline)").expect("released");
        let del = b.find("delete_draft(&path)").expect("the draft goes through delete_draft");
        assert!(rel < del, "release first, then delete");
    }

    use super::*;

    /// the recorder's global phase: these tests take turns
    fn turn() -> std::sync::MutexGuard<'static, ()> {
        static T: Mutex<()> = Mutex::new(());
        T.lock().unwrap_or_else(|p| p.into_inner())
    }

    #[test]
    fn the_cap_is_fifteen_minutes_of_samples() {
        assert_eq!(CAP_SAMPLES, 43_200_000);
    }

    #[test]
    fn the_chain_is_the_designed_one() {
        let d = recorder_desc("audiotestsrc is-live=true", false);
        assert!(!d.contains("audiornnoise"));
        let d = recorder_desc("audiotestsrc is-live=true", true);
        // denoise sits before the tee, so both branches carry the same samples
        let rn = d.find("audiornnoise").unwrap();
        let tee = d.find("tee name=vnt").unwrap();
        assert!(rn < tee);
        assert!(d.contains("appsink name=vnmeter emit-signals=false drop=false sync=false"));
        assert!(d.contains("format=S16LE,rate=48000,channels=1,layout=interleaved ! opusenc name=vnenc ! oggmux"));
        // no branch may drop: every queue is leaky=no
        assert_eq!(d.matches("queue").count(), d.matches("leaky=no").count());
        assert!(!d.contains("leaky=downstream") && !d.contains("leaky=upstream"));
        // no echo canceller / AGC in the recorder
        assert!(!d.contains("webrtcdsp"));
    }

    /// review r1 #2: the call's capture waits for the CONFIRMED release, and gives up (false) at its bound
    #[test]
    fn a_call_waits_for_the_confirmed_release_and_fails_at_its_bound() {
        let _t = turn();
        let _ = gst::init(); // (it builds an empty pipeline; alone, no other test initialised GStreamer)
        set_idle();
        assert!(release_within(10), "idle: free at once");
        // releasing (a stop in progress, its NULL not done): not free until Idle
        *lock_phase() = Phase::Stopping { id: 9 };
        let t0 = std::time::Instant::now();
        assert!(!release_within(150), "a stop still releasing the device is NOT a free microphone");
        assert!(t0.elapsed() >= Duration::from_millis(140));
        let h = std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(100));
            set_idle();
        });
        assert!(release_within(2000), "free as soon as the release is confirmed");
        h.join().unwrap();
        // a start in progress: cancelled, and the call waits for it too
        let cancel = reserve(10).unwrap();
        assert!(!release_within(100));
        assert!(cancel.load(Ordering::SeqCst), "the pending start is cancelled");
        assert!(adopt(10, &cancel, gst::Pipeline::new(), PathBuf::from("/nonexistent"), Arc::new(Live::default())).is_err(), "a cancelled start never becomes a recording");
        set_idle();
        // a running recording is asked to stop for the call (and nothing else may start meanwhile)
        let req = Arc::new(Mutex::new(None));
        *lock_phase() = Phase::Running { id: 11, live: Arc::new(Live::default()), req: req.clone() };
        assert!(reserve(12).is_none(), "one owner of the microphone at a time");
        assert!(!release_within(50));
        assert_eq!(*req.lock().unwrap(), Some(Req::Call));
        set_idle();
    }

    /// review r2 #1: a worker that cannot be created leaves the REAL pipeline with us: it goes to NULL, and only its own
    /// reservation is released (a late cleanup never erases a newer one)
    #[test]
    fn a_failed_worker_spawn_shuts_the_real_pipeline_down() {
        let _t = turn();
        let _ = gst::init();
        set_idle();
        let cancel = reserve(201).unwrap();
        let p = gst::parse::launch("audiotestsrc is-live=true ! fakesink sync=false").unwrap().downcast::<gst::Pipeline>().unwrap();
        assert!(crate::call::leased_start(crate::capture_lease::Kind::VoiceNote, &p, 6000, "spawn-failure test"));
        let _ = p.state(gst::ClockTime::from_seconds(5)); // (a live source finishes its async change to PLAYING)
        assert_eq!(p.current_state(), gst::State::Playing);
        FAIL_SPAWN.store(true, Ordering::SeqCst);
        assert!(adopt(201, &cancel, p.clone(), PathBuf::from("/nonexistent"), Arc::new(Live::default())).is_err());
        assert_eq!(p.current_state(), gst::State::Null, "the capture pipeline itself was shut down");
        assert!(release_within(10), "and only then the microphone was released");
        // generation check: a stale release never frees a newer reservation
        let _c = reserve(202).unwrap();
        assert!(!set_idle_if(201));
        assert!(reserve(203).is_none(), "202 still owns the microphone");
        assert!(set_idle_if(202));
        assert!(release_within(10));
    }

    /// review r2 #3: a NULL transition that is not confirmed keeps the ownership (and the pipeline); the call's wait runs
    /// into its bound; the release comes only once a retry confirms NULL
    #[test]
    fn a_failed_null_keeps_the_microphone_owned() {
        let _t = turn();
        let _ = gst::init();
        set_idle();
        let _c = reserve(301).unwrap();
        FAIL_NULL.store(1_000_000, Ordering::SeqCst); // every NULL fails until the device "lets go"
        assert!(!release(301, gst::Pipeline::new()));
        assert!(!release_within(200), "NULL not confirmed: the microphone is NOT free");
        assert!(reserve(302).is_none(), "nobody else may take it meanwhile");
        assert!(!wait_released(301, 300), "and nothing waiting on it (a finish) goes on as if it were released");
        FAIL_NULL.store(0, Ordering::SeqCst);
        assert!(release_within(4000), "free once a retry confirmed NULL");
        // a cancel goes the same way: the worker's cancel path releases through `release`
        set_idle();
    }

    /// review r3 #1: without a cleanup worker the pipeline is never dropped unconfirmed — it is parked, retried
    /// synchronously (bounded) and by every call wait, and released only once NULL is confirmed
    #[test]
    fn without_a_cleanup_worker_the_pipeline_stays_parked_until_null_is_confirmed() {
        let _t = turn();
        let _ = gst::init();
        set_idle();
        let _c = reserve(401).unwrap();
        FAIL_NULL.store(1_000_000, Ordering::SeqCst);
        let t0 = std::time::Instant::now();
        assert!(!release_with(401, gst::Pipeline::new(), false), "not released");
        assert!(t0.elapsed() >= Duration::from_millis(900), "it retried synchronously first (bounded)");
        assert!(parked().lock().unwrap().iter().any(|(id, _)| *id == 401), "the pipeline is held, not dropped");
        assert!(owned_by(401) && !release_within(100));
        FAIL_NULL.store(0, Ordering::SeqCst);
        assert!(release_within(2000), "a call wait's own retry confirms NULL and frees it");
        assert!(!parked().lock().unwrap().iter().any(|(id, _)| *id == 401));
    }

    /// review r2 #2: valid audio followed by a decode failure is a failed decode, never a salvage
    #[test]
    fn decode_failure_after_valid_audio_is_refused() {
        if gst::init().is_err() || gst::ElementFactory::find("opusenc").is_none() || gst::ElementFactory::find("opusdec").is_none() {
            return;
        }
        let path = std::env::temp_dir().join(format!("vn-dfail-{}.ogg", std::process::id()));
        let enc = gst::parse::launch(&format!(
            "audiotestsrc num-buffers=100 samplesperbuffer=960 ! audio/x-raw,rate=48000,channels=1 ! audioconvert ! opusenc ! oggmux ! filesink location={}",
            path.display()
        ))
        .unwrap();
        enc.set_state(gst::State::Playing).unwrap();
        let _ = enc.bus().unwrap().timed_pop_filtered(gst::ClockTime::from_seconds(10), &[gst::MessageType::Eos, gst::MessageType::Error]);
        let _ = enc.set_state(gst::State::Null);
        assert_eq!(decode_ogg(&path), Ok(96_000), "the file itself decodes");
        let failing = "filesrc name=f ! oggdemux ! opusdec ! identity error-after=20 ! audioconvert ! audioresample ! \
                       audio/x-raw,format=S16LE,rate=48000,channels=1 ! appsink name=s sync=false emit-signals=false";
        let r = decode_with(failing, &path);
        assert!(r.is_err(), "audio, then an error: not a decode ({r:?})");
        let _ = std::fs::remove_file(&path);
    }

    struct Rec {
        samples: u64,
        f: Finished,
        bytes: Vec<u8>,
    }

    /// The PRODUCTION instrumentation (cap probe, meter, bus handler, encoder settings), worker and finish, on a
    /// synthetic head, started through leased_start. `stop`: ask for the stop once every sample went through; else the
    /// cap must stop it by itself.
    fn record(head: &str, total: u64, cap: u64, stop: bool, id: u64) -> Rec {
        let dir = std::env::temp_dir().join(format!("vn-test-{}-{id}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("voice-test.ogg");
        let cancel = reserve(id).expect("idle");
        let (p, live) = instrument(&recorder_desc(head, false), &path, cap).unwrap();
        assert!(crate::call::leased_start(crate::capture_lease::Kind::VoiceNote, &p, 6000, "voice note test"));
        adopt(id, &cancel, p, path.clone(), live.clone()).unwrap();
        if stop {
            let t0 = std::time::Instant::now();
            while live.samples.load(Ordering::SeqCst) < total && t0.elapsed() < Duration::from_secs(10) {
                std::thread::sleep(Duration::from_millis(10));
            }
            request_stop(id);
        }
        let f = wait_result(id, 15_000).expect("a result");
        assert!(release_within(10), "the device is released once the result is out");
        let bytes = std::fs::read(&path).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        Rec { samples: f.samples, f, bytes }
    }

    /// Independent checks of the file: every page's CRC, lacing, BOS/EOS, the OpusHead pre-skip and the final granule
    /// (= pre-skip + the samples captured: the end trim), then a decode by a separate pipeline.
    fn check_file(r: &Rec, want: u64) {
        let pages = ogg::pages(&r.bytes).expect("valid Ogg pages (CRC, lengths)");
        assert!(pages.len() >= 3);
        assert_eq!(pages[0].flags & 2, 2, "BOS on the first page");
        assert_eq!(pages.last().unwrap().flags & 4, 4, "EOS on the last page");
        assert!(pages.iter().all(|p| p.serial == pages[0].serial));
        for (i, p) in pages.iter().enumerate() {
            assert_eq!(p.seq as usize, i, "page sequence");
            assert_eq!(p.body.len(), p.lacing.iter().map(|&x| x as usize).sum::<usize>(), "lacing adds up");
        }
        assert_eq!(&pages[0].body[..8], b"OpusHead");
        let pre_skip = u16::from_le_bytes([pages[0].body[10], pages[0].body[11]]) as i64;
        assert!(pre_skip > 0);
        assert_eq!(&pages[1].body[..8], b"OpusTags");
        let last = pages.last().unwrap().granule;
        assert_eq!((last - pre_skip) as u64, want, "final granule − pre-skip = the samples captured");
        let path = std::env::temp_dir().join(format!("vn-dec-{}-{}.ogg", std::process::id(), want));
        std::fs::write(&path, &r.bytes).unwrap();
        let dec = decode_ogg(&path).expect("decodes");
        let _ = std::fs::remove_file(&path);
        assert_eq!(dec, want, "an independent decode yields exactly the samples captured");
    }

    /// review r1 #9: boundary / off-boundary / cap-truncated recordings through the production chain, decoded back
    #[test]
    fn notes_round_trip_through_the_production_chain() {
        if gst::init().is_err() || gst::ElementFactory::find("opusenc").is_none() || gst::ElementFactory::find("opusdec").is_none() {
            eprintln!("skipped: no opusenc/opusdec here");
            return;
        }
        let _t = turn();
        set_idle();
        // on a 20 ms packet boundary: 50 × 960 = 48 000 samples
        // (live sources: a non-live one can push its whole stream while the lease gate is still closed, so the counts
        // are set by the production cap probe — exact, whatever the gate let through first)
        // on a 20 ms packet boundary: the cap at 48 000 samples, 960 per buffer — it stops by ITSELF (no request, no view)
        let r = record("audiotestsrc is-live=true samplesperbuffer=960 wave=sine freq=440 volume=0.5", 0, 48_000, false, 101);
        assert!(r.f.clean && r.f.sendable() && r.f.capped && r.f.reason == Reason::Cap, "{:?}", r.f);
        assert_eq!(r.samples, 48_000);
        assert_eq!(r.f.duration_ms, 1000);
        assert_eq!(r.f.waveform.len(), 100);
        assert!(r.f.waveform.iter().all(|&w| w > 900), "a steady tone fills the waveform: {:?}", r.f.waveform);
        check_file(&r, 48_000);
        // off a boundary: the cap at 25 641 samples cuts a 333-sample buffer part-way
        let r = record("audiotestsrc is-live=true samplesperbuffer=333 wave=sine freq=300 volume=0.3", 0, 25_641, false, 102);
        assert!(r.f.clean && r.f.capped, "{:?}", r.f);
        assert_eq!(r.samples, 25_641);
        assert_eq!(r.f.duration_ms, 534);
        check_file(&r, 25_641);
        // a user Stop at an arbitrary point: the file holds exactly what the meter counted
        let r = record("audiotestsrc is-live=true samplesperbuffer=480 wave=sine freq=500 volume=0.4", 20_000, CAP_SAMPLES, true, 103);
        assert!(r.f.clean && !r.f.capped && r.f.reason == Reason::Stop, "{:?}", r.f);
        assert!(r.samples >= 20_000);
        check_file(&r, r.samples);
    }

    /// review r1 #4: a file that never got its EOS is not sendable unless an independent decode proves what is there
    #[test]
    fn an_unfinished_file_is_salvaged_only_by_a_decode() {
        if gst::init().is_err() || gst::ElementFactory::find("opusdec").is_none() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("vn-salv-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let junk = dir.join("junk.ogg");
        std::fs::write(&junk, b"OggS not really").unwrap();
        assert!(decode_ogg(&junk).is_err() || decode_ogg(&junk) == Ok(0), "garbage never decodes to audio");
        let f = Finished {
            id: 1, path: junk, samples: 48_000, duration_ms: 1000, waveform: vec![1], size: 15, reason: Reason::Error,
            capped: false, error: Some("device gone".into()), clean: false, salvaged: false,
        };
        assert!(!f.sendable(), "not clean and not salvaged: not sendable");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
