// TrustDeviceDialog.qml — Settings → Devices & security → the phone's "Trust…" (trust_device.rs; Tin Hat Web builds the
// same sheet: features/settings/panes/SecurityPane.tsx TrustSheet). It shows the phone device's fingerprint as the
// server has it, then "Trust" runs the contract's steps 2–5 for that device and that key only. The words come from
// settings.trustWords(name) (the web's TRUST_WORDS, byte for byte); the answers through trustAnswers. "signed" /
// "already" close it and hand the toast to the window; "keys-locked" / "no-cross-signing" offer "Verify this device…".
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th

Controls.Dialog {
    id: dlg
    property var app: null
    property string deviceId: ""
    property string deviceName: ""
    signal done(string text)
    signal verifyRequested()

    property string key: ""
    property string note: ""
    property bool needsVerify: false
    property bool busy: false
    property int _answers: 0
    readonly property var words: dlg.app ? JSON.parse(String(dlg.app.settings.trustWords(dlg.deviceName))) : ({})

    title: LinkGate.plain(dlg.words.title || "")   // the header draws AutoText (textformat-lint)
    modal: true
    parent: Controls.Overlay.overlay
    anchors.centerIn: parent
    closePolicy: dlg.busy ? Controls.Popup.NoAutoClose : Controls.Popup.CloseOnEscape
    readonly property real boxWidth: Math.min(460, (parent ? parent.width : 460) - Kirigami.Units.gridUnit * 2)
    implicitWidth: dlg.boxWidth

    function show(id, name) {
        dlg.deviceId = id
        dlg.deviceName = name
        dlg.key = ""
        dlg.note = ""
        dlg.needsVerify = false
        var s = dlg.app.settings
        dlg._answers = s.trustAnswers
        s.trustLoad(id, name)
        dlg.busy = true
        dlg.open()
    }
    function trust() {
        if (dlg.busy || dlg.key.length === 0) return
        var s = dlg.app.settings
        dlg.note = ""
        dlg._answers = s.trustAnswers
        dlg.busy = true   // trustDevice always answers once (trustAnswerFor = this device)
        s.trustDevice(dlg.deviceId, dlg.key, dlg.deviceName)
    }
    Timer {
        interval: 150
        repeat: true
        running: dlg.visible && dlg.busy
        onTriggered: {
            var s = dlg.app.settings
            if (s.trustAnswers === dlg._answers) return
            dlg._answers = s.trustAnswers
            // a late answer about another device (this dialog was closed and opened on another row meanwhile)
            if (String(s.trustAnswerFor) !== dlg.deviceId) return
            dlg.busy = false
            var out = String(s.trustOutcome)
            if (out === "loaded") { dlg.key = String(s.trustKey); return }
            if (out === "signed" || out === "already") { dlg.close(); dlg.done(String(s.trustText)); return }
            dlg.note = String(s.trustText)
            dlg.needsVerify = out === "keys-locked" || out === "no-cross-signing"
        }
    }

    ColumnLayout {
        width: dlg.boxWidth - dlg.leftPadding - dlg.rightPadding
        spacing: 8
        Controls.Label {
            textFormat: Text.PlainText   // the device name is the server's text: never markup
            Layout.fillWidth: true
            text: dlg.words.body || ""
            color: Th.settingsHintFg
            font.pixelSize: Math.round(13 * Th.textScale)
            wrapMode: Text.Wrap
        }
        Controls.Label {
            objectName: "trustFingerprint"
            textFormat: Text.PlainText
            Layout.fillWidth: true
            text: dlg.key.length > 0 ? String(dlg.app.settings.trustFingerprint(dlg.key)) : (dlg.busy ? "…" : "—")
            font.family: Th.fontMono
            font.pixelSize: Math.round(13 * Th.textScale)
            wrapMode: Text.WrapAnywhere
            Accessible.name: "Fingerprint " + text
        }
        Controls.Label {
            objectName: "trustNote"
            visible: dlg.note.length > 0
            textFormat: Text.PlainText
            Layout.fillWidth: true
            text: dlg.note
            color: Th.settingsErrorFg
            font.pixelSize: Math.round(13 * Th.textScale)
            wrapMode: Text.Wrap
            Accessible.role: Accessible.AlertMessage
        }
    }

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
                visible: !dlg.needsVerify
                text: dlg.busy && dlg.key.length > 0 ? (dlg.words.working || "") : (dlg.words.button || "")
                enabled: !dlg.busy && dlg.key.length > 0
                onClicked: dlg.trust()
            }
            ThButton {
                app: dlg.app
                highlighted: true
                visible: dlg.needsVerify
                text: dlg.words.verify || ""
                onClicked: { dlg.close(); dlg.verifyRequested() }
            }
            ThButton {
                app: dlg.app
                text: dlg.words.cancel || ""
                enabled: !dlg.busy || dlg.key.length === 0
                onClicked: dlg.close()
            }
        }
    }
}
