// ThemeEditorIconButton.qml — the theme editor header's icon button (THEMES-UI.md §13.6; the web's editor.module.css
// `.iconBtn`): 32 × 32, a 16 px glyph in settings.fg, radius 5; hover = settings.themeEditor.row.bg.hover, a pressed
// toggle (Pick) = settings.themeEditor.row.bg.selected, disabled at 0.4; the keyboard ring 2 px inside the edge.
import QtQuick
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the settings.themeEditor.* tokens

Controls.AbstractButton {
    id: b
    property string iconName: ""
    property string tip: ""
    property bool pressedOn: false          // a checkable header button that is on (Pick armed)

    implicitWidth: 32
    implicitHeight: 32
    hoverEnabled: true
    focusPolicy: Qt.StrongFocus
    opacity: enabled ? 1.0 : 0.4
    Accessible.role: Accessible.Button
    Accessible.name: b.tip
    Accessible.checkable: b.pressedOn
    Controls.ToolTip.text: b.tip
    Controls.ToolTip.visible: b.hovered && b.tip.length > 0
    Controls.ToolTip.delay: 600

    background: Rectangle {
        radius: 5
        color: b.pressedOn ? Th.settingsThemeEditorRowBgSelected
             : (b.hovered && b.enabled ? Th.settingsThemeEditorRowBgHover : "transparent")
        border.width: b.visualFocus ? 2 : 0
        border.color: Th.settingsThemeCardBorderFocus
    }
    contentItem: Item {
        Kirigami.Icon {
            anchors.centerIn: parent
            width: 16; height: 16
            source: b.iconName
            color: Th.settingsFg
            isMask: true
        }
    }
}
