// C++ shim for native remote-video tiles — the bits cxx-qt-lib can't express.
//
//   * tinhat_video_preinit(): force the OpenGL scene-graph backend (qml6glsink
//     renders into a GL texture) and dlopen+register the qml6 GStreamer plugin so
//     its QML type (org.freedesktop.gstreamer.Qt6GLVideoItem / GstGLQt6VideoItem)
//     exists BEFORE the engine loads Main.qml. Must run before QGuiApplication.
//
//   * tinhat_video_attach(): point a running gst pipeline's `qml6glsink` at the
//     QQuickItem (a GstGLQt6VideoItem) created by QML, found by objectName, then
//     set the pipeline PLAYING from the scenegraph render thread so it shares Qt's
//     GL context (otherwise the tile stays black). Proven recipe (qml6-smoke).
//
// No Q_OBJECT here, so no moc step: plain extern "C" + a QRunnable subclass.

#ifdef _WIN32
// windows.h (pulled via dwmapi.h / GL headers) defines min/max macros that break
// std::min below unless NOMINMAX lands BEFORE the first windows.h inclusion —
// which include-order changes (e.g. the MpvQt block) can silently reshuffle.
#define NOMINMAX
#endif
#include <memory>
#include <thread>
#ifndef _WIN32
// KF6 theming is Linux-only in the Windows port: KColorScheme is dropped
// (theming goes through a Kirigami.Theme-respecting QQC2 style instead — see
// windows/PLATFORM_AUDIT.md §10). The extern "C" symbols below stay defined on
// BOTH platforms so video.rs FFI is unchanged.
#include <KColorSchemeManager>
#include <KColorScheme>       // themes (1b): tinhat_apply_color_scheme_file — createApplicationPalette
#include <KColorSchemeModel>  // themes (1b): PathRole
#include <KSharedConfig>      // themes (1b): re-read a rewritten scheme file
#include <MpvQt/MpvAbstractItem>
#elif defined(TINHAT_HAVE_MPVQT)
// Windows watch-party (bare-metal port): MpvQt built from source with MSVC
// against the app's Qt, linked against an import lib generated from shinchiro's
// libmpv-2.dll (plain C ABI — cross-toolchain safe). build.rs defines
// TINHAT_HAVE_MPVQT only when the MpvQt install prefix is present, so builds
// without it fall back to the QML stub in tinhat_mpv_register.
#include <MpvQt/MpvAbstractItem>
#endif
#include <QAbstractItemModel>
#ifdef _WIN32
// Only the MpvVideo placeholder registration needs these (tinhat_mpv_register).
#include <QDir>
#include <QFile>
#include <QStandardPaths>
// QQuickStyle: force the QQC2 style before the engine loads (tinhat_force_quick_style).
#include <QQuickStyle>
// Breeze icon theme registration (tinhat_icon_theme): rcc icon bundles + QIcon theme.
#include <QIcon>
#include <QResource>
// Dark DWM title bars (tinhat_dark_titlebars): the default is white regardless of the
// app palette, which looks broken on a dark-themed app.
#include <QEvent>
#include <QWindow>
#include <dwmapi.h>
#endif
// Video tiles via QtMultimedia — BOTH platforms since 0.8.19: a pipeline whose tail is
// a plain appsink gets each I420 buffer copied into a QVideoFrame for a QML
// VideoOutput's QVideoSink (queued to the GUI thread). Windows always used this path
// (MEDIA_PIPELINE.md §6.3); Linux now uses it for the Memories side tiles, because the
// GstGL tail (glupload ! glcolorconvert ! qml6glsink) painted white / striped / nothing
// on a Wayland/GLES2/NVIDIA desktop while frames demonstrably flowed,
// and the harness (llvmpipe GLX) could never see it. Live call tiles on Linux keep
// qml6glsink. The sink element's factory decides at attach time.
#include <QPointer>
#include <QVideoFrame>
#include <QVideoFrameFormat>
#include <QVideoSink>
#include <algorithm>
#include <atomic>
#include <cmath>
#include <functional>
#include <gst/video/video.h>
#include <QCoreApplication>
#include <QEventLoop>
#include <QGuiApplication>
#include <QIcon>       // icon-theme fallback (tinhat_icon_theme) — both platforms
#include <QGuiApplication> // tinhat_app_icon (0.8.26)
#include <cstdio>

// 0.8.26 — Tin Hat icon pack: when iconpack.rs found a pack with a static theme it
// exports TINHAT_ICON_THEME_DIR (the theme search path) + TINHAT_ICON_THEME_NAME. Register it
// as the PRIMARY QIcon theme; whatever was primary before (breeze-dark on Windows, the
// desktop's theme or breeze on Linux) becomes the fallback, and the pack's index.theme
// Inherits breeze too — so any name the pack lacks still resolves exactly as today.
static void tinhat_apply_icon_pack(void) {
    const QString dir = qEnvironmentVariable("TINHAT_ICON_THEME_DIR");
    const QString name = qEnvironmentVariable("TINHAT_ICON_THEME_NAME");
    if (dir.isEmpty() || name.isEmpty())
        return;
    QStringList paths = QIcon::themeSearchPaths();
    paths.prepend(dir);
    QIcon::setThemeSearchPaths(paths);
    const QString prev = QIcon::themeName();
    QIcon::setThemeName(name);
    if (!prev.isEmpty() && prev != name)
        QIcon::setFallbackThemeName(prev);
    std::fprintf(stderr, "[theme] icon pack theme '%s' from %s (fallback '%s')\n",
                 name.toUtf8().constData(), dir.toUtf8().constData(),
                 QIcon::fallbackThemeName().toUtf8().constData());
}

// 0.8.26 — application icon (window decorations, taskbar, alt-tab) from a PNG on disk.
extern "C" void tinhat_app_icon(const char *path) {
    if (!path || !*path)
        return;
    const QIcon icon(QString::fromUtf8(path));
    if (icon.isNull())
        return;
    QGuiApplication::setWindowIcon(icon);
}
#include <QModelIndex>
#include <QQmlComponent>
#include <QQmlContext>
#include <QQmlEngine>
#include <QQuickItem>
#include <QOpenGLContext>
#include <QQuickWindow>
#include <QSurfaceFormat>
#include <QRunnable>
#include <QSGRendererInterface>
#include <QString>
#include <QUrl>
#include <QWindow>
#include <gst/gst.h>

