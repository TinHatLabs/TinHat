// SettingsWindow.qml — Settings as its own window (contract SETTINGS-WINDOW.md;
// docs/SETTINGS-WINDOW.md; the web's in-app `settings` window). The 2026-09-29 request: "the actual settings window, can we
// just put some elbow grease to make it as user friendly as possible and its own window instead of a floating one".
//
// A real top-level Window (like SpaceSettingsWindow): the rail on the left (search, four groups of panes), one pane at
// a time on the right as cards of rows (SettingsPage / SettingsSection / SettingsCard / SettingsRow), the status strip
// and the Profile save bar at the bottom. Every setting, pref key and controller call is the old Settings sheet's
// (Main.qml, 11.1): this is a re-layout and a re-home, not a feature cut.
//
// Main.qml owns the instance (id settingsWindow; the old name `settingsSheet` is an alias of it) and the surfaces that
// live in the main window: `app.settingsHandOff(pane, what, arg)` steps this window aside, opens one of them (verify,
// key backup, noise test, keyboard shortcuts, report a problem, what's new, sign out, the tour, the row editor) and
// brings Settings back on `pane` when it closes.
//
// Keys: Ctrl+F or / focuses the search; ↑/↓ in the rail cycles panes (↑/↓ + Return in the search field walk the
// results); Esc clears a search, else closes the window (open popups take it first).
import QtQuick
import QtQuick.Window
import QtQuick.Layouts
import QtQuick.Dialogs
import QtQuick.Controls as Controls
import QtQuick.Templates as T
import QtMultimedia
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the settings.* tokens; Motion

