// ActionsHost.qml — the desktop's actions runtime (ACTIONS.md §14; the web's is src/features/actions + src/actions/run.ts,
// the same rules):
//
// * snapshotJson — the app's real state as the actions engine reads it (§4.3, §14.2): the mirrored root state (m*),
//   the rooms you joined (basic values for all, the full set for the room on screen and the call's room), spaces, you,
//   themes, sounds, devices and audio sources, `current`. A binding: it re-evaluates when something it reads changes,
//   and a ButtonRow re-resolves only when the TEXT changed (string compare) — nothing repaints for a no-op.
// * run(id, inv) — the one run path for every surface (a row button, the Ctrl+K palette, a host): present the action
//   (ActionsController.present) — unavailable → nothing runs; it picks → the palette's stage 2 when the palette can list
//   its choices, else the command with no subject opens the app's own picker; the table's confirm → the dialog below;
//   then the command.
// * exec(id, ctx) — the id → command map: every placeable desktop action calls what the app already has (the call
//   controller's toggles, the windows, the soundboard, the menus). Where the app's own path shows a confirm the command
//   is the step after it (the run path already asked). Context-only actions run through the surface that provides their
//   context (inv.handler).
//
// Main.qml instantiates one and hands it the ids of the surfaces it opens (`ui`). Colours: ConfirmDialog's (Th).
import QtQuick
import dev.tinhat.app

