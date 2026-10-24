// ThemePickOverlay.qml — the theme editor's Pick tool on the desktop (THEMES-UI.md §15; the web's PickOverlay.tsx +
// pick.ts): while armed, a catcher covers the app (under the editor panel — the panel itself is not pickable): the pointer
// is a crosshair and the item under it gets a 2 px settings.themeEditor.pick.outline over a pick.fill veil — moved only
// when the pointer reaches another item (nothing is drawn per frame). A click samples and disarms; Esc disarms.
//
// Sampling (§15.2): the deepest visible item under the point (child-at recursion from the host's pickRoot, skipping this
// overlay and the editor panel), then its visual parents — at most 8 layers. Per item: every colour property of its
// meta-object, plus border.color, gradient stop colours and shadow.color. THE PROBE PASS, in one call with no event-loop
// turn in between (bindings update at once; the scene graph syncs only after the GUI thread returns to its loop, so no
// frame is ever drawn in the probe colours): the motion switch off (Motion.probing: every duration 0), Th.applyColours(
// the probe colours) — every role and token a colour encoding its index —, read, Th.applyColours(the real colours), the
// switch back. A property counts when its probe value differs from its real value and decodes (ThemeEditorController
// .pickList: the shared codec + pickList, vectors editor/probe-*).
import QtQuick
import QtQuick.Controls as Controls
import dev.tinhat.app   // Th: settings.themeEditor.pick.*

