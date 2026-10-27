// MenuSectionLabel — dim group header inside a Menu (menus accept plain controls as
// children; a disabled MenuItem reads as a broken row, this reads as a heading).
import QtQuick
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the active theme (docs/MIGRATING-THEME.md)

Controls.Label {
    property var app
    textFormat: Text.PlainText   // security review 2026-10-26 (M7): a caption is text, never markup
    leftPadding: Kirigami.Units.largeSpacing
    topPadding: Kirigami.Units.smallSpacing
    bottomPadding: 2
    color: Th.menuSectionFg
    font.bold: true
    font.pointSize: Kirigami.Theme.smallFont.pointSize * Th.textScale   // ENGINE.md §10: the theme's text size
    font.letterSpacing: 0.6
}
