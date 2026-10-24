//! Windows "share audio from this app" enumerator — the applications currently
//! rendering audio, one entry per process, so the music-share picker can offer
//! per-APP capture instead of whole-endpoint monitors (HANDOFF_BAREMETAL §7 item 8:
//! "per-APP listing needs an audio-session enumerator").
//!
//! Pairs with `call.rs`'s process-loopback head: pick a [`WinAudioSession`],
//! then capture it with
//! `wasapi2src loopback=true loopback-mode=include-process-tree loopback-target-pid=<pid>`
//! (Windows 10 20H1+ `AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK`, device-less).
//! The pid reported is the process that OWNS the audio session. For Chromium
//! browsers that is the audio-service utility child, not the browser's main process
//! — which is exactly the pid process loopback wants (it captures that process plus
//! its children), so no parent-walk is needed for capture; a picker that wants one
//! row per *app* can group by `exe`.
//!
//! Mechanism: the WASAPI session API (`mmdeviceapi` / `audiopolicy`) — the same
//! list the Volume Mixer and Discord read. Hand-bound COM in the style of
//! `capture_windows.rs` (no `windows`/`winapi` crate): `#[repr(C)]` vtables in the
//! exact header order (IUnknown first, parent interface before derived), real GUID
//! constants, and an RAII [`ComPtr`] so every interface pointer is `Release`d on
//! every path. COM is initialised per call (MTA, tolerating an existing apartment —
//! the Qt GUI thread is an STA) and uninitialised before returning; no COM object
//! outlives the call, so [`list_sessions`] may be called from any thread.
//!
//! Cost: one device-collection walk + one session enumeration per active render
//! endpoint + one `OpenProcess` per distinct pid — a few milliseconds. Call it fresh
//! each time the picker opens; do not poll it at 1 Hz.

#![cfg(windows)]

use core::ffi::c_void;
use std::ptr::null_mut;

// ── minimal Win32/COM FFI (no external crate — matches capture_windows.rs) ───

type Hresult = i32;
const S_OK: Hresult = 0;
const S_FALSE: Hresult = 1;
/// CoInitializeEx: the thread already has an apartment of the OTHER model. COM is
/// still usable on it; the balancing CoUninitialize belongs to whoever set it up.
const RPC_E_CHANGED_MODE: Hresult = 0x8001_0106_u32 as Hresult;
const COINIT_MULTITHREADED: u32 = 0;
/// CLSCTX_INPROC_SERVER | INPROC_HANDLER | LOCAL_SERVER | REMOTE_SERVER.
const CLSCTX_ALL: u32 = 0x17;
/// EDataFlow::eRender / ERole::eConsole.
const E_RENDER: u32 = 0;
const E_CONSOLE: u32 = 0;
const DEVICE_STATE_ACTIVE: u32 = 0x1;
/// AudioSessionState: Inactive = 0, Active = 1, Expired = 2.
const AUDIO_SESSION_STATE_ACTIVE: i32 = 1;
const AUDIO_SESSION_STATE_EXPIRED: i32 = 2;
/// Granted to everyone for every non-protected process, whatever its integrity level
/// (an elevated game is still readable from a normal Tin Hat).
const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;

#[repr(C)]
struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

