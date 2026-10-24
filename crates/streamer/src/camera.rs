//! Local camera (webcam) capture + enumeration for the unified call session.
//!
//! Mirrors the screen-share publish path (`lk_publish.rs`): a GStreamer `v4l2src`
//! captures the camera, converts to I420 at a publish-safe size, and (optionally) a
//! `tee` splits it into (1) an `appsink` whose frames feed a libwebrtc
//! `NativeVideoSource` (the published, E2EE-encrypted track) and (2) a `qml6glsink`
//! self-view that renders into the QML grid on Qt's GL context.
//!
//! NOTE: gst Video/Source devices expose `device.path` (e.g. /dev/video0); they do
//! NOT carry the PipeWire `node.name` the audio enumerator keys off — so cameras get
//! their own enumerator here, keyed by `device.path`.

use anyhow::Result;
use gstreamer as gst;
use gstreamer::prelude::*;

// Default camera publish resolution. 1280x720 is universally supported by UVC cams and
// well under H.264's 4096 limit; videoscale add-borders letterboxes other aspect ratios.
// The user can opt into 1080p via the persisted CamQuality (see `cam_quality`).
pub const CAM_W: u32 = 1280;
pub const CAM_H: u32 = 720;
pub const CAM_FPS: u32 = 30;

/// User-selectable camera publish quality (mirrors the ShareQuality pattern): the
/// requested output size/fps/bitrate plus whether to publish adaptive simulcast layers.
/// `simulcast` only engages on the hardware (NVENC) encode path — see `publish_camera`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CamQuality {
    pub w: u32,
    pub h: u32,
    pub fps: u32,
    pub bitrate_kbps: u32,
    pub simulcast: bool,
}
impl Default for CamQuality {
    fn default() -> Self {
        Self { w: CAM_W, h: CAM_H, fps: CAM_FPS, bitrate_kbps: 2_500, simulcast: true }
    }
}

fn cam_quality_path() -> std::path::PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| {
            std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
        });
    base.join("tinhat/cam-quality")
}
/// Clamp + snap camera dims so every simulcast rung stays encoder-legal: rungs are w/2
/// and w/4, and H.264 encoders need EVEN dimensions at every rung, so w/h must be
/// multiples of 8 (e.g. 1366 -> half rung 683px, odd -> the SimulcastEncoderAdapter
/// silently drops to OpenH264 and every receiver gets blank rungs).
fn sanitize_cam_dims(w: u32, h: u32) -> (u32, u32) {
    let snap8 = |v: u32, lo: u32, hi: u32| (v.clamp(lo, hi) / 8) * 8;
    (snap8(w, 320, 1920), snap8(h, 184, 1080))
}

