// RigTable.qml — mini games v1 (GAMES-V1-SPEC.md §5; the frozen view contract `vectors/play/views/rig/*`): The Rig's
// table, drawn from the server's view and nothing else. The view is the truth: QML never computes a game rule; a button
// is offered when the view says the move is possible now (phase, counts left, flags) and the server refuses the rest.
//
// The look and layout are the web's (tinhat-web src/features/play/RigTable.tsx + Rig.module.css; desktop and web are
// one product): the panel header above shows the game's name; here
//
//   top line   "Round n · Lite/Full · <phase> · <secs>s" · my role pill · Join / Leave table / Close table (host)
//   LOBBY      the pitch, the seated players (avatars, open seats; the host's kick), the host's Start box (Machine
//              Lite/Full, Bankroll 3–10, Patch for Full) or the wait line
//   ROUND      two columns. Left: the MACHINE (who spins, "Spin n of N", the bankroll coin + its delta, three reel
//              windows with the drawn symbols (RigSymbol.qml; one-shot landing when a new spin lands and the window is
//              focused), the House's Nudge arrows on the reels, the bet dial 1·2·3, the Spinner's Call a card / Audit /
//              Walk / SPIN or the status line, the Call and Audit pickers, the Audit result), then the history (one slot
//              per spin of the round, empty ones too) or the reveal, then the Rule Board (a grid of cards: name, rule,
//              bite dots, Reel/Payout card, Tax, Out / Busted / Called wrong). Right: "At the table" (avatars + role
//              pills, the net after the reveal), the House secret panel (my card, MUTE, Rake, Patch, Market, Whisper,
//              Peek, the Nudge note; collapses to rig.secret_hidden while I share my screen), the Whispers to the
//              Spinner, the paytable. One column when narrow.
//   REVEAL     every slot face up with its owner, Calls / Mutes / Nudges / Peeks / Market / Whispers with authors, the
//              replay (honest → final per spin) and the seed (selectable)
//   END        "Everyone has spun": the leaderboard + Pit Boss; the host's Start box
//
// Moves: rig.play.act(JSON.stringify({do: …})) with the contract's move names and fields (README.views.md "Moves");
// the live dial goes through play.dial(n). Words: words.json (`rig.*`, `games.*`, `table.*`, `card.*`, `wallet.*`);
// strings words.json does not have yet go through rig.nw(key, the web's exact text).
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app

