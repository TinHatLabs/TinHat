// ui_scale_bridge.cpp — App size (2026-10-31, docs/UI-ZOOM.md; the rules: ui_scale.rs). GUI thread, plain extern "C"
// (no Q_OBJECT, no moc; the shape of theme_bridge.cpp).
//
// The size is Qt's own global high-DPI factor — what QT_SCALE_FACTOR sets at start — changed while the app runs, so
// every window, popup and menu renders at the new device pixel ratio: text, SVG icons and layers are drawn at the new
// resolution (crisp, not a stretched picture), layouts re-flow, windows keep their size on screen.
//
// Qt has no public call for that factor at run time. Three exported QtGui functions do the job, the ones a platform
// plugin uses when a screen's scale changes:
//   QHighDpiScaling::setGlobalFactor(qreal)                                     — the factor
//   QWindowSystemInterface::handleWindowDevicePixelRatioChanged<Synchronous>(w) — the window's cached DPR + its event
//   QWindowSystemInterface::handleGeometryChange<Synchronous>(w, nativeRect)    — Qt's bookkeeping of the window's
//                       logical geometry: the resize event AND the width/height signals QML binds to (a hand-made
//                       QResizeEvent re-laid the content out but left Window.width stale — review 2026-10-31)
// They are found BY NAME at run time (dlsym / GetProcAddress; the names checked in Qt 6.11's libQt6Gui.so.6 and in a
// shipped Windows Qt6Gui.dll), not linked, so a Qt without them can never stop Tin Hat from starting: the size then
// goes through QT_SCALE_FACTOR at the next start (tinhat_ui_scale_preinit) and a change says "when Tin Hat restarts".
// The native rectangle comes from the window's QPlatformWindow (qpa/qplatformwindow.h, a Qt private header; build.rs
// adds the version's private include dirs — without them this file builds the restart-only path).
// Proven on Qt 6.11 xcb + Wayland (a window, a popup, a second window: re-laid out, crisp, Window.width/height
// updated, the same size on screen).

#include <QCoreApplication>
#include <QGuiApplication>
#include <QList>
#include <QLoggingCategory>
#include <QPointer>
#include <QRect>
#include <QWindow>
#include <QtGlobal>

#if __has_include(<qpa/qplatformwindow.h>)
#include <qpa/qplatformwindow.h>
#define TINHAT_UI_SCALE_QPA 1
#else
#define TINHAT_UI_SCALE_QPA 0
#endif

#if defined(_WIN32)
#include <windows.h>
#else
#include <dlfcn.h>
#endif