/// CLSID_MMDeviceEnumerator {BCDE0395-E52F-467C-8E3D-C4579291692E}
const CLSID_MMDEVICE_ENUMERATOR: Guid = Guid {
    data1: 0xBCDE_0395,
    data2: 0xE52F,
    data3: 0x467C,
    data4: [0x8E, 0x3D, 0xC4, 0x57, 0x92, 0x91, 0x69, 0x2E],
};
/// IID_IMMDeviceEnumerator {A95664D2-9614-4F35-A746-DE8DB63617E6}
const IID_IMMDEVICE_ENUMERATOR: Guid = Guid {
    data1: 0xA956_64D2,
    data2: 0x9614,
    data3: 0x4F35,
    data4: [0xA7, 0x46, 0xDE, 0x8D, 0xB6, 0x36, 0x17, 0xE6],
};
/// IID_IAudioSessionManager2 {77AA99A0-1BD6-484F-8BC7-2C654C9A9B6F}
const IID_IAUDIO_SESSION_MANAGER2: Guid = Guid {
    data1: 0x77AA_99A0,
    data2: 0x1BD6,
    data3: 0x484F,
    data4: [0x8B, 0xC7, 0x2C, 0x65, 0x4C, 0x9A, 0x9B, 0x6F],
};
/// IID_IAudioSessionControl2 {BFB7FF88-7239-4FC9-8FA2-07C950BE9C6D}
const IID_IAUDIO_SESSION_CONTROL2: Guid = Guid {
    data1: 0xBFB7_FF88,
    data2: 0x7239,
    data3: 0x4FC9,
    data4: [0x8F, 0xA2, 0x07, 0xC9, 0x50, 0xBE, 0x9C, 0x6D],
};

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
    fn CoTaskMemFree(p: *mut c_void);
}

#[link(name = "kernel32")]
extern "system" {
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> isize;
    fn QueryFullProcessImageNameW(process: isize, flags: u32, buf: *mut u16, len: *mut u32) -> i32;
    fn CloseHandle(handle: isize) -> i32;
}

#[link(name = "shlwapi")]
extern "system" {
    fn SHLoadIndirectString(
        src: *const u16,
        out: *mut u16,
        out_len: u32,
        reserved: *mut *mut c_void,
    ) -> Hresult;
}

// ── COM vtables — slot ORDER is the ABI; names/signatures only matter for the
//    slots we call. `Unused` marks a slot we never touch (only its position counts).

type Unused = unsafe extern "system" fn(*mut c_void) -> Hresult;

#[repr(C)]
struct IUnknownVtbl {
    query_interface: unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> Hresult,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
}

#[repr(C)]
struct IMMDeviceEnumeratorVtbl {
    base: IUnknownVtbl,
    enum_audio_endpoints:
        unsafe extern "system" fn(*mut c_void, u32, u32, *mut *mut IMMDeviceCollection) -> Hresult,
    get_default_audio_endpoint:
        unsafe extern "system" fn(*mut c_void, u32, u32, *mut *mut IMMDevice) -> Hresult,
    get_device: Unused,
    register_endpoint_notification_callback: Unused,
    unregister_endpoint_notification_callback: Unused,
}

#[repr(C)]
struct IMMDeviceCollectionVtbl {
    base: IUnknownVtbl,
    get_count: unsafe extern "system" fn(*mut c_void, *mut u32) -> Hresult,
    item: unsafe extern "system" fn(*mut c_void, u32, *mut *mut IMMDevice) -> Hresult,
}

#[repr(C)]
struct IMMDeviceVtbl {
    base: IUnknownVtbl,
    /// Activate(REFIID, DWORD clsctx, PROPVARIANT *params, void **out) — FIRST after IUnknown.
    activate: unsafe extern "system" fn(*mut c_void, *const Guid, u32, *mut c_void, *mut *mut c_void) -> Hresult,
    open_property_store: Unused,
    get_id: unsafe extern "system" fn(*mut c_void, *mut *mut u16) -> Hresult,
    get_state: Unused,
}

#[repr(C)]
struct IAudioSessionManagerVtbl {
    base: IUnknownVtbl,
    get_audio_session_control: Unused,
    get_simple_audio_volume: Unused,
}

#[repr(C)]
struct IAudioSessionManager2Vtbl {
    base: IAudioSessionManagerVtbl,
    get_session_enumerator:
        unsafe extern "system" fn(*mut c_void, *mut *mut IAudioSessionEnumerator) -> Hresult,
    register_session_notification: Unused,
    unregister_session_notification: Unused,
    register_duck_notification: Unused,
    unregister_duck_notification: Unused,
}

#[repr(C)]
struct IAudioSessionEnumeratorVtbl {
    base: IUnknownVtbl,
    get_count: unsafe extern "system" fn(*mut c_void, *mut i32) -> Hresult,
    get_session: unsafe extern "system" fn(*mut c_void, i32, *mut *mut IAudioSessionControl) -> Hresult,
}

