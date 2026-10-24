// UpNextWidget.qml — widgets: Up next (`upNext`, contract WIDGETS.md §4 + §10.4, WIDGETS-UI.md §4; the web's
// features/widgets/upnext/UpNext.tsx). The calendar events of every joined room that holds a calendar
// (WidgetsController.eventsJson; `upNext(days, 0, now)` = the shared selectUpNext). M = the next event as a hero (date
// + countdown ring, title, when, who's going, the cover or "Game TBD · Pick", RSVP — or Join call when it starts soon
// and its room has a call) + up to two compact rows; L = the `days`-long agenda with day sections; S / collapsed = one
// line. Click a row → the event in the calendar.
//
// One timer only: a timeout at the earliest `nextAt` of the shown events (time::ring_of, ≥ 1 s away); nothing ticks
// otherwise (and nothing at all while hidden). Motion (only while the host animates): ringStep, rowSlide,
// avatarSlide, coverFade, soonEdge.
import QtQuick
import QtQuick.Layouts
import QtQuick.Window
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
    readonly property var wc: w.hostRef ? w.hostRef.widgets : null
    readonly property int days: (w.settings && w.settings.days) ? w.settings.days : 7
    readonly property real body: Kirigami.Theme.defaultFont.pointSize > 0 ? Kirigami.Theme.defaultFont.pointSize : 10

    // ── the clock: one timeout at the earliest nextAt (a ring step / a phase change) ──
    property double now: Date.now()
    readonly property bool ready: !!w.wc && String(w.wc.eventsJson).length > 0
    readonly property var realEvents: {
        var r = w.wc ? w.wc.rev : 0
        var n = w.now
        if (!w.wc || !w.ready) return []
        try { return JSON.parse(String(w.wc.upNext(w.days, 0, n))) } catch (e) { return [] }
    }
    readonly property bool hasData: w.ready && w.realEvents.length > 0
    readonly property bool sample: w.preview && !w.hasData
    readonly property var events: {
        var list = w.sample ? w.sampleEvents : w.realEvents
        return list.map(function (e) { var o = {}; for (var k in e) o[k] = e[k]; o.ring = w.ringOf(e); return o })
    }
    function ringOf(e) {
        if (!w.wc || !e) return ({ phase: "later", minutesLeft: 0, fill: 0, nextAt: null })
        try { return JSON.parse(String(w.wc.ring(e.start, e.end, w.now))) } catch (x) { return ({ phase: "later", fill: 0, nextAt: null }) }
    }
    readonly property double nextAt: {
        var best = 0
        for (var i = 0; i < w.events.length; ++i) {
            var r = w.events[i].ring
            if (r.nextAt !== null && r.nextAt !== undefined && (best === 0 || r.nextAt < best)) best = r.nextAt
        }
        return best
    }
    Timer {
        running: w.nextAt > 0 && !!w.hostRef && w.hostRef.winState !== "hidden" && !w.preview
        interval: Math.max(1000, Math.min(2147483000, w.nextAt - Date.now() + 20))
        repeat: false
        onTriggered: w.now = Date.now()
    }
    Connections {   // back from hidden: catch up at once
        target: w.hostRef
        function onWinStateChanged() { if (w.hostRef.winState !== "hidden") w.now = Date.now() }
    }

    // ── labels (the app's clock setting; "Today", "Tomorrow", "Sat" within 6 days, else "Sat 3 Oct") ──
    readonly property var dows: ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"]
    readonly property var mons: ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"]
    function dayStart(ms) { var d = new Date(ms); return new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime() }
    function dayWord(ms, brief) {   // not short: Qt 6.8 (the Windows build) reserves it
        var days = Math.round((w.dayStart(ms) - w.dayStart(w.now)) / 86400000)
        var d = new Date(ms)
        if (days === 0) return "Today"
        if (days === 1) return brief ? "Tmrw" : "Tomorrow"
        if (days > 1 && days < 7) return brief ? w.dows[d.getDay()] + " " + d.getDate() : w.dows[d.getDay()]
        return brief ? w.dows[d.getDay()] + " " + d.getDate() : w.dows[d.getDay()] + " " + d.getDate() + " " + w.mons[d.getMonth()]
    }
    // the app's clock setting (12 h / 24 h); "System" = the locale's short time WITHOUT seconds (WIDGETS-UI.md §4:
    // "7:30 PM" / "19:30" — the C locale's short format carries ":ss")
    function timeOf(ms) {
        var d = new Date(ms)
        var mode = w.app ? String(w.app.clockMode || "") : ""
        if (mode === "12") return Qt.formatTime(d, "h:mm AP")
        if (mode === "24") return Qt.formatTime(d, "HH:mm")
        return Qt.formatTime(d, String(Qt.locale().timeFormat(Locale.ShortFormat)).replace(/[:.]ss/, ""))
    }
    function until(ms) { return w.wc ? String(w.wc.fmtUntil(ms)) : "" }
    function whenLine(e) {
        if (e.ring.phase === "live") return "Now · until " + w.timeOf(e.end)
        var base = w.dayWord(e.start, false) + " " + w.timeOf(e.start)
        return e.start - w.now < 86400000 ? base + " · " + w.until(e.start - w.now) : base
    }
    readonly property string summaryText: {
        if (w.events.length === 0) return "Nothing planned"
        var e = w.events[0]
        if (e.ring.phase === "live") return e.title + " · now"
        if (e.start - w.now < 86400000) return e.title + " · " + w.until(e.start - w.now)
        return e.title + " · " + w.dayWord(e.start, false) + " " + w.timeOf(e.start)
    }
    readonly property bool live: w.events.length > 0 && (w.events[0].ring.phase === "soon" || w.events[0].ring.phase === "live")
    readonly property Component summaryComponent: Component { WidgetText { role: "meta"; text: w.summaryText; color: Th.widgetSummaryFg } }
    readonly property Component extraComponent: null

    // rowSlide: an event that newly enters the list (after the first fill) — its row: opacity 0→1, y +4→0 (long,
    // OutCubic). `prevKeys` trails `keys` by one event-loop turn, so a row created or re-bound by this change still
    // sees the list before it.
    readonly property var keys: w.events.map(function (e) { return e.container + "/" + e.id })
    property var prevKeys: null
    onKeysChanged: Qt.callLater(function () { w.prevKeys = w.keys })
    Component.onCompleted: { w.prevKeys = w.keys; w._holdServers(w.wantServers) }
    function isFresh(k) { return w.prevKeys !== null && w.prevKeys.indexOf(k) < 0 }

    // ── RSVP (the calendar's toggle: the current answer again clears it) ──
    function answer(e, v) {
        if (w.preview || !w.wc || !e.canWrite) return
        var cur = e.my ? String(e.my.status || "") : ""
        var clear = cur === v
        w.wc.rsvp(e.container, e.id, clear ? "" : v, JSON.stringify(clear ? [] : (e.my && e.my.options ? e.my.options : [])))
    }
    function openEvent(e) { if (!w.preview && w.app) w.app.openCalendarEvent(e.container, e.id) }
    function myOf(e) { return e && e.my ? String(e.my.status || "") : "" }
    // the call room with a live call (WIDGETS-UI.md §4 "Call room", resolved by the controller)
    function joinOf(e) {
        var room = e.callRoom || ""
        var inIt = !!room && !!w.app && w.app.mInCall && w.app.mCallRoomId === room
        return { room: room, active: !!room && (e.callCount || 0) > 0, inIt: inIt }
    }

    // ── the Game servers link (11.1, docs/GAMESERVERS.md §5): the linked server's line in the hero, its dot in compact
    //    rows, "Start server" for admins while the event starts soon. The status is polled only while a shown event
    //    names a game (a non-member's 403 rests the poller). ──
    readonly property var gsCtl: w.app ? w.app.gameServersCtl : null
    readonly property bool wantServers: !w.preview && !!w.gsCtl
        && w.events.slice(0, w.size === "L" ? w.events.length : 3).some(function (e) { return !!e.game || !!e.gameId })
    property bool _gsHeld: false
    onWantServersChanged: w._holdServers(w.wantServers)
    function _holdServers(on) {
        if (!w.gsCtl || on === w._gsHeld) return
        w._gsHeld = on
        if (on) w.gsCtl.acquire(); else w.gsCtl.release()
    }
    Component.onDestruction: w._holdServers(false)
    readonly property string gsText: w.gsCtl ? String(w.gsCtl.statusJson) : ""
    readonly property var gsView: { try { return w.gsText.length > 0 ? JSON.parse(w.gsText) : null } catch (e) { return null } }
    function linkedOf(e) {
        if (!w.gsView || !e || !w.gsCtl || !(e.game || e.gameId)) return null
        var id = String(w.gsCtl.linkServer(w.gsText, JSON.stringify({ game: e.game || "", gameId: e.gameId || "" })))
        if (id.length === 0) return null
        var list = w.gsView.servers || []
        for (var i = 0; i < list.length; ++i) if (list[i].id === id) return list[i]
        return null
    }
    GameServerPower { id: gsPower; app: w.app; viewText: w.gsText; preview: w.preview }

    implicitHeight: !w.ready && !w.sample ? 44 : (w.events.length === 0 ? emptyBand.height : col.implicitHeight)

    // ── not read yet ──
    WidgetText {
        visible: !w.ready && !w.sample
        x: 6; width: parent.width - 12; height: 44
        horizontalAlignment: Text.AlignHCenter
        role: "meta"
        text: "Catching up…"
        color: Th.widgetMutedFg
    }
    // ── empty (M/L) ──
    Item {
        id: emptyBand
        visible: (w.ready || w.sample) && w.events.length === 0
        x: 6; width: parent.width - 12
        height: Math.max(64, emptyCol.implicitHeight + 16)
        Column {
            id: emptyCol
            anchors.centerIn: parent
            spacing: 4
            ThIcon {
                anchors.horizontalCenter: parent.horizontalCenter
                width: 20; height: 20
                app: w.app
                name: "view-calendar-symbolic"
                color: Th.widgetMutedFg
            }
            WidgetText {
                anchors.horizontalCenter: parent.horizontalCenter
                role: "text"
                text: "Nothing planned"
                color: Th.widgetMetaFg
            }
            Controls.AbstractButton {
                id: newBtn
                anchors.horizontalCenter: parent.horizontalCenter
                implicitWidth: newTxt.implicitWidth + 16; implicitHeight: newTxt.implicitHeight + 4
                hoverEnabled: true
                focusPolicy: Qt.TabFocus
                background: Rectangle { radius: 6; color: (newBtn.hovered || newBtn.visualFocus) ? Th.widgetUpNextTbdBgHover : "transparent" }
                contentItem: WidgetText { id: newTxt; role: "meta"; semibold: true; text: "+ New event"; color: Th.widgetUpNextTbdFg; horizontalAlignment: Text.AlignHCenter }
                onClicked: if (w.app && !w.preview) w.app.openCalendarNew()
            }
        }
    }

    Column {
        id: col
        visible: (w.ready || w.sample) && w.events.length > 0
        x: 6; width: parent.width - 12
        spacing: 0
        Hero {
            width: col.width
            ev: w.events.length > 0 ? w.events[0] : null
        }
        // divider: 1 px, margin 4 8
        Item {
            width: col.width; height: 9
            visible: w.events.length > 1 && (w.size === "L" || w.preview || true)
            Rectangle { x: 8; y: 4; width: parent.width - 16; height: 1; color: Th.widgetDivider }
        }
        // M: events 2–3 as compact rows · L: every event, with a day label where the day changes
        Repeater {
            model: Math.max(0, Math.min(w.events.length, w.size === "L" && !w.preview ? w.events.length : 3) - 1)
            delegate: Column {
                id: rowBox
                required property int index
                readonly property var ev: w.events[index + 1]
                readonly property string day: rowBox.ev ? w.dayWord(rowBox.ev.start, false).toUpperCase() : ""
                readonly property string prevDay: index > 0 && w.events[index] ? w.dayWord(w.events[index].start, false).toUpperCase() : ""
                width: col.width
                Item {   // L: "TOMORROW", "WED", "SAT 3 OCT" (20 px, text on the bottom, padding 0 8 2)
                    width: parent.width; height: 20
                    visible: w.size === "L" && !w.preview && (index === 0 || rowBox.day !== rowBox.prevDay)
                    WidgetText {
                        x: 8; y: parent.height - 2 - height
                        role: "label"
                        text: rowBox.day
                        color: Th.widgetMutedFg
                    }
                }
                Compact { width: parent.width; ev: rowBox.ev; alwaysButtons: w.size === "L" && !w.preview }
            }
        }
    }

    // ── the hero: padding 8 8 8 10, radius 8; ring 40 · 10 · text column · 10 · cover 32 × 48 ──
    component Hero: Rectangle {
        id: hero
        property var ev
        readonly property string key: hero.ev ? hero.ev.container + "/" + hero.ev.id : ""
        onKeyChanged: if (w.isFresh(hero.key)) heroIn.play()
        Component.onCompleted: if (w.isFresh(hero.key)) heroIn.play()
        SlideIn { id: heroIn; target: hero; dy: 4 }
        readonly property var r: hero.ev ? hero.ev.ring : ({ phase: "later", fill: 0 })
        readonly property bool soon: hero.r.phase === "soon" || hero.r.phase === "live"
        readonly property var join: hero.ev ? w.joinOf(hero.ev) : ({ room: "", active: false, inIt: false })
        readonly property bool hasCover: !!hero.ev && !!hero.ev.cover && hero.ev.cover.length > 0
        readonly property bool tbd: !!hero.ev && !(hero.ev.game && hero.ev.game.length > 0) && hero.ev.kind !== "focus" && !hero.ev.canceled
        readonly property var linked: w.linkedOf(hero.ev)
        readonly property bool startServer: hero.soon && !!hero.linked && !!w.gsView && w.gsView.canManage === true
            && hero.linked.state === "offline" && !hero.linked.pending
        height: textCol.implicitHeight + 16
        radius: 8
        color: hero.soon ? Th.widgetUpNextSoonBg : (heroMa.containsMouse ? Th.widgetRowBgHover : "transparent")
        Behavior on color { enabled: !!w.hostRef && w.hostRef.dur > 0; ColorAnimation { duration: w.hostRef ? w.hostRef.dur : 0; easing.type: Easing.OutCubic } }
        MouseArea { id: heroMa; anchors.fill: parent; hoverEnabled: true; enabled: !w.preview; cursorShape: Qt.PointingHandCursor; onClicked: w.openEvent(hero.ev) }
        // soonEdge: the 3 px edge (x 0, top 8, bottom 8) fades in with the wash (dur)
        Rectangle {
            x: 0; y: 8; width: 3; height: parent.height - 16; radius: 2
            color: Th.widgetUpNextSoonEdge
            opacity: hero.soon ? 1 : 0
            Behavior on opacity { enabled: !!w.hostRef && w.hostRef.dur > 0; NumberAnimation { duration: w.hostRef ? w.hostRef.dur : 0; easing.type: Easing.OutCubic } }
        }
        // date chip + countdown ring (weekday and day number centred as one column: 0.7 + 1 px + 1.1 × body)
        Item {
            id: chip
            x: 10; y: 8
            width: 40; height: 40
            readonly property real pxPerPt: Screen.logicalPixelDensity > 0 ? Screen.logicalPixelDensity * 25.4 / 72 : 96 / 72
            readonly property real dowPx: w.body * 0.7 * chip.pxPerPt
            readonly property real domPx: w.body * 1.1 * chip.pxPerPt
            readonly property real topY: (40 - (chip.dowPx + 1 + chip.domPx)) / 2
            WidgetRing {
                anchors.fill: parent
                diameter: 40; stroke: 3
                fill: hero.r.fill || 0
                live: hero.r.phase === "live"
                dur: w.hostRef ? w.hostRef.durLong : 0
            }
            WidgetText {
                anchors.horizontalCenter: parent.horizontalCenter
                y: chip.topY + chip.dowPx / 2 - height / 2
                role: "dow"
                text: hero.ev ? w.dows[new Date(hero.ev.start).getDay()] : ""
                color: Th.widgetUpNextDateLabelFg
            }
            WidgetText {
                anchors.horizontalCenter: parent.horizontalCenter
                y: chip.topY + chip.dowPx + 1 + chip.domPx / 2 - height / 2
                role: "date"
                text: hero.ev ? String(new Date(hero.ev.start).getDate()) : ""
                color: Th.widgetUpNextDateFg
            }
        }
        // the cover (the Reel picked a catalogue game): 32 × 48, radius 4; coverFade on a change
        WidgetCover {
            visible: hero.hasCover
            anchors.right: parent.right; anchors.rightMargin: 8
            y: 8
            width: 32; height: 48
            radius: 4
            source: hero.hasCover ? hero.ev.cover : ""
            fadeDur: w.hostRef ? w.hostRef.durVeryLong : 0
            behind: hero.soon ? Qt.tint(Th.widgetBg, Th.widgetUpNextSoonBg) : (heroMa.containsMouse ? Qt.tint(Th.widgetBg, Th.widgetRowBgHover) : Th.widgetBg)
        }
        Column {
            id: textCol
            x: 10 + 40 + 10; y: 8
            width: parent.width - x - 8 - (hero.hasCover ? 32 + 10 : 0)
            spacing: 0
            WidgetText {
                width: parent.width
                role: "strong"
                text: hero.ev ? hero.ev.title : ""
                color: Th.widgetFg
            }
            WidgetText {
                width: parent.width
                role: "num"
                text: hero.ev ? w.whenLine(hero.ev) : ""
                color: hero.r.phase === "live" ? Th.widgetUpNextLiveFg : Th.widgetMetaFg
            }
            GameServerLine {   // the Game servers link: the linked server's state + join address
                visible: !!hero.linked
                width: parent.width
                app: w.app
                sv: hero.linked
                preview: w.preview
            }
            // line 3 (margin-top 6, height 20): who's going (yes, then maybe) · "Game TBD · Pick"
            Item {
                width: parent.width; height: 6 + 20
                People { y: 6; height: 20; ev: hero.ev; soon: hero.soon }
                Controls.AbstractButton {
                    id: tbdBtn
                    visible: hero.tbd
                    anchors.right: parent.right
                    y: 6
                    implicitHeight: 20
                    implicitWidth: tbdText.implicitWidth + 16
                    hoverEnabled: true
                    focusPolicy: Qt.TabFocus
                    enabled: !w.preview
                    background: Rectangle {
                        radius: 10
                        color: (tbdBtn.hovered || tbdBtn.visualFocus) ? Th.widgetUpNextTbdBgHover : "transparent"
                        border.width: 1
                        border.color: Th.widgetUpNextTbdBorder
                    }
                    contentItem: WidgetText { id: tbdText; role: "meta"; semibold: true; text: "Game TBD · Pick"; color: Th.widgetUpNextTbdFg; horizontalAlignment: Text.AlignHCenter }
                    onClicked: if (w.app && hero.ev) w.app.openGameReelFor(hero.ev.container, hero.ev.id)
                }
            }
            // line 4 (margin-top 6): RSVP — or Join call / "You're in the call" + the three answers icon-only
            Item {
                width: parent.width; height: 6 + 24
                RowLayout {
                    y: 6
                    width: parent.width
                    height: 24
                    spacing: 4
                    ThButton {   // the Game servers link: the linked server is offline, an admin starts it
                        visible: hero.startServer
                        Layout.fillWidth: true
                        Layout.preferredHeight: 24
                        app: w.app
                        highlighted: true
                        text: "Start server"
                        icon.name: "media-playback-start-symbolic"
                        implicitHeight: 24
                        leftPadding: 8; rightPadding: 8
                        font.pointSize: w.body * 0.85
                        enabled: !w.preview
                        onClicked: gsPower.run("start", hero.linked.id)
                    }
                    ThButton {
                        visible: hero.soon && hero.join.active && !hero.join.inIt && !hero.startServer
                        Layout.fillWidth: true
                        Layout.preferredHeight: 24
                        app: w.app
                        highlighted: true
                        text: "Join call"
                        icon.name: "call-start-symbolic"
                        implicitHeight: 24
                        leftPadding: 8; rightPadding: 8
                        font.pointSize: w.body * 0.85
                        enabled: !w.preview
                        onClicked: if (w.app) { w.app.openRoomId(hero.join.room); w.app.callctl.switchCall(hero.join.room) }
                    }
                    WidgetText {
                        visible: hero.soon && hero.join.inIt && !hero.startServer
                        Layout.fillWidth: true
                        role: "meta"
                        text: "You're in the call"
                        color: Th.widgetMutedFg
                    }
                    Rsvp {
                        Layout.fillWidth: !(hero.soon && (hero.join.active || hero.join.inIt)) && !hero.startServer
                        Layout.preferredHeight: 24
                        ev: hero.ev
                        iconOnly: (hero.soon && (hero.join.active || hero.join.inIt)) || hero.startServer
                    }
                }
            }
        }
    }

    // ── who's going · RSVP · ✓ ? ✕ — widgets W2 moved them into WidgetPeople / WidgetRsvp / WidgetAnswerMark (unchanged) so
    //    Game night shows the same pieces; `host: w` hands them this widget's app, motion steps, preview and answer() ──
    component People: WidgetPeople { host: w }
    component Rsvp: WidgetRsvp { host: w }
    component AnswerMark: WidgetAnswerMark { app: w.app }

    // ── a compact row (28, padding 0 8, gap 8): ring 16 · day 52 · title · time · my answer 14 (hover / L: the three
    //    answers as 20 × 20 buttons instead of the time + glyph; L keeps the time) ──
    component Compact: Rectangle {
        id: cr
        property var ev
        readonly property string key: cr.ev ? cr.ev.container + "/" + cr.ev.id : ""
        onKeyChanged: if (w.isFresh(cr.key)) crIn.play()
        Component.onCompleted: if (w.isFresh(cr.key)) crIn.play()
        SlideIn { id: crIn; target: cr; dy: 4 }
        property bool alwaysButtons: false
        readonly property var r: cr.ev ? cr.ev.ring : ({ phase: "later", fill: 0 })
        readonly property bool soon: cr.r.phase === "soon" || cr.r.phase === "live"
        readonly property string my: w.myOf(cr.ev)
        readonly property bool hot: crHover.hovered && !w.preview
        readonly property bool showButtons: cr.alwaysButtons || cr.hot
        readonly property bool showTime: !cr.hot
        readonly property bool showGlyph: !cr.hot && !cr.alwaysButtons
        height: 28
        radius: 6
        color: cr.soon ? Th.widgetUpNextSoonBg : (cr.hot ? Th.widgetRowBgHover : "transparent")
        HoverHandler { id: crHover; enabled: !w.preview }
        MouseArea { anchors.fill: parent; enabled: !w.preview; cursorShape: Qt.PointingHandCursor; onClicked: w.openEvent(cr.ev) }
        Rectangle { visible: cr.soon; x: 0; y: 6; width: 3; height: parent.height - 12; radius: 2; color: Th.widgetUpNextSoonEdge }
        RowLayout {
            anchors.fill: parent
            anchors.leftMargin: 8; anchors.rightMargin: 8
            spacing: 8
            WidgetRing {
                Layout.preferredWidth: 16; Layout.preferredHeight: 16
                diameter: 16; stroke: 2
                fill: cr.r.fill || 0
                live: cr.r.phase === "live"
                dur: w.hostRef ? w.hostRef.durLong : 0
            }
            WidgetText {
                Layout.preferredWidth: 52
                role: "meta"
                text: cr.ev ? w.dayWord(cr.ev.start, true) : ""
                color: Th.widgetMetaFg
            }
            Rectangle {   // the Game servers link: the linked server's state dot
                readonly property var linked: w.linkedOf(cr.ev)
                readonly property string tone: linked && linked.chip ? String(linked.chip.tone) : "offline"
                visible: !!linked
                Layout.preferredWidth: 6; Layout.preferredHeight: 6
                radius: 3
                color: tone === "online" ? Th.widgetServersStateOnline : (tone === "busy" ? Th.widgetServersStateBusy : Th.widgetServersStateOffline)
            }
            WidgetText {
                Layout.fillWidth: true
                role: "text"
                text: cr.ev ? cr.ev.title : ""
                color: Th.widgetFg
            }
            WidgetText {
                visible: cr.showTime
                role: "num"
                text: !cr.ev ? "" : (cr.r.phase === "live" ? "now" : w.timeOf(cr.ev.start))
                color: Th.widgetMetaFg
            }
            Item {
                visible: cr.showGlyph
                Layout.preferredWidth: 14; Layout.preferredHeight: 14
                AnswerMark {
                    anchors.centerIn: parent
                    visible: cr.my === "yes" || cr.my === "maybe" || cr.my === "no"
                    v: cr.my
                    px: 12
                    ink: cr.my === "yes" ? Th.widgetUpNextRsvpYesFgChecked : (cr.my === "maybe" ? Th.widgetUpNextRsvpMaybeFgChecked : Th.widgetUpNextRsvpNoFgChecked)
                }
            }
            Rsvp {
                visible: cr.showButtons
                mini: true
                ev: cr.ev
            }
        }
    }

    // a one-shot enter motion: opacity 0→1 and y +dy→0 (long 200 ms, OutCubic) through a Translate — never a
    // layout change; nothing when the host does not animate
    component SlideIn: ParallelAnimation {
        id: si
        property Item target
        property real dy: 4
        property Translate shift: Translate { y: 0 }
        Component.onCompleted: if (si.target) si.target.transform = [si.shift]
        NumberAnimation { target: si.target; property: "opacity"; from: 0; to: 1; duration: w.hostRef ? w.hostRef.durLong : 0; easing.type: Easing.OutCubic }
        NumberAnimation { target: si.shift; property: "y"; from: si.dy; to: 0; duration: w.hostRef ? w.hostRef.durLong : 0; easing.type: Easing.OutCubic }
        function play() { if (w.hostRef && w.hostRef.durLong > 0) { si.stop(); si.start() } }
    }

    // ── the sample state (the gallery's preview without real data; the web's sample) ──
    readonly property var sampleEvents: {
        var n = w.now
        var d = new Date(n); d.setHours(19, 30, 0, 0)
        var tonight = Math.max(n + 2 * 3600000 + 10 * 60000, d.getTime())
        function p(mxid, name) { return { mxid: mxid, name: name, avatar: "" } }
        function mk(i, title, start, going, my, tbd) {
            return { container: "sample", id: "ev-" + i, title: title, kind: "session", start: start, end: start + 3 * 3600000,
                     game: tbd ? "" : "Valheim", cover: "", rsvp: { yes: going, maybe: [], no: [] }, my: { status: my, options: [] },
                     canWrite: false, callRoom: "", callCount: 0 }
        }
        return [
            mk(1, "Game night", tonight, [p("@ana:x", "Ana"), p("@bo:x", "Bo"), p("@cy:x", "Cy")], "yes", true),
            mk(2, "Boss run", tonight + 86400000 + 30 * 60000, [p("@bo:x", "Bo")], "maybe", false),
            mk(3, "Valheim night", tonight + 3 * 86400000, [], "", false)
        ]
    }
}
