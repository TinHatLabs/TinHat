// GlowButton — the primary call-to-action button for Tin Hat (Send, Sign in, Save,
// Join…: exactly ONE per surface). Ports the web fork's .mx_AccessibleButton_kind_primary
// box-shadow (rest: 0 2px 12px accent .35, hover: 0 4px 18px accent .55) plus a subtle
// hover lift and press settle. Theme-agnostic: fill/glow derive from the app accent, the
// label uses pal.onAccent. `busy: true` swaps the label for a spinner (Sign in, Save).
// Keyboard: tab-focusable with a visible accentBright focus ring (custom backgrounds
// don't get the Breeze ring for free).
import QtQuick
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the active theme (docs/MIGRATING-THEME.md)

Controls.Button {
    id: btn
    required property var app   // ApplicationWindow (theme tokens); set by parent
    property bool busy: false   // show a spinner instead of the label (still themed/lit)
    FallbackTheme { id: fallbackTheme }
    readonly property var theme: app || fallbackTheme

    // Hover-lift offset, eased.
    readonly property real lift: btn.down ? 0 : (btn.hovered ? -2 : 0)
    readonly property color accent: btn.down
        ? Th.buttonCtaBgPressed
        : (btn.hovered ? Th.buttonCtaBgHover : Th.buttonCtaBg)

    leftPadding: theme.padLg; rightPadding: theme.padLg
    topPadding: theme.pad; bottomPadding: theme.pad
    font.bold: true
    icon.color: Th.buttonCtaFg
    focusPolicy: Qt.StrongFocus
    // Disabled: drop the chroma, no glow (0.5 = the app-wide disabled opacity).
    opacity: enabled ? 1.0 : 0.5

    contentItem: Item {
        implicitWidth: btn.busy ? spinner.implicitWidth : row.implicitWidth
        implicitHeight: btn.busy ? spinner.implicitHeight : row.implicitHeight
        RowLayoutShim {
            id: row
            anchors.centerIn: parent
            visible: !btn.busy
            spacing: Kirigami.Units.smallSpacing
            labelText: btn.text
            labelColor: Th.buttonCtaFg
            labelFont: btn.font   // the wrapper Item breaks parent-font inheritance
            iconSource: btn.icon.name
        }
        ThBusyIndicator {   // 11.0 (GPU.md §2.5): still
            id: spinner
            anchors.centerIn: parent
            visible: btn.busy
            running: btn.busy
            size: 25
        }
    }

    background: Item {
        // The pill itself, with the soft accent glow (the ported box-shadow: rest 0 2px 12px, hover 0 4px 18px).
        // 11.0 (GPU.md §2.9, R15): the glow is Kirigami's static SDF shadow drawn with the pill in one pass — it was a
        // live MultiEffect blur (an offscreen layer + blur passes) on the always-visible Send button.
        Kirigami.ShadowedRectangle {
            id: pill
            anchors.fill: parent
            anchors.topMargin: btn.lift
            anchors.bottomMargin: -btn.lift
            radius: btn.theme.radius
            color: btn.accent
            border.width: btn.visualFocus ? 2 : 0
            border.color: Th.buttonCtaBorderFocus
            shadow.size: btn.enabled ? (btn.hovered ? 18 : 12) : 0
            shadow.yOffset: btn.hovered ? 4 : 2
            shadow.color: btn.hovered ? Th.buttonCtaGlowHover : Th.buttonCtaGlow
            Behavior on color { ColorAnimation { duration: btn.theme.dur; easing.type: Easing.OutCubic } }
            Behavior on anchors.topMargin { NumberAnimation { duration: btn.theme.dur; easing.type: Easing.OutCubic } }
        }
    }

    // Space is handled natively by AbstractButton; Return is not — add it (a focused
    // primary CTA should accept Enter, e.g. Sign in).
    Keys.onReturnPressed: if (enabled && !busy) clicked()
}
