import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the rail.* / space.* / dm.* tokens
import QtQml.Models

// SpacesIsland.qml — islands C3 (ISLANDS-ARCH §4.2): extracted from Main.qml as it was (the flush layout, pixel-
// identical); controllers in through `app` (the Main.qml root) and the Main.qml ids it needs, intents out.
Item {
    id: island
    required property var app   // the Main.qml root (rail.app)
    required property var dmModelRef   // Main.qml's `dmModel`, passed in (never the same name: it would shadow the id)
    required property var railRef   // Main.qml's `rail`, passed in (never the same name: it would shadow the id)
    required property var realModelRef   // Main.qml's `realModel`, passed in (never the same name: it would shadow the id)
    // 11.0 (EDITMODE-UI §8, the Side monitor preset — `opts.spaces.orient = "h"`): the spaces left→right in a sideways-
    // scrolling row (no drag-to-reorder there, as on the web), the pill under the bubble, + at the end
    property bool horizontal: false
    implicitWidth: island.horizontal ? hrow.implicitWidth : col.implicitWidth
    implicitHeight: island.horizontal ? 52 : col.implicitHeight

    ColumnLayout {
        id: col
        anchors.fill: parent
        visible: !island.horizontal
        // islands C3: the divider, the real spaces (drag to reorder) and add / join, spaced like the rail column
        spacing: 4

        // divider between meta + real spaces — and, 10.6, the strip's divider (one, never two)
        Rectangle {
            visible: island.railRef._real.length > 0 || island.dmModelRef.count > 0
            Layout.alignment: Qt.AlignHCenter
            width: 28; height: 2; radius: 1
            color: Th.railDivider
        }

        // ── REAL spaces (drag — press-and-hold — to reorder) ────────────────
        ListView {
            id: realList
            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
            spacing: 4
            interactive: contentHeight > height
            model: island.horizontal ? null : island.realModelRef
            boundsBehavior: Flickable.StopAtBounds
            Controls.ScrollBar.vertical: Controls.ScrollBar { policy: Controls.ScrollBar.AsNeeded }
        }

        // ── add / join ──────────────────────────────────────────────────────
        Rectangle {
            Layout.alignment: Qt.AlignHCenter
            width: 28; height: 2; radius: 1
            color: Th.railDivider
        }
        Item {
            Layout.alignment: Qt.AlignHCenter
            Layout.bottomMargin: 2
            width: 68; height: 52

            Rectangle {
                id: addDisc
                anchors.centerIn: parent
                width: 46; height: 46
                radius: addMa.containsMouse ? 14 : width / 2
                color: addMa.containsMouse ? Th.spaceAddBgHover : "transparent"
                border.width: addMa.containsMouse ? 0 : 2
                border.color: Th.spaceAddBorder
                Behavior on radius { NumberAnimation { duration: island.railRef._a.dur; easing.type: Easing.OutCubic } }

                // 0.8.30: the pack's plus (hover story) instead of a typeset "+".
                ThIcon {
                    anchors.centerIn: parent
                    width: 22; height: 22
                    app: island.railRef._a
                    name: "list-add-symbolic"
                    color: addMa.containsMouse ? Th.spaceAddIconHover : Th.spaceAddIcon
                    isMask: true
                    hovered: addMa.containsMouse
                }
            }
            MouseArea {
                id: addMa
                anchors.fill: parent
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                onClicked: island.railRef.addRequested()
            }
            Controls.ToolTip.text: "Join with a link, or make a space"
            Controls.ToolTip.visible: addMa.containsMouse
            Controls.ToolTip.delay: 400
        }
    }

    // ── the row (Side monitor) ──
    RowLayout {
        id: hrow
        anchors.fill: parent
        visible: island.horizontal
        spacing: 4
        Rectangle {
            visible: island.railRef._real.length > 0 || island.dmModelRef.count > 0
            Layout.alignment: Qt.AlignVCenter
            Layout.preferredWidth: 2
            Layout.preferredHeight: 28
            radius: 1
            color: Th.railDivider
        }
        ListView {
            id: hList
            Layout.fillWidth: true
            Layout.preferredHeight: 56
            Layout.alignment: Qt.AlignVCenter
            orientation: ListView.Horizontal
            clip: true
            spacing: 4
            interactive: contentWidth > width
            model: island.horizontal ? island.realModelRef : null
            boundsBehavior: Flickable.StopAtBounds
        }
        Rectangle {
            Layout.alignment: Qt.AlignVCenter
            Layout.preferredWidth: 2
            Layout.preferredHeight: 28
            radius: 1
            color: Th.railDivider
        }
        Item {
            Layout.alignment: Qt.AlignVCenter
            Layout.preferredWidth: 52
            Layout.preferredHeight: 52
            Rectangle {
                anchors.centerIn: parent
                width: 46; height: 46
                radius: hAddMa.containsMouse ? 14 : width / 2
                color: hAddMa.containsMouse ? Th.spaceAddBgHover : "transparent"
                border.width: hAddMa.containsMouse ? 0 : 2
                border.color: Th.spaceAddBorder
                ThIcon {
                    anchors.centerIn: parent
                    width: 22; height: 22
                    app: island.railRef._a
                    name: "list-add-symbolic"
                    color: hAddMa.containsMouse ? Th.spaceAddIconHover : Th.spaceAddIcon
                    isMask: true
                    hovered: hAddMa.containsMouse
                }
            }
            MouseArea {
                id: hAddMa
                anchors.fill: parent
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                onClicked: island.railRef.addRequested()
            }
            Controls.ToolTip.text: "Join with a link, or make a space"
            Controls.ToolTip.visible: hAddMa.containsMouse
            Controls.ToolTip.delay: 400
        }
    }
}