#[repr(C)]
struct IAudioSessionControlVtbl {
    base: IUnknownVtbl,
    get_state: unsafe extern "system" fn(*mut c_void, *mut i32) -> Hresult,
    get_display_name: unsafe extern "system" fn(*mut c_void, *mut *mut u16) -> Hresult,
    set_display_name: Unused,
    get_icon_path: Unused,
    set_icon_path: Unused,
    get_grouping_param: Unused,
    set_grouping_param: Unused,
    register_audio_session_notification: Unused,
    unregister_audio_session_notification: Unused,
}

#[repr(C)]
struct IAudioSessionControl2Vtbl {
    base: IAudioSessionControlVtbl,
    get_session_identifier: Unused,
    get_session_instance_identifier: Unused,
    get_process_id: unsafe extern "system" fn(*mut c_void, *mut u32) -> Hresult,
    /// S_OK = this is the "System Sounds" session, S_FALSE = a real app.
    is_system_sounds_session: unsafe extern "system" fn(*mut c_void) -> Hresult,
    set_ducking_preference: Unused,
}

// Interface objects: a COM object pointer points at its vtable pointer.
#[repr(C)]
struct IMMDeviceEnumerator {
    vtbl: *const IMMDeviceEnumeratorVtbl,
}
#[repr(C)]
struct IMMDeviceCollection {
    vtbl: *const IMMDeviceCollectionVtbl,
}
#[repr(C)]
struct IMMDevice {
    vtbl: *const IMMDeviceVtbl,
}
#[repr(C)]
struct IAudioSessionManager2 {
    vtbl: *const IAudioSessionManager2Vtbl,
}
#[repr(C)]
struct IAudioSessionEnumerator {
    vtbl: *const IAudioSessionEnumeratorVtbl,
}
#[repr(C)]
struct IAudioSessionControl {
    vtbl: *const IAudioSessionControlVtbl,
}
#[repr(C)]
struct IAudioSessionControl2 {
    vtbl: *const IAudioSessionControl2Vtbl,
}

trait Interface {
    type Vtbl;
}
impl Interface for IMMDeviceEnumerator {
    type Vtbl = IMMDeviceEnumeratorVtbl;
}
impl Interface for IMMDeviceCollection {
    type Vtbl = IMMDeviceCollectionVtbl;
}
impl Interface for IMMDevice {
    type Vtbl = IMMDeviceVtbl;
}
impl Interface for IAudioSessionManager2 {
    type Vtbl = IAudioSessionManager2Vtbl;
}
impl Interface for IAudioSessionEnumerator {
    type Vtbl = IAudioSessionEnumeratorVtbl;
}
impl Interface for IAudioSessionControl {
    type Vtbl = IAudioSessionControlVtbl;
}
impl Interface for IAudioSessionControl2 {
    type Vtbl = IAudioSessionControl2Vtbl;
}

/// Owning COM interface pointer: `Release`d on drop, so every early `return` /
/// `continue` in the walk below still balances the refcount (the never-leak rule).
/// Never holds null.
struct ComPtr<T: Interface>(*mut T);

impl<T: Interface> ComPtr<T> {
    /// Adopt an out-param the callee already `AddRef`'d for us; null → None.
    unsafe fn adopt(p: *mut T) -> Option<Self> {
        if p.is_null() {
            None
        } else {
            Some(ComPtr(p))
        }
    }
    fn this(&self) -> *mut c_void {
        self.0 as *mut c_void
    }
    unsafe fn vt(&self) -> &T::Vtbl {
        &**(self.0 as *const *const T::Vtbl)
    }
    /// Every vtable starts with IUnknown's three slots (repr(C) nesting), so the
    /// IUnknown methods are reachable through any interface pointer.
    unsafe fn unknown(&self) -> &IUnknownVtbl {
        &**(self.0 as *const *const IUnknownVtbl)
    }
    unsafe fn query<U: Interface>(&self, iid: &Guid) -> Option<ComPtr<U>> {
        let mut out: *mut c_void = null_mut();
        if (self.unknown().query_interface)(self.this(), iid, &mut out) < 0 {
            return None;
        }
        ComPtr::<U>::adopt(out as *mut U)
    }
}

