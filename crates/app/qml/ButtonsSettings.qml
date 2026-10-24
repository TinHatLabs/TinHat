// ButtonsSettings.qml — Settings → Appearance → Buttons (11.0 buttons; the web's src/actions/ui/ButtonsSettings.tsx — the
// same rows, texts and numbers), as two cards of the Settings window (SETTINGS-WINDOW.md §5): every editable row of
// today's surfaces with "Edit…" (the row editor; a row you changed also offers "Reset"), your own button islands (Edit… /
// Remove) and "Add a button island" with where it goes (up to 8).
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the active theme (docs/MIGRATING-THEME.md)

ColumnLayout {
    id: bs
    property var app: null
    spacing: 24

    readonly property var store: bs.app && bs.app.layoutStore ? bs.app.layoutStore : null
    // the rows the list offers, in this order (the web's EDITABLE_ROWS)
    readonly property var rows: [
        { island: "header", row: "main", title: "Room header", hint: "Game reel, calendar, Memories, the call, call settings, room info" },
        { island: "composer", row: "start", title: "Message box — left", hint: "Attach" },
        { island: "composer", row: "end", title: "Message box — right", hint: "Emoji, GIF, soundboard, pinned messages, Send" },
        { island: "account", row: "main", title: "Your account card", hint: "Report a problem, tuck the nav away, the menu" },
        { island: "call", row: "bar", title: "Call controls", hint: "Mute, deafen, camera, share, soundboard … Leave" }
    ]
    property int placeIndex: 0
    property string errorText: ""
    property string removing: ""
    readonly property var islands: bs.store ? bs.store.buttonIslands : []

    // the row editor lives in the main window: Settings steps aside and comes back when it closes (Main.qml settingsHandOff)
    function edit(island, row, title) {
        if (bs.app && bs.app.settingsHandOff) bs.app.settingsHandOff("appearance", "rowEditor", JSON.stringify({ island: island, row: row, form: "desktop", title: title }))
        else if (bs.app && bs.app.actions) bs.app.actions.openRowEditor({ island: island, row: row, form: "desktop", title: title })
    }

    SettingsSection {
        title: "Buttons"
        hint: "Choose the buttons of the room header, the message box, your account card and the call controls — hide the ones you never use, reorder them, add others. You can also right-click any of these rows to edit it there."
        Repeater {
            model: bs.rows
            delegate: SettingsRow {
                id: line
                required property var modelData
                readonly property bool changed: !!bs.store && String(bs.store.rowJson(modelData.island, modelData.row, "desktop")).length > 0
                app: bs.app
                label: modelData.title
                chip: line.changed ? "Changed" : ""
                description: modelData.hint
                trailing: [
                    ThButton { visible: line.changed; app: bs.app; flat: true; text: "Reset"; onClicked: if (bs.store) bs.store.setRow(line.modelData.island, line.modelData.row, null) },
                    ThButton { app: bs.app; text: "Edit…"; onClicked: bs.edit(line.modelData.island, line.modelData.row, line.modelData.title) }
                ]
            }
        }
    }
    SettingsSection {
        title: "Your button islands"
        hint: "A row of buttons of your own, anywhere in the window — join a room's call, play a sound, set a status, switch theme… (up to 8)."
        Repeater {
            model: bs.islands
            delegate: SettingsRow {
                id: isl
                required property string modelData
                app: bs.app
                label: bs.store ? bs.store.islandName(isl.modelData) : isl.modelData
                trailing: [
                    ThButton { app: bs.app; flat: true; text: "Remove"; onClicked: { bs.removing = isl.modelData; removeAsk.open() } },
                    ThButton { app: bs.app; text: "Edit…"; onClicked: bs.edit(isl.modelData, "main", bs.store ? bs.store.islandName(isl.modelData) : "Button island") }
                ]
            }
        }
        SettingsRow {
            app: bs.app
            label: "Add a button island"
            description: bs.errorText.length > 0 ? bs.errorText : "Pick where it goes, then choose its buttons."
            descriptionColor: bs.errorText.length > 0 ? Th.settingsErrorFg : Th.settingsRowDescriptionFg
            trailing: [
                ThComboBox {
                    id: placeBox
                    Layout.preferredWidth: 240
                    textRole: "label"
                    model: bs.store ? bs.store.placements : []
                    currentIndex: bs.placeIndex
                    onActivated: (index) => bs.placeIndex = index
                    Accessible.name: "Where the new island goes"
                },
                ThButton {
                    app: bs.app
                    highlighted: true
                    icon.name: "list-add-symbolic"
                    text: "Add"
                    enabled: bs.islands.length < 8
                    onClicked: {
                        if (!bs.store) return
                        var pl = bs.store.placements[bs.placeIndex] || bs.store.placements[0]
                        var r = bs.store.addButtonIsland(pl.id, [])
                        if (!r.ok) {
                            bs.errorText = r.error === "too-many" ? "You have 8 button islands already." : "That place has no room for it — try another."
                            return
                        }
                        bs.errorText = ""
                        bs.edit(r.ref, "main", bs.store.islandName(r.ref))
                    }
                }
            ]
        }
    }
    ConfirmDialog {
        id: removeAsk
        app: bs.app
        dialogTitle: bs.removing.length > 0 && bs.store ? "Remove " + bs.store.islandName(bs.removing) + "?" : ""
        message: "Its buttons go with it."
        confirmText: "Remove"
        destructive: true
        onConfirmed: { if (bs.store && bs.removing.length > 0) bs.store.removeButtonIsland(bs.removing); bs.removing = "" }
        onCancelled: bs.removing = ""
    }
}
