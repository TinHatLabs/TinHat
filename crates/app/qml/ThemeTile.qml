// ThemeTile.qml — the "All 1,190 themes" and "Import a theme" tiles of Settings → Appearance (themes step 1b, contract
// THEMES-UI.md §5.3; the web's ThemeTile): a card-sized button with a 1 px dashed border, transparent (the tile hover
// fill on hover), an icon 22 over a centred label of up to 2 lines, gap 4, padding 8. Focus: a 2 px solid focus
// border. Arrow keys move through the grid it ends (ThemeGrid.qml).
import QtQuick
import QtQuick.Shapes
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the active theme (docs/MIGRATING-THEME.md)

Controls.AbstractButton {
    id: tile
    property var app: null
    property string iconName: ""
    signal arrowKey(int dx, int dy)
    implicitWidth: 148
    implicitHeight: 108
    hoverEnabled: true
    focusPolicy: Qt.StrongFocus
    Accessible.role: Accessible.Button
    Accessible.name: text
    Keys.onSpacePressed: (e) => { tile.clicked(); e.accepted = true }
    Keys.onReturnPressed: (e) => { tile.clicked(); e.accepted = true }
    Keys.onEnterPressed: (e) => { tile.clicked(); e.accepted = true }
    Keys.onLeftPressed: (e) => { tile.arrowKey(-1, 0); e.accepted = true }
    Keys.onRightPressed: (e) => { tile.arrowKey(1, 0); e.accepted = true }
    Keys.onUpPressed: (e) => { tile.arrowKey(0, -1); e.accepted = true }
    Keys.onDownPressed: (e) => { tile.arrowKey(0, 1); e.accepted = true }

    background: Item {
        Shape {
            anchors.fill: parent
            visible: !tile.visualFocus
            ShapePath {
                strokeColor: Th.settingsThemeTileBorder
                strokeWidth: 1
                strokeStyle: ShapePath.DashLine
                dashPattern: [3, 3]
                fillColor: tile.hovered ? Th.settingsThemeTileBgHover : "transparent"
                PathRectangle { x: 0.5; y: 0.5; width: tile.width - 1; height: tile.height - 1; radius: 8 }
            }
        }
        Rectangle {
            anchors.fill: parent
            visible: tile.visualFocus
            radius: 8
            color: tile.hovered ? Th.settingsThemeTileBgHover : "transparent"
            border.width: 2
            border.color: Th.settingsThemeCardBorderFocus
        }
    }
    contentItem: Item {
        Column {
            anchors.centerIn: parent
            width: parent.width - 16
            spacing: 4
            Kirigami.Icon {
                anchors.horizontalCenter: parent.horizontalCenter
                width: 22; height: 22
                source: tile.iconName
                color: Th.settingsThemeTileIcon
                isMask: true
            }
            Text {
                width: parent.width
                horizontalAlignment: Text.AlignHCenter
                wrapMode: Text.Wrap
                maximumLineCount: 2
                elide: Text.ElideRight
                text: tile.text
                color: Th.settingsThemeTileFg
                font.pointSize: Kirigami.Theme.defaultFont.pointSize
            }
        }
    }
}
