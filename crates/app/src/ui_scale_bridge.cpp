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
#include <QEvent>
#include <QGuiApplication>
#include <QList>
#include <QLoggingCategory>
#include <QObject>
#include <QPointer>
#include <QQuickItem>
#include <QQuickWindow>
#include <QRect>
#include <QSet>
#include <QWindow>
#include <QtGlobal>

#include <cstring>
#include <utility>

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

// -- The guarded DPR walk (2026-11-04, a user's coredump: docs/UI-ZOOM.md) --
// A DPR change makes QQuickWindow walk its item tree (QQuickWindow::physicalDpiChanged -> the recursive static
// updatePixelRatioHelper, qquickwindow.cpp): per level it COPIES childItems() into a list of raw pointers, then tells
// each item that has ItemHasContents "ItemDevicePixelRatioHasChanged", then recurses. Those notifications run app code
// synchronously: an Image reloads at the new ratio, a natively rendered Text re-lays itself out (the Kirigami breeze /
// desktop platform plugin turns native text rendering on at an integer DPR), and their signals run bindings. If any of
// that deletes an item at once that is still in a list copied higher up, the walk then calls the freed item: a user's
// 12.1.8 Flatpak on Plasma Wayland died exactly there (SEGV in QQuickItemPrivate::itemChange, qquickitem.cpp:7175,
// loading the freed item's vtable, 28 levels deep in updatePixelRatioHelper). Qt takes that walk for a monitor's own
// scale change too; the live App size change only made it run on every key press.
// The guard, while applyNow() delivers the change (QWindowPrivate::updateDevicePixelRatio has already stored the new
// ratio and sends QEvent::DevicePixelRatioChange synchronously; this event filter on each QQuickWindow sees it first):
//   1. the notification runs over a snapshot of the WHOLE tree held as QPointers, taken before any item is told: an item
//      deleted by an earlier one's reaction is skipped (its QPointer is null), never called;
//   2. every item that has ItemHasContents then has the flag cleared and the event goes on (other filters, e.g.
//      Kirigami's AlignedSize, and QQuickWindow::event run as before): Qt's own walk only tells items with that flag, so
//      it tells none (no app code runs inside it), and it still records the ratio, polishes and emits
//      devicePixelRatioChanged;
//   3. when the synchronous delivery returns, applyNow() gives the flag back and calls update() + polish() on those items
//      (update() on an item without the flag is dropped, and Qt's polish pass saw none): one full redraw at the new ratio.
// Clearing/setting the flag only stores it (QQuickItem::setFlags); no render sync can happen in between because the GUI
// thread does not return to the event loop. Public API only: QQuickItem::itemChange is protected, called through a
// pointer to member taken in a derived class (exactly what QQuickItemPrivate::itemChange does for this change).
#if TINHAT_UI_SCALE_QPA
struct ItemChangeCall : QQuickItem {
    using QQuickItem::itemChange;
};

struct WalkStats {
    int walks = 0;                  // guarded walks this apply (one per DPR change of a Qt Quick window)
    int items = 0;                  // items told
    int gone = 0;                   // items deleted by another item's reaction during a walk (skipped)
    int muted = 0;                  // items whose ItemHasContents was held for Qt's own walk
    QList<QByteArray> goneTypes;    // the deleted items' class names (the first few, for the log: the field trigger)
};
WalkStats g_walk;

struct Muted {
    QPointer<QQuickWindow> window;
    QList<QPointer<QQuickItem>> items;   // had ItemHasContents; cleared for Qt's walk, given back by restoreMuted()
};
QList<Muted> g_muted;

void collectItems(QQuickItem *item, QList<QPointer<QQuickItem>> &out, QList<QByteArray> *types) {
    out.append(item);
    if (types) types->append(QByteArray(item->metaObject()->className()));   // copied now: a dynamic metaobject dies with its item
    const QList<QQuickItem *> kids = item->childItems();                     // nothing runs until the snapshot is complete
    for (QQuickItem *k : kids) collectItems(k, out, types);
}

bool g_inGuard = false;   // inside a guarded walk: a nested DPR event is left to Qt (no recursion)

