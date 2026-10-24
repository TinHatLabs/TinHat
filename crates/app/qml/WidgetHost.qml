// WidgetHost.qml — widgets (modular UI step 2): the stack of widget islands under the room list, above the account
// card (contract WIDGETS.md §1, WIDGETS-UI.md §1–§2, §7; the web's features/widgets/Stack.tsx). Each widget is its own
// small island (WidgetFrame).
//
// Where it lives: ThRoomList puts it under its room list (side margins 6, bottom margin 61 + the one-time card), the
// list takes the rest (≥ 160 px); `maxHeight` is what the column can spare. When the stack does not fit, widgets fold
// to their headers from the bottom up (not saved; a folded widget the user opens takes priority and the next one up
// folds instead); if even the headers do not fit, the stack scrolls. With no widgets (`[]`) nothing at all is drawn —
// the room panel is pixel-identical to the app before widgets (`shown` false). Corner mode: no widgets in v1.
//
// Low GPU (GPU.md): the host owns its widgets' window state — "focused" | "unfocused" | "hidden" (R1: any of our
// windows active = focused; minimised / not shown = hidden) — and `animate` (focused AND NOT reduce motion). Widgets
// move only on real data changes, one-shot, ≤ 400 ms, and snap while `animate` is off. The app-wide window state of
// the GPU work replaces this one when it lands.
//
// Storage (until the dock host's synced layout): this device, the pref `ui-widgets` = the list
// [{w, size?, settings?, collapsed?}] (LAYOUT.md §3.9 opts.widgets shape; a size equal to the widget's default is left
// out, like the web's store); none stored = the registry's defaults; `[]` = no widgets. Read and repaired by
// crates/widgets registry (WidgetsController.normalizeItems). `TINHAT_WIDGETS` (JSON) pins the list for the gates
// (the pixel gate runs every pre-widget state with `TINHAT_WIDGETS=[]`); a pinned list is never saved.
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the active theme (docs/MIGRATING-THEME.md)

