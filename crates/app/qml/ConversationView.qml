// ConversationView.qml — THE timeline + composer. One component, three hosts: Legacy's room
// page (9.3) and the floating chat bubbles.
//
// 9.3 (the "Buggy chat" report): Legacy's room page used to carry its own copy of this view
// (Main.qml, ~900 lines) — the duplicated-view mistake in waiting: a fix applied to one of two
// duplicated timelines ships as done while the one people use keeps the bug (the
// "doesn't land at the bottom" bug was fixed in the room page only; the bubbles still had it).
// Now every host mounts this file. What changed for the chat itself:
//   • a room switch never shows the previous room's messages, never blanks, rebuilds once;
//     the draft you were typing and where you were scrolled are remembered per room
//   • what you send is on screen instantly (local echo: dimmed → sent; failed → Retry)
//   • the typing notice lives in a fixed-height strip: it can no longer push the timeline
//   • a NEW-messages divider where you left off, and "N new messages" on the jump chip
//   • hover action bar on every message (react · reply · edit · more), Up edits your last
//     message, `/me`, `:emoji:` autocomplete, click a quote to jump to it, staged
//     attachments with a preview before they go, link previews, @mention wash, pins
//
// RECEIPT-SAFETY (CRITICAL): `primary: true` = "this is the only view of this room"; it
// switches on the read marker and the typing notice. A floating bubble leaves it false and
// stays receipt-free. Two primary views of one room ping-pong receipts → notification churn
// → plasmashell FD exhaustion → the desktop crash this file's ancestor warned about. Set it
// true in at most one place per room, ever (bubble-gate.sh checks the defaults).
//
// CXX-Qt note: QML can't reliably bind a controller's property getters, so a short Timer
// mirrors `room.messagesJson`/`roomName`/… into plain QML props the delegates bind to.
import QtQuick
import QtQuick.Window
import QtQuick.Layouts
import QtQuick.Controls as Controls
import QtQuick.Dialogs
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the timeline.* / composer.* / gif.* tokens