namespace {
// Set a pipeline PLAYING from the render thread, where Qt's GL context is current.
// The qml6glsink's NULL->READY transition needs the GstGLQt6VideoItem to have been
// rendered at least once (else "Could not initialize window system" -> FAILURE). A
// freshly-created item hasn't rendered yet on the first render job, so on FAILURE we
// reschedule ourselves for a later frame until the item's GL context is ready.
//
// STAGE: AfterRendering, NOT BeforeSynchronizing. A BeforeSynchronizing job on the
// frame right after the item's creation runs BEFORE the item has ever been synced —
// the READY attempt then FAILS, and on gst 1.28.5 that failure POISONS the ITEM's
// cached GL wrap (gl_api=NONE): the item short-circuits the next initWinSys with the
// poisoned cache, the retry "succeeds", and glupload rejects every buffer forever
// ("GL API's not compatible: none" -> permanently black). The sink-NULL cleanup below
// no longer clears item-side state on this stack (observed live). At AfterRendering
// the item is guaranteed to have completed a sync+render in the SAME frame, so the
// first attempt succeeds and the poison path is never entered. (In the main grid the
// placeholder usually rendered long before attach, which is why in-grid tiles mostly
// worked; a fresh pop-out window's item had NEVER rendered — its first attempt always
// failed, always poisoned, hence the guaranteed black pop-out box.)
class SetPlaying : public QRunnable {
public:
    SetPlaying(GstElement *p, QQuickWindow *w, int attempt)
        : pipeline_(p ? GST_ELEMENT(gst_object_ref(p)) : nullptr), window_(w), attempt_(attempt) {}
    ~SetPlaying() override {
        if (pipeline_)
            gst_object_unref(pipeline_);
    }
    void run() override {
        if (!pipeline_)
            return;
        // Retired (tinhat_video_predetach ran)? Abandon the chain — do NOT reschedule.
        // A retry job outliving teardown would set the retired pipeline PLAYING again,
        // racing retire()'s detached-thread NULL (zombie chains observed rescheduling
        // long after their tile was torn down).
        if (g_object_get_data(G_OBJECT(pipeline_), "tinhat-retired")) {
            g_printerr("[video] SetPlaying: pipeline retired — abandoning (attempt %d)\n",
                       attempt_);
            return;
        }
        // One-shot share-group diagnostic (we ARE on this window's render thread, its GL
        // context current at AfterRenderingStage): whether THIS window's scenegraph
        // context shares with the global share context. On drivers where Qt silently
        // dropped sharing (NVIDIA — see tinhat_pin_gstgl_for_qpa), sharing=0 here is the
        // smoking gun for cross-window texture adoption sampling black. Grid tiles live
        // in the main window so they render either way; this line makes a diagnostics
        // report decisive.
        if (attempt_ == 0) {
            QOpenGLContext *cur = QOpenGLContext::currentContext();
            QOpenGLContext *g = QOpenGLContext::globalShareContext();
            g_printerr("[video] SetPlaying: render ctx=%p valid=%d globalShare=%p sharing=%d\n",
                       (void *)cur, cur ? (int)cur->isValid() : -1, (void *)g,
                       (cur && g) ? (int)QOpenGLContext::areSharing(cur, g) : -1);
        }
        // qml6glsink docs: the sink must reach READY and propagate its Qt GstGLDisplay
        // BEFORE any other GL element creates its context. Concretely: bring ONLY the
        // sink to READY first (this calls the video item's initWinSys + wraps the Qt
        // QOpenGLContext). It FAILS ("could not initialize window system") until the
        // item has been rendered once, and a racy/failed init yields a context with
        // gl_api=NONE -> glupload later rejects every buffer ("GL API's not compatible:
        // none"). So retry the SINK-only READY (glupload untouched) until it succeeds
        // cleanly, THEN start the whole pipeline so glupload finds the sink's valid
        // context.
        GstElement *sink = gst_bin_get_by_name(GST_BIN(pipeline_), "sink");
        if (!sink) {
            g_printerr("[video] SetPlaying: no 'sink' element\n");
            return;
        }
        GstStateChangeReturn sr = gst_element_set_state(sink, GST_STATE_READY);
        if (sr == GST_STATE_CHANGE_FAILURE) {
            gst_element_set_state(sink, GST_STATE_NULL); // clear the half-baked context
            gst_object_unref(sink);
            if (window_ && attempt_ < 60) {
                window_->scheduleRenderJob(new SetPlaying(pipeline_, window_, attempt_ + 1),
                                           QQuickWindow::AfterRenderingStage);
                // A render job only runs when the window renders a FRAME. The main call
                // grid animates constantly, but a static window (the PiP pop-out shows
                // nothing until the video starts) schedules no frames on its own — the
                // rescheduled job would wait forever. update() is documented safe from
                // the render thread. Only every 4th failed attempt (0, 4, 8, ...):
                // hammering a forced repaint per retry was measurable repaint pressure,
                // and attach() already forces the frame that runs attempt 0.
                if (attempt_ % 4 == 0)
                    window_->update();
            } else {
                g_printerr("[video] SetPlaying: sink never reached READY after %d tries\n", attempt_);
            }
            return;
        }
        gst_object_unref(sink);
        // Re-check retirement IMMEDIATELY before going PLAYING: predetach can land on
        // the GUI thread between the entry check and here (sink READY takes real time),
        // and PLAYING a retired pipeline races retire()'s NULL.
        if (g_object_get_data(G_OBJECT(pipeline_), "tinhat-retired")) {
            g_printerr("[video] SetPlaying: pipeline retired after sink READY — abandoning\n");
            return;
        }
        // Sink is READY with a valid Qt GL context — now bring the whole pipeline up.
        GstStateChangeReturn r = gst_element_set_state(pipeline_, GST_STATE_PLAYING);
        g_printerr("[video] SetPlaying: sink READY (attempt %d), pipeline PLAYING -> %s\n",
                   attempt_, gst_element_state_change_return_get_name(r));
        if (r == GST_STATE_CHANGE_FAILURE && window_ && attempt_ < 60) {
            gst_element_set_state(pipeline_, GST_STATE_NULL);
            window_->scheduleRenderJob(new SetPlaying(pipeline_, window_, attempt_ + 1),
                                       QQuickWindow::AfterRenderingStage);
            if (attempt_ % 4 == 0)
                window_->update(); // see above — every 4th attempt, less repaint pressure
        }
    }

private:
    GstElement *pipeline_;
    QQuickWindow *window_;
    int attempt_;
};

// Find a QQuickItem by objectName by walking the VISUAL tree (childItems), NOT the
// QObject tree. Repeater/ListView delegates are visual children of the scene but are
// NOT QObject-children reachable from the window via findChild/findChildren — which is
// exactly why earlier findChild lookups always returned "not found" even though the
// tile was on screen.
QQuickItem *findVisualItem(QQuickItem *root, const QString &name) {
    if (!root)
        return nullptr;
    if (root->objectName() == name)
        return root;
    const auto kids = root->childItems();
    for (QQuickItem *k : kids) {
        if (QQuickItem *found = findVisualItem(k, name))
            return found;
    }
    return nullptr;
}

// Per-attach bridge context: the pad probe (GStreamer streaming thread) copies each
// I420 buffer into a QVideoFrame and queues setVideoFrame onto the GUI thread. The
// QPointer is only dereferenced ON the GUI thread (safe against tile teardown); `dead`
// is the fast-path stop flag predetach flips BEFORE the tile QML is destroyed.
struct QvsCtx {
    QPointer<QVideoSink> sink;
    std::atomic<bool> dead{false};
};

// Buffer probe on the appsink's sink pad — core-gst only (no gstapp dependency); the
// appsink itself just discards (max-buffers=2 drop=true, nobody pulls).
GstPadProbeReturn qvsBufferProbe(GstPad *pad, GstPadProbeInfo *info, gpointer user_data) {
    QvsCtx *ctx = static_cast<QvsCtx *>(user_data);
    if (!ctx || ctx->dead.load(std::memory_order_relaxed))
        return GST_PAD_PROBE_OK;
    GstBuffer *buf = GST_PAD_PROBE_INFO_BUFFER(info);
    if (!buf)
        return GST_PAD_PROBE_OK;
    GstCaps *caps = gst_pad_get_current_caps(pad);
    if (!caps)
        return GST_PAD_PROBE_OK;
    GstVideoInfo vinfo;
    gboolean ok = gst_video_info_from_caps(&vinfo, caps);
    gst_caps_unref(caps);
    if (!ok || GST_VIDEO_INFO_FORMAT(&vinfo) != GST_VIDEO_FORMAT_I420)
        return GST_PAD_PROBE_OK;
    GstVideoFrame vframe;
    if (!gst_video_frame_map(&vframe, &vinfo, buf, GST_MAP_READ))
        return GST_PAD_PROBE_OK;

    const int w = GST_VIDEO_INFO_WIDTH(&vinfo);
    const int h = GST_VIDEO_INFO_HEIGHT(&vinfo);
    QVideoFrameFormat fmt(QSize(w, h), QVideoFrameFormat::Format_YUV420P);
    QVideoFrame vf(fmt);
    if (vf.map(QVideoFrame::WriteOnly)) {
        for (int plane = 0; plane < 3; ++plane) {
            const guint8 *src = static_cast<const guint8 *>(GST_VIDEO_FRAME_PLANE_DATA(&vframe, plane));
            const int srcStride = GST_VIDEO_FRAME_PLANE_STRIDE(&vframe, plane);
            uchar *dst = vf.bits(plane);
            const int dstStride = vf.bytesPerLine(plane);
            const int rows = (plane == 0) ? h : (h + 1) / 2;
            const int rowBytes = (std::min)(srcStride, dstStride); // parens defeat any min macro
            for (int r = 0; r < rows; ++r)
                memcpy(dst + r * dstStride, src + r * srcStride, rowBytes);
        }
        vf.unmap();
        // QVideoFrame is refcounted — capture by value; deref the QPointer on the GUI
        // thread only.
        QPointer<QVideoSink> sp = ctx->sink;
        QMetaObject::invokeMethod(
            qGuiApp, [sp, vf]() { if (sp) sp->setVideoFrame(vf); }, Qt::QueuedConnection);
    }
    gst_video_frame_unmap(&vframe);
    return GST_PAD_PROBE_OK;
}

// Which display path a pipeline wants: an `appsink` tail = the QtMultimedia
// (QVideoSink) path; anything else (qml6glsink) = the GstGL path.
bool sinkIsAppsink(GstElement *sink) {
    if (!sink)
        return false;
    GstElementFactory *f = gst_element_get_factory(sink);
    if (!f)
        return false;
    const gchar *n = gst_plugin_feature_get_name(GST_PLUGIN_FEATURE(f));
    return n && g_strcmp0(n, "appsink") == 0;
}
} // namespace

