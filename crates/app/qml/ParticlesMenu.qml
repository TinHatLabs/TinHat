// ParticlesMenu.qml — the ☰ app menu's "Particles" submenu (round 4, P-A7; the web's ambience/particlesMenu.ts): the
// open room's effect (the Chat background sheet's own write: ambience particlesPickJson = the shared particles_pick),
// "Chat background…", then the two personal switches of Settings → Appearance → Performance (the SAME stored prefs) and
// "Effects settings…". The rows are the shared particles_menu (testvectors/ambience/vectors.json). Main.qml puts it
// right after Theme; AmbienceGallery.qml (lab builds) shows the same component for the review captures.
import QtQuick
import QtQuick.Controls as Controls

Controls.Menu {
    id: menu
    /// the app root (Main.qml): ambience, settings, mUserId, mChatBackgrounds, mChatEffects, mChatEffectsPause,
    /// mCallUnfocusedSaver
    property var app
    /// the open room's RoomController (backgroundJson, canSetBackground, setBackground)
    property var roomCtl: null
    property bool roomOpen: false
    signal openSheet()
    signal openSettings()

    title: LinkGate.plain(app ? String(app.ambience.word("particles")) : "")
    // its name for a screen reader (the web's role=menu aria-label): the popup menu AND its list of rows say "Particles"
    function _name() {
        if (!menu.contentItem) return
        menu.contentItem.Accessible.name = menu.title
        if (menu.contentItem.parent) menu.contentItem.parent.Accessible.name = menu.title
    }
    Component.onCompleted: _name()
    onAboutToShow: _name()
    icon.name: "video-display-brightness-symbolic"
    readonly property string me: app && app.mUserId ? String(app.mUserId) : ""
    readonly property string bgJson: {
        var raw = roomOpen && roomCtl ? String(roomCtl.backgroundJson) : ""
        if (!app || raw.length === 0) return JSON.stringify({ kind: "unset", preset: null, effect: "none", image: null, by: "" })
        var o = JSON.parse(raw)
        return String(app.ambience.parse(JSON.stringify(o.content), o.sender))
    }
    readonly property var rows: app ? JSON.parse(app.ambience.particlesMenuJson(roomOpen, bgJson, roomOpen && !!roomCtl && roomCtl.canSetBackground === true,
        me, app.mChatBackgrounds, app.mChatEffects, app.mChatEffectsPause, app.mCallUnfocusedSaver)) : []
    function row(id) {
        for (var i = 0; i < rows.length; i++) if (rows[i].id === id) return rows[i]
        return null
    }
    // sized to its rows like the web's menus (the Theme submenu's rule)
    implicitWidth: {
        var w = 0
        for (var i = 0; i < menu.count; i++) {
            var it = menu.itemAt(i)
            if (it && it.visible) w = Math.max(w, it.implicitWidth)
        }
        return Math.max(implicitBackgroundWidth + leftInset + rightInset, w + leftPadding + rightPadding)
    }
    function run(id) {
        var r = row(id)
        if (!r || !r.enabled) return
        if (id.indexOf("effect:") === 0) {
            // the menu's one write, and only what particles_write allows ("" = NOTHING is sent: no room, no permission,
            // a greyed row, or no change); the bindings are live, so this is the room as it is now
            var c = String(app.ambience.particlesWriteJson(roomOpen, bgJson, roomOpen && !!roomCtl && roomCtl.canSetBackground === true, me, id.slice(7)))
            if (c.length > 0) roomCtl.setBackground(c)
        } else if (id === "background") menu.openSheet()
        else if (id === "effects") { app.mChatEffects = !r.checked; app.settings.applyUiPref(app.ambience.prefKey("effects"), r.checked ? "0" : "1") }
        else if (id === "pause") { app.mChatEffectsPause = !r.checked; app.settings.applyUiPref(app.ambience.prefKey("pause"), r.checked ? "0" : "1") }
        else if (id === "settings") menu.openSettings()
    }
    // one row of `rows` by id — hidden when the shared rules leave it out
    component Row: CheckMenuItem {
        property string rid
        readonly property var r: menu.row(rid)
        visible: !!r
        height: visible ? implicitHeight : 0
        text: LinkGate.plain(r ? String(r.label) : "")
        checkable: !!r && r.checked !== null
        isChecked: () => !!menu.row(rid) && menu.row(rid).checked === true
        enabled: !!r && r.enabled === true
        // applied once the menu has closed (the Theme submenu's lesson: rows rebuilt mid-trigger kept the menu open)
        onTriggered: { var id = rid; Qt.callLater(function () { menu.run(id) }) }
    }
    component Note: Controls.MenuItem {
        property string rid
        readonly property var r: menu.row(rid)
        visible: !!r
        height: visible ? implicitHeight : 0
        text: LinkGate.plain(r ? String(r.label) : "")
        enabled: false
    }
    Row { rid: "effect:rain" }
    Row { rid: "effect:embers" }
    Row { rid: "effect:snow" }
    Row { rid: "effect:fireflies" }
    Row { rid: "effect:none" }
    Note { rid: "note:readOnly" }
    Note { rid: "note:otherImage" }
    Row { rid: "background" }
    Controls.MenuSeparator { visible: menu.roomOpen; height: visible ? implicitHeight : 0 }
    Row { rid: "effects" }
    Note { rid: "note:offHere" }
    Row { rid: "pause" }
    Note { rid: "note:pauseSaver" }
    Row { rid: "settings" }
}
