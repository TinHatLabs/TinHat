// AmbienceLayer.qml — the room's shared background + particle layer behind the message list (docs/AMBIENCE.md; the
// web's ambience/layer.tsx). The scene (gradient + glow), your own image when it shows, an art slot (game art), the
// theme scrim that keeps text on the background readable, and the particles (AmbienceItem: the shared simulation's
// mesh in one scene-graph node). The particles meet the panels of the list — every delegate's ambRects(): bubbles,
// names, day pills, reactions — and never draw inside one. When effects run: AmbienceController.effectsState
// (Settings → Appearance → Performance; hidden / minimised always pauses).
import QtQuick
import QtQuick.Window
import QtQuick.Shapes
import dev.tinhat.app

Item {
    id: amb
    required property var app
    /// the parsed background (AmbienceController.parse): {kind, preset, effect, image, by}
    property var background: ({ kind: "unset", preset: null, effect: "none", image: null, by: "" })
    /// kind "image": a file URL of the decrypted image when imageShown said yes; "" = the preset fallback
    property string imageSource: ""
    /// its colour cells over every pixel (RoomController.backgroundImageCells); the image shows only with them
    property string imageCells: ""
    /// the message list (its delegates answer ambRects(target, out))
    property Item list: null
    /// the main view runs particles; pop-out chats show the still scene
    property bool particles: true
    /// game art: its layer goes here (above the scene, below the scrim + particles)
    default property alias art: artSlot.data
    property bool artShown: false

    readonly property var ambCtl: app ? app.ambience : null
    readonly property Item effectItem: fx
    readonly property var preset: background && background.preset ? background.preset : ""
    readonly property bool shown: !!ambCtl && app.mChatBackgrounds && (preset.length > 0 || artShown)
    readonly property string mode: ambCtl ? ambCtl.themeMode(String(Th.timelineBg)) : "dark"
    readonly property var look: ambCtl && preset.length > 0 ? JSON.parse(ambCtl.lookJson(preset, mode)) : null
    // the text drawn straight on the background: the timeline's own + the name colours in view (the scrim keeps them all)
    property var nameColours: []
    readonly property var fgs: JSON.stringify([String(Th.timelineStartFg), String(Th.timelineEmptyFg), String(Th.timelineNewLineFg)].concat(nameColours))

    readonly property real scrim: {
        if (!ambCtl || !shown) return 0
        if (imageSource.length > 0 && imageCells.length > 0 && background.kind === "image")
            return ambCtl.scrimForCells(imageCells, String(Th.timelineBg), fgs)
        return look ? ambCtl.scrimFor(preset, mode, String(Th.timelineBg), fgs) : 0
    }
    readonly property string fxState: !ambCtl || !shown ? "off"
        : ambCtl.effectsState(app.mChatBackgrounds, app.mChatEffects, app.mChatEffectsPause, app.mCallUnfocusedSaver, Motion.reducedMotion,
                           ambCtl.liveWinState(Motion.realWinState === "hidden", Motion.realWinState === "focused"),
                           GraphicsInfo.api !== GraphicsInfo.Software && !fx.cpuDevice, particles ? String(background.effect) : "none",
                           app.mInCall === true || app.mConnecting === true)   // both apps: no particles in a call (ambience_ctl call_pause)
    onFxStateChanged: {
        console.log("[ambience] " + (shown ? preset + " / " + background.effect : "none") + ": " + fxState)
        if (fxState === "off") fx.clear()
    }

    visible: shown
    clip: true

    // the scene: a vertical gradient (three stops) …
    Rectangle {
        anchors.fill: parent
        visible: !!amb.look
        gradient: Gradient {
            GradientStop { position: amb.look ? amb.look.stops[0][0] : 0; color: amb.look ? amb.look.stops[0][1] : "transparent" }
            GradientStop { position: amb.look ? amb.look.stops[1][0] : 0.5; color: amb.look ? amb.look.stops[1][1] : "transparent" }
            GradientStop { position: amb.look ? amb.look.stops[2][0] : 1; color: amb.look ? amb.look.stops[2][1] : "transparent" }
        }
    }
    // … and a glow: an ellipse fading linearly to nothing (a circle of the x radius, squashed to the y radius)
    Item {
        id: glow
        visible: !!amb.look
        readonly property real r: amb.look ? Math.max(1, amb.look.glow.rx * amb.width) : 1
        readonly property real cx: amb.look ? amb.look.glow.x * amb.width : 0
        readonly property real cy: amb.look ? amb.look.glow.y * amb.height : 0
        x: cx - r; y: cy - r; width: 2 * r; height: 2 * r
        transform: Scale { origin.x: glow.r; origin.y: glow.r; yScale: amb.look ? (amb.look.glow.ry * amb.height) / glow.r : 1 }
        Shape {
            anchors.fill: parent
            preferredRendererType: Shape.GeometryRenderer
            ShapePath {
                strokeWidth: -1
                fillGradient: RadialGradient {
                    centerX: glow.r; centerY: glow.r; centerRadius: glow.r
                    focalX: glow.r; focalY: glow.r
                    GradientStop { position: 0; color: amb.look ? Qt.alpha(amb.look.glow.color, amb.look.glow.a) : "transparent" }
                    GradientStop { position: 1; color: amb.look ? Qt.alpha(amb.look.glow.color, 0) : "transparent" }
                }
                startX: 0; startY: 0
                PathLine { x: glow.width; y: 0 }
                PathLine { x: glow.width; y: glow.height }
                PathLine { x: 0; y: glow.height }
                PathLine { x: 0; y: 0 }
            }
        }
    }
    // your own image (only when it showed: else the scene above is the fallback)
    Image {
        id: img
        anchors.fill: parent
        visible: amb.background.kind === "image" && amb.imageSource.length > 0 && amb.imageCells.length > 0 && status === Image.Ready
        source: amb.background.kind === "image" ? amb.imageSource : ""
        fillMode: Image.PreserveAspectCrop
        asynchronous: true
        cache: false
        sourceSize.width: 2560
    }
    Item { id: artSlot; anchors.fill: parent }
    Rectangle {
        anchors.fill: parent
        color: Th.timelineBg
        opacity: amb.scrim
        visible: amb.scrim > 0
    }

    // the particles
    AmbienceItem {
        id: fx
        anchors.fill: parent
        visible: amb.fxState !== "off"
        effect: amb.fxState === "off" ? "none" : String(amb.background.effect)
        mode: amb.mode
        density: amb.width < 600 ? 0.6 : 1
    }
    property bool _dirty: true
    property int _frames: 0
    // paused: the last frame would stay where the panels WERE — drop it when they move (the next running frame redraws)
    function _moved() { _dirty = true; if (fxState === "paused") fx.clear() }
    function _collect() {
        var out = []
        if (list && list.contentItem) {
            var kids = list.contentItem.children
            var top = list.contentY - 40, bottom = list.contentY + list.height + 40
            for (var i = 0; i < kids.length; i++) {
                var c = kids[i]
                if (!c.visible || typeof c.ambRects !== "function") continue
                if (c.y + c.height < top || c.y > bottom) continue
                c.ambRects(fx, out)
            }
        }
        fx.setColliders(JSON.stringify(out))
    }
    function _names() {
        var set = {}, out = []
        if (list && list.contentItem) {
            var kids = list.contentItem.children
            for (var i = 0; i < kids.length && out.length < 24; i++) {
                var c = kids[i]
                if (!c.visible || typeof c.ambNameColour !== "function") continue
                var h = c.ambNameColour()
                if (h.length > 0 && !set[h]) { set[h] = true; out.push(h) }
            }
        }
        out.sort()
        if (JSON.stringify(out) !== JSON.stringify(nameColours)) nameColours = out
    }
    Timer { id: namesTimer; interval: 400; onTriggered: amb._names() }
    Component.onCompleted: namesTimer.restart()
    Connections {
        target: amb.list
        function onContentYChanged() { amb._moved(); namesTimer.restart() }
        function onContentHeightChanged() { amb._moved() }
        function onCountChanged() { amb._moved(); namesTimer.restart() }
        function onWidthChanged() { amb._moved() }
        function onHeightChanged() { amb._moved() }
    }
    // The effect's clock. The particles RIDE ALONG frames the window draws anyway (scrolling, an animated picture, a
    // video tile): a step taken in afterAnimating is picked up by that same frame, so no extra repaint. Only when the
    // window would otherwise sit still does the Timer step them and so ask for a frame of its own, at the effect's rate
    // (30 a second; the old FrameAnimation drew 60). In a call there are no particles (call_pause).
    readonly property real fxPeriodMs: 1000 / (amb.ambCtl ? amb.ambCtl.effectFps(String(amb.background.effect)) : 30)
    property real _lastTickMs: 0
    property real _ownFrameMs: 0       // when the Timer last asked for a frame of its own
    property real _naturalFrameMs: 0   // when the window last drew a frame nobody here asked for (video, scrolling…)
    function _step(now) {
        var dt = _lastTickMs > 0 ? Math.min(0.1, (now - _lastTickMs) / 1000) : fxPeriodMs / 1000
        _lastTickMs = now
        // a panel can change size without the list noticing (an edit, a reaction): re-read them twice a second
        if (++amb._frames % 15 === 0) amb._dirty = true
        if (amb._dirty) { amb._dirty = false; amb._collect() }
        fx.tick(dt)
    }
    readonly property bool _running: amb.fxState === "run" && amb.visible
    on_RunningChanged: { _lastTickMs = 0; _naturalFrameMs = 0 }
    Connections {
        target: amb.Window.window
        enabled: amb._running
        function onAfterAnimating() {
            var now = Date.now()
            if (now - amb._ownFrameMs > 20) amb._naturalFrameMs = now
            if (now - amb._lastTickMs >= amb.fxPeriodMs - 4) amb._step(now)
        }
    }
    Timer {
        id: fxClock
        interval: Math.max(16, Math.round(amb.fxPeriodMs))
        repeat: true
        running: amb._running
        onTriggered: {
            // the window has drawn nothing of its own lately: this step asks for a frame itself
            var now = Date.now()
            if (now - amb._naturalFrameMs > 60 && now - amb._lastTickMs >= amb.fxPeriodMs - 4) { amb._ownFrameMs = now; amb._step(now) }
        }
    }
}
