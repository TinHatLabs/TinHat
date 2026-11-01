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
// QOL (2026-10-31; the web's PadExtras.tsx, same order and words): my row headed "On {device}" · Watch ("{name} is
// sharing · Watch" → this computer joins to watch: no mic, sound off; "This computer": Hear here · Talk here · Stop
// watching) · the soundboard gains the web search under Favourites (a result plays on {device}: sound.play.web; ☆ adds
// it to my board) · Media Party (the call room's on {device}: ▶/⏸ −10 s +10 s ⏭, Up next, Add a link; party.*)
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

    // ── Pad QOL (2026-10-31) ──
    // search: this device's own soundboard search (its server, its bearer); only myinstants sound files are offered
    // (padrules web_sound_ok: the same rule the receiver applies)
    function webSoundOk(u) {
        var m = /^https:\/\/(?:www\.)?myinstants\.com\/media\/sounds\/([A-Za-z0-9_.-]{1,200})$/.exec(String(u))
        return !!m && m[1].indexOf("..") === -1 && /\.(?:mp3|ogg|wav|m4a|opus)$/.test(m[1])
    }
    property string webQuery: ""
    property var starred: ({})
    readonly property var webResults: {
        if (pad.webQuery.trim().length < 2 || !pad.app.soundboardCtl) return []
        var r = []
        try { r = JSON.parse(String(pad.app.soundboardCtl.searchResultsJson || "[]")) } catch (e) { r = [] }
        var out = []
        for (var i = 0; i < r.length && out.length < 24; i++) {
            var u = String((r[i] && (r[i].url || r[i].sourceUrl)) || "")
            if (pad.webSoundOk(u)) out.push({ name: String(r[i].name || "Sound"), url: u, emoji: "" })
        }
        return out
    }
    readonly property var favUrls: {
        var m = {}, l = pad.app.mPersonalSounds || []
        for (var i = 0; i < l.length; i++) if (l[i] && l[i].sourceUrl) m[String(l[i].sourceUrl)] = true
        return m
    }
    readonly property bool webUsable: pad.live && pad.gateOk && pad.has("sound.play.web")
    function tapWeb(url, name) {
        if (!pad.live) return 0
        if (url === pad.playing) return pad.send("sound", "", { op: "sound.stop" })
        // cut by code points like the web (UTF-16 substring could split an emoji into a lone surrogate — review #12)
        return pad.send("sound", url, { op: "sound.play.web", url: url, name: Array.from(String(name || "Sound")).slice(0, 64).join("") })
    }
    function starWeb(name, url) {
        var o = {}; for (var k in pad.starred) o[k] = true; o[url] = true; pad.starred = o
        pad.app.soundboardCtl.addPersonalSound(String(name || "Sound"), String(url), "", 0)
    }
    // watch: this computer joins the call to WATCH (a normal, visible join: no microphone captured, sound off)
    readonly property bool watchingHere: !!pad.app.callctl && pad.app.callctl.watching === true && String(pad.app.callctl.roomId) === pad.roomId
                                         && (pad.app.callctl.inCall === true || pad.app.callctl.connecting === true)
    readonly property bool inOtherCall: !!pad.app.callctl && (pad.app.callctl.inCall === true || pad.app.callctl.connecting === true) && !pad.watchingHere
    readonly property var sharers: {
        var l = pad.view ? pad.view.people : [], out = []
        for (var i = 0; i < l.length; i++) if (l[i] && l[i].share === true) out.push(l[i].u)
        return out
    }
    // Media Party: the receiver's call room's; the position carried forward from the state (the web's partyView)
    readonly property var party: pad.view && pad.view.party ? pad.view.party : null
    property double partySeenAt: 0
    property double partyNow: Date.now()
    onPartyChanged: pad.partySeenAt = Date.now()
    function partyPos() {
        var p = pad.party
        if (!p) return 0
        var playing = pad.opt("party:play", p.playing === true)
        var rt = pad.st ? Number(pad.st.rt || 0) : 0, at = Number(p.at || 0)
        var ahead = p.playing === true && at > 0 && rt > at ? Math.min(86400, (rt - at) / 1000) : 0
        var x = Number(p.pos || 0) + ahead + (playing && pad.partyNow > pad.partySeenAt ? (pad.partyNow - pad.partySeenAt) / 1000 : 0)
        var seekTo = pad.opt("party:pos", -1)
        if (seekTo >= 0) x = seekTo
        return p.dur > 0 ? Math.min(p.dur, x) : x
    }
    function fmt(secs) {
        var t = Math.max(0, Math.floor(secs)), h = Math.floor(t / 3600), m = Math.floor((t % 3600) / 60), s = t % 60
        return (h > 0 ? h + ":" + (m < 10 ? "0" : "") : "") + m + ":" + (s < 10 ? "0" : "") + s
    }
    function partyLinkOk(u) {
        u = String(u)
        if (u.length === 0 || u.length > 2048 || !/^[\x21-\x7E]+$/.test(u)) return false
        var m = /^https:\/\/([^\/?#]+)(?:[\/?].*)?$/i.exec(u)
        return !!m && ["youtube.com", "www.youtube.com", "m.youtube.com", "music.youtube.com", "youtu.be", "open.spotify.com", "music.apple.com"].indexOf(m[1].toLowerCase()) !== -1
    }
    readonly property bool partyDrive: pad.live && !!pad.party && pad.party.host === true && pad.has("party.play")
    function partyPlay(on) { return pad.send("party:play", on, { op: "party.play", url: String(pad.party.url), playing: on }) }
    function partySeek(x) { var v = Math.max(0, Math.min(86400, Math.floor(x))); pad.partySeenAt = Date.now(); return pad.send("party:pos", v, { op: "party.seek", url: String(pad.party.url), pos: v }) }
    function partyNext() { return pad.send("", true, { op: "party.next", url: String(pad.party.url) }) }
    // this computer's own Media Party window on the call room's party (the call bar's Join + open)
    function watchPartyHere() { pad.app.openWatchPartyFor(pad.roomId) }
    function partyAdd(u, now) { return pad.send("", true, { op: "party.add", url: String(u), now: now }) }
    Timer { interval: 1000; repeat: true; running: pad.opened && !!pad.party && pad.party.playing === true; onTriggered: pad.partyNow = Date.now() }
    // a link this session asked {device}'s party to add couldn't be added: its answer was the acceptance, call.party.add
    // the outcome (the web's controller does the same; said once per seq)
    // (deduplicated by "sid:seq" — an outcome that arrives out of order is still said once; review pass 2)
    property var addSeen: ({})
    readonly property var partyAddOutcome: pad.party && pad.party.add ? pad.party.add : null
    onPartyAddOutcomeChanged: {
        var a = pad.partyAddOutcome
        if (!a || !pad.mine || String(a.sid) !== String(pad.remote.sid || "")) return
        var k = String(a.sid) + ":" + Number(a.re)
        if (pad.addSeen[k]) return
        var o = {}, keys = Object.keys(pad.addSeen).slice(-63)
        for (var i = 0; i < keys.length; i++) o[keys[i]] = true
        o[k] = true
        pad.addSeen = o
        if (a.ok !== true) pad.say("bad-link", "party.add")
    }

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
                Controls.Label {
                    textFormat: Text.PlainText
                    Layout.leftMargin: 18; Layout.topMargin: 4
                    text: "On " + pad.deviceName
                    color: Th.soundboardSectionHintFg
                    font.bold: true
                    font.pointSize: Kirigami.Theme.smallFont.pointSize
                }
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

                // ---- Watch (only while someone shares, or while this computer watches) ----
                Rectangle {
                    // 12.1.6: Watch is off (call_ctl.rs WATCH_AVAILABLE; back in 12.1.7) — the strip stays hidden
                    readonly property bool watchAvailable: false
                    visible: watchAvailable && !pad.watchingHere && pad.sharers.length > 0
                    Layout.fillWidth: true
                    Layout.leftMargin: 14; Layout.rightMargin: 14; Layout.topMargin: 6
                    implicitHeight: watchCol.implicitHeight + 20
                    radius: 12
                    color: Th.padListBarBg
                    border.width: 1
                    border.color: Th.padListBarBorder
                    ColumnLayout {
                        id: watchCol
                        anchors.fill: parent; anchors.margins: 10
                        spacing: 4
                        RowLayout {
                            spacing: 10
                            ThIcon { app: pad.app; name: "video-display-symbolic"; color: Th.padListBarFg; isMask: true; Layout.preferredWidth: 20; Layout.preferredHeight: 20 }
                            Controls.Label {
                                textFormat: Text.PlainText
                                Layout.fillWidth: true
                                elide: Text.ElideRight
                                font.weight: Font.DemiBold
                                color: Th.padListBarFg
                                text: pad.sharers.length > 1 ? pad.personName(pad.sharers[0]) + " and " + (pad.sharers.length - 1) + " more are sharing"
                                                             : (pad.sharers.length ? pad.personName(pad.sharers[0]) + " is sharing" : "")
                            }
                            ThButton { app: pad.app; text: "Watch"; enabled: !pad.inOtherCall; onClicked: pad.app.callctl.watchCall(pad.roomId) }
                        }
                        Controls.Label {
                            textFormat: Text.PlainText
                            Layout.fillWidth: true
                            wrapMode: Text.WordWrap
                            color: Th.soundboardSectionHintFg
                            font.pointSize: Kirigami.Theme.smallFont.pointSize
                            text: pad.inOtherCall ? "Leave your call on this computer first." : "Joins on this computer to watch — muted, sound off. Everyone sees you joined."
                        }
                    }
                }
                SectionHead {
                    visible: pad.watchingHere
                    Layout.topMargin: 8
                    title: pad.app.callctl && pad.app.callctl.inCall !== true ? "Joining to watch…" : "Watching"
                    hint: pad.sharers.length === 0 ? "nobody is sharing right now" : "the share is on the call screen"
                }
                Rectangle {
                    visible: pad.watchingHere
                    Layout.fillWidth: true
                    Layout.leftMargin: 14; Layout.rightMargin: 14
                    implicitHeight: phoneRow.implicitHeight + 20
                    radius: 16
                    color: Th.padMyRowBg
                    border.width: 1
                    border.color: Th.padMyRowBorder
                    Flow {
                        id: phoneRow
                        anchors.fill: parent; anchors.margins: 10
                        spacing: 14
                        Controls.Label { textFormat: Text.PlainText; width: phoneRow.width; text: "This computer"; color: Th.soundboardSectionHintFg; font.bold: true; font.pointSize: Kirigami.Theme.smallFont.pointSize }
                        ThSwitch {
                            text: "Hear here"
                            checked: !!pad.app.callctl && pad.app.callctl.deafened !== true
                            onToggled: pad.app.callctl.watchHear(checked)
                        }
                        ThSwitch {
                            readonly property bool talking: !!pad.app.callctl && pad.app.callctl.noCapture !== true && pad.app.callctl.micMuted !== true
                            text: talking ? "Talk here · live" : "Talk here"
                            checked: talking
                            onToggled: pad.app.callctl.watchTalk(checked)
                        }
                        ThButton { app: pad.app; text: "Stop watching"; onClicked: pad.app.callctl.leaveCall() }
                    }
                }
                Controls.Label {
                    visible: pad.watchingHere
                    textFormat: Text.PlainText
                    Layout.leftMargin: 18
                    text: "Mute, deafen and Leave above are " + pad.deviceName + "'s."
                    color: Th.soundboardSectionHintFg
                    font.pointSize: Kirigami.Theme.smallFont.pointSize
                }

                // ---- the soundboard ----
                SectionHead { title: "Soundboard"; hint: "plays on " + pad.deviceName + ", into the call"; Layout.topMargin: 8 }
                SubHead { title: "Favourites" }
                SoundGrid { sounds: pad.favourites; visible: pad.favourites.length > 0 }
                Controls.Label {
                    visible: pad.favourites.length === 0
                    Layout.fillWidth: true; Layout.leftMargin: 16; Layout.rightMargin: 16
                    textFormat: Text.StyledText
                    text: "Star sounds in the soundboard to pin them here, or <a href=\"search\">search sounds</a>."
                    linkColor: Th.soundboardSectionFg
                    onLinkActivated: webField.forceActiveFocus()
                    color: Th.soundboardEmptyFg
                    font.pointSize: Kirigami.Theme.smallFont.pointSize
                }
                // the web search (this computer's own search; a result plays on {device}, ☆ adds it to my board)
                Kirigami.SearchField {
                    id: webField
                    Layout.fillWidth: true
                    Layout.leftMargin: 14; Layout.rightMargin: 14; Layout.topMargin: 2
                    placeholderText: "Search sounds on the web…"
                    onTextChanged: { pad.webQuery = text; if (pad.app.soundboardCtl) pad.app.soundboardCtl.search(text.trim().length >= 2 ? text.trim() : "") }
                }
                SectionHead {
                    visible: pad.webQuery.trim().length >= 2
                    title: "From the web" + (pad.webResults.length ? " · " + pad.webResults.length : "")
                    hint: pad.app.soundboardCtl && pad.app.soundboardCtl.searching ? "searching…"
                        : (pad.has("sound.play.web") ? "tap plays on " + pad.deviceName + " · ☆ adds to your board" : "☆ adds it to your board")
                }
                Controls.Label {
                    textFormat: Text.PlainText
                    visible: pad.webQuery.trim().length >= 2 && pad.live && !pad.has("sound.play.web")
                    Layout.fillWidth: true; Layout.leftMargin: 16; Layout.rightMargin: 16
                    wrapMode: Text.WordWrap
                    text: pad.why("old-op", "") + " To play a result there now, ☆ it first."
                    color: Th.soundboardEmptyFg
                    font.pointSize: Kirigami.Theme.smallFont.pointSize
                }
                Controls.Label {
                    textFormat: Text.PlainText
                    visible: pad.webQuery.trim().length >= 2 && !!pad.app.soundboardCtl && !pad.app.soundboardCtl.searching && pad.webResults.length === 0
                    Layout.leftMargin: 16
                    text: "No sounds found for “" + pad.webQuery.trim() + "”."
                    color: Th.soundboardEmptyFg
                    font.pointSize: Kirigami.Theme.smallFont.pointSize
                }
                GridLayout {
                    id: webGrid
                    visible: pad.webResults.length > 0
                    Layout.fillWidth: true
                    Layout.leftMargin: 14; Layout.rightMargin: 14
                    columns: 4
                    columnSpacing: 8
                    rowSpacing: 8
                    Repeater {
                        model: pad.webResults
                        delegate: Item {
                            id: wc
                            required property var modelData
                            readonly property bool fav: !!pad.favUrls[modelData.url] || !!pad.starred[modelData.url]
                            Layout.fillWidth: true
                            Layout.preferredWidth: 120
                            Layout.maximumWidth: Math.max(60, (pad.width - 28 - webGrid.columnSpacing * (webGrid.columns - 1)) / webGrid.columns)
                            implicitHeight: wb.implicitHeight
                            PadSoundButton {
                                id: wb
                                anchors.left: parent.left; anchors.right: parent.right
                                name: String(wc.modelData.name || "Sound")
                                emoji: ""
                                playing: wc.modelData.url === pad.playing
                                pending: playing && pad.pendingKey("sound")
                                enabled: pad.webUsable
                                onClicked: pad.tapWeb(wc.modelData.url, wc.modelData.name)
                            }
                            ThToolButton {
                                anchors.top: parent.top; anchors.right: parent.right; anchors.margins: 2
                                app: pad.app
                                iconName: wc.fav ? "starred-symbolic" : "non-starred-symbolic"
                                iconColor: wc.fav ? Th.soundboardSectionFg : Th.soundboardSectionHintFg
                                iconSize: 16
                                enabled: !wc.fav
                                tip: wc.fav ? "On your board" : "Add to your board"
                                onClicked: pad.starWeb(wc.modelData.name, wc.modelData.url)
                            }
                        }
                    }
                }
                Controls.Label {
                    visible: pad.webQuery.trim().length >= 2
                    Layout.leftMargin: 16
                    textFormat: Text.PlainText
                    text: "web results by myinstants"
                    color: Th.soundboardFooterFg
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

                // ---- Media Party (the call room's, on {device}) ----
                SectionHead { visible: !!pad.party; Layout.topMargin: 8; title: "Media Party"; hint: pad.party && pad.party.in !== true ? "on in this room" : "on " + pad.deviceName }
                RowLayout {
                    visible: !!pad.party && pad.party.in !== true
                    Layout.leftMargin: 16
                    spacing: 8
                    ThButton { app: pad.app; text: "Join on " + pad.deviceName; enabled: pad.live && pad.has("party.join"); onClicked: pad.send("party:in", true, { op: "party.join" }) }
                    ThButton { app: pad.app; text: "Watch on this computer"; onClicked: pad.watchPartyHere() }
                }
                ThButton {
                    visible: !!pad.party && pad.party.in === true
                    Layout.leftMargin: 16
                    app: pad.app
                    text: "Watch on this computer"
                    onClicked: pad.watchPartyHere()
                }
                ColumnLayout {
                    visible: !!pad.party && pad.party.in === true
                    Layout.fillWidth: true
                    Layout.leftMargin: 16; Layout.rightMargin: 16
                    spacing: 6
                    Controls.Label {
                        textFormat: Text.PlainText
                        Layout.fillWidth: true
                        elide: Text.ElideRight
                        font.weight: Font.DemiBold
                        color: Th.soundboardSectionFg
                        text: pad.party ? (String(pad.party.url).length ? (String(pad.party.title || "").length ? String(pad.party.title) : String(pad.party.url)) : "Nothing playing") : ""
                    }
                    RowLayout {
                        visible: !!pad.party && String(pad.party.url).length > 0
                        Layout.fillWidth: true
                        spacing: 8
                        Controls.Label { textFormat: Text.PlainText; text: pad.fmt(pad.partyPos()); color: Th.soundboardSectionHintFg; font.pointSize: Kirigami.Theme.smallFont.pointSize }
                        Rectangle {
                            Layout.fillWidth: true; implicitHeight: 4; radius: 2
                            color: Th.padMyRowBorder
                            Rectangle {
                                height: parent.height; radius: 2
                                width: pad.party && pad.party.dur > 0 ? parent.width * Math.min(1, pad.partyPos() / pad.party.dur) : 0
                                color: Th.padPersonRingSpeaking
                            }
                        }
                        Controls.Label { textFormat: Text.PlainText; text: pad.party && pad.party.dur > 0 ? pad.fmt(pad.party.dur) : "--:--"; color: Th.soundboardSectionHintFg; font.pointSize: Kirigami.Theme.smallFont.pointSize }
                    }
                    RowLayout {
                        Layout.alignment: Qt.AlignHCenter
                        spacing: 12
                        readonly property bool hasUrl: !!pad.party && String(pad.party.url).length > 0
                        ThToolButton { app: pad.app; label: "−10 s"; tip: "Back 10 s"; enabled: pad.partyDrive && parent.hasUrl; onClicked: pad.partySeek(pad.partyPos() - 10) }
                        ThToolButton {
                            app: pad.app
                            readonly property bool on: pad.opt("party:play", !!pad.party && pad.party.playing === true)
                            iconName: on ? "media-playback-pause" : "media-playback-start"
                            tip: on ? "Pause" : "Play"
                            enabled: pad.partyDrive && parent.hasUrl
                            onClicked: pad.partyPlay(!on)
                        }
                        ThToolButton { app: pad.app; label: "+10 s"; tip: "Forward 10 s"; enabled: pad.partyDrive && parent.hasUrl; onClicked: pad.partySeek(pad.party.dur > 0 ? Math.min(pad.party.dur, pad.partyPos() + 10) : pad.partyPos() + 10) }
                        ThToolButton { app: pad.app; iconName: "media-skip-forward"; tip: "Play next in queue"; enabled: pad.partyDrive && !!pad.party && pad.party.next.length > 0; onClicked: pad.partyNext() }
                    }
                    Controls.Label {
                        textFormat: Text.PlainText
                        visible: !!pad.party && pad.party.host !== true
                        Layout.fillWidth: true
                        wrapMode: Text.WordWrap
                        text: pad.why("not-host", "") + " You can still add links."
                        color: Th.soundboardEmptyFg
                        font.pointSize: Kirigami.Theme.smallFont.pointSize
                    }
                    SubHead { visible: !!pad.party && pad.party.next.length > 0; title: "Up next"; Layout.leftMargin: 0 }
                    Repeater {
                        model: pad.party ? pad.party.next : []
                        delegate: Controls.Label {
                            required property var modelData
                            required property int index
                            textFormat: Text.PlainText
                            Layout.fillWidth: true
                            elide: Text.ElideRight
                            text: (index + 1) + ". " + String(modelData.title || "")
                            color: Th.soundboardSectionFg
                        }
                    }
                    RowLayout {
                        visible: pad.live && pad.has("party.add")
                        Layout.fillWidth: true
                        spacing: 8
                        ThTextField {
                            id: linkField
                            Layout.fillWidth: true
                            placeholderText: "Add a link (YouTube, Spotify…)"
                            onAccepted: addBtn.clicked()
                        }
                        ThButton {
                            id: addBtn
                            app: pad.app
                            text: "Add"
                            enabled: linkField.text.trim().length > 0
                            onClicked: { if (!pad.partyLinkOk(linkField.text.trim())) { linkErr.visible = true; return } linkErr.visible = false; if (pad.partyAdd(linkField.text.trim(), false) > 0) linkField.text = "" }
                        }
                        ThButton {
                            app: pad.app
                            visible: !!pad.party && pad.party.host === true
                            text: "Play now"
                            enabled: linkField.text.trim().length > 0
                            onClicked: { if (!pad.partyLinkOk(linkField.text.trim())) { linkErr.visible = true; return } linkErr.visible = false; if (pad.partyAdd(linkField.text.trim(), true) > 0) linkField.text = "" }
                        }
                    }
                    Controls.Label {
                        id: linkErr
                        visible: false
                        textFormat: Text.PlainText
                        text: "Paste a YouTube, YouTube Music, Spotify or Apple Music link."
                        color: Th.soundboardStatusFgError
                        font.pointSize: Kirigami.Theme.smallFont.pointSize
                    }
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
        // Pad QOL: the web search, Watch and the Media Party card (the same functions as their controls)
        if (d === "web-search") { webField.text = String(s.q || ""); return "searching " + webField.text }
        if (d === "web-results") return { n: pad.webResults.length, urls: pad.webResults.map(function (x) { return x.url }), searching: !!pad.app.soundboardCtl && pad.app.soundboardCtl.searching === true }
        if (d === "web-tap") { var w = pad.webResults[Number(s.i || 0)]; return w && pad.webUsable ? pad.tapWeb(w.url, w.name) : "inert" }
        if (d === "web-star") { var w2 = pad.webResults[Number(s.i || 0)]; if (!w2) return "inert"; pad.starWeb(w2.name, w2.url); return "starred " + w2.url }
        if (d === "pad-watch") { if (pad.sharers.length === 0 || pad.inOtherCall) return "inert"; pad.app.callctl.watchCall(pad.roomId); return "watching " + pad.roomId }
        if (d === "party-play") return pad.partyDrive ? pad.partyPlay(!pad.opt("party:play", pad.party.playing === true)) : "inert"
        if (d === "party-next") return pad.partyDrive && pad.party.next.length > 0 ? pad.partyNext() : "inert"
        if (d === "view") return {
            opened: pad.opened, link: pad.link, device: pad.deviceName, room: pad.roomId, targets: pad.targets.length,
            mic: pad.mic, micCaption: pad.micCaption, micUsable: pad.micUsable, pttLine: pad.mic === "ptt" ? pad.why("ptt", "mic") : "",
            deaf: pad.deaf, brb: pad.brb, playing: pad.playing, gateOk: pad.gateOk,
            roleLine: !!pad.me && !pad.gateOk ? (pad.me.gate && String(pad.me.gate.hint || "").length > 0 ? String(pad.me.gate.hint) : pad.why("off-role", "")) : "",
            notice: pad.noticeText, people: pad.view ? pad.view.people : [], pending: Object.keys(pad.optimistic),
            favourites: pad.favourites.length, roomSounds: pad.roomSounds.length, recent: pad.recent.length,
            sharers: pad.sharers, watchingHere: pad.watchingHere, party: pad.party, ops: pad.ops }
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
