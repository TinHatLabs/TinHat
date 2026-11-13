//! Windows: the unread count as an overlay on our taskbar button(s) — `ITaskbarList3::SetOverlayIcon` with the red
//! disc [`super::render_overlay`] draws, `None` at 0 (the overlay goes). Hand-bound COM in the style of
//! `audioapps_win.rs` (no `windows`/`winapi` crate): `#[repr(C)]` vtables in header order, real GUIDs, every pointer
//! released on every path.
//!
//! Which windows: every visible, unowned top-level window of this process that is not a tool window — exactly the
//! ones with a taskbar button (the main window; a pop-out that has its own button gets the same count). Runs on the
//! GUI thread (an STA: Qt initialised OLE there), from nav's refresh. An unchanged (count, windows) pair is re-sent at
//! most once a minute (Explorer restarting drops overlays: the next refresh after a minute puts it back); after a
//! failed set (the button doesn't exist yet) the state is unknown and the next refresh sets it again, 0 included.

use std::ffi::c_void;
use std::sync::Mutex;
use std::time::{Duration, Instant};

type Hresult = i32;
type Hwnd = isize;
type Hicon = isize;
type Hbitmap = isize;

#[repr(C)]
struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

/// CLSID_TaskbarList {56FDF344-FD6D-11D0-958A-006097C9A090}
const CLSID_TASKBAR_LIST: Guid =
    Guid { data1: 0x56FD_F344, data2: 0xFD6D, data3: 0x11D0, data4: [0x95, 0x8A, 0x00, 0x60, 0x97, 0xC9, 0xA0, 0x90] };
/// IID_ITaskbarList3 {EA1AFB91-9E28-4B86-90E9-9E9F8A5EEFAF}
const IID_ITASKBAR_LIST3: Guid =
    Guid { data1: 0xEA1A_FB91, data2: 0x9E28, data3: 0x4B86, data4: [0x90, 0xE9, 0x9E, 0x9F, 0x8A, 0x5E, 0xEF, 0xAF] };

const COINIT_APARTMENTTHREADED: u32 = 0x2;
const CLSCTX_INPROC_SERVER: u32 = 0x1;
const RPC_E_CHANGED_MODE: Hresult = 0x8001_0106_u32 as i32;
const GW_OWNER: u32 = 4;
const GWL_EXSTYLE: i32 = -20;
const WS_EX_TOOLWINDOW: isize = 0x0000_0080;
const SM_CXSMICON: i32 = 49;

#[repr(C)]
struct IconInfo {
    f_icon: i32,
    x_hotspot: u32,
    y_hotspot: u32,
    hbm_mask: Hbitmap,
    hbm_color: Hbitmap,
}

#[link(name = "ole32")]
extern "system" {
    fn CoInitializeEx(reserved: *mut c_void, coinit: u32) -> Hresult;
    fn CoUninitialize();
    fn CoCreateInstance(
        clsid: *const Guid,
        outer: *mut c_void,
        clsctx: u32,
        iid: *const Guid,
        out: *mut *mut c_void,
    ) -> Hresult;
}

#[link(name = "user32")]
extern "system" {
    fn EnumWindows(cb: unsafe extern "system" fn(Hwnd, isize) -> i32, lparam: isize) -> i32;
    fn GetWindowThreadProcessId(hwnd: Hwnd, pid: *mut u32) -> u32;
    fn IsWindowVisible(hwnd: Hwnd) -> i32;
    fn GetWindow(hwnd: Hwnd, cmd: u32) -> Hwnd;
    fn GetWindowLongPtrW(hwnd: Hwnd, index: i32) -> isize;
    fn GetSystemMetrics(index: i32) -> i32;
    fn CreateIconIndirect(info: *const IconInfo) -> Hicon;
    fn DestroyIcon(icon: Hicon) -> i32;
}

#[link(name = "gdi32")]
extern "system" {
    fn CreateBitmap(w: i32, h: i32, planes: u32, bpp: u32, bits: *const c_void) -> Hbitmap;
    fn DeleteObject(obj: isize) -> i32;
}

#[link(name = "kernel32")]
extern "system" {
    fn GetCurrentProcessId() -> u32;
}

// ── COM vtables — slot ORDER is the ABI (shobjidl_core.h); `Unused` marks a slot we never call.

type Unused = unsafe extern "system" fn(*mut c_void) -> Hresult;

#[repr(C)]
struct IUnknownVtbl {
    query_interface: unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> Hresult,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
}

