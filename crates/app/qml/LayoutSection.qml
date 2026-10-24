// LayoutSection.qml — Settings → Appearance → Layout, the layout rows (EDITMODE-UI.md §9, §13; the web's LayoutSection.tsx,
// the same controls and texts) as rows of the Layout card (SETTINGS-WINDOW.md §5): which layout this device uses, Edit
// layout, Reset layout (this device first), New / Rename / Delete, Restore previous version. Every change here is ONE
// save of the synced document (LayoutStore → layout_store.rs) — never per click of anything else.
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th

SettingsRowGroup {
    id: sec
    property var app: null                 // Main.qml root
    readonly property var store: sec.app ? sec.app.layoutStore : null
    readonly property var view: sec.store ? sec.store.view : ({ layouts: [] })
    readonly property bool ro: sec.view.readOnly === true
    readonly property var list: (sec.view.layouts && sec.view.layouts.length > 0) ? sec.view.layouts : [{ id: "main", name: "Main" }]

    function outcome(r, ok) {
        if (r === "saved") sec.app.settingsToast(ok, 3000)
        else if (r === "noop") return
        else if (r === "read-only") sec.app.settingsToast("Saved by a newer Tin Hat — update to edit layouts.", 5000)
        else if (r === "busy") sec.app.settingsToast("Finish editing the layout first (Done or Cancel).", 5000)
        else if (r === "bad-save: too many layouts") sec.app.settingsToast("You have 12 layouts — delete one first.", 5000)
        else sec.app.settingsToast("That didn't work (" + r + ").", 5000)
    }

    SettingsRow {
        app: sec.app
        label: "Layout on this device"
        description: sec.ro ? "Saved by a newer Tin Hat — update to edit layouts." : "Your layouts follow you to every device."
        trailing: ThComboBox {
            id: layoutPick
            objectName: "layoutPick"
            Layout.preferredWidth: 240
            model: sec.list.map(function (l) { return l.name })
            currentIndex: Math.max(0, sec.list.findIndex(function (l) { return l.id === (sec.view.id || "main") }))
            Accessible.name: "Layout on this device"
            onActivated: (i) => { if (sec.list[i]) sec.store.setActive(sec.list[i].id) }
        }
    }
    SettingsRow {
        app: sec.app
        label: "Edit layout"
        description: "Move, resize and add islands. Ctrl+E or right-clicking a gap between islands opens it anywhere."
        trailing: [
            ThButton {
                app: sec.app
                text: "Reset layout"
                icon.name: "view-refresh-symbolic"
                Controls.ToolTip.visible: hovered
                Controls.ToolTip.text: "Classic on this device first — then keep it for all devices or restore"
                Controls.ToolTip.delay: 600
                onClicked: sec.app.resetLayoutHere()
            },
            ThButton {
                app: sec.app
                objectName: "layoutEditBtn"
                text: "Edit layout"
                icon.name: "window-symbolic"
                highlighted: true
                onClicked: sec.app.openEditLayout(true)
            }
        ]
    }
    SettingsRow {
        app: sec.app
        label: "Your layouts"
        description: "Save what you see as a new layout, rename or delete one, or go back to the previous version."
        Flow {
            Layout.fillWidth: true
            spacing: 8
            ThButton {
                app: sec.app
                text: "New layout…"
                enabled: !sec.ro
                onClicked: { namePrompt.mode = "new"; nameField.text = "Layout " + (sec.list.length + 1); namePrompt.open() }
            }
            ThButton {
                app: sec.app
                text: "Rename…"
                enabled: !sec.ro
                onClicked: { namePrompt.mode = "rename"; nameField.text = sec.view.name || "Main"; namePrompt.open() }
            }
            ThButton {
                app: sec.app
                text: "Delete"
                danger: true
                visible: (sec.view.id || "main") !== "main" && sec.view.source === "doc"
                enabled: !sec.ro
                onClicked: deleteAsk.open()
            }
            ThButton {
                app: sec.app
                text: "Restore previous version"
                icon.name: "edit-undo-symbolic"
                visible: sec.view.hasPrevious === true
                enabled: !sec.ro
                onClicked: sec.outcome(sec.store.restorePrevious(), "The previous version is back (the one it replaced is kept as the previous).")
            }
        }
    }

    Kirigami.PromptDialog {
        id: namePrompt
        property string mode: "new"
        title: namePrompt.mode === "new" ? "New layout" : "Rename layout"
        subtitle: namePrompt.mode === "new" ? "A copy of the layout you see now." : ""
        preferredWidth: 360
        standardButtons: Kirigami.Dialog.Save | Kirigami.Dialog.Cancel
        onOpened: { nameField.forceActiveFocus(); nameField.selectAll() }
        onAccepted: sec.acceptName()
        ThTextField {
            id: nameField
            Layout.fillWidth: true
            maximumLength: 40
            Accessible.name: "Layout name"
            onAccepted: { sec.acceptName(); namePrompt.close() }
        }
    }
    function acceptName() {
        var name = nameField.text.trim().substring(0, 40)
        if (name.length === 0) return
        var r = sec.store.rendered()
        if (namePrompt.mode === "new") {
            var id = sec.store.newLayoutId()
            var res = sec.store.saveLayout(id, { name: name, desktop: r.desktop, phone: r.phone })
            if (res === "saved") sec.store.setActive(id)
            sec.outcome(res, "“" + name + "” saved — this device uses it now.")
        } else sec.outcome(sec.store.saveLayout(r.id, { name: name, desktop: r.desktop, phone: r.phone }), "Renamed.")
    }
    ConfirmDialog {
        id: deleteAsk
        app: sec.app
        dialogTitle: "Delete “" + (sec.view.name || "") + "”?"
        message: "It goes from every device. Devices that used it switch to Main."
        confirmText: "Delete"
        destructive: true
        onConfirmed: {
            var name = sec.view.name || ""
            var res = sec.store.saveLayout(sec.view.id, { deleted: true })
            sec.outcome(res, "“" + name + "” deleted.")
            sec.store.setActive("main")
        }
    }
}
