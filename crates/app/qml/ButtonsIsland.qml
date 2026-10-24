// ButtonsIsland.qml — one of your own button islands (`buttons:<n>`, 11.0 buttons; DESIGN.md "Your own button islands:
// drop a buttons island anywhere and fill it, Stream Deck style"; the web's src/actions/ui/ButtonsIsland.tsx — the same
// parts, the same numbers): its row (opts.<ref>.rows.main, row kind buttons.main) drawn with ButtonRow — wrapping when
// the island is narrow, a toggle reads on / off (checked) — and its right-click menu (Edit buttons…, Remove this
// island… with a confirm). An empty island says so and offers the editor. It reports its natural size to the dock host:
// `oneLineWidth` (its buttons on one line + 4 px around — Main.qml passes it as extra.w: a floating island is as wide
// as its buttons) and its height at the width it got (implicitHeight).
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the active theme (docs/MIGRATING-THEME.md)

Item {
    id: bi
    required property var app
    property string islandRef: ""

    readonly property var store: bi.app && bi.app.layoutStore ? bi.app.layoutStore : null
    readonly property var storedValue: bi.store ? bi.store.rowValue(bi.islandRef, "main") : null
    readonly property string title: bi.store ? bi.store.islandName(bi.islandRef) : "Button island"
    readonly property int shown: row.shownCount
    readonly property real oneLineWidth: (bi.shown > 0 ? row.oneLineWidth : emptyLine.implicitWidth) + 8

    implicitWidth: bi.oneLineWidth
    implicitHeight: (bi.shown > 0 ? row.implicitHeight : emptyLine.implicitHeight) + 8

    function edit() { if (bi.app && bi.app.actions) bi.app.actions.openRowEditor({ island: bi.islandRef, row: "main", form: "desktop", title: bi.title }) }

    ButtonRow {
        id: row
        x: 4
        y: 4
        width: Math.max(0, bi.width - 8)
        visible: bi.shown > 0
        app: bi.app
        rowKind: "buttons.main"
        value: bi.storedValue
        wrap: true
    }
    // the empty state: one line — a hint and "Add buttons…"
    RowLayout {
        id: emptyLine
        x: 8
        y: 4
        visible: bi.shown === 0
        spacing: 8
        height: Math.max(40, implicitHeight)
        Controls.Label {
            Layout.alignment: Qt.AlignVCenter
            text: "No buttons yet"
            color: Th.labelHintFg
            font.pointSize: Kirigami.Theme.smallFont.pointSize
        }
        ThButton {
            Layout.alignment: Qt.AlignVCenter
            app: bi.app
            flat: true
            text: "Add buttons…"
            icon.name: "list-add-symbolic"
            onClicked: bi.edit()
        }
    }

    // right-click anywhere on the island: Edit buttons… / Remove this island…
    TapHandler {
        acceptedButtons: Qt.RightButton
        onTapped: islandMenu.popup()
    }
    Controls.Menu {
        id: islandMenu
        Controls.MenuItem { text: "Edit buttons…"; icon.name: "document-edit-symbolic"; onTriggered: bi.edit() }
        Controls.MenuItem { text: "Remove this island…"; icon.name: "edit-delete-symbolic"; onTriggered: removeAsk.open() }
    }
    ConfirmDialog {
        id: removeAsk
        app: bi.app
        dialogTitle: "Remove " + bi.title + "?"
        message: bi.shown > 0
                 ? "Its " + bi.shown + " button" + (bi.shown === 1 ? "" : "s") + " go with it. You can make a new one in Settings → Appearance → Buttons."
                 : "You can make a new one in Settings → Appearance → Buttons."
        confirmText: "Remove"
        destructive: true
        onConfirmed: if (bi.store) bi.store.removeButtonIsland(bi.islandRef)
    }
}