impl<T: Interface> Drop for ComPtr<T> {
    fn drop(&mut self) {
        unsafe {
            (self.unknown().release)(self.this());
        }
    }
}

// ── string helpers ───────────────────────────────────────────────────────────

fn utf16_to_string(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// Read + free a CoTaskMem-allocated wide string (the session API's out-strings).
unsafe fn take_cotaskmem_string(p: *mut u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut n = 0usize;
    while *p.add(n) != 0 {
        n += 1;
    }
    let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, n));
    CoTaskMemFree(p as *mut c_void);
    s
}

/// Session display names may be indirect resource strings
/// (`@%SystemRoot%\...dll,-123`, `@{Package?ms-resource://...}` for UWP apps).
/// Resolve those the way the Volume Mixer does, or drop them rather than show the
/// raw reference; plain names pass through trimmed.
fn resolve_display_name(raw: String) -> String {
    let s = raw.trim();
    if !s.starts_with('@') {
        return s.to_string();
    }
    let src: Vec<u16> = s.encode_utf16().chain(std::iter::once(0)).collect();
    let mut buf = [0u16; 512];
    let hr = unsafe { SHLoadIndirectString(src.as_ptr(), buf.as_mut_ptr(), buf.len() as u32, null_mut()) };
    if hr < 0 {
        return String::new();
    }
    utf16_to_string(&buf).trim().to_string()
}

/// Full image path of `pid`, or None when the process is gone or protected.
fn process_image_path(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h == 0 {
            return None;
        }
        let mut path = None;
        // MAX_PATH is 260 but long-path-aware processes exist; grow once before giving up.
        for cap in [1024usize, 32 * 1024] {
            let mut buf = vec![0u16; cap];
            let mut len = cap as u32;
            if QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len) != 0 {
                path = Some(String::from_utf16_lossy(&buf[..len as usize]));
                break;
            }
        }
        CloseHandle(h);
        path
    }
}

/// `C:\Program Files\Google\Chrome\Application\chrome.exe` → `chrome`.
fn exe_base_name(path: &str) -> String {
    let file = path.rsplit(['\\', '/']).next().unwrap_or(path);
    let b = file.as_bytes();
    let stem = if b.len() > 4 && b[b.len() - 4..].eq_ignore_ascii_case(b".exe") {
        &file[..file.len() - 4]
    } else {
        file
    };
    stem.to_string()
}

// ── the enumeration ──────────────────────────────────────────────────────────

/// One process that has an audio session on a render endpoint.
#[derive(Clone, Debug)]
pub struct WinAudioSession {
    /// Process to hand to `wasapi2src loopback-target-pid=` (with
    /// `loopback-mode=include-process-tree`).
    pub pid: u32,
    /// Executable base name without extension (`chrome`, `Spotify`, `valheim`).
    pub exe: String,
    /// The session's own display name if the app set one (`Spotify`); usually empty.
    pub display_name: String,
    /// At least one of the process's sessions is `AudioSessionStateActive`
    /// (a stream is running right now, not merely opened-then-paused).
    pub active: bool,
}

impl WinAudioSession {
    /// What a picker should print: the app's own name when it set one, else the exe.
    pub fn label(&self) -> &str {
        if self.display_name.is_empty() {
            &self.exe
        } else {
            &self.display_name
        }
    }
}

/// A session as read off the API, before per-process merging.
struct RawSession {
    pid: u32,
    display_name: String,
    active: bool,
}

unsafe fn device_id(dev: &ComPtr<IMMDevice>) -> String {
    let mut id: *mut u16 = null_mut();
    if (dev.vt().get_id)(dev.this(), &mut id) < 0 {
        return String::new();
    }
    take_cotaskmem_string(id)
}

