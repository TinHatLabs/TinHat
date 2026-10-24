// WidgetCover.qml — a game's portrait art in a widget (WIDGETS-UI.md §4 Up next hero 32 × 48 r4, §6 Who's around
// 24 × 36 r3; the web's CrossFade): `widget.cover.bg` behind while loading, a 1 px `widget.cover.border` inside,
// rounded by CornerMask bites in `behind` (the colour around the cover). No art (or art that failed to load): the
// activity's emoji on the tile. coverFade: when the art CHANGES the new cover fades in over the old (verylong 400 ms,
// InOutQuad) — only while the host animates; the first load and every change otherwise just appear.
import QtQuick
import dev.tinhat.app   // Th: the active theme (docs/MIGRATING-THEME.md)

Item {
    id: cv
    property string source: ""
    property real radius: 4
    property color behind: Th.widgetBg
    property int fadeDur: 0                  // the host's verylong step (0 = no fade)
    property string emoji: ""                // no art: the activity's emoji on the tile
    property string _prev: ""
    readonly property bool showArt: cv.source.length > 0 && over.status !== Image.Error
    onSourceChanged: {
        if (cv._prev.length > 0 && cv.source.length > 0 && cv.fadeDur > 0) {
            under.source = cv._prev
            over.opacity = 0
            fade.duration = cv.fadeDur
            fade.restart()
        } else {
            fade.stop()
            under.source = ""
            over.opacity = 1
        }
        cv._prev = cv.source
    }
    Component.onCompleted: cv._prev = cv.source
    Rectangle { anchors.fill: parent; radius: cv.radius; color: Th.widgetCoverBg }
    Text {
        anchors.centerIn: parent
        visible: !cv.showArt && cv.emoji.length > 0
        text: cv.emoji
        font.pixelSize: 16
        textFormat: Text.PlainText
    }
    Image {
        id: under
        anchors.fill: parent
        fillMode: Image.PreserveAspectCrop
        asynchronous: true
        sourceSize.width: 96; sourceSize.height: 144
        visible: source.toString().length > 0 && status === Image.Ready
    }
    Image {
        id: over
        anchors.fill: parent
        source: cv.source
        fillMode: Image.PreserveAspectCrop
        asynchronous: true
        sourceSize.width: 96; sourceSize.height: 144
        visible: cv.showArt
    }
    NumberAnimation { id: fade; target: over; property: "opacity"; to: 1; easing.type: Easing.InOutQuad; onFinished: under.source = "" }
    CornerMask { radius: cv.radius; color: cv.behind }
    Rectangle { anchors.fill: parent; radius: cv.radius; color: "transparent"; border.width: 1; border.color: Th.widgetCoverBorder }
}
