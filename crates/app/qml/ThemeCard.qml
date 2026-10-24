// ThemeCard.qml — a theme's card in Settings → Appearance and the theme gallery (themes step 1b, contract THEMES-UI.md
// §5.1–§5.2; the web's ThemeCard.tsx + ThemeCard.module.css are the reference build): a live miniature of Tin Hat
// painted from 15 roles of the CARD's theme (`themeCtl.card(id)` — never the active theme's, never a full render), a
// 26 px caption with the name and the selected check, and a ⋯ button (11.0, THEMES-UI §13.1): My themes — Use · Edit… ·
// Rename… · Share to a room… · Remove; every other card — Use · New theme from this….
// Geometry (px at 1×): the card's 1 px border stays put; the selected / keyboard ring is a 2 px stroke drawn OVER the
// card's edge (no layout shift); the mini box is the card inside its border minus the caption, and every mini position
// is relative to it. The ⋯ button sits beside the card button in the cell (it does not scale with a hovered card).
// Nothing loops: a hover eases the scale once (app.dur; 0 with Reduce motion), everything else is static.
import QtQuick
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the active theme (docs/MIGRATING-THEME.md)

Item {
    id: cell
    property var app: null
    property string themeId: ""
    /// the card's colours + name/author/variant/adjusted/ok/error — themeCtl.card(id), or cardForDoc(json) for a preview
    property var c: ({})
    /// the caption's text when it is not the theme's own name (the import preview shows the Name field's text)
    property string captionText: ""
    property bool selected: false
    property bool mine: false           // one of My themes: the ⋯ menu's Edit… / Rename… / Share to a room… / Remove
    property bool interactive: true     // the import preview is not clickable (and has no ⋯)
    /// the gallery's grids move the keyboard focus themselves (arrows), so their cards take the focus only when told to
    /// (not `activeFocusOnTab`: Item's own is FINAL — Qt 6.11 refuses a component that redeclares it)
    property alias cardFocusOnTab: btn.activeFocusOnTab
    property alias cardFocusPolicy: btn.focusPolicy   // (not `focusPolicy`: Item's own since Qt 6.7)
    readonly property alias cardHovered: btn.hovered
    readonly property alias cardFocused: btn.activeFocus
    signal useRequested()
    signal renameRequested()
    signal removeRequested()
    signal editRequested()              // 11.0 (THEMES-UI §13.1): Edit… (mine) / New theme from this… (any other)
    signal shareRequested()             // §14.5: Share to a room… (mine)
    signal arrowKey(int dx, int dy)      // the gallery's grid navigation
    signal escapePressed()

    readonly property bool usable: c !== undefined && c !== null && c.ok === true
    /// the miniature's colours: the card's own, or transparent while it has none (the import preview before a file is
    /// read) — so no colour binding ever reads a missing key
    readonly property var k: usable ? c : ({ accent: "transparent", accentBright: "transparent", border: "transparent", island: "transparent", name1: "transparent", name2: "transparent", name3: "transparent", name4: "transparent", name5: "transparent", onAccent: "transparent", raised: "transparent", selected: "transparent", text: "transparent", textSecondary: "transparent", window: "transparent" })
    readonly property string themeName: (c && c.name) ? c.name : themeId
    readonly property string shownName: captionText.length > 0 ? captionText : themeName
    readonly property string tipText: usable
        ? themeName + ((c && c.author && c.author.length > 0) ? " · by " + c.author : "")
          + " · " + ((c && c.variant === "light") ? "Light theme" : "Dark theme")
        : themeName + " can't be used by this version of Tin Hat (" + ((c && c.error) ? String(c.error).split(":")[0] : "refused") + ")"

    implicitWidth: 148
    implicitHeight: 108

    function takeFocus() { btn.forceActiveFocus(Qt.TabFocusReason) }
    function activate() { if (cell.interactive && cell.usable && !cell.selected) cell.useRequested() }

    HoverHandler { id: cellHover }

    Controls.AbstractButton {
        id: btn
        anchors.fill: parent
        hoverEnabled: cell.interactive
        focusPolicy: cell.interactive ? Qt.StrongFocus : Qt.NoFocus
        Accessible.role: Accessible.RadioButton
        Accessible.name: cell.themeName + " theme"
        Accessible.checked: cell.selected
        Controls.ToolTip.text: cell.tipText
        Controls.ToolTip.visible: btn.hovered && cell.interactive
        Controls.ToolTip.delay: 600

        onClicked: cell.activate()
        Keys.onSpacePressed: (e) => { cell.activate(); e.accepted = true }
        Keys.onReturnPressed: (e) => { cell.activate(); e.accepted = true }
        Keys.onEnterPressed: (e) => { cell.activate(); e.accepted = true }
        Keys.onLeftPressed: (e) => { cell.arrowKey(-1, 0); e.accepted = true }
        Keys.onRightPressed: (e) => { cell.arrowKey(1, 0); e.accepted = true }
        Keys.onUpPressed: (e) => { cell.arrowKey(0, -1); e.accepted = true }
        Keys.onDownPressed: (e) => { cell.arrowKey(0, 1); e.accepted = true }
        Keys.onEscapePressed: (e) => { cell.escapePressed(); e.accepted = true }

        // hover (a usable card that is not the selected one): the border takes the card's accent, the card lifts 3 %
        readonly property bool lift: btn.hovered && cell.interactive && !cell.selected && cell.usable
        scale: lift ? 1.03 : 1.0
        Behavior on scale { NumberAnimation { duration: cell.app ? cell.app.dur : 0; easing.type: Easing.OutCubic } }

        background: Rectangle {
            radius: 8
            color: cell.usable ? cell.k.window : "transparent"
            border.width: 1
            border.color: btn.lift ? cell.k.accent : Th.settingsThemeCardBorder
            Behavior on border.color { ColorAnimation { duration: cell.app ? cell.app.dur : 0 } }
        }

        contentItem: Item {
            Item {
                id: frame
                anchors.fill: parent
                anchors.margins: 1          // inside the border
                clip: true

                // ---- the miniature: w × h = the frame minus the caption ---------------------------------------------
                Item {
                    id: mini
                    width: frame.width
                    height: frame.height - 26
                    visible: cell.usable
                    // rail: three dots Ø7 at x 6, y 6 / 17 / 28 (accent, name 1, name 2)
                    Repeater {
                        model: 3
                        Rectangle {
                            required property int index
                            x: 6; y: 6 + 11 * index
                            width: 7; height: 7; radius: 3.5
                            color: index === 0 ? cell.k.accent : (index === 1 ? cell.k.name1 : cell.k.name2)
                        }
                    }
                    // rooms island: x 18, y 5, width (w − 23) × 0.32, height h − 10
                    Rectangle {
                        id: rooms
                        x: 18; y: 5
                        width: Math.round((mini.width - 23) * 0.32)
                        height: mini.height - 10
                        radius: 4
                        color: cell.k.island
                        // row 2's band: x +2, width island − 4, height 9, 2 above its row
                        Rectangle { x: 2; y: 5 + 11 - 2; width: rooms.width - 4; height: 9; radius: 2; color: cell.k.selected }
                        Repeater {
                            model: 3
                            Item {
                                required property int index
                                y: 5 + 11 * index
                                Rectangle {
                                    x: 4; width: 5; height: 5; radius: 2.5
                                    color: index === 0 ? cell.k.name3 : (index === 1 ? cell.k.name4 : cell.k.name5)
                                }
                                Rectangle {
                                    x: 12; y: 1; height: 3; radius: 1.5
                                    width: Math.round((rooms.width - 16) * (index === 0 ? 0.60 : (index === 1 ? 0.45 : 0.52)))
                                    color: cell.k.textSecondary
                                }
                            }
                        }
                    }
                    // main island: from the rooms island + 4 to w − 5; header bar, their bubble, your bubble, composer
                    // (widths are fractions of the island, positions from its edges — the web's CSS)
                    Rectangle {
                        id: mainIsland
                        x: rooms.x + rooms.width + 4; y: 5
                        width: mini.width - 5 - x
                        height: mini.height - 10
                        radius: 4
                        color: cell.k.island
                        Rectangle { x: 5; y: 5; width: Math.round(mainIsland.width * 0.38); height: 3; radius: 1.5; color: cell.k.text }
                        Rectangle { x: 5; y: 13; width: Math.round(mainIsland.width * 0.46); height: 9; radius: 3; color: cell.k.raised }
                        Rectangle {
                            width: Math.round(mainIsland.width * 0.40); height: 9; radius: 3
                            x: mainIsland.width - 5 - width; y: 25
                            color: cell.k.accent
                        }
                        Rectangle {
                            x: 5; y: mainIsland.height - 14; width: mainIsland.width - 10; height: 9; radius: 3
                            color: cell.k.raised
                            border.width: 1; border.color: cell.k.border
                        }
                    }
                }
                // a theme the engine refuses (a damaged My theme): the warning glyph on the tile-hover fill
                Rectangle {
                    visible: !cell.usable
                    width: mini.width; height: mini.height
                    color: Th.settingsThemeTileBgHover
                    Kirigami.Icon {
                        anchors.centerIn: parent
                        width: 22; height: 22
                        source: "dialog-warning"
                        color: Th.settingsThemeNoteIcon
                        isMask: true
                    }
                }

                // ---- the caption: the bottom 26 (its 1 px top line included), the island fill, padding 0 8 ----------
                Rectangle {
                    y: frame.height - 26
                    width: frame.width; height: 26
                    color: cell.usable ? cell.k.island : "transparent"
                    Rectangle { width: parent.width; height: 1; color: cell.usable ? cell.k.border : Th.settingsThemeCardBorder }
                    Text {
                        x: 8; y: 1
                        width: parent.width - 16 - ((cell.selected && cell.usable) ? 14 + 6 : 0)
                        height: 25
                        verticalAlignment: Text.AlignVCenter
                        elide: Text.ElideRight
                        text: cell.shownName
                        color: cell.usable ? cell.k.text : Th.settingsThemeMetaFg
                        font.pointSize: Kirigami.Theme.defaultFont.pointSize
                        font.bold: cell.selected
                    }
                    // the selected check: Ø14 at the caption's right padding, the card's accent + its onAccent glyph
                    Rectangle {
                        visible: cell.selected && cell.usable
                        width: 14; height: 14; radius: 7
                        x: parent.width - 8 - 14
                        y: 1 + Math.round((25 - 14) / 2)
                        color: cell.k.accent
                        Kirigami.Icon {
                            anchors.centerIn: parent
                            width: 10; height: 10
                            source: "checkmark-symbolic"
                            color: cell.k.onAccent
                            isMask: true
                        }
                    }
                }
            }
            // the selected / keyboard ring: 2 px over the card's edge (the 1 px border stays: no layout shift)
            Rectangle {
                anchors.fill: parent
                radius: 8
                color: "transparent"
                visible: btn.visualFocus || (cell.selected && cell.usable)
                border.width: 2
                border.color: btn.visualFocus ? Th.settingsThemeCardBorderFocus : cell.k.accentBright
            }
        }
    }

    // ---- ⋯ (every card since 11.0): Ø22, 4 from the top-right edges; while the cell is hovered / has the focus / its
    // menu is open
    Controls.AbstractButton {
        id: more
        // the menu's `visible`, not `opened` (false during its enter transition, while the pointer is already on the
        // menu instead of the cell)
        readonly property bool wanted: cellHover.hovered || btn.activeFocus || more.activeFocus || menu.visible
        visible: cell.interactive && opacity > 0
        opacity: wanted ? 1 : 0
        Behavior on opacity { NumberAnimation { duration: cell.app ? cell.app.dur : 0 } }
        x: cell.width - 4 - 22
        y: 4
        z: 1
        width: 22; height: 22
        hoverEnabled: true
        focusPolicy: Qt.StrongFocus
        Accessible.role: Accessible.Button
        Accessible.name: "More for " + cell.themeName
        Controls.ToolTip.text: "More"
        Controls.ToolTip.visible: more.hovered
        Controls.ToolTip.delay: 600
        onClicked: menu.popup(more, 0, more.height)
        background: Rectangle {
            radius: 11
            color: Th.settingsThemeCardMenuBg
            border.width: more.visualFocus ? 2 : 0
            border.color: Th.settingsThemeCardBorderFocus
        }
        contentItem: Item {
            Kirigami.Icon {
                anchors.centerIn: parent
                width: 16; height: 16
                source: "view-more-symbolic"
                color: Th.settingsThemeCardMenuIcon
                isMask: true
            }
        }
        Controls.Menu {
            id: menu
            // Kirigami's menu layer (z 400): the cards live in the Settings sheet, a Kirigami OverlaySheet at z 200 — a
            // plain Menu (z 0) opened BEHIND it and the ⋯ seemed to do nothing (10.8: My themes' ⋯ never showed a menu)
            Kirigami.OverlayZStacking.layer: Kirigami.OverlayZStacking.Menu
            z: Kirigami.OverlayZStacking.z
            Controls.MenuItem {
                text: "Use"
                icon.name: "dialog-ok-apply-symbolic"
                enabled: cell.usable && !cell.selected
                onTriggered: cell.useRequested()
            }
            // My themes: Edit… · Rename… · Share to a room… · Remove (THEMES-UI §13.1, §14.5)
            Controls.MenuItem { visible: cell.mine; height: visible ? implicitHeight : 0; text: "Edit…"; icon.name: "document-edit-symbolic"; enabled: cell.usable; onTriggered: cell.editRequested() }
            Controls.MenuItem { visible: cell.mine; height: visible ? implicitHeight : 0; text: "Rename…"; icon.name: "document-edit-symbolic"; onTriggered: cell.renameRequested() }
            Controls.MenuItem { visible: cell.mine; height: visible ? implicitHeight : 0; text: "Share to a room…"; icon.name: "document-send-symbolic"; enabled: cell.usable; onTriggered: cell.shareRequested() }
            Controls.MenuSeparator { visible: cell.mine; height: visible ? implicitHeight : 0 }
            // destructive (the web's `danger` row): text + glyph in menu.item.danger.fg, in both styles
            Controls.MenuItem {
                visible: cell.mine
                height: visible ? implicitHeight : 0
                text: "Remove"
                icon.name: "edit-delete-symbolic"
                icon.color: Th.menuItemDangerFg
                palette.windowText: Th.menuItemDangerFg
                Kirigami.Theme.textColor: Th.menuItemDangerFg
                onTriggered: cell.removeRequested()
            }
            // every other card: New theme from this…
            Controls.MenuItem { visible: !cell.mine; height: visible ? implicitHeight : 0; text: "New theme from this…"; icon.name: "list-add-symbolic"; enabled: cell.usable; onTriggered: cell.editRequested() }
        }
    }
}
