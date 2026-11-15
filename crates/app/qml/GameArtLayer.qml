import QtQuick
import QtQuick.Shapes
import dev.tinhat.app

// GameArtLayer.qml — "Match the game" chat backgrounds (lane sun-gamebg): the desktop twin of Tin Hat Web's
// GameArtView (src/features/chat/gamebg/GameArtLayer.tsx). Goes in the chat background layer's art slot
// (AmbienceLayer.qml, sun-ambience): above its scene, below its scrim + particles.
//   • the full-size art from the person's own server (GameArtController: downloaded once, measured at full size);
//   • top-anchored, covering, but never past one art pixel per device pixel (fitJson = the shared fit_art): where
//     covering would upscale the art keeps its size and fades into the theme at its edges;
//   • a measured theme veil (the timeline's on-background text keeps 4.5:1) + a vignette; no blur;
//   • a slow crossfade when the game changes (none under reduce motion); nothing loaded or timed while hidden.
Item {
    id: gl
    required property var app
    property string roomId: ""
    /** the room's own dev.tinhat.chat.background CONTENT as JSON ("" = none set) */
    property string backgroundJson: ""
    /** "Show chat backgrounds" + "Game art backgrounds" */
    property bool backgroundsOn: true
    property bool gameArtOn: true
    /** the chat itself is hidden (a kept-alive conversation): no work, like a hidden window */
    property bool viewHidden: false
    /** the room's role (custom) name colours, as ChatBubble draws them (3-2: the veil measures them too) */
    property var roleColours: []
    /** the gallery: a local art file instead of a room ("" = follow the room) */
    property string previewFile: ""
    readonly property bool hasArt: artCtl.artUrl.length > 0
    readonly property string gameId: artCtl.gameId
    readonly property real shownVeil: artCtl.veil
    readonly property real fadeMs: Motion.reducedMotion || !Motion.animate ? 0 : 2400

    anchors.fill: parent
    clip: true
    visible: hasArt || slideA.opacity > 0 || slideB.opacity > 0

    GameArtController { id: artCtl }

    readonly property string themeMode: {
        var c = Th.timelineBg
        function lin(v) { return v <= 0.04045 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4) }
        var L = 0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b)
        return L > 0.4 ? "light" : (L < 0.002 ? "oled" : "dark")
    }
    function hex(c) {
        return "#" + [c.r, c.g, c.b].map(function (v) { var s = Math.round(v * 255).toString(16); return s.length < 2 ? "0" + s : s }).join("")
    }
    // every colour a person's NAME can have in this theme (AppCore.senderColor: the palette's 12, or the hue band)
    function nameColours() {
        var out = []
        if (Th.names === "palette") {
            for (var n = 1; n <= 12; n++) out.push(hex(Th.byId("name." + n)))
            return out
        }
        var dark = Th.surfaceWindow.hslLightness < 0.5   // the web's senderColor: hslLightness(surface.window)
        for (var i = 0; i < 56; i++) out.push(hex(Qt.hsla(0.02 + i / 360.0, 0.6, dark ? 0.7 : 0.42, 1)))
        for (var j = 0; j < 22; j++) out.push(hex(Qt.hsla(0.45 + j / 360.0, 0.6, dark ? 0.7 : 0.42, 1)))
        return out
    }
    // ChatBubble.senderHue: a role colour Qt can read is drawn as is (alpha over the timeline), else the usual name colour
    function pushRoles() {
        var out = []
        var bg = Th.timelineBg
        var list = gl.roleColours || []
        for (var i = 0; i < list.length; i++) {
            var c = Qt.color(String(list[i]))
            if (!c.valid) continue
            var h = hex(Qt.rgba(c.r * c.a + bg.r * (1 - c.a), c.g * c.a + bg.g * (1 - c.a), c.b * c.a + bg.b * (1 - c.a), 1))
            if (out.indexOf(h) < 0) out.push(h)
        }
        artCtl.setRoleColours(JSON.stringify(out.sort()))
    }
    function pushInk() {
        var texts = [hex(Th.timelineStartFg), hex(Th.timelineEmptyFg), hex(Th.timelineNewLineFg)].concat(nameColours())
        artCtl.setInk(hex(Th.timelineBg), JSON.stringify(texts), themeMode)
    }
    Component.onCompleted: {
        pushRoles()
        pushInk()
        artCtl.setPrefs(backgroundsOn, gameArtOn)
        artCtl.setBackground(backgroundJson)
        artCtl.setHidden(Motion.realWinState === "hidden" || viewHidden)
        if (previewFile.length > 0) artCtl.preview(previewFile)
        else artCtl.open(roomId)
    }
    onRoomIdChanged: if (previewFile.length === 0) artCtl.open(roomId)
    onPreviewFileChanged: previewFile.length > 0 ? artCtl.preview(previewFile) : artCtl.open(roomId)
    onBackgroundJsonChanged: artCtl.setBackground(backgroundJson)
    onBackgroundsOnChanged: artCtl.setPrefs(backgroundsOn, gameArtOn)
    onGameArtOnChanged: artCtl.setPrefs(backgroundsOn, gameArtOn)
    onThemeModeChanged: pushInk()
    onRoleColoursChanged: pushRoles()
    onViewHiddenChanged: artCtl.setHidden(Motion.realWinState === "hidden" || viewHidden)
    Connections {
        target: Th
        function onTimelineBgChanged() { gl.pushRoles(); gl.pushInk() }
        function onTimelineStartFgChanged() { gl.pushInk() }
        function onTimelineEmptyFgChanged() { gl.pushInk() }
        function onTimelineNewLineFgChanged() { gl.pushInk() }
        function onNamesChanged() { gl.pushInk() }
        function onSurfaceWindowChanged() { gl.pushInk() }
    }
    Connections {
        target: Motion
        function onRealWinStateChanged() { artCtl.setHidden(Motion.realWinState === "hidden" || gl.viewHidden) }
    }

    // two slides: the new art loads into the back one, then crossfades over the front
    property Item front: slideA
    property Item back: slideB
    // the controller publishes an image and its veil TOGETHER (artJson, set last): they never arrive apart
    Connections {
        target: artCtl
        function onArtJsonChanged() {
            if (artCtl.artJson.length === 0) { fadeIn.stop(); slideA.opacity = 0; slideB.opacity = 0; return }
            var a = JSON.parse(artCtl.artJson)
            if (gl.front.src === a.url) {   // the same image (shown, or waiting for its fade-in): its veil in place
                gl.front.veil = a.veil
                if (!fadeIn.running) gl.front.opacity = 1
                return
            }
            if (gl.back.src === a.url) {                            // already loading / loaded in the back slide
                gl.back.veil = a.veil
                if (gl.back.ready) gl.swap()
                return
            }
            gl.back.artW = a.w
            gl.back.artH = a.h
            gl.back.veil = a.veil
            gl.back.src = a.url
        }
    }
    // A change dissolves THROUGH the theme: the old slide fades out in the first half, the new one in during the second.
    // One image at a time, mixed toward the theme = a stronger veil, which the shared veil rule keeps safe (a crossfade of
    // two images was not: their mix can leave both images' bounds).
    Timer { id: fadeIn; interval: Math.max(1, gl.fadeMs / 2); onTriggered: gl.front.opacity = 1 }
    function swap() {
        var f = front
        front = back
        back = f
        front.z = 1
        back.z = 0
        back.opacity = 0
        if (gl.fadeMs === 0 || f.opacity === 0) front.opacity = 1
        else fadeIn.restart()
    }

    component Slide: Item {
        id: sl
        property string src: ""
        property int artW: 0
        property int artH: 0
        property real veil: 0.92
        readonly property bool ready: img.status === Image.Ready
        // while fading, the slide is ONE composited layer: its veil is never weakened over its own art
        layer.enabled: opacity > 0 && opacity < 1
        readonly property var fit: JSON.parse(artCtl.fitJson(artW, artH, gl.width, gl.height, Screen.devicePixelRatio))
        anchors.fill: parent
        opacity: 0
        Behavior on opacity { NumberAnimation { duration: gl.fadeMs / 2; easing.type: Easing.InOutQuad } }
        Image {
            id: img
            source: sl.src
            asynchronous: true
            cache: false
            smooth: true
            mipmap: true       // a good downscale where the art is bigger than the window
            x: sl.fit.x; y: sl.fit.y; width: sl.fit.w; height: sl.fit.h
            fillMode: Image.Stretch
            onStatusChanged: if (status === Image.Ready && sl === gl.back && artCtl.artJson.length > 0 && sl.src === JSON.parse(artCtl.artJson).url) gl.swap()
        }
        // the art's own edges fade into the theme where it doesn't reach (bottom; sides)
        Rectangle {
            visible: sl.fit.fadeBottom
            x: 0; width: parent.width
            y: sl.fit.h * 0.55; height: sl.fit.h * 0.45 + 1
            gradient: Gradient {
                GradientStop { position: 0; color: Qt.rgba(Th.timelineBg.r, Th.timelineBg.g, Th.timelineBg.b, 0) }
                GradientStop { position: 1; color: Th.timelineBg }
            }
        }
        Rectangle {
            visible: sl.fit.fadeSides
            x: sl.fit.x - 1; y: 0; width: sl.fit.w * 0.12 + 1; height: parent.height
            gradient: Gradient {
                orientation: Gradient.Horizontal
                GradientStop { position: 0; color: Th.timelineBg }
                GradientStop { position: 1; color: Qt.rgba(Th.timelineBg.r, Th.timelineBg.g, Th.timelineBg.b, 0) }
            }
        }
        Rectangle {
            visible: sl.fit.fadeSides
            x: sl.fit.x + sl.fit.w * 0.88; y: 0; width: sl.fit.w * 0.12 + 1; height: parent.height
            gradient: Gradient {
                orientation: Gradient.Horizontal
                GradientStop { position: 0; color: Qt.rgba(Th.timelineBg.r, Th.timelineBg.g, Th.timelineBg.b, 0) }
                GradientStop { position: 1; color: Th.timelineBg }
            }
        }
        // the measured veil
        Rectangle { anchors.fill: parent; color: Th.timelineBg; opacity: sl.veil }
    }
    Slide { id: slideA }
    Slide { id: slideB }

    // the vignette (an ellipse ~120 % × 90 % at 50 % / 35 %, clear to 55 %, the theme at the rim; 70 %)
    Shape {
        anchors.fill: parent
        z: 2
        opacity: 0.7
        visible: gl.hasArt
        ShapePath {
            strokeWidth: -1
            fillGradient: RadialGradient {
                centerX: gl.width / 2; centerY: gl.height * 0.35
                focalX: gl.width / 2; focalY: gl.height * 0.35
                centerRadius: Math.max(gl.width * 0.6, gl.height * 0.45) * 1.41
                GradientStop { position: 0.55; color: Qt.rgba(Th.timelineBg.r, Th.timelineBg.g, Th.timelineBg.b, 0) }
                GradientStop { position: 1; color: Th.timelineBg }
            }
            startX: 0; startY: 0
            PathLine { x: gl.width; y: 0 }
            PathLine { x: gl.width; y: gl.height }
            PathLine { x: 0; y: gl.height }
            PathLine { x: 0; y: 0 }
        }
    }
}
