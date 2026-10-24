// CallStatsWidget.qml — widgets: Call stats (`callStats`, contract WIDGETS.md §5–§7, WIDGETS-UI.md §5; the web's
// features/widgets/callstats/CallStats.tsx).
//
// In a call — one row per remote person (the recorder excluded), sorted by name: avatar + the LiveKit connection dot,
// name, codec / resolution chips, the incoming lane (↓ what you get from them: receiver stats) and the outgoing lane
// (↑ how they get you: their `tinhat.rxq` report), a 60-sample sparkline of what you get; the header's extra = your
// totals. Click a row → the details (per track, both directions). L adds the You row (per published track + the
// encoder's limitation, from the Stream health data). Data: CallStatsController (streamer callstats.rs; every 1 s
// focused, 5 s otherwise). Outside a call — your last call and this week per friend, from the call history
// (WidgetsController, crates/widgets history).
//
// Motion (only while the host animates): laneEase (bar widths, long, OutCubic), sparkStep (the line slides one step),
// lossFlash (a lane that turns poor flashes once, verylong). Numbers change without motion.
// Gate hook: TINHAT_WIDGETS_DEMO_MS (WidgetsController.demoMs) shows two sample people whose numbers change on that
// period — the same code paths as a call, for the GPU gate's "one widget update" (never while hidden).
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
    property bool preview: false

    // ── data ──
    readonly property var cs: w.hostRef ? w.hostRef.callStats : null
    readonly property var wc: w.hostRef ? w.hostRef.widgets : null
    readonly property var snap: {
        var r = w.cs ? w.cs.rev : 0   // re-read on every new snapshot
        var j = w.cs ? String(w.cs.json || "") : ""
        if (j.length < 2) return null
        try { return JSON.parse(j) } catch (e) { return null }
    }
    readonly property var hist: {
        var r = w.wc ? w.wc.rev : 0
        var j = w.wc ? String(w.wc.historyJson || "") : ""
        if (j.length < 2) return null
        try { return JSON.parse(j) } catch (e) { return null }
    }
    readonly property int demoMs: w.wc && !w.preview ? w.wc.demoMs() : 0
    property int demoSeq: 0
    Timer {
        interval: Math.max(250, w.demoMs)
        running: w.demoMs > 0 && !!w.hostRef && w.hostRef.winState !== "hidden"
        repeat: true
        onTriggered: w.demoSeq++
    }
    readonly property bool inCall: w.demoMs > 0 || (!!w.snap && w.snap.on === true)
    readonly property bool hasData: w.inCall || (!!w.hist && !!w.hist.calls && w.hist.calls.length > 0)
    readonly property bool sample: w.preview && !w.hasData
    readonly property bool showCall: w.inCall || w.sample
    readonly property bool live: w.showCall
    // the call history is read while at least one Call stats widget is shown (the gallery's preview counts: it shows
    // real data where there is some, like the web's)
    Component.onCompleted: if (w.wc) w.wc.wantHistory(true)
    Component.onDestruction: if (w.wc) w.wc.wantHistory(false)

    // clock for "… ago" labels outside a call (one minute boundary at a time; nothing while hidden)
    property double now: Date.now()
    Timer {
        interval: Math.max(1000, 60000 - (Date.now() % 60000))
        running: !w.inCall && !w.preview && !!w.hostRef && w.hostRef.winState !== "hidden"
        repeat: true
        onTriggered: { w.now = Date.now(); interval = Math.max(1000, 60000 - (Date.now() % 60000)) }
    }
    Connections {
        target: w.hostRef
        function onWinStateChanged() { if (w.hostRef.winState !== "hidden") w.now = Date.now() }
    }

    // ── people: a LiveKit identity → mxid, name, avatar (the call's tiles know them) ──
    function mxidOf(identity) { return w.app ? w.app.tileMxid(identity) : identity }
    function personOf(identity) {
        var tiles = (w.app && w.app.mVideoTiles) ? w.app.mVideoTiles : []
        var mx = w.mxidOf(identity)
        for (var i = 0; i < tiles.length; ++i) {
            var p = tiles[i].participant || ""
            if (p === identity || w.mxidOf(p) === mx) return { mxid: mx, name: tiles[i].name || w.app.localpart(mx), avatar: tiles[i].avatar || "" }
        }
        return { mxid: mx, name: w.app ? w.app.localpart(mx) : mx, avatar: "" }
    }
    function histPerson(mxid) {
        var p = (w.hist && w.hist.people) ? w.hist.people[mxid] : null
        return { mxid: mxid, name: p ? p.name : (w.app ? w.app.localpart(mxid) : mxid), avatar: p ? (p.avatar || "") : "" }
    }
    // 2026-09-27: no per-person call time. This week's friends are ordered by their number of calls (most first),
    // then the shared people order on the name the row shows (people::friends_by_calls = the web's callstats/order.ts),
    // so the order gives no time away; the rows show no time and the week total none either (WIDGETS-UI.md §5).
    function friendsByCalls(list) {
        var l = list || []
        if (!w.wc || l.length < 2) return l
        var input = l.map(function (f) { return { user: String(f.user), calls: f.calls || 0, name: String(w.histPerson(f.user).name || "") } })
        var order = []
        try { order = JSON.parse(String(w.wc.friendsOrder(JSON.stringify(input)))) } catch (e) { return l }
        var by = {}
        l.forEach(function (f) { by[f.user] = f })
        return order.map(function (u) { return by[u] }).filter(function (f) { return !!f })
    }
    function bandColor(b) {
        return b === "good" ? Th.widgetQualityGoodFill : b === "ok" ? Th.widgetQualityOkFill
             : b === "poor" ? Th.widgetQualityPoorFill : Th.widgetQualityUnknownFill
    }
    function rate(kbps) { return w.wc ? String(w.wc.fmtRate(kbps)) : "" }
    readonly property var rank: ({ good: 0, ok: 1, poor: 2 })
    function worstOf(list) {
        var b = null
        for (var i = 0; i < list.length; ++i) { var q = list[i]; if (q && (b === null || w.rank[q] > w.rank[b])) b = q }
        return b
    }

    // ── the rows (the web's PersonView): lanes, chips, worst band; sorted by name (lower case), then identity ──
    function viewOf(p) {
        var inT = (p["in"] && p["in"].t) ? p["in"].t : []
        var inc = { kbps: p["in"] ? (p["in"].kbps || 0) : 0, band: p["in"] ? (p["in"].q || null) : null, fill: p["in"] ? (p["in"].fill || 0) : 0, none: inT.length === 0 }
        var out = p.out ? { kbps: p.out.kbps || 0, band: p.out.q || null, fill: p.out.fill || 0, none: false } : { kbps: 0, band: null, fill: 0, none: true }
        var main = null
        for (var i = 0; i < inT.length; ++i) if (inT[i].k === "s") main = inT[i]
        if (!main) for (var j = 0; j < inT.length; ++j) if (inT[j].k === "v") main = inT[j]
        if (!main) for (var k = 0; k < inT.length; ++k) if (inT[k].k === "a") main = inT[k]
        var chips = []
        if (main && main.c) chips.push(main.c)
        if (main && main.k !== "a" && main.res) chips.push(main.res)
        return { id: p.id, cq: p.dot || null, cqRaw: p.cq || "", inc: inc, out: out, never: !p.outEver, spark: p.spark || [],
                 chips: chips, incTracks: inT, outTracks: p.out && p.out.t ? p.out.t : [], worst: w.worstOf([inc.band, out.band, p.dot || null]) }
    }
    readonly property var rowViews: {
        if (w.sample) return w.sampleRows
        if (w.demoMs > 0) return w.demoRows
        if (!w.inCall) return []
        return (w.snap.peers || []).map(function (p) { var v = w.viewOf(p); v.who = w.personOf(p.id); return v })
    }
    // order + bands + summary: the shared callPeople (WIDGETS.md §10.6, people.rs — the web's CallStats.tsx): rows by
    // name (ASCII lower-case, the name, the id), each row's band, the FIRST row with the worst band
    readonly property var cp: {
        var none = { order: [], bands: [], summary: { kind: "alone" } }
        if (!w.wc) return none
        var input = w.rowViews.map(function (v) { return { id: v.id, name: String(v.who && v.who.name ? v.who.name : ""), cq: v.cqRaw || null, inBand: v.inc.band || null, outBand: v.out.band || null } })
        try { return JSON.parse(String(w.wc.callPeople(JSON.stringify(input)))) } catch (e) { return none }
    }
    readonly property var rows: {
        var byId = ({})
        w.rowViews.forEach(function (v) { byId[v.id] = v })
        return w.cp.order.map(function (id, i) { var v = byId[id]; if (v) v.worst = w.cp.bands[i]; return v }).filter(function (v) { return !!v })
    }
    readonly property var worstRow: {
        var sm = w.cp.summary
        if (!sm || sm.kind !== "worst") return null
        for (var i = 0; i < w.rows.length; ++i) if (w.rows[i].id === sm.id) return w.rows[i]
        return null
    }
    // You (L): your published tracks by kind, from the Stream health data (the web's youTracks)
    readonly property var you: {
        if (w.sample) return []
        var s = (w.inCall && w.app && w.app.mPublishStats) ? w.app.mPublishStats : []
        var groups = ({}), order = ["mic", "camera", "screen", "music", "sound"]
        var label = { mic: "Mic", camera: "Camera", screen: "Screen", music: "Music", sound: "Sounds" }
        for (var i = 0; i < s.length; ++i) {
            var k = String(s[i].kind || "")
            if (k.indexOf("camera/") === 0) k = "camera"
            if (!groups[k]) groups[k] = []
            groups[k].push(s[i])
        }
        var keys = Object.keys(groups).sort(function (a, b) { return order.indexOf(a) - order.indexOf(b) })
        return keys.map(function (k) {
            var l = groups[k]
            var top = l.slice().sort(function (a, b) { return (b.w || 0) * (b.h || 0) - (a.w || 0) * (a.h || 0) })[0]
            var lim = l.some(function (x) { return x.qlim === "bandwidth" }) ? "bandwidth" : (l.some(function (x) { return x.qlim === "cpu" }) ? "CPU" : "")
            var res = ""
            if ((k === "camera" || k === "screen") && (top.w || 0) > 0 && (top.h || 0) > 0) res = Math.min(Math.round(top.w), Math.round(top.h)) + "p" + (top.fps > 0 ? Math.round(top.fps) : "")
            return { label: label[k] || k, kbps: l.reduce(function (a, x) { return a + (x.sendKnown ? (x.sendKbps || 0) : 0) }, 0),
                     codec: String(top.encPick || "").replace(/ \(browser\)$/, ""), res: res, limit: lim }
        })
    }
    readonly property real totalIn: w.sample ? 1498 : w.rows.reduce(function (a, r) { return a + r.inc.kbps }, 0)
    readonly property real totalOut: w.sample ? 1260 : w.you.reduce(function (a, t) { return a + t.kbps }, 0)

    // ── the header: totals (open) / the one-line summary (folded, S) ──
    // the width of a space in W-num (the web's " " between the parts; Text drops trailing spaces)
    WidgetText { id: spaceRef; visible: false; role: "num"; text: "a a" }
    WidgetText { id: noSpaceRef; visible: false; role: "num"; text: "aa" }
    readonly property real spaceW: Math.max(1, spaceRef.implicitWidth - noSpaceRef.implicitWidth)
    readonly property Component extraComponent: w.showCall ? totalsC : null
    Component {
        id: totalsC
        Row {
            spacing: w.spaceW
            WidgetText { role: "num"; text: "↓"; color: Th.widgetMetaFg }
            WidgetText { role: "num"; text: w.rate(w.totalIn); color: Th.widgetStatsTotalFg }
            WidgetText { role: "num"; text: "↑"; color: Th.widgetMetaFg }
            WidgetText { role: "num"; text: w.rate(w.totalOut); color: Th.widgetStatsTotalFg }
        }
    }
    readonly property bool worstShown: w.showCall && !!w.worstRow && w.worstRow.worst !== "good"
    readonly property Component summaryComponent: Component {
        Row {
            spacing: w.spaceW
            WidgetText {
                role: "num"
                color: Th.widgetSummaryFg
                text: {
                    // (the web's Summary: alone → only what you send; else the rate pair + the worst person / all good)
                    if (w.showCall) {
                        var k = w.cp.summary ? w.cp.summary.kind : "alone"
                        if (k === "alone") return "↑ " + w.rate(w.totalOut) + " · just you"
                        var pair = w.wc ? String(w.wc.fmtRatePair(w.totalIn, w.totalOut)) : ""
                        return pair + " ·" + (k === "measuring" ? " measuring…" : (w.worstShown ? "" : " all good"))
                    }
                    var l = w.hist ? w.hist.last : null
                    if (l && w.wc) return "Last call " + (l.ongoing ? "still going" : String(w.wc.fmtAgo(w.now - l.myEnd))) + " · " + String(w.wc.fmtDur(l.myMs))
                    return w.hist ? "No calls yet this week" : "Looking back…"
                }
            }
            Rectangle {
                visible: w.worstShown
                anchors.verticalCenter: parent.verticalCenter
                width: 6; height: 6; radius: 3
                color: w.bandColor(w.worstRow ? w.worstRow.worst : "")
            }
            WidgetText {
                visible: w.worstShown
                role: "num"
                color: Th.widgetSummaryFg
                text: w.worstRow ? w.worstRow.who.name : ""
            }
        }
    }

    implicitHeight: w.showCall ? callCol.implicitHeight : outside.implicitHeight

    Column {
        id: callCol
        visible: w.showCall
        x: 6; width: parent.width - 12
        spacing: 0
        // ── L: the You row ──
        Rectangle {
            visible: w.size === "L" && !w.preview && w.inCall
            width: parent.width
            height: visible ? youCol.implicitHeight + 12 : 0
            radius: 8
            color: Th.widgetChipBg
            Column {
                id: youCol
                x: 8; y: 6
                width: parent.width - 16
                spacing: 2
                WidgetText { role: "strong"; text: "You"; color: Th.widgetFg }
                WidgetText { visible: w.you.length === 0; width: parent.width; role: "num"; text: "Nothing published"; color: Th.widgetMetaFg }
                Repeater {
                    model: w.you
                    delegate: Row {
                        required property var modelData
                        width: youCol.width
                        clip: true
                        WidgetText {
                            role: "num"
                            color: Th.widgetMetaFg
                            text: modelData.label + " " + w.rate(modelData.kbps) + (modelData.codec ? " · " + modelData.codec : "") + (modelData.res ? " · " + modelData.res : "")
                        }
                        WidgetText { visible: modelData.limit.length > 0; role: "num"; text: " · limited: " + modelData.limit; color: Th.widgetStatsLimitFg }
                    }
                }
            }
        }
        Item { visible: w.size === "L" && !w.preview && w.inCall; width: 1; height: 2 }
        WidgetText {
            visible: w.inCall && w.rows.length === 0
            width: parent.width; height: 40
            horizontalAlignment: Text.AlignHCenter
            role: "meta"
            text: "Nobody else is here yet"
            color: Th.widgetMutedFg
        }
        Repeater {
            // a count, not the array: every new snapshot updates the rows in place (their lanes can ease)
            model: w.showCall ? w.rows.length : 0
            delegate: PersonRow {
                required property int index
                width: callCol.width
                row: w.rows[index] || w.rows[0]
            }
        }
    }

    // ── one person (58 px, padding 6): avatar + dot · name + chips · ↓ ↑ lanes · sparkline ──
    component PersonRow: Rectangle {
        id: pr
        property var row
        readonly property bool open: details.opened && details.pid === pr.row.id
        height: 58
        radius: 8
        color: (prMa.containsMouse || pr.open) && !w.preview ? Th.widgetRowBgHover : "transparent"
        Behavior on color { enabled: !!w.hostRef && w.hostRef.dur > 0; ColorAnimation { duration: w.hostRef ? w.hostRef.dur : 0 } }
        MouseArea {
            id: prMa
            anchors.fill: parent
            hoverEnabled: true
            enabled: !w.preview
            cursorShape: Qt.PointingHandCursor
            onClicked: pr.open ? details.close() : details.openFor(pr.row.id, pr)
        }
        Avatar { x: 6; y: 6; app: w.app; size: 24; name: pr.row.who.name; seed: pr.row.who.mxid; imageSource: pr.row.who.avatar || "" }
        // the connection dot: 8 px at the avatar's bottom-right (−2, −2) with a 2 px ring in the widget's fill
        Rectangle {
            x: 6 + 24 + 2 - 8 - 2; y: 6 + 24 + 2 - 8 - 2
            width: 12; height: 12; radius: 6
            color: Th.widgetBg
            Rectangle { x: 2; y: 2; width: 8; height: 8; radius: 4; color: w.bandColor(pr.row.cq) }
        }
        // line 1 (18): name + chips (gap 4)
        RowLayout {
            x: 38; y: 6
            width: parent.width - 38 - 6
            height: 18
            spacing: 4
            WidgetText { Layout.fillWidth: true; role: "text"; text: pr.row.who.name; color: Th.widgetFg }
            Repeater {
                model: pr.row.chips
                delegate: Rectangle {
                    required property var modelData
                    Layout.preferredHeight: 16
                    Layout.preferredWidth: chipT.implicitWidth + 10
                    radius: 4
                    color: Th.widgetChipBg
                    WidgetText { id: chipT; anchors.centerIn: parent; role: "chip"; text: String(modelData); color: Th.widgetChipFg }
                }
            }
        }
        Lane {
            x: 38; y: 26
            width: parent.width - 38 - 6 - 8 - 48
            arrow: "↓"
            lane: pr.row.inc
            tip: "What you get from " + pr.row.who.name
        }
        Lane {
            x: 38; y: 40
            width: parent.width - 38 - 6 - 8 - 48
            arrow: "↑"
            lane: pr.row.out
            tip: pr.row.never ? "No report from their app yet" : "How " + pr.row.who.name + " gets you"
        }
        WidgetSparkline {
            x: parent.width - 6 - 48; y: 26
            width: 48; height: 26
            values: pr.row.spark || []
            ctl: w.wc
            seq: w.demoMs > 0 ? w.demoSeq : (w.snap ? (w.snap.seq || 0) : 0)
            dur: w.hostRef ? w.hostRef.durLong : 0
        }
    }

    // a lane (12): arrow 10 · 4 · bar (flex, 4 px) · 6 · value 58 right-aligned (a stale / missing lane: "—")
    component Lane: Item {
        id: ln
        property string arrow: "↓"
        property var lane: ({ kbps: 0, band: null, fill: 0, none: true })
        property string tip: ""
        readonly property bool none: !ln.lane || ln.lane.none === true
        readonly property string band: ln.lane && ln.lane.band ? ln.lane.band : ""
        height: 12
        WidgetText { width: 10; height: 12; role: "meta"; text: ln.arrow; color: Th.widgetMetaFg }
        Rectangle {
            id: track
            x: 14; y: 4
            width: Math.max(0, ln.width - 14 - 6 - 58)
            height: 4; radius: 2
            color: Th.widgetStatsLaneTrack
            clip: true
            Rectangle {
                id: bar
                height: 4; radius: 2
                width: ln.none ? 0 : track.width * Math.max(0, Math.min(1000, ln.lane.fill || 0)) / 1000
                color: w.bandColor(ln.band)
                Behavior on width { enabled: !!w.hostRef && w.hostRef.durLong > 0; NumberAnimation { duration: w.hostRef ? w.hostRef.durLong : 0; easing.type: Easing.OutCubic } }
                Behavior on color { enabled: !!w.hostRef && w.hostRef.durLong > 0; ColorAnimation { duration: w.hostRef ? w.hostRef.durLong : 0 } }
            }
            Rectangle {   // lossFlash: the track lights up once when the lane turns poor, fading back (verylong, OutCubic)
                id: flash
                anchors.fill: parent; radius: 2
                color: Th.widgetStatsLaneHighlight
                opacity: 0
            }
        }
        property string _prevBand: ""
        onBandChanged: {
            if (ln.band === "poor" && ln._prevBand !== "poor" && w.hostRef && w.hostRef.durVeryLong > 0) {
                flashAnim.duration = w.hostRef.durVeryLong
                flashAnim.restart()
            }
            ln._prevBand = ln.band
        }
        NumberAnimation { id: flashAnim; target: flash; property: "opacity"; from: 1; to: 0; easing.type: Easing.OutCubic }
        WidgetText {
            anchors.right: parent.right
            width: 58; height: 12
            horizontalAlignment: Text.AlignRight
            role: "num"
            text: ln.none ? "—" : w.rate(ln.lane.kbps || 0)
            color: ln.none ? Th.widgetStatsStaleFg : Th.widgetStatsValueFg
        }
        HoverHandler { id: laneHover; enabled: ln.tip.length > 0 && !w.preview }
        Controls.ToolTip.text: ln.tip
        Controls.ToolTip.visible: laneHover.hovered
        Controls.ToolTip.delay: 500
    }

    // ── outside a call: LAST CALL + THIS WEEK (the web's OutsideBody) ──
    Column {
        id: outside
        visible: !w.showCall
        x: 6; width: parent.width - 12
        readonly property var last: w.hist ? w.hist.last : null
        readonly property var week: w.hist ? w.hist.week : null
        readonly property var friends: w.hist && w.hist.week ? w.friendsByCalls(w.hist.week.friends).slice(0, w.size === "L" && !w.preview ? 10 : 3) : []
        // loading (the first read of the history)
        Item {
            width: parent.width; height: 44
            visible: !w.hist
            Row {
                anchors.centerIn: parent
                spacing: 6
                WidgetBusyMark { anchors.verticalCenter: parent.verticalCenter; size: 16; animate: !!w.hostRef && w.hostRef.animate }
                WidgetText { anchors.verticalCenter: parent.verticalCenter; role: "meta"; text: "Looking back through your calls…"; color: Th.widgetMutedFg }
            }
        }
        // LAST CALL (18: the label on the bottom, padding 0 8 2) + its row (28)
        SectionLabel2 {
            visible: !!outside.last
            text: "Last call"
            rightText: outside.last ? (outside.last.ongoing ? "still going" : (w.wc ? String(w.wc.fmtAgo(w.now - outside.last.myEnd)) : "")) : ""
        }
        Rectangle {
            id: lastRow
            visible: !!outside.last
            width: parent.width; height: visible ? 28 : 0
            radius: 6
            color: lastMa.containsMouse ? Th.widgetRowBgHover : "transparent"
            readonly property var l: outside.last
            readonly property var withIds: lastRow.l && lastRow.l["with"] ? lastRow.l["with"].map(function (x) { return x.user }) : []
            MouseArea { id: lastMa; anchors.fill: parent; hoverEnabled: true; enabled: !w.preview; cursorShape: Qt.PointingHandCursor; onClicked: if (lastRow.l && w.app) w.app.openRoomId(lastRow.l.room) }
            RowLayout {
                anchors.fill: parent
                anchors.leftMargin: 8; anchors.rightMargin: 8
                spacing: 8
                Row {
                    spacing: -4
                    Repeater {
                        model: lastRow.withIds.slice(0, 3)
                        delegate: Item {
                            required property var modelData
                            width: 18; height: 18
                            Rectangle { x: -1.5; y: -1.5; width: 21; height: 21; radius: 10.5; color: Th.widgetBg }
                            Avatar { anchors.fill: parent; app: w.app; size: 18; seed: String(modelData); name: w.histPerson(modelData).name; imageSource: w.histPerson(modelData).avatar }
                        }
                    }
                }
                WidgetText {
                    Layout.fillWidth: true
                    role: "meta"
                    color: Th.widgetMetaFg
                    text: {
                        var l = lastRow.l
                        if (!l || !w.wc) return ""
                        var names = lastRow.withIds.map(function (u) { return w.histPerson(u).name })
                        var who = names.length <= 2 ? names.join(", ") : names.slice(0, 2).join(", ") + " +" + (names.length - 2)
                        var room = (w.hist.rooms && w.hist.rooms[l.room]) ? w.hist.rooms[l.room] : ""
                        return who + " · " + String(w.wc.fmtDur(l.myMs)) + " · " + room
                    }
                }
            }
        }
        // THIS WEEK
        SectionLabel2 {
            visible: !!w.hist
            text: "This week"
            rightText: outside.week && outside.week.myCalls > 0 ? outside.week.myCalls + (outside.week.myCalls === 1 ? " call" : " calls") : ""
        }
        WidgetText {
            visible: !!w.hist && outside.friends.length === 0
            width: parent.width; height: 40
            horizontalAlignment: Text.AlignHCenter
            role: "meta"
            text: "No calls yet this week"
            color: Th.widgetMutedFg
        }
        Repeater {
            model: outside.friends.length
            delegate: Rectangle {
                id: fr
                required property int index
                readonly property var f: outside.friends[index] || ({ user: "", calls: 0, started: 0, joined: 0 })
                readonly property var who: w.histPerson(fr.f.user)
                width: outside.width
                height: 32
                radius: 6
                color: frMa.containsMouse ? Th.widgetRowBgHover : "transparent"
                MouseArea {
                    id: frMa
                    anchors.fill: parent
                    hoverEnabled: true
                    enabled: !w.preview
                    cursorShape: Qt.PointingHandCursor
                    onClicked: if (w.app && w.app.login) w.app.login.createDm(fr.f.user)
                }
                Controls.ToolTip.text: "You started " + fr.f.started + " · joined " + fr.f.joined
                Controls.ToolTip.visible: frMa.containsMouse
                Controls.ToolTip.delay: 700
                RowLayout {
                    anchors.fill: parent
                    anchors.leftMargin: 8; anchors.rightMargin: 8
                    spacing: 8
                    Avatar { Layout.preferredWidth: 20; Layout.preferredHeight: 20; app: w.app; size: 20; seed: fr.f.user; name: fr.who.name; imageSource: fr.who.avatar }
                    Column {
                        Layout.fillWidth: true
                        WidgetText { width: parent.width; role: "text"; text: fr.who.name; color: Th.widgetFg }
                        WidgetText {
                            width: parent.width
                            role: "meta"
                            color: Th.widgetMutedFg
                            text: fr.f.calls + (fr.f.calls === 1 ? " call" : " calls") + (fr.f.started > 0 ? " · you started " + fr.f.started : "")
                        }
                    }
                }
            }
        }
    }

    // a section label row (18): LABEL on the left (W-label muted), the right-hand text in W-num meta — text on the bottom
    component SectionLabel2: Item {
        id: sl
        property string text: ""
        property string rightText: ""
        width: parent ? parent.width : 0
        height: visible ? 18 : 0
        WidgetText { x: 8; y: sl.height - 2 - height; role: "label"; text: sl.text; color: Th.widgetMutedFg }
        WidgetText {
            anchors.right: parent.right; anchors.rightMargin: 8
            y: sl.height - 2 - height
            role: "num"
            text: sl.rightText
            color: Th.widgetMetaFg
        }
    }

    // ── the gate's demo (TINHAT_WIDGETS_DEMO_MS): two people, new numbers every tick; bo's incoming lane turns poor on
    //    every 4th tick (the loss flash) ──
    readonly property var demoRows: {
        var k = w.demoSeq
        function lane(base, band) { var kb = Math.round(base * (0.8 + 0.2 * Math.sin(k / 2))); return { kbps: kb, band: band, fill: w.wc ? Number(w.wc.fillPm(kb)) : 0, none: false } }
        function spark(base) { var a = []; for (var i = 0; i < 60; ++i) a.push(Math.round(base * (0.8 + 0.2 * Math.sin((i + k) / 5)))); return a }
        var boBand = k % 4 === 3 ? "poor" : "ok"
        return [
            { id: "@ana:demo:DEV", who: { mxid: "@ana:demo", name: "ana", avatar: "" }, cq: "good", cqRaw: "excellent",
              inc: lane(1450, "good"), out: lane(620, "good"), never: false, spark: spark(1450), chips: ["AV1", "720p30"], incTracks: [], outTracks: [], worst: "good" },
            { id: "@bo:demo:DEV", who: { mxid: "@bo:demo", name: "bo", avatar: "" }, cq: "ok", cqRaw: "good",
              inc: lane(48, boBand), out: lane(46, "good"), never: false, spark: spark(48), chips: ["Opus"], incTracks: [], outTracks: [], worst: boBand }
        ]
    }

    // ── the sample state (the gallery's preview without real data; the web's sample) ──
    function sparkSample(base) {
        var a = []
        for (var i = 0; i < 60; ++i) a.push(Math.round(base * (0.8 + 0.2 * Math.sin(i / 5) + 0.05 * Math.cos(i * 1.7))))
        return a
    }
    readonly property var sampleRows: [
        { id: "@ana:example.com:DEV", who: { mxid: "@ana:example.com", name: "ana", avatar: "" }, cq: "good", cqRaw: "excellent",
          inc: { kbps: 1450, band: "good", fill: 755, none: false }, out: { kbps: 620, band: "good", fill: 610, none: false }, never: false,
          spark: w.sparkSample(1450), chips: ["AV1", "720p30"], incTracks: [], outTracks: [], worst: "good" },
        { id: "@bo:example.com:DEV", who: { mxid: "@bo:example.com", name: "bo", avatar: "" }, cq: "ok", cqRaw: "poor",
          inc: { kbps: 48, band: "ok", fill: 250, none: false }, out: { kbps: 46, band: "good", fill: 234, none: false }, never: false,
          spark: w.sparkSample(48), chips: ["Opus"], incTracks: [], outTracks: [], worst: "ok" }
    ]

    // ── details (click a row): per track, both directions; live-updates while open (the house Popover) ──
    Controls.Popup {
        id: details
        parent: Controls.Overlay.overlay
        width: 340
        padding: 12
        modal: false
        closePolicy: Controls.Popup.CloseOnEscape | Controls.Popup.CloseOnPressOutside
        property string pid: ""
        readonly property var row: {
            for (var i = 0; i < w.rows.length; ++i) if (w.rows[i].id === details.pid) return w.rows[i]
            return null
        }
        onRowChanged: if (details.opened && !details.row) details.close()
        function openFor(id, anchorItem) {
            details.pid = id
            var p = anchorItem.mapToItem(Controls.Overlay.overlay, anchorItem.width + 8 + 6, 0)
            details.x = Math.min(p.x, Controls.Overlay.overlay.width - details.width - 6)
            details.y = Math.max(6, Math.min(p.y, Controls.Overlay.overlay.height - details.implicitHeight - 6))
            details.open()
        }
        background: Rectangle { color: Th.popoverBg; border.width: 1; border.color: Th.popoverBorder }
        readonly property var kinds: {
            if (!details.row) return ["a"]
            var r = details.row
            var ks = ["a", "v", "s"].filter(function (k) {
                return r.incTracks.some(function (t) { return t.k === k }) || r.outTracks.some(function (t) { return t.k === k })
            })
            return ks.length > 0 ? ks : ["a"]
        }
        contentItem: Column {
            spacing: 6
            RowLayout {
                width: 316
                spacing: 8
                Avatar { Layout.preferredWidth: 20; Layout.preferredHeight: 20; app: w.app; size: 20; name: details.row ? details.row.who.name : ""; seed: details.row ? details.row.who.mxid : ""; imageSource: details.row ? (details.row.who.avatar || "") : "" }
                WidgetText { Layout.fillWidth: true; role: "strong"; text: details.row ? details.row.who.name : ""; color: Th.widgetFg }
                WidgetText {
                    role: "meta"
                    readonly property var words: ({ excellent: "excellent", good: "good", poor: "poor", lost: "lost" })
                    text: "Connection: " + (details.row && words[details.row.cqRaw] ? words[details.row.cqRaw] : "unknown")
                    color: Th.widgetMetaFg
                }
            }
            DirHead { band: details.row ? details.row.inc.band : null; text: details.row ? "You get from " + details.row.who.name : "" }
            StatGrid { tracks: details.row ? details.row.incTracks : []; kinds: details.kinds }
            DirHead { band: details.row ? details.row.out.band : null; text: details.row ? details.row.who.name + " gets from you" : "" }
            WidgetText { visible: !!details.row && details.row.never; x: 8; role: "meta"; text: "Needs Tin Hat 11 on their side"; color: Th.widgetMutedFg }
            StatGrid { visible: !!details.row && !details.row.never; tracks: details.row ? details.row.outTracks : []; kinds: details.kinds }
            WidgetText {
                width: 316
                leftPadding: 8; rightPadding: 8; topPadding: 4; bottomPadding: 2
                role: "meta"
                wrapMode: Text.Wrap
                maximumLineCount: 2
                text: "Updates every second while Tin Hat is focused, every 5 seconds otherwise."
                color: Th.widgetMutedFg
            }
        }
    }
    // a direction's heading (margin-top 4): a 6 px band dot + the W-label text
    component DirHead: Item {
        id: dh
        property var band: null
        property string text: ""
        width: 316; height: 4 + dhT.implicitHeight
        Row {
            y: 4
            spacing: 6
            Rectangle { anchors.verticalCenter: parent.verticalCenter; width: 6; height: 6; radius: 3; color: w.bandColor(dh.band || "") }
            WidgetText { id: dhT; role: "label"; text: dh.text; color: Th.widgetMutedFg }
        }
    }
    // the per-track grid: a label column + one column per kind (Voice / Camera / Screen), rows Bitrate · Loss · Jitter
    // · Round trip · Codec · Video; missing values "—"
    component StatGrid: GridLayout {
        id: sg
        property var tracks: []
        property var kinds: ["a"]
        width: 316
        columns: 1 + sg.kinds.length
        columnSpacing: 12
        rowSpacing: 2
        function trackOf(k) { for (var i = 0; i < sg.tracks.length; ++i) if (sg.tracks[i].k === k) return sg.tracks[i]; return null }
        function cell(r, k) {
            var t = sg.trackOf(k)
            if (!t) return "—"
            var v = ""
            if (r === 0) v = t.kbps !== undefined ? w.rate(t.kbps) : ""
            else if (r === 1) v = t.loss !== undefined && w.wc ? String(w.wc.fmtLoss(t.loss)) : ""
            else if (r === 2) v = t.jit !== undefined ? t.jit + " ms" : ""
            else if (r === 3) v = t.rtt !== undefined ? t.rtt + " ms" : ""
            else if (r === 4) v = t.c ? String(t.c) : ""
            else v = t.k !== "a" && t.res ? String(t.res) : ""
            return v.length > 0 ? v : "—"
        }
        Item { Layout.preferredHeight: 1; Layout.preferredWidth: 1 }
        Repeater {
            model: sg.kinds
            delegate: WidgetText {
                required property var modelData
                Layout.fillWidth: true
                Layout.preferredWidth: 1
                role: "meta"; semibold: true
                text: modelData === "a" ? "Voice" : (modelData === "v" ? "Camera" : "Screen")
                color: Th.widgetMetaFg
            }
        }
        Repeater {
            model: ["Bitrate", "Loss", "Jitter", "Round trip", "Codec", "Video"].length * (1 + sg.kinds.length)
            delegate: WidgetText {
                required property int index
                readonly property int cols: 1 + sg.kinds.length
                readonly property int r: Math.floor(index / cols)
                readonly property int c: index % cols
                Layout.fillWidth: c > 0
                Layout.preferredWidth: c > 0 ? 1 : -1
                role: "num"
                text: c === 0 ? ["Bitrate", "Loss", "Jitter", "Round trip", "Codec", "Video"][r] : sg.cell(r, sg.kinds[c - 1])
                color: c === 0 ? Th.widgetMetaFg : Th.widgetFg
            }
        }
    }
}
