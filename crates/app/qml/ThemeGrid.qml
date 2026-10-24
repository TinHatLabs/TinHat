// ThemeGrid.qml — a wrapping grid of theme cards + one trailing tile, for the short lists of Settings → Appearance
// (the favourites, My themes; themes step 1b, contract THEMES-UI.md §3.3–§3.4, §5.5): n = max(1, floor((W + 8) / 156))
// columns, cell width (W − 8·(n − 1)) / n, gaps 8, row pitch = card height + 8. Not virtualized (≤ 51 items); the
// 1,190-theme gallery page has its own virtualized list (ThemeGalleryPage.qml). Keyboard (the web's ThemeGrid): Tab
// enters at the first cell and leaves the grid; ←/→ move one cell, ↑/↓ one row (the last cell of a shorter last row),
// Home/End the first/last cell; Enter/Space applies.
import QtQuick
import dev.tinhat.app   // Th: the active theme (docs/MIGRATING-THEME.md)

Flow {
    id: grid
    property var app: null
    property var ctl: null               // ThemeController
    /// [{id, name, author, variant}] (galleryJson / stateJson.mine items)
    property var items: []
    property string selectedId: ""
    property bool mine: false            // My themes: the ⋯ menu on every card
    property string tileText: ""         // "" = no tile
    property string tileIcon: ""
    property int cardHeight: 108
    property int stateRev: 0             // re-read the cards when My themes change
    signal use(string id)
    signal rename(string id, string name)
    signal remove(string id, string name)
    signal editTheme(string id, string name)      // 11.0 (THEMES-UI §13.1): Edit… (mine) / New theme from this…
    signal shareTheme(string id, string name)     // §14.5: Share to a room…
    signal tileClicked()

    readonly property int columns: Math.max(1, Math.floor((width + 8) / (148 + 8)))
    readonly property real cellWidth: Math.floor((width - 8 * (columns - 1)) / columns)
    readonly property int cellCount: cards.count + (tile.visible ? 1 : 0)

    spacing: 8

    function focusCell(i) {
        if (grid.cellCount === 0) return
        i = Math.max(0, Math.min(grid.cellCount - 1, i))
        if (i < cards.count) { var c = cards.itemAt(i); if (c) c.takeFocus() }
        else tile.forceActiveFocus(Qt.TabFocusReason)
    }
    function move(i, dx, dy) {
        var n = grid.cellCount, cols = grid.columns
        if (dx <= -100000) { grid.focusCell(0); return }
        if (dx >= 100000) { grid.focusCell(n - 1); return }
        if (dx !== 0) { if (i + dx >= 0 && i + dx < n) grid.focusCell(i + dx); return }
        if (dy < 0) { if (i - cols >= 0) grid.focusCell(i - cols); return }
        if (Math.floor(i / cols) < Math.floor((n - 1) / cols)) grid.focusCell(Math.min(i + cols, n - 1))
    }

    Repeater {
        id: cards
        model: grid.items
        delegate: ThemeCard {
            required property var modelData
            required property int index
            app: grid.app
            width: grid.cellWidth
            height: grid.cardHeight
            themeId: modelData.id
            c: (grid.ctl && grid.stateRev >= 0) ? grid.ctl.card(modelData.id) : ({})
            selected: modelData.id === grid.selectedId
            mine: grid.mine
            cardFocusOnTab: index === 0
            onUseRequested: grid.use(modelData.id)
            onRenameRequested: grid.rename(modelData.id, themeName)
            onRemoveRequested: grid.remove(modelData.id, themeName)
            onEditRequested: grid.editTheme(modelData.id, themeName)
            onShareRequested: grid.shareTheme(modelData.id, themeName)
            onArrowKey: (dx, dy) => grid.move(index, dx, dy)
            Keys.onPressed: (e) => {
                if (e.key === Qt.Key_Home) { grid.move(index, -100000, 0); e.accepted = true }
                else if (e.key === Qt.Key_End) { grid.move(index, 100000, 0); e.accepted = true }
            }
        }
    }
    ThemeTile {
        id: tile
        visible: grid.tileText.length > 0
        app: grid.app
        width: grid.cellWidth
        height: grid.cardHeight
        text: grid.tileText
        iconName: grid.tileIcon
        activeFocusOnTab: cards.count === 0
        onClicked: grid.tileClicked()
        onArrowKey: (dx, dy) => grid.move(cards.count, dx, dy)
        Keys.onPressed: (e) => {
            if (e.key === Qt.Key_Home) { grid.move(cards.count, -100000, 0); e.accepted = true }
            else if (e.key === Qt.Key_End) { grid.move(cards.count, 100000, 0); e.accepted = true }
        }
    }
}
