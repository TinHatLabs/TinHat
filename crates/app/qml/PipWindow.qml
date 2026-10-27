// PipWindow.qml — #9 Stream pop-out (PiP): a small always-on-top window showing ONE call
// tile's video. The video here is a pure VIEWER (PipVideoItem, a CPU-painted item fed by
// a second libwebrtc sink on the same track — callctl.pipStart/pipStop): the grid tile's
// GL pipeline is never borrowed or re-pointed. WHY: qml6glsink in a second QQuickWindow
// needs cross-context GL texture sharing, which NVIDIA silently breaks (black box, zero
// errors — the r137 rebuild architecture fixed Intel but not the NVIDIA desktop). The CPU
// path has no GL hand-off at all, so it renders identically on every driver.
//
// 0.8.21: the pop-out carries the tile's controls — a hover strip on top with
// the person's name, mute + volume for THIS stream (the same per-(person × category)
// prefs the tile and the left bar use), the quality pick, and close. It shows for a
// moment when the window opens so people know it is there, then only on hover.
import QtQuick
import QtQuick.Window
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app // PipVideoItem (cxx-qt qml_element)

Window {
    id: pip
    // The CallController + AudioPrefsController, injected by AppCore.openPip(). Colours: the pip.* tokens and, for the
    // window's own Kirigami.Theme / the quality menu's palette, the roles (Th).
    property var ctl
    property var audioPrefs
    // The grid tile id whose track we view (== the streamer video-tile registry key).
    property string tileId: ""
    property string participant: ""     // full LiveKit identity (@user:server:DEVICE)
    property string name: ""            // resolved display name
    property bool isShare: false        // screen share (vs camera) — picks the gain category
    property string volMxid: ""         // BARE mxid — the audio-prefs key (Main.qml tileMxid)
    readonly property string volCat: isShare ? "screenshare" : "mic"
    // Display name, else the localpart of the identity (never the raw @user:server:DEVICE).
    readonly property string who: name.length > 0 ? name
        : ((/^@([^:]+):/.exec(participant) || [])[1] || participant)
    // "" (unknown) reads as Auto.
    readonly property string qualityLabel: (quality === "" || quality === "auto") ? "Auto" : quality + "p"
    property real vol: 1.0
    property bool muted: false
    property string quality: "auto"

    width: 360
    height: 214
    minimumWidth: 260
    minimumHeight: 150
    title: pip.who + (pip.isShare ? " — stream" : "") + " — Tin Hat"   // 10.1 (TH-POP-22): the app's name, like every other window
    color: Th.pipBg
    // Decoration hints are EXPLICIT: once anything is OR'd onto Qt.Window, Qt
    // uses the flag set as-is — KWin decorates server-side regardless (why this
    // looked fine on Linux), but on Windows the bare combo renders frameless and
    // undraggable (found live, first bare-metal session).
    flags: Qt.Window | Qt.WindowStaysOnTopHint | Qt.WindowTitleHint |
           Qt.WindowSystemMenuHint | Qt.WindowCloseButtonHint | Qt.WindowMinMaxButtonsHint
    visible: true

    function openQualityMenu() { qualityMenu.popup(qualityBtn, 0, qualityBtn.height + 4) }
    Shortcut { sequence: "Escape"; onActivated: pip.close() }   // 10.1 (TH-A11Y-12)

    Component.onCompleted: {
        if (audioPrefs && volMxid.length > 0) {
            vol = parseFloat(audioPrefs.volumeFor(volMxid, volCat))
            muted = audioPrefs.mutedFor(volMxid, volCat)
        }
        if (ctl) quality = ctl.tileQuality(tileId)
    }

    // Drag-anywhere fallback (same startSystemMove recipe as FloatingChats):
    // harmless with decorations present, and keeps the window movable even on
    // platforms that ignore the decoration hints.
    DragHandler {
        target: null
        onActiveChanged: if (active) pip.startSystemMove()
    }

    // The video surface: registers itself (by tileId) to receive the tap's RGBA frames.
    PipVideoItem {
        id: surface
        anchors.fill: parent
        tileId: pip.tileId
        Component.onCompleted: attachViewer()
        Component.onDestruction: detachViewer()
    }
    // 10.1 (TH-POP-17): a black box with a name reads as broken — say what is happening.
    Column {
        anchors.centerIn: parent
        visible: !surface.hasFrame
        spacing: 6
        ThIcon { anchors.horizontalCenter: parent.horizontalCenter; app: null; name: pip.isShare ? "monitor-symbolic" : "camera-video-symbolic"; width: 36; height: 36; color: Th.pipEmptyIcon; isMask: true }
        Text { textFormat: Text.PlainText; anchors.horizontalCenter: parent.horizontalCenter; text: waitLong.running ? "Waiting for video…" : ("No video from " + pip.who + " right now"); color: Th.pipEmptyFg }
        Timer { id: waitLong; interval: 4000; running: true }
    }

    // Start the viewer tap; retries until the tile's track handle exists (mirrors the
    // grid tile's 120ms attach poll — a pop-out can only be clicked on a live tile, so
    // this normally succeeds on the first tick).
    Timer {
        interval: 120
        repeat: true
        running: true
        onTriggered: if (pip.ctl && pip.ctl.pipStart(pip.tileId)) running = false
    }

    // Stop the tap with the window. The grid tile needs no hand-back — it kept playing.
    onClosing: if (ctl) ctl.pipStop(tileId)

    // Controls show on hover, and for a moment right after opening.
    HoverHandler { id: hov }
    Timer { id: introTimer; interval: 2500; running: true }
    readonly property bool showControls: hov.hovered || introTimer.running || qualityMenu.visible

    // Caption strip (bottom) — the name, always.
    Rectangle {
        anchors { left: parent.left; right: parent.right; bottom: parent.bottom }
        height: 26
        gradient: Gradient {
            GradientStop { position: 0.0; color: "transparent" }
            GradientStop { position: 1.0; color: Th.pipCaptionBg }
        }
        Controls.Label {
            textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
            anchors { left: parent.left; bottom: parent.bottom; margins: 6 }
            text: pip.who + (pip.isShare ? " — stream" : "")
            color: Th.pipFg
            font.bold: true
            elide: Text.ElideRight
            width: parent.width - 12
        }
    }

    // Control strip (top): mute · volume · quality · close.
    Rectangle {
        id: strip
        anchors { left: parent.left; right: parent.right; top: parent.top }
        height: 40
        opacity: pip.showControls ? 1.0 : 0.0
        Behavior on opacity { NumberAnimation { duration: Motion.durSlow } }
        gradient: Gradient {
            GradientStop { position: 0.0; color: Th.pipStripBg }
            GradientStop { position: 1.0; color: "transparent" }
        }
        // Kirigami.Theme customization does NOT cross window boundaries, and this window's
        // root children share no themable ancestor item — re-declare the app scheme ONCE
        // here; every control below inherits it instead of the SYSTEM color scheme.
        Item {
            id: themed
            anchors.fill: parent
            Kirigami.Theme.inherit: false
            Kirigami.Theme.backgroundColor: Th.surfaceWindow
            Kirigami.Theme.textColor: Th.textPrimary
            Kirigami.Theme.disabledTextColor: Th.textSecondary
            Kirigami.Theme.highlightColor: Th.accentBase
            Kirigami.Theme.highlightedTextColor: Th.textOnAccent
            Kirigami.Theme.alternateBackgroundColor: Th.surfaceIsland
            Kirigami.Theme.activeTextColor: Th.accentBright
            Kirigami.Theme.linkColor: Th.accentBright
            Kirigami.Theme.positiveTextColor: Th.stateSuccess
            Kirigami.Theme.negativeTextColor: Th.stateDangerBright
            Kirigami.Theme.hoverColor: Th.accentBase
            Kirigami.Theme.focusColor: Th.accentBase
            RowLayout {
                anchors { left: parent.left; right: parent.right; top: parent.top; margins: 4 }
                spacing: 2
                // Mute THIS stream, just for me (same lever as the tile's speaker button).
                Controls.ToolButton {
                    icon.name: pip.muted ? "audio-volume-muted-symbolic" : "audio-volume-high-symbolic"
                    icon.color: pip.muted ? Th.pipButtonIconMuted : Th.pipButtonIcon
                    display: Controls.AbstractButton.IconOnly
                    Controls.ToolTip.text: pip.muted ? "Unmute for me" : "Mute for me"
                    Controls.ToolTip.visible: hovered
                    Controls.ToolTip.delay: 400
                    onClicked: {
                        if (!pip.audioPrefs || pip.volMxid.length === 0) return
                        pip.muted = !pip.muted
                        pip.audioPrefs.setMuted(pip.volMxid, pip.volCat, pip.muted)
                    }
                }
                ThSlider {
                    id: volSlider
                    callHandle: true   // the web's pop-out (Pip.module.css .slider): the call menus' handle
                    Layout.fillWidth: true
                    Layout.minimumWidth: 60
                    from: 0.0; to: 2.0
                    stepSize: 0.01   // the web's pop-out dial (Pip.tsx stepSize 0.01): whole percents
                    snapMode: Controls.Slider.SnapAlways
                    wheelEnabled: true   // as Plasma's style and the web (Basic ignored the wheel)
                    Kirigami.StyleHints.tickMarkStepSize: -1   // no Plasma tick row (a tick per step)
                    value: pip.vol
                    enabled: !pip.muted && pip.volMxid.length > 0
                    // Live-push while dragging (no disk write); commit once on release —
                    // or at once for a wheel notch (a move without a press).
                    onMoved: {
                        pip.vol = value
                        if (pip.audioPrefs && pip.volMxid.length > 0) {
                            pip.audioPrefs.pushVolume(pip.volMxid, pip.volCat, value)
                            if (!pressed) pip.audioPrefs.setVolume(pip.volMxid, pip.volCat, value)
                        }
                    }
                    onPressedChanged: {
                        if (!pressed && pip.audioPrefs && pip.volMxid.length > 0)
                            pip.audioPrefs.setVolume(pip.volMxid, pip.volCat, value)
                    }
                }
                Controls.Label {
                    textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                    text: Math.round(pip.vol * 100) + "%"
                    color: Th.pipFg
                    Layout.preferredWidth: 40
                    horizontalAlignment: Text.AlignRight
                    font.pointSize: Kirigami.Theme.smallFont.pointSize
                }
                // Quality pick — the same layers the tile's stream menu offers. Explicit
                // colours throughout: a stock ToolButton's TEXT rendered invisible in this
                // second window (icon buttons with icon.color were fine).
                Rectangle {
                    id: qualityBtn
                    implicitHeight: 24
                    implicitWidth: qualityLbl.implicitWidth + 18
                    radius: 12
                    color: qualityTap.pressed || qualityMenu.visible ? Th.pipQualityPillBgPressed : Th.pipQualityPillBg
                    border.width: 1
                    border.color: Th.pipQualityPillBorder
                    Controls.Label {
                        textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                        id: qualityLbl
                        anchors.centerIn: parent
                        text: pip.qualityLabel + " ▾"
                        color: Th.pipFg
                        font.pointSize: Kirigami.Theme.smallFont.pointSize
                        font.bold: true
                    }
                    TapHandler { id: qualityTap; onTapped: pip.openQualityMenu() }
                    Controls.ToolTip.text: "Quality"
                    Controls.ToolTip.visible: qualityHover.hovered && !qualityMenu.visible
                    Controls.ToolTip.delay: 400
                    HoverHandler { id: qualityHover }
                    Controls.Menu {
                        id: qualityMenu
                        margins: 6
                        // Popups don't inherit the wrapper's theme: set both schemes explicitly.
                        Kirigami.Theme.inherit: false
                        Kirigami.Theme.backgroundColor: Th.pipMenuBg
                        Kirigami.Theme.textColor: Th.textPrimary
                        Kirigami.Theme.highlightColor: Th.accentBase
                        Kirigami.Theme.highlightedTextColor: Th.textOnAccent
                        palette.window: Th.pipMenuBg
                        palette.base: Th.pipMenuBg
                        palette.text: Th.textPrimary
                        palette.windowText: Th.textPrimary
                        palette.buttonText: Th.textPrimary
                        palette.highlight: Th.accentBase
                        palette.highlightedText: Th.textOnAccent
                        Repeater {
                            model: [
                                { label: "Auto",  mode: "auto" },
                                { label: "1080p", mode: "1080" },
                                { label: "720p",  mode: "720" },
                                { label: "360p",  mode: "360" },
                                { label: "180p",  mode: "180" }
                            ]
                            delegate: Controls.MenuItem {
                                required property var modelData
                                text: LinkGate.plain(((pip.quality === modelData.mode || (pip.quality === "" && modelData.mode === "auto")) ? "✓ " : "   ") + modelData.label)
                                palette.text: Th.textPrimary
                                palette.windowText: Th.textPrimary
                                onTriggered: {
                                    if (pip.ctl) pip.ctl.applyTileQuality(pip.tileId, modelData.mode)
                                    pip.quality = modelData.mode
                                }
                            }
                        }
                    }
                }
                // Close → the grid tile is already showing the stream; just shut this viewer.
                Controls.ToolButton {
                    icon.name: "window-close-symbolic"
                    icon.color: Th.pipButtonIcon
                    display: Controls.AbstractButton.IconOnly
                    Controls.ToolTip.text: "Close pop-out"
                    Controls.ToolTip.visible: hovered
                    Controls.ToolTip.delay: 400
                    onClicked: pip.close()
                }
            }
        }
    }
}
