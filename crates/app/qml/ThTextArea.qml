// ThTextArea — the house multi-line field (notes, rules) (0.8.33; on QtQuick.Templates since
// 0.8.34: no platform style underneath — see ThComboBox). Stock TextArea API,
// ThTextField's visuals. Colours: the input.* tokens of Th.
import QtQuick
import QtQuick.Templates as T
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the active theme (docs/MIGRATING-THEME.md)

T.TextArea {
    id: area
    property real radius: 8

    // The Basic-style sizing rules, so layouts measure exactly as they did.
    implicitWidth: Math.max(contentWidth + leftPadding + rightPadding,
                            implicitBackgroundWidth + leftInset + rightInset,
                            placeholder.implicitWidth + leftPadding + rightPadding)
    implicitHeight: Math.max(contentHeight + topPadding + bottomPadding,
                             implicitBackgroundHeight + topInset + bottomInset,
                             placeholder.implicitHeight + topPadding + bottomPadding)
    padding: 10
    hoverEnabled: true
    font.pointSize: Kirigami.Theme.defaultFont.pointSize * Th.textScale   // ENGINE.md §10: the theme's text size
    color: Th.inputFg
    placeholderTextColor: Th.inputPlaceholder
    selectionColor: Th.inputSelectionBg
    selectedTextColor: Th.inputSelectionFg
    selectByMouse: true
    wrapMode: TextEdit.Wrap

    Text {
        textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
        id: placeholder
        x: area.leftPadding
        y: area.topPadding
        width: area.width - (area.leftPadding + area.rightPadding)
        height: area.height - (area.topPadding + area.bottomPadding)
        text: area.placeholderText
        font: area.font
        color: area.placeholderTextColor
        verticalAlignment: area.verticalAlignment
        visible: !area.length && !area.preeditText && (!area.activeFocus || area.horizontalAlignment !== Qt.AlignHCenter)
        elide: Text.ElideRight
        renderType: area.renderType
    }
    background: Rectangle {
        opacity: parent && parent.enabled === false ? 0.5 : 1.0   // 10.1 (TH-DS-09): a disabled field looks disabled
        radius: area.radius
        color: area.activeFocus ? Th.inputBgFocus : (area.hovered ? Th.inputBgHover : Th.inputBg)
        border.width: area.activeFocus ? 2 : 1
        border.color: area.activeFocus ? Th.inputBorderFocus : Th.inputBorder
        Behavior on color { ColorAnimation { duration: Motion.dur } }
        Behavior on border.color { ColorAnimation { duration: Motion.dur } }
    }
}
