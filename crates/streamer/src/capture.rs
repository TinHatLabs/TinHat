//! KDE Wayland screen capture via the xdg-desktop-portal ScreenCast portal.
//!
//! `pick_screencast()` pops the KDE picker (on-screen — needs the user present;
//! cannot run headless) and returns a live [`ScreenCast`] handle wrapping the
//! PipeWire fd + node id. Feed those into `pipewiresrc fd=<fd> path=<node_id>`.
//!
//! Hold the [`ScreenCast`] for the entire stream: dropping the fd tears down the
//! PipeWire remote and the pipeline EOSes. Call [`ScreenCast::close`] when done to
//! also close the portal D-Bus session (clears KDE's "you are sharing" indicator
//! and frees the server-side session); plain `Drop` only closes the fd.

use anyhow::{bail, Result};
use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType};
use ashpd::desktop::{PersistMode, Session};
use std::os::fd::{AsRawFd, OwnedFd};

/// A live screen-cast: keep it alive for the lifetime of the GStreamer pipeline.
pub struct ScreenCast {
    fd: OwnedFd,
    node_id: u32,
    // The captured source's pixel size (from the portal), used to publish at the
    // source's REAL aspect ratio instead of squishing it into a fixed 16:9 frame.
    size: Option<(u32, u32)>,
    // The REAL source size as the capture pipeline saw it on its first frame, when the portal (and the stream
    // probe) gave none. A re-publish of the same cast (a quality change) then knows the size up front.
    learned: std::sync::Arc<std::sync::Mutex<Option<(u32, u32)>>>,
    // `Session<T>` owns a `Proxy<'static>`, so it's fully storable and keeps the
    // portal session (and thus the cast) open until explicitly closed.
    session: Session<Screencast>,
}

impl ScreenCast {
    /// The GStreamer source-head fragment for this capture — the platform seam
    /// shared with the Windows module (`d3d11screencapturesrc` there): [`source_head`].
    pub(crate) fn src_fragment(&self) -> String {
        source_head(Some(self.raw_fd()), self.node_id(), share_min_buffers())
    }

    /// 12.1.8: the source head with the share's frame rate as the stream's MAXIMUM, negotiated with the compositor
    /// (`max-framerate` in the PipeWire format; pipewiresrc 1.4 carries it from these caps). KWin offers
    /// max-framerate as a range up to the monitor's refresh rate and paces its screencast at what was negotiated, so
    /// without it a 60 fps share of a 120 fps game made KWin render and copy 120 frames a second (28 MB each at
    /// 3233x2151) on the GPU the game is using, and `videorate` threw half away after the copy (SHARE-GAME-LAG).
    /// Off with TINHAT_SHARE_NO_RATE_CAP set. A producer that can't do a maximum of `fps` negotiates uncapped (see
    /// [`source_head_with_cap`]).
    pub(crate) fn src_fragment_paced(&self, fps: u32) -> String {
        source_head_paced(&self.src_fragment(), fps)
    }

    /// The zero-copy (DMA-BUF only) head for this cast, see [`source_head_dmabuf`].
    pub(crate) fn src_fragment_dmabuf(&self, fps: u32) -> String {
        source_head_dmabuf(&self.src_fragment(), fps, source_rate_cap_on())
    }

    /// Raw fd for `pipewiresrc fd=`. The fd stays owned by `self`.
    pub fn raw_fd(&self) -> i32 {
        self.fd.as_raw_fd()
    }
    /// PipeWire node id for `pipewiresrc path=`.
    pub fn node_id(&self) -> u32 {
        self.node_id
    }
    /// Captured source size in pixels: the portal's, else the one the capture learned from its first frame.
    pub fn size(&self) -> Option<(u32, u32)> {
        self.size.or_else(|| *self.learned.lock().unwrap())
    }
    /// Where the capture builders record the real source size (see [`Self::size`]).
    pub(crate) fn size_memo(&self) -> std::sync::Arc<std::sync::Mutex<Option<(u32, u32)>>> {
        self.learned.clone()
    }
    /// Close the portal session (async), then drop the fd. Call AFTER the pipeline
    /// is stopped so pipewiresrc isn't reading a closing remote.
    pub async fn close(self) {
        let _ = self.session.close().await;
        // self.fd drops here.
    }
}