Item {
    id: host
    required property var app          // Main.qml root: controllers (callctl, room, login …), mirrors (m*), functions
    property var ui: ({})              // Main.qml ids: appMenu, settingsSheet, devicesMenu, callIsland, quickSwitcher …
    visible: false
    width: 0; height: 0

    // ---- the snapshot (§4.3) -------------------------------------------------------------------------------------
    readonly property string currentRoomId: (app && app.roomShown !== false && app.room) ? String(app.room.roomId || "") : ""
    readonly property string snapshotJson: JSON.stringify(_snapshot())

    function _roomValues(r, full) {
        var id = r.id
        var v = {
            "room.joined": r.membership === undefined || r.membership === "join",
            "room.invited": r.membership === "invite",
            "room.current": id === host.currentRoomId,
            "room.isDm": r.dm === true,
            "room.favourite": r.favourite === true,
            "room.hasUnread": (r.unread || 0) > 0,
            "room.callLive": r.call === true,
            "room.inCall": app.mInCall === true && app.mCallRoomId === id,
            // 12.0.2 Pad: one of my other devices is in this room's call and this device is not in (or joining) it — the
            // rooms where the Pad button shows beside Join (Main.qml padTargetIn; the web: providers.ts, the same rule)
            "room.padTarget": (r.membership === undefined || r.membership === "join")
                && !((app.mInCall === true || app.mConnecting === true) && app.mCallRoomId === id) && app.padTargetIn(id) !== null
        }
        if (r.notifyMode) v["room.notifyMode"] = String(r.notifyMode)
        // the call's room (decision 1: Rec / Clip act on it): its recording marker (RoomController.callRecordOn)
        if (full && id !== host.currentRoomId && app.mInCall === true && id === app.mCallRoomId && app.room.callRecordRoom === id)
            v["room.recordOn"] = app.room.callRecordOn === true
        if (full && id === host.currentRoomId) {
            // the room on screen: the RoomController's direct values and the mirrors
            v["room.canChangeAvatar"] = app.room.canChangePhoto === true
            v["room.canChangeName"] = app.mCanChangeName === true
            v["room.canChangeTopic"] = app.mCanChangeTopic === true
            v["room.guestLinkOn"] = false   // retired: the guest room (Settings → Guests) replaced the per-room link
            v["room.recordOn"] = app.room.recordOn === true
            var pinned = app.conversationView ? (app.conversationView.pinnedCount || 0) : 0
            v["room.hasPinned"] = pinned > 0
            v["room.pinnedCount"] = String(pinned)
            var act = app.mActivity
            v["room.hasActivity"] = !!(act && act.game)
            v["room.hasJoinCode"] = !!(act && act.code)
            if (act && act.game) v["room.activity"] = String(act.game)
            v["room.myStatus"] = String(app.mMyStatus || "").length > 0
        }
        return v
    }

    function _snapshot() {
        if (!app) return ({})
        var sp = ui.streamPopover
        var sb = ui.soundboardPanel
        var qi = sp ? sp.presetIndex : -1
        var values = {
            "app.dev": Qt.application.arguments.indexOf("--tinhat-dev") >= 0,
            "app.portable": app.mPortableRun === true,
            "platform.windows": Qt.platform.os === "windows",
            "platform.screenCapture": true,
            "session.busy": app.mBusy === true,
            "ui.roomOpen": host.currentRoomId.length > 0,
            "ui.phone": false,
            "ui.navCorner": app.navMode === "corner",
            "ui.callShown": app.dockView ? app.dockView.callShown === true : false,
            "ui.callMaximized": app.callMaximized === true,
            "ui.callCollapsed": app.callCollapsed === true,
            "ui.rightPanelOpen": !!(ui.rightPanel && ui.rightPanel.shown),
            "ui.rightPanelTab": (ui.rightPanel && ui.rightPanel.shown) ? (ui.rightPanel.tab === 0 ? "info" : "members") : "none",
            "ui.selfPip": app.selfPip === true,
            "ui.reelOpen": !!(ui.gameReelWindow && ui.gameReelWindow.visible),
            "ui.calendarOpen": !!(ui.calendarWindow && ui.calendarWindow.visible),
            "ui.soundboardOpen": !!(sb && (sb.visible || String(sb.playingUrl || "").length > 0)),
            "ui.healthOpen": !!(ui.callIsland && ui.callIsland.statsPopup && ui.callIsland.statsPopup.visible),
            "crypto.unverified": app.mLoggedIn === true && app.mVerified !== true,
            "crypto.backup": !!(app.login && app.login.backupReady),
            "update.state": (app.updater && app.updater.checking) ? "checking" : (app.mUpdateReady ? "ready" : ((app.mUpdateAvailable || app.mUpdateDownloading) ? "available" : "none")),
            "gif.available": app.gifCtl !== null && app.gifCtl !== undefined,
            "overlay.available": app.mOverlayUnavailable !== true,
            "call.in": app.mInCall === true,
            "call.connecting": app.mConnecting === true || app.mReconnecting === true,
            "call.hangingUp": app.mHangingUp === true,
            "call.aside": app.mAside === true,
            "call.micMuted": app.mMicMuted === true,
            "call.deafened": app.mDeafened === true,
            "call.cameraOn": app.mCameraOn === true,
            "call.sharing": app.mCallSharing === true,
            "call.shareRecOnly": app.mShareRecOnly === true,
            "call.musicPlaying": app.mMusicPlaying === true,
            "call.gameChat": app.mGameChat === true,
            "call.afk": app.mLocalAfk === true,
            "call.recActive": app.recActive === true,
            "call.soundPlaying": app.mSoundCallPlaying === true || !!(sb && String(sb.playingUrl || "").length > 0),
            "call.hiddenTiles": (app.hiddenTileCount || 0) > 0,
            "call.pinned": String(app.pinnedTile || "").length > 0,
            "call.hasSavedAudioSource": String(app.mMusicSourceName || "").length > 0,
            "prefs.agc": app.mMicAgc === true,
            "prefs.echoCancel": app.mMicAec === true,
            "prefs.nsMode": String(app.mMicNsMode || "standard"),
            "prefs.pushToTalk": !!(app.callctl && app.callctl.pushToTalk),
            "prefs.callLayout": String(app.callLayout || "auto"),
            "prefs.callAnimate": app.callAnimate === true,
            "prefs.callUnfocusedSaver": app.mCallUnfocusedSaver === true,
            "prefs.callBarPinned": app.callBarPinned === true,
            "prefs.desktopNotifications": !!(app.settings && app.settings.desktopNotifications),
            "prefs.recordOptOut": !!(app.settings && app.settings.recordOptout),
            "prefs.reduceMotion": app.mReducedMotion === true,
            "prefs.gifAutoplay": Motion.gifAutoplay === true,
            "prefs.logship": app.mLogship === 1,
            "prefs.overlay": app.mOverlayEnabled === true,
            "prefs.shareQuality": qi >= 0 && qi <= 2 ? ["smooth", "sharp", "max"][qi] : "custom",
            "prefs.theme": String(app.themeName || ""),
            "presence.known": app.mPresenceKnown === true,
            "presence.sharing": app.mPresenceSharing === true,
            "presence.busy": app.mPresenceBusy === true,
            "presence.hasStatus": !!app.mMyPresenceStatus,
            "push.available": false,
            "push.enabled": false,
            "calendar.hasContainer": (app.mCalContainers || []).length > 0,
            "calendar.reminders": app.mCalReminders === true,
            "watchParty.in": app.mWatchInParty === true,
            // 11.4 (Media Party): with a host set only the host drives; no host = open floor
            "watchParty.canControl": app.mWatchInParty === true
                && (String((app.mWatchParty && app.mWatchParty.hostId) || "").length === 0 || String(app.mWatchParty.hostId) === String(app.mUserId || "")),
            "watchParty.isHost": app.mWatchInParty === true
                && String((app.mWatchParty && app.mWatchParty.hostId) || "").length > 0 && String(app.mWatchParty.hostId) === String(app.mUserId || "")
        }
        if (sb && String(sb.playingName || "").length > 0) values["call.soundName"] = String(sb.playingName)
        // selfhost (server_offer.rs; the web's predicateValues): a gated action is hidden while the server says no, and
        // the retired ones (the per-room guest link) always
        if (app.serverPredicates) { var sv = app.serverPredicates(); for (var sk in sv) values[sk] = sv[sk] }
        // rooms (every joined / invited room; the full set for the room on screen and the call's room)
        var rooms = {}, order = []
        var list = app.mRooms || []
        for (var i = 0; i < list.length; i++) {
            var r = list[i]
            if (r.space === true) continue
            order.push(r.id)
            rooms[r.id] = { name: String(r.name || r.id), values: host._roomValues(r, true) }
            if (r.alias) rooms[r.id].alias = String(r.alias)
        }
        // spaces, you
        var spaces = {}, spaceOrder = []
        var navSpaces = app.mNavSpaces || []
        for (var j = 0; j < navSpaces.length; j++) {
            var s = navSpaces[j]
            if (s.kind !== "space") continue
            spaces[s.key] = { name: String(s.label || s.key), values: { "space.joined": s.membership === "join", "space.invited": s.membership === "invite" } }
            spaceOrder.push(s.key)
        }
        var person = {}
        if (app.mUserId) person[app.mUserId] = { values: { "person.me": true } }
        // sounds: favourites, the open room's, recent
        var sounds = {}, soundOrder = []
        var playing = sb ? String(sb.playingUrl || "") : ""
        function addSound(url, name) {
            if (!url || sounds[url] !== undefined) return
            sounds[url] = { name: String(name || url), values: { "sound.playing": playing === url } }
            soundOrder.push(url)
        }
        var ps = app.mPersonalSounds || [], rs = app.mRoomSounds || [], rec = app.mSbRecent || []
        for (var a = 0; a < ps.length; a++) addSound(ps[a].sourceUrl, ps[a].name)
        for (var b = 0; b < rec.length; b++) addSound(rec[b].url, rec[b].name)
        for (var c = 0; c < rs.length; c++) addSound(rs[c].sourceUrl, rs[c].name)
        // devices and audio sources (device-local; the palette lists them per picker)
        var devices = {}, inputs = [], outputs = [], cameras = []
        function addDev(d, into) { if (!d || !d.node) return; devices[d.node] = { name: String(d.label || d.node), values: {} }; into.push(d.node) }
        var mi = app.mInputs || [], mo = app.mOutputs || [], mc = app.mCameras || []
        for (var e = 0; e < mi.length; e++) addDev(mi[e], inputs)
        for (var f = 0; f < mo.length; f++) addDev(mo[f], outputs)
        for (var g = 0; g < mc.length; g++) addDev(mc[g], cameras)
        var sources = {}, sourceOrder = []
        var as = app.mAudioSources || [], aa = app.mAudioApps || []
        for (var h = 0; h < aa.length; h++) { sources[aa[h].id] = { name: String(aa[h].label || aa[h].id), values: {} }; sourceOrder.push(aa[h].id) }
        for (var k = 0; k < as.length; k++) { sources[as[k].node] = { name: String(as[k].label || as[k].node), values: {} }; sourceOrder.push(as[k].node) }
        // themes: the built-ins, the library, your own (the gallery's list; re-read when the theme state changes)
        var themes = {}, themeOrder = []
        var tl = host._themeList
        for (var t = 0; t < tl.length; t++) { if (themes[tl[t].id] === undefined) { themes[tl[t].id] = { name: String(tl[t].name || tl[t].id), values: {} }; themeOrder.push(tl[t].id) } }
        var active = String(app.mNavActiveSpace || "")
        return {
            values: values,
            current: host.currentRoomId.length > 0 ? { room: host.currentRoomId, activeSpace: active.indexOf("!") === 0 ? active : undefined }
                                                   : (active.indexOf("!") === 0 ? { activeSpace: active } : {}),
            subjects: { room: rooms, space: spaces, person: person, sound: sounds, device: devices, audioSource: sources, theme: themes },
            lists: { room: order, space: spaceOrder, sound: soundOrder, inputs: inputs, outputs: outputs, cameras: cameras,
                     audioSources: sourceOrder, theme: themeOrder }
        }
    }
    // the theme list (an invokable of the theme controller — not reactive: re-read on its state revision)
    property var _themeList: []
    function _readThemes() {
        var tc = ui.themeCtl
        if (!tc) { _themeList = [{ id: "oled", name: "OLED" }, { id: "goose", name: "Goose" }]; return }
        try {
            var g = JSON.parse(tc.galleryJson("", "all"))
            var out = [], seen = {}
            var groups = [g.favourites || [], g.mine || [], g.all || []]
            for (var i = 0; i < groups.length; i++) for (var j = 0; j < groups[i].length; j++) {
                var x = groups[i][j]
                if (!seen[x.id]) { seen[x.id] = true; out.push({ id: x.id, name: x.name }) }
            }
            _themeList = out
        } catch (e) { _themeList = [{ id: "oled", name: "OLED" }, { id: "goose", name: "Goose" }] }
    }
    Connections {
        target: host.ui.themeCtl || null
        ignoreUnknownSignals: true
        function onStateRevChanged() { host._readThemes() }
    }
    Component.onCompleted: { _readThemes(); _refreshCallRecord() }

    // ---- Product decisions (2026-09-27) — one constant each; the web's are
    // src/actions/decisions.ts (REC_CLIP_ROOM, DIM_CALL_CONTROLS_WHILE_CONNECTING, PALETTE_PEOPLE). Flip both apps
    // together (the parity rule; ACTIONS.md §15).
    /** (1) Rec and Clip act on the CALL's room ("call"), even while you look at another room; "screen" = the room on
     *  screen (what the desktop did before 11.0). */
    readonly property string recClipRoom: "call"
    /** (2) The call controls dim while the call is still connecting (true). false = they stay usable: the call's own
     *  rows read "connecting" as "in the call" (callRowSnapshotJson). */
    readonly property bool dimCallControlsWhileConnecting: true
    /** (3) Ctrl+K lists the people you have DMs with ("dmPartners"); "roomMembers" also lists the members of the room
     *  on screen (QuickSwitcher.qml builds the list). */
    readonly property string palettePeople: "dmPartners"

    /** The room Rec and Clip act on — decision (1). */
    readonly property string recClipRoomId: (host.recClipRoom === "call" && app && String(app.mCallRoomId || "").length > 0)
                                            ? String(app.mCallRoomId) : host.currentRoomId
    /** The recording marker of that room (the open room's, else the call room's). */
    readonly property bool recClipRoomRecordOn: (app && app.room) ? (host.recClipRoomId === String(app.room.roomId || "")
                                                ? app.room.recordOn === true : (app.room.callRecordOn === true && app.room.callRecordRoom === host.recClipRoomId)) : false
    function _refreshCallRecord() {
        if (!app || !app.room || !app.room.refreshCallRecord) return
        var r = String(app.mCallRoomId || "")
        if (String(app.room.callRecordRoom || "") !== r) app.room.refreshCallRecord(r)
    }
    Connections {
        target: host.app
        ignoreUnknownSignals: true
        function onMCallRoomIdChanged() { host._refreshCallRecord() }
    }
    /** The snapshot the call's own rows (the bar) are evaluated against — decision (2). The screenshot mode's fake call
     *  (TINHAT_GUI_FAKECALL: the call island with placeholder tiles, no real call) is "in a call" for these rows. */
    readonly property string callRowSnapshotJson: {
        var fake = !!app && app.fakeCall === true
        if (host.dimCallControlsWhileConnecting && !fake) return host.snapshotJson
        var s = JSON.parse(host.snapshotJson)
        if (!s.values) return host.snapshotJson
        if (!fake && (s.values["call.connecting"] !== true || s.values["call.in"] === true)) return host.snapshotJson
        s.values["call.in"] = true
        return JSON.stringify(s)
    }

    // ---- keys (ACTIONS.md §8): this device's own keys (pref `shortcuts`, never synced), the bindings in effect ----------
    // The default Shortcut items of Main.qml take `keysFor(<their action>)` (the keys that are still theirs); a person's
    // key for any other action gets a Shortcut here (the Repeater below); push to talk reads pttKeys.
    // TINHAT_SHORTCUTS pins the document for the review captures: shown, never saved (actionsCtl.envShortcuts)
    readonly property string envShortcuts: (app && app.actionsCtl) ? String(app.actionsCtl.envShortcuts() || "") : ""
    property string shortcutsDocText: envShortcuts.length > 0 ? envShortcuts : ((app && app.settings) ? String(app.settings.pref("shortcuts") || "") : "")
    readonly property var effectiveKeys: {
        var doc = null
        try { doc = host.shortcutsDocText.length > 0 ? JSON.parse(host.shortcutsDocText) : null } catch (e) { doc = null }
        var input = { app: "desktop" }
        if (doc !== null) input.doc = doc
        try { return JSON.parse(String(app.actionsCtl.call("effective", JSON.stringify(input)))) } catch (e2) { return ({ bindings: [], conflicts: [] }) }
    }
    /** The keys that belong to an action right now (its own, or its defaults; minus any another action took). */
    function keysFor(id) {
        var out = [], bs = host.effectiveKeys.bindings || []
        for (var i = 0; i < bs.length; i++) if (bs[i].action === id && bs[i].wins && out.indexOf(bs[i].key) < 0) out.push(bs[i].key)
        return out
    }
    /** Every key the action has (the person's, else its defaults), in order. */
    function allKeysFor(id) {
        var out = [], bs = host.effectiveKeys.bindings || []
        for (var i = 0; i < bs.length; i++) if (bs[i].action === id && out.indexOf(bs[i].key) < 0) out.push(bs[i].key)
        return out
    }
    readonly property var pttKeys: host.keysFor("call.pushToTalk.hold")
    /** Push to talk's key as a Qt key code test (a modifier held alone, or an F-key). */
    function isPttKey(qtKey) {
        var ks = host.pttKeys
        for (var i = 0; i < ks.length; i++) {
            var k = ks[i]
            if ((k === "Ctrl" && qtKey === Qt.Key_Control) || (k === "Alt" && qtKey === Qt.Key_Alt) || (k === "Shift" && qtKey === Qt.Key_Shift)
                || (k === "Meta" && qtKey === Qt.Key_Meta)) return true
            if (k.length >= 2 && k.charAt(0) === "F" && qtKey === Qt.Key_F1 + parseInt(k.substring(1)) - 1) return true
        }
        return false
    }
    /** The actions with a Shortcut item of their own in Main.qml (their keys come from keysFor). */
    readonly property var ownShortcutActions: ["app.settings.open", "call.fill.toggle", "nav.quickSwitcher.open", "call.mic.toggle",
        "call.deafen.toggle", "soundboard.open", "calendar.open", "space.settings.open", "nav.room.previous", "nav.room.next",
        "nav.unread.previous", "nav.unread.next", "layout.edit"]
    /** A person's keys for every other action (they get a Shortcut below). */
    readonly property var userShortcuts: {
        var out = [], bs = host.effectiveKeys.bindings || []
        for (var i = 0; i < bs.length; i++) {
            var b = bs[i]
            if (b.source === "user" && b.wins && !b.hold && host.ownShortcutActions.indexOf(b.action) < 0) out.push({ action: b.action, key: b.key, scope: b.scope })
        }
        return out
    }
    /** Set an action's keys (null = back to its defaults, [] = no key) — §8.8's edit, stored or removed. */
    function setActionKeys(id, keys) {
        var doc = null
        try { doc = host.shortcutsDocText.length > 0 ? JSON.parse(host.shortcutsDocText) : null } catch (e) { doc = null }
        var input = { app: "desktop", action: id, keys: keys === undefined ? null : keys }
        if (doc !== null) input.doc = doc
        var r = JSON.parse(String(app.actionsCtl.call("setBinding", JSON.stringify(input))))
        var text = r.doc ? JSON.stringify(r.doc) : ""
        if (host.envShortcuts.length === 0) app.settings.applyPref("shortcuts", text)
        host.shortcutsDocText = text
    }
    function resetAllShortcuts() { if (host.envShortcuts.length === 0) app.settings.applyPref("shortcuts", ""); host.shortcutsDocText = "" }
    /** Settings → Shortcuts' list (§8.8). */
    function shortcutsView() {
        var doc = null
        try { doc = host.shortcutsDocText.length > 0 ? JSON.parse(host.shortcutsDocText) : null } catch (e) { doc = null }
        var input = { app: "desktop" }
        if (doc !== null) input.doc = doc
        try { return JSON.parse(String(app.actionsCtl.call("shortcuts", JSON.stringify(input)))) } catch (e2) { return ({ groups: [], conflicts: 0 }) }
    }
    /** The actions this device gave keys of its own (the document's entries; the help dialog shows theirs). */
    readonly property var changedActions: {
        try {
            var d = host.shortcutsDocText.length > 0 ? JSON.parse(host.shortcutsDocText) : null
            return d && d.bindings && typeof d.bindings === "object" ? Object.keys(d.bindings) : []
        } catch (e) { return [] }
    }
    /** The help dialog's key text of an action: its own keys once this device changed them, else `def` (today's text). */
    function helpKey(id, def) {
        if (host.changedActions.indexOf(id) < 0) return def
        var ks = host.allKeysFor(id)
        return ks.length > 0 ? ks.join(" / ") : "no key"
    }
    Repeater {
        model: host.userShortcuts
        delegate: Item {
            required property var modelData
            Shortcut {
                sequence: modelData.key
                // ACTIONS.md §8.7: scope `main` = the main window's, `app` = every Tin Hat window
                context: modelData.scope === "main" ? Qt.WindowShortcut : Qt.ApplicationShortcut
                enabled: !!host.app && host.app.mLoggedIn === true
                onActivated: host.run(modelData.action, { source: "key" })
            }
        }
    }

    // ---- the row editor (11.0 buttons: "Edit buttons…" on an editable row, Settings → Appearance → Buttons, a button
    // island's menu) — one at a time; RowEditor.qml (the web's RowEditor.tsx)
    function openRowEditor(req) {
        if (!req || !req.island || !req.row) return
        rowEditor.openFor(req)
    }
    RowEditor {
        id: rowEditor
        app: host.app
    }
    readonly property alias rowEditorDialog: rowEditor   // Settings → Buttons hands off to it (Main.qml settingsHandOff)
    // GUI-verification hooks (review captures, like Main.qml's --tinhat-*): --tinhat-rowedit=<island>.<row>[:<action>]
    // opens the row editor (and picks that action's target) 6 s after start; --tinhat-keyrec=<action> opens Settings →
    // Shortcuts and the key recorder for that action.
    function _hookArg(name) {
        var a = Qt.application.arguments
        for (var i = 0; i < a.length; i++) if (a[i].indexOf(name + "=") === 0) return a[i].substring(name.length + 1)
        return ""
    }
    Timer {
        running: host._hookArg("--tinhat-rowedit").length > 0 || host._hookArg("--tinhat-keyrec").length > 0
        interval: 6000
        onTriggered: {
            var re = host._hookArg("--tinhat-rowedit")
            if (re.length > 0) {
                // <island>.<row>[:<action>] — the island may hold a ":" (buttons:b1), the action dots (app.theme.set)
                var dot = re.indexOf("."), isl = re.substring(0, dot), rest = re.substring(dot + 1)
                var k = rest.indexOf(":"), row = k < 0 ? rest : rest.substring(0, k), pick = k < 0 ? "" : rest.substring(k + 1)
                var spec = isl + "." + row
                var titles = { "header.main": "Room header", "composer.start": "Message box — left", "composer.end": "Message box — right",
                               "account.main": "Your account card", "call.bar": "Call controls" }
                var title = titles[isl + "." + row] || (app.layoutStore ? app.layoutStore.islandName(isl) : "Buttons")
                rowEditor.openFor({ island: isl, row: row, form: "desktop", title: title })
                if (pick.length > 0) {
                    var acts = app.actionsTable ? app.actionsTable.actions : [], plabel = pick
                    for (var q = 0; q < acts.length; q++) if (acts[q].id === pick) plabel = acts[q].label
                    rowEditor.pickOrAdd({ a: pick, label: plabel, pick: true })
                }
                console.log("[buttons] gui hook: row editor " + spec + (pick.length > 0 ? " pick " + pick : ""))
            }
            var kr = host._hookArg("--tinhat-keyrec")
            if (kr.length > 0 && ui.settingsSheet) {
                ui.settingsSheet.openAt("shortcuts")
                host._keyRecFor = kr
                keyRecT.start()
            }
        }
    }
    property string _keyRecFor: ""
    signal keyRecRequested(string action)
    Timer { id: keyRecT; interval: 1500; onTriggered: { host.keyRecRequested(host._keyRecFor); console.log("[buttons] gui hook: key recorder " + host._keyRecFor) } }

    // ---- the run path (§14.1) ----------------------------------------------------------------------------------------
    property var _pending: null
    function run(id, inv) {
        if (!app || !app.actionsCtl) return "unavailable"
        inv = inv || ({ source: "host" })
        var snap = host.snapshotJson
        var p = JSON.parse(app.actionsCtl.present(id, snap, inv.target ? JSON.stringify(inv.target) : "",
                                                  inv.context ? JSON.stringify(inv.context) : "", inv.source === "palette"))
        if (p.error || p.visible !== true || p.enabled !== true) return "unavailable"
        if (p.pick === true && ui.quickSwitcher) {
            var ch = JSON.parse(app.actionsCtl.call("choices", JSON.stringify({ app: "desktop", snapshot: JSON.parse(snap), action: id })))
            if (ch.items !== null && ch.items !== undefined) { ui.quickSwitcher.openAt(id); return "picked" }
        }
        var ctx = { id: id, subject: p.subject, params: p.params || ({}), target: inv.target || null, context: inv.context || null,
                    anchor: inv.anchor || null, source: inv.source || "host", hold: inv.hold || "" }
        if (p.confirm) {
            host._pending = { ctx: ctx, handler: inv.handler || null }
            confirmDlg.dialogTitle = p.confirm.title
            confirmDlg.message = p.confirm.text
            confirmDlg.confirmText = p.confirm.confirmLabel
            confirmDlg.destructive = p.confirm.kind === "destructive"
            confirmDlg.open()
            return "confirming"
        }
        return host._exec(ctx, inv.handler || null)
    }
    function _exec(ctx, handler) {
        try {
            if (handler) handler(ctx)
            else if (!host.exec(ctx.id, ctx)) { console.warn("[actions] no command for", ctx.id); return "unhandled" }
            return "ran"
        } catch (e) { console.warn("[actions]", ctx.id, "failed:", e); return "failed" }
    }
    ConfirmDialog {
        id: confirmDlg
        app: host.app
        onConfirmed: { var p = host._pending; host._pending = null; if (p) host._exec(p.ctx, p.handler) }
        onCancelled: host._pending = null
    }

    // ---- the id → command map (§14.1) ---------------------------------------------------------------------------------
    function _param(ctx, k) { var v = ctx.params ? ctx.params[k] : undefined; return typeof v === "string" ? v : "" }
    function _room(ctx) { return ctx.subject ? ctx.subject : host.currentRoomId }
    function _nameOf(kind, id) {
        try { var s = JSON.parse(host.snapshotJson).subjects[kind][id]; if (s && s.name) return s.name } catch (e) {}
        return id
    }
    function _roomRow(id) { var l = app.mRooms || []; for (var i = 0; i < l.length; i++) if (l[i].id === id) return l[i]; return null }
    readonly property var _sharePresets: ({ smooth: 0, sharp: 1, max: 2 })

    /** Run the command of an action; false when the id has none here. */
    function exec(id, ctx) {
        var r
        switch (id) {
        // ---- app ----
        case "app.about": ui.settingsSheet.openAt("about"); return true   // Settings → About & help (SETTINGS-WINDOW.md §5)
        case "layout.edit": app.openEditLayout(false); return true   // 11.0 edit mode: open — or, editing, Done (EDITMODE-UI §1)
        case "app.bugReport": if (!ui.bugReportDialog.keepNote) ui.bugReportDialog.clearForm(); ui.bugReportDialog.open(); return true
        case "app.iconGallery": ui.iconGallery.openOrRaise(); return true
        case "app.install": ui.installDialog.open(); return true
        case "app.logship.toggle": { var on = app.mLogship !== 1; app.settings.applyLogship(on); app.mLogship = on ? 1 : 0; return true }
        case "app.menu.open":
            if (ctx.anchor) ui.appMenu.popup(ctx.anchor, 0, ctx.anchor.height); else ui.appMenu.popup()
            return true
        case "app.notifications.toggle": app.settings.applyDesktopNotifications(!app.settings.desktopNotifications); return true
        case "app.recordOptOut.toggle": app.settings.applyRecordOptout(!app.settings.recordOptout); return true
        case "app.reduceMotion.toggle": app.mReducedMotion = !app.mReducedMotion; app.settings.applyUiPref("reduce-motion", app.mReducedMotion ? "1" : "0"); return true
        // 10.8: Settings → Appearance → Motion → Play GIFs automatically (pref gif-autoplay, default on)
        case "app.gifAutoplay.toggle": Motion.gifAutoplay = !Motion.gifAutoplay; app.settings.applyUiPref("gif-autoplay", Motion.gifAutoplay ? "1" : "0"); return true
        case "app.rooms.refresh": app.login.refreshRooms(); return true
        case "app.settings.open": {
            // a pane key (SETTINGS-WINDOW.md §3; "sessions" = security); the window ignores an unknown one
            var pane = host._param(ctx, "pane")
            if (pane.length > 0) ui.settingsSheet.openAt(pane); else ui.settingsSheet.open()
            return true
        }
        case "app.shortcuts.open": ui.shortcutsDialog.open(); return true
        case "app.sidebar.toggle": app.toggleSidebar(); return true
        // the table asked "Sign out?" (with the no-backup variant) — sign out now
        case "app.signOut": app.login.logout(); return true
        case "app.theme.set": if (ctx.subject) ui.chooseTheme(ctx.subject); return true
        case "app.tour.start": ui.welcomeTour.start(); return true
        case "app.update":
            if (app.mUpdateReady) app.updater.restart()
            else if (app.mUpdateAvailable && (app.mCanSelfUpdate || app.updater.channel === "flatpak")) app.updater.downloadAndApply()
            else if (app.mUpdateAvailable) app.showPassiveNotification("Update: " + app.manualUpdateHint)
            else app.updater.checkForUpdates()
            return true
        case "app.verify": ui.verifySheet.open(); return true
        // ---- calendar ----
        case "calendar.event.new": app.openCalendarNew(); return true
        case "calendar.open":
            if (ctx.subject) app.calCtl.openContainer(ctx.subject)
            ui.calendarWindow.openOrRaise()
            return true
        case "calendar.reminders.toggle": app.calCtl.applyReminders(!app.mCalReminders); return true
        // ---- calls ----
        case "call.afk.toggle": app.afkCtl.setAfk(!app.mLocalAfk); return true
        case "call.agc.toggle": app.callctl.applyMicAgc(!app.mMicAgc); return true
        case "call.audioShare.source": if (ctx.subject) ui.streamPopover.chooseAudio(ctx.subject); return true
        case "call.audioShare.toggle":
            if (app.mMusicPlaying) app.callctl.stopMusic()
            else if (String(app.mMusicSourceName || "").length > 0) app.callctl.startMusicSource(app.mMusicSourceName)
            else ui.streamPopover.open()
            return true
        case "call.camera.set": app.callctl.setCamera(ctx.subject || ""); return true
        case "call.camera.toggle": app.callctl.toggleCamera(); return true
        // decision (1): the call's room (recClipRoomId), also while another room is on screen
        case "call.clip": {
            var cr = host.recClipRoomId
            if (cr.length === 0) return true
            if (cr === String(app.room.roomId || "")) app.room.clipMoment(); else app.room.clipMomentIn(cr)
            app.showPassiveNotification("Clipped — bookmarked on the Memories timeline", "short")
            return true
        }
        case "call.collapse.toggle": app.setCallCollapsed(!app.callCollapsed); return true
        case "call.corner.leave": app.callctl.leaveCorner(); return true
        case "call.deafen.toggle": app.callctl.applyDeafen(!app.mDeafened); return true
        case "call.devices.open": app.refreshDeviceMenus(); ui.devicesMenu.popup(); return true
        case "call.echoCancel.toggle": app.callctl.applyMicAec(!app.mMicAec); return true
        case "call.echoOutputs.open": app.openEcOutputs(); return true
        case "call.fill.toggle": app.callMaximized = !app.callMaximized; return true
        case "call.gameChat.toggle": app.callctl.applyGameChat(!app.mGameChat); return true
        case "call.health.open": if (ui.callIsland && ui.callIsland.statsPopup) ui.callIsland.statsPopup.open(); return true
        case "call.hidden.show": {
            var tile = ctx.context && ctx.context.tile ? ctx.context.tile : ctx.subject
            if (tile) { if (app.isRecOnlyHidden(tile)) app.callctl.setTileTunedIn(tile, true); else app.unhideTile(tile) }
            else if (ui.callIsland && ui.callIsland.hiddenMenu) ui.callIsland.hiddenMenu.popup(ui.callIsland.hiddenChip, 0, ui.callIsland.hiddenChip.height + 4)
            return true
        }
        case "call.hidden.showAll": app.unhideAllTiles(); return true
        case "call.input.set": app.callctl.setInput(ctx.subject || ""); return true
        case "call.join":
            r = host._room(ctx)
            if (r.length > 0) { if (app.mInCall && app.mCallRoomId !== r) app.callctl.switchCall(r); else app.callctl.joinCall(r) }
            return true
        // 12.0.2 Pad (docs/PAD.md): the Pad button beside Join for the target room's call (Main.qml openPad: a greyed Pad
        // shows its one line instead)
        case "call.pad.open":
            r = host._room(ctx)
            if (r.length > 0) app.openPad(r, app.padTargetIn(r))
            return true
        // 11.0.2: "Keep call buttons visible" — the bar's pin and Settings → Calls run the same (AppCore.setCallBarPinned)
        case "call.bar.pin.toggle": app.setCallBarPinned(!app.callBarPinned); return true
        case "call.layout.animate.toggle": app.callAnimate = !app.callAnimate; return true
        case "call.layout.set": { var lv = host._param(ctx, "value"); if (lv.length > 0) app.setCallLayout(lv); return true }
        case "call.leave": app.callctl.leaveCall(); return true
        case "call.mic.toggle":
            if (app.mServerMuted) app.showPassiveNotification(app.serverMutedText(), 3000); else app.callctl.toggleMic()
            return true
        case "call.micCheck": ui.callOptions.toggleFor(ctx.anchor || ui.callIsland || null, "mic"); return true
        case "call.music.file": ui.musicFileDialog.open(); return true
        case "call.music.url": ui.musicUrlField.text = ""; ui.musicUrlDialog.open(); return true
        case "call.noiseSuppression.set": { var ns = host._param(ctx, "value"); if (ns.length > 0) app.callctl.applyMicNsMode(ns); return true }
        case "call.noiseTest.open": ui.noiseTestSheet.open(); return true
        case "call.options.open": { var face = host._param(ctx, "value"); if (face.length > 0) ui.callOptions.toggleFor(ctx.anchor || ui.callIsland || null, face); return true }
        case "call.output.set": app.callctl.setOutput(ctx.subject || ""); return true
        case "call.overlay.toggle": { var ov = !app.mOverlayEnabled; app.settings.applyOverlayEnabled(ov); if (ov) app.ensureOverlay(); return true }
        case "call.pin.clear": app.pinnedTile = ""; return true
        // a hold action: the button's press and release (ActionButton) — never from the palette
        case "call.pushToTalk.hold": app.callctl.pttHold(ctx.hold !== "up"); return true
        case "call.pushToTalk.toggle": app.callctl.applyPushToTalk(!app.callctl.pushToTalk); return true
        case "call.screenShare.toggle": if (app.mCallSharing) app.callctl.stopScreen(); else ui.streamPopover.open(); return true
        case "call.share.panel": if (ui.streamPopover.visible) ui.streamPopover.close(); else ui.streamPopover.open(); return true
        case "call.share.recordingOnly.toggle": app.callctl.applyShareRecordingOnly(!app.mShareRecOnly); return true
        case "call.shareQuality.set": {
            var qi2 = host._sharePresets[host._param(ctx, "value")]
            if (qi2 !== undefined) { var qp = ui.streamPopover.presets[qi2]; app.callctl.applyShareQuality(qp.w, qp.h, qp.fps, qp.kbps) }
            return true
        }
        case "call.testConnection": app.callctl.runConnectionTest(); return true
        // 10.8.1: Settings → Appearance → Performance — the master switch of every unfocused rule, default OFF (AppCore
        // reads it through lowgpu.unfocusedSaverOn; the switch writes "1" / "0")
        case "call.unfocusedSaver.toggle": { var sv = !app.mCallUnfocusedSaver; app.mCallUnfocusedSaver = sv; app.settings.applyUiPref("call-unfocused-saver", sv ? "1" : "0"); return true }
        // ---- navigation ----
        case "conversation.jumpToLatest": if (ui.conversation && ui.conversation.jumpToLatest) ui.conversation.jumpToLatest(); return true
        case "nav.quickSwitcher.open": if (app.mLoggedIn) ui.quickSwitcher.open(); return true
        case "nav.room.next": app.stepRoom(1, false); return true
        case "nav.room.previous": app.stepRoom(-1, false); return true
        case "nav.room.open": if (ctx.subject) app.openRoomId(ctx.subject); return true
        case "nav.roomList.filter": if (ui.roomsIsl && ui.roomsIsl.filterField) ui.roomsIsl.filterField.forceActiveFocus(); return true
        case "nav.space.select": { var key = host._param(ctx, "rail") || ctx.subject || ""; if (key.length > 0) app.nav.selectSpace(key); return true }
        case "nav.unread.next": app.stepRoom(1, true); return true
        case "nav.unread.previous": app.stepRoom(-1, true); return true
        // ---- rooms ----
        case "dm.new": ui.newDmField.text = ""; ui.newDmDialog.open(); return true
        case "person.message": if (ctx.subject) app.login.createDm(ctx.subject); return true
        case "room.create":
            ui.newRoomName.text = ""; ui.newRoomTopic.text = ""; ui.newRoomPrivate.checked = true; ui.newRoomIsSpace.checked = false
            ui.newRoomDialog.spaceId = String(app.mNavActiveSpace || "").indexOf("!") === 0 ? app.mNavActiveSpace : ""
            ui.newRoomDialog.spaceName = app.mNavActiveLabel
            ui.newRoomInSpace.checked = true
            ui.newRoomDialog.open()
            return true
        case "room.favourite.toggle": {
            r = host._room(ctx); var row = host._roomRow(r)
            if (r.length > 0) app.login.setFavourite(r, !(row && row.favourite === true))
            return true
        }
        // retired (server_offer.rs RETIRED_ACTIONS, always hidden): the guest room (Settings → Guests) replaced the
        // per-room guest link
        case "room.guestLink":
        case "room.guestLink.disable": return false
        case "room.info.edit": if (ui.rightPanel) { ui.rightPanel.tab = 0; ui.rightPanel.editingInfo = true; ui.rightPanel.open() } return true
        case "room.info.toggle": if (ui.rightPanel) { if (ui.rightPanel.opened) ui.rightPanel.close(); else ui.rightPanel.open() } return true
        case "room.invite":
            r = host._room(ctx)
            app.ctxRoomId = r; app.ctxRoomName = host._nameOf("room", r); ui.inviteField.text = ""; ui.inviteDialog.open()
            return true
        case "room.inviteLink.copy": {
            r = host._room(ctx); var lr = host._roomRow(r)
            app.copyToClipboard(app.roomInviteLink(r, lr ? lr.alias : ""))
            app.showPassiveNotification(lr && lr.publicJoin ? "Invite link copied — anyone with it can join" : "Link copied — they will still need an invite to this private room", "short")
            return true
        }
        // the table asked "Leave room?" for a joined room (an invite is declined without asking)
        case "room.leave":
            r = host._room(ctx)
            if (r.length > 0) { app.login.leaveRoom(r); if (r === host.currentRoomId) app.roomShown = false }
            return true
        case "room.markRead": r = host._room(ctx); if (r.length > 0) app.nav.markRoomRead(r); return true
        case "room.members.show": if (ui.rightPanel) { ui.rightPanel.tab = 1; ui.rightPanel.open() } return true
        case "room.notifications.set": {
            r = host._room(ctx); var nv = host._param(ctx, "value")
            if (r.length > 0 && nv.length > 0) app.login.setRoomNotifyMode(r, nv)
            return true
        }
        case "room.photo.change": ui.roomPhotoDialog.open(); return true
        // the table asked "Remove the room photo?" (11.0: this had no confirm)
        case "room.photo.remove": app.room.applyRoomPhoto(""); return true
        case "room.popOut": {
            r = host._room(ctx)
            if (app.floatingCtl.isOpen(r)) ui.floatingChats.focusBubble(r)
            else if (!app.floatingCtl.openBubble(r)) app.showPassiveNotification("You can have " + app.floatingCtl.maxBubbles + " popped-out chats at once — close one first", "short")
            return true
        }
        // the table asked "Record calls here?" when turning it on — no second confirm
        case "room.recording.toggle": {
            // the resolved room: the open room (a button's absent rule), or the call's room (the call rows' context)
            r = host._room(ctx)
            if (r.length === 0) return true
            if (r === String(app.room.roomId || "")) app.room.setRecordCall(!app.room.recordOn)
            else app.room.setRecordCallIn(r, !(app.room.callRecordRoom === r && app.room.callRecordOn === true))
            return true
        }
        case "room.refresh": app.room.refresh(); return true
        case "room.status.clear": app.statusCtl.postStatus("", 0); return true
        case "room.status.post": {
            var st = host._param(ctx, "text")
            if (st.length > 0) app.statusCtl.postStatus(st, 30)
            else if (ui.rightPanel) { ui.rightPanel.tab = 1; ui.rightPanel.open() }
            return true
        }
        case "space.create":
            ui.newRoomName.text = ""; ui.newRoomTopic.text = ""; ui.newRoomIsSpace.checked = true; ui.newRoomPrivate.checked = true
            ui.newRoomDialog.spaceId = ""; ui.newRoomDialog.spaceName = ""; ui.newRoomDialog.open()
            return true
        case "space.invite":
            if (ctx.subject) { app.ctxRoomId = ctx.subject; app.ctxRoomName = host._nameOf("space", ctx.subject); ui.inviteField.text = ""; ui.inviteDialog.open() }
            return true
        case "space.joinOrCreate": ui.joinOrCreateDialog.open(); return true
        // the table asked "Leave space?"
        case "space.leave": if (ctx.subject) app.login.leaveRoom(ctx.subject); return true
        case "space.link.copy": {
            if (!ctx.subject) return true
            var srow = host._roomRow(ctx.subject)
            app.copyToClipboard(app.roomInviteLink(ctx.subject, srow ? srow.alias : ""))
            app.showPassiveNotification(srow && srow.publicJoin ? "Space link copied — anyone with it can join" : "Space link copied — they will still need an invite", 2500)
            return true
        }
        case "space.markAllRead":
            if (ctx.subject) { app.nav.markSpaceRead(ctx.subject); app.showPassiveNotification("Marking everything in " + host._nameOf("space", ctx.subject) + " as read…", "short") }
            return true
        case "space.settings.open": if (ctx.subject) app.openSpaceSettings(ctx.subject, host._param(ctx, "tab") || "overview"); return true
        // ---- fun ----
        case "memories.open": {
            r = host._room(ctx)
            if (r.length === 0) return true
            ui.memoriesWindow.roomId = r
            ui.memoriesWindow.roomName = host._nameOf("room", r)
            if (r === host.currentRoomId) { app.room.fetchMemories(); app.room.fetchClips() }
            else { app.room.fetchMemoriesFor(r); app.room.fetchClipsFor(r) }
            ui.memoriesWindow.openOrRaise()
            return true
        }
        case "reel.open": ui.gameReelWindow.openOrRaise(); return true
        case "room.activity.clear": app.activityCtl.setActivity("", "", ""); return true
        case "room.activity.open":
            if (ui.callIsland && ui.callIsland.gamePopup) ui.callIsland.gamePopup.open()
            else if (ui.rightPanel) { ui.rightPanel.tab = 0; ui.rightPanel.open() }
            return true
        case "room.activity.set": {
            var game = host._param(ctx, "game")
            if (game.length > 0) app.activityCtl.setActivity(game, "", "")
            else if (ui.rightPanel) { ui.rightPanel.tab = 0; ui.rightPanel.open() }
            return true
        }
        case "sound.play": if (ctx.subject) ui.soundboardPanel.toggleSound(ctx.subject, host._nameOf("sound", ctx.subject), ""); return true
        case "sound.stop": ui.soundboardPanel.stopCurrent(); return true
        case "soundboard.open": app.soundboardRefresh(); ui.soundboardPanel.openOrRaise(); return true
        case "watchParty.leave": app.watchCtl.leaveParty(); return true
        case "watchParty.open": {
            // 11.4 (Media Party): the target room (the call bar passes the call's room; the palette the room on screen)
            var wr = host._room(ctx)
            if (!app.watchCtl.inParty && wr.length > 0) app.watchCtl.joinParty(wr)
            ui.watchPartyWindow.openOrRaise()
            return true
        }
        // ---- you ----
        case "presence.sharing.toggle": app.presenceCtl.chooseSharing(!app.mPresenceSharing); return true
        case "status.clear": app.presenceCtl.clearStatus(); return true
        case "status.open": if (ui.navState && ui.navState.listState && ui.navState.listState.statusPopObj) ui.navState.listState.statusPopObj.open(); return true
        // the table asked "Show your status to friends?" when sharing is off: the last argument turns it on
        case "status.set": {
            var sid = host._param(ctx, "status")
            if (sid === "custom") {
                var text = host._param(ctx, "text")
                if (text.length === 0) return host.exec("status.open", ctx)
                app.presenceCtl.setStatus("custom", text, host._param(ctx, "emoji"), host._param(ctx, "colour"), 0, !app.mPresenceSharing)
                return true
            }
            var presets = app.mStatusPresets || []
            for (var si = 0; si < presets.length; si++) if (presets[si].id === sid) {
                var sp2 = presets[si]
                app.presenceCtl.setStatus(sp2.id, sp2.text, sp2.emoji, sp2.color, Date.now() + 4 * 3600000, !app.mPresenceSharing)
            }
            return true
        }
        }
        return false
    }
}
