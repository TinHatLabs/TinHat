// FloatingChats.qml — the manager for Feature #6 floating chat bubbles (Discord-style
// chat-heads). Renders the FloatingController's open-bubble registry as a Repeater of
// top-level QtQuick `Window` items. Each bubble Window owns its OWN RoomController
// instance (independent sync/key task) feeding a RECEIPT-FREE ConversationView.
//
// MINIMIZE-TO-AVATAR (single morphing window): each bubble is ONE frameless, always-on-top
// Window that MORPHS between a full chat (380x520) and a small circular chat-head (64x64) by
// changing its size + swapping which content is visible. The window is NEVER hidden and its
// flags NEVER change, so the platform surface is never re-mapped or recreated.
//   WHY: the old design used two separate windows toggled by `visible`. Re-mapping a hidden
//   Qt-Wayland `xdg_toplevel` doesn't reliably come back on KWin, so a minimized bubble went
//   dead — its restore click did nothing and it couldn't be moved or closed. Frameless input
//   (hover/click/drag) works fine; only hide→show was broken. Morphing one always-visible
//   window sidesteps that entirely.
//
// WAYLAND/KWIN: the app can't self-position or self-size-by-pixel its windows. Dragging uses
// the compositor (startSystemMove); the resize grip uses startSystemResize. The window's title
// flips between "<room> — Chat" and "<room> — head" so the optional KWin rules in
// packaging/kwinrulesrc.bubbles can match it — KWin matches by wmclass (the binary's name) + title,
// NOT by objectName (a Qt objectName isn't surfaced over xdg-shell). The objectName flip is
// internal-only. Placement is the only thing rules do — the app never force-sizes from a rule.
//
// RECEIPT-SAFETY: the ConversationView never calls markRead — see its header. This manager
// also never sends receipts; it only reads metadata via FloatingController.roomsMetaJson.
import QtQuick
import QtQuick.Window
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app

