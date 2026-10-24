// Avatar.qml — reusable round avatar for Tin Hat.
//
// Draws a circular avatar with a deterministic warm-hue background derived from
// `name` (so the same person/room is always the same color) and a centered
// uppercase initial. An optional `imageSource` replaces the initial with a photo
// (loaded asynchronously, circularly masked). An optional presence/call dot can be
// shown in the lower-right.
//
// Furry flair (avatar.theme.isFurry, no image): a subtle blobfox tint sits behind the
// initial as a soft mascot wash — tasteful, never overpowering.
//
// CXX-Qt note: this component is pure-presentational and takes everything via
// plain QML properties set by the delegate (modelData) — it never binds to a Rust
// controller getter directly (those don't drive bindings reliably here).
import QtQuick
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the active theme (docs/MIGRATING-THEME.md)

Item {
    id: avatar
    required property var app   // ApplicationWindow (theme tokens); set by parent
    FallbackTheme { id: fallbackTheme }
    readonly property var theme: app || fallbackTheme

    // ---- public API ----
    property string name: ""            // mxid or display name (used for the initial)
    // Stable unique identity for the hashed visuals (background hue + furry fox face).
    // Defaults to `name` for back-compat, but call sites should pass a STABLE unique id
    // (mxid / LiveKit participant / room id) so two people with the same or an
    // unresolved ("") display name never collide onto the same color/fox.
    property string seed: name
    property int size: 40               // diameter in px
    property string imageSource: ""     // optional photo (file://, mxc resolved upstream)
    property bool showDot: false        // show the presence/call dot
    property color dotColor: Th.avatarDot
    // 0.8.30: the shape of the disc. A circle by default; a container that morphs into a
    // squircle (the spaces rail's selected / hovered bubble) binds this to its own radius so
    // the picture fits the frame it sits in instead of staying a circle inside a rounded square.
    property real radius: size / 2

    // ---- 10.6 presence (docs/REACH.md §2.6 + A4.5) ----
    // "online" | "away" | "call" | "" (= no dot: absent, or not sharing). Replaces the old call
    // dot on the same corner while set. Fixed colours in every theme: online / call #3BA55C,
    // away #FAA81A — or the custom status colour (A4.5, computed in Rust; pass it as presenceColor).
    property string presence: ""
    property color presenceColor: presence === "away" ? Th.presenceAwayDot : (presence === "call" ? Th.presenceIncallDot : Th.presenceOnlineDot)
    property string presenceText: presence === "away" ? "Away" : (presence === "call" ? "In a call" : (presence === "online" ? "Online" : ""))
    property bool presenceLeft: false                 // the rail's DM strip: bottom-LEFT (the badge owns bottom-right)
    property real presenceSize: Math.max(8, size * 0.28)   // the existing Avatar dot rule
    property color presenceRing: Th.presenceRing     // the surface behind the avatar ("cut" ring)
    property real presenceRingWidth: Math.max(1.5, size * 0.05)

    implicitWidth: size
    implicitHeight: size

    // Decode budget for the photo/fox Images: 2x `size` for HiDPI headroom, quantized
    // to 64px buckets (capped 512) so resize-driven `size` bindings (e.g. the call-grid
    // tile) don't force a full async re-decode on every pixel of resize.
    readonly property int decodePx: Math.min(512, 64 * Math.ceil(Math.max(1, size * 2) / 64))

    // First visible character, uppercased. Skips a leading '@' / '#' / '!' sigil.
    readonly property string initial: {
        var s = (name || "").replace(/^[@#!+]/, "").trim()
        return s.length > 0 ? s.charAt(0).toUpperCase() : "?"
    }

    // Deterministic warm hue from the name. Hue is biased toward the warm arc
    // (reds/oranges/golds/teals) so it always feels at home in every palette;
    // furry leans hardest into fox-orange territory.
    readonly property color baseHue: {
        var s = seed || name || ""
        var h = 0
        for (var i = 0; i < s.length; i++)
            h = (h * 31 + s.charCodeAt(i)) >>> 0
        // themes 1b (THEMES-UI §10.1): a palette theme colours the disc from its name.1 … name.12, like the name
        if (Th.names === "palette") return Th.byId("name." + ((h % 12) + 1))
        // Warm-biased hue band: 0.02..0.16 (red->gold) plus an occasional teal.
        var pick = h % 100
        var hue = (pick < 80) ? (0.02 + (h % 56) / 360.0)   // warm reds/oranges/golds
                              : (0.45 + (h % 22) / 360.0)   // occasional teal accent
        var sat = 0.52
        var light = 0.46
        return Qt.hsla(hue, sat, light, 1.0)
    }


    // Circular base with a soft inner gradient (top sheen -> darker base) for depth.
    Kirigami.ShadowedRectangle {
        id: disc
        anchors.fill: parent
        radius: avatar.radius
        color: avatar.baseHue
        Kirigami.Theme.inherit: false

        shadow.size: Math.max(2, avatar.size * 0.12)
        shadow.color: Th.avatarShadow
        shadow.yOffset: 1

        // Thin warm ring for definition against panels.
        border.width: 1
        border.color: Th.avatarBorder

        // Soft top-sheen gradient (ShadowedRectangle has no `gradient` property, so
        // overlay a circular gradient Rectangle behind the initial/mascot).
        Rectangle {
            anchors.fill: parent
            radius: avatar.radius
            gradient: Gradient {
                GradientStop { position: 0.0; color: Qt.lighter(avatar.baseHue, 1.35) }
                GradientStop { position: 1.0; color: Qt.darker(avatar.baseHue, 1.18) }
            }
        }


        // Centered initial (hidden when a photo or a furry fox face is shown).
        Controls.Label {
            anchors.centerIn: parent
            visible: avatar.imageSource.length === 0 || photo.status === Image.Error   // 10.1 (TH-MSG-26): a dead photo shows the letter, not an empty disc
            text: avatar.initial
            color: Th.avatarFallbackFg
            font.bold: true
            font.pixelSize: Math.round(avatar.size * 0.44)
            font.family: avatar.theme.font.family
            style: Text.Raised
            styleColor: Th.avatarFallbackShadow
        }
    }

    // Optional photo, REALLY circularly cropped (0.8.26). The old `Rectangle { radius; clip }`
    // only clipped to the rectangular bounds, so photos sat square on the round disc. Kirigami's
    // ShadowedImage masks the texture to the radius — the circle that houses the avatar crops it.
    Kirigami.ShadowedImage {
        id: photo
        anchors.fill: parent
        visible: avatar.imageSource.length > 0 && status !== Image.Error
        source: avatar.imageSource
        radius: avatar.radius
        color: "transparent"
        fillMode: Image.PreserveAspectCrop
        sourceSize: Qt.size(avatar.decodePx, avatar.decodePx)
        asynchronous: true
        mipmap: true
        shadow.size: 0
        border.width: 1
        border.color: Th.avatarPhotoBorder
    }

    // 10.6: the presence dot — the dot itself is presenceSize, the ring is drawn OUTSIDE it in the
    // colour of the surface behind, so it cuts into the avatar edge (Discord's look). In a call it
    // carries a white headset glyph when the dot is ≥ 12 px (the icon pack has no
    // audio-headset-symbolic; its headphones-symbolic is the agreed fallback, web + desktop).
    Rectangle {
        id: presenceCut
        visible: avatar.presence.length > 0
        width: avatar.presenceSize + 2 * avatar.presenceRingWidth
        height: width
        radius: width / 2
        color: avatar.presenceRing
        anchors.bottom: parent.bottom
        anchors.bottomMargin: -avatar.presenceRingWidth
        anchors.right: avatar.presenceLeft ? undefined : parent.right
        anchors.rightMargin: -avatar.presenceRingWidth
        anchors.left: avatar.presenceLeft ? parent.left : undefined
        anchors.leftMargin: -avatar.presenceRingWidth
        Accessible.role: Accessible.Indicator
        Accessible.name: avatar.presenceText      // A2.14: every dot has a text alternative
        Rectangle {
            anchors.centerIn: parent
            width: avatar.presenceSize
            height: width
            radius: width / 2
            color: avatar.presenceColor
            Behavior on color { ColorAnimation { duration: Motion.dur } }
            Kirigami.Icon {
                anchors.centerIn: parent
                visible: avatar.presence === "call" && avatar.presenceSize >= 12
                width: Math.round(avatar.presenceSize * 0.72)
                height: width
                source: "headphones-symbolic"
                color: Th.presenceIncallIcon
                isMask: true
            }
        }
    }

    // Presence / active-call dot, lower-right, with a panel-colored cutout ring
    // so it reads as a separate badge floating over the avatar edge.
    Rectangle {
        visible: avatar.showDot && avatar.presence.length === 0
        width: Math.max(8, avatar.size * 0.28)
        height: width
        radius: width / 2
        color: avatar.dotColor
        border.width: Math.max(1.5, avatar.size * 0.05)
        border.color: Th.avatarDotRing
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        anchors.rightMargin: -1
        anchors.bottomMargin: -1
    }
}