Item {
    id: conv
    // 11.0 buttons: "Jump to the latest message" from a button of your own or the Ctrl+K palette (ActionsHost)
    function jumpToLatest() { msgView.jumpToLatest() }
    // The ApplicationWindow-like theme host (Main.qml `root`), for palette/tokens.
    required property var app
    // 10.1 (TH-FLOAT-08): the window that should show this view's lightbox and toasts. A floating
    // chat-head passes itself; the main window (the default) keeps everything in place.
    property var host: null
    readonly property var _hostObj: (host && host.showPassiveNotification) ? host : app
    function _toast(msg, t) { if (conv._hostObj && conv._hostObj.showPassiveNotification) conv._hostObj.showPassiveNotification(LinkGate.plain(msg), t) }
    // A RoomController instance already .open(roomId)'d by the parent (Legacy's shared `room`,
    // or a bubble's own controller).
    required property var room

    readonly property string myId: (app && app.mUserId) ? app.mUserId : ""

    // A shared GifController (gifCtl) passed down from Main.qml. Optional — the GIF button
    // hides when it's unset.
    property var gif: null
    // 10.0.6: since 9.4 the composer's buttons sit at the RIGHT end of the row, so a popup
    // anchored at its button's left edge (a Popup's x defaults to 0) opened past the window's right
    // edge and was clipped there — the GIF grid and the emoji picker showed one column. Popup x is
    // relative to the button: right-align the popup to it, then keep it inside the window.
    // ---- 11.0 buttons: the composer's two rows (composer.start / composer.end), the stored rows or the preset's
    // defaults (today's buttons). The room is the rows' context. Right-click the composer's buttons → "Edit buttons…".
    RowModel {
        id: crowStart
        app: conv.app
        islandRef: "composer"
        rowId: "start"
        context: conv.room && conv.room.roomId.length > 0 ? ({ room: conv.room.roomId }) : null
        bespoke: ["composer.attach"]
    }
    RowModel {
        id: crowEnd
        app: conv.app
        islandRef: "composer"
        rowId: "end"
        context: conv.room && conv.room.roomId.length > 0 ? ({ room: conv.room.roomId }) : null
        bespoke: ["composer.emoji", "composer.gif", "soundboard.open", "room.pinned.open", "composer.send"]
    }
    // right-click a composer row → "Edit buttons…" (the row editor), like the header, the account card and the call bar
    function openRowMenu(rm, title) { composerRowMenu.rowModel = rm; composerRowMenu.rowTitle = title; composerRowMenu.popup() }
    Controls.Menu {
        id: composerRowMenu
        property var rowModel: null
        property string rowTitle: ""
        Controls.MenuItem {
            text: "Edit buttons…"
            icon.name: "document-edit-symbolic"
            onTriggered: if (composerRowMenu.rowModel) composerRowMenu.rowModel.edit(composerRowMenu.rowTitle)
        }
    }
    /** Open one of the composer's popups at a button (today's, or a generic one from the row). */
    function openPopupAt(popup, anchor) {
        popup.parent = anchor
        popup.open()
    }
    function openGifAt(anchor) {
        bubbleGifSearch.text = ""
        conv.openPopupAt(bubbleGifPopup, anchor)
        if (conv.gif) conv.gif.search("")   // trending on open
        bubbleGifSearch.forceActiveFocus()
    }
    /** The composer's own command for its context-only actions run from a generic button (ACTIONS.md §14.1). */
    function composerHandler(a) {
        switch (a) {
        case "composer.attach": return function (c) { attachDialog.open() }
        case "composer.emoji": return function (c) { conv.openPopupAt(bubbleEmojiPicker, c.anchor || bubbleEmojiButton) }
        case "composer.gif": return function (c) { conv.openGifAt(c.anchor || bubbleGifButton) }
        case "room.pinned.open": return function (c) { conv.openPopupAt(pinnedPopup, c.anchor || pinnedButton) }
        case "composer.send": return function (c) { conv.sendComposer() }
        }
        return null
    }

    function popupX(btn, w) {
        var win = conv.Window.window
        var _dep = conv.width + (btn ? btn.x : 0)   // re-evaluate when the view or the button moves
        if (!btn) return 0
        var want = btn.width - w
        if (!win) return want
        var left = btn.mapToItem(null, 0, 0).x
        return Math.max(8 - left, Math.min(want, win.width - w - 8 - left))
    }

    // The ONE view that owns this room's read marker + typing notice (see the header).
    property bool primary: false
    // 11.0 (islands C6/C7): the main window's dock host shows the composer section (the reply / edit bar, the staged
    // files, the message box) in an island of its own — `composerHost` is that island's content item and the section
    // moves there; it stays this view's (drafts, the reply target, staged files, focus). The composer island draws
    // the box (fill, frame — the focus colour through composerFocused — and radius), so the box draws none there.
    // Floating chats leave it null: the composer stays under their timeline, as before.
    property Item composerHost: null
    readonly property bool _inIsland: conv.composerHost !== null
    // 10.8 Flat layout: the composer island is flat, so the box draws its own card again (fill, frame, radius, margins)
    property bool boxCard: false
    readonly property bool _islandBox: conv._inIsland && !conv.boxCard
    readonly property real composerImplicitHeight: composerSection.implicitHeight
    readonly property bool composerFocused: composer.activeFocus
    // 9.3: the soundboard button in the composer (the room page wants it; a
    // floating bubble has no room for it).
    property bool showSoundboard: false
    // 9.3: Legacy hides the whole conversation while a call fills the app.
    property bool hidden: false
    // 10.1 (TL-03 / TH-NOTIF-01): "attending" = someone can actually SEE this view — it is on
    // screen, not tucked behind a maximised call, and its window is the active one. Only an
    // attending primary view sends read receipts, and only an attending view (main window or a
    // bubble) tells the notifier "they can already see it". Alt-tabbed into a game, messages
    // stay unread — badge, NEW line and notifications — until you come back.
    readonly property bool attending: !conv.hidden && conv.visible && conv.Window.active
    // 10.3: the view that READS the room — sends receipts and plays the receive sound. The primary
    // view always; a floating bubble only while the primary view is not showing this room (hidden
    // behind a maximised call, on another room, or in an inactive window). One reader at a time,
    // never none: reading to the bottom of a bubble during a call used to leave the badge climbing.
    readonly property bool reader: conv.primary || !(conv.app && conv.app.conversationView && conv.app.conversationView !== conv
                                                    && conv.app.conversationView.attending && conv.app.conversationView.room && conv.room
                                                    && conv.app.conversationView.room.roomId === conv.room.roomId)
    onAttendingChanged: {
        if (conv.room) conv.room.setAttending(conv.attending)
        if (conv.attending && conv.reader && msgView && msgView.stickBottom && conv.room) conv.room.markRead()
    }
    onReaderChanged: if (conv.reader && conv.attending && msgView && msgView.stickBottom && conv.room) conv.room.markRead()
    Component.onCompleted: if (conv.room) conv.room.setAttending(conv.attending)
    // 9.3.2 gate hook: `--tinhat-scrolllog` logs one "[scroll]" line per wheel/page/bar gesture.
    readonly property bool _scrollLog: !!(app && app._hookArg && app._hookArg("--tinhat-scrolllog") !== null)
    // Emitted when a sender avatar / name is clicked, so a host can show a profile.
    signal avatarActivated(string mxid)
    // A host may want the composer to start focused (opening a chat from a keyboard route).
    function focusComposer() { composer.forceActiveFocus() }
    // 9.3: hosts (the pinned-messages popover, a notification) can jump to a message.
    function jumpTo(eventId) { conv._jumpTo(eventId) }
    // widgets W1: is the message loaded in this view (Main.qml's jumpToEvent waits for it after opening a room)
    function hasEvent(eventId) {
        for (var i = 0; i < msgModel.count; i++) if (msgModel.get(i).eventId === eventId) return true
        return false
    }

    // RESOLVED room display name, threaded in by the host. RoomController.roomName (mirrored as
    // mRoomName) returns the RAW room id for a DM / a room with no m.room.name, so we prefer
    // this resolved name and only fall back to mRoomName — never a raw "!…:server".
    property string displayName: ""
    readonly property string _shownName: {
        if (displayName.length > 0 && displayName.charAt(0) !== "!") return displayName
        var n = mRoomName
        return (n.length > 0 && n.charAt(0) !== "!") ? n : ""
    }

    // Readable sender name: the controller-resolved display name when present, else the
    // LOCALPART of the mxid — never the raw mxid.
    function _name(sn, sender) {
        if (sn !== undefined && sn !== null && String(sn).length > 0) return String(sn)
        var s = String(sender || "")
        if (s.length === 0) return ""
        return s.replace(/^@/, "").split(":")[0]
    }

    // ---- Controller mirror (no direct getter binding; see note above) ----
    property var mMessages: []
    property string _soundTail: ""   // 10.0: newest row id seen — a new REMOTE tail plays the receive sound
    property double _soundTailTs: 0   // 10.3: … and only when it is NEWER (a deleted tail slides back, no chime)
    property string _messagesRaw: ""
    // Side store for the keyed-revision timeline model: row key -> { payload, rawJson }.
    // MUTATED in place, never reassigned mid-room, so the per-row `rev` bump is the ONLY
    // thing that re-evaluates a delegate's payload.
    property var _msgById: ({})
    property string mRoomName: ""
    property bool mBusy: false
    property bool mLoadingOlder: false
    property bool mHasMore: false
    property bool mCanPin: false
    property var mTyping: []
    property string _typingRaw: ""
    property var mMembers: []
    // 10.4 (space tags): mxid -> {name, color} for members who hold a CUSTOM role in the space
    // (never Admin / Moderator / Everyone); ChatBubble colours the sender name with it and shows
    // the tag. Rebuilt with mMembers.
    property var mRoleTags: ({})
    property string _membersRaw: ""
    property var mPinned: []
    property string _pinnedRaw: ""
    readonly property int pinnedCount: mPinned.length

    // ---- compose targets (reply / edit) ----
    property string replyToId: ""
    property string replyToBody: ""
    property string editToId: ""
    property string editToBody: ""
    readonly property bool composing: replyToId.length > 0 || editToId.length > 0

    // ---- 9.3: per-room memory (draft, compose target, scroll place) --------------------------
    // Lives on the host (`app.chatMemory`) so it survives the view being recreated; a bubble
    // without a host keeps its own.
    property var _localMemory: ({})
    readonly property var memory: (app && app.chatMemory) ? app.chatMemory : _localMemory
    property string _roomIdSeen: ""
    // The unread divider: my read receipt at the moment the room came on screen, frozen for
    // as long as it stays there (our own later receipts must not move the line under the
    // reader). "" = none. `unreadDividerId` = the first row after it that isn't mine.
    property string unreadMarker: ""
    property bool _markerFrozen: false
    property string unreadDividerId: ""
    property int _unreadHint: 0
    // 10.1 (TL-01): the newest row at freeze time. Nothing newer than it can ever become the NEW
    // line — the marker used to latch onto the first message that arrived while you were watching.
    property string _frozenTailId: ""
    property bool _dividerSettled: false
    // 10.1 (TL-06): the controller's status line ("Couldn't reach the server…"); "" = fine.
    property string mStatus: ""
    // "N new messages" on the jump chip while the reader is scrolled up.
    property int newBelow: 0
    // A row to flash (jump-to-reply / pinned jump).
    property string flashId: ""
    Timer { id: flashTimer; interval: 1600; onTriggered: conv.flashId = "" }

    function _sameDay(a, b) {
        var da = new Date(a), db = new Date(b)
        return da.getFullYear() === db.getFullYear() && da.getMonth() === db.getMonth() && da.getDate() === db.getDate()
    }
    function _dayLabel(ts) {
        var d = new Date(ts), now = new Date()
        if (_sameDay(ts, now.getTime())) return "Today"
        var y = new Date(now.getTime() - 86400000)
        if (_sameDay(ts, y.getTime())) return "Yesterday"
        return Qt.formatDate(d, "ddd, MMM d")
    }

    // Keyed-revision timeline model. Reassigning a JS-array model resets the ListView — EVERY
    // visible ChatBubble is destroyed/recreated per incoming message and a playing inline
    // embed collapses to its poster. So the ListView is driven by this ListModel with two flat
    // roles only ({eventId, rev}; payloads stay in conv._msgById as plain JS objects), and each
    // poll is DIFFED into it.
    ListModel { id: msgModel }

    function _msgKey(m, i) {
        return (m.id !== undefined && m.id !== null && String(m.id).length > 0)
            ? String(m.id) : ("noid:" + i)
    }

    function _rebuildTimeline(arr) {
        console.log("[timeline] FULL REBUILD, " + msgModel.count + " -> " + arr.length + " rows")
        msgModel.clear()
        conv._msgById = ({})
        var byId = conv._msgById
        for (var i = 0; i < arr.length; i++) {
            var key = conv._msgKey(arr[i], i)
            byId[key] = ({ payload: arr[i], rawJson: JSON.stringify(arr[i]) })
            msgModel.append({ eventId: key, rev: 0 })
        }
    }

    function _resetTimeline() {
        msgModel.clear()
        conv._msgById = ({})
        conv._messagesRaw = ""
        conv.mMessages = []
        conv._soundTail = ""; conv._soundTailTs = 0
        conv._pinnedRaw = ""; conv.mPinned = []
        conv._typingRaw = ""; conv.mTyping = []
        msgView.olderAnchor = -1
        msgView.topIndex = 0
        msgView.stickBottom = true
        msgView._autoFills = 0
        msgView._armSettle()
        openWatch.restart()
        conv.replyToId = ""; conv.replyToBody = ""; conv.editToId = ""; conv.editToBody = ""
        conv.unreadMarker = ""; conv._markerFrozen = false; conv.unreadDividerId = ""
        conv._frozenTailId = ""; conv._dividerSettled = false
        conv.newBelow = 0
        conv.flashId = ""
        conv.staged = []
    }

    // 9.3: the room under this view changed (Legacy re-targets the shared controller; a bubble
    // gets its own). Remember where the old room was, start the new one clean, restore what
    // we remembered for it. The mirror below calls this the moment `room.roomId` differs.
    function _switchRoom(rid) {
        var old = conv._roomIdSeen
        if (old.length > 0) conv._remember(old)
        conv._roomIdSeen = rid
        conv._resetTimeline()
        composer.text = ""
        composer._mentions = []
        var mem = conv.memory[rid]
        if (mem) {
            if (mem.draft) { composer.text = mem.draft; console.log("[chat] draft restored for " + rid) }
            if (mem.replyToId) { conv.replyToId = mem.replyToId; conv.replyToBody = mem.replyToBody || ""; conv.replyToName = mem.replyToName || "" }
            if (mem.staged && mem.staged.length > 0) conv.staged = mem.staged
            composer.cursorPosition = composer.text.length
        }
        // The composer takes focus when a room opens (Discord: you can just start typing).
        if (conv.primary) Qt.callLater(function() { if (conv.visible) composer.forceActiveFocus() })
        // Unread hint from the host's room list (the receipt clears the count the moment we
        // mark read, so it is read once, here).
        conv._unreadHint = 0
        if (conv.app && conv.app.mRooms) {
            var rooms = conv.app.mRooms
            for (var i = 0; i < rooms.length; i++) if (rooms[i].id === rid) { conv._unreadHint = rooms[i].unread || 0; break }
        }
    }
    function _remember(rid) {
        if (!rid || rid.length === 0) return
        var atBottom = msgView.stickBottom || msgView.calcNearBottom()
        var topId = ""
        if (!atBottom) {
            var idx = msgView._rowAtTop()   // 10.5.2 review: _topIndexProbe() always answered -1 here
            if (idx >= 0 && idx < msgModel.count) topId = msgModel.get(idx).eventId
        }
        conv.memory[rid] = ({
            draft: conv.editToId.length > 0 ? "" : composer.text,
            replyToId: conv.replyToId, replyToBody: conv.replyToBody, replyToName: conv.replyToName,
            staged: conv.staged,   // 10.1 (TH-CONV-10): thumbnails survive a room switch like the draft does
            atBottom: atBottom, topId: topId
        })
        if (conv.app && conv.app.persistChatMemory) conv.app.persistChatMemory()   // 10.1 (TH-CONV-22)
    }
    // Different controller object => a fresh view of whatever room it holds.
    onRoomChanged: if (msgModel && msgView && conv.room) conv._switchRoom(conv.room.roomId)

    // Diff one parsed poll into msgModel. An unchanged event keeps its payload INSTANCE so its
    // delegate re-evaluates nothing; only an event whose own JSON changed gets a new payload +
    // a rev bump.
    function _syncTimeline(arr) {
        var byId = conv._msgById
        var hadAnchor = msgView.olderAnchor >= 0
        var c0 = msgModel.count
        var present = ({})
        var i
        for (i = 0; i < arr.length; i++) present[conv._msgKey(arr[i], i)] = true
        for (var r = msgModel.count - 1; r >= 0; r--) {
            var rid = msgModel.get(r).eventId
            if (present[rid] !== true) { msgModel.remove(r); delete byId[rid] }
        }
        var row = 0
        var appended = 0
        for (i = 0; i < arr.length; i++) {
            var key = conv._msgKey(arr[i], i)
            var raw = JSON.stringify(arr[i])
            if (row < msgModel.count && msgModel.get(row).eventId === key) {
                if (byId[key] === undefined || byId[key].rawJson !== raw) {
                    byId[key] = ({ payload: arr[i], rawJson: raw })
                    msgModel.setProperty(row, "rev", msgModel.get(row).rev + 1)
                }
            } else if (byId[key] !== undefined) {
                // Key survived the removal pass but isn't at the walk position: the timeline
                // REORDERED (a local echo swapped for its server row lands here). Rebuild rather
                // than risk duplicate rows from an in-place merge.
                conv._rebuildTimeline(arr)
                if (hadAnchor) msgView.olderAnchor = -1
                return
            } else {
                byId[key] = ({ payload: arr[i], rawJson: raw })
                msgModel.insert(row, { eventId: key, rev: 0 })
                if (row === msgModel.count - 1 && arr[i].sender !== conv.myId && !(arr[i].echo === true)) appended++
            }
            row++
        }
        if (hadAnchor && msgModel.count !== c0) msgView.olderAnchor = -1
        // 9.3: count what arrived below a reader who has scrolled up.
        if (appended > 0 && !msgView.stickBottom && !msgView.settling) conv.newBelow += appended
    }

    // 9.3: where the NEW line goes — computed once the marker is frozen; stable afterwards.
    function _recomputeDivider() {
        if (conv.unreadDividerId.length > 0 || conv._dividerSettled) return
        var arr = conv.mMessages
        if (arr.length === 0) return
        var start = -1
        if (conv.unreadMarker.length > 0) {
            for (var i = 0; i < arr.length; i++) if (arr[i].id === conv.unreadMarker) { start = i; break }
            if (start < 0) {
                // The marker is older than the loaded window (or on a hidden event type). 10.1
                // (TL-12): trust the room list's count instead of calling the whole window new —
                // the divider lands that many rows from the end.
                if (conv._unreadHint <= 0) return
                start = Math.max(-1, arr.length - 1 - conv._unreadHint)
            }
        } else if (conv._unreadHint <= 0) {
            return
        }
        // 10.1 (TL-01): only rows up to the freeze-time tail are candidates.
        var end = arr.length
        if (conv._frozenTailId.length > 0)
            for (var t = arr.length - 1; t >= 0; t--) if (arr[t].id === conv._frozenTailId) { end = t + 1; break }
        for (var j = start + 1; j < end; j++) {
            if (arr[j].sender !== conv.myId && arr[j].echo !== true) { conv.unreadDividerId = arr[j].id; return }
        }
        // The frozen window has been scanned with no candidate: it stays that way for this open.
        if (conv._frozenTailId.length > 0) conv._dividerSettled = true
    }

    // 10.1 (TL-21): PgUp/PgDn/Home/End page the timeline whenever no text field has focus (they
    // used to work only while the composer was focused — click a bubble and they died).
    readonly property bool _noTextFocus: !(conv.Window.activeFocusItem && ("cursorPosition" in conv.Window.activeFocusItem))
    Shortcut { sequence: "PgUp"; enabled: conv.visible && !conv.hidden && conv._noTextFocus; onActivated: msgView.pageBy(-1) }
    Shortcut { sequence: "PgDown"; enabled: conv.visible && !conv.hidden && conv._noTextFocus; onActivated: msgView.pageBy(1) }
    Shortcut { sequence: "End"; enabled: conv.visible && !conv.hidden && conv._noTextFocus; onActivated: msgView.jumpToLatest() }
    Shortcut { sequence: "Home"; enabled: conv.visible && !conv.hidden && conv._noTextFocus; onActivated: { msgView.positionViewAtBeginning(); msgView._maybeLoadOlder() } }
    // 10.1 (TL-17): the shared controller re-targets in place — react to the room id at once
    // instead of on the next 120 ms tick (the header switched, the old messages lingered).
    Connections {
        target: conv.room
        function onRoomIdChanged() { if (conv.room && msgModel && msgView && conv.room.roomId !== conv._roomIdSeen) conv._switchRoom(conv.room.roomId) }
    }
    Timer {
        // 11.0 (GPU.md §2.8, R14): 120 ms while the window is focused, 1 s otherwise (lowgpu interval_for)
        interval: Motion.focused ? 120 : 1000
        // Gated on effective visibility: a bubble minimized to its chat-head keeps this view
        // alive but hidden, and polling/parsing the whole timeline 8x/s for invisible pixels is
        // pure waste while gaming. 10.1 (TH-CONV-17): a call that fills the app hides the
        // conversation too — no parsing behind a 4K60 share. (TH-CONV-09): the first tick runs
        // at once, so the cached timeline paints before the "No messages" card can flash.
        running: conv.visible && !conv.hidden
        repeat: true
        triggeredOnStart: true
        onTriggered: {
            if (!conv.room) return
            var rid = conv.room.roomId
            if (rid !== conv._roomIdSeen) conv._switchRoom(rid)
            conv.mRoomName = conv.room.roomName
            conv.mBusy = conv.room.busy
            conv.mStatus = conv.room.status || ""   // 10.1 (TL-06): a failed load says so
            conv.mLoadingOlder = conv.room.loadingOlder
            conv.mHasMore = conv.room.hasMore
            conv.mCanPin = conv.room.canPin === true
            if (conv.room.typingJson !== conv._typingRaw) {
                conv._typingRaw = conv.room.typingJson
                conv.mTyping = conv._typingRaw.length > 0 ? JSON.parse(conv._typingRaw) : []
            }
            if (conv.room.membersJson !== conv._membersRaw) {
                conv._membersRaw = conv.room.membersJson
                conv.mMembers = conv._membersRaw.length > 0 ? JSON.parse(conv._membersRaw) : []
                var tags = {}
                for (var ti = 0; ti < conv.mMembers.length; ti++) {
                    var tm = conv.mMembers[ti]
                    if (tm && tm.tagName && String(tm.tagName).length > 0)
                        tags[tm.mxid] = { name: String(tm.tagName), color: String(tm.tagColor || "") }
                }
                conv.mRoleTags = tags
            }
            if (conv.room.pinnedJson !== conv._pinnedRaw) {
                conv._pinnedRaw = conv.room.pinnedJson
                try { conv.mPinned = conv._pinnedRaw.length > 0 ? JSON.parse(conv._pinnedRaw) : [] } catch (e) { conv.mPinned = [] }
            }
            if (conv.room.messagesJson !== conv._messagesRaw) {
                conv._messagesRaw = conv.room.messagesJson
                // 10.5.2 (timeline): a string that doesn't parse keeps the rows on screen. JSON.parse used
                // to throw here AFTER _messagesRaw had taken the bad string, so every later tick compared
                // equal and the conversation stayed blank until the next live load replaced it.
                var parsed = null
                try { parsed = conv._messagesRaw.length > 0 ? JSON.parse(conv._messagesRaw) : [] } catch (e) { parsed = null }
                if (!Array.isArray(parsed)) {
                    console.warn("[timeline] unreadable timeline JSON (" + conv._messagesRaw.length + " bytes) — kept what is on screen")
                    parsed = conv.mMessages
                } else parsed = conv._dropIgnored(parsed)
                conv.mMessages = parsed
                // 10.0 receive sound: a NEW newest row from someone else, in the primary view, and
                // not a history load. The first parse after a room open only seeds the tail.
                if (conv.mMessages.length > 0) {
                    var tail = conv.mMessages[conv.mMessages.length - 1]
                    var tailId = tail.id !== undefined ? String(tail.id) : ""
                    var tailTs = Number(tail.ts) || 0
                    // 10.3: a NEWER row only (the tail sliding back after a deletion used to chime), and
                    // the reading view chimes — a bubble too while the main view is hidden by a call.
                    // 10.6 (REACH A5.6, audit 15b): only while the window is ACTIVE, as on web — in the
                    // background the notifier's own "notify" sound covers it (two chimes used to play).
                    if (conv._soundTail.length > 0 && tailId !== conv._soundTail && conv.reader && tailTs > conv._soundTailTs
                        && tail.sender !== conv.myId && tail.echo !== true && msgView.olderAnchor < 0
                        && conv.Window.active
                        && conv.app && conv.app.playUiSound) conv.app.playUiSound("receive")
                    conv._soundTail = tailId; conv._soundTailTs = tailTs
                }
                if (msgModel.count === 0) conv._rebuildTimeline(conv.mMessages)
                else conv._syncTimeline(conv.mMessages)
                if (conv._markerFrozen) conv._recomputeDivider()
            }
            // The marker is final once the first live load has landed (Rust publishes it before
            // that load). Freeze it then, and restore the remembered scroll place.
            if (!conv._markerFrozen && !conv.mBusy && rid.length > 0) {
                conv._markerFrozen = true
                conv.unreadMarker = conv.room.readMarkerId || ""
                conv._frozenTailId = conv.mMessages.length > 0 ? String(conv.mMessages[conv.mMessages.length - 1].id) : ""
                conv._recomputeDivider()
                var mem = conv.memory[rid]
                if (mem && mem.atBottom === false && mem.topId) Qt.callLater(function() { conv._restoreScroll(mem.topId) })
            }
            msgView._fillIfShort()   // 10.5.2
        }
    }
    // 10.5.2 (timeline): field evidence — a conversation that is still empty 6 s after it was opened
    // while there is (or may be) something to show says so in the log, with what the controller was
    // doing, instead of being a silent blank.
    Timer {
        id: openWatch
        interval: 6000
        onTriggered: {
            if (!conv.room || msgModel.count > 0) return
            var st = String(conv.room.status || "")
            if (!(conv.room.busy || conv.room.hasMore || st.length > 0)) return   // a genuinely empty room
            console.warn("[chat] " + conv._roomIdSeen + " still empty 6 s after opening (busy=" + conv.room.busy
                         + " hasMore=" + conv.room.hasMore + " loadingOlder=" + conv.room.loadingOlder
                         + " autoFills=" + msgView._autoFills + " status=\"" + st + "\" visible=" + conv.visible
                         + " hidden=" + conv.hidden + ")")
        }
    }
    function _restoreScroll(topId) {
        for (var i = 0; i < msgModel.count; i++) {
            if (msgModel.get(i).eventId === topId) {
                msgView.settling = false; settleTimer.stop()
                msgView.stickBottom = false
                // 10.5.2 review: keep onCountChanged's prepend re-anchor (topIndex >= 0) off, as it has
                // always been in practice — see _rowAtTop()
                msgView.topIndex = -1
                msgView.positionViewAtIndex(i, ListView.Beginning)
                return
            }
        }
        console.log("[chat] the remembered place " + topId + " is not loaded any more — opening at the latest")
    }
    function _jumpTo(eventId) {
        for (var i = 0; i < msgModel.count; i++) {
            if (msgModel.get(i).eventId === eventId) {
                msgView.settling = false; settleTimer.stop()
                msgView.stickBottom = false
                msgView.positionViewAtIndex(i, ListView.Center)
                conv.flashId = eventId
                flashTimer.restart()
                return
            }
        }
        conv._toast("That message isn't loaded — scroll up to find it", "short")
    }
    // Media chip resolved (this view's own controller) → open externally.
    Connections {
        target: conv.room
        function onMediaOpenedChanged() {
            if (conv.room && conv.room.mediaOpened.length > 0) LinkGate.openMedia(conv.room.mediaOpened)   // security review 2026-10-26
        }
        // launch-gate's web review #8: a text-like attachment is shown in the app as plain text, never opened outside
        // (only here: the main view's controller is AppCore's `room` too, so a second handler there would show it twice)
        function onMediaTextChanged() {
            if (conv.room && conv.room.mediaText.length > 0) convTextViewer.show(conv.room.mediaText)
        }
    }
    TextViewerDialog { id: convTextViewer }
    // Mirror of room.mediaResolved for the inline video players.
    property var mResolvedMedia: ({})
    property string _resolvedRaw: ""
    Connections {
        target: conv.room
        function onMediaResolvedChanged() {
            var r = conv.room ? conv.room.mediaResolved : ""
            if (r === conv._resolvedRaw) return
            conv._resolvedRaw = r
            try { conv.mResolvedMedia = r.length > 0 ? JSON.parse(r) : ({}) } catch (e) { conv.mResolvedMedia = ({}) }
        }
    }

    // account safety (safety.rs): what someone I ignore already sent stays off screen (the server sends no more); the
    // web's useRoomTimeline does the same. A change of the list re-reads the timeline.
    readonly property string _ignoredRaw: (conv.app && conv.app.settings) ? String(conv.app.settings.ignoredJson || "") : ""
    on_IgnoredRawChanged: conv._messagesRaw = ""   // the next poll re-parses (and filters) the timeline
    function _dropIgnored(rows) {
        var ign = []
        try { ign = conv._ignoredRaw.length > 0 ? JSON.parse(conv._ignoredRaw) : [] } catch (e) { ign = [] }
        if (!ign.length) return rows
        var out = []
        for (var i = 0; i < rows.length; i++) if (!rows[i] || ign.indexOf(String(rows[i].sender)) < 0) out.push(rows[i])
        return out
    }

    // ---- message action menu (right-click / long-press / the hover bar's "more") ----------
    property string ctxMsgId: ""
    property string ctxMsgBody: ""
    property bool ctxMsgMine: false
    property bool ctxMsgIsMedia: false
    property string ctxMsgSenderName: ""   // 10.1 (TH-CONV-07): for "Replying to <name>"
    property string ctxMsgSender: ""   // account safety: the sender's id (Report…)
    property bool ctxMsgRedacted: false
    property bool ctxMsgPinned: false
    property bool ctxMsgEcho: false
    property string ctxMsgName: ""
    property bool ctxMsgIsDice: false   // widgets W3: a dice roll is never edited
    property bool ctxMsgIsPoll: false   // widgets W3: nor is a poll

    function copyText(t) {
        if (conv.app && conv.app.copyToClipboard) { conv.app.copyToClipboard(String(t)); return }
        clip.text = String(t); clip.selectAll(); clip.copy(); clip.text = ""
    }
    TextEdit { id: clip; visible: false }

    function askSaveMedia(eventId, suggestedName) {
        conv._saveId = eventId
        saveMediaDialog.selectedFile = saveMediaDialog.currentFolder + "/" + suggestedName
        saveMediaDialog.open()
    }
    property string _saveId: ""
    FileDialog {
        id: saveMediaDialog
        title: "Save attachment"
        fileMode: FileDialog.SaveFile
        onAccepted: if (conv._saveId.length > 0) conv.room.saveMediaAs(conv._saveId, String(selectedFile))
    }
    FileDialog {
        id: attachDialog
        title: "Send a file or image"
        fileMode: FileDialog.OpenFiles
        onAccepted: { for (var i = 0; i < selectedFiles.length; i++) conv.stageFile(String(selectedFiles[i])) }
    }

    function _openMenuFor(md, mine) {
        var mt = md.msgtype !== undefined ? md.msgtype : "m.text"
        conv.ctxMsgId = md.id !== undefined ? String(md.id) : ""
        conv.ctxMsgBody = md.body !== undefined ? String(md.body) : ""
        conv.ctxMsgMine = mine
        conv.ctxMsgIsMedia = mt === "m.image" || mt === "m.video" || mt === "m.file" || mt === "m.audio"
        conv.ctxMsgName = (md.body !== undefined && String(md.body).length > 0) ? String(md.body) : "file"
        conv.ctxMsgRedacted = md.redacted === true || md.undecryptable === true   // 10.1: nothing to reply to / react to on a locked row
        conv.ctxMsgSenderName = md.senderName !== undefined ? String(md.senderName) : ""   // 10.1 (TH-CONV-07)
        conv.ctxMsgSender = md.sender !== undefined ? String(md.sender) : ""   // account safety
        conv.ctxMsgPinned = md.pinned === true
        conv.ctxMsgEcho = md.echo === true
        conv.ctxMsgIsDice = md.dice !== undefined && md.dice !== null && md.redacted !== true
        conv.ctxMsgIsPoll = md.poll !== undefined && md.poll !== null && md.redacted !== true
        if (conv.ctxMsgId.length > 0) msgMenu.popup()
    }
    // 10.1 (TH-CONV-07): the bar names WHO you are replying to (three people said "yeah").
    property string replyToName: ""
    function startReply(id, body, name) {
        conv.editToId = ""; conv.editToBody = ""
        conv.replyToId = id
        conv.replyToBody = body
        conv.replyToName = (name !== undefined && name !== null) ? String(name) : ""
        composer.forceActiveFocus()
    }
    function startEdit(id, body) {
        conv.replyToId = ""; conv.replyToBody = ""
        conv.editToBody = body
        conv.editToId = id
    }
    Controls.Menu {
        id: msgMenu
        Controls.Menu {
            id: reactSub
            title: "React"
            enabled: !conv.ctxMsgRedacted && !conv.ctxMsgEcho
            Repeater {
                model: ["👍", "❤️", "😂", "🔥", "🎉", "🦊"]
                delegate: Controls.MenuItem {
                    required property var modelData
                    text: LinkGate.plain(modelData)
                    onTriggered: conv.room.react(conv.ctxMsgId, modelData)
                }
            }
            Controls.MenuSeparator {}
            Controls.MenuItem {
                text: "More…"
                icon.name: "list-add-symbolic"
                onTriggered: reactPicker.open()
            }
        }
        Controls.MenuItem {
            text: "Reply"
            icon.name: "mail-reply-sender-symbolic"
            visible: !conv.ctxMsgRedacted && !conv.ctxMsgEcho
            height: visible ? implicitHeight : 0
            onTriggered: conv.startReply(conv.ctxMsgId, conv.ctxMsgBody, conv.ctxMsgSenderName)
        }
        Controls.MenuItem {
            text: "Edit"
            icon.name: "document-edit-symbolic"
            // 10.1 (TH-MSG-01): never on media — an edit sends m.text, which REPLACES the attachment.
            visible: conv.ctxMsgMine && !conv.ctxMsgRedacted && !conv.ctxMsgEcho && !conv.ctxMsgIsMedia && !conv.ctxMsgIsDice && !conv.ctxMsgIsPoll
            height: visible ? implicitHeight : 0
            onTriggered: conv.startEdit(conv.ctxMsgId, conv.ctxMsgBody)
        }
        Controls.MenuItem {
            text: conv.ctxMsgPinned ? "Unpin" : "Pin message"
            icon.name: "window-pin-symbolic"   // 10.1 (TH-ICON-01): "pin-symbolic" is not in the pack
            visible: conv.mCanPin && !conv.ctxMsgRedacted && !conv.ctxMsgEcho
            height: visible ? implicitHeight : 0
            onTriggered: conv.room.pinMessage(conv.ctxMsgId, !conv.ctxMsgPinned)
        }
        Controls.MenuItem {
            text: "Save as…"
            icon.name: "document-save-symbolic"
            visible: conv.ctxMsgIsMedia && !conv.ctxMsgRedacted && !conv.ctxMsgEcho
            height: visible ? implicitHeight : 0
            onTriggered: conv.askSaveMedia(conv.ctxMsgId, conv.ctxMsgName)
        }
        Controls.MenuItem {
            text: "Copy text"
            icon.name: "edit-copy-symbolic"
            visible: !conv.ctxMsgRedacted
            height: visible ? implicitHeight : 0
            onTriggered: conv.copyText(conv.ctxMsgBody)
        }
        Controls.MenuItem {
            text: "Copy link"
            icon.name: "link-symbolic"
            visible: !conv.ctxMsgEcho
            height: visible ? implicitHeight : 0
            onTriggered: {
                conv.copyText(conv.app.messageLink(conv.room.roomId, conv.ctxMsgId))   // 12.0.6: share_link.rs
                conv._toast("Message link copied", "short")
            }
        }
        Controls.MenuSeparator {}
        Controls.MenuItem {
            text: "Delete"
            icon.name: "delete-symbolic"
            visible: conv.ctxMsgMine && !conv.ctxMsgRedacted && !conv.ctxMsgEcho
            height: visible ? implicitHeight : 0
            onTriggered: msgDeleteConfirm.open()
        }
        // account safety (safety.rs): someone else's message → Report… (the web's message-menu entry, after Delete)
        Controls.MenuSeparator {
            visible: reportMsgItem.visible
            height: visible ? implicitHeight : 0
        }
        Controls.MenuItem {
            id: reportMsgItem
            text: LinkGate.plain(conv.app && conv.app.safety ? String(conv.app.safety.w.report) : "")
            icon.name: "dialog-warning"
            visible: !conv.ctxMsgMine && !conv.ctxMsgEcho && conv.ctxMsgSender.length > 0 && conv.ctxMsgId.length > 0
                     && conv.ctxMsgId.charAt(0) === "$" && !!conv.app && !!conv.app.safety
            height: visible ? implicitHeight : 0
            onTriggered: conv.app.safety.openReport(conv.ctxMsgSender, conv.room.roomId, conv.ctxMsgId, conv.ctxMsgSenderName)
        }
    }
    EmojiPicker {
        id: reactPicker
        app: conv.app
        parent: conv
        x: Math.round((conv.width - width) / 2)
        y: Math.round((conv.height - height) / 2)
        onPicked: (emoji) => { if (conv.ctxMsgId.length > 0) conv.room.react(conv.ctxMsgId, emoji) }
    }
    ConfirmDialog {
        id: msgDeleteConfirm
        app: conv.app
        dialogTitle: "Delete message?"
        confirmText: "Delete"
        destructive: true
        message: "Delete this message for everyone?"
        onConfirmed: conv.room.redactMessage(conv.ctxMsgId)
    }

    // ---- 9.3: staged attachments (preview before they go) ------------------------------------
    // Drop / pick → a strip above the composer with a thumbnail (or a file chip) and an ✕;
    // Send sends the text and then each file (each shows up at once as a local echo).
    property var staged: []
    function stageFile(url) {
        if (!url || url.length === 0) return
        var name = decodeURIComponent(String(url).replace(/^.*\//, ""))
        var lower = name.toLowerCase()
        var isImage = /\.(png|jpe?g|gif|webp|bmp|avif)$/.test(lower)
        var arr = conv.staged.slice()
        for (var i = 0; i < arr.length; i++) if (arr[i].url === url) return
        arr.push({ url: String(url), name: name, isImage: isImage })
        conv.staged = arr
        composer.forceActiveFocus()
    }
    function unstage(i) { var arr = conv.staged.slice(); arr.splice(i, 1); conv.staged = arr }
    // 9.3.2: Ctrl+V with an image (a screenshot, a browser's "Copy image") or files copied in a
    // file manager stages them like a drop — no dragging needed. Returns false for a text-only
    // clipboard so the TextArea's own paste runs. Rust saves the image as a PNG first.
    function pasteAttachments() {
        if (!conv.room || conv.room.roomId.length === 0 || !conv.room.clipboardAttachments) return false
        var urls = []
        try { urls = JSON.parse(conv.room.clipboardAttachments()) } catch (e) { urls = [] }
        if (!urls || urls.length === 0) return false
        for (var i = 0; i < urls.length; i++) conv.stageFile(String(urls[i]))
        console.log("[chat] pasted " + urls.length + " attachment(s) from the clipboard")
        return true
    }
    DropArea {
        anchors.fill: parent
        z: 40
        keys: ["text/uri-list"]   // 10.0.1: files only — never the Spaces rail's reorder drag
        onDropped: (drop) => {
            if (!conv.room || conv.room.roomId.length === 0) return
            if (drop.hasUrls && drop.urls.length > 0) {
                for (var i = 0; i < drop.urls.length; i++) conv.stageFile(String(drop.urls[i]))
                drop.accept()
            }
        }
        Rectangle {
            anchors.fill: parent
            visible: parent.containsDrag
            color: Th.dropInlineBg
            radius: 8
            border.width: 2
            border.color: Th.dropInlineBorder
            Controls.Label {
                anchors.centerIn: parent
                text: "Drop to attach — you can add a message before it goes"
                color: Th.dropInlineFg
                font.bold: true
            }
        }
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: 0
        visible: !conv.hidden

        // ---- Timeline ----
        ListView {
            id: msgView
            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
            model: msgModel
            spacing: Kirigami.Units.smallSpacing
            leftMargin: Kirigami.Units.smallSpacing
            rightMargin: Kirigami.Units.smallSpacing + 12   // room for the scroll bar
            // Big rooms: keep delegates around while scrolling back and forth.
            cacheBuffer: 600

            // 9.3.2: scrolling with energy. Qt's Flickable turns every wheel notch into a flick
            // IMPULSE (velocity + 1500 px/s² deceleration) — "trekking through mud" — and the raw
            // ListView lost Kirigami's ScrollablePage wheel handling and scroll bar when the room
            // page became a plain Page in 9.3.0. Chromium/Discord move a fixed distance per notch
            // with a short ease-out; touchpads (pixelDelta) track the fingers 1:1; the ends stop
            // dead instead of rubber-banding. The house scroll bar (FoxScrollBar) is always there
            // when there is something to scroll.
            boundsBehavior: Flickable.StopAtBounds
            // 10.0.3: the bar shows only while scrolling or while the pointer is on the
            // right edge (was AlwaysOn since 9.3.2), and the delegates stop short of its gutter.
            Controls.ScrollBar.vertical: FoxScrollBar {
                app: conv.app
                policy: Controls.ScrollBar.AsNeeded
                idleAlpha: 0.6
                track: true
                edgeHovered: edgeHover.hovered
                onPressedChanged: if (!pressed) msgView._afterScroll()
            }
            // The right-edge hover strip that reveals the bar (hover only — clicks and the wheel pass through).
            Item {
                anchors.right: parent.right; anchors.top: parent.top; anchors.bottom: parent.bottom
                width: 28
                HoverHandler { id: edgeHover }
            }
            // Width the delegates leave free on the right for the bar (thumb 10 + padding + breathing room).
            readonly property int barGutter: 16
            readonly property real wheelStep: Kirigami.Units.gridUnit * 6   // px per notch (≈ 3 chat lines)
            property real wheelGoal: 0
            property real wheelRun: 0   // px asked for in the current run (the gate reads it)
            NumberAnimation {
                id: wheelAnim
                target: msgView; property: "contentY"
                duration: Motion.durSlow; easing.type: Easing.OutCubic
                onFinished: msgView._afterScroll()
            }
            function _clampY(y) { return Math.max(originY, Math.min(y, originY + Math.max(0, contentHeight - height))) }
            // Scroll by `dy` px: animated (wheel notches, PgUp/PgDn — successive notches extend
            // the same run, so five quick clicks glide five steps) or immediate (touchpad).
            function scrollBy(dy, animate) {
                settling = false; settleTimer.stop()
                var from = wheelAnim.running ? wheelGoal : contentY
                if (!wheelAnim.running) wheelRun = 0
                wheelRun += dy
                var goal = _clampY(from + dy)
                wheelAnim.stop()
                if (!animate || Math.abs(goal - contentY) < 2) { contentY = goal; _afterScroll(); return }
                wheelGoal = goal
                wheelAnim.from = contentY; wheelAnim.to = goal; wheelAnim.start()
            }
            // What onMovementEnded does for drags/flicks, for programmatic moves (wheel, keys,
            // the scroll bar): re-derive the bottom pin, remember the top row, mark read at the
            // end, pull older history at the top.
            // 10.1 (TL-16): the point 2 px under the top edge can land in the gap BETWEEN rows
            // (indexAt = -1, "no row remembered"); probe a little further down before giving up.
            function _topIndexProbe() {
                var i = indexAt(contentX, contentY + 2)
                if (i < 0) i = indexAt(contentX, contentY + 2 + Kirigami.Units.smallSpacing)
                if (i < 0) i = indexAt(contentX, contentY + 14)
                return i
            }
            // 10.5.2 review (fix/timeline verification): the row under the top edge, probed INSIDE the
            // rows. _topIndexProbe() probes at x = contentX, which is the leftMargin gutter (a ListView
            // with leftMargin rests at contentX = -leftMargin): indexAt() answered -1 for every row, so
            // _remember() never stored a place and a room you had scrolled up in always reopened at the
            // latest (qmltestrunner, leftMargin 4: indexAt(contentX, contentY + 2) = -1,
            // indexAt(contentX + width / 2, contentY + 2) = 50). Only _remember() uses this: the other
            // _topIndexProbe() callers (topIndex's prepend re-anchor, the older-page anchor) have only
            // ever run with -1 and are left as they are. Walks down past what isn't a row (the "start of
            // the conversation" header at the very top, the spacing between rows) to the first row.
            function _rowAtTop() {
                var x = contentX + width / 2
                for (var dy = 2; dy < height; dy += 8) {
                    var i = indexAt(x, contentY + dy)
                    if (i >= 0) return i
                }
                return -1
            }
            // 10.1 (TL-14): pull the next page while there is still a screen and a half above the
            // reader, anchored on the row under the top edge (+ its sub-row offset) so the page
            // lands without a lurch. Was: only at the very top, anchored on the first row.
            function _maybeLoadOlder() {
                if (!(conv.mHasMore && !conv.mLoadingOlder && conv.room)) return
                if ((contentY - originY) >= height * 1.5) return
                var ti = _topIndexProbe()
                if (ti < 0) ti = 0
                var it = itemAtIndex(ti)
                olderAnchorOff = it ? (contentY - it.y) : 0
                olderAnchor = count - ti
                conv.room.loadOlder()
            }
            property real olderAnchorOff: 0
            // 10.5.2 (timeline): a window that doesn't fill the view pulls the next page by itself.
            // Only scroll gestures ever loaded older history, so a room whose newest events are mostly
            // call media-key traffic came back with a handful of rows — or none, drawn as "No
            // messages" — with nothing on screen to scroll. Waits for the view to settle; at most 8
            // pages per open (each page is one budgeted pass in Rust).
            // A view with NO rows keeps going (every page moves further back; 60 pages ≈ 54,000 events)
            // and shows the spinner meanwhile; a page that failed stops it (status set — a scroll retries).
            property int _autoFills: 0
            readonly property int _fillCap: count > 0 ? 8 : 60
            readonly property bool fillingEmpty: count === 0 && conv.mHasMore && conv.mStatus.length === 0 && _autoFills < 60
            function _fillIfShort() {
                if (!conv.room || conv.hidden || !conv.visible) return
                if (conv.mBusy || !conv.mHasMore || conv.mLoadingOlder || settling || olderAnchor >= 0) return
                if (conv.mStatus.length > 0) return
                if (count > 0 && contentHeight > height) return
                if (_autoFills >= _fillCap) return
                _autoFills++
                console.log("[timeline] " + count + " rows don't fill the view — loading older history (" + _autoFills + "/" + _fillCap + ")")
                conv.room.loadOlder()
            }
            function _afterScroll() {
                stickBottom = calcNearBottom()
                topIndex = _topIndexProbe()
                if (conv.reader && conv.attending && atYEnd && conv.room) conv.room.markRead()
                _maybeLoadOlder()
                if (conv._scrollLog) console.log("[scroll] y=" + Math.round(contentY - originY) + "/" + Math.round(Math.max(0, contentHeight - height))
                                                 + " run=" + Math.round(wheelRun) + " atEnd=" + atYEnd + " atTop=" + atYBeginning)
            }
            WheelHandler {
                acceptedDevices: PointerDevice.Mouse | PointerDevice.TouchPad
                onWheel: (ev) => {
                    if (ev.pixelDelta.y !== 0) msgView.scrollBy(-ev.pixelDelta.y, false)
                    else if (ev.angleDelta.y !== 0) msgView.scrollBy(-(ev.angleDelta.y / 120) * msgView.wheelStep, true)
                }
            }

            property bool stickBottom: true
            readonly property real bottomSlack: Kirigami.Units.gridUnit * 2
            // A FUNCTION, deliberately, not a bound property: a binding on contentY is not
            // guaranteed to have re-evaluated by the time onContentYChanged runs.
            function calcNearBottom() {
                return contentHeight <= height
                    || (contentY - originY) >= contentHeight - height - bottomSlack   // originY-relative
            }
            onContentYChanged: if (!settling) stickBottom = calcNearBottom()
            onStickBottomChanged: if (stickBottom) conv.newBelow = 0
            property int olderAnchor: -1
            property int topIndex: 0
            // Root cause B: a load-older round-trip that adds ZERO rows never hits the
            // anchor consume; on the falling edge of loadingOlder clear whatever survived.
            onOlderAnchorChanged: {}
            Connections {
                target: conv
                function onMLoadingOlderChanged() {
                    if (!conv.mLoadingOlder && msgView.olderAnchor >= 0)
                        Qt.callLater(function() {
                            if (!conv.mLoadingOlder && msgView.olderAnchor >= 0) {
                                msgView.olderAnchor = -1
                                msgView.stickBottom = msgView.calcNearBottom()
                            }
                        })
                }
            }
            // 10.1 (TL-04): rows appended at the tail move nothing above the reader — only a
            // change of the FIRST row (a prepend / rebuild) needs the top row re-anchored.
            property string _firstId: ""
            onCountChanged: {
                var first = count > 0 ? msgModel.get(0).eventId : ""
                var prepended = _firstId.length > 0 && first !== _firstId
                _firstId = first
                if (olderAnchor >= 0) {
                    wheelAnim.stop()
                    positionViewAtIndex(Math.max(0, count - olderAnchor), ListView.Beginning)
                    if (olderAnchorOff > 0) contentY += olderAnchorOff
                } else if (stickBottom) {
                    positionViewAtEnd()
                    // One receipt per sync BATCH, not per row (deferred + deduped).
                    if (conv.reader && conv.attending && !_markReadQueued) { _markReadQueued = true; Qt.callLater(msgView._markReadAtBottom) }
                } else if (prepended && topIndex >= 0 && topIndex < count) {
                    positionViewAtIndex(topIndex, ListView.Beginning)
                }
            }
            property bool _markReadQueued: false
            function _markReadAtBottom() { _markReadQueued = false; if (conv.reader && conv.attending && conv.room) conv.room.markRead() }
            onMovementEnded: {
                stickBottom = calcNearBottom()
                topIndex = _topIndexProbe()
                if (conv.reader && conv.attending && atYEnd && conv.room) conv.room.markRead()
                _maybeLoadOlder()
            }
            // After a room switch the delegates size asynchronously (images, embeds) and
            // a growth can tick contentY before contentHeight; onContentYChanged then read "not
            // near the bottom" and latched stickBottom OFF. `settling` holds the pin through
            // that window: armed on a room switch, re-armed by every content growth, released
            // 900 ms after the last one (4 s hard cap) — or the moment the user scrolls.
            property bool settling: false
            property double _settleStart: 0
            function _armSettle() { settling = true; _settleStart = Date.now(); settleTimer.restart() }
            Timer { id: settleTimer; interval: 900; onTriggered: msgView.settling = false }
            onMovingChanged: if (moving) { settling = false; settleTimer.stop() }
            onDraggingChanged: if (dragging) { settling = false; settleTimer.stop() }
            function pinBottom() { if ((stickBottom || settling) && olderAnchor < 0) positionViewAtEnd() }
            onContentHeightChanged: {
                if (settling) {
                    if (Date.now() - _settleStart > 4000) settling = false
                    else settleTimer.restart()
                }
                pinBottom()
            }
            onHeightChanged: pinBottom()
            // Wheel-free keyboard scrolling from the composer (PageUp/PageDown, Esc → bottom).
            function pageBy(dir) { scrollBy(dir * height * 0.85, true) }
            function jumpToLatest() {
                wheelAnim.stop()
                stickBottom = true
                positionViewAtEnd()
                conv.newBelow = 0
                if (conv.reader && conv.room) conv.room.markRead()
            }

            // 10.1 (TL-14): the start of the conversation is a stable header row — hitting the
            // beginning used to look exactly like a load that stopped working.
            header: Item {
                width: msgView.width
                height: (!conv.mHasMore && msgView.count > 0) ? Kirigami.Units.gridUnit * 2.4 : 0
                visible: height > 0
                Controls.Label {
                    anchors.centerIn: parent
                    text: "This is the start of the conversation"
                    color: Th.timelineStartFg
                    font.pointSize: Kirigami.Theme.smallFont.pointSize
                }
            }
            // The history spinner floats over the viewport instead of inserting itself as a
            // header (which shoved the content down ~36 px on every page).
            ThBusyIndicator {   // 11.0 (GPU.md §2.5): still
                parent: msgView
                z: 20
                anchors.top: parent.top
                anchors.horizontalCenter: parent.horizontalCenter
                anchors.topMargin: Kirigami.Units.smallSpacing
                running: conv.mLoadingOlder
                visible: running
                size: 27
            }

            // Jump-to-latest chip (pinned to the viewport). 9.3: says how many arrived below.
            Rectangle {
                parent: msgView
                z: 20
                anchors.right: parent.right
                anchors.bottom: parent.bottom
                anchors.rightMargin: Kirigami.Units.largeSpacing
                anchors.bottomMargin: Kirigami.Units.largeSpacing
                visible: msgView.count > 0 && !msgView.atYEnd && !msgView.stickBottom
                implicitWidth: conv.newBelow > 0 ? jumpRow.implicitWidth + 22 : 36
                implicitHeight: 36
                radius: 18
                color: conv.newBelow > 0 ? Th.timelineJumpBgUnread : Th.timelineJumpBg
                border.width: 1
                border.color: conv.newBelow > 0 ? Th.timelineJumpBorderUnread : Th.timelineJumpBorder
                Row {
                    id: jumpRow
                    anchors.centerIn: parent
                    spacing: 6
                    Controls.Label {
                        textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                        anchors.verticalCenter: parent.verticalCenter
                        visible: conv.newBelow > 0
                        text: conv.newBelow + (conv.newBelow === 1 ? " new message" : " new messages")
                        color: Th.timelineJumpFgUnread
                        font.bold: true
                        font.pointSize: Kirigami.Theme.smallFont.pointSize
                    }
                    ThIcon {   // 10.1 (TL-20): a pack glyph, not U+2913 (no glyph in the Windows UI font)
                        anchors.verticalCenter: parent.verticalCenter
                        width: 16; height: 16
                        app: conv.app
                        name: "go-bottom-symbolic"
                        color: conv.newBelow > 0 ? Th.timelineJumpIconUnread : Th.timelineJumpIcon
                        isMask: true
                    }
                }
                MouseArea {
                    id: jumpMa
                    anchors.fill: parent
                    hoverEnabled: true
                    cursorShape: Qt.PointingHandCursor
                    onClicked: msgView.jumpToLatest()
                }
                Controls.ToolTip.text: conv.newBelow > 0 ? "Jump to the new messages" : "Jump to the latest message"
                Controls.ToolTip.visible: jumpMa.containsMouse
                Controls.ToolTip.delay: 500
            }

            delegate: Item {
                id: msgDelegate
                required property string eventId
                required property int rev
                required property int index
                readonly property var modelData: (rev, conv._msgById[eventId] ? conv._msgById[eventId].payload : ({}))
                width: ListView.view ? ListView.view.width - ListView.view.barGutter : 0   // 10.0.3: clear the scroll bar
                implicitHeight: col.implicitHeight

                readonly property bool mine: modelData.sender === conv.myId
                readonly property var prev: (index > 0 && index - 1 < conv.mMessages.length) ? conv.mMessages[index - 1] : null
                readonly property bool newDay: !prev || !conv._sameDay(prev.ts, modelData.ts)
                readonly property bool isDivider: conv.unreadDividerId.length > 0 && conv.unreadDividerId === eventId
                // Group under the previous message: same sender, same day, within 7 minutes
                // (Discord's grouping window), and never across the NEW line.
                readonly property bool grouped: prev && !newDay && prev.sender === modelData.sender
                                                && !isDivider && (modelData.ts - prev.ts) < 7 * 60 * 1000
                readonly property real maxBubble: width * 0.74

                Column {
                    id: col
                    width: parent.width
                    spacing: 2
                    topPadding: msgDelegate.grouped ? 1 : Kirigami.Units.smallSpacing

                    // Date separator at day boundaries.
                    Item {
                        width: parent.width
                        visible: msgDelegate.newDay
                        height: visible ? daypill.implicitHeight + Kirigami.Units.smallSpacing : 0
                        Controls.Label {
                            textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                            id: daypill
                            anchors.centerIn: parent
                            text: conv._dayLabel(msgDelegate.modelData.ts)
                            font.pointSize: Kirigami.Theme.smallFont.pointSize
                            color: Th.timelineDayPillFg
                            leftPadding: 10; rightPadding: 10; topPadding: 2; bottomPadding: 2
                            background: Rectangle { color: Th.timelineDayPillBg; radius: height / 2 }
                        }
                    }

                    // 9.3: the NEW line — where you left off.
                    Item {
                        width: parent.width
                        visible: msgDelegate.isDivider
                        height: visible ? 18 : 0
                        Rectangle {
                            anchors.left: parent.left; anchors.right: newTag.left
                            anchors.rightMargin: 6
                            anchors.verticalCenter: parent.verticalCenter
                            height: 1
                            // the token's alpha stays the item's opacity (docs/MIGRATING-THEME.md §3.8)
                            color: Qt.alpha(Th.timelineNewLineDivider, 1)
                            opacity: Th.timelineNewLineDivider.a
                        }
                        Controls.Label {
                            id: newTag
                            anchors.right: parent.right
                            anchors.rightMargin: 4
                            anchors.verticalCenter: parent.verticalCenter
                            text: "NEW"
                            font.bold: true
                            font.pointSize: Kirigami.Theme.smallFont.pointSize * 0.85
                            color: Th.timelineNewLineFg
                        }
                    }

                    ChatBubble {
                        app: conv.app
                        width: parent.width
                        // 11.0 (GPU.md §2.6): in the viewport (not only in ListView's cache) — GIFs play only then
                        inView: msgDelegate.y + msgDelegate.height > msgView.contentY && msgDelegate.y < msgView.contentY + msgView.height
                        mine: msgDelegate.mine
                        grouped: msgDelegate.grouped
                        body: msgDelegate.modelData.body !== undefined ? msgDelegate.modelData.body : ""
                        sender: msgDelegate.modelData.sender !== undefined ? msgDelegate.modelData.sender : ""
                        senderName: conv._name(msgDelegate.modelData.senderName, msgDelegate.modelData.sender)
                        roleName: (conv.mRoleTags[msgDelegate.modelData.sender] || {}).name || ""     // 10.4 space tags
                        roleColor: (conv.mRoleTags[msgDelegate.modelData.sender] || {}).color || ""
                        formatted: msgDelegate.modelData.formatted !== undefined ? msgDelegate.modelData.formatted : ""
                        msgtype: msgDelegate.modelData.msgtype !== undefined ? msgDelegate.modelData.msgtype : "m.text"
                        media: msgDelegate.modelData.media !== undefined ? msgDelegate.modelData.media : ""
                        mediaBusy: conv.room ? String(conv.room.mediaBusy || "") : ""   // 10.1 (TH-MSG-13)
                        mimetype: msgDelegate.modelData.mimetype !== undefined ? msgDelegate.modelData.mimetype : ""
                        canOpen: msgDelegate.modelData.canOpen === true   // security review 2026-10-26 (viewer.rs)
                        durationMs: msgDelegate.modelData.durationMs !== undefined ? msgDelegate.modelData.durationMs : 0
                        fileSize: msgDelegate.modelData.size !== undefined ? msgDelegate.modelData.size : 0
                        imageW: msgDelegate.modelData.w !== undefined ? msgDelegate.modelData.w : 0
                        imageH: msgDelegate.modelData.h !== undefined ? msgDelegate.modelData.h : 0
                        onImageClicked: (url, w, h, alt) => {
                            var h_ = (conv.host && conv.host.openLightbox) ? conv.host : conv.app   // 10.1 (TH-FLOAT-08)
                            if (h_ && h_.openLightbox) h_.openLightbox(url, w, h, alt)
                        }
                        onMediaClicked: (id) => conv.room.openMedia(id)
                        onSaveRequested: (eventId, suggestedName) => conv.askSaveMedia(eventId, suggestedName)
                        onVideoPlayRequested: (id) => conv.room.resolveMedia(id)
                        resolvedMedia: conv.mResolvedMedia
                        embeds: msgDelegate.modelData.embeds !== undefined ? msgDelegate.modelData.embeds : []
                        onlyEmbed: msgDelegate.modelData.onlyEmbed === true
                        preview: msgDelegate.modelData.preview !== undefined ? msgDelegate.modelData.preview : null
                        calContainer: msgDelegate.modelData.calContainer !== undefined ? msgDelegate.modelData.calContainer : ""
                        calEvent: msgDelegate.modelData.calEvent !== undefined ? msgDelegate.modelData.calEvent : ""
                        themeShare: msgDelegate.modelData.themeShare !== undefined ? msgDelegate.modelData.themeShare : null
                        dice: msgDelegate.modelData.dice !== undefined ? msgDelegate.modelData.dice : null   // widgets W3
                        poll: msgDelegate.modelData.poll !== undefined ? msgDelegate.modelData.poll : null   // widgets W3
                        onPollVote: (a) => conv.room.votePoll(msgDelegate.modelData.id, JSON.stringify(a))
                        onPollEnd: conv.room.endPoll(msgDelegate.modelData.id)
                        onCalendarClicked: (c, e) => { if (conv.app && conv.app.openCalendarEvent) conv.app.openCalendarEvent(c, e) }
                        onCalendarRsvp: (c, e, s) => { if (conv.app && conv.app.calendarRsvp) conv.app.calendarRsvp(c, e, s) }
                        reactions: msgDelegate.modelData.reactions !== undefined ? msgDelegate.modelData.reactions : []
                        receipts: msgDelegate.modelData.receipts !== undefined ? msgDelegate.modelData.receipts : []
                        replyToId: msgDelegate.modelData.replyTo !== undefined ? msgDelegate.modelData.replyTo : ""
                        replyToSender: msgDelegate.modelData.replyToSender !== undefined ? msgDelegate.modelData.replyToSender : ""
                        replyToBody: msgDelegate.modelData.replyToBody !== undefined ? msgDelegate.modelData.replyToBody : ""
                        edited: msgDelegate.modelData.edited === true
                        redacted: msgDelegate.modelData.redacted === true
                        undecryptable: msgDelegate.modelData.undecryptable === true
                        hostView: msgView   // 10.1 (TH-MSG-21): so the hover bar knows when it is at the top
                        pending: msgDelegate.modelData.pending === true
                        txn: msgDelegate.modelData.txn !== undefined ? String(msgDelegate.modelData.txn) : ""
                        failed: msgDelegate.modelData.failed === true
                        sendError: msgDelegate.modelData.error !== undefined ? msgDelegate.modelData.error : ""
                        mentionsMe: msgDelegate.modelData.mentionsMe === true
                        pinned: msgDelegate.modelData.pinned === true
                        flash: conv.flashId.length > 0 && conv.flashId === msgDelegate.eventId
                        canPin: conv.mCanPin
                        eventId: msgDelegate.modelData.id !== undefined ? msgDelegate.modelData.id : ""
                        onReact: (key) => conv.room.react(msgDelegate.modelData.id, key)
                        onUnreact: (reactionId) => conv.room.unreact(reactionId)
                        onMenuRequested: conv._openMenuFor(msgDelegate.modelData, msgDelegate.mine)
                        onReplyRequested: conv.startReply(msgDelegate.modelData.id, msgDelegate.modelData.body || "", msgDelegate.modelData.senderName || "")
                        onEditRequested: conv.startEdit(msgDelegate.modelData.id, msgDelegate.modelData.body || "")
                        onMoreReactRequested: { conv.ctxMsgId = msgDelegate.modelData.id; reactPicker.open() }
                        onQuoteClicked: (id) => conv._jumpTo(id)
                        onRetryRequested: (txn) => conv.room.retrySend(txn)
                        onDiscardRequested: (txn) => conv.room.discardEcho(txn)
                        onPinRequested: (on) => conv.room.pinMessage(msgDelegate.modelData.id, on)
                        onAvatarClicked: (mxid) => conv.avatarActivated(mxid)
                        ts: msgDelegate.modelData.ts !== undefined ? msgDelegate.modelData.ts : 0
                        maxBubble: msgDelegate.maxBubble
                        avatarSource: msgDelegate.modelData.avatar !== undefined ? msgDelegate.modelData.avatar : ""
                    }
                }
            }

            // Empty / loading states. The host's EmptyState (furry-aware) when there is a host.
            // 10.1 (TH-TL-07): pinned to the viewport like the jump chip — as a plain child of the
            // ListView it was reparented into the (0-high) content item and centred on its top edge.
            Loader {
                parent: msgView
                anchors.centerIn: parent
                width: parent.width - Kirigami.Units.gridUnit * 2
                // 10.1 (TH-CONV-09): not before the first live load has landed — the cached timeline
                // paints first, and a genuinely empty room says so only once that is known.
                active: msgView.count === 0 && !!conv.app && (conv._markerFrozen || conv.mBusy || conv.mStatus.length > 0)
                sourceComponent: EmptyState {
                    app: conv.app
                    // 10.5.2: still paging back to the first message is loading, not "No messages".
                    readonly property bool _loading: conv.mBusy || msgView.fillingEmpty
                    loading: _loading
                    // 10.1 (TL-06): a load that FAILED is not an empty room.
                    iconName: conv.mStatus.length > 0 ? "network-disconnect-symbolic" : "dialog-messages-symbolic"
                    title: _loading ? "" : (conv.mStatus.length > 0 ? "Couldn't load this conversation" : "No messages")
                    explanation: _loading ? "" : (conv.mStatus.length > 0 ? conv.mStatus : "Say something to get started.")
                }
            }
            Controls.Label {
                parent: msgView   // 10.1 (TH-TL-07)
                anchors.centerIn: parent
                width: parent.width - Kirigami.Units.gridUnit * 2
                visible: msgView.count === 0 && !conv.mBusy && !conv.app
                horizontalAlignment: Text.AlignHCenter
                wrapMode: Text.WordWrap
                color: Th.timelineEmptyFg
                text: "No messages yet — say something."
            }
            ThBusyIndicator {   // 11.0 (GPU.md §2.5): still
                parent: msgView   // 10.1 (TH-TL-07)
                anchors.centerIn: parent
                running: msgView.count === 0 && conv.mBusy && !conv.app
                visible: running
                size: 48
            }
        }

        Kirigami.Separator { Layout.fillWidth: true }

        // 9.3: the typing notice in a FIXED-height strip. It used to be an animated row whose
        // height went 0 → 18 → 0 every time anyone started or stopped typing — and every change
        // resized the timeline (the "chat bounces while someone types" report). Nothing
        // moves: only the text fades. 10.1.2: ABOVE the composer now (was under it) so
        // the box can sit level with the rail's + and the room-list footer card.
        Item {
            Layout.fillWidth: true
            Layout.leftMargin: Kirigami.Units.smallSpacing + 4
            Layout.rightMargin: Kirigami.Units.smallSpacing
            Layout.topMargin: 1
            implicitHeight: Math.max(14, typingLabel.implicitHeight)   // 10.0.3: ≥ the 14 px above the composer box; 10.1 (TL-18): never shorter than its own text
            RowLayout {
                anchors.fill: parent
                spacing: Kirigami.Units.smallSpacing
                opacity: conv.mTyping.length > 0 ? 1 : 0
                Behavior on opacity { NumberAnimation { duration: Motion.durSlow } }
                Row {
                    id: typingDots
                    Layout.alignment: Qt.AlignVCenter
                    spacing: 3
                    // 11.0 (GPU.md §2.2, R6 — the web's TypingStrip): STATIC dots. The lit one moves ONE step only when
                    // the typing state changes (someone starts or stops typing), as a `dur` transition while the motion
                    // switch is on — never on a timer (it stepped every 400 ms for as long as anyone typed). The strip
                    // appearing lights the first dot.
                    property int phase: 0
                    readonly property string who: conv.mTyping.join("\n")
                    property string _prevWho: ""
                    onWhoChanged: {
                        var was = typingDots._prevWho
                        typingDots._prevWho = typingDots.who
                        if (typingDots.who.length === 0) return      // stopped: the strip fades out as it is
                        typingDots.phase = was.length > 0 ? (typingDots.phase + 1) % 3 : 0
                    }
                    Repeater {
                        model: 3
                        delegate: Rectangle {
                            required property int index
                            width: 5; height: 5; radius: 2.5
                            anchors.verticalCenter: parent.verticalCenter
                            color: Th.typingDot
                            opacity: index === typingDots.phase ? 1.0 : 0.35
                            Behavior on opacity { NumberAnimation { duration: Motion.dur } }
                        }
                    }
                }
                Controls.Label {
                    textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                    id: typingLabel
                    Layout.fillWidth: true
                    elide: Text.ElideRight
                    text: {
                        var t = conv.mTyping
                        if (t.length === 0) return ""
                        if (t.length === 1) return t[0] + " is typing…"
                        if (t.length === 2) return t[0] + " and " + t[1] + " are typing…"
                        var rest = t.length - 2
                        return t[0] + ", " + t[1] + " and " + rest + (rest === 1 ? " other are typing…" : " others are typing…")
                    }
                    color: Th.typingFg
                    font.italic: true
                    font.pointSize: Kirigami.Theme.smallFont.pointSize * 0.95
                }
            }
        }

        // 11.0 (islands C6/C7): the composer section sits here, under the timeline — or in the composer island (composerHost)
        Item {
            id: composerSlot
            Layout.fillWidth: true
            Layout.preferredHeight: conv._inIsland ? 0 : composerSection.implicitHeight
        }

    }

    ColumnLayout {
        id: composerSection
        parent: conv.composerHost ? conv.composerHost : composerSlot
        anchors.fill: parent
        spacing: 0
        visible: !conv.hidden

            // Reply / edit context bar. Height animates so it slides in rather than snapping the
            // footer; `visible: implicitHeight > 0` takes it out of the layout at rest.
            Rectangle {
                Layout.fillWidth: true
                Layout.leftMargin: Kirigami.Units.smallSpacing
                Layout.rightMargin: Kirigami.Units.smallSpacing
                Layout.topMargin: conv.composing ? Kirigami.Units.smallSpacing : 0
                clip: true
                visible: implicitHeight > 0
                implicitHeight: conv.composing ? ctxBarRow.implicitHeight + Kirigami.Units.smallSpacing : 0
                opacity: conv.composing ? 1 : 0
                Behavior on implicitHeight { NumberAnimation { duration: Motion.durSlow; easing.type: Easing.OutCubic } }
                Behavior on opacity { NumberAnimation { duration: Motion.dur } }
                color: Th.replyBg
                radius: 6
                RowLayout {
                    id: ctxBarRow
                    anchors.fill: parent
                    anchors.leftMargin: Kirigami.Units.smallSpacing
                    spacing: Kirigami.Units.smallSpacing
                    Rectangle { width: 3; Layout.fillHeight: true; radius: 1.5; color: Th.replyStripe }
                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: 0
                        Controls.Label {
                            textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                            text: conv.editToId.length > 0 ? "Editing message" : ("Replying to " + (conv.replyToName.length > 0 ? conv.replyToName : "a message"))
                            color: Th.replyTitleFg
                            font.bold: true
                            font.pointSize: Kirigami.Theme.smallFont.pointSize
                        }
                        Controls.Label {
                            textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                            Layout.fillWidth: true
                            text: conv.editToId.length > 0 ? conv.editToBody : conv.replyToBody
                            color: Th.replyBodyFg
                            elide: Text.ElideRight
                            maximumLineCount: 1
                            font.pointSize: Kirigami.Theme.smallFont.pointSize
                        }
                    }
                    Controls.ToolButton {
                        icon.name: "window-close-symbolic"
                        Controls.ToolTip.text: "Cancel (Esc)"
                        Controls.ToolTip.visible: hovered
                        Controls.ToolTip.delay: 400
                        onClicked: conv.cancelCompose()
                    }
                }
            }

            // 9.3: staged attachments strip.
            Flow {
                Layout.fillWidth: true
                Layout.leftMargin: Kirigami.Units.smallSpacing
                Layout.rightMargin: Kirigami.Units.smallSpacing
                Layout.topMargin: visible ? Kirigami.Units.smallSpacing : 0
                visible: conv.staged.length > 0
                spacing: 6
                Repeater {
                    model: conv.staged
                    delegate: Rectangle {
                        required property var modelData
                        required property int index
                        width: modelData.isImage ? 96 : Math.min(240, fileChip.implicitWidth + 40)
                        height: modelData.isImage ? 96 : 40
                        radius: 8
                        color: Th.composerStagedBg
                        border.width: 1
                        border.color: Th.composerStagedBorder
                        clip: true
                        Image {
                            anchors.fill: parent
                            anchors.margins: 3
                            visible: modelData.isImage
                            source: modelData.isImage ? modelData.url : ""
                            fillMode: Image.PreserveAspectCrop
                            asynchronous: true
                            sourceSize.width: 192
                            autoTransform: true
                        }
                        RowLayout {
                            id: fileChip
                            anchors.fill: parent
                            anchors.leftMargin: 8; anchors.rightMargin: 30
                            visible: !modelData.isImage
                            spacing: 6
                            Kirigami.Icon { source: "document-send-symbolic"; Layout.preferredWidth: 18; Layout.preferredHeight: 18 }
                            Controls.Label { textFormat: Text.PlainText; Layout.fillWidth: true; text: modelData.name; elide: Text.ElideMiddle; color: Th.composerStagedFg; font.pointSize: Kirigami.Theme.smallFont.pointSize }
                        }
                        Rectangle {
                            anchors.top: parent.top; anchors.right: parent.right; anchors.margins: 3
                            width: 20; height: 20; radius: 10
                            color: Th.composerStagedRemoveBg
                            ThIcon { anchors.centerIn: parent; width: 12; height: 12; app: conv.app; name: "window-close-symbolic"; color: Th.composerStagedRemoveIcon; isMask: true }
                            MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor; onClicked: conv.unstage(index) }
                        }
                        Controls.ToolTip.text: modelData.name
                        Controls.ToolTip.visible: chipHover.hovered
                        Controls.ToolTip.delay: 500
                        HoverHandler { id: chipHover }
                    }
                }
            }

            // ---- Composer box: the toolbar (attach + emoji + GIF + Send) and the input. ----
            Rectangle {
                id: composerBox
                // 10.0.3 gate hook: --tinhat-layoutlog prints where the box sits relative to the window.
                Timer {
                    interval: 2500; running: Qt.application.arguments.indexOf("--tinhat-layoutlog") >= 0
                    onTriggered: {
                        var w = composerBox.Window.window
                        var top = composerBox.mapToItem(null, 0, 0).y, bot = top + composerBox.height
                        var convBot = conv.mapToItem(null, 0, conv.height).y
                        console.log("[layout] window h=" + (w ? w.height : -1) + " conv bottom=" + Math.round(convBot)
                                    + " composer top=" + Math.round(top) + " bottom=" + Math.round(bot)
                                    + " (below composer: conv " + Math.round(convBot - bot) + " px, window " + Math.round((w ? w.height : 0) - bot) + " px)")
                    }
                }
                Layout.fillWidth: true
                Layout.leftMargin: conv._islandBox ? 0 : Kirigami.Units.smallSpacing
                Layout.rightMargin: conv._islandBox ? 0 : Kirigami.Units.smallSpacing
                // 10.1.2: the box sits 9 px off the window edge — level with the rail's + disc and
                // the room-list footer card (everything on the bottom row sits 9 px up). The typing strip
                // that used to sit under it (10.0.3 centred the box between divider and edge) is above it
                // now; with the reply/edit bar or staged files showing, the box hugs them.
                Layout.topMargin: (conv.composing || conv.staged.length > 0) ? 4 : 1
                Layout.bottomMargin: conv._islandBox ? 0 : 9
                // islands C2 (ISLANDS-ARCH §4.2): a whole number of pixels at 1-3 lines (the dock host measures it)
                implicitHeight: Math.ceil(composerGrid.implicitHeight) + 8
                radius: 12
                // 9.3: a real field in Legacy too — the Basic style (Windows, the rig) draws no frame,
                // so the composer was a bare placeholder line floating over the typing strip.
                color: conv._islandBox ? "transparent" : Th.composerBg
                border.width: conv._islandBox ? 0 : 1
                border.color: composer.activeFocus ? Th.composerBorderFocus : Th.composerBorder
                Behavior on border.color { ColorAnimation { duration: Motion.dur } }
            // 9.4: ONE row — [+] [the input, growing up to 8 lines] [emoji] [GIF] [soundboard] [pins]
            // [Send], the buttons riding the bottom line (Discord's shape). Was: a toolbar row above
            // the input.
            RowLayout {
                id: composerGrid
                anchors.fill: parent
                anchors.margins: 4
                spacing: 2

                // 11.0 buttons: the composer's left row (composer.start: the + by default) — a one-row grid in the
                // composer's first cell; the row's order places the composer's own buttons (Layout.column), anything
                // else is an ActionButton (generics). Right-click → "Edit buttons…".
                GridLayout {
                    Layout.alignment: Qt.AlignBottom
                    visible: crowStart.cells.length > 0
                    rows: 1
                    rowSpacing: 0
                    columnSpacing: 0
                    TapHandler { acceptedButtons: Qt.RightButton; onTapped: conv.openRowMenu(crowStart, "Message box — left") }
                    ThToolButton {
                        Layout.row: 0; Layout.column: Math.max(0, crowStart.colOf("composer.attach"))
                        visible: crowStart.shows("composer.attach")
                        app: conv.app
                        iconName: "list-add-symbolic"
                        tip: "Attach a file or image (or drag one into the chat)"
                        enabled: conv.room && conv.room.roomId.length > 0
                        onClicked: attachDialog.open()
                    }
                    Repeater {
                        model: crowStart.generics
                        delegate: ActionButton {
                            required property var modelData
                            Layout.row: 0
                            Layout.column: modelData.col
                            app: conv.app
                            item: modelData.item
                            rowStyle: crowStart.result.style
                            rowScale: crowStart.result.scale
                            onRun: (anchor) => crowStart.run(modelData.item, anchor, { handler: conv.composerHandler(modelData.item.a) })
                            onHold: (anchor, down) => crowStart.run(modelData.item, anchor, { hold: down ? "down" : "up" })
                        }
                    }
                }
            // ---- Composer (multiline; Enter sends, Shift+Enter newline) ----
            Controls.ScrollView {
                Layout.fillWidth: true
                Layout.minimumWidth: Kirigami.Units.gridUnit * 8   // 10.1 (TH-CONV-12): the input wins the shrink fight
                Layout.maximumHeight: Kirigami.Units.gridUnit * 8
                Controls.TextArea {
                    id: composer
                    // ENGINE.md §10: the theme's chat font (type.chat, else type.ui) — the message you write
                    font.family: Th.fontChat.length > 0 ? Th.fontChat : Qt.application.font.family
                    color: palette.text
                    placeholderTextColor: palette.placeholderText
                    Component.onCompleted: { background = null; leftPadding = 10; rightPadding = 10; topPadding = 7; bottomPadding = 6 }
                    property bool _typingActive: false
                    // @mentions picked from autocomplete: [{name, userId}].
                    property var _mentions: []
                    property int _mentionStart: -1
                    // 9.3: :emoji: autocomplete.
                    property int _emojiStart: -1
                    function _mxLocalpart(mx) { return ("" + mx).replace(/^@/, "").split(":")[0] }
                    // A "@partial" token at the cursor opens the member list; a ":partial" (2+ chars)
                    // opens the emoji list. Picking inserts the full thing plus a space, which
                    // dismisses the popup.
                    function _updateMention() {
                        var c = composer.cursorPosition
                        var pre = composer.text.substring(0, c)
                        var m = pre.match(/(?:^|\s)@([^\s@]*)$/)
                        if (m && conv.room && conv.room.roomId.length > 0) {
                            emojiPopup.close()
                            composer._mentionStart = c - m[1].length - 1
                            var q = m[1].toLowerCase()
                            var list = (conv.mMembers || []).filter(function (mem) {
                                if (mem.membership && mem.membership !== "join") return false   // 10.1 (TH-GAP-12): not the invited / banned
                                return (("" + (mem.name || "")).toLowerCase().indexOf(q) !== -1)
                                    || (composer._mxLocalpart(mem.mxid).toLowerCase().indexOf(q) !== -1)
                            }).slice(0, 8)
                            if ("room".indexOf(q) === 0 && q.length > 0) list.unshift({ name: "room", mxid: "@room", room: true })
                            mentionList.model = list
                            if (mentionList.model.length > 0) {
                                mentionList.currentIndex = 0
                                if (!mentionPopup.visible) mentionPopup.open()
                            } else mentionPopup.close()
                            return
                        }
                        mentionPopup.close()
                        var e = pre.match(/(?:^|\s):([a-z0-9_+-]{2,})$/i)
                        if (e) {
                            composer._emojiStart = c - e[1].length - 1
                            var eq = e[1].toLowerCase()
                            var all = bubbleEmojiPicker.allEmojis || []
                            var hits = []
                            for (var i = 0; i < all.length && hits.length < 8; i++) {
                                var k = all[i].k
                                if (k.indexOf(eq) === 0 || k.indexOf(" " + eq) !== -1) hits.push(all[i])
                            }
                            emojiList.model = hits
                            if (hits.length > 0) {
                                emojiList.currentIndex = 0
                                if (!emojiPopup.visible) emojiPopup.open()
                            } else emojiPopup.close()
                        } else emojiPopup.close()
                    }
                    function _pickMention(member) {
                        var c = composer.cursorPosition
                        var before = composer.text.substring(0, composer._mentionStart)
                        var after = composer.text.substring(c)
                        var ins = "@" + member.name + " "
                        composer.text = before + ins + after
                        composer.cursorPosition = (before + ins).length
                        if (member.room !== true) {
                            var arr = composer._mentions
                            arr.push({ name: member.name, userId: member.mxid })
                            composer._mentions = arr
                        }
                        mentionPopup.close()
                        composer.forceActiveFocus()
                    }
                    function _pickEmoji(item) {
                        var c = composer.cursorPosition
                        var before = composer.text.substring(0, composer._emojiStart)
                        var after = composer.text.substring(c)
                        var ins = item.e + " "
                        composer.text = before + ins + after
                        composer.cursorPosition = (before + ins).length
                        emojiPopup.close()
                        composer.forceActiveFocus()
                    }
                    // 9.3: Up in an empty composer edits your last message (Discord / Slack).
                    function _editLastMine() {
                        for (var i = conv.mMessages.length - 1; i >= 0; i--) {
                            var m = conv.mMessages[i]
                            if (m.sender === conv.myId && m.echo !== true && m.redacted !== true
                                && (m.msgtype === "m.text" || m.msgtype === "m.emote" || m.msgtype === undefined)) {
                                conv.startEdit(m.id, m.body || "")
                                return true
                            }
                        }
                        return false
                    }
                    wrapMode: TextEdit.Wrap
                    placeholderText: conv.editToId.length > 0 ? "Edit your message…"
                        : (conv.replyToId.length > 0 ? "Reply…" : "Message " + conv._shownName)
                    Keys.onUpPressed: (event) => {
                        if (mentionPopup.visible && mentionList.count > 0) {
                            mentionList.currentIndex = (mentionList.currentIndex - 1 + mentionList.count) % mentionList.count
                            event.accepted = true
                        } else if (emojiPopup.visible && emojiList.count > 0) {
                            emojiList.currentIndex = (emojiList.currentIndex - 1 + emojiList.count) % emojiList.count
                            event.accepted = true
                        } else if (composer.text.length === 0 && conv.editToId.length === 0) {
                            event.accepted = composer._editLastMine()
                        } else event.accepted = false
                    }
                    Keys.onDownPressed: (event) => {
                        if (mentionPopup.visible && mentionList.count > 0) {
                            mentionList.currentIndex = (mentionList.currentIndex + 1) % mentionList.count
                            event.accepted = true
                        } else if (emojiPopup.visible && emojiList.count > 0) {
                            emojiList.currentIndex = (emojiList.currentIndex + 1) % emojiList.count
                            event.accepted = true
                        } else event.accepted = false
                    }
                    Keys.onTabPressed: (event) => {
                        if (mentionPopup.visible && mentionList.count > 0) {
                            composer._pickMention(mentionList.model[mentionList.currentIndex]); event.accepted = true
                        } else if (emojiPopup.visible && emojiList.count > 0) {
                            composer._pickEmoji(emojiList.model[emojiList.currentIndex]); event.accepted = true
                        } else event.accepted = false
                    }
                    Keys.onEscapePressed: {
                        if (mentionPopup.visible) mentionPopup.close()
                        else if (emojiPopup.visible) emojiPopup.close()
                        // 10.1 (TH-CONV-04): Escape cancels the reply/edit TARGET only — never the text you
                        // typed or the files you staged (the ✕ in the reply bar / on each thumbnail do that
                        // explicitly). Leaving an edit empties the box (that text was the old message);
                        // leaving a reply keeps what you wrote.
                        else if (conv.composing) conv.cancelTarget()
                        else msgView.jumpToLatest()   // Discord: Esc = back to the newest, mark read
                    }
                    Keys.onPressed: (event) => {
                        if (event.key === Qt.Key_PageUp) { msgView.pageBy(-1); event.accepted = true }
                        else if (event.key === Qt.Key_PageDown) { msgView.pageBy(1); event.accepted = true }
                        else if (event.matches(StandardKey.Paste)) { if (conv.pasteAttachments()) event.accepted = true }
                    }
                    Keys.onReturnPressed: (event) => composer._enter(event)
                    Keys.onEnterPressed: (event) => composer._enter(event)
                    function _enter(event) {
                        if (mentionPopup.visible && mentionList.count > 0) {
                            composer._pickMention(mentionList.model[mentionList.currentIndex]); event.accepted = true
                        } else if (emojiPopup.visible && emojiList.count > 0) {
                            composer._pickEmoji(emojiList.model[emojiList.currentIndex]); event.accepted = true
                        } else if (event.modifiers & Qt.ShiftModifier) { event.accepted = false }
                        else { event.accepted = true; conv.sendComposer() }
                    }
                    onTextChanged: { conv.composerTextChanged(); composer._updateMention() }

                    // @mention autocomplete — focus:false so typing stays in the composer.
                    Controls.Popup {
                        id: mentionPopup
                        focus: false
                        y: -height - 4
                        x: 0
                        width: 280
                        height: Math.min(Math.max(mentionList.count, 1) * 34 + 8, 210)
                        padding: 4
                        closePolicy: Controls.Popup.CloseOnPressOutside
                        background: Rectangle { color: Th.composerAutocompleteBg; border.color: Th.composerAutocompleteBorder; border.width: 1; radius: 8 }
                        ListView {
                            id: mentionList
                            anchors.fill: parent
                            clip: true
                            model: []
                            delegate: Controls.ItemDelegate {
                                required property var modelData
                                required property int index
                                width: ListView.view ? ListView.view.width : 0
                                height: 34
                                highlighted: ListView.isCurrentItem
                                onClicked: composer._pickMention(modelData)
                                contentItem: RowLayout {
                                    spacing: 6
                                    Avatar {
                                        app: conv.app; size: 24
                                        name: modelData.name !== undefined ? modelData.name : ""
                                        seed: modelData.mxid !== undefined ? modelData.mxid : (modelData.name !== undefined ? modelData.name : "")
                                        imageSource: modelData.avatar !== undefined ? modelData.avatar : ""
                                    }
                                    Controls.Label {
                                        textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                        Layout.fillWidth: true
                                        text: modelData.room === true ? "@room — everyone here" : (modelData.name !== undefined ? modelData.name : "")
                                        color: Th.composerAutocompleteFg
                                        elide: Text.ElideRight
                                    }
                                }
                            }
                        }
                    }
                    // 9.3: :emoji: autocomplete.
                    Controls.Popup {
                        id: emojiPopup
                        focus: false
                        y: -height - 4
                        x: 0
                        width: 260
                        height: Math.min(Math.max(emojiList.count, 1) * 30 + 8, 250)
                        padding: 4
                        closePolicy: Controls.Popup.CloseOnPressOutside
                        background: Rectangle { color: Th.composerAutocompleteBg; border.color: Th.composerAutocompleteBorder; border.width: 1; radius: 8 }
                        ListView {
                            id: emojiList
                            anchors.fill: parent
                            clip: true
                            model: []
                            delegate: Controls.ItemDelegate {
                                required property var modelData
                                required property int index
                                width: ListView.view ? ListView.view.width : 0
                                height: 30
                                highlighted: ListView.isCurrentItem
                                onClicked: composer._pickEmoji(modelData)
                                contentItem: RowLayout {
                                    spacing: 8
                                    Controls.Label { textFormat: Text.PlainText; text: modelData.e; font.pixelSize: 18 }
                                    Controls.Label {
                                        textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                        Layout.fillWidth: true
                                        text: ":" + String(modelData.k).split(" ")[0] + ":"
                                        color: Th.composerAutocompleteHintFg
                                        elide: Text.ElideRight
                                        font.pointSize: Kirigami.Theme.smallFont.pointSize
                                    }
                                }
                            }
                        }
                    }
                }
            }
            // 11.0 buttons: the composer's right row (composer.end) — a one-row GridLayout (was a RowLayout; the same
            // geometry) so the row's order places the composer's own buttons; anything else is an ActionButton.
            GridLayout {
                Layout.alignment: Qt.AlignBottom
                rows: 1
                rowSpacing: 0
                columnSpacing: 0
                TapHandler { acceptedButtons: Qt.RightButton; onTapped: conv.openRowMenu(crowEnd, "Message box — right") }

                ThToolButton {
                    id: bubbleEmojiButton
                    Layout.row: 0; Layout.column: Math.max(0, crowEnd.colOf("composer.emoji"))
                    visible: crowEnd.shows("composer.emoji")
                    app: conv.app
                    iconName: "face-smile-symbolic"
                    tip: "Insert emoji (type :name: for quick search)"
                    onClicked: conv.openPopupAt(bubbleEmojiPicker, bubbleEmojiButton)
                    EmojiPicker {
                        id: bubbleEmojiPicker
                        app: conv.app
                        y: -height - 6
                        x: conv.popupX(parent, width)
                        // 10.1 (TH-CONV-15): stays open across picks (its search keeps focus);
                        // Escape or a click on the composer closes it and hands focus back.
                        stayOpen: true
                        onPicked: (emoji) => composer.insert(composer.cursorPosition, emoji)
                        onClosed: composer.forceActiveFocus()
                    }
                }

                ThToolButton {   // 10.1 (TH-CONV-06): the house button (it was a raw ToolButton showing bold text "GIF")
                    id: bubbleGifButton
                    Layout.row: 0; Layout.column: Math.max(0, crowEnd.colOf("composer.gif"))
                    app: conv.app
                    iconName: "image-x-generic"
                    label: "GIF"
                    visible: crowEnd.shows("composer.gif") && conv.gif !== null
                    tip: "Send a GIF"
                    onClicked: conv.openGifAt(bubbleGifButton)
                    Controls.Popup {
                        id: bubbleGifPopup
                        y: -height - 6
                        x: conv.popupX(parent, width)
                        width: 430   // 10.1 (TH-VIS-05): three columns, not two clipped ones
                        height: 430
                        padding: 8
                        modal: false
                        focus: true
                        closePolicy: Controls.Popup.CloseOnEscape | Controls.Popup.CloseOnPressOutside
                        // Do NOT clear() the SHARED GifController on close — it also backs every other
                        // popup. We just stop mirroring while hidden; on reopen search("") repopulates.
                        onClosed: { bubbleGifPopup.gifs = []; bubbleGifPopup._gifsRaw = "" }
                        property var gifs: []
                        property string _gifsRaw: ""
                        Timer {
                            interval: 150
                            running: bubbleGifPopup.visible && conv.gif !== null
                            repeat: true
                            onTriggered: {
                                if (!conv.gif) return
                                if (conv.gif.resultsJson !== bubbleGifPopup._gifsRaw) {
                                    bubbleGifPopup._gifsRaw = conv.gif.resultsJson
                                    bubbleGifPopup.gifs = bubbleGifPopup._gifsRaw.length > 0 ? JSON.parse(bubbleGifPopup._gifsRaw) : []
                                }
                            }
                        }
                        background: Rectangle {
                            color: Th.gifBg
                            border.color: Th.gifBorder
                            border.width: 1
                            radius: 10
                        }
                        ColumnLayout {
                            anchors.fill: parent
                            spacing: 6
                            Kirigami.SearchField {
                                id: bubbleGifSearch
                                Layout.fillWidth: true
                                placeholderText: "Search GIFs…"
                                Kirigami.Theme.inherit: false
                                Kirigami.Theme.backgroundColor: Th.gifSearchBg
                                Kirigami.Theme.textColor: Th.gifSearchFg
                                palette.base: Th.gifSearchBg
                                palette.text: Th.gifSearchFg
                                palette.placeholderText: Th.gifSearchPlaceholder
                                palette.highlight: Th.gifSearchBorderFocus
                                color: Th.gifSearchFg
                                placeholderTextColor: Th.gifSearchPlaceholder
                                onTextChanged: if (conv.gif) conv.gif.search(text)
                            }
                            GridView {
                                id: bubbleGifGrid
                                Layout.fillWidth: true
                                Layout.fillHeight: true
                                clip: true
                                cellWidth: Math.floor((width - 14) / 3)   // 10.1 (TH-VIS-05): three across, bar reserved
                                cellHeight: 100
                                model: bubbleGifPopup.gifs
                                Controls.ScrollBar.vertical: FoxScrollBar { app: conv.app }
                                delegate: Item {
                                    required property var modelData
                                    width: bubbleGifGrid.cellWidth
                                    height: bubbleGifGrid.cellHeight
                                    Rectangle {
                                        anchors.fill: parent
                                        anchors.margins: 3
                                        radius: 6
                                        clip: true
                                        color: Th.gifTileBg
                                        Kirigami.ShadowedImage {
                                            anchors.fill: parent
                                            radius: 6
                                            color: "transparent"
                                            fillMode: Image.PreserveAspectCrop
                                            source: (modelData.preview !== undefined) ? modelData.preview : ""
                                            asynchronous: true
                                        }
                                        MouseArea {
                                            anchors.fill: parent
                                            cursorShape: Qt.PointingHandCursor
                                            onClicked: {
                                                if (conv.gif && conv.room && conv.room.roomId.length > 0)
                                                    conv.gif.pick(conv.room.roomId, modelData.url)
                                                bubbleGifPopup.close()
                                            }
                                        }
                                    }
                                }
                            }
                            Controls.Label {
                                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                Layout.fillWidth: true
                                horizontalAlignment: Text.AlignHCenter
                                visible: bubbleGifPopup.gifs.length === 0
                                text: (conv.gif && conv.gif.loading) ? "Loading…"
                                    : (conv.gif && conv.gif.error.length > 0 ? conv.gif.error : "No GIFs")
                                color: Th.gifStatusFg
                                font.pixelSize: Math.round(Kirigami.Theme.smallFont.pixelSize)
                            }
                            Controls.Label {
                                Layout.fillWidth: true
                                horizontalAlignment: Text.AlignRight
                                text: "Powered by GIPHY"
                                color: Qt.alpha(Th.gifCreditFg, 1)   // the token's alpha is the item's opacity (§3.8)
                                opacity: Th.gifCreditFg.a
                                font.pointSize: Kirigami.Theme.smallFont.pointSize * 0.85
                            }
                        }
                    }
                }

                // The soundboard — one of the most-used things, so it sits in the composer.
                ThToolButton {
                    Layout.row: 0; Layout.column: Math.max(0, crowEnd.colOf("soundboard.open"))
                    visible: crowEnd.shows("soundboard.open") && conv.showSoundboard && !!conv.app && !!conv.app.soundboardCtl && !!conv.app.soundboardPanel
                    opacity: crowEnd.dimmed("soundboard.open") ? 0.5 : 1
                    app: conv.app
                    iconName: "audio-speakers-symbolic"
                    tip: "Soundboard — Ctrl+Shift+S"
                    enabled: conv.room && conv.room.roomId.length > 0
                    onClicked: { conv.app.soundboardCtl.refresh(conv.room.roomId); conv.app.soundboardPanel.openOrRaise() }
                }

                // 9.3: pinned messages, when there are any.
                Controls.ToolButton {
                    id: pinnedButton
                    Layout.row: 0; Layout.column: Math.max(0, crowEnd.colOf("room.pinned.open"))
                    visible: crowEnd.shows("room.pinned.open") && conv.pinnedCount > 0
                    text: LinkGate.plain("📌 " + conv.pinnedCount)
                    display: Controls.AbstractButton.TextOnly
                    Controls.ToolTip.text: "Pinned messages"
                    Controls.ToolTip.visible: hovered
                    Controls.ToolTip.delay: 400
                    onClicked: conv.openPopupAt(pinnedPopup, pinnedButton)
                    Controls.Popup {
                        id: pinnedPopup
                        y: -height - 6
                        x: conv.popupX(parent, width)
                        width: 360
                        height: Math.min(420, pinnedList.contentHeight + 46)
                        padding: 8
                        modal: false
                        focus: true
                        closePolicy: Controls.Popup.CloseOnEscape | Controls.Popup.CloseOnPressOutside
                        background: Rectangle { color: Th.composerPinnedBg; border.color: Th.composerPinnedBorder; border.width: 1; radius: 10 }
                        ColumnLayout {
                            anchors.fill: parent
                            spacing: 4
                            Controls.Label { text: "Pinned messages"; color: Th.composerPinnedTitleFg; font.bold: true }
                            ListView {
                                id: pinnedList
                                Layout.fillWidth: true
                                Layout.fillHeight: true
                                clip: true
                                model: conv.mPinned
                                spacing: 4
                                delegate: Rectangle {
                                    required property var modelData
                                    width: ListView.view ? ListView.view.width : 0
                                    height: pinCol.implicitHeight + 12
                                    radius: 6
                                    color: pinMa.containsMouse ? Th.composerPinnedRowBgHover : "transparent"
                                    ColumnLayout {
                                        id: pinCol
                                        anchors.left: parent.left; anchors.right: parent.right; anchors.verticalCenter: parent.verticalCenter
                                        anchors.margins: 6
                                        spacing: 1
                                        Controls.Label { textFormat: Text.PlainText; text: (modelData.senderName || "") + " · " + Qt.formatDateTime(new Date(modelData.ts || 0), "MMM d") + ", " + ((conv.app && conv.app.fmtClock) ? conv.app.fmtClock(modelData.ts || 0) : Qt.formatTime(new Date(modelData.ts || 0), Locale.ShortFormat)); color: Th.composerPinnedRowMetaFg; font.bold: true; font.pointSize: Kirigami.Theme.smallFont.pointSize }
                                        Controls.Label { textFormat: Text.PlainText; Layout.fillWidth: true; text: modelData.body || ""; color: Th.composerPinnedRowFg; wrapMode: Text.Wrap; maximumLineCount: 3; elide: Text.ElideRight }
                                    }
                                    MouseArea {
                                        id: pinMa
                                        anchors.fill: parent
                                        hoverEnabled: true
                                        cursorShape: Qt.PointingHandCursor
                                        onClicked: { pinnedPopup.close(); conv._jumpTo(modelData.id) }
                                    }
                                }
                            }
                        }
                    }
                }

                GlowButton {
                    Layout.row: 0; Layout.column: Math.max(0, crowEnd.colOf("composer.send"))
                    visible: crowEnd.shows("composer.send")
                    app: conv.app
                    text: conv.width < 360 ? "" : (conv.editToId.length > 0 ? "Save" : "Send")
                    icon.name: "document-send-symbolic"
                    enabled: composer.text.trim().length > 0 || conv.staged.length > 0
                    Controls.ToolTip.text: conv.editToId.length > 0 ? "Save the edit" : "Send (Enter · Shift+Enter for a new line)"
                    Controls.ToolTip.visible: hovered
                    Controls.ToolTip.delay: 600
                    onClicked: conv.sendComposer()
                }
                Repeater {
                    model: crowEnd.generics
                    delegate: ActionButton {
                        required property var modelData
                        Layout.row: 0
                        Layout.column: modelData.col
                        app: conv.app
                        item: modelData.item
                        rowStyle: crowEnd.result.style
                        rowScale: crowEnd.result.scale
                        onRun: (anchor) => crowEnd.run(modelData.item, anchor, { handler: conv.composerHandler(modelData.item.a) })
                        onHold: (anchor, down) => crowEnd.run(modelData.item, anchor, { hold: down ? "down" : "up" })
                    }
                }
                Repeater {
                    model: crowEnd.seps
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

            }   // composerGrid
            }   // composerBox
    }

    // ---- send paths -------------------------------------------------------------------------
    // Slash commands (Discord's everyday set): /me, /shrug, /tableflip, /unflip, /lenny.
    function _expandSlash(t) {
        var s = t
        if (/^\/shrug\b/i.test(s)) s = s.replace(/^\/shrug\s?/i, "") + " ¯\\_(ツ)_/¯"
        else if (/^\/tableflip\b/i.test(s)) s = s.replace(/^\/tableflip\s?/i, "") + " (╯°□°)╯︵ ┻━┻"
        else if (/^\/unflip\b/i.test(s)) s = s.replace(/^\/unflip\s?/i, "") + " ┬─┬ノ( º _ ºノ)"
        else if (/^\/lenny\b/i.test(s)) s = s.replace(/^\/lenny\s?/i, "") + " ( ͡° ͜ʖ ͡°)"
        return s.trim()
    }
    function sendComposer() {
        var t = composer.text
        if (!conv.room) return
        var hasText = t.trim().length > 0
        if (!hasText && conv.staged.length === 0) return
        // 10.1 (TH-CONV-05): attachments can't carry a reply relation yet, so sending files
        // under an armed reply used to leave the reply ARMED — and the next line you typed was
        // quietly threaded under a message from ten minutes ago. Ask for a line of text instead.
        if (!hasText && conv.replyToId.length > 0) {
            if (conv.app && conv.app.notify) conv.app.notify("Type something to go with the reply, or cancel the reply first", "short")
            return
        }
        if (!hasText && conv.editToId.length > 0) { conv.editToId = ""; conv.editToBody = "" }
        if (hasText) {
            if (conv.editToId.length > 0) {
                conv.room.editMessage(conv.editToId, t)
                conv.editToId = ""; conv.editToBody = ""
            } else if (conv.replyToId.length > 0) {
                // 10.1 (TH-GAP-14): "/me" inside a reply used to post literally.
                var rt = /^\/me\s+\S/i.test(t) ? "* " + conv._expandSlash(t.replace(/^\/me\s+/i, "")) : conv._expandSlash(t)
                // 10.6 (REACH A5.2): mentions picked while replying are kept (with their links); the
                // parent's sender is mentioned by Rust, so the person replied to is notified.
                var rlive = composer._mentions.filter(function (m) { return t.indexOf("@" + m.name) !== -1 })
                if (rlive.length > 0) conv.room.sendReplyMentions(conv.replyToId, rt, JSON.stringify(rlive))
                else conv.room.sendReply(conv.replyToId, rt)
                conv.replyToId = ""; conv.replyToBody = ""
            } else if (/^\/me\s+\S/i.test(t)) {
                conv.room.sendEmote(t.replace(/^\/me\s+/i, ""))
            } else if (composer._mentions.length > 0) {
                var live = composer._mentions.filter(function (m) { return t.indexOf("@" + m.name) !== -1 })
                if (live.length > 0) conv.room.sendWithMentions(conv._expandSlash(t), JSON.stringify(live))
                else conv.room.send(conv._expandSlash(t))
            } else {
                conv.room.send(conv._expandSlash(t))
            }
        }
        for (var i = 0; i < conv.staged.length; i++) conv.room.sendAttachment(conv.staged[i].url)
        if (conv.app && conv.app.playUiSound) conv.app.playUiSound("send")   // 10.0
        // Whatever was sent, the compose target is consumed (TH-CONV-05).
        conv.replyToId = ""; conv.replyToBody = ""; conv.editToId = ""; conv.editToBody = ""
        conv.staged = []
        composer.text = ""
        composer._mentions = []
        mentionPopup.close(); emojiPopup.close()
        conv.stopTyping()
        // Sending = you are at the bottom now.
        msgView.stickBottom = true
        Qt.callLater(msgView.positionViewAtEnd)
    }
    function cancelCompose() {
        conv.replyToId = ""; conv.replyToBody = ""; conv.editToId = ""; conv.editToBody = ""
        composer.text = ""
        composer._mentions = []
        conv.staged = []
        mentionPopup.close(); emojiPopup.close()
        conv.stopTyping()
    }
    // 10.1 (TH-CONV-04): drop the reply/edit target, keep the draft (an edit's text was the old
    // message, so leaving an edit does empty the box).
    function cancelTarget() {
        var wasEdit = conv.editToId.length > 0
        conv.replyToId = ""; conv.replyToBody = ""; conv.replyToName = ""; conv.editToId = ""; conv.editToBody = ""
        if (wasEdit) { composer.text = ""; composer._mentions = [] }
        mentionPopup.close(); emojiPopup.close()
    }
    onEditToIdChanged: if (conv.editToId.length > 0) { composer.text = conv.editToBody; composer.forceActiveFocus(); composer.cursorPosition = composer.text.length }

    // Typing notice: `true` on the first keystroke, auto-cleared after 4 s idle (or on send /
    // cancel / empty). Only a `primary` view sends it — a bubble must stay silent.
    function composerTextChanged() {
        if (!conv.primary || !conv.room) return
        if (composer.text.length > 0 && conv.editToId.length === 0) {
            if (!composer._typingActive) { composer._typingActive = true; conv.room.setTyping(true) }
            typingStopTimer.restart()
        } else if (composer._typingActive) {
            conv.stopTyping()
        }
    }
    function stopTyping() {
        if (composer._typingActive && conv.room) { composer._typingActive = false; conv.room.setTyping(false) }
        typingStopTimer.stop()
    }
    Timer { id: typingStopTimer; interval: 4000; onTriggered: conv.stopTyping() }
    Component.onDestruction: { if (conv.primary) conv.stopTyping(); conv._remember(conv._roomIdSeen) }
}