fn load_cam_quality() -> CamQuality {
    let d = CamQuality::default();
    let Ok(s) = std::fs::read_to_string(cam_quality_path()) else { return d };
    // Format: "W H FPS KBPS SIM01" (all tokens after the first four optional).
    let toks: Vec<&str> = s.split_whitespace().collect();
    let n: Vec<u32> = toks.iter().take(4).filter_map(|x| x.parse().ok()).collect();
    if n.len() == 4 && n[0] > 0 && n[1] > 0 && n[2] > 0 {
        let simulcast = toks.get(4).map(|t| *t != "0").unwrap_or(d.simulcast);
        // Same sanitation as set_cam_quality: a corrupted/hand-edited file (e.g. kbps 0,
        // or a width whose /2 or /4 rung is odd) otherwise reaches VideoEncoding/
        // simulcast-layer math unvalidated — the exact condition that silently drops the
        // whole SimulcastEncoderAdapter to OpenH264 (blank rungs for every receiver).
        let (w, h) = sanitize_cam_dims(n[0], n[1]);
        CamQuality { w, h, fps: n[2].clamp(1, 60), bitrate_kbps: n[3].clamp(500, 8_000), simulcast }
    } else {
        d
    }
}
fn save_cam_quality(q: &CamQuality) {
    let p = cam_quality_path();
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(
        &p,
        format!("{} {} {} {} {}", q.w, q.h, q.fps, q.bitrate_kbps, if q.simulcast { 1 } else { 0 }),
    );
}
fn cam_quality_cell() -> &'static std::sync::Mutex<CamQuality> {
    static Q: std::sync::OnceLock<std::sync::Mutex<CamQuality>> = std::sync::OnceLock::new();
    Q.get_or_init(|| std::sync::Mutex::new(load_cam_quality()))
}
/// The current (persisted) camera quality request.
pub fn cam_quality() -> CamQuality {
    *cam_quality_cell().lock().unwrap()
}
/// Set + persist the camera resolution/fps/bitrate (the simulcast flag is preserved —
/// change it separately via [`set_cam_simulcast`]). Takes effect on the next publish;
/// the app re-publishes a live camera to apply mid-call.
pub fn set_cam_quality(w: u32, h: u32, fps: u32, bitrate_kbps: u32) {
    let mut q = cam_quality();
    (q.w, q.h) = sanitize_cam_dims(w, h);
    q.fps = fps.clamp(1, 60);
    q.bitrate_kbps = bitrate_kbps.clamp(500, 8_000);
    *cam_quality_cell().lock().unwrap() = q;
    save_cam_quality(&q);
}
/// Set + persist just the camera simulcast flag. Takes effect on the next publish.
pub fn set_cam_simulcast(on: bool) {
    let mut q = cam_quality();
    q.simulcast = on;
    *cam_quality_cell().lock().unwrap() = q;
    save_cam_quality(&q);
}
/// Compact "W H FPS KBPS SIM01" label for the QML mirror.
pub fn cam_quality_label() -> String {
    let q = cam_quality();
    format!("{} {} {} {} {}", q.w, q.h, q.fps, q.bitrate_kbps, if q.simulcast { 1 } else { 0 })
}

/// Resolve the EFFECTIVE publish size/fps for `device` under the current CamQuality.
/// A >720p request is honoured ONLY when the device offers a 16:9 capture mode STRICTLY
/// LARGER than it — the capture-strictly-larger active-downscale invariant is what keeps
/// qml6glsink's glupload off the r109/r111 SIGSEGV path (see `probe_camera_caps`), so a
/// camera without such a mode falls back to the proven 1280x720 publish instead of
/// gambling on a never-validated same-size negotiation.
pub fn effective_output(device: Option<&str>) -> (u32, u32, u32) {
    let q = cam_quality();
    if q.w <= CAM_W {
        return (q.w, q.h, q.fps);
    }
    let (_, size) = probe_camera_caps(device, q.w, q.fps);
    if size.is_some() {
        (q.w, q.h, q.fps)
    } else {
        eprintln!(
            "[camera] no 16:9 capture mode strictly larger than {}x{} — keeping the proven {CAM_W}x{CAM_H} publish",
            q.w, q.h
        );
        (CAM_W, CAM_H, q.fps)
    }
}