/// Append every usable session on one render endpoint to `out`.
fn sessions_on(dev: &ComPtr<IMMDevice>, out: &mut Vec<RawSession>) {
    unsafe {
        let mut p: *mut c_void = null_mut();
        let hr = (dev.vt().activate)(dev.this(), &IID_IAUDIO_SESSION_MANAGER2, CLSCTX_ALL, null_mut(), &mut p);
        if hr < 0 {
            return;
        }
        let Some(mgr) = ComPtr::<IAudioSessionManager2>::adopt(p as *mut _) else {
            return;
        };
        let mut e: *mut IAudioSessionEnumerator = null_mut();
        if (mgr.vt().get_session_enumerator)(mgr.this(), &mut e) < 0 {
            return;
        }
        let Some(list) = ComPtr::adopt(e) else {
            return;
        };
        let mut count = 0i32;
        if (list.vt().get_count)(list.this(), &mut count) < 0 {
            return;
        }
        for i in 0..count.max(0) {
            let mut c: *mut IAudioSessionControl = null_mut();
            if (list.vt().get_session)(list.this(), i, &mut c) < 0 {
                continue;
            }
            let Some(ctl) = ComPtr::adopt(c) else {
                continue;
            };
            // pid + system-sounds live on the v2 interface (Vista's v1 has neither).
            let Some(ctl2) = ctl.query::<IAudioSessionControl2>(&IID_IAUDIO_SESSION_CONTROL2) else {
                continue;
            };
            if (ctl2.vt().is_system_sounds_session)(ctl2.this()) == S_OK {
                continue;
            }
            let mut pid = 0u32;
            if (ctl2.vt().get_process_id)(ctl2.this(), &mut pid) < 0 || pid == 0 {
                continue; // no owning process (or the system-sounds/pid-0 session)
            }
            let mut state = 0i32;
            if (ctl.vt().get_state)(ctl.this(), &mut state) < 0 || state == AUDIO_SESSION_STATE_EXPIRED {
                continue; // the process closed its stream and is on the way out
            }
            let mut name: *mut u16 = null_mut();
            let display_name = if (ctl.vt().get_display_name)(ctl.this(), &mut name) >= 0 {
                resolve_display_name(take_cotaskmem_string(name))
            } else {
                String::new()
            };
            out.push(RawSession {
                pid,
                display_name,
                active: state == AUDIO_SESSION_STATE_ACTIVE,
            });
        }
    }
}

/// The system's MMDevice enumerator (COM must already be initialised on this thread).
fn device_enumerator() -> Option<ComPtr<IMMDeviceEnumerator>> {
    unsafe {
        let mut p: *mut c_void = null_mut();
        let hr = CoCreateInstance(
            &CLSID_MMDEVICE_ENUMERATOR,
            null_mut(),
            CLSCTX_ALL,
            &IID_IMMDEVICE_ENUMERATOR,
            &mut p,
        );
        if hr < 0 {
            eprintln!("[audioapps] CoCreateInstance(MMDeviceEnumerator) failed: 0x{:08x}", hr as u32);
            return None;
        }
        ComPtr::adopt(p as *mut _)
    }
}

/// The default render endpoint (eConsole — the one the Volume Mixer shows), or None
/// when there is no render device at all: E_NOTFOUND (0x80070490) on a headless VM,
/// with every output unplugged, or from a session-0 process (services, sshd
/// children). Nothing can be playing then, so an empty list is the right answer.
fn default_render_endpoint(en: &ComPtr<IMMDeviceEnumerator>) -> Option<ComPtr<IMMDevice>> {
    unsafe {
        let mut d: *mut IMMDevice = null_mut();
        let hr = (en.vt().get_default_audio_endpoint)(en.this(), E_RENDER, E_CONSOLE, &mut d);
        if hr < 0 {
            eprintln!("[audioapps] no default render endpoint: 0x{:08x}", hr as u32);
            return None;
        }
        ComPtr::adopt(d)
    }
}

