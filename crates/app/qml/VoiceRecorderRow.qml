// VoiceRecorderRow.qml — voice notes (docs/VOICE-NOTES.md §2): the composer row while a voice draft exists.
//
//   recording:  [✕ Cancel]  ● 0:07  ▁▃▅▂▇▃ (live level, scrolling)  [■ Stop]  [➤ Send]
//   review:     [🗑 Delete]  [▶]  0:07  ▂▅▇▅▃ (the note's waveform, a seek slider)  [➤ Send]
//
// Everything it shows comes from the conversation's snapshot of VoiceNoteController (`vs`); every press goes back
// through the conversation (`conv.vn*`). Keyboard (§2 "Keyboard"): every control is a real button in the tab order
// (Enter/Space activate the focused one natively); the row adds only two keys, scoped to the row having focus and
// ignoring auto-repeat: Escape = Stop while recording (into review, never a discard) / back to the timeline in review,
// and Ctrl+Enter = Send while recording or in review. No unmodified Enter/Backspace/Delete key of its own.
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import QtMultimedia
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the active theme (docs/MIGRATING-THEME.md)

FocusScope {
    id: row
    required property var app
    required property var conv         // the ConversationView: vnStop / vnCancel / vnSend / vnDelete / vnEscape
    property var vs: ({ state: "idle" })
    readonly property string st: row.vs && row.vs.state ? String(row.vs.state) : "idle"
    readonly property bool recording: row.st === "requesting" || row.st === "recording" || row.st === "finalizing"
    readonly property bool reviewing: row.st === "review" || row.st === "sending"
    readonly property bool sending: row.st === "sending"
    readonly property real durMs: row.vs && row.vs.durationMs ? Number(row.vs.durationMs) : 0
    readonly property var wave: row.vs && row.vs.waveform ? row.vs.waveform : []
    readonly property var levels: row.vs && row.vs.levels ? row.vs.levels : []

    implicitHeight: col.implicitHeight
    Accessible.role: Accessible.Grouping
    Accessible.name: recording ? "Voice message recorder" : "Voice message review"

    function _clock(ms) {
        var s = Math.floor(Math.max(0, ms) / 1000)
        var m = Math.floor(s / 60)
        s = s % 60
        return m + ":" + (s < 10 ? "0" : "") + s
    }
    function focusStop() { stopBtn.forceActiveFocus() }
    function focusPlay() { playBtn.forceActiveFocus() }
    function stopPreview() { player.stop() }

    Keys.onPressed: (event) => {
        if (event.isAutoRepeat) return
        var ctrl = (event.modifiers & Qt.ControlModifier) || (event.modifiers & Qt.MetaModifier)
        if ((event.key === Qt.Key_Return || event.key === Qt.Key_Enter) && ctrl) {
            if (row.recording || row.st === "review") { player.stop(); row.conv.vnSend() }
            event.accepted = true
        } else if (event.key === Qt.Key_Escape) {
            if (row.st === "recording") row.conv.vnStop()
            else if (row.reviewing) row.conv.vnEscape()
            event.accepted = true
        }
    }

    MediaPlayer {
        id: player
        source: row.reviewing && row.vs.fileUrl ? row.vs.fileUrl : ""
        audioOutput: AudioOutput {}
    }

    ColumnLayout {
    id: col
    anchors.fill: parent
    spacing: 2
    Rectangle {
        id: box
        Layout.fillWidth: true
        implicitHeight: content.implicitHeight + 8
        radius: 12
        color: Th.composerBg
        border.width: 1
        border.color: row.activeFocus ? Th.composerBorderFocus : Th.composerBorder
        opacity: row.sending ? 0.6 : 1

        RowLayout {
            id: content
            anchors.fill: parent
            anchors.margins: 4
            spacing: Kirigami.Units.smallSpacing

            // ---- recording: Cancel · ● m:ss · live level ----
            ThToolButton {
                visible: row.recording
                app: row.app
                iconName: "window-close-symbolic"
                tip: "Cancel recording"
                Accessible.name: "Cancel recording"
                Accessible.description: "Discards the recording; nothing is sent"
                onClicked: row.conv.vnCancel()
            }
            Rectangle {
                visible: row.recording
                Layout.alignment: Qt.AlignVCenter
                implicitWidth: 10; implicitHeight: 10; radius: 5
                color: Th.stateDanger
                Accessible.ignored: true
                SequentialAnimation on opacity {
                    running: row.st === "recording" && Motion.animate
                    loops: Animation.Infinite
                    NumberAnimation { to: 0.35; duration: 700 }
                    NumberAnimation { to: 1; duration: 700 }
                }
            }
            Controls.Label {
                visible: row.recording
                textFormat: Text.PlainText
                text: row.st === "requesting" ? "Opening the microphone…" : String(row.vs.clock || "0:00")
                color: row.vs.warn === true ? Th.stateWarning : Th.composerFg
                font.bold: row.vs.warn === true
                Accessible.role: Accessible.StaticText
                Accessible.name: text
                // the timer itself is not announced every second (§2 "Screen readers")
            }
            Item {
                // the live level strip: the last ~3 s of frame peaks, scrolling (decorative)
                visible: row.recording
                Layout.fillWidth: true
                Layout.preferredHeight: 28
                Accessible.ignored: true
                Row {
                    anchors.right: parent.right
                    anchors.verticalCenter: parent.verticalCenter
                    spacing: 2
                    Repeater {
                        model: row.levels
                        delegate: Rectangle {
                            required property var modelData
                            width: 3
                            height: Math.max(2, Number(modelData) * 26)
                            radius: 1.5
                            anchors.verticalCenter: parent ? parent.verticalCenter : undefined
                            color: Th.accentBase
                        }
                    }
                }
            }
            ThToolButton {
                id: stopBtn
                visible: row.recording
                enabled: row.st === "recording"
                app: row.app
                iconName: "media-playback-stop-symbolic"
                tip: "Stop recording (Esc)"
                Accessible.name: "Stop recording"
                Accessible.description: "Stops and lets you listen before sending"
                onClicked: row.conv.vnStop()
            }

            // ---- review: Delete · Play/Pause · m:ss · waveform seek ----
            ThToolButton {
                visible: row.reviewing
                enabled: !row.sending
                app: row.app
                iconName: "user-trash-symbolic"
                tip: "Delete voice message"
                Accessible.name: "Delete voice message"
                onClicked: { player.stop(); row.conv.vnDelete() }
            }
            ThToolButton {
                id: playBtn
                visible: row.reviewing
                app: row.app
                readonly property bool playing: player.playbackState === MediaPlayer.PlayingState
                iconName: playing ? "media-playback-pause-symbolic" : "media-playback-start-symbolic"
                tip: playing ? "Pause voice message" : "Play voice message"
                Accessible.name: tip
                onClicked: playing ? player.pause() : player.play()
            }
            Controls.Label {
                visible: row.reviewing
                textFormat: Text.PlainText
                text: player.playbackState === MediaPlayer.StoppedState ? row._clock(row.durMs) : row._clock(player.position)
                color: Th.composerFg
            }
            // review r1 #8: the waveform IS a slider (the house ThSlider = Controls.Slider): value / range / step and the
            // accessibility increase / decrease actions come with it; arrows ±1 s, Home / End; the waveform is its groove
            ThSlider {
                id: seek
                visible: row.reviewing
                Layout.fillWidth: true
                Layout.preferredHeight: 30
                readonly property real total: player.duration > 0 ? player.duration : row.durMs
                from: 0
                to: Math.max(1, seek.total)
                stepSize: 1000
                live: true
                // review r2 #5: playback moves the slider through `_follow` (marked as ours); EVERY other change of its
                // value — a drag, a click, arrows, Home / End, an assistive increase / decrease — is the user's and seeks
                property bool _syncing: false
                function _follow() {
                    if (seek.pressed) return
                    seek._syncing = true
                    seek.value = player.position
                    seek._syncing = false
                }
                Connections {
                    target: player
                    function onPositionChanged() { seek._follow() }
                }
                onPressedChanged: if (!seek.pressed) player.position = seek.value
                onValueChanged: if (!seek._syncing) player.position = seek.value
                Accessible.name: "Voice message position"
                Accessible.description: row._clock(seek.value) + " of " + row._clock(seek.total)
                Keys.onPressed: (event) => {
                    if (event.key === Qt.Key_Home) { seek.value = 0; event.accepted = true }
                    else if (event.key === Qt.Key_End) { seek.value = seek.to; event.accepted = true }
                }
                background: Item {
                    x: seek.leftPadding
                    y: seek.topPadding
                    width: seek.availableWidth
                    height: seek.availableHeight
                    Rectangle {
                        anchors.fill: parent
                        radius: 6
                        color: "transparent"
                        border.width: seek.activeFocus ? 1 : 0
                        border.color: Th.composerBorderFocus
                    }
                    Row {
                        id: bars
                        anchors.fill: parent
                        anchors.margins: 3
                        spacing: 1
                        readonly property int n: Math.max(1, row.wave.length)
                        Repeater {
                            model: row.wave
                            delegate: Rectangle {
                                required property var modelData
                                required property int index
                                width: Math.max(1, (bars.width - (bars.n - 1)) / bars.n)
                                height: Math.max(2, Number(modelData) / 1024 * bars.height)
                                anchors.verticalCenter: parent ? parent.verticalCenter : undefined
                                radius: 1
                                color: (index + 0.5) / bars.n <= seek.visualPosition ? Th.accentBase : Th.textMuted
                            }
                        }
                    }
                }
                handle: Rectangle {
                    x: seek.leftPadding + seek.visualPosition * (seek.availableWidth - width)
                    y: seek.topPadding + (seek.availableHeight - height) / 2
                    implicitWidth: 3
                    implicitHeight: seek.availableHeight
                    radius: 1.5
                    color: Th.accentBase
                }
            }

            // ---- both: Send ----
            GlowButton {
                id: sendBtn
                app: row.app
                text: ""
                icon.name: "document-send-symbolic"
                busy: row.sending || row.st === "finalizing"
                enabled: row.st === "recording" || row.st === "review"
                Accessible.name: "Send voice message"
                Accessible.description: "Ctrl+Enter"
                Controls.ToolTip.text: "Send voice message (Ctrl+Enter)"
                Controls.ToolTip.visible: hovered
                Controls.ToolTip.delay: 600
                onClicked: { player.stop(); row.conv.vnSend() }
            }
        }
    }

    // the error under the row (Couldn't send — try again / too large / the microphone stopped); Send retries
    Controls.Label {
        Layout.leftMargin: 12
        Layout.fillWidth: true
        wrapMode: Text.Wrap
        visible: row.reviewing && String(row.vs.error || "").length > 0
        textFormat: Text.PlainText
        text: String(row.vs.error || "")
        color: Th.statusErrorFg
        font.pointSize: Kirigami.Theme.smallFont.pointSize
    }
    }
}
