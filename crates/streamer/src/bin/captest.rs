//! captest — screen-capture diagnostic harness (dev tool; not built into the GUI).
//!
//! Written to chase the "peers see a blank screen-share tile" bug, whose signature in the
//! app's own [lk-stats] log is decisive: the encoder reports ONE keyframe and then
//! `0x0@0` forever while peers PLI-storm for a picture. That means capture delivers ~one
//! frame and then dies — so this harness reproduces the app's CAPTURE pipeline in
//! isolation and counts what actually reaches the appsink that feeds libwebrtc.
//!
//! Two source modes:
//!   * portal (default) — the real xdg-desktop-portal ScreenCast (needs one click).
//!   * CAPTEST_NODE=<pipewire node id> — attach to ANY existing PipeWire video node, so
//!     the pipeline can be exercised headlessly (e.g. against a node published by
//!     `gst-launch videotestsrc ! pipewiresink`). This is what makes the GStreamer-side
//!     behaviour testable without a human at the screen.
//!
//! The `appexact` variant reproduces the app verbatim, INCLUDING the one-shot pad probe
//! that re-pins the output capsfilter's width/height on the first buffer — the prime
//! suspect: a mid-flight caps swap that can wedge negotiation and stop the buffer flow.
//! `pinned` is the proposed fix (size decided BEFORE the pipeline starts, no re-pin).
//!
//! Usage: captest [variant] [seconds]      variants: appexact | pinned | noprobe | norate | dup

// Portal/PipeWire only exist on Linux (ashpd is a linux-only target dep), so every
// item is cfg-gated and other targets get a stub main — keeps a workspace-wide
// `cargo build` green on Windows (HANDOFF_WINDOWS.md §0, same class as
// capture_windows.rs). Attributes only; the Linux code is untouched.
#[cfg(target_os = "linux")]
use anyhow::{bail, Context, Result};
#[cfg(target_os = "linux")]
use gstreamer as gst;
#[cfg(target_os = "linux")]
use gstreamer::prelude::*;
#[cfg(target_os = "linux")]
use gstreamer_video::prelude::VideoFrameExt;
#[cfg(target_os = "linux")]
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(target_os = "linux")]
use std::sync::Arc;

#[cfg(target_os = "linux")]
use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType};
#[cfg(target_os = "linux")]
use ashpd::desktop::PersistMode;

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("captest is a Linux-only capture harness (xdg-desktop-portal + pipewiresrc).");
    std::process::exit(2);
}

#[cfg(target_os = "linux")]
fn token_path() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_default())
        .join(".cache/tinhat-captest-token")
}

/// Round DOWN to an even, 16-aligned fit inside the box (mirrors lk_publish::fit_even).
#[cfg(target_os = "linux")]
fn fit_even(rw: u32, rh: u32, bw: u32, bh: u32) -> (u32, u32) {
    if rw == 0 || rh == 0 {
        return (bw & !15, bh & !15);
    }
    let s = f64::min(bw as f64 / rw as f64, bh as f64 / rh as f64).min(1.0);
    let w = ((rw as f64 * s) as u32).max(16) & !15;
    let h = ((rh as f64 * s) as u32).max(16) & !15;
    (w, h)
}

