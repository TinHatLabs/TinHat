// ThNav.qml — the Discord-style navigation: the SPACES rail + the ROOM panel side
// by side. This single component is the room-picker in BOTH view modes — mounted as a
// persistent left sidebar (mode "persistent"), or tucked into the top-left corner and
// revealed on hover (mode "corner", for OLED users avoiding burn-in). Identical
// functionality either way; only where it's mounted differs.
import QtQuick
import QtQuick.Layouts
import org.kde.kirigami as Kirigami

RowLayout {
    id: nav
    required property var app
    property var navController
    property var audioPrefs                     // AudioPrefsController (per-participant call volume)
    property string currentRoomId: ""
    property string openRoomId: ""
    property bool sidebarCollapsed: false

    signal roomActivated(string roomId)
    signal roomContext(string roomId)
    signal messageUser(string mxid)
    signal joinRequested(string roomId, string roomName)
    signal roomCallRequested(string roomId)   // 12.1.5: a double-clicked room row → the room's call (Main.qml: call.join)
    property string myMxid: ""
    signal acceptInvite(string roomId)
    signal declineInvite(string roomId)
    signal menuRequested(var anchorItem)
    signal newMenuRequested(var anchorItem)                 // the room column's + (2026-09-29)
    signal setActivity(string game)
    // Never emitted (statuses post via RightPanel) but Main.qml binds onPostStatus on
    // both ThNav instances — removing this without touching Main.qml would break it.
    signal postStatus(string text, int ttlMin)
    signal toggleNavRequested()
    // Forwarded from the room-list footer's bug button. Main.qml binds
    // onBugReportRequested on BOTH nav modes; without this declaration the
    // persistent-mode binding assigns to a non-existent property and the ENTIRE
    // QML tree fails to load — silently, no window. That was the 0.3.5 no-boot.
    signal bugReportRequested()
    // Forwarded from the spaces rail's "+". Declared here for the same reason as
    // bugReportRequested above: Main.qml binds it on BOTH nav instances, and a binding
    // to an undeclared signal takes the whole QML tree down silently.
    signal addSpaceRequested()
    signal spaceMenuRequested(string key, string label)   // 0.8.27
    signal spaceInviteRequested(string key, bool join)    // RBAC §7.3: Join / Decline from the rail's invite menu
    signal spaceSettingsRequested(string key)             // RBAC §7.1: the room-list header gear
    // 10.6 (REACH): declared here AND bound on both ThNav instances in Main.qml (a binding to an
    // undeclared signal takes the whole QML tree down — see bugReportRequested above).
    signal dmOpenRequested(string roomId)                 // §1.3: a DM bubble on the rail → People + open it
    signal dmInviteRequested(string roomId, bool accept)  // §1.3: a DM invite's Accept / Decline from the rail
    signal privacySettingsRequested()                     // §2.7: the footer popover's "Privacy settings…"
    signal profileSettingsRequested()                     // 2026-09-29: the footer popover's "Profile settings…"
    // 11.0 (islands C6/C7): ThNav is the STATE of the nav column's islands (the rail's, the room list's); the window's
    // dock host places the islands and reads these two, and hands back its rooms island and account card.
    readonly property alias railState: railItem
    readonly property alias listState: listItem
    property Item roomsIslandItem: null
    property Item footerCardItem: null

    spacing: 0

    ThSpaceRail {
        id: railItem
        app: nav.app
        navController: nav.navController
        Layout.fillHeight: true
        Layout.preferredWidth: 68
        onAddRequested: nav.addSpaceRequested()
        onSpaceMenuRequested: (key, label) => nav.spaceMenuRequested(key, label)
        onSpaceInviteRequested: (key, join) => nav.spaceInviteRequested(key, join)
        onDmOpenRequested: (roomId) => nav.dmOpenRequested(roomId)
        onDmInviteRequested: (roomId, accept) => nav.dmInviteRequested(roomId, accept)
    }
    ThRoomList {
        id: listItem
        app: nav.app
        roomsIslandItem: nav.roomsIslandItem
        footerCardItem: nav.footerCardItem
        navController: nav.navController
        audioPrefs: nav.audioPrefs
        currentRoomId: nav.currentRoomId
        openRoomId: nav.openRoomId
        sidebarCollapsed: nav.sidebarCollapsed
        Layout.fillHeight: true
        Layout.fillWidth: true
        onRoomActivated: (roomId) => nav.roomActivated(roomId)
        onRoomContext: (roomId) => nav.roomContext(roomId)
        onMessageUser: (mxid) => nav.messageUser(mxid)
        onJoinRequested: (roomId, roomName) => nav.joinRequested(roomId, roomName)
        onRoomCallRequested: (roomId) => nav.roomCallRequested(roomId)
        myMxid: nav.myMxid
        onAcceptInvite: (roomId) => nav.acceptInvite(roomId)
        onDeclineInvite: (roomId) => nav.declineInvite(roomId)
        onMenuRequested: (anchorItem) => nav.menuRequested(anchorItem)
        onNewMenuRequested: (anchorItem) => nav.newMenuRequested(anchorItem)
        onSetActivity: (game) => nav.setActivity(game)
        onToggleNavRequested: nav.toggleNavRequested()
        onBugReportRequested: nav.bugReportRequested()
        onSpaceSettingsRequested: (key) => nav.spaceSettingsRequested(key)
        onPrivacySettingsRequested: nav.privacySettingsRequested()
        onProfileSettingsRequested: nav.profileSettingsRequested()
    }
}