// 1 + 2 above; false = the window itself went away (the event must not go on to it)
bool guardedDprWalk(QQuickWindow *w) {
    QPointer<QQuickWindow> alive(w);
    if (!w->contentItem()) return true;
    // what physicalDpiChanged hands its helper (a float); an Image compares it with the ratio it holds
    const float ratio = float(w->effectiveDevicePixelRatio());
    void (QQuickItem::*notify)(QQuickItem::ItemChange, const QQuickItem::ItemChangeData &) = &ItemChangeCall::itemChange;
    ++g_walk.walks;
    // told: every item this walk has handled (held as QPointers: an address reused by a new item is not "told")
    QList<QPointer<QQuickItem>> told;
    // round 0 = the whole tree; rounds 1..3 = items that appeared during the previous round (created, or moved in from
    // elsewhere) — Qt's walk would have reached those too, and nothing is muted before it was told
    for (int round = 0; round < 4; ++round) {
        QList<QPointer<QQuickItem>> items;
        QList<QByteArray> types;
        collectItems(w->contentItem(), items, &types);
        if (round > 0) {
            QSet<QQuickItem *> seen;
            for (const QPointer<QQuickItem> &t : told)
                if (t) seen.insert(t.data());
            QList<QPointer<QQuickItem>> fresh;
            QList<QByteArray> freshTypes;
            for (qsizetype i = 0; i < items.size(); ++i)
                if (items.at(i) && !seen.contains(items.at(i).data())) { fresh.append(items.at(i)); freshTypes.append(types.at(i)); }
            if (fresh.isEmpty()) break;
            items = fresh;
            types = freshTypes;
        }
        for (qsizetype i = 0; i < items.size(); ++i) {
            QQuickItem *it = items.at(i).data();
            told.append(items.at(i));
            if (!it) {
                ++g_walk.gone;
                if (g_walk.goneTypes.size() < 6) g_walk.goneTypes.append(types.at(i));
                continue;
            }
            if (it->window() != w || !(it->flags() & QQuickItem::ItemHasContents)) continue;   // moved away / nothing to tell
            ++g_walk.items;
            (it->*notify)(QQuickItem::ItemDevicePixelRatioHasChanged, QQuickItem::ItemChangeData(qreal(ratio)));
            if (!alive) return false;
        }
        if (!alive || !w->contentItem()) return alive;
    }
    // 2: hold ItemHasContents over Qt's walk (a fresh snapshot: the reactions above may have changed the tree)
    QList<QPointer<QQuickItem>> now;
    collectItems(w->contentItem(), now, nullptr);
    Muted m;
    m.window = w;
    for (const QPointer<QQuickItem> &p : now)
        if (p && (p->flags() & QQuickItem::ItemHasContents)) {
            p->setFlag(QQuickItem::ItemHasContents, false);
            m.items.append(p);
        }
    g_walk.muted += int(m.items.size());
    g_muted.append(m);
    return true;
}

// 3 above: after the synchronous delivery (and as a safety net at the end of every apply)
void restoreMuted() {
    const QList<Muted> all = std::exchange(g_muted, {});
    for (const Muted &m : all)
        for (const QPointer<QQuickItem> &p : m.items)
            if (p) {
                p->setFlag(QQuickItem::ItemHasContents, true);
                p->update();
                p->polish();
            }
}

class DprWalkGuard final : public QObject {
public:
    bool eventFilter(QObject *o, QEvent *e) override {
        if (e->type() == QEvent::DevicePixelRatioChange && !g_inGuard)
            if (auto *qw = qobject_cast<QQuickWindow *>(o)) {
                g_inGuard = true;
                const bool alive = guardedDprWalk(qw);
                g_inGuard = false;
                return !alive;   // the window gone: stop the event here
            }
        return false;
    }
};

DprWalkGuard *dprGuard() {
    static DprWalkGuard *g = new DprWalkGuard;   // lives as long as the app (never parented, never deleted)
    return g;
}
#endif

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
    // the guarded walk (above) runs for every Qt Quick window while its DPR change is delivered. Not on the `windows`
    // plugin yet: the live change there is the shipped, crash-free path, and this guard has not run on Windows (a Windows
    // run first; docs/UI-ZOOM.md). TINHAT_UI_SCALE_GUARD=0/1 forces it off/on (gates).
    g_walk = WalkStats();
    const QByteArray forced = qgetenv("TINHAT_UI_SCALE_GUARD");
    const bool guard = forced == "1" || (forced != "0" && QGuiApplication::platformName() != QLatin1String("windows"));
    if (guard)
        for (const QPointer<QWindow> &w : wins)
            if (QQuickWindow *qw = qobject_cast<QQuickWindow *>(w.data())) qw->installEventFilter(dprGuard());
    // each step can run app code synchronously (a handler may close a window): re-check the window after every one
    for (const QPointer<QWindow> &w : wins) {
        if (!w || !w->handle()) continue;   // not created yet: it starts at the new factor
        api().dpr(w);                       // -> the guard's walk, then Qt's (muted) walk, synchronously
        restoreMuted();
        if (!w || !w->handle()) continue;
        const QRect native = w->handle()->geometry();
        api().geo(w, native);
        if (w) w->requestUpdate();
    }
    restoreMuted();
    for (const QPointer<QWindow> &w : wins)
        if (w) w->removeEventFilter(dprGuard());
    // one line when the guard actually saved a walk: in the field it names the trigger (which item types went away)
    if (g_walk.gone > 0)
        qWarning("[ui-scale] guarded DPR walk: %d item(s) deleted during the change, skipped (%s); %d told, %d window(s)",
                 g_walk.gone, g_walk.goneTypes.join(", ").constData(), g_walk.items, g_walk.walks);
