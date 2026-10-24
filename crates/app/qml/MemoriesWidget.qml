// MemoriesWidget.qml — widgets W2: Memories (`memories`, contract WIDGETS.md §21, WIDGETS-UI.md §21; the web's
// features/widgets/memories/Memories.tsx). Your latest recordings from the Memories DVR — the rooms you're in
// that record — newest first. M = the last 4, L = 8: a 64 × 36 poster (the Memories window's own frame, fetched once),
// the room's name and "2 h ago · 1 h 20 m" ("Recording now", "Processing…"); S / collapsed = "3 new memories · latest 2 h
// ago". Click → the Memories window on that memory (Main.openMemory → the window's own tryOpen: a live or processing
// one shows the window's notice).
//
// Data: the host's MemoriesController (memories_ctl.rs: the rooms with a recording marker, `/api/vods/sessions`, the list
// = crates/widgets memories.rs = the web's memories.ts; posters as local files). Samples (the gallery's preview without
// data, and the gates' TINHAT_WIDGETS_SCENARIO=memories=<key>) = the web's samples, the same rows. Motion: rowSlide (a
// memory that newly enters, long, OutCubic). No timer of its own.
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
    readonly property var ctl: w.hostRef ? w.hostRef.memories : null
    readonly property string scenario: w.wc ? String(w.wc.scenarioOf("memories") || "") : ""
    readonly property int limit: w.size === "L" && !w.preview ? 8 : 4

    Component.onCompleted: { if (w.ctl && w.scenario.length === 0) w.ctl.acquire(); w.prevKeys = w.keys }
    Component.onDestruction: if (w.ctl && w.scenario.length === 0) w.ctl.release()

    // ── the model (the web's MemoriesModel) ──
    readonly property var liveItems: {
        var t = w.ctl ? String(w.ctl.itemsJson) : "[]"
        try { return JSON.parse(t) } catch (e) { return [] }
    }
    readonly property bool hasData: w.liveItems.length > 0
    readonly property bool sample: w.scenario.length > 0 || (w.preview && !w.hasData)
    readonly property var model: w.sample ? w.sampleModel(w.scenario.length > 0 ? w.scenario : "preview")
        : ({ items: w.liveItems, loading: !!w.ctl && w.ctl.loading, error: !!w.ctl && w.ctl.failed, now: (w.ctl && w.ctl.readAt > 0) ? w.ctl.readAt : Date.now() })
    readonly property var shown: w.model.items.slice(0, w.limit)
    readonly property bool live: false

    readonly property string summaryText: (w.model.loading && w.model.items.length === 0) ? "Looking for memories…"
        : (w.ctl ? String(w.ctl.summary(JSON.stringify(w.model.items), w.model.now)) : "")
    readonly property Component summaryComponent: Component { WidgetHeaderText { text: w.summaryText } }

    // rowSlide: a memory that newly enters the list (after the first fill) — opacity 0→1, y +4→0 (long, OutCubic)
    readonly property var keys: w.shown.map(function (m) { return m.room + "/" + m.id })
    property var prevKeys: null
    onKeysChanged: Qt.callLater(function () { w.prevKeys = w.keys })
    function isFresh(k) { return w.prevKeys !== null && w.prevKeys.indexOf(k) < 0 }

    function lineOf(m) { return w.ctl ? String(w.ctl.line(JSON.stringify(m), w.model.now)) : "" }
    function glyphOf(m) { return m.state === "live" ? "🔴" : (m.state === "processing" ? "⏳" : "📼") }
    function open(m) {
        if (w.preview || w.sample || !w.app || String(m.room).indexOf(":sample.invalid") >= 0) return
        w.app.openMemory(m.room, m.id, m.title)
    }

    implicitHeight: bodyCol.implicitHeight

    Column {
        id: bodyCol
        x: 6; width: parent.width - 12
        // loading / error / empty
        WidgetText {
            visible: w.shown.length === 0 && w.model.loading
            width: parent.width; height: visible ? 44 : 0
            horizontalAlignment: Text.AlignHCenter
            role: "meta"
            text: "Looking for memories…"
            color: Th.widgetMutedFg
        }
        Item {
            visible: w.shown.length === 0 && !w.model.loading && w.model.error
            width: parent.width; height: visible ? 44 : 0
            Row {
                anchors.centerIn: parent
                spacing: 4
                WidgetText { anchors.verticalCenter: parent.verticalCenter; role: "meta"; text: "Can't reach the recordings server ·"; color: Th.widgetMutedFg }
                Controls.AbstractButton {
                    id: retry
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
        Item {
            visible: w.shown.length === 0 && !w.model.loading && !w.model.error
            width: parent.width; height: visible ? Math.max(56, emptyCol.implicitHeight) : 0
            Column {
                id: emptyCol
                anchors.centerIn: parent
                spacing: 2
                WidgetText { anchors.horizontalCenter: parent.horizontalCenter; role: "text"; text: "No memories yet"; color: Th.widgetMetaFg }
                WidgetText { anchors.horizontalCenter: parent.horizontalCenter; role: "meta"; text: "Press ⏺ Rec in a call to keep it for reliving later."; color: Th.widgetMutedFg }
            }
        }
        Repeater {
            model: w.shown.length
            delegate: MemRow { required property int index; width: bodyCol.width; m: w.shown[index] }
        }
    }

    // ── a memory: 44, padding 4 8, radius 8, gap 8: the 64 × 36 poster · the room's name over "2 h ago · 1 h 20 m" ──
    component MemRow: Rectangle {
        id: mr
        property var m: ({ room: "", id: "", title: "", state: "ready", posterPath: "" })
        readonly property string key: String(mr.m.room) + "/" + String(mr.m.id)
        onKeyChanged: if (w.isFresh(mr.key)) mrIn.play()
        Component.onCompleted: if (w.isFresh(mr.key)) mrIn.play()
        SlideIn { id: mrIn; target: mr }
        height: 44
        radius: 8
        color: mrHover.hovered && !w.preview ? Th.widgetRowBgHover : "transparent"
        Accessible.role: Accessible.Button
        Accessible.name: String(mr.m.title) + " · " + w.lineOf(mr.m)
        HoverHandler { id: mrHover; enabled: !w.preview }
        MouseArea {
            anchors.fill: parent
            enabled: !w.preview
            cursorShape: Qt.PointingHandCursor
            onClicked: w.open(mr.m)
        }
        RowLayout {
            anchors.fill: parent
            anchors.leftMargin: 8; anchors.rightMargin: 8
            spacing: 8
            // the poster: the Memories window's poster colours; the glyph until (unless) the frame is here
            Rectangle {
                Layout.preferredWidth: 64; Layout.preferredHeight: 36
                Layout.alignment: Qt.AlignVCenter
                radius: 4
                color: Th.memoriesCardPosterBg
                clip: true
                Image {
                    id: poster
                    anchors.fill: parent
                    visible: status === Image.Ready
                    source: String(mr.m.posterPath || "")
                    fillMode: Image.PreserveAspectCrop
                    asynchronous: true
                    cache: true
                    sourceSize.width: 128
                }
                Text {
                    anchors.centerIn: parent
                    visible: poster.status !== Image.Ready
                    text: w.glyphOf(mr.m)
                    font.pixelSize: 13
                    color: Th.memoriesCardPosterFg
                }
                Rectangle {   // 1 px widget.cover.border inside
                    anchors.fill: parent
                    radius: 4
                    color: "transparent"
                    border.width: 1
                    border.color: Th.widgetCoverBorder
                }
            }
            Column {
                Layout.fillWidth: true
                Layout.alignment: Qt.AlignVCenter
                WidgetText { width: parent.width; role: "text"; text: String(mr.m.title || ""); color: Th.widgetFg }
                Row {
                    width: parent.width
                    spacing: 5
                    Rectangle {
                        visible: mr.m.state === "live"
                        anchors.verticalCenter: parent.verticalCenter
                        width: 6; height: 6; radius: 3
                        color: Th.memoriesCardLiveBg
                    }
                    WidgetText {
                        width: parent.width - (mr.m.state === "live" ? 11 : 0)
                        role: "num"
                        text: w.lineOf(mr.m)
                        color: mr.m.state === "processing" ? Th.memoriesCardProcessingFg : Th.widgetMetaFg
                    }
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

    // ── the sample states (the gallery's preview, the review screenshots, the gates): the web's samples, the same rows;
    //    times at fixed local hours read against today 6:00 PM, so a capture reads the same all day ──
    function localAt(daysAgo, h, m) {
        var d = new Date()
        d.setDate(d.getDate() - daysAgo)
        d.setHours(h, m, 0, 0)
        return d.getTime()
    }
    function sampleModel(key) {
        var now = w.localAt(0, 18, 0)
        var S = function (id, room, title, start, end, state) {
            return { room: room, id: id, title: title, start: start, end: end, state: state || "ready", durMs: end === null ? null : end - start, poster: { file: "cam.mkv", t: 8 }, people: [], posterPath: "" }
        }
        var L = "!lounge:sample.invalid", Q = "!squad:sample.invalid"
        var all = [
            S("m1", L, "Game lounge", w.localAt(0, 14, 0), w.localAt(0, 15, 20)),
            S("m2", Q, "Squad chat", w.localAt(1, 21, 10), w.localAt(1, 22, 5)),
            S("m3", L, "Game lounge", w.localAt(2, 20, 0), w.localAt(2, 22, 40)),
            S("m4", L, "Game lounge", w.localAt(4, 19, 30), w.localAt(4, 20, 15)),
            S("m5", Q, "Squad chat", w.localAt(6, 18, 0), w.localAt(6, 18, 45)),
            S("m6", L, "Game lounge", w.localAt(9, 20, 0), w.localAt(9, 23, 0)),
            S("m7", Q, "Squad chat", w.localAt(12, 21, 0), w.localAt(12, 21, 30)),
            S("m8", L, "Game lounge", w.localAt(20, 20, 0), w.localAt(20, 21, 10))
        ]
        var items = all
        if (key === "live") items = [S("m0", L, "Game lounge", now - 25 * 60000, null, "live")].concat(all.slice(0, 7))
        else if (key === "processing") { var p = S("m1", L, "Game lounge", w.localAt(0, 14, 0), w.localAt(0, 15, 20), "processing"); items = [p].concat(all.slice(1)) }
        else if (key === "one") items = all.slice(0, 1)
        else if (key === "empty" || key === "loading" || key === "error") items = []
        return { items: items, loading: key === "loading", error: key === "error", now: now }
    }
}
