// ButtonRow.qml — a row of action buttons (ACTIONS.md §9; web src/actions/ui/ButtonRow.tsx): a layout row value (the
// layout's opts.<island>.rows.<row>, or the preset's default row — ActionsController.defaultRow) resolved against the
// live snapshot by the shared engine (ActionsController.row → crates/actions) and drawn with ActionButton. Hidden items
// are left out (edit mode draws them faint); `sepBefore` draws a divider. Clicking runs the item through the one run
// path (app.actions.run: present → pick / confirm → the command), with its stored target, the row's context and the
// button as the anchor for menus and popovers.
//
//   ButtonRow { app: root; rowKind: "buttons.main"; value: islandOpts.rows.main }
//   ButtonRow { app: root; rowKind: "composer.end"; value: …; context: ({ room: roomId }) }
//
// Low GPU: the row re-resolves only when the snapshot TEXT or its inputs change (string compare), and a re-resolve that
// changes nothing visible repaints nothing (the delegates keep their identity: the model is the item count).
import QtQuick
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: action.separator

Item {
    id: br
    required property var app
    property string rowKind: "buttons.main"
    property var value: null                // the stored row value (any JSON; the engine normalizes it)
    property var context: null              // the subjects the host supplies ({ room: … })
    property bool editing: false
    property bool vertical: false
    property bool wrap: false               // your own button islands: wrap onto more lines when narrow
    // run an item yourself (tests); default: app.actions.run
    property var runner: null

    // evaluate against this snapshot instead of the live one (previews, tests); "" = the live one
    property string snapshotOverride: ""

    readonly property var ctl: app && app.actionsCtl ? app.actionsCtl : null
    readonly property string snapshotJson: snapshotOverride.length > 0 ? snapshotOverride
                                           : (app && app.actions ? app.actions.snapshotJson : "{}")
    readonly property string valueJson: JSON.stringify(value === undefined ? null : value)
    readonly property string contextJson: JSON.stringify(context === undefined ? null : context)
    readonly property string resultJson: ctl ? ctl.row(rowKind, valueJson, snapshotJson, contextJson) : ""
    property var result: ({ items: [], look: "icon", scale: "S", style: "tool" })
    function _parse() {
        try { result = resultJson.length > 0 ? JSON.parse(resultJson) : ({ items: [], look: "icon", scale: "S", style: "tool" }) }
        catch (e) { result = ({ items: [], look: "icon", scale: "S", style: "tool" }) }
    }
    onResultJsonChanged: _parse()
    Component.onCompleted: _parse()

    readonly property int count: result.items ? result.items.length : 0
    // the items drawn (hidden ones left out; edit mode draws them all)
    readonly property int shownCount: {
        var n = 0, its = br.result.items || []
        for (var i = 0; i < its.length; i++) if (br.editing || its[i].state !== "hidden") n++
        return n
    }
    // 11.0: the width of the shown buttons on ONE line (a button island reports it: a floating island is as wide as its
    // buttons, whatever the width it got) — measured from the delegates, coalesced
    property real oneLineWidth: 0
    function _measureLine() {
        var w = 0, n = 0
        for (var i = 0; i < rep.count; i++) {
            var c = rep.itemAt(i)
            if (c && c.visible) { w += c.implicitWidth; n++ }
        }
        br.oneLineWidth = w + Math.max(0, n - 1) * flow.spacing
    }
    property bool _measurePending: false
    function _remeasure() {
        if (br._measurePending) return
        br._measurePending = true
        Qt.callLater(function () { br._measurePending = false; br._measureLine() })
    }
    implicitWidth: flow.implicitWidth
    implicitHeight: flow.implicitHeight
    visible: count > 0

    function runItem(it, anchor) {
        if (runner) { runner(it, anchor); return }
        if (app && app.actions) app.actions.run(it.a, { source: "button", target: it.target, context: br.context, anchor: anchor })
    }

    Flow {
        id: flow
        width: br.wrap ? br.width : implicitWidth
        flow: br.vertical ? Flow.TopToBottom : Flow.LeftToRight
        spacing: Kirigami.Units.smallSpacing
        Repeater {
            id: rep
            model: br.count
            onCountChanged: br._remeasure()
            delegate: Row {
                id: cell
                required property int index
                readonly property var it: br.result.items[index] || ({})
                visible: br.editing || it.state !== "hidden"
                spacing: Kirigami.Units.smallSpacing
                onImplicitWidthChanged: br._remeasure()
                onVisibleChanged: br._remeasure()
                Component.onCompleted: br._remeasure()
                // sepBefore: a 1 × 30 divider (the call bar's before Leave)
                Rectangle {
                    visible: cell.it.sepBefore === true
                    anchors.verticalCenter: parent.verticalCenter
                    width: br.vertical ? 30 : 1
                    height: br.vertical ? 1 : 30
                    color: Th.actionSeparator
                }
                ActionButton {
                    app: br.app
                    item: cell.it
                    rowStyle: br.result.style || "tool"
                    rowScale: br.result.scale || "S"
                    toggleLook: br.rowKind === "buttons.main" ? "checked" : "pose"
                    editing: br.editing
                    onRun: (anchor) => br.runItem(cell.it, anchor)
                    onHold: (anchor, down) => { if (br.app && br.app.actions) br.app.actions.run(cell.it.a, { source: "button", target: cell.it.target, context: br.context, anchor: anchor, hold: down ? "down" : "up" }) }
                }
            }
        }
    }
}
