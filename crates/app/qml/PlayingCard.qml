// PlayingCard.qml — mini games v1 (GAMES-V1-SPEC.md §4): one playing card, drawn as Tin Hat Web's PlayingCard (bits.tsx +
// bits.module.css: the same four sizes, corner rank + small suit, a big pip bottom-right, the card art colours of
// ui.json). `card` = the wire's two letters ("Td"), "" = face down (the back); `ghost` = an empty board slot.
import QtQuick
import QtQuick.Controls as Controls
import dev.tinhat.app

Rectangle {
    id: pc
    property var play: null
    property string card: ""
    property bool ghost: false
    property bool dim: false
    property string size: "md"        // sm 26×36 · md 40×56 · lg 56×78 · xl 92×128 (× the text size, as the web)
    property var art: ({})            // ui.json art (QML colours), from the table
    // compatibility with older callers
    property bool slot: false
    readonly property bool isGhost: pc.ghost || pc.slot
    readonly property real k: Th.textScale > 0 ? Th.textScale : 1
    readonly property var dims: ({ sm: [26, 36, 4, 11, 8, 13], md: [40, 56, 6, 15, 11, 22], lg: [56, 78, 8, 20, 14, 32], xl: [92, 128, 10, 32, 22, 56] })
    readonly property var d: pc.dims[pc.size] || pc.dims.md
    implicitWidth: Math.round(pc.d[0] * pc.k)
    implicitHeight: Math.round(pc.d[1] * pc.k)
    width: implicitWidth
    height: implicitHeight
    radius: pc.d[2]
    readonly property var face: {
        if (!pc.card || !pc.play || pc.isGhost) return null
        var s = pc.play.cardFace(pc.card)
        return s ? JSON.parse(s) : null
    }
    readonly property color ink: pc.face && pc.face.red ? (pc.art.cardRed || Th.statusErrorFg) : (pc.art.cardBlack || Th.widgetDiceFaceFg)
    color: pc.isGhost ? "transparent" : (pc.face ? (pc.art.cardFace || Th.widgetDiceFaceBg) : (pc.art.cardBack || Th.accentDim))
    border.width: pc.isGhost ? 1 : 0
    border.color: pc.art.feltLine || Th.borderSubtle
    opacity: pc.dim ? 0.45 : 1
    Accessible.role: Accessible.StaticText
    Accessible.name: pc.face ? (pc.face.rank + " " + pc.face.suit) : (pc.isGhost ? "" : "A face-down card")

    // the edge + soft shadow under a real card (the web's box-shadow)
    Rectangle {
        visible: !pc.isGhost
        z: -1
        x: 0; y: 1
        width: pc.width; height: pc.height
        radius: pc.radius
        color: pc.art.cardEdge || Th.shadow
    }
    // the back: an inset frame with a diagonal lattice
    Rectangle {
        visible: !pc.face && !pc.isGhost
        anchors.fill: parent
        anchors.margins: 3
        radius: Math.max(2, pc.radius - 2)
        color: "transparent"
        border.width: 1
        border.color: pc.art.cardBackLine || Th.accentBright
        clip: true
        Repeater {
            model: 8
            Rectangle {
                required property int index
                width: 1
                height: pc.height * 2
                x: (index - 1) * pc.width * 0.2
                y: -pc.height * 0.5
                rotation: 35
                color: pc.art.cardBackLine || Th.accentBase
                opacity: 0.5
            }
        }
    }
    // the face: corner rank + small suit, the big pip
    Column {
        visible: !!pc.face
        x: pc.size === "xl" ? 8 : (pc.size === "sm" ? 3 : 5)
        y: pc.size === "xl" ? 8 : (pc.size === "sm" ? 3 : 4)
        spacing: -2
        Controls.Label {
            text: pc.face ? pc.face.rank : ""
            textFormat: Text.PlainText
            color: pc.ink
            font.bold: true
            font.pixelSize: Math.round(pc.d[3] * pc.k)
        }
        Controls.Label {
            text: pc.face ? pc.face.suit : ""
            textFormat: Text.PlainText
            color: pc.ink
            font.pixelSize: Math.round(pc.d[4] * pc.k)
        }
    }
    Controls.Label {
        visible: !!pc.face
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        anchors.rightMargin: pc.size === "xl" ? 10 : (pc.size === "sm" ? 3 : 7)
        anchors.bottomMargin: pc.size === "xl" ? 8 : (pc.size === "sm" ? 2 : 5)
        text: pc.face ? pc.face.suit : ""
        textFormat: Text.PlainText
        color: pc.ink
        font.pixelSize: Math.round(pc.d[5] * pc.k)
    }
}
