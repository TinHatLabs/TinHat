// PinnedWidget.qml — widgets W1 (2026-09-28): Pinned (`pinned`, contract WIDGETS.md §11, WIDGETS-UI.md §11; the web's
// features/widgets/pinned/Pinned.tsx).
//
// The open room's pins (`m.room.pinned_events`), newest pin first — M the first 3, L the first 8, S / collapsed
// "3 pins · latest: …" (recent::pin_summary). A row = the sender's avatar and name, the time (recent::when_label: the
// app's clock time today, "Yesterday", "Sat", "Sep 3"), and the message's one-line snippet or an attachment icon + its
// name (recent::snippet_of, computed in Rust on the event as the timeline decrypted it). Click → the message in the
// timeline: the conversation's own jumpTo, the one the pinned popover uses. ⋯ (hover) / right-click → Jump to
// message · Unpin (when the room lets you edit its pins: RoomController.canPin, the message menu's rule).
// Data: the open room's RoomController (Main.qml `room`): pinnedJson (the rows, now with sender / avatar / kind / text /
// pinIndex), pinnedCount, canPin, pinMessage. No timer of its own. Motion: rowSlide (a newly pinned message).
// Gate / review: TINHAT_WIDGETS_SCENARIO "pinned=<key>" (WidgetsController.scenarioOf) shows the sample state <key>
// — data, many, one, empty, noroom, loading — the web's samples, the same rows.
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
    readonly property var room: w.app ? w.app.room : null
    readonly property int limit: w.size === "L" && !w.preview ? 8 : 3

    // ── the model (the web's usePinned) ──
    readonly property string roomId: w.room ? String(w.room.roomId || "") : ""
    readonly property var realRows: {
        var raw = w.room ? String(w.room.pinnedJson || "") : ""
        var list = []
        try { list = raw.length > 0 ? JSON.parse(raw) : [] } catch (e) { list = [] }
        list = list.filter(function (r) { return r && typeof r.pinIndex === "number" && r.pinIndex >= 0 })
        list.sort(function (a, b) { return a.pinIndex - b.pinIndex })
        return list
    }
    readonly property var realModel: ({
        room: w.roomId,
        roomName: w.room ? String(w.room.roomName || "") : "",
        count: w.room ? (w.room.pinnedCount || 0) : 0,
        rows: w.realRows,
        loading: false,
        canUnpin: !!w.room && w.room.canPin === true
    })
    readonly property bool hasData: w.realModel.room.length > 0 && w.realModel.count > 0 && w.realModel.rows.length > 0
    readonly property string scenario: w.wc ? String(w.wc.scenarioOf("pinned") || "") : ""
    readonly property bool sample: w.scenario.length > 0 || (w.preview && !w.hasData)
    readonly property var model: w.sample ? w.sampleModel(w.scenario.length > 0 ? w.scenario : "preview") : w.realModel
    readonly property var shown: w.model.rows.slice(0, w.limit)
    readonly property bool live: false

    readonly property string summaryText: !w.model.room ? "No room open"
        : (w.wc ? String(w.wc.pinSummary(w.model.count, w.model.rows.length > 0 ? String(w.model.rows[0].text || "") : "")) : "")
    readonly property Component summaryComponent: Component { WidgetHeaderText { text: w.summaryText } }
    // M / L header: the room the pins are from
    readonly property Component extraComponent: Component { WidgetHeaderText { text: w.model.room ? w.model.roomName : "" } }

    // rowSlide: a pin that newly enters the list (after the first fill) — opacity 0→1, y +4→0 (long, OutCubic)
    readonly property var keys: w.shown.map(function (r) { return String(r.id) })
    property var prevKeys: null
    onKeysChanged: Qt.callLater(function () { w.prevKeys = w.keys })
    Component.onCompleted: w.prevKeys = w.keys
    function isFresh(k) { return w.prevKeys !== null && w.prevKeys.indexOf(k) < 0 }

    // ── actions ──
    function jump(id) {
        if (w.preview || w.sample || !w.app) return
        w.app.jumpToEvent(w.model.room, id)
    }
    function unpin(id) { if (!w.preview && !w.sample && w.room) w.room.pinMessage(id, false) }
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
    // the snippet kinds that lead with an icon (the attachment's)
    function kindIcon(k) {
        return k === "image" ? "image-x-generic" : k === "video" ? "video-x-generic" : k === "audio" ? "audio-x-generic"
            : k === "voice" ? "audio-input-microphone-symbolic" : k === "file" ? "text-x-generic"
            : k === "sticker" ? "face-smile-symbolic" : k === "encrypted" ? "channel-secure-symbolic" : ""
    }

    implicitHeight: bodyCol.implicitHeight

    Column {
        id: bodyCol
        x: 6; width: parent.width - 12
        // no room open
        WidgetText {
            visible: !w.model.room
            width: parent.width; height: visible ? 44 : 0
            horizontalAlignment: Text.AlignHCenter
            role: "meta"
            text: "Open a room to see its pins"
            color: Th.widgetMutedFg
        }
        // no pins
        Item {
            visible: !!w.model.room && w.model.count === 0
            width: parent.width; height: visible ? Math.max(56, emptyCol.implicitHeight) : 0
            Column {
                id: emptyCol
                anchors.centerIn: parent
                spacing: 2
                WidgetText { anchors.horizontalCenter: parent.horizontalCenter; role: "text"; text: "No pins in this room"; color: Th.widgetMetaFg }
                WidgetText { anchors.horizontalCenter: parent.horizontalCenter; role: "meta"; text: "Pin a message from its menu to keep it here."; color: Th.widgetMutedFg }
            }
        }
        // pins, none resolved yet
        WidgetText {
            visible: !!w.model.room && w.model.count > 0 && w.shown.length === 0
            width: parent.width; height: visible ? 44 : 0
            horizontalAlignment: Text.AlignHCenter
            role: "meta"
            text: w.model.loading ? "Loading pins…" : "Can't show these pins"
            color: Th.widgetMutedFg
        }
        Repeater {
            model: w.model.room ? w.shown.length : 0
            delegate: PinRow { required property int index; width: bodyCol.width; r: w.shown[index] }
        }
        // + n more (L shows 8, M 3)
        WidgetText {
            visible: !!w.model.room && w.shown.length > 0 && w.model.count > w.limit
            x: 8
            width: parent.width - 16; height: visible ? 22 : 0
            role: "meta"
            text: "+ " + (w.model.count - w.limit) + " more"
            color: Th.widgetMutedFg
        }
    }

    // ── a pin: 40, padding 0 8, radius 8, gap 8: avatar 20 · name + time over the snippet; ⋯ over the time on hover ──
    component PinRow: Rectangle {
        id: pr
        property var r: ({ id: "", sender: "", senderName: "", avatar: "", ts: 0, kind: "text", text: "" })
        readonly property bool quiet: pr.r.kind === "deleted" || pr.r.kind === "encrypted"
        readonly property string icon: w.kindIcon(pr.r.kind)
        readonly property bool hot: (prHover.hovered || (rowMenu.opened && !!rowMenu.pin && rowMenu.pin.id === pr.r.id)) && !w.preview
        readonly property string key: String(pr.r.id)
        onKeyChanged: if (w.isFresh(pr.key)) prIn.play()
        Component.onCompleted: if (w.isFresh(pr.key)) prIn.play()
        SlideIn { id: prIn; target: pr }
        height: 40
        radius: 8
        color: pr.hot ? Th.widgetRowBgHover : "transparent"
        Accessible.role: Accessible.Button
        Accessible.name: (pr.r.senderName || "") + ": " + (pr.r.text || "")
        HoverHandler { id: prHover; enabled: !w.preview }
        MouseArea {
            anchors.fill: parent
            enabled: !w.preview
            acceptedButtons: Qt.LeftButton | Qt.RightButton
            cursorShape: Qt.PointingHandCursor
            onClicked: (m) => { if (m.button === Qt.RightButton) rowMenu.openFor(pr.r, null); else w.jump(pr.r.id) }
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
                seed: pr.r.sender || ""
                name: pr.r.senderName || ""
                imageSource: pr.r.avatar || ""
            }
            Column {
                Layout.fillWidth: true
                Layout.alignment: Qt.AlignVCenter
                RowLayout {
                    width: parent.width
                    spacing: 8
                    WidgetText { Layout.fillWidth: true; role: "text"; text: pr.r.senderName || ""; color: Th.widgetFg }
                    WidgetText { role: "num"; text: w.whenText(pr.r.ts || 0); color: Th.widgetMetaFg; opacity: pr.hot ? 0 : 1 }
                }
                RowLayout {
                    width: parent.width
                    spacing: 4
                    ThIcon {
                        visible: pr.icon.length > 0
                        Layout.preferredWidth: 12; Layout.preferredHeight: 12
                        Layout.alignment: Qt.AlignVCenter
                        app: w.app
                        name: pr.icon
                        color: pr.quiet ? Th.widgetMutedFg : Th.widgetMetaFg
                    }
                    WidgetText { Layout.fillWidth: true; role: "meta"; text: pr.r.text || ""; color: pr.quiet ? Th.widgetMutedFg : Th.widgetMetaFg }
                }
            }
        }
        // ⋯ in the time's place while the row is hovered (22 × 22, radius 6, 4 from the right, 1 from the top)
        Controls.AbstractButton {
            id: moreBtn
            visible: pr.hot
            x: pr.width - 4 - width; y: 1
            width: 22; height: 22
            hoverEnabled: true
            focusPolicy: Qt.TabFocus
            Accessible.name: "Pinned message options"
            Controls.ToolTip.text: "More"
            Controls.ToolTip.visible: hovered && !rowMenu.opened
            Controls.ToolTip.delay: 500
            background: Rectangle {
                radius: 6
                color: (moreBtn.hovered || moreBtn.pressed || (rowMenu.opened && !!rowMenu.pin && rowMenu.pin.id === pr.r.id)) ? Th.widgetMenuBgHover : "transparent"
                border.width: moreBtn.visualFocus ? 1 : 0
                border.color: Th.widgetRingFocus
            }
            contentItem: Item {
                ThIcon { anchors.centerIn: parent; width: 16; height: 16; app: w.app; name: "overflow-menu-symbolic"; color: Th.widgetMenuIcon }
            }
            onClicked: rowMenu.opened ? rowMenu.close() : rowMenu.openFor(pr.r, moreBtn)
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

    // ── ⋯ / right-click: Jump to message · Unpin (when permitted) ──
    Controls.Menu {
        id: rowMenu
        modal: false
        property var pin: null
        function openFor(p, anchor) {
            rowMenu.pin = p
            if (anchor) rowMenu.popup(anchor, 0, anchor.height)
            else rowMenu.popup()
        }
        Controls.MenuItem { text: "Jump to message"; icon.name: "go-next-symbolic"; onTriggered: if (rowMenu.pin) w.jump(rowMenu.pin.id) }
        Controls.MenuItem {
            text: "Unpin"
            icon.name: "window-pin-symbolic"
            visible: w.model.canUnpin
            height: visible ? implicitHeight : 0
            onTriggered: if (rowMenu.pin) w.unpin(rowMenu.pin.id)
        }
    }

    // ── the sample states (the gallery's preview, the review screenshots, the gates): the web's samplePins, same rows ──
    // local time `daysAgo` days back at h:m — the samples' times read the same all day (today 6:00 PM, Yesterday, a
    // weekday, a date), so two captures of a sample state never differ by the clock (the web's localAt)
    function localAt(daysAgo, h, m) {
        var d = new Date()
        d.setDate(d.getDate() - daysAgo)
        d.setHours(h, m, 0, 0)
        return d.getTime()
    }
    function sampleModel(key) {
        var sp = function (id) { return "@" + id + ":sample.invalid" }
        var pin = function (id, who, name, ts, kind, text) { return { id: id, sender: sp(who), senderName: name, avatar: "", ts: ts, kind: kind, text: text } }
        var all = [
            pin("$p1", "ana", "Ana", w.localAt(0, 18, 0), "text", "Tonight: bring potions and the good snacks"),
            pin("$p2", "bo", "Bo", w.localAt(1, 20, 15), "image", "boss-route.png"),
            pin("$p3", "cy", "Cy", w.localAt(3, 12, 0), "text", "Raid starts 9 PM sharp — meet at the portal"),
            pin("$p4", "dee", "Dee", w.localAt(4, 12, 0), "voice", "Voice message"),
            pin("$p5", "eli", "Eli", w.localAt(9, 12, 0), "file", "patch-notes.pdf"),
            pin("$p6", "fay", "Fay", w.localAt(12, 12, 0), "encrypted", "Encrypted message"),
            pin("$p7", "gus", "Gus", w.localAt(20, 12, 0), "video", "clutch-1v4.mp4"),
            pin("$p8", "hal", "Hal", w.localAt(40, 12, 0), "text", "House rules: no spoilers in #general")
        ]
        var count = key === "many" ? 12 : key === "one" ? 1 : (key === "empty" || key === "noroom") ? 0 : 5
        return {
            room: key === "noroom" ? "" : "!lounge:sample.invalid",
            roomName: "Game lounge",
            count: count,
            rows: key === "loading" ? [] : all.slice(0, Math.min(count, 8)),
            loading: key === "loading",
            canUnpin: true
        }
    }
}