/// Walk the default render endpoint, then every other active render endpoint.
/// Every COM object created here is released before this returns (RAII), which is
/// what lets the caller `CoUninitialize` right after.
fn collect() -> Vec<RawSession> {
    let mut raw = Vec::new();
    let Some(en) = device_enumerator() else {
        return raw;
    };
    unsafe {
        // Default render endpoint first.
        let mut default_id = String::new();
        if let Some(dev) = default_render_endpoint(&en) {
            default_id = device_id(&dev);
            sessions_on(&dev, &mut raw);
        }

        // Then every other ACTIVE render endpoint: one collection walk, and an app
        // playing to a non-default device (game on the headset while music is on the
        // speakers) stays shareable. The default is skipped by id, not re-walked.
        let mut c: *mut IMMDeviceCollection = null_mut();
        if (en.vt().enum_audio_endpoints)(en.this(), E_RENDER, DEVICE_STATE_ACTIVE, &mut c) >= 0 {
            if let Some(coll) = ComPtr::adopt(c) {
                let mut n = 0u32;
                if (coll.vt().get_count)(coll.this(), &mut n) >= 0 {
                    for i in 0..n {
                        let mut d: *mut IMMDevice = null_mut();
                        if (coll.vt().item)(coll.this(), i, &mut d) < 0 {
                            continue;
                        }
                        let Some(dev) = ComPtr::adopt(d) else {
                            continue;
                        };
                        if device_id(&dev) == default_id {
                            continue;
                        }
                        sessions_on(&dev, &mut raw);
                    }
                }
            }
        }
    }
    raw
}

