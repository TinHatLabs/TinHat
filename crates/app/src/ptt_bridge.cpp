// Push to talk's hold key, seen whatever has the keyboard (launch-gate 2026-11-07, review item 1; no Q_OBJECT, no moc: the
// shape of theme_bridge.cpp). The key used to be read by one focused Item in Main.qml, so a press in the message box never
// reached it, and a release with a menu open went to the menu: the mic stayed open.
//
// An application event filter looks at each key event once, as it reaches a window (before Qt Quick hands it to an item),
// and never consumes it: the message box still gets its Ctrl, and shortcuts still work. Main.qml decides whether the key is
// push to talk's (it knows the person's binding).
//   - a press starts a hold only from the main window, and not while a modal window is up
//   - a release from ANY window ends it (a transient window can have taken the keyboard)
// Losing the window's focus ends the hold in Main.qml (onActiveChanged / onKeysHereChanged).

#include <QGuiApplication>
#include <QKeyEvent>
#include <QMetaObject>
#include <QObject>
#include <QPointer>
#include <QVariant>
#include <QWindow>

namespace {

const char *const kMethod = "tinhatPttKeyEvent(QVariant,QVariant,QVariant)";

QWindow *findMainWindow() {
    static QPointer<QWindow> cached;
    if (cached) return cached;
    for (QWindow *w : QGuiApplication::allWindows()) {
        if (w->metaObject()->indexOfMethod(kMethod) >= 0) {
            cached = w;
            return w;
        }
    }
    return nullptr;
}

// 4-1 (2026-11-14): a modifier held besides the key itself (Shift+F10 = the menu key, Shift+Ctrl = a chord): such a press
// never starts a hold (ptt_rules::press_starts). The key's own modifier bit and the keypad / group bits don't count.
bool otherModifiers(const QKeyEvent *key) {
    // (12.1.16 review round: AltGr = GroupSwitchModifier is a modifier of its own on some layouts, not Ctrl / Alt)
    Qt::KeyboardModifiers m = key->modifiers() & (Qt::ShiftModifier | Qt::ControlModifier | Qt::AltModifier | Qt::MetaModifier | Qt::GroupSwitchModifier);
    switch (key->key()) {
    case Qt::Key_Control: m &= ~Qt::ControlModifier; break;
    case Qt::Key_Shift: m &= ~Qt::ShiftModifier; break;
    case Qt::Key_Alt: m &= ~Qt::AltModifier; break;
    case Qt::Key_AltGr: m &= ~(Qt::AltModifier | Qt::GroupSwitchModifier); break;
    case Qt::Key_Meta:
    case Qt::Key_Super_L:
    case Qt::Key_Super_R: m &= ~Qt::MetaModifier; break;
    default: break;
    }
    return m != Qt::NoModifier;
}

class PttKeyFilter : public QObject {
public:
    using QObject::QObject;

protected:
    bool eventFilter(QObject *obj, QEvent *ev) override {
        const QEvent::Type t = ev->type();
        if (t == QEvent::ShortcutOverride) {
            // Qt offers a key to its shortcuts first, and a key a shortcut takes never arrives as a KeyPress: this is the only
            // sign of Ctrl+K during a Ctrl hold. It can reach several objects (the focus item, the window): ending a hold twice is
            // harmless. A modifier on its own is the press of the hold's own key, not another key.
            QKeyEvent *chord = static_cast<QKeyEvent *>(ev);
            const int k = chord->key();
            if (chord->isAutoRepeat() || k == Qt::Key_Control || k == Qt::Key_Shift || k == Qt::Key_Alt || k == Qt::Key_AltGr ||
                k == Qt::Key_Meta || k == Qt::Key_Super_L || k == Qt::Key_Super_R)
                return false;
            if (QWindow *main = findMainWindow())
                QMetaObject::invokeMethod(main, "tinhatPttChordKey", Qt::DirectConnection, Q_ARG(QVariant, QVariant(k)));
            return false;
        }
        if (t != QEvent::KeyPress && t != QEvent::KeyRelease) return false;
        // once per event: the same event is sent on to the focused item afterwards
        QWindow *from = qobject_cast<QWindow *>(obj);
        if (!from) return false;
        QKeyEvent *key = static_cast<QKeyEvent *>(ev);
        if (key->isAutoRepeat()) return false;
        QWindow *main = findMainWindow();
        if (!main) return false;
        const bool down = t == QEvent::KeyPress;
        if (down && (from != main || QGuiApplication::modalWindow())) return false;
        QMetaObject::invokeMethod(main, "tinhatPttKeyEvent", Qt::DirectConnection, Q_ARG(QVariant, QVariant(key->key())),
                                  Q_ARG(QVariant, QVariant(down)), Q_ARG(QVariant, QVariant(otherModifiers(key))));
        return false;
    }
};

} // namespace

extern "C" void tinhat_install_ptt_key_filter() {
    static PttKeyFilter *filter = nullptr;
    if (filter || !qGuiApp) return;
    filter = new PttKeyFilter(qGuiApp);
    qGuiApp->installEventFilter(filter);
}