/// Enumerate Video/Source (camera) devices as `(device_path, label)`, e.g.
/// `("/dev/video0", "Elgato 4K X: Elgato 4K X")`. Keyed by `device.path` (the v4l2
/// node), which is what `v4l2src device=` wants — NOT `node.name` (PipeWire-only,
/// absent on v4l2 Video/Source devices).
pub fn list_cameras() -> Vec<(String, String)> {
    // 10.1.3: the shared, never-stopped monitor (see call::device_monitor) — a start/stop
    // cycle per enumeration is what crashed the Flatpak's PipeWire device provider.
    let mut out = Vec::new();
    if let Some(monitor) = crate::call::device_monitor() {
        for d in monitor.devices().iter().filter(|d| d.has_classes("Video/Source")) {
            let label = d.display_name().to_string();
            if let Some(props) = d.properties() {
                // Want a /dev/videoN path for `v4l2src device=`. The pure-v4l2 provider
                // exposes `device.path`; the PipeWire provider (default here) wraps the
                // same camera and exposes `api.v4l2.path` (= /dev/video0) instead, with
                // only a pipewire `node.name` otherwise. Prefer the real v4l2 node.
                //
                // Windows Media Foundation VIRTUAL cameras (NVIDIA Broadcast, OBS 28+
                // frame-server) have NO device.path — no kernel symbolic link exists —
                // only device.name. They used to be silently DROPPED here (a field
                // report, "NVIDIA Broadcast not functioning": it never made the dropdown).
                // `name:<device.name>` selects by mfvideosrc device-name instead; the
                // Linux arms are untouched (every Linux provider has one of the first
                // three keys, so the new arm can never fire there).
                let path = props
                    .get::<String>("device.path")
                    .ok()
                    // A DirectShow software-filter moniker (`@device:sw:{CLSID}\{…}`)
                    // is a "path" mfvideosrc can OPEN but never stream from: the MF
                    // source reader binds the DirectShow-only filter and the basesrc
                    // loop dies with error -5 on the first pull (NVIDIA Broadcast,
                    // field logs — black camera, retry died too). Treat it
                    // as pathless so the device-name arm selects the MF virtual camera
                    // the way MF actually supports.
                    .filter(|p| !p.starts_with("@device:"))
                    .or_else(|| props.get::<String>("api.v4l2.path").ok())
                    .or_else(|| props.get::<String>("node.name").ok())
                    .or_else(|| props.get::<String>("device.name").ok().map(|n| format!("name:{n}")));
                match path {
                    Some(p) => {
                        eprintln!(
                            "[camera] enumerated: \"{label}\" via {}",
                            if p.starts_with("name:") { "device-name (no path — virtual cam?)" } else { "device path" }
                        );
                        out.push((p, label));
                    }
                    None => {
                        eprintln!("[camera] enumerated but UNSELECTABLE (no path/name property): \"{label}\"");
                    }
                }
            }
        }
    }
    out
}

/// Headless diagnostic (no LiveKit): capture from the camera for `secs` and log the
/// first frame's caps-derived stride vs the buffer's ACTUAL (mapped) stride. If they
/// differ, the buffer is row-padded — confirming why the unpadded caps stride sheared
/// the published video (green tearing). Run via `TINHAT_CAMPROBE=1`.
pub fn probe_camera(device: Option<&str>, secs: u64) {
    use gstreamer::prelude::*;
    let _ = crate::ensure_init();
    let pipeline = match build_camera_capture_pipeline(device) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[camprobe] pipeline build failed: {e}");
            return;
        }
    };
    let Some(sink) = pipeline.by_name("sink") else {
        eprintln!("[camprobe] no appsink");
        return;
    };
    let appsink = match sink.downcast::<gstreamer_app::AppSink>() {
        Ok(a) => a,
        Err(_) => {
            eprintln!("[camprobe] sink not an AppSink");
            return;
        }
    };
    let logged = std::sync::atomic::AtomicBool::new(false);
    appsink.set_callbacks(
        gstreamer_app::AppSinkCallbacks::builder()
            .new_sample(move |s| {
                let sample = s.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                if !logged.swap(true, std::sync::atomic::Ordering::Relaxed) {
                    if let (Some(buffer), Some(caps)) = (sample.buffer(), sample.caps()) {
                        if let Ok(info) = gstreamer_video::VideoInfo::from_caps(caps) {
                            if let Ok(frame) =
                                gstreamer_video::VideoFrameRef::from_buffer_ref_readable(buffer, &info)
                            {
                                use gstreamer_video::prelude::VideoFrameExt;
                                eprintln!(
                                    "[camprobe] {}x{}  caps_stride={:?}  frame_stride={:?}  (differ => was the bug)",
                                    info.width(),
                                    info.height(),
                                    info.stride(),
                                    frame.plane_stride(),
                                );
                            }
                        }
                    }
                }
                Ok(gst::FlowSuccess::Ok)
            })
            .build(),
    );
    if let Err(e) = pipeline.set_state(gst::State::Playing) {
        eprintln!("[camprobe] play failed: {e}");
        return;
    }
    // Poll the bus for errors/warnings (else a v4l2 negotiation failure is silent).
    if let Some(bus) = pipeline.bus() {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
        while std::time::Instant::now() < deadline {
            if let Some(msg) = bus.timed_pop(gst::ClockTime::from_mseconds(400)) {
                use gst::MessageView;
                match msg.view() {
                    MessageView::Error(e) => {
                        eprintln!("[camprobe] gst ERROR from {:?}: {} ({:?})",
                            e.src().map(|s| s.name()), e.error(), e.debug());
                    }
                    MessageView::Warning(w) => eprintln!("[camprobe] gst WARN: {}", w.error()),
                    MessageView::StateChanged(_) => {}
                    _ => {}
                }
            }
        }
    } else {
        std::thread::sleep(std::time::Duration::from_secs(secs));
    }
    let _ = pipeline.set_state(gst::State::Null);
    eprintln!("[camprobe] done");
}

