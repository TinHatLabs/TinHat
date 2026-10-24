// GameServerIconButton.qml — the Game servers widget's small icon buttons (docs/GAMESERVERS.md §4; the web's
// gameservers/parts.tsx IconBtn): start / stop / restart, copy, show / hide the password. 22 × 22 (20 in a server
// line), radius 6, glyph 14 in `widget.servers.button.icon`, hover / pressed wash `widget.servers.button.bg.hover`, the
// widgets' focus ring; the label is the tooltip and the accessible name.
import QtQuick
import QtQuick.Controls as Controls
import dev.tinhat.app   // Th: the active theme (docs/MIGRATING-THEME.md)

Controls.AbstractButton {
    id: b
    property var app
    property string iconName: ""
    property string label: ""
    property color glyph: Th.widgetServersButtonIcon
    property int side: 22
    implicitWidth: b.side
    implicitHeight: b.side
    hoverEnabled: true
    focusPolicy: Qt.TabFocus
    opacity: b.enabled ? 1 : 0.4
    Accessible.name: b.label
    Controls.ToolTip.text: b.label
    Controls.ToolTip.visible: b.hovered && b.label.length > 0
    Controls.ToolTip.delay: 500
    background: Rectangle {
        radius: 6
        color: b.enabled && (b.hovered || b.pressed) ? Th.widgetServersButtonBgHover : "transparent"
        border.width: b.visualFocus ? 1 : 0
        border.color: Th.widgetRingFocus
    }
    contentItem: Item {
        ThIcon {
            anchors.centerIn: parent
            width: 14; height: 14
            app: b.app
            name: b.iconName
            color: b.glyph
        }
    }
}
