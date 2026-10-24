// QtWebEngine bootstrap shim — the one bit of WebEngine setup that can't be done
// from QML or cxx-qt-lib.
//
//   * tinhat_webengine_init(): QtWebEngineQuick::initialize() MUST be called before the
//     QGuiApplication is constructed — it sets Qt::AA_ShareOpenGLContexts (required so
//     WebEngine's GPU thread can share the Qt scene-graph GL context) and registers the
//     QtWebEngine QML types. This is cheap: it does NOT spawn the Chromium GPU/render
//     helper processes — those start only when the first WebEngineView is instantiated
//     (which we keep lazy via a QML Loader, so users who never open an iframe embed pay
//     nothing). Called on the GUI path only (never on the headless probe paths).
//
// No Q_OBJECT here, so no moc step: plain extern "C".

#include <QtWebEngineQuick/QtWebEngineQuick>

extern "C" {

// Call BEFORE constructing QGuiApplication (next to tinhat_video_preinit). Idempotent.
void tinhat_webengine_init(void) {
    QtWebEngineQuick::initialize();
}

} // extern "C"
