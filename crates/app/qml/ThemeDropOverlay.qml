// ThemeDropOverlay.qml — "Drop the theme file to import it" over a drop target while a file is dragged over it
// (themes step 1b, contract THEMES-UI.md §5.4): the drop fill, a 2 px dashed border, radius 8, a centred H4 label.
import QtQuick
import QtQuick.Shapes
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the active theme (docs/MIGRATING-THEME.md)

Item {
    id: ov
    Shape {
        anchors.fill: parent
        ShapePath {
            strokeColor: Th.settingsThemeImportDropBorderActive
            strokeWidth: 2
            strokeStyle: ShapePath.DashLine
            dashPattern: [3, 2]
            fillColor: Th.settingsThemeImportDropBgActive
            PathRectangle { x: 1; y: 1; width: ov.width - 2; height: ov.height - 2; radius: 8 }
        }
    }
    Kirigami.Heading {
        anchors.centerIn: parent
        level: 4
        text: "Drop the theme file to import it"
        color: Th.settingsThemeNameFg
    }
}
