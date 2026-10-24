// WidgetBusyMark.qml — the house busy mark (GPU.md §2.5, the web's ui/BusyIndicator): ten circles on a ring, radius
// size/12, outlined — and STILL. It advances one step (dot i filled iff i ≤ k and (k − i) mod 20 < 10) per REAL
// progress event (a changing `progress`), at most one per 250 ms and only while `animate`; no events = no motion.
import QtQuick
import dev.tinhat.app   // Th: the active theme (docs/MIGRATING-THEME.md)

Item {
    id: mark
    property int size: 16
    property var progress: undefined
    property bool animate: false
    property color color: Th.widgetMutedFg
    property int step: -1
    property var _last: undefined
    property double _lastAt: 0
    implicitWidth: size
    implicitHeight: size
    onProgressChanged: {
        if (mark.progress === undefined || mark.progress === mark._last) return
        mark._last = mark.progress
        if (!mark.animate) return
        var now = Date.now()
        if (now - mark._lastAt < 250) return
        mark._lastAt = now
        mark.step = mark.step + 1
    }
    readonly property int r: Math.max(1, Math.floor(size / 12))
    Repeater {
        model: 10
        delegate: Rectangle {
            required property int index
            readonly property real a: (index / 10 - 0.25) * 2 * Math.PI
            x: mark.size / 2 + (mark.size / 2 - mark.r) * Math.cos(a) - mark.r
            y: mark.size / 2 + (mark.size / 2 - mark.r) * Math.sin(a) - mark.r
            width: mark.r * 2; height: mark.r * 2; radius: mark.r
            border.width: 1
            border.color: mark.color
            color: (index <= mark.step && ((mark.step - index) % 20) < 10) ? mark.color : "transparent"
        }
    }
}
