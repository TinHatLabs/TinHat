//! Memories LIVE side tiles (0.8.13): one muted, VIDEO-ONLY playback
//! pipeline per side tile in the Memories relive grid, rendered by the SAME
//! per-platform machinery as live call tiles (video_bridge attach: qml6glsink on
//! Linux, appsink -> QVideoFrame -> QML VideoOutput on Windows).
//!
//! Why: the 0.8.10 side tiles were /vods/thumb ffmpeg stills refreshed every 2s —
//! a field report: "the side car feeds are 1fps" — and N tiles x a fresh ffmpeg every 2s
//! flattened the server's 2 cores (the field "server replied:" empty-image errors).
//! mpvqt reliably paints only ONE surface per process, so more mpvs was never an
//! option; gst tiles are exactly how the live call grid already renders N videos.
//! The QML keeps the thumbnail path as a FALLBACK when create() fails (e.g. a
//! machine missing a gst playback plugin).
//!
//! Threading: create/attach/sync/retire are called from Qt GUI-thread invokables
//! (qml6glsink affinity — same rule as call.rs tiles). retire() moves the pipeline
//! to NULL on a DETACHED thread (mirrors call.rs rebuild_video_tile: a GUI-thread
//! NULL can deadlock against the render thread; if gst ever wedges we leak one
//! idle pipeline instead of freezing the app).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use anyhow::{Context, Result};
use gst::prelude::*;
use gstreamer as gst;

