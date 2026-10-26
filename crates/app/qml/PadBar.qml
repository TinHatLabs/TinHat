// PadBar.qml — Pad's bar at the top of the room list (DESIGN §1 "Where Pad appears"; the web's Entry.tsx room-list bar):
// one of my OTHER devices is in a call this device isn't in → "You're in **{room}**'s call on **{device}** · [Pad] [Join]".
// Pad opens the Pad window for that device's call; Join joins it here too (the existing Join). A Pad that can't work is
// greyed: its reason as the tooltip, a click shows it as a toast ("Verify this device to use Pad." with Devices &
// security). Data: AppCore's mPadTargets (pad_ctl.rs refreshTargets: every 3 s from the local room state, no network).
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: pad.listBar.* (defaults = the call join banner's tokens)

Rectangle {
    id: bar
    required property var app
    // the NEWEST call (room) with one of my other devices in it (pad_engine::calls_newest_first = the web's
    // callsNewestFirst: the room whose newest membership is newest; ties by room id), unless this device is in or
    // joining that call itself
    readonly property var entry: {
        var order = (bar.app.mPadTargets && bar.app.mPadTargets.order) ? bar.app.mPadTargets.order : []
        for (var i = 0; i < order.length; i++) {
            var room = String(order[i])
            if ((bar.app.mInCall || bar.app.mConnecting) && bar.app.mCallRoomId === room) continue
            var t = bar.app.padTargetIn(room)
            if (t) return { room: room, t: t }
        }
        return null
    }
    readonly property string roomName: { var n = bar.entry ? String(bar.app.roomNameOf(bar.entry.room) || "") : ""; return n.length > 0 ? n : "a room" }
    function esc(s) { return String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;") }
    visible: bar.app.mLoggedIn === true && bar.entry !== null
    implicitHeight: visible ? col.implicitHeight + 16 : 0
    radius: bar.app.radius
    color: Th.padListBarBg
    border.width: 1
    border.color: Th.padListBarBorder
    ColumnLayout {
        id: col
        anchors.left: parent.left; anchors.right: parent.right
        anchors.verticalCenter: parent.verticalCenter
        anchors.leftMargin: 10; anchors.rightMargin: 8
        spacing: 6
        RowLayout {
            Layout.fillWidth: true
            spacing: 8
            Controls.Label {
                Layout.fillWidth: true
                textFormat: Text.StyledText
                text: bar.entry ? "You're in <b>" + bar.esc(bar.roomName) + "</b>'s call on <b>" + bar.esc(bar.entry.t.name) + "</b>" : ""
                color: Th.padListBarFg
                wrapMode: Text.WordWrap
                maximumLineCount: 2
                elide: Text.ElideRight
            }
        }
        RowLayout {
            Layout.fillWidth: true
            spacing: 6
            Item { Layout.fillWidth: true }
            ThButton {
                app: bar.app
                text: "Pad"
                icon.name: "smartphone-symbolic"
                opacity: bar.entry && bar.entry.t.ready ? 1 : 0.5   // greyed, still clickable: the click explains
                Controls.ToolTip.text: bar.entry && bar.entry.t.ready ? "Control the call on " + bar.entry.t.name + " from here — sounds, mute, volumes" : bar.app.padWhyLine(bar.entry ? bar.entry.t : null)
                Controls.ToolTip.visible: hovered
                Controls.ToolTip.delay: 500
                Accessible.name: bar.entry && !bar.entry.t.ready ? "Pad — " + bar.app.padWhyLine(bar.entry.t) : "Pad — control the call on " + (bar.entry ? bar.entry.t.name : "")
                onClicked: if (bar.entry) bar.app.openPad(bar.entry.room, bar.entry.t)
            }
            GlowButton {
                app: bar.app
                text: bar.app.mConnecting ? "Joining…" : "Join"
                busy: bar.app.mConnecting
                enabled: !bar.app.mConnecting
                onClicked: if (bar.entry) bar.app.callctl.joinCall(bar.entry.room)
            }
        }
    }
}
