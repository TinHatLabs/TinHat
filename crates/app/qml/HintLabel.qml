// HintLabel — dim wrapped caption under a control. Uses pointSize (NOT
// Math.round(smallFont.pixelSize), which is -1 on point-configured desktop fonts and
// silently produced full-size "small print" + QFont warnings — r135 audit).
import QtQuick
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the active theme (docs/MIGRATING-THEME.md)

Controls.Label {
    property var app

    wrapMode: Text.Wrap
    color: Th.labelHintFg
    font.pointSize: Kirigami.Theme.smallFont.pointSize * Th.textScale   // ENGINE.md §10: the theme's text size
}
