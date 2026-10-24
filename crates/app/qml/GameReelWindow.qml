// GameReelWindow — the Legacy host for the Game Reel: a secondary window like the calendar's,
// opened from the app menu / the calendar's "Game TBD" affordance. The panel inside is shared
// (GameReelPanel.qml).
import QtQuick
import QtQuick.Window
import dev.tinhat.app

Window {
    id: win
    required property var app
    Shortcut { sequence: "Escape"; onActivated: win.close() }   // 10.1 (TH-A11Y-12)
    // 10.1 (TH-CAL-22 / TH-MEM-08 / TH-WP-15): remembered size (+ position off Wayland) across runs.
    WindowMemory { target: win; key: "gamereel"; store: (win.app && win.app.prefStore) ? win.app.prefStore : null }
    required property var ctl
    title: "What do we play tonight? — Tin Hat"
    width: 980
    height: 760
    minimumWidth: 640
    minimumHeight: 480
    color: Th.reelBg
    visible: false
    function openOrRaise() {
        if (!visible) visible = true
        raise(); requestActivate()
        ctl.refresh()
    }
    GameReelPanel {
        anchors.fill: parent
        app: win.app
        ctl: win.ctl
    }
}
