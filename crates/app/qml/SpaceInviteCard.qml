// SpaceInviteCard — what the room panel shows for a space you were INVITED to but have not
// joined (RBAC, docs/RBAC.md §7.3). The rooms only exist for you after joining, so the list
// is replaced by one card: who asked, which space, how big it is, and Join / Decline.
// Props in, signals out; the host wires `join` to login.acceptInvite and `decline` to
// login.rejectInvite (a space is a room).
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the rooms.inviteCard.* tokens

Item {
    id: card
    required property var app
    property string spaceName: ""
    property string spaceId: ""     // the colour (selfhost/firstrun §6): a space is its id, never its name
    property string avatar: ""
    property string inviter: ""
    property string inviterMxid: ""
    property int memberCount: 0
    property bool busy: false
    signal join()
    signal decline()

    FallbackTheme { id: fallbackTheme }
    readonly property var theme: app || fallbackTheme
    function esc(s) { return String(s || "").replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;") }

    ColumnLayout {
        anchors.centerIn: parent
        width: Math.min(parent.width - Kirigami.Units.largeSpacing * 2, Kirigami.Units.gridUnit * 16)
        spacing: Kirigami.Units.largeSpacing

        Avatar {
            Layout.alignment: Qt.AlignHCenter
            app: card.theme
            size: 64
            radius: 16            // a space is a container: squircle, not a person's circle
            name: card.spaceName
            seed: card.spaceId.length > 0 ? Seeds.avatarSeed(card.spaceId) : card.spaceName
            imageSource: card.avatar
        }
        Controls.Label {
            Layout.fillWidth: true
            horizontalAlignment: Text.AlignHCenter
            textFormat: Text.RichText
            text: (card.inviter.length > 0 ? "<b>" + card.esc(card.inviter) + "</b> invited you to " : "You're invited to ")
                  + "<b>" + card.esc(card.spaceName.length > 0 ? card.spaceName : "a space") + "</b>"
            color: Th.roomsInviteCardFg
            wrapMode: Text.WordWrap
        }
        Controls.Label {
            textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
            Layout.alignment: Qt.AlignHCenter
            visible: card.memberCount > 0 || card.inviterMxid.length > 0
            text: (card.memberCount > 0 ? (card.memberCount + (card.memberCount === 1 ? " member" : " members")) : "")
                  + (card.memberCount > 0 && card.inviterMxid.length > 0 ? "  ·  " : "")
                  + card.inviterMxid
            color: Th.roomsInviteCardHintFg
            font.pointSize: Kirigami.Theme.smallFont.pointSize
            elide: Text.ElideMiddle
            Layout.maximumWidth: parent.width
        }
        GlowButton {
            Layout.fillWidth: true
            Layout.topMargin: Kirigami.Units.smallSpacing
            app: card.theme
            text: "Join space"
            busy: card.busy
            enabled: !card.busy
            onClicked: card.join()
        }
        ThButton {
            Layout.alignment: Qt.AlignHCenter
            app: card.theme
            flat: true
            text: "Decline"
            enabled: !card.busy
            onClicked: card.decline()
        }
        HintLabel {
            Layout.fillWidth: true
            app: card.theme
            horizontalAlignment: Text.AlignHCenter
            text: "You'll see its rooms after you join."
        }
    }
}
