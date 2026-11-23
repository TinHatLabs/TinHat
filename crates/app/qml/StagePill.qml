// StagePill.qml — games round 2, B4 (GAMES-V1.1-ADDENDUM §1.1): the call head's two-segment switch "Table | Call" while
// a table of this call is open on my screen. Table = the table IS the call's stage (the call's people as a strip);
// Call = the normal tiles with the compact table in the corner. One click; G while the call island has focus.
// The web's StagePill draws the same (words table.r2.stage_table / stage_call).
import QtQuick
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app

Rectangle {
    id: pill
    property var app: null
    readonly property var r2: pill.app && pill.app.playWords && pill.app.playWords.table && pill.app.playWords.table.r2 ? pill.app.playWords.table.r2 : ({})
    readonly property bool tableOpen: !!pill.app && !!pill.app.playCtl && pill.app.playCtl.tableId !== ""
    objectName: "stagePill"
    visible: pill.tableOpen
    implicitWidth: segs.implicitWidth + 4
    implicitHeight: segs.implicitHeight + 4
    radius: height / 2
    color: Th.surfaceSunken
    border.width: 1
    border.color: Th.borderSubtle
    Accessible.role: Accessible.PageTabList
    Row {
        id: segs
        anchors.centerIn: parent
        spacing: 0
        Repeater {
            model: [{ id: "table", label: String(pill.r2.stage_table || "Table") }, { id: "call", label: String(pill.r2.stage_call || "Call") }]
            Rectangle {
                id: seg
                required property var modelData
                readonly property bool on: !!pill.app && pill.app.gameStage === seg.modelData.id
                objectName: "stagePill-" + seg.modelData.id
                implicitWidth: lbl.implicitWidth + 20
                implicitHeight: lbl.implicitHeight + 6
                radius: height / 2
                color: seg.on ? Th.accentBase : (hov.hovered ? Th.surfaceHover : "transparent")
                Controls.Label {
                    id: lbl
                    anchors.centerIn: parent
                    text: seg.modelData.label
                    textFormat: Text.PlainText
                    color: seg.on ? Th.buttonPrimaryFg : Th.textSecondary
                    font.weight: seg.on ? Font.DemiBold : Font.Normal
                    font.pointSize: Kirigami.Theme.defaultFont.pointSize * Th.textScale * 0.92
                }
                HoverHandler { id: hov; cursorShape: Qt.PointingHandCursor }
                TapHandler { onTapped: if (pill.app && !seg.on) pill.app.toggleGameStage() }
                Accessible.role: Accessible.PageTab
                Accessible.name: seg.modelData.label
                Accessible.selected: seg.on
                Accessible.onPressAction: if (pill.app && !seg.on) pill.app.toggleGameStage()
            }
        }
    }
}
