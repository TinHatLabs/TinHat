// LevelMeter.qml — the house level meter (was Main.qml's inline component; moved out for the Settings window, which
// is its own document — SETTINGS-WINDOW.md): 16 bars driven by a 0..1 `level`, perceptual scaling + a per-bar ripple so
// normal speech visibly animates. Used by Settings → Audio (input / output level) and the noise-cancellation test.
// 11.0 (GPU.md §2.7, R13 — the web's ui/meter.ts): the bars change only on the meter's own ≤ 10/s tick, with the
// level SAMPLED at the tick (the live level's rate draws nothing new); eased by half the gap while the motion switch
// is on (tinhat_widgets::lowgpu::wave_ease), snapped under Reduce motion; the flat rest pose and NO tick while the
// window is not focused or all is quiet (it ran a 33 ms tick for as long as the bars moved).
import QtQuick
import dev.tinhat.app   // Motion

Row {
    id: meter
    property real level: 0.0
    property color barColor   // every instance sets it (settings meter fill)
    property real shown: 0.0     // the eased amplitude
    property real sampled: 0.0   // the level at the last tick (0 = the rest pose)
    readonly property bool active: meter.sampled > 0.02
    spacing: 3
    function amp(l) { return l > 0.02 ? Math.pow(Math.min(1.0, (l - 0.02) / 0.48), 0.6) : 0 }
    function rest() { meterTick.stop(); meter.shown = 0; meter.sampled = 0 }
    function tick() {
        if (!Motion.focused || !meter.visible) { meter.rest(); return }
        var target = meter.amp(meter.level), d = target - meter.shown
        meter.shown = Motion.animate ? (Math.abs(d) < 0.015 ? target : meter.shown + d * 0.5) : target
        meter.sampled = meter.level
        if (meter.shown === 0 && meter.level <= 0.02) meterTick.stop()
    }
    function wake() {
        if (meterTick.running) return
        if (!Motion.focused || !meter.visible) { meter.rest(); return }
        if (meter.level <= 0.02 && meter.shown === 0) { meter.rest(); return }
        meter.tick()
        if (!(meter.shown === 0 && meter.level <= 0.02)) meterTick.start()
    }
    onLevelChanged: meter.wake()
    onVisibleChanged: meter.wake()
    Connections {
        target: Motion
        function onWinStateChanged() { if (Motion.focused) meter.wake(); else meter.rest() }
        function onAnimateChanged() { meter.wake() }
    }
    Timer { id: meterTick; interval: 100; repeat: true; onTriggered: meter.tick() }   // WAVE_TICK_MS
    Repeater {
        model: 16
        delegate: Rectangle {
            required property int index
            width: 4; radius: 2
            anchors.verticalCenter: parent.verticalCenter
            color: meter.barColor
            opacity: meter.active ? 0.95 : 0.3
            readonly property real bell: 1.0 - Math.abs(index - 7.5) * 0.07
            readonly property real ripple: 0.6 + 0.4 * Math.abs(Math.sin(index * 1.1 + meter.sampled * 22.0))
            height: 3 + meter.shown * bell * ripple * 33
        }
    }
}
