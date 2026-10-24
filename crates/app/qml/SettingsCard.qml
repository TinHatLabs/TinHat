// SettingsCard.qml — a card of Settings rows (contract SETTINGS-WINDOW.md §3): settings.card.bg, a 1 px
// settings.card.border, radius 10, clipped; the rows draw the 1 px settings.card.divider between themselves
// (SettingsRow.topDivider). `plain: true` = no card (the Theme gallery sits in its section full width).
import QtQuick
import QtQuick.Layouts
import dev.tinhat.app   // Th: the settings.card.* tokens; Motion

Rectangle {
    id: card
    property bool plain: false
    default property alias rows: col.data
    Layout.fillWidth: true
    implicitWidth: col.implicitWidth + 2
    implicitHeight: col.implicitHeight + (card.plain ? 0 : 2)
    radius: card.plain ? 0 : 10
    color: card.plain ? "transparent" : Th.settingsCardBg
    border.width: card.plain ? 0 : 1
    border.color: Th.settingsCardBorder
    clip: !card.plain
    // the search jump's flash for a whole card (a section anchor: Theme, Shortcuts) — over the fill, under the rows
    function flash() {
        hl.opacity = 1
        if (Motion.animate) { holdT.stop(); fade.restart() } else { fade.stop(); holdT.restart() }
    }
    Rectangle {
        id: hl
        anchors.fill: parent
        anchors.margins: card.plain ? -8 : 0
        radius: 10
        color: Th.settingsRowBgHighlight
        opacity: 0
    }
    SequentialAnimation {
        id: fade
        PauseAnimation { duration: 400 }
        NumberAnimation { target: hl; property: "opacity"; to: 0; duration: 800; easing.type: Easing.InOutQuad }
    }
    Timer { id: holdT; interval: 1200; onTriggered: hl.opacity = 0 }
    ColumnLayout {
        id: col
        x: card.plain ? 0 : 1
        y: card.plain ? 0 : 1
        width: card.width - (card.plain ? 0 : 2)
        spacing: 0
    }
}
