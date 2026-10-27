// RowEditor.qml — edit one button row (11.0 buttons; the web's src/actions/ui/RowEditor.tsx — the same parts, the same
// numbers). DESIGN.md "Buttons": hide / reorder / add today's buttons, your own button islands, the look per row, and
// per button "hide when it doesn't apply". A simple per-row editor until edit mode (step 4) hosts it: a live preview of
// the row as it will look, its Look (icons / icons and labels / labels) and Size (small / medium), the buttons in order
// (▲ ▼ −, "Hide when it doesn't apply"; a newer Tin Hat's button is kept as it is) and the drawer of every action that
// may go into this row (ACTIONS.md §9.6, ActionsController.drawer) — an action that needs a target asks for it (the
// palette's stage-2 choices, §10.6) or can pick it each time ("Ask every time"). Nothing is saved until Done
// (LayoutStore.setRow); Reset to default puts the preset's row back; the default row itself is never stored.
//
// One instance (ActionsHost.qml), opened with openFor({island, row, form, title}). Colours: the dialog's tokens and the
// surface roles (the web uses the same: surface.sunken / surface.raised / surface.hover, dialog.*).
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the active theme (docs/MIGRATING-THEME.md)

Controls.Dialog {
    id: dlg
    property var app: null
    property var req: null                 // {island, row, form, title}
    title: LinkGate.plain(dlg.req ? "Edit buttons — " + dlg.req.title : "Edit buttons")
    modal: true
    parent: Controls.Overlay.overlay
    anchors.centerIn: parent
    width: Math.min(720, (parent ? parent.width : 720) - Kirigami.Units.gridUnit * 2)
    // a stable height (the lists scroll inside it): the dialog never jumps while you add, remove or pick, and it does not
    // depend on a style's implicit-size rules (Breeze's Dialog kept the height it opened with)
    height: Math.min(660, (parent ? parent.height : 800) - Kirigami.Units.gridUnit * 2)
    closePolicy: Controls.Popup.CloseOnEscape      // not on a click outside (the web's closeOnOutside: false)
    standardButtons: Controls.Dialog.Reset | Controls.Dialog.Cancel | Controls.Dialog.Ok

    // ---- the working copy ------------------------------------------------------------------------------------------
    property string kind: ""                // the row kind (header.main, buttons.main …)
    property var rowKindDef: ({ look: "icon", scale: "S" })
    property string lookV: "icon"
    property string sizeV: "S"
    property var work: []                   // the row's items [{a, target?, hideWhenOff?, sepBefore?}]
    property string defaultJson: "null"     // the preset's default row (JSON; "null" = none: a button island)
    property var picking: null              // {a, label} while choosing a target
    property string query: ""
    property string foldedQuery: ""

    readonly property var ctl: dlg.app && dlg.app.actionsCtl ? dlg.app.actionsCtl : null
    readonly property string snapshotJson: dlg.app && dlg.app.actions ? dlg.app.actions.snapshotJson : "{}"
    readonly property int maxItems: (dlg.app && dlg.app.actionsTable && dlg.app.actionsTable.limits) ? (dlg.app.actionsTable.limits.rowItems || 24) : 24
    readonly property bool full: dlg.work.length >= dlg.maxItems

    /** The working copy → a row value (look / scale only when they differ from the row kind's). */
    function valueOf(look, size, items) {
        var v = {}
        if (look !== dlg.rowKindDef.look) v.look = look
        if (size !== dlg.rowKindDef.scale) v.scale = size
        v.items = items.map(function (x) { return JSON.parse(JSON.stringify(x)) })
        return v
    }
    /** A row value → [look, size, items] (the row kind's defaults filled in). */
    function workOf(v) {
        var o = (v && typeof v === "object" && !Array.isArray(v)) ? v : {}
        var lk = (o.look === "icon" || o.look === "iconLabel" || o.look === "label") ? o.look : dlg.rowKindDef.look
        var sz = (o.scale === "S" || o.scale === "M") ? o.scale : dlg.rowKindDef.scale
        var its = Array.isArray(o.items) ? o.items.filter(function (x) { return x && typeof x === "object" && !Array.isArray(x) })
                                                  .map(function (x) { return JSON.parse(JSON.stringify(x)) }) : []
        return [lk, sz, its]
    }
    readonly property string valueJson: JSON.stringify(dlg.valueOf(dlg.lookV, dlg.sizeV, dlg.work))
    readonly property string defaultValueJson: {
        var d = JSON.parse(dlg.defaultJson)
        if (d === null) return "null"
        var w = dlg.workOf(d)
        return JSON.stringify(dlg.valueOf(w[0], w[1], w[2]))
    }
    readonly property bool isDefault: dlg.defaultJson === "null" ? dlg.work.length === 0 : dlg.valueJson === dlg.defaultValueJson

    // every item's face now (labels, icons, targets, known) — hidden ones included
    readonly property var faces: {
        if (!dlg.ctl || dlg.kind.length === 0 || !dlg.visible) return []
        try { return JSON.parse(String(dlg.ctl.row(dlg.kind, dlg.valueJson, dlg.snapshotJson, "null"))).items || [] } catch (e) { return [] }
    }
    // the drawer of the row kind (§9.6): [{group, label, items: [{a, label, icon, pick}]}]
    readonly property var groups: {
        if (!dlg.ctl || dlg.kind.length === 0 || !dlg.visible) return []
        try { return JSON.parse(String(dlg.ctl.drawer(dlg.kind, dlg.snapshotJson))).groups || [] } catch (e) { return [] }
    }
    // the picked action's choices (stage 2's entries, §10.6)
    readonly property var choices: {
        if (!dlg.ctl || !dlg.picking || !dlg.visible) return []
        try {
            var r = JSON.parse(String(dlg.ctl.call("choices", JSON.stringify({ app: "desktop", snapshot: JSON.parse(dlg.snapshotJson), action: dlg.picking.a }))))
            return r.items || []
        } catch (e) { return [] }
    }
    /** The engine's fold (ACTIONS.md §10.3) — the search matches the way the palette does. */
    property var _foldCache: ({})
    function fold(s) {
        var k = String(s || "")
        if (dlg._foldCache[k] !== undefined) return dlg._foldCache[k]
        var f = k.toLowerCase()
        try { f = JSON.parse(String(dlg.ctl.call("fold", JSON.stringify({ text: k })))).folded || "" } catch (e) {}
        dlg._foldCache[k] = f
        return f
    }
    function matches(label) { return dlg.foldedQuery.length === 0 || dlg.fold(label).indexOf(dlg.foldedQuery) >= 0 }
    onQueryChanged: dlg.foldedQuery = dlg.query.length > 0 ? dlg.fold(dlg.query) : ""

    /** Open the editor for one row: {island, row, form, title}. */
    function openFor(r) {
        dlg.req = r
        var ref = String(r.island)
        dlg.kind = ref.split(":")[0] + "." + String(r.row)
        var t = dlg.app && dlg.app.actionsTable ? dlg.app.actionsTable : null
        var rk = t && t.rows ? t.rows[dlg.kind] : null
        dlg.rowKindDef = rk ? rk : ({ look: "icon", scale: "S" })
        dlg.defaultJson = dlg.ctl ? String(dlg.ctl.defaultRow(ref, String(r.row), "desktop", "classic")) : "null"
        var stored = dlg.app && dlg.app.layoutStore ? String(dlg.app.layoutStore.rowJson(ref, String(r.row), "desktop")) : ""
        var w = dlg.workOf(stored.length > 0 ? JSON.parse(stored) : JSON.parse(dlg.defaultJson))
        dlg.lookV = w[0]; dlg.sizeV = w[1]; dlg.work = w[2]
        dlg.picking = null
        dlg.query = ""
        searchField.text = ""
        dlg.open()
    }

    // ---- editing ---------------------------------------------------------------------------------------------------
    function _edit(fn) { var its = dlg.work.slice(); fn(its); dlg.work = its }
    function moveItem(i, d) { dlg._edit(function (its) { var j = i + d; if (j < 0 || j >= its.length) return; var x = its[i]; its[i] = its[j]; its[j] = x }) }
    function removeItem(i) { dlg._edit(function (its) { its.splice(i, 1) }) }
    function setFlag(i, k, on) { dlg._edit(function (its) { var x = JSON.parse(JSON.stringify(its[i])); if (on) x[k] = true; else delete x[k]; its[i] = x }) }
    function addItem(a, target) {
        if (dlg.full) return
        dlg._edit(function (its) {
            var x = { a: a }
            if (target && typeof target === "object" && Object.keys(target).length > 0) x.target = JSON.parse(JSON.stringify(target))
            its.push(x)
        })
    }
    function pickOrAdd(it) {
        if (it.pick === true && dlg.ctl) {
            var ch = null
            try { ch = JSON.parse(String(dlg.ctl.call("choices", JSON.stringify({ app: "desktop", snapshot: JSON.parse(dlg.snapshotJson), action: it.a })))).items } catch (e) { ch = null }
            if (ch !== null && ch !== undefined) { dlg.picking = { a: it.a, label: it.label }; dlg.query = ""; searchField.text = ""; return }
        }
        dlg.addItem(it.a, null)
    }
    function saveRow() {
        if (!dlg.req || !dlg.app || !dlg.app.layoutStore) return
        // the default row itself is not stored (the preset keeps it, and a later default change reaches it)
        var v = (dlg.defaultJson !== "null" && dlg.isDefault) ? null : JSON.parse(dlg.valueJson)
        var res = dlg.app.layoutStore.setRow(String(dlg.req.island), String(dlg.req.row), v)
        if (res !== "ok") console.warn("[buttons] saving the row failed:", res)
    }
    function resetToDefault() {
        var w = dlg.workOf(JSON.parse(dlg.defaultJson))
        dlg.lookV = w[0]; dlg.sizeV = w[1]; dlg.work = w[2]
    }
    onAccepted: dlg.saveRow()
    onReset: dlg.resetToDefault()
    onClosed: { dlg.picking = null; dlg.query = "" }

    // the button box: Reset to default · Cancel · Done (the style's own buttons, relabelled)
    function _labelButtons() {
        var ok = dlg.standardButton(Controls.Dialog.Ok)
        if (ok) ok.text = "Done"
        var rs = dlg.standardButton(Controls.Dialog.Reset)
        if (rs) { rs.text = "Reset to default"; rs.enabled = Qt.binding(function () { return !dlg.isDefault }) }
    }
    Component.onCompleted: dlg._labelButtons()
    onAboutToShow: dlg._labelButtons()

    readonly property var lookLabels: ({ icon: "Icons", iconLabel: "Icons and labels", label: "Labels" })
    readonly property var sizeLabels: ({ S: "Small", M: "Medium" })

    // ONE content child, filling the content item: the dialog places that item between its title and its buttons
    // (availableHeight still counts them — Breeze drew the lists under the buttons and past the frame)
    ColumnLayout {
        id: body
        anchors.fill: parent
        spacing: 10

        // ---- the preview: the row as it will look (clicks do nothing) ----
        Rectangle {
            Layout.fillWidth: true
            implicitHeight: Math.max(58, preview.implicitHeight + 16)
            radius: 8
            color: Th.surfaceSunken
            border.width: 1
            border.color: Th.dialogBorder
            ButtonRow {
                id: preview
                x: 8
                anchors.verticalCenter: parent.verticalCenter
                width: parent.width - 16
                visible: dlg.work.length > 0
                app: dlg.app
                rowKind: dlg.kind.length > 0 ? dlg.kind : "buttons.main"
                value: JSON.parse(dlg.valueJson)
                wrap: true
                runner: function (it, anchor) {}
            }
            Controls.Label {
                visible: dlg.work.length === 0
                x: 8
                anchors.verticalCenter: parent.verticalCenter
                text: "No buttons in this row"
                color: Th.dialogHintFg
                font.pointSize: Kirigami.Theme.smallFont.pointSize
            }
        }

        // ---- Look and Size ----
        Flow {
            Layout.fillWidth: true
            spacing: 24
            RowLayout {
                spacing: 12
                Controls.Label { text: "Look"; color: Th.dialogHintFg; font.pointSize: Kirigami.Theme.smallFont.pointSize; Layout.minimumWidth: 36 }
                Repeater {
                    model: ["icon", "iconLabel", "label"]
                    delegate: ThRadioButton {
                        required property string modelData
                        text: dlg.lookLabels[modelData]
                        checked: dlg.lookV === modelData
                        onToggled: if (checked) dlg.lookV = modelData
                    }
                }
            }
            RowLayout {
                spacing: 12
                Controls.Label { text: "Size"; color: Th.dialogHintFg; font.pointSize: Kirigami.Theme.smallFont.pointSize; Layout.minimumWidth: 36 }
                Repeater {
                    model: ["S", "M"]
                    delegate: ThRadioButton {
                        required property string modelData
                        text: dlg.sizeLabels[modelData]
                        checked: dlg.sizeV === modelData
                        onToggled: if (checked) dlg.sizeV = modelData
                    }
                }
            }
        }

        // ---- the two columns: the row's buttons | the drawer ----
        RowLayout {
            Layout.fillWidth: true
            Layout.fillHeight: true
            spacing: 16

            // In this row
            ColumnLayout {
                Layout.fillWidth: true
                Layout.fillHeight: true
                Layout.preferredWidth: 1
                Layout.alignment: Qt.AlignTop
                spacing: 6
                SectionLabel { text: "In this row" }
                HintLabel { visible: dlg.work.length === 0; Layout.fillWidth: true; text: "Nothing yet — add buttons from the list." }
                Flickable {
                    id: itemsFlick
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    contentHeight: itemsCol.implicitHeight
                    clip: true
                    boundsBehavior: Flickable.StopAtBounds
                    Controls.ScrollBar.vertical: FoxScrollBar { app: dlg.app; visible: itemsFlick.contentHeight > itemsFlick.height }
                    Column {
                        id: itemsCol
                        width: itemsFlick.width
                        spacing: 2
                        Repeater {
                            model: dlg.work.length
                            delegate: Rectangle {
                                id: itemRow
                                required property int index
                                readonly property var x0: dlg.work[index] || ({})
                                readonly property var face: dlg.faces[index] || null
                                readonly property bool known: !!itemRow.face && itemRow.face.known === true
                                width: itemsCol.width
                                implicitHeight: Math.max(40, itemLine.implicitHeight + 4)
                                radius: 5
                                color: Th.surfaceRaised
                                RowLayout {
                                    id: itemLine
                                    anchors.fill: parent
                                    anchors.leftMargin: 8
                                    anchors.rightMargin: 4
                                    spacing: 8
                                    // the glyph: an emoji (a status preset's), else the icon (the target's)
                                    Item {
                                        Layout.preferredWidth: 22
                                        Layout.preferredHeight: 22
                                        Layout.alignment: Qt.AlignVCenter
                                        Controls.Label {
                                            textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                            anchors.centerIn: parent
                                            visible: !!itemRow.face && !!itemRow.face.emoji
                                            text: itemRow.face && itemRow.face.emoji ? itemRow.face.emoji : ""
                                            font.pixelSize: 16
                                        }
                                        ThIcon {
                                            anchors.fill: parent
                                            readonly property string g: itemRow.face ? (itemRow.face.icon || itemRow.face.targetIcon || "") : ""
                                            visible: g.length > 0 && !(itemRow.face && itemRow.face.emoji)
                                            app: dlg.app
                                            name: g
                                            color: itemRow.known ? Th.buttonToolFg : Th.actionPlaceholderFg
                                            isMask: true
                                        }
                                    }
                                    ColumnLayout {
                                        // the leftover width only (preferred 0): the three tool buttons always fit
                                        Layout.fillWidth: true
                                        Layout.preferredWidth: 0
                                        Layout.topMargin: 4
                                        Layout.bottomMargin: 4
                                        spacing: 2
                                        Controls.Label {
                                            textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                            Layout.fillWidth: true
                                            text: !itemRow.face ? "" : (itemRow.known ? (itemRow.face.label || "") : (itemRow.face.label || "Needs a newer Tin Hat"))
                                            color: Th.dialogFg
                                            elide: Text.ElideRight
                                        }
                                        ThCheckBox {
                                            Layout.fillWidth: true
                                            visible: itemRow.known
                                            text: "Hide when it doesn't apply"
                                            font.pointSize: Kirigami.Theme.smallFont.pointSize
                                            checked: itemRow.x0.hideWhenOff === true
                                            onToggled: dlg.setFlag(itemRow.index, "hideWhenOff", checked)
                                        }
                                        Controls.Label {
                                            Layout.fillWidth: true
                                            wrapMode: Text.Wrap
                                            visible: !!itemRow.face && !itemRow.known
                                            text: "Made by a newer Tin Hat — kept as it is"
                                            color: Th.dialogHintFg
                                            font.pointSize: Kirigami.Theme.smallFont.pointSize
                                        }
                                    }
                                    ThToolButton {
                                        app: dlg.app
                                        iconName: "go-up-symbolic"
                                        tip: "Move left / up"
                                        enabled: itemRow.index > 0
                                        onClicked: dlg.moveItem(itemRow.index, -1)
                                    }
                                    ThToolButton {
                                        app: dlg.app
                                        iconName: "go-down-symbolic"
                                        tip: "Move right / down"
                                        enabled: itemRow.index < dlg.work.length - 1
                                        onClicked: dlg.moveItem(itemRow.index, 1)
                                    }
                                    ThToolButton {
                                        app: dlg.app
                                        iconName: "list-remove-symbolic"
                                        tip: "Take it out of the row"
                                        onClicked: dlg.removeItem(itemRow.index)
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Add a button (the drawer) — or, while picking, the target's choices
            ColumnLayout {
                Layout.fillWidth: true
                Layout.fillHeight: true
                Layout.preferredWidth: 1
                Layout.alignment: Qt.AlignTop
                spacing: 6
                RowLayout {
                    spacing: 4
                    ThToolButton {
                        visible: dlg.picking !== null
                        app: dlg.app
                        iconName: "go-previous-symbolic"
                        tip: "Back to every action"
                        onClicked: { dlg.picking = null; dlg.query = ""; searchField.text = "" }
                    }
                    SectionLabel { text: dlg.picking ? dlg.picking.label + " — pick one" : "Add a button" }
                }
                ThTextField {
                    id: searchField
                    Layout.fillWidth: true
                    placeholderText: dlg.picking ? "Search…" : "Search actions…"
                    onTextChanged: dlg.query = text
                }
                HintLabel { visible: dlg.full && !dlg.picking; Layout.fillWidth: true; text: "This row is full (" + dlg.maxItems + " buttons)." }
                Flickable {
                    id: drawerFlick
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    contentHeight: drawerCol.implicitHeight
                    clip: true
                    boundsBehavior: Flickable.StopAtBounds
                    Controls.ScrollBar.vertical: FoxScrollBar { app: dlg.app; visible: drawerFlick.contentHeight > drawerFlick.height }
                    Column {
                        id: drawerCol
                        width: drawerFlick.width
                        spacing: 1
                        // picking: "Ask every time", then the choices
                        DrawerRow {
                            app: dlg.app
                            visible: dlg.picking !== null
                            iconName: "view-more-symbolic"
                            label: "Ask every time"
                            enabled: !dlg.full
                            onPicked: { dlg.addItem(dlg.picking.a, null); dlg.picking = null; dlg.query = ""; searchField.text = "" }
                        }
                        Repeater {
                            model: dlg.picking !== null ? dlg.choices : []
                            delegate: DrawerRow {
                                required property var modelData
                                app: dlg.app
                                visible: dlg.matches(modelData.label)
                                emoji: modelData.emoji || ""
                                iconName: modelData.icon || "list-add-symbolic"
                                label: modelData.label
                                enabled: !dlg.full && modelData.enabled !== false
                                onPicked: { dlg.addItem(dlg.picking.a, modelData.target || null); dlg.picking = null; dlg.query = ""; searchField.text = "" }
                            }
                        }
                        // the drawer: every action that may go into this row, by group
                        Repeater {
                            model: dlg.picking === null ? dlg.groups : []
                            delegate: Column {
                                id: grp
                                required property var modelData
                                readonly property var shownItems: (modelData.items || []).filter(function (it) { return dlg.matches(it.label) })
                                visible: grp.shownItems.length > 0
                                width: drawerCol.width
                                spacing: 1
                                bottomPadding: 6
                                Controls.Label {
                                    textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                    leftPadding: 8
                                    topPadding: 4
                                    bottomPadding: 2
                                    text: grp.modelData.label
                                    color: Th.dialogHintFg
                                    font.pointSize: Kirigami.Theme.smallFont.pointSize
                                    font.bold: true
                                }
                                Repeater {
                                    model: grp.shownItems
                                    delegate: DrawerRow {
                                        required property var modelData
                                        app: dlg.app
                                        width: grp.width
                                        iconName: modelData.icon || "list-add-symbolic"
                                        label: modelData.label
                                        more: modelData.pick === true
                                        enabled: !dlg.full
                                        onPicked: dlg.pickOrAdd(modelData)
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // one 32 px drawer line: glyph (or emoji), label, "…" for an action that asks for a target
    component DrawerRow: Rectangle {
        id: dr
        property var app: null                 // (an inline component sees no id of this file)
        property string iconName: ""
        property string emoji: ""
        property string label: ""
        property bool more: false
        signal picked()
        width: parent ? parent.width : 200
        implicitHeight: 32
        radius: 5
        color: drHover.hovered && dr.enabled ? Th.surfaceHover : "transparent"
        opacity: dr.enabled ? 1 : 0.5
        activeFocusOnTab: true
        Accessible.role: Accessible.Button
        Accessible.name: dr.label
        Keys.onReturnPressed: if (dr.enabled) dr.picked()
        Keys.onSpacePressed: if (dr.enabled) dr.picked()
        HoverHandler { id: drHover; cursorShape: dr.enabled ? Qt.PointingHandCursor : Qt.ArrowCursor }
        TapHandler { enabled: dr.enabled; onTapped: dr.picked() }
        Rectangle {   // keyboard focus
            anchors.fill: parent
            radius: dr.radius
            color: "transparent"
            border.width: dr.activeFocus ? 2 : 0
            border.color: Th.borderFocus
            visible: dr.activeFocus
        }
        RowLayout {
            anchors.fill: parent
            anchors.leftMargin: 8
            anchors.rightMargin: 8
            spacing: 8
            Controls.Label {
                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                visible: dr.emoji.length > 0
                Layout.preferredWidth: 16
                horizontalAlignment: Text.AlignHCenter
                text: dr.emoji
                font.pixelSize: 14
            }
            ThIcon {
                visible: dr.emoji.length === 0
                Layout.preferredWidth: 16
                Layout.preferredHeight: 16
                app: dr.app
                name: dr.iconName
                color: Th.actionPaletteIcon
                isMask: true
            }
            Controls.Label {
                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                Layout.fillWidth: true
                text: dr.label
                color: Th.dialogFg
                elide: Text.ElideRight
            }
            Controls.Label {
                visible: dr.more
                text: "…"
                color: Th.dialogHintFg
            }
        }
    }
}