/// The share's PipeWire source element: `pipewiresrc [fd=N] path=NODE do-timestamp=true [min-buffers=N]`.
///
/// A PipeWire consumer hands a buffer back through the stream one delivered frame late, and a compositor that
/// records on a fixed schedule skips the frame when it has no free buffer: KWin 6.8 does (a fixed grid at the
/// negotiated rate, 3 buffers), and so does the wlroots portal (2 buffers). So the capture must let go of every
/// buffer as soon as its frame is converted:
///   - NO `keepalive-time` and NO `resend-last` (12.1.12): with either set, pipewiresrc keeps a reference to the
///     NEWEST frame until the next one arrives. A 60 fps share then locked at 48, 45 or 30 fps against KWin 6.8 for
///     its whole life, on every capture path and at any size, and ran at about 1 fps through the wlroots portal.
///     A still screen needs no resend from here: the feeder re-pushes the last frame itself (lk_publish.rs, the
///     keepalive repeater). The one thing the source's keepalive still did: a still screen whose single start
///     frame arrived before the lease's source gate opened got it again a second later. Such a start now shows
///     its first picture when the screen first changes (a mouse move is one); see call.rs wait_first_frame.
///   - `min-buffers` only where [`share_min_buffers`] says so (KWin), 0 = no buffer count at all.
/// The other holder was the CPU capture's mid-stream re-pin (lk_publish.rs cpu_presized_desc).
pub(crate) fn source_head(fd: Option<i32>, node: u32, min_buffers: u32) -> String {
    let fd = fd.map(|fd| format!("fd={fd} ")).unwrap_or_default();
    let buffers = if min_buffers > 0 { format!(" min-buffers={min_buffers}") } else { String::new() };
    format!("pipewiresrc {fd}path={node} do-timestamp=true{buffers}")
}

/// How many stream buffers a share asks KWin for. KWin's own 3 are exactly enough when nothing is late: one frame
/// with the capture, one on its way back, one to record into. A capture thread that is held up for a frame interval
/// (a game loading every core) then costs a frame each time; measured on KWin 6.8.2 under CPU load, 3 buffers ran a
/// 60 fps share at 43 to 55 fps in most starts and 4 kept it at the cap. Every KWin since 5.27 allows 4 (2 to 16
/// up to 6.0, 2 to 4 since 6.1).
pub(crate) const KWIN_MIN_BUFFERS: u32 = 4;

/// The `min-buffers` of a share's head on this desktop: [`KWIN_MIN_BUFFERS`] on KDE Plasma (where the compositor
/// behind the portal is KWin), no buffer count anywhere else. Asking for one makes pipewiresrc renegotiate its
/// buffers right after the stream started; KWin takes that, but the wlroots portal backend does not survive it in
/// about half the starts (no frames at all, or the portal exits), and mutter's own 8 need no help.
/// TINHAT_SHARE_MIN_BUFFERS=N overrides it (0 = never ask). A compositor that refused the request once
/// ([`note_buffer_refusal`]) is not asked again in this run.
pub(crate) fn share_min_buffers() -> u32 {
    if BUFFER_REQUEST_REFUSED.load(std::sync::atomic::Ordering::SeqCst) {
        return 0;
    }
    min_buffers_for(std::env::var("XDG_CURRENT_DESKTOP").ok().as_deref(), std::env::var("TINHAT_SHARE_MIN_BUFFERS").ok().as_deref())
}

/// [`share_min_buffers`] for a desktop name list (XDG_CURRENT_DESKTOP, colon-separated) and the override's text.
pub(crate) fn min_buffers_for(xdg_current_desktop: Option<&str>, switch: Option<&str>) -> u32 {
    if let Some(n) = switch.and_then(|v| v.trim().parse::<u32>().ok()) {
        return n.min(16);
    }
    let kde = xdg_current_desktop.is_some_and(|d| d.split(':').any(|p| p.trim().eq_ignore_ascii_case("KDE")));
    if kde {
        KWIN_MIN_BUFFERS
    } else {
        0
    }
}

static BUFFER_REQUEST_REFUSED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// `why` is a capture error's text. When it is the compositor refusing the stream buffers we asked for, later heads
/// of this run stop asking ([`share_min_buffers`]): the share that hit it fails, the next one starts. True = it was.
/// PipeWire words the refusal "error alloc buffers: Invalid argument" and pipewiresrc posts it as
/// "stream error: error alloc buffers: …"; an allocation failure further down the pipeline is worded differently.
pub(crate) fn note_buffer_refusal(why: &str) -> bool {
    if !buffer_request_refused(why) {
        return false;
    }
    if !BUFFER_REQUEST_REFUSED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        eprintln!("[capture] the compositor refused the stream buffers this share asked for ({why}) — later shares take its own count");
    }
    true
}

