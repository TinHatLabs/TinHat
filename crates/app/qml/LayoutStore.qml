// LayoutStore.qml — which layout this device renders (11.0; the web's editmode/layoutDoc.ts + actions/layoutStore.ts,
// the same rules): the synced layouts document `dev.tinhat.layouts` (LAYOUT.md §9, EDITMODE-UI.md §16 — the device side
// is layout_store.rs behind LayoutController.store*), the device prefs `layout-active` (which layout this device uses)
// and `layout-override` (the device-local reset, EDITMODE-UI §9), and — until the first save of a layout — the device
// pref `layout-desktop` (JSON, the buttons step's form), which that first save copies verbatim. While edit mode is open
// (EditMode.qml) the host renders the session's working copy instead.
//
// What renders: TINHAT_LAYOUT (pinned, review captures: shown, never saved) → the override (Classic) → the synced entry
// of the active layout → the pref form → the Classic preset, byte for byte (the dock host's form of 10.8). Every read
// is repaired, every write goes through the engine (setOpt / restore / hide, §8) or repair (LayoutController).
// The button rows live in opts.<island>.rows.<row> (LAYOUT.md §3.9, ACTIONS.md §9.2), your own button islands are
// `buttons:<n>` leaves. A row / button-island change outside edit mode is one save of the active layout once it is in
// the document (before that: the pref, as in the buttons step); inside edit mode it is a step of the session.
import QtQuick