Item {
    id: rig
    property var app: null          // the Main.qml root (ThButton { app: rig.app } …)
    property var play: null         // PlayController: act(json) → op id, dial(n), liveDial
    property var view: null         // the parsed view (null while loading)
    property var words: ({})        // parsed words.json
    property string myId: ""        // my Matrix id
    property bool hideSecret: false // I share my screen: the House secret panel collapses
    property real clockOffset: 0    // server now − local now (ms)
    property bool focusedWindow: true  // false: no motion (the reels show the result at once)
    readonly property var art: rig.app && rig.app.playArt ? rig.app.playArt : ({})   // ui.json art (both apps)

    // ── names ─────────────────────────────────────────────────────────────────────────────────────────────────────
    function isSomeone(u) { return /^~[1-9][0-9]?$/.test(String(u || "")) }
    function nameOf(mxid) {
        if (!mxid) return ""
        if (rig.isSomeone(mxid)) return rig.nw("someone", "someone")
        if (rig.app && rig.app.displayNameFor) {
            var n = rig.app.displayNameFor(mxid)
            if (n) return String(n)
        }
        var s = String(mxid)
        var colon = s.indexOf(":")
        return s.substring(s.charAt(0) === "@" ? 1 : 0, colon > 0 ? colon : s.length)
    }

    // ── words ─────────────────────────────────────────────────────────────────────────────────────────────────────
    function wd(group, key, fb) {
        var g = rig.words ? rig.words[group] : null
        var v = g ? g[key] : undefined
        return (v === undefined || v === null) ? (fb === undefined ? "" : fb) : v
    }
    function rw(key) { var v = rig.wd("rig", key, key); return typeof v === "string" ? v : key }
    // a string words.json does not have yet: rig.<key> once it is added, the web's exact text until then
    function nw(key, fb) { var v = rig.wd("rig", key, fb); return typeof v === "string" ? v : fb }
    function fill(s, vals) {
        var out = String(s === undefined || s === null ? "" : s)
        for (var k in vals) out = out.split("{" + k + "}").join(String(vals[k]))
        return out
    }
    function sub(key, sk) {
        var g = rig.words && rig.words.rig ? rig.words.rig[key] : null
        var v = g ? g[sk] : undefined
        return (typeof v === "string") ? v : String(sk)
    }
    function cardInfo(id) {
        var c = rig.words && rig.words.rig && rig.words.rig.cards ? rig.words.rig.cards[id] : null
        return c ? c : { name: String(id || ""), rule: "", bite: 0, tax: false, kind: "" }
    }
    function cardName(id) { return rig.cardInfo(id).name }
    readonly property var templateKeys: ["bet-size", "not-moon", "last-spin", "reel-2", "bluffing", "trust-nobody"]
    function templateRaw(key) {
        var list = rig.words && rig.words.rig && rig.words.rig.whisper_templates ? rig.words.rig.whisper_templates : []
        var i = rig.templateKeys.indexOf(key)
        return (i >= 0 && i < list.length) ? String(list[i]) : String(key || "")
    }
    function templateText(key, person) { return rig.fill(rig.templateRaw(key), { person: person ? rig.nameOf(person) : rig.nw("someone", "someone") }) }
    function whisperText(w) {
        if (!w) return ""
        return w.text ? String(w.text) : rig.templateText(w.template, w.person)
    }
    // a free-text Whisper the server will take: 1–60 characters, no control characters, no links (the web's whisperOk)
    function whisperOk(text) {
        var t = String(text || "").trim()
        return t.length >= 1 && t.length <= 60 && !/[\u0000-\u001f\u007f-\u009f]/.test(t) && !/:\/\/|www\./i.test(t)
    }
    function symWord(s) { return rig.sub("symbols", s) }
    function symNames(reels) { return reels && reels.length ? reels.map(rig.symWord).join(", ") : "" }
    function stampWord(st) {
        var m = rig.words && rig.words.rig ? rig.words.rig.stamps : null
        if (m && typeof m[st] === "string") return m[st]
        return st === "refilled" ? rig.nw("stamp_refilled", "Slot refilled") : String(st)
    }
    function phaseText(p) {
        var m = rig.words && rig.words.rig ? rig.words.rig.phases : null
        if (m && typeof m[p] === "string") return m[p]
        return p === "paused" ? rig.nw("phase_paused", "Paused: the Spinner dropped out") : String(p || "")
    }
    function signed(n) { n = Number(n || 0); return n > 0 ? "+" + n : String(n) }
    function roleWord(r) { return r ? rig.rw(r) : "" }
    function sideWord(side) { return side === "up" ? rig.rw("up") : rig.rw("not_up") }
    function act(obj) { return rig.play ? rig.play.act(JSON.stringify(obj)) : "" }
    function fp(px) { return Math.max(6, Kirigami.Theme.defaultFont.pointSize) * Th.textScale * px / 13 }   // the web's px at 13 px = the default font

    // ── the view, guarded ────────────────────────────────────────────────────────────────────────────────────────
    readonly property bool wide: rig.width >= 640
    readonly property bool motionOk: rig.focusedWindow && Motion.animate
    readonly property color winC: rig.art.win || Th.stateSuccess
    readonly property color lossC: rig.art.loss || Th.statusErrorFg
    readonly property color trimC: rig.art.machineTrim || Th.accentBright
    readonly property var rd: rig.view && rig.view.round ? rig.view.round : null
    readonly property var you: rig.view && rig.view.you ? rig.view.you : null
    readonly property var me: rig.you ? rig.you : ({})
    readonly property string tableStatus: rig.view && rig.view.status ? String(rig.view.status) : ""
    readonly property string myRole: rig.you && rig.you.role ? String(rig.you.role) : ""
    readonly property string phase: rig.rd && rig.rd.phase ? String(rig.rd.phase) : ""
    readonly property bool inRound: rig.tableStatus === "round" && !!rig.rd
    readonly property bool isHost: !!rig.view && rig.myId !== "" && rig.view.host === rig.myId
    readonly property var players: rig.view && rig.view.players ? rig.view.players : []
    readonly property var activePlayers: rig.players.filter(function (p) { return p && !p.left })
    readonly property bool joined: rig.activePlayers.some(function (p) { return p.u === rig.myId })
    readonly property bool isSpinner: rig.inRound && rig.myRole === "spinner" && rig.rd.spinner === rig.myId
    readonly property bool isHouse: rig.inRound && (rig.myRole === "mechanic" || rig.myRole === "sharp" || rig.myRole === "railbird")
    readonly property bool isMech: rig.myRole === "mechanic"
    readonly property var feats: rig.rd && rig.rd.features ? rig.rd.features : ({})
    readonly property var history: rig.rd && rig.rd.history ? rig.rd.history : []
    readonly property var lastSpin: rig.history.length > 0 ? rig.history[rig.history.length - 1] : null
    readonly property var board: rig.rd && rig.rd.board ? rig.rd.board : []
    readonly property var openBoard: rig.board.filter(function (b) { return b && b.state === "open" })
    readonly property var reveal: rig.view && rig.view.reveal ? rig.view.reveal : (rig.rd && rig.rd.reveal ? rig.rd.reveal : null)
    readonly property bool revealShown: rig.inRound && !!rig.reveal
    readonly property var tableEnd: rig.view && rig.view.end ? rig.view.end : null
    readonly property int bankStart: rig.rd && rig.rd.bankroll ? Number(rig.rd.bankroll.start || 0) : 0
    readonly property int bankNow: rig.rd && rig.rd.bankroll ? Number(rig.rd.bankroll.now || 0) : 0
    readonly property int bankNet: rig.bankNow - rig.bankStart
    readonly property int callsLeft: rig.rd ? Number(rig.rd.calls_left || 0) : 0
    readonly property bool canTurn: rig.isSpinner && rig.phase === "turn"
    readonly property bool canCall: rig.canTurn && rig.callsLeft > 0 && !rig.rd.call_used_this_spin && rig.bankNow >= 2
    readonly property bool canAudit: rig.canTurn && !rig.rd.audit_used && rig.history.length > 0
    readonly property bool canWalk: rig.canTurn && rig.rd.spin >= rig.rd.walk_from
    readonly property var myWhispers: rig.you && rig.you.whispers ? rig.you.whispers : (rig.rd && rig.rd.whispers ? rig.rd.whispers : [])
    readonly property string myCardId: rig.you && rig.you.card && rig.you.card.card ? String(rig.you.card.card) : ""
    readonly property var otherPlayers: rig.activePlayers.filter(function (p) { return p.u !== rig.myId })
    readonly property var peekTargets: rig.activePlayers.filter(function (p) { return p.role === "mechanic" && p.u !== rig.myId })
    readonly property var netMap: rig.reveal && rig.reveal.net ? rig.reveal.net : null
    function histAt(n) {
        for (var i = 0; i < rig.history.length; ++i) if (rig.history[i] && rig.history[i].spin === n) return rig.history[i]
        return null
    }

    readonly property string phaseWord: !rig.rd ? "" : (rig.rd.paused ? rig.phaseText("paused") : rig.phaseText(rig.phase))
    readonly property string metaText: {
        if (!rig.view) return ""
        if (rig.inRound) {
            var s = rig.fill(rig.nw("round_n", "Round {n}"), { n: rig.rd.r }) + " · "
                    + (rig.rd.preset === "full" ? rig.rw("full") : rig.rw("lite")) + " · " + rig.phaseWord
            if (rig.secsLeft > 0 && !rig.rd.paused) s += " · " + rig.secsLeft + "s"
            return s
        }
        if (rig.tableStatus === "end") return rig.phaseText("end")
        if (rig.tableStatus === "lobby") return rig.phaseText("lobby")
        return ""
    }

    // ── local UI state (never game state) ──────────────────────────────────────────────────────────────────────────
    property string pickMode: ""    // "" | "call" (a Rule Board card) | "audit" (a past spin) — the Spinner's pickers
    property string peekTarget: ""  // the Mechanic I'm peeking (then a card)
    property bool bluffPick: false  // the "{person} is bluffing" Whisper: pick the person
    property string whisperDraft: ""
    property int myDial: 0
    property string myDialKey: ""
    property string hostPreset: "lite"
    property int hostBankroll: 6
    property bool hostPatch: true
    property string _optsFor: ""
    readonly property bool hostLite: rig.hostPreset === "lite" || rig.activePlayers.length <= 3   // Full needs 4 or more
    readonly property string turnKey: rig.rd ? rig.rd.r + "/" + rig.rd.spin + "/" + rig.phase : ""
    readonly property string dialKey: rig.rd ? rig.rd.r + "/" + rig.rd.spin : ""
    onTurnKeyChanged: { rig.pickMode = ""; rig.peekTarget = ""; rig.bluffPick = false }
    onViewChanged: {
        rig.nowMs = Date.now()
        if (rig.view && rig.view.opts && rig.view.table !== rig._optsFor) {
            rig._optsFor = String(rig.view.table)
            rig.hostPreset = rig.view.opts.preset === "full" ? "full" : "lite"
            rig.hostBankroll = Math.max(3, Math.min(10, Number(rig.view.opts.bankroll || 6)))
            rig.hostPatch = rig.view.opts.patch !== false
        }
    }

    // the dial everyone sees: the locked bet, else my own dial (Spinner, this spin), else the live frame, else the view
    readonly property int dialShown: {
        if (!rig.rd) return 0
        if (rig.rd.bet) return Number(rig.rd.bet)
        if (rig.isSpinner && rig.myDial > 0 && rig.myDialKey === rig.dialKey) return rig.myDial
        if (rig.phase === "turn" && rig.play && rig.play.liveDial >= 0) return rig.play.liveDial
        return Number(rig.rd.dial || 1)
    }
    function setDial(n) {
        if (!rig.canTurn || n > rig.bankNow) return
        if (rig.play) rig.play.dial(n)
        rig.myDial = n
        rig.myDialKey = rig.dialKey
    }
    function doSpin() { if (!rig.canTurn || rig.bankNow < 1) return; rig.pickMode = ""; rig.act({ do: "spin" }) }
    function toggleMute() {
        if (!rig.you) return
        rig.act({ do: "mute", armed: !rig.you.armed })
    }
    function togglePick(mode) { rig.pickMode = rig.pickMode === mode ? "" : mode }
    function callCard(id) {
        if (rig.pickMode === "call" && rig.canCall) rig.act({ do: "call", card: id })
        rig.pickMode = ""
    }
    function auditSpin(n) {
        if (rig.pickMode === "audit" && rig.canAudit) rig.act({ do: "audit", spin: n })
        rig.pickMode = ""
    }
    function peekCard(id) {
        // a Peek is secret: never from a shared screen
        if (rig.peekTarget !== "" && rig.phase === "turn" && !rig.hideSecret) rig.act({ do: "peek", mechanic: rig.peekTarget, card: id })
        rig.peekTarget = ""
    }
    function sendWhisper(obj) {
        obj.do = "whisper"
        rig.act(obj)
        rig.bluffPick = false
    }
    function sendWhisperText() {
        var t = rig.whisperDraft.trim()
        if (!rig.whisperOk(t)) return
        rig.sendWhisper({ text: t })
        rig.whisperDraft = ""
        whisperField.text = ""
    }
    function startTable() {
        var n = rig.activePlayers.length
        if (n < 3 || n > 8) return
        rig.act({ do: "start", preset: rig.hostLite ? "lite" : "full", bankroll: rig.hostBankroll, patch: !rig.hostLite && rig.hostPatch })
    }

    // ── the countdown (server clock) ───────────────────────────────────────────────────────────────────────────────
    property real nowMs: Date.now()
    readonly property real deadlineMs: {
        if (!rig.rd || rig.rd.paused) return 0
        var nd = rig.rd.nudge
        if (rig.phase === "nudge" && nd && nd.open && nd.deadline) return Number(nd.deadline)
        return Number(rig.rd.deadline || 0)
    }
    readonly property int secsLeft: rig.deadlineMs > 0 ? Math.max(0, Math.ceil((rig.deadlineMs - (rig.nowMs + rig.clockOffset)) / 1000)) : -1
    Timer {
        interval: 500
        repeat: true
        running: rig.deadlineMs > 0 && rig.visible
        triggeredOnStart: true
        onTriggered: rig.nowMs = Date.now()
    }

    // ── the reels: what the machine shows + the one-shot landing ───────────────────────────────────────────────────
    readonly property var shownReels: rig.rd && rig.rd.reels ? rig.rd.reels : (rig.lastSpin ? rig.lastSpin.reels : null)
    readonly property string shownKey: !rig.rd ? "" : rig.rd.r + "/" + (rig.rd.reels ? rig.rd.spin : (rig.lastSpin ? rig.lastSpin.spin : 0))
    readonly property string reelsSig: rig.shownKey + "|" + (rig.shownReels ? rig.shownReels.join(",") : "")
    property string _reelKey: ""
    property bool _reelsReady: false
    onReelsSigChanged: Qt.callLater(rig._syncReels)
    Component.onCompleted: Qt.callLater(rig._syncReels)
    function _syncReels() {
        var reels = rig.shownReels
        var newSpin = rig.shownKey !== rig._reelKey
        for (var i = 0; i < 3; ++i) {
            var it = reelRep.itemAt(i)
            if (!it) continue
            var s = reels && reels.length > i && reels[i] ? String(reels[i]) : ""
            var changed = s !== it.shown
            if (changed || (newSpin && s !== ""))
                it.land(s, rig._reelsReady && rig.motionOk && s !== "" && (newSpin || changed))
        }
        rig._reelKey = rig.shownKey
        if (rig.rd) rig._reelsReady = true
    }
    // the payout flash ("+8" / "Push" / "-2") while the phase is payout
    readonly property string payKey: rig.phase === "payout" && rig.lastSpin ? rig.dialKey + "/" + rig.lastSpin.spin : ""
    readonly property int lastPay: rig.lastSpin ? Number(rig.lastSpin.payout || 0) - Number(rig.lastSpin.bet || 0) : 0
    onPayKeyChanged: if (rig.payKey !== "" && rig.motionOk && Motion.durVeryLong > 0) flashAnim.restart()

    // ── small house parts ──────────────────────────────────────────────────────────────────────────────────────────
    component Txt: Controls.Label {
        property real px: 13          // the web's px size (13 = the default font)
        textFormat: Text.PlainText
        color: Th.textPrimary
        font.pointSize: rig.fp(px)
    }

    // the chip coin (bits.module.css .chip): art.chip with an art.chipEdge ring
    component Coin: Rectangle {
        property bool big: false
        implicitWidth: big ? 14 : 10
        implicitHeight: implicitWidth
        radius: implicitWidth / 2
        color: rig.art.chip || Th.accentBase
        border.width: big ? 3 : 2
        border.color: rig.art.chipEdge || Th.accentBright
    }

    component Face: Avatar {
        property string who: ""
        app: rig.app
        name: rig.nameOf(who)
        seed: who
    }

    // a role pill (Spinner = the machine trim, Mechanic = the accent, the rest quiet)
    component RolePill: Rectangle {
        id: rp
        property string roleId: ""
        readonly property color tone: rp.roleId === "spinner" ? rig.trimC : (rp.roleId === "mechanic" ? Th.accentBright : Th.textSecondary)
        // the web's .roleBadge / .personRole: 2 px × 8 px padding INSIDE a 1 px border (a Rectangle draws its border
        // inside its box, so the border is added on top of the padding), 11 px (0.85 × body) bold
        implicitWidth: rpText.implicitWidth + 2 * (8 + rp.border.width)
        implicitHeight: rpText.implicitHeight + 2 * (2 + rp.border.width)
        radius: height / 2
        color: "transparent"
        border.width: 1
        border.color: rp.roleId === "spinner" || rp.roleId === "mechanic" ? rp.tone : Th.borderStrong
        Accessible.role: Accessible.StaticText
        Accessible.name: rpText.text
        Txt {
            id: rpText
            anchors.centerIn: parent
            text: rig.roleWord(rp.roleId)
            px: 11
            font.bold: true
            color: rp.tone
        }
    }

    // a panel of the right column (Rig.module.css .panel)
    component Panel: Rectangle {
        default property alias content: panelCol.data
        implicitHeight: panelCol.implicitHeight + 20
        radius: Th.shapeRadius
        color: Th.surfaceRaised
        border.width: 1
        border.color: Th.borderSubtle
        ColumnLayout {
            id: panelCol
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.top: parent.top
            anchors.topMargin: 10
            anchors.leftMargin: 12
            anchors.rightMargin: 12
            spacing: 6
        }
    }

    // a panel head: bold, with a muted tail ("Rule Board · 3 live · 11 could be")
    component Head: Flow {
        property string main: ""
        property string tail: ""
        Layout.fillWidth: true
        spacing: 0
        Txt { text: parent.main; px: 12; font.bold: true; color: Th.textSecondary }
        Txt { visible: parent.tail !== ""; text: " · " + parent.tail; px: 12; font.bold: true; color: Th.textMuted }
    }

    component Muted: Txt {
        Layout.fillWidth: true
        px: 12
        color: Th.textMuted
        wrapMode: Text.Wrap
    }

    // a pick chip (Rig.module.css .pickChip): a 28 px pill; `lit` = picked
    component PickChip: Rectangle {
        id: pc
        property string label: ""
        property bool lit: false
        property bool can: true
        signal activated()
        implicitWidth: pcText.implicitWidth + 20
        implicitHeight: 28
        radius: 14
        color: pcMouse.containsMouse && pc.can ? Th.surfaceHover : "transparent"
        border.width: 1
        border.color: pc.lit || (pc.can && (pcMouse.containsMouse || pc.activeFocus)) ? Th.accentBright : Th.borderStrong
        opacity: pc.can ? 1 : 0.5
        activeFocusOnTab: pc.can
        Accessible.role: Accessible.Button
        Accessible.name: pc.label
        Accessible.checkable: pc.lit
        Accessible.checked: pc.lit
        Accessible.onPressAction: if (pc.can) pc.activated()
        Keys.onReturnPressed: if (pc.can) pc.activated()
        Keys.onSpacePressed: if (pc.can) pc.activated()
        Txt {
            id: pcText
            anchors.centerIn: parent
            text: pc.label
            px: 12
            color: pc.lit ? Th.accentBright : Th.textPrimary
        }
        MouseArea {
            id: pcMouse
            anchors.fill: parent
            enabled: pc.can
            hoverEnabled: true
            cursorShape: Qt.PointingHandCursor
            onClicked: pc.activated()
        }
    }

    // a round 28 px step button (the lobby's bankroll − / +)
    component RoundBtn: Rectangle {
        id: rb
        property string label: ""
        property string a11y: ""
        property bool can: true
        signal activated()
        implicitWidth: 28
        implicitHeight: 28
        radius: 14
        color: rbMouse.containsMouse && rb.can ? Th.surfaceHover : "transparent"
        border.width: 1
        border.color: rb.activeFocus ? Th.accentBright : Th.borderStrong
        opacity: rb.can ? 1 : 0.4
        activeFocusOnTab: rb.can
        Accessible.role: Accessible.Button
        Accessible.name: rb.a11y !== "" ? rb.a11y : rb.label
        Accessible.onPressAction: if (rb.can) rb.activated()
        Keys.onReturnPressed: if (rb.can) rb.activated()
        Keys.onSpacePressed: if (rb.can) rb.activated()
        Txt { anchors.centerIn: parent; text: rb.label; px: 15 }
        MouseArea {
            id: rbMouse
            anchors.fill: parent
            enabled: rb.can
            hoverEnabled: true
            cursorShape: Qt.PointingHandCursor
            onClicked: rb.activated()
        }
    }

    // one segment of the lobby's Lite | Full switch
    component SegBtn: Rectangle {
        id: sg
        property string label: ""
        property string tip: ""
        property bool lit: false
        property bool can: true
        signal activated()
        implicitWidth: sgText.implicitWidth + 28
        implicitHeight: 28
        color: sg.lit ? Th.surfaceSelected : (sgMouse.containsMouse && sg.can ? Th.surfaceHover : "transparent")
        activeFocusOnTab: sg.can
        Accessible.role: Accessible.RadioButton
        Accessible.name: sg.label
        Accessible.checkable: true
        Accessible.checked: sg.lit
        Accessible.onPressAction: if (sg.can) sg.activated()
        Keys.onReturnPressed: if (sg.can) sg.activated()
        Keys.onSpacePressed: if (sg.can) sg.activated()
        Controls.ToolTip.visible: sg.tip !== "" && sgMouse.containsMouse
        Controls.ToolTip.text: sg.tip
        Controls.ToolTip.delay: 400
        Txt {
            id: sgText
            anchors.centerIn: parent
            text: sg.label
            font.bold: sg.lit
            color: sg.lit ? Th.textPrimary : Th.textSecondary
            opacity: sg.can ? 1 : 0.4
        }
        MouseArea {
            id: sgMouse
            anchors.fill: parent
            hoverEnabled: true
            cursorShape: sg.can ? Qt.PointingHandCursor : Qt.ArrowCursor
            onClicked: if (sg.can) sg.activated()
        }
    }

    // a Rule Board card (Rig.module.css .rCard): name + bite dots, the rule, "Reel card"/"Payout card", Tax, the state
    component BoardCard: Rectangle {
        id: bc
        property string cardId: ""
        property string cardState: "open"
        property int bustSpin: 0
        property bool mine: false
        property bool pickable: false
        signal activated()
        readonly property var info: rig.cardInfo(bc.cardId)
        readonly property bool dim: bc.cardState === "dead" || bc.cardState === "wrong"
        readonly property string stateWord: bc.cardState === "dead" ? rig.nw("out", "Out")
            : (bc.cardState === "busted" ? (bc.bustSpin > 0 ? rig.fill(rig.nw("busted_on", "Busted on spin {n}"), { n: bc.bustSpin }) : rig.rw("busted"))
            : (bc.cardState === "wrong" ? rig.stampWord("called-wrong") : ""))
        implicitWidth: 150
        implicitHeight: Math.max(86, bcTop.implicitHeight + bcFoot.implicitHeight + 22)
        radius: 8
        color: Th.surfaceRaised
        border.width: bc.mine ? 2 : 1
        border.color: bc.mine || (bc.pickable && (bcMouse.containsMouse || bc.activeFocus)) ? Th.accentBright
                    : (bc.cardState === "busted" ? rig.winC : Th.borderStrong)
        opacity: bc.dim ? 0.42 : 1
        activeFocusOnTab: bc.pickable
        Accessible.role: bc.pickable ? Accessible.Button : Accessible.StaticText
        Accessible.name: (bc.info.name || "") + ". " + (bc.info.rule || "") + (bc.stateWord !== "" ? ". " + bc.stateWord : "")
        Accessible.onPressAction: if (bc.pickable) bc.activated()
        Keys.onReturnPressed: if (bc.pickable) bc.activated()
        Keys.onSpacePressed: if (bc.pickable) bc.activated()

        Rectangle {   // my card's glow
            visible: bc.mine
            anchors.fill: parent
            anchors.margins: -4
            radius: bc.radius + 4
            color: "transparent"
            border.width: 3
            border.color: Th.accentGlow
        }
        ColumnLayout {
            id: bcTop
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.top: parent.top
            anchors.topMargin: 8
            anchors.leftMargin: 10
            anchors.rightMargin: 10
            spacing: 4
            RowLayout {
                Layout.fillWidth: true
                spacing: 6
                Txt {
                    Layout.fillWidth: true
                    Layout.alignment: Qt.AlignTop
                    text: bc.info.name || ""
                    px: 12.5
                    font.bold: true
                    wrapMode: Text.Wrap
                }
                Txt {
                    Layout.alignment: Qt.AlignTop
                    text: "●".repeat(Math.max(0, Number(bc.info.bite || 0)))
                    px: 8
                    font.letterSpacing: 1
                    color: rig.lossC
                    Accessible.name: "bite " + Number(bc.info.bite || 0)
                }
            }
            Txt {
                Layout.fillWidth: true
                text: bc.info.rule || ""
                px: 11.5
                color: Th.textSecondary
                wrapMode: Text.Wrap
            }
        }
        RowLayout {
            id: bcFoot
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.bottom: parent.bottom
            anchors.bottomMargin: 8
            anchors.leftMargin: 10
            anchors.rightMargin: 10
            spacing: 6
            Txt {
                text: bc.info.kind === "reel" ? rig.nw("reel_card", "Reel card") : rig.nw("payout_card", "Payout card")
                px: 10
                color: Th.textMuted
            }
            Rectangle {
                visible: !!bc.info.tax
                implicitWidth: taxText.implicitWidth + 10
                implicitHeight: taxText.implicitHeight + 2
                radius: 4
                color: "transparent"
                border.width: 1
                border.color: Th.borderStrong
                Txt { id: taxText; anchors.centerIn: parent; text: rig.rw("tax"); px: 10; color: Th.textMuted }
            }
            Item { Layout.fillWidth: true }
            Txt {
                visible: bc.stateWord !== ""
                text: bc.stateWord
                px: 10
                font.bold: true
                color: bc.cardState === "busted" ? rig.winC : Th.textMuted
            }
        }
        MouseArea {
            id: bcMouse
            anchors.fill: parent
            enabled: bc.pickable
            hoverEnabled: true
            cursorShape: Qt.PointingHandCursor
            onClicked: bc.activated()
        }
    }

    // a Nudge arrow on a reel (Rig.module.css .nudgeBtn)
    component NudgeBtn: Rectangle {
        id: nb
        property string glyph: ""
        property string a11y: ""
        signal activated()
        implicitWidth: 40
        implicitHeight: 22
        radius: 11
        color: nbMouse.containsMouse ? Th.surfaceHover : "transparent"
        border.width: nb.activeFocus ? 2 : 1
        border.color: Th.accentBright
        activeFocusOnTab: true
        Accessible.role: Accessible.Button
        Accessible.name: nb.a11y
        Accessible.onPressAction: nb.activated()
        Keys.onReturnPressed: nb.activated()
        Keys.onSpacePressed: nb.activated()
        Txt { anchors.centerIn: parent; text: nb.glyph; px: 11; color: Th.accentBright }
        MouseArea {
            id: nbMouse
            anchors.fill: parent
            hoverEnabled: true
            cursorShape: Qt.PointingHandCursor
            onClicked: nb.activated()
        }
    }

    // one reel: the House's ▲ (back) over the window, the window, ▼ (forward) under it. land(sym, true) = a one-shot drop
    // (the old symbol rolls out below, the new one in from above; the reels start one short step apart). No loop; with
    // motion off the new symbol is simply there.
    component ReelBox: Column {
        id: reel
        property int idx: 0
        property string shown: ""
        property string prevSym: ""
        property real roll: 0
        property bool nudgeable: false
        function land(sym, animate) {
            rollAnim.stop()
            reel.prevSym = reel.shown
            reel.shown = sym
            if (animate && Motion.durVeryLong > 0) { reel.roll = 1; rollAnim.restart() }
            else reel.roll = 0
        }
        spacing: 4
        NudgeBtn {
            visible: reel.nudgeable
            anchors.horizontalCenter: parent.horizontalCenter
            glyph: "▲"
            a11y: rig.fill(rig.nw("nudge_back", "Nudge reel {n} back"), { n: reel.idx + 1 })
            onActivated: rig.act({ do: "nudge", reel: reel.idx + 1, step: -1 })
        }
        Rectangle {
            id: win
            width: 96
            height: 108
            radius: 12
            color: rig.art.reelWindow || Th.surfaceSunken
            border.width: 1
            border.color: rig.art.reelLine || Th.borderSubtle
            clip: true
            Accessible.role: Accessible.StaticText
            Accessible.name: rig.fill(rig.nw("reel_n", "Reel {n}"), { n: reel.idx + 1 }) + ": " + (reel.shown !== "" ? rig.symWord(reel.shown) : "?")

            RigSymbol {
                visible: reel.roll > 0 && reel.prevSym !== ""
                sym: reel.prevSym
                size: 64
                art: rig.art
                x: (win.width - 64) / 2
                y: (win.height - 64) / 2 + (1 - reel.roll) * win.height
                opacity: reel.roll
            }
            RigSymbol {
                visible: reel.shown !== ""
                sym: reel.shown
                label: rig.symWord(reel.shown)
                size: 64
                art: rig.art
                x: (win.width - 64) / 2
                y: (win.height - 64) / 2 - reel.roll * win.height
            }
            Txt {
                visible: reel.shown === ""
                anchors.centerIn: parent
                text: "?"
                px: 34
                color: Th.textMuted
            }
            // the drum: inner shade at the top and the bottom, then the glass
            Rectangle {
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.top: parent.top
                height: 28
                radius: win.radius
                gradient: Gradient {
                    GradientStop { position: 0.0; color: rig.art.shadeDeep || "transparent" }
                    GradientStop { position: 1.0; color: "transparent" }
                }
            }
            Rectangle {
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.bottom: parent.bottom
                height: 28
                radius: win.radius
                gradient: Gradient {
                    GradientStop { position: 0.0; color: "transparent" }
                    GradientStop { position: 1.0; color: rig.art.shadeDeep || "transparent" }
                }
            }
            Rectangle {
                anchors.fill: parent
                radius: win.radius
                gradient: Gradient {
                    GradientStop { position: 0.0; color: rig.art.reelGlass || "transparent" }
                    GradientStop { position: 0.4; color: "transparent" }
                    GradientStop { position: 0.6; color: "transparent" }
                    GradientStop { position: 1.0; color: rig.art.reelGlass || "transparent" }
                }
            }
        }
        NudgeBtn {
            visible: reel.nudgeable
            anchors.horizontalCenter: parent.horizontalCenter
            glyph: "▼"
            a11y: rig.fill(rig.nw("nudge_forward", "Nudge reel {n} forward"), { n: reel.idx + 1 })
            onActivated: rig.act({ do: "nudge", reel: reel.idx + 1, step: 1 })
        }
        SequentialAnimation {
            id: rollAnim
            PauseAnimation { duration: Motion.durShort * reel.idx }
            NumberAnimation { target: reel; property: "roll"; from: 1; to: 0; duration: Motion.durVeryLong; easing.type: Easing.OutBack }
        }
    }

    // a bet dial circle (Rig.module.css .dialBtn)
    component DialBtn: Rectangle {
        id: db
        property int n: 1
        readonly property bool lit: rig.dialShown === db.n
        readonly property bool can: rig.canTurn && db.n <= rig.bankNow
        implicitWidth: 34
        implicitHeight: 34
        radius: 17
        color: rig.art.shadeSoft || "transparent"
        border.width: db.activeFocus ? 2 : 1
        border.color: db.lit ? rig.trimC : Th.borderStrong
        opacity: db.lit || db.can ? 1 : 0.5
        activeFocusOnTab: db.can
        Accessible.role: Accessible.RadioButton
        Accessible.name: rig.rw("dial") + " " + db.n
        Accessible.checkable: true
        Accessible.checked: db.lit
        Accessible.onPressAction: if (db.can) rig.setDial(db.n)
        Keys.onReturnPressed: if (db.can) rig.setDial(db.n)
        Keys.onSpacePressed: if (db.can) rig.setDial(db.n)
        Rectangle {   // the lit glow
            visible: db.lit
            anchors.fill: parent
            anchors.margins: -3
            radius: width / 2
            color: "transparent"
            border.width: 3
            border.color: rig.trimC
            opacity: 0.35
        }
        Txt {
            anchors.centerIn: parent
            text: String(db.n)
            font.bold: true
            color: db.lit ? rig.trimC : Th.textPrimary
        }
        MouseArea {
            anchors.fill: parent
            enabled: db.can
            cursorShape: Qt.PointingHandCursor
            onClicked: rig.setDial(db.n)
        }
    }

    // the host's Start box (the lobby, and another table at the end)
    component StartBox: Rectangle {
        id: sb
        property string startText: ""
        readonly property int n: rig.activePlayers.length
        Layout.alignment: Qt.AlignHCenter
        Layout.preferredWidth: Math.max(320, sbCol.implicitWidth + 32)
        implicitHeight: sbCol.implicitHeight + 28
        radius: Th.shapeRadius
        color: Th.surfaceRaised
        border.width: 1
        border.color: Th.borderSubtle
        ColumnLayout {
            id: sbCol
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.top: parent.top
            anchors.topMargin: 14
            anchors.leftMargin: 16
            anchors.rightMargin: 16
            spacing: 10
            RowLayout {
                Layout.fillWidth: true
                spacing: 12
                Txt { Layout.fillWidth: true; text: rig.nw("machine", "Machine"); font.bold: true; color: Th.textSecondary }
                Rectangle {
                    implicitWidth: segRow.implicitWidth + 2
                    implicitHeight: 30
                    radius: 15
                    color: "transparent"
                    border.width: 1
                    border.color: Th.borderStrong
                    clip: true
                    Row {
                        id: segRow
                        anchors.centerIn: parent
                        SegBtn {
                            label: rig.rw("lite")
                            lit: rig.hostLite
                            onActivated: rig.hostPreset = "lite"
                        }
                        SegBtn {
                            label: rig.rw("full")
                            lit: !rig.hostLite
                            can: sb.n > 3
                            tip: sb.n > 3 ? "" : rig.nw("full_needs", "Full needs 4 or more players")
                            onActivated: rig.hostPreset = "full"
                        }
                    }
                }
            }
            RowLayout {
                Layout.fillWidth: true
                spacing: 12
                Txt { Layout.fillWidth: true; text: rig.nw("bankroll_label", "Bankroll"); font.bold: true; color: Th.textSecondary }
                RowLayout {
                    spacing: 10
                    RoundBtn {
                        label: "−"
                        a11y: rig.fill(rig.rw("bankroll"), { n: rig.hostBankroll - 1 })
                        can: rig.hostBankroll > 3
                        onActivated: rig.hostBankroll = Math.max(3, rig.hostBankroll - 1)
                    }
                    Row {
                        spacing: 4
                        Accessible.role: Accessible.StaticText
                        Accessible.name: rig.fill(rig.rw("bankroll"), { n: rig.hostBankroll })
                        Coin { big: true; anchors.verticalCenter: parent.verticalCenter }
                        Txt { text: String(rig.hostBankroll); px: 15; font.bold: true }
                    }
                    RoundBtn {
                        label: "+"
                        a11y: rig.fill(rig.rw("bankroll"), { n: rig.hostBankroll + 1 })
                        can: rig.hostBankroll < 10
                        onActivated: rig.hostBankroll = Math.min(10, rig.hostBankroll + 1)
                    }
                }
            }
            RowLayout {
                visible: !rig.hostLite
                Layout.fillWidth: true
                spacing: 12
                Txt { text: rig.rw("patch_option"); font.bold: true; color: Th.textSecondary }
                Item { Layout.fillWidth: true }
                ThSwitch {
                    checked: rig.hostPatch
                    Accessible.name: rig.nw("patch_explain", "Busted Mechanics may refill their slot")
                    onToggled: rig.hostPatch = checked
                }
                Txt {
                    Layout.maximumWidth: 200
                    text: rig.nw("patch_explain", "Busted Mechanics may refill their slot")
                    px: 12
                    wrapMode: Text.Wrap
                }
            }
            ThButton {
                Layout.fillWidth: true
                app: rig.app
                text: sb.n < 3 ? rig.fill(rig.nw("needs_more", "Needs {n} more"), { n: 3 - sb.n }) : sb.startText
                highlighted: true
                enabled: sb.n >= 3 && sb.n <= 8
                implicitHeight: 38
                leftPadding: Math.max(14, (width - implicitContentWidth) / 2)   // the label centred, like the web's
                font.bold: true
                Accessible.name: text
                onClicked: rig.startTable()
            }
        }
    }

    // ══ the table ═════════════════════════════════════════════════════════════════════════════════════════════════
    ColumnLayout {
        id: frame
        anchors.fill: parent
        spacing: 0

        // ── the top line ──
        Item {
            Layout.fillWidth: true
            implicitHeight: barRow.implicitHeight + 16
            RowLayout {
                id: barRow
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                anchors.leftMargin: 12
                anchors.rightMargin: 12
                spacing: 8
                Txt {
                    Layout.fillWidth: true
                    text: rig.metaText
                    px: 12
                    color: Th.textMuted
                    elide: Text.ElideRight
                }
                RolePill {
                    visible: rig.joined && rig.myRole !== ""
                    roleId: rig.myRole
                }
                ThButton {
                    visible: !rig.joined && rig.tableStatus === "lobby"
                    app: rig.app
                    text: rig.rw("join")
                    highlighted: true
                    enabled: rig.activePlayers.length < 8
                    implicitHeight: 30
                    font.pointSize: rig.fp(12)
                    Accessible.name: text
                    onClicked: rig.act({ do: "join" })
                }
                ThButton {
                    visible: rig.joined && rig.tableStatus !== "closed"
                    app: rig.app
                    text: String(rig.wd("table", "leave_table", "Leave table"))
                    flat: true
                    implicitHeight: 30
                    font.pointSize: rig.fp(12)
                    Accessible.name: text
                    onClicked: rig.act({ do: "leave" })
                }
                ThButton {
                    visible: rig.isHost && !!rig.view && rig.tableStatus !== "closed"
                    app: rig.app
                    text: String(rig.wd("table", "close", "Close table"))
                    flat: true
                    implicitHeight: 30
                    font.pointSize: rig.fp(12)
                    Accessible.name: text
                    onClicked: closeAsk.open()
                }
            }
            Rectangle {
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.bottom: parent.bottom
                height: 1
                color: Th.borderSubtle
            }
        }
        ConfirmDialog {
            id: closeAsk
            app: rig.app
            dialogTitle: rig.nw("close_title", "Close The Rig?")
            message: rig.nw("close_body", "The table closes after this round's reveal. Every wager is settled first.")
            confirmText: String(rig.wd("table", "close", "Close table"))
            destructive: true
            onConfirmed: rig.act({ do: "close" })
        }

        Txt {
            Layout.fillWidth: true
            Layout.leftMargin: 12
            Layout.rightMargin: 12
            Layout.topMargin: 6
            visible: !!(rig.you && rig.you.at_cap)
            text: String(rig.wd("wallet", "at_cap_table", ""))
            px: 12
            color: Th.accentBright
            wrapMode: Text.Wrap
        }

        Flickable {
            id: flick
            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
            contentWidth: width
            contentHeight: body.implicitHeight + 24
            boundsBehavior: Flickable.StopAtBounds
            Controls.ScrollBar.vertical: FoxScrollBar { app: rig.app }

            ColumnLayout {
                id: body
                x: 12
                y: 12
                width: flick.width - 24
                spacing: 12

                Txt {
                    visible: !rig.view
                    Layout.fillWidth: true
                    text: String(rig.wd("card", "loading", "…"))
                    color: Th.textSecondary
                    horizontalAlignment: Text.AlignHCenter
                }
                Txt {
                    visible: rig.tableStatus === "closed"
                    Layout.fillWidth: true
                    Layout.topMargin: 12
                    text: String(rig.wd("table", "closed", ""))
                    color: Th.textSecondary
                    horizontalAlignment: Text.AlignHCenter
                    wrapMode: Text.Wrap
                }

                // ══ LOBBY ══
                ColumnLayout {
                    visible: rig.tableStatus === "lobby"
                    Layout.fillWidth: true
                    Layout.maximumWidth: Number.POSITIVE_INFINITY   // a layout is otherwise capped by its children's sizes
                    Layout.topMargin: 12
                    spacing: 18
                    ColumnLayout {
                        Layout.alignment: Qt.AlignHCenter
                        Layout.maximumWidth: 460
                        Layout.fillWidth: true
                        spacing: 6
                        Txt {
                            Layout.fillWidth: true
                            text: rig.nw("lobby_title", "Rig the machine. Catch the riggers.")
                            px: 20
                            font.weight: Font.ExtraBold
                            horizontalAlignment: Text.AlignHCenter
                            wrapMode: Text.Wrap
                        }
                        Txt {
                            Layout.fillWidth: true
                            text: rig.nw("lobby_sub", "Everyone spins once. While you spin, the others secretly rig the machine against you. Call their cards, audit a spin, walk away rich.")
                            color: Th.textSecondary
                            horizontalAlignment: Text.AlignHCenter
                            wrapMode: Text.Wrap
                            lineHeight: 1.15
                        }
                    }
                    Flow {
                        id: lobbyPeople
                        readonly property int count: rig.activePlayers.length + Math.max(0, 3 - rig.activePlayers.length)
                        Layout.alignment: Qt.AlignHCenter
                        Layout.preferredWidth: Math.min(body.width, lobbyPeople.count * 92 - 16)
                        spacing: 16
                        Repeater {
                            model: rig.tableStatus === "lobby" ? rig.activePlayers : []
                            Item {
                                id: lp
                                required property var modelData
                                width: 76
                                height: lpCol.implicitHeight
                                opacity: lp.modelData.connected === false ? 0.5 : 1
                                Column {
                                    id: lpCol
                                    width: parent.width
                                    spacing: 6
                                    Face {
                                        anchors.horizontalCenter: parent.horizontalCenter
                                        who: lp.modelData.u
                                        size: 40
                                    }
                                    Txt {
                                        width: parent.width
                                        text: rig.nameOf(lp.modelData.u)
                                        px: 12
                                        font.bold: lp.modelData.u === rig.myId
                                        elide: Text.ElideRight
                                        horizontalAlignment: Text.AlignHCenter
                                    }
                                }
                                Rectangle {   // the host's kick
                                    visible: rig.isHost && lp.modelData.u !== rig.myId
                                    x: parent.width / 2 + 10
                                    y: -4
                                    width: 18
                                    height: 18
                                    radius: 9
                                    color: kickMouse.containsMouse ? Th.surfaceHover : Th.surfaceRaised
                                    border.width: kick.activeFocus ? 2 : 1
                                    border.color: Th.borderStrong
                                    Item {
                                        id: kick
                                        anchors.fill: parent
                                        activeFocusOnTab: parent.visible
                                        Accessible.role: Accessible.Button
                                        Accessible.name: "Remove " + rig.nameOf(lp.modelData.u)
                                        Accessible.onPressAction: rig.act({ do: "kick", u: lp.modelData.u })
                                        Keys.onReturnPressed: rig.act({ do: "kick", u: lp.modelData.u })
                                        Keys.onSpacePressed: rig.act({ do: "kick", u: lp.modelData.u })
                                    }
                                    Txt { anchors.centerIn: parent; text: "✕"; px: 9; color: Th.textSecondary }
                                    MouseArea {
                                        id: kickMouse
                                        anchors.fill: parent
                                        hoverEnabled: true
                                        cursorShape: Qt.PointingHandCursor
                                        onClicked: rig.act({ do: "kick", u: lp.modelData.u })
                                    }
                                }
                            }
                        }
                        Repeater {
                            model: rig.tableStatus === "lobby" ? Math.max(0, 3 - rig.activePlayers.length) : 0
                            Column {
                                width: 76
                                spacing: 6
                                opacity: 0.5
                                Rectangle {
                                    anchors.horizontalCenter: parent.horizontalCenter
                                    width: 40
                                    height: 40
                                    radius: 20
                                    color: "transparent"
                                    border.width: 1.5
                                    border.color: Th.borderStrong
                                }
                                Txt {
                                    width: parent.width
                                    text: rig.nw("open_seat", "Open seat")
                                    px: 12
                                    horizontalAlignment: Text.AlignHCenter
                                }
                            }
                        }
                    }
                    StartBox {
                        visible: rig.isHost
                        startText: rig.nw("start_first", "Start the first round")
                    }
                    Txt {
                        visible: !rig.isHost
                        Layout.alignment: Qt.AlignHCenter
                        text: {
                            var n = rig.activePlayers.length
                            if (n >= 3) return rig.nw("wait_host", "Waiting for the host to start")
                            return 3 - n === 1 ? rig.nw("wait_one", "Waiting for 1 more player")
                                               : rig.fill(rig.nw("wait_n", "Waiting for {n} more players"), { n: 3 - n })
                        }
                        color: Th.textSecondary
                    }
                }

                // ══ TABLE END ══ (the web's .lobby.endWrap: the block centred in the panel, 24 px above and below)
                Item {
                    id: endPane
                    visible: rig.tableStatus === "end"
                    Layout.fillWidth: true
                    Layout.maximumWidth: Number.POSITIVE_INFINITY
                    implicitHeight: Math.max(flick.height - 24, endCol.implicitHeight + 48)
                    ColumnLayout {
                        id: endCol
                        anchors.left: parent.left
                        anchors.right: parent.right
                        anchors.verticalCenter: parent.verticalCenter
                        spacing: 18
                        Txt {
                            Layout.alignment: Qt.AlignHCenter
                            text: rig.nw("everyone_spun", "Everyone has spun")
                            px: 20
                            font.weight: Font.ExtraBold
                        }
                        ColumnLayout {
                            Layout.alignment: Qt.AlignHCenter
                            Layout.fillWidth: false
                            Layout.preferredWidth: Math.min(body.width, 360)
                            spacing: 6
                            Repeater {
                                model: rig.tableStatus === "end" && rig.tableEnd && rig.tableEnd.leaderboard ? rig.tableEnd.leaderboard : []
                                Rectangle {
                                    id: lead
                                    required property var modelData
                                    required property int index
                                    readonly property bool boss: !!rig.tableEnd && rig.tableEnd.pit_boss === lead.modelData.u
                                    Layout.fillWidth: true
                                    implicitHeight: leadRow.implicitHeight + 16
                                    radius: 8
                                    color: Th.surfaceRaised
                                    border.width: 1
                                    border.color: lead.boss ? rig.trimC : Th.borderSubtle
                                    Accessible.role: Accessible.StaticText
                                    Accessible.name: (lead.index + 1) + ". " + rig.nameOf(lead.modelData.u) + " " + rig.signed(lead.modelData.net)
                                                     + (lead.boss ? ", " + rig.rw("pit_boss") : "")
                                    RowLayout {
                                        id: leadRow
                                        anchors.left: parent.left
                                        anchors.right: parent.right
                                        anchors.verticalCenter: parent.verticalCenter
                                        anchors.leftMargin: 12
                                        anchors.rightMargin: 12
                                        spacing: 10
                                        Txt { Layout.preferredWidth: 18; text: String(lead.index + 1); font.bold: true; color: Th.textMuted }
                                        Face { who: lead.modelData.u; size: 28 }
                                        Txt {
                                            Layout.fillWidth: true
                                            text: rig.nameOf(lead.modelData.u)
                                            font.bold: true
                                            color: rig.isSomeone(lead.modelData.u) ? Th.textMuted : Th.textPrimary
                                            elide: Text.ElideRight
                                        }
                                        Txt {
                                            text: rig.signed(lead.modelData.net)
                                            px: 12
                                            font.bold: true
                                            color: lead.modelData.net > 0 ? rig.winC : (lead.modelData.net < 0 ? rig.lossC : Th.textSecondary)
                                        }
                                        Txt {
                                            visible: lead.boss
                                            text: rig.rw("pit_boss")
                                            px: 11
                                            font.weight: Font.ExtraBold
                                            color: rig.trimC
                                        }
                                    }
                                }
                            }
                        }
                        // the spec's TABLE END: "Host: another table or close (= everyone leaves)". The host
                        // sees New table (a fresh Rig table in this room, the same options) + Close (the bar's confirm);
                        // everyone else sees Back to Games (the web's rig-end-back). No same-table restart.
                        RowLayout {
                            Layout.alignment: Qt.AlignHCenter
                            spacing: 8
                            ThButton {
                                id: endBack
                                visible: !rig.isHost
                                app: rig.app
                                text: String(rig.wd("lobby", "closed_back", "Back to Games"))
                                highlighted: true
                                Accessible.name: text
                                onClicked: {
                                    if (rig.play) rig.play.closeTable()
                                    if (rig.app) rig.app.gamePanelOpen = true
                                }
                            }
                            ThButton {
                                id: endNewTable
                                visible: rig.isHost
                                app: rig.app
                                text: String(rig.rw("new_table"))
                                highlighted: true
                                Accessible.name: text
                                // the server still counts this ended table as the room's Rig table (only closed ones
                                // don't count: one-per-room), so close it first; the fresh one opens once that's acked
                                // (PlayController.newTableAfterClose, as the web's newTable)
                                onClicked: if (rig.play) rig.play.newTableAfterClose()
                            }
                            ThButton {
                                id: endClose
                                visible: rig.isHost
                                app: rig.app
                                text: String(rig.wd("table", "close", "Close table"))
                                Accessible.name: text
                                onClicked: closeAsk.open()
                            }
                        }
                    }
                }

                // ══ ROUND ══
                GridLayout {
                    visible: rig.inRound
                    Layout.fillWidth: true
                    columns: rig.wide ? 2 : 1
                    columnSpacing: 12
                    rowSpacing: 12

                    ColumnLayout {
                        id: mainCol
                        Layout.fillWidth: true
                        Layout.alignment: Qt.AlignTop
                        spacing: 12

                        // ── the MACHINE ──
                        Item {
                            id: machine
                            Layout.fillWidth: true
                            Layout.leftMargin: 4
                            Layout.rightMargin: 4
                            Layout.topMargin: 4
                            implicitHeight: machineCol.implicitHeight + 28
                            readonly property bool wideCtl: machine.width >= 540
                            Rectangle {   // the outer ring
                                anchors.fill: parent
                                anchors.margins: -4
                                radius: 22
                                color: rig.art.shade || "transparent"
                            }
                            Rectangle {   // the cabinet (ui.json art: the same in every theme)
                                anchors.fill: parent
                                radius: 18
                                border.width: 1
                                border.color: rig.trimC
                                gradient: Gradient {
                                    GradientStop { position: 0.0; color: rig.art.machineBody || Th.surfaceSunken }
                                    GradientStop { position: 1.0; color: rig.art.reelWindow || Th.surfaceSunken }
                                }
                            }
                            ColumnLayout {
                                id: machineCol
                                anchors.left: parent.left
                                anchors.right: parent.right
                                anchors.top: parent.top
                                anchors.margins: 14
                                spacing: 12

                                // who spins · Spin n of N · the bankroll
                                RowLayout {
                                    Layout.fillWidth: true
                                    spacing: 8
                                    RowLayout {
                                        spacing: 6
                                        Face { who: rig.rd ? String(rig.rd.spinner || "") : ""; size: 22 }
                                        Txt {
                                            Layout.maximumWidth: 140
                                            text: rig.rd ? rig.nameOf(rig.rd.spinner) : ""
                                            font.bold: true
                                            elide: Text.ElideRight
                                        }
                                        Txt { text: rig.nw("spins", "spins"); px: 12; color: Th.textMuted }
                                    }
                                    Item { Layout.fillWidth: true }
                                    Txt {
                                        text: rig.rd ? rig.fill(rig.rw("spin_n"), { n: Math.max(1, Number(rig.rd.spin || 0)), max: rig.rd.spins }) : ""
                                        color: Th.textSecondary
                                    }
                                    Item { Layout.fillWidth: true }
                                    RowLayout {
                                        spacing: 8
                                        Accessible.role: Accessible.StaticText
                                        Accessible.name: rig.fill(rig.rw("bankroll"), { n: rig.bankNow }) + (rig.bankNet !== 0 ? " " + rig.signed(rig.bankNet) : "")
                                        RowLayout {
                                            spacing: 4
                                            Coin { big: true }
                                            Txt { text: String(rig.bankNow); px: 15; font.bold: true }
                                        }
                                        Txt {
                                            visible: rig.bankNet !== 0
                                            text: rig.bankNet > 0 ? "+" + rig.bankNet : "−" + (-rig.bankNet)
                                            px: 12
                                            font.bold: true
                                            color: rig.bankNet > 0 ? rig.winC : rig.lossC
                                        }
                                    }
                                }

                                // the reels (+ the House's Nudge arrows) and the payout flash
                                Item {
                                    Layout.fillWidth: true
                                    implicitHeight: reelRow.implicitHeight
                                    Row {
                                        id: reelRow
                                        anchors.horizontalCenter: parent.horizontalCenter
                                        spacing: 10
                                        Repeater {
                                            id: reelRep
                                            model: 3
                                            ReelBox {
                                                required property int index
                                                idx: index
                                                // the House's Nudge (Full; the 4-second window). Hidden while I share my screen.
                                                nudgeable: !!rig.feats.nudge && rig.phase === "nudge" && !!rig.rd && !!rig.rd.nudge && !!rig.rd.nudge.open
                                                           && !rig.isSpinner && rig.isHouse && Number(rig.me.nudge_left || 0) > 0 && !rig.hideSecret
                                            }
                                        }
                                    }
                                    Rectangle {
                                        id: payFlash
                                        visible: rig.payKey !== ""
                                        anchors.right: parent.right
                                        anchors.rightMargin: 6
                                        y: -6
                                        implicitWidth: payText.implicitWidth + 20
                                        implicitHeight: payText.implicitHeight + 6
                                        radius: height / 2
                                        color: rig.art.shadeDeep || Th.surfaceSunken
                                        Accessible.role: Accessible.StaticText
                                        Accessible.name: payText.text
                                        Txt {
                                            id: payText
                                            anchors.centerIn: parent
                                            text: rig.lastPay > 0 ? "+" + rig.lastPay : (rig.lastPay === 0 ? rig.nw("push", "Push") : String(rig.lastPay))
                                            px: 15
                                            font.weight: Font.ExtraBold
                                            color: rig.lastPay > 0 ? rig.winC : (rig.lastPay < 0 ? rig.lossC : Th.textPrimary)
                                        }
                                        ParallelAnimation {
                                            id: flashAnim
                                            NumberAnimation { target: payFlash; property: "scale"; from: 1.4; to: 1; duration: Motion.durVeryLong; easing.type: Easing.OutCubic }
                                            NumberAnimation { target: payFlash; property: "opacity"; from: 0; to: 1; duration: Motion.durVeryLong; easing.type: Easing.OutCubic }
                                        }
                                    }
                                }

                                Txt {
                                    Layout.fillWidth: true
                                    visible: !!rig.rd && !!rig.rd.nudge && !rig.rd.nudge.open && !!rig.rd.nudge.reel
                                    text: rig.rd && rig.rd.nudge ? rig.fill(rig.nw("someone_nudged", "Someone nudged reel {n}."), { n: rig.rd.nudge.reel }) : ""
                                    color: Th.textSecondary
                                    wrapMode: Text.Wrap
                                }
                                Txt {
                                    Layout.fillWidth: true
                                    visible: !!rig.rd && !!rig.rd.paused
                                    text: rig.rd && rig.rd.paused
                                          ? rig.fill(rig.nw("paused_line", "Paused: the Spinner dropped out ({n} s left before they walk)."),
                                                     { n: Math.ceil(Number(rig.rd.paused.budget_ms || 0) / 1000) }) : ""
                                    color: Th.textSecondary
                                    wrapMode: Text.Wrap
                                }

                                // the bet dial · the Spinner's controls or the status line
                                GridLayout {
                                    Layout.fillWidth: true
                                    columns: machine.wideCtl ? 3 : 1
                                    columnSpacing: 10
                                    rowSpacing: 10
                                    RowLayout {
                                        spacing: 6
                                        Accessible.role: Accessible.Grouping
                                        Accessible.name: rig.rw("dial")
                                        Txt { text: rig.rw("dial"); font.bold: true; color: Th.textSecondary; Layout.rightMargin: 2 }
                                        DialBtn { n: 1 }
                                        DialBtn { n: 2 }
                                        DialBtn { n: 3 }
                                    }
                                    Item { visible: machine.wideCtl; Layout.fillWidth: true }
                                    RowLayout {
                                        visible: rig.canTurn
                                        spacing: 6
                                        ThButton {
                                            app: rig.app
                                            text: rig.pickMode === "call" ? rig.nw("pick_call", "Pick a card to call…")
                                                                         : rig.rw("call_card") + " (" + rig.callsLeft + ")"
                                            flat: true
                                            enabled: rig.canCall
                                            Accessible.name: text
                                            Accessible.checkable: true
                                            Accessible.checked: rig.pickMode === "call"
                                            onClicked: rig.togglePick("call")
                                        }
                                        ThButton {
                                            app: rig.app
                                            text: rig.pickMode === "audit" ? rig.nw("pick_audit", "Pick a spin…") : rig.rw("audit")
                                            flat: true
                                            enabled: rig.canAudit
                                            Accessible.name: text
                                            Accessible.checkable: true
                                            Accessible.checked: rig.pickMode === "audit"
                                            onClicked: rig.togglePick("audit")
                                        }
                                        ThButton {
                                            id: walkBtn
                                            app: rig.app
                                            text: rig.rw("walk")
                                            flat: true
                                            enabled: rig.canWalk
                                            Accessible.name: text
                                            Accessible.description: Controls.ToolTip.text
                                            Controls.ToolTip.visible: walkBtn.hovered
                                            Controls.ToolTip.delay: 400
                                            Controls.ToolTip.text: rig.rd && rig.rd.spin < rig.rd.walk_from
                                                                   ? rig.fill(rig.nw("walk_from", "You can walk from spin {n}"), { n: rig.rd.walk_from })
                                                                   : rig.nw("walk_tip", "Keep your bankroll and end the round")
                                            onClicked: { rig.pickMode = ""; rig.act({ do: "walk" }) }
                                        }
                                        Rectangle {   // SPIN
                                            id: spinBtn
                                            readonly property bool can: rig.canTurn && rig.bankNow >= 1
                                            implicitWidth: Math.max(112, spinText.implicitWidth + 36)
                                            implicitHeight: 44
                                            radius: 22
                                            opacity: spinBtn.can ? (spinMouse.pressed ? 0.9 : 1) : 0.55
                                            border.width: spinBtn.activeFocus ? 2 : 0
                                            border.color: Th.accentBright
                                            gradient: Gradient {
                                                GradientStop { position: 0.0; color: rig.trimC }
                                                GradientStop { position: 1.0; color: rig.art.machineTrimDark || rig.trimC }
                                            }
                                            activeFocusOnTab: spinBtn.can
                                            Accessible.role: Accessible.Button
                                            Accessible.name: rig.rw("spin")
                                            Accessible.onPressAction: rig.doSpin()
                                            Keys.onReturnPressed: rig.doSpin()
                                            Keys.onSpacePressed: rig.doSpin()
                                            Txt {
                                                id: spinText
                                                anchors.centerIn: parent
                                                anchors.verticalCenterOffset: spinMouse.pressed ? 1 : 0
                                                text: rig.rw("spin")
                                                px: 16
                                                font.weight: Font.Black
                                                font.letterSpacing: 1.5
                                                color: rig.art.spinFg || Th.textOnAccent
                                            }
                                            MouseArea {
                                                id: spinMouse
                                                anchors.fill: parent
                                                enabled: spinBtn.can
                                                cursorShape: Qt.PointingHandCursor
                                                onClicked: rig.doSpin()
                                            }
                                        }
                                    }
                                    Txt {
                                        visible: !rig.canTurn
                                        Layout.fillWidth: !machine.wideCtl
                                        text: rig.isSpinner ? rig.phaseText(rig.phase)
                                              : (rig.phase === "turn" ? rig.nw("choosing", "The Spinner is choosing a bet") : rig.phaseText(rig.phase))
                                        color: Th.textSecondary
                                        wrapMode: Text.Wrap
                                    }
                                }

                                // Call a card: which one?
                                Flow {
                                    visible: rig.canTurn && rig.pickMode === "call"
                                    Layout.fillWidth: true
                                    spacing: 6
                                    Txt {
                                        width: Math.min(implicitWidth, parent.width)
                                        height: 28
                                        verticalAlignment: Text.AlignVCenter
                                        text: rig.nw("call_hint", "Which card is live? Right: its owner's stake is yours. Wrong: 2 chips.")
                                        px: 12
                                        color: Th.textMuted
                                        elide: Text.ElideRight
                                    }
                                    Repeater {
                                        model: rig.canTurn && rig.pickMode === "call" ? rig.openBoard : []
                                        PickChip {
                                            required property var modelData
                                            label: rig.cardName(modelData.card)
                                            onActivated: rig.callCard(modelData.card)
                                        }
                                    }
                                }
                                // Audit: which spin?
                                Flow {
                                    visible: rig.canTurn && rig.pickMode === "audit"
                                    Layout.fillWidth: true
                                    spacing: 6
                                    Repeater {
                                        model: rig.canTurn && rig.pickMode === "audit" ? rig.history : []
                                        PickChip {
                                            required property var modelData
                                            label: rig.fill(rig.nw("spin_label", "Spin {n}"), { n: modelData.spin })
                                            onActivated: rig.auditSpin(modelData.spin)
                                        }
                                    }
                                }
                                // the Audit result (the Spinner, until the reveal)
                                Rectangle {
                                    id: auditBox
                                    readonly property var audit: rig.rd && rig.rd.audit ? rig.rd.audit : null
                                    visible: !!auditBox.audit
                                    Layout.fillWidth: true
                                    implicitHeight: auditFlow.implicitHeight + 16
                                    radius: 8
                                    color: rig.art.shadeSoft || Th.surfaceSunken
                                    Flow {
                                        id: auditFlow
                                        anchors.left: parent.left
                                        anchors.right: parent.right
                                        anchors.verticalCenter: parent.verticalCenter
                                        anchors.leftMargin: 10
                                        anchors.rightMargin: 10
                                        spacing: 10
                                        Txt {
                                            height: 22
                                            verticalAlignment: Text.AlignVCenter
                                            text: auditBox.audit ? rig.fill(rig.nw("audit_of", "Audit of spin {n}"), { n: auditBox.audit.spin }) : ""
                                            font.bold: true
                                        }
                                        Row {
                                            spacing: 3
                                            Accessible.role: Accessible.StaticText
                                            Accessible.name: auditBox.audit ? rig.symNames(auditBox.audit.honest_reels) : ""
                                            Txt {
                                                height: 22
                                                verticalAlignment: Text.AlignVCenter
                                                text: rig.nw("honest", "honest:")
                                                color: Th.textSecondary
                                            }
                                            Repeater {
                                                model: auditBox.audit && auditBox.audit.honest_reels ? auditBox.audit.honest_reels : []
                                                RigSymbol {
                                                    required property var modelData
                                                    anchors.verticalCenter: parent.verticalCenter
                                                    sym: String(modelData)
                                                    size: 20
                                                    art: rig.art
                                                }
                                            }
                                            Txt {
                                                height: 22
                                                verticalAlignment: Text.AlignVCenter
                                                text: auditBox.audit ? rig.fill(rig.nw("pays", "pays {n}"), { n: auditBox.audit.honest_payout }) : ""
                                                color: Th.textSecondary
                                            }
                                        }
                                        Txt {
                                            height: 22
                                            verticalAlignment: Text.AlignVCenter
                                            text: {
                                                var n = auditBox.audit ? Number(auditBox.audit.touched || 0) : 0
                                                if (n === 0) return rig.nw("touched_none", "No card touched it")
                                                return n === 1 ? rig.rw("touched_one") : rig.fill(rig.rw("touched_n"), { n: n })
                                            }
                                            px: 12
                                            color: Th.textMuted
                                        }
                                    }
                                }
                            }
                        }

                        // ── the history: one slot per spin of the round ──
                        GridLayout {
                            id: histGrid
                            visible: rig.inRound && !rig.revealShown
                            Layout.fillWidth: true
                            readonly property int slots: rig.rd ? Math.max(0, Number(rig.rd.spins || 0)) : 0
                            readonly property int cols: Math.max(1, Math.min(Math.max(1, histGrid.slots), Math.floor((width + 6) / 68)))
                            readonly property real tileW: Math.floor((width - 6 * (histGrid.cols - 1)) / histGrid.cols)
                            columns: histGrid.cols
                            columnSpacing: 6
                            rowSpacing: 6
                            Repeater {
                                model: histGrid.visible ? histGrid.slots : 0
                                Rectangle {
                                    id: ht
                                    required property int index
                                    readonly property var h: rig.histAt(ht.index + 1)
                                    Layout.preferredWidth: histGrid.tileW
                                    Layout.fillWidth: true   // a layout whose children cannot stretch never grows past their sum
                                    Layout.fillHeight: true
                                    Layout.minimumHeight: 58
                                    implicitHeight: htCol.implicitHeight + 12
                                    radius: 8
                                    color: Th.surfaceRaised
                                    border.width: 1
                                    border.color: ht.h && ht.h.result === "win" ? rig.winC : Th.borderSubtle
                                    opacity: ht.h ? 1 : 0.4
                                    Accessible.role: Accessible.StaticText
                                    Accessible.name: ht.h ? rig.fill(rig.nw("spin_label", "Spin {n}"), { n: ht.h.spin }) + ": " + rig.rw("dial") + " " + ht.h.bet + ", "
                                                            + rig.symNames(ht.h.reels) + ", " + ht.h.payout
                                                            + ((ht.h.stamps || []).length ? ", " + ht.h.stamps.map(rig.stampWord).join(", ") : "")
                                                          : rig.fill(rig.nw("spin_label", "Spin {n}"), { n: ht.index + 1 })
                                    Column {
                                        id: htCol
                                        anchors.left: parent.left
                                        anchors.right: parent.right
                                        anchors.top: parent.top
                                        anchors.topMargin: 6
                                        anchors.leftMargin: 4
                                        anchors.rightMargin: 4
                                        spacing: 2
                                        Txt {
                                            width: parent.width
                                            text: String(ht.index + 1)
                                            px: 10
                                            color: Th.textMuted
                                            horizontalAlignment: Text.AlignHCenter
                                        }
                                        Row {
                                            visible: !!ht.h
                                            anchors.horizontalCenter: parent.horizontalCenter
                                            spacing: 1
                                            Repeater {
                                                model: ht.h && ht.h.reels ? ht.h.reels : []
                                                RigSymbol {
                                                    required property var modelData
                                                    sym: String(modelData)
                                                    size: 16
                                                    art: rig.art
                                                }
                                            }
                                        }
                                        Txt {
                                            visible: !!ht.h
                                            width: parent.width
                                            text: ht.h ? ht.h.bet + "→" + ht.h.payout : ""
                                            px: 11
                                            font.bold: true
                                            horizontalAlignment: Text.AlignHCenter
                                        }
                                        Txt {
                                            visible: !!ht.h && (ht.h.stamps || []).length > 0
                                            width: parent.width
                                            text: ht.h ? (ht.h.stamps || []).map(rig.stampWord).join(" · ") : ""
                                            px: 9.5
                                            color: Th.accentBright
                                            horizontalAlignment: Text.AlignHCenter
                                            wrapMode: Text.Wrap
                                        }
                                    }
                                }
                            }
                        }

                        // ── the REVEAL ──
                        Rectangle {
                            id: revealCard
                            visible: rig.revealShown
                            Layout.fillWidth: true
                            implicitHeight: revCol.implicitHeight + 24
                            radius: Th.shapeRadius
                            color: Th.surfaceRaised
                            border.width: 1
                            border.color: rig.trimC
                            readonly property var rv: rig.reveal ? rig.reveal : ({})
                            // the lines under the slots: [{ t: text, italic }]
                            readonly property var lines: {
                                var rv = revealCard.rv, out = []
                                var spinW = function (n) { return rig.fill(rig.nw("spin_label", "Spin {n}"), { n: n }) }
                                var rightW = function (ok) { return ok ? rig.nw("right", "right") : rig.nw("wrong_word", "wrong") };
                                (rv.calls || []).forEach(function (c) {
                                    out.push({ t: rig.fill(rig.nw("rv_call", "{spin}: called {card}: {result}"),
                                                           { spin: spinW(c.spin), card: rig.cardName(c.card), result: rightW(c.right) }), italic: false })
                                });
                                (rv.mutes || []).forEach(function (m) {
                                    out.push({ t: rig.fill(rig.nw("rv_mute", "{spin}: {who} muted their card"), { spin: spinW(m.spin), who: rig.nameOf(m.owner) }), italic: false })
                                });
                                (rv.nudges || []).forEach(function (n) {
                                    out.push({ t: rig.fill(rig.nw("rv_nudge", "{spin}: {who} nudged reel {reel} {way}"),
                                                           { spin: spinW(n.spin), who: rig.nameOf(n.by), reel: n.reel,
                                                             way: n.step > 0 ? rig.nw("forward", "forward") : rig.nw("back", "back") }), italic: false })
                                });
                                (rv.peeks || []).forEach(function (p) {
                                    out.push({ t: rig.fill(rig.nw("rv_peek", "{who} peeked {target}: {card}, {result} ({delta})"),
                                                           { who: rig.nameOf(p.who), target: rig.nameOf(p.target), card: rig.cardName(p.card),
                                                             result: rightW(p.right), delta: rig.signed(p.delta) }), italic: false })
                                });
                                (rv.market || []).forEach(function (m) {
                                    out.push({ t: rig.nameOf(m.who) + ": " + rig.rw("market") + " " + rig.sideWord(m.side) + " " + m.stake + " → " + m.back, italic: false })
                                });
                                (rv.whispers || []).forEach(function (w) {
                                    out.push({ t: rig.nameOf(w.who) + ": “" + rig.whisperText(w) + "”", italic: true })
                                })
                                return out
                            }
                            ColumnLayout {
                                id: revCol
                                anchors.left: parent.left
                                anchors.right: parent.right
                                anchors.top: parent.top
                                anchors.margins: 12
                                spacing: 8
                                Head {
                                    main: rig.phaseText("reveal")
                                    tail: revealCard.rv.up ? rig.nw("ended_up", "the Spinner ended UP") : rig.nw("ended_not_up", "the Spinner ended not up")
                                }
                                // the slots, face up, with their owners
                                GridLayout {
                                    id: slotGrid
                                    Layout.fillWidth: true
                                    readonly property int cols: Math.max(1, Math.floor((width + 8) / 168))
                                    readonly property real tileW: Math.floor((width - 8 * (slotGrid.cols - 1)) / slotGrid.cols)
                                    columns: slotGrid.cols
                                    columnSpacing: 8
                                    rowSpacing: 8
                                    Repeater {
                                        model: revealCard.rv.slots || []
                                        ColumnLayout {
                                            id: slotItem
                                            required property var modelData
                                            Layout.preferredWidth: slotGrid.tileW
                                            Layout.fillWidth: true   // a layout whose children cannot stretch never grows past their sum
                                            Layout.alignment: Qt.AlignTop
                                            spacing: 4
                                            BoardCard {
                                                Layout.fillWidth: true
                                                cardId: String(slotItem.modelData.card)
                                                mine: slotItem.modelData.owner === rig.myId
                                            }
                                            Txt {
                                                Layout.fillWidth: true
                                                text: rig.fill(rig.nw("rigged_by", "rigged by {who}"), { who: rig.nameOf(slotItem.modelData.owner) })
                                                      + (slotItem.modelData.from_spin > 1
                                                         ? rig.fill(rig.nw("from_spin", " from spin {n}"), { n: slotItem.modelData.from_spin }) : "")
                                                px: 11.5
                                                color: Th.textSecondary
                                                wrapMode: Text.Wrap
                                            }
                                        }
                                    }
                                }
                                ColumnLayout {
                                    visible: revealCard.lines.length > 0
                                    Layout.fillWidth: true
                                    spacing: 2
                                    Repeater {
                                        model: revealCard.lines
                                        Txt {
                                            required property var modelData
                                            Layout.fillWidth: true
                                            text: String(modelData.t)
                                            px: 12
                                            font.italic: !!modelData.italic
                                            color: modelData.italic ? Th.textSecondary : Th.textMuted
                                            wrapMode: Text.Wrap
                                        }
                                    }
                                }
                                // the replay: honest → final, per spin
                                ColumnLayout {
                                    visible: (revealCard.rv.spins || []).length > 0
                                    Layout.fillWidth: true
                                    spacing: 2
                                    Repeater {
                                        model: revealCard.rv.spins || []
                                        Flow {
                                            id: replay
                                            required property var modelData
                                            Layout.fillWidth: true
                                            spacing: 4
                                            Accessible.role: Accessible.StaticText
                                            Accessible.name: rig.fill(rig.nw("spin_label", "Spin {n}"), { n: replay.modelData.spin }) + ", " + rig.rw("dial") + " " + replay.modelData.bet
                                                             + ", " + rig.symNames(replay.modelData.honest) + " " + replay.modelData.honest_payout
                                                             + " → " + rig.symNames(replay.modelData.final) + " " + replay.modelData.payout
                                            Txt {
                                                height: 18
                                                verticalAlignment: Text.AlignVCenter
                                                text: rig.fill(rig.nw("spin_label", "Spin {n}"), { n: replay.modelData.spin }) + " · " + rig.rw("dial") + " " + replay.modelData.bet + " ·"
                                                px: 12
                                                color: Th.textMuted
                                            }
                                            Repeater {
                                                model: replay.modelData.honest || []
                                                RigSymbol { required property var modelData; sym: String(modelData); size: 18; art: rig.art }
                                            }
                                            Txt { height: 18; verticalAlignment: Text.AlignVCenter; text: String(replay.modelData.honest_payout) + "  →"; px: 12; color: Th.textMuted }
                                            Repeater {
                                                model: replay.modelData.final || []
                                                RigSymbol { required property var modelData; sym: String(modelData); size: 18; art: rig.art }
                                            }
                                            Txt {
                                                height: 18
                                                verticalAlignment: Text.AlignVCenter
                                                text: String(replay.modelData.payout)
                                                      + ((replay.modelData.touched || []).length ? " · " + replay.modelData.touched.map(rig.cardName).join(", ") : "")
                                                px: 12
                                                color: Th.textMuted
                                            }
                                        }
                                    }
                                }
                                RowLayout {
                                    visible: !!revealCard.rv.seed
                                    Layout.fillWidth: true
                                    spacing: 6
                                    Txt { text: rig.rw("seed"); px: 10.5; color: Th.textMuted; Layout.alignment: Qt.AlignTop; font.family: Th.fontMono }
                                    TextEdit {
                                        Layout.fillWidth: true
                                        text: revealCard.rv.seed ? String(revealCard.rv.seed) : ""
                                        textFormat: TextEdit.PlainText
                                        readOnly: true
                                        selectByMouse: true
                                        wrapMode: TextEdit.WrapAnywhere
                                        color: Th.textMuted
                                        selectionColor: Th.accentBase
                                        selectedTextColor: Th.textOnAccent
                                        font.family: Th.fontMono
                                        font.pointSize: rig.fp(10.5)
                                        Accessible.role: Accessible.EditableText
                                        Accessible.name: rig.rw("seed")
                                    }
                                }
                            }
                        }

                        // ── the RULE BOARD ──
                        ColumnLayout {
                            visible: rig.inRound && rig.board.length > 0
                            Layout.fillWidth: true
                            spacing: 8
                            Head {
                                main: rig.rw("rule_board")
                                tail: rig.rd ? rig.fill(rig.nw("board_counts", "{live} live · {open} could be"), { live: Number(rig.rd.mechanics || 0), open: rig.openBoard.length })
                                               + (rig.rd.refilled && rig.rd.refilled.length
                                                  ? " · " + rig.fill(rig.rw("refilled"), { n: rig.rd.refilled.map(function (x) { return x.spin }).join(", ") }) : "")
                                             : ""
                            }
                            GridLayout {
                                id: boardGrid
                                Layout.fillWidth: true
                                readonly property int cols: Math.max(1, Math.floor((width + 8) / 158))
                                readonly property real tileW: Math.floor((width - 8 * (boardGrid.cols - 1)) / boardGrid.cols)
                                columns: boardGrid.cols
                                columnSpacing: 8
                                rowSpacing: 8
                                Repeater {
                                    model: rig.board
                                    BoardCard {
                                        required property var modelData
                                        Layout.preferredWidth: boardGrid.tileW
                                        Layout.fillWidth: true   // a layout whose children cannot stretch never grows past their sum
                                        Layout.fillHeight: true
                                        cardId: String(modelData.card)
                                        cardState: String(modelData.state || "open")
                                        bustSpin: Number(modelData.spin || 0)
                                        // my card is marked on the public board only while I'm not sharing my screen
                                        mine: !rig.hideSecret && rig.myCardId !== "" && modelData.card === rig.myCardId
                                    }
                                }
                            }
                            // the web's .seed: the first 16 hex + "…", the whole hash on hover (and to a screen reader);
                            // hidden during the reveal, where the reveal card shows the seed itself
                            Txt {
                                id: seedLine
                                readonly property string fullHash: rig.rd && rig.rd.seed_commit ? String(rig.rd.seed_commit) : ""
                                Layout.fillWidth: true
                                visible: seedLine.fullHash !== "" && rig.phase !== "reveal"
                                text: seedLine.fullHash !== "" ? rig.rw("seed_commit") + " " + seedLine.fullHash.slice(0, 16) + "…" : ""
                                px: 10.5
                                color: Th.textMuted
                                font.family: Th.fontMono
                                wrapMode: Text.WrapAnywhere
                                Accessible.role: Accessible.StaticText
                                Accessible.name: rig.rw("seed_commit") + " " + seedLine.fullHash
                                Controls.ToolTip.visible: seedHover.hovered && seedLine.fullHash !== ""
                                Controls.ToolTip.delay: 400
                                Controls.ToolTip.text: seedLine.fullHash
                                HoverHandler { id: seedHover }
                            }
                        }
                    }

                    // ══ the right column ══
                    ColumnLayout {
                        Layout.alignment: Qt.AlignTop
                        Layout.fillWidth: !rig.wide
                        Layout.preferredWidth: rig.wide ? 250 : -1
                        spacing: 12

                        // ── at the table ──
                        Panel {
                            Layout.fillWidth: true
                            Head {
                                main: rig.nw("at_table", "At the table")
                                tail: rig.view && rig.view.spectators > 0 ? rig.fill(rig.nw("n_watching", "{n} watching"), { n: rig.view.spectators }) : ""
                            }
                            Repeater {
                                model: rig.inRound ? rig.activePlayers : []
                                RowLayout {
                                    id: person
                                    required property var modelData
                                    readonly property bool hasNet: !!rig.netMap && rig.netMap[person.modelData.u] !== undefined
                                    readonly property int net: person.hasNet ? Number(rig.netMap[person.modelData.u]) : 0
                                    Layout.fillWidth: true
                                    Layout.minimumHeight: 28
                                    spacing: 8
                                    opacity: person.modelData.connected === false ? 0.5 : 1
                                    Face { who: person.modelData.u; size: 24 }
                                    Txt {
                                        Layout.fillWidth: true
                                        text: rig.nameOf(person.modelData.u)
                                        font.bold: true
                                        elide: Text.ElideRight
                                    }
                                    RolePill { visible: !!person.modelData.role; roleId: String(person.modelData.role || "") }
                                    Txt {
                                        visible: person.hasNet
                                        text: rig.signed(person.net)
                                        px: 12
                                        font.bold: true
                                        color: person.net > 0 ? rig.winC : (person.net < 0 ? rig.lossC : Th.textSecondary)
                                    }
                                }
                            }
                        }

                        // ── the House secret panel ──
                        Panel {
                            id: secret
                            visible: rig.isHouse && rig.phase !== "reveal"
                            Layout.fillWidth: true
                            border.color: Th.accentDim
                            readonly property var peek: rig.me.peek ? rig.me.peek : null
                            readonly property bool hasMarket: rig.myRole === "mechanic" || rig.myRole === "sharp"
                            readonly property bool marketOpen: secret.hasMarket && rig.phase === "setup" && rig.me.market !== undefined && !rig.me.market_done
                            readonly property var deal: rig.isMech && rig.phase === "setup" && rig.me.deal && !rig.me.kept ? rig.me.deal : []
                            readonly property bool canWhisper: !!rig.feats.whisper && rig.me.whisper_used === false && rig.phase === "turn"
                            readonly property bool canPeek: rig.myRole !== "railbird" && !!secret.peek && !secret.peek.placed && !!rig.rd
                                                            && !rig.rd.peeks_locked && rig.phase === "turn" && rig.peekTargets.length > 0
                            readonly property int mutesLeft: Number(rig.me.mutes_left || 0)
                            readonly property bool armed: !!rig.me.armed

                            Head {
                                main: rig.nw("your_secrets", "Your secrets")
                                tail: rig.hideSecret ? "" : rig.nw("only_you", "only you see this")
                            }
                            Muted { visible: rig.hideSecret; text: rig.rw("secret_hidden") }

                            // SETUP: keep one of the two dealt cards
                            Muted { visible: !rig.hideSecret && secret.deal.length > 0; text: rig.nw("keep_one", "Keep one card for your slot:") }
                            RowLayout {
                                visible: !rig.hideSecret && secret.deal.length > 0
                                Layout.fillWidth: true
                                spacing: 6
                                Repeater {
                                    model: rig.hideSecret ? [] : secret.deal
                                    BoardCard {
                                        required property var modelData
                                        Layout.fillWidth: true
                                        Layout.preferredWidth: 1
                                        Layout.fillHeight: true
                                        cardId: String(modelData)
                                        pickable: rig.phase === "setup"
                                        Accessible.description: rig.rw("keep")
                                        onActivated: rig.act({ do: "keep", card: String(modelData) })
                                    }
                                }
                            }

                            // my card · MUTE · Rake
                            BoardCard {
                                visible: !rig.hideSecret && rig.isMech && rig.myCardId !== ""
                                Layout.fillWidth: true
                                cardId: rig.myCardId
                                mine: true
                            }
                            RowLayout {
                                visible: !rig.hideSecret && rig.isMech && rig.myCardId !== ""
                                Layout.fillWidth: true
                                Layout.topMargin: 2
                                spacing: 8
                                Rectangle {   // MUTE (ARMED glows)
                                    id: muteBtn
                                    readonly property bool can: rig.phase === "turn" && (secret.armed || secret.mutesLeft > 0)
                                    visible: !!rig.feats.mutes && !rig.me.busted
                                    Layout.fillWidth: true
                                    implicitHeight: 40
                                    radius: 20
                                    color: rig.art.shadeSoft || Th.surfaceSunken
                                    border.width: 1.5
                                    border.color: secret.armed || muteBtn.activeFocus ? Th.accentBright : Th.borderStrong
                                    opacity: muteBtn.can ? 1 : 0.45
                                    activeFocusOnTab: muteBtn.can
                                    Accessible.role: Accessible.Button
                                    Accessible.checkable: true
                                    Accessible.checked: secret.armed
                                    Accessible.name: muteText.text + ", " + rig.fill(rig.rw("mutes_left"), { n: secret.mutesLeft })
                                    Accessible.onPressAction: if (muteBtn.can) rig.toggleMute()
                                    Keys.onReturnPressed: if (muteBtn.can) rig.toggleMute()
                                    Keys.onSpacePressed: if (muteBtn.can) rig.toggleMute()
                                    Rectangle {
                                        visible: secret.armed
                                        anchors.fill: parent
                                        anchors.margins: -4
                                        radius: muteBtn.radius + 4
                                        color: "transparent"
                                        border.width: 4
                                        border.color: Th.accentGlow
                                    }
                                    Txt {
                                        id: muteText
                                        anchors.left: parent.left
                                        anchors.right: parent.right
                                        anchors.verticalCenter: parent.verticalCenter
                                        anchors.leftMargin: 8
                                        anchors.rightMargin: 8
                                        horizontalAlignment: Text.AlignHCenter
                                        elide: Text.ElideRight
                                        text: secret.armed ? rig.rw("mute") + " " + rig.rw("armed")
                                              : (secret.mutesLeft > 0 ? rig.rw("mute") + " (" + secret.mutesLeft + ")" : rig.nw("no_mutes", "No mutes left"))
                                        font.weight: Font.ExtraBold
                                        font.letterSpacing: 0.6
                                        color: secret.armed ? Th.accentBright : Th.textPrimary
                                    }
                                    MouseArea {
                                        anchors.fill: parent
                                        enabled: muteBtn.can
                                        cursorShape: Qt.PointingHandCursor
                                        onClicked: rig.toggleMute()
                                    }
                                }
                                Item { visible: !muteBtn.visible; Layout.fillWidth: true }
                                RowLayout {
                                    spacing: 6
                                    Accessible.role: Accessible.StaticText
                                    Accessible.name: rig.fill(rig.rw("rake"), { n: Number(rig.me.rake || 0) })
                                    Txt { text: rig.nw("rake_label", "Rake"); color: Th.textSecondary }
                                    Coin {}
                                    Txt { text: String(Number(rig.me.rake || 0)); px: 12; font.bold: true }
                                }
                            }

                            // busted: what's left · PATCH (Full, before the next SPIN)
                            Muted {
                                visible: !rig.hideSecret && rig.isMech && !!rig.me.busted
                                text: rig.nw("busted_note", "Your card was called: you're busted for this round. You can still Peek, bet the Market, Nudge and Whisper.")
                            }
                            ThButton {
                                visible: !rig.hideSecret && rig.isMech && !!rig.me.busted && !!rig.me.patch && !!rig.feats.patch
                                Layout.fillWidth: true
                                app: rig.app
                                text: rig.me.patch ? rig.fill(rig.nw("patch_put_back", "Patch: put {card} back (3 chips)"), { card: rig.cardName(rig.me.patch.card) }) : ""
                                highlighted: true
                                enabled: !!rig.me.patch && !!rig.me.patch.ok && rig.phase === "turn"
                                font.pointSize: rig.fp(12)
                                Accessible.name: text
                                onClicked: rig.act({ do: "patch" })
                            }

                            // MARKET (setup)
                            Muted { visible: !rig.hideSecret && secret.marketOpen; text: rig.nw("market_ask", "Market: will the Spinner end UP?") }
                            Flow {
                                visible: !rig.hideSecret && secret.marketOpen
                                Layout.fillWidth: true
                                spacing: 6
                                Repeater {
                                    model: !rig.hideSecret && secret.marketOpen ? [["up", 1], ["up", 2], ["not-up", 1], ["not-up", 2]] : []
                                    PickChip {
                                        required property var modelData
                                        label: rig.sideWord(modelData[0]) + " · " + modelData[1]
                                        lit: !!rig.me.market && rig.me.market.side === modelData[0] && rig.me.market.stake === modelData[1]
                                        onActivated: rig.act({ do: "market", side: modelData[0], stake: modelData[1] })
                                    }
                                }
                                PickChip {
                                    label: rig.rw("pass")
                                    onActivated: rig.act({ do: "market", side: "pass" })
                                }
                            }
                            Muted {
                                visible: !rig.hideSecret && !!rig.me.market && (rig.phase !== "setup" || !!rig.me.market_done)
                                text: rig.me.market ? rig.fill(rig.nw("market_mine", "Market: {side} for {n}"), { side: rig.sideWord(rig.me.market.side), n: rig.me.market.stake }) : ""
                            }

                            // WHISPER (Full; once a round; a template or up to 60 characters)
                            // never promise anonymity when it isn't true (client-rules whisper_alone)
                            Txt {
                                objectName: "whisperAloneWarning"
                                Layout.fillWidth: true
                                visible: !rig.hideSecret && secret.canWhisper && !!rig.play && rig.play.whisperAlone(rig.play.viewJson, rig.myId)
                                text: rig.rw("whisper_alone")
                                px: 12
                                wrapMode: Text.Wrap
                                color: Th.statusErrorFg
                            }
                            Muted { visible: !rig.hideSecret && secret.canWhisper; text: rig.nw("whisper_label", "Whisper to the Spinner (once, no name on it)") }
                            Flow {
                                visible: !rig.hideSecret && secret.canWhisper
                                Layout.fillWidth: true
                                spacing: 6
                                Repeater {
                                    model: !rig.hideSecret && secret.canWhisper ? rig.templateKeys : []
                                    PickChip {
                                        required property var modelData
                                        label: modelData === "bluffing"
                                               ? rig.fill(rig.templateRaw("bluffing"), { person: rig.nw("someone_cap", "Someone") }) + "…"
                                               : rig.templateRaw(modelData)
                                        lit: modelData === "bluffing" && rig.bluffPick
                                        onActivated: {
                                            if (modelData === "bluffing") rig.bluffPick = !rig.bluffPick
                                            else rig.sendWhisper({ template: modelData })
                                        }
                                    }
                                }
                            }
                            Flow {
                                visible: !rig.hideSecret && secret.canWhisper && rig.bluffPick
                                Layout.fillWidth: true
                                spacing: 6
                                Repeater {
                                    model: !rig.hideSecret && secret.canWhisper && rig.bluffPick ? rig.otherPlayers : []
                                    PickChip {
                                        required property var modelData
                                        label: rig.templateText("bluffing", modelData.u)
                                        onActivated: rig.sendWhisper({ template: "bluffing", person: modelData.u })
                                    }
                                }
                            }
                            RowLayout {
                                visible: !rig.hideSecret && secret.canWhisper
                                Layout.fillWidth: true
                                spacing: 6
                                ThTextField {
                                    id: whisperField
                                    Layout.fillWidth: true
                                    implicitHeight: 30
                                    maximumLength: 60
                                    text: rig.whisperDraft
                                    placeholderText: rig.nw("whisper_placeholder", "Or say something (60 characters)")
                                    font.pointSize: rig.fp(13)
                                    Accessible.name: rig.nw("whisper_label", "Whisper to the Spinner (once, no name on it)")
                                    onTextEdited: rig.whisperDraft = whisperField.text
                                    onAccepted: rig.sendWhisperText()
                                }
                                ThButton {
                                    app: rig.app
                                    text: rig.nw("send", "Send")
                                    enabled: rig.whisperOk(rig.whisperDraft)
                                    implicitHeight: 30
                                    font.pointSize: rig.fp(12)
                                    Accessible.name: text
                                    onClicked: rig.sendWhisperText()
                                }
                            }

                            // PEEK (a live Mechanic + a card; locks after spin locks_after)
                            Muted {
                                visible: !rig.hideSecret && rig.myRole !== "railbird" && !!secret.peek && !secret.peek.placed
                                text: rig.rd && rig.rd.peeks_locked ? rig.nw("peeks_locked", "Peeks are locked")
                                      : (secret.peek ? rig.fill(rig.nw("peek_offer", "Peek (2 chips): guess a Mechanic's card · locks after spin {n}"), { n: secret.peek.locks_after }) : "")
                            }
                            Flow {
                                visible: !rig.hideSecret && secret.canPeek
                                Layout.fillWidth: true
                                spacing: 6
                                Repeater {
                                    model: !rig.hideSecret && secret.canPeek ? rig.peekTargets : []
                                    PickChip {
                                        required property var modelData
                                        label: rig.nameOf(modelData.u)
                                        lit: rig.peekTarget === modelData.u
                                        onActivated: rig.peekTarget = rig.peekTarget === modelData.u ? "" : modelData.u
                                    }
                                }
                            }
                            Flow {
                                visible: !rig.hideSecret && secret.canPeek && rig.peekTarget !== ""
                                Layout.fillWidth: true
                                spacing: 6
                                Repeater {
                                    model: !rig.hideSecret && secret.canPeek && rig.peekTarget !== "" ? rig.openBoard : []
                                    PickChip {
                                        required property var modelData
                                        label: rig.cardName(modelData.card)
                                        onActivated: rig.peekCard(modelData.card)
                                    }
                                }
                            }
                            Muted {
                                visible: !rig.hideSecret && !!secret.peek && !!secret.peek.placed
                                text: secret.peek && secret.peek.placed
                                      ? (secret.peek.card ? rig.fill(rig.nw("peek_placed_card", "Peek placed: {card}. It pays at the reveal."), { card: rig.cardName(secret.peek.card) })
                                                          : rig.nw("peek_placed", "Peek placed. It pays at the reveal.")) : ""
                            }

                            // NUDGE (Full): the arrows show on the reels in the 4-second window
                            Muted {
                                visible: !rig.hideSecret && !!rig.feats.nudge && Number(rig.me.nudge_left || 0) > 0
                                text: rig.nw("nudge_note", "You have a Nudge: when the reels land, tap ▲ or ▼ on a reel within 4 seconds.")
                            }
                        }

                        // ── the Whispers to the Spinner (no authors) ──
                        Panel {
                            visible: rig.isSpinner && rig.myWhispers.length > 0
                            Layout.fillWidth: true
                            Head { main: rig.nw("whispers_to_you", "Whispers to you") }
                            Repeater {
                                model: rig.isSpinner ? rig.myWhispers : []
                                Flow {
                                    id: wrow
                                    required property var modelData
                                    Layout.fillWidth: true
                                    spacing: 0
                                    Txt {
                                        width: Math.min(implicitWidth, wrow.width)
                                        text: "“" + rig.whisperText(wrow.modelData) + "”"
                                        font.italic: true
                                        color: Th.textSecondary
                                        wrapMode: Text.Wrap
                                    }
                                    Txt {
                                        visible: wrow.modelData.spin > 0
                                        text: " · " + rig.fill(rig.rw("on_spin"), { n: wrow.modelData.spin })
                                        px: 12
                                        font.italic: true
                                        color: Th.textMuted
                                    }
                                }
                            }
                        }

                        // ── the paytable ──
                        Panel {
                            Layout.fillWidth: true
                            Head { main: rig.nw("pays_head", "Pays (× bet)") }
                            Repeater {
                                model: rig.words && rig.words.rig && rig.words.rig.paytable ? rig.words.rig.paytable : []
                                RowLayout {
                                    required property var modelData
                                    Layout.fillWidth: true
                                    spacing: 6
                                    Txt {
                                        Layout.fillWidth: true
                                        text: String(modelData[0])
                                        px: 12
                                        color: Th.textSecondary
                                        elide: Text.ElideRight
                                    }
                                    Txt {
                                        text: String(modelData[1])
                                        px: 12
                                        font.bold: true
                                        color: rig.trimC
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
