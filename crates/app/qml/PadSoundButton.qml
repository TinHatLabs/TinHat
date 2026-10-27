// PadSoundButton.qml — one sound on the Pad window's grid (docs/PAD.md): the Soundboard pad widget's button
// (SoundPadWidget.qml PadButton, WIDGETS.md §22 — the same soundboard card tokens, rest / hover / playing / focus, the
// emoji over the name), plus Pad's two states: `playing` = the receiver's call plays it now (a tap stops it), and
// `pending` = tapped here, not confirmed by the receiver yet (DESIGN §1 "optimistic"): a 2 px ring in pad.sound.pending.ring
// (= the sound card's focus ring, as the web paints it), then the playing ring once the receiver answers.
import QtQuick
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the soundboard.* tokens + pad.sound.pending.ring

Controls.AbstractButton {
    id: pb
    property string name: ""
    property string emoji: ""
    property bool playing: false
    property bool pending: false
    implicitHeight: 56
    hoverEnabled: true
    focusPolicy: Qt.TabFocus
    opacity: pb.enabled ? 1 : 0.5
    Accessible.name: (pb.playing ? "Stop " : "Play ") + pb.name
    Controls.ToolTip.text: pb.playing ? "Stop " + pb.name : pb.name
    Controls.ToolTip.visible: hovered && nameText.truncated
    Controls.ToolTip.delay: 500
    background: Rectangle {
        radius: 10
        color: pb.playing ? Th.soundboardCardBgActive : (pb.hovered && pb.enabled ? Th.soundboardCardBgHover : Th.soundboardCardBg)
        border.width: pb.visualFocus || pb.playing || pb.pending ? 2 : 1
        border.color: pb.visualFocus ? Th.soundboardCardBorderFocus
                    : (pb.pending ? Th.padSoundPendingRing : (pb.playing ? Th.soundboardCardBorderActive : Th.soundboardCardBorder))
        Behavior on color { ColorAnimation { duration: Motion.dur } }
        Behavior on border.color { ColorAnimation { duration: Motion.dur } }
    }
    scale: pb.down ? 0.96 : 1.0
    Behavior on scale { NumberAnimation { duration: Motion.durShort } }
    contentItem: Item {
        Column {
            anchors.centerIn: parent
            width: parent.width
            spacing: 2
            Text {
                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                anchors.horizontalCenter: parent.horizontalCenter
                text: pb.emoji.length > 0 ? pb.emoji : "🔊"
                font.pixelSize: 20
                color: Th.soundboardCardFg
            }
            Controls.Label {
                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                id: nameText
                width: parent.width - 12
                anchors.horizontalCenter: parent.horizontalCenter
                horizontalAlignment: Text.AlignHCenter
                elide: Text.ElideRight
                text: pb.name
                color: Th.soundboardCardFg
                font.weight: Font.DemiBold
                font.pointSize: Kirigami.Theme.smallFont.pointSize
            }
        }
    }
}
