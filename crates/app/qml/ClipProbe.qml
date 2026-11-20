// ClipProbe.qml — P22 (12.1.17) test hook: lists the text a window CLIPS, for the text size gate
// (scripts/textsize-gate.sh; the web's tools/textsize-states.mjs runs the same rules in the DOM). Loaded only when the
// gate's lab switch is set (login.autoClipProbe; compiled out of a release), never in normal use. Every second it walks `target` (a window's
// root item: its content and its popups/menus) and prints each NEW finding once to stderr:
//   [clipprobe] <win> v   <path> «text» <needed>><had>   a Text shorter than its text (it draws over its neighbours
//                                                         or is cut by a clipping parent) — never intended;
//   [clipprobe] <win> h   …                              the same sideways, on a one-line Text WITHOUT an elide;
//   [clipprobe] <win> anc … +<px>                        a Text partly outside a parent that clips (any of them up
//                                                         to the first scrolling view, either axis).
// Scrolling views (a Flickable: ListView, ScrollView …) clip their rows on purpose and stop the ancestor walk; an
// elided Text is the intended way to shorten. The gate compares every size with 100 % / Cozy and fails only on what
// is NEW at a bigger size. A finding is printed only when two passes in a row see it (not a layout mid-animation).
// Round 3 (the per-window font scope): when a window has been scanned, a throwaway unsized Label is put in its content
// for a moment and what they got is printed once: "[clipprobe] font <win> <button pt> base <the platform's pt> label <label pt>"
// — the size plain controls in that window draw at (the gate fails a window whose controls don't follow the size).
import QtQuick
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami

