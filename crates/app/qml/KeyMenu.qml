// KeyMenu — a Menu that takes the keyboard when the keyboard opened it (accessibility round 2, m11). Opened with
// Space / Return on a button that had the KEYBOARD focus (visualFocus: Tab, Backtab, a shortcut), the menu showed but
// took the keys only at the first ↓, and Escape handed the focus to the window's focus chain (the composer), not back
// to the button. Now: the menu has the focus at once with its first row highlighted, and when it closes the focus goes
// back to the button (with a keyboard focus reason: its ring shows and a second Space opens it the same way) — unless
// the row that was chosen put the focus somewhere on purpose (a dialog, a field), and never out of another popup.
// Opened by the pointer, nothing changes (the opener had no visual focus). Known limit: "opened by the keyboard" is
// read from the opener's visual focus — a button focused by a click and then pressed with Space opens as by pointer. The web's Menu does the same (src/ui/Menu.tsx, round 1). Submenus stay
// plain Controls.Menu: Qt hands the keys between a menu and its submenu itself.
import QtQuick
import QtQuick.Controls as Controls

Controls.Menu {
    id: km
    property Item _kbOpener: null
    property bool _focusMoved: false
    function _inside(it) {
        for (var p = it; p; p = p.parent) if (p === km.contentItem || (km.contentItem && p === km.contentItem.parent)) return true
        return false
    }
    function _firstEnabled() {
        for (var i = 0; i < km.count; i++) {
            var it = km.itemAt(i)
            if (it && it.enabled && it.visible && (it.highlighted !== undefined)) return i
        }
        return -1
    }
    // connected, not onX handlers: an instance's own onOpened / onClosed (the message menu's) must keep working beside
    // these. (A Popup's `opened` is also a property: its change is followed, not the signal.)
    function _kbShow() {
        var a = km.parent ? km.parent.Window.activeFocusItem : null
        km._kbOpener = (a && a.visualFocus === true) ? a : null
        km._focusMoved = false
        // a keyboard open takes the focus itself (_kbOpened): with the popup's own `focus` off, Qt's teardown does not
        // drop the menu's focus BEFORE aboutToHide (that move looked like an action's). Set on EVERY opening (a reopen
        // that interrupts the exit never sees `closed`); a pointer opening gets the instance's own setting back.
        km.focus = km._kbOpener ? false : km._focusPref
    }
    property bool _focusPref: true   // the instance's own `focus` (read once; no KeyMenu binds it)
    // a submenu closing hands the focus to the popups whose `focus` is on — not this one while a keyboard open is up:
    // take it back while still open
    property var _kbSubs: []
    function _kbSubClosed() { if (km.visible && km._kbOpener) km.forceActiveFocus(Qt.PopupFocusReason) }
    // While the menu is open, a focus change to a real item outside it is the chosen row's action moving the focus on
    // purpose (a dialog, a field: the row's own onTriggered runs before Qt dismisses the menu). Qt hands the focus back
    // only after aboutToHide (no longer watched; the popup's `focus` is off while a keyboard open is up, see _kbShow).
    property var _kbWin: null
    property bool _kbWatching: false
    function _kbFocusChanged() {
        if (!km._kbWatching || !km._kbWin) return
        var a = km._kbWin.activeFocusItem
        if (!a || a === km._kbWin.contentItem || km._inside(a) || a === km._kbOpener) return
        km._focusMoved = true
    }
    function _inPopup(it) {   // inside another popup (a dialog, a sheet) — never pulled out of it
        for (var p = it; p; p = p.parent) if (String(p).indexOf("QQuickPopupItem") === 0) return true
        return false
    }
    function _kbOpened() {
        if (!km._kbOpener) return
        km.forceActiveFocus(Qt.PopupFocusReason)
        km.currentIndex = km._firstEnabled()
        km._kbWin = km._kbOpener.Window.window
        if (km._kbWin) { km._kbWin.activeFocusItemChanged.connect(km._kbFocusChanged); km._kbWatching = true }
        var subs = []
        for (var i = 0; i < km.count; i++) {
            var it = km.itemAt(i)
            if (it && it.subMenu) { it.subMenu.closed.connect(km._kbSubClosed); subs.push(it.subMenu) }
        }
        km._kbSubs = subs
    }
    function _kbHide() { km._kbWatching = false }
    function _kbClosed() {
        km.focus = km._focusPref
        for (var i = 0; i < km._kbSubs.length; i++) if (km._kbSubs[i]) km._kbSubs[i].closed.disconnect(km._kbSubClosed)
        km._kbSubs = []
        if (km._kbWin) km._kbWin.activeFocusItemChanged.disconnect(km._kbFocusChanged)
        km._kbWin = null; km._kbWatching = false
        var o = km._kbOpener, moved = km._focusMoved
        km._kbOpener = null; km._focusMoved = false
        if (!o || moved) return
        Qt.callLater(function () {   // after Qt's own hand-back to the focus chain
            var a = o.Window.activeFocusItem
            if (a && a !== o && km._inPopup(a)) return
            if (o.visible && o.enabled) o.forceActiveFocus(Qt.ShortcutFocusReason)
        })
    }
    Component.onCompleted: {
        km._focusPref = km.focus
        km.aboutToShow.connect(km._kbShow)
        km.openedChanged.connect(function () { if (km.opened) km._kbOpened() })
        km.aboutToHide.connect(km._kbHide)
        km.closed.connect(km._kbClosed)
    }
}