struct TileHandle {
    pipeline: gst::Pipeline,
    // NULL posts no EOS, so the bus thread would block on timed_pop forever without
    // this flag (same leak call.rs's VideoHandle.stop plugs).
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    // Set by the bus thread on a pipeline ERROR (e.g. "Missing decoder: H.264" on a
    // box without gst-libav). The QML health poll reads it via is_dead() and flips
    // the tile to the thumbnail fallback — creation succeeding says nothing about
    // whether the pipeline can actually run.
    dead: std::sync::Arc<std::sync::atomic::AtomicBool>,
    // Buffers that reached the sink pad (the probe increments it). The health poll's
    // PRIMARY signal: the decoder-less repro wedged in preroll with NO bus error, so
    // "attached + playing + zero frames after a grace period" is the honest test.
    frames: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

fn tiles() -> &'static Mutex<HashMap<String, TileHandle>> {
    static T: OnceLock<Mutex<HashMap<String, TileHandle>>> = OnceLock::new();
    T.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Build + register a muted VIDEO-ONLY playback pipeline for `url` under `id`,
/// replacing any previous pipeline for that id (focus changes reshuffle which track
/// occupies which slot). Returns the pipeline ptr for the attach shim. MUST run on
/// the Qt GUI thread. The pipeline starts in NULL; the attach dance takes it up.
pub fn create(id: &str, url: &str) -> Result<usize> {
    crate::ensure_init()?;
    // Validate BEFORE any side effect: this check used to sit below retire(id), so a
    // rejected url destroyed the healthy previous tile it was meant to replace.
    if url.contains('"') || url.contains('\\') || url.contains(char::is_whitespace) {
        anyhow::bail!("memtile: refusing url with quote/backslash/whitespace");
    }
    kick_probe();
    if probe_state() == 2 {
        // The REAL test decode failed (see kick_probe): a pipeline that only fails at
        // runtime (error or a silent preroll wedge) is worse than no pipeline — its
        // teardown can stop the whole window painting, burying even the thumbnail
        // fallback (the decoder gate). Unknown (0, probe still in flight) proceeds
        // OPTIMISTICALLY like Ok (1): the QML health poll (is_dead + frame_count)
        // stays the backstop for any tile built before a slow probe finishes.
        anyhow::bail!("no H.264 decoder in the gst registry (install gst-libav)");
    }
    retire(id); // idempotent slot reuse
    // caps="video/x-raw(ANY)" + a `video/x-raw` capsfilter AFTER decodebin (0.8.15):
    // decodebin matches its `caps` against each candidate decoder's src TEMPLATE
    // with gst_caps_is_subset(). Current gst-libav advertises non-system-memory
    // structures on avdec_* too, so the old `caps=video/x-raw` made decodebin
    // discard the software decoder as "not final caps" before ever plugging it —
    // the field "Missing decoder: H.264 (… stream-format=avc)", reproduced
    // on the build box. Accepting ANY raw video at the decodebin boundary and
    // pinning SYSTEM memory by negotiation downstream gives the per-platform tails
    // plain raw frames every time (HW decoders that can't are deranked at init —
    // lib.rs derank_offsystem_video_decoders). ≤5 small tiles, so software decode
    // is acceptable. expose-all-streams=false: the raw capture tracks carry no
    // audio today, but never rely on that — the mixed master audio lives in the
    // focused mpv, and a side tile that also decoded audio would fight the master
    // clock (and, headless-gate trap, black mpv). sync=true: this is clocked FILE
    // playback (live call tiles run sync=false because SFU frames are already paced).
    // 0.8.16 tail: side tiles are SMALL (a fifth of the window at most), so decode
    // output is rate-capped at 30 fps and scaled into a <=960x540 box BEFORE it
    // reaches the display path — a 2560x1440@60 share no longer costs a full-size
    // colour conversion + upload per frame. Linux keeps the 0.8.15 I420 → glupload →
    // glcolorconvert → qml6glsink shape (0.8.17): 0.8.16 tried a single-plane RGBA
    // frame into the sink and a Wayland/GLES2/NVIDIA field box painted every tile
    // solid WHITE while frames flowed — glcolorconvert becomes a passthrough
    // for RGBA and the sink then samples glupload's raw texture instead of a
    // converter-made one; I420 demonstrably rendered there. The green-frame-at-
    // start that motivated RGBA is covered by the veil-until-synced + output-corrupt
    // =false instead. Windows keeps I420 for its QVideoFrame path (video_bridge).
    // 0.8.19 tail — ONE shape on both platforms: 30 fps cap, scaled into a <=960x540
    // box in SYSTEM memory (appsink proposes no GL pool), plain appsink. The C++
    // bridge copies each I420 buffer into a QVideoFrame for a QML VideoOutput
    // (QtMultimedia) — no GstGL anywhere in the tile path. History: the GstGL tail
    // (glupload ! glcolorconvert ! qml6glsink) painted GREEN (0.8.15), WHITE (0.8.16,
    // RGBA), STRIPED (0.8.17, scaled I420) and nothing (0.8.18, decoder-pool I420) on
    // a Wayland/GLES2/NVIDIA field box while frames flowed every time (0.8.15–0.8.18);
    // the llvmpipe/GLX gate box rendered all of them. Live call tiles keep qml6glsink.
    let tail = "videorate drop-only=true max-rate=30 ! videoconvertscale n-threads=2 ! \
         video/x-raw,format=I420,width=(int)[1,960],height=(int)[1,540],pixel-aspect-ratio=1/1 ! \
         appsink name=sink emit-signals=false max-buffers=2 drop=true sync=true";
    let desc = format!(
        "uridecodebin uri=\"{url}\" expose-all-streams=false caps=\"video/x-raw(ANY)\" ! \
         queue max-size-buffers=8 ! video/x-raw ! {tail}"
    );
    let pipeline = gst::parse::launch(&desc)
        .with_context(|| format!("memtile parse/plugin failure ({desc})"))?
        .downcast::<gst::Pipeline>()
        .map_err(|_| anyhow::anyhow!("memtile: parsed element is not a Pipeline"))?;
    // Never paint pre-recovery frames: gst-libav's avdec_* default output-corrupt=true
    // emits frames decoded before the first keyframe (zeroed chroma = the classic green
    // frame with coloured blotches). decodebin autoplugs the decoder later, so hook
    // deep-element-added and flip the property on whatever avdec_* shows up.
    pipeline.connect("deep-element-added", false, |args| {
        if let Some(el) = args.get(2).and_then(|v| v.get::<gst::Element>().ok()) {
            let is_avdec = el
                .factory()
                .map(|f| f.name().starts_with("avdec_"))
                .unwrap_or(false);
            if is_avdec && el.find_property("output-corrupt").is_some() {
                el.set_property("output-corrupt", false);
            }
        }
        None
    });
    // Frame-flow evidence for gates + field logs: the first buffer proves the tile
    // actually renders; the 300-frame cadence line makes "is it really ~30fps?"
    // checkable from log timestamps (a field lesson: smoothness reports need NUMBERS).
    let frames = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    if let Some(sink) = pipeline.by_name("sink") {
        if let Some(pad) = sink.static_pad("sink") {
            let counter = frames.clone();
            let label = id.to_string();
            pad.add_probe(gst::PadProbeType::BUFFER, move |_p, _info| {
                let n = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if n == 0 {
                    eprintln!("[memtile] {label}: first frame rendered");
                } else if n % 300 == 0 {
                    eprintln!("[memtile] {label}: {n} frames");
                }
                gst::PadProbeReturn::Ok
            });
        }
    }
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let dead = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    if let Some(bus) = pipeline.bus() {
        let label = id.to_string();
        let stop_w = stop.clone();
        let dead_w = dead.clone();
        std::thread::spawn(move || loop {
            if stop_w.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }
            let Some(msg) = bus.timed_pop(gst::ClockTime::from_mseconds(500)) else { continue };
            use gst::MessageView;
            match msg.view() {
                MessageView::Error(e) => {
                    // The first-attach "Could not initialize window system" is the
                    // DESIGNED SetPlaying sink-READY retry (video_bridge loops until
                    // the item has rendered) — recovery is normal, don't kill the
                    // tile for it. Everything else (missing decoder, TLS, 404) is
                    // fatal for this pipeline: flag it so QML falls back.
                    let s = e.error().to_string();
                    if !s.contains("Could not initialize window system") {
                        dead_w.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                    eprintln!("[memtile] {label} gst ERROR: {} ({:?})", e.error(), e.debug());
                    // No break: keep draining until the stop flag. Exiting on the
                    // first Error froze `dead` at whatever that first message was —
                    // late errors (TLS/404 on a later range read) and revive seeks
                    // after one must keep `dead` accurate.
                }
                MessageView::Warning(w) => eprintln!("[memtile] {label} gst WARN: {}", w.error()),
                // EOS: tile reached the end of its capture — keep the last frame on
                // screen; a later master scrub-back revives it via a flush seek — and
                // a REVIVED pipeline posts more messages, so no break here either.
                // This thread exits solely via the stop flag (retire()).
                MessageView::Eos(_) => {}
                _ => {}
            }
        });
    }
    let ptr = pipeline.as_ptr() as usize;
    tiles()
        .lock()
        .unwrap()
        .insert(id.to_string(), TileHandle { pipeline, stop, dead, frames });
    eprintln!("[memtile] {id} pipeline created");
    Ok(ptr)
}

/// Canned 2-frame 64x64 black H.264 annex-B sample (openh264enc, byte-stream; starts
/// 00 00 00 01, ~67 bytes) generated on the build box and committed — the probe's
/// input never depends on the network or on what's installed where the app runs.
const PROBE_SAMPLE: &[u8] = include_bytes!("../assets/probe.h264");

/// Real-decode probe verdict. Unknown = the OnceLock still unset (probe in flight or
/// never kicked).
#[derive(Clone, Copy, PartialEq, Eq)]
enum ProbeVerdict {
    Ok,
    Broken,
}

static PROBE: OnceLock<ProbeVerdict> = OnceLock::new();
static PROBE_KICKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Start (once) the REAL H.264 test decode, asynchronously. The old
/// factories_with_type registry probe FAILED in the field: a registered-but-unusable
/// decoder (vah264dec on NVIDIA without VA drivers; byte-stream-only decoders against
/// our avc streams) passes a factory-existence check, then decodebin still errors
/// "Missing decoder: H.264" at runtime. The only honest test is pushing real bytes
/// through decodebin and seeing decoded frames come out. NO caps filter on the probe:
/// with `caps=video/x-raw` decodebin fixed nvh264dec's output to CUDAMemory and then
/// reported "Missing decoder: … video/x-raw(memory:CUDAMemory)" on the test rig — where the
/// live tiles demonstrably work — a false-Broken that kills the feature on capable
/// machines. Any decoded raw video (system, GL or CUDA memory) proves
/// the decoder runs; the tiles' own uridecodebin negotiates memory for itself, and
/// the frame-health poll remains the backstop for a decoder that runs but paints
/// nothing. Async on a std::thread because a broken stack can take the full 3s
/// timeout to say so — never block the GUI thread.
///
/// The registry probe survives ONLY as an instant-negative pre-check: zero registered
/// H.264 decoders is a guaranteed Broken with no thread or 3s wait. A PASSING
/// pre-check proves nothing (that was the field bug) — only the real decode says Ok.
pub fn kick_probe() {
    if PROBE.get().is_some() || PROBE_KICKED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    if crate::ensure_init().is_err() {
        let _ = PROBE.set(ProbeVerdict::Broken);
        return;
    }
    if !h264_decoder_registered() {
        eprintln!("[memtile] probe: no H.264 decoder registered — side tiles will use thumbnails");
        let _ = PROBE.set(ProbeVerdict::Broken);
        return;
    }
    std::thread::spawn(|| {
        let verdict = match run_probe_decode() {
            Ok(()) => {
                eprintln!("[memtile] probe: H.264 test decode OK");
                ProbeVerdict::Ok
            }
            Err(e) => {
                eprintln!(
                    "[memtile] probe: H.264 test decode FAILED — side tiles will use \
                     thumbnails ({e:#})"
                );
                ProbeVerdict::Broken
            }
        };
        let _ = PROBE.set(verdict);
    });
}

/// Probe verdict for QML: 0 = unknown (still probing / never kicked), 1 = ok,
/// 2 = broken. create() bails only on 2; 0 proceeds optimistically (health poll is
/// the backstop).
pub fn probe_state() -> i32 {
    match PROBE.get() {
        None => 0,
        Some(ProbeVerdict::Ok) => 1,
        Some(ProbeVerdict::Broken) => 2,
    }
}

/// Decode the canned sample through decodebin: PLAYING, push the whole sample as ONE
/// buffer, EOS, then poll the bus up to 3s. AsyncDone (prerolled — a decoded frame
/// reached the sink) or Eos without a prior Error = Ok; any Error = Broken. The 3s
/// expiring with neither resolves OPTIMISTICALLY (Ok): a false-Broken silently removes
/// live tiles on a working machine, whereas a false-Ok is caught within ~6s by the
/// per-tile frame-health poll (the decoder-less wedge posts no bus error but
/// also renders no frames, and that poll is what flips it to thumbnails).
fn run_probe_decode() -> Result<()> {
    let pipeline = gst::parse::launch(
        "appsrc name=src caps=video/x-h264,stream-format=byte-stream ! h264parse ! \
         decodebin ! video/x-raw ! fakesink sync=false",
    )
    .context("probe: parse/plugin failure")?
    .downcast::<gst::Pipeline>()
    .map_err(|_| anyhow::anyhow!("probe: parsed element is not a Pipeline"))?;
    // Closure so every exit path below still NULLs the pipeline afterwards.
    let res = (|| -> Result<()> {
        let src = pipeline
            .by_name("src")
            .context("probe: no appsrc")?
            .downcast::<gstreamer_app::AppSrc>()
            .map_err(|_| anyhow::anyhow!("probe: src is not an AppSrc"))?;
        pipeline
            .set_state(gst::State::Playing)
            .context("probe: PLAYING refused")?;
        src.push_buffer(gst::Buffer::from_slice(PROBE_SAMPLE))
            .map_err(|e| anyhow::anyhow!("probe: push_buffer: {e:?}"))?;
        let _ = src.end_of_stream();
        let bus = pipeline.bus().context("probe: no bus")?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                eprintln!(
                    "[memtile] probe: no verdict within 3s — assuming OK (the tile \
                     frame-health poll is the backstop)"
                );
                return Ok(());
            }
            let Some(msg) = bus.timed_pop(gst::ClockTime::from_mseconds(left.as_millis() as u64))
            else {
                continue;
            };
            use gst::MessageView;
            match msg.view() {
                MessageView::Error(e) => {
                    anyhow::bail!("probe: gst ERROR: {} ({:?})", e.error(), e.debug())
                }
                MessageView::Eos(_) | MessageView::AsyncDone(_) => return Ok(()),
                _ => {}
            }
        }
    })();
    let _ = pipeline.set_state(gst::State::Null);
    res
}

