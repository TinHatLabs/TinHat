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

    // 12.1.5 (2026-10-30): what a click and a double-click on a room row do — the web's src/features/nav/rowGesture.ts,
    // the same rule; tools/room-row-gesture-test.mjs runs this code (between the markers) on the shared vectors
    // (crates/app/testvectors/room-row-gesture/vectors.json = a byte copy of the web's).
    //   click     joined → "open" · listed, not joined → "askJoin" (the join dialog) · invite → "nothing" (Accept/Decline)
    //   dblclick  joined + armed (its first click landed on this row while it was joined) → "call" (the call.join action,
    //             the call button's path) · anything else → "nothing": the first click already did what a click does, and a
    //             double-click never joins a call in a room not opened/accepted (nor one that started on Accept)
    // BEGIN pure
    function rowGesture(i) {
        var joined = i.membership === "join"
        if (i.gesture === "click") return i.membership === "none" ? "askJoin" : (i.membership === "invite" ? "nothing" : "open")
        if (i.gesture === "dblclick") return (joined && !i.touch && i.armed === true) ? "call" : "nothing"
        return "nothing"
    }
    // END pure
    // The one armed row for the whole list (Astra 2026-10-30, second pass): set by a joined row's own left click, cleared
    // by any other row's click and by a press on a control in the list (the header buttons, the filter, Accept / Decline,
    // the now-playing pencil / editor / clear) — Qt pairs two presses by time and distance, not by item, so an arm kept
    // per row could be paired with a press that began somewhere else.
    property string armedRoom: ""
    property double armedAt: 0
    function disarm() { island.armedRoom = ""; island.armedAt = 0; island.clearPending() }
    // 12.1.5 (assembly review): Qt emits doubleClicked on the second PRESS. That press only marks what its release does:
    // "call" (the join) or "click" (a double-click that doesn't join is an ordinary click, as on the web: the row opens).
    // The release runs it over the same row; a cancel (the list took the press for a drag/flick) drops it.
    // Bound to the PRESS that set it (pressSeq: every row press counts): a release only runs what its own press marked,
    // so a pending action left over (the delegate's room replaced by a model refresh mid-press) never fires later.
    property string pendingRoom: ""
    property string pendingWhat: ""
    property int pendingSeq: -1
    property int pressSeq: 0
    function clearPending() { island.pendingRoom = ""; island.pendingWhat = ""; island.pendingSeq = -1 }

    // ── header: active space name ───────────────────────────────────────
    Item {
        Layout.fillWidth: true
        Layout.preferredHeight: 44
        HoverHandler { id: headerHover }
        ThHeading {
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
            onPressed: island.disarm()
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
            onPressed: island.disarm()
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
        onPressed: island.disarm()
        Layout.fillWidth: true
        Layout.margins: Kirigami.Units.smallSpacing
        visible: !island.panelRef.activeInvited   // nothing to filter until you join
        placeholderText: "Filter rooms…"
        // the web's useTip(filterWrap, "Ctrl+F"): the key, while the field is empty
        readonly property string keyHint: {
            var h = island.app && island.app.actions ? island.app.actions : null
            var ks = h ? h.keysFor("nav.roomList.filter") : []
            return ks.length > 0 ? String(ks[0]) : ""
        }
        Controls.ToolTip.text: keyHint
        Controls.ToolTip.visible: hovered && text.length === 0 && keyHint.length > 0
        Controls.ToolTip.delay: 700
        onTextChanged: { island.panelRef.query = text; roomView.positionViewAtBeginning() }   // 10.1 (TH-NAV-12): the match is never off-screen
        Keys.onEscapePressed: text = ""
        // P1 (2026-11-14): typing never opens a room (Kirigami's auto-accept opened — and marked read — every room typed
        // past, and Esc's empty field the first room). Enter acts on the highlighted first match; Down hands focus to
        // the list.
        autoAccept: false
        // Kirigami's SearchField binds StandardKey.Find (Ctrl+F) itself: the table's nav.roomList.filter key (Main.qml,
        // keysFor — it follows Settings → Shortcuts) is the only one; two Shortcuts on Ctrl+F made it ambiguous
        focusSequences: []
        onAccepted: island.acceptFilter()
        Keys.onDownPressed: roomView.kbEnter(true)   // a11y round 2 (M2): the first room that matches
    }

    // Enter in the filter: what a click on its first match does (rowGesture: open, or the join dialog for a listed room
    // you are not in; an invite or an empty filter: nothing) — the web's roomFilter.ts filterEnter
    function acceptFilter() {
        var id = island.panelRef.filterFirstId
        if (!id || !island.roomModelRef) return
        for (var i = 0; i < island.roomModelRef.count; i++) {
            if (island.roomModelRef.get(i).rid !== id) continue
            var r = island.roomModelRef.get(i).room
            var g = island.rowGesture({ gesture: "click", membership: String(r.membership || "") })
            if (g === "askJoin") island.panelRef.joinRequested(r.id, r.name)
            else if (g === "open") island.panelRef.roomActivated(r.id)
            return
        }
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
        // 12.1.5 (assembly review): another room on screen (keyboard Alt+Up/Down, Ctrl+K, a notification…) disarms;
        // the armed room's own click opening it does not
        function onCurrentRoomIdChanged() { if (island.panelRef.currentRoomId !== island.armedRoom) island.disarm(); island.clearPending() }
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
        // a11y round 2 (M2; the web's room list does the same, src/features/nav/RoomList.tsx): the list is ONE Tab stop
        // ("Rooms"); Tab lands on the room on screen (else the first that matches the filter), Down in the filter on the
        // first match; Up/Down walk the rooms the filter shows, Home/End the ends, Return opens (or asks to join, as a
        // click does), Escape goes back to the filter. Controls inside a row (Accept / Decline, Now playing) keep their
        // own Tab stops. Its own index (kbIndex), painted as the hover wash + the focus ring (rooms.item.ring.focus),
        // read out through an accessibility announcement; no current item (the list keeps the focus itself, as the
        // message list does).
        property int kbIndex: -1
        property string kbRid: ""        // the keyboard row's ROOM: the list re-sorts and shrinks under it
        activeFocusOnTab: true
        keyNavigationEnabled: false
        currentIndex: -1
        Accessible.role: Accessible.List
        Accessible.name: "Rooms"
        function kbMatches(i) {   // the shared filter (ThRoomList roomShown, ACTIONS.md §10.8)
            return island.panelRef.roomShown(island.roomModelRef.get(i).room)
        }
        function kbStep(from, dir) {   // the next row the filter shows from `from` (exclusive) in `dir`, -1 = none
            for (var i = from + dir; i >= 0 && i < count; i += dir) if (kbMatches(i)) return i
            return -1
        }
        function kbResolve() {   // the keyboard row's index from its room (-1 when the room left the list)
            if (kbRid.length === 0) { kbIndex = -1; return }
            for (var i = 0; i < count; i++) if (island.roomModelRef.get(i).rid === kbRid) { kbIndex = i; return }
            kbIndex = -1; kbRid = ""
        }
        onCountChanged: if (kbRid.length > 0) kbResolve()
        Connections {
            target: island.panelRef
            function on_ModelRevChanged() { if (roomView.kbRid.length > 0) roomView.kbResolve() }
        }
        function kbGo(i) {
            if (i < 0 || i >= count) return
            kbIndex = i
            kbRid = String(island.roomModelRef.get(i).rid)
            positionViewAtIndex(i, ListView.Contain)
            var r = island.roomModelRef.get(i).room
            // its name and place ("Game night, 2 of 5" — what a screen reader says for a web list item)
            var at = 0, of = 0
            for (var j = 0; j < count; j++) if (kbMatches(j)) { of++; if (j <= i) at++ }
            if (r && typeof Accessible.announce === "function") Accessible.announce(String(r.name || "") + ", " + at + " of " + of)
        }
        // fromFilter: the first match; else the room on screen when the filter shows it, else the first match
        function kbEnter(fromFilter) {
            var at = -1
            if (!fromFilter) for (var i = 0; i < count; i++) if (island.roomModelRef.get(i).rid === island.panelRef.currentRoomId && kbMatches(i)) { at = i; break }
            if (at < 0) at = kbStep(-1, 1)
            if (!activeFocus) { _kbEntering = true; forceActiveFocus(Qt.OtherFocusReason); _kbEntering = false }
            if (at >= 0) kbGo(at)
        }
        property bool _kbEntering: false
        onActiveFocusChanged: { if (!activeFocus) { kbIndex = -1; kbRid = "" } else if (!_kbEntering && kbIndex < 0) Qt.callLater(function () { if (roomView.activeFocus && roomView.kbIndex < 0) roomView.kbEnter(false) }) }
        function _kbKey(event) {
            if (event.modifiers & (Qt.ControlModifier | Qt.AltModifier | Qt.MetaModifier)) return false
            var k = event.key
            return k === Qt.Key_Up || k === Qt.Key_Down || k === Qt.Key_Home || k === Qt.Key_End || k === Qt.Key_Escape
                || k === Qt.Key_Return || k === Qt.Key_Enter
        }
        Keys.onShortcutOverride: (event) => { if (_kbKey(event)) event.accepted = true }
        Keys.onPressed: (event) => {
            if (!_kbKey(event)) return
            kbResolve()
            var k = event.key
            if (k === Qt.Key_Up) { var u = kbStep(kbIndex < 0 ? count : kbIndex, -1); if (u >= 0) kbGo(u) }
            else if (k === Qt.Key_Down) { var d = kbStep(kbIndex, 1); if (d >= 0) kbGo(d) }
            else if (k === Qt.Key_Home) kbGo(kbStep(-1, 1))
            else if (k === Qt.Key_End) kbGo(kbStep(count, -1))
            else if (k === Qt.Key_Escape) { if (filterField.visible) filterField.forceActiveFocus(Qt.ShortcutFocusReason) }
            else if (kbIndex >= 0 && kbIndex < count) {   // Return / Enter: what a click does (on the room it names)
                var r = island.roomModelRef.get(kbIndex).room
                var g = island.rowGesture({ gesture: "click", membership: String(r.membership || "") })
                if (g === "askJoin") island.panelRef.joinRequested(r.id, r.name)
                else if (g === "open") island.panelRef.roomActivated(r.id)
            }
            event.accepted = true
        }
        HoverHandler {
            id: listHover
            onHoveredChanged: if (!hovered && island.panelRef._pendingResort) { island.panelRef._pendingResort = false; island.panelRef._syncRooms() }
        }
        boundsBehavior: Flickable.StopAtBounds
        Controls.ScrollBar.vertical: FoxScrollBar { app: island.panelRef._a }   // 10.1 (TH-DS-05): the house scrollbar everywhere

        delegate: Item {
            id: row
            required property var room
            required property int index
            readonly property bool kbCurrent: roomView.activeFocus && roomView.kbIndex === row.index   // a11y round 2 (M2)
            width: ListView.view ? ListView.view.width : 0
            // Filter-as-visibility: typing in the filter only toggles row heights,
            // it never rebuilds delegates.
            readonly property bool matches: island.panelRef.roomShown(room)   // the shared filter (ACTIONS.md §10.8)
            // P1: the filter's first match (the one its Enter acts on) wears the hover wash
            readonly property bool filterFirst: island.panelRef.filterFirstId.length > 0 && room.id === island.panelRef.filterFirstId
            visible: matches
            readonly property bool isInvite: room.membership === "invite"
            // Listed but not joined: a public/space-restricted room discovered in the
            // space. Never a private one — the hierarchy call filters those out.
            readonly property bool notJoined: room.membership === "none"
            readonly property bool unjoined: isInvite || notJoined
            // the row's membership for rowGesture (nav.rs always sets it; a row without one never joins a call)
            readonly property string membershipKey: String(room.membership || "")
            // 12.1.5: a touch tap arrives as a synthesized mouse event — it never arms or joins
            function fromTouch(m) { return m.source !== undefined && m.source !== Qt.MouseEventNotSynthesized }
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
                : ((rowMa.containsMouse || row.kbCurrent || row.filterFirst) ? Th.roomsItemBgHover : "transparent")
            Rectangle {
                anchors.fill: parent
                anchors.leftMargin: Kirigami.Units.smallSpacing
                anchors.rightMargin: Kirigami.Units.smallSpacing
                anchors.topMargin: 1; anchors.bottomMargin: 1
                radius: island.panelRef._a.radius
                color: row.washColor
                Behavior on color { ColorAnimation { duration: island.panelRef._a.dur } }
                // a11y round 2 (M2): the keyboard row's focus ring (the web's .row:focus-visible inset 1 px ring)
                border.width: row.kbCurrent ? 1 : 0
                border.color: Th.roomsItemRingFocus
            }

            MouseArea {
                id: rowMa
                anchors.fill: parent  // whole delegate incl. the call-participant strip
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                acceptedButtons: Qt.LeftButton | Qt.RightButton
                // a left click on the row; mayArm = false for the second click of a pair (it never arms, as on the web)
                function rowClick(m, mayArm) {
                    // this row's own left click arms a double-click only while it is a joined room (never a touch tap)
                    if (mayArm && row.membershipKey === "join" && !row.fromTouch(m)) { island.armedRoom = row.room.id; island.armedAt = Date.now() }
                    else island.disarm()
                    var g = island.rowGesture({ gesture: "click", membership: row.membershipKey })
                    if (g === "askJoin") island.panelRef.joinRequested(row.room.id, row.room.name)
                    else if (g === "open") island.panelRef.roomActivated(row.room.id)
                }
                onPressed: island.pressSeq++
                onClicked: (m) => {
                    if (m.button === Qt.RightButton) { island.disarm(); island.panelRef.roomContext(row.room.id); return }
                    rowMa.rowClick(m, true)
                }
                // 12.1.5: the second press inside the platform's double-click interval (QStyleHints) — the first click
                // has already opened the room (never delayed); two slow clicks stay two clicks. Joined rooms only.
                // Armed = this row's own first click, the last press in the list, within the interval (a double-click that
                // began on Accept or anywhere else arrives here disarmed: Qt pairs the presses by time, not by item).
                // The second press only marks what its release does (island.pendingRoom): Qt suppresses the pair's second
                // clicked() once doubleClicked is handled, so the release runs the ordinary click itself when it doesn't join.
                onDoubleClicked: (m) => {
                    var age = Date.now() - island.armedAt
                    var armed = island.armedRoom === row.room.id && island.armedAt > 0 && age >= 0 && age <= Qt.styleHints.mouseDoubleClickInterval
                    island.disarm()
                    island.clearPending()
                    if (m.button !== Qt.LeftButton) return
                    island.pendingWhat = island.rowGesture({ gesture: "dblclick", membership: row.membershipKey, touch: row.fromTouch(m), armed: armed }) === "call" ? "call" : "click"
                    island.pendingRoom = row.room.id
                    island.pendingSeq = island.pressSeq   // this press (onPressed ran first)
                }
                onReleased: (m) => {
                    if (island.pendingRoom === "" || island.pendingRoom !== row.room.id || island.pendingSeq !== island.pressSeq) { island.clearPending(); return }
                    var what = island.pendingWhat
                    island.clearPending()
                    if (m.button !== Qt.LeftButton || !rowMa.containsMouse) return   // released elsewhere: nothing, like a click
                    if (what === "call") island.panelRef.roomCallRequested(row.room.id)
                    else rowMa.rowClick(m, false)
                }
                onCanceled: if (island.pendingRoom === row.room.id) island.clearPending()
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
                height: Th.row(40)   // P22: 40 at 100 % / Cozy; grows with the person's text size, Compact −8, Comfortable +8
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
                ColumnLayout {
                    Layout.fillWidth: true
                    Layout.minimumWidth: 72   // 10.1 (TH-NAV-09): the invite actions can no longer crush the name to nothing
                    spacing: 0
                RowLayout {   // mini games v1: a DM row is its person: their prestige token before the name
                    Layout.fillWidth: true
                    spacing: 0
                PrestigeToken {
                    mxid: row.isDM && !row.isInvite ? row.peer : ""
                    roomId: String(row.room.id || "")
                    opacity: row.room.muted === true ? 0.55 : 1
                }
                Controls.Label {
                    textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                    Layout.fillWidth: true
                    text: row.room.name
                    elide: Text.ElideRight
                    // Dimmed while you are not in it, so "haven't joined" reads at a
                    // glance rather than needing the badge to be noticed.
                    color: row.notJoined ? Th.roomsItemNotJoinedFg : Th.roomsItemFg
                    // …and while it is muted (TH-NAV-06): you did that, the list should say so.
                    opacity: row.room.muted === true ? 0.55 : 1
                    font.bold: (row.room.unread || 0) > 0 || row.isInvite
                }
                }
                // D09.4 (Sunday QoL): a DM invite shows who is really asking — the inviter's full address, server
                // included, in grey under the name (a display name alone can copy anyone's; the web's RoomList.tsx)
                Controls.Label {
                    textFormat: Text.PlainText
                    Layout.fillWidth: true
                    visible: row.isInvite && row.room.dm === true && String(row.room.peer || "").length > 0
                    text: String(row.room.peer || "")
                    elide: Text.ElideMiddle
                    color: Th.roomsItemNotJoinedFg
                    font.pointSize: Kirigami.Theme.smallFont.pointSize * Th.textScale
                }
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
                        font.pointSize: Kirigami.Theme.smallFont.pointSize * Th.textScale
                    }
                }
                // invite actions
                // 10.1 (TH-NAV-09): one visual language — house buttons, compact.
                ThButton {
                    visible: row.isInvite
                    app: island.panelRef._a
                    text: "Accept"
                    implicitHeight: 26
                    onPressed: island.disarm()
                    onClicked: island.panelRef.acceptInvite(row.room.id)
                }
                ThToolButton {
                    visible: row.isInvite
                    app: island.panelRef._a
                    iconName: "dialog-close-symbolic"
                    tip: "Decline invite"
                    onPressed: island.disarm()
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
                font.pointSize: ((Kirigami.Theme.smallFont.pointSize > 0 ? Kirigami.Theme.smallFont.pointSize : Math.max(7, Kirigami.Theme.defaultFont.pointSize * 0.85))) * Th.textScale
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
                    font.pixelSize: Math.round(Kirigami.Theme.smallFont.pixelSize * Th.textScale)
                    font.italic: !row.showActivity
                }
                // Pencil — open the inline editor (open non-DM row only).
                Controls.ToolButton {
                    visible: row.isOpenRow
                    icon.name: "document-edit-symbolic"
                    Layout.preferredHeight: 22
                    Accessible.name: "Edit now playing"   // a11y pass: icon-only (the tooltip is its name)
                    Controls.ToolTip.text: "Edit now playing"
                    Controls.ToolTip.visible: hovered
                    Controls.ToolTip.delay: 400
                    onPressed: island.disarm()
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
                    onPressed: island.disarm()
                    font.pixelSize: Math.round(Kirigami.Theme.smallFont.pixelSize * Th.textScale)
                    onAccepted: { island.panelRef.setActivity(text.trim()); island.panelRef.editingRoomId = ""; focus = false }
                }
                Controls.ToolButton {
                    icon.name: "edit-clear-symbolic"
                    Accessible.name: "Clear now playing"   // a11y pass: icon-only (the tooltip is its name)
                    Controls.ToolTip.text: "Clear now playing"
                    Controls.ToolTip.visible: hovered
                    Controls.ToolTip.delay: 400
                    onPressed: island.disarm()
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
                            PrestigeToken {   // mini games v1: 4 px before the name
                                mxid: partRow.partMxid
                                roomId: String(row.room.id || "")
                                gap: false
                                Layout.rightMargin: 4 - partContent.spacing
                            }
                            Controls.Label {
                                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                Layout.fillWidth: true
                                text: partRow.modelData.name
                                elide: Text.ElideRight
                                color: Th.roomsCallerFg
                                font.pixelSize: Math.round(Kirigami.Theme.smallFont.pixelSize * Th.textScale)
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
                    delegate: RowLayout {   // mini games v1: the person's prestige token before "Sam: playing …"
                        id: stLine
                        required property var modelData
                        Layout.fillWidth: true
                        spacing: 0
                        PrestigeToken { mxid: String(stLine.modelData.userId || ""); roomId: String(row.room.id || "") }
                        Controls.Label {
                        textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                        Layout.fillWidth: true
                        text: String(stLine.modelData.name || stLine.modelData.userId || "") + ": " + String(stLine.modelData.text || "")
                        elide: Text.ElideRight
                        wrapMode: Text.NoWrap
                        color: Th.roomsStatusFg
                        font.pixelSize: Math.round(Kirigami.Theme.smallFont.pixelSize * Th.textScale)
                        font.italic: true
                        }
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
    ColumnLayout {
        Layout.fillWidth: true
        Layout.fillHeight: true
        visible: !island.panelRef.activeInvited && island.panelRef._matchCount === 0
        spacing: 10
        Item { Layout.fillHeight: true }
        Controls.Label {
            Layout.fillWidth: true
            horizontalAlignment: Text.AlignHCenter
            wrapMode: Text.Wrap
            color: Th.roomsEmptyFg
            text: island.panelRef.query.length > 0 ? "No matching rooms"
                : ((island.panelRef.app && island.panelRef.app.login && island.panelRef.app.login.syncDown) ? "Can't reach the server — your rooms will show up when it's back"
                : ((island.panelRef.app && island.panelRef.app.mBusy) ? "Catching up…" : "No rooms here yet"))   // 10.1 (TH-STATE-05)
        }
        // A10.3: no room matches — the same text in Ctrl+K, which also finds people and actions (the web's empty state)
        ThButton {
            Layout.alignment: Qt.AlignHCenter
            visible: island.panelRef.query.trim().length > 0
            app: island.panelRef._a
            icon.name: "edit-find"
            text: "Search everywhere"
            onClicked: { island.disarm(); island.app.searchEverywhere(island.panelRef.query) }
        }
        Item { Layout.fillHeight: true }
    }
}
