// ThemeChecker.qml — the checkerboard behind translucent colours in the theme editor (THEMES-UI.md §13.8–§13.9): 6 px
// squares of settings.themeEditor.swatch.checker, clipped to `radius`. Painted once (and again when that colour or the
// size changes) — static, nothing animates (GPU.md).
import QtQuick
import dev.tinhat.app   // Th: settings.themeEditor.swatch.checker

Canvas {
    id: cv
    property real radius: 0
    property int cell: 6
    readonly property color ink: Th.settingsThemeEditorSwatchChecker
    onInkChanged: requestPaint()
    onPaint: {
        var c = getContext("2d")
        c.reset()
        c.clearRect(0, 0, width, height)
        if (width <= 0 || height <= 0) return
        c.save()
        var r = Math.min(cv.radius, width / 2, height / 2)
        c.beginPath()
        c.moveTo(r, 0); c.lineTo(width - r, 0); c.arcTo(width, 0, width, r, r)
        c.lineTo(width, height - r); c.arcTo(width, height, width - r, height, r)
        c.lineTo(r, height); c.arcTo(0, height, 0, height - r, r)
        c.lineTo(0, r); c.arcTo(0, 0, r, 0, r)
        c.closePath()
        c.clip()
        c.fillStyle = cv.ink
        for (var y = 0; y < height; y += cv.cell)
            for (var x = ((y / cv.cell) % 2) * cv.cell; x < width; x += 2 * cv.cell)
                c.fillRect(x, y, cv.cell, cv.cell)
        c.restore()
    }
    onWidthChanged: requestPaint()
    onHeightChanged: requestPaint()
}
