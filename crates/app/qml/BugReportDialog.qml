// BugReportDialog — in-app problem report (room-list bug button): free-text note +
// the LIVE session log ship immediately via settings.submitBugReport (the caller
// wires onSubmitted). Cloned from ConfirmDialog's skeleton for consistency.
// (Historical note: the 0.3.5 no-boot was once blamed on inline dialogs. It was
// not — ThNav was missing a signal declaration. Inline dialogs are fine; these
// live in component files for readability, not for safety.)
import QtQuick
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the bugReport.* tokens

Controls.Dialog {
    id: dlg
    property var app: null
    FallbackTheme { id: fallbackTheme }
    readonly property var theme: app || fallbackTheme

    signal submitted(string text)
    // named clearForm, NOT reset: "reset" collides with QQC2 Dialog's built-in
    // reset() signal (qt.qml.invalidOverride at every load)
    function clearForm() { noteArea.text = "" }
    // 10.1 (TH-PKG-06): a failed send keeps the note — the next open shows it again instead of an
    // empty box, so a flaky network can't eat what you just typed.
    property bool keepNote: false   // set by Main.qml from settings.bugReportStatus

    title: "Report a problem"
    modal: true
    parent: Controls.Overlay.overlay
    anchors.centerIn: parent
    // 9.2: the dialog carries the app palette itself. Under a plain QQC2 style (Windows Basic,
    // the test rig without a KDE colour scheme) it came up as a WHITE box with grey
    // buttons in the middle of a dark app.
    palette.window: Th.bugReportBg
    palette.windowText: Th.bugReportFg
    palette.base: Th.bugReportNoteBg
    palette.text: Th.bugReportNoteFg
    palette.placeholderText: Th.bugReportNotePlaceholder
    palette.button: Th.bugReportButtonBg
    palette.buttonText: Th.bugReportButtonFg
    palette.highlight: Th.bugReportHighlightBg
    palette.highlightedText: Th.bugReportHighlightFg
    background: Rectangle {
        color: Th.bugReportBg
        radius: dlg.theme.radiusLg
        border.width: 1
        border.color: Th.bugReportBorder
    }
    // Same anti-cycle rule as ConfirmDialog: content width is a stable constant, NOT
    // the dialog's own negotiated width.
    readonly property real boxWidth: Math.min(520, (parent ? parent.width : 520) - Kirigami.Units.gridUnit * 2)
    implicitWidth: dlg.boxWidth
    standardButtons: Controls.Dialog.Ok | Controls.Dialog.Cancel
    onAccepted: if (noteArea.text.trim().length > 0) dlg.submitted(noteArea.text)

    // Relabel Ok -> "Send report" + gate it on a non-empty note (imperative, the
    // ConfirmDialog pattern — a declarative override can't fall back to style default).
    function _applyStyle() {
        var ok = dlg.standardButton(Controls.Dialog.Ok)
        if (!ok) return
        ok.text = "Send report"
        ok.enabled = Qt.binding(function() { return noteArea.text.trim().length > 0 })
    }
    Component.onCompleted: _applyStyle()
    onAboutToShow: { _applyStyle(); noteArea.forceActiveFocus() }   // 10.1 (TH-A11Y-14)

    Column {
        width: dlg.boxWidth - dlg.leftPadding - dlg.rightPadding
        spacing: Kirigami.Units.smallSpacing
        Controls.Label {
            width: parent.width
            wrapMode: Text.WordWrap
            text: "Describe what happened. Your note plus the current session's log "
                  + "(scrubbed of IPs and tokens) goes straight to the problem repo — "
                  + "no restart needed."
        }
        Controls.ScrollView {
            width: parent.width
            height: Kirigami.Units.gridUnit * 6
            Controls.TextArea {
                id: noteArea
                placeholderText: "What were you doing? What did you expect? What happened instead?"
                wrapMode: TextEdit.Wrap
                color: Th.bugReportNoteFg
                placeholderTextColor: Th.bugReportNotePlaceholder
                background: Rectangle { color: Th.bugReportNoteBg; radius: dlg.theme.radiusSm; border.width: 1; border.color: noteArea.activeFocus ? Th.bugReportNoteBorderFocus : Th.bugReportNoteBorder }
            }
        }
    }
}
