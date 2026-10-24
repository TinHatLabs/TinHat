// CallOptionsPopover.qml — 0.8.33: the slide-out behind the call bar's ▴ chevrons.
//
// One popup, three faces — mic / speaker / camera — holding exactly the settings that belong
// to that button, rising from right above it. Every control drives the same CallController
// setter Settings › Audio / Calls uses (this is a front, not a second settings store), so the
// two never disagree. The request (2026-09-10): "the buttons at the bottom of the call that have
// amplifying options: a small up carrot that slides out the appropriate settings, like audio
// controls for the mic". The round button keeps its one-click action; the chevron is the
// settings. `toggleFor(button, section)` opens / closes / switches face.
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import QtQuick.Templates as T
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the callbar.popover.* tokens

T.Popup {
    id: pop
    required property var app   // Main.qml root (mirrors + helpers: mInputs, mMicAec, openEcOutputs, …)
    required property var ctl   // callctl
    property string section: "mic"   // mic | speaker | camera
    property var anchorItem: null
    // The button's top-left in overlay coordinates + its width (reposition() refreshes them). x/y
    // are BINDINGS off these and the popover's own height, so a face switch or a device list
    // arriving grows the popover UPWARD from the button instead of jumping (a field report: "flyouts do
    // not pop out above the button every time, and jump once updated" — 0.8.33 assigned y
    // imperatively while the enter animation was still driving y to a stale target).
    property point anchorPos: Qt.point(0, 0)
    property real anchorW: 0
    // The enter rise: the content slides up 16 px while fading in — a transform, so it never fights the y binding.
    property real rise: 0

    // QtQuick.Templates, not Controls: no platform style (breeze / Basic) underneath — see ThComboBox.
    parent: T.Overlay.overlay
    modal: false
    dim: false
    focus: true
    closePolicy: T.Popup.CloseOnEscape | T.Popup.CloseOnPressOutside
    width: 380
    padding: 18   // 10.1 (TH-POP-04): the four call popovers share one padding
    implicitHeight: contentHeight + topPadding + bottomPadding
    x: Math.max(12, Math.min((parent ? parent.width : width) - width - 12, Math.round(anchorPos.x + anchorW / 2 - width / 2)))
    y: Math.max(12, Math.round(anchorPos.y - height - 14))
    enter: Transition {
        NumberAnimation { property: "opacity"; from: 0; to: 1; duration: Motion.dur }
        NumberAnimation { target: pop; property: "rise"; from: 16; to: 0; duration: Motion.durSlow; easing.type: Easing.OutCubic }
    }
    exit: Transition {
        NumberAnimation { property: "opacity"; from: 1; to: 0; duration: Motion.durShort }
    }

    function toggleFor(item, sec) {
        // `visible`, not `opened`: CloseOnPressOutside has already begun closing the popover when
        // the chevron's press lands, and `opened` is false during the exit — the chevron must still
        // read as "close me", not reopen it.
        if (pop.visible && pop.section === sec) { pop.close(); return }
        pop.section = sec
        pop.anchorItem = item
        pop.reposition()
        pop.open()
    }
    function reposition() {
        if (!anchorItem || !parent) return
        var p = anchorItem.mapToItem(parent, 0, 0)
        anchorPos = Qt.point(p.x, p.y)
        anchorW = anchorItem.width
    }
    onAnchorItemChanged: reposition()
    Connections {
        target: pop.parent
        function onWidthChanged() { pop.reposition() }
        function onHeightChanged() { pop.reposition() }
    }
    // The speaker face shows a live output meter: the standalone monitor runs only while it shows.
    onOpened: { reposition(); if (section === "speaker") ctl.startOutputMonitor() }
    onClosed: ctl.stopOutputMonitor()

    background: Rectangle {
        color: Th.callbarPopoverBg
        radius: 14
        border.width: 1
        border.color: Th.callbarPopoverBorder
        transform: Translate { y: pop.rise }
    }

    // The 16-bar level meter Settings › Audio uses (Main.qml's LevelMeter is file-local).
    component Meter: Row {
        id: meter
        property real level: 0.0
        property color barColor: Th.callbarMeterFill
        // 11.0 (GPU.md §2.7, R13 — Main.qml LevelMeter, the web's ui/meter.ts): the bars change only on this meter's own
        // ≤ 10/s tick with the level SAMPLED at the tick, eased while the motion switch is on, snapped under Reduce
        // motion; the flat rest pose and NO tick while the window is not focused or all is quiet (it ran a 30 Hz tick).
        property real shown: 0.0
        property real sampled: 0.0
        readonly property bool active: meter.sampled > 0.02
        spacing: 3
        height: 36
        function amp(l) { return l > 0.02 ? Math.pow(Math.min(1.0, (l - 0.02) / 0.48), 0.6) : 0 }
        function rest() { meterTick.stop(); meter.shown = 0; meter.sampled = 0 }
        function tick() {
            if (!Motion.focused || !meter.visible) { meter.rest(); return }
            var target = meter.amp(meter.level), d = target - meter.shown
            meter.shown = Motion.animate ? (Math.abs(d) < 0.015 ? target : meter.shown + d * 0.5) : target
            meter.sampled = meter.level
            if (meter.shown === 0 && meter.level <= 0.02) meterTick.stop()
        }
        function wake() {
            if (meterTick.running) return
            if (!Motion.focused || !meter.visible) { meter.rest(); return }
            if (meter.level <= 0.02 && meter.shown === 0) { meter.rest(); return }
            meter.tick()
            if (!(meter.shown === 0 && meter.level <= 0.02)) meterTick.start()
        }
        onLevelChanged: meter.wake()
        onVisibleChanged: meter.wake()
        Connections {
            target: Motion
            function onWinStateChanged() { if (Motion.focused) meter.wake(); else meter.rest() }
            function onAnimateChanged() { meter.wake() }
        }
        Timer { id: meterTick; interval: 100; repeat: true; onTriggered: meter.tick() }   // WAVE_TICK_MS
        Repeater {
            model: 16
            delegate: Rectangle {
                required property int index
                width: 4; radius: 2
                anchors.verticalCenter: parent.verticalCenter
                color: meter.barColor
                opacity: meter.active ? 0.95 : 0.3
                readonly property real bell: 1.0 - Math.abs(index - 7.5) * 0.07
                readonly property real ripple: 0.6 + 0.4 * Math.abs(Math.sin(index * 1.1 + meter.sampled * 22.0))
                height: 3 + meter.shown * bell * ripple * 33
            }
        }
    }
    component Head: RowLayout {
        property string title: ""
        property string badge: ""
        property color stateColor: Th.callbarPopoverBadgeFg
        Layout.fillWidth: true
        Controls.Label { Layout.fillWidth: true; text: parent.title; font.bold: true; font.pointSize: Kirigami.Theme.defaultFont.pointSize + 1; color: Th.callbarPopoverTitleFg }
        Controls.Label { text: parent.badge; color: parent.stateColor; font.pointSize: Kirigami.Theme.smallFont.pointSize; font.bold: true }
    }
    component Hint: Controls.Label {
        Layout.fillWidth: true
        wrapMode: Text.WordWrap
        color: Th.callbarPopoverHintFg
        font.pointSize: Kirigami.Theme.smallFont.pointSize
    }

    contentItem: ColumnLayout {
        spacing: 8
        transform: Translate { y: pop.rise }

        // ---------------------------------------------------------------- mic
        ColumnLayout {
            visible: pop.section === "mic"
            Layout.fillWidth: true
            spacing: 8
            Head {
                title: "Microphone"
                badge: app.mMicMuted ? "MUTED" : (app.mGameChat ? "GAME CHAT" : "LIVE")
                stateColor: app.mMicMuted ? Th.callbarPopoverBadgeFgMuted : Th.callbarPopoverBadgeFgLive
            }
            ThComboBox {
                Layout.fillWidth: true
                textRole: "label"
                model: [{ node: "", label: "System default" }].concat(app.mInputs)
                currentIndex: {
                    for (var i = 0; i < model.length; i++) if (model[i].node === app.mInputName) return i
                    return 0
                }
                onActivated: (idx) => ctl.setInput(model[idx].node)
            }
            RowLayout {
                Layout.fillWidth: true
                spacing: 10
                Controls.Label { text: "Level"; color: Th.callbarPopoverHintFg; font.pointSize: Kirigami.Theme.smallFont.pointSize }
                Meter { level: app.mMicLevel }
                // 10.5: the mic check (hear.rs — the RAW capture). Replaces "no signal", which read the
                // processed audio and so appeared on every pause of a gated mic.
                Controls.Label {
                    readonly property string st: (app.mHear && app.mHear.mic) ? app.mHear.mic.check : "ok"
                    visible: app.mInCall && st !== "ok" && st !== "wait"
                    text: st === "hint" ? "no sound yet" : (st === "dead" ? "nothing from this mic" : (st === "osmuted" ? "muted in system" : "volume 0 %"))
                    color: st === "hint" ? Th.callbarPopoverHintFg : Th.callbarPopoverFgError
                    font.pointSize: Kirigami.Theme.smallFont.pointSize
                }
            }
            // 10.5: who receives us — reported by the listeners' own apps (tinhat.rx), not guessed.
            Controls.Label {
                visible: app.mInCall && app.heardSummary().length > 0
                Layout.fillWidth: true
                text: app.heardSummary()
                wrapMode: Text.WordWrap
                color: (app.mHear && app.mHear.heard && app.mHear.heard.not && app.mHear.heard.not.length > 0)
                       ? Th.callbarPopoverFgError : Th.callbarPopoverHintFg
                font.pointSize: Kirigami.Theme.smallFont.pointSize
            }
            Kirigami.Separator { Layout.fillWidth: true; opacity: 0.5 }
            ThCheckBox {
                text: app.mEcDegraded ? "Echo cancellation — NOT ACTIVE" : "Echo cancellation"
                checked: app.mMicAec
                onToggled: { ctl.applyMicAec(checked); checked = Qt.binding(() => app.mMicAec) }
                labelColor: app.mEcDegraded ? Th.callbarPopoverFgError : Th.callbarPopoverFg
            }
            RowLayout {
                Layout.fillWidth: true
                Layout.leftMargin: 28
                spacing: 8
                ThButton {
                    app: pop.app;
                    text: "Speaker outputs" + (app.mEcOutputs.length > 0 ? " (" + app.mEcOutputs.length + ")" : "…")
                    enabled: app.mMicAec
                    onClicked: app.openEcOutputs()
                    Controls.ToolTip.visible: hovered
                    Controls.ToolTip.text: "Pick every output that plays through your speakers — all of them get cancelled."
                }
                Controls.Label {
                    Layout.fillWidth: true
                    visible: app.mEcOutputs.length > 0
                    text: app.ecOutputLabels()
                    elide: Text.ElideRight
                    color: Th.callbarPopoverHintFg
                    font.pointSize: Kirigami.Theme.smallFont.pointSize
                }
            }
            Hint {
                Layout.leftMargin: 28
                visible: app.mMicAec && app.mEcNote.length > 0 && app.mInCall
                text: app.mEcNote
            }
            ThCheckBox {
                text: "Auto gain control"
                checked: app.mMicAgc
                onToggled: { ctl.applyMicAgc(checked); checked = Qt.binding(() => app.mMicAgc) }
            }
            ThCheckBox {
                text: "Push to talk"
                checked: ctl.pushToTalk
                onToggled: ctl.applyPushToTalk(checked)
            }
            Flow {
                Layout.fillWidth: true
                spacing: 4
                Controls.Label { text: "Noise suppression"; color: Th.callbarPopoverFg; height: nsOff.height; verticalAlignment: Text.AlignVCenter; rightPadding: 6 }
                ThRadioButton {
                    id: nsOff
                    text: "Off"
                    checked: app.mMicNsMode === "off"
                    onClicked: { ctl.applyMicNsMode("off"); checked = Qt.binding(() => app.mMicNsMode === "off") }
                }
                ThRadioButton {
                    text: "Standard"
                    checked: app.mMicNsMode === "standard"
                    onClicked: { ctl.applyMicNsMode("standard"); checked = Qt.binding(() => app.mMicNsMode === "standard") }
                }
                ThRadioButton {
                    enabled: app.mNcAvailable
                    opacity: app.mNcAvailable ? 1.0 : 0.5
                    text: app.mNcAvailable ? "Strong" : "Strong (not installed)"
                    checked: app.mMicNsMode === "strong" && app.mNcAvailable
                    onClicked: { ctl.applyMicNsMode("strong"); checked = Qt.binding(() => app.mMicNsMode === "strong" && app.mNcAvailable) }
                }
            }
        }

        // ------------------------------------------------------------ speaker
        ColumnLayout {
            visible: pop.section === "speaker"
            Layout.fillWidth: true
            spacing: 8
            Head {
                title: "Speaker"
                badge: app.mDeafened ? "DEAFENED" : ""
                stateColor: Th.callbarPopoverBadgeFgMuted
            }
            ThComboBox {
                Layout.fillWidth: true
                textRole: "label"
                model: [{ node: "", label: "System default" }].concat(app.mOutputs)
                currentIndex: {
                    for (var i = 0; i < model.length; i++) if (model[i].node === app.mOutputName) return i
                    return 0
                }
                onActivated: (idx) => ctl.setOutput(model[idx].node)
            }
            RowLayout {
                Layout.fillWidth: true
                spacing: 10
                Controls.Label { text: "Level"; color: Th.callbarPopoverHintFg; font.pointSize: Kirigami.Theme.smallFont.pointSize }
                Meter { level: app.mOutputLevel }
                Controls.Label {
                    visible: app.mOutputMissing
                    text: "no output device"
                    color: Th.callbarPopoverFgError
                    font.pointSize: Kirigami.Theme.smallFont.pointSize
                }
            }
            Kirigami.Separator { Layout.fillWidth: true; opacity: 0.5 }
            ThCheckBox {
                text: "Deafen — mute everyone (Ctrl+Shift+D)"
                checked: app.mDeafened
                onToggled: { ctl.applyDeafen(checked); checked = Qt.binding(() => app.mDeafened) }
            }
            Hint { text: "Someone too loud or too quiet? Right-click their tile (or their name in the left bar) for per-person volumes." }
        }

        // ------------------------------------------------------------- camera
        ColumnLayout {
            visible: pop.section === "camera"
            Layout.fillWidth: true
            spacing: 8
            Head {
                title: "Camera"
                badge: app.mCameraOn ? "ON" : "OFF"
                stateColor: app.mCameraOn ? Th.callbarPopoverBadgeFgLive : Th.callbarPopoverBadgeFg
            }
            ThComboBox {
                Layout.fillWidth: true
                textRole: "label"
                model: [{ node: "", label: "Default (first camera)" }].concat(app.mCameras)
                currentIndex: {
                    for (var i = 0; i < model.length; i++) if (model[i].node === app.mCameraName) return i
                    return 0
                }
                onActivated: (idx) => ctl.setCamera(model[idx].node)
            }
            ThComboBox {
                Layout.fillWidth: true
                textRole: "label"
                model: [
                    { label: "720p 30fps (default)",             w: 1280, h: 720,  fps: 30, kbps: 2500 },
                    { label: "1080p 30fps — needs a >1080p cam", w: 1920, h: 1080, fps: 30, kbps: 3000 }
                ]
                currentIndex: app._cqW() === 1920 ? 1 : 0
                onActivated: (idx) => ctl.applyCamQuality(model[idx].w, model[idx].h, model[idx].fps, model[idx].kbps)
            }
            ThCheckBox {
                text: "Adaptive layers"
                checked: app._cqSim()
                onToggled: { ctl.applyCamSimulcast(checked); checked = Qt.binding(() => app._cqSim()) }
                Controls.ToolTip.visible: hovered
                Controls.ToolTip.text: "Publish 3 quality rungs so each viewer's connection picks what fits (NVENC only)"
            }
            Hint { text: "A device change applies to the next camera start; the quality applies live." }
        }

        // ------------------------------------------------------------- footer
        RowLayout {
            Layout.fillWidth: true
            Layout.topMargin: 2
            Item { Layout.fillWidth: true }
            ThButton {
                app: pop.app;
                flat: true
                text: "All settings…"
                onClicked: {
                    pop.close()
                    if (pop.section === "camera") app.openCallSettings()
                    else app.openAudioSettings()
                }
            }
        }
    }
}
