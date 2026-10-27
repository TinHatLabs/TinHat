// DeleteAccountDialog.qml — Settings → Devices & security → Your account → "Delete my account" → "Delete…" (account
// safety; Tin Hat Web's features/safety/sections.tsx DeleteAccountRow is the twin): deleteTitle · the six lines (the
// shared words, this account's id filled in) · "Enter your account password to confirm." · the password · [Delete my
// account] (red, enabled with a password) [Cancel]. settings.deleteAccount (safety.rs: POST /account/deactivate with
// erase, the password step); "wrong" clears the field; "deleted" closes the dialog and signs out here
// (loginController.signOutDeleted: the sign-in card says "Your account has been deleted."). No password stays in a field
// behind a closed dialog.
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app

Controls.Dialog {
    id: dlg
    property var app: null
    signal deleted()
    readonly property var w: {
        try { return (JSON.parse(String(dlg.app ? dlg.app.settings.safetyWords() : "{}")).words) || ({}) } catch (e) { return ({}) }
    }
    property var lines: []
    property string error: ""
    property bool busy: false
    property int _answers: 0
    readonly property bool ready: !dlg.busy && pw.text.length > 0

    title: LinkGate.plain(String(dlg.w.deleteTitle || ""))
    modal: true
    parent: Controls.Overlay.overlay
    anchors.centerIn: parent
    closePolicy: dlg.busy ? Controls.Popup.NoAutoClose : Controls.Popup.CloseOnEscape
    readonly property real boxWidth: Math.min(460, (parent ? parent.width : 460) - Kirigami.Units.gridUnit * 2)
    implicitWidth: dlg.boxWidth

    onAboutToShow: {
        pw.text = ""; dlg.error = ""; dlg.busy = false
        try { dlg.lines = JSON.parse(String(dlg.app.settings.deleteLinesJson())) } catch (e) { dlg.lines = [] }
        dlg._answers = dlg.app ? dlg.app.settings.deleteAnswers : 0
    }
    onOpened: pw.forceActiveFocus()
    onClosed: pw.text = ""

    function submit() {
        if (!dlg.ready || !dlg.app) return
        var s = dlg.app.settings
        dlg.error = ""
        dlg._answers = s.deleteAnswers
        s.deleteAccount(pw.text)
        dlg.busy = s.deleteBusy
    }
    Timer {
        interval: 150; repeat: true
        running: dlg.visible && dlg.busy
        onTriggered: {
            var s = dlg.app.settings
            if (s.deleteAnswers === dlg._answers) return
            dlg._answers = s.deleteAnswers
            dlg.busy = false
            var out = String(s.deleteOutcome)
            if (out === "deleted") {
                // (the sync loop may have signed out already: the deletion killed the token; deleted() still runs —
                // signOutDeleted then only sets the sign-in card's words)
                pw.text = ""
                dlg.close()
                dlg.deleted()
            } else {
                dlg.error = String(s.deleteText)
                if (out === "wrong") { pw.text = ""; pw.forceActiveFocus() }
            }
        }
    }

    ColumnLayout {
        width: dlg.boxWidth - dlg.leftPadding - dlg.rightPadding
        spacing: 8
        Repeater {
            model: dlg.lines
            RowLayout {
                required property var modelData
                Layout.fillWidth: true
                spacing: 6
                Controls.Label { text: "•"; Layout.alignment: Qt.AlignTop }
                Controls.Label {
                    textFormat: Text.PlainText
                    Layout.fillWidth: true
                    text: String(modelData)
                    wrapMode: Text.Wrap
                }
            }
        }
        Controls.Label {
            textFormat: Text.PlainText
            Layout.fillWidth: true
            Layout.topMargin: 4
            text: String(dlg.w.deletePassword || "")
            color: Th.settingsHintFg
            wrapMode: Text.Wrap
        }
        ThTextField {
            id: pw
            Layout.fillWidth: true
            enabled: !dlg.busy
            placeholderText: "Account password"
            echoMode: TextInput.Password
            Accessible.name: "Account password"
            onAccepted: dlg.submit()
        }
        Controls.Label {
            textFormat: Text.PlainText
            Layout.fillWidth: true
            visible: dlg.error.length > 0
            text: dlg.error
            color: Th.settingsErrorFg
            wrapMode: Text.Wrap
            Accessible.role: Accessible.AlertMessage
        }
    }
    footer: Item {
        implicitHeight: delRow.implicitHeight + 24
        RowLayout {
            id: delRow
            anchors.fill: parent
            anchors.leftMargin: 12
            anchors.rightMargin: 12
            spacing: 8
            Item { Layout.fillWidth: true }
            ThButton {
                app: dlg.app
                danger: true
                text: String(dlg.w.deleteConfirm || "Delete my account")
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
