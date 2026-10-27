// ThemeEditorPanel.qml — the theme editor (11.0 step 5, contract THEMES-UI.md §13; the web's ThemeEditor.tsx +
// editorStore.ts are the reference build): a NON-MODAL panel docked to the window's right edge — x = W − 8 − w, y = 8,
// w = min(400, W − 16), h = H − 16, radius 8, the settings.themeEditor.* fill / border / static shadow — above the app's
// islands and below menus, popovers and dialogs, with no veil: the whole app stays visible and usable, painted in the
// working document (the preview). Its header (Edit theme + the meta line; Undo, Redo, Pick, More, Close), the Simple /
// Advanced switch with "Not saved", the body (ThemeEditorSimple / ThemeEditorAdvanced) and the footer (a save error,
// Cancel, Save), plus the discard confirmation, Save as and Export. The session and every rule live in
// ThemeEditorController (theme_editor.rs → tinhat_theme::editor); this file holds the UI state only (mode, search,
// open groups, the Pick tool's result) — like the web's editorStore. The preview itself is painted by the host
// (AppCore.previewTheme) whenever `ctl.stateRev` moves.
//
// `host` (Main.qml / the review harness): notify(text, timeout) · shareDoc(docJson, name) · pickRoot (the item the
// Pick tool walks) · realColours() (the colours Th shows, for the Pick tool's probe pass).
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import QtQuick.Dialogs
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the settings.themeEditor.* tokens

