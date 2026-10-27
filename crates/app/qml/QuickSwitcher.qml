// QuickSwitcher.qml — the Ctrl+K palette (ACTIONS.md §10; web features/nav/QuickSwitcher.tsx — the same rules): one
// field over rooms, pending invites, people and every action, ranked by the shared engine (ActionsController.palette →
// crates/actions, proven by the palette vectors). An empty query lists the pending invites (newest first), then the
// rooms as Ctrl+K always did (the room you are in last); `>` actions only, `@` people, `#` rooms and invites
// (2026-09-28: invites are back, as in 10.8's switcher; §10.1). An invite row = the room list's avatar and name + its
// "Invited" pill, never a badge. Enter runs the selection: a room opens (chosen(roomId)), an invite opens where it waits
// with Accept / Decline (inviteChosen(roomId) → Main.qml showInvite — never accepted or declined here), a person gets a
// DM, an action runs (app.actions.run) — or, when it needs a target (a theme, a status, a sound …), the palette switches
// to stage 2 and lists the choices (Backspace on an empty field goes back).
// ↑/↓, Enter, Esc; the pointer moving over a row selects it; a click runs it. Colours: the switcher.* and
// action.palette.* tokens (Th).
// 11.0 step 5 (THEMES-UI.md §14.5): pickRoom(title, placeholder, onPick) — the same switcher as a room PICKER ("Share
// ‹name› to…"): its own title over the field, its own placeholder, rooms only (the palette's `#` mode), the room open
// now listed first, and a pick handed to onPick(roomId) instead of opening the room (the web's QuickSwitcher pick mode);
// never an invite (you can't share into a room you have not joined).
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the switcher.* and action.palette.* tokens

