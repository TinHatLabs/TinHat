// WidgetSparkline.qml — the Call stats sparkline (WIDGETS-UI.md §5): the incoming kbps of the last 60 samples in a
// w × h box — points from crates/widgets quality::spark_points (WidgetsController.sparkPath, centi-pixels / 100), a
// 1.5 px line `widget.stats.spark.edge` and the area under it `widget.stats.spark.fill`. `sparkStep`: on a new sample
// the line is drawn with the new point and slides left by one step (x + step → x, long 200 ms, linear), only while the
// host animates.
import QtQuick
import QtQuick.Shapes
import dev.tinhat.app   // Th: the active theme (docs/MIGRATING-THEME.md)

Item {
    id: sp
    property var values: []                   // numbers, oldest first
    property var ctl: null                    // WidgetsController (sparkPath)
    property int seq: 0                       // the sample counter: a step per change
    property int dur: 0                       // the host's `long` step (0 = snap)
    implicitWidth: 48
    implicitHeight: 26
    clip: true
    readonly property real step: (width - 2) / 59
    property var pts: []
    function refresh() {
        var s = sp.ctl ? String(sp.ctl.sparkPath(JSON.stringify(sp.values || []), sp.width, sp.height)) : ""
        var out = []
        if (s.length > 0) {
            var parts = s.split(" ")
            for (var i = 0; i < parts.length; ++i) {
                var xy = parts[i].split(",")
                out.push(Qt.point(parseFloat(xy[0]), parseFloat(xy[1])))
            }
        }
        sp.pts = out
        if (sp.dur > 0 && out.length > 1) { slide.stop(); shape.x = sp.step; slide.start() } else shape.x = 0
    }
    onSeqChanged: refresh()
    onWidthChanged: refresh()
    Component.onCompleted: refresh()
    NumberAnimation { id: slide; target: shape; property: "x"; to: 0; duration: sp.dur; easing.type: Easing.Linear }
    Shape {
        id: shape
        width: sp.width; height: sp.height
        preferredRendererType: Shape.CurveRenderer
        visible: sp.pts.length >= 2   // one point is no line (the web draws nothing)
        ShapePath {   // the area under the line
            strokeColor: "transparent"
            fillColor: Th.widgetStatsSparkFill
            PathPolyline {
                path: sp.pts.length > 0
                    ? [Qt.point(sp.pts[0].x, sp.height - 1)].concat(sp.pts).concat([Qt.point(sp.pts[sp.pts.length - 1].x, sp.height - 1)])
                    : []
            }
        }
        ShapePath {
            strokeColor: Th.widgetStatsSparkEdge
            strokeWidth: 1.5
            fillColor: "transparent"
            joinStyle: ShapePath.RoundJoin
            capStyle: ShapePath.RoundCap
            PathPolyline { path: sp.pts }
        }
    }
}
