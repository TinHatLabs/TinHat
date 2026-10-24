//! Windows screen capture — monitors via DXGI Desktop Duplication, windows via
//! Windows Graphics Capture (WGC), both through `d3d11screencapturesrc`.
//!
//! Mirrors the public API of the Linux portal module (`capture.rs`): a
//! [`ScreenCast`] handle that the capture pipeline builders turn into a source
//! head via [`ScreenCast::src_fragment`]. Where Linux pops the KDE system picker
//! (an OS dialog), Windows has no system picker reachable from plain Win32, so
//! the app presents its own: [`list_share_sources`] enumerates monitors +
//! shareable top-level windows for the QML picker dialog, and the chosen entry
//! comes back through [`ScreenCast::from_parts`]. [`pick_screencast`] (the
//! portal-shaped entry point used by non-UI callers) falls back to the primary
//! monitor.
//!
//! No fd/node in this model: the GStreamer element owns the capture session, so
//! [`ScreenCast::close`] is a no-op (dropping the pipeline stops the capture)
//! and the fd/node accessors exist only so Linux-only legacy paths compile.

use anyhow::{bail, Result};

// ── minimal Win32 FFI (no external crate — matches platform.rs's approach) ──

#[repr(C)]
#[derive(Clone, Copy)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[repr(C)]
struct MonitorInfoExW {
    cb_size: u32,
    rc_monitor: Rect,
    rc_work: Rect,
    dw_flags: u32,
    sz_device: [u16; 32],
}

const MONITORINFOF_PRIMARY: u32 = 1;
const GWL_EXSTYLE: i32 = -20;
const WS_EX_TOOLWINDOW: i32 = 0x0000_0080;
const GA_ROOT: u32 = 2;
const DWMWA_CLOAKED: u32 = 14;

#[link(name = "user32")]
extern "system" {
    fn EnumDisplayMonitors(
        hdc: isize,
        clip: *const Rect,
        cb: extern "system" fn(isize, isize, *mut Rect, isize) -> i32,
        data: isize,
    ) -> i32;
    fn GetMonitorInfoW(hmon: isize, info: *mut MonitorInfoExW) -> i32;
    fn EnumWindows(cb: extern "system" fn(isize, isize) -> i32, lparam: isize) -> i32;
    fn IsWindow(hwnd: isize) -> i32;
    fn IsWindowVisible(hwnd: isize) -> i32;
    fn IsIconic(hwnd: isize) -> i32;
    fn GetWindowTextW(hwnd: isize, buf: *mut u16, len: i32) -> i32;
    fn GetWindowLongW(hwnd: isize, idx: i32) -> i32;
    fn GetAncestor(hwnd: isize, flags: u32) -> isize;
    fn GetWindowRect(hwnd: isize, rect: *mut Rect) -> i32;
    fn GetShellWindow() -> isize;
}

#[link(name = "dwmapi")]
extern "system" {
    fn DwmGetWindowAttribute(hwnd: isize, attr: u32, out: *mut core::ffi::c_void, cb: u32) -> i32;
}

