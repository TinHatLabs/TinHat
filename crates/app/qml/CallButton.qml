import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import QtQuick.Dialogs
import QtQuick.Window
import QtMultimedia
import org.kde.kirigami as Kirigami
import dev.tinhat.app

// CallButton.qml — was Main.qml's inline component; islands C3 (ISLANDS-ARCH §4.2) gives it a file so the
// extracted islands can draw it too. `app` = the Main.qml root.
Item {
    id: cb
    property var app: null   // the Main.qml root
    property string iconName: ""
    property string tip: ""
    property bool danger: false
    property bool active: false
    property bool busy: false
    // `live` (NOT Item.enabled): a disabled Item kills its MouseArea, and hover must
    // survive so the tooltip can explain WHY the button is off ("Hanging up…").
    property bool live: true
    property color iconColor: Th.callbarButtonIcon
    signal clicked()
    // 0.8.33: `hasMenu` draws the little ▴ (settings behind this button); `menuOpen` flips it.
    property bool hasMenu: false
    property bool menuOpen: false
    signal menuClicked()
    implicitWidth: 50
    implicitHeight: 50
    // 11.0.2: the glyph's size (the call bar's small pin: a 32 px button with a 16 px glyph)
    property int glyphSize: 22
    opacity: live ? 1.0 : 0.5
    activeFocusOnTab: true
    Keys.onSpacePressed: if (live && !busy) cb.clicked()
    Keys.onReturnPressed: if (live && !busy) cb.clicked()
    // A11y: the round buttons are icon-only; expose the tooltip text as the accessible
    // name so screen readers announce "Mute", "Hang up", etc. (every call site sets tip).
    Accessible.role: Accessible.Button
    Accessible.name: cb.tip
    Accessible.onPressAction: cb.clicked()
    // `slashed`: draw a diagonal strike over the glyph — the unambiguous "this is
    // OFF" cue for toggles whose icon can't change per-state (camera has no
    // breeze camera-disabled glyph; friends read the old accent/no-accent fill
    // difference as "everything looks greyed out").
    property bool slashed: false
    // `tinted`: a third fill colour for "on, but not in the red sense" — game chat (blue), push-to-talk speaking
    // (green), the aside corner (gold). Danger still wins (a muted mic is red even in game chat).
    // themes (desktop C2): `variant` names it and the fill / hover / pressed / frame are the callbar.button.<variant>.*
    // tokens (they were Qt.lighter/darker of a `tint` colour — the same values; the web's CallButton has the same
    // three variants).
    property bool tinted: false
    property string variant: "gameChat"   // "gameChat" | "ptt" | "aside"
    readonly property color _tintBg: variant === "ptt" ? Th.callbarButtonPttBg
                                   : (variant === "aside" ? Th.callbarButtonAsideBg : Th.callbarButtonGameChatBg)
    readonly property color _tintBgHover: variant === "ptt" ? Th.callbarButtonPttBgHover
                                        : (variant === "aside" ? Th.callbarButtonAsideBgHover : Th.callbarButtonGameChatBgHover)
    readonly property color _tintBgPressed: variant === "ptt" ? Th.callbarButtonPttBgPressed
                                          : (variant === "aside" ? Th.callbarButtonAsideBgPressed : Th.callbarButtonGameChatBgPressed)
    readonly property color _tintBorder: variant === "ptt" ? Th.callbarButtonPttBorder
                                       : (variant === "aside" ? Th.callbarButtonAsideBorder : Th.callbarButtonGameChatBorder)
    // `badge`: a small text glyph at the bottom-right of the icon (🎮 = game chat).
    property string badge: ""
    // 10.5: an outer status ring (red = talking into a muted mic; amber = the call isn't getting
    // this mic) and a pulse for the muted talk. Outside the fill, so the glyph stays readable.
    // The pulse runs only while it is true (seconds), never in Reduce motion.
    property color ringColor: "transparent"
    property bool pulse: false
    Rectangle {
        anchors.centerIn: parent
        width: parent.width + 8; height: width
        radius: width / 2
        color: "transparent"
        border.width: 2
        border.color: cb.ringColor
        visible: cb.ringColor.a > 0
    }
    Rectangle {
        id: cbPulse
        anchors.centerIn: parent
        width: parent.width; height: width
        radius: width / 2
        color: "transparent"
        border.width: 3
        border.color: Th.callbarButtonPulseRing
        opacity: 0
        visible: cbPulseAnim.running
        // 11.0 (GPU.md §2.4, R9): ONE pulse per warning — its rising edge; it looped every 900 ms for the whole
        // episode — then only the static red ring (ringColor). None while the motion switch is off (unfocused,
        // Reduce motion): the warning just shows the ring.
        ParallelAnimation {
            id: cbPulseAnim
            onStopped: { cbPulse.opacity = 0; cbPulse.scale = 1.0 }
            NumberAnimation { target: cbPulse; property: "scale"; from: 1.0; to: 1.55; duration: Motion.durVeryLong; easing.type: Easing.OutCubic }
            NumberAnimation { target: cbPulse; property: "opacity"; from: 0.9; to: 0.0; duration: Motion.durVeryLong; easing.type: Easing.OutCubic }
        }
    }
    onPulseChanged: if (cb.pulse && Motion.animate) cbPulseAnim.restart()
    Rectangle {
        anchors.fill: parent
        radius: width / 2
        color: {
            var hov = cbMa.containsMouse && cb.live
            var press = cbMa.pressed && cb.live
            if (cb.danger)
                return press ? Th.callbarButtonDangerBgPressed
                     : (hov ? Th.callbarButtonDangerBgHover : Th.callbarButtonDangerBg)
            if (cb.tinted)
                return press ? cb._tintBgPressed : (hov ? cb._tintBgHover : cb._tintBg)
            if (cb.active)
                return press ? Th.callbarButtonActiveBgPressed
                     : (hov ? Th.callbarButtonActiveBgHover : Th.callbarButtonActiveBg)
            // OFF: brighter fill than the old 0.10 wash so the button reads as a
            // *button that is off*, not a disabled ghost (live friends feedback).
            return press ? Th.callbarButtonBgPressed : (hov ? Th.callbarButtonBgHover : Th.callbarButtonBg)
        }
        border.width: cb.activeFocus ? 2 : 1
        border.color: cb.activeFocus ? Th.callbarButtonBorderFocus
            : cb.danger ? Th.callbarButtonDangerBorder
            : cb.tinted ? cb._tintBorder
            : cb.active ? Th.callbarButtonActiveBorder : Th.callbarButtonBorder
        Behavior on color { ColorAnimation { duration: Motion.dur } }
    }
    ThIcon {
        anchors.centerIn: parent
        width: cb.glyphSize; height: cb.glyphSize
        app: cb.app
        name: cb.iconName
        color: cb.iconColor
        // Windows has no KIconThemes recolor machinery: an icon name that misses
        // the hand-built breeze-DARK rcc falls back to plain breeze and renders
        // its BAKED #232629 glyph — near-invisible on the dark bar. isMask forces
        // monochrome tinting with `color` regardless of what's baked in.
        // 0.8.26: ThIcon — an icon pack can animate the glyph (hover / on).
        isMask: true
        hovered: cbMa.containsMouse && cb.live
        active: cb.active || cb.tinted
        visible: !cb.busy
        scale: (cbMa.containsMouse && cb.live) ? 1.14 : 1.0
        Behavior on scale { NumberAnimation { duration: Motion.dur; easing.type: Easing.OutBack } }
    }
    Rectangle {
        // The OFF slash: 45° bar across the glyph. pal.danger-tinted edge on a
        // light core so it reads on both accent and translucent fills.
        visible: cb.slashed && !cb.busy
        anchors.centerIn: parent
        width: 30; height: 3
        radius: 1.5
        rotation: -45
        color: Th.callbarButtonSlashFill
        border.width: 1
        border.color: Th.callbarButtonSlashBorder
    }
    Text {
        textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
        visible: cb.badge.length > 0 && !cb.busy
        text: cb.badge
        anchors.right: parent.right; anchors.bottom: parent.bottom
        anchors.rightMargin: 4; anchors.bottomMargin: 3
        font.pixelSize: 13
        color: Th.callbarButtonBadgeFg
        style: Text.Outline; styleColor: Th.callbarButtonBadgeShadow
    }
    ThBusyIndicator {   // 11.0 (GPU.md §2.5): still
        anchors.centerIn: parent
        size: 26
        color: cb.iconColor
        visible: cb.busy
        running: cb.busy
    }
    MouseArea {
        id: cbMa
        anchors.fill: parent
        hoverEnabled: true
        cursorShape: cb.live ? Qt.PointingHandCursor : Qt.ArrowCursor
        onClicked: if (cb.live) cb.clicked()
    }
    // 0.8.33: the ▴ on buttons that have settings behind them (mic / speaker / camera /
    // share audio): click it for the slide-out; the round button keeps its one-click action.
    Rectangle {
        id: chev
        visible: cb.hasMenu
        width: 20; height: 20; radius: 10
        anchors.right: parent.right; anchors.top: parent.top
        anchors.rightMargin: -5; anchors.topMargin: -5
        color: (chevMa.containsMouse || cb.menuOpen) ? Th.callbarChevronBgHover : Th.callbarChevronBg
        border.width: 1
        border.color: (chevMa.containsMouse || cb.menuOpen) ? Th.callbarChevronBorderHover : Th.callbarChevronBorder
        scale: chevMa.containsMouse ? 1.15 : 1.0
        Behavior on scale { NumberAnimation { duration: Motion.dur; easing.type: Easing.OutBack } }
        Behavior on color { ColorAnimation { duration: Motion.dur } }
        Canvas {
            id: chevGlyph
            anchors.centerIn: parent
            width: 10; height: 6
            rotation: cb.menuOpen ? 180 : 0
            Behavior on rotation { NumberAnimation { duration: Motion.dur } }
            // the stroke is a token: repaint on a theme switch
            Connections { target: Th; function onRevisionChanged() { chevGlyph.requestPaint() } }
            onPaint: {
                var c = getContext("2d"); c.clearRect(0, 0, width, height)
                c.strokeStyle = Th.callbarChevronIcon; c.lineWidth = 1.8; c.lineCap = "round"; c.lineJoin = "round"
                c.beginPath(); c.moveTo(1.2, 5); c.lineTo(5, 1.2); c.lineTo(8.8, 5); c.stroke()
            }
        }
        MouseArea {
            id: chevMa
            anchors.fill: parent
            anchors.margins: -3 // a fatter hit target than the 20 px pill
            hoverEnabled: true
            cursorShape: Qt.PointingHandCursor
            onClicked: cb.menuClicked()
        }
        Controls.ToolTip.text: "Settings"
        Controls.ToolTip.visible: chevMa.containsMouse
        Controls.ToolTip.delay: 400
    }
    Controls.ToolTip.text: cb.tip
    Controls.ToolTip.visible: cbMa.containsMouse && cb.tip.length > 0
    Controls.ToolTip.delay: 400
}
