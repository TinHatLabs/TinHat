// RigSymbol.qml — mini games v1 (GAMES-V1-SPEC.md §5): one of The Rig's six reel symbols (ring order Hat, Saucer, Dish,
// Eye, Moon, Lizard), drawn, not emoji, so every platform shows the same picture. The paths are the web's
// (tinhat-web src/features/play/RigArt.tsx): a 48 × 48 drawing, stroke 2.2, round caps and joins, the same fill
// opacities; circles and ellipses are written as two SVG arcs. The colour is the shared art's `sym_<symbol>`
// (ui.json `symbol_colours`, Main.qml playArt) unless a caller sets `colour`.
//
// Each element is one or two Shapes (the fill at its opacity, then the stroke), so a fill opacity is the Shape's
// opacity and no colour is made in place.
import QtQuick
import QtQuick.Shapes
import dev.tinhat.app

Item {
    id: sy
    property string sym: ""          // hat | saucer | dish | eye | moon | lizard
    property real size: 48           // the drawn size in px (the 48-unit drawing is scaled to it)
    property var art: null           // RigTable's art (Main.qml playArt): the default colour
    property string label: ""        // the accessible name (the symbol's word)
    property color colour: sy.art && sy.art["sym_" + sy.sym] ? sy.art["sym_" + sy.sym] : Th.textSecondary

    implicitWidth: sy.size
    implicitHeight: sy.size
    Accessible.role: Accessible.Graphic
    Accessible.name: sy.label !== "" ? sy.label : sy.sym

    // d = the SVG path; fill = the fill opacity (0 = no fill); stroke = drawn with the 2.2 stroke; op = the element's
    // own opacity (the web's opacity="0.6" groups)
    readonly property var parts: ({
        hat: [
            { d: "M 24 7 L 37 33 H 11 Z", fill: 0.18, stroke: true, op: 1 },
            { d: "M 6 35 Q 24 41 42 35", fill: 0, stroke: true, op: 1 },
            { d: "M 18 22 L 24 12 L 30 22", fill: 0, stroke: true, op: 0.6 }
        ],
        saucer: [
            { d: "M 6 27 A 18 6.5 0 1 0 42 27 A 18 6.5 0 1 0 6 27 Z", fill: 0.18, stroke: true, op: 1 },
            { d: "M 14 24 Q 16 13 24 13 Q 32 13 34 24", fill: 0, stroke: true, op: 1 },
            { d: "M 17 38 L 14 43 M 24 38 V 44 M 31 38 L 34 43", fill: 0, stroke: true, op: 0.6 }
        ],
        dish: [
            { d: "M 9 14 Q 14 34 34 33 Z", fill: 0.18, stroke: true, op: 1 },
            { d: "M 21 24 L 33 12", fill: 0, stroke: true, op: 1 },
            { d: "M 32 11 A 2.5 2.5 0 1 0 37 11 A 2.5 2.5 0 1 0 32 11 Z", fill: 1, stroke: true, op: 1 },
            { d: "M 22 33 L 18 42 H 32 L 28 34", fill: 0, stroke: true, op: 1 }
        ],
        eye: [
            { d: "M 5 24 Q 24 6 43 24 Q 24 42 5 24 Z", fill: 0.14, stroke: true, op: 1 },
            { d: "M 17 24 A 7 7 0 1 0 31 24 A 7 7 0 1 0 17 24 Z", fill: 0, stroke: true, op: 1 },
            { d: "M 21.2 24 A 2.8 2.8 0 1 0 26.8 24 A 2.8 2.8 0 1 0 21.2 24 Z", fill: 1, stroke: true, op: 1 }
        ],
        moon: [
            { d: "M 31 8 A 16 16 0 1 0 40 33 A 13 13 0 1 1 31 8 Z", fill: 0.22, stroke: true, op: 1 },
            { d: "M 18.4 22 A 1.6 1.6 0 1 0 21.6 22 A 1.6 1.6 0 1 0 18.4 22 Z", fill: 1, stroke: false, op: 1 },
            { d: "M 22.8 31 A 1.2 1.2 0 1 0 25.2 31 A 1.2 1.2 0 1 0 22.8 31 Z", fill: 1, stroke: false, op: 1 }
        ],
        lizard: [
            { d: "M 10 30 Q 16 20 26 22 Q 34 23 38 15", fill: 0, stroke: true, op: 1 },
            { d: "M 17 23.5 A 8 4.5 0 1 0 33 23.5 A 8 4.5 0 1 0 17 23.5 Z", fill: 0.2, stroke: true, op: 1 },
            { d: "M 36.5 13.5 A 3 3 0 1 0 42.5 13.5 A 3 3 0 1 0 36.5 13.5 Z", fill: 0.3, stroke: true, op: 1 },
            { d: "M 20 26 L 17 32 M 28 27 L 31 32 M 21 21 L 18 16 M 29 21 L 32 17 M 10 30 Q 6 34 8 39", fill: 0, stroke: true, op: 1 }
        ]
    })

    Item {
        id: drawing
        width: 48
        height: 48
        scale: sy.size / 48
        transformOrigin: Item.TopLeft
        Repeater {
            model: sy.parts[sy.sym] ? sy.parts[sy.sym] : []
            Item {
                id: part
                required property var modelData
                width: 48
                height: 48
                opacity: part.modelData.op
                Shape {
                    visible: part.modelData.fill > 0
                    anchors.fill: parent
                    opacity: part.modelData.fill
                    preferredRendererType: Shape.CurveRenderer
                    ShapePath {
                        strokeWidth: 0
                        strokeColor: "transparent"
                        fillColor: sy.colour
                        PathSvg { path: part.modelData.d }
                    }
                }
                Shape {
                    visible: part.modelData.stroke
                    anchors.fill: parent
                    preferredRendererType: Shape.CurveRenderer
                    ShapePath {
                        strokeWidth: 2.2
                        strokeColor: sy.colour
                        fillColor: "transparent"
                        capStyle: ShapePath.RoundCap
                        joinStyle: ShapePath.RoundJoin
                        PathSvg { path: part.modelData.d }
                    }
                }
            }
        }
    }
}
