// SoundboardPopover.qml — the soundboard as a product (0.8.22).
//
// USAGE it is built for: mid-call, "I need that clip in under two seconds", ten times an
// evening. So: it lives INSIDE the main window as a popover above the call bar (never lost
// behind the app), opens from the call-bar button / the composer button / Ctrl+Shift+S,
// Esc closes it, one search box on top filters your sounds instantly and searches the web
// from two letters, favourites carry 1-9 hotkeys, the playing card shows a progress ring,
// and you can upload your own clips (button or drop a file on it). Playback and publish
// both come from the server's clip cache (0.8.21), so the board no longer depends on
// myinstants being reachable for what you already saved.
//
// Public surface kept from SoundboardPanel (Main.qml callers unchanged): roomId,
// roomSounds, personalSounds, searchResults, searching, errorText, callPlaying,
// playingUrl, playingName, playStartedAt, localNote, openOrRaise(), stopCurrent(),
// startSound(url, name), the playSound/stopSound/add*/remove*/searchSounds signals.
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import QtQuick.Shapes
import QtQuick.Dialogs
import QtMultimedia
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the soundboard.* tokens

Controls.Popup {
    id: sheet
    required property var app
    property string roomId: ""
    property var roomSounds: []      // [{stateKey,name,sourceUrl,emoji,duration,setBy}]
    property var personalSounds: []  // [{name,sourceUrl,emoji,duration}] — the FAVOURITES
    property var searchResults: []   // [{name,url,duration}]
    property bool searching: false
    property string errorText: ""
    property bool callPlaying: false // mirrored: the call's "sound" track is live
    property bool uploading: false
    readonly property string myId: app.mMyId || ""

    signal playSound(string url)
    signal stopSound()
    signal addRoomSound(string name, string url, string emoji, int duration)
    signal removeRoomSound(string stateKey)
    signal addPersonalSound(string name, string url, string emoji, int duration)
    signal removePersonalSound(int index)
    signal searchSounds(string query)
    signal uploadRequested(string fileUrl, string name)

    parent: Controls.Overlay.overlay
    modal: false
    dim: false
    focus: true
    closePolicy: Controls.Popup.CloseOnEscape | Controls.Popup.CloseOnPressOutside
    width: Math.min(680, (parent ? parent.width : 680) - 40)
    // As tall as its content, capped — an empty board is a compact card, a full one scrolls.
    height: Math.min((parent ? parent.height : 700) - 140, Math.max(220, implicitHeight))
    x: parent ? Math.round((parent.width - width) / 2) : 0
    y: parent ? Math.max(16, parent.height - height - 120) : 0
    padding: 0

    function openOrRaise() {
        if (!visible) open()
        Qt.callLater(function() { searchField.forceActiveFocus() })
    }
    onOpened: { searchField.forceActiveFocus(); if (app.mRoomId && app.mRoomId.length > 0) app.soundboardRefresh() }

    // ---- playback state (identical model to 0.8.21) ----
    property string playingUrl: ""
    property string playingName: ""
    property double playStartedAt: 0
    property string localNote: ""
    property string query: ""
    function matches(d) {
        var q = sheet.query.trim().toLowerCase()
        return q.length === 0 || String((d && d.name) || "").toLowerCase().indexOf(q) >= 0
    }
    readonly property var favShown: sheet.personalSounds.filter(sheet.matches)
    readonly property var roomShown: sheet.roomSounds.filter(sheet.matches)
    readonly property var recentShown: (app.mSbRecent || []).filter(sheet.matches)
    readonly property bool webSearchable: sheet.query.trim().length >= 2
    // 11.0 (GPU.md §2.5 determinate progress, §2.8 — the web's 4f6d741): the card's ring moves at most 4× a second and
    // only while the motion switch is on (it followed every position update of the player); its timer runs at 1 s while
    // the window is not focused, and nothing moves then
    property real progress: 0
    onPlayingUrlChanged: if (sheet.playingUrl.length === 0) sheet.progress = 0
    Timer {
        interval: Motion.focused ? 250 : 1000; repeat: true
        running: sheet.playingUrl.length > 0
        triggeredOnStart: true
        onTriggered: {
            if (!Motion.animate) return
            var p = localPlayer.duration > 0 ? Math.min(1, localPlayer.position / localPlayer.duration) : 0
            if (Math.abs(p - sheet.progress) > 0.002) sheet.progress = p
        }
    }

    function _localFailed() {
        var inCall = !!(sheet.app && sheet.app.mInCall)
        sheet.localNote = inCall ? "Couldn't play that on your speakers — the call still hears it."
                                 : "Couldn't play that sound."
        if (!inCall) { sheet.playingUrl = ""; sheet.playingName = "" }
    }
    Timer {
        interval: Motion.focused ? 300 : 1000; repeat: true   // 11.0 (GPU.md §2.8): the watchdog ≥ 1 s while not focused
        running: sheet.playingUrl.length > 0
        onTriggered: {
            if (Date.now() - sheet.playStartedAt < 1500) return
            var localBusy = localPlayer.playbackState === MediaPlayer.PlayingState
            if (!localBusy && !sheet.callPlaying) { sheet.playingUrl = ""; sheet.playingName = "" }
        }
    }
    MediaPlayer {
        id: localPlayer
        audioOutput: AudioOutput { id: localOut; volume: 1.0 }
        onMediaStatusChanged: {
            if (mediaStatus === MediaPlayer.EndOfMedia) { sheet.playingUrl = ""; sheet.playingName = "" }
            else if (mediaStatus === MediaPlayer.InvalidMedia) { console.warn("[soundboard] local playback: invalid media", source); sheet._localFailed() }
        }
        onErrorOccurred: function(err, msg) { console.warn("[soundboard] local playback error:", err, msg); sheet._localFailed() }
    }
    // widgets W2 (the Soundboard pad, WIDGETS.md §22): the advisory `use_soundboard` permission — in a call whose roles
    // turned it off for you nothing plays (not even the local copy); the panel and the pad show the same line (red)
    property bool noteIsError: false
    function gate() {
        var c = sheet.app ? sheet.app.soundboardCtl : null
        if (!c) return { allowed: true, hint: "" }
        try { return JSON.parse(String(c.padGate(!!sheet.app.mInCall, JSON.stringify(sheet.app.mCallPerms || {})))) } catch (e) { return { allowed: true, hint: "" } }
    }
    function startSound(url, name, emoji) {
        if (!url || url.length === 0) return
        var g = sheet.gate()
        if (!g.allowed) { sheet.localNote = g.hint; sheet.noteIsError = true; return }
        sheet.noteIsError = false
        stopCurrent()
        var playUrl = sheet.app ? sheet.app.soundboardClipUrl(url) : url
        console.log("[soundboard] play", name || "", "->", playUrl)
        localPlayer.source = ""
        localPlayer.source = playUrl
        localPlayer.play()
        sheet.playingUrl = url
        sheet.playingName = name || ""
        sheet.playStartedAt = Date.now()
        sheet.localNote = ""
        sheet.playSound(url)
        if (sheet.app.sbPushRecent) sheet.app.sbPushRecent(name || "", url, emoji || "")
    }
    function stopCurrent() {
        if (sheet.playingUrl.length === 0) return
        localPlayer.stop()
        sheet.stopSound()
        sheet.playingUrl = ""
        sheet.playingName = ""
    }
    function toggleSound(url, name, emoji) {
        if (url.length > 0 && url === sheet.playingUrl) stopCurrent()
        else startSound(url, name, emoji)
    }
    function isFavourite(url) {
        for (var i = 0; i < sheet.personalSounds.length; i++) if (sheet.personalSounds[i].sourceUrl === url) return true
        return false
    }
    function favouriteIndex(url) {
        for (var i = 0; i < sheet.personalSounds.length; i++) if (sheet.personalSounds[i].sourceUrl === url) return i
        return -1
    }
    function roomEntry(url) {
        for (var i = 0; i < sheet.roomSounds.length; i++) if (sheet.roomSounds[i].sourceUrl === url) return sheet.roomSounds[i]
        return null
    }
    // Hotkeys 1-9 play the first nine favourites while the popover is open.
    function playHotkey(n) {
        var list = sheet.favShown
        if (n >= 1 && n <= list.length) { var d = list[n - 1]; sheet.toggleSound(d.sourceUrl || d.url || "", d.name || "", d.emoji || "") }
    }

    // ---- upload ----
    FileDialog {
        id: uploadDialog
        title: "Add a sound"
        nameFilters: ["Audio (*.mp3 *.ogg *.opus *.wav *.m4a *.webm)", "All files (*)"]
        onAccepted: sheet._uploadUrl(selectedFile)
    }
    function _uploadUrl(fileUrl) {
        var s = String(fileUrl)
        var base = s.substring(s.lastIndexOf("/") + 1).replace(/\.[a-z0-9]+$/i, "")
        try { base = decodeURIComponent(base) } catch (e) {}
        sheet.uploadRequested(s, base.replace(/[_-]+/g, " "))
    }

    background: Rectangle {
        color: Th.soundboardBg
        radius: 16
        border.width: 1
        border.color: Th.soundboardBorder
        // Drop an audio file anywhere on the board to add it.
        DropArea {
            anchors.fill: parent
            onEntered: (drag) => { if (drag.hasUrls) drag.accept() }
            onDropped: (drop) => { if (drop.hasUrls && drop.urls.length > 0) { sheet._uploadUrl(drop.urls[0]); drop.accept() } }
            Rectangle {
                anchors.fill: parent
                visible: parent.containsDrag
                radius: 16
                color: Th.soundboardDropBg
                border.width: 2; border.color: Th.soundboardDropBorder
                Controls.Label { anchors.centerIn: parent; text: "Drop to add this sound"; color: Th.soundboardDropFg; font.bold: true; font.pointSize: 14 }
            }
        }
    }

    // ---- a sound card ----
    component SoundCard: Rectangle {
        id: card
        property string soundName: ""
        property string soundUrl: ""
        property string soundEmoji: ""
        property int hotkey: 0            // 1..9 shows a badge
        property bool removable: false
        property string kind: "fav"       // fav | room | recent | web
        signal remove()
        readonly property bool isPlaying: soundUrl.length > 0 && soundUrl === sheet.playingUrl
        implicitWidth: 108
        implicitHeight: 92
        radius: 12
        color: isPlaying ? Th.soundboardCardBgActive
             : (cardHover.hovered ? Th.soundboardCardBgHover : Th.soundboardCardBg)
        border.width: isPlaying ? 2 : 1
        border.color: isPlaying ? Th.soundboardCardBorderActive : Th.soundboardCardBorder
        Behavior on color { ColorAnimation { duration: Motion.dur } }
        scale: cardTap.pressed ? 0.96 : 1.0
        Behavior on scale { NumberAnimation { duration: Motion.durShort } }
        HoverHandler { id: cardHover }
        TapHandler {
            id: cardTap
            acceptedButtons: Qt.LeftButton
            onTapped: sheet.toggleSound(card.soundUrl, card.soundName, card.soundEmoji)
        }
        TapHandler {
            acceptedButtons: Qt.RightButton
            onTapped: cardMenu.popup()
        }
        // Emoji + progress ring.
        Item {
            anchors.top: parent.top
            anchors.topMargin: 8
            anchors.horizontalCenter: parent.horizontalCenter
            width: 40; height: 40
            Shape {
                anchors.fill: parent
                visible: card.isPlaying
                antialiasing: true
                ShapePath {
                    strokeColor: Th.soundboardProgressTrack; strokeWidth: 3; fillColor: "transparent"; capStyle: ShapePath.RoundCap
                    PathAngleArc { centerX: 20; centerY: 20; radiusX: 18; radiusY: 18; startAngle: -90; sweepAngle: 360 }
                }
                ShapePath {
                    strokeColor: Th.soundboardProgressFill; strokeWidth: 3; fillColor: "transparent"; capStyle: ShapePath.RoundCap
                    PathAngleArc { centerX: 20; centerY: 20; radiusX: 18; radiusY: 18; startAngle: -90; sweepAngle: 360 * sheet.progress }
                }
            }
            Controls.Label {
                anchors.centerIn: parent
                visible: !card.isPlaying
                text: card.soundEmoji.length > 0 ? card.soundEmoji : "🔊"
                font.pointSize: Kirigami.Theme.defaultFont.pointSize * 1.6
            }
            Kirigami.Icon {
                anchors.centerIn: parent
                visible: card.isPlaying
                source: "media-playback-stop"
                color: Th.soundboardCardIconActive
                implicitWidth: 18; implicitHeight: 18
            }
        }
        Controls.Label {
            anchors { left: parent.left; right: parent.right; bottom: parent.bottom; margins: 6 }
            text: card.soundName
            color: Th.soundboardCardFg
            horizontalAlignment: Text.AlignHCenter
            elide: Text.ElideRight
            maximumLineCount: 2
            wrapMode: Text.WordWrap
            font.pointSize: Kirigami.Theme.smallFont.pointSize
        }
        // Hotkey badge.
        Rectangle {
            visible: card.hotkey > 0
            anchors { top: parent.top; left: parent.left; margins: 5 }
            width: 18; height: 18; radius: 5
            color: Th.soundboardHotkeyBg
            border.width: 1; border.color: Th.soundboardHotkeyBorder
            Controls.Label { anchors.centerIn: parent; text: card.hotkey; color: Th.soundboardHotkeyFg; font.bold: true; font.pointSize: Kirigami.Theme.smallFont.pointSize }
        }
        // Remove ✕ on hover (own sounds only).
        Controls.ToolButton {
            z: 1
            visible: card.removable && cardHover.hovered
            anchors { top: parent.top; right: parent.right; margins: 2 }
            icon.name: "edit-delete-remove"
            implicitWidth: 22; implicitHeight: 22
            Controls.ToolTip.text: "Remove"
            Controls.ToolTip.visible: hovered
            onClicked: card.remove()
        }
        Controls.Menu {
            id: cardMenu
            margins: 6
            Controls.MenuItem {
                text: card.isPlaying ? "Stop" : "Play"
                icon.name: card.isPlaying ? "media-playback-stop" : "media-playback-start"
                onTriggered: sheet.toggleSound(card.soundUrl, card.soundName, card.soundEmoji)
            }
            Controls.MenuSeparator {}
            Controls.MenuItem {
                text: sheet.isFavourite(card.soundUrl) ? "Remove from favourites" : "Add to favourites"
                icon.name: sheet.isFavourite(card.soundUrl) ? "starred-symbolic" : "non-starred-symbolic"
                onTriggered: {
                    var i = sheet.favouriteIndex(card.soundUrl)
                    if (i >= 0) sheet.removePersonalSound(i)
                    else sheet.addPersonalSound(card.soundName || "Sound", card.soundUrl, card.soundEmoji, 0)
                }
            }
            Controls.MenuItem {
                readonly property var re: sheet.roomEntry(card.soundUrl)
                enabled: sheet.roomId.length > 0 && (!re || (sheet.myId.length > 0 && re.setBy === sheet.myId))
                text: re ? "Remove from room sounds" : "Add to room sounds"
                icon.name: re ? "list-remove-symbolic" : "list-add-symbolic"
                onTriggered: { if (re) sheet.removeRoomSound(re.stateKey || ""); else sheet.addRoomSound(card.soundName || "Sound", card.soundUrl, card.soundEmoji, 0) }
            }
            Controls.MenuSeparator {}
            Controls.MenuItem {
                text: "Copy link"
                icon.name: "edit-copy-symbolic"
                onTriggered: sheet.app.copyText(card.soundUrl)
            }
        }
    }

    component CardGrid: Flow {
        id: grid
        property var sounds: []
        property bool hotkeys: false
        property bool removableAll: false
        property string kind: "fav"
        signal removeItem(var d, int i)
        Layout.fillWidth: true
        spacing: 8
        Repeater {
            model: grid.sounds
            delegate: SoundCard {
                required property var modelData
                required property int index
                soundName: modelData.name || ""
                soundUrl: modelData.sourceUrl || modelData.url || ""
                soundEmoji: modelData.emoji || ""
                hotkey: grid.hotkeys && index < 9 ? index + 1 : 0
                removable: grid.removableAll || (grid.kind === "room" && sheet.myId.length > 0 && modelData.setBy === sheet.myId)
                kind: grid.kind
                onRemove: grid.removeItem(modelData, index)
            }
        }
    }
    component SectionHead: RowLayout {
        property string title: ""
        property string hint: ""
        Layout.fillWidth: true
        Layout.topMargin: 6
        Controls.Label { text: title; color: Th.soundboardSectionFg; font.bold: true }
        Controls.Label { text: hint; color: Th.soundboardSectionHintFg; font.pointSize: Kirigami.Theme.smallFont.pointSize; Layout.fillWidth: true; elide: Text.ElideRight }
    }

    contentItem: ColumnLayout {
        spacing: 0
        focus: true
        Keys.onPressed: (ev) => {
            if (ev.key >= Qt.Key_1 && ev.key <= Qt.Key_9 && !searchField.activeFocus) { sheet.playHotkey(ev.key - Qt.Key_0); ev.accepted = true }
            else if (ev.key === Qt.Key_Escape) { sheet.close(); ev.accepted = true }
        }
        // ---- header ----
        RowLayout {
            Layout.fillWidth: true
            Layout.margins: 14
            Layout.bottomMargin: 8
            spacing: 10
            Kirigami.Icon { source: "emblem-music-symbolic"; color: Th.soundboardTitleIcon; implicitWidth: 22; implicitHeight: 22 }
            Controls.Label {
                text: "Soundboard"
                color: Th.soundboardTitleFg; font.bold: true
                font.pointSize: Kirigami.Theme.defaultFont.pointSize + 2
            }
            Kirigami.SearchField {
                id: searchField
                Layout.fillWidth: true
                placeholderText: "Search your sounds and the web…"
                onTextChanged: { sheet.query = text; sheet.searchSounds(text.trim().length >= 2 ? text.trim() : "") }
                Keys.onPressed: (ev) => {
                    // Digits type into the search box; Enter plays the first match.
                    if (ev.key === Qt.Key_Return || ev.key === Qt.Key_Enter) { sheet.playHotkey(1); ev.accepted = true }
                }
            }
            Controls.ToolButton {
                icon.name: "list-add-symbolic"
                text: sheet.uploading ? "Uploading…" : "Add"
                enabled: !sheet.uploading
                Controls.ToolTip.text: "Upload your own clip (mp3/ogg/wav, 3 MB) — or drop a file on the board"
                Controls.ToolTip.visible: hovered
                onClicked: uploadDialog.open()
            }
            Controls.ToolButton { icon.name: "window-close-symbolic"; onClicked: sheet.close(); Controls.ToolTip.text: "Close"; Controls.ToolTip.visible: hovered; Accessible.name: "Close" }
        }
        Controls.Label {
            visible: sheet.errorText.length > 0 || sheet.localNote.length > 0
            Layout.fillWidth: true
            Layout.leftMargin: 14; Layout.rightMargin: 14; Layout.bottomMargin: 6
            text: sheet.errorText.length > 0 ? sheet.errorText : sheet.localNote
            color: (sheet.errorText.length > 0 || sheet.noteIsError) ? Th.soundboardStatusFgError : Th.soundboardStatusFg
            wrapMode: Text.WordWrap
        }
        // ---- body ----
        Controls.ScrollView {
            Layout.fillWidth: true
            Layout.fillHeight: true
            contentWidth: availableWidth
            Controls.ScrollBar.horizontal.policy: Controls.ScrollBar.AlwaysOff
            ColumnLayout {
                width: parent.width
                spacing: 6
                Item { Layout.preferredHeight: 2 }
                // Favourites (personal) — hotkeys 1-9
                SectionHead {
                    Layout.leftMargin: 14; Layout.rightMargin: 14
                    title: "Favourites" + (sheet.favShown.length > 0 ? " · " + sheet.favShown.length : "")
                    hint: sheet.favShown.length > 0 ? "press 1-9 to play" : ""
                }
                CardGrid {
                    Layout.leftMargin: 14; Layout.rightMargin: 14
                    visible: sheet.favShown.length > 0
                    sounds: sheet.favShown
                    hotkeys: true
                    removableAll: true
                    kind: "fav"
                    onRemoveItem: (d, i) => sheet.removePersonalSound(sheet.favouriteIndex(d.sourceUrl || d.url || ""))
                }
                Controls.Label {
                    visible: sheet.favShown.length === 0
                    Layout.fillWidth: true; Layout.leftMargin: 14; Layout.rightMargin: 14
                    text: sheet.personalSounds.length === 0
                        ? "Nothing here yet — add a clip with + Add, or search below. Right-click any sound to favourite it."
                        : "No favourites match “" + sheet.query.trim() + "”."
                    color: Th.soundboardEmptyFg; font.pointSize: Kirigami.Theme.smallFont.pointSize; wrapMode: Text.WordWrap
                }
                // Room sounds
                SectionHead {
                    Layout.leftMargin: 14; Layout.rightMargin: 14
                    title: "Room sounds" + (sheet.roomShown.length > 0 ? " · " + sheet.roomShown.length : "")
                    hint: "shared with everyone in this room"
                }
                CardGrid {
                    Layout.leftMargin: 14; Layout.rightMargin: 14
                    visible: sheet.roomShown.length > 0
                    sounds: sheet.roomShown
                    kind: "room"
                    onRemoveItem: (d, i) => sheet.removeRoomSound(d.stateKey || "")
                }
                Controls.Label {
                    visible: sheet.roomShown.length === 0
                    Layout.fillWidth: true; Layout.leftMargin: 14; Layout.rightMargin: 14
                    text: sheet.roomSounds.length === 0 ? "Nothing shared yet — right-click a sound → Add to room sounds."
                                                        : "No room sounds match “" + sheet.query.trim() + "”."
                    color: Th.soundboardEmptyFg; font.pointSize: Kirigami.Theme.smallFont.pointSize; wrapMode: Text.WordWrap
                }
                // Recent
                SectionHead {
                    visible: sheet.recentShown.length > 0
                    Layout.leftMargin: 14; Layout.rightMargin: 14
                    title: "Recent"
                }
                CardGrid {
                    Layout.leftMargin: 14; Layout.rightMargin: 14
                    visible: sheet.recentShown.length > 0
                    sounds: sheet.recentShown
                    kind: "recent"
                }
                // From the web
                SectionHead {
                    Layout.leftMargin: 14; Layout.rightMargin: 14
                    title: "From the web" + (sheet.searchResults.length > 0 ? " · " + sheet.searchResults.length : "")
                    hint: sheet.searching ? "searching…" : (sheet.webSearchable ? "" : "type two letters to search myinstants")
                }
                CardGrid {
                    Layout.leftMargin: 14; Layout.rightMargin: 14
                    visible: sheet.searchResults.length > 0
                    sounds: sheet.searchResults
                    kind: "web"
                }
                Controls.Label {
                    visible: sheet.webSearchable && !sheet.searching && sheet.searchResults.length === 0
                    Layout.fillWidth: true; Layout.leftMargin: 14; Layout.rightMargin: 14
                    text: "No results on the web for “" + sheet.query.trim() + "”."
                    color: Th.soundboardEmptyFg; font.pointSize: Kirigami.Theme.smallFont.pointSize
                }
                Controls.Label {
                    Layout.fillWidth: true; Layout.leftMargin: 14; Layout.rightMargin: 14; Layout.bottomMargin: 12
                    horizontalAlignment: Text.AlignRight
                    text: "Right-click a sound for more · web results by myinstants"
                    color: Th.soundboardFooterFg; opacity: 0.7
                    font.pointSize: Kirigami.Theme.smallFont.pointSize * 0.9
                }
            }
        }
    }
}