extern "C" {
void tinhat_video_relayout(const char *object_name);

// Activate a KDE color scheme by name (e.g. "TinHatOLED"), recoloring the WHOLE
// Kirigami app — including the global header/toolbars that ignore per-element
// Kirigami.Theme overrides — via qqc2-desktop-style. Must run on the GUI thread
// after the QGuiApplication exists. Schemes are .colors files we wrote into
// ~/.local/share/color-schemes/ (see colorscheme.rs).
#ifdef _WIN32
// Force the QQC2 style to `Basic` BEFORE the QML engine's first Controls import
// (TEST_PLAN.md §2.5): the native "windows" style paints chrome from the platform
// palette, ignores Kirigami.Theme/palette overrides, and warns on every customized
// control. Basic is the palette-driven, no-extra-dependency style. Done via the C++
// API rather than QT_QUICK_CONTROLS_STYLE: the Rust exe is static-CRT while Qt's DLLs
// use their own CRT, and an env var set after process start on the Rust side is not
// reliably visible to Qt (verified live — shell-set worked, in-app set_var did not).
// An explicit developer env override still wins.
extern "C" void tinhat_force_quick_style(void) {
    if (qEnvironmentVariableIsSet("QT_QUICK_CONTROLS_STYLE"))
        return;
    QQuickStyle::setStyle(QStringLiteral("Basic"));
    g_printerr("[theme] QQC2 style forced to Basic (Windows default ignores palettes)\n");
}

// Breeze icon theme: Linux gets it from the system; Windows has NO system icon
// theme, so every Kirigami/QQC2 `icon.name` lookup came back empty and buttons
// rendered as blank circles. Register the breeze-icons binary resources (built
// from invent.kde.org/frameworks/breeze-icons v6.24, aliases resolved by its
// qrcAlias tool) and point QIcon at them. The rcc has NO internal prefix — the
// registerResource root supplies the `/icons/<theme-dir>` layout QIcon expects
// (the same trick KIconThemes uses). breeze-dark primary (the app is dark),
// breeze as the inheritance/fallback link. Bundle dir first, dev prefix second.
extern "C" void tinhat_icon_theme(void) {
    const QString appDir = QCoreApplication::applicationDirPath() + QStringLiteral("/icons");
    const QString devDir = QStringLiteral("C:/kde/build/breeze-icons/icons");
    const QString base = QFile::exists(appDir + QStringLiteral("/breeze-icons.rcc")) ? appDir : devDir;
    bool any = false;
    any |= QResource::registerResource(base + QStringLiteral("/breeze-icons.rcc"),
                                       QStringLiteral("/icons/breeze"));
    any |= QResource::registerResource(base + QStringLiteral("/breeze-icons-dark.rcc"),
                                       QStringLiteral("/icons/breeze-dark"));
    if (!any) {
        g_printerr("[theme] breeze-icons.rcc not found (%s) — icons will be blank\n",
                   base.toUtf8().constData());
        return;
    }
    QIcon::setThemeSearchPaths(QStringList() << QStringLiteral(":/icons"));
    QIcon::setThemeName(QStringLiteral("breeze-dark"));
    QIcon::setFallbackThemeName(QStringLiteral("breeze"));
    tinhat_apply_icon_pack();
    g_printerr("[theme] breeze icon theme registered from %s\n", base.toUtf8().constData());
}

// Dark title bars: DWM paints the frame independent of the app palette, so a dark app
// gets a glaring white bar. DWMWA_USE_IMMERSIVE_DARK_MODE (=20, Win10 1903+/Win11)
// flips it per WINDOW. Apply to every existing top-level window and — because PiP
// pop-outs / overlay / dialogs are created lazily — to every window that later gains
// focus (first focus fires at show). Call AFTER engine.load (main window exists).
namespace {
// Apply-dark on EVERY window as it shows (and again if Qt recreates its native
// handle — WinIdChange). The original one-shot sweep + focus hook missed the
// main window in the packaged build (its HWND is minted after the sweep and the
// focus hook only fires on a focus TRANSITION, which a fresh launch's
// already-focused window never makes) — seen as a glaring white bar on the
// installed bundle while dev runs looked fine.
class DarkBarFilter : public QObject {
public:
    static void applyTo(QWindow *w) {
        if (!w)
            return;
        HWND h = reinterpret_cast<HWND>(w->winId());
        BOOL dark = TRUE;
        DwmSetWindowAttribute(h, 20 /* DWMWA_USE_IMMERSIVE_DARK_MODE */, &dark, sizeof(dark));
    }
    bool eventFilter(QObject *obj, QEvent *ev) override {
        if (ev->type() == QEvent::Show || ev->type() == QEvent::WinIdChange || ev->type() == QEvent::Expose) {
            if (auto *w = qobject_cast<QWindow *>(obj))
                applyTo(w);
        }
        return QObject::eventFilter(obj, ev);
    }
};
} // namespace

extern "C" void tinhat_dark_titlebars(void) {
    auto apply = [](QWindow *w) { DarkBarFilter::applyTo(w); };
    const auto windows = QGuiApplication::topLevelWindows();
    for (QWindow *w : windows)
        apply(w);
    QObject::connect(qGuiApp, &QGuiApplication::focusWindowChanged, qGuiApp,
                     [apply](QWindow *w) { apply(w); });
    // The catch-all: every QWindow re-stamps itself on show/handle-recreate.
    static DarkBarFilter *filter = nullptr;
    if (!filter) {
        filter = new DarkBarFilter();
        qGuiApp->installEventFilter(filter);
    }
    g_printerr("[theme] dark DWM title bars applied (%d window(s) + future ones)\n",
               (int)windows.size());
}
#else  // !_WIN32

// Linux: icons come from the SYSTEM theme, so nothing was ever registered here.
// That is fine on KDE (the platform theme is already breeze) but silently breaks
// everywhere else: with a theme that lacks our breeze-specific names — GNOME's
// Adwaita, or a bare X server with no platform theme at all — Qt gives an icon-only
// ToolButton a zero-size icon, so the control collapses to padding and the button
// effectively disappears. That is what hid the room-list footer's bug-report /
// collapse-nav / settings buttons (found 2026-08-10 driving the GUI headlessly).
// breeze-icons is a hard dependency of our package, so point the FALLBACK theme at
// it: on KDE nothing changes, everyone else gets their icons back.
extern "C" void tinhat_icon_theme(void) {
    // NOTE: with no platform theme Qt reports "hicolor" — NOT an empty string — and
    // hicolor carries almost none of the names we use, so an isEmpty() guard never
    // fires where it is actually needed. Treat hicolor as "no real theme".
    const QString cur = QIcon::themeName();
    if (cur.isEmpty() || cur == QLatin1String("hicolor"))
        QIcon::setThemeName(QStringLiteral("breeze"));
    // Always fall back to breeze: it only applies to names the primary theme lacks,
    // so a user on Papirus/Adwaita keeps their look and just stops losing our buttons.
    QIcon::setFallbackThemeName(QStringLiteral("breeze"));
    tinhat_apply_icon_pack();
    g_printerr("[theme] icon theme='%s' fallback='%s'\n",
               QIcon::themeName().toUtf8().constData(),
               QIcon::fallbackThemeName().toUtf8().constData());
}
#endif

void tinhat_apply_color_scheme(const char *name) {
    if (!name)
        return;
#ifdef _WIN32
    // KColorScheme is dropped on Windows (PLATFORM_AUDIT.md §10): theming will come from
    // a Kirigami.Theme-respecting QQC2 style driven by the QML palette. Symbol kept
    // stable so video.rs::apply_color_scheme needs no cfg. QPalette stub lands with the
    // theming pass; until then this is an explicit, logged no-op.
    g_printerr("[theme] scheme '%s' requested — KColorScheme not used on Windows (QML palette drives colors)\n",
               name);
#else
    KColorSchemeManager *mgr = KColorSchemeManager::instance();
    if (!mgr) {
        g_printerr("[theme] no KColorSchemeManager instance\n");
        return;
    }
    QModelIndex idx = mgr->indexForScheme(QString::fromUtf8(name));
    if (idx.isValid()) {
        mgr->activateScheme(idx);
        g_printerr("[theme] activated KDE color scheme '%s'\n", name);
    } else {
        g_printerr("[theme] color scheme '%s' not found in the model\n", name);
    }
#endif
}

// themes (step 1b): activate the scheme FILE of any theme — also one written after KColorSchemeManager scanned the
// color-schemes folder (it scans once, at its first instance(), so a theme applied later was "not found in the
// model"). A scheme the model knows at this very path is activated through the manager like the built-ins; any
// other is applied the way the manager applies a scheme (kcolorschememanager.cpp activateSchemeInternal): the
// KDE_COLOR_SCHEME_PATH hint first (the platform theme and KColorScheme's default config read it on the palette
// change), then the application palette from the file. The file is re-read first: KSharedConfig caches an open file
// by name, and a theme's scheme may have been rewritten since it was last read.
void tinhat_apply_color_scheme_file(const char *name, const char *path) {
    if (!name || !path)
        return;
#ifdef _WIN32
    g_printerr("[theme] scheme '%s' requested — KColorScheme not used on Windows (QML palette drives colors)\n",
               name);
#else
    const QString file = QString::fromUtf8(path);
    KSharedConfigPtr cfg = KSharedConfig::openConfig(file);
    cfg->reparseConfiguration();
    KColorSchemeManager *mgr = KColorSchemeManager::instance();
    const QModelIndex idx = mgr ? mgr->indexForScheme(QString::fromUtf8(name)) : QModelIndex();
    if (idx.isValid() && idx.data(KColorSchemeModel::PathRole).toString() == file) {
        mgr->activateScheme(idx);
        g_printerr("[theme] activated KDE color scheme '%s'\n", name);
        return;
    }
    qApp->setProperty("KDE_COLOR_SCHEME_PATH", file);
    qApp->setPalette(KColorScheme::createApplicationPalette(cfg));
    g_printerr("[theme] applied KDE color scheme '%s' from its file\n", name);
#endif
}

// Pick the scene-graph RHI. Call AFTER QGuiApplication and BEFORE the first QQuickWindow —
// the startup window (0.8.24). Needs no GStreamer (that is what lets the startup window
// show before gst_init(); the qml6glsink probe lives in tinhat_video_preinit below).
void tinhat_video_set_graphics_api(void) {
    // Safe mode (0.8.25, boot.rs): the software scene graph on every platform — the
    // whole point is to come up on a GPU/driver whose GL or D3D context creation is what
    // killed the previous start. QVideoSink tiles render fine under it (CPU frames).
    if (qEnvironmentVariableIsSet("TINHAT_SAFE_MODE_ACTIVE")) {
        QQuickWindow::setGraphicsApi(QSGRendererInterface::Software);
        g_printerr("[video] SAFE MODE: software scene graph (no GPU context)\n");
        return;
    }
#ifdef _WIN32
#ifdef TINHAT_HAVE_MPVQT
    // Watch-party: mpvqt's MpvAbstractItem renders ONLY under the OpenGL scene
    // graph (it hard-checks QQuickWindow::graphicsApi()==OpenGL), and Qt Quick
    // has ONE RHI per process — so with MpvQt compiled in, Windows moves off the
    // default D3D11 RHI onto OpenGL, same as Linux. The appsink->QVideoSink tile
    // path is RHI-agnostic (QVideoFrame upload works under any backend) — verified
    // by TINHAT_QVS_TEST under this configuration. TINHAT_FORCE_D3D11=1 is the
    // diagnostic escape hatch: keeps D3D11 (watch-party video will be black).
    if (qEnvironmentVariableIsSet("TINHAT_FORCE_D3D11")) {
        g_printerr("[video] preinit: Windows — TINHAT_FORCE_D3D11 set, keeping D3D11 RHI (watch-party disabled)\n");
    } else {
        QQuickWindow::setGraphicsApi(QSGRendererInterface::OpenGL);
        g_printerr("[video] preinit: Windows — OpenGL RHI forced (mpvqt watch-party); QVideoSink tiles are RHI-agnostic\n");
    }
#else
    // Windows primary video path is appsink -> QVideoSink -> QML VideoOutput
    // (PLATFORM_AUDIT.md §4), which wants Qt6's DEFAULT D3D11 RHI — so do NOT force
    // OpenGL here. qml6glsink also ships in no upstream Windows GStreamer binary, so
    // the probe would be pointless. Deliberate no-op.
    g_printerr("[video] preinit: Windows — default RHI kept (QVideoSink path), no qml6glsink probe\n");
#endif
#else
    QQuickWindow::setGraphicsApi(QSGRendererInterface::OpenGL);
#endif
}

// Requires gst_init() already done; call BEFORE the engine loads Main.qml (after the
// startup window is up — the registry scan behind gst_init() is what it narrates).
// Linux: probe-create qml6glsink so its QML type (GstGLQt6VideoItem) registers.
void tinhat_video_preinit(void) {
#ifndef _WIN32
    if (GstElement *probe = gst_element_factory_make("qml6glsink", nullptr))
        gst_object_unref(probe);
#endif
}

// Startup window (0.8.24): deliver the queued stage update + expose/render events while
// the GUI thread is between blocking startup steps and not yet in app.exec().
void tinhat_pump_events(int ms) {
    QCoreApplication::sendPostedEvents();
    QCoreApplication::processEvents(QEventLoop::AllEvents, ms);
}

// Pin GstGL to EGL/GLES2 ONLY when Qt's QPA platform is Wayland. Background: the qml6
// video sink WRAPS Qt's own scenegraph GL context for GStreamer. On the wayland QPA
// platform that context is EGL/OpenGL-ES, so gstgl must match (EGL/GLES2) — this is the
// NVIDIA/Wayland case the pin was originally added for. But on the xcb platform
// (X11/XWayland — e.g. the Intel iGPU session) Qt's context is GLX/desktop-GL, and the
// qml6 sink wraps it as GLX; pinning gstgl to EGL/GLES2 there leaves glupload unable to
// find an API-compatible context (gl_api=NONE -> "GL API's not compatible: none" ->
// not-negotiated -> BLACK tiles). So on xcb we must NOT pin — let gstgl default to
// GLX/desktop-GL so it matches the sink's wrapped Qt context. Call AFTER QGuiApplication
// exists (so platformName() is valid) and BEFORE any video pipeline is built (gstgl reads
// these vars lazily when it creates a GL context, i.e. at pipeline PLAYING during a call).
// Respect any value the user set explicitly. qputenv is Qt's process-env setter.
void tinhat_pin_gstgl_for_qpa(void) {
    const QString plat = QGuiApplication::platformName();
#ifdef _WIN32
    // Only relevant if the GL (qml6glsink) route is ever taken on Windows; the primary
    // QVideoSink path never creates a GstGL context. Leave unpinned, log for decisiveness.
    g_printerr("[video] QPA platform '%s' -> GstGL left default (Windows QVideoSink path)\n",
               qPrintable(plat));
    return;
#endif
    const bool wayland = plat.startsWith(QStringLiteral("wayland"));
    if (wayland) {
        if (!qEnvironmentVariableIsSet("GST_GL_PLATFORM"))
            qputenv("GST_GL_PLATFORM", "egl");
        if (!qEnvironmentVariableIsSet("GST_GL_API")) {
            // Pin the API to what Qt's scenegraph context ACTUALLY is — never a guess.
            // The old hardcoded "gles2" matched the NVIDIA/Wayland context this pin was
            // written on, but Qt's context type varies by GPU/driver: on an Intel ARL
            // iGPU (Mesa) Qt creates desktop OpenGL 4.6 COMPATIBILITY over EGL, which
            // gstgl classifies as the legacy "opengl" API — the gles2 pin then wraps it
            // as gl_api=NONE and glupload rejects every buffer ("GL API's not
            // compatible: none" -> every remote tile black on that hardware; observed
            // live, GST_DEBUG-verified). Mapping: GLES -> gles2; desktop CORE profile ->
            // opengl3; desktop COMPATIBILITY/no profile -> opengl.
            // Qt 6.11: globalShareContext() is created LAZILY on first call and a failed
            // create() is SWALLOWED (qopenglcontext.cpp installs it unchecked) — so never
            // trust its format() blindly: an invalid context reports the REQUESTED format
            // (DefaultRenderableType / 2.0), and a pin derived from that is garbage. A
            // resolved EGL format can never be DefaultRenderableType (Qt resolves
            // renderableType at construction and version/profile during create()).
            QOpenGLContext *share = QOpenGLContext::globalShareContext();
            QSurfaceFormat f;
            bool resolved = false;
            if (share && share->isValid()) {
                f = share->format(); // created context -> format() is the RESOLVED one
                resolved = true;
            } else {
                QOpenGLContext probe; // no/failed share context: create one to resolve
                if (share)
                    probe.setShareContext(share);
                if (probe.create()) {
                    f = probe.format();
                    resolved = true;
                    // Qt silently drops EGL sharing when share-creation fails
                    // (qeglplatformcontext.cpp retries without share; adopt() then nulls
                    // shareContext). On such a driver qml6glsink textures created against
                    // one window are foreign to another -> silent black. Log it loudly.
                    if (share && !probe.shareContext())
                        g_printerr("[video] WARNING: driver dropped GL share-group "
                                   "membership (probe created UNSHARED) — cross-window "
                                   "GL texture sharing is broken on this stack\n");
                } else {
                    f = QSurfaceFormat::defaultFormat();
                    g_printerr("[video] WARNING: GL probe context create() FAILED — "
                               "format below is the UNRESOLVED default, the GST_GL_API "
                               "pin derived from it is a guess\n");
                }
            }
            const char *api;
            if (f.renderableType() == QSurfaceFormat::OpenGLES)
                api = "gles2";
            else if (f.profile() == QSurfaceFormat::CoreProfile)
                api = "opengl3";
            else
                api = "opengl";
            qputenv("GST_GL_API", api);
            g_printerr("[video] Qt context: renderable=%d profile=%d %d.%d resolved=%d -> "
                       "GST_GL_API=%s\n",
                       (int)f.renderableType(), (int)f.profile(), f.majorVersion(),
                       f.minorVersion(), (int)resolved, api);
        }
        g_printerr("[video] QPA platform '%s' -> pinned GstGL to EGL + Qt-matched API\n",
                   qPrintable(plat));
    } else {
        // xcb / other: leave GstGL unpinned so it defaults to GLX/desktop-GL, matching
        // the sink's wrapped Qt GLX context (avoids the gl_api=NONE black-tile bug).
        g_printerr("[video] QPA platform '%s' -> GstGL left default (GLX/desktop-GL)\n",
                   qPrintable(plat));
    }
    // Echo the resolved state so a future black-tile report is decisive: '' on the xcb
    // branch proves we left gstgl unset; 'egl'/'gles2' proves the Wayland pin landed.
    g_printerr("[video] gstgl resolved: QPA='%s' GST_GL_PLATFORM='%s' GST_GL_API='%s'\n",
               qPrintable(plat), qgetenv("GST_GL_PLATFORM").constData(),
               qgetenv("GST_GL_API").constData());
}

// Register the UPSTREAM mpvqt video item directly as a QML type — the watch-party
// player surface (WatchPartyWindow.qml `import dev.tinhat.mpv`). MpvAbstractItem is a
// QQuickFramebufferObject that's ALREADY a Q_OBJECT moc'd inside libMpvQt; registering it
// here instantiates the meta machinery from the upstream lib, so THIS file still needs no
// moc step. mpvqt 1.2.0 ships no QML plugin of its own, hence this manual registration.
// Must run BEFORE the engine loads Main.qml (call next to tinhat_video_preinit). The
// surface drives play/pause/seek/loadfile via the public command()/setProperty()
// Q_INVOKABLEs and polls getProperty() for time-pos/duration/eof (no protected
// MpvController signal needed → moc-free).
void tinhat_mpv_register(void) {
#if defined(_WIN32) && defined(TINHAT_HAVE_MPVQT)
    // Bare-metal port: MpvQt IS built for MSVC on this machine (source build
    // against the app's Qt + an import lib over shinchiro's libmpv-2.dll), so
    // register the real item exactly like Linux. Requires the OpenGL RHI —
    // tinhat_video_preinit's TINHAT_HAVE_MPVQT arm forces it.
    qmlRegisterType<MpvAbstractItem>("dev.tinhat.mpv", 1, 0, "MpvVideo");
    g_printerr("[mpv] real MpvQt MpvVideo registered (Windows MSVC build)\n");
#elif defined(_WIN32)
    // mpv/mpvqt have no MSVC build (BUILD_WINDOWS.md §0/§5.4) — the watch-party mpv
    // surface is deferred out of the first Windows build. But Main.qml instantiates
    // WatchPartyWindow, whose `import dev.tinhat.mpv` is a hard load-time dependency —
    // an unregistered module fails the WHOLE Main.qml engine load ("module not
    // installed"). A bare QQuickItem placeholder is not enough either: the QML binds
    // `onReady:` declaratively, and assigning a handler to a signal the type doesn't
    // have ALSO fails the load ("Cannot assign to non-existent property"). A C++ type
    // with a signal would need moc (this file is deliberately moc-free), so register a
    // QML-FILE stub instead — a QML component can declare the signal + the three
    // functions WatchPartyWindow calls (command/setProperty/getProperty) as plain JS
    // no-ops. Written to temp at startup; never emits ready() (nothing to init).
    // The watch-party video area is simply black until the surface is ported.
    // Symbol kept so video.rs/main.rs need no cfg.
    static const char kMpvStubQml[] =
        "import QtQuick\n"
        "// Windows placeholder for mpvqt's MpvAbstractItem (watch-party deferred).\n"
        "Item {\n"
        "    signal ready()\n"
        "    function command(args) {}\n"
        "    function setProperty(name, value) {}\n"
        "    function getProperty(name) { return undefined }\n"
        "}\n";
    const QString stubDir = QStandardPaths::writableLocation(QStandardPaths::TempLocation)
        + QStringLiteral("/tinhat-mpv-stub");
    QDir().mkpath(stubDir);
    const QString stubPath = stubDir + QStringLiteral("/MpvVideoStub.qml");
    QFile f(stubPath);
    if (f.open(QIODevice::WriteOnly | QIODevice::Truncate)) {
        f.write(kMpvStubQml);
        f.close();
        qmlRegisterType(QUrl::fromLocalFile(stubPath), "dev.tinhat.mpv", 1, 0, "MpvVideo");
        g_printerr("[mpv] watch-party mpv disabled on Windows — QML placeholder MpvVideo registered (%s)\n",
                   qPrintable(stubPath));
    } else {
        // Fall back to the bare item: app still loads unless the QML grows more
        // declarative mpv bindings than onReady (then the load error names them).
        qmlRegisterType<QQuickItem>("dev.tinhat.mpv", 1, 0, "MpvVideo");
        g_printerr("[mpv] MpvVideo stub write FAILED (%s) — registered bare QQuickItem\n",
                   qPrintable(stubPath));
    }
#else
    qmlRegisterType<MpvAbstractItem>("dev.tinhat.mpv", 1, 0, "MpvVideo");
#endif
}

// Find the QML container Item named `object_name` (the tile delegate), CREATE a
// `GstGLQt6VideoItem` inside it (the qml6glsink video surface — created here in C++
// via QQmlComponent because the runtime-registered QML type is invisible to QML's
// qmlcachegen at build time, and QML `Qt.createQmlObject` failed silently), point the
// pipeline's `sink` (qml6glsink) at that item, and schedule PLAYING on the render
// thread (GL-context sharing). `pipeline` = the gst::Pipeline (GstElement*). Returns
// 1 once attached (or on a deterministic create failure, to stop the retry loop), 0
// if the container isn't realized yet (caller retries).
// ── Tile view (0.8.22): fit / fill / zoom / pan ──────────────────────────
// The QML tile container declares thZoom (1..6), thPanX/thPanY (-1..1 of the
// overflow) and thFill (cover vs fit) as the user's intent, plus thVidW/thVidH —
// the stream's natural size, written HERE from the sink caps (now and on every caps
// change). The video item is sized to the scaled video, centred, then panned; the
// container clips. Unknown size = fill the container and let the item letterbox
// itself (the pre-0.8.22 look), so nothing regresses before the first caps arrive.
static void applyVideoGeometry(QQuickItem *video, QQuickItem *container) {
    if (!video || !container)
        return;
    const double cw = container->width(), ch = container->height();
    if (cw <= 0 || ch <= 0)
        return;
    const double vw = container->property("thVidW").toDouble();
    const double vh = container->property("thVidH").toDouble();
    double zoom = container->property("thZoom").toDouble();
    if (!(zoom > 0.0))
        zoom = 1.0;
    if (zoom > 6.0)
        zoom = 6.0;
    const bool fill = container->property("thFill").toBool();
    double panX = container->property("thPanX").toDouble();
    double panY = container->property("thPanY").toDouble();
    if (vw <= 0 || vh <= 0) {
        video->setX(0);
        video->setY(0);
        video->setWidth(cw);
        video->setHeight(ch);
        return;
    }
    const double coverBase = std::max(cw / vw, ch / vh), fitBase = std::min(cw / vw, ch / vh);
    const double base = fill ? coverBase : fitBase;
    // In fill mode the zoom may go below 1, but never below the fit scale (the
    // whole frame visible); in fit mode 1 is the floor. Mirrors the QML minZoom.
    const double minZoom = fill ? fitBase / coverBase : 1.0;
    if (zoom < minZoom)
        zoom = minZoom;
    const double s = base * zoom;
    const double iw = std::round(vw * s), ih = std::round(vh * s);
    const double ox = std::max(0.0, (iw - cw) / 2.0), oy = std::max(0.0, (ih - ch) / 2.0);
    panX = std::clamp(panX, -1.0, 1.0);
    panY = std::clamp(panY, -1.0, 1.0);
    video->setWidth(iw);
    video->setHeight(ih);
    video->setX(std::round((cw - iw) / 2.0 - panX * ox));
    video->setY(std::round((ch - ih) / 2.0 - panY * oy));
}

// QML changed thZoom/thPan/thFill on a tile: re-run the geometry.
void tinhat_video_relayout(const char *object_name) {
    if (!object_name)
        return;
    const QString name = QString::fromUtf8(object_name);
    for (QWindow *w : QGuiApplication::topLevelWindows()) {
        QQuickWindow *qw = qobject_cast<QQuickWindow *>(w);
        if (!qw)
            continue;
        QQuickItem *container = findVisualItem(qw->contentItem(), name);
        if (!container)
            continue;
        if (QQuickItem *v = container->findChild<QQuickItem *>(name + QStringLiteral("_gl")))
            applyVideoGeometry(v, container);
        return;
    }
}

int tinhat_video_attach(void *pipeline, const char *object_name) {
    if (!pipeline || !object_name)
        return 0;
    GstElement *pipe = static_cast<GstElement *>(pipeline);
    const QString name = QString::fromUtf8(object_name);
    const QString glName = name + QStringLiteral("_gl");
    const auto windows = QGuiApplication::topLevelWindows();
    int qquick_windows = 0;
    for (QWindow *w : windows) {
        QQuickWindow *qw = qobject_cast<QQuickWindow *>(w);
        if (!qw)
            continue;
        ++qquick_windows;
        // Walk the VISUAL tree (childItems), not QObject findChild — delegate items
        // are visual children but not QObject descendants of the window.
        QQuickItem *container = findVisualItem(qw->contentItem(), name);
        if (!container)
            continue;

        GstElement *sinkProbe = gst_bin_get_by_name(GST_BIN(pipe), "sink");
        if (!sinkProbe) {
            g_printerr("[video] attach %s: no 'sink' element in pipeline\n", object_name);
            return 0;
        }
        const bool useQvs = sinkIsAppsink(sinkProbe);
        gst_object_unref(sinkProbe);
        // Reuse the surface item if a previous retry already created it.
        QQuickItem *video = container->findChild<QQuickItem *>(glName);
        if (!video) {
            QQmlEngine *engine = qmlEngine(container);
            if (!engine) {
                g_printerr("[video] attach %s: no QQmlEngine for container\n", object_name);
                return 0;
            }
            QQmlComponent comp(engine);
            if (useQvs) {
                // QtMultimedia surface: QML VideoOutput — its QVideoSink receives
                // QVideoFrames from the appsink pad probe. Same objectName suffix as the
                // GL item so detach/reuse logic stays shared.
                comp.setData("import QtMultimedia\nVideoOutput { }", QUrl());
            } else {
                comp.setData("import org.freedesktop.gstreamer.Qt6GLVideoItem 1.0\n"
                             "GstGLQt6VideoItem { }",
                             QUrl());
            }
            if (comp.isError()) {
                g_printerr("[video] attach %s: video surface create FAILED: %s\n",
                           object_name, qPrintable(comp.errorString()));
                return 1; // deterministic — stop the QML retry loop
            }
            QObject *obj = comp.create(qmlContext(container));
            video = qobject_cast<QQuickItem *>(obj);
            if (!video) {
                g_printerr("[video] attach %s: created object is not a QQuickItem\n", object_name);
                if (obj)
                    obj->deleteLater();
                return 1;
            }
            video->setObjectName(glName);
            video->setParentItem(container); // visual parent
            video->setParent(container);     // ownership
            // Size it to the tile explicitly — `anchors.fill: parent` in the QML string
            // does NOT bind for a C++-created item (no declarative parent at creation),
            // which left it 0×0 / native-size and rendering outside the tile. Keep it
            // matched to the container on resize. z=0 (added last) paints above the black
            // placeholder Rectangle and below the participant-name Label (z=1).
            applyVideoGeometry(video, container);
            // QUEUED, not direct: a synchronous resize fired from the container's
            // widthChanged/heightChanged would run INSIDE the parent Flow's updatePolish(), a
            // reentrant relayout during layout. Deferring to the event loop runs the resize
            // AFTER the layout pass; the >0 guard skips the transient zero-size frame that
            // would otherwise trip gst_video_center_rect 'dst->h != 0'. (The infinite polish
            // loop itself was a separate bug — negative tile sizes — fixed in Main.qml.)
            QObject::connect(
                container, &QQuickItem::widthChanged, video,
                [video, container]() { applyVideoGeometry(video, container); },
                Qt::QueuedConnection);
            QObject::connect(
                container, &QQuickItem::heightChanged, video,
                [video, container]() { applyVideoGeometry(video, container); },
                Qt::QueuedConnection);
            g_printerr("[video] attach %s: created %s %gx%g in container %gx%g\n",
                       object_name, useQvs ? "VideoOutput" : "GstGLQt6VideoItem",
                       video->width(), video->height(),
                       container->width(), container->height());
        }

        GstElement *sink = gst_bin_get_by_name(GST_BIN(pipe), "sink");
        if (!sink) {
            g_printerr("[video] attach %s: no 'sink' element in pipeline\n", object_name);
            return 0;
        }
        // Learn the stream's natural (PAR-corrected) size from the sink caps —
        // right now if already negotiated, and on every CAPS event after — and hand it
        // to the tile so it can fit/fill/zoom precisely. Probe runs on the streaming
        // thread; the QML write hops to the GUI thread (QPointer guards teardown).
        if (GstPad *cpad = gst_element_get_static_pad(sink, "sink")) {
            QPointer<QQuickItem> cptr(container);
            auto publish = [cptr, glName](GstCaps *caps) {
                if (!caps)
                    return;
                GstVideoInfo vi;
                if (!gst_video_info_from_caps(&vi, caps))
                    return;
                const int w = GST_VIDEO_INFO_WIDTH(&vi), h = GST_VIDEO_INFO_HEIGHT(&vi);
                const int dw = (vi.par_n > 0 && vi.par_d > 0)
                    ? (int)std::llround((double)w * (double)vi.par_n / (double)vi.par_d) : w;
                if (dw <= 0 || h <= 0)
                    return;
                QMetaObject::invokeMethod(qApp, [cptr, glName, dw, h]() {
                    if (!cptr)
                        return;
                    cptr->setProperty("thVidW", dw);
                    cptr->setProperty("thVidH", h);
                    if (QQuickItem *v = cptr->findChild<QQuickItem *>(glName))
                        applyVideoGeometry(v, cptr);
                }, Qt::QueuedConnection);
            };
            if (GstCaps *cur = gst_pad_get_current_caps(cpad)) {
                publish(cur);
                gst_caps_unref(cur);
            }
            if (gulong oldId = (gulong)(guintptr)g_object_get_data(G_OBJECT(sink), "tinhat-caps-probe"))
                gst_pad_remove_probe(cpad, oldId);
            auto *fn = new std::function<void(GstCaps *)>(publish);
            gulong id = gst_pad_add_probe(
                cpad, GST_PAD_PROBE_TYPE_EVENT_DOWNSTREAM,
                [](GstPad *, GstPadProbeInfo *info, gpointer d) -> GstPadProbeReturn {
                    GstEvent *ev = GST_PAD_PROBE_INFO_EVENT(info);
                    if (ev && GST_EVENT_TYPE(ev) == GST_EVENT_CAPS) {
                        GstCaps *c = nullptr;
                        gst_event_parse_caps(ev, &c);
                        (*static_cast<std::function<void(GstCaps *)> *>(d))(c);
                    }
                    return GST_PAD_PROBE_OK;
                },
                fn, [](gpointer d) { delete static_cast<std::function<void(GstCaps *)> *>(d); });
            g_object_set_data(G_OBJECT(sink), "tinhat-caps-probe", (gpointer)(guintptr)id);
            gst_object_unref(cpad);
        }
        if (useQvs) {
        // Bind the VideoOutput's QVideoSink to the appsink via a buffer pad probe.
        // Unlike qml6glsink there is no GL winsys init: attaching to a live pipeline
        // is safe (the probe just starts seeing buffers), and PLAYING is set directly
        // from the GUI thread — no render-thread dance.
        QVideoSink *vsink = video->property("videoSink").value<QVideoSink *>();
        if (!vsink) {
            g_printerr("[video] attach %s: VideoOutput has no videoSink yet\n", object_name);
            gst_object_unref(sink);
            return 0; // not fully realized — caller retries
        }
        if (GstPad *pad = gst_element_get_static_pad(sink, "sink")) {
            // Replace any previous binding (re-attach after a rebuild): kill the old
            // ctx/probe first. The probe's destroy-notify frees the old ctx.
            if (gpointer old = g_object_get_data(G_OBJECT(sink), "tinhat-qvs-ctx"))
                static_cast<QvsCtx *>(old)->dead.store(true);
            if (gulong oldId = (gulong)(guintptr)g_object_get_data(G_OBJECT(sink), "tinhat-qvs-probe"))
                gst_pad_remove_probe(pad, oldId);
            QvsCtx *ctx = new QvsCtx;
            ctx->sink = vsink;
            gulong id = gst_pad_add_probe(
                pad, GST_PAD_PROBE_TYPE_BUFFER, qvsBufferProbe, ctx,
                [](gpointer d) { delete static_cast<QvsCtx *>(d); });
            g_object_set_data(G_OBJECT(sink), "tinhat-qvs-ctx", ctx);
            g_object_set_data(G_OBJECT(sink), "tinhat-qvs-probe", (gpointer)(guintptr)id);
            gst_object_unref(pad);
        }
        gst_object_unref(sink);
        GstStateChangeReturn r = gst_element_set_state(pipe, GST_STATE_PLAYING);
        g_printerr("[video] attach %s: QVideoSink bound (pad probe) + PLAYING -> %s\n",
                   object_name, gst_element_state_change_return_get_name(r));
        return 1;
        }
        // A LIVE pipeline must never be re-pointed here: qml6glsink's winsys init runs
        // only during NULL->READY, so binding a fresh item to a playing sink leaves it
        // permanently black (and every in-place reset strategy deadlocks — see
        // rebuild_video_tile in the streamer crate, which the call_ctl layer uses to
        // swap in a FRESH pipeline before calling us). Warn loudly if that invariant is
        // ever violated so a regression can't ship as a silent black box again.
        GstState cur = GST_STATE_NULL;
        gst_element_get_state(pipe, &cur, nullptr, 0);
        if (cur != GST_STATE_NULL) {
            g_printerr("[video] attach %s: WARNING — pipeline is LIVE (state %s); a re-bound "
                       "item will render BLACK. The caller must rebuild_video_tile first.\n",
                       object_name, gst_element_state_get_name(cur));
        }
        g_object_set(sink, "widget", video, nullptr);
        gst_object_unref(sink);
        qw->scheduleRenderJob(new SetPlaying(pipe, qw, 0), QQuickWindow::AfterRenderingStage);
        // Force a frame so the job actually runs: a freshly-created static window (the
        // PiP pop-out before its video starts) schedules no frames on its own, and a
        // scheduled render job waits for one — observed live as a queued job that never
        // executed.
        qw->update();
        g_printerr("[video] attach %s: bound sink to GstGLQt6VideoItem + PLAYING scheduled\n",
                   object_name);
        return 1;
    }
    g_printerr("[video] attach %s: container not found (%d top-level windows, %d QQuickWindows)\n",
               object_name, (int)windows.size(), qquick_windows);
    return 0;
}

// Teardown step ONE (call BEFORE the pipeline is NULLed and BEFORE QML destroys the tile
// delegate): unbind the qml6glsink's `widget` so the sink drops its pointer to the
// `GstGLQt6VideoItem`. Setting a gst pipeline to NULL does NOT clear the sink's widget
// pointer, so without this the sink (and any in-flight render job) can dereference a
// QQuickItem that QML is tearing down on participant-leave → intermittent crash. The
// qml6glsink widget setter is internally serialized (proxy mutex), so this is safe from
// the GUI thread. No-op if the pipeline has no `sink`/widget. `pipeline` = GstElement*.
void tinhat_video_predetach(void *pipeline) {
    if (!pipeline)
        return;
    GstElement *pipe = static_cast<GstElement *>(pipeline);
    // Mark RETIRED before anything else: any in-flight SetPlaying retry chain checks
    // this key (at entry AND right before its final PLAYING) and abandons itself
    // instead of resurrecting a torn-down pipeline behind retire()'s NULL. Every
    // predetach caller tears the pipeline down right after (retire/rebuild/NULL) —
    // no flow re-attaches the same GstElement — so the mark never needs clearing.
    g_object_set_data(G_OBJECT(pipe), "tinhat-retired", GINT_TO_POINTER(1));
    if (GstElement *sink = gst_bin_get_by_name(GST_BIN(pipe), "sink")) {
        if (sinkIsAppsink(sink)) {
        // QtMultimedia twin: stop the frame bridge BEFORE QML tears the tile down — flip
        // the ctx dead flag (streaming thread sees it immediately) and remove the probe.
        if (gpointer old = g_object_get_data(G_OBJECT(sink), "tinhat-qvs-ctx"))
            static_cast<QvsCtx *>(old)->dead.store(true);
        if (GstPad *pad = gst_element_get_static_pad(sink, "sink")) {
            if (gulong id = (gulong)(guintptr)g_object_get_data(G_OBJECT(sink), "tinhat-qvs-probe"))
                gst_pad_remove_probe(pad, id);
            gst_object_unref(pad);
        }
        g_object_set_data(G_OBJECT(sink), "tinhat-qvs-ctx", nullptr);
        g_object_set_data(G_OBJECT(sink), "tinhat-qvs-probe", nullptr);
        g_printerr("[video] predetach: QVideoSink probe removed\n");
        } else {
        g_object_set(sink, "widget", nullptr, nullptr);
        g_printerr("[video] predetach: unbound qml6glsink widget\n");
        }
        gst_object_unref(sink);
    }
}

// Detach a tile's video surface: find the container Item named `object_name` and DELETE
// the `<name>_gl` GstGLQt6VideoItem child created by tinhat_video_attach, revealing the
// avatar placeholder underneath. Used when the local camera turns off so the self-view
// tile reverts to the avatar (instead of a frozen last frame) while the tile stays in the
// grid. Must run on the Qt GUI thread, AFTER the tile's gst pipeline is in NULL. (NULLing
// does NOT clear the sink's widget pointer — what makes the delete safe here is that a
// NULL pipeline has no in-flight render job AND GstGLQt6VideoItem self-unregisters from the
// sink proxy on destruction. For the live-pipeline teardown path use tinhat_video_predetach
// to unbind the widget first.) No-op if the surface isn't there.
void tinhat_video_detach(const char *object_name) {
    if (!object_name)
        return;
    const QString name = QString::fromUtf8(object_name);
    const QString glName = name + QStringLiteral("_gl");
    for (QWindow *w : QGuiApplication::topLevelWindows()) {
        QQuickWindow *qw = qobject_cast<QQuickWindow *>(w);
        if (!qw)
            continue;
        QQuickItem *container = findVisualItem(qw->contentItem(), name);
        if (!container)
            continue;
        if (QQuickItem *video = container->findChild<QQuickItem *>(glName)) {
            video->setParentItem(nullptr);
            video->deleteLater();
            g_printerr("[video] detach %s: GstGLQt6VideoItem removed\n", object_name);
        }
        return;
    }
}

// Is this pipeline past NULL? The call_ctl layer asks before an attach: a LIVE pipeline
// being re-pointed (PiP pop-out / pop-back) must be REBUILT for the target window, not
// re-bound (see the warning in tinhat_video_attach / streamer rebuild_video_tile).
int tinhat_video_is_live(void *pipeline) {
    if (!pipeline)
        return 0;
    GstState cur = GST_STATE_NULL;
    gst_element_get_state(static_cast<GstElement *>(pipeline), &cur, nullptr, 0);
    return cur != GST_STATE_NULL ? 1 : 0;
}

// #9 Pause-hidden: set ONE pipeline's play state by raw pointer (the Rust side holds the
// GstElement* in its video_registry). PAUSED stops upstream decode/render for a tile nobody
// can see (window minimized/hidden); resuming to PLAYING is safe because the qml6glsink
// already reached PLAYING once (its Qt GL context is established), so this needs no
// render-thread READY dance. gst_element_set_state is thread-safe — fine from the GUI thread.
void tinhat_video_set_paused(void *pipeline, int paused) {
    if (!pipeline)
        return;
    GstElement *pipe = static_cast<GstElement *>(pipeline);
    gst_element_set_state(pipe, paused ? GST_STATE_PAUSED : GST_STATE_PLAYING);
}

} // extern "C"

