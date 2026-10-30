// AppCore — Tin Hat's brain, hoisted out of Main.qml (9.0).
//
// The 22 Rust controllers, the 120 ms mirror loop that copies their getters into plain QML
// properties (see the note at the top of Main.qml for why), the call-grid model with its
// hide / view / watched memory, PiP + overlay window management, chimes, the bootstrap and
// the boot reveal. Nothing here draws anything. Main.qml instantiates ONE AppCore, aliases what it needs, and answers the few signals that
// need a window: notify(), openIconGallery(), focusCalendarEvent(), openCalendarWindow(),
// bootReveal(), booted(), mirrored(). Exactly one AppCore may exist per process — two
// would double every controller and every signal handler.
import QtQuick
import QtQuick.Window
import QtMultimedia
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app

Item {
    id: core
    visible: false
    width: 0; height: 0

    // The window that owns us: PiP / overlay parenting, visibility, and the Legacy `pal`
    // the pop-out windows still take.
    required property var shell
    // Shell-owned facts the loop needs (bound by the shell).
    property bool navOpened: false          // the corner dock is open → full nav recompute is worth it
    property bool memoriesWanted: false     // the Memories window is visible → mirror its lists
    // Out to the shell.
    signal notify(var message, var timeout) // showPassiveNotification(message[, timeout])
    signal notifyAction(var message, var timeout, var actionText, var callback) // themes 1b: a toast with an action
    signal openIconGallery()
    signal focusCalendarEvent(string eventId)
    signal openCalendarWindow()
    signal mirrored()                       // one mirror pass finished (the shell re-checks the tour)
    signal booted()                         // bootstrap done (theme + nav mode restored, restore() issued)
    signal bootReveal(string why)           // show the main window now

    // Controllers, by their Main.qml ids (a shell re-exposes them under the same names).
    readonly property LoginController c_login: login
    readonly property BootController c_bootctl: bootctl
    readonly property RoomController c_room: room
    readonly property StreamController c_stream: stream
    readonly property CallController c_callctl: callctl
    readonly property SettingsController c_settings: settings
    readonly property DiagnosticsController c_diagnostics: diagnostics
    readonly property UpdateController c_updater: updater
    readonly property GameActivityController c_activityCtl: activityCtl
    readonly property UserStatusController c_statusCtl: statusCtl
    readonly property GifController c_gifCtl: gifCtl
    readonly property AudioPrefsController c_audioPrefsCtl: audioPrefsCtl
    readonly property SoundboardController c_soundboardCtl: soundboardCtl
    readonly property SoundboardController c_joinSoundCtl: joinSoundCtl
    readonly property WatchPartyController c_watchCtl: watchCtl
    readonly property NavController c_nav: nav
    readonly property AfkController c_afkCtl: afkCtl
    readonly property FloatingController c_floatingCtl: floatingCtl
    readonly property CalendarController c_calCtl: calCtl
    readonly property IconPackController c_iconPack: iconPack
    readonly property GamesController c_gamesCtl: gamesCtl
    readonly property SpaceController c_spaceCtl: spaceCtl   // RBAC: space settings (docs/RBAC.md §5.3)
    readonly property InviteController c_invitesCtl: invitesCtl   // 10.4: invite codes (docs/INVITES.md)
    readonly property GuestsController c_guestsCtl: guestsCtl   // selfhost: the guest room (docs/GUESTS.md)
    readonly property PresenceController c_presenceCtl: presenceCtl   // 10.6: opt-in presence (docs/REACH.md §2)
    readonly property ThemeController c_themeCtl: themeCtl   // themes (C1): the engine → the Th singleton
    readonly property ThemeEditorController c_editorCtl: editorCtl   // 11.0 step 5: the theme editor, share cards, Pick
    readonly property LowGpuController c_lowgpu: lowgpuCtl   // 11.0 (GPU.md): the low-GPU contract's rules
    readonly property ActionsController c_actionsCtl: actionsCtl   // 11.0 step 3 (buttons): the actions engine (ACTIONS.md)
    readonly property GameServersController c_gameServers: gameServersCtl   // 11.1: the Game servers widget (GAMESERVERS.md)
    readonly property PadController c_padCtl: padCtl   // Pad: the phone as a remote for my call (docs/PAD.md)
    readonly property var c_tileModel: tileModel

    property string themeName: "oled"
    // themes (desktop C1, docs/MIGRATING-THEME.md): the ONE place a theme reaches the UI. ThemeController (the Rust
    // engine, crates/theme) resolves the id — an unknown or refused theme is OLED — and the Th singleton takes every
    // colour; the DEPRECATED palette shim (Th.pal → Main.qml `pal`) follows. Setting themeName (menu, Appearance
    // cards, the bootstrap) is how a theme is switched; the sounds and the KDE scheme still key on the name.
    // 11.0 step 5: while a PREVIEW is on (the theme editor, a share card's Try — THEMES-UI.md §13.4 / §14.4) the effective
    // theme is not painted: the store keeps following it and endPreview() paints it again.
    function applyTheme(id) {
        if (editorCtl.previewOwner.length > 0) return
        var colours = themeCtl.resolve(id)
        core._thColours = colours
        Th.apply(colours, themeCtl.themeId, themeCtl.themeName, themeCtl.variant, themeCtl.names, themeCtl.shape, themeCtl.look)
        if (core.themeName !== themeCtl.themeId) core.themeName = themeCtl.themeId
    }
    // (ENGINE.md §10: a theme's fonts and text size come with Th.apply — Main.qml puts them on the window's font)
    /// the colours Th shows now (the Pick tool puts them back after its probe pass)
    property var _thColours: ({})

    // ---- previews (THEMES-UI.md §13.4, §14.4): a document painted without saving anything — no account write, no My
    // themes write, no boot cache, the theme choice unchanged. One owner at a time ("editor" | "try").
    function previewTheme(owner, docJson) {
        var colours = editorCtl.previewDoc(owner, docJson)
        var any = false
        for (var k in colours) { any = true; break }
        if (!any) return false
        core._thColours = colours
        Th.apply(colours, editorCtl.previewId, editorCtl.previewName, editorCtl.previewVariant, editorCtl.previewNames, editorCtl.previewShape, editorCtl.previewLook)
        return true
    }
    /// End `owner`'s preview: the effective theme is painted again.
    function endPreview(owner) {
        if (editorCtl.endPreview(owner)) core.applyTheme(core.themeName)
    }

    // ---- the theme editor (THEMES-UI.md §13): the session lives in ThemeEditorController; every change of the working
    // document is painted through the preview. Opening it ends a Try (the editor takes the screen). `mineId` "" = a new
    // theme from `docJson`. Returns true when it opened (the caller closes the Settings sheet).
    signal themeEditorOpened()
    function openThemeEditor(docJson, mineId, baseName) {
        var err = editorCtl.openEditor(docJson, mineId || "", baseName || "", false)
        if (err.length > 0) { core.notify(err, "long"); return false }
        if (editorCtl.previewOwner === "try") core.endTrial(false)
        core._paintEditor()
        core.themeEditorOpened()
        return true
    }
    function _paintEditor() {
        var d = editorCtl.workingDoc()
        if (d.length > 0 && !core.previewTheme("editor", d)) console.warn("[theme] editor preview refused")
    }
    Connections {
        target: editorCtl
        function onStateRevChanged() { if (editorCtl.open) core._paintEditor() }
        function onOpenChanged() { if (!editorCtl.open) core.endPreview("editor") }
        function onShared(ok, text) { core.notify(text, ok ? "short" : "long") }
    }

    // ---- Try and Keep (THEMES-UI.md §14.4): a shared theme painted for 30 s (the preview "try"), then back; one trial at a
    // time (Try on another card replaces it — the theme it goes back to stays the one from before the first Try); any
    // real apply (a card, the ☰ menu, Keep, the editor's Save — ThemeController.appliedRev) ends it without going back.
    property var themeTrial: null                   // {key, docJson, name, left}
    function tryTheme(key, docJson, name) {
        if (editorCtl.previewOwner === "editor") return false
        if (!core.previewTheme("try", docJson)) { core.notify("This theme can't be used by this version of Tin Hat.", "long"); return false }
        core.themeTrial = { key: key, docJson: docJson, name: name, left: 30 }
        trialTick.restart()
        return true
    }
    function endTrial(back) {
        if (!core.themeTrial) return
        trialTick.stop()
        core.themeTrial = null
        core.endPreview("try")
    }
    Timer {
        id: trialTick
        interval: 1000
        repeat: true
        // once a second: a changed number is the only thing that moves (GPU.md)
        onTriggered: {
            var t = core.themeTrial
            if (!t) { stop(); return }
            if (t.left <= 1) core.endTrial(true)
            else core.themeTrial = { key: t.key, docJson: t.docJson, name: t.name, left: t.left - 1 }
        }
    }
    /// Keep (§14.4, keepPlan): a library theme as shipped → applied by id; the same as one of My themes → that one; else
    /// saved as a new My theme and applied ("‹name› added to My themes").
    function keepTheme(themeJson) {
        var r = JSON.parse(editorCtl.keep(themeJson))
        if (!r.ok) core.notify(r.text, "long")
        else if (r.toast && r.toast.length > 0) core.notify(r.toast, "short")
        core.endTrial(false)
    }
    onThemeNameChanged: core.applyTheme(core.themeName)
    // themes step 1b (THEMES-SYNC.md, theme_store.rs): a person's theme pick — the ☰ menu, the Appearance cards, the
    // gallery. It goes through the theme store (the account, or this device with its own theme on), whose answer comes
    // back as effectiveKey below. Gate mode (TINHAT_GUI_THEMESYNC=0: the gates' account never gets the sync keys): the
    // 10.6 path — themeName + the legacy file + the KDE scheme (login.setTheme) — and the store notes it for this run.
    // `inSettings`: the pick came from Settings → Appearance (a sound offer shows there, not as a toast).
    function chooseTheme(id, inSettings) {
        if (!themeCtl.syncing) { core.themeName = id; login.setTheme(id); themeCtl.gateTheme(id); return "" }
        return themeCtl.applyTheme(id, inSettings === true)
    }
    // What the store renders changed (a pick here, another device's pick, the device override …): apply it — after the
    // bootstrap, which applies the start theme itself. The key is "<id>|<hash of its own document, 0 = the library's>".
    property bool _themeBooted: false
    property string _themeDoc: "0"
    Connections {
        target: themeCtl
        // a real apply (a card, the ☰ menu, Keep, the editor's Save, Undo) ends a Try without going back (§14.4)
        function onAppliedRevChanged() { core.endTrial(false) }
        function onEffectiveKeyChanged() {
            if (!core._themeBooted) return
            var id = themeCtl.effectiveTheme
            var doc = themeCtl.effectiveKey.split("|")[1] || "0"
            if (core.themeName !== id) { core.themeName = id; login.applyColorScheme(id) }
            else if (doc !== core._themeDoc) { core.applyTheme(id); login.applyColorScheme(id) }   // same id, another document
            core._themeDoc = doc
        }
        // THEMES-UI §7 "Your theme was changed to ‹new› on another device." [Keep ‹old› here] · §10.6 a sound offer
        // made outside the settings: "‹theme› comes with its own sounds. Use ‹pack› sounds too?" [Use ‹pack› sounds]
        function onNotice(kind, text, action, payload) {
            core.notifyAction(text, 8000, action, function () {
                if (kind === "offer") themeCtl.applyPack("sounds", payload)
                else themeCtl.keepHere(payload)
            })
        }
    }
    // THEMES-UI §10.3 / §10.4: the icon style and the sound pack are their own settings now (not the theme's name).
    readonly property string mIconStyle: themeCtl.iconStyle
    readonly property string soundPackDir: themeCtl.soundPack === "goose" ? "goose" : "oled"
    // Themed call join/leave chimes: file://…/assets/sounds/<theme>/{join,leave}.mp3 (assets.rs).
    readonly property string soundsDir: "file://" + login.assetsPath + "/sounds"
    // ---- In-call view state ----
    // Screenshot hook (env TINHAT_GUI_FAKECALL): render the call panel with placeholder
    // tiles so the layout can be captured without joining a live call.
    readonly property bool fakeCall: login.autoFakeCall.length > 0
    // mCallFailed keeps the call panel (and its "Call ended — Rejoin" banner) visible after a
    // terminal disconnect, when mInCall has been cleared — otherwise the failure UI hides itself.
    readonly property bool callShown: mInCall || mConnecting || fakeCall || mCallFailed
    property var fakeTiles: [
        { "id": "vid_selfview", "participant": "You" },
        // vidW/vidH: a natural size for the harness only (real tiles get theirs from the
        // sink caps) — a 4:3 camera in a 16:9-ish tile exercises the auto fill + zoom pill.
        { "id": "fake_morgan", "participant": "@morgan:example.com", "vidW": 640, "vidH": 480 },
        { "id": "fake_sam", "participant": "@sam:example.com", "vidW": 1280, "vidH": 720 },
        // A fake SHARE so the layout gates can exercise the streams-first placements
        // (side column / portrait under-band) without a real capture pipeline.
        { "id": "fake_share", "participant": "@morgan:example.com", "kind": "screenshare", "screenshare": true }
    ]
    readonly property var callTiles: fakeCall ? fakeTiles : mVideoTiles
    // Gate hook: --tinhat-zoomprobe makes every tile log its view state (Qt.application
    // .arguments is a QML list, not a JS Array — indexOf works, .some does not).
    readonly property bool zoomProbe: Qt.application.arguments.indexOf("--tinhat-zoomprobe") >= 0
    // 0.8.28: --tinhat-tilesync (+ TINHAT_GUI_FAKECALL) drives the grid model through
    // join / mute / reorder / leave and asserts the delegates survive (gate-tilesync.sh).
    readonly property bool tileSyncProbe: Qt.application.arguments.indexOf("--tinhat-tilesync") >= 0
    // 0.8.34: --tinhat-noverify skips the automatic "Verify this device" prompt on sign-in — a modal
    // that swallows hover for the whole scene and whose Close button moves with the QQC2 style, so
    // GUI gates could never dismiss it reliably (the 0.8.34 harness). Settings / an incoming SAS still open it.
    readonly property bool noVerifyPrompt: Qt.application.arguments.indexOf("--tinhat-noverify") >= 0
    property int _tileCreated: 0
    property int _tileDestroyed: 0

    // ===================== The call grid's model (0.8.28) =====================
    // The grid Repeater used to run straight on the callTiles JS array. A Repeater treats a
    // new array as a new model: EVERY delegate was destroyed and re-created on every roster
    // change (someone joins, leaves, mutes, toggles a camera, a soundboard track appears) —
    // which tore down and rebuilt every remote video pipeline (black flash + decoder restart;
    // 77 rebuilds in one night on one machine) and re-bound the LIVE self-view to a fresh GL
    // item, which renders black. Now the Repeater runs on this ListModel, synced IN PLACE by
    // tile id: rows are inserted / moved / updated / removed individually, in callTiles
    // order (the layout engine addresses tiles by index), so a delegate lives as long as its
    // tile does. Everything else keeps reading callTiles (the array) as before.
    ListModel { id: tileModel }
    function _tileRow(t) {
        var r = {
            id: (t && t.id) ? String(t.id) : "",
            participant: (t && t.participant) ? String(t.participant) : "",
            kind: (t && t.kind) ? String(t.kind) : "video",
            name: (t && t.name) ? String(t.name) : "",
            screenshare: !!(t && t.screenshare),
            muted: !!(t && t.muted),
            avatar: (t && t.avatar) ? String(t.avatar) : "",
            recordingOnly: !!(t && t.recordingOnly),
            tunedIn: !!(t && t.tunedIn),
            vidW: (t && t.vidW) ? Number(t.vidW) : 0,
            vidH: (t && t.vidH) ? Number(t.vidH) : 0,
            cameraMuted: !!(t && t.cameraMuted)
        }
        r.raw = JSON.stringify(r)
        return r
    }
    function _syncTileModel() {
        var arr = core.callTiles || []
        var n = arr.length
        for (var i = 0; i < n; i++) {
            var row = core._tileRow(arr[i])
            var found = -1
            for (var j = i; j < tileModel.count; j++) {
                if (tileModel.get(j).id === row.id) { found = j; break }
            }
            if (found < 0) {
                tileModel.insert(i, row)
                continue
            }
            if (found !== i) tileModel.move(found, i, 1)
            var cur = tileModel.get(i)
            if (cur.raw !== row.raw) {
                for (var k in row) if (cur[k] !== row[k]) tileModel.setProperty(i, k, row[k])
            }
        }
        while (tileModel.count > n) tileModel.remove(tileModel.count - 1)
    }
    // The self-view pipeline was rebuilt on the Rust side (camera switch / stall restart):
    // that ONE row is dropped and re-inserted so its delegate is re-created and its attach
    // loop binds the new pipeline (the strip/inject bounce alone no longer re-creates it).
    function _rebuildSelfviewRow() {
        for (var i = 0; i < tileModel.count; i++) {
            if (tileModel.get(i).id === "vid_selfview") { tileModel.remove(i); break }
        }
        core._syncTileModel()
    }
    Connections {
        target: callctl
        function onSelfviewGenChanged() { core._rebuildSelfviewRow() }
    }

    // ===================== Hide / reshow call-grid entities =====================
    // Any tile — a person's camera, their screen share, an audio-only avatar — can be
    // hidden from the grid and brought back later. View-only: hiding never mutes
    // anyone and never touches what we publish, it just stops giving them grid space.
    // The tile's delegate stays alive (so the video sink is not detached/reattached);
    // it is made invisible and the layout gives it a zero slot, like the AFK cut.
    //
    // TWO LAYERS (0.8.7): `mHiddenTiles` is the SESSION layer — tile ids, cleared
    // when the call ends; it also serves the PiP pop-out's auto-hide, which must
    // never be remembered. `hiddenKeys` is the MEMORY — "mxid|kind" keys persisted
    // via settings and re-applied to matching tiles whenever the tile list changes,
    // so a hide (or a show) survives across calls and restarts. Keys use the BARE
    // mxid: tile.participant is the full LiveKit identity @user:server:DEVICE and
    // the device rotates per login — remembering it would forget people weekly.
    property var mHiddenTiles: []
    property var hiddenKeys: []
    function tileMxid(p) {
        var m = /^(@[^:]+:[^:]+)/.exec(p || "")
        return m ? m[1] : (p || "")
    }
    function tileKey(md) {
        if (!md || !md.participant) return ""
        var kind = (md.screenshare === true || md.kind === "screenshare") ? "share"
                 : (md.kind === "avatar" ? "avatar" : "cam")
        return core.tileMxid(md.participant) + "|" + kind
    }
    function isKeyHidden(md) { return core.hiddenKeys.indexOf(core.tileKey(md)) !== -1 }
    // 0.8.22: per-person tile VIEW memory {key: {z,px,py,fill}} — a zoom, pan or
    // fit/fill you set on someone follows them into every later call (same key as hides).
    property var tileViews: ({})
    // Cameras fill the tile by default (no letterbox); screen shares fit (cropping a
    // share loses content).
    // Default view mode per tile kind: cameras "auto" — fill (cover-crop) only when
    // the crop is small, else fit with bars; screen shares "fit" — never crop someone's screen.
    function defaultFillModeFor(md) { return (md && (md.screenshare === true || md.kind === "screenshare")) ? "fit" : "auto" }
    function restoreTileView(t) {
        if (!t || !t.modelData) return
        var v = core.tileViews[core.tileKey(t.modelData)]
        if (!v) return
        if (typeof v.mode === "string") t.thFillMode = v.mode
        else if (typeof v.fill === "boolean") t.thFillMode = v.fill ? "fill" : "fit" // 0.8.22 files
        if (typeof v.z === "number" && v.z > 0) t.thZoom = Math.min(6, v.z)
        if (typeof v.px === "number") t.thPanX = v.px
        if (typeof v.py === "number") t.thPanY = v.py
    }
    function saveTileView(t) {
        if (!t || !t.modelData) return
        var k = core.tileKey(t.modelData)
        if (k.length === 0) return
        var next = Object.assign({}, core.tileViews)
        var def = core.defaultFillModeFor(t.modelData)
        if (Math.abs(t.thZoom - 1.0) <= 0.001 && t.thPanX === 0 && t.thPanY === 0 && t.thFillMode === def) delete next[k]
        else next[k] = { z: Math.round(t.thZoom * 100) / 100, px: Math.round(t.thPanX * 100) / 100,
                         py: Math.round(t.thPanY * 100) / 100, mode: t.thFillMode }
        core.tileViews = next
        settings.applyCallTileView(JSON.stringify(next))
    }
    Timer {
        id: tileViewSaveTimer
        interval: 600
        property var pending: null
        onTriggered: { try { if (pending && pending.modelData) core.saveTileView(pending) } catch (e) {} pending = null }
    }
    function saveTileViewLater(t) { tileViewSaveTimer.pending = t; tileViewSaveTimer.restart() }
    // Self view: a small picture-in-picture over the grid whenever someone else is on
    // screen ("I want to see her, and a small me"); "grid" puts it back as a normal tile.
    property string selfViewMode: "pip"
    readonly property bool selfPip: core.selfViewMode === "pip"
    // 10.1 (TH-CALL-24): the PiP slot only exists beside a share or under a speaker's stage; in a
    // plain gallery / voice grid it landed ON the last person's tile — there, self is a tile.
    readonly property bool callHasShare: (core.callTiles || []).some(function (t) { return core.isShareTile(t) })
    readonly property bool selfPipEffective: core.selfPip && (core.callHasShare || core.callLayout === "speaker")
    function setSelfViewMode(m) {
        core.selfViewMode = m
        var next = Object.assign({}, core.tileViews); next._self = { mode: m }
        core.tileViews = next
        settings.applyCallTileView(JSON.stringify(next))
    }
    function _persistHiddenKeys() { settings.applyCallHidden(JSON.stringify(core.hiddenKeys)) }
    // A peer's "recording only" share stays hidden (and muted, streamer-side)
    // until this user tunes in — same visual treatment as a hidden tile, own way back.
    function isRecOnly(id) { var f = core.mRecOnlyFlags[id]; return !!f && f[0] === true }
    function isTunedIn(id) { var f = core.mRecOnlyFlags[id]; return !!f && f[1] === true }
    function isRecOnlyHidden(id) { return core.isRecOnly(id) && !core.isTunedIn(id) }
    readonly property var recOnlyHiddenIds: {
        var out = []
        for (var id in core.mRecOnlyFlags) if (core.isRecOnlyHidden(id)) out.push(id)
        return out
    }
    function isTileHidden(id) { return core.mHiddenTiles.indexOf(id) !== -1 || core.isRecOnlyHidden(id) }
    // Game chat: `peers`/`muted` hold FULL LiveKit identities; strip rows carry bare mxids.
    function isGameChat(identity) { return core.mGameChatPeers.peers.indexOf(identity) !== -1 }
    function isGameChatMuted(identity) { return core.mGameChatPeers.muted.indexOf(identity) !== -1 }
    function isGameChatMxid(mxid) {
        var ps = core.mGameChatPeers.peers
        for (var i = 0; i < ps.length; i++) if (core.tileMxid(ps[i]) === mxid) return true
        return false
    }
    // 9.4 corners: which ACTIVE corner is this identity / mxid in ("" = none)?
    function cornerOf(identity) {
        var cs = core.mCorner.corners
        for (var id in cs) if (cs[id].indexOf(identity) !== -1) return id
        return ""
    }
    function cornerOfMxid(mxid) {
        var cs = core.mCorner.corners
        for (var id in cs) for (var i = 0; i < cs[id].length; i++) if (core.tileMxid(cs[id][i]) === mxid) return id
        return ""
    }
    function isAside(identity) { return core.cornerOf(identity).length > 0 }
    function isAsideMxid(mxid) { return core.isAsideMxid_(mxid) }
    function isAsideMxid_(mxid) { return core.cornerOfMxid(mxid).length > 0 }
    function inMyCorner(identity) { return core.mCorner.active && core.mCorner.members.indexOf(identity) !== -1 }
    // Display names for a list of identities (tile names, else the localpart), "you" excluded.
    function cornerNames(ids) {
        var out = []
        for (var i = 0; i < (ids || []).length; i++) {
            var p = ids[i]
            if (core.tileMxid(p) === core.mUserId) continue
            var name = ""
            for (var j = 0; j < core.mVideoTiles.length; j++)
                if (core.mVideoTiles[j].participant === p && core.mVideoTiles[j].name) { name = core.mVideoTiles[j].name; break }
            out.push(name.length > 0 ? name : core.localpart(core.tileMxid(p)))
        }
        return out.join(", ")
    }
    // Names of the people aside with the given identity (their corner minus themselves and me).
    function cornerMatesOf(identity) {
        var id = core.cornerOf(identity)
        if (id.length === 0) return ""
        return core.cornerNames(core.mCorner.corners[id].filter(function (p) { return p !== identity }))
    }
    // A stream's audio plays only while you WATCH it — pinned, popped out, an
    // explicit Listen, or the only stream on screen. Several streams visible: silent until
    // you choose one. QML owns this truth and pushes it to the streamer (_syncWatched).
    function isShareTile(t) { return !!t && (t.screenshare === true || t.kind === "screenshare") }
    function isShareTileId(id) {
        for (var i = 0; i < core.callTiles.length; i++)
            if (core.callTiles[i] && core.callTiles[i].id === id) return core.isShareTile(core.callTiles[i])
        return false
    }
    readonly property var visibleShareIds: {
        var out = []
        for (var i = 0; i < core.callTiles.length; i++) {
            var t = core.callTiles[i]
            if (core.isShareTile(t) && t.id !== "vid_selfshare" && !core.isTileHidden(t.id)) out.push(t.id)
        }
        return out
    }
    readonly property var watchedShareIds: {
        void core._pipsRev
        var vis = core.visibleShareIds
        var out = []
        for (var i = 0; i < vis.length; i++) {
            var id = vis[i]
            if (core._pips[id] || core.pinnedTile === id || core.listenTiles.indexOf(id) !== -1) out.push(id)
        }
        // A popped-out stream leaves the grid (auto-hide) yet is very much being watched.
        for (var k in core._pips) if (out.indexOf(k) === -1 && core.isShareTileId(k)) out.push(k)
        if (out.length === 0 && vis.length === 1) out.push(vis[0])
        return out
    }
    function isWatched(id) { return core.watchedShareIds.indexOf(id) !== -1 }
    function isListening(id) { return core.listenTiles.indexOf(id) !== -1 }
    function setListening(id, on) {
        var next = core.listenTiles.filter(function(x) { return x !== id })
        if (on) next.push(id)
        core.listenTiles = next
    }
    // Push the watched set to the streamer (idempotent there; it logs real transitions).
    function _syncWatched() {
        if (core.fakeCall) return
        var w = core.watchedShareIds
        for (var i = 0; i < core.callTiles.length; i++) {
            var t = core.callTiles[i]
            if (core.isShareTile(t) && t.id !== "vid_selfshare") callctl.setTileWatched(t.id, w.indexOf(t.id) !== -1)
        }
    }
    onWatchedShareIdsChanged: core._syncWatched()
    function _hookArg(flag) {
        var args = Qt.application.arguments
        for (var i = 0; i < args.length; i++) {
            var a = String(args[i])
            if (a === flag) return ""
            if (a.indexOf(flag + "=") === 0) return a.substring(flag.length + 1)
        }
        return null
    }
    function _navHasCall() {
        if (core.mInCall) return true
        for (var i = 0; i < core.mNavRooms.length; i++) if (core.mNavRooms[i] && core.mNavRooms[i].call === true) return true
        return false
    }
    // For the room we are IN the strip shows the SFU-truth roster (what the tile
    // grid shows) — instant and ghost-free. Self is added by hand (no tile without a camera).
    readonly property var liveCallParts: {
        if (!core.mInCall) return []
        var out = [], seen = {}
        var meId = core.mUserId
        if (meId.length > 0) { seen[meId] = true; out.push({ mxid: meId, name: core.localpart(meId), avatar: core.mMyAvatar || "" }) }
        for (var i = 0; i < core.mVideoTiles.length; i++) {
            var t = core.mVideoTiles[i]
            if (!t || !t.participant || t.id === "vid_selfview" || t.id === "vid_selfshare") continue
            var m = core.tileMxid(t.participant)
            if (!m || seen[m]) continue
            seen[m] = true
            out.push({ mxid: m, name: (t.name && t.name.length > 0) ? t.name : core.localpart(t.participant), avatar: t.avatar || "" })
        }
        return out
    }
    function hideTile(id) {
        if (!id || core.isTileHidden(id)) return
        var next = core.mHiddenTiles.slice()
        next.push(id)
        core.mHiddenTiles = next          // reassign (not push) so bindings re-evaluate
    }
    // A USER hide: session-hide the tile AND remember person|kind for future calls.
    function hideTileRemember(md) {
        if (!md) return
        core.hideTile(md.id)
        var k = core.tileKey(md)
        if (k.length > 0 && core.hiddenKeys.indexOf(k) === -1) {
            var next = core.hiddenKeys.slice()
            next.push(k)
            core.hiddenKeys = next
            core._persistHiddenKeys()
        }
        core.notify("Hidden — remembered for future calls too. Bring it back from the \"hidden\" chip in the call header.", "short")
    }
    function unhideTile(id) {
        var next = core.mHiddenTiles.filter(function(x) { return x !== id })
        if (next.length !== core.mHiddenTiles.length) core.mHiddenTiles = next
        // Showing is remembered exactly like hiding: drop this tile's memory key.
        for (var i = 0; i < core.callTiles.length; i++) {
            var t = core.callTiles[i]
            if (t && t.id === id) {
                var k = core.tileKey(t)
                var kept = core.hiddenKeys.filter(function(x) { return x !== k })
                if (kept.length !== core.hiddenKeys.length) {
                    core.hiddenKeys = kept
                    core._persistHiddenKeys()
                }
                break
            }
        }
    }
    // "Show all & forget": clears the session layer AND the whole memory — a full
    // reset the chip's menu spells out, so nobody stays invisibly remembered-hidden.
    function unhideAllTiles() {
        if (core.mHiddenTiles.length > 0) core.mHiddenTiles = []
        if (core.hiddenKeys.length > 0) {
            core.hiddenKeys = []
            core._persistHiddenKeys()
        }
    }
    // Re-apply the memory whenever the tile list changes (tiles get NEW ids every
    // call — the memory matches people, this maps them back to today's ids). QML
    // allows ONE onCallTilesChanged handler per object and the PiP cleanup already
    // owns it — this is called from the top of that handler.
    function _applyHiddenMemory() {
        var add = []
        for (var i = 0; i < core.callTiles.length; i++) {
            var t = core.callTiles[i]
            if (t && core.isKeyHidden(t) && !core.isTileHidden(t.id)) add.push(t.id)
        }
        if (add.length > 0) core.mHiddenTiles = core.mHiddenTiles.concat(add)
    }
    // Startup path: the memory loads from disk AFTER callTiles' initial binding (and
    // a static tile list — fakeTiles — never re-fires onCallTilesChanged at all), so
    // re-apply whenever the KEY SET itself changes too. Caught by the hidemem gate:
    // R2's relaunch shot showed the hidden tile back in the grid with the pref intact.
    onHiddenKeysChanged: _applyHiddenMemory()
    // 🔴 the Memories recorder is capturing this call. 0.8.8: the recorder no longer
    // renders as a participant tile (field report: "I don't want it showing as another
    // user") — call_ctl filters it out of the grid and flips callctl.recActive off
    // its REAL SFU presence instead, so the cue still can't claim a recording that
    // isn't happening.
    readonly property bool recActive: callctl.recActive && core.callShown
    // How many of the CURRENT tiles are hidden (ignores stale ids from tiles that
    // have since left the call, so the "N hidden" chip can never lie).
    readonly property int hiddenTileCount: core.callTiles.filter(function(t) {
        return t && core.isTileHidden(t.id)
    }).length
    property string navMode: "persistent"
    // True while the user is drag-reordering the Spaces rail — pauses the spaces mirror
    // so a refresh can't reset the DelegateModel mid-drag.
    property bool navReordering: false
    // #9 Pause-hidden: stop decoding remote call video while the window is minimized/hidden
    // (saves GPU/decode on tiles nobody can see). Re-applied when shown + when tiles change.
    function _applyVisibilityPause() {
        // 11.0 (GPU.md §2.11, R17): the window state per remote track (unfocused → cameras on the LOW layer at ≤ 15 fps,
        // hidden → released unless a visible pop-out shows it) + the self-view pause; the pop-outs' tiles keep playing.
        // Always pushed (the policy outlives a call); the older minimise pause below stays as it was.
        callctl.setPopoutTiles(JSON.stringify(Object.keys(core._pips)))
        callctl.setWindowState(core.winState, core.mCallUnfocusedSaver)
        var hidden = (core.shell.visibility === Window.Minimized || core.shell.visibility === Window.Hidden)
        // VERIFIED 2026-07-03 (KWin script minimized the window; no event arrived): KWin
        // Wayland does NOT surface minimize to the client — Window.Minimized never fires
        // here, so this branch only engages on Hidden (and on X11/other platforms). The
        // real per-tile savings come from the tile-size/AFK/receive-cap SFU levers. Keep
        // the branch: correct where the compositor DOES report it, and future Qt/KWin
        // may map xdg-toplevel 'suspended' into visibility.
        console.log("[vispause] windowVisibility=" + core.shell.visibility + " -> " + (hidden ? "pause" : "resume") + " inCall=" + callctl.inCall)
        if (!callctl.inCall) return
        // Don't pause remote tiles while ANY tile is popped out — a PiP is always-on-top and
        // stays visible even when the main window is minimized, so pausing would freeze it
        // (bug C). Keep everything live until the last PiP closes.
        if (Object.keys(_pips).length > 0) { callctl.pauseRemoteTiles(false); return }
        callctl.pauseRemoteTiles(hidden)
    }

    // #9 PiP pop-out: open a small always-on-top window showing ONE call tile's video. Keyed
    // by tile id so a tile can't be popped out twice. The PiP is a pure VIEWER (PipVideoItem +
    // a second sink on the same track — see PipWindow.qml): the grid tile keeps its qml6glsink
    // and keeps playing the whole time, so open/close needs no grid-side release/re-arm at all.
    property var _pips: ({})
    // 0.8.21: the pop-out gets the tile's name, share flag and the audio-prefs
    // controller so it can carry mute/volume/quality controls of its own.
    function openPip(tileId, participant, name, isShare) {
        if (_pips[tileId]) { _pips[tileId].raise(); _pips[tileId].requestActivate(); return }
        // 12.1.3: a muted camera has no video to pop out (its tile shows the avatar)
        if (callTiles.some(function(t) { return t.id === tileId && t.cameraMuted === true })) return
        var comp = Qt.createComponent("PipWindow.qml")
        if (comp.status !== Component.Ready) { console.warn("[pip] component:", comp.errorString()); return }
        var w = comp.createObject(core.shell, { ctl: callctl, audioPrefs: audioPrefsCtl, tileId: tileId,
                                          participant: participant, name: name || "", isShare: isShare === true,
                                          volMxid: core.tileMxid(participant) })
        if (!w) { console.warn("[pip] createObject failed"); return }
        _pips[tileId] = w
        core._pipsRev++
        // 10.1 (TH-POP-09): the pop-out comes back the size you left it (and where, off Wayland).
        var pw = parseInt(settings.uiPref("pip-w")), ph = parseInt(settings.uiPref("pip-h"))
        if (!isNaN(pw) && !isNaN(ph) && pw >= w.minimumWidth && ph >= 120 && pw < 8000 && ph < 8000) { w.width = pw; w.height = ph }
        if (Qt.platform.pluginName.indexOf("wayland") !== 0) {
            var px = parseInt(settings.uiPref("pip-x")), py = parseInt(settings.uiPref("pip-y"))
            if (!isNaN(px) && !isNaN(py) && px > -4000 && py > -4000) { w.x = px; w.y = py }
        }
        // By request: a popped-out stream should leave the grid — the same
        // pixels twice wastes space AND decode. VIEW-side hide only (the tile is
        // hidden, not SFU-paused: the PiP still needs the frames flowing). Track
        // which tiles WE hid so closing the PiP only un-hides what the pop-out
        // hid — never a tile the user had hidden by hand before popping out.
        var autoHid = !core.isTileHidden(tileId)
        if (autoHid) core.hideTile(tileId)
        w.closing.connect(function() {
            if (w.width >= w.minimumWidth && w.height >= 120) {
                settings.applyUiPref("pip-w", String(Math.round(w.width))); settings.applyUiPref("pip-h", String(Math.round(w.height)))
                settings.applyUiPref("pip-x", String(Math.round(w.x))); settings.applyUiPref("pip-y", String(Math.round(w.y)))
            }
            delete core._pips[tileId]
            core._pipsRev++
            w.destroy()
            if (autoHid) core.unhideTile(tileId)
            core._applyVisibilityPause()   // re-pause the now-non-popped tiles if minimized
        })
    }
    // Refresh everything the gear menu displays (device lists + persisted toggles) —
    // shared by BOTH openers (room-header gear, in-call bar gear). Loading the persisted
    // mic toggles matters when the menu opens BEFORE a call: the checkboxes would read
    // the default-OFF qprops until do_join refreshes them.
    function refreshDeviceMenus() {
        callctl.refreshInputs(); callctl.refreshOutputs(); callctl.refreshCameras()
        callctl.refreshAudioSources()
        callctl.refreshMicDenoise(); callctl.refreshMicApm(); callctl.refreshMicAec()
        settings.refreshOverlay()
    }
    property var _overlayWin: null
    property bool mOverlayUnavailable: false
    function ensureOverlay() {
        if (_overlayWin || mOverlayUnavailable) return
        var comp = Qt.createComponent("OverlayWindow.qml")
        if (comp.status !== Component.Ready) {
            console.warn("[overlay] component:", comp.errorString())
            mOverlayUnavailable = true
            return
        }
        _overlayWin = comp.createObject(core.shell, { app: core.shell })
        if (!_overlayWin) { console.warn("[overlay] createObject failed"); mOverlayUnavailable = true }
    }
    // If a popped-out tile leaves the call (participant left / track gone), close its PiP so it
    // doesn't sit frozen on a black/last frame and its _pips slot doesn't block re-opening that
    // id later (bugs B + D). callTiles changes only when the real tile list changes.
    onCallTilesChanged: {
        core._syncTileModel()     // 0.8.28: the grid model follows the array in place
        core._applyHiddenMemory() // 0.8.7 hide/show memory (single-handler rule)
        core._syncWatched()       // new share tiles start unwatched — push the truth now
        var keys = Object.keys(_pips)
        if (keys.length === 0) return
        // 12.1.3: a muted camera keeps its tile (drawn as the avatar) but its pop-out would sit on the last frame:
        // it counts as gone for the PiP (closed), like a tile that left
        var ids = callTiles.filter(function(t) { return t.cameraMuted !== true }).map(function(t) { return t.id })
        // A pinned tile that left the call must not keep an empty main area pinned (a muted camera stays pinned).
        if (core.pinnedTile.length > 0 && !callTiles.some(function(t) { return t.id === core.pinnedTile })) core.pinnedTile = ""
        for (var i = 0; i < keys.length; ++i) {
            if (ids.indexOf(keys[i]) < 0 && _pips[keys[i]])
                _pips[keys[i]].close()   // -> its onClosing cleans _pips + destroys
        }
    }

    // ── 10.5 "can they hear me?" helpers ─────────────────────────────────────────────────────
    // What hear.rs knows about one person, by LiveKit identity (tiles may carry the identity or the
    // bare mxid — matched like tileAudioLevel does).
    function hearPeer(identity) {
        var peers = (mHear && mHear.peers) ? mHear.peers : null
        if (!peers || !identity) return null
        if (peers[identity]) return peers[identity]
        for (var k in peers) {
            if (k.indexOf(identity + ":") === 0 || identity.indexOf(k + ":") === 0) return peers[k]
        }
        return null
    }
    // A display name for an identity: the call tile's resolved name, else the localpart.
    function hearName(identity) {
        var tiles = mVideoTiles || []
        for (var i = 0; i < tiles.length; ++i) {
            var p = tiles[i].participant || ""
            if (tiles[i].name && (p === identity || identity.indexOf(p + ":") === 0 || p.indexOf(identity + ":") === 0))
                return tiles[i].name
        }
        var lp = String(identity).split(":")[0]
        return lp.charAt(0) === "@" ? lp.substring(1) : lp
    }
    function hearNames(ids) {
        var n = (ids || []).map(function (i) { return core.hearName(i) })
        if (n.length <= 1) return n.join("")
        return n.slice(0, -1).join(", ") + " and " + n[n.length - 1]
    }
    // A receive problem in words. `aboutMe` = their report about US (else: what WE receive from them).
    function hearReason(st, aboutMe) {
        if (aboutMe) {
            if (st === "silent") return "your audio reaches them silent"
            if (st === "noaudio") return "no audio from you is arriving there"
            if (st === "nokey") return "they can't decrypt you — the key is missing"
            if (st === "stalled") return "their speakers stalled — their app is repairing it"
            if (st === "nosub") return "their app isn't subscribed to your mic"
            if (st === "muted") return "they still see you as muted"
            return "their app reports a problem"
        }
        if (st === "silent") return "they're talking, but their audio arrives silent"
        if (st === "noaudio") return "no audio from them is arriving"
        if (st === "nokey") return "their encryption key is missing"
        if (st === "stalled") return "your output for them stalled — repairing"
        return st
    }
    function hearProblem(st) { return st === "silent" || st === "noaudio" || st === "nokey" || st === "stalled" }
    // 11.3 same room helpers (pure; pass the state in so bindings re-evaluate on change).
    // Who 10.5's detector pairs me with (their stream carries my voice < 120 ms, or they report the same about
    // my mic) — minus whoever I'm already linked with or asking.
    function sameRoomSuggest(hear, sr) {
        var out = []
        if (!hear || !hear.v) return out
        var rm = (hear.echoMe && hear.echoMe.room) ? hear.echoMe.room : []
        for (var i = 0; i < rm.length; i++) if (out.indexOf(rm[i]) < 0) out.push(rm[i])
        var peers = hear.peers || {}
        for (var id in peers) { var p = peers[id]; if (p && p.echo && p.echo.room && out.indexOf(id) < 0) out.push(id) }
        var linked = (sr && sr.linked) ? sr.linked : []
        var pend = (sr && sr.pending) ? sr.pending : ""
        return out.filter(function(x) { return linked.indexOf(x) < 0 && x !== pend })
    }
    // The group a tile belongs to ("" = me): { m, mic, live } or null.
    function sameRoomGroupOf(identity, sr) {
        if (!sr || !sr.linked) return null
        if (identity === "") return sr.linked.length > 0 ? ({ m: sr.linked.concat(["You"]), mic: sr.live ? "You" : sr.holder, live: sr.live }) : null
        var g = sr.groups ? sr.groups[identity] : null
        return g ? ({ m: g.m || [], mic: g.mic || "", live: g.mic === identity }) : null
    }
    function sameRoomTip(isSelf, identity, sr) {
        var g = sameRoomGroupOf(isSelf ? "" : identity, sr)
        if (!g) return ""
        var self0 = isSelf ? "You" : identity
        var others = g.m.filter(function(m) { return m !== self0 }).map(function(m) { return m === "You" ? "you" : hearName(m) }).join(", ")
        if (isSelf) return "You're in a room with " + others + " — Tin Hat sends one voice from your room" + (g.live ? " (your mic is the room mic right now)" : " (their mic is the room mic right now)")
        return hearName(identity) + " is in a room with " + others + " — Tin Hat sends one voice from their room" + (g.live ? " (their mic is live right now)" : "")
    }
    // "Heard by 3 of 3" — reported by the listeners' apps (tinhat.rx receipts), never guessed.
    function heardSummary() {
        var h = (mHear && mHear.heard) ? mHear.heard : null
        if (!h || !mInCall || h.total === 0) return ""
        if (mMicMuted) return "You're muted — nobody hears your mic"
        // Only people whose app reports count towards "of N"; the rest are named as unconfirmed,
        // never as a failure (Element X / the web client don't send receipts).
        var unconf = h.unconf ? h.unconf.length : 0
        var pending = h.pending ? h.pending.length : 0   // subscribed, first audio not in yet
        var deaf = h.deaf || []                           // they receive us but have deafened / gone away
        var reporting = h.total - unconf - pending
        var parts = []
        if (reporting > 0) parts.push("Heard by " + h.ok + " of " + reporting)
        if (h.not && h.not.length > 0) parts.push(hearNames(h.not) + (h.not.length === 1 ? " isn't" : " aren't") + " receiving you")
        if (deaf.length > 0) parts.push(hearNames(deaf) + (deaf.length === 1 ? " has" : " have") + " deafened or stepped away")
        if (unconf > 0) parts.push(hearNames(h.unconf) + (unconf === 1 ? "'s app doesn't" : "'s apps don't") + " report what it hears")
        return parts.join(" · ")
    }
    // The "You're muted" bubble: at most once per 30 s, gone 3 s after the talking stops; after two
    // dismissals in a call only the mic button's pulse is left. Optional sound (off by default).
    property bool mMutedTalkBubble: false
    property int _mutedTalkDismissals: 0
    property double _mutedTalkShownAt: 0
    property double _mutedTalkSoundAt: 0
    readonly property bool mutedTalkNow: !!(mHear && mHear.mutedTalk) && mMicMuted && !mServerMuted
    onMutedTalkNowChanged: {
        if (mutedTalkNow) {
            mutedTalkHide.stop()
            var now = Date.now()
            if (!mMutedTalkBubble && _mutedTalkDismissals < 2 && now - _mutedTalkShownAt > 30000) {
                mMutedTalkBubble = true
                _mutedTalkShownAt = now
            }
            if (settings.pref("snd.mutedtalk") === "1" && now - _mutedTalkSoundAt > 10000) {
                _mutedTalkSoundAt = now
                uiSoundOut.volume = core.sndVolume()
                uiSound.stop(); uiSound.source = core.soundUrl("muted"); uiSound.play()
            }
        } else if (mMutedTalkBubble) {
            mutedTalkHide.restart()
        }
    }
    Timer { id: mutedTalkHide; interval: 3000; onTriggered: core.mMutedTalkBubble = false }
    function dismissMutedTalk() { mMutedTalkBubble = false; _mutedTalkDismissals += 1 }
    Connections {
        target: callctl
        function onInCallChanged() { core._mutedTalkDismissals = 0; core._mutedTalkShownAt = 0; core.mMutedTalkBubble = false }
        function onMicMutedChanged() { if (!callctl.micMuted) core.mMutedTalkBubble = false }
    }

    // Themed call join/leave chimes — a local MediaPlayer plays the CURRENT theme's sound when a
    // REMOTE participant joins/leaves (callctl signals). Local-only; peers don't hear it.
    MediaPlayer {
        id: callChime
        audioOutput: AudioOutput { id: callChimeOut; volume: 0.85 }
    }
    function playCallChime(kind) {   // kind = "join" | "leave"
        // 10.1 (TH-NOTIF-02): honours the sound switch + slider like every other sound (the
        // Settings copy always said it did).
        if (!core.sndOn("callchime")) return
        callChimeOut.volume = core.sndVolume()
        callChime.stop()
        callChime.source = core.soundsDir + "/" + core.soundPackDir + "/" + kind + ".mp3"   // themes 1b: the sound pack
        callChime.play()
    }
    // ---- 10.0 UI sounds: message send/receive (open room), the notification sound, the ringtone.
    // All local. Prefs (settings.pref): snd.send / snd.receive / snd.notify / snd.ring = "0" to switch
    // off (default on); snd.notify.variant / snd.ring.variant = "" | "2" | "3" | "4" (the pack's
    // options); snd.volume = 0..100 (default 70). Files: soundsDir/<theme>/<slot>[-variant].mp3.
    function sndOn(key) { return settings.pref("snd." + key) !== "0" }
    function sndVariant(key) { var v = settings.pref("snd." + key + ".variant"); return (v === "2" || v === "3" || v === "4") ? "-" + v : "" }
    function sndVolume() { var v = parseInt(settings.pref("snd.volume")); return (isNaN(v) ? 70 : Math.max(0, Math.min(100, v))) / 100 }
    function soundUrl(file) { return core.soundsDir + "/" + core.soundPackDir + "/" + file + ".mp3" }   // themes 1b: the sound pack
    MediaPlayer {
        id: uiSound
        audioOutput: AudioOutput { id: uiSoundOut; volume: 0.7 }
    }
    property double _lastUiSoundAt: 0
    property string _lastUiSoundKind: ""
    property string _previewFile: ""
    // kind = "send" | "receive" | "notify". Bursts of the same kind within 350 ms play once.
    function playUiSound(kind) {
        if (!core.sndOn(kind)) return
        var now = Date.now()
        if (kind === core._lastUiSoundKind && now - core._lastUiSoundAt < 350) return
        core._lastUiSoundKind = kind; core._lastUiSoundAt = now
        core._previewFile = ""
        uiSoundOut.volume = core.sndVolume()
        uiSound.stop(); uiSound.source = core.soundUrl(kind === "notify" ? "notify" + core.sndVariant("notify") : kind); uiSound.play()
    }
    // Settings previews: the same player; clicking the playing one stops it (the ringtones are long).
    function previewSound(file) {
        if (core._previewFile === file && uiSound.playbackState === MediaPlayer.PlayingState) { uiSound.stop(); core._previewFile = ""; return }
        core._previewFile = file
        uiSoundOut.volume = core.sndVolume()
        uiSound.stop(); uiSound.source = core.soundUrl(file); uiSound.play()
    }
    // The ringtone loops until stopRing(); the incoming-call toast bounds it.
    MediaPlayer {
        id: ringPlayer
        loops: MediaPlayer.Infinite
        audioOutput: AudioOutput { id: ringOut; volume: 0.7 }
    }
    function startRing() { if (!core.sndOn("ring")) return; ringOut.volume = core.sndVolume(); ringPlayer.stop(); ringPlayer.source = core.soundUrl("ring" + core.sndVariant("ring")); ringPlayer.play() }
    function stopRing() { ringPlayer.stop() }

    // Chimes suppressed until this epoch-ms — set when WE join, because the SFU seeds EVERY peer
    // already in the call as a ParticipantJoined on connect, which would blast a chime per peer.
    property double _chimeArmAt: 0
    Connections {
        target: callctl
        // 10.0: your OWN transitions sound too — join on the way in, leave on the way out (a
        // call switch plays leave then join as the two edges happen).
        function onInCallChanged() { if (callctl.inCall) { core._chimeArmAt = Date.now() + 2500; core.playCallChime("join") } else core.playCallChime("leave") }
        // BOTH edges refresh the strip (a join never did — only leaves).
        function onParticipantJoined(identity) { if (Date.now() >= core._chimeArmAt) core.playCallChime("join"); nav.refreshParticipants() }
        function onParticipantLeft(identity) { core.playCallChime("leave"); nav.refreshParticipants() } // drop their bubble from the room-list call strip
    }

    // After leaving a call, the room-list call-participant strip reads m.call.member STATE, which
    // only clears once our async membership withdrawal round-trips via /sync. Re-refresh the nav a
    // few times so the strip clears WITHOUT the user navigating away + back (an immediate refresh
    // alone reads the still-active pre-sync membership).
    // Self-heal: the watched set is process-global in the streamer; re-push it every
    // 5 s while in a call (idempotent) so a missed edge can never leave a stream muted.
    Timer { interval: 5000; repeat: true; running: core.mInCall; onTriggered: core._syncWatched() }
    Timer {
        id: leaveStripRefresh
        property int _n: 0
        interval: 2500; repeat: true
        onTriggered: { nav.refresh(); _n += 1; if (_n >= 3) { stop(); _n = 0 } }
    }
    // 0.8.33: the echo canceller's speaker-output list (EcOutputsPopup).
    function ecOutputLabels() {
        var out = []
        for (var i = 0; i < core.mEcOutputs.length; i++) {
            var n = core.mEcOutputs[i], label = n
            for (var j = 0; j < core.mOutputs.length; j++) if (core.mOutputs[j].node === n) { label = core.mOutputs[j].label; break }
            out.push(label)
        }
        return out.join(", ")
    }
    function toggleEcOutput(node) {
        var list = core.mEcOutputs.slice()
        var k = list.indexOf(node)
        if (k >= 0) list.splice(k, 1); else list.push(node)
        callctl.applyEcOutputs(JSON.stringify(list))
    }
    LoginController { id: login }
    BootController { id: bootctl } // startup window handshake (0.8.24)
    RoomController { id: room }
    StreamController { id: stream }
    CallController { id: callctl }
    SettingsController { id: settings }
    DiagnosticsController { id: diagnostics }
    UpdateController { id: updater }
    GameActivityController { id: activityCtl }   // "what we're playing" (dev.tinhat.activity)
    UserStatusController { id: statusCtl }       // per-person "looking to play" (dev.tinhat.userstatus)
    GifController { id: gifCtl }                 // in-chat Giphy picker (via the server's Tin Hat API)
    AudioPrefsController { id: audioPrefsCtl }   // per-participant call volume/mute (#5)
    SoundboardController { id: soundboardCtl }   // room + personal sounds, web search (dev.tinhat.soundboard)
    SoundboardController { id: joinSoundCtl }    // Settings → join-sound picker search: its OWN results
    WatchPartyController { id: watchCtl }         // watch party: queue + sync-clock + viewer presence + search
    NavController { id: nav }                     // corner-dock navigation (spaces -> rooms drill-down)
    AfkController { id: afkCtl }                  // #7 BRB/AFK presence in a call (Matrix reactions/state)
    FloatingController { id: floatingCtl }        // #6 floating chat-bubble registry + per-room metadata
    CalendarController { id: calCtl }             // 0.8.26 squad calendar (dev.tinhat.calendar.*) in the space
    IconPackController { id: iconPack }           // 0.8.26 uniform/animated icon pack discovery
    GamesController { id: gamesCtl }              // 9.0 Game Reel: what do we play tonight (dev.tinhat.games.*)
    SpaceController { id: spaceCtl }              // RBAC: a space's overview / members / roles / rooms (docs/RBAC.md)
    InviteController { id: invitesCtl }           // 10.4: registration invite codes for server operators (docs/INVITES.md)
    GuestsController { id: guestsCtl }            // selfhost: the guest room — Settings → Guests, the bar, the notices (docs/GUESTS.md)
    PresenceController { id: presenceCtl }        // 10.6: opt-in Online / Away / In a call + the "at the computer" pings (docs/REACH.md)
    ThemeController { id: themeCtl }              // themes (C1): resolves a theme id through the engine for the Th singleton
    ThemeEditorController { id: editorCtl }       // 11.0 step 5: the theme editor, share cards (Try / Keep), the Pick tool
    LowGpuController { id: lowgpuCtl }            // 11.0 (GPU.md): window state, motion switch, the duration token, ring, busy mark …
    ActionsController { id: actionsCtl }          // 11.0 step 3 (buttons): rows, availability, the Ctrl+K palette (crates/actions)
    // 11.1 (docs/GAMESERVERS.md): the Game servers status — ONE poller for the widget, Up next and the calendar card; it
    // follows the RENDER window state (focused 15 s, unfocused 60 s, hidden: nothing) and forgets everything on sign-out
    GameServersController { id: gameServersCtl; Component.onCompleted: gameServersCtl.setWindow(core.winState) }
    // Pad (docs/PAD.md, pad_ctl.rs): my other verified devices drive my call here (the receiver: PadHost.qml in the
    // shell runs their commands), and this device drives theirs (the Pad window). Mirrored below on `changed`.
    PadController { id: padCtl }
    property bool mPadOn: true                       // Settings → Calls → Pad
    property var mPadChips: []                       // [{device, name}] my devices with a live Pad session here
    property var mPadTargets: ({ rooms: {}, names: {} })   // my other devices in each call (discovery + why not)
    property var mPadRemote: null                    // the session this device drives (state, reach, pending)
    property var mPadSounds: ({})                    // {room, roomSounds, people} for the Pad window
    property int _padRev: -1
    property bool padDemo: false                     // --tinhat-paddemo (Main.qml): sample data stays, the mirror waits
    function _mirrorPad() {
        if (core.padDemo || padCtl.rev === core._padRev) return
        core._padRev = padCtl.rev
        core.mPadOn = padCtl.padOn
        try { core.mPadChips = JSON.parse(padCtl.chipsJson) || [] } catch (e) { core.mPadChips = [] }
        try { core.mPadTargets = JSON.parse(padCtl.targetsJson) || ({ rooms: {}, names: {} }) } catch (e) { core.mPadTargets = ({ rooms: {}, names: {} }) }
        try { core.mPadRemote = JSON.parse(padCtl.remoteJson) } catch (e) { core.mPadRemote = null }
        try { core.mPadSounds = JSON.parse(padCtl.soundsJson) || ({}) } catch (e) { core.mPadSounds = ({}) }
    }
    Connections {
        target: padCtl
        function onChanged() { core._mirrorPad() }
        // the first Pad ever from a device: one toast (DESIGN §1 "On the receiver")
        function onFirstPad(device, name) {
            core.notifyAction(String(name) + " is now a Pad for this call", 8000, "Settings", function () { if (core.shell && core.shell.openCallSettings) core.shell.openCallSettings() })
        }
    }
    // discovery: my other devices in the calls around me, every 3 s while signed in (local store only)
    Timer {
        interval: Motion.focused ? 3000 : 10000; repeat: true; triggeredOnStart: true
        running: core.mLoggedIn
        onTriggered: { padCtl.refreshTargets(); core._mirrorPad() }
    }
    // One-shot startup update check, a few seconds after we're logged in (non-blocking;
    // fails silently to "up to date / check failed" if no manifest is hosted yet).
    Timer {
        id: updateCheckTimer
        interval: 4000
        repeat: false
        property bool done: false
        running: core.mLoggedIn && !done
        onTriggered: { done = true; updater.checkForUpdates() }
    }
    // Parsed once here rather than per bubble: every video bubble binds to this and
    // only the one whose id matches reacts.
    property var mResolvedMedia: ({})
    Connections {
        target: room
        function onMediaResolvedChanged() {
            try { core.mResolvedMedia = JSON.parse(room.mediaResolved) }
            catch (e) { core.mResolvedMedia = ({}) }
        }
    }
    // A resolved video/file/audio (room.openMedia) opens in the system handler from ConversationView's own handler
    // only: the main view's controller is this `room`, so a second handler here opened every attachment twice (gate 6,
    // the old double "Open"; sec-night 2026-10-27). tools/link-gate-test.mjs checks there is exactly one.
    Connections {
        target: settings
        function onBugReportStatusChanged() {
            if (settings.bugReportStatus.length > 0) {
                var failed = settings.bugReportStatus.indexOf("Report failed") === 0
                core.notify(settings.bugReportStatus + (failed ? " — your note is kept; press the bug button to try again." : ""), failed ? 12000 : 6000)
            }
        }
    }
    // Re-apply saved per-participant volumes after join: tracks subscribe asynchronously,
    // so nudge a few times over the first few seconds so a remembered level lands on the
    // track once it exists (each call is a NO-OP for not-yet-subscribed tracks).
    Timer {
        id: reapplyPrefsTimer
        interval: 1500; repeat: true
        property int ticks: 0
        onTriggered: {
            audioPrefsCtl.reapplyAll()
            ticks += 1
            if (ticks >= 4 || !core.mInCall) { ticks = 0; stop() }
        }
        onRunningChanged: if (running) ticks = 0
    }
    // ---- Mirrored controller state (see file header) ----
    property bool mLoggedIn: false
    property bool mVerified: false
    property bool mBusy: false
    property string mStatus: ""
    property string mResetStage: ""
    property string mResetStatus: ""
    property string mResetMode: ""   // selfhost (password.rs): "" | "checking" | "email" | "owner"
    property string mResetNote: ""
    property string mRegisterStage: ""     // 10.4: "" | "created"
    property string mRegisterStatus: ""
    property string mUserId: ""
    property var mRooms: []
    property string _roomsRaw: ""
    property string mRoomName: ""
    // Game + join code now live on the SYNCED activity state event (core.mActivity.game /
    // .code), set free-text from the call-bar game chip — no more hardcoded game list or the
    // local-only `dev.tinhat.game.code` timeline event.
    // 9.3: per-room chat memory {roomId: {draft, replyToId, replyToBody, atBottom, topId}}.
    property var chatMemory: ({})
    // 10.1 (TH-CONV-22): drafts + reply targets survive a restart — a debounced JSON blob of the
    // rooms that have one (staged files and scroll positions stay per-run).
    Timer { id: chatMemoryTimer; interval: 1500; onTriggered: core._writeChatMemory() }
    function persistChatMemory() { chatMemoryTimer.restart() }
    function _writeChatMemory() {
        var out = {}, n = 0
        for (var k in core.chatMemory) {
            var m = core.chatMemory[k]
            if (!m) continue
            if ((m.draft && m.draft.length > 0) || (m.replyToId && m.replyToId.length > 0)) {
                out[k] = { draft: m.draft || "", replyToId: m.replyToId || "", replyToBody: m.replyToBody || "" }
                if (++n >= 40) break
            }
        }
        settings.applyUiBlob("chat-drafts", JSON.stringify(out))
    }
    function restoreChatMemory() {
        try {
            var raw = settings.uiBlob("chat-drafts")
            if (!raw || raw.length === 0) return
            var o = JSON.parse(raw)
            for (var k in o) if (!core.chatMemory[k]) core.chatMemory[k] = o[k]
        } catch (e) { console.warn("[chat] drafts restore failed: " + e) }
    }
    property var mTyping: []
    property string _typingRaw: ""
    property bool mLoadingOlder: false
    property bool mHasMore: false
    property bool mRoomBusy: false
    property bool mCallActive: false
    property string mRoomStatus: ""
    property bool mSharing: false
    property bool mPreparing: false
    property string mStreamStatus: ""
    property bool mInCall: false
    // Leaving a call clears only the SESSION hide layer. The memory (hiddenKeys)
    // stays — 0.8.7's whole point — and the "N hidden ▾" chip + the hide
    // notification keep a remembered hide from ever being a silent surprise.
    onMInCallChanged: if (!mInCall && core.mHiddenTiles.length > 0) core.mHiddenTiles = []
    // 11.4 (Media Party): follow the call's room's party while in a call (the call bar's cue + strip); stop after.
    onMCallRoomIdChanged: watchCtl.watchRoom(mInCall ? mCallRoomId : "")
    // Screen-share quality lists — ON ROOT, not inside sqMenu: the Settings → Calls
    // pane's combos referenced sqMenu.* across the popup boundary and rendered BLANK
    // models there (a 0.3.3 screenshot); root-level data resolves everywhere.
    // w:0 = Auto (fit connection) — resolution follows the measured grant.
    readonly property var shareResList: [
        { label: "Auto — fit my connection", w: 0, h: 0, base: 12000 },
        { label: "720p",             w: 1280, h: 720,  base: 4000 },
        { label: "1080p",            w: 1920, h: 1080, base: 8000 },
        { label: "1440p",            w: 2560, h: 1440, base: 12000 },
        { label: "2160p (4K)",       w: 3840, h: 2160, base: 16000 },
        { label: "Source (native)",  w: 4096, h: 4096, base: 18000 }
    ]
    readonly property var shareBwList: [
        { label: "Good — lightest",        f: 0.6 },
        { label: "Great — balanced",       f: 1.0 },
        { label: "Best — most bandwidth",  f: 1.5 },
        // Publisher-side safe (congestion control + audio priority pacing); the trade
        // is weak-DOWNLINK viewers of the single-layer share (their video stalls).
        { label: "Unleashed — LAN-grade",  f: 2.5 }
    ]
    function shareCurRes() {
        for (var i = 0; i < shareResList.length; i++)
            if (shareResList[i].w === _sqW() && shareResList[i].h === _sqH()) return shareResList[i]
        return shareResList[2] // unknown stored size -> treat as 1080p
    }
    function shareCurTier() {
        var base = shareCurRes().base
        var best = 1, bestD = 1e12
        for (var i = 0; i < shareBwList.length; i++) {
            var d = Math.abs(_sqKbps() - Math.round(base * shareBwList[i].f))
            if (d < bestD) { bestD = d; best = i }
        }
        return best
    }
    property bool mConnecting: false
    property bool mCallSharing: false
    property bool mMicMuted: false
    property string mCallStatus: ""
    property var mOutputs: []
    property string _outputsRaw: ""
    property string mOutputName: ""
    property var mInputs: []
    property string _inputsRaw: ""
    property string mInputName: ""
    property bool mCameraOn: false
    property var mCameras: []
    property string _camerasRaw: ""
    property string mCameraName: ""
    property string mShareQuality: ""   // "maxW maxH fps kbps"
    property string mCamQuality: ""     // "W H FPS KBPS SIM01" camera publish quality
    property string mRecvQuality: ""    // "auto"|"high"|"medium"|"low" incoming-video cap
    // 10.8.1 (GPU.md §1 amendment): Settings → Appearance → Performance → "Save GPU while Tin Hat isn't focused" — the
    // master switch of every unfocused rule (it was Calls-only, on by default, in 10.8.0). Pref `call-unfocused-saver`
    // (the 10.8.0 key: a stored "1" / "0" is the user's own choice), default OFF (lowgpu unfocused_saver_on).
    property bool mCallUnfocusedSaver: false
    onMCallUnfocusedSaverChanged: { Qt.callLater(core.logWinState); core._applyVisibilityPause() }
    property bool mMusicPlaying: false  // #7 music 2nd-track live
    property bool mNcAvailable: true    // RNNoise element installed? false => the toggle can do nothing
    property bool mEcDegraded: false    // EC wanted but not running (peers hear your echo)
    property string mEcNote: ""        // 0.8.31: what the canceller is doing, measured
    property var mEcOutputs: []         // 0.8.33: speaker outputs summed into the echo reference ([] = Tin Hat's own)
    property string _ecOutputsRaw: ""
    property bool mMicDenoise: false    // #7 RNNoise mic noise-cancel (default OFF)
    property bool mMicAec: true         // acoustic echo cancellation (default ON)
    property bool mMicAgc: false        // libwebrtc auto-gain-control (SEPARATE; default OFF)
    property string mMicNsMode: "off"   // 0.8.25: "off" | "standard" (WebRTC NS) | "strong" (RNNoise)
    property string mAecEngine: ""      // 0.8.25: "webrtc-dsp" (in-app) | "pipewire-module" (legacy)
    property bool mMicSilent: false     // 0.8.25: the live mic delivered only digital silence for 10 s
    property bool mOutputMissing: false // 0.8.25: no usable output device at join (PipeWire dummy output)
    property int mLogship: -1              // -1 never asked (popup), 0 off, 1 on — crate::logship
    property bool mOverlayEnabled: false   // voice overlay HUD (default OFF; see OverlayWindow.qml)
    property string mOverlayMode: "speaking" // "speaking" (pills only while talking) | "always"
    property string mOverlayCorner: "tl"   // "tl"|"tr"|"bl"|"br"
    property bool mOverlayHideShare: false // hide the HUD while screen-sharing
    property bool mOverlayDemo: false      // TINHAT_OVERLAY_DEMO=1 pins the preview pills
    property bool mReconnecting: false  // SFU connection dropped, auto-reconnecting
    property bool mCallFailed: false    // call ended unexpectedly (terminal disconnect)
    property string mCallFailReason: "" // 10.1: why the last JOIN failed ("" = a mid-call drop)
    property var mAudioSources: []      // #1 shareable audio sources [{node,label}]
    property string _audioSourcesRaw: ""
    property string mMusicSourceName: "" // #1 persisted music share-source node
    property var mAudioApps: []         // 0.8.22 per-app share sources [{id,label,detail,active}]
    property string _audioAppsRaw: ""
    property bool mShareRecOnly: false  // our share is "for the recording only"
    property var mRecOnlyFlags: ({})    // {tileId: [recOnly, tunedIn]} for peers' recording-only shares
    property string _recOnlyRaw: ""
    property bool mGameChat: false       // 0.8.23: I am in the game's own voice chat
    // 9.4 corners ("pull aside"): mine = my corner id ("" = none; set while waiting for the
    // answer too), active = a peer shares it, members = the peers in it, corners = every ACTIVE
    // corner {id: [identities, me included]}. Full LiveKit identities throughout.
    property var mCorner: ({ mine: "", active: false, members: [], corners: {} })
    property bool _cornerLocal: false   // 10.1: the next corner edge was started here (no "pulled you aside" toast)
    property string _cornerRaw: ""
    readonly property bool mAside: mCorner.mine.length > 0
    property var mGameChatPeers: ({ me: false, peers: [], muted: [] }) // full LiveKit identities
    property string _gameChatRaw: ""
    property string mCallRoomId: ""      // the room whose call we are in (SFU-truth strip)
    property double _navPartAt: 0       // 11.0: time-based (the mirror slows to 1 s when not focused)
    property var listenTiles: []         // share tiles explicitly listened to (session only)
    property int _pipsRev: 0             // bumped on pop-out open/close (_pips itself never notifies)
    property bool mLocalAfk: false      // #7 we are BRB/AFK in the call
    property bool _afkApplied: false    // #4 last mLocalAfk we pushed into the incoming-cut
    property bool _afkMutedMic: false   // #7 AFK (not the user) muted the mic; restore on un-BRB
    property var mAwayUsers: []         // #7 userIds currently away in the call
    property string _awayRaw: "[]"
    property double _afkPollAt: 0
    property var mVideoTiles: []
    property string _videoTilesRaw: ""
    // #B live audio levels: a map keyed "user|category" -> rms (0.0..1.0), plus the local
    // mic RMS, driving the small per-name waveform bars on each call tile.
    property var mAudioLevels: ({})
    property string _audioLevelsRaw: ""
    // SFU active-speaker list {me, active:[identities]} — server-side voice detection
    // (drives the overlay pills; robust where an RMS floor is not, e.g. AGC-boosted rooms).
    property var mSpeakers: ({ me: "", active: [] })
    property string _speakersRaw: ""
    // 10.5 "can they hear me?" — the streamer's measured view (hear.rs): mic check, talking while
    // muted, "not coming through", who receives us ("heard"), what we receive from each person,
    // echo (theirs through us, ours through them, and our own speakers into our mic).
    property var mHear: ({})
    property string _hearRaw: ""
    // 11.3 same room (mirror of callctl.sameRoomJson): {linked[],holder,live,ask{from,at}|null,pending,spk,groups{id:{m,mic}},rawOk}
    property var mSameRoom: ({ linked: [], holder: "", live: true, ask: null, pending: "", spk: true, groups: {}, rawOk: false })
    property string _sameRoomRaw: ""
    readonly property var _sameRoomIdle: ({ linked: [], holder: "", live: true, ask: null, pending: "", spk: true, groups: {}, rawOk: false })
    // Outgoing stream health: [{kind,enc,encPick,rid,w,h,fps,targetKbps,sendKbps,qlim,
    // lossPct,rttMs,jitterMs,key,nack,pli,active}] — one entry per live outgoing stream
    // ("screen", "camera/<rid>" per simulcast rung, "mic"). Refreshed by the streamer
    // every 3s; drives the call bar's Stream health popup + status dot.
    property var mPublishStats: []
    property string _publishStatsRaw: ""
    property real mMicLevel: 0.0
    property real mMicLevelPre: 0.0     // PRE-denoise mic RMS (noise-cancel before/after meter)
    property real mOutputLevel: 0.0     // speaker/sink RMS (settings Audio-pane speaker meter)
    property bool mDeafened: false      // all incoming call audio muted (output deafen)
    property bool mHangingUp: false     // leave teardown in progress (rejoin blocked)
    // "Fill the app" call mode: the call view takes over the whole window content area
    // (nav + timeline + composer hidden); NOT OS fullscreen. Toggled from the call status bar.
    property bool callMaximized: false
    // Call-grid preset (persisted via settings), pinned tile, side-column split,
    // and the animation toggle. See videoGrid.weightedLayout for what each preset does.
    property string callLayout: "auto"     // "auto" | "gallery" | "speaker" | "sidebar"
    property string pinnedTile: ""         // tile id forced into the main area ("" = none)
    property real callSideFrac: 0.28       // side column's share of the grid width (seam drag)
    // 9.3.1: the seam between the call and the chat (Legacy). callPaneFrac = the call panel's
    // share of the page height once the divider has been dragged (-1 = never: the grid keeps its
    // natural height, capped so the chat keeps a quarter of the page); callCollapsed = the tiles
    // are tucked away behind the status + control bars so the chat has the room. Both persist.
    property real callPaneFrac: -1
    property bool callCollapsed: false
    function setCallPaneFrac(f) {
        core.callPaneFrac = f
        settings.applyUiPref("call-pane-frac", f < 0 ? "" : String(Math.round(f * 1000) / 1000))
    }
    function setCallCollapsed(on) {
        core.callCollapsed = on
        settings.applyUiPref("call-collapsed", on ? "1" : "0")
    }
    // 11.0.2 (2026-09-28, ACTIONS.md §15): "Keep call buttons visible" — the call bar's pin, Settings → Calls and
    // the action call.bar.pin.toggle. Pinned, the call bar stays shown in a call; otherwise it hides 3 s after the pointer
    // stops (Main.qml callChromeShown). Device-local pref `call-bar-pinned` ("1" = on, anything else = off; default off —
    // the web's is the same key). --tinhat-callpin (the gates) pins it for the run only, nothing written.
    property bool callBarPinned: false
    function setCallBarPinned(on) {
        core.callBarPinned = on
        settings.applyUiPref("call-bar-pinned", on ? "1" : "0")
    }
    property bool callAnimate: true
    function setCallLayout(m) { settings.applyCallLayout(m); core.callLayout = m }
    // Right-hand panel: joined members + room summary (parsed from RoomController).
    property var mMembers: []
    property string _membersRaw: ""
    property var mRoomSummary: ({})
    property string _roomSummaryRaw: ""
    // Room-settings/moderation permissions (mirrored from RoomController).
    property int mMyPower: 0
    property bool mCanChangeName: false
    property bool mCanChangeTopic: false
    property bool mCanInvite: false
    property bool mCanKick: false
    property bool mCanBan: false
    property bool mCanChangePower: false
    property string mModError: ""   // last failed moderation write (toast once when set)
    // mxid whose profile flyout is open (avatar click / member-row tap).
    property string profileMxid: ""
    // Auto-update state (mirrored from UpdateController).
    property string mAppVersion: ""
    // Shared across all three clients; see UNIVERSAL.md.
    property string mUniversalVersion: ""
    property string mBuildHash: ""
    property string mLatestVersion: ""
    property bool mUpdateAvailable: false
    property bool mUpdateDownloading: false
    property bool mUpdateReady: false
    property bool mCanSelfUpdate: true   // false = pacman/system-managed → notify-only
    // Platform-correct manual-update advice for !canSelfUpdate installs. The old
    // hardcoded "sudo pacman -Syu" was shown to a Windows user running from a zip
    // extracted into Downloads (2026-08-20) — Linux package advice on Windows.
    // 10.0: the updater knows the channel (Flatpak / final pacman / bundle) — one hint per channel.
    readonly property string manualUpdateHint: updater.manualUpdateHint
    // 0.8.16 Windows install experience (mirrored from UpdateController)
    property bool mPortableRun: false
    property string mInstalledVersion: ""
    property bool mInstallPromptDue: false
    property bool mInstalling: false
    function verNewerOrSame(a, b) { // "0.8.16" >= "0.8.15"
        var pa = String(a).split("."), pb = String(b).split(".")
        for (var i = 0; i < Math.max(pa.length, pb.length); i++) {
            var x = parseInt(pa[i] || "0"), y = parseInt(pb[i] || "0")
            if (x !== y) return x > y
        }
        return true
    }
    property string mUpdateStatus: ""
    // A failed apply used to only park its reason in the settings pane while the
    // banner silently re-armed — the 0.3.0 rollout looped "Update now" with zero
    // feedback. Surface failures where the user is looking.
    onMUpdateStatusChanged: if (mUpdateStatus.indexOf("Update failed") === 0)
                                core.notify(mUpdateStatus, 8000)
    property string mReleaseNotes: ""
    // Gaming: room activity + per-person status (mirrored from activityCtl/statusCtl).
    property var mActivity: ({})
    property string _activityRaw: ""
    property var mStatuses: []
    property string _statusesRaw: ""
    property string mMyStatus: ""
    property bool mStatusIsDm: false
    property string mActivityError: ""   // last failed activity write (toast once when set)
    property string mStatusError: ""     // last failed status write (toast once when set)
    property double _gamePollAt: 0   // 11.0: time-based (the mirror slows to 1 s when not focused)
    // GIF picker (mirrored from GifController).
    property var mGifs: []
    property string _gifsRaw: ""
    property bool mGifLoading: false
    property string mGifError: ""

    // Soundboard (mirrored from SoundboardController).
    property var mRoomSounds: []
    property string _roomSoundsRaw: ""
    property var mPersonalSounds: []
    property string _personalSoundsRaw: ""
    property var mSoundSearch: []
    property string _soundSearchRaw: ""
    property bool mSoundSearching: false
    property string mSoundError: ""
    property bool mSoundCallPlaying: false        // the call's "sound" track is live (0.8.21)
    property bool mSoundUploading: false          // own-clip upload in flight (0.8.22)
    property var mSbRecent: []                    // last played sounds [{name,url,emoji}] (0.8.22)
    property var mJoinSoundSearch: []
    property string _joinSoundSearchRaw: ""
    property bool mJoinSoundSearching: false

    // Watch party (mirrored from WatchPartyController).
    property bool mWatchInParty: false
    property bool mWatchAdvancer: false
    property var mWatchParty: ({ queue: [], hostId: "" })
    property string _watchPartyRaw: ""
    property var mWatchPlayback: ({})
    property string _watchPlaybackRaw: ""
    property var mWatchViewers: []
    property string _watchViewersRaw: ""
    property var mWatchSearch: []
    property string _watchSearchRaw: ""
    property bool mWatchSearching: false
    property string mWatchError: ""
    // 11.4 (Media Party): the party in the CALL's room, whether or not I'm in it — the call bar's button dot + strip.
    // `{ live, viewers, url, playing, item:{url,title,addedBy,art?,artist?,kind?} }`, polled by watchCtl.watchRoom.
    property var mCallParty: ({ live: false, viewers: 0, url: "", playing: false, item: ({}) })
    property string _callPartyRaw: ""
    // 11.4: the one box's last addLink outcome + in-flight flag (watchCtl.linkJson / adding).
    property var mWatchLink: ({})
    property string _watchLinkRaw: ""
    property bool mWatchAdding: false
    // 11.4 Browse pane: the last browse() answer, in flight, suggestions, the room's History.
    property var mWatchBrowse: ({})
    property string _watchBrowseRaw: ""
    property bool mWatchBrowsing: false
    property var mWatchSuggest: []
    property string _watchSuggestRaw: ""
    property var mWatchHistory: []
    property string _watchHistoryRaw: ""

    // Memories / DVR (0.8.8) — mirrored from RoomController while the window is open.
    property var mMemories: []
    property string mMemoriesError: ""   // last fetch failure (list kept; the window shows a retry)
    property string _memoriesRaw: ""
    property var mClips: []
    property string _clipsRaw: ""
    Timer {
        interval: Motion.focused ? 400 : 1000; repeat: true   // 11.0 (GPU.md §2.8): ≥ 1 s while not focused
        running: core.memoriesWanted
        onTriggered: {
            if (room.memoriesJson !== _memoriesRaw) {
                _memoriesRaw = room.memoriesJson
                mMemoriesError = ""
                try { var pe = JSON.parse(_memoriesRaw); if (pe && pe.error) mMemoriesError = String(pe.error) } catch (e) {}
                try {
                    var p = _memoriesRaw.length > 0 ? JSON.parse(_memoriesRaw) : {}
                    mMemories = (p && p.sessions) ? p.sessions : []
                } catch (e) { mMemories = [] }
            }
            if (room.clipsJson !== _clipsRaw) {
                _clipsRaw = room.clipsJson
                try { mClips = _clipsRaw.length > 0 ? JSON.parse(_clipsRaw) : [] } catch (e) { mClips = [] }
            }
        }
    }

    // Corner-dock navigation (mirrored from NavController).
    property var mNavSpaces: []
    property string _navSpacesRaw: ""
    property var mNavRooms: []
    // My own photo, for the sidebar footer. Mirrored from settings so ThRoomList can
    // bind it — `settings` is an id in THIS document and is not visible inside that one.
    property string mMyAvatar: ""
    property bool _myAvatarAsked: false
    property string _navRoomsRaw: ""
    property string mNavActiveSpace: "home"
    property string mNavActiveLabel: "Home"
    property string mNavBadge: ""
    // 10.6 (REACH §1): the rail's DM strip {entries:[{id,name,avatar,peer,unread,highlight,invite,recency}], overflow}.
    property var mNavDmStrip: ({ entries: [], overflow: 0 })
    property string _navDmStripRaw: ""

    // 10.6 (REACH §2 + A4): presence, mirrored from PresenceController. mPresence = mxid →
    // {state, room?, dot, label, chip?, chipColor?} (the view is computed in Rust, presence_model.rs).
    property var mPresence: ({})
    property int _presenceRev: -1
    property bool mPresenceKnown: false
    property bool mPresenceSharing: false
    property bool mPresenceAsked: false
    property bool mPresenceTurningOff: false
    property bool mPresenceBusy: false
    property string mPresenceMine: ""        // what others see of me (A2.15): online | away | call | ""
    property string mPresenceMyRoom: ""
    property var mMyPresenceStatus: null     // A4: my custom status as set {id,text,emoji,color,until} (the picker) — not mMyStatus (the room "looking to play" one)
    property var mStatusPresets: []          // A4.2 [{id,emoji,text,color,hex}] — constant, from presence_model.rs
    property var mStatusPalette: []          // A4.3 [{key,hex}]
    property string _myStatusRaw: ""
    property int _presenceErrRev: 0
    // What a person's presence reads, or null (absent / not sharing = no dot).
    function presenceOf(mxid) {
        var m = core.mPresence
        return (mxid && m && m[mxid]) ? m[mxid] : null
    }
    function presenceWord(state) {
        return state === "away" ? "Away" : (state === "call" ? "In a call" : (state === "online" ? "Online" : ""))
    }
    // A room's display name from the room list ("" when we are not in it).
    function roomNameOf(id) {
        var rs = core.mRooms || []
        for (var i = 0; i < rs.length; i++) if (rs[i] && rs[i].id === id) return String(rs[i].name || "")
        return ""
    }
    // The call we are connected to, for presence beats ("" = none) — pushed on change.
    readonly property string presenceCallRoom: core.mInCall ? core.mCallRoomId : ""
    onPresenceCallRoomChanged: presenceCtl.setCallRoom(core.presenceCallRoom)
    // §2.5 cadence: 20 s while the window is shown, 60 s otherwise, at once on focus.
    readonly property bool presenceWinVisible: !!core.shell && core.shell.visible === true
        && core.shell.visibility !== Window.Minimized && core.shell.visibility !== Window.Hidden
    readonly property bool presenceWinFocused: !!core.shell && core.shell.active === true
    onPresenceWinVisibleChanged: presenceCtl.setWindowState(core.presenceWinVisible, core.presenceWinFocused)
    onPresenceWinFocusedChanged: presenceCtl.setWindowState(core.presenceWinVisible, core.presenceWinFocused)

    // ── 11.0 (GPU.md §1, R1/R2): the app-wide window state + the motion switch → the Motion singleton ──
    // focused = the main window or a pop-out of ours (PiP, calendar, Memories …) is the active window
    // (Qt.application.state); hidden = the main window is not shown or not EXPOSED — its `thExposed`, set by the C++
    // input filter from real expose events (KWin Wayland never reports a minimise); unfocused otherwise. Reduce motion
    // = the app's switch OR the platform's (Windows "Animation effects" off; KDE animation speed "Instant", which
    // makes Kirigami's durations 0). Every rule below the window keys off Motion (lowgpu_ctl.rs = the web's lowgpu.ts).
    // 10.8.1 (GPU.md §1 amendment): TWO states. `realWinState` = what the window really is — the incoming-call ring reads
    // it (Motion.realWinState); the call chrome and read receipts / notifications read Window.active themselves.
    // `winState` = the RENDER state every low-GPU rule acts on (Motion.winState / focused / animate, the mirror below,
    // the call's receive policy, widgets and stats): an unfocused window renders as focused unless Settings → Appearance
    // → Performance → "Save GPU while Tin Hat isn't focused" is on (default off); hidden stays hidden either way.
    readonly property bool appActive: Qt.application.state === Qt.ApplicationActive
    // 10.8.1: "not exposed" counts as hidden once it has lasted 2 s; exposed again = at once. Qt's Wayland plugin also
    // reports a window not exposed when a frame callback is 100 ms late (a compositor busy with a game can do that to a
    // window that is on screen); KWin's real "suspended" (minimised, another desktop / activity, Show Desktop, screen
    // locked, DPMS off — never merely unfocused or covered) stays unexposed. Minimized / Hidden (X11, Windows) stay
    // immediate.
    readonly property bool mainExposedNow: !!core.shell && core.shell.thExposed !== false
    property bool mainUnexposedLong: false
    onMainExposedNowChanged: { if (core.mainExposedNow) { unexposedSettle.stop(); core.mainUnexposedLong = false } else unexposedSettle.restart() }
    Timer { id: unexposedSettle; interval: 2000; onTriggered: core.mainUnexposedLong = !core.mainExposedNow }
    readonly property bool mainShown: !!core.shell && core.shell.visible === true && !core.mainUnexposedLong
        && core.shell.visibility !== Window.Minimized && core.shell.visibility !== Window.Hidden
    readonly property string realWinState: lowgpuCtl.computeWinState(core.mainShown, core.appActive)
    readonly property string winState: lowgpuCtl.computeRenderState(core.mainShown, core.appActive, core.mCallUnfocusedSaver)
    readonly property bool platformReducedMotion: lowgpuCtl.platformReducedMotion || Kirigami.Units.longDuration <= 1
    readonly property bool reducedMotion: core.mReducedMotion || core.platformReducedMotion
    readonly property bool animate: lowgpuCtl.animateFor(core.winState, core.reducedMotion)
    // One "[winstate]" line per change of the real state, the render state or the option (lowgpu win_state_log_text —
    // the web logs the same line), e.g. "[winstate] focused → unfocused (render: focused, option off)". Coalesced with
    // Qt.callLater and computed at the source: a handler can run before the bindings above have re-read their inputs.
    property string _winLogged: ""
    property string _winLoggedReal: ""
    function logWinState() {
        var real = lowgpuCtl.computeWinState(core.mainShown, core.appActive)
        var render = lowgpuCtl.computeRenderState(core.mainShown, core.appActive, core.mCallUnfocusedSaver)
        var key = real + "|" + render + "|" + core.mCallUnfocusedSaver
        if (key === core._winLogged) return
        console.log("[winstate] " + lowgpuCtl.winStateLogText(core._winLoggedReal.length > 0 ? core._winLoggedReal : real, real, render, core.mCallUnfocusedSaver))
        core._winLogged = key
        core._winLoggedReal = real
    }
    onRealWinStateChanged: Qt.callLater(core.logWinState)
    onWinStateChanged: { Qt.callLater(core.logWinState); core._applyVisibilityPause(); gameServersCtl.setWindow(core.winState) }
    onReducedMotionChanged: lowgpuCtl.setCaretReduced(core.reducedMotion)   // R5: a steady caret under Reduce motion
    Binding { target: Motion; property: "winState"; value: core.winState }
    Binding { target: Motion; property: "realWinState"; value: core.realWinState }
    Binding { target: Motion; property: "reducedMotion"; value: core.reducedMotion }
    Binding { target: Motion; property: "animate"; value: core.animate }
    Binding { target: Motion; property: "stepDur"; value: lowgpuCtl.durMs("dur") }
    Binding { target: Motion; property: "stepShort"; value: lowgpuCtl.durMs("short") }
    Binding { target: Motion; property: "stepSlow"; value: lowgpuCtl.durMs("slow") }
    Binding { target: Motion; property: "stepLong"; value: lowgpuCtl.durMs("long") }
    Binding { target: Motion; property: "stepVeryLong"; value: lowgpuCtl.durMs("verylong") }

    // Squad calendar (0.8.26) — mirrored from CalendarController.
    property var mCalEvents: []
    property string _calEventsRaw: ""
    property var mCalGames: []
    property string _calGamesRaw: ""
    property var mCalUpcoming: []
    property string _calUpcomingRaw: ""
    property var mCalContainers: []
    property string _calContainersRaw: ""
    property string mCalContainer: ""
    property string mCalContainerLabel: ""
    property bool mCalCanEdit: false
    property bool mCalIsMod: false
    property string mCalMyId: ""
    property bool mCalBusy: false
    property bool mCalReminders: true
    property string mCalError: ""
    property var mCalMissing: []          // 0.8.27
    property string _calMissingRaw: ""
    property bool mCalCanInvite: false
    property string mCalUnjoinedParent: ""
    property string mCalSourceLabel: ""
    property string _calRoom: ""
    property bool _calStarted: false
    property bool _gamesStarted: false
    property string _gamesRoom: ""
    onLiveCallPartsChanged: gamesCtl.setPresent(JSON.stringify(liveCallParts.map(function (p) { return p.mxid })))
    property bool _calAutoOpened: false
    // ---- RBAC (docs/RBAC.md §5.3) — mirrored from SpaceController. The JSON models are parsed
    // ONCE per `rev` (the controller bumps it on any data change); busy/spaceId are cheap scalars.
    property string mSpaceId: ""
    property bool mSpaceBusy: false
    property int mSpaceRev: -1
    property var mSpaceSummary: ({})
    property var mSpaceMembers: []
    property var mSpaceRoles: []
    property var mSpacePerms: ({})
    property var mSpaceRooms: []
    property var mSpaceUserSearch: []
    property bool mSpaceRolesActive: false
    property int mSpaceMyRank: 0
    property var mPermissionCatalog: []
    property string _permissionCatalogRaw: ""
    // status / lastError travel with a revision so the same sentence toasts twice.
    property string mSpaceStatus: ""
    property int mSpaceStatusRev: 0
    property string mSpaceError: ""
    property int mSpaceErrorRev: 0
    // ---- 10.4 invite codes (docs/INVITES.md) — mirrored from InviteController.
    property bool mInviteOperator: false   // the signed-in account may mint codes (Settings shows Invites)
    property bool mInviteBusy: false
    property int mInviteRev: -1
    property var mInvites: []              // [{code,usesAllowed,pending,completed,expiresAt,active}]
    property string mInviteServer: ""
    // ---- selfhost: the guest room (docs/GUESTS.md) — mirrored from GuestsController (guests_ctl.rs view_json): THE
    // Guests room's id, its bar + waiting rows, Settings → Guests' plan, the link, the words — all decided in Rust.
    property int mGuestsRev: -1
    property var mGuests: ({ offered: false, guest: false, room: "", on: false, bar: null, rows: [], pane: null, link: "", note: "", noteError: false, busy: false, words: {} })
    property bool _guestsStarted: false
    property bool mGuestsHere: false       // the open room is THE Guests room (its bar shows: HeaderIsland.qml)
    // selfhost (server_offer.rs; the web's useServerFeature): what the user's server offers — {server, api,
    // features: {calls, soundboard, gifs, watch, games, steam, gameservers, presence, push, memories, reports, guests},
    // recorder, guest}. Only an explicit false turns a feature off; off = hidden, never dimmed; a temporary guest
    // account (guest: true) never gets the gates' guestWithheld (reports). Mirrored from login.
    property var mServerOffer: ({ server: "", api: "", features: {}, recorder: "", guest: false })
    property string _serverOfferRaw: ""
    readonly property var serverGates: {
        try { return JSON.parse(String(login.serverGatesJson())) } catch (e) { return ({ features: [], actions: [], settings: {}, widgets: {}, permissions: {}, panes: {}, retired: [], retiredPredicate: "selfhost.retired", guestWithheld: [] }) }
    }
    function serverOn(f) {
        var o = core.mServerOffer || {}
        var fs = o.features || {}
        if (fs[f] === false) return false
        return !(o.guest === true && (core.serverGates.guestWithheld || []).indexOf(f) >= 0)
    }
    function widgetOffered(id) { var g = core.serverGates.widgets[id]; return !g || core.serverOn(g) }
    function permOffered(key) { var g = core.serverGates.permissions[key]; return !g || core.serverOn(g) }
    function settingOffered(id) { var g = core.serverGates.settings[id]; return !g || core.serverOn(g) }
    function paneOffered(key) { var g = (core.serverGates.panes || {})[key]; return !g || core.serverOn(g) }
    // the actions snapshot's server predicates (offer.ts predicateValues): one per feature + the retired actions' (false)
    function serverPredicates() {
        var v = {}
        var fs = core.serverGates.features || []
        for (var i = 0; i < fs.length; ++i) v["server." + fs[i]] = core.serverOn(fs[i])
        v[core.serverGates.retiredPredicate || "selfhost.retired"] = false
        return v
    }
    // the Memories recorder the server announces (offer.ts isRecorderId): the account, or "<account>:<device>"
    function isRecorder(id) {
        var r = String((core.mServerOffer && core.mServerOffer.recorder) || ""), s = String(id || "")
        return r.length > 0 && s.length > 0 && (s === r || s.indexOf(r + ":") === 0)
    }
    // the signed-in account is one of the server's temporary guest accounts (offer.ts isGuestAccount)
    readonly property bool mGuestAccount: !!core.mServerOffer && core.mServerOffer.guest === true
    // the window came back (the web's visibilitychange): ask again, at most every 10 min
    onAppActiveChanged: if (appActive && login.loggedIn) login.refreshServerOffer()
    property int mInviteStatusRev: 0
    property int mInviteErrorRev: 0
    property bool _invitesProbed: false
    // The space that owns the OPEN room ("" = none) — RightPanel's gear + "Manage roles…".
    property string mRoomSpaceId: ""
    // ---- RBAC §5.4 — call moderation, mirrored from CallController.
    property var mCallPerms: ({})          // {"kick":true,…} my effective permissions in the call room
    property string _callPermsRaw: ""
    property bool mCanModerateCall: false
    property bool mServerMuted: false      // a moderator muted MY mic for everyone
    property string mServerMutedBy: ""
    property string mRemovedBy: ""         // who disconnected me from the last call ("" = nobody)
    property int _moderationRev: -1
    property int _callErrorRev: 0
    // {identity: "<moderator name>"} for every tile whose mic a moderator muted (self-view included).
    property var mServerMutedMap: ({})
    function _deriveServerMutedMap() {
        var m = {}, tiles = core.mVideoTiles || []
        for (var i = 0; i < tiles.length; i++) {
            var t = tiles[i]
            if (t && t.serverMuted === true && t.participant) m[String(t.participant)] = String(t.serverMutedBy || "a moderator")
        }
        var a = Object.keys(m), b = Object.keys(core.mServerMutedMap || {})
        if (a.length === b.length && a.every(function (k) { return core.mServerMutedMap[k] === m[k] })) return
        core.mServerMutedMap = m
    }
    onMVideoTilesChanged: core._deriveServerMutedMap()
    // Who muted this person, by full identity (tiles) or by mxid — any device (the roster rows
    // only know the mxid). "" = not server-muted.
    function serverMutedByFor(identity, mxid) {
        var map = core.mServerMutedMap || {}
        if (identity && identity.length > 0 && map[identity] !== undefined) return String(map[identity])
        if (mxid && mxid.length > 0) {
            var pre = mxid + ":"
            for (var k in map) if (k.indexOf(pre) === 0) return String(map[k])
        }
        return ""
    }
    // The LiveKit identity of an mxid in the current call (first device seen), "" when absent.
    function identityForMxid(mxid) {
        if (!mxid || mxid.length === 0) return ""
        var tiles = core.mVideoTiles || []
        for (var i = 0; i < tiles.length; i++) {
            var t = tiles[i]
            if (!t || !t.participant || t.id === "vid_selfview" || t.id === "vid_selfshare") continue
            if (core.tileMxid(t.participant) === mxid) return String(t.participant)
        }
        return ""
    }
    // ---- RBAC §7.3 — space invites seen so far ({key: true}); null until the nav has produced
    // its first non-empty snapshot, so nothing toasts on the first poll after login.
    property var _spaceInviteKeys: null
    onMCalErrorChanged: if (mCalError.length > 0) core.notify(mCalError, 6000)
    // Icon pack (0.8.26) — {dir, path, name, theme, animated, icons:{name:{anim,trigger,variants}}} or null.
    property var mIconPack: null
    // Motion preference read by ThIcon (hover stories switch to their end pose instantly).
    property bool mReducedMotion: false
    property string _iconPackRaw: ""
    property bool _galleryOpened: false
    // The bundled app icon (sign-in card); assets are unpacked before the engine loads.
    readonly property string appIconUrl: "file://" + login.assetsPath + "/appicon/tinhat-256.png"
    // The controller mirror. 11.0 (GPU.md §2.8, R14): 120 ms while the window is focused, 1 s otherwise (it woke the
    // app ~8×/s forever, hidden or not); the 3 s sub-cadences below go by the clock, not by tick counts. (10.8.1: the
    // render state — an unfocused window keeps 120 ms unless "Save GPU while Tin Hat isn't focused" is on.)
    Timer {
        interval: lowgpuCtl.intervalFor(core.winState, 120, 1000, 1000)
        running: true
        repeat: true
        onTriggered: {
            mLoggedIn = login.loggedIn
            mVerified = login.verified
            mBusy = login.busy
            mStatus = login.status
            mUserId = login.userId
            mResetStage = login.resetStage
            mResetStatus = login.resetStatus
            mResetMode = login.resetMode
            mResetNote = login.resetNote
            mRegisterStage = login.registerStage
            mRegisterStatus = login.registerStatus
            // 0.8.26 icon pack — before the logged-in guard so the sign-in screen gets it too.
            if (iconPack.packJson !== _iconPackRaw) {
                _iconPackRaw = iconPack.packJson
                try { mIconPack = _iconPackRaw.length > 0 ? JSON.parse(_iconPackRaw) : null } catch (e) { mIconPack = null }
            }
            if (iconPack.galleryRequested && !_galleryOpened) { _galleryOpened = true; core.openIconGallery() }
            // selfhost: the guest room follows the session (docs/GUESTS.md) — its doc, the Guests room through sync;
            // before the logged-out guard, so a sign-out withdraws its notices and the next sign-in starts afresh.
            if (mLoggedIn !== core._guestsStarted) {
                core._guestsStarted = mLoggedIn
                if (mLoggedIn) guestsCtl.start(); else { guestsCtl.reset(); core.mGuestsHere = false }
            }
            // Logged-out (login screen): every room/call/nav getter below is empty/idle, so skip
            // them — no point marshaling the whole controller surface across cxx-qt 8x/sec.
            if (!mLoggedIn) return
            if (login.roomsJson !== _roomsRaw) {
                _roomsRaw = login.roomsJson
                mRooms = _roomsRaw.length > 0 ? JSON.parse(_roomsRaw) : []
                nav.refreshBadge() // cheap unread-only: keep the badge fresh
                // Full recompute when the nav is actually visible: always in persistent
                // (it's the sidebar), or only while the corner dock is open.
                if (core.navMode === "persistent" || core.navOpened) nav.refresh()
            }
            mRoomName = room.roomName
            mRoomBusy = room.busy
            mCallActive = room.callActive
            mRoomStatus = room.status
            mLoadingOlder = room.loadingOlder
            mHasMore = room.hasMore
            if (room.typingJson !== _typingRaw) {
                _typingRaw = room.typingJson
                mTyping = _typingRaw.length > 0 ? JSON.parse(_typingRaw) : []
            }
            if (room.membersJson !== _membersRaw) {
                _membersRaw = room.membersJson
                mMembers = _membersRaw.length > 0 ? JSON.parse(_membersRaw) : []
            }
            if (room.roomSummaryJson !== _roomSummaryRaw) {
                _roomSummaryRaw = room.roomSummaryJson
                mRoomSummary = _roomSummaryRaw.length > 0 ? JSON.parse(_roomSummaryRaw) : ({})
            }
            mMyPower = room.myPower
            mCanChangeName = room.canChangeName
            mCanChangeTopic = room.canChangeTopic
            mCanInvite = room.canInvite
            mCanKick = room.canKick
            mCanBan = room.canBan
            mCanChangePower = room.canChangePower
            // Surface moderation/settings write failures as a passive toast (once per change).
            if (room.modError !== mModError) {
                mModError = room.modError
                if (mModError.length > 0) core.notify(mModError)
            }
            mAppVersion = updater.appVersion
            mUniversalVersion = updater.universalVersion
            mBuildHash = updater.buildHash
            mLatestVersion = updater.latestVersion
            mUpdateAvailable = updater.updateAvailable
            mUpdateDownloading = updater.downloading
            mUpdateReady = updater.readyToRestart
            mCanSelfUpdate = updater.canSelfUpdate
            mPortableRun = updater.portableRun
            mInstalledVersion = updater.installedVersion
            mInstallPromptDue = updater.installPromptDue
            mInstalling = updater.installing
            mUpdateStatus = updater.status
            mReleaseNotes = updater.releaseNotes
            // Gaming controllers follow the open room; poll ~every 3s for others' changes
            // (no per-room state-event live hook yet — single sync loop).
            if (room.roomId !== activityCtl.roomId) {
                activityCtl.open(room.roomId)
                statusCtl.open(room.roomId)
            }
            // 11.0: every 3 s by the clock (the mirror's own period changes with the window state)
            var gpNow = Date.now()
            if (room.roomId.length > 0 && gpNow - _gamePollAt >= 3000) {
                _gamePollAt = gpNow
                activityCtl.refresh()
                statusCtl.refresh()
            }
            if (activityCtl.activityJson !== _activityRaw) {
                _activityRaw = activityCtl.activityJson
                mActivity = _activityRaw.length > 0 ? JSON.parse(_activityRaw) : ({})
                nav.refresh() // keep the nav "Now playing" line in sync promptly (not just on poll)
            }
            if (statusCtl.statusesJson !== _statusesRaw) {
                _statusesRaw = statusCtl.statusesJson
                mStatuses = _statusesRaw.length > 0 ? JSON.parse(_statusesRaw) : []
                nav.refresh() // keep per-person statuses in the nav fresh after a write
            }
            mMyStatus = statusCtl.myStatus
            mStatusIsDm = statusCtl.isDm
            // Surface gaming write failures (permission, etc.) as a passive toast.
            if (activityCtl.lastError !== mActivityError) {
                mActivityError = activityCtl.lastError
                if (mActivityError.length > 0) core.notify(mActivityError)
            }
            if (statusCtl.lastError !== mStatusError) {
                mStatusError = statusCtl.lastError
                if (mStatusError.length > 0) core.notify(mStatusError)
            }
            if (gifCtl.resultsJson !== _gifsRaw) {
                _gifsRaw = gifCtl.resultsJson
                mGifs = _gifsRaw.length > 0 ? JSON.parse(_gifsRaw) : []
            }
            mGifLoading = gifCtl.loading
            mGifError = gifCtl.error
            if (soundboardCtl.roomSoundsJson !== _roomSoundsRaw) {
                _roomSoundsRaw = soundboardCtl.roomSoundsJson
                mRoomSounds = _roomSoundsRaw.length > 0 ? JSON.parse(_roomSoundsRaw) : []
            }
            if (soundboardCtl.personalSoundsJson !== _personalSoundsRaw) {
                _personalSoundsRaw = soundboardCtl.personalSoundsJson
                mPersonalSounds = _personalSoundsRaw.length > 0 ? JSON.parse(_personalSoundsRaw) : []
            }
            if (soundboardCtl.searchResultsJson !== _soundSearchRaw) {
                _soundSearchRaw = soundboardCtl.searchResultsJson
                mSoundSearch = _soundSearchRaw.length > 0 ? JSON.parse(_soundSearchRaw) : []
            }
            mSoundSearching = soundboardCtl.searching
            mSoundError = soundboardCtl.error
            soundboardCtl.refreshState()
            mSoundCallPlaying = soundboardCtl.callPlaying
            mSoundUploading = soundboardCtl.uploading
            if (joinSoundCtl.searchResultsJson !== _joinSoundSearchRaw) {
                _joinSoundSearchRaw = joinSoundCtl.searchResultsJson
                mJoinSoundSearch = _joinSoundSearchRaw.length > 0 ? JSON.parse(_joinSoundSearchRaw) : []
            }
            mJoinSoundSearching = joinSoundCtl.searching
            // Watch party (mirror controller getters; the 1s sync loop lives in Rust).
            mWatchInParty = watchCtl.inParty
            mWatchAdvancer = watchCtl.amAdvancer
            if (watchCtl.partyJson !== _watchPartyRaw) {
                _watchPartyRaw = watchCtl.partyJson
                mWatchParty = _watchPartyRaw.length > 0 ? JSON.parse(_watchPartyRaw) : ({ queue: [], hostId: "" })
            }
            if (watchCtl.playbackJson !== _watchPlaybackRaw) {
                _watchPlaybackRaw = watchCtl.playbackJson
                mWatchPlayback = _watchPlaybackRaw.length > 0 ? JSON.parse(_watchPlaybackRaw) : ({})
            }
            if (watchCtl.viewersJson !== _watchViewersRaw) {
                _watchViewersRaw = watchCtl.viewersJson
                mWatchViewers = _watchViewersRaw.length > 0 ? JSON.parse(_watchViewersRaw) : []
            }
            if (watchCtl.searchResultsJson !== _watchSearchRaw) {
                _watchSearchRaw = watchCtl.searchResultsJson
                mWatchSearch = _watchSearchRaw.length > 0 ? JSON.parse(_watchSearchRaw) : []
            }
            mWatchSearching = watchCtl.searching
            mWatchError = watchCtl.error
            if (watchCtl.linkJson !== _watchLinkRaw) {
                _watchLinkRaw = watchCtl.linkJson
                mWatchLink = _watchLinkRaw.length > 0 ? JSON.parse(_watchLinkRaw) : ({})
            }
            mWatchAdding = watchCtl.adding
            if (watchCtl.browseJson !== _watchBrowseRaw) {
                _watchBrowseRaw = watchCtl.browseJson
                mWatchBrowse = _watchBrowseRaw.length > 0 ? JSON.parse(_watchBrowseRaw) : ({})
            }
            mWatchBrowsing = watchCtl.browsing
            if (watchCtl.suggestJson !== _watchSuggestRaw) {
                _watchSuggestRaw = watchCtl.suggestJson
                mWatchSuggest = _watchSuggestRaw.length > 0 ? JSON.parse(_watchSuggestRaw) : []
            }
            if (watchCtl.historyJson !== _watchHistoryRaw) {
                _watchHistoryRaw = watchCtl.historyJson
                mWatchHistory = _watchHistoryRaw.length > 0 ? JSON.parse(_watchHistoryRaw) : []
            }
            if (watchCtl.callPartyJson !== _callPartyRaw) {
                _callPartyRaw = watchCtl.callPartyJson
                mCallParty = _callPartyRaw.length > 0 ? JSON.parse(_callPartyRaw) : ({ live: false, viewers: 0, url: "", playing: false, item: ({}) })
            }
            // Don't reassign the spaces array mid drag-reorder (it would reset the
            // DelegateModel and drop the in-progress drag); the committed order arrives
            // on the next tick once the drag releases.
            if (!core.navReordering && nav.spacesJson !== _navSpacesRaw) {
                _navSpacesRaw = nav.spacesJson
                mNavSpaces = _navSpacesRaw.length > 0 ? JSON.parse(_navSpacesRaw) : []
                // RBAC §7.3: a space invite that was not in the previous snapshot → one toast.
                // Skipped until the nav has produced a real snapshot (the meta-spaces are always
                // present once it has), so a restored session never re-announces old invites.
                if (mNavSpaces.length > 0) {
                    var seen = {}
                    for (var si = 0; si < mNavSpaces.length; si++) {
                        var se = mNavSpaces[si]
                        if (!se || se.kind !== "space" || se.membership !== "invite") continue
                        seen[String(se.key)] = true
                        if (core._spaceInviteKeys !== null && !core._spaceInviteKeys[String(se.key)]) {
                            var who = (se.inviter && String(se.inviter).length > 0) ? String(se.inviter)
                                    : (se.inviterMxid && String(se.inviterMxid).length > 0 ? core.localpart(String(se.inviterMxid)) : "Someone")
                            core.notify(who + " invited you to " + String(se.label || "a space"), 4000)
                            if (core.playUiSound) core.playUiSound("notify")
                        }
                    }
                    core._spaceInviteKeys = seen
                }
            }
            if (nav.roomsJson !== _navRoomsRaw) {
                _navRoomsRaw = nav.roomsJson
                mNavRooms = _navRoomsRaw.length > 0 ? JSON.parse(_navRoomsRaw) : []
            }
            // 10.6 (REACH §1.4): the DM strip, mirrored like the rail.
            if (nav.dmStripJson !== _navDmStripRaw) {
                _navDmStripRaw = nav.dmStripJson
                try {
                    var ds = _navDmStripRaw.length > 0 ? JSON.parse(_navDmStripRaw) : null
                    mNavDmStrip = (ds && ds.entries) ? ds : ({ entries: [], overflow: 0 })
                } catch (e) { mNavDmStrip = ({ entries: [], overflow: 0 }) }
            }
            // 10.6 (REACH §2): presence.
            mPresenceKnown = presenceCtl.known
            mPresenceSharing = presenceCtl.sharing
            mPresenceAsked = presenceCtl.asked
            mPresenceTurningOff = presenceCtl.turningOff
            mPresenceBusy = presenceCtl.busy
            if (presenceCtl.rev !== _presenceRev) {
                _presenceRev = presenceCtl.rev
                try { mPresence = presenceCtl.usersJson.length > 0 ? JSON.parse(presenceCtl.usersJson) : ({}) }
                catch (e) { mPresence = ({}) }
                mPresenceMine = presenceCtl.myState
                mPresenceMyRoom = presenceCtl.myRoom
            }
            if (presenceCtl.myStatusJson !== _myStatusRaw) {
                _myStatusRaw = presenceCtl.myStatusJson
                try { mMyPresenceStatus = _myStatusRaw.length > 0 ? JSON.parse(_myStatusRaw) : null } catch (e) { mMyPresenceStatus = null }
            }
            if (presenceCtl.errorRev !== _presenceErrRev) {
                _presenceErrRev = presenceCtl.errorRev
                if (presenceCtl.lastError.length > 0) core.notify(presenceCtl.lastError, 6000)
            }
            // Fetch my photo once a session exists. Component.onCompleted is too early:
            // it runs before login.restore(), so there is no client to ask yet.
            if (login.loggedIn && !core._myAvatarAsked) {
                core._myAvatarAsked = true
                settings.refreshMyAvatar()
                settings.startSafety()   // account safety: the ignored list (safety.rs)
            }
            if (settings.avatarHttp !== mMyAvatar) mMyAvatar = settings.avatarHttp
            mNavActiveSpace = nav.activeSpace
            mNavActiveLabel = nav.activeLabel
            mNavBadge = nav.globalBadge
            // 0.8.26 squad calendar mirror.
            if (calCtl.eventsJson !== _calEventsRaw) {
                _calEventsRaw = calCtl.eventsJson
                try { mCalEvents = _calEventsRaw.length > 0 ? JSON.parse(_calEventsRaw) : [] } catch (e) { mCalEvents = [] }
            }
            if (calCtl.gamesJson !== _calGamesRaw) {
                _calGamesRaw = calCtl.gamesJson
                try { mCalGames = _calGamesRaw.length > 0 ? JSON.parse(_calGamesRaw) : [] } catch (e) { mCalGames = [] }
            }
            if (calCtl.upcomingJson !== _calUpcomingRaw) {
                _calUpcomingRaw = calCtl.upcomingJson
                try { mCalUpcoming = _calUpcomingRaw.length > 0 ? JSON.parse(_calUpcomingRaw) : [] } catch (e) { mCalUpcoming = [] }
            }
            if (calCtl.containersJson !== _calContainersRaw) {
                _calContainersRaw = calCtl.containersJson
                try { mCalContainers = _calContainersRaw.length > 0 ? JSON.parse(_calContainersRaw) : [] } catch (e) { mCalContainers = [] }
            }
            mCalContainer = calCtl.containerId
            mCalContainerLabel = calCtl.containerLabel
            mCalCanEdit = calCtl.canEdit
            mCalIsMod = calCtl.isMod
            mCalMyId = calCtl.myId
            mCalBusy = calCtl.busy
            mCalReminders = calCtl.reminders
            mCalError = calCtl.lastError
            if (calCtl.missingJson !== _calMissingRaw) {
                _calMissingRaw = calCtl.missingJson
                try { mCalMissing = _calMissingRaw.length > 0 ? JSON.parse(_calMissingRaw) : [] } catch (e) { mCalMissing = [] }
            }
            mCalCanInvite = calCtl.canInvite
            mCalUnjoinedParent = calCtl.unjoinedParent
            mCalSourceLabel = calCtl.sourceLabel
            if (!_calStarted) { _calStarted = true; calCtl.start() }
            // 9.0 Game Reel follows the same container as the calendar; presence = the live call roster.
            if (!_gamesStarted) { _gamesStarted = true; gamesCtl.start() }
            if (room.roomId !== _gamesRoom) { _gamesRoom = room.roomId; gamesCtl.setRoom(room.roomId) }
            if (room.roomId !== _calRoom) { _calRoom = room.roomId; calCtl.setRoom(room.roomId) }
            if (calCtl.focusEvent.length > 0) { var fe = calCtl.focusEvent; calCtl.clearFocus(); core.focusCalendarEvent(fe) }
            if (calCtl.autoOpen && !_calAutoOpened && mCalContainer.length > 0) { _calAutoOpened = true; core.openCalendarWindow() }
            // RBAC: the SpaceController mirror — models re-parsed only when its data revision moves.
            if (spaceCtl.rev !== mSpaceRev) {
                mSpaceRev = spaceCtl.rev
                mSpaceRolesActive = spaceCtl.rolesActive
                mSpaceMyRank = spaceCtl.myRank
                try { mSpaceSummary = spaceCtl.summaryJson.length > 0 ? JSON.parse(spaceCtl.summaryJson) : ({}) } catch (e) { mSpaceSummary = ({}) }
                try { mSpaceMembers = spaceCtl.membersJson.length > 0 ? JSON.parse(spaceCtl.membersJson) : [] } catch (e) { mSpaceMembers = [] }
                try { mSpaceRoles = spaceCtl.rolesJson.length > 0 ? JSON.parse(spaceCtl.rolesJson) : [] } catch (e) { mSpaceRoles = [] }
                try { mSpacePerms = spaceCtl.myPermsJson.length > 0 ? JSON.parse(spaceCtl.myPermsJson) : ({}) } catch (e) { mSpacePerms = ({}) }
                try { mSpaceRooms = spaceCtl.roomsJson.length > 0 ? JSON.parse(spaceCtl.roomsJson) : [] } catch (e) { mSpaceRooms = [] }
                try { mSpaceUserSearch = spaceCtl.userSearchJson.length > 0 ? JSON.parse(spaceCtl.userSearchJson) : [] } catch (e) { mSpaceUserSearch = [] }
            }
            if (spaceCtl.spaceId !== mSpaceId) mSpaceId = spaceCtl.spaceId
            if (spaceCtl.busy !== mSpaceBusy) mSpaceBusy = spaceCtl.busy
            if (spaceCtl.permissionCatalogJson !== _permissionCatalogRaw) {
                _permissionCatalogRaw = spaceCtl.permissionCatalogJson
                try { mPermissionCatalog = _permissionCatalogRaw.length > 0 ? JSON.parse(_permissionCatalogRaw) : [] } catch (e) { mPermissionCatalog = [] }
            }
            if (spaceCtl.statusRev !== mSpaceStatusRev) {
                mSpaceStatusRev = spaceCtl.statusRev
                mSpaceStatus = spaceCtl.status
                if (mSpaceStatus.length > 0) core.notify(mSpaceStatus, "short")
            }
            if (spaceCtl.errorRev !== mSpaceErrorRev) {
                mSpaceErrorRev = spaceCtl.errorRev
                mSpaceError = spaceCtl.lastError
                if (mSpaceError.length > 0) core.notify(mSpaceError, 6000)
            }
            // 10.4: invite codes — probe once per sign-in (operator or not), forget on sign-out.
            // 11.1: the Game servers poller starts with the session (a widget acquired before the restore waits for it)
            if (login.loggedIn && !core._invitesProbed) { core._invitesProbed = true; invitesCtl.probe(); gameServersCtl.refresh() }
            else if (!login.loggedIn && core._invitesProbed) { core._invitesProbed = false; invitesCtl.reset(); gameServersCtl.reset() }
            if (invitesCtl.rev !== mInviteRev) {
                mInviteRev = invitesCtl.rev
                mInviteOperator = invitesCtl.isOperator
                mInviteServer = invitesCtl.serverName
                try { mInvites = invitesCtl.invitesJson.length > 0 ? JSON.parse(invitesCtl.invitesJson) : [] } catch (e) { mInvites = [] }
            }
            if (invitesCtl.busy !== mInviteBusy) mInviteBusy = invitesCtl.busy
            if (login.serverOfferJson !== _serverOfferRaw) {
                _serverOfferRaw = login.serverOfferJson
                try { mServerOffer = JSON.parse(_serverOfferRaw) } catch (e) { mServerOffer = { server: "", api: "", features: {}, recorder: "", guest: false } }
                if (core._guestsStarted) guestsCtl.offerChanged()   // the guest room may be on (or off) now
            }
            if (guestsCtl.rev !== mGuestsRev) {
                mGuestsRev = guestsCtl.rev
                try { mGuests = JSON.parse(guestsCtl.stateJson) } catch (e) { /* keep the last */ }
            }
            var guestsHere = room.roomId.length > 0 && room.roomId === String(mGuests.room || "")
            if (guestsHere !== mGuestsHere) mGuestsHere = guestsHere
            if (invitesCtl.statusRev !== mInviteStatusRev) {
                mInviteStatusRev = invitesCtl.statusRev
                if (invitesCtl.status.length > 0) core.notify(invitesCtl.status, "short")
            }
            if (invitesCtl.errorRev !== mInviteErrorRev) {
                mInviteErrorRev = invitesCtl.errorRev
                if (invitesCtl.lastError.length > 0) core.notify(invitesCtl.lastError, 6000)
            }
            var roomSpace = room.roomId.length > 0 ? String(nav.spaceOf(room.roomId)) : ""
            if (roomSpace !== mRoomSpaceId) mRoomSpaceId = roomSpace
            mSharing = stream.sharing
            mPreparing = stream.preparing
            mStreamStatus = stream.status
            if (callctl.inCall && !mInCall) {
                // Rising edge of "in call": re-apply remembered per-participant volumes
                // onto the freshly-subscribed tracks. Tracks subscribe asynchronously, so
                // also nudge a few more times over the next ~6s (NO-OP for absent tracks).
                audioPrefsCtl.reapplyAll()
                reapplyPrefsTimer.restart()
                // #7 BRB/away does NOT ride LiveKit (it's Matrix-side); on a leave/rejoin to
                // the SAME room the roomId-changed check below is skipped, and the stale
                // afkCtl.awayJson/_awayRaw would either show everyone "normal" until the next
                // ~3s forced poll or briefly flash a stale list. Drop the stale mirrors and
                // force an immediate authoritative re-read on THIS rejoin edge.
                _awayRaw = "[]"; mAwayUsers = []; _afkPollAt = Date.now()
                afkCtl.setRoom(callctl.roomId)   // setRoom() already issues refresh()/load_away even when the room is unchanged
            }
            if (!callctl.inCall && mInCall) {
                core.callMaximized = false  // leaving a call exits fill-the-app
                core.listenTiles = []       // explicit listens are per-call
                nav.refresh()               // immediate (may be pre-sync) — the Timer re-refreshes
                leaveStripRefresh._n = 0; leaveStripRefresh.restart() // clear our OWN bubble post-sync
            }
            mInCall = callctl.inCall
            mConnecting = callctl.connecting
            mCallSharing = callctl.sharing
            mMicMuted = callctl.micMuted
            mDeafened = callctl.deafened
            mHangingUp = callctl.hangingUp
            mCallStatus = callctl.status
            if (callctl.outputs !== _outputsRaw) {
                _outputsRaw = callctl.outputs
                mOutputs = _outputsRaw.length > 0 ? JSON.parse(_outputsRaw) : []
            }
            mOutputName = callctl.outputName
            if (callctl.inputs !== _inputsRaw) {
                _inputsRaw = callctl.inputs
                mInputs = _inputsRaw.length > 0 ? JSON.parse(_inputsRaw) : []
            }
            mInputName = callctl.inputName
            mCameraOn = callctl.cameraOn
            callctl.refreshMusicState() // clear musicPlaying if a finite track hit EOS on its own
            mMusicPlaying = callctl.musicPlaying
            if (callctl.cameras !== _camerasRaw) {
                _camerasRaw = callctl.cameras
                mCameras = _camerasRaw.length > 0 ? JSON.parse(_camerasRaw) : []
            }
            mCameraName = callctl.cameraName
            mShareQuality = callctl.shareQuality
            mCamQuality = callctl.camQuality
            mRecvQuality = callctl.recvQuality
            mNcAvailable = callctl.ncAvailable
            mEcDegraded = callctl.ecDegraded
            mEcNote = callctl.ecNote
            mMicDenoise = callctl.micDenoise
            mMicAec = callctl.micAec
            if (callctl.ecOutputs !== _ecOutputsRaw) {
                _ecOutputsRaw = callctl.ecOutputs
                try { mEcOutputs = _ecOutputsRaw.length > 0 ? JSON.parse(_ecOutputsRaw) : [] } catch (e) { mEcOutputs = [] }
            }
            mMicAgc = callctl.micAgc
            mMicNsMode = callctl.micNsMode
            mAecEngine = callctl.aecEngine
            mMicSilent = callctl.micSilent
            mOutputMissing = callctl.outputMissing
            mOverlayEnabled = settings.overlayEnabled
            mOverlayMode = settings.overlayMode
            mOverlayCorner = settings.overlayCorner
            mOverlayHideShare = settings.overlayHideShare
            mOverlayDemo = settings.overlayDemo
            // Lazy-create the HUD only once it's actually wanted (also self-heals if the
            // persisted enable was loaded after startup).
            if ((mOverlayEnabled || mOverlayDemo) && !_overlayWin) ensureOverlay()
            mReconnecting = callctl.reconnecting
            mCallFailed = callctl.callFailed
            mCallFailReason = callctl.failReason
            // RBAC §5.4: call moderation. Perms re-parse on a raw change; the server-muted map is
            // re-derived from the tiles whenever the controller says it moved.
            if (callctl.callPermsJson !== _callPermsRaw) {
                _callPermsRaw = callctl.callPermsJson
                try { mCallPerms = _callPermsRaw.length > 0 ? JSON.parse(_callPermsRaw) : ({}) } catch (e) { mCallPerms = ({}) }
            }
            if (callctl.canModerateCall !== mCanModerateCall) mCanModerateCall = callctl.canModerateCall
            if (callctl.serverMuted !== mServerMuted) mServerMuted = callctl.serverMuted
            if (callctl.serverMutedBy !== mServerMutedBy) mServerMutedBy = callctl.serverMutedBy
            if (callctl.removedBy !== mRemovedBy) mRemovedBy = callctl.removedBy
            if (callctl.moderationRev !== _moderationRev) { _moderationRev = callctl.moderationRev; core._deriveServerMutedMap() }
            if (callctl.errorRev !== _callErrorRev) {
                _callErrorRev = callctl.errorRev
                if (callctl.lastError.length > 0) core.notify(callctl.lastError, 6000)
            }
            if (callctl.audioSources !== _audioSourcesRaw) {
                _audioSourcesRaw = callctl.audioSources
                mAudioSources = _audioSourcesRaw.length > 0 ? JSON.parse(_audioSourcesRaw) : []
            }
            mMusicSourceName = callctl.musicSourceName
            if (callctl.audioApps !== _audioAppsRaw) {
                _audioAppsRaw = callctl.audioApps
                mAudioApps = _audioAppsRaw.length > 0 ? JSON.parse(_audioAppsRaw) : []
            }
            mShareRecOnly = callctl.shareRecOnly
            callctl.refreshRecOnlyFlags()
            if (callctl.recOnlyFlags !== _recOnlyRaw) {
                _recOnlyRaw = callctl.recOnlyFlags
                mRecOnlyFlags = _recOnlyRaw.length > 1 ? JSON.parse(_recOnlyRaw) : ({})
            }
            if (!core.fakeCall) {
                callctl.refreshGameChat()
                if (callctl.gameChatPeers !== _gameChatRaw) {
                    _gameChatRaw = callctl.gameChatPeers
                    try { mGameChatPeers = _gameChatRaw.length > 1 ? JSON.parse(_gameChatRaw) : ({ me: false, peers: [], muted: [] }) }
                    catch (e) { mGameChatPeers = { me: false, peers: [], muted: [] } }
                }
                mGameChat = callctl.gameChat
                // 9.4 corners: the tick also drains the announcements the streamer queued.
                callctl.refreshCorners()
                if (callctl.cornerJson !== _cornerRaw) {
                    var prevMine = (mCorner && mCorner.mine) ? mCorner.mine : ""
                    _cornerRaw = callctl.cornerJson
                    try { mCorner = _cornerRaw.length > 1 ? JSON.parse(_cornerRaw) : ({ mine: "", active: false, members: [], corners: {} }) }
                    catch (e) { mCorner = { mine: "", active: false, members: [], corners: {} } }
                    // 10.1 (TH-FLOAT-02): being pulled aside used to be silent — the room went quiet and
                    // nobody could hear you, with only a yellow button to explain. Say who, and the way out.
                    var nowMine = (mCorner && mCorner.mine) ? mCorner.mine : ""
                    if (nowMine.length > 0 && prevMine.length === 0) {
                        if (!core._cornerLocal) {
                            var mates = (core.shell && core.shell.cornerNames) ? core.shell.cornerNames(mCorner.members || []) : ""
                            core.notify((mates.length > 0 ? mates : "Someone") + " pulled you aside — everyone else is at 10 % for you and can't hear your mic. The yellow mic button takes you back", 7000)
                            if (core.playUiSound) core.playUiSound("notify")
                        }
                        core._cornerLocal = false
                    } else if (nowMine.length === 0 && prevMine.length > 0) {
                        core._cornerLocal = false
                    }
                }
                mCallRoomId = callctl.roomId
            }
            // The strip only ever refreshed when the room-list JSON changed (which
            // carries no participants). Tick the cheap participants-only recompute every 3 s
            // while any listed room has a call (or we are in one).
            if (Date.now() - _navPartAt >= 3000) {
                _navPartAt = Date.now()
                if (core._navHasCall()) nav.refreshParticipants()
            }
            // Self-heal: re-publish the authoritative tile model from Rust if a queued
            // tile closure was ever dropped (a rejoin race), so the delegate (and its
            // attach Timer, which lazily builds the pipeline) still appears.
            if (mInCall) callctl.refreshVideoTiles()
            if (callctl.videoTiles !== _videoTilesRaw) {
                _videoTilesRaw = callctl.videoTiles
                mVideoTiles = _videoTilesRaw.length > 0 ? JSON.parse(_videoTilesRaw) : []
                core._applyVisibilityPause() // newly-attached tiles inherit the hidden/shown state
            }
            // #B live audio levels: poll the streamer, re-map only on a raw-string change
            // (cf. videoTiles), and mirror the local mic level for the self-view bars.
            // Poll levels while in a call OR while the standalone mic monitor runs (the
            // settings noise-cancel before/after meter, which works outside a call).
            if (mInCall || callctl.micMonitoring) {
                callctl.refreshAudioLevels()
                // 11.0 (GPU.md §2.7, R18): the LEVELS (waveforms, meters, speaking rings) only while the window is
                // focused — back to 0 otherwise; the speakers / hear / publish stats below keep flowing (the voice HUD
                // over a game and the muted-talk warning use those, not levels).
                if (Motion.focused) {
                    if (callctl.audioLevels !== _audioLevelsRaw) {
                        _audioLevelsRaw = callctl.audioLevels
                        var arr = _audioLevelsRaw.length > 0 ? JSON.parse(_audioLevelsRaw) : []
                        var m = {}
                        for (var i = 0; i < arr.length; ++i) {
                            m[arr[i].user + "|" + arr[i].category] = arr[i].rms
                        }
                        mAudioLevels = m
                    }
                    mMicLevel = parseFloat(callctl.micLevel) || 0.0
                    mMicLevelPre = parseFloat(callctl.micLevelPre) || 0.0
                } else if (_audioLevelsRaw.length > 0 || mMicLevel > 0.0 || mMicLevelPre > 0.0) {
                    _audioLevelsRaw = ""; mAudioLevels = {}; mMicLevel = 0.0; mMicLevelPre = 0.0
                }
                if (callctl.speakers !== _speakersRaw) {
                    _speakersRaw = callctl.speakers
                    mSpeakers = _speakersRaw.length > 0 ? JSON.parse(_speakersRaw)
                                                        : ({ me: "", active: [] })
                }
                // Outgoing stream health (same guard idiom; the JSON changes ~every 3s).
                if (callctl.publishStats !== _publishStatsRaw) {
                    _publishStatsRaw = callctl.publishStats
                    mPublishStats = _publishStatsRaw.length > 0 ? JSON.parse(_publishStatsRaw) : []
                }
                // 10.5 "can they hear me?" — same guard; the streamer rebuilds it at most 4x/s.
                if (callctl.hearJson !== _hearRaw) {
                    _hearRaw = callctl.hearJson
                    try { mHear = _hearRaw.length > 2 ? JSON.parse(_hearRaw) : ({}) } catch (e) { mHear = ({}) }
                }
                // 11.3 same room — same guard; the streamer rebuilds it only on a change.
                if (callctl.sameRoomJson !== _sameRoomRaw) {
                    _sameRoomRaw = callctl.sameRoomJson
                    try { mSameRoom = _sameRoomRaw.length > 2 ? JSON.parse(_sameRoomRaw) : _sameRoomIdle } catch (e) { mSameRoom = _sameRoomIdle }
                    if (!mSameRoom.linked) mSameRoom = _sameRoomIdle
                }
            } else if (_audioLevelsRaw.length > 0 || mMicLevel > 0.0 || mMicLevelPre > 0.0
                       || _speakersRaw.length > 0 || _publishStatsRaw.length > 0 || _hearRaw.length > 0) {
                _audioLevelsRaw = ""; mAudioLevels = {}; mMicLevel = 0.0; mMicLevelPre = 0.0
                _speakersRaw = ""; mSpeakers = ({ me: "", active: [] })
                _publishStatsRaw = ""; mPublishStats = []
                _hearRaw = ""; mHear = ({})
                _sameRoomRaw = ""; mSameRoom = _sameRoomIdle
            }
            // Speaker meter: polled independently of the call (the settings Audio pane runs it
            // outside a call). refreshAudioLevels (called above when in a call OR mic-monitoring)
            // already updates outputLevel; call it here too for the output-only case, then mirror.
            if (callctl.outputMonitoring && Motion.focused) {   // 11.0 (R18): a meter only while focused
                if (!mInCall && !callctl.micMonitoring) callctl.refreshAudioLevels()
                mOutputLevel = parseFloat(callctl.outputLevel) || 0.0
            } else if (mOutputLevel > 0.0) {
                mOutputLevel = 0.0
            }
            // #7 AFK/BRB: track the active call's room; poll ~every 3s for peers' away state.
            if (mInCall) {
                if (callctl.roomId !== afkCtl.roomId) afkCtl.setRoom(callctl.roomId)
                if (Date.now() - _afkPollAt >= 3000) { _afkPollAt = Date.now(); afkCtl.refresh() }
                mLocalAfk = afkCtl.localAfk
                if (afkCtl.awayJson !== _awayRaw) {
                    _awayRaw = afkCtl.awayJson
                    mAwayUsers = _awayRaw.length > 0 ? JSON.parse(_awayRaw) : []
                }
                // #4 AFK CUTS BOTH DIRECTIONS. On the rising edge of OUR away: stop
                // TRANSMITTING (mute mic via LiveKit, turn camera off) AND stop
                // RECEIVING (cut incoming audio + hide remote tiles — handled by the
                // tile delegates' visible binding on mLocalAfk). On the falling edge we
                // only restore INCOMING; mic/cam stay as the user left them (no
                // surprise-unmute) — the user re-enables them manually.
                if (mLocalAfk !== _afkApplied) {
                    _afkApplied = mLocalAfk
                    if (mLocalAfk) {
                        // Record whether WE (AFK) muted the mic, so coming back restores exactly
                        // that and never surprise-unmutes a mic the user muted themselves. Use the
                        // LIVE callctl.micMuted (not the 120ms-lagged mirror) + an EXPLICIT setter
                        // (not toggle) so the edge can't mis-fire.
                        _afkMutedMic = !callctl.micMuted
                        if (_afkMutedMic) callctl.forceMicMuted(true)   // mute mic (signals LiveKit)
                        if (mCameraOn) callctl.toggleCamera()           // unpublish camera
                        callctl.setAwayPrivacy(true)                    // cut incoming audio
                    } else {
                        callctl.setAwayPrivacy(false)                   // restore incoming audio
                        // Force-unmute iff AFK paused it — EXPLICIT (not toggle) + idempotent, so
                        // it ALWAYS drives the mic pipeline back to Playing (resuming capture),
                        // independent of mirror timing or the Matrix away-write 403'ing.
                        if (_afkMutedMic) callctl.forceMicMuted(false)
                        _afkMutedMic = false
                    }
                } else if (mLocalAfk) {
                    // Still away: re-apply the incoming cut so audio tracks that subscribe
                    // mid-AFK are muted too (idempotent; never touches a #5 user mute).
                    callctl.setAwayPrivacy(true)
                }
            } else if (mAwayUsers.length > 0 || mLocalAfk || _afkApplied) {
                if (_afkApplied) { callctl.setAwayPrivacy(false); _afkApplied = false }
                mAwayUsers = []; mLocalAfk = false; _awayRaw = "[]"; _afkMutedMic = false
            }
            // First-run tour: re-evaluate each tick until shown/considered, so it waits
            // for verify + any in-progress call to settle before auto-popping.
            core.mirrored()
        }
    }
    // Decided 2026-10-26: a steam:// link opens only after "Open in Steam?" (LinkGate.ask; every other app scheme is refused)
    ConfirmDialog {
        id: steamConfirm
        property string url: ""
        dialogTitle: "Open in Steam?"
        // the web's words (core/linkgate.ts openMessageLink; gate 6 parity note, assembly verify L)
        message: "This link opens the Steam app: " + steamConfirm.url
        confirmText: "Open"
        cancelText: "Cancel"
        onConfirmed: LinkGate.openAsked(steamConfirm.url)
    }
    // Security review 2026-10-26: a link or attachment LinkGate refused says so (the log has which one and why).
    Connections {
        target: LinkGate
        function onAsk(url, kind) {
            if (kind !== "steam") return
            steamConfirm.url = url
            steamConfirm.open()
        }
        function onRefused(url, why) {
            core.notify(why === "not-a-viewer-file" ? "Tin Hat doesn't open that kind of file — use Save as… to keep it"
                                                    : "Tin Hat doesn't open that kind of link (only web, email and Steam links)", 6000)
        }
    }
    // Security review 2026-10-26 (M7): every attached tooltip shows its text as typed. The style's one shared tooltip
    // draws it with Qt's default AutoText, so a name or a status with markup in it was rendered (and an <img> in it
    // loaded); its label(s) become PlainText once, at start (Basic: the Text content item; KDE: a Label inside an Item).
    function _plainTooltips() {
        var n = 0
        var walk = function (it, depth) {
            if (!it || depth > 4) return
            if (it.textFormat !== undefined) { it.textFormat = Text.PlainText; n++ }
            var kids = it.children || []
            for (var i = 0; i < kids.length; i++) walk(kids[i], depth + 1)
        }
        var tip = Controls.ToolTip.toolTip
        walk(tip ? tip.contentItem : null, 0)
        // launch-gate's review #7: fail closed — the room and DM tooltips escape their text unless this worked
        LinkGate.tooltipsPlain = n > 0
        if (n === 0) console.warn("[tooltips] no text item found in the shared tooltip — tooltips stay AutoText, LinkGate.tipText escapes")
    }
    // On launch: apply a forced theme (test hook TINHAT_THEME), then silently sign
    // back in if a saved session exists.
    Component.onCompleted: {
        try { core._plainTooltips() } catch (e) { console.warn("[tooltips] " + e) }   // never in the way of the start
        padCtl.attach()       // Pad: events reach the controller from now on (before the session is restored)
        core._syncTileModel() // 0.8.28: seed the grid model (fake tiles / empty)
        // 10.6 (REACH §2): the presence engine waits for a session on its own; it never runs in
        // the recorder (no QML there, and presence.rs refuses under TINHAT_VOD_SERVICE).
        presenceCtl.start()
        presenceCtl.setWindowState(core.presenceWinVisible, core.presenceWinFocused)
        try { core.mStatusPresets = JSON.parse(presenceCtl.presetsJson) } catch (e) { core.mStatusPresets = [] }
        try { core.mStatusPalette = JSON.parse(presenceCtl.paletteJson) } catch (e) { core.mStatusPalette = [] }
        // Load the persisted push-to-talk choice; without this the toggle reads OFF on
        // every launch while the saved preference says otherwise.
        callctl.restorePushToTalk()
        // themes 1b: the theme store publishes what this device renders (THEMES-SYNC.md §8: TINHAT_THEME > this device's
        // own theme > the account's > the pre-11.0 setting); in gate mode the 10.6 precedence below stands.
        themeCtl.start()
        // Theme precedence: TINHAT_THEME env (test override) > persisted choice > default.
        var startTheme = themeCtl.syncing ? themeCtl.effectiveTheme
            : ((login.autoTheme && login.autoTheme.length > 0) ? login.autoTheme
               : (login.savedTheme && login.savedTheme.length > 0 ? login.savedTheme : "oled"))
        if (core.themeName === startTheme) core.applyTheme(startTheme)   // the default already: no change signal
        else core.themeName = startTheme                                   // onThemeNameChanged applies it
        console.log("[theme] active=" + core.themeName + " (saved=" + login.savedTheme + " env=" + login.autoTheme + ")")
        login.applyColorScheme(core.themeName) // recolor the Kirigami chrome to match
        core._themeDoc = themeCtl.effectiveKey.split("|")[1] || "0"
        core._themeBooted = true
        // Restore the saved nav mode (env TINHAT_GUI_DOCK forces "corner" for screenshots).
        core.navMode = (login.autoDock === "corner" || login.savedNavMode === "corner") ? "corner" : "persistent"
        callctl.refreshShareQuality() // populate the screen-share quality menu state
        callctl.refreshCamQuality()   // populate the camera quality menu state
        callctl.refreshRecvQuality()  // populate the incoming-video quality menu state
        settings.refreshOverlay()     // voice-overlay prefs from disk (+ TINHAT_OVERLAY_DEMO env)
        settings.refreshCallLayout()  // call-grid preset from disk
        core.callLayout = settings.callLayout.length > 0 ? settings.callLayout : "auto"
        // 9.3.1: the call/chat seam from disk.
        var pf = parseFloat(settings.uiPref("call-pane-frac"))
        core.callPaneFrac = (!isNaN(pf) && pf >= 0.2 && pf <= 0.85) ? pf : -1
        core.callCollapsed = settings.uiPref("call-collapsed") === "1"
        core.callBarPinned = settings.uiPref("call-bar-pinned") === "1" || Qt.application.arguments.indexOf("--tinhat-callpin") >= 0
        settings.refreshCallHidden()  // 0.8.7: hide/show memory from disk
        settings.refreshCallTileView() // 0.8.22: per-person tile view memory
        try { core.mSbRecent = JSON.parse(settings.pref("soundboard-recent") || "[]") } catch (e) { core.mSbRecent = [] }
        try {
            core.tileViews = JSON.parse(settings.callTileView || "{}")
            if (core.tileViews._self && core.tileViews._self.mode) core.selfViewMode = core.tileViews._self.mode
        } catch (e) { core.tileViews = {} }
        try { core.hiddenKeys = JSON.parse(settings.callHidden || "[]") } catch (e) { core.hiddenKeys = [] }
        settings.refreshRecordOptout() // 0.8.7: "never record me" from disk
        login.restore()
        core.booted()
    }
    // ---- Startup reveal (0.8.24) ----
    // The window stays hidden while the StartupWindow narrates restore + room load. Reveal
    // when the rooms are in (from the local store — well before the first sync), when there
    // is no session to restore (login card), or on a hard cap so a stuck restore can never
    // hide the app. TINHAT_BOOT_SLOW=1 holds the startup window ~3 s (screenshots).
    property bool _bootShown: false
    function _bootReveal(why) {
        if (_bootShown) return
        _bootShown = true
        console.log("[boot] revealing main window: " + why)
        core.bootReveal(why)
    }
    Timer {
        id: bootRevealTimer
        interval: 100; repeat: true; running: !core._bootShown
        property int ticks: 0
        onTriggered: {
            ticks += 1
            if (bootctl.slow && ticks < 30) return
            if (core.mLoggedIn && core.mRooms.length > 0) core._bootReveal("rooms loaded")
            else if (core.mLoggedIn && ticks >= 25) core._bootReveal("signed in, rooms still loading")
            else if (!core.mLoggedIn && !core.mBusy && ticks >= 3) core._bootReveal("no session to restore")
            else if (ticks >= 200) core._bootReveal("timeout")
        }
    }
    // ---- Screen-share helpers (parse the "maxW maxH fps kbps encoder" mirror). ----
    function _sqTokens() { return (core.mShareQuality || "").split(" ") }
    function _sqParts() {
        var p = _sqTokens().slice(0, 4).map(Number)
        return (p.length === 4 && !isNaN(p[0])) ? p : [3840, 1440, 30, 10000]
    }
    function _sqW() { return _sqParts()[0] }
    function _sqH() { return _sqParts()[1] }
    function _sqFps() { return _sqParts()[2] }
    function _sqKbps() { return _sqParts()[3] }
    function _sqEnc() { return _sqTokens()[4] || "nvenc" }

    // ---- Camera-quality helpers (parse the "W H FPS KBPS SIM01" mirror). ----
    function _cqTokens() { return (core.mCamQuality || "").split(" ") }
    function _cqParts() {
        var p = _cqTokens().slice(0, 4).map(Number)
        return (p.length === 4 && !isNaN(p[0])) ? p : [1280, 720, 30, 2500]
    }
    function _cqW() { return _cqParts()[0] }
    function _cqH() { return _cqParts()[1] }
    function _cqSim() { return (_cqTokens()[4] || "1") !== "0" }

    // #B: this tile's live audio RMS (0.0..1.0) for the per-name waveform bars. The self-
    // view uses the local mic level; a remote tile carries the full LiveKit identity
    // (@u:server:DEVICE) while levels are keyed by that same identity — but tracks may also
    // register under a bare mxid, so match by mxid PREFIX (cf. the #5 gain path / isAway).
    function tileAudioLevel(participant, isSelf) {
        if (isSelf) return core.mMicLevel
        var p = participant || ""
        var best = 0.0
        for (var k in core.mAudioLevels) {
            var bar = k.indexOf("|")
            var id = bar >= 0 ? k.substring(0, bar) : k
            if (id === p || p.indexOf(id + ":") === 0 || id.indexOf(p + ":") === 0) {
                var v = core.mAudioLevels[k]
                if (v > best) best = v
            }
        }
        return best
    }

    // ---- Timeline rendering helpers ----
    // "@alex:example.com" -> "alex" (display until we resolve member profiles).
    function localpart(mxid) {
        if (!mxid) return ""
        var m = /^@([^:]+):/.exec(mxid)
        return m ? m[1] : mxid
    }
    // Stable per-sender color (hashed hue), readable on the Plasma theme background.
    function senderColor(mxid) {
        var s = mxid || ""
        var h = 0
        for (var i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) >>> 0
        // themes 1b (THEMES-UI §10.1): a palette theme colours names from its own name.1 … name.12
        if (Th.names === "palette") return Th.byId("name." + ((h % 12) + 1))
        var dark = Kirigami.Theme.backgroundColor.hslLightness < 0.5
        // 10.1 (TH-MSG-19): the same warm hue band Avatar.qml derives from the same hash, so a
        // person's name and their disc agree (they used to be two unrelated colours side by side).
        var pick = h % 100
        var hue = (pick < 80) ? (0.02 + (h % 56) / 360.0) : (0.45 + (h % 22) / 360.0)
        return Qt.hsla(hue, 0.6, dark ? 0.7 : 0.42, 1)
    }
    // Friendly day label for the date separators.
    function dayLabel(ms) {
        var d = new Date(ms)
        var now = new Date()
        var t = new Date(now.getFullYear(), now.getMonth(), now.getDate())
        var dd = new Date(d.getFullYear(), d.getMonth(), d.getDate())
        var diff = Math.round((t - dd) / 86400000)
        if (diff === 0) return "Today"
        if (diff === 1) return "Yesterday"
        return Qt.formatDate(d, diff < 7 ? "dddd" : (d.getFullYear() === new Date().getFullYear() ? "dddd, MMMM d" : "dddd, MMMM d, yyyy"))   // 10.1 (TL-19): the year once it matters
    }
    function sameDay(a, b) {
        var x = new Date(a), y = new Date(b)
        return x.getFullYear() === y.getFullYear() && x.getMonth() === y.getMonth() && x.getDate() === y.getDate()
    }
    // Screen-share lifecycle -> passive notifications.
    onMStreamStatusChanged: if (mStreamStatus.length > 0) core.notify(mStreamStatus, "short")
    onMCallStatusChanged: if (mCallStatus.length > 0) core.notify(mCallStatus, "short")
    // 10.1 (TH-SHELL-01): after sign-in, LoginController.status had no home on screen — a bad
    // invite link, a malformed user id in "New chat", an invite to nobody all failed in silence.
    // The sign-in card keeps owning it before login.
    onMStatusChanged: if (mLoggedIn && mStatus.length > 0) core.notify(mStatus, "short")
    // Surface room-level errors (send / upload / load failures) instead of swallowing them.
    onMRoomStatusChanged: if (mRoomStatus.length > 0) core.notify(mRoomStatus, "short")
}
