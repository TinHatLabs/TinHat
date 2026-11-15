// ThreadsPane.qml — threads (docs/THREADS.md): the right panel's "Threads" tab. Tin Hat Web's threads/ThreadsPane.tsx:
//   the list   All threads / My threads, newest activity first (the room controller's threadsJson: the server's
//              /threads + the window's thread replies, ordered by the shared core's thread_list), each with its root's
//              sender and text, the summary line and an unread dot;
//   a thread   Back (→ the list) + the thread in a ConversationView driven by its OWN RoomController in thread mode
//              (threadRoot set before open(): root, replies, its own composer; receipts with the thread's id).
// Keyboard: ↑/↓/Home/End walk the list, Enter / Space opens; Escape in a thread goes back to the list; focus returns to
// the control that opened the thread (a summary) when it is still there, else to the thread's row in the list.
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app

Item {
    id: tp
    required property var app
    property var roomCtl: null     // the main view's RoomController (threadsJson, loadThreads, threadSummaryText)
    property string roomId: ""
    property var gif: null
    property bool active: false    // the tab is showing
    property string filter: "all"
    property string openRoot: ""   // "" = the list
    property var opener: null
    property var listData: ({ items: [], all: [], mine: [] })
    property int tick: 0
    signal avatarActivated(string mxid)

    property string focusEvent: ""   // a reply to centre once the thread shows it (a jump to that reply)
    function openThread(root, op, focusEvent) {
        tp.opener = op || null
        tp.openRoot = String(root || "")
        tp.focusEvent = String(focusEvent || "")
        if (tp.focusEvent.length > 0 && tp.focusEvent !== tp.openRoot) { jumpTimer.tries = 0; jumpTimer.restart() }
    }
    // a jump to a reply: centre it once the thread has it, paging the thread back toward it (up to ~15 s); restarted
    // for every jump, also to the same reply
    Timer {
        id: jumpTimer
        property int tries: 0
        interval: 500; repeat: true
        onTriggered: {
            tries++
            var it = threadLoader.item
            var cv = it ? it.conv : null
            if (cv && cv.hasEvent(tp.focusEvent)) { stop(); cv._jumpTo(tp.focusEvent); tp.focusEvent = ""; return }
            if (tries > 30) { stop(); tp.focusEvent = ""; return }
            // not loaded yet: one more page of the thread every 2 s while it has older replies
            if (it && it.ctl && tries % 4 === 0 && it.ctl.hasMore && !it.ctl.loadingOlder) it.ctl.loadOlder()
        }
    }
    function closeThread() {
        var root = tp.openRoot
        tp.openRoot = ""
        Qt.callLater(function () {
            if (tp.opener && tp.opener.visible && tp.opener.forceActiveFocus) { tp.opener.forceActiveFocus(Qt.OtherFocusReason); return }
            for (var i = 0; i < list.count; i++) {
                var it = list.itemAtIndex(i)
                if (it && it.root === root) { list.currentIndex = i; it.forceActiveFocus(Qt.OtherFocusReason); return }
            }
            list.forceActiveFocus(Qt.OtherFocusReason)
        })
    }
    function reload() { if (tp.roomCtl && tp.roomId.length > 0) tp.roomCtl.loadThreads() }
    function itemFor(root) {
        var its = tp.listData.items || []
        for (var i = 0; i < its.length; i++) if (its[i].root === root) return its[i]
        return null
    }
    readonly property var order: tp.filter === "mine" ? (tp.listData.mine || []) : (tp.listData.all || [])

    onRoomIdChanged: { tp.openRoot = ""; tp.listData = ({ items: [], all: [], mine: [] }); tp._lastJson = ""; if (tp.active) tp.reload() }
    onActiveChanged: if (tp.active) tp.reload()
    onOpenRootChanged: {
        // a thread opened while another one shows: a fresh controller for it
        threadLoader.active = false
        threadLoader.active = Qt.binding(function () { return tp.openRoot.length > 0 && tp.roomId.length > 0 })
        if (tp.openRoot.length === 0 && tp.active) tp.reload()
    }
    // CXX-Qt note (ConversationView): the controller's getters are polled, not bound
    Timer {
        interval: 400; repeat: true; running: tp.active && tp.openRoot.length === 0
        onTriggered: {
            var j = tp.roomCtl ? String(tp.roomCtl.threadsJson || "") : ""
            if (j.length === 0 || j === tp._lastJson) return
            tp._lastJson = j
            try { tp.listData = JSON.parse(j) } catch (e) { tp.listData = ({ items: [], all: [], mine: [] }) }
        }
    }
    property string _lastJson: ""
    // the list refreshes from the server every 20 s while shown (new threads; the summaries move on)
    Timer { interval: 20000; repeat: true; running: tp.active && tp.openRoot.length === 0; onTriggered: { tp.tick++; tp.reload() } }

    // ---- the list ----
    ColumnLayout {
        anchors.fill: parent
        visible: tp.openRoot.length === 0
        spacing: 0
        RowLayout {
            Layout.fillWidth: true
            Layout.leftMargin: 10; Layout.rightMargin: 10; Layout.topMargin: 8; Layout.bottomMargin: 6
            spacing: 6
            Accessible.role: Accessible.Grouping
            Accessible.name: "Which threads"
            Controls.Button {
                text: "All threads"
                checkable: true; autoExclusive: true
                checked: tp.filter === "all"
                onClicked: tp.filter = "all"
            }
            Controls.Button {
                text: "My threads"
                checkable: true; autoExclusive: true
                checked: tp.filter === "mine"
                onClicked: tp.filter = "mine"
            }
            Item { Layout.fillWidth: true }
        }
        ListView {
            id: list
            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
            model: tp.order
            spacing: 0
            keyNavigationEnabled: true
            activeFocusOnTab: true
            Accessible.role: Accessible.List
            Accessible.name: "Threads"
            Controls.ScrollBar.vertical: FoxScrollBar { app: tp.app }
            Keys.onPressed: (ev) => {
                if (ev.key === Qt.Key_Home) { list.currentIndex = 0; ev.accepted = true }
                else if (ev.key === Qt.Key_End) { list.currentIndex = list.count - 1; ev.accepted = true }
                else if ((ev.key === Qt.Key_Return || ev.key === Qt.Key_Enter || ev.key === Qt.Key_Space) && list.currentItem) {
                    tp.openThread(list.currentItem.root, list.currentItem); ev.accepted = true
                }
            }
            // "Load more threads" while the server has older ones (Tin Hat Web's list button)
            footer: Controls.ToolButton {
                width: list.width
                visible: tp.listData.more === true
                height: visible ? implicitHeight : 0
                text: "Load more threads"
                onClicked: if (tp.roomCtl) tp.roomCtl.loadMoreThreads()
            }
            delegate: Rectangle {
                id: row
                required property var modelData
                required property int index
                readonly property string root: String(modelData)
                readonly property var it: tp.itemFor(root) || ({})
                readonly property int unread: it.unread !== undefined ? it.unread : 0
                readonly property string sumText: it.count !== undefined && tp.roomCtl ? (tp.tick, tp.roomCtl.threadSummaryText(it.count, it.lastName || "", it.lastTs || 0)) : ""
                width: list.width
                implicitHeight: rowCol.implicitHeight + 16
                color: (rowMa.containsMouse || (list.activeFocus && list.currentIndex === index)) ? Th.membersRowBgHover : "transparent"
                radius: 8
                activeFocusOnTab: false
                Accessible.role: Accessible.Button
                Accessible.name: (it.rootName || "") + ": " + (it.snippet || "") + ". " + sumText + (unread > 0 ? ". Unread replies" : "")
                Accessible.onPressAction: tp.openThread(root, row)
                border.width: activeFocus ? 2 : 0
                border.color: Th.bubbleReactionOwnBorder
                RowLayout {
                    id: rowCol
                    anchors { left: parent.left; right: parent.right; top: parent.top; margins: 8 }
                    spacing: 8
                    Avatar {
                        Layout.alignment: Qt.AlignTop
                        app: tp.app
                        size: 28
                        name: row.it.rootName || "?"
                        seed: row.it.rootSender || ""
                        imageSource: row.it.rootAvatar ? String(row.it.rootAvatar) : ""
                    }
                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: 2
                        Controls.Label {
                            Layout.fillWidth: true
                            textFormat: Text.PlainText
                            text: row.it.rootName || ""
                            font.weight: Font.DemiBold
                            font.pixelSize: Math.round(Kirigami.Theme.smallFont.pixelSize)
                            color: row.unread > 0 ? Th.panelLinkFg : Th.panelFg
                            elide: Text.ElideRight
                        }
                        Controls.Label {
                            Layout.fillWidth: true
                            textFormat: Text.PlainText
                            text: row.it.snippet || ""
                            wrapMode: Text.Wrap
                            maximumLineCount: 2
                            elide: Text.ElideRight
                            color: Th.panelFg
                        }
                        Controls.Label {
                            Layout.fillWidth: true
                            textFormat: Text.PlainText
                            text: row.sumText
                            color: Th.membersMetaFg
                            font.pixelSize: Math.round(Kirigami.Theme.smallFont.pixelSize)
                            elide: Text.ElideRight
                        }
                    }
                    Rectangle {
                        Layout.alignment: Qt.AlignTop
                        Layout.topMargin: 6
                        visible: row.unread > 0
                        implicitWidth: 8; implicitHeight: 8; radius: 4
                        color: Th.bubbleReactionOwnBorder
                    }
                }
                MouseArea {
                    id: rowMa
                    anchors.fill: parent
                    hoverEnabled: true
                    cursorShape: Qt.PointingHandCursor
                    onClicked: { list.currentIndex = row.index; tp.openThread(row.root, row) }
                }
            }
            Controls.Label {
                anchors.centerIn: parent
                width: parent.width - 24
                visible: list.count === 0
                horizontalAlignment: Text.AlignHCenter
                wrapMode: Text.WordWrap
                textFormat: Text.PlainText
                // Tin Hat Web's words (ThreadsPane.tsx): loading (nothing answered yet) / failed / empty
                text: tp._lastJson.length === 0 ? "Loading threads…"
                    : (tp.listData.error === true ? "Couldn't load the threads"
                    : (tp.filter === "mine" ? "You're not in any threads here yet" : "No threads here yet"))
                color: Th.panelHintFg
                font.pixelSize: Math.round(Kirigami.Theme.smallFont.pixelSize)
            }
        }
    }

    // ---- a thread ----
    Item {
        id: threadView
        anchors.fill: parent
        visible: tp.openRoot.length > 0
        Keys.onEscapePressed: (ev) => { tp.closeThread(); ev.accepted = true }
        ColumnLayout {
            anchors.fill: parent
            spacing: 0
            RowLayout {
                Layout.fillWidth: true
                Layout.margins: 6
                Controls.ToolButton {
                    id: backBtn
                    icon.name: "go-previous-symbolic"
                    text: "Threads"
                    display: Controls.AbstractButton.TextBesideIcon
                    Accessible.name: "Back to threads"
                    onClicked: tp.closeThread()
                }
                Item { Layout.fillWidth: true }
            }
            Rectangle { Layout.fillWidth: true; implicitHeight: 1; color: Th.panelHeaderDivider }
            Loader {
                id: threadLoader
                Layout.fillWidth: true
                Layout.fillHeight: true
                focus: true
                active: false
                // a new controller per thread: its open() reads threadRoot (set first by the binding)
                sourceComponent: Component {
                    Item {
                        RoomController { id: threadRoom; threadRoot: tp.openRoot }
                        // review (focus): the thread takes the keyboard — its composer — so Escape reaches the pane
                        Component.onCompleted: { threadRoom.open(tp.roomId); Qt.callLater(function () { if (threadConv.focusComposer) threadConv.focusComposer() }) }
                        readonly property var conv: threadConv
                        readonly property var ctl: threadRoom
                        ConversationView {
                            id: threadConv
                            anchors.fill: parent
                            app: tp.app
                            room: threadRoom
                            gif: tp.gif
                            threadRoot: tp.openRoot
                            onAvatarActivated: (mxid) => tp.avatarActivated(mxid)
                        }
                    }
                }
            }
        }
    }
}
