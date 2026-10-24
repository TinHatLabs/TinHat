// MentionsWidget.qml — widgets W1 (2026-09-28): Mentions (`mentions`, contract WIDGETS.md §13, WIDGETS-UI.md §13; the
// web's features/widgets/mentions/Mentions.tsx).
//
// Your recent mentions and replies across rooms, newest first — M the last 5, L the last 12, S / collapsed "2 new
// mentions · Gaming Central". The list is this device's (MentionsController: the events the push rules highlighted
// after decryption — the same list the mention badges and notifications count — kept as ids in <data>/mentions.json;
// the text is resolved when a row shows it). A row you haven't opened carries the mention wash + edge; the header
// shows how many (live dot). Click → the message (Main.jumpToEvent) and it counts as seen; a room shown in this app
// marks its mentions seen. ⋯ / right-click → Jump to message · Mark as seen · Mark all as seen.
// Motion: rowSlide (a new mention). No timer. Gate / review: TINHAT_WIDGETS_SCENARIO "mentions=<key>" (data, many,
// allseen, empty, loading) — the web's samples, the same rows.
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
    readonly property int limit: w.size === "L" && !w.preview ? 12 : 5

    MentionsController { id: mentionsCtl }
    Component.onCompleted: { mentionsCtl.start(); mentionsCtl.wantRows(w.limit); w.prevKeys = w.keys }
    Component.onDestruction: mentionsCtl.wantRows(0)
    onLimitChanged: mentionsCtl.wantRows(w.limit)

    // ── the model (the web's useMentions) ──
    readonly property var realRows: {
        var raw = String(mentionsCtl.rowsJson || "")
        try { return raw.length > 0 ? JSON.parse(raw) : [] } catch (e) { return [] }
    }
    readonly property var realModel: ({ rows: w.realRows, total: mentionsCtl.total, unseen: mentionsCtl.unseen, summary: String(mentionsCtl.summary || ""), ready: String(mentionsCtl.rowsJson || "").length > 0 })
    readonly property bool hasData: w.realModel.total > 0
    readonly property string scenario: w.wc ? String(w.wc.scenarioOf("mentions") || "") : ""
    readonly property bool sample: w.scenario.length > 0 || (w.preview && !w.hasData)
    readonly property var model: w.sample ? w.sampleModel(w.scenario.length > 0 ? w.scenario : "preview") : w.realModel
    readonly property var shown: w.model.rows.slice(0, w.limit)
    readonly property bool live: w.model.unseen > 0

    readonly property Component summaryComponent: Component { WidgetHeaderText { text: w.model.summary } }
    // M / L header: "2 new"
    readonly property Component extraComponent: Component { WidgetHeaderText { text: w.model.unseen > 0 ? w.model.unseen + " new" : "" } }

    // rowSlide: a mention that newly enters the list (after the first fill) — opacity 0→1, y +4→0 (long, OutCubic)
    readonly property var keys: w.shown.map(function (r) { return String(r.e) })
    property var prevKeys: null
    onKeysChanged: Qt.callLater(function () { w.prevKeys = w.keys })
    function isFresh(k) { return w.prevKeys !== null && w.prevKeys.indexOf(k) < 0 }

    // ── actions ──
    function jump(r) {
        if (w.preview || w.sample || !w.app) return
        mentionsCtl.markSeen(r.e)
        w.app.jumpToEvent(r.r, r.e)
    }
    function markSeen(r) { if (!w.preview && !w.sample) mentionsCtl.markSeen(r.e) }
    function markAll() { if (!w.preview && !w.sample) mentionsCtl.markAllSeen() }
    function whenText(ts) {
        if (!w.wc) return ""
        var now = Date.now()
        return String(w.wc.whenLabel(ts, now, -new Date(now).getTimezoneOffset(), w.timeOf(ts)))
    }
    // the app's clock setting (12 h / 24 h); "System" = the locale's short time WITHOUT seconds (Up next's timeOf)
    function timeOf(ms) {
        var d = new Date(ms)
        var mode = w.app ? String(w.app.clockMode || "") : ""
        if (mode === "12") return Qt.formatTime(d, "h:mm AP")
        if (mode === "24") return Qt.formatTime(d, "HH:mm")
        return Qt.formatTime(d, String(Qt.locale().timeFormat(Locale.ShortFormat)).replace(/[:.]ss/, ""))
    }
    // the snippet kinds that lead with an icon (Pinned's)
    function kindIcon(k) {
        return k === "image" ? "image-x-generic" : k === "video" ? "video-x-generic" : k === "audio" ? "audio-x-generic"
            : k === "voice" ? "audio-input-microphone-symbolic" : k === "file" ? "text-x-generic"
            : k === "sticker" ? "face-smile-symbolic" : k === "encrypted" ? "channel-secure-symbolic" : ""
    }

    implicitHeight: bodyCol.implicitHeight

    Column {
        id: bodyCol
        x: 6; width: parent.width - 12
        // nothing listed
        Item {
            visible: w.model.total === 0 && (w.model.ready || w.sample)
            width: parent.width; height: visible ? Math.max(56, emptyCol.implicitHeight) : 0
            Column {
                id: emptyCol
                anchors.centerIn: parent
                spacing: 2
                WidgetText { anchors.horizontalCenter: parent.horizontalCenter; role: "text"; text: "No mentions"; color: Th.widgetMetaFg }
                WidgetText { anchors.horizontalCenter: parent.horizontalCenter; role: "meta"; text: "When someone mentions you or replies to you, it shows up here."; color: Th.widgetMutedFg }
            }
        }
        // not read yet
        WidgetText {
            visible: !w.model.ready && !w.sample
            width: parent.width; height: visible ? 44 : 0
            horizontalAlignment: Text.AlignHCenter
            role: "meta"
            text: "Loading mentions…"
            color: Th.widgetMutedFg
        }
        Repeater {
            model: w.shown.length
            delegate: MentionRow { required property int index; width: bodyCol.width; r: w.shown[index] }
        }
        WidgetText {
            visible: w.shown.length > 0 && w.model.total > w.limit
            x: 8
            width: parent.width - 16; height: visible ? 22 : 0
            role: "meta"
            text: "+ " + (w.model.total - w.limit) + " more"
            color: Th.widgetMutedFg
        }
    }

    // ── a mention: 40, padding 0 8, radius 8: avatar 20 · name · room + time over the snippet; unseen = wash + edge ──
    component MentionRow: Rectangle {
        id: mr
        property var r: ({ r: "", e: "", s: "", t: 0, seen: true, room: "", name: "", avatar: "", kind: "text", text: "", ready: true })
        readonly property bool unseen: mr.r.seen !== true
        readonly property bool quiet: mr.r.kind === "deleted" || mr.r.kind === "encrypted" || mr.r.ready === false
        readonly property string icon: w.kindIcon(mr.r.kind)
        readonly property bool menuHere: rowMenu.opened && !!rowMenu.m && rowMenu.m.e === mr.r.e
        readonly property bool hot: (mrHover.hovered || mr.menuHere) && !w.preview
        readonly property string key: String(mr.r.e)
        onKeyChanged: if (w.isFresh(mr.key)) mrIn.play()
        Component.onCompleted: if (w.isFresh(mr.key)) mrIn.play()
        SlideIn { id: mrIn; target: mr }
        height: 40
        radius: 8
        color: mr.unseen ? Th.widgetMentionsUnseenBg : "transparent"
        Accessible.role: Accessible.Button
        Accessible.name: (mr.r.name || "") + " in " + (mr.r.room || "") + ": " + (mr.r.text || "") + (mr.unseen ? " (new)" : "")
        // hover wash over the unseen wash (both paint: the row reads "new" while hovered)
        Rectangle { anchors.fill: parent; radius: 8; color: Th.widgetRowBgHover; visible: mr.hot }
        // the unseen edge: 3 px, radius 2, top / bottom 8 (Up next's soon edge)
        Rectangle { visible: mr.unseen; x: 0; y: 8; width: 3; height: parent.height - 16; radius: 2; color: Th.widgetMentionsUnseenEdge }
        HoverHandler { id: mrHover; enabled: !w.preview }
        MouseArea {
            anchors.fill: parent
            enabled: !w.preview
            acceptedButtons: Qt.LeftButton | Qt.RightButton
            cursorShape: Qt.PointingHandCursor
            onClicked: (m) => { if (m.button === Qt.RightButton) rowMenu.openFor(mr.r, null); else w.jump(mr.r) }
        }
        RowLayout {
            anchors.fill: parent
            anchors.leftMargin: 8; anchors.rightMargin: 8
            spacing: 8
            Avatar {
                Layout.preferredWidth: 20; Layout.preferredHeight: 20
                Layout.alignment: Qt.AlignVCenter
                app: w.app
                size: 20
                seed: mr.r.s || ""
                name: mr.r.name || ""
                imageSource: mr.r.avatar || ""
            }
            Column {
                Layout.fillWidth: true
                Layout.alignment: Qt.AlignVCenter
                RowLayout {
                    width: parent.width
                    spacing: 0
                    // the name keeps its width (at most 60 % of the line); the room elides first
                    WidgetText { id: nameT; Layout.fillWidth: false; Layout.preferredWidth: Math.min(implicitWidth, parent.width * 0.6); role: "text"; semibold: mr.unseen; text: mr.r.name || ""; color: Th.widgetFg }
                    WidgetText { Layout.fillWidth: true; Layout.minimumWidth: 0; role: "meta"; text: mr.r.room ? " · " + mr.r.room : ""; color: Th.widgetMutedFg }
                    WidgetText { Layout.leftMargin: 8; role: "num"; text: w.whenText(mr.r.t || 0); color: Th.widgetMetaFg; opacity: mr.hot ? 0 : 1 }
                }
                RowLayout {
                    width: parent.width
                    spacing: 4
                    ThIcon {
                        visible: mr.icon.length > 0
                        Layout.preferredWidth: 12; Layout.preferredHeight: 12
                        Layout.alignment: Qt.AlignVCenter
                        app: w.app
                        name: mr.icon
                        color: mr.quiet ? Th.widgetMutedFg : Th.widgetMetaFg
                    }
                    WidgetText { Layout.fillWidth: true; role: "meta"; text: mr.r.ready === false ? "…" : (mr.r.text || ""); color: mr.quiet ? Th.widgetMutedFg : Th.widgetMetaFg }
                }
            }
        }
        // ⋯ in the time's place while the row is hovered (Pinned's button)
        Controls.AbstractButton {
            id: moreBtn
            visible: mr.hot
            x: mr.width - 4 - width; y: 1
            width: 22; height: 22
            hoverEnabled: true
            focusPolicy: Qt.TabFocus
            Accessible.name: "Mention options"
            Controls.ToolTip.text: "More"
            Controls.ToolTip.visible: hovered && !rowMenu.opened
            Controls.ToolTip.delay: 500
            background: Rectangle {
                radius: 6
                color: (moreBtn.hovered || moreBtn.pressed || mr.menuHere) ? Th.widgetMenuBgHover : "transparent"
                border.width: moreBtn.visualFocus ? 1 : 0
                border.color: Th.widgetRingFocus
            }
            contentItem: Item {
                ThIcon { anchors.centerIn: parent; width: 16; height: 16; app: w.app; name: "overflow-menu-symbolic"; color: Th.widgetMenuIcon }
            }
            onClicked: rowMenu.opened ? rowMenu.close() : rowMenu.openFor(mr.r, moreBtn)
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

    // ── ⋯ / right-click: Jump to message · Mark as seen · Mark all as seen ──
    Controls.Menu {
        id: rowMenu
        modal: false
        property var m: null
        function openFor(x, anchor) {
            rowMenu.m = x
            if (anchor) rowMenu.popup(anchor, 0, anchor.height)
            else rowMenu.popup()
        }
        Controls.MenuItem { text: "Jump to message"; icon.name: "go-next-symbolic"; onTriggered: if (rowMenu.m) w.jump(rowMenu.m) }
        Controls.MenuItem {
            text: "Mark as seen"
            icon.name: "object-select-symbolic"
            visible: !!rowMenu.m && rowMenu.m.seen !== true
            height: visible ? implicitHeight : 0
            onTriggered: if (rowMenu.m) w.markSeen(rowMenu.m)
        }
        Controls.MenuItem {
            text: "Mark all as seen"
            icon.name: "checkmark-symbolic"
            visible: w.model.unseen > 0
            height: visible ? implicitHeight : 0
            onTriggered: w.markAll()
        }
    }

    // ── the sample states (the gallery's preview, the review screenshots, the gates): the web's samples, same rows ──
    // local time `daysAgo` days back at h:m — a capture reads the same all day (Pinned's localAt)
    function localAt(daysAgo, h, m) {
        var d = new Date()
        d.setDate(d.getDate() - daysAgo)
        d.setHours(h, m, 0, 0)
        return d.getTime()
    }
    function sampleModel(key) {
        var sp = function (id) { return "@" + id + ":sample.invalid" }
        var LOUNGE = "!lounge:sample.invalid", SQUAD = "!squad:sample.invalid", RAID = "!raid:sample.invalid"
        var names = {}
        names[LOUNGE] = "Game lounge"; names[SQUAD] = "Squad chat"; names[RAID] = "Raid planning"
        var row = function (e, who, name, room, ts, seen, kind, text) {
            return { r: room, e: e, s: sp(who), t: ts, seen: seen, room: names[room], name: name, avatar: "", kind: kind, text: text, ready: true }
        }
        var all = [
            row("$m1", "ana", "Ana", LOUNGE, w.localAt(0, 18, 40), false, "text", "are you on tonight? we need a healer"),
            row("$m2", "bo", "Bo", SQUAD, w.localAt(0, 12, 5), false, "text", "that clip of yours was insane 😂"),
            row("$m3", "cy", "Cy", LOUNGE, w.localAt(1, 21, 30), true, "image", "loadout.png"),
            row("$m4", "dee", "Dee", RAID, w.localAt(2, 12, 0), true, "text", "can you bring the fire resist potions?"),
            row("$m5", "eli", "Eli", SQUAD, w.localAt(5, 12, 0), true, "encrypted", "Encrypted message"),
            row("$m6", "fay", "Fay", RAID, w.localAt(8, 12, 0), true, "text", "you're tank on Thursday"),
            row("$m7", "gus", "Gus", LOUNGE, w.localAt(9, 12, 0), true, "voice", "Voice message"),
            row("$m8", "hal", "Hal", SQUAD, w.localAt(11, 12, 0), true, "text", "gg, see you next week"),
            row("$m9", "ivy", "Ivy", RAID, w.localAt(13, 12, 0), true, "file", "raid-roster.pdf"),
            row("$m10", "jo", "Jo", LOUNGE, w.localAt(16, 12, 0), true, "text", "who's hosting?"),
            row("$m11", "kit", "Kit", SQUAD, w.localAt(20, 12, 0), true, "text", "thanks for carrying lol"),
            row("$m12", "ana", "Ana", RAID, w.localAt(25, 12, 0), true, "text", "first raid night is set"),
            row("$m13", "bo", "Bo", LOUNGE, w.localAt(30, 12, 0), true, "text", "welcome to the lounge"),
            row("$m14", "cy", "Cy", SQUAD, w.localAt(40, 12, 0), true, "text", "hi!")
        ]
        var count = key === "many" ? 14 : key === "empty" ? 0 : 5
        var rows = all.slice(0, count)
        if (key === "allseen") rows = rows.map(function (x) { var o = {}; for (var k in x) o[k] = x[k]; o.seen = true; return o })
        if (key === "loading") rows = rows.map(function (x) { var o = {}; for (var k in x) o[k] = x[k]; o.ready = false; o.text = ""; o.kind = "text"; return o })
        var unseen = rows.filter(function (x) { return x.seen !== true }).length
        var summary = count === 0 ? "No mentions" : unseen === 0 ? "No new mentions"
            : (unseen === 1 ? "1 new mention" : unseen + " new mentions") + " · " + rows.filter(function (x) { return x.seen !== true })[0].room
        return { rows: rows, total: count, unseen: unseen, summary: summary, ready: true }
    }
}
