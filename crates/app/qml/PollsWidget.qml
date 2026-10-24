// PollsWidget.qml — widgets W3 (2026-09-28): Polls (`polls`, contract WIDGETS.md §17, WIDGETS-UI.md §17; the web's
// features/widgets/polls/Polls.tsx).
//
// The open polls of the room on screen, from its loaded timeline (RoomController.pollsJson: every start in the window
// tallied with its responses and ends by the shared logic), newest first: M 3, L 8, then "+ n more"; each row = the
// question over the shared row line ("3 votes · you voted"); a click jumps to the poll in the chat (the pinned widget's
// jump), where the card votes and ends. "New" (the header) / "New poll" (the empty state) open NewPollDialog for the
// room. S / collapsed: the shared pollsSummary (PollsController.summary). No timer of its own.
// Gate / review: TINHAT_WIDGETS_SCENARIO "polls=<key>" — data, many, empty, noroom — the web's samples (PollsController).
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

    PollsController { id: pollsCtl }

    // ── the model (the web's usePollsModel) ──
    readonly property string roomId: w.room ? String(w.room.roomId || "") : ""
    readonly property var realRows: {
        var raw = w.room ? String(w.room.pollsJson || "") : ""
        try { return raw.length > 0 ? JSON.parse(raw) : [] } catch (e) { return [] }
    }
    readonly property var realModel: ({
        room: w.roomId,
        canNew: w.roomId.length > 0 && !!w.room && w.room.canSendPoll === true,
        rows: w.realRows,
        sample: false
    })
    readonly property string scenario: w.wc ? String(w.wc.scenarioOf("polls") || "") : ""
    readonly property bool sample: w.scenario.length > 0 || (w.preview && w.realRows.length === 0)
    readonly property var model: w.sample ? w.sampleModel(w.scenario.length > 0 ? w.scenario : "preview") : w.realModel
    readonly property var shown: w.model.rows.slice(0, w.limit)
    readonly property int more: Math.max(0, w.model.rows.length - w.shown.length)
    readonly property bool live: false

    function sampleModel(key) {
        var rows = []
        try { rows = JSON.parse(String(pollsCtl.sampleRows(key) || "[]")) } catch (e) { rows = [] }
        return { room: key === "noroom" ? "" : "!lounge:sample.invalid", canNew: key !== "noroom", rows: rows, sample: true }
    }
    function jump(id) {
        if (w.preview || w.sample || !w.app || String(id).charAt(0) !== "$") return
        w.app.jumpToEvent(w.model.room, id)
    }
    function openNew() {
        if (w.preview || w.sample || !w.model.canNew) return
        newDialog.open()
    }

    readonly property string summaryText: w.model.room.length === 0 ? "No room open"
        : String(pollsCtl.summary(w.model.rows.length, w.model.rows.length > 0 ? String(w.model.rows[0].question) : ""))
    readonly property Component summaryComponent: Component { WidgetHeaderText { text: w.summaryText } }
    // M / L header: "+ New"
    readonly property Component extraComponent: Component {
        NewButton { visible: !w.preview; label: false }
    }

    // rowSlide: a poll that newly enters the list (after the first fill) — opacity 0→1, y +4→0 (long, OutCubic)
    readonly property var keys: w.shown.map(function (r) { return String(r.id) })
    property var prevKeys: null
    onKeysChanged: Qt.callLater(function () { w.prevKeys = w.keys })
    Component.onCompleted: w.prevKeys = w.keys
    function isFresh(k) { return w.prevKeys !== null && w.prevKeys.indexOf(k) < 0 }

    implicitHeight: bodyCol.implicitHeight

    Column {
        id: bodyCol
        x: 6; width: parent.width - 12
        spacing: 0
        // ── no room ──
        Item {
            visible: w.model.room.length === 0
            width: parent.width; height: visible ? 44 : 0
            WidgetText { anchors.centerIn: parent; role: "meta"; text: "Open a room to see its polls"; color: Th.widgetMutedFg }
        }
        // ── empty ──
        Item {
            visible: w.model.room.length > 0 && w.shown.length === 0
            width: parent.width; height: visible ? Math.max(56, emptyCol.implicitHeight + 10) : 0
            Column {
                id: emptyCol
                anchors.centerIn: parent
                width: parent.width - 16
                spacing: 2
                WidgetText { anchors.horizontalCenter: parent.horizontalCenter; role: "text"; text: "No open polls"; color: Th.widgetMetaFg }
                WidgetText {
                    width: parent.width
                    horizontalAlignment: Text.AlignHCenter
                    wrapMode: Text.Wrap
                    role: "meta"
                    text: "Ask the room something — everyone votes right in the chat."
                    color: Th.widgetMutedFg
                }
                Item { width: 1; height: 4; visible: !w.preview }
                NewButton { visible: !w.preview; anchors.horizontalCenter: parent.horizontalCenter; label: true }
            }
        }
        // ── the rows ──
        Repeater {
            model: w.shown.length
            delegate: PollRow { required property int index; width: bodyCol.width; r: w.shown[index] }
        }
        Item {
            visible: w.more > 0
            width: parent.width; height: visible ? 22 : 0
            WidgetText { x: 8; anchors.verticalCenter: parent.verticalCenter; role: "meta"; text: "+ " + w.more + " more"; color: Th.widgetMutedFg }
        }
    }

    NewPollDialog {
        id: newDialog
        app: w.app
        roomCtl: w.room
    }

    // ── a row: 40, padding 0 8, radius 8, gap 8: the glyph (14) · the question over the row line; click → the poll ──
    component PollRow: Rectangle {
        id: pr
        property var r: null
        readonly property string key: pr.r ? String(pr.r.id) : ""
        onKeyChanged: if (w.isFresh(pr.key)) prIn.play()
        Component.onCompleted: if (w.isFresh(pr.key)) prIn.play()
        SlideIn { id: prIn; target: pr }
        height: 40
        radius: 8
        color: prMa.containsMouse && !w.preview ? Th.widgetRowBgHover : "transparent"
        border.width: prMa.activeFocus ? 1 : 0
        border.color: Th.widgetRingFocus
        Accessible.role: Accessible.Button
        Accessible.name: pr.r ? pr.r.question + " — " + pr.r.line + ". Open the poll" : ""
        MouseArea {
            id: prMa
            anchors.fill: parent
            enabled: !w.preview
            hoverEnabled: true
            activeFocusOnTab: !w.preview
            cursorShape: Qt.PointingHandCursor
            onClicked: if (pr.r) w.jump(pr.r.id)
            Keys.onPressed: (k) => { if ((k.key === Qt.Key_Return || k.key === Qt.Key_Enter || k.key === Qt.Key_Space) && pr.r) { w.jump(pr.r.id); k.accepted = true } }
            Controls.ToolTip.text: "Open the poll"
            Controls.ToolTip.visible: containsMouse && !w.preview
            Controls.ToolTip.delay: 500
        }
        RowLayout {
            anchors.fill: parent
            anchors.leftMargin: 8; anchors.rightMargin: 8
            spacing: 8
            ThIcon {
                Layout.preferredWidth: 14; Layout.preferredHeight: 14
                Layout.alignment: Qt.AlignVCenter
                app: w.app
                name: "view-statistics-symbolic"
                color: Th.widgetPollsMetaFg
            }
            ColumnLayout {
                Layout.fillWidth: true
                Layout.alignment: Qt.AlignVCenter
                spacing: 0
                WidgetText { Layout.fillWidth: true; role: "text"; text: pr.r ? pr.r.question : ""; color: Th.widgetFg; elide: Text.ElideRight }
                WidgetText { Layout.fillWidth: true; role: "meta"; text: pr.r ? pr.r.line : ""; color: Th.widgetMetaFg; elide: Text.ElideRight }
            }
        }
    }

    // "+ New" (the header, 20 high) / "+ New poll" (the empty state, 24 high): widget.polls.new.* + a tooltip
    component NewButton: Controls.AbstractButton {
        id: nb
        property bool label: false
        enabled: w.model.room.length > 0 && w.model.canNew
        hoverEnabled: true
        focusPolicy: Qt.TabFocus
        implicitWidth: nbRow.implicitWidth + (nb.label ? 20 : 14)
        implicitHeight: nb.label ? 24 : 20
        opacity: nb.enabled ? 1 : 0.45
        Accessible.name: "New poll"
        Controls.ToolTip.text: w.model.room.length === 0 ? "Open a room to start a poll" : !w.model.canNew ? "You can't start polls in this room" : "Start a poll in this room"
        Controls.ToolTip.visible: nb.hovered
        Controls.ToolTip.delay: 500
        background: Rectangle {
            radius: 6
            color: nb.enabled && (nb.hovered || nb.down) ? Th.widgetPollsNewBgHover : "transparent"
            border.width: nb.visualFocus ? 1 : 0
            border.color: Th.widgetRingFocus
        }
        contentItem: Item {
            Row {
                id: nbRow
                anchors.centerIn: parent
                spacing: 4
                ThIcon {
                    anchors.verticalCenter: parent.verticalCenter
                    width: 12; height: 12
                    app: w.app
                    name: "list-add-symbolic"
                    color: nb.enabled && nb.hovered ? Th.widgetPollsNewFgHover : Th.widgetPollsNewFg
                }
                WidgetText {
                    anchors.verticalCenter: parent.verticalCenter
                    role: nb.label ? "meta" : "chip"
                    semibold: true
                    text: nb.label ? "New poll" : "New"
                    color: nb.enabled && nb.hovered ? Th.widgetPollsNewFgHover : Th.widgetPollsNewFg
                }
            }
        }
        onClicked: w.openNew()
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
}
