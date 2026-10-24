// A pool tier: heading + a wrap of small covers with owners / missing names / removal reasons.
import QtQuick
import dev.tinhat.app   // Th: the reel.* tokens
import QtQuick.Layouts

ColumnLayout {
    id: tier
    required property var app
    required property var panel
    property string title: ""
    property string subtitle: ""
    property var games: []
    property var artFn: function (id) { return "" }
    property bool dim: false
    property bool showReason: false
    spacing: 6
    Text { text: tier.title; color: Th.reelSectionFg; font.pixelSize: 12; font.letterSpacing: 1; font.capitalization: Font.AllUppercase }
    Text { visible: tier.subtitle.length > 0; text: tier.subtitle; color: Th.reelMetaFg; font.pixelSize: 11; wrapMode: Text.WordWrap; Layout.fillWidth: true }
    Flow {
        Layout.fillWidth: true
        spacing: 10
        Repeater {
            model: tier.games
            delegate: Item {
                required property var modelData
                width: 96; height: 176
                opacity: tier.dim ? 0.6 : 1
                ColumnLayout {
                    anchors.fill: parent
                    spacing: 4
                    Rectangle {
                        Layout.fillWidth: true; Layout.preferredHeight: 128; radius: app.radiusSm; color: Th.reelTierArtBg; clip: true
                        Image { anchors.fill: parent; source: tier.artFn(modelData.id); fillMode: Image.PreserveAspectCrop; asynchronous: true }
                        Rectangle { visible: !!modelData.weight; anchors { left: parent.left; right: parent.right; bottom: parent.bottom } height: 4; color: Th.reelOddsTrack
                                    Rectangle { width: parent.width * Math.min(1, modelData.odds || 0); height: 4; color: Th.reelOddsFill } }
                    }
                    Text { text: modelData.name || modelData.id; color: Th.reelFg; font.pixelSize: 11; elide: Text.ElideRight; Layout.fillWidth: true }
                    Text {
                        Layout.fillWidth: true
                        elide: Text.ElideRight
                        color: Th.reelMetaFg; font.pixelSize: 10
                        text: tier.showReason ? (modelData.reason || "")
                              : (modelData.missing && modelData.missing.length) ? "Not " + modelData.missing.map(function (m) { return tier.panel.nameOf(m) }).join(", ")
                              : (modelData.owners || []).length + " of you would play this"
                    }
                }
                HoverHandler { id: h }
                Rectangle {
                    visible: h.hovered && (modelData.explain || []).length > 0
                    anchors { left: parent.left; right: parent.right; top: parent.top }
                    height: t.implicitHeight + 10; radius: app.radiusSm; color: Th.reelTierExplainBg; z: 5
                    Text { id: t; anchors { fill: parent; margins: 5 } text: (modelData.explain || []).join("\n"); color: Th.reelTierExplainFg; font.pixelSize: 10; wrapMode: Text.WordWrap }
                }
            }
        }
    }
}
