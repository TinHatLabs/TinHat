// ThEmbedCard.qml — inline click-to-PLAY media embed (Phase 2).
//
// Renders one embed descriptor parsed by embeds.rs:
//   { id, platform, mode, src, poster?, label, layout }
//
// By mode:
//   • image  → inline Image (GIF/still — loads immediately, it's lightweight)
//   • video  → poster card + ▶; on click a lazy Loader spins up a QtMultimedia
//              MediaPlayer + VideoOutput and auto-plays the direct media URL
//   • iframe → poster/branded card + ▶; on click a lazy Loader spins up a
//              QtWebEngine WebEngineView pointed at the embed src
//
// Privacy-preserving like the fork: NOTHING loads from the third party (no Chromium
// GPU process, no network) until the user clicks. An "open externally" affordance is
// always present (and is the graceful fallback if WebEngine misbehaves), and once a
// surface is playing a small ✕ collapses it back to the poster.
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the embed.* tokens
import QtMultimedia
import QtWebEngine

Item {
    id: card
    required property var app            // ApplicationWindow (theme tokens); may be null
    property bool inView: true   // 11.0 (GPU.md §2.6): inside the timeline's viewport (ChatBubble passes it)
    property var embed: ({})             // one embed descriptor (see header)
    property real maxW: 320              // max width available in the bubble
    // Media Party 11.4 (DESIGN §2.2): the bubble's room — while I'm in THAT room's call a music / video card offers
    // "Add to Media Party" + ▶ Play now (Main.qml mediaPartyAdd: joins the party first, then the link pipeline).
    property string roomId: ""
    readonly property bool partyOffer: !!(card.app && card.app.mInCall && card.roomId.length > 0 && String(card.app.mCallRoomId || "") === card.roomId)
                                       && ["youtube", "spotify", "apple", "media"].indexOf(String((card.embed && card.embed.platform) || "")) >= 0
    readonly property string linkUrl: (embed && embed.url) ? String(embed.url) : ((embed && embed.src) ? String(embed.src) : "")

    FallbackTheme { id: fallbackTheme }
    readonly property var theme: app ? app : fallbackTheme
    // A colour for the embed's own HTML page (the web engine renders it): the token as CSS text.
    function cssColour(c) {
        return "rgba(" + Math.round(c.r * 255) + ", " + Math.round(c.g * 255) + ", " + Math.round(c.b * 255) + ", " + c.a + ")"
    }

    readonly property string mode:   (embed && embed.mode)   ? embed.mode   : ""
    readonly property string layout: (embed && embed.layout) ? embed.layout : "video"
    readonly property string src:    (embed && embed.src)    ? embed.src    : ""
    readonly property string poster: (embed && embed.poster) ? embed.poster : ""
    readonly property string label:  (embed && embed.label)  ? embed.label  : "Link"
    readonly property bool isImage:  mode === "image"
    // M9 (security review 2026-10-26): a GIF from a host other than Giphy waits for a click (embeds.rs `autoload`;
    // a descriptor without the field — an older cached timeline — waits too)
    readonly property bool imageWaits: isImage && !(embed && embed.autoload === true) && !activated

    // Flipped true on the first click — load-bearing: until then no third-party load.
    property bool activated: false

    // ---- 10.4: fullscreen for iframe embeds (the player's own ⛶ button) ---------------------
    // Chromium raises fullScreenRequested; we accept it, lift the LIVE surface out of the bubble
    // into a black layer over the whole window and take the window itself fullscreen (put back
    // on exit). The surface is moved, never re-created, so the video keeps playing across the
    // move. Out again: the player's button, Esc (Chromium's own, or ours), the ✕ pill, or the
    // card going away (room switch) — every path lands in leaveFullscreen().
    property bool fullscreen: false
    property int _prevVisibility: Window.Windowed
    property var fsSurface: null     // the loaded web surface while it is lifted
    property var fsWeb: null         // its WebEngineView (fullScreenCancelled on our own exits)
    readonly property var hostWindow: card.Window.window
    readonly property Item overlayItem: Controls.Overlay.overlay
    function enterFullscreen(surface, web) {
        if (card.fullscreen || !card.hostWindow || !surface) return
        card._prevVisibility = card.hostWindow.visibility
        card.fsSurface = surface; card.fsWeb = web
        surface.parent = fsHost
        card.fullscreen = true
        fsHint.opacity = 1; fsHintTimer.restart()
        if (card._prevVisibility !== Window.FullScreen) card.hostWindow.visibility = Window.FullScreen
        console.log("[embed] fullscreen on")
    }
    // `ours` = we are leaving on our own (Esc / ✕), so Chromium must be told; a request from the
    // page (toggleOn false) already knows.
    function leaveFullscreen(ours) {
        if (!card.fullscreen) return
        card.fullscreen = false
        if (ours && card.fsWeb) card.fsWeb.fullScreenCancelled()
        if (card.fsSurface) card.fsSurface.parent = webLoader
        card.fsSurface = null; card.fsWeb = null
        if (card.hostWindow && card._prevVisibility !== Window.FullScreen && card.hostWindow.visibility === Window.FullScreen)
            card.hostWindow.visibility = card._prevVisibility
        console.log("[embed] fullscreen off")
    }
    Rectangle {
        id: fsHost
        parent: card.overlayItem
        anchors.fill: parent
        z: 100000
        visible: card.fullscreen
        color: Th.embedSurfaceBg
        // Browser idiom: "Press Esc to exit full screen", fades after a moment; the pill stays
        // reachable on hover at the top-right.
        Rectangle {
            id: fsHint
            anchors.horizontalCenter: parent.horizontalCenter
            anchors.top: parent.top
            anchors.topMargin: 28
            z: 2
            radius: height / 2
            color: Th.embedFullscreenHintBg
            border.width: 1
            border.color: Th.embedFullscreenBorder
            implicitWidth: fsHintText.implicitWidth + 28
            implicitHeight: fsHintText.implicitHeight + 14
            opacity: 0
            visible: opacity > 0
            Behavior on opacity { NumberAnimation { duration: Motion.durVeryLong } }
            Controls.Label { id: fsHintText; anchors.centerIn: parent; text: "Press Esc to exit full screen"; color: Th.embedOverlayFg; font.bold: true }
        }
        Timer { id: fsHintTimer; interval: 2600; onTriggered: fsHint.opacity = 0 }
        Rectangle {
            anchors.right: parent.right
            anchors.top: parent.top
            anchors.margins: 14
            z: 2
            width: 34; height: 34; radius: width / 2
            color: fsExitMa.containsMouse ? Th.embedFullscreenExitBgHover : Th.embedFullscreenExitBg
            border.width: 1
            border.color: fsExitMa.containsMouse ? Th.embedFullscreenBorderHover : Th.embedFullscreenBorder
            opacity: (fsExitMa.containsMouse || fsHint.opacity > 0) ? 1 : 0.35
            Behavior on opacity { NumberAnimation { duration: card.theme.dur } }
            Controls.ToolTip.text: "Exit full screen (Esc)"
            Controls.ToolTip.visible: fsExitMa.containsMouse
            Controls.ToolTip.delay: 400
            ThIcon { anchors.centerIn: parent; width: 16; height: 16; app: card.app; name: "view-restore-symbolic"; color: Th.embedOverlayFg; isMask: true }
            MouseArea { id: fsExitMa; anchors.fill: parent; hoverEnabled: true; cursorShape: Qt.PointingHandCursor; onClicked: card.leaveFullscreen(true) }
        }
        Shortcut { sequence: "Escape"; enabled: card.fullscreen; context: Qt.WindowShortcut; onActivated: card.leaveFullscreen(true) }
    }
    Component.onDestruction: if (card.fullscreen) card.leaveFullscreen(true)

    readonly property real cardW: Math.min(maxW, 400)
    readonly property real cardH: {
        if (layout === "audioSm") return 160     // Spotify track / SoundCloud
        if (layout === "audioLg") return 352     // Spotify album / playlist
        return Math.round(cardW * 9 / 16)        // video / iframe → 16:9
    }

    implicitWidth: cardW
    implicitHeight: isImage
        ? (imageWaits ? 56 : (gifImg.paintedHeight > 0 ? Math.min(300, gifImg.paintedHeight) : 160))
        : cardH

    // ---- inline GIF / still image ------------------------------------------------
    // AnimatedImage (0.8.21): Giphy picks and pasted .gif links arrive as plain
    // links rendered here, and a plain Image shows only the first frame. A still image
    // renders the same through AnimatedImage.
    // 11.0 (GPU.md §2.6): ThGif — ≤ 3 loops or 15 s while focused + in view, then its first frame; a hover replays
    ThGif {
        id: gifImg
        visible: card.isImage
        inView: card.inView
        clickPlays: false
        anchors.left: parent.left
        anchors.top: parent.top
        width: parent.width
        fillMode: Image.PreserveAspectFit
        source: (card.isImage && !card.imageWaits) ? card.src : ""
        asynchronous: true
        cache: true
    }
    // M9: the click-to-show face of a GIF from another host — nothing is fetched until it is clicked
    // (the poster look of the click-to-play cards below: their gradient + scrim, so the overlay text reads in every theme)
    Rectangle {
        visible: card.imageWaits
        anchors.fill: parent
        radius: card.theme.radiusLg
        clip: true
        border.width: 1
        border.color: Th.embedBorder
        gradient: Gradient {
            GradientStop { position: 0.0; color: Th.embedPosterTopBg }
            GradientStop { position: 1.0; color: Th.embedPosterBottomBg }
        }
        Rectangle {
            anchors.fill: parent
            radius: parent.radius
            color: gifShowMa.containsMouse ? Th.embedControlBgHover : Th.embedPosterOverlay
        }
        RowLayout {
            anchors.fill: parent
            anchors.leftMargin: 12
            anchors.rightMargin: 12
            spacing: 8
            ThIcon { Layout.preferredWidth: 18; Layout.preferredHeight: 18; app: card.app; name: "image-x-generic-symbolic"; color: Th.embedOverlayFg; isMask: true }
            Controls.Label {
                Layout.fillWidth: true
                textFormat: Text.PlainText
                elide: Text.ElideMiddle
                text: "GIF from " + String((card.embed && card.embed.host) || "another site") + " — click to show"
                color: Th.embedOverlayFg
            }
        }
        MouseArea {
            id: gifShowMa
            anchors.fill: parent
            hoverEnabled: true
            cursorShape: Qt.PointingHandCursor
            onClicked: card.activated = true
        }
    }

    // ---- non-image (video / iframe): poster card OR the live player --------------
    Rectangle {
        visible: !card.isImage
        anchors.fill: parent
        radius: card.theme.radiusLg
        clip: true
        color: Th.embedBg
        border.width: 1
        border.color: Th.embedBorder

        // ===== IDLE: poster + ▶ + platform chip =====
        Item {
            anchors.fill: parent
            visible: !card.activated

            // Poster thumbnail (YouTube etc.) — branded gradient when none (audio).
            Image {
                anchors.fill: parent
                visible: card.poster.length > 0
                source: card.poster
                fillMode: Image.PreserveAspectCrop
                asynchronous: true
                cache: true
            }
            Rectangle {
                anchors.fill: parent
                visible: card.poster.length === 0
                gradient: Gradient {
                    GradientStop { position: 0.0; color: Th.embedPosterTopBg }
                    GradientStop { position: 1.0; color: Th.embedPosterBottomBg }
                }
            }
            // Darkening scrim so the ▶ + chip read on any thumbnail.
            Rectangle {
                anchors.fill: parent
                color: Th.embedPosterOverlay
            }
            // Play ▶ circle — lifts + darkens while the poster is hovered.
            Rectangle {
                anchors.centerIn: parent
                width: 54; height: 54; radius: width / 2
                color: posterMa.containsMouse ? Th.embedPlayBgHover : Th.embedPlayBg
                border.width: 2
                border.color: Th.embedOverlayFg
                scale: posterMa.containsMouse ? 1.08 : 1.0
                Behavior on scale { NumberAnimation { duration: card.theme.dur; easing.type: Easing.OutCubic } }
                Behavior on color { ColorAnimation { duration: card.theme.dur } }
                Controls.ToolTip.text: "Play"
                Controls.ToolTip.visible: posterMa.containsMouse
                Controls.ToolTip.delay: 400
                ThIcon {   // 0.8.30: pack glyph (was a typeset ▶)
                    anchors.centerIn: parent
                    anchors.horizontalCenterOffset: 2
                    width: 22; height: 22
                    app: card.app
                    name: "media-playback-start-symbolic"
                    color: Th.embedOverlayFg
                    isMask: true
                    hovered: posterMa.containsMouse
                }
            }
            // Media Party 11.4: "Add to Media Party" + ▶ (bottom-right, the chip's look) while in the room's call.
            Row {
                anchors.right: parent.right
                anchors.bottom: parent.bottom
                anchors.margins: 8
                spacing: 4
                visible: card.partyOffer
                z: 4
                Rectangle {
                    width: mpAddLbl.implicitWidth + 24; height: mpAddLbl.implicitHeight + 6; radius: 5
                    color: mpAddMa.containsMouse ? Th.embedControlBgHover : Th.embedChipBg
                    Row {
                        anchors.centerIn: parent
                        spacing: 4
                        ThIcon { width: 14; height: 14; anchors.verticalCenter: parent.verticalCenter; app: card.app; name: "video-television-symbolic"; color: Th.embedOverlayFg; isMask: true }
                        Controls.Label { id: mpAddLbl; text: "Add to Media Party"; color: Th.embedOverlayFg; font.bold: true; font.pointSize: Kirigami.Theme.smallFont.pointSize }
                    }
                    MouseArea { id: mpAddMa; anchors.fill: parent; hoverEnabled: true; cursorShape: Qt.PointingHandCursor; onClicked: card.app.mediaPartyAdd(card.roomId, card.linkUrl, false) }
                }
                Rectangle {
                    width: 26; height: mpAddLbl.implicitHeight + 6; radius: 5
                    color: mpPlayMa.containsMouse ? Th.embedControlBgHover : Th.embedChipBg
                    Controls.ToolTip.text: "Play now in the Media Party"
                    Controls.ToolTip.visible: mpPlayMa.containsMouse
                    Controls.ToolTip.delay: 400
                    ThIcon { anchors.centerIn: parent; width: 14; height: 14; app: card.app; name: "media-playback-start"; color: Th.embedOverlayFg; isMask: true }
                    MouseArea { id: mpPlayMa; anchors.fill: parent; hoverEnabled: true; cursorShape: Qt.PointingHandCursor; onClicked: card.app.mediaPartyAdd(card.roomId, card.linkUrl, true) }
                }
            }
            // Platform label chip (bottom-left).
            Rectangle {
                anchors.left: parent.left
                anchors.bottom: parent.bottom
                anchors.margins: 8
                radius: card.theme.radiusSm
                color: Th.embedChipBg
                implicitWidth: platLabel.implicitWidth + 12
                implicitHeight: platLabel.implicitHeight + 6
                Controls.Label {
                    textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                    id: platLabel
                    anchors.centerIn: parent
                    text: card.label
                    color: Th.embedOverlayFg
                    font.bold: true
                    font.pixelSize: Math.round(Kirigami.Theme.smallFont.pixelSize)
                }
            }
            // Click anywhere on the poster → play inline.
            MouseArea {
                id: posterMa
                anchors.fill: parent
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                onClicked: card.activated = true
            }
        }

        // ===== ACTIVATED: the live surface (lazy) =====
        Loader {
            id: playerLoader
            anchors.fill: parent
            active: card.activated && card.mode === "video"
            sourceComponent: videoComponent
        }
        Loader {
            id: webLoader
            anchors.fill: parent
            active: card.activated && card.mode === "iframe"
            sourceComponent: webComponent
        }

        // Small overlay controls while playing (top-right): ↗ external, ✕ collapse.
        Row {
            anchors.right: parent.right
            anchors.top: parent.top
            anchors.margins: 6
            spacing: 4
            visible: card.activated
            z: 5
            // 22px dark circles — the app's one drawn-close size (matches the chat-head ✕).
            Rectangle {
                width: 22; height: 22; radius: width / 2
                color: extMa.containsMouse ? Th.embedControlBgHover : Th.embedControlBg
                scale: extMa.containsMouse ? 1.1 : 1.0
                Behavior on scale { NumberAnimation { duration: card.theme.dur; easing.type: Easing.OutCubic } }
                Behavior on color { ColorAnimation { duration: card.theme.dur } }
                Controls.ToolTip.text: "Open link"
                Controls.ToolTip.visible: extMa.containsMouse
                Controls.ToolTip.delay: 400
                Controls.Label {
                    anchors.centerIn: parent; text: "↗"; color: Th.embedOverlayFg; font.pixelSize: 12
                }
                MouseArea {
                    id: extMa
                    anchors.fill: parent
                    hoverEnabled: true
                    cursorShape: Qt.PointingHandCursor
                    onClicked: LinkGate.open(card.src)
                }
            }
            Rectangle {
                width: 22; height: 22; radius: width / 2
                color: dismissMa.containsMouse ? Th.embedControlBgHover : Th.embedControlBg
                scale: dismissMa.containsMouse ? 1.1 : 1.0
                Behavior on scale { NumberAnimation { duration: card.theme.dur; easing.type: Easing.OutCubic } }
                Behavior on color { ColorAnimation { duration: card.theme.dur } }
                Controls.ToolTip.text: "Dismiss"
                Controls.ToolTip.visible: dismissMa.containsMouse
                Controls.ToolTip.delay: 400
                ThIcon {   // 0.8.30: pack glyph (was a typeset ✕)
                    anchors.centerIn: parent; width: 12; height: 12
                    app: card.app; name: "window-close-symbolic"; color: Th.embedOverlayFg; isMask: true
                }
                MouseArea {
                    id: dismissMa
                    anchors.fill: parent
                    hoverEnabled: true
                    cursorShape: Qt.PointingHandCursor
                    onClicked: card.activated = false   // collapse back to poster, frees the surface
                }
            }
        }
    }

    // ---- lazy surface components (instantiated only when activated) --------------
    Component {
        id: videoComponent
        VideoOutput {
            id: vout
            MediaPlayer {
                id: player
                source: card.src
                videoOutput: vout
                audioOutput: AudioOutput {}
                Component.onCompleted: play()
            }
            // Tap toggles play/pause.
            MouseArea {
                anchors.fill: parent
                onClicked: player.playbackState === MediaPlayer.PlayingState
                    ? player.pause() : player.play()
            }
        }
    }

    Component {
        id: webComponent
        Item {
            id: webSurface
            anchors.fill: parent      // fills the Loader in the bubble, fsHost while lifted (10.4)
            // The Loader destroys this with the card / on ✕ — put the window back first.
            Component.onDestruction: if (card.fullscreen) card.leaveFullscreen(true)
            WebEngineView {
                id: web
                anchors.fill: parent
                settings.playbackRequiresUserGesture: false  // user already clicked the card
                settings.javascriptCanOpenWindows: false
                settings.localStorageEnabled: true
                // 10.4: the player's ⛶ works — accept, then lift the surface (see enterFullscreen).
                settings.fullScreenSupportEnabled: true
                onFullScreenRequested: (request) => {
                    request.accept()
                    if (request.toggleOn) card.enterFullscreen(webSurface, web)
                    else card.leaveFullscreen(false)
                }
                // Host the embed inside a real <iframe> within a wrapper document whose
                // origin is "localhost" (matching embeds.rs TWITCH_PARENT). Loading a
                // youtube-nocookie /embed URL as a TOP-LEVEL navigation makes YouTube
                // refuse with "Error 153" (it detects it isn't actually iframe-embedded);
                // wrapping it fixes every platform at once and mirrors how the web fork
                // embedded them (iframe inside the app page).
                Component.onCompleted: loadHtml(
                    '<!doctype html><html><head><meta charset="utf-8"><style>'
                    + 'html,body{margin:0;height:100%;background:' + card.cssColour(Th.embedSurfaceBg) + ';overflow:hidden}'
                    + 'iframe{border:0;width:100%;height:100%;display:block}</style></head><body>'
                    + '<iframe src="' + ('' + card.src).replace(/&/g, '&amp;') + '" '
                    + 'allow="autoplay; encrypted-media; picture-in-picture; fullscreen; clipboard-write" '
                    + 'allowfullscreen referrerpolicy="origin-when-cross-origin"></iframe>'
                    + '</body></html>',
                    'https://localhost/')
            }
            // Loading mark until the wrapper paints — 11.0 (GPU.md §2.5): still; it steps with the page's load progress.
            ThBusyIndicator {
                anchors.centerIn: parent
                running: web.loadProgress < 100
                visible: running
                progress: web.loadProgress
            }
        }
    }
}