#[cfg(target_os = "linux")]
async fn portal_cast() -> Result<(Option<std::os::fd::OwnedFd>, u32, Option<(u32, u32)>, Option<ashpd::desktop::Session<Screencast>>)> {
    use std::os::fd::AsRawFd;
    let proxy = Screencast::new().await?;
    let session = proxy.create_session(Default::default()).await?;
    let saved = std::fs::read_to_string(token_path()).ok().filter(|s| !s.trim().is_empty());
    proxy
        .select_sources(
            &session,
            SelectSourcesOptions::default()
                .set_cursor_mode(CursorMode::Embedded)
                .set_sources(SourceType::Monitor | SourceType::Window)
                .set_multiple(false)
                .set_restore_token(saved.as_deref())
                .set_persist_mode(PersistMode::ExplicitlyRevoked),
        )
        .await?;
    let resp = proxy.start(&session, None, Default::default()).await?.response()?;
    if let Some(tok) = resp.restore_token() {
        let p = token_path();
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(&p, tok);
        eprintln!("[captest] saved restore token (future runs prompt-free)");
    }
    let Some(stream) = resp.streams().first() else { bail!("portal returned no streams (cancelled?)") };
    let node = stream.pipe_wire_node_id();
    let size = stream.size().map(|(w, h)| (w.max(0) as u32, h.max(0) as u32)).filter(|(w, h)| *w > 0 && *h > 0);
    let fd = proxy.open_pipe_wire_remote(&session, Default::default()).await?;
    eprintln!("[captest] portal: node={node} size={size:?} fd={}", fd.as_raw_fd());
    Ok((Some(fd), node, size, Some(session)))
}

