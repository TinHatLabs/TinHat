// FallbackTheme — the non-colour metrics (radii, paddings, durations) and small helpers for components that take
// `property var app` and may run without it. Every colour comes from the Th singleton (docs/MIGRATING-THEME.md).
import QtQuick
import dev.tinhat.app

QtObject {
    readonly property string baseFontFamily: ""
    readonly property var font: ({ family: "" })
    readonly property real radiusSm: 5
    readonly property real radius: 8
    readonly property real radiusLg: 12
    readonly property real radiusXl: 16
    readonly property real pad: 8
    readonly property real padLg: 12
    // 11.0 (GPU.md §1): the duration token's steps follow the app-wide motion switch (0 while it is off)
    readonly property int dur: Motion.dur
    readonly property int durShort: Motion.durShort
    readonly property int durSlow: Motion.durSlow
    readonly property int durLong: Motion.durLong
    readonly property int durVeryLong: Motion.durVeryLong
    readonly property bool animate: Motion.animate
    function senderColor(s) { return Th.accentBright }
    function localpart(s) { return s ? String(s).replace(/^@/, "").split(":")[0] : "" }
}
