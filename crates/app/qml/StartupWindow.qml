// StartupWindow — the small "starting up" window (0.8.24; portrait redesign 0.8.30).
//
// Shown by main.rs the moment Qt can paint, BEFORE the GStreamer registry scan and the
// Main.qml load, and kept up until Main.qml reveals its window (session restored + rooms
// in, or no session). BootController (boot.rs) pushes the stage / detail / percent /
// finished properties; nothing here polls.
//
// 0.8.30: a PORTRAIT card, everything centred — the Tin Hat Element mark (its
// emblem turns while we boot) on top, the wordmark and build under it, then the progress
// bar with what is loading written underneath it. The disc used to paint as a green
// SQUARE: QtSvg ignores <clipPath>, so the facet polygons that overhang the circle were
// drawn whole — the layer SVG is now pre-clipped (assets/appicon/layers/disc.svg).
//
// Pure QtQuick on purpose: no Kirigami / Controls import — those are among the heavy
// loads this window exists to cover, and it must come up in well under 100 ms. The bar's
// sweep and both fades are render-thread Animators, so they keep moving while the GUI
// thread is blocked in gst_init() or the Main.qml load (threaded render loop; they simply
// pause on the basic loop).
import QtQuick
import QtQuick.Window
import dev.tinhat.app

Window {
    id: splash
    // NOT "Tin Hat": qml-smoke / winsmoke assert the MAIN window by its exact title.
    title: "Starting Tin Hat"
    flags: Qt.SplashScreen | Qt.FramelessWindowHint | Qt.WindowStaysOnTopHint
    width: 320
    height: 440
    minimumWidth: width; maximumWidth: width
    minimumHeight: height; maximumHeight: height
    // Centered, a touch above the middle (X11 / Windows honour this; Wayland places it itself).
    x: Screen.virtualX + Math.round((Screen.width - width) / 2)
    y: Screen.virtualY + Math.round((Screen.height - height) / 2) - 30
    color: Th.startupBg
    visible: true

    BootController {
        id: boot
        Component.onCompleted: boot.attach()
    }

    // themes (desktop C2): the colours are the startup.* tokens of the theme the app starts in. This window paints
    // before the main window — and AppCore.applyTheme — exist, so it puts that theme into Th itself, once, before
    // its first frame: boot.theme (TINHAT_THEME > the saved choice, AppCore's precedence), resolved by the engine (an
    // unknown or refused id is OLED, like everywhere). AppCore's bootstrap applies the same theme again.
    ThemeController { id: bootTheme }
    Component.onCompleted: Th.apply(bootTheme.resolve(boot.theme), bootTheme.themeId, bootTheme.themeName,
                                    bootTheme.variant, bootTheme.names, bootTheme.shape)
    readonly property string assetsUrl: "file://" + boot.assetsPath
    // 11.0 (GPU.md §1/§2.5): this window stands in for the main one while it boots (the main window is still hidden,
    // so the app-wide switch reads "hidden"): it moves while it is shown, unless Reduce motion. The fades take the
    // duration token's steps; the mark's light steps once per boot stage; nothing loops.
    readonly property bool bootAnimate: splash.visible && !Motion.reducedMotion
    readonly property int bootDurSlow: splash.bootAnimate ? Motion.stepSlow : 0
    readonly property int bootDurLong: splash.bootAnimate ? Motion.stepLong : 0
    readonly property string fontFamily: Qt.application.font.family

    // ---- Fade in on first paint, fade out when the main window is up ----
    Item {
        id: sheet
        anchors.fill: parent
        opacity: 0
        Component.onCompleted: fadeIn.start()
        OpacityAnimator { id: fadeIn; target: sheet; from: 0; to: 1; duration: splash.bootDurSlow }
        OpacityAnimator {
            id: fadeOut; target: sheet; from: 1; to: 0; duration: splash.bootDurSlow
            onStopped: splash.visible = false
        }

        Rectangle { anchors.fill: parent; color: Th.startupBg }
        // Soft accent wash behind the mark (the sign-in card's corner glow, centred here).
        Rectangle {
            anchors.horizontalCenter: parent.horizontalCenter
            y: -70
            width: 360; height: width; radius: width / 2
            opacity: 0.14
            gradient: Gradient {
                GradientStop { position: 0.0; color: Th.startupWashGlow }
                GradientStop { position: 1.0; color: "transparent" }
            }
        }
        // Frame: 1 px separator border + a 3 px accent strip along the top edge.
        Rectangle { anchors.fill: parent; color: "transparent"; border.width: 1; border.color: Th.startupBorder }
        Rectangle {
            anchors { top: parent.top; left: parent.left; right: parent.right; margins: 1 }
            height: 3
            gradient: Gradient {
                orientation: Gradient.Horizontal
                GradientStop { position: 0.0; color: Th.startupStripeStartFill }
                GradientStop { position: 1.0; color: Th.startupStripeEndFill }
            }
        }

        // ---- The mark, centred ----
        Item {
            id: mascotBox
            anchors.horizontalCenter: parent.horizontalCenter
            y: 46
            width: 128; height: 128
            Rectangle { // glow disc behind the mascot (sign-in card idiom)
                anchors.centerIn: parent
                width: parent.width * 1.4; height: width; radius: width / 2
                opacity: 0.35
                gradient: Gradient {
                    GradientStop { position: 0.0; color: Th.startupMarkGlow }
                    GradientStop { position: 1.0; color: "transparent" }
                }
            }
            // The Tin Hat mark — its light moves one step per boot stage (the percent), never on its own.
            TinHatLoader {
                anchors.fill: parent
                running: !boot.finished
                animate: splash.bootAnimate
                progress: boot.percent
            }
        }
        Text {
            id: wordmark
            anchors.horizontalCenter: parent.horizontalCenter
            anchors.top: mascotBox.bottom
            anchors.topMargin: 18
            text: "Tin Hat"
            color: Th.startupFg
            font.family: splash.fontFamily
            font.pixelSize: 26
            font.weight: Font.Bold
        }
        Text {
            id: versionText
            anchors.horizontalCenter: parent.horizontalCenter
            anchors.top: wordmark.bottom
            anchors.topMargin: 2
            text: boot.version
            color: Th.startupHintFg
            font.family: splash.fontFamily
            font.pixelSize: 12
        }

        // Safe mode (0.8.25): the previous start never reached the main window.
        Rectangle {
            visible: boot.safeMode
            anchors.horizontalCenter: parent.horizontalCenter
            anchors.top: versionText.bottom
            anchors.topMargin: 8
            implicitWidth: safeLbl.implicitWidth + 14
            implicitHeight: 18
            radius: 9
            color: Th.startupSafeModeBg
            Text {
                id: safeLbl
                anchors.centerIn: parent
                text: "SAFE MODE"
                color: Th.startupSafeModeFg
                font.family: splash.fontFamily
                font.pixelSize: 10
                font.weight: Font.Bold
            }
        }

        // ---- Progress: percent fill + a still glint at the track's start while loading (the sweep used to loop
        //      every 1.4 s — GPU.md §2.5: no loop) ----
        Rectangle {
            id: track
            anchors.horizontalCenter: parent.horizontalCenter
            y: 318
            width: 232
            height: 6
            radius: 3
            color: Th.startupProgressTrack
            clip: true
            Rectangle {
                id: fill
                height: parent.height
                radius: 3
                width: Math.max(12, parent.width * boot.percent / 100)
                color: Th.startupProgressFill
                Behavior on width { NumberAnimation { duration: splash.bootDurLong; easing.type: Easing.OutCubic } }
            }
            Rectangle {
                id: sweep
                width: 90; height: parent.height
                visible: !boot.finished
                gradient: Gradient {
                    orientation: Gradient.Horizontal
                    GradientStop { position: 0.0; color: "transparent" }
                    GradientStop { position: 0.5; color: Th.startupProgressHighlight }
                    GradientStop { position: 1.0; color: "transparent" }
                }
            }
        }

        // ---- What is happening right now, under the bar ----
        Text {
            id: stageText
            anchors.horizontalCenter: parent.horizontalCenter
            anchors.top: track.bottom
            anchors.topMargin: 14
            width: parent.width - 48
            horizontalAlignment: Text.AlignHCenter
            text: boot.finished ? "Ready" : boot.stage
            color: Th.startupFg
            font.family: splash.fontFamily
            font.pixelSize: 15
            elide: Text.ElideRight
        }
        Text {
            id: detailText
            anchors.horizontalCenter: parent.horizontalCenter
            anchors.top: stageText.bottom
            anchors.topMargin: 3
            width: stageText.width
            horizontalAlignment: Text.AlignHCenter
            text: boot.detail
            color: Th.startupHintFg
            font.family: splash.fontFamily
            font.pixelSize: 12
            elide: Text.ElideMiddle
            visible: text.length > 0
        }
        Text {
            anchors.horizontalCenter: parent.horizontalCenter
            anchors.bottom: parent.bottom
            anchors.bottomMargin: 16
            text: boot.finished ? "" : (boot.percent + "%")
            color: Th.startupHintFg
            font.family: splash.fontFamily
            font.pixelSize: 11
        }
    }

    Connections {
        target: boot
        function onFinishedChanged() { if (boot.finished) fadeOut.start() }
    }
    // Safety net: never sit on top of the desktop forever if the reveal handshake broke.
    Timer {
        interval: 40000; running: true
        onTriggered: if (splash.visible) { console.log("[boot] startup window timed out — hiding"); splash.visible = false }
    }
}