// ── 10.6 (REACH §2.4): Tin Hat's own input clock ────────────────────────────────────────
// The presence idle probe's LAST fallback (after the freedesktop ScreenSaver and Mutter's
// IdleMonitor): the time since the last keyboard / pointer / wheel / touch event in ANY Tin Hat
// window. One application-wide event filter stamps a steady clock into an atomic; presence.rs
// reads its age from a tokio worker. Both platforms (the Windows build also compiles this).
#include <QEvent>
#include <QGuiApplication>
#include <QKeyEvent>
#include <QKeySequence>
#include <QMetaObject>
#include <QMouseEvent>
#include <QQuickItem>
#include <QObject>
#include <QStyleHints>
#include <QTimer>
#include <QVariant>
#include <QWindow>
#include <atomic>
#include <chrono>

namespace {
std::atomic<long long> g_thLastInputMs{0};

// ── 11.0 (GPU.md §2.1, R5): the text caret ──────────────────────────────────────────────────
// It blinks for 5 s after the last key / pointer / touch press, focus-in or window activation, then stays solid; always
// solid under Reduce motion. The blink redrew the window twice a second for as long as a field had the focus — the ONLY
// thing that drew in a focused, idle Tin Hat (the composer takes the focus on every room open). One application-wide
// switch: QStyleHints' cursor flash time, which every TextInput / TextEdit follows (0 = a steady caret). GUI thread only.
int g_thPlatformFlash = -1;          // the platform's flash time, read at install (-1 = not installed)
bool g_thCaretReduced = false;       // Reduce motion (the app's or the platform's): always steady
QTimer *g_thCaretTimer = nullptr;    // the 5 s blink window

void thCaretSteady() {
    QStyleHints *h = QGuiApplication::styleHints();
    if (h && h->cursorFlashTime() != 0)
        h->setCursorFlashTime(0);
}

void thCaretWake() {
    if (g_thPlatformFlash < 0 || !g_thCaretTimer)
        return;
    if (g_thCaretReduced || g_thPlatformFlash <= 0) {
        g_thCaretTimer->stop();
        thCaretSteady();
        return;
    }
    QStyleHints *h = QGuiApplication::styleHints();
    if (h && h->cursorFlashTime() != g_thPlatformFlash)
        h->setCursorFlashTime(g_thPlatformFlash);
    g_thCaretTimer->start();   // CARET_BLINK_MS
}

long long thSteadyMs() {
    return std::chrono::duration_cast<std::chrono::milliseconds>(
               std::chrono::steady_clock::now().time_since_epoch())
        .count();
}

// ── 11.0 (ISLANDS-ARCH §2.3 / §2.8, islands C5): ONE Back / Forward path per window ─────────────────────────────────
// The page row (Kirigami PageRow) carries its own StandardKey.Back/Forward Shortcuts; a second Shortcut with the same
// sequence would only make Qt report both as ambiguous and fire neither. So the application event filter sees the
// ShortcutOverride (and the mouse Back/Forward buttons) BEFORE the shortcut map: a window that declares
// thBack()/thForward() gets them. An open popup (menu, dialog, sheet — never a tooltip) closes first, and that is
// the whole Back: a modal never has the layout changed underneath it. Handled keys are consumed (the KeyPress too).
// 11.2.1: the keys are the web's (dispatch.ts): Alt+Left / Alt+Right and nothing else held, or the Back / Forward
// key. NOT QKeySequence::Back/Forward: on Windows those standard keys also mean a plain Backspace / Shift+Backspace,
// and this filter runs before any text field, so on Windows since 10.8 every Backspace closed the open dialog (the
// bug report closed at the first typo) or opened the collapsed nav instead of deleting a character.
int g_thSwallowKey = 0;
static bool thAltOnly(const QKeyEvent *k) { return (k->modifiers() & ~Qt::KeypadModifier) == Qt::AltModifier; }
bool thIsBack(const QKeyEvent *k) { return k->key() == Qt::Key_Back || (k->key() == Qt::Key_Left && thAltOnly(k)); }
bool thIsForward(const QKeyEvent *k) { return k->key() == Qt::Key_Forward || (k->key() == Qt::Key_Right && thAltOnly(k)); }
bool thInherits(const QMetaObject *mo, const char *name) {
    for (; mo; mo = mo->superClass())
        if (qstrcmp(mo->className(), name) == 0)
            return true;
    return false;
}
bool thCloseTopPopup(QObject *root) {
    QObject *top = nullptr;
    qreal topZ = 0;
    const auto all = root->findChildren<QObject *>();
    for (QObject *o : all) {
        const QMetaObject *mo = o->metaObject();
        if (!thInherits(mo, "QQuickPopup") || thInherits(mo, "QQuickToolTip") || !o->property("opened").toBool())
            continue;
        const qreal z = o->property("z").toReal();
        if (!top || z >= topZ) {
            top = o;
            topZ = z;
        }
    }
    if (!top)
        return false;
    QMetaObject::invokeMethod(top, "close");
    return true;
}
QWindow *thWindowOf(QObject *obj) {
    if (obj->isWindowType())
        return static_cast<QWindow *>(obj);
    if (auto *item = qobject_cast<QQuickItem *>(obj))
        return item->window();
    return nullptr;
}
bool thDispatchBack(QWindow *w, bool back) {
    if (!w || w->metaObject()->indexOfMethod(back ? "thBack()" : "thForward()") < 0)
        return false;
    if (back && thCloseTopPopup(w))
        return true;
    QVariant ret;
    QMetaObject::invokeMethod(w, back ? "thBack" : "thForward", Q_RETURN_ARG(QVariant, ret));
    return ret.toBool();
}

class ThInputClock : public QObject {
public:
    bool eventFilter(QObject *obj, QEvent *ev) override {
        switch (ev->type()) {
        case QEvent::ShortcutOverride: {
            auto *k = static_cast<QKeyEvent *>(ev);
            const bool back = thIsBack(k);
            if ((back || thIsForward(k)) && thDispatchBack(thWindowOf(obj), back)) {
                ev->accept();
                g_thSwallowKey = k->key();   // the KeyPress that follows belongs to this Back too
                return true;
            }
            break;
        }
        case QEvent::KeyPress:
            if (g_thSwallowKey != 0 && static_cast<QKeyEvent *>(ev)->key() == g_thSwallowKey) {
                g_thSwallowKey = 0;
                return true;
            }
            [[fallthrough]];
        case QEvent::MouseButtonPress:
            if (obj->isWindowType()) {
                const Qt::MouseButton b = static_cast<QMouseEvent *>(ev)->button();
                if ((b == Qt::BackButton || b == Qt::ForwardButton) && thDispatchBack(static_cast<QWindow *>(obj), b == Qt::BackButton))
                    return true;
            }
            [[fallthrough]];
        case QEvent::MouseMove:
        case QEvent::Wheel:
        case QEvent::TouchBegin:
        case QEvent::TabletPress:
            g_thLastInputMs.store(thSteadyMs(), std::memory_order_relaxed);
            if (ev->type() != QEvent::MouseMove)
                thCaretWake();   // a key / pointer press: the caret blinks again (R5)
            break;
        case QEvent::FocusIn:
        case QEvent::WindowActivate:
            thCaretWake();       // focus-in / window activation (R5)
            break;
        case QEvent::Expose:
        case QEvent::Show:
        case QEvent::Hide:
            // 11.0 (GPU.md §1): a window's REAL exposure → its `thExposed` property (the main window declares it;
            // AppCore's window state reads it). KWin Wayland never reports a minimise; an unmapped / suspended window
            // gets an expose event with an empty region instead.
            if (obj->isWindowType())
                thQueueExposure(static_cast<QWindow *>(obj));
            break;
        default:
            break;
        }
        return QObject::eventFilter(obj, ev);
    }

private:
    // Read after the event is processed (queued: the window's exposed state is final then), and only for a window
    // that declares the property (setProperty would add a dynamic one to any other window).
    static void thQueueExposure(QWindow *w) {
        if (w->metaObject()->indexOfProperty("thExposed") < 0)
            return;
        QMetaObject::invokeMethod(
            w,
            [w]() {
                const bool exposed = w->isVisible() && w->isExposed();
                if (w->property("thExposed").toBool() != exposed)
                    w->setProperty("thExposed", exposed);
            },
            Qt::QueuedConnection);
    }
};
} // namespace