// Capture-mode width cap for a given publish width: at the default 720p publish don't
// pull a 4K mode (we downscale anyway); a 1080p publish must allow >1920 capture so a
// strictly-larger mode (2560x1440, else 3840x2160) exists to actively downscale from.
fn cap_max_w(out_w: u32) -> i32 {
    if out_w > CAM_W { 3840 } else { 1920 }
}

/// SINGLE READY-state probe of the device's REAL caps -> `(offers_raw, best_downscale_size)`.
///
/// `offers_raw`: any `video/x-raw` mode (raw cameras capture with a plain `videoconvert`; MJPEG-
/// only cameras need `jpegdec`; never `decodebin` — with a downstream format caps it fails to
/// autoplug RAW cameras, e.g. the Logitech MX Brio). Defaults to raw on any error (universal path).
///
/// `best_downscale_size`: the SMALLEST 16:9 capture mode STRICTLY LARGER than the `out_w` (x9/16)
/// output (capped at `cap_max_w(out_w)`) so downstream `videoscale` ALWAYS actively downscales. WHY: pinning
/// the EXACT output size (1280x720) makes videoscale a passthrough, and the passed-through buffer
/// SIGSEGV's `gst_gl_upload` into Qt's shared GLES2 context on this NVIDIA stack (r109 AND r111
/// both crashed camera-on that way — confirmed by coredump). Active scaling allocates fresh,
/// glupload-safe buffers (the r110-proven-safe pattern), and a true-16:9 source (e.g. 1920x1080)
/// also fixes the Elgato 4K X's 720x480 anamorphic squish. `None` (nothing larger-and-16:9 — e.g.
/// a 720p-max webcam) => emit no capsfilter, free-negotiate (the prior stable behaviour).
/// NULL state reports only the generic template, so we probe at READY.
///
/// FPS-AWARE (two-pass): pass 1 keeps every constraint above AND requires the mode to reach
/// `out_fps`, preferring raw then MJPEG — so a USB2 C920-class cam (raw 1080p@~5fps, MJPEG
/// 1080p30) takes the jpegdec branch instead of publishing a duplicated-frame slideshow. If
/// NO mode of either format qualifies on fps, pass 2 returns exactly the old size-only
/// selection, so no previously-working camera can regress.
fn probe_camera_caps(device: Option<&str>, out_w: u32, out_fps: u32) -> (bool, Option<(u32, u32)>) {
    // Windows cameras come via Media Foundation (MEDIA_PIPELINE.md §2.2); the READY-state
    // caps probe below is source-agnostic and carries over unchanged.
    let (factory, dev_prop) = if cfg!(windows) { ("mfvideosrc", "device-path") } else { ("v4l2src", "device") };
    let mut b = gst::ElementFactory::make(factory);
    if let Some(dev) = device {
        // `name:` marks a pathless MF virtual camera (list_cameras) — select by name.
        if let Some(n) = dev.strip_prefix("name:").filter(|_| cfg!(windows)) {
            b = b.property("device-name", n);
        } else {
            b = b.property(dev_prop, dev);
        }
    }
    let Ok(src) = b.build() else { return (true, None) };
    if src.set_state(gst::State::Ready).is_err() {
        let _ = src.set_state(gst::State::Null);
        return (true, None);
    }
    let caps = src.static_pad("src").map(|p| p.query_caps(None));
    let _ = src.set_state(gst::State::Null);
    let Some(caps) = caps else { return (true, None) };
    if caps.is_any() || caps.is_empty() {
        return (true, None);
    }
    let offers_raw = (0..caps.size()).any(|i| {
        caps.structure(i)
            .map(|s| s.name().starts_with("video/x-raw"))
            .unwrap_or(false)
    });
    let is_16_9 = |w: i32, h: i32| (w * 9 - h * 16).abs() <= 1;
    // Does the structure's framerate reach CAM_FPS? v4l2src at READY emits it as a fixed
    // Fraction, a List of Fractions, or a FractionRange — take the max of whichever shape.
    // An ABSENT or unparseable field PASSES (never downgrade a device on a parse quirk).
    let fps_ok = |s: &gst::StructureRef| -> bool {
        let Ok(v) = s.value("framerate") else { return true };
        let fps = |f: gst::Fraction| f.numer() as f64 / f.denom().max(1) as f64;
        let max = if let Ok(f) = v.get::<gst::Fraction>() {
            fps(f)
        } else if let Ok(l) = v.get::<gst::List>() {
            let m = l
                .iter()
                .filter_map(|x| x.get::<gst::Fraction>().ok())
                .map(fps)
                .fold(None, |a: Option<f64>, b| Some(a.map_or(b, |a| a.max(b))));
            match m {
                Some(m) => m,
                None => return true,
            }
        } else if let Ok(r) = v.get::<gst::FractionRange>() {
            fps(r.max())
        } else {
            return true;
        };
        max >= out_fps as f64
    };
    // Smallest 16:9 fixed size strictly larger than the output, capped at cap_max_w;
    // `check_fps` additionally requires the mode to reach out_fps.
    let cap_w = cap_max_w(out_w);
    let select = |want: &str, check_fps: bool| -> Option<(u32, u32)> {
        let mut best: Option<(i32, i32)> = None;
        for i in 0..caps.size() {
            let Some(s) = caps.structure(i) else { continue };
            if !s.name().starts_with(want) {
                continue;
            }
            let (Some(w), Some(h)) = (
                s.get_optional::<i32>("width").ok().flatten(),
                s.get_optional::<i32>("height").ok().flatten(),
            ) else {
                continue;
            };
            if w > out_w as i32 && w <= cap_w && is_16_9(w, h) && (!check_fps || fps_ok(s)) {
                best = match best {
                    Some((bw, bh)) if bw <= w => Some((bw, bh)),
                    _ => Some((w, h)),
                };
            }
        }
        best.map(|(w, h)| (w as u32, h as u32))
    };
    // Pass 1 — size AND fps must qualify, raw preferred: a USB2 C920-class cam offers raw
    // 1080p only at ~5fps (bandwidth limit) but MJPEG at 1080p30; pinning raw there made
    // videorate duplicate a ~5fps slideshow up to the 30fps output caps.
    if let Some(sz) = select("video/x-raw", true) {
        return (true, Some(sz));
    }
    if let Some(sz) = select("image/jpeg", true) {
        return (false, Some(sz));
    }
    // Pass 2 — NOTHING qualifies on fps: fall back to EXACTLY the old size-only selection,
    // so no currently-working camera regresses (a raw-only 720p-max cam keeps the
    // free-negotiate videoconvert path; a raw 1080p@25 cam keeps its pinned active
    // downscale rather than dropping to free negotiation).
    let want = if offers_raw { "video/x-raw" } else { "image/jpeg" };
    (offers_raw, select(want, false))
}

