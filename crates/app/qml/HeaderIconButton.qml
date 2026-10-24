// HeaderIconButton — a round, flat icon button for the room header.
//
// Ports the web `.mx_RoomHeader_button` hover feel: scale-up + slight rotate +
// fox-orange drop-glow on hover. Colours: the button.icon.* tokens of Th. `accent` tints the
// glyph with the bright accent (used to flag active call / unverified / sharing).
import QtQuick
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the active theme (docs/MIGRATING-THEME.md)

Controls.AbstractButton {
    id: btn
    required property var app   // ApplicationWindow (theme tokens); set by parent
    FallbackTheme { id: fallbackTheme }
    readonly property var theme: app || fallbackTheme

    // NOT `icon`: AbstractButton.icon is a FINAL grouped property — overriding it makes
    // the whole component fail to load at runtime (qmlcachegen won't catch it).
    property string iconName: ""
    property string tip: ""
    property bool accent: false
    property real size: Kirigami.Units.gridUnit * 2

    implicitWidth: size
    implicitHeight: size
    hoverEnabled: true
    opacity: enabled ? 1.0 : 0.5

    Controls.ToolTip.text: btn.tip
    Controls.ToolTip.visible: btn.hovered && btn.tip.length > 0
    Controls.ToolTip.delay: 400

    background: Item {
        // Soft circular hover wash + glow.
        Rectangle {
            anchors.centerIn: parent
            width: parent.width - 4
            height: parent.height - 4
            radius: width / 2
            color: Th.buttonIconBgHover
            opacity: btn.hovered ? (btn.pressed ? 0.30 : 0.18) : 0.0
            Behavior on opacity { NumberAnimation { duration: btn.theme.dur } }
        }
        // Outer glow ring on hover (the drop-shadow feel from CSS).
        Rectangle {
            anchors.centerIn: parent
            width: parent.width + 2
            height: parent.height + 2
            radius: width / 2
            color: "transparent"
            border.width: 2
            border.color: Th.buttonIconRingHover
            opacity: btn.hovered ? 0.35 : 0.0
            Behavior on opacity { NumberAnimation { duration: btn.theme.dur } }
        }
        // Keyboard focus ring — solid accentBright, matching the field focus style.
        Rectangle {
            anchors.centerIn: parent
            width: parent.width - 2
            height: parent.height - 2
            radius: width / 2
            color: "transparent"
            border.width: 2
            border.color: Th.buttonIconRingFocus
            visible: btn.visualFocus
        }
    }

    contentItem: Item {
        // 0.8.26: ThIcon — the icon pack can animate this glyph (hover / active triggers);
        // without a pack it is the same Kirigami.Icon as before.
        ThIcon {
            id: glyph
            anchors.centerIn: parent
            width: Kirigami.Units.iconSizes.smallMedium
            height: Kirigami.Units.iconSizes.smallMedium
            app: btn.app
            name: btn.iconName
            color: btn.accent ? Th.buttonIconIconActive : Th.buttonIconIcon
            isMask: true
            hovered: btn.hovered
            active: btn.accent
            // Hover: scale up + a playful tilt (furry leans into the rotate more).
            scale: btn.hovered ? 1.16 : 1.0
            rotation: btn.hovered ? -3 : 0
            Behavior on scale { NumberAnimation { duration: btn.theme.dur; easing.type: Easing.OutBack } }
            Behavior on rotation { NumberAnimation { duration: btn.theme.dur } }
        }
    }
}
