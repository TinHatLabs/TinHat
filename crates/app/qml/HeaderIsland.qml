import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import QtQuick.Dialogs
import QtQuick.Window
import QtMultimedia
import org.kde.kirigami as Kirigami
import dev.tinhat.app

// HeaderIsland.qml — islands C3 (ISLANDS-ARCH §4.2): extracted from Main.qml as it was (the flush layout, pixel-
// identical); controllers in through `app` (the Main.qml root) and the Main.qml ids it needs, intents out.
ColumnLayout {
    id: island
    property real bgRadius: 0   // 11.0 (islands C6/C7): the header island's radius (the gradient rounds itself)
    required property var app   // Main.qml root
    required property var calendarWindowRef   // Main.qml's `calendarWindow`, passed in (never the same name: it would shadow the id)
    required property var devicesMenuRef   // Main.qml's `devicesMenu`, passed in (never the same name: it would shadow the id)
    required property var gameReelWindowRef   // Main.qml's `gameReelWindow`, passed in (never the same name: it would shadow the id)
    required property var memoriesWindowRef   // Main.qml's `memoriesWindow`, passed in (never the same name: it would shadow the id)
    required property var rightPanelRef   // Main.qml's `rightPanel`, passed in (never the same name: it would shadow the id)
    required property var watchPartyWindowRef   // Main.qml's `watchPartyWindow`, passed in (never the same name: it would shadow the id)
    // The page header column spaced these three parts 4 px apart; they keep that spacing in here, and the island is
    // shown only while one of them is (a visible, empty island would take a spacing slot of its own). Each part's rule
    // is written once, here — never read back through a child's `visible`, which turns false with the parent's.
    spacing: Kirigami.Units.smallSpacing
    readonly property bool barShown: !(island.app.callMaximized && island.app.callShown)
    readonly property bool joinShown: island.app.mCallActive && !island.app.mInCall && !island.app.mConnecting && !island.app.fakeCall
    readonly property bool wpShown: island.app.mLoggedIn && island.app.room.watchPartyOn && !island.app.watchCtl.inParty
    // selfhost (docs/GUESTS.md): THE Guests room's bar — what it shows is decided in Rust (guests_ctl.rs view_json: the
    // room state through sync, the doc's roles; null for guests, for those who may not see it, and when the server's
    // offer says `guests: false`)
    readonly property var guests: island.app.mGuests || ({})
    readonly property bool guestsShown: island.app.mLoggedIn && island.app.mGuestsHere && !!island.guests.bar
    // the doc (the roles a device can't see, the link) is read again whenever the bar shows (GuestsBar.tsx's effect)
    onGuestsShownChanged: if (island.guestsShown) island.app.guestsCtl.refresh()
    function guestsButton(id) {
        switch (id) {
        case "copy": island.app.guestsCtl.copyLink(); break
        case "newLink": island.app.guestsCtl.newLink(); break
        case "turnOff": island.app.guestsCtl.turnOff(); break
        case "turnOn": island.app.guestsCtl.turnOn(); break
        }
    }
    visible: island.barShown || island.joinShown || island.wpShown || island.guestsShown

    // Gradient room header: title + avatar + call/mic/camera/share/verify/
    // refresh + room-info. Replaces the old page actions[] toolbar; its
    // intent signals route to the same controllers the actions used.
    RoomHeaderBar {
        Layout.fillWidth: true
        // Hidden while the call fills the app — the call status bar carries the
        // restore button, so the room title chrome isn't needed.
        visible: island.barShown
        Layout.preferredHeight: visible ? implicitHeight : 0
        app: island.app

        roomName: app.mRoomName
        roomId: app.room ? app.room.roomId : ""   // Avatar seed → header face matches the room list
        // 0.8.26 — space calendar: next-up chip + the calendar button.
        upcoming: app.mCalUpcoming
        calendarOpen: island.calendarWindowRef.visible
        bgRadius: island.bgRadius
        leftInset: app.headerInset   // 10.1 (TH-SHELL-21): clear of the drawer handle (11.0: the solver's inset)
        onCalendarRequested: island.calendarWindowRef.openOrRaise()
        reelOpen: island.gameReelWindowRef.visible
        onGameReelRequested: island.gameReelWindowRef.openOrRaise()   // 10.1 (TH-REEL-18)
        onMemoriesRequested: {
            if (app.room.roomId.length === 0) return
            island.memoriesWindowRef.roomId = app.room.roomId; island.memoriesWindowRef.roomName = app.room.roomName   // 10.1 (TH-MEM-09)
            app.room.fetchMemories(); app.room.fetchClips()
            if (island.memoriesWindowRef.openOrRaise) island.memoriesWindowRef.openOrRaise(); else island.memoriesWindowRef.show()
        }
        // Real room avatar (resolved file:// from the room summary) — same source the
        // nav/space-rail uses; letter fallback only when there's genuinely none.
        avatarSource: (app.mRoomSummary && app.mRoomSummary.avatar) ? app.mRoomSummary.avatar : ""
        subtitle: (app.mRoomSummary && app.mRoomSummary.memberCount !== undefined)
            ? (app.mRoomSummary.memberCount + (app.mRoomSummary.memberCount === 1 ? " member" : " members")
               + (app.mRoomSummary.topic && app.mRoomSummary.topic.length > 0
                  ? "  ·  " + app.mRoomSummary.topic : ""))
            : ""
        callActive: app.mCallActive
        inCall: app.mInCall || app.fakeCall
        connecting: app.mConnecting

        // Header now only starts/joins a call (button hidden while in one); mic /
        // camera / share / hang-up live on the in-call control bar, and verify /
        // refresh / devices live in the Settings ▾ menu — so those signal handlers
        // were removed as dead code (RoomHeaderBar no longer has those buttons).
        onCallToggled: app.callctl.joinCall(app.room.roomId)
        infoOpen: island.rightPanelRef.opened
        onInfoRequested: island.rightPanelRef.opened ? island.rightPanelRef.close() : island.rightPanelRef.open()
        // Settings ▾ — refresh the device lists, then open the consolidated menu.
        onSettingsRequested: { app.refreshDeviceMenus(); island.devicesMenuRef.popup() }
    }

    // selfhost (docs/GUESTS.md "The apps" 2; the web's GuestsBar.tsx): under the header of THE Guests room — on: "Guest room
    // is on · [Copy guest link] [New link] [Turn off]" (New link / Turn off for managers), off: "Guest room is off · [Turn
    // on]" (managers) or "The guest room is off"; then one row per waiting guest "Ana is waiting · [Let in] [Turn away]",
    // its buttons waiting while that guest's request is out. The account-ask card's look (the web: account.ask.*).
    Rectangle {
        id: guestsBar
        Layout.fillWidth: true
        Layout.margins: Kirigami.Units.smallSpacing
        visible: island.guestsShown
        implicitHeight: guestsCol.implicitHeight + 12
        radius: 12
        color: Th.accountAskBg
        border.width: 1
        border.color: Th.accountAskBorder
        Accessible.role: Accessible.Grouping
        Accessible.name: String((island.guests.words || {}).section || "")
        ColumnLayout {
            id: guestsCol
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.verticalCenter: parent.verticalCenter
            anchors.leftMargin: 12
            anchors.rightMargin: 8
            spacing: 2
            RowLayout {
                Layout.fillWidth: true
                Layout.minimumHeight: 32
                spacing: 12
                ThIcon { Layout.preferredWidth: 16; Layout.preferredHeight: 16; app: island.app; name: "system-users-symbolic"; color: Th.accountAskIcon; isMask: true }
                Controls.Label {
                    Layout.fillWidth: true
                    text: island.guests.bar ? String(island.guests.bar.text) : ""
                    color: Th.accountAskFg
                    wrapMode: Text.Wrap
                }
                Repeater {
                    model: island.guests.bar ? island.guests.bar.buttons : []
                    delegate: ThButton {
                        required property var modelData
                        app: island.app
                        text: String(modelData.label)
                        highlighted: modelData.highlighted === true
                        flat: modelData.flat === true
                        enabled: modelData.enabled === true
                        onClicked: island.guestsButton(String(modelData.id))
                    }
                }
            }
            Repeater {
                model: island.guestsShown ? (island.guests.rows || []) : []
                delegate: ColumnLayout {
                    id: waitRow
                    required property var modelData
                    Layout.fillWidth: true
                    spacing: 2
                    // a hairline between the state line and each waiting guest
                    Rectangle { Layout.fillWidth: true; Layout.preferredHeight: 1; color: Th.accountAskBorder }
                    RowLayout {
                        Layout.fillWidth: true
                        Layout.minimumHeight: 32
                        spacing: 12
                        Controls.Label {
                            Layout.fillWidth: true
                            leftPadding: 28   // lines up with the state text (past the 16 px icon and the 12 px gap)
                            textFormat: Text.StyledText   // "<b>Ana</b> is waiting" — the name escaped in Rust
                            text: String(waitRow.modelData.html)
                            color: Th.accountAskFg
                            wrapMode: Text.Wrap
                        }
                        ThButton {
                            app: island.app
                            text: String((island.guests.words || {}).letIn || "")
                            highlighted: true
                            enabled: waitRow.modelData.enabled === true
                            onClicked: island.app.guestsCtl.letIn(String(waitRow.modelData.id))
                        }
                        ThButton {
                            app: island.app
                            text: String((island.guests.words || {}).turnAway || "")
                            flat: true
                            enabled: waitRow.modelData.enabled === true
                            onClicked: island.app.guestsCtl.turnAway(String(waitRow.modelData.id))
                        }
                    }
                }
            }
        }
    }

    // "Active call — tap to join" banner (a call is live but we're not in it).
    Rectangle {
        Layout.fillWidth: true
        Layout.margins: Kirigami.Units.smallSpacing
        visible: island.joinShown
        implicitHeight: joinRow.implicitHeight + Kirigami.Units.largeSpacing
        radius: island.app.radius
        color: Th.callJoinBannerBg
        border.width: 1
        border.color: Th.callJoinBannerBorder
        RowLayout {
            id: joinRow
            anchors.fill: parent
            anchors.leftMargin: Kirigami.Units.largeSpacing
            anchors.rightMargin: Kirigami.Units.smallSpacing
            spacing: Kirigami.Units.smallSpacing
            Controls.Label {
                Layout.fillWidth: true
                elide: Text.ElideRight
                // 10.1 (TH-CONV-14): say who is in it (the nav row carries the participants).
                text: {
                    var names = []
                    for (var i = 0; i < island.app.mNavRooms.length; i++) {
                        var r = island.app.mNavRooms[i]
                        if (!r || r.id !== island.app.room.roomId) continue
                        var ps = r.participants || []
                        for (var j = 0; j < ps.length; j++) names.push(String(ps[j].name || ps[j].mxid || ""))
                        break
                    }
                    names = names.filter(function (n) { return n.length > 0 })
                    if (names.length === 0) return "There's a call going on in this room"
                    if (names.length === 1) return names[0] + " is in a call"
                    if (names.length === 2) return names[0] + " and " + names[1] + " are in a call"
                    if (names.length === 3) return names[0] + ", " + names[1] + " and " + names[2] + " are in a call"
                    return names[0] + ", " + names[1] + " and " + (names.length - 2) + " others are in a call"
                }
                color: Th.callJoinBannerFg
            }
            GlowButton {
                app: island.app
                text: app.mConnecting ? "Joining…" : "Join"
                busy: app.mConnecting   // 10.1 (TH-MOTION-05): the header's call button already did this
                enabled: !app.mConnecting
                onClicked: app.callctl.joinCall(app.room.roomId)
            }
        }
    }

    // 10.1 (TH-WP-04): a watch party running in this room gets a passive cue and a one-click
    // join — the only way in used to be a row at the bottom of the gear menu.
    Rectangle {
        Layout.fillWidth: true
        Layout.margins: Kirigami.Units.smallSpacing
        visible: island.wpShown
        implicitHeight: wpRow.implicitHeight + Kirigami.Units.largeSpacing
        radius: island.app.radius
        color: Th.bannerWatchPartyBg
        border.width: 1
        border.color: Th.bannerWatchPartyBorder
        RowLayout {
            id: wpRow
            anchors.fill: parent
            anchors.leftMargin: Kirigami.Units.largeSpacing
            anchors.rightMargin: Kirigami.Units.smallSpacing
            spacing: Kirigami.Units.smallSpacing
            ThIcon { Layout.preferredWidth: 16; Layout.preferredHeight: 16; app: island.app; name: "video-television-symbolic"; color: Th.bannerWatchPartyIcon; isMask: true }
            Controls.Label { Layout.fillWidth: true; elide: Text.ElideRight; text: "A Media Party is on in this room"; color: Th.bannerWatchPartyFg }
            ThButton {
                app: island.app
                text: "Join"
                onClicked: {
                    if (!app.watchCtl.inParty) app.watchCtl.joinParty(app.room.roomId)
                    if (island.watchPartyWindowRef.openOrRaise) island.watchPartyWindowRef.openOrRaise(); else island.watchPartyWindowRef.show()
                }
            }
        }
    }
}
