// GameServerPower.qml — the power flow of the Game servers widget and of Up next's "Start server" (docs/GAMESERVERS.md
// §3.4–§3.5, §4; the web's gameservers/parts.tsx usePower): run(action, id) previews the server's rules on the VIEW it
// holds — refused / memory → a toast with the server's words; a third game → the switch dialog ("Stop X and start Y",
// one button per running game); else the confirmation of confirmFor (players on / LAN only) or straight to the
// controller's power(). The server decides again on every request; a refusal comes back as the controller's toast.
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the active theme (docs/MIGRATING-THEME.md)

Item {
    id: pw
    property var app
    property string viewText: ""            // the controller's statusJson (or a sample's view)
    property bool preview: false
    readonly property var ctl: pw.app ? pw.app.gameServersCtl : null
    readonly property var view: { try { return pw.viewText.length > 0 ? JSON.parse(pw.viewText) : null } catch (e) { return null } }
    visible: false
    width: 0; height: 0

    function serverOf(id) {
        var list = pw.view && pw.view.servers ? pw.view.servers : []
        for (var i = 0; i < list.length; ++i) if (list[i].id === id) return list[i]
        return null
    }
    function toast(t) { if (pw.app && pw.app.showPassiveNotification) pw.app.showPassiveNotification(t, 4000) }
    function send(action, id, first) { if (pw.ctl && !pw.preview) pw.ctl.power(action, id, first || "") }
    function run(action, id) {
        if (!pw.ctl || pw.preview || !pw.view) return
        if (action === "start") {
            var p = {}
            try { p = JSON.parse(String(pw.ctl.planStart(pw.viewText, id))) } catch (e) { p = { kind: "unknown" } }
            if (p.kind === "unknown") { pw.toast("No such game server"); return }
            if (p.kind === "refused" || p.kind === "memory") { pw.toast(String(p.text)); return }
            if (p.kind === "cap") { switchDlg.openFor(id, p.running || []); return }
        }
        var c = String(pw.ctl.confirmFor(pw.viewText, action, id, ""))
        if (c.length === 0) { pw.send(action, id, ""); return }
        var cf = JSON.parse(c)
        confirm.pending = { action: action, id: id }
        confirm.dialogTitle = cf.title
        confirm.message = cf.body
        confirm.confirmText = cf.confirm
        confirm.destructive = cf.destructive === true
        confirm.open()
    }

    ConfirmDialog {
        id: confirm
        app: pw.app
        property var pending: null
        onConfirmed: if (confirm.pending) pw.send(confirm.pending.action, confirm.pending.id, "")
    }

    // "Two games are already running": one button per running game — "Stop X and start Y" (+ its players line)
    Controls.Dialog {
        id: switchDlg
        property string target: ""
        property var running: []
        readonly property var tsv: pw.serverOf(switchDlg.target)
        function openFor(id, list) { switchDlg.target = id; switchDlg.running = list; switchDlg.open() }
        title: "Two games are already running"
        modal: true
        parent: Controls.Overlay.overlay
        anchors.centerIn: parent
        readonly property real boxWidth: Math.min(420, (parent ? parent.width : 420) - Kirigami.Units.gridUnit * 2)
        implicitWidth: switchDlg.boxWidth
        standardButtons: Controls.Dialog.Cancel
        contentItem: ColumnLayout {
            spacing: 10
            Controls.Label {
                Layout.fillWidth: true
                text: switchDlg.tsv ? "Stop one to start " + switchDlg.tsv.name + ":" : ""
                wrapMode: Text.Wrap
                textFormat: Text.PlainText
            }
            Repeater {
                model: switchDlg.running
                delegate: ColumnLayout {
                    id: opt
                    required property var modelData
                    readonly property var fsv: pw.serverOf(String(opt.modelData))
                    readonly property int players: opt.fsv && opt.fsv.state === "running" && opt.fsv.players ? opt.fsv.players.count : 0
                    Layout.fillWidth: true
                    spacing: 3
                    ThButton {
                        Layout.fillWidth: true
                        app: pw.app
                        text: {
                            var c = pw.ctl ? String(pw.ctl.confirmFor(pw.viewText, "start", switchDlg.target, String(opt.modelData))) : ""
                            try { return c.length > 0 ? JSON.parse(c).confirm : "" } catch (e) { return "" }
                        }
                        icon.name: "media-playback-stop-symbolic"
                        onClicked: { switchDlg.close(); pw.send("start", switchDlg.target, String(opt.modelData)) }
                    }
                    WidgetText {
                        visible: opt.players > 0
                        Layout.fillWidth: true
                        role: "meta"
                        text: opt.fsv ? (opt.players === 1 ? "1 person is" : opt.players + " people are") + " on " + opt.fsv.name : ""
                        color: Th.widgetMutedFg
                    }
                }
            }
            WidgetText {
                visible: !!switchDlg.tsv && switchDlg.tsv.outside !== true
                Layout.fillWidth: true
                role: "meta"
                wrapMode: Text.Wrap
                maximumLineCount: 3
                elide: Text.ElideNone
                text: "Ports closed — LAN only: people outside this network can't join (ask the server's owner to open them)."
                color: Th.widgetMutedFg
            }
        }
    }
}
