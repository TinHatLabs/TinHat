// ThemeSwatch.qml — a ☰ Theme row's swatch (THEMES-UI.md §12; the web's ThemeSwatch in features/nav/menus.tsx): 30 × 16,
// radius 4, the ROW theme's window colour inside a 1 px border in its subtle border colour, three Ø6 dots (its accent,
// name 1, name 2) at x 5 / 12 / 19, y 5. `colours` = [fill, border, dot 1, dot 2, dot 3] as Qt colour strings — the row
// theme's own roles (theme_store.rs recent_rows), never the active theme's; nothing is drawn without them.
import QtQuick

Rectangle {
    id: sw
    property var colours: null
    readonly property bool ok: !!sw.colours && sw.colours.length === 5
    width: 30
    height: 16
    radius: 4
    visible: sw.ok
    color: sw.ok ? sw.colours[0] : "transparent"
    border.width: 1
    border.color: sw.ok ? sw.colours[1] : "transparent"
    Repeater {
        model: 3
        delegate: Rectangle {
            required property int index
            x: 5 + 7 * index
            y: 5
            width: 6
            height: 6
            radius: 3
            color: sw.ok ? sw.colours[2 + index] : "transparent"
        }
    }
}
