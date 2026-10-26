// PadChip.qml — on the device IN the call (docs/PAD.md security rule 8 "Visible"; DESIGN §1 "On the receiver"): while
// one of my devices drives this call from its Pad, the call's status strip shows "📱 Pad · <device> ×". The web's
// features/pad/PadChip.tsx is the same element: the status strip's chip shape (the "N hidden ▾" chip's), the pad.chip.*
// tokens (defaults = call.hiddenChip.*), the device name bold, and × as its own button — it ends that device's Pad and
// refuses it for the rest of this call (PadController.refuseDevice). Hovering the chip says what it means.
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: pad.chip.*

Rectangle {
    id: chip
    property var app: null          // the Main.qml root
    property string device: ""
    property string name: ""
    signal disconnect()
    function esc(s) { return String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;") }
    readonly property real fullW: Math.ceil(8 + emoji.implicitWidth + 5 + label.implicitWidth + 5 + 20 + 3)
    implicitWidth: fullW
    implicitHeight: Math.max(label.implicitHeight + 4, 22) + 2
    // in the call header's RowLayout: its full width when there is room (at most the web's 260 px), the name elides first
    Layout.fillWidth: true
    Layout.preferredWidth: Math.min(260, fullW)
    Layout.maximumWidth: Math.min(260, fullW)
    Layout.minimumWidth: Math.min(fullW, 8 + emoji.implicitWidth + 5 + 40 + 5 + 20 + 3)
    radius: height / 2
    color: Th.padChipBg
    border.width: 1
    border.color: Th.padChipBorder
    HoverHandler { id: chipHover }
    Controls.ToolTip.visible: chipHover.hovered
    Controls.ToolTip.delay: 300
    // rich text on purpose, with the name escaped: a device name is shown literally, never as markup
    Controls.ToolTip.text: "<html>" + chip.esc(chip.name) + " is controlling this call with Pad (sounds, mute, volumes). × disconnects it for this call.</html>"
    Accessible.role: Accessible.StaticText
    Accessible.name: "Pad · " + chip.name
    Row {
        anchors.left: parent.left
        anchors.leftMargin: 8
        anchors.verticalCenter: parent.verticalCenter
        spacing: 5
        Controls.Label {
            id: emoji
            anchors.verticalCenter: parent.verticalCenter
            text: "📱"
            color: Th.padChipFg
            font.pointSize: Kirigami.Theme.smallFont.pointSize
        }
        Controls.Label {
            id: label
            anchors.verticalCenter: parent.verticalCenter
            width: Math.max(0, Math.min(implicitWidth, chip.width - 8 - emoji.implicitWidth - 5 - 5 - 20 - 3))
            textFormat: Text.StyledText
            text: "Pad · <b>" + chip.esc(chip.name) + "</b>"
            elide: Text.ElideRight
            color: Th.padChipFg
            font.pointSize: Kirigami.Theme.smallFont.pointSize
        }
        Controls.AbstractButton {   // × — its own button, as on the web: Tab reaches it, Space (Qt's) or Enter presses it
            id: xBtn
            anchors.verticalCenter: parent.verticalCenter
            width: 20; height: 20
            focusPolicy: Qt.TabFocus
            hoverEnabled: true
            Accessible.name: "Disconnect the Pad on " + chip.name + " for this call"
            onClicked: chip.disconnect()
            // a web button also takes Enter; Qt's buttons take Space (the platform's press keys) — accepted here, so a
            // platform that already presses on Enter doesn't disconnect twice
            Keys.onReturnPressed: (e) => { e.accepted = true; chip.disconnect() }
            Keys.onEnterPressed: (e) => { e.accepted = true; chip.disconnect() }
            HoverHandler { cursorShape: Qt.PointingHandCursor }
            background: Rectangle {
                radius: 10
                color: xBtn.hovered || xBtn.down ? Th.padChipBgHover : "transparent"
                border.width: xBtn.visualFocus ? 2 : 0
                border.color: Th.buttonIconRingFocus   // the icon buttons' keyboard focus ring
            }
            contentItem: Item {
                ThIcon {
                    anchors.centerIn: parent
                    width: 12; height: 12
                    app: chip.app
                    name: "window-close-symbolic"
                    color: Th.padChipFg
                    isMask: true
                }
            }
        }
    }
}
