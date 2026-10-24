// ThButton — the house push button (0.8.33; on QtQuick.Templates since 0.8.34: no
// platform style underneath — see ThComboBox). Stock Button API (text / icon.name /
// highlighted / flat / enabled / onClicked); dark rounded fill, accent when `highlighted`,
// transparent when `flat`, the icon drawn through ThIcon when `app` is given (the pack's
// glyph, white-on-dark on Windows too) or Kirigami.Icon otherwise. Colours: the button.* tokens of Th.
import QtQuick
import QtQuick.Templates as T
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the active theme (docs/MIGRATING-THEME.md)

T.Button {
    id: btn
    property var app: null          // Main.qml root: enables the icon pack (ThIcon)
    property bool danger: false     // red fill (destructive)
    property real radius: 8
    readonly property color fg: btn.danger ? Th.buttonDangerFg : (btn.highlighted ? Th.buttonPrimaryFg : (btn.flat ? Th.buttonGhostFg : Th.buttonSecondaryFg))

    implicitWidth: Math.max(implicitBackgroundWidth + leftInset + rightInset,
                            implicitContentWidth + leftPadding + rightPadding)
    implicitHeight: 34
    leftPadding: 14
    rightPadding: 14
    spacing: 6
    font.pointSize: Kirigami.Theme.defaultFont.pointSize * Th.textScale   // ENGINE.md §10: the theme's text size
    hoverEnabled: true
    opacity: enabled ? 1.0 : 0.55

    background: Rectangle {
        radius: btn.radius
        color: {
            if (btn.danger) return btn.pressed ? Th.buttonDangerBgPressed : (btn.hovered ? Th.buttonDangerBgHover : Th.buttonDangerBg)
            if (btn.highlighted) return btn.pressed ? Th.buttonPrimaryBgPressed : (btn.hovered ? Th.buttonPrimaryBgHover : Th.buttonPrimaryBg)
            if (btn.flat) return btn.pressed ? Th.buttonGhostBgPressed : (btn.hovered ? Th.buttonGhostBgHover : "transparent")
            return btn.pressed ? Th.buttonSecondaryBgPressed : (btn.hovered ? Th.buttonSecondaryBgHover : Th.buttonSecondaryBg)
        }
        border.width: (btn.flat && !btn.activeFocus) ? 0 : (btn.flat ? 2 : 1)   // 10.1 (TH-A11Y-09): a flat button still shows its focus ring
        border.color: btn.activeFocus ? Th.buttonBorderFocus : (btn.highlighted || btn.danger ? "transparent" : Th.buttonSecondaryBorder)
        Behavior on color { ColorAnimation { duration: Motion.durShort } }
    }
    contentItem: Row {
        spacing: iconLoader.active ? btn.spacing : 0
        leftPadding: 0
        Loader {
            id: iconLoader
            active: btn.icon.name.length > 0
            anchors.verticalCenter: parent.verticalCenter
            sourceComponent: btn.app ? packIcon : plainIcon
        }
        Component {
            id: packIcon
            ThIcon { app: btn.app; name: btn.icon.name; width: 16; height: 16; color: btn.fg; isMask: true; hovered: btn.hovered }
        }
        Component {
            id: plainIcon
            Kirigami.Icon { source: btn.icon.name; implicitWidth: 16; implicitHeight: 16; color: btn.fg; isMask: true }
        }
        Text {
            anchors.verticalCenter: parent.verticalCenter
            text: btn.text
            font: btn.font
            color: btn.fg
            elide: Text.ElideRight
        }
    }
}
