// GameNightWidget.qml — widgets W2: Game night (`gameNight`, contract WIDGETS.md §23, WIDGETS-UI.md §23; the web's
// features/widgets/gamenight/GameNight.tsx). The next game night as the hero — the Game Reel's sessions (calendar kind
// "session") of every joined space, read through Up next's own WidgetsController.upNext: the game's cover (or the "Game
// TBD" tile + "Pick / Spin" → the Game Reel for that night), the countdown (one timer at the next minute boundary, never
// while hidden), who's going (Up next's faces, WidgetPeople: a new one slides in), your RSVP (Up next's buttons,
// WidgetRsvp: the calendar's write), your Reel credit (GameNightController.loadCredit — the Reel's own computation,
// read-only) and, when the picked game has a hosted game server, its state + join address (GameServerLine). L adds the next
// two nights; S / collapsed = "Valheim · in 2 h" after the title. Up next lists every event; this is the game-night hero.
// Samples = the web's (TINHAT_WIDGETS_SCENARIO=gameNight=<key>). Motion: avatarSlide, coverFade, rowSlide.
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
    readonly property var wc: w.hostRef ? w.hostRef.widgets : null
    readonly property string scenario: w.wc ? String(w.wc.scenarioOf("gameNight") || "") : ""
    GameNightController { id: gnCtl }

    // ── the clock: one timeout at the next minute boundary of the hero's countdown (Up next's ring rule) ──
    property double now: Date.now()
    readonly property bool ready: w.sample || (!!w.wc && String(w.wc.eventsJson).length > 0)
    readonly property var realNights: {
        var r = w.wc ? w.wc.rev : 0
        var n = w.now
        if (!w.wc || String(w.wc.eventsJson).length === 0) return []
        var all = []
        try { all = JSON.parse(String(w.wc.upNext(14, 0, n))) } catch (e) { all = [] }
        var picked = []
        try { picked = JSON.parse(String(gnCtl.selectNights(JSON.stringify(all.map(function (e) { return { container: e.container, id: e.id, kind: e.kind, start: e.start, end: e.end, canceled: e.canceled === true } })), n, 3))) } catch (e2) { picked = [] }
        return picked.map(function (p) {
            for (var i = 0; i < all.length; ++i) if (all[i].container === p.container && all[i].id === p.id) return all[i]
            return null
        }).filter(function (e) { return !!e })
    }
    readonly property bool hasData: w.realNights.length > 0
    readonly property bool sample: w.scenario.length > 0 || (w.preview && !w.hasData)
    readonly property var sampleSet: w.sample ? w.sampleModel(w.scenario.length > 0 ? w.scenario : "preview") : null
    readonly property double clock: w.sample ? w.sampleSet.now : w.now
    readonly property var nights: w.sample ? w.sampleSet.nights : w.realNights
    readonly property var hero: w.nights.length > 0 ? w.nights[0] : null
    // no game yet (Up next's tbd for a game night): the TBD tile + Pick / Spin
    readonly property bool heroTbd: !!w.hero && !(w.hero.game && String(w.hero.game).length > 0)
    function ringOf(e) {
        if (!w.wc || !e) return ({ phase: "later", fill: 0, nextAt: null })
        try { return JSON.parse(String(w.wc.ring(e.start, e.end, w.clock))) } catch (x) { return ({ phase: "later", fill: 0, nextAt: null }) }
    }
    readonly property var heroRing: w.ringOf(w.hero)
    Timer {
        running: !w.sample && !!w.heroRing.nextAt && !!w.hostRef && w.hostRef.winState !== "hidden" && !w.preview
        interval: Math.max(1000, Math.min(2147483000, (w.heroRing.nextAt || 0) - Date.now() + 20))
        repeat: false
        onTriggered: w.now = Date.now()
    }
    // back from hidden: catch up at once and read the credit again (the web's useCredit); focus changes only tick the clock
    property string _lastWin: ""   // set imperatively (a binding could update before the handler reads it)
    Connections {
        target: w.hostRef
        function onWinStateChanged() {
            var was = w._lastWin
            w._lastWin = String(w.hostRef.winState)
            if (w.hostRef.winState === "hidden") return
            w.now = Date.now()
            if (was === "hidden") w.readCredit()
        }
    }

    // ── my Reel credit for the hero: read when the hero changes, then every 10 minutes while not hidden ──
    readonly property string heroKey: w.hero && !w.sample ? w.hero.container + "/" + w.hero.id : ""
    function readCredit() {
        if (w.preview || w.sample || !w.hero || (w.hostRef && w.hostRef.winState === "hidden")) return
        gnCtl.loadCredit(w.hero.container, w.hero.id)
    }
    onHeroKeyChanged: w.readCredit()
    Timer { interval: 600000; repeat: true; running: w.heroKey.length > 0 && !!w.hostRef && w.hostRef.winState !== "hidden"; onTriggered: w.readCredit() }
    readonly property string creditLine: {
        if (w.sample) return w.sampleSet.creditLine
        var t = String(gnCtl.creditJson)
        var v = null
        try { v = t.length > 0 ? JSON.parse(t) : null } catch (e) { v = null }
        return v && v.key === w.heroKey ? String(gnCtl.creditText(t)) : ""
    }

    // ── labels (Up next's: the app's clock setting; "Today", "Tomorrow", "Sat", "Sat 3 Oct"; compact "Tmrw", "Sun 28") ──
    readonly property var dows: ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"]
    readonly property var mons: ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"]
    function dayStart(ms) { var d = new Date(ms); return new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime() }
    function dayWord(ms, brief) {
        var days = Math.round((w.dayStart(ms) - w.dayStart(w.clock)) / 86400000)
        var d = new Date(ms)
        if (days === 0) return "Today"
        if (days === 1) return brief ? "Tmrw" : "Tomorrow"
        if (days > 1 && days < 7) return brief ? w.dows[d.getDay()] + " " + d.getDate() : w.dows[d.getDay()]
        return brief ? w.dows[d.getDay()] + " " + d.getDate() : w.dows[d.getDay()] + " " + d.getDate() + " " + w.mons[d.getMonth()]
    }
    function timeOf(ms) {
        var d = new Date(ms)
        var mode = w.app ? String(w.app.clockMode || "") : ""
        if (mode === "12") return Qt.formatTime(d, "h:mm AP")
        if (mode === "24") return Qt.formatTime(d, "HH:mm")
        return Qt.formatTime(d, String(Qt.locale().timeFormat(Locale.ShortFormat)).replace(/[:.]ss/, ""))
    }
    function until(ms) { return w.wc ? String(w.wc.fmtUntil(ms)) : "" }
    // Up next's hero "when" line
    function whenLine(e) {
        var r = w.ringOf(e)
        if (r.phase === "live") return "Now · until " + w.timeOf(e.end)
        var base = w.dayWord(e.start, false) + " " + w.timeOf(e.start)
        return e.start - w.clock < 86400000 ? base + " · " + w.until(e.start - w.clock) : base
    }
    readonly property string whenText: w.hero ? String(gnCtl.nightWhen(w.hero.start, w.hero.end, w.clock, w.dayWord(w.hero.start, false) + " " + w.timeOf(w.hero.start))) : ""
    readonly property string summaryText: !w.ready ? "Catching up…" : String(gnCtl.nightSummary(!!w.hero, w.hero ? String(w.hero.game || "") : "", w.whenText))
    readonly property bool live: !!w.hero && (w.heroRing.phase === "soon" || w.heroRing.phase === "live")
    readonly property Component summaryComponent: Component { WidgetHeaderText { text: w.summaryText } }
    readonly property Component extraComponent: Component { WidgetHeaderText { text: w.whenText } }

    // ── what the shared pieces ask of their host (WidgetPeople / WidgetRsvp): the calendar's RSVP toggle ──
    function answer(e, v) {
        if (w.preview || w.sample || !w.wc || !e.canWrite) return
        var cur = e.my ? String(e.my.status || "") : ""
        var clear = cur === v
        w.wc.rsvp(e.container, e.id, clear ? "" : v, JSON.stringify(clear ? [] : (e.my && e.my.options ? e.my.options : [])))
    }
    function myOf(e) { return e && e.my ? String(e.my.status || "") : "" }
    function openEvent(e) { if (!w.preview && !w.sample && w.app && e) w.app.openCalendarEvent(e.container, e.id) }
    function openReel(e) { if (!w.preview && !w.sample && w.app && e) w.app.openGameReelFor(e.container, e.id) }

    // ── the Game servers link (docs/GAMESERVERS.md §5): polled only while the hero names a game ──
    readonly property var gsCtl: w.app ? w.app.gameServersCtl : null
    readonly property bool wantServers: !w.preview && !w.sample && !!w.gsCtl && !!w.hero && (!!w.hero.game || !!w.hero.gameId)
    property bool _gsHeld: false
    onWantServersChanged: w._holdServers(w.wantServers)
    Component.onCompleted: { w._holdServers(w.wantServers); w.prevKeys = w.keys; w._lastWin = w.hostRef ? String(w.hostRef.winState) : "" }
    Component.onDestruction: w._holdServers(false)
    function _holdServers(on) {
        if (!w.gsCtl || on === w._gsHeld) return
        w._gsHeld = on
        if (on) w.gsCtl.acquire(); else w.gsCtl.release()
    }
    readonly property string gsText: w.gsCtl ? String(w.gsCtl.statusJson) : ""
    readonly property var linked: {
        if (!w.wantServers || w.gsText.length === 0) return null
        var id = String(w.gsCtl.linkServer(w.gsText, JSON.stringify({ game: w.hero.game || "", gameId: w.hero.gameId || "" })))
        if (id.length === 0) return null
        var v = null
        try { v = JSON.parse(w.gsText) } catch (e) { return null }
        var list = v.servers || []
        for (var i = 0; i < list.length; ++i) if (list[i].id === id) return list[i]
        return null
    }

    // rowSlide: a night that newly enters the L list (after the first fill)
    readonly property var keys: w.nights.map(function (e) { return e.container + "/" + e.id })
    property var prevKeys: null
    onKeysChanged: Qt.callLater(function () { w.prevKeys = w.keys })
    function isFresh(k) { return w.prevKeys !== null && w.prevKeys.indexOf(k) < 0 }

    implicitHeight: !w.ready ? 44 : (w.nights.length === 0 ? emptyBand.height : col.implicitHeight)

    WidgetText {
        visible: !w.ready
        width: parent.width; height: 44
        horizontalAlignment: Text.AlignHCenter
        role: "meta"; text: "Catching up…"; color: Th.widgetMutedFg
    }
    // ── empty: "No game night planned" + New game night (the calendar's new-event path) ──
    Item {
        id: emptyBand
        visible: w.ready && w.nights.length === 0
        width: parent.width; height: 64
        Column {
            anchors.centerIn: parent
            spacing: 4
            WidgetText { anchors.horizontalCenter: parent.horizontalCenter; role: "text"; text: "No game night planned"; color: Th.widgetMetaFg }
            Controls.AbstractButton {
                id: newBtn
                anchors.horizontalCenter: parent.horizontalCenter
                implicitWidth: newTxt.implicitWidth; implicitHeight: newTxt.implicitHeight
                enabled: !w.preview && !w.sample
                opacity: w.preview ? 0.5 : 1   // the web's disabled button in the gallery's preview; a gate sample only swallows the click
                hoverEnabled: true
                focusPolicy: Qt.TabFocus
                contentItem: WidgetText { id: newTxt; role: "meta"; semibold: true; text: "New game night"; color: Th.accentBright; font.underline: newBtn.hovered || newBtn.visualFocus }
                onClicked: if (w.app) w.app.openCalendarNew()
            }
        }
    }

    Column {
        id: col
        visible: w.ready && w.nights.length > 0
        x: 6; width: parent.width - 12
        topPadding: 2   // the web's hero box: padding 2 2 0, gap 6 (hero · RSVP · credit)
        spacing: 6
        // ── the hero (padding 6, radius 8, hover wash): cover 48 × 72 · gap 10 · the text column ──
        Rectangle {
            id: heroBox
            x: 2; width: parent.width - 4
            height: Math.max(72, heroText.implicitHeight) + 12
            radius: 8
            color: heroMa.containsMouse && !w.preview ? Th.widgetRowBgHover : "transparent"
            MouseArea { id: heroMa; anchors.fill: parent; hoverEnabled: true; enabled: !w.preview; cursorShape: Qt.PointingHandCursor; onClicked: w.openEvent(w.hero) }
            WidgetCover {
                visible: !!w.hero && String(w.hero.cover || "").length > 0
                x: 6; y: 6
                width: 48; height: 72
                radius: 4
                source: visible ? w.hero.cover : ""
                fadeDur: w.hostRef ? w.hostRef.durVeryLong : 0
                behind: heroMa.containsMouse && !w.preview ? Qt.tint(Th.widgetBg, Th.widgetRowBgHover) : Th.widgetBg
            }
            // a named game without cover art (the calendar's game name, no Reel pick): the plain cover box
            Rectangle {
                visible: !!w.hero && String(w.hero.cover || "").length === 0 && !w.heroTbd
                x: 6; y: 6
                width: 48; height: 72
                radius: 4
                color: Th.widgetCoverBg
                border.width: 1; border.color: Th.widgetCoverBorder
            }
            // "Game TBD": the tile opens the Game Reel for this night
            Controls.AbstractButton {
                id: tbdTile
                visible: !!w.hero && String(w.hero.cover || "").length === 0 && w.heroTbd
                x: 6; y: 6
                width: 48; height: 72
                hoverEnabled: true
                enabled: !w.preview
                focusPolicy: Qt.TabFocus
                Controls.ToolTip.text: "Pick tonight's game — spin the Game Reel"
                Controls.ToolTip.visible: hovered
                Controls.ToolTip.delay: 500
                background: Rectangle {
                    radius: 4
                    color: tbdTile.hovered || tbdTile.visualFocus ? Th.widgetUpNextTbdBgHover : Th.widgetCoverBg
                    border.width: 1; border.color: Th.widgetCoverBorder
                }
                contentItem: Item {
                    Column {
                        anchors.centerIn: parent
                        spacing: 4
                        Text { anchors.horizontalCenter: parent.horizontalCenter; text: "🎲"; font.pixelSize: 18; color: Th.widgetUpNextTbdFg }
                        WidgetText { anchors.horizontalCenter: parent.horizontalCenter; horizontalAlignment: Text.AlignHCenter; role: "chip"; text: "Game\nTBD"; maximumLineCount: 2; wrapMode: Text.Wrap; lineHeight: 0.85; color: Th.widgetUpNextTbdFg }
                    }
                }
                onClicked: w.openReel(w.hero)
            }
            Column {
                id: heroText
                x: 6 + 48 + 10; y: 6
                width: parent.width - x - 6
                WidgetText { width: parent.width; role: "strong"; text: w.hero ? String(w.hero.title) : ""; color: Th.widgetFg }
                WidgetText { width: parent.width; role: "meta"; text: w.hero ? String(w.hero.game || "Game TBD") : ""; color: Th.widgetMetaFg }
                WidgetText { width: parent.width; role: "num"; text: w.hero ? w.whenLine(w.hero) : ""; color: w.heroRing.phase === "live" ? Th.widgetUpNextLiveFg : Th.widgetMetaFg }
                // who's going (Up next's faces) · Pick / Spin while no game
                Item {
                    width: parent.width; height: 4 + 20
                    WidgetPeople { host: w; y: 4; height: 20; ev: w.hero; soon: false }
                    Controls.AbstractButton {
                        id: pickBtn
                        visible: w.heroTbd
                        anchors.right: parent.right
                        y: 4
                        implicitHeight: 20
                        implicitWidth: pickTxt.implicitWidth + 16
                        hoverEnabled: true
                        focusPolicy: Qt.TabFocus
                        enabled: !w.preview
                        background: Rectangle {
                            radius: 10
                            color: (pickBtn.hovered || pickBtn.visualFocus) ? Th.widgetUpNextTbdBgHover : "transparent"
                            border.width: 1
                            border.color: Th.widgetUpNextTbdBorder
                        }
                        contentItem: WidgetText { id: pickTxt; role: "meta"; semibold: true; text: "Pick / Spin"; color: Th.widgetUpNextTbdFg; horizontalAlignment: Text.AlignHCenter }
                        onClicked: w.openReel(w.hero)
                    }
                }
                Item { visible: !!w.linked; width: 1; height: visible ? 4 : 0 }
                GameServerLine {   // the Game servers link: the linked server's state + join address
                    visible: !!w.linked
                    width: parent.width
                    app: w.app
                    sv: w.linked
                    preview: w.preview
                }
            }
        }
        // your RSVP (Up next's buttons, full width)
        WidgetRsvp { host: w; x: 8; width: parent.width - 16; height: 24; ev: w.hero }
        Item {   // the credit line (padding 0 8 2 inside the hero box)
            visible: w.creditLine.length > 0
            width: parent.width
            height: creditTxt.implicitHeight + 2
            WidgetText {
                id: creditTxt
                x: 10; width: parent.width - 20
                role: "meta"
                text: w.creditLine
                color: Th.widgetMutedFg
                Controls.ToolTip.text: "Credit raises your favourite's odds on the Game Reel after it loses."
                HoverHandler { id: creditHover }
                Controls.ToolTip.visible: creditHover.hovered
                Controls.ToolTip.delay: 500
            }
        }
        // ── L: a divider (margin 6 8 4) + the next two nights (28, padding 0 8, radius 6, no gap): day · title · game ·
        //    time · my answer ──
        Column {
            visible: w.size === "L" && !w.preview && w.nights.length > 1
            width: parent.width
            Item { width: parent.width; height: 1 + 4; Rectangle { x: 8; width: parent.width - 16; height: 1; color: Th.widgetDivider } }
            Repeater {
                model: w.size === "L" && !w.preview ? Math.max(0, Math.min(3, w.nights.length) - 1) : 0
                delegate: NightRow { required property int index; width: col.width; ev: w.nights[index + 1] }
            }
        }
    }

    component NightRow: Rectangle {
        id: nr
        property var ev
        readonly property string key: nr.ev ? nr.ev.container + "/" + nr.ev.id : ""
        onKeyChanged: if (w.isFresh(nr.key)) nrIn.play()
        Component.onCompleted: if (w.isFresh(nr.key)) nrIn.play()
        SlideIn { id: nrIn; target: nr }
        height: 28
        radius: 6
        color: nrMa.containsMouse ? Th.widgetRowBgHover : "transparent"
        MouseArea { id: nrMa; anchors.fill: parent; hoverEnabled: true; cursorShape: Qt.PointingHandCursor; onClicked: w.openEvent(nr.ev) }
        RowLayout {
            anchors.fill: parent
            anchors.leftMargin: 8; anchors.rightMargin: 8
            spacing: 8
            WidgetText { Layout.preferredWidth: 44; role: "meta"; text: nr.ev ? w.dayWord(nr.ev.start, true) : ""; color: Th.widgetMetaFg }
            WidgetText { Layout.fillWidth: true; role: "text"; text: nr.ev ? String(nr.ev.title) : ""; color: Th.widgetFg }
            WidgetText { Layout.maximumWidth: (nr.width - 16) * 0.34; role: "meta"; text: nr.ev ? String(nr.ev.game || "TBD") : ""; color: Th.widgetMetaFg }
            WidgetText { role: "num"; text: nr.ev ? w.timeOf(nr.ev.start) : ""; color: Th.widgetMetaFg }
            // my answer: ✓ ? ✕ (Up next's glyph)
            Item {
                Layout.preferredWidth: 14; Layout.preferredHeight: 14
                readonly property string my: w.myOf(nr.ev)
                WidgetAnswerMark {
                    anchors.centerIn: parent
                    visible: parent.my.length > 0
                    app: w.app
                    v: parent.my.length > 0 ? parent.my : "yes"
                    px: 12
                    ink: parent.my === "yes" ? Th.widgetUpNextRsvpYesFgChecked : parent.my === "maybe" ? Th.widgetUpNextRsvpMaybeFgChecked : Th.widgetUpNextRsvpNoFgChecked
                }
            }
        }
    }

    component SlideIn: ParallelAnimation {
        id: si
        property Item target
        property Translate shift: Translate { y: 0 }
        Component.onCompleted: if (si.target) si.target.transform = [si.shift]
        NumberAnimation { target: si.target; property: "opacity"; from: 0; to: 1; duration: w.hostRef ? w.hostRef.durLong : 0; easing.type: Easing.OutCubic }
        NumberAnimation { target: si.shift; property: "y"; from: 4; to: 0; duration: w.hostRef ? w.hostRef.durLong : 0; easing.type: Easing.OutCubic }
        function play() { if (w.hostRef && w.hostRef.durLong > 0) { si.stop(); si.start() } }
    }

    // ── the sample states (the gallery's preview, the review screenshots, the gates): the web's samples, the same
    //    nights; the clock = today 5:20 PM, so "in 2 h 10 m" reads the same all day ──
    function at(days, h, m) { var d = new Date(); d.setDate(d.getDate() + days); d.setHours(h, m, 0, 0); return d.getTime() }
    function sampleModel(key) {
        var now = w.at(0, 17, 20)
        var P = function (id, name) { return { mxid: "@" + id + ":sample.invalid", name: name, avatar: "" } }
        var team = [P("ana", "Ana"), P("bo", "Bo"), P("cy", "Cy"), P("dee", "Dee"), P("eli", "Eli"), P("fay", "Fay"), P("gus", "Gus")]
        var cover = function (id) { return String(gnCtl.coverArt(id)) }
        var night = function (i, title, start, over) {
            var e = { container: "!squad:sample.invalid", id: "gn" + i, title: title, kind: "session", start: start, end: start + 3 * 3600000,
                      game: "", gameId: "", cover: "", rsvp: { yes: [], maybe: [], no: [] }, my: { status: "", options: [] }, canWrite: true, canceled: false }
            for (var k in over) e[k] = over[k]
            return e
        }
        var valheim = { game: "Valheim", gameId: "steam:892970", cover: cover("steam:892970") }
        var first = key === "tbd" ? night(1, "Friday game night", w.at(0, 19, 30), { rsvp: { yes: team.slice(0, 3), maybe: team.slice(3, 4), no: [] }, my: { status: "maybe", options: [] } })
            : key === "soon" ? night(1, "Valheim night", w.at(0, 17, 26), Object.assign({ rsvp: { yes: team.slice(0, 5), maybe: [], no: [] }, my: { status: "yes", options: [] } }, valheim))
            : night(1, "Valheim night", w.at(0, 19, 30), Object.assign({ rsvp: { yes: team.slice(0, 4), maybe: team.slice(4, 5), no: [] }, my: { status: "yes", options: [] } }, valheim))
        var nights = key === "empty" ? [] : [
            first,
            night(2, "Zomboid Sunday", w.at(2, 20, 0), { game: "Project Zomboid", gameId: "steam:108600", cover: cover("steam:108600"), rsvp: { yes: team.slice(0, 2), maybe: [], no: [] }, my: { status: "yes", options: [] } }),
            night(3, "Game night", w.at(5, 19, 30), { rsvp: { yes: team.slice(1, 3), maybe: [], no: [] } })
        ]
        var credit = key === "empty" ? "" : String(gnCtl.creditText(JSON.stringify({ credit: key === "tbd" ? 0 : 2, mode: "earn" })))
        return { now: now, nights: nights, creditLine: credit }
    }
}