fn utf16_to_string(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

// ── source enumeration (feeds the QML picker) ────────────────────────────────

/// One pickable capture source. `kind` is `"monitor"` or `"window"`; `handle`
/// is the HMONITOR / HWND as an integer (valid only for the enumeration's
/// lifetime — windows close, monitors unplug — so [`ScreenCast::from_parts`]
/// re-validates it).
pub struct ShareSource {
    pub kind: &'static str,
    pub handle: u64,
    pub label: String,
    pub width: u32,
    pub height: u32,
}

struct MonitorHit {
    hmon: isize,
    rect: Rect,
    primary: bool,
    device: String,
}

extern "system" fn monitor_cb(hmon: isize, _hdc: isize, _rc: *mut Rect, data: isize) -> i32 {
    let out = unsafe { &mut *(data as *mut Vec<MonitorHit>) };
    let mut info = MonitorInfoExW {
        cb_size: std::mem::size_of::<MonitorInfoExW>() as u32,
        rc_monitor: Rect { left: 0, top: 0, right: 0, bottom: 0 },
        rc_work: Rect { left: 0, top: 0, right: 0, bottom: 0 },
        dw_flags: 0,
        sz_device: [0; 32],
    };
    if unsafe { GetMonitorInfoW(hmon, &mut info) } != 0 {
        out.push(MonitorHit {
            hmon,
            rect: info.rc_monitor,
            primary: info.dw_flags & MONITORINFOF_PRIMARY != 0,
            device: utf16_to_string(&info.sz_device),
        });
    }
    1 // continue
}

fn monitors() -> Vec<MonitorHit> {
    let mut hits: Vec<MonitorHit> = Vec::new();
    unsafe {
        EnumDisplayMonitors(0, std::ptr::null(), monitor_cb, &mut hits as *mut _ as isize);
    }
    hits
}

extern "system" fn window_cb(hwnd: isize, lparam: isize) -> i32 {
    let out = unsafe { &mut *(lparam as *mut Vec<(isize, String, Rect)>) };
    unsafe {
        // Standard share-picker heuristics: visible, top-level, titled, not a
        // tool window, not minimized (WGC shows stale/black content for iconic
        // windows), not DWM-cloaked (suspended UWP ghosts), not the shell.
        if IsWindowVisible(hwnd) == 0 || IsIconic(hwnd) != 0 {
            return 1;
        }
        if GetAncestor(hwnd, GA_ROOT) != hwnd || hwnd == GetShellWindow() {
            return 1;
        }
        if GetWindowLongW(hwnd, GWL_EXSTYLE) & WS_EX_TOOLWINDOW != 0 {
            return 1;
        }
        let mut cloaked: u32 = 0;
        if DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            &mut cloaked as *mut _ as *mut core::ffi::c_void,
            4,
        ) == 0
            && cloaked != 0
        {
            return 1;
        }
        let mut buf = [0u16; 256];
        let n = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
        if n <= 0 {
            return 1;
        }
        let title = utf16_to_string(&buf);
        if title.trim().is_empty() {
            return 1;
        }
        let mut rect = Rect { left: 0, top: 0, right: 0, bottom: 0 };
        if GetWindowRect(hwnd, &mut rect) == 0 {
            return 1;
        }
        out.push((hwnd, title, rect));
    }
    1
}

/// Enumerate the pickable sources: every monitor first (primary leading), then
/// every shareable top-level window. Cheap (one Win32 walk) — call fresh each
/// time the picker opens so the window list is current.
pub fn list_share_sources() -> Vec<ShareSource> {
    let mut out = Vec::new();
    let mut mons = monitors();
    mons.sort_by_key(|m| !m.primary); // primary first
    for (i, m) in mons.iter().enumerate() {
        let w = (m.rect.right - m.rect.left).max(0) as u32;
        let h = (m.rect.bottom - m.rect.top).max(0) as u32;
        let primary = if m.primary { ", primary" } else { "" };
        let _ = &m.device; // device name kept for future multi-GPU labeling
        out.push(ShareSource {
            kind: "monitor",
            handle: m.hmon as u64,
            label: format!("Screen {} ({w}\u{00d7}{h}{primary})", i + 1),
            width: w,
            height: h,
        });
    }
    let mut wins: Vec<(isize, String, Rect)> = Vec::new();
    unsafe {
        EnumWindows(window_cb, &mut wins as *mut _ as isize);
    }
    for (hwnd, title, rect) in wins {
        let w = (rect.right - rect.left).max(0) as u32;
        let h = (rect.bottom - rect.top).max(0) as u32;
        if w == 0 || h == 0 {
            continue;
        }
        out.push(ShareSource {
            kind: "window",
            handle: hwnd as u64,
            label: title,
            width: w,
            height: h,
        });
    }
    out
}

// ── the capture handle ───────────────────────────────────────────────────────

enum SourceKind {
    Monitor,
    Window,
}

/// A selected capture source. Same-shape stand-in for the Linux portal handle;
/// holds no OS resources (the GStreamer element owns the capture session), so
/// keeping it alive for the pipeline's lifetime is a formality the shared code
/// already observes.
pub struct ScreenCast {
    kind: SourceKind,
    handle: u64,
    size: Option<(u32, u32)>,
    // Shared with the Linux handle: the size the capture saw on its first frame.
    learned: std::sync::Arc<std::sync::Mutex<Option<(u32, u32)>>>,
}