#[repr(C)]
struct ITaskbarListVtbl {
    base: IUnknownVtbl,
    hr_init: unsafe extern "system" fn(*mut c_void) -> Hresult,
    add_tab: Unused,
    delete_tab: Unused,
    activate_tab: Unused,
    set_active_alt: Unused,
}

#[repr(C)]
struct ITaskbarList2Vtbl {
    base: ITaskbarListVtbl,
    mark_fullscreen_window: Unused,
}

#[repr(C)]
struct ITaskbarList3Vtbl {
    base: ITaskbarList2Vtbl,
    set_progress_value: Unused,
    set_progress_state: Unused,
    register_tab: Unused,
    unregister_tab: Unused,
    set_tab_order: Unused,
    set_tab_active: Unused,
    thumb_bar_add_buttons: Unused,
    thumb_bar_update_buttons: Unused,
    thumb_bar_set_image_list: Unused,
    /// SetOverlayIcon(HWND, HICON, LPCWSTR description)
    set_overlay_icon: unsafe extern "system" fn(*mut c_void, Hwnd, Hicon, *const u16) -> Hresult,
    set_thumbnail_tooltip: Unused,
    set_thumbnail_clip: Unused,
}

#[repr(C)]
struct ITaskbarList3 {
    vtbl: *const ITaskbarList3Vtbl,
}

/// Owns one reference; released on drop.
struct TaskbarList(*mut ITaskbarList3);

impl Drop for TaskbarList {
    fn drop(&mut self) {
        // SAFETY: self.0 is a live ITaskbarList3 we hold one reference to.
        unsafe { ((*(*self.0).vtbl).base.base.base.release)(self.0.cast()) };
    }
}

/// Owns an HICON; destroyed on drop (the taskbar keeps its own copy).
struct Icon(Hicon);

impl Drop for Icon {
    fn drop(&mut self) {
        // SAFETY: self.0 came from CreateIconIndirect and is destroyed once.
        unsafe { DestroyIcon(self.0) };
    }
}

const RESEND: Duration = Duration::from_secs(60);

/// What the taskbar shows, as far as we know.
enum Shown {
    /// Nothing was ever set (a 0 needs no clearing).
    Nothing,
    /// This count on these windows, set at this time.
    Set(u32, Vec<Hwnd>, Instant),
    /// A set failed (maybe half-way): send the next number whatever it is, 0 included.
    Unknown,
}

static SHOWN: Mutex<Shown> = Mutex::new(Shown::Nothing);

pub(super) fn publish(n: u32) {
    let windows = taskbar_windows();
    if windows.is_empty() {
        return; // no button yet: the next refresh tries again
    }
    let mut shown = SHOWN.lock().unwrap_or_else(|e| e.into_inner());
    let before = match &*shown {
        Shown::Set(m, w, at) if *m == n && *w == windows && at.elapsed() < RESEND => return,
        Shown::Nothing if n == 0 => return, // nothing drawn yet: nothing to clear
        Shown::Set(m, _, _) => Some(*m),
        _ => None,
    };
    match set_overlay(&windows, n) {
        Ok(()) => {
            if before != Some(n) {
                eprintln!("[badge] taskbar count {n} on {} button(s)", windows.len());
            }
            *shown = Shown::Set(n, windows, Instant::now());
        }
        Err(e) => {
            eprintln!("[badge] taskbar overlay failed: {e}");
            *shown = Shown::Unknown;
        }
    }
}

/// This process's windows that have a taskbar button: top-level, visible, unowned, not a tool window.
fn taskbar_windows() -> Vec<Hwnd> {
    unsafe extern "system" fn each(hwnd: Hwnd, lparam: isize) -> i32 {
        // SAFETY: lparam is the &mut Vec passed below, alive for the whole EnumWindows call.
        let out = unsafe { &mut *(lparam as *mut Vec<Hwnd>) };
        let mut pid = 0u32;
        // SAFETY: plain Win32 queries on a window handle EnumWindows just gave us.
        unsafe {
            GetWindowThreadProcessId(hwnd, &mut pid);
            if pid == GetCurrentProcessId()
                && IsWindowVisible(hwnd) != 0
                && GetWindow(hwnd, GW_OWNER) == 0
                && GetWindowLongPtrW(hwnd, GWL_EXSTYLE) & WS_EX_TOOLWINDOW == 0
            {
                out.push(hwnd);
            }
        }
        1 // keep going
    }
    let mut out: Vec<Hwnd> = Vec::new();
    // SAFETY: the callback only touches `out` through lparam during this call.
    unsafe { EnumWindows(each, &mut out as *mut Vec<Hwnd> as isize) };
    out.sort_unstable();
    out
}

