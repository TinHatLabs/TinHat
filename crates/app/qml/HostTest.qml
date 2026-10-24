// HostTest.qml — islands C4's offscreen host test (ISLANDS-ARCH §4.2 C4: "an offscreen qml6 test placing dummy islands
// per 20 vectors"). Run: TINHAT_HOST_TEST=<dir of testvectors/layout/solve> QML_XHR_ALLOW_FILE_READ=1
// QT_QPA_PLATFORM=offscreen tinhat. For each vector: the dummy islands report the vector's content sizes, the REAL
// DockHost measures them (quantized), solves through the LayoutController and applies the rects; PASS when the solve
// output equals the vector's expectation AND every Island item sits exactly at its expected rect / z / visibility.
// Then the episode rules with scripted measurers: a two-cycle (a height that flips with every applied layout) must end
// after at most four solves, frozen at the maximum. 10.8: plus the composer's minimum (Island.minIsNatural), checked
// both ways, and the focus a closing drawer hands back to its handle. Exit 0 = all passed.
import QtQuick
import QtQuick.Window
import dev.tinhat.app

Window {
    id: win
    width: 1600; height: 1000
    visible: true
    title: "Tin Hat host test"

    readonly property string dir: String(Qt.application.arguments.filter(function (a) { return a.indexOf("--hosttest-dir=") === 0 })[0] || "").substring(15)
    readonly property var names: [
        "classic-idle", "classic-call-600", "classic-call-800-capped", "classic-corner-forced", "classic-maximised-call",
        "classic-no-room", "classic-panel-open", "classic-panel-open-1100x800", "classic-700-drawer-open",
        "classic-480x520-call-panel", "callfirst-1200x100-emergency-level", "all-content-zero",
        "call-shown-without-room", "astra-release-counterexample", "random-0002", "random-0065", "random-0077",
        "random-0090", "random-0117", "random-0121"
    ]
    property var files: []
    property int idx: 0
    property int fails: 0
    property int passes: 0

    LayoutController { id: layoutCtl }

    readonly property var refs: ["nav", "dms", "spaces", "rooms", "widgets", "account", "header", "call", "timeline", "composer", "panel"]
    DockHost {
        id: host
        layoutCtl: layoutCtl
        x: 0; y: 0
        Repeater {
            model: win.refs
            delegate: Island {
                required property string modelData
                ref: modelData
                flush: true   // geometry only: the vectors' content sizes are the frames' sizes
                Item { implicitHeight: 0; implicitWidth: 0 }
            }
        }
    }
    // 10.8: a stand-in for the nav drawer's handle (ThDock.handleItem)
    Item { id: dummyHandle; x: 0; y: 0; width: 40; height: 40; activeFocusOnTab: true }
    // the Repeater's delegates are the host's children once created; the host re-reads them
    function islandOf(ref) { return host.islands[ref] }

    function readJson(path) {
        var x = new XMLHttpRequest()
        x.open("GET", "file://" + path, false)
        x.send()
        return JSON.parse(x.responseText)
    }
    function canon(v) {
        if (v === null || typeof v !== "object") return JSON.stringify(v)
        if (Array.isArray(v)) return "[" + v.map(canon).join(",") + "]"
        return "{" + Object.keys(v).sort().map(function (k) { return JSON.stringify(k) + ":" + canon(v[k]) }).join(",") + "}"
    }

    function runVector(name) {
        var v = win.readJson(win.dir + "/" + name + ".json")
        // content: the dummies report the vector's sizes; islands it does not list report nothing
        for (var i = 0; i < win.refs.length; i++) {
            var r = win.refs[i], it = win.islandOf(r)
            var c = v.content ? v.content[r] : undefined
            it.reports = c !== undefined
            it.empty = !!(c && c.empty)
            var ex = ({})
            if (c) for (var k in c) if (k !== "h" && k !== "empty") ex[k] = c[k]
            it.extra = ex
            it.contentItem.implicitHeight = c && typeof c.h === "number" ? c.h : 0
        }
        var form = v.form !== undefined ? v.form : JSON.parse(layoutCtl.preset(v.preset || "classic")).form
        host.width = v.size && typeof v.size.w === "number" ? v.size.w : 0
        host.height = v.size && typeof v.size.h === "number" ? v.size.h : 0
        host.unitPm = typeof v.unitPm === "number" ? v.unitPm : 1000
        host.shape = v.shape !== undefined ? v.shape : ({})
        host.view = v.view !== undefined ? v.view : ({})
        host.formJson = JSON.stringify(form)
        host.solveWith(host.measure())
        var out = host.lastOut
        var ok = true, why = []
        if (!out || win.canon(out.islands) !== win.canon(v.expect.islands)) { ok = false; why.push("islands differ from the vector") }
        for (var j = 0; j < win.refs.length; j++) {
            var ref = win.refs[j], it2 = win.islandOf(ref), e = v.expect.islands[ref]
            if (!e) { if (it2.visible) { ok = false; why.push(ref + " visible without a rect") } continue }
            var vis = e.vis !== false && e.w > 0 && e.h > 0
            if (it2.visible !== vis) { ok = false; why.push(ref + " vis " + it2.visible + " vs " + vis) }
            if (vis && (it2.x !== e.x || it2.y !== e.y || it2.width !== e.w || it2.height !== e.h || it2.z !== (e.z || 0))) {
                ok = false; why.push(ref + " at " + it2.x + "," + it2.y + " " + it2.width + "x" + it2.height + " z" + it2.z + " vs " + e.x + "," + e.y + " " + e.w + "x" + e.h + " z" + (e.z || 0))
            }
        }
        if (ok) win.passes++; else win.fails++
        console.log("[hosttest] " + (ok ? "PASS " : "FAIL ") + name + (why.length ? " — " + why.join("; ") : ""))
    }

    // 10.8 (a tall composer under a call): an island with minIsNatural (the composer) reports its natural height as its
    // minimum (content.minH, LAYOUT.md §7.4) — in a short host the call gives way and the composer keeps what it shows
    // (a reply bar + several lines). Checked both ways: without the flag the same inputs squeeze the composer, so the
    // case discriminates. Runs before the vectors (they reset every island but this flag, which it clears itself).
    function runComposerMin() {
        var ok = true, why = []
        function setup(flag) {
            for (var i = 0; i < win.refs.length; i++) {
                var it = win.islandOf(win.refs[i])
                it.reports = false; it.empty = false; it.extra = ({}); it.minIsNatural = false
                it.contentItem.implicitHeight = 0
            }
            var hs = ({ header: 40, call: 600, composer: 150 })
            for (var r in hs) { var x = win.islandOf(r); x.reports = true; x.contentItem.implicitHeight = hs[r] }
            win.islandOf("composer").minIsNatural = flag
            host.width = 1200; host.height = 700; host.unitPm = 1000; host.shape = ({})
            host.view = ({ roomOpen: true, callShown: true })
            host.formJson = JSON.stringify(JSON.parse(layoutCtl.preset("classic")).form)
            var m = host.measure()
            host.solveWith(m)
            return { m: m, out: host.lastOut }
        }
        var a = setup(true)
        var ca = a.out ? a.out.islands.composer : null, cl = a.out ? a.out.islands.call : null
        if (!a.m.composer || a.m.composer.minH !== 150) { ok = false; why.push("measure: composer.minH " + (a.m.composer ? a.m.composer.minH : "none") + " vs 150") }
        if (!ca || ca.h < 150) { ok = false; why.push("flag on: composer " + (ca ? ca.h : "none") + " px < its natural 150") }
        if (!cl || cl.h >= 600) { ok = false; why.push("flag on: the call did not give way (" + (cl ? cl.h : "none") + ")") }
        var b = setup(false)
        var cb = b.out ? b.out.islands.composer : null
        if (b.m.composer && b.m.composer.minH !== undefined) { ok = false; why.push("flag off: minH reported") }
        if (!cb || cb.h >= 150) { ok = false; why.push("flag off: the composer was not squeezed (" + (cb ? cb.h : "none") + ") — the case does not discriminate") }
        win.islandOf("composer").minIsNatural = false
        if (ok) win.passes++; else win.fails++
        console.log("[hosttest] " + (ok ? "PASS " : "FAIL ") + "composer minIsNatural: on " + (ca ? ca.h : "-") + " px (call " + (cl ? cl.h : "-")
                    + "), off " + (cb ? cb.h : "-") + " px" + (why.length ? " — " + why.join("; ") : ""))
    }

    // 10.8: a focus inside a drawer island goes back to the drawer's handle when the drawer closes around it (ISLANDS-ARCH
    // §2.7, the invoker) — not to the first visible island; without a handle the region order applies as before.
    function runDrawerFocus() {
        var ok = true, why = []
        for (var i = 0; i < win.refs.length; i++) {
            var it = win.islandOf(win.refs[i])
            it.reports = false; it.empty = false; it.extra = ({}); it.minIsNatural = false
            it.contentItem.implicitHeight = 0
        }
        var hs = ({ nav: 228, account: 46, header: 58, composer: 57 })
        for (var r in hs) { var x = win.islandOf(r); x.reports = true; x.contentItem.implicitHeight = hs[r] }
        host.width = 1560; host.height = 960; host.unitPm = 1000; host.shape = ({})
        host.formJson = JSON.stringify(JSON.parse(layoutCtl.preset("classic")).form)
        var rooms = win.islandOf("rooms"), header = win.islandOf("header")
        function drawer(open) {
            host.view = ({ roomOpen: true, forceCollapse: ["navcol"], drawer: { open: open ? "navcol" : null } })
            host.solveWith(host.measure())
        }
        function run(withHandle) {
            host.drawerHandle = withHandle ? dummyHandle : null
            drawer(true)
            if (!rooms.visible || !rooms.inDrawer) { ok = false; why.push("rooms not shown in the open drawer (vis " + rooms.visible + ", inDrawer " + rooms.inDrawer + ")") }
            rooms.contentItem.forceActiveFocus()
            if (!rooms.contentItem.activeFocus) { ok = false; why.push("could not focus the rooms island (window active " + win.active + ")") }
            drawer(false)
            if (rooms.visible) { ok = false; why.push("rooms still visible with the drawer closed") }
            return win.activeFocusItem
        }
        var a = run(true)
        if (a !== dummyHandle) { ok = false; why.push("with a handle the focus went to " + (a ? String(a.objectName || a) : "nothing")) }
        var b = run(false)
        if (b !== header.contentItem) { ok = false; why.push("without a handle the focus went to " + (b ? String(b.parent && b.parent.parent ? b.parent.parent.objectName : b) : "nothing") + ", not the header") }
        host.drawerHandle = null
        if (ok) win.passes++; else win.fails++
        console.log("[hosttest] " + (ok ? "PASS " : "FAIL ") + "drawer focus → handle (fallback: header)" + (why.length ? " — " + why.join("; ") : ""))
    }

    // episodes: a scripted measurer whose height flips on every applied layout (a two-cycle)
    function runEpisode() {
        var it = win.islandOf("rooms")
        var flips = 0
        var conn = function () { flips++; it.contentItem.implicitHeight = (flips % 2) ? 300 : 340 }
        host.layoutApplied.connect(conn)
        var before = host.solves
        host.trigger()
        // let the queued solves run
        episodeCheck.before = before
        episodeCheck.conn = conn
        episodeCheck.start()
    }
    Timer {
        id: episodeCheck
        property int before: 0
        property var conn: null
        interval: 400
        onTriggered: {
            host.layoutApplied.disconnect(conn)
            var n = host.solves - before
            var ok = n <= 4 && host._frozen
            if (ok) win.passes++; else win.fails++
            console.log("[hosttest] " + (ok ? "PASS " : "FAIL ") + "episode two-cycle: " + n + " solves, frozen=" + host._frozen)
            console.log("[hosttest] done: " + win.passes + " passed, " + win.fails + " failed")
            Qt.exit(win.fails > 0 ? 1 : 0)
        }
    }

    Timer {
        interval: 300; running: true
        onTriggered: {
            if (win.dir.length === 0) { console.log("[hosttest] FAIL no --hosttest-dir"); Qt.exit(2); return }
            try { win.runComposerMin() } catch (e) { win.fails++; console.log("[hosttest] FAIL composer minIsNatural — " + e) }
            try { win.runDrawerFocus() } catch (e) { win.fails++; console.log("[hosttest] FAIL drawer focus — " + e) }
            for (var i = 0; i < win.names.length; i++) {
                try { win.runVector(win.names[i]) } catch (e) { win.fails++; console.log("[hosttest] FAIL " + win.names[i] + " — " + e) }
            }
            win.runEpisode()
        }
    }
}
