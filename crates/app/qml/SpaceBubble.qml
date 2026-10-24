import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the rail.* / space.* / dm.* tokens
import QtQml.Models

// SpaceBubble.qml — one bubble of the spaces rail (a meta space, a DM, a real space): the round avatar or glyph, the
// Discord-style selection pill, the squircle morph, the unread badge. It was ThSpaceRail's inline component; islands C3
// (ISLANDS-ARCH §4.2) gives it a file of its own because the rail's three islands (NavIsland, DmsIsland, SpacesIsland)
// all draw it. `rail` = the ThSpaceRail it sits in (its app, motion tokens and icon names).
Item {
    id: bub
    required property var railRef   // the ThSpaceRail (never `rail`: it would shadow the rail's id at the instance)
    property var sp
    property bool selected: false
    property bool hovered: false
    property bool dragging: false
    width: 68
    height: 52
    readonly property bool isMeta: sp && sp.kind === "meta"
    // RBAC §7.3: a space you were invited to but have not joined — dim, dashed ring, dot.
    readonly property bool invited: !!sp && sp.membership === "invite"

    // 11.0 (EDITMODE-UI §8, the Side monitor preset): in a row the pill sits UNDER the bubble
    property bool pillBelow: false
    // Discord-style left selection pill (grows on hover/selected).
    Rectangle {
        visible: !bub.pillBelow
        anchors.verticalCenter: parent.verticalCenter
        x: 0; width: 4
        height: bub.selected ? 32 : (bub.hovered ? 18 : 0)
        radius: 2
        color: Th.railPillBg
        Behavior on height { NumberAnimation { duration: bub.railRef._a.dur; easing.type: Easing.OutCubic } }
    }
    Rectangle {
        visible: bub.pillBelow
        anchors.horizontalCenter: parent.horizontalCenter
        y: parent.height
        height: 4
        width: bub.selected ? 32 : (bub.hovered ? 18 : 0)
        radius: 2
        color: Th.railPillBg
        Behavior on width { NumberAnimation { duration: bub.railRef._a.dur; easing.type: Easing.OutCubic } }
    }
    // bubble (circle → squircle when active/dragging)
    Rectangle {
        id: disc
        anchors.centerIn: parent
        width: 44; height: 44
        // 0.8.30: hover morphs too (Discord muscle memory), and the picture follows (below).
        radius: (bub.selected || bub.dragging || bub.hovered) ? bub.railRef._a.radiusLg : width / 2
        scale: bub.dragging ? 1.12 : 1.0
        Behavior on radius { NumberAnimation { duration: bub.railRef._a.dur } }
        Behavior on scale { NumberAnimation { duration: bub.railRef._a.dur } }
        color: bub.isMeta
            ? (bub.selected || bub.hovered
                ? Th.railNavBgActive
                // 0.22 read as near-black on the OLED background ("icons too
                // dark", live feedback) — 0.34 keeps the quiet look but visible.
                : Th.railNavBg)
            : "transparent"
        // Meta bubbles keep their inset ring; a real space's ring is drawn OUTSIDE the
        // picture below (this one was painted under the avatar, i.e. never seen).
        border.width: (bub.selected && bub.isMeta) ? 2 : 0
        border.color: Th.railNavBorderSelected
        Behavior on color { ColorAnimation { duration: bub.railRef._a.dur } }

        ThIcon {
            anchors.centerIn: parent
            visible: bub.isMeta
            width: 24; height: 24
            app: bub.railRef._a
            name: bub.sp ? bub.railRef._metaIcon(bub.sp.key) : ""
            // on the lit disc (selected / hovered) its own glyph token: bub.railRef.nav.icon is picked for the quiet disc
            color: (bub.selected || bub.hovered) ? Th.railNavIconActive : Th.railNavIcon
            // Force the tint — dark-rcc-missing names otherwise paint their baked
            // dark glyph (see HeaderIconButton.qml note). 0.8.26: pack-animated on hover.
            isMask: true
            hovered: bub.hovered
            active: bub.selected
        }
        Avatar {
            anchors.fill: parent
            visible: bub.sp && !bub.isMeta
            app: bub.railRef._a
            size: 44
            radius: disc.radius   // 0.8.30: circle -> squircle with the bubble
            name: bub.sp ? bub.sp.label : ""
            seed: bub.sp ? Seeds.avatarSeed(String(bub.sp.key || "")) : ""   // §6: a space is its id, never its name
            imageSource: (bub.sp && bub.sp.avatar) ? bub.sp.avatar : ""
            opacity: bub.invited ? 0.7 : 1.0
            Behavior on opacity { NumberAnimation { duration: bub.railRef._a.dur } }
        }
    }
    // RBAC §7.3: the invite ring — dashed, 2 px, accent, following the bubble's morph.
    Canvas {
        id: inviteRing
        visible: bub.invited
        anchors.centerIn: disc
        width: disc.width + 8; height: width
        readonly property real ringRadius: disc.radius + 4
        readonly property color ringColor: Th.spaceInviteRing
        onRingRadiusChanged: requestPaint()
        onRingColorChanged: requestPaint()
        onVisibleChanged: if (visible) requestPaint()
        onPaint: {
            var ctx = getContext("2d")
            ctx.reset()
            ctx.clearRect(0, 0, width, height)
            ctx.lineWidth = 2
            ctx.strokeStyle = ringColor
            ctx.setLineDash([4, 3])
            var rr = Math.min(ringRadius, (width - 2) / 2)
            ctx.beginPath()
            ctx.roundedRect(1, 1, width - 2, height - 2, rr, rr)
            ctx.stroke()
        }
    }
    // …and the dot: something is waiting on you here.
    Rectangle {
        visible: bub.invited
        width: 12; height: 12; radius: 6
        anchors.right: disc.right; anchors.top: disc.top
        anchors.rightMargin: -4; anchors.topMargin: -4
        color: Th.spaceInviteDot
        border.width: 2; border.color: Th.spaceInviteDotBorder
    }
    // 0.8.30: selection ring for a real space, outside the picture.
    Rectangle {
        visible: !bub.isMeta && bub.selected
        anchors.centerIn: disc
        width: disc.width + 8; height: width
        radius: disc.radius + 4
        color: "transparent"
        border.width: 2
        border.color: Th.spaceRingSelected
        Behavior on radius { NumberAnimation { duration: bub.railRef._a.dur } }
    }
    // unread rollup badge — the rail rollup has always been the danger-red one
    UnreadBadge {
        app: bub.railRef._a
        count: bub.sp ? (bub.sp.unread || 0) : 0
        highlight: bub.sp ? (bub.sp.highlight === true) : false   // 10.1 (TH-NAV-10): red = someone said your name, not any message
        border.width: 2; border.color: Th.spaceBadgeBorder
        anchors.right: disc.right; anchors.bottom: disc.bottom
        anchors.rightMargin: -3; anchors.bottomMargin: -3
    }
}