FocusScope {
    id: panel
    property var app: null
    property var ctl: null                  // ThemeEditorController
    property var host: null

    /// the session as the panel shows it (ctl.stateJson, re-read on every stateRev)
    readonly property var st: {
        var _r = panel.ctl ? panel.ctl.stateRev : 0
        try { return panel.ctl && panel.ctl.stateJson.length > 0 ? JSON.parse(panel.ctl.stateJson) : ({ open: false }) }
        catch (e) { return ({ open: false }) }
    }
    readonly property bool shown: panel.st.open === true
    readonly property bool dirty: panel.st.dirty === true

    // ---- UI state (the web's editorStore): kept across sessions — the mode — or reset on open
    property string mode: "simple"          // "simple" | "advanced"
    property string query: ""
    property bool changedOnly: false
    property var openGroups: ({})
    property var picked: null               // the Pick tool's last result [{id, kind}] ("Picked"), null = none
    property bool pickArmed: false
    property string saveError: ""
    onShownChanged: {
        if (panel.shown) {
            panel.query = ""; panel.changedOnly = false; panel.openGroups = ({}); panel.picked = null
            panel.pickArmed = false; panel.saveError = ""
            panel.forceActiveFocus()
        } else {
            panel.pickArmed = false
            panel.closePopups()
        }
    }
    Connections {
        target: panel.ctl
        function onStateRevChanged() { panel.saveError = "" }
    }

    // ---- actions ------------------------------------------------------------------------------------------------
    function edit(op, gesture) {
        if (!panel.ctl) return "closed"
        return panel.ctl.edit(JSON.stringify(op), gesture > 0 ? gesture : 0)
    }
    function newGesture() { return panel.ctl ? panel.ctl.newGesture() : 0 }
    function undo() { if (panel.ctl) panel.ctl.undo() }
    function redo() { if (panel.ctl) panel.ctl.redo() }
    function resetAll() { if (panel.ctl) panel.ctl.resetAll() }
    function togglePick() { if (panel.shown) panel.pickArmed = !panel.pickArmed }
    /// Cancel (§13.4): with changes the confirmation first.
    function cancel(force) {
        if (!panel.shown) return
        if (force !== true && panel.dirty) { discardDialog.open(); return }
        panel.ctl.close()
    }
    /// Save (§13.4): My themes + applied, the editor closes; `asName` = Save as a new theme with that name.
    function save(asName) {
        var r = JSON.parse(panel.ctl.save(asName || ""))
        if (!r.ok) { panel.saveError = r.text; return false }
        if (panel.host) panel.host.notify(r.name + " saved to My themes", "short")
        return true
    }
    function exportFile() {
        exportDialog.selectedFile = panel.ctl.exportUrl()
        exportDialog.open()
    }
    function shareWorking() {
        var d = panel.ctl.workingDoc()
        if (d.length > 0 && panel.host) panel.host.shareDoc(d, String(panel.st.doc.name || ""))
    }
    /// The Pick tool's result (ThemePickOverlay): the "Picked" rows first in Advanced, the search cleared.
    function showPicked(list) {
        panel.pickArmed = false
        if (list.length === 0) { if (panel.host) panel.host.notify("No named colours there.", "short"); return }
        panel.picked = list
        panel.mode = "advanced"
        panel.query = ""
        panel.changedOnly = false
    }

    // ---- the review harness's hooks (ThemeEditorGallery.qml: the web's editor-shots clicks)
    function testOpenPopover(kind, id) {
        if (kind === "background" || kind === "accent") simpleMode.openPopover(kind)
        else advancedMode.openRowPopover(kind, id)
    }
    function testOpenMore() { moreMenu.popup(moreBtn, 0, moreBtn.height) }
    function testPickHover(x, y) { pickOverlay.testHover(x, y) }
    function testPickClick(x, y) { panel.showPicked(pickOverlay.sampleAt(x, y)) }
    /// every popup of the panel closed (the editor closing; between two review states)
    function closePopups() {
        simpleMode.closePopover(); advancedMode.closePopover()
        moreMenu.close(); discardDialog.close(); saveAsDialog.close()
    }

    // ---- keys (§13.3, §15.3): Ctrl+Z / Ctrl+Shift+Z / Ctrl+Y anywhere but a text field (a text field takes them
    // first: ShortcutOverride); Esc and P while the panel has the focus
    Shortcut { sequences: ["Ctrl+Z"]; enabled: panel.shown; context: Qt.ApplicationShortcut; onActivated: panel.undo() }
    Shortcut { sequences: ["Ctrl+Shift+Z", "Ctrl+Y"]; enabled: panel.shown; context: Qt.ApplicationShortcut; onActivated: panel.redo() }
    Keys.onPressed: (e) => {
        if (e.modifiers & (Qt.ControlModifier | Qt.AltModifier | Qt.MetaModifier)) return
        if (e.key === Qt.Key_Escape) { panel.cancel(false); e.accepted = true }
        else if (e.key === Qt.Key_P) { panel.togglePick(); e.accepted = true }
    }

    visible: panel.shown
    // docked right (THEMES-UI §13.6): the parent is the window's content
    x: (parent ? parent.width : 416) - 8 - width
    y: 8
    width: Math.min(400, (parent ? parent.width : 416) - 16)
    height: (parent ? parent.height : 816) - 16

    // eat pointer events that reach the panel's background (the app under it stays untouched)
    MouseArea { anchors.fill: parent; acceptedButtons: Qt.AllButtons; onWheel: (w) => { w.accepted = true } }

    Kirigami.ShadowedRectangle {
        id: frame
        anchors.fill: parent
        radius: 8
        color: Th.settingsThemeEditorBg
        border.width: 1
        border.color: Th.settingsThemeEditorBorder
        // a static shadow (GPU.md: never animated): blur 16, y 4
        shadow.size: 16
        shadow.yOffset: 4
        shadow.color: Th.settingsThemeEditorShadow
    }

    ColumnLayout {
        anchors.fill: parent
        anchors.margins: 1
        spacing: 0

        // ---- the header: 52 high, padding 0 8 0 12, a 1 px rule under it -------------------------------------------
        Item {
            Layout.fillWidth: true
            Layout.preferredHeight: 52
            RowLayout {
                anchors.fill: parent
                anchors.leftMargin: 11      // 12 from the panel's edge (inside its 1 px border)
                anchors.rightMargin: 7
                spacing: 2
                ColumnLayout {
                    Layout.fillWidth: true
                    Layout.minimumWidth: 0
                    spacing: 0
                    Controls.Label {
                        text: "Edit theme"
                        color: Th.settingsFg
                        font.pointSize: Kirigami.Theme.defaultFont.pointSize * 1.1 * Th.textScale
                        font.weight: Font.DemiBold
                    }
                    Controls.Label {
                        textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                        Layout.fillWidth: true
                        text: panel.st.target === "mine" ? "One of My themes" : "A new theme from " + (panel.st.baseName || "")
                        color: Th.settingsHintFg
                        font.pointSize: Kirigami.Theme.smallFont.pointSize * Th.textScale
                        elide: Text.ElideRight
                    }
                }
                ThemeEditorIconButton {
                    iconName: "edit-undo-symbolic"; tip: "Undo"
                    enabled: panel.st.canUndo === true
                    onClicked: panel.undo()
                }
                ThemeEditorIconButton {
                    iconName: "edit-redo-symbolic"; tip: "Redo"
                    enabled: panel.st.canRedo === true
                    onClicked: panel.redo()
                }
                ThemeEditorIconButton {
                    iconName: "color-picker"; tip: "Pick a colour from the app"
                    pressedOn: panel.pickArmed
                    onClicked: panel.togglePick()
                }
                ThemeEditorIconButton {
                    id: moreBtn
                    iconName: "view-more-symbolic"; tip: "More"
                    onClicked: moreMenu.popup(moreBtn, 0, moreBtn.height)
                }
                ThemeEditorIconButton {
                    iconName: "window-close-symbolic"; tip: "Close"
                    onClicked: panel.cancel(false)
                }
            }
            Rectangle { anchors { left: parent.left; right: parent.right; bottom: parent.bottom } height: 1; color: Th.settingsThemeEditorDivider }
        }
        Controls.Menu {
            id: moreMenu
            Controls.MenuItem { text: "Save as a new theme…"; icon.name: "list-add-symbolic"; onTriggered: saveAsDialog.openFor() }
            Controls.MenuItem { text: "Export…"; icon.name: "document-save-symbolic"; onTriggered: panel.exportFile() }
            Controls.MenuItem { text: "Share to a room…"; icon.name: "document-send-symbolic"; onTriggered: panel.shareWorking() }
            Controls.MenuSeparator {}
            Controls.MenuItem { text: "Reset all changes"; icon.name: "edit-undo-symbolic"; enabled: panel.dirty; onTriggered: panel.resetAll() }
        }

        // ---- the mode row (padding 8 12): Simple · Advanced, gap 0; "Not saved" right while dirty ------------------
        RowLayout {
            Layout.fillWidth: true
            Layout.leftMargin: 11
            Layout.rightMargin: 11
            Layout.topMargin: 8
            Layout.bottomMargin: 8
            spacing: 0
            Repeater {
                model: [["simple", "Simple"], ["advanced", "Advanced"]]
                delegate: Controls.ToolButton {
                    required property var modelData
                    text: LinkGate.plain(modelData[1])
                    display: Controls.AbstractButton.TextOnly
                    checkable: true
                    checked: panel.mode === modelData[0]
                    implicitHeight: 32
                    Accessible.role: Accessible.RadioButton
                    onClicked: { panel.mode = modelData[0]; checked = Qt.binding(() => panel.mode === modelData[0]) }
                }
            }
            Item { Layout.fillWidth: true }
            Controls.Label {
                visible: panel.dirty
                text: "Not saved"
                color: Th.settingsHintFg
                font.pointSize: Kirigami.Theme.smallFont.pointSize * Th.textScale
            }
        }

        // ---- the body ---------------------------------------------------------------------------------------------
        Item {
            Layout.fillWidth: true
            Layout.fillHeight: true
            ThemeEditorSimple {
                id: simpleMode
                anchors.fill: parent
                visible: panel.mode === "simple"
                app: panel.app
                panel: panel
            }
            ThemeEditorAdvanced {
                id: advancedMode
                anchors.fill: parent
                visible: panel.mode === "advanced"
                app: panel.app
                panel: panel
            }
        }

        // ---- the footer (padding 8 12, a 1 px rule above): a save error, then Cancel · Save ------------------------
        Rectangle { Layout.fillWidth: true; Layout.preferredHeight: 1; color: Th.settingsThemeEditorDivider }
        ColumnLayout {
            Layout.fillWidth: true
            Layout.leftMargin: 11
            Layout.rightMargin: 11
            Layout.topMargin: 8
            Layout.bottomMargin: 8
            spacing: 8
            Kirigami.InlineMessage {
                Layout.fillWidth: true
                visible: panel.saveError.length > 0
                type: Kirigami.MessageType.Error
                text: LinkGate.plain(panel.saveError)
            }
            RowLayout {
                Layout.fillWidth: true
                spacing: 8
                Item { Layout.fillWidth: true }
                ThButton { app: panel.app; text: "Cancel"; onClicked: panel.cancel(false) }
                ThButton {
                    app: panel.app
                    text: "Save"
                    highlighted: true
                    enabled: !(panel.st.target === "mine" && !panel.dirty)
                    onClicked: panel.save("")
                }
            }
        }
    }

    // ---- the Pick tool (§15.3): the catcher + outline over the app, under this panel --------------------------------
    ThemePickOverlay {
        id: pickOverlay
        parent: panel.parent
        anchors.fill: parent
        z: panel.z - 1
        armed: panel.shown && panel.pickArmed
        panel: panel
    }

    // ---- dialogs ------------------------------------------------------------------------------------------------
    ConfirmDialog {
        id: discardDialog
        app: panel.app
        dialogTitle: "Discard changes?"
        message: "Discard your changes to " + String(panel.st.doc ? panel.st.doc.name : "") + "?"
        confirmText: "Discard"
        cancelText: "Keep editing"
        destructive: true
        onConfirmed: panel.cancel(true)
    }
    // Save as a new theme… (§13.4): the name (Edit: its copyName), Save / Cancel
    Kirigami.PromptDialog {
        id: saveAsDialog
        title: "Save as a new theme"
        preferredWidth: 360
        standardButtons: Kirigami.Dialog.Save | Kirigami.Dialog.Cancel
        function openFor() {
            var n = String(panel.st.doc ? panel.st.doc.name : "")
            saveAsField.text = panel.st.target === "mine" ? panel.ctl.copyName(n) : n
            saveAsDialog.open()
        }
        function accept() {
            var n = saveAsField.text.trim()
            if (n.length < 1 || n.length > 80) return
            saveAsDialog.close()
            panel.save(n)
        }
        onOpened: { saveAsField.forceActiveFocus(); saveAsField.selectAll() }
        onAccepted: saveAsDialog.accept()
        ThTextField {
            id: saveAsField
            Layout.fillWidth: true
            maximumLength: 80
            Accessible.name: "Theme name"
            onAccepted: saveAsDialog.accept()
        }
    }
    // Export… (§13.4): `<slug>.tinhat-theme.json`, a Save dialog in Downloads
    FileDialog {
        id: exportDialog
        title: "Export theme"
        fileMode: FileDialog.SaveFile
        nameFilters: ["Tin Hat themes (*.tinhat-theme.json)", "All files (*)"]
        onAccepted: {
            var err = panel.ctl.exportTo(String(selectedFile))
            if (panel.host) panel.host.notify(err.length > 0 ? err : "Exported " + panel.ctl.exportName(), err.length > 0 ? "long" : "short")
        }
    }
}
