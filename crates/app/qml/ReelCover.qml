// One portrait on the reel strip: cover, name, odds bar + percentage, badges, explain on hover.
import QtQuick
import dev.tinhat.app   // Th: the reel.* tokens
import QtQuick.Layouts

Item {
    id: cover
    required property var app
    property var game: ({})
    property string artUrl: ""
    property bool winner: false
    readonly property var badges: game.badges || []
    HoverHandler { id: hov }
    // 10.1 (TH-REEL-17): the winner pops once when the reel lands on it.
    transformOrigin: Item.Center
    // 11.0 (GPU.md R4): a one-shot → only while the motion switch is on; a pop in flight finishes at once when it goes off
    onWinnerChanged: if (winner && Motion.animate) winPulse.restart()
    Connections {
        target: Motion
        function onAnimateChanged() { if (!Motion.animate && winPulse.running) winPulse.complete() }
    }
    SequentialAnimation {
        id: winPulse
        // 11.0 (GPU.md R4): the whole pop = the `verylong` step (400 ms), the old 150 : 320 split
        NumberAnimation { target: cover; property: "scale"; to: 1.12; duration: Math.round(Motion.durVeryLong * 150 / 470); easing.type: Easing.OutCubic }
        NumberAnimation { target: cover; property: "scale"; to: 1.0; duration: Motion.durVeryLong - Math.round(Motion.durVeryLong * 150 / 470); easing.type: Easing.OutBack }
    }
    Rectangle {
        anchors.fill: parent
        radius: app.radius
        color: cover.winner ? Th.reelCoverBgSelected : "transparent"
        border.width: cover.winner ? 2 : 0
        border.color: Th.reelCoverBorderSelected
        Behavior on color { ColorAnimation { duration: app.dur } }
    }
    ColumnLayout {
        anchors.fill: parent
        anchors.margins: 8
        spacing: 6
        Item {
            Layout.fillWidth: true
            Layout.fillHeight: true
            Rectangle { anchors.fill: parent; radius: app.radiusSm; color: Th.reelCoverArtBg }
            Image {
                anchors.fill: parent
                source: cover.artUrl
                fillMode: Image.PreserveAspectCrop
                asynchronous: true
                cache: true
                // 10.1 (TH-REEL-16): no off-screen layer for no effect (×96 covers), and decode at the
                // size drawn instead of 600×900 per portrait. (TH-REEL-19): rounded corners for real.
                sourceSize.width: Math.max(160, Math.round(cover.width * 2))
                clip: true
                CornerMask { anchors.fill: parent; radius: app.radiusSm; color: cover.winner ? Th.reelCoverBgSelected : Th.reelBg }
            }
            // badges (top-left)
            Column {
                anchors { left: parent.left; top: parent.top; margins: 6 }
                spacing: 4
                Repeater {
                    model: cover.badges
                    delegate: Rectangle {
                        required property var modelData
                        radius: 7; height: 18; width: bt.implicitWidth + 12
                        color: modelData.kind === "credit" ? Th.reelCoverBadgeCreditBg : modelData.kind === "favorite" ? Th.reelCoverBadgeFavoriteBg : Th.reelCoverBadgeBg
                        Text { textFormat: Text.PlainText; id: bt; anchors.centerIn: parent; text: modelData.text; color: modelData.kind === "credit" || modelData.kind === "favorite" ? Th.reelCoverBadgeFilledFg : Th.reelCoverBadgeFg; font.pixelSize: 10; font.weight: Font.DemiBold }
                    }
                }
            }
        }
        Text { textFormat: Text.PlainText; text: cover.game.name || cover.game.id || ""; color: Th.reelFg; font.pixelSize: 12; font.weight: Font.DemiBold; elide: Text.ElideRight; Layout.fillWidth: true }
        RowLayout {
            Layout.fillWidth: true
            spacing: 6
            Rectangle {
                Layout.fillWidth: true; height: 6; radius: 3; color: Th.reelOddsTrack
                Rectangle { width: parent.width * Math.min(1, cover.game.odds || 0); height: parent.height; radius: 3; color: Th.reelOddsFill; Behavior on width { NumberAnimation { duration: Motion.durLong } } }
            }
            Text { textFormat: Text.PlainText; text: Math.round((cover.game.odds || 0) * 100) + "%"; color: Th.reelMetaFg; font.pixelSize: 11 }
        }
    }
    // explain on hover
    Rectangle {
        visible: hov.hovered && (cover.game.explain || []).length > 0
        anchors { left: parent.left; right: parent.right; bottom: parent.bottom; margins: 6 }
        height: ex.implicitHeight + 12
        radius: app.radiusSm
        color: Th.reelCoverExplainBg
        z: 5
        Text { id: ex; anchors { fill: parent; margins: 6 } text: (cover.game.explain || []).join("\n"); color: Th.reelCoverExplainFg; font.pixelSize: 11; wrapMode: Text.WordWrap }
    }
}
