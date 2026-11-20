// GameChatCard.qml — mini games v1 (GAMES-V1-SPEC.md §6.6): a game card in chat. The message carries only a pointer
// (`dev.tinhat.game`: kind, game, server, table, round) and a body with no amounts; the card is drawn from the ROOM'S
// OWN server's record (PlayController.cardView: a card naming another server is "Couldn't load this game" and nothing is
// fetched from the server it names; the record's room must be this room). A game hidden here (gameVisible) is the
// neutral line. The web's features/play/GameCard.tsx (+ GameCard.module.css) is the same card (punch #21):
//   - a quiet one-line card (muted, no tile) while loading, failed, gone, hidden, or for an invite whose table closed;
//   - invite, table live: the 30 px art tile, "<name + prestige token> opened a Texas Hold'em table" (bold), the muted
//     "N seated · join from the call" (The Rig: "N joined"), and Open (chip.open) at the right;
//   - result / summary: the tile + "<game> results" / "<game> table closed" + "N hands · biggest pot N" (The Rig: rounds),
//     then net per player, best first (the leaderboard, else the net map), wins/losses in the art's win/loss colours.
// Who opened an invite comes from the live table list (the server's), never the event. The desktop lists only the
// tables of the call I'm in, so an invite seen outside that call can't tell live from closed: it draws the tile, the
// game and "join from the call" without a name or a button (the web lists every room's tables).
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app