/// The capture source chain up to (and including) `videoconvert`: format-aware (raw -> `videoconvert`,
/// MJPEG-only -> `jpegdec`; never `decodebin`) AND, when the device offers a larger 16:9 mode,
/// size-pinned to force an ACTIVE downscale (see `probe_camera`). Test/diagnostic override:
/// `TINHAT_CAM_FORCE=WxH` pins an exact capture size (used by the TINHAT_CAM_GLTEST harness to
/// reproduce the passthrough crash vs validate the downscale fix); not used in normal runs.
fn camera_src_chain(device: Option<&str>, out_w: u32, out_fps: u32) -> String {
    let src = if cfg!(windows) {
        match device {
            // Windows MF device paths are symbolic links like
            // `\\?\usb#vid_0fd9&pid_009b...#{guid}\global`. gst parse-launch
            // treats `\` inside a quoted string as an escape and EATS it, so the
            // interpolated path never matches the device (probe_camera_caps is
            // immune — it sets the property directly). Double every backslash so
            // the parser reconstructs the original path.
            // `name:` = pathless MF virtual camera (NVIDIA Broadcast / OBS frame-server;
            // see list_cameras) — mfvideosrc selects those by device-name.
            Some(dev) if dev.starts_with("name:") => {
                let esc = dev["name:".len()..].replace('\\', "\\\\").replace('"', "\\\"");
                format!("mfvideosrc device-name=\"{esc}\"")
            }
            Some(dev) => {
                let esc = dev.replace('\\', "\\\\");
                format!("mfvideosrc device-path=\"{esc}\"")
            }
            None => "mfvideosrc".to_string(),
        }
    } else {
        match device {
            Some(dev) => format!("v4l2src device=\"{dev}\""),
            None => "v4l2src".to_string(),
        }
    };
    let (raw, mut size) = probe_camera_caps(device, out_w, out_fps);
    if let Some(f) = std::env::var("TINHAT_CAM_FORCE").ok().and_then(|s| {
        let (w, h) = s.split_once('x')?;
        Some((w.trim().parse::<u32>().ok()?, h.trim().parse::<u32>().ok()?))
    }) {
        size = Some(f);
    }
    if raw {
        let cap = size
            .map(|(w, h)| format!(" ! video/x-raw,width={w},height={h}"))
            .unwrap_or_default();
        format!("{src}{cap} ! videoconvert")
    } else {
        let cap = size
            .map(|(w, h)| format!(",width={w},height={h}"))
            .unwrap_or_default();
        format!("{src} ! image/jpeg{cap} ! jpegdec ! videoconvert")
    }
}

