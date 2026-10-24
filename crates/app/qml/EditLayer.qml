// EditLayer.qml — edit mode's chrome over the dock host (EDITMODE-UI.md §2-§6, §11; the web's EditLayer.tsx, the same
// parts and numbers): created only while editing (a Loader in the host, z band 30+), in the host's coordinates, above
// every island. It takes the pointer, so island contents don't react while editing (they stay mounted and live). Per
// visible island: the veil + 1 px outline (2 px `island.border.active` when selected), the handle chip (drag source,
// focus stop) and the ⋯ menu — in the islands' z order (a float's chrome above the docked ones'). The placeholders of
// islands whose content is absent are drawn by the islands themselves (Island.qml), so a float covers them. The gaps become resize handles. While a
// move is in progress: the hovered zone, the landing preview and the ghost — nothing else draws, nothing loops (§15).
// No layer, no effect items (GPU.md §2.9).
import QtQuick
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th

Item {
    id: el
    required property var em               // EditMode.qml
    required property var host             // the DockHost
    property var toolbar: null             // EditToolbar (Tab order: toolbar → chips → gaps)
    readonly property var out: host.lastOut
    readonly property real radius: host.islandRadius
    objectName: "editLayer"
    focus: true
    Accessible.role: Accessible.Application
    Accessible.name: "Edit layout"

    /** the islands with chrome: visible, under the z band 30, region order then the rest in ref order */
    readonly property var refs: {
        var o = el.out
        if (!o || !o.islands) return []
        var vis = []
        for (var r in o.islands) {
            var ir = o.islands[r]
            if (ir.vis !== false && ir.w > 0 && ir.h > 0 && (ir.z || 0) < 30) vis.push(r)
        }
        var order = el.host.regionOrder
        var known = order.filter(function (x) { return vis.indexOf(x) >= 0 })
        var rest = vis.filter(function (x) { return order.indexOf(x) < 0 }).sort()
        return known.concat(rest)
    }
    readonly property var gaps: (el.out && el.out.gaps && !el.em.readOnly) ? el.out.gaps : []
    // the Repeaters' models change only when the LIST changes (a new array re-creates every delegate: a gap drag would
    // lose its press, a chip its keyboard focus); during a gap drag the gap list is held
    property var refList: []
    property var gapIds: []
    property bool gapDragging: false
    function _same(a, b) { return JSON.stringify(a) === JSON.stringify(b) }
    function syncLists() {
        if (!el._same(el.refs, el.refList)) el.refList = el.refs
        if (el.gapDragging) return
        var ids = el.gaps.map(function (g) { return g.id })
        if (!el._same(ids, el.gapIds)) el.gapIds = ids
    }
    onRefsChanged: el.syncLists()
    onGapsChanged: el.syncLists()
    function gapById(id) {
        for (var i = 0; i < el.gaps.length; i++) if (el.gaps[i].id === id) return el.gaps[i]
        return null
    }

    // the chips and gap handles by position (Tab order)
    property var _chips: ({})
    property var _gaps: ({})
    function focusChip(i) {
        var r = el.refList[i]
        var c = r !== undefined ? el._chips[r] : null
        if (c) { c.forceActiveFocus(Qt.TabFocusReason); return true }
        return false
    }
    function focusGap(i) {
        var id = el.gapIds[i]
        var h = id !== undefined ? el._gaps[id] : null
        if (h) { h.forceActiveFocus(Qt.TabFocusReason); return true }
        return false
    }
    /** Tab / Shift+Tab: toolbar items → chips → gaps (EDITMODE-UI §11), wrapping. */
    function focusFirst() { if (!el.focusChip(0) && !el.focusGap(0) && el.toolbar) el.toolbar.focusFirst() }
    function focusLast() { if (!el.focusGap(el.gapIds.length - 1) && !el.focusChip(el.refList.length - 1) && el.toolbar) el.toolbar.focusLast() }
    function stepFrom(kind, i, dir) {
        if (kind === "chip") {
            if (dir > 0) { if (el.focusChip(i + 1) || el.focusGap(0)) return }
            else if (el.focusChip(i - 1)) return
        } else {
            if (dir > 0) { if (el.focusGap(i + 1)) return }
            else if (el.focusGap(i - 1) || el.focusChip(el.refList.length - 1)) return
        }
        if (el.toolbar) { if (dir > 0) el.toolbar.focusFirst(); else el.toolbar.focusLast() }
    }

    // the layer eats the pointer over island contents; a press clears a menu and takes the keyboard
    MouseArea {
        anchors.fill: parent
        acceptedButtons: Qt.AllButtons
        hoverEnabled: true
        onPressed: el.forceActiveFocus()
        onWheel: (wheel) => { wheel.accepted = true }
    }
    Keys.onEscapePressed: (event) => { el.em.cancelDrag(); event.accepted = true }   // Esc never leaves edit mode
    // from the layer itself (nothing selected yet): Tab enters the order at the toolbar, Shift+Tab at its end
    Keys.onTabPressed: (event) => { if (el.toolbar) el.toolbar.focusFirst(); else el.focusFirst(); event.accepted = true }
    Keys.onBacktabPressed: (event) => { el.focusLast(); event.accepted = true }
    Connections {   // a window blur cancels a move (EDITMODE-UI §5)
        target: el.Window.window
        function onActiveChanged() { if (el.Window.window && !el.Window.window.active) el.em.cancelDrag() }
    }

    // ---- per island: veil, outline, placeholder, chip, ⋯ ----
    Repeater {
        model: el.refList
        delegate: Item {
            id: isl
            required property string modelData
            required property int index
            readonly property var r: (el.out && el.out.islands && el.out.islands[modelData]) || ({ x: 0, y: 0, w: 0, h: 0 })
            readonly property var t: el.em.textOf(modelData)
            readonly property bool unknownRef: !el.em.known(modelData)
            readonly property bool narrow: r.w < 132
            readonly property bool isShort: r.h < 72
            readonly property bool selected: el.em.dragRef === modelData || chip.activeFocus
            objectName: "edit:" + modelData
            x: r.x
            y: r.y
            width: r.w
            height: r.h
            z: 1 + (r.z || 0)   // a floating island's chrome above the docked ones' (the island z bands)

            Rectangle {   // the veil
                anchors.fill: parent
                radius: el.radius
                color: Th.editmodeIslandVeil
            }
            Rectangle {   // the inside frame: 1 px outline, 2 px active when selected
                anchors.fill: parent
                radius: el.radius
                color: "transparent"
                border.width: isl.selected ? 2 : 1
                border.color: isl.selected ? Th.islandBorderActive : Th.editmodeIslandOutline
            }
            // right-click on the island: its menu
            MouseArea {
                anchors.fill: parent
                acceptedButtons: Qt.RightButton
                onPressed: (mouse) => { if (!isl.unknownRef) el.openMenu(isl.modelData, isl, mouse.x, mouse.y) }
            }

            // ---- the handle chip (drag source, focus stop) ----
            Rectangle {
                id: chip
                objectName: "editchip:" + isl.modelData
                readonly property real fullW: 8 + 6 + 8 + chipName.implicitWidth + 10
                x: isl.narrow ? Math.round((isl.width - 28) / 2) : 8
                y: isl.isShort ? Math.round(isl.height / 2 - 14) : 8
                width: isl.narrow ? 28 : Math.min(chip.fullW, Math.max(28, isl.width - 16 - 36))
                height: 28
                radius: 8
                color: chipMa.containsMouse && !isl.unknownRef ? Th.editmodeHandleBgHover : Th.editmodeHandleBg
                border.width: 1
                border.color: Th.editmodeHandleBorder
                opacity: (isl.unknownRef || el.em.readOnly) ? 0.6 : 1
                activeFocusOnTab: false
                Accessible.role: Accessible.Button
                Accessible.name: isl.unknownRef ? isl.t.name : "Move " + isl.t.name
                Component.onCompleted: { var m = el._chips; m[isl.modelData] = chip; el._chips = m }
                Component.onDestruction: { var m = el._chips; if (m && m[isl.modelData] === chip) delete m[isl.modelData] }
                Rectangle {   // keyboard focus ring
                    anchors.fill: parent
                    anchors.margins: -3
                    radius: 10
                    color: "transparent"
                    border.width: 2
                    border.color: Th.borderFocus
                    visible: chip.activeFocus
                }
                Grid {   // the grip: 6 dots, 2 columns × 3 rows, 2 px each, 2 px apart
                    x: isl.narrow ? 11 : 8
                    anchors.verticalCenter: parent.verticalCenter
                    columns: 2
                    spacing: 2
                    Repeater { model: 6; delegate: Rectangle { width: 2; height: 2; radius: 1; color: Th.editmodeHandleFg } }
                }
                Text {
                    id: chipName
                    visible: !isl.narrow
                    x: 8 + 6 + 8
                    width: Math.max(0, chip.width - x - 10)
                    anchors.verticalCenter: parent.verticalCenter
                    elide: Text.ElideRight
                    font.pixelSize: 12
                    font.weight: Font.DemiBold
                    color: Th.editmodeHandleFg
                    text: isl.t.name
                }
                Controls.ToolTip.visible: isl.narrow && chipMa.containsMouse && el.em.dragRef.length === 0
                Controls.ToolTip.text: isl.t.name
                Controls.ToolTip.delay: 600
                MouseArea {
                    id: chipMa
                    anchors.fill: parent
                    hoverEnabled: true
                    preventStealing: true
                    enabled: !isl.unknownRef && !el.em.readOnly
                    cursorShape: pressed ? Qt.ClosedHandCursor : Qt.OpenHandCursor
                    property real x0: 0
                    property real y0: 0
                    property bool dragging: false
                    onPressed: (mouse) => {
                        var p = chipMa.mapToItem(el, mouse.x, mouse.y)
                        chipMa.x0 = p.x; chipMa.y0 = p.y; chipMa.dragging = false
                        chip.forceActiveFocus(Qt.MouseFocusReason)
                    }
                    onPositionChanged: (mouse) => {
                        if (!chipMa.pressed) return
                        var p = chipMa.mapToItem(el, mouse.x, mouse.y)
                        if (!chipMa.dragging) {
                            if (Math.hypot(p.x - chipMa.x0, p.y - chipMa.y0) < 4) return
                            if (!el.em.beginDrag(isl.modelData, "pointer", p.x, p.y)) return
                            chipMa.dragging = true
                        }
                        el.em.movePointer(Math.round(p.x), Math.round(p.y))
                    }
                    onReleased: {
                        if (chipMa.dragging) { chipMa.dragging = false; el.em.dropDrag() }
                        else if (isl.narrow) el.openMenu(isl.modelData, chip, 0, chip.height)
                    }
                    onCanceled: { if (chipMa.dragging) { chipMa.dragging = false; el.em.cancelDrag() } }
                }
                Keys.onPressed: (event) => {
                    var em = el.em
                    if (em.dragRef.length > 0 && em.dragMode === "keys") {
                        var k = event.key
                        if (k === Qt.Key_Left || k === Qt.Key_Up) em.stepHover(-1)
                        else if (k === Qt.Key_Right || k === Qt.Key_Down) em.stepHover(1)
                        else if (k === Qt.Key_Return || k === Qt.Key_Enter || k === Qt.Key_Space) em.dropDrag()
                        else if (k === Qt.Key_Escape) em.cancelDrag()
                        else if (k === Qt.Key_Tab || k === Qt.Key_Backtab) em.cancelDrag()
                        event.accepted = true
                        return
                    }
                    if (em.dragRef.length > 0) { if (event.key === Qt.Key_Escape) { em.cancelDrag(); event.accepted = true } return }
                    if (event.key === Qt.Key_Tab) { el.stepFrom("chip", isl.index, 1); event.accepted = true }
                    else if (event.key === Qt.Key_Backtab) { el.stepFrom("chip", isl.index, -1); event.accepted = true }
                    else if (isl.unknownRef || em.readOnly) return
                    else if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter || event.key === Qt.Key_Space) {
                        em.beginDrag(isl.modelData, "keys", 0, 0); event.accepted = true
                    } else if (event.key === Qt.Key_Delete || event.key === Qt.Key_Backspace) {
                        if (em.hideable(isl.modelData)) em.hide(isl.modelData)
                        event.accepted = true
                    } else if (event.key === Qt.Key_Menu || (event.key === Qt.Key_F10 && (event.modifiers & Qt.ShiftModifier))) {
                        el.openMenu(isl.modelData, chip, 0, chip.height); event.accepted = true
                    } else if (event.key === Qt.Key_Escape) event.accepted = true
                }
            }
            // ---- the ⋯ button ----
            Rectangle {
                id: more
                visible: !isl.narrow && !isl.unknownRef
                x: isl.width - 8 - 28
                y: isl.isShort ? Math.round(isl.height / 2 - 14) : 8
                width: 28
                height: 28
                radius: 8
                color: moreMa.containsMouse ? Th.editmodeHandleBgHover : Th.editmodeHandleBg
                border.width: 1
                border.color: Th.editmodeHandleBorder
                opacity: el.em.readOnly ? 0.6 : 1
                Accessible.role: Accessible.Button
                Accessible.name: isl.t.name + " options"
                Kirigami.Icon {
                    anchors.centerIn: parent
                    width: 16; height: 16
                    source: "view-more-horizontal-symbolic"
                    color: Th.editmodeHandleFg
                    isMask: true
                }
                MouseArea {
                    id: moreMa
                    anchors.fill: parent
                    hoverEnabled: true
                    enabled: !el.em.readOnly
                    onClicked: el.openMenu(isl.modelData, more, 0, more.height)
                }
            }
        }
    }

    // ---- stacks: their tabs as chips over the strip — click shows that tab, drag moves that island (§4.2) ----
    Repeater {
        model: {
            var o = el.out, out = []
            if (o && o.stacks) for (var id in o.stacks) {
                var st = o.stacks[id]
                if (st.vis !== false && st.tab > 0 && (st.z || 0) < 30) out.push({ id: id, s: st })
            }
            return out
        }
        delegate: Rectangle {
            id: ts
            required property var modelData
            readonly property var st: modelData.s
            objectName: "edittabs:" + modelData.id
            x: st.x
            y: st.y
            z: 2 + (st.z || 0)
            width: st.w
            height: st.tab
            color: Th.dockTabBg
            radius: el.radius
            Row {
                x: 2
                anchors.verticalCenter: parent.verticalCenter
                spacing: 4
                Repeater {
                    model: ts.st.tabs
                    delegate: Rectangle {
                        id: tc
                        required property string modelData
                        readonly property bool active: tc.modelData === ts.st.active
                        objectName: "edittab:" + tc.modelData
                        height: 24
                        width: Math.min(tcName.implicitWidth + 6 + 6 + 6 + 8, Math.max(40, Math.floor(ts.width / Math.max(1, ts.st.tabs.length)) - 4))
                        radius: 6
                        color: tcMa.containsMouse ? Th.editmodeHandleBgHover : Th.editmodeHandleBg
                        border.width: 1
                        border.color: tc.active ? Th.islandBorderActive : Th.editmodeHandleBorder
                        Accessible.role: Accessible.Button
                        Accessible.name: "Move " + el.em.islandName(tc.modelData) + " (tab)"
                        activeFocusOnTab: false
                        Keys.onPressed: (event) => {
                            var em = el.em
                            if (em.dragRef.length > 0 && em.dragMode === "keys") {
                                var k = event.key
                                if (k === Qt.Key_Left || k === Qt.Key_Up) em.stepHover(-1)
                                else if (k === Qt.Key_Right || k === Qt.Key_Down) em.stepHover(1)
                                else if (k === Qt.Key_Return || k === Qt.Key_Enter || k === Qt.Key_Space) em.dropDrag()
                                else em.cancelDrag()
                                event.accepted = true
                            } else if (!em.readOnly && (event.key === Qt.Key_Return || event.key === Qt.Key_Enter || event.key === Qt.Key_Space)) {
                                em.beginDrag(tc.modelData, "keys", 0, 0); event.accepted = true
                            }
                        }
                        Grid {
                            x: 6
                            anchors.verticalCenter: parent.verticalCenter
                            columns: 2
                            spacing: 2
                            Repeater { model: 6; delegate: Rectangle { width: 2; height: 2; radius: 1; color: Th.editmodeHandleFg } }
                        }
                        Text {
                            id: tcName
                            x: 18
                            width: Math.max(0, tc.width - x - 8)
                            anchors.verticalCenter: parent.verticalCenter
                            elide: Text.ElideRight
                            font.pixelSize: 12
                            font.weight: Font.DemiBold
                            color: Th.editmodeHandleFg
                            text: el.em.islandName(tc.modelData)
                        }
                        MouseArea {
                            id: tcMa
                            anchors.fill: parent
                            hoverEnabled: true
                            preventStealing: true
                            enabled: !el.em.readOnly
                            acceptedButtons: Qt.LeftButton | Qt.RightButton
                            property real x0: 0
                            property real y0: 0
                            property bool dragging: false
                            onPressed: (mouse) => {
                                if (mouse.button === Qt.RightButton) { el.openMenu(tc.modelData, tc, mouse.x, mouse.y); return }
                                var p = tcMa.mapToItem(el, mouse.x, mouse.y)
                                tcMa.x0 = p.x; tcMa.y0 = p.y; tcMa.dragging = false
                                tc.forceActiveFocus(Qt.MouseFocusReason)
                            }
                            onPositionChanged: (mouse) => {
                                if (!(tcMa.pressedButtons & Qt.LeftButton)) return
                                var p = tcMa.mapToItem(el, mouse.x, mouse.y)
                                if (!tcMa.dragging) {
                                    if (Math.hypot(p.x - tcMa.x0, p.y - tcMa.y0) < 4) return
                                    if (!el.em.beginDrag(tc.modelData, "pointer", p.x, p.y)) return
                                    tcMa.dragging = true
                                }
                                el.em.movePointer(Math.round(p.x), Math.round(p.y))
                            }
                            onReleased: (mouse) => {
                                if (mouse.button !== Qt.LeftButton) return
                                if (tcMa.dragging) { tcMa.dragging = false; el.em.dropDrag() }
                                else if (el.em.app && el.em.app.setLayoutTab) el.em.app.setLayoutTab(ts.modelData.id, tc.modelData)
                            }
                            onCanceled: { if (tcMa.dragging) { tcMa.dragging = false; el.em.cancelDrag() } }
                        }
                    }
                }
            }
        }
    }

    // ---- gaps: resize handles (EDITMODE-UI §6) ----
    Repeater {
        model: el.em.dragRef.length > 0 ? [] : el.gapIds
        delegate: Item {
            id: gh
            required property string modelData
            required property int index
            readonly property var g: el.gapById(modelData) || ({ id: modelData, axis: "x", x: 0, y: 0, w: 0, h: 0, a: "", b: "" })
            readonly property bool ax: g.axis === "x"
            readonly property int band: 8
            objectName: "editgap:" + g.id
            x: gh.ax ? Math.round(g.x + g.w / 2 - Math.max(band, g.w) / 2) : g.x
            y: gh.ax ? g.y : Math.round(g.y + g.h / 2 - Math.max(band, g.h) / 2)
            width: gh.ax ? Math.max(band, g.w) : g.w
            height: gh.ax ? g.h : Math.max(band, g.h)
            z: 2
            property bool pressed: false
            Accessible.role: Accessible.Separator
            Accessible.name: "Resize between " + g.a + " and " + g.b
            Component.onCompleted: { var m = el._gaps; m[gh.modelData] = gh; el._gaps = m }
            Component.onDestruction: { var m = el._gaps; if (m && m[gh.modelData] === gh) delete m[gh.modelData] }
            Rectangle {   // the 2 px line: hovered / focused / pressed
                x: gh.ax ? Math.round(gh.width / 2 - 1) : 0
                y: gh.ax ? 0 : Math.round(gh.height / 2 - 1)
                width: gh.ax ? 2 : gh.width
                height: gh.ax ? gh.height : 2
                visible: gh.pressed || gapMa.containsMouse || gh.activeFocus
                color: gh.pressed ? Th.editmodeZoneEdge : Th.editmodeGapHover
            }
            function resizeBy(startForm, delta) {
                var r = null
                try { r = JSON.parse(String(el.em.engineApplyOp(startForm, JSON.stringify({ op: "resize", a: gh.g.a, b: gh.g.b, delta: delta, into: true }), gh._ctx))) } catch (e) { r = null }
                return (r && r.ok) ? JSON.stringify(r.form) : ""
            }
            property string _start: ""
            property string _ctx: ""
            property string _last: ""
            MouseArea {
                id: gapMa
                anchors.fill: parent
                hoverEnabled: true
                preventStealing: true
                acceptedButtons: Qt.LeftButton | Qt.RightButton
                cursorShape: gh.ax ? Qt.SplitHCursor : Qt.SplitVCursor
                property real p0: 0
                onPressed: (mouse) => {
                    if (mouse.button === Qt.RightButton) { el.openGapMenu(gh.g, gh, mouse.x, mouse.y); return }
                    var p = gapMa.mapToItem(el, mouse.x, mouse.y)
                    gapMa.p0 = gh.ax ? p.x : p.y
                    gh._start = el.em.formJson
                    gh._ctx = el.em.ctxJson()
                    gh._last = ""
                    gh.pressed = true
                    el.gapDragging = true
                }
                onPositionChanged: (mouse) => {
                    if (!gh.pressed) return
                    var p = gapMa.mapToItem(el, mouse.x, mouse.y)
                    var f = gh.resizeBy(gh._start, Math.round((gh.ax ? p.x : p.y) - gapMa.p0))
                    if (f.length > 0) { gh._last = f; el.em.previewJson = f }
                }
                onReleased: (mouse) => {
                    if (!gh.pressed) return
                    gh.pressed = false
                    var last = gh._last
                    el.em.previewJson = ""
                    el.gapDragging = false
                    if (last.length > 0) el.em.commitForm(last)
                    el.syncLists()
                }
                onCanceled: { gh.pressed = false; el.em.previewJson = ""; el.gapDragging = false; el.syncLists() }
                onDoubleClicked: (mouse) => { if (mouse.button === Qt.LeftButton) el.em.apply({ op: "resetSizes", a: gh.g.a, b: gh.g.b }, true) }
            }
            Keys.onPressed: (event) => {
                if (event.key === Qt.Key_Tab) { el.stepFrom("gap", gh.index, 1); event.accepted = true; return }
                if (event.key === Qt.Key_Backtab) { el.stepFrom("gap", gh.index, -1); event.accepted = true; return }
                var step = (event.modifiers & Qt.ShiftModifier) ? 32 : 4
                var delta = 0
                if (event.key === Qt.Key_Left || event.key === Qt.Key_Up) delta = -step
                else if (event.key === Qt.Key_Right || event.key === Qt.Key_Down) delta = step
                else if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter) {
                    el.em.apply({ op: "resetSizes", a: gh.g.a, b: gh.g.b }, true); event.accepted = true; return
                } else return
                event.accepted = true
                gh._ctx = el.em.ctxJson()
                var f = gh.resizeBy(el.em.formJson, delta)
                if (f.length > 0) el.em.commitForm(f)
            }
        }
    }

    // ---- a move in progress: the hovered zone, the landing preview, the ghost ----
    Item {
        id: zoneMark
        readonly property var zone: (el.em.hover >= 0 && el.em.hover < el.em.zones.length) ? el.em.zones[el.em.hover] : null
        readonly property var zop: zone ? ((zone.op.op === "restore" && zone.op.to && typeof zone.op.to === "object") ? zone.op.to : zone.op) : null
        visible: zone !== null
        z: 50
        x: zone ? zone.rect.x : 0
        y: zone ? zone.rect.y : 0
        width: zone ? zone.rect.w : 0
        height: zone ? zone.rect.h : 0
        readonly property string side: zop ? String(zop.side || "") : ""
        readonly property string anchor: zop ? String(zop.anchor || "") : ""
        readonly property bool corner: zone !== null && zone.kind === "corner"
        Rectangle { anchors.fill: parent; radius: 6; color: Th.editmodeZoneFill }
        Rectangle { visible: zoneMark.side === "left" || (zoneMark.corner && zoneMark.anchor.indexOf("l") >= 0); x: 0; y: 0; width: 3; height: parent.height; color: Th.editmodeZoneEdge }
        Rectangle { visible: zoneMark.side === "right" || (zoneMark.corner && zoneMark.anchor.indexOf("r") >= 0); x: parent.width - 3; y: 0; width: 3; height: parent.height; color: Th.editmodeZoneEdge }
        Rectangle { visible: zoneMark.side === "top" || (zoneMark.corner && zoneMark.anchor.indexOf("t") >= 0); x: 0; y: 0; width: parent.width; height: 3; color: Th.editmodeZoneEdge }
        Rectangle { visible: zoneMark.side === "bottom" || (zoneMark.corner && zoneMark.anchor.indexOf("b") >= 0); x: 0; y: parent.height - 3; width: parent.width; height: 3; color: Th.editmodeZoneEdge }
        Kirigami.Icon {
            visible: zoneMark.zone !== null && zoneMark.zone.kind === "centre"
            anchors.centerIn: parent
            width: 24; height: 24
            source: "window-duplicate-symbolic"
            color: Th.editmodeZoneEdge
            isMask: true
        }
    }
    Rectangle {   // the landing preview
        visible: el.em.preview !== null
        z: 51
        x: el.em.preview ? el.em.preview.x : 0
        y: el.em.preview ? el.em.preview.y : 0
        width: el.em.preview ? el.em.preview.w : 0
        height: el.em.preview ? el.em.preview.h : 0
        radius: el.radius
        color: Th.editmodeGhostBg
        border.width: 2
        border.color: Th.editmodeGhostBorder
    }
    Rectangle {   // the ghost: a chip-looking card following the pointer at (+12, +12)
        visible: el.em.dragRef.length > 0 && el.em.dragMode === "pointer"
        z: 52
        x: el.em.px + 12
        y: el.em.py + 12
        width: 8 + 6 + 8 + ghostName.implicitWidth + 10
        height: 28
        radius: 8
        color: Th.editmodeHandleBg
        border.width: 2
        border.color: Th.editmodeGhostBorder
        Grid {
            x: 8
            anchors.verticalCenter: parent.verticalCenter
            columns: 2
            spacing: 2
            Repeater { model: 6; delegate: Rectangle { width: 2; height: 2; radius: 1; color: Th.editmodeHandleFg } }
        }
        Text {
            id: ghostName
            x: 22
            anchors.verticalCenter: parent.verticalCenter
            font.pixelSize: 12
            font.weight: Font.DemiBold
            color: Th.editmodeHandleFg
            text: el.em.dragRef.length > 0 ? el.em.islandName(el.em.dragRef) : ""
        }
    }
    // screen readers: the live region
    Text {
        visible: false
        text: el.em.announce
        Accessible.role: Accessible.StaticText
        Accessible.name: el.em.announce
    }

    // ---- the island menu (⋯, right-click, Shift+F10 / Menu on a chip; EDITMODE-UI §4.1) ----
    // the island's button rows first (the row editor; its changes are steps of this session), then Hide · Float / Dock ·
    // Reset this island (or, a button island: Remove this button island)
    readonly property var editableRows: [
        { island: "header", row: "main", title: "Room header" },
        { island: "composer", row: "start", title: "Message box — left" },
        { island: "composer", row: "end", title: "Message box — right" },
        { island: "account", row: "main", title: "Your account card" },
        { island: "call", row: "bar", title: "Call controls" }
    ]
    property string menuRef: ""
    readonly property bool menuButtons: el.em.kindOf(el.menuRef) === "buttons"
    readonly property var menuRows: el.menuRef.length === 0 ? []
        : el.menuButtons ? [{ island: el.menuRef, row: "main", title: el.em.islandName(el.menuRef) }]
        : el.editableRows.filter(function (r) { return r.island === el.menuRef })
    function openMenu(ref, item, mx, my) {
        if (el.em.readOnly || !el.em.known(ref)) return
        el.menuRef = ref
        islandMenu.popup(item, mx, my)
    }
    function editRow(i) {
        var r = el.menuRows[i]
        if (r && el.em.app && el.em.app.actions) el.em.app.actions.openRowEditor({ island: r.island, row: r.row, form: "desktop", title: r.title })
    }
    Controls.Menu {
        id: islandMenu
        objectName: "editIslandMenu"
        modal: false
        readonly property string ref: el.menuRef
        readonly property bool hideable: el.em.hideable(ref)
        readonly property bool floating: ref.length > 0 && el.em.floating(ref)
        readonly property bool floatable: el.em.floatable(ref) && !floating
        Controls.MenuItem {
            text: el.menuRows.length > 1 ? "Buttons: " + (el.menuRows[0] ? el.menuRows[0].title : "") + "…" : "Edit buttons…"
            icon.name: "document-edit-symbolic"
            visible: el.menuRows.length >= 1
            height: visible ? implicitHeight : 0
            onTriggered: el.editRow(0)
        }
        Controls.MenuItem {
            text: "Buttons: " + (el.menuRows[1] ? el.menuRows[1].title : "") + "…"
            icon.name: "document-edit-symbolic"
            visible: el.menuRows.length >= 2
            height: visible ? implicitHeight : 0
            onTriggered: el.editRow(1)
        }
        Controls.MenuSeparator { visible: el.menuRows.length > 0; height: visible ? implicitHeight : 0 }
        Controls.MenuItem {
            text: "Hide"
            icon.name: "view-hidden-symbolic"
            visible: islandMenu.hideable
            height: visible ? implicitHeight : 0
            onTriggered: el.em.hide(islandMenu.ref)
        }
        Controls.MenuItem {
            text: "Float"
            icon.name: "window-pin-symbolic"
            visible: islandMenu.floatable
            height: visible ? implicitHeight : 0
            onTriggered: {
                var r = el.out && el.out.islands ? el.out.islands[islandMenu.ref] : null
                if (r) el.em.floatIsland(islandMenu.ref, r)
            }
        }
        Controls.MenuItem {
            text: "Dock"
            icon.name: "view-restore-symbolic"
            visible: islandMenu.floating
            height: visible ? implicitHeight : 0
            onTriggered: el.em.dock(islandMenu.ref)
        }
        Controls.MenuSeparator { visible: islandMenu.hideable; height: visible ? implicitHeight : 0 }
        Controls.MenuItem {
            text: "Reset this island"
            icon.name: "view-refresh-symbolic"
            visible: islandMenu.hideable && !el.menuButtons
            height: visible ? implicitHeight : 0
            onTriggered: el.em.resetIsland(islandMenu.ref)
        }
        Controls.MenuItem {
            text: "Remove this button island"
            icon.name: "edit-delete-symbolic"
            visible: el.menuButtons
            height: visible ? implicitHeight : 0
            onTriggered: el.em.removeButtonIsland(islandMenu.ref)
        }
        Controls.MenuItem {
            text: "This island stays where the room is"
            enabled: false
            visible: !islandMenu.hideable && !el.em.floatable(islandMenu.ref)
            height: visible ? implicitHeight : 0
        }
    }
    // ---- right-click on a gap in edit mode: Reset these sizes · Done editing (§6) ----
    property var menuGap: null
    function openGapMenu(g, item, mx, my) { el.menuGap = g; gapMenu.popup(item, mx, my) }
    Controls.Menu {
        id: gapMenu
        modal: false
        Controls.MenuItem {
            text: "Reset these sizes"
            icon.name: "view-refresh-symbolic"
            onTriggered: if (el.menuGap) el.em.apply({ op: "resetSizes", a: el.menuGap.a, b: el.menuGap.b }, true)
        }
        Controls.MenuItem {
            text: "Done editing"
            icon.name: "dialog-ok-apply-symbolic"
            onTriggered: el.em.done()
        }
    }

    Component.onCompleted: { el.syncLists(); el.forceActiveFocus() }
}
