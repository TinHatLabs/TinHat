// ThemePickerHandle.qml — the colour popover's handle (THEMES-UI.md §13.9): Ø14, a 2 px settings.themeEditor.picker.ring
// inside a 1 px settings.themeEditor.picker.ring.outline (outside and inside the ring), filled with the colour it points
// at (the web's `.handle`).
import QtQuick
import dev.tinhat.app   // Th: settings.themeEditor.picker.*

Item {
    id: h
    property color fill: "transparent"
    width: 14; height: 14
    Rectangle { anchors.fill: parent; anchors.margins: -1; radius: 8; color: Th.settingsThemeEditorPickerRingOutline }
    Rectangle { anchors.fill: parent; radius: 7; color: Th.settingsThemeEditorPickerRing }
    Rectangle { anchors.fill: parent; anchors.margins: 2; radius: 5; color: Th.settingsThemeEditorPickerRingOutline }
    Rectangle { anchors.fill: parent; anchors.margins: 3; radius: 4; color: h.fill }
}
