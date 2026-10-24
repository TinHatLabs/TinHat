// ChangePasswordDialog.qml — Settings → Devices & security → Your account → Password → "Change…" (selfhost,
// 2026-10-22: a self-hosted Tin Hat Server sends no email, so its owner hands out a new password and you choose your own
// here). Tin Hat Web builds the same dialog to the same contract: the title, the subtitle ("Your other devices stay
// signed in." or the current error in its place), the three password fields, the hint ("At least 8 characters" →
// "Those two don't match."), "Change password" (enabled only when the current one is filled in, the new one has at
// least 8 characters and is typed the same twice) and "Cancel"; the words of every outcome.
//
// The request is settings.changePassword (settings.rs → password.rs: standard Matrix user-interactive auth, the other
// devices stay signed in); its answer comes back through the poll below (passwordAnswers counts them). Success closes
// the dialog and `changed(text)` hands the toast to the window; a wrong current password clears that field. No password
// stays in a field behind a closed dialog.
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the settings.* tokens (docs/MIGRATING-THEME.md)

Controls.Dialog {
    id: dlg
    property var app: null
    // success: the words for the window's toast ("Password changed. Your other devices stay signed in.")
    signal changed(string text)

    // the subtitle's error ("" = "Your other devices stay signed in.")
    property string error: ""
    property bool busy: false
    property int _answers: 0
    readonly property bool mismatch: againField.text.length > 0 && againField.text !== newField.text
    readonly property bool ready: !dlg.busy && currentField.text.length > 0 && newField.text.length >= 8
                                  && newField.text === againField.text

    title: "Change password"
    modal: true
    parent: Controls.Overlay.overlay
    anchors.centerIn: parent
    closePolicy: dlg.busy ? Controls.Popup.NoAutoClose : Controls.Popup.CloseOnEscape
    // a stable content width (never the dialog's negotiated one: the wrapping subtitle would loop)
    readonly property real boxWidth: Math.min(400, (parent ? parent.width : 400) - Kirigami.Units.gridUnit * 2)
    implicitWidth: dlg.boxWidth

    function clearFields() { currentField.text = ""; newField.text = ""; againField.text = "" }
    onAboutToShow: {
        dlg.clearFields()
        dlg.error = ""
        dlg.busy = false
        dlg._answers = dlg.app ? dlg.app.settings.passwordAnswers : 0
    }
    onOpened: currentField.forceActiveFocus()
    onClosed: dlg.clearFields()

    function submit() {
        if (!dlg.ready || !dlg.app) return
        var s = dlg.app.settings
        dlg.error = ""
        dlg._answers = s.passwordAnswers
        s.changePassword(currentField.text, newField.text)
        dlg.busy = s.passwordBusy   // false = the controller didn't start one (signed out meanwhile)
    }
    // the answer (settings.rs): once per passwordAnswers step
    Timer {
        interval: 150
        repeat: true
        running: dlg.visible && dlg.busy
        onTriggered: {
            var s = dlg.app.settings
            if (s.passwordAnswers === dlg._answers) return
            dlg._answers = s.passwordAnswers
            dlg.busy = false
            var out = String(s.passwordOutcome)
            var text = String(s.passwordText)
            if (out === "changed") {
                dlg.clearFields()
                dlg.close()
                dlg.changed(text)
            } else if (out === "wrong") {
                dlg.error = text
                currentField.text = ""
                currentField.forceActiveFocus()
            } else {
                dlg.error = text
            }
        }
    }

    ColumnLayout {
        width: dlg.boxWidth - dlg.leftPadding - dlg.rightPadding
        spacing: 8
        Controls.Label {
            Layout.fillWidth: true
            Layout.bottomMargin: 4
            text: dlg.error.length > 0 ? dlg.error : "Your other devices stay signed in."
            color: dlg.error.length > 0 ? Th.settingsErrorFg : Th.settingsHintFg
            font.pixelSize: Math.round(13 * Th.textScale)
            wrapMode: Text.Wrap
            Accessible.role: dlg.error.length > 0 ? Accessible.AlertMessage : Accessible.StaticText
        }
        ThTextField {
            id: currentField
            Layout.fillWidth: true
            enabled: !dlg.busy
            placeholderText: "Current password"
            echoMode: TextInput.Password
            Accessible.name: "Current password"
            onAccepted: newField.forceActiveFocus()
        }
        ThTextField {
            id: newField
            Layout.fillWidth: true
            enabled: !dlg.busy
            placeholderText: "New password"
            echoMode: TextInput.Password
            Accessible.name: "New password"
            onAccepted: againField.forceActiveFocus()
        }
        ThTextField {
            id: againField
            Layout.fillWidth: true
            enabled: !dlg.busy
            placeholderText: "New password again"
            echoMode: TextInput.Password
            Accessible.name: "New password again"
            onAccepted: dlg.submit()
        }
        Controls.Label {
            Layout.fillWidth: true
            text: dlg.mismatch ? "Those two don't match." : "At least 8 characters"
            color: dlg.mismatch ? Th.settingsErrorFg : Th.settingsHintFg
            font.pixelSize: Math.round(12 * Th.textScale)
            wrapMode: Text.Wrap
        }
    }

    // the two buttons in a plain row: a DialogButtonBox drew the primary one unhighlighted (and an Ok/Accept button
    // would close the dialog before the answer)
    footer: Item {
        implicitHeight: footRow.implicitHeight + 24
        RowLayout {
            id: footRow
            anchors.fill: parent
            anchors.leftMargin: 12
            anchors.rightMargin: 12
            spacing: 8
            Item { Layout.fillWidth: true }
            ThButton {
                app: dlg.app
                highlighted: true
                text: "Change password"
                enabled: dlg.ready
                onClicked: dlg.submit()
            }
            ThButton {
                app: dlg.app
                text: "Cancel"
                enabled: !dlg.busy
                onClicked: dlg.close()
            }
        }
    }
}