Item {
    id: probe
    property Item target: null
    property string win: "main"
    // round 2: every window of the app at once — `windows` (Window objects) with their `names`; a hidden one is skipped
    property var windows: []
    property var names: []
    // round 3: the look the app draws (set by Main.qml), printed whenever it changes: "[clipprobe] look <size> <density>"
    // (the gate checks the Handheld layout draws "touch" whatever the person's density)
    property int lookSize: 0
    property string lookDensity: ""
    onLookSizeChanged: probe.lookLine()
    onLookDensityChanged: probe.lookLine()
    function lookLine() { if (probe.lookSize > 0) console.warn("[clipprobe] look " + probe.lookSize + " " + probe.lookDensity) }
    property var seen: ({})
    // a finding counts only when two passes in a row see it: a window opening or resizing (its layout mid-animation)
    // is not clipped text (the Pad's open animation was caught that way in the first round-2 run)
    property var lastPass: ({})
    property var thisPass: ({})

    function typeName(it) {
        var s = String(it)
        var i = s.indexOf("(")
        s = i > 0 ? s.substring(0, i) : s
        return s.replace(/_QMLTYPE_\d+/, "").replace(/_QML_\d+/, "")
    }
    function path(it) {
        var parts = []
        for (var p = it, n = 0; p && n < 4; p = p.parent, n++) parts.unshift(p.objectName ? p.objectName : typeName(p))
        return parts.join(">")
    }
    function isText(it) {
        return it.text !== undefined && typeof it.text === "string" && it.contentHeight !== undefined
            && it.elide !== undefined && it.wrapMode !== undefined
    }
    function report(kind, it, extra) {
        var key = probe.win + " " + kind + " " + path(it) + " «" + it.text.replace(/\s+/g, " ").substring(0, 60) + "»"
        // persistence by the item itself (String(item) carries its address) and the kind: a clipped live text whose
        // words change every second still counts, two different delegates with the same words don't add up
        var id = kind + " " + String(it)
        probe.thisPass[id] = true
        if (probe.seen[key] || !probe.lastPass[id]) return
        probe.seen[key] = true
        console.warn("[clipprobe] " + key + " " + extra)
    }
    function check(it) {
        if (!it.visible || it.opacity === 0 || it.width < 1 || it.height < 1 || it.text.length === 0) return
        var w = it.Window.window
        if (w) {
            var g = it.mapToItem(null, 0, 0)
            if (g.x > w.width || g.y > w.height || g.x + it.width < 0 || g.y + it.height < 0) return
        }
        if (it.contentHeight > it.height + 2 && !it.truncated)
            report("v", it, Math.round(it.contentHeight) + ">" + Math.round(it.height))
        if (it.elide === Text.ElideNone && it.wrapMode === Text.NoWrap && it.contentWidth > it.width + 2)
            report("h", it, Math.round(it.contentWidth) + ">" + Math.round(it.width))
        // every clipping parent up to the first scrolling view, both axes (a Text that fits an inner clip can still be
        // cut by an outer one)
        for (var a = it.parent; a; a = a.parent) {
            if (a.contentY !== undefined && a.flickableDirection !== undefined) break   // a Flickable scrolls its rows
            if (!a.clip) continue
            var r = it.mapToItem(a, 0, 0)
            var top = r.y, bottom = r.y + Math.min(it.height, it.contentHeight)
            var left = r.x, right = r.x + Math.min(it.width, it.contentWidth)
            var partlyV = top < a.height - 1 && bottom > 1 && (bottom > a.height + 1 || top < -1)
            var partlyH = left < a.width - 1 && right > 1 && (right > a.width + 1 || left < -1)
            if (partlyV) { report("anc", it, "+" + Math.round(Math.max(bottom - a.height, -top)) + " in " + typeName(a)); break }
            if (partlyH) { report("anc", it, "+" + Math.round(Math.max(right - a.width, -left)) + " in " + typeName(a)); break }
        }
    }
    function walk(it, depth) {
        if (!it || depth > 80 || !it.visible || it.opacity === 0) return   // a transparent subtree shows nothing
        if (isText(it)) check(it)
        var kids = it.children
        for (var i = 0; i < kids.length; i++) walk(kids[i], depth + 1)
    }
    // what controls that size nothing themselves get in this window (they inherit the window's font, if the window passes
    // one): a Button, and a Label (under KDE's org.kde.desktop style its Label binds Kirigami.Theme.defaultFont instead)
    Component { id: fontProbeLabelC; Controls.Label { visible: false; text: "probe" } }
    Component { id: fontProbeButtonC; Controls.Button { visible: false; text: "probe" } }
    function fontCheck(content, name) {
        var l = fontProbeLabelC.createObject(content), b = fontProbeButtonC.createObject(content)
        if (!l || !b) {
            console.warn("[clipprobe] font " + name + " none")   // the gate counts a window without a reading as a failure
        } else {
            console.warn("[clipprobe] font " + name + " " + b.font.pointSize.toFixed(2) + " base " + Kirigami.Theme.defaultFont.pointSize.toFixed(2)
                         + " label " + l.font.pointSize.toFixed(2))
        }
        if (l) l.destroy()
        if (b) b.destroy()
    }
    // passes that really walked each window (by name): "[clipprobe] scanned <name>" once two passes did — the gate
    // requires it for the window a state opens, so a window that never opened can't pass as clean
    property var scans: ({})
    function pass() {
        probe.thisPass = ({})
        if (probe.target) probe.walk(probe.target, 0)
        for (var i = 0; i < probe.windows.length; i++) {
            var w = probe.windows[i]
            if (!w || !w.visible || !w.contentItem) continue
            probe.win = probe.names[i] || ("window" + i)
            // the scene's root: an ApplicationWindow's contentItem sits under it (with the menus' overlay); a plain
            // Window's contentItem IS the root (its parent is null)
            var r = w.contentItem
            while (r.parent) r = r.parent
            probe.walk(r, 0)
            var n = (probe.scans[probe.win] || 0) + 1
            probe.scans[probe.win] = n
            if (n === 2) {
                console.warn("[clipprobe] scanned " + probe.win)
                probe.fontCheck(w.contentItem, probe.win)
            }
        }
        probe.lastPass = probe.thisPass
    }
    // "on" once the probe has a window to walk: the gate counts a capture without this line as missing, not clean
    onTargetChanged: if (probe.target) console.warn("[clipprobe] on " + probe.win)
    Component.onCompleted: {
        if (probe.target || probe.windows.length > 0) console.warn("[clipprobe] on " + (probe.names[0] || probe.win))
        probe.lookLine()
    }
    Timer {
        interval: 1000; repeat: true; running: probe.target !== null || probe.windows.length > 0
        onTriggered: probe.pass()
    }
}