/// Every audio session on the DEFAULT render endpoint plus every other active render
/// endpoint, merged to one entry per process: pid, exe base name without extension
/// (`chrome`), the session's display name if the app set one (`Spotify`), and
/// `active` = `AudioSessionStateActive` on at least one of that process's sessions
/// (Chromium opens a session per tab/stream, all owned by one audio-service pid).
///
/// Excludes: our own process, pid 0 / the "System Sounds" session, and sessions
/// with no process (exited/expired, or a protected process we cannot open).
/// Order: active first, then A–Z by [`WinAudioSession::label`].
///
/// Never fails: any COM/Win32 error logs to stderr and yields an empty list. COM is
/// initialised (MTA) and uninitialised inside this call — see the module docs — so
/// it is safe from any thread, including one that already has an STA.
pub fn list_sessions() -> Vec<WinAudioSession> {
    let me = std::process::id();
    let raw = unsafe {
        let hr = CoInitializeEx(null_mut(), COINIT_MULTITHREADED);
        // S_OK / S_FALSE: we own one init on this thread and must balance it.
        // RPC_E_CHANGED_MODE: already an STA (Qt's GUI thread) — usable, not ours to close.
        let balance = hr == S_OK || hr == S_FALSE;
        if hr < 0 && hr != RPC_E_CHANGED_MODE {
            eprintln!("[audioapps] CoInitializeEx failed: 0x{:08x}", hr as u32);
            return Vec::new();
        }
        let raw = collect(); // all COM objects released inside, BEFORE CoUninitialize
        if balance {
            CoUninitialize();
        }
        raw
    };

    let mut out: Vec<WinAudioSession> = Vec::new();
    for r in raw {
        if r.pid == me {
            continue;
        }
        if let Some(e) = out.iter_mut().find(|e| e.pid == r.pid) {
            e.active |= r.active;
            if e.display_name.is_empty() {
                e.display_name = r.display_name;
            }
            continue;
        }
        let Some(path) = process_image_path(r.pid) else {
            continue; // gone between enumeration and now, or protected: "no process"
        };
        out.push(WinAudioSession {
            pid: r.pid,
            exe: exe_base_name(&path),
            display_name: r.display_name,
            active: r.active,
        });
    }
    // Playing apps first, then A–Z on the label the picker will show.
    out.sort_by(|a, b| {
        b.active
            .cmp(&a.active)
            .then_with(|| a.label().to_lowercase().cmp(&b.label().to_lowercase()))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exe_base_name_strips_dir_and_extension() {
        assert_eq!(exe_base_name(r"C:\Program Files\Google\Chrome\Application\chrome.exe"), "chrome");
        assert_eq!(exe_base_name(r"C:\Users\x\AppData\Roaming\Spotify\Spotify.EXE"), "Spotify");
        assert_eq!(exe_base_name(r"D:\games\valheim"), "valheim");
        assert_eq!(exe_base_name("plain.exe"), "plain");
        assert_eq!(exe_base_name(".exe"), ".exe");
    }

    /// The whole module rests on vtable slot ORDER matching the SDK headers
    /// (IUnknown's 3, then parent interface before derived). These indexes were
    /// cross-checked against `offsetof(<Interface>Vtbl, <Method>) / sizeof(void*)`
    /// from the Windows SDK C headers (mmdeviceapi.h / audiopolicy.h, CINTERFACE) on
    /// the build box; the totals catch a missing or extra slot anywhere in a chain.
    #[test]
    fn vtable_slots_match_sdk_headers() {
        use std::mem::{offset_of, size_of};
        const P: usize = size_of::<usize>();
        assert_eq!(offset_of!(IUnknownVtbl, query_interface) / P, 0);
        assert_eq!(offset_of!(IUnknownVtbl, release) / P, 2);
        assert_eq!(offset_of!(IMMDeviceEnumeratorVtbl, enum_audio_endpoints) / P, 3);
        assert_eq!(offset_of!(IMMDeviceEnumeratorVtbl, get_default_audio_endpoint) / P, 4);
        assert_eq!(size_of::<IMMDeviceEnumeratorVtbl>() / P, 8);
        assert_eq!(offset_of!(IMMDeviceCollectionVtbl, get_count) / P, 3);
        assert_eq!(offset_of!(IMMDeviceCollectionVtbl, item) / P, 4);
        assert_eq!(size_of::<IMMDeviceCollectionVtbl>() / P, 5);
        assert_eq!(offset_of!(IMMDeviceVtbl, activate) / P, 3);
        assert_eq!(offset_of!(IMMDeviceVtbl, get_id) / P, 5);
        assert_eq!(size_of::<IMMDeviceVtbl>() / P, 7);
        assert_eq!(offset_of!(IAudioSessionManager2Vtbl, get_session_enumerator) / P, 5);
        assert_eq!(size_of::<IAudioSessionManager2Vtbl>() / P, 10);
        assert_eq!(offset_of!(IAudioSessionEnumeratorVtbl, get_count) / P, 3);
        assert_eq!(offset_of!(IAudioSessionEnumeratorVtbl, get_session) / P, 4);
        assert_eq!(size_of::<IAudioSessionEnumeratorVtbl>() / P, 5);
        assert_eq!(offset_of!(IAudioSessionControlVtbl, get_state) / P, 3);
        assert_eq!(offset_of!(IAudioSessionControlVtbl, get_display_name) / P, 4);
        assert_eq!(size_of::<IAudioSessionControlVtbl>() / P, 12);
        assert_eq!(offset_of!(IAudioSessionControl2Vtbl, get_process_id) / P, 14);
        assert_eq!(offset_of!(IAudioSessionControl2Vtbl, is_system_sounds_session) / P, 15);
        assert_eq!(size_of::<IAudioSessionControl2Vtbl>() / P, 17);
    }

    /// The hex transcription of each GUID against its canonical registry string.
    #[test]
    fn guids_match_canonical_strings() {
        fn canon(g: &Guid) -> String {
            let d = g.data4;
            format!(
                "{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}",
                g.data1, g.data2, g.data3, d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]
            )
        }
        assert_eq!(canon(&CLSID_MMDEVICE_ENUMERATOR), "BCDE0395-E52F-467C-8E3D-C4579291692E");
        assert_eq!(canon(&IID_IMMDEVICE_ENUMERATOR), "A95664D2-9614-4F35-A746-DE8DB63617E6");
        assert_eq!(canon(&IID_IAUDIO_SESSION_MANAGER2), "77AA99A0-1BD6-484F-8BC7-2C654C9A9B6F");
        assert_eq!(canon(&IID_IAUDIO_SESSION_CONTROL2), "BFB7FF88-7239-4FC9-8FA2-07C950BE9C6D");
        assert_eq!(RPC_E_CHANGED_MODE as u32, 0x8001_0106);
    }

    /// Smoke: enumerate on this box and print what we see. A headless box can
    /// legitimately have ZERO sessions, so an empty list is a pass; what must hold is
    /// no crash, no self entry, and that a second call (COM init/uninit balance)
    /// works just like the first.
    #[test]
    fn list_sessions_smoke() {
        let v = list_sessions();
        println!("[audioapps] {} process(es) with audio sessions", v.len());
        for s in &v {
            println!(
                "  pid {:>6}  exe={:<24} name={:<32} {}",
                s.pid,
                s.exe,
                s.display_name,
                if s.active { "ACTIVE" } else { "inactive" }
            );
        }
        assert!(v.iter().all(|s| s.pid != std::process::id()), "own process must be excluded");
        assert!(v.iter().all(|s| s.pid != 0), "pid 0 must be excluded");
        assert!(v.iter().all(|s| !s.exe.is_empty()), "every entry must have an exe name");
        let again = list_sessions();
        println!("[audioapps] second call: {} process(es)", again.len());
    }

    // Test-only: make THIS process render audio so the walk has something to find.
    #[link(name = "winmm")]
    extern "system" {
        fn PlaySoundW(name: *const u16, hmod: isize, flags: u32) -> i32;
    }
    const SND_ASYNC: u32 = 0x0001;
    const SND_NODEFAULT: u32 = 0x0002;
    const SND_LOOP: u32 = 0x0008;
    const SND_FILENAME: u32 = 0x0002_0000;

    /// Proof the COM walk works end to end: loop a stock Windows wav from this
    /// process, then the RAW walk must report our own pid as ACTIVE — while the
    /// public [`list_sessions`] must still hide it. Skips (with a note, not a
    /// failure) when the sound cannot start, i.e. this session has no render
    /// endpoint; SND_NODEFAULT keeps a missing wav from turning into a beep that
    /// would report success without an endpoint.
    #[test]
    fn raw_walk_sees_our_own_playing_session() {
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        let wav: Vec<u16> = format!(r"{root}\Media\ding.wav")
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let started =
            unsafe { PlaySoundW(wav.as_ptr(), 0, SND_FILENAME | SND_ASYNC | SND_LOOP | SND_NODEFAULT) } != 0;
        if !started {
            println!("[audioapps] PlaySound could not start — walk unverified here");
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(600));
        // SND_ASYNC reports success even with no device (the open fails later on
        // winmm's thread), so ask the walk itself whether this session has an
        // endpoint: session-0 processes (sshd children, services) never do.
        let (raw, endpoint) = unsafe {
            let hr = CoInitializeEx(null_mut(), COINIT_MULTITHREADED);
            let endpoint = device_enumerator().and_then(|en| default_render_endpoint(&en)).is_some();
            let r = if endpoint { collect() } else { Vec::new() };
            if hr == S_OK || hr == S_FALSE {
                CoUninitialize();
            }
            (r, endpoint)
        };
        let hidden = list_sessions();
        unsafe {
            PlaySoundW(std::ptr::null(), 0, 0); // stop the loop
        }
        if !endpoint {
            println!("[audioapps] no render endpoint in this session (session-0 ssh?) — walk unverified here; run from the console session");
            return;
        }
        let me = std::process::id();
        println!("[audioapps] raw walk while we play: {} session(s)", raw.len());
        for r in &raw {
            println!("  pid {:>6}  name={:<32} {}", r.pid, r.display_name, if r.active { "ACTIVE" } else { "inactive" });
        }
        let mine: Vec<&RawSession> = raw.iter().filter(|r| r.pid == me).collect();
        assert!(!mine.is_empty(), "our own playing session must be visible to the raw walk");
        assert!(mine.iter().any(|r| r.active), "our session must be ACTIVE while the wav loops");
        assert!(hidden.iter().all(|s| s.pid != me), "list_sessions must exclude our own process");
    }
}
