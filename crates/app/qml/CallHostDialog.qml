// CallHostDialog.qml — calls round M1 (share calls-round/DESIGN.md §2.4): "Join a call hosted elsewhere?". Shown when
// the call in this room runs on another server's call service (callctl.callHostPrompt, set by call_ctl.rs instead of a
// join failure). Nothing has been requested from that service yet: Join asks it once for this call, "Always trust"
// also saves the host to account data dev.tinhat.call_hosts (synced; Settings › Calls lists it), Cancel sends nothing.
// The words come from Rust (callrules::host_sheet_words) so they stay byte-identical to Tin Hat Web's sheet.
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th

Controls.Dialog {
    id: dlg
    property var app: null
    // the prompt JSON: {room, host, serviceUrl, advertiser, advertiserName, words:{title, body, join, always, cancel}}
    property string promptJson: ""
    readonly property var prompt: {
        try { return dlg.promptJson.length > 0 ? JSON.parse(dlg.promptJson) : ({}) } catch (e) { return ({}) }
    }
    readonly property var words: dlg.prompt.words || ({})
    property bool _answered: false

    objectName: "callHostDialog"
    title: LinkGate.plain(dlg.words.title || "")
    modal: true
    parent: Controls.Overlay.overlay
    anchors.centerIn: parent
    closePolicy: Controls.Popup.CloseOnEscape
    readonly property real boxWidth: Math.min(460, (parent ? parent.width : 460) - Kirigami.Units.gridUnit * 2)
    implicitWidth: dlg.boxWidth

    onPromptJsonChanged: {
        if (dlg.promptJson.length > 0) { dlg._answered = false; dlg.open() }
        else if (dlg.visible) { dlg._answered = true; dlg.close() }
    }
    function answer(always) {
        dlg._answered = true
        var p = dlg.prompt
        dlg.close()
        dlg.app.callctl.confirmCallHost(String(p.room || ""), String(p.serviceUrl || ""), always)
    }
    onClosed: if (!dlg._answered) { dlg._answered = true; dlg.app.callctl.cancelCallHost() }

    // No default action (as the web's sheet): Enter / Return never answers — a stray key press must not send a sign-in
    // proof to another server. Join, Always trust and Cancel need a click (or Space on a focused button); Esc = Cancel.
    onOpened: guard.forceActiveFocus()
    ColumnLayout {
        id: guard
        width: dlg.boxWidth - dlg.leftPadding - dlg.rightPadding
        spacing: 8
        focus: true
        Keys.onReturnPressed: (event) => { event.accepted = true }
        Keys.onEnterPressed: (event) => { event.accepted = true }
        Controls.Label {
            objectName: "callHostBody"
            textFormat: Text.PlainText   // host and names come from other people's state events: never markup
            Layout.fillWidth: true
            text: dlg.words.body || ""
            color: Th.settingsHintFg
            font.pixelSize: Math.round(13 * Th.textScale)
            wrapMode: Text.Wrap
        }
    }

    // the buttons wrap onto more lines when "Always trust {host}" doesn't fit (long hosts, narrow windows, big text)
    footer: Item {
        implicitHeight: footFlow.implicitHeight + 24
        Flow {
            id: footFlow
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.verticalCenter: parent.verticalCenter
            anchors.leftMargin: 12
            anchors.rightMargin: 12
            layoutDirection: Qt.RightToLeft
            spacing: 8
            ThButton {
                objectName: "callHostCancel"
                app: dlg.app
                width: Math.min(implicitWidth, footFlow.width)
                text: dlg.words.cancel || ""
                onClicked: dlg.close()
            }
            ThButton {
                objectName: "callHostAlways"
                app: dlg.app
                width: Math.min(implicitWidth, footFlow.width)
                clip: true
                text: dlg.words.always || ""
                Accessible.name: text
                onClicked: dlg.answer(true)
            }
            ThButton {
                objectName: "callHostJoin"
                app: dlg.app
                highlighted: true
                width: Math.min(implicitWidth, footFlow.width)
                text: dlg.words.join || ""
                onClicked: dlg.answer(false)
            }
        }
    }
}