/// Instant-NEGATIVE pre-check only: is ANY autopluggable (rank > NONE) H.264 video
/// decoder registered? Empty = guaranteed Broken. Non-empty proves NOTHING — a
/// registered-but-unusable decoder passes this and still fails the real decode.
fn h264_decoder_registered() -> bool {
    let caps = gst::Caps::builder("video/x-h264").build();
    gst::ElementFactory::factories_with_type(
        gst::ElementFactoryType::DECODER | gst::ElementFactoryType::MEDIA_VIDEO,
        gst::Rank::MARGINAL,
    )
    .iter()
    .any(|f| f.can_sink_any_caps(&caps))
}

/// Buffers that have reached tile `id`'s sink (-1 = no such tile). The QML health
/// poll treats "attached + playing + still 0 after the grace period" as dead.
pub fn frame_count(id: &str) -> i64 {
    tiles()
        .lock()
        .unwrap()
        .get(id)
        .map(|h| h.frames.load(std::sync::atomic::Ordering::Relaxed).min(i64::MAX as u64) as i64)
        .unwrap_or(-1)
}

/// Has tile `id`'s pipeline hit a fatal runtime error (bus ERROR)? Missing tiles
/// count as dead. The QML health poll uses this to engage the thumbnail fallback.
pub fn is_dead(id: &str) -> bool {
    tiles()
        .lock()
        .unwrap()
        .get(id)
        .map(|h| h.dead.load(std::sync::atomic::Ordering::Relaxed))
        .unwrap_or(true)
}

