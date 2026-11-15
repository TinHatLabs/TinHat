// ScreenFit — A33.3 / A33.4: is a remembered window position still on a connected screen, and where the next
// pop-out goes. Logic only (WindowMemory.qml, AppCore.qml's pop-outs and WatchPartyWindow.qml use it); tested by
// scripts/screen-fit-test.qml (run by scripts/qml-lint.sh). A screen is { virtualX, virtualY, width, height }
// (Qt.application.screens). Desktop only: a browser places its own windows.
import QtQml

QtObject {
    // A window counts as on screen when a grabbable piece of its top edge is: 80 px wide and 20 px tall of the band
    // where the title bar sits. A screen that was unplugged (or a smaller one) fails this, and the window is left
    // for the system to place. No screen list (can't tell) = keep the old behaviour.
    function onScreen(x, y, w, h, screens) {
        if (!screens || screens.length === 0) return true
        for (var i = 0; i < screens.length; i++) {
            var s = screens[i]
            var ix = Math.min(x + w, s.virtualX + s.width) - Math.max(x, s.virtualX)
            var iy = Math.min(y + 40, s.virtualY + s.height) - Math.max(y, s.virtualY)
            if (ix >= 80 && iy >= 20) return true
        }
        return false
    }
    // A new pop-out takes the remembered spot, or the first spot in 32 px steps down-right (a cascade), then up-left,
    // that is on a screen and that no open pop-out sits on (`taken`: their [{x, y}], within 16 px counts as the same
    // spot), so they never stack on top of each other; the remembered spot only when nothing is free.
    function nextSpot(x, y, w, h, taken, screens) {
        for (var k = 0; k < 64; k++) {
            var step = 32 * (k < 32 ? k : 31 - k)   // 0, +32 … +992, then -32 … -1024
            var cx = x + step, cy = y + step
            if (k > 0 && !onScreen(cx, cy, w, h, screens)) continue
            var free = true
            for (var i = 0; i < (taken ? taken.length : 0); i++)
                if (Math.abs(taken[i].x - cx) < 16 && Math.abs(taken[i].y - cy) < 16) { free = false; break }
            if (free) return { x: cx, y: cy }
        }
        return { x: x, y: y }
    }
}
