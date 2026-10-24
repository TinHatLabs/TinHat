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
    /// shared with the Windows module (`d3d11screencapturesrc` there). The
    /// string here is EXACTLY the head the capture builders always used.
    pub(crate) fn src_fragment(&self) -> String {
        format!(
            "pipewiresrc fd={} path={} do-timestamp=true keepalive-time=1000",
            self.raw_fd(),
            self.node_id()
        )
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
/// such share took the CPU path. It now uses the capture's own source head (keepalive included) and reads the caps at the
/// first BUFFER (they are final by then), for up to 4 s, and says why it failed — the caps it saw, or that no frame came.
/// The capture no longer depends on it (the GPU path learns the size from its first frame, call.rs start_share_capture).
fn probe_stream_size(fd: i32, node_id: u32) -> Option<(u32, u32)> {
    use gst::prelude::*;
    use gstreamer as gst;
    let desc = format!(
        "pipewiresrc fd={fd} path={node_id} do-timestamp=true keepalive-time=1000 ! fakesink name=fs sync=false"
    );
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
    if !crate::call::set_state_bounded(pipeline.upcast_ref(), gst::State::Playing, 6000, "share size probe") {
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