#[cfg(target_os = "linux")]
fn main() -> Result<()> {
    gst::init()?;
    let variant = std::env::args().nth(1).unwrap_or_else(|| "appexact".into());
    let secs: u64 = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(12);
    let fps: u32 = std::env::var("CAPTEST_FPS").ok().and_then(|s| s.parse().ok()).unwrap_or(60);
    let (box_w, box_h) = (1920u32, 1080u32);

    // Source: an existing PipeWire node (headless testing) or the real portal.
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    let (fd, node, portal_size, session) = match std::env::var("CAPTEST_NODE").ok().and_then(|s| s.parse::<u32>().ok()) {
        Some(n) => {
            eprintln!("[captest] using EXISTING pipewire node {n} (no portal)");
            (None, n, None, None)
        }
        None => rt.block_on(portal_cast())?,
    };

    use std::os::fd::AsRawFd;
    let fd_part = fd.as_ref().map(|f| format!("fd={} ", f.as_raw_fd())).unwrap_or_default();

    // The app's tail, verbatim. `outcaps` is what the app re-pins mid-flight.
    let tail_probe = "videoconvert ! videoscale method=lanczos add-borders=false name=vscale ! \
                      capsfilter name=outcaps caps=video/x-raw,format=I420 ! \
                      queue max-size-buffers=4 leaky=downstream ! \
                      appsink name=sink emit-signals=true max-buffers=2 drop=true sync=false";

    // Fix candidate: size decided up-front, capsfilter fully fixed at build. No re-pin.
    let pinned_wh = portal_size.map(|(w, h)| fit_even(w, h, box_w, box_h)).unwrap_or((box_w, box_h));
    let tail_pinned = format!(
        "videoconvert ! videoscale method=lanczos add-borders=false ! \
         video/x-raw,format=I420,width={},height={},pixel-aspect-ratio=1/1 ! \
         queue max-size-buffers=4 leaky=downstream ! \
         appsink name=sink emit-signals=true max-buffers=2 drop=true sync=false",
        pinned_wh.0, pinned_wh.1
    );

    let head = format!("pipewiresrc {fd_part}path={node} do-timestamp=true keepalive-time=1000");
    let desc = match variant.as_str() {
        // App verbatim: videorate drop-only + (ANY) caps + the one-shot re-pin probe.
        "appexact" => format!("{head} ! videorate drop-only=true ! video/x-raw(ANY),framerate={fps}/1 ! {tail_probe}"),
        // Proposed fix: identical, but the output size is fixed BEFORE PLAYING (no probe).
        "pinned" => format!("{head} ! videorate drop-only=true ! video/x-raw(ANY),framerate={fps}/1 ! {tail_pinned}"),
        // Same as appexact but with NO probe installed (isolates the probe itself).
        "noprobe" => format!("{head} ! videorate drop-only=true ! video/x-raw(ANY),framerate={fps}/1 ! {tail_probe}"),
        "norate" => format!("{head} ! {tail_probe}"),
        // GPU downscale candidates: keep the expensive convert+scale of a 5K source OFF the
        // CPU. Only the final (small) frame is downloaded to system memory for libwebrtc.
        "cuda" => format!(
            "{head} ! videorate drop-only=true ! video/x-raw(ANY),framerate={fps}/1 ! \
             cudaupload ! cudaconvertscale ! \
             video/x-raw(memory:CUDAMemory),format=I420,width={},height={} ! cudadownload ! \
             queue max-size-buffers=4 leaky=downstream ! \
             appsink name=sink emit-signals=true max-buffers=2 drop=true sync=false",
            pinned_wh.0, pinned_wh.1
        ),
        "gl" => format!(
            "{head} ! videorate drop-only=true ! video/x-raw(ANY),framerate={fps}/1 ! \
             glupload ! glcolorscale ! \
             video/x-raw(memory:GLMemory),width={},height={} ! gldownload ! \
             videoconvert ! video/x-raw,format=I420 ! \
             queue max-size-buffers=4 leaky=downstream ! \
             appsink name=sink emit-signals=true max-buffers=2 drop=true sync=false",
            pinned_wh.0, pinned_wh.1
        ),
        "dup" => format!("{head} ! videorate ! video/x-raw(ANY),framerate={fps}/1 ! {tail_probe}"),
        o => bail!("unknown variant {o}"),
    };
    eprintln!("[captest] variant={variant} secs={secs}\n[captest] {desc}\n");

    let pipeline = gst::parse::launch(&desc)?.downcast::<gst::Pipeline>().map_err(|_| anyhow::anyhow!("not a pipeline"))?;

    // Install the app's one-shot caps re-pin probe for the variants that model it.
    if matches!(variant.as_str(), "appexact" | "norate" | "dup") {
        if let (Some(outcaps), Some(vscale)) = (pipeline.by_name("outcaps"), pipeline.by_name("vscale")) {
            if let Some(sinkpad) = vscale.static_pad("sink") {
                let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
                sinkpad.add_probe(gst::PadProbeType::BUFFER, move |pad, _info| {
                    if done.swap(true, Ordering::SeqCst) {
                        return gst::PadProbeReturn::Remove;
                    }
                    if let Some(caps) = pad.current_caps() {
                        if let Some(s) = caps.structure(0) {
                            if let (Ok(rw), Ok(rh)) = (s.get::<i32>("width"), s.get::<i32>("height")) {
                                let (ow, oh) = fit_even(rw.max(0) as u32, rh.max(0) as u32, box_w, box_h);
                                let nc = gst::Caps::builder("video/x-raw")
                                    .field("format", "I420")
                                    .field("width", ow as i32)
                                    .field("height", oh as i32)
                                    .field("pixel-aspect-ratio", gst::Fraction::new(1, 1))
                                    .build();
                                eprintln!("[captest] PROBE: re-pinning outcaps {rw}x{rh} -> {ow}x{oh} (MID-FLIGHT caps swap)");
                                outcaps.set_property("caps", &nc);
                            }
                        }
                    }
                    gst::PadProbeReturn::Remove
                });
            }
        }
    }

    let appsink = pipeline.by_name("sink").context("no sink")?.downcast::<gstreamer_app::AppSink>().map_err(|_| anyhow::anyhow!("not appsink"))?;
    let count = Arc::new(AtomicU64::new(0));
    let c2 = count.clone();
    let seen = Arc::new(std::sync::Mutex::new(String::new()));
    let s2 = seen.clone();
    appsink.set_callbacks(
        gstreamer_app::AppSinkCallbacks::builder()
            .new_sample(move |s| {
                if let Ok(sample) = s.pull_sample() {
                    let n = c2.fetch_add(1, Ordering::Relaxed);
                    if n == 30 && std::env::var_os("CAPTEST_DUMP").is_some() {
                        if let (Some(buf), Some(caps)) = (sample.buffer(), sample.caps()) {
                            if let Ok(info) = gstreamer_video::VideoInfo::from_caps(caps) {
                                if let Ok(fr) = gstreamer_video::VideoFrameRef::from_buffer_ref_readable(buf, &info) {
                                    let (w, h) = (info.width() as usize, info.height() as usize);
                                    let sy = fr.plane_stride()[0] as usize;
                                    let su = fr.plane_stride()[1] as usize;
                                    let sv = fr.plane_stride()[2] as usize;
                                    let (py, pu, pv) = (fr.plane_data(0).unwrap_or(&[]), fr.plane_data(1).unwrap_or(&[]), fr.plane_data(2).unwrap_or(&[]));
                                    let mut ppm = format!("P6\n{w} {h}\n255\n").into_bytes();
                                    for y in 0..h {
                                        for x in 0..w {
                                            let yy = py[y * sy + x] as f32;
                                            let uu = pu[(y / 2) * su + x / 2] as f32 - 128.0;
                                            let vv = pv[(y / 2) * sv + x / 2] as f32 - 128.0;
                                            let r = (yy + 1.402 * vv).clamp(0.0, 255.0) as u8;
                                            let g = (yy - 0.344 * uu - 0.714 * vv).clamp(0.0, 255.0) as u8;
                                            let b = (yy + 1.772 * uu).clamp(0.0, 255.0) as u8;
                                            ppm.extend_from_slice(&[r, g, b]);
                                        }
                                    }
                                    let _ = std::fs::write("/tmp/captest_frame.ppm", ppm);
                                    eprintln!("[captest] dumped frame {w}x{h} -> /tmp/captest_frame.ppm");
                                }
                            }
                        }
                    }
                    if let Some(c) = sample.caps() {
                        let cs = c.to_string();
                        let mut g = s2.lock().unwrap();
                        if *g != cs {
                            eprintln!("[captest] SINK CAPS: {cs}");
                            *g = cs;
                        }
                    }
                }
                Ok(gst::FlowSuccess::Ok)
            })
            .build(),
    );

    let bus = pipeline.bus().unwrap();
    std::thread::spawn(move || {
        for msg in bus.iter_timed(gst::ClockTime::NONE) {
            match msg.view() {
                gst::MessageView::Error(e) => eprintln!("[captest] *** GST ERROR: {} | {:?}", e.error(), e.debug()),
                gst::MessageView::Warning(w) => eprintln!("[captest] gst WARN: {} | {:?}", w.error(), w.debug()),
                gst::MessageView::Eos(_) => eprintln!("[captest] *** EOS"),
                _ => {}
            }
        }
    });

    pipeline.set_state(gst::State::Playing)?;
    eprintln!("[captest] PLAYING — counting frames into the appsink (this is what feeds libwebrtc)\n");

    let mut last = 0u64;
    let mut zero_secs = 0;
    for t in 1..=secs {
        std::thread::sleep(std::time::Duration::from_secs(1));
        let n = count.load(Ordering::Relaxed);
        let d = n - last;
        last = n;
        if d == 0 {
            zero_secs += 1;
        }
        eprintln!("[captest] t={t:>2}s  frames_this_sec={d:<5} total={n}");
    }

    let total = count.load(Ordering::Relaxed);
    eprintln!("\n[captest] ===== RESULT variant={variant} =====");
    eprintln!("[captest] total={total} over {secs}s  ({:.1} fps avg), seconds_with_zero_frames={zero_secs}/{secs}", total as f64 / secs as f64);
    if total <= 2 {
        eprintln!("[captest] VERDICT: *** CAPTURE STALLS *** — reproduces the blank-tile bug.");
    } else if zero_secs > 0 {
        eprintln!("[captest] VERDICT: INTERMITTENT — gaps mean frozen video for peers.");
    } else {
        eprintln!("[captest] VERDICT: continuous feed. GOOD.");
    }

    pipeline.set_state(gst::State::Null)?;
    if let Some(s) = session {
        rt.block_on(async { let _ = s.close().await; });
    }
    Ok(())
}
