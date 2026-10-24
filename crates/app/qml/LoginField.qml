// LoginField — a rounded, icon-led text field with a warm focus glow, used on the
// login card. Its colours are the login.field.* tokens of the active theme (Th).
//
// Exposes `text`, fires `textEdited(text)` on every keystroke and `accepted()` on
// Enter (so the parent can chain focus / submit). `focusField()` programmatically
// focuses the inner field. `enabled` propagates from the layout to the inner field.
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the login.field.* tokens

ColumnLayout {
    id: field
    required property var app   // ApplicationWindow (theme tokens); set by parent
    FallbackTheme { id: fallbackTheme }
    readonly property var theme: app || fallbackTheme   // sizes and motion (radiusLg, dur)

    property string label: ""
    property string icon: ""
    property string placeholder: ""
    property bool password: false
    property bool showPassword: false   // eye-toggle state (password fields only)
    property string prefix: ""          // 10.4: fixed text drawn before what you type (the invite code's "TH-")
    property alias text: input.text

    signal textEdited(string text)
    signal accepted()

    function focusField() { input.forceActiveFocus() }

    spacing: 3

    Controls.Label {
        text: field.label
        color: Th.loginFieldLabelFg
        font.pointSize: Kirigami.Theme.smallFont.pointSize
        font.bold: true
        Layout.leftMargin: 4
    }

    Rectangle {
        id: frame
        Layout.fillWidth: true
        implicitHeight: Kirigami.Units.gridUnit * 2.2
        radius: field.theme.radiusLg
        color: input.activeFocus ? Th.loginFieldBgFocus : Th.loginFieldBg
        border.width: input.activeFocus ? 2 : 1
        border.color: input.activeFocus ? Th.loginFieldBorderFocus : Th.loginFieldBorder
        Behavior on border.color { ColorAnimation { duration: field.theme.dur } }
        Behavior on color { ColorAnimation { duration: field.theme.dur } }

        // Warm focus halo — a soft ring that fades in when the field is focused.
        Rectangle {
            anchors.fill: parent
            anchors.margins: -3
            radius: parent.radius + 3
            color: "transparent"
            border.width: 3
            border.color: Th.loginFieldGlowFocus
            opacity: input.activeFocus ? 0.22 : 0.0
            Behavior on opacity { NumberAnimation { duration: Motion.dur } }
        }

        RowLayout {
            anchors.fill: parent
            anchors.leftMargin: Kirigami.Units.smallSpacing
            anchors.rightMargin: Kirigami.Units.smallSpacing
            spacing: Kirigami.Units.smallSpacing

            Kirigami.Icon {
                source: field.icon
                visible: field.icon.length > 0
                color: input.activeFocus ? Th.loginFieldIconFocus : Th.loginFieldIcon
                implicitWidth: Kirigami.Units.iconSizes.small
                implicitHeight: Kirigami.Units.iconSizes.small
            }

            Controls.Label {
                visible: field.prefix.length > 0
                text: field.prefix
                color: input.activeFocus ? Th.loginFieldPrefixFgFocus : Th.loginFieldPrefixFg
                font.bold: true
                font.family: Th.fontMono
                Layout.rightMargin: -2
            }

            Controls.TextField {
                id: input
                Layout.fillWidth: true
                Layout.fillHeight: true
                placeholderText: field.placeholder
                echoMode: (field.password && !field.showPassword) ? TextInput.Password : TextInput.Normal
                color: Th.loginFieldFg
                placeholderTextColor: Th.loginFieldPlaceholder
                leftPadding: 0
                rightPadding: 0
                verticalAlignment: TextInput.AlignVCenter
                background: null // the frame is the visual; keep the input chromeless
                onTextChanged: field.textEdited(text)
                onAccepted: field.accepted()
            }

            // Show/hide-password eye toggle (password fields only).
            Item {
                visible: field.password
                Layout.preferredWidth: Kirigami.Units.iconSizes.smallMedium + 4
                Layout.fillHeight: true

                // Soft circular hover wash behind the eye.
                Rectangle {
                    anchors.centerIn: parent
                    width: Kirigami.Units.iconSizes.smallMedium + 6
                    height: width
                    radius: width / 2
                    color: Th.loginFieldEyeBgHover
                    opacity: eyeMa.containsMouse ? (eyeMa.pressed ? 0.30 : 0.16) : 0.0
                    Behavior on opacity { NumberAnimation { duration: field.theme.dur } }
                }
                Kirigami.Icon {
                    anchors.centerIn: parent
                    implicitWidth: Kirigami.Units.iconSizes.small
                    implicitHeight: Kirigami.Units.iconSizes.small
                    source: field.showPassword ? "view-hidden-symbolic" : "view-visible-symbolic"
                    color: eyeMa.containsMouse ? Th.loginFieldEyeIconHover : Th.loginFieldEyeIcon
                }
                MouseArea {
                    id: eyeMa
                    anchors.fill: parent
                    hoverEnabled: true
                    cursorShape: Qt.PointingHandCursor
                    onClicked: field.showPassword = !field.showPassword
                }
                Controls.ToolTip.text: field.showPassword ? "Hide password" : "Show password"
                Controls.ToolTip.visible: eyeMa.containsMouse
                Controls.ToolTip.delay: 400
            }
        }
    }
}
