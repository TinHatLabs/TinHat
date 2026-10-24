// ThBusyIndicator.qml — the house busy mark (11.0, GPU.md §2.5, R10; the web's ui/BusyIndicator.tsx): Controls.
// BusyIndicator's ten circles on a ring, radius size/12, outlined in the pen colour — but STILL. The stock indicator
// spun at the display's refresh rate for as long as anything loaded (reconnecting, a stalled image, waiting on the
// other device — sometimes for minutes); the low-GPU contract allows no loop. The mark rests as ten outlined circles
// and advances one step (the next dot of the stock fill/empty order: dot i filled iff i ≤ k and (k − i) mod 20 < 10 —
// tinhat_widgets::lowgpu::busy_dot_filled) per REAL progress event — a stage change, a received chunk, a retry —
// passed as a changing `progress`, at most one per 250 ms and only while the motion switch is on (a change while it is
// off is dropped, not queued: no catch-up burst on focus). With no events it stays still.
// `label` + `shownLabel`: the caption next to it changes ONCE after 10 s without progress ("Loading…" → "Still
// loading…"; the right panel's 12 s cap, generalised) — one timeout, no ticking.
import QtQuick
import dev.tinhat.app   // Th: the active theme · Motion: the app-wide motion switch

Item {
    id: mark
    property int size: 36
    property bool running: true
    property color color: Th.loaderBusyFill   // the stock (Basic) ink; call sites that want a visible mark pass one
    property var progress: undefined           // changes on every real progress event
    property string label: ""
    readonly property string shownLabel: (mark._still && mark.running) ? mark.stillLabel(mark.label) : mark.label
    property int step: -1                      // -1 = at rest (all ten outlined)
    property var _last: undefined
    property double _lastAt: -1e9
    property bool _still: false
    implicitWidth: mark.size
    implicitHeight: mark.size
    opacity: mark.running ? 1 : 0
    Behavior on opacity { NumberAnimation { duration: Motion.durLong } }
    Accessible.role: Accessible.ProgressBar
    Accessible.name: "Loading"

    // lowgpu.ts stillLabel (same code): a label already starting with the word "Still" stays
    function stillLabel(l) {
        if (!l) return l
        if (/^still\b/i.test(l)) return l
        return "Still " + l.charAt(0).toLowerCase() + l.slice(1)
    }
    onProgressChanged: {
        mark._still = false
        if (mark.running) stillTimer.restart()
        if (mark.progress === undefined || mark.progress === mark._last) return
        mark._last = mark.progress
        if (!Motion.animate) return
        var now = Date.now()
        if (now - mark._lastAt < 250) return          // BUSY_STEP_MS: ≤ 4 steps/s
        mark._lastAt = now
        mark.step = mark.step + 1
    }
    onRunningChanged: {
        mark._still = false
        if (mark.running) stillTimer.restart(); else stillTimer.stop()
    }
    Component.onCompleted: if (mark.running) stillTimer.start()
    Timer { id: stillTimer; interval: 10000; onTriggered: mark._still = true }   // BUSY_STILL_MS

    readonly property int r: Math.max(1, Math.floor(mark.size / 12))
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
            color: (mark.step >= 0 && index <= mark.step && ((mark.step - index) % 20) < 10) ? mark.color : "transparent"
        }
    }
}
