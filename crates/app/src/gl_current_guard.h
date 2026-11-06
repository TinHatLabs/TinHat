// gl_current_guard.h: keep GStreamer's thread-current GL context alive on every Qt
// scene-graph render thread that paints qml6glsink tiles (Linux GL tiles only).
//
// The bug (crash report 2026-11-04, 12.1.8 Flatpak, KDE Wayland): the qml6 plugin's
// Qt6GLVideoItem::updatePaintNode activates ITS OWN wrapped Qt context on the render
// thread when nothing is current (gst_gl_context_activate stores a raw, UNREFFED pointer
// in thread-local storage) and never deactivates it. Every other tile's material then
// records "the current context" in its setBuffer and waits on it at bind
// (gst_gl_sync_meta_wait -> gst_gl_context_thread_add, which requires that context to be
// active on this thread). Tiles created together each wrap their own context, so when the
// tile whose context is current is torn down (a participant's camera/share ends) its
// context is finalized while the render thread still holds it as current: each frame of
// every remaining tile then hits `g_weak_ref_set: assertion 'object == NULL ||
// G_IS_OBJECT (object)' failed`, and the freed memory is reused -> crash.
//
// The fix keeps that context alive: right after each sync (where every updatePaintNode
// runs, with the GUI thread blocked), take a strong ref on whatever context is current on
// this render thread. One ref per render THREAD (thread_local), not per window: the basic
// render loop paints every window on the GUI thread, which shares one current context.
// The ref moves when the current context changes and is dropped when the thread exits.
// The context is a wrapped one (no GL resources of its own; it wraps the window's Qt GL
// context): keeping it costs a small object and a display ref, no GL resources. Frame time only takes refs (no activate/deactivate); the one
// deactivate is at render-thread exit.
#pragma once
#ifndef _WIN32
#include <QQuickWindow>
#include <QVariant>
#include <gst/gst.h>

extern "C" {
typedef struct _GstGLContext GstGLContext;
GstGLContext *gst_gl_context_get_current(void); // gstreamer-gl-1.0, transfer none
gboolean gst_gl_context_activate(GstGLContext *context, gboolean activate);
}

namespace tinhat_glguard {

struct Kept {
    GstGLContext *ctx = nullptr;
    ~Kept() { // render thread exit
        if (!ctx)
            return;
        // Still current here? Unset it first, so no later TLS user on this thread sees a
        // freed pointer (a wrapped context's deactivate is bookkeeping only, no GL call).
        if (gst_gl_context_get_current() == ctx)
            gst_gl_context_activate(ctx, FALSE);
        gst_object_unref(ctx);
    }
};

// Render thread, right after the sync. The current context was made current during
// this sync by a live item, or is the one already kept: either way it is alive here.
inline void keep_current() {
    static thread_local Kept kept;
    GstGLContext *cur = gst_gl_context_get_current();
    if (cur == kept.ctx)
        return;
    GstGLContext *old = kept.ctx;
    kept.ctx = cur ? static_cast<GstGLContext *>(gst_object_ref(cur)) : nullptr;
    if (old)
        gst_object_unref(old); // no longer current on this thread: safe to let go
    g_printerr("[video] gl-guard: render thread current context %s\n",
               cur ? "kept alive" : "none");
}

// GUI thread, before the first qml6glsink is bound into this window: a tile's
// updatePaintNode activates nothing before caps, and caps can only reach the sink after
// the bind (the pipeline goes to PAUSED/PLAYING from the render job that follows it).
// Idempotent per window; every window with GL tiles (main, PiP) goes through attach.
//
// Known and NOT changed here (upstream, pre-existing): if Qt destroys and recreates the
// window's native GL context on a thread that lives on, the plugin keeps reusing the
// current wrapper of the old handle (gst_qml6_get_gl_wrapcontext matches on the display
// only). This guard keeps that wrapper alive exactly as a surviving tile would.
inline void install(QQuickWindow *qw) {
    if (!qw || qw->property("tinhat-gl-guard").toBool())
        return;
    qw->setProperty("tinhat-gl-guard", true);
    QObject::connect(qw, &QQuickWindow::afterSynchronizing, qw, [] { keep_current(); },
                     Qt::DirectConnection);
}

} // namespace tinhat_glguard
#endif
