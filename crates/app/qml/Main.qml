// Tin Hat — native Matrix client (Kirigami). Talks to the Rust controllers
// over CXX-Qt: LoginController (auth + room list), RoomController (timeline + send),
// StreamController (NVENC screen-share publish into a room's Element Call).
//
// NOTE: cxx-qt 0.8 property NOTIFY signals do not reliably drive QML bindings in
// this setup (the UI got stuck on the login screen even though login + room load
// succeeded on the Rust side). So instead of binding the UI directly to
// `login.loggedIn` etc., a short Timer mirrors the controller getters into plain
// QML properties (`m*`), and the UI binds to those — QML's own property system
// propagates those reliably. Method calls (login(), open(), startShare()) read
// getters at call time, so they work regardless.
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import QtQuick.Dialogs
import QtQuick.Window
import QtMultimedia
import org.kde.kirigami as Kirigami
import dev.tinhat.app
// NOTE: the GstGLQt6VideoItem type is registered PROGRAMMATICALLY at runtime by the
// gst qml6 plugin (tinhat_video_preinit), with no QML module on disk — so a STATIC
// import + `GstGLQt6VideoItem {}` is invisible to qmlcachegen at build time and never
// instantiates. We create the item at runtime via Qt.createQmlObject instead.

Kirigami.ApplicationWindow {
    id: root
    // ===================== AppCore: controllers + glue (9.0 hoist) =====================
    // Everything that is NOT this window's look lives in AppCore.qml so a second shell could
    // share it. Every moved property/function comes back here as an alias/forwarder so the
    // Legacy shell below stays exactly as it was. See docs/SHELLS.md.
    AppCore {
        id: core
        shell: root
        navOpened: cornerNav.opened
        memoriesWanted: memoriesWindow.visible
        onNotify: (m, t) => { if (t === undefined || t === null) root.showPassiveNotification(LinkGate.plain(m)); else root.showPassiveNotification(LinkGate.plain(m), t) }
        onNotifyAction: (m, t, a, cb) => root.showPassiveNotification(LinkGate.plain(m), t, a, cb)   // themes 1b: a toast with an action
        onOpenIconGallery: iconGallery.openOrRaise()
        onFocusCalendarEvent: (id) => calendarWindow.focusEvent(id)
        onOpenCalendarWindow: calendarWindow.openOrRaise()
        onMirrored: root._maybeAutoShowTour()
        onBootReveal: (why) => { if (mainMemo.wantMaximized) root.showMaximized(); else root.visible = true; root.raise(); root.requestActivate(); bootctl.finish() }
        onThemeEditorOpened: settingsWindow.close()   // THEMES-UI §13.1: the editor works on the live app
    }
    // 10.1 (TH-SHELL-08 / TH-CONV-11 / TH-VIS-01): the verify / offline / update banners are ONE
    // window-wide strip above everything, not InlineMessages wedged into the sidebar column
    // (which sliced the Spaces rail + room list and were invisible in corner mode).
    header: ColumnLayout {
        id: bannerStrip
        // 11.0.1 (the Windows idle-CPU loop): the strip's width is the window's, SET here. ApplicationWindow sizes its
        // header with an implicit width (it sets the width, then clears the item's "width set" flag), so each time this
        // layout's size hints changed, setImplicitSize() put the width back to the implicit one — the banners' side
        // margins, 12 px (an InlineMessage has no implicit width). Qt 6.8 (the Windows builds) then re-arranged the strip
        // at that stale width after the window had already restored it (QQuickLayout::geometryChange; fixed in Qt 6.10,
        // 9352ef3c / QTBUG-87708): the banner laid out 0 px wide, its text wrapped a word per line (330 px tall), the strip
        // grew, the next pass laid it out full width again, and so on — 10,000 polish passes a frame (~0.8 core while the
        // unverified banner shows), InlineMessage.qml "Binding loop … implicitHeight" by the ten thousand, and the empty
        // band under the banner. A width that is set is never reset, so every pass settles at once. Linux (Qt 6.11)
        // already settled: the same width, the same pixels.
        width: root.width
        spacing: 0
        visible: (bootctl.safeMode && !safeModeBanner.dismissed)
                 || (root.mLoggedIn && ((!root.mVerified && !login.firstDevicePending && !root.mGuestAccount) || login.syncDown || root.mUpdateAvailable || root.mUpdateDownloading || root.mUpdateReady))
        height: visible ? implicitHeight : 0
        // Safe mode (0.8.25, boot.rs): this start runs with software rendering because the
        // previous one never reached the main window. Say so; OK = normal mode next start.
        // 10.1 (TH-SHELL-09): part of the strip (pushes the UI down) instead of a z:9998 overlay.
        Rectangle {
            id: safeModeBanner
            property bool dismissed: false
            Layout.fillWidth: true
            implicitHeight: 34
            visible: bootctl.safeMode && !dismissed
            color: Th.bannerSafeModeBg
            RowLayout {
                anchors { fill: parent; leftMargin: 12; rightMargin: 8 }
                spacing: 10
                Kirigami.Icon {
                    source: "dialog-warning"
                    Layout.preferredWidth: 16; Layout.preferredHeight: 16
                    color: Th.bannerSafeModeIcon
                }
                Controls.Label {
                    textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                    Layout.fillWidth: true
                    elide: Text.ElideRight
                    color: Th.bannerSafeModeFg
                    font.bold: true
                    text: "Safe mode — " + bootctl.safeReason + ". Running with software rendering; "
                        + "the failed start's log has been sent. The next start returns to normal."
                }
                ThButton {
                    app: root
                    text: "OK"
                    onClicked: { bootctl.clearSafeMode(); safeModeBanner.dismissed = true }
                }
            }
        }
        // Device-verification prompt (E2EE rooms can't decrypt until verified).
        Kirigami.InlineMessage {
            Layout.fillWidth: true
            // 10.8: in the islands look the strip lines up with the islands (their margin; the host's own margin is the
            // gap below it); Flat keeps the 4 px all round it always had
            Layout.leftMargin: root.bannerMargin
            Layout.rightMargin: root.bannerMargin
            Layout.topMargin: root.bannerMargin
            Layout.bottomMargin: root.flatLayout ? Kirigami.Units.smallSpacing : 0
            // F1: not while a brand-new account's first device is still setting its encryption up (it ends verified);
            // never for a temporary guest account (docs/GUESTS.md: no encryption set-up, no verify prompt)
            visible: root.mLoggedIn && !root.mVerified && !login.firstDevicePending && !root.mGuestAccount
            type: Kirigami.MessageType.Warning
            // keyFirst (moved.rs): no identity to verify against (a moved server): the Security Key is the way in
            text: login.keyFirst
                  ? "Encrypted messages can't be read on this device yet. Unlock them with your Security Key."
                  : "This device isn't verified, so encrypted messages can't be read. Verify it from a device you're already signed in to."
            actions: [
                Kirigami.Action {
                    text: login.keyFirst ? "Unlock…" : "Verify…"
                    onTriggered: verifySheet.open()
                }
            ]
        }

        // 10.1 (TH-FIRST-03 / TH-STATE-01): the app's only "are we online?" signal outside a
        // call. Three sync failures in a row; clears itself on the first success.
        Kirigami.InlineMessage {
            Layout.fillWidth: true
            // 10.8: in the islands look the strip lines up with the islands (their margin; the host's own margin is the
            // gap below it); Flat keeps the 4 px all round it always had
            Layout.leftMargin: root.bannerMargin
            Layout.rightMargin: root.bannerMargin
            Layout.topMargin: root.bannerMargin
            Layout.bottomMargin: root.flatLayout ? Kirigami.Units.smallSpacing : 0
            visible: root.mLoggedIn && login.syncDown
            type: Kirigami.MessageType.Warning
            text: "Can't reach the server — reconnecting… New messages will catch up when it's back."
        }

        // 11.0 edit mode (EDITMODE-UI §9): the device-local reset — Keep for all devices / Restore
        Kirigami.InlineMessage {
            objectName: "resetBanner"
            Layout.fillWidth: true
            Layout.leftMargin: root.bannerMargin
            Layout.rightMargin: root.bannerMargin
            Layout.topMargin: root.bannerMargin
            Layout.bottomMargin: root.flatLayout ? Kirigami.Units.smallSpacing : 0
            visible: root.mLoggedIn && layoutStoreObj.overrideOn && !editModeObj.open
            type: Kirigami.MessageType.Information
            text: "Layout reset on this device."
            actions: [
                Kirigami.Action {
                    text: "Keep for all devices"
                    onTriggered: {
                        var r = layoutStoreObj.keepOverride()
                        if (r !== "saved" && r !== "noop") root.showPassiveNotification(LinkGate.plain("That didn't work (" + r + ")."), 5000)
                    }
                },
                Kirigami.Action { text: "Restore"; onTriggered: layoutStoreObj.clearOverride() }
            ]
        }
        // 11.0 (EDITMODE-UI §13): this device's layout save was replaced by a newer one from another device
        Kirigami.InlineMessage {
            objectName: "conflictBanner"
            Layout.fillWidth: true
            Layout.leftMargin: root.bannerMargin
            Layout.rightMargin: root.bannerMargin
            Layout.topMargin: root.bannerMargin
            Layout.bottomMargin: root.flatLayout ? Kirigami.Units.smallSpacing : 0
            visible: root.mLoggedIn && layoutStoreObj.view.conflict === true && !layoutStoreObj.overrideOn && !editModeObj.open
            type: Kirigami.MessageType.Warning
            text: "Your layout change was replaced by a newer one from another device."
            actions: [
                Kirigami.Action { text: "Restore mine"; onTriggered: layoutStoreObj.resolveConflict(true) },
                Kirigami.Action { text: "Dismiss"; onTriggered: layoutStoreObj.resolveConflict(false) }
            ]
        }

        // Auto-update banner: surfaces a newer build + one-click pull/restart.
        Kirigami.InlineMessage {
            Layout.fillWidth: true
            // 10.8: in the islands look the strip lines up with the islands (their margin; the host's own margin is the
            // gap below it); Flat keeps the 4 px all round it always had
            Layout.leftMargin: root.bannerMargin
            Layout.rightMargin: root.bannerMargin
            Layout.topMargin: root.bannerMargin
            Layout.bottomMargin: root.flatLayout ? Kirigami.Units.smallSpacing : 0
            visible: root.mUpdateAvailable || root.mUpdateDownloading || root.mUpdateReady
            type: root.mUpdateReady ? Kirigami.MessageType.Positive : Kirigami.MessageType.Information
            text: LinkGate.plain(root.mUpdateReady
                ? ("Update installed (v" + root.mLatestVersion + ") — restart to apply.")
                : (root.mUpdateDownloading
                   ? (updater.channel === "flatpak" ? root.mUpdateStatus : "Downloading update…")   // 10.0.2: the portal path narrates itself
                   : ("Update available: v" + root.mAppVersion + " → v" + root.mLatestVersion
                      + (root.mCanSelfUpdate ? "" : " — " + root.manualUpdateHint))))
            actions: [
                Kirigami.Action {
                    text: "What's new"
                    visible: root.mUpdateAvailable && root.mReleaseNotes.length > 0
                    onTriggered: updateNotesSheet.open()
                },
                // Self-update path (AppImage / user-writable install).
                Kirigami.Action {
                    text: "Update now"
                    visible: root.mUpdateAvailable && !root.mUpdateDownloading && (root.mCanSelfUpdate || updater.channel === "flatpak")
                    onTriggered: updater.downloadAndApply()
                },
                // A zip run can't swap itself — install once, then it can.
                Kirigami.Action {
                    text: "Install to your apps…"
                    visible: root.mUpdateAvailable && !root.mCanSelfUpdate && root.mPortableRun && !root.mInstalling
                    onTriggered: installDialog.open()
                },
                Kirigami.Action {
                    text: "Restart now"
                    visible: root.mUpdateReady
                    onTriggered: updater.restart()
                }
            ]
        }
    }
    readonly property var login: core.c_login
    readonly property var bootctl: core.c_bootctl
    readonly property var room: core.c_room
    readonly property var stream: core.c_stream
    readonly property var callctl: core.c_callctl
    readonly property var settings: core.c_settings
    readonly property var diagnostics: core.c_diagnostics
    readonly property var updater: core.c_updater
    readonly property var activityCtl: core.c_activityCtl
    readonly property var statusCtl: core.c_statusCtl
    readonly property var gifCtl: core.c_gifCtl
    readonly property var audioPrefsCtl: core.c_audioPrefsCtl
    readonly property var soundboardCtl: core.c_soundboardCtl
    readonly property var joinSoundCtl: core.c_joinSoundCtl
    readonly property var watchCtl: core.c_watchCtl
    readonly property var nav: core.c_nav
    readonly property var afkCtl: core.c_afkCtl
    readonly property var floatingCtl: core.c_floatingCtl
    readonly property var calCtl: core.c_calCtl
    readonly property var iconPack: core.c_iconPack
    readonly property var gamesCtl: core.c_gamesCtl
    readonly property var spaceCtl: core.c_spaceCtl   // RBAC (docs/RBAC.md §5.3)
    readonly property var invitesCtl: core.c_invitesCtl   // 10.4 invite codes (docs/INVITES.md)
    readonly property var guestsCtl: core.c_guestsCtl   // selfhost: the guest room (docs/GUESTS.md)
    readonly property var gameServersCtl: core.c_gameServers   // 11.1 the Game servers widget (docs/GAMESERVERS.md)
    readonly property var padCtl: core.c_padCtl   // Pad: the phone as a remote for my call (docs/PAD.md)
    property alias mPadOn: core.mPadOn
    property alias mPadChips: core.mPadChips
    property alias mPadTargets: core.mPadTargets
    property alias mPadRemote: core.mPadRemote
    property alias mPadSounds: core.mPadSounds
    // My other devices in `roomId`'s call, ready ones first ([] = none): {device, name, ready, why, createdTs}.
    function padTargetsIn(roomId) {
        var rs = (root.mPadTargets && root.mPadTargets.rooms) ? root.mPadTargets.rooms : ({})
        var l = (roomId && rs[roomId]) ? rs[roomId].slice() : []
        l.sort(function (a, b) { return (a.ready === b.ready) ? 0 : (a.ready ? -1 : 1) })   // stable: newest first within each
        return l
    }
    function padTargetIn(roomId) { var l = root.padTargetsIn(roomId); return l.length > 0 ? l[0] : null }
    // The one line under a greyed Pad (DESIGN §1): Verify this device / Verify {device} / Update Tin Hat on {device}.
    function padWhyLine(t) { return t && !t.ready ? String(padCtl.whyText(t.why, t.name, "")) : "" }
    // Pad → the Pad window for that device's call (a greyed one explains in a toast; "Verify this device…" offers
    // Devices & security).
    function openPad(roomId, t) {
        if (!t) return
        if (!t.ready) {
            if (t.why === "unverified") root.showPassiveNotification(LinkGate.plain(root.padWhyLine(t)), 4000, "Devices & security", function () { settingsWindow.openAt("security") })
            else root.showPassiveNotification(LinkGate.plain(root.padWhyLine(t)), 4000)
            return
        }
        padWindow.openFor(String(roomId), String(t.device))
    }
    readonly property var presenceCtl: core.c_presenceCtl   // 10.6 opt-in presence (docs/REACH.md §2)
    // 11.0 step 3 (buttons, ACTIONS.md): the actions engine, its table, and the runtime (the live snapshot, the one run
    // path, the id → command map — ActionsHost.qml; its `ui` below names the surfaces it opens)
    readonly property var actionsCtl: core.c_actionsCtl
    readonly property var actionsTable: JSON.parse(core.c_actionsCtl.tableJson())
    readonly property alias actions: actionsHost
    // 11.0 step 5 (THEMES-UI.md §13–§15): the theme editor, the share cards' Try / Keep, Share to a room
    readonly property var themeCtl: core.c_themeCtl
    readonly property var editorCtl: core.c_editorCtl
    property alias themeTrial: core.themeTrial
    // Media Party 11.4: a chat card's "Add to Media Party" / "Play now" — join `roomId`'s party first when not in one
    // (joinParty is destructive — never re-join while watching), then the link pipeline (watchCtl.addLink → linkJson,
    // the window's onLinkInfoChanged shows the toast).
    function mediaPartyAdd(roomId, url, playNow) {
        if (!watchCtl.inParty) watchCtl.joinParty(roomId)
        else if (String(watchCtl.roomId) !== String(roomId)) { root.showPassiveNotification("You're in another room's Media Party — leave it first.", 3000); return }
        watchCtl.addLink(url, playNow)
    }
    function tryTheme(key, docJson, name) { return core.tryTheme(key, docJson, name) }
    function endThemeTrial(back) { core.endTrial(back) }
    function keepTheme(themeJson) { core.keepTheme(themeJson) }
    function openThemeEditor(docJson, mineId, baseName) { return core.openThemeEditor(docJson, mineId, baseName) }
    /// Share to a room… (§14.5): the quick switcher's pick mode, then the card as an m.room.message
    function shareThemeToRoom(docJson, name) {
        // from Settings → Appearance: the room picker is this window's — Settings steps aside and comes back after
        if (settingsWindow.visible) root.settingsHandOff("appearance", "roomPicker", "")
        quickSwitcher.pickRoom("Share " + name + " to…", "Search rooms", function (roomId) {
            var err = core.c_editorCtl.shareToRoom(roomId, docJson)
            if (err.length > 0) root.showPassiveNotification(LinkGate.plain(err), "long")
        })
    }
    readonly property string mIconStyle: core.mIconStyle   // themes 1b: "duotone" | "mono" (ThIcon's accent tone)
    readonly property var tileModel: core.c_tileModel
    property alias themeName: core.themeName
    property alias soundsDir: core.soundsDir
    property alias fakeCall: core.fakeCall
    property alias callShown: core.callShown
    property alias fakeTiles: core.fakeTiles
    property alias callTiles: core.callTiles
    property alias zoomProbe: core.zoomProbe
    property alias tileSyncProbe: core.tileSyncProbe
    property alias noVerifyPrompt: core.noVerifyPrompt
    property alias _tileCreated: core._tileCreated
    property alias _tileDestroyed: core._tileDestroyed
    property alias mHiddenTiles: core.mHiddenTiles
    property alias hiddenKeys: core.hiddenKeys
    property alias tileViews: core.tileViews
    property alias selfViewMode: core.selfViewMode
    property alias selfPip: core.selfPip
    property alias selfPipEffective: core.selfPipEffective   // 10.1 (TH-CALL-24)
    property alias recOnlyHiddenIds: core.recOnlyHiddenIds
    property alias visibleShareIds: core.visibleShareIds
    property alias watchedShareIds: core.watchedShareIds
    property alias liveCallParts: core.liveCallParts
    property alias recActive: core.recActive
    property alias hiddenTileCount: core.hiddenTileCount
    property alias navMode: core.navMode
    property alias navReordering: core.navReordering
    property alias _pips: core._pips
    property alias _overlayWin: core._overlayWin
    property alias mOverlayUnavailable: core.mOverlayUnavailable
    property alias _chimeArmAt: core._chimeArmAt
    property alias mResolvedMedia: core.mResolvedMedia
    property alias mLoggedIn: core.mLoggedIn
    // 10.1: the pref store handed to component files (they must not reach for `settings` themselves).
    readonly property var prefStore: settings
    // 10.1: the mounted room-page ConversationView (null until a room page exists) — the
    // window-level file drop stages into it.
    property var conversationView: null
    property alias mVerified: core.mVerified
    property alias mBusy: core.mBusy
    property alias mStatus: core.mStatus
    property alias mResetStage: core.mResetStage
    property alias mResetStatus: core.mResetStatus
    property alias mResetMode: core.mResetMode
    property alias mResetNote: core.mResetNote
    property alias mRegisterStage: core.mRegisterStage     // 10.4
    property alias mRegisterStatus: core.mRegisterStatus
    property alias mUserId: core.mUserId
    property alias mRooms: core.mRooms
    // 9.3: per-room draft / scroll memory for ConversationView (lives here so it survives
    // the view being recreated).
    property alias chatMemory: core.chatMemory
    property alias soundboardPanel: soundboardPanel
    property alias _roomsRaw: core._roomsRaw
    property alias mRoomName: core.mRoomName
    property alias mTyping: core.mTyping
    property alias _typingRaw: core._typingRaw
    property alias mLoadingOlder: core.mLoadingOlder
    property alias mHasMore: core.mHasMore
    property alias mRoomBusy: core.mRoomBusy
    property alias mCallActive: core.mCallActive
    property alias mRoomStatus: core.mRoomStatus
    property alias mSharing: core.mSharing
    property alias mPreparing: core.mPreparing
    property alias mStreamStatus: core.mStreamStatus
    property alias mInCall: core.mInCall
    property alias shareResList: core.shareResList
    property alias shareBwList: core.shareBwList
    property alias mConnecting: core.mConnecting
    property alias mCallSharing: core.mCallSharing
    property alias mMicMuted: core.mMicMuted
    property alias mCallStatus: core.mCallStatus
    property alias mOutputs: core.mOutputs
    property alias _outputsRaw: core._outputsRaw
    property alias mOutputName: core.mOutputName
    property alias mInputs: core.mInputs
    property alias _inputsRaw: core._inputsRaw
    property alias mInputName: core.mInputName
    property alias mCameraOn: core.mCameraOn
    property alias mCameras: core.mCameras
    property alias _camerasRaw: core._camerasRaw
    property alias mCameraName: core.mCameraName
    property alias mShareQuality: core.mShareQuality
    property alias mCamQuality: core.mCamQuality
    property alias mRecvQuality: core.mRecvQuality
    property alias mCallUnfocusedSaver: core.mCallUnfocusedSaver   // 10.8.1: the unfocused master switch (AppearancePane)
    property alias mMusicPlaying: core.mMusicPlaying
    property alias mNcAvailable: core.mNcAvailable
    property alias mEcDegraded: core.mEcDegraded
    property alias mEcNote: core.mEcNote
    property alias mEcOutputs: core.mEcOutputs
    property alias _ecOutputsRaw: core._ecOutputsRaw
    property alias mMicDenoise: core.mMicDenoise
    property alias mMicAec: core.mMicAec
    property alias mMicAgc: core.mMicAgc
    property alias mMicNsMode: core.mMicNsMode
    property alias mAecEngine: core.mAecEngine
    property alias mMicSilent: core.mMicSilent
    property alias mOutputMissing: core.mOutputMissing
    property alias mLogship: core.mLogship
    property alias mOverlayEnabled: core.mOverlayEnabled
    property alias mOverlayMode: core.mOverlayMode
    property alias mOverlayCorner: core.mOverlayCorner
    property alias mOverlayHideShare: core.mOverlayHideShare
    property alias mOverlayDemo: core.mOverlayDemo
    property alias mReconnecting: core.mReconnecting
    property alias mCallFailed: core.mCallFailed
    property alias mCallFailReason: core.mCallFailReason
    property alias mAudioSources: core.mAudioSources
    property alias _audioSourcesRaw: core._audioSourcesRaw
    property alias mMusicSourceName: core.mMusicSourceName
    property alias mAudioApps: core.mAudioApps
    property alias _audioAppsRaw: core._audioAppsRaw
    property alias mShareRecOnly: core.mShareRecOnly
    property alias mRecOnlyFlags: core.mRecOnlyFlags
    property alias _recOnlyRaw: core._recOnlyRaw
    property alias mGameChat: core.mGameChat
    property alias mGameChatPeers: core.mGameChatPeers
    property alias _gameChatRaw: core._gameChatRaw
    property alias mCorner: core.mCorner           // 9.4 corners
    property alias _cornerRaw: core._cornerRaw
    property alias mAside: core.mAside
    property alias mCallRoomId: core.mCallRoomId
    property alias _navPartAt: core._navPartAt
    property alias listenTiles: core.listenTiles
    property alias _pipsRev: core._pipsRev
    property alias mLocalAfk: core.mLocalAfk
    property alias _afkApplied: core._afkApplied
    property alias _afkMutedMic: core._afkMutedMic
    property alias mAwayUsers: core.mAwayUsers
    property alias _awayRaw: core._awayRaw
    property alias _afkPollAt: core._afkPollAt
    property alias mVideoTiles: core.mVideoTiles
    property alias _videoTilesRaw: core._videoTilesRaw
    property alias mAudioLevels: core.mAudioLevels
    property alias _audioLevelsRaw: core._audioLevelsRaw
    property alias mSpeakers: core.mSpeakers
    // 10.5 "can they hear me?" (AppCore: mHear + helpers)
    property alias mHear: core.mHear
    property alias mSameRoom: core.mSameRoom   // 11.3 same room
    property alias mMutedTalkBubble: core.mMutedTalkBubble
    function hearPeer(identity) { return core.hearPeer(identity) }
    function hearName(identity) { return core.hearName(identity) }
    // 11.3 same room helpers (AppCore) — CallIsland / ParticipantVolumeMenu reach them through the Main root
    function sameRoomSuggest(hear, sr) { return core.sameRoomSuggest(hear, sr) }
    function sameRoomGroupOf(identity, sr) { return core.sameRoomGroupOf(identity, sr) }
    function sameRoomTip(isSelf, identity, sr) { return core.sameRoomTip(isSelf, identity, sr) }
    function hearNames(ids) { return core.hearNames(ids) }
    function hearReason(st, aboutMe) { return core.hearReason(st, aboutMe) }
    function hearProblem(st) { return core.hearProblem(st) }
    function heardSummary() { return core.heardSummary() }
    // The echo pill after a fix: "Checking the echo…" until it clears (then "Echo gone ✓") or 90 s pass.
    property bool _echoChecking: false
    Timer { id: echoCheckTimer; interval: 90000; onTriggered: root._echoChecking = false }
    property alias _speakersRaw: core._speakersRaw
    property alias mPublishStats: core.mPublishStats
    property alias _publishStatsRaw: core._publishStatsRaw
    property alias mMicLevel: core.mMicLevel
    property alias mMicLevelPre: core.mMicLevelPre
    property alias mOutputLevel: core.mOutputLevel
    property alias mDeafened: core.mDeafened
    property alias mHangingUp: core.mHangingUp
    property alias callMaximized: core.callMaximized
    // 9.3.1: the call/chat seam.
    property alias callPaneFrac: core.callPaneFrac
    property alias callCollapsed: core.callCollapsed
    function setCallPaneFrac(f) { core.setCallPaneFrac(f) }
    function setCallCollapsed(on) { core.setCallCollapsed(on) }
    property alias callBarPinned: core.callBarPinned   // 11.0.2: "Keep call buttons visible" (AppCore)
    function setCallBarPinned(on) { core.setCallBarPinned(on) }
    property alias callLayout: core.callLayout
    property alias pinnedTile: core.pinnedTile
    property alias callSideFrac: core.callSideFrac
    property alias callAnimate: core.callAnimate
    property alias mMembers: core.mMembers
    property alias _membersRaw: core._membersRaw
    property alias mRoomSummary: core.mRoomSummary
    property alias _roomSummaryRaw: core._roomSummaryRaw
    property alias mMyPower: core.mMyPower
    property alias mCanChangeName: core.mCanChangeName
    property alias mCanChangeTopic: core.mCanChangeTopic
    property alias mCanInvite: core.mCanInvite
    property alias mCanKick: core.mCanKick
    property alias mCanBan: core.mCanBan
    property alias mCanChangePower: core.mCanChangePower
    property alias mModError: core.mModError
    property alias profileMxid: core.profileMxid
    property alias mAppVersion: core.mAppVersion
    property alias mUniversalVersion: core.mUniversalVersion
    property alias mBuildHash: core.mBuildHash
    property alias mLatestVersion: core.mLatestVersion
    property alias mUpdateAvailable: core.mUpdateAvailable
    property alias mUpdateDownloading: core.mUpdateDownloading
    property alias mUpdateReady: core.mUpdateReady
    property alias mCanSelfUpdate: core.mCanSelfUpdate
    property alias manualUpdateHint: core.manualUpdateHint
    property alias mPortableRun: core.mPortableRun
    property alias mInstalledVersion: core.mInstalledVersion
    property alias mInstallPromptDue: core.mInstallPromptDue
    property alias mInstalling: core.mInstalling
    property alias mUpdateStatus: core.mUpdateStatus
    property alias mReleaseNotes: core.mReleaseNotes
    property alias mActivity: core.mActivity
    property alias _activityRaw: core._activityRaw
    property alias mStatuses: core.mStatuses
    property alias _statusesRaw: core._statusesRaw
    property alias mMyStatus: core.mMyStatus
    property alias mStatusIsDm: core.mStatusIsDm
    property alias mActivityError: core.mActivityError
    property alias mStatusError: core.mStatusError
    property alias _gamePollAt: core._gamePollAt
    property alias mGifs: core.mGifs
    property alias _gifsRaw: core._gifsRaw
    property alias mGifLoading: core.mGifLoading
    property alias mGifError: core.mGifError
    property alias mRoomSounds: core.mRoomSounds
    property alias _roomSoundsRaw: core._roomSoundsRaw
    property alias mPersonalSounds: core.mPersonalSounds
    property alias _personalSoundsRaw: core._personalSoundsRaw
    property alias mSoundSearch: core.mSoundSearch
    property alias _soundSearchRaw: core._soundSearchRaw
    property alias mSoundSearching: core.mSoundSearching
    property alias mSoundError: core.mSoundError
    property alias mSoundCallPlaying: core.mSoundCallPlaying
    property alias mSoundUploading: core.mSoundUploading
    property alias mSbRecent: core.mSbRecent
    property alias mJoinSoundSearch: core.mJoinSoundSearch
    property alias _joinSoundSearchRaw: core._joinSoundSearchRaw
    property alias mJoinSoundSearching: core.mJoinSoundSearching
    property alias mWatchInParty: core.mWatchInParty
    property alias mWatchAdvancer: core.mWatchAdvancer
    property alias mWatchParty: core.mWatchParty
    property alias _watchPartyRaw: core._watchPartyRaw
    property alias mWatchPlayback: core.mWatchPlayback
    property alias _watchPlaybackRaw: core._watchPlaybackRaw
    property alias mWatchViewers: core.mWatchViewers
    property alias _watchViewersRaw: core._watchViewersRaw
    property alias mWatchSearch: core.mWatchSearch
    property alias _watchSearchRaw: core._watchSearchRaw
    property alias mWatchSearching: core.mWatchSearching
    property alias mWatchError: core.mWatchError
    property alias mCallParty: core.mCallParty   // 11.4 (Media Party): the call room's party (button dot + strip)
    property alias mWatchLink: core.mWatchLink   // 11.4: the one box's last addLink outcome
    property alias mWatchAdding: core.mWatchAdding
    property alias mWatchBrowse: core.mWatchBrowse     // 11.4 Browse pane
    property alias mWatchBrowsing: core.mWatchBrowsing
    property alias mWatchSuggest: core.mWatchSuggest
    property alias mWatchHistory: core.mWatchHistory
    property alias mMemories: core.mMemories
    property alias mMemoriesError: core.mMemoriesError
    property alias _memoriesRaw: core._memoriesRaw
    property alias mClips: core.mClips
    property alias _clipsRaw: core._clipsRaw
    property alias mNavSpaces: core.mNavSpaces
    property alias _navSpacesRaw: core._navSpacesRaw
    property alias mNavRooms: core.mNavRooms
    property alias mMyAvatar: core.mMyAvatar
    property alias _myAvatarAsked: core._myAvatarAsked
    property alias _navRoomsRaw: core._navRoomsRaw
    property alias mNavActiveSpace: core.mNavActiveSpace
    // RBAC (docs/RBAC.md): the space-settings + call-moderation mirrors.
    property alias mSpaceId: core.mSpaceId
    property alias mSpaceBusy: core.mSpaceBusy
    property alias mSpaceSummary: core.mSpaceSummary
    property alias mSpaceMembers: core.mSpaceMembers
    property alias mSpaceRoles: core.mSpaceRoles
    property alias mSpacePerms: core.mSpacePerms
    property alias mSpaceRooms: core.mSpaceRooms
    property alias mSpaceUserSearch: core.mSpaceUserSearch
    property alias mSpaceRolesActive: core.mSpaceRolesActive
    property alias mInviteOperator: core.mInviteOperator   // 10.4 invite codes (docs/INVITES.md)
    property alias mInviteBusy: core.mInviteBusy
    property alias mInvites: core.mInvites
    property alias mGuests: core.mGuests   // selfhost: the guest room (guests_ctl.rs view_json)
    property alias mGuestsHere: core.mGuestsHere   // the open room is THE Guests room
    property alias mInviteServer: core.mInviteServer
    // selfhost: what the user's server offers (server_offer.rs). Off = hidden, never dimmed.
    property alias mServerOffer: core.mServerOffer
    function serverOn(f) { return core.serverOn(f) }
    function widgetOffered(id) { return core.widgetOffered(id) }
    function permOffered(key) { return core.permOffered(key) }
    function settingOffered(id) { return core.settingOffered(id) }
    function paneOffered(key) { return core.paneOffered(key) }
    function serverPredicates() { return core.serverPredicates() }
    function isRecorder(id) { return core.isRecorder(id) }
    readonly property bool mGuestAccount: core.mGuestAccount
    readonly property bool serverMemories: core.serverOn("memories")
    readonly property bool serverSoundboard: core.serverOn("soundboard")
    // the signed-in account's server name ("@me:server" → "server"); "" signed out
    function myServer() { var u = String(root.mUserId || ""); var i = u.indexOf(":"); return i >= 0 ? u.substring(i + 1) : "" }
    property alias mSpaceMyRank: core.mSpaceMyRank
    property alias mPermissionCatalog: core.mPermissionCatalog
    property alias mSpaceStatus: core.mSpaceStatus
    property alias mSpaceStatusRev: core.mSpaceStatusRev
    property alias mSpaceError: core.mSpaceError
    property alias mSpaceErrorRev: core.mSpaceErrorRev
    property alias mRoomSpaceId: core.mRoomSpaceId
    property alias mCallPerms: core.mCallPerms
    property alias mCanModerateCall: core.mCanModerateCall
    property alias mServerMuted: core.mServerMuted
    property alias mServerMutedBy: core.mServerMutedBy
    property alias mRemovedBy: core.mRemovedBy
    property alias mServerMutedMap: core.mServerMutedMap
    function serverMutedByFor(identity, mxid) { return core.serverMutedByFor(identity, mxid) }
    function identityForMxid(mxid) { return core.identityForMxid(mxid) }
    property alias mNavActiveLabel: core.mNavActiveLabel
    property alias mNavBadge: core.mNavBadge
    // 10.6 (docs/REACH.md): the DM strip + presence mirrors, for ThSpaceRail / ThRoomList / RightPanel.
    property alias mNavDmStrip: core.mNavDmStrip
    property alias mPresence: core.mPresence
    property alias mPresenceKnown: core.mPresenceKnown
    property alias mPresenceSharing: core.mPresenceSharing
    property alias mPresenceAsked: core.mPresenceAsked
    property alias mPresenceTurningOff: core.mPresenceTurningOff
    property alias mPresenceBusy: core.mPresenceBusy
    property alias mPresenceMine: core.mPresenceMine
    property alias mPresenceMyRoom: core.mPresenceMyRoom
    property alias mMyPresenceStatus: core.mMyPresenceStatus
    property alias mStatusPresets: core.mStatusPresets
    property alias mStatusPalette: core.mStatusPalette
    function presenceOf(mxid) { return core.presenceOf(mxid) }
    function presenceWord(state) { return core.presenceWord(state) }
    function roomNameOf(id) { return core.roomNameOf(id) }
    property alias mCalEvents: core.mCalEvents
    property alias _calEventsRaw: core._calEventsRaw
    property alias mCalGames: core.mCalGames
    property alias _calGamesRaw: core._calGamesRaw
    property alias mCalUpcoming: core.mCalUpcoming
    property alias _calUpcomingRaw: core._calUpcomingRaw
    property alias mCalContainers: core.mCalContainers
    property alias _calContainersRaw: core._calContainersRaw
    property alias mCalContainer: core.mCalContainer
    property alias mCalContainerLabel: core.mCalContainerLabel
    property alias mCalCanEdit: core.mCalCanEdit
    property alias mCalIsMod: core.mCalIsMod
    property alias mCalMyId: core.mCalMyId
    property alias mCalBusy: core.mCalBusy
    property alias mCalReminders: core.mCalReminders
    property alias mCalError: core.mCalError
    property alias mCalMissing: core.mCalMissing
    property alias _calMissingRaw: core._calMissingRaw
    property alias mCalCanInvite: core.mCalCanInvite
    property alias mCalUnjoinedParent: core.mCalUnjoinedParent
    property alias mCalSourceLabel: core.mCalSourceLabel
    property alias _calRoom: core._calRoom
    property alias _calStarted: core._calStarted
    property alias _calAutoOpened: core._calAutoOpened
    property alias mIconPack: core.mIconPack
    property alias mReducedMotion: core.mReducedMotion
    property alias _iconPackRaw: core._iconPackRaw
    property alias _galleryOpened: core._galleryOpened
    property alias appIconUrl: core.appIconUrl
    property alias _bootShown: core._bootShown
    function _tileRow(t) { return core._tileRow(t) }
    function _syncTileModel() { return core._syncTileModel() }
    function _rebuildSelfviewRow() { return core._rebuildSelfviewRow() }
    function tileMxid(p) { return core.tileMxid(p) }
    function tileKey(md) { return core.tileKey(md) }
    function isKeyHidden(md) { return core.isKeyHidden(md) }
    function defaultFillModeFor(md) { return core.defaultFillModeFor(md) }
    function restoreTileView(t) { return core.restoreTileView(t) }
    function saveTileView(t) { return core.saveTileView(t) }
    function saveTileViewLater(t) { return core.saveTileViewLater(t) }
    function setSelfViewMode(m) { return core.setSelfViewMode(m) }
    function _persistHiddenKeys() { return core._persistHiddenKeys() }
    function isRecOnly(id) { return core.isRecOnly(id) }
    function isTunedIn(id) { return core.isTunedIn(id) }
    function isRecOnlyHidden(id) { return core.isRecOnlyHidden(id) }
    function isTileHidden(id) { return core.isTileHidden(id) }
    function isGameChat(identity) { return core.isGameChat(identity) }
    function isGameChatMuted(identity) { return core.isGameChatMuted(identity) }
    function isGameChatMxid(mxid) { return core.isGameChatMxid(mxid) }
    function cornerOf(identity) { return core.cornerOf(identity) }
    function cornerOfMxid(mxid) { return core.cornerOfMxid(mxid) }
    function isAside(identity) { return core.isAside(identity) }
    function isAsideMxid(mxid) { return core.isAsideMxid(mxid) }
    function inMyCorner(identity) { return core.inMyCorner(identity) }
    function cornerNames(ids) { return core.cornerNames(ids) }
    function cornerMatesOf(identity) { return core.cornerMatesOf(identity) }
    // 9.4 gate hook: the first participant in the call who is not me (a full identity).
    function _firstOtherParticipant() {
        var out = []
        for (var i = 0; i < root.mVideoTiles.length; i++) {
            var t = root.mVideoTiles[i]
            var p = t.participant || ""
            // The self tile carries participant "You"; real peers are full "@…" identities.
            if (t.id === "vid_selfview" || p.charAt(0) !== "@") continue
            if (root.tileMxid(p) !== login.userId && out.indexOf(p) === -1) out.push(p)
        }
        out.sort()   // deterministic: the gate's roles depend on it (test2 before test3)
        return out.length > 0 ? out[0] : ""
    }
    function isShareTile(t) { return core.isShareTile(t) }
    function isShareTileId(id) { return core.isShareTileId(id) }
    function isWatched(id) { return core.isWatched(id) }
    function isListening(id) { return core.isListening(id) }
    function setListening(id, on) { return core.setListening(id, on) }
    function _syncWatched() { return core._syncWatched() }
    function _hookArg(flag) { return core._hookArg(flag) }
    function _navHasCall() { return core._navHasCall() }
    function hideTile(id) { return core.hideTile(id) }
    function hideTileRemember(md) { return core.hideTileRemember(md) }
    function unhideTile(id) { return core.unhideTile(id) }
    function unhideAllTiles() { return core.unhideAllTiles() }
    function _applyHiddenMemory() { return core._applyHiddenMemory() }
    function _applyVisibilityPause() { return core._applyVisibilityPause() }
    function openPip(tileId, participant, name, isShare) { return core.openPip(tileId, participant, name, isShare) }
    function persistChatMemory() { core.persistChatMemory() }
    function markLocalCorner() { core._cornerLocal = true }   // 10.1 (TH-FLOAT-02)
    function refreshDeviceMenus() { return core.refreshDeviceMenus() }
    function ensureOverlay() { return core.ensureOverlay() }
    function playCallChime(kind) { return core.playCallChime(kind) }
    function playUiSound(kind) { return core.playUiSound(kind) }
    function previewSound(file) { return core.previewSound(file) }
    function startRing() { return core.startRing() }
    function stopRing() { return core.stopRing() }
    // 10.6 (REACH A5.6, audit 15d): the OS notification only when the window is NOT active — the
    // in-app card and the ring already cover a focused window (the web posts it only when hidden).
    function notifyIncomingCall(roomId, roomName, who) { if (settings.pref("notify.calls") !== "0" && !root.active) login.notifyIncomingCall(roomId, roomName, who) }
    // Islands C2 gate hook: --tinhat-fracprobe lists, 12 s after start (and again at 24 s), every visible item whose
    // window-space top or height is not a whole pixel — the lengths the dock host will measure must be integers (the
    // solver is integer-only; ISLANDS-ARCH §4.2 C2). Never on for users.
    Timer {
        interval: 12000; repeat: true; running: Qt.application.arguments.indexOf("--tinhat-fracprobe") >= 0
        property int n: 0
        onTriggered: {
            n++; if (n > 2) { stop(); return }
            var out = [], seen = 0
            function frac(v) { return Math.abs(v - Math.round(v)) > 0.001 }
            function walk(it, depth) {
                if (!it || !it.visible || depth > 40) return
                seen++
                var p = it.mapToItem(null, 0, 0)
                if ((frac(p.y) || frac(it.height)) && it.height > 0.5 && it.width > 0.5)
                    out.push(depth + " " + String(it).replace(/_QMLTYPE_\d+|\(0x[0-9a-f]+\)/g, "") + (it.objectName ? " '" + it.objectName + "'" : "")
                             + " y=" + p.y.toFixed(3) + " h=" + it.height.toFixed(3) + " x=" + p.x.toFixed(1) + " w=" + it.width.toFixed(1))
                for (var i = 0; i < it.children.length; i++) walk(it.children[i], depth + 1)
            }
            walk(root.contentItem, 0)
            console.log("[fracprobe] pass " + n + ": " + out.length + " of " + seen + " visible items fractional")
            for (var j = 0; j < Math.min(out.length, 400); j++) console.log("[fracprobe] " + out[j])
        }
    }

    // 11.0 GPU gate hooks (modular/gpu/desktop/idle-gate.sh --extra; never on for users):
    //   --tinhat-busymark   the house busy mark, no progress, over the window (the "busy indicator 60 s" row)
    //   --tinhat-ring=SECS  a test incoming-call card SECS after the main window shows (the ring rows; no call exists)
    readonly property bool _busyProbe: Qt.application.arguments.indexOf("--tinhat-busymark") >= 0
    readonly property int _ringProbe: {
        var a = Qt.application.arguments
        for (var i = 0; i < a.length; i++) if (String(a[i]).indexOf("--tinhat-ring=") === 0) return parseInt(String(a[i]).substring("--tinhat-ring=".length)) || 20
        return 0
    }
    ThBusyIndicator {
        visible: root._busyProbe
        running: visible
        anchors.centerIn: parent
        z: 900
        size: 36
        color: Th.loaderSpinnerFg
        label: "Loading…"
    }
    Timer {
        interval: Math.max(1, root._ringProbe) * 1000
        running: root._ringProbe > 0
        onTriggered: { console.log("[gpugate] ring probe: presenting a test incoming-call card"); incomingToast.present("!gpugate-ring:test", "Ring test", "jordan") }
    }

    // 10.0 incoming calls: the card over everything + the ring; Join/Switch opens the room and
    // hands the call controller the switch (leave-then-join when already in a call).
    IncomingCallToast {
        id: incomingToast
        app: root
        parent: Controls.Overlay.overlay
        z: 1000
        width: Math.min(parent.width - Kirigami.Units.gridUnit * 2, Kirigami.Units.gridUnit * 28)
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.top: parent.top
        anchors.topMargin: Kirigami.Units.gridUnit
        onJoinRequested: function (roomId) {
            root.openRoomId(roomId)
            callctl.switchCall(roomId)
        }
    }
    Connections {
        target: login
        function onIncomingCall(roomId, roomName, who) { incomingToast.present(roomId, roomName, who) }
        function onIncomingCallEnded(roomId) { incomingToast.ended(roomId) }
        // selfhost/firstrun §5: accepting a DM invite (room list or DM strip) opens that DM
        function onDmAccepted(roomId) { root.openRoomId(roomId) }
        function onToastAction(action) {
            if (action === "sound:notify") { root.playUiSound("notify"); return }
            // 10.6 (REACH A5.6): a new DM / DM invite / mention flashes the taskbar button while the
            // window is not active (desktop notifications on — Rust decides).
            if (action === "alert") { if (!root.active) root.alert(0); return }
            // 10.6 (REACH A5.1): a NEW DM invite — the notify sound (the OS notification "<inviter>
            // wants to chat" and the "alert" flash come from Rust). A NEW room invite also gets the
            // in-app toast.
            if (action.indexOf("invite-dm:") === 0) { root.playUiSound("notify"); return }
            // selfhost (docs/GUESTS.md, the web's watch.ts): "Ana is waiting to come in as a guest" — the in-app toast
            // with Open (the chime and the OS notification come from Rust, guests_ctl.rs announce)
            if (action.indexOf("guest-waiting:") === 0) {
                var gt = action.indexOf("\t"), gRoom = action.substring(14, gt > 0 ? gt : action.length)
                if (gt > 0) root.showPassiveNotification(LinkGate.plain(action.substring(gt + 1)), 8000, "Open", function () { root.openRoomId(gRoom) })
                return
            }
            if (action.indexOf("invite-room:") === 0) {
                var tab = action.indexOf("\t")
                if (tab > 0) root.showPassiveNotification(LinkGate.plain(action.substring(tab + 1)), 4000)
                root.playUiSound("notify")
                return
            }
            // A clicked invite notification: "invite:people:<id>" (a DM → People, where it waits
            // with Accept / Decline) or "invite:home:<id>" (a room → Home).
            if (action.indexOf("invite:") === 0) {
                var inv = action.substring(7), c = inv.indexOf(":")
                root.visible = true; root.raise(); root.requestActivate()
                if (c > 0 && nav.activeSpace !== inv.substring(0, c)) nav.selectSpace(inv.substring(0, c))
                return
            }
            if (action.indexOf("join:") === 0) { root.requestActivate(); incomingToast.join(action.substring(5)); return }
            // 10.1 (TH-GAP-06 / TH-CAL-11): a clicked message toast opens the room; a clicked
            // reminder opens the event.
            if (action.indexOf("open:") === 0) {
                root.visible = true; root.raise(); root.requestActivate()
                root.openRoomId(action.substring(5))
                return
            }
            if (action.indexOf("calendar:") === 0) {
                var rest = action.substring(9), k = rest.indexOf("/")
                root.visible = true; root.raise(); root.requestActivate()
                if (k > 0) root.openCalendarEvent(rest.substring(0, k), rest.substring(k + 1))
                return
            }
        }
    }
    // selfhost: the guest room's answers (guests_ctl.rs): its toasts where the person is looking, the guest link onto the
    // clipboard, "Open the Guests room" (joined first when needed) — Settings steps aside for the room
    Connections {
        target: guestsCtl
        function onToast(text, timeoutMs) { root.settingsToast(text, timeoutMs) }
        function onCopyReady(link, text) { root.copyToClipboard(link); root.settingsToast(text, 5000) }
        function onOpenReady(roomId) {
            settingsWindow.close()
            root.visible = true; root.raise(); root.requestActivate()
            root.openRoomId(roomId)
        }
    }
    function ecOutputLabels() { return core.ecOutputLabels() }
    function toggleEcOutput(node) { return core.toggleEcOutput(node) }
    function shareCurRes() { return core.shareCurRes() }
    function shareCurTier() { return core.shareCurTier() }
    function setCallLayout(m) { return core.setCallLayout(m) }
    function verNewerOrSame(a, b) { return core.verNewerOrSame(a, b) }
    function _bootReveal(why) { return core._bootReveal(why) }
    function _sqTokens() { return core._sqTokens() }
    function _sqParts() { return core._sqParts() }
    function _sqW() { return core._sqW() }
    function _sqH() { return core._sqH() }
    function _sqFps() { return core._sqFps() }
    function _sqKbps() { return core._sqKbps() }
    function _sqEnc() { return core._sqEnc() }
    function _cqTokens() { return core._cqTokens() }
    function _cqParts() { return core._cqParts() }
    function _cqW() { return core._cqW() }
    function _cqH() { return core._cqH() }
    function _cqSim() { return core._cqSim() }
    function tileAudioLevel(participant, isSelf) { return core.tileAudioLevel(participant, isSelf) }
    function localpart(mxid) { return core.localpart(mxid) }
    function senderColor(mxid) { return core.senderColor(mxid) }
    function dayLabel(ms) { return core.dayLabel(ms) }
    function sameDay(a, b) { return core.sameDay(a, b) }
    onVisibilityChanged: root._applyVisibilityPause()   // Window signal; the work lives in the core
    title: "Tin Hat"
    // 0.8.24: hidden until the session is restored and the rooms are in (or there is no
    // session) — the StartupWindow narrates the wait; _bootReveal() shows us + closes it.
    visible: false
    width: 980
    height: 720
    minimumWidth: 480
    minimumHeight: 520
    // 10.1 (TH-SHELL-05 / TH-PREF-07): the window comes back the size (and, off Wayland, the place)
    // you left it, maximised if it was maximised. Every launch used to be 980×720.
    WindowMemory { id: mainMemo; target: root; key: "main"; store: settings }

    // Closing the main window must QUIT the process — not just hide it. cxx-qt-lib 0.8 can't
    // set quitOnLastWindowClosed from Rust, and secondary top-level windows (PiP / floating
    // chats / WebEngine surfaces) keep the auto-quit from firing, so the event loop would run
    // forever with the call still publishing (the zombie-camera-broadcast bug). Qt.quit()
    // makes app.exec() return regardless; main.rs then runs the (bounded) leave_on_exit()
    // teardown which releases the camera + disconnects the SFU.
    onClosing: (close) => {
        // 10.1 (TH-SHELL-04): mid-call, the ✕ asks first; quitConfirm re-enters with the flag set.
        if (root.mInCall && !root._quitConfirmed) {
            close.accepted = false
            quitConfirm.open()
            return
        }
        // Close secondary top-level Windows FIRST. cxx-qt-lib 0.8 can't set
        // quitOnLastWindowClosed from Rust, so a lingering secondary window (soundboard /
        // watch-party) can keep the event loop alive even after Qt.quit() — leaving a zombie
        // that never runs leave_on_exit (the SFU-socket ghost). Closing them makes quit reliable.
        soundboardPanel.close()
        watchPartyWindow.quietClose = true   // 11.3 QOL: no Rejoin toast while quitting
        if (watchPartyWindow.close) watchPartyWindow.close()
        if (memoriesWindow.close) memoriesWindow.close()   // Y3: leaves a synced replay cleanly
        if (calendarWindow.close) calendarWindow.close(); gameReelWindow.close()   // 0.8.26
        if (iconGallery.close) iconGallery.close()
        if (_overlayWin) _overlayWin.close()
        Qt.quit()
    }

    // ---- Theming (desktop C1, docs/MIGRATING-THEME.md): every colour comes from the Th singleton — the ACTIVE theme,
    // one colour per role and element token (qml/Th.qml, generated from crates/theme/tokens.json), filled by
    // AppCore.applyTheme() through the Rust engine at start and on every switch. Paint an element with its token
    // (Th.composerBorderFocus); a status colour (danger/warning/success/onAccent) is a token too — never a literal.

    property string baseFontFamily: Qt.application.font.family
    font.family: baseFontFamily

    // ---- Global polish design tokens (shared with the component .qml files,
    // which receive `root` as their `app` property). Radii grow a touch for furry.
    readonly property real radiusSm: 5
    readonly property real radius: 8
    readonly property real radiusLg: 12
    readonly property real radiusXl: 16
    readonly property real pad: Kirigami.Units.largeSpacing
    readonly property real padLg: Kirigami.Units.largeSpacing * 1.5
    // 11.0 (GPU.md §1, R3): THE duration token and its steps come from the Motion singleton — 0 while the motion
    // switch is off (window not focused, or Reduce motion — the app's or the platform's), so every Behavior in the
    // app snaps then, and the reel jumps. Steps: dur 120 · durShort 100 · durSlow 180 · durLong 200 · durVeryLong 400.
    readonly property int dur: Motion.dur
    readonly property int durShort: Motion.durShort
    readonly property int durLong: Motion.durLong
    readonly property int durVeryLong: Motion.durVeryLong
    readonly property bool animate: Motion.animate
    readonly property string winState: Motion.winState
    readonly property bool reducedMotion: Motion.reducedMotion
    // 11.0 (GPU.md): the low-GPU contract's rules (lowgpu_ctl.rs) for components that take `app`
    readonly property LowGpuController lowgpu: core.c_lowgpu
    // 11.0 (GPU.md §1): this window's REAL exposure — written by the C++ input filter (video_bridge.cpp) on every
    // expose / show / hide event; AppCore's window state reads it (KWin Wayland never reports a minimise).
    property bool thExposed: true
    // 11.0 (GPU.md §1): the in-app frame meter — TINHAT_FRAME_METER=1 logs this window's drawn frames every 5 s ("[frames]
    // main …"), for the gates on a GPU machine or under Wayland where the scene-graph log isn't available. Off: nothing runs.
    property int _frames: 0
    Connections { target: root; enabled: root.lowgpu.frameMeter; function onFrameSwapped() { root._frames++ } }
    Timer {
        interval: 5000; repeat: true; running: root.lowgpu.frameMeter
        onTriggered: { root.lowgpu.logFrames("main", root._frames, 5); root._frames = 0 }
    }
    // 10.1 (TH-PREF-11): one clock for the app — "system" | "12" | "24" (Appearance pane).
    property string clockMode: "system"
    function setClockMode(m) { root.clockMode = m; settings.applyUiPref("clock", m) }
    function fmtClock(ms) {
        var d = new Date(ms)
        return root.clockMode === "12" ? Qt.formatTime(d, "h:mm AP")
             : root.clockMode === "24" ? Qt.formatTime(d, "HH:mm")
             : Qt.formatTime(d, Locale.ShortFormat)
    }
    readonly property int durSlow: Motion.durSlow


    // 0.8.31: right-click / long-press a call tile → that person's volume dials, the
    // SAME menu the left bar opens (ParticipantVolumeMenu). Own tile: nothing to set. The menu
    // is anchored to the window content, not the delegate — tiles are rebuilt on roster
    // changes and a Menu whose parent dies closes with it.
    function openTileVolumeMenu(tile, pos) {
        if (!tile || tile.isSelf || !tile.modelData) return
        var md = tile.modelData
        var mxid = root.tileMxid(md.participant || "")
        if (mxid.length === 0) return
        tileVolMenu.mxid = mxid
        tileVolMenu.participant = md.participant || ""   // 9.4: corners target a full identity
        // Same fallback as the tile's name strip: display name, else the localpart.
        tileVolMenu.name = (md.name && md.name.length > 0) ? md.name : root.localpart(md.participant || "")
        // Mapped INTO root.contentItem, the item it pops at (fix/sliders-volume): mapToItem(null) gave window coordinates,
        // and the content sits under the window-wide banner strip (header), so with a banner up (unverified device,
        // update, offline, safe mode) the menu opened one banner-height (~50 px) below the pointer.
        var pt = tile.mapToItem(root.contentItem, pos.x, pos.y)
        tileVolMenu.popup(root.contentItem, pt.x, pt.y)
    }

    // Navigation view mode (saved, like the theme) — ONE view type or the other:
    //   "persistent" = the Discord-style Spaces rail + room panel pinned as a left
    //                  sidebar, with the selected room as context (default);
    //   "corner"     = the SAME nav tucked into a top-left hover dock, no static
    //                  sidebar at all (for OLED users avoiding burn-in).
    // DynamicColumns shows sidebar+conversation side-by-side; SingleColumn = conversation
    // fills the window (the nav lives only in the corner dock).
    readonly property bool sidebarCollapsed: navMode === "corner"
    // 11.0 (islands C5): the dock host reads the mode through its view state (root.dockView: corner = the nav
    // collapsed into the drawer; "fill the app" collapses it too), so nothing here touches a page row any more.
    function setNavMode(mode) {
        if (mode !== "persistent" && mode !== "corner") return
        root.navMode = mode
        login.setNavMode(mode)
    }
    // "Hide room list" now means "tuck the nav into the corner" and vice-versa.
    function toggleSidebar() { root.setNavMode(root.navMode === "persistent" ? "corner" : "persistent") }


    // ---- Voice overlay HUD (OverlayWindow.qml): created lazily and dynamically so a
    // platform without org.kde.layershell (e.g. a future Windows build) just loses the
    // feature instead of failing Main.qml's load. All state flows through bindings on
    // `app`; the window manages its own visibility (it unmaps itself when quiet).
    // GUI-verification hooks (screenshot automation without coordinate clicking):
    //   --tinhat-settings   opens the Settings sheet after login and advances one pane
    //                        every 4s (wraps), so a screenshot loop can tour all panes.
    Timer {
        id: guiSettingsTour
        property int tick: 0
        interval: 4000; repeat: true
        running: Qt.application.arguments.indexOf("--tinhat-settings") >= 0 && root.mLoggedIn
        onTriggered: {
            if (tick === 0) settingsWindow.openAt("profile")
            else if (!settingsWindow.visible) { running = false; return } // user closed it — stop
            else settingsWindow.cyclePane(tick - 1)
            tick++
        }
    }

    //   --tinhat-pane=<key|n>   opens Settings PINNED to that pane (a key — profile, privacy, security, invites, audio,
    //                        calls, appearance, notifications, shortcuts, overlay, about — or a 10.x number) and never
    //                        advances, so a screenshot/interaction test can click and type without racing the tour timer
    //                        above (which cycles every 4s and silently moved the target mid-test).
    readonly property string guiPinnedPane: {
        var args = Qt.application.arguments
        for (var i = 0; i < args.length; i++)
            if (args[i].indexOf("--tinhat-pane=") === 0) return String(args[i].substring("--tinhat-pane=".length))
        return ""
    }
    Timer {
        id: guiPanePin
        interval: 3500; repeat: false
        running: root.guiPinnedPane.length > 0 && root.mLoggedIn
        onTriggered: settingsWindow.openAt(root.guiPinnedPane)
    }




    // Open the zoomable image lightbox (called from ChatBubble.imageClicked, incl. floating
    // chats which reach it via their `app` reference).
    // 10.1 (TH-FLOAT-08, interim): a photo tapped in a floating chat-head opens HERE, so bring
    // this window forward instead of opening the lightbox behind the game.
    function openLightbox(url, w, h, alt) { root.raise(); root.requestActivate(); lightbox.show(url, w, h, alt) }
    function openProfileFlyout(mxid) { root.profileMxid = mxid; root.raise(); root.requestActivate(); profileFlyout.open() }
    // widgets (step 2): the app menu's "Add widget…" (WidgetHost opens its gallery), Up next's "+ New event" and
    // "Game TBD · Pick" (the Game Reel on that event, its container first).
    signal widgetGalleryRequested()
    function openCalendarNew() { calendarWindow.openOrRaise(); calendarWindow.startNew("") }
    function openGameReelFor(container, id) { gamesCtl.setRoom(container); gamesCtl.setSession(id); gameReelWindow.openOrRaise() }
    // 0.8.33: calendar cards in chat — open the event in the calendar window / answer in place.
    function openCalendarEvent(container, id) {
        calendarWindow.openOrRaise()
        calCtl.openEvent(container, id)
    }
    function calendarRsvp(container, id, status) {
        if (container.length > 0 && container !== root.mCalContainer) { openCalendarEvent(container, id); return }
        var ev = null
        for (var i = 0; i < root.mCalEvents.length; i++) if (root.mCalEvents[i].id === id) { ev = root.mCalEvents[i]; break }
        var cur = (ev && ev.my) ? String(ev.my.status || "") : ""
        var opts = (ev && ev.my && ev.my.options) ? ev.my.options : []
        calCtl.rsvp(id, cur === status ? "" : status, JSON.stringify(opts))
    }
    function openEcOutputs() { callOptions.close(); callctl.refreshOutputs(); ecOutputsPopup.open() }   // 10.1 (TH-POP-18): one panel at a time
    function openAudioSettings() { settingsWindow.openAt("audio") }
    // 0.8.33: the call CHROME — every tile button and the call bar — stays out of the way.
    // It shows only while the window is focused AND the pointer is over the call, fades out after
    // 3 s without pointer movement and comes straight back on movement. A popover / menu opened
    // from the bar, or the pointer resting on the bar, holds it. Layout never moves: the bar keeps
    // its space and fades (a slight rise), so nothing jumps under the pointer.
    property bool callPointerIdle: false
    readonly property bool chromeProbe: Qt.application.arguments.indexOf("--tinhat-chrome") >= 0 // gates: always shown
    // The call page is created by the PageRow in its OWN component scope, so root cannot reach
    // its ids — the page PUBLISHES hover / bar-hover / menu state into these three instead.
    property bool callHoverActive: false   // pointer over the call panel
    property bool callBarHovered: false    // pointer resting on the call bar
    property bool callMenuHold: false      // a menu / popup opened from the bar is up
    readonly property bool callChromeShown: root.chromeProbe
        || (core.mMutedTalkBubble && root.mInCall) // 10.5: the "You're muted" bubble brings the bar (and its pulsing mic) up
        || (root.active && ((root.callHoverActive && !root.callPointerIdle) || root.callBarHovered || root.callMenuHold
                            || callOptions.opened || streamPopover.opened || soundboardPanel.opened || tileVolMenu.opened || ecOutputsPopup.opened))
    // 11.0.2 (2026-09-28): the call BAR also stays while "Keep call buttons visible" is on (its pin, Settings →
    // Calls, call.bar.pin.toggle) — whatever the pointer or the focus; the tile buttons keep following callChromeShown.
    // A pinned bar is static: no timer or animation of its own (the idle timer only feeds the tile chrome then).
    readonly property bool callBarShown: root.callChromeShown || root.callBarPinned
    function callChromeMoved() { root.callPointerIdle = false; callChromeIdle.restart() }
    Timer { id: callChromeIdle; interval: 3000; onTriggered: root.callPointerIdle = true }
    // 0.8.33 gate hook: --tinhat-callopts=mic|speaker|camera|outputs opens that slide-out over
    // the (fake) call bar for screenshots.
    readonly property string callOptsProbe: {
        var a = Qt.application.arguments
        for (var i = 0; i < a.length; i++) if (String(a[i]).indexOf("--tinhat-callopts=") === 0) return String(a[i]).substring("--tinhat-callopts=".length)
        return ""
    }

    // LevelMeter (the house meter) is its own file since the Settings window (LevelMeter.qml).
    // Look up a nav room by id (for the context menu).
    // Tapped a room you can see but have not joined. Confirm first — joining is public
    // (everyone there sees you arrive), so it must never happen on a stray click. Once
    // confirmed it is seamless: no invite round-trip.
    property string joinRoomId: ""
    property string joinRoomName: ""
    function askJoinRoom(roomId, roomName) {
        root.joinRoomId = roomId
        root.joinRoomName = roomName
        joinConfirm.open()
    }

    // 0.8.27: right-click on a space bubble. A space is a room, so the invite dialog, the
    // link helper and leaveRoom all work on it unchanged — the rail just never offered them.
    function openSpaceContext(id, label) {
        root.ctxRoomId = id
        root.ctxRoomName = label
        root.ctxRoomAlias = ""; root.ctxRoomPublic = false
        // 10.1 (TH-GAP-05): the main room list knows a space's alias + join rule too.
        var rooms = root.mRooms || []
        for (var i = 0; i < rooms.length; i++)
            if (rooms[i].id === id) { root.ctxRoomAlias = String(rooms[i].alias || ""); root.ctxRoomPublic = rooms[i].publicJoin === true; break }
        root.ctxRoomFav = false
        root.ctxSpaceInvited = false
        var sps = root.mNavSpaces || []
        for (var s = 0; s < sps.length; s++)
            if (sps[s].key === id) { root.ctxSpaceInvited = sps[s].membership === "invite"; break }
        spaceMenu.popup()
    }
    property bool ctxSpaceInvited: false   // RBAC §7.3: the menu's space is a pending invite
    Controls.Menu {
        id: spaceMenu
        // RBAC §7.1: first, when a member.
        Controls.MenuItem {
            text: "Space settings…"
            icon.name: "configure-symbolic"
            visible: !root.ctxSpaceInvited
            height: visible ? implicitHeight : 0
            onTriggered: root.openSpaceSettings(root.ctxRoomId, "overview")
        }
        // 10.1 (TH-NAV-03): every unread room in this space, read.
        Controls.MenuItem {
            text: "Mark all as read"
            icon.name: "checkmark-symbolic"
            onTriggered: { nav.markSpaceRead(root.ctxRoomId); root.showPassiveNotification(LinkGate.plain("Marking everything in " + root.ctxRoomName + " as read…"), "short") }
        }
        Controls.MenuItem {
            text: "Invite people…"
            icon.name: "list-add-user"
            onTriggered: { inviteField.text = ""; inviteDialog.open() }
        }
        Controls.MenuItem {
            text: "Copy space link"
            icon.name: "link-symbolic"
            onTriggered: {
                root.copyToClipboard(root.roomInviteLink(root.ctxRoomId, root.ctxRoomAlias))
                showPassiveNotification(root.ctxRoomPublic ? "Space link copied — anyone with it can join" : "Space link copied — they will still need an invite", 2500)
            }
        }
        Controls.MenuItem {
            text: "Open the space calendar"
            icon.name: "view-calendar-symbolic"
            onTriggered: { calCtl.openContainer(root.ctxRoomId); calendarWindow.openOrRaise() }
        }
        Controls.MenuSeparator {}
        Controls.MenuItem {
            text: "Leave space"
            icon.name: "go-previous-symbolic"
            onTriggered: leaveSpaceConfirm.open()
        }
    }
    ConfirmDialog {
        id: leaveSpaceConfirm
        app: root
        dialogTitle: "Leave space?"
        confirmText: "Leave"
        destructive: true
        message: "Leave \"" + root.ctxRoomName + "\"? Its rooms stay joined; you'll need a new invite to get the space (and its calendar) back."
        onConfirmed: login.leaveRoom(root.ctxRoomId)
    }
    function openRoomContext(id) {
        for (var i = 0; i < root.mNavRooms.length; i++) {
            if (root.mNavRooms[i].id === id) {
                root.ctxRoomId = id
                root.ctxRoomName = root.mNavRooms[i].name
                root.ctxRoomFav = root.mNavRooms[i].favourite === true
                root.ctxRoomUnread = Number(root.mNavRooms[i].unread || 0)
                root.ctxRoomNotify = String(root.mNavRooms[i].notifyMode || "all")
                root.ctxRoomAlias = String(root.mNavRooms[i].alias || "")
                root.ctxRoomPublic = root.mNavRooms[i].publicJoin === true
                roomMenu.popup()
                return
            }
        }
    }

    // Reusable round call-control button (mic / camera / share / leave) with a hover
    // grow + glow. `danger` = red (leave); `active` = accent-filled (toggled-on state);
    // `iconColor` lets a call site tint the glyph (e.g. muted mic = danger red so the
    // bar agrees with the tile pill); `busy` swaps the icon for a spinner (hang-up).
    // Full state set: hover, PRESSED (darker settle), DISABLED (0.5 + tooltip still
    // hoverable, so "Hanging up…" can explain itself), keyboard (tab + Space/Return +
    // visible focus ring — the nine most-used buttons in the app were mouse-only).
    // 10.5 "can they hear me?": a header pill — one measured fact, at most one action. Tones: "info"
    // (neutral — what to try), "warn" (amber — degraded, or someone else is affected), "danger" (red —
    // you can't be heard / can't hear). Hover explains; clicking the pill runs its one fix.
    // Recolor via BOTH channels for full coverage: the QtQuick Controls `palette`
    // (cascades to every Controls child — buttons, toolbars, list views, fields) AND
    // Kirigami.Theme (Kirigami components + our message bubbles, which use
    // Kirigami.Theme.highlightColor). Persistence is added back once theming is proven.
    // themes (C1): the role layer (NAMING.md §2) — every role the palette/Kirigami set before, no new ones (P46).
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

    Kirigami.Theme.inherit: false
    Kirigami.Theme.backgroundColor: Th.surfaceWindow
    Kirigami.Theme.textColor: Th.textPrimary
    Kirigami.Theme.disabledTextColor: Th.textSecondary
    Kirigami.Theme.highlightColor: Th.accentBase
    Kirigami.Theme.highlightedTextColor: Th.textOnAccent
    Kirigami.Theme.alternateBackgroundColor: Th.surfaceIsland
    Kirigami.Theme.activeTextColor: Th.accentBright
    Kirigami.Theme.linkColor: Th.textLink
    Kirigami.Theme.positiveTextColor: Th.stateSuccess
    Kirigami.Theme.negativeTextColor: Th.stateDangerBright
    // PARITY P71: Kirigami's ONE neutral colour (the NeutralText emblems + the Warning InlineMessage), never set
    // before (Kirigami's #f67400) — the token pins exactly that for OLED/Goose; a palette theme gets its warning.
    Kirigami.Theme.neutralTextColor: Th.iconSchemeNeutral
    Kirigami.Theme.hoverColor: Th.accentBase
    Kirigami.Theme.focusColor: Th.accentBase
    color: Th.windowBg
    // ENGINE.md §10 on the desktop (11.0 step 5): a theme's look puts its interface font (type.ui) and its text size
    // (type.size) on the window's font — every control in the window inherits it, popups and dialogs included
    // (ApplicationWindow font propagation); the house text components scale their own sizes by Th.textScale and the
    // messages take Th.fontChat / Th.fontMono. Only while the theme has them: without a look nothing is set here.
    Binding { target: root; property: "font.family"; value: Th.fontUi; when: Th.fontUi.length > 0; restoreMode: Binding.RestoreValue }
    Binding {
        target: root; property: "font.pointSize"; restoreMode: Binding.RestoreValue
        when: Th.textSize !== 100 && Kirigami.Theme.defaultFont.pointSize > 0
        value: Kirigami.Theme.defaultFont.pointSize * Th.textScale
    }

    // 0.8.16: a release bundle running from a folder (a zip in Downloads) offers to
    // install itself ONCE — Start-menu shortcut, Apps & features entry, and from then
    // on the in-app updater replaces "re-download the zip for every release".
    Timer {
        id: installPromptTimer
        interval: 2500
        repeat: false
        property bool done: false
        running: root.mLoggedIn && root.mInstallPromptDue && !done
        onTriggered: { done = true; if (root.mInstallPromptDue) installDialog.open() }
    }
    Kirigami.PromptDialog {
        id: installDialog
        title: root.mInstalledVersion.length > 0 ? "Tin Hat is already installed" : "Install Tin Hat?"
        standardButtons: Kirigami.Dialog.NoButton
        readonly property bool installedIsNewer: root.mInstalledVersion.length > 0 && root.verNewerOrSame(root.mInstalledVersion, root.mAppVersion)
        customFooterActions: [
            Kirigami.Action {
                text: root.mInstalledVersion.length > 0
                      ? (installDialog.installedIsNewer ? "Open the installed app" : "Update the installed app to this version")
                      : "Install"
                icon.name: "system-software-install"
                enabled: !root.mInstalling
                onTriggered: {
                    if (installDialog.installedIsNewer) updater.openInstalled()
                    else updater.installToApps()
                }
            },
            Kirigami.Action {
                text: "Not now"
                enabled: !root.mInstalling
                onTriggered: { updater.dismissInstallPrompt(); installDialog.close() }
            }
        ]
        ColumnLayout {
            spacing: 8
            Controls.Label {
                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                Layout.fillWidth: true
                Layout.preferredWidth: Kirigami.Units.gridUnit * 26
                wrapMode: Text.WordWrap
                text: root.mInstalledVersion.length > 0
                    ? ("You're running Tin Hat " + root.mAppVersion + " from a folder, but version " + root.mInstalledVersion
                       + " is already installed on this PC with a Start-menu shortcut."
                       + (installDialog.installedIsNewer ? " The installed one is the one that keeps itself up to date."
                                                         : " This copy is newer — it can replace the installed one."))
                    : "You're running Tin Hat straight from the folder you unzipped it into. Installing takes a few seconds: "
                      + "it goes into your apps with a Start-menu and desktop shortcut, and from then on it updates itself — "
                      + "no more downloading a new zip for every release."
            }
            Controls.Label {
                Layout.fillWidth: true
                wrapMode: Text.WordWrap
                color: Th.dialogHintFg
                font.pointSize: Kirigami.Theme.smallFont.pointSize
                text: root.mInstalling ? "Copying… the installed app opens by itself when it's done."
                    : "No administrator needed. You can always do this later from Settings → About."
            }
            ThBusyIndicator { Layout.alignment: Qt.AlignHCenter; size: 36; color: Th.loaderSpinnerFg; visible: root.mInstalling; running: root.mInstalling }   // 11.0: still
        }
    }



    // ---- Room-list search (filters the room list by name) ----
    property string roomFilter: ""

    // "Save as…" from a file card. One dialog reused for every attachment: the pending
    // event id is captured when it opens, the same pattern the room context menu uses.
    property string saveMediaId: ""
    function askSaveMedia(eventId, suggestedName) {
        root.saveMediaId = eventId
        // Pre-fill the sender's filename so people keep the name they were given.
        saveMediaDialog.selectedFile = saveMediaDialog.currentFolder + "/" + suggestedName
        saveMediaDialog.open()
    }
    FileDialog {
        id: roomPhotoDialog
        title: "Choose a room photo"
        nameFilters: ["Images (*.png *.jpg *.jpeg *.gif *.webp)"]
        onAccepted: room.applyRoomPhoto(String(selectedFile))
    }
    FileDialog {
        id: saveMediaDialog
        title: "Save attachment"
        fileMode: FileDialog.SaveFile
        onAccepted: if (root.saveMediaId.length > 0) room.saveMediaAs(root.saveMediaId, String(selectedFile))
    }


    // ---- Room-list context menu (right-click / long-press a room) ----
    property string ctxRoomId: ""
    property string ctxRoomName: ""
    property bool ctxRoomFav: false
    property int ctxRoomUnread: 0          // 10.1 (TH-NAV-03): offers "Mark as read"
    property var roomInfoPanel: null       // 10.1 (TH-CONV-08): the room page's docked info column (null until a room is open)
    property string ctxRoomNotify: "all"   // "all" | "mentions" | "mute"
    property string ctxRoomAlias: ""
    property bool ctxRoomPublic: false
    /// The best shareable link for a room. Prefers the published alias, which is
    /// readable and resolvable anywhere; otherwise falls back to the room id WITH a via
    /// hint, because a bare id leaves the receiving client with no server to ask.
    function roomInviteLink(roomId, alias) {
        if (alias && alias.length > 0)
            return "https://matrix.to/#/" + encodeURIComponent(alias)
        var server = roomId.indexOf(":") >= 0 ? roomId.split(":").pop() : root.myServer()
        return "https://matrix.to/#/" + encodeURIComponent(roomId) + "?via=" + server
    }
    // 10.1 (TH-SET-11 / TH-SET-03): a surface opened FROM Settings brings you back to the pane you left — settingsHandOff
    // (by the Settings window below); key backup set-up is its "backup" hand-off.
    // 10.1: "3 days ago" for timestamps in ms (sessions pane).
    function _agoText(ts) {
        if (!ts) return ""
        var m = Math.floor((Date.now() - ts) / 60000)
        if (m < 2) return "just now"
        if (m < 60) return m + " min ago"
        var h = Math.floor(m / 60)
        if (h < 48) return h + (h === 1 ? " hour ago" : " hours ago")
        var d = Math.floor(h / 24)
        return d + (d === 1 ? " day ago" : " days ago")
    }
    TextEdit { id: clipboardHelper; visible: false }
    function copyText(t) { copyToClipboard(t) }   // alias used by SoundboardPopover (0.8.22)
    // 10.4 invite codes: the one-line state of a code, and the message to paste to the person invited.
    function inviteSummary(inv) {
        var parts = []
        if (inv.usesAllowed === null || inv.usesAllowed === undefined) parts.push("unlimited uses")
        else parts.push((inv.completed || 0) + " of " + inv.usesAllowed + " used")
        if ((inv.pending || 0) > 0) parts.push(inv.pending + " signing up")
        if (inv.expiresAt === null || inv.expiresAt === undefined) parts.push("never expires")
        else parts.push((inv.expiresAt < Date.now() ? "expired " : "expires ") + Qt.formatDate(new Date(inv.expiresAt), "MMM d"))
        if (!inv.active) parts.push("no longer works")
        return parts.join(" · ")
    }
    // The part people type: our codes are TH-XXXX-XXXX and the sign-in field shows the TH- itself.
    function inviteBlip(code) { var c = String(code || ""); return /^TH-/i.test(c) ? c.substring(3) : c }
    function inviteMessage(inv) {
        var server = root.mInviteServer.length > 0 ? root.mInviteServer : root.myServer()
        var until = (inv.expiresAt === null || inv.expiresAt === undefined) ? "" : (" It's good until " + Qt.formatDate(new Date(inv.expiresAt), "MMMM d") + ".")
        // selfhost (the web's inviteMessage without an install URL): Tin Hat Web on this server, the app optional
        var web = String(login.serverWebUrl() || "")
        if (web.length === 0) web = "https://" + server + "/"
        return "You're invited to " + server + " on Tin Hat.\n"
             + "1. Open " + web + " in your browser (or install the Tin Hat app).\n"
             + "2. Pick \"Create an account\" under the sign-in button.\n"
             + "3. Server: " + server + " — invite code: " + root.inviteBlip(inv.code) + "." + until
    }
    function copyToClipboard(text) {
        clipboardHelper.text = text
        clipboardHelper.selectAll()
        clipboardHelper.copy()
        clipboardHelper.text = ""
    }

    Controls.Menu {
        id: roomMenu
        Controls.MenuItem {
            text: root.ctxRoomFav ? "Unfavourite" : "Favourite"
            // 10.1 (TH-ICON-03): the pack's two-state star, not the static outline "rating" glyph.
            icon.name: root.ctxRoomFav ? "starred-symbolic" : "non-starred-symbolic"
            onTriggered: {
                login.setFavourite(root.ctxRoomId, !root.ctxRoomFav)
                root.showPassiveNotification(root.ctxRoomFav ? "Removed from Favourites" : "Added to Favourites — it moves up the list and into the ★ rail", "short")
            }
        }
        // 10.1 (TH-NAV-03): clear a room's badge without opening it.
        Controls.MenuItem {
            text: "Mark as read"
            icon.name: "checkmark-symbolic"
            visible: root.ctxRoomUnread > 0
            height: visible ? implicitHeight : 0
            onTriggered: nav.markRoomRead(root.ctxRoomId)
        }
        Controls.MenuItem {
            text: "Invite people…"
            icon.name: "list-add-user"
            onTriggered: { inviteField.text = ""; inviteDialog.open() }
        }
        // Per-room notification level. Writes a SERVER push rule, so the choice follows
        // the account everywhere (web included) instead of being a per-install setting.
        Controls.Menu {
            title: "Notifications"
            Controls.MenuItem {
                text: "All messages"
                checkable: true
                checked: root.ctxRoomNotify === "all"
                onTriggered: login.setRoomNotifyMode(root.ctxRoomId, "all")
            }
            Controls.MenuItem {
                text: "Mentions only"
                checkable: true
                checked: root.ctxRoomNotify === "mentions"
                onTriggered: login.setRoomNotifyMode(root.ctxRoomId, "mentions")
            }
            Controls.MenuItem {
                text: "Mute"
                checkable: true
                checked: root.ctxRoomNotify === "mute"
                onTriggered: login.setRoomNotifyMode(root.ctxRoomId, "mute")
            }
        }
        Controls.MenuItem {
            text: room.recordOn ? "Stop recording calls" : "Record calls here (Memories)…"
            icon.name: room.recordOn ? "media-playback-stop-symbolic" : "media-record-symbolic"
            visible: root.ctxRoomId === room.roomId && root.serverMemories   // selfhost: only where the server records
            height: visible ? implicitHeight : 0
            onTriggered: { recordConfirm.forRoom = ""; room.recordOn ? room.setRecordCall(false) : recordConfirm.open() }
        }
        Controls.MenuItem {
            text: "Change photo…"
            icon.name: "insert-image-symbolic"
            // Only for the open room: the picker acts on room.roomId, and offering it on
            // a room you are not in would silently target the wrong one.
            visible: root.ctxRoomId === room.roomId && room.canChangePhoto
            height: visible ? implicitHeight : 0
            onTriggered: roomPhotoDialog.open()
        }
        Controls.MenuItem {
            text: "Remove photo"
            icon.name: "edit-delete-symbolic"
            visible: root.ctxRoomId === room.roomId && room.canChangePhoto
            height: visible ? implicitHeight : 0
            onTriggered: room.applyRoomPhoto("")
        }
        Controls.MenuItem {
            text: "Copy invite link"
            icon.name: "edit-copy-symbolic"
            onTriggered: {
                root.copyToClipboard(root.roomInviteLink(root.ctxRoomId, root.ctxRoomAlias))
                // Say which kind of link it is. A private room's link is not an invite on
                // its own, and finding that out when your friend cannot get in is worse
                // than being told now.
                root.showPassiveNotification(
                    root.ctxRoomPublic
                        ? "Invite link copied — anyone with it can join"
                        : "Link copied — they will still need an invite to this private room",
                    "short")
            }
        }
        Controls.MenuItem {
            text: "Pop out chat"
            icon.name: "window-new"
            // Open (or focus, if already open) a floating chat bubble for this room.
            onTriggered: {
                if (floatingCtl.isOpen(root.ctxRoomId)) floatingChats.focusBubble(root.ctxRoomId)
                else if (!floatingCtl.openBubble(root.ctxRoomId))   // 10.1 (TH-FLOAT-09): the cap used to no-op silently
                    root.showPassiveNotification(LinkGate.plain("You can have " + floatingCtl.maxBubbles + " popped-out chats at once — close one first"), "short")
            }
        }
        Controls.MenuSeparator {}
        Controls.MenuItem {
            text: "Leave room"
            icon.name: "go-previous-symbolic"
            onTriggered: leaveConfirm.open()
        }
    }
    ConfirmDialog {
        id: recordConfirm
        // 11.0: the room it records ("" = the open room): the call status line's Rec asks for the CALL's room
        property string forRoom: ""
        app: root
        dialogTitle: "Record calls in this room?"
        confirmText: "Start recording"
        destructive: true
        message: "Calls in this room get recorded for the Memories tab — every stream and voice separately, so replays can switch points of view.\n\nNo extra participant appears: everyone in the call sees a 🔴 REC cue in their call header instead (and anyone who turned on \"Never record me\" in Settings → Calls is left out of the recording entirely — their cue says so). Stop any time with the ⏹ Rec button."
        onConfirmed: {
            var fr = recordConfirm.forRoom
            recordConfirm.forRoom = ""
            if (fr.length > 0 && fr !== room.roomId) room.setRecordCallIn(fr, true)
            else room.setRecordCall(true)
        }
        onCancelled: recordConfirm.forRoom = ""
    }
    ConfirmDialog {
        id: leaveConfirm
        app: root
        dialogTitle: "Leave room?"
        confirmText: "Leave"
        destructive: true
        message: "Leave \"" + root.ctxRoomName + "\"? You'll need a new invite to rejoin if it's private."
        onConfirmed: {
            login.leaveRoom(root.ctxRoomId)
            if (room.roomId === root.ctxRoomId) { room.setTyping(false); root.roomShown = false }
        }
    }

    // Push-to-talk hold key. Works while Tin Hat has focus; capturing it while
    // you are in a game needs the desktop's global-shortcut portal, which is a separate
    // piece of work — so the setting says so rather than implying otherwise.
    Item {
        anchors.fill: parent
        focus: true
        z: -1000
        // 11.0: the key is push to talk's (Ctrl unless Settings → Shortcuts moved it: a modifier alone or an F-key)
        Keys.onPressed: (e) => {
            if (actionsHost.isPttKey(e.key) && !e.isAutoRepeat) callctl.pttHold(true)
        }
        Keys.onReleased: (e) => {
            if (actionsHost.isPttKey(e.key) && !e.isAutoRepeat) callctl.pttHold(false)
        }
    }

    // Open whatever createDm/createRoom just produced (or re-found). The controller
    // publishes the id once the room exists; clearing it after use means opening the
    // same room twice in a row still works.
    Connections {
        target: login
        function onCreatedRoomChanged() {
            var id = login.createdRoom
            if (!id || id.length === 0) return
            newDmDialog.close()
            newRoomDialog.close()
            root.openRoomId(id)
            login.createdRoom = ""
        }
    }

    // Kirigami.PromptDialog, NOT Kirigami.Dialog: this stack has a documented history of
    // Kirigami.Dialog collapsing empty (see the share-picker comment), and PromptDialog is
    // the pattern already proven to render here (musicUrlDialog).
    Kirigami.PromptDialog {
        id: newDmDialog
        title: "New DM"
        standardButtons: Kirigami.Dialog.NoButton
        // 2026-09-29: a plain username is enough ("alex" → @alex:<our server>; login.dmTarget = dm_target.rs,
        // the web's dmTarget.ts, shared vectors). "" = not a user.
        readonly property string target: login.dmTarget(newDmField.text)
        onOpened: newDmField.forceActiveFocus()   // type right away (the web's autoFocus)
        customFooterActions: [
            Kirigami.Action {
                text: "Message"
                icon.name: "document-edit-symbolic"
                enabled: newDmDialog.target.length > 0
                onTriggered: login.createDm(newDmDialog.target)
            }
        ]
        ColumnLayout {
            spacing: 8
            Controls.Label {
                text: "Who do you want to message?"
                Layout.fillWidth: true
            }
            Controls.TextField {
                id: newDmField
                implicitWidth: Kirigami.Units.gridUnit * 20
                Layout.fillWidth: true
                placeholderText: "Their username, e.g. alex"
                inputMethodHints: Qt.ImhNoAutoUppercase | Qt.ImhNoPredictiveText
                onAccepted: if (newDmDialog.target.length > 0) login.createDm(newDmDialog.target)
            }
            Controls.Label {
                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                text: newDmField.text.trim().length === 0
                      ? "Just their username is enough. Someone on another server: @name:their.server."
                      : (newDmDialog.target.length > 0 ? "Messages " + newDmDialog.target
                                                        : "That isn't a username — letters, numbers and . _ - only.")
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
                color: Th.dialogHintFg
                font.pointSize: Kirigami.Theme.smallFont.pointSize
            }
            Controls.Label {
                text: "If you already have a DM with them, it opens that one instead of starting a second."
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
                color: Th.dialogHintFg
                font.pointSize: Kirigami.Theme.smallFont.pointSize
            }
        }
    }

    ConfirmDialog {
        id: joinConfirm
        app: root
        dialogTitle: "Join " + root.joinRoomName + "?"
        confirmText: "Join"
        message: "You'll show up in the member list, and everyone already in there will see you join."
        onConfirmed: {
            login.joinRoom(root.joinRoomId)
            root.openRoomId(root.joinRoomId)
        }
    }

    // Invite to whichever room was right-clicked (ctxRoomId), which may be a space.
    Kirigami.PromptDialog {
        id: inviteDialog
        title: LinkGate.plain("Invite to " + root.ctxRoomName)
        standardButtons: Kirigami.Dialog.NoButton
        customFooterActions: [
            Kirigami.Action {
                text: "Invite"
                icon.name: "list-add-user"
                enabled: inviteField.text.trim().length > 0
                onTriggered: {
                    login.inviteToRoom(root.ctxRoomId, inviteField.text.trim())
                    inviteDialog.close()
                }
            }
        ]
        ColumnLayout {
            spacing: 8
            Controls.TextField {
                id: inviteField
                implicitWidth: Kirigami.Units.gridUnit * 20
                Layout.fillWidth: true
                placeholderText: "@someone:" + (root.myServer().length > 0 ? root.myServer() : "example.com")
                onAccepted: if (text.trim().length > 0) {
                    login.inviteToRoom(root.ctxRoomId, text.trim())
                    inviteDialog.close()
                }
            }
            Controls.Label {
                text: "They'll get an invite they can accept or decline. Remove someone again from the member list on the right."
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
                color: Th.dialogHintFg
                font.pointSize: Kirigami.Theme.smallFont.pointSize
            }
        }
    }

    Kirigami.PromptDialog {
        id: newRoomDialog
        title: LinkGate.plain(newRoomIsSpace.checked ? "New space"
             : (newRoomInSpace.checked && newRoomDialog.spaceName.length > 0
                ? "New room in " + newRoomDialog.spaceName : "New room"))
        // The space selected in the nav when the dialog opened. Captured on open rather
        // than bound live, so switching spaces behind a half-filled dialog cannot move
        // the room somewhere the user did not choose.
        property string spaceId: ""
        property string spaceName: ""
        standardButtons: Kirigami.Dialog.NoButton
        customFooterActions: [
            Kirigami.Action {
                text: "Create"
                icon.name: "folder-new-symbolic"
                enabled: newRoomName.text.trim().length > 0
                onTriggered: login.createRoom(
                    newRoomName.text.trim(), newRoomTopic.text.trim(),
                    newRoomPrivate.checked, newRoomIsSpace.checked,
                    // A space cannot nest inside another here, and the checkbox is the
                    // opt-out. "" means a top-level room, as before.
                    (!newRoomIsSpace.checked && newRoomInSpace.checked)
                        ? newRoomDialog.spaceId : "")
            }
        ]
        ColumnLayout {
            spacing: 8
            Controls.TextField {
                id: newRoomName
                implicitWidth: Kirigami.Units.gridUnit * 20
                Layout.fillWidth: true
                placeholderText: newRoomIsSpace.checked ? "Space name" : "Room name"
            }
            Controls.TextField {
                id: newRoomTopic
                Layout.fillWidth: true
                placeholderText: "What's it about? (optional)"
                visible: !newRoomIsSpace.checked
            }
            Controls.CheckBox {
                id: newRoomPrivate
                text: "Private — people need an invite"
                checked: true
            }
            Controls.CheckBox {
                id: newRoomInSpace
                visible: !newRoomIsSpace.checked && newRoomDialog.spaceId.length > 0
                text: LinkGate.plain("Put it in " + newRoomDialog.spaceName)
                checked: true
            }
            Controls.Label {
                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                // only a PUBLIC room in the space is open to its members; a private one still needs an invite
                visible: newRoomInSpace.visible && newRoomInSpace.checked && !newRoomPrivate.checked
                text: "Everyone in " + newRoomDialog.spaceName + " can open this room without waiting for an invite."
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
                color: Th.dialogHintFg
                font.pointSize: Kirigami.Theme.smallFont.pointSize
            }
            Controls.Label {
                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                // 2026-10-23: encrypted or not, in the web's words (login.newRoomHint = new_room.rs hint, shared
                // vectors). The parent is exactly what Create hands createRoom.
                id: newRoomEncHint
                text: login.newRoomHint(newRoomPrivate.checked, newRoomIsSpace.checked,
                                        (!newRoomIsSpace.checked && newRoomInSpace.checked) ? newRoomDialog.spaceId : "")
                visible: text.length > 0
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
                color: Th.dialogHintFg
                font.pointSize: Kirigami.Theme.smallFont.pointSize
            }
            Controls.CheckBox {
                id: newRoomIsSpace
                text: "Make this a space (a group of rooms)"
                checked: false
            }
            Controls.Label {
                text: "Calls, the Media Party and the soundboard are set up for everyone here automatically."
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
                color: Th.dialogHintFg
                font.pointSize: Kirigami.Theme.smallFont.pointSize
            }
        }
    }

    // The "+" at the foot of the spaces rail. Discord's shape: one button that covers
    // both "someone sent me a link" and "I want to start something", because a new user
    // does not yet know which of those they need.
    Kirigami.PromptDialog {
        id: joinOrCreateDialog
        title: "Join a space"
        standardButtons: Kirigami.Dialog.NoButton
        onOpened: { joinLinkField.text = ""; joinLinkField.forceActiveFocus() }
        customFooterActions: [
            Kirigami.Action {
                text: "Make a space instead"
                icon.name: "folder-new-symbolic"
                onTriggered: {
                    joinOrCreateDialog.close()
                    newRoomName.text = ""
                    newRoomTopic.text = ""
                    newRoomIsSpace.checked = true      // this button is for SPACES
                    newRoomPrivate.checked = true
                    newRoomDialog.spaceId = ""
                    newRoomDialog.spaceName = ""
                    newRoomDialog.open()
                }
            },
            Kirigami.Action {
                text: "Join"
                icon.name: "go-next-symbolic"
                enabled: joinLinkField.text.trim().length > 0
                onTriggered: {
                    login.joinByLink(joinLinkField.text.trim())
                    joinOrCreateDialog.close()
                }
            }
        ]
        ColumnLayout {
            spacing: 8
            Controls.TextField {
                id: joinLinkField
                implicitWidth: Kirigami.Units.gridUnit * 22
                Layout.fillWidth: true
                placeholderText: "Paste an invite link"
                onAccepted: if (text.trim().length > 0) {
                    login.joinByLink(text.trim())
                    joinOrCreateDialog.close()
                }
            }
            Controls.Label {
                text: "Anything works: an invite link, a matrix.to link, or an address like #general:yourserver.tinhat.me."
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
                color: Th.dialogHintFg
                font.pointSize: Kirigami.Theme.smallFont.pointSize
            }
            Controls.Label {
                text: "Haven't got a link? Make your own space and invite people to it."
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
                color: Th.dialogHintFg
                font.pointSize: Kirigami.Theme.smallFont.pointSize
            }
        }
    }

    // ---- App menu (the ☰ in the account island). 2026-09-29: just Settings, Edit layout, Theme, About and Sign
    // out (contract actions.json menus.app; Tin Hat Web draws the same). Where the rest went: New DM / New room → the
    // room column's + (newMenu below); the Game Reel → the room header; Add widget → Edit layout's toolbar; the corner
    // dock → Settings › Appearance; updates, the tour and shortcuts → Settings › About & help; Verify → the verify
    // banner + Settings › Devices & security; Refresh rooms is gone (sync is live). ----
    Controls.Menu {
        id: appMenu
        modal: false   // 10.1 (TH-SHELL-18): every other menu is non-modal; this one dimmed the whole window
        Controls.MenuItem {
            text: "Settings…"
            icon.name: "settings-configure"
            onTriggered: settingsSheet.open()   // settingsSheet.onOpened refreshes
        }
        Controls.MenuItem {   // 11.0 edit mode (EDITMODE-UI §1): the layout.edit action
            text: "Edit layout…"
            icon.name: "window-symbolic"
            onTriggered: root.openEditLayout(false)
        }
        // THEMES-UI §12: your 5 most recent themes on this device — the one on screen checked; before any pick: it, OLED
        // and Goose (theme_store.rs recent_rows, tinhat_theme::recent) — each with its swatch, then "More themes…" =
        // Settings → Appearance at the gallery and "Edit current theme…" = the theme editor on the theme on screen
        // (Appearance's Customize…; amendment 2026-09-28). A row applies its theme like a gallery card (core.chooseTheme:
        // the theme store, the account or this device; outside the settings, so a sound offer comes as a toast).
        Controls.Menu {
            id: themeMenu
            title: "Theme"
            icon.name: "preferences-desktop-color"
            readonly property var rows: {
                var c = core.c_themeCtl
                var _rev = c ? c.stateRev : 0
                if (!c || c.stateJson.length === 0) return []
                try { var s = JSON.parse(c.stateJson); return Array.isArray(s.recent) ? s.recent : [] } catch (e) { return [] }
            }
            // sized to its rows like the web's menus (D2: the content, the style's own minimum) — Basic's fixed 200 px
            // cut theme names short; the Breeze style computes the same width itself
            implicitWidth: {
                var w = 0
                for (var i = 0; i < themeMenu.count; i++) {
                    var it = themeMenu.itemAt(i)
                    if (it) w = Math.max(w, it.implicitWidth)
                }
                return Math.max(implicitBackgroundWidth + leftInset + rightInset, w + leftPadding + rightPadding)
            }
            function pick(id, current) {
                if (current) return   // the checked row: nothing to do (THEMES-UI §3.5)
                // applied once the menu has closed: a pick reorders the recents, so the Instantiator below rebuilds these
                // rows — a row removed while its `triggered` is still being delivered never reaches the Menu's own
                // handler, and the whole ☰ menu stayed open after the switch (the web's menus close on the click; S7
                // gate A, 2026-09-27: live menu switches vs menuref)
                Qt.callLater(function () {
                    var err = core.chooseTheme(id, false)
                    if (err && err.length > 0) root.showPassiveNotification(LinkGate.plain(err), 6000)
                })
            }
            Instantiator {
                model: themeMenu.rows
                delegate: CheckMenuItem {   // 10.1 (TH-SHELL-17): the last "✓  "-prefix idiom (labels shifted on selection)
                    id: themeRow
                    required property var modelData
                    text: themeRow.modelData.name
                    isChecked: () => themeRow.modelData.current === true
                    rightPadding: themeRow.padding + 36   // the swatch (30) and a 6 px gap at the row's right end
                    onTriggered: themeMenu.pick(themeRow.modelData.id, themeRow.modelData.current === true)
                    ThemeSwatch {
                        anchors.right: parent.right
                        anchors.rightMargin: themeRow.padding
                        anchors.verticalCenter: parent.verticalCenter
                        colours: themeRow.modelData.swatch
                    }
                }
                onObjectAdded: (i, o) => themeMenu.insertItem(i, o)
                onObjectRemoved: (i, o) => themeMenu.removeItem(o)
            }
            Controls.MenuSeparator {}
            Controls.MenuItem {
                text: "More themes…"
                icon.name: "view-grid-symbolic"
                onTriggered: root.openThemeGallery()
            }
            // THEMES-UI §12 amendment 2026-09-28: the theme editor on the theme on screen — Appearance's own
            // Customize… (one of My themes → Edit…, any other → New theme from this, §13.1); the last row, as on the web
            Controls.MenuItem {
                text: "Edit current theme…"
                icon.name: "document-edit-symbolic"
                onTriggered: root.editCurrentTheme()
            }
        }
        Controls.MenuSeparator {}
        Controls.MenuItem {
            text: "Icon gallery"
            // 10.1 (TH-SHELL-18): a pack-authoring tool — only with --tinhat-dev.
            visible: Qt.application.arguments.indexOf("--tinhat-dev") >= 0
            height: visible ? implicitHeight : 0
            icon.name: "preferences-desktop-color"
            onTriggered: iconGallery.openOrRaise()   // 0.8.26: review an icon pack in the real app
        }
        Controls.MenuItem {
            text: "About Tin Hat"
            icon.name: "help-about-symbolic"
            onTriggered: actionsHost.run("app.about")   // the app.about action: Settings › About & help
        }
        Controls.MenuItem {
            text: "Sign out"
            icon.name: "system-log-out-symbolic"
            onTriggered: logoutConfirm.open()
        }
    }
    // ---- The room column's + (2026-09-29, contract menus.new): New DM, and New room only while a space is
    // selected. Tin Hat Web: nav/menus.tsx NewMenu. ----
    Controls.Menu {
        id: newMenu
        modal: false
        Controls.MenuItem {
            text: "New DM…"
            icon.name: "document-edit-symbolic"
            onTriggered: { newDmField.text = ""; newDmDialog.open() }
        }
        Controls.MenuItem {
            text: "New room…"
            icon.name: "folder-new-symbolic"
            // only while a space is selected: the room is made in that space (2026-09-29)
            visible: root.mNavActiveSpace.indexOf("!") === 0
            height: visible ? implicitHeight : 0
            onTriggered: {
                newRoomName.text = ""
                newRoomTopic.text = ""
                newRoomPrivate.checked = true
                newRoomIsSpace.checked = false
                newRoomDialog.spaceId = root.mNavActiveSpace.indexOf("!") === 0 ? root.mNavActiveSpace : ""
                newRoomDialog.spaceName = root.mNavActiveLabel
                newRoomInSpace.checked = true
                newRoomDialog.open()
            }
        }
    }
    ConfirmDialog {
        id: logoutConfirm
        app: root
        dialogTitle: "Sign out?"
        // 10.1 (TH-FIRST-04): tell the truth. Signing out deletes this device's crypto store, which is
        // only safe when the room keys are in a server-side backup (login.backupReady).
        confirmText: login.backupReady ? "Sign out" : "Sign out anyway"
        destructive: true
        message: login.backupReady
            ? "Sign out of " + root.mUserId + "? Your encryption keys are backed up on the server, so your encrypted messages will still be readable when you sign back in."
            : "Sign out of " + root.mUserId + "? This account has no key backup. Signing out deletes this device's encryption keys, so the encrypted messages you can read now will become unreadable — here and on any new device. To keep them, set up recovery first (Settings → Security)."
        onConfirmed: login.logout()
    }
    // 10.1 (TH-SHELL-04): the title-bar ✕ during a call is a hang-up for everyone — ask first.
    property bool _quitConfirmed: false
    ConfirmDialog {
        id: quitConfirm
        app: root
        dialogTitle: "Leave the call and quit?"
        message: "You're in a call. Quitting Tin Hat hangs up and stops anything you're sharing."
        confirmText: "Quit"
        destructive: true
        onConfirmed: { root._quitConfirmed = true; root.close() }
    }
    // (10.1 TH-SET-17: the two destructive Settings buttons' confirms live in SettingsWindow.qml now)

    // First-run log-sharing consent (a tester's request: friends should be TOLD logs
    // are collected, and be able to opt out). Shown once after first sign-in when the
    // consent flag is unset (settings.logshipState() === -1). "Keep on" = accept,
    // "Turn off" = decline; either way the flag is now set so this never re-shows.
    ConfirmDialog {
        id: logshipConsent
        app: root
        dialogTitle: "Help improve Tin Hat?"
        confirmText: "Keep it on"
        cancelText: "Turn it off"
        message: "When something breaks, this app can automatically send its debug log to your server's owner so the problem " +
                 "can be fixed. Tokens, encryption keys, and IP addresses are stripped out first, and your messages " +
                 "are never logged. It's on by default — you can turn it off now or any time in Settings → Privacy."
        onConfirmed: { settings.applyLogship(true); root.mLogship = 1 }
        onCancelled: { settings.applyLogship(false); root.mLogship = 0 }
    }

    // ---- About → Settings → About & help (SettingsWindow.qml; `aboutSheet` below the Settings window is its old name) ----

    // ---- First-run onboarding tour (Feature #9) ----
    // Auto-shows ONCE after login (never over the login screen, never during a call,
    // never again after Done/Skip — gated by login.savedTourSeen, persisted exactly
    // like the theme/nav-mode prefs). Re-openable from ☰ → "Help & tour".
    Tour {
        id: welcomeTour
        app: root
        // Done / Skip / ESC -> persist "seen" so it doesn't auto-show again.
        onFinished: login.setTourSeen(true)
    }
    // Has the auto-show been considered this session? Prevents re-triggering on every
    // mLoggedIn flap and lets a sign-out -> sign-in re-evaluate cleanly.
    property bool _tourAutoConsidered: false
    function _maybeAutoShowTour() {
        if (root._tourAutoConsidered) return
        if (!root.mLoggedIn) return            // never over the login screen
        if (login.savedTourSeen) { root._tourAutoConsidered = true; return } // already seen
        if (root.callShown) return             // never pop during / over a live call
        // A pending device-verification prompt takes precedence — defer the tour until
        // it's resolved (verified, or the user closed the verify dialog).
        if (!root.mVerified && verifySheet.visible) return
        // F1/F2: a new account's first device — its Security Key dialog comes first
        if (login.firstDevicePending || (login.firstDevice && verifySheet.visible)) return
        if (logshipConsent.visible) return   // 10.1 (TH-FIRST-24): one popup at a time
        root._tourAutoConsidered = true
        welcomeTour.start()
    }

    // ---- Release-notes sheet (from the update banner's "What's new") ----
    Kirigami.OverlaySheet {
        id: updateNotesSheet
        parent: Controls.Overlay.overlay
        header: Kirigami.Heading { textFormat: Text.PlainText; text: "What's new in Tin Hat " + (root.mLatestVersion.length > 0 ? root.mLatestVersion : root.mAppVersion) }
        Controls.Label {
            textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
            Layout.preferredWidth: Kirigami.Units.gridUnit * 22
            wrapMode: Text.Wrap
            color: Th.sheetFg
            text: root.mReleaseNotes.length > 0 ? root.mReleaseNotes : "No release notes provided."
        }
    }

    // ---- Attachment upload: file picker + drag-and-drop ----
    FileDialog {
        id: attachDialog
        title: "Send a file or image"
        onAccepted: if (room.roomId.length > 0) room.sendAttachment(selectedFile)
    }
    // In-app bug report (room-list 🐞): note + LIVE session-log tail ship immediately —
    // no restart. The click is the consent (works with the passive log toggle off).
    // Component file for readability. (The old claim that an inline Controls.Dialog here
    // deadlocks QML load was a misdiagnosis of the 0.3.5 no-boot — see ThNav.qml.)
    // 10.1 (TH-SHELL-20): every binding in one place.
    Kirigami.PromptDialog {
        id: shortcutsDialog
        title: "Keyboard shortcuts"
        standardButtons: Kirigami.Dialog.Close
        GridLayout {
            columns: 2
            columnSpacing: Kirigami.Units.largeSpacing
            rowSpacing: Kirigami.Units.smallSpacing
            // 11.0 (ACTIONS.md §8): a row whose action got keys of its own on this device shows them (actionsHost.helpKey);
            // everything else reads exactly as before (the web's dialogs.tsx shortcutRows does the same)
            Controls.Label { textFormat: Text.PlainText; text: actionsHost.helpKey("nav.quickSwitcher.open", "Ctrl+K"); font.bold: true }            Controls.Label { text: "Jump to any room" }
            Controls.Label { textFormat: Text.PlainText; text: actionsHost.helpKey("app.settings.open", "Ctrl+,"); font.bold: true }            Controls.Label { text: "Settings" }
            Controls.Label {
                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                text: (actionsHost.changedActions.indexOf("nav.room.previous") >= 0 || actionsHost.changedActions.indexOf("nav.room.next") >= 0)
                      ? actionsHost.helpKey("nav.room.previous", "Alt+Up") + " / " + actionsHost.helpKey("nav.room.next", "Alt+Down") : "Alt+↑ / Alt+↓"
                font.bold: true
            }
            Controls.Label { text: "Previous / next room  (+Shift: unread only)" }
            Controls.Label { textFormat: Text.PlainText; text: actionsHost.helpKey("call.mic.toggle", "Ctrl+Shift+M"); font.bold: true }      Controls.Label { text: "Mute / unmute your mic" }
            Controls.Label { textFormat: Text.PlainText; text: actionsHost.helpKey("call.deafen.toggle", "Ctrl+Shift+D"); font.bold: true }      Controls.Label { text: "Deafen / undeafen" }
            Controls.Label { textFormat: Text.PlainText; text: actionsHost.helpKey("call.pushToTalk.hold", "Ctrl") + " (hold)"; font.bold: true }       Controls.Label { text: "Push to talk, when enabled" }
            Controls.Label { textFormat: Text.PlainText; text: "Esc / " + actionsHost.helpKey("call.fill.toggle", "F11"); font.bold: true }         Controls.Label { text: "Leave / toggle \"fill the app\" in a call" }
            Controls.Label { textFormat: Text.PlainText; text: actionsHost.helpKey("soundboard.open", "Ctrl+Shift+S"); font.bold: true }      Controls.Label { text: "Soundboard" }
            Controls.Label { textFormat: Text.PlainText; text: actionsHost.helpKey("calendar.open", "Ctrl+Shift+E"); font.bold: true }      Controls.Label { text: "Calendar" }
            Controls.Label { text: "PgUp / PgDn / Home / End"; font.bold: true }  Controls.Label { text: "Scroll the conversation" }
            Controls.Label { text: "Esc"; font.bold: true }               Controls.Label { text: "Cancel a reply or edit; jump to the newest message" }
            Controls.Label { text: "Ctrl+V"; font.bold: true }            Controls.Label { text: "Paste an image or file into the composer" }
            Controls.Label { text: "↑ (empty composer)"; font.bold: true } Controls.Label { text: "Edit your last message" }
        }
    }
    BugReportDialog {
        id: bugReportDialog
        app: root
        onSubmitted: (text) => settings.submitBugReport(text)
    }
    Connections {
        target: settings
        function onBugReportStatusChanged() {
            if (settings.bugReportStatus.indexOf("Report failed") === 0) bugReportDialog.keepNote = true
            else if (settings.bugReportStatus.indexOf("Report sent") === 0) { bugReportDialog.keepNote = false; bugReportDialog.clearForm() }
        }
    }
    // #7 Music: pick a local audio file to stream into the active call (selectedFile is a
    // file:// URL — gst's uridecodebin decodes it). Opened from the device menu's Music ▸.
    FileDialog {
        id: musicFileDialog
        title: "Play music in call"
        nameFilters: ["Audio files (*.mp3 *.flac *.ogg *.opus *.wav *.m4a *.aac)", "All files (*)"]
        onAccepted: callctl.startMusic(selectedFile)
    }
    // #7 Music: paste an http(s) stream/track URL to play into the active call.
    Kirigami.PromptDialog {
        id: musicUrlDialog
        title: "Play music from URL"
        standardButtons: Kirigami.Dialog.NoButton
        customFooterActions: [
            Kirigami.Action {
                text: "Play"
                icon.name: "media-playback-start-symbolic"
                enabled: musicUrlField.text.trim().length > 0
                onTriggered: { callctl.startMusic(musicUrlField.text.trim()); musicUrlDialog.close() }
            }
        ]
        Controls.TextField {
            id: musicUrlField
            implicitWidth: Kirigami.Units.gridUnit * 20
            placeholderText: "https://…  (stream or audio file URL)"
            onAccepted: if (text.trim().length > 0) { callctl.startMusic(text.trim()); musicUrlDialog.close() }
        }
    }
    // Windows screen-share source picker: monitors + windows from callctl.shareSources().
    // Never opens on Linux (sources are [] there — the KDE portal shows its own picker).
    // A Controls.Menu with a Repeater — the exact pattern the Share-audio-output menu
    // uses (proven to render on this stack; a Kirigami.Dialog+Repeater collapsed empty).
    Controls.Menu {
        id: sharePicker
        property var sources: []
        MenuSectionLabel { app: root; text: "Share your screen" }
        Repeater {
            model: sharePicker.sources
            delegate: Controls.MenuItem {
                required property var modelData
                text: LinkGate.plain(modelData.label)
                icon.name: modelData.kind === "monitor"
                    ? "video-display-brightness-symbolic" : "window-duplicate-symbolic"
                onTriggered: callctl.shareScreenSource(modelData.kind, modelData.handle)
            }
        }
    }
    // Drop files anywhere on an open conversation to upload them. 0.8.21: ONE
    // DropArea for the whole window — it sits above the page stack, so the timeline's
    // own DropArea never saw a drag and its "Drop to send" feedback never showed. The
    // feedback lives here now (naming the room) and every step logs as [dnd].
    DropArea {
        id: fileDrop
        anchors.fill: parent
        enabled: root.roomShown && room.roomId.length > 0 && root.conversationView !== null
        // 10.0.1: files only. Qt 6 hands a drag to EVERY DropArea under the pointer, so the
        // Spaces rail's reorder drag (an internal Drag with no mime data) lit this "Drop to
        // send" overlay too. `keys` filters on the drag's mime types before onEntered runs.
        keys: ["text/uri-list"]
        onEntered: (drag) => {
            console.log("[dnd] entered hasUrls=" + drag.hasUrls + " formats=" + drag.formats)
            if (drag.hasUrls) drag.accept(); else drag.accepted = false
        }
        onDropped: (drop) => {
            console.log("[dnd] dropped hasUrls=" + drop.hasUrls + " n=" + (drop.hasUrls ? drop.urls.length : 0))
            if (drop.hasUrls) {
                // 10.1 (TH-CONV-01): a drop STAGES. The 9.3 attachment strip (thumbnail, ✕ per
                // file, a message before it goes) is the one place files leave from — nothing is
                // uploaded on release any more. Same behaviour as a floating bubble.
                if (root.conversationView) {
                    for (var i = 0; i < drop.urls.length; i++) {
                        console.log("[dnd] stage " + drop.urls[i])
                        root.conversationView.stageFile(String(drop.urls[i]))
                    }
                    root.conversationView.focusComposer()
                }
                drop.accept()
            }
        }
        Rectangle {
            anchors.fill: parent
            visible: fileDrop.containsDrag
            color: Th.dropOverlayBg
            Rectangle {
                anchors.centerIn: parent
                width: Math.min(parent.width - 80, 520)
                height: 150
                radius: 16
                color: Th.dropCardBg
                border.width: 2
                border.color: Th.dropCardBorder
                ColumnLayout {
                    anchors.centerIn: parent
                    spacing: 6
                    Kirigami.Icon {
                        Layout.alignment: Qt.AlignHCenter
                        source: "document-send-symbolic"
                        color: Th.dropCardIcon
                        implicitWidth: 40
                        implicitHeight: 40
                    }
                    Controls.Label {
                        textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                        Layout.alignment: Qt.AlignHCenter
                        text: "Drop to attach in " + (root.mRoomName.length > 0 ? root.mRoomName : "this room")
                        font.bold: true
                        font.pointSize: 14
                        color: Th.dropCardFg
                    }
                    Controls.Label {
                        Layout.alignment: Qt.AlignHCenter
                        text: "You can add a message before it goes."
                        color: Th.dropCardHintFg
                    }
                }
            }
        }
    }
    // 10.1 (TH-SET-18): Settings has a shortcut, like every app this replaces.
    // 11.0 (ACTIONS.md §8): every default key below is `actionsHost.keysFor(<its action>)` — Ctrl+, … while this device
    // keeps it for that action (Settings → Shortcuts may move it or give it to another action)
    Shortcut { sequences: actionsHost.keysFor("app.settings.open"); context: Qt.ApplicationShortcut; enabled: root.mLoggedIn; onActivated: settingsWindow.open() }
    // 10.1 (TH-CALL-16): Escape leaves "fill the app"; F11 toggles it.
    Shortcut {
        sequence: "Escape"
        enabled: root.callMaximized && root.callShown && !callOptions.opened && !streamPopover.opened && !soundboardPanel.opened && !settingsWindow.visible
        onActivated: root.callMaximized = false
    }
    Shortcut { sequences: actionsHost.keysFor("call.fill.toggle"); enabled: root.callShown && root.mLoggedIn; onActivated: root.callMaximized = !root.callMaximized }
    // Ctrl+K: the palette over rooms, people and actions (ACTIONS.md §10; QuickSwitcher.qml).
    QuickSwitcher {
        id: quickSwitcher
        app: root
        rooms: root.mRooms
        onChosen: (roomId) => root.openRoomId(roomId)
        onInviteChosen: (roomId) => root.showInvite(roomId)   // a pending invite: where it waits (ACTIONS.md §14.3)
    }
    // 11.0 step 3 (buttons): what every action button, the palette and (later) a key run through. `ui` = the surfaces
    // the id → command map opens (ActionsHost.exec); an island that moves one of them re-points it here.
    ActionsHost {
        id: actionsHost
        app: root
        ui: ({
            aboutSheet: aboutSheet, appMenu: appMenu, bugReportDialog: bugReportDialog, calendarWindow: calendarWindow,
            callIsland: callIsland, callOptions: callOptions, chooseTheme: function (id) { return core.chooseTheme(id) },
            conversation: conversation, devicesMenu: devicesMenu, floatingChats: floatingChats, gameReelWindow: gameReelWindow,
            iconGallery: iconGallery, installDialog: installDialog, inviteDialog: inviteDialog, inviteField: inviteField,
            joinOrCreateDialog: joinOrCreateDialog, memoriesWindow: memoriesWindow, musicFileDialog: musicFileDialog,
            musicUrlDialog: musicUrlDialog, musicUrlField: musicUrlField, navState: navState, newDmDialog: newDmDialog,
            newDmField: newDmField, newRoomDialog: newRoomDialog, newRoomInSpace: newRoomInSpace, newRoomIsSpace: newRoomIsSpace,
            newRoomName: newRoomName, newRoomPrivate: newRoomPrivate, newRoomTopic: newRoomTopic, noiseTestSheet: noiseTestSheet,
            quickSwitcher: quickSwitcher, rightPanel: rightPanel, roomPhotoDialog: roomPhotoDialog, roomsIsl: roomsIsl,
            settingsSheet: settingsWindow, shortcutsDialog: shortcutsDialog, soundboardPanel: soundboardPanel,
            streamPopover: streamPopover, themeCtl: core.c_themeCtl, verifySheet: verifySheet, watchPartyWindow: watchPartyWindow,
            welcomeTour: welcomeTour
        })
    }

    // ---- 11.0 step 5: the theme editor (THEMES-UI.md §13: non-modal, docked right, over the islands and under menus,
    // popovers and dialogs; its Pick overlay under it) and a share card's Try bar (§14.4)
    ThemeEditorPanel {
        id: themeEditor
        z: 900
        app: root
        ctl: core.c_editorCtl
        host: QtObject {
            readonly property Item pickRoot: root.contentItem
            function notify(text, timeout) { root.showPassiveNotification(LinkGate.plain(text), timeout) }
            function shareDoc(docJson, name) { root.shareThemeToRoom(docJson, name) }
            function realColours() { return core._thColours }
        }
    }
    ThemeTryBar {
        z: 901
        app: root
        trial: core.themeTrial
        onKeep: if (core.themeTrial) core.keepTheme(core.themeTrial.docJson)
        onUndo: core.endTrial(true)
    }
    Shortcut {
        sequences: actionsHost.keysFor("nav.quickSwitcher.open")
        context: Qt.ApplicationShortcut   // 10.1 (TH-A11Y-10): from any Tin Hat window, like mute/deafen
        onActivated: if (root.mLoggedIn) quickSwitcher.open()
    }
    // In-call keyboard mute/deafen (same invokables as the call-bar buttons). The
    // mInCall guard is load-bearing: toggling the mic mirror outside a call would
    // leave a stale mute state with no call to apply it to.
    Shortcut {
        sequences: actionsHost.keysFor("call.mic.toggle")
        context: Qt.ApplicationShortcut
        onActivated: {
            if (!root.mInCall) return
            if (root.mServerMuted) root.showPassiveNotification(LinkGate.plain(root.serverMutedText()), 3000)   // RBAC §7.4: locked
            else callctl.toggleMic()
        }
    }
    // RBAC §7.4: the one sentence every locked-mic surface repeats.
    function serverMutedText() { return (root.mServerMutedBy.length > 0 ? root.mServerMutedBy : "A moderator") + " muted your mic for everyone" }
    Shortcut {
        sequences: actionsHost.keysFor("call.deafen.toggle")
        context: Qt.ApplicationShortcut
        onActivated: if (root.mInCall) callctl.applyDeafen(!root.mDeafened)
    }

    // Soundboard (room + personal sounds, web search). Opened from the ☰ app menu for
    // the currently-open room. Tapping a sound plays it LOCALLY (self-audible, always) via the
    // panel's own QtMultimedia MediaPlayer, and ALSO publishes a 2nd audio track into the active
    // call (playSound) when in one so peers hear it. Tapping the playing tile stops both (stopSound
    // unpublishes the call track; the panel stops its local player).
    // 0.8.22: the stream popover (share source · quality · per-app audio · recording-only ·
    // live preview) — one surface for everything "share my screen".
    StreamPopover { id: streamPopover; app: root; ctl: callctl }
    // 0.8.33: the call bar chevrons' slide-out settings + the echo canceller's speaker-output picker.
    CallOptionsPopover { id: callOptions; app: root; ctl: callctl }
    EcOutputsPopup { id: ecOutputsPopup; app: root; ctl: callctl }
    Shortcut { sequences: actionsHost.keysFor("soundboard.open"); context: Qt.ApplicationShortcut; enabled: root.serverSoundboard; onActivated: { root.soundboardRefresh(); soundboardPanel.openOrRaise() } }
    Shortcut { sequences: actionsHost.keysFor("calendar.open"); context: Qt.ApplicationShortcut; onActivated: calendarWindow.openOrRaise() }   // 0.8.26 calendar (E for events)
    function openCallSettings() { settingsWindow.openAt("calls") }
    // The URL a sound is played/published from (the server's clip cache for myinstants).
    function soundboardClipUrl(u) { return soundboardCtl.clipUrl(u) }
    function soundboardRefresh() { if (room.roomId.length > 0) soundboardCtl.refresh(room.roomId) }
    // Recents: the last 8 distinct sounds played, persisted.
    function sbPushRecent(name, url, emoji) {
        if (!url || url.length === 0) return
        var next = [{ name: name, url: url, emoji: emoji || "" }]
        for (var i = 0; i < root.mSbRecent.length && next.length < 8; i++)
            if (root.mSbRecent[i].url !== url) next.push(root.mSbRecent[i])
        root.mSbRecent = next
        settings.applyPref("soundboard-recent", JSON.stringify(next))
    }
    // Pad (docs/PAD.md): the receiver's executor + snapshot, and the Pad window (this device driving another of mine).
    PadHost { id: padHost; app: root }
    PadWindow { id: padWindow; app: root }
    SoundboardPopover {
        id: soundboardPanel
        app: root
        roomId: room.roomId
        callPlaying: root.mSoundCallPlaying
        uploading: root.mSoundUploading
        onUploadRequested: (fileUrl, name) => soundboardCtl.uploadFile(fileUrl, name)
        roomSounds: root.mRoomSounds
        personalSounds: root.mPersonalSounds
        searchResults: root.mSoundSearch
        searching: root.mSoundSearching
        errorText: root.mSoundError
        onPlaySound: (url) => soundboardCtl.playSound(room.roomId, url)
        onStopSound: () => soundboardCtl.stopSound()
        onAddRoomSound: (name, url, emoji, dur) => soundboardCtl.addRoomSound(room.roomId, name, url, emoji, dur)
        onRemoveRoomSound: (sk) => soundboardCtl.removeRoomSound(room.roomId, sk)
        onAddPersonalSound: (name, url, emoji, dur) => soundboardCtl.addPersonalSound(name, url, emoji, dur)
        onRemovePersonalSound: (idx) => soundboardCtl.removePersonalSound(idx)
        onSearchSounds: (q) => soundboardCtl.search(q)
    }

    // Watch-party player shell (Feature #8a) — queue + transport + STUBBED video surface +
    // YouTube search. watchCtl owns the 1s drift sync clock; desiredSeek drives the (future)
    // player surface, reportPosition feeds the player's local position back to the clock.
    // Memories (0.8.8) — the DVR player: session list + POV player + clip markers +
    // optional call-synced replay riding the watch-party clock (tinhat-memory:// urls).
    MemoriesWindow {
        id: memoriesWindow
        app: root
        vodctl: room
        // 10.1 (TH-MEM-09): roomId / roomName are SET when the window is opened (below), not bound
        // to the main window's room — clicking another chat no longer tears the replay down.
        memories: root.mMemories
        memoriesError: root.mMemoriesError
        clips: root.mClips
        inCall: root.mInCall
        watchPlayback: root.mWatchPlayback
        watchInParty: root.mWatchInParty
        onRefreshRequested: { room.fetchMemoriesFor(memoriesWindow.roomId); room.fetchClipsFor(memoriesWindow.roomId) }   // 10.1 (TH-MEM-09)
        // Y1 (0.8.15): ONE Rust-side call. It joins the party and reads the CURRENT
        // playback state from the store — following a live replay of the same memory
        // (updatedAt < 60s) instead of resetting everyone to 0, else starting fresh.
        // The old QML comparison against root.mWatchPlayback was definitionally
        // against an EMPTY mirror at join time (joinParty clears it synchronously).
        onSyncStart: function(memUrl) { watchCtl.syncStartMemory(room.roomId, memUrl) }
        onSyncStop: watchCtl.leaveParty()
        onSyncPlay: watchCtl.play()
        onSyncPause: watchCtl.pause()
        onSyncSeek: function(posSecs) { watchCtl.seek(posSecs) }
        onSyncReport: function(posSecs) { watchCtl.reportPosition(posSecs) }
    }

    WatchPartyWindow {
        id: watchPartyWindow
        app: root
        roomId: room.roomId
        inParty: root.mWatchInParty
        amAdvancer: root.mWatchAdvancer
        party: root.mWatchParty
        playback: root.mWatchPlayback
        viewers: root.mWatchViewers
        searchResults: root.mWatchSearch
        searching: root.mWatchSearching
        errorText: root.mWatchError
        linkInfo: root.mWatchLink
        adding: root.mWatchAdding
        browseInfo: root.mWatchBrowse
        browsing: root.mWatchBrowsing
        suggestions: root.mWatchSuggest
        history: root.mWatchHistory
        onJoinParty: watchCtl.joinParty(room.roomId)
        onLeaveParty: watchCtl.leaveParty()
        onAssignHost: (u) => watchCtl.assignHost(u)   // 10.1 (TH-WP-05)
        onEnqueue: (url, title) => watchCtl.enqueue(url, title)
        onEnqueueItem: (json) => watchCtl.enqueueItem(json)     // 11.4
        onRemoveAt: (i) => watchCtl.removeAt(i)                 // 11.4
        onMoveItem: (from, to) => watchCtl.moveItem(from, to)   // 11.4
        onClearQueue: watchCtl.clearQueue()                     // 11.4
        // 11.3 QOL (QOL.md §5): one write each through the controller's queue edit path (vectors queueops.json)
        onMoveToNext: (from) => watchCtl.moveToNext(from, watchPartyWindow.nowUrl)
        onClearPlayed: watchCtl.clearPlayed(watchPartyWindow.nowUrl)
        onShuffleUpcoming: watchCtl.shuffleUpcoming(watchPartyWindow.nowUrl)
        // 11.3 QOL (§7): the window was closed (not Leave) while watching → a toast here with Rejoin (re-joins + re-opens)
        onLeftByClosing: (partyRoomId) => root.showPassiveNotification("You left the Media Party", 6000, "Rejoin", function () {
            if (!watchCtl.inParty) watchCtl.joinParty(partyRoomId)
            watchPartyWindow.openOrRaise()
        })
        onAddLink: (text, playNow) => watchCtl.addLink(text, playNow)   // 11.4
        onEnqueueItems: (json) => watchCtl.enqueueItems(json)           // 11.4
        onBrowse: (kind, params) => watchCtl.browse(kind, params)       // 11.4
        onSuggest: (q) => watchCtl.suggest(q)                           // 11.4
        onPlayUrl: (url) => watchCtl.playUrl(url)
        onPlayPressed: watchCtl.play()
        onPausePressed: watchCtl.pause()
        onSeekTo: (posSecs) => watchCtl.seek(posSecs)
        onSearchVideos: (q) => watchCtl.search(q)
        // The mpv surface feeds its local clock back so the sync loop can measure drift
        // (STRING across the boundary — never a raw double).
        onReportPosition: (posSecs) => watchCtl.reportPosition(posSecs)
        // mpv hit EOF (edge-latched in the sheet → fires once): if I'm the advancer, roll to
        // the item AFTER the one that just ended (mirrors the skip-forward button). The queue
        // is append-only, so we ADVANCE BY INDEX via nextQueueUrl() rather than re-playing
        // queue[0] (which would loop the first item forever). Empty ("" — last item / not
        // found) means stop advancing. Followers wait for the advancer's playback write.
        onMediaEnded: {
            if (!root.mWatchAdvancer)
                return
            var next = watchPartyWindow.nextQueueUrl()
            if (next.length > 0)
                watchCtl.playUrl(next)
        }
    }

    // Squad calendar (0.8.26): one per space — game nights, boss runs, free play, proposals,
    // RSVPs, standing rules. Opens from the room header (button + next-up chip) / Ctrl+Shift+E.
    // 9.0 Game Reel (shared panel)
    GameReelWindow {
        id: gameReelWindow
        app: root
        ctl: gamesCtl
    }

    CalendarWindow {
        id: calendarWindow
        app: root
        roomId: room.roomId
        roomName: root.mRoomName
        containerId: root.mCalContainer
        containerLabel: root.mCalContainerLabel
        events: root.mCalEvents
        games: root.mCalGames
        containers: root.mCalContainers
        canEdit: root.mCalCanEdit
        isMod: root.mCalIsMod
        myId: root.mCalMyId
        busy: root.mCalBusy
        reminders: root.mCalReminders
        errorText: root.mCalError
        missing: root.mCalMissing
        canInvite: root.mCalCanInvite
        unjoinedParent: root.mCalUnjoinedParent
        sourceLabel: root.mCalSourceLabel
        onInviteMissing: calCtl.inviteMissing()
        onRefreshRequested: calCtl.refresh()
        onPickGame: (id) => { gamesCtl.setSession(id); gameReelWindow.openOrRaise() }
        onOpenContainer: (id) => calCtl.openContainer(id)
        onSaveEvent: (json) => calCtl.saveEvent(json)
        onDeleteEvent: (id, when) => calCtl.deleteEvent(id, when)
        onRsvp: (id, status, optionsJson) => calCtl.rsvp(id, status, optionsJson)
        onLockOption: (id, index, when) => calCtl.lockOption(id, index, when)
        onLinkLegacyCards: (json, hint) => calCtl.linkLegacyCards(json, hint)
        onSaveGame: (json) => calCtl.saveGame(json)
        onDeleteGame: (slug) => calCtl.deleteGame(slug)
        onSetReminders: (on) => calCtl.applyReminders(on)
    }
    // RBAC (docs/RBAC.md §7.2): a space's settings — Overview / Members / Roles / Rooms. Props in
    // from the mSpace* mirrors, every signal out to spaceCtl; the window never reads a controller.
    SpaceSettingsWindow {
        id: spaceSettingsWindow
        app: root
        spaceId: root.mSpaceId
        busy: root.mSpaceBusy
        summary: root.mSpaceSummary
        members: root.mSpaceMembers
        roles: root.mSpaceRoles
        perms: root.mSpacePerms
        rooms: root.mSpaceRooms
        userSearch: root.mSpaceUserSearch
        rolesActive: root.mSpaceRolesActive
        myRank: root.mSpaceMyRank
        permissionCatalog: root.mPermissionCatalog
        myMxid: root.mUserId
        statusText: root.mSpaceStatus
        statusRev: root.mSpaceStatusRev
        errorText: root.mSpaceError
        errorRev: root.mSpaceErrorRev
        onOpenRequested: (id) => spaceCtl.open(id)
        onCloseRequested: spaceCtl.close()
        onRefreshRequested: spaceCtl.refresh()
        onApplyName: (name) => spaceCtl.applyName(name)
        onApplyTopic: (topic) => spaceCtl.applyTopic(topic)
        onApplyPhoto: (fileUrl) => spaceCtl.applyPhoto(fileUrl)
        onApplyJoinRule: (rule) => spaceCtl.applyJoinRule(rule)
        onInvite: (mxid) => spaceCtl.invite(mxid)
        onSearchUsers: (query) => spaceCtl.searchUsers(query)
        onKick: (mxid, reason) => spaceCtl.kick(mxid, reason)
        onBan: (mxid, reason) => spaceCtl.ban(mxid, reason)
        onUnban: (mxid) => spaceCtl.unban(mxid)
        onSetMemberRoles: (mxid, rolesJson) => spaceCtl.setMemberRoles(mxid, rolesJson)
        onSaveRole: (roleJson) => spaceCtl.saveRole(roleJson)
        onDeleteRole: (roleId) => spaceCtl.deleteRole(roleId)
        onSetupRoles: spaceCtl.setupRoles()
        onSyncRooms: spaceCtl.syncRooms()
        onSyncRoom: (roomId) => spaceCtl.syncRooms()   // v1: the controller reconciles the whole space
        onLeave: spaceCtl.leave()
        onViewProfile: (mxid) => root.openProfileFlyout(mxid)
        onMessageUser: (mxid) => login.createDm(mxid)
        onCopyLink: (link) => { root.copyToClipboard(link); root.showPassiveNotification("Space link copied", 2500) }
    }
    // §7.1: every entry point lands here. tab ∈ overview | members | roles | rooms.
    function openSpaceSettings(spaceId, tab) {
        var id = String(spaceId || "")
        if (id.indexOf("!") !== 0) { root.showPassiveNotification("Pick a space first — this room isn't in one", 3000); return }
        spaceCtl.open(id)
        spaceSettingsWindow.openOrRaise(tab || "overview")
    }
    Shortcut {
        sequences: actionsHost.keysFor("space.settings.open")
        context: Qt.ApplicationShortcut
        enabled: root.mLoggedIn && root.mNavActiveSpace.indexOf("!") === 0
        onActivated: root.openSpaceSettings(root.mNavActiveSpace, "overview")
    }
    // Icon gallery (0.8.26): every icon name through ThIcon — review an icon pack in the real app.
    IconGalleryWindow {
        id: iconGallery
        app: root
        onReloadRequested: iconPack.reload()
    }

    // The watch-party sync clock's seek signal → the live mpv player surface. A HARD drift
    // correction: seek the LOCAL surface only (the playback-state write already happened on
    // the writer side, so we don't echo it back).
    Connections {
        target: watchCtl
        function onDesiredSeek(posSecs) {
            var wpUrl = String((root.mWatchPlayback && root.mWatchPlayback.url) || "")
            if (memoriesWindow.synced)
                memoriesWindow.applyRemoteSeek(posSecs)
            else if (wpUrl.indexOf("tinhat-memory://") !== 0)
                watchPartyWindow.applyRemoteSeek(posSecs)
            // else: a memory clock we are NOT synced to — drop it. A stale party
            // (finished group replay still in room state) must never move a local
            // player, and tinhat-memory urls must never reach the YouTube surface
            // (the stale clock once pinned an unsynced Memories player).
        }
    }

    // #6 Floating chat bubbles — manager renders open bubbles as top-level Windows, each
    // with its OWN RoomController feeding a receipt-free ConversationView. Mounted once at
    // the root; only does anything once there are open bubbles (registry empty by default).
    FloatingChats {
        id: floatingChats
        app: root
        floating: floatingCtl
        gif: gifCtl   // shared Giphy picker for each bubble's composer GIF button
    }


    // User-profile flyout (avatar click in the timeline, or a member-row tap).
    Controls.Popup {
        id: profileFlyout
        parent: Controls.Overlay.overlay
        modal: true
        focus: true
        closePolicy: Controls.Popup.CloseOnEscape | Controls.Popup.CloseOnPressOutside
        width: 320
        // 10.6: + the presence line (REACH §2.6 item 4) when they share
        height: 300 + (profileFlyout.pres ? 26 : 0)
        x: (parent.width - width) / 2
        y: (parent.height - height) / 2
        padding: Kirigami.Units.largeSpacing
        readonly property var pres: root.presenceOf(root.profileMxid)

        // Resolve the clicked mxid against the loaded member list (name/avatar/role).
        readonly property var profile: {
            var list = root.mMembers || []
            for (var i = 0; i < list.length; i++)
                if (list[i].mxid === root.profileMxid) return list[i]
            return ({})
        }

        background: Kirigami.ShadowedRectangle {
            radius: root.radius
            color: Th.profileBg
            border.width: 1
            border.color: Th.profileBorder
            shadow.size: 16; shadow.color: Th.profileShadow; shadow.yOffset: 4
        }

        ColumnLayout {
            anchors.fill: parent
            spacing: Kirigami.Units.smallSpacing
            Avatar {
                Layout.alignment: Qt.AlignHCenter
                app: root
                size: 72
                name: profileFlyout.profile.name || root.profileMxid
                seed: root.profileMxid
                imageSource: profileFlyout.profile.avatar || ""
            }
            Controls.Label {
                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                Layout.fillWidth: true
                horizontalAlignment: Text.AlignHCenter
                text: profileFlyout.profile.name || root.localpart(root.profileMxid)
                // 10.4 (space tags): the custom role's colour, else the hashed hue.
                color: (profileFlyout.profile.tagColor && String(profileFlyout.profile.tagColor).length > 0)
                       ? String(profileFlyout.profile.tagColor) : root.senderColor(root.profileMxid)
                font.bold: true
                font.pointSize: Kirigami.Theme.defaultFont.pointSize * 1.2
                wrapMode: Text.WordWrap
            }
            Controls.Label {
                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                Layout.fillWidth: true
                horizontalAlignment: Text.AlignHCenter
                text: root.profileMxid
                color: Th.profileMxidFg
                font.pointSize: Kirigami.Theme.smallFont.pointSize
                wrapMode: Text.WrapAnywhere
            }
            // 10.6 (REACH §2.6 item 4 + A4.5): "● Online" / "● In a call · <room>" / the status chip.
            PresenceLine {
                Layout.alignment: Qt.AlignHCenter
                Layout.maximumWidth: parent.width
                app: root
                pres: profileFlyout.pres
                roomName: (profileFlyout.pres && profileFlyout.pres.state === "call" && profileFlyout.pres.room) ? root.roomNameOf(String(profileFlyout.pres.room)) : ""
            }
            // 10.4 (space tags): the role tag — the custom role (name + colour) when they have one,
            // else the legacy Admin / Mod word from the power level; the baseline role wears nothing.
            RoleBadge {
                Layout.alignment: Qt.AlignHCenter
                app: root
                readonly property bool hasTag: !!(profileFlyout.profile.tagName && String(profileFlyout.profile.tagName).length > 0)
                readonly property string rn: hasTag ? String(profileFlyout.profile.tagName) : String(profileFlyout.profile.role || "")
                readonly property string rc: hasTag ? String(profileFlyout.profile.tagColor || "") : ""
                name: (rn === "Member" || rn === "Everyone") ? "" : rn
                roleColor: rc.length > 0 ? rc : (rn === "Admin" ? String(Th.rolesBadgeAdminFg) : String(Th.rolesBadgeModFg))
            }
            GlowButton {
                Layout.fillWidth: true
                Layout.topMargin: Kirigami.Units.smallSpacing
                app: root
                text: "Direct message"
                // 10.1 (TH-SHELL-03): createDm has worked for months — the card said "coming soon".
                onClicked: { login.createDm(root.profileMxid); profileFlyout.close() }
            }
        }
    }

    // THEMES-UI §12: the ☰ Theme menu's "More themes…" — Settings → Appearance with the theme gallery open (§4), a fresh
    // one (no search, All, at the top), like the web's openThemeGallery
    function openThemeGallery() {
        settingsWindow.appearance.openGalleryFresh()
        settingsWindow.openAt("appearance")
    }
    // THEMES-UI §12 (amendment 2026-09-28): the ☰ Theme menu's "Edit current theme…" — the theme editor on the theme on
    // screen, through the editor's own entry point Customize… (AppearancePane.customize → AppCore.openThemeEditor: one of
    // My themes → Edit…, any other → New theme from this, §13.1), like the web's editCurrentTheme. The editor docks over
    // the live app (§13.6), so no Settings window opens; an editor with changes stays, with its toast (§13.1).
    function editCurrentTheme() {
        settingsWindow.appearance.customize()
    }

    // ======================= Settings window (contract SETTINGS-WINDOW.md, docs/SETTINGS-WINDOW.md) ==============
    // A real top-level window since 2026-09-29 (was the OverlaySheet `settingsSheet`): SettingsWindow.qml. `settingsSheet`
    // stays as its other name (the ☰ menu, the gap menu, ActionsHost's ui) — open() / openAt(key) / close() / visible.
    // Keys: profile privacy security (sessions) invites audio calls appearance notifications shortcuts overlay about;
    // the 10.x ints still work (openAt(3) = calls).
    SettingsWindow {
        id: settingsWindow
        app: root
        core: core
    }
    property alias settingsSheet: settingsWindow
    // the old About sheet's name: the ☰ menu's "About Tin Hat" and app.about open Settings → About & help
    QtObject {
        id: aboutSheet
        function open() { settingsWindow.openAt("about") }
    }
    // A toast where the person is looking: the Settings window's strip while it is the active window, else the main
    // window's passive notification (Appearance's Undo, layout outcomes, pack errors …)
    function settingsToast(message, timeout, actionText, callBack) {
        if (settingsWindow.visible && settingsWindow.active) settingsWindow.toast(message, timeout, actionText, callBack)
        else if (actionText !== undefined) root.showPassiveNotification(LinkGate.plain(message), timeout, actionText, callBack)
        else if (timeout !== undefined) root.showPassiveNotification(LinkGate.plain(message), timeout)
        else root.showPassiveNotification(LinkGate.plain(message))
    }
    // 10.1 (TH-SET-11), now between windows: a surface of THIS window opened from Settings — Settings steps aside, this
    // window comes forward, and closing that surface brings Settings back on the pane you left (`pane` "" = no return).
    property string _returnToSettingsPane: ""
    property var _handOffPopup: null
    function settingsHandOff(pane, what, arg) {
        var pop = null
        switch (what) {
        case "verify": pop = verifySheet; break
        case "backup": pop = verifySheet; break          // 10.1 (TH-SET-03): the verify sheet shows progress + the key
        case "noiseTest": pop = noiseTestSheet; break
        case "shortcuts": pop = shortcutsDialog; break
        case "bugReport": if (!bugReportDialog.keepNote) bugReportDialog.clearForm(); pop = bugReportDialog; break
        case "whatsNew": pop = updateNotesSheet; break
        case "signOut": pop = logoutConfirm; break
        case "rowEditor": pop = actionsHost.rowEditorDialog; break
        case "roomPicker": pop = quickSwitcher; break    // the caller opens it (shareThemeToRoom)
        case "tour": pane = ""; break
        default: return
        }
        root._returnToSettingsPane = pop ? String(pane || "") : ""
        root._handOffPopup = pop
        settingsWindow.close()
        root.raise(); root.requestActivate()
        if (what === "tour") welcomeTour.start()   // re-openable any time, ignores the seen flag
        else if (what === "backup") { verifySheet.open(); login.setUpRecovery(arg) }
        else if (what === "rowEditor") actionsHost.openRowEditor(JSON.parse(arg))
        else if (what !== "roomPicker") pop.open()
    }
    Connections {
        target: root._handOffPopup
        ignoreUnknownSignals: true
        function onClosed() {
            var p = root._returnToSettingsPane
            root._handOffPopup = null; root._returnToSettingsPane = ""
            if (p.length > 0 && root.mLoggedIn) settingsWindow.openAt(p)
        }
    }
    Connections {   // signing out from About & help: no way back to Settings
        target: logoutConfirm
        function onConfirmed() { root._returnToSettingsPane = "" }
    }
    Connections {   // signed out (or the session died): the window of the account goes too
        target: root
        function onMLoggedInChanged() { if (!root.mLoggedIn) settingsWindow.close() }
    }

    // Noise-cancellation before/after meter. ROOT-scoped (not inside the room page) so
    // both openers work: the gear menu's "Test noise cancellation…" and the Settings →
    // Audio pane's button. Opening starts a standalone mic monitor (works outside a
    // call) feeding micLevelPre (raw, left) + micLevel (post-RNNoise, right); toggling
    // noise cancellation should visibly shrink the right meter on background noise.
    Kirigami.OverlaySheet {
        id: noiseTestSheet
        parent: Controls.Overlay.overlay
        property bool isOpen: false
        onOpened: { isOpen = true; callctl.startMicMonitor() }
        // Always request teardown: stopMicMonitor is idempotent + clears the
        // "wanted" flag (so an in-flight async start tears itself down), and it only
        // zeroes the shared mic level when a monitor was actually running — so it
        // can't leak the monitor on a fast open→close, nor stomp a live call's level.
        onClosed: { isOpen = false; callctl.stopMicMonitor() }
        title: "Noise cancellation"
        ColumnLayout {
            spacing: Kirigami.Units.largeSpacing
            Controls.Label {
                Layout.fillWidth: true
                Layout.maximumWidth: Kirigami.Units.gridUnit * 26
                wrapMode: Text.WordWrap
                color: Th.dialogHintFg
                text: "Talk into your mic. Left is your raw input; right is after RNNoise. "
                    + "Toggle it below — steady background noise should shrink on the right."
            }
            ThSwitch {
                enabled: root.mNcAvailable
                opacity: root.mNcAvailable ? 1.0 : 0.5
                text: root.mNcAvailable ? "Strong suppression (RNNoise) enabled"
                                        : "Strong suppression (RNNoise) — not installed on this machine"   // 10.1 (TH-COPY-08): same words as the Audio pane
                checked: root.mMicDenoise && root.mNcAvailable
                onToggled: { callctl.applyMicDenoise(checked); checked = Qt.binding(() => root.mMicDenoise) }
            }
            // The actionable half of the error. The old hint named gst-plugins-bad, which does
            // not contain audiornnoise at all — users installed it, nothing changed, and the
            // toggle went on looking available.
            Controls.Label {
                visible: !root.mNcAvailable
                Layout.fillWidth: true
                wrapMode: Text.WordWrap
                color: Th.dialogErrorFg
                text: "The RNNoise plugin isn't installed, so noise cancellation can't run. "
                    + "Install gst-plugin-rsaudiofx (part of gst-plugins-rs) and restart Tin Hat. "
                    + "It is not part of gst-plugins-bad."
            }
            // Shown when the live monitor couldn't start (e.g. the audiornnoise plugin
            // from gst-plugins-bad is missing) so the user isn't left staring at flat bars.
            Controls.Label {
                Layout.fillWidth: true
                Layout.maximumWidth: Kirigami.Units.gridUnit * 26
                visible: noiseTestSheet.isOpen && !callctl.micMonitoring
                wrapMode: Text.WordWrap
                color: Th.dialogHintFg
                text: "Live meter unavailable — the noise-cancellation plugin (gst-plugin-rsaudiofx, from gst-plugins-rs) isn't installed, "
                    + "or the mic couldn't be opened."
            }
            RowLayout {
                visible: callctl.micMonitoring
                Layout.fillWidth: true
                spacing: Kirigami.Units.largeSpacing * 2
                ColumnLayout {
                    spacing: Kirigami.Units.smallSpacing
                    Controls.Label { text: "Before (raw)"; color: Th.dialogFg; font.bold: true }
                    LevelMeter { level: root.mMicLevelPre; barColor: Th.settingsMeterRawFill }
                }
                ColumnLayout {
                    spacing: Kirigami.Units.smallSpacing
                    Controls.Label {
                        text: root.mMicDenoise ? "After (cancelled)" : "After (off — same as raw)"
                        color: Th.dialogFg; font.bold: true
                    }
                    LevelMeter { level: root.mMicLevel; barColor: Th.settingsMeterFill }
                }
            }
        }
    }

    // Self-test only (env TINHAT_GUI_AUTOJOIN): once logged in, open the target room
    // and join its call automatically, so the live render path can be verified
    // without a manual click. No-op when the property is empty.
    Timer {
        id: autoJoinTimer
        interval: 1500
        repeat: true
        running: login.autoJoinRoom.length > 0
        property bool done: false
        onTriggered: {
            if (done) { running = false; return }
            if (login.loggedIn && login.autoJoinRoom.length > 0) {
                done = true
                root.openRoomId(login.autoJoinRoom)
                callctl.joinCall(login.autoJoinRoom)
                // Test hook: also publish the camera a moment after the call connects.
                if (login.autoCam) autoCamTimer.start()
                if (login.autoMute) autoMuteTimer.start()
                root.raise(); root.requestActivate() // bring to front for the self-test screenshot
                running = false
            }
        }
    }
    // Self-test only (env TINHAT_GUI_SECKEY_FILE: a lab gate's file with the account's Security Key): once signed in
    // and the first-device check is over, verify THIS device with it — login.verify(), what the verify sheet's
    // "Verify with Security Key" button calls — so a gate's desktop device is a verified device of its account.
    Timer {
        id: autoKeyTimer
        interval: 1500
        repeat: true
        running: !autoKeyTimer.done && login.autoSecKey.length > 0
        property bool done: false
        onTriggered: {
            if (!login.loggedIn || login.firstDevicePending || root.mBusy) return
            done = true
            var key = login.autoSecKey
            login.autoSecKey = ""   // used once: the key isn't kept in the property (Astra 2026-10-26)
            if (login.verified) { console.log("[autokey] this device is verified already"); return }
            console.log("[autokey] verifying this device with the Security Key (TINHAT_GUI_SECKEY_FILE)")
            login.verify(key)
        }
    }
    // Test hook (TINHAT_GUI_AUTOMUTE): mute the mic ~6s after auto-join (call connected),
    // so a peer can confirm the muted indicator now shows in their pill.
    Timer {
        id: autoMuteTimer
        interval: 6000; repeat: false
        onTriggered: { console.log("[automute] muting mic for pill test"); callctl.toggleMic() }
    }
    // Test hook (TINHAT_GUI_AUTOCAM): enable the camera ~4s after auto-join (call connected).
    Timer {
        id: autoCamTimer
        interval: 4000; repeat: false
        onTriggered: if (!callctl.cameraOn) callctl.toggleCamera()
    }


    // Screenshot hook (env TINHAT_GUI_OPENROOM): once logged in, OPEN the target room
    // (timeline only — does NOT join the call), so the conversation UI can be captured
    // without disturbing audio/mic. No-op when the property is empty.
    Timer {
        id: autoOpenTimer
        interval: 1200
        repeat: true
        running: login.autoOpenRoom.length > 0
        property bool done: false
        onTriggered: {
            if (done) { running = false; return }
            if (login.loggedIn && root.mRooms.length > 0) {
                done = true
                // RBAC screenshot hooks: "space:<id>:<tab>" selects that space in the rail and
                // opens its Space settings on <tab>; "selectspace:<id>" only selects it (the
                // invited-space rail + card capture). Neither opens a room.
                if (login.autoOpenRoom.indexOf("space:") === 0 || login.autoOpenRoom.indexOf("selectspace:") === 0) {
                    var sel = login.autoOpenRoom.indexOf("selectspace:") === 0
                    var rest = login.autoOpenRoom.substring(sel ? 12 : 6)
                    var tabName = "overview", sid = rest
                    var lc = rest.lastIndexOf(":")
                    var tail = lc >= 0 ? rest.substring(lc + 1) : ""
                    if (!sel && ["overview", "members", "roles", "rooms"].indexOf(tail) >= 0) { tabName = tail; sid = rest.substring(0, lc) }
                    if (nav && nav.selectSpace) nav.selectSpace(sid)
                    if (!sel) Qt.callLater(function() { root.openSpaceSettings(sid, tabName) })
                    running = false
                    return
                }
                // TINHAT_GUI_OPENROOM="0".."N" picks that room index; "!room:id" (9.3) picks that
                // room; anything else = first.
                var idx = parseInt(login.autoOpenRoom)
                if (isNaN(idx) || idx < 0 || idx >= root.mRooms.length) idx = 0
                if (login.autoOpenRoom.charAt(0) === "!")
                    for (var ri = 0; ri < root.mRooms.length; ri++) if (root.mRooms[ri].id === login.autoOpenRoom) { idx = ri; break }
                root.openRoomId(root.mRooms[idx].id)
                // Verification aid: tuck the nav away for a full-width fake-call capture.
                // Set the mode in-memory only (NOT setNavMode) so the screenshot hook never
                // persists "corner" over the user's saved preference.
                if (root.fakeCall) root.navMode = "corner"
                // Screenshot/debug aid: TINHAT_GUI_OPENROOM="panel"|"info" also opens the
                // right panel (members / info tab) so it can be captured headless.
                if ((login.autoOpenRoom === "panel" || login.autoOpenRoom === "info") && root.roomInfoPanel) {
                    if (login.autoOpenRoom === "info") root.roomInfoPanel.tab = 0
                    Qt.callLater(root.roomInfoPanel.open)
                }
                running = false
            }
        }
    }






    // On sign-out, drop any open conversation page.
    onMLoggedInChanged: {
        if (!mLoggedIn) {
            root._tourAutoConsidered = false // re-evaluate the tour on the next sign-in
            loginPage.clearCredentials()     // 10.1 (TH-FIRST-12)
            // the session's end left the call already (bridge end_session: the media, the membership); its controls
            // go too, and the verify dialog closes (clearing what was typed into it)
            if (callctl.inCall || callctl.connecting) callctl.leaveCall()
            verifySheet.close()
            room.setTyping(false)
            root.roomShown = false   // 11.0: the room's islands leave the layout
            editModeObj.cancel()     // 11.0 edit mode: signing out = Cancel without asking (EDITMODE-UI §1)
        } else {
            // F1: while a new account's first device sets itself up, the prompt waits (onFirstDevicePendingChanged)
            if (!mVerified && !root.noVerifyPrompt && !login.firstDevicePending && !guestsCtl.isGuestAccount(login.userId))
                verifySheet.open() // prompt to verify on sign-in (auto-closes once verified); never a guest account
            // First-run log-sharing consent: load the persisted flag, and if it's
            // never been set (-1), inform the user + let them opt out. Deferred a beat
            // so it doesn't collide with the verify dialog on a brand-new device.
            root.mLogship = settings.logshipState()
            if (root.mLogship === -1 && root.serverOn("reports"))   // selfhost: only where the server takes logs
                logshipConsentTimer.restart()
            // 10.1 (TH-SHELL-06 / TH-CONV-22): back to the room you were in, with your drafts.
            core.restoreChatMemory()
            // 10.1: Appearance prefs that live in the ui-pref store.
            root.mReducedMotion = settings.uiPref("reduce-motion") === "1"
            root.layoutStyle = settings.uiPref("layout-style") === "flat" ? "flat" : "islands"   // 10.8
            Motion.gifAutoplay = settings.uiPref("gif-autoplay") !== "0"   // 11.0 (GPU.md §2.6): default on
            core.mCallUnfocusedSaver = root.lowgpu.unfocusedSaverOn(settings.uiPref("call-unfocused-saver"))   // 10.8.1: default OFF
            var cm = settings.uiPref("clock"); root.clockMode = (cm === "12" || cm === "24") ? cm : "system"
            lastRoomTimer.tries = 0
            lastRoomTimer.restart()
        }
    }
    // 10.1 (TH-CONV-03): the sidebar next to a blank column was the first thing a new sign-in saw.
    function _showWelcome() {
        // 11.0 (islands C6/C7): nothing to push — with no room open the timeline island shows "Pick a room to start"
    }
    // Waits for the room list (it lands a beat after mLoggedIn), then opens the remembered room —
    // unless a gate hook owns the first screen, or the user has already picked a room.
    Timer {
        id: lastRoomTimer
        property int tries: 0
        interval: 700
        onTriggered: {
            if (!root.mLoggedIn || root.roomShown) return
            if (login.autoOpenRoom.length > 0 || login.autoJoinRoom.length > 0 || login.autoFakeCall.length > 0) return
            var last = settings.uiPref("last-room")
            if (!last || last.length === 0) { root._showWelcome(); return }
            var rooms = root.mRooms || []
            for (var i = 0; i < rooms.length; i++)
                if (rooms[i].id === last && rooms[i].membership !== "invite" && rooms[i].space !== true) { root.openRoomId(last); return }
            if (++tries < 8) lastRoomTimer.restart()   // the list is still loading
            else root._showWelcome()
        }
    }
    Timer {
        id: logshipConsentTimer
        interval: 1200
        onTriggered: {
            if (root.mLogship !== -1) return
            // F1/F2: not over a new account's first-device setup or its one-time Security Key
            if (login.firstDevicePending || (login.firstDevice && verifySheet.visible)) logshipConsentTimer.restart()
            else logshipConsent.open()
        }
    }

    // Auto-close the verify sheet once the device becomes verified.
    property bool _verifyToastArmed: false
    onMVerifiedChanged: {
        if (!mVerified) return
        // not while the one-time Security Key is on screen (set up recovery / F2 first device) — Tin Hat Web keeps it too
        if (login.sasState === "recovery-done" || login.sasState === "recovery-working") return
        verifySheet.close()
        if (root._verifyToastArmed) { root._verifyToastArmed = false; root.showPassiveNotification("This device is verified.", 4000) }
    }

    // ======================= Verify-this-device sheet ========================
    // Component file, NOT inline — see VerifyDeviceDialog.qml / the 0.3.5 no-boot.
    VerifyDeviceDialog {
        id: verifySheet
        app: root
        busy: root.mBusy
        status: root.mStatus
        sasState: login.sasState
        sasEmoji: login.sasEmoji
        sasNote: login.sasNote
        sasPeer: login.sasPeer
        sasIncoming: login.sasIncoming
        sasCanStart: login.sasCanStart
        onVerifyRequested: (key) => login.verify(key)
        onUseAnotherDeviceRequested: login.startDeviceVerification()
        onSasAccepted: login.acceptDeviceVerification()
        onSasConfirmed: login.confirmSasMatch()
        onSasMismatched: login.mismatchDeviceVerification()
        onSasCancelled: login.cancelDeviceVerification()
        onStartComparisonRequested: login.startEmojiComparison()
        recoveryKey: login.recoveryKey
        firstDevice: login.firstDevice
        canBootstrap: login.sasCanBootstrap
        keyFirst: login.keyFirst
        onSetUpRecoveryRequested: (pw) => login.setUpRecovery(pw)
        startOverNote: login.startOverNote
        onStartOverRequested: (pw) => login.startOver(pw)
    }

    // A device we're already signed in to can be asked to verify a NEW one, so the
    // request has to surface even when the user isn't sitting in the verify dialog.
    Connections {
        target: login
        function onSasStateChanged() {
            if (login.sasState === "waiting" && login.sasIncoming && !verifySheet.opened)
                verifySheet.open()
            // F2: the first device's new Security Key (--tinhat-noverify too: it is shown once, never skipped)
            // (12.0.1: and any other new key — Start over's, Set up recovery's — if the sheet was closed meanwhile)
            if (login.sasState === "recovery-done" && (login.firstDevice || login.recoveryKey.length > 0) && !verifySheet.opened)
                verifySheet.open()
            // 10.1 (TH-FIRST-19): the emoji matched — the keys arrive a moment later; say "verified" only then.
            // Braces (2026-09-25): without them the toast fired on EVERY sasState change
            // (waiting, emoji, cancelled …), not just on done.
            if (login.sasState === "done") {
                root._verifyToastArmed = true
                root.showPassiveNotification("Emoji matched — finishing up…", 3000)
            }
        }
        // F1: the first-device check ended. Take the verified flag now (the 120 ms poll would leave the banner up for a
        // tick), then the sign-in prompt it held back — only if this device still is not verified.
        function onFirstDevicePendingChanged() {
            if (login.firstDevicePending) return
            root.mVerified = login.verified
            if (root.mLoggedIn && !login.verified && !root.noVerifyPrompt && !verifySheet.opened && !guestsCtl.isGuestAccount(login.userId))
                verifySheet.open()
        }
    }

    // ======================= The room's menu + gate hooks ====================================
    // 11.0 (islands C6/C7): these lived in the room page (a Kirigami.Page created on the first open); the room's
    // islands are always there now, so they sit at the window (hooks keyed to "signed in" also wait for a room).
    // Consolidated device + call menu opened by the header's gear ▾ (and the
    // in-call bar's gear). Grouped with section headers so the ~20 rows read as
    // five short lists, not one grab-bag; selection marks are NATIVE check
    // indicators (CheckMenuItem — labels no longer shift when picked).
    Controls.Menu {
        id: devicesMenu
        modal: true
        MenuSectionLabel { app: root; text: "Devices" }
        Controls.Menu {
            title: "Microphone"
            icon.name: "audio-input-microphone-symbolic"
            CheckMenuItem {
                text: "System default"
                isChecked: () => root.mInputName.length === 0
                onTriggered: callctl.setInput("")
            }
            Repeater {
                model: root.mInputs
                delegate: CheckMenuItem {
                    required property var modelData
                    text: modelData.label
                    isChecked: () => root.mInputName === modelData.node
                    onTriggered: callctl.setInput(modelData.node)
                }
            }
        }
        Controls.Menu {
            title: "Speaker"
            icon.name: "audio-volume-high-symbolic"
            CheckMenuItem {
                text: "System default"
                isChecked: () => root.mOutputName.length === 0
                onTriggered: callctl.setOutput("")
            }
            Repeater {
                model: root.mOutputs
                delegate: CheckMenuItem {
                    required property var modelData
                    text: modelData.label
                    isChecked: () => root.mOutputName === modelData.node
                    onTriggered: callctl.setOutput(modelData.node)
                }
            }
        }
        Controls.Menu {
            title: "Camera"
            icon.name: "camera-video-symbolic"
            CheckMenuItem {
                text: "Default (first camera)"
                isChecked: () => root.mCameraName.length === 0
                onTriggered: callctl.setCamera("")
            }
            Repeater {
                model: root.mCameras
                delegate: CheckMenuItem {
                    required property var modelData
                    text: modelData.label
                    isChecked: () => root.mCameraName === modelData.node
                    onTriggered: callctl.setCamera(modelData.node)
                }
            }
        }
        // (Microphone-processing toggles moved to Settings → Audio — the gear
        // menu was "too confusing" per live friends feedback; it now holds only
        // quick device switches, share actions, and the share-quality submenu.)
        Controls.MenuSeparator {}
        MenuSectionLabel { app: root; text: "Share audio" }
        // #1 Music — SHARE A PICKED AUDIO OUTPUT (the primary UX): capture a
        // PipeWire source (a speaker/app .monitor, or an input) you select. The
        // choice is remembered across sessions (check mark), and "Start sharing
        // saved source" reuses it. File/URL stay below for ad-hoc playback.
        Controls.Menu {
            title: "Share audio output"
            icon.name: "folder-music-symbolic"
            Controls.MenuItem {
                text: "Start sharing saved source"
                enabled: root.mInCall && root.mMusicSourceName.length > 0
                onTriggered: callctl.startMusicSource("")
            }
            Controls.MenuSeparator {}
            Repeater {
                model: root.mAudioSources
                delegate: CheckMenuItem {
                    required property var modelData
                    text: modelData.label
                    isChecked: () => root.mMusicSourceName === modelData.node
                    // Selecting persists the choice; the share itself starts only
                    // when in a call (startMusicSource no-ops otherwise), so a
                    // source can be picked ahead of time — same as Settings.
                    onTriggered: callctl.startMusicSource(modelData.node)
                }
            }
        }
        Controls.MenuItem {
            text: "Play music from file…"
            enabled: root.mInCall
            onTriggered: musicFileDialog.open()
        }
        Controls.MenuItem {
            text: "Play music from URL…"
            icon.name: "link-symbolic"
            enabled: root.mInCall
            onTriggered: { musicUrlField.text = ""; musicUrlDialog.open() }
        }
        Controls.MenuItem {
            // One verb for both forms (source share AND file/URL playback are the
            // same second audio track to peers).
            text: "Stop sharing audio"
            icon.name: "media-playback-stop-symbolic"
            enabled: root.mMusicPlaying
            onTriggered: callctl.stopMusic()
        }
        Controls.MenuSeparator {}
        MenuSectionLabel { app: root; text: "Quality" }
        Controls.Menu {
            id: sqMenu
            // Redesigned per live friends feedback ("the options are messy"):
            // three orthogonal choices — Resolution × Frame rate × Bandwidth —
            // instead of opaque named presets. kbps is DERIVED: a per-resolution
            // base scaled by the bandwidth tier, so every combination stays
            // sane without users thinking in megabits. Encoder stays advanced.
            title: "Screen-share quality"
            icon.name: "monitor-symbolic"
            // Base kbps per resolution box (upper bound; congestion control
            // still adapts downward), and the Good/Great/Best multipliers.
            // Data + helpers hoisted to root (shareResList/shareBwList — see
            // the comment there); these aliases keep the menu internals terse.
            readonly property var resList: root.shareResList
            readonly property var bwList: root.shareBwList
            function _curRes() { return root.shareCurRes() }
            function _curTier() { return root.shareCurTier() }
            MenuSectionLabel { app: root; text: "Resolution" }
            Repeater {
                model: sqMenu.resList
                delegate: CheckMenuItem {
                    required property var modelData
                    text: modelData.label
                    isChecked: () => root._sqW() === modelData.w && root._sqH() === modelData.h
                    onTriggered: callctl.applyShareQuality(
                        modelData.w, modelData.h, root._sqFps(),
                        Math.round(modelData.base * sqMenu.bwList[sqMenu._curTier()].f))
                }
            }
            Controls.MenuSeparator {}
            MenuSectionLabel { app: root; text: "Frame rate" }
            Repeater {
                model: [5, 15, 30, 60]
                delegate: CheckMenuItem {
                    required property var modelData
                    text: modelData + " fps"
                    isChecked: () => root._sqFps() === modelData
                    onTriggered: callctl.applyShareQuality(root._sqW(), root._sqH(), modelData, root._sqKbps())
                }
            }
            Controls.MenuSeparator {}
            MenuSectionLabel { app: root; text: "Bandwidth" }
            Repeater {
                model: sqMenu.bwList
                delegate: CheckMenuItem {
                    required property var modelData
                    required property int index
                    text: modelData.label + "  (~" + (Math.round(sqMenu._curRes().base * modelData.f / 100) / 10) + " Mbps)"
                    isChecked: () => sqMenu._curTier() === index
                    onTriggered: callctl.applyShareQuality(
                        root._sqW(), root._sqH(), root._sqFps(),
                        Math.round(sqMenu._curRes().base * modelData.f))
                }
            }
            Controls.MenuSeparator {}
            MenuSectionLabel { app: root; text: "Encoder (advanced)" }
            Repeater {
                model: [
                    { label: "GPU · best (AV1 on RTX 40+, else H.264)", tag: "nvenc", code: 0 },
                    { label: "Sharpest · VP9 (CPU) — best for text/docs",    tag: "vp9",   code: 1 },
                    { label: "GPU · force H.264 (compat)",               tag: "h264",  code: 2 }
                ]
                delegate: CheckMenuItem {
                    required property var modelData
                    text: modelData.label
                    isChecked: () => root._sqEnc() === modelData.tag
                    onTriggered: callctl.applyShareEncoder(modelData.code)
                }
            }
        }
        Controls.MenuItem {
            // Camera quality / simulcast / incoming-video moved to the
            // settings pane (friends feedback: dropdown too confusing).
            text: "All call && quality settings…"
            icon.name: "settings-configure-symbolic"
            onTriggered: { settingsWindow.openAt("calls") }
        }
        Controls.MenuSeparator {}
        // 10.1 (TH-CONV-19): Soundboard (composer button) and Memories (header button) no
        // longer repeat here — the menu is what its tooltip says, plus Watch party.
        MenuSectionLabel { app: root; text: "Call features" }
        Controls.MenuItem {
            text: "Media Party"
            icon.name: "video-television-symbolic"
            enabled: room.roomId.length > 0
            // Auto-join on open so queue/play work immediately (queuing needs an active
            // party — room_id is only set by joinParty); the window's button becomes "Leave".
            // Guard on mWatchInParty: joinParty is DESTRUCTIVE (resets the sync state), so
            // re-opening while already watching must NOT re-join (would blank the player).
            onTriggered: {
                if (!watchCtl.inParty) watchCtl.joinParty(room.roomId)
                if (watchPartyWindow.openOrRaise) watchPartyWindow.openOrRaise(); else watchPartyWindow.show()
            }
        }
        Controls.MenuSeparator {}
        Controls.MenuItem {
            text: "Refresh this room"
            icon.name: "view-refresh-symbolic"
            onTriggered: room.refresh()
        }
    }

    // --tinhat-bubble (+ TINHAT_GUI_OPENROOM): pop out a floating chat for the open room
    // once it is up, so the shared ConversationView can be screenshotted and checked for
    // QML errors in its host.
    Timer {
        interval: 6000
        running: Qt.application.arguments.indexOf("--tinhat-bubble") >= 0 && room.roomId.length > 0
        onTriggered: {
            if (floatingCtl.isOpen(room.roomId)) floatingChats.focusBubble(room.roomId)
            else if (!floatingCtl.openBubble(room.roomId))
                root.showPassiveNotification(LinkGate.plain("You can have " + floatingCtl.maxBubbles + " popped-out chats at once — close one first"), "short")
        }
    }

    // --tinhat-maximize: flip "fill the app with the call" once the call panel
    // is up (works with TINHAT_GUI_FAKECALL too) — the portrait-layout gate needs
    // the maximized grid, and the toggle button has no stable position to click.
    Timer {
        interval: 1500
        running: Qt.application.arguments.indexOf("--tinhat-maximize") >= 0 && root.callShown
        onTriggered: root.callMaximized = true
    }
    // --tinhat-hidechip: 0.8.7 gate — hide the 3rd fake tile the USER way
    // (exercises the memory write), then pop the chip's menu for the shot.
    Timer {
        interval: 2200
        running: Qt.application.arguments.indexOf("--tinhat-hidechip") >= 0 && root.callShown
        onTriggered: if (root.callTiles.length > 2) root.hideTileRemember(root.callTiles[2])
    }
    Timer {
        interval: 3400
        running: Qt.application.arguments.indexOf("--tinhat-hidechip") >= 0 && root.callShown
        onTriggered: callIsland.hiddenMenu.popup(callIsland.hiddenChip, 0, callIsland.hiddenChip.height + 4)
    }
    // --tinhat-automusic=<node> (+ TINHAT_GUI_AUTOJOIN): start the audio share from
    // that PipeWire node 6 s into the call. 0.8.21 gate: the peer's log
    // must file the track as "screenshare" (it used to land under "music").
    Timer {
        interval: 6000
        // Qt.application.arguments is a QML list, not a JS Array (no .some) — scan it.
        readonly property string node: {
            var args = Qt.application.arguments
            for (var i = 0; i < args.length; i++)
                if (String(args[i]).indexOf("--tinhat-automusic=") === 0) return String(args[i]).substring("--tinhat-automusic=".length)
            return ""
        }
        running: root.mInCall && node.length > 0
        onTriggered: {
            console.log("[automusic] starting audio share from", node)
            callctl.startMusicSource(node)
        }
    }
    // --tinhat-tilesync (+ TINHAT_GUI_FAKECALL): drive the grid model through join /
    // mute / reorder / leave and assert that delegates survive (0.8.28).
    Timer {
        id: tileSyncProbeTimer
        property int step: 0
        property var s: ({})
        property var fails: []
        property var work: ({})
        interval: 400; repeat: true
        running: root.tileSyncProbe && root.callShown
        function snap() { return { c: root._tileCreated, d: root._tileDestroyed } }
        function ids() { var o = []; for (var i = 0; i < callIsland.tileRepeater.count; i++) { var it = callIsland.tileRepeater.itemAt(i); o.push(it ? it.objectName : "?") } return o.join(",") }
        function check(label, ok, detail) {
            console.log("[tilesync] " + (ok ? "PASS" : "FAIL") + " " + label + (detail ? " (" + detail + ")" : ""))
            if (!ok) fails.push(label)
        }
        onTriggered: {
            var w = work, st = s
            switch (step) {
            case 0:
                if (root._tileCreated < 2) return // wait for the fake grid
                w.base = root.fakeTiles.slice()
                st.s0 = snap()
                check("baseline delegates", callIsland.tileRepeater.count === w.base.length, callIsland.tileRepeater.count + " vs " + w.base.length)
                w.joined = w.base.concat([{ id: "avatar_@newcomer:example.com:ABC", participant: "@newcomer:example.com:ABC", kind: "avatar", name: "Newcomer" }])
                root.fakeTiles = w.joined
                break
            case 1:
                st.s1 = snap()
                check("join creates 1, destroys 0", st.s1.c - st.s0.c === 1 && st.s1.d === st.s0.d, (st.s1.c - st.s0.c) + " created, " + (st.s1.d - st.s0.d) + " destroyed")
                w.muted = w.joined.map(function(t, i) { var c = JSON.parse(JSON.stringify(t)); if (i === 1) c.muted = true; return c })
                root.fakeTiles = w.muted
                break
            case 2:
                st.s2 = snap()
                var it1 = callIsland.tileRepeater.itemAt(1)
                check("mute updates in place", st.s2.c === st.s1.c && st.s2.d === st.s1.d && !!it1 && it1.modelData.muted === true,
                      (st.s2.c - st.s1.c) + " created, " + (st.s2.d - st.s1.d) + " destroyed, muted=" + (it1 ? it1.modelData.muted : "?"))
                w.re = w.muted.slice(); w.last = w.re.pop(); w.re.unshift(w.last)
                root.fakeTiles = w.re
                break
            case 3:
                st.s3 = snap()
                var it0 = callIsland.tileRepeater.itemAt(0)
                check("reorder moves, no rebuild", st.s3.c === st.s2.c && st.s3.d === st.s2.d && !!it0 && it0.objectName === w.last.id, ids())
                w.left = w.re.slice(); w.left.splice(2, 1)
                root.fakeTiles = w.left
                break
            case 4:
                st.s4 = snap()
                check("leave destroys exactly 1", st.s4.c === st.s3.c && st.s4.d - st.s3.d === 1 && callIsland.tileRepeater.count === w.left.length,
                      (st.s4.d - st.s3.d) + " destroyed, count " + callIsland.tileRepeater.count)
                // self-view rebuild: the row must be re-created (one destroy + one create)
                w.hadSelf = false
                for (var i = 0; i < w.left.length; i++) if (w.left[i].id === "vid_selfview") w.hadSelf = true
                root._rebuildSelfviewRow()
                break
            case 5:
                st.s5 = snap()
                check("self-view rebuild re-creates one tile", !w.hadSelf || (st.s5.c - st.s4.c === 1 && st.s5.d - st.s4.d === 1 && callIsland.tileRepeater.count === w.left.length),
                      (st.s5.c - st.s4.c) + " created, " + (st.s5.d - st.s4.d) + " destroyed, count " + callIsland.tileRepeater.count)
                console.log("[tilesync] VERDICT: " + (fails.length === 0 ? "PASS" : "FAIL " + fails.join("; ")))
                running = false
                Qt.quit()
                return
            }
            step++
        }
    }
    // --tinhat-pipshot (+ TINHAT_GUI_FAKECALL): pop out the first fake tile so the
    // pop-out's control strip can be screenshotted (no track behind it — black video).
    Timer {
        interval: 3500
        running: Qt.application.arguments.indexOf("--tinhat-pipshot") >= 0 && root.callShown
        onTriggered: {
            for (var i = 0; i < root.callTiles.length; ++i) {
                var t = root.callTiles[i]
                if (t.id !== "vid_selfview") {
                    root.openPip(t.id, t.participant, t.name || "", t.screenshare === true)
                    // --tinhat-pipmenu: also pop the quality menu for the screenshot
                    if (Qt.application.arguments.indexOf("--tinhat-pipmenu") >= 0 && root._pips[t.id])
                        pipMenuTimer.target = root._pips[t.id]
                    break
                }
            }
        }
    }
    Timer {
        id: pipMenuTimer
        property var target: null
        interval: 1500
        running: target !== null
        onTriggered: { if (target) target.openQualityMenu(); target = null }
    }
    // --tinhat-streampop (+ TINHAT_GUI_FAKECALL or a real call): open the stream
    // popover 4 s after the call panel is up (screenshot).
    Timer {
        interval: 4000
        running: Qt.application.arguments.indexOf("--tinhat-streampop") >= 0 && root.callShown
        onTriggered: streamPopover.open()
    }
    // --tinhat-tunein (+ TINHAT_GUI_AUTOJOIN): tune in to the first recording-only
    // share that appears (the recording-only receiver gate: hidden+muted -> tuned in).
    Timer {
        interval: 1500
        repeat: true
        running: Qt.application.arguments.indexOf("--tinhat-tunein") >= 0 && root.mInCall
        onTriggered: {
            if (root.recOnlyHiddenIds.length > 0) {
                console.log("[tunein] tuning in to", root.recOnlyHiddenIds[0])
                callctl.setTileTunedIn(root.recOnlyHiddenIds[0], true)
                running = false
            }
        }
    }
    // --tinhat-gamechat=<secs>: 6 s after joining, switch game chat ON; <secs> later
    // switch it OFF (the game-chat two-client gate reads the mute/unmute log lines).
    Timer {
        id: gcHookOn
        interval: 6000
        running: root.mInCall && root._hookArg("--tinhat-gamechat") !== null
        onTriggered: { console.log("[gamechat-hook] ON"); callctl.applyGameChat(true); gcHookOff.start() }
    }
    Timer {
        id: gcHookOff
        interval: Math.max(3000, parseInt(root._hookArg("--tinhat-gamechat") || "20") * 1000)
        onTriggered: { console.log("[gamechat-hook] OFF"); callctl.applyGameChat(false) }
    }
    // 9.4 --tinhat-pullaside=<secs>: 8 s after joining, pull the first other participant
    // aside; <secs> later leave the corner (the three-client corner gate reads the logs).
    Timer {
        id: pullHookOn
        interval: 2000
        repeat: true
        property int tries: 0
        running: root.mInCall && root._hookArg("--tinhat-pullaside") !== null
        onTriggered: {
            // From 8 s on, as soon as another participant is actually here (a peer's
            // client can take a dozen seconds to join); give up after 60 s.
            tries++
            if (tries < 4) return
            var t = root._firstOtherParticipant()
            if (t.length === 0) { if (tries > 30) { console.log("[corner-hook] PULL: nobody to pull"); stop() } return }
            console.log("[corner-hook] PULL " + t)
            console.log("[corner-hook] pull result='" + callctl.pullAside(t) + "'")
            stop()
            pullHookOff.start()
        }
    }
    Timer {
        id: pullHookOff
        interval: Math.max(3000, parseInt(root._hookArg("--tinhat-pullaside") || "20") * 1000)
        onTriggered: { console.log("[corner-hook] LEAVE"); callctl.leaveCorner() }
    }
    // 9.4 --tinhat-joincorner=<secs>: <secs> after joining, join the corner of the first
    // participant who is aside (a third client proving Join + the outsider policy).
    Timer {
        id: joinHook
        interval: 2000
        repeat: true
        property int tries: 0
        readonly property int after: Math.max(2, parseInt(root._hookArg("--tinhat-joincorner") || "14"))
        running: root.mInCall && root._hookArg("--tinhat-joincorner") !== null
        onTriggered: {
            // From <secs> on, as soon as a corner exists; give up after 60 s.
            tries++
            if (tries * 2 < after) return
            var cs = root.mCorner.corners, t = ""
            for (var id in cs) { for (var i = 0; i < cs[id].length; i++) if (root.tileMxid(cs[id][i]) !== login.userId) { t = cs[id][i]; break } if (t.length > 0) break }
            if (t.length === 0) { if (tries > 30) { console.log("[corner-hook] JOIN: no corner to join"); stop() } return }
            console.log("[corner-hook] JOIN " + t)
            console.log("[corner-hook] join result='" + callctl.joinCorner(t) + "'")
            stop()
        }
    }
    // --tinhat-listen: 25 s after joining, explicitly Listen to the first visible share;
    // 15 s later stop (the listen gate reads the streamer's watched/mute log lines).
    Timer {
        id: listenHookOn
        interval: 25000
        running: root.mInCall && root._hookArg("--tinhat-listen") !== null
        onTriggered: {
            var vis = root.visibleShareIds
            console.log("[listen-hook] visible shares:", JSON.stringify(vis), "watched:", JSON.stringify(root.watchedShareIds))
            if (vis.length > 0) { root.setListening(vis[0], true); listenHookOff.start() }
        }
    }
    Timer {
        id: listenHookOff
        interval: 15000
        onTriggered: { console.log("[listen-hook] stop listening; watched:", JSON.stringify(root.watchedShareIds)); root.listenTiles = [] }
    }
    // --tinhat-stripdump: log the side panel's call strips every second (the call-strip gate).
    Timer {
        interval: 1000; repeat: true
        running: root.mLoggedIn && root.roomShown && root._hookArg("--tinhat-stripdump") !== null
        property int _n: 0
        onTriggered: {
            _n += 1
            if (_n % 5 === 1) {
                var navCall = [], loginCall = []
                for (var a = 0; a < root.mNavRooms.length; a++) if (root.mNavRooms[a] && root.mNavRooms[a].call === true) navCall.push(root.mNavRooms[a].id)
                for (var b = 0; b < root.mRooms.length; b++) if (root.mRooms[b] && root.mRooms[b].call === true) loginCall.push(root.mRooms[b].id)
                console.log("[strip-diag] navRooms=" + root.mNavRooms.length + " navCall=" + JSON.stringify(navCall) + " loginRooms=" + root.mRooms.length + " loginCall=" + JSON.stringify(loginCall) + " navMode=" + root.navMode)
            }
            for (var i = 0; i < root.mNavRooms.length; i++) {
                var r = root.mNavRooms[i]
                if (!r || r.call !== true) continue
                var live = (root.mInCall && r.id === root.mCallRoomId) ? root.liveCallParts : (r.participants || [])
                console.log("[strip]", r.id, "parts=" + live.map(function(p) { return p.mxid }).join(","))
            }
        }
    }
    // --tinhat-gamechatdemo (+ TINHAT_GUI_FAKECALL): show the game-chat state for a screenshot.
    Timer {
        interval: 2500
        running: root.fakeCall && root._hookArg("--tinhat-gamechatdemo") !== null
        onTriggered: {
            root.mGameChat = true
            root.mGameChatPeers = { me: true, peers: ["@morgan:example.com"], muted: ["@morgan:example.com"] }
        }
    }
    // 9.4 --tinhat-cornerdemo (+ TINHAT_GUI_FAKECALL): show the aside state for a screenshot.
    Timer {
        interval: 2500
        running: root.fakeCall && root._hookArg("--tinhat-cornerdemo") !== null
        onTriggered: {
            root.mCorner = { mine: "demo", active: true, members: ["@morgan:example.com"],
                             corners: { demo: ["@morgan:example.com", login.userId + ":ME"] } }
        }
    }
    // --tinhat-paddrive=FILE (+ QML_XHR_ALLOW_FILE_READ=1; lab gates only — the Pad phase-3 gate): a gate drives the
    // REAL Pad window. FILE gets one JSON step per line: {"do": "open"[, room, device]} = the room-list bar's / banner's
    // Pad button, anything else = PadWindow.drive() (what that control's click runs). Each step and every answer is
    // logged "[paddrive] …" with this process's clock (t=ms), so the gate can time tap → ack and tap → heard.
    Timer {
        id: padDrive
        interval: 120
        repeat: true
        running: root.mLoggedIn && root._hookArg("--tinhat-paddrive") !== null
        property int done: 0
        property bool reading: false
        onTriggered: {
            if (padDrive.reading) return
            padDrive.reading = true
            var x = new XMLHttpRequest()
            x.onreadystatechange = function () {
                if (x.readyState !== XMLHttpRequest.DONE) return
                padDrive.reading = false
                var lines = String(x.responseText || "").split("\n")
                // only whole lines (the last piece may still be being written)
                for (var i = padDrive.done; i < lines.length - 1; i++) {
                    padDrive.done = i + 1
                    if (lines[i].trim().length === 0) continue
                    var step = null
                    try { step = JSON.parse(lines[i]) } catch (e) { console.log("[paddrive] #" + i + " not JSON") }
                    if (step) root._padDriveStep(i, step)
                }
            }
            x.open("GET", "file://" + String(root._hookArg("--tinhat-paddrive")))
            x.send()
        }
    }
    function _padDriveStep(n, s) {
        var r
        if (s.do === "open") {
            var order = (root.mPadTargets && root.mPadTargets.order) ? root.mPadTargets.order : []
            var room = s.room ? String(s.room) : (order.length > 0 ? String(order[0]) : "")
            var l = root.padTargetsIn(room), t = null
            for (var i = 0; i < l.length; i++) if (!s.device || l[i].device === String(s.device)) { t = l[i]; break }
            if (!t) r = "no target"
            else { root.openPad(room, t); r = t.ready ? "opened " + t.device : "greyed (" + t.why + "): " + t.device + " not opened — its line shown" }
        } else if (s.do === "targets") r = root.mPadTargets
        // the receiver's side: the Settings switches and the chip's ×, by the same calls as their handlers
        else if (s.do === "pad-allow") { padCtl.applyPadOn(s.on === true); r = "pad " + (s.on === true ? "on" : "off") }
        else if (s.do === "ptt") { callctl.applyPushToTalk(s.on === true); r = "ptt " + (s.on === true ? "on" : "off") }
        else if (s.do === "chip-x") { padCtl.refuseDevice(String(s.device || "")); r = "refused " + String(s.device || "") }
        else if (s.do === "rx") {
            var v = {}
            if (s.user) for (var k = 0, cs = ["mic", "screenshare", "music"]; k < cs.length; k++)
                v[cs[k]] = { level: parseFloat(audioPrefsCtl.volumeFor(String(s.user), cs[k])), muted: audioPrefsCtl.mutedFor(String(s.user), cs[k]) }
            r = { verified: login.verified, inCall: root.mInCall, room: root.mCallRoomId, micMuted: callctl.micMuted, ptt: callctl.pushToTalk,
                  serverMuted: callctl.serverMuted, deaf: callctl.deafened, brb: !!afkCtl && afkCtl.localAfk === true,
                  playing: soundboardPanel ? String(soundboardPanel.playingUrl || "") : "", chips: root.mPadChips, padOn: padCtl.padOn, vol: v }
        }
        else r = padWindow.drive(s)
        console.log("[paddrive] #" + n + " " + s.do + " -> " + (typeof r === "object" ? JSON.stringify(r) : String(r)) + " t=" + Date.now())
    }
    Connections {
        target: root._hookArg("--tinhat-paddrive") !== null ? padCtl : null
        ignoreUnknownSignals: true
        function onAcked(seq, ok, why, op) { console.log("[paddrive] acked seq=" + seq + " ok=" + ok + " why=" + why + " op=" + op + " t=" + Date.now()) }
        function onFailed(seq, op) { console.log("[paddrive] failed seq=" + seq + " op=" + op + " t=" + Date.now()) }
        function onEnded(why) { console.log("[paddrive] ended why=" + why + " t=" + Date.now()) }
    }
    // --tinhat-paddemo=window|pending|notice|unreachable|replaced|connecting|volume|ptt|server|chooser|chip|entry (docs/PAD.md): Pad's states for the review
    // screenshots — sample data in the Pad mirrors (AppCore.padDemo holds the real mirror back); nothing is sent.
    Timer {
        interval: 4500
        running: root.mLoggedIn && root._hookArg("--tinhat-paddemo") !== null
        onTriggered: root._padDemo(String(root._hookArg("--tinhat-paddemo") || "window"))
    }
    function _padDemo(kind) {
        var room = root.room.roomId.length > 0 ? root.room.roomId : (root.mRooms.length > 0 ? root.mRooms[0].id : "!demo:lab.test")
        var pc = "DEMOPC", phone = "DEMOPHONE"
        var mi = function (f) { return "https://sounds.example.org/" + f }
        var names = {}; names[pc] = "Desktop PC"; names[phone] = "Phone"
        var rooms = {}; rooms[room] = [{ device: pc, name: "Desktop PC", ready: true, why: "", createdTs: Date.now() }]
        if (kind === "chooser") rooms[room].push({ device: "DEMOLAPTOP", name: "Work laptop", ready: true, why: "", createdTs: Date.now() - 60000 })
        core.padDemo = true
        core.mPadTargets = { rooms: rooms, order: Object.keys(rooms), names: names }
        if (kind === "chip") {
            core.mPadChips = [{ device: phone, name: "Phone" }]
            core.notifyAction("Phone is now a Pad for this call", 8000, "Settings", function () { root.openCallSettings() })
            return
        }
        if (kind === "entry") return
        var favs = [{ name: "Bruh", sourceUrl: mi("bruh.mp3"), emoji: "😐" }, { name: "MLG Airhorn", sourceUrl: mi("airhorn.mp3"), emoji: "📯" },
                    { name: "Sad trombone", sourceUrl: mi("sadtrombone.mp3"), emoji: "🎺" }, { name: "Vine boom", sourceUrl: mi("vine-boom.mp3"), emoji: "💥" },
                    { name: "Emotional damage", sourceUrl: mi("emotional-damage.mp3"), emoji: "😭" }, { name: "Drum roll", sourceUrl: mi("drumroll.mp3"), emoji: "🥁" }]
        core.mPersonalSounds = favs
        var ptt = kind === "ptt"
        var state = { v: 1, sid: "RGVtb1NpZDAwMDAw", rseq: 42, rt: Date.now(), re: 0, ok: true, why: "",
            ops: ["sound.play", "sound.stop", "mic", "deafen", "brb", "vol", "vol.reset", "leave"],
            call: { room: room, gen: "demo1", since: Date.now() - 1800000,
                me: { mic: ptt ? "ptt" : (kind === "server" ? "server" : "on"), deaf: false, brb: false, talk: false, sound: ptt ? null : { url: mi("airhorn.mp3") },
                      gate: ptt ? { allowed: false, hint: "The soundboard is off for your role in this call." } : { allowed: true, hint: "" } },
                people: [{ u: "@morgan:lab.test", talk: true, micOff: false, away: false },
                         { u: "@riley:lab.test", talk: false, micOff: true, away: false, mute: ["music"] },
                         { u: "@sam:lab.test", talk: false, micOff: false, away: true, vol: { mic: 0.6 } }],
                more: 0,
                recent: [{ name: "Vine boom", url: mi("vine-boom.mp3"), emoji: "💥" }, { name: "Oof", url: mi("oof.mp3"), emoji: "🔊" }, { name: "Bruh", url: mi("bruh.mp3"), emoji: "😐" }] } }
        core.mPadSounds = { room: room, roomSounds: [{ name: "Victory fanfare", url: mi("fanfare.mp3"), emoji: "🏆" }, { name: "GG", url: mi("gg.mp3"), emoji: "🎮" }, { name: "Crickets", url: mi("crickets.mp3"), emoji: "🦗" }],
            people: [{ mxid: "@morgan:lab.test", name: "Morgan", avatar: "" }, { mxid: "@riley:lab.test", name: "Riley", avatar: "" }, { mxid: "@sam:lab.test", name: "Sam", avatar: "" }] }
        core.mPadRemote = { room: room, device: pc, name: "Desktop PC", sid: state.sid, reach: kind === "unreachable" ? "unreachable" : "ok", opened: true,
            ended: null, gen: "demo1", state: state, ops: state.ops, pending: [] }
        if (kind === "replaced") { core.mPadRemote.ended = "replaced"; core.mPadRemote = JSON.parse(JSON.stringify(core.mPadRemote)) }
        if (kind === "connecting") core.mPadRemote = { room: room, device: pc, sid: state.sid, reach: "connecting", opened: false, ended: null, gen: "", state: null, ops: [], pending: [] }
        var opt = kind === "pending" ? { mic: { v: true, seq: 43, at: Date.now() + 1e9 }, sound: { v: mi("vine-boom.mp3"), seq: 44, at: Date.now() + 1e9 } } : ({})
        padWindow.demo(room, pc, opt)
        if (kind === "volume") Qt.callLater(function () { padWindow.demoVolume(2) })
        if (kind === "notice") padWindow.demoNotice("unknown-sound", "sound.play")
    }
    // --tinhat-soundboard: open the soundboard window 3 s after login (screenshot).
    Timer {
        interval: 3000
        running: Qt.application.arguments.indexOf("--tinhat-soundboard") >= 0 && root.mLoggedIn && root.roomShown
        onTriggered: {
            soundboardCtl.refresh(room.roomId); soundboardPanel.openOrRaise()
            if (root.fakeCall) { // screenshot data (a fake call has no real sounds)
                soundboardPanel.personalSounds = [
                    { name: "Bruh", sourceUrl: "https://www.myinstants.com/media/sounds/bruh.mp3", emoji: "😐" },
                    { name: "MLG Airhorn", sourceUrl: "https://www.myinstants.com/media/sounds/airhorn.mp3", emoji: "📯" },
                    { name: "Sad trombone", sourceUrl: "https://www.myinstants.com/media/sounds/sadtrombone.mp3", emoji: "🎺" },
                    { name: "Vine boom", sourceUrl: "https://www.myinstants.com/media/sounds/vine-boom.mp3", emoji: "💥" } ]
                soundboardPanel.roomSounds = [
                    { name: "Emotional damage", sourceUrl: "https://www.myinstants.com/media/sounds/emotional-damage.mp3", emoji: "😭", setBy: "@x:y" },
                    { name: "Discord call", sourceUrl: "https://www.myinstants.com/media/sounds/discord-call.mp3", emoji: "📞", setBy: "@x:y" } ]
                soundboardPanel.playingUrl = "https://www.myinstants.com/media/sounds/airhorn.mp3"
                soundboardPanel.playingName = "MLG Airhorn"
                soundboardPanel.playStartedAt = Date.now() + 1e9
            }
        }
    }
    // --tinhat-sbpill (+ TINHAT_GUI_FAKECALL): pretend a clip is playing so the call
    // bar's now-playing pill renders for a screenshot.
    Timer {
        interval: 3000
        running: Qt.application.arguments.indexOf("--tinhat-sbpill") >= 0 && root.callShown
        onTriggered: { soundboardPanel.playingName = "Bruh sound effect #2"; soundboardPanel.playingUrl = "gate://pill"; soundboardPanel.playStartedAt = Date.now() + 1e9 }
    }
    // 11.3 (Media Party QOL) --tinhat-mpstrip (+ TINHAT_GUI_FAKECALL): a fake call has no call room — watch the OPEN
    // room's party so the call bar's Media Party strip (▶/⏸ · next · the "title — artist" tooltip) renders for a shot.
    Timer {
        interval: 3000
        running: Qt.application.arguments.indexOf("--tinhat-mpstrip") >= 0 && root.callShown && root.mLoggedIn && root.roomShown
        onTriggered: watchCtl.watchRoom(room.roomId)
    }
    // --tinhat-memories: fetch + open the Memories window for the open room
    // (0.8.8 gate hook — the room menu has no stable coordinates to click).
    Timer {
        interval: 2500
        running: Qt.application.arguments.indexOf("--tinhat-memories") >= 0 && root.mLoggedIn && root.roomShown
        onTriggered: {
            memoriesWindow.roomId = room.roomId; memoriesWindow.roomName = room.roomName
            room.fetchMemories(); room.fetchClips()
            memoriesWindow.show(); memoriesWindow.raise()
        }
    }
    // --tinhat-gearmenu (+ TINHAT_GUI_AUTOJOIN): pops the call & device menu once
    // in-call, for screenshot verification.
    Timer {
        interval: 6000
        running: Qt.application.arguments.indexOf("--tinhat-gearmenu") >= 0 && root.mInCall
        onTriggered: { root.refreshDeviceMenus(); devicesMenu.popup() }
    }
    // --tinhat-healthpopup (+ TINHAT_GUI_AUTOJOIN): opens the Stream health popup
    // once in-call, for screenshot verification. 12s so the first 3s stats tick
    // (and usually the SFU's first receiver report) has landed and the cards show
    // real numbers rather than the empty state.
    Timer {
        interval: 12000
        running: Qt.application.arguments.indexOf("--tinhat-healthpopup") >= 0 && root.mInCall
        onTriggered: callIsland.statsPopup.open()
    }
    // --tinhat-popout (+ TINHAT_GUI_AUTOJOIN + a TINHAT_VIDPUB helper): pops out the
    // first REMOTE video tile into the PiP window, then closes it 25s later —
    // screenshot verification of the PiP CPU viewer ("[pip] first frame PAINTED"
    // in the log) AND that the grid tile keeps playing throughout (it is never
    // detached — the PiP is an independent viewer). Retries until a tile exists.
    Timer {
        id: pipTestOpen
        interval: 2000
        repeat: true
        running: Qt.application.arguments.indexOf("--tinhat-popout") >= 0 && root.mInCall
        onTriggered: {
            for (var i = 0; i < root.mVideoTiles.length; ++i) {
                var t = root.mVideoTiles[i]
                if (t.id !== "vid_selfview" && !root._pips[t.id]) {
                    console.log("[pip-test] popping out", t.id, t.participant)
                    root.openPip(t.id, t.participant, t.name || "", t.screenshare === true)
                    running = false
                    pipTestClose.tileId = t.id
                    pipTestClose.start()
                    return
                }
            }
        }
    }
    Timer {
        id: pipTestClose
        property string tileId: ""
        interval: 25000
        onTriggered: {
            if (root._pips[tileId]) {
                console.log("[pip-test] closing pop-out", tileId)
                root._pips[tileId].close()
            }
        }
    }

    // ======================= The shell page: sign-in, then the dock host =====================
    // 11.0 (ISLANDS-ARCH §2.3, islands C5): the page row keeps this ONE page and never pushes. Signed out it shows the
    // sign-in card; signed in, the dock host places the islands — until C6 the two compat islands of `classic-flush`
    // (stage 1: `_nav` = the nav column, `_room` = the room page; the pre-islands geometry, gated at 0 px against B2,
    // deleted with every flush-only path in C7).
    pageStack.initialPage: Kirigami.Page {
        id: shellPage
        objectName: "shellPage"
        title: root.mLoggedIn ? "" : "Sign in"
        padding: 0
        // Hide the empty global toolbar header (it would reserve a blank strip above everything).
        globalToolBarStyle: Kirigami.ApplicationHeaderStyle.None
        Kirigami.ColumnView.fillWidth: true
        // The page row's leftovers, switched off (§2.3): a one-page row stays touch-draggable (a handheld) and keeps an
        // empty global toolbar unless told otherwise; its Back/Forward Shortcuts are pre-empted by the window event
        // filter (thBack / thForward below).
        Component.onCompleted: {
            root.pageStack.interactive = false
            root.pageStack.columnView.columnResizeMode = Kirigami.ColumnView.SingleColumn
            root.pageStack.globalToolBar.style = Kirigami.ApplicationHeaderStyle.None
        }

        StackLayout {
            id: mainStack
            anchors.fill: parent
            currentIndex: root.mLoggedIn ? 1 : 0

            // ----------------------- index 0: LOGIN VIEW -----------------------
            // The polished auth card (LoginView): soft-shadowed card, warm-focus
            // fields, a glowing primary button; Furry layers on the aurora splash,
            // a floof-corner mascot and a pride-strip footer. All controller state
            // is pushed in via plain props; intent comes back as signals.
            LoginView {
                id: loginPage
                appIconSource: root.appIconUrl   // 0.8.26: bundled app icon on the sign-in card
                app: root
                autoLogin: login.autoLogin
                baseFontFamily: root.baseFontFamily
                busy: root.mBusy
                status: root.mStatus
                homeserver: login.homeserver
                movedNote: login.movedNote
                movedTo: login.movedTo
                resetStage: root.mResetStage
                resetStatus: root.mResetStatus
                resetMode: root.mResetMode
                resetNote: root.mResetNote
                onHomeserverEdited: (value) => { login.homeserver = value; movedCheck.restart() }
                // the moved server's new address, on the person's click (bridge.rs check_moved no longer switches it)
                onUseMovedServer: () => { if (String(login.movedTo).length > 0) { login.homeserver = login.movedTo; movedCheck.restart() } }
                // moved.rs: does the server say it moved to its own domain? At start, each time the card shows (a move
                // ends every session; logout() puts the server you were on in the field) and once typing stops, as
                // Tin Hat Web's LoginView (on mount and on each edit). Signed in, the note goes (the web view unmounts).
                Timer { id: movedCheck; interval: 600; onTriggered: login.checkMoved(login.homeserver) }
                Component.onCompleted: login.checkMoved(login.homeserver)
                onVisibleChanged: {
                    if (visible) {
                        login.checkMoved(login.homeserver)
                    } else {
                        login.movedNote = ""
                        login.movedTo = ""
                    }
                }
                onSubmit: (username, password) => { login.login(username, password) }
                onResetRequested: (email) => { login.requestPasswordReset(email) }
                onResetCompleted: (newPassword) => { login.completePasswordReset(newPassword) }
                onResetCancelled: () => { login.cancelPasswordReset() }
                // selfhost (password.rs): does the typed server mail a link, or does its owner set new passwords?
                onResetCheck: (server) => { login.checkPasswordReset(server) }
                // 10.4: create an account with an invite code (docs/INVITES.md)
                registerStage: root.mRegisterStage
                registerStatus: root.mRegisterStatus
                onCreateAccount: (code, username, password, displayName) => { login.createAccount(username, password, code, displayName) }
                onCreateCancelled: () => { login.cancelCreateAccount() }
            }

            // ----------------------- index 1: THE DOCK HOST ---------------------
            // 11.0 (ISLANDS-ARCH §3.2, islands C6/C7): the Classic preset — every part of the window its own island (the
            // theme's shape: radius, gap, margin, frame), placed by the layout engine (crates/layout; the web runs the
            // same solver, byte for byte). The nav column's islands read their state from navState (ThNav, zero-size,
            // at the window root); the room's islands are always there and leave the layout while no room is open.
            // 11.0 edit mode (EDITMODE-UI.md §3): while editing, the toolbar takes a 52 px strip above the dock host
            Item {
            id: shellBody
            Loader {
                id: editToolbarLoader
                active: editModeObj.open
                z: 100
                width: parent.width
                height: 52
                sourceComponent: Component {
                    EditToolbar {
                        em: editModeObj
                        host: dockHost
                        overlay: shellBody
                        radius: dockHost.islandRadius
                        editLayer: editLayerLoader.item
                    }
                }
            }
            DockHost {
                id: dockHost
                anchors.fill: parent
                anchors.topMargin: editModeObj.open ? 52 : 0
                layoutCtl: layoutCtl
                formJson: layoutStoreObj.formJson   // 11.0: what this device renders (LayoutStore: the synced layout, the reset override, the pref form, Classic) — edit mode's working copy while editing
                view: root.dockView
                shape: root.dockShape
                unitPm: Math.round(Kirigami.Units.gridUnit * 1000 / 18)   // layout lengths scale with gridUnit (18 = 1000)
                editing: editModeObj.open
                tabLabel: function (ref) { return editModeObj.islandName(ref) }
                absentRefs: root.editAbsent
                placeholderTextOf: function (ref) { return editModeObj.textOf(ref) }
                onTabPicked: (stackId, ref) => root.setLayoutTab(stackId, ref)
                onBackgroundMenu: (x, y) => { if (root.mLoggedIn) gapMenu.popup(dockHost, x, y) }
                // edit mode shows the drawer of a collapsed column open and pinned (EDITMODE-UI §2)
                drawerOpacity: editModeObj.open ? 1 : cornerNav.panelItem.opacity
                drawerShift: editModeObj.open ? 0 : cornerNav.panelShift
                drawerHandle: cornerNav.handleItem   // 10.8: a drawer that closes around the focus hands it back here
                // Escape after everything inside the islands had its turn (§2.8 steps 6-7: an overlaid panel, an open
                // unpinned drawer). Popups, the composer and "fill the app" keep their own Escape (steps 2-5).
                Keys.onEscapePressed: (event) => { event.accepted = root.thEscapeLayout() }

                // ---- the nav column: the rail's three islands, the rooms, the widgets, you ----
                Island {
                    id: navIsl
                    ref: "nav"
                    fill: Th.islandBgNav
                    // 11.0 (the Side monitor preset): in a row it reports its natural width (the orientH pref is auto)
                    extra: root.railH("nav") ? ({ w: navContent.implicitWidth + 2 * navIsl.inset }) : ({})
                    NavIsland { id: navContent; anchors.fill: parent; app: root; railRef: navState.railState; horizontal: root.railH("nav") }
                }
                Island {
                    id: dmsIsl
                    ref: "dms"
                    fill: Th.islandBgDms
                    empty: navState.railState.dmCount === 0
                    extra: root.railH("dms") ? ({ w: dmsContent.implicitWidth + 2 * dmsIsl.inset }) : ({})
                    DmsIsland { id: dmsContent; anchors.fill: parent; app: root; dmMenuRef: navState.railState.dmMenuObj; dmModelRef: navState.railState.dmModelObj; railRef: navState.railState; horizontal: root.railH("dms") }
                }
                Island {
                    ref: "spaces"
                    fill: Th.islandBgSpaces
                    reports: false
                    SpacesIsland { anchors.fill: parent; app: root; dmModelRef: navState.railState.dmModelObj; railRef: navState.railState; realModelRef: navState.railState.realModelObj; horizontal: root.railH("spaces") }
                }
                Island {
                    ref: "rooms"
                    fill: Th.islandBgRooms
                    reports: false
                    ColumnLayout {
                        anchors.fill: parent
                        spacing: 0
                        // 11.0 (GPU.md §2.5): a still busy mark that steps on each sign-in stage (the status line); the
                        // caption changes once after 10 s without one ("Still loading your rooms…").
                        ThBusyIndicator {
                            id: bootMark
                            Layout.alignment: Qt.AlignHCenter
                            Layout.topMargin: Kirigami.Units.largeSpacing
                            size: 32
                            visible: root.mBusy
                            running: root.mBusy
                            progress: root.mStatus
                            label: root.mLoggedIn ? "Loading your rooms…" : "Signing in…"
                        }
                        Controls.Label { textFormat: Text.PlainText;   // 10.1 (TH-MOTION-14): a spinner with a caption
                            Layout.alignment: Qt.AlignHCenter
                            visible: root.mBusy
                            text: bootMark.shownLabel
                            color: Th.roomsLoadingFg
                            font.pointSize: Kirigami.Theme.smallFont.pointSize
                        }
                        RoomsIsland {
                            id: roomsIsl
                            Layout.fillWidth: true
                            Layout.fillHeight: true
                            app: root
                            panelRef: navState.listState
                            roomModelRef: navState.listState.roomModelObj
                            sharedVolMenuRef: navState.listState.sharedVolMenuObj
                        }
                    }
                }
                Island {
                    id: widgetsIsl
                    ref: "widgets"
                    flush: true   // every widget draws its own card
                    empty: !widgetStack.shown
                    extra: ({ minH: widgetStack.foldedTotal })
                    WidgetHost {
                        id: widgetStack
                        anchors.fill: parent
                        app: root
                        islandMode: true
                        maxHeight: widgetsIsl.height > 0 ? widgetsIsl.height : 100000
                    }
                }
                Island {
                    ref: "account"
                    fill: root.flatLayout ? Th.islandBgRooms : Th.islandBgAccount
                    Item {
                        anchors.fill: parent
                        // Flat: the account card sits in the room column as it did before islands (6 px in, 9 px up)
                        implicitHeight: accountIsl.implicitHeight + (root.flatLayout ? 6 + 9 : 0)
                        implicitWidth: accountIsl.implicitWidth
                        AccountIsland {
                            id: accountIsl
                            anchors.fill: parent
                            anchors.leftMargin: root.flatLayout ? 6 : 0
                            anchors.rightMargin: root.flatLayout ? 6 : 0
                            anchors.topMargin: root.flatLayout ? 6 : 0
                            anchors.bottomMargin: root.flatLayout ? 9 : 0
                            app: root
                            framed: root.flatLayout
                            panelRef: navState.listState
                            statusPopRef: navState.listState.statusPopObj
                        }
                    }
                }

                // ---- the room: header · call · timeline · composer · panel ----
                Island {
                    ref: "header"
                    fill: Th.islandBgHeader
                    contentInset: 0   // the header's gradient rounds itself (bgRadius)
                    HeaderIsland { anchors.fill: parent; bgRadius: dockHost.islandRadius; app: root; calendarWindowRef: calendarWindow; devicesMenuRef: devicesMenu; gameReelWindowRef: gameReelWindow; memoriesWindowRef: memoriesWindow; rightPanelRef: rightPanel; watchPartyWindowRef: watchPartyWindow }
                }
                Island {
                    id: callFrame
                    ref: "call"
                    fill: Th.islandBgCall
                    // 10.8: the island measures what the call WANTS (naturalH); the call fills what it gets
                    Item {
                        anchors.fill: parent
                        implicitHeight: callIsland.naturalH
                        CallIsland { id: callIsland; anchors.fill: parent; app: root; callOptionsRef: callOptions; coreRef: core; devicesMenuRef: devicesMenu; echoCheckTimerRef: echoCheckTimer; recordConfirmRef: recordConfirm; pageH: root.callPageH; streamPopoverRef: streamPopover
                            leftInset: Math.max(0, root.callInset - callFrame.inset) }   // 10.8: clear of the emergency drawer's handle
                    }
                }
                Island {
                    ref: "timeline"
                    fill: Th.islandBgTimeline
                    reports: false
                    Item {
                        anchors.fill: parent
                    // 9.3: THE conversation — the same component the floating chat
                    // bubbles mount. Legacy carried its own ~900-line copy of the timeline + composer
                    // here; a fix landing in one copy and not the other was how two earlier fixes shipped
                    // half-fixed. `primary: true` = this is the room's one read-marking view.
                    ConversationView {
                        id: conversation
                        anchors.fill: parent
                        visible: root.roomShown
                        // 11.0 (islands C6/C7): the composer section lives in the composer island (Flat: the box draws its own card)
                        composerHost: composerHost
                        boxCard: root.flatLayout
                        app: root
                        room: root.room
                        gif: root.gifCtl
                        primary: true
                        showSoundboard: true
                        displayName: root.mRoomName
                        // "Fill the app" hides the conversation so the call owns the whole content area.
                        hidden: root.callMaximized && root.callShown
                        onAvatarActivated: (mxid) => { root.profileMxid = mxid; profileFlyout.open() }
                        Component.onCompleted: root.conversationView = conversation
                        Component.onDestruction: if (root.conversationView === conversation) root.conversationView = null
                    }
                        // 10.1 (TH-CONV-03): before any room is open the timeline island says where to start; with the nav
                        // in the drawer (corner mode, a narrow window) it points at the handle instead.
                        EmptyState {
                            anchors.centerIn: parent
                            width: Math.min(parent.width - Kirigami.Units.gridUnit * 4, Kirigami.Units.gridUnit * 28)
                            visible: root.mLoggedIn && !root.roomShown && !root.mBusy
                            app: root
                            iconName: root.navInDrawer ? "documentinfo-symbolic" : "dialog-messages-symbolic"
                            title: root.navInDrawer ? "Your rooms are tucked away" : "Pick a room to start"
                            explanation: root.navInDrawer ? "Hover the handle in the top-left corner to open Spaces and rooms."
                                                          : "Your rooms are on the left. Ctrl+K jumps to any of them, Alt+↑/↓ steps through the list."
                        }
                    }
                }
                Island {
                    ref: "composer"
                    minIsNatural: true   // 10.8: a reply bar + several lines keep their height under a call (the call gives way)
                    fill: root.flatLayout ? Th.islandBgTimeline : Th.islandBgComposer
                    borderColor: conversation.composerFocused ? Th.composerBorderFocus : Th.islandBorder
                    Item { id: composerHost; anchors.fill: parent; implicitHeight: conversation.composerImplicitHeight }
                }
                Island {
                    ref: "panel"
                    fill: Th.islandBgPanel
                    reports: false
                    // 10.1 (TH-CONV-08): room info / members — a docked column (was a modal OverlayDrawer
                    // that dimmed the room and blocked typing). Opened from the header's info button.
                    RightPanel {
                        id: rightPanel
                        anchors.fill: parent
                        framed: root.flatLayout   // Flat: the panel draws its own fill and left edge, as before islands
                        // docked beside the conversation, or overlaying it in a narrow window: the solver decides (the collapse
                        // chain's panel → overlay step puts it at z 2)
                        readonly property bool docked: !(dockHost.lastOut && dockHost.lastOut.islands.panel && (dockHost.lastOut.islands.panel.z || 0) > 0)
                        Component.onCompleted: root.roomInfoPanel = rightPanel
                        Component.onDestruction: if (root.roomInfoPanel === rightPanel) root.roomInfoPanel = null
                        app: root
                        members: root.mMembers
                        summary: root.mRoomSummary
                        roomId: room.roomId
                        activity: root.mActivity
                        statuses: root.mStatuses
                        myStatus: root.mMyStatus
                        statusIsDm: root.mStatusIsDm
                        // Moderation/settings permissions (gate the edit/kick/ban affordances).
                        myPower: root.mMyPower
                        canChangeName: root.mCanChangeName
                        canChangeTopic: root.mCanChangeTopic
                        canInvite: root.mCanInvite
                        canKick: root.mCanKick
                        canBan: root.mCanBan
                        canChangePower: root.mCanChangePower
                        onAvatarClicked: (mxid) => { root.profileMxid = mxid; profileFlyout.open() }
                        onSetActivity: (game) => activityCtl.setActivity(game, "", (root.mActivity && root.mActivity.game === game) ? String(root.mActivity.code || "") : "")
                        onPostStatus: (text, ttlMin) => statusCtl.postStatus(text, ttlMin)
                        // Room-settings edits + moderation -> RoomController invokables.
                        onRenameRoom: (name) => room.applyRoomName(name)
                        onEditTopic: (topic) => room.applyRoomTopic(topic)
                        onInviteUser: (mxid) => room.inviteUser(mxid)
                        onKickUser: (mxid, reason) => room.kickUser(mxid, reason)
                        onBanUser: (mxid, reason) => room.banUser(mxid, reason)
                        onUnbanUser: (mxid) => room.unbanUser(mxid, "")   // 10.1 (TH-RP-03)
                        onSetMemberPower: (mxid, power) => room.setMemberPower(mxid, power)
                        cardJson: settings.cardJson
                        onRequestProfileCard: (mxid) => settings.fetchProfileCard(mxid)
                        onPlayJoinSound: (url) => soundboardCtl.playSound(room.roomId, url)
                        // RBAC §7.5: the room's space → gear on the Members tab + "Manage roles…" in member menus.
                        inSpace: root.mRoomSpaceId.length > 0
                        onOpenSpaceSettings: root.openSpaceSettings(root.mRoomSpaceId, "overview")
                        onManageRoles: (mxid) => root.openSpaceSettings(root.mRoomSpaceId, "members")
                    }
                }

                // ---- 11.0 buttons: your own button islands (`buttons:<n>` leaves of this device's form — LayoutStore;
                // the web's ButtonsIsland). Children of the host like every island (the Repeater parents its delegates
                // here): the host finds them by objectName. They report their natural width (the buttons on one line —
                // a floating island is as wide as they are) through `extra.w`, the height through the content.
                Repeater {
                    model: layoutStoreObj.buttonIslands
                    delegate: Island {
                        id: bIsland
                        required property string modelData
                        ref: modelData
                        fill: Th.islandBgButtons
                        extra: ({ w: bContent.oneLineWidth + 2 * bIsland.inset })
                        ButtonsIsland {
                            id: bContent
                            anchors.fill: parent
                            app: root
                            islandRef: bIsland.modelData
                        }
                    }
                }

                // ---- the nav drawer's chrome (corner mode, or a window narrower than 720): ThDock's handle, panel frame
                // and hover intent (110 ms open / 280 ms close, the near-top reveal zone, pin, badge) at the solver's
                // rects, under the drawer's islands (z 20) — for OLED users who don't want a static sidebar (burn-in).
                ThDock {
                    id: cornerNav
                    z: 19
                    app: root
                    side: "left"
                    handleIcon: "application-menu-symbolic"
                    ariaLabel: "Spaces & rooms"
                    visible: root.mLoggedIn && root.navInDrawer && !(root.callMaximized && root.callShown)
                    panelWidth: 340
                    panelHeight: Math.min(680, root.height * 0.82)
                    handleRect: root._drawerRect("handle")
                    panelRect: root._drawerRect("panel")
                    islandHovered: dockHost.drawerIslandHovered
                    badgeText: root.mNavBadge
                    onOpenedChanged: if (opened) nav.refresh()
                    // 10.1 (TH-NAV-17): refresh as the pointer approaches the corner, so the panel that
                    // slides in 110 ms later already shows the current list (not the one from last time).
                    onNearTopChanged: if (nearTop && !opened) nav.refresh()
                    onHandleHoverChanged: if (handleHover && !opened) nav.refresh()
                }

                // ---- 11.0 edit mode's chrome (EditLayer.qml): only while editing, above every island ----
                Loader {
                    id: editLayerLoader
                    active: editModeObj.open
                    z: 40
                    anchors.fill: parent
                    sourceComponent: Component {
                        EditLayer {
                            em: editModeObj
                            host: dockHost
                            toolbar: editToolbarLoader.item
                        }
                    }
                }
            }
            }
        }
    }

    // 11.0 (islands C6/C7): the nav column's STATE — the rail's models and menus, the room list's model, filters and the
    // status popover. Its islands live in the dock host; this draws nothing (zero size) and keeps the nav's handlers.
    ThNav {
        id: navState
        width: 0
        height: 0
        roomsIslandItem: roomsIsl
        footerCardItem: accountIsl
        app: root
        navController: nav
        audioPrefs: audioPrefsCtl
        currentRoomId: room.roomId
        openRoomId: room.roomId
        sidebarCollapsed: root.sidebarCollapsed
        onRoomActivated: (roomId) => { root.openRoomId(roomId); if (root.navInDrawer) cornerNav.close() }
        onRoomContext: (roomId) => root.openRoomContext(roomId)
        onMessageUser: (mxid) => login.createDm(mxid)
        onJoinRequested: (roomId, roomName) => root.askJoinRoom(roomId, roomName)
        myMxid: login.userId
        onAcceptInvite: (roomId) => login.acceptInvite(roomId)
        onDeclineInvite: login.rejectInvite(roomId)
        onMenuRequested: (anchorItem) => appMenu.popup(anchorItem, 0, anchorItem.height)
        onNewMenuRequested: (anchorItem) => newMenu.popup(anchorItem, 0, anchorItem.height)   // 2026-09-29
        onAddSpaceRequested: joinOrCreateDialog.open()
        onSpaceMenuRequested: (key, label) => root.openSpaceContext(key, label)   // 0.8.27
        onSpaceInviteRequested: (key, join) => { if (join) login.acceptInvite(key); else login.rejectInvite(key) }   // RBAC §7.3
        onSpaceSettingsRequested: (key) => root.openSpaceSettings(key, "overview")   // RBAC §7.1
        onSetActivity: (game) => activityCtl.setActivity(game, "", (root.mActivity && root.mActivity.game === game) ? String(root.mActivity.code || "") : "")
        onPostStatus: (text, ttlMin) => statusCtl.postStatus(text, ttlMin)
        onToggleNavRequested: root.toggleSidebar()
        onBugReportRequested: { if (!bugReportDialog.keepNote) bugReportDialog.clearForm(); bugReportDialog.open(); if (root.navInDrawer) cornerNav.close() }
        onDmOpenRequested: (roomId) => { root.openDmFromRail(roomId); if (root.navInDrawer) cornerNav.close() }                        // 10.6 (REACH §1.3)
        onDmInviteRequested: (roomId, accept) => { if (accept) login.acceptInvite(roomId); else login.rejectInvite(roomId) }
        onPrivacySettingsRequested: { if (root.navInDrawer) cornerNav.close(); settingsWindow.openAt("privacy") }       // 10.6 (REACH §2.7)
        onProfileSettingsRequested: { if (root.navInDrawer) cornerNav.close(); settingsWindow.openAt("profile") }       // 2026-09-29: the status popover's "Profile settings…"
    }

    // 10.6 (REACH §1.3): a DM bubble on the rail — the DM opens with PEOPLE selected, so the room
    // list shows your DMs with this one current. openRoomId() may first follow the room to its
    // owning space (Home, for a DM that is not in the list on screen); People is selected after.
    function openDmFromRail(id) {
        if (!id || id.length === 0) return
        root.openRoomId(id)
        if (nav.activeSpace !== "people") nav.selectSpace("people")
    }

    // Ctrl+K on a pending invite (ACTIONS.md §14.3; the web's QuickSwitcher.tsx showInvite takes the same steps): the
    // invite shows where it waits with Accept / Decline — its room-list row; nothing is accepted or declined here. The
    // rail goes to People (a DM invite) or Home (a room invite), as a clicked invite notification does, unless the list
    // on screen already holds the row (openRoomId's TH-NAV-02 rule); a tucked nav's drawer opens; the row scrolls into
    // view once the list holds it.
    function showInvite(id) {
        if (!id || id.length === 0 || !root.mLoggedIn) return
        var here = (root.mNavRooms || []).some(function (r) { return !!r && r.id === id })
        if (!here) {
            var all = root.mRooms || [], dm = false
            for (var i = 0; i < all.length; i++) if (all[i] && all[i].id === id) { dm = all[i].dm === true; break }
            var key = dm ? "people" : "home"
            if (nav.activeSpace !== key) nav.selectSpace(key)
        }
        if (cornerNav.visible && !cornerNav.opened) { nav.refresh(); cornerNav.opened = true }
        roomsIsl.revealRoom(id)
    }

    // ---- 11.0 islands (ISLANDS-ARCH §2.3, §3.2, C6/C7): the dock host's inputs ----
    LayoutController {
        id: layoutCtl
        Component.onCompleted: layoutCtl.startSync()   // 11.0: storeRev follows the synced layouts (layout_store.rs)
        // a save of this device that another device's newer save replaced (EDITMODE-UI §13 / §16)
        onStoreRevChanged: {
            var lost = []
            try { lost = JSON.parse(String(layoutCtl.storeTakeLost())) } catch (e) { lost = [] }
            if (lost.indexOf(layoutStoreObj.activeId) >= 0)
                root.showPassiveNotification("Your layout change was replaced by a newer one from another device.", 8000)
        }
    }
    // 11.0 edit mode (EditMode.qml — the session, a move in progress, the actions; EditLayer / EditToolbar draw it)
    EditMode {
        id: editModeObj
        app: root
        layoutCtl: layoutCtl
        store: layoutStoreObj
        host: dockHost
        // leaving edit mode hands the keyboard back to where people type (the chrome that had it is gone)
        onClosed: Qt.callLater(function () { if (root.roomShown && root.conversationView) root.conversationView.focusComposer() })
    }
    readonly property alias editMode: editModeObj
    /** Edit layout (Ctrl+E, the ☰ menu, a gap's menu, Settings → Appearance → Layout): open — or, open, Done. */
    function openEditLayout(fromSettings) {
        if (fromSettings) settingsWindow.close()
        if (!root.mLoggedIn) return
        if (editModeObj.open) { editModeObj.done(); return }
        cornerNav.close()
        editModeObj.openEdit()
    }
    /** Reset layout on this device (EDITMODE-UI §9): the device-local override, at once. */
    function resetLayoutHere() {
        if (editModeObj.open) editModeObj.cancel()
        layoutStoreObj.setOverride()
        root.settingsToast("Layout reset on this device.", 3000)
    }
    Shortcut {   // EDITMODE-UI §1: layout.edit (Ctrl+E) opens edit mode (works while typing, with every button hidden); editing = Done
        sequences: actionsHost.keysFor("layout.edit")
        enabled: root.mLoggedIn
        onActivated: root.openEditLayout(false)
    }
    // the way in that always works (EDITMODE-UI §10): right-click (a long press) on a gap / the margin / the host
    Controls.Menu {
        id: gapMenu
        objectName: "gapMenu"
        modal: false
        Controls.MenuItem { text: "Edit layout"; icon.name: "window-symbolic"; onTriggered: root.openEditLayout(false) }
        Controls.MenuItem { text: "Settings"; icon.name: "settings-configure"; onTriggered: settingsSheet.open() }
        Controls.MenuSeparator {}
        Controls.MenuItem { text: "Reset layout on this device"; icon.name: "view-refresh-symbolic"; onTriggered: root.resetLayoutHere() }
    }
    // gate hooks: --reset-layout (the reset path that needs no keyboard at start-up, ISLANDS-ARCH §7), --tinhat-editmode
    // (captures: edit mode opens after the first layout), --tinhat-editpreset=<name> (then that preset is picked)
    Timer {
        id: editHook
        interval: 2500
        running: root.mLoggedIn && Qt.application.arguments.indexOf("--tinhat-editmode") >= 0
        onTriggered: {
            editModeObj.openEdit()
            var args = Qt.application.arguments
            for (var i = 0; i < args.length; i++) {
                var a = String(args[i])
                if (a.indexOf("--tinhat-editpreset=") === 0) editModeObj.preset(a.substring("--tinhat-editpreset=".length))
                if (a.indexOf("--tinhat-editop=") === 0) editModeObj.apply(JSON.parse(a.substring("--tinhat-editop=".length)), true)
                if (a.indexOf("--tinhat-editpanel=") === 0) editHook2.panel = a.substring("--tinhat-editpanel=".length)
                if (a.indexOf("--tinhat-editmenu=") === 0) editHook2.menu = a.substring("--tinhat-editmenu=".length)
                if (a.indexOf("--tinhat-editmove=") === 0) editHook2.move = a.substring("--tinhat-editmove=".length)
            }
            if (editHook2.panel.length + editHook2.menu.length + editHook2.move.length > 0) editHook2.start()
            if (args.indexOf("--tinhat-editdone") >= 0) editModeObj.done()   // the sync check: the ops above, saved
        }
    }
    Timer {   // the second step of the capture hooks, once the chrome exists: a toolbar panel, an island menu, a keyboard move
        id: editHook2
        interval: 800
        property string panel: ""   // presets | add | confirm
        property string menu: ""    // an island ref: its ⋯ menu
        property string move: ""    // "<ref>:<zone index>": a keyboard move hovering that zone (the zone + landing preview)
        onTriggered: {
            if (editHook2.panel.length > 0 && editToolbarLoader.item) editToolbarLoader.item.panel = editHook2.panel
            var L = editLayerLoader.item
            if (editHook2.menu.length > 0 && L && L._chips[editHook2.menu]) L.openMenu(editHook2.menu, L._chips[editHook2.menu], 0, 28)
            if (editHook2.move.length > 0) {
                var k = editHook2.move.lastIndexOf(":")
                var ref = editHook2.move.substring(0, k)
                if (L && L._chips[ref]) L._chips[ref].forceActiveFocus()
                if (editModeObj.beginDrag(ref, "keys", 0, 0)) editModeObj.setHover(parseInt(editHook2.move.substring(k + 1)))
            }
        }
    }
    QtObject {
        Component.onCompleted: {
            if (Qt.application.arguments.indexOf("--reset-layout") >= 0) {
                layoutStoreObj.setOverride()
                console.log("[editmode] --reset-layout: this device renders Classic until you keep it or restore")
            }
        }
    }
    // the Classic preset (x-tinhat.presets.classic); named layouts and edit mode come with step 4
    readonly property string _classicFormJson: JSON.stringify(JSON.parse(layoutCtl.preset("classic")).form)
    // 11.0 buttons: this device's desktop form (the pref layout-desktop, repaired; the Classic preset when none) — the
    // button rows (opts.<island>.rows.<row>) and your own button islands; edit mode (step 4) moves it into the synced
    // layout document. RowModel.qml reads the rows through `layoutStore`.
    LayoutStore { id: layoutStoreObj; app: root; layoutCtl: layoutCtl; editMode: editModeObj }
    readonly property alias layoutStore: layoutStoreObj
    property bool roomShown: false   // a room is open: openRoomId shows it; sign-out / leaving the open room close it
    // the view state (device-local, §6.8): corner mode = the `nav_mode` pref (read through here until C9 stores it in the
    // view state), the drawer open while its panel is on screen (open, or fading out), "fill the app" = the call maximised
    // 11.0 edit mode (EDITMODE-UI §2): every placed island is present — a room, the call and the panel as if open
    // (placeholders where the real content is absent), the collapsed column's drawer open — and nothing is maximised;
    // a stack's shown tab = the device-local pref `layout-tabs` (§4.4)
    readonly property var dockView: {
        var v = editModeObj.open ? ({
            roomOpen: true,
            callShown: true,
            panelOpen: true,
            maximized: null,
            forceCollapse: root.navMode === "corner" ? ["navcol"] : [],
            drawer: { open: root.navDrawerId }
        }) : ({
            roomOpen: root.roomShown,
            callShown: root.roomShown && root.callShown,
            panelOpen: root.roomShown && rightPanel.shown,
            maximized: (root.roomShown && root.callShown && root.callMaximized) ? "call" : null,
            forceCollapse: root.navMode === "corner" ? ["navcol"] : [],
            drawer: { open: cornerNav.panelShown ? root.navDrawerId : null }
        })
        if (Object.keys(root.layoutTabs).length > 0) v.active = root.layoutTabs
        return v
    }
    // 11.0 (EDITMODE-UI §8): nav · dms · spaces laid out in a row when the form says `opts.<ref>.orient = "h"` (Side monitor)
    function railH(ref) {
        var o = layoutStoreObj.form && layoutStoreObj.form.opts ? layoutStoreObj.form.opts[ref] : null
        return !!o && o.orient === "h"
    }
    // the tab each stack shows (device-local, not synced, not an undo step — EDITMODE-UI §4.4)
    property var layoutTabs: {
        var t = String(settings.pref("layout-tabs") || "")
        try { var o = t.length > 0 ? JSON.parse(t) : ({}); return (o && typeof o === "object" && !Array.isArray(o)) ? o : ({}) } catch (e) { return ({}) }
    }
    function setLayoutTab(stackId, ref) {
        var n = ({})
        for (var k in root.layoutTabs) n[k] = root.layoutTabs[k]
        n[stackId] = ref
        root.layoutTabs = n
        settings.applyPref("layout-tabs", JSON.stringify(n))
    }
    // edit mode's placeholders: the islands whose real content is absent right now (EDITMODE-UI §2)
    readonly property var editAbsent: {
        if (!editModeObj.open) return []
        var a = root.roomShown ? [] : ["header", "call", "timeline", "composer", "panel"]
        if (root.roomShown && !root.callShown) a.push("call")
        if (root.roomShown && !rightPanel.shown) a.push("panel")
        if (navState.railState.dmCount === 0) a.push("dms")
        if (!widgetStack.shown) a.push("widgets")   // an empty widget list: "Add a widget to fill it"
        return a
    }
    // 10.8: in the emergency arrangement (LAYOUT.md §7.12 — a very short window, e.g. under 560 px with a call) the nav
    // column's islands sit in the `emergency` drawer instead of `navcol`: the same chrome (ThDock) shows it, so the
    // rooms stay reachable (the handle used to vanish with the navcol drawer). A string: it changes only when the
    // arrangement does, so the view (and a solve) follows only then.
    readonly property string navDrawerId: (dockHost.lastOut && dockHost.lastOut.emergency === true
        && (dockHost.lastOut.drawers || []).some(function (d) { return d.id === "emergency" })) ? "emergency" : "navcol"

    // 10.8: the layout style, per device (the pref `layout-style`): "islands" = the theme's shape block (Th.shape*:
    // ENGINE.md §4 step 1b); "flat" = edge to edge without frames, the look before islands (Settings → Appearance)
    property string layoutStyle: settings.uiPref("layout-style") === "flat" ? "flat" : "islands"
    readonly property bool flatLayout: root.layoutStyle === "flat"
    readonly property int bannerMargin: root.flatLayout ? Kirigami.Units.smallSpacing : root.dockShape.margin
    function setLayoutStyle(s) {
        if (s !== "islands" && s !== "flat") return
        root.layoutStyle = s
        settings.applyUiPref("layout-style", s)
    }
    readonly property var dockShape: root.flatLayout ? ({ gap: 0, margin: 0, radius: 0, border: 0 })
        : ({ gap: Th.shapeGap, margin: Th.shapeMargin, radius: Th.shapeRadius, border: Th.shapeBorder })
    readonly property bool navInDrawer: dockHost.lastOut !== null && ((dockHost.lastOut.collapsed || []).indexOf("navcol:drawer") >= 0
        || root.navDrawerId === "emergency")
    // the room header clears the drawer handle by the solver's inset (10.1 TH-SHELL-21 used a fixed 46 in corner mode)
    readonly property int headerInset: (dockHost.lastOut && dockHost.lastOut.islands.header && dockHost.lastOut.islands.header.inset)
        ? (dockHost.lastOut.islands.header.inset.l || 0) : 0
    // 10.8: the emergency arrangement hides the header; the call (then the column's first island) carries the handle inset
    readonly property int callInset: (dockHost.lastOut && dockHost.lastOut.islands.call && dockHost.lastOut.islands.call.inset)
        ? (dockHost.lastOut.islands.call.inset.l || 0) : 0
    // the call's page height (its seam share and its caps): the main column = the host minus its margins
    readonly property real callPageH: Math.max(0, dockHost.height - 2 * root.dockShape.margin)
    function _drawerRect(k) {
        var o = dockHost.lastOut
        var ds = (o && o.drawers) ? o.drawers : []
        for (var i = 0; i < ds.length; i++)
            if (ds[i].id === root.navDrawerId && ds[i][k]) return Qt.rect(ds[i][k].x, ds[i][k].y, ds[i][k].w, ds[i][k].h)
        return Qt.rect(0, 0, 0, 0)
    }

    // §2.8 — Back (Alt+Left, the platform Back key, the mouse Back button): the window event filter (video_bridge.cpp)
    // calls this before Qt's shortcut map, after closing an open popup (that alone is a Back). An overlaid panel closes,
    // then an open unpinned drawer; else the collapsed nav's drawer opens. true = handled.
    function thBack() {
        if (root.roomInfoPanel && root.roomInfoPanel.opened && !root.roomInfoPanel.docked) { root.roomInfoPanel.close(); return true }
        if (cornerNav.visible && cornerNav.opened && !cornerNav.pinned) { cornerNav.close(); return true }
        if (cornerNav.visible && !cornerNav.opened) { nav.refresh(); cornerNav.opened = true; return true }
        return false
    }
    function thForward() { return false }
    // Escape's layout steps (§2.8 6-7), after popups, the composer and "fill the app" had theirs.
    function thEscapeLayout() {
        if (root.roomInfoPanel && root.roomInfoPanel.opened && !root.roomInfoPanel.docked) { root.roomInfoPanel.close(); return true }
        if (cornerNav.visible && cornerNav.opened && !cornerNav.pinned) { cornerNav.close(); return true }
        return false
    }

    // Navigate the main view to a room (shared by the room-list pillar, the quick
    // switcher, and the corner nav dock).
    // 9.3: ONE room page, re-targeted. It used to be popped and pushed again on
    // every room switch — the whole page (call panel, menus, timeline) was destroyed and
    // rebuilt, the fresh timeline seeded itself from the previous room's mirror, and the
    // reader saw the old room's messages flash inside the new one, then a blank, then the
    // real thing. Now the page stays put and only the controller moves.
    // selfhost/firstrun §6 (one colour per person): a DM's other person and picture by room id, from the room list
    // (rooms_json: dm, peer, avatar — a DM without its own picture carries the person's). "" when it isn't a DM.
    function _roomListEntry(id) {
        var rs = root.mRooms || []
        for (var i = 0; i < rs.length; i++) if (rs[i].id === id) return rs[i]
        return null
    }
    function dmPeerOf(id) { var e = root._roomListEntry(id); return e && e.dm ? String(e.peer || "") : "" }
    function dmPictureOf(id) { var e = root._roomListEntry(id); return e && e.dm ? String(e.avatar || "") : "" }
    function openRoomId(id) {
        if (!id || id.length === 0) return
        settings.applyUiPref("last-room", id)   // 10.1 (TH-SHELL-06): restored on the next launch
        // 10.1 (TH-NAV-02): the rail + list follow the room you opened (Ctrl+K, a toast, a new DM) —
        // the conversation used to open with a different space showing and nothing highlighted.
        if (root.mNavRooms && !root.mNavRooms.some(function (r) { return r.id === id })) {
            var key = nav.spaceOf(id)
            if (key.length > 0 && key !== nav.activeSpace) nav.selectSpace(key)
        }
        // 11.0 (islands C5): the room page is the `_room` island's content — created by the first open, then kept.
        room.open(id)
        root.roomShown = true
    }
    // widgets W2 (the Memories widget): the Memories window on one memory — its room's list is fetched through the
    // window's own refresh path and the window opens that session once the list holds it (MemoriesWindow.pendingOpen →
    // its tryOpen: a live or processing memory shows the window's notice, like a click on its card)
    function openMemory(roomId, sessionId, roomName) {
        if (!roomId || !sessionId) return
        memoriesWindow.roomId = roomId
        memoriesWindow.roomName = roomName || ""
        room.fetchMemoriesFor(roomId); room.fetchClipsFor(roomId)
        memoriesWindow.pendingOpen = String(sessionId)
        memoriesWindow.openOrRaise()
    }
    // widgets W1 (Pinned, Mentions): jump to a message — the conversation's own jumpTo when it shows the room (the
    // pinned popover's path); another room opens first and the jump follows once its timeline holds the event (a
    // bounded wait: up to 15 s, checked every 250 ms only while a jump is pending; then jumpTo's own "isn't loaded").
    property var _pendingJump: null
    function jumpToEvent(roomId, eventId) {
        if (!roomId || !eventId) return
        if (room.roomId === roomId && root.roomShown && root.conversationView) { root._pendingJump = null; root.conversationView.jumpTo(eventId); return }
        root._pendingJump = { room: roomId, id: eventId, at: Date.now() }
        root.openRoomId(roomId)
        pendingJumpTimer.restart()
    }
    Timer {
        id: pendingJumpTimer
        interval: 250
        repeat: true
        onTriggered: {
            var p = root._pendingJump
            if (!p) { stop(); return }
            var cv = root.conversationView
            var here = room.roomId === p.room && !!cv
            if (here && cv.hasEvent(p.id)) { root._pendingJump = null; stop(); cv.jumpTo(p.id); return }
            if (Date.now() - p.at > 15000) { root._pendingJump = null; stop(); if (here) cv.jumpTo(p.id) }
        }
    }
    // 9.3: Alt+Up / Alt+Down step through the room list as the sidebar shows it (Discord);
    // with Shift, only rooms that have unread messages. The order is snapshotted for a few
    // seconds of stepping: the sidebar re-sorts when a room is marked read, and without the
    // snapshot Down-then-Up landed somewhere else instead of back where you were.
    property var _stepOrder: []
    property double _stepAt: 0
    function stepRoom(dir, unreadOnly) {
        if (!root.mLoggedIn) return
        var now = Date.now()
        if (now - root._stepAt > 4000 || root._stepOrder.length === 0) {
            var src = (root.mNavRooms && root.mNavRooms.length > 0) ? root.mNavRooms : (root.mRooms || [])
            var ids = []
            for (var i = 0; i < src.length; i++) {
                var r = src[i]
                if (!r || !r.id || r.space === true || r.invite === true || r.isInvite === true || r.membership === "invite") continue
                ids.push(r.id)
            }
            root._stepOrder = ids
        }
        root._stepAt = now
        var order = root._stepOrder
        if (order.length === 0) return
        var cur = order.indexOf(room.roomId)
        for (var step = 1; step <= order.length; step++) {
            var j = ((cur + dir * step) % order.length + order.length) % order.length
            var id = order[j]
            if (id === room.roomId) return
            if (unreadOnly && !(root._unreadOf(id) > 0)) continue
            root.openRoomId(id)
            return
        }
    }
    function _unreadOf(id) {
        var lists = [root.mNavRooms || [], root.mRooms || []]
        for (var l = 0; l < lists.length; l++)
            for (var k = 0; k < lists[l].length; k++)
                if (lists[l][k] && lists[l][k].id === id) return lists[l][k].unread || 0
        return 0
    }
    Shortcut { sequences: actionsHost.keysFor("nav.room.previous"); onActivated: root.stepRoom(-1, false) }
    Shortcut { sequences: actionsHost.keysFor("nav.room.next"); onActivated: root.stepRoom(1, false) }
    Shortcut { sequences: actionsHost.keysFor("nav.unread.previous"); onActivated: root.stepRoom(-1, true) }
    Shortcut { sequences: actionsHost.keysFor("nav.unread.next"); onActivated: root.stepRoom(1, true) }

    // Screenshot hook (env TINHAT_GUI_DOCK="corner"): pin the corner nav open a moment
    // after login so the hover panel can be captured without simulating hover.
    Timer {
        interval: 1500
        repeat: false
        running: root.mLoggedIn && login.autoDock === "corner"
        onTriggered: { nav.refresh(); cornerNav.pinned = true; cornerNav.opened = true }
    }

    // 0.8.31: the per-person volume menu for call tiles — one hoisted instance of
    // the component the left bar uses, parameterized by openTileVolumeMenu().
    ParticipantVolumeMenu {
        id: tileVolMenu
        app: root
        audioPrefs: audioPrefsCtl
        myMxid: login.userId
        onMessageUser: (m) => login.createDm(m)
    }

    // Full-window zoomable image viewer (timeline images → click to open). Topmost overlay.
    Lightbox {
        id: lightbox
    }
}
