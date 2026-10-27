// SafetyHost.qml — account safety's dialogs (safety.rs; Tin Hat Web's features/safety/SafetyHost.tsx is the twin, same
// words from the shared safety words, same order):
//   Ignore   "Ignore {name}?" · ignoreText · [Ignore] (destructive) [Cancel]
//   Report   "Report {name}" / "Report this message" · reportIntro · "What's wrong?" + the five reasons · the details
//            (≤ 1000) · "Also ignore them" (someone else, not yet ignored) · [Send report] [Cancel]
// Every person menu and the message menu open them through root.safety (Main.qml): openIgnore / toggleIgnore /
// openReport. The answers come back through the settings controller's counters (ignoreAnswers, reportAnswers).
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app

Item {
    id: host
    property var app: null
    readonly property var s: host.app ? host.app.settings : null
    readonly property var sw: {
        try { return JSON.parse(String(host.s ? host.s.safetyWords() : "{}")) } catch (e) { return ({}) }
    }
    readonly property var w: host.sw.words || ({})
    readonly property var categories: host.sw.categories || []
    readonly property var ignored: {
        try { return JSON.parse(String(host.s && host.s.ignoredJson ? host.s.ignoredJson : "[]")) } catch (e) { return [] }
    }
    function fill(t, vars) {
        return String(t || "").replace(/\{(\w+)\}/g, function(m, k) { return vars.hasOwnProperty(k) ? String(vars[k]) : m })
    }
    function isIgnored(mxid) { return host.ignored.indexOf(String(mxid)) >= 0 }
    function isMe(mxid) { return !!host.app && String(mxid) === String(host.app.mUserId || "") }
    function toast(t, ms) { if (host.app && t && String(t).length > 0) host.app.showPassiveNotification(LinkGate.plain(String(t)), ms || 4000) }

    // ---- the run path -----------------------------------------------------------------------------------------------
    function openIgnore(mxid, name) {
        if (host.isIgnored(mxid)) { host.setIgnored(mxid, name, false); return }
        var why = String(host.s.ignoreRefusal(mxid))
        if (why.length > 0) { host.toast(why); return }
        ignoreDlg.mxid = String(mxid); ignoreDlg.name = String(name)
        ignoreDlg.open()
    }
    function toggleIgnore(mxid, name) { host.openIgnore(mxid, name) }
    property int _ignoreAnswers: 0
    function setIgnored(mxid, name, on) {
        host._ignoreAnswers = host.s.ignoreAnswers
        host.s.setIgnored(String(mxid), String(name), on)
        ignorePoll.restart()
    }
    Timer {
        id: ignorePoll
        interval: 150; repeat: true
        property int waited: 0
        onTriggered: {
            if (host.s.ignoreAnswers === host._ignoreAnswers) { if (++waited > 200) stop(); return }
            stop(); waited = 0
            host._ignoreAnswers = host.s.ignoreAnswers
            var err = String(host.s.ignoreText)
            host.toast(err.length > 0 ? err : String(host.s.ignoreToast), err.length > 0 ? 5000 : 4000)
        }
    }
    function openReport(user, room, event, name) {
        reportDlg.user = String(user); reportDlg.room = String(room || ""); reportDlg.eventId = String(event || "")
        reportDlg.name = String(name || user)
        reportDlg.open()
    }

    // ---- lab self-test (--tinhat-safety-test=@someone:lab.server, with TINHAT_GUI_LOGIN on a THROWAWAY lab account) ----
    // Drives the real controller paths once signed in and logs "[safety-test] PASS|FAIL <step>": ignore + the list, the
    // dialogs on screen (for screenshots), a report of them (no Tin Hat API on a plain lab Synapse: the Matrix
    // fallback), stop ignoring, a wrong-password delete, then the real delete and the sign-out with its words.
    // scripts/account-safety-gate.sh checks the server side. Never on a real server: it deletes the account.
    readonly property string _testTarget: (host.app && host.app._hookArg("--tinhat-safety-test") !== null) ? String(host.app._hookArg("--tinhat-safety-test")) : ""
    // the account the gate signed in as (--tinhat-safety-test-self=<mxid>): the self-test runs only as exactly that account.
    // No account names in the app itself (the release binary scan refuses test-account names); the gate script checks
    // that it is a throwaway lab account.
    readonly property string _testSelf: (host.app && host.app._hookArg("--tinhat-safety-test-self") !== null) ? String(host.app._hookArg("--tinhat-safety-test-self")) : ""
    property int _step: 0
    property int _mark: 0
    function _log(ok, what) { console.log("[safety-test] " + (ok ? "PASS " : "FAIL ") + what) }
    Timer {
        id: selfTest
        interval: 1500; repeat: true
        // refuses anything but a loopback lab server and exactly the account the gate named (it DELETES the account it runs as)
        readonly property bool labOnly: !!host.app && /^http:\/\/127\.0\.0\.1:\d+\/?$/.test(String(host.app.login.homeserver || ""))
                                        && host._testSelf.length > 0 && String(host.app.mUserId || "") === host._testSelf
        running: host._testTarget.length > 0 && !!host.app && (host._step >= 11 || (host.app.mLoggedIn && selfTest.labOnly)) && host._step < 13
        onTriggered: {
            var s = host.s, t = host._testTarget
            var pw = String(host.app.login.autoLogin || ""); pw = pw.substring(pw.indexOf(":") + 1)
            switch (host._step) {
            case 0: if (!selfTest.labOnly) { console.log("[safety-test] REFUSED: not a lab account on a loopback server"); host._step = 13; return }
                host._step = 1; return   // let the first sync land
            case 1: host._mark = s.ignoreAnswers; s.setIgnored(t, "them", true); host._step = 2; return
            case 2: if (s.ignoreAnswers === host._mark) return
                host._log(String(s.ignoreText).length === 0 && host.isIgnored(t), "ignore → on the list (" + s.ignoredJson + ")")
                host._log(String(s.ignoreToast) === host.fill(host.w.ignoredToast, { name: "them" }), "the toast's words")
                console.log("[safety-test] SHOT ignored"); host._step = 3; return
            case 3: host.openIgnore("@someone-else:" + t.split(":").slice(1).join(":"), "someone-else"); console.log("[safety-test] SHOT ignore-dialog"); host._step = 4; return
            case 4: ignoreDlg.close(); host.openReport(t, "", "", "them"); console.log("[safety-test] SHOT report-dialog"); host._step = 5; return
            case 5: reportDlg.close()
                host._log(!s.reportReady("", "") && s.reportReady("spam", "") && !s.reportReady("other", " "), "reportReady rules")
                host._mark = s.reportAnswers; s.sendReport(t, "", "", "harassment", "desktop lab " + Date.now()); host._step = 6; return
            case 6: if (s.reportAnswers === host._mark) return
                host._log(s.reportSent === true, "report of a person reached the server (Matrix fallback)")
                host._mark = s.ignoreAnswers; s.setIgnored(t, "them", false); host._step = 7; return
            case 7: if (s.ignoreAnswers === host._mark) return
                host._log(!host.isIgnored(t), "stop ignoring → off the list (" + s.ignoredJson + ")")
                host._log(String(s.deleteRefusal()).length === 0, "an ordinary account may be deleted")
                host._log(String(s.deleteLinesJson()).indexOf(String(host.app.mUserId)) > 0, "the explainer names this account")
                host.app.settingsSheet.openAt("security"); host._step = 8; return
            case 8: console.log("[safety-test] SHOT settings-security"); host.app.settingsSheet.openAt("privacy"); host._step = 9; return
            case 9: console.log("[safety-test] SHOT settings-privacy"); host.app.settingsSheet.close()
                host._mark = s.deleteAnswers; s.deleteAccount("not-the-password"); host._step = 10; return
            case 10: if (s.deleteAnswers === host._mark) return
                host._log(String(s.deleteOutcome) === "wrong" && String(s.deleteText) === String(host.w.deleteWrong), "a wrong password: " + s.deleteText)
                host._mark = s.deleteAnswers; s.deleteAccount(pw); host._step = 11; return
            case 11: if (s.deleteAnswers === host._mark) return
                host._log(String(s.deleteOutcome) === "deleted", "deleted: " + s.deleteOutcome)
                host.app.login.signOutDeleted(); host._step = 12; return
            case 12: if (host.app.mLoggedIn) return
                host._log(String(host.app.login.status) === String(host.w.deleteDone), "signed out: " + host.app.login.status)
                console.log("[safety-test] SHOT signed-out"); console.log("[safety-test] DONE"); host._step = 13; return
            }
        }
    }
    ConfirmDialog {
        id: ignoreDlg
        app: host.app
        property string mxid: ""
        property string name: ""
        dialogTitle: host.fill(host.w.ignoreTitle, { name: ignoreDlg.name })
        message: String(host.w.ignoreText || "")
        confirmText: String(host.w.ignoreConfirm || "Ignore")
        destructive: true
        onConfirmed: host.setIgnored(ignoreDlg.mxid, ignoreDlg.name, true)
    }

    Controls.Dialog {
        id: reportDlg
        property string user: ""
        property string room: ""
        property string eventId: ""
        property string name: ""
        property string category: ""
        property string error: ""
        property bool busy: false
        property int _answers: 0
        readonly property bool canIgnore: !host.isMe(reportDlg.user) && !host.isIgnored(reportDlg.user)
        readonly property bool ready: !reportDlg.busy && !!host.s && host.s.reportReady(reportDlg.category, details.text)
        title: LinkGate.plain(reportDlg.eventId.length > 0 ? String(host.w.reportMessageTitle || "") : host.fill(host.w.reportPersonTitle, { name: reportDlg.name }))
        modal: true
        parent: Controls.Overlay.overlay
        anchors.centerIn: parent
        closePolicy: reportDlg.busy ? Controls.Popup.NoAutoClose : Controls.Popup.CloseOnEscape
        readonly property real boxWidth: Math.min(440, (parent ? parent.width : 440) - Kirigami.Units.gridUnit * 2)
        implicitWidth: reportDlg.boxWidth
        onAboutToShow: {
            reportDlg.category = ""; details.text = ""; alsoIgnore.checked = false
            reportDlg.error = ""; reportDlg.busy = false
            reportDlg._answers = host.s ? host.s.reportAnswers : 0
        }
        property string _from: ""   // the account that sent it (an "Also ignore" never lands on the next one)
        function send() {
            if (!reportDlg.ready) return
            reportDlg._from = String(host.app.mUserId || "")
            reportDlg.error = ""
            reportDlg._answers = host.s.reportAnswers
            host.s.sendReport(reportDlg.user, reportDlg.room, reportDlg.eventId, reportDlg.category, details.text)
            reportDlg.busy = host.s.reportBusy
        }
        Timer {
            interval: 150; repeat: true
            running: reportDlg.visible && reportDlg.busy
            onTriggered: {
                if (host.s.reportAnswers === reportDlg._answers) return
                reportDlg._answers = host.s.reportAnswers
                reportDlg.busy = false
                if (!host.s.reportSent) { reportDlg.error = String(host.w.reportFailed || ""); return }
                var ign = alsoIgnore.checked && reportDlg.canIgnore && String(host.app.mUserId || "") === reportDlg._from
                var who = reportDlg.user, nm = reportDlg.name
                reportDlg.close()
                host.toast(host.w.reportSent)
                if (ign) host.setIgnored(who, nm, true)
            }
        }
        ColumnLayout {
            width: reportDlg.boxWidth - reportDlg.leftPadding - reportDlg.rightPadding
            spacing: 8
            Controls.Label {
                textFormat: Text.PlainText
                Layout.fillWidth: true
                text: host.fill(host.w.reportIntro, { server: host.app ? host.app.myServer() : "" })
                color: Th.settingsHintFg
                font.pixelSize: Math.round(13 * Th.textScale)
                wrapMode: Text.Wrap
            }
            Controls.Label {
                textFormat: Text.PlainText
                text: String(host.w.reportWhat || "")
                font.bold: true
            }
            Repeater {
                model: host.categories
                ThRadioButton {
                    required property var modelData
                    text: String(modelData[1])
                    checked: reportDlg.category === String(modelData[0])
                    enabled: !reportDlg.busy
                    onToggled: if (checked) reportDlg.category = String(modelData[0])
                }
            }
            ThTextArea {
                id: details
                Layout.fillWidth: true
                Layout.preferredHeight: 64
                enabled: !reportDlg.busy
                placeholderText: String(host.w.reportDetails || "")
                Accessible.name: String(host.w.reportDetails || "")
                onTextChanged: if (text.length > (host.sw.detailsMax || 1000)) text = text.substring(0, host.sw.detailsMax || 1000)
            }
            ThCheckBox {
                id: alsoIgnore
                visible: reportDlg.canIgnore
                text: String(host.w.reportAlsoIgnore || "")
                enabled: !reportDlg.busy
            }
            Controls.Label {
                textFormat: Text.PlainText
                Layout.fillWidth: true
                visible: reportDlg.error.length > 0
                text: reportDlg.error
                color: Th.settingsErrorFg
                wrapMode: Text.Wrap
                Accessible.role: Accessible.AlertMessage
            }
        }
        footer: Item {
            implicitHeight: repRow.implicitHeight + 24
            RowLayout {
                id: repRow
                anchors.fill: parent
                anchors.leftMargin: 12
                anchors.rightMargin: 12
                spacing: 8
                Item { Layout.fillWidth: true }
                ThButton {
                    app: host.app
                    highlighted: true
                    text: String(host.w.reportSend || "Send report")
                    enabled: reportDlg.ready
                    onClicked: reportDlg.send()
                }
                ThButton {
                    app: host.app
                    text: "Cancel"
                    enabled: !reportDlg.busy
                    onClicked: reportDlg.close()
                }
            }
        }
    }
}
