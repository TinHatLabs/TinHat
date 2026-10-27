// PadWindow.qml — Pad on the desktop: this device as a remote for ANOTHER of my devices' call (docs/PAD.md; DESIGN §1
// "On the controller"; the web's features/pad/PadScreen.tsx desktop panel — the same structure, words and tokens). The
// receiver does the work: everything here is a command to it (pad_ctl.rs → pad_net.rs, Olm to exactly that device) and
// everything shown is its state.
//   header   "● Pad · {room}" / "on {device} ▾" (the chooser only with ≥ 2 of my devices in the call) + the link status
//   notice   the receiver's refusal, in its words (whyText) — the call screen's warn / bad banner (Try again / ×)
//   my row   Mute/Unmute · Deafen/Undeafen · BRB/I'm back · Stop sound · Leave (wide, red; "Leave the call on {device}?"
//            asked HERE); push to talk there = "Push to talk" (inert) + its line; a moderator's lock = "Muted for all"
//   sounds   "Soundboard — plays on {device}, into the call": Favourites (the Soundboard pad's list, starred order),
//            This room, Recent on {device}; the playing one glows (a tap stops it); the role line when roles turned it off
//   people   "In the call · n": talking ring, mic-off, 💤, their non-default volumes; a click → their three volumes
// No call to show (connecting / not reaching / it ended): one centred state instead, with Try again / Take it back / Close.
// Presses are optimistic: the control moves at once with a thin ring in the focus colour until the receiver answers.
// The soundboard window's place and size (680 wide at most, 120 px above the window's bottom). Colours: Pad's own pad.*
// tokens (contract tools/pad-contract.py; defaults = the tokens these pieces were painted with before, 0 px) for its own
// elements; the pieces that ARE existing elements (CallButton, the sound cards, the volume menu, the notice banner) keep
// theirs.
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import QtQuick.Window
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the pad.* / soundboard.* / callConnBanner.* / callVolume.* tokens

