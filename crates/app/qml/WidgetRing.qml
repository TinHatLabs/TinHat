// WidgetRing.qml — the Up next countdown ring (WIDGETS-UI.md §4): a track circle and a sweep from 12 o'clock clockwise,
// sweep = fill / 1440 × 360° (crates/widgets time::ring_of via WidgetsController.ring). Round caps. `ringStep`: the sweep
// eases to the new minute (long 200 ms, OutCubic) only while the host animates; otherwise it snaps.
import QtQuick
import QtQuick.Shapes
import dev.tinhat.app   // Th: the active theme (docs/MIGRATING-THEME.md)

Item {
    id: ring
    property int diameter: 40
    property real stroke: 3
    property real fill: 0                     // 0 … 1440 minutes
    property bool live: false
    property int dur: 0                       // the host's `long` step (0 = snap)
    implicitWidth: diameter
    implicitHeight: diameter
    readonly property real sweep: Math.max(0, Math.min(1440, ring.fill)) / 1440 * 360
    property real shownSweep: sweep
    Behavior on shownSweep { enabled: ring.dur > 0; NumberAnimation { duration: ring.dur; easing.type: Easing.OutCubic } }
    Shape {
        anchors.fill: parent
        preferredRendererType: Shape.CurveRenderer
        ShapePath {
            strokeColor: Th.widgetUpNextRingTrack
            strokeWidth: ring.stroke
            fillColor: "transparent"
            capStyle: ShapePath.RoundCap
            PathAngleArc {
                centerX: ring.diameter / 2; centerY: ring.diameter / 2
                radiusX: (ring.diameter - ring.stroke) / 2; radiusY: (ring.diameter - ring.stroke) / 2
                startAngle: 0; sweepAngle: 360
            }
        }
        ShapePath {
            strokeColor: ring.shownSweep > 0.01 ? (ring.live ? Th.widgetUpNextRingFillLive : Th.widgetUpNextRingFill) : "transparent"
            strokeWidth: ring.stroke
            fillColor: "transparent"
            capStyle: ShapePath.RoundCap
            PathAngleArc {
                centerX: ring.diameter / 2; centerY: ring.diameter / 2
                radiusX: (ring.diameter - ring.stroke) / 2; radiusY: (ring.diameter - ring.stroke) / 2
                startAngle: -90; sweepAngle: Math.min(359.99, ring.shownSweep)
            }
        }
    }
}
