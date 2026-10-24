import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the rail.* / space.* / dm.* tokens
import QtQml.Models

// DmsIsland.qml — islands C3 (ISLANDS-ARCH §4.2): extracted from Main.qml as it was (the flush layout, pixel-
// identical); controllers in through `app` (the Main.qml root) and the Main.qml ids it needs, intents out.
ColumnLayout {
    id: island
    required property var app   // the Main.qml root (rail.app)
    // islands C3: the DM strip + its "+N" disc; shown while either has height (both animate it), spaced like the rail
    spacing: 4
    visible: dmList.Layout.preferredHeight > 0.5 || dmMore.Layout.preferredHeight > 0.5
    required property var dmMenuRef   // Main.qml's `dmMenu`, passed in (never the same name: it would shadow the id)
    required property var dmModelRef   // Main.qml's `dmModel`, passed in (never the same name: it would shadow the id)
    required property var railRef   // Main.qml's `rail`, passed in (never the same name: it would shadow the id)
    // 11.0 (EDITMODE-UI §8, the Side monitor preset — `opts.dms.orient = "h"`): the DMs left→right, the pill under
    property bool horizontal: false

    ListView {
        id: dmList
        Layout.alignment: Qt.AlignHCenter
        orientation: island.horizontal ? ListView.Horizontal : ListView.Vertical
        Layout.preferredWidth: island.horizontal ? (island.dmModelRef.count > 0 ? island.dmModelRef.count * 68 + (island.dmModelRef.count - 1) * 4 : 0) : 68
        // The height follows the model with the rail's `dur`, so the spaces slide down / up
        // instead of jumping; `dur` is 0 under reduced motion (A2.14).
        Layout.preferredHeight: island.horizontal ? (island.dmModelRef.count > 0 ? 52 : 0)
                                : (island.dmModelRef.count > 0 ? island.dmModelRef.count * 52 + (island.dmModelRef.count - 1) * 4 : 0)
        Behavior on Layout.preferredHeight { NumberAnimation { duration: island.railRef._a.dur; easing.type: Easing.OutCubic } }
        visible: Layout.preferredHeight > 0.5
        interactive: false
        spacing: 4
        model: island.dmModelRef
        add: Transition {
            ParallelAnimation {
                NumberAnimation { property: "opacity"; from: 0; to: 1; duration: island.railRef._a.dur }
                NumberAnimation { property: "scale"; from: 0.5; to: 1; duration: island.railRef._a.dur; easing.type: Easing.OutCubic }
            }
        }
        remove: Transition {
            ParallelAnimation {
                NumberAnimation { property: "opacity"; to: 0; duration: island.railRef._a.dur }
                NumberAnimation { property: "scale"; to: 0.5; duration: island.railRef._a.dur; easing.type: Easing.InCubic }
            }
        }
        displaced: Transition { NumberAnimation { properties: "x,y"; duration: island.railRef._a.dur; easing.type: Easing.OutCubic } }
        move: Transition { NumberAnimation { properties: "x,y"; duration: island.railRef._a.dur; easing.type: Easing.OutCubic } }
        delegate: Item {
            id: dmSlot
            required property string rid
            required property var e
            width: 68
            height: 52
            readonly property bool invite: !!dmSlot.e && dmSlot.e.invite === true
            // §2.6 / A2.14: a dot only when the DM has exactly one other person (peer set) who shares.
            readonly property var pres: (dmSlot.e && dmSlot.e.peer && island.railRef.app && island.railRef.app.presenceOf)
                ? island.railRef.app.presenceOf(String(dmSlot.e.peer)) : null

            // the rail's hover pill (18 px) — like a space; a DM is never "selected" here
            Rectangle {
                visible: !island.horizontal
                anchors.verticalCenter: parent.verticalCenter
                x: 0; width: 4
                height: dmMa.containsMouse ? 18 : 0
                radius: 2
                color: Th.railPillBg
                Behavior on height { NumberAnimation { duration: island.railRef._a.dur; easing.type: Easing.OutCubic } }
            }
            Rectangle {   // in a row: under the bubble
                visible: island.horizontal
                anchors.horizontalCenter: parent.horizontalCenter
                y: parent.height
                height: 4
                width: dmMa.containsMouse ? 18 : 0
                radius: 2
                color: Th.railPillBg
                Behavior on width { NumberAnimation { duration: island.railRef._a.dur; easing.type: Easing.OutCubic } }
            }
            // DMs are people: always a circle, never the space squircle.
            Avatar {
                id: dmAv
                anchors.centerIn: parent
                width: 44; height: 44
                size: 44
                app: island.railRef._a
                name: dmSlot.e ? String(dmSlot.e.name || "") : ""
                // §6: the DM's person, as its room-list row and its header, so all three agree
                seed: dmSlot.e ? Seeds.avatarSeed(String(dmSlot.e.id || ""), String(dmSlot.e.peer || "")) : ""
                imageSource: (dmSlot.e && dmSlot.e.avatar) ? String(dmSlot.e.avatar) : ""
                opacity: dmSlot.invite ? 0.7 : 1.0
                presence: dmSlot.pres ? String(dmSlot.pres.state || "") : ""
                presenceColor: (dmSlot.pres && dmSlot.pres.dot) ? Th.colourRef(dmSlot.pres.dot)
                               : ((dmSlot.pres && dmSlot.pres.state === "away") ? Th.presenceAwayDot : Th.presenceOnlineDot)
                presenceText: dmSlot.pres ? String(dmSlot.pres.label || "") : ""
                presenceLeft: true
                presenceSize: 12
                presenceRingWidth: 2
                presenceRing: Th.dmPresenceRing
            }
            // an invite looks like an invited space: dashed 2 px accent ring + accent dot
            Canvas {
                id: dmInviteRing
                visible: dmSlot.invite
                anchors.centerIn: dmAv
                width: 52; height: 52
                readonly property color ringColor: Th.dmInviteRing
                onRingColorChanged: requestPaint()
                onVisibleChanged: if (visible) requestPaint()
                onPaint: {
                    var ctx = getContext("2d")
                    ctx.reset()
                    ctx.clearRect(0, 0, width, height)
                    ctx.lineWidth = 2
                    ctx.strokeStyle = ringColor
                    ctx.setLineDash([4, 3])
                    ctx.beginPath()
                    ctx.arc(width / 2, height / 2, width / 2 - 1, 0, 2 * Math.PI)
                    ctx.stroke()
                }
            }
            Rectangle {
                visible: dmSlot.invite
                width: 12; height: 12; radius: 6
                anchors.right: dmAv.right; anchors.top: dmAv.top
                anchors.rightMargin: -4; anchors.topMargin: -4
                color: Th.dmInviteDot
                border.width: 2; border.color: Th.dmInviteDotBorder
            }
            // every DM message is for you: always the red highlight badge
            UnreadBadge {
                app: island.railRef._a
                count: (dmSlot.e && !dmSlot.invite) ? Number(dmSlot.e.unread || 0) : 0
                highlight: true
                border.width: 2; border.color: Th.dmBadgeBorder
                anchors.right: dmAv.right; anchors.bottom: dmAv.bottom
                anchors.rightMargin: -3; anchors.bottomMargin: -3
            }
            MouseArea {
                id: dmMa
                anchors.fill: parent
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                acceptedButtons: Qt.LeftButton | Qt.RightButton
                pressAndHoldInterval: 500
                function openMenu() {
                    island.dmMenuRef.rid = dmSlot.rid
                    island.dmMenuRef.label = dmSlot.e ? String(dmSlot.e.name || "") : ""
                    island.dmMenuRef.invite = dmSlot.invite
                    island.dmMenuRef.popup()
                }
                onClicked: (mouse) => {
                    if (mouse.button === Qt.RightButton) { openMenu(); return }
                    if (dmSlot.invite) { if (island.railRef.navController) island.railRef.navController.selectSpace("people") }
                    else island.railRef.dmOpenRequested(dmSlot.rid)
                }
                onPressAndHold: openMenu()
            }
            Controls.ToolTip.text: island.railRef._tipSafe(island.railRef._dmTip(dmSlot.e, dmSlot.pres))
            Controls.ToolTip.visible: dmMa.containsMouse
            Controls.ToolTip.delay: 400
        }
    }
    // "+N": more DMs want you than the strip shows — looks like a meta disc, opens People.
    Item {
        id: dmMore
        readonly property bool shown: island.railRef._dmOverflow > 0 && island.dmModelRef.count > 0 && !island.horizontal
        Layout.alignment: Qt.AlignHCenter
        Layout.preferredWidth: 68
        Layout.preferredHeight: shown ? 52 : 0
        Behavior on Layout.preferredHeight { NumberAnimation { duration: island.railRef._a.dur; easing.type: Easing.OutCubic } }
        visible: Layout.preferredHeight > 0.5
        opacity: shown ? 1 : 0
        Behavior on opacity { NumberAnimation { duration: island.railRef._a.dur } }
        Rectangle {
            anchors.verticalCenter: parent.verticalCenter
            x: 0; width: 4
            height: moreMa.containsMouse ? 18 : 0
            radius: 2
            color: Th.railPillBg
            Behavior on height { NumberAnimation { duration: island.railRef._a.dur; easing.type: Easing.OutCubic } }
        }
        Rectangle {
            anchors.centerIn: parent
            width: 44; height: 44
            radius: moreMa.containsMouse ? island.railRef._a.radiusLg : width / 2
            Behavior on radius { NumberAnimation { duration: island.railRef._a.dur } }
            color: moreMa.containsMouse
                ? Th.dmMoreBgHover
                : Th.dmMoreBg
            Behavior on color { ColorAnimation { duration: island.railRef._a.dur } }
            Controls.Label {
                anchors.centerIn: parent
                text: "+" + island.railRef._dmOverflow
                color: Th.dmMoreFg
                font.bold: true
                font.pixelSize: 15
            }
        }
        MouseArea {
            id: moreMa
            anchors.fill: parent
            hoverEnabled: true
            cursorShape: Qt.PointingHandCursor
            onClicked: if (island.railRef.navController) island.railRef.navController.selectSpace("people")
        }
        Controls.ToolTip.text: island.railRef._dmOverflow + " more — open People"
        Controls.ToolTip.visible: moreMa.containsMouse
        Controls.ToolTip.delay: 400
    }
}
