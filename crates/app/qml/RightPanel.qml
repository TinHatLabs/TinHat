// RightPanel.qml — the room's right-hand info/members panel (web `.mx_RightPanel`).
//
// 10.1 (TH-CONV-08): a DOCKED column beside the conversation (Discord's member list), not a
// modal drawer that dimmed the room, swallowed typing and closed on any click. open() /
// close() / opened keep the old API; the host anchors it and animates its width.
// An Info | Members toggle. Read-only v1: room
// summary (name / topic / alias / encryption / member count) + the joined member
// list with avatars and role badges. Tapping a member emits avatarClicked(mxid) so
// the parent (Main.qml) opens the profile flyout.
//
// Data + helpers arrive via plain props / `app` (CXX-Qt: no direct controller
// binding; senderColor/localpart/radius live on Main.qml's root; the colours are Th's).
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the panel.* / members.* / profile.card.* tokens

Item {
    id: panel
    property bool shown: false
    readonly property bool opened: shown
    function open() { shown = true }
    function close() { shown = false }
    implicitWidth: Kirigami.Units.gridUnit * 18
    clip: true
    // 10.1 (TH-RP-08): plain text with URLs → rich text with anchors (escaped first). Security review 2026-10-26: quotes
    // too — a " in a link closed the href attribute; the anchors open through LinkGate.
    // BEGIN linkify (tools/linkify-test.mjs; Tin Hat Web's spaces/linkify.ts is the same walk)
    /// Topic / bio text → escaped HTML with http(s) links, trailing punctuation outside. URLs are found and trimmed in the
    /// RAW text, then escaped (assembly verify C, sec-night 2026-10-27: trimming the escaped text cut the `;` off
    /// `&quot;`). Clicks go through LinkGate like every link.
    function linkify(s) {
        var esc = function (x) { return x.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;") }
        var escText = function (x) { return esc(x).replace(/\n/g, "<br>") }
        var raw = String(s || ""), re = /https?:\/\/\S+/g, out = "", last = 0, m
        while ((m = re.exec(raw)) !== null) {
            var u = m[0], t = /[.,;:!?)]+$/.exec(u), trail = t ? t[0] : "", core = u.slice(0, u.length - trail.length)
            out += escText(raw.slice(last, m.index)) + "<a href=\"" + esc(core) + "\">" + esc(core) + "</a>" + esc(trail)
            last = m.index + u.length
        }
        return out + escText(raw.slice(last))
    }
    // END linkify
    required property var app
    property var members: []
    property var summary: ({})
    property string roomId: ""
    // Gaming: "what we're playing" activity + per-person "looking to play" status.
    property var activity: ({})          // {game,emoji,setBy} or {}
    property var statuses: []            // [{userId,text,expiresAt}]
    property string myStatus: ""
    property bool statusIsDm: false
    // Moderation/settings permissions (from RoomController; gate the affordances).
    property int myPower: 0
    property bool canChangeName: false
    property bool canChangeTopic: false
    property bool canInvite: false
    property bool canKick: false
    property bool canBan: false
    property bool canChangePower: false
    // RBAC §7.5: the room belongs to a space → the Members-tab gear + "Manage roles…" (roles live there).
    property bool inSpace: false
    signal openSpaceSettings()
    signal manageRoles(string mxid)
    signal avatarClicked(string mxid)
    signal setActivity(string game)      // "" clears
    signal postStatus(string text, int ttlMin)
    // Room-settings editing + moderation (routed to RoomController by Main.qml).
    signal renameRoom(string name)
    signal unbanUser(string mxid)        // 10.1 (TH-RP-03)
    signal editTopic(string topic)
    signal inviteUser(string mxid)
    signal kickUser(string mxid, string reason)
    signal banUser(string mxid, string reason)
    signal setMemberPower(string mxid, int power)
    signal requestProfileCard(string mxid)   // -> settings.fetchProfileCard in Main.qml
    signal playJoinSound(string url)         // -> soundboardCtl.playSound in Main.qml

    // Whether we may act on a given member: never on ourselves, and only on members
    // strictly below our power level (matches the server's auth rules so we don't offer
    // an action that will 403).
    function canActOn(member) {
        if (!member || member.mxid === (panel.app ? panel.app.mUserId : "")) return false
        return panel.myPower > (member.power || 0)
    }

    function statusFor(mxid) {
        var s = panel.statuses || []
        for (var i = 0; i < s.length; i++) if (s[i].userId === mxid) return s[i].text || ""
        return ""
    }

    FallbackTheme { id: fallbackTheme }
    readonly property var _a: app || fallbackTheme   // sizes and motion; colours are Th's panel.* / members.* / profile.card.*
    property int tab: 1   // 0 = Info, 1 = Members
    // Info pane: whether the name/topic inline editor is open.
    property bool editingInfo: false
    onTabChanged: if (tab !== 0) editingInfo = false
    onRoomIdChanged: editingInfo = false   // never carry an open editor across rooms
    // Member the moderation menu/dialogs currently act on (set when the menu opens).
    property string actorMxid: ""
    property string actorMembership: "join"   // 10.1 (TH-GAP-12): the row's membership (join | invite | ban)
    // 10.1 (TH-GAP-12): the tab label counts people who are actually in the room.
    readonly property int joinedCount: {
        var n = 0
        for (var i = 0; i < members.length; i++) if (!members[i].membership || members[i].membership === "join") n++
        return n
    }
    // Profile-card subject, captured when the card opens so it can't change under the
    // popup if the member list refreshes behind it.
    property string actorAvatar: ""
    property string cardMxid: ""
    readonly property var cardPres: (panel.app && panel.app.presenceOf && panel.cardMxid.length > 0) ? panel.app.presenceOf(panel.cardMxid) : null
    property string cardName: ""
    property string cardAvatar: ""
    property var cardData: ({})
    // Fed by Main.qml from settings.cardJson — `settings` is an id in THAT document, so
    // reading it here silently yielded undefined and the card never populated.
    property string cardJson: ""
    onCardJsonChanged: {
        try { panel.cardData = cardJson.length > 0 ? JSON.parse(cardJson) : ({}) }
        catch (e) { panel.cardData = ({}) }
    }

    // Who someone is: their photo, their words, their colour — the things the rest of
    // this release made settable (QOL 1.5.3).
    Controls.Popup {
        id: profileCard
        modal: true
        parent: Controls.Overlay.overlay
        anchors.centerIn: parent
        width: 340
        padding: 18
        background: Rectangle {
            color: Th.profileCardBg
            radius: 14
            border.width: 1
            border.color: (panel.cardData && panel.cardData.accent && panel.cardData.accent.length > 0)
                          ? panel.cardData.accent : Th.profileCardBorder
        }
        ColumnLayout {
            anchors.fill: parent
            spacing: 12
            RowLayout {
                spacing: 12
                Avatar {
                    app: panel._a
                    size: 64
                    name: panel.cardName.length > 0 ? panel.cardName : panel.cardMxid
                    seed: panel.cardMxid
                    imageSource: panel.cardAvatar
                }
                ColumnLayout {
                    spacing: 2
                    Controls.Label {
                        textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                        text: panel.cardName.length > 0 ? panel.cardName : panel.cardMxid
                        font.bold: true
                        font.pointSize: Kirigami.Theme.defaultFont.pointSize + 2
                    }
                    Controls.Label {
                        textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                        text: panel.cardMxid
                        color: Th.profileCardHintFg
                        font.pointSize: Kirigami.Theme.smallFont.pointSize
                    }
                    // 10.6 (REACH §2.6 item 4 + A4.5): "● Online" / "● In a call · <room>" / the chip.
                    PresenceLine {
                        Layout.topMargin: 2
                        Layout.maximumWidth: 220
                        app: panel.app
                        pres: panel.cardPres
                        roomName: (panel.cardPres && panel.cardPres.state === "call" && panel.cardPres.room && panel.app && panel.app.roomNameOf)
                                  ? panel.app.roomNameOf(String(panel.cardPres.room)) : ""
                    }
                }
                Item { Layout.fillWidth: true }
                Rectangle {
                    visible: !!(panel.cardData && panel.cardData.accent && panel.cardData.accent.length > 0)
                    Layout.preferredWidth: 18
                    Layout.preferredHeight: 18
                    radius: 9
                    color: visible ? panel.cardData.accent : "transparent"
                }
            }
            Controls.Label {
                visible: !!(panel.cardData && panel.cardData.bio && panel.cardData.bio.length > 0)
                text: visible ? panel.linkify(panel.cardData.bio) : ""
                textFormat: Text.RichText
                linkColor: Th.profileCardLinkFg
                onLinkActivated: (link) => LinkGate.open(link)
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
                HoverHandler { cursorShape: parent.hoveredLink.length > 0 ? Qt.PointingHandCursor : Qt.ArrowCursor }
            }
            Controls.Label {
                visible: !!(panel.cardData && panel.cardData.loading === true)
                text: "Loading…"
                color: Th.profileCardHintFg
            }
            Controls.Label {
                visible: !!(panel.cardData && panel.cardData.loading === false
                         && (!panel.cardData.bio || panel.cardData.bio.length === 0))
                text: "No profile set yet."
                color: Th.profileCardHintFg
                font.pointSize: Kirigami.Theme.smallFont.pointSize
            }
            // account safety (safety.rs): Ignore… / Stop ignoring · Report… (the web's ProfileCard PersonButtons)
            RowLayout {
                visible: panel.cardMxid.length > 0 && panel.cardMxid !== String(panel.app.mUserId || "") && !!panel.app.safety
                Layout.alignment: Qt.AlignHCenter
                Controls.Button {
                    icon.name: "dialog-cancel-symbolic"
                    text: LinkGate.plain(parent.visible ? (panel.app.safety.isIgnored(panel.cardMxid) ? String(panel.app.safety.w.unignore) : String(panel.app.safety.w.ignore)) : "")
                    onClicked: { var m = panel.cardMxid, n = panel.cardName || panel.cardMxid; profileCard.close(); panel.app.safety.openIgnore(m, n) }
                }
                Controls.Button {
                    icon.name: "dialog-warning"
                    text: LinkGate.plain(parent.visible ? String(panel.app.safety.w.report) : "")
                    onClicked: { var m = panel.cardMxid, n = panel.cardName || panel.cardMxid; profileCard.close(); panel.app.safety.openReport(m, panel.roomId, "", n) }
                }
            }
            RowLayout {
                Controls.Button {
                    text: "Play join sound"
                    icon.name: "media-playback-start"
                    visible: !!(panel.cardData && panel.cardData.joinSound && panel.cardData.joinSound.length > 0)
                    // Local preview only: playSound is a no-op outside a call, so this
                    // cannot blast someone else's join sound into a live room.
                    onClicked: panel.playJoinSound(panel.cardData.joinSound)
                }
                Item { Layout.fillWidth: true }
                Controls.Button { text: "Close"; onClicked: profileCard.close() }
            }
        }
    }
    property string actorName: ""
    property int actorPower: 0

    function roleColor(role) {
        if (role === "Admin") return Th.rolesBadgeAdminFg    // gold
        if (role === "Mod") return Th.rolesBadgeModFg // accent
        if (role === "Banned") return Th.rolesBadgeBannedFg   // 10.1 (TH-GAP-12)
        if (role === "Invited") return Th.rolesBadgeInvitedFg  // RBAC §7.5: the pending-invite badge
        return Th.rolesBadgeDefaultFg
    }
    // 10.4 (space tags): a row's member-list group — a hoisted space role's name, else the
    // power-level word. Rows from the old disk cache carry no `group` until the first live load.
    function groupOf(m) { return (m && m.group !== undefined) ? String(m.group || "") : String((m && m.role) || "") }
    function groupColor(section) {
        for (var i = 0; i < panel.members.length; i++) {
            var m = panel.members[i]
            if (panel.groupOf(m) === section && m.groupColor && String(m.groupColor).length > 0) return String(m.groupColor)
        }
        return panel.roleColor(section)
    }
    function groupLabel(section) {
        if (section === "Admin") return "Admins"
        if (section === "Mod") return "Moderators"
        if (section === "" || section === "Member") return "Members"
        return section     // Invited / Banned / a space role's own name
    }

    // 11.0 (islands C6/C7): in the dock host the panel ISLAND draws the fill and the frame — framed: false
    property bool framed: true
    Rectangle {
        anchors.fill: parent
        visible: panel.framed
        color: Th.panelBg
        Rectangle {
            anchors { left: parent.left; top: parent.top; bottom: parent.bottom }
            width: 1; color: Th.panelEdge
        }
    }

    ColumnLayout {
        anchors { left: parent.left; top: parent.top; bottom: parent.bottom }
        width: panel.implicitWidth   // laid out at full width even mid-animation (the host clips)
        spacing: Kirigami.Units.smallSpacing

        // ---- Info | Members toggle ----
        RowLayout {
            Layout.fillWidth: true
            Layout.margins: Kirigami.Units.largeSpacing
            spacing: Kirigami.Units.smallSpacing
            Controls.Button {
                text: "Info"
                checkable: true; autoExclusive: true
                checked: panel.tab === 0
                onClicked: panel.tab = 0
            }
            Controls.Button {
                text: LinkGate.plain("Members" + (panel.joinedCount > 0 ? " · " + panel.joinedCount : ""))
                checkable: true; autoExclusive: true
                checked: panel.tab === 1
                onClicked: panel.tab = 1
            }
            Item { Layout.fillWidth: true }
            // Edit room name/topic (Info tab; only if we may change either).
            Controls.ToolButton {
                icon.name: panel.editingInfo ? "dialog-cancel-symbolic" : "document-edit-symbolic"
                visible: panel.tab === 0 && (panel.canChangeName || panel.canChangeTopic)
                Controls.ToolTip.text: panel.editingInfo ? "Cancel editing" : "Edit room info"
                Controls.ToolTip.visible: hovered
                onClicked: panel.editingInfo = !panel.editingInfo
            }
            // RBAC §7.5: this room's space — roles, members, bans live in its settings.
            ThToolButton {
                visible: panel.tab === 1 && panel.inSpace
                app: panel.app
                iconName: "configure-symbolic"
                tip: "Space settings — roles and who's in the space"
                onClicked: panel.openSpaceSettings()
            }
        }

        // ---- Info pane ----
        // 10.1 (TH-RP-05): the Info tab scrolls — a long topic or a tall proposal list used to push
        // the controls off the bottom of the column.
        Controls.ScrollView {
            id: infoScroll
            Layout.fillWidth: true
            Layout.fillHeight: true
            visible: panel.tab === 0 && panel.summary.memberCount !== undefined
            clip: true
            contentWidth: availableWidth
            Controls.ScrollBar.vertical: FoxScrollBar { app: panel.app }
        ColumnLayout {
            x: Kirigami.Units.largeSpacing
            width: infoScroll.availableWidth - Kirigami.Units.largeSpacing * 2
            spacing: Kirigami.Units.smallSpacing

            Avatar {
                Layout.alignment: Qt.AlignHCenter
                app: panel.app
                size: 72
                name: panel.summary.name || ""
                // §6: the room list's face — a DM is its person (and their picture when it has none), a room its id
                seed: panel.roomId.length > 0 ? Seeds.avatarSeed(panel.roomId, panel.app.dmPeerOf(panel.roomId)) : (panel.summary.name || "")
                imageSource: panel.summary.avatar || panel.app.dmPictureOf(panel.roomId)
            }
            Controls.Label {
                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                Layout.fillWidth: true
                horizontalAlignment: Text.AlignHCenter
                visible: !panel.editingInfo
                text: panel.summary.name || ""
                color: Th.panelFg
                font.bold: true
                font.pointSize: Kirigami.Theme.defaultFont.pointSize * 1.15
                wrapMode: Text.WordWrap
            }
            Controls.Label {
                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                Layout.fillWidth: true
                horizontalAlignment: Text.AlignHCenter
                visible: !panel.editingInfo && text.length > 0
                text: panel.summary.alias || ""
                color: Th.panelHintFg
                font.pointSize: Kirigami.Theme.smallFont.pointSize
            }

            // ---- Inline name + topic editor (shown via the pencil; gated per-field) ----
            ColumnLayout {
                Layout.fillWidth: true
                visible: panel.editingInfo
                spacing: Kirigami.Units.smallSpacing
                // Re-seed the fields from the live summary whenever the editor opens.
                onVisibleChanged: if (visible) {
                    nameField.text = panel.summary.name || ""
                    topicField.text = panel.summary.topic || ""
                    if (panel.canChangeName) nameField.forceActiveFocus()
                    else if (panel.canChangeTopic) topicField.forceActiveFocus()
                }
                Controls.Label {
                    text: "Room name"
                    visible: panel.canChangeName
                    color: Th.panelSectionFg
                    font.pointSize: Kirigami.Theme.smallFont.pointSize
                    font.bold: true
                }
                Controls.TextField {
                    id: nameField
                    Layout.fillWidth: true
                    visible: panel.canChangeName
                    placeholderText: "Room name"
                }
                Controls.Label {
                    text: "Topic"
                    visible: panel.canChangeTopic
                    color: Th.panelSectionFg
                    font.pointSize: Kirigami.Theme.smallFont.pointSize
                    font.bold: true
                }
                Controls.TextArea {
                    id: topicField
                    Layout.fillWidth: true
                    visible: panel.canChangeTopic
                    wrapMode: Text.WordWrap
                    placeholderText: "Topic"
                }
                RowLayout {
                    Layout.fillWidth: true
                    spacing: Kirigami.Units.smallSpacing
                    Item { Layout.fillWidth: true }
                    Controls.Button {
                        text: "Cancel"
                        onClicked: panel.editingInfo = false
                    }
                    Controls.Button {
                        text: "Save"
                        icon.name: "dialog-ok-apply-symbolic"
                        onClicked: {
                            if (panel.canChangeName && nameField.text !== (panel.summary.name || ""))
                                panel.renameRoom(nameField.text)
                            if (panel.canChangeTopic && topicField.text !== (panel.summary.topic || ""))
                                panel.editTopic(topicField.text)
                            panel.editingInfo = false
                        }
                    }
                }
            }
            // Encryption pill.
            Rectangle {
                Layout.alignment: Qt.AlignHCenter
                visible: panel.summary.encrypted !== undefined
                implicitWidth: encRow.implicitWidth + Kirigami.Units.largeSpacing
                implicitHeight: Kirigami.Units.gridUnit * 1.5
                radius: height / 2
                color: panel.summary.encrypted ? Th.panelEncryptionBgActive : "transparent"
                border.width: 1
                border.color: panel.summary.encrypted ? Th.panelEncryptionBorderActive : Th.panelEncryptionBorder
                RowLayout {
                    id: encRow
                    anchors.centerIn: parent
                    spacing: Kirigami.Units.smallSpacing
                    Kirigami.Icon {
                        source: panel.summary.encrypted ? "channel-secure-symbolic" : "channel-insecure-symbolic"
                        implicitWidth: 16; implicitHeight: 16
                        color: panel.summary.encrypted ? Th.panelEncryptionFgActive : Th.panelEncryptionFg
                    }
                    Controls.Label {
                        text: panel.summary.encrypted ? "Encrypted" : "Not encrypted"
                        color: panel.summary.encrypted ? Th.panelEncryptionFgActive : Th.panelEncryptionFg
                        font.pointSize: Kirigami.Theme.smallFont.pointSize
                    }
                }
            }
            Kirigami.Separator { Layout.fillWidth: true; visible: !panel.editingInfo && (panel.summary.topic || "").length > 0 }
            Controls.Label {
                Layout.fillWidth: true
                visible: !panel.editingInfo && (panel.summary.topic || "").length > 0
                text: panel.linkify(panel.summary.topic || "")
                textFormat: Text.RichText
                linkColor: Th.panelLinkFg
                onLinkActivated: (link) => LinkGate.open(link)
                color: Th.panelFg
                wrapMode: Text.WordWrap
                font.pointSize: Kirigami.Theme.smallFont.pointSize
                HoverHandler { cursorShape: parent.hoveredLink.length > 0 ? Qt.PointingHandCursor : Qt.ArrowCursor }
            }

            // "What we're playing" — the room activity (dev.tinhat.activity).
            Kirigami.Separator { Layout.fillWidth: true }
            Controls.Label {
                text: "Now playing"
                color: Th.panelSectionFg
                font.pointSize: Kirigami.Theme.smallFont.pointSize
                font.bold: true
            }
            RowLayout {
                Layout.fillWidth: true
                visible: !!(panel.activity && panel.activity.game)
                spacing: Kirigami.Units.smallSpacing
                // A real icon unless the activity carries its own emoji.
                Kirigami.Icon {
                    visible: !(panel.activity && panel.activity.emoji && panel.activity.emoji.length > 0)
                    source: "applications-games-symbolic"
                    Layout.preferredWidth: 16; Layout.preferredHeight: 16
                    Layout.alignment: Qt.AlignTop
                    color: Th.panelActivityFg
                }
                Controls.Label {
                    textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                    Layout.fillWidth: true
                    text: ((panel.activity.emoji && panel.activity.emoji.length > 0) ? panel.activity.emoji + " "
                          : "")
                        + (panel.activity.game || "")
                        + ((panel.activity.setBy && panel.app) ? "  ·  " + panel.app.localpart(panel.activity.setBy) : "")
                    color: Th.panelActivityFg
                    wrapMode: Text.WordWrap
                }
            }
            RowLayout {
                Layout.fillWidth: true
                spacing: Kirigami.Units.smallSpacing
                Controls.TextField {
                    id: activityField
                    Layout.fillWidth: true
                    placeholderText: "What are you playing?"
                    onAccepted: { if (text.length > 0) { panel.setActivity(text); text = "" } }
                }
                Controls.ToolButton {
                    icon.name: "dialog-cancel-symbolic"
                    visible: !!(panel.activity && panel.activity.game)
                    Controls.ToolTip.text: "Clear now playing"
                    Controls.ToolTip.visible: hovered
                    onClicked: panel.setActivity("")
                }
            }
            Item { Layout.fillHeight: true }
        }
        }

        // ---- My status editor (Members tab, non-DM rooms only) ----
        ColumnLayout {
            Layout.fillWidth: true
            Layout.leftMargin: Kirigami.Units.largeSpacing
            Layout.rightMargin: Kirigami.Units.largeSpacing
            spacing: 2
            visible: panel.tab === 1 && !panel.statusIsDm && panel.members.length > 0
            Controls.Label {
                text: "Your status"
                color: Th.panelSectionFg
                font.pointSize: Kirigami.Theme.smallFont.pointSize
                font.bold: true
            }
            RowLayout {
                Layout.fillWidth: true
                spacing: Kirigami.Units.smallSpacing
                Controls.TextField {
                    id: statusField
                    Layout.fillWidth: true
                    placeholderText: panel.myStatus.length > 0 ? panel.myStatus : "Looking to play…"
                    onAccepted: { if (text.length > 0) { panel.postStatus(text, ttlCombo.currentValue); text = "" } }
                }
                Controls.ComboBox {
                    id: ttlCombo
                    Layout.preferredWidth: Kirigami.Units.gridUnit * 3.2
                    textRole: "t"; valueRole: "v"
                    model: [{t:"15m",v:15}, {t:"30m",v:30}, {t:"1h",v:60}, {t:"3h",v:180}, {t:"∞",v:0}]
                    Component.onCompleted: currentIndex = 1   // default 30m
                }
                Controls.ToolButton {
                    icon.name: "dialog-cancel-symbolic"
                    visible: panel.myStatus.length > 0
                    Controls.ToolTip.text: "Clear your status"
                    Controls.ToolTip.visible: hovered
                    onClicked: panel.postStatus("", 0)
                }
            }
        }

        // ---- Invite a user (Members tab; only if we may invite) ----
        RowLayout {
            Layout.fillWidth: true
            Layout.leftMargin: Kirigami.Units.largeSpacing
            Layout.rightMargin: Kirigami.Units.largeSpacing
            visible: panel.tab === 1 && panel.canInvite && panel.members.length > 0
            spacing: Kirigami.Units.smallSpacing
            Controls.TextField {
                id: inviteField
                Layout.fillWidth: true
                placeholderText: "Invite @user:server…"
                onAccepted: {
                    if (text.trim().length > 0) { panel.inviteUser(text.trim()); text = "" }
                }
            }
            Controls.ToolButton {
                icon.name: "list-add-user"   // 10.1 (TH-ICON-06): the same invite glyph as everywhere else
                enabled: inviteField.text.trim().length > 0
                Controls.ToolTip.text: "Invite"
                Controls.ToolTip.visible: hovered
                onClicked: {
                    if (inviteField.text.trim().length > 0) {
                        panel.inviteUser(inviteField.text.trim()); inviteField.text = ""
                    }
                }
            }
        }

        // ---- Members pane ----
        ListView {
            id: memberList
            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
            visible: panel.tab === 1 && panel.members.length > 0
            model: panel.members
            spacing: 2
            // Rows arrive sorted by power level, so identical roles are already
            // contiguous; the header just marks where each run starts. Counting is done
            // here rather than in Rust because it is a presentation question, and the
            // count has to match exactly what this list is showing.
            section.property: "group"   // 10.4: hoisted space roles group the list (Admins, Streamers, …)
            section.criteria: ViewSection.FullString
            section.delegate: Item {
                id: secHeader
                required property string section
                width: memberList.width
                height: Kirigami.Units.gridUnit * 1.6
                Controls.Label {
                    textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                    anchors.left: parent.left
                    anchors.bottom: parent.bottom
                    anchors.bottomMargin: 2
                    text: {
                        var n = 0
                        for (var i = 0; i < panel.members.length; i++)
                            if (panel.groupOf(panel.members[i]) === secHeader.section) n++
                        // "Admin" is what the badge says; the header is a group of
                        // people, so it reads as a plural. A space role keeps its own name.
                        return panel.groupLabel(secHeader.section) + " — " + n
                    }
                    color: panel.groupColor(secHeader.section)
                    font.bold: true
                    font.pointSize: Kirigami.Theme.smallFont.pointSize
                }
            }
            delegate: Item {
                id: row
                required property var modelData
                // 10.6 (REACH §2.6 / A4.5): what the presence service says about this member (null = no dot)
                readonly property var pres: (panel.app && panel.app.presenceOf) ? panel.app.presenceOf(String(row.modelData.mxid || "")) : null
                width: memberList.width
                height: Kirigami.Units.gridUnit * 2.6
                Rectangle {
                    anchors.fill: parent
                    anchors.leftMargin: Kirigami.Units.smallSpacing
                    anchors.rightMargin: Kirigami.Units.smallSpacing
                    color: mma.containsMouse ? Th.membersRowBgHover : "transparent"
                    radius: panel._a.radiusSm
                    // Declared FIRST so it is the bottom layer: row taps/right-clicks
                    // propagate to it through the non-interactive labels, while the
                    // overflow ToolButton (later sibling in the RowLayout) sits on top
                    // and gets its own clicks.
                    MouseArea {
                        id: mma
                        anchors.fill: parent
                        hoverEnabled: true
                        acceptedButtons: Qt.LeftButton | Qt.RightButton
                        cursorShape: Qt.PointingHandCursor
                        onClicked: (mouse) => {
                            if (mouse.button === Qt.RightButton) {
                                if (panel.canActOn(row.modelData)
                                    && (panel.canKick || panel.canBan || panel.canChangePower)) {
                                    panel.actorMxid = row.modelData.mxid
                                    panel.actorMembership = row.modelData.membership || "join"
                                    panel.actorName = row.modelData.name || row.modelData.mxid
                                    panel.actorAvatar = row.modelData.avatar || ""
                                    panel.actorPower = row.modelData.power || 0
                                    memberMenu.popup()
                                }
                            } else {
                                panel.avatarClicked(row.modelData.mxid)
                            }
                        }
                    }
                    RowLayout {
                        anchors.fill: parent
                        anchors.leftMargin: Kirigami.Units.smallSpacing
                        anchors.rightMargin: Kirigami.Units.smallSpacing
                        spacing: Kirigami.Units.smallSpacing
                        Avatar {
                            app: panel.app
                            size: Kirigami.Units.gridUnit * 1.8
                            name: row.modelData.name || row.modelData.mxid
                            seed: row.modelData.mxid || row.modelData.name || ""
                            imageSource: row.modelData.avatar || ""
                            // 10.6 (REACH §2.6 item 3): their dot when they share; the list order never changes.
                            presence: row.pres ? String(row.pres.state || "") : ""
                            presenceColor: (row.pres && row.pres.dot) ? Th.colourRef(row.pres.dot) : ((row.pres && row.pres.state === "away") ? Th.presenceAwayDot : Th.presenceOnlineDot)
                            presenceText: row.pres ? String(row.pres.label || "") : ""
                            presenceRing: mma.containsMouse ? Th.membersPresenceRingHover : Th.membersPresenceRing
                        }
                        ColumnLayout {
                            Layout.fillWidth: true
                            spacing: 0
                            Controls.Label {
                                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                Layout.fillWidth: true
                                text: row.modelData.name || (panel.app ? panel.app.localpart(row.modelData.mxid) : row.modelData.mxid)
                                // 10.4 (space tags): the custom role's colour, else the hashed hue.
                                color: (row.modelData.tagColor && String(row.modelData.tagColor).length > 0) ? String(row.modelData.tagColor)
                                       : panel.app.senderColor(row.modelData.mxid)   // `app` is required
                                font.bold: true
                                elide: Text.ElideRight
                                // 10.1 (TH-RP-15): the full id on hover (two people can share a display name).
                                HoverHandler { id: nameHover }
                                Controls.ToolTip.text: row.modelData.mxid || ""
                                Controls.ToolTip.visible: nameHover.hovered && (row.modelData.mxid || "").length > 0
                                Controls.ToolTip.delay: 500
                            }
                            // 10.6 (A4.5): a custom status is the member row's secondary line, as a chip.
                            StatusChip {
                                visible: !!row.pres && !!row.pres.chip
                                Layout.maximumWidth: parent.width
                                app: panel.app
                                text: (row.pres && row.pres.chip) ? String(row.pres.chip) : ""
                                chipColor: (row.pres && row.pres.chipColor) ? row.pres.chipColor : "#95A5A6"
                                maxWidth: parent.width
                            }
                            Controls.Label {
                                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                visible: !(row.pres && row.pres.chip)
                                Layout.fillWidth: true
                                // Show the member's "looking to play" status if they have one,
                                // else their mxid.
                                property string st: panel.statusFor(row.modelData.mxid)
                                text: st.length > 0 ? st : row.modelData.mxid
                                color: st.length > 0 ? Th.membersStatusFg : Th.membersMetaFg
                                font.italic: st.length > 0
                                font.pointSize: Kirigami.Theme.smallFont.pointSize
                                elide: Text.ElideRight
                            }
                        }
                        // Role badge — RBAC §7.5: the space role's name + colour when the row carries
                        // them (roleName / roleColor; rows from the old disk cache have neither until
                        // the first live load), else the power-level fallback. The baseline roles
                        // ("Member", "Everyone") wear no chip — Discord's @everyone doesn't either.
                        RoleBadge {
                            // 10.4 (space tags): the custom role first; else the top role / power-level word as before.
                            readonly property bool hasTag: !!(row.modelData.tagName && String(row.modelData.tagName).length > 0)
                            readonly property string rn: hasTag ? String(row.modelData.tagName)
                                                         : (row.modelData.roleName !== undefined ? String(row.modelData.roleName || "") : String(row.modelData.role || ""))
                            readonly property string rc: hasTag ? String(row.modelData.tagColor || "") : String(row.modelData.roleColor || "")
                            visible: rn.length > 0 && rn !== "Member" && rn !== "Everyone"
                            Layout.alignment: Qt.AlignVCenter
                            app: panel.app
                            name: rn
                            roleColor: rc.length > 0 ? rc : String(panel.roleColor(rn))
                        }
                        // Per-member moderation overflow (kick / ban / change power),
                        // gated on our power level vs the target.
                        Controls.ToolButton {
                            id: modBtn
                            Layout.alignment: Qt.AlignVCenter
                            icon.name: "view-more-symbolic"
                            visible: (mma.containsMouse || memberMenu.visible)
                                     && panel.canActOn(row.modelData)
                                     && (panel.canKick || panel.canBan || panel.canChangePower)
                            onClicked: {
                                panel.actorMxid = row.modelData.mxid
                                panel.actorMembership = row.modelData.membership || "join"
                                panel.actorName = row.modelData.name || row.modelData.mxid
                                panel.actorAvatar = row.modelData.avatar || ""
                                panel.actorPower = row.modelData.power || 0
                                memberMenu.popup()
                            }
                        }
                    }
                }
            }
            Controls.ScrollBar.vertical: FoxScrollBar { app: panel._a }   // 10.1 (TH-DS-05): the house scrollbar everywhere
        }

        // Loading state — shown before the live member sync lands. The disk cache makes
        // repeat opens instant; this only appears on a first-ever (cache-miss) open, so it
        // reads as "loading" instead of looking broken/empty.
        ColumnLayout {
            id: rpLoading
            Layout.fillWidth: true
            Layout.fillHeight: true
            visible: (panel.tab === 0 && panel.summary.memberCount === undefined)
                     || (panel.tab === 1 && panel.members.length === 0)
            // 10.1 (TH-STATE-07): a spinner that never resolves is not "loading" — after 12 s say so.
            property bool slow: false
            Timer { interval: 12000; running: rpLoading.visible; onTriggered: rpLoading.slow = true }
            onVisibleChanged: if (!visible) slow = false
            Item { Layout.fillHeight: true }
            ThBusyIndicator {   // 11.0 (GPU.md §2.5): still
                Layout.alignment: Qt.AlignHCenter
                size: 36
                color: Th.loaderSpinnerFg
                running: parent.visible && !rpLoading.slow
                visible: running
            }
            Controls.Label {
                Layout.alignment: Qt.AlignHCenter
                Layout.fillWidth: true
                horizontalAlignment: Text.AlignHCenter
                wrapMode: Text.WordWrap
                text: rpLoading.slow ? "Still loading — the server may be slow or unreachable. Close and reopen this panel to try again."
                    : (panel.tab === 1 ? "Loading members…" : "Loading room info…")
                color: Th.panelHintFg
            }
            Item { Layout.fillHeight: true }
        }
    }

    // ---- Per-member moderation menu (kick / ban / change power) ----
    // Opened from a member row's overflow button or right-click; acts on panel.actorMxid.
    Controls.Menu {
        id: memberMenu
        modal: true
        Controls.MenuItem {
            text: "View profile"
            icon.name: "user-identity"
            onTriggered: {
                panel.cardMxid = panel.actorMxid
                panel.cardName = panel.actorName
                panel.cardAvatar = panel.actorAvatar
                panel.requestProfileCard(panel.actorMxid)
                profileCard.open()
            }
        }
        // account safety (safety.rs): Ignore… / Stop ignoring · Report… (the web's PersonMenuItems; never for myself)
        Controls.MenuItem {
            readonly property bool other: panel.actorMxid.length > 0 && panel.actorMxid !== String(panel.app.mUserId || "")
            visible: other
            height: visible ? implicitHeight : 0
            text: LinkGate.plain(panel.app.safety.isIgnored(panel.actorMxid) ? String(panel.app.safety.w.unignore) : String(panel.app.safety.w.ignore))
            icon.name: "dialog-cancel-symbolic"
            onTriggered: panel.app.safety.openIgnore(panel.actorMxid, panel.actorName)
        }
        Controls.MenuItem {
            visible: panel.actorMxid.length > 0 && panel.actorMxid !== String(panel.app.mUserId || "")
            height: visible ? implicitHeight : 0
            text: LinkGate.plain(String(panel.app.safety.w.report))
            icon.name: "dialog-warning"
            onTriggered: panel.app.safety.openReport(panel.actorMxid, panel.roomId, "", panel.actorName)
        }
        Controls.MenuSeparator {}
        Controls.MenuItem {
            text: "Change role…"
            icon.name: "system-users-symbolic"
            visible: panel.canChangePower && panel.actorMembership === "join"
            height: visible ? implicitHeight : 0
            onTriggered: powerMenu.popup()
        }
        Controls.MenuSeparator { visible: panel.canChangePower && (panel.canKick || panel.canBan) }
        Controls.MenuItem {
            text: panel.actorMembership === "invite" ? "Withdraw invite…" : "Kick…"   // 10.1 (TH-GAP-12): a kick on an invitee revokes the invite
            icon.name: "im-kick-user-symbolic"
            visible: panel.canKick && panel.actorMembership !== "ban"
            height: visible ? implicitHeight : 0
            onTriggered: kickConfirm.open()
        }
        Controls.MenuItem {
            text: "Ban…"
            icon.name: "im-ban-user-symbolic"
            visible: panel.canBan && panel.actorMembership !== "ban"
            height: visible ? implicitHeight : 0
            onTriggered: banConfirm.open()
        }
        // 10.1 (TH-RP-03): the unban that Rust always had and nothing called.
        Controls.MenuItem {
            text: "Unban"
            icon.name: "checkmark-symbolic"
            visible: panel.canBan && panel.actorMembership === "ban"
            height: visible ? implicitHeight : 0
            onTriggered: panel.unbanUser(panel.actorMxid)
        }
        // RBAC §7.5: roles are a space thing — hand over to the space's Members tab.
        Controls.MenuSeparator { visible: panel.inSpace && panel.canChangePower; height: visible ? implicitHeight : 0 }
        Controls.MenuItem {
            text: "Manage roles…"
            icon.name: "configure-symbolic"
            visible: panel.inSpace && panel.canChangePower
            height: visible ? implicitHeight : 0
            onTriggered: panel.manageRoles(panel.actorMxid)
        }
    }

    // Power-level picker (Member / Moderator / Admin) — the common presets.
    Controls.Menu {
        id: powerMenu
        modal: true
        Repeater {
            model: [{ t: "Admin", v: 100 }, { t: "Moderator", v: 50 }, { t: "Member", v: 0 }]   // 10.1 (TH-COPY-11): roles, not power-level numbers
            delegate: Controls.MenuItem {
                required property var modelData
                // Only offer levels strictly below our own — the server rejects
                // granting power at/above the actor's level (matches canActOn).
                visible: modelData.v < panel.myPower
                height: visible ? implicitHeight : 0
                checkable: true
                checked: panel.actorPower === modelData.v
                text: LinkGate.plain(modelData.t)
                onTriggered: {
                    panel.setMemberPower(panel.actorMxid, modelData.v)
                    // the click toggled `checked` imperatively, killing the binding —
                    // restore it so the next popup reflects the (new) actor's level
                    checked = Qt.binding(function () { return panel.actorPower === modelData.v })
                }
            }
        }
    }

    ConfirmDialog {
        id: kickConfirm
        app: panel.app
        dialogTitle: panel.actorMembership === "invite" ? "Withdraw invite?" : "Kick member?"
        confirmText: panel.actorMembership === "invite" ? "Withdraw" : "Kick"
        destructive: true
        message: panel.actorMembership === "invite"
            ? "Withdraw the invite for " + panel.actorName + "? They can be invited again later."
            : "Remove " + panel.actorName + " from this room? They can rejoin if it's public or re-invited."
        onConfirmed: panel.kickUser(panel.actorMxid, "")
    }
    ConfirmDialog {
        id: banConfirm
        app: panel.app
        dialogTitle: "Ban member?"
        confirmText: "Ban"
        destructive: true
        message: "Ban " + panel.actorName + " from this room? They won't be able to rejoin until unbanned."
        onConfirmed: panel.banUser(panel.actorMxid, "")
    }
}