extern "C" void tinhat_input_watch_install(void) {
    static ThInputClock *clock = nullptr;
    if (clock || !qGuiApp)
        return;
    g_thLastInputMs.store(thSteadyMs(), std::memory_order_relaxed); // launching counts as input
    clock = new ThInputClock();
    qGuiApp->installEventFilter(clock);
    // the caret (R5): remember the platform's blink, then blink only for 5 s after input
    if (QStyleHints *h = QGuiApplication::styleHints())
        g_thPlatformFlash = h->cursorFlashTime();
    g_thCaretTimer = new QTimer(clock);
    g_thCaretTimer->setSingleShot(true);
    g_thCaretTimer->setInterval(5000);
    QObject::connect(g_thCaretTimer, &QTimer::timeout, clock, []() { thCaretSteady(); });
    thCaretWake();
}

// 11.0 (GPU.md §2.1): Reduce motion (the app's or the platform's) → a steady caret; off → it blinks again for 5 s.
extern "C" void tinhat_caret_set_reduced(int reduced) {
    g_thCaretReduced = reduced != 0;
    thCaretWake();
}

// 11.0 (GPU.md §1, R2): the OS asks for no animations. Windows: Settings → Accessibility → Animation effects
// (SPI_GETCLIENTAREAANIMATION — what Chromium's prefers-reduced-motion reads there). Elsewhere 0: Linux QML reads KDE's
// animation speed through Kirigami.Units (0 = "Instant").
extern "C" int tinhat_platform_reduced_motion(void) {
#ifdef _WIN32
    BOOL on = TRUE;
    if (!SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, &on, 0))
        return 0;
    return on ? 0 : 1;
#else
    return 0;
#endif
}

extern "C" long long tinhat_input_idle_ms(void) {
    const long long last = g_thLastInputMs.load(std::memory_order_relaxed);
    if (last <= 0)
        return -1; // not installed (headless)
    const long long d = thSteadyMs() - last;
    return d < 0 ? 0 : d;
}
