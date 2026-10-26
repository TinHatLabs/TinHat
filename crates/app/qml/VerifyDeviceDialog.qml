// VerifyDeviceDialog — "Verify this device".
//
// Two ways in, and the ORDER matters. Pasting a Security Key means going to another
// client, digging the key out of Secure Backup settings, and typing it across — which
// is the step people abandon, leaving them unverified, unable to read encrypted
// history, and nagged forever. So the default offer is the one that needs nothing but
// a device you're already signed in to: compare seven emoji, tap "They match", done.
// The Security Key stays as the fallback for when there IS no second device.
//
// Driven entirely by `sasState`, set by the login controller:
//   ""  choose a method   ·  "waiting" asking your other device  ·  "emoji" compare
//   "confirming" waiting on the other side  ·  "done"  ·  "cancelled"
// 2026-09-25 (shared spec with Tin Hat Web): while "waiting" as the requester, `sasCanStart` means
// the other device offers QR too and lets its user pick (Element Classic / Element Web) — we wait
// for its start and offer "Start emoji comparison" (S2). On the verifier side (sasIncoming) the
// title stays "Verify the other device" for the whole flow (S7).
//
// Content is a plain Column with explicit widths, NOT a ColumnLayout: a Layout as a
// Controls.Dialog content child reported no height here and the dialog opened invisible
// (it "did nothing" on click). Every working dialog in this app uses the Column shape.
import QtQuick
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the dialog.* tokens

