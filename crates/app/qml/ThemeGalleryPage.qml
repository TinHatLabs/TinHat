// ThemeGalleryPage.qml — the theme gallery ("Themes"; themes step 1b, contract THEMES-UI.md §4; the web's
// ThemeGallery.tsx): replaces the Appearance pane's content while open (the Settings rail stays). A toolbar (back,
// "Themes", Import; the notices; search, All / Dark / Light, the summary + Undo), then one scroller with the sections
// Favourites, My themes and All themes (1,190, by name) — virtualized: the list's delegates are ROWS of cards, so only
// the rows in view ± 2 exist. Search and filter run in Rust (themeCtl.galleryJson). Keyboard: Tab enters the grid, the
// arrows move between cards, Enter / Space applies, Esc clears the search (else goes back). A file dragged over the
// page shows the drop overlay; dropping it opens the import dialog with it.
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the active theme (docs/MIGRATING-THEME.md)

FocusScope {
    id: page
    property var app: null
    property var core: null
    property var ctl: null
    property var st: ({})
    property int libraryCount: 0
    signal back()
    signal openImport(string fileUrl)
    signal renameMine(string id, string name)
    signal removeMine(string id, string name)
    signal editTheme(string id, string name, bool mine)   // 11.0 (THEMES-UI §13.1)
    signal shareTheme(string id, string name)             // §14.5
    signal customize()                                    // the summary's Customize…

    property string query: ""
    property string filter: "all"
    readonly property int cardHeight: 108
    readonly property int pitch: cardHeight + 8
    // the sections, recomputed when the query, the filter or My themes change (not `data`: Item's default property)
    readonly property var galleryData: {
        var _rev = page.ctl ? page.ctl.stateRev : 0
        return page.ctl ? JSON.parse(page.ctl.galleryJson(page.query, page.filter)) : ({ favourites: [], mine: [], all: [] })
    }
    readonly property bool searching: page.query.trim().length > 0 || page.filter !== "all"
    readonly property int columns: Math.max(1, Math.floor((list.width + 8) / (148 + 8)))
    readonly property real cellWidth: Math.floor((list.width - 8 * (columns - 1)) / columns)
    // THEMES-UI §4: a section without a (matching) theme is left out — My themes too when there are none
    readonly property var sections: [
        { name: "Favourites", items: page.galleryData.favourites, mine: false, show: page.galleryData.favourites.length > 0 },
        { name: "My themes", items: page.galleryData.mine, mine: true, show: page.galleryData.mine.length > 0 },
        { name: "All themes", items: page.galleryData.all, mine: false, show: page.galleryData.all.length > 0 }
    ]
    // flattened rows: {kind: "head", text, first} | {kind: "cards", items: [...], first: <global card index>, mine}
    readonly property var rows: {
        var out = [], g = 0, n = 0
        for (var i = 0; i < page.sections.length; i++) {
            var sec = page.sections[i]
            if (!sec.show) { g += sec.items.length; continue }
            out.push({ kind: "head", text: sec.name + " · " + sec.items.length.toLocaleString(Qt.locale("en_US"), "f", 0), top: n === 0 })
            for (var k = 0; k < sec.items.length; k += page.columns)
                out.push({ kind: "cards", items: sec.items.slice(k, k + page.columns), first: g + k, mine: sec.mine })
            g += sec.items.length
            n++
        }
        return out
    }
    readonly property int cardCount: page.galleryData.favourites.length + page.galleryData.mine.length + page.galleryData.all.length
    property int focusIndex: -1          // the keyboard's card (global index over the sections), -1 = none yet

    function pick(id) {
        var err = page.core ? page.core.chooseTheme(id, true) : ""
        if (err && err.length > 0 && page.app) page.app.settingsToast(err, 6000)
    }
    function clearSearch() { searchField.text = ""; page.query = ""; page.filter = "all" }
    function setQuery(q) { searchField.text = q }
    /// THEMES-UI §12: opened fresh by the ☰ Theme menu's "More themes…" (the web's gallery page mounts anew every time):
    /// no search, All, no keyboard card, the list at its top
    function reset() { page.clearSearch(); page.focusIndex = -1; list.positionViewAtBeginning() }
    // the row of a global card index
    function rowOf(g) {
        for (var r = 0; r < page.rows.length; r++) {
            var row = page.rows[r]
            if (row.kind === "cards" && g >= row.first && g < row.first + row.items.length) return r
        }
        return -1
    }
    // (section, offset) of a global index, over the SHOWN sections
    function locate(g) {
        var base = 0
        for (var i = 0; i < page.sections.length; i++) {
            var n = page.sections[i].items.length
            if (g < base + n) return { section: i, offset: g - base, base: base, count: n }
            base += n
        }
        return null
    }
    function focusCard(g) {
        if (page.cardCount === 0) return
        g = Math.max(0, Math.min(page.cardCount - 1, g))
        page.focusIndex = g
        var r = page.rowOf(g)
        if (r < 0) return
        list.positionViewAtIndex(r, ListView.Contain)
        Qt.callLater(function () {
            var item = list.itemAtIndex(r)
            if (item && item.focusCard) item.focusCard(g)
        })
    }
    function moveFocus(dx, dy) {
        var g = page.focusIndex < 0 ? 0 : page.focusIndex
        if (dx !== 0) { page.focusCard(g + dx); return }
        var at = page.locate(g)
        if (!at) return
        var col = at.offset % page.columns
        if (dy > 0) {
            if (at.offset + page.columns < at.count) page.focusCard(g + page.columns)
            else if (Math.floor(at.offset / page.columns) < Math.floor((at.count - 1) / page.columns)) page.focusCard(at.base + at.count - 1)
            else {
                // the next non-empty section, same column
                var base = at.base + at.count
                for (var i = at.section + 1; i < page.sections.length; i++) {
                    var n = page.sections[i].items.length
                    if (n > 0 && page.sections[i].show) { page.focusCard(base + Math.min(col, n - 1)); return }
                    base += n
                }
            }
        } else {
            if (at.offset - page.columns >= 0) page.focusCard(g - page.columns)
            else {
                var b = at.base
                for (var j = at.section - 1; j >= 0; j--) {
                    var m = page.sections[j].items.length
                    b -= m
                    if (m > 0 && page.sections[j].show) {
                        var lastRowStart = Math.floor((m - 1) / page.columns) * page.columns
                        page.focusCard(b + Math.min(lastRowStart + col, m - 1))
                        return
                    }
                }
            }
        }
    }

    Keys.onEscapePressed: (e) => { page.back(); e.accepted = true }

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        // ---- the toolbar (sticky: the body scrolls under it) ------------------------------------------------------
        Rectangle {
            Layout.fillWidth: true
            implicitHeight: head.implicitHeight + 8 + 1
            color: Th.settingsThemeGalleryToolbarBg
            ColumnLayout {
                id: head
                anchors { left: parent.left; right: parent.right; top: parent.top }
                spacing: 8
                RowLayout {
                    Layout.fillWidth: true
                    spacing: 8
                    Controls.ToolButton {
                        icon.name: "go-previous-symbolic"
                        text: "Appearance"
                        display: Controls.AbstractButton.TextBesideIcon
                        onClicked: page.back()
                    }
                    Kirigami.Heading { level: 2; text: "Themes"; color: Th.settingsThemeNameFg }
                    Item { Layout.fillWidth: true }
                    ThButton {
                        app: page.app
                        visible: page.st.themesReadOnly !== true
                        text: "Import a theme…"
                        icon.name: "document-open-symbolic"
                        onClicked: page.openImport("")
                    }
                }
                ThemeNotices { Layout.fillWidth: true; st: page.st; ctl: page.ctl }
                RowLayout {
                    Layout.fillWidth: true
                    spacing: 8
                    Kirigami.SearchField {
                        id: searchField
                        Layout.preferredWidth: 280
                        placeholderText: "Search " + page.libraryCount.toLocaleString(Qt.locale("en_US"), "f", 0) + " themes"
                        Accessible.name: "Search themes"
                        onTextChanged: page.query = text
                        Keys.onEscapePressed: (e) => {
                            if (text.length > 0) text = ""
                            else page.back()
                            e.accepted = true
                        }
                        Keys.onDownPressed: (e) => { page.focusCard(page.focusIndex < 0 ? 0 : page.focusIndex); e.accepted = true }
                    }
                    Row {
                        spacing: 0
                        Accessible.role: Accessible.RadioButton
                        Repeater {
                            model: [["all", "All"], ["dark", "Dark"], ["light", "Light"]]
                            delegate: Controls.ToolButton {
                                required property var modelData
                                text: modelData[1]
                                display: Controls.AbstractButton.TextOnly
                                checkable: true
                                checked: page.filter === modelData[0]
                                Accessible.role: Accessible.RadioButton
                                onClicked: { page.filter = modelData[0]; checked = Qt.binding(() => page.filter === modelData[0]) }
                            }
                        }
                    }
                    ThemeSummary {
                        Layout.fillWidth: true
                        app: page.app
                        ctl: page.ctl
                        st: page.st
                        compact: true
                        undoVisible: page.st.canUndo === true
                        customizeVisible: true
                        onCustomize: page.customize()
                        onUndo: {
                            var err = page.ctl.undoTheme()
                            if (err.length > 0 && page.app) page.app.settingsToast(err, 6000)
                        }
                    }
                }
            }
            Rectangle { anchors { left: parent.left; right: parent.right; bottom: parent.bottom } height: 1; color: Th.settingsDivider }
        }

        // ---- the body ------------------------------------------------------------------------------------------
        Item {
            Layout.fillWidth: true
            Layout.fillHeight: true

            ListView {
                id: list
                anchors.fill: parent
                clip: true
                model: page.rows
                spacing: 0
                topMargin: 8
                bottomMargin: 16
                cacheBuffer: 2 * page.pitch
                boundsBehavior: Flickable.StopAtBounds
                activeFocusOnTab: true
                keyNavigationEnabled: false
                onActiveFocusChanged: if (activeFocus) page.focusCard(page.focusIndex < 0 ? 0 : page.focusIndex)
                Controls.ScrollBar.vertical: FoxScrollBar { app: page.app }
                delegate: Item {
                    id: rowItem
                    required property var modelData
                    required property int index
                    width: list.width
                    // a section: (16 above it unless first) + its label + 8; a card row: the card + 8
                    height: modelData.kind === "head" ? (modelData.top ? 0 : 16) + headLabel.implicitHeight + 8 : page.pitch
                    function focusCard(g) {
                        for (var i = 0; i < cards.count; i++) {
                            var c = cards.itemAt(i)
                            if (c && c.globalIndex === g) { c.takeFocus(); return }
                        }
                    }
                    Controls.Label {
                        id: headLabel
                        visible: rowItem.modelData.kind === "head"
                        y: rowItem.modelData.top ? 0 : 16
                        text: rowItem.modelData.text || ""
                        color: Th.settingsThemeMetaFg
                    }
                    Row {
                        visible: rowItem.modelData.kind === "cards"
                        spacing: 8
                        Repeater {
                            id: cards
                            model: rowItem.modelData.kind === "cards" ? rowItem.modelData.items : []
                            delegate: ThemeCard {
                                required property var modelData
                                required property int index
                                readonly property int globalIndex: rowItem.modelData.first + index
                                app: page.app
                                width: page.cellWidth
                                height: page.cardHeight
                                themeId: modelData.id
                                c: (page.ctl && page.ctl.stateRev >= 0) ? page.ctl.card(modelData.id) : ({})
                                selected: page.ctl !== null && modelData.id === page.ctl.effectiveTheme
                                mine: rowItem.modelData.mine === true
                                cardFocusOnTab: false
                                cardFocusPolicy: Qt.ClickFocus
                                onCardFocusedChanged: if (cardFocused) page.focusIndex = globalIndex
                                onUseRequested: page.pick(modelData.id)
                                onRenameRequested: page.renameMine(modelData.id, themeName)
                                onRemoveRequested: page.removeMine(modelData.id, themeName)
                                onEditRequested: page.editTheme(modelData.id, themeName, rowItem.modelData.mine === true)
                                onShareRequested: page.shareTheme(modelData.id, themeName)
                                onArrowKey: (dx, dy) => page.moveFocus(dx, dy)
                                onEscapePressed: page.back()
                            }
                        }
                    }
                }
            }

            // no match (THEMES-UI §4): the house empty state + Clear search
            Item {
                anchors.fill: parent
                visible: page.cardCount === 0
                EmptyState {
                    id: noMatch
                    app: page.app
                    anchors { left: parent.left; right: parent.right; top: parent.top; topMargin: 48 }
                    height: Kirigami.Units.iconSizes.enormous + Kirigami.Units.largeSpacing + 40
                    iconName: "search"
                    title: page.query.trim().length > 0 ? "No themes match “" + page.query.trim() + "”."
                                                        : (page.filter === "light" ? "No light themes." : "No dark themes.")
                }
                ThButton {
                    app: page.app
                    anchors { horizontalCenter: parent.horizontalCenter; top: noMatch.bottom; topMargin: 12 }
                    text: "Clear search"
                    onClicked: page.clearSearch()
                }
            }
        }
    }

    // a file dragged over the page → the drop overlay; dropped → the import dialog with it (THEMES-UI §4, §5.4)
    DropArea {
        id: drop
        anchors.fill: parent
        keys: ["text/uri-list"]
        onDropped: (d) => {
            if (d.hasUrls && d.urls.length > 0) { page.openImport(String(d.urls[0])); d.acceptProposedAction() }
        }
    }
    ThemeDropOverlay { anchors.fill: parent; visible: drop.containsDrag }
}
