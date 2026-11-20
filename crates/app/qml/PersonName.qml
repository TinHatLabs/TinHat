// PersonName.qml — a person's name where it stands as its own label (mini games v1, GAMES-V1-SPEC §3.2), with the
// prestige token to its left (rank ≥ 1 on the rank server of the context: the room's server in a room, your own outside
// one). The web's features/spaces/PersonName.tsx is the same piece. Not for names inside sentences, tooltips, toasts or
// notifications.
//
//   PersonName { mxid: m; roomId: r; text: name; font.bold: true; color: Th.bubbleSenderFg; Layout.fillWidth: true }
//
// The label is an ordinary ThLabel-like Controls.Label (plain text, elides on the right); `label` exposes it for the
// properties a surface sets that aren't forwarded. Rank 0: the token takes no room, the label sits where it always did.
import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts
import dev.tinhat.app

RowLayout {
    id: pn
    property string mxid: ""
    property string roomId: ""
    property alias text: lbl.text
    property alias color: lbl.color
    property alias font: lbl.font
    property alias elide: lbl.elide
    property alias horizontalAlignment: lbl.horizontalAlignment
    property alias label: lbl
    property int tokenSize: 14
    readonly property int rank: tok.rank

    spacing: 0

    PrestigeToken {
        id: tok
        mxid: pn.mxid
        roomId: pn.roomId
        size: pn.tokenSize
    }
    Controls.Label {
        id: lbl
        textFormat: Text.PlainText   // a display name is never markup
        elide: Text.ElideRight
        maximumLineCount: 1
        Layout.fillWidth: true
        Layout.minimumWidth: 0
        Layout.alignment: Qt.AlignVCenter
    }
}
