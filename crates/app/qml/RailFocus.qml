// RailFocus — accessibility round 2 (M2): makes a rail slot (a space, a meta space, a DM, "+N", "+") a keyboard and
// screen-reader button. Put it inside the slot (it fills it and takes no pointer input): the slot becomes a Tab stop
// named `name` (role button; `current` = the space on screen, read as selected), Return / Enter / Space = `activated`
// (what a left click does), the Menu key / Shift+F10 = `menuRequested` (what a right click does; nothing when unset).
// The keyboard focus draws the rail's focus ring (rail.ring.focus, as the web's .bubble:focus-visible) around the
// 44 px disc. The web's rail bubbles are <button>s (src/features/nav/SpaceRail.tsx): the same Tab stops and keys.
import QtQuick
import dev.tinhat.app   // Th: rail.ring.focus

Item {
    id: rf
    property string name: ""
    property bool current: false
    property bool hasMenu: false
    property real ringRadius: 24          // the disc's radius + 2 (a circle; the slot passes its squircle's)
    property real discSize: 44
    signal activated()
    signal menuRequested()
    anchors.fill: parent
    activeFocusOnTab: true
    // only the keyboard focuses it (it takes no pointer input): a menu it opens takes the keys (KeyMenu reads this)
    readonly property bool visualFocus: activeFocus
    Accessible.role: Accessible.Button
    Accessible.name: rf.name
    Accessible.selectable: true
    Accessible.selected: rf.current
    Accessible.onPressAction: rf.activated()
    Keys.onPressed: (event) => {
        var k = event.key
        if (event.modifiers & (Qt.ControlModifier | Qt.AltModifier | Qt.MetaModifier)) return
        if (k === Qt.Key_Return || k === Qt.Key_Enter || k === Qt.Key_Space) { rf.activated(); event.accepted = true }
        else if (rf.hasMenu && (k === Qt.Key_Menu || (k === Qt.Key_F10 && (event.modifiers & Qt.ShiftModifier)))) { rf.menuRequested(); event.accepted = true }
    }
    Rectangle {
        anchors.centerIn: parent
        width: rf.discSize + 4; height: width
        radius: rf.ringRadius
        color: "transparent"
        border.width: 2
        border.color: Th.railRingFocus
        visible: rf.activeFocus
    }
}
