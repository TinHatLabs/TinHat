// RowModel.qml — one button row of a surface, resolved for drawing (11.0 buttons, ACTIONS.md §9; the web's
// src/actions/ui/rowHost.tsx useRow): the row stored in this device's layout form (LayoutStore.qml:
// opts.<island>.rows.<row>) or the preset's default (ActionsController.defaultRow), evaluated against the live snapshot
// by the shared engine (ActionsController.row → crates/actions).
//
// Hosts keep TODAY's elements for the actions they draw themselves (`bespoke`: the header's round buttons, the call
// bar's, the composer's Send …) and place them with GridLayout columns — `colOf(a)` is the column of an action's first
// shown occurrence, -1 = not shown (hidden, or not in the row) — so a reorder moves the same element, never a copy;
// every other item (added from the editor, or every item once the row's look / size is not the row kind's default) is
// a cell of `generics` (the host draws ActionButton at `col`); `seps` = the dividers (sepBefore). Columns start at
// `firstColumn` (the host's own items before the row: the header's avatar and title). With the default rows the columns
// are today's order, so the default configuration paints what 10.8 painted (the gates prove it).
//
// Low GPU: nothing ticks — the row re-resolves when the snapshot TEXT, the stored value or the context change.
import QtQuick

QtObject {
    id: rm
    required property var app
    property string islandRef: ""       // (not `island`: several hosts' root ids are `island`)
    property string rowId: ""
    property string form: "desktop"
    property var context: null
    property var bespoke: []              // the action ids the host draws itself
    property int firstColumn: 0
    property string snapshotOverride: ""   // evaluate against this snapshot (the call rows: decision 2), "" = the live one

    readonly property string kind: String(islandRef).split(":")[0] + "." + rowId
    readonly property var ctl: app && app.actionsCtl ? app.actionsCtl : null
    // the stored row (LayoutStore: the device pref layout-desktop until edit mode syncs it), else the default
    readonly property string storedJson: app && app.layoutStore ? app.layoutStore.rowJson(islandRef, rowId, form) : ""
    readonly property string valueJson: storedJson.length > 0 ? storedJson
                                        : (ctl ? String(ctl.defaultRow(islandRef, rowId, form, "classic")) : "null")
    readonly property string snapshotJson: snapshotOverride.length > 0 ? snapshotOverride
                                           : (app && app.actions ? app.actions.snapshotJson : "{}")
    readonly property string contextJson: JSON.stringify(context === undefined || context === null ? null : context)
    readonly property string resultJson: ctl ? String(ctl.row(kind, valueJson, snapshotJson, contextJson)) : ""
    readonly property bool stored: storedJson.length > 0

    property var result: ({ items: [], look: "icon", scale: "S", style: "tool", editable: false })
    property var cells: []            // [{col, sep, a, i, item, bespoke}] — shown items in order (+ their dividers)
    property var generics: []         // cells the host draws with ActionButton
    property var seps: []             // divider cells
    property var _cols: ({})          // action id → column of its bespoke element
    property var _items: ({})         // action id → that occurrence's ItemOut
    property bool defaultLook: true
    property int lastColumn: firstColumn - 1

    function colOf(a) { return rm._cols[a] !== undefined ? rm._cols[a] : -1 }
    function shows(a) { return rm.colOf(a) >= 0 }
    function itemOf(a) { return rm._items[a] || null }
    function dimmed(a) { var it = rm._items[a]; return !!it && it.state === "dimmed" }

    function _rowKindDefaults() {
        var t = app && app.actionsTable ? app.actionsTable : null
        var rk = t && t.rows ? t.rows[rm.kind] : null
        return rk ? rk : { look: "icon", scale: "S" }
    }
    function _parse() {
        var r
        try { r = resultJson.length > 0 ? JSON.parse(resultJson) : null } catch (e) { r = null }
        if (!r || r.error || !r.items) r = { items: [], look: "icon", scale: "S", style: "tool", editable: false }
        var rk = rm._rowKindDefaults()
        var dl = r.look === rk.look && r.scale === rk.scale
        var cells = [], gens = [], seps = [], cols = ({}), items = ({})
        var col = rm.firstColumn
        for (var i = 0; i < r.items.length; i++) {
            var it = r.items[i]
            if (it.state === "hidden") continue
            if (it.sepBefore === true) { var sc = { col: col++, sep: true, a: "", i: i, item: null, bespoke: false }; cells.push(sc); seps.push(sc) }
            var own = dl && it.known === true && it.target === null && rm.bespoke.indexOf(it.a) >= 0 && cols[it.a] === undefined
            var c = { col: col++, sep: false, a: it.a, i: i, item: it, bespoke: own }
            cells.push(c)
            if (own) { cols[it.a] = c.col; items[it.a] = it } else gens.push(c)
        }
        rm.defaultLook = dl
        rm._cols = cols
        rm._items = items
        rm.cells = cells
        rm.generics = gens
        rm.seps = seps
        rm.lastColumn = col - 1
        rm.result = r
    }
    onResultJsonChanged: _parse()
    onBespokeChanged: _parse()
    onFirstColumnChanged: _parse()
    Component.onCompleted: _parse()

    /** Run one item of this row through the one run path (its stored target, the row's context, the button as the anchor). */
    function run(it, anchor, extra) {
        if (!app || !app.actions || !it) return
        var inv = { source: "button", target: it.target || null, context: rm.context || null, anchor: anchor || null }
        if (extra) for (var k in extra) inv[k] = extra[k]
        app.actions.run(it.a, inv)
    }
    /** "Edit buttons…" — the row editor (ActionsHost.openRowEditor). */
    function edit(title) { if (app && app.actions) app.actions.openRowEditor({ island: rm.islandRef, row: rm.rowId, form: rm.form, title: title }) }
}
