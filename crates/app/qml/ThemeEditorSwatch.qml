// ThemeEditorSwatch.qml — a colour swatch of the theme editor (THEMES-UI.md §13.7–§13.9; the web's `.swatch`): the colour
// over the 6 px checkerboard (translucent colours show it), a ring inside its edge (1 px settings.themeEditor.swatch.border;
// the accent chip in use: 2 px settings.fg). `colour` is a QML colour (ThemeEditorLib.qmlColour of a canonical one).
import QtQuick
import dev.tinhat.app   // Th: settings.themeEditor.swatch.*

Item {
    id: sw
    property color colour: "transparent"
    property real radius: 5
    property int ringWidth: 1
    property color ringColour: Th.settingsThemeEditorSwatchBorder

    implicitWidth: 24
    implicitHeight: 24

    ThemeChecker { anchors.fill: parent; radius: sw.radius }
    Rectangle { anchors.fill: parent; radius: sw.radius; color: sw.colour }
    Rectangle {
        anchors.fill: parent
        radius: sw.radius
        color: "transparent"
        border.width: sw.ringWidth
        border.color: sw.ringColour
    }
}
