// ShortcutsPane.qml — Settings → Shortcuts in the Settings window's card system (SETTINGS-WINDOW.md §5; 11.0 buttons,
// ACTIONS.md §8.8; DESIGN.md "Shortcuts: any action can get a keyboard shortcut in Settings → Shortcuts; clashes are
// flagged"; the web's src/actions/ui/ShortcutsPane.tsx — the same list, texts and numbers). Every action a person may give
// a key (the engine's `shortcuts` view: per group, by label), its keys now (chips), "Change…" (the key recorder), "↺" back
// to Tin Hat's key; a key two actions share is flagged on both rows and runs the one it was given to (the dispatch order,
// §8.6). Keys stay on this device (pref `shortcuts`); ActionsHost.qml applies them (the default Shortcut items, a Shortcut
// per key of your own, push to talk).
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the active theme (docs/MIGRATING-THEME.md)

ColumnLayout {
    id: pane
    property var app: null
    spacing: 24
    readonly property var host: pane.app && pane.app.actions ? pane.app.actions : null
    // the list (re-read when this device's keys change)
    readonly property var view: {
        var _t = pane.host ? pane.host.shortcutsDocText : ""
        return pane.host ? pane.host.shortcutsView() : ({ groups: [], conflicts: 0 })
    }
    property string query: ""
    property string folded: ""
    onQueryChanged: pane.folded = pane.query.length > 0 ? pane.fold(pane.query) : ""
    property var _foldCache: ({})
    function fold(s) {
        var k = String(s || "")
        if (pane._foldCache[k] !== undefined) return pane._foldCache[k]
        var f = k.toLowerCase()
        try { f = JSON.parse(String(pane.app.actionsCtl.call("fold", JSON.stringify({ text: k })))).folded || "" } catch (e) {}
        pane._foldCache[k] = f
        return f
    }
    readonly property bool anyChanged: {
        var gs = pane.view.groups || []
        for (var g = 0; g < gs.length; g++) for (var i = 0; i < gs[g].items.length; i++) if (gs[g].items[i].changed) return true
        return false
    }
    // the review captures' --tinhat-keyrec hook (ActionsHost): open the recorder for that action
    Connections {
        target: pane.host
        ignoreUnknownSignals: true
        function onKeyRecRequested(action) {
            var gs = pane.view.groups || []
            for (var g = 0; g < gs.length; g++) for (var i = 0; i < gs[g].items.length; i++) if (gs[g].items[i].a === action) { recorder.openFor(gs[g].items[i]); return }
        }
    }
    function labelOf(id) {
        var t = pane.app && pane.app.actionsTable ? pane.app.actionsTable.actions : []
        for (var i = 0; i < t.length; i++) if (t[i].id === id) return t[i].label
        return id
    }

    // the search field and "Reset all" (a toolbar, not a card), then one card per group (SETTINGS-WINDOW.md §5)
    SettingsSection {
        anchorId: "shortcuts.shortcuts"
        plain: true
        hint: "Change or remove the keys Tin Hat comes with, or give any action one of your own. Your keys stay on this device."
        RowLayout {
            Layout.fillWidth: true
            Layout.topMargin: 4
            spacing: 8
            ThTextField {
                Layout.fillWidth: true
                placeholderText: "Search actions…"
                Accessible.name: "Search actions"
                onTextChanged: pane.query = text
            }
            ThButton {
                visible: pane.anyChanged
                app: pane.app
                flat: true
                text: "Reset all"
                icon.name: "edit-undo-symbolic"
                onClicked: if (pane.host) pane.host.resetAllShortcuts()
            }
        }
        Controls.Label {
            textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
            visible: (pane.view.conflicts || 0) > 0
            Layout.fillWidth: true
            Layout.topMargin: 6
            wrapMode: Text.Wrap
            color: Th.stateWarning
            font.pixelSize: Math.round(12 * Th.textScale)
            text: (pane.view.conflicts === 1 ? "One key is on two actions" : pane.view.conflicts + " keys are on two actions")
                  + " — it runs the one you gave it to; the other one has no key there."
        }
    }
    Repeater {
        model: pane.view.groups || []
        delegate: SettingsSection {
            id: grp
            required property var modelData
            readonly property var shownItems: (modelData.items || []).filter(function (it) { return pane.folded.length === 0 || pane.fold(it.label).indexOf(pane.folded) >= 0 })
            visible: grp.shownItems.length > 0
            title: grp.modelData.label
            Repeater {
                model: grp.shownItems
                delegate: SettingsRow {
                    id: row
                    required property var modelData
                    app: pane.app
                    padV: 11
                    iconName: row.modelData.icon || "input-keyboard-symbolic"
                    iconColor: Th.actionPaletteIcon
                    label: row.modelData.label
                    description: (row.modelData.clashes || []).map(function (c) { return c.key + " is also on " + c.with.map(function (x) { return pane.labelOf(x) }).join(", ") }).join("\n")
                    descriptionColor: Th.stateWarning
                    trailing: [
                        Controls.Label {
                            visible: (row.modelData.keys || []).length === 0
                            text: "No key"
                            color: Th.settingsRowDescriptionFg
                            font.pixelSize: Math.round(12 * Th.textScale)
                        },
                        Repeater {
                            model: row.modelData.keys || []
                            delegate: Rectangle {
                                required property string modelData
                                readonly property bool clash: (row.modelData.clashes || []).some(function (c) { return c.key === modelData })
                                implicitWidth: chipText.implicitWidth + 14
                                implicitHeight: 24
                                radius: 6
                                color: Th.surfaceSunken
                                border.width: 1
                                border.color: clash ? Th.stateWarning : Th.settingsCardBorder
                                Controls.Label {
                                    textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                    id: chipText
                                    anchors.centerIn: parent
                                    text: modelData + (row.modelData.hold ? " (hold)" : "")
                                    color: Th.settingsRowLabelFg
                                    font.family: Th.fontMono
                                    font.pixelSize: Math.round(12 * Th.textScale)
                                }
                            }
                        },
                        ThToolButton {
                            visible: row.modelData.changed === true
                            app: pane.app
                            iconName: "edit-undo-symbolic"
                            tip: "Back to Tin Hat's key"
                            onClicked: if (pane.host) pane.host.setActionKeys(row.modelData.a, null)
                        },
                        Item { visible: row.modelData.changed !== true; implicitWidth: 32; implicitHeight: 1 },
                        ThButton {
                            app: pane.app
                            flat: true
                            text: "Change…"
                            onClicked: recorder.openFor(row.modelData)
                        }
                    ]
                }
            }
        }
    }

    // ---- the key recorder: press the keys (a modifier held alone for push to talk), see what it means, Save ----
    Controls.Dialog {
        id: recorder
        property var item: null
        property string recKey: ""
        property string pendingMod: ""
        readonly property var check: {
            if (!recorder.item || recorder.recKey.length === 0 || !pane.app) return null
            try { return JSON.parse(String(pane.app.actionsCtl.call("checkBinding", JSON.stringify({ app: "desktop", action: recorder.item.a, key: recorder.recKey })))) } catch (e) { return null }
        }
        readonly property var errorTexts: ({
            "reserved": "Tin Hat keeps this key for moving around (Escape, Tab, F6).",
            "hold-key": "Push to talk takes a modifier held on its own (Ctrl, Alt, Shift or Meta) or an F-key.",
            "hold-only": "A modifier on its own is only for push to talk — add a key to it.",
            "needs-modifier": "This key types text — add Ctrl, Alt or Meta, or use an F-key.",
            "browser-reserved": "Your browser keeps this key for itself — pick another one.",
            "fixed-taken": "Tin Hat uses this key for something that can't move (Ctrl+V pastes, Return sends).",
            "bad-sequence": "That key can't be used."
        })
        readonly property string errorText: recorder.check && recorder.check.ok === false ? (recorder.errorTexts[recorder.check.error] || recorder.errorTexts["bad-sequence"]) : ""
        // the actions this key would take it from (a clash after Save)
        readonly property var takes: {
            if (!recorder.check || recorder.check.ok !== true || !pane.host) return []
            try {
                var doc0 = pane.host.shortcutsDocText.length > 0 ? JSON.parse(pane.host.shortcutsDocText) : null
                var inp = { app: "desktop", action: recorder.item.a, keys: [recorder.check.key] }
                if (doc0 !== null) inp.doc = doc0
                var doc = JSON.parse(String(pane.app.actionsCtl.call("setBinding", JSON.stringify(inp)))).doc
                var inp2 = { app: "desktop" }
                if (doc) inp2.doc = doc
                var eff = JSON.parse(String(pane.app.actionsCtl.call("effective", JSON.stringify(inp2))))
                var out = []
                for (var i = 0; i < eff.conflicts.length; i++) {
                    var c = eff.conflicts[i]
                    if (c.key !== recorder.check.key) continue
                    var other = c.actions[0] === recorder.item.a ? c.actions[1] : (c.actions[1] === recorder.item.a ? c.actions[0] : "")
                    if (other.length > 0 && out.indexOf(other) < 0) out.push(other)
                }
                return out
            } catch (e) { return [] }
        }
        function openFor(it) { recorder.item = it; recorder.recKey = ""; recorder.pendingMod = ""; recorder.open() }
        title: LinkGate.plain(recorder.item ? "Key for “" + recorder.item.label + "”" : "Key")
        modal: true
        parent: Controls.Overlay.overlay
        anchors.centerIn: parent
        width: Math.min(440, (parent ? parent.width : 440) - Kirigami.Units.gridUnit * 2)
        closePolicy: Controls.Popup.CloseOnEscape
        standardButtons: Controls.Dialog.Reset | Controls.Dialog.Cancel | Controls.Dialog.Save
        onOpened: recBox.forceActiveFocus()
        onAccepted: if (recorder.check && recorder.check.ok === true && pane.host) pane.host.setActionKeys(recorder.item.a, [recorder.check.key])
        onReset: { if (pane.host && recorder.item) pane.host.setActionKeys(recorder.item.a, []); recorder.close() }
        function _labelButtons() {
            var rs = recorder.standardButton(Controls.Dialog.Reset)
            if (rs) rs.text = "No key"
            var sv = recorder.standardButton(Controls.Dialog.Save)
            if (sv) sv.enabled = Qt.binding(function () { return !!recorder.check && recorder.check.ok === true })
        }
        Component.onCompleted: recorder._labelButtons()
        onAboutToShow: recorder._labelButtons()

        function modName(k) {
            if (k === Qt.Key_Control) return "Ctrl"
            if (k === Qt.Key_Alt) return "Alt"
            if (k === Qt.Key_Shift) return "Shift"
            if (k === Qt.Key_Meta) return "Meta"
            return ""
        }
        // a Qt key → the actions table's key name (letters, digits and punctuation as on a US layout)
        function keyName(k, shift) {
            if (k >= Qt.Key_A && k <= Qt.Key_Z) return String.fromCharCode(k)
            if (k >= Qt.Key_0 && k <= Qt.Key_9) return String.fromCharCode(k)
            if (k >= Qt.Key_F1 && k <= Qt.Key_F24) return "F" + (k - Qt.Key_F1 + 1)
            var named = {}
            named[Qt.Key_Escape] = "Escape"; named[Qt.Key_Return] = "Return"; named[Qt.Key_Enter] = "Return"; named[Qt.Key_Space] = "Space"
            named[Qt.Key_Tab] = "Tab"; named[Qt.Key_Backtab] = "Tab"; named[Qt.Key_Backspace] = "Backspace"; named[Qt.Key_Delete] = "Delete"
            named[Qt.Key_Insert] = "Insert"; named[Qt.Key_Up] = "Up"; named[Qt.Key_Down] = "Down"; named[Qt.Key_Left] = "Left"
            named[Qt.Key_Right] = "Right"; named[Qt.Key_PageUp] = "PgUp"; named[Qt.Key_PageDown] = "PgDown"; named[Qt.Key_Home] = "Home"
            named[Qt.Key_End] = "End"; named[Qt.Key_Pause] = "Pause"; named[Qt.Key_Print] = "Print"; named[Qt.Key_Menu] = "Menu"
            named[Qt.Key_Comma] = ","; named[Qt.Key_Less] = ","; named[Qt.Key_Period] = "."; named[Qt.Key_Greater] = "."
            named[Qt.Key_Slash] = "/"; named[Qt.Key_Question] = "/"; named[Qt.Key_Semicolon] = ";"; named[Qt.Key_Colon] = ";"
            named[Qt.Key_Apostrophe] = "'"; named[Qt.Key_QuoteDbl] = "'"; named[Qt.Key_BracketLeft] = "["; named[Qt.Key_BraceLeft] = "["
            named[Qt.Key_BracketRight] = "]"; named[Qt.Key_BraceRight] = "]"; named[Qt.Key_Backslash] = "\\"; named[Qt.Key_Bar] = "\\"
            named[Qt.Key_Minus] = "-"; named[Qt.Key_Underscore] = "-"; named[Qt.Key_Equal] = "="; named[Qt.Key_QuoteLeft] = "`"
            named[Qt.Key_AsciiTilde] = "`"; named[Qt.Key_Plus] = shift ? "=" : "+"
            named[Qt.Key_Exclam] = "1"; named[Qt.Key_At] = "2"; named[Qt.Key_NumberSign] = "3"; named[Qt.Key_Dollar] = "4"
            named[Qt.Key_Percent] = "5"; named[Qt.Key_AsciiCircum] = "6"; named[Qt.Key_Ampersand] = "7"; named[Qt.Key_Asterisk] = "8"
            named[Qt.Key_ParenLeft] = "9"; named[Qt.Key_ParenRight] = "0"
            return named[k] !== undefined ? named[k] : ""
        }

        // the content's height drives the dialog's (Breeze's Dialog kept the height it opened with when a line appeared)
        contentHeight: recBody.implicitHeight
        ColumnLayout {
            id: recBody
            width: recorder.availableWidth
            spacing: 8
            HintLabel {
                app: pane.app
                Layout.fillWidth: true
                text: recorder.item && recorder.item.hold ? "Hold a modifier (Ctrl, Alt, Shift or Meta) and let go, or press an F-key."
                                                         : "Press the keys you want — Ctrl, Alt or Meta with a key, or an F-key."
            }
            Rectangle {
                id: recBox
                Layout.fillWidth: true
                implicitHeight: 64
                radius: 8
                color: Th.surfaceSunken
                border.width: 1
                border.color: recBox.activeFocus ? Th.borderFocus : Th.dialogBorder
                activeFocusOnTab: true
                Accessible.role: Accessible.EditableText
                Accessible.name: "Press the keys"
                // no application shortcut fires while a key is being recorded (the press comes here instead)
                Keys.onShortcutOverride: (event) => { if (event.key !== Qt.Key_Escape && event.key !== Qt.Key_Tab && event.key !== Qt.Key_Backtab) event.accepted = true }
                Keys.onPressed: (event) => {
                    if (event.key === Qt.Key_Escape || event.key === Qt.Key_Tab || event.key === Qt.Key_Backtab) return
                    event.accepted = true
                    if (event.isAutoRepeat) return
                    var m = recorder.modName(event.key)
                    if (m.length > 0) { recorder.pendingMod = m; return }
                    recorder.pendingMod = ""
                    var mods = event.modifiers
                    var name = recorder.keyName(event.key, (mods & Qt.ShiftModifier) !== 0)
                    if (name.length === 0) return
                    var parts = []
                    if (mods & Qt.ControlModifier) parts.push("Ctrl")
                    if (mods & Qt.AltModifier) parts.push("Alt")
                    if (mods & Qt.ShiftModifier) parts.push("Shift")
                    if (mods & Qt.MetaModifier) parts.push("Meta")
                    parts.push(name)
                    recorder.recKey = parts.join("+")
                }
                Keys.onReleased: (event) => {
                    var m = recorder.modName(event.key)
                    if (m.length > 0 && m === recorder.pendingMod) { recorder.recKey = m; recorder.pendingMod = ""; event.accepted = true }
                }
                TapHandler { onTapped: recBox.forceActiveFocus() }
                Controls.Label {
                    textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                    anchors.centerIn: parent
                    text: recorder.recKey.length > 0 ? recorder.recKey : "Press the keys…"
                    color: recorder.recKey.length === 0 ? Th.dialogHintFg : (recorder.errorText.length > 0 ? Th.dialogErrorFg : Th.dialogFg)
                    font.family: recorder.recKey.length > 0 ? "monospace" : Kirigami.Theme.defaultFont.family
                    font.pointSize: recorder.recKey.length > 0 ? Kirigami.Theme.defaultFont.pointSize * 1.1 : Kirigami.Theme.defaultFont.pointSize
                    font.bold: recorder.recKey.length > 0
                }
            }
            Controls.Label {
                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                visible: recorder.errorText.length > 0
                Layout.fillWidth: true
                wrapMode: Text.Wrap
                text: recorder.errorText
                color: Th.dialogErrorFg
                font.pointSize: Kirigami.Theme.smallFont.pointSize
            }
            Controls.Label {
                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                visible: recorder.errorText.length === 0 && recorder.takes.length > 0
                Layout.fillWidth: true
                wrapMode: Text.Wrap
                text: recorder.recKey + " is on " + recorder.takes.map(function (x) { return pane.labelOf(x) }).join(", ")
                      + " now — it will run “" + (recorder.item ? recorder.item.label : "") + "” instead."
                color: Th.stateWarning
                font.pointSize: Kirigami.Theme.smallFont.pointSize
            }
            Controls.Label {
                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                visible: !!recorder.item && (recorder.item.defaults || []).length > 0
                Layout.fillWidth: true
                text: "Tin Hat's key: " + (recorder.item ? (recorder.item.defaults || []).join(", ") : "")
                color: Th.dialogHintFg
                font.pointSize: Kirigami.Theme.smallFont.pointSize
            }
        }
    }
}
