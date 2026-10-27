// DockHost.qml — the flat dock host (ISLANDS-ARCH §2, islands C4: the skeleton; C5 wires it into the window).
//
// Every island is an `Island` child of this one item, forever (no reparenting: calls, video sinks, timeline scroll and
// composer drafts survive any move). The host measures the islands (their contents' implicit sizes, quantized to whole
// pixels, §2.1), hands form + context to the layout engine (LayoutController.solve — the Rust port, byte-identical with
// the web's reference) and writes the answer back in ONE imperative pass: x, y, width, height, z, visible per island.
// Nothing runs per frame and nothing is bound to anything that ticks (GPU.md): the host solves only on events — its
// own resize, a form / view / shape change, or a content-size change an island reports.
//
// Stabilization episodes (§2.1): a size change caused by the host's own new rects is a `reflow`; within one episode the
// host solves at most three times, then freezes the content sizes at the component-wise maximum of the last two
// measurement sets, solves once more and ignores reflows until the next external trigger (logged `[layout] unstable`).
//
// Focus recovery (§2.7): after every applied layout, a focus left inside an island that is no longer visible moves to
// the first visible island in the logical region order (never to nothing) — 10.8: a drawer island's focus (the drawer
// closed around it) goes back to the drawer's handle (`drawerHandle`, the invoker) while that is shown.
//
// 11.0 edit mode (EDITMODE-UI.md): `editing` = every island takes part even when its content says it is empty (§2), the
// last solve's inputs are kept (`lastCtx`: edit mode's drop zones, previews and resizes use the same inputs, LAYOUT §8),
// a stack draws its tab strip (§4.4 — normal use too; a pick = `tabPicked`, the view's `active`), and a right-click on
// the host's own background (a gap, the margin — never an island) asks for the gap menu (§10, `backgroundMenu`).
//
// Gate hook: --tinhat-layoutlog prints every applied rect ("[layout] island:<ref> x,y w×h z=… vis=…") when it changes.
import QtQuick
import dev.tinhat.app   // LayoutController

