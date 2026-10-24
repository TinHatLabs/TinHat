// WindowMemory.qml — 10.1 (TH-SHELL-05 / TH-CAL-22 / TH-MEM-08 / TH-POP-09): remember a Window's
// size across runs — and its position where the platform lets an app choose one (X11, Windows;
// Wayland never does) — under one `key`. Restore happens once, at creation, before the window is
// shown; saves are debounced. A maximised window is remembered as "maximised", not as a size, so
// un-maximising still returns to the last windowed size. Nothing here touches Rust: it rides on
// the 9.3.1 ui-pref flag files (`settings.applyUiPref` / `uiPref`), which the host passes in as
// `store` so this file stays a plain component.
import QtQuick
import QtQuick.Window

Item {
    id: memo
    visible: false
    required property Window target
    required property string key       // [a-z0-9-] — the pref prefix
    required property var store        // the settings controller (uiPref / applyUiPref); null = off
    property bool remembered: false
    // 11.3 (Media Party QOL): while true nothing is saved — the host holds it while its window shows a temporary geometry
    // (the Media Party mini player), so that geometry never becomes the remembered one.
    property bool hold: false
    readonly property bool canPlace: Qt.platform.pluginName.indexOf("wayland") !== 0
    // The host applies this itself when it shows the window (showing a hidden window as
    // Maximized here would make it visible before the boot reveal).
    readonly property bool wantMaximized: store ? store.uiPref(key + "-max") === "1" : false

    function restore() {
        if (!store) return
        var w = parseInt(store.uiPref(key + "-w")), h = parseInt(store.uiPref(key + "-h"))
        if (!isNaN(w) && !isNaN(h) && w >= target.minimumWidth && h >= target.minimumHeight && w < 16000 && h < 16000) {
            target.width = w; target.height = h
        }
        if (canPlace) {
            var x = parseInt(store.uiPref(key + "-x")), y = parseInt(store.uiPref(key + "-y"))
            if (!isNaN(x) && !isNaN(y) && x > -4000 && y > -4000 && x < 16000 && y < 16000) { target.x = x; target.y = y }
        }
        remembered = true
    }
    function save() {
        if (!store || !remembered || hold) return
        var vis = target.visibility
        if (vis === Window.Maximized) { store.applyUiPref(key + "-max", "1"); return }
        if (vis !== Window.Windowed) return          // hidden / minimised / fullscreen: nothing to learn
        store.applyUiPref(key + "-max", "0")
        if (target.width < target.minimumWidth || target.height < target.minimumHeight) return
        store.applyUiPref(key + "-w", String(Math.round(target.width)))
        store.applyUiPref(key + "-h", String(Math.round(target.height)))
        if (canPlace) {
            store.applyUiPref(key + "-x", String(Math.round(target.x)))
            store.applyUiPref(key + "-y", String(Math.round(target.y)))
        }
    }
    Timer { id: saveTimer; interval: 900; onTriggered: memo.save() }
    Connections {
        target: memo.target
        function onWidthChanged() { saveTimer.restart() }
        function onHeightChanged() { saveTimer.restart() }
        function onXChanged() { saveTimer.restart() }
        function onYChanged() { saveTimer.restart() }
        function onVisibilityChanged() { saveTimer.restart() }
    }
    Component.onCompleted: restore()
}
