// HoldemTable.qml — mini games v1: Texas Hold'em at one table (GAMES-V1-SPEC.md §4), drawn as Tin Hat Web's
// HoldemTable.tsx + Holdem.module.css (parity: the same pieces, sizes and words). Everything comes from the server's
// view (§4.3): seats, board, pots, whose turn, `you.legal` — the app never computes what is legal. Moves go through
// PlayController.act (the Rust side names the hand + turn of the view ON SCREEN).
//
//   the bar      "Hand n · blinds 1/2" (the game's name only when `titled`; the panel's header names it)  ·
//                Deal (host, lobby) · Sit out / I'm back · Leave table / Sit down · Close table (host, asks first)
//   the stage    the felt (ui.json art, the web's radial cloth), up to 8 seats round it (ui.json seat_slot: me at the
//                bottom centre; each seat centred on its slot, the end seats' plates beside the avatar, outside), each an
//                avatar with the turn ring + the dealer button, the name, chips and a tag; the board, the pot pill or the
//                result; bets in front of the seats
//   my area      my cards (face down + hold to peek while my Pad has the table or I share my screen), my name + chips,
//                and on my turn: the clock, Fold · Check · Call N · Raise to N / Bet N / All-in N, the slider + presets
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import QtQuick.Shapes
import org.kde.kirigami as Kirigami
import dev.tinhat.app

