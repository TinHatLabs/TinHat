//! FFI to the C++ video bridge (`video_bridge.cpp`). Native remote-video tiles use
//! the proven `qml6glsink` path; these calls are the parts cxx-qt-lib can't express
//! (forcing OpenGL, registering the qml6 QML type, and attaching a gst sink to a
//! QML `GstGLQt6VideoItem`).

use std::ffi::CString;
use std::os::raw::{c_char, c_int, c_longlong, c_void};

extern "C" {
    fn tinhat_video_preinit();
    fn tinhat_video_set_graphics_api();
    fn tinhat_pump_events(ms: c_int);
    fn tinhat_pin_gstgl_for_qpa();
    fn tinhat_video_attach(pipeline: *mut c_void, object_name: *const c_char) -> c_int;
    fn tinhat_video_detach(object_name: *const c_char);
    fn tinhat_video_relayout(object_name: *const c_char);
    fn tinhat_video_predetach(pipeline: *mut c_void);
    fn tinhat_video_set_paused(pipeline: *mut c_void, paused: c_int);
    fn tinhat_video_is_live(pipeline: *mut c_void) -> c_int;
    fn tinhat_apply_color_scheme(name: *const c_char);
    fn tinhat_apply_color_scheme_file(name: *const c_char, path: *const c_char);
    fn tinhat_mpv_register();
    #[cfg(windows)]
    fn tinhat_force_quick_style();
    #[cfg(windows)]
    fn tinhat_dark_titlebars();
    fn tinhat_icon_theme();
    fn tinhat_app_icon(path: *const c_char);
    fn tinhat_input_watch_install();
    fn tinhat_input_idle_ms() -> c_longlong;
    fn tinhat_platform_reduced_motion() -> c_int;
    fn tinhat_caret_set_reduced(reduced: c_int);
}

/// 10.6 (REACH §2.4): start Tin Hat's own input clock — an application-wide event filter that
/// stamps every key / pointer / wheel / touch event in ANY Tin Hat window. The presence idle
/// probe's last fallback. Call once, after the QGuiApplication exists (GUI thread).
pub fn input_watch_install() {
    unsafe { tinhat_input_watch_install() }
}

/// Milliseconds since the last input event in any Tin Hat window (None before the clock is
/// installed — a headless mode). Safe from any thread (one atomic load).
pub fn input_idle_ms() -> Option<u64> {
    let v = unsafe { tinhat_input_idle_ms() };
    if v < 0 { None } else { Some(v as u64) }
}

/// 11.0 (GPU.md §1, R2): the OS asks for no animations (Windows: "Animation effects" off). Linux: false here — the
/// QML reads KDE's animation speed through Kirigami.Units. Needs no QGuiApplication.
pub fn platform_reduced_motion() -> bool {
    unsafe { tinhat_platform_reduced_motion() != 0 }
}

/// 11.0 (GPU.md §2.1, R5): Reduce motion → a steady text caret; off → it blinks for 5 s after input again. GUI thread.
pub fn caret_set_reduced(reduced: bool) {
    unsafe { tinhat_caret_set_reduced(if reduced { 1 } else { 0 }) }
}

/// Windows: force the palette-driven `Basic` QQC2 style (native style ignores
/// Kirigami.Theme — TEST_PLAN §2.5). Call BEFORE the engine's first Controls import.
#[cfg(windows)]
pub fn force_quick_style() {
    unsafe { tinhat_force_quick_style() }
}

/// Windows: dark DWM title bars for the main window + every later-created window.
/// Call AFTER engine.load (the main window must exist).
#[cfg(windows)]
pub fn dark_titlebars() {
    unsafe { tinhat_dark_titlebars() }
}

/// Register the icon theme. Windows: the bundled breeze rcc bundles (no system icon
/// theme exists there — every `icon.name` was blank). Linux: point the FALLBACK theme
/// at breeze so icon-only buttons still resolve on non-KDE desktops, where an
/// unresolved icon collapses the button to an invisible sliver. Call after the app
/// object exists, before engine.load.
pub fn icon_theme() {
    unsafe { tinhat_icon_theme() }
}

/// Window / taskbar icon from the bundled app-icon PNG (0.8.26). KDE takes the .desktop
/// icon on Linux; plain Qt on Wayland/other desktops and Windows take the application icon.
/// Runs after `assets::ensure_assets()` has written the PNG to the runtime asset dir.
pub fn app_icon() {
    let p = crate::assets::assets_dir().join("appicon").join("tinhat-256.png");
    if !p.exists() {
        return;
    }
    if let Ok(c) = std::ffi::CString::new(p.to_string_lossy().as_bytes()) {
        unsafe { tinhat_app_icon(c.as_ptr()) }
    }
}

/// Activate the KDE color scheme named `scheme` (e.g. "TinHatOLED"), recoloring the
/// whole Kirigami app incl. chrome. Must be called on the Qt GUI thread.
pub fn apply_color_scheme(scheme: &str) {
    if let Ok(c) = CString::new(scheme) {
        unsafe { tinhat_apply_color_scheme(c.as_ptr()) }
    }
}