Controls.Popup {
    id: qs
    property var app
    property var rooms: []          // [{id, name, avatar, unread, highlight, dm, peer, alias, space, membership, inviteRank, …}] (root.mRooms)
    signal chosen(string roomId)
    signal inviteChosen(string roomId)   // Ctrl+K on a pending invite: Main.qml shows it where it waits (ACTIONS.md §14.3)

    FallbackTheme { id: fallbackTheme }
    readonly property var _a: app || fallbackTheme   // sizes (radius…); colours come from Th

    modal: true
    focus: true
    closePolicy: Controls.Popup.CloseOnEscape | Controls.Popup.CloseOnPressOutside
    width: Kirigami.Units.gridUnit * 24
    height: Kirigami.Units.gridUnit * 21
    padding: 0
    anchors.centerIn: Controls.Overlay.overlay
    // Kirigami's dialog layer (z 300): Share to a room… opens it from the Settings sheet (a Kirigami OverlaySheet, z 200),
    // and a plain Popup (z 0) opened BEHIND the sheet — dimmed, invisible, but holding the keyboard
    Kirigami.OverlayZStacking.layer: Kirigami.OverlayZStacking.Dialog
    z: Kirigami.OverlayZStacking.z

    property string query: ""
    /// pick mode: {title, placeholder, onPick} (null = Ctrl+K)
    property var pick: null
    function pickRoom(title, placeholder, onPick) {
        qs.pick = { title: title, placeholder: placeholder, onPick: onPick }
        qs.stage = ""
        qs.stageFromOutside = false
        qs.open()
    }
    property string stage: ""       // the action whose target is being picked (stage 2), "" = everything
    property bool stageFromOutside: false   // opened at a stage by a button: Backspace does not go back
    readonly property var _texts: app && app.actionsTable ? app.actionsTable.palette.texts
                                  : ({ placeholder: "Search rooms, people and actions…", pickPlaceholder: "{label} — pick one…",
                                       empty: "Nothing matches \"{query}\"", emptyRooms: "No rooms", inviteMarker: "Invited" })

    /** Type into the field (tests, the review captures). */
    function setQuery(t) { qsField.text = t }
    /** Open at stage 2 of an action (a button without its target). */
    function openAt(actionId) {
        qs.stage = actionId
        qs.stageFromOutside = true
        open()
    }

    // The room (and invite) entries by id (the unread badge, the avatar).
    readonly property var _roomById: {
        var m = {}
        for (var i = 0; i < (rooms || []).length; i++) m[rooms[i].id] = rooms[i]
        return m
    }
    // The palette's snapshot: the live actions snapshot + the room list (nav order) + the pending invites (newest first)
    // + the people it can find (ACTIONS.md §4.3, §10.1; the web's paletteLists builds the same from its room rows).
    property string _snapJson: "{}"
    function _buildSnapshot() {
        var snap = {}
        try { snap = JSON.parse(app && app.actions ? app.actions.snapshotJson : "{}") } catch (e) { snap = {} }
        if (!snap.subjects) snap.subjects = {}
        if (!snap.lists) snap.lists = {}
        if (!snap.current) snap.current = {}
        var subjRoom = snap.subjects.room || {}, subjPerson = snap.subjects.person || {}
        var order = [], people = [], invites = []
        for (var i = 0; i < (rooms || []).length; i++) {
            var r = rooms[i]
            if (r.space === true) continue
            order.push(r.id)
            var e = subjRoom[r.id] || { values: {} }
            if (!e.values) e.values = {}
            e.name = r.name !== undefined ? String(r.name) : r.id
            if (r.alias) e.alias = String(r.alias)
            var joined = r.membership === undefined || r.membership === "join"
            var invite = r.membership === "invite"
            e.values["room.joined"] = joined
            e.values["room.invited"] = invite
            // an invite has nothing unread to show (the room list draws its pill, never a badge; the web's rows say 0)
            e.values["room.hasUnread"] = !invite && (r.unread || 0) > 0
            e.values["room.isDm"] = r.dm === true
            subjRoom[r.id] = e
            if (invite) invites.push({ id: String(r.id), rank: r.inviteRank !== undefined ? Number(r.inviteRank) : 1e9 })
            // your JOINED DM partners (§15, decision 3): a pending DM invite is its invite row, not a person too
            var peer = r.peer || r.dmUser || ""
            if (joined && r.dm === true && peer.length > 0 && subjPerson[peer] === undefined) {
                subjPerson[peer] = { name: e.name, values: {} }
                people.push(peer)
            }
        }
        // the pending invites newest first (§4.3 lists.invite): the invite's time, then the lower-cased name by code
        // point, then the id — ranked by nav::palette_invite_order in rooms_json (the web's paletteInviteOrder)
        invites.sort(function (a, b) { return a.rank - b.rank })
        snap.subjects.room = subjRoom
        snap.subjects.person = subjPerson
        snap.lists.room = order
        snap.lists.invite = invites.map(function (x) { return x.id })
        snap.lists.person = people
        if (app && app.room && app.room.roomId) snap.current.room = app.room.roomId
        _snapJson = JSON.stringify(snap)
    }
    // pick mode asks for rooms only (the `#` prefix)
    readonly property string _resultJson: (app && app.actionsCtl && qs.opened)
                                          ? app.actionsCtl.palette(_snapJson, qs.pick ? "#" + query : query, stage) : ""
    property var results: []
    on_ResultJsonChanged: {
        var r = []
        try { r = _resultJson.length > 0 ? (JSON.parse(_resultJson).items || []) : [] } catch (e) { r = [] }
        // pick mode: joined rooms only (`#` also lists invites — you can't share into a room you have not joined)
        if (qs.pick) r = r.filter(function (x) { return x.kind === "room" })
        if (qs.pick && query.trim().length === 0) {
            // pick mode: the room open now first (Share to a room… most often means this one)
            var cur = (app && app.room) ? app.room.roomId : ""
            var here = r.filter(function (x) { return x.id === cur }), rest = r.filter(function (x) { return x.id !== cur })
            r = here.concat(rest)
        }
        results = r
        list.currentIndex = 0
    }

    onAboutToShow: { query = ""; qsField.text = ""; _buildSnapshot(); list.currentIndex = 0 }
    onOpened: qsField.forceActiveFocus()
    onClosed: { stage = ""; stageFromOutside = false; pick = null }
    onRoomsChanged: if (opened) _buildSnapshot()
    Connections {
        target: qs.app && qs.app.actions ? qs.app.actions : null
        function onSnapshotJsonChanged() { if (qs.opened) qs._buildSnapshot() }
    }

    function _label(id) {
        var a = app && app.actionsCtl ? JSON.parse(app.actionsCtl.call("evaluate", JSON.stringify({ app: "desktop", action: id, snapshot: {} }))) : null
        return a && a.label ? a.label : id
    }
    readonly property string _stageLabel: stage.length > 0 ? _label(stage) : ""

    function choose(it) {
        if (!it || it.enabled !== true) return
        if (it.kind === "room") {
            var p = qs.pick
            qs.close()
            if (p) p.onPick(it.id)
            else qs.chosen(it.id)
            return
        }
        if (it.kind === "invite") {
            // where it waits with Accept / Decline (Main.qml showInvite, ACTIONS.md §14.3) — never accepted here
            qs.close()
            qs.inviteChosen(it.id)
            return
        }
        if (it.kind === "person") {
            qs.close()
            if (app && app.actions) app.actions.run("person.message", { source: "palette", target: { person: it.id } })
            return
        }
        if (it.kind === "action" && it.pick === true) {
            // stage 2: pick the target here
            qs.stage = it.id
            qs.stageFromOutside = false
            qs.query = ""; qsField.text = ""
            return
        }
        var a = it.a, t = it.kind === "choice" ? it.target : null
        qs.close()
        if (a && app && app.actions) app.actions.run(a, { source: "palette", target: t })
    }
    function accept() {
        if (results.length > 0) choose(results[Math.max(0, Math.min(list.currentIndex, results.length - 1))])
    }

    background: Kirigami.ShadowedRectangle {
        radius: qs._a.radius
        color: Th.switcherBg
        border.width: 1
        border.color: Th.switcherBorder
        shadow.size: 24
        shadow.color: Th.switcherShadow
        shadow.yOffset: 6
    }

    ColumnLayout {
        anchors.fill: parent
        anchors.margins: Kirigami.Units.smallSpacing
        spacing: Kirigami.Units.smallSpacing

        // pick mode: the title over the field
        Controls.Label {
            textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
            visible: qs.pick !== null
            Layout.fillWidth: true
            Layout.leftMargin: 8
            Layout.rightMargin: 8
            Layout.topMargin: 4
            text: qs.pick ? qs.pick.title : ""
            color: Th.switcherItemFg
            font.weight: Font.DemiBold
            elide: Text.ElideRight
        }
        RowLayout {
            Layout.fillWidth: true
            spacing: Kirigami.Units.smallSpacing
            // stage 2: the action being given a target; clicking it goes back to everything
            Rectangle {
                visible: qs.stage.length > 0
                Layout.preferredHeight: 28
                Layout.preferredWidth: stageRow.implicitWidth + 16
                radius: qs._a.radiusSm
                color: Th.actionPaletteStageBg
                Row {
                    id: stageRow
                    anchors.centerIn: parent
                    spacing: 4
                    Controls.Label { textFormat: Text.PlainText; anchors.verticalCenter: parent.verticalCenter; text: qs._stageLabel; color: Th.actionPaletteStageFg }
                    ThIcon {
                        anchors.verticalCenter: parent.verticalCenter
                        width: 12; height: 12
                        app: qs.app
                        name: "go-next-symbolic"
                        color: Th.actionPaletteStageFg
                        isMask: true
                    }
                }
                MouseArea {
                    anchors.fill: parent
                    cursorShape: Qt.PointingHandCursor
                    onClicked: { qs.stage = ""; qs.stageFromOutside = false; qs.query = ""; qsField.text = ""; qsField.forceActiveFocus() }
                }
            }
            Kirigami.SearchField {
                id: qsField
                Layout.fillWidth: true
                placeholderText: qs.pick ? qs.pick.placeholder
                                         : qs.stage.length > 0 ? String(qs._texts.pickPlaceholder).replace("{label}", qs._stageLabel)
                                                               : qs._texts.placeholder
                Accessible.name: qs.pick ? qs.pick.placeholder : "Search rooms, people and actions"
                // 9.2: themed from the app palette — a plain QQC2 style drew a white field over the
                // dark list (the rig and Windows).
                Kirigami.Theme.inherit: false
                Kirigami.Theme.backgroundColor: Th.switcherFieldBg
                Kirigami.Theme.textColor: Th.switcherFieldFg
                palette.base: Th.switcherFieldBg
                palette.text: Th.switcherFieldFg
                palette.placeholderText: Th.switcherFieldPlaceholder
                palette.highlight: Th.switcherFieldBorderFocus
                color: Th.switcherFieldFg
                placeholderTextColor: Th.switcherFieldPlaceholder
                onTextChanged: { qs.query = text; list.currentIndex = 0 }
                Keys.onDownPressed: list.currentIndex = Math.min(list.currentIndex + 1, qs.results.length - 1)
                Keys.onUpPressed: list.currentIndex = Math.max(list.currentIndex - 1, 0)
                Keys.onReturnPressed: qs.accept()
                Keys.onEnterPressed: qs.accept()
                Keys.onEscapePressed: qs.close()
                Keys.onPressed: (event) => {
                    if (event.key === Qt.Key_Backspace && text.length === 0 && qs.stage.length > 0 && !qs.stageFromOutside) {
                        qs.stage = ""
                        event.accepted = true
                    }
                }
            }
        }

        ListView {
            id: list
            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
            model: qs.results
            currentIndex: 0
            Controls.Label {
                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                anchors.centerIn: parent
                visible: qs.results.length === 0
                text: qs.query.trim().length > 0 ? String(qs._texts.empty).replace("{query}", qs.query.trim())
                      : (qs.stage.length > 0 ? String(qs._texts.empty).replace("{query}", "") : qs._texts.emptyRooms)
                color: Th.switcherEmptyFg
            }
            delegate: Item {
                id: row
                required property var modelData
                required property int index
                readonly property bool off: modelData.enabled !== true
                readonly property var roomEntry: (modelData.kind === "room" || modelData.kind === "invite") ? (qs._roomById[modelData.id] || null) : null
                width: list.width
                height: 44
                Rectangle {
                    anchors.fill: parent
                    anchors.margins: 2
                    radius: qs._a.radiusSm
                    color: index === list.currentIndex ? Th.switcherItemBgSelected : "transparent"
                    RowLayout {
                        anchors.fill: parent
                        anchors.leftMargin: 8
                        anchors.rightMargin: 8
                        spacing: Kirigami.Units.smallSpacing
                        Avatar {
                            // a room, an invite (the room list's picture and name), a person
                            visible: row.modelData.kind === "room" || row.modelData.kind === "invite" || row.modelData.kind === "person"
                            app: qs.app
                            size: 28
                            name: row.modelData.label !== undefined ? row.modelData.label : ""
                            // §6: a person is their id, a DM its person, a room its id (as the room list)
                            seed: row.modelData.kind === "person" ? Seeds.avatarSeed(row.modelData.id)
                                  : Seeds.avatarSeed(row.modelData.id, row.roomEntry && row.roomEntry.dm ? row.roomEntry.peer : "")
                            imageSource: row.roomEntry && row.roomEntry.avatar !== undefined ? row.roomEntry.avatar : ""
                        }
                        Item {
                            visible: row.modelData.kind === "action" || row.modelData.kind === "choice"
                            Layout.preferredWidth: 28
                            Layout.preferredHeight: 28
                            Controls.Label {
                                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                visible: !!row.modelData.emoji
                                anchors.centerIn: parent
                                text: row.modelData.emoji ? row.modelData.emoji : ""
                                font.pixelSize: 18
                            }
                            ThIcon {
                                visible: !!row.modelData.icon && !row.modelData.emoji
                                anchors.centerIn: parent
                                width: Kirigami.Units.iconSizes.smallMedium
                                height: Kirigami.Units.iconSizes.smallMedium
                                app: qs.app
                                name: row.modelData.icon ? row.modelData.icon : ""
                                color: row.off ? Th.actionPaletteItemFgDisabled : Th.actionPaletteIcon
                                isMask: true
                            }
                        }
                        Controls.Label {
                            textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                            Layout.fillWidth: true
                            text: row.modelData.label !== undefined ? row.modelData.label : ""
                            color: row.off ? Th.actionPaletteItemFgDisabled : Th.switcherItemFg
                            elide: Text.ElideRight
                        }
                        // an invite: the room list's "Invited" pill (RoomsIsland.qml; the word is the table's
                        // palette.texts.inviteMarker, ACTIONS.md §10.1) — and no unread badge
                        Rectangle {
                            visible: row.modelData.kind === "invite"
                            radius: height / 2
                            implicitHeight: invitePillText.implicitHeight + 4
                            implicitWidth: invitePillText.implicitWidth + 12
                            color: Th.roomsPillInviteBg
                            Controls.Label {
                                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                id: invitePillText
                                anchors.centerIn: parent
                                text: String(qs._texts.inviteMarker || "Invited")
                                color: Th.roomsPillInviteFg
                                font.pointSize: Kirigami.Theme.smallFont.pointSize
                            }
                        }
                        UnreadBadge {
                            // rooms only, AND the badge's own rule (count > 0): a `visible` given here replaces
                            // UnreadBadge's `visible: count > 0` — 11.0's first palette drew a "0" on every room row
                            visible: row.modelData.kind === "room" && count > 0
                            app: qs.app
                            count: row.roomEntry && row.roomEntry.unread !== undefined ? row.roomEntry.unread : 0
                            highlight: row.roomEntry !== null && row.roomEntry.highlight === true   // 10.1 (TH-NAV-10)
                        }
                        Controls.Label {
                            textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                            // the first key in effect on this device (Settings → Shortcuts may have changed it, ACTIONS.md §8.8)
                            readonly property string keyHint: row.modelData.kind === "action" && row.modelData.a && qs.app && qs.app.actions
                                                              ? (qs.app.actions.keysFor(row.modelData.a)[0] || "") : (row.modelData.hint || "")
                            visible: keyHint.length > 0
                            text: keyHint
                            color: Th.actionPaletteHintFg
                            font.pointSize: Kirigami.Theme.smallFont.pointSize
                        }
                        ThIcon {
                            visible: row.modelData.kind === "choice" && row.modelData.active === true
                            Layout.preferredWidth: 16
                            Layout.preferredHeight: 16
                            app: qs.app
                            name: "checkmark-symbolic"
                            color: Th.actionPaletteHintFg
                            isMask: true
                        }
                        ThIcon {
                            visible: row.modelData.pick === true
                            Layout.preferredWidth: 16
                            Layout.preferredHeight: 16
                            app: qs.app
                            name: "go-next-symbolic"
                            color: Th.actionPaletteHintFg
                            isMask: true
                        }
                    }
                    MouseArea {
                        anchors.fill: parent
                        hoverEnabled: true
                        cursorShape: row.off ? Qt.ArrowCursor : Qt.PointingHandCursor
                        // onPositionChanged (not onEntered) so keyboard-scrolling rows
                        // under a stationary cursor doesn't steal the highlight.
                        onPositionChanged: list.currentIndex = row.index
                        onClicked: { list.currentIndex = row.index; qs.choose(row.modelData) }
                    }
                }
            }
            Controls.ScrollBar.vertical: FoxScrollBar { app: qs.app }   // 10.1 (TH-DS-05): the house scrollbar everywhere
        }
    }
}
