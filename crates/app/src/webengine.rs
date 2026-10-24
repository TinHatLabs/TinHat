//! FFI to the C++ QtWebEngine bootstrap shim (`webengine_bridge.cpp`). The single
//! call `init()` runs `QtWebEngineQuick::initialize()`, which MUST happen before the
//! `QGuiApplication` is constructed (it sets `Qt::AA_ShareOpenGLContexts`). Only the
//! GUI path calls it — headless probes never touch WebEngine.

extern "C" {
    fn tinhat_webengine_init();
}

/// Initialize QtWebEngine. Call BEFORE `QGuiApplication::new()` and before the QML
/// engine loads. Cheap (no Chromium helper processes spawn until a `WebEngineView` is
/// actually created — lazily, on user click). Safe to call once on the GUI path.
pub fn init() {
    unsafe { tinhat_webengine_init() }
}