/// Is this capture error PipeWire refusing the stream buffers that were asked for?
pub(crate) fn buffer_request_refused(why: &str) -> bool {
    let w = why.to_ascii_lowercase();
    w.contains("stream error") && w.contains("alloc buffers")
}

/// Drive the KDE portal picker and start a single-source cast. Async — run on the
/// app's tokio runtime. The cursor is embedded into the frames (so peers see it);
/// `PersistMode::DoNot` means the picker shows every time (swap to
/// `PersistMode::Application` + a saved restore token later to skip it).
pub async fn pick_screencast() -> Result<ScreenCast> {
    let proxy = Screencast::new().await?;
    let session = proxy.create_session(Default::default()).await?;

    proxy
        .select_sources(
            &session,
            SelectSourcesOptions::default()
                .set_cursor_mode(CursorMode::Embedded)
                .set_sources(SourceType::Monitor | SourceType::Window)
                .set_multiple(false)
                .set_persist_mode(PersistMode::DoNot),
        )
        .await?;

    let streams = proxy
        .start(&session, None, Default::default())
        .await?
        .response()?;
    let Some(stream) = streams.streams().first() else {
        bail!("screen-cast portal returned no streams (user cancelled?)");
    };
    let node_id = stream.pipe_wire_node_id();
    let size = stream
        .size()
        .map(|(w, h)| (w.max(0) as u32, h.max(0) as u32))
        .filter(|(w, h)| *w > 0 && *h > 0);
    eprintln!("[capture] portal source size = {size:?}");

    let fd = proxy.open_pipe_wire_remote(&session, Default::default()).await?;
    // 0.8.20: the portal is allowed to omit the size (the KDE portal did, for a monitor
    // whose size it had reported before),
    // and a missing size silently demoted the capture to the CPU scale path — a 5K
    // frame lanczos-scaled per frame on the CPU publishes at ~11 fps (the choppy
    // recording of a 0.8.20 field report). Ask the PipeWire stream itself: a throwaway
    // pipewiresrc -> fakesink negotiates the stream format in well under a second.
    let size = match size {
        Some(s) => Some(s),
        None => {
            let probed = tokio::task::spawn_blocking({
                let raw = fd.as_raw_fd();
                move || probe_stream_size(raw, node_id)
            })
            .await
            .ok()
            .flatten();
            eprintln!("[capture] portal gave no size -> PipeWire stream probe = {probed:?}");
            probed
        }
    };
    Ok(ScreenCast {
        fd,
        node_id,
        size,
        learned: Default::default(),
        session,
    })
}

