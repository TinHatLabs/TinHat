import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the rooms.* / account.* / status.* tokens

// RoomsIsland.qml — islands C3 (ISLANDS-ARCH §4.2): extracted from Main.qml as it was (the flush layout, pixel-
// identical); controllers in through `app` (the Main.qml root) and the Main.qml ids it needs, intents out.
ColumnLayout {
    id: island
    required property var app   // the Main.qml root (panel.app)
    // islands C3: the room list column's top part — the space header, the filter, the rooms (or the invite card /
    // the empty state); the widget stack and the account card stay below it in ThRoomList. Spaced like that column.
    spacing: 0
    // what ThRoomList still reads: the filter (the widget stack's height) and the list hover (no resort under it)
    property alias filterField: filterField
    property alias listHover: listHover
    required property var panelRef   // a ThRoomList's `panel`, passed in (never the same name: it would shadow the id)
    required property var roomModelRef   // a ThRoomList's `roomModel`, passed in (never the same name: it would shadow the id)
    required property var sharedVolMenuRef   // a ThRoomList's `sharedVolMenu`, passed in (never the same name: it would shadow the id)

    // ── header: active space name ───────────────────────────────────────
    Item {
        Layout.fillWidth: true
        Layout.preferredHeight: 44
        HoverHandler { id: headerHover }
        Kirigami.Heading {
            textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
            anchors.left: parent.left; anchors.right: parent.right
            anchors.verticalCenter: parent.verticalCenter
            anchors.leftMargin: Kirigami.Units.largeSpacing
            anchors.rightMargin: Kirigami.Units.largeSpacing + 32 + (headerGear.visible ? 28 : 0)   // room for the + (and the gear)
            level: 4
            elide: Text.ElideRight
            text: island.panelRef.app ? island.panelRef.app.mNavActiveLabel : "Rooms"
            color: Th.roomsHeaderFg
        }
        // 2026-09-29: the + opens its own small menu — New DM, and New room while a space is selected
        // (Main.qml newMenu; it used to open the whole app menu, a duplicate of the account island's).
        ThToolButton {
            id: headerAdd
            anchors.right: parent.right
            anchors.rightMargin: Kirigami.Units.smallSpacing
            anchors.verticalCenter: parent.verticalCenter
            implicitWidth: 28; implicitHeight: 28
            app: island.panelRef._a
            iconName: "list-add-symbolic"
            iconSize: 16
            tip: island.panelRef.activeIsRealSpace ? "New DM or room" : "New DM"
            onClicked: island.panelRef.newMenuRequested(headerAdd)
        }
        // RBAC §7.1: the active space's settings — a gear that fades in on hover (real, joined spaces).
        ThToolButton {
            id: headerGear
            anchors.right: headerAdd.left
            anchors.rightMargin: 2
            anchors.verticalCenter: parent.verticalCenter
            implicitWidth: 28; implicitHeight: 28
            visible: island.panelRef.activeIsRealSpace && !island.panelRef.activeInvited
            opacity: (headerHover.hovered || hovered || activeFocus) ? 1 : 0
            Behavior on opacity { NumberAnimation { duration: island.panelRef._a.dur } }
            app: island.panelRef._a
            iconName: "configure-symbolic"
            iconSize: 16
            tip: "Space settings  (Ctrl+Shift+,)"
            onClicked: island.panelRef.spaceSettingsRequested(island.panelRef.activeSpaceKey)
        }
        Kirigami.Separator { anchors.bottom: parent.bottom; width: parent.width; height: 1 }
    }

    // ── Pad (docs/PAD.md): "You're in {room}'s call on {device} · Pad · Join" ──
    PadBar {
        Layout.fillWidth: true
        Layout.leftMargin: Kirigami.Units.smallSpacing
        Layout.rightMargin: Kirigami.Units.smallSpacing
        Layout.topMargin: Kirigami.Units.smallSpacing
        app: island.app
    }

    // ── filter ──────────────────────────────────────────────────────────
    Kirigami.SearchField {
        id: filterField
        Layout.fillWidth: true
        Layout.margins: Kirigami.Units.smallSpacing
        visible: !island.panelRef.activeInvited   // nothing to filter until you join
        placeholderText: "Filter rooms…"
        onTextChanged: { island.panelRef.query = text; roomView.positionViewAtBeginning() }   // 10.1 (TH-NAV-12): the match is never off-screen
        Keys.onEscapePressed: text = ""
        // Enter opens the first match; Down hands focus to the list.
        onAccepted: island.panelRef.openFirstMatch()
        Keys.onDownPressed: roomView.forceActiveFocus()
    }

    // Ctrl+K on a pending invite (Main.qml showInvite): bring its row into view as soon as the list holds it — the rail
    // may be switching to People / Home right now and the rows follow a moment later; given up after 3 s.
    property string _revealId: ""
    function revealRoom(id) {
        island._revealId = String(id || "")
        revealGiveUp.restart()
        island._tryReveal()
    }
    function _tryReveal() {
        if (island._revealId.length === 0 || !island.roomModelRef) return
        for (var i = 0; i < island.roomModelRef.count; i++) {
            if (island.roomModelRef.get(i).rid === island._revealId) {
                roomView.positionViewAtIndex(i, ListView.Contain)
                island._revealId = ""
                return
            }
        }
    }
    Connections {
        target: island.panelRef
        function on_ModelRevChanged() { island._tryReveal() }
    }
    Timer { id: revealGiveUp; interval: 3000; onTriggered: island._revealId = "" }

    // ── rooms ───────────────────────────────────────────────────────────
    ListView {
        id: roomView
        Layout.fillWidth: true
        Layout.fillHeight: true
        visible: !island.panelRef.activeInvited
        clip: true
        spacing: 1
        bottomMargin: 8   // 11.0: the account card is its own island now (no floating card to clear)
        model: island.roomModelRef
        HoverHandler {
            id: listHover
            onHoveredChanged: if (!hovered && island.panelRef._pendingResort) { island.panelRef._pendingResort = false; island.panelRef._syncRooms() }
        }
        boundsBehavior: Flickable.StopAtBounds
        Controls.ScrollBar.vertical: FoxScrollBar { app: island.panelRef._a }   // 10.1 (TH-DS-05): the house scrollbar everywhere

        delegate: Item {
            id: row
            required property var room
            width: ListView.view ? ListView.view.width : 0
            // Filter-as-visibility: typing in the filter only toggles row heights,
            // it never rebuilds delegates.
            readonly property bool matches: {
                var q = island.panelRef.query.trim().toLowerCase()
                return q.length === 0 || String(room.name).toLowerCase().indexOf(q) >= 0
            }
            visible: matches
            readonly property bool isInvite: room.membership === "invite"
            // Listed but not joined: a public/space-restricted room discovered in the
            // space. Never a private one — the hierarchy call filters those out.
            readonly property bool notJoined: room.membership === "none"
            readonly property bool unjoined: isInvite || notJoined
            readonly property bool selected: room.id === island.panelRef.currentRoomId
            // The room we are IN shows the live SFU roster (instant); the others
            // show membership state, refreshed on join/leave edges + a 3 s in-call ticker.
            readonly property var parts: (island.panelRef.app && island.panelRef.app.mInCall && room.id === island.panelRef.app.mCallRoomId && island.panelRef.app.liveCallParts.length > 0)
                ? island.panelRef.app.liveCallParts : (room.participants || [])
            readonly property bool isDM: room.dm === true
            // 10.6 (REACH §2.6 / A2.14 / A4.5): the DM's one other person as the presence service
            // sees them (null = absent or not sharing — no dot, no line).
            readonly property string peer: isDM ? String(room.peer || "") : ""
            readonly property var pres: (peer.length > 0 && island.panelRef.app && island.panelRef.app.presenceOf) ? island.panelRef.app.presenceOf(peer) : null
            readonly property string presRoom: (pres && pres.state === "call" && pres.room && island.panelRef.app && island.panelRef.app.roomNameOf)
                ? island.panelRef.app.roomNameOf(String(pres.room)) : ""
            // A4.5: a custom status is the DM row's secondary line; the state alone goes in the tooltip.
            readonly property bool showPresLine: !!pres && !!pres.chip
            // gaming surface — non-DM only, null-safe
            readonly property var activity: (!isDM && room.activity) ? room.activity : null
            readonly property string activityGame: activity ? String(activity.game || "") : ""
            readonly property string activityEmoji: activity ? String(activity.emoji || "") : ""
            readonly property var statuses: (!isDM && room.statuses) ? room.statuses : []
            readonly property bool showActivity: activityGame.length > 0
            // inline "what are you playing?" editor: only on the currently-open, non-DM row
            readonly property bool isOpenRow: !isDM && room.id === island.panelRef.openRoomId
            // The editor is HIDDEN by default; the pencil next to "Now playing" opens it.
            // Bound to the panel-level editingRoomId (not a local flag) so a delegate
            // rebuild from a model refresh keeps the editor open while typing.
            readonly property bool editing: !isDM && room.id === island.panelRef.editingRoomId
            readonly property bool editorShown: isOpenRow && editing
            // Height = the row's actual CONTENT EXTENT, not a hand-summed formula.
            //
            // The blocks below are positioned by a three-level anchor chain
            // (partCol sits under editorRow or nowPlaying or mainRow; statusCol sits
            // under whichever of those is last). Summing their heights separately
            // means the same ordering is encoded in two places, and any disagreement
            // makes the row shorter than what it draws — which, because anchored
            // children are not clipped to the delegate, paints the call participants
            // straight over the room below. Measuring `y + height` per block cannot
            // disagree with where they actually are.
            implicitHeight: matches ? (Math.max(
                  mainRow.y + mainRow.height,
                  presLine.visible ? presLine.y + presLine.height : 0,
                  nowPlaying.visible ? nowPlaying.y + nowPlaying.height : 0,
                  editorRow.visible ? editorRow.y + editorRow.height : 0,
                  partCol.visible ? partCol.y + partCol.height : 0,
                  statusCol.visible ? statusCol.y + statusCol.height : 0
                ) + 2) : 0

            // hover / selected wash
            readonly property color washColor: row.selected
                ? Th.roomsItemBgSelected
                : (rowMa.containsMouse ? Th.roomsItemBgHover : "transparent")
            Rectangle {
                anchors.fill: parent
                anchors.leftMargin: Kirigami.Units.smallSpacing
                anchors.rightMargin: Kirigami.Units.smallSpacing
                anchors.topMargin: 1; anchors.bottomMargin: 1
                radius: island.panelRef._a.radius
                color: row.washColor
                Behavior on color { ColorAnimation { duration: island.panelRef._a.dur } }
            }

            MouseArea {
                id: rowMa
                anchors.fill: parent  // whole delegate incl. the call-participant strip
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                acceptedButtons: Qt.LeftButton | Qt.RightButton
                onClicked: (m) => {
                    if (m.button === Qt.RightButton) island.panelRef.roomContext(row.room.id)
                    else if (row.notJoined) island.panelRef.joinRequested(row.room.id, row.room.name)
                    else if (!row.isInvite) island.panelRef.roomActivated(row.room.id)
                }
            }
            // 10.6 (REACH §2.6 item 2): a DM row with no secondary line carries the state here.
            Controls.ToolTip.text: row.pres ? island.panelRef._tipSafe(String(row.room.name || "") + " · " + String(row.pres.label || "")
                                              + (row.presRoom.length > 0 ? " · " + row.presRoom : "")) : ""
            Controls.ToolTip.visible: !!row.pres && rowMa.containsMouse
            Controls.ToolTip.delay: 600

            // main room row
            RowLayout {
                id: mainRow
                width: parent.width
                height: 40
                spacing: Kirigami.Units.smallSpacing
                Item { Layout.preferredWidth: Kirigami.Units.smallSpacing; Layout.preferredHeight: 1 }
                Avatar {
                    app: island.panelRef._a
                    Layout.preferredWidth: 28; Layout.preferredHeight: 28
                    size: 28
                    // 10.1 (TH-NAV-06): people are circles, rooms are squircles — the universal cue.
                    radius: row.isDM ? 14 : 8
                    name: row.room.name
                    seed: Seeds.avatarSeed(row.room.id, row.isDM ? row.room.peer : "")   // §6: a DM is its person, a room its id
                    imageSource: row.room.avatar || ""
                    showDot: row.room.call === true
                    dotColor: Th.roomsItemCallDot
                    // 10.6: the peer's presence (takes the call dot's corner while set)
                    presence: row.pres ? String(row.pres.state || "") : ""
                    presenceColor: (row.pres && row.pres.dot) ? Th.colourRef(row.pres.dot) : ((row.pres && row.pres.state === "away") ? Th.presenceAwayDot : Th.presenceOnlineDot)
                    presenceText: row.pres ? String(row.pres.label || "") : ""
                    presenceRing: row.selected ? Th.roomsItemPresenceRingSelected
                                               : (rowMa.containsMouse ? Th.roomsItemPresenceRingHover : Th.roomsItemPresenceRing)
                }
                // 10.1 (TH-NAV-06): the favourite star — the treatment RoomTile.qml wrote and nothing mounted.
                ThIcon {
                    visible: row.room.favourite === true
                    Layout.preferredWidth: 12; Layout.preferredHeight: 12
                    app: island.panelRef._a
                    name: "starred-symbolic"
                    color: Th.roomsItemFavouriteIcon
                    isMask: true
                }
                Controls.Label {
                    textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                    Layout.fillWidth: true
                    Layout.minimumWidth: 72   // 10.1 (TH-NAV-09): the invite actions can no longer crush the name to nothing
                    text: row.room.name
                    elide: Text.ElideRight
                    // Dimmed while you are not in it, so "haven't joined" reads at a
                    // glance rather than needing the badge to be noticed.
                    color: row.notJoined ? Th.roomsItemNotJoinedFg : Th.roomsItemFg
                    // …and while it is muted (TH-NAV-06): you did that, the list should say so.
                    opacity: row.room.muted === true ? 0.55 : 1
                    font.bold: (row.room.unread || 0) > 0 || row.isInvite
                }
                ThIcon {
                    visible: row.room.muted === true && !row.isInvite
                    Layout.preferredWidth: 14; Layout.preferredHeight: 14
                    app: island.panelRef._a
                    name: "notifications-disabled-symbolic"
                    color: Th.roomsItemMutedIcon
                    isMask: true
                }
                // The state badge. Invites are an action waiting on you, so they get
                // the accent; a joinable room is just information.
                Rectangle {
                    visible: row.unjoined
                    radius: height / 2
                    implicitHeight: badgeText.implicitHeight + 4
                    implicitWidth: badgeText.implicitWidth + 12
                    color: row.isInvite ? Th.roomsPillInviteBg
                                        : Th.roomsPillNotJoinedBg
                    border.width: row.isInvite ? 0 : 1
                    border.color: Th.roomsPillNotJoinedBorder
                    Controls.Label {
                        id: badgeText
                        anchors.centerIn: parent
                        text: row.isInvite ? "Invited" : "Not joined"
                        color: row.isInvite ? Th.roomsPillInviteFg : Th.roomsPillNotJoinedFg
                        font.pointSize: Kirigami.Theme.smallFont.pointSize
                    }
                }
                // invite actions
                // 10.1 (TH-NAV-09): one visual language — house buttons, compact.
                ThButton {
                    visible: row.isInvite
                    app: island.panelRef._a
                    text: "Accept"
                    implicitHeight: 26
                    onClicked: island.panelRef.acceptInvite(row.room.id)
                }
                ThToolButton {
                    visible: row.isInvite
                    app: island.panelRef._a
                    iconName: "dialog-close-symbolic"
                    tip: "Decline invite"
                    onClicked: island.panelRef.declineInvite(row.room.id)
                }
                // unread badge
                UnreadBadge {
                    app: island.panelRef._a
                    visible: !row.isInvite && (row.room.unread || 0) > 0
                    Layout.rightMargin: Kirigami.Units.smallSpacing
                    count: row.room.unread || 0
                    highlight: row.room.highlight === true
                }
                Item { visible: !row.isInvite && (row.room.unread || 0) === 0; Layout.preferredWidth: 1 }
            }

            // 10.6 (A4.5): a DM's custom status as its secondary line — "🎯 Looking for group · In a call".
            Controls.Label {
                id: presLine
                visible: row.showPresLine
                anchors.top: mainRow.bottom
                anchors.topMargin: -9   // hug the name: the row's content sits centred in its 40 px
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.leftMargin: Kirigami.Units.smallSpacing * 3 + 28
                anchors.rightMargin: Kirigami.Units.smallSpacing * 2
                height: visible ? implicitHeight + 4 : 0
                text: row.showPresLine ? String(row.pres.label || "") + (row.presRoom.length > 0 ? " · " + row.presRoom : "") : ""
                textFormat: Text.PlainText   // 10.6.0 review: a status is never markup
                color: row.showPresLine && row.pres.chipColor ? row.pres.chipColor : Th.roomsItemStatusFg
                font.pointSize: (Kirigami.Theme.smallFont.pointSize > 0 ? Kirigami.Theme.smallFont.pointSize : Math.max(7, Kirigami.Theme.defaultFont.pointSize * 0.85))
                elide: Text.ElideRight
                maximumLineCount: 1
            }

            // "Now playing: <game>" — subtle accent line under the room row (non-DM only),
            // with a pencil to open the editor. Shows when there's activity OR on the open
            // non-DM row (so the user can still open the editor to SET an activity).
            RowLayout {
                id: nowPlaying
                visible: row.showActivity || (row.isOpenRow && !row.editing)
                anchors.top: mainRow.bottom
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.leftMargin: Kirigami.Units.gridUnit * 2
                anchors.rightMargin: Kirigami.Units.smallSpacing
                height: visible ? implicitHeight : 0
                spacing: Kirigami.Units.smallSpacing
                // A real icon unless the activity carries its own emoji.
                Kirigami.Icon {
                    visible: row.activityEmoji.length === 0
                    source: "applications-games-symbolic"
                    Layout.preferredWidth: 14; Layout.preferredHeight: 14
                    color: row.showActivity ? Th.roomsActivityIcon : Th.roomsActivityIconOff
                }
                Controls.Label {
                    textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                    Layout.fillWidth: true
                    readonly property string prefix: row.activityEmoji.length > 0
                        ? row.activityEmoji + " " : ""
                    text: row.showActivity
                        ? (prefix + "Now playing: " + row.activityGame)
                        : (prefix + "Set now playing…")
                    elide: Text.ElideRight
                    color: row.showActivity ? Th.roomsActivityFg : Th.roomsActivityPlaceholder
                    font.pixelSize: Math.round(Kirigami.Theme.smallFont.pixelSize)
                    font.italic: !row.showActivity
                }
                // Pencil — open the inline editor (open non-DM row only).
                Controls.ToolButton {
                    visible: row.isOpenRow
                    icon.name: "document-edit-symbolic"
                    Layout.preferredHeight: 22
                    Controls.ToolTip.text: "Edit now playing"
                    Controls.ToolTip.visible: hovered
                    Controls.ToolTip.delay: 400
                    onClicked: { island.panelRef.editingRoomId = row.room.id; activityField.forceActiveFocus() }
                }
            }

            // inline "What are you playing?" editor for the OPEN room (non-DM only).
            // Opened by the pencil; Enter sets the activity (and closes), ✕ clears it.
            RowLayout {
                id: editorRow
                visible: row.editorShown
                anchors.top: nowPlaying.visible ? nowPlaying.bottom : mainRow.bottom
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.leftMargin: Kirigami.Units.gridUnit * 2
                anchors.rightMargin: Kirigami.Units.smallSpacing
                height: visible ? implicitHeight : 0
                spacing: Kirigami.Units.smallSpacing
                Controls.TextField {
                    id: activityField
                    Layout.fillWidth: true
                    text: row.activityGame
                    placeholderText: "What are you playing?"
                    font.pixelSize: Math.round(Kirigami.Theme.smallFont.pixelSize)
                    onAccepted: { island.panelRef.setActivity(text.trim()); island.panelRef.editingRoomId = ""; focus = false }
                }
                Controls.ToolButton {
                    icon.name: "edit-clear-symbolic"
                    Controls.ToolTip.text: "Clear now playing"
                    Controls.ToolTip.visible: hovered
                    Controls.ToolTip.delay: 400
                    onClicked: { activityField.text = ""; island.panelRef.setActivity(""); island.panelRef.editingRoomId = "" }
                }
            }

            // connected-users bubbles (Discord voice-channel style), under the room
            ColumnLayout {
                id: partCol
                anchors.top: row.editorShown ? editorRow.bottom
                             : (nowPlaying.visible ? nowPlaying.bottom : (presLine.visible ? presLine.bottom : mainRow.bottom))
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.leftMargin: Kirigami.Units.gridUnit * 2
                anchors.rightMargin: Kirigami.Units.smallSpacing
                spacing: 1
                visible: row.parts.length > 0
                Repeater {
                    // 10.1 (TH-NAV-04): six rows for a room you are not in — a busy call used to swallow the list.
                    model: row.selected ? row.parts : row.parts.slice(0, 6)
                    delegate: Item {
                        id: partRow
                        required property var modelData
                        readonly property string partMxid: String(modelData.mxid || "")
                        // Live mute reflection for the row glyph. mutedFor is a
                        // non-notifying invokable, so these bindings depend on
                        // panel.volRev — bumped by the shared volume menu after every
                        // mute commit — to re-query immediately after a toggle.
                        // Three categories: mic / stream (screenshare) / soundboard (music).
                        property bool micMuted: {
                            void island.panelRef.volRev
                            return island.panelRef.audioPrefs && partRow.partMxid.length > 0
                                ? island.panelRef.audioPrefs.mutedFor(partRow.partMxid, "mic") : false
                        }
                        property bool streamMuted: {
                            void island.panelRef.volRev
                            return island.panelRef.audioPrefs && partRow.partMxid.length > 0
                                ? island.panelRef.audioPrefs.mutedFor(partRow.partMxid, "screenshare") : false
                        }
                        property bool soundMuted: {
                            void island.panelRef.volRev
                            return island.panelRef.audioPrefs && partRow.partMxid.length > 0
                                ? island.panelRef.audioPrefs.mutedFor(partRow.partMxid, "music") : false
                        }
                        // Row glyph shows if ANY of the three is muted.
                        readonly property bool partMuted: micMuted || streamMuted || soundMuted
                        Layout.fillWidth: true
                        implicitHeight: partContent.implicitHeight

                        // subtle hover fill — surfaces the right-click volume affordance
                        Rectangle {
                            anchors.fill: parent
                            radius: island.panelRef._a.radiusSm
                            color: partMa.containsMouse
                                ? Th.roomsCallerBgHover
                                : "transparent"
                            Behavior on color { ColorAnimation { duration: island.panelRef._a.dur } }
                        }

                        RowLayout {
                            id: partContent
                            anchors.fill: parent
                            spacing: Kirigami.Units.smallSpacing
                            Avatar {
                                app: island.panelRef._a
                                Layout.preferredWidth: 20; Layout.preferredHeight: 20
                                size: 20
                                name: partRow.modelData.name
                                seed: partRow.partMxid.length > 0 ? partRow.partMxid : partRow.modelData.name
                                imageSource: partRow.modelData.avatar || ""
                            }
                            Controls.Label {
                                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                Layout.fillWidth: true
                                text: partRow.modelData.name
                                elide: Text.ElideRight
                                color: Th.roomsCallerFg
                                font.pixelSize: Math.round(Kirigami.Theme.smallFont.pixelSize)
                            }
                            // unobtrusive muted glyph (only when this user is muted by us)
                            Kirigami.Icon {
                                visible: partRow.partMuted
                                source: "audio-volume-muted-symbolic"
                                Layout.preferredWidth: 14; Layout.preferredHeight: 14
                                color: Th.roomsCallerMutedIcon
                            }
                            // RBAC §7.4: a moderator muted their mic for everyone — mic-off + a tiny shield.
                            Item {
                                id: srvGlyph
                                readonly property string by: (island.panelRef.app && island.panelRef.app.serverMutedByFor) ? String(island.panelRef.app.serverMutedByFor("", partRow.partMxid)) : ""
                                visible: by.length > 0
                                Layout.preferredWidth: 14; Layout.preferredHeight: 14
                                Kirigami.Icon {
                                    anchors.fill: parent
                                    source: "microphone-sensitivity-muted-symbolic"
                                    color: Th.roomsCallerServerMutedIcon
                                }
                                ThIcon {
                                    app: island.panelRef.app
                                    name: "security-high-symbolic"
                                    width: 8; height: 8
                                    anchors.right: parent.right; anchors.bottom: parent.bottom
                                    anchors.rightMargin: -3; anchors.bottomMargin: -2
                                    color: Th.roomsCallerServerMutedIcon
                                }
                                HoverHandler { id: srvHover }
                                Controls.ToolTip.text: "Muted by " + srvGlyph.by
                                Controls.ToolTip.visible: srvHover.hovered
                                Controls.ToolTip.delay: 300
                            }
                            // Blue mic = in the game's voice chat (muted here if we are too).
                            Kirigami.Icon {
                                visible: !!island.panelRef.app && island.panelRef.app.isGameChatMxid(partRow.partMxid)
                                source: "audio-input-microphone-symbolic"
                                Layout.preferredWidth: 14; Layout.preferredHeight: 14
                                color: Th.roomsCallerGameChatIcon
                                Controls.ToolTip.text: (island.panelRef.app && island.panelRef.app.mGameChat)
                                    ? "In game chat with you — you hear each other in-game, so their mic is muted here"
                                    : "In the game's voice chat"
                                Controls.ToolTip.visible: gcHover.hovered
                                HoverHandler { id: gcHover }
                            }
                            // 9.4: yellow mic = pulled aside (in a corner). ThIcon tints on every platform.
                            ThIcon {
                                app: island.panelRef.app
                                visible: !!island.panelRef.app && island.panelRef.app.isAsideMxid(partRow.partMxid)
                                name: "audio-input-microphone-symbolic"
                                Layout.preferredWidth: 14; Layout.preferredHeight: 14
                                width: 14; height: 14
                                color: Th.roomsCallerAsideIcon
                                Controls.ToolTip.text: (island.panelRef.app && island.panelRef.app.mCorner.active && island.panelRef.app.cornerOfMxid(partRow.partMxid) === island.panelRef.app.mCorner.mine)
                                    ? "Aside with you"
                                    : "Pulled aside — you don't hear their mic. Right-click to join them"
                                Controls.ToolTip.visible: asideHover.hovered
                                HoverHandler { id: asideHover }
                            }
                        }

                        // Right-click / long-press a participant bubble -> per-user
                        // volume (0-200%) + mute. Drives AudioPrefsController (persists
                        // + pushes live to that user's "mic" track). Unobtrusive: only
                        // appears on demand; default (unity) leaves audio untouched.
                        // ONE shared menu (sharedVolMenu, panel scope) is parameterized
                        // here instead of instantiating a full Menu tree per participant.
                        MouseArea {
                            id: partMa
                            anchors.fill: parent
                            hoverEnabled: true
                            acceptedButtons: Qt.RightButton
                            cursorShape: Qt.PointingHandCursor
                            enabled: island.panelRef.audioPrefs && partRow.partMxid.length > 0
                            function _openVolMenu() {
                                island.panelRef.volMxid = partRow.partMxid
                                island.panelRef.volName = String(partRow.modelData.name || "")
                                // Anchor the popup to the PANEL, not this delegate: the
                                // participant Repeater is rebuilt on every roster refresh
                                // (constantly during a call) and a Menu whose parent item
                                // dies closes with it — "right-click does nothing".
                                var pt = partRow.mapToItem(island.panelRef, 0, partRow.height)
                                island.sharedVolMenuRef.popup(island.panelRef, pt.x, pt.y)
                            }
                            onClicked: _openVolMenu()
                            onPressAndHold: _openVolMenu()
                        }
                    }
                }
            }

            // per-person statuses ("looking to play…") — small accent text near the
            // participant bubbles (non-DM only, expiry-filtered upstream).
            ColumnLayout {
                id: statusCol
                anchors.top: row.parts.length > 0 ? partCol.bottom
                             : (row.editorShown ? editorRow.bottom
                             : (nowPlaying.visible ? nowPlaying.bottom : mainRow.bottom))
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.leftMargin: Kirigami.Units.gridUnit * 2
                anchors.rightMargin: Kirigami.Units.smallSpacing
                spacing: 1
                visible: row.statuses.length > 0
                Repeater {
                    model: row.statuses
                    delegate: Controls.Label {
                        textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                        required property var modelData
                        Layout.fillWidth: true
                        text: String(modelData.name || modelData.userId || "") + ": " + String(modelData.text || "")
                        elide: Text.ElideRight
                        wrapMode: Text.NoWrap
                        color: Th.roomsStatusFg
                        font.pixelSize: Math.round(Kirigami.Theme.smallFont.pixelSize)
                        font.italic: true
                    }
                }
            }
        }
    }

    // RBAC §7.3: an invited space has no rooms for you yet — the card stands in for the list.
    SpaceInviteCard {
        Layout.fillWidth: true
        Layout.fillHeight: true
        visible: island.panelRef.activeInvited
        app: island.panelRef._a
        spaceName: island.panelRef.activeSpaceEntry ? String(island.panelRef.activeSpaceEntry.label || "") : ""
        spaceId: island.panelRef.activeSpaceKey
        avatar: island.panelRef.activeSpaceEntry ? String(island.panelRef.activeSpaceEntry.avatar || "") : ""
        inviter: island.panelRef.activeInviter
        inviterMxid: island.panelRef.activeSpaceEntry ? String(island.panelRef.activeSpaceEntry.inviterMxid || "") : ""
        memberCount: island.panelRef.activeSpaceEntry ? Number(island.panelRef.activeSpaceEntry.memberCount || 0) : 0
        busy: !!island.panelRef.app && island.panelRef.app.mBusy === true
        onJoin: island.panelRef.acceptInvite(island.panelRef.activeSpaceKey)      // a space is a room: login.acceptInvite
        onDecline: island.panelRef.declineInvite(island.panelRef.activeSpaceKey)
    }

    // empty state
    Controls.Label {
        Layout.fillWidth: true
        Layout.fillHeight: true
        visible: !island.panelRef.activeInvited && island.panelRef._matchCount === 0
        horizontalAlignment: Text.AlignHCenter
        verticalAlignment: Text.AlignVCenter
        wrapMode: Text.Wrap
        color: Th.roomsEmptyFg
        text: island.panelRef.query.length > 0 ? "No matching rooms"
            : ((island.panelRef.app && island.panelRef.app.login && island.panelRef.app.login.syncDown) ? "Can't reach the server — your rooms will show up when it's back"
            : ((island.panelRef.app && island.panelRef.app.mBusy) ? "Catching up…" : "No rooms here yet"))   // 10.1 (TH-STATE-05)
    }
}
