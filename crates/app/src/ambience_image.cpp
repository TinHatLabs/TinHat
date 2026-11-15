// ambience_image.cpp — the plain-C++ side of chat backgrounds: is the window drawn on the CPU (below), and a chat
// background's own image, re-encoded before it is posted (docs/AMBIENCE.md §3.2; the web's
// ambience/room.ts reencode): EXIF orientation applied, ≤ max side on the long edge, JPEG through a quality ladder and
// then smaller until it fits the byte cap. A re-encode keeps no metadata at all (no EXIF, no GPS). Plain extern "C",
// no Q_OBJECT (like the other bridges here).
#include <QBuffer>
#include <QByteArray>
#include <QFile>
#include <QImage>
#include <QImageReader>
#include <QPainter>
#include <QString>
#include <QOpenGLContext>
#include <QOpenGLFunctions>
#include <QQuickWindow>
#include <algorithm>
#include <cmath>
#if __has_include(<QtGui/rhi/qrhi.h>)
#include <QtGui/rhi/qrhi.h>
#define TINHAT_HAVE_QRHI 1
#endif

// Is the window drawn by a CPU rasteriser (llvmpipe / softpipe / SwiftShader / WARP)? Then every animated frame redraws
// the whole window on the CPU, so the particles stay off (AmbienceLayer.qml). Called on the render thread from
// updatePaintNode. Qt's RHI says it directly where its header is reachable; else the OpenGL renderer's name.
extern "C" bool tinhat_ambience_cpu_device(QQuickWindow *w) {
    if (!w) return false;
    QByteArray name;
#ifdef TINHAT_HAVE_QRHI
    if (QRhi *rhi = w->rhi()) {
        const QRhiDriverInfo d = rhi->driverInfo();
        if (d.deviceType == QRhiDriverInfo::CpuDevice) return true;
        name = d.deviceName;
    }
#endif
    if (name.isEmpty()) {
        if (QOpenGLContext *c = QOpenGLContext::currentContext()) {
            const GLubyte *r = c->functions()->glGetString(GL_RENDERER);
            if (r) name = QByteArray(reinterpret_cast<const char *>(r));
        }
    }
    name = name.toLower();
    return name.contains("llvmpipe") || name.contains("softpipe") || name.contains("swiftshader") || name.contains("basic render");
}

extern "C" long long tinhat_ambience_reencode(const char *src, const char *dst, int maxSide, long long maxBytes,
                                              const double *qualities, int nq, double shrink, int *outW, int *outH) {
    QImageReader r(QString::fromUtf8(src));
    r.setAutoTransform(true);
    QImage img = r.read();
    if (img.isNull() || img.width() < 1 || img.height() < 1 || nq < 1) return -1;
    // the shared fitImage: the long side capped, aspect kept, whole pixels
    const double k = std::fmin(1.0, double(maxSide) / double(std::max(img.width(), img.height())));
    int w = std::max(1, int(std::floor(img.width() * k + 0.5)));
    int h = std::max(1, int(std::floor(img.height() * k + 0.5)));
    // a transparent image gets a black matte (JPEG has no alpha), the same as the web's canvas
    QImage flat(img.size(), QImage::Format_RGB32);
    flat.fill(Qt::black);
    {
        QPainter p(&flat);
        p.drawImage(0, 0, img);
    }
    for (int round = 0; round < 8; ++round) {
        const QImage scaled = (w == flat.width() && h == flat.height()) ? flat : flat.scaled(w, h, Qt::IgnoreAspectRatio, Qt::SmoothTransformation);
        for (int i = 0; i < nq; ++i) {
            QByteArray bytes;
            QBuffer buf(&bytes);
            buf.open(QIODevice::WriteOnly);
            if (!scaled.save(&buf, "JPG", int(std::floor(qualities[i] * 100.0 + 0.5)))) return -1;
            if (bytes.size() <= maxBytes) {
                QFile f(QString::fromUtf8(dst));
                if (!f.open(QIODevice::WriteOnly) || f.write(bytes) != bytes.size()) return -1;
                if (outW) *outW = w;
                if (outH) *outH = h;
                return bytes.size();
            }
        }
        w = std::max(1, int(std::floor(w * shrink + 0.5)));
        h = std::max(1, int(std::floor(h * shrink + 0.5)));
    }
    return -1;
}

// A chat background image's colour cells (docs/AMBIENCE.md, ambience::image_cells): its first frame split 8 × 8, each
// cell the per-channel min / max over EVERY pixel in it (downsampling would average a thin line away). `out` gets 6
// doubles per cell (lo r g b, hi r g b, 0..1). Returns the number of cells, or -1 (unreadable, larger than maxSide, or a
// header that doesn't give the size: nothing is decoded before its size is known).
extern "C" int tinhat_ambience_cells(const char *path, int maxSide, double *out, int cap) {
    QImageReader r(QString::fromUtf8(path));
    r.setAutoTransform(true);
    // 3-6: the size from the header BEFORE decoding; a picture whose header doesn't say its size is never decoded
    const QSize sz = r.size();
    if (!sz.isValid() || sz.width() > maxSide || sz.height() > maxSide) return -1;
    QImage img = r.read();
    if (img.isNull() || img.width() > maxSide || img.height() > maxSide) return -1;
    // a transparent pixel shows the scene behind it: such a cell could be any colour (the widest box)
    const bool alpha = img.hasAlphaChannel();
    const QImage argb = alpha ? img.convertToFormat(QImage::Format_ARGB32) : QImage();
    img = img.convertToFormat(QImage::Format_RGB32);
    const int w = img.width(), h = img.height(), grid = 8;
    int n = 0;
    for (int gy = 0; gy < grid; ++gy)
        for (int gx = 0; gx < grid; ++gx) {
            const int x0 = gx * w / grid, x1 = std::max(x0 + 1, (gx + 1) * w / grid);
            const int y0 = gy * h / grid, y1 = std::max(y0 + 1, (gy + 1) * h / grid);
            int lo[3] = {255, 255, 255}, hi[3] = {0, 0, 0};
            bool any = false;
            for (int y = y0; y < y1 && y < h; ++y) {
                const QRgb *line = reinterpret_cast<const QRgb *>(img.constScanLine(y));
                for (int x = x0; x < x1 && x < w; ++x) {
                    if (alpha && qAlpha(reinterpret_cast<const QRgb *>(argb.constScanLine(y))[x]) < 255) {
                        lo[0] = lo[1] = lo[2] = 0;
                        hi[0] = hi[1] = hi[2] = 255;
                        any = true;
                        continue;
                    }
                    const int c[3] = {qRed(line[x]), qGreen(line[x]), qBlue(line[x])};
                    for (int k = 0; k < 3; ++k) {
                        lo[k] = std::min(lo[k], c[k]);
                        hi[k] = std::max(hi[k], c[k]);
                    }
                    any = true;
                }
            }
            if (!any || n >= cap) continue;
            for (int k = 0; k < 3; ++k) {
                out[n * 6 + k] = lo[k] / 255.0;
                out[n * 6 + 3 + k] = hi[k] / 255.0;
            }
            ++n;
        }
    return n;
}
