// CornerMask.qml — rounded corners for a surface that cannot be masked (0.8.30).
//
// Item.clip only ever clips to the RECTANGLE, so a video surface (the GStreamer GL item /
// QtMultimedia VideoOutput the call tiles attach, the mpv surfaces, PipVideoItem) or a
// square-clipped Image painted its corners straight over the tile's rounded border —
// content showed outside the bevelled border (a field report). A true mask (layer.enabled +
// MultiEffect) is an off-screen pass per tile per frame and is unusable under the software
// safe-mode renderer; this is four filled corner bites in the colour BEHIND the container,
// painted above the surface and below the frame border. Costs nothing, works on every
// backend. `color` must be what surrounds the container (pal.bg / pal.panel).
import QtQuick
import QtQuick.Shapes

Shape {
    id: mask
    property real radius: 8
    property color color   // every instance passes the surface behind the container (the dead #000000 default is gone)
    // 11.0: a surface whose backdrop changes top to bottom (my own chat bubble's gradient) passes the colour at the top
    // and at the bottom edge; each pair of bites takes its own (default: `color`)
    property color topColor: mask.color
    property color bottomColor: mask.color
    anchors.fill: parent
    // Curve renderer = anti-aliased arcs without MSAA (Qt 6.6+; both platforms ship 6.8+).
    preferredRendererType: Shape.CurveRenderer
    readonly property real r: Math.max(0, Math.min(radius, width / 2, height / 2))
    visible: r > 0.5

    // Each bite: corner point → along one edge to the arc start → the quarter arc (bending
    // INTO the corner, i.e. around the rounded rect's corner centre) → back along the other edge.
    ShapePath { // top-left
        strokeWidth: -1; fillColor: mask.topColor
        startX: 0; startY: 0
        PathLine { x: mask.r; y: 0 }
        PathArc { x: 0; y: mask.r; radiusX: mask.r; radiusY: mask.r; direction: PathArc.Counterclockwise }
        PathLine { x: 0; y: 0 }
    }
    ShapePath { // top-right
        strokeWidth: -1; fillColor: mask.topColor
        startX: mask.width; startY: 0
        PathLine { x: mask.width; y: mask.r }
        PathArc { x: mask.width - mask.r; y: 0; radiusX: mask.r; radiusY: mask.r; direction: PathArc.Counterclockwise }
        PathLine { x: mask.width; y: 0 }
    }
    ShapePath { // bottom-right
        strokeWidth: -1; fillColor: mask.bottomColor
        startX: mask.width; startY: mask.height
        PathLine { x: mask.width - mask.r; y: mask.height }
        PathArc { x: mask.width; y: mask.height - mask.r; radiusX: mask.r; radiusY: mask.r; direction: PathArc.Counterclockwise }
        PathLine { x: mask.width; y: mask.height }
    }
    ShapePath { // bottom-left
        strokeWidth: -1; fillColor: mask.bottomColor
        startX: 0; startY: mask.height
        PathLine { x: 0; y: mask.height - mask.r }
        PathArc { x: mask.r; y: mask.height; radiusX: mask.r; radiusY: mask.r; direction: PathArc.Counterclockwise }
        PathLine { x: 0; y: mask.height }
    }
}
