// A Templates button in the reel.button.* tokens (both shells). Primary / quiet / checked variants.
import QtQuick
import QtQuick.Templates as T
import dev.tinhat.app   // Th: the reel.button.* tokens

T.Button {
    id: b
    property real radius: 8
    property bool primary: false
    property bool quiet: false
    property string hint: ""
    implicitWidth: Math.max(72, implicitContentWidth + 28)
    implicitHeight: 34
    hoverEnabled: true
    background: Rectangle {
        radius: b.radius
        color: b.primary ? (b.pressed ? Th.reelButtonPrimaryBgPressed : b.hovered ? Th.reelButtonPrimaryBgHover : Th.reelButtonPrimaryBg)
             // PARITY P72 (fix-desktop): 10.6 read .r/.g/.b of the palette's accentBright STRING (undefined → black), so the
             // checked fill is black at the token's alpha on the desktop; the web paints the token (accent.bright at 22 %).
             : b.checked ? Qt.rgba(0, 0, 0, Th.reelButtonBgChecked.a)   // theme-lint: ok PARITY P72 — 10.6's black wash, 0 px in 1a
             : b.quiet ? (b.hovered ? Th.reelButtonQuietBgHover : "transparent")
             : (b.hovered ? Th.reelButtonBgHover : Th.reelButtonBg)
        border.color: b.primary ? "transparent" : b.checked ? Th.reelButtonBorderChecked : Th.reelButtonBorder
        opacity: b.enabled ? 1 : 0.45
    }
    contentItem: Text {
        textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
        text: b.text
        color: b.primary ? Th.reelButtonPrimaryFg : Th.reelButtonFg
        font.pixelSize: 13
        font.weight: b.primary ? Font.DemiBold : Font.Medium
        horizontalAlignment: Text.AlignHCenter
        verticalAlignment: Text.AlignVCenter
        opacity: b.enabled ? 1 : 0.6
    }
    T.ToolTip.visible: hint.length > 0 && hovered
    T.ToolTip.text: hint
    T.ToolTip.delay: 500
}