impl ScreenCast {
    /// Build from a picker selection (`kind` = `"monitor"` / `"window"`, `handle`
    /// from [`list_share_sources`]). Re-validates the handle — the source can
    /// vanish between enumeration and click — and re-reads its CURRENT size so
    /// the publish box matches the real aspect.
    pub fn from_parts(kind: &str, handle: u64) -> Result<ScreenCast> {
        match kind {
            "monitor" => {
                let mut info = MonitorInfoExW {
                    cb_size: std::mem::size_of::<MonitorInfoExW>() as u32,
                    rc_monitor: Rect { left: 0, top: 0, right: 0, bottom: 0 },
                    rc_work: Rect { left: 0, top: 0, right: 0, bottom: 0 },
                    dw_flags: 0,
                    sz_device: [0; 32],
                };
                if unsafe { GetMonitorInfoW(handle as isize, &mut info) } == 0 {
                    bail!("monitor is gone (display changed?) — pick again");
                }
                let w = (info.rc_monitor.right - info.rc_monitor.left).max(0) as u32;
                let h = (info.rc_monitor.bottom - info.rc_monitor.top).max(0) as u32;
                Ok(ScreenCast {
                    kind: SourceKind::Monitor,
                    handle,
                    size: (w > 0 && h > 0).then_some((w, h)),
                    learned: Default::default(),
                })
            }
            "window" => {
                let hwnd = handle as isize;
                if unsafe { IsWindow(hwnd) } == 0 {
                    bail!("window is gone — pick again");
                }
                let mut rect = Rect { left: 0, top: 0, right: 0, bottom: 0 };
                let ok = unsafe { GetWindowRect(hwnd, &mut rect) } != 0;
                let w = (rect.right - rect.left).max(0) as u32;
                let h = (rect.bottom - rect.top).max(0) as u32;
                Ok(ScreenCast {
                    kind: SourceKind::Window,
                    handle,
                    // A window can resize mid-share; the capture builder's one-shot
                    // outcaps probe re-reads the REAL negotiated size anyway, so this
                    // only seeds the quality-box choice.
                    size: (ok && w > 0 && h > 0).then_some((w, h)),
                    learned: Default::default(),
                })
            }
            other => bail!("unknown share source kind {other:?}"),
        }
    }

    /// The GStreamer source-head fragment for this capture. Monitors use DXGI
    /// Desktop Duplication (lowest overhead); windows need WGC (DXGI cannot
    /// capture a single window). Cursor is embedded, matching the portal's
    /// `CursorMode::Embedded` on Linux.
    pub(crate) fn src_fragment(&self) -> String {
        match self.kind {
            SourceKind::Monitor => format!(
                "d3d11screencapturesrc show-cursor=true monitor-handle={}",
                self.handle
            ),
            SourceKind::Window => format!(
                "d3d11screencapturesrc capture-api=wgc show-cursor=true window-handle={}",
                self.handle
            ),
        }
    }

    /// Like [`Self::src_fragment`] but with a FIXED framerate pinned directly on the
    /// capture element's caps, so WGC/DXGI paces the capture itself instead of copying
    /// every present of a 165Hz game and letting videorate discard most of them
    /// downstream — each of those per-present GPU copies contends with the game's own
    /// presents ("game very choppy while streaming even though it shows 165 fps").
    pub(crate) fn src_fragment_paced(&self, fps: u32) -> String {
        format!(
            "{} ! video/x-raw(memory:D3D11Memory),framerate={fps}/1",
            self.src_fragment()
        )
    }

    /// Linux legacy accessor (PipeWire remote fd). Never used on Windows paths.
    pub fn raw_fd(&self) -> i32 {
        -1
    }
    /// Linux legacy accessor (PipeWire node id). Never used on Windows paths.
    pub fn node_id(&self) -> u32 {
        0
    }
    /// Captured source size in pixels, if known at selection time (else the one the capture learned).
    pub fn size(&self) -> Option<(u32, u32)> {
        self.size.or_else(|| *self.learned.lock().unwrap())
    }
    /// Where the capture builders record the real source size.
    pub(crate) fn size_memo(&self) -> std::sync::Arc<std::sync::Mutex<Option<(u32, u32)>>> {
        self.learned.clone()
    }
    /// Nothing to close: the pipeline owns the capture session.
    pub async fn close(self) {}
}

/// Portal-shaped entry point (no UI): capture the PRIMARY monitor. The GUI's
/// share button goes through the QML picker + [`ScreenCast::from_parts`]
/// instead; this serves headless/CLI callers (`stream.rs`, tests).
pub async fn pick_screencast() -> Result<ScreenCast> {
    let mons = monitors();
    let Some(m) = mons.iter().find(|m| m.primary).or_else(|| mons.first()) else {
        bail!("no monitors found to capture");
    };
    let w = (m.rect.right - m.rect.left).max(0) as u32;
    let h = (m.rect.bottom - m.rect.top).max(0) as u32;
    eprintln!("[capture] windows: primary monitor {w}x{h} (no picker — headless path)");
    Ok(ScreenCast {
        kind: SourceKind::Monitor,
        handle: m.hmon as u64,
        size: (w > 0 && h > 0).then_some((w, h)),
        learned: Default::default(),
    })
}
