// PrestigeToken.qml — the small tin-hat token to the left of a name (mini games v1, GAMES-V1-SPEC §3.1). The web's
// features/prestige/PrestigeToken.tsx draws the same: one pack glyph (prestige-symbolic / -filled / -ring / -star)
// tinted with the prestige.* theme tokens, 14 px beside 15 px text (12 px in widgets), 4 px before the name, centred on
// the text. The glyph is the pack's SVG tinted before decode (Prestige.tinted), not Kirigami.Icon's recolour, which
// paints white on Windows and with the software scene graph. `roomId` = the surface's context: the room the surface is
// about, or a space's own id on its own surfaces; "" = no token (rev 2 §3.3). Rank 13+: a one-time shine (400 ms: the glyph flashes to prestige.shine.highlight and back with a 1.15×
// pop) the first time that person's token appears while the window is focused and motion is on; never looping.
// Rank 0 / no games on that server: invisible and 0 wide (rows and layouts skip it).
//
//   PrestigeToken { mxid: m; roomId: r }            before an existing name label (the 4 px gap is included)
//   PrestigeToken { mxid: m; roomId: r; size: 12 }  widgets
import QtQuick
import QtQuick.Window
import QtQuick.Controls as Controls
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th (prestige.* tokens), Prestige, Motion

Item {
    id: tok
    property string mxid: ""
    property string roomId: ""
    property int size: 14
    property bool gap: true   // the 4 px between the token and the name

    readonly property var look: Prestige.lookOf(tok.mxid, tok.roomId)   // roomId: the surface's context id
    readonly property int rank: tok.look ? tok.look.r : 0
    readonly property color ink: !tok.look ? "transparent"
        : (tok.look.c === "muted" ? Th.prestigeOutlineFg : (tok.look.c === "text" ? Th.prestigeFilledFg : Th.prestigeAccentFg))

    visible: tok.rank > 0
    implicitWidth: tok.visible ? tok.size + (tok.gap ? 4 : 0) : 0
    implicitHeight: tok.visible ? tok.size : 0
    Layout.preferredWidth: tok.implicitWidth
    Layout.preferredHeight: tok.implicitHeight
    Layout.alignment: Qt.AlignVCenter

    Accessible.role: Accessible.Graphic
    Accessible.name: Prestige.label(tok.rank)

    // what this label asked for (want / forget are refcounted per label)
    property string _askedMxid: ""
    property string _askedCtx: ""
    function _ask() {
        if (tok._askedMxid === tok.mxid && tok._askedCtx === tok.roomId) return
        Prestige.forget(tok._askedMxid, tok._askedCtx)
        tok._askedMxid = tok.mxid
        tok._askedCtx = tok.roomId
        Prestige.want(tok.mxid, tok.roomId)
    }
    Component.onCompleted: tok._ask()
    Component.onDestruction: Prestige.forget(tok._askedMxid, tok._askedCtx)
    onMxidChanged: tok._ask()
    onRoomIdChanged: tok._ask()

    Item {
        id: glyphBox
        width: tok.size
        height: tok.size
        anchors.verticalCenter: parent.verticalCenter
        x: 0
        Image {
            anchors.fill: parent
            sourceSize: Qt.size(Math.ceil(tok.size * Screen.devicePixelRatio), Math.ceil(tok.size * Screen.devicePixelRatio))
            source: tok.look ? Prestige.tinted(tok.look.g, tok.ink) : ""
            opacity: tok.ink.a
            smooth: true
            cache: true
        }
        Image {
            id: shineGlyph
            anchors.fill: parent
            sourceSize: Qt.size(Math.ceil(tok.size * Screen.devicePixelRatio), Math.ceil(tok.size * Screen.devicePixelRatio))
            source: tok.look && tok.look.s ? Prestige.tinted(tok.look.g, Th.prestigeShineHighlight) : ""
            opacity: 0
            visible: opacity > 0
            smooth: true
        }
        HoverHandler { id: hov }
        Controls.ToolTip.text: Prestige.label(tok.rank)
        Controls.ToolTip.visible: hov.hovered && tok.rank > 0
        Controls.ToolTip.delay: Kirigami.Units.toolTipDelay
    }

    // the one-time shine (§3.1): only for a look that has it, focused + motion on, once per person per run
    readonly property bool canShine: !!tok.look && tok.look.s === true && Motion.focused && Motion.animate
    function maybeShine() {
        if (tok.canShine && tok.visible && Prestige.claimShine(tok.mxid)) shineAnim.start()
    }
    onCanShineChanged: maybeShine()
    onVisibleChanged: maybeShine()
    ParallelAnimation {
        id: shineAnim
        SequentialAnimation {
            NumberAnimation { target: shineGlyph; property: "opacity"; from: 0; to: 1; duration: 160; easing.type: Easing.InOutQuad }
            NumberAnimation { target: shineGlyph; property: "opacity"; to: 0; duration: 240; easing.type: Easing.InOutQuad }
        }
        SequentialAnimation {
            NumberAnimation { target: glyphBox; property: "scale"; from: 1; to: 1.15; duration: 160; easing.type: Easing.InOutQuad }
            NumberAnimation { target: glyphBox; property: "scale"; to: 1; duration: 240; easing.type: Easing.InOutQuad }
        }
    }
}
