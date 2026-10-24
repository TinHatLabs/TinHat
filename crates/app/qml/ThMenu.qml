// ThMenu — a Menu as wide as its widest row under every style (fix/sliders-volume, 2026-09-28). The menus that carry
// slider rows use it: the per-person volume menu (ParticipantVolumeMenu: a call tile's right-click / ⋮, the left bar's
// participant bubbles) and a stream tile's controls menu (CallIsland qualityMenu).
//
// The Basic style's Menu.qml (Windows, Qt 6.8 — and 6.11) is a fixed 200 px whatever its rows need: its ListView has no
// implicit width, so the dial rows (label 118 + slider 150 + % + mute ≈ 370 px) ran past the menu's edge and were
// clipped — no slider, no percentage, no mute button (field report: "windows right click for volume sliders are bugged").
// Breeze's Menu.qml sizes its ListView to the widest VISIBLE row (`visibleChildren … implicitWidth`), and the web's Menu
// is max-content (min 366 / 260 there, src/features/callui/menus.tsx) — this is that same rule, so Breeze is unchanged
// (its implicitWidth already is this width) and Basic now matches both. No binding loop: a row's implicitWidth never
// depends on the width the menu gives it (MenuItem / Label / RowLayout implicit sizes come from their content).
import QtQuick
import QtQuick.Controls as Controls

Controls.Menu {
    readonly property real rowsWidth: (contentItem && contentItem.contentItem)
        ? contentItem.contentItem.visibleChildren.reduce((w, c) => Math.max(w, c.implicitWidth), 0) : 0
    width: Math.max(implicitWidth, rowsWidth + leftPadding + rightPadding)
}
