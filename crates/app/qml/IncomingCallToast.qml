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
    // A15.3 (2026-11-13): every card is past its ring (RING_LIMIT 25 s): the card settles (Main.qml: top right, narrower;
    // the second line goes). The web's Incoming.tsx: the same rule.
    readonly property bool settled: {
        var n = toast.now
        if (toast.calls.length === 0) return false
        for (var i = 0; i < toast.calls.length; i++) {
            var since = toast.arrived[toast.calls[i].roomId]
            if (since === undefined || n - since < 25000) return false
        }
        return true
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
        // a11y round 2 (M3): a screen reader hears the card at once (urgent), not only the ringtone; the focus stays
        // where it is (typing is never interrupted)
        if (app && typeof app.announceIncoming === "function") app.announceIncoming(roomName, who)
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
    // P8 (2026-11-13): a DM ring can be declined (the web's Incoming.tsx: the same button, the same word). The card and
    // the ring go here; ringglue.rs tells the caller ("<name> declined"), my other devices and the server.
    function decline(roomId) {
        dismiss(roomId)
        if (app && app.login) app.login.declineIncoming(roomId)
    }
    // (lab A 2026-11-14: m.direct can be missing on the invitee's side; the Decline's own rule is two joined people)
    function declinable(roomId) { return !!app && typeof app.twoPersonCall === "function" && app.twoPersonCall(roomId) }
    // A15.2/A15.6: the card's two lines (callannounce.rs incoming_card = the web's incomingCard, shared vectors); the
    // people in the call from the room list's row (the join banner's source), me left out
    function cardLines(c) {
        var names = []
        var rooms = (app && app.mNavRooms) ? app.mNavRooms : []
        for (var i = 0; i < rooms.length; i++) {
            if (!rooms[i] || rooms[i].id !== c.roomId) continue
            var ps = rooms[i].participants || []
            for (var j = 0; j < ps.length; j++) if (ps[j].mxid !== app.mUserId) names.push(String(ps[j].name || ps[j].mxid || ""))
            break
        }
        var fallback = { title: "Incoming call · " + c.roomName, sub: c.who && c.who.length > 0 ? c.who + " started a call" : "Someone started a call" }
        if (!app || typeof app.incomingCardJson !== "function") return fallback
        try { return JSON.parse(app.incomingCardJson(toast.declinable(c.roomId), String(c.roomName || ""), String(c.who || ""), JSON.stringify(names), toast.inOtherCall)) }
        catch (e) { return fallback }
    }
    function join(roomId) {
        if (app) app.stopRing()
        calls = calls.filter(function (c) { return c.roomId !== roomId })
        toast._forget(roomId)
        toast.joinRequested(roomId)
    }
    Timer { id: ringLimit; interval: 25000; onTriggered: { if (app) app.stopRing(); toast.now = Date.now() } }   // (+ A15.3: settle)

    ColumnLayout {
        id: column
        width: parent.width
        spacing: Kirigami.Units.smallSpacing
        Repeater {
            model: toast.calls
            delegate: Rectangle {
                required property var modelData
                Accessible.role: Accessible.Grouping
                Accessible.name: "Incoming call · " + modelData.roomName
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
                        // A15.2/A15.6 (2026-11-13): a DM names the caller; a group says who is in the call now
                        readonly property var lines: toast.cardLines(modelData)
                        Controls.Label {
                            textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                            Layout.fillWidth: true
                            text: parent.lines.title
                            color: Th.callIncomingFg
                            font.bold: true
                            elide: Text.ElideRight
                        }
                        Controls.Label {
                            textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                            Layout.fillWidth: true
                            text: parent.lines.sub
                            visible: !toast.settled   // A15.3: one line once settled
                            color: Th.callIncomingSubFg
                            font.pointSize: Kirigami.Theme.smallFont.pointSize * Th.textScale
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
                        visible: toast.declinable(modelData.roomId)
                        iconName: "call-stop-symbolic"
                        tip: "Decline"
                        onClicked: toast.decline(modelData.roomId)
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