/// themes (1b): activate the KDE color scheme `scheme` from its `.colors` file — any theme, also one whose file was
/// written after KColorSchemeManager scanned the folder (video_bridge.cpp). GUI thread only.
pub fn apply_color_scheme_file(scheme: &str, path: &std::path::Path) {
    if let (Ok(n), Ok(p)) = (CString::new(scheme), CString::new(path.to_string_lossy().as_bytes())) {
        unsafe { tinhat_apply_color_scheme_file(n.as_ptr(), p.as_ptr()) }
    }
}

/// The tile's zoom/pan/fill changed — re-run the C++ geometry for its video item.
pub fn relayout(object_name: &str) {
    if let Ok(c) = CString::new(object_name) {
        unsafe { tinhat_video_relayout(c.as_ptr()) }
    }
}

/// Force the OpenGL backend + register the qml6 video QML type. Call BEFORE the
/// `QGuiApplication` is created and before the QML engine loads.
pub fn preinit() {
    unsafe { tinhat_video_preinit() }
}

/// Pick the scene-graph RHI (OpenGL wherever qml6glsink / mpvqt need it). MUST run after
/// `QGuiApplication` and before the FIRST `QQuickWindow` — the startup window (0.8.24).
/// Needs no GStreamer, which is what lets that window show before `gst_init()`.
pub fn set_graphics_api() {
    unsafe { tinhat_video_set_graphics_api() }
}

/// Run the Qt event loop for up to `ms` — the startup window's repaint between blocking
/// startup steps, while the GUI thread is not yet in `app.exec()`.
pub fn pump_events(ms: i32) {
    unsafe { tinhat_pump_events(ms as c_int) }
}

/// Pin GstGL's platform/API to match Qt's scenegraph context, based on the live QPA
/// platform: EGL/GLES2 on Wayland (matches Qt's EGL context), unpinned on xcb
/// (X11/XWayland) so gstgl defaults to GLX/desktop-GL and matches the qml6 sink's
/// wrapped Qt GLX context — pinning EGL on xcb gives glupload a NONE-api context and
/// black tiles. MUST be called AFTER `QGuiApplication` is constructed and BEFORE any
/// video pipeline is built (gstgl reads these env vars lazily at GL-context creation).
pub fn pin_gstgl_for_qpa() {
    unsafe { tinhat_pin_gstgl_for_qpa() }
}

/// Register the upstream mpvqt `MpvAbstractItem` as the QML type `MpvVideo`
/// (module `dev.tinhat.mpv`) — the watch-party player surface. Must be called
/// BEFORE the QML engine loads the root component (so the import resolves).
pub fn register_mpv() {
    unsafe { tinhat_mpv_register() }
}

/// Attach the pipeline (a `GstElement*`/`GstPipeline*` as `usize`) to the QML
/// `GstGLQt6VideoItem` named `object_name`, and start it on the render thread.
/// Returns `true` if the item was found (else the delegate isn't realized yet —
/// retry). MUST be called on the Qt GUI thread (touches QQuickItem + scenegraph).
pub fn attach(pipeline_ptr: usize, object_name: &str) -> bool {
    let Ok(name) = CString::new(object_name) else {
        return false;
    };
    unsafe { tinhat_video_attach(pipeline_ptr as *mut c_void, name.as_ptr()) != 0 }
}

/// Unbind the qml6glsink `widget` for `pipeline_ptr` so the sink drops its pointer to
/// the `GstGLQt6VideoItem`. MUST run on the Qt GUI thread, BEFORE the pipeline is set to
/// NULL and BEFORE the QML delegate (its video item) is destroyed — otherwise the sink
/// can dereference a torn-down QQuickItem (intermittent crash on participant-leave).
/// 0 ptr = no-op.
pub fn predetach(pipeline_ptr: usize) {
    if pipeline_ptr == 0 {
        return;
    }
    unsafe { tinhat_video_predetach(pipeline_ptr as *mut c_void) }
}

/// Remove the `GstGLQt6VideoItem` surface from the tile named `object_name`, revealing
/// the avatar placeholder underneath. Used when the local camera turns off so the
/// persistent self-view tile reverts to the avatar instead of a frozen frame. MUST be
/// called on the Qt GUI thread, AFTER the tile's gst pipeline has been NULLed.
pub fn detach(object_name: &str) {
    let Ok(name) = CString::new(object_name) else {
        return;
    };
    unsafe { tinhat_video_detach(name.as_ptr()) }
}

/// #9 Pause-hidden: PAUSE (`true`) or resume (`false`) one tile pipeline by pointer.
/// Used to stop decoding remote video while the window is minimized/hidden. A 0 pointer
/// is a no-op. MUST be called on the Qt GUI thread.
pub fn set_paused(pipeline_ptr: usize, paused: bool) {
    if pipeline_ptr == 0 {
        return;
    }
    unsafe { tinhat_video_set_paused(pipeline_ptr as *mut c_void, c_int::from(paused)) }
}

/// Is the pipeline past NULL? Asked before an attach: a LIVE pipeline being re-pointed
/// (PiP pop-out / pop-back) must be REBUILT for the target window, never re-bound —
/// qml6glsink's winsys init is NULL->READY-only, so a re-bound live sink renders black.
pub fn is_live(pipeline_ptr: usize) -> bool {
    if pipeline_ptr == 0 {
        return false;
    }
    unsafe { tinhat_video_is_live(pipeline_ptr as *mut c_void) != 0 }
}