Rectangle {
    id: gameCardRoot
    property var app: null
    property var block: null
    property string roomId: ""
    property real maxW: 360
    readonly property var play: gameCardRoot.app ? gameCardRoot.app.playCtl : null
    readonly property var words: gameCardRoot.app && gameCardRoot.app.playWords ? gameCardRoot.app.playWords : ({})
    readonly property var cw: gameCardRoot.words.card ? gameCardRoot.words.card : ({})
    readonly property var art: gameCardRoot.app && gameCardRoot.app.playArt ? gameCardRoot.app.playArt : ({})
    readonly property var rec: {
        var _ = gameCardRoot.play ? gameCardRoot.play.cardsRev : 0   // re-read when a record arrives
        if (!gameCardRoot.play || !gameCardRoot.block || !gameCardRoot.roomId) return null
        var s = gameCardRoot.play.cardView(gameCardRoot.roomId, JSON.stringify(gameCardRoot.block))
        try { return s ? JSON.parse(s) : null } catch (e) { return null }
    }
    readonly property string st: gameCardRoot.rec ? String(gameCardRoot.rec.state) : "loading"
    readonly property string kind: gameCardRoot.block ? String(gameCardRoot.block.kind) : ""
    readonly property bool isInvite: gameCardRoot.kind === "invite"
    readonly property var rdata: gameCardRoot.rec && gameCardRoot.rec.data ? gameCardRoot.rec.data : null
    // this room's call is mine: the call strip's table list is this room's
    readonly property bool inCall: !!gameCardRoot.app && !!gameCardRoot.roomId && gameCardRoot.app.mInCall === true && gameCardRoot.app.mCallRoomId === gameCardRoot.roomId
    // that list has been read from the server (the web's `loaded`)
    // ("" until this call's room listing arrives: "online" alone came before the room subscription)
    readonly property bool listed: gameCardRoot.inCall && !!gameCardRoot.play && gameCardRoot.play.tablesJson !== ""
    // the invite's table in that list while it isn't closed (the web's `live`); null = not listed
    readonly property var liveRow: {
        if (!gameCardRoot.isInvite || !gameCardRoot.block) return null
        // outside my call: the room's own listing, read by load_card (the row, or {"closed": true})
        if (!gameCardRoot.inCall) {
            var d = gameCardRoot.rdata
            return d && !d.closed && d.table === gameCardRoot.block.table ? d : null
        }
        if (!gameCardRoot.play) return null
        try {
            var t = JSON.parse(gameCardRoot.play.tablesJson || "[]")
            for (var i = 0; i < t.length; ++i)
                if (t[i].table === gameCardRoot.block.table && t[i].status !== "closed") return t[i]
        } catch (e) {}
        return null
    }
    readonly property bool live: gameCardRoot.liveRow !== null
    // the card's action (play_wire::card_action, spec §4.5; the web mirrors it): "open" | "join_call" | "".
    // An ended table outside the call: "Table over", no action (the chat's results card carries the results).
    readonly property string action: gameCardRoot.isInvite && gameCardRoot.play
        ? String(gameCardRoot.play.cardAction("invite", gameCardRoot.liveRow ? String(gameCardRoot.liveRow.status || "") : "", gameCardRoot.inCall))
        : ""
    // which game it is: the server's record (the listing / the summary) where there is one, else the pointer
    readonly property string recGame: {
        if (gameCardRoot.liveRow && gameCardRoot.liveRow.game) return String(gameCardRoot.liveRow.game)
        if (!gameCardRoot.isInvite && gameCardRoot.rdata && gameCardRoot.rdata.game) return String(gameCardRoot.rdata.game)
        return gameCardRoot.block ? String(gameCardRoot.block.game) : ""
    }
    readonly property bool isRig: gameCardRoot.recGame === "rig"
    // the one-line quiet card's sentence ("" = the full card)
    readonly property string quietText: {
        var s = gameCardRoot.st
        if (s === "failed") return String(gameCardRoot.cw.failed || "")
        if (s === "hidden") return String(gameCardRoot.cw.neutral || "")
        if (s === "gone") return String(gameCardRoot.cw.gone || "")
        if (s !== "ok") return String(gameCardRoot.cw.loading || "")
        if (gameCardRoot.isInvite) {
            if (gameCardRoot.live) return ""
            var known = gameCardRoot.inCall ? gameCardRoot.listed : !!gameCardRoot.rdata
            return known ? String(gameCardRoot.words.table && gameCardRoot.words.table.closed || "") : String(gameCardRoot.cw.loading || "")
        }
        return gameCardRoot.rdata ? "" : String(gameCardRoot.cw.loading || "")
    }
    // decided by the state, never by whether a word is non-empty
    readonly property bool quiet: gameCardRoot.st !== "ok" || (gameCardRoot.isInvite ? !gameCardRoot.live : !gameCardRoot.rdata)
    readonly property string hostId: gameCardRoot.liveRow && gameCardRoot.liveRow.host ? String(gameCardRoot.liveRow.host) : ""
    readonly property string gameWord: gameCardRoot.words.games ? String(gameCardRoot.words.games[gameCardRoot.recGame] || "") : ""
    // the invite's sentence after the name (words card.opened_*; the web had these as literals)
    readonly property string openedText: {
        if (!gameCardRoot.live) return String(gameCardRoot.isRig ? (gameCardRoot.cw.invite_rig || "") : (gameCardRoot.cw.invite_holdem || ""))
        return String(gameCardRoot.isRig ? (gameCardRoot.cw.opened_rig || "") : (gameCardRoot.cw.opened_holdem || ""))
    }
    // an ended table: "Table over" (rig.phases.end), as the web's card
    readonly property string inviteSub: gameCardRoot.live
        ? (gameCardRoot.liveRow.status === "end"
            ? String(gameCardRoot.words.rig && gameCardRoot.words.rig.phases ? gameCardRoot.words.rig.phases.end : "")
            : gameCardRoot.fill(gameCardRoot.isRig ? gameCardRoot.cw.joined_line : gameCardRoot.cw.seated_line, Number(gameCardRoot.liveRow.seated || 0)))
        : String(gameCardRoot.words.table && gameCardRoot.words.table.closed || "")
    // result / summary (the web: "<game> results" / "<game> table closed"; "N hands" or "N rounds", " · biggest pot N")
    readonly property string resultTitle: String((gameCardRoot.rdata && gameCardRoot.rdata.status === "closed" ? gameCardRoot.cw.closed_title : gameCardRoot.cw.results_title) || "").split("{game}").join(gameCardRoot.gameWord)
    readonly property string resultSub: {
        var d = gameCardRoot.rdata
        if (!d) return ""
        var s = gameCardRoot.isRig ? (d.rounds ? gameCardRoot.fill(gameCardRoot.cw.rounds, d.rounds) : "") : (d.hands ? gameCardRoot.fill(gameCardRoot.cw.hands, d.hands) : "")
        if (d.biggest_pot) s += (s ? " · " : "") + gameCardRoot.fill(gameCardRoot.cw.biggest_pot_short, d.biggest_pot)
        return s
    }
    // net per player, best first: the summary's leaderboard, else its net map (a Rig round record: reveal.net)
    readonly property var netRows: {
        var d = gameCardRoot.rdata
        if (gameCardRoot.quiet || gameCardRoot.isInvite || !d) return []
        var rows = []
        if (d.leaderboard && d.leaderboard.length) {
            for (var i = 0; i < d.leaderboard.length; ++i) rows.push({ u: String(d.leaderboard[i].u), n: Number(d.leaderboard[i].net) || 0 })
        } else {
            var net = d.net || (d.reveal ? d.reveal.net : null)
            if (net) for (var u in net) rows.push({ u: u, n: Number(net[u]) || 0 })
        }
        rows.sort(function (a, b) { return b.n - a.n })
        return rows
    }
    readonly property string a11yText: {
        if (gameCardRoot.quiet) return gameCardRoot.quietText
        if (gameCardRoot.isInvite)
            return (gameCardRoot.live ? gameCardRoot.nameOf(gameCardRoot.hostId) + " " : "") + gameCardRoot.openedText + ". " + gameCardRoot.inviteSub
        return gameCardRoot.resultTitle + (gameCardRoot.resultSub ? ". " + gameCardRoot.resultSub : "")
    }
    function nameOf(u) { return gameCardRoot.app && gameCardRoot.app.localpart ? String(gameCardRoot.app.localpart(u)) : String(u) }
    function fill(s, n) { return String(s === undefined ? "" : s).replace("{n}", n) }
    // the body size; every piece = body × the web's ratio to its 13 px body (GameCard.module.css)
    readonly property real body: (Kirigami.Theme.defaultFont.pointSize > 0 ? Kirigami.Theme.defaultFont.pointSize : 10) * Th.textScale

    implicitWidth: Math.min(gameCardRoot.maxW, 420)
    implicitHeight: col.implicitHeight + 20
    width: implicitWidth
    height: implicitHeight
    radius: 12
    color: Th.bubbleOtherBg
    border.width: 1
    border.color: gameCardRoot.isRig && !gameCardRoot.quiet ? Th.accentDim : Th.bubbleOtherBorder
    objectName: "gameChatCard"
    Accessible.role: Accessible.StaticText
    Accessible.name: gameCardRoot.a11yText

    ColumnLayout {
        id: col
        anchors.fill: parent
        anchors.leftMargin: 12
        anchors.rightMargin: 12
        anchors.topMargin: 10
        anchors.bottomMargin: 10
        spacing: 6

        // the quiet card: one muted line (the web's .quiet, 12.5 px)
        Controls.Label {
            Layout.fillWidth: true
            visible: gameCardRoot.quiet
            text: gameCardRoot.quietText
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
            color: Th.textMuted
            font.pointSize: gameCardRoot.body * 0.96
        }

        RowLayout {
            Layout.fillWidth: true
            visible: !gameCardRoot.quiet
            spacing: gameCardRoot.isInvite ? 10 : 8

            Rectangle {   // the game's tile in the art colours (♠ on the felt; The Rig's gold-trimmed ★ on the machine body)
                Accessible.ignored: true
                Layout.alignment: Qt.AlignVCenter
                implicitWidth: 30; implicitHeight: 30; radius: 8
                color: gameCardRoot.isRig ? (gameCardRoot.art.machineBody || Th.bubbleOtherBg) : (gameCardRoot.art.feltCentre || Th.accentDim)
                border.width: gameCardRoot.isRig ? 1 : 0
                border.color: gameCardRoot.art.machineTrim || Th.accentBase
                Controls.Label {
                    anchors.centerIn: parent
                    text: gameCardRoot.isRig ? "★" : "♠"
                    textFormat: Text.PlainText
                    color: gameCardRoot.isRig ? (gameCardRoot.art.machineTrim || Th.accentBright) : (gameCardRoot.art.cardFace || Th.textPrimary)
                    font.pointSize: gameCardRoot.body * 1.31   // 17 px
                }
            }

            // invite: the sentence + the seated line
            ColumnLayout {
                Layout.fillWidth: true
                Layout.alignment: Qt.AlignVCenter
                visible: gameCardRoot.isInvite
                spacing: 2
                RowLayout {
                    Layout.fillWidth: true
                    spacing: 4
                    PersonName {
                        visible: gameCardRoot.live
                        Layout.fillWidth: false
                        Layout.alignment: Qt.AlignTop
                        Layout.maximumWidth: Math.max(40, col.width * 0.5)
                        mxid: gameCardRoot.hostId
                        roomId: gameCardRoot.roomId
                        text: gameCardRoot.nameOf(gameCardRoot.hostId)
                        color: Th.bubbleOtherFg
                        font.bold: true
                        font.pointSize: gameCardRoot.body
                    }
                    Controls.Label {
                        Layout.fillWidth: true
                        Layout.alignment: Qt.AlignTop
                        text: gameCardRoot.openedText
                        textFormat: Text.PlainText
                        wrapMode: Text.Wrap
                        color: Th.bubbleOtherFg
                        font.bold: true
                        font.pointSize: gameCardRoot.body
                    }
                }
                Controls.Label {
                    Layout.fillWidth: true
                    text: gameCardRoot.inviteSub
                    textFormat: Text.PlainText
                    elide: Text.ElideRight
                    color: Th.textMuted
                    font.pointSize: gameCardRoot.body * 0.92   // the web's .sub (12 px)
                }
            }

            // result / summary: the title + its numbers on one line
            Controls.Label {
                visible: !gameCardRoot.isInvite
                Layout.alignment: Qt.AlignVCenter
                text: gameCardRoot.resultTitle
                textFormat: Text.PlainText
                color: Th.bubbleOtherFg
                font.bold: true
                font.pointSize: gameCardRoot.body
            }
            Controls.Label {
                visible: !gameCardRoot.isInvite
                Layout.fillWidth: true
                Layout.alignment: Qt.AlignVCenter
                text: gameCardRoot.resultSub
                textFormat: Text.PlainText
                elide: Text.ElideRight
                color: Th.textMuted
                font.pointSize: gameCardRoot.body * 0.92
            }

            ThButton {
                // Open (in this room's call) or Join the call (outside it: no live views there; the panel then opens the table)
                visible: gameCardRoot.action !== ""
                Layout.alignment: Qt.AlignVCenter
                app: gameCardRoot.app
                highlighted: true
                text: gameCardRoot.action === "join_call" ? String(gameCardRoot.cw.join_call || "Join the call")
                                                          : String(gameCardRoot.words.chip && gameCardRoot.words.chip.open || "Open")
                Accessible.name: text
                onClicked: gameCardRoot.action === "join_call"
                    ? gameCardRoot.app.joinCallForGame(String(gameCardRoot.roomId), String(gameCardRoot.block.table))
                    : gameCardRoot.app.openGames(String(gameCardRoot.block.table))
            }
        }

        // results: net per player, best first
        Repeater {
            model: gameCardRoot.netRows
            delegate: RowLayout {
                id: netRow
                required property var modelData
                readonly property bool someone: /^~/.test(netRow.modelData.u)
                Layout.fillWidth: true
                Layout.leftMargin: 2
                Layout.rightMargin: 2
                spacing: 8
                PersonName {
                    visible: !netRow.someone
                    Layout.fillWidth: true
                    mxid: netRow.modelData.u
                    roomId: gameCardRoot.roomId
                    text: gameCardRoot.nameOf(netRow.modelData.u)
                    color: Th.bubbleOtherFg
                    font.weight: Font.DemiBold   // the web's .rowName
                    font.pointSize: gameCardRoot.body
                }
                Controls.Label {
                    visible: netRow.someone
                    Layout.fillWidth: true
                    text: String(gameCardRoot.words.rig && gameCardRoot.words.rig.someone || "someone")
                    textFormat: Text.PlainText
                    elide: Text.ElideRight
                    color: Th.bubbleOtherFg
                    font.weight: Font.DemiBold
                    font.pointSize: gameCardRoot.body
                }
                Controls.Label {
                    text: (netRow.modelData.n > 0 ? "+" : "") + netRow.modelData.n
                    textFormat: Text.PlainText
                    color: netRow.modelData.n > 0 ? (gameCardRoot.art.win || Th.accentBright)
                         : (netRow.modelData.n < 0 ? (gameCardRoot.art.loss || Th.textMuted) : Th.bubbleOtherFg)
                    font.bold: true
                    font.features: ({ "tnum": 1 })
                    font.pointSize: gameCardRoot.body
                }
            }
        }
    }
}
