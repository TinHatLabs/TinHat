// ThemeImportDialog.qml — "Import a theme" (themes step 1b, contract THEMES-UI.md §6; the web's ImportDialog.tsx):
// paste a theme, drop a file on the dialog or choose one; the engine's importers (tinhat_theme::import_theme: kitty,
// Alacritty, Ghostty, iTerm2, WezTerm, Windows Terminal, Xresources, base16/base24, Tin Hat theme files) read it 150 ms
// after the last change; a refusal is shown in plain words (§6.1); a read theme shows its card at 1.5× (a 160 × 108
// card → 240 × 162, captioned with the Name field), the Name field, the format and variant, the adjusted note, and
// saves to My themes (THEMES-SYNC §4.1: its own id) — "Save and use" applies it too. An outside click never closes it
// (a paste would be lost); a file over 256 KB is not read.
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import QtQuick.Dialogs as Dialogs
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the active theme (docs/MIGRATING-THEME.md)

Controls.Dialog {
    id: dlg
    property var app: null
    property var ctl: null
    signal saved(string id, string name, bool used)

    title: "Import a theme"
    modal: true
    closePolicy: Controls.Popup.CloseOnEscape
    parent: Controls.Overlay.overlay
    anchors.centerIn: parent
    width: Math.min(560, (parent ? parent.width : 560) - Kirigami.Units.gridUnit * 2)

    property string fileName: ""
    // (not `result`: Dialog's own is FINAL — Qt 6.11 refuses a component that redeclares it)
    property var readResult: null        // importText's answer (or a file refusal {ok: false, text})
    property bool nameTouched: false
    property bool loading: false         // the text is being set from a file (keeps its name)
    readonly property bool readOk: readResult !== null && readResult.ok === true
    readonly property string nameText: nameField.text.trim()
    readonly property bool nameOk: nameText.length >= 1 && nameText.length <= 80
    readonly property bool canSave: readOk && nameOk
    property string saveError: ""

    // (not `reset`: Dialog has a reset() signal — a function of that name is an invalid override, and the call
    // could emit the signal instead of clearing the form)
    function clearForm() {
        dlg.loading = true
        area.text = ""
        dlg.loading = false
        dlg.fileName = ""; dlg.readResult = null; dlg.saveError = ""; dlg.nameTouched = false; nameField.text = ""
    }
    /// open with a file (a drop on the gallery page) or empty
    function openWith(url) {
        dlg.clearForm()
        dlg.open()
        if (url && url.length > 0) dlg.loadFile(url)
    }
    function loadFile(url) {
        var r = JSON.parse(dlg.ctl.readFile(url))
        dlg.loading = true
        area.text = r.ok ? r.text : ""
        dlg.loading = false
        dlg.fileName = r.name || ""
        dlg.nameTouched = false
        dlg.saveError = ""
        if (r.ok) dlg.parseNow()
        else { parseT.stop(); dlg.readResult = { ok: false, text: r.text } }
    }
    function parseNow() {
        parseT.stop()
        if (area.text.trim().length === 0) { dlg.readResult = null; return }
        dlg.readResult = JSON.parse(dlg.ctl.importText(area.text, dlg.fileName))
        if (dlg.readResult.ok && !dlg.nameTouched) {
            nameField.text = dlg.readResult.name
            dlg.nameTouched = false
        }
    }
    function save(useIt) {
        if (!dlg.canSave) return
        var r = JSON.parse(dlg.ctl.saveImported(JSON.stringify(dlg.readResult.doc), dlg.nameText, useIt))
        if (!r.ok) { dlg.saveError = r.text; return }
        var n = dlg.nameText
        dlg.close()
        dlg.saved(r.id, n, useIt)
    }

    Timer { id: parseT; interval: 150; onTriggered: dlg.parseNow() }

    Dialogs.FileDialog {
        id: fileDialog
        title: "Choose a theme file"
        onAccepted: dlg.loadFile(String(selectedFile))
    }

    contentItem: Item {
        implicitWidth: col.implicitWidth
        implicitHeight: col.implicitHeight
        ColumnLayout {
            id: col
            anchors.fill: parent
            spacing: 8
            HintLabel {
                app: dlg.app
                Layout.fillWidth: true
                text: "Paste a theme, drop a file here, or choose one. Tin Hat reads kitty, Alacritty, Ghostty, iTerm2, WezTerm, "
                      + "Windows Terminal, Xresources and base16 / base24 themes, and Tin Hat theme files."
            }
            Controls.ScrollView {
                id: areaScroll
                Layout.fillWidth: true
                Layout.preferredHeight: Math.round(fontMetrics.height * 8 + area.topPadding + area.bottomPadding)
                Controls.ScrollBar.vertical: FoxScrollBar { app: dlg.app }
                ThTextArea {
                    id: area
                    placeholderText: "Paste the theme here…"
                    font.family: Th.fontMono
                    wrapMode: TextEdit.NoWrap
                    Accessible.name: "Theme text"
                    onTextChanged: {
                        if (dlg.loading) return
                        dlg.fileName = ""          // typed / pasted text is no longer the file's
                        dlg.saveError = ""
                        parseT.restart()
                    }
                    FontMetrics { id: fontMetrics; font: area.font }
                }
            }
            RowLayout {
                spacing: 8
                ThButton {
                    app: dlg.app
                    text: "Choose file…"
                    icon.name: "document-open-symbolic"
                    onClicked: fileDialog.open()
                }
                Controls.Label {
                    Layout.fillWidth: true
                    visible: dlg.fileName.length > 0
                    text: dlg.fileName
                    color: Th.settingsThemeMetaFg
                    elide: Text.ElideRight
                }
            }
            Kirigami.InlineMessage {
                Layout.fillWidth: true
                visible: dlg.readResult !== null && dlg.readResult.ok === false
                type: Kirigami.MessageType.Error
                text: (dlg.readResult && dlg.readResult.text) ? dlg.readResult.text : ""
            }
            Kirigami.InlineMessage {
                Layout.fillWidth: true
                visible: dlg.saveError.length > 0
                type: Kirigami.MessageType.Error
                text: dlg.saveError
            }
            RowLayout {
                visible: dlg.readOk
                spacing: 12
                // the card at 1.5×: a 160 × 108 card scaled into 240 × 162
                Item {
                    Layout.alignment: Qt.AlignTop
                    Layout.preferredWidth: 240
                    Layout.preferredHeight: 162
                    ThemeCard {
                        width: 160
                        height: 108
                        scale: 1.5
                        transformOrigin: Item.TopLeft
                        app: dlg.app
                        interactive: false
                        captionText: dlg.nameText
                        c: dlg.readOk ? dlg.ctl.cardForDoc(JSON.stringify(dlg.readResult.doc)) : ({})
                    }
                }
                ColumnLayout {
                    Layout.fillWidth: true
                    Layout.alignment: Qt.AlignTop
                    spacing: 6
                    Controls.Label { text: "Name"; color: Th.settingsLabelFg }
                    ThTextField {
                        id: nameField
                        Layout.fillWidth: true
                        maximumLength: 80
                        Accessible.name: "Name"
                        onTextEdited: dlg.nameTouched = true
                        onAccepted: dlg.save(true)
                    }
                    Controls.Label {
                        Layout.fillWidth: true
                        color: Th.settingsThemeMetaFg
                        elide: Text.ElideRight
                        text: dlg.readOk ? dlg.readResult.formatName + " theme · " + (dlg.readResult.variant === "light" ? "Light" : "Dark") : ""
                    }
                    Controls.Label {
                        Layout.fillWidth: true
                        visible: dlg.readOk && dlg.readResult.adjusted > 0
                        wrapMode: Text.Wrap
                        color: Th.settingsThemeMetaFg
                        text: dlg.readOk ? (dlg.readResult.adjusted === 1 ? "1 colour adjusted for readability"
                                                                      : dlg.readResult.adjusted + " colours adjusted for readability") : ""
                    }
                }
            }
        }
        // the whole dialog is a drop target (THEMES-UI §6 item 2)
        DropArea {
            id: drop
            anchors.fill: parent
            keys: ["text/uri-list"]
            onDropped: (d) => { if (d.hasUrls && d.urls.length > 0) { dlg.loadFile(String(d.urls[0])); d.acceptProposedAction() } }
        }
        ThemeDropOverlay { anchors.fill: parent; visible: drop.containsDrag }
    }

    // one role for all three: the box keeps them in this order under every style's button layout (Cancel · Save to My
    // themes · Save and use, like the web); a refused save keeps the dialog open, so none of them is an Accept button
    footer: Controls.DialogButtonBox {
        Controls.Button {
            text: "Cancel"
            Controls.DialogButtonBox.buttonRole: Controls.DialogButtonBox.ActionRole
            onClicked: dlg.close()
        }
        Controls.Button {
            text: "Save to My themes"
            enabled: dlg.canSave
            Controls.DialogButtonBox.buttonRole: Controls.DialogButtonBox.ActionRole
            onClicked: dlg.save(false)
        }
        Controls.Button {
            text: "Save and use"
            highlighted: true
            enabled: dlg.canSave
            Controls.DialogButtonBox.buttonRole: Controls.DialogButtonBox.ActionRole
            onClicked: dlg.save(true)
        }
    }
}
