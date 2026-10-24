// TinHatLoader.qml — the Tin Hat mark (a faceted tin-foil hat): 21 facets whose grey shifts by
// amplitude × sin(2π(phase − offset)) × strength per channel. 11.0 (GPU.md §2.5, R11 — the web's ui/TinHatLoader.tsx):
// NO loop any more (the light used to sweep round the facets once per 2.4 s for as long as anything loaded). The light
// moves ONE step — an eighth of a turn at lightStrength 3.2 — per real progress event (a changing `progress`, e.g.
// the boot stage's percent), at most one per 250 ms and only while `animate`; a change while it is off is dropped,
// not queued. At rest (no progress yet, not running) it shows the resting shade (phase 0, strength 1).
import QtQuick
import QtQuick.Shapes
import dev.tinhat.app   // Motion: the app-wide motion switch

Item {
    id: root
    property bool running: true
    property bool reducedMotion: false          // kept for callers; the motion switch (animate) already includes it
    property bool animate: Motion.animate && !root.reducedMotion
    property var progress: undefined            // changes on every real progress event
    property real lightStrength: 3.2
    property int step: -1                       // -1 = at rest
    property var _last: undefined
    property double _lastAt: -1e9
    readonly property real side: Math.min(width, height)
    implicitWidth: 64
    implicitHeight: 64
    Accessible.role: Accessible.Indicator
    Accessible.name: running ? qsTr("Loading") : qsTr("Tin Hat")

    onProgressChanged: {
        if (!root.running || root.progress === undefined || root.progress === root._last) return
        root._last = root.progress
        if (!root.animate) return
        var now = Date.now()
        if (now - root._lastAt < 250) return          // BUSY_STEP_MS: ≤ 4 steps/s
        root._lastAt = now
        root.step = root.step + 1
    }
    readonly property bool _lit: root.running && root.step >= 0
    readonly property real _phase: root._lit ? ((root.step + 1) % 8) / 8 : 0
    readonly property real _strength: root._lit ? Math.max(0, Math.min(5, root.lightStrength)) : 1

    function litColor(hex, amplitude, offset) {
        const delta = amplitude * Math.sin(2 * Math.PI * (root._phase - offset)) * root._strength;
        function channel(start) {
            return Math.max(0, Math.min(255, parseInt(hex.substring(start, start + 2), 16) + delta)) / 255;
        }
        return Qt.rgba(channel(1), channel(3), channel(5), 1);
    }

    Item {
        width: 512
        height: 512
        x: (root.width - root.side) / 2
        y: (root.height - root.side) / 2
        scale: root.side / 512
        transformOrigin: Item.TopLeft
        Shape {
            x: -14.384
            y: 62.96
            scale: 1.046
            transformOrigin: Item.TopLeft
            antialiasing: true
            ShapePath {
                strokeWidth: -1
                fillColor: root.litColor("#CBD3DA", 0, 0)
                PathSvg { path: "M 308 40 L 313 103 L 373 178 L 409 258 L 267 245 L 114 231 L 181 139 L 256 70 Z" }
            }
            ShapePath {
                strokeWidth: -1
                fillColor: root.litColor("#CBD3DA", 0, 0)
                PathSvg { path: "M 52 218 L 104 201 L 152 217 L 280 231 L 405 242 L 465 267 L 426 294 L 267 277 L 85 264 Z" }
            }
            ShapePath {
                strokeWidth: -1
                fillColor: root.litColor("#BAC3CC", 9, 0.16)
                PathSvg { path: "M 52 218 L 104 201 L 152 217 L 280 231 L 405 242 L 465 267 L 413 276 L 267 252 L 119 239 Z" }
            }
            ShapePath {
                strokeWidth: -1
                fillColor: root.litColor("#8F9CA9", 4, 0.12)
                PathSvg { path: "M 70 216 L 104 207 L 123 229 Z" }
            }
            ShapePath {
                strokeWidth: -1
                fillColor: root.litColor("#F5F7F9", 6, 0.07)
                PathSvg { path: "M 52 218 L 104 201 L 104 207 L 70 216 L 123 229 Z" }
            }
            ShapePath {
                strokeWidth: -1
                fillColor: root.litColor("#8E9CA8", 5, 0.6)
                PathSvg { path: "M 397 242 L 447 263 L 413 265 Z" }
            }
            ShapePath {
                strokeWidth: -1
                fillColor: root.litColor("#F8FAFC", 5, 0.04)
                PathSvg { path: "M 308 40 L 256 70 L 269 107 Z" }
            }
            ShapePath {
                strokeWidth: -1
                fillColor: root.litColor("#E2E7EC", 12, 0.1)
                PathSvg { path: "M 256 70 L 181 139 L 269 107 Z" }
            }
            ShapePath {
                strokeWidth: -1
                fillColor: root.litColor("#D3DAE1", 15, 0.17)
                PathSvg { path: "M 181 139 L 114 231 L 267 245 Z" }
            }
            ShapePath {
                strokeWidth: -1
                fillColor: root.litColor("#EEF2F5", 11, 0.3)
                PathSvg { path: "M 181 139 L 269 107 L 267 245 Z" }
            }
            ShapePath {
                strokeWidth: -1
                fillColor: root.litColor("#A9B3BE", 15, 0.44)
                PathSvg { path: "M 269 107 L 308 40 L 309 101 L 367 178 Z" }
            }
            ShapePath {
                strokeWidth: -1
                fillColor: root.litColor("#BFC8D0", 18, 0.52)
                PathSvg { path: "M 269 107 L 367 178 L 267 245 Z" }
            }
            ShapePath {
                strokeWidth: -1
                fillColor: root.litColor("#8F9DAA", 14, 0.61)
                PathSvg { path: "M 367 178 L 405 257 L 267 245 Z" }
            }
            ShapePath {
                strokeWidth: -1
                fillColor: root.litColor("#CDD6DE", 11, 0.7)
                PathSvg { path: "M 308 40 L 313 103 L 373 178 L 409 258 L 405 257 L 367 178 L 309 101 Z" }
            }
            ShapePath {
                strokeWidth: -1
                fillColor: root.litColor("#A8B3BF", 8, 0.2)
                PathSvg { path: "M 52 218 L 114 231 L 85 264 Z" }
            }
            ShapePath {
                strokeWidth: -1
                fillColor: root.litColor("#F0F4F7", 8, 0.37)
                PathSvg { path: "M 114 231 L 267 245 L 85 264 Z" }
            }
            ShapePath {
                strokeWidth: -1
                fillColor: root.litColor("#CAD3DC", 15, 0.45)
                PathSvg { path: "M 85 264 L 267 245 L 267 277 Z" }
            }
            ShapePath {
                strokeWidth: -1
                fillColor: root.litColor("#D7DFE5", 11, 0.64)
                PathSvg { path: "M 267 245 L 405 257 L 426 294 Z" }
            }
            ShapePath {
                strokeWidth: -1
                fillColor: root.litColor("#9BAAB7", 9, 0.72)
                PathSvg { path: "M 267 245 L 426 294 L 267 277 Z" }
            }
            ShapePath {
                strokeWidth: -1
                fillColor: root.litColor("#ADBCC8", 10, 0.79)
                PathSvg { path: "M 405 257 L 465 267 L 426 294 Z" }
            }
            ShapePath {
                strokeWidth: -1
                fillColor: root.litColor("#F1F6F6", 6, 0.85)
                PathSvg { path: "M 405 257 L 465 267 L 452 270 L 408 262 Z" }
            }
        }
    }
}