Item {
    id: host
    property var layoutCtl: null           // a LayoutController
    property string formJson: ""           // the active desktop form (a repaired projection or a preset)
    property var view: ({})                // device-local view state (callShown, roomOpen, forceCollapse, open, …)
    property var shape: ({})               // the theme's shape (gap, margin, radius, …; {} = the preset's)
    // what the island frames draw with (Island.qml reads these from its parent host): the shape's radius and frame width
    readonly property real islandRadius: (host.shape && host.shape.radius !== undefined) ? host.shape.radius : Th.shapeRadius
    readonly property int islandBorder: (host.shape && host.shape.border !== undefined) ? host.shape.border : Th.shapeBorder
    property int unitPm: 1000              // gridUnit 18 = 1000 (§6.6: layout lengths scale with it)
    // the logical region order (x-tinhat.regionOrder): Tab / F6 / screen readers and focus recovery
    property var regionOrder: ["nav", "dms", "spaces", "rooms", "widgets", "account", "header", "call", "timeline", "composer", "panel"]
    readonly property bool logOn: Qt.application.arguments.indexOf("--tinhat-layoutlog") >= 0

    property var lastOut: null             // the last solve output (the gates read it through the layout log)
    // the nav drawer's panel fade + slide (Main.qml binds them to its chrome); the drawer's islands follow them, and while
    // the pointer is over one of them the drawer stays open (drawerIslandHovered → the chrome's hover intent)
    property real drawerOpacity: 1
    property real drawerShift: 0
    property var _drawerHov: ({})
    property bool drawerIslandHovered: false
    function drawerHover(ref, on) {
        var h = host._drawerHov
        h[ref] = on
        var any = false
        for (var k in h) if (h[k]) any = true
        host.drawerIslandHovered = any
    }
    property Item drawerHandle: null       // the nav drawer's handle (ThDock.handleItem): focus returns there when it closes
    property int solves: 0                 // solves so far (tests)
    signal layoutApplied(var out)
    // ── 11.0 edit mode ──
    property bool editing: false           // every island is present (EDITMODE-UI §2): nothing reports `empty`
    property var lastCtx: null             // the inputs of the last solve {size, unitPm, shape, content, view}
    property var tabLabel: null            // function(ref) → a tab's label (the island's name)
    property var absentRefs: []            // while editing: the islands whose real content is absent (Island.qml placeholders)
    property var placeholderTextOf: null   // function(ref) → {icon, name, absent} (EditMode.textOf)
    signal tabPicked(string stackId, string ref)
    signal backgroundMenu(real x, real y)

    // ── the islands (children of type Island, found by objectName) ──
    readonly property var islands: {
        var m = ({})
        for (var i = 0; i < host.children.length; i++) {
            var c = host.children[i]
            if (c && String(c.objectName).indexOf("island:") === 0) m[c.ref] = c
        }
        return m
    }

    // ── measurement (§2.1): q(v) = ceil(round(64 v) / 64) — Qt's 1/64 px text grid, then up to a whole pixel ──
    function q(v) { return Math.ceil(Math.round(64 * Number(v || 0)) / 64) }
    function measure() {
        var content = ({})
        for (var ref in host.islands) {
            var it = host.islands[ref]
            if (!it.contentItem || !it.reports) continue     // unmeasured: the registry's pref / minimum applies
            var e = ({ h: host.q(it.naturalHeight) })
            if (it.empty && !host.editing) e.empty = true
            for (var k in it.extra) e[k] = host.q(it.extra[k])
            if (it.minIsNatural) e.minH = Math.max(e.minH || 0, e.h)   // (the island's own flag: never below its natural)
            content[ref] = e
        }
        return content
    }

    // ── episodes ──
    property int _pass: 0                  // solves in the current episode
    property var _m1: null                 // the last two measurement sets
    property var _m2: null
    property bool _frozen: false
    property var _frozenContent: null
    property bool _inReflowWindow: false   // a size change right after our own apply = reflow
    property bool _pending: false

    function trigger() {                   // an external trigger: a new episode
        host._pass = 0; host._frozen = false; host._frozenContent = null; host._m1 = null; host._m2 = null
        host._schedule()
    }
    function contentChanged() {            // an island's natural size moved
        if (host._inReflowWindow) {
            if (host._frozen) return       // reflows are ignored until the next external trigger
            host._schedule()
        } else host.trigger()              // `data`: a new episode
    }
    function _schedule() {
        if (host._pending) return
        host._pending = true
        Qt.callLater(host._run)
    }
    function _maxOf(a, b) {
        var out = ({})
        for (var ref in a) out[ref] = Object.assign({}, a[ref])
        for (var r in b) {
            if (!out[r]) { out[r] = Object.assign({}, b[r]); continue }
            for (var k in b[r]) if (typeof b[r][k] === "number") out[r][k] = Math.max(out[r][k] || 0, b[r][k])
        }
        return out
    }
    function _run() {
        host._pending = false
        if (!host.layoutCtl || host.formJson.length === 0 || host.width <= 0 || host.height <= 0) return
        var content
        if (host._frozen) content = host._frozenContent
        else {
            content = host.measure()
            host._m2 = host._m1
            host._m1 = content
            host._pass++
            if (host._pass > 3) {
                // still moving after three solves: freeze at the component-wise max of the last two sets
                host._frozen = true
                host._frozenContent = host._maxOf(host._m1 || ({}), host._m2 || ({}))
                content = host._frozenContent
                console.log("[layout] unstable " + Object.keys(content).join(","))
            }
        }
        host.solveWith(content)
    }

    // ── solve + apply ──
    function solveWith(content) {
        var ctx = { size: { w: Math.round(host.width), h: Math.round(host.height) }, unitPm: host.unitPm,
                    shape: host.shape, content: content, view: host.view }
        var out
        try { out = JSON.parse(host.layoutCtl.solve(host.formJson, JSON.stringify(ctx))) } catch (e) { console.warn("[layout] solve failed:", e); return }
        host.solves++
        host.lastCtx = ctx
        host.applyLayout(out)
    }
    function applyLayout(out) {
        if (!out || !out.islands) return
        host._inReflowWindow = true
        for (var ref in host.islands) {
            var it = host.islands[ref], r = out.islands[ref]
            if (!r) { it.visible = false; continue }
            it.x = r.x; it.y = r.y; it.width = r.w; it.height = r.h; it.z = r.z || 0
            it.visible = r.vis !== false && r.w > 0 && r.h > 0
            it.inDrawer = (r.z || 0) >= 20 && (r.z || 0) < 30   // §2.5 z bands: drawer panels 20
            if (host.logOn) host._log(ref, r)
        }
        host.lastOut = out
        host.recoverFocus()
        host.layoutApplied(out)
        Qt.callLater(function () { host._inReflowWindow = false })
    }
    property var _logged: ({})
    function _log(ref, r) {
        var line = "[layout] island:" + ref + " " + r.x + "," + r.y + " " + r.w + "x" + r.h + " z=" + (r.z || 0) + " vis=" + (r.vis !== false)
            + (r.inset && r.inset.l ? " inset.l=" + r.inset.l : "")
        if (host._logged[ref] === line) return
        host._logged[ref] = line
        console.log(line)
    }

    // ── focus recovery (§2.7) ──
    function _islandOf(item) {
        for (var p = item; p; p = p.parent) if (String(p.objectName).indexOf("island:") === 0 && p.parent === host) return p
        return null
    }
    function recoverFocus() {
        var w = host.Window.window
        var f = w ? w.activeFocusItem : null
        var isl = f ? host._islandOf(f) : null
        if (!isl || isl.visible) return
        if (isl.inDrawer && host.drawerHandle && host.drawerHandle.visible) { host.drawerHandle.forceActiveFocus(); return }
        for (var i = 0; i < host.regionOrder.length; i++) {
            var cand = host.islands[host.regionOrder[i]]
            if (cand && cand.visible && cand.contentItem) { cand.contentItem.forceActiveFocus(); return }
        }
        if (w && w.contentItem) w.contentItem.forceActiveFocus()
    }

    // ── triggers ──
    onWidthChanged: host.trigger()
    onHeightChanged: host.trigger()
    onFormJsonChanged: host.trigger()
    onViewChanged: host.trigger()
    onShapeChanged: host.trigger()
    onUnitPmChanged: host.trigger()
    onEditingChanged: host.trigger()
    // content sizes: every island's natural size and extras
    Instantiator {
        model: Object.keys(host.islands)
        delegate: Connections {
            required property string modelData
            target: host.islands[modelData] || null
            function onNaturalHeightChanged() { host.contentChanged() }
            function onNaturalWidthChanged() { host.contentChanged() }
            function onExtraChanged() { host.contentChanged() }
            function onEmptyChanged() { host.contentChanged() }
        }
    }
    Component.onCompleted: host.trigger()

    // ── the gap menu's way in (EDITMODE-UI §10): a right-click on the host's own background — under every island, and
    // a click that lands inside an island's rect (content that takes no clicks) is not a gap ──
    MouseArea {
        z: -1
        anchors.fill: parent
        // right-click; on a touch screen (e.g. a handheld) a long press of 500 ms — a touch arrives as a synthesized left press
        acceptedButtons: Qt.LeftButton | Qt.RightButton
        pressAndHoldInterval: 500
        enabled: !host.editing
        onPressed: (mouse) => {
            var o = host.lastOut
            if (o && o.islands) for (var r in o.islands) {
                var ir = o.islands[r]
                if (ir.vis !== false && ir.w > 0 && ir.h > 0 && mouse.x >= ir.x && mouse.y >= ir.y && mouse.x < ir.x + ir.w && mouse.y < ir.y + ir.h) {
                    mouse.accepted = false
                    return
                }
            }
            if (mouse.button === Qt.RightButton) host.backgroundMenu(mouse.x, mouse.y)
            else if (mouse.source === Qt.MouseEventNotSynthesized) mouse.accepted = false   // a mouse's left click: nothing here
        }
        onPressAndHold: (mouse) => { if (mouse.button === Qt.LeftButton) host.backgroundMenu(mouse.x, mouse.y) }
        onWheel: (wheel) => { wheel.accepted = false }
    }

    // ── stack tab strips (EDITMODE-UI §4.4): equal tabs ≤ 160 px at the stack's top edge; a pick sets view.active ──
    Repeater {
        model: {
            var o = host.lastOut, out = []
            if (o && o.stacks) for (var id in o.stacks) { var st = o.stacks[id]; if (st.vis !== false && st.tab > 0 && st.w > 0) out.push({ id: id, s: st }) }
            return out
        }
        delegate: Item {
            id: strip
            required property var modelData
            readonly property var st: modelData.s
            objectName: "tabs:" + modelData.id
            x: st.x
            y: st.y
            z: (st.z || 0) + 1
            width: st.w
            height: st.tab
            readonly property int tabW: st.tabs.length > 0 ? Math.min(160, Math.floor(st.w / st.tabs.length)) : 0
            Rectangle { anchors.fill: parent; color: Th.dockTabBg; radius: 8 }
            Row {
                Repeater {
                    model: strip.st.tabs
                    delegate: Rectangle {
                        id: tab
                        required property string modelData
                        readonly property bool active: tab.modelData === strip.st.active
                        width: strip.tabW
                        height: strip.height
                        radius: 8
                        color: tab.active ? Th.dockTabBgActive : (tabMa.containsMouse ? Th.dockTabBgHover : Th.dockTabBg)
                        Rectangle {   // square the bottom corners (radius 8 on the top corners only)
                            anchors.left: parent.left; anchors.right: parent.right; anchors.bottom: parent.bottom
                            height: 8
                            color: parent.color
                        }
                        Text {
                            textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                            anchors.fill: parent
                            anchors.leftMargin: 10
                            anchors.rightMargin: 10
                            verticalAlignment: Text.AlignVCenter
                            elide: Text.ElideRight
                            font.pixelSize: 12
                            font.weight: Font.DemiBold
                            color: tab.active ? Th.dockTabFgActive : Th.dockTabFg
                            text: host.tabLabel ? host.tabLabel(tab.modelData) : tab.modelData
                        }
                        activeFocusOnTab: tab.active
                        Accessible.role: Accessible.PageTab
                        Accessible.name: host.tabLabel ? host.tabLabel(tab.modelData) : tab.modelData
                        Keys.onPressed: (event) => {
                            var t = strip.st.tabs, i = t.indexOf(strip.st.active), n = t.length
                            if (event.key === Qt.Key_Left || event.key === Qt.Key_Right) {
                                host.tabPicked(strip.modelData.id, t[(i + (event.key === Qt.Key_Right ? 1 : n - 1)) % n])
                                event.accepted = true
                            } else if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter || event.key === Qt.Key_Space) {
                                host.tabPicked(strip.modelData.id, tab.modelData)
                                event.accepted = true
                            }
                        }
                        MouseArea {
                            id: tabMa
                            anchors.fill: parent
                            hoverEnabled: true
                            onClicked: host.tabPicked(strip.modelData.id, tab.modelData)
                        }
                    }
                }
            }
        }
    }
}
