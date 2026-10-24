// FoxScrollBar — a reusable, theme-agnostic ScrollBar for every ListView /
// ScrollView / Flickable in Tin Hat. Warm rounded thumb at low alpha
// (pal.accent ~0.35), auto-hides when idle, and grows + brightens on hover
// /drag. Attach with:  Controls.ScrollBar.vertical: FoxScrollBar {}
//
// Theme-agnostic: every colour derives from pal.* (the active palette via
// app || FallbackTheme), so OLED / Goose / Furry all get the same crafted
// scrollbar.
import QtQuick
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the active theme (docs/MIGRATING-THEME.md)

Controls.ScrollBar {
    id: sb
    // Optional (NOT required): the documented attach-usage `ScrollBar.vertical:
    // FoxScrollBar {}` can't set `app`; the fallback covers it.
    property var app: null
    FallbackTheme { id: fallbackTheme }
    readonly property var theme: app || fallbackTheme   // durations only; colours come from Th
    // Thin by default, a touch wider while interacted with — feels alive.
    readonly property real thinW: 6
    readonly property real fatW: 10
    // 9.3.2: the chat timeline wants a bar that is visibly THERE (field report: "there is no scroll
    // bar") — an idle alpha above the default whisper and a faint track behind the thumb.
    // The defaults keep every other list exactly as it was.
    property real idleAlpha: Th.scrollbarThumb.a   // the scrollbar.thumb token's alpha unless a host asks for more
    property bool track: false
    // 10.0.3: a host may flag "the pointer is near my edge" — the bar shows while it is.
    property bool edgeHovered: false
    // Idle when there's nothing to scroll, or after the auto-hide delay. `active` is Qt's own
    // "the flickable is moving / the bar is in use" flag (it drops a moment after scrolling stops).
    // NOTE: must NOT be named `active` — Controls.ScrollBar.active is FINAL.
    readonly property bool shown: sb.size < 1.0
        && (sb.hovered || sb.pressed || sb.active || sb.edgeHovered || sb.policy === Controls.ScrollBar.AlwaysOn)

    // Auto-hide: ScrollBar already flips `active` to false a moment after
    // scrolling stops; we drive opacity off that with a gentle fade.
    policy: Controls.ScrollBar.AsNeeded
    minimumSize: 0.08
    padding: 2
    visible: sb.size < 1.0

    contentItem: Rectangle {
        implicitWidth: (sb.hovered || sb.pressed) ? sb.fatW : sb.thinW
        implicitHeight: (sb.hovered || sb.pressed) ? sb.fatW : sb.thinW
        radius: width / 2
        color: sb.pressed
            ? Th.scrollbarThumbPressed
            : sb.hovered
                ? Th.scrollbarThumbHover
                : Qt.alpha(Th.scrollbarThumb, sb.idleAlpha)
        opacity: sb.shown ? 1.0 : 0.0

        Behavior on color { ColorAnimation { duration: sb.theme.dur; easing.type: Easing.OutCubic } }
        Behavior on opacity { NumberAnimation { duration: sb.theme.durSlow; easing.type: Easing.OutCubic } }
        Behavior on implicitWidth { NumberAnimation { duration: sb.theme.dur; easing.type: Easing.OutCubic } }
        Behavior on implicitHeight { NumberAnimation { duration: sb.theme.dur; easing.type: Easing.OutCubic } }
    }

    // No track chrome by default — the thumb floats over content (Element-style); `track`
    // draws a faint full-length rail behind it.
    background: Rectangle {
        visible: sb.track && sb.shown
        implicitWidth: sb.fatW
        implicitHeight: sb.fatW
        radius: width / 2
        color: Th.scrollbarTrack
    }
}
