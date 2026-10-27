// GameServersWidget.qml — widgets: Game servers (`gameServers`, docs/GAMESERVERS.md §4–§5; the web's
// features/widgets/gameservers/GameServers.tsx). What runs on the space's game servers (the Tin Hat server's
// /api/gameservers/status through GameServersController — never the panel itself), who's on, uptime, CPU and RAM, the
// join address + password (masked until shown), a line of info, the "Ports closed — LAN only" chip and the next
// calendar event for the game; admins (`manage_game_servers`, checked by the server on every request) start, stop and
// restart servers (GameServerPower: the confirmations and the "Stop X and start Y" switch). S / collapsed = one line;
// M = the running servers as cards + "Other games (n)" (collapsed); L = every server as a card.
//
// Data: the controller's statusJson (a VIEW: the read status + per-server chip / players line / uptime / bars / size,
// computed by crates/widgets gameservers.rs = the web's gameservers.ts). Samples (the gallery's preview without data,
// and the gates' TINHAT_GAMESERVERS_DEMO=<key>) are built here and read through the same `view()`. Motion (only while the
// host animates): stateFade (the dot), barEase (the bars), rowSlide (a card that appears). Nothing ticks.
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the active theme (docs/MIGRATING-THEME.md)

Item {
    id: w
    property var app
    property var hostRef
    property string size: "M"
    property var settings: ({})
    property bool preview: false
    readonly property var ctl: w.app ? w.app.gameServersCtl : null
    readonly property string demo: w.ctl ? String(w.ctl.demoKey()) : ""

    // the poller runs while this widget exists (the gallery's preview shows live data too)
    Component.onCompleted: { if (w.ctl && w.demo.length === 0) w.ctl.acquire(); w.prevIds = w.upIds }
    Component.onDestruction: if (w.ctl && w.demo.length === 0) w.ctl.release()

    // ── the live data ──
    readonly property string liveText: w.ctl ? String(w.ctl.statusJson) : ""
    readonly property string liveError: w.ctl ? String(w.ctl.error) : ""
    readonly property bool liveReady: !!w.ctl && (!w.ctl.loading || w.liveText.length > 0 || w.liveError.length > 0)
    readonly property bool hasData: w.liveText.length > 0
    // the sample: TINHAT_GAMESERVERS_DEMO wins; the gallery shows "preview" without data
    readonly property string sampleKey: w.demo.length > 0 ? w.demo : ((w.preview && !w.hasData) ? "preview" : "")
    readonly property bool sample: w.sampleKey.length > 0
    readonly property var sampleModel: w.sample ? w.sampleOf(w.sampleKey) : null
    readonly property bool ready: w.sample ? w.sampleModel.ready : w.liveReady
    readonly property string error: w.sample ? w.sampleModel.error : w.liveError
    readonly property string viewText: w.sample ? w.sampleModel.viewText : w.liveText
    readonly property var view: { try { return w.viewText.length > 0 ? JSON.parse(w.viewText) : null } catch (e) { return null } }
    readonly property var servers: w.view && w.view.servers ? w.view.servers : []
    readonly property bool canManage: !!w.view && w.view.canManage === true && !w.preview
    readonly property var upList: w.servers.filter(function (s) { return s.up === true })
    readonly property var downList: w.servers.filter(function (s) { return s.up !== true })
    // the calendar's events (the "Next:" line): WidgetsController.eventsJson, or the sample's
    readonly property var wc: w.hostRef ? w.hostRef.widgets : null
    readonly property string eventsText: w.sample ? JSON.stringify(w.sampleModel.events) : (w.wc ? String(w.wc.eventsJson) : "")
    readonly property var events: { try { return w.eventsText.length > 0 ? JSON.parse(w.eventsText) : [] } catch (e) { return [] } }
    property double now: Date.now()
    onViewTextChanged: w.now = Date.now()

    readonly property bool live: w.upList.length > 0
    readonly property string summaryText: !w.ready ? "Checking…" : (w.error === "not-member" ? "Only for the members of its space"
        : (w.view ? String(w.view.summary) : "Can't reach the game servers"))
    // (the header Loader sizes its item: a bare Text there never showed — WidgetHeaderText, W1's shared fix, elides)
    readonly property Component summaryComponent: Component { WidgetHeaderText { text: w.summaryText } }
    // the header's extra (M / L): "1 of 2 running" while something runs
    readonly property Component extraComponent: (!!w.view && w.upList.length > 0) ? runningC : null
    Component {
        id: runningC
        WidgetHeaderText { role: "num"; text: w.view ? w.upList.length + " of " + w.view.cap + " running" : "" }
    }

    // a failed switch of mine: its error, once; a refused power request: the controller's toast
    property string _jobSeen: ""
    onViewChanged: {
        var j = w.view ? w.view.job : null
        if (w.sample || !j || j.step !== "failed" || !j.error || !w.app || j.by !== String(w.app.mUserId || "")) return
        var k = j.from + "/" + j.to + "/" + j.since
        if (k === w._jobSeen) return
        w._jobSeen = k
        power.toast(String(j.error))
    }
    Connections {
        target: w.ctl
        ignoreUnknownSignals: true
        function onToastRevChanged() { if (!w.preview && w.ctl && String(w.ctl.toast).length > 0) power.toast(String(w.ctl.toast)) }
    }

    // rowSlide for a card that appears (after the first fill)
    readonly property var upIds: w.upList.map(function (s) { return s.id })
    property var prevIds: null
    onUpIdsChanged: Qt.callLater(function () { w.prevIds = w.upIds })
    function isFresh(id) { return w.prevIds !== null && w.prevIds.indexOf(id) < 0 }

    // ── labels (Up next's words and the app's clock setting) ──
    readonly property var dows: ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"]
    readonly property var mons: ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"]
    function dayStart(ms) { var d = new Date(ms); return new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime() }
    function dayWord(ms) {
        var days = Math.round((w.dayStart(ms) - w.dayStart(w.now)) / 86400000)
        var d = new Date(ms)
        if (days === 0) return "Today"
        if (days === 1) return "Tomorrow"
        if (days > 1 && days < 7) return w.dows[d.getDay()]
        return w.dows[d.getDay()] + " " + d.getDate() + " " + w.mons[d.getMonth()]
    }
    function timeOf(ms) {
        var d = new Date(ms)
        var mode = w.app ? String(w.app.clockMode || "") : ""
        if (mode === "12") return Qt.formatTime(d, "h:mm AP")
        if (mode === "24") return Qt.formatTime(d, "HH:mm")
        return Qt.formatTime(d, String(Qt.locale().timeFormat(Locale.ShortFormat)).replace(/[:.]ss/, ""))
    }
    function nextOf(sv) {
        if (!w.ctl || !sv || sv.calendar === false) return null
        var i = w.ctl.nextEvent(w.viewText, sv.id, w.eventsText, w.now)
        return i >= 0 && i < w.events.length ? w.events[i] : null
    }
    function coverOf(sv) { return sv && sv.cover ? String(sv.cover) : "" }
    function toneColor(t) { return t === "online" ? Th.widgetServersStateOnline : (t === "busy" ? Th.widgetServersStateBusy : Th.widgetServersStateOffline) }
    function copy(text) { if (w.app && !w.preview) { w.app.copyToClipboard(text); w.app.showPassiveNotification("Copied", 2000) } }
    function busyOf(sv) { return !!sv.pending || sv.state === "starting" || sv.state === "stopping" }

    GameServerPower { id: power; app: w.app; viewText: w.viewText; preview: w.preview || w.sample }

    property bool othersOpen: false
    readonly property bool large: w.size === "L" && !w.preview
    readonly property var cards: w.large ? w.upList.concat(w.downList) : w.upList
    readonly property var others: w.large ? [] : w.downList

    implicitHeight: col.implicitHeight

    Column {
        id: col
        x: 6; width: parent.width - 12
        spacing: 6

        // ── not read yet / members only / no answer (a quiet 44 px band) ──
        Item {
            visible: !w.ready || w.error === "not-member" || (!w.view && w.ready)
            width: parent.width; height: 44
            Row {
                anchors.centerIn: parent
                spacing: 4
                WidgetText {
                    anchors.verticalCenter: parent.verticalCenter
                    role: "meta"
                    text: !w.ready ? "Checking the game servers…" : (w.error === "not-member" ? "Only for the members of its space" : "Can't reach the game servers ·")
                    color: Th.widgetMutedFg
                }
                Controls.AbstractButton {
                    id: retry
                    visible: w.ready && !w.view && w.error !== "not-member"
                    anchors.verticalCenter: parent.verticalCenter
                    implicitWidth: retryTxt.implicitWidth; implicitHeight: retryTxt.implicitHeight
                    enabled: !w.preview && !w.sample
                    hoverEnabled: true
                    focusPolicy: Qt.TabFocus
                    contentItem: WidgetText { id: retryTxt; role: "meta"; semibold: true; text: "Retry"; color: Th.accentBright; font.underline: retry.hovered || retry.visualFocus }
                    onClicked: if (w.ctl) w.ctl.refresh()
                }
            }
        }

        // ── nothing running ──
        Item {
            visible: !!w.view && w.upList.length === 0
            width: parent.width
            height: Math.max(64, emptyCol.implicitHeight + 12)
            Column {
                id: emptyCol
                anchors.centerIn: parent
                spacing: 4
                ThIcon {
                    anchors.horizontalCenter: parent.horizontalCenter
                    width: 20; height: 20
                    app: w.app
                    name: "network-server-symbolic"
                    color: Th.widgetMutedFg
                }
                WidgetText {
                    anchors.horizontalCenter: parent.horizontalCenter
                    role: "text"
                    text: "No game running"
                    color: Th.widgetMetaFg
                }
                StartLink { anchors.horizontalCenter: parent.horizontalCenter; visible: w.canManage }
            }
        }

        // ── the cards ──
        Repeater {
            model: w.cards
            delegate: Card { width: col.width; sv: modelData }
        }

        // ── other games (n) ──
        Item {
            visible: w.others.length > 0
            width: parent.width; height: 24
            Controls.AbstractButton {
                id: othersBtn
                anchors.left: parent.left; anchors.verticalCenter: parent.verticalCenter
                implicitHeight: 22
                implicitWidth: othersRow.implicitWidth + 8
                hoverEnabled: true
                focusPolicy: Qt.TabFocus
                Accessible.name: "Other games (" + w.others.length + ")"
                background: Rectangle { radius: 6; color: (othersBtn.hovered || othersBtn.visualFocus) ? Th.widgetRowBgHover : "transparent" }
                contentItem: Item {
                    Row {
                        id: othersRow
                        anchors.verticalCenter: parent.verticalCenter
                        x: 4
                        spacing: 4
                        ThIcon {
                            anchors.verticalCenter: parent.verticalCenter
                            width: 12; height: 12
                            app: w.app
                            name: w.othersOpen ? "go-down-symbolic" : "go-next-symbolic"
                            color: Th.widgetMetaFg
                        }
                        WidgetText { anchors.verticalCenter: parent.verticalCenter; role: "meta"; semibold: true; text: "Other games (" + w.others.length + ")"; color: Th.widgetMetaFg }
                    }
                }
                onClicked: w.othersOpen = !w.othersOpen
            }
            StartLink {
                anchors.right: parent.right; anchors.verticalCenter: parent.verticalCenter
                visible: w.canManage && w.upList.length > 0
            }
        }
        Repeater {
            model: w.othersOpen ? w.others : []
            delegate: OtherRow { width: col.width; sv: modelData }
        }
    }

    // ── "Start a game…": every offline server that can start, with its memory size and the LAN chip ──
    component StartLink: Controls.AbstractButton {
        id: sl
        readonly property var startable: w.servers.filter(function (s) { return s.state === "offline" && !s.pending && !s.unavailable })
        implicitWidth: slTxt.implicitWidth; implicitHeight: slTxt.implicitHeight + 4
        enabled: !w.preview && !w.sample && sl.startable.length > 0
        opacity: sl.enabled || w.sample ? 1 : 0.5
        hoverEnabled: true
        focusPolicy: Qt.TabFocus
        contentItem: WidgetText { id: slTxt; role: "meta"; semibold: true; text: "Start a game…"; color: Th.accentBright; font.underline: sl.hovered || sl.visualFocus }
        onClicked: picker.popup(sl, 0, sl.height)
        Controls.Menu {
            id: picker
            modal: false
            Repeater {
                model: sl.startable
                delegate: Controls.MenuItem {
                    id: mi
                    required property var modelData
                    text: LinkGate.plain(mi.modelData.name)
                    icon.name: "media-playback-start-symbolic"
                    contentItem: RowLayout {
                        spacing: 6
                        WidgetText { Layout.fillWidth: true; role: "text"; text: mi.modelData.name; color: Th.widgetFg }
                        WidgetText { role: "num"; text: String(mi.modelData.size); color: Th.widgetMutedFg }
                        LanChip { visible: mi.modelData.outside !== true; brief: true }
                    }
                    onTriggered: power.run("start", mi.modelData.id)
                }
            }
        }
    }

    component LanChip: Rectangle {
        id: lc
        property bool brief: false
        implicitWidth: lcTxt.implicitWidth + 10; implicitHeight: 16
        radius: 4
        color: Th.widgetServersLanBg
        WidgetText { id: lcTxt; anchors.centerIn: parent; role: "chip"; text: lc.brief ? "LAN only" : "Ports closed — LAN only"; color: Th.widgetServersLanFg }
        HoverHandler { id: lcHover }
        Controls.ToolTip.text: "People outside this network can't join — ask the server's owner to open the ports"
        Controls.ToolTip.visible: lcHover.hovered
        Controls.ToolTip.delay: 500
    }

    component Bar: RowLayout {
        id: bar
        property string label: ""
        property real pm: 0
        property string value: ""
        spacing: 4
        WidgetText { role: "label"; text: bar.label; color: Th.widgetMutedFg }
        Rectangle {
            Layout.fillWidth: true
            Layout.minimumWidth: 16
            implicitHeight: 4
            radius: 2
            color: Th.widgetServersBarTrack
            Rectangle {
                height: parent.height; radius: 2
                width: parent.width * Math.max(0, Math.min(1000, bar.pm)) / 1000
                color: bar.pm >= 900 ? Th.widgetServersBarFillHigh : Th.widgetServersBarFill
                Behavior on width { enabled: !!w.hostRef && w.hostRef.durLong > 0; NumberAnimation { duration: w.hostRef ? w.hostRef.durLong : 0; easing.type: Easing.OutCubic } }
            }
        }
        WidgetText { role: "num"; text: bar.value; color: Th.widgetStatsValueFg }
    }

    // ── a server card: padding 8, radius 8, cover 32 × 48 · 10 · the text column; the join row, info, next under both ──
    component Card: Rectangle {
        id: card
        property var sv
        readonly property bool running: !!card.sv && card.sv.state === "running"
        readonly property bool offline: !!card.sv && card.sv.state === "offline"
        readonly property var next: w.nextOf(card.sv)
        property bool shown: false
        height: foot.y + foot.height + 8
        radius: 8
        color: Th.widgetServersCardBg
        border.width: 1
        border.color: Th.widgetServersCardBorder
        Component.onCompleted: if (card.sv && w.isFresh(card.sv.id) && w.hostRef && w.hostRef.durLong > 0) { cardIn.stop(); cardIn.start() }
        ParallelAnimation {
            id: cardIn
            property Translate shift: Translate { y: 0 }
            Component.onCompleted: card.transform = [cardIn.shift]
            NumberAnimation { target: card; property: "opacity"; from: 0; to: 1; duration: w.hostRef ? w.hostRef.durLong : 0; easing.type: Easing.OutCubic }
            NumberAnimation { target: cardIn.shift; property: "y"; from: 4; to: 0; duration: w.hostRef ? w.hostRef.durLong : 0; easing.type: Easing.OutCubic }
        }
        WidgetCover {
            x: 8; y: 8
            width: 32; height: 48
            radius: 4
            source: w.coverOf(card.sv)
            emoji: card.sv && card.sv.emoji ? card.sv.emoji : "🎮"
            fadeDur: w.hostRef ? w.hostRef.durVeryLong : 0
            behind: Qt.tint(Th.widgetBg, Th.widgetServersCardBg)
        }
        Column {
            id: txtCol
            x: 8 + 32 + 10; y: 8
            width: parent.width - x - 8
            spacing: 2
            RowLayout {   // name + the admin buttons
                width: parent.width
                height: 22
                spacing: 2
                WidgetText { Layout.fillWidth: true; role: "strong"; text: card.sv ? card.sv.name : ""; color: Th.widgetFg }
                GameServerIconButton {
                    visible: w.canManage && card.offline && !card.sv.pending
                    app: w.app; iconName: "media-playback-start-symbolic"; label: card.sv ? "Start " + card.sv.name : ""
                    enabled: !w.sample && !card.sv.unavailable
                    onClicked: power.run("start", card.sv.id)
                }
                GameServerIconButton {
                    visible: w.canManage && !(card.offline && !card.sv.pending)
                    app: w.app; iconName: "view-refresh-symbolic"; label: card.sv ? "Restart " + card.sv.name : ""
                    enabled: !w.sample && card.running && !w.busyOf(card.sv)
                    onClicked: power.run("restart", card.sv.id)
                }
                GameServerIconButton {
                    visible: w.canManage && !(card.offline && !card.sv.pending)
                    app: w.app; iconName: "media-playback-stop-symbolic"; label: card.sv ? "Stop " + card.sv.name : ""
                    enabled: !w.sample && !w.busyOf(card.sv)
                    onClicked: power.run("stop", card.sv.id)
                }
            }
            RowLayout {   // the chip line: the dot + the chip text + " · up 2 d 21 h"
                width: parent.width
                spacing: 5
                Rectangle {
                    implicitWidth: 6; implicitHeight: 6; radius: 3
                    color: w.toneColor(card.sv && card.sv.chip ? card.sv.chip.tone : "offline")
                    Behavior on color { enabled: !!w.hostRef && w.hostRef.durLong > 0; ColorAnimation { duration: w.hostRef ? w.hostRef.durLong : 0; easing.type: Easing.OutCubic } }
                }
                WidgetText { role: "meta"; text: card.sv && card.sv.chip ? card.sv.chip.text : ""; color: Th.widgetMetaFg }
                WidgetText {
                    Layout.fillWidth: true
                    visible: card.running && !card.sv.pending
                    role: "num"
                    text: card.sv ? "· " + card.sv.uptime : ""
                    color: Th.widgetMetaFg
                }
                Item { Layout.fillWidth: true; visible: !(card.running && !card.sv.pending) }
            }
            WidgetText {   // who's on: the names (nothing when the chip already says it)
                visible: !card.offline && text !== ""
                width: parent.width
                role: "meta"
                text: card.sv && card.sv.whoLine ? card.sv.whoLine : ""
                color: Th.widgetMetaFg
            }
            RowLayout {   // CPU / RAM
                visible: !card.offline
                width: parent.width
                spacing: 10
                Bar { Layout.fillWidth: true; label: "CPU"; pm: card.sv ? card.sv.cpuPm : 0; value: card.sv ? card.sv.cpuText : "" }
                Bar { Layout.fillWidth: true; label: "RAM"; pm: card.sv ? card.sv.memPm : 0; value: card.sv ? card.sv.memText : "" }
            }
        }
        Column {
            id: foot
            x: 8
            y: Math.max(8 + 48, txtCol.y + txtCol.height) + 6
            width: parent.width - 16
            spacing: 4
            RowLayout {   // the join row: address + copy · password (masked) + show / hide + copy
                width: parent.width
                height: 24
                spacing: 2
                WidgetText { Layout.maximumWidth: parent.width * 0.62; role: "num"; text: card.sv ? card.sv.join.address : ""; color: Th.widgetFg }
                GameServerIconButton { app: w.app; iconName: "edit-copy-symbolic"; label: "Copy address"; enabled: !w.preview && !w.sample; onClicked: w.copy(String(card.sv.join.address)) }
                Item { Layout.fillWidth: true; Layout.minimumWidth: 6 }
                WidgetText {
                    visible: !!card.sv && card.sv.join.password === null
                    role: "meta"; text: "No password"; color: Th.widgetMutedFg
                }
                WidgetText {
                    visible: !!card.sv && card.sv.join.password !== null
                    Layout.maximumWidth: parent.width * 0.4
                    role: "num"
                    text: card.shown && card.sv ? String(card.sv.join.password) : "••••••"
                    color: card.shown ? Th.widgetFg : Th.widgetMetaFg
                    font.letterSpacing: card.shown ? 0 : 1
                }
                GameServerIconButton {
                    visible: !!card.sv && card.sv.join.password !== null
                    app: w.app
                    iconName: card.shown ? "view-hidden-symbolic" : "view-visible-symbolic"
                    label: card.shown ? "Hide password" : "Show password"
                    enabled: !w.preview && !w.sample
                    onClicked: card.shown = !card.shown
                }
                GameServerIconButton {
                    visible: !!card.sv && card.sv.join.password !== null
                    app: w.app; iconName: "edit-copy-symbolic"; label: "Copy password"; enabled: !w.preview && !w.sample
                    onClicked: w.copy(String(card.sv.join.password))
                }
            }
            RowLayout {   // the LAN chip + the info line
                visible: !!card.sv && (card.sv.info.length > 0 || card.sv.outside !== true)
                width: parent.width
                spacing: 6
                LanChip { visible: !!card.sv && card.sv.outside !== true }
                WidgetText {
                    Layout.fillWidth: true
                    visible: !!card.sv && card.sv.info.length > 0
                    role: "meta"
                    text: card.sv ? card.sv.info : ""
                    color: Th.widgetMutedFg
                    wrapMode: w.large ? Text.Wrap : Text.NoWrap
                    maximumLineCount: w.large ? 2 : 1
                    HoverHandler { id: infoHover }
                    Controls.ToolTip.text: card.sv ? card.sv.info : ""
                    Controls.ToolTip.visible: infoHover.hovered && !w.large
                    Controls.ToolTip.delay: 500
                }
            }
            Controls.AbstractButton {   // "Next: Valheim night · Wed 7:00 PM" → the event
                id: nextBtn
                visible: card.next !== null
                width: Math.min(parent.width, nextTxt.implicitWidth)
                height: nextTxt.implicitHeight
                enabled: !w.preview && !w.sample
                hoverEnabled: true
                focusPolicy: Qt.TabFocus
                contentItem: WidgetText {
                    id: nextTxt
                    role: "meta"
                    text: card.next ? "Next: " + card.next.title + " · " + w.dayWord(card.next.start) + " " + w.timeOf(card.next.start) : ""
                    color: nextBtn.hovered ? Th.widgetFg : Th.widgetMetaFg
                    font.underline: nextBtn.hovered || nextBtn.visualFocus
                }
                onClicked: if (w.app && card.next) w.app.openCalendarEvent(card.next.container, card.next.id)
            }
            WidgetText {   // L: the next backup / restart
                visible: w.large && !!card.sv && !!card.sv.next
                role: "meta"
                text: card.sv && card.sv.next ? (card.sv.next.what === "backup" ? "Backup " : "Restart ") + w.dayWord(card.sv.next.at) + " " + w.timeOf(card.sv.next.at) : ""
                color: Th.widgetMutedFg
            }
        }
    }

    // ── an other game: height 26 — the 16 × 24 cover, the name, the chip text, the LAN chip, Start ──
    component OtherRow: Rectangle {
        id: orow
        property var sv
        height: 26
        radius: 6
        color: orHover.hovered ? Th.widgetRowBgHover : "transparent"
        HoverHandler { id: orHover }
        RowLayout {
            anchors.fill: parent
            anchors.leftMargin: 8; anchors.rightMargin: 4
            spacing: 8
            WidgetCover {
                implicitWidth: 16; implicitHeight: 24
                Layout.preferredWidth: 16; Layout.preferredHeight: 24
                radius: 3
                source: w.coverOf(orow.sv)
                emoji: orow.sv && orow.sv.emoji ? orow.sv.emoji : "🎮"
                behind: orHover.hovered ? Qt.tint(Th.widgetBg, Th.widgetRowBgHover) : Th.widgetBg
            }
            WidgetText { Layout.fillWidth: true; role: "text"; text: orow.sv ? orow.sv.name : ""; color: Th.widgetFg }
            WidgetText { role: "meta"; text: orow.sv && orow.sv.chip ? orow.sv.chip.text : ""; color: Th.widgetMutedFg }
            LanChip { visible: !!orow.sv && orow.sv.outside !== true; brief: true }
            GameServerIconButton {
                visible: w.canManage
                app: w.app; iconName: "media-playback-start-symbolic"; label: orow.sv ? "Start " + orow.sv.name : ""
                enabled: !w.sample && !!orow.sv && orow.sv.state === "offline" && !orow.sv.pending && !orow.sv.unavailable
                onClicked: power.run("start", orow.sv.id)
            }
        }
    }

    // ── samples (the gallery's preview, the gates' TINHAT_GAMESERVERS_DEMO; the web's samples, same data) ──
    function dayAt(ms, h, m) { var d = new Date(ms); d.setHours(h, m, 0, 0); return d.getTime() }
    function sampleOf(key) {
        var n = Date.now()
        function srv(id, name, game, app, emoji, limitMB, port, over) {
            var o = { id: id, name: name, game: game, steamAppId: app, emoji: emoji, state: "offline", unavailable: null, uptimeMs: 0,
                      cpu: { used: 0, limit: 300 }, memory: { usedMB: 0, limitMB: limitMB }, players: null,
                      join: { address: "play.example.com:" + port, password: "sample-pw" }, info: "", outside: false, calendar: true, pending: null, next: null }
            for (var k in over) o[k] = over[k]
            return o
        }
        var list = [
            srv("8e6c2f05", "Valheim Vanilla", "Valheim", 892970, "🪓", 8192, 2456, { outside: true,
                info: "Steam only · unlisted. Modded: install the same mods as the server (see the pinned message), or it refuses you." }),
            srv("6c3f9a02", "Project Zomboid B42 Vanilla", "Project Zomboid", 108600, "🧟", 14336, 16261, { outside: true, cpu: { used: 0, limit: 1600 },
                info: "Workshop mods download automatically when you join." }),
            srv("7d4a1b93", "Palworld", "Palworld", 1623730, "🐾", 16384, 8211, { cpu: { used: 0, limit: 1600 }, info: "Unlisted — join with the address (direct connect)." }),
            srv("5b2d7e40", "Satisfactory", "Satisfactory", 526870, "🏭", 8192, 7777, { cpu: { used: 0, limit: 1600 },
                join: { address: "play.example.com:7777", password: null }, info: "Add it in the Server Manager with the address." }),
            srv("a1c0de77", "Abiotic Factor", "Abiotic Factor", 427410, "🧪", 6144, 7779, { cpu: { used: 0, limit: 400 } }),
            srv("e2b4c6d8", "FTB Skies 2", "Minecraft", null, "⛏️", 10240, 25565, { cpu: { used: 0, limit: 600 },
                join: { address: "play.example.com:25565", password: null }, info: "The FTB Skies 2 modpack — install the same version from the FTB or CurseForge app." })
        ]
        function running(sv, players, used, memMB) {
            var o = {}; for (var k in sv) o[k] = sv[k]
            o.state = "running"; o.uptimeMs = 2 * 86400000 + 21 * 3600000 + 17 * 60000
            o.cpu = { used: used, limit: sv.cpu.limit }; o.memory = { usedMB: memMB, limitMB: sv.memory.limitMB }; o.players = players
            o.next = { what: "backup", at: w.dayAt(n + 86400000, 4, 40) }
            return o
        }
        if (key !== "empty" && key !== "empty-member") list[0] = running(list[0], { count: 3, names: ["Darrow", "Sable", "Wren"] }, 17.6, 2028)
        if (key === "two") list[1] = running(list[1], { count: 1, names: ["Rowan"] }, 54.2, 6310)
        var up = list.filter(function (s) { return s.state !== "offline" })
        var st = { v: 1, at: n, space: "!sample:sample.invalid", announceRoom: null,
                   canManage: key === "admin" || key === "two" || key === "empty", cap: 2, running: up.length, memoryBudgetMB: 30720,
                   memoryUsedMB: up.reduce(function (a, s) { return a + s.memory.limitMB }, 0), stale: false, servers: list, job: null }
        var events = [{ container: "!lounge:sample.invalid", id: "ev-valheim", title: "Valheim night", kind: "session",
                        start: w.dayAt(n + 2 * 86400000, 19, 0), end: w.dayAt(n + 2 * 86400000, 22, 0), canceled: false, game: "Valheim", gameId: "steam:892970" }]
        if (key === "loading") return { ready: false, error: "", viewText: "", events: [] }
        if (key === "members-only") return { ready: true, error: "not-member", viewText: "", events: [] }
        if (key === "error") return { ready: true, error: "network", viewText: "", events: [] }
        return { ready: true, error: "", viewText: w.ctl ? String(w.ctl.view(JSON.stringify(st))) : "", events: events }
    }
}
