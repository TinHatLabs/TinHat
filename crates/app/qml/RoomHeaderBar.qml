// RoomHeaderBar — the gradient header that sits atop an open conversation.
//
// Ports the web `.mx_RoomHeader` look: a vertical warm gradient with a thin
// accent bottom border, the room title + a topic/subtitle line, a live-call chip,
// and three icon buttons (start/join call · gear ▾ · room info) with a hover
// scale-and-glow (web `.mx_RoomHeader_button`).
//
// Data + intent flow through plain QML properties/signals (CXX-Qt quirk: no direct
// controller binding). The parent (Main.qml) passes mirrored state in and wires the
// emitted signals to the controllers.
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the header.* tokens

Item {
    id: headerBar
    required property var app   // ApplicationWindow (theme tokens); set by parent
    FallbackTheme { id: fallbackTheme }

    // 11.0 buttons: the header's buttons are a row of the actions table (header.main) — the stored row or the preset's
    // default (today's six, same order). The six elements below are still the header's own; the row places them in
    // columns (after the avatar, the title and the next-up chip) and hides the ones it doesn't show (a dimmed one is
    // disabled: HeaderIconButton's own half opacity); anything else in the row is an ActionButton (generics).
    // Right-click → "Edit buttons…".
    RowModel {
        id: hrow
        app: headerBar.app
        islandRef: "header"
        rowId: "main"
        firstColumn: 3
        bespoke: ["reel.open", "calendar.open", "memories.open", "call.join", "call.devices.open", "room.info.toggle"]
    }

    // ---- Mirrored state (pushed by parent) ----
    property string roomName: ""
    property string roomId: ""        // canonical room id — Avatar seed (matches the room list)
    property string avatarSource: ""  // resolved room avatar (file://); empty -> letter fallback
    property string subtitle: ""      // topic line / status (optional)
    property bool callActive: false   // room has a live call
    property bool inCall: false       // we're joined
    property bool connecting: false
    // 0.8.26 — the space calendar: next events (from CalendarController) + whether the window is open.
    property var upcoming: []
    property bool calendarOpen: false

    // ---- Intent signals (only the three the header still emits — the per-action
    // mic/camera/share/verify/refresh buttons moved to the in-call bar / menus long ago,
    // and their never-emitted signals here only misdirected maintenance) ----
    signal callToggled()
    signal memoriesRequested() // open Memories (this room's recorded calls) — 2026-09-02: its own button
    signal infoRequested()    // open the right-hand room-info / members panel
    property bool infoOpen: false   // 10.1 (TH-RP-13): the panel is showing → the button reads pressed
    property real bgRadius: 0      // 11.0 (islands C6/C7): the header island's radius — the gradient rounds itself
    property int leftInset: 0       // 10.1 (TH-SHELL-21): corner mode parks its handle over the avatar's spot
    signal settingsRequested() // open the gear ▾ (devices / audio / quality / call features)
    signal calendarRequested() // 0.8.26: open the space calendar (button + the next-up chip)
    signal gameReelRequested() // 10.1 (TH-REEL-18): "what do we play tonight?" — one click, not a menu row
    property bool reelOpen: false

    // Next-up chip text: "Valheim · tomorrow 7:00 PM · 5 in"
    function relWhen(ms) {
        var now = new Date(); var d = new Date(ms)
        var t0 = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime()
        var t1 = new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime()
        var diff = Math.round((t1 - t0) / 86400000)
        var time = Qt.formatTime(d, "h:mm AP")
        if (ms <= now.getTime()) return "now"
        if (diff === 0) return "today " + time
        if (diff === 1) return "tomorrow " + time
        if (diff < 7) return Qt.formatDate(d, "ddd") + " " + time
        return Qt.formatDate(d, "MMM d")
    }
    function nextUpText(ev) {
        var when = ev.effStart ? relWhen(ev.effStart) : "vote"
        return (ev.game && ev.game.length > 0 ? ev.game : ev.title) + " · " + when
            + ((ev.rsvp && ev.rsvp.yes && ev.rsvp.yes.length > 0) ? " · " + ev.rsvp.yes.length + " in" : "")
    }
    function nextUpLong(ev) {
        return ev.effStart ? Qt.formatDateTime(new Date(ev.effStart), "dddd, MMM d · h:mm AP") : "Time to be decided — vote on the calendar"
    }

    implicitHeight: bar.implicitHeight

    Rectangle {
        id: bar
        anchors.fill: parent
        radius: headerBar.bgRadius
        // islands C2 (ISLANDS-ARCH §4.2): a whole number of pixels — 3.2 gu was 57.6 at gridUnit 18; the web draws 58
        implicitHeight: Math.max(Math.round(Kirigami.Units.gridUnit * 3.2), Math.ceil(row.implicitHeight + Kirigami.Units.largeSpacing))

        // Warm vertical gradient (web: 58,34,20 -> 36,21,16): the header.top.bg / .bottom.bg tokens, so
        // every theme gets a tasteful top-lit panel.
        gradient: Gradient {
            GradientStop { position: 0.0; color: Th.headerTopBg }
            GradientStop { position: 1.0; color: Th.headerBottomBg }
        }

        // Thin accent bottom border (web: rgba(255,122,51,0.18)); an island has its own frame instead.
        Rectangle {
            visible: headerBar.bgRadius === 0
            anchors { left: parent.left; right: parent.right; bottom: parent.bottom }
            height: 1
            gradient: Gradient {
                orientation: Gradient.Horizontal
                GradientStop { position: 0.0; color: "transparent" }
                GradientStop { position: 0.5; color: Th.headerDivider }
                GradientStop { position: 1.0; color: "transparent" }
            }
        }

        // 11.0 buttons: a one-row GridLayout (was a RowLayout — the same geometry: hidden items and empty columns take no
        // room and no spacing) so the row's order can place the header's own buttons (Layout.column).
        GridLayout {
            id: row
            anchors.fill: parent
            anchors.leftMargin: Kirigami.Units.largeSpacing + headerBar.leftInset
            anchors.rightMargin: Kirigami.Units.smallSpacing
            rows: 1
            rowSpacing: 0
            columnSpacing: Kirigami.Units.smallSpacing

            // Room avatar — the SAME central Avatar component (and new furry portrait set) as
            // every other avatar in the app, so a room with no image gets a per-room furry
            // portrait instead of a single static blobfox. Seeded by the canonical room id (NOT
            // the name) so the header portrait + hue MATCH the room-list / space-rail portrait
            // for the same room (those seed by id too).
            Avatar {
                Layout.row: 0; Layout.column: 0
                Layout.preferredWidth: Kirigami.Units.iconSizes.medium
                Layout.preferredHeight: Kirigami.Units.iconSizes.medium
                Layout.alignment: Qt.AlignVCenter
                app: headerBar.app
                size: Kirigami.Units.iconSizes.medium
                name: headerBar.roomName
                // §6: a DM is its person, a room its id — the room list's face, picture included (a DM without
                // its own picture shows the person's)
                seed: headerBar.roomId.length > 0
                      ? Seeds.avatarSeed(headerBar.roomId, headerBar.app ? headerBar.app.dmPeerOf(headerBar.roomId) : "")
                      : headerBar.roomName
                imageSource: headerBar.avatarSource.length > 0 ? headerBar.avatarSource
                             : (headerBar.app ? headerBar.app.dmPictureOf(headerBar.roomId) : "")
            }

            // Title + topic/subtitle.
            ColumnLayout {
                Layout.row: 0; Layout.column: 1
                Layout.fillWidth: true
                Layout.minimumWidth: Kirigami.Units.gridUnit * 6   // 10.1 (TH-NAV-23): chrome yields before the room's name
                Layout.alignment: Qt.AlignVCenter
                spacing: 0

                Controls.Label {
                    textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                    Layout.fillWidth: true
                    text: headerBar.roomName.length > 0 ? headerBar.roomName : "Room"
                    color: Th.headerTitleFg
                    font.bold: true
                    font.pointSize: Kirigami.Theme.defaultFont.pointSize * 1.08
                    elide: Text.ElideRight
                }
                Controls.Label {
                    textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                    Layout.fillWidth: true
                    visible: text.length > 0
                    text: headerBar.subtitle
                    color: Th.headerSubtitleFg
                    font.pointSize: Kirigami.Theme.smallFont.pointSize
                    elide: Text.ElideRight
                }
            }

            // 10.1 (TH-CONV-14): the pulsing "Live call" chip is gone — the join banner under the
            // header (with the names) and the accented call button are the two cues.

            // 0.8.26 — the next thing on the space calendar, at a glance (click opens it).
            Rectangle {
                id: nextUpChip
                readonly property var ev: (headerBar.upcoming && headerBar.upcoming.length > 0) ? headerBar.upcoming[0] : null
                visible: ev !== null && headerBar.width > Kirigami.Units.gridUnit * 34   // 10.1 (TH-NAV-23)
                Layout.row: 0; Layout.column: 2
                Layout.alignment: Qt.AlignVCenter
                implicitHeight: 26
                implicitWidth: nextRow.implicitWidth + 18
                radius: 13
                color: nextMa.containsMouse ? Th.headerNextUpBgHover : Th.headerNextUpBg
                border.width: 1
                border.color: Th.headerNextUpBorder
                Behavior on color { ColorAnimation { duration: Motion.dur } }
                Row {
                    id: nextRow
                    anchors.centerIn: parent
                    spacing: 6
                    ThIcon {
                        anchors.verticalCenter: parent.verticalCenter
                        app: headerBar.app
                        name: "view-calendar-symbolic"
                        width: 14; height: 14
                        color: Th.headerNextUpIcon
                        hovered: nextMa.containsMouse
                    }
                    Controls.Label {
                        textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                        anchors.verticalCenter: parent.verticalCenter
                        text: nextUpChip.ev ? headerBar.nextUpText(nextUpChip.ev) : ""
                        color: Th.headerNextUpFg
                        font.pointSize: Kirigami.Theme.smallFont.pointSize
                        elide: Text.ElideRight
                        width: Math.min(implicitWidth, 260)
                    }
                }
                MouseArea {
                    id: nextMa
                    anchors.fill: parent
                    hoverEnabled: true
                    cursorShape: Qt.PointingHandCursor
                    onClicked: headerBar.calendarRequested()
                }
                Controls.ToolTip.text: nextUpChip.ev ? (nextUpChip.ev.title + "\n" + headerBar.nextUpLong(nextUpChip.ev)) : ""
                Controls.ToolTip.visible: nextMa.containsMouse && nextUpChip.ev !== null
                Controls.ToolTip.delay: 400
            }

            // ---- Action icon buttons -------------------------------------
            // 10.1 (TH-REEL-18): the game reel was one app-menu row and a chip three clicks deep.
            HeaderIconButton {
                Layout.row: 0; Layout.column: Math.max(3, hrow.colOf("reel.open"))
                visible: hrow.shows("reel.open")
                enabled: !hrow.dimmed("reel.open")
                app: headerBar.app
                iconName: "applications-games-symbolic"
                accent: headerBar.reelOpen
                tip: "What do we play tonight? — the game reel"
                onClicked: headerBar.gameReelRequested()
            }
            // 0.8.26 — the space calendar (game nights, boss runs, free play, who's in).
            HeaderIconButton {
                Layout.row: 0; Layout.column: Math.max(3, hrow.colOf("calendar.open"))
                visible: hrow.shows("calendar.open")
                enabled: !hrow.dimmed("calendar.open")
                app: headerBar.app
                iconName: "view-calendar-symbolic"
                accent: headerBar.calendarOpen
                tip: "Calendar — what we're playing and when, who's in"
                onClicked: headerBar.calendarRequested()
            }
            // Memories 📼 — the room's recorded calls, one click, left of the call button
            // (it used to hide in the gear menu).
            HeaderIconButton {
                Layout.row: 0; Layout.column: Math.max(3, hrow.colOf("memories.open"))
                visible: hrow.shows("memories.open")
                enabled: !hrow.dimmed("memories.open")
                app: headerBar.app
                iconName: "camera-video-symbolic"
                tip: "Memories — relive this room's recorded calls"
                onClicked: headerBar.memoriesRequested()
            }
            // Start / join a call — only when NOT already in one. In a call, all controls
            // (hang-up, mic, camera, share, deafen, BRB) live on the in-call control bar, so
            // the header stays a clean [settings ▾] [info] pair.
            HeaderIconButton {
                Layout.row: 0; Layout.column: Math.max(3, hrow.colOf("call.join"))
                app: headerBar.app
                visible: hrow.shows("call.join") && !headerBar.inCall   // (it draws its own "Connecting…")
                iconName: "call-start-symbolic"
                accent: headerBar.callActive
                enabled: !headerBar.connecting
                tip: headerBar.connecting ? "Connecting…"
                    : (headerBar.callActive ? "Join the active call" : "Start a call")
                onClicked: headerBar.callToggled()
            }
            // Gear ▾ — the call & device menu. Distinct from the room-list footer's gear
            // (app Settings): this one is everything about calls in THIS room.
            HeaderIconButton {
                Layout.row: 0; Layout.column: Math.max(3, hrow.colOf("call.devices.open"))
                visible: hrow.shows("call.devices.open")
                enabled: !hrow.dimmed("call.devices.open")
                app: headerBar.app
                iconName: "configure-symbolic"
                tip: "Call & device settings"
                onClicked: headerBar.settingsRequested()
            }
            HeaderIconButton {
                Layout.row: 0; Layout.column: Math.max(3, hrow.colOf("room.info.toggle"))
                visible: hrow.shows("room.info.toggle")
                enabled: !hrow.dimmed("room.info.toggle")
                app: headerBar.app
                iconName:"documentinfo-symbolic"
                accent: headerBar.infoOpen
                tip: headerBar.infoOpen ? "Close room info" : "Room info & members"
                onClicked: headerBar.infoRequested()
            }
            // the Guests room only: "Copy guest link" after the row (the web's CopyLink.tsx header button; guests_ctl.rs
            // copyOffered = where the bar under the header offers Copy)
            HeaderIconButton {
                readonly property var g: headerBar.app && headerBar.app.mGuests ? headerBar.app.mGuests : ({})
                Layout.row: 0; Layout.column: hrow.lastColumn + 1
                visible: g.copyOffered === true && headerBar.roomId.length > 0 && g.room === headerBar.roomId
                app: headerBar.app
                iconName: "link-symbolic"
                tip: String((g.words || {}).copy || "Copy guest link")
                onClicked: headerBar.app.guestsCtl.copyLink()
            }
            // the row's other buttons (added from the editor, or all of them in another look / size) and its dividers
            Repeater {
                model: hrow.generics
                delegate: ActionButton {
                    required property var modelData
                    Layout.row: 0
                    Layout.column: modelData.col
                    Layout.alignment: Qt.AlignVCenter
                    app: headerBar.app
                    item: modelData.item
                    rowStyle: hrow.result.style
                    rowScale: hrow.result.scale
                    onRun: (anchor) => hrow.run(modelData.item, anchor)
                    onHold: (anchor, down) => hrow.run(modelData.item, anchor, { hold: down ? "down" : "up" })
                }
            }
            Repeater {
                model: hrow.seps
                delegate: Rectangle {
                    required property var modelData
                    Layout.row: 0
                    Layout.column: modelData.col
                    Layout.alignment: Qt.AlignVCenter
                    implicitWidth: 1
                    implicitHeight: 30
                    color: Th.actionSeparator
                }
            }
        }
        // 11.0 buttons: right-click the header → "Edit buttons…" (the row editor)
        TapHandler {
            acceptedButtons: Qt.RightButton
            onTapped: (point) => headerMenu.popup()
        }
        Controls.Menu {
            id: headerMenu
            Controls.MenuItem {
                text: "Edit buttons…"
                icon.name: "document-edit-symbolic"
                onTriggered: hrow.edit("Room header")
            }
        }
    }

}