/// Build the camera CAPTURE-ONLY pipeline (no self-view): selected `device` (or the
/// first camera if `None`) -> convert/scale -> I420 CAM_WxCAM_H -> appsink "sink".
/// Used by the headless self-test (no GUI) and as the no-selfview publish path.
pub fn build_camera_capture_pipeline(device: Option<&str>) -> Result<gst::Pipeline> {
    // Format-aware source (raw -> videoconvert, MJPEG-only -> jpegdec); NO decodebin (it broke
    // raw cameras). videoscale normalizes to I420 at the publish size; add-borders letterboxes.
    let (ow, oh, ofps) = effective_output(device);
    let src = camera_src_chain(device, ow, ofps);
    let desc = format!(
        "{src} ! videoscale add-borders=true ! videorate ! \
         video/x-raw,format=I420,width={ow},height={oh},framerate={ofps}/1,pixel-aspect-ratio=1/1 ! \
         queue max-size-buffers=4 leaky=downstream ! \
         appsink name=sink emit-signals=false max-buffers=2 drop=true sync=false"
    );
    gst::parse::launch(&desc)?
        .downcast::<gst::Pipeline>()
        .map_err(|_| anyhow::anyhow!("camera: parsed element is not a Pipeline"))
}

/// Build the camera capture pipeline WITH a self-view branch. A `tee` splits the
/// converted I420 stream into:
///   * branch "pub":  queue -> appsink name=pubsink  (feeds NativeVideoSource → publish)
///   * branch "view": queue -> glupload -> glcolorconvert -> qml6glsink name=sink
/// The qml6glsink is named "sink" (NOT "pubsink") so the EXISTING C++ attach
/// (`gst_bin_get_by_name(pipe,"sink")` + g_object_set "widget") binds it unchanged;
/// the appsink is "pubsink". The pipeline is returned in NULL — the app (GUI thread)
/// sets it PLAYING via the C++ attach so the qml6glsink shares Qt's GL context.
pub fn build_camera_selfview_pipeline(device: Option<&str>) -> Result<gst::Pipeline> {
    let (ow, oh, ofps) = effective_output(device);
    let src = camera_src_chain(device, ow, ofps); // raw -> videoconvert / MJPEG -> jpegdec; NO decodebin
    // Windows: the view branch ends in a plain appsink (same name "sink") — the C++
    // attach copies I420 into a QML VideoOutput's QVideoSink instead of binding a GL
    // widget (MEDIA_PIPELINE.md §6.3). Linux tail unchanged.
    let view_tail = if cfg!(windows) {
        "appsink name=sink emit-signals=false max-buffers=2 drop=true sync=false"
    } else {
        "glupload ! glcolorconvert ! qml6glsink name=sink sync=false"
    };
    let desc = format!(
        "{src} ! videoscale add-borders=true ! videorate ! \
         video/x-raw,format=I420,width={ow},height={oh},framerate={ofps}/1,pixel-aspect-ratio=1/1 ! \
         tee name=t \
         t. ! queue max-size-buffers=4 leaky=downstream ! \
              appsink name=pubsink emit-signals=false max-buffers=2 drop=true sync=false \
         t. ! queue max-size-buffers=4 leaky=downstream ! {view_tail}"
    );
    gst::parse::launch(&desc)?
        .downcast::<gst::Pipeline>()
        .map_err(|_| anyhow::anyhow!("camera: parsed element is not a Pipeline"))
}

