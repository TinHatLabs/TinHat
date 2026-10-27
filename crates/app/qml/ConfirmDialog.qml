// ConfirmDialog — a small reusable confirm/cancel dialog for destructive actions
// (leave room, delete message, sign out, …). Set `dialogTitle` + `message`, connect
// `confirmed()`. Optional: `confirmText` relabels the confirm button ("Leave", "Delete"…)
// and `destructive: true` gives it the dialog.button.danger.* colours of Th. The rest is the
// style's dialog over the app palette, like every other Controls dialog.
import QtQuick
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the active theme (docs/MIGRATING-THEME.md)

Controls.Dialog {
    id: dlg
    // Optional theme source for the destructive fill (call sites may omit it; the
    // fallback matches OLED, whose danger tokens read correctly on every theme).
    property var app: null
    FallbackTheme { id: fallbackTheme }
    readonly property var theme: app || fallbackTheme

    property string dialogTitle: "Are you sure?"
    property string message: ""
    property string confirmText: "OK"
    // Optional: relabel the Cancel button (e.g. "Turn it off"). Empty = style default.
    property string cancelText: ""
    property bool destructive: false
    // RBAC (docs/RBAC.md §7.2): kick/ban confirms carry an optional reason. Opt-in — every
    // existing call site keeps its plain message. `reason` is what the caller sends along.
    property bool askReason: false
    property string reasonPlaceholder: "Reason (optional) — they'll see it"
    readonly property string reason: askReason ? reasonField.text.trim() : ""
    signal confirmed()
    // Emitted when the user explicitly cancels/declines (Cancel button or Esc). Most
    // call sites ignore it; the log-sharing consent uses it as "decline".
    signal cancelled()

    title: LinkGate.plain(dlg.dialogTitle)
    modal: true
    parent: Controls.Overlay.overlay
    anchors.centerIn: parent
    // Content width is a stable constant (NOT the dialog's own negotiated width) so the
    // WordWrap label's implicitHeight -> contentHeight -> Dialog.implicitHeight chain can't
    // cycle (this was a runtime implicitHeight binding loop via breeze Dialog.qml:17).
    readonly property real boxWidth: Math.min(380, (parent ? parent.width : 380) - Kirigami.Units.gridUnit * 2)
    implicitWidth: dlg.boxWidth
    standardButtons: Controls.Dialog.Ok | Controls.Dialog.Cancel
    onAccepted: dlg.confirmed()
    onRejected: dlg.cancelled()

    // Restyle the stock Ok button imperatively: the style's default look must survive
    // untouched when !destructive, and a declarative background override can't fall
    // back to the style default.
    property bool _confirmStyled: false
    function _applyConfirmStyle() {
        var ok = dlg.standardButton(Controls.Dialog.Ok)
        if (!ok) return
        ok.text = dlg.confirmText
        if (dlg.cancelText.length > 0) {
            var cancel = dlg.standardButton(Controls.Dialog.Cancel)
            if (cancel) cancel.text = dlg.cancelText
        }
        if (dlg.destructive && !dlg._confirmStyled) {
            dlg._confirmStyled = true
            // Capture the style's control height first so the confirm button
            // stays the same size as its default-styled Cancel sibling.
            var h = ok.implicitHeight
            ok.contentItem = dangerText.createObject(ok)
            ok.background = dangerFill.createObject(ok, { implicitHeight: h })
        }
    }
    Component.onCompleted: _applyConfirmStyle()
    onAboutToShow: { _applyConfirmStyle(); if (dlg.askReason) reasonField.text = "" }
    onOpened: if (dlg.askReason) reasonField.forceActiveFocus()
    onConfirmTextChanged: _applyConfirmStyle()

    Component {
        id: dangerText
        Controls.Label {
            textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
            text: dlg.confirmText
            color: Th.dialogButtonDangerFg
            font.bold: true
            horizontalAlignment: Text.AlignHCenter
            verticalAlignment: Text.AlignVCenter
        }
    }
    Component {
        id: dangerFill
        Rectangle {
            implicitHeight: Kirigami.Units.gridUnit * 1.6
            radius: dlg.theme.radius
            // `parent` is the Ok button (QQuickControl reparents its background).
            // Hover keeps the DARK danger fill (the bold onAccent label needs 4.5:1 —
            // dangerBright as a fill dropped it to ~2.4) and signals via the border.
            color: parent && parent.down ? Th.dialogButtonDangerBgPressed : Th.dialogButtonDangerBg
            border.width: parent && parent.visualFocus ? 2 : (parent && parent.hovered ? 2 : 0)
            border.color: parent && parent.visualFocus ? Th.dialogButtonDangerBorderFocus : Th.dialogButtonDangerBorderHover
            Behavior on color { ColorAnimation { duration: dlg.theme.dur; easing.type: Easing.OutCubic } }
        }
    }

    // ONE content child (the popup's implicit size follows a single child), width pinned to
    // the stable constant above — see the binding-loop note on boxWidth.
    Column {
        width: dlg.boxWidth - dlg.leftPadding - dlg.rightPadding
        spacing: Kirigami.Units.largeSpacing
        Controls.Label {
            textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
            width: parent.width
            text: dlg.message
            wrapMode: Text.WordWrap
        }
        ThTextField {
            id: reasonField
            width: parent.width
            visible: dlg.askReason
            placeholderText: dlg.reasonPlaceholder
            Accessible.name: "Reason"
            onAccepted: dlg.accept()
        }
    }
}