QtObject {
    id: ls
    required property var app
    required property var layoutCtl
    property var editMode: null            // EditMode.qml (Main.qml)

    readonly property string classicJson: JSON.stringify(JSON.parse(String(layoutCtl.preset("classic"))).form)
    readonly property string classicPhoneJson: JSON.stringify(JSON.parse(String(layoutCtl.load("", ""))).phone)
    // (initial bindings, evaluated once at creation; a save assigns them)
    readonly property bool pinned: String(layoutCtl.envForm() || "").length > 0
    property string pinText: String(layoutCtl.envForm() || "")
    property string prefText: (app && app.settings) ? String(app.settings.pref("layout-desktop") || "") : ""
    property string phonePrefText: (app && app.settings) ? String(app.settings.pref("layout-phone") || "") : ""
    property string activePref: (app && app.settings) ? String(app.settings.pref("layout-active") || "") : ""
    property string overridePref: (app && app.settings) ? String(app.settings.pref("layout-override") || "") : ""

    /** the active layout id (device pref `layout-active`, JSON string; "main" when none) */
    readonly property string activeId: {
        var v = ls.activePref
        if (v.length === 0) return "main"
        try { var s = JSON.parse(v); return (typeof s === "string" && s.length > 0) ? s : "main" } catch (e) { return v }
    }
    /** the synced state for the active id (layout_store.rs `view`) — re-read whenever the store changes */
    readonly property var view: {
        var _r = ls.layoutCtl.storeRev
        try { return JSON.parse(String(ls.layoutCtl.storeView(ls.activeId))) } catch (e) { return ({ source: "none", tuple: [0, 0, ""], layouts: [] }) }
    }
    /** the device-local reset (EDITMODE-UI §9): {id, base} or null */
    readonly property var override: {
        var v = ls.overridePref
        if (v.length === 0) return null
        try {
            var o = JSON.parse(v)
            if (!o || o.v !== 1 || o.form !== "classic" || typeof o.id !== "string") return null
            return { id: o.id, base: (o.base && o.base.length === 3) ? o.base : [0, 0, ""] }
        } catch (e) { return null }
    }
    /** where the rendered form comes from: "pin" | "override" | "doc" | "pref" | "classic" */
    readonly property bool overrideOn: ls.override !== null && ls.override.id === (ls.view.id || ls.activeId)   // only for its layout
    readonly property string source: ls.pinned ? "pin" : ls.overrideOn ? "override" : ls.view.source === "doc" ? "doc"
                                     : ls.prefText.length > 0 ? "pref" : "classic"
    function _repaired(t) {
        if (!t || t.length === 0) return ls.classicJson
        try { return JSON.stringify(JSON.parse(String(layoutCtl.repair("desktop", t))).form) } catch (e) { return ls.classicJson }
    }
    /** the desktop form this device renders outside edit mode (canonical JSON text) */
    readonly property string renderedJson: ls.source === "pin" ? ls._repaired(ls.pinText)
                                           : ls.source === "override" ? ls.classicJson
                                           : ls.source === "doc" ? JSON.stringify(ls.view.desktop)
                                           : ls._repaired(ls.prefText)
    readonly property string renderedPhoneJson: {
        if (ls.source === "override") return ls.classicPhoneJson
        if (ls.source === "doc") return JSON.stringify(ls.view.phone)
        if (ls.phonePrefText.length === 0) return ls.classicPhoneJson
        try { return JSON.stringify(JSON.parse(String(layoutCtl.repair("phone", ls.phonePrefText))).form) } catch (e) { return ls.classicPhoneJson }
    }
    /** The form the dock host renders (canonical JSON text): edit mode's working copy while a desktop session is open. */
    readonly property string formJson: (ls.editMode && ls.editMode.open && ls.editMode.hostFormJson.length > 0) ? ls.editMode.hostFormJson : ls.renderedJson
    readonly property var form: { try { return JSON.parse(ls.formJson) } catch (e) { return JSON.parse(ls.classicJson) } }
    // the gates / the sync check read what renders from the log
    onRenderedJsonChanged: {
        var f = null
        try { f = JSON.parse(ls.renderedJson) } catch (e) { f = null }
        console.log("[editmode] renders " + (ls.view.id || ls.activeId) + " (" + ls.source + ") preset " + (f && f.preset ? f.preset : "-")
                    + " rev " + (ls.view.tuple ? ls.view.tuple[0] : 0) + (ls.view.hasPrevious ? " +previous" : ""))
    }
    /** What a session starts from / what saves build on: {id, name, desktop, phone, source, readOnly}. */
    function rendered() {
        return { id: ls.view.id || ls.activeId, name: ls.view.name || "Main", desktop: JSON.parse(ls.renderedJson),
                 phone: JSON.parse(ls.renderedPhoneJson), source: ls.source,
                 readOnly: ls.view.readOnly === true }
    }

    // ---- saves (EDITMODE-UI §16: Done, Keep for all devices, Restore previous, New / Rename / Delete) ----
    /** One save of layout `id`: "saved" | "noop" | "read-only" | "signed-out" | an engine error. Pinned runs never save. */
    function saveLayout(id, edit, fromSession) {
        if (ls.pinned) {
            if (edit && edit.desktop) ls.pinText = JSON.stringify(edit.desktop)
            return "saved"
        }
        // an open edit session owns its layout: only its Done saves it ("busy" for everything else)
        if (fromSession === true) return String(ls.layoutCtl.storeSaveDone(id, JSON.stringify(edit)))
        return String(ls.layoutCtl.storeSave(id, JSON.stringify(edit)))
    }
    function setActive(id) {
        ls.activePref = JSON.stringify(id)
        if (app && app.settings) app.settings.applyPref("layout-active", ls.activePref)
    }
    function setOverride() {
        var v = ls.view
        ls.overridePref = JSON.stringify({ v: 1, id: v.id || ls.activeId, form: "classic", base: v.tuple || [0, 0, ""] })
        if (app && app.settings) app.settings.applyPref("layout-override", ls.overridePref)
        console.log("[editmode] reset override on (" + (v.id || ls.activeId) + ")")
    }
    function clearOverride() {
        ls.overridePref = ""
        if (app && app.settings) app.settings.applyPref("layout-override", "")
    }
    /** Keep for all devices: save Classic as the active layout (the replaced entry becomes `previous`). */
    function keepOverride() {
        var r = ls.saveLayout(ls.view.id || ls.activeId, { name: ls.view.name || "Main", desktop: JSON.parse(ls.classicJson), phone: JSON.parse(ls.classicPhoneJson) })
        if (r === "saved" || r === "noop") ls.clearOverride()
        return r
    }
    function restorePrevious() {
        var p = ls.view.previous
        if (!p) return "none"
        return ls.saveLayout(ls.view.id, { name: ls.view.name, desktop: p.desktop, phone: p.phone })
    }
    function newLayoutId() {
        var known = (ls.view.layouts || []).map(function (l) { return l.id })
        var t = Date.now()
        var id = "l" + t.toString(36)
        while (known.indexOf(id) >= 0) id = "l" + (++t).toString(36)
        return id
    }
    function resolveConflict(take) { return String(ls.layoutCtl.storeResolve(ls.view.id || ls.activeId, take)) }

    /** Your own button islands that are placed (the root tree and the floats), in code-point order. */
    readonly property var buttonIslands: {
        var out = []
        function walk(n) {
            if (!n || typeof n !== "object") return
            if (typeof n.i === "string" && n.i.indexOf("buttons:") === 0 && out.indexOf(n.i) < 0) out.push(n.i)
            if (n.c && n.c.length) for (var k = 0; k < n.c.length; k++) walk(n.c[k])
        }
        walk(ls.form.root)
        var fl = ls.form.floats || []
        for (var i = 0; i < fl.length; i++) if (fl[i]) walk(fl[i].node)
        return out.sort()
    }

    /** The stored row of an island's row as JSON text, "" = none (the preset's default row applies). */
    function rowJson(island, row, formKind) {
        if (formKind !== undefined && formKind !== "desktop") return ""
        var o = ls.form.opts, i = o ? o[island] : null, rows = i ? i.rows : null
        var v = rows ? rows[row] : undefined
        return v === undefined ? "" : JSON.stringify(v)
    }
    /** The stored row as a value (null = none). */
    function rowValue(island, row) {
        var t = ls.rowJson(island, row, "desktop")
        return t.length > 0 ? JSON.parse(t) : null
    }
    function islandName(ref) {
        var n = ls.buttonIslands.indexOf(ref)
        return "Button island " + (n >= 0 ? (n + 1) : "")
    }

    /** Store a form (repaired first). In edit mode: one step of the session. Once the active layout is in the synced
     *  document (or the reset override is on): one save of it (the override ends — you changed what you see). Before
     *  that: the pref (the preset itself removes it). Pinned: shown this run, never saved. */
    function setForm(f) {
        var txt = JSON.stringify(f)
        var canon = txt
        try { canon = JSON.stringify(JSON.parse(String(layoutCtl.repair("desktop", txt))).form) } catch (e) { return "bad-layout" }
        if (ls.editMode && ls.editMode.open) return ls.editMode.commitForm(canon) ? "ok" : "ok"
        if (ls.pinned) { ls.pinText = canon; return "ok" }
        if (ls.source === "doc" || ls.source === "override") {
            var r = ls.saveLayout(ls.view.id || ls.activeId, { name: ls.view.name || "Main", desktop: JSON.parse(canon), phone: JSON.parse(ls.renderedPhoneJson) })
            if (r !== "saved" && r !== "noop") return r
            if (ls.source === "override") ls.clearOverride()
            return "ok"
        }
        var stored = canon === ls.classicJson ? "" : canon
        if (app && app.settings) app.settings.applyPref("layout-desktop", stored)
        ls.prefText = stored
        return "ok"
    }
    function _op(form, op) {
        var r
        try { r = JSON.parse(String(layoutCtl.applyOp(JSON.stringify(form), JSON.stringify(op), ""))) } catch (e) { return { ok: false, error: "bad-op" } }
        return r
    }
    /** Set (value) or remove (null: back to the default row) one row of one island. "ok" or an engine error. */
    function setRow(island, row, value) {
        var o = ls.form.opts && ls.form.opts[island] ? ls.form.opts[island] : {}
        var rows = {}
        if (o.rows) for (var k in o.rows) rows[k] = o.rows[k]
        if (value === null || value === undefined) delete rows[row]; else rows[row] = value
        var op = { op: "setOpt", ref: island, key: "rows" }
        if (Object.keys(rows).length > 0) op.value = rows
        var r = ls._op(ls.form, op)
        if (!r.ok) return r.error || "bad-op"
        return ls.setForm(r.form)
    }

    // ---- your own button islands (the web's addButtonIsland / removeButtonIsland) ----
    readonly property var placements: [
        { id: "roomColumn", label: "Under the room list", to: { op: "dockBeside", target: "account", side: "top" } },
        { id: "underHeader", label: "Under the room header", to: { op: "dockBeside", target: "header", side: "bottom" } },
        { id: "bottom", label: "Along the bottom of the window", to: { op: "edge", side: "bottom" } },
        { id: "float", label: "Floating over the chat, on the right", to: { op: "float", anchor: "r", dx: 16, dy: 0 } }
    ]
    /** A form with a new empty button island at a placement id: {ok, form, ref} | {ok: false, error} ("too-many" at 8). */
    function withButtonIsland(f, placeId, items) {
        var placed = 0
        function walk(n) {
            if (!n || typeof n !== "object") return
            if (typeof n.i === "string" && n.i.indexOf("buttons:") === 0) placed++
            if (n.c && n.c.length) for (var k = 0; k < n.c.length; k++) walk(n.c[k])
        }
        walk(f.root)
        var fls = f.floats || []
        for (var q = 0; q < fls.length; q++) if (fls[q]) walk(fls[q].node)
        if (placed >= 8) return { ok: false, error: "too-many" }
        var used = []
        function collect(n) {
            if (!n || typeof n !== "object") return
            if (typeof n.i === "string" && n.i.indexOf("buttons:") === 0) used.push(n.i)
            if (n.c && n.c.length) for (var k = 0; k < n.c.length; k++) collect(n.c[k])
        }
        collect(f.root)
        for (var q2 = 0; q2 < fls.length; q2++) if (fls[q2]) collect(fls[q2].node)
        var hid = f.hidden || []
        for (var h = 0; h < hid.length; h++) if (hid[h] && typeof hid[h].i === "string") used.push(hid[h].i)
        for (var k2 in (f.opts || {})) if (k2.indexOf("buttons:") === 0) used.push(k2)
        var n = 1
        while (used.indexOf("buttons:b" + n) >= 0) n++
        var ref = "buttons:b" + n
        f.hidden = (f.hidden || []).concat([{ t: "island", i: ref }])
        var to = null
        for (var p = 0; p < ls.placements.length; p++) if (ls.placements[p].id === placeId) to = ls.placements[p].to
        if (!to) to = ls.placements[0].to
        var r = ls._op(f, { op: "restore", ref: ref, to: to })
        if (!r.ok) return { ok: false, error: r.error || "bad-op" }
        var f2 = r.form
        if (!f2.opts) f2.opts = {}
        f2.opts[ref] = { rows: { main: { items: items || [] } } }
        return { ok: true, form: f2, ref: ref }
    }
    /** Add an empty button island at a placement id: {ok, ref} | {ok: false, error} ("too-many" at 8). */
    function addButtonIsland(placeId, items) {
        if (ls.buttonIslands.length >= 8) return { ok: false, error: "too-many" }
        var r = ls.withButtonIsland(JSON.parse(ls.formJson), placeId, items)
        if (!r.ok) return r
        var s = ls.setForm(r.form)
        return s === "ok" ? { ok: true, ref: r.ref } : { ok: false, error: s }
    }
    /** Remove a button island for good (its place and its buttons). */
    function removeButtonIsland(ref) {
        var f = JSON.parse(ls.formJson)
        if (ls.buttonIslands.indexOf(ref) >= 0) {
            var r = ls._op(f, { op: "hide", ref: ref })
            if (!r.ok) return r.error || "bad-op"
            f = r.form
        }
        f.hidden = (f.hidden || []).filter(function (x) { return !(x && x.i === ref) })
        if (f.opts) delete f.opts[ref]
        return ls.setForm(f)
    }
}
