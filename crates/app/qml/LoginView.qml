// LoginView — the polished sign-in screen for Tin Hat.
//
// A centered "auth card" with soft shadow + rounded corners, warm-focus fields, a
// glowing primary button, and the app wordmark. The Furry theme layers on the
// AuroraBackground, a FloofCorner mascot, and a PrideStrip footer — these are inline
// components of the sibling FurryFlair type (owned by the furry-flair agent), so they
// are referenced as FurryFlair.AuroraBackground / .FloofCorner / .PrideStrip.
//
// CXX-Qt quirk (see Main.qml header): NOTIFY signals don't reliably drive bindings,
// so this view takes ALL controller state via plain QML properties pushed in by the
// parent (busy/status/homeserver/...), and emits intent via signals the parent wires
// to the LoginController invokables. No direct getter binding here.
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the login.* tokens

Item {
    id: loginView
    required property var app   // ApplicationWindow (theme tokens); set by parent
    FallbackTheme { id: fallbackTheme }
    readonly property var theme: app || fallbackTheme

    // Colours: the login.* tokens of the active theme (Th); sizes (radius…) come from `theme`.
    property string appIconSource: ""   // 0.8.26: file:// of the bundled app icon PNG
    property string baseFontFamily: ""

    // ---- Mirrored controller state (plain props, pushed by parent) ----------
    property bool busy: false
    property string status: ""
    property string homeserver: ""
    // a server that moved to its own domain (moved.rs): "OLD says it moved to NEW…" (the field changes only on a click)
    property string movedNote: ""
    property string movedTo: ""   // the address the note offers (bridge.rs check_moved); the field changes only on a click
    // "" idle | "sent" (link is in their inbox) | "done" (password changed)
    property string resetStage: ""
    property string resetStatus: ""
    // selfhost (password.rs): how the typed server takes back a forgotten password — "" not asked yet | "checking" |
    // "email" (the form below) | "owner" (no email: resetNote says who sets a new one)
    property string resetMode: ""
    property string resetNote: ""
    readonly property bool resetByEmail: loginView.resetMode === "email"
    // 10.4: "Create an account" with an invite code (docs/INVITES.md). Stage "created" = the
    // account exists and the normal sign-in is running; status carries progress / failure text.
    property string registerStage: ""
    property string registerStatus: ""
    property bool showCreate: false

    // Which face of the card is showing. Local, because backing out of the reset
    // form is a pure UI decision — the controller only hears about it if there is
    // actually a reset in flight to abandon.
    property bool showReset: false

    // ---- Intent signals (parent wires these to LoginController) -------------
    signal homeserverEdited(string value)
    signal useMovedServer()   // the person accepted the moved server's new address (the note's link)
    signal submit(string username, string password)

    // 10.0.6: self-test sign-in (env TINHAT_GUI_LOGIN="user:password") — the headless gates run the
    // real sign-in form instead of needing a pre-signed profile (the Flatpak E2E gate on the Flatpak build machine).
    property string autoLogin: ""
    property bool _autoLoginDone: false   // fires ONCE — the view stays instantiated after a sign-in
    // 10.1 (TH-FIRST-17 / TH-A11Y-04): the caret lands where you type next. (TH-FIRST-12): a sign-out
    // clears the password — it used to sit in the field, one eye-toggle from plaintext.
    function focusFirst() {
        if (loginView.showReset) return
        if (usernameField.text.length > 0) passwordField.focusField(); else usernameField.focusField()
    }
    function clearCredentials() { passwordField.text = ""; passwordField.showPassword = false; Qt.callLater(loginView.focusFirst) }
    Component.onCompleted: Qt.callLater(loginView.focusFirst)
    onVisibleChanged: if (visible) Qt.callLater(loginView.focusFirst)
    // (TH-FIRST-22): after 8 s the status line admits it is still trying.
    property bool slow: false
    Timer { id: slowTimer; interval: 8000; running: loginView.busy; onTriggered: loginView.slow = true }
    onBusyChanged: if (!busy) loginView.slow = false
    Timer {
        interval: 800; repeat: false
        running: !loginView._autoLoginDone && loginView.visible && loginView.autoLogin.indexOf(":") > 0 && !loginView.busy
        onTriggered: {
            loginView._autoLoginDone = true
            var i = loginView.autoLogin.indexOf(":")
            usernameField.text = loginView.autoLogin.substring(0, i)
            passwordField.text = loginView.autoLogin.substring(i + 1)
            console.log("[login] self-test sign-in as " + usernameField.text)
            loginView.tryLogin()
        }
    }
    signal resetRequested(string email)
    signal resetCompleted(string newPassword)
    signal resetCancelled()
    // selfhost: the reset face opened (or its Server field changed): ask that server how it resets passwords
    signal resetCheck(string server)
    Timer { id: resetCheckT; interval: 600; onTriggered: if (loginView.showReset && loginView.resetStage === "") loginView.resetCheck(homeserverField.text) }
    // the email form shows once the server answered: the caret goes to its field (the field was hidden while it asked)
    onResetModeChanged: if (loginView.showReset && loginView.resetMode === "email" && loginView.resetStage === "") resetEmailField.focusField()
    signal createAccount(string code, string username, string password, string displayName)   // 10.4
    signal createCancelled()

    // Pick a blobfox emote url (only meaningful when furry).

    // ----------------------------------------------------------------------
    // Background: a base wash for every theme; furry overlays the aurora.
    // ----------------------------------------------------------------------
    Rectangle {
        anchors.fill: parent
        color: Th.loginBg
    }

    // Soft top-corner accent glow (subtle radial-ish wash
    // faked with a large, very-low-opacity rounded rect behind everything).
    Rectangle {
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.top: parent.top
        anchors.topMargin: -height * 0.55
        width: Math.max(parent.width, parent.height) * 0.9
        height: width
        radius: width / 2
        opacity: 0.10
        gradient: Gradient {
            GradientStop { position: 0.0; color: Th.loginWashGlow }
            GradientStop { position: 1.0; color: "transparent" }
        }
    }


    // ----------------------------------------------------------------------
    // The auth card.
    // ----------------------------------------------------------------------
    Item {
        id: cardWrap
        anchors.centerIn: parent
        width: Math.min(parent.width - Kirigami.Units.gridUnit * 3, Kirigami.Units.gridUnit * 25)
        height: cardCol.implicitHeight + Kirigami.Units.gridUnit * 3

        // Soft drop shadow: stacked, offset, blurred-look rounded rects behind the
        // card. (No Qt5Compat DropShadow dependency — pure QtQuick so it always
        // compiles.) Three rings of increasing size + decreasing opacity.
        Repeater {
            model: 3
            Rectangle {
                required property int index
                anchors.centerIn: card
                width: card.width + (index + 1) * 6
                height: card.height + (index + 1) * 6
                radius: card.radius + (index + 1) * 3
                color: "transparent"
                border.width: 6
                // ring i paints the token at its alpha ÷ i (contract PARITY P69 — the web's rule once its alpha is float)
                border.color: Qt.alpha(Th.loginCardShadow, Th.loginCardShadow.a / (index + 1))
                y: card.y + (index + 1) * 3
            }
        }

        Rectangle {
            id: card
            anchors.fill: parent
            radius: loginView.theme.radiusXl
            color: Th.loginCardBg
            border.width: 1
            border.color: Th.loginCardBorder

            // A faint top-edge highlight so the card reads as lit from above.
            Rectangle {
                anchors { top: parent.top; left: parent.left; right: parent.right }
                anchors.margins: 1
                height: parent.radius
                radius: parent.radius
                gradient: Gradient {
                    GradientStop { position: 0.0; color: Th.loginCardHighlight }
                    GradientStop { position: 1.0; color: "transparent" }
                }
            }

            ColumnLayout {
                id: cardCol
                anchors.fill: parent
                anchors.margins: Kirigami.Units.gridUnit * 1.5
                spacing: Kirigami.Units.largeSpacing

                // ---- Wordmark -------------------------------------------------
                Item {
                    Layout.alignment: Qt.AlignHCenter
                    Layout.preferredWidth: Kirigami.Units.iconSizes.huge
                    Layout.preferredHeight: Kirigami.Units.iconSizes.huge

                    // Glow disc behind the logo/mascot.
                    Rectangle {
                        anchors.centerIn: parent
                        width: parent.width * 1.25
                        height: width
                        radius: width / 2
                        opacity: 0.35
                        gradient: Gradient {
                            GradientStop { position: 0.0; color: Th.loginMarkGlow }
                            GradientStop { position: 1.0; color: "transparent" }
                        }
                    }
                    // 0.8.26: the bundled app icon (assets/appicon) — the theme name
                    // "dev.tinhat.TinHat" only ever resolved on a machine that had the
                    // .desktop icon installed (never on Windows).
                    // 0.8.26: the Tin Hat Element mark. 11.0 (GPU.md §2.5): it no longer turns while a sign-in is in
                    // flight — a sign-in has no progress events, so the mark rests (the status line says what happens).
                    TinHatLoader {
                        anchors.fill: parent
                        visible: loginView.appIconSource.length > 0
                        running: loginView.busy
                    }
                    Kirigami.Icon {
                        anchors.fill: parent
                        visible: loginView.appIconSource.length === 0
                        source: "dev.tinhat.TinHat"
                    }
                }

                Kirigami.Heading {
                    Layout.fillWidth: true
                    horizontalAlignment: Text.AlignHCenter
                    level: 1
                    text: "Tin Hat"
                    color: Th.loginTitleFg
                    font.bold: true
                }
                Controls.Label {
                    Layout.fillWidth: true
                    Layout.topMargin: -Kirigami.Units.largeSpacing
                    horizontalAlignment: Text.AlignHCenter
                    text: loginView.showReset ? "Reset your password" : (loginView.showCreate ? "Create your account" : "Sign in to continue")
                    color: Th.loginSubtitleFg
                    font.pointSize: Kirigami.Theme.smallFont.pointSize
                }

                Item { Layout.preferredHeight: Kirigami.Units.smallSpacing } // spacer

                // ---- Fields ---------------------------------------------------
                LoginField {
                    app: loginView.app
                    id: homeserverField
                    Layout.fillWidth: true
                    label: "Server"   // 10.1 (TH-COPY-12): "Homeserver" is Matrix jargon on the first screen a stranger sees
                    icon: "network-server-symbolic"
                    placeholder: "yourname.tinhat.me"   // selfhost: no server of the app's own; the last one signed in to is remembered
                    text: loginView.homeserver
                    enabled: !loginView.busy
                    property int prevLen: 0   // a paste arrives as many characters at once; typing, one at a time
                    onTextEdited: (text) => {
                        // selfhost/firstrun §4: an invite link pasted here is an invite, not a server — its code goes
                        // to the Invite code field (the create form opens) and the server stays what it was
                        var pasted = text.length - homeserverField.prevLen >= 6
                        homeserverField.prevLen = text.length
                        var code = pasted ? InviteLink.codeFromPaste(text) : null
                        if (code !== null) {
                            loginView.showCreate = true
                            inviteCodeField.text = code
                            homeserverField.text = Qt.binding(function () { return loginView.homeserver })
                            inviteCodeField.focusField()
                            return
                        }
                        loginView.homeserverEdited(text)
                        if (loginView.showReset) resetCheckT.restart()   // the reset face follows the server typed
                    }
                    onAccepted: usernameField.focusField()
                }
                Controls.Label {
                    textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                    Layout.fillWidth: true
                    visible: loginView.movedNote.length > 0 && !loginView.showReset
                    wrapMode: Text.WordWrap
                    horizontalAlignment: Text.AlignHCenter
                    text: loginView.movedNote
                    color: Th.loginSubtitleFg
                    font.pointSize: Kirigami.Theme.smallFont.pointSize
                }
                // the moved server's new address, taken only when the person asks (a quiet link, like the two below)
                Controls.Label {
                    readonly property string typed: String(loginView.homeserver).trim().replace(/^https?:\/\//i, "").replace(/\/+$/, "").toLowerCase()
                    visible: loginView.movedNote.length > 0 && loginView.movedTo.length > 0 && !loginView.showReset
                             && typed !== loginView.movedTo
                    Layout.fillWidth: true
                    horizontalAlignment: Text.AlignHCenter
                    textFormat: Text.PlainText
                    text: "Use " + loginView.movedTo
                    color: movedLinkArea.containsMouse ? Th.loginLinkFgHover : Th.loginLinkFg
                    font.pointSize: Kirigami.Theme.smallFont.pointSize
                    font.underline: movedLinkArea.containsMouse
                    MouseArea {
                        id: movedLinkArea
                        anchors.fill: parent
                        hoverEnabled: true
                        cursorShape: Qt.PointingHandCursor
                        enabled: !loginView.busy
                        onClicked: loginView.useMovedServer()
                    }
                }
                LoginField {
                    app: loginView.app
                    id: usernameField
                    visible: !loginView.showReset && !loginView.showCreate
                    Layout.fillWidth: true
                    label: "Username"
                    icon: "user-symbolic"
                    placeholder: "you"
                    enabled: !loginView.busy
                    onAccepted: passwordField.focusField()
                }
                LoginField {
                    app: loginView.app
                    id: passwordField
                    visible: !loginView.showReset && !loginView.showCreate
                    Layout.fillWidth: true
                    label: "Password"
                    icon: "lock-symbolic"
                    password: true
                    enabled: !loginView.busy
                    onAccepted: loginView.tryLogin()
                }

                // ---- Primary sign-in CTA (GlowButton: glow, focus ring, spinner) ----
                GlowButton {
                    id: signInBtn
                    app: loginView.app
                    visible: !loginView.showReset && !loginView.showCreate
                    Layout.fillWidth: true
                    // Stays lit while busy (the spinner shows progress); tryLogin()
                    // guards re-submission.
                    busy: loginView.busy
                    enabled: usernameField.text.length > 0 && passwordField.text.length > 0 && loginView.homeserver.trim().length > 0
                    text: "Sign in"
                    onClicked: loginView.tryLogin()
                }

                // ---- Forgot password ------------------------------------------
                // A quiet text link, not a second button: it must be findable when you
                // need it without competing with the thing everyone came here to do.
                Controls.Label {
                    visible: !loginView.showReset && !loginView.showCreate
                    Layout.fillWidth: true
                    horizontalAlignment: Text.AlignHCenter
                    text: "Forgot your password?"
                    color: resetLinkArea.containsMouse ? Th.loginLinkFgHover : Th.loginLinkFg
                    font.pointSize: Kirigami.Theme.smallFont.pointSize
                    font.underline: resetLinkArea.containsMouse
                    MouseArea {
                        id: resetLinkArea
                        anchors.fill: parent
                        hoverEnabled: true
                        cursorShape: Qt.PointingHandCursor
                        enabled: !loginView.busy
                        onClicked: {
                            // Seed the email box with whatever they typed as a username
                            // only if it already looks like an address — otherwise the
                            // field would start out wrong and have to be cleared.
                            if (usernameField.text.indexOf("@") > 0 && resetEmailField.text.length === 0)
                                resetEmailField.text = usernameField.text
                            loginView.showReset = true
                            // selfhost: a server without email says its owner sets new passwords (password.rs)
                            loginView.resetCheck(homeserverField.text)
                            resetEmailField.focusField()
                        }
                    }
                }

                // ---- Create an account (10.4) -----------------------------------
                // The second quiet link: for someone holding an invite code from a server operator.
                Controls.Label {
                    visible: !loginView.showReset && !loginView.showCreate
                    Layout.fillWidth: true
                    horizontalAlignment: Text.AlignHCenter
                    text: "New here? Create an account"
                    color: createLinkArea.containsMouse ? Th.loginLinkFgHover : Th.loginLinkFg
                    font.pointSize: Kirigami.Theme.smallFont.pointSize
                    font.underline: createLinkArea.containsMouse
                    MouseArea {
                        id: createLinkArea
                        anchors.fill: parent
                        hoverEnabled: true
                        cursorShape: Qt.PointingHandCursor
                        enabled: !loginView.busy
                        onClicked: { loginView.showCreate = true; inviteCodeField.focusField() }
                    }
                }
                // Invite code, username, display name, password twice. The code is a Synapse
                // registration token; the Server field above is where the account lands.
                ColumnLayout {
                    visible: loginView.showCreate
                    Layout.fillWidth: true
                    spacing: Kirigami.Units.smallSpacing
                    LoginField {
                        app: loginView.app
                        id: inviteCodeField
                        Layout.fillWidth: true
                        label: "Invite code"
                        icon: "mail-message-symbolic"
                        prefix: "TH-"                 // 10.4: only the last part is typed
                        placeholder: "XXXX-XXXX"
                        enabled: !loginView.busy
                        // A pasted full code keeps working: drop the prefix the field already shows. A pasted invite
                        // link puts its code here (selfhost/firstrun §4, InviteLink.qml).
                        property int prevLen: 0   // a paste arrives as many characters at once; typing, one at a time
                        onTextEdited: (t) => {
                            var pasted = t.length - inviteCodeField.prevLen >= 6
                            inviteCodeField.prevLen = t.length
                            var code = pasted ? InviteLink.codeFromPaste(t) : null
                            if (code !== null) { inviteCodeField.text = code; return }
                            if (/^\s*th-/i.test(t)) inviteCodeField.text = t.replace(/^\s*th-/i, "")
                        }
                        onAccepted: newUserField.focusField()
                    }
                    LoginField {
                        app: loginView.app
                        id: newUserField
                        Layout.fillWidth: true
                        label: "Username"
                        icon: "user-symbolic"
                        placeholder: "lowercase, no spaces"
                        enabled: !loginView.busy
                        onAccepted: displayNameField.focusField()
                    }
                    LoginField {
                        app: loginView.app
                        id: displayNameField
                        Layout.fillWidth: true
                        label: "Display name (optional)"
                        icon: "contact-new-symbolic"
                        placeholder: "How people see you"
                        enabled: !loginView.busy
                        onAccepted: newPwField.focusField()
                    }
                    LoginField {
                        app: loginView.app
                        id: newPwField
                        Layout.fillWidth: true
                        label: "Password"
                        icon: "lock-symbolic"
                        password: true
                        enabled: !loginView.busy
                        onAccepted: newPw2Field.focusField()
                    }
                    LoginField {
                        app: loginView.app
                        id: newPw2Field
                        Layout.fillWidth: true
                        label: "Password again"
                        icon: "lock-symbolic"
                        password: true
                        enabled: !loginView.busy
                        onAccepted: loginView.tryCreate()
                    }
                    Controls.Label {
                        Layout.fillWidth: true
                        horizontalAlignment: Text.AlignHCenter
                        wrapMode: Text.WordWrap
                        color: (newPw2Field.text.length > 0 && newPw2Field.text !== newPwField.text) ? Th.loginErrorFg : Th.loginHintFg
                        font.pointSize: Kirigami.Theme.smallFont.pointSize
                        text: (newPw2Field.text.length > 0 && newPw2Field.text !== newPwField.text) ? "The passwords don't match."
                            : "Usernames are lowercase letters, numbers, dots, dashes and underscores. Passwords are at least 8 characters."
                    }
                    GlowButton {
                        id: createBtn
                        app: loginView.app
                        Layout.fillWidth: true
                        busy: loginView.busy
                        enabled: loginView.createReady
                        text: "Create account"
                        onClicked: loginView.tryCreate()
                    }
                    Controls.Label {
                        textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                        Layout.fillWidth: true
                        horizontalAlignment: Text.AlignHCenter
                        wrapMode: Text.WordWrap
                        visible: loginView.registerStatus.length > 0
                        text: loginView.registerStatus
                        color: (loginView.busy || loginView.registerStage === "created") ? Th.loginHintFg : Th.loginErrorFg
                        font.pointSize: Kirigami.Theme.smallFont.pointSize
                    }
                    Controls.Label {
                        Layout.fillWidth: true
                        horizontalAlignment: Text.AlignHCenter
                        text: "Back to sign in"
                        color: createBackArea.containsMouse ? Th.loginLinkFgHover : Th.loginLinkFg
                        font.pointSize: Kirigami.Theme.smallFont.pointSize
                        font.underline: createBackArea.containsMouse
                        MouseArea {
                            id: createBackArea
                            anchors.fill: parent
                            hoverEnabled: true
                            cursorShape: Qt.PointingHandCursor
                            enabled: !loginView.busy
                            onClicked: loginView.leaveCreate()
                        }
                    }
                }

                // ---- Reset panel ----------------------------------------------
                // Step 1 (ask for the email) and step 3 (set the new password) are the
                // only steps this app can perform; step 2 happens in the user's mail
                // client, which is why the two halves are shown at once with the
                // second one disabled rather than as separate screens — you can see
                // where you are in the flow without losing the page.
                // selfhost (password.rs): the form only once the typed server answered that it mails links; a server
                // whose owner sets new passwords (its well-known: account.password_reset "owner") gets the owner face —
                // no email field, resetNote and "Back to sign in". "Checking <host>…" (resetStatus) while it asks.
                ColumnLayout {
                    visible: loginView.showReset
                    Layout.fillWidth: true
                    spacing: Kirigami.Units.smallSpacing

                    Controls.Label {
                        textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                        visible: loginView.resetMode === "owner" && loginView.resetStage === ""
                        Layout.fillWidth: true
                        wrapMode: Text.WordWrap
                        horizontalAlignment: Text.AlignHCenter
                        text: loginView.resetNote
                        color: Th.loginSubtitleFg
                        font.pointSize: Kirigami.Theme.smallFont.pointSize
                    }

                    Controls.Label {
                        visible: loginView.resetStage !== "done" && loginView.resetByEmail
                        Layout.fillWidth: true
                        wrapMode: Text.WordWrap
                        horizontalAlignment: Text.AlignHCenter
                        text: "We'll email you a link. Open it, then set a new password here."
                        color: Th.loginHintFg
                        font.pointSize: Kirigami.Theme.smallFont.pointSize
                    }

                    LoginField {
                        app: loginView.app
                        id: resetEmailField
                        visible: loginView.resetStage !== "done" && loginView.resetByEmail
                        Layout.fillWidth: true
                        label: "Email"
                        icon: "mail-message-symbolic"
                        placeholder: "you@example.com"
                        enabled: !loginView.busy
                        onAccepted: loginView.tryReset()
                    }
                    GlowButton {
                        id: resetSendBtn
                        app: loginView.app
                        visible: loginView.resetStage !== "done" && loginView.resetByEmail
                        Layout.fillWidth: true
                        busy: loginView.busy
                        enabled: !loginView.busy && resetEmailField.text.indexOf("@") > 0
                        // Wording carries the state: the second press is a re-send, and
                        // saying so stops it reading as "did that not work?"
                        text: loginView.resetStage === "sent" ? "Resend the email" : "Email me a reset link"
                        onClicked: loginView.tryReset()
                    }

                    Item {
                        visible: loginView.resetStage === "sent"
                        Layout.preferredHeight: Kirigami.Units.smallSpacing
                    }
                    LoginField {
                        app: loginView.app
                        id: resetPwField
                        visible: loginView.resetStage === "sent"
                        Layout.fillWidth: true
                        label: "New password"
                        icon: "lock-symbolic"
                        password: true
                        enabled: !loginView.busy
                        onAccepted: loginView.tryResetComplete()
                    }
                    LoginField {
                        app: loginView.app
                        id: resetPw2Field
                        visible: loginView.resetStage === "sent"
                        Layout.fillWidth: true
                        label: "Confirm new password"
                        icon: "lock-symbolic"
                        password: true
                        enabled: !loginView.busy
                        onAccepted: loginView.tryResetComplete()
                    }
                    // Mismatch is caught here rather than after a round trip, so you
                    // aren't told about a typo by the server.
                    Controls.Label {
                        visible: loginView.resetStage === "sent" && resetPw2Field.text.length > 0
                                 && resetPwField.text !== resetPw2Field.text
                        Layout.fillWidth: true
                        horizontalAlignment: Text.AlignHCenter
                        text: "Those two don't match."
                        color: Th.loginErrorFg
                        font.pointSize: Kirigami.Theme.smallFont.pointSize
                    }
                    GlowButton {
                        app: loginView.app
                        visible: loginView.resetStage === "sent"
                        Layout.fillWidth: true
                        busy: loginView.busy
                        enabled: !loginView.busy && resetPwField.text.length >= 8
                                 && resetPwField.text === resetPw2Field.text
                        text: "Set new password"
                        onClicked: loginView.tryResetComplete()
                    }

                    // Done: the only thing left to do is sign in, so make that the button.
                    GlowButton {
                        app: loginView.app
                        visible: loginView.resetStage === "done"
                        Layout.fillWidth: true
                        text: "Back to sign in"
                        onClicked: loginView.leaveReset()
                    }

                    Controls.Label {
                        textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                        Layout.fillWidth: true
                        horizontalAlignment: Text.AlignHCenter
                        wrapMode: Text.WordWrap
                        visible: loginView.resetStatus.length > 0
                        text: loginView.resetStatus
                        // "done" is good news, "Checking <host>…" is progress; anything else while idle is a problem.
                        color: (loginView.busy || loginView.resetStage.length > 0 || loginView.resetMode === "checking")
                               ? Th.loginHintFg : Th.loginErrorFg
                        font.pointSize: Kirigami.Theme.smallFont.pointSize
                    }

                    Controls.Label {
                        visible: loginView.resetStage !== "done"
                        Layout.fillWidth: true
                        horizontalAlignment: Text.AlignHCenter
                        text: "Back to sign in"
                        color: backLinkArea.containsMouse ? Th.loginLinkFgHover : Th.loginLinkFg
                        font.pointSize: Kirigami.Theme.smallFont.pointSize
                        font.underline: backLinkArea.containsMouse
                        MouseArea {
                            id: backLinkArea
                            anchors.fill: parent
                            hoverEnabled: true
                            cursorShape: Qt.PointingHandCursor
                            enabled: !loginView.busy
                            onClicked: loginView.leaveReset()
                        }
                    }
                }

                // ---- Status line ---------------------------------------------
                Controls.Label {
                    textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                    Layout.fillWidth: true
                    horizontalAlignment: Text.AlignHCenter
                    wrapMode: Text.WordWrap
                    visible: !loginView.showReset && !loginView.showCreate && (loginView.status.length > 0 || (loginView.busy && loginView.slow))
                    text: (loginView.busy && loginView.slow) ? "Still trying to reach " + loginView.homeserver + "…" : loginView.status
                    // 10.1 (TH-FIRST-09 / TH-FIRST-20): only a FAILURE is red, from the app palette — "Signed out"
                    // and "sign in again" are information, not errors.
                    color: loginView.busy ? Th.loginHintFg
                         : (/^(signed out|you were signed out|your saved session)/i.test(loginView.status) ? Th.loginHintFg : Th.loginErrorFg)
                    font.pointSize: Kirigami.Theme.smallFont.pointSize
                }
            }
        }
    }



    // Submit helper — read both fields at call time (works regardless of bindings).
    function tryLogin() {
        if (usernameField.text.length > 0 && passwordField.text.length > 0 && !loginView.busy && loginView.homeserver.trim().length > 0)
            loginView.submit(usernameField.text, passwordField.text)
    }

    // 10.4: the create face — everything filled in, passwords agree, not mid-request.
    readonly property bool createReady: !loginView.busy && loginView.homeserver.trim().length > 0
        && inviteCodeField.text.trim().length > 0 && newUserField.text.trim().length > 0
        && newPwField.text.length >= 8 && newPwField.text === newPw2Field.text
    function tryCreate() {
        if (loginView.createReady)
            loginView.createAccount(inviteCodeField.text.trim(), newUserField.text.trim(), newPwField.text, displayNameField.text.trim())
    }
    // Back to sign-in; the typed secrets go with it (same reason as leaveReset).
    function leaveCreate() {
        newPwField.text = ""
        newPw2Field.text = ""
        loginView.showCreate = false
        loginView.createCancelled()
        usernameField.focusField()
    }
    // The account exists and the normal sign-in is running: show that on the sign-in face, with
    // the new username filled in so a failed sign-in can simply be retried there.
    onRegisterStageChanged: {
        if (loginView.registerStage === "created") {
            usernameField.text = newUserField.text.trim()
            newPwField.text = ""
            newPw2Field.text = ""
            loginView.showCreate = false
        }
    }

    function tryReset() {
        if (!loginView.busy && loginView.resetByEmail && resetEmailField.text.indexOf("@") > 0)
            loginView.resetRequested(resetEmailField.text)
    }

    function tryResetComplete() {
        if (!loginView.busy && resetPwField.text.length >= 8
                && resetPwField.text === resetPw2Field.text)
            loginView.resetCompleted(resetPwField.text)
    }

    // Leave the reset face and drop the typed secrets on the way out — a new
    // password sitting in a field behind a card you've navigated away from is
    // exactly the kind of thing that ends up in a screenshot.
    function leaveReset() {
        resetPwField.text = ""
        resetPw2Field.text = ""
        resetCheckT.stop()
        loginView.showReset = false
        loginView.resetCancelled()
        passwordField.focusField()
    }
}