Controls.Dialog {
    id: dlg
    property var app: null
    FallbackTheme { id: fallbackTheme }
    readonly property var theme: app || fallbackTheme

    property bool busy: false
    property string status: ""
    property string sasState: ""
    property string sasEmoji: "[]"
    property string sasNote: ""
    property string sasPeer: ""
    property bool sasIncoming: false
    property bool sasCanStart: false
    property string recoveryKey: ""
    property bool firstDevice: false        // F2: recovery-done is a new account's first Security Key
    property bool canBootstrap: false
    property bool keyPathOpen: false
    // Secret Storage but no identity (a moved server, moved.rs): no other device can vouch, the Security Key first
    property bool keyFirst: false
    // 12.0.1: "Lost your Security Key? Start over" (start_over.rs; Tin Hat Web's same panel)
    property bool startOverOpen: false
    property string startOverNote: ""       // why it didn't happen ("" = nothing to say); nothing changed then

    signal verifyRequested(string key)      // Security-Key path (fallback)
    signal useAnotherDeviceRequested()      // start emoji verification
    signal sasAccepted()                    // accept an incoming request
    signal sasConfirmed()                   // "they match"
    signal sasMismatched()                  // "they don't match" (tells the other side why)
    signal sasCancelled()                   // cancel / dismissed
    signal startComparisonRequested()       // S2 "Start emoji comparison" (requester, waiting)
    signal setUpRecoveryRequested(string password) // no device can vouch — create recovery instead
    signal startOverRequested(string password)     // no key, no device: a new identity + Security Key

    // named clearForm, NOT reset: "reset" collides with QQC2 Dialog's built-in
    // reset() signal (qt.qml.invalidOverride at every load)
    function clearForm() { secKeyField.text = ""; pwField.text = ""; soPwField.text = ""; dlg.keyPathOpen = false; dlg.startOverOpen = false }

    // Hold (same rule as Tin Hat Web's S9 hold, SHEET_HOLD_MS; adversarial review 2026-09-25): the dialog is
    // centred, so the NEXT state's buttons appear where the pointer just was — in the real app a double click on
    // "Start emoji comparison" (90-250 ms apart) landed its second click on "They match" and confirmed emoji nobody
    // had looked at (3/3). The action buttons stay disabled for HOLD_MS after the dialog opens or its state (or
    // Start button / direction) changes.
    readonly property int holdMs: 1000
    property bool held: false
    // (+ the emoji themselves: new emoji under the pointer are a new layout too — the web's holdKey is the same)
    readonly property string holdKey: dlg.sasState + "|" + (dlg.sasCanStart ? 1 : 0) + "|" + (dlg.sasIncoming ? 1 : 0) + "|" + dlg.sasEmoji
    function _hold() { dlg.held = true; holdTimer.restart() }
    onHoldKeyChanged: if (dlg.opened) dlg._hold()
    onOpened: dlg._hold()
    // the Security Key and the password never stay in the fields once the dialog closes (they used to outlive a
    // sign-out and fill the next session's dialog)
    onClosed: dlg.clearForm()
    Timer { id: holdTimer; interval: dlg.holdMs; onTriggered: dlg.held = false }

    readonly property var emojiList: {
        try { return JSON.parse(dlg.sasEmoji || "[]") } catch (e) { return [] }
    }

    // S7: an incoming flow is about the OTHER device from the request to done/cancelled.
    // F2: sas_policy::FIRST_DEVICE_TITLE (web FIRST_DEVICE_TITLE)
    title: (dlg.firstDevice && dlg.sasState === "recovery-done") ? "Your Security Key"
           : (dlg.sasIncoming && dlg.sasState !== "" && dlg.sasState.indexOf("recovery") !== 0)
           ? "Verify the other device" : "Verify this device"
    modal: true
    parent: Controls.Overlay.overlay
    anchors.centerIn: parent
    readonly property real boxWidth: Math.min(Kirigami.Units.gridUnit * 26,
                                              (parent ? parent.width : Kirigami.Units.gridUnit * 26) - Kirigami.Units.gridUnit * 2)
    implicitWidth: dlg.boxWidth
    // 10.1 (TH-SET-24 / TH-FIRST-06): the one-time recovery key can't be closed away until
    // the user says they have it somewhere (Escape still works; this is a nudge, not a lock).
    standardButtons: (dlg.sasState === "recovery-done" && !savedCheck.checked) ? Controls.Dialog.NoButton : Controls.Dialog.Close
    onRejected: if (dlg.sasState !== "" && dlg.sasState !== "done") dlg.sasCancelled()

    Column {
        width: dlg.boxWidth - dlg.leftPadding - dlg.rightPadding
        spacing: Kirigami.Units.largeSpacing

        // ---- Method chooser (idle) ----
        Column {
            width: parent.width
            spacing: Kirigami.Units.smallSpacing
            visible: dlg.sasState === "" && !dlg.keyFirst

            Controls.Label {
                width: parent.width
                wrapMode: Text.WordWrap
                text: "Verifying proves this really is you, so encrypted messages can be "
                      + "read here and others stop seeing warnings about you."
            }
            Controls.Button {
                width: parent.width
                text: "Use another device"
                highlighted: true
                enabled: !dlg.busy
                onClicked: dlg.useAnotherDeviceRequested()
            }
            Controls.Label {
                width: parent.width
                wrapMode: Text.WordWrap
                color: Th.dialogHintFg
                text: "Compare seven emoji with a device you're already signed in to. Nothing to type."
            }
            Controls.Button {
                width: parent.width
                flat: true
                text: dlg.keyPathOpen ? "Hide the Security Key option"
                                      : "No other device? Use your Security Key"
                onClicked: dlg.keyPathOpen = !dlg.keyPathOpen
            }
        }

        // ---- Security Key (fallback) ----
        Column {
            width: parent.width
            spacing: Kirigami.Units.smallSpacing
            visible: dlg.sasState === "" && (dlg.keyPathOpen || dlg.keyFirst)

            Controls.Label {
                width: parent.width
                wrapMode: Text.WordWrap
                visible: dlg.keyFirst
                text: "Unlock your encrypted messages on this device with your Security Key."
            }
            Controls.Label {
                width: parent.width
                wrapMode: Text.WordWrap
                color: Th.dialogHintFg
                text: dlg.keyFirst
                      ? "It's the key you saved when you first signed in. If your server moved to a new address, "
                        + "this brings your messages back and makes this device your new verified one."
                      : "It's the Security Key you saved when you first set up encryption (Tin Hat shows it "
                        + "only once, so look where you keep it: a password manager, a note, a printout)."
            }
            Controls.TextField {
                id: secKeyField
                width: parent.width
                placeholderText: "Security Key (e.g. EsTx …)"
                echoMode: TextInput.Password
                enabled: !dlg.busy
                onAccepted: if (text.length > 0) dlg.verifyRequested(text)
            }
            Controls.Button {
                width: parent.width
                text: dlg.busy ? (dlg.keyFirst ? "Unlocking…" : "Verifying…")
                               : (dlg.keyFirst ? "Unlock with Security Key" : "Verify with Security Key")
                highlighted: dlg.keyFirst
                enabled: !dlg.busy && secKeyField.text.length > 0
                onClicked: dlg.verifyRequested(secKeyField.text)
            }
            Controls.Label {
                width: parent.width
                wrapMode: Text.WordWrap
                visible: dlg.status.length > 0
                text: dlg.status
                color: dlg.busy ? Th.dialogFg : Th.dialogErrorFg
            }

            // 12.0.1: no Security Key and no other signed-in device — the way out (start_over.rs). Before this, such
            // an account could sign in but never unlock or verify anything again, and nobody could help.
            Controls.Button {
                width: parent.width
                flat: true
                text: dlg.startOverOpen ? "Never mind, I'll keep looking" : "Lost your Security Key? Start over"
                onClicked: dlg.startOverOpen = !dlg.startOverOpen
            }
            Column {
                width: parent.width
                spacing: Kirigami.Units.smallSpacing
                visible: dlg.startOverOpen
                Controls.Label {
                    width: parent.width
                    wrapMode: Text.WordWrap
                    font.bold: true
                    text: "Start over with a new Security Key"
                }
                Controls.Label {
                    width: parent.width
                    wrapMode: Text.WordWrap
                    color: Th.dialogHintFg
                    text: "Only for when no other device is signed in and your Security Key is gone. Messages from "
                          + "before stay locked, unless a device that still has them signs in again. People who "
                          + "verified you will see that your security changed, and may verify you again."
                }
                Controls.TextField {
                    id: soPwField
                    width: parent.width
                    placeholderText: "Account password"
                    echoMode: TextInput.Password
                    enabled: !dlg.busy
                    onAccepted: if (text.length > 0) dlg.startOverRequested(text)
                }
                Controls.Button {
                    width: parent.width
                    text: "Start over"
                    enabled: !dlg.busy && soPwField.text.length > 0
                    onClicked: dlg.startOverRequested(soPwField.text)
                }
                Controls.Label {
                    width: parent.width
                    wrapMode: Text.WordWrap
                    visible: dlg.startOverNote.length > 0
                    text: dlg.startOverNote
                    color: Th.dialogErrorFg
                }
            }
        }

        // ---- Waiting on the other device ----
        Column {
            width: parent.width
            spacing: Kirigami.Units.largeSpacing
            visible: dlg.sasState === "waiting" || dlg.sasState === "confirming"

            ThBusyIndicator {   // 11.0 (GPU.md §2.5): still — it steps as the verification moves on
                anchors.horizontalCenter: parent.horizontalCenter
                running: parent.visible
                size: 48
                color: Th.loaderSpinnerFg
                progress: dlg.sasState
            }
            Controls.Label {
                width: parent.width
                wrapMode: Text.WordWrap
                horizontalAlignment: Text.AlignHCenter
                text: dlg.sasNote
            }
            Controls.Button {
                width: parent.width
                visible: dlg.sasIncoming && dlg.sasState === "waiting"
                enabled: !dlg.held
                highlighted: true
                text: "Accept"
                onClicked: dlg.sasAccepted()
            }
            // S2: the other device lets its user choose the method, so we don't start on our own —
            // but the user can (hidden again once a comparison exists).
            Controls.Button {
                width: parent.width
                visible: !dlg.sasIncoming && dlg.sasState === "waiting" && dlg.sasCanStart
                enabled: !dlg.held
                highlighted: true
                text: "Start emoji comparison"
                onClicked: dlg.startComparisonRequested()
            }
            Controls.Button {
                width: parent.width
                flat: true
                enabled: !dlg.held
                text: "Cancel"
                onClicked: dlg.sasCancelled()
            }
        }

        // ---- Emoji comparison ----
        Column {
            width: parent.width
            spacing: Kirigami.Units.largeSpacing
            visible: dlg.sasState === "emoji"

            Controls.Label {
                width: parent.width
                wrapMode: Text.WordWrap
                text: dlg.sasPeer.length > 0
                      ? "Confirm these emoji appear in the same order on " + dlg.sasPeer + "."
                      : dlg.sasNote
            }
            Grid {
                anchors.horizontalCenter: parent.horizontalCenter
                columns: 4
                spacing: Kirigami.Units.largeSpacing
                Repeater {
                    model: dlg.emojiList
                    delegate: Column {
                        required property var modelData
                        spacing: 0
                        Controls.Label {
                            anchors.horizontalCenter: parent.horizontalCenter
                            text: parent.modelData.e
                            font.pointSize: 24
                        }
                        Controls.Label {
                            anchors.horizontalCenter: parent.horizontalCenter
                            text: parent.modelData.name
                            color: Th.dialogHintFg
                        }
                    }
                }
            }
            Controls.Button {
                width: parent.width
                highlighted: true
                enabled: !dlg.held
                text: "They match"
                onClicked: dlg.sasConfirmed()
            }
            Controls.Button {
                width: parent.width
                flat: true
                enabled: !dlg.held
                text: "They don't match"
                onClicked: dlg.sasMismatched()
            }
        }

        // ---- Terminal states ----
        Column {
            width: parent.width
            spacing: Kirigami.Units.smallSpacing
            visible: dlg.sasState === "done" || dlg.sasState === "cancelled"

            Controls.Label {
                width: parent.width
                wrapMode: Text.WordWrap
                text: dlg.sasNote
                color: dlg.sasState === "done" ? Th.dialogSuccessFg
                                               : Th.dialogErrorFg
            }
            Controls.Button {
                width: parent.width
                visible: dlg.sasState === "cancelled"
                enabled: !dlg.held
                text: "Try again"
                onClicked: dlg.useAnotherDeviceRequested()
            }
            // Offered when nothing can vouch for this device. Without it such an account
            // is a dead end — no key to recover with, and no verified device to compare
            // emoji against.
            // Uploading cross-signing keys is a protected action, so the server asks
            // for the password. Nothing else here needs it, which is why it lives with
            // this button rather than in the main flow.
            Controls.Label {
                width: parent.width
                wrapMode: Text.WordWrap
                visible: dlg.sasState === "cancelled" && dlg.canBootstrap
                color: Th.dialogHintFg
                text: "Setting this up needs your account password — the server asks for it before it will trust new keys."
            }
            Controls.TextField {
                id: pwField
                width: parent.width
                visible: dlg.sasState === "cancelled" && dlg.canBootstrap
                placeholderText: "Account password"
                echoMode: TextInput.Password
                onAccepted: if (text.length > 0) dlg.setUpRecoveryRequested(text)
            }
            Controls.Button {
                width: parent.width
                visible: dlg.sasState === "cancelled" && dlg.canBootstrap
                highlighted: true
                enabled: pwField.text.length > 0
                text: "Set up encryption on this device"
                onClicked: dlg.setUpRecoveryRequested(pwField.text)
            }
        }

        // ---- Recovery setup ----
        Column {
            width: parent.width
            spacing: Kirigami.Units.largeSpacing
            visible: dlg.sasState === "recovery-working"
            ThBusyIndicator {   // 11.0 (GPU.md §2.5): still — it steps with each recovery note
                anchors.horizontalCenter: parent.horizontalCenter
                running: parent.visible
                size: 48
                color: Th.loaderSpinnerFg
                progress: dlg.sasNote
            }
            Controls.Label {
                width: parent.width
                wrapMode: Text.WordWrap
                horizontalAlignment: Text.AlignHCenter
                text: dlg.sasNote
            }
        }

        Column {
            width: parent.width
            spacing: Kirigami.Units.smallSpacing
            visible: dlg.sasState === "recovery-done"
            Controls.Label {
                width: parent.width
                wrapMode: Text.WordWrap
                text: dlg.sasNote
                color: Th.dialogSuccessFg
            }
            // Selectable, not a plain Label: the user has to be able to copy this, and
            // it is shown exactly once.
            Controls.TextArea {
                id: keyArea
                width: parent.width
                readOnly: true
                selectByMouse: true
                wrapMode: TextEdit.WrapAnywhere
                text: dlg.recoveryKey
                font.family: Th.fontMono
            }
            Row {
                spacing: Kirigami.Units.smallSpacing
                ThButton {
                    app: dlg.app
                    text: "Copy key"
                    onClicked: {
                        if (dlg.app && dlg.app.copyToClipboard) dlg.app.copyToClipboard(dlg.recoveryKey)
                        else { keyArea.selectAll(); keyArea.copy(); keyArea.deselect() }
                        if (dlg.app && dlg.app.showPassiveNotification) dlg.app.showPassiveNotification("Recovery key copied — paste it somewhere safe", "short")
                    }
                }
            }
            Controls.CheckBox {
                id: savedCheck
                width: parent.width
                text: "I've saved this key somewhere safe"
                checked: false
            }
        }
    }
}