/// Pipeline ptr for the attach shim (`video::attach`), or None if `id` has no tile.
pub fn pipeline_ptr(id: &str) -> Option<usize> {
    tiles().lock().unwrap().get(id).map(|h| h.pipeline.as_ptr() as usize)
}

/// Land tile `id` at `target` seconds (its own file's timeline). `hard` always
/// seeks (scrub commit / focus swap / remote seek); soft is the 5s drift check —
/// seek only when drifted > 0.75s, and leave a tile that can't answer a position
/// query alone (still prerolling). ACCURATE (0.8.16): the recorder writes a keyframe
/// every ~2.5s, so an exact landing costs at most one GOP of decode-forward (~10ms
/// measured on the 93-min field session) — whereas KEY_UNIT landed up to a whole
/// GOP EARLY, the 0.75s drift check then re-seeked, landed early again, and every
/// side tile rewound a second or two every 5s for the entire replay.
pub fn sync_to(id: &str, target: f64, hard: bool) {
    let reg = tiles().lock().unwrap();
    let Some(h) = reg.get(id) else { return };
    if !target.is_finite() {
        return;
    }
    let t = target.max(0.0);
    if !hard {
        match h.pipeline.query_position::<gst::ClockTime>() {
            Some(pos) => {
                let cur = pos.nseconds() as f64 / 1e9;
                if (cur - t).abs() < 0.75 {
                    return;
                }
            }
            None => return,
        }
    }
    let _ = h.pipeline.seek_simple(
        gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
        gst::ClockTime::from_nseconds((t * 1e9) as u64),
    );
}