Item {
    id: ht
    property var app: null
    property var play: null
    property var view: null
    property var words: ({})
    property string myId: ""
    // the prestige context of every name here (PrestigeToken roomId): the table's bound space (view.space)
    property string roomId: ""
    property bool sharing: false
    property real clockOffset: 0
    property bool focusedWindow: true
    // the web's HoldemTable `titled`: false in the game panel (its header already names the game), so the bar
    // carries only "Hand n · blinds sb/bb" + the buttons
    property bool titled: false
    readonly property var art: ht.app && ht.app.playArt ? ht.app.playArt : ({})

    readonly property var w: ht.words && ht.words.table ? ht.words.table : ({})
    function fill(s, map) {
        s = String(s === undefined || s === null ? "" : s)
        for (var k in map) s = s.split("{" + k + "}").join(String(map[k]))
        return s
    }
    function say(key, map) { return ht.fill(ht.w[key] !== undefined ? ht.w[key] : key, map || ({})) }
    function nameOf(u) {
        if (!u) return ""
        if (ht.app && ht.app.localpart) return String(ht.app.localpart(u))
        var s = String(u); var i = s.indexOf(":"); return i > 1 ? s.substring(1, i) : s
    }
    function actionWord(a, to) {
        var A = ht.w.actions || ({})
        if (a === "call") return to ? ht.fill(A.call_n, { n: to }) : String(A.call || "")
        if (a === "raise") return to ? ht.fill(A.raise_n, { n: to }) : String(A.raise || "")
        if (a === "bet") return to ? ht.fill(A.bet_n, { n: to }) : String(A.bet || "")
        return A[a] !== undefined ? String(A[a]) : ""
    }

    readonly property var opts: ht.view && ht.view.opts ? ht.view.opts : ({})
    readonly property int nSeats: Math.max(2, Math.min(8, ht.opts.seats || 8))
    readonly property var seats: ht.view && ht.view.seats ? ht.view.seats : []
    readonly property var hand: ht.view && ht.view.hand ? ht.view.hand : null
    readonly property var result: ht.view && ht.view.result ? ht.view.result : null
    readonly property var you: ht.view && ht.view.you ? ht.view.you : null
    readonly property var legal: ht.you && ht.you.legal ? ht.you.legal : null
    readonly property var mine: {
        for (var i = 0; i < ht.seats.length; ++i) if (ht.seats[i].u === ht.myId) return ht.seats[i]
        if (ht.you && ht.you.seat) for (var j = 0; j < ht.seats.length; ++j) if (ht.seats[j].seat === ht.you.seat) return ht.seats[j]
        return null
    }
    readonly property int mySeat: ht.mine ? ht.mine.seat : 0
    readonly property bool isHost: !!ht.view && ht.view.host === ht.myId
    readonly property var toAct: ht.hand && ht.hand.to_act ? ht.hand.to_act : null
    readonly property bool myTurn: !!ht.legal && !!ht.toAct && ht.toAct.seat === ht.mySeat && ht.mySeat > 0
    function seatAt(n) {
        for (var i = 0; i < ht.seats.length; ++i) if (ht.seats[i].seat === n) return ht.seats[i]
        return null
    }
    // ui.json seat_slot + the web's seatLayout: the slot's centre in % of the felt box
    function seatPos(n) {
        var slot = ht.play ? ht.play.seatSlot(n, ht.mySeat, ht.nSeats) : n - 1
        var a = Math.PI / 2 + slot * 2 * Math.PI / ht.nSeats
        return Qt.point(Math.round((50 + 46 * Math.cos(a)) * 10) / 10, Math.round((50 + 44 * Math.sin(a)) * 10) / 10)
    }
    readonly property int potTotal: {
        var t = 0
        var p = ht.hand && ht.hand.pots ? ht.hand.pots : []
        for (var i = 0; i < p.length; ++i) t += p[i].amount || 0
        for (var j = 0; j < ht.seats.length; ++j) t += ht.seats[j].bet || 0
        return t
    }
    readonly property bool anyBet: { for (var i = 0; i < ht.seats.length; ++i) if ((ht.seats[i].bet || 0) > 0) return true; return false }
    // at the pause: who won what (result.pots[].paid), with their made hand
    readonly property var winners: {
        var r = ht.result
        if (!r || r.voided === true || !ht.view || ht.view.status !== "pause") return []
        var won = ({})
        var pots = r.pots || []
        for (var i = 0; i < pots.length; ++i) for (var k in (pots[i].paid || {})) won[k] = (won[k] || 0) + pots[i].paid[k]
        var out = []
        for (var s in won) out.push({ seat: parseInt(s), n: won[s], u: ht.seatAt(parseInt(s)) ? ht.seatAt(parseInt(s)).u : "", made: r.best && r.best[s] ? r.best[s].category : "" })
        return out
    }
    function wonBy(n) { for (var i = 0; i < ht.winners.length; ++i) if (ht.winners[i].seat === n) return ht.winners[i].n; return 0 }
    function shownOf(n) {
        var s = ht.seatAt(n)
        if (s && s.shown) return s.shown
        if (ht.result && ht.result.shown && ht.result.shown[String(n)]) return ht.result.shown[String(n)]
        return null
    }

    // the clock: server time, one tick a second
    property real nowMs: Date.now() + ht.clockOffset
    Timer { interval: 1000; repeat: true; running: !!ht.toAct && !!ht.toAct.deadline; onTriggered: ht.nowMs = Date.now() + ht.clockOffset }
    readonly property int secsLeft: ht.toAct && ht.toAct.deadline ? Math.max(0, Math.ceil((ht.toAct.deadline - ht.nowMs) / 1000)) : 0

    // raise presets (the web's raisePresets: ½ pot, Pot, All-in; a repeat or one ≥ max is dropped)
    readonly property var presets: {
        if (!ht.legal || !ht.legal.raise || !ht.play) return []
        var r = ht.legal.raise, out = []
        var add = function (id, label, to) {
            if (to < 0) return
            if (id !== "allin") { for (var i = 0; i < out.length; ++i) if (out[i].to === to) return; if (to >= r.max) return }
            out.push({ id: id, label: label, to: to })
        }
        add("half", ht.say("half_pot"), ht.play.raisePreset(ht.play.viewJson, 0.5))
        add("pot", ht.say("pot"), ht.play.raisePreset(ht.play.viewJson, 1))
        add("allin", ht.say("max"), r.max)
        return out
    }
    property int raiseTo: 0
    readonly property string turnKey: ht.hand && ht.hand.to_act ? ht.hand.n + ":" + ht.hand.to_act.turn : ""
    onLegalChanged: if (ht.legal && ht.legal.raise && (ht.raiseTo < ht.legal.raise.min || ht.raiseTo > ht.legal.raise.max)) ht.raiseTo = ht.legal.raise.min

    // my cards (round 2 B1/B2, GAMES-V1.1-ADDENDUM §1.4; the shared rule games_rules::cards_face):
    //   up · peek (a Pad has the table: face down, hold to peek, "Your cards are on your phone · Show here")
    //   · down (I share my screen: face down, NO peek, and nothing derived from my cards shows) · none
    // The rule re-runs at once on every input (the share stopping included): it's a binding, never latched.
    readonly property var screens: ht.you && ht.you.screens ? ht.you.screens : []
    readonly property bool hasCards: !!ht.you && !!ht.you.cards && ht.you.cards.length > 0
    readonly property bool padHas: ht.screens.indexOf("pad") >= 0
    // Show here: holds until a Pad attaches AGAIN (screens goes no-pad → pad) or the table closes (show_here_next)
    // kept on the app (Main.gameShowHere) so the compact table on the Call stage follows it too; a table change resets it
    readonly property bool showHere: !!ht.app && ht.app.gameShowHere === true
    function setShowHere(on) { if (ht.app) ht.app.gameShowHere = on }
    property var prevScreens: []
    onScreensChanged: {
        if (ht.play && ht.showHere) ht.setShowHere(ht.play.showHereNext(ht.showHere, JSON.stringify(ht.prevScreens), JSON.stringify(ht.screens), false))
        ht.prevScreens = ht.screens
    }
    readonly property string face: ht.play ? String(ht.play.cardsFace(ht.hasCards, ht.sharing, ht.showHere, JSON.stringify(ht.screens))) : "up"
    readonly property bool hideMine: ht.face === "down" || ht.face === "peek"
    property bool peeking: false
    onFaceChanged: ht.peeking = false
    // what I see of my own cards on this screen right now: "up" / "peeking" show them; everything derived from my cards
    // (the made hand, the F5 highlight, the odds) follows this
    readonly property string seen: ht.face === "up" ? "up" : (ht.face === "peek" && ht.peeking ? "peeking" : ht.face)
    readonly property bool mineVisible: ht.seen === "up" || ht.seen === "peeking"

    function act(o) { if (ht.play) ht.play.act(JSON.stringify(o)) }

    // B4 (addendum §1.1): a seated player who is in the call: their seat avatar IS their call presence (the call's
    // speaking ring, 2 px, and the muted-mic glyph at the bottom LEFT; the dealer button owns the bottom right)
    function callTileOf(u) {
        var ts = ht.app && ht.app.callTiles ? ht.app.callTiles : []
        for (var i = 0; i < ts.length; ++i) {
            var t = ts[i]
            if (t.kind === "screenshare" || t.screenshare === true) continue
            if ((t.id === "vid_selfview" && u === ht.myId) || String(t.participant || "") === u) return t
        }
        return null
    }

    // B10 (addendum §8): away seats. The server decides (one timeout → away; dealt out with no blinds; stand up after
    // AWAY_STAND_HANDS dealt-out hands or AWAY_STAND_MS, chips kept); the app only draws it: `seat.away` / `you.away`
    // = {reason, since, stand_at, hands_left} on 0.3.21 (12.2.0 servers: state "away" only).
    readonly property bool amAway: !!ht.mine && ht.mine.state === "away"
    readonly property var myAway: ht.you && ht.you.away ? ht.you.away : (ht.mine && ht.mine.away ? ht.mine.away : null)
    readonly property string standLine: {
        var a = ht.myAway
        if (!ht.amAway || !a || typeof a.hands_left !== "number") return ""
        var why = a.reason ? ht.r2say("away_reason_" + a.reason) : ""
        if (why.indexOf("away_reason_") === 0) why = ""
        var stand = a.hands_left === 1 ? ht.r2say("away_stand_one") : ht.r2say("away_stand_hands", { n: a.hands_left })
        return why !== "" ? why + " · " + stand : stand
    }
    function awayTag(st) {
        var base = ht.w.seat_states ? String(ht.w.seat_states.away || "") : ""
        return st.away && typeof st.away.hands_left === "number" ? ht.r2say("away_pill_hands", { n: st.away.hands_left }) : base
    }
    // standing up (the server freed my away seat): one toast with the chips that went back to my wallet
    signal toast(string text)
    // F7 (addendum §4): table reactions, no money. The tray sends play.react (the bucket's mirror greys it); arrivals
    // (play.reactJson) are drawn at the sender's seat plate for 1.2 s (a still badge for 2 s when the window isn't
    // focused or motion is reduced), announced politely (coalesced to one per 3 s), and dropped when muted (all, or that
    // person; local only, the sender isn't told).
    readonly property var reactIds: ["tip", "knock", "clap", "toss", "sweat", "foil"]
    readonly property var reactGlyph: ({ knock: "✊", clap: "👏", toss: "●", sweat: "💧", foil: "✦", tip: "" })
    property bool reactReady: true
    Timer { interval: 500; repeat: true; running: !!ht.play && ht.visible; onTriggered: ht.reactReady = ht.play.reactReady() }
    function sendReact(r, to) {
        if (!ht.play) return
        var op = String(ht.play.react(r, to === undefined ? "pot" : String(to)))
        ht.reactReady = ht.play.reactReady()
    }
    readonly property var settingsCtl: ht.app && ht.app.settings ? ht.app.settings : null
    property bool muteAll: !!ht.settingsCtl && String(ht.settingsCtl.uiPref("games-react-mute")) === "1"
    property var mutedUsers: { try { return JSON.parse(ht.settingsCtl ? String(ht.settingsCtl.uiBlob("games-react-muted") || "[]") : "[]") } catch (e) { return [] } }
    function setMuteAll(on) { ht.muteAll = on; if (ht.settingsCtl) ht.settingsCtl.applyUiPref("games-react-mute", on ? "1" : "0") }
    function toggleMuteUser(u) {
        var m = ht.mutedUsers.slice(), i = m.indexOf(u)
        if (i >= 0) m.splice(i, 1); else m.push(u)
        ht.mutedUsers = m
        if (ht.settingsCtl) ht.settingsCtl.applyUiBlob("games-react-muted", JSON.stringify(m))
    }
    property var bursts: []          // [{key, u, seat, r, to, still}]
    property int _burstN: 0
    property var _sayQueue: []
    Timer { id: sayTimer; interval: 3000; onTriggered: ht.flushSay() }
    function flushSay() {
        if (ht._sayQueue.length === 0) return
        var t = ht._sayQueue.length === 1 ? ht._sayQueue[0] : ht.r2say("react_n", { n: ht._sayQueue.length })
        ht._sayQueue = []
        if (ht.app && ht.app.gameAnnounce) ht.app.gameAnnounce(t)
    }
    function reactWords(x) {
        var who = ht.nameOf(x.u)
        var target = x.to === "pot" || x.to === undefined || x.to === null ? ht.r2say("react_pot") : ht.nameOf(ht.seatAt(Number(x.to)) ? ht.seatAt(Number(x.to)).u : "")
        var line = ht.w.react && ht.w.react[x.r] ? ht.fill(ht.w.react[x.r], { person: target }) : x.r
        return who + " " + line
    }
    Connections {
        target: ht.play
        ignoreUnknownSignals: true
        function onReactRevChanged() {
            var x; try { x = JSON.parse(ht.play.reactJson) } catch (e) { return }
            if (!x || ht.muteAll || ht.mutedUsers.indexOf(String(x.u)) >= 0) return
            var still = !Motion.focused || !Motion.animate || Motion.reducedMotion || !ht.focusedWindow
            ht._burstN += 1
            var b = ht.bursts.slice(); b.push({ key: ht._burstN, u: String(x.u), seat: x.seat, r: String(x.r), to: x.to, still: still })
            if (b.length > 12) b.shift()
            ht.bursts = b
            var q = ht._sayQueue.slice(); q.push(ht.reactWords(x)); ht._sayQueue = q
            if (!sayTimer.running) { ht.flushSay(); sayTimer.start() }
        }
    }
    function dropBurst(key) { ht.bursts = ht.bursts.filter(function (b) { return b.key !== key }) }

    // §8.3 "your turn" notices (opt-in: TURN_NOTIFY_DEFAULT false until answered, the line under the action bar the first
    // time I sit; Settings → Games / the ⋯ change it). Sent when my turn begins and the table isn't on screen and focused
    // (window unfocused, the stage on Call, another room); a second at TURN_NOTIFY_LAST_S left; withdrawn when the turn
    // passes. Never cards, odds or amounts. The OS's own Do Not Disturb applies (it's the OS notification path).
    readonly property int turnNotifyLastS: 10
    property int turnPrefRev: 0
    readonly property string turnPref: ht.turnPrefRev >= 0 && ht.settingsCtl ? String(ht.settingsCtl.uiPref("games-turn-notify")) : ""
    function setTurnPref(on) { if (ht.settingsCtl) ht.settingsCtl.applyUiPref("games-turn-notify", on ? "1" : "0"); ht.turnPrefRev += 1 }
    readonly property bool tableOnScreen: ht.focusedWindow && Motion.focused && !!ht.app && ht.app.gameStage === "table" && ht.visible
    property string _noticedTurn: ""
    property bool _lastNoticed: false
    readonly property string gameTitle: String(ht.words.games ? ht.words.games.holdem : "")
    function turnNotify(last) {
        if (!ht.play || ht.turnPref !== "1") return
        var room = ht.view && ht.view.room ? String(ht.view.room) : ""
        var roomName = ht.app && ht.app.mRoomName ? String(ht.app.mRoomName) : ""
        ht.play.turnNotice(ht.r2say("turn_title"), (last ? ht.r2say("turn_last", { s: ht.turnNotifyLastS }) + " · " : "") + ht.r2say("turn_body", { game: ht.gameTitle, room: roomName }), room)
    }
    onMyTurnChanged: {
        if (!ht.myTurn) { ht._noticedTurn = ""; ht._lastNoticed = false; if (ht.play) ht.play.turnNoticeClear(); return }
        if (ht.turnKey !== ht._noticedTurn) { ht._noticedTurn = ht.turnKey; ht._lastNoticed = false; if (!ht.tableOnScreen) ht.turnNotify(false) }
    }
    onTurnKeyChanged: {
        ht.raiseTo = ht.legal && ht.legal.raise ? ht.legal.raise.min : 0
        if (ht.myTurn && ht.turnKey !== ht._noticedTurn) { ht._noticedTurn = ht.turnKey; ht._lastNoticed = false; if (!ht.tableOnScreen) ht.turnNotify(false) }
    }
    onSecsLeftChanged: {
        if (ht.myTurn && !ht._lastNoticed && ht.secsLeft > 0 && ht.secsLeft <= ht.turnNotifyLastS && !ht.tableOnScreen) { ht._lastNoticed = true; ht.turnNotify(true) }
    }
    onTableOnScreenChanged: if (ht.tableOnScreen && ht.play) ht.play.turnNoticeClear()

    // F9: busted at the table ("Out of chips · Tune In"), offered only when the panel says the rule can hold
    property bool tuneOffered: false
    signal tuneRequested()
    property var lastMine: null
    onMineChanged: {
        if (!ht.mine && ht.lastMine && ht.lastMine.state === "away" && ht.view && ht.view.status !== "closed")
            ht.toast(ht.r2say("stood_up", { n: ht.lastMine.stack || 0 }))
        ht.lastMine = ht.mine ? JSON.parse(JSON.stringify(ht.mine)) : null
    }

    // F5: the hand-rankings popover; my current made hand (my cards + the board, the app's own evaluator) lights its row
    // while I can see my cards here
    function toggleRankings(btn, above) {
        if (rankPop.opened) { var same = rankPop.opener === btn; rankPop.close(); if (same) return }
        rankPop.opener = btn
        rankPop.above = above
        rankPop.open()
    }
    readonly property var r2: ht.w && ht.w.r2 ? ht.w.r2 : ({})
    function r2say(key, map) { return ht.fill(ht.r2[key] !== undefined ? ht.r2[key] : key, map || ({})) }
    readonly property var board: ht.hand && ht.hand.board ? ht.hand.board : []
    readonly property string myMade: {
        if (!ht.play || !ht.hasCards || !ht.mineVisible || !ht.mine || ht.mine.state === "folded") return ""
        return String(ht.play.madeHand(JSON.stringify(ht.you.cards.concat(ht.board))))
    }

    // F6: my odds (only my cards, the board and the live-opponent count go in: PlayController.requestOdds reads them
    // from the view with games_rules::odds_inputs). Drawn only while the answer's tag is this view's.
    property int oddsOnRev: 0
    readonly property bool oddsOn: ht.oddsOnRev >= 0 && (!ht.app || !ht.app.settings || String(ht.app.settings.uiPref("games-odds")) !== "0")
    readonly property string oddsTag: ht.play && ht.view ? String(ht.play.oddsTag(JSON.stringify(ht.view))) : ""
    readonly property bool oddsShown: ht.play && ht.view && ht.oddsTag !== "" ? ht.play.oddsShown(JSON.stringify(ht.view), ht.oddsOn, ht.seen) : false
    onOddsTagChanged: if (ht.oddsShown && ht.play) ht.play.requestOdds(JSON.stringify(ht.view))
    onOddsShownChanged: if (ht.oddsShown && ht.play) ht.play.requestOdds(JSON.stringify(ht.view))
    readonly property var odds: {
        if (!ht.oddsShown || !ht.play || !ht.play.oddsJson) return null
        var o = JSON.parse(ht.play.oddsJson)
        return o && o.tag === ht.oddsTag && !o.none ? o : null
    }
    readonly property string oddsLine: {
        var parts = []
        if (ht.myMade !== "" && ht.w.categories) parts.push(String(ht.w.categories[ht.myMade] || ""))
        if (ht.odds) {
            parts.push(ht.r2say("odds_win", { p: ht.odds.win_label }))
            if (ht.odds.tie_label !== "0%") parts.push(ht.r2say("odds_tie", { p: ht.odds.tie_label }))
            if (ht.odds.outs && ht.odds.outs.total > 0)
                parts.push(ht.odds.outs.total === 1 ? ht.r2say("odds_improve_one") : ht.r2say("odds_improve", { n: ht.odds.outs.total }))
        }
        return parts.join(" · ")
    }
    readonly property string outsTip: {
        if (!ht.odds || !ht.odds.outs || !ht.odds.outs.by) return ""
        var out = []
        for (var k in ht.odds.outs.by) out.push(String(ht.w.categories ? ht.w.categories[k] || k : k) + " " + ht.odds.outs.by[k])
        return out.join(" · ") + "\n" + ht.r2say("odds_info")
    }

    // the body size; every piece = fBody × the web's ratio to its 13 px body (Holdem.module.css, bits.module.css)
    readonly property real fBody: (Kirigami.Theme.defaultFont.pointSize > 0 ? Kirigami.Theme.defaultFont.pointSize : 10) * Th.textScale

    // ── a chip amount ("● 18"), the web's Chips ──
    component Chips: Row {
        id: chipsRow
        property int n: 0
        property bool big: false
        spacing: 4
        Rectangle {
            anchors.verticalCenter: parent.verticalCenter
            width: chipsRow.big ? 14 : 10; height: width; radius: width / 2
            color: ht.art.chip || Th.accentBase
            border.width: chipsRow.big ? 3 : 2
            border.color: ht.art.chipEdge || Th.accentBright
        }
        Controls.Label {
            anchors.verticalCenter: parent.verticalCenter
            text: String(chipsRow.n)
            textFormat: Text.PlainText
            color: Th.textPrimary
            font.weight: Font.DemiBold
            font.pointSize: ht.fBody * (chipsRow.big ? 1.15 : 0.92)   // the web's .chipsBig 15 px / .chips 12 px
        }
    }

    // ── the turn ring: an arc whose length is the time left ──
    component TurnRing: Shape {
        id: ring
        property real secs: 0
        property real total: 30
        property real sz: 50
        width: sz; height: sz
        preferredRendererType: Shape.CurveRenderer
        readonly property real frac: total > 0 ? Math.max(0, Math.min(1, secs / total)) : 0
        ShapePath {
            strokeColor: Th.borderSubtle; strokeWidth: 3; fillColor: "transparent"
            PathAngleArc { centerX: ring.sz / 2; centerY: ring.sz / 2; radiusX: ring.sz / 2 - 2; radiusY: ring.sz / 2 - 2; startAngle: 0; sweepAngle: 360 }
        }
        ShapePath {
            strokeColor: ring.secs <= 5 ? Th.statusErrorFg : Th.accentBase; strokeWidth: 3; fillColor: "transparent"; capStyle: ShapePath.RoundCap
            PathAngleArc { centerX: ring.sz / 2; centerY: ring.sz / 2; radiusX: ring.sz / 2 - 2; radiusY: ring.sz / 2 - 2; startAngle: -90; sweepAngle: 360 * ring.frac }
        }
    }

    // ── an action button (the web's .act: at least 88 wide, 36 tall, semibold, the label in the middle) ──
    component ActButton: ThButton {
        implicitWidth: Math.max(88, implicitContentWidth + 28)
        implicitHeight: 36
        leftPadding: Math.max(14, (width - implicitContentWidth) / 2)
        font.weight: Font.DemiBold
    }

    // F5: the rankings popover (360 px, anchored to the Hands button that opened it; Esc / a click outside closes it;
    // not modal: the table keeps running and the turn timer stays visible)
    Controls.Popup {
        id: rankPop
        objectName: "handRankingsPopup"
        property Item opener: null
        property bool above: true
        parent: rankPop.opener || ht
        width: 360
        height: Math.min(600, Math.max(260, (ht.height || 600) - 40))
        x: rankPop.opener ? rankPop.opener.width - width : 0
        y: rankPop.opener ? (rankPop.above ? -height - 6 : rankPop.opener.height + 6) : 0
        padding: 0
        modal: false
        focus: true
        closePolicy: Controls.Popup.CloseOnEscape | Controls.Popup.CloseOnPressOutsideParent
        background: Item {}
        contentItem: HandRankings {
            play: ht.play
            art: ht.art
            w: ht.w
            mine: ht.myMade
            fBody: ht.fBody
            // players only (design2 14:53): Show my odds + the "your turn" notices sit at the sheet's bottom
            toggles: !!ht.mine
            oddsOn: ht.oddsOn
            turnOn: ht.turnPref === "1"
            onOddsToggled: function (on) { if (ht.settingsCtl) ht.settingsCtl.applyUiPref("games-odds", on ? "1" : "0"); ht.oddsOnRev += 1 }
            onTurnToggled: function (on) { ht.setTurnPref(on) }
            onCloseRequested: rankPop.close()
        }
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        // ── the bar ──
        RowLayout {
            Layout.fillWidth: true
            Layout.leftMargin: 12
            Layout.rightMargin: 12
            Layout.topMargin: 6
            Layout.bottomMargin: 6
            spacing: 8
            Controls.Label {   // the web's `titled` (off in the game panel: its header names the game)
                visible: ht.titled
                text: String(ht.words.games ? ht.words.games.holdem : "")
                textFormat: Text.PlainText
                color: Th.textPrimary
                font.bold: true
                font.pointSize: ht.fBody * 1.15
            }
            Controls.Label {
                text: (ht.hand ? ht.say("hand_meta", { n: ht.hand.n }) : "") + ht.say("blinds", { sb: ht.opts.sb || 1, bb: ht.opts.bb || 2 })
                textFormat: Text.PlainText
                color: Th.textMuted
                font.pointSize: ht.fBody * 0.92
            }
            Item { Layout.fillWidth: true }
            Row {   // F7: the reactions tray (seated players + watchers in the call)
                objectName: "reactTray"
                spacing: 2
                Repeater {
                    model: ht.reactIds
                    Rectangle {
                        id: rb
                        required property var modelData
                        objectName: "react-" + rb.modelData
                        width: 30; height: 30; radius: 8
                        color: rbHover.hovered && ht.reactReady ? Th.surfaceHover : "transparent"
                        opacity: ht.reactReady ? 1 : 0.4
                        Image {
                            visible: rb.modelData === "tip"
                            anchors.centerIn: parent
                            width: 18; height: 18
                            sourceSize: Qt.size(18, 18)
                            source: rb.modelData === "tip" ? Prestige.tinted("prestige-symbolic", Th.textPrimary) : ""
                        }
                        Rectangle {   // toss: the art's coin
                            visible: rb.modelData === "toss"
                            anchors.centerIn: parent
                            width: 14; height: 14; radius: 7
                            color: ht.art.chip || Th.accentBase
                            border.width: 3; border.color: ht.art.chipEdge || Th.accentBright
                        }
                        Controls.Label {
                            visible: rb.modelData !== "tip" && rb.modelData !== "toss"
                            anchors.centerIn: parent
                            text: ht.reactGlyph[rb.modelData] || ""
                            textFormat: Text.PlainText
                            color: Th.textPrimary
                            font.pointSize: ht.fBody * 1.04
                        }
                        HoverHandler { id: rbHover; cursorShape: ht.reactReady ? Qt.PointingHandCursor : Qt.ArrowCursor }
                        TapHandler { enabled: ht.reactReady; onTapped: ht.sendReact(rb.modelData, "pot") }
                        // F-D1 (12.2.1): a Tab stop as on the web (Reactions.tsx buttons): Space / Enter react (not on
                        // auto-repeat; the cooldown still applies, the Tab stop stays put through it); the keyboard
                        // focus draws the web's ring (2 px accent, :focus-visible); a click never focuses it
                        activeFocusOnTab: true
                        Keys.onSpacePressed: (event) => { if (!event.isAutoRepeat && ht.reactReady) ht.sendReact(rb.modelData, "pot") }
                        Keys.onReturnPressed: (event) => { if (!event.isAutoRepeat && ht.reactReady) ht.sendReact(rb.modelData, "pot") }
                        Keys.onEnterPressed: (event) => { if (!event.isAutoRepeat && ht.reactReady) ht.sendReact(rb.modelData, "pot") }
                        Rectangle { anchors.fill: parent; anchors.margins: -2; radius: 10; color: "transparent"; border.width: 2; border.color: Th.accentBright; visible: rb.activeFocus }
                        Controls.ToolTip.visible: rbHover.hovered
                        Controls.ToolTip.delay: 400
                        Controls.ToolTip.text: ht.r2say("react_tip_" + rb.modelData)
                        Accessible.role: Accessible.Button
                        Accessible.name: ht.r2say("react_tip_" + rb.modelData)
                        Accessible.onPressAction: if (ht.reactReady) ht.sendReact(rb.modelData, "pot")
                    }
                }
                Rectangle {   // the tray's last item: Mute reactions (all, this table and later ones; local only)
                    id: rmBtn
                    objectName: "reactMuteAll"
                    // F-D1: a Tab stop (the web's Mute reactions check box): Space / Enter toggle it, the focus ring as above
                    activeFocusOnTab: true
                    Keys.onSpacePressed: (event) => { if (!event.isAutoRepeat) ht.setMuteAll(!ht.muteAll) }
                    Keys.onReturnPressed: (event) => { if (!event.isAutoRepeat) ht.setMuteAll(!ht.muteAll) }
                    Keys.onEnterPressed: (event) => { if (!event.isAutoRepeat) ht.setMuteAll(!ht.muteAll) }
                    Rectangle { anchors.fill: parent; anchors.margins: -2; radius: 10; color: "transparent"; border.width: 2; border.color: Th.accentBright; visible: rmBtn.activeFocus }
                    width: 30; height: 30; radius: 8
                    color: rmHover.hovered ? Th.surfaceHover : (ht.muteAll ? Th.surfaceSelected : "transparent")
                    Controls.Label { anchors.centerIn: parent; text: ht.muteAll ? "⊘" : "◌"; color: ht.muteAll ? Th.accentBright : Th.textSecondary; font.pointSize: ht.fBody * 1.04 }
                    HoverHandler { id: rmHover; cursorShape: Qt.PointingHandCursor }
                    TapHandler { onTapped: ht.setMuteAll(!ht.muteAll) }
                    Controls.ToolTip.visible: rmHover.hovered
                    Controls.ToolTip.delay: 400
                    Controls.ToolTip.text: ht.muteAll ? ht.r2say("react_unmute_all") : ht.r2say("react_mute_all")
                    Accessible.role: Accessible.CheckBox
                    Accessible.checked: ht.muteAll
                    Accessible.name: ht.r2say("react_mute_all")
                    Accessible.onPressAction: ht.setMuteAll(!ht.muteAll)
                }
            }
            ThButton {   // F5 for spectators (seated players have it at the end of the my area)
                id: hdBarBtn
                objectName: "holdemHandsBar"
                app: ht.app
                visible: !ht.mine
                flat: !(rankPop.opened && rankPop.opener === hdBarBtn)
                highlighted: rankPop.opened && rankPop.opener === hdBarBtn
                text: "ⓘ  " + ht.r2say("hands_button")
                Accessible.name: ht.r2say("hands_title")
                onClicked: ht.toggleRankings(hdBarBtn, false)
            }
            ThButton {
                app: ht.app
                highlighted: true
                // the server's `you.can_deal` (contract 626d4e75) when it sends one, else the same count the web makes
                visible: ht.isHost && !!ht.view && ht.view.status === "lobby"
                         && (ht.view.you && ht.view.you.can_deal !== undefined ? ht.view.you.can_deal === true
                             : (function () { var n = 0; for (var i = 0; i < ht.seats.length; ++i) if (ht.seats[i].stack > 0 && ht.seats[i].state !== "away") n++; return n >= 2 })())
                text: ht.say("deal")
                Accessible.name: text
                onClicked: ht.act({ do: "deal" })
            }
            ThButton {
                app: ht.app
                flat: true
                visible: !!ht.mine && ht.mine.state !== "away"
                text: ht.say("sit_out")
                Accessible.name: text
                onClicked: ht.act({ do: "away", on: true })
            }
            ThButton {
                app: ht.app
                flat: true
                visible: !!ht.mine
                enabled: !!ht.mine && ht.mine.state !== "leaving"
                text: ht.mine && ht.mine.state === "leaving" ? ht.say("leaving_after") : ht.say("leave_table")
                Accessible.name: text
                onClicked: ht.act({ do: "leave" })
            }
            ThButton {
                app: ht.app
                highlighted: true
                visible: !ht.mine && ht.seats.length < ht.nSeats
                text: ht.say("sit")
                Accessible.name: text
                onClicked: ht.act({ do: "sit" })
            }
            ThButton {
                app: ht.app
                flat: true
                visible: ht.isHost
                text: ht.say("close")
                Accessible.name: text
                onClicked: closeAsk.open()
            }
        }
        Rectangle { Layout.fillWidth: true; implicitHeight: 1; color: Th.borderSubtle }

        // ── the stage ──
        Item {
            id: stage
            Layout.fillWidth: true
            Layout.fillHeight: true
            // the felt box: 2 : 1.08, at most 940 wide (the web's feltWrap: min(100%, 940px, 100cqh × 2 / 1.08)), inside
            // the web's 18 / 104 / 30 padding (the side seats' plates sit out in the 104 px)
            readonly property real availW: Math.max(200, stage.width - 208)
            readonly property real availH: Math.max(120, stage.height - 48)
            readonly property real fw: Math.min(940, stage.availW, stage.availH * 2 / 1.08)
            readonly property real fh: stage.fw * 1.08 / 2
            Item {
                id: feltWrap
                width: stage.fw
                height: stage.fh
                x: (stage.width - width) / 2
                y: 18 + (stage.height - 48 - height) / 2
                // the rim (10 px + a 1 px light edge), then the felt (inset 9 % / 7 %) with the line 14 px in
                Rectangle {
                    x: feltWrap.width * 0.07 - 11; y: feltWrap.height * 0.09 - 11
                    width: feltWrap.width * 0.86 + 22; height: feltWrap.height * 0.82 + 22
                    radius: height / 2
                    color: ht.art.feltRimLight || Th.borderStrong
                    Rectangle {
                        anchors.fill: parent; anchors.margins: 1
                        radius: height / 2
                        color: ht.art.feltRim || Th.surfaceRaised
                    }
                }
                Item {
                    id: felt
                    x: feltWrap.width * 0.07; y: feltWrap.height * 0.09
                    width: feltWrap.width * 0.86; height: feltWrap.height * 0.82
                    // the web's radial-gradient(ellipse at 50% 42%, centre 0%, edge 78%): an ellipse sized to the
                    // farthest corner (rx = √2 × 0.5 w, ry = √2 × 0.58 h) = a circle of radius ry stretched by rx / ry
                    Shape {
                        id: feltCloth
                        anchors.fill: parent
                        preferredRendererType: Shape.CurveRenderer
                        readonly property real gx: felt.width * 0.5
                        readonly property real gy: felt.height * 0.42
                        readonly property real gry: Math.max(1, felt.height * 0.58 * Math.SQRT2)
                        readonly property real grx: Math.max(1, felt.width * 0.5 * Math.SQRT2)
                        ShapePath {
                            strokeWidth: -1
                            strokeColor: "transparent"
                            fillGradient: RadialGradient {
                                centerX: feltCloth.gx; centerY: feltCloth.gy
                                focalX: feltCloth.gx; focalY: feltCloth.gy
                                centerRadius: feltCloth.gry
                                GradientStop { position: 0.0; color: ht.art.feltCentre || Th.surfaceSunken }
                                GradientStop { position: 0.78; color: ht.art.feltEdge || Th.surfaceSunken }
                            }
                            fillTransform: PlanarTransform.fromScale(feltCloth.grx / feltCloth.gry, 1, feltCloth.gx, feltCloth.gy)
                            PathRectangle { x: 0; y: 0; width: felt.width; height: felt.height; radius: felt.height / 2 }
                        }
                    }
                    Rectangle {
                        anchors.fill: parent; anchors.margins: 14
                        radius: height / 2
                        color: "transparent"
                        border.width: 1.5
                        border.color: ht.art.feltLine || Th.borderSubtle
                    }
                    // the middle: the board, then the pot / the result / the lobby line
                    Column {
                        anchors.centerIn: parent
                        spacing: 10
                        Row {
                            anchors.horizontalCenter: parent.horizontalCenter
                            spacing: 6
                            Repeater {
                                model: 5
                                PlayingCard {
                                    required property int index
                                    art: ht.art
                                    play: ht.play
                                    size: felt.width < 420 ? "md" : "lg"
                                    card: ht.hand && ht.hand.board && index < ht.hand.board.length ? ht.hand.board[index] : ""
                                    ghost: !(ht.hand && ht.hand.board && index < ht.hand.board.length)
                                }
                            }
                        }
                        Rectangle {   // the pot pill
                            anchors.horizontalCenter: parent.horizontalCenter
                            visible: potRow.visibleKids
                            radius: height / 2
                            color: ht.art.shadeSoft || Th.widgetChipBg
                            implicitWidth: potRow.implicitWidth + 20
                            implicitHeight: Math.max(22, potRow.implicitHeight + 6)
                            Row {
                                id: potRow
                                readonly property bool visibleKids: ht.potTotal > 0 || resultLine.text !== ""
                                anchors.centerIn: parent
                                spacing: 8
                                Controls.Label {
                                    visible: ht.potTotal > 0
                                    anchors.verticalCenter: parent.verticalCenter
                                    text: ht.say("pot_word").toUpperCase()
                                    textFormat: Text.PlainText
                                    color: Th.textSecondary
                                    font.bold: true
                                    font.letterSpacing: 1
                                    font.pointSize: ht.fBody * 0.85
                                }
                                Chips { visible: ht.potTotal > 0; anchors.verticalCenter: parent.verticalCenter; n: ht.potTotal; big: true }
                                Controls.Label {
                                    visible: ht.potTotal > 0 && !!ht.hand && !!ht.hand.pots && ht.hand.pots.length > 1
                                    anchors.verticalCenter: parent.verticalCenter
                                    text: {
                                        if (!ht.hand || !ht.hand.pots) return ""
                                        var parts = []
                                        for (var i = 0; i < ht.hand.pots.length; ++i) parts.push((i ? ht.say("side") : ht.say("main")) + " " + ht.hand.pots[i].amount)
                                        return parts.join(" · ")
                                    }
                                    textFormat: Text.PlainText
                                    color: Th.textMuted
                                    font.pointSize: ht.fBody * 0.85
                                }
                                Controls.Label {   // the result / the restart / the lobby line
                                    id: resultLine
                                    visible: ht.potTotal === 0 && text !== ""
                                    anchors.verticalCenter: parent.verticalCenter
                                    objectName: "holdemResult"
                                    text: {
                                        if (ht.result && ht.result.voided === true) return String(ht.w.closed_reasons ? ht.w.closed_reasons.restart : "")
                                        if (ht.winners.length > 0) {
                                            var parts = []
                                            // the web's winLine: "You win 60 with a straight" / "dee wins 60 with a straight";
                                            // no made hand: "You win 60" / "dee won 60" (won_n)
                                            var wonWith = String(ht.w.won_with || "")
                                            var inSentence = wonWith.indexOf(" with ") >= 0
                                            for (var i = 0; i < ht.winners.length; ++i) {
                                                var x = ht.winners[i]
                                                var cat = String(x.made || "")
                                                var what = !cat ? "" : String(inSentence ? ((ht.w.categories_in_sentence || {})[cat] || cat) : ((ht.w.categories || {})[cat] || cat))
                                                var me = (x.seat === ht.mySeat && ht.mySeat > 0) || (x.u !== "" && x.u === ht.myId)
                                                if (me) parts.push(what ? ht.say("you_win_with", { n: x.n, what: what }) : ht.say("you_win", { n: x.n }))
                                                else parts.push(what ? ht.say("won_with", { who: ht.nameOf(x.u), n: x.n, what: what })
                                                                     : ht.fill(ht.w.won_n !== undefined ? ht.w.won_n : "{who} won {n}", { who: ht.nameOf(x.u), n: x.n }))
                                            }
                                            return parts.join(" · ")
                                        }
                                        if (ht.view && ht.view.status === "lobby") return ht.isHost ? ht.say("deal_hint") : ht.say("wait_host")
                                        return ""
                                    }
                                    // the web's .result (the win / restart line: text colour, semibold, 13 px) or .wait (lobby)
                                    readonly property bool isResult: ht.winners.length > 0 || (!!ht.result && ht.result.voided === true)
                                    textFormat: Text.PlainText
                                    color: resultLine.isResult ? Th.textPrimary : Th.textSecondary
                                    font.weight: resultLine.isResult ? Font.DemiBold : Font.Normal
                                    font.pointSize: resultLine.isResult ? ht.fBody : ht.fBody * 0.92
                                }
                            }
                        }
                    }
                }

                // F7: reactions at the sender's plate (a watcher's at the felt's top), a toss's coin flies to its target
                Repeater {
                    model: ht.bursts
                    delegate: Item {
                        id: bu
                        required property var modelData
                        readonly property point from: bu.modelData.seat ? ht.seatPos(bu.modelData.seat) : Qt.point(50, 4)
                        readonly property point dest: bu.modelData.r !== "toss" ? bu.from
                            : (bu.modelData.to && bu.modelData.to !== "pot" ? ht.seatPos(Number(bu.modelData.to)) : Qt.point(50, 50))
                        z: 6
                        width: 64; height: 64
                        x: feltWrap.width * bu.from.x / 100 - 32
                        y: feltWrap.height * bu.from.y / 100 - 70
                        opacity: 1
                        objectName: "reactBurst"
                        Rectangle {   // the badge (glyph on a soft disc), anchored to the plate
                            id: badge
                            anchors.centerIn: parent
                            width: 40; height: 40; radius: 20
                            color: ht.art.shade || Th.surfaceOverlay
                            border.width: bu.modelData.r === "foil" ? 2 : 0
                            border.color: ht.art.machineTrim || Th.accentBright
                            scale: bu.modelData.still ? 1 : 0.4
                            Image {
                                visible: bu.modelData.r === "tip"
                                anchors.centerIn: parent; width: 24; height: 24; sourceSize: Qt.size(24, 24)
                                source: bu.modelData.r === "tip" ? Prestige.tinted("prestige-symbolic", Th.textPrimary) : ""
                            }
                            Rectangle {
                                visible: bu.modelData.r === "toss"
                                anchors.centerIn: parent; width: 16; height: 16; radius: 8
                                color: ht.art.chip || Th.accentBase; border.width: 3; border.color: ht.art.chipEdge || Th.accentBright
                            }
                            Controls.Label {
                                visible: bu.modelData.r !== "tip" && bu.modelData.r !== "toss"
                                anchors.centerIn: parent
                                text: ht.reactGlyph[bu.modelData.r] || ""
                                textFormat: Text.PlainText
                                color: Th.textPrimary
                                font.pointSize: ht.fBody * 1.3
                            }
                        }
                        Rectangle {   // a toss: the coin's flight (cosmetic: moves no chips)
                            id: coin
                            visible: bu.modelData.r === "toss" && !bu.modelData.still
                            width: 14; height: 14; radius: 7
                            color: ht.art.chip || Th.accentBase; border.width: 3; border.color: ht.art.chipEdge || Th.accentBright
                            x: 25; y: 25
                        }
                        SequentialAnimation {
                            running: !bu.modelData.still
                            ParallelAnimation {
                                NumberAnimation { target: badge; property: "scale"; from: 0.4; to: 1.0; duration: 220; easing.type: Easing.OutBack }
                                NumberAnimation {
                                    target: coin; property: "x"; from: 25; duration: 900; easing.type: Easing.InOutQuad
                                    to: 25 + feltWrap.width * (bu.dest.x - bu.from.x) / 100
                                }
                                NumberAnimation {
                                    target: coin; property: "y"; from: 25; duration: 900; easing.type: Easing.InOutQuad
                                    to: 25 + feltWrap.height * (bu.dest.y - bu.from.y) / 100 + 70
                                }
                            }
                            PauseAnimation { duration: 500 }
                            NumberAnimation { target: bu; property: "opacity"; to: 0; duration: 380 }
                            ScriptAction { script: ht.dropBurst(bu.modelData.key) }
                        }
                        Timer { running: bu.modelData.still; interval: 2000; onTriggered: ht.dropBurst(bu.modelData.key) }
                    }
                }

                // bets, in front of the seats (the web: p + (50 − p) × 0.42 / 0.5)
                Repeater {
                    model: ht.seats
                    delegate: Rectangle {
                        required property var modelData
                        readonly property point p: ht.seatPos(modelData.seat)
                        visible: (modelData.bet || 0) > 0
                        x: feltWrap.width * (p.x + (50 - p.x) * 0.42) / 100 - width / 2
                        y: feltWrap.height * (p.y + (50 - p.y) * 0.5) / 100 - height / 2
                        radius: height / 2
                        z: 3
                        color: ht.art.shade || Th.widgetChipBg   // the web's .bet: shade, 2 px / 7 px padding
                        implicitWidth: betChips.implicitWidth + 14
                        implicitHeight: betChips.implicitHeight + 4
                        Chips { id: betChips; anchors.centerIn: parent; n: modelData.bet || 0 }
                    }
                }

                // the seats
                Repeater {
                    model: ht.nSeats
                    delegate: Item {
                        id: seatItem
                        required property int index
                        readonly property int seatNo: index + 1
                        readonly property var s: ht.seatAt(seatItem.seatNo)
                        readonly property point p: ht.seatPos(seatItem.seatNo)
                        readonly property bool turn: !!ht.toAct && ht.toAct.seat === seatItem.seatNo
                        readonly property bool mineSeat: !!seatItem.s && seatItem.s.seat === ht.mySeat && ht.mySeat > 0
                        readonly property int wonN: ht.wonBy(seatItem.seatNo)
                        readonly property bool winner: seatItem.wonN > 0
                        readonly property var shown: ht.shownOf(seatItem.seatNo)
                        readonly property var cards: seatItem.shown ? seatItem.shown : (seatItem.s && seatItem.s.cards > 0 && !seatItem.mineSeat ? ["", ""] : [])
                        // the web's `side`: a seat at the oval's left / right end puts its name plate OUTSIDE, beside the
                        // avatar on the outer side (never on the rail)
                        readonly property string side: seatItem.p.x < 22 ? "l" : (seatItem.p.x > 78 ? "r" : "")
                        readonly property bool canSit: ht.mySeat === 0 && ht.seats.length < ht.nSeats
                        // the web's .seat column: the hole cards overlap the avatar by 10 px (6 px when shown), gap 3
                        readonly property real avTop: holes.visible ? holes.height + (seatItem.shown ? -6 : -10) + 3 : 0
                        width: seatItem.s ? 112 : emptySeat.width
                        height: seatItem.s ? (seatItem.side !== "" ? seatItem.avTop + 50 : seatItem.avTop + 50 + 3 + plate.height) : emptySeat.height
                        // the web's .seatPos: the whole seat (cards, avatar, plate) is centred on the slot's point
                        x: feltWrap.width * seatItem.p.x / 100 - width / 2
                        y: feltWrap.height * seatItem.p.y / 100 - height / 2
                        z: 2
                        opacity: seatItem.s && seatItem.s.state === "folded" ? 0.55 : (seatItem.s && seatItem.s.state === "away" ? 0.6 : 1)

                        // an empty chair (the web's .empty / .emptyQuiet): a dashed "Sit here" pill while I watch, else a
                        // faint dashed ring
                        Item {
                            id: emptySeat
                            visible: !seatItem.s
                            width: seatItem.canSit ? 84 : 40
                            height: seatItem.canSit ? 44 : 40
                            Shape {
                                anchors.fill: parent
                                preferredRendererType: Shape.CurveRenderer
                                ShapePath {
                                    strokeWidth: 1.5
                                    strokeStyle: ShapePath.DashLine
                                    dashPattern: [3, 2]
                                    strokeColor: seatItem.canSit ? (sitHover.hovered ? Th.accentBright : Th.borderStrong) : (ht.art.feltLine || Th.borderSubtle)
                                    fillColor: seatItem.canSit ? (sitHover.hovered ? Th.surfaceHover : (ht.art.shadeSoft || "transparent")) : "transparent"
                                    PathRectangle { x: 0.75; y: 0.75; width: emptySeat.width - 1.5; height: emptySeat.height - 1.5; radius: (emptySeat.height - 1.5) / 2 }
                                }
                            }
                            HoverHandler { id: sitHover; cursorShape: seatItem.canSit ? Qt.PointingHandCursor : Qt.ArrowCursor }
                            Controls.Label {
                                anchors.centerIn: parent
                                visible: seatItem.canSit
                                text: ht.say("sit_here")
                                textFormat: Text.PlainText
                                color: sitHover.hovered ? Th.textPrimary : Th.textSecondary
                                font.pointSize: ht.fBody * 0.92
                            }
                            TapHandler { enabled: seatItem.canSit; onTapped: ht.act({ do: "sit", seat: seatItem.seatNo }) }
                            Accessible.role: Accessible.Button
                            Accessible.name: ht.say("sit_here") + " " + seatItem.seatNo
                        }

                        Row {   // hole cards (backs, or what they showed, 1.15×), over the avatar's top edge
                            id: holes
                            visible: !!seatItem.s && seatItem.cards.length > 0
                            x: (seatItem.width - width) / 2
                            y: 0
                            z: 1
                            spacing: 2
                            Repeater {
                                model: seatItem.cards
                                PlayingCard {
                                    required property var modelData
                                    art: ht.art
                                    play: ht.play
                                    size: "sm"
                                    scale: seatItem.shown ? 1.15 : 1
                                    card: String(modelData)
                                    dim: !!seatItem.s && seatItem.s.state === "folded"
                                }
                            }
                        }
                        Item {   // the avatar + the turn ring + the dealer button (the web's avatarWrap, 50 × 50)
                            visible: !!seatItem.s
                            x: (seatItem.width - width) / 2
                            y: seatItem.avTop
                            width: 50; height: 50
                            Avatar {
                                anchors.centerIn: parent
                                app: ht.app
                                size: 44
                                name: seatItem.s ? ht.nameOf(seatItem.s.u) : ""
                                seed: seatItem.s ? String(seatItem.s.u) : ""
                            }
                            Item {   // B4: the call presence of this seat's person
                                id: pres
                                anchors.fill: parent
                                readonly property var tile: seatItem.s ? ht.callTileOf(String(seatItem.s.u)) : null
                                readonly property bool self: !!pres.tile && pres.tile.id === "vid_selfview"
                                readonly property bool muted: !!pres.tile && (pres.self ? (!!ht.app && (ht.app.mMicMuted || ht.app.mServerMuted)) : pres.tile.muted === true)
                                readonly property bool speaking: !!pres.tile && !pres.muted && Motion.focused
                                    && ht.app.tileAudioLevel(pres.tile.members || pres.tile.participant, pres.self, false) > 0.02
                                Rectangle {
                                    objectName: "seatSpeaking"
                                    anchors.centerIn: parent
                                    width: 48; height: 48; radius: 24
                                    color: "transparent"
                                    border.width: 2
                                    border.color: Th.tileRingSpeaking
                                    visible: pres.speaking && !seatItem.turn
                                }
                                Rectangle {   // muted: a 16 px disc with the mic-off glyph, bottom left
                                    objectName: "seatMuted"
                                    visible: pres.muted
                                    width: 16; height: 16; radius: 8
                                    x: -4; y: parent.height + 2 - height
                                    color: Th.surfaceOverlay
                                    border.width: 1; border.color: Th.borderSubtle
                                    Kirigami.Icon {   // the call tiles' mic-off glyph (the same source and token)
                                        anchors.centerIn: parent
                                        width: 11; height: 11
                                        source: "microphone-sensitivity-muted-symbolic"
                                        color: Th.tileMicOffIcon
                                    }
                                    Accessible.role: Accessible.StaticText
                                    Accessible.name: "muted"
                                }
                            }
                            TurnRing {
                                visible: seatItem.turn
                                anchors.centerIn: parent
                                sz: 50
                                secs: ht.secsLeft
                                total: ht.opts.turn_s || 30
                            }
                            Rectangle {   // the dealer button (the web: right −6, bottom −2)
                                visible: !!ht.hand && ht.hand.button === seatItem.seatNo
                                width: 18; height: 18; radius: 9
                                x: parent.width + 6 - width; y: parent.height + 2 - height
                                color: ht.art.dealer || Th.widgetDiceFaceBg
                                Controls.Label {
                                    anchors.centerIn: parent
                                    text: ht.say("dealer")
                                    textFormat: Text.PlainText
                                    color: ht.art.dealerFg || Th.widgetDiceFaceFg
                                    font.bold: true
                                    font.pointSize: ht.fBody * 0.77   // the web's .dealer (10 px)
                                }
                            }
                        }

                        // the plate's glow (the web: turn = accent ring + accent glow, winner = art.win ring + win glow)
                        Repeater {
                            model: seatItem.s && (seatItem.winner || seatItem.turn) ? [3, 7] : []
                            Rectangle {
                                required property var modelData
                                required property int index
                                x: plate.x - modelData; y: plate.y - modelData
                                width: plate.width + 2 * modelData; height: plate.height + 2 * modelData
                                radius: plate.radius + modelData
                                color: "transparent"
                                border.width: index === 0 ? 3 : 4
                                border.color: seatItem.winner ? (ht.art.win || Th.accentBright) : Th.accentGlow
                                opacity: (seatItem.winner ? 0.35 : 1) * (index === 0 ? 1 : 0.45)
                            }
                        }
                        Rectangle {   // the name plate (the web's .seatText): below the avatar, or beside it at the ends
                            id: plate
                            visible: !!seatItem.s
                            // F7: right-click (or long-press) = Toss a coin to them · Mute their reactions
                            TapHandler {
                                acceptedButtons: Qt.RightButton
                                onTapped: if (seatItem.s && seatItem.s.u !== ht.myId) plateMenu.popup()
                            }
                            TapHandler { onLongPressed: if (seatItem.s && seatItem.s.u !== ht.myId) plateMenu.popup() }
                            Controls.Menu {
                                id: plateMenu
                                Controls.MenuItem {
                                    text: LinkGate.plain(ht.r2say("react_toss_to", { person: seatItem.s ? ht.nameOf(seatItem.s.u) : "" }))
                                    enabled: ht.reactReady
                                    onTriggered: ht.sendReact("toss", seatItem.seatNo)
                                }
                                Controls.MenuItem {
                                    text: LinkGate.plain(seatItem.s && ht.mutedUsers.indexOf(String(seatItem.s.u)) >= 0
                                          ? ht.r2say("react_unmute_user", { person: ht.nameOf(seatItem.s.u) })
                                          : ht.r2say("react_mute_user", { person: seatItem.s ? ht.nameOf(seatItem.s.u) : "" }))
                                    onTriggered: if (seatItem.s) ht.toggleMuteUser(String(seatItem.s.u))
                                }
                            }
                            width: seatItem.side !== "" ? Math.max(84, Math.min(112, plateCol.implicitWidth + 16)) : Math.min(112, plateCol.implicitWidth + 16)
                            height: plateCol.implicitHeight + 7
                            x: seatItem.side === "l" ? seatItem.width / 2 - 30 - width : (seatItem.side === "r" ? seatItem.width / 2 + 30 : (seatItem.width - width) / 2)
                            y: seatItem.side !== "" ? seatItem.avTop + 50 - height : seatItem.avTop + 53
                            radius: 6
                            color: seatItem.mineSeat ? Th.surfaceSelected : Th.surfaceOverlay
                            border.width: seatItem.winner || seatItem.turn ? 2 : 1
                            border.color: seatItem.winner ? (ht.art.win || Th.accentBright) : (seatItem.turn ? Th.accentBright : Th.borderSubtle)
                            Column {
                                id: plateCol
                                x: seatItem.side === "l" ? plate.width - 8 - width : (seatItem.side === "r" ? 8 : (plate.width - width) / 2)
                                y: 3
                                spacing: 2
                                Row {   // B3: the prestige token 4 px before the name (12 px: a plate is widget-sized text)
                                    x: seatItem.side === "l" ? plateCol.width - width : (seatItem.side === "r" ? 0 : (plateCol.width - width) / 2)
                                    PrestigeToken {
                                        id: plateTok
                                        anchors.verticalCenter: parent.verticalCenter
                                        mxid: seatItem.s ? String(seatItem.s.u) : ""; roomId: ht.roomId; size: 12
                                    }
                                    Controls.Label {
                                        anchors.verticalCenter: parent.verticalCenter
                                        width: Math.min(96 - plateTok.implicitWidth, implicitWidth)
                                        text: seatItem.s ? ht.nameOf(seatItem.s.u) : ""
                                        textFormat: Text.PlainText
                                        elide: Text.ElideRight
                                        color: Th.textPrimary
                                        font.weight: Font.DemiBold
                                        font.pointSize: ht.fBody * 0.92
                                    }
                                }
                                Row {
                                    x: seatItem.side === "l" ? plateCol.width - width : (seatItem.side === "r" ? 0 : (plateCol.width - width) / 2)
                                    spacing: 6
                                    Chips { n: seatItem.s ? seatItem.s.stack : 0 }
                                    Controls.Label {
                                        readonly property string tag: {
                                            var st = seatItem.s
                                            if (!st) return ""
                                            if (seatItem.wonN > 0) return ht.say("won_tag", { n: seatItem.wonN })
                                            if (seatItem.turn) return ht.secsLeft + "s"
                                            if (st.connected === false) return ht.say("offline")
                                            if (st.state === "away") return ht.awayTag(st)
                                            var sw = ht.w.seat_states ? ht.w.seat_states[st.state] : ""
                                            if (sw) return sw
                                            if (ht.hand && ht.hand.last && ht.hand.last.seat === seatItem.seatNo) return ht.actionWord(ht.hand.last.a, ht.hand.last.to)
                                            return ""
                                        }
                                        anchors.verticalCenter: parent.verticalCenter
                                        visible: tag !== ""
                                        text: tag
                                        textFormat: Text.PlainText
                                        color: seatItem.wonN > 0 ? (ht.art.win || Th.accentBright) : (seatItem.turn ? Th.accentBright : Th.textSecondary)
                                        font.bold: seatItem.wonN > 0 || seatItem.turn
                                        font.pointSize: ht.fBody * 0.81
                                    }
                                }
                                Controls.Label {   // the made hand at the showdown
                                    visible: text !== ""
                                    x: seatItem.side === "l" ? plateCol.width - width : (seatItem.side === "r" ? 0 : (plateCol.width - width) / 2)
                                    text: {
                                        var r = ht.result
                                        var c = r && r.best && r.best[String(seatItem.seatNo)] ? r.best[String(seatItem.seatNo)].category : ""
                                        return c && ht.w.categories ? String(ht.w.categories[c] || "") : ""
                                    }
                                    textFormat: Text.PlainText
                                    color: Th.textSecondary
                                    font.pointSize: ht.fBody * 0.81
                                }
                            }
                        }
                    }
                }
            }
        }

        // ── my area / watching ──
        Rectangle { Layout.fillWidth: true; implicitHeight: 1; color: ht.myTurn ? Th.accentBase : Th.borderSubtle }
        Controls.Label {
            visible: !ht.mine
            Layout.fillWidth: true
            Layout.margins: 12
            horizontalAlignment: Text.AlignHCenter
            text: ht.seats.length < ht.nSeats ? ht.say("watching_sit") : ht.say("watching_full")
            textFormat: Text.PlainText
            color: Th.textSecondary
            font.pointSize: ht.fBody
        }
        RowLayout {
            visible: !!ht.mine
            Layout.fillWidth: true
            Layout.margins: 12
            spacing: 14
            Item {   // my cards (hold to peek while hidden)
                implicitWidth: myCards.implicitWidth
                implicitHeight: myCards.implicitHeight
                Row {
                    id: myCards
                    spacing: 6
                    Repeater {
                        model: ht.you && ht.you.cards ? ht.you.cards : []
                        PlayingCard {
                            required property var modelData
                            art: ht.art
                            play: ht.play
                            size: "lg"
                            card: ht.mineVisible ? String(modelData) : ""
                            dim: !!ht.mine && ht.mine.state === "folded"
                        }
                    }
                    ThButton {   // F9: out of chips → Tune In
                        objectName: "holdemTuneIn"
                        app: ht.app
                        visible: !!ht.mine && ht.mine.state === "out" && ht.tuneOffered
                        anchors.verticalCenter: parent.verticalCenter
                        highlighted: true
                        implicitHeight: 30
                        text: ht.r2say("tune_title")
                        Accessible.name: text
                        onClicked: ht.tuneRequested()
                    }
                    Controls.Label {
                        visible: !ht.you || !ht.you.cards || ht.you.cards.length === 0
                        anchors.verticalCenter: parent.verticalCenter
                        text: ht.mine && ht.mine.state === "out" ? ht.say("out") : (ht.mine && ht.mine.state === "away" ? ht.say("sitting_out") : String(ht.w.seat_states ? ht.w.seat_states.waiting : ""))
                        textFormat: Text.PlainText
                        color: Th.textMuted
                        font.pointSize: ht.fBody   // the web's .noCards (the body size)
                    }
                }
                MouseArea {   // hold to peek (press = faces, release = backs): only for a Pad's `peek`, never while sharing
                    objectName: "holdemPeek"
                    anchors.fill: parent
                    enabled: ht.face === "peek"
                    onPressed: ht.peeking = true
                    onReleased: ht.peeking = false
                    onCanceled: ht.peeking = false
                    Accessible.role: Accessible.Button
                    Accessible.name: ht.say("peek_pad")
                }
            }
            ColumnLayout {
                spacing: 2
                RowLayout {   // B3: my name with my prestige token (14 px beside the body-size name)
                    spacing: 0
                    PrestigeToken { mxid: ht.mine ? String(ht.mine.u) : ""; roomId: ht.roomId }
                    Controls.Label {
                        text: ht.mine ? ht.nameOf(ht.mine.u) : ""
                        textFormat: Text.PlainText
                        color: Th.textPrimary
                        font.weight: Font.DemiBold
                        font.pointSize: ht.fBody
                    }
                }
                Chips { n: ht.mine ? ht.mine.stack : 0; big: true }
                RowLayout {   // B2: why my cards are face down here (+ Show here for a Pad)
                    objectName: "holdemHiddenLine"
                    visible: ht.hideMine
                    spacing: 8
                    Controls.Label {
                        text: ht.face === "down" ? ht.r2say("hidden_sharing") : ht.r2say("cards_on_phone")
                        textFormat: Text.PlainText
                        color: Th.textMuted
                        font.pointSize: ht.fBody * 0.85
                    }
                    Controls.Label {   // Show here: a link-style accent button (web2's look), only for a Pad's `peek`
                        objectName: "holdemShowHere"
                        visible: ht.face === "peek"
                        text: ht.r2say("show_here")
                        textFormat: Text.PlainText
                        color: shHover.hovered ? Th.accentBright : Th.accentBase
                        font.weight: Font.DemiBold
                        font.underline: shHover.hovered
                        font.pointSize: ht.fBody * 0.85
                        HoverHandler { id: shHover; cursorShape: Qt.PointingHandCursor }
                        TapHandler { onTapped: ht.setShowHere(true) }
                        Accessible.role: Accessible.Button
                        Accessible.name: text
                        Accessible.onPressAction: ht.setShowHere(true)
                    }
                }
                RowLayout {   // F5/F6: my made hand · Win n% · Tie n% · n cards improve you ⓘ
                    objectName: "holdemOddsLine"
                    visible: ht.oddsLine !== ""
                    spacing: 4
                    Controls.Label {
                        text: ht.oddsLine
                        textFormat: Text.PlainText
                        color: Th.textSecondary
                        font.pointSize: ht.fBody * 0.85
                    }
                    Item {   // ⓘ: the improving cards by category + "Improving isn't always winning"
                        visible: ht.outsTip !== ""
                        implicitWidth: infoGlyph.implicitWidth + 4
                        implicitHeight: infoGlyph.implicitHeight
                        Controls.Label {
                            id: infoGlyph
                            anchors.centerIn: parent
                            text: "ⓘ"
                            textFormat: Text.PlainText
                            color: infoHover.hovered ? Th.textPrimary : Th.textMuted
                            font.pointSize: ht.fBody * 0.92
                        }
                        HoverHandler { id: infoHover }
                        Controls.ToolTip.visible: infoHover.hovered
                        Controls.ToolTip.delay: 300
                        Controls.ToolTip.text: ht.outsTip
                        Accessible.role: Accessible.StaticText
                        Accessible.name: ht.outsTip
                    }
                }
                Controls.Label {
                    visible: !!ht.you && ht.you.at_cap === true
                    text: ht.words.wallet ? String(ht.words.wallet.at_cap_table) : ""
                    textFormat: Text.PlainText
                    color: ht.art.chip || Th.accentBright
                    font.pointSize: ht.fBody * 0.85
                }
                Controls.Label {
                    visible: !!ht.you && (ht.you.timeouts || 0) > 0 && !!ht.mine && ht.mine.state !== "away" && ht.you.away === undefined
                    text: ht.say("timeout_note")
                    textFormat: Text.PlainText
                    color: Th.textMuted
                    font.pointSize: ht.fBody * 0.85
                }
            }
            ThButton {   // F5: Hands (the rankings popover above the button)
                id: hdBtn
                objectName: "holdemHands"
                app: ht.app
                Layout.alignment: Qt.AlignVCenter
                radius: height / 2
                flat: !(rankPop.opened && rankPop.opener === hdBtn)
                highlighted: rankPop.opened && rankPop.opener === hdBtn
                text: "ⓘ  " + ht.r2say("hands_button")
                Accessible.name: ht.r2say("hands_title")
                onClicked: ht.toggleRankings(hdBtn, true)
            }
            Item { Layout.fillWidth: true }
            ColumnLayout {   // B10 (addendum §8.2): away = a big I'm back in place of the action bar + when I'll stand up
                objectName: "holdemAwayBlock"
                visible: ht.amAway
                spacing: 4
                ThButton {
                    objectName: "holdemImBack"
                    app: ht.app
                    highlighted: true
                    Layout.alignment: Qt.AlignRight
                    implicitHeight: 40
                    leftPadding: 22
                    rightPadding: 22
                    font.weight: Font.DemiBold
                    text: ht.say("back")
                    Accessible.name: text
                    onClicked: ht.act({ do: "away", on: false })
                }
                Controls.Label {
                    visible: text !== ""
                    Layout.alignment: Qt.AlignRight
                    text: ht.standLine
                    textFormat: Text.PlainText
                    color: Th.textMuted
                    font.pointSize: ht.fBody * 0.85
                }
            }
            ColumnLayout {   // the actions
                visible: !ht.amAway
                spacing: 6
                RowLayout {   // §8.3: the one-time ask (until answered)
                    objectName: "turnNotifyAsk"
                    visible: !!ht.mine && ht.turnPref === ""
                    Layout.alignment: Qt.AlignRight
                    spacing: 6
                    Controls.Label { text: ht.r2say("turn_ask"); textFormat: Text.PlainText; color: Th.textSecondary; font.pointSize: ht.fBody * 0.81 }
                    ThButton { objectName: "turnNotifyYes"; app: ht.app; implicitHeight: 24; leftPadding: 8; rightPadding: 8; font.pointSize: ht.fBody * 0.81; text: ht.r2say("turn_yes"); onClicked: ht.setTurnPref(true) }
                    ThButton { app: ht.app; flat: true; implicitHeight: 24; leftPadding: 8; rightPadding: 8; font.pointSize: ht.fBody * 0.81; text: ht.r2say("turn_no"); onClicked: ht.setTurnPref(false) }
                }
                Controls.Label {
                    visible: !ht.myTurn
                    Layout.alignment: Qt.AlignRight
                    text: ht.toAct ? ht.say("wait_turn") : (ht.view && ht.view.status === "pause" ? ht.say("next_hand") : "")
                    textFormat: Text.PlainText
                    color: Th.textMuted
                    font.pointSize: ht.fBody
                }
                RowLayout {
                    visible: ht.myTurn
                    Layout.alignment: Qt.AlignRight
                    spacing: 8
                    Controls.Label {   // the web's .timer: 34 px min, right-aligned
                        Layout.minimumWidth: 34
                        Layout.rightMargin: 4
                        horizontalAlignment: Text.AlignRight
                        text: ht.secsLeft + "s"
                        textFormat: Text.PlainText
                        color: ht.secsLeft <= 5 ? (ht.art.loss || Th.statusErrorFg) : Th.accentBright
                        font.bold: true
                        font.pointSize: ht.fBody
                    }
                    ActButton {
                        app: ht.app
                        visible: !!ht.legal && ht.legal.fold === true
                        text: ht.say("fold")
                        Accessible.name: text
                        onClicked: ht.act({ do: "fold" })
                    }
                    ActButton {
                        app: ht.app
                        visible: !!ht.legal && ht.legal.check === true
                        text: ht.say("check")
                        Accessible.name: text
                        onClicked: ht.act({ do: "check" })
                    }
                    ActButton {
                        app: ht.app
                        highlighted: true
                        visible: !!ht.legal && typeof ht.legal.call === "number" && ht.legal.call > 0
                        text: ht.legal && typeof ht.legal.call === "number" ? ht.actionWord("call", ht.legal.call) : ""
                        Accessible.name: text
                        onClicked: ht.act({ do: "call" })
                    }
                    ActButton {
                        app: ht.app
                        readonly property var r: ht.legal && ht.legal.raise ? ht.legal.raise : null
                        readonly property bool allIn: !!r && ht.raiseTo >= r.max
                        visible: !!r
                        highlighted: !(ht.legal && typeof ht.legal.call === "number" && ht.legal.call > 0)
                        text: !r ? "" : (allIn ? ht.say("allin_n", { n: r.max }) : ht.actionWord(ht.anyBet ? "raise" : "bet", ht.raiseTo))
                        Accessible.name: text
                        onClicked: ht.act(allIn ? { do: "allin" } : { do: "raise", to: ht.raiseTo })
                    }
                }
                RowLayout {
                    visible: ht.myTurn && !!ht.legal && !!ht.legal.raise && ht.legal.raise.max > ht.legal.raise.min
                    Layout.alignment: Qt.AlignRight
                    spacing: 8
                    ThSlider {
                        Layout.preferredWidth: 240
                        from: ht.legal && ht.legal.raise ? ht.legal.raise.min : 0
                        to: ht.legal && ht.legal.raise ? ht.legal.raise.max : 1
                        stepSize: 1
                        snapMode: Controls.Slider.SnapAlways
                        value: ht.raiseTo
                        onMoved: ht.raiseTo = Math.round(value)
                        Accessible.name: "Raise amount"
                    }
                    Repeater {
                        model: ht.presets
                        Rectangle {   // a preset pill (the web's .preset / .presetOn)
                            required property var modelData
                            readonly property bool isOn: ht.raiseTo === modelData.to
                            implicitWidth: presetLabel.implicitWidth + 18
                            implicitHeight: 26
                            radius: height / 2
                            color: presetHover.hovered ? Th.surfaceHover : "transparent"
                            border.width: 1
                            border.color: isOn ? Th.accentBase : Th.borderSubtle
                            HoverHandler { id: presetHover; cursorShape: Qt.PointingHandCursor }
                            TapHandler { onTapped: ht.raiseTo = modelData.to }
                            Accessible.role: Accessible.Button
                            Accessible.name: String(modelData.label)
                            Controls.Label {
                                id: presetLabel
                                anchors.centerIn: parent
                                text: String(modelData.label)
                                textFormat: Text.PlainText
                                color: parent.isOn ? Th.accentBright : Th.textSecondary
                                font.pointSize: ht.fBody * 0.92
                            }
                        }
                    }
                }
            }
        }
    }

    ConfirmDialog {
        id: closeAsk
        app: ht.app
        dialogTitle: ht.say("close_title")
        message: ht.hand && ht.view && ht.view.status === "hand" ? ht.say("close_body_hand") : ht.say("close_body")
        confirmText: ht.say("close")
        destructive: true
        onConfirmed: ht.act({ do: "close" })
    }
}
