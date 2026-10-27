// EditMode.qml — 11.0 edit mode's state and actions (contract EDITMODE-UI.md; the web's features/editmode store.ts +
// drag.ts): the open session (LayoutController.edit* — the Rust twin of the web's session.ts: undo / redo over whole
// forms, Cancel, Done once, the host logic that chooses operations), a move in progress (the dragged island, its drop
// zones computed ONCE at the start, the hovered zone and the landing preview computed when the zone changes — never
// per pointer move) and the actions the toolbar, the island chrome, the shortcuts and the settings call. Done saves once
// through LayoutStore (the synced `dev.tinhat.layouts`); Cancel throws the session away. No visuals here: EditLayer.qml
// (the chrome over the dock host) and EditToolbar.qml draw from this.
//
// Gate hooks: --tinhat-editmode opens edit mode after the first layout (captures); TINHAT_GUI_EDITPRESET=<name> picks a
// preset right after (the per-preset captures); [editmode] log lines for every open / step / save.
import QtQuick
import dev.tinhat.app   // LayoutController

QtObject {
    id: em
    required property var app          // Main.qml root (toasts)
    required property var layoutCtl
    property var store: null           // LayoutStore
    property var host: null            // the DockHost (solve inputs, last solve)

    // ---- the session ----
    readonly property var state: {
        var _r = em.layoutCtl.editRev
        try { return JSON.parse(String(em.layoutCtl.editState())) } catch (e) { return ({ open: false }) }
    }
    readonly property bool open: em.state.open === true && em.state.kind === "desktop"
    property bool readOnly: false
    readonly property string formJson: em.open ? JSON.stringify(em.state.form) : ""
    readonly property string startJson: em.open ? JSON.stringify(em.state.start) : ""
    /** a live result that is not a step yet (a gap drag in progress): the host renders it instead of the working copy */
    property string previewJson: ""
    readonly property bool dirty: em.open && em.state.dirty === true
    readonly property bool canUndo: em.open && !em.readOnly && em.state.undo > 0
    readonly property bool canRedo: em.open && !em.readOnly && em.state.redo > 0
    readonly property string name: em.open ? String(em.state.name) : ""
    readonly property string currentPreset: em.open && typeof em.state.form.preset === "string" ? em.state.form.preset : ""
    readonly property var hidden: em.open ? JSON.parse(String(em.layoutCtl.hiddenRefs(em.formJson))) : []
    /** the host form: the preview while a gap drags, else the working copy */
    readonly property string hostFormJson: em.previewJson.length > 0 ? em.previewJson : em.formJson

    property string sessionId: ""          // the layout the open session edits (its Done owns that layout's saves)
    signal opened()
    signal closed()

    function _log(s) { console.log("[editmode] " + s) }
    // the engine for the chrome (the layer's gap drags, the toolbar's thumbnails): JSON text in / out
    function engineApplyOp(formJson, opJson, ctxJson) { return String(em.layoutCtl.applyOp(formJson, opJson, ctxJson)) }
    function engineSolve(formJson, ctxJson) { return String(em.layoutCtl.solve(formJson, ctxJson)) }
    function enginePreset(name) { return String(em.layoutCtl.preset(name)) }
    function toast(text, ms) { if (em.app && em.app.showPassiveNotification) em.app.showPassiveNotification(LinkGate.plain(text), ms || 3000) }

    /** Open a session on the layout this device renders (the reset override included). */
    function openEdit() {
        if (em.open || !em.store) return
        var r = em.store.rendered()
        em.readOnly = r.readOnly === true
        em.previewJson = ""
        em.layoutCtl.editOpen(JSON.stringify({ id: r.id, name: r.name, desktop: r.desktop, phone: r.phone }), "desktop")
        // the sync device begins the session (LAYOUT §9.7): a save another device makes meanwhile becomes `previous`
        if (!em.readOnly && r.source !== "pin") em.layoutCtl.storeBegin(r.id)
        em.sessionId = r.id
        em._log("open " + r.id + " (" + r.source + ")" + (em.readOnly ? " read-only" : ""))
        em.opened()
    }
    function toggle() { if (em.open) em.done(); else em.openEdit() }

    /** Done (EDITMODE-UI §1, §16): save once when something changed, then close. false = the save failed: the session
     *  goes on exactly as it was (start, history, dirty). */
    function done() {
        if (!em.open) return true
        em.cancelDrag()
        em.previewJson = ""
        if (em.readOnly) { em.layoutCtl.editCancel(); em._log("close (read-only)"); em.closed(); return true }
        var ov = em.store.override
        var startTuple = em.store.view.tuple || [0, 0, ""]
        var res = null
        try { res = JSON.parse(String(em.layoutCtl.editFinish())) } catch (e) { res = null }
        if (res) {
            var out = em.store.saveLayout(res.id, { name: res.name, desktop: res.desktop, phone: res.phone }, true)
            em._log("done → save " + res.id + ": " + out)
            if (out !== "saved" && out !== "noop") {
                em.layoutCtl.editReopen()
                em.toast(out === "bad-layout: too large" ? "This layout is too big to save (64 KB). Hide or remove something and try again."
                         : out === "read-only" ? "Saved by a newer Tin Hat — update to edit layouts."
                         : "The layout couldn't be saved (" + out + ").", 6000)
                return false
            }
            if (ov && ov.id === res.id) em.store.clearOverride()
            var now = em.store.view
            if (now.hasPrevious && startTuple[0] > 0 && now.tuple && now.tuple[0] > startTuple[0] + 1)
                em.toast("Saved. A change from your other device is kept as the previous version.", 6000)
        } else {
            em.layoutCtl.storeEnd(em.sessionId)
            em._log("done (nothing changed)")
        }
        em.layoutCtl.editCancel()   // the finished session goes (nothing more is saved)
        em.sessionId = ""
        em.closed()
        return true
    }
    function cancel() {
        if (!em.open) return
        em.cancelDrag()
        em.previewJson = ""
        em.layoutCtl.editCancel()
        if (em.sessionId.length > 0) em.layoutCtl.storeEnd(em.sessionId)
        em.sessionId = ""
        em._log("cancel")
        em.closed()
    }

    // ---- steps ----
    readonly property var _errText: ({
        "bad-op: required island": "That island can't be hidden or put there — the app needs it.",
        "bad-op: locked group": "The room's header, messages and composer stay together.",
        "bad-op: not stackable": "That island can't share a spot as a tab.",
        "bad-op: not floatable": "That island can't float.",
        "bad-op: not hideable": "That island can't be hidden."
    })
    function explain(error) {
        if (error === "bad-op: no room" || error === "closed") return ""
        if (String(error).indexOf("bad-layout:") === 0) return "That would make the layout too big."
        return em._errText[error] || "That move isn't possible."
    }
    function _report(text, what) {
        var r = ({ ok: false, error: "internal" })
        try { r = JSON.parse(String(text)) } catch (e) {}
        em._log(what + " → " + (r.ok ? "ok" : r.error))
        if (!r.ok) { var m = em.explain(r.error); if (m.length > 0) em.toast(m, 3000) }
        return r
    }
    function ctxJson() { return em.host && em.host.lastCtx ? JSON.stringify(em.host.lastCtx) : "" }
    function apply(op, withCtx) {
        if (!em.open || em.readOnly) return ({ ok: false, error: "closed" })
        return em._report(em.layoutCtl.editApply(JSON.stringify(op), withCtx ? em.ctxJson() : ""), JSON.stringify(op))
    }
    function commitForm(form) {
        if (!em.open || em.readOnly) return false
        var ok = em.layoutCtl.editCommit(typeof form === "string" ? form : JSON.stringify(form))
        em._log("commit → " + ok)
        return ok
    }
    function undo() { if (em.open && !em.readOnly && em.dragRef.length === 0 && em.layoutCtl.editUndo()) em._log("undo") }
    function redo() { if (em.open && !em.readOnly && em.dragRef.length === 0 && em.layoutCtl.editRedo()) em._log("redo") }
    function preset(name) { if (em.open && !em.readOnly) em._report(em.layoutCtl.editPreset(name), "preset " + name) }
    function reset() { em.preset("reset") }
    function hide(ref) { em.apply({ op: "hide", ref: ref }) }
    function restore(ref) { em.apply({ op: "restore", ref: ref }) }
    function floatIsland(ref, rect) {
        if (!em.open || em.readOnly || !em.host) return
        em._report(em.layoutCtl.editFloat(ref, JSON.stringify({ x: rect.x, y: rect.y, w: rect.w, h: rect.h }),
                                          Math.round(em.host.width), Math.round(em.host.height)), "float " + ref)
    }
    function dock(ref) { if (em.open && !em.readOnly) em._report(em.layoutCtl.editDock(ref), "dock " + ref) }
    function resetIsland(ref) {
        if (!em.open || em.readOnly) return
        var r = em._report(em.layoutCtl.editResetIsland(ref), "reset island " + ref)
        if (!r.ok && em.explain(r.error).length === 0) em.toast("Can't reset this island here.", 3000)
    }
    /** New button island (EDITMODE-UI §7): `buttons:b<n>` at its registry home (the bottom edge), an empty row — one step. */
    function newButtonIsland() {
        if (!em.open || em.readOnly) return
        var r = em._report(em.layoutCtl.editNewButtonIsland(), "new button island")
        if (!r.ok && r.error === "too-many") { em.toast("You can have 8 button islands.", 3000); return }
        // then the row editor opens for it (its changes are steps of this session)
        if (r.ok && r.ref && em.app && em.app.actions)
            Qt.callLater(function () { em.app.actions.openRowEditor({ island: r.ref, row: "main", form: "desktop", title: em.islandName(r.ref) }) })
    }
    function removeButtonIsland(ref) { if (em.open && !em.readOnly) em._report(em.layoutCtl.editRemoveButtonIsland(ref), "remove " + ref) }
    /** Add widget (§7): the widget gallery; a hidden widgets island comes back first (one undo step). */
    function addWidget() {
        if (em.hidden.indexOf("widgets") >= 0) em.restore("widgets")
        if (em.app && em.app.widgetGalleryRequested) em.app.widgetGalleryRequested()
    }

    // ---- a move in progress (the web's drag.ts) ----
    property string dragRef: ""
    property string dragMode: ""            // "pointer": hit order, hover by position · "keys": keys order, the arrows
    property var zones: []
    property int hover: -1
    property var preview: null              // the landing preview {x, y, w, h} (host px) or null
    property real px: 0                     // the pointer (host px) — the ghost
    property real py: 0
    property string _dragForm: ""
    property string _dragCtx: ""
    property string announce: ""

    /** Start a move of `ref` (placed or hidden). false when it has no zones. */
    function beginDrag(ref, mode, x, y) {
        if (!em.open || em.readOnly || !em.host || !em.host.lastCtx) return false
        var ctx = em.ctxJson()
        var z = null
        try { z = JSON.parse(String(em.layoutCtl.dropZones(em.formJson, ctx, ref))) } catch (e) { z = null }
        var list = z ? (mode === "keys" ? z.keys : z.hit) : null
        if (!list || list.length === 0) { em._log("drag " + ref + ": no zones"); return false }
        em._dragForm = em.formJson
        em._dragCtx = ctx
        em.zones = list
        em.dragMode = mode
        em.hover = -1
        em.preview = null
        em.px = x || 0
        em.py = y || 0
        em.dragRef = ref
        em.announce = em.islandName(ref) + " picked up"
        em._log("drag " + ref + " (" + mode + "): " + list.length + " zones")
        if (mode === "keys") em.setHover(0)
        return true
    }
    function _previewFor(i) {
        var z = em.zones[i]
        if (!z) return null
        var r = null
        try { r = JSON.parse(String(em.layoutCtl.applyOp(em._dragForm, JSON.stringify(z.op), ""))) } catch (e) { return null }
        if (!r || !r.ok) return null
        var o = null
        try { o = JSON.parse(String(em.layoutCtl.solve(JSON.stringify(r.form), em._dragCtx))) } catch (e2) { return null }
        var ir = o && o.islands ? o.islands[em.dragRef] : null
        return (ir && ir.w > 0 && ir.h > 0) ? ({ x: ir.x, y: ir.y, w: ir.w, h: ir.h }) : null
    }
    function setHover(i) {
        if (em.dragRef.length === 0 || i === em.hover) return
        em.preview = i >= 0 ? em._previewFor(i) : null
        em.hover = i
        em.announce = i >= 0 ? em.zoneText(em.zones[i]) : ""
    }
    /** The pointer moved (host px). */
    function movePointer(x, y) {
        if (em.dragRef.length === 0) return
        var hit = -1
        for (var i = 0; i < em.zones.length; i++) {
            var r = em.zones[i].rect
            if (x >= r.x && y >= r.y && x < r.x + r.w && y < r.y + r.h) { hit = i; break }
        }
        if (hit !== em.hover) em.setHover(hit)
        em.px = x
        em.py = y
    }
    function stepHover(dir) {
        if (em.dragRef.length === 0 || em.zones.length === 0) return
        var n = em.zones.length
        em.setHover(em.hover < 0 ? 0 : (em.hover + dir + n) % n)
    }
    function dropDrag() {
        if (em.dragRef.length === 0) return false
        var z = em.hover >= 0 ? em.zones[em.hover] : null
        em._endDrag()
        if (!z) { em.announce = "Cancelled"; return false }
        var r = em.apply(z.op)
        em.announce = r.ok ? "Dropped" : "Cancelled"
        return r.ok
    }
    function cancelDrag() {
        if (em.dragRef.length === 0) return
        em._endDrag()
        em.announce = "Cancelled"
        em._log("drag cancelled")
    }
    function _endDrag() {
        em.hover = -1
        em.preview = null
        em.dragRef = ""
        em.dragMode = ""
        em.zones = []
    }
    readonly property var _sideText: ({ left: "left of", right: "right of", top: "above", bottom: "below" })
    readonly property var _cornerText: ({ tl: "top-left", tr: "top-right", bl: "bottom-left", br: "bottom-right" })
    function zoneText(z) {
        if (!z) return ""
        var op = z.op
        if (op.op === "restore" && op.to && typeof op.to === "object") op = op.to
        var t = em.islandName(String(op.target || ""))
        switch (z.kind) {
        case "edge": return "Dock " + (em._sideText[op.side] || "beside") + " " + t
        case "centre": return "Stack with " + t + " as a tab"
        case "window": return "New " + ((op.side === "left" || op.side === "right") ? "column" : "row") + " on the " + op.side
        case "corner": return "Float in the " + (em._cornerText[op.anchor] || "") + " corner"
        }
        return ""
    }

    // ---- what edit mode calls each island (the web's names.ts) ----
    readonly property var texts: ({
        nav: { name: "Navigation", icon: "go-home-symbolic", about: "Home, Favourites, People, Rooms", absent: "" },
        dms: { name: "DMs", icon: "mail-message-new-symbolic", about: "DM bubbles, while you have unread DMs", absent: "Shows while you have unread DMs" },
        spaces: { name: "Spaces", icon: "view-grid-symbolic", about: "Your spaces and the + button", absent: "" },
        rooms: { name: "Rooms", icon: "view-list-details-symbolic", about: "Space name, filter and the room list", absent: "" },
        widgets: { name: "Widgets", icon: "view-calendar-upcoming-events-symbolic", about: "Up next, Call stats, Who's around", absent: "Add a widget to fill it" },
        account: { name: "Account", icon: "user-symbolic", about: "You, your status and your buttons", absent: "" },
        header: { name: "Room header", icon: "documentinfo-symbolic", about: "The room's name, topic and buttons", absent: "Shows when a room is open" },
        call: { name: "Call", icon: "call-start-symbolic", about: "Call tiles, streams and controls", absent: "Shows during a call" },
        timeline: { name: "Messages", icon: "dialog-messages-symbolic", about: "The room's messages", absent: "Shows when a room is open" },
        composer: { name: "Composer", icon: "document-edit-symbolic", about: "Where you write", absent: "Shows when a room is open" },
        panel: { name: "Room info", icon: "documentinfo-symbolic", about: "Room info and members", absent: "Room info and members" },
        buttons: { name: "Button island", icon: "view-more-horizontal-symbolic", about: "A button row you made yourself", absent: "" }
    })
    function kindOf(ref) { var i = String(ref).indexOf(":"); return i < 0 ? String(ref) : String(ref).substring(0, i) }
    function known(ref) {
        var k = em.kindOf(ref)
        if (!em.texts[k]) return false
        return (k === "buttons") === (String(ref).indexOf(":") >= 0)
    }
    function textOf(ref) {
        if (!em.known(ref)) return { name: "Needs a newer Tin Hat", icon: "help-about-symbolic", about: "Made by a newer Tin Hat — kept as it is", absent: "" }
        var t = em.texts[em.kindOf(ref)]
        if (em.kindOf(ref) !== "buttons") return t
        return { name: em.islandName(ref), icon: t.icon, about: t.about, absent: t.absent }
    }
    function islandName(ref) {
        if (em.kindOf(ref) === "buttons" && em.known(ref)) {
            var b = em.buttonRefs()
            var k = b.indexOf(ref)
            return "Button island" + (k >= 0 ? " " + (k + 1) : "")
        }
        var t = em.texts[em.kindOf(ref)]
        return t ? t.name : (em.known(ref) ? String(ref) : "Needs a newer Tin Hat")
    }
    /** the button islands of the host's last solve, code-point order (k = 1-based position, EDITMODE-UI §2) */
    function buttonRefs() {
        var o = em.host ? em.host.lastOut : null
        var out = []
        if (o && o.islands) for (var r in o.islands) if (em.kindOf(r) === "buttons") out.push(r)
        return out.sort()
    }
    // registry facts the menus need (x-tinhat.islands: hideable / required / floatable)
    readonly property var _hideable: ["nav", "dms", "spaces", "rooms", "widgets", "account", "header", "panel", "buttons"]
    readonly property var _floatable: ["nav", "dms", "spaces", "rooms", "widgets", "account", "panel", "call", "buttons"]
    function hideable(ref) { return em.known(ref) && em._hideable.indexOf(em.kindOf(ref)) >= 0 }
    function floatable(ref) { return em.known(ref) && em._floatable.indexOf(em.kindOf(ref)) >= 0 }
    function floating(ref) {
        var f = em.open ? em.state.form : null
        var fl = f && f.floats ? f.floats : []
        function has(n) {
            if (!n || typeof n !== "object") return false
            if (n.t === "island" && n.i === ref) return true
            var c = n.c || []
            for (var i = 0; i < c.length; i++) if (has(c[i])) return true
            return false
        }
        for (var i = 0; i < fl.length; i++) if (fl[i] && has(fl[i].node)) return true
        return false
    }
}
