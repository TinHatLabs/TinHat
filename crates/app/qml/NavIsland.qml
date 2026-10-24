import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the rail.* / space.* / dm.* tokens
import QtQml.Models

// NavIsland.qml — islands C3 (ISLANDS-ARCH §4.2): extracted from Main.qml as it was (the flush layout, pixel-
// identical); controllers in through `app` (the Main.qml root) and the Main.qml ids it needs, intents out.
Item {
    id: island
    required property var app   // the Main.qml root (rail.app)
    required property var railRef   // Main.qml's `rail`, passed in (never the same name: it would shadow the id)
    // 11.0 (EDITMODE-UI §8, the Side monitor preset — `opts.nav.orient = "h"`): the bubbles left→right, the pill under
    property bool horizontal: false
    visible: island.railRef._meta.length > 0
    implicitWidth: island.horizontal ? hrow.implicitWidth : col.implicitWidth
    implicitHeight: island.horizontal ? 52 : col.implicitHeight

    // islands C3: the meta spaces (Home · Favourites · People · Rooms), spaced like the rail column they came from
    ColumnLayout {
        id: col
        anchors.fill: parent
        visible: !island.horizontal
        spacing: 4
        Repeater {
            model: island.horizontal ? [] : island.railRef._meta
            delegate: SpaceBubble {
                id: metaBubble
                railRef: island.railRef
                required property var modelData
                Layout.alignment: Qt.AlignHCenter
                sp: modelData
                selected: modelData.key === island.railRef.activeKey
                hovered: metaMa.containsMouse
                MouseArea {
                    id: metaMa
                    anchors.fill: parent
                    hoverEnabled: true
                    cursorShape: Qt.PointingHandCursor
                    onClicked: if (island.railRef.navController) island.railRef.navController.selectSpace(metaBubble.modelData.key)
                }
                Controls.ToolTip.text: modelData.label
                Controls.ToolTip.visible: metaMa.containsMouse
                Controls.ToolTip.delay: 400
            }
        }
    }
    Row {
        id: hrow
        visible: island.horizontal
        anchors.verticalCenter: parent.verticalCenter
        anchors.horizontalCenter: parent.horizontalCenter
        spacing: 4
        Repeater {
            model: island.horizontal ? island.railRef._meta : []
            delegate: SpaceBubble {
                id: hBubble
                railRef: island.railRef
                required property var modelData
                pillBelow: true
                sp: modelData
                selected: modelData.key === island.railRef.activeKey
                hovered: hMa.containsMouse
                MouseArea {
                    id: hMa
                    anchors.fill: parent
                    hoverEnabled: true
                    cursorShape: Qt.PointingHandCursor
                    onClicked: if (island.railRef.navController) island.railRef.navController.selectSpace(hBubble.modelData.key)
                }
                Controls.ToolTip.text: modelData.label
                Controls.ToolTip.visible: hMa.containsMouse
                Controls.ToolTip.delay: 400
            }
        }
    }
}
