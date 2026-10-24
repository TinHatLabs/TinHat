// ThemeShareButton.qml — a share card's Try / Keep button (THEMES-UI.md §14.3; the web's share.module.css `.btn`),
// painted in the SHARED theme's roles (`k` = the card's colours): 30 high, radius 6, padding 0 12, body text. Try: fill
// surface.raised, a 1 px border.subtle edge, text.primary, hover surface.hover. Keep (`primary`): fill accent.base,
// text.onAccent, hover accent.bright. Disabled at 0.55; the keyboard ring 2 px in the card's border.focus, 1 px outside.
import QtQuick
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th.textScale

Controls.AbstractButton {
    id: b
    property var k: ({})
    property bool primary: false
    property string tip: ""

    implicitHeight: 30
    implicitWidth: label.implicitWidth + 24
    hoverEnabled: true
    focusPolicy: Qt.StrongFocus
    opacity: enabled ? 1.0 : 0.55
    Accessible.role: Accessible.Button
    Controls.ToolTip.text: b.tip
    Controls.ToolTip.visible: b.tip.length > 0 && hoverHandler.hovered
    Controls.ToolTip.delay: 600
    // a disabled button still shows its tooltip (the web's title on a disabled button)
    HoverHandler { id: hoverHandler }

    background: Item {
        Rectangle {
            anchors.fill: parent
            radius: 6
            color: b.primary ? ((b.hovered && b.enabled) ? (b.k.bright || "transparent") : (b.k.accent || "transparent"))
                             : ((b.hovered && b.enabled) ? (b.k.hover || "transparent") : (b.k.raised || "transparent"))
            border.width: b.primary ? 0 : 1
            border.color: b.k.border || "transparent"
        }
        Rectangle {
            anchors.fill: parent
            anchors.margins: -3
            radius: 9
            color: "transparent"
            visible: b.visualFocus
            border.width: 2
            border.color: b.k.focus || "transparent"
        }
    }
    contentItem: Item {
        // a Label (not a Text): it inherits the window's font — a theme's interface font (ENGINE.md §10)
        Controls.Label {
            id: label
            anchors.centerIn: parent
            text: b.text
            color: b.primary ? (b.k.on || "transparent") : (b.k.fg || "transparent")
            font.pointSize: Kirigami.Theme.defaultFont.pointSize * Th.textScale
        }
    }
}