fn set_overlay(windows: &[Hwnd], n: u32) -> Result<(), String> {
    // SAFETY: COM on this (GUI) thread; balanced CoUninitialize only when this call initialised it.
    let init = unsafe { CoInitializeEx(std::ptr::null_mut(), COINIT_APARTMENTTHREADED) };
    if init < 0 && init != RPC_E_CHANGED_MODE {
        return Err(format!("CoInitializeEx 0x{:08x}", init as u32));
    }
    let r = set_overlay_com(windows, n);
    if init >= 0 {
        // SAFETY: matches the successful CoInitializeEx above.
        unsafe { CoUninitialize() };
    }
    r
}

fn set_overlay_com(windows: &[Hwnd], n: u32) -> Result<(), String> {
    let mut raw: *mut c_void = std::ptr::null_mut();
    // SAFETY: standard CoCreateInstance; `raw` receives an ITaskbarList3 with one reference on success.
    let hr = unsafe {
        CoCreateInstance(&CLSID_TASKBAR_LIST, std::ptr::null_mut(), CLSCTX_INPROC_SERVER, &IID_ITASKBAR_LIST3, &mut raw)
    };
    if hr < 0 || raw.is_null() {
        return Err(format!("CoCreateInstance(TaskbarList) 0x{:08x}", hr as u32));
    }
    let tb = TaskbarList(raw.cast());
    // SAFETY: tb.0 is live; HrInit must be called once before any other method.
    let hr = unsafe { ((*(*tb.0).vtbl).base.base.hr_init)(tb.0.cast()) };
    if hr < 0 {
        return Err(format!("HrInit 0x{:08x}", hr as u32));
    }
    let icon = if n == 0 { None } else { Some(make_icon(n)?) };
    let desc: Vec<u16> = if n == 0 {
        vec![0]
    } else {
        format!("{n} unread").encode_utf16().chain(std::iter::once(0)).collect()
    };
    let mut first_err = None;
    for &hwnd in windows {
        // SAFETY: tb.0 is live and initialised; the icon (or 0 = remove) and the NUL-terminated description outlive
        // the call; the taskbar copies the icon.
        let hr = unsafe {
            ((*(*tb.0).vtbl).set_overlay_icon)(
                tb.0.cast(),
                hwnd,
                icon.as_ref().map_or(0, |i| i.0),
                if n == 0 { std::ptr::null() } else { desc.as_ptr() },
            )
        };
        if hr < 0 && first_err.is_none() {
            first_err = Some(format!("SetOverlayIcon 0x{:08x}", hr as u32));
        }
    }
    first_err.map_or(Ok(()), Err)
}

fn make_icon(n: u32) -> Result<Icon, String> {
    // SAFETY: a plain metric query.
    let size = usize::try_from(unsafe { GetSystemMetrics(SM_CXSMICON) }).unwrap_or(16).clamp(16, 64);
    let px = super::render_overlay(n, size);
    let bgra: Vec<u8> = px.iter().flatten().copied().collect();
    // the AND mask: 1 bpp, WORD-aligned rows, MSB = leftmost pixel; 1 = transparent (where alpha is 0), for whatever
    // reads the mask instead of the alpha
    let stride = size.div_ceil(16) * 2;
    let mut mask = vec![0u8; stride * size];
    for (i, p) in px.iter().enumerate() {
        if p[3] == 0 {
            let (x, y) = (i % size, i / size);
            mask[y * stride + x / 8] |= 0x80 >> (x % 8);
        }
    }
    let s = size as i32;
    // SAFETY: the bit buffers are exactly the size CreateBitmap reads for these dimensions and outlive the calls;
    // both bitmaps are deleted after CreateIconIndirect copied them.
    unsafe {
        let color = CreateBitmap(s, s, 1, 32, bgra.as_ptr().cast());
        let mono = CreateBitmap(s, s, 1, 1, mask.as_ptr().cast());
        let icon = if color != 0 && mono != 0 {
            CreateIconIndirect(&IconInfo { f_icon: 1, x_hotspot: 0, y_hotspot: 0, hbm_mask: mono, hbm_color: color })
        } else {
            0
        };
        if color != 0 {
            DeleteObject(color);
        }
        if mono != 0 {
            DeleteObject(mono);
        }
        if icon == 0 {
            return Err("CreateIconIndirect failed".into());
        }
        Ok(Icon(icon))
    }
}
