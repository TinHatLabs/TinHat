// theme_bridge.cpp — the theme editor's Qt-side helper (11.0 step 5), GUI thread, plain extern "C" (no Q_OBJECT, no
// moc; the shape of clipboard_bridge.cpp): the review harness's window grab (ThemeEditorGallery.qml) —
// QQuickWindow::grabWindow of the visible Quick window, the whole scene, popups and dialogs included (an item's
// grabToImage cannot reach the popup overlay).
//
// (A theme look's fonts are NOT set here: ENGINE.md §10 on the desktop is the main window's font — Main.qml binds
// Th.fontUi / Th.textScale onto it and every control inherits it — plus the house text components' sizes. Changing
// QGuiApplication::font at run time reaches neither the controls nor the Basic style's Kirigami fonts, and it would make
// a plain Text's family depend on when it was created.)

#include <QGuiApplication>
#include <QImage>
#include <QQuickWindow>
#include <QWindow>
#include <QString>

extern "C" {

// Grab the visible Quick window titled `title` into `path` (PNG). 1 = saved, 0 = no such window / not saved.
int tinhat_grab_window(const char *path, const char *title) {
    const QString want = QString::fromUtf8(title ? title : "");
    const QWindowList wins = QGuiApplication::topLevelWindows();
    for (QWindow *w : wins) {
        auto *qw = qobject_cast<QQuickWindow *>(w);
        if (!qw || !qw->isVisible() || qw->title() != want) continue;
        const QImage img = qw->grabWindow();
        if (img.isNull()) return 0;
        return img.save(QString::fromUtf8(path ? path : ""), "PNG") ? 1 : 0;
    }
    return 0;
}

}  // extern "C"
