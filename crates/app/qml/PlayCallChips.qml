// PlayCallChips.qml — mini games v1 (GAMES-V1-SPEC.md §6.4): in the call's status strip, one chip per table in this
// call's room, drawn as Tin Hat Web's CallChip (CallChip.module.css): a 26 px pill, the game's glyph drawn (♠ in the text
// colour, The Rig's gold ★ tile: no emoji, which differ per OS), the bold label, and a filled "Join" pill. The call bar's
// Games button (CallIsland.qml) opens the Games list, so there is no extra Games chip here. ui.json chip_label stays the
// accessible name. Only while the call room's space runs games (`play.available`), or the room could use a space's chips
// / its space is still loading (then one quiet chip opens the panel's explanation).
// While the chips show, my wallet is asked again every 60 s (earned chips show up without a reload).
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app

RowLayout {
    id: chips
    property var app: null
    property var play: null
    property var words: ({})
    spacing: Kirigami.Units.smallSpacing
    readonly property var sp: {
        if (!chips.play || !chips.play.spaceJson) return null
        try { return JSON.parse(chips.play.spaceJson) } catch (e) { return null }
    }
    readonly property bool explain: !!chips.sp && ((chips.sp.why === "no-space" && chips.sp.link && chips.sp.link.length > 0)
                                                   || chips.sp.why === "unavailable" || chips.sp.why === "owner-unavailable")
    visible: !!chips.play && (chips.play.available || chips.explain)
    readonly property var tables: {
        if (!chips.play || !chips.play.tablesJson) return []
        try { return JSON.parse(chips.play.tablesJson) } catch (e) { return [] }
    }
    readonly property var art: chips.app && chips.app.playArt ? chips.app.playArt : ({})
    readonly property var cw: chips.words && chips.words.chip ? chips.words.chip : ({})
    readonly property var gw: chips.words && chips.words.games ? chips.words.games : ({})
    // the body size; every piece = body × the web's ratio to its 13 px body (CallChip.module.css: label 12 px, Join 11 px)
    readonly property real fBody: (Kirigami.Theme.defaultFont.pointSize > 0 ? Kirigami.Theme.defaultFont.pointSize : 10) * Th.textScale
    function label(t) { return chips.play ? String(chips.play.chipLabel(JSON.stringify(t))) : "" }   // ui.json chip_label
    // the label without its leading glyph and trailing "· Join" (the pill draws both)
    function body(t) { return chips.label(t).replace(/^\S+\s+/, "").replace(/\s+·\s+\S+$/, "") }

    Timer {   // the wallet, again every 60 s while the chips show
        interval: 60000
        repeat: true
        running: chips.visible && !!chips.play && chips.play.available
        onTriggered: chips.play.refreshWallet()
    }

    Repeater {
        // a chip only where chip_label gives one (lobby | hand | pause | round): an ended table gets none
        model: chips.tables.filter(function (t) { return chips.label(t) !== "" })
        delegate: Rectangle {
            id: chip
            required property var modelData
            readonly property bool rig: modelData.game === "rig"
            readonly property bool open: !!chips.play && chips.play.tableId === modelData.table
            implicitHeight: 26
            implicitWidth: chipRow.implicitWidth + 14
            radius: 13
            color: chipHover.hovered ? Th.surfaceHover : Th.surfaceOverlay
            border.width: 1
            border.color: chip.open ? Th.accentBase : (chip.rig ? Th.accentDim : Th.borderStrong)
            HoverHandler { id: chipHover; cursorShape: Qt.PointingHandCursor }
            TapHandler { onTapped: chips.app.openGames(String(modelData.table)) }
            Accessible.role: Accessible.Button
            Accessible.name: chips.label(modelData)
            Accessible.onPressAction: chips.app.openGames(String(modelData.table))
            Row {
                id: chipRow
                anchors.verticalCenter: parent.verticalCenter
                x: 10
                spacing: 6
                Rectangle {   // the glyph: ♠, or The Rig's gold ★ tile
                    anchors.verticalCenter: parent.verticalCenter
                    width: 16; height: 16; radius: 4
                    color: "transparent"
                    border.width: chip.rig ? 1 : 0
                    border.color: chips.art.chip || Th.accentBase
                    Controls.Label {
                        anchors.centerIn: parent
                        text: chip.rig ? "★" : "♠"
                        textFormat: Text.PlainText
                        color: chip.rig ? (chips.art.chip || Th.accentBright) : Th.textPrimary
                        font.pointSize: chips.fBody * (chip.rig ? 0.77 : 0.92)   // the web's .glyphRig 10 px / .glyph 12 px
                    }
                }
                Controls.Label {
                    anchors.verticalCenter: parent.verticalCenter
                    text: chips.body(modelData)
                    textFormat: Text.PlainText
                    color: Th.textPrimary
                    font.weight: Font.DemiBold
                    font.pointSize: chips.fBody * 0.92
                }
                Rectangle {   // the Join pill
                    anchors.verticalCenter: parent.verticalCenter
                    radius: 10
                    color: Th.accentBase
                    implicitWidth: joinLabel.implicitWidth + 18   // 9 px each side (the web's .join: 2px 9px)
                    implicitHeight: joinLabel.implicitHeight + 4
                    Controls.Label {
                        id: joinLabel
                        anchors.centerIn: parent
                        text: String(chips.words.card && chips.words.card.join ? chips.words.card.join : "Join")
                        textFormat: Text.PlainText
                        color: Th.textOnAccent
                        font.bold: true
                        font.pointSize: chips.fBody * 0.85
                    }
                }
            }
        }
    }
    // no table to show but something to explain (no space yet, the space loading): one quiet chip opens the panel
    Rectangle {
        visible: chips.explain && !chips.play.available
        implicitHeight: 26
        implicitWidth: explainLabel.implicitWidth + 20
        radius: 13
        color: explainHover.hovered ? Th.surfaceHover : Th.surfaceOverlay
        border.width: 1
        border.color: Th.borderStrong
        HoverHandler { id: explainHover; cursorShape: Qt.PointingHandCursor }
        TapHandler { onTapped: chips.app.gamePanelOpen = true }
        Accessible.role: Accessible.Button
        Accessible.name: explainLabel.text
        Accessible.onPressAction: chips.app.gamePanelOpen = true
        Controls.Label {
            id: explainLabel
            anchors.centerIn: parent
            text: "♠ " + String(chips.cw.new || "Games")
            textFormat: Text.PlainText
            color: Th.textSecondary
            font.pointSize: chips.fBody * 0.92
        }
    }
}
