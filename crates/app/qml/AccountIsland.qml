import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the rooms.* / account.* / status.* tokens

// AccountIsland.qml — islands C3 (ISLANDS-ARCH §4.2): extracted as it was (the flush layout, pixel-identical); the
// parent keeps its placement (anchors, size) on the instance.
Rectangle {
    id: island
    required property var app   // the Main.qml root (panel.app)
    // islands C3: the account card (your avatar and name → "Your status", the settings buttons); ThRoomList still
    // anchors it at its bottom edge (the flush layout) — the ask card and the status popover sit relative to it
    required property var panelRef   // a ThRoomList's `panel`, passed in (never the same name: it would shadow the id)
    required property var statusPopRef   // a ThRoomList's `statusPop`, passed in (never the same name: it would shadow the id)

    // 11.0 (islands C6/C7): in the dock host the account ISLAND draws the card (fill, frame, radius) — framed: false
    property bool framed: true
    implicitHeight: island.panelRef.footerH - (island.framed ? 0 : 8)
    implicitWidth: 280
    radius: 12
    color: island.framed ? Th.accountBg : "transparent"
    border.width: island.framed ? 1 : 0
    border.color: Th.accountBorder
    // The card owns its pixels: a click or hover on its empty parts must never reach the
    // room rows scrolling underneath. (11.0 buttons: its right-click opens "Edit buttons…".)
    MouseArea {
        anchors.fill: parent
        hoverEnabled: true
        acceptedButtons: Qt.LeftButton | Qt.RightButton
        onClicked: (mouse) => { if (mouse.button === Qt.RightButton) accountMenu.popup() }
    }
    // 11.0 buttons: the card's buttons are a row of the actions table (account.main) — the stored row or the preset's
    // default (Report a problem, Tuck nav into corner, Menu); the three elements below are still the card's own, placed
    // in the row's order (Layout.column) after "you"; anything else in the row is an ActionButton (generics).
    RowModel {
        id: arow
        app: island.app
        islandRef: "account"
        rowId: "main"
        firstColumn: 1
        bespoke: ["app.bugReport", "app.sidebar.toggle", "app.menu.open"]
    }
    Controls.Menu {
        id: accountMenu
        Controls.MenuItem {
            text: "Edit buttons…"
            icon.name: "document-edit-symbolic"
            onTriggered: arow.edit("Your account card")
        }
    }
    // (a one-row GridLayout — was a RowLayout; the same geometry — so the row's order can place the buttons)
    GridLayout {
        anchors.fill: parent
        anchors.leftMargin: island.framed ? 8 : 4   // clear of the rounded corner (the island insets 4 already)
        anchors.rightMargin: island.framed ? 6 : 2
        rows: 1
        rowSpacing: 0
        columnSpacing: Kirigami.Units.smallSpacing
        // 10.6 (REACH §2.7): your avatar + name open "Your status". While you share, your own
        // dot (what others see — A2.15) and your custom status line under your name (A4.5).
        Item {
            id: meBtn
            Layout.row: 0; Layout.column: 0
            Layout.fillWidth: true
            Layout.fillHeight: true
            // selfhost: Reach only where the server offers it (presence)
            readonly property bool reach: !!island.panelRef.app && island.panelRef.app.serverOn("presence")
            readonly property var mine: (meBtn.reach && island.panelRef.app.mPresenceSharing && island.panelRef.app.presenceOf)
                ? island.panelRef.app.presenceOf(String(island.panelRef.app.mUserId || "")) : null
            Rectangle {
                anchors.fill: parent
                anchors.topMargin: 5; anchors.bottomMargin: 5; anchors.leftMargin: -4
                radius: 8
                color: (meMa.containsMouse || island.statusPopRef.opened) ? Th.accountMeBgHover : "transparent"
                Behavior on color { ColorAnimation { duration: island.panelRef._a.dur } }
            }
            RowLayout {
                anchors.fill: parent
                spacing: Kirigami.Units.smallSpacing
                Avatar {
                    app: island.panelRef._a
                    Layout.preferredWidth: 30; Layout.preferredHeight: 30
                    size: 30
                    name: island.panelRef.app ? island.panelRef.app.mUserId : ""
                    imageSource: island.panelRef.app ? island.panelRef.app.mMyAvatar : ""
                    presence: meBtn.mine ? String(meBtn.mine.state || "") : ""
                    presenceColor: (meBtn.mine && meBtn.mine.dot) ? Th.colourRef(meBtn.mine.dot) : ((meBtn.mine && meBtn.mine.state === "away") ? Th.presenceAwayDot : Th.presenceOnlineDot)
                    presenceText: meBtn.mine ? String(meBtn.mine.label || "") : ""
                    presenceRing: (meMa.containsMouse || island.statusPopRef.opened) ? Th.accountPresenceRingHover : Th.accountPresenceRing
                }
                ColumnLayout {
                    Layout.fillWidth: true
                    Layout.alignment: Qt.AlignVCenter
                    spacing: 1
                    Controls.Label {
                        textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                        Layout.fillWidth: true
                        text: island.panelRef.app ? island.panelRef._a.localpart(island.panelRef.app.mUserId) : ""
                        elide: Text.ElideRight
                        color: Th.accountNameFg
                        font.bold: true
                    }
                    StatusChip {
                        visible: !!meBtn.mine && !!meBtn.mine.chip
                        Layout.maximumWidth: parent.width
                        app: island.panelRef._a
                        text: (meBtn.mine && meBtn.mine.chip) ? String(meBtn.mine.chip) : ""
                        chipColor: (meBtn.mine && meBtn.mine.chipColor) ? meBtn.mine.chipColor : "#95A5A6"
                        pointSize: (Kirigami.Theme.smallFont.pointSize > 0 ? Kirigami.Theme.smallFont.pointSize : Math.max(7, Kirigami.Theme.defaultFont.pointSize * 0.85))
                        maxWidth: parent.width
                    }
                }
            }
            MouseArea {
                id: meMa
                anchors.fill: parent
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                // the press that closed the popup (outside it) must not reopen it
                onClicked: { if (!meBtn.reach) return; if (island.statusPopRef.opened) island.statusPopRef.close(); else if (Date.now() - island.panelRef._statusPopClosedAt > 250) island.statusPopRef.open() }
            }
            Controls.ToolTip.text: "Your status"
            Controls.ToolTip.visible: meBtn.reach && meMa.containsMouse && !island.statusPopRef.opened
            Controls.ToolTip.delay: 500
        }
        // 0.8.30: the three footer buttons draw through the icon pack (hover stories),
        // like the header and call bar — they were the style's static glyphs.
        ThToolButton {
            id: bugBtn
            Layout.row: 0; Layout.column: Math.max(1, arow.colOf("app.bugReport"))
            visible: arow.shows("app.bugReport")
            opacity: arow.dimmed("app.bugReport") ? 0.5 : 1
            enabled: !arow.dimmed("app.bugReport")
            app: island.panelRef._a
            iconName: "tools-report-bug-symbolic"
            iconColor: Th.buttonToolFg
            tip: "Report a problem (sends your note + current session log)"
            onClicked: island.panelRef.bugReportRequested()
        }
        ThToolButton {
            id: toggleNavBtn
            Layout.row: 0; Layout.column: Math.max(1, arow.colOf("app.sidebar.toggle"))
            visible: arow.shows("app.sidebar.toggle")
            opacity: arow.dimmed("app.sidebar.toggle") ? 0.5 : 1
            enabled: !arow.dimmed("app.sidebar.toggle")
            app: island.panelRef._a
            iconName: island.panelRef.sidebarCollapsed ? "sidebar-expand-left-symbolic" : "sidebar-collapse-left-symbolic"
            iconColor: Th.buttonToolFg
            active: island.panelRef.sidebarCollapsed
            // paired with the app menu's "Tuck nav into corner (OLED)" wording
            tip: island.panelRef.sidebarCollapsed ? "Pin nav as sidebar" : "Tuck nav into corner"
            onClicked: island.panelRef.toggleNavRequested()
        }
        ThToolButton {
            id: gearBtn
            Layout.row: 0; Layout.column: Math.max(1, arow.colOf("app.menu.open"))
            visible: arow.shows("app.menu.open")
            opacity: arow.dimmed("app.menu.open") ? 0.5 : 1
            enabled: !arow.dimmed("app.menu.open")
            app: island.panelRef._a
            iconName: "settings-configure-symbolic"
            iconColor: Th.buttonToolFg
            tip: "Menu — settings, theme, sign out…  (Ctrl+, opens Settings)"
            onClicked: island.panelRef.menuRequested(gearBtn)
        }
        // the row's other buttons (added from the editor, or all of them in another look / size) and its dividers
        Repeater {
            model: arow.generics
            delegate: ActionButton {
                required property var modelData
                Layout.row: 0
                Layout.column: modelData.col
                Layout.alignment: Qt.AlignVCenter
                app: island.app
                item: modelData.item
                rowStyle: arow.result.style
                rowScale: arow.result.scale
                onRun: (anchor) => arow.run(modelData.item, anchor)
                onHold: (anchor, down) => arow.run(modelData.item, anchor, { hold: down ? "down" : "up" })
            }
        }
        Repeater {
            model: arow.seps
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

    // ── 10.6 (REACH §2.7): the one-time question — a small card above the footer card, once per
    // account (`dev.tinhat.presence_asked`), only while not sharing, never during a call.
    Rectangle {
        id: askCard
        readonly property bool due: !!island.panelRef.app && island.panelRef.app.mLoggedIn === true && island.panelRef.app.mPresenceKnown === true
            && !island.panelRef.app.mPresenceAsked && !island.panelRef.app.mPresenceSharing && !island.panelRef.app.mPresenceTurningOff
            && !island.panelRef.app.mInCall && island.panelRef.app.serverOn("presence")
        visible: due || opacity > 0.01
        opacity: due ? 1 : 0
        Behavior on opacity { NumberAnimation { duration: island.panelRef._a.dur } }
        // 11.0: above the account ISLAND (it overflows the island upwards, over the room list, as it floated before)
        anchors.left: island.left; anchors.right: island.right
        anchors.leftMargin: island.framed ? 0 : -4; anchors.rightMargin: island.framed ? 0 : -4
        anchors.bottom: island.top; anchors.bottomMargin: island.framed ? 8 : 12
        height: askCol.implicitHeight + 24
        radius: 12
        color: Th.accountAskBg
        border.width: 1
        border.color: Th.accountAskBorder
        MouseArea { anchors.fill: parent; hoverEnabled: true }   // owns its pixels, like the footer card
        ColumnLayout {
            id: askCol
            anchors.left: parent.left; anchors.right: parent.right; anchors.top: parent.top
            anchors.margins: 12
            spacing: 10
            RowLayout {
                Layout.fillWidth: true
                spacing: 8
                Rectangle {   // the green of "Online" — what this card is about
                    Layout.alignment: Qt.AlignTop
                    Layout.topMargin: 4
                    implicitWidth: 10; implicitHeight: 10; radius: 5
                    color: Th.accountAskDot
                }
                Controls.Label {
                    Layout.fillWidth: true
                    wrapMode: Text.Wrap
                    color: Th.accountAskFg
                    text: "Let friends see when you're online? Only Online, Away or In a call — you can turn it off any time."
                }
            }
            RowLayout {
                Layout.alignment: Qt.AlignRight
                spacing: 6
                ThButton {
                    app: island.panelRef._a
                    flat: true
                    text: "No thanks"
                    implicitHeight: 30
                    onClicked: if (island.panelRef.app && island.panelRef.app.presenceCtl) island.panelRef.app.presenceCtl.answerAsk(false)
                }
                ThButton {
                    app: island.panelRef._a
                    highlighted: true
                    text: "Turn on"
                    implicitHeight: 30
                    onClicked: if (island.panelRef.app && island.panelRef.app.presenceCtl) island.panelRef.app.presenceCtl.answerAsk(true)
                }
            }
        }
    }
}