Item {
    id: mgr
    // Theme host (Main.qml root) + the registry controller.
    required property var app
    required property var floating
    // Shared GifController (gifCtl) from Main.qml — forwarded into each bubble's
    // ConversationView so its composer GIF picker reuses the Giphy path. Optional.
    property var gif: null

    FallbackTheme { id: fallbackTheme }
    // 10.1 (TH-FLOAT-07): a bubble opens at the size you last resized one to (position is the
    // compositor's on Wayland; off Wayland the window manager places it).
    readonly property var store: (app && app.prefStore) ? app.prefStore : null
    function _rememberedW() { var w = store ? parseInt(store.uiPref("bubble-w")) : NaN; return (!isNaN(w) && w >= 280 && w < 8000) ? w : 380 }
    function _rememberedH() { var h = store ? parseInt(store.uiPref("bubble-h")) : NaN; return (!isNaN(h) && h >= 240 && h < 8000) ? h : 520 }
    property real _pendingW: 0
    property real _pendingH: 0
    function noteBubbleSize(w, h) { _pendingW = w; _pendingH = h; bubbleSizeTimer.restart() }
    Timer { id: bubbleSizeTimer; interval: 900; onTriggered: if (mgr.store && mgr._pendingW >= 280 && mgr._pendingH >= 240) { mgr.store.applyUiPref("bubble-w", String(Math.round(mgr._pendingW))); mgr.store.applyUiPref("bubble-h", String(Math.round(mgr._pendingH))) } }

    // Mirror of FloatingController.openIdsJson / roomsMetaJson (no direct getter binding).
    property var openIds: []
    property string _openIdsRaw: ""
    property var roomsMeta: []
    property string _roomsMetaRaw: ""

    function _metaFor(roomId) {
        for (var i = 0; i < roomsMeta.length; i++)
            if (roomsMeta[i].id === roomId) return roomsMeta[i]
        return ({ id: roomId, name: "", isDm: false, avatar: "", unread: 0, highlight: false })
    }

    // Stable-identity bubble model. The Repeater is driven by THIS ListModel, not the raw
    // `openIds` JS array — reassigning a JS array makes a Repeater rebuild EVERY delegate
    // (re-running each surviving bubble's RoomController.open() => re-sync/re-key, and
    // losing its window position/scroll/minimized state). Instead we DIFF openIds into the
    // model: only the added/removed bubble's row (and thus its Window + RoomController) is
    // created/destroyed; untouched bubbles keep their delegate.
    ListModel { id: bubbleModel }

    function _syncModel() {
        // Remove rows whose id is no longer open.
        for (var i = bubbleModel.count - 1; i >= 0; i--) {
            var rid = bubbleModel.get(i).roomId
            if (mgr.openIds.indexOf(rid) === -1) bubbleModel.remove(i)
        }
        // Append rows for newly-open ids (preserve existing rows' identity/order).
        for (var j = 0; j < mgr.openIds.length; j++) {
            var id = mgr.openIds[j]
            var found = false
            for (var k = 0; k < bubbleModel.count; k++) {
                if (bubbleModel.get(k).roomId === id) { found = true; break }
            }
            if (!found) bubbleModel.append({ roomId: id })
        }
    }

    onOpenIdsChanged: _syncModel()

    Timer {
        interval: Motion.focused ? 200 : 1000   // 11.0 (GPU.md §2.8, R14): ≥ 1 s while the window is not focused
        running: true
        repeat: true
        onTriggered: {
            if (!mgr.floating) return
            if (mgr.floating.openIdsJson !== mgr._openIdsRaw) {
                mgr._openIdsRaw = mgr.floating.openIdsJson
                mgr.openIds = mgr._openIdsRaw.length > 0 ? JSON.parse(mgr._openIdsRaw) : []
            }
            if (mgr.floating.roomsMetaJson !== mgr._roomsMetaRaw) {
                mgr._roomsMetaRaw = mgr.floating.roomsMetaJson
                mgr.roomsMeta = mgr._roomsMetaRaw.length > 0 ? JSON.parse(mgr._roomsMetaRaw) : []
            }
        }
    }
    // Keep the open rooms' metadata (unread/name/avatar) fresh while bubbles are up.
    Timer {
        interval: 4000
        running: mgr.openIds.length > 0
        repeat: true
        onTriggered: if (mgr.floating) mgr.floating.refreshMeta()
    }
    // 10.1 (TH-FLOAT-10): sign-out takes the bubbles with it — the previous account's DMs must not
    // stay frozen on screen in always-on-top windows.
    Connections {
        target: mgr.app
        function onMLoggedInChanged() {
            if (!mgr.app || mgr.app.mLoggedIn || !mgr.floating) return
            var ids = mgr.openIds.slice()
            for (var i = 0; i < ids.length; i++) mgr.floating.closeBubble(ids[i])
        }
    }

    // Auto-focus an already-open bubble when the user re-triggers "Pop out chat":
    // FloatingController.openBubble is idempotent (no new slot), so the Window stays;
    // we surface it via a signal the parent relays here. restore() also un-minimizes
    // a head so re-popping a minimized chat brings the full window back.
    function focusBubble(roomId) {
        for (var i = 0; i < bubbleRepeater.count; i++) {
            var item = bubbleRepeater.itemAt(i)
            if (item && item.roomId === roomId) { item.restore(); return }
        }
    }

    Repeater {
        id: bubbleRepeater
        model: bubbleModel

        // Each entry is a per-room bubble: ONE morphing frameless Window + its own
        // RoomController. See the file header for why it's a single always-visible window.
        delegate: Item {
            id: slot
            required property string roomId
            required property int index
            readonly property var meta: mgr._metaFor(roomId)
            property bool minimized: false
            // True once the user has dismissed this bubble: hide its content INSTANTLY (the
            // actual window teardown is poll-driven via the registry mirror, up to ~200ms
            // later, so without this the closed bubble would linger as a frozen chat).
            property bool dismissing: false
            // Remembered full-chat size so a manual grip-resize survives minimize/restore.
            // (Window size is applied IMPERATIVELY in _applyMode — NOT bound to `minimized`
            // — because a declarative width/height binding fights the compositor-driven
            // startSystemResize: the binding would snap the window back to 380x520 on the
            // first minimize/restore, silently discarding the user's resize.)
            property real fullW: mgr._rememberedW()
            property real fullH: mgr._rememberedH()

            // Apply the current mode's geometry imperatively. On minimize we first capture
            // the (possibly user-resized) full size; on restore we lift the min/max ceiling
            // BEFORE growing so the window isn't clamped at 64.
            function _applyMode() {
                if (slot.minimized) {
                    if (bubbleWin.width > 64) slot.fullW = bubbleWin.width
                    if (bubbleWin.height > 64) slot.fullH = bubbleWin.height
                    bubbleWin.minimumWidth = 64;  bubbleWin.minimumHeight = 64
                    bubbleWin.maximumWidth = 64;  bubbleWin.maximumHeight = 64
                    bubbleWin.width = 64;         bubbleWin.height = 64
                } else {
                    bubbleWin.maximumWidth = 16777215; bubbleWin.maximumHeight = 16777215
                    bubbleWin.minimumWidth = 280;      bubbleWin.minimumHeight = 240
                    bubbleWin.width = slot.fullW;       bubbleWin.height = slot.fullH
                }
            }
            onMinimizedChanged: _applyMode()

            // Always-visible window — show()/raise()/requestActivate() are best-effort on
            // Wayland but harmless (it's already mapped + always-on-top).
            function surface() {
                bubbleWin.show()
                bubbleWin.raise()
                bubbleWin.requestActivate()
            }
            function restore() { slot.minimized = false; slot.surface() }
            function dismiss() {
                slot.dismissing = true            // hide content now; teardown follows via the mirror
                bubbleRoom.close()
                if (mgr.floating) mgr.floating.closeBubble(slot.roomId)
                // 10.1 (TH-FLOAT-12): drop the row NOW — the mirror's 200 ms poll used to leave an
                // invisible, click-eating window behind. _syncModel() is idempotent, so the poll is a no-op.
                for (var i = bubbleModel.count - 1; i >= 0; i--) if (bubbleModel.get(i).roomId === slot.roomId) { bubbleModel.remove(i); break }
            }

            // Independent per-bubble timeline source (its own sync/key task). Opened once.
            RoomController { id: bubbleRoom }
            Component.onCompleted: bubbleRoom.open(roomId)

            Window {
                id: bubbleWin
                // title flips so KWin's title-matched rules can still apply per-mode (KWin
                // matches wmclass+title, not objectName). objectName is internal-only but
                // kept stable per mode. Both are plain property changes — no recreation.
                objectName: (slot.minimized ? "bubblehead_" : "bubble_") + slot.roomId
                title: (slot.meta.name.length > 0 ? slot.meta.name : "Chat")
                       + (slot.minimized ? " — head" : " — Chat")
                visible: true
                // Initial (full-mode) geometry. Size is then driven IMPERATIVELY by
                // slot._applyMode() on minimize/restore — see fullW/fullH above — so it
                // never fights the grip's startSystemResize.
                width: slot.fullW
                height: slot.fullH
                onWidthChanged: if (!slot.minimized && width > 64) mgr.noteBubbleSize(width, height)
                onHeightChanged: if (!slot.minimized && height > 64) mgr.noteBubbleSize(width, height)
                minimumWidth: 280
                minimumHeight: 240
                // CONSTANT transparent surface (alpha buffer present from creation, so the
                // head's round corners are truly transparent) — full mode paints its own
                // opaque panel on top. Toggling color at runtime can't re-negotiate alpha.
                color: "transparent"
                // CONSTANT flags => the platform window is never recreated. Frameless (we
                // draw our own chrome) + always-on-top (a deliberately-floating overlay).
                flags: Qt.FramelessWindowHint | Qt.WindowStaysOnTopHint | Qt.Window

                // 10.1 (TH-FLOAT-08): a photo or a toast from THIS chat used to land in the main window
                // (behind the game). The bubble hosts its own lightbox and a small toast pill.
                function openLightbox(url, w, h, alt) { bubbleLightbox.show(url, w, h, alt) }
                function showPassiveNotification(msg, t) { bubbleToast.text = String(msg); bubbleToast.shown = true; bubbleToastT.restart() }
                Lightbox { id: bubbleLightbox }
                Rectangle {
                    id: bubbleToast
                    property bool shown: false
                    property alias text: bubbleToastLabel.text
                    z: 9000
                    anchors.horizontalCenter: parent.horizontalCenter
                    anchors.bottom: parent.bottom
                    anchors.bottomMargin: 64
                    width: Math.min(parent.width - 24, bubbleToastLabel.implicitWidth + 24)
                    height: bubbleToastLabel.implicitHeight + 14
                    radius: height / 2
                    color: Th.chatHeadToastBg
                    border.width: 1; border.color: Th.chatHeadToastBorder
                    opacity: shown ? 1 : 0
                    visible: opacity > 0.01
                    Behavior on opacity { NumberAnimation { duration: Motion.durSlow } }
                    Controls.Label { id: bubbleToastLabel; anchors.centerIn: parent; width: Math.min(implicitWidth, bubbleWin.width - 48); elide: Text.ElideRight; color: Th.chatHeadToastFg; font.pointSize: Kirigami.Theme.smallFont.pointSize }
                    Timer { id: bubbleToastT; interval: 2600; onTriggered: bubbleToast.shown = false }
                }

                onClosing: {
                    // Abort the bubble's live sync + typing tasks WHILE the RoomController
                    // QObject is still alive, then free the registry slot.
                    bubbleRoom.close()
                    if (mgr.floating) mgr.floating.closeBubble(slot.roomId)
                }

                // ============================ FULL CHAT ============================
                // Opaque panel fills the (transparent) surface in full mode.
                Rectangle {
                    anchors.fill: parent
                    visible: !slot.minimized && !slot.dismissing
                    color: Th.chatHeadWindowBg
                    border.width: 1
                    border.color: Th.chatHeadWindowBorder
                    // Kirigami.Theme customization does NOT cross window boundaries —
                    // re-declare the app scheme per bubble window or every stock QQC2
                    // control here renders in the SYSTEM color scheme.
                    Kirigami.Theme.inherit: false
                    Kirigami.Theme.backgroundColor: Th.surfaceWindow
                    Kirigami.Theme.textColor: Th.textPrimary
                    Kirigami.Theme.disabledTextColor: Th.textSecondary
                    Kirigami.Theme.highlightColor: Th.accentBase
                    Kirigami.Theme.highlightedTextColor: Th.textOnAccent
                    Kirigami.Theme.alternateBackgroundColor: Th.surfaceIsland
                    Kirigami.Theme.activeTextColor: Th.accentBright
                    Kirigami.Theme.linkColor: Th.textLink
                    Kirigami.Theme.positiveTextColor: Th.stateSuccess
                    Kirigami.Theme.negativeTextColor: Th.stateDangerBright
                    Kirigami.Theme.hoverColor: Th.accentBase
                    Kirigami.Theme.focusColor: Th.accentBase

                    ColumnLayout {
                        anchors.fill: parent
                        anchors.margins: 1   // sit inside the 1px frame border
                        spacing: 0

                        // App-drawn title bar (frameless => no KWin titlebar): drag to move
                        // (compositor), minimize-to-head, close.
                        Rectangle {
                            Layout.fillWidth: true
                            implicitHeight: Kirigami.Units.gridUnit * 2.2
                            color: Th.chatHeadTitleBarBg
                            // A bare DragHandler with target:null + startSystemMove lets the
                            // compositor move the window; the app can't self-position on Wayland.
                            DragHandler {
                                target: null
                                onActiveChanged: if (active) bubbleWin.startSystemMove()
                            }
                            RowLayout {
                                anchors.fill: parent
                                anchors.leftMargin: Kirigami.Units.smallSpacing
                                anchors.rightMargin: Kirigami.Units.smallSpacing
                                spacing: Kirigami.Units.smallSpacing
                                Avatar {
                                    app: mgr.app
                                    size: Kirigami.Units.gridUnit * 1.4
                                    name: slot.meta.name
                                    seed: Seeds.avatarSeed(slot.roomId, mgr.app ? mgr.app.dmPeerOf(slot.roomId) : "")   // §6
                                    imageSource: slot.meta.avatar || (mgr.app ? mgr.app.dmPictureOf(slot.roomId) : "")
                                }
                                Controls.Label {
                                    textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                    Layout.fillWidth: true
                                    text: slot.meta.name.length > 0 ? slot.meta.name : "Chat"
                                    color: Th.chatHeadTitleBarFg
                                    font.bold: true
                                    elide: Text.ElideRight
                                }
                                HeaderIconButton {
                                    app: mgr.app
                                    iconName: "window-minimize-symbolic"
                                    tip: "Minimize to chat-head"
                                    onClicked: slot.minimized = true
                                }
                                HeaderIconButton {
                                    app: mgr.app
                                    iconName: "window-close-symbolic"
                                    tip: "Close chat"
                                    onClicked: slot.dismiss()
                                }
                            }
                        }

                        Kirigami.Separator { Layout.fillWidth: true }

                        // The receipt-free conversation.
                        ConversationView {
                            Layout.fillWidth: true
                            Layout.fillHeight: true
                            app: mgr.app
                            host: bubbleWin   // 10.1 (TH-FLOAT-08): lightbox + toasts stay in this window
                            room: bubbleRoom
                            gif: mgr.gif
                            // 10.1 (TH-FLOAT-08): an avatar tap was dead in a bubble — open the profile in the main window.
                            onAvatarActivated: (mxid) => { if (mgr.app && mgr.app.openProfileFlyout) mgr.app.openProfileFlyout(mxid) }
                            // Thread the RESOLVED name (same source as the window title) so the
                            // composer placeholder shows "Message AetherBot", never a raw room id.
                            displayName: slot.meta.name
                        }
                    }

                    // Bottom-right resize grip (frameless => no KWin resize borders). Uses
                    // the compositor's interactive resize (Wayland-safe).
                    Item {
                        width: 16; height: 16
                        anchors.right: parent.right
                        anchors.bottom: parent.bottom
                        z: 10
                        DragHandler {
                            target: null
                            onActiveChanged: if (active) bubbleWin.startSystemResize(Qt.RightEdge | Qt.BottomEdge)
                        }
                        HoverHandler { cursorShape: Qt.SizeFDiagCursor }
                        Controls.Label {
                            anchors.centerIn: parent
                            text: "◢"   // ◢ lower-right triangle grip hint
                            color: Qt.alpha(Th.chatHeadGripFg, 1)   // the token's alpha is the item's opacity (§3.8)
                            opacity: Th.chatHeadGripFg.a
                            font.pixelSize: 12
                        }
                    }
                }

                // ============================ CHAT-HEAD ============================
                Item {
                    anchors.fill: parent
                    visible: slot.minimized && !slot.dismissing

                    // Drag past the threshold => compositor move; a plain tap (no movement)
                    // falls through to the restore TapHandler. Same recipe as the titlebar.
                    DragHandler {
                        target: null
                        onActiveChanged: if (active) bubbleWin.startSystemMove()
                    }
                    TapHandler {
                        acceptedButtons: Qt.LeftButton
                        onTapped: slot.restore()
                    }
                    // Right/middle-click dismisses without restoring first.
                    TapHandler {
                        acceptedButtons: Qt.RightButton | Qt.MiddleButton
                        onTapped: slot.dismiss()
                    }
                    HoverHandler { cursorShape: Qt.PointingHandCursor }

                    Rectangle {
                        anchors.fill: parent
                        radius: width / 2
                        color: Th.chatHeadHeadBg
                        border.width: 2
                        border.color: Th.chatHeadHeadBorder

                        Avatar {
                            anchors.centerIn: parent
                            app: mgr.app
                            size: 50
                            name: slot.meta.name
                            seed: Seeds.avatarSeed(slot.roomId, mgr.app ? mgr.app.dmPeerOf(slot.roomId) : "")   // §6
                            imageSource: slot.meta.avatar || (mgr.app ? mgr.app.dmPictureOf(slot.roomId) : "")
                        }

                        // Unread badge (top-right) — the app-wide badge component.
                        UnreadBadge {
                            anchors.right: parent.right
                            anchors.top: parent.top
                            app: mgr.app
                            count: slot.meta.unread
                            highlight: slot.meta.highlight
                        }

                        // ALWAYS-VISIBLE close affordance (top-left). The old ✕ was hover-
                        // gated and never appeared because the head never got a hover state.
                        // A MouseArea takes the exclusive grab so closing never also restores.
                        Rectangle {
                            anchors.left: parent.left
                            anchors.top: parent.top
                            // 22px dark-circle ✕ — the app's one drawn-close size (matches
                            // the ThEmbedCard overlay ✕).
                            width: 22; height: 22
                            radius: width / 2
                            z: 5
                            color: Th.chatHeadCloseBg
                            border.width: 1
                            border.color: Th.chatHeadCloseFg
                            ThIcon {   // 10.1 (TH-ICON-04): the pack's close glyph, like ThEmbedCard's
                                anchors.centerIn: parent
                                width: 12; height: 12
                                app: mgr.app
                                name: "window-close-symbolic"
                                color: Th.chatHeadCloseFg
                                isMask: true
                            }
                            MouseArea {
                                anchors.fill: parent
                                cursorShape: Qt.PointingHandCursor
                                onClicked: slot.dismiss()
                            }
                        }
                    }
                }
            }
        }
    }

    // ---- Smoke-test hook: env TINHAT_BUBBLE_TEST=<roomId> auto-opens one bubble so the
    // QML (ConversationView + bubble Window) is proven to instantiate at runtime. ----
    Component.onCompleted: {
        if (floating && floating.selfTest && floating.selfTest.length > 0) {
            console.log("[floating] TINHAT_BUBBLE_TEST -> opening bubble for", floating.selfTest)
            floating.openBubble(floating.selfTest)
        }
    }
}