Controls.Popup {
    id: pad
    required property var app
    parent: Controls.Overlay.overlay
    modal: false
    dim: false
    focus: true
    closePolicy: Controls.Popup.CloseOnEscape
    width: Math.min(680, (parent ? parent.width : 680) - 40)
    height: Math.min((parent ? parent.height : 780) - 140, Math.max(320, shell.implicitHeight + 8))
    x: parent ? Math.round((parent.width - width) / 2) : 0
    y: parent ? Math.max(16, parent.height - height - 120) : 0
    padding: 0

    // ── which session ──
    property string roomId: ""
    property string deviceId: ""
    function openFor(room, device) {
        pad.isDemo = false
        pad.roomId = room
        pad.deviceId = device
        pad.optimistic = ({})
        pad.notice = ({ code: "", op: "" })
        pad.app.padCtl.openPad(room, device)
        pad.app.padCtl.loadSounds(room)
        if (pad.app.soundboardCtl) pad.app.soundboardCtl.refreshPersonal()
        if (!pad.opened) pad.open()
    }
    function retry() { if (!pad.isDemo) pad.openFor(pad.roomId, pad.deviceId) }
    onClosed: { if (!pad.isDemo) pad.app.padCtl.closePad(); pad.optimistic = ({}); pad.notice = ({ code: "", op: "" }) }
    // pings while it is seen (vis:false = the receiver stops its pushes; the next ping's answer catches up)
    readonly property bool seen: pad.opened && !Motion.hidden
    onSeenChanged: pad.app.padCtl.setVisible(pad.seen)

    readonly property var remote: pad.app.mPadRemote
    // the session follows the receiver into another call (pad_engine.rs: room + gen from its state): so does the window
    readonly property bool mine: !!pad.remote && pad.remote.device === pad.deviceId
    readonly property string followRoom: pad.mine && !pad.isDemo && pad.remote.state && pad.remote.state.call ? String(pad.remote.state.call.room) : ""
    onFollowRoomChanged: if (pad.followRoom.length > 0 && pad.followRoom !== pad.roomId) { pad.roomId = pad.followRoom; pad.app.padCtl.loadSounds(pad.roomId) }
    readonly property var st: pad.mine && pad.remote.state ? pad.remote.state : null
    // opening | open | unreachable | ended (the web's link)
    readonly property string link: {
        if (!pad.mine) return "ended"
        var e = pad.remote.ended ? String(pad.remote.ended) : ""
        if (e === "unreachable" || pad.remote.reach === "unreachable") return "unreachable"
        if (e.length > 0) return "ended"
        if (pad.remote.opened !== true) return "opening"
        // the receiver isn't in a call right now (it left; the session lives on and follows its next call): the web's
        // link is "ended" here too — grey dot + "Not connected", not "Connected" (phase-3 parity)
        if (pad.st && pad.st.call === null) return "ended"
        return "open"
    }
    readonly property var view: (pad.link === "open" || pad.link === "unreachable") && pad.st && pad.st.call ? pad.st.call : null
    readonly property var me: pad.view ? pad.view.me : null
    readonly property bool live: pad.link === "open" && pad.view !== null
    readonly property var ops: pad.mine && pad.remote.ops ? pad.remote.ops : []
    function has(op) { return pad.ops.indexOf(op) !== -1 }
    readonly property var targets: pad.app.padTargetsIn(pad.roomId)
    readonly property var names: (pad.app.mPadTargets && pad.app.mPadTargets.names) ? pad.app.mPadTargets.names : ({})
    function nameOf(d) { return (d && pad.names[d]) ? String(pad.names[d]) : String(d || "") }
    readonly property string deviceName: pad.nameOf(pad.deviceId)
    readonly property string roomName: { var n = String(pad.app.roomNameOf(pad.roomId) || ""); return n.length > 0 ? n : "the call" }
    function why(code, op) { return String(pad.app.padCtl.whyText(code, pad.deviceName, op || "")) }
    function esc(s) { return String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;") }

    // ── the notice: the receiver's last word (a refusal, the session's end, the call's end) ──
    property var notice: ({ code: "", op: "" })
    function say(code, op) { pad.notice = { code: String(code || ""), op: String(op || "") } }
    readonly property string noticeCode: {
        if (pad.link === "unreachable") return "unreachable"
        if (pad.link === "ended" && pad.mine && pad.remote.ended && String(pad.remote.ended) !== "closed-here") return String(pad.remote.ended)
        if (pad.notice.code.length > 0) return pad.notice.code
        if (pad.st && !pad.st.call && pad.link !== "opening") return String(pad.st.why || "").length > 0 ? String(pad.st.why) : "not-in-call"
        return ""
    }
    readonly property string noticeText: pad.noticeCode.length > 0 ? pad.why(pad.noticeCode, pad.notice.op) : ""

    // ── optimistic presses: key → {v, seq, at}; cleared by the answer (acked / failed) or after 7 s ──
    property var optimistic: ({})
    function opt(key, real) { var o = pad.optimistic[key]; return o !== undefined ? o.v : real }
    function pendingKey(key) { return pad.optimistic[key] !== undefined }
    function send(key, value, cmd) {
        var seq = pad.app.padCtl.send(JSON.stringify(cmd))
        if (key) {
            var o = {}
            for (var k in pad.optimistic) o[k] = pad.optimistic[k]
            o[key] = { v: value, seq: seq, at: Date.now() }
            pad.optimistic = o
        }
        return seq
    }
    function _drop(pred) {
        var o = {}, changed = false
        for (var k in pad.optimistic) { if (pred(pad.optimistic[k])) changed = true; else o[k] = pad.optimistic[k] }
        if (changed) pad.optimistic = o
    }
    Connections {
        target: pad.app.padCtl
        ignoreUnknownSignals: true
        function onAcked(seq, ok, why, op) {
            pad._drop(function (o) { return o.seq === seq })
            if (!ok && pad.why(why, op).length > 0) pad.say(why, op)
            else if (ok && op === "leave") pad.say("ended", "")   // my own Leave: "{device} left the call."
            else if (ok && pad.notice.code.length > 0 && pad.notice.code !== "ended") pad.say("", "")
        }
        function onFailed(seq, op) { pad._drop(function (o) { return o.seq === seq }) }
        function onEnded(why) { pad.optimistic = ({}) }
    }
    Timer {   // stale presses (a paced tap that was replaced, a lost answer) + the call room's people / sounds
        interval: 3000; repeat: true; running: pad.opened
        onTriggered: {
            var now = Date.now()
            pad._drop(function (o) { return now - o.at > 7000 })
            if (!pad.isDemo) pad.app.padCtl.loadSounds(pad.roomId)
        }
    }

    // ── what is shown ──
    readonly property string mic: pad.me ? String(pad.me.mic) : "on"   // on | muted | server | ptt (server > ptt > muted)
    readonly property bool micMuted: pad.opt("mic", pad.mic === "muted")
    readonly property bool deaf: pad.opt("deaf", !!pad.me && pad.me.deaf === true)
    readonly property bool brb: pad.opt("brb", !!pad.me && pad.me.brb === true)
    readonly property string playing: pad.opt("sound", (pad.me && pad.me.sound && pad.me.sound.url) ? String(pad.me.sound.url) : "")
    readonly property bool gateOk: !pad.me || !pad.me.gate || pad.me.gate.allowed !== false
    readonly property var favourites: {
        if (!pad.app.soundboardCtl) return []
        try { return JSON.parse(String(pad.app.soundboardCtl.padSounds(JSON.stringify(pad.app.mPersonalSounds || []), 12))) } catch (e) { return [] }
    }
    readonly property var roomSounds: (pad.app.mPadSounds && pad.app.mPadSounds.room === pad.roomId && pad.app.mPadSounds.roomSounds)
        ? pad.app.mPadSounds.roomSounds.slice(0, 8) : []
    readonly property var recent: pad.view && pad.view.recent ? pad.view.recent : []
    readonly property var peopleInfo: {
        var m = {}, l = (pad.app.mPadSounds && pad.app.mPadSounds.room === pad.roomId && pad.app.mPadSounds.people) ? pad.app.mPadSounds.people : []
        for (var i = 0; i < l.length; i++) if (l[i] && l[i].mxid) m[l[i].mxid] = l[i]
        return m
    }
    function personName(u) { var p = pad.peopleInfo[u]; return p && p.name && p.name !== u ? String(p.name) : String(pad.app.localpart(u)) }
    function tapSound(url) {
        if (!pad.live) return 0
        if (url === pad.playing) return pad.send("sound", "", { op: "sound.stop" })
        return pad.send("sound", url, { op: "sound.play", url: url })
    }
    // my row's controls: what each shows, whether it takes a press, and the press (one place: the buttons below and
    // the --tinhat-paddrive gate hook run the same functions)
    readonly property string micCaption: pad.mic === "ptt" ? "Push to talk" : pad.mic === "server" ? "Muted for all" : (pad.micMuted ? "Unmute" : "Mute")
    readonly property bool micUsable: pad.live && pad.has("mic") && pad.mic !== "ptt" && pad.mic !== "server"
    readonly property bool deafUsable: pad.live && pad.has("deafen")
    readonly property bool brbUsable: pad.live && pad.has("brb")
    readonly property bool stopUsable: pad.live && pad.has("sound.stop") && pad.playing.length > 0
    readonly property bool leaveUsable: pad.live && pad.has("leave")
    readonly property bool soundsUsable: pad.live && pad.gateOk && pad.has("sound.play")
    function pressMic() { return pad.send("mic", !pad.micMuted, { op: "mic", muted: !pad.micMuted }) }
    function pressDeafen() { return pad.send("deaf", !pad.deaf, { op: "deafen", on: !pad.deaf }) }
    function pressBrb() { return pad.send("brb", !pad.brb, { op: "brb", on: !pad.brb }) }
    function pressStop() { return pad.send("sound", "", { op: "sound.stop" }) }
    function confirmLeave() { return pad.send("", true, { op: "leave" }) }

    background: Rectangle {
        color: Th.padPanelBg
        radius: 16
        border.width: 1
        border.color: Th.padPanelBorder
    }

    contentItem: ColumnLayout {
        id: shell
        spacing: 0
        // ---- header: ● Pad · room / on device ▾ · status ----
        RowLayout {
            Layout.fillWidth: true
            Layout.leftMargin: 16; Layout.rightMargin: 10; Layout.topMargin: 12; Layout.bottomMargin: 8
            spacing: 10
            ColumnLayout {
                Layout.fillWidth: true
                spacing: 2
                RowLayout {
                    spacing: 7
                    Rectangle {
                        Layout.alignment: Qt.AlignVCenter
                        implicitWidth: 9; implicitHeight: 9; radius: 4.5
                        color: pad.link === "open" ? Th.padDotLive : Th.padDot
                        Behavior on color { ColorAnimation { duration: Motion.dur } }
                        HoverHandler { id: dotHover }
                        Controls.ToolTip.visible: dotHover.hovered
                        Controls.ToolTip.text: pad.link === "open" ? "Connected" : pad.link === "opening" ? "Connecting to " + pad.deviceName + "…"
                                             : pad.link === "unreachable" ? "Not reaching " + pad.deviceName : "Not connected"
                    }
                    Controls.Label {
                        text: "Pad"
                        color: Th.soundboardTitleFg; font.bold: true
                        font.pointSize: Kirigami.Theme.defaultFont.pointSize + 2
                    }
                    Controls.Label { text: "·"; color: Th.soundboardSectionHintFg; font.pointSize: Kirigami.Theme.defaultFont.pointSize + 2 }
                    Controls.Label {
                        textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                        Layout.fillWidth: true
                        Layout.minimumWidth: 0
                        text: pad.roomName
                        color: roomHover.hovered ? Th.accentBright : Th.soundboardTitleFg
                        font.bold: true
                        font.pointSize: Kirigami.Theme.defaultFont.pointSize + 2
                        elide: Text.ElideRight
                        HoverHandler { id: roomHover; cursorShape: Qt.PointingHandCursor }
                        TapHandler { onTapped: if (pad.roomId.length > 0 && !pad.isDemo) pad.app.openRoomId(pad.roomId) }
                        Controls.ToolTip.visible: roomHover.hovered
                        Controls.ToolTip.text: "Open " + pad.roomName
                        Controls.ToolTip.delay: 500
                    }
                }
                RowLayout {
                    spacing: 8
                    Controls.AbstractButton {
                        id: chooser
                        readonly property bool several: pad.targets.length > 1
                        implicitWidth: chooserText.implicitWidth
                        implicitHeight: chooserText.implicitHeight
                        enabled: chooser.several
                        hoverEnabled: true
                        focusPolicy: chooser.several ? Qt.TabFocus : Qt.NoFocus
                        Accessible.name: "Which device: " + pad.deviceName
                        contentItem: Controls.Label {
                            id: chooserText
                            textFormat: Text.StyledText
                            text: "on <b>" + pad.esc(pad.deviceName) + "</b>" + (chooser.several ? "  ▾" : "")
                            color: chooser.hovered ? Th.accentBright : Th.soundboardSectionHintFg
                            font.underline: chooser.visualFocus
                        }
                        onClicked: deviceMenu.popup(chooser, 0, chooser.height + 4)
                        ThMenu {
                            id: deviceMenu
                            MenuSectionLabel { app: pad.app; text: "Control the call on…" }
                            Repeater {
                                // ready devices first (padTargetsIn sorts them), the rest with why they can't
                                model: pad.targets
                                delegate: Controls.MenuItem {
                                    required property var modelData
                                    text: LinkGate.plain(modelData.ready ? String(modelData.name) : String(modelData.name) + " — " + pad.app.padWhyLine(modelData))
                                    enabled: modelData.ready === true
                                    checkable: true
                                    checked: modelData.device === pad.deviceId
                                    onTriggered: if (modelData.device !== pad.deviceId) pad.openFor(pad.roomId, modelData.device)
                                }
                            }
                        }
                    }
                    Rectangle { visible: pad.link !== "open"; implicitWidth: 1; implicitHeight: 14; color: Th.callStatusBarBorder }
                    Controls.Label {
                        visible: pad.link !== "open"
                        text: pad.link === "opening" ? "Connecting…" : pad.link === "unreachable" ? "Not reaching it" : "Not connected"
                        color: pad.link === "unreachable" ? Th.callConnBannerTitleFg : Th.soundboardSectionHintFg
                        font.pointSize: Kirigami.Theme.smallFont.pointSize
                    }
                }
            }
            ThToolButton {
                Layout.alignment: Qt.AlignTop
                app: pad.app
                iconName: "window-close-symbolic"
                tip: "Close the Pad (Esc)"
                onClicked: pad.close()
            }
        }
        // ---- the notice: the call screen's warn / bad banner ----
        Rectangle {
            id: noticeBar
            Layout.fillWidth: true
            Layout.leftMargin: 14; Layout.rightMargin: 14; Layout.bottomMargin: 8
            visible: pad.view !== null && pad.noticeText.length > 0
            implicitHeight: noticeRow.implicitHeight + 14
            radius: pad.app.radiusSm
            readonly property bool bad: pad.link === "unreachable"
            color: noticeBar.bad ? Th.callConnBannerBadBg : Th.callConnBannerWarnBg
            border.width: 1
            border.color: noticeBar.bad ? Th.callConnBannerBadBorder : Th.callConnBannerWarnBorder
            RowLayout {
                id: noticeRow
                anchors.fill: parent
                anchors.leftMargin: 12; anchors.rightMargin: 6
                spacing: 8
                Controls.Label {
                    textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                    Layout.fillWidth: true
                    text: pad.noticeText
                    color: Th.callConnBannerTitleFg
                    wrapMode: Text.WordWrap
                }
                ThButton {
                    visible: pad.link === "unreachable"
                    app: pad.app
                    text: "Try again"
                    onClicked: pad.retry()
                }
                ThToolButton {
                    visible: pad.link !== "unreachable"
                    app: pad.app
                    iconName: "window-close-symbolic"
                    iconColor: Th.callConnBannerTitleFg
                    tip: "Dismiss"
                    onClicked: pad.say("", "")
                }
            }
        }

        // ---- no call to show: connecting / not reaching / it ended ----
        ColumnLayout {
            visible: pad.view === null
            Layout.fillWidth: true
            Layout.topMargin: 24; Layout.bottomMargin: 28
            spacing: 12
            ThBusyIndicator {
                Layout.alignment: Qt.AlignHCenter
                visible: pad.link === "opening"
                running: visible
                size: 36
            }
            ThIcon {
                Layout.alignment: Qt.AlignHCenter
                visible: pad.link !== "opening"
                Layout.preferredWidth: 40; Layout.preferredHeight: 40
                app: pad.app
                name: pad.link === "unreachable" ? "network-disconnect-symbolic" : "call-stop-symbolic"
                color: Th.soundboardEmptyFg
                isMask: true
            }
            Controls.Label {
                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                Layout.fillWidth: true
                Layout.leftMargin: 24; Layout.rightMargin: 24
                horizontalAlignment: Text.AlignHCenter
                wrapMode: Text.WordWrap
                text: pad.link === "opening" ? "Connecting to " + pad.deviceName + "…"
                    : pad.link === "unreachable" ? "Not reaching " + pad.deviceName
                    : (pad.noticeText.length > 0 ? pad.noticeText : pad.deviceName + " isn't in a call any more.")
                color: Th.soundboardTitleFg
                font.bold: true
            }
            Controls.Label {
                visible: pad.link === "unreachable"
                Layout.fillWidth: true
                horizontalAlignment: Text.AlignHCenter
                wrapMode: Text.WordWrap
                text: "It may be asleep, offline, or not running Tin Hat any more."
                color: Th.soundboardSectionHintFg
                font.pointSize: Kirigami.Theme.smallFont.pointSize
            }
            RowLayout {
                Layout.alignment: Qt.AlignHCenter
                visible: pad.link !== "opening"
                spacing: 8
                ThButton {
                    visible: pad.link === "unreachable" || pad.noticeCode === "replaced"
                    app: pad.app
                    highlighted: true
                    text: pad.noticeCode === "replaced" ? "Take it back" : "Try again"
                    onClicked: pad.retry()
                }
                ThButton {
                    visible: pad.link !== "unreachable"
                    app: pad.app
                    text: "Close"
                    onClicked: pad.close()
                }
            }
        }

        Controls.ScrollView {
            id: scroller
            visible: pad.view !== null
            Layout.fillWidth: true
            Layout.fillHeight: true
            Layout.preferredHeight: visible ? body.implicitHeight : 0
            contentWidth: availableWidth
            Controls.ScrollBar.horizontal.policy: Controls.ScrollBar.AlwaysOff
            ColumnLayout {
                id: body
                width: scroller.availableWidth
                spacing: 6
                opacity: pad.live ? 1 : 0.6
                readonly property int count: pad.view ? pad.view.people.length + (pad.view.more || 0) : 0

                // ---- my row: the call bar's buttons in the call bar's frame ----
                Rectangle {
                    Layout.fillWidth: true
                    Layout.leftMargin: 14; Layout.rightMargin: 14; Layout.topMargin: 4
                    implicitHeight: myRow.implicitHeight + 22
                    radius: 16
                    color: Th.padMyRowBg
                    border.width: 1
                    border.color: Th.padMyRowBorder
                    RowLayout {
                        id: myRow
                        anchors.centerIn: parent
                        spacing: 18
                        PadAction {
                            glyph: pad.mic === "ptt" ? "audio-input-microphone-symbolic"
                                 : (pad.mic === "server" || pad.micMuted) ? "microphone-sensitivity-muted-symbolic" : "audio-input-microphone-symbolic"
                            caption: pad.micCaption
                            danger: pad.mic === "server" || (pad.mic !== "ptt" && pad.micMuted)
                            pending: pad.pendingKey("mic")
                            usable: pad.micUsable
                            tip: pad.mic === "ptt" ? pad.why("ptt", "mic")
                               : pad.mic === "server" ? pad.why("server-muted", "mic")
                               : (pad.micMuted ? "Unmute — " + pad.deviceName + "'s mic goes live" : "Mute")
                            onActivated: pad.pressMic()
                        }
                        PadAction {
                            glyph: pad.deaf ? "audio-volume-muted-symbolic" : "audio-volume-high-symbolic"
                            caption: pad.deaf ? "Undeafen" : "Deafen"
                            danger: pad.deaf
                            pending: pad.pendingKey("deaf")
                            usable: pad.deafUsable
                            tip: pad.deaf ? "Undeafen" : "Deafen — mute everything for you"
                            onActivated: pad.pressDeafen()
                        }
                        PadAction {
                            glyph: "appointment-missed-symbolic"
                            caption: pad.brb ? "I'm back" : "BRB"
                            lit: pad.brb
                            pending: pad.pendingKey("brb")
                            usable: pad.brbUsable
                            tip: pad.brb ? "I'm back" : "Be right back"
                            onActivated: pad.pressBrb()
                        }
                        PadAction {
                            glyph: "media-playback-stop"
                            caption: "Stop sound"
                            usable: pad.stopUsable
                            tip: "Stop the sound (for everyone)"
                            onActivated: pad.pressStop()
                        }
                        PadAction {
                            glyph: "call-stop-symbolic"
                            caption: "Leave"
                            danger: true
                            wide: true
                            usable: pad.leaveUsable
                            tip: "Leave the call on " + pad.deviceName
                            onActivated: leaveConfirm.open()
                        }
                    }
                }
                Controls.Label {
                    textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                    Layout.fillWidth: true
                    Layout.leftMargin: 16; Layout.rightMargin: 16
                    visible: pad.mic === "ptt"
                    horizontalAlignment: Text.AlignHCenter
                    wrapMode: Text.WordWrap
                    text: pad.why("ptt", "mic")
                    color: Th.callStatusBarCountFg
                    font.pointSize: Kirigami.Theme.smallFont.pointSize
                }

                // ---- the soundboard ----
                SectionHead { title: "Soundboard"; hint: "plays on " + pad.deviceName + ", into the call"; Layout.topMargin: 8 }
                SubHead { title: "Favourites" }
                SoundGrid { sounds: pad.favourites; visible: pad.favourites.length > 0 }
                Controls.Label {
                    visible: pad.favourites.length === 0
                    Layout.fillWidth: true; Layout.leftMargin: 16; Layout.rightMargin: 16
                    text: "Star sounds in the soundboard to pin them here."
                    color: Th.soundboardEmptyFg
                    font.pointSize: Kirigami.Theme.smallFont.pointSize
                }
                SubHead { visible: pad.roomSounds.length > 0; title: "This room" }
                SoundGrid { sounds: pad.roomSounds; visible: pad.roomSounds.length > 0 }
                SubHead { visible: pad.recent.length > 0; title: "Recent on " + pad.deviceName }
                SoundGrid { sounds: pad.recent; visible: pad.recent.length > 0 }
                Controls.Label {
                    textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                    visible: !!pad.me && !pad.gateOk
                    Layout.fillWidth: true; Layout.leftMargin: 16; Layout.rightMargin: 16
                    text: pad.me && pad.me.gate && String(pad.me.gate.hint || "").length > 0 ? String(pad.me.gate.hint) : pad.why("off-role", "")
                    color: Th.soundboardStatusFgError
                    wrapMode: Text.WordWrap
                    font.pointSize: Kirigami.Theme.smallFont.pointSize
                }

                // ---- people ----
                SectionHead {
                    Layout.topMargin: 8
                    title: body.count > 0 ? "In the call · " + body.count : "In the call"
                    hint: body.count > 0 ? "tap someone for their volumes" : ""
                }
                Controls.Label {
                    visible: body.count === 0
                    Layout.leftMargin: 16
                    text: "Nobody else is in the call yet."
                    color: Th.soundboardEmptyFg
                    font.pointSize: Kirigami.Theme.smallFont.pointSize
                }
                GridLayout {
                    Layout.fillWidth: true
                    Layout.leftMargin: 10; Layout.rightMargin: 10
                    columns: 2
                    columnSpacing: 6
                    rowSpacing: 2
                    Repeater {
                        model: pad.view ? pad.view.people : []
                        delegate: PersonRow { required property var modelData; who: modelData }
                    }
                }
                Controls.Label {
                    textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                    visible: !!pad.view && (pad.view.more || 0) > 0
                    Layout.leftMargin: 16
                    text: "+" + (pad.view ? pad.view.more : 0) + " more"
                    color: Th.soundboardSectionHintFg
                    font.pointSize: Kirigami.Theme.smallFont.pointSize
                }
                Item { Layout.preferredHeight: 10 }
            }
        }
    }

    // ── pieces ──
    component SectionHead: RowLayout {
        property string title: ""
        property string hint: ""
        Layout.fillWidth: true
        Layout.leftMargin: 16; Layout.rightMargin: 16
        Layout.topMargin: 4
        spacing: 8
        Controls.Label { textFormat: Text.PlainText; text: parent.title; color: Th.soundboardSectionFg; font.bold: true }
        Controls.Label { textFormat: Text.PlainText; text: parent.hint; color: Th.soundboardSectionHintFg; font.pointSize: Kirigami.Theme.smallFont.pointSize; Layout.fillWidth: true; elide: Text.ElideRight }
    }
    component SubHead: Controls.Label {
        textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
        property string title: ""
        Layout.leftMargin: 16
        Layout.topMargin: 2
        text: title
        color: Th.soundboardSectionFg
        font.weight: Font.DemiBold
    }
    component SoundGrid: GridLayout {
        id: grid
        property var sounds: []
        Layout.fillWidth: true
        Layout.leftMargin: 14; Layout.rightMargin: 14
        columns: 4
        columnSpacing: 8
        rowSpacing: 8
        Repeater {
            model: grid.sounds
            delegate: PadSoundButton {
                required property var modelData
                readonly property string url: String(modelData.url || modelData.sourceUrl || "")
                Layout.fillWidth: true
                Layout.preferredWidth: 120
                // every tile one column wide, like the web's grid (a lone room sound no longer spans the row). From the
                // window's own width (fixed: min(680, the window − 40); padding 0), never the grid's: a layout's maximum
                // is its children's, so a child capped by the grid's width would shrink the grid to nothing.
                Layout.maximumWidth: Math.max(60, (pad.width - 28 - grid.columnSpacing * (grid.columns - 1)) / grid.columns)
                name: String(modelData.name || "Sound")
                emoji: String(modelData.emoji || "")
                playing: url.length > 0 && url === pad.playing
                pending: playing && pad.pendingKey("sound")
                enabled: pad.soundsUsable
                onClicked: pad.tapSound(url)
            }
        }
    }
    component PadAction: ColumnLayout {
        id: act
        property string glyph: ""
        property string caption: ""
        property string tip: ""
        property bool lit: false
        property bool danger: false
        property bool pending: false
        property bool usable: true
        property bool wide: false
        signal activated()
        spacing: 5
        CallButton {
            Layout.alignment: Qt.AlignHCenter
            app: pad.app
            implicitWidth: act.wide ? 72 : 50
            iconName: act.glyph
            active: act.lit
            danger: act.danger
            live: act.usable
            tip: act.tip
            ringColor: act.pending ? Th.padPendingRing : "transparent"
            onClicked: act.activated()
        }
        Controls.Label {
            textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
            Layout.alignment: Qt.AlignHCenter
            text: act.caption
            color: Th.callStatusBarCountFg
            font.pointSize: Kirigami.Theme.smallFont.pointSize
        }
    }
    component PersonRow: Rectangle {
        id: pr
        property var who: ({ u: "", talk: false, micOff: false, away: false })
        readonly property var info: pad.peopleInfo[pr.who.u] || null
        readonly property string name: pad.personName(pr.who.u)
        // the web's custom(): non-default volumes only — "mic 60% · sounds muted"
        readonly property string volLine: {
            var bits = [], labels = [["mic", "mic"], ["screenshare", "share"], ["music", "sounds"]]
            var v = pr.who.vol || {}, m = pr.who.mute || []
            for (var i = 0; i < labels.length; i++) {
                var c = labels[i][0], l = labels[i][1]
                if (m.indexOf(c) !== -1) bits.push(l + " muted")
                else if (typeof v[c] === "number" && v[c] !== 1) bits.push(l + " " + Math.round(v[c] * 100) + "%")
            }
            return bits.join(" · ")
        }
        Layout.fillWidth: true
        Layout.preferredWidth: 1
        // one column of two, like the web's people list (one person no longer spans both) — from the window's width
        // (the margins 10 + 10 and the 6 between), never the grid's (see SoundGrid)
        Layout.maximumWidth: Math.max(120, (pad.width - 26) / 2)
        implicitHeight: 52
        radius: 12
        // the web's .person: hover → pad.person.bg.hover, pressed → pad.person.bg.pressed
        color: !pad.live ? "transparent" : (prMa.pressed ? Th.padPersonBgPressed : (prMa.containsMouse ? Th.padPersonBgHover : "transparent"))
        Behavior on color { ColorAnimation { duration: Motion.dur } }
        MouseArea {
            id: prMa
            anchors.fill: parent
            hoverEnabled: true
            enabled: pad.live
            cursorShape: Qt.PointingHandCursor
            onClicked: {
                volMenu.mxid = pr.who.u
                volMenu.name = pr.name
                volMenu.person = pr.who
                volMenu.popup(pr, Math.max(0, pr.width - 380), pr.height)
            }
        }
        Accessible.role: Accessible.Button
        Accessible.name: pr.name + " — volumes"
        RowLayout {
            anchors.fill: parent
            anchors.leftMargin: 6; anchors.rightMargin: 10
            spacing: 10
            Item {
                Layout.preferredWidth: 42; Layout.preferredHeight: 42
                Rectangle {   // the talking ring (≈ 0.5 s behind the receiver's speakers)
                    anchors.centerIn: parent
                    width: 42; height: 42; radius: 21
                    color: "transparent"
                    border.width: 3
                    border.color: pr.who.talk ? Th.padPersonRingSpeaking : "transparent"
                    Behavior on border.color { ColorAnimation { duration: Motion.dur } }
                }
                Avatar {
                    anchors.centerIn: parent
                    app: pad.app
                    size: 36
                    seed: pr.who.u
                    name: pr.name
                    imageSource: pr.info && pr.info.avatar ? String(pr.info.avatar) : ""
                }
            }
            ColumnLayout {
                Layout.fillWidth: true
                spacing: 0
                Controls.Label {
                    textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                    Layout.fillWidth: true
                    text: pr.name
                    color: Th.callVolumeLabelFg
                    font.weight: Font.DemiBold
                    elide: Text.ElideRight
                }
                Controls.Label {
                    textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                    Layout.fillWidth: true
                    visible: pr.volLine.length > 0
                    text: pr.volLine
                    color: Th.callVolumeValueFg
                    elide: Text.ElideRight
                    font.pointSize: Kirigami.Theme.smallFont.pointSize
                }
            }
            Controls.Label { visible: pr.who.away === true; text: "💤"; font.pixelSize: 16 }
            ThIcon {
                visible: pr.who.micOff === true
                Layout.preferredWidth: 18; Layout.preferredHeight: 18
                app: pad.app
                name: "microphone-sensitivity-muted-symbolic"
                color: Th.callVolumeLabelFgMuted
                isMask: true
            }
            Canvas {   // the web's chevron (8 × 14)
                id: chev
                Layout.preferredWidth: 8; Layout.preferredHeight: 14
                Connections { target: Th; function onRevisionChanged() { chev.requestPaint() } }
                onPaint: {
                    var c = getContext("2d"); c.clearRect(0, 0, width, height)
                    c.strokeStyle = Th.callVolumeValueFg; c.lineWidth = 1.8; c.lineCap = "round"; c.lineJoin = "round"
                    c.beginPath(); c.moveTo(1.5, 1.5); c.lineTo(6.5, 7); c.lineTo(1.5, 12.5); c.stroke()
                }
            }
        }
    }
    PadVolumeMenu {
        id: volMenu
        app: pad.app
        deviceName: pad.deviceName
        // follow the receiver's newest values for the person the menu is open for
        readonly property var liveEntry: {
            var l = pad.view ? pad.view.people : []
            for (var i = 0; i < l.length; i++) if (l[i].u === volMenu.mxid) return l[i]
            return null
        }
        onLiveEntryChanged: if (volMenu.opened && volMenu.liveEntry) volMenu.person = volMenu.liveEntry
        onCommand: (cmd) => {
            var key = cmd.op === "vol.reset" ? "" : ("vol:" + cmd.user + ":" + cmd.cat)
            if (cmd.live === true) pad.app.padCtl.send(JSON.stringify(cmd))   // live pushes are never answered
            else pad.send(key, true, cmd)
        }
    }
    ConfirmDialog {
        id: leaveConfirm
        app: pad.app
        parent: Controls.Overlay.overlay
        dialogTitle: "Leave the call on " + pad.deviceName + "?"
        message: pad.deviceName + " hangs up. You can join again from there, or with Join here."
        confirmText: "Leave"
        cancelText: "Cancel"
        destructive: true
        onConfirmed: pad.confirmLeave()
    }

    // --tinhat-paddrive (Main.qml; lab gates only): one step = what one control's click runs — the same functions as
    // the handlers above, refused the same way when the control is inert. Returns the command's seq (0 = not sent:
    // paced / no session), "inert", or a description.
    function drive(s) {
        var d = String((s && s.do) || "")
        if (d === "mic") return pad.micUsable ? pad.pressMic() : "inert"
        if (d === "deafen") return pad.deafUsable ? pad.pressDeafen() : "inert"
        if (d === "brb") return pad.brbUsable ? pad.pressBrb() : "inert"
        if (d === "stop") return pad.stopUsable ? pad.pressStop() : "inert"
        if (d === "leave") { if (!pad.leaveUsable) return "inert"; leaveConfirm.open(); return "confirm open" }
        if (d === "leave-confirm") { if (!leaveConfirm.opened) return "no confirm open"; leaveConfirm.close(); return pad.confirmLeave() }
        if (d === "sound") return pad.soundsUsable ? pad.tapSound(String(s.url || "")) : "inert"
        if (d === "person" || d === "vol" || d === "mute" || d === "reset") {
            var u = String(s.user || ""), who = null, l = pad.view ? pad.view.people : []
            for (var i = 0; i < l.length; i++) if (l[i].u === u) who = l[i]
            if (!pad.live || !who) return "inert"
            if (!volMenu.opened || volMenu.mxid !== u) {   // a tap on their row (PersonRow's onClicked)
                volMenu.mxid = u; volMenu.name = pad.personName(u); volMenu.person = who
                volMenu.popup(pad.contentItem, Math.max(0, pad.width - 420), Math.max(0, pad.height - 250))
            }
            if (d === "person") return "menu open"
            // the menu's own commands (PadVolumeMenu: a slider's release, a row's mute, Reset all)
            var cmd = d === "reset" ? { op: "vol.reset", user: u }
                    : d === "mute" ? { op: "vol", user: u, cat: String(s.cat), muted: s.muted === true }
                    : { op: "vol", user: u, cat: String(s.cat), level: Number(s.level) }
            volMenu.command(cmd)
            var o = pad.optimistic[d === "reset" ? "" : ("vol:" + u + ":" + cmd.cat)]
            return o ? o.seq : "sent"
        }
        if (d === "menu-close") { volMenu.close(); return "closed" }
        if (d === "chooser") { if (!chooser.several) return "inert"; deviceMenu.popup(chooser, 0, chooser.height + 4); return "menu open" }
        if (d === "pick") { deviceMenu.close(); pad.openFor(pad.roomId, String(s.device || "")); return "opening " + String(s.device || "") }
        if (d === "close") { pad.close(); return "closed" }
        if (d === "view") return {
            opened: pad.opened, link: pad.link, device: pad.deviceName, room: pad.roomId, targets: pad.targets.length,
            mic: pad.mic, micCaption: pad.micCaption, micUsable: pad.micUsable, pttLine: pad.mic === "ptt" ? pad.why("ptt", "mic") : "",
            deaf: pad.deaf, brb: pad.brb, playing: pad.playing, gateOk: pad.gateOk,
            roleLine: !!pad.me && !pad.gateOk ? (pad.me.gate && String(pad.me.gate.hint || "").length > 0 ? String(pad.me.gate.hint) : pad.why("off-role", "")) : "",
            notice: pad.noticeText, people: pad.view ? pad.view.people : [], pending: Object.keys(pad.optimistic),
            favourites: pad.favourites.length, roomSounds: pad.roomSounds.length, recent: pad.recent.length }
        return "unknown step"
    }

    // --tinhat-paddemo (Main.qml): show sample data already in the mirrors; nothing is opened or sent
    property bool isDemo: false
    function demo(room, device, optimistic) {
        pad.isDemo = true
        pad.roomId = room
        pad.deviceId = device
        pad.optimistic = optimistic || ({})
        pad.notice = ({ code: "", op: "" })
        if (!pad.opened) pad.open()
    }
    function demoNotice(code, op) { pad.say(code, op) }
    function demoVolume(i) {
        var l = pad.view ? pad.view.people : []
        if (i >= l.length) return
        volMenu.mxid = l[i].u
        volMenu.name = pad.personName(l[i].u)
        volMenu.person = l[i]
        demoVolTimer.start()
    }
    Timer { id: demoVolTimer; interval: 900; onTriggered: volMenu.popup(pad.contentItem, Math.max(0, pad.width - 420), Math.max(0, pad.height - 250)) }
}
