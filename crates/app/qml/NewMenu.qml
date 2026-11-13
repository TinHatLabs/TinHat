// NewMenu — the room column's + (contract actions.json menus.new): New DM… and New room…, in that order, everywhere.
// In a space the room goes into it; on Home / People it is a room of its own (QA D21, 12.1.12: on Home the + offered
// only New DM, and the way to a plain room went through "Join a space" → "Make a space instead" → unticking "Make this
// a space"). Main.qml opens the dialogs. Tin Hat Web: nav/menus.tsx NewMenu + nav/newMenu.ts (the same rows).
// Test: scripts/new-menu-test.qml (against the contract).
import QtQuick
import QtQuick.Controls as Controls

Controls.Menu {
    id: menu
    modal: false
    // the rail's selection when the menu opens: a space id ("!…") or Home / People, and its label
    property string activeSpace: ""
    property string activeLabel: ""
    signal dmRequested()
    // where New room puts the room: the selected space, or "" / "" on Home / People (a room of its own; the web's
    // newRoomOpen)
    signal roomRequested(string spaceId, string spaceName)
    function roomTarget() {
        var inSpace = menu.activeSpace.indexOf("!") === 0
        return { spaceId: inSpace ? menu.activeSpace : "", spaceName: inSpace ? menu.activeLabel : "" }
    }
    Controls.MenuItem {
        objectName: "dm.new"
        text: "New DM…"
        icon.name: "document-edit-symbolic"
        onTriggered: menu.dmRequested()
    }
    Controls.MenuItem {
        objectName: "room.create"
        text: "New room…"
        icon.name: "folder-new-symbolic"
        onTriggered: { var t = menu.roomTarget(); menu.roomRequested(t.spaceId, t.spaceName) }
    }
}
