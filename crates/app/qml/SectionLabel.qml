// SectionLabel — settings/panel section header: bold, dim, slightly spaced. One look
// for every "group of controls starts here" moment (settings panes, menus, panels).
import QtQuick
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the active theme (docs/MIGRATING-THEME.md)

Controls.Label {
    property var app
    textFormat: Text.PlainText   // security review 2026-10-26 (M7): a caption is text, never markup

    color: Th.labelSectionFg
    font.bold: true
    font.pointSize: Kirigami.Theme.smallFont.pointSize * Th.textScale   // ENGINE.md §10: the theme's text size
    font.letterSpacing: 0.4
}
