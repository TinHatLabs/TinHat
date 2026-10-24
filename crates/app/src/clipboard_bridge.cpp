// Clipboard shim for Ctrl+V attachments (9.3.2). QML has no clipboard API beyond TextEdit's
// text paste, so the composer asks here — on the GUI thread, through the QGuiApplication
// clipboard — what a paste should ATTACH rather than type:
//
//   * an image (Spectacle / Snipping Tool "copy to clipboard", a browser's "Copy image") is
//     saved as a PNG under `dir` (the media cache, so the startup sweep reclaims it later)
//   * files copied in a file manager (text/uri-list) arrive as their local paths
//   * text only → nothing; the TextArea pastes it itself
//
// Also the image-header probe used for m.image info.w/h (tinhat_image_size).
// No Q_OBJECT here, so no moc step: plain extern "C" (same shape as webengine_bridge.cpp).

#include <QGuiApplication>
#include <QClipboard>
#include <QMimeData>
#include <QImage>
#include <QImageReader>
#include <QDateTime>
#include <QDir>
#include <QFileInfo>
#include <QUrl>
#include <QString>
#include <QStringList>
#include <QByteArray>
#include <cstring>

extern "C" {

// Writes newline-separated local paths (UTF-8) into `out` (capacity `out_len` bytes).
// Returns the byte count written, 0 when there is nothing to attach, -1 on error.
int tinhat_clipboard_attachments(const char* dir, char* out, int out_len) {
    if (!out || out_len <= 0) return -1;
    out[0] = 0;
    QClipboard* cb = QGuiApplication::clipboard();
    if (!cb) return 0;
    const QMimeData* md = cb->mimeData();
    if (!md) return 0;
    QStringList paths;
    if (md->hasImage()) {
        const QImage img = qvariant_cast<QImage>(md->imageData());
        if (!img.isNull()) {
            QDir d(QString::fromUtf8(dir));
            if (!d.exists()) d.mkpath(QStringLiteral("."));
            const QString name = QStringLiteral("pasted-%1.png")
                .arg(QDateTime::currentDateTime().toString(QStringLiteral("yyyyMMdd-HHmmss-zzz")));
            const QString path = d.filePath(name);
            if (img.save(path, "PNG")) paths << path;
        }
    }
    if (paths.isEmpty() && md->hasUrls()) {
        const QList<QUrl> urls = md->urls();
        for (const QUrl& u : urls) {
            if (!u.isLocalFile()) continue;
            const QString p = u.toLocalFile();
            if (QFileInfo(p).isFile()) paths << p;
        }
    }
    if (paths.isEmpty()) return 0;
    const QByteArray joined = paths.join(QLatin1Char('\n')).toUtf8();
    if (joined.size() + 1 > out_len) return -1;
    std::memcpy(out, joined.constData(), static_cast<size_t>(joined.size()));
    out[joined.size()] = 0;
    return static_cast<int>(joined.size());
}

// Pixel size of an image file from its header (no full decode). Returns 1 and fills w/h,
// or 0 when the file is not an image Qt can read. Sent as m.image `info.w/h` so every
// receiving timeline reserves the row's height BEFORE the bytes arrive (no jump while the
// picture loads) — and the local echo row gets its true size at once.
int tinhat_image_size(const char* path, int* w, int* h) {
    if (!path || !w || !h) return 0;
    QImageReader r(QString::fromUtf8(path));
    r.setAutoTransform(true);   // EXIF orientation → the size the viewer will show
    const QSize s = r.size();
    if (!s.isValid() || s.width() <= 0 || s.height() <= 0) return 0;
    *w = s.width(); *h = s.height();
    return 1;
}

} // extern "C"