// Holds the TINHAT_CAM_GLTEST selfview pipeline alive for the test process (instead of leaking),
// so the harness can stop it cleanly and a PASS exits 0 — distinguishing an in-run glupload crash
// from a process-teardown artifact.
static GLTEST_PIPELINE: std::sync::Mutex<Option<gst::Pipeline>> = std::sync::Mutex::new(None);

/// TEST-ONLY (the `TINHAT_CAM_GLTEST` self-test harness): build the selfview pipeline, hold it alive,
/// and return its `GstElement*` as a `usize` for `video::attach`. Lets the app crate drive the REAL
/// selfview glupload -> qml6glsink path against Qt's GL context — the ONLY way to reproduce the
/// camera-on `gst_gl_upload` crash — without the app crate needing a gstreamer dependency.
pub fn cam_gltest_selfview_ptr(device: Option<&str>) -> Result<usize> {
    let p = build_camera_selfview_pipeline(device)?;
    let ptr = p.as_ptr() as usize;
    *GLTEST_PIPELINE.lock().unwrap() = Some(p);
    Ok(ptr)
}

/// TEST-ONLY: stop + drop the held selfview pipeline (set to Null). Call AFTER `video::predetach`
/// (which unbinds the qml6glsink widget on the GUI thread) so teardown is clean.
pub fn cam_gltest_stop() {
    if let Some(p) = GLTEST_PIPELINE.lock().unwrap().take() {
        let _ = p.set_state(gst::State::Null);
    }
}