Window {
    id: win
    required property var app            // Main.qml root: the controllers (settings, callctl, …) and the m* mirrors
    property var core: null              // AppCore (Appearance)

    // NEVER exactly "Tin Hat": the boot smoke finds the main window by that exact title.
    title: "Settings · Tin Hat"
    width: 1000
    height: 720
    minimumWidth: 760
    minimumHeight: 540
    color: Th.settingsWindowBg
    visible: false
    // the main window's palette (Main.qml): the few stock Controls here (Switch, ToolTip) paint from it
    palette.window: Th.surfaceWindow
    palette.windowText: Th.textPrimary
    palette.base: Th.surfaceIsland
    palette.alternateBase: Th.surfaceRaised
    palette.text: Th.textPrimary
    palette.button: Th.surfaceIsland
    palette.buttonText: Th.textPrimary
    palette.highlight: Th.accentBase
    palette.highlightedText: Th.textOnAccent
    palette.mid: Th.borderSubtle
    palette.light: Th.surfaceRaised
    palette.dark: Th.surfaceWindow
    palette.placeholderText: Th.textSecondary
    palette.link: Th.textLink
    WindowMemory { id: memo; target: win; key: "settings"; store: (win.app && win.app.prefStore) ? win.app.prefStore : null }

    // The old sheet's opened / closed (AppearancePane: a theme Undo lasts while Settings is open)
    signal opened()
    signal closed()
    readonly property alias appearance: appearancePane   // Main.qml: the ☰ Theme menu's "More themes…" / "Edit current theme…"

    // ======================================================================================
    // Panes (§3): key · label · icon · description, in rail order
    readonly property var paneInfo: ({
        profile:       { t: "Profile",            i: "user-symbolic",              d: "Your name, photo, colour and join sound" },
        privacy:       { t: "Privacy",            i: "view-private-symbolic",      d: "Your status, time zone, recordings and logs" },
        security:      { t: "Devices & security", i: "security-high-symbolic",     d: "Where you're signed in, verification and key backup" },
        invites:       { t: "Invites",            i: "contact-new-symbolic",       d: "Invite codes for new accounts" },
        guests:        { t: "Guests",             i: "system-users-symbolic",      d: "Let people in for a while without an account" },
        audio:         { t: "Audio",              i: "audio-volume-high-symbolic", d: "Microphone, speaker, echo and noise" },
        calls:         { t: "Calls & video",      i: "call-start-symbolic",        d: "Push to talk, camera, screen share and connection" },
        appearance:    { t: "Appearance",         i: "contrast-symbolic",          d: "Theme, sounds, navigation and motion" },
        notifications: { t: "Notifications",      i: "notifications",              d: "Desktop alerts and sounds" },
        shortcuts:     { t: "Shortcuts",          i: "input-keyboard-symbolic",    d: "Give any action a key" },
        overlay:       { t: "Overlay",            i: "window-pin-symbolic",        d: "See who's talking over fullscreen games" },
        about:         { t: "About & help",       i: "help-about-symbolic",        d: "Version, updates, help and sign out" }
    })
    readonly property var groups: [
        { g: "Account",       keys: ["profile", "privacy", "security", "invites", "guests"] },
        { g: "Voice & video", keys: ["audio", "calls"] },
        { g: "App",           keys: ["appearance", "notifications", "shortcuts", "overlay"] },
        { g: "Tin Hat",       keys: ["about"] }
    ]
    readonly property bool isOperator: !!(win.app && win.app.mInviteOperator)
    // selfhost (docs/GUESTS.md): Settings → Guests while the server offers the guest room (PANE_GATES), never for a
    // temporary guest account
    readonly property bool guestsOffered: !!win.app && win.app.paneOffered("guests") && !win.app.mGuestAccount
    readonly property var paneKeys: {
        var out = []
        for (var g = 0; g < win.groups.length; g++)
            for (var i = 0; i < win.groups[g].keys.length; i++) {
                var k = win.groups[g].keys[i]
                if ((k !== "invites" || win.isOperator) && (k !== "guests" || win.guestsOffered)) out.push(k)
            }
        return out
    }
    // the 10.x int panes (--tinhat-pane=<n>, old deep links) → keys
    readonly property var legacyPanes: ["profile", "appearance", "audio", "calls", "notifications", "security", "security", "privacy", "overlay", "shortcuts", "invites"]
    function normKey(k) {
        var s = String(k === undefined || k === null ? "" : k)
        if (/^\d+$/.test(s)) s = win.legacyPanes[parseInt(s)] || ""
        if (s === "sessions") s = "security"
        if (!win.paneInfo[s]) return ""
        if (s === "invites" && !win.isOperator) return "profile"
        if (s === "guests" && !win.guestsOffered) return "profile"
        return s
    }
    property string currentKey: "profile"
    readonly property bool searching: win.query.trim().length > 0
    // the pane on screen (none while the window is hidden or search results show) — drives the per-pane work
    readonly property string activePane: (win.visible && !win.searching) ? win.currentKey : ""
    // content < 560 px wide: every row stacks (§3)
    readonly property real innerWidth: Math.min(720, contentArea.width - 56)
    readonly property bool settingsNarrow: win.innerWidth < 560

    function openAt(key) {
        var k = win.normKey(key)
        if (k.length > 0) win.currentKey = k
        win.query = ""
        if (!win.visible) {
            if (!memo.remembered || !win._placedOnce) win._center()
            win.visible = true
        }
        win.raise(); win.requestActivate()
        rail.forceActiveFocus()
    }
    function open() { win.openAt("") }          // the last pane of this session (Profile the first time)
    function cyclePane(n) { var ks = win.paneKeys; win.currentKey = ks[((n % ks.length) + ks.length) % ks.length] }
    property bool _placedOnce: false
    function _center() {
        win._placedOnce = true
        if (!win.app || memo.canPlace === false) return
        var hasMemo = win.app.prefStore && String(win.app.prefStore.uiPref("settings-x")).length > 0
        if (hasMemo) return
        win.x = Math.round(win.app.x + (win.app.width - win.width) / 2)
        win.y = Math.round(win.app.y + (win.app.height - win.height) / 2)
    }
    function handOff(pane, what, arg) { if (win.app) win.app.settingsHandOff(pane, what, arg === undefined ? "" : arg) }

    // ======================================================================================
    // Mirrors of the controller getters while the window shows (cxx-qt NOTIFY does not reliably drive bindings here;
    // the old sheet's scoped poll, 150 ms focused / 1 s unfocused — 11.0 R14)
    property string sName: ""
    property string sAvatar: ""
    property var sDevices: []
    property string _devRaw: ""
    property string sStatus: ""
    property bool sStatusError: false
    property bool sBusy: false
    property bool sBackup: false
    property bool sNotify: true
    property bool sXsign: false
    property string sUserId: ""
    property string sBio: ""
    property string sAccent: ""
    property string sJoinSound: ""
    property bool uChecking: false
    property string _statusSeen: ""
    // 10.1 (TH-SET-12): 2.5 s after the Audio pane opens, a meter that never started means the device could not be
    // opened — say so instead of quietly hiding the row
    property bool audioSettled: false
    Timer { id: audioSettleT; interval: 2500; onTriggered: win.audioSettled = true }

    Timer {
        interval: Motion.focused ? 150 : 1000; repeat: true; running: win.visible
        triggeredOnStart: true
        onTriggered: {
            var s = win.app.settings
            win.sName = s.displayName
            win.sAvatar = s.avatarHttp
            if (s.status !== win._statusSeen) { win._statusSeen = s.status; if (s.status.length > 0 && s.status !== "Saving…") win.toast(s.status, 4000) }
            win.sBusy = s.busy
            win.sBackup = s.backupEnabled
            win.sNotify = s.desktopNotifications
            win.sXsign = s.crossSigningReady
            win.sUserId = s.userId
            win.sBio = s.profileBio
            win.sAccent = s.profileAccent
            win.sJoinSound = s.joinSound
            win.uChecking = win.app.updater.checking
            if (s.devicesJson !== win._devRaw) {
                win._devRaw = s.devicesJson
                var list = s.devicesJson.length > 0 ? JSON.parse(s.devicesJson) : []
                list.sort(function (a, b) { return (b.isCurrent ? 1 : 0) - (a.isCurrent ? 1 : 0) || (b.lastSeenTs || 0) - (a.lastSeenTs || 0) })
                win.sDevices = list
            }
        }
    }

    onVisibleChanged: {
        if (visible) {
            var s = win.app.settings
            s.refresh()
            s.refreshEmails()
            s.refreshDesktopNotifications()
            s.refreshOverlay()
            // the persisted mic-DSP toggles live on the Audio pane — load them so the switches show real state even
            // before any call refreshed them
            win.app.callctl.refreshMicDenoise(); win.app.callctl.refreshMicApm(); win.app.callctl.refreshMicAec()
            win.app.diagnostics.refresh()
            win.sNotify = s.desktopNotifications
            win.opened()
        } else {
            win.query = ""
            joinPreview.stop(); joinPreview.playingUrl = ""
            changePasswordDialog.close()   // its fields empty themselves (no password stays behind a closed window)
            win.closed()
        }
    }
    // (G) the Audio pane refreshes the device + share-source lists and runs BOTH live meters (speaker + mic); both stop
    // on leaving the pane or hiding the window, so no device is held open in the background
    onActivePaneChanged: {
        var c = win.app.callctl
        if (win.activePane === "overlay") win.app.ensureOverlay()   // 10.1 (TH-SET-20): "unavailable" before a tick
        if (win.activePane === "guests") win.app.guestsCtl.refresh()   // selfhost: the guest room's doc, read when the pane shows
        if (win.activePane === "calls") { c.refreshCameras(); c.refreshShareQuality(); c.refreshCamQuality(); c.refreshRecvQuality() }
        win.audioSettled = false
        if (win.activePane === "audio") {
            c.refreshInputs(); c.refreshOutputs(); c.refreshAudioSources()
            c.startOutputMonitor(); c.startMicMonitor()
            audioSettleT.restart()
        } else {
            c.stopOutputMonitor(); c.stopMicMonitor()
        }
    }
    onCurrentKeyChanged: { var p = win.pageFor(win.currentKey); if (p) p.scrollToTop() }
    onPaneKeysChanged: if (win.paneKeys.indexOf(win.currentKey) < 0) win.currentKey = "profile"

    // ======================================================================================
    // Status strip (the old 4 s "Saved" / error line) — also this window's toasts (a theme's Undo …)
    property string stripText: ""
    property bool stripError: false
    property string stripAction: ""
    property var _stripCb: null
    Timer { id: stripTimer; interval: 4000; onTriggered: { win.stripText = ""; win.stripAction = ""; win._stripCb = null } }
    function toast(text, timeout, actionText, cb) {
        var t = String(text || "")
        if (t.length === 0) return
        win.stripText = t
        win.stripError = /fail|couldn|error|denied|isn't|can't/i.test(t)
        win.stripAction = actionText ? String(actionText) : ""
        win._stripCb = cb || null
        stripTimer.interval = timeout === "long" ? 8000 : (timeout === "short" || timeout === undefined || timeout === null ? 4000 : Math.max(1500, Number(timeout) || 4000))
        stripTimer.restart()
    }

    // ======================================================================================
    // Profile drafts (§5): name / about / colour change nothing until "Save changes"; the poll keeps an untouched field
    // in step with the server and never overwrites what you typed
    property string _seedName: ""
    property string _seedBio: ""
    property string _seedAccent: ""
    property string dAccent: ""
    onSNameChanged: { if (nameField.text === win._seedName) nameField.text = win.sName; win._seedName = win.sName }
    onSBioChanged: { if (bioField.text === win._seedBio) bioField.text = win.sBio; win._seedBio = win.sBio }
    onSAccentChanged: { if (win.dAccent === win._seedAccent) win.dAccent = win.sAccent; win._seedAccent = win.sAccent }
    readonly property bool nameDirty: nameField.text !== win.sName
    readonly property bool bioDirty: bioField.text !== win.sBio
    readonly property bool accentDirty: win.dAccent !== win.sAccent
    readonly property bool profileDirty: win.nameDirty || win.bioDirty || win.accentDirty
    readonly property bool nameEmpty: nameField.text.trim().length === 0
    function saveProfile() {
        if (!win.profileDirty || win.sBusy || win.nameEmpty) return
        if (win.nameDirty) win.app.settings.saveDisplayName(nameField.text.trim())
        if (win.bioDirty) win.app.settings.saveProfileBio(bioField.text)
        if (win.accentDirty) win.app.settings.saveProfileAccent(win.dAccent)
    }
    function resetProfile() { nameField.text = win.sName; bioField.text = win.sBio; win.dAccent = win.sAccent }
    readonly property bool saveBarShown: win.profileDirty && win.currentKey === "profile" && !win.searching
    readonly property var emailList: { try { return JSON.parse(win.app.settings.emailsJson || "[]") } catch (e) { return [] } }

    // ======================================================================================
    // Search (§4)
    property string query: ""
    onQueryChanged: { if (searchField.text !== win.query) searchField.text = win.query; win.selIndex = 0 }
    readonly property var results: {
        if (!win.searching || !win.app) return []
        var all = []
        try { all = JSON.parse(String(win.app.settings.searchSettings(win.query, win.isOperator))) } catch (e) { return [] }
        // only rows of the panes this window shows (Settings → Guests: where the server offers it, never for a guest)
        return all.filter(function (r) { return win.paneKeys.indexOf(String(r.pane)) >= 0 })
    }
    property int selIndex: 0
    function openResult(i) { if (i >= 0 && i < win.results.length) win.jumpTo(String(win.results[i].id)) }
    property string _jumpId: ""
    function jumpTo(id) {
        var pane = String(win.app.settings.settingsPaneOf(id))
        if (pane.length === 0) return
        win.query = ""
        win.currentKey = win.normKey(pane)
        win._jumpId = id
        jumpT.restart()
    }
    Timer { id: jumpT; interval: 90; onTriggered: win._doJump() }
    function pageFor(key) {
        switch (key) {
        case "profile": return profilePage
        case "privacy": return privacyPage
        case "security": return securityPage
        case "invites": return invitesPage
        case "guests": return guestsPage
        case "audio": return audioPage
        case "calls": return callsPage
        case "appearance": return appearancePane.page
        case "notifications": return notifyPage
        case "shortcuts": return shortcutsPage
        case "overlay": return overlayPage
        case "about": return aboutPage
        }
        return null
    }
    // every item under `root` that carries a search anchor, in reading order
    function _anchors(root, out) {
        var kids = root.children
        for (var i = 0; i < kids.length; i++) {
            var k = kids[i]
            if ((k.anchorId !== undefined && String(k.anchorId).length > 0) || (k.anchorIds !== undefined && k.anchorIds.length > 0)) out.push(k)
            win._anchors(k, out)
        }
        return out
    }
    function _hasId(item, id) { return item.anchorId === id || (item.anchorIds !== undefined && item.anchorIds.indexOf(id) >= 0) }
    function _doJump() {
        var page = win.pageFor(win.currentKey)
        if (!page) return
        if (win.currentKey === "appearance") appearancePane.galleryOpen = false
        var list = win._anchors(page, [])
        var at = -1
        for (var i = 0; i < list.length; i++) if (win._hasId(list[i], win._jumpId)) { at = i; break }
        if (at < 0) return
        // a row shown only in some states (Speaker outputs to cancel while echo cancellation is off …): the nearest
        // visible row before it, which is the switch that shows it
        while (at >= 0 && !list[at].visible) at--
        if (at < 0) return
        var target = list[at]
        page.scrollToItem(target)
        if (typeof target.flash === "function") target.flash()
        console.log("[settings] jump " + win._jumpId + " → " + (target.anchorId || win._jumpId))
    }

    // ======================================================================================
    // Dialogs and popups of this window
    FileDialog {
        id: avatarDialog
        title: "Choose a photo"
        nameFilters: ["Images (*.png *.jpg *.jpeg *.gif *.webp)"]
        onAccepted: win.app.settings.setAvatar(selectedFile)
    }
    // 10.1 (TH-SET-17): the two destructive buttons go through the house confirm
    ConfirmDialog {
        id: removeAvatarConfirm
        app: win.app
        dialogTitle: "Remove your photo?"
        message: "Your picture comes off your profile for everyone."
        confirmText: "Remove"
        destructive: true
        onConfirmed: win.app.settings.clearAvatar()
    }
    ConfirmDialog {
        id: removeEmailConfirm
        property string address: ""
        app: win.app
        dialogTitle: "Remove this email address?"
        message: "Remove " + removeEmailConfirm.address + "? It's what lets you reset your password if you ever forget it."
        confirmText: "Remove"
        destructive: true
        onConfirmed: win.app.settings.removeEmail(removeEmailConfirm.address)
    }
    // selfhost: Devices & security → Your account → Password → "Change…" (password.rs; the web's twin)
    ChangePasswordDialog {
        id: changePasswordDialog
        app: win.app
        onChanged: (text) => win.toast(text, "long")
    }
    // 0.8.33: which outputs the echo canceller listens to (the call bar's popup, here in this window)
    EcOutputsPopup { id: ecPop; app: win.app; ctl: win.app.callctl }
    // the join sound's self-audible preview (the soundboard's publish path is silent outside a call)
    MediaPlayer {
        id: joinPreview
        audioOutput: AudioOutput { volume: 1.0 }
        property string playingUrl: ""
        onMediaStatusChanged: if (mediaStatus === MediaPlayer.EndOfMedia || mediaStatus === MediaPlayer.InvalidMedia) playingUrl = ""
        onErrorOccurred: function (err, msg) { console.warn("[joinsound] preview error:", err, msg); playingUrl = "" }
        function toggle(url) {
            if (!url || url.length === 0) return
            if (playingUrl === url) { stop(); playingUrl = ""; return }
            stop(); source = win.app.soundboardClipUrl(url); playingUrl = url; play()   // via the clip cache
        }
    }

    // GUI-verification hooks (screenshot automation, like Main.qml's --tinhat-pane):
    //   --tinhat-settings-search=<q>   types <q> into the search once the window shows
    //   --tinhat-settings-jump=<id>    jumps to a row (the flash) once the window shows
    //   --tinhat-settings-draft[=<name>]  puts an unsaved display name (and about line) in the Profile draft (the save bar)
    readonly property var _hooks: {
        var a = Qt.application.arguments, o = ({})
        for (var i = 0; i < a.length; i++) {
            var s = String(a[i])
            if (s.indexOf("--tinhat-settings-search=") === 0) o.search = s.substring("--tinhat-settings-search=".length)
            else if (s.indexOf("--tinhat-settings-jump=") === 0) o.jump = s.substring("--tinhat-settings-jump=".length)
            else if (s === "--tinhat-settings-draft") o.draft = true
            else if (s.indexOf("--tinhat-settings-draft=") === 0) { o.draft = true; o.draftName = s.substring("--tinhat-settings-draft=".length) }
        }
        return o
    }
    property bool _hooksDone: false
    Timer {
        id: hookT
        interval: 1500
        running: win.visible && !win._hooksDone && (win._hooks.search !== undefined || win._hooks.jump !== undefined || win._hooks.draft === true)
        onTriggered: {
            win._hooksDone = true
            if (win._hooks.draft === true) {
                if (win._hooks.draftName !== undefined) { nameField.text = win._hooks.draftName; bioField.text = "" }
                else { nameField.text = (win.sName.length > 0 ? win.sName : "Me") + " (new)"; bioField.text = "Trying out the new Settings window" }
            }
            if (win._hooks.search !== undefined) { searchField.forceActiveFocus(); win.query = win._hooks.search }
            if (win._hooks.jump !== undefined) win.jumpTo(win._hooks.jump)
        }
    }

    // ---- small house pieces ----
    // a destructive action as an outlined button (settings.danger.*): Sign out, Revoke, Remove
    component DangerButton: ThButton {
        id: db
        flat: true
        background: Rectangle {
            radius: 8
            color: db.pressed ? Th.buttonGhostBgPressed : (db.hovered ? Th.buttonGhostBgHover : "transparent")
            border.width: db.activeFocus ? 2 : 1
            border.color: db.activeFocus ? Th.buttonBorderFocus : Th.settingsDangerBorder
        }
        contentItem: Text {
            text: db.text
            font: db.font
            color: Th.settingsDangerFg
            horizontalAlignment: Text.AlignHCenter
            verticalAlignment: Text.AlignVCenter
        }
    }
    // a live level meter at a fixed height (the rows do not jump while it moves)
    component MeterBox: Item {
        id: mb
        property real level: 0
        property bool live: false
        implicitWidth: 16 * 7
        implicitHeight: 38
        Layout.preferredHeight: 38
        LevelMeter { anchors.verticalCenter: parent.verticalCenter; level: mb.level; barColor: Th.settingsMeterFill; opacity: mb.live ? 1 : 0.35 }
    }

    // ======================================================================================
    FocusScope {
        id: rootScope
        anchors.fill: parent
        focus: true
        // Ctrl+F is this window's: claim it before any Find shortcut of the app (a Kirigami search field's) takes it
        Keys.onShortcutOverride: (event) => {
            if (event.key === Qt.Key_F && (event.modifiers & (Qt.ControlModifier | Qt.AltModifier | Qt.MetaModifier)) === Qt.ControlModifier)
                event.accepted = true
        }
        Keys.onPressed: (event) => {
            var mods = event.modifiers & (Qt.ControlModifier | Qt.AltModifier | Qt.MetaModifier)
            if (event.key === Qt.Key_Escape) {
                event.accepted = true
                if (win.searching) { win.query = ""; searchField.forceActiveFocus() }
                else win.close()
            } else if ((event.key === Qt.Key_F && mods === Qt.ControlModifier) || (event.text === "/" && mods === 0)) {
                event.accepted = true
                searchField.forceActiveFocus(); searchField.selectAll()
            }
        }

        RowLayout {
            anchors.fill: parent
            spacing: 0

            // ---- rail -------------------------------------------------------------------------------------------
            Rectangle {
                Layout.preferredWidth: 240
                Layout.fillHeight: true
                color: Th.settingsWindowRailBg
                Rectangle { anchors.right: parent.right; width: 1; height: parent.height; color: Th.settingsWindowRailDivider }

                // search field (§3): 34 tall, radius 8, magnifier, clear ✕ while non-empty
                Rectangle {
                    id: searchBox
                    x: 12; y: 12
                    width: parent.width - 24 - 1
                    height: 34
                    radius: 8
                    color: Th.settingsSearchBg
                    border.width: 1
                    border.color: searchField.activeFocus ? Th.settingsSearchBorderFocus : Th.settingsSearchBorder
                    ThIcon {
                        x: 10
                        anchors.verticalCenter: parent.verticalCenter
                        width: 16; height: 16
                        app: win.app
                        name: "edit-find-symbolic"
                        color: Th.settingsSearchIcon
                    }
                    T.TextField {
                        id: searchField
                        x: 32
                        width: parent.width - 32 - (clearBtn.visible ? 30 : 8)
                        height: parent.height
                        verticalAlignment: Text.AlignVCenter
                        color: Th.settingsSearchFg
                        selectionColor: Th.inputSelectionBg
                        selectedTextColor: Th.inputSelectionFg
                        selectByMouse: true
                        font.pixelSize: Math.round(13 * Th.textScale)
                        placeholderText: "Search settings"
                        Accessible.name: "Search settings"
                        onTextEdited: win.query = text
                        Text {
                            anchors.fill: parent
                            verticalAlignment: Text.AlignVCenter
                            visible: searchField.length === 0 && !searchField.preeditText
                            text: searchField.placeholderText
                            color: Th.settingsSearchPlaceholderFg
                            font: searchField.font
                            elide: Text.ElideRight
                        }
                        Keys.onDownPressed: (event) => { if (win.searching) { win.selIndex = Math.min(win.results.length - 1, win.selIndex + 1); resultsList.positionViewAtIndex(win.selIndex, ListView.Contain) } else rail.forceActiveFocus() }
                        Keys.onUpPressed: (event) => { if (win.searching) { win.selIndex = Math.max(0, win.selIndex - 1); resultsList.positionViewAtIndex(win.selIndex, ListView.Contain) } }
                        Keys.onReturnPressed: win.openResult(win.selIndex)
                        Keys.onEnterPressed: win.openResult(win.selIndex)
                    }
                    ThToolButton {
                        id: clearBtn
                        anchors.right: parent.right
                        anchors.rightMargin: 3
                        anchors.verticalCenter: parent.verticalCenter
                        implicitWidth: 26; implicitHeight: 26
                        visible: searchField.text.length > 0
                        app: win.app
                        iconName: "window-close-symbolic"
                        iconSize: 14
                        iconColor: Th.settingsSearchIcon
                        tip: "Clear the search"
                        onClicked: { win.query = ""; searchField.forceActiveFocus() }
                    }
                }

                Flickable {
                    id: railFlick
                    anchors.top: searchBox.bottom
                    anchors.bottom: parent.bottom
                    width: parent.width - 1
                    clip: true
                    contentHeight: rail.implicitHeight + 12
                    boundsBehavior: Flickable.StopAtBounds
                    Controls.ScrollBar.vertical: FoxScrollBar { app: win.app }
                    ColumnLayout {
                        id: rail
                        x: 12
                        width: railFlick.width - 24
                        spacing: 0
                        focus: true
                        Keys.onUpPressed: { var ks = win.paneKeys, i = ks.indexOf(win.currentKey); win.query = ""; win.currentKey = ks[(i + ks.length - 1) % ks.length] }
                        Keys.onDownPressed: { var ks = win.paneKeys, i = ks.indexOf(win.currentKey); win.query = ""; win.currentKey = ks[(i + 1) % ks.length] }
                        Repeater {
                            model: win.groups
                            delegate: ColumnLayout {
                                id: grp
                                required property var modelData
                                Layout.fillWidth: true
                                spacing: 0
                                visible: grp.modelData.keys.some(function (k) { return win.paneKeys.indexOf(k) >= 0 })
                                Controls.Label {
                                    Layout.fillWidth: true
                                    Layout.topMargin: 16
                                    Layout.bottomMargin: 6
                                    leftPadding: 10
                                    text: String(grp.modelData.g).toUpperCase()
                                    color: Th.settingsGroupFg
                                    font.pixelSize: Math.round(11 * Th.textScale)
                                    font.weight: Font.DemiBold
                                    font.letterSpacing: 0.6
                                }
                                Repeater {
                                    model: grp.modelData.keys
                                    delegate: T.ItemDelegate {
                                        id: railItem
                                        required property string modelData
                                        readonly property var info: win.paneInfo[railItem.modelData]
                                        readonly property bool selected: !win.searching && win.currentKey === railItem.modelData
                                        Layout.fillWidth: true
                                        Layout.bottomMargin: 2
                                        visible: win.paneKeys.indexOf(railItem.modelData) >= 0
                                        implicitHeight: 36
                                        hoverEnabled: true
                                        focusPolicy: Qt.TabFocus
                                        leftPadding: 10
                                        rightPadding: 10
                                        Accessible.name: railItem.info.t
                                        Accessible.description: railItem.info.d
                                        onClicked: { win.query = ""; win.currentKey = railItem.modelData; rail.forceActiveFocus() }
                                        contentItem: RowLayout {
                                            spacing: 10
                                            ThIcon {
                                                Layout.preferredWidth: 18; Layout.preferredHeight: 18
                                                app: win.app
                                                name: railItem.info.i
                                                color: railItem.selected ? Th.settingsRailItemFgSelected : Th.settingsRailItemFg
                                                hovered: railItem.hovered
                                                active: railItem.selected
                                            }
                                            Controls.Label {
                                                Layout.fillWidth: true
                                                text: railItem.info.t
                                                color: railItem.selected ? Th.settingsRailItemFgSelected : Th.settingsRailItemFg
                                                font.pixelSize: Math.round(13 * Th.textScale)
                                                font.weight: railItem.selected ? Font.DemiBold : Font.Normal
                                            }
                                        }
                                        background: Rectangle {
                                            radius: 8
                                            color: railItem.selected ? Th.settingsRailItemBgSelected
                                                 : railItem.hovered ? Th.settingsRailItemBgHover : "transparent"
                                            border.width: railItem.visualFocus ? 1 : 0
                                            border.color: Th.settingsRailItemBorderFocus
                                            Behavior on color { ColorAnimation { duration: Motion.dur } }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // ---- content ----------------------------------------------------------------------------------------
            Item {
                id: contentArea
                Layout.fillWidth: true
                Layout.fillHeight: true

                // ================================ search results ================================
                ListView {
                    id: resultsList
                    visible: win.searching
                    x: 28
                    width: win.innerWidth
                    height: parent.height
                    topMargin: 24
                    bottomMargin: 32
                    clip: true
                    model: win.results
                    spacing: 2
                    currentIndex: win.selIndex
                    boundsBehavior: Flickable.StopAtBounds
                    Controls.ScrollBar.vertical: FoxScrollBar { app: win.app }
                    header: ColumnLayout {
                        width: resultsList.width
                        spacing: 4
                        Controls.Label {
                            Layout.fillWidth: true
                            text: "Search results"
                            color: Th.settingsPaneTitleFg
                            font.pixelSize: Math.round(22 * Th.textScale)
                            font.weight: Font.DemiBold
                        }
                        Controls.Label {
                            Layout.fillWidth: true
                            Layout.bottomMargin: 16
                            text: win.results.length === 0 ? "No settings match “" + win.query.trim() + "”."
                                                           : (win.results.length === 1 ? "1 setting matches" : win.results.length + " settings match")
                                                             + " “" + win.query.trim() + "”."
                            color: win.results.length === 0 ? Th.settingsResultEmptyFg : Th.settingsPaneDescriptionFg
                            font.pixelSize: Math.round(13 * Th.textScale)
                            wrapMode: Text.Wrap
                        }
                    }
                    delegate: Rectangle {
                        id: res
                        required property var modelData
                        required property int index
                        width: resultsList.width
                        height: 44
                        radius: 8
                        color: (resMa.containsMouse || win.selIndex === res.index) ? Th.settingsResultBgHover : "transparent"
                        RowLayout {
                            anchors.fill: parent
                            anchors.leftMargin: 12
                            anchors.rightMargin: 12
                            spacing: 12
                            ThIcon {
                                Layout.preferredWidth: 18; Layout.preferredHeight: 18
                                app: win.app
                                name: win.paneInfo[res.modelData.pane] ? win.paneInfo[res.modelData.pane].i : ""
                                color: Th.settingsResultPathFg
                                hovered: resMa.containsMouse
                            }
                            ColumnLayout {
                                Layout.fillWidth: true
                                spacing: 1
                                Controls.Label {
                                    Layout.fillWidth: true
                                    text: String(res.modelData.labelHtml)
                                    textFormat: Text.StyledText
                                    color: Th.settingsResultFg
                                    font.pixelSize: Math.round(14 * Th.textScale)
                                    elide: Text.ElideRight
                                }
                                Controls.Label {
                                    Layout.fillWidth: true
                                    text: res.modelData.paneTitle + " › " + res.modelData.section
                                    color: Th.settingsResultPathFg
                                    font.pixelSize: Math.round(12 * Th.textScale)
                                    elide: Text.ElideRight
                                }
                            }
                        }
                        MouseArea {
                            id: resMa
                            anchors.fill: parent
                            hoverEnabled: true
                            cursorShape: Qt.PointingHandCursor
                            onClicked: win.openResult(res.index)
                        }
                    }
                }

                // ================================ Profile ================================
                SettingsPage {
                    id: profilePage
                    anchors.fill: parent
                    visible: !win.searching && win.currentKey === "profile"
                    app: win.app
                    title: win.paneInfo.profile.t
                    description: win.paneInfo.profile.d
                    bottomSpace: win.saveBarShown ? 72 : 0

                    SettingsSection {
                        SettingsRow {
                            app: win.app
                            anchorId: "profile.photo"
                            label: win.sName.length > 0 ? win.sName : win.app.localpart(win.sUserId)
                            labelPixelSize: 17
                            labelWeight: Font.DemiBold
                            description: win.sUserId
                            leading: Avatar {
                                Layout.preferredWidth: 72; Layout.preferredHeight: 72
                                app: win.app
                                size: 72
                                name: win.sName.length > 0 ? win.sName : win.app.localpart(win.sUserId)
                                seed: win.sUserId
                                imageSource: win.sAvatar
                            }
                            trailing: [
                                ThButton { app: win.app; text: "Change photo…"; icon.name: "insert-image-symbolic"; enabled: !win.sBusy; onClicked: avatarDialog.open() },
                                ThButton { app: win.app; flat: true; text: "Remove"; visible: win.sAvatar.length > 0; enabled: !win.sBusy; onClicked: removeAvatarConfirm.open() }
                            ]
                        }
                    }
                    SettingsSection {
                        title: "Profile"
                        SettingsRow {
                            app: win.app
                            anchorId: "profile.display-name"
                            label: "Display name"
                            description: "What people see next to your messages."
                            ThTextField {
                                id: nameField
                                Layout.fillWidth: true
                                enabled: !win.sBusy
                                placeholderText: "Your name"
                                maximumLength: 255
                                Accessible.name: "Display name"
                                onAccepted: win.saveProfile()
                            }
                        }
                        SettingsRow {
                            app: win.app
                            anchorId: "profile.about-me"
                            label: "About me"
                            description: "A short line other people can see on your profile."
                            ThTextField {
                                id: bioField
                                Layout.fillWidth: true
                                enabled: !win.sBusy
                                placeholderText: "Say something about yourself"
                                Accessible.name: "About me"
                                onAccepted: win.saveProfile()
                            }
                        }
                        SettingsRow {
                            app: win.app
                            anchorId: "profile.colour"
                            label: "Your colour"
                            description: "A personal colour for your profile card."
                            trailing: [
                                Repeater {
                                    // the colours people pick are wire data (other people's UI paints them)
                                    model: ["#7aa2f7", "#bb9af7", "#9ece6a", "#e0af68", "#f7768e", "#2ac3de"]   // theme-lint: ok content: profile colour choices
                                    delegate: Rectangle {
                                        id: sw
                                        required property string modelData
                                        readonly property bool picked: win.dAccent.toLowerCase() === sw.modelData
                                        implicitWidth: 26; implicitHeight: 26
                                        radius: 13
                                        color: sw.modelData
                                        border.width: sw.picked ? 3 : 1
                                        border.color: sw.picked ? Th.settingsSwatchBorderSelected : Th.settingsSwatchBorder
                                        Accessible.role: Accessible.RadioButton
                                        Accessible.name: "Colour " + sw.modelData
                                        MouseArea {
                                            anchors.fill: parent
                                            enabled: !win.sBusy
                                            cursorShape: Qt.PointingHandCursor
                                            onClicked: win.dAccent = sw.modelData
                                        }
                                    }
                                },
                                ThButton { app: win.app; flat: true; text: "Clear"; visible: win.dAccent.length > 0; enabled: !win.sBusy; onClicked: win.dAccent = "" }
                            ]
                        }
                    }
                    SettingsSection {
                        visible: win.app.serverSoundboard   // selfhost: the soundboard hosts join sounds
                        title: "Join sound"
                        hint: "Played for everyone already in a call when you join — trimmed to 3 seconds, at their soundboard volume, so people can turn it down."
                        SettingsRow {
                            app: win.app
                            anchorId: "profile.join-sound"
                            label: "Join sound"
                            description: "Pick one of your sounds, or search for a new one."
                            RowLayout {
                                Layout.fillWidth: true
                                spacing: 8
                                ThComboBox {
                                    id: joinSoundBox
                                    Layout.fillWidth: true
                                    enabled: !win.sBusy
                                    textRole: "name"
                                    Accessible.name: "Join sound"
                                    // your personal sounds + an explicit "none"; a set-but-unlisted sound never reads as "No join sound"
                                    model: {
                                        var out = [{ name: "No join sound", url: "" }], found = false
                                        var ps = win.app.mPersonalSounds || []
                                        for (var i = 0; i < ps.length; i++) {
                                            var u = ps[i].sourceUrl || ps[i].url || ""
                                            out.push({ name: ps[i].name || "Sound " + (i + 1), url: u })
                                            if (u === win.sJoinSound) found = true
                                        }
                                        if (!found && win.sJoinSound.length > 0) out.push({ name: "Current join sound", url: win.sJoinSound })
                                        return out
                                    }
                                    currentIndex: {
                                        for (var i = 0; i < model.length; i++) if (model[i].url === win.sJoinSound) return i
                                        return 0
                                    }
                                    onActivated: (i) => { joinPreview.stop(); joinPreview.playingUrl = ""; win.app.settings.saveJoinSound(model[i].url) }
                                }
                                ThButton {
                                    app: win.app
                                    readonly property bool playing: joinPreview.playingUrl === win.sJoinSound && win.sJoinSound.length > 0
                                    text: playing ? "Stop" : "Preview"
                                    icon.name: playing ? "media-playback-stop" : "media-playback-start"
                                    enabled: win.sJoinSound.length > 0
                                    onClicked: joinPreview.toggle(win.sJoinSound)
                                }
                            }
                            // The same search the soundboard has (myinstants via the proxy)
                            ThTextField {
                                id: joinSearchField
                                Layout.fillWidth: true
                                enabled: !win.sBusy
                                placeholderText: "Search sound effects…"
                                Accessible.name: "Search sound effects"
                                onTextChanged: win.app.joinSoundCtl.search(text)
                            }
                            Controls.Label {
                                Layout.fillWidth: true
                                visible: win.app.mJoinSoundSearching || (joinSearchField.text.length > 0 && win.app.mJoinSoundSearch.length === 0)
                                text: win.app.mJoinSoundSearching ? "Searching…" : "No results"
                                color: Th.settingsHintFg
                                font.pixelSize: Math.round(12 * Th.textScale)
                            }
                            ListView {
                                id: joinResults
                                Layout.fillWidth: true
                                Layout.preferredHeight: Math.min(win.app.mJoinSoundSearch.length, 5) * 38
                                visible: joinSearchField.text.length > 0 && win.app.mJoinSoundSearch.length > 0
                                clip: true
                                interactive: contentHeight > height
                                model: win.app.mJoinSoundSearch
                                spacing: 2
                                delegate: Rectangle {
                                    id: jsRow
                                    required property var modelData
                                    readonly property string jsUrl: modelData.url || ""
                                    readonly property bool jsPlaying: jsUrl.length > 0 && jsUrl === joinPreview.playingUrl
                                    readonly property bool jsChosen: jsUrl.length > 0 && jsUrl === win.sJoinSound
                                    width: ListView.view.width
                                    height: 36
                                    radius: 8
                                    color: jsPlaying ? Th.settingsSoundRowBgActive : (jsMa.containsMouse ? Th.settingsSoundRowBgHover : "transparent")
                                    MouseArea {
                                        id: jsMa
                                        anchors.fill: parent
                                        hoverEnabled: true
                                        cursorShape: Qt.PointingHandCursor
                                        onClicked: joinPreview.toggle(jsRow.jsUrl)   // a row click auditions it; again stops
                                    }
                                    RowLayout {
                                        anchors.fill: parent
                                        anchors.leftMargin: 8
                                        anchors.rightMargin: 4
                                        spacing: 8
                                        ThIcon {
                                            Layout.preferredWidth: 16; Layout.preferredHeight: 16
                                            app: win.app
                                            name: jsRow.jsPlaying ? "media-playback-stop" : "media-playback-start"
                                            color: jsRow.jsPlaying ? Th.settingsSoundRowIconActive : Th.settingsSoundRowIcon
                                        }
                                        Controls.Label {
                                            Layout.fillWidth: true
                                            text: jsRow.modelData.name || jsRow.jsUrl
                                            color: Th.settingsSoundRowFg
                                            font.pixelSize: Math.round(13 * Th.textScale)
                                            elide: Text.ElideRight
                                        }
                                        ThButton {
                                            app: win.app
                                            flat: true
                                            implicitHeight: 28
                                            text: jsRow.jsChosen ? "Join sound" : "Use"
                                            icon.name: jsRow.jsChosen ? "checkmark-symbolic" : ""
                                            enabled: !jsRow.jsChosen && jsRow.jsUrl.length > 0
                                            // added to My sounds as well, so it keeps a name in the picker (and the soundboard)
                                            onClicked: {
                                                var known = false, ps = win.app.mPersonalSounds || []
                                                for (var i = 0; i < ps.length; i++) if ((ps[i].sourceUrl || ps[i].url || "") === jsRow.jsUrl) { known = true; break }
                                                if (!known) win.app.soundboardCtl.addPersonalSound(jsRow.modelData.name || "Sound", jsRow.jsUrl, jsRow.modelData.emoji || "",
                                                                                                   Math.floor(Number(jsRow.modelData.duration) || 0))
                                                win.app.settings.saveJoinSound(jsRow.jsUrl)
                                                win.sJoinSound = jsRow.jsUrl
                                            }
                                        }
                                    }
                                }
                            }
                            Controls.Label {
                                Layout.fillWidth: true
                                visible: (win.app.mPersonalSounds || []).length === 0 && joinSearchField.text.length === 0
                                text: "Search above to find a sound, or add some to your soundboard first."
                                color: Th.settingsHintFg
                                font.pixelSize: Math.round(12 * Th.textScale)
                                wrapMode: Text.Wrap
                            }
                        }
                    }
                    SettingsSection {
                        title: "Email address"
                        hint: "Used to reset your password if you ever lose it. Only the server sees it."
                        Repeater {
                            model: win.emailList
                            delegate: SettingsRow {
                                required property var modelData
                                app: win.app
                                iconName: "mail-message-symbolic"
                                label: modelData.address || ""
                                trailing: DangerButton { app: win.app; text: "Remove"; enabled: !win.sBusy; onClicked: { removeEmailConfirm.address = modelData.address; removeEmailConfirm.open() } }
                            }
                        }
                        SettingsRow {
                            app: win.app
                            anchorId: "profile.email"
                            label: win.emailList.length > 0 ? "Add another address" : "Add an address"
                            description: (win.app.settings.emailStatus || "").length > 0 ? win.app.settings.emailStatus : "We mail you a link to confirm it."
                            RowLayout {
                                Layout.fillWidth: true
                                spacing: 8
                                ThTextField {
                                    id: emailField
                                    Layout.fillWidth: true
                                    placeholderText: "you@example.com"
                                    enabled: !win.sBusy
                                    inputMethodHints: Qt.ImhEmailCharactersOnly | Qt.ImhNoAutoUppercase
                                    Accessible.name: "Email address"
                                    onAccepted: if (text.indexOf("@") > 0) win.app.settings.addEmail(text)
                                }
                                GlowButton {
                                    app: win.app
                                    text: "Add"
                                    busy: win.sBusy
                                    enabled: !win.sBusy && emailField.text.indexOf("@") > 0
                                    onClicked: win.app.settings.addEmail(emailField.text)
                                }
                            }
                        }
                        // the homeserver mails a link, then wants the account password before it binds the address
                        SettingsRow {
                            app: win.app
                            visible: win.app.settings.emailPending
                            label: "Confirm your address"
                            description: "Click the link we mailed you, then enter your password here."
                            RowLayout {
                                Layout.fillWidth: true
                                spacing: 8
                                ThTextField {
                                    id: emailPwField
                                    Layout.fillWidth: true
                                    placeholderText: "Your password"
                                    echoMode: TextInput.Password
                                    enabled: !win.sBusy
                                    Accessible.name: "Your password, to confirm"
                                    onAccepted: { win.app.settings.confirmEmail(text); text = "" }
                                }
                                GlowButton {
                                    app: win.app
                                    text: "Confirm"
                                    busy: win.sBusy
                                    enabled: !win.sBusy && emailPwField.text.length > 0
                                    onClicked: { win.app.settings.confirmEmail(emailPwField.text); emailPwField.text = "" }
                                }
                            }
                        }
                    }
                }

                // ================================ Privacy ================================
                // Everything here is the user's choice and nothing on this pane phones home: the recording opt-out is a
                // flag other clients honour, diagnostics only ever writes a file they can read.
                SettingsPage {
                    id: privacyPage
                    anchors.fill: parent
                    visible: !win.searching && win.currentKey === "privacy"
                    app: win.app
                    title: win.paneInfo.privacy.t
                    description: win.paneInfo.privacy.d

                    // 10.6 (docs/REACH.md §2.7 + A2.1 + A3): opt-in presence, off by default; the switch is the only
                    // thing that ever writes the choice (A2.2)
                    SettingsSection {
                        visible: win.app.serverOn("presence")   // selfhost: Reach statuses are the server's
                        title: "Your status"
                        hint: "Turning it off stops it at once — it leaves friends' screens within about 20 seconds and nothing is kept. Messages, typing and read receipts are separate; other Matrix apps may share a status on their own."
                        SettingsRow {
                            app: win.app
                            anchorId: "privacy.share-status"
                            label: win.app.mPresenceTurningOff ? "Turning off…" : "Share my status"
                            description: "Off by default. On: people who share a room with you see Online, Away or In a call — never a last-seen time or what you're doing."
                            toggle: shareStatusSw
                            trailing: ThSwitch {
                                id: shareStatusSw
                                Accessible.name: "Share my status"
                                checked: win.app.mPresenceSharing
                                enabled: win.app.mPresenceKnown && !win.app.mPresenceBusy && !win.app.mPresenceTurningOff
                                onToggled: { win.app.presenceCtl.chooseSharing(checked); checked = Qt.binding(() => win.app.mPresenceSharing) }
                            }
                        }
                        // A2.15: what the SERVER shows others (your own entry), never a local guess
                        SettingsRow {
                            app: win.app
                            visible: win.app.mPresenceSharing
                            label: "Others see you as"
                            trailing: [
                                PresenceLine {
                                    app: win.app
                                    pres: win.app.presenceOf(win.app.mUserId)
                                    roomName: (win.app.mPresenceMine === "call" && win.app.mPresenceMyRoom.length > 0) ? win.app.roomNameOf(win.app.mPresenceMyRoom) : ""
                                    pointSize: Kirigami.Theme.defaultFont.pointSize
                                    textColor: Th.settingsPresenceFg
                                },
                                Controls.Label {
                                    visible: win.app.presenceOf(win.app.mUserId) === null
                                    text: "checking…"
                                    color: Th.settingsLabelFg
                                    font.italic: true
                                }
                            ]
                        }
                    }
                    // A7 (docs/REACH.md): the time zone Friends' clocks shows — opt-OUT, on by default (the web's
                    // TimeZoneSection); absent after the first sync = the first-run write is on its way, so it reads on
                    SettingsSection {
                        visible: win.app.serverOn("presence")   // selfhost: Friends' clocks ride on Reach
                        title: "Time zone"
                        SettingsRow {
                            app: win.app
                            anchorId: "privacy.share-time-zone"
                            label: "Share my time zone"
                            description: "Friends see your local time in Friends' clocks."
                            toggle: shareTzSw
                            trailing: ThSwitch {
                                id: shareTzSw
                                readonly property var tzv: { try { return JSON.parse(String(win.app.presenceCtl.tzJson || "{}")) } catch (e) { return ({}) } }
                                readonly property bool on: shareTzSw.tzv.share === true || ((shareTzSw.tzv.share === null || shareTzSw.tzv.share === undefined) && shareTzSw.tzv.known === true)
                                Accessible.name: "Share my time zone"
                                checked: shareTzSw.on
                                enabled: shareTzSw.tzv.known === true && shareTzSw.tzv.busy !== true
                                onToggled: { win.app.presenceCtl.tzShare(checked); checked = Qt.binding(() => shareTzSw.on) }
                            }
                        }
                        SettingsRow {
                            app: win.app
                            anchorId: "privacy.time-zone"
                            label: "Your time zone"
                            description: String(shareTzSw.tzv.error || "")
                            descriptionColor: Th.statusErrorFg
                            trailing: ThComboBox {
                                id: tzCombo
                                Layout.preferredWidth: 240
                                readonly property var tzv: shareTzSw.tzv
                                readonly property string picked: (tzCombo.tzv.share !== null && tzCombo.tzv.share !== undefined && tzCombo.tzv.auto === false) ? String(tzCombo.tzv.tz || "") : ""
                                readonly property var list: { try { return JSON.parse(String(win.app.presenceCtl.tzChoices())) } catch (e) { return [] } }
                                readonly property var choices: {
                                    var out = [""], known = false
                                    for (var i = 0; i < tzCombo.list.length; ++i) { out.push(tzCombo.list[i].tz); if (tzCombo.list[i].tz === tzCombo.picked) known = true }
                                    if (tzCombo.picked.length > 0 && !known) out.push(tzCombo.picked)
                                    return out
                                }
                                model: tzCombo.choices.map(function (z) {
                                    if (z.length > 0) return String(win.app.presenceCtl.tzChoiceText(z))
                                    var dev = String(tzCombo.tzv.device || "")
                                    return dev.length > 0 ? "Automatic (" + String(win.app.presenceCtl.tzChoiceText(dev)) + ")" : "Automatic"
                                })
                                currentIndex: Math.max(0, tzCombo.choices.indexOf(tzCombo.picked))
                                enabled: tzCombo.tzv.known === true && tzCombo.tzv.busy !== true
                                Accessible.name: "Your time zone"
                                onActivated: (i) => win.app.presenceCtl.tzPick(tzCombo.choices[i])
                            }
                        }
                    }
                    SettingsSection {
                        visible: win.app.serverMemories   // selfhost: only where the server records
                        title: "Call recordings"
                        SettingsRow {
                            app: win.app
                            anchorId: "privacy.never-record"
                            label: "Never record me"
                            description: "When a call is recorded (🔴 REC in everyone's call header), your camera, screen shares and voice are left out. Takes effect when you join a call."
                            toggle: recordSw
                            trailing: ThSwitch {
                                id: recordSw
                                Accessible.name: "Never record me"
                                checked: win.app.settings.recordOptout
                                onToggled: win.app.settings.applyRecordOptout(checked)
                            }
                        }
                    }
                    SettingsSection {
                        title: "Diagnostics"
                        SettingsRow {
                            app: win.app
                            anchorId: "privacy.diagnostics"
                            label: "Diagnostic reports"
                            description: "Off by default. Nothing is collected in the background and nothing is ever uploaded."
                            toggle: diagSw
                            trailing: ThSwitch {
                                id: diagSw
                                Accessible.name: "Allow creating diagnostic reports"
                                checked: win.app.diagnostics.enabled
                                onToggled: win.app.diagnostics.setOptIn(checked)
                            }
                        }
                        SettingsRow {
                            app: win.app
                            label: "Create a report"
                            description: win.app.diagnostics.status.length > 0 ? win.app.diagnostics.status
                                : "Writes a text file to your home folder: your app log with tokens and keys stripped, plus your audio and video settings. Read it, then send it to your server's owner if you like. Messages are never logged."
                            trailing: [
                                ThButton { app: win.app; text: "Create report"; enabled: win.app.diagnostics.enabled; onClicked: win.app.diagnostics.createReport() },
                                ThButton { app: win.app; flat: true; text: "Open report"; visible: win.app.diagnostics.reportPath.length > 0; onClicked: Qt.openUrlExternally("file://" + win.app.diagnostics.reportPath) }
                            ]
                        }
                    }
                    SettingsSection {
                        visible: win.app.serverOn("reports")   // selfhost: only where the server's owner takes logs
                        title: "Automatic log sharing"
                        SettingsRow {
                            app: win.app
                            anchorId: "privacy.log-sharing"
                            label: "Send my debug logs automatically"
                            description: "On startup, uploads the previous session's app log to your server's owner so bugs get fixed. Tokens, keys and IP addresses are stripped first; messages are never logged. Off takes effect at once."
                            toggle: logshipSw
                            trailing: ThSwitch {
                                id: logshipSw
                                Accessible.name: "Send my debug logs automatically"
                                // 10.1 (TH-PREF-22): only a recorded YES shows as on; -1 (never asked) is off
                                checked: win.app.mLogship === 1
                                onToggled: { win.app.settings.applyLogship(checked); win.app.mLogship = checked ? 1 : 0 }
                            }
                        }
                    }
                }

                // ================================ Devices & security ================================
                SettingsPage {
                    id: securityPage
                    anchors.fill: parent
                    visible: !win.searching && win.currentKey === "security"
                    app: win.app
                    title: win.paneInfo.security.t
                    description: win.paneInfo.security.d

                    SettingsSection {
                        title: "This device"
                        SettingsRow {
                            app: win.app
                            anchorId: "security.verification"
                            iconName: win.sXsign ? "security-high-symbolic" : "security-low-symbolic"
                            iconColor: win.sXsign ? Th.settingsSuccessFg : Th.settingsErrorFg
                            label: "Verification"
                            chip: win.sXsign ? "Verified" : ""
                            description: win.sXsign ? "This device is cross-signed — it can read your encrypted messages."
                                                    : "Not verified — encrypted messages can't be read here yet."
                            trailing: ThButton { app: win.app; highlighted: true; text: "Verify…"; visible: !win.sXsign; onClicked: win.handOff("security", "verify") }   // 10.1 (TH-SET-11)
                        }
                        // 10.1 (TH-SET-03 / TH-GAP-03): login.setUpRecovery asks for the account password and shows the
                        // recovery key once (the verify dialog carries the progress)
                        SettingsRow {
                            id: backupRow
                            property bool setupOpen: false
                            app: win.app
                            anchorId: "security.key-backup"
                            iconName: win.sBackup ? "backup-symbolic" : "dialog-warning"
                            iconColor: win.sBackup ? Th.settingsSuccessFg : Th.stateWarning
                            label: "Key backup"
                            chip: win.sBackup ? "On" : ""
                            description: win.sBackup ? "Your encrypted history is saved on the server, so a new device can read it."
                                : (win.sXsign ? "Off — signing out or losing this device makes your encrypted history unreadable."
                                              : "Verify this device first.")
                            trailing: ThButton {
                                app: win.app
                                text: "Set up…"
                                visible: win.sXsign && !win.sBackup && !backupRow.setupOpen
                                onClicked: { backupRow.setupOpen = true; backupPwField.forceActiveFocus() }
                            }
                            ColumnLayout {
                                Layout.fillWidth: true
                                visible: backupRow.setupOpen && win.sXsign && !win.sBackup
                                spacing: 8
                                Controls.Label {
                                    Layout.fillWidth: true
                                    text: "Enter your account password. You get a recovery key to write down."
                                    color: Th.settingsHintFg
                                    font.pixelSize: Math.round(12 * Th.textScale)
                                    wrapMode: Text.Wrap
                                }
                                RowLayout {
                                    Layout.fillWidth: true
                                    spacing: 8
                                    ThTextField {
                                        id: backupPwField
                                        Layout.fillWidth: true
                                        placeholderText: "Account password"
                                        echoMode: TextInput.Password
                                        Accessible.name: "Account password"
                                        onAccepted: if (text.length > 0) { var p = text; text = ""; backupRow.setupOpen = false; win.handOff("security", "backup", p) }
                                    }
                                    GlowButton {
                                        app: win.app
                                        text: "Set up key backup"
                                        enabled: backupPwField.text.length > 0
                                        onClicked: { var p = backupPwField.text; backupPwField.text = ""; backupRow.setupOpen = false; win.handOff("security", "backup", p) }
                                    }
                                    ThButton { app: win.app; flat: true; text: "Cancel"; onClicked: { backupPwField.text = ""; backupRow.setupOpen = false } }
                                }
                            }
                        }
                    }
                    SettingsSection {
                        title: "Where you're signed in"
                        hint: "Every device and app signed in to your account, this one first."
                        // 10.1 (TH-STATE-06): never legitimately empty (this device is always in it)
                        SettingsRow {
                            app: win.app
                            anchorId: "security.devices"
                            visible: win.sDevices.length === 0
                            label: win.sBusy ? "Loading your sessions…" : "Couldn't load your sessions"
                            description: win.sBusy ? "" : "Close and reopen Settings to try again."
                        }
                        Repeater {
                            model: win.sDevices
                            delegate: SettingsRow {
                                id: devRow
                                required property var modelData
                                required property int index
                                app: win.app
                                anchorId: devRow.index === 0 ? "security.devices" : ""
                                // 10.1 (TH-SET-08): the glyph follows the device NAME; when it was last active; the id in a tooltip
                                iconName: /phone|android|ios|iphone|ipad|element x|mobile/i.test(devRow.modelData.name || "") ? "smartphone-symbolic" : "computer-symbolic"
                                iconColor: devRow.modelData.isCurrent ? Th.settingsSessionFgOwn : Th.settingsRowDescriptionFg
                                label: (devRow.modelData.name && devRow.modelData.name.length > 0) ? devRow.modelData.name : devRow.modelData.id
                                chip: devRow.modelData.isCurrent ? "This device" : ""
                                description: (devRow.modelData.lastSeenTs > 0 ? "Last active " + win.app._agoText(devRow.modelData.lastSeenTs) : "")
                                             + (devRow.modelData.lastSeenIp && devRow.modelData.lastSeenIp.length > 0 ? (devRow.modelData.lastSeenTs > 0 ? "  ·  " : "") + devRow.modelData.lastSeenIp : "")
                                HoverHandler { id: devHover }
                                Controls.ToolTip.text: "Session " + devRow.modelData.id
                                Controls.ToolTip.visible: devHover.hovered
                                Controls.ToolTip.delay: 600
                            }
                        }
                    }
                    // selfhost (2026-10-22): a self-hosted server sends no email — its owner hands out a new password,
                    // you choose your own here (every server: standard Matrix; never for a temporary guest account, whose
                    // password is the server's: server_offer GUEST_HIDDEN_SETTINGS)
                    SettingsSection {
                        visible: !win.app.mGuestAccount
                        title: "Your account"
                        SettingsRow {
                            app: win.app
                            anchorId: "security.password"   // settings-index.json (the search lands here)
                            iconName: "lock-symbolic"   // as the web's row
                            label: "Password"
                            description: "Change the password you sign in with"
                            trailing: ThButton { app: win.app; text: "Change…"; onClicked: changePasswordDialog.open() }
                        }
                    }
                }

                // ================================ Invites (operators) ================================
                // 10.4 (docs/INVITES.md): codes are Synapse registration tokens minted by the Tin Hat server; "Create an
                // account" on the sign-in screen redeems them
                SettingsPage {
                    id: invitesPage
                    anchors.fill: parent
                    visible: !win.searching && win.currentKey === "invites"
                    app: win.app
                    title: win.paneInfo.invites.t
                    description: win.paneInfo.invites.d

                    SettingsSection {
                        title: "Invite someone"
                        hint: "A code lets someone create an account on " + (win.app.mInviteServer.length > 0 ? win.app.mInviteServer : "this server")
                              + " from Tin Hat's sign-in screen (“Create an account”). They type just the 8 characters — the TH- is already in the box."
                        SettingsRow {
                            app: win.app
                            label: "Good for"
                            trailing: ThComboBox { id: inviteUses; Layout.preferredWidth: 240; model: ["1 person", "5 people", "10 people", "Unlimited"]; Accessible.name: "Good for" }
                        }
                        SettingsRow {
                            app: win.app
                            label: "Valid for"
                            trailing: ThComboBox { id: inviteDays; Layout.preferredWidth: 240; model: ["7 days", "30 days", "90 days", "No expiry"]; Accessible.name: "Valid for" }
                        }
                        SettingsRow {
                            app: win.app
                            anchorId: "invites.create-invite"
                            label: "Create invite"
                            description: "Then copy the invite message below and send it to them."
                            trailing: GlowButton {
                                app: win.app
                                text: "Create"
                                busy: win.app.mInviteBusy
                                enabled: !win.app.mInviteBusy
                                onClicked: win.app.invitesCtl.create([1, 5, 10, 0][inviteUses.currentIndex], [7, 30, 90, 0][inviteDays.currentIndex])
                            }
                        }
                    }
                    SettingsSection {
                        title: "Codes" + (win.app.mInvites.length > 0 ? " · " + win.app.mInvites.length : "")
                        hint: "Revoking a code stops anyone still holding it; accounts already made keep working. Used-up and expired codes stay here, dimmed, until you remove them."
                        headerTrailing: ThToolButton { implicitWidth: 26; implicitHeight: 26; iconSize: 16; app: win.app; iconName: "view-refresh-symbolic"; tip: "Refresh"; enabled: !win.app.mInviteBusy; onClicked: win.app.invitesCtl.refresh() }
                        SettingsRow {
                            app: win.app
                            anchorId: "invites.codes"
                            visible: win.app.mInvites.length === 0
                            label: "No invite codes yet"
                        }
                        Repeater {
                            model: win.app.mInvites
                            delegate: SettingsRow {
                                id: invRow
                                required property var modelData
                                required property int index
                                app: win.app
                                anchorId: invRow.index === 0 ? "invites.codes" : ""
                                opacity: invRow.modelData.active ? 1 : 0.55
                                labelFormat: Text.StyledText
                                labelFontFamily: Th.fontMono
                                labelWeight: Font.Bold
                                label: (/^TH-/i.test(String(invRow.modelData.code)) ? "<font color=\"" + Th.settingsInviteCodePrefixFg + "\">TH-</font>" : "")
                                       + win.app.inviteBlip(invRow.modelData.code)
                                labelColor: invRow.modelData.active ? Th.settingsInviteCodeFg : Th.settingsInviteCodeFgOff
                                description: win.app.inviteSummary(invRow.modelData)
                                trailing: [
                                    ThButton { app: win.app; flat: true; visible: invRow.modelData.active; text: "Copy code"
                                                 onClicked: { win.app.copyToClipboard(win.app.inviteBlip(invRow.modelData.code)); win.toast("Copied " + win.app.inviteBlip(invRow.modelData.code), "short") } },
                                    ThButton { app: win.app; flat: true; visible: invRow.modelData.active; text: "Copy message"
                                                 onClicked: { win.app.copyToClipboard(win.app.inviteMessage(invRow.modelData)); win.toast("Invite message copied — paste it to them", "short") } },
                                    DangerButton { app: win.app; enabled: !win.app.mInviteBusy; text: invRow.modelData.active ? "Revoke" : "Remove"; onClicked: win.app.invitesCtl.revoke(invRow.modelData.code) }
                                ]
                            }
                        }
                    }
                }

                // ================================ Guests (selfhost) ================================
                // docs/GUESTS.md "The apps" 1 (the web's GuestsPane.tsx): what the guest room is (two sentences), its state,
                // and the buttons this user may use — managers Turn on the guest room / Turn off · Copy guest link · New link
                // · Open the Guests room; members Copy guest link · Open the Guests room; everyone else Open the Guests room
                // once it exists. The plan, the words and on/off as sync has it come from guests_ctl.rs (view_json).
                SettingsPage {
                    id: guestsPage
                    anchors.fill: parent
                    visible: !win.searching && win.currentKey === "guests"
                    app: win.app
                    title: win.paneInfo.guests.t
                    description: win.paneInfo.guests.d
                    readonly property var g: win.app.mGuests || ({})
                    readonly property var w: guestsPage.g.words || ({})
                    readonly property var plan: guestsPage.g.pane || null
                    readonly property var buttons: guestsPage.plan ? guestsPage.plan.buttons : []
                    function run(id) {
                        switch (id) {
                        case "turnOn": win.app.guestsCtl.turnOn(); break
                        case "turnOff": win.app.guestsCtl.turnOff(); break
                        case "copy": win.app.guestsCtl.copyLink(); break
                        case "newLink": win.app.guestsCtl.newLink(); break
                        case "open": win.app.guestsCtl.openRoom(); break   // joins it first when needed; Main.qml closes Settings (onOpenReady)
                        }
                    }

                    SettingsSection {
                        title: String(guestsPage.w.section || "")
                        hint: String(guestsPage.w.explain || "")
                        // no doc yet: "Checking…", or why there is none (the server's words)
                        SettingsRow {
                            app: win.app
                            visible: !guestsPage.plan
                            label: String(guestsPage.g.note || "")
                            labelColor: guestsPage.g.noteError === true ? Th.settingsDangerFg : Th.settingsRowLabelFg
                        }
                        SettingsRow {
                            app: win.app
                            anchorId: "guests.state"   // settings-index.json (the search lands here)
                            visible: !!guestsPage.plan
                            label: guestsPage.plan ? String(guestsPage.plan.text) : ""
                            // one or two buttons beside the text; more (a manager's four) under it, wrapping
                            trailing: Repeater {
                                model: guestsPage.buttons.length <= 2 ? guestsPage.buttons : []
                                delegate: ThButton {
                                    required property var modelData
                                    app: win.app
                                    text: String(modelData.label)
                                    highlighted: modelData.highlighted === true
                                    flat: modelData.flat === true
                                    enabled: modelData.enabled === true
                                    onClicked: guestsPage.run(String(modelData.id))
                                }
                            }
                            Flow {
                                Layout.fillWidth: true
                                visible: guestsPage.buttons.length > 2
                                spacing: 8
                                Repeater {
                                    model: guestsPage.buttons.length > 2 ? guestsPage.buttons : []
                                    delegate: ThButton {
                                        required property var modelData
                                        app: win.app
                                        text: String(modelData.label)
                                        highlighted: modelData.highlighted === true
                                        flat: modelData.flat === true
                                        enabled: modelData.enabled === true
                                        onClicked: guestsPage.run(String(modelData.id))
                                    }
                                }
                            }
                        }
                        // the link as text too (selectable, to copy by hand)
                        SettingsRow {
                            app: win.app
                            anchorId: "guests.link"
                            visible: !!guestsPage.plan && guestsPage.plan.showLink === true && String(guestsPage.g.link || "").length > 0
                            label: String(guestsPage.w.link || "")
                            TextEdit {
                                Layout.fillWidth: true
                                readOnly: true
                                selectByMouse: true
                                wrapMode: TextEdit.WrapAnywhere
                                text: String(guestsPage.g.link || "")
                                color: Th.settingsRowDescriptionFg
                                selectionColor: Th.inputSelectionBg
                                selectedTextColor: Th.inputSelectionFg
                                font.family: Th.fontMono
                                font.pixelSize: Math.round(12 * Th.textScale)
                                Accessible.name: String(guestsPage.w.link || "")
                            }
                        }
                    }
                }

                // ================================ Audio ================================
                // (G) device + share-source choice lives here; the in-call chevrons keep their quick copies
                SettingsPage {
                    id: audioPage
                    anchors.fill: parent
                    visible: !win.searching && win.currentKey === "audio"
                    app: win.app
                    title: win.paneInfo.audio.t
                    description: win.paneInfo.audio.d

                    SettingsSection {
                        title: "Microphone"
                        SettingsRow {
                            app: win.app
                            anchorId: "audio.input-device"
                            label: "Input device"
                            trailing: ThComboBox {
                                Layout.preferredWidth: 240
                                textRole: "label"
                                Accessible.name: "Input device"
                                model: [{ node: "", label: "System default" }].concat(win.app.mInputs)
                                currentIndex: { for (var i = 0; i < model.length; i++) if (model[i].node === win.app.mInputName) return i; return 0 }
                                onActivated: (i) => win.app.callctl.setInput(model[i].node)
                            }
                        }
                        SettingsRow {
                            app: win.app
                            anchorId: "audio.input-level"
                            label: "Input level"
                            // 10.1 (TH-SET-12)
                            description: (!win.app.callctl.micMonitoring && win.audioSettled)
                                         ? "No signal from this microphone — it may be unplugged or in use by another app. Pick another one above."
                                         : "Talk and watch the bars move."
                            MeterBox { level: win.app.mMicLevel; live: win.app.callctl.micMonitoring }
                        }
                        // A lit switch while the canceller is DOWN reassures the one person who cannot hear it
                        SettingsRow {
                            app: win.app
                            anchorId: "audio.echo-cancellation"
                            label: win.app.mEcDegraded ? "Echo cancellation — not active" : "Echo cancellation"
                            labelColor: win.app.mEcDegraded ? Th.settingsErrorFg : Th.settingsRowLabelFg
                            description: win.app.mEcDegraded ? "It's on but not running, so others can hear their own voice come back from you. Tin Hat keeps retrying."
                                : (win.app.mMicAec && win.app.mEcNote.length > 0 && (win.app.mInCall || win.app.mEcNote.indexOf("not needed") === 0))
                                  ? "Echo cancellation: " + win.app.mEcNote
                                  : "Removes your speakers' sound from your mic. Runs inside Tin Hat (" + (win.app.mAecEngine === "pipewire-module" ? "PipeWire echo-cancel module" : "WebRTC engine") + ")."
                            toggle: aecSw
                            trailing: ThSwitch {
                                id: aecSw
                                Accessible.name: "Echo cancellation"
                                checked: win.app.mMicAec
                                onToggled: { win.app.callctl.applyMicAec(checked); checked = Qt.binding(() => win.app.mMicAec) }
                            }
                        }
                        // 0.8.33: every output that plays through the speakers (a mixer's Game + Chat + Virtual A), summed
                        SettingsRow {
                            app: win.app
                            anchorId: "audio.echo-outputs"
                            visible: win.app.mMicAec
                            label: "Speaker outputs to cancel"
                            description: win.app.mEcOutputs.length > 0 ? "Cancelling what plays on: " + win.app.ecOutputLabels()
                                                                       : "Pick every output that plays through your speakers — Tin Hat cancels all of them at once."
                            trailing: ThButton {
                                app: win.app
                                icon.name: "audio-speakers-symbolic"
                                text: "Choose…" + (win.app.mEcOutputs.length > 0 ? " (" + win.app.mEcOutputs.length + ")" : "")
                                onClicked: { win.app.callctl.refreshOutputs(); ecPop.open() }
                            }
                        }
                        SettingsRow {
                            app: win.app
                            anchorId: "audio.auto-gain"
                            label: "Auto gain control"
                            description: "Keeps your voice at a steady level when you're too quiet or too loud."
                            toggle: agcSw
                            trailing: ThSwitch {
                                id: agcSw
                                Accessible.name: "Auto gain control"
                                checked: win.app.mMicAgc
                                onToggled: { win.app.callctl.applyMicAgc(checked); checked = Qt.binding(() => win.app.mMicAgc) }
                            }
                        }
                        // ONE noise-suppression control (0.8.25): Standard = WebRTC's suppressor in the same in-app stage
                        // as the canceller; Strong = RNNoise. Exclusive on purpose — never two suppressors in series.
                        SettingsRow {
                            app: win.app
                            anchorId: "audio.noise-suppression"
                            label: "Noise suppression"
                            description: win.app.mNcAvailable
                                ? "Standard is light on the CPU; Strong (RNNoise) removes the most. If people say you sound robotic, try Off first."
                                : "Strong needs gst-plugin-rsaudiofx (part of gst-plugins-rs, not gst-plugins-bad); install it and restart Tin Hat."
                            trailing: ThComboBox {
                                id: nsBox
                                Layout.preferredWidth: 240
                                readonly property var modes: ["off", "standard", "strong"]
                                Accessible.name: "Noise suppression"
                                model: ["Off", "Standard", win.app.mNcAvailable ? "Strong (RNNoise)" : "Strong — not installed"]   // 10.1 (TH-COPY-08)
                                currentIndex: Math.max(0, nsBox.modes.indexOf(win.app.mMicNsMode === "strong" && !win.app.mNcAvailable ? "off" : win.app.mMicNsMode))
                                onActivated: (i) => {
                                    if (i === 2 && !win.app.mNcAvailable) win.toast("Strong suppression needs gst-plugin-rsaudiofx — install it and restart Tin Hat.", "long")
                                    else win.app.callctl.applyMicNsMode(nsBox.modes[i])
                                    currentIndex = Qt.binding(() => Math.max(0, nsBox.modes.indexOf(win.app.mMicNsMode === "strong" && !win.app.mNcAvailable ? "off" : win.app.mMicNsMode)))
                                }
                            }
                        }
                        SettingsRow {
                            app: win.app
                            anchorId: "audio.noise-test"
                            label: "Test noise cancellation"
                            description: "Talk and compare your raw mic with what the call hears."
                            trailing: ThButton { app: win.app; icon.name: "media-record-symbolic"; text: "Test…"; onClicked: win.handOff("audio", "noiseTest") }   // 10.1 (TH-SET-11)
                        }
                    }
                    SettingsSection {
                        title: "Speaker"
                        SettingsRow {
                            app: win.app
                            anchorId: "audio.output-device"
                            label: "Output device"
                            trailing: ThComboBox {
                                Layout.preferredWidth: 240
                                textRole: "label"
                                Accessible.name: "Output device"
                                model: [{ node: "", label: "System default" }].concat(win.app.mOutputs)
                                currentIndex: { for (var i = 0; i < model.length; i++) if (model[i].node === win.app.mOutputName) return i; return 0 }
                                onActivated: (i) => win.app.callctl.setOutput(model[i].node)
                            }
                        }
                        // the selected output's live meter (taps the sink's .monitor) — only while this pane shows
                        SettingsRow {
                            app: win.app
                            anchorId: "audio.output-level"
                            label: "Output level"
                            description: (!win.app.callctl.outputMonitoring && win.audioSettled)
                                         ? "Can't watch this output — it may be gone or exclusive to another app. Pick another one above."
                                         : "Moves while something plays."
                            MeterBox { level: win.app.mOutputLevel; live: win.app.callctl.outputMonitoring }
                        }
                    }
                    SettingsSection {
                        title: "Share audio"
                        SettingsRow {
                            app: win.app
                            anchorId: "audio.share-audio"
                            label: "Share audio source"
                            description: "Music or app sound for calls. Remembered." + (win.app.mInCall ? "" : " Sharing starts when you're in a call.")
                            trailing: ThComboBox {
                                Layout.preferredWidth: 240
                                textRole: "label"
                                Accessible.name: "Share audio source"
                                model: [{ node: "", label: "None" }].concat(win.app.mAudioSources)
                                currentIndex: { for (var i = 0; i < model.length; i++) if (model[i].node === win.app.mMusicSourceName) return i; return 0 }
                                // picking persists it; it starts only in a call (startMusicSource is a no-op otherwise)
                                onActivated: (i) => win.app.callctl.startMusicSource(model[i].node)
                            }
                        }
                    }
                    // 11.3 same room: link up automatically with remembered people + the list of them.
                    SettingsSection {
                        title: "Same room"
                        SettingsRow {
                            app: win.app
                            anchorId: "audio.same-room-auto"
                            label: "Link up automatically"
                            description: "When someone you've linked with before is in the call, link up again without asking. Linked people in one room send one voice and don't play each other through Tin Hat."
                            toggle: srAutoSw
                            trailing: ThSwitch {
                                id: srAutoSw
                                Accessible.name: "Link up automatically"
                                checked: win.app.callctl.sameRoomAuto
                                onToggled: { win.app.callctl.applySameRoomAuto(checked); checked = Qt.binding(() => win.app.callctl.sameRoomAuto) }
                            }
                        }
                        SettingsRow {
                            id: srPairsRow
                            app: win.app
                            anchorId: "audio.same-room-pairs"
                            label: "People you've linked with"
                            description: srPairsRow.pairs.length > 0 ? "Remove forgets them — you can link up again any time from their tile."
                                                                     : "Nobody yet. In a call, right-click someone sitting next to you → \"We're in the same room\"."
                            stacked: true
                            property var pairs: []
                            function reload() {
                                var out = []
                                try {
                                    var o = JSON.parse(win.app.callctl.sameRoomPairsJson())
                                    for (var k in o) out.push(k)
                                } catch (e) { out = [] }
                                out.sort()
                                pairs = out
                            }
                            Component.onCompleted: reload()
                            onVisibleChanged: if (visible) reload()
                            ColumnLayout {
                                Layout.fillWidth: true
                                spacing: Kirigami.Units.smallSpacing
                                visible: srPairsRow.pairs.length > 0
                                Repeater {
                                    model: srPairsRow.pairs
                                    delegate: RowLayout {
                                        required property string modelData
                                        Layout.fillWidth: true
                                        spacing: Kirigami.Units.largeSpacing
                                        Controls.Label {
                                            Layout.fillWidth: true
                                            elide: Text.ElideRight
                                            color: Th.settingsRowLabelFg
                                            text: win.app.localpart(modelData)
                                        }
                                        ThButton {
                                            app: win.app
                                            text: "Remove"
                                            onClicked: { win.app.callctl.sameRoomForget(modelData); srPairsRow.reload() }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // ================================ Calls & video ================================
                SettingsPage {
                    id: callsPage
                    anchors.fill: parent
                    visible: !win.searching && win.currentKey === "calls"
                    app: win.app
                    title: win.paneInfo.calls.t
                    description: win.paneInfo.calls.d

                    SettingsSection {
                        title: "Talking"
                        // 10.1 (TH-VIS-22 / TH-SET-04): push-to-talk is a call setting
                        SettingsRow {
                            app: win.app
                            anchorId: "calls.push-to-talk"
                            label: "Push to talk"
                            description: "Hold " + (win.app.actions && win.app.actions.pttKeys.length > 0 ? win.app.actions.pttKeys[0] : "Ctrl")
                                         + " to speak while Tin Hat is the focused window (not yet while you're tabbed into a game on this platform)."
                            toggle: pttSw
                            trailing: ThSwitch {
                                id: pttSw
                                Accessible.name: "Push to talk"
                                checked: win.app.callctl.pushToTalk
                                onToggled: win.app.callctl.applyPushToTalk(checked)
                            }
                        }
                        // 10.5: the "You're muted" reminder can also make a sound (off by default)
                        SettingsRow {
                            app: win.app
                            anchorId: "calls.muted-sound"
                            label: "Sound when I talk while muted"
                            description: "Tin Hat always shows “You're muted” when it hears you; this adds a sound for fullscreen games. Nothing leaves your computer while you're muted."
                            toggle: mutedSw
                            trailing: ThSwitch {
                                id: mutedSw
                                Accessible.name: "Play a sound when I talk while muted"
                                checked: win.app.settings.pref("snd.mutedtalk") === "1"
                                onToggled: win.app.settings.applyPref("snd.mutedtalk", checked ? "1" : "0")
                            }
                        }
                        // 11.0.2: the call bar's pin (AppCore.callBarPinned, device-local, default off)
                        SettingsRow {
                            app: win.app
                            anchorId: "calls.keep-buttons"
                            label: "Keep call buttons visible"
                            description: "Otherwise they hide a few seconds after you stop moving the mouse."
                            toggle: pinSw
                            trailing: ThSwitch {
                                id: pinSw
                                Accessible.name: "Keep call buttons visible"
                                checked: win.app.callBarPinned
                                onToggled: win.app.setCallBarPinned(checked)
                            }
                        }
                    }
                    // Pad (docs/PAD.md): this device's call may be driven from my other verified devices (default on)
                    SettingsSection {
                        title: "Pad"
                        SettingsRow {
                            app: win.app
                            anchorId: "calls.pad"   // the Settings search lands here (settings_index.json)
                            label: "Let my other verified devices control my calls here"
                            description: "While this device is in a call, your phone (or another device of yours) can play sounds, mute you and set people's volumes here. Only your own verified devices can; the call bar shows a chip while one does."
                            toggle: padSw
                            trailing: ThSwitch {
                                id: padSw
                                Accessible.name: "Let my other verified devices control my calls here"
                                checked: win.app.mPadOn
                                onToggled: win.app.padCtl.applyPadOn(checked)
                            }
                        }
                    }
                    SettingsSection {
                        title: "Camera"
                        SettingsRow {
                            app: win.app
                            anchorId: "calls.camera"
                            label: "Camera"
                            trailing: ThComboBox {
                                Layout.preferredWidth: 240
                                textRole: "label"
                                Accessible.name: "Camera"
                                model: [{ node: "", label: "Default (first camera)" }].concat(win.app.mCameras)
                                currentIndex: { for (var i = 0; i < model.length; i++) if (model[i].node === win.app.mCameraName) return i; return 0 }
                                onActivated: (i) => win.app.callctl.setCamera(model[i].node)
                            }
                        }
                        SettingsRow {
                            app: win.app
                            anchorId: "calls.camera-quality"
                            label: "Quality"
                            description: "1080p needs a camera that does more than 1080p."
                            trailing: ThComboBox {
                                Layout.preferredWidth: 240
                                textRole: "label"
                                Accessible.name: "Camera quality"
                                model: [
                                    { label: "720p · 30 fps (default)", w: 1280, h: 720,  fps: 30, kbps: 2500 },
                                    { label: "1080p · 30 fps",          w: 1920, h: 1080, fps: 30, kbps: 3000 }
                                ]
                                currentIndex: win.app._cqW() === 1920 ? 1 : 0
                                onActivated: (i) => win.app.callctl.applyCamQuality(model[i].w, model[i].h, model[i].fps, model[i].kbps)
                            }
                        }
                        SettingsRow {
                            app: win.app
                            anchorId: "calls.adaptive-layers"
                            label: "Adaptive layers"
                            description: "Sends 3 quality levels so each viewer's connection gets what fits (NVENC only)."
                            toggle: simSw
                            trailing: ThSwitch {
                                id: simSw
                                Accessible.name: "Adaptive layers"
                                checked: win.app._cqSim()
                                onToggled: win.app.callctl.applyCamSimulcast(checked)
                            }
                        }
                    }
                    SettingsSection {
                        title: "Screen share"
                        SettingsRow {
                            app: win.app
                            anchorId: "calls.share-resolution"
                            label: "Resolution"
                            trailing: ThComboBox {
                                Layout.preferredWidth: 240
                                textRole: "label"
                                Accessible.name: "Screen share resolution"
                                model: win.app.shareResList
                                currentIndex: { for (var i = 0; i < model.length; i++) if (model[i].w === win.app._sqW() && model[i].h === win.app._sqH()) return i; return 2 }
                                onActivated: (i) => win.app.callctl.applyShareQuality(model[i].w, model[i].h, win.app._sqFps(),
                                                                                      Math.round(model[i].base * win.app.shareBwList[win.app.shareCurTier()].f))
                            }
                        }
                        SettingsRow {
                            app: win.app
                            anchorId: "calls.share-fps"
                            label: "Frame rate"
                            trailing: ThComboBox {
                                Layout.preferredWidth: 240
                                readonly property var rates: [5, 15, 30, 60]
                                Accessible.name: "Frame rate"
                                model: ["5 fps", "15 fps", "30 fps", "60 fps"]
                                currentIndex: Math.max(0, rates.indexOf(win.app._sqFps()))
                                onActivated: (i) => win.app.callctl.applyShareQuality(win.app._sqW(), win.app._sqH(), rates[i], win.app._sqKbps())
                            }
                        }
                        SettingsRow {
                            app: win.app
                            anchorId: "calls.share-bandwidth"
                            label: "Bandwidth"
                            trailing: ThComboBox {
                                Layout.preferredWidth: 240
                                textRole: "label"
                                Accessible.name: "Bandwidth"
                                model: win.app.shareBwList
                                currentIndex: win.app.shareCurTier()
                                onActivated: (i) => win.app.callctl.applyShareQuality(win.app._sqW(), win.app._sqH(), win.app._sqFps(),
                                                                                      Math.round(win.app.shareCurRes().base * model[i].f))
                            }
                        }
                        SettingsRow {
                            app: win.app
                            anchorId: "calls.share-encoder"
                            label: "Encoder"
                            description: "GPU uses AV1 on RTX 40 and newer, else H.264. VP9 runs on the CPU and keeps text sharpest."
                            trailing: ThComboBox {
                                Layout.preferredWidth: 240
                                textRole: "label"
                                Accessible.name: "Encoder"
                                model: [
                                    { label: "GPU (best)", tag: "nvenc", code: 0 },
                                    { label: "VP9 (CPU)",  tag: "vp9",   code: 1 }
                                ]
                                currentIndex: win.app._sqEnc() === "vp9" ? 1 : 0
                                onActivated: (i) => win.app.callctl.applyShareEncoder(model[i].code)
                            }
                        }
                    }
                    SettingsSection {
                        title: "Incoming video"
                        SettingsRow {
                            app: win.app
                            anchorId: "calls.incoming-quality"
                            label: "Quality"
                            description: "Auto matches each tile's size on your screen."
                            trailing: ThComboBox {
                                Layout.preferredWidth: 240
                                textRole: "label"
                                Accessible.name: "Incoming video quality"
                                model: [
                                    { label: "Auto (recommended)", tag: "auto" },
                                    { label: "Always highest",     tag: "high" },
                                    { label: "Medium (360p)",      tag: "medium" },
                                    { label: "Data saver (180p)",  tag: "low" }
                                ]
                                currentIndex: { for (var i = 0; i < model.length; i++) if (model[i].tag === win.app.mRecvQuality) return i; return 0 }
                                onActivated: (i) => win.app.callctl.applyRecvQuality(model[i].tag)
                            }
                        }
                    }
                    SettingsSection {
                        title: "Connection"
                        SettingsRow {
                            app: win.app
                            anchorId: "calls.speed-test"
                            label: "Test connection speed"
                            description: win.app.callctl.connTest.length > 0 ? win.app.callctl.connTest
                                : "Checks your upload to the server, then what this machine can encode (about 25 s) — your share quality is capped by the lower one. Run it outside calls."
                            trailing: ThButton { app: win.app; text: "Run test"; enabled: !win.app.mInCall; onClicked: win.app.callctl.runConnectionTest() }
                        }
                    }
                }

                // ================================ Appearance ================================
                AppearancePane {
                    id: appearancePane
                    anchors.fill: parent
                    visible: !win.searching && win.currentKey === "appearance"
                    app: win.app
                    core: win.core
                    sheet: win
                    title: win.paneInfo.appearance.t
                    description: win.paneInfo.appearance.d
                }

                // ================================ Notifications ================================
                // 10.3: desktop toasts + every sound the app makes (they lived under Privacy)
                SettingsPage {
                    id: notifyPage
                    anchors.fill: parent
                    visible: !win.searching && win.currentKey === "notifications"
                    app: win.app
                    title: win.paneInfo.notifications.t
                    description: win.paneInfo.notifications.d

                    SettingsSection {
                        title: "Desktop notifications"
                        // 10.6 (REACH A5.11): what actually notifies
                        hint: "Change a room: right-click it (long-press on phones) → Notifications."
                        SettingsRow {
                            app: win.app
                            anchorId: "notifications.notify-mentions"
                            label: "Mentions and DMs"
                            description: "DMs, @mentions, replies to you, new invites and rooms set to All messages."
                            toggle: notifySw
                            trailing: ThSwitch {
                                id: notifySw
                                Accessible.name: "Desktop notifications for mentions and DMs"
                                checked: win.sNotify
                                onToggled: { win.app.settings.applyDesktopNotifications(checked); win.sNotify = checked }
                            }
                        }
                        SettingsRow {   // 10.1 (TH-NOTIF-08)
                            app: win.app
                            anchorId: "notifications.notify-calls"
                            label: "When someone starts a call"
                            description: "Calls ring in every room you haven't muted."
                            toggle: callAlertSw
                            trailing: ThSwitch {
                                id: callAlertSw
                                Accessible.name: "Desktop alert when someone starts a call"
                                checked: win.app.settings.pref("notify.calls") !== "0"
                                onToggled: win.app.settings.applyPref("notify.calls", checked ? "1" : "0")
                            }
                        }
                    }
                    SettingsSection {
                        title: "Sounds"
                        SettingsRow {
                            app: win.app
                            anchorId: "notifications.sound-open-room"
                            label: "Sound for messages in the open room"
                            toggle: sndRecvSw
                            trailing: ThSwitch {
                                id: sndRecvSw
                                Accessible.name: "Sound when a message arrives in the room you're looking at"
                                checked: win.app.settings.pref("snd.receive") !== "0"
                                onToggled: win.app.settings.applyPref("snd.receive", checked ? "1" : "0")
                            }
                        }
                        SettingsRow {
                            app: win.app
                            anchorId: "notifications.sound-send"
                            label: "Sound when you send"
                            toggle: sndSendSw
                            trailing: ThSwitch {
                                id: sndSendSw
                                Accessible.name: "Sound when you send a message"
                                checked: win.app.settings.pref("snd.send") !== "0"
                                onToggled: win.app.settings.applyPref("snd.send", checked ? "1" : "0")
                            }
                        }
                        SettingsRow {   // 10.1 (TH-NOTIF-02)
                            app: win.app
                            anchorId: "notifications.sound-join-leave"
                            label: "Join and leave chime"
                            description: "When someone joins or leaves your call. The chimes come from the sound pack in Appearance."
                            toggle: sndChimeSw
                            trailing: ThSwitch {
                                id: sndChimeSw
                                Accessible.name: "Chime when someone joins or leaves the call"
                                checked: win.app.settings.pref("snd.callchime") !== "0"
                                onToggled: win.app.settings.applyPref("snd.callchime", checked ? "1" : "0")
                            }
                        }
                        SettingsRow {
                            app: win.app
                            anchorId: "notifications.notification-sound"
                            label: "Notification sound"
                            toggle: sndNotifyOn
                            trailing: ThSwitch {
                                id: sndNotifyOn
                                Accessible.name: "Notification sound"
                                checked: win.app.settings.pref("snd.notify") !== "0"
                                onToggled: win.app.settings.applyPref("snd.notify", checked ? "1" : "0")
                            }
                            RowLayout {
                                spacing: 8
                                enabled: sndNotifyOn.checked
                                ThComboBox {
                                    id: sndNotifyPick
                                    Layout.preferredWidth: 240
                                    Accessible.name: "Which notification sound"
                                    model: ["Sound 1 (default, 1 s)", "Sound 2 (3 s)", "Sound 3 (1.5 s)", "Sound 4 (1 s)"]   // 10.1 (TH-SET-14)
                                    currentIndex: { var v = win.app.settings.pref("snd.notify.variant"); return v === "2" ? 1 : v === "3" ? 2 : v === "4" ? 3 : 0 }
                                    onActivated: function (i) {
                                        win.app.settings.applyPref("snd.notify.variant", i === 0 ? "" : String(i + 1))
                                        win.app.previewSound("notify" + (i === 0 ? "" : "-" + (i + 1)))
                                    }
                                }
                                ThButton {
                                    app: win.app
                                    flat: true
                                    icon.name: "media-playback-start-symbolic"
                                    text: "Preview"
                                    onClicked: win.app.previewSound("notify" + (sndNotifyPick.currentIndex === 0 ? "" : "-" + (sndNotifyPick.currentIndex + 1)))
                                }
                            }
                        }
                        SettingsRow {
                            app: win.app
                            anchorId: "notifications.ringtone"
                            label: "Ring for incoming calls"
                            toggle: sndRingOn
                            trailing: ThSwitch {
                                id: sndRingOn
                                Accessible.name: "Ring for incoming calls"
                                checked: win.app.settings.pref("snd.ring") !== "0"
                                onToggled: win.app.settings.applyPref("snd.ring", checked ? "1" : "0")
                            }
                            RowLayout {
                                spacing: 8
                                enabled: sndRingOn.checked
                                ThComboBox {
                                    id: sndRingPick
                                    Layout.preferredWidth: 240
                                    Accessible.name: "Which ring"
                                    model: ["Ring 1 (default, 8 s)", "Ring 2 (15 s)"]   // 10.1 (TH-SET-14)
                                    currentIndex: win.app.settings.pref("snd.ring.variant") === "2" ? 1 : 0
                                    onActivated: function (i) {
                                        win.app.settings.applyPref("snd.ring.variant", i === 0 ? "" : "2")
                                        win.app.previewSound("ring" + (i === 0 ? "" : "-2"))
                                    }
                                }
                                ThButton {
                                    app: win.app
                                    flat: true
                                    icon.name: "media-playback-start-symbolic"
                                    text: "Preview"
                                    onClicked: win.app.previewSound("ring" + (sndRingPick.currentIndex === 0 ? "" : "-2"))
                                }
                            }
                        }
                        SettingsRow {
                            app: win.app
                            anchorId: "notifications.sound-volume"
                            label: "Sound volume"
                            description: "Every sound above, and the join and leave chimes."
                            trailing: RowLayout {
                                spacing: 8
                                ThSlider {
                                    id: sndVolume
                                    Layout.preferredWidth: 200
                                    from: 0; to: 100; stepSize: 5
                                    Accessible.name: "Sound volume"
                                    value: { var v = parseInt(win.app.settings.pref("snd.volume")); return isNaN(v) ? 70 : v }
                                    onMoved: win.app.settings.applyPref("snd.volume", String(Math.round(value)))
                                }
                                Controls.Label {
                                    Layout.preferredWidth: 40
                                    text: Math.round(sndVolume.value) + "%"
                                    color: Th.settingsLabelFg
                                    font.pixelSize: Math.round(13 * Th.textScale)
                                    horizontalAlignment: Text.AlignRight
                                }
                            }
                        }
                    }
                }

                // ================================ Shortcuts ================================
                // 11.0 (ACTIONS.md §8.8; the web's ShortcutsPane.tsx): any action can get a key, clashes flagged
                SettingsPage {
                    id: shortcutsPage
                    anchors.fill: parent
                    visible: !win.searching && win.currentKey === "shortcuts"
                    app: win.app
                    title: win.paneInfo.shortcuts.t
                    description: win.paneInfo.shortcuts.d
                    ShortcutsPane { Layout.fillWidth: true; app: win.app }
                }

                // ================================ Overlay ================================
                // the in-game "who's talking" HUD (OverlayWindow.qml): a click-through compositor layer ABOVE fullscreen
                // windows — never injected into a game, never scans what's running
                SettingsPage {
                    id: overlayPage
                    anchors.fill: parent
                    visible: !win.searching && win.currentKey === "overlay"
                    app: win.app
                    title: win.paneInfo.overlay.t
                    description: win.paneInfo.overlay.d

                    SettingsSection {
                        title: "Voice overlay"
                        SettingsRow {   // 10.1 (TH-SET-20)
                            app: win.app
                            visible: win.app.mOverlayUnavailable
                            iconName: "dialog-warning"
                            iconColor: Th.settingsErrorFg
                            label: "Overlay unavailable"
                            labelColor: Th.settingsErrorFg
                            description: "The KDE layer-shell QML module didn't load on this system."
                        }
                        SettingsRow {
                            app: win.app
                            anchorId: "overlay.overlay"
                            label: "Voice overlay"
                            description: "Click-through pills in a corner showing who's talking — visible over fullscreen games. Nothing is injected or scanned (no anti-cheat risk), and it costs nothing while nobody talks."
                            toggle: overlaySw
                            trailing: ThSwitch {
                                id: overlaySw
                                Accessible.name: "Voice overlay (who's talking)"
                                enabled: !win.app.mOverlayUnavailable
                                checked: win.app.mOverlayEnabled
                                onToggled: { win.app.settings.applyOverlayEnabled(checked); if (checked) win.app.ensureOverlay(); checked = Qt.binding(() => win.app.mOverlayEnabled) }
                            }
                        }
                        SettingsRow {
                            app: win.app
                            anchorId: "overlay.overlay-show"
                            enabled: win.app.mOverlayEnabled && !win.app.mOverlayUnavailable
                            label: "Show"
                            trailing: ThComboBox {
                                Layout.preferredWidth: 240
                                Accessible.name: "Show the overlay"
                                model: ["Only while someone talks", "Always during a call"]
                                currentIndex: win.app.mOverlayMode === "always" ? 1 : 0
                                onActivated: (i) => win.app.settings.applyOverlayMode(i === 1 ? "always" : "speaking")
                            }
                        }
                        SettingsRow {
                            app: win.app
                            anchorId: "overlay.overlay-corner"
                            enabled: win.app.mOverlayEnabled && !win.app.mOverlayUnavailable
                            label: "Corner"
                            trailing: ThComboBox {
                                id: overlayCornerBox
                                Layout.preferredWidth: 240
                                readonly property var vals: ["tl", "tr", "bl", "br"]
                                Accessible.name: "Overlay corner"
                                model: ["Top left", "Top right", "Bottom left", "Bottom right"]
                                currentIndex: Math.max(0, vals.indexOf(win.app.mOverlayCorner))
                                onActivated: (i) => win.app.settings.applyOverlayCorner(vals[i])
                            }
                        }
                        SettingsRow {
                            app: win.app
                            anchorId: "overlay.overlay-hide-sharing"
                            enabled: win.app.mOverlayEnabled && !win.app.mOverlayUnavailable
                            label: "Hide while screen sharing"
                            description: "The overlay shows in your share when it's on the shared screen."
                            toggle: hideShareSw
                            trailing: ThSwitch {
                                id: hideShareSw
                                Accessible.name: "Hide while screen sharing"
                                checked: win.app.mOverlayHideShare
                                onToggled: win.app.settings.applyOverlayHideShare(checked)
                            }
                        }
                        // Preview works even while the overlay is off (demo pills), so the corner can be judged first
                        SettingsRow {
                            app: win.app
                            enabled: !win.app.mOverlayUnavailable
                            label: "Preview"
                            description: "Shows sample pills for 10 seconds."
                            trailing: ThButton { app: win.app; icon.name: "view-preview-symbolic"; text: "Preview (10 s)"
                                                   onClicked: { win.app.ensureOverlay(); if (win.app._overlayWin) win.app._overlayWin.startPreview() } }
                        }
                    }
                }

                // ================================ About & help ================================
                // (replaces the About sheet: the ☰ menu's "About Tin Hat" and app.about land here)
                SettingsPage {
                    id: aboutPage
                    anchors.fill: parent
                    visible: !win.searching && win.currentKey === "about"
                    app: win.app
                    title: win.paneInfo.about.t
                    description: win.paneInfo.about.d

                    SettingsSection {
                        SettingsRow {
                            id: tinhatRow
                            app: win.app
                            anchorIds: ["about.updates", "about.whats-new"]
                            readonly property bool selfUpdates: win.app.mCanSelfUpdate || win.app.updater.channel === "flatpak"
                            readonly property string channelText: win.app.updater.channel === "flatpak" ? "Flatpak"
                                : (win.app.updater.channel === "pacman-final" ? "pacman" : (Qt.platform.os === "windows" ? "Windows" : ""))
                            readonly property bool statusWorthShowing: win.app.mUpdateStatus.length > 0
                                && win.app.mUpdateStatus !== "You're on the latest version" && win.app.mUpdateStatus !== "Update available"
                                && win.app.mUpdateStatus !== "Checking for updates…"
                            // a failed check says so plainly; the details stay in a tooltip (what a bug report needs)
                            readonly property string statusText: win.app.mUpdateStatus.indexOf("Update check failed") === 0
                                ? "Couldn't reach the update server — try again later." : win.app.mUpdateStatus
                            HoverHandler { id: tinhatHover }
                            Controls.ToolTip.text: win.app.mUpdateStatus
                            Controls.ToolTip.visible: tinhatHover.hovered && tinhatRow.statusWorthShowing && tinhatRow.statusText !== win.app.mUpdateStatus
                            Controls.ToolTip.delay: 600
                            label: "Tin Hat"
                            labelPixelSize: 17
                            labelWeight: Font.DemiBold
                            // 10.1 (TH-DOC-12): the build version + channel is what a person (and a bug report) needs
                            description: "Version " + win.app.mAppVersion + " (" + win.app.mBuildHash + ")"
                                         + (tinhatRow.channelText.length > 0 ? " · " + tinhatRow.channelText : "")
                                         + (tinhatRow.statusWorthShowing ? "\n" + tinhatRow.statusText : "")
                            leading: TinHatLoader { Layout.preferredWidth: 56; Layout.preferredHeight: 56; running: false }
                            trailing: [
                                ThButton {
                                    app: win.app
                                    highlighted: win.app.mUpdateReady || win.app.mUpdateAvailable
                                    // the updater's states (10.1 TH-PKG-02: the same condition as the ☰ item and the banner)
                                    text: win.app.mUpdateReady ? "Restart to update"
                                        : win.app.mUpdateDownloading ? "Downloading…"
                                        : win.uChecking ? "Checking…"
                                        : win.app.mUpdateAvailable ? (tinhatRow.selfUpdates ? "Update to " + win.app.mLatestVersion : "How to update")
                                        : win.app.mUpdateStatus === "You're on the latest version" ? "You're up to date" : "Check for updates"
                                    icon.name: win.app.mUpdateReady ? "view-refresh-symbolic" : (win.app.mUpdateStatus === "You're on the latest version" && !win.app.mUpdateAvailable ? "checkmark-symbolic" : "")
                                    enabled: !win.app.mUpdateDownloading && !win.uChecking
                                    onClicked: {
                                        if (win.app.mUpdateReady) win.app.updater.restart()
                                        else if (win.app.mUpdateAvailable && tinhatRow.selfUpdates) win.app.updater.downloadAndApply()
                                        else if (win.app.mUpdateAvailable) win.toast("Update: " + win.app.manualUpdateHint, "long")
                                        else win.app.updater.checkForUpdates()
                                    }
                                },
                                ThButton { app: win.app; flat: true; text: "What's new"; onClicked: win.handOff("about", "whatsNew") }
                            ]
                        }
                        // 0.8.16 (Windows): where this copy lives + the one-click install
                        SettingsRow {
                            app: win.app
                            visible: Qt.platform.os === "windows"
                            label: win.app.mPortableRun ? "Running from a folder" : "Installed"
                            description: win.app.mPortableRun
                                ? (win.app.mInstalledVersion.length > 0 ? "Version " + win.app.mInstalledVersion + " is installed separately." : "Install it for a Start-menu shortcut and automatic updates.")
                                : (win.app.mCanSelfUpdate ? "In your apps — updates apply automatically." : "Development build.")
                            trailing: GlowButton {
                                app: win.app
                                visible: win.app.mPortableRun
                                readonly property bool haveNewer: win.app.mInstalledVersion.length > 0 && win.app.verNewerOrSame(win.app.mInstalledVersion, win.app.mAppVersion)
                                text: win.app.mInstalling ? "Installing…" : (haveNewer ? "Open installed" : "Install to your apps")
                                busy: win.app.mInstalling
                                enabled: !win.app.mInstalling
                                onClicked: { if (haveNewer) win.app.updater.openInstalled(); else win.app.updater.installToApps() }
                            }
                        }
                    }
                    SettingsSection {
                        title: "Help"
                        SettingsRow {
                            app: win.app
                            anchorId: "about.tour"
                            clickable: true
                            iconName: "help-hint-symbolic"
                            label: "Take the tour"
                            description: "A quick walk around Tin Hat."
                            onClicked: win.handOff("", "tour")
                        }
                        SettingsRow {
                            app: win.app
                            anchorId: "about.shortcut-list"
                            clickable: true
                            iconName: "input-keyboard-symbolic"
                            label: "Keyboard shortcuts"
                            description: "Every key at a glance. Change them in Shortcuts."
                            onClicked: win.handOff("about", "shortcuts")
                        }
                        SettingsRow {
                            visible: win.app.serverOn("reports")   // selfhost: only where the server's owner takes reports
                            app: win.app
                            anchorId: "about.report"
                            clickable: true
                            iconName: "tools-report-bug-symbolic"
                            label: "Report a problem"
                            description: "Sends a note and this session's log to your server's owner."
                            onClicked: win.handOff("about", "bugReport")
                        }
                    }
                    SettingsSection {
                        title: "Account"
                        SettingsRow {
                            app: win.app
                            anchorId: "about.sign-out"
                            label: "Signed in as"
                            description: win.app.mUserId
                            trailing: DangerButton { app: win.app; text: "Sign out"; onClicked: win.handOff("about", "signOut") }
                        }
                    }
                    Controls.Label {
                        Layout.fillWidth: true
                        text: "A ground-up native client for your Tin Hat server — no Electron, no webview (Tin Hat Web is the same app in a browser). "
                            + "First-class Plasma/Wayland feel, with hardware (NVENC) screen sharing in calls."
                        color: Th.settingsHintFg
                        font.pixelSize: Math.round(12 * Th.textScale)
                        wrapMode: Text.Wrap
                    }
                }

                // ================================ the save bar (Profile, §5) ================================
                Rectangle {
                    id: saveBar
                    visible: win.saveBarShown
                    x: 28
                    width: win.innerWidth
                    height: 48
                    y: contentArea.height - 16 - height
                    radius: 10
                    color: Th.settingsSaveBarBg
                    border.width: 1
                    border.color: Th.settingsSaveBarBorder
                    RowLayout {
                        anchors.fill: parent
                        anchors.leftMargin: 16
                        anchors.rightMargin: 8
                        spacing: 8
                        Controls.Label {
                            Layout.fillWidth: true
                            text: win.nameEmpty ? "Your display name can't be empty." : "You have unsaved changes."
                            color: win.nameEmpty ? Th.settingsErrorFg : Th.settingsSaveBarFg
                            font.pixelSize: Math.round(13 * Th.textScale)
                            elide: Text.ElideRight
                        }
                        ThButton { app: win.app; flat: true; text: "Reset"; enabled: !win.sBusy; onClicked: win.resetProfile() }
                        GlowButton { app: win.app; text: "Save changes"; busy: win.sBusy; enabled: !win.sBusy && !win.nameEmpty; onClicked: win.saveProfile() }
                    }
                }

                // ================================ the status strip ================================
                Rectangle {
                    id: strip
                    visible: win.stripText.length > 0
                    readonly property real maxW: Math.min(win.innerWidth, 560)
                    width: Math.min(stripRow.implicitWidth + 32, strip.maxW)
                    height: Math.max(36, stripRow.implicitHeight + 12)
                    x: 28 + (win.innerWidth - width) / 2
                    y: contentArea.height - 16 - height - (win.saveBarShown ? 48 + 10 : 0)
                    radius: height / 2
                    color: Th.settingsCardBg
                    border.width: 1
                    border.color: win.stripError ? Th.settingsDangerBorder : Th.settingsCardBorder
                    RowLayout {
                        id: stripRow
                        anchors.centerIn: parent
                        width: Math.min(implicitWidth, strip.maxW - 32)
                        spacing: 10
                        Controls.Label {
                            Layout.fillWidth: true
                            Layout.maximumWidth: strip.maxW - 32 - (stripBtn.visible ? stripBtn.implicitWidth + 10 : 0)
                            text: win.stripText
                            // outcome-coloured: errors must not celebrate in accent
                            color: win.stripError ? Th.settingsStatusFgError : Th.settingsStatusFg
                            font.pixelSize: Math.round(13 * Th.textScale)
                            elide: Text.ElideRight
                        }
                        ThButton {
                            id: stripBtn
                            app: win.app
                            flat: true
                            implicitHeight: 26
                            visible: win.stripAction.length > 0
                            text: win.stripAction
                            onClicked: { var cb = win._stripCb; win.stripText = ""; win.stripAction = ""; win._stripCb = null; if (cb) cb() }
                        }
                    }
                }
            }
        }
    }
}