Item {
    id: ov
    property bool armed: false
    property var panel: null                // ThemeEditorPanel (ctl, host, showPicked)

    visible: ov.armed
    property Item hovered: null
    property rect hoverRect: Qt.rect(0, 0, 0, 0)
    /// the review harness pointed at an item itself (testHover): the real pointer is ignored until the tool disarms
    property bool testPinned: false
    onArmedChanged: if (!ov.armed) { ov.hovered = null; ov.testPinned = false }

    readonly property var skip: ({
        "palette": 1, "layer": 1, "anchors": 1, "data": 1, "resources": 1, "children": 1, "visibleChildren": 1,
        "states": 1, "transitions": 1, "transform": 1, "parent": 1, "font": 1, "containmentMask": 1, "contentItem": 1,
        "background": 1, "childrenRect": 1, "Layout": 1, "Keys": 1, "objectName": 1, "text": 1
    })

    /// The item that paints the topmost colour at the scene point (gx, gy) under `root`, or null — the desktop's
    /// elementsFromPoint: the items in reverse paint order (children above their parent, then siblings by z and
    /// declaration order, topmost first), the first whose own rect holds the point and that paints a colour there
    /// (paintsAt). A parent need not hold the point (QML children may lie outside their parent — a zero-size delegate
    /// wrapper, a Column) unless it clips; invisible and fully transparent subtrees, this overlay and the editor panel
    /// are skipped. Layers that paint nothing there (a transparent full-window layer, an island's border-only frame
    /// above its content) never end the search.
    property int _visits: 0
    function deepestAt(root, gx, gy) { ov._visits = 0; return ov._pickIn(root, gx, gy, 0) }
    function _pickIn(node, gx, gy, depth) {
        if (!node || depth > 80 || ++ov._visits > 40000) return null
        var kids = []
        var ch = node.children
        for (var i = 0; i < ch.length; i++) kids.push({ item: ch[i], z: ch[i].z, i: i })
        kids.sort(function (a, b) { return (b.z - a.z) || (b.i - a.i) })
        for (var k = 0; k < kids.length; k++) {
            var c = kids[k].item
            if (!c || !c.visible || c.opacity <= 0 || c === ov || (ov.panel && c === ov.panel)) continue
            var p = c.mapFromItem(null, gx, gy)
            var inside = p.x >= 0 && p.y >= 0 && p.x < c.width && p.y < c.height
            if (c.clip && !inside) continue
            var deeper = ov._pickIn(c, gx, gy, depth + 1)
            if (deeper) return deeper
            if (inside && ov.paintsAt(c, p.x, p.y)) return c
        }
        return null
    }
    /// does the item paint a colour of its own at its local point (px, py)? A colour property that paints now with some
    /// alpha — a border only within its width of the edge (an island's frame is a border-only Rectangle ABOVE the
    /// content: it must not hide the content inside it)
    function paintsAt(item, px, py) {
        var acc = ov.accessors(item)
        for (var i = 0; i < acc.length; i++) {
            var c = acc[i].get()
            if (!c || c.a <= 0) continue
            if (acc[i].kind === "border") {
                var bw = item.border.width
                if (px < bw || py < bw || px >= item.width - bw || py >= item.height - bw) return true
                continue
            }
            return true
        }
        return false
    }
    function kindOf(item, key) {
        if (key === "color") {
            if (item.isMask !== undefined || item.source !== undefined && item.text === undefined) return "icon"
            if (item.text !== undefined && item.font !== undefined) return "text"
            return "fill"
        }
        // a component's own colour properties, by their names (Island: fill / borderColor, a card's ringColour …)
        var k = String(key).toLowerCase()
        if (k.indexOf("border") >= 0 || k.indexOf("outline") >= 0 || k.indexOf("ring") >= 0) return "border"
        if (k.indexOf("shadow") >= 0) return "shadow"
        if (k.indexOf("icon") >= 0 || k.indexOf("glyph") >= 0) return "icon"
        if (k.indexOf("text") >= 0 || k === "fg" || /fg$/.test(k) || k.indexOf("ink") >= 0) return "text"
        if (k.indexOf("fill") >= 0 || k === "bg" || /bg$/.test(k) || k.indexOf("background") >= 0) return "fill"
        return "other"
    }
    function isColour(v) { return v !== null && v !== undefined && typeof v === "object" && v.hsvHue !== undefined && v.a !== undefined }
    /// a colour property that paints nothing right now — a text's link colour without a link, its selection colours
    /// without a selection, a field's placeholder colour behind text — is not "under the pointer" (the web samples only
    /// what its elements paint: color, backgrounds, borders …)
    function paintsNothing(item, key) {
        try {
            if (key === "linkColor") return !(typeof item.text === "string" && item.text.indexOf("<a") >= 0)
            if (key === "selectionColor" || key === "selectedTextColor") return !(typeof item.selectedText === "string" && item.selectedText.length > 0)
            if (key === "placeholderTextColor") return !(item.text === "" && typeof item.placeholderText === "string" && item.placeholderText.length > 0)
        } catch (e) { }
        return false
    }
    /// The colour accessors of one item: [{kind, get: () => colour}]
    function accessors(item) {
        var out = []
        for (var key in item) {
            if (ov.skip[key] === 1 || key.charAt(0) === "_") continue
            var v
            try { v = item[key] } catch (e) { continue }
            if (typeof v === "function") continue
            if (ov.isColour(v) && !ov.paintsNothing(item, key)) {
                (function (k) { out.push({ kind: ov.kindOf(item, k), get: function () { return item[k] } }) })(key)
            }
        }
        try {
            // a border paints only with a width (a Rectangle's border.color defaults to black)
            if (item.border && item.border.width > 0 && ov.isColour(item.border.color)) out.push({ kind: "border", get: function () { return item.border.color } })
        } catch (e1) { }
        try {
            if (item.gradient && item.gradient.stops) {
                var stops = item.gradient.stops
                for (var s = 0; s < stops.length; s++)
                    (function (st) { if (ov.isColour(st.color)) out.push({ kind: "fill", get: function () { return st.color } }) })(stops[s])
            }
        } catch (e2) { }
        try {
            if (item.shadow && item.shadow.size > 0 && ov.isColour(item.shadow.color)) out.push({ kind: "shadow", get: function () { return item.shadow.color } })
        } catch (e3) { }
        return out
    }
    function describe(item) {
        var s = String(item)
        var m = /^([A-Za-z_][A-Za-z0-9_]*)/.exec(s)
        return m ? m[1] : "item"
    }
    /// §15.2: sample the layers at the scene point → the Pick list [{id, kind}] (nearest layer first).
    function sampleAt(gx, gy) {
        var root = ov.panel && ov.panel.host ? ov.panel.host.pickRoot : null
        if (!root || !ov.panel.ctl) return []
        var deepest = ov.deepestAt(root, gx, gy)
        var chain = []
        for (var it = deepest; it && it !== root && chain.length < 8; it = it.parent) chain.push(it)
        if (chain.length === 0) return []
        var acc = chain.map(function (item) { return ov.accessors(item) })
        var real = acc.map(function (list) { return list.map(function (a) { return String(a.get()) }) })
        var probeColours = ov.panel.ctl.probeMap()
        var realColours = ov.panel.host.realColours()
        // ---- the probe pass: one call, no event-loop turn, no animation
        Motion.probing = true
        Th.applyColours(probeColours)
        var probed = acc.map(function (list) { return list.map(function (a) { return String(a.get()) }) })
        Th.applyColours(realColours)
        Motion.probing = false
        // ----
        var layers = chain.map(function (item, i) {
            var props = []
            for (var j = 0; j < acc[i].length; j++) props.push({ kind: acc[i][j].kind, real: real[i][j], probe: probed[i][j] })
            return { element: ov.describe(item), props: props }
        })
        return JSON.parse(ov.panel.ctl.pickList(JSON.stringify(layers)))
    }
    function hover(gx, gy) {
        var root = ov.panel && ov.panel.host ? ov.panel.host.pickRoot : null
        var it = root ? ov.deepestAt(root, gx, gy) : null
        if (it === ov.hovered) return
        ov.hovered = it
        if (it) {
            var p = it.mapToItem(ov, 0, 0)
            ov.hoverRect = Qt.rect(p.x, p.y, it.width, it.height)
        }
    }

    /// the review harness: hover the scene point (gx, gy) as a pointer would, and keep it there
    function testHover(gx, gy) { ov.testPinned = false; ov.hover(gx, gy); ov.testPinned = true }

    Shortcut { sequence: "Esc"; enabled: ov.armed; context: Qt.WindowShortcut; onActivated: { if (ov.panel) ov.panel.pickArmed = false } }

    MouseArea {
        id: catcher
        anchors.fill: parent
        hoverEnabled: true
        acceptedButtons: Qt.LeftButton | Qt.RightButton
        cursorShape: Qt.CrossCursor
        onPositionChanged: (m) => { if (ov.testPinned) return; var g = catcher.mapToItem(null, m.x, m.y); ov.hover(g.x, g.y) }
        onExited: if (!ov.testPinned) ov.hovered = null
        onClicked: (m) => {
            if (m.button !== Qt.LeftButton) return
            var g = catcher.mapToItem(null, m.x, m.y)
            var list = ov.sampleAt(g.x, g.y)
            ov.hovered = null
            ov.panel.showPicked(list)
        }
    }
    Rectangle {
        visible: ov.hovered !== null
        x: ov.hoverRect.x; y: ov.hoverRect.y
        width: ov.hoverRect.width; height: ov.hoverRect.height
        radius: 4
        color: Th.settingsThemeEditorPickFill
        border.width: 2
        border.color: Th.settingsThemeEditorPickOutline
    }
}