#endif
}

}  // namespace

extern "C" {

int tinhat_ui_scale_apply(double factor);

// Before QGuiApplication. `force_restart`: TINHAT_UI_SCALE_RESTART=1 (the size changes only at a start). When the live
// path is missing (or forced off) and the stored size is not 100 %, Qt gets it through QT_SCALE_FACTOR (qputenv: Qt's
// own CRT, so Windows' static-CRT exe can't lose it — see tinhat_force_quick_style). An environment that already sets
// QT_SCALE_FACTOR keeps it, multiplied.
// 2026-11-01 (Restart now re-execs a binary with this process's environment): the QT_SCALE_FACTOR the person's
// environment had is kept in TINHAT_UI_SCALE_ENV_ORIG ("-" = none) the first time we change it, and put back first
// thing at every start, so a size is never multiplied by the previous run's size (0.9 × 0.8 …).
void tinhat_ui_scale_preinit(double factor, int force_restart) {
    g_restartOnly = force_restart != 0;
    if (qEnvironmentVariableIsSet("TINHAT_UI_SCALE_ENV_ORIG")) {
        const QByteArray orig = qgetenv("TINHAT_UI_SCALE_ENV_ORIG");
        if (orig == "-") qunsetenv("QT_SCALE_FACTOR"); else qputenv("QT_SCALE_FACTOR", orig);
    }
    if ((api().ok() && !g_restartOnly) || qFuzzyCompare(factor, 1.0)) return;
    if (!qEnvironmentVariableIsSet("TINHAT_UI_SCALE_ENV_ORIG")) {
        const QByteArray cur = qgetenv("QT_SCALE_FACTOR");
        qputenv("TINHAT_UI_SCALE_ENV_ORIG", cur.isEmpty() ? QByteArray("-") : cur);
    }
    bool ok = false;
    const double env = qEnvironmentVariable("QT_SCALE_FACTOR").toDouble(&ok);
    const double f = (ok && env > 0.0) ? env * factor : factor;
    qputenv("QT_SCALE_FACTOR", QByteArray::number(f, 'g', 6));
}

// The running platform plugin's name ("xcb", "wayland", "windows", …) into buf (NUL-terminated, cut to cap); the
// length written, 0 before QGuiApplication exists. ui_scale.rs decides on it where a live change is allowed
// (2026-11-01: the live change crashed the Flatpak on KDE Plasma Wayland — docs/UI-ZOOM.md "Where it changes live").
int tinhat_ui_scale_platform(char *buf, int cap) {
    if (!buf || cap <= 0) return 0;
    buf[0] = 0;
    if (!qobject_cast<QGuiApplication *>(QCoreApplication::instance())) return 0;
    const QByteArray n = QGuiApplication::platformName().toUtf8();
    const int len = qMin(int(n.size()), cap - 1);
    memcpy(buf, n.constData(), size_t(len));
    buf[len] = 0;
    return len;
}

// 1 = this Qt has the calls to change the size live (the platform check is ui_scale.rs's), 0 = only at the next start.
int tinhat_ui_scale_live_capable(void) { return (api().ok() && !g_restartOnly) ? 1 : 0; }

// Why (for the start-up log line, so a restart-only build never goes unnoticed): 0 live, 1 built without Qt's private
// headers, 2 this Qt lacks one of the three calls, 3 TINHAT_UI_SCALE_RESTART=1.
int tinhat_ui_scale_why(void) {
    if (!TINHAT_UI_SCALE_QPA) return 1;
    if (!api().ok()) return 2;
    if (g_restartOnly) return 3;
    return 0;
}

// The size at start: Qt's factor set right after QGuiApplication, ONLY while no window exists yet (on every platform
// with the calls — the live change of a running app is ui_scale.rs's call). 1 = set, 0 = not (a window already
// exists, or no calls here: the preinit environment carried it).
int tinhat_ui_scale_apply_startup(double factor) {
    if (!QGuiApplication::allWindows().isEmpty()) return 0;
    return tinhat_ui_scale_apply(factor);
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
