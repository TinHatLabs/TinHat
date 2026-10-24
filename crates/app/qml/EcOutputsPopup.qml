// EcOutputsPopup.qml — 0.8.33: "which outputs go through my speakers?" — the echo canceller's
// reference list (the 2026-09-10 request: "next to echo cancellation a button where you can select
// the outputs that are actually going through the speakers which it will remember and know
// which outputs to attempt to cancel all at once").
//
// Tick every output that is audible in the room — a RODECaster's Game + Chat + Virtual A, the
// desktop speakers AND the TV, … — and Tin Hat listens to each one (its own reference and
// cancel stage, in series), so every one of them is removed from the mic. Nothing ticked = the pre-0.8.33
// behaviour: only the output Tin Hat itself plays to. The list is remembered across
// sessions (CallController.applyEcOutputs → streamer aec::set_ref_outputs) and applied live.
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import QtQuick.Templates as T
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the callbar.popover.* tokens

// QtQuick.Templates, not Controls: no platform style (breeze / Basic) underneath — see ThComboBox.
T.Popup {
    id: pop
    required property var app   // Main.qml root (mOutputs / mOutputName / mEcOutputs + helpers)
    required property var ctl   // callctl
    // The device monitor can list a sink once per provider — one row per node here.
    readonly property var outputs: {
        var seen = {}, out = []
        for (var i = 0; i < app.mOutputs.length; i++) {
            var o = app.mOutputs[i]
            if (seen[o.node]) continue
            seen[o.node] = true; out.push(o)
        }
        return out
    }

    parent: T.Overlay.overlay
    // 10.1 (TH-POP-18 / TH-POP-04): no scrim — it opened as a dimmed modal on top of the live
    // mic slide-out; now it takes the slide-out's place, like its three siblings.
    modal: false
    dim: false
    focus: true
    closePolicy: T.Popup.CloseOnEscape | T.Popup.CloseOnPressOutside
    anchors.centerIn: parent
    width: Math.min(480, (parent ? parent.width : 480) - 40)
    padding: 18
    // 10.1 (TH-POP-03): never taller than the window; the device list scrolls, Done stays put.
    implicitHeight: Math.min((parent ? parent.height : 600) - 80, contentHeight + topPadding + bottomPadding)
    T.Overlay.modal: Rectangle { color: Th.callbarPopoverOverlay }
    onOpened: ctl.refreshOutputs()

    background: Rectangle {
        color: Th.callbarPopoverBg
        radius: 14
        border.width: 1
        border.color: Th.callbarPopoverBorder
    }

    contentItem: ColumnLayout {
        spacing: 10
        RowLayout {
            Layout.fillWidth: true
            spacing: 8
            Kirigami.Icon { source: "audio-speakers-symbolic"; color: Th.callbarPopoverIconActive; implicitWidth: 22; implicitHeight: 22 }   // 10.1 (TH-POP-21): the pack glyph, not an emoji
            Controls.Label {
                Layout.fillWidth: true
                text: "Speaker outputs"
                font.bold: true
                font.pointSize: Kirigami.Theme.defaultFont.pointSize + 2
                color: Th.callbarPopoverTitleFg
            }
        }
        Controls.Label {
            Layout.fillWidth: true
            wrapMode: Text.WordWrap
            color: Th.callbarPopoverHintFg
            text: "Tick every output that actually plays through your speakers — game, chat, music, a mixer's "
                + "channels. Echo cancellation listens to all of them at once and removes what they play "
                + "from your mic. Nothing ticked = only what Tin Hat itself plays."
        }
        ThCheckBox {
            Layout.fillWidth: true
            text: "Only what Tin Hat plays to (default)"
            checked: app.mEcOutputs.length === 0
            onToggled: {
                if (checked) ctl.applyEcOutputs("[]")
                checked = Qt.binding(() => app.mEcOutputs.length === 0)
            }
        }
        Kirigami.Separator { Layout.fillWidth: true; opacity: 0.5 }
        Controls.Label {
            visible: pop.outputs.length === 0
            text: "No outputs found."
            color: Th.callbarPopoverHintFg
        }
        Controls.ScrollView {
            id: outputsScroll
            Layout.fillWidth: true
            Layout.fillHeight: true
            Layout.maximumHeight: Math.max(120, (pop.parent ? pop.parent.height : 600) - 320)
            clip: true
            Controls.ScrollBar.horizontal.policy: Controls.ScrollBar.AlwaysOff
            ColumnLayout {
                width: outputsScroll.availableWidth
                spacing: 6
                Repeater {
                    model: pop.outputs
                    delegate: ThCheckBox {
                        required property var modelData
                        Layout.fillWidth: true
                        text: modelData.label + (modelData.node === app.mOutputName ? "   — Tin Hat plays here" : "")
                        checked: app.mEcOutputs.indexOf(modelData.node) >= 0
                        onToggled: {
                            app.toggleEcOutput(modelData.node)
                            checked = Qt.binding(() => app.mEcOutputs.indexOf(modelData.node) >= 0)
                        }
                    }
                }
            }
        }
        Controls.Label {
            Layout.fillWidth: true
            wrapMode: Text.WordWrap
            color: Th.callbarPopoverHintFg
            font.pointSize: Kirigami.Theme.smallFont.pointSize
            text: app.mEcOutputs.length > 0
                ? "Cancelling what plays on: " + app.ecOutputLabels() + ". Headphones? Untick them — there is nothing to cancel from headphones."
                : "Using a mixer with several channels (Game, Chat, Virtual…)? Tick each one that reaches the speakers."
        }
        RowLayout {
            Layout.fillWidth: true
            Item { Layout.fillWidth: true }
            ThButton {
                app: pop.app; text: "Done"; highlighted: true; onClicked: pop.close() }
        }
    }
}
