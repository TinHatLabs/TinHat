// EditToolbar.qml — edit mode's toolbar (EDITMODE-UI.md §3, §7, §8, §9, §11; the web's Toolbar.tsx, the same parts and
// numbers): a 52 px strip above the dock host (the host is that much shorter while editing, so the bar never covers an
// island's handle), the 44 px bar centred in it. Editing: ‹name› · Undo · Redo · Presets ▾ · Add island ▾ · Add widget ·
// Reset · Cancel · Done. Its panels (the presets with their static schematic thumbnails, the hidden-islands drawer,
// the discard confirm) are drawn here too, over the host — plain items, not popups, so a row of the drawer can be
// dragged into the layout with the same press. The session's keys live here: Ctrl+Z · Ctrl+Shift+Z / Ctrl+Y ·
// Ctrl+Enter (Ctrl+E is Main.qml's). Exists only while editing (a Loader in Main.qml).
import QtQuick
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th

Item {
    id: tb
    required property var em               // EditMode.qml
    required property var host             // the DockHost (its size: the thumbnails, the narrow rules)
    property var editLayer: null           // the EditLayer (Tab order)
    property Item overlay: null            // the item the panels' outside-click catcher covers (the shell body)
    property real radius: 12
    objectName: "editToolbar"
    height: 52

    readonly property real hostW: tb.host ? tb.host.width : tb.width
    readonly property bool labels: tb.hostW >= 980
    readonly property bool nameChip: tb.hostW >= 760
    property string panel: ""              // "" | "presets" | "add" | "confirm"
    function closePanels() { tb.panel = "" }
    onPanelChanged: if (tb.panel === "presets") presetsPanel.refresh()

    function focusFirst() { if (tb.em.readOnly) doneBtn.forceActiveFocus(Qt.TabFocusReason); else undoBtn.forceActiveFocus(Qt.TabFocusReason) }
    function focusLast() { doneBtn.forceActiveFocus(Qt.TabFocusReason) }

    Shortcut { sequence: "Ctrl+Z"; enabled: tb.em.open; onActivated: tb.em.undo() }
    Shortcut { sequences: ["Ctrl+Shift+Z", "Ctrl+Y"]; enabled: tb.em.open; onActivated: tb.em.redo() }
    Shortcut { sequences: ["Ctrl+Return", "Ctrl+Enter"]; enabled: tb.em.open; onActivated: tb.em.done() }

    component TBtn: Rectangle {
        id: b
        property string iconName: ""
        property string label: ""
        property bool showLabel: true
        property string tip: ""
        property bool accent: false
        property bool danger: false
        property int badge: 0
        property bool isDisabled: false
        signal clicked()
        height: 32
        width: b.showLabel ? Math.max(b.accent ? 72 : 0, row.implicitWidth + 20) : 32
        radius: 8
        color: b.accent ? Th.accentBase : (ma.containsMouse && !b.isDisabled ? Th.surfaceHover : "transparent")
        border.width: b.accent && ma.containsMouse ? 1 : 0
        border.color: Th.accentBright
        opacity: b.isDisabled ? 0.4 : 1
        activeFocusOnTab: true
        Accessible.role: Accessible.Button
        Accessible.name: b.label
        Keys.onReturnPressed: if (!b.isDisabled) b.clicked()
        Keys.onEnterPressed: if (!b.isDisabled) b.clicked()
        Keys.onSpacePressed: if (!b.isDisabled) b.clicked()
        Rectangle {   // keyboard focus ring
            anchors.fill: parent
            anchors.margins: -3
            radius: 10
            color: "transparent"
            border.width: 2
            border.color: Th.borderFocus
            visible: b.activeFocus
        }
        Row {
            id: row
            anchors.centerIn: parent
            spacing: 6
            Kirigami.Icon {
                visible: b.iconName.length > 0
                anchors.verticalCenter: parent.verticalCenter
                width: 16; height: 16
                source: b.iconName
                color: b.accent ? Th.textOnAccent : Th.textPrimary
                isMask: true
            }
            Text {
                visible: b.showLabel
                anchors.verticalCenter: parent.verticalCenter
                font.pixelSize: 13
                font.weight: (b.accent || b.danger) ? Font.DemiBold : Font.Normal
                color: b.accent ? Th.textOnAccent : b.danger ? Th.stateDangerBright : Th.textPrimary
                text: b.label
            }
            Rectangle {
                visible: b.badge > 0
                anchors.verticalCenter: parent.verticalCenter
                height: 16
                width: Math.max(16, badgeText.implicitWidth + 8)
                radius: 8
                color: Th.badgeBg
                Text {
                    id: badgeText
                    anchors.centerIn: parent
                    font.pixelSize: 11
                    font.weight: Font.DemiBold
                    color: Th.textOnAccent
                    text: String(b.badge)
                }
            }
        }
        Controls.ToolTip.visible: ma.containsMouse && (b.tip.length > 0 || !b.showLabel)
        Controls.ToolTip.text: b.tip.length > 0 ? b.tip : b.label
        Controls.ToolTip.delay: 600
        MouseArea {
            id: ma
            anchors.fill: parent
            hoverEnabled: true
            onClicked: if (!b.isDisabled) b.clicked()
        }
    }
    component Sep: Rectangle { width: 1; height: 20; color: Th.editmodeToolbarSep; anchors.verticalCenter: parent ? parent.verticalCenter : undefined }

    // ---- the outside-click catcher (under the panels, over everything else) ----
    MouseArea {
        visible: tb.panel.length > 0
        enabled: visible
        parent: tb.overlay ? tb.overlay : tb
        anchors.fill: parent
        z: 90
        acceptedButtons: Qt.AllButtons
        onPressed: tb.closePanels()
        onWheel: (wheel) => { wheel.accepted = true }
    }

    // ---- the bar ----
    Rectangle {   // the static shadow (0 4 16): two soft steps, no effect item
        anchors.fill: bar
        anchors.topMargin: 4
        anchors.bottomMargin: -4
        radius: bar.radius
        color: Th.editmodeToolbarShadow
        opacity: 0.35
    }
    Rectangle {
        id: bar
        objectName: "editToolbarBar"
        anchors.centerIn: parent
        height: 44
        width: Math.min(items.implicitWidth + 12, tb.width - 16)
        radius: tb.radius
        color: Th.editmodeToolbarBg
        border.width: 1
        border.color: Th.editmodeToolbarBorder
        clip: true
        Accessible.role: Accessible.ToolBar
        Accessible.name: "Edit layout"
        Row {
            id: items
            x: 6
            anchors.verticalCenter: parent.verticalCenter
            spacing: 4
            Text {
                visible: tb.nameChip
                anchors.verticalCenter: parent.verticalCenter
                leftPadding: 6
                rightPadding: 6
                width: Math.min(implicitWidth, 220)
                elide: Text.ElideRight
                textFormat: Text.StyledText
                font.pixelSize: 13
                color: Th.textSecondary
                text: (tb.em.readOnly ? "Viewing" : "Editing") + ": <b><font color=\"" + Th.textPrimary + "\">" + tb.em.name.replace(/&/g, "&amp;").replace(/</g, "&lt;") + "</font></b>"
            }
            Sep { visible: tb.nameChip && !tb.em.readOnly }
            TBtn {
                id: undoBtn
                visible: !tb.em.readOnly
                iconName: "edit-undo-symbolic"; label: "Undo"; showLabel: false; tip: "Undo (Ctrl+Z)"
                isDisabled: !tb.em.canUndo
                onClicked: tb.em.undo()
                Keys.onBacktabPressed: { if (tb.editLayer) tb.editLayer.focusLast() }
            }
            TBtn {
                visible: !tb.em.readOnly
                iconName: "edit-redo-symbolic"; label: "Redo"; showLabel: false; tip: "Redo (Ctrl+Shift+Z)"
                isDisabled: !tb.em.canRedo
                onClicked: tb.em.redo()
            }
            Sep { visible: !tb.em.readOnly }
            TBtn {
                id: presetsBtn
                objectName: "editTool:presets"
                visible: !tb.em.readOnly
                iconName: "view-grid-symbolic"; label: "Presets"; showLabel: tb.labels
                onClicked: tb.panel = tb.panel === "presets" ? "" : "presets"
            }
            TBtn {
                id: addBtn
                objectName: "editTool:add-island"
                visible: !tb.em.readOnly
                iconName: "window-new"; label: "Add island"; showLabel: tb.labels
                badge: tb.em.hidden.length
                onClicked: tb.panel = tb.panel === "add" ? "" : "add"
            }
            TBtn {
                visible: !tb.em.readOnly
                iconName: "list-add-symbolic"; label: "Add widget"; showLabel: tb.labels
                onClicked: { tb.closePanels(); tb.em.addWidget() }
            }
            Sep { visible: !tb.em.readOnly }
            TBtn {
                visible: !tb.em.readOnly
                iconName: "view-refresh-symbolic"; label: "Reset"; showLabel: tb.labels; tip: "Reset to Classic"
                onClicked: { tb.closePanels(); tb.em.reset() }
            }
            Sep { visible: !tb.em.readOnly }
            TBtn {
                id: cancelBtn
                visible: !tb.em.readOnly
                label: "Cancel"
                onClicked: {
                    tb.em.cancelDrag()
                    if (tb.em.dirty) tb.panel = "confirm"
                    else tb.em.cancel()
                }
            }
            TBtn {
                id: doneBtn
                objectName: "editTool:done"
                accent: true
                label: tb.em.readOnly ? "Close" : "Done"
                tip: tb.em.readOnly ? "" : (tb.em.dirty ? "Save and close (Ctrl+E)" : "Close (Ctrl+E)")
                onClicked: { tb.closePanels(); tb.em.done() }
                Keys.onTabPressed: { if (tb.editLayer) tb.editLayer.focusFirst() }
            }
        }
    }

    // ---- panels (over the host) ----
    component Panel: Rectangle {
        radius: 10
        color: Th.surfaceOverlay
        border.width: 1
        border.color: Th.borderStrong
        z: 100
    }
    function _below(btn, w) {
        var p = btn.mapToItem(tb, 0, btn.height)
        return { x: Math.max(8, Math.min(p.x, tb.width - w - 8)), y: p.y + 6 }
    }

    // Presets ▾ (EDITMODE-UI §8): 360 wide, padding 4, one 72 px row per preset with its static schematic (96 × 60,
    // computed when the popover opens), name and line; the current preset checked; Previous when the working copy
    // differs from the start
    Panel {
        id: presetsPanel
        objectName: "editPanel:presets"
        visible: tb.panel === "presets"
        width: 360
        readonly property var pos: tb.panel === "presets" ? tb._below(presetsBtn, width) : ({ x: 0, y: 0 })
        x: pos.x
        y: pos.y
        height: presetCol.implicitHeight + 8
        readonly property var presets: [
            { id: "classic", name: "Classic", line: "Spaces and rooms on the left, the room on the right." },
            { id: "compact", name: "Compact", line: "Tighter gaps and a narrower room list." },
            { id: "callFirst", name: "Call-first", line: "The call takes the big spot; chat sits beside it." },
            { id: "sideMonitor", name: "Side monitor", line: "For a tall, narrow window: spaces across the top." },
            { id: "handheld", name: "Handheld", line: "Rooms and widgets share a column as tabs." }
        ]
        property var thumbs: ({})
        function refresh() {
            var out = ({})
            var ctx = tb.host ? tb.host.lastCtx : null
            if (!ctx) { presetsPanel.thumbs = out; return }
            var W = 96, H = 60
            var k = Math.min(W / Math.max(1, ctx.size.w), H / Math.max(1, ctx.size.h))
            for (var i = 0; i < presetsPanel.presets.length; i++) {
                var id = presetsPanel.presets[i].id
                var rects = []
                try {
                    var f = JSON.parse(String(tb.em.enginePreset(id))).form
                    var o = JSON.parse(String(tb.em.engineSolve(JSON.stringify(f), JSON.stringify(ctx))))
                    for (var r in o.islands) {
                        var ir = o.islands[r]
                        if (ir.vis === false || ir.w <= 0 || ir.h <= 0 || (ir.z || 0) >= 20) continue
                        rects.push({ main: r === "call" || r === "timeline", x: Math.round(ir.x * k), y: Math.round(ir.y * k),
                                     w: Math.max(1, Math.round(ir.w * k)), h: Math.max(1, Math.round(ir.h * k)) })
                    }
                } catch (e) {}
                out[id] = rects
            }
            presetsPanel.thumbs = out
        }
        Column {
            id: presetCol
            x: 4
            y: 4
            width: parent.width - 8
            Repeater {
                model: presetsPanel.presets
                delegate: Rectangle {
                    id: prow
                    required property var modelData
                    objectName: "editPreset:" + modelData.id
                    width: presetCol.width
                    height: 72
                    radius: 8
                    color: pma.containsMouse ? Th.surfaceHover : "transparent"
                    Rectangle {   // the schematic
                        id: thumb
                        x: 6
                        anchors.verticalCenter: parent.verticalCenter
                        width: 96
                        height: 60
                        radius: 4
                        color: Th.editmodePresetBg
                        clip: true
                        Repeater {
                            model: presetsPanel.thumbs[prow.modelData.id] || []
                            delegate: Rectangle {
                                required property var modelData
                                x: modelData.x; y: modelData.y; width: modelData.w; height: modelData.h
                                radius: 2
                                color: modelData.main ? Th.editmodePresetMain : Th.editmodePresetIsland
                            }
                        }
                    }
                    Column {
                        anchors.left: thumb.right
                        anchors.leftMargin: 10
                        anchors.right: check.left
                        anchors.rightMargin: 4
                        anchors.verticalCenter: parent.verticalCenter
                        spacing: 2
                        Text { width: parent.width; elide: Text.ElideRight; font.pixelSize: 13; font.weight: Font.DemiBold; color: Th.textPrimary; text: prow.modelData.name }
                        Text { width: parent.width; wrapMode: Text.Wrap; maximumLineCount: 2; elide: Text.ElideRight; font.pixelSize: 12; color: Th.textSecondary; text: prow.modelData.line }
                    }
                    Kirigami.Icon {
                        id: check
                        anchors.right: parent.right
                        anchors.rightMargin: 4
                        anchors.verticalCenter: parent.verticalCenter
                        width: 16; height: 16
                        source: "checkmark-symbolic"
                        color: Th.accentBright
                        isMask: true
                        opacity: tb.em.currentPreset === prow.modelData.id ? 1 : 0
                    }
                    MouseArea {
                        id: pma
                        anchors.fill: parent
                        hoverEnabled: true
                        onClicked: { tb.closePanels(); tb.em.preset(prow.modelData.id) }
                    }
                }
            }
            Rectangle {
                visible: tb.em.formJson !== tb.em.startJson
                width: presetCol.width
                height: visible ? 44 : 0
                radius: 8
                color: prevMa.containsMouse ? Th.surfaceHover : "transparent"
                Kirigami.Icon { x: 10; anchors.verticalCenter: parent.verticalCenter; width: 16; height: 16; source: "edit-undo-symbolic"; color: Th.textPrimary; isMask: true }
                Column {
                    x: 36
                    width: parent.width - 42
                    anchors.verticalCenter: parent.verticalCenter
                    Text { font.pixelSize: 13; font.weight: Font.DemiBold; color: Th.textPrimary; text: "Previous" }
                    Text { width: parent.width; elide: Text.ElideRight; font.pixelSize: 12; color: Th.textSecondary; text: "What you had when you opened edit mode." }
                }
                MouseArea { id: prevMa; anchors.fill: parent; hoverEnabled: true; onClicked: { tb.closePanels(); tb.em.preset("previous") } }
            }
        }
    }

    // Add island ▾ (EDITMODE-UI §7): the hidden islands (Restore, or drag a row into the layout), New button island (the
    // rows for islands a newer Tin Hat placed are DEFERRED, as on the web)
    Panel {
        id: addPanel
        objectName: "editPanel:add"
        visible: tb.panel === "add"
        opacity: tb.em.dragRef.length > 0 ? 0 : 1      // a row being dragged: the panel steps aside, the press lives on
        width: 320
        readonly property var pos: tb.panel === "add" ? tb._below(addBtn, width) : ({ x: 0, y: 0 })
        x: pos.x
        y: pos.y
        height: Math.min(addCol.implicitHeight + 12, (tb.overlay ? tb.overlay.height : 600) * 0.6)
        Flickable {
            anchors.fill: parent
            anchors.margins: 6
            contentHeight: addCol.implicitHeight
            clip: true
            boundsBehavior: Flickable.StopAtBounds
            interactive: contentHeight > height
            Column {
                id: addCol
                width: parent.width
                Text {
                    visible: tb.em.hidden.length === 0
                    width: parent.width
                    padding: 10
                    wrapMode: Text.Wrap
                    font.pixelSize: 12
                    color: Th.textSecondary
                    text: "Nothing hidden. Hide an island from its ⋯ menu to keep it here."
                }
                Repeater {
                    model: tb.em.hidden
                    delegate: Rectangle {
                        id: hrow
                        required property string modelData
                        readonly property var t: tb.em.textOf(modelData)
                        objectName: "editHidden:" + modelData
                        width: addCol.width
                        height: 44
                        radius: 8
                        color: rowMa.containsMouse ? Th.surfaceHover : "transparent"
                        Kirigami.Icon { x: 10; anchors.verticalCenter: parent.verticalCenter; width: 20; height: 20; source: hrow.t.icon; color: Th.textPrimary; isMask: true }
                        Column {
                            x: 40
                            width: parent.width - 40 - restoreBtn.width - 12
                            anchors.verticalCenter: parent.verticalCenter
                            Text { width: parent.width; elide: Text.ElideRight; font.pixelSize: 13; font.weight: Font.DemiBold; color: Th.textPrimary; text: hrow.t.name }
                            Text { width: parent.width; elide: Text.ElideRight; font.pixelSize: 12; color: Th.textSecondary; text: hrow.t.about }
                        }
                        MouseArea {
                            id: rowMa
                            anchors.fill: parent
                            hoverEnabled: true
                            preventStealing: true
                            property real x0: 0
                            property real y0: 0
                            property bool dragging: false
                            onPressed: (mouse) => { rowMa.x0 = mouse.x; rowMa.y0 = mouse.y; rowMa.dragging = false }
                            onPositionChanged: (mouse) => {
                                if (!rowMa.pressed || !tb.host) return
                                var p = rowMa.mapToItem(tb.host, mouse.x, mouse.y)
                                if (!rowMa.dragging) {
                                    if (Math.hypot(mouse.x - rowMa.x0, mouse.y - rowMa.y0) < 4) return
                                    if (!tb.em.beginDrag(hrow.modelData, "pointer", p.x, p.y)) return
                                    rowMa.dragging = true
                                }
                                tb.em.movePointer(Math.round(p.x), Math.round(p.y))
                            }
                            onReleased: {
                                if (rowMa.dragging) { rowMa.dragging = false; tb.closePanels(); tb.em.dropDrag() }
                                else { tb.closePanels(); tb.em.restore(hrow.modelData) }
                            }
                            onCanceled: { if (rowMa.dragging) { rowMa.dragging = false; tb.em.cancelDrag() } }
                        }
                        TBtn {
                            id: restoreBtn
                            anchors.right: parent.right
                            anchors.rightMargin: 4
                            anchors.verticalCenter: parent.verticalCenter
                            label: "Restore"
                            onClicked: { tb.closePanels(); tb.em.restore(hrow.modelData) }
                        }
                    }
                }
                Rectangle {   // New button island (after the buttons merge; ACTIONS.md: max 8)
                    objectName: "editHidden:new-buttons"
                    width: addCol.width
                    height: 44
                    radius: 8
                    color: nbMa.containsMouse ? Th.surfaceHover : "transparent"
                    Kirigami.Icon { x: 10; anchors.verticalCenter: parent.verticalCenter; width: 20; height: 20; source: "list-add-symbolic"; color: Th.textPrimary; isMask: true }
                    Column {
                        x: 40
                        width: parent.width - 48
                        anchors.verticalCenter: parent.verticalCenter
                        Text { font.pixelSize: 13; font.weight: Font.DemiBold; color: Th.textPrimary; text: "New button island" }
                        Text { width: parent.width; elide: Text.ElideRight; font.pixelSize: 12; color: Th.textSecondary; text: "A button row you fill yourself, Stream Deck style" }
                    }
                    MouseArea { id: nbMa; anchors.fill: parent; hoverEnabled: true; onClicked: { tb.closePanels(); tb.em.newButtonIsland() } }
                }
            }
        }
    }

    // Cancel with changes: "Discard your layout changes?" — Keep editing / Discard (danger)
    Panel {
        id: confirmPanel
        objectName: "editPanel:confirm"
        visible: tb.panel === "confirm"
        width: confirmCol.implicitWidth + 24
        height: confirmCol.implicitHeight + 20
        readonly property var pos: tb.panel === "confirm" ? cancelBtn.mapToItem(tb, cancelBtn.width, cancelBtn.height) : ({ x: 0, y: 0 })
        x: Math.max(8, Math.min(pos.x - width, tb.width - width - 8))
        y: pos.y + 6
        Column {
            id: confirmCol
            x: 12
            y: 10
            spacing: 10
            Text { font.pixelSize: 13; color: Th.textPrimary; text: "Discard your layout changes?" }
            Row {
                anchors.right: parent.right
                spacing: 6
                TBtn { label: "Keep editing"; onClicked: tb.closePanels() }
                TBtn { objectName: "editTool:discard"; label: "Discard"; danger: true; onClicked: { tb.closePanels(); tb.em.cancel() } }
            }
        }
    }
}
