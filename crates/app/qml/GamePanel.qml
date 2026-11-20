// GamePanel.qml — mini games v1 (GAMES-V1-SPEC.md §6.4): the game island's content. Shown over the timeline island
// while a table is open (or the Games picker is up from the call strip's Games chip); "×" goes back to the chat.
// Contents, in order of precedence:
//   • the call room's server runs no games → one plain line
//   • no wallet yet → the one-time line (§2.2: "Tin Hat counts your time in calls on <server> …"; OK / Not now)
//   • no table open → this call's tables (Open) + "Open a table": Texas Hold'em (seats) / The Rig (Lite/Full,
//     bankroll, Patch), The Rig only where gameVisible says so
//   • a table → HoldemTable.qml / RigTable.qml from the server's view
// The header carries my chips + prestige (GET /me, `wallet` pushes) and the connection state; refused moves and
// the dealer's restarts surface as a toast (`play.notice`).
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app

Rectangle {
    id: gp
    property var app: null
    property var play: null
    property var words: ({})
    property string myId: ""
    property bool sharing: false
    readonly property bool shown: !!gp.app && !!gp.play && gp.play.state !== "" && (gp.app.gamePanelOpen || gp.play.tableId !== "")
    // rev 2: the call room's game space ({space, name, owner, why, link}); why "" = found
    readonly property var sp: {
        if (!gp.play || !gp.play.spaceJson) return null
        try { return JSON.parse(gp.play.spaceJson) } catch (e) { return null }
    }
    readonly property string spaceWhy: gp.sp ? String(gp.sp.why || "") : ""
    visible: gp.shown
    clip: true
    color: Th.islandBgCall
    objectName: "gamePanel"

    readonly property var view: {
        if (!gp.play || !gp.play.viewJson) return null
        try { return JSON.parse(gp.play.viewJson) } catch (e) { return null }
    }
    readonly property var wallet: {
        if (!gp.play || !gp.play.walletJson) return null
        try { return JSON.parse(gp.play.walletJson) } catch (e) { return null }
    }
    readonly property var tables: {
        if (!gp.play || !gp.play.tablesJson) return []
        try { return JSON.parse(gp.play.tablesJson) } catch (e) { return [] }
    }
    // server clock − local clock, from each view's `now`
    property real clockOffset: 0
    onViewChanged: if (gp.view && gp.view.now) gp.clockOffset = gp.view.now - Date.now()
    readonly property var art: gp.app && gp.app.playArt ? gp.app.playArt : ({})
    function lw(key, map) { return gp.fill(gp.words.lobby && gp.words.lobby[key] !== undefined ? gp.words.lobby[key] : key, map || ({})) }
    function nameOf(u) { return gp.app && gp.app.localpart ? String(gp.app.localpart(u)) : String(u) }
    // while seated at a Hold'em table: my stack (what's in front of me, not my bet); else null (the header shows the wallet)
    readonly property var myStack: {
        var v = gp.view
        if (!v || v.game !== "holdem" || !v.you) return null
        for (var i = 0; i < (v.seats || []).length; i++)
            if (v.seats[i].seat === v.you.seat) return v.seats[i].stack
        return null
    }
    function fill(s, map) {
        s = String(s === undefined ? "" : s)
        for (var k in map) s = s.split("{" + k + "}").join(String(map[k]))
        return s
    }
    // the body size; every piece = body × the web's ratio to its 13 px body (GamePanel / Games.module.css)
    readonly property real body: (Kirigami.Theme.defaultFont.pointSize > 0 ? Kirigami.Theme.defaultFont.pointSize : 10) * Th.textScale
    readonly property bool focusedWindow: !gp.app || !gp.app.core || gp.app.core.winState === undefined || gp.app.core.winState === "focused"

    // opening the panel (or a table) is an opt-in touch (§2.2) and the top-up check (§2.4): read /me
    // and a table needs room: the call fills the app while the panel is up ("fill the app", the same as the ⛶ button), and
    // goes back afterwards only if the panel was what filled it
    property bool _filledByUs: false
    onShownChanged: {
        if (gp.shown) {
            if (gp.play) gp.play.refreshWallet()
            if (gp.app && !gp.app.callMaximized) { gp._filledByUs = true; gp.app.callMaximized = true }
        } else if (gp._filledByUs) {
            gp._filledByUs = false
            if (gp.app) gp.app.callMaximized = false
        }
    }

    // the panel lies over the call tiles: no press, hold, wheel or hover may reach a tile underneath (a long press on a
    // tile opens its volume menu)
    MouseArea {
        anchors.fill: parent
        acceptedButtons: Qt.AllButtons
        hoverEnabled: true
        onWheel: function (wheel) { wheel.accepted = true }
    }

    ColumnLayout {
        anchors.fill: parent
        anchors.margins: 12
        spacing: 10

        // ── header (the web's GamePanel head: ‹ All games · the title · the space badge + my chips · ×) ──
        RowLayout {
            Layout.fillWidth: true
            spacing: 8
            ThToolButton {
                app: gp.app
                visible: !!gp.play && gp.play.tableId !== ""
                iconName: "go-previous-symbolic"
                iconSize: 16
                tip: gp.lw("all_games")
                Accessible.name: tip
                onClicked: { gp.play.closeTable(); gp.app.gamePanelOpen = true }
            }
            Controls.Label {
                // the web's panel head: the game's name while a table is open (its bar doesn't repeat it), else "Games"
                text: gp.view && gp.play && gp.play.tableId !== "" && gp.words.games && gp.words.games[gp.view.game] ? String(gp.words.games[gp.view.game]) : gp.lw("title")
                textFormat: Text.PlainText
                color: Th.textPrimary
                font.bold: true
                font.pointSize: gp.body * 1.04
            }
            Controls.Label {
                visible: gp.play && gp.play.state !== "online" && gp.play.state !== "" && gp.play.state !== "nospace"
                text: gp.play && gp.play.state === "retrying" ? "Reconnecting…" : gp.lw("connecting")
                textFormat: Text.PlainText
                color: Th.textMuted
                font.pointSize: gp.body * 0.85
            }
            Item { Layout.fillWidth: true }
            // whose chips are in play: the space's avatar, name and owner, never the name alone (§1.1)
            Row {
                visible: !!gp.sp && gp.spaceWhy === "" && !!gp.sp.space
                spacing: 6
                Avatar {
                    anchors.verticalCenter: parent.verticalCenter
                    app: gp.app
                    size: 20
                    name: gp.sp ? String(gp.sp.name || gp.sp.space) : ""
                    seed: gp.sp ? String(gp.sp.space) : ""
                }
                Controls.Label {
                    anchors.verticalCenter: parent.verticalCenter
                    text: gp.sp ? String(gp.sp.name || gp.sp.space) : ""
                    textFormat: Text.PlainText
                    color: Th.textPrimary
                    font.bold: true
                    font.pointSize: gp.body * 0.96   // the web's .spaceBadge (12.5 px)
                }
                Controls.Label {
                    anchors.verticalCenter: parent.verticalCenter
                    text: gp.sp ? "· " + String(gp.sp.owner) : ""
                    textFormat: Text.PlainText
                    color: Th.textMuted
                    font.pointSize: gp.body * 0.96
                }
            }
            Rectangle {   // my chips in this space
                visible: !!gp.wallet
                HoverHandler { id: chipHover }
                Controls.ToolTip.visible: chipHover.hovered
                Controls.ToolTip.delay: 300
                Controls.ToolTip.text: gp.wallet ? gp.fill(gp.words.wallet ? (gp.myStack !== null ? gp.words.wallet.stack_tip : gp.words.wallet.wallet_tip) : "", { space: gp.sp ? (gp.sp.name || gp.sp.space) : "" }) : ""
                Accessible.role: Accessible.StaticText
                Accessible.name: chipLabel.text + " · " + Controls.ToolTip.text
                radius: height / 2
                color: Th.surfaceRaised
                implicitWidth: walletRow.implicitWidth + 16
                implicitHeight: walletRow.implicitHeight + 8
                Row {
                    id: walletRow
                    anchors.centerIn: parent
                    spacing: 4
                    Rectangle {
                        anchors.verticalCenter: parent.verticalCenter
                        width: 10; height: 10; radius: 5
                        color: gp.art.chip || Th.accentBase
                        border.width: 2
                        border.color: gp.art.chipEdge || Th.accentBright
                    }
                    Controls.Label {
                        id: chipLabel
                        anchors.verticalCenter: parent.verticalCenter
                        // the web's rule: while seated my stack at this table (the number on my seat plate), else my wallet
                        text: gp.wallet ? String(gp.myStack !== null ? gp.myStack : gp.wallet.balance) : ""
                        textFormat: Text.PlainText
                        color: Th.textPrimary
                        font.weight: Font.DemiBold
                        font.pointSize: gp.body * 0.92   // the chips number
                    }
                    Controls.Label {   // "Prestige n", as the web's chip
                        visible: !!gp.wallet && gp.wallet.rank > 0
                        anchors.verticalCenter: parent.verticalCenter
                        text: gp.wallet ? gp.fill(gp.words.wallet ? gp.words.wallet.rank : "", { n: gp.wallet.rank }) : ""
                        textFormat: Text.PlainText
                        color: Th.textMuted
                        font.pointSize: gp.body * 0.85   // the web's .rank (11 px)
                    }
                }
            }
            ThToolButton {
                app: gp.app
                iconName: "window-close-symbolic"
                iconSize: 16
                tip: gp.lw("back_to_chat")
                Accessible.name: "Close the game panel"
                onClicked: { gp.app.gamePanelOpen = false; gp.play.closeTable() }
            }
        }
        Rectangle { Layout.fillWidth: true; implicitHeight: 1; color: Th.borderSubtle }

        // ── body ──
        Item {
            Layout.fillWidth: true
            Layout.fillHeight: true

            // rev 2: no games here, and why (a room in no space, the space's server, …) + "Use <space>'s chips here"
            ColumnLayout {
                visible: gp.play && !gp.play.available && gp.play.state !== "checking"
                anchors.centerIn: parent
                width: Math.min(parent.width - 20, 440)
                spacing: 12
                Controls.Label {
                    Layout.fillWidth: true
                    wrapMode: Text.Wrap
                    horizontalAlignment: Text.AlignHCenter
                    text: {
                        var why = gp.spaceWhy
                        if (why === "is-space") return String(gp.words.space ? gp.words.space.is_space : "")
                        if (why === "no-space" || why === "not-a-room" || why === "") return String(gp.words.space ? gp.words.space.none : "")
                        return String(gp.words.codes && gp.words.codes[why] ? gp.words.codes[why] : (gp.words.space ? gp.words.space.unconfirmed : ""))
                    }
                    textFormat: Text.PlainText
                    color: Th.textSecondary
                    font.pointSize: gp.body
                }
                Repeater {
                    model: gp.sp && gp.sp.link ? gp.sp.link : []
                    delegate: ColumnLayout {
                        required property var modelData
                        Layout.fillWidth: true
                        spacing: 6
                        Controls.Label {
                            Layout.fillWidth: true
                            wrapMode: Text.Wrap
                            horizontalAlignment: Text.AlignHCenter
                            text: gp.words.space ? gp.fill(gp.words.space.use_here_hint, { space: modelData.name }) : ""
                            textFormat: Text.PlainText
                            color: Th.textMuted
                            font.pointSize: gp.body * 0.92
                        }
                        ThButton {
                            Layout.alignment: Qt.AlignHCenter
                            app: gp.app
                            highlighted: true
                            text: gp.words.space ? gp.fill(gp.words.space.use_here, { space: modelData.name }) : ""
                            Accessible.name: text
                            onClicked: gp.play.useSpaceHere(String(modelData.space))
                        }
                    }
                }
            }

            // the one-time line (§2.2)
            Rectangle {
                id: optIn
                visible: gp.play && gp.play.available && gp.play.needsOptIn
                anchors.centerIn: parent
                // the house confirm dialog's box (ConfirmDialog: 380 wide, the dialog tokens, square corners; punch #8)
                width: Math.min(parent.width - 20, 380)
                height: optCol.implicitHeight + 40
                radius: 4
                color: Th.dialogBg
                border.width: 1
                border.color: Th.dialogBorder
                ColumnLayout {
                    id: optCol
                    anchors.fill: parent
                    anchors.margins: 20
                    spacing: 12
                    Controls.Label {
                        text: gp.words.optin ? String(gp.words.optin.title) : ""
                        textFormat: Text.PlainText
                        color: Th.dialogFg
                        font.bold: true
                        font.pointSize: gp.body * 1.15
                    }
                    Controls.Label {
                        Layout.fillWidth: true
                        wrapMode: Text.Wrap
                        text: gp.words.optin ? gp.fill(gp.words.optin.body, { space: gp.sp ? (gp.sp.name || gp.sp.space) : "" }) : ""
                        textFormat: Text.PlainText
                        color: Th.dialogFg
                        font.pointSize: gp.body
                    }
                    Controls.Label {   // the small print (polish #8)
                        Layout.fillWidth: true
                        wrapMode: Text.Wrap
                        text: gp.words.optin ? gp.fill(gp.words.optin.small, { space: gp.sp ? (gp.sp.name || gp.sp.space) : "", owner: gp.sp ? gp.sp.owner : "" }) : ""
                        textFormat: Text.PlainText
                        color: Th.textMuted   // the small print in muted grey (the web's)
                        font.pointSize: gp.body * 0.92
                    }
                    RowLayout {   // the house confirm order: the action first (filled), then Not now
                        Layout.fillWidth: true
                        spacing: 8
                        ThButton {
                            objectName: "playOptInOk"
                            Layout.fillWidth: true
                            Layout.preferredWidth: 1
                            // the label centred in the wide button (as the Hold'em action buttons; ThButton draws it left)
                            implicitWidth: implicitContentWidth + 28
                            leftPadding: Math.max(14, (width - implicitContentWidth) / 2)
                            app: gp.app
                            highlighted: true
                            text: gp.words.optin ? String(gp.words.optin.ok) : "OK"
                            Accessible.name: text
                            onClicked: gp.play.optIn()
                        }
                        ThButton {
                            // the equal second button (the house confirm order: the action first, filled; then this one)
                            Layout.fillWidth: true
                            Layout.preferredWidth: 1
                            implicitWidth: implicitContentWidth + 28
                            leftPadding: Math.max(14, (width - implicitContentWidth) / 2)
                            app: gp.app
                            text: gp.words.optin ? String(gp.words.optin.not_now) : "Not now"
                            Accessible.name: text
                            onClicked: { gp.app.gamePanelOpen = false; gp.play.closeTable() }
                        }
                    }
                }
            }

            // the Games list (the web's GamesWindow lobby): my chips in this space, this call's tables, Open a table
            Flickable {
                id: picker
                visible: gp.play && gp.play.available && !gp.play.needsOptIn && gp.play.tableId === ""
                anchors.fill: parent
                contentHeight: pickCol.implicitHeight + 24
                clip: true
                boundsBehavior: Flickable.StopAtBounds
                readonly property string spaceName: gp.sp ? String(gp.sp.name || gp.sp.space) : ""
                function openOf(g) { for (var i = 0; i < gp.tables.length; ++i) if (gp.tables[i].game === g && gp.tables[i].status !== "closed") return gp.tables[i]; return null }
                property int seats: 8
                ColumnLayout {
                    id: pickCol
                    width: picker.width
                    height: Math.max(implicitHeight, picker.height - 24)   // the foot line sits at the panel's foot (punch #19)
                    spacing: 14
                    // my chips in this space
                    Rectangle {
                        objectName: "gamesWallet"
                        Layout.fillWidth: true
                        implicitHeight: walletCol.implicitHeight + 28
                        radius: 12
                        gradient: Gradient {
                            orientation: Gradient.Horizontal
                            GradientStop { position: 0.0; color: gp.art.feltEdge || Th.surfaceRaised }
                            GradientStop { position: 0.6; color: Th.surfaceRaised }
                        }
                        border.width: 1
                        border.color: Th.borderSubtle
                        ColumnLayout {
                            id: walletCol
                            anchors.left: parent.left; anchors.right: parent.right; anchors.top: parent.top
                            anchors.margins: 14
                            spacing: 6
                            RowLayout {
                                spacing: 8
                                visible: !!gp.wallet
                                Rectangle {
                                    width: 14; height: 14; radius: 7
                                    color: gp.art.chip || Th.accentBase
                                    border.width: 3
                                    border.color: gp.art.chipEdge || Th.accentBright
                                }
                                Controls.Label { text: gp.wallet ? String(gp.wallet.balance) : ""; textFormat: Text.PlainText; color: Th.textPrimary; font.weight: Font.DemiBold; font.pointSize: gp.body * 1.15 }
                                Controls.Label { text: gp.lw("chips_in", { space: picker.spaceName }); textFormat: Text.PlainText; color: Th.textSecondary; font.pointSize: gp.body }
                            }
                            Controls.Label {
                                visible: !!gp.wallet
                                text: gp.wallet ? gp.lw("cap") + " " + gp.wallet.cap + "      " + gp.lw("earned_today") + " " + (gp.wallet.earned_today || 0) + " " + gp.lw("of", { n: gp.wallet.day_max || 8 }) + "      " + gp.lw("per_hour") : ""
                                textFormat: Text.PlainText
                                color: Th.textMuted
                                font.pointSize: gp.body * 0.92
                            }
                            Controls.Label {
                                visible: !!gp.wallet && gp.wallet.holdings >= gp.wallet.cap && !gp.wallet.at_table
                                text: gp.words.wallet ? String(gp.words.wallet.at_cap_idle) : ""
                                textFormat: Text.PlainText
                                color: gp.art.chip || Th.accentBright
                                font.pointSize: gp.body * 0.92
                            }
                            Controls.Label {
                                visible: !!gp.wallet && !!gp.wallet.topup && gp.wallet.topup.available === true
                                text: gp.words.wallet ? String(gp.words.wallet.topup) : ""
                                textFormat: Text.PlainText
                                color: Th.textSecondary
                                font.pointSize: gp.body * 0.92
                            }
                            Controls.Label {
                                visible: !gp.wallet
                                text: gp.lw("connecting_to", { owner: gp.sp ? gp.sp.owner : "" })
                                textFormat: Text.PlainText
                                color: Th.textSecondary
                                font.pointSize: gp.body
                            }
                        }
                    }
                    // in this call
                    Controls.Label { text: gp.lw("in_this_call").toUpperCase(); textFormat: Text.PlainText; color: Th.textSecondary; font.bold: true; font.letterSpacing: 0.3; font.pointSize: gp.body * 0.92 }
                    Controls.Label { visible: gp.tables.length === 0; text: gp.lw("no_tables"); textFormat: Text.PlainText; color: Th.textMuted; font.pointSize: gp.body }
                    Repeater {
                        model: gp.tables
                        delegate: Rectangle {
                            required property var modelData
                            Layout.fillWidth: true
                            implicitHeight: 56
                            radius: 10
                            color: Th.surfaceRaised
                            border.width: 1
                            border.color: Th.borderSubtle
                            // the web's row (punch #19): tile 30 · 10 px · name over "n seated · host x" · stretch · Join 12 px from the right
                            Rectangle {   // the game icon (♠ on felt green, ★ ringed in gold)
                                id: rowTile
                                x: 12
                                anchors.verticalCenter: parent.verticalCenter
                                width: 30; height: 30; radius: 6
                                color: modelData.game === "rig" ? "transparent" : (gp.art.feltCentre || Th.accentDim)
                                border.width: modelData.game === "rig" ? 1 : 0
                                border.color: gp.art.chip || Th.accentBase
                                Controls.Label { anchors.centerIn: parent; text: modelData.game === "rig" ? "★" : "♠"; textFormat: Text.PlainText; color: modelData.game === "rig" ? (gp.art.chip || Th.accentBright) : (gp.art.cardFace || Th.textPrimary); font.pointSize: gp.body }
                            }
                            Column {
                                anchors.left: rowTile.right
                                anchors.leftMargin: 10
                                anchors.right: rowJoin.left
                                anchors.rightMargin: 12
                                anchors.verticalCenter: parent.verticalCenter
                                spacing: 1
                                Controls.Label { width: parent.width; elide: Text.ElideRight; text: String(gp.words.games ? gp.words.games[modelData.game] : modelData.game); textFormat: Text.PlainText; color: Th.textPrimary; font.bold: true; font.pointSize: gp.body }
                                Controls.Label {
                                    width: parent.width
                                    elide: Text.ElideRight
                                    text: {
                                        var c = gp.play ? String(gp.play.chipLabel(JSON.stringify(modelData))) : ""
                                        // no chip (an ended table): "Table over" (words rig.phases.end), as the web
                                        c = c === "" ? String(gp.words.rig && gp.words.rig.phases ? gp.words.rig.phases.end : "")
                                                     : c.replace(/^\S+ [^·]+· /, "").replace(/ · \S+$/, "")
                                        return c + (modelData.host ? " " + gp.lw("host") + " " + gp.nameOf(modelData.host) : "")
                                    }
                                    textFormat: Text.PlainText
                                    color: Th.textMuted
                                    font.pointSize: gp.body * 0.92
                                }
                            }
                            ThButton {
                                id: rowJoin
                                anchors.right: parent.right
                                anchors.rightMargin: 12
                                anchors.verticalCenter: parent.verticalCenter
                                app: gp.app
                                // an ended table (still listed until it closes): a plain Open for its results, never Join
                                readonly property bool ended: !!gp.play && String(gp.play.chipLabel(JSON.stringify(modelData))) === ""
                                highlighted: !ended
                                flat: ended
                                text: ended ? String(gp.words.chip && gp.words.chip.open ? gp.words.chip.open : "Open") : gp.lw("join")
                                Accessible.name: text + " " + String(gp.words.games ? gp.words.games[modelData.game] : "")
                                onClicked: gp.play.openTable(String(modelData.table))
                            }
                        }
                    }
                    // open a table
                    Controls.Label { text: gp.lw("open_a_table").toUpperCase(); textFormat: Text.PlainText; color: Th.textSecondary; font.bold: true; font.letterSpacing: 0.3; font.pointSize: gp.body * 0.92 }
                    Flow {
                        Layout.fillWidth: true
                        spacing: 10
                        // Texas Hold'em
                        Rectangle {
                            width: Math.min(322, picker.width)
                            height: hCol.implicitHeight + 28
                            radius: 12
                            color: Th.surfaceRaised
                            border.width: 1
                            border.color: Th.borderSubtle
                            ColumnLayout {
                                id: hCol
                                anchors.left: parent.left; anchors.right: parent.right; anchors.top: parent.top
                                anchors.margins: 14
                                spacing: 10
                                RowLayout {
                                    spacing: 8
                                    Rectangle {
                                        implicitWidth: 30; implicitHeight: 30; radius: 6
                                        color: gp.art.feltCentre || Th.accentDim
                                        Controls.Label { anchors.centerIn: parent; text: "♠"; textFormat: Text.PlainText; color: gp.art.cardFace || Th.textPrimary; font.pointSize: gp.body }
                                    }
                                    Controls.Label { text: String(gp.words.games ? gp.words.games.holdem : ""); textFormat: Text.PlainText; color: Th.textPrimary; font.bold: true; font.pointSize: gp.body * 1.08 }
                                }
                                Controls.Label { Layout.fillWidth: true; wrapMode: Text.Wrap; text: gp.lw("holdem_blurb"); textFormat: Text.PlainText; color: Th.textSecondary; font.pointSize: gp.body * 0.96 }
                                RowLayout {
                                    Layout.fillWidth: true
                                    Controls.Label { text: gp.lw("seats"); textFormat: Text.PlainText; color: Th.textSecondary; font.pointSize: gp.body * 0.92 }
                                    Item { Layout.fillWidth: true }
                                    Rectangle {   // the seats segment (2 4 6 8)
                                        radius: height / 2
                                        color: Th.surfaceSunken
                                        implicitWidth: segRow.implicitWidth + 8
                                        implicitHeight: segRow.implicitHeight + 6
                                        Row {
                                            id: segRow
                                            anchors.centerIn: parent
                                            spacing: 2
                                            Repeater {
                                                model: [2, 4, 6, 8]
                                                Rectangle {
                                                    required property var modelData
                                                    width: 30; height: 24; radius: 12
                                                    color: picker.seats === modelData ? Th.surfaceSelected : "transparent"
                                                    Controls.Label { anchors.centerIn: parent; text: String(modelData); textFormat: Text.PlainText; color: picker.seats === modelData ? Th.textPrimary : Th.textSecondary; font.bold: picker.seats === modelData; font.pointSize: gp.body }
                                                    TapHandler { onTapped: picker.seats = modelData }
                                                    HoverHandler { cursorShape: Qt.PointingHandCursor }
                                                    Accessible.role: Accessible.Button
                                                    Accessible.name: gp.lw("seats") + " " + modelData
                                                }
                                            }
                                        }
                                    }
                                }
                                ThButton {
                                    Layout.fillWidth: true
                                    app: gp.app
                                    highlighted: !picker.openOf("holdem")
                                    text: picker.openOf("holdem") ? gp.lw("table_open_join") : gp.lw("open_holdem")
                                    Accessible.name: text
                                    onClicked: picker.openOf("holdem") ? gp.play.openTable(String(picker.openOf("holdem").table)) : gp.play.createTable("holdem", JSON.stringify({ seats: picker.seats }))
                                }
                            }
                        }
                        // The Rig (only where gameVisible says so)
                        Rectangle {
                            visible: gp.play && gp.play.rigOffered
                            width: Math.min(322, picker.width)
                            height: rCol.implicitHeight + 28
                            radius: 12
                            color: Th.surfaceRaised
                            border.width: 1
                            border.color: Th.borderSubtle
                            ColumnLayout {
                                id: rCol
                                anchors.left: parent.left; anchors.right: parent.right; anchors.top: parent.top
                                anchors.margins: 14
                                spacing: 10
                                RowLayout {
                                    spacing: 8
                                    Rectangle {
                                        implicitWidth: 30; implicitHeight: 30; radius: 6
                                        color: "transparent"
                                        border.width: 1
                                        border.color: gp.art.chip || Th.accentBase
                                        Controls.Label { anchors.centerIn: parent; text: "★"; textFormat: Text.PlainText; color: gp.art.chip || Th.accentBright; font.pointSize: gp.body }
                                    }
                                    Controls.Label { text: String(gp.words.games ? gp.words.games.rig : ""); textFormat: Text.PlainText; color: Th.textPrimary; font.bold: true; font.pointSize: gp.body * 1.08 }
                                }
                                Controls.Label { Layout.fillWidth: true; wrapMode: Text.Wrap; text: gp.lw("rig_blurb"); textFormat: Text.PlainText; color: Th.textSecondary; font.pointSize: gp.body * 0.96 }
                                ThButton {
                                    Layout.fillWidth: true
                                    app: gp.app
                                    highlighted: !picker.openOf("rig")
                                    text: picker.openOf("rig") ? gp.lw("table_open_join") : gp.lw("open_rig")
                                    Accessible.name: text
                                    // Lite is the first round's preset (§5.0); the host picks the rest in the lobby
                                    onClicked: picker.openOf("rig") ? gp.play.openTable(String(picker.openOf("rig").table)) : gp.play.createTable("rig", JSON.stringify({ preset: "lite", bankroll: 6, patch: true }))
                                }
                            }
                        }
                    }
                    Item { Layout.fillHeight: true; Layout.fillWidth: true }
                    Controls.Label {
                        Layout.fillWidth: true
                        Layout.topMargin: 8
                        horizontalAlignment: Text.AlignHCenter
                        wrapMode: Text.Wrap
                        text: gp.lw("footnote", { space: picker.spaceName })
                        textFormat: Text.PlainText
                        color: Th.textMuted
                        font.pointSize: gp.body * 0.88
                    }
                }
            }

            Controls.Label {
                visible: gp.play && gp.play.available && !gp.play.needsOptIn && gp.play.tableId !== "" && !gp.view
                anchors.centerIn: parent
                text: gp.words.card ? String(gp.words.card.loading) : ""
                textFormat: Text.PlainText
                color: Th.textMuted
                font.pointSize: gp.body
            }
            Loader {
                anchors.fill: parent
                active: !!gp.view && gp.play.tableId !== "" && gp.view.game === "holdem" && !gp.play.needsOptIn && gp.view.status !== "closed"
                visible: active
                sourceComponent: HoldemTable {
                    app: gp.app
                    play: gp.play
                    view: gp.view
                    words: gp.words
                    myId: gp.myId
                    sharing: gp.sharing
                    clockOffset: gp.clockOffset
                    focusedWindow: gp.focusedWindow
                }
            }
            Loader {
                anchors.fill: parent
                active: !!gp.view && gp.play.tableId !== "" && gp.view.game === "rig" && !gp.play.needsOptIn && gp.view.status !== "closed"
                visible: active
                sourceComponent: RigTable {
                    app: gp.app
                    play: gp.play
                    view: gp.view
                    words: gp.words
                    myId: gp.myId
                    hideSecret: gp.sharing
                    clockOffset: gp.clockOffset
                    focusedWindow: gp.focusedWindow
                }
            }

            // a closed table (polish #11): centred, the game's glyph tile, the full reason, my chips, Back to Games
            ColumnLayout {
                objectName: "tableClosedLine"
                visible: !!gp.view && gp.view.status === "closed" && gp.play.tableId !== ""
                anchors.centerIn: parent
                width: Math.min(parent.width - 40, 460)
                spacing: 12
                Rectangle {
                    Layout.alignment: Qt.AlignHCenter
                    implicitWidth: 40; implicitHeight: 40; radius: 8
                    color: gp.view && gp.view.game === "rig" ? "transparent" : (gp.art.feltCentre || Th.accentDim)
                    border.width: gp.view && gp.view.game === "rig" ? 1 : 0
                    border.color: gp.art.chip || Th.accentBase
                    Controls.Label { anchors.centerIn: parent; text: gp.view && gp.view.game === "rig" ? "★" : "♠"; textFormat: Text.PlainText; color: gp.view && gp.view.game === "rig" ? (gp.art.chip || Th.accentBright) : (gp.art.cardFace || Th.textPrimary); font.pointSize: gp.body * 1.3 }
                }
                Controls.Label {
                    Layout.fillWidth: true
                    horizontalAlignment: Text.AlignHCenter
                    wrapMode: Text.Wrap
                    text: {
                        var r = gp.view && gp.view.closed ? String(gp.view.closed.reason) : ""
                        var m = gp.words.table && gp.words.table.closed_reasons ? gp.words.table.closed_reasons : ({})
                        return m[r] !== undefined ? String(m[r]) : String(gp.words.table ? gp.words.table.closed : "")
                    }
                    textFormat: Text.PlainText
                    color: Th.textPrimary
                    font.pointSize: gp.body
                }
                Controls.Label {
                    Layout.alignment: Qt.AlignHCenter
                    visible: !!gp.wallet
                    text: gp.wallet ? gp.lw("you_have", { n: gp.wallet.holdings !== undefined ? gp.wallet.holdings : gp.wallet.balance }) : ""
                    textFormat: Text.PlainText
                    color: Th.textSecondary
                    font.pointSize: gp.body
                }
                ThButton {
                    Layout.alignment: Qt.AlignHCenter
                    app: gp.app
                    highlighted: true
                    text: gp.lw("closed_back")
                    Accessible.name: text
                    onClicked: { gp.play.closeTable(); gp.app.gamePanelOpen = true }
                }
            }

            // the toast: refused moves, restarts
            Rectangle {
                id: toast
                property string text: ""
                anchors.horizontalCenter: parent.horizontalCenter
                anchors.top: parent.top
                anchors.topMargin: 4
                visible: toast.text !== ""
                z: 10
                radius: 10
                color: Th.statusPopoverBg
                border.width: 1
                border.color: Th.statusPopoverBorder
                implicitWidth: Math.min(parent.width - 40, toastLabel.implicitWidth + 28)
                implicitHeight: toastLabel.implicitHeight + 14
                Controls.Label {
                    id: toastLabel
                    anchors.centerIn: parent
                    width: Math.min(implicitWidth, toast.width - 28)
                    text: toast.text
                    textFormat: Text.PlainText
                    elide: Text.ElideRight
                    color: Th.statusPopoverFg
                    font.pointSize: gp.body
                }
                Timer { id: toastTimer; interval: 4000; onTriggered: toast.text = "" }
                Connections {
                    target: gp.play
                    function onNoticeRevChanged() { toast.text = gp.play.notice; toastTimer.restart() }
                }
                Accessible.role: Accessible.AlertMessage
                Accessible.name: toast.text
            }
        }
    }
}