namespace {

using SetGlobalFactor = void (*)(double);
using DprChanged = void (*)(QWindow *);
using GeometryChange = void (*)(QWindow *, const QRect &);

struct Api {
    SetGlobalFactor set = nullptr;
    DprChanged dpr = nullptr;
    GeometryChange geo = nullptr;
    bool ok() const { return set && dpr && geo; }
};

void *lookup(const char *itanium, const char *msvc) {
#if defined(_WIN32)
    (void)itanium;
    for (const wchar_t *dll : {L"Qt6Gui.dll", L"Qt6Guid.dll"})
        if (HMODULE m = GetModuleHandleW(dll))
            if (FARPROC f = GetProcAddress(m, msvc)) return reinterpret_cast<void *>(f);
    return nullptr;
#else
    (void)msvc;
    return dlsym(RTLD_DEFAULT, itanium);
#endif
}

const Api &api() {
    static const Api a = [] {
        Api r;
#if TINHAT_UI_SCALE_QPA
        r.set = reinterpret_cast<SetGlobalFactor>(lookup(
            "_ZN15QHighDpiScaling15setGlobalFactorEd",
            "?setGlobalFactor@QHighDpiScaling@@SAXN@Z"));
        r.dpr = reinterpret_cast<DprChanged>(lookup(
            "_ZN22QWindowSystemInterface35handleWindowDevicePixelRatioChangedINS_19SynchronousDeliveryEEEvP7QWindow",
            "??$handleWindowDevicePixelRatioChanged@USynchronousDelivery@QWindowSystemInterface@@@QWindowSystemInterface@@SAXPEAVQWindow@@@Z"));
        r.geo = reinterpret_cast<GeometryChange>(lookup(
            "_ZN22QWindowSystemInterface20handleGeometryChangeINS_19SynchronousDeliveryEEEvP7QWindowRK5QRect",
            "??$handleGeometryChange@USynchronousDelivery@QWindowSystemInterface@@@QWindowSystemInterface@@SAXPEAVQWindow@@AEBVQRect@@@Z"));
#endif
        return r;
    }();
    return a;
}

bool g_restartOnly = false;
double g_current = 0.0;    // the factor Tin Hat last set (0 = none yet: Qt has whatever it started with)
bool g_applying = false;   // inside apply(): a nested request is coalesced
double g_pending = 0.0;    // … into this, applied when the outer one is done

void applyNow(double factor) {
    QList<QPointer<QWindow>> wins;
    for (QWindow *w : QGuiApplication::allWindows()) wins.append(w);
    // Qt warns "Should only be called when no windows exist" on every call: true for a bare setGlobalFactor, and the
    // notifications below are exactly what makes it safe — so that one line is kept out of the log (the default
    // category's warnings off for this call only, GUI thread)
    QLoggingCategory *dc = QLoggingCategory::defaultCategory();
    const bool warnOn = dc && dc->isWarningEnabled();
    if (dc) dc->setEnabled(QtWarningMsg, false);
    api().set(factor);
    if (dc) dc->setEnabled(QtWarningMsg, warnOn);
    g_current = factor;
#if TINHAT_UI_SCALE_QPA
    // each step can run app code synchronously (a handler may close a window): re-check the window after every one
    for (const QPointer<QWindow> &w : wins) {
        if (!w || !w->handle()) continue;   // not created yet: it starts at the new factor
        api().dpr(w);
        if (!w || !w->handle()) continue;
        const QRect native = w->handle()->geometry();
        api().geo(w, native);
        if (w) w->requestUpdate();
    }
#endif
}

}  // namespace

extern "C" {

// Before QGuiApplication. `force_restart`: TINHAT_UI_SCALE_RESTART=1 (the size changes only at a start). When the live
// path is missing (or forced off) and the stored size is not 100 %, Qt gets it through QT_SCALE_FACTOR (qputenv: Qt's
// own CRT, so Windows' static-CRT exe can't lose it — see tinhat_force_quick_style). An environment that already sets
// QT_SCALE_FACTOR keeps it, multiplied.
void tinhat_ui_scale_preinit(double factor, int force_restart) {
    g_restartOnly = force_restart != 0;
    if ((api().ok() && !g_restartOnly) || qFuzzyCompare(factor, 1.0)) return;
    bool ok = false;
    const double env = qEnvironmentVariable("QT_SCALE_FACTOR").toDouble(&ok);
    const double f = (ok && env > 0.0) ? env * factor : factor;
    qputenv("QT_SCALE_FACTOR", QByteArray::number(f, 'g', 6));
}

// 1 = sizes change live here, 0 = only at the next start.
int tinhat_ui_scale_live_capable(void) { return (api().ok() && !g_restartOnly) ? 1 : 0; }

// Why (for the start-up log line, so a restart-only build never goes unnoticed): 0 live, 1 built without Qt's private
// headers, 2 this Qt lacks one of the three calls, 3 TINHAT_UI_SCALE_RESTART=1.
int tinhat_ui_scale_why(void) {
    if (!TINHAT_UI_SCALE_QPA) return 1;
    if (!api().ok()) return 2;
    if (g_restartOnly) return 3;
    return 0;
}

// Set Qt's factor to `factor` now (also right after QGuiApplication, before the first window). 1 = done live,
// 0 = not possible here: the stored size applies at the next start.
int tinhat_ui_scale_apply(double factor) {
    if (!tinhat_ui_scale_live_capable() || !(factor > 0.0)) return 0;
    if (g_applying) { g_pending = factor; return 1; }   // a handler asked again mid-change: done after this one
    if (g_current != 0.0 && qFuzzyCompare(factor, g_current)) return 1;
    g_applying = true;
    applyNow(factor);
    while (g_pending != 0.0) {   // the last nested request wins
        const double next = g_pending;
        g_pending = 0.0;
        if (!qFuzzyCompare(next, g_current)) applyNow(next);
    }
    g_applying = false;
    return 1;
}

}  // extern "C"
