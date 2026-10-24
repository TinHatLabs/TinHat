// IncomingCallToast.qml — 10.0: the in-app card for a call that just started in a room you are
// not in. Stacks one card per room, rings (the ringtone stops after 25 s or on any answer), and
// offers Join — or Switch when you are already in another call (leave, then join, in one click).
// Hosted by Main.qml over the whole window; fed by LoginController.incomingCall / incomingCallEnded.
// 11.0 (GPU.md §2.3, R7/R8 — the web's features/callui/Incoming.tsx): the visual ring runs only while `ring` (the
// window is unfocused, no Reduce motion, < 25 s since that card arrived — tinhat_widgets::lowgpu::ring_on): the glow
// blinks in two states at 2 Hz (0.22 / 0.06, set directly) from ONE 250 ms timer for all cards — it was a
// refresh-rate 1.8 s fade that ran for the whole 25 s. Focused: static (the card's entrance is a normal transition);
// hidden: nothing runs, not even the timer (the OS notification and the ringtone carry the alert); Reduce motion: the
// static glow (0.12). A card goes as soon as you are in — or connecting to — that call by ANY route (auto-join, the
// room's call button), not only from its own Join (it used to stay for the whole call offering "Switch").
import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the call.incoming.* tokens

Item {
    id: toast
    property var app: null                     // the shell root: mInCall, startRing/stopRing (colours: Th's call.incoming.*)
    property var calls: []                     // [{ roomId, roomName, who }]
    readonly property bool inOtherCall: !!app && app.mInCall === true
    signal joinRequested(string roomId)
    // the call I am in or connecting to ("" = none): its card goes (R8)
    readonly property string activeCallRoom: (!!app && (app.mInCall === true || app.mConnecting === true)) ? String(app.mCallRoomId || "") : ""
    onActiveCallRoomChanged: if (toast.activeCallRoom.length > 0) toast.dismiss(toast.activeCallRoom)
    // the ring (R7): each card's arrival time, and a clock that moves only while something rings
    property var arrived: ({})
    property double now: Date.now()
    function ringing(roomId) {
        var since = toast.arrived[roomId]
        // the REAL window state (10.8.1): the card rings while another window has the focus, whatever the render state
        return !!toast.app && toast.app.lowgpu.ringOn(since === undefined ? -1 : since, toast.now, Motion.realWinState, Motion.reducedMotion)
    }
    function glow(roomId) {
        if (!toast.app) return 0.12
        var l = toast.app.lowgpu
        if (!toast.ringing(roomId)) return l.num("ringRest")
        return l.ringHigh(toast.arrived[roomId], toast.now) ? l.num("ringHigh") : l.num("ringLow")
    }
    readonly property bool anyRing: {
        var n = toast.now, w = Motion.realWinState, r = Motion.reducedMotion   // dependencies
        for (var i = 0; i < toast.calls.length; i++) if (toast.ringing(toast.calls[i].roomId)) return true
        return false
    }
    // ONE 250 ms timer for all cards (RING_STEP_MS — the one timer exempt from the ≥ 1 s rule), only while one rings
    Timer { id: ringStep; interval: 250; repeat: true; running: toast.anyRing; onTriggered: toast.now = Date.now() }
    Connections {   // a window-state change re-reads the clock first (a stale one would ring a card that is done)
        target: Motion
        function onRealWinStateChanged() { toast.now = Date.now() }
        function onReducedMotionChanged() { toast.now = Date.now() }
    }

    visible: calls.length > 0
    implicitHeight: column.implicitHeight
    height: implicitHeight

    function present(roomId, roomName, who) {
        for (var i = 0; i < calls.length; i++) if (calls[i].roomId === roomId) return
        if (roomId === toast.activeCallRoom) return        // R8: already in (or joining) that call
        var a = Object.assign({}, toast.arrived); a[roomId] = Date.now(); toast.arrived = a
        toast.now = Date.now()
        var next = calls.slice(); next.push({ roomId: roomId, roomName: roomName, who: who })
        calls = next
        // 10.1 (TH-POP-10): already on a call → one notify blip, not a 25 s ringtone into headphones.
        if (app) { if (app.mInCall) app.playUiSound("notify"); else app.startRing(); app.notifyIncomingCall(roomId, roomName, who) }
        ringLimit.restart()
    }
    function dismiss(roomId) {
        var had = calls.some(function (c) { return c.roomId === roomId })
        var next = calls.filter(function (c) { return c.roomId !== roomId })
        calls = next
        toast._forget(roomId)
        if (had && next.length === 0 && app) app.stopRing()
    }
    function _forget(roomId) {
        if (toast.arrived[roomId] === undefined) return
        var a = Object.assign({}, toast.arrived); delete a[roomId]; toast.arrived = a
    }
    function ended(roomId) { dismiss(roomId) }
    function join(roomId) {
        if (app) app.stopRing()
        calls = calls.filter(function (c) { return c.roomId !== roomId })
        toast._forget(roomId)
        toast.joinRequested(roomId)
    }
    Timer { id: ringLimit; interval: 25000; onTriggered: if (app) app.stopRing() }

    ColumnLayout {
        id: column
        width: parent.width
        spacing: Kirigami.Units.smallSpacing
        Repeater {
            model: toast.calls
            delegate: Rectangle {
                required property var modelData
                Layout.fillWidth: true
                implicitHeight: row.implicitHeight + Kirigami.Units.largeSpacing * 2
                radius: 14
                color: Th.callIncomingBg
                border.width: 1
                border.color: Th.callIncomingBorder
                // 10.1 (TH-MOTION-09): it fades in instead of materialising.
                opacity: 0
                Component.onCompleted: opacity = 1
                Behavior on opacity { NumberAnimation { duration: Motion.durSlow } }
                // The accent glow: a 2 Hz two-state blink while it rings unfocused, else static (R7) — the card never moves.
                Rectangle {
                    anchors.fill: parent; radius: parent.radius; color: Th.callIncomingGlow
                    opacity: toast.glow(modelData.roomId)
                }
                RowLayout {
                    id: row
                    anchors { left: parent.left; right: parent.right; verticalCenter: parent.verticalCenter; margins: Kirigami.Units.largeSpacing }
                    spacing: Kirigami.Units.largeSpacing
                    Kirigami.Icon {
                        source: "call-start-symbolic"
                        color: Th.callIncomingIcon
                        Layout.preferredWidth: Kirigami.Units.iconSizes.medium
                        Layout.preferredHeight: Kirigami.Units.iconSizes.medium
                    }
                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: 2
                        Controls.Label {
                            Layout.fillWidth: true
                            text: "Incoming call · " + modelData.roomName
                            color: Th.callIncomingFg
                            font.bold: true
                            elide: Text.ElideRight
                        }
                        Controls.Label {
                            Layout.fillWidth: true
                            text: (modelData.who && modelData.who.length > 0 ? modelData.who + " started a call" : "Someone started a call")
                                  + (toast.inOtherCall ? " — you're in another call" : "")
                            color: Th.callIncomingSubFg
                            font.pointSize: Kirigami.Theme.smallFont.pointSize
                            elide: Text.ElideRight
                        }
                    }
                    GlowButton {   // 10.1 (TH-SHELL-16): the most-seen toast, on the house CTA
                        app: toast.app
                        text: toast.inOtherCall ? "Switch" : "Join"
                        icon.name: "call-start-symbolic"
                        onClicked: toast.join(modelData.roomId)
                    }
                    ThToolButton {
                        app: toast.app
                        iconName: "window-close-symbolic"
                        tip: "Dismiss"
                        onClicked: toast.dismiss(modelData.roomId)
                    }
                }
            }
        }
    }
}
