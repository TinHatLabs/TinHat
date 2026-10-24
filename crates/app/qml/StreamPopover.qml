// StreamPopover.qml — the stream popover (0.8.22).
//
// ONE surface for "share my screen": what to share (source), how it looks (three
// quality presets + custom), what people hear (an APP's audio — per-application capture —
// or everything, or nothing), and whether the stream is "for the recording only" (peers
// hide + mute it until they tune in; the Memories recorder still captures it). While
// live it shows a small real-time preview of exactly what is being sent, so you can tell
// the share works without asking anyone.
//
// Everything here drives the same CallController entry points the old ⚙ menus used —
// this is a front, not a new pipeline. Opened from the share button in the call bar.
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app // PipVideoItem (the preview surface, CPU-painted, NVIDIA-safe)

Controls.Popup {
    id: pop
    required property var app   // Main.qml root (mirrors: mCallSharing, mMusicPlaying, mAudioApps, …)
    required property var ctl   // callctl

    parent: Controls.Overlay.overlay
    modal: false
    dim: false
    focus: true
    closePolicy: Controls.Popup.CloseOnEscape | Controls.Popup.CloseOnPressOutside
    width: Math.min(560, (parent ? parent.width : 560) - 40)
    x: parent ? Math.round((parent.width - width) / 2) : 0
    // Above the call bar (which sits ~110 px up from the bottom of the window).
    y: parent ? Math.max(16, parent.height - height - 120) : 0
    padding: 18

    readonly property bool live: app.mCallSharing
    // 10.1 (TH-POP-01): audio-only sharing is just as live — the chip + Stop follow both.
    readonly property bool anyLive: app.mCallSharing || app.mMusicPlaying
    // An audio pick that has not started yet (it starts with "Start sharing" / "Share audio").
    readonly property bool audioPending: audioChoice.length > 0 && !(app.mMusicPlaying && app.mMusicSourceName === audioChoice)
    // Windows (TH-CALL-01): the primary button must never silently share the primary monitor —
    // it stays disabled until a row is selected (one is pre-selected on open, visibly).
    readonly property bool needsPick: sources.length > 0 && chosenHandle.length === 0
    function sourceLabel(handle) {
        for (var i = 0; i < sources.length; i++) if (String(sources[i].handle) === handle) return sources[i].label
        return ""
    }
    readonly property bool inCall: app.mInCall || app.fakeCall === true
    // The first output's monitor = "everything I hear" (the pre-0.8.22 behaviour).
    readonly property string everythingId: {
        for (var i = 0; i < app.mAudioSources.length; i++)
            if (/\.monitor$/.test(app.mAudioSources[i].node)) return app.mAudioSources[i].node
        return ""
    }
    // Audio selection: "" = none · "app:<serial>" / "pid:<n>" · a device node (e.g. "<sink>.monitor")
    property string audioChoice: ""
    property bool moreSources: false
    // Windows: monitors + windows from ctl.shareSources(); [] on Linux (the portal picks).
    property var sources: []
    property string chosenKind: ""
    property string chosenHandle: ""
    readonly property var presets: [
        { label: "Smooth", sub: "1080p · 60 fps", w: 1920, h: 1080, fps: 60, kbps: 8000 },
        { label: "Sharp",  sub: "1440p · 30 fps", w: 2560, h: 1440, fps: 30, kbps: 12000 },
        { label: "Max",    sub: "4K · 60 fps",    w: 3840, h: 2160, fps: 60, kbps: 24000 }
    ]
    // Which preset the CURRENT quality ("maxW maxH fps kbps") matches; -1 = custom/auto.
    readonly property int presetIndex: {
        var q = String(app.mShareQuality || "").split(" ")
        for (var i = 0; i < presets.length; i++)
            if (parseInt(q[0]) === presets[i].w && parseInt(q[1]) === presets[i].h && parseInt(q[2]) === presets[i].fps)
                return i
        return -1
    }
    readonly property string qualitySummary: {
        var q = String(app.mShareQuality || "").split(" ")
        if (q.length < 4) return ""
        if (parseInt(q[0]) === 0) return "Auto · " + q[2] + " fps"
        return q[1] + "p · " + q[2] + " fps · " + Math.round(parseInt(q[3]) / 1000) + " Mbps"
    }
    function audioLabelFor(id) {
        if (!id || id.length === 0) return "No audio"
        var i
        for (i = 0; i < app.mAudioApps.length; i++) if (app.mAudioApps[i].id === id) return app.mAudioApps[i].label
        for (i = 0; i < app.mAudioSources.length; i++) if (app.mAudioSources[i].node === id) return app.mAudioSources[i].label
        return id.replace(/\.monitor$/, "")
    }
    readonly property string audioSummary: app.mMusicPlaying ? audioLabelFor(app.mMusicSourceName) : "no audio"

    function syncFromApp() {
        audioChoice = app.mMusicPlaying ? app.mMusicSourceName : ""
        try { sources = JSON.parse(ctl.shareSources()) } catch (e) { sources = [] }
        chosenKind = ""; chosenHandle = ""
        // Pre-select the first monitor so the highlighted button always has a visible, NAMED
        // target (the row highlights + "Sharing: Screen 1" reads under the list).
        if (!live)
            for (var i = 0; i < sources.length; i++)
                if (sources[i].kind === "monitor") { chosenKind = "monitor"; chosenHandle = String(sources[i].handle); break }
    }
    onOpened: {
        syncFromApp()
        ctl.refreshAudioSources()
        if (live && !previewPaused) ctl.pipStart("vid_selfshare")
    }
    // 11.0 (GPU.md §2.11): the preview pauses on its last frame while the window is hidden, or unfocused with "Save GPU
    // while Tin Hat isn't focused" on (10.8.1: off by default — Motion.winState is the render state), and picks up again on focus
    readonly property bool previewPaused: app.lowgpu.previewPaused(Motion.winState, app.mCallUnfocusedSaver)
    onPreviewPausedChanged: if (visible && live) { if (previewPaused) ctl.pipStop("vid_selfshare"); else ctl.pipStart("vid_selfshare") }
    onClosed: ctl.pipStop("vid_selfshare")
    // 10.1 (TH-POP-02): the lists refresh while open — the empty state promises "start the game
    // and this list fills in", which was only true after a close + reopen.
    Timer {
        interval: 2000; repeat: true; running: pop.visible && pop.inCall
        onTriggered: {
            ctl.refreshAudioSources()
            if (pop.sources.length > 0 || !pop.live) {
                try { var s = JSON.parse(ctl.shareSources()); if (JSON.stringify(s) !== JSON.stringify(pop.sources)) pop.sources = s } catch (e) {}
            }
        }
    }
    // The preview starts once the share is live, even if the popover was open before.
    onLiveChanged: if (visible) { if (live && !previewPaused) ctl.pipStart("vid_selfshare"); else ctl.pipStop("vid_selfshare") }

    // 10.1 (TH-POP-01): picking an app is a CHOICE, not a broadcast. It only goes live at once
    // when audio is ALREADY being shared (the user is switching what people hear); otherwise
    // "Start sharing" / "Share audio" starts it. Nothing leaves the machine on a tap.
    function chooseAudio(id) {
        audioChoice = id
        if (!inCall) return
        if (id.length === 0) { if (app.mMusicPlaying) ctl.stopMusic() }
        else if (app.mMusicPlaying) ctl.startMusicSource(id)
    }
    function startShare() {
        if (!inCall) return
        if (!live) {
            if (sources.length > 0 && chosenHandle.length > 0) ctl.shareScreenSource(chosenKind, chosenHandle)
            else if (sources.length > 0) return   // Windows with nothing picked: never share the primary monitor blind
            else ctl.shareScreen()
        }
        if (audioChoice.length > 0 && !(app.mMusicPlaying && app.mMusicSourceName === audioChoice))
            ctl.startMusicSource(audioChoice)
        // The portal picker (Linux) is a system dialog: get out of its way.
        if (!live && sources.length === 0) close()
    }

    background: Rectangle {
        color: Th.callbarPopoverBg
        radius: 14
        border.width: 1
        border.color: Th.callbarPopoverBorder
    }

    component SectionTitle: Controls.Label {
        Layout.fillWidth: true
        Layout.topMargin: 4
        color: Th.callbarPopoverSectionFg
        font.bold: true
        font.pointSize: Kirigami.Theme.smallFont.pointSize
    }
    // A selectable pill (presets, audio choices).
    component Pill: Rectangle {
        id: pill
        property string text: ""
        property string sub: ""
        property bool checked: false
        property bool enabledPill: true
        signal clicked()
        implicitHeight: sub.length > 0 ? 44 : 32
        implicitWidth: Math.max(72, pillCol.implicitWidth + 24)
        radius: 10
        opacity: enabledPill ? 1 : 0.45
        color: checked ? Th.callbarPillBgSelected
             : (pillHover.hovered ? Th.callbarPillBgHover : Th.callbarPillBg)
        border.width: 1
        border.color: checked ? Th.callbarPillBorderSelected : Th.callbarPillBorder
        HoverHandler { id: pillHover }
        TapHandler { enabled: pill.enabledPill; onTapped: pill.clicked() }
        Column {
            id: pillCol
            anchors.centerIn: parent
            spacing: 1
            Controls.Label { text: pill.text; color: Th.callbarPillFg; font.bold: true; anchors.horizontalCenter: parent.horizontalCenter }
            Controls.Label { visible: pill.sub.length > 0; text: pill.sub; color: Th.callbarPillSubFg; font.pointSize: Kirigami.Theme.smallFont.pointSize; anchors.horizontalCenter: parent.horizontalCenter }
        }
    }

    contentItem: ColumnLayout {
        spacing: 10

        // ---- header ----
        RowLayout {
            Layout.fillWidth: true
            spacing: 8
            Kirigami.Icon {
                source: "video-display-brightness-symbolic"
                color: pop.live ? Th.callbarPopoverIconActive : Th.callbarPopoverIcon
                implicitWidth: 22; implicitHeight: 22
            }
            Controls.Label {
                text: pop.live ? "Sharing your screen" : (app.mMusicPlaying ? "Sharing audio" : "Share your screen")
                color: Th.callbarPopoverTitleFg
                font.bold: true
                font.pointSize: Kirigami.Theme.defaultFont.pointSize + 2
            }
            Item { Layout.fillWidth: true }
            Rectangle {
                visible: pop.anyLive
                implicitHeight: 22
                implicitWidth: liveLbl.implicitWidth + 16
                radius: 11
                color: Th.callbarLiveChipBg   // 10.1 (TH-POP-21): palette reds
                border.width: 1; border.color: Th.callbarLiveChipBorder
                Controls.Label {
                    id: liveLbl
                    anchors.centerIn: parent
                    text: "● LIVE" + (app.mCallSharing ? " · screen" : "") + (app.mMusicPlaying ? " · audio" : "") + (app.mShareRecOnly ? " · recording only" : "")
                    color: Th.callbarLiveChipFg; font.bold: true
                    font.pointSize: Kirigami.Theme.smallFont.pointSize
                }
            }
            Controls.ToolButton {
                icon.name: "window-close-symbolic"
                Controls.ToolTip.text: "Close"; Controls.ToolTip.visible: hovered; Accessible.name: "Close"
                onClicked: pop.close()
            }
        }
        Controls.Label {
            visible: !pop.inCall
            Layout.fillWidth: true
            text: "Join a call first — then pick what to share here."
            color: Th.callbarPopoverHintFg
            wrapMode: Text.WordWrap
        }

        // ---- live preview ----
        Rectangle {
            visible: pop.live
            Layout.fillWidth: true
            Layout.preferredHeight: Math.min(220, Math.round(width * 9 / 16))
            radius: 10
            color: Th.callbarPreviewBg
            clip: true
            // 0.8.30: rounded corners over the preview surface (see CornerMask.qml).
            CornerMask { radius: 10; color: Th.callbarPopoverBg; z: 15 }
            PipVideoItem {
                anchors.fill: parent
                tileId: "vid_selfshare"
                Component.onCompleted: attachViewer()
                Component.onDestruction: detachViewer()
            }
            Rectangle {
                anchors { left: parent.left; right: parent.right; bottom: parent.bottom }
                height: 24
                gradient: Gradient {
                    GradientStop { position: 0.0; color: "transparent" }
                    GradientStop { position: 1.0; color: Th.callbarPreviewCaptionBg }
                }
                Controls.Label {
                    anchors { left: parent.left; bottom: parent.bottom; margins: 6 }
                    text: "What you're sending · " + pop.qualitySummary + " · " + pop.audioSummary
                    color: Th.callbarPreviewCaptionFg
                    font.pointSize: Kirigami.Theme.smallFont.pointSize
                    elide: Text.ElideRight
                    width: parent.width - 12
                }
            }
        }

        // ---- source ----
        SectionTitle { text: "What to share" }
        // Linux: the desktop portal owns the picker (screens, windows, permissions).
        Controls.Button {
            visible: pop.sources.length === 0
            Layout.fillWidth: true
            enabled: pop.inCall
            icon.name: "monitor-symbolic"
            text: pop.live ? "Change what you're sharing…" : "Choose a screen or window…"
            onClicked: { ctl.shareScreen(); if (!pop.live) pop.close() }
        }
        // Windows: our own list (monitors first, then windows).
        Rectangle {
            visible: pop.sources.length > 0
            Layout.fillWidth: true
            Layout.preferredHeight: Math.min(170, srcList.contentHeight + 4)
            radius: 8
            color: Th.callbarSourceListBg
            border.width: 1; border.color: Th.callbarSourceListBorder
            ListView {
                id: srcList
                anchors.fill: parent
                anchors.margins: 2
                clip: true
                model: pop.sources
                spacing: 1
                interactive: contentHeight > height
                Controls.ScrollBar.vertical: FoxScrollBar { app: pop.app }   // 10.1 (TH-DS-05): the house scrollbar everywhere
                delegate: Rectangle {
                    required property var modelData
                    readonly property bool chosen: pop.chosenHandle === String(modelData.handle)
                    width: ListView.view.width
                    height: 30
                    radius: 6
                    color: chosen ? Th.callbarSourceListItemBgSelected
                         : (srcHover.hovered ? Th.callbarSourceListItemBgHover : "transparent")
                    HoverHandler { id: srcHover }
                    TapHandler {
                        onTapped: {
                            pop.chosenKind = modelData.kind; pop.chosenHandle = String(modelData.handle)
                            if (pop.live) ctl.shareScreenSource(modelData.kind, String(modelData.handle))
                        }
                    }
                    RowLayout {
                        anchors.fill: parent; anchors.leftMargin: 8; anchors.rightMargin: 8
                        spacing: 8
                        Kirigami.Icon {
                            source: modelData.kind === "monitor" ? "monitor-symbolic" : "window-symbolic"
                            color: Th.callbarSourceListItemIcon; implicitWidth: 16; implicitHeight: 16
                        }
                        Controls.Label { Layout.fillWidth: true; text: modelData.label; color: Th.callbarSourceListItemFg; elide: Text.ElideRight }
                        Kirigami.Icon { visible: chosen; source: "checkmark-symbolic"; color: Th.callbarSourceListItemIconSelected; implicitWidth: 16; implicitHeight: 16 }
                    }
                }
            }
        }
        // 10.1 (TH-CALL-01): say what the button will share, by name.
        Controls.Label {
            visible: pop.sources.length > 0
            Layout.fillWidth: true
            text: pop.chosenHandle.length > 0 ? "Sharing: " + pop.sourceLabel(pop.chosenHandle) : "Pick a screen or window above."
            color: pop.chosenHandle.length > 0 ? Th.callbarPopoverFg : Th.callbarPopoverHintFg
            font.pointSize: Kirigami.Theme.smallFont.pointSize
            elide: Text.ElideRight
        }

        // ---- quality ----
        RowLayout {
            Layout.fillWidth: true
            SectionTitle { text: "Quality"; Layout.fillWidth: true }
            Controls.Label {
                text: pop.presetIndex < 0 ? "Custom: " + pop.qualitySummary : pop.qualitySummary
                color: Th.callbarPopoverHintFg
                font.pointSize: Kirigami.Theme.smallFont.pointSize
            }
        }
        RowLayout {
            Layout.fillWidth: true
            spacing: 8
            Repeater {
                model: pop.presets
                delegate: Pill {
                    required property var modelData
                    required property int index
                    Layout.fillWidth: true
                    text: modelData.label
                    sub: modelData.sub
                    checked: pop.presetIndex === index
                    onClicked: ctl.applyShareQuality(modelData.w, modelData.h, modelData.fps, modelData.kbps)
                }
            }
            Pill {
                text: "Custom…"
                sub: "res · fps · encoder"
                checked: pop.presetIndex < 0
                onClicked: { pop.close(); app.openCallSettings() }
            }
        }

        // ---- audio ----
        SectionTitle { text: "Audio" }
        Flow {
            Layout.fillWidth: true
            spacing: 6
            Pill {
                text: "No audio"
                checked: pop.audioChoice.length === 0
                onClicked: pop.chooseAudio("")
            }
            Repeater {
                model: app.mAudioApps
                delegate: Pill {
                    required property var modelData
                    text: modelData.label
                    sub: modelData.detail || ""
                    checked: pop.audioChoice === modelData.id
                    onClicked: pop.chooseAudio(modelData.id)
                }
            }
            Pill {
                text: "Everything"
                sub: "all sound on my output"
                checked: pop.audioChoice.length > 0 && /\.monitor$/.test(pop.audioChoice)
                enabledPill: pop.everythingId.length > 0
                onClicked: pop.chooseAudio(pop.everythingId)
            }
            Pill {
                text: pop.moreSources ? "Fewer…" : "More…"
                sub: "inputs & devices"
                onClicked: pop.moreSources = !pop.moreSources
            }
        }
        Controls.Label {
            visible: app.mAudioApps.length === 0
            Layout.fillWidth: true
            text: "No app is playing sound right now — start the game or player and this list fills in."
            color: Th.callbarPopoverHintFg
            font.pointSize: Kirigami.Theme.smallFont.pointSize
            wrapMode: Text.WordWrap
        }
        Flow {
            visible: pop.moreSources
            Layout.fillWidth: true
            spacing: 6
            Repeater {
                model: app.mAudioSources
                delegate: Pill {
                    required property var modelData
                    text: modelData.label
                    checked: pop.audioChoice === modelData.node
                    onClicked: pop.chooseAudio(modelData.node)
                }
            }
        }

        // ---- recording only ----
        RowLayout {
            Layout.fillWidth: true
            Layout.topMargin: 4
            spacing: 10
            // selfhost: a server without Memories has no "recording only" (hidden; a kept-on choice is undone)
            visible: app.serverMemories
            Component.onCompleted: if (!app.serverMemories && app.mShareRecOnly) ctl.applyShareRecordingOnly(false)
            ThSwitch {
                id: recSwitch
                checked: app.mShareRecOnly
                onToggled: ctl.applyShareRecordingOnly(checked)
            }
            ColumnLayout {
                Layout.fillWidth: true
                spacing: 0
                Controls.Label { text: "For the recording only"; color: Th.callbarPopoverFg; font.bold: true }
                Controls.Label {
                    Layout.fillWidth: true
                    text: "Streams straight into Memories. Everyone else sees it hidden and muted unless they tune in."
                    color: Th.callbarPopoverHintFg
                    font.pointSize: Kirigami.Theme.smallFont.pointSize
                    wrapMode: Text.WordWrap
                }
            }
        }

        // ---- actions ----
        RowLayout {
            Layout.fillWidth: true
            Layout.topMargin: 6
            Item { Layout.fillWidth: true }
            Controls.Button {
                visible: pop.anyLive
                text: "Stop sharing"
                icon.name: "media-playback-stop-symbolic"
                onClicked: { if (pop.live) ctl.stopScreen(); if (app.mMusicPlaying) ctl.stopMusic(); pop.close() }
            }
            Controls.Button {
                highlighted: true
                enabled: pop.inCall && !pop.needsPick
                text: pop.needsPick ? "Pick what to share"
                    : (!pop.live ? "Start sharing" : (pop.audioPending ? "Share audio" : "Done"))
                icon.name: (pop.live && !pop.audioPending) ? "dialog-ok-symbolic" : "media-record-symbolic"
                onClicked: { if (pop.live && !pop.audioPending) pop.close(); else pop.startShare() }
            }
        }
    }
}
