// ThRoomList.qml — the Discord-style ROOM panel that sits next to the spaces rail.
// Header (active space name) → filter → the active space's rooms (each a small avatar
// bubble + name + unread/call badge), with the connected users shown as avatar bubbles
// UNDER any room that has a live call (like Discord's voice-channel members). Pending
// invites sort to the top with Accept/Decline. A Discord-style account+gear footer pins
// to the bottom.
//
// Data via app.mNavRooms / app.mNavActiveLabel (the CXX-Qt mirror). Acting on a room is
// delegated up via signals so this component stays presentational.
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the rooms.* / account.* / status.* tokens

Item {
    id: panel
    required property var app
    property var navController
    property var audioPrefs                   // AudioPrefsController — per-participant call volume/mute (#5)
    property string currentRoomId: ""        // highlight the open room
    property string openRoomId: ""           // the currently-open room (gets the inline editor)
    // Which room's "now playing" editor is open. Hoisted to the panel so a model refresh
    // (the 120ms mirror reassigns mNavRooms on any rooms-JSON change, rebuilding delegates)
    // can't reset a per-delegate flag and close the editor mid-type.
    property string editingRoomId: ""
    property bool sidebarCollapsed: false     // mirrors app.sidebarCollapsed for the footer toggle
    readonly property var _a: app

    signal roomActivated(string roomId)
    signal roomContext(string roomId)
    signal acceptInvite(string roomId)
    signal declineInvite(string roomId)
    signal menuRequested(var anchorItem)      // open the app menu (settings/theme/sign-out/…)
    signal newMenuRequested(var anchorItem)   // the room column's + : New DM / New room (2026-09-29)
    signal setActivity(string game)           // "" clears — set the open room's "now playing"
    signal toggleNavRequested()               // hide/show the room-list sidebar
    signal bugReportRequested()               // open the in-app bug-report dialog
    signal messageUser(string mxid)           // start (or re-open) a DM with someone
    signal joinRequested(string roomId, string roomName)  // tapped a room you're not in
    signal spaceSettingsRequested(string key)             // RBAC §7.1: the header gear → Space settings
    signal privacySettingsRequested()                     // 10.6 (REACH §2.7): the status popover's "Privacy settings…"
    signal profileSettingsRequested()                     // 2026-09-29: the status popover's "Profile settings…"

    property string query: ""

    // ---- RBAC §7.3: the active space entry — an INVITED space replaces the room list with a card.
    readonly property string activeSpaceKey: panel.app ? String(panel.app.mNavActiveSpace || "") : ""
    readonly property var activeSpaceEntry: {
        var sps = (panel.app && panel.app.mNavSpaces) ? panel.app.mNavSpaces : []
        for (var i = 0; i < sps.length; i++) if (sps[i] && sps[i].key === panel.activeSpaceKey) return sps[i]
        return null
    }
    readonly property bool activeInvited: !!panel.activeSpaceEntry && panel.activeSpaceEntry.membership === "invite"
    readonly property bool activeIsRealSpace: panel.activeSpaceKey.indexOf("!") === 0
    // "<name> invited you" — the display name, else the localpart of the inviter's id ("" = unknown).
    readonly property string activeInviter: {
        var e = panel.activeSpaceEntry
        if (!e) return ""
        if (e.inviter && String(e.inviter).length > 0) return String(e.inviter)
        var m = /^@([^:]+):/.exec(String(e.inviterMxid || ""))
        return m ? m[1] : ""
    }

    // Shared volume menu parameters — which participant sharedVolMenu acts on, set right
    // before popup() (same pattern as RightPanel's memberMenu/actorMxid). volRev is bumped
    // by the menu after every mute commit so the per-row muted glyphs (bindings on it)
    // re-query mutedFor() immediately without a delegate rebuild.
    property string volMxid: ""
    property string myMxid: ""
    property string volName: ""
    property int volRev: 0
    property bool _pendingResort: false   // 10.1 (TH-NAV-05)
    // 10.6.0 review: a tooltip was auto-detected rich text — a status or a name with markup in it would render (and load
    // an <img>). Security review 2026-10-26: every tooltip is PlainText (AppCore._plainTooltips), so the text goes in as it
    // is — but only once that worked (LinkGate.tooltipsPlain); until then, or if it couldn't, it is escaped (review #7).
    function _tipSafe(s) {
        return LinkGate.tipText(s)
    }
    function openFirstMatch() {           // 10.1 (TH-NAV-12)
        var q = panel.query.toLowerCase()
        for (var i = 0; i < roomModel.count; i++) {
            var r = roomModel.get(i).room
            if (!r || r.membership === "invite") continue
            if (q.length === 0 || String(r.name || "").toLowerCase().indexOf(q) !== -1) { panel.roomActivated(r.id); return }
        }
    }

    // Stable-identity room model, diffed from app.mNavRooms (keyed by room id) instead of
    // binding the ListView to a computed JS array — reassigning a JS array tears down and
    // rebuilds EVERY delegate on any rooms-JSON change. Each room lives under ONE var role
    // (`room`) so participants/statuses/activity stay plain JS arrays/objects rather than
    // nested ListModels; `raw` is the row's JSON string for cheap change detection.
    ListModel { id: roomModel }
    property int _modelRev: 0                 // bumped by _syncRooms so _matchCount recounts

    // Rooms matching the filter — drives the empty state. The filter itself is a
    // per-delegate visibility toggle, so typing never rebuilds delegates.
    readonly property int _matchCount: {
        void _modelRev
        var q = query.trim().toLowerCase()
        var n = 0
        for (var i = 0; i < roomModel.count; i++) {
            var r = roomModel.get(i).room
            if (q.length === 0 || String(r.name).toLowerCase().indexOf(q) >= 0) n++
        }
        return n
    }

    function _syncRooms() {
        var rs = (app && app.mNavRooms) ? app.mNavRooms : []
        // Remove rows whose id vanished.
        for (var i = roomModel.count - 1; i >= 0; i--) {
            var rid = roomModel.get(i).rid
            var found = false
            for (var j = 0; j < rs.length; j++) {
                if (rs[j].id === rid) { found = true; break }
            }
            if (!found) roomModel.remove(i)
        }
        // Walk the source in order: insert new rows at k, move() survivors to k (the list
        // is sorted upstream — invites pinned to top, activity re-ranks — so source order
        // must be preserved), and set() only rows whose JSON actually changed.
        for (var k = 0; k < rs.length; k++) {
            var r = rs[k]
            var raw = JSON.stringify(r)
            var at = -1
            for (var m = k; m < roomModel.count; m++) {
                if (roomModel.get(m).rid === r.id) { at = m; break }
            }
            if (at === -1) {
                roomModel.insert(k, { rid: r.id, room: r, raw: raw })
            } else {
                // 10.1 (TH-NAV-05): rows never move under the pointer — the row you are about to
                // click stays put; the new order lands once the mouse leaves the list.
                if (at !== k) { if (panel.roomsIslandItem && panel.roomsIslandItem.listHover.hovered) panel._pendingResort = true; else roomModel.move(at, k, 1) }
                if (roomModel.get(k).raw !== raw) roomModel.set(k, { rid: r.id, room: r, raw: raw })
            }
        }
        _modelRev++
    }

    Connections {
        target: panel.app
        function onMNavRoomsChanged() { panel._syncRooms() }
    }
    Component.onCompleted: _syncRooms()

    // 11.0 (islands C6/C7): the room list draws nothing any more — the window's dock host places the rooms island and the
    // account island (with the one-time question card) and hands them this panel (panelRef) with its model and menus.
    // The host's instances come back here for what the panel itself needs: the rooms island (a hovered list defers a
    // re-sort) and the account card (the status popover opens above it).
    property Item roomsIslandItem: null
    property Item footerCardItem: null
    readonly property alias roomModelObj: roomModel
    readonly property alias sharedVolMenuObj: sharedVolMenu
    readonly property alias statusPopObj: statusPop
    readonly property int footerH: Math.max(46, Math.round(Kirigami.Units.gridUnit * 2.5))
    readonly property int footerBottom: 9

    // ── 10.6 (REACH §2.7 + A2.1 + A4.6): "Your status" — the footer popover. The same switch as
    // Settings → Privacy, one line saying what it means, the status picker, "Privacy settings…".
    property bool _customOpen: false
    property bool _holdStatusPop: false
    property double _statusPopClosedAt: 0
    property string _customEmoji: ""
    property string _customColor: "blurple"
    property int _customClear: 2          // index into _clearChoices (4 hours)
    readonly property var _clearChoices: ["30 minutes", "1 hour", "4 hours", "Tonight (until 4 AM)", "Don't clear"]
    // A4.6 "Clear after" → an absolute `until` in ms (0 = don't clear). "Tonight" = the next 04:00 local.
    function _untilFor(choice) {
        var now = Date.now()
        if (choice === 0) return now + 30 * 60000
        if (choice === 1) return now + 60 * 60000
        if (choice === 2) return now + 4 * 3600000
        if (choice === 3) {
            var d = new Date(now)
            var at = new Date(d.getFullYear(), d.getMonth(), d.getDate(), 4, 0, 0, 0)
            if (at.getTime() <= now) at.setDate(at.getDate() + 1)
            return at.getTime()
        }
        return 0
    }
    // A pick while sharing is off asks first (A4.6): "This turns on sharing."
    property var _pendingStatus: null
    function _pickStatus(s) {
        if (!panel.app || !panel.app.presenceCtl) return
        if (panel.app.mPresenceSharing) {
            panel.app.presenceCtl.setStatus(s.id, s.text, s.emoji, s.color, s.until, false)
            panel._customOpen = false
        } else {
            panel._pendingStatus = s
            panel._holdStatusPop = true
            shareForStatus.open()
        }
    }
    function _presetById(id) {
        var ps = (panel.app && panel.app.mStatusPresets) ? panel.app.mStatusPresets : []
        for (var i = 0; i < ps.length; i++) if (ps[i].id === id) return ps[i]
        return null
    }
    Controls.Popup {
        id: statusPop
        parent: panel.footerCardItem ? panel.footerCardItem : panel
        x: 0
        y: -height - 8
        width: 336
        padding: 14
        modal: false
        focus: true
        // Held open while its own emoji picker / "turns on sharing" question is up (a press in
        // those is outside this popup).
        closePolicy: panel._holdStatusPop ? Controls.Popup.CloseOnEscape
                                          : (Controls.Popup.CloseOnEscape | Controls.Popup.CloseOnPressOutside)
        onClosed: { panel._customOpen = false; panel._statusPopClosedAt = Date.now() }
        background: Kirigami.ShadowedRectangle {
            radius: 12
            color: Th.statusPopoverBg
            border.width: 1
            border.color: Th.statusPopoverBorder
            shadow.size: 18; shadow.color: Th.statusPopoverShadow; shadow.yOffset: 4
        }
        contentItem: ColumnLayout {
            spacing: 8
            Kirigami.Heading {
                level: 4
                text: "Your status"
                color: Th.statusPopoverTitleFg
            }
            ThSwitch {
                id: popSwitch
                text: (panel.app && panel.app.mPresenceTurningOff) ? "Turning off…" : "Share my status"
                checked: !!panel.app && panel.app.mPresenceSharing === true
                enabled: !!panel.app && panel.app.mPresenceKnown === true && !panel.app.mPresenceBusy && !panel.app.mPresenceTurningOff
                onToggled: {
                    if (panel.app && panel.app.presenceCtl) panel.app.presenceCtl.chooseSharing(checked)
                    checked = Qt.binding(function () { return !!panel.app && panel.app.mPresenceSharing === true })
                }
            }
            Controls.Label {
                Layout.fillWidth: true
                wrapMode: Text.Wrap
                color: Th.statusPopoverHintFg
                font.pointSize: (Kirigami.Theme.smallFont.pointSize > 0 ? Kirigami.Theme.smallFont.pointSize : Math.max(7, Kirigami.Theme.defaultFont.pointSize * 0.85))
                // Never "Off" while the DELETE is still pending (A2.2): the switch and this line say so.
                text: (panel.app && panel.app.mPresenceTurningOff) ? "Turning off…"
                    : ((panel.app && panel.app.mPresenceSharing) ? "On — people you share a room with see Online, Away or In a call."
                                                                 : "Off — nobody sees your status.")
            }
            Kirigami.Separator { Layout.fillWidth: true; Layout.topMargin: 2 }
            SectionLabel { app: panel._a; text: "Set a status" }
            Flow {
                Layout.fillWidth: true
                spacing: 6
                Repeater {
                    model: (panel.app && panel.app.mStatusPresets) ? panel.app.mStatusPresets : []
                    delegate: Rectangle {
                        id: presetChip
                        required property var modelData
                        readonly property bool current: !!panel.app && !!panel.app.mMyPresenceStatus && panel.app.mMyPresenceStatus.id === modelData.id
                        readonly property color hex: modelData.hex
                        height: 28
                        width: presetRow.implicitWidth + 18
                        radius: 14
                        color: current ? Qt.rgba(hex.r, hex.g, hex.b, 0.22)   // theme-lint: ok content: the status colour's wash
                                       : (presetMa.containsMouse ? Qt.rgba(hex.r, hex.g, hex.b, 0.14) : Th.statusPresetBg)   // theme-lint: ok content: the status colour's wash
                        border.width: current ? 1.5 : 1
                        border.color: current ? hex : Th.statusPresetBorder
                        Behavior on color { ColorAnimation { duration: panel._a.dur } }
                        Row {
                            id: presetRow
                            anchors.centerIn: parent
                            spacing: 6
                            Text { textFormat: Text.PlainText; text: presetChip.modelData.emoji; font.pixelSize: 13; anchors.verticalCenter: parent.verticalCenter }
                            Text {
                                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                text: presetChip.modelData.text
                                color: presetChip.current ? presetChip.hex : Th.statusPresetFg
                                font.pointSize: (Kirigami.Theme.smallFont.pointSize > 0 ? Kirigami.Theme.smallFont.pointSize : Math.max(7, Kirigami.Theme.defaultFont.pointSize * 0.85))
                                anchors.verticalCenter: parent.verticalCenter
                            }
                            Rectangle { width: 8; height: 8; radius: 4; color: presetChip.hex; anchors.verticalCenter: parent.verticalCenter }
                        }
                        MouseArea {
                            id: presetMa
                            anchors.fill: parent
                            hoverEnabled: true
                            cursorShape: Qt.PointingHandCursor
                            onClicked: panel._pickStatus({ id: presetChip.modelData.id, text: presetChip.modelData.text,
                                                           emoji: presetChip.modelData.emoji, color: presetChip.modelData.color,
                                                           until: panel._untilFor(2) })   // presets clear after 4 hours (A4.6)
                        }
                        Accessible.role: Accessible.Button
                        Accessible.name: presetChip.modelData.text
                    }
                }
                Rectangle {
                    id: customChip
                    readonly property bool current: !!panel.app && !!panel.app.mMyPresenceStatus && panel.app.mMyPresenceStatus.id === "custom"
                    height: 28
                    width: customLabel.implicitWidth + 22
                    radius: 14
                    color: (panel._customOpen || customChip.current) ? Th.statusCustomBgActive
                         : (customMa.containsMouse ? Th.statusCustomBgHover : Th.statusPresetBg)
                    border.width: 1
                    border.color: (panel._customOpen || customChip.current) ? Th.statusCustomBorderActive : Th.statusPresetBorder
                    Text {
                        id: customLabel
                        anchors.centerIn: parent
                        text: customChip.current ? ((panel.app.mMyPresenceStatus.emoji ? panel.app.mMyPresenceStatus.emoji + " " : "") + panel.app.mMyPresenceStatus.text) : "Custom…"
                        textFormat: Text.PlainText
                        color: Th.statusPresetFg
                        font.pointSize: (Kirigami.Theme.smallFont.pointSize > 0 ? Kirigami.Theme.smallFont.pointSize : Math.max(7, Kirigami.Theme.defaultFont.pointSize * 0.85))
                    }
                    MouseArea {
                        id: customMa
                        anchors.fill: parent
                        hoverEnabled: true
                        cursorShape: Qt.PointingHandCursor
                        onClicked: {
                            panel._customOpen = !panel._customOpen
                            if (panel._customOpen) {
                                var s = panel.app ? panel.app.mMyPresenceStatus : null
                                customText.text = (s && s.id === "custom") ? s.text : ""
                                panel._customEmoji = (s && s.id === "custom") ? String(s.emoji || "") : ""
                                panel._customColor = (s && s.id === "custom") ? String(s.color || "blurple") : "blurple"
                                panel._customClear = 2
                                customText.forceActiveFocus()
                            }
                        }
                    }
                }
            }
            // "Custom…": your own words (≤ 40), one emoji, a palette colour, when it clears.
            ColumnLayout {
                visible: panel._customOpen
                Layout.fillWidth: true
                spacing: 8
                RowLayout {
                    Layout.fillWidth: true
                    spacing: 6
                    ThButton {
                        id: emojiBtn
                        app: panel._a
                        implicitWidth: 34
                        leftPadding: panel._customEmoji.length > 0 ? 8 : 9
                        rightPadding: 8
                        icon.name: panel._customEmoji.length > 0 ? "" : "face-smile-symbolic"
                        text: panel._customEmoji
                        onClicked: { panel._holdStatusPop = true; statusEmoji.open() }
                        Controls.ToolTip.text: panel._customEmoji.length > 0 ? "Change the emoji (right-click: none)" : "Pick an emoji"
                        Controls.ToolTip.visible: hovered
                        Controls.ToolTip.delay: 400
                        TapHandler { acceptedButtons: Qt.RightButton; onTapped: panel._customEmoji = "" }
                    }
                    ThTextField {
                        id: customText
                        Layout.fillWidth: true
                        maximumLength: 40
                        placeholderText: "What are you up to?"
                        onAccepted: saveCustom.clicked()
                    }
                }
                Flow {
                    Layout.fillWidth: true
                    spacing: 5
                    Repeater {
                        model: (panel.app && panel.app.mStatusPalette) ? panel.app.mStatusPalette : []
                        delegate: Rectangle {
                            id: swatch
                            required property var modelData
                            width: 20; height: 20; radius: 10   // 12 × 20 + 11 × 5 fits one row of the 308 px popover
                            color: modelData.hex
                            border.width: panel._customColor === modelData.key ? 2 : 0
                            border.color: Th.statusSwatchBorderChecked
                            MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor; onClicked: panel._customColor = swatch.modelData.key }
                            Accessible.role: Accessible.Button
                            Accessible.name: swatch.modelData.key
                        }
                    }
                }
                RowLayout {
                    Layout.fillWidth: true
                    spacing: 8
                    Controls.Label { text: "Clear after"; color: Th.statusLabelFg; font.pointSize: (Kirigami.Theme.smallFont.pointSize > 0 ? Kirigami.Theme.smallFont.pointSize : Math.max(7, Kirigami.Theme.defaultFont.pointSize * 0.85)) }
                    ThComboBox {
                        Layout.fillWidth: true
                        model: panel._clearChoices
                        currentIndex: panel._customClear
                        onActivated: (i) => panel._customClear = i
                    }
                }
                ThButton {
                    id: saveCustom
                    Layout.alignment: Qt.AlignRight
                    app: panel._a
                    highlighted: true
                    text: "Save"
                    enabled: customText.text.trim().length > 0
                    onClicked: panel._pickStatus({ id: "custom", text: customText.text.trim(), emoji: panel._customEmoji,
                                                   color: panel._customColor, until: panel._untilFor(panel._customClear) })
                }
            }
            ThButton {
                visible: !!panel.app && !!panel.app.mMyPresenceStatus
                app: panel._a
                flat: true
                icon.name: "edit-clear-symbolic"
                text: "Clear status"
                onClicked: if (panel.app && panel.app.presenceCtl) panel.app.presenceCtl.clearStatus()
            }
            Kirigami.Separator { Layout.fillWidth: true }
            // 2026-09-29: Profile settings next to Privacy settings (Tin Hat Web: presence/controls.tsx popLinks)
            Flow {
                Layout.fillWidth: true
                spacing: 4
                ThButton {
                    app: panel._a
                    flat: true
                    icon.name: "user-symbolic"
                    text: "Profile settings…"
                    onClicked: { statusPop.close(); panel.profileSettingsRequested() }
                }
                ThButton {
                    app: panel._a
                    flat: true
                    icon.name: "view-private-symbolic"
                    text: "Privacy settings…"
                    onClicked: { statusPop.close(); panel.privacySettingsRequested() }
                }
            }
        }
    }
    EmojiPicker {
        id: statusEmoji
        app: panel._a
        parent: Controls.Overlay.overlay
        x: Math.round((parent.width - width) / 2)
        y: Math.round((parent.height - height) / 2)
        onPicked: (emoji) => { panel._customEmoji = emoji; statusEmoji.close() }
        onClosed: panel._holdStatusPop = false
    }
    ConfirmDialog {
        id: shareForStatus
        app: panel._a
        dialogTitle: "Show your status to friends?"
        message: "This turns on sharing."
        confirmText: "Turn on"
        onConfirmed: {
            var s = panel._pendingStatus
            panel._pendingStatus = null
            if (s && panel.app && panel.app.presenceCtl) {
                panel.app.presenceCtl.setStatus(s.id, s.text, s.emoji, s.color, s.until, true)
                panel._customOpen = false
            }
        }
        onCancelled: panel._pendingStatus = null
        onClosed: panel._holdStatusPop = false
    }

    // ---- Shared per-participant volume menu ----
    // ONE hoisted instance for the whole panel, parameterized by panel.volMxid/volName
    // (set right before popup() — same pattern as RightPanel's memberMenu/actorMxid).
    // THREE labeled dials — Mic / Stream / Soundboard — each wired to the matching
    // gain-registry category ("mic" / "screenshare" / "music") on AudioPrefsController.
    // Each row: live-push on drag, persist on release, per-category mute, and a glyph;
    // a single Reset clears all three when any is off.
    // 0.8.31: the SAME component the call grid opens on a tile right-click
    // (ParticipantVolumeMenu.qml) — one menu, two surfaces, so they cannot drift.
    ParticipantVolumeMenu {
        id: sharedVolMenu
        app: panel.app
        audioPrefs: panel.audioPrefs
        mxid: panel.volMxid
        myMxid: panel.myMxid
        name: panel.volName
        onMessageUser: (m) => panel.messageUser(m)
        onChanged: panel.volRev++   // refresh the per-row muted glyphs
    }
}
