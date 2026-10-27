// ThTextField — the house single-line field (0.8.33; on QtQuick.Templates since 0.8.34:
// no platform style underneath — see ThComboBox). Stock TextField API, our visuals: dark
// rounded frame (LoginField's look), accent focus ring, dim placeholder — identical on Linux and
// Windows whatever QQC2 style the machine runs. Colours: the input.* tokens of Th.
import QtQuick
import QtQuick.Templates as T
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the active theme (docs/MIGRATING-THEME.md)

T.TextField {
    id: field
    property real radius: 8

    // The Basic-style sizing rule, so layouts measure exactly as they did.
    implicitWidth: implicitBackgroundWidth + leftInset + rightInset
                   || Math.max(contentWidth, placeholder.implicitWidth) + leftPadding + rightPadding
    implicitHeight: 34
    leftPadding: 10
    rightPadding: 10
    hoverEnabled: true
    font.pointSize: Kirigami.Theme.defaultFont.pointSize * Th.textScale   // ENGINE.md §10: the theme's text size
    color: Th.inputFg
    placeholderTextColor: Th.inputPlaceholder
    selectionColor: Th.inputSelectionBg
    selectedTextColor: Th.inputSelectionFg
    selectByMouse: true
    verticalAlignment: Text.AlignVCenter

    // The template draws no placeholder: this is it.
    Text {
        textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
        id: placeholder
        x: field.leftPadding
        y: field.topPadding
        width: field.width - (field.leftPadding + field.rightPadding)
        height: field.height - (field.topPadding + field.bottomPadding)
        text: field.placeholderText
        font: field.font
        color: field.placeholderTextColor
        verticalAlignment: field.verticalAlignment
        visible: !field.length && !field.preeditText && (!field.activeFocus || field.horizontalAlignment !== Qt.AlignHCenter)
        elide: Text.ElideRight
        renderType: field.renderType
    }
    background: Rectangle {
        opacity: parent && parent.enabled === false ? 0.5 : 1.0   // 10.1 (TH-DS-09): a disabled field looks disabled
        radius: field.radius
        color: field.activeFocus ? Th.inputBgFocus : (field.hovered ? Th.inputBgHover : Th.inputBg)
        border.width: field.activeFocus ? 2 : 1
        border.color: field.activeFocus ? Th.inputBorderFocus : Th.inputBorder
        Behavior on color { ColorAnimation { duration: Motion.dur } }
        Behavior on border.color { ColorAnimation { duration: Motion.dur } }
    }
}