/// Negotiate the cast's format once with a throwaway pipeline and read width/height
/// off the caps. pipewiresrc dups the fd it is given, so the real capture pipeline
/// connects afterwards exactly as before (re-publishes already reuse the fd this way).
/// None on any failure — the caller keeps the old behaviour (CPU path).
///
/// Later: on a KDE desktop this probe had returned None on EVERY share since 0.8.26 (all of its logs), so every
/// such share took the CPU path. It now uses the capture's own source element ([`source_head`]) and reads the caps at the
/// first BUFFER (they are final by then), for up to 4 s, and says why it failed — the caps it saw, or that no frame came.
/// The capture no longer depends on it (the GPU path learns the size from its first frame, call.rs start_share_capture).
fn probe_stream_size(fd: i32, node_id: u32) -> Option<(u32, u32)> {
    use gst::prelude::*;
    use gstreamer as gst;
    let desc = format!("{} ! fakesink name=fs sync=false", source_head(Some(fd), node_id, 0));
    let pipeline = match gst::parse::launch(&desc).map(|e| e.downcast::<gst::Pipeline>()) {
        Ok(Ok(p)) => p,
        _ => {
            eprintln!("[capture] size probe: could not build the probe pipeline");
            return None;
        }
    };
    let pad = pipeline.by_name("fs")?.static_pad("sink")?;
    // (width, height) or the caps text when they carry no usable size; set by the first buffer
    let seen: std::sync::Arc<std::sync::Mutex<Option<Result<(u32, u32), String>>>> = Default::default();
    let seen_cb = seen.clone();
    pad.add_probe(gst::PadProbeType::BUFFER, move |pad, _| {
        let caps = pad.current_caps();
        let size = caps.as_ref().and_then(|c| c.structure(0)).and_then(|s| {
            match (s.get::<i32>("width"), s.get::<i32>("height")) {
                (Ok(w), Ok(h)) if w > 0 && h > 0 => Some((w as u32, h as u32)),
                _ => None,
            }
        });
        *seen_cb.lock().unwrap() = Some(size.ok_or_else(|| caps.map(|c| c.to_string()).unwrap_or_else(|| "no caps".into())));
        gst::PadProbeReturn::Remove
    });
    // round 9: the probe is a capture start like every other — under a lease (refused while watching), gated at its
    // source (leased_play)
    let Some(lease) = crate::capture_lease::take(crate::capture_lease::Kind::Screen, None) else {
        eprintln!("[capture] size probe: watching — not started");
        return None;
    };
    if !crate::call::leased_play(&lease, &pipeline, 6000, "share size probe") {
        eprintln!("[capture] size probe: the probe pipeline did not start");
        crate::call::set_state_bounded(pipeline.upcast_ref(), gst::State::Null, 1500, "share size probe");
        return None;
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(4000);
    let mut out = None;
    while std::time::Instant::now() < deadline {
        if let Some(r) = seen.lock().unwrap().clone() {
            out = Some(r);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(40));
    }
    crate::call::set_state_bounded(pipeline.upcast_ref(), gst::State::Null, 1500, "share size probe");
    match out {
        Some(Ok(size)) => Some(size),
        Some(Err(caps)) => {
            eprintln!("[capture] size probe: the first frame's caps carry no size: {caps}");
            None
        }
        None => {
            let caps = pad.current_caps().map(|c| c.to_string()).unwrap_or_else(|| "none".into());
            eprintln!("[capture] size probe: no frame in 4 s (caps so far: {caps})");
            None
        }
    }
}

/// Whether share capture heads carry the frame-rate cap: on unless TINHAT_SHARE_NO_RATE_CAP is set (to anything).
pub(crate) fn source_rate_cap_on() -> bool {
    std::env::var_os("TINHAT_SHARE_NO_RATE_CAP").is_none()
}

/// 12.1.9 (SHARE-GAME-LAG): the zero-copy capture is opt-in for the A/B while it is proven on real desktops:
/// TINHAT_SHARE_DMABUF=1 turns it on.
pub(crate) fn share_dmabuf_on() -> bool {
    std::env::var("TINHAT_SHARE_DMABUF").map(|v| v == "1").unwrap_or(false)
}

/// The zero-copy head: like [`source_head_with_cap`], but it accepts ONLY the compositor's DMA-BUF (`memory:DMABuf`),
/// capped first, then uncapped. No shm structure on purpose: a compositor that can't hand over a DMA-BUF must make this
/// candidate FAIL (no first frame), so the share falls back to the CUDA path instead of uploading a full-size shm frame
/// into GL, which would cost what the CUDA path costs and save nothing.
pub(crate) fn source_head_dmabuf(head: &str, fps: u32, cap: bool) -> String {
    if fps == 0 || !cap {
        return format!("{head} ! capsfilter name=ratecap caps=\"video/x-raw(memory:DMABuf)\"");
    }
    format!(
        "{head} ! capsfilter name=ratecap caps=\"video/x-raw(memory:DMABuf),max-framerate=(fraction){fps}/1; video/x-raw(memory:DMABuf)\""
    )
}

/// The app's head: [`source_head_with_cap`] with the cap on unless switched off.
pub(crate) fn source_head_paced(head: &str, fps: u32) -> String {
    source_head_with_cap(head, fps, source_rate_cap_on())
}

/// `head ! capsfilter name=ratecap caps="video/x-raw(ANY),max-framerate=fps/1; video/x-raw(ANY)"`, or the head alone
/// (cap off, or fps 0). TWO structures, in order of preference, each its own EnumFormat for PipeWire:
///   1. max-framerate EXACTLY fps — not a [0, fps] range: pipewiresrc 1.4 exports a range with its MINIMUM as the
///      preferred value, which against KWin's [0, refresh] offer can settle on 0 (= unthrottled) and against a
///      [1, refresh] offer (mutter, xdpw, Hyprland) on 1 fps;
///   2. anything — so a producer whose maximum can't be fps (a fixed higher one, a slower display) still negotiates,
///      uncapped as before, inside the same negotiation (no failed start, no app-level retry).
/// A named capsfilter with QUOTED caps: gst_parse rejects bare caps holding ranges or lists.
pub(crate) fn source_head_with_cap(head: &str, fps: u32, cap: bool) -> String {
    if fps == 0 || !cap {
        return head.to_string();
    }
    format!("{head} ! capsfilter name=ratecap caps=\"video/x-raw(ANY),max-framerate=(fraction){fps}/1; video/x-raw(ANY)\"")
}
