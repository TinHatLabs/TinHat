// StatusChip.qml — 10.6 (docs/REACH.md A4.5): a custom status as a pill — the status colour at 14 %
// alpha behind, the text in the status colour, emoji first. Used by the profile cards, the footer
// line under your name, the member list's secondary line and the status picker. Presentational
// only: the text and colour come precomputed from Rust (presence_model.rs `status_view`).
import QtQuick
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami

Rectangle {
    id: chip
    property var app
    property string text: ""
    property color chipColor: "#95A5A6"
    // smallFont can be unset (-1) without a KDE platform theme (the Xvfb rig) — then 85 % of the default
    property real pointSize: (Kirigami.Theme.smallFont.pointSize > 0 ? Kirigami.Theme.smallFont.pointSize : Math.max(7, Kirigami.Theme.defaultFont.pointSize * 0.85))
    property real maxWidth: 100000
    radius: height / 2
    color: Qt.rgba(chip.chipColor.r, chip.chipColor.g, chip.chipColor.b, 0.14)
    implicitHeight: Math.round(label.implicitHeight + 4)
    implicitWidth: Math.min(chip.maxWidth, Math.ceil(label.implicitWidth) + 14)
    Accessible.role: Accessible.StaticText
    Accessible.name: chip.text
    Controls.Label {
        id: label
        anchors.verticalCenter: parent.verticalCenter
        x: 7
        width: Math.max(0, chip.width - 14)
        text: chip.text
        // 10.6.0 review: a status is someone else's text — never markup (an <img> would load)
        textFormat: Text.PlainText
        color: chip.chipColor
        font.pointSize: chip.pointSize
        elide: Text.ElideRight
        maximumLineCount: 1
    }
}
