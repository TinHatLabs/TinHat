// Lightbox.qml — full-window zoomable image viewer for timeline images. A dark backdrop +
// centred image with wheel-to-zoom and drag-to-pan when zoomed. Click the backdrop, the ✕,
// or press Escape to close. One instance lives at the root; ChatBubble.imageClicked → show().
import QtQuick
import QtQuick.Controls as Controls
import dev.tinhat.app   // Th: the lightbox.* tokens

Item {
    id: lb
    property string imgSource: ""
    property string alt: ""

    // No `app` ref here — FallbackTheme carries the same canonical dur/durSlow tokens the app root uses; the
    // colours are the lightbox.* tokens (Th).
    FallbackTheme { id: fallbackTheme }

    property bool shownState: false

    anchors.fill: parent
    // Visible the instant show() runs (focus/Escape need a visible item), and lingers
    // through the fade-out.
    visible: shownState || opacity > 0
    opacity: shownState ? 1 : 0
    Behavior on opacity { NumberAnimation { duration: fallbackTheme.durSlow; easing.type: Easing.OutCubic } }
    z: 10000
    focus: visible

    // Release the texture only after the fade-out finishes — clearing in hide() would
    // blank the image mid-fade.
    onOpacityChanged: if (!shownState && opacity === 0) imgSource = ""

    // 10.1 (TH-MSG-14): the natural size the caller knows — a small meme is shown at 1:1, not
    // blown up to 92 % of a 4K panel.
    property real natW: 0
    property real natH: 0
    readonly property real _fit: (natW > 0 && natH > 0) ? Math.min(1.0, (lb.width * 0.92) / natW, (lb.height * 0.92) / natH) : 0
    function show(url, w, h, altText) {
        lb.imgSource = url
        lb.natW = (w && w > 0) ? w : 0
        lb.natH = (h && h > 0) ? h : 0
        lb.alt = altText || ""
        scaleBehavior.enabled = false   // reset must not animate from the previous zoom
        img.scale = 1.0
        img.x = 0; img.y = 0
        scaleBehavior.enabled = true
        lb.shownState = true
        lb.forceActiveFocus()
    }
    function hide() { lb.shownState = false }

    // Dark backdrop — clicking it (anywhere not on the image) closes.
    Rectangle { anchors.fill: parent; color: Th.lightboxBg }
    MouseArea { anchors.fill: parent; onClicked: lb.hide() }

    // GIFs play in the lightbox too. 11.0 (GPU.md §2.6): by the low-GPU rule (ThGif) — opening it was a click,
    // so under Reduce motion it plays one loop, and every click on it plays one more.
    ThGif {
        id: img
        source: lb.imgSource
        inView: lb.shownState
        playOnMount: "one"
        anchors.centerIn: parent
        width: lb._fit > 0 ? Math.round(lb.natW * lb._fit) : parent.width * 0.92
        height: lb._fit > 0 ? Math.round(lb.natH * lb._fit) : parent.height * 0.92
        fillMode: Image.PreserveAspectFit
        smooth: true
        transformOrigin: Item.Center

        Behavior on scale { id: scaleBehavior; NumberAnimation { duration: fallbackTheme.dur; easing.type: Easing.OutCubic } }

        // Wheel to zoom (clamped); snapping back to 1.0 re-centres. The recentre check
        // tests the assigned target `s`, not img.scale (a mid-animation read).
        WheelHandler {
            onWheel: (e) => {
                var f = e.angleDelta.y > 0 ? 1.12 : (1.0 / 1.12)
                var s = Math.max(1.0, Math.min(8.0, img.scale * f))
                img.scale = s
                if (s <= 1.0) { img.x = 0; img.y = 0 }
            }
        }
        // Drag to pan once zoomed in.
        DragHandler { enabled: img.scale > 1.0; target: img }
        // Swallow taps on the image so the backdrop MouseArea doesn't close it mid-interaction.
        TapHandler { onTapped: {} }
    }

    Controls.Label {
        anchors { bottom: parent.bottom; horizontalCenter: parent.horizontalCenter; bottomMargin: 24 }
        visible: lb.alt.length > 0
        text: lb.alt
        color: Qt.alpha(Th.lightboxCaptionFg, 1)   // the token's alpha is the item's opacity (docs/MIGRATING-THEME.md §3.8)
        opacity: Th.lightboxCaptionFg.a
    }

    Controls.ToolButton {
        id: closeBtn
        anchors { top: parent.top; right: parent.right; margins: 16 }
        icon.name: "window-close-symbolic"
        icon.color: Th.lightboxCloseFg
        display: Controls.AbstractButton.IconOnly
        Controls.ToolTip.text: "Close (Esc)"
        Controls.ToolTip.visible: hovered
        Controls.ToolTip.delay: 400
        // Dark-circle overlay so the glyph reads on bright images — same idiom as the
        // embed-card overlay buttons.
        background: Rectangle {
            radius: width / 2
            color: closeBtn.hovered ? Th.lightboxCloseBgHover : Th.lightboxCloseBg
            border.width: 1
            border.color: Th.lightboxCloseFg
            Behavior on color { ColorAnimation { duration: fallbackTheme.dur } }
        }
        onClicked: lb.hide()
    }

    Keys.onEscapePressed: lb.hide()
}