Item {
    id: host
    required property var app                // Main.qml root
    property real maxHeight: 400             // what the room column can spare for the stack (px)
    readonly property bool shown: host.items.length > 0 && !!host.app && host.app.navMode !== "corner"

    // ── window state + motion switch (GPU.md §1): the app-wide ones (Motion, bound by AppCore) ──
    readonly property string winState: Motion.winState
    readonly property bool reduceMotion: Motion.reducedMotion
    readonly property bool animate: Motion.animate
    // R3: the duration token's steps, 0 while `animate` is off
    readonly property int dur: Motion.dur
    readonly property int durLong: Motion.durLong
    readonly property int durVeryLong: Motion.durVeryLong

    // ── the data controllers (one per process; the stack is their only user) ──
    readonly property alias callStats: callStatsCtl
    readonly property alias widgets: widgetsCtl
    readonly property alias justMe: justMeCtl        // widgets W2: "Is it just me?" (WIDGETS.md §20)
    readonly property alias memories: memoriesCtl    // widgets W2: Memories (WIDGETS.md §21)
    CallStatsController { id: callStatsCtl }
    WidgetsController { id: widgetsCtl }
    JustMeController { id: justMeCtl }
    MemoriesController { id: memoriesCtl }
    onWinStateChanged: { callStatsCtl.setWindow(host.winState); widgetsCtl.setWindow(host.winState); justMeCtl.setWindow(host.winState); memoriesCtl.setWindow(host.winState) }
    // the engines run only while something shows their data: the stack, or the gallery's previews
    readonly property bool needData: (host.shown || gallery.opened) && !!host.app && host.app.mLoggedIn === true
    onNeedDataChanged: host._start()
    readonly property bool needCallStats: host.needData && (gallery.opened || host.has("callStats"))
    onNeedCallStatsChanged: callStatsCtl.setActive(host.needCallStats)
    function _start() {
        if (!host.needData) return
        widgetsCtl.start()
        callStatsCtl.start()
        callStatsCtl.setActive(host.needCallStats)
    }
    Connections {
        target: host.app
        ignoreUnknownSignals: true
        function onWidgetGalleryRequested() { gallery.open() }
        // widgets W2: sign-out → the checker forgets its answers
        function onMLoggedInChanged() { if (host.app && host.app.mLoggedIn !== true) { justMeCtl.reset(); memoriesCtl.reset() } }
    }
    Component.onCompleted: {
        callStatsCtl.setWindow(host.winState)
        widgetsCtl.setWindow(host.winState)
        justMeCtl.setWindow(host.winState)
        memoriesCtl.setWindow(host.winState)
        host.load()
        host._start()
    }
    // GUI-verification hook (screenshots without coordinate clicks, like --tinhat-themes): --tinhat-widgets=gallery
    // opens the gallery once the account is signed in
    readonly property bool _galleryHook: Qt.application.arguments.indexOf("--tinhat-widgets=gallery") >= 0
    property bool _galleryHookDone: false
    readonly property bool _galleryHookReady: host._galleryHook && !host._galleryHookDone && !!host.app && host.app.mLoggedIn === true
    on_GalleryHookReadyChanged: if (host._galleryHookReady) { host._galleryHookDone = true; hookTimer.start() }
    Timer { id: hookTimer; interval: 1500; onTriggered: gallery.open() }

    // ── the registry (widgets.json, the contract copy in crates/widgets) ──
    readonly property var registry: {
        try { return JSON.parse(String(widgetsCtl.registryJson())) } catch (e) { return ({ widgets: [], defaults: [] }) }
    }
    function regOf(w) {
        var list = host.registry.widgets || []
        for (var i = 0; i < list.length; ++i) if (list[i].id === w) return list[i]
        return null
    }
    function regHasSettings(r) { var st = r ? r["settings"] : null; return !!st && st.length > 0 }

    // ── the list ──
    property var items: []                   // [{w, size?, settings?, collapsed?}] — order = stack order
    property var sizes: []                   // the size each renders at (registry sizeOf)
    property var settingsOf: []              // the settings each runs with (registry settingsOf)
    property bool pinned: false              // TINHAT_WIDGETS pins the list (gates): never saved
    // selfhost: a widget of a feature the server doesn't offer (server_offer.rs WIDGET_GATES) is not drawn, but it stays
    // in the saved list where it was (the web's Stack: "it stays in the list") — parked here with its place in the
    // whole list, put back at that place on every save, and drawn again once the server offers it (load() on every
    // change of the offer)
    property var _parked: []                 // [{it, at}]: a hidden widget and its index in the whole list
    function _apply(r) {
        var items = r.items || [], sizes = r.sizes || [], sets = r.settings || []
        var keep = [], park = []
        for (var i = 0; i < items.length; ++i) {
            if (!host.app || !host.app.widgetOffered || host.app.widgetOffered(items[i].w)) keep.push(i)
            else park.push({ it: items[i], at: i })
        }
        host._parked = park
        host.items = keep.map(function (i) { return items[i] })
        host.sizes = keep.map(function (i) { return sizes[i] })
        host.settingsOf = keep.map(function (i) { return sets[i] })
    }
    function _withParked(list) {
        var out = list.slice()
        for (var i = 0; i < host._parked.length; ++i) {
            var p = host._parked[i]
            if (out.some(function (it) { return it.w === p.it.w })) continue
            out.splice(Math.min(p.at, out.length), 0, p.it)
        }
        return out
    }
    // the server's answer changed (sign-in, a refresh): the stack follows it
    Connections {
        target: host.app
        function onMServerOfferChanged() { host.load() }
    }
    function _normalize(v) { return JSON.parse(String(widgetsCtl.normalizeItems(JSON.stringify(v)))) }
    function load() {
        var env = String(widgetsCtl.envItems() || "")
        var raw = env.length > 0 ? env : ((host.app && host.app.settings) ? String(host.app.settings.pref("widgets") || "") : "")
        host.pinned = env.length > 0
        var v = null
        try { v = raw.length > 0 ? JSON.parse(raw) : null } catch (e) { v = null }
        var r = host._normalize(v)
        if (r.items === null) r = host._normalize(JSON.parse(String(widgetsCtl.defaultItems())))
        host._apply(r)
    }
    function save(list) {
        host._apply(host._normalize(host._withParked(list)))
        if (!host.pinned && host.app && host.app.settings) host.app.settings.applyPref("widgets", JSON.stringify(host._withParked(host.items)))
    }
    function _copy() { return host.items.map(function (it) { var o = {}; for (var k in it) o[k] = it[k]; return o }) }
    function indexOfId(w) { for (var i = 0; i < host.items.length; ++i) if (host.items[i].w === w) return i; return -1 }
    // a size equal to the widget's default is left out (the web's setSize)
    function setSize(i, s) {
        var a = host._copy(); var r = host.regOf(a[i].w)
        if (r && s === r["default"]) delete a[i].size; else a[i].size = s
        host.save(a)
    }
    function setCollapsed(i, c) { var a = host._copy(); if (c) a[i].collapsed = true; else delete a[i].collapsed; host.save(a) }
    function setSetting(i, id, v) { var a = host._copy(); var s = {}; var old = a[i].settings || {}; for (var k in old) s[k] = old[k]; s[id] = v; a[i].settings = s; host.save(a) }
    function remove(i) { var a = host._copy(); a.splice(i, 1); host.save(a) }
    function has(w) { return host.items.some(function (it) { return it.w === w }) }
    property string _justAdded: ""
    function add(w) {
        var r = host.regOf(w)
        if (!r || host.has(w) || host.items.length >= 12) return
        var a = host._copy(); a.push({ w: w }); host._justAdded = w; host.save(a)
    }
    function removeId(w) { var i = host.indexOfId(w); if (i >= 0) host.remove(i) }
    // move to position `to` (0 = top), like the web's moveWidget
    function move(from, to) {
        if (from < 0 || from >= host.items.length) return
        to = Math.max(0, Math.min(host.items.length - 1, to))
        if (from === to) return
        var a = host._copy(); var it = a.splice(from, 1)[0]; a.splice(to, 0, it); host.save(a)
    }
    // the header of a widget: S → its default size (M when S is the default); auto-folded → open it with priority;
    // else collapse / expand (saved)
    function headerClicked(i) {
        var it = host.items[i]; if (!it) return
        var r = host.regOf(it.w)
        if (host.sizes[i] === "S") { host.setSize(i, r && r["default"] !== "S" ? r["default"] : "M"); return }
        if (host.folded[i] === true) { host.pinnedOpen = it.w; return }
        host.setCollapsed(i, !(it.collapsed === true))
    }

    // ── auto-fold: which widgets show only their header, bottom-up, so the stack fits ──
    readonly property int gap: 6
    readonly property int addRowH: 22
    property string pinnedOpen: ""           // an auto-folded widget the user opened (takes priority, not saved)
    onItemsChanged: if (host.pinnedOpen.length > 0 && !host.items.some(function (it) { return it.w === host.pinnedOpen && it.collapsed !== true })) host.pinnedOpen = ""
    readonly property var folded: {
        var n = host.items.length
        var c = frames.count                 // re-evaluate once the frames exist
        var f = ({})
        var total = host.addRowH + 4         // "+ Add widget" sits 4 px under the last widget
        var open = []
        for (var i = 0; i < n; ++i) {
            var fr = frames.itemAt(i)
            var h = fr ? fr.openHeight : 32
            open.push(fr ? fr.openHeight - fr.foldedHeight : 0)
            total += h + (i > 0 ? host.gap : 0)
        }
        for (var j = n - 1; j >= 0 && total > host.maxHeight + 0.5; --j) {   // 0.5: sub-pixel sums never fold
            if (open[j] <= 0 || (host.items[j] && host.items[j].w === host.pinnedOpen)) continue
            f[j] = true
            total -= open[j]
        }
        return f
    }
    readonly property real naturalHeight: stack.height + 4 + host.addRowH
    // 11.0 (islands C6/C7): in the dock host the widgets are an island of their own — its natural height is the stack
    // with every widget OPEN (`openTotal`), the least it can take is every widget folded to its header (`foldedTotal`,
    // reported as minH); the island's height comes back as maxHeight and the fold rule above fits the stack into it.
    property bool islandMode: false
    readonly property real openTotal: host._total(false)
    readonly property real foldedTotal: host._total(true)
    function _total(folded) {
        var c = frames.count
        var t = host.addRowH + 4
        for (var i = 0; i < host.items.length; ++i) {
            var fr = frames.itemAt(i)
            t += (fr ? (folded ? fr.foldedHeight : fr.openHeight) : 32) + (i > 0 ? host.gap : 0)
        }
        return t
    }

    visible: host.shown
    implicitHeight: host.shown ? (host.islandMode ? host.openTotal : Math.min(host.maxHeight, host.naturalHeight)) : 0
    implicitWidth: 280

    // drag reorder state (WIDGETS-UI.md §2): the dragged widget's index and the slot it would land in
    property int dragFrom: -1
    property int dragTo: -1
    property real dropLineY: 0
    Keys.onEscapePressed: if (host.dragFrom >= 0) host.cancelDrag()
    function cancelDrag() { host.dragFrom = -1; host.dragTo = -1 }

    Flickable {
        id: flick
        anchors.fill: parent
        contentHeight: host.naturalHeight
        interactive: host.naturalHeight > height + 0.5
        clip: interactive
        boundsBehavior: Flickable.StopAtBounds
        Controls.ScrollBar.vertical: FoxScrollBar { app: host.app; visible: flick.interactive }

        Column {
            id: stack
            width: flick.width
            spacing: host.gap

            Repeater {
                id: frames
                model: host.items.length
                delegate: WidgetFrame {
                    id: frame
                    required property int index
                    readonly property var item: host.items[index] || ({})
                    readonly property var reg: host.regOf(item.w)
                    width: stack.width
                    app: host.app
                    hostRef: host
                    widgetId: String(item.w || "")
                    known: frame.reg !== null
                    title: frame.reg ? frame.reg.title : "Widget"
                    iconName: frame.reg ? frame.reg.icon : "dialog-question-symbolic"
                    sizes: frame.reg ? frame.reg.sizes : []
                    hasSettings: host.regHasSettings(frame.reg)
                    maxH: frame.reg && frame.reg.maxH ? frame.reg.maxH : ({ M: 220, L: 420 })
                    size: host.sizes[index] || "M"
                    settings: host.settingsOf[index] || ({})
                    userCollapsed: item.collapsed === true
                    autoFolded: host.folded[index] === true
                    canMoveUp: index > 0
                    canMoveDown: index < host.items.length - 1
                    dragging: host.dragFrom === index
                    onSizeRequested: (s) => host.setSize(frame.index, s)
                    onHeaderClicked: host.headerClicked(frame.index)
                    onRemoveRequested: host.remove(frame.index)
                    onMoveRequested: (delta) => host.move(frame.index, frame.index + delta)
                    onSettingsRequested: settingsDialog.openFor(frame.index)
                    onAddRequested: gallery.open()
                    onDragStarted: { host.dragFrom = frame.index; host.dragTo = frame.index; host.forceActiveFocus() }
                    onDragMoved: (centerY) => host.dropAt(centerY)
                    onDragEnded: (commit) => { var f = host.dragFrom, t = host.dragTo; host.cancelDrag(); if (commit && f >= 0 && t >= 0) host.move(f, t) }
                    Component.onCompleted: {
                        // rowSlide: a widget added from the gallery slides in (long, OutCubic) — never at start
                        if (host._justAdded.length > 0 && host._justAdded === frame.widgetId) {
                            host._justAdded = ""
                            if (host.animate) frame.slideIn()
                        }
                    }
                }
            }
        }

        // ── + Add widget (22 px, radius 6), 4 px under the last widget ──
        Controls.AbstractButton {
            id: addBtn
            y: stack.height + 4
            width: stack.width
            height: host.addRowH
            hoverEnabled: true
            focusPolicy: Qt.TabFocus
            Accessible.name: "Add widget"
            readonly property bool lit: addBtn.hovered || addBtn.visualFocus
            background: Rectangle {
                radius: 6
                color: addBtn.lit ? Th.widgetAddBgHover : "transparent"
            }
            contentItem: Item {
                Row {
                    anchors.centerIn: parent
                    spacing: 4
                    ThIcon {
                        anchors.verticalCenter: parent.verticalCenter
                        width: 12; height: 12
                        app: host.app
                        name: "list-add-symbolic"
                        color: addBtn.lit ? Th.widgetAddFgHover : Th.widgetAddFg
                    }
                    WidgetText {
                        anchors.verticalCenter: parent.verticalCenter
                        role: "meta"; semibold: true
                        text: "Add widget"
                        color: addBtn.lit ? Th.widgetAddFgHover : Th.widgetAddFg
                    }
                }
            }
            onClicked: gallery.open()
        }

        // the drop line while reordering: 2 px `widget.drop.edge` between widgets
        Rectangle {
            visible: host.dragFrom >= 0 && host.dragTo >= 0 && host.dragTo !== host.dragFrom
            width: stack.width; height: 2; radius: 1
            color: Th.widgetDropEdge
            z: 20
            y: host.dropLineY
        }
    }

    /// The slot a dragged widget's centre (stack coordinates) lands in, and where the drop line goes (the web's rule:
    /// the first other widget whose middle is below the centre; the line in the gap above it, or under the last).
    function dropAt(cy) {
        var others = []
        for (var i = 0; i < host.items.length; ++i) {
            if (i === host.dragFrom) continue
            var fr = frames.itemAt(i)
            if (fr) others.push({ i: i, top: fr.y, h: fr.height })
        }
        var idx = others.length
        for (var k = 0; k < others.length; ++k) if (cy < others[k].top + others[k].h / 2) { idx = k; break }
        host.dragTo = idx
        host.dropLineY = idx < others.length ? others[idx].top - host.gap / 2 - 1
            : (others.length > 0 ? others[others.length - 1].top + others[others.length - 1].h : 0) + host.gap / 2 - 1
    }

    WidgetGallery {
        id: gallery
        app: host.app
        hostRef: host
    }
    WidgetSettingsDialog {
        id: settingsDialog
        app: host.app
        hostRef: host
    }
}