/// Current position of tile `id` in seconds of its own file, or -1.0 when the
/// pipeline can't answer yet (prerolling) / no such tile.
pub fn position(id: &str) -> f64 {
    let reg = tiles().lock().unwrap();
    let Some(h) = reg.get(id) else { return -1.0 };
    match h.pipeline.query_position::<gst::ClockTime>() {
        Some(pos) => pos.nseconds() as f64 / 1e9,
        None => -1.0,
    }
}

/// PAUSED (true) / PLAYING (false) — follows the master transport.
pub fn set_paused(id: &str, paused: bool) {
    let reg = tiles().lock().unwrap();
    if let Some(h) = reg.get(id) {
        let _ = h
            .pipeline
            .set_state(if paused { gst::State::Paused } else { gst::State::Playing });
    }
}

/// Drop tile `id`: stop its bus thread, retire the pipeline to NULL on a DETACHED
/// thread (see module docs). Caller must `video::predetach` on the GUI thread FIRST
/// (sink-widget rule — video_bridge.cpp predetach docs).
pub fn retire(id: &str) {
    let Some(h) = tiles().lock().unwrap().remove(id) else { return };
    h.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let label = id.to_string();
    std::thread::spawn(move || {
        let _ = h.pipeline.set_state(gst::State::Null);
        eprintln!("[memtile] {label} retired");
    });
}

/// Ids of all live memory tiles (the clear-all path predetaches each first).
pub fn ids() -> Vec<String> {
    tiles().lock().unwrap().keys().cloned().collect()
}
