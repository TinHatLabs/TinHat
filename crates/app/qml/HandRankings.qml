// HandRankings.qml — games round 2, F5 (GAMES-V1.1-ADDENDUM §2): the hand-rankings sheet (Tin Hat Web's popover draws the
// same). The Hold'em table's Hands button opens it in a 360 px popover; strongest first, each row = the name, five example
// cards (the table's own faces, small) and one line; the foot line. `mine` = my current made hand (my cards + the board,
// the app's own evaluator, only while I can see my cards): that row gets the accent border and "You: <hand>". Words:
// words.json `table.r2` (hands_*), `table.rank_lines`, `table.rank_eg`, `table.categories` (byte-identical with the web).
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app

Rectangle {
    id: hr
    property var play: null
    property var art: ({})
    property var w: ({})          // words.json `table`
    property string mine: ""      // a category id ("flush") or ""
    property real fBody: 10
    property bool toggles: false      // a seated player: Show my odds + the turn notices at the bottom
    property bool oddsOn: true
    property bool turnOn: false
    signal closeRequested()
    signal oddsToggled(bool on)
    signal turnToggled(bool on)

    readonly property var rw: hr.w && hr.w.r2 ? hr.w.r2 : ({})
    // strongest first (= play_hand::CATEGORIES reversed; a test holds the words' order to it)
    readonly property var order: ["straight-flush", "quads", "full-house", "flush", "straight", "trips", "two-pair", "pair", "high-card"]
    readonly property var rows: hr.order.map(function (id) {
        return { id: id, what: hr.w && hr.w.rank_lines ? String(hr.w.rank_lines[id] || "") : "", eg: hr.w && hr.w.rank_eg ? hr.w.rank_eg[id] || [] : [] }
    })
    function catName(id) { return hr.w && hr.w.categories && hr.w.categories[id] ? String(hr.w.categories[id]) : id }

    objectName: "handRankings"
    color: Th.surfaceRaised
    border.width: 1
    border.color: Th.borderSubtle
    radius: 10
    Accessible.role: Accessible.Dialog
    Accessible.name: String(hr.rw.hands_title || "")
    focus: true
    Keys.onEscapePressed: hr.closeRequested()
    // a click on the sheet stays on the sheet (never reaches the felt's seats underneath)
    MouseArea { anchors.fill: parent; acceptedButtons: Qt.AllButtons; onWheel: function (e) { e.accepted = false } }

    ColumnLayout {
        anchors.fill: parent
        anchors.margins: 12
        spacing: 8
        RowLayout {
            Layout.fillWidth: true
            Controls.Label {
                Layout.fillWidth: true
                text: String(hr.rw.hands_title || "")
                textFormat: Text.PlainText
                color: Th.textPrimary
                font.bold: true
                font.pointSize: hr.fBody * 1.04
            }
            ThButton {
                objectName: "handRankingsClose"
                flat: true
                implicitHeight: 28
                text: String(hr.rw.hands_close || "")
                Accessible.name: text
                onClicked: hr.closeRequested()
            }
        }
        Flickable {
            id: fl
            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
            contentHeight: rowsCol.implicitHeight
            boundsBehavior: Flickable.StopAtBounds
            Controls.ScrollBar.vertical: Controls.ScrollBar { policy: fl.contentHeight > fl.height ? Controls.ScrollBar.AsNeeded : Controls.ScrollBar.AlwaysOff }
            Column {
                id: rowsCol
                width: fl.width
                spacing: 4
                Repeater {
                    model: hr.rows
                    Rectangle {
                        id: row
                        required property var modelData
                        required property int index
                        readonly property bool lit: hr.mine !== "" && hr.mine === String(row.modelData.id)
                        objectName: "rankRow-" + String(row.modelData.id)
                        width: rowsCol.width
                        height: rowCol.implicitHeight + 12
                        radius: 8
                        color: row.lit ? Th.surfaceSelected : "transparent"
                        border.width: row.lit ? 1 : 0
                        border.color: Th.accentBright
                        Accessible.role: Accessible.ListItem
                        Accessible.name: (row.index + 1) + ". " + hr.catName(row.modelData.id) + ": " + String(row.modelData.what || "")
                                         + (row.lit ? " " + String(hr.rw.hands_you || "").split("{what}").join(hr.catName(row.modelData.id)) : "")
                        Column {
                            id: rowCol
                            x: 8; y: 6
                            width: row.width - 16
                            spacing: 4
                            RowLayout {
                                width: parent.width
                                spacing: 6
                                Controls.Label {
                                    text: String(row.index + 1)
                                    textFormat: Text.PlainText
                                    color: Th.textMuted
                                    font.pointSize: hr.fBody * 0.81
                                    Layout.preferredWidth: hr.fBody * 1.4
                                }
                                Controls.Label {
                                    text: hr.catName(row.modelData.id)
                                    textFormat: Text.PlainText
                                    color: Th.textPrimary
                                    font.weight: Font.DemiBold
                                    font.pointSize: hr.fBody * 0.92
                                }
                                Item { Layout.fillWidth: true }
                                Controls.Label {
                                    visible: row.lit
                                    text: String(hr.rw.hands_you || "").split("{what}").join(hr.catName(row.modelData.id))
                                    textFormat: Text.PlainText
                                    color: Th.accentBright
                                    font.bold: true
                                    font.pointSize: hr.fBody * 0.81
                                }
                            }
                            Row {
                                x: hr.fBody * 1.4 + 6
                                spacing: 3
                                Repeater {
                                    model: row.modelData.eg || []
                                    PlayingCard {
                                        required property var modelData
                                        art: hr.art
                                        play: hr.play
                                        size: "sm"
                                        card: String(modelData)
                                    }
                                }
                            }
                            Controls.Label {
                                x: hr.fBody * 1.4 + 6
                                width: parent.width - x
                                text: String(row.modelData.what || "")
                                textFormat: Text.PlainText
                                wrapMode: Text.WordWrap
                                color: Th.textSecondary
                                font.pointSize: hr.fBody * 0.81
                            }
                        }
                    }
                }
                Item { width: 1; height: 4 }
                Repeater {
                    model: [hr.rw.hands_foot]
                    Controls.Label {
                        required property var modelData
                        width: rowsCol.width
                        text: String(modelData || "")
                        textFormat: Text.PlainText
                        wrapMode: Text.WordWrap
                        color: Th.textMuted
                        font.pointSize: hr.fBody * 0.81
                    }
                }
            }
        }
        ColumnLayout {   // the players' switches (design2 14:53)
            visible: hr.toggles
            Layout.fillWidth: true
            spacing: 0
            Rectangle { Layout.fillWidth: true; implicitHeight: 1; color: Th.borderSubtle }
            Controls.CheckBox {
                objectName: "handsOddsToggle"
                text: LinkGate.plain(String(hr.rw.odds_toggle || ""))
                checked: hr.oddsOn
                onToggled: hr.oddsToggled(checked)
                font.pointSize: hr.fBody * 0.85
            }
            Controls.CheckBox {
                objectName: "handsTurnToggle"
                text: LinkGate.plain(String(hr.rw.turn_toggle || ""))
                checked: hr.turnOn
                onToggled: hr.turnToggled(checked)
                font.pointSize: hr.fBody * 0.85
            }
        }
    }
}
