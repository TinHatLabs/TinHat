// PresenceLine.qml — 10.6 (docs/REACH.md §2.6 item 4 + A4.5): the profile-card line under a name,
// led by the dot: "Online", "Away", "In a call · Gaming Central", or the custom status chip plus the
// state when it is not plain Online ("[🎯 Looking for group] · In a call"). Nothing at all when the
// person is absent (not sharing / offline). `pres` is an app.presenceOf() entry.
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: presence.line.fg, the presence.*.dot tokens

RowLayout {
    id: line
    property var app
    property var pres: null            // {state, room?, dot, label, chip?, chipColor?} or null
    property string roomName: ""       // the call's room, when we are in it too (the server only says then)
    property real pointSize: (Kirigami.Theme.smallFont.pointSize > 0 ? Kirigami.Theme.smallFont.pointSize : Math.max(7, Kirigami.Theme.defaultFont.pointSize * 0.85))
    property color textColor: Th.presenceLineFg
    readonly property bool hasChip: !!pres && !!pres.chip
    readonly property string stateWord: !pres ? "" : (pres.state === "away" ? "Away" : (pres.state === "call" ? "In a call" : "Online"))
    readonly property string tail: {
        if (!pres) return ""
        var parts = []
        if (!line.hasChip || pres.state !== "online") parts.push(line.stateWord)
        if (pres.state === "call" && line.roomName.length > 0) parts.push(line.roomName)
        return parts.join(" · ")
    }
    visible: !!pres
    spacing: 6
    property real maxTailWidth: 260
    // a layout fills its parent's width by default — this line is only as wide as what it says,
    // so a centred card (the profile flyout) centres it
    Layout.fillWidth: false

    Rectangle {
        Layout.alignment: Qt.AlignVCenter
        implicitWidth: 10; implicitHeight: 10
        radius: 5
        color: line.pres && line.pres.dot ? Th.colourRef(line.pres.dot) : (line.pres && line.pres.state === "away" ? Th.presenceAwayDot : Th.presenceOnlineDot)
        Accessible.role: Accessible.Indicator
        Accessible.name: line.pres ? String(line.pres.label || line.stateWord) : ""
    }
    StatusChip {
        visible: line.hasChip
        Layout.alignment: Qt.AlignVCenter
        Layout.maximumWidth: line.width > 0 ? line.width - 16 : 100000
        app: line.app
        text: line.hasChip ? String(line.pres.chip) : ""
        chipColor: line.hasChip && line.pres.chipColor ? line.pres.chipColor : "#95A5A6"
        pointSize: line.pointSize
    }
    Controls.Label {
        visible: line.tail.length > 0
        Layout.alignment: Qt.AlignVCenter
        // not fillWidth: the line is only as wide as what it says, so a centred card centres it
        Layout.maximumWidth: line.maxTailWidth
        text: (line.hasChip ? "· " : "") + line.tail
        textFormat: Text.PlainText   // the call's room name is someone else's text
        color: line.textColor
        font.pointSize: line.pointSize
        elide: Text.ElideRight
    }
}
