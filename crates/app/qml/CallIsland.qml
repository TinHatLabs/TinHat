import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import QtQuick.Dialogs
import QtQuick.Window
import QtMultimedia
import org.kde.kirigami as Kirigami
import dev.tinhat.app

// CallIsland.qml — islands C3 (ISLANDS-ARCH §4.2): extracted from Main.qml as it was (the flush layout, pixel-
// identical); controllers in through `app` (the Main.qml root) and the Main.qml ids it needs, intents out.
ColumnLayout {
    id: island
    required property var app   // Main.qml root
    // islands C3: the in-call panel (tiles, call bar, banners) + the call/chat seam, from the room page's header column
    // (the flush layout): the column's 4 px spacing inside, shown while the call is
    spacing: Kirigami.Units.smallSpacing
    visible: island.app.callShown
    // what Main.qml's gate hooks still reach (the hidden-tile chip + its menu, the stats popup, the tile repeater)
    property alias hiddenChip: hiddenChip
    property alias hiddenMenu: hiddenMenu
    property alias statsPopup: statsPopup
    property alias gamePopup: gamePopup   // 11.0 buttons: the Game / join code popover from a button or the Ctrl+K palette
    property alias tileRepeater: tileRepeater
    required property var callOptionsRef   // Main.qml's `callOptions`, passed in (never the same name: it would shadow the id)
    required property var coreRef   // Main.qml's `core`, passed in (never the same name: it would shadow the id)
    required property var devicesMenuRef   // Main.qml's `devicesMenu`, passed in (never the same name: it would shadow the id)
    required property var echoCheckTimerRef   // Main.qml's `echoCheckTimer`, passed in (never the same name: it would shadow the id)
    required property var recordConfirmRef   // Main.qml's `recordConfirm`, passed in (never the same name: it would shadow the id)
    // 11.0 (islands C6/C7): the height of the room's column (the dock host's main column), for the seam share and
    // the caps (was the room page's height)
    property real pageH: 0
    // 10.8: the drawer handle's inset on this island (the emergency arrangement puts the call first in the column, under
    // the nav drawer's handle): the status bar starts clear of it
    property int leftInset: 0
    // 10.8 (islands): the height this call WANTS — the dock host measures it (the call island's natural height) and
    // caps it; the grid then lays out in what the island really has (videoGrid.haveBudget)
    readonly property real naturalH: videoGrid.chromeH + videoGrid.wantH + (callSeam.visible ? callSeam.implicitHeight : 0)
    required property var streamPopoverRef   // Main.qml's `streamPopover`, passed in (never the same name: it would shadow the id)

    ColumnLayout {
        Layout.fillWidth: true
        Layout.margins: Kirigami.Units.smallSpacing
        visible: island.app.callShown
        spacing: Kirigami.Units.smallSpacing
        // 0.8.33: pointer over the call (+ movement) = the chrome may show (callChromeShown).
        HoverHandler {
            id: callChromeHover
            // Qt Quick re-delivers hover on every rendered frame (the live dot never stops
            // animating), so `point` changes constantly: only a real position change counts.
            property point lastPos: Qt.point(-1, -1)
            onHoveredChanged: { island.app.callHoverActive = hovered; if (hovered) island.app.callChromeMoved() }
            onPointChanged: {
                var p = point.position
                if (Math.abs(p.x - lastPos.x) > 1 || Math.abs(p.y - lastPos.y) > 1) { lastPos = p; island.app.callChromeMoved() }
            }
        }

        // Status bar: pulsing live dot + state + stream count (+ furry fox).
        Rectangle {
            id: callStatusBar
            Layout.fillWidth: true
            Layout.leftMargin: Math.max(0, island.leftInset - Kirigami.Units.smallSpacing)   // (the column's own margin)
            // 10.8: a narrow call island (a side monitor, the 480 px minimum) must not be widened by the bar's
            // buttons — the whole column (and the tile grid under it) followed the bar past the island's edge
            Layout.minimumWidth: 0
            clip: true
            implicitHeight: statusRow.implicitHeight + Kirigami.Units.smallSpacing * 2
            radius: island.app.radiusSm
            gradient: Gradient {
                orientation: Gradient.Horizontal
                GradientStop { position: 0.0; color: Th.callStatusBarStartBg }
                GradientStop { position: 1.0; color: Th.callStatusBarEndBg }
            }
            border.width: 1
            border.color: Th.callStatusBarBorder
            RowLayout {
                id: statusRow
                anchors.fill: parent
                anchors.leftMargin: Kirigami.Units.largeSpacing
                anchors.rightMargin: Kirigami.Units.largeSpacing
                spacing: Kirigami.Units.smallSpacing
                Rectangle {
                    id: inCallDot
                    Layout.alignment: Qt.AlignVCenter
                    width: 10; height: 10; radius: 5
                    color: island.app.mConnecting ? Th.callStatusBarDot : Th.callStatusBarDotLive
                    // 11.0 (GPU.md §2.2, R6): STATIC — shown lit. Its 1.2 s blink (9.2, itself the fix for a
                    // 60 fps fade) still redrew the window ~5 times a second for the whole call, audio-only
                    // calls included, focused or not (the 10.6 GPU baseline).
                }
                Controls.Label {
                    text: island.app.mConnecting ? "Connecting…" : "In call"
                    color: Th.callStatusBarFg
                    font.bold: true
                }
                // Name the room this call (and the chat under it) belongs to —
                // the header that carries the room name is hidden while the call
                // fills the app, and people typed into the wrong room.
                Controls.Label {
                    textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                    visible: island.app.mRoomName.length > 0
                    text: "· " + island.app.mRoomName
                    color: Th.callStatusBarFg
                    elide: Text.ElideRight
                    Layout.maximumWidth: 260
                    Layout.minimumWidth: 0   // 10.8: the room name gives way first in a narrow call
                    Layout.fillWidth: true
                }
                Controls.Label {
                    textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                    visible: island.app.callTiles.length > 0
                    text: "· " + island.app.callTiles.length + (island.app.callTiles.length === 1 ? " stream" : " streams")
                    color: Th.callStatusBarCountFg
                    font.pointSize: Kirigami.Theme.smallFont.pointSize
                }
                // Pad (docs/PAD.md, security rule 8): a device of mine drives this call from its Pad — shown for as long
                // as it lasts; × ends that Pad and refuses the device for the rest of this call (PadChip.qml = the web's
                // PadChip.tsx: the strip chip's shape, pad.chip.* tokens).
                Repeater {
                    model: (island.app.mInCall || island.app.fakeCall) ? island.app.mPadChips : []
                    delegate: PadChip {
                        required property var modelData
                        app: island.app
                        device: String(modelData.device || "")
                        name: String(modelData.name || "")
                        onDisconnect: island.app.padCtl.refuseDevice(modelData.device)
                    }
                }
                // "N hidden ▾" — the way back for hidden tiles, in the call
                // header where it is always reachable (the tiles themselves are
                // gone). 0.8.7: a click opens a MENU naming each hidden stream
                // with its own Show — plus "Show all & forget" — because hides
                // are now REMEMBERED across calls and a one-shot blanket
                // "show all" was too blunt for that.
                // 10.5 "can they hear me?" — one pill per fact, shown only while it is true (hear.rs
                // measures; nothing here guesses). Replaces 0.8.25's "mic hears nothing", which
                // judged the PROCESSED audio and so blinked on every pause of a gated mic.
                HearPill {
                    app: island.app
                    id: micCheckPill
                    readonly property string st: (app.mHear && app.mHear.mic) ? app.mHear.mic.check : "ok"
                    readonly property string dev: (app.mHear && app.mHear.mic && app.mHear.mic.dev) ? app.mHear.mic.dev : "your microphone"
                    // The neutral hint waits while you're muted (a gated mic is silent until you
                    // talk — no need to nag a listener); the red states always show.
                    visible: app.mInCall && ((st === "hint" && !app.mMicMuted) || st === "dead" || st === "osmuted" || st === "osvol0")
                    tone: st === "hint" ? "info" : "danger"
                    icon: "microphone-sensitivity-muted-symbolic"
                    text: st === "hint" ? "No sound from your mic yet"
                        : st === "dead" ? "Mic isn't picking anything up"
                        : st === "osmuted" ? "Mic muted in system settings" : "Mic volume at 0 %"
                    action: st === "osmuted" ? "Unmute" : (st === "osvol0" ? "Turn up" : (st === "dead" ? "Pick input" : ""))
                    shortText: st === "hint" ? "No mic sound yet" : (st === "dead" ? "Mic silent" : (st === "osmuted" ? "Mic muted (system)" : "Mic at 0 %"))
                    tip: st === "hint"
                         ? "Nothing has come from " + dev + " since you joined. Say something to check — this goes away at the first sound. (A mic with a noise gate stays silent until you talk.)"
                         : st === "dead"
                         ? dev + " has sent a completely flat signal for a minute while you were unmuted — it may be muted with a switch on the mic or headset, unplugged, or the wrong input"
                           + (Qt.platform.os === "windows" ? " (or muted in Windows sound settings)" : "") + "."
                         : st === "osmuted" ? dev + " is muted in your system's sound settings, so nobody can hear you. Unmute fixes it from here."
                         : dev + "'s input volume is 0 % in your system's sound settings. Turn up sets it to 100 %."
                    onActivated: {
                        if (st === "osmuted" || st === "osvol0") app.callctl.fixSystemLevel(false)
                        else island.callOptionsRef.toggleFor(micBtn, "mic")
                    }
                }
                HearPill {
                    app: island.app
                    readonly property string st: (app.mHear && app.mHear.out) ? app.mHear.out.check : "ok"
                    readonly property string dev: (app.mHear && app.mHear.out && app.mHear.out.dev) ? app.mHear.out.dev : "your speakers"
                    visible: app.mInCall && (st === "osmuted" || st === "osvol0")
                    tone: "danger"
                    icon: "audio-volume-muted-symbolic"
                    text: st === "osmuted" ? "Speakers muted in system settings" : "Speaker volume at 0 %"
                    action: st === "osmuted" ? "Unmute" : "Turn up"
                    shortText: st === "osmuted" ? "Speakers muted (system)" : "Speakers at 0 %"
                    tip: "You won't hear anyone: " + dev + (st === "osmuted" ? " is muted" : " is at 0 %") + " in your system's sound settings."
                    onActivated: app.callctl.fixSystemLevel(true)
                }
                HearPill {
                    app: island.app
                    readonly property string st: (app.mHear && app.mHear.tx) ? app.mHear.tx.state : "ok"
                    visible: app.mInCall && !app.mMicMuted && (st === "notsent" || st === "quiet")
                    tone: st === "notsent" ? "warn" : "info"
                    icon: st === "notsent" ? "dialog-warning" : "audio-volume-low-symbolic"
                    text: st === "notsent" ? "The call isn't getting your mic" : "Mic very quiet"
                    action: st === "notsent" ? "Reconnect" : (app.mMicAgc ? "" : "Auto gain")
                    shortText: st === "notsent" ? "Mic not reaching the call" : "Mic quiet"
                    tip: st === "notsent"
                         ? "Nobody can hear you: " + ((app.mHear.tx && app.mHear.tx.why) ? app.mHear.tx.why : "the call server didn't receive your voice") + ". Reconnect mic publishes it again."
                         : "People may barely hear you — your voice reaches the call below the level it counts as speech. "
                           + (app.mMicAgc ? "Raise the input volume in your sound settings." : "Auto gain evens it out (or raise the input volume).")
                    onActivated: { if (st === "notsent") app.callctl.reconnectMic(); else app.callctl.applyMicAgc(true) }
                }
                // 11.3 same room: 10.5's detector pairing (their stream carries our voice < 120 ms, or they report
                // the same about our mic) becomes a one-click link; linked = status + Unlink; two devices playing
                // the call = guidance. The rules + wire live in the streamer (sameroom/); this only shows them.
                HearPill {
                    app: island.app
                    id: srSuggestPill
                    readonly property var sr: island.app.mSameRoom
                    readonly property var cand: island.app.sameRoomSuggest(island.app.mHear, island.app.mSameRoom)
                    readonly property string who: (cand && cand.length > 0) ? island.app.hearName(cand[0]) : ""
                    visible: app.mInCall && !!sr && !!sr.linked && sr.linked.length === 0 && sr.pending.length === 0 && !!cand && cand.length > 0
                    tone: "info"
                    icon: "audio-input-microphone-symbolic"
                    text: "Sounds like you and " + who + " are in the same room"
                    action: "Link up"
                    shortText: "Same room as " + who + "?"
                    shortAction: "Link up"
                    tip: who + " is heard through your mic as well as their own, so everyone else hears them twice. Linking up makes Tin Hat send one voice from your room (whoever's own mic hears them loudest) and stops your devices playing each other."
                    onActivated: {
                        var err = app.callctl.sameRoomLink(cand[0])
                        if (err.length > 0) app.showPassiveNotification(LinkGate.plain(err), 4000)
                    }
                }
                HearPill {
                    app: island.app
                    readonly property var sr: island.app.mSameRoom
                    visible: app.mInCall && sr.linked.length === 0 && sr.pending.length > 0
                    tone: "info"
                    icon: "audio-input-microphone-symbolic"
                    text: "Asking " + island.app.hearName(sr.pending) + " to link up…"
                    action: "Cancel"
                    shortText: "Linking up…"
                    shortAction: "Cancel"
                    tip: "They see a card. Once they say yes, Tin Hat sends one voice from your room and stops your two devices playing each other."
                    onActivated: app.callctl.sameRoomCancel()
                }
                HearPill {
                    app: island.app
                    id: srLinkedPill
                    readonly property var sr: island.app.mSameRoom
                    readonly property string names: island.app.hearNames(sr.linked)
                    readonly property string micName: sr.live ? "you" : (sr.holder.length > 0 ? island.app.hearName(sr.holder) : "—")
                    visible: app.mInCall && sr.linked.length > 0
                    tone: "info"
                    icon: "audio-input-microphone-symbolic"
                    text: "In a room with " + names + " · room mic: " + micName
                    action: "Unlink"
                    shortText: "Same room: " + names
                    shortAction: "Unlink"
                    tip: "Only one of your mics is live at a time — whoever's own mic hears them loudest, so everyone else hears each of you once. You don't hear " + names + " through Tin Hat: you hear " + (sr.linked.length === 1 ? "them" : "each other") + " in person."
                         + (sr.rawOk ? "" : " (The level meter runs on the processed mic here, so a loud speaker right next to you may still leak through.)")
                    onActivated: app.callctl.sameRoomUnlink()
                }
                HearPill {
                    app: island.app
                    id: srSpkPill
                    readonly property var sr: island.app.mSameRoom
                    readonly property bool echoNow: !!(app.mHear && ((app.mHear.localEcho) || (app.mHear.echoMe && app.mHear.echoMe.by && app.mHear.echoMe.by.length > 0)))
                    readonly property string first: sr.linked.length > 0 ? island.app.hearName(sr.linked[0]) : ""
                    visible: app.mInCall && sr.linked.length > 0 && sr.spk && echoNow
                    tone: "warn"
                    icon: "audio-volume-high-symbolic"
                    text: "Two devices in your room play the call — others may hear echo"
                    action: "Hear it through " + first + "'s speakers"
                    shortText: "Two devices playing"
                    shortAction: "Use theirs"
                    tip: "Each device's echo cancellation only knows its own speakers, so the far end comes back through the other device's mic. Let one device play the call (headphones also fix it). Your mic keeps working."
                    onActivated: app.callctl.sameRoomHearThrough(false)
                }
                HearPill {
                    app: island.app
                    readonly property var sr: island.app.mSameRoom
                    readonly property string first: sr.linked.length > 0 ? island.app.hearName(sr.linked[0]) : ""
                    visible: app.mInCall && sr.linked.length > 0 && !sr.spk
                    tone: "info"
                    icon: "headphones-symbolic"
                    text: "The call plays on " + first + "'s device"
                    action: "Play here too"
                    shortText: "Playing on their device"
                    shortAction: "Play here"
                    tip: "Your speakers stay quiet for the call so the room hears it once; your mic still works. Remembered for next time."
                    onActivated: app.callctl.sameRoomHearThrough(true)
                }
                HearPill {
                    app: island.app
                    id: echoPill
                    readonly property var em: (app.mHear && app.mHear.echoMe) ? app.mHear.echoMe : ({ by: [] })
                    readonly property var le: (app.mHear && app.mHear.localEcho) ? app.mHear.localEcho : null
                    readonly property var fix: (app.mHear && app.mHear.ecFix) ? app.mHear.ecFix : ({ kind: "" })
                    readonly property bool reported: !!(em.by && em.by.length > 0)
                    readonly property string fixText: fix.kind === "turn_on" ? "Echo cancellation removes it."
                        : fix.kind === "add_output" ? ((fix.label && fix.label.length > 0 ? fix.label : "That output") + " isn't in echo cancellation's speaker list — adding it lets the echo be removed.")
                        : fix.kind === "retry" ? "Echo cancellation is switched on but isn't running right now."
                        : "Echo cancellation is on — lower your speaker volume or use headphones."
                    visible: app.mInCall && (reported || le !== null) && !srSpkPill.visible
                    tone: app._echoChecking ? "info" : "warn"
                    icon: "audio-volume-high-symbolic"
                    // The request (2026-09-22): "a little pill to recommend echo cancellation" — the fix IS the
                    // pill's text; who hears it and how loud is in the tooltip.
                    text: app._echoChecking ? "Checking the echo…"
                        : (fix.kind === "volume" || fix.kind === "" ) ? "Echo — lower your speakers or use headphones" : "Echo"
                    action: app._echoChecking ? ""
                        : fix.kind === "turn_on" ? "Turn on echo cancellation"
                        : fix.kind === "add_output" ? "Add " + (fix.label && fix.label.length > 0 ? fix.label : "this output") + " to echo cancellation"
                        : fix.kind === "retry" ? "Retry echo cancellation" : ""
                    shortText: app._echoChecking ? "Checking echo…" : ((fix.kind === "volume" || fix.kind === "") ? "Echo — lower speakers" : "Echo")
                    shortAction: fix.kind === "turn_on" ? "Turn on echo cancel" : (fix.kind === "add_output" ? "Add output" : (fix.kind === "retry" ? "Retry" : ""))
                    tip: app._echoChecking ? "Measuring again — this pill goes away once the echo is gone."
                        : reported ? app.hearNames(em.by) + (em.by.length === 1 ? " hears" : " hear") + " their own voice come back through your mic, about " + em.lag + " ms later. " + fixText
                        : "Your mic picks up what your speakers play (" + (le && le.src === "system" ? "system audio — a game, music or a video" : "the call") + "), so others can hear it back. " + fixText
                    onActivated: {
                        if (fix.kind === "add_output") app.callctl.addEcOutput(fix.out)
                        else app.callctl.applyMicAec(true)
                        app._echoChecking = true
                        island.echoCheckTimerRef.restart()
                    }
                    onVisibleChanged: {
                        if (!visible && app._echoChecking) {
                            app._echoChecking = false
                            island.echoCheckTimerRef.stop()
                            app.showPassiveNotification("Echo gone ✓", 3000)
                        }
                    }
                }
                Rectangle {
                    id: noOutputChip
                    visible: island.app.mInCall && island.app.mOutputMissing
                    implicitWidth: noOutputRow.implicitWidth + 16
                    implicitHeight: noOutputRow.implicitHeight + 6
                    radius: height / 2
                    color: Th.hearNoOutputChipBg
                    border.width: 1
                    border.color: Th.hearNoOutputChipBorder
                    HoverHandler { id: noOutputHover }
                    Controls.ToolTip.visible: noOutputHover.hovered
                    Controls.ToolTip.delay: 300
                    Controls.ToolTip.text: "No usable audio output was found when you joined (the system only offers a dummy output). "
                        + "Nothing can be heard until the sound device is back — check the system sound settings (Linux: wpctl status)."
                    Row {
                        id: noOutputRow
                        anchors.centerIn: parent
                        spacing: 5
                        Kirigami.Icon {
                            anchors.verticalCenter: parent.verticalCenter
                            width: 13; height: 13
                            source: "audio-volume-muted-symbolic"
                            color: Th.hearNoOutputChipFg
                        }
                        Controls.Label {
                            anchors.verticalCenter: parent.verticalCenter
                            text: "no speaker device"
                            color: Th.hearNoOutputChipFg
                            font.pointSize: Kirigami.Theme.smallFont.pointSize
                        }
                    }
                }
                Rectangle {
                    id: hiddenChip
                    visible: island.app.hiddenTileCount > 0
                    implicitWidth: hiddenRow.implicitWidth + 16
                    implicitHeight: hiddenRow.implicitHeight + 6
                    radius: height / 2
                    color: showAllHover.hovered ? Th.callHiddenChipBgHover : Th.callHiddenChipBg
                    border.width: 1
                    border.color: Th.callHiddenChipBorder
                    HoverHandler { id: showAllHover }
                    TapHandler { onTapped: hiddenMenu.popup(hiddenChip, 0, hiddenChip.height + 4) }
                    Row {
                        id: hiddenRow
                        anchors.centerIn: parent
                        spacing: 5
                        Kirigami.Icon {
                            anchors.verticalCenter: parent.verticalCenter
                            width: 13; height: 13
                            source: "layer-visible-off-symbolic"
                            color: Th.callHiddenChipFg
                        }
                        Controls.Label {
                            textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                            anchors.verticalCenter: parent.verticalCenter
                            text: island.app.hiddenTileCount + " hidden ▾"
                            color: Th.callHiddenChipFg
                            font.pointSize: Kirigami.Theme.smallFont.pointSize
                        }
                    }
                    Controls.ToolTip.visible: showAllHover.hovered && !hiddenMenu.visible
                    Controls.ToolTip.text: "See who's hidden — show them one by one, or all at once"
                    Controls.Menu {
                        id: hiddenMenu
                        modal: true
                        MenuSectionLabel { app: island.app; text: "Hidden streams" }
                        Instantiator {
                            model: island.app.callTiles.filter(function(t) { return t && island.app.isTileHidden(t.id) })
                            delegate: Controls.MenuItem {
                                readonly property bool recOnly: island.app.isRecOnlyHidden(modelData.id)
                                text: LinkGate.plain(recOnly
                                    ? "Tune in to " + (modelData.name || island.app.tileMxid(modelData.participant)) + "'s recording stream"
                                    : "Show " + (modelData.name || island.app.tileMxid(modelData.participant))
                                        + (modelData.screenshare === true ? " — screen share"
                                           : (modelData.kind === "avatar" ? "" : " — camera")))
                                icon.name: recOnly ? "media-record-symbolic" : "layer-visible-on-symbolic"
                                onTriggered: {
                                    if (recOnly) island.app.callctl.setTileTunedIn(modelData.id, true)
                                    else island.app.unhideTile(modelData.id)
                                }
                            }
                            onObjectAdded: (i, o) => hiddenMenu.insertItem(i + 1, o)
                            onObjectRemoved: (i, o) => hiddenMenu.removeItem(o)
                        }
                        Controls.MenuSeparator {}
                        Controls.MenuItem {
                            text: "Show all && forget"
                            icon.name: "edit-undo-symbolic"
                            onTriggered: island.app.unhideAllTiles()
                        }
                        Controls.MenuItem {
                            enabled: false
                            text: "Remembered between calls"
                            font.pointSize: Kirigami.Theme.smallFont.pointSize
                        }
                    }
                }
                Controls.Label {
                    visible: island.app.mCallSharing || island.app.mSharing
                    text: island.app.mShareRecOnly ? "· sharing screen (recording only)" : "· sharing screen"
                    color: Th.callStatusBarShareFg
                    font.pointSize: Kirigami.Theme.smallFont.pointSize
                    font.bold: true
                }
                // 🔴 REC — the Memories recorder is a participant in this call right now.
                // Driven by its real presence (recActive), not the room
                // marker, so it can never claim a recording that isn't
                // happening. Opted-out users ("Never record me") are
                // excluded by the recorder itself.
                Controls.Label {
                    visible: island.app.recActive
                    text: island.app.settings.recordOptout ? "· 🔴 REC (not recording you)" : "· 🔴 REC"
                    color: Th.callStatusBarRecFg
                    font.pointSize: Kirigami.Theme.smallFont.pointSize
                    font.bold: true
                }
                Item { Layout.fillWidth: true }
                // 0.8.8 DVR: record toggle (room mods) + clip bookmark
                // (anyone, while a recording is live).
                ThToolButton {
                    // 0.8.11: any member may record (was mod-gated) —
                    // needs the room provisioned (admin opened it on
                    // 0.8.11+); a 403 surfaces honestly in the status.
                    // 0.8.30: pack glyph + label instead of typeset ⏺/⏹.
                    // 11.0 (product decision 2026-09-27, ActionsHost.recClipRoom): Rec and Clip act on the CALL's
                    // room, also while another room is on screen (they acted on the open room; the web's never did).
                    id: recBtn
                    visible: island.app.serverMemories   // selfhost: only where the server records (hidden, never dimmed)
                    readonly property string recRoom: app.actions ? app.actions.recClipRoomId : String(app.room.roomId || "")
                    readonly property bool recOn: app.actions ? app.actions.recClipRoomRecordOn : app.room.recordOn
                    app: island.app
                    iconName: recBtn.recOn ? "media-playback-stop-symbolic" : "media-record-symbolic"
                    iconColor: recBtn.recOn ? Th.callRecButtonIconActive : Th.callRecButtonIcon
                    label: "Rec"
                    active: recBtn.recOn
                    tip: recBtn.recOn
                        ? "Stop recording this room's calls"
                        : "Record this room's calls — REC shows for everyone"
                    onClicked: {
                        if (recBtn.recOn) {
                            if (recBtn.recRoom === String(app.room.roomId || "")) app.room.setRecordCall(false)
                            else app.room.setRecordCallIn(recBtn.recRoom, false)
                        } else {
                            island.recordConfirmRef.forRoom = recBtn.recRoom
                            island.recordConfirmRef.open()
                        }
                    }
                }
                ThToolButton {
                    visible: app.recActive && island.app.serverMemories
                    app: island.app
                    iconName: "bookmark-new-symbolic"
                    label: "Clip"
                    tip: "Bookmark this moment on the recording's timeline"
                    onClicked: {
                        var cr = app.actions ? app.actions.recClipRoomId : String(app.room.roomId || "")
                        if (cr === String(app.room.roomId || "")) app.room.clipMoment(); else app.room.clipMomentIn(cr)
                        app.showPassiveNotification("Clipped — bookmarked on the Memories timeline", "short")
                    }
                }
                // Layout presets. 0.8.30: pack glyph + label (the pack has a grid now).
                ThToolButton {
                    id: layoutBtn
                    Layout.alignment: Qt.AlignVCenter
                    app: island.app
                    iconName: "view-grid-symbolic"
                    label: "Layout"
                    active: layoutMenu.visible
                    tip: "How the call grid is arranged"
                    Controls.ToolTip.visible: hovered && !layoutMenu.visible
                    onClicked: layoutMenu.popup(layoutBtn, 0, layoutBtn.height + 2)
                    Controls.Menu {
                        id: layoutMenu
                        modal: true
                        MenuSectionLabel { app: island.app; text: "Layout" }
                        Controls.MenuItem { text: "Auto — streams big, cams beside"; checkable: true; checked: island.app.callLayout === "auto"; onTriggered: island.app.setCallLayout("auto") }
                        Controls.MenuItem { text: "Gallery — everyone equal"; checkable: true; checked: island.app.callLayout === "gallery"; onTriggered: island.app.setCallLayout("gallery") }
                        Controls.MenuItem { text: "Speaker — one big, the rest below"; checkable: true; checked: island.app.callLayout === "speaker"; onTriggered: island.app.setCallLayout("speaker") }
                        Controls.MenuItem { text: "Side strip — streams big, everyone beside"; checkable: true; checked: island.app.callLayout === "sidebar"; onTriggered: island.app.setCallLayout("sidebar") }
                        Controls.MenuSeparator {}
                        Controls.MenuItem { text: island.app.pinnedTile.length > 0 ? "Unpin the main tile" : "Pin: hover a tile and press the pin"; enabled: island.app.pinnedTile.length > 0; onTriggered: island.app.pinnedTile = "" }
                        Controls.MenuItem { text: "Animate layout changes"; checkable: true; checked: island.app.callAnimate; onTriggered: island.app.callAnimate = !island.app.callAnimate }
                    }
                }
                // 9.3.1: tuck the tiles away (the bars stay) so the chat has the room.
                ThToolButton {
                    Layout.alignment: Qt.AlignVCenter
                    visible: !app.callMaximized
                    app: island.app
                    iconName: app.callCollapsed ? "go-down-symbolic" : "go-up-symbolic"
                    active: app.callCollapsed
                    tip: app.callCollapsed ? "Show the call tiles" : "Collapse the call — the chat gets the room (drag the seam below to resize)"
                    onClicked: app.setCallCollapsed(!app.callCollapsed)
                }
                // "Fill the app" — expand the call to the whole window content area
                // (nav + timeline + composer hidden); NOT OS fullscreen.
                ThToolButton {
                    Layout.alignment: Qt.AlignVCenter
                    app: island.app
                    iconName: app.callMaximized ? "view-restore-symbolic" : "view-fullscreen-symbolic"
                    active: app.callMaximized
                    tip: app.callMaximized ? "Restore layout" : "Fill the app with the call"
                    onClicked: app.callMaximized = !app.callMaximized
                }
            }
        }

        // #7 BRB/AFK SELF banner — unmistakable indicator that WE are away. Driven by
        // mLocalAfk (set optimistically by setAfk, so it reflects immediately). Click
        // anywhere on it to come back (clears AFK). The BRB toggle button below also
        // shows its accent active-state, but this banner makes "you're away" obvious.
        Rectangle {
            id: afkBanner
            Layout.fillWidth: true
            visible: island.app.mLocalAfk
            implicitHeight: afkRow.implicitHeight + Kirigami.Units.smallSpacing * 2
            radius: island.app.radiusSm
            color: Th.callAfkBannerBg
            border.width: 1
            border.color: Th.callAfkBannerBorder
            // 11.0 (GPU.md §2.2, R6): STATIC (its 1.2 s pulse redrew the window for the whole time you
            // were away — the very moment nobody looks at it).
            RowLayout {
                id: afkRow
                anchors.fill: parent
                anchors.leftMargin: Kirigami.Units.largeSpacing
                anchors.rightMargin: Kirigami.Units.largeSpacing
                spacing: Kirigami.Units.smallSpacing
                Kirigami.Icon {
                    Layout.preferredWidth: 20; Layout.preferredHeight: 20
                    // appointment-missed-symbolic uses currentColor/ColorScheme so the
                    // color override below actually applies. (user-away-symbolic resolves
                    // to a hardcoded #fdbc4b in breeze-dark and ignores `color:`.)
                    source: "appointment-missed-symbolic"
                    color: Th.callAfkBannerIcon
                }
                Controls.Label {
                    Layout.fillWidth: true
                    text: "💤 You're away (BRB) — click to return"
                    color: Th.callAfkBannerFg
                    font.bold: true
                }
            }
            MouseArea {
                anchors.fill: parent
                cursorShape: Qt.PointingHandCursor
                onClicked: island.app.afkCtl.setAfk(false)
            }
        }

        // 11.3 same room: "X says you're in the same room — link up?" (45 s, until answered or the asker cancels).
        Kirigami.InlineMessage {
            id: srAskCard
            Layout.fillWidth: true
            readonly property var ask: (island.app.mSameRoom && island.app.mSameRoom.ask) ? island.app.mSameRoom.ask : null
            visible: island.app.mInCall && ask !== null
            type: Kirigami.MessageType.Information
            text: LinkGate.plain(ask ? (island.app.hearName(ask.from) + " says you're in the same room — link up? One of your mics sends at a time and you won't hear each other through Tin Hat.") : "")
            actions: [
                Kirigami.Action { text: "Link up"; icon.name: "dialog-ok"; onTriggered: island.app.callctl.sameRoomAnswer(true) },
                Kirigami.Action { text: "Not now"; onTriggered: island.app.callctl.sameRoomAnswer(false) }
            ]
        }
        // RBAC §7.4: a moderator muted my mic for everyone — say so where I am looking,
        // for as long as it lasts (the mic button is locked meanwhile).
        Kirigami.InlineMessage {
            Layout.fillWidth: true
            visible: island.app.mServerMuted && (island.app.mInCall || island.app.mConnecting)
            type: Kirigami.MessageType.Warning
            text: LinkGate.plain(island.app.serverMutedText() + ". Ask them to unmute you.")
        }
        // Connection trouble banner: amber while auto-reconnecting, red + Rejoin
        // when the call dropped terminally (so a dead call never looks fine).
        Rectangle {
            id: connBanner
            Layout.fillWidth: true
            visible: island.app.mReconnecting || island.app.mCallFailed
            implicitHeight: connRow.implicitHeight + Kirigami.Units.smallSpacing * 2
            radius: island.app.radiusSm
            color: island.app.mCallFailed ? Th.callConnBannerBadBg : Th.callConnBannerWarnBg
            border.width: 1
            border.color: island.app.mCallFailed ? Th.callConnBannerBadBorder : Th.callConnBannerWarnBorder
            RowLayout {
                id: connRow
                anchors.fill: parent
                anchors.leftMargin: Kirigami.Units.largeSpacing
                anchors.rightMargin: Kirigami.Units.largeSpacing
                spacing: Kirigami.Units.smallSpacing
                ThBusyIndicator {   // 11.0 (GPU.md §2.5): still; the title changes once after 10 s
                    id: reconnMark
                    running: island.app.mReconnecting && !island.app.mCallFailed
                    visible: running
                    size: 18
                    color: Th.callConnBannerWarnIcon
                    label: "Reconnecting to the call…"
                }
                ColumnLayout {
                    Layout.fillWidth: true
                    spacing: 0
                    Controls.Label {
                        textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                        Layout.fillWidth: true
                        text: !island.app.mCallFailed ? reconnMark.shownLabel
                            : (island.app.mRemovedBy.length > 0 ? island.app.mRemovedBy + " disconnected you from the call"   // RBAC §7.4
                            : (island.app.mCallFailReason.length > 0 ? "Couldn't join the call" : "Call ended — connection lost"))
                        color: Th.callConnBannerTitleFg
                        font.bold: true
                        elide: Text.ElideRight
                    }
                    // 10.1 (TH-CALL-03): the reason, in a sentence, instead of a 4 s toast.
                    Controls.Label {
                        textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                        Layout.fillWidth: true
                        visible: island.app.mCallFailed && island.app.mCallFailReason.length > 0
                        text: island.app.mCallFailReason.length > 0 ? island.app.mCallFailReason.charAt(0).toUpperCase() + island.app.mCallFailReason.slice(1) : ""
                        color: Th.callConnBannerReasonFg
                        font.pointSize: Kirigami.Theme.smallFont.pointSize
                        wrapMode: Text.WordWrap
                        maximumLineCount: 2
                        elide: Text.ElideRight
                    }
                }
                GlowButton {
                    app: island.app
                    visible: app.mCallFailed
                    // Disabled while the dropped call's teardown still runs —
                    // joinCall is gated on hangingUp (the rejoin/teardown race
                    // clobbered the new call's E2EE state), so an enabled button
                    // would just no-op confusingly. Re-enables within seconds.
                    enabled: !app.mHangingUp
                    text: app.mHangingUp ? "Cleaning up…" : (app.mCallFailReason.length > 0 ? "Try again" : "Rejoin")
                    onClicked: app.callctl.joinCall(app.room.roomId)
                }
            }
        }

        // Video tile grid — tiles are ABSOLUTE-positioned so each ROW is centered (the
        // "fill in and center" ask). Was a Flow (packs top-left, can't center a partial
        // row); a plain Item lets each tile self-position. Polish-loop guards preserved:
        // every size clamped >= 0, height from explicit row math (never childrenRect).
        Item {
            id: videoGrid
            Layout.fillWidth: true
            Layout.minimumWidth: 0   // 10.8: the grid takes the island's width, never its content's
            // A little breathing room from the window edges when maximized (the
            // user-requested "bit of padding"); 0 in the normal header strip.
            Layout.margins: island.app.callMaximized ? Kirigami.Units.largeSpacing : 0
            // The in-call panel lives in a ScrollablePage HEADER, which is sized to
            // its implicitHeight and is NOT stretched — so Layout.fillHeight gives no
            // height. For the maximized mosaic, compute the viewport height EXPLICITLY
            // from the page minus the (visible) status + control bars; the header then
            // grows to ~page height, filling the app. Normal mode: content height.
            // EVERYTHING in the panel that is not the grid — status bar, control bar
            // (+ its topMargin), any visible banner (+ its spacing gap), the panel's
            // outer margins and the two inter-child spacing gaps around the grid.
            // Under-counting this is exactly how the control bar used to get pushed
            // off-screen (found live: 3+ tiles in "fill the app" hid the hangup bar
            // until the user un-maximized).
            readonly property real chromeH:
                callStatusBar.height + callControlBar.height
                + Kirigami.Units.smallSpacing            // callControlBar.Layout.topMargin
                + (afkBanner.visible ? afkBanner.height + Kirigami.Units.smallSpacing : 0)
                + (connBanner.visible ? connBanner.height + Kirigami.Units.smallSpacing : 0)
                + Kirigami.Units.smallSpacing * 2        // panel Layout.margins (top+bottom)
                + Kirigami.Units.smallSpacing * 2        // spacing gaps flanking the grid
            readonly property real availH: island.app.callMaximized
                ? Math.max(160, island.pageH - chromeH - Kirigami.Units.largeSpacing * 2)
                : 0
            // Normal (non-maximized) mode: cap the grid so the header NEVER exceeds
            // the page — the control bar must stay on-screen at any tile count — and
            // keep a slice of the page for the timeline/composer below.
            // 9.3.1: once the seam has been dragged, the call's share of the page is
            // exactly what the user set (tiles fill it); before that, the old rule —
            // natural height, capped so the chat keeps at least a quarter of the page.
            // islands C2 (ISLANDS-ARCH §4.2): whole pixels, rounded the way the layout solver rounds a
            // per-mille length (fdiv = floor): the seam share as per mille of the page, the chat's quarter as
            // floor(pageH / 4) — the dock host reproduces exactly these heights (the web's engineHeight too)
            readonly property int seamPm: Math.round(island.app.callPaneFrac * 1000)
            readonly property real normalCapH: island.app.callPaneFrac >= 0
                ? Math.max(120, Math.floor(island.pageH * seamPm / 1000) - chromeH)
                : Math.max(200, island.pageH - chromeH - Math.max(160, Math.floor(island.pageH / 4)))
            // Live (visible) tile count — when WE are AFK only the self-view shows, so the
            // grid collapses to 1 (self full-area). Drives the weighted layout below.
            readonly property int liveCount: island.app.mLocalAfk ? 1 : island.app.callTiles.length
            readonly property real spacing: Kirigami.Units.smallSpacing  // gap (Item has no built-in spacing)
            visible: island.app.callTiles.length > 0 && !(island.app.callCollapsed && !island.app.callMaximized)

            // Mosaic fit (HELPER): pack `n` equal cells into WxH. Pick the column count
            // that best fills the area — fewest empty trailing cells, tie-broken by the
            // cell shape closest to a pleasant ~16:9. Used by weightedLayout for the
            // primary (spotlight) region and the all-audio fallback. Tiles FILL their
            // cell (no forced 16:9 — qml6glsink letterboxes the real stream inside).
            function mosaicFit(W, H, n) {
                if (n <= 0 || W <= 0 || H <= 0) return { cols: 1, tileW: 0, tileH: 0 }
                var gap = videoGrid.spacing
                var target = 16 / 9
                var best = null
                for (var c = 1; c <= n; c++) {
                    var r = Math.ceil(n / c)
                    var tw = Math.floor((W - (c - 1) * gap) / c)
                    var th = Math.floor((H - (r - 1) * gap) / r)
                    if (tw <= 0 || th <= 0) continue
                    // log-ratio so 2x and 1/2x off-target score equally; + a penalty per
                    // empty trailing cell so we favour layouts that fill the grid.
                    var shape = Math.abs(Math.log((tw / th) / target))
                    var empty = c * r - n
                    var score = shape + empty * 0.5
                    if (best === null || score < best.score)
                        best = { cols: c, tileW: tw, tileH: th, score: score }
                }
                return best || { cols: 1, tileW: Math.floor(W), tileH: Math.floor(H) }
            }

            // Place `n` equal cells (mosaicFit packing) inside the rect (ox,oy,W,H),
            // CENTERING every row (left-pad by half the row's leftover, same math the
            // tiles used before). Writes {x,y,w,h} into `out` at out[base+i]. Returns the
            // bottom edge (oy + used height) so callers can stack regions / size normalH.
            function placeMosaic(out, base, idxs, ox, oy, W, H) {
                var n = idxs.length
                if (n <= 0) return oy
                var m = mosaicFit(W, H, n)
                var cols = Math.max(1, m.cols)
                var tw = Math.max(0, m.tileW), th = Math.max(0, m.tileH)
                var gap = videoGrid.spacing
                var rows = Math.ceil(n / cols)
                for (var i = 0; i < n; i++) {
                    var rr = Math.floor(i / cols)
                    var cc = i % cols
                    var inRow = Math.max(1, Math.min(cols, n - rr * cols))
                    var rowW = inRow * tw + (inRow - 1) * gap
                    out[idxs[i]] = {
                        x: Math.max(0, Math.round(ox + (W - rowW) / 2 + cc * (tw + gap))),
                        y: Math.max(0, Math.round(oy + rr * (th + gap))),
                        w: Math.max(0, tw), h: Math.max(0, th)
                    }
                }
                return oy + rows * th + (rows - 1) * gap
            }

            // Place `n` SMALL cells as ONE centered horizontal filmstrip row in the band
            // (ox,oy,W,bandH). Each avatar tile is square-ish (capped at bandH*16/9 wide)
            // and shrinks to fit when there are many. Returns the band's bottom edge.
            // 10.1 (TH-CALL-09): a strip tile is never narrower than STRIP_MIN_W — past that
            // the strip wraps into rows (stripRowsFor sizes the band to match), so a
            // twelve-person voice night is three rows of faces, not a row of anonymous dots.
            readonly property int stripMinW: 120
            function stripRowsFor(n, W) {
                if (n <= 0) return 0
                var gap = videoGrid.spacing
                var perRow = Math.max(1, Math.floor((W + gap) / (stripMinW + gap)))
                return Math.ceil(n / perRow)
            }
            function placeStrip(out, idxs, ox, oy, W, bandH) {
                var n = idxs.length
                if (n <= 0) return oy
                var gap = videoGrid.spacing
                var rows = Math.max(1, stripRowsFor(n, W))
                var perRow = Math.ceil(n / rows)   // balanced rows (7 → 4 + 3, not 6 + 1)
                var rowH = Math.max(0, (bandH - (rows - 1) * gap) / rows)
                var maxW = Math.round(rowH * 16 / 9)
                var fitW = Math.floor((W - (perRow - 1) * gap) / perRow)
                var tw = Math.max(0, Math.min(maxW, fitW))
                var th = Math.max(0, rowH)
                for (var i = 0; i < n; i++) {
                    var r = Math.floor(i / perRow), c = i % perRow
                    var inRow = Math.min(perRow, n - r * perRow)
                    var rowW = inRow * tw + (inRow - 1) * gap
                    out[idxs[i]] = {
                        x: Math.max(0, Math.round(ox + (W - rowW) / 2 + c * (tw + gap))),
                        y: Math.max(0, Math.round(oy + r * (rowH + gap))),
                        w: tw, h: th
                    }
                }
                return oy + bandH
            }

            // Place `idxs` as ONE vertical column at (ox,oy), width colW, height H:
            // 16:9 tiles stacked top-down, shrinking to fit when there are many.
            // Returns the column's bottom edge.
            function placeColumn(out, idxs, ox, oy, colW, H) {
                var n = idxs.length
                if (n <= 0) return oy
                var gap = videoGrid.spacing
                var th = Math.max(0, Math.min(Math.round(colW * 9 / 16), Math.floor((H - (n - 1) * gap) / n)))
                for (var i = 0; i < n; i++) {
                    out[idxs[i]] = {
                        x: Math.max(0, Math.round(ox)),
                        y: Math.max(0, Math.round(oy + i * (th + gap))),
                        w: Math.max(0, colW), h: th
                    }
                }
                return oy + n * th + (n - 1) * gap
            }

            // WEIGHTED LAYOUT: one engine, four presets + a pin.
            //   auto     — screen shares fill the main area, cameras sit in a side
            //              column, audio-only avatars in a bottom strip; with no
            //              share the cameras take the main area (the old behaviour).
            //   gallery  — everyone equal: one mosaic of cams, shares and avatars.
            //   speaker  — one big tile (pinned, else first share, else first cam,
            //              else first avatar); everyone else in the bottom strip.
            //   sidebar  — shares (or, without one, the pinned / first cam) in the
            //              main area; EVERYONE else stacked in the side column.
            // A pinned tile is the main area in every preset except gallery.
            // Returns `slots[]` of {x,y,w,h} per tile index in app.callTiles order
            // (hidden/AFK-cut tiles get a zero slot), plus `.bottom` = the lowest
            // used edge (drives normalH) and `.sideX` = the side column's left edge
            // (0 when there is none) for the drag seam. All geometry is pure
            // arithmetic (no childrenRect reads) so the documented "Flow polish()
            // inside updatePolish()" loop guard is preserved; every size is clamped
            // >= 0 so a transient 0-width grid can't make a negative tile.
            // `hidden` / `mode` / `pinned` / `sideFrac` are passed in (not read off
            // root inside the body) so they are real binding dependencies.
            // The self view floats as a small PiP (bottom-right of the used
            // area) whenever another camera/share is on screen; it is passed to the
            // engine as "hidden" so it claims no grid space, then given its slot here.
            function weightedLayout(W, H, tiles, afk, hidden, mode, pinned, sideFrac) {
                var selfIdx = -1, others = 0, i
                var hid = hidden || []
                for (i = 0; i < tiles.length; i++) {
                    var t = tiles[i]
                    if (!t) continue
                    if (t.id === "vid_selfview") selfIdx = i
                    else if (hid.indexOf(t.id) === -1 && t.kind !== "avatar") others++
                }
                var pip = island.app.selfPipEffective && !afk && selfIdx >= 0 && others > 0   // 10.1 (TH-CALL-24)
                var out = weightedLayoutInner(W, H, tiles, afk, pip ? hid.concat(["vid_selfview"]) : hidden, mode, pinned, sideFrac)
                if (pip && out.length > selfIdx) {
                    var gap = videoGrid.spacing
                    var pw = Math.max(140, Math.min(300, Math.round(W / 5)))
                    var ph = Math.round(pw * 9 / 16)
                    // Bottom-right of the MAIN area: left of a side column, above an
                    // under-band — never over other people's tiles.
                    var rightEdge = out.sideX > 0 ? out.sideX - gap : W
                    var bottomEdge = out.sideY > 0 ? out.sideY - gap : (out.bottom > 0 ? out.bottom : H)
                    // 10.8 (the web's callui/layout.ts clamp): never larger than the area it sits in — the call at its
                    // minimum height (252 px; a tall composer now keeps its height and the call gives way) left a ~90 px
                    // stage and the 16:9 PiP ran down over the control bar
                    if (pw > rightEdge - 2 * gap) { pw = Math.max(0, rightEdge - 2 * gap); ph = Math.round(pw * 9 / 16) }
                    if (ph > bottomEdge - 2 * gap) { ph = Math.max(0, bottomEdge - 2 * gap); pw = Math.round(ph * 16 / 9) }
                    out[selfIdx] = { x: Math.max(0, rightEdge - pw - gap), y: Math.max(0, bottomEdge - ph - gap), w: pw, h: ph, pip: true }
                }
                return out
            }
            function weightedLayoutInner(W, H, tiles, afk, hidden, mode, pinned, sideFrac) {
                var out = []
                var i
                for (i = 0; i < tiles.length; i++) out.push({ x: 0, y: 0, w: 0, h: 0 })
                out.bottom = 0
                out.sideX = 0
                out.sideY = 0
                if (W <= 0 || H <= 0 || tiles.length === 0) return out
                // AFK: only the self-view (index 0) is live → fill the whole area.
                if (afk) {
                    out[0] = { x: 0, y: 0, w: Math.max(0, Math.floor(W)), h: Math.max(0, Math.floor(H)) }
                    out.bottom = Math.max(0, Math.floor(H))
                    return out
                }
                // Classify. Hidden tiles keep their zero slot and claim no space.
                var shares = [], cams = [], avatars = [], all = []
                var hid = hidden || []
                var pinIdx = -1
                for (i = 0; i < tiles.length; i++) {
                    var t = tiles[i]
                    if (!t || hid.indexOf(t.id) !== -1) continue
                    all.push(i)
                    if (pinned && t.id === pinned) pinIdx = i
                    if (t.kind === "avatar") avatars.push(i)
                    else if (t.kind === "screenshare" || t.screenshare === true) shares.push(i)
                    else cams.push(i)
                }
                // Everything hidden → nothing to place (the chip offers "show all").
                if (all.length === 0) return out
                var gap = videoGrid.spacing
                var sideW = Math.max(160, Math.min(Math.round(W * 0.5), Math.round(W * (sideFrac || 0.28))))
                // 10.1 (TH-CALL-09): the band grows one row-height per wrapped row (capped at half
                // the height) so the main area gives way instead of the faces shrinking.
                var stripHFor = function(n) {
                    if (n <= 0) return 0
                    var base = Math.max(96, Math.min(200, Math.round(H * 0.22)))
                    var rows = Math.max(1, videoGrid.stripRowsFor(n, W))
                    return Math.min(Math.round(H * 0.5), rows * base + (rows - 1) * gap)
                }
                var without = function(list, idx) { return list.filter(function(k) { return k !== idx }) }

                if (mode === "gallery") {
                    out.bottom = placeMosaic(out, 0, all, 0, 0, W, H)
                    return out
                }

                if (mode === "speaker" || (pinIdx >= 0 && mode !== "sidebar")) {
                    var main = pinIdx >= 0 ? pinIdx
                        : (shares.length ? shares[0] : (cams.length ? cams[0] : avatars[0]))
                    var rest = without(all, main)
                    var sH = stripHFor(rest.length)
                    var mainH = Math.max(0, H - (sH > 0 ? sH + gap : 0))
                    out[main] = { x: 0, y: 0, w: Math.max(0, Math.floor(W)), h: Math.max(0, Math.floor(mainH)) }
                    var b = mainH
                    if (sH > 0) b = placeStrip(out, rest, 0, mainH + gap, W, sH)
                    out.bottom = Math.max(0, Math.round(b))
                    return out
                }

                // PORTRAIT (0.8.6, a portrait monitor): a side COLUMN is a
                // landscape idea — on a grid taller than wide it squeezed the
                // stream into a corner and hung the cams down the left. When the
                // grid is portrait, the "beside" placements become a band
                // UNDERNEATH the main area instead, sized by the same sideFrac
                // the seam drags (the seam turns horizontal via out.sideY).
                var portrait = H > W
                // Height of the under-band: sideFrac of the area it shares with
                // the main region, kept sane on both ends.
                var bandHFor = function(avail) {
                    return Math.max(110, Math.min(Math.round(avail * 0.45), Math.round(avail * (sideFrac || 0.28))))
                }

                if (mode === "sidebar") {
                    var mains = shares.length ? shares.slice()
                        : (pinIdx >= 0 ? [pinIdx] : (cams.length ? [cams[0]] : []))
                    if (pinIdx >= 0 && mains.indexOf(pinIdx) === -1) mains = [pinIdx]
                    var side = all.filter(function(k) { return mains.indexOf(k) === -1 })
                    if (mains.length === 0) { out.bottom = placeMosaic(out, 0, side, 0, 0, W, H); return out }
                    if (side.length === 0) { out.bottom = placeMosaic(out, 0, mains, 0, 0, W, H); return out }
                    if (portrait) {
                        var bandH = bandHFor(H)
                        var mainH2 = Math.max(0, H - bandH - gap)
                        var mbP = placeMosaic(out, 0, mains, 0, 0, W, mainH2)
                        var sbP = placeStrip(out, side, 0, mainH2 + gap, W, bandH)
                        out.sideY = mainH2 + gap
                        out.bottom = Math.max(0, Math.round(Math.max(mbP, sbP)))
                        return out
                    }
                    var mainW = Math.max(0, W - sideW - gap)
                    var mb = placeMosaic(out, 0, mains, 0, 0, mainW, H)
                    var sb = placeColumn(out, side, mainW + gap, 0, sideW, H)
                    out.sideX = mainW + gap
                    out.bottom = Math.max(0, Math.round(Math.max(mb, sb)))
                    return out
                }

                // auto: streams first — cams beside on a wide grid, cams in a band
                // UNDERNEATH on a portrait one; avatars below either way.
                if (shares.length > 0) {
                    var sH2 = stripHFor(avatars.length)
                    var bodyH = Math.max(0, H - (sH2 > 0 ? sH2 + gap : 0))
                    var b2
                    if (portrait && cams.length > 0) {
                        var camBand = bandHFor(bodyH)
                        var shareH = Math.max(0, bodyH - camBand - gap)
                        placeMosaic(out, 0, shares, 0, 0, W, shareH)
                        placeStrip(out, cams, 0, shareH + gap, W, camBand)
                        out.sideY = shareH + gap
                        b2 = bodyH
                    } else {
                        var mainW2 = cams.length > 0 ? Math.max(0, W - sideW - gap) : W
                        var mb2 = placeMosaic(out, 0, shares, 0, 0, mainW2, bodyH)
                        var cb2 = cams.length > 0 ? placeColumn(out, cams, mainW2 + gap, 0, sideW, bodyH) : 0
                        if (cams.length > 0) out.sideX = mainW2 + gap
                        b2 = Math.max(mb2, cb2, bodyH)
                    }
                    if (sH2 > 0) b2 = placeStrip(out, avatars, 0, bodyH + gap, W, sH2)
                    out.bottom = Math.max(0, Math.round(b2))
                    return out
                }
                // No share: all-audio → one uniform avatar mosaic (clean degrade).
                if (cams.length === 0) {
                    out.bottom = placeMosaic(out, 0, avatars, 0, 0, W, H)
                    return out
                }
                // Cameras in the main area, a bottom filmstrip only when there ARE avatars.
                var sH3 = stripHFor(avatars.length)
                var primH = Math.max(0, H - (sH3 > 0 ? sH3 + gap : 0))
                var b3 = placeMosaic(out, 0, cams, 0, 0, W, primH)
                if (sH3 > 0) b3 = placeStrip(out, avatars, 0, primH + gap, W, sH3)
                out.bottom = Math.max(0, Math.round(b3))
                return out
            }

            // Recomputed whenever size / tile-count / AFK changes. MAXIMIZED fills the
            // explicit viewport height; NORMAL uses a compact derived height (so the
            // ScrollablePage header stays small) — same engine, smaller box.
            readonly property real normalBoxH: {
                // Compact primary region: ~2-up rows of 16:9 at the current grid width,
                // plus the filmstrip band if any avatar tiles are present. Pure arithmetic.
                if (liveCount <= 0) return 0
                var c = liveCount <= 1 ? 1 : 2
                var w = Math.max(0, Math.floor((width - (c - 1) * spacing) / c))
                // Hidden tiles take no space, so the compact height must not
                // reserve rows for them either.
                var shownTiles = island.app.callTiles.filter(function(t){ return t && !island.app.isTileHidden(t.id) })
                var primTiles = island.app.mLocalAfk ? 1 : shownTiles.filter(function(t){ return t.kind !== "avatar" }).length
                var hasAvatar = !island.app.mLocalAfk && shownTiles.some(function(t){ return t.kind === "avatar" })
                var rows = Math.max(1, Math.ceil(Math.max(1, primTiles) / c))
                var h = rows * Math.round(w * 9 / 16) + (rows - 1) * spacing
                if (hasAvatar) h += spacing + Math.max(96, Math.min(160, Math.round(w * 9 / 16 * 0.5)))
                return Math.max(0, h)
            }
            // 10.8 (islands): what the grid wants (the seam share once dragged, else its natural height — the layout
            // engine applies the conversation's minimum now) and what the call island gives it after the chrome and
            // the seam; the tiles are laid out in the smaller of the two
            readonly property real wantBudget: island.app.callPaneFrac >= 0 ? normalCapH : normalBoxH
            readonly property real haveBudget: island.height > 0
                ? Math.max(0, island.height - chromeH - (callSeam.visible ? callSeam.implicitHeight : 0)) : wantBudget
            readonly property real wantH: !visible ? 0 : (weightedLayout(width, wantBudget, island.app.callTiles, island.app.mLocalAfk,
                island.app.mHiddenTiles.concat(island.app.recOnlyHiddenIds), island.app.callLayout, island.app.pinnedTile,
                island.app.callSideFrac).bottom || 0)
            readonly property var layout: weightedLayout(
                width,
                island.app.callMaximized ? availH : Math.min(wantBudget, haveBudget),
                island.app.callTiles,
                island.app.mLocalAfk,
                island.app.mHiddenTiles.concat(island.app.recOnlyHiddenIds),
                island.app.callLayout,
                island.app.pinnedTile,
                island.app.callSideFrac)
            readonly property real normalH: layout.bottom || 0
            Layout.preferredHeight: island.app.callMaximized ? availH : ((island.app.callCollapsed) ? 0 : Math.max(0, normalH))

            // Drag the seam between the main area and the side column —
            // vertical seam on a wide grid, HORIZONTAL seam above the under-band
            // on a portrait grid (0.8.6). Same sideFrac either way.
            Rectangle {
                id: sideSeam
                readonly property real seamX: (videoGrid.layout && videoGrid.layout.sideX) ? videoGrid.layout.sideX : 0
                readonly property real seamY: (videoGrid.layout && videoGrid.layout.sideY) ? videoGrid.layout.sideY : 0
                readonly property bool horizontal: seamY > 0
                visible: (seamX > 0 || seamY > 0) && !island.app.mLocalAfk
                x: horizontal ? 0 : seamX - videoGrid.spacing
                y: horizontal ? seamY - videoGrid.spacing : 0
                width: horizontal ? videoGrid.width : videoGrid.spacing
                height: horizontal ? videoGrid.spacing
                                   : (videoGrid.layout ? Math.max(0, videoGrid.layout.bottom || 0) : 0)
                z: 6
                radius: 2
                color: (seamArea.containsMouse || seamArea.pressed) ? Th.callSideSeamFillHover : "transparent"
                MouseArea {
                    id: seamArea
                    anchors.fill: parent
                    anchors.leftMargin: sideSeam.horizontal ? 0 : -4
                    anchors.rightMargin: sideSeam.horizontal ? 0 : -4
                    anchors.topMargin: sideSeam.horizontal ? -4 : 0
                    anchors.bottomMargin: sideSeam.horizontal ? -4 : 0
                    hoverEnabled: true
                    cursorShape: sideSeam.horizontal ? Qt.SplitVCursor : Qt.SplitHCursor
                    onPositionChanged: (mouse) => {
                        if (!pressed) return
                        var p = mapToItem(videoGrid, mouse.x, mouse.y)
                        if (sideSeam.horizontal) {
                            // Dragging DOWN shrinks the band; the divisor is the
                            // grid's used height so the mapping tracks what the
                            // eye sees. Clamped, so precision is taste not safety.
                            var gh = Math.max(1, (videoGrid.layout && videoGrid.layout.bottom) || videoGrid.height)
                            island.app.callSideFrac = Math.max(0.15, Math.min(0.5, 1 - p.y / gh))
                        } else if (videoGrid.width > 0) {
                            island.app.callSideFrac = Math.max(0.15, Math.min(0.5, 1 - p.x / videoGrid.width))
                        }
                    }
                }
            }

            Repeater {
                id: tileRepeater
                model: island.app.tileModel   // 0.8.28: synced by id from app.callTiles
                delegate: Item {
                    id: tile
                    required property var modelData
                    // C++ (tinhat_video_attach) finds this container by objectName.
                    objectName: modelData.id
                    readonly property bool isSelf: modelData.id === "vid_selfview"
                    // View state. thFillMode is the user's intent:
                    //   "auto" (default for cameras) — fill (cover-crop) when the crop is
                    //   small (aspect mismatch ≤ 35 %), otherwise fit with bars: a 16:9
                    //   camera in a 16:9-ish tile fills, a 4:3 camera in a wide tile
                    //   keeps its head and chin;  "fill" / "fit" — forced.
                    // thZoom is relative to that base: 1 = the base. In fill mode it
                    // may go BELOW 1, down to the fit scale (zoom out until the whole
                    // frame shows — 0.8.22 clamped at 1, which is why "fill couldn't
                    // zoom"); up to 6 either way. The C++ bridge sizes the video item
                    // from thFill/thZoom/pan + thVidW/H (the natural size it
                    // writes from the sink caps); the container clips.
                    property string thFillMode: island.app.defaultFillModeFor(modelData)
                    property real thZoom: 1.0
                    property real thPanX: 0
                    property real thPanY: 0
                    property real thVidW: modelData.vidW || 0
                    property real thVidH: modelData.vidH || 0
                    property bool _viewRestored: false
                    // cover ÷ fit scale: how much of the frame a fill would crop away (1 = none).
                    readonly property real coverRatio: (thVidW > 0 && thVidH > 0 && width > 0 && height > 0)
                        ? Math.max(width / thVidW, height / thVidH) / Math.min(width / thVidW, height / thVidH) : 1
                    readonly property bool thFill: thFillMode === "fill" ? true
                                                   : thFillMode === "fit" ? false
                                                   : coverRatio <= 1.35
                    // Zoom floor: the fit scale (whole frame visible) — below 1 only in fill mode.
                    readonly property real minZoom: thFill ? 1 / coverRatio : 1.0
                    clip: true
                    z: (slot && slot.pip) ? 20 : 0
                    onThZoomChanged: island.app.callctl.relayoutTileVideo(modelData.id)
                    onThPanXChanged: island.app.callctl.relayoutTileVideo(modelData.id)
                    onThPanYChanged: island.app.callctl.relayoutTileVideo(modelData.id)
                    onThFillChanged: island.app.callctl.relayoutTileVideo(modelData.id)
                    onThFillModeChanged: if (_viewRestored) island.app.saveTileView(tile)
                    onThVidWChanged: if (!_viewRestored && thVidW > 0) { _viewRestored = true; island.app.restoreTileView(tile) }
                    readonly property real fitScale: (thVidW > 0 && thVidH > 0 && width > 0 && height > 0)
                        ? Math.min(width / thVidW, height / thVidH) : 0
                    readonly property real viewScale: {
                        if (thVidW <= 0 || thVidH <= 0 || width <= 0 || height <= 0) return 0
                        var base = thFill ? Math.max(width / thVidW, height / thVidH)
                                            : Math.min(width / thVidW, height / thVidH)
                        return base * Math.max(minZoom, thZoom)
                    }
                    // Readout: 100 % = the whole frame visible (the fit scale), whatever the mode.
                    readonly property int zoomPercent: fitScale > 0 ? Math.round(viewScale / fitScale * 100) : 100
                    readonly property real overflowX: viewScale > 0 ? Math.max(0, (thVidW * viewScale - width) / 2) : 0
                    readonly property real overflowY: viewScale > 0 ? Math.max(0, (thVidH * viewScale - height) / 2) : 0
                    function zoomBy(f) {
                        thZoom = Math.max(minZoom, Math.min(6.0, thZoom * f))
                        if (thZoom <= minZoom + 0.001) { thPanX = 0; thPanY = 0 }
                        island.app.saveTileViewLater(tile)
                    }
                    function resetView() { thZoom = 1.0; thPanX = 0; thPanY = 0; island.app.saveTileView(tile) }
                    // --tinhat-zoomprobe (gate-zoom.sh): log the view state periodically.
                    Timer {
                        interval: 2500; repeat: true
                        running: island.app.zoomProbe && tile.visible
                        onTriggered: console.log("[zoomprobe] " + modelData.id + " mode=" + tile.thFillMode
                            + " fill=" + tile.thFill + " percent=" + tile.zoomPercent + " z=" + tile.thZoom.toFixed(2)
                            + " min=" + tile.minZoom.toFixed(2) + " size=" + Math.round(tile.width) + "x" + Math.round(tile.height))
                    }
                    // #4 AFK CUTS BOTH DIRECTIONS: while WE are away, hide every
                    // REMOTE tile (we shouldn't watch the call while shown away).
                    // The self-view (our own camera) stays — it's already off.
                    // …and hidden tiles: the delegate stays alive (video sink is
                    // NOT detached, so bringing it back is instant) but takes no
                    // space — weightedLayout already gave it a zero slot.
                    readonly property bool isHidden: island.app.isTileHidden(modelData.id)
                    visible: (!island.app.mLocalAfk || isSelf) && !isHidden
                    // #7 AFK: this participant marked BRB? (self-view uses our own flag)
                    readonly property bool isAway: tile.isSelf
                        ? island.app.mLocalAfk
                        // mAwayUsers holds bare mxids (@user:server); a remote tile's
                        // participant is the full LiveKit identity @user:server:DEVICE —
                        // match by mxid prefix (cf. the #5 gain path), not exact compare.
                        : island.app.mAwayUsers.some(function(u) {
                            var p = tile.modelData.participant || ""
                            return p === u || p.indexOf(u + ":") === 0
                        })
                    // WEIGHTED layout (Element-style spotlight + filmstrip). videoGrid
                    // computes EVERY tile's absolute slot at once (screenshare / camera =
                    // large primary tiles; audio-only avatars = small bottom filmstrip);
                    // this delegate just reads its own slot by `index`. Works the same in
                    // maximized + normal mode (only the box height differs).
                    required property int index
                    readonly property int liveCount: videoGrid.liveCount
                    // My slot {x,y,w,h} from the central engine (undefined before the grid
                    // has size → fall back to a zero slot; every value clamped >= 0 so a
                    // transient 0-width grid can't produce a negative-sized tile, which
                    // would re-trigger the documented infinite layout polish).
                    readonly property var slot: (videoGrid.layout && index < videoGrid.layout.length)
                        ? videoGrid.layout[index] : null
                    readonly property real tW: slot ? Math.max(0, slot.w) : 0
                    readonly property real tH: slot ? Math.max(0, slot.h) : 0
                    // A hidden (AFK-cut) remote tile reserves no grid space (the engine
                    // already gives non-live tiles a zero slot, but keep the guard).
                    width: !visible ? 0 : Math.max(0, tW)
                    height: !visible ? 0 : Math.max(0, tH)
                    x: !visible ? 0 : Math.max(0, Math.round(slot ? slot.x : 0))
                    y: !visible ? 0 : Math.max(0, Math.round(slot ? slot.y : 0))
                    // Glide between layouts instead of snapping. Off for a
                    // hidden tile — its zero slot must land instantly (the SFU-pause
                    // edge and the C++ video item's queued resize key off final size).
                    // 11.0 (GPU.md §1): the glide is the `long` step and only while the motion switch is on
                    Behavior on x { enabled: island.app.callAnimate && Motion.animate && tile.visible; NumberAnimation { duration: Motion.durLong; easing.type: Easing.OutCubic } }
                    Behavior on y { enabled: island.app.callAnimate && Motion.animate && tile.visible; NumberAnimation { duration: Motion.durLong; easing.type: Easing.OutCubic } }
                    Behavior on width { enabled: island.app.callAnimate && Motion.animate && tile.visible; NumberAnimation { duration: Motion.durLong; easing.type: Easing.OutCubic } }
                    Behavior on height { enabled: island.app.callAnimate && Motion.animate && tile.visible; NumberAnimation { duration: Motion.durLong; easing.type: Easing.OutCubic } }

                    // Adaptive receive: (a) an in-app-hidden tile (AFK cut) is
                    // SFU-paused outright; (b) a visible tile reports its rendered
                    // size (physical px) so the SFU sends the closest simulcast
                    // rung / SVC layer — small grid tiles stop pulling 1080p.
                    // Debounced: the grid animates, so wait for the size to settle
                    // (the streamer also quantizes to layer buckets). Skip the
                    // self-view (not a remote subscription).
                    // ⚠️ A tile a PiP holds must NEVER be SFU-paused: the
                    // pop-out auto-hide makes the grid tile invisible, but the
                    // PiP window is still rendering that very track — pausing
                    // it blanks the pop-out (a field report). _pips is read at
                    // event time (not bound), so plain lookups are safe here.
                    onVisibleChanged: if (!isSelf && !island.app._pips[modelData.id]) island.app.callctl.setTileVideoHidden(modelData.id, !visible)
                    onTWChanged: if (!isSelf) tileSizeSync.restart()
                    onTHChanged: if (!isSelf) tileSizeSync.restart()
                    Component.onDestruction: {
                        island.app._tileDestroyed++
                        if (island.app.tileSyncProbe) console.log("[tilesync] destroyed " + objectName)
                    }
                    Component.onCompleted: {
                        island.app._tileCreated++
                        if (island.app.tileSyncProbe) console.log("[tilesync] created " + modelData.id)
                        if (!isSelf) {
                            // SYMMETRIC sync — not just the !visible case. The
                            // hidden flag is process-global (survives delegate
                            // teardown): room-nav destroys the delegates, their
                            // dying onVisibleChanged pauses every tile at the
                            // SFU, and a delegate recreated already-VISIBLE
                            // never emits an unhide edge — remote videos stayed
                            // SFU-paused for most of an hour (a field log;
                            // the "pop out then close re-shows the video"
                            // was the one path that happened to re-fire the
                            // edge). Setting the CURRENT state clears any stale
                            // pause; the Rust side no-ops when nothing changed.
                            if (!island.app._pips[modelData.id]) island.app.callctl.setTileVideoHidden(modelData.id, !visible)
                            tileSizeSync.restart()
                        }
                    }
                    // Self-healing resync: a visible tile must never sit
                    // SFU-paused. Idempotent (Rust no-ops on no-change, logs
                    // any real transition), so this is free in steady state
                    // and heals any missed unhide edge within 5s.
                    Timer {
                        interval: 5000; repeat: true
                        running: tile.visible && !tile.isSelf
                        onTriggered: if (!island.app._pips[tile.modelData.id]) island.app.callctl.setTileVideoHidden(tile.modelData.id, false)
                    }
                    // Bound on the tile ITEM (not read inside the Timer scope) so a
                    // mixed-DPI multi-monitor setup resolves THIS window's screen —
                    // the Timer's attached Screen can report the wrong monitor's DPR
                    // (one layer bucket off for every tile).
                    readonly property real tileDpr: Screen.devicePixelRatio
                    Timer {
                        id: tileSizeSync
                        interval: 400
                        onTriggered: {
                            if (tile.isSelf || tile.tW <= 0 || tile.tH <= 0) return
                            island.app.callctl.setTileVideoSize(tile.modelData.id,
                                Math.round(tile.tW * tile.tileDpr),
                                Math.round(tile.tH * tile.tileDpr))
                        }
                    }

                    // Per-participant fox/avatar placeholder behind the C++
                    // video item (visible while the stream is still attaching).
                    Rectangle {
                        anchors.fill: parent
                        radius: island.app.radius
                        color: Th.tileBg
                        clip: true
                        // 10.1 (TH-VIS-09): a screen share is not a person — no giant initial.
                        ColumnLayout {
                            anchors.centerIn: parent
                            visible: tile.modelData.screenshare === true
                            spacing: 8
                            ThIcon { Layout.alignment: Qt.AlignHCenter; app: island.app; name: "monitor-symbolic"; width: 44; height: 44; color: Th.tileWaitingFg; isMask: true }
                            Controls.Label { Layout.alignment: Qt.AlignHCenter; text: "Waiting for the first frame…"; color: Th.tileWaitingFg; font.pointSize: Kirigami.Theme.smallFont.pointSize }
                        }
                        Avatar {
                            app: island.app
                            anchors.centerIn: parent
                            visible: tile.modelData.screenshare !== true
                            // Profile pictures were tiny (150 px cap, 56–67 px in the
                            // strip). Scale with the tile, cap high, never wider than fits.
                            size: Math.max(56, Math.min(320, Math.round(Math.min(tile.height * 0.55, tile.width * 0.42))))
                            // The participant's real profile picture, resolved by
                            // call_ctl::refresh_call_names. Empty => the generated fox
                            // below, which used to be EVERYONE (QOL Phase 1.5.0).
                            imageSource: tile.modelData.avatar || ""
                            // Prefer the resolved display name (better initials) over the raw mxid. The
                            // self-view ("You") shows your own initial, as the footer does.
                            name: tile.modelData.participant === "You" ? island.app.mUserId
                                  : ((tile.modelData.name && tile.modelData.name.length > 0)
                                     ? tile.modelData.name : tile.modelData.participant)
                            // Hue + fox face key on the STABLE identity so two distinct participants never
                            // collide while their display names are still "" / unresolved — the person, not
                            // the LiveKit identity's device part, so the tile matches their chat colour (§6).
                            seed: Seeds.avatarSeed(tile.modelData.participant === "You" ? island.app.mUserId
                                                   : tile.modelData.participant)
                        }
                    }

                    // 0.8.30: the video surface the C++ bridge attaches is a plain
                    // rectangle and `clip: true` only clips to the rectangle, so its
                    // corners painted straight over the rounded frame ("content
                    // outside the beveled border"). CornerMask paints the four corner
                    // bites in the grid background above the video and the name strip;
                    // the frame border sits above the bites so its arc stays crisp.
                    CornerMask {
                        radius: island.app.radius
                        color: Th.stageBg
                        z: 50
                    }
                    // Rounded frame (self-view gets an accent ring). 10.1 (TH-CALL-04): a SPEAKING
                    // ring — the cue every other client uses, readable peripherally across nine
                    // tiles. Held ~450 ms after the level drops so it doesn't strobe between
                    // syllables. A border colour change is a one-shot repaint, not per-frame.
                    // 11.0 (GPU.md §2.7): lit only while the window is focused — unfocused it goes (and stays)
                    // unlit after the hold, whatever the level (waveRow.speaking is focus-gated).
                    Rectangle {
                        id: tileFrame
                        anchors.fill: parent
                        radius: island.app.radius
                        color: "transparent"
                        property bool lit: false
                        Timer { id: litRelease; interval: 450; onTriggered: tileFrame.lit = false }
                        Connections {
                            target: waveRow
                            function onSpeakingChanged() { if (waveRow.speaking) { tileFrame.lit = true; litRelease.stop() } else litRelease.restart() }
                        }
                        border.width: lit ? 3 : (tile.isSelf ? 2 : 1)
                        border.color: lit ? Th.tileRingSpeaking : (tile.isSelf ? Th.tileRingOwn : Th.tileRing)
                        Behavior on border.color { ColorAnimation { duration: Motion.dur } }
                        z: 60
                    }

                    // Name overlay (gradient bottom strip).
                    Rectangle {
                        anchors.left: parent.left
                        anchors.right: parent.right
                        anchors.bottom: parent.bottom
                        height: 30
                        z: 3
                        gradient: Gradient {
                            GradientStop { position: 0.0; color: "transparent" }
                            GradientStop { position: 1.0; color: Th.tileNameStripBg }
                        }
                        Row {
                            anchors.left: parent.left
                            anchors.bottom: parent.bottom
                            anchors.leftMargin: 8
                            anchors.bottomMargin: 5
                            spacing: 6
                            // Mic-off glyph — peer's mic muted (or our own when muted).
                            // RBAC §7.4: muted BY A MODERATOR adds a tiny shield + "Muted by <name>".
                            Item {
                                id: micOffGlyph
                                anchors.verticalCenter: parent.verticalCenter
                                readonly property bool srv: tile.isSelf ? island.app.mServerMuted : (tile.modelData.serverMuted === true)
                                readonly property string srvBy: tile.isSelf
                                    ? (island.app.mServerMutedBy.length > 0 ? island.app.mServerMutedBy : "a moderator")
                                    : ((tile.modelData.serverMutedBy && String(tile.modelData.serverMutedBy).length > 0) ? String(tile.modelData.serverMutedBy) : "a moderator")
                                visible: tile.isSelf ? (island.app.mMicMuted || island.app.mServerMuted) : (tile.modelData.muted === true || srv)
                                width: 16; height: 16
                                Kirigami.Icon {
                                    anchors.fill: parent
                                    source: "microphone-sensitivity-muted-symbolic"
                                    color: Th.tileMicOffIcon
                                }
                                ThIcon {
                                    visible: micOffGlyph.srv
                                    app: island.app
                                    name: "security-high-symbolic"
                                    width: 10; height: 10
                                    anchors.right: parent.right; anchors.bottom: parent.bottom
                                    anchors.rightMargin: -4; anchors.bottomMargin: -3
                                    color: Th.tileMicOffIcon
                                }
                                HoverHandler { id: micOffHover; enabled: micOffGlyph.srv }
                                Controls.ToolTip.text: "Muted by " + micOffGlyph.srvBy
                                Controls.ToolTip.visible: micOffGlyph.srv && micOffHover.hovered
                                Controls.ToolTip.delay: 300
                            }
                            Rectangle {
                                anchors.verticalCenter: parent.verticalCenter
                                visible: micOffGlyph.srv && tile.width > 330
                                radius: height / 2
                                implicitHeight: srvPillText.implicitHeight + 4
                                implicitWidth: srvPillText.implicitWidth + 12
                                color: Th.tileServerMutedBg
                                border.width: 1
                                border.color: Th.tileServerMutedBorder
                                Controls.Label {
                                    textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                    id: srvPillText
                                    anchors.centerIn: parent
                                    text: "Muted by " + micOffGlyph.srvBy
                                    color: Th.tileServerMutedFg
                                    font.pointSize: Kirigami.Theme.smallFont.pointSize
                                    font.bold: true
                                }
                            }
                            // Blue mic = in the game's voice chat.
                            Kirigami.Icon {
                                anchors.verticalCenter: parent.verticalCenter
                                visible: tile.isSelf ? (island.app.mGameChat && !island.app.mMicMuted) : island.app.isGameChat(tile.modelData.participant)
                                source: "audio-input-microphone-symbolic"
                                color: Th.tileGameChatIcon
                                implicitWidth: 16; implicitHeight: 16
                                Controls.ToolTip.text: tile.isSelf ? "You're in game chat"
                                    : (island.app.isGameChatMuted(tile.modelData.participant)
                                       ? "In game chat with you — you hear each other in-game, so their mic is muted here"
                                       : "In the game's voice chat")
                                Controls.ToolTip.visible: gcTileHover.hovered
                                HoverHandler { id: gcTileHover }
                            }
                            // 9.4: yellow mic = pulled aside (in a corner). ThIcon, not
                            // Kirigami.Icon: the icon pack ignores Kirigami's recolour, so a
                            // `color` on Kirigami.Icon paints white there (and on Windows).
                            ThIcon {
                                anchors.verticalCenter: parent.verticalCenter
                                app: island.app
                                visible: tile.isSelf ? app.mCorner.active : app.isAside(tile.modelData.participant)
                                name: "audio-input-microphone-symbolic"
                                color: Th.tileAsideIcon
                                width: 16; height: 16
                                Controls.ToolTip.text: tile.isSelf ? "You're aside with " + app.cornerNames(app.mCorner.members)
                                    : (app.inMyCorner(tile.modelData.participant)
                                       ? "Aside with you"
                                       : "Pulled aside with " + app.cornerMatesOf(tile.modelData.participant) + " — you don't hear their mic. Right-click to join them")
                                Controls.ToolTip.visible: asideTileHover.hovered
                                HoverHandler { id: asideTileHover }
                            }
                            // 11.3 same room: green mic = linked with someone in the same room (one voice from that room);
                            // the dot = this mic is the room's live mic right now.
                            ThIcon {
                                id: srTileIcon
                                anchors.verticalCenter: parent.verticalCenter
                                app: island.app
                                readonly property var srg: island.app.sameRoomGroupOf(tile.isSelf ? "" : tile.modelData.participant, island.app.mSameRoom)
                                visible: tile.modelData.screenshare !== true && !!srg
                                name: "audio-input-microphone-symbolic"
                                // live mic = the bright colour, the rest of the room = the green; the icon pack's own
                                // accent part follows the same ink
                                color: (!!srg && srg.live) ? Th.tileSameRoomLive : Th.tileSameRoomIcon
                                accentColor: color
                                width: 16; height: 16
                                Controls.ToolTip.text: island.app.sameRoomTip(tile.isSelf, tile.isSelf ? "" : tile.modelData.participant, island.app.mSameRoom)
                                Controls.ToolTip.visible: srTileHover.hovered
                                Controls.ToolTip.delay: 300
                                HoverHandler { id: srTileHover }
                            }
                            // 10.5 "can they hear me?": what hear.rs knows about THIS person — deafened,
                            // not receiving us (their app's receipt), not reaching us (click to repair),
                            // and their mic sending our own voice back (echo). Nothing when all is well.
                            Item {
                                id: hearGlyphs
                                readonly property var hp: (tile.isSelf || tile.modelData.screenshare === true) ? null : island.app.hearPeer(tile.modelData.participant)
                                readonly property string who: (tile.modelData.name && tile.modelData.name.length > 0) ? tile.modelData.name : island.app.localpart(tile.modelData.participant)
                                readonly property bool deaf: !!(hp && (hp.deaf || hp.away))
                                readonly property bool notMe: !!(hp && !island.app.mMicMuted && hp.notMe && hp.notMe.length > 0)
                                readonly property bool notThem: !!(hp && island.app.hearProblem(hp.iHear))
                                readonly property bool echo: !!(hp && hp.echo)
                                visible: deaf || notMe || notThem || echo
                                anchors.verticalCenter: parent.verticalCenter
                                width: hgRow.implicitWidth; height: 16
                                Row {
                                    id: hgRow
                                    anchors.verticalCenter: parent.verticalCenter
                                    spacing: 4
                                    Item {
                                        visible: hearGlyphs.deaf
                                        width: 16; height: 16
                                        ThIcon { anchors.fill: parent; app: island.app; name: "headphones-symbolic"; color: Th.tileDeafenedIcon; isMask: true }
                                        Rectangle { anchors.centerIn: parent; width: 19; height: 2; radius: 1; rotation: -45; color: Th.tileDeafenedIcon }
                                        HoverHandler { id: deafHover }
                                        Controls.ToolTip.visible: deafHover.hovered
                                        Controls.ToolTip.delay: 300
                                        Controls.ToolTip.text: hearGlyphs.who + ((hearGlyphs.hp && hearGlyphs.hp.away && !hearGlyphs.hp.deaf) ? " is away — they can't hear the call right now" : " is deafened — they can't hear anyone right now")
                                    }
                                    ThIcon {
                                        visible: hearGlyphs.notMe
                                        app: island.app; name: "audio-volume-muted-symbolic"; width: 16; height: 16
                                        color: Th.tileNotReceivingIcon; isMask: true
                                        HoverHandler { id: notMeHover }
                                        Controls.ToolTip.visible: notMeHover.hovered
                                        Controls.ToolTip.delay: 300
                                        Controls.ToolTip.text: hearGlyphs.hp ? hearGlyphs.who + " isn't receiving you — " + app.hearReason(hearGlyphs.hp.notMe, true) : ""
                                    }
                                    ThIcon {
                                        visible: hearGlyphs.notThem
                                        app: island.app; name: "dialog-warning"; width: 16; height: 16
                                        color: Th.tileCantHearIcon; isMask: true
                                        HoverHandler { id: notThemHover; cursorShape: Qt.PointingHandCursor }
                                        TapHandler { onTapped: island.app.callctl.healPeerAudio(tile.modelData.participant) }
                                        Controls.ToolTip.visible: notThemHover.hovered
                                        Controls.ToolTip.delay: 300
                                        Controls.ToolTip.text: hearGlyphs.hp ? "You can't hear " + hearGlyphs.who + " — " + app.hearReason(hearGlyphs.hp.iHear, false) + ". Click to repair." : ""
                                    }
                                    Rectangle {
                                        visible: hearGlyphs.echo
                                        anchors.verticalCenter: parent.verticalCenter
                                        radius: height / 2
                                        width: echoTileText.implicitWidth + 10; height: echoTileText.implicitHeight + 2
                                        color: Th.tileEchoBg
                                        border.width: 1
                                        border.color: Th.tileEchoBorder
                                        Controls.Label {
                                            id: echoTileText
                                            anchors.centerIn: parent
                                            text: "echo"
                                            color: Th.tileEchoFg
                                            font.pointSize: Kirigami.Theme.smallFont.pointSize
                                            font.bold: true
                                        }
                                        HoverHandler { id: echoTileHover }
                                        Controls.ToolTip.visible: echoTileHover.hovered
                                        Controls.ToolTip.delay: 300
                                        Controls.ToolTip.text: (hearGlyphs.hp && hearGlyphs.hp.echo)
                                            ? (hearGlyphs.hp.echo.room
                                               ? hearGlyphs.who + "'s mic picks up your voice in the room — everyone else hears you twice (their app has been told)"
                                               : "You hear yourself through " + hearGlyphs.who + "'s mic, about " + hearGlyphs.hp.echo.lag + " ms later — their app has been told how to fix it")
                                            : ""
                                    }
                                }
                            }
                            Controls.Label {
                                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                anchors.verticalCenter: parent.verticalCenter
                                text: {
                                    var who = tile.isSelf ? "You"
                                        : ((tile.modelData.name && tile.modelData.name.length > 0)
                                           ? tile.modelData.name
                                           : island.app.localpart(tile.modelData.participant))
                                    return tile.modelData.screenshare === true ? (who + " — screen") : who
                                }
                                color: Th.tileNameFg
                                font.bold: true
                                font.pointSize: Kirigami.Theme.smallFont.pointSize
                                // 10.1 (TH-CALL-10): long names elide instead of running off the tile
                                // and shoving the level bars away (room for the glyphs + the zoom pill).
                                width: Math.min(implicitWidth, Math.max(40, tile.width - 8 - 60 - (waveRow.visible ? 43 + 6 : 0) - 70))
                                elide: Text.ElideRight
                            }
                            // #B live waveform: a few small bars whose height tracks
                            // this participant's audio RMS. Overlay-only (z:3 strip),
                            // no layout/pipeline impact. Below a small floor the bars
                            // sit flat (idle / muted). Updated off the 120ms mirror via
                            // mAudioLevels / mMicLevel (app.tileAudioLevel).
                            Row {
                                id: waveRow
                                anchors.verticalCenter: parent.verticalCenter
                                spacing: 2
                                visible: tile.width > 120   // 10.1 (TH-CALL-10): a tiny strip tile keeps the name
                                // The tile's current level (0..1), recomputed when the
                                // mirror updates the maps (each is a binding dependency).
                                // A muted participant never animates as "speaking".
                                readonly property real level: (tile.isSelf && island.app.mMicMuted) || (tile.modelData.muted === true)
                                    ? 0.0
                                    : island.app.tileAudioLevel(tile.modelData.participant, tile.isSelf)
                                // Low floor so normal speech (low RMS) registers; below it the
                                // bars rest flat (idle / muted background noise).
                                readonly property real floor: 0.02
                                // the speaking ring's input: live, and only while the window is focused (GPU.md §2.7)
                                readonly property bool speaking: Motion.focused && level > floor
                                readonly property real maxBar: 18
                                readonly property real minBar: 2
                                // 11.0 (GPU.md §2.7, R13 — the web's Tile.tsx Waveform + ui/meter.ts): the bars change
                                // only on this strip's own ≤ 10/s tick, with the level SAMPLED at the tick (the 120 ms
                                // mirror draws nothing new), eased by half the gap while the motion switch is on,
                                // snapped under Reduce motion; flat and no tick while the window is not focused or the
                                // person is quiet (it ran a 30 Hz tick; one talking tile repainted ≈ 28×/s).
                                property real shown: 0.0       // the eased amplitude
                                property real sampled: 0.0     // the level at the last tick (0 = rest)
                                readonly property bool lit: sampled > floor
                                function amp(l) { return l > waveRow.floor ? Math.pow(Math.min(1.0, (l - waveRow.floor) / (0.5 - waveRow.floor)), 0.6) : 0 }
                                function rest() { waveTick.stop(); waveRow.shown = 0; waveRow.sampled = 0 }
                                function tick() {
                                    if (!Motion.focused || !waveRow.visible) { waveRow.rest(); return }
                                    var target = waveRow.amp(waveRow.level), d = target - waveRow.shown
                                    waveRow.shown = Motion.animate ? (Math.abs(d) < 0.015 ? target : waveRow.shown + d * 0.5) : target
                                    waveRow.sampled = waveRow.level
                                    if (waveRow.shown === 0 && waveRow.level <= waveRow.floor) waveTick.stop()
                                }
                                function wake() {
                                    if (waveTick.running) return
                                    if (!Motion.focused || !waveRow.visible) { waveRow.rest(); return }
                                    if (waveRow.level <= waveRow.floor && waveRow.shown === 0) { waveRow.rest(); return }
                                    waveRow.tick()
                                    if (!(waveRow.shown === 0 && waveRow.level <= waveRow.floor)) waveTick.start()
                                }
                                onLevelChanged: waveRow.wake()
                                onVisibleChanged: waveRow.wake()
                                Connections {
                                    target: Motion
                                    function onWinStateChanged() { if (Motion.focused) waveRow.wake(); else waveRow.rest() }
                                    function onAnimateChanged() { waveRow.wake() }
                                }
                                Timer { id: waveTick; interval: 100; repeat: true; onTriggered: waveRow.tick() }   // WAVE_TICK_MS
                                Repeater {
                                    model: 9
                                    delegate: Rectangle {
                                        required property int index
                                        width: 3
                                        radius: 1.5
                                        // Accent while speaking so it reads as "active".
                                        color: waveRow.lit ? Th.tileWaveFillSpeaking : Th.tileWaveFill
                                        // 10.1 (TH-VIS-06): at rest the bars all but vanish — nine dim dashes
                                        // after every name read as a rendering fault in screenshots.
                                        opacity: waveRow.lit ? 0.95 : 0.10
                                        anchors.verticalCenter: parent.verticalCenter
                                        // Bell envelope (taller centre) × a per-bar ripple seeded
                                        // by index + level, so the 9 bars dance like a spectrum
                                        // instead of all moving in lockstep (we only have a scalar
                                        // RMS, so the ripple fakes per-band variation).
                                        readonly property real bell: 1.0 - Math.abs(index - 4) * 0.13
                                        readonly property real ripple:
                                            0.6 + 0.4 * Math.abs(Math.sin(index * 1.3 + waveRow.sampled * 22.0))
                                        height: waveRow.minBar + Math.min(1, waveRow.shown) * bell * ripple
                                                * (waveRow.maxBar - waveRow.minBar)
                                    }
                                }
                            }
                        }
                    }

                    // #7 AFK: dim the tile + show a 💤 glyph when the participant
                    // is marked away/BRB. Purely an overlay (no pipeline impact).
                    Rectangle {
                        anchors.fill: parent
                        radius: island.app.radius
                        visible: tile.isAway
                        color: Th.tileAwayBg
                        z: 4
                        Controls.Label {
                            anchors.centerIn: parent
                            text: "💤"
                            font.pointSize: Math.max(20, Math.round(tile.height * 0.18))
                        }
                    }

                    // #9 PiP: a pop-out button on every REMOTE tile (video OR
                    // screenshare) that floats the stream in an always-on-top window
                    // (closing returns it to the grid). PERSISTENT (not hover-only —
                    // it was undiscoverable) at half opacity, brightening on hover.
                    // Not for the self-view or screenshot mode, and hidden on tiny tiles.
                    HoverHandler { id: tileHover }
                    // Ctrl+wheel zooms, drag pans while the video overflows the
                    // tile, double-click resets. A transparent layer ABOVE the video item —
                    // the GstGL item swallows pointer events otherwise — and below the
                    // buttons (z 5).
                    Item {
                        id: viewLayer
                        anchors.fill: parent
                        z: 2
                        WheelHandler {
                            acceptedModifiers: Qt.ControlModifier
                            onWheel: (ev) => {
                                // Some input stacks (Wayland touchpads, high-res wheels)
                                // deliver pixelDelta with angleDelta 0 — a zero step must
                                // be a no-op, not a zoom-out (the old `> 0 ? in : out`).
                                var dy = ev.angleDelta.y !== 0 ? ev.angleDelta.y : ev.pixelDelta.y
                                if (dy === 0) return
                                tile.zoomBy(dy > 0 ? 1.15 : 1 / 1.15)
                            }
                        }
                        DragHandler {
                            id: panDrag
                            target: null
                            enabled: tile.overflowX > 0 || tile.overflowY > 0
                            cursorShape: Qt.ClosedHandCursor
                            property real startX: 0
                            property real startY: 0
                            onActiveChanged: {
                                if (active) { startX = tile.thPanX; startY = tile.thPanY }
                                else island.app.saveTileViewLater(tile)
                            }
                            onTranslationChanged: {
                                if (!active) return
                                if (tile.overflowX > 0) tile.thPanX = Math.max(-1, Math.min(1, startX - translation.x / tile.overflowX))
                                if (tile.overflowY > 0) tile.thPanY = Math.max(-1, Math.min(1, startY - translation.y / tile.overflowY))
                            }
                        }
                        TapHandler { onDoubleTapped: tile.resetView() }
                        // 0.8.31: right-click (or long-press) → this person's volumes.
                        TapHandler {
                            acceptedButtons: Qt.RightButton
                            gesturePolicy: TapHandler.ReleaseWithinBounds
                            onTapped: (eventPoint, button) => island.app.openTileVolumeMenu(tile, eventPoint.position)
                        }
                        TapHandler {
                            acceptedButtons: Qt.LeftButton
                            onLongPressed: island.app.openTileVolumeMenu(tile, point.position)
                        }
                        // Zoom pill — "− NNN% +". Shown on hover or while zoomed
                        // away from the default; 100 % = the whole frame visible. The
                        // buttons make zoom reachable without Ctrl+scroll (which some
                        // input stacks never deliver) and show the crop at a glance.
                        Rectangle {
                            id: zoomPill
                            readonly property bool zoomed: Math.abs(tile.thZoom - 1.0) > 0.001
                            visible: island.app.callChromeShown && tile.fitScale > 0 && (tileHover.hovered || zoomed) && tile.width > 140 && tile.height > 60
                            anchors { right: parent.right; bottom: parent.bottom; margins: 8 }
                            implicitHeight: 22
                            implicitWidth: zoomRow.implicitWidth + 10
                            radius: 11
                            color: Th.tileZoomPillBg
                            border.width: 1
                            border.color: Th.tileZoomPillBorder
                            opacity: tileHover.hovered ? 1.0 : 0.65
                            Row {
                                id: zoomRow
                                anchors.centerIn: parent
                                spacing: 2
                                Controls.Label {
                                    width: 18; height: 20
                                    text: "−"
                                    color: zoomOutTap.pressed ? Th.tileZoomPillFgPressed : Th.tileZoomPillFg
                                    horizontalAlignment: Text.AlignHCenter; verticalAlignment: Text.AlignVCenter
                                    font.pointSize: Kirigami.Theme.smallFont.pointSize + 2
                                    opacity: tile.thZoom > tile.minZoom + 0.001 ? 1.0 : 0.35
                                    TapHandler { id: zoomOutTap; onTapped: tile.zoomBy(1 / 1.15) }
                                }
                                Controls.Label {
                                    textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                    id: zoomLbl
                                    height: 20
                                    text: (Math.round(tile.zoomPercent / 5) * 5) + "%"
                                    color: Th.tileZoomPillFg
                                    verticalAlignment: Text.AlignVCenter
                                    font.pointSize: Kirigami.Theme.smallFont.pointSize
                                    TapHandler { onDoubleTapped: tile.resetView() }
                                }
                                Controls.Label {
                                    width: 18; height: 20
                                    text: "+"
                                    color: zoomInTap.pressed ? Th.tileZoomPillFgPressed : Th.tileZoomPillFg
                                    horizontalAlignment: Text.AlignHCenter; verticalAlignment: Text.AlignVCenter
                                    font.pointSize: Kirigami.Theme.smallFont.pointSize + 2
                                    opacity: tile.thZoom < 5.999 ? 1.0 : 0.35
                                    TapHandler { id: zoomInTap; onTapped: tile.zoomBy(1.15) }
                                }
                            }
                        }
                    }
                    // Self view: swap between the small PiP and a normal grid tile.
                    // 0.8.30: every tile control is a ThToolButton — the pack's glyph,
                    // white on video, with its hover story (the pin / listen ones were
                    // typeset 📌 🎧 because breeze had no glyph; the pack does).
                    ThToolButton {
                        visible: tile.isSelf && tile.width > 100 && tile.height > 50 && !app.mLocalAfk
                        anchors.top: parent.top
                        anchors.right: parent.right
                        anchors.margins: 6
                        z: 5
                        opacity: app.callChromeShown ? ((tileHover.hovered || hovered) ? 1.0 : 0.5) : 0.0
                        enabled: app.callChromeShown
                        Behavior on opacity { NumberAnimation { duration: island.app.dur } }
                        implicitWidth: 28
                        implicitHeight: 28
                        app: island.app
                        iconName: app.selfPip ? "view-fullscreen-symbolic" : "view-restore-symbolic"
                        iconSize: 16
                        iconColor: Th.tileButtonIcon
                        background: Rectangle {
                            radius: 6
                            color: parent.hovered ? Th.tileButtonBgHover : Th.tileButtonBg
                        }
                        tip: app.selfPip ? "Show my camera as a normal tile" : "Shrink my camera into a corner"
                        onClicked: app.setSelfViewMode(app.selfPip ? "grid" : "pip")
                    }
                    ThToolButton {
                        anchors.top: parent.top
                        anchors.right: parent.right
                        anchors.margins: 6
                        z: 5
                        visible: !tile.isSelf && !app.fakeCall && tile.width > 80 && tile.height > 60
                        opacity: app.callChromeShown ? ((tileHover.hovered || hovered) ? 1.0 : 0.5) : 0.0
                        enabled: app.callChromeShown
                        Behavior on opacity { NumberAnimation { duration: island.app.dur } }
                        implicitWidth: 28
                        implicitHeight: 28
                        app: island.app
                        iconName: "window-new"
                        iconSize: 16
                        iconColor: Th.tileButtonIcon
                        background: Rectangle {
                            radius: 6
                            color: parent.hovered ? Th.tileButtonBgHover : Th.tileButtonBg
                        }
                        tip: "Pop out into its own window"
                        onClicked: app.openPip(tile.modelData.id, tile.modelData.participant, tile.modelData.name || "", tile.modelData.screenshare === true)
                    }
                    // 10.1 (TH-FLOAT-04): the per-person menu (volume, pull aside…) with a VISIBLE
                    // handle — a right-click-only menu was the discoverability bug.
                    ThToolButton {
                        anchors.top: parent.top
                        anchors.right: parent.right
                        anchors.margins: 6
                        anchors.rightMargin: 176
                        z: 5
                        visible: !tile.isSelf && tile.width > 330 && tile.height > 60
                        opacity: app.callChromeShown ? ((tileHover.hovered || hovered) ? 1.0 : 0.5) : 0.0
                        enabled: app.callChromeShown
                        Behavior on opacity { NumberAnimation { duration: island.app.dur } }
                        implicitWidth: 28
                        implicitHeight: 28
                        app: island.app
                        iconName: "overflow-menu-symbolic"
                        iconSize: 16
                        iconColor: Th.tileButtonIcon
                        background: Rectangle {
                            radius: 6
                            color: parent.hovered ? Th.tileButtonBgHover : Th.tileButtonBg
                        }
                        tip: "More — their volume, pull aside…"
                        onClicked: app.openTileVolumeMenu(tile, Qt.point(x, y + height))
                    }

                    // Hide this tile — sits left of the pop-out button, same treatment
                    // (persistent at half opacity; hover brightens). VIEW-ONLY: it gives
                    // the tile no grid space, it does NOT mute the person or stop their
                    // stream. The "N hidden · Show all" chip in the call header is the
                    // way back. Never on the self-view — turning off your own preview is
                    // what the camera button is for.
                    ThToolButton {
                        anchors.top: parent.top
                        anchors.right: parent.right
                        anchors.rightMargin: 40
                        anchors.topMargin: 6
                        z: 5
                        // Shown in fakeCall too (unlike the pop-out): that mode is the
                        // only way to drive the grid in automated UI verification, and a
                        // control that cannot be exercised there cannot be gate-tested.
                        visible: !tile.isSelf && tile.width > 80 && tile.height > 60
                        opacity: app.callChromeShown ? ((tileHover.hovered || hovered) ? 1.0 : 0.5) : 0.0
                        enabled: app.callChromeShown
                        Behavior on opacity { NumberAnimation { duration: island.app.dur } }
                        app: island.app
                        iconName: "layer-visible-off-symbolic"
                        // Explicit size + white icon: the tile behind is dark video,
                        // where a theme-coloured icon at default size is nearly
                        // invisible and a poor click target.
                        implicitWidth: 28
                        implicitHeight: 28
                        iconSize: 16
                        iconColor: Th.tileButtonIcon
                        background: Rectangle {
                            radius: 6
                            color: parent.hovered ? Th.tileButtonBgHover : Th.tileButtonBg
                        }
                        tip: tile.modelData.screenshare === true
                            ? "Hide this screen share (they keep sharing) — remembered for future calls"
                            : "Hide this tile (they stay in the call) — remembered for future calls"
                        onClicked: app.hideTileRemember(tile.modelData)
                    }

                    // Listen toggle for streams — audio plays only while WATCHED
                    // (pinned, popped out, the only stream, or this toggle). Accent when
                    // audible. 0.8.30: the pack's headphones (was a typeset 🎧).
                    ThToolButton {
                        anchors.top: parent.top
                        anchors.right: parent.right
                        anchors.rightMargin: 142
                        anchors.topMargin: 6
                        z: 5
                        visible: !tile.isSelf && app.isShareTile(tile.modelData) && tile.width > 200 && tile.height > 60
                        opacity: app.callChromeShown ? ((tileHover.hovered || hovered || app.isWatched(tile.modelData.id)) ? 1.0 : 0.5) : 0.0
                        enabled: app.callChromeShown
                        Behavior on opacity { NumberAnimation { duration: island.app.dur } }
                        implicitWidth: 28
                        implicitHeight: 28
                        app: island.app
                        iconName: "headphones-symbolic"
                        iconSize: 16
                        iconColor: app.isWatched(tile.modelData.id) ? Th.tileButtonIconActive : Th.tileButtonIcon
                        active: app.isWatched(tile.modelData.id)
                        background: Rectangle {
                            radius: 6
                            color: island.app.isWatched(tile.modelData.id)
                                ? Th.tileButtonBgActive
                                : (parent.hovered ? Th.tileButtonBgHover : Th.tileButtonBg)
                        }
                        tip: app.isWatched(tile.modelData.id)
                            ? (app.isListening(tile.modelData.id) ? "Listening — click to stop" : "You hear this stream (you're watching it)")
                            : "Listen to this stream"
                        onClicked: app.setListening(tile.modelData.id, !app.isListening(tile.modelData.id))
                    }
                    // Several streams on screen — this one's audio is off until chosen.
                    Rectangle {
                        visible: !tile.isSelf && island.app.isShareTile(tile.modelData) && !island.app.isWatched(tile.modelData.id) && tile.width > 220
                        anchors { top: parent.top; left: parent.left; margins: 6 }
                        anchors.topMargin: island.app.isTunedIn(tile.modelData.id) ? 36 : 6
                        z: 5
                        implicitHeight: 22
                        implicitWidth: nwLabel.implicitWidth + 16
                        radius: 11
                        color: Th.tileNotListeningBg
                        border.width: 1; border.color: Th.tileNotListeningBorder
                        Row {
                            id: nwLabel
                            anchors.centerIn: parent
                            spacing: 5
                            ThIcon {   // 0.8.30: pack glyph (was a typeset 🔇)
                                anchors.verticalCenter: parent.verticalCenter
                                width: 14; height: 14
                                app: island.app
                                name: "audio-volume-muted-symbolic"
                                color: Th.tileNotListeningFg
                                isMask: true
                            }
                            Controls.Label {
                                anchors.verticalCenter: parent.verticalCenter
                                visible: tile.width > 430   // 10.1 (TH-CALL-11): icon-only when the button row needs the space
                                text: "not listening · click to listen"
                                color: Th.tileNotListeningFg
                                font.pointSize: Kirigami.Theme.smallFont.pointSize
                            }
                        }
                        TapHandler { onTapped: island.app.setListening(tile.modelData.id, true) }
                    }

                    // Pin: make this tile the main view in every preset except
                    // gallery (press again to unpin). 0.8.30: the pack's pin (was 📌).
                    ThToolButton {
                        anchors.top: parent.top
                        anchors.right: parent.right
                        anchors.rightMargin: 108
                        anchors.topMargin: 6
                        z: 5
                        visible: !tile.isSelf && tile.width > 160 && tile.height > 60
                        opacity: app.callChromeShown ? ((tileHover.hovered || hovered || app.pinnedTile === tile.modelData.id) ? 1.0 : 0.5) : 0.0
                        enabled: app.callChromeShown
                        Behavior on opacity { NumberAnimation { duration: island.app.dur } }
                        implicitWidth: 28
                        implicitHeight: 28
                        app: island.app
                        iconName: "window-pin-symbolic"
                        iconSize: 16
                        iconColor: app.pinnedTile === tile.modelData.id ? Th.tileButtonIconActive : Th.tileButtonIcon
                        active: app.pinnedTile === tile.modelData.id
                        background: Rectangle {
                            radius: 6
                            color: island.app.pinnedTile === tile.modelData.id
                                ? Th.tileButtonBgActive
                                : (parent.hovered ? Th.tileButtonBgHover : Th.tileButtonBg)
                        }
                        tip: app.pinnedTile === tile.modelData.id ? "Unpin" : "Pin as the main view"
                        onClicked: app.pinnedTile = (app.pinnedTile === tile.modelData.id) ? "" : tile.modelData.id
                    }

                    // Receive-quality dropdown (YouTube-style):
                    // pick the simulcast/SVC layer the SFU forwards for THIS tile.
                    // Sits left of the hide button, same persistent-at-half-opacity
                    // treatment. "Auto" = tile-size steering (the default); a manual
                    // pick beats it and — unlike auto — also applies to screenshares
                    // (explicit choice, not accidental blur). Never on the self-view:
                    // the local preview isn't SFU-forwarded, there's nothing to pick.
                    ThToolButton {
                        id: qualityBtn
                        anchors.top: parent.top
                        anchors.right: parent.right
                        anchors.rightMargin: 74
                        anchors.topMargin: 6
                        z: 5
                        // Visible in fakeCall for the same reason as the hide button:
                        // the automated GUI gate can only drive what it can see.
                        visible: !tile.isSelf && tile.width > 120 && tile.height > 60
                        opacity: app.callChromeShown ? ((tileHover.hovered || hovered || qualityMenu.visible) ? 1.0 : 0.5) : 0.0
                        enabled: app.callChromeShown
                        Behavior on opacity { NumberAnimation { duration: island.app.dur } }
                        implicitWidth: 28
                        implicitHeight: 28
                        app: island.app
                        iconName: "monitor-symbolic"
                        iconSize: 16
                        iconColor: Th.tileButtonIcon
                        active: qualityMenu.visible
                        background: Rectangle {
                            radius: 6
                            color: parent.hovered ? Th.tileButtonBgHover : Th.tileButtonBg
                        }
                        tip: "Stream controls (quality · volume · mute)"
                        Controls.ToolTip.visible: hovered && !qualityMenu.visible
                        onClicked: {
                            qualityMenu.current = app.callctl.tileQuality(tile.modelData.id)
                            // Seed volume/mute from the SAME per-(person x category)
                            // store the strip's right-click menu uses (#5) — one
                            // backend, two surfaces. The user reported the strip
                            // path failing; this surface exercises it directly.
                            if (qualityMenu.volMxid.length > 0) {
                                qualityMenu.vol = parseFloat(app.audioPrefsCtl.volumeFor(qualityMenu.volMxid, qualityMenu.volCat))
                                qualityMenu.muted = app.audioPrefsCtl.mutedFor(qualityMenu.volMxid, qualityMenu.volCat)
                            }
                            qualityMenu.popup(qualityBtn, 0, qualityBtn.height + 2)
                        }
                        // Per-stream control menu (YouTube-style): volume + mute up
                        // top, layer picker below, then the receiver-side kill.
                        // A ThMenu (fix/sliders-volume): as wide as its rows — Basic's 200 px clipped the volume row.
                        ThMenu {
                            id: qualityMenu
                            modal: true
                            margins: 6   // keep it on-screen
                            // Re-read on every open — the picks live in the streamer
                            // and the audio-prefs store, not in QML.
                            property string current: "auto"
                            // BARE mxid (tileMxid), the same key the left bar's menu
                            // writes: the full LK identity carried a per-login device
                            // id, so the two surfaces saved different prefs.
                            readonly property string volMxid: island.app.tileMxid(tile.modelData.participant || "")
                            // A screenshare tile's audio is the "screenshare" gain
                            // category; a camera/person tile is their "mic".
                            readonly property string volCat: tile.modelData.screenshare === true ? "screenshare" : "mic"
                            property real vol: 1.0
                            property bool muted: false

                            CheckMenuItem { // listen toggle
                                visible: island.app.isShareTile(tile.modelData)
                                text: "Listen to this stream (audio plays only while watched)"
                                isChecked: () => island.app.isWatched(tile.modelData.id)
                                onTriggered: island.app.setListening(tile.modelData.id, !island.app.isListening(tile.modelData.id))
                            }
                            Controls.MenuSeparator { visible: island.app.isShareTile(tile.modelData) }
                            MenuSectionLabel { app: island.app; text: "Volume" }
                            RowLayout {
                                // Layout.* margins are ignored for RowLayout children
                                // of Controls.Menu — inset with explicit spacer Items
                                // (same pattern as the strip's volume menu).
                                spacing: Kirigami.Units.smallSpacing
                                Item { Layout.preferredWidth: Kirigami.Units.smallSpacing }
                                ThSlider {
                                    callHandle: true   // the web's stream menu (menus.module.css .volSlider)
                                    Layout.preferredWidth: 150
                                    from: 0.0; to: 2.0
                                    stepSize: 0.01   // the web's dial: whole percents (a key / a wheel notch = 1 %)
                                    snapMode: Controls.Slider.SnapAlways
                                    wheelEnabled: true   // as Plasma's style and the web (Basic ignored the wheel)
                                    Kirigami.StyleHints.tickMarkStepSize: -1   // no Plasma tick row (a tick per step)
                                    enabled: !qualityMenu.muted && qualityMenu.volMxid.length > 0
                                    value: qualityMenu.vol
                                    // Live-push while dragging (no disk write); commit
                                    // once on release — the proven #5 pattern. A wheel
                                    // notch has no release: commit it at once.
                                    onMoved: {
                                        qualityMenu.vol = value
                                        if (qualityMenu.volMxid.length > 0) {
                                            island.app.audioPrefsCtl.pushVolume(qualityMenu.volMxid, qualityMenu.volCat, value)
                                            if (!pressed) island.app.audioPrefsCtl.setVolume(qualityMenu.volMxid, qualityMenu.volCat, value)
                                        }
                                    }
                                    onPressedChanged: {
                                        if (!pressed && qualityMenu.volMxid.length > 0)
                                            island.app.audioPrefsCtl.setVolume(qualityMenu.volMxid, qualityMenu.volCat, value)
                                    }
                                }
                                Controls.Label {
                                    textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                    Layout.preferredWidth: 38
                                    text: Math.round(qualityMenu.vol * 100) + "%"
                                    color: Th.callVolumeValueFg
                                    horizontalAlignment: Text.AlignRight
                                }
                                Controls.ToolButton {
                                    icon.name: qualityMenu.muted ? "audio-volume-muted-symbolic" : "audio-volume-high-symbolic"
                                    Controls.ToolTip.text: qualityMenu.muted ? "Unmute this stream" : "Mute this stream (just for you)"
                                    Controls.ToolTip.visible: hovered
                                    Controls.ToolTip.delay: 400
                                    onClicked: {
                                        if (qualityMenu.volMxid.length > 0) {
                                            qualityMenu.muted = !qualityMenu.muted
                                            island.app.audioPrefsCtl.setMuted(qualityMenu.volMxid, qualityMenu.volCat, qualityMenu.muted)
                                        }
                                    }
                                }
                                Item { Layout.preferredWidth: Kirigami.Units.smallSpacing }
                            }

                            Controls.MenuSeparator {}
                            MenuSectionLabel { app: island.app; text: "Quality" }
                            Repeater {
                                model: [
                                    { label: "Auto",  mode: "auto" },
                                    { label: "1080p", mode: "1080" },
                                    { label: "720p",  mode: "720" },
                                    { label: "360p",  mode: "360" },
                                    { label: "180p",  mode: "180" }
                                ]
                                delegate: CheckMenuItem {
                                    required property var modelData
                                    text: modelData.label
                                    isChecked: () => qualityMenu.current === modelData.mode
                                    onTriggered: {
                                        island.app.callctl.applyTileQuality(tile.modelData.id, modelData.mode)
                                        qualityMenu.current = modelData.mode
                                    }
                                }
                            }

                            Controls.MenuSeparator {}
                            MenuSectionLabel { app: island.app; text: "View" }
                            CheckMenuItem {
                                text: "Auto — fill when the crop is small"
                                isChecked: () => tile.thFillMode === "auto"
                                onTriggered: tile.thFillMode = "auto"
                            }
                            CheckMenuItem {
                                text: "Fill the tile (crop, no bars)"
                                isChecked: () => tile.thFillMode === "fill"
                                onTriggered: tile.thFillMode = "fill"
                            }
                            CheckMenuItem {
                                text: "Fit the whole picture (bars)"
                                isChecked: () => tile.thFillMode === "fit"
                                onTriggered: tile.thFillMode = "fit"
                            }
                            Controls.MenuItem {
                                text: "Reset zoom && position"
                                enabled: Math.abs(tile.thZoom - 1.0) > 0.001 || tile.thPanX !== 0 || tile.thPanY !== 0
                                onTriggered: tile.resetView()
                            }
                            Controls.MenuItem {
                                enabled: false
                                text: "Ctrl+scroll or the − / + pill zooms · drag pans · double-click resets"
                                font.pointSize: Kirigami.Theme.smallFont.pointSize
                            }
                            Controls.MenuSeparator {}
                            Controls.MenuItem {
                                // The receiver-side kill: same lever as the tile's
                                // hide button — SFU stops forwarding (a real
                                // bandwidth stop), they keep streaming, and the
                                // "N hidden · Show all" chip is the way back.
                                text: "Hide stream (stops its bandwidth)"
                                icon.name: "layer-visible-off-symbolic"
                                onTriggered: island.app.hideTileRemember(tile.modelData)
                            }
                        }
                    }

                    // This is someone's recording-only stream we tuned in to.
                    Rectangle {
                        visible: island.app.isTunedIn(tile.modelData.id) && tile.width > 160
                        anchors { top: parent.top; left: parent.left; margins: 6 }
                        z: 5
                        implicitHeight: 24
                        implicitWidth: tunedRow.implicitWidth + 16
                        radius: 12
                        color: Th.tileTunedInBg
                        border.width: 1; border.color: Th.tileTunedInBorder
                        RowLayout {
                            id: tunedRow
                            anchors.centerIn: parent
                            spacing: 6
                            Controls.Label { text: tile.width > 430 ? "● recording-only · tuned in" : "● tuned in"; color: Th.tileTunedInFg; font.bold: true; font.pointSize: Kirigami.Theme.smallFont.pointSize }   // 10.1 (TH-CALL-11)
                            Controls.Label {
                                text: "Leave"; color: Th.tileTunedInLeaveFg; font.pointSize: Kirigami.Theme.smallFont.pointSize; font.underline: true
                                TapHandler { onTapped: island.app.callctl.setTileTunedIn(tile.modelData.id, false) }
                            }
                        }
                    }
                    // True once the native gst sink is bound to this tile.
                    property bool vidAttached: false

                    // #9 PiP note: popping this tile out needs NO grid-side release —
                    // the pop-out is an independent CPU viewer on the same track
                    // (PipWindow.qml), so this tile's qml6glsink keeps playing.

                    // Retry the native attach until the sink binds (skipped in
                    // the fake-call screenshot mode, where there's no pipeline).
                    // The self-view tile is always present (avatar placeholder
                    // when the camera's off), so it only polls while the camera
                    // is on — and re-polls each time it's toggled back on.
                    Timer {
                        interval: 120
                        repeat: true
                        // Avatar tiles (camera-off peers) have NO pipeline to attach,
                        // so never poll for them — else N voice-only peers spin N
                        // perpetual 8 Hz no-op attach timers.
                        running: !island.app.fakeCall && !tile.vidAttached
                                 && tile.modelData.kind !== "avatar"
                                 && (!tile.isSelf || island.app.callctl.cameraOn)
                        onTriggered: if (island.app.callctl.attachVideo(tile.objectName)) tile.vidAttached = true
                    }
                    // Self-view only: when the camera turns off the Rust side
                    // tears the pipeline down + removes the GL surface, so clear
                    // the attached flag to re-arm the attach loop for next time.
                    Connections {
                        target: island.app.callctl
                        enabled: tile.isSelf
                        function onCameraOnChanged() {
                            if (!island.app.callctl.cameraOn) tile.vidAttached = false
                        }
                    }
                }
            }
        }

        // Empty state: in a call but no video streams yet.
        Controls.Label {
            Layout.fillWidth: true
            Layout.topMargin: Kirigami.Units.largeSpacing
            horizontalAlignment: Text.AlignHCenter
            visible: island.app.callShown && island.app.callTiles.length === 0
            text: island.app.mConnecting ? "Connecting to the call…"
                : "You're in the call. Turn on your camera or wait for others."
            color: Th.callEmptyFg
            wrapMode: Text.WordWrap
        }

        // Floating control bar (per request): mute · deafen · camera · share ·
        // BRB · hang-up. Device pickers + soundboard + noise-cancel moved to the
        // header Settings ▾ / composer, keeping the bar to the six core actions.
        Rectangle {
            id: callControlBar
            Layout.alignment: Qt.AlignHCenter
            Layout.topMargin: Kirigami.Units.smallSpacing
            implicitHeight: 64
            implicitWidth: ctlRow.implicitWidth + Kirigami.Units.largeSpacing * 2
            // 10.1 (TH-CALL-05): twelve buttons + a now-playing pill can want ~900 px; in a
            // narrower column the centred bar used to overflow BOTH edges. Scale it down
            // to fit (down to 0.7) instead — every action stays reachable.
            scale: parent ? Math.max(0.7, Math.min(1, (parent.width - Kirigami.Units.largeSpacing) / Math.max(1, implicitWidth))) : 1
            transformOrigin: Item.Center
            radius: height / 2
            color: Th.callbarBg
            border.width: 1
            border.color: Th.callbarBorder
            // 0.8.33: hidden until the window is focused and the pointer is over the call. 11.0.2: or pinned (callBarShown:
            // "Keep call buttons visible" keeps it, its pin below)
            opacity: island.app.callBarShown ? 1.0 : 0.0
            enabled: island.app.callBarShown
            Behavior on opacity { NumberAnimation { duration: island.app.dur } }
            transform: Translate {
                y: island.app.callBarShown ? 0 : 10
                Behavior on y { NumberAnimation { duration: island.app.dur; easing.type: Easing.OutCubic } }
            }
            HoverHandler { id: callBarHover; onHoveredChanged: island.app.callBarHovered = hovered }
            Binding { target: island.app; property: "callMenuHold"; value: island.devicesMenuRef.opened || statsPopup.opened || gamePopup.opened }
            // 0.8.33 gate hook (lives HERE: the bar's ids are page-scoped): --tinhat-callopts=…
            // opens that face 20 s after the (fake) call shows, after the verify prompt is gone.
            Timer {
                running: island.app.callOptsProbe.length > 0 && island.app.callShown
                interval: 20000; repeat: false
                onTriggered: {
                    var s = island.app.callOptsProbe
                    console.log("[callopts] opening " + s)
                    if (s === "outputs") island.app.openEcOutputs()
                    else if (s === "mic") island.callOptionsRef.toggleFor(micBtn, "mic")
                    else if (s === "speaker") island.callOptionsRef.toggleFor(deafenBtn, "speaker")
                    else if (s === "camera") island.callOptionsRef.toggleFor(camBtn, "camera")
                }
            }
            // 11.0 buttons: the bar is a row of the actions table (call.bar) — the stored row or the preset's default
            // (today's buttons, same order). Each action keeps its element here, placed in the row's order
            // (Layout.column; hidden ones leave the grid); anything else in the row is an ActionButton (generics).
            // The call's room is the row's context; it reads ActionsHost.callRowSnapshotJson (product decision 2: the
            // controls dim while the call connects). Right-click the bar → "Edit buttons…".
            RowModel {
                id: brow
                app: island.app
                islandRef: "call"
                rowId: "bar"
                context: String(island.app.mCallRoomId || "").length > 0 ? ({ room: String(island.app.mCallRoomId) }) : null
                snapshotOverride: island.app.actions ? island.app.actions.callRowSnapshotJson : ""
                bespoke: ["call.mic.toggle", "call.gameChat.toggle", "call.corner.leave", "call.deafen.toggle", "call.camera.toggle",
                          "call.screenShare.toggle", "call.audioShare.toggle", "call.health.open", "soundboard.open", "sound.stop",
                          "watchParty.open", "room.activity.open", "call.afk.toggle", "call.devices.open", "call.leave"]
            }
            TapHandler {
                acceptedButtons: Qt.RightButton
                onTapped: callBarMenu.popup()
            }
            Controls.Menu {
                id: callBarMenu
                Controls.MenuItem { text: "Edit buttons…"; icon.name: "document-edit-symbolic"; onTriggered: brow.edit("Call controls") }
            }
            // (a one-row GridLayout — was a RowLayout; the same geometry: hidden items and empty columns take no room)
            GridLayout {
                id: ctlRow
                anchors.centerIn: parent
                rows: 1
                rowSpacing: 0
                columnSpacing: Kirigami.Units.smallSpacing
                CallButton { // mute mic
                    app: island.app
                    id: micBtn
                    Layout.row: 0; Layout.column: Math.max(0, brow.colOf("call.mic.toggle"))
                    visible: brow.shows("call.mic.toggle")
                    live: !brow.dimmed("call.mic.toggle")
                    hasMenu: true
                    menuOpen: island.callOptionsRef.opened && island.callOptionsRef.section === "mic"
                    onMenuClicked: island.callOptionsRef.toggleFor(micBtn, "mic")
                    iconName: (app.mMicMuted || app.mServerMuted) ? "microphone-sensitivity-muted-symbolic" : "audio-input-microphone-symbolic"
                    // Muted = danger-red FILL (danger wins over active in the fill
                    // logic) with the default onAccent glyph — the bar, tile pill,
                    // and overlay warn state all agree on what "muted" looks like,
                    // and the glyph keeps >=3:1 contrast on every theme.
                    // In game chat the live mic is BLUE (not red): it is
                    // live to the room, silent only to the other game-chat people.
                    // RBAC §7.4: server-muted = the same red, but LOCKED (clicks explain instead of toggling).
                    active: app.mMicMuted || app.mServerMuted
                    danger: app.mMicMuted || app.mServerMuted
                    // 10.1 (TH-GAP-24): push-to-talk key down = a green mic (the cue callctl
                    // always shipped and nothing drew).
                    tinted: app.callctl.pttSpeaking || (app.mGameChat && !app.mMicMuted)
                    variant: app.callctl.pttSpeaking ? "ptt" : "gameChat"
                    badge: app.mGameChat ? "🎮" : ""
                    tip: app.mServerMuted ? app.serverMutedText() + " — ask them to unmute you"
                       : (app.callctl.pttSpeaking ? "Transmitting (push-to-talk)\n" : "") + (app.mMicMuted ? "Unmute" : "Mute") + " — Ctrl+Shift+M"
                         + (app.mGameChat && !app.mMicMuted ? "\nGame chat: people in the game's voice chat don't hear this mic here; everyone else does" : "")
                         + (app.heardSummary().length > 0 ? "\n" + app.heardSummary() : "")
                    // 10.5: red ring + pulse while you talk into a muted mic; amber ring while the call isn't getting it.
                    pulse: island.coreRef.mutedTalkNow   // 11.0: one pulse per warning, only while the motion switch is on
                    ringColor: island.coreRef.mutedTalkNow ? Th.callbarButtonRingMuted
                        : (app.mInCall && !app.mMicMuted && app.mHear && app.mHear.tx && app.mHear.tx.state === "notsent") ? Th.callbarButtonRingError
                        : "transparent"
                    onClicked: { if (app.mServerMuted) app.showPassiveNotification(LinkGate.plain(app.serverMutedText()), 3000); else app.callctl.toggleMic() }
                    // 10.5: "You're muted" — the reminder every voice app should have (it lived only in the
                    // overlay HUD, off by default). Above the button, one action, never steals focus.
                    Controls.Popup {
                        id: mutedTalkBubble
                        visible: island.app.mMutedTalkBubble && island.app.mInCall
                        x: Math.round((micBtn.width - width) / 2)
                        y: -height - 16
                        padding: 0
                        modal: false
                        focus: false
                        closePolicy: Controls.Popup.NoAutoClose
                        background: Rectangle {
                            radius: 12
                            color: Th.callbarMutedBubbleBg
                            border.width: 1
                            border.color: Th.callbarMutedBubbleBorder
                        }
                        contentItem: RowLayout {
                            spacing: 8
                            ThIcon {
                                Layout.leftMargin: 12
                                app: island.app
                                name: "microphone-sensitivity-muted-symbolic"
                                Layout.preferredWidth: 16; Layout.preferredHeight: 16
                                color: Th.callbarMutedBubbleIcon
                                isMask: true
                            }
                            Controls.Label {
                                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                Layout.topMargin: 9; Layout.bottomMargin: 9
                                text: island.app.callctl.pushToTalk ? "You're muted — hold " + (island.app.actions && island.app.actions.pttKeys.length > 0 ? island.app.actions.pttKeys[0] : "Ctrl") + " to talk" : "You're muted"
                                color: Th.callbarMutedBubbleFg
                                font.bold: true
                            }
                            ThButton {
                                visible: !app.callctl.pushToTalk
                                app: island.app
                                text: "Unmute"
                                onClicked: { app.mMutedTalkBubble = false; app.callctl.toggleMic() }
                            }
                            Controls.Label {
                                Layout.rightMargin: 10
                                text: "✕"
                                color: bubbleCloseHover.hovered ? Th.callbarMutedBubbleCloseFgHover : Th.callbarMutedBubbleCloseFg
                                HoverHandler { id: bubbleCloseHover; cursorShape: Qt.PointingHandCursor }
                                TapHandler { onTapped: island.coreRef.dismissMutedTalk() }
                                Accessible.role: Accessible.Button
                                Accessible.name: "Dismiss"
                            }
                        }
                    }
                }
                CallButton { // game chat toggle (0.8.23): my mic stays live to the room,
                    app: island.app
                    // 11.0: one element for two actions — game chat, or "Leave the corner" while aside (the row shows one)
                    Layout.row: 0
                    Layout.column: Math.max(0, brow.shows("call.gameChat.toggle") ? brow.colOf("call.gameChat.toggle") : brow.colOf("call.corner.leave"))
                    visible: brow.shows("call.gameChat.toggle") || brow.shows("call.corner.leave")
                    live: !(brow.shows("call.gameChat.toggle") ? brow.dimmed("call.gameChat.toggle") : brow.dimmed("call.corner.leave"))
                             // but peers who are ALSO in game chat don't hear it here — we hear
                             // each other in-game, so nobody is heard twice. Hangers-on still
                             // hear and talk to everyone. Per-call; off at every join.
                             // 9.4: while pulled aside (a corner) this button is YELLOW and
                             // means "leave the corner" — game chat and a corner never overlap.
                    // 10.1 (TH-CALL-07): its own glyph — it sat next to Mute wearing the same microphone.
                    iconName: app.mAside ? "audio-input-microphone-symbolic" : "applications-games-symbolic"
                    badge: app.mAside ? "🤫" : "🎮"
                    active: app.mGameChat || app.mAside
                    tinted: app.mGameChat || app.mAside
                    busy: app.mAside && !app.mCorner.active   // 10.1 (TH-FLOAT-15): a pending pull spins; it used to look identical to active
                    variant: app.mAside ? "aside" : "gameChat"
                    iconColor: app.mAside ? Th.callbarButtonAsideIcon : Th.callbarButtonIcon
                    tip: app.mAside
                         ? (app.mCorner.active
                            ? "Aside with " + app.cornerNames(app.mCorner.members) + " — everyone else is at 10 % and can't hear your mics. Click to go back to everyone"
                            : "Pulling them aside… (click to cancel)")
                         : (app.mGameChat
                            ? "Leave game chat — everyone in the call hears your mic again"
                            : "Game chat — I'm in the game's own voice chat: people who are too won't hear my mic twice; everyone else still does")
                    onClicked: app.mAside ? app.callctl.leaveCorner() : app.callctl.applyGameChat(!app.mGameChat)
                }
                CallButton { // deafen — mute ALL incoming call audio (output)
                    app: island.app
                    Layout.row: 0; Layout.column: Math.max(0, brow.colOf("call.deafen.toggle"))
                    visible: brow.shows("call.deafen.toggle")
                    live: !brow.dimmed("call.deafen.toggle")
                    id: deafenBtn
                    hasMenu: true
                    menuOpen: island.callOptionsRef.opened && island.callOptionsRef.section === "speaker"
                    onMenuClicked: island.callOptionsRef.toggleFor(deafenBtn, "speaker")
                    iconName: app.mDeafened ? "audio-volume-muted-symbolic" : "audio-volume-high-symbolic"
                    active: app.mDeafened
                    danger: app.mDeafened // deafened = you hear nothing; warn like mute
                    tip: (app.mDeafened ? "Undeafen (restore audio)" : "Deafen (mute all output)") + " — Ctrl+Shift+D"
                    onClicked: app.callctl.applyDeafen(!app.mDeafened)
                }
                CallButton { // camera toggle
                    app: island.app
                    Layout.row: 0; Layout.column: Math.max(0, brow.colOf("call.camera.toggle"))
                    visible: brow.shows("call.camera.toggle")
                    live: !brow.dimmed("call.camera.toggle")
                    id: camBtn
                    hasMenu: true
                    menuOpen: island.callOptionsRef.opened && island.callOptionsRef.section === "camera"
                    onMenuClicked: island.callOptionsRef.toggleFor(camBtn, "camera")
                    // "camera-disabled-symbolic" is an Adwaita name absent from Breeze
                    // (renders BLANK). Use the known-good "camera-video-symbolic" for
                    // both states; OFF gets the CallButton slash overlay (friends
                    // couldn't tell fill-vs-no-fill apart — live feedback).
                    iconName: "camera-video-symbolic"
                    active: app.mCameraOn
                    slashed: !app.mCameraOn
                    tip: app.mCameraOn ? "Turn camera off" : "Turn camera on"
                    onClicked: app.callctl.toggleCamera()
                }
                CallButton { // share screen — 10.1 (TH-POP-13): one click STOPS a live share; the ▴ opens the popover
                    app: island.app
                    Layout.row: 0; Layout.column: Math.max(0, brow.colOf("call.screenShare.toggle"))
                    visible: brow.shows("call.screenShare.toggle")
                    live: !brow.dimmed("call.screenShare.toggle")
                    iconName: "video-display-brightness-symbolic"
                    active: app.mCallSharing
                    hasMenu: true
                    menuOpen: island.streamPopoverRef.opened
                    onMenuClicked: island.streamPopoverRef.visible ? island.streamPopoverRef.close() : island.streamPopoverRef.open()
                    tip: app.mCallSharing
                         ? "Stop sharing" + (app.mShareRecOnly ? " (recording only)" : "") + " — ▴ for preview & options"
                         : "Share your screen…"
                    onClicked: {
                        if (app.mCallSharing) app.callctl.stopScreen()
                        else if (island.streamPopoverRef.visible) island.streamPopoverRef.close()
                        else island.streamPopoverRef.open()
                    }
                }
                CallButton { // share audio — capture an output sink's .monitor (desktop/app/game audio)
                    app: island.app
                    Layout.row: 0; Layout.column: Math.max(0, brow.colOf("call.audioShare.toggle"))
                    visible: brow.shows("call.audioShare.toggle")
                    live: !brow.dimmed("call.audioShare.toggle")
                    hasMenu: true // 0.8.33: the source picker lives in the stream popover
                    menuOpen: island.streamPopoverRef.opened
                    onMenuClicked: island.streamPopoverRef.visible ? island.streamPopoverRef.close() : island.streamPopoverRef.open()
                    iconName: "audio-speakers-symbolic"
                    active: app.mMusicPlaying
                    tip: app.mMusicPlaying ? "Stop sharing audio (" + island.streamPopoverRef.audioLabelFor(app.mMusicSourceName) + ")"
                        : (app.mMusicSourceName.length > 0
                           ? "Share audio (" + island.streamPopoverRef.audioLabelFor(app.mMusicSourceName) + ")"
                           : "Share audio — pick an app in the share popover first")
                    onClicked: app.mMusicPlaying ? app.callctl.stopMusic()
                                                  : app.callctl.startMusicSource(app.mMusicSourceName)
                }
                CallButton { // stream health — what's actually going OUT (encoder, res/fps,
                    app: island.app
                    Layout.row: 0; Layout.column: Math.max(0, brow.colOf("call.health.open"))
                    visible: brow.shows("call.health.open")
                    live: !brow.dimmed("call.health.open")
                              // bitrate, SFU-reported loss/RTT), one card per outgoing stream.
                    id: healthChip
                    // view-statistics-symbolic: present in Breeze (verified on
                    // disk) — network-transmit-symbolic is an Adwaita-only name
                    // that rendered as the blank fallback glyph, same trap as
                    // camera-disabled-symbolic above.
                    iconName: "view-statistics-symbolic"
                    active: statsPopup.opened
                    tip: "Stream health — what you're sending"

                    // Worst-of health across the outgoing streams: "" (fine/nothing),
                    // "warn" (encoder limited or 1-3% loss), "bad" (>=3% loss).
                    // lossPct is -1 until the SFU's first receiver report — never
                    // counted as loss. Colors are GLYPH-ON-DARK tokens by design
                    // (a 7px dot on the dark bar, not a fill under onAccent).
                    readonly property string worst: {
                        var w = ""
                        for (var i = 0; i < app.mPublishStats.length; ++i) {
                            var e = app.mPublishStats[i]
                            if (e.lossPct >= 3) return "bad"
                            if ((e.qlim && e.qlim !== "none") || e.lossPct >= 1) w = "warn"
                        }
                        return w
                    }
                    Rectangle { // degradation dot (hidden while everything is healthy,
                                // and while the popup is open: active turns the button
                                // into an ACCENT fill, where warning/dangerBright are
                                // exactly the glyph-on-dark tokens that fail as fills —
                                // the open popup shows per-stream status anyway)
                        visible: healthChip.worst !== "" && !statsPopup.opened
                        width: 9; height: 9; radius: 4.5
                        anchors.top: parent.top
                        anchors.right: parent.right
                        anchors.margins: 3
                        color: healthChip.worst === "bad" ? Th.callbarHealthDotBadFill : Th.callbarHealthDotWarnFill
                        border.width: 1
                        border.color: Th.callbarHealthDotBorder
                    }
                    onClicked: statsPopup.opened ? statsPopup.close() : statsPopup.open()

                    Controls.Popup {
                        id: statsPopup
                        width: 340
                        x: healthChip.width / 2 - width / 2
                        // 10.1 (TH-CALL-08): above the bar when there is room, below it otherwise; never off-window.
                        y: (healthChip.mapToItem(Controls.Overlay.overlay, 0, 0).y > height + 16) ? -height - 8 : healthChip.height + 8
                        margins: 6
                        modal: false
                        padding: Kirigami.Units.largeSpacing
                        background: Rectangle {
                            color: Th.callbarPopupBg; radius: island.app.radius
                            border.width: 1; border.color: Th.callbarPopupBorder
                        }
                        // Human labels for the registry keys ("screen", "camera/<rid>", "mic").
                        function kindLabel(k) {
                            if (k === "screen") return "Screen share"
                            if (k === "mic") return "Microphone"
                            if (String(k).indexOf("camera") === 0) {
                                var rid = String(k).indexOf("/") > 0 ? String(k).split("/")[1] : ""
                                var names = { f: "high", h: "medium", q: "low" }
                                return "Camera" + (rid ? " — " + (names[rid] || rid) + " layer" : "")
                            }
                            return k
                        }
                        function fmtRate(kbps) {
                            if (kbps >= 1000) return (kbps / 1000).toFixed(1) + " Mbps"
                            return kbps + " kbps"
                        }
                        // 10.1 (TH-CALL-21): a sentence a person can act on first; the numbers after it.
                        function plainStatus(e) {
                            // 10.5 (audio review #3): "Looking good" needs something to actually be leaving.
                            if (e.kind === "mic" && island.app.mMicMuted) return "Muted"
                            if (e.sendKnown === false) return "Measuring…"
                            if (e.active === false || e.sendKbps <= 0) return "Nothing is being sent right now"
                            if (e.qlim === "cpu") return "Your computer can't keep up — try 30 fps or a lower resolution"
                            if (e.qlim === "bandwidth" || e.lossPct >= 3) return "Your connection can't keep up — lower the bitrate"
                            if (e.qlim && e.qlim !== "none") return "Limited by " + e.qlim
                            if (e.lossPct >= 1) return "A little packet loss — should be fine"
                            return "Looking good"
                        }
                        function statusText(e) {
                            var parts = []
                            if (e.lossPct >= 0) parts.push("loss " + e.lossPct.toFixed(1) + "%")
                            if (e.rttMs >= 0) parts.push("RTT " + e.rttMs + " ms")
                            return plainStatus(e) + (parts.length ? "  ·  " + parts.join(" · ") : "")
                        }
                        function statusColor(e) {
                            if (e.sendKnown !== false && !(e.kind === "mic" && island.app.mMicMuted) && (e.active === false || e.sendKbps <= 0)) return Th.healthStatusBadFg
                            if (e.lossPct >= 3) return Th.healthStatusBadFg
                            if ((e.qlim && e.qlim !== "none") || e.lossPct >= 1) return Th.healthStatusWarnFg
                            return Th.healthStatusGoodFg
                        }
                        ColumnLayout {
                            anchors.fill: parent
                            spacing: Kirigami.Units.smallSpacing
                            Controls.Label {
                                text: "Outgoing streams"
                                font.bold: true
                                color: Th.callbarPopoverTitleFg
                            }
                            Controls.Label {
                                visible: island.app.mPublishStats.length === 0
                                text: "Nothing is being sent right now."
                                color: Th.callbarPopoverHintFg
                                font.pointSize: Kirigami.Theme.smallFont.pointSize
                            }
                            Repeater {
                                model: island.app.mPublishStats
                                delegate: Rectangle {
                                    required property var modelData
                                    Layout.fillWidth: true
                                    implicitHeight: cardCol.implicitHeight + Kirigami.Units.smallSpacing * 2
                                    radius: island.app.radiusSm
                                    color: Th.healthCardBg
                                    opacity: modelData.active === false ? 0.55 : 1.0
                                    ColumnLayout {
                                        id: cardCol
                                        anchors.fill: parent
                                        anchors.margins: Kirigami.Units.smallSpacing
                                        spacing: 1
                                        RowLayout {
                                            Layout.fillWidth: true
                                            spacing: Kirigami.Units.smallSpacing
                                            Controls.Label {
                                                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                                text: statsPopup.kindLabel(modelData.kind)
                                                font.bold: true
                                                color: Th.healthCardTitleFg
                                                elide: Text.ElideRight
                                                Layout.fillWidth: true
                                            }
                                            Rectangle {
                                                width: 8; height: 8; radius: 4
                                                color: statsPopup.statusColor(modelData)
                                            }
                                        }
                                        Controls.Label {
                                            textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                            // What actually ENGAGED (libwebrtc's implementation
                                            // string), with the requested pick alongside when the
                                            // stats haven't reported yet / differ.
                                            text: {
                                                var enc = String(modelData.enc || "")
                                                var pick = String(modelData.encPick || "")
                                                if (pick.length > 0 && enc !== pick) return enc + "  (" + pick + ")"
                                                return enc
                                            }
                                            color: Th.healthCardDetailFg
                                            font.pointSize: Kirigami.Theme.smallFont.pointSize
                                            elide: Text.ElideRight
                                            Layout.fillWidth: true
                                        }
                                        Controls.Label {
                                            textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                            visible: modelData.w > 0
                                            text: modelData.w + "×" + modelData.h + " @ " + modelData.fps + " fps"
                                            color: Th.healthCardValueFg
                                        }
                                        Controls.Label {
                                            textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                            text: statsPopup.fmtRate(modelData.sendKbps)
                                                  + (modelData.targetKbps > 0
                                                     ? "  ·  cap " + statsPopup.fmtRate(modelData.targetKbps) : "")
                                            color: Th.healthCardValueFg
                                        }
                                        Controls.Label {
                                            textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                            text: statsPopup.statusText(modelData)
                                            color: statsPopup.statusColor(modelData)
                                            font.pointSize: Kirigami.Theme.smallFont.pointSize
                                        }
                                        Controls.Label {
                                            textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                            visible: modelData.w > 0
                                            text: "keyframes " + modelData.key + " · nack " + modelData.nack + " · pli " + modelData.pli
                                            color: Th.healthCardDetailFg
                                            font.pointSize: Kirigami.Theme.smallFont.pointSize
                                        }
                                    }
                                }
                            }
                            HintLabel {
                                app: island.app
                                visible: app.mPublishStats.length > 0
                                Layout.fillWidth: true
                                text: "Loss and RTT are what the server reports receiving from you. Updates every 3 s."
                            }
                        }
                    }
                }
                CallButton { // soundboard — opens (or re-raises) the soundboard window
                    app: island.app
                    Layout.row: 0; Layout.column: Math.max(0, brow.colOf("soundboard.open"))
                    visible: brow.shows("soundboard.open")
                    live: !brow.dimmed("soundboard.open")
                    iconName: "emblem-music-symbolic"
                    active: app.soundboardPanel.visible || app.soundboardPanel.playingUrl.length > 0
                    tip: app.soundboardPanel.playingUrl.length > 0
                         ? "Soundboard — playing " + app.soundboardPanel.playingName : "Soundboard"
                    onClicked: { app.soundboardCtl.refresh(app.room.roomId); app.soundboardPanel.openOrRaise() }
                }
                // Now playing (0.8.21): the clip's name + a stop, right in the
                // call bar. The clicker had no sign anything fired — the panel is a
                // separate window and was usually behind the app.
                Rectangle {
                    id: sbNowPlaying
                    // 11.0: the row's "Stop the sound" (hidden while nothing plays — the default row's hideWhenOff; a
                    // row that keeps it shows it dimmed then)
                    Layout.row: 0; Layout.column: Math.max(0, brow.colOf("sound.stop"))
                    visible: brow.shows("sound.stop") && (brow.dimmed("sound.stop") || island.app.soundboardPanel.playingUrl.length > 0)
                    opacity: brow.dimmed("sound.stop") ? 0.5 : 1
                    enabled: !brow.dimmed("sound.stop")
                    implicitHeight: 34
                    implicitWidth: sbNpRow.implicitWidth + 20
                    radius: 17
                    color: Th.callbarNowPlayingBg
                    border.width: 1
                    border.color: Th.callbarNowPlayingBorder
                    RowLayout {
                        id: sbNpRow
                        anchors.centerIn: parent
                        spacing: 6
                        Kirigami.Icon {
                            id: sbNpIcon
                            source: "media-playback-start"
                            color: Th.callbarNowPlayingIcon
                            implicitWidth: 16
                            implicitHeight: 16
                            // 11.0 (GPU.md §2.2, R6): STATIC — shown lit (it blinked every 700 ms while a sound played).
                        }
                        Controls.Label {
                            textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                            text: island.app.soundboardPanel.playingName.length > 0 ? island.app.soundboardPanel.playingName : "Sound"
                            color: Th.callbarNowPlayingFg
                            font.bold: true
                            elide: Text.ElideRight
                            Layout.maximumWidth: 180
                        }
                        Controls.ToolButton {
                            icon.name: "media-playback-stop"
                            implicitWidth: 26
                            implicitHeight: 26
                            Controls.ToolTip.text: "Stop the sound (for everyone)"
                            Controls.ToolTip.visible: hovered
                            Controls.ToolTip.delay: 400
                            onClicked: island.app.soundboardPanel.stopCurrent()
                        }
                    }
                }
                // 11.4 (Media Party): one click from the call — the bespoke `watchParty.open` button (join + open) with a
                // dot while a party is live in the call's room, and beside it the now-playing strip (art · title · who
                // queued it · play/pause for me · click opens). Both sit in the action's own cell; the strip mirrors
                // sbNowPlaying's pill. `mCallParty` is Rust's 3 s poll of the call room (whether or not I'm in).
                RowLayout {
                    id: mpCell
                    Layout.row: 0; Layout.column: Math.max(0, brow.colOf("watchParty.open"))
                    visible: brow.shows("watchParty.open")
                    spacing: Kirigami.Units.smallSpacing
                    readonly property var party: island.app.mCallParty || ({})
                    readonly property string npUrl: String(party.url || "")
                    readonly property bool npShown: !!party.live && mpCell.npUrl.length > 0 && mpCell.npUrl.indexOf("tinhat-memory://") !== 0
                    readonly property var npItem: party.item || ({})
                    readonly property string npNext: String(party.next || "")   // 11.3 QOL: the url after the playing one
                    readonly property string hostId: island.app.mWatchInParty ? String((island.app.mWatchParty && island.app.mWatchParty.hostId) || "")
                                                                              : String(party.hostId || "")
                    readonly property bool canControl: island.app.mWatchInParty && (hostId.length === 0 || hostId === String(island.app.mUserId || ""))
                    function openParty() {
                        if (island.app.actions) island.app.actions.exec("watchParty.open", { subject: String(island.app.mCallRoomId || "") })
                    }
                    CallButton {
                        id: mpBtn
                        app: island.app
                        live: !brow.dimmed("watchParty.open")
                        iconName: "video-television-symbolic"
                        active: island.app.mWatchInParty
                        tip: island.app.mWatchInParty ? "Media Party — you're in it (click to show the window)"
                           : (mpCell.party.live ? "Media Party — " + mpCell.party.viewers + (mpCell.party.viewers === 1 ? " person is" : " people are") + " in it · click to join"
                                                 : "Media Party — watch videos and listen to music together")
                        onClicked: mpCell.openParty()
                        // the live dot: lit while a party runs in this room; ONE pulse on its rising edge (GPU.md §2.2/§2.4:
                        // nothing loops while the call sits idle), like the muted-talk ring
                        Rectangle {
                            id: mpDot
                            anchors.right: parent.right; anchors.top: parent.top
                            anchors.rightMargin: 2; anchors.topMargin: 2
                            width: 12; height: 12; radius: 6
                            visible: !!mpCell.party.live && !island.app.mWatchInParty
                            color: Th.callbarNowPlayingIcon
                            border.width: 2; border.color: Th.callbarNowPlayingBorder
                            Rectangle {
                                id: mpDotPulse
                                anchors.centerIn: parent; width: parent.width; height: width; radius: width / 2
                                color: "transparent"; border.width: 2; border.color: Th.callbarNowPlayingIcon
                                opacity: 0; visible: mpDotPulseAnim.running
                                ParallelAnimation {
                                    id: mpDotPulseAnim
                                    onStopped: { mpDotPulse.opacity = 0; mpDotPulse.scale = 1.0 }
                                    NumberAnimation { target: mpDotPulse; property: "scale"; from: 1.0; to: 2.4; duration: Motion.durVeryLong; easing.type: Easing.OutCubic }
                                    NumberAnimation { target: mpDotPulse; property: "opacity"; from: 0.9; to: 0.0; duration: Motion.durVeryLong; easing.type: Easing.OutCubic }
                                }
                            }
                            onVisibleChanged: if (visible && Motion.animate) mpDotPulseAnim.restart()
                        }
                    }
                    Rectangle {
                        id: mpNowPlaying
                        visible: mpCell.npShown
                        opacity: brow.dimmed("watchParty.open") ? 0.5 : 1
                        enabled: !brow.dimmed("watchParty.open")
                        implicitHeight: 34
                        implicitWidth: mpNpRow.implicitWidth + 20
                        radius: 17
                        color: Th.callbarNowPlayingBg
                        border.width: 1
                        border.color: Th.callbarNowPlayingBorder
                        // 11.3 QOL (QOL.md §7): the tooltip names what plays — "title — artist"
                        Controls.ToolTip.text: String(mpCell.npItem.title || "").length > 0
                            ? String(mpCell.npItem.title) + (String(mpCell.npItem.artist || "").length > 0 ? " — " + String(mpCell.npItem.artist) : "")
                            : "Media Party — open the window"
                        Controls.ToolTip.visible: mpNpMa.containsMouse
                        Controls.ToolTip.delay: 400
                        MouseArea { id: mpNpMa; anchors.fill: parent; hoverEnabled: true; cursorShape: Qt.PointingHandCursor; onClicked: mpCell.openParty() }
                        RowLayout {
                            id: mpNpRow
                            anchors.centerIn: parent
                            spacing: 6
                            // art 24 px (the item's thumbnail / cover), else the party glyph
                            Rectangle {
                                implicitWidth: 24; implicitHeight: 24; radius: 4; clip: true
                                color: Th.callbarNowPlayingBorder
                                Image {
                                    anchors.fill: parent
                                    source: LinkGate.imageSrc(mpCell.npItem.art)   // review #5: https only
                                    visible: status === Image.Ready
                                    fillMode: Image.PreserveAspectCrop
                                    asynchronous: true
                                    sourceSize.width: 48; sourceSize.height: 48
                                }
                                Kirigami.Icon {
                                    anchors.centerIn: parent
                                    visible: LinkGate.imageSrc(mpCell.npItem.art) === ""
                                    source: "video-television-symbolic"
                                    color: Th.callbarNowPlayingIcon
                                    implicitWidth: 16; implicitHeight: 16
                                }
                            }
                            ColumnLayout {
                                spacing: 0
                                Controls.Label {
                                    textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                    text: String(mpCell.npItem.title || "").length > 0 ? String(mpCell.npItem.title) : "Media Party"
                                    color: Th.callbarNowPlayingFg
                                    font.bold: true
                                    elide: Text.ElideRight
                                    Layout.maximumWidth: 180
                                }
                                Controls.Label {
                                    textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                    visible: String(mpCell.npItem.addedBy || "").length > 0
                                    text: "queued by " + island.app.localpart(String(mpCell.npItem.addedBy || ""))
                                    color: Th.callbarNowPlayingFg
                                    opacity: 0.75
                                    font.pointSize: Kirigami.Theme.smallFont.pointSize
                                    elide: Text.ElideRight
                                    Layout.maximumWidth: 180
                                }
                            }
                            Controls.ToolButton {
                                icon.name: mpCell.party.playing ? "media-playback-pause" : "media-playback-start"
                                implicitWidth: 26
                                implicitHeight: 26
                                enabled: !island.app.mWatchInParty || mpCell.canControl
                                Controls.ToolTip.text: !island.app.mWatchInParty ? "Join the Media Party"
                                    : (!mpCell.canControl ? "Only the host (" + island.app.localpart(mpCell.hostId) + ") controls playback"
                                       : (mpCell.party.playing ? "Pause for everyone" : "Play for everyone"))
                                Controls.ToolTip.visible: hovered
                                Controls.ToolTip.delay: 400
                                onClicked: {
                                    if (!island.app.mWatchInParty) { mpCell.openParty(); return }
                                    if (mpCell.party.playing) island.app.watchCtl.pause(); else island.app.watchCtl.play()
                                }
                            }
                            // 11.3 QOL (QOL.md §7): next — shown when there is a next item, host-gated like ▶/⏸ (not in the
                            // party yet: opens it, like ▶)
                            Controls.ToolButton {
                                visible: mpCell.npNext.length > 0
                                icon.name: "media-skip-forward"
                                implicitWidth: 26
                                implicitHeight: 26
                                enabled: !island.app.mWatchInParty || mpCell.canControl
                                Controls.ToolTip.text: !island.app.mWatchInParty ? "Join the Media Party"
                                    : (!mpCell.canControl ? "Only the host (" + island.app.localpart(mpCell.hostId) + ") controls playback"
                                       : "Play next in queue")
                                Controls.ToolTip.visible: hovered
                                Controls.ToolTip.delay: 400
                                onClicked: {
                                    if (!island.app.mWatchInParty) { mpCell.openParty(); return }
                                    island.app.watchCtl.playUrl(mpCell.npNext)
                                }
                            }
                        }
                    }
                }
                CallButton { // game / join code — free-text "what are we playing" + a
                    app: island.app
                    Layout.row: 0; Layout.column: Math.max(0, brow.colOf("room.activity.open"))
                    visible: brow.shows("room.activity.open")
                    live: !brow.dimmed("room.activity.open")
                              // join code, both broadcast to the whole room (synced state).
                    id: gameChip
                    iconName: "applications-games-symbolic"
                    active: app.mActivity && String(app.mActivity.code || "").length > 0
                    tip: (app.mActivity && String(app.mActivity.game || "").length > 0)
                         ? ("Playing " + String(app.mActivity.game) + (active ? " — join code shared" : ""))
                         : "Game / join code"
                    onClicked: gamePopup.opened ? gamePopup.close() : gamePopup.open()

                    Controls.Popup {
                        id: gamePopup
                        width: 320
                        x: gameChip.width / 2 - width / 2
                        y: (gameChip.mapToItem(Controls.Overlay.overlay, 0, 0).y > height + 16) ? -height - 8 : gameChip.height + 8   // 10.1 (TH-CALL-08)
                        margins: 6
                        modal: false
                        focus: true
                        padding: Kirigami.Units.largeSpacing
                        background: Rectangle {
                            color: Th.callbarPopupBg; radius: island.app.radius
                            border.width: 1; border.color: Th.callbarPopupBorder
                        }
                        function apply() {
                            // Game name required, code optional. Writes the SYNCED activity
                            // state event so the whole room sees both (no local-only codes).
                            var g = gameField.text.trim()
                            if (g.length === 0) return
                            island.app.activityCtl.setActivity(g, "", codeField.text.trim())
                            gamePopup.close()
                        }
                        onOpened: {
                            gameField.text = (island.app.mActivity && island.app.mActivity.game) ? String(island.app.mActivity.game) : ""
                            codeField.text = (island.app.mActivity && island.app.mActivity.code) ? String(island.app.mActivity.code) : ""
                            gameField.forceActiveFocus()
                        }
                        ColumnLayout {
                            anchors.fill: parent
                            spacing: Kirigami.Units.smallSpacing
                            Controls.Label { text: "What are we playing?"; font.bold: true; color: Th.callbarPopoverTitleFg }
                            ThSwitch { // second entry point for game chat
                                text: "I'm in the game's voice chat"
                                checked: island.app.mGameChat
                                onToggled: island.app.callctl.applyGameChat(checked)
                                Controls.ToolTip.text: "People who also switch this on won't hear your mic here (you hear each other in-game); everyone else still does"
                                Controls.ToolTip.visible: hovered
                            }
                            Controls.TextField {
                                id: gameField
                                Layout.fillWidth: true
                                placeholderText: "Game name (e.g. Helldivers 2)"
                                onAccepted: codeField.forceActiveFocus()
                            }
                            Controls.Label { text: "Join / lobby code (optional)"; color: Th.callbarPopoverHintFg }
                            RowLayout {
                                Layout.fillWidth: true
                                spacing: Kirigami.Units.smallSpacing
                                Controls.TextField {
                                    id: codeField
                                    Layout.fillWidth: true
                                    placeholderText: "Paste a join code"
                                    onAccepted: gamePopup.apply()
                                }
                                Controls.Button {
                                    text: "Set"
                                    enabled: gameField.text.trim().length > 0
                                    onClicked: gamePopup.apply()
                                }
                            }
                            // Currently shared with the room (everyone sees it) — copy / clear.
                            Rectangle {
                                visible: island.app.mActivity && String(island.app.mActivity.game || "").length > 0
                                Layout.fillWidth: true
                                implicitHeight: sharedCol.implicitHeight + Kirigami.Units.smallSpacing * 2
                                radius: island.app.radiusSm
                                color: Th.callbarGamePopupSharedBg
                                ColumnLayout {
                                    id: sharedCol
                                    anchors.fill: parent
                                    anchors.margins: Kirigami.Units.smallSpacing
                                    spacing: 2
                                    Controls.Label {
                                        textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                        Layout.fillWidth: true
                                        text: (island.app.mActivity && island.app.mActivity.game) ? ("🎮  " + String(app.mActivity.game)) : ""
                                        color: Th.callbarGamePopupSharedFg; font.bold: true; elide: Text.ElideRight
                                    }
                                    RowLayout {
                                        Layout.fillWidth: true
                                        visible: island.app.mActivity && String(island.app.mActivity.code || "").length > 0
                                        spacing: Kirigami.Units.smallSpacing
                                        Controls.Label {
                                            textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                            Layout.fillWidth: true
                                            text: (island.app.mActivity && island.app.mActivity.code) ? String(island.app.mActivity.code) : ""
                                            font.family: Th.fontMono; color: Th.callbarGamePopupSharedFg; elide: Text.ElideRight
                                        }
                                        Controls.ToolButton {
                                            icon.name: "edit-copy-symbolic"
                                            Controls.ToolTip.text: "Copy code"; Controls.ToolTip.visible: hovered
                                            onClicked: if (island.app.mActivity && island.app.mActivity.code) island.app.copyToClipboard(String(island.app.mActivity.code))
                                        }
                                    }
                                    Controls.ToolButton {
                                        Layout.alignment: Qt.AlignRight
                                        text: "Clear"
                                        icon.name: "edit-clear-all-symbolic"
                                        onClicked: { island.app.activityCtl.setActivity("", "", ""); gamePopup.close() }
                                    }
                                }
                            }
                        }
                    }
                }
                CallButton { // BRB / AFK (peers see our tile dimmed + 💤)
                    app: island.app
                    Layout.row: 0; Layout.column: Math.max(0, brow.colOf("call.afk.toggle"))
                    visible: brow.shows("call.afk.toggle")
                    live: !brow.dimmed("call.afk.toggle")
                    iconName: "appointment-missed-symbolic"
                    active: app.mLocalAfk
                    tip: app.mLocalAfk ? "I'm back — your mic and what you hear come back (your camera stays off until you turn it on)"
                                        : "Be right back — mutes your mic, turns your camera off and mutes everyone for you; peers see 💤 on your tile"
                    onClicked: app.afkCtl.setAfk(!app.mLocalAfk)
                }
                CallButton { // call & device settings — the gear menu was
                    app: island.app
                    Layout.row: 0; Layout.column: Math.max(0, brow.colOf("call.devices.open"))
                    visible: brow.shows("call.devices.open")
                    live: !brow.dimmed("call.devices.open")
                             // unreachable while the call is maximized (its other
                             // opener lives in the room header, hidden then).
                    iconName: "settings-configure-symbolic"
                    tip: "Call & device settings"
                    onClicked: { app.refreshDeviceMenus(); island.devicesMenuRef.popup() }
                }
                CallButton { // hang up — 10.1 (TH-CALL-06): a wider pill, so the one irreversible button never reads like the mute/deafen circles
                    app: island.app
                    implicitWidth: 72
                    // (its own busy face while hanging up — never dimmed twice)
                    Layout.row: 0; Layout.column: Math.max(0, brow.colOf("call.leave"))
                    visible: brow.shows("call.leave")
                    iconName: "call-stop-symbolic"
                    danger: true
                    live: !app.mHangingUp
                    busy: app.mHangingUp
                    tip: app.mHangingUp ? "Hanging up…" : "Leave call"
                    onClicked: app.callctl.leaveCall()
                }
                // the row's other buttons (added from the editor, or all of them in another look / size) and its dividers
                // (sepBefore: the default row's before Leave — the bar's own divider)
                Repeater {
                    model: brow.generics
                    delegate: ActionButton {
                        required property var modelData
                        Layout.row: 0
                        Layout.column: modelData.col
                        Layout.alignment: Qt.AlignVCenter
                        app: island.app
                        item: modelData.item
                        rowStyle: brow.result.style
                        rowScale: brow.result.scale
                        onRun: (anchor) => brow.run(modelData.item, anchor)
                        onHold: (anchor, down) => brow.run(modelData.item, anchor, { hold: down ? "down" : "up" })
                    }
                }
                Repeater {
                    model: brow.seps
                    delegate: Rectangle {
                        required property var modelData
                        Layout.row: 0
                        Layout.column: modelData.col
                        Layout.alignment: Qt.AlignVCenter
                        implicitWidth: 1
                        implicitHeight: 30
                        color: Th.callbarDivider
                    }
                }
                // 11.0.2 (2026-09-28): the bar's own pin — "Keep call buttons visible" (call.bar.pin.toggle, the same
                // preference as Settings → Calls). Not a row item: after the row, behind its own divider, in every stored
                // row; small, so it never reads like a call control. Tooltip = the action's short faces; pressed = pinned.
                Rectangle {
                    Layout.row: 0
                    Layout.column: brow.lastColumn + 1
                    Layout.alignment: Qt.AlignVCenter
                    implicitWidth: 1
                    implicitHeight: 30
                    color: Th.callbarDivider
                }
                CallButton {
                    app: island.app
                    objectName: "callBarPin"
                    Layout.row: 0
                    Layout.column: brow.lastColumn + 2
                    Layout.alignment: Qt.AlignVCenter
                    implicitWidth: 32
                    implicitHeight: 32
                    glyphSize: 16
                    iconName: "window-pin-symbolic"
                    active: island.app.callBarPinned
                    tip: island.app.callBarPinned ? "Let buttons hide" : "Keep buttons visible"
                    onClicked: island.app.setCallBarPinned(!island.app.callBarPinned)
                }
            }
        }
    }

    // 9.3.1: the seam between the call and the chat. Drag it to give either side more
    // room; click the chevron (or double-click the seam) to tuck the call tiles away
    // behind their bars so the chat has the space. Both are remembered.
    Item {
        id: callSeam
        Layout.fillWidth: true
        visible: island.app.callShown && !island.app.callMaximized
        implicitHeight: 16
        Layout.minimumHeight: 16   // 10.8: never squeezed away (a call island shorter than it wants shrinks the grid)
        Rectangle {
            anchors.centerIn: parent
            width: 72; height: 4; radius: 2
            color: (seamHover.hovered || seamDrag.active) && !island.app.callCollapsed ? Th.callSeamFillHover : Th.callSeamFill
            Behavior on color { ColorAnimation { duration: Motion.dur } }
        }
        HoverHandler { id: seamHover; cursorShape: island.app.callCollapsed ? Qt.ArrowCursor : Qt.SplitVCursor }
        DragHandler {
            id: seamDrag
            enabled: !island.app.callCollapsed
            target: null
            xAxis.enabled: false
            yAxis.enabled: true
            property real startFrac: 0
            onActiveChanged: {
                if (active) {
                    startFrac = island.app.callPaneFrac >= 0 ? island.app.callPaneFrac
                        : Math.min(0.85, Math.max(0.2, (videoGrid.height + videoGrid.chromeH) / Math.max(1, island.pageH)))
                } else if (island.app.callPaneFrac >= 0) {
                    island.app.setCallPaneFrac(island.app.callPaneFrac)   // persist on release
                    console.log("[seam] frac=" + island.app.callPaneFrac.toFixed(3) + " gridH=" + Math.round(videoGrid.height))
                }
            }
            onTranslationChanged: if (active) island.app.callPaneFrac = Math.min(0.85, Math.max(0.2, startFrac + translation.y / Math.max(1, island.pageH)))
        }
        TapHandler { acceptedButtons: Qt.LeftButton; onDoubleTapped: island.app.setCallCollapsed(!island.app.callCollapsed) }
        // 10.1 (TH-VIS-07): the "Collapse call" pill that sat on the seam (over the tail of the
        // chat) is gone — the in-call header has the same button, and the seam keeps its grip
        // (drag to resize, double-click to collapse).
        Timer {
            id: seamHookReport
            interval: 700
            onTriggered: console.log("[seam] hook frac=" + island.app.callPaneFrac.toFixed(2) + " collapsed=" + island.app.callCollapsed
                                     + " gridVisible=" + videoGrid.visible + " gridH=" + Math.round(videoGrid.height) + " pageH=" + Math.round(island.pageH))
        }
        // Gate hooks (TINHAT_GUI_FAKECALL): --tinhat-collapsecall tucks the call away once it is
        // up; --tinhat-seam=<frac> sets the dragged share, as if the user had dragged it.
        Timer {
            interval: 2500
            running: island.app.callShown && (Qt.application.arguments.indexOf("--tinhat-collapsecall") >= 0 || (island.app._hookArg("--tinhat-seam") || "").length > 0)
            onTriggered: {
                var f = parseFloat(island.app._hookArg("--tinhat-seam") || "")
                if (!isNaN(f)) { island.app.callPaneFrac = f; island.app.setCallPaneFrac(f) }
                if (Qt.application.arguments.indexOf("--tinhat-collapsecall") >= 0) island.app.setCallCollapsed(true)
                seamHookReport.restart()   // the layout pass runs after this handler
            }
        }
    }
}
