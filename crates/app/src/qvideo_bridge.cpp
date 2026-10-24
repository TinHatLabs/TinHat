// Windows-primary video display tail - prototype/self-test shim (HANDOFF #6 item 3,
// MEDIA_PIPELINE.md #6.3): appsink -> QVideoFrame -> QVideoSink -> QML VideoOutput.
//
// This file is compiled ONLY on Windows (see build.rs). It provides the TINHAT_QVS_TEST
// gate's C++ half: a window with a VideoOutput created from an INLINE QML string (no
// qml_files/build.rs QML changes - the Linux qmlcache is untouched), plus an I420
// frame-push entry the Rust side feeds from a GStreamer appsink. The real per-tile
// attach/detach strategy swap (video_bridge.cpp's GlQt6 -> AppsinkQVideo) builds on
// exactly this path once the prototype is proven.
//
// Threading: QVideoFrame is implicitly shared and safe to construct off-thread;
// QVideoSink::setVideoFrame must run on the sink's (GUI) thread - marshalled with a
// queued QMetaObject::invokeMethod, mirroring qml6glsink's GUI-thread affinity.
//
// No Q_OBJECT here, so no moc step: plain extern "C" (matches video_bridge.cpp).

#include <atomic>
#include <cstdint>
#include <cstdio>
#include <cstring>

#include <QGuiApplication>
#include <QMetaObject>
#include <QObject>
#include <QQmlApplicationEngine>
#include <QQuickItem>
#include <QVariant>
#include <QVideoFrame>
#include <QVideoFrameFormat>
#include <QVideoSink>

namespace {
std::atomic<long> g_frames_seen{0};
QQmlApplicationEngine *g_qvs_engine = nullptr; // leaked on purpose (test window, process lifetime)
} // namespace

extern "C" {

// Create the test window (inline QML: Window + VideoOutput objectName "qvstile"),
// find its QVideoSink and connect a frame counter. Returns the QVideoSink* as an
// opaque pointer, or null on any failure (logged). Call on the GUI thread AFTER
// QGuiApplication exists.
void *tinhat_qvs_open_window(void) {
    static const char kQml[] =
        "import QtQuick\n"
        "import QtQuick.Window\n"
        "import QtMultimedia\n"
        "Window {\n"
        "    visible: true; width: 640; height: 360; title: 'TINHAT_QVS_TEST'\n"
        "    color: '#222222'\n"
        "    VideoOutput { objectName: 'qvstile'; anchors.fill: parent }\n"
        "}\n";
    g_qvs_engine = new QQmlApplicationEngine();
    g_qvs_engine->loadData(QByteArray(kQml));
    if (g_qvs_engine->rootObjects().isEmpty()) {
        std::fprintf(stderr, "[qvs] test window failed to load\n");
        return nullptr;
    }
    QObject *root = g_qvs_engine->rootObjects().first();
    QObject *tile = root->findChild<QObject *>(QStringLiteral("qvstile"));
    if (!tile) {
        std::fprintf(stderr, "[qvs] VideoOutput 'qvstile' not found\n");
        return nullptr;
    }
    QVideoSink *sink = tile->property("videoSink").value<QVideoSink *>();
    if (!sink) {
        std::fprintf(stderr, "[qvs] VideoOutput has no videoSink\n");
        return nullptr;
    }
    QObject::connect(sink, &QVideoSink::videoFrameChanged, sink,
                     [](const QVideoFrame &) { g_frames_seen.fetch_add(1); });
    std::fprintf(stderr, "[qvs] test window up, QVideoSink acquired\n");
    return sink;
}

// Push one I420 frame into the sink (copies the three planes, honoring source
// strides vs the QVideoFrame's bytesPerLine). Safe from any thread. Returns 1 on
// success, 0 on failure.
int tinhat_qvs_push_i420(void *sink_ptr, const uint8_t *y, int y_stride, const uint8_t *u,
                       int u_stride, const uint8_t *v, int v_stride, int width, int height) {
    if (!sink_ptr || !y || !u || !v || width <= 0 || height <= 0)
        return 0;
    QVideoSink *sink = static_cast<QVideoSink *>(sink_ptr);

    QVideoFrameFormat fmt(QSize(width, height), QVideoFrameFormat::Format_YUV420P);
    QVideoFrame frame(fmt);
    if (!frame.map(QVideoFrame::WriteOnly))
        return 0;
    // Plane 0: Y (width x height); planes 1/2: U,V (width/2 x height/2).
    const uint8_t *src[3] = {y, u, v};
    const int srcStride[3] = {y_stride, u_stride, v_stride};
    for (int p = 0; p < 3; ++p) {
        const int rows = (p == 0) ? height : height / 2;
        const int rowBytes = (p == 0) ? width : width / 2;
        uint8_t *dst = frame.bits(p);
        const int dstStride = frame.bytesPerLine(p);
        for (int r = 0; r < rows; ++r)
            std::memcpy(dst + r * dstStride, src[p] + r * srcStride[p], rowBytes);
    }
    frame.unmap();

    // Queued: setVideoFrame on the sink's own (GUI) thread; QVideoFrame is COW-shared.
    QMetaObject::invokeMethod(
        sink, [sink, frame]() { sink->setVideoFrame(frame); }, Qt::QueuedConnection);
    return 1;
}

// How many frames the sink has CONSUMED (videoFrameChanged) - the verdict source.
long tinhat_qvs_frames_seen(void) {
    return g_frames_seen.load();
}

} // extern "C"
