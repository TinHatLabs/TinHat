// ThRadioButton — the house radio button (ThCheckBox's look, round) (0.8.33; on
// QtQuick.Templates since 0.8.34: no platform style underneath — see ThComboBox).
// Stock RadioButton API. Colours: the toggle.* tokens of Th.
import QtQuick
import QtQuick.Templates as T
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the active theme (docs/MIGRATING-THEME.md)

T.RadioButton {
    id: rb

    implicitWidth: Math.max(implicitBackgroundWidth + leftInset + rightInset,
                            implicitContentWidth + leftPadding + rightPadding)
    implicitHeight: Math.max(implicitBackgroundHeight + topInset + bottomInset,
                             implicitContentHeight + topPadding + bottomPadding,
                             implicitIndicatorHeight + topPadding + bottomPadding)
    spacing: 8
    padding: 2
    hoverEnabled: true
    font.pointSize: Kirigami.Theme.defaultFont.pointSize * Th.textScale   // ENGINE.md §10: the theme's text size

    indicator: Rectangle {
        implicitWidth: 20; implicitHeight: 20
        x: rb.leftPadding
        y: (rb.height - height) / 2
        radius: 10
        color: rb.hovered ? Th.toggleIndicatorBgHover : Th.toggleIndicatorBg
        border.width: rb.activeFocus ? 2 : 1
        border.color: rb.checked || rb.activeFocus ? Th.toggleIndicatorBorderChecked : Th.toggleIndicatorBorder
        Rectangle {
            anchors.centerIn: parent
            width: 10; height: 10; radius: 5
            color: Th.toggleIndicatorDot
            scale: rb.checked ? 1 : 0
            Behavior on scale { NumberAnimation { duration: Motion.durShort; easing.type: Easing.OutBack } }
        }
    }
    contentItem: Text {
        textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
        text: rb.text
        font: rb.font
        color: rb.enabled ? Th.toggleFg : Th.toggleFgDisabled
        opacity: rb.enabled ? 1.0 : 0.6
        verticalAlignment: Text.AlignVCenter
        leftPadding: rb.indicator.width + rb.spacing
    }
}
