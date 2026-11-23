// CompactTable.qml — games round 2, B4 + B10 (GAMES-V1.1-ADDENDUM §8.4): the always-visible table strip, 280 × 64:
// the game glyph, my two cards (by the shared cards-face rule: face down while a Pad has them or I share), the board,
// the pot, whose turn + the countdown ring, and on my turn Check / Call N / Fold (no raise: raising opens the table).
// Shown in the call stage while the stage is on Call (bottom-right over the tiles). A click on it (not on a quick
// button) = the stage back to Table. Low-GPU: the ring ticks at 1 Hz; nothing else moves. The web draws the same.
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import QtQuick.Shapes
import org.kde.kirigami as Kirigami
import dev.tinhat.app

Rectangle {
    id: ct
    property var app: null
    property var play: null
    property string myId: ""
    property bool sharing: false
    property real clockOffset: 0
    readonly property var view: { try { return ct.play && ct.play.viewJson ? JSON.parse(ct.play.viewJson) : null } catch (e) { return null } }
    readonly property var art: ct.app && ct.app.playArt ? ct.app.playArt : ({})
    readonly property var w: ct.app && ct.app.playWords && ct.app.playWords.table ? ct.app.playWords.table : ({})
    readonly property bool holdem: !!ct.view && ct.view.game === "holdem"
    readonly property var hand: ct.view && ct.view.hand ? ct.view.hand : null
    readonly property var you: ct.view && ct.view.you ? ct.view.you : null
    readonly property var legal: ct.you && ct.you.legal ? ct.you.legal : null
    readonly property var toAct: ct.hand && ct.hand.to_act ? ct.hand.to_act : null
    readonly property int mySeat: ct.you && ct.you.seat ? ct.you.seat : 0
    readonly property bool myTurn: !!ct.legal && !!ct.toAct && ct.mySeat > 0 && ct.toAct.seat === ct.mySeat
    readonly property var screens: ct.you && ct.you.screens ? ct.you.screens : []
    readonly property bool hasCards: !!ct.you && !!ct.you.cards && ct.you.cards.length > 0
    // the shared rule with the table's Show here (Main.gameShowHere); no peek here: face down = backs
    readonly property string face: ct.play ? String(ct.play.cardsFace(ct.hasCards, ct.sharing, !!ct.app && ct.app.gameShowHere === true, JSON.stringify(ct.screens))) : "up"
    readonly property int pot: {
        var t = 0, p = ct.hand && ct.hand.pots ? ct.hand.pots : [], s = ct.view && ct.view.seats ? ct.view.seats : []
        for (var i = 0; i < p.length; ++i) t += p[i].amount || 0
        for (var j = 0; j < s.length; ++j) t += s[j].bet || 0
        return t
    }
    function nameOfSeat(n) {
        var s = ct.view && ct.view.seats ? ct.view.seats : []
        for (var i = 0; i < s.length; ++i) if (s[i].seat === n) return ct.app && ct.app.localpart ? String(ct.app.localpart(s[i].u)) : String(s[i].u)
        return ""
    }
    onViewChanged: if (ct.view && ct.view.now) ct.clockOffset = ct.view.now - Date.now()
    onClockOffsetChanged: ct.nowMs = Date.now() + ct.clockOffset
    property real nowMs: Date.now() + ct.clockOffset
    Timer { interval: 1000; repeat: true; running: ct.visible && !!ct.toAct && !!ct.toAct.deadline; onTriggered: ct.nowMs = Date.now() + ct.clockOffset }
    readonly property int secsLeft: ct.toAct && ct.toAct.deadline ? Math.max(0, Math.ceil((ct.toAct.deadline - ct.nowMs) / 1000)) : 0
    function act(o) { if (ct.play) ct.play.act(JSON.stringify(o)) }
    function fill(s, map) { s = String(s === undefined || s === null ? "" : s); for (var k in map) s = s.split("{" + k + "}").join(String(map[k])); return s }

    objectName: "compactTable"
    width: 280
    height: 64
    radius: 10
    color: Th.surfaceRaised
    border.width: ct.myTurn ? 2 : 1
    border.color: ct.myTurn ? Th.accentBright : Th.borderSubtle
    Accessible.role: Accessible.Button
    Accessible.name: (ct.holdem ? String(ct.app && ct.app.playWords && ct.app.playWords.games ? ct.app.playWords.games.holdem : "") : "")
                     + (ct.myTurn ? " · " + String(ct.w.r2 ? ct.w.r2.your_turn : "") : "")
    TapHandler { onTapped: if (ct.app) ct.app.setGameStage("table") }
    HoverHandler { cursorShape: Qt.PointingHandCursor }

    // the countdown ring, 22 px (1 Hz)
    component Ring: Shape {
        id: ring
        property real frac: 0
        property color ink: Th.accentBase
        width: 22; height: 22
        preferredRendererType: Shape.CurveRenderer
        ShapePath {
            strokeColor: Th.borderSubtle; strokeWidth: 2.5; fillColor: "transparent"
            PathAngleArc { centerX: 11; centerY: 11; radiusX: 9.5; radiusY: 9.5; startAngle: 0; sweepAngle: 360 }
        }
        ShapePath {
            strokeColor: ring.ink; strokeWidth: 2.5; fillColor: "transparent"; capStyle: ShapePath.RoundCap
            PathAngleArc { centerX: 11; centerY: 11; radiusX: 9.5; radiusY: 9.5; startAngle: -90; sweepAngle: 360 * ring.frac }
        }
    }

    ColumnLayout {   // web2's compact strip: line 1 = ♠ · my 2 mini cards · the board (or my quick buttons) · the ring;
        anchors.fill: parent   // line 2 = "Dee · 12s" / "Your turn · 28s" + the pot · the pill
        anchors.leftMargin: 8; anchors.rightMargin: 8; anchors.topMargin: 5; anchors.bottomMargin: 5
        spacing: 3
        RowLayout {
            Layout.fillWidth: true
            spacing: 5
            Rectangle {   // the game's glyph tile (♠ on the felt colour, The Rig's ★ in its trim)
                Layout.preferredWidth: 18; Layout.preferredHeight: 18; radius: 4
                color: ct.holdem ? (ct.art.feltCentre || Th.accentDim) : "transparent"
                border.width: ct.holdem ? 0 : 1
                border.color: ct.art.machineTrim || Th.accentBright
                Controls.Label { anchors.centerIn: parent; text: ct.holdem ? "♠" : "★"; textFormat: Text.PlainText; color: ct.holdem ? "white" : (ct.art.machineTrim || Th.accentBright); font.pointSize: Kirigami.Theme.defaultFont.pointSize * Th.textScale * 0.72 }
            }
            Row {   // my cards (xs: 17 × 23)
                visible: ct.holdem && ct.hasCards
                spacing: 2
                Repeater {
                    model: ct.hasCards ? ct.you.cards : []
                    PlayingCard { required property var modelData; art: ct.art; play: ct.play; size: "xs"; card: ct.face === "up" ? String(modelData) : "" }
                }
            }
            Rectangle { visible: ct.holdem && ct.hasCards; width: 1; height: 18; color: Th.borderSubtle }
            Row {   // the board (not on my turn: the quick buttons take its place)
                visible: ct.holdem && !ct.myTurn
                spacing: 2
                Repeater {
                    model: ct.hand && ct.hand.board ? ct.hand.board : []
                    PlayingCard { required property var modelData; art: ct.art; play: ct.play; size: "xs"; card: String(modelData) }
                }
            }
            // on my turn: Check / Call N / Fold (no raise here: raising opens the table)
            ThButton {
                objectName: "compactCheck"
                visible: ct.myTurn && !!ct.legal && ct.legal.check === true
                implicitHeight: 23; leftPadding: 7; rightPadding: 7
                font.pointSize: Kirigami.Theme.defaultFont.pointSize * Th.textScale * 0.81
                text: ct.w.actions ? String(ct.w.actions.check || "") : ""
                Accessible.name: text
                onClicked: ct.act({ do: "check" })
            }
            ThButton {
                objectName: "compactCall"
                visible: ct.myTurn && !!ct.legal && typeof ct.legal.call === "number" && ct.legal.call > 0
                highlighted: true
                implicitHeight: 23; leftPadding: 7; rightPadding: 7
                font.pointSize: Kirigami.Theme.defaultFont.pointSize * Th.textScale * 0.81
                text: ct.w.actions ? ct.fill(ct.w.actions.call_n, { n: ct.legal ? ct.legal.call : 0 }) : ""
                Accessible.name: text
                onClicked: ct.act({ do: "call" })
            }
            ThButton {
                objectName: "compactFold"
                visible: ct.myTurn && !!ct.legal && ct.legal.fold === true
                flat: true
                implicitHeight: 23; leftPadding: 7; rightPadding: 7
                font.pointSize: Kirigami.Theme.defaultFont.pointSize * Th.textScale * 0.81
                text: ct.w.actions ? String(ct.w.actions.fold || "") : ""
                Accessible.name: text
                onClicked: ct.act({ do: "fold" })
            }
            Item { Layout.fillWidth: true }
            Ring {
                visible: !!ct.toAct && !!ct.toAct.deadline
                frac: ct.secsLeft / Math.max(1, (ct.view && ct.view.opts && ct.view.opts.turn_s) || 30)
                ink: ct.myTurn ? (ct.secsLeft <= 5 ? Th.statusErrorFg : Th.accentBright) : Th.accentBase
            }
        }
        RowLayout {
            Layout.fillWidth: true
            spacing: 6
            Controls.Label {
                Layout.fillWidth: true
                elide: Text.ElideRight
                textFormat: Text.PlainText
                font.pointSize: Kirigami.Theme.defaultFont.pointSize * Th.textScale * 0.81
                color: ct.myTurn ? Th.accentBright : Th.textSecondary
                font.bold: ct.myTurn
                text: ct.myTurn ? String(ct.w.r2 ? ct.w.r2.your_turn : "") + " · " + ct.secsLeft + "s"
                    : (ct.toAct ? ct.nameOfSeat(ct.toAct.seat) + " · " + ct.secsLeft + "s" : "")
            }
            Row {   // the pot
                visible: ct.pot > 0
                spacing: 3
                Rectangle { anchors.verticalCenter: parent.verticalCenter; width: 9; height: 9; radius: 4.5; color: ct.art.chip || Th.accentBase; border.width: 2; border.color: ct.art.chipEdge || Th.accentBright }
                Controls.Label { anchors.verticalCenter: parent.verticalCenter; text: String(ct.pot); textFormat: Text.PlainText; color: Th.textPrimary; font.weight: Font.DemiBold; font.pointSize: Kirigami.Theme.defaultFont.pointSize * Th.textScale * 0.81 }
            }
            StagePill { app: ct.app; scale: 0.85; transformOrigin: Item.Right }
        }
    }
}
