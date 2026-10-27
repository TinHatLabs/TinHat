// WatchPartyWindow.qml — the Media Party window (11.4; the file keeps its name — ids and events are the wire).
// A libmpv/yt-dlp video surface (MpvVideo, registered in video_bridge.cpp) + a now-playing header + a transport bar
// + who's watching + the shared queue + the one box (a URL adds, words search YouTube) + the Browse pane.
//
// 11.3 QOL (the Media Party QOL spec — the web's WatchPartyWindow is the twin, same behaviour + look):
//   layout   WIDE (content ≥ 960 px): the STAGE (header · now playing · video · transport · watching · join) on the
//            left, a 1 px divider (6 px grab, drag = the side's width, double-click resets; pref watchparty-side-w,
//            380 / 300 … min(560, 50 %)) and the SIDE panel (queue · the box · browse) with its own scroll. The video is
//            16:9 as wide as the stage, its height capped so the stage fits the window (≥ 200 px, else the stage
//            scrolls). STACKED (< 960): one column, no width cap, the video's height capped at max(200, H − 160).
//            THEATER (T, pref watchparty-theater): the stage only, the video fills the window above a visible
//            transport. FULLSCREEN (F / double-click / button): OS fullscreen, video + the transport overlay (fades
//            2.5 s after the pointer rests). MINI (I): a 480×270 video-only player on top (Qt.WindowStaysOnTopHint;
//            first at the screen's bottom-right, then where it was left: watchparty-mini-x/-y), the transport on
//            hover; Esc / I / "Back to the full window" restore the exact previous geometry (WindowMemory holds).
//   surface  click = play / pause for everyone (deferred 250 ms), double-click = fullscreen, wheel = my volume;
//            the OSD pill (mine + what OTHERS do: the playback event's sender = playback.by, a mirror-only field).
//   keys     testvectors/mediaparty/shortcuts.json (26) = watchparty.rs MP_SHORTCUTS = the Shortcut items below.
//   queue    drag to reorder (one moveItem write), Now playing / Up next tags, played rows at 55 %, hover actions
//            (Play now · Play next · Remove), a right-click menu, double-click = Play now, the header's time left
//            + Shuffle + Clear played (watchparty.rs queue ops, vectors queueops.json).
//
// Sync is wired end-to-end: the WatchPartyController (Main.qml `watchCtl`) owns the 1s drift clock; it emits
// desiredSeek(posSecs) the MpvVideo consumes, and reads back the local position via reportPosition(). Data + helpers
// arrive via plain props / `app` (CXX-Qt: no direct controller binding; everything is mirrored into root m* props on
// Main.qml). The `window:watchParty` context actions (playPause / next / mute / queue / host.toggle) are run HERE
// (ACTIONS.md §14.1): runAction(id, ctx).
import QtQuick
import QtQuick.Window
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the watchparty.* tokens
import dev.tinhat.mpv   // MpvVideo — the upstream mpvqt item, registered in C++ (video_bridge.cpp)
import QtMultimedia      // 11.4: the song card's 30 s preview (Apple's preview url)

// Pop-out OS window (was a modal Kirigami.OverlaySheet) so watching together no longer blocks
// the rest of the app — mirrors the PiP / floating-chat pop-out windows. Declared once in
// Main.qml; shown via show() and closed on app quit (Main.qml onClosing).
Window {
    id: sheet
    required property var app
    // selfhost: YouTube search / browsing only where the server offers it (watch); pasted links always work, and the
    // Browse pane keeps only History (room state) — the web's BrowsePane `watch` prop
    readonly property bool searchOn: !app || !app.serverOn || app.serverOn("watch")
    onSearchOnChanged: if (!searchOn && browseTab !== "history") browseTab = "history"
    // 10.1 (TH-CAL-22 / TH-MEM-08 / TH-WP-15): remembered size (+ position off Wayland) across runs. 11.3 QOL: held
    // while mini / fullscreen so neither geometry ever becomes the remembered one.
    WindowMemory { id: winMemo; target: sheet; key: "watchparty"; store: (sheet.app && sheet.app.prefStore) ? sheet.app.prefStore : null; hold: sheet.mini || sheet._miniLeaving }
    property string roomId: ""
    property bool inParty: false
    property bool amAdvancer: false
    property var party: ({ queue: [], hostId: "" })   // { queue:[{url,title,addedBy,kind?,source?,art?,artist?,durationSec?,preview?}], hostId }
    property var playback: ({})                         // { url, positionSecs, playing, updatedAt, seekNonce, by }
    property var viewers: []                            // [{userId,joinedAt,lastSeen}]
    property var searchResults: []                      // [{id,url,title,thumbnail}]
    property bool searching: false
    property string errorText: ""
    // 11.4: the last addLink outcome ({status, …}) and whether one is in flight — mirrored from watchCtl.
    property var linkInfo: ({})
    property bool adding: false
    // 11.4 Browse pane (DESIGN §2.4) — mirrored from watchCtl: the last browse() answer, in-flight flag, suggestions,
    // the room's History (CONTRACT §2).
    property var browseInfo: ({})
    property bool browsing: false
    property var suggestions: []
    property var history: []
    property string browseTab: "search"   // search | music | trending | playlists | history
    property string searchType: "video"   // search tab chips: video | music | playlist | channel
    property int trendingCat: 0
    property var listing: null            // the Playlists tab's open playlist / channel
    property bool listingMusic: false     // it came from a YouTube Music link / the Music tab: its items queue as songs
    readonly property var browseItems: (browseInfo && browseInfo.ok && browseInfo.items) ? browseInfo.items : []
    readonly property string browseError: (browseInfo && browseInfo.ok === false && browseInfo.error) ? String(browseInfo.error) : ""
    readonly property string browseNext: (browseInfo && browseInfo.nextPage) ? String(browseInfo.nextPage) : ""
    // 10.1 (TH-WP-03): your own volume / mute for the shared video (the other libmpv window,
    // Memories, always had one). Remembered across runs.
    property real volume: 100
    property bool muted: false
    readonly property var store: (sheet.app && sheet.app.prefStore) ? sheet.app.prefStore : null
    // Main.qml sets this before closing the window on app quit (no "You left the Media Party" toast then).
    property bool quietClose: false
    function applyVolume() {
        if (typeof mpv === "undefined" || !mpv) return
        mpv.setProperty("volume", Math.round(sheet.volume))
        mpv.setProperty("mute", sheet.muted ? "yes" : "no")
    }
    Timer { id: wpVolSave; interval: 800; onTriggered: if (sheet.store) { sheet.store.applyUiPref("watchparty-volume", String(Math.round(sheet.volume))); sheet.store.applyUiPref("watchparty-muted", sheet.muted ? "1" : "0") } }
    onVolumeChanged: { applyVolume(); wpVolSave.restart() }
    onMutedChanged: { applyVolume(); wpVolSave.restart() }
    Component.onCompleted: {
        if (store) {
            var v = parseFloat(store.uiPref("watchparty-volume")); if (!isNaN(v) && v >= 0 && v <= 100) sheet.volume = v
            sheet.muted = store.uiPref("watchparty-muted") === "1"
            // 11.3 QOL: the window-local layout prefs
            var sw = parseInt(store.uiPref("watchparty-side-w")); if (!isNaN(sw) && sw >= 300 && sw <= 560) sheet.sideW = sw
            sheet.theater = store.uiPref("watchparty-theater") === "1"
            sheet.timeRemaining = store.uiPref("watchparty-time-remaining") === "1"
        }
    }
    function savePref(k, v) { if (sheet.store) sheet.store.applyUiPref(k, String(v)) }

    // Bubble user intent up to Main.qml (which owns watchCtl).
    signal joinParty()
    signal leaveParty()
    signal assignHost(string userId)     // 10.1 (TH-WP-05): "" releases the floor
    signal enqueue(string url, string title)
    signal enqueueItem(string itemJson)  // 11.4: a full item (CONTRACT §1)
    signal removeAt(int index)           // 11.4
    signal moveItem(int from, int to)    // 11.4
    signal clearQueue()                  // 11.4 (host-gated here)
    signal moveToNext(int from)          // 11.3 QOL: "Play next" (queue op moveToNext — anyone in the party)
    signal clearPlayed()                 // 11.3 QOL (host-gated here)
    signal shuffleUpcoming()             // 11.3 QOL (host-gated here)
    signal addLink(string text, bool playNow)   // 11.4: the one box / a chat card
    signal enqueueItems(string itemsJson)       // 11.4: "Add all"
    signal browse(string kind, string paramsJson)   // 11.4: search | trending | playlist | channel
    signal suggest(string q)                    // 11.4: autocomplete
    signal playUrl(string url)
    signal playPressed()
    signal pausePressed()
    signal seekTo(string posSecs)        // STRING — never a raw f64 across the boundary
    signal searchVideos(string query)
    signal reportPosition(string posSecs) // mpv time-pos → watchCtl.reportPosition (STRING)
    signal mediaEnded()                   // mpv endFile(EOF) → advance the queue if I'm the advancer
    signal leftByClosing(string partyRoomId)   // 11.3 QOL (§7): closed while watching → the main window's Rejoin toast

    // Apply a REMOTE drift correction to the local mpv surface ONLY (no playback-state
    // write — that already happened on the writer side). Main.qml's onDesiredSeek calls this.
    // posSecs is a STRING; mpv `seek <pos> absolute` accepts it verbatim.
    function applyRemoteSeek(posSecs) {
        if (!mpv || sheet.nowUrl.length === 0)
            return
        // Belt-and-braces clamp: never hand mpv a position outside the media (a stale shared clock once
        // produced 15-day targets; every such `seek` fails with MPV_ERROR_COMMAND and leaves a black surface).
        var p = Number(posSecs)
        if (!isFinite(p) || p < 0)
            p = 0
        if (sheet.mpvDuration > 0 && p > sheet.mpvDuration)
            p = Math.max(0, sheet.mpvDuration - 0.5)
        mpv.command(["seek", String(p), "absolute"])
    }

    // The url after the currently-playing one in the shared queue (or "" if nowUrl isn't in
    // the queue / is the last item). Used for auto-advance on EOF and the skip-forward button.
    function nextQueueUrl() {
        var q = sheet.queue
        if (!q || q.length === 0)
            return ""
        var cur = sheet.nowUrl
        if (cur.length === 0)
            return q[0].url || ""
        for (var i = 0; i < q.length; ++i) {
            if ((q[i].url || "") === cur)
                return (i + 1 < q.length) ? (q[i + 1].url || "") : ""
        }
        return q[0].url || ""
    }

    // 11.3 QOL: every time label is "m:ss", "h:mm:ss" from one hour — watchparty.rs fmt_time (vectors fmtTime).
    function fmtTime(s) {
        var c = (sheet.app && sheet.app.watchCtl) ? sheet.app.watchCtl : null
        return c ? String(c.fmtTime(String(Number(s)))) : "0:00"
    }
    // The badge a queue item shows (medialinks.rs service_label).
    function badgeOf(it) {
        var svc = (it && it.source && it.source.service) ? String(it.source.service) : ""
        if (svc === "spotify") return "Spotify"
        if (svc === "apple") return "Apple Music"
        if (svc === "ytmusic") return "YouTube Music"
        if (svc === "youtube") return "YouTube"
        var k = (it && it.kind) ? String(it.kind) : ""
        return k === "file" ? "File" : (k === "song" ? "Song" : "")
    }
    function openInLabel(svc) {
        return svc === "spotify" ? "Open in Spotify" : (svc === "apple" ? "Open in Apple Music" : (svc === "ytmusic" ? "Open in YouTube Music" : "Open link"))
    }
    // 11.4 Browse: what the tabs ask the controller for (the controller answers in browseInfo).
    function fmtViews(n) {
        n = Number(n || 0)
        if (!n) return ""
        if (n >= 1e6) return (n / 1e6).toFixed(n >= 1e7 ? 0 : 1) + "M views"
        if (n >= 1e3) return (n / 1e3).toFixed(n >= 1e4 ? 0 : 1) + "K views"
        return n + " views"
    }
    // The queue item a browse row adds (watchparty.rs queue_item_of).
    function queueItemOf(it, music) {
        var q = { url: String(it.url || ""), title: String(it.title || ""), addedBy: "", kind: music ? "song" : "video",
                  source: { service: music ? "ytmusic" : "youtube", url: String(it.url || ""), id: String(it.id || "") } }
        if (LinkGate.imageSrc(it.thumb)) q.art = LinkGate.imageSrc(it.thumb)
        if (it.channel) q.artist = String(it.channel)
        if (it.durationSec) q.durationSec = Number(it.durationSec)
        return q
    }
    function runBrowse() {
        if (!sheet.searchOn) return
        var q = searchField.text.trim()
        if (sheet.browseTab === "search" || sheet.browseTab === "music") {
            if (sheet.boxIsLink) return
            sheet.browse("search", JSON.stringify({ q: q, type: sheet.browseTab === "music" ? "music" : sheet.searchType }))
        } else if (sheet.browseTab === "trending") {
            sheet.browse("trending", JSON.stringify({ cat: sheet.trendingCat }))
        }
    }
    function browseMore() {
        var bi = sheet.browseInfo || {}
        if (!bi.ok || !sheet.browseNext || !bi.params) return
        var p = JSON.parse(JSON.stringify(bi.params)); p.page = sheet.browseNext
        sheet.browse(String(bi.kind), JSON.stringify(p))
    }
    function setTab(t) {
        if (!sheet.searchOn && t !== "history") return   // selfhost: only History without the server's search
        if (sheet.browseTab === t) return
        sheet.browseTab = t
        if (t === "trending" || t === "search" || t === "music") sheet.runBrowse()
    }
    function openListing(kind, idOrHandle, music) {
        if (!sheet.searchOn) return   // selfhost: a listing is the server's YouTube browsing
        sheet.browseTab = "playlists"
        sheet.listing = null
        sheet.listingMusic = !!music
        if (kind === "channel" && String(idOrHandle).charAt(0) === "@") sheet.browse("channel", JSON.stringify({ handle: idOrHandle }))
        else sheet.browse(kind, JSON.stringify({ id: idOrHandle }))
    }
    function addAll() {
        var l = sheet.listing
        if (!l || !l.items) return
        var out = []
        for (var i = 0; i < l.items.length; ++i) if (l.items[i].kind === "video") out.push(sheet.queueItemOf(l.items[i], sheet.listingMusic))
        if (out.length > 0) { sheet.enqueueItems(JSON.stringify(out)); for (var j = 0; j < out.length; ++j) sheet.markAdded(out[j].url) }
    }
    // browse answers: pages append; a listing (playlist / channel) lands in `listing`
    property var _browseAcc: []
    onBrowseInfoChanged: {
        var bi = sheet.browseInfo || {}
        if (!bi.ok) return
        var page = bi.params && bi.params.page ? String(bi.params.page) : ""
        if (bi.listing) {
            var l = bi.listing
            if (page && sheet.listing) { var merged = JSON.parse(JSON.stringify(sheet.listing)); merged.items = merged.items.concat(l.items || []); merged.nextPage = l.nextPage || ""; sheet.listing = merged }
            else sheet.listing = l
            return
        }
        sheet._browseAcc = page ? sheet._browseAcc.concat(bi.items || []) : (bi.items || [])
    }
    // the one box → Search / Music (words), Playlists (a pasted playlist / channel link), autocomplete
    Timer { id: browseDebounce; interval: 450; onTriggered: sheet.runBrowse() }
    // 11.3 QOL (§6): a typed playlist / channel link waits 400 ms after the last keystroke (a paste = one request)
    Timer {
        id: listingDebounce
        interval: 400
        onTriggered: {
            var l = sheet.boxLink
            if (l.kind === "playlist" && l.id) sheet.openListing("playlist", l.id, l.service === "ytmusic")
            else if (l.kind === "channel" && (l.handle || l.id)) sheet.openListing("channel", l.handle || l.id, false)
        }
    }
    function onBoxChanged() {
        sheet.addNote = ""
        var l = sheet.boxLink
        if (l.kind === "text") {
            listingDebounce.stop()
            if (!sheet.searchOn) return   // selfhost: no search on this server — words wait for a link
            if (sheet.browseTab !== "search" && sheet.browseTab !== "music") sheet.browseTab = "search"
            sheet.suggest(searchField.text)
            browseDebounce.restart()
        } else {
            sheet.suggest("")
            if (sheet.searchOn && ((l.kind === "playlist" && l.id) || (l.kind === "channel" && (l.handle || l.id)))) listingDebounce.restart()
            else listingDebounce.stop()
        }
    }

    // A member of the party's room (name + avatar) — from the room on screen's member list.
    function memberOf(userId) {
        var list = (sheet.app && sheet.app.mMembers) ? sheet.app.mMembers : []
        for (var i = 0; i < list.length; ++i) if (list[i].mxid === userId) return list[i]
        return null
    }
    function nameOf(userId) { var m = memberOf(userId); return (m && m.name) ? String(m.name) : (sheet.app ? sheet.app.localpart(userId) : userId) }
    function avatarOf(userId) { var m = memberOf(userId); return (m && m.avatar) ? String(m.avatar) : "" }

    FallbackTheme { id: fallbackTheme }
    readonly property var theme: app ? app : fallbackTheme
    readonly property string myId: (app && app.mUserId) ? app.mUserId : ""
    // 10.1 (TH-WP-05): with a host set only the host drives play / pause / seek / skip; anyone can still queue.
    readonly property string hostId: (party && party.hostId) ? String(party.hostId) : ""
    readonly property bool amHost: hostId.length > 0 && hostId === myId
    readonly property bool canControl: inParty && (hostId.length === 0 || amHost)
    readonly property string hostName: hostId.length > 0 ? sheet.nameOf(hostId) : ""
    // 11.3 QOL: why a control is refused — the OSD / tooltips say it
    readonly property string controlRefusal: !sheet.inParty ? "Join the Media Party first"
                                             : ("Only the host (" + sheet.hostName + ") controls playback")

    // Derived view of the shared clock (all values arrive as strings/ints — no f64 binding).
    readonly property string nowUrl: (playback && playback.url) ? playback.url : ""
    readonly property bool isPlaying: !!(playback && playback.playing)
    readonly property real posSecs: (playback && playback.positionSecs) ? Number(playback.positionSecs) : 0
    readonly property var queue: (party && party.queue) ? party.queue : []
    readonly property bool memoryClock: nowUrl.indexOf("tinhat-memory://") === 0
    // 11.3 QOL: the playing row's index (-1 = nothing plays / not queued) — tags, played rows, Clear played
    readonly property int nowIndex: {
        if (sheet.nowUrl.length === 0) return -1
        var q = sheet.queue
        for (var i = 0; i < q.length; ++i) if ((q[i].url || "") === sheet.nowUrl) return i
        return -1
    }
    // 11.4: the queue entry of the playing url (or a bare one) — the now-playing header.
    readonly property var nowItem: {
        if (sheet.nowUrl.length === 0) return null
        return sheet.nowIndex >= 0 ? sheet.queue[sheet.nowIndex] : { url: sheet.nowUrl, title: "", addedBy: "" }
    }
    // 11.3 QOL (§5): "Queue · 12 · 48 min left" — watchparty.rs upcoming_label (vectors upcomingLabel)
    readonly property string upcomingLabel: (sheet.app && sheet.app.watchCtl && sheet.queue.length > 0)
        ? String(sheet.app.watchCtl.upcomingLabel(JSON.stringify(sheet.queue), sheet.nowUrl)) : ""
    readonly property int upcomingCount: sheet.nowIndex >= 0 ? sheet.queue.length - sheet.nowIndex - 1 : sheet.queue.length

    // Live values polled off the mpv surface (the player is the source of truth for these).
    property real mpvDuration: 0
    property real mpvPos: 0
    property real mpvCacheEnd: 0          // 11.3 QOL: mpv demuxer-cache-time — the scrub bar's buffered band
    property string mLoadedUrl: ""
    property bool mEofSeen: false
    property string playbackError: ""
    property double mLoadStartedAt: 0
    property bool mSawPlayback: false
    // 11.4: mpv `paused-for-cache` → the buffering spinner.
    property bool buffering: false

    // ---- 11.3 QOL (§1) layout -------------------------------------------------------------------------------------
    readonly property bool fullscreen: sheet.visibility === Window.FullScreen
    property bool mini: false
    property bool theater: false            // a LAYOUT (pref watchparty-theater): Esc never leaves it
    property bool timeRemaining: false      // the time label shows "−3:20 / 4:05" (pref watchparty-time-remaining)
    readonly property bool videoOnly: sheet.fullscreen || sheet.mini
    readonly property bool stageOnly: sheet.theater && !sheet.videoOnly
    readonly property bool wide: !sheet.videoOnly && !sheet.stageOnly && sheet.width >= 960
    readonly property bool stacked: !sheet.videoOnly && !sheet.stageOnly && !sheet.wide
    property real sideW: 380
    readonly property real sideWEff: Math.max(300, Math.min(sheet.sideW, 560, sheet.width * 0.5))
    readonly property real pad: (sheet.videoOnly || sheet.stageOnly) ? 0 : Kirigami.Units.largeSpacing
    // a narrow window (< 520 px): 32 px transport buttons, no volume slider (QOL §3); < 400 px of stage: no Theater / Keys
    // buttons either (T / ? still work)
    readonly property bool tightBar: !sheet.fullscreen && sheet.width < 520
    readonly property int gutter: 12        // a constant scrollbar gutter (no 16:9 ⇄ scrollbar oscillation)
    function setTheater(on) { sheet.theater = on; sheet.savePref("watchparty-theater", on ? "1" : "0") }
    function setFullscreen(on) {
        if (on && sheet.mini) sheet.setMini(false)
        sheet.visibility = on ? Window.FullScreen : (sheet._wasMax ? Window.Maximized : Window.Windowed)
        sheet.poke()
    }
    // the last WINDOWED geometry + whether it was maximised: what the mini player restores, exactly
    property var _normal: null
    property bool _wasMax: false
    property var _preMini: null
    property bool _miniLeaving: false
    function noteGeom() {
        if (sheet.mini || sheet._miniLeaving || !sheet.visible) return
        if (sheet.visibility === Window.Windowed) {
            sheet._normal = { x: sheet.x, y: sheet.y, w: sheet.width, h: sheet.height }
            sheet._wasMax = false
        } else if (sheet.visibility === Window.Maximized) sheet._wasMax = true
    }
    // noted 250 ms later: a WM's fullscreen / maximise resize may land before the state change itself
    Timer { id: geomNote; interval: 250; onTriggered: sheet.noteGeom() }
    onXChanged: { geomNote.restart(); if (sheet.mini) miniPosSave.restart() }
    onYChanged: { geomNote.restart(); if (sheet.mini) miniPosSave.restart() }
    onWidthChanged: geomNote.restart()
    onHeightChanged: geomNote.restart()
    onVisibilityChanged: geomNote.restart()
    Timer { id: miniPosSave; interval: 700; onTriggered: if (sheet.mini && winMemo.canPlace) { sheet.savePref("watchparty-mini-x", Math.round(sheet.x)); sheet.savePref("watchparty-mini-y", Math.round(sheet.y)) } }
    function miniPos() {
        var x = parseInt(sheet.store ? sheet.store.uiPref("watchparty-mini-x") : ""), y = parseInt(sheet.store ? sheet.store.uiPref("watchparty-mini-y") : "")
        if (!isNaN(x) && !isNaN(y) && x > -4000 && y > -4000 && x < 16000 && y < 16000) return { x: x, y: y }
        // first time: the bottom-right of the screen's available area, 24 px in
        var right = Math.min(Screen.virtualX + Screen.width, Screen.desktopAvailableWidth)
        var bottom = Math.min(Screen.virtualY + Screen.height, Screen.desktopAvailableHeight)
        return { x: right - 480 - 24, y: bottom - 270 - 24 }
    }
    Timer { id: miniApply; interval: 220; onTriggered: sheet._applyMiniGeom() }
    function _applyMiniGeom() {
        sheet.width = 480; sheet.height = 270
        if (winMemo.canPlace) { var p = sheet.miniPos(); sheet.x = p.x; sheet.y = p.y }
        sheet.raise()
    }
    Timer { id: miniLeft; interval: 600; onTriggered: sheet._miniLeaving = false }
    // the mini player (QOL §1): video only, 480×270, on top (best effort — Wayland compositors may ignore the hint)
    function setMini(on) {
        if (on === sheet.mini) return
        if (on) {
            var here = { x: sheet.x, y: sheet.y, w: sheet.width, h: sheet.height }
            var g = sheet.visibility === Window.Windowed ? here : (sheet._normal || here)
            sheet._preMini = { x: g.x, y: g.y, w: g.w, h: g.h, max: sheet.visibility === Window.Maximized || (sheet.fullscreen && sheet._wasMax) }
            keysCard.visible = false
            var transition = sheet.visibility !== Window.Windowed
            sheet.mini = true                       // holds WindowMemory first
            if (transition) { sheet.showNormal(); miniApply.restart() }   // let the WM leave fullscreen / maximised first
            else sheet._applyMiniGeom()
        } else {
            var p = sheet._preMini
            miniApply.stop()
            sheet._miniLeaving = true               // keep WindowMemory held until the restored geometry settled
            sheet.mini = false
            if (p) {
                sheet.width = p.w; sheet.height = p.h
                if (winMemo.canPlace) { sheet.x = p.x; sheet.y = p.y }
                if (p.max) sheet.showMaximized()
            }
            sheet._preMini = null
            miniLeft.restart()
        }
        sheet.poke()
    }
    flags: sheet.mini ? (Qt.Window | Qt.WindowStaysOnTopHint) : Qt.Window
    // the transport overlay (fullscreen: fades 2.5 s after the pointer rests; mini: on hover)
    property bool chromeIdle: false
    readonly property bool chromeShown: !sheet.videoOnly || (sheet.mini ? (winHover.hovered || keysCard.visible) : !sheet.chromeIdle)
    Timer { id: chromeIdleT; interval: 2500; onTriggered: if (sheet.fullscreen && !transportHover.hovered) sheet.chromeIdle = true }
    function poke() { sheet.chromeIdle = false; if (sheet.fullscreen) chromeIdleT.restart() }
    // 11.4: the one box's text classified (medialinks.rs via watchCtl.classifyLink) — a URL shows Add / Play now.
    readonly property var boxLink: (sheet.app && sheet.app.watchCtl) ? JSON.parse(sheet.app.watchCtl.classifyLink(searchField.text)) : ({ kind: "text" })
    readonly property bool boxIsLink: boxLink && boxLink.kind !== "text"
    property string addNote: ""
    // 11.4 (DESIGN §2.5): a Spotify / Apple link that could not be queued (no YouTube match, an album / playlist) stays
    // a card: {link, meta, message}; null = none.
    property var songCard: null
    MediaPlayer { id: previewPlayer; audioOutput: AudioOutput {} }
    readonly property bool previewPlaying: previewPlayer.playbackState === MediaPlayer.PlayingState
    function togglePreview(url) {
        if (sheet.previewPlaying) { previewPlayer.stop(); return }
        previewPlayer.source = url; previewPlayer.play()
    }
    function shareToChat() {
        var it = sheet.nowItem
        if (!it || !sheet.app || !sheet.app.room) return
        var link = (it.source && it.source.url) ? String(it.source.url) : String(it.url || "")
        var what = [String(it.title || it.url || ""), it.artist ? String(it.artist) : ""].filter(function (x) { return x.length > 0 }).join(" — ")
        sheet.app.room.send("🎬 Media Party: " + what + "\n" + link)
        sheet.app.showPassiveNotification("Shared to the chat", 2000)
    }

    width: Kirigami.Units.gridUnit * 32
    height: Kirigami.Units.gridUnit * 40
    minimumWidth: sheet.mini ? 240 : 360
    minimumHeight: sheet.mini ? 135 : 400
    color: Th.watchpartyBg
    title: "Media Party — Tin Hat"
    objectName: "watchparty"   // stable id for KWin rules
    visible: false             // shown via show() from Main.qml

    // Open (first time) or re-raise (already open). 11.4: the FIRST open (no remembered position) snaps the window
    // beside the main window (right of it when that fits the screen, else left), DESIGN §2.3.
    function openOrRaise() {
        if (sheet.visible) {
            sheet.raise(); sheet.requestActivate()
        } else {
            searchField.text = ""
            sheet.addNote = ""
            if (winMemo.canPlace && sheet.app && (!sheet.store || String(sheet.store.uiPref("watchparty-x")).length === 0)) {
                var mw = sheet.app
                var gap = 8
                var sw = Screen.desktopAvailableWidth
                if (mw.x + mw.width + gap + sheet.width <= sw) sheet.x = mw.x + mw.width + gap
                else if (mw.x - gap - sheet.width >= 0) sheet.x = mw.x - gap - sheet.width
                else sheet.x = Math.max(0, sw - sheet.width)
                sheet.y = Math.max(0, mw.y)
            }
            if (winMemo.wantMaximized) sheet.showMaximized(); else sheet.show()
            sheet.raise(); sheet.requestActivate()
            sheet._osdArmAt = Date.now() + 2500
            Qt.callLater(function() { searchField.forceActiveFocus() })
        }
    }

    // Stop the player when the window closes — otherwise libmpv keeps DECODING + playing AUDIO with no visible
    // surface. Always leave on close (leaveParty is idempotent; not gated on the lagging inParty mirror).
    // 11.3 QOL (§7): closing (not the Leave button) while watching something → the main window's Rejoin toast; the
    // mini / fullscreen geometry is left first so the next open is the normal window.
    onClosing: {
        var watching = sheet.inParty && sheet.nowUrl.length > 0 && !sheet.memoryClock && !sheet.quietClose
        var rid = (sheet.app && sheet.app.watchCtl) ? String(sheet.app.watchCtl.roomId || "") : ""
        keysCard.visible = false
        if (sheet.mini) sheet.setMini(false)
        if (sheet.fullscreen) sheet.setFullscreen(false)
        if (mpv) { mpv.command(["stop"]); sheet.mLoadedUrl = "" }
        sheet.leaveParty()
        if (watching && rid.length > 0) sheet.leftByClosing(rid)
    }
    onVisibleChanged: { if (visible && mpv) mpv.loadCurrent(); if (visible && !searchOn && browseTab !== "history") browseTab = "history" }
    onInPartyChanged: if (inParty) sheet._osdArmAt = Date.now() + 2500

    // ---- the window's own actions (window:watchParty context, ACTIONS.md §14.1) ------------------------------------
    // 11.3 QOL: my last play / pause counts until the shared clock answers (≤ 5 s) — on a slow server two quick
    // presses used to read the same stale clock and both "play"
    property var _localPlaying: null
    readonly property bool playingNow: sheet._localPlaying !== null ? sheet._localPlaying : sheet.isPlaying
    Timer { id: localPlayingT; interval: 5000; onTriggered: sheet._localPlaying = null }
    onIsPlayingChanged: { sheet._localPlaying = null; localPlayingT.stop() }
    function togglePlay() {
        if (sheet.nowUrl.length === 0) return
        if (!sheet.canControl) { sheet.showOsd(sheet.controlRefusal, "user-identity"); return }
        var willPlay = !sheet.playingNow
        mpv.setProperty("pause", !willPlay)   // optimistic local response; the shared write follows
        if (willPlay) sheet.playPressed(); else sheet.pausePressed()
        sheet._localPlaying = willPlay; localPlayingT.restart()
        sheet.showOsd(willPlay ? "Playing" : "Paused", willPlay ? "media-playback-start" : "media-playback-pause")
    }
    function seekToSecs(x, osdText) {
        if (sheet.nowUrl.length === 0) return
        if (!sheet.canControl) { sheet.showOsd(sheet.controlRefusal, "user-identity"); return }
        x = Math.max(0, x)
        if (sheet.mpvDuration > 0) x = Math.min(x, Math.max(0, sheet.mpvDuration - 0.5))
        mpv.command(["seek", x.toFixed(3), "absolute"])
        sheet.seekTo(x.toFixed(3))
        sheet.showOsd(osdText.length > 0 ? osdText : "Jumped to " + sheet.fmtTime(x), osdText.indexOf("−") === 0 ? "media-seek-backward" : "media-seek-forward")
    }
    function seekBy(secs) { sheet.seekToSecs(sheet.mpvPos + secs, (secs < 0 ? "−" : "+") + Math.abs(secs) + " s") }
    function jumpPct(pct) { if (sheet.mpvDuration > 0) sheet.seekToSecs(sheet.mpvDuration * pct / 100, "") }
    function stepVolume(d) {
        var v = Math.max(0, Math.min(100, Math.round(sheet.volume) + d))
        sheet.volume = v
        if (sheet.muted && d > 0) sheet.muted = false
        sheet.showOsd(sheet.muted || v === 0 ? "Muted" : "Volume " + v + " %", sheet.muted || v === 0 ? "audio-volume-muted-symbolic" : "audio-volume-high-symbolic")
    }
    function toggleMute() {
        sheet.muted = !sheet.muted
        sheet.showOsd(sheet.muted ? "Muted" : "Unmuted", sheet.muted ? "audio-volume-muted-symbolic" : "audio-volume-high-symbolic")
    }
    function playNext() {
        var u = sheet.nextQueueUrl()
        if (u.length === 0) return
        if (!sheet.canControl) { sheet.showOsd(sheet.controlRefusal, "user-identity"); return }
        sheet.playUrl(u)
    }
    function playNowAt(i) {
        if (i < 0 || i >= sheet.queue.length) return
        if (!sheet.canControl) { sheet.showOsd(sheet.controlRefusal, "user-identity"); return }
        sheet.playUrl(String(sheet.queue[i].url || ""))
    }
    function runAction(id, ctx) {
        ctx = ctx || {}
        switch (id) {
        case "watchParty.playPause": sheet.togglePlay(); return true
        case "watchParty.next": sheet.playNext(); return true
        case "watchParty.mute": sheet.toggleMute(); return true
        case "watchParty.host.toggle": if (sheet.inParty) sheet.assignHost(sheet.amHost ? "" : sheet.myId); return true
        case "watchParty.queue": {
            var url = ctx.subject ? String(ctx.subject) : ""
            if (url.length === 0) return true
            var op = (ctx.params && ctx.params.op) ? String(ctx.params.op) : "queue"
            if (op === "playNow") { if (sheet.canControl) sheet.playUrl(url) }
            else sheet.enqueue(url, (ctx.params && ctx.params.title) ? String(ctx.params.title) : "")
            return true
        }
        }
        return false
    }
    // Esc (QOL §4): the keys card → fullscreen → the mini player → close. Theater is a layout: Esc never leaves it.
    function handleEscape() {
        if (keysCard.visible) keysCard.visible = false
        else if (sheet.fullscreen) sheet.setFullscreen(false)
        else if (sheet.mini) sheet.setMini(false)
        else sheet.close()
    }
    // "/" focuses the box from anywhere — it lives in the side / stacked column, so leave the video-only views first
    function focusBox() {
        if (sheet.mini) sheet.setMini(false)
        if (sheet.fullscreen) sheet.setFullscreen(false)
        if (sheet.theater) sheet.setTheater(false)
        keysCard.visible = false
        Qt.callLater(function () { searchField.forceActiveFocus(); searchField.selectAll() })
    }
    // keys (testvectors/mediaparty/shortcuts.json = MP_SHORTCUTS): never while typing in the one box
    readonly property bool keysOn: sheet.visible && !searchField.activeFocus
    Shortcut { sequence: "Space"; enabled: sheet.keysOn; onActivated: sheet.runAction("watchParty.playPause") }
    Shortcut { sequence: "K"; enabled: sheet.keysOn; onActivated: sheet.runAction("watchParty.playPause") }
    Shortcut { sequence: "Left"; enabled: sheet.keysOn; onActivated: sheet.seekBy(-5) }
    Shortcut { sequence: "Right"; enabled: sheet.keysOn; onActivated: sheet.seekBy(5) }
    Shortcut { sequence: "J"; enabled: sheet.keysOn; onActivated: sheet.seekBy(-10) }
    Shortcut { sequence: "L"; enabled: sheet.keysOn; onActivated: sheet.seekBy(10) }
    Shortcut { sequence: "0"; enabled: sheet.keysOn; onActivated: sheet.jumpPct(0) }
    Shortcut { sequence: "1"; enabled: sheet.keysOn; onActivated: sheet.jumpPct(10) }
    Shortcut { sequence: "2"; enabled: sheet.keysOn; onActivated: sheet.jumpPct(20) }
    Shortcut { sequence: "3"; enabled: sheet.keysOn; onActivated: sheet.jumpPct(30) }
    Shortcut { sequence: "4"; enabled: sheet.keysOn; onActivated: sheet.jumpPct(40) }
    Shortcut { sequence: "5"; enabled: sheet.keysOn; onActivated: sheet.jumpPct(50) }
    Shortcut { sequence: "6"; enabled: sheet.keysOn; onActivated: sheet.jumpPct(60) }
    Shortcut { sequence: "7"; enabled: sheet.keysOn; onActivated: sheet.jumpPct(70) }
    Shortcut { sequence: "8"; enabled: sheet.keysOn; onActivated: sheet.jumpPct(80) }
    Shortcut { sequence: "9"; enabled: sheet.keysOn; onActivated: sheet.jumpPct(90) }
    Shortcut { sequence: "Up"; enabled: sheet.keysOn; onActivated: sheet.stepVolume(5) }
    Shortcut { sequence: "Down"; enabled: sheet.keysOn; onActivated: sheet.stepVolume(-5) }
    Shortcut { sequence: "M"; enabled: sheet.keysOn; onActivated: sheet.runAction("watchParty.mute") }
    Shortcut { sequence: "N"; enabled: sheet.keysOn; onActivated: sheet.runAction("watchParty.next") }
    Shortcut { sequence: "F"; enabled: sheet.keysOn; onActivated: sheet.setFullscreen(!sheet.fullscreen) }
    Shortcut { sequence: "T"; enabled: sheet.keysOn; onActivated: sheet.setTheater(!sheet.theater) }
    Shortcut { sequence: "I"; enabled: sheet.keysOn; onActivated: sheet.setMini(!sheet.mini) }
    Shortcut { sequence: "/"; enabled: sheet.keysOn; onActivated: sheet.focusBox() }
    Shortcut { sequence: "?"; enabled: sheet.keysOn; onActivated: keysCard.visible = !keysCard.visible }
    // 10.1 (TH-A11Y-12): Escape closes every secondary window — 11.3 QOL: the keys card / fullscreen / mini first.
    // In the one box Esc is the box's own (suggestions → clear → blur) and never closes the window.
    Shortcut { sequence: "Escape"; enabled: sheet.keysOn; onActivated: sheet.handleEscape() }

    // ---- 11.3 QOL (§2) the OSD: one pill at the top-centre of the video, 1.2 s then a 200 ms fade (none with reduced
    // motion); the newest replaces the current one. Mine (keys / clicks / wheel) and what OTHERS do (below).
    function showOsd(text, icon) {
        osd.text = String(text)
        osd.icon = icon ? String(icon) : ""
        osd.fading = false
        osd.opacity = 1
        osdT.restart()
    }
    Timer { id: osdT; interval: 1200; onTriggered: { osd.fading = true; osd.opacity = 0 } }
    // What others do: the playback state event's SENDER (playback.by — the Rust mirror reads it; never me, never the
    // advancer's drift writes): only when the url changes, `seekNonce` changes or `playing` flips (in that order — the
    // web's osd.ts remoteChange; a stale-clock park is not a pause). Armed 2.5 s after the window opens / I join, so the
    // state I find on arrival is not announced.
    property var _pbPrev: ({})
    property double _osdArmAt: 0
    onPlaybackChanged: sheet.notePlayback()
    function notePlayback() {
        var pb = sheet.playback || {}, prev = sheet._pbPrev || {}
        sheet._pbPrev = pb
        if (!sheet.visible || !sheet.inParty || Date.now() < sheet._osdArmAt) return
        if (Number(pb.updatedAt || 0) === Number(prev.updatedAt || 0)) return
        var by = String(pb.by || "")
        if (by.length === 0 || by === sheet.myId) return
        var url = String(pb.url || ""), purl = String(prev.url || "")
        if (url !== purl) {
            if (url.length === 0 || url.indexOf("tinhat-memory://") === 0) return
            var it = null
            for (var i = 0; i < sheet.queue.length; ++i) if (String(sheet.queue[i].url || "") === url) { it = sheet.queue[i]; break }
            var t = (it && it.title) ? String(it.title) : url
            var adder = (it && it.addedBy) ? String(it.addedBy) : ""
            sheet.showOsd("Now playing: " + t + (adder.length > 0 ? " · added by " + (adder === sheet.myId ? "you" : sheet.nameOf(adder)) : ""),
                          (it && it.kind === "song") ? "emblem-music-symbolic" : "video-television-symbolic")
        } else if (Number(pb.seekNonce || 0) !== Number(prev.seekNonce || 0) && Number(pb.seekNonce || 0) !== 0) {
            sheet.showOsd(sheet.nameOf(by) + " jumped to " + sheet.fmtTime(Number(pb.positionSecs || 0)), "media-seek-forward")
        } else if (!!pb.playing !== !!prev.playing) {
            // the advancer parking a STALE clock (> 60 s without a write: PLAYBACK_LIVE_TTL_MS) is not a pause
            if (!pb.playing && Number(pb.updatedAt || 0) - Number(prev.updatedAt || 0) > 60000) return
            sheet.showOsd(sheet.nameOf(by) + (pb.playing ? " pressed play" : " paused"), pb.playing ? "media-playback-start" : "media-playback-pause")
        }
    }

    // 11.4: the one box's outcome (watchCtl.linkJson, mirrored): words → search; queued / playing → clear the box +
    // a toast; a card / browse / error → a note under the box.
    onLinkInfoChanged: {
        var li = sheet.linkInfo || {}
        var st = li.status ? String(li.status) : ""
        if (st === "search") {
            // selfhost (the web's add flow): no search on this server — words wait for a link
            if (!sheet.searchOn) { sheet.addNote = "Paste a link to a video or song — searching isn't available on this server."; return }
            if (sheet.browseTab !== "music") sheet.browseTab = "search"; sheet.runBrowse(); return
        }
        if (st === "queued" || st === "playing" || st === "card" || st === "error" || st === "browse") sheet.songCard = null
        if (st === "card") { sheet.songCard = { link: li.link || {}, meta: li.meta || null, message: li.message ? String(li.message) : "" }; sheet.addNote = ""; return }
        if (st === "queued" || st === "playing") {
            searchField.text = ""
            sheet.addNote = ""
            var t = (li.item && li.item.title) ? String(li.item.title) : ""
            if (li.item && li.item.url) sheet.markAdded(String(li.item.url))
            if (sheet.app) sheet.app.showPassiveNotification(LinkGate.plain(st === "playing" ? ("Playing " + (t.length > 0 ? t : "now")) : ("Added to the queue" + (t.length > 0 ? " — " + t : ""))), 2500)
            return
        }
        if (st === "browse") { if (sheet.searchOn) sheet.browseTab = "playlists"; return }
        if (st === "error") sheet.addNote = li.message ? String(li.message) : ""
    }
    function submitBox(playNow) {
        if (!sheet.boxIsLink || sheet.adding) return
        sheet.addNote = ""
        sheet.addLink(searchField.text, playNow)
    }

    // 11.3 QOL (§6): browse / History rows whose url is queued show "In the queue"; an Add flips it at once
    // (optimistic, until the queue mirror carries it — entries older than 8 s are dropped on the next queue change)
    property var _addedLocal: ({})
    function markAdded(url) { var m = Object.assign({}, sheet._addedLocal); m[String(url)] = Date.now(); sheet._addedLocal = m }
    onQueueChanged: {
        var m = {}, now = Date.now(), changed = false
        for (var k in sheet._addedLocal) { if (now - sheet._addedLocal[k] < 8000) m[k] = sheet._addedLocal[k]; else changed = true }
        if (changed) sheet._addedLocal = m
    }
    readonly property var queuedSet: {
        var m = {}
        for (var i = 0; i < sheet.queue.length; ++i) m[String(sheet.queue[i].url || "")] = true
        for (var k in sheet._addedLocal) m[k] = true
        return m
    }

    // ---- 11.3 QOL (§5) drag to reorder: press the grip (or a row + 6 px of travel) → a ghost follows the pointer and a
    // drop line marks the target; the drop is ONE moveItem(from, to) write. Auto-scrolls near the list's edges.
    property int dragFrom: -1
    property int dragIns: -1
    property real dragY: 0
    function beginDrag(i) {
        if (!sheet.inParty || i < 0 || i >= sheet.queue.length) return
        sheet.dragFrom = i; sheet.dragIns = i
    }
    function dragMove(viewY) {
        if (sheet.dragFrom < 0) return
        sheet.dragY = viewY
        var cy = viewY + queueView.contentY - queueView.originY
        var ins = Math.floor((cy + queueView.pitch / 2) / queueView.pitch)
        sheet.dragIns = Math.max(0, Math.min(sheet.queue.length, ins))
    }
    function endDrag() {
        var from = sheet.dragFrom, ins = sheet.dragIns
        console.log("[watchparty] drop: from " + from + " at " + ins)
        sheet.dragFrom = -1; sheet.dragIns = -1
        if (from < 0 || ins < 0) return
        var to = ins > from ? ins - 1 : ins
        if (to !== from && to >= 0 && to < sheet.queue.length) sheet.moveItem(from, to)
    }
    function cancelDrag() { sheet.dragFrom = -1; sheet.dragIns = -1 }
    Timer {
        interval: 30; repeat: true
        running: sheet.dragFrom >= 0 && (sheet.dragY < 28 || sheet.dragY > queueView.height - 28)
        onTriggered: {
            var maxY = Math.max(queueView.originY, queueView.originY + queueView.contentHeight - queueView.height)
            var step = sheet.dragY < 28 ? -12 : 12
            queueView.contentY = Math.max(queueView.originY, Math.min(maxY, queueView.contentY + step))
            sheet.dragMove(sheet.dragY)
        }
    }
    // the list follows the playing row (QOL §5)
    onNowUrlChanged: Qt.callLater(sheet.followNow)
    function followNow() { if (sheet.nowIndex >= 0 && queueView.count > sheet.nowIndex) queueView.positionViewAtIndex(sheet.nowIndex, ListView.Contain) }

    // the row menu (right-click a queue row): Play now · Play next · Move to top · Move up · Move down · Open link ·
    // Copy link · Remove
    function openRowMenu(i) { rowMenu.idx = i; rowMenu.popup() }
    Controls.Menu {
        id: rowMenu
        property int idx: -1
        readonly property var item: (idx >= 0 && idx < sheet.queue.length) ? sheet.queue[idx] : null
        readonly property string link: item ? String((item.source && item.source.url) ? item.source.url : (item.url || "")) : ""
        Controls.MenuItem { opacity: enabled ? 1 : 0.45; text: "Play now"; icon.name: "media-playback-start"; enabled: sheet.canControl; onTriggered: sheet.playNowAt(rowMenu.idx) }
        Controls.MenuItem { opacity: enabled ? 1 : 0.45; text: "Play next"; icon.name: "media-playlist-append"; enabled: sheet.inParty && rowMenu.idx !== sheet.nowIndex && rowMenu.idx !== sheet.nowIndex + 1; onTriggered: sheet.moveToNext(rowMenu.idx) }
        Controls.MenuItem { opacity: enabled ? 1 : 0.45; text: "Move to top"; icon.name: "go-top-symbolic"; enabled: sheet.inParty && rowMenu.idx > 0; onTriggered: sheet.moveItem(rowMenu.idx, 0) }
        Controls.MenuItem { opacity: enabled ? 1 : 0.45; text: "Move up"; icon.name: "go-up-symbolic"; enabled: sheet.inParty && rowMenu.idx > 0; onTriggered: sheet.moveItem(rowMenu.idx, rowMenu.idx - 1) }
        Controls.MenuItem { opacity: enabled ? 1 : 0.45; text: "Move down"; icon.name: "go-down-symbolic"; enabled: sheet.inParty && rowMenu.idx >= 0 && rowMenu.idx < sheet.queue.length - 1; onTriggered: sheet.moveItem(rowMenu.idx, rowMenu.idx + 1) }
        Controls.MenuSeparator {}
        Controls.MenuItem { opacity: enabled ? 1 : 0.45; text: "Open link"; icon.name: "link-symbolic"; enabled: rowMenu.link.length > 0; onTriggered: LinkGate.open(rowMenu.link) }
        Controls.MenuItem { opacity: enabled ? 1 : 0.45; text: "Copy link"; icon.name: "edit-copy-symbolic"; enabled: rowMenu.link.length > 0; onTriggered: { if (sheet.app) sheet.app.copyToClipboard(rowMenu.link); sheet.showOsd("Link copied", "edit-copy-symbolic") } }
        Controls.MenuSeparator {}
        Controls.MenuItem { opacity: enabled ? 1 : 0.45; text: "Remove"; icon.name: "edit-delete-remove-symbolic"; enabled: sheet.inParty; onTriggered: sheet.removeAt(rowMenu.idx) }
    }

    // ================================================================================================================
    // The visual tree. `body` re-declares the app scheme (Kirigami.Theme does NOT cross window boundaries); `outer`
    // scrolls the whole stacked column; in WIDE the stage and the side scroll on their own (stageFlick / sideFlick).
    // ================================================================================================================
    Item {
        id: body
        anchors.fill: parent
        focus: true
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
        // the chrome wakes on a REAL pointer move only: Qt Quick re-sends a synthetic hover after every repaint (the
        // clock repaints 8× a second), which kept the fullscreen transport from ever fading
        HoverHandler {
            id: winHover
            property point last: Qt.point(-1, -1)
            onPointChanged: {
                var p = point.position
                if (Math.abs(p.x - last.x) + Math.abs(p.y - last.y) < 2) return
                last = Qt.point(p.x, p.y)
                sheet.poke()
            }
        }

        Flickable {
            id: outer
            anchors.fill: parent
            clip: true
            interactive: sheet.stacked
            boundsBehavior: Flickable.StopAtBounds
            contentWidth: width
            contentHeight: sheet.stacked ? stagePane.height + sidePane.height : height
            Controls.ScrollBar.vertical: Controls.ScrollBar { policy: sheet.stacked ? Controls.ScrollBar.AlwaysOn : Controls.ScrollBar.AlwaysOff }
            // a resize across 960 / a mode change: the stacked scroll position means nothing in the new layout
            onInteractiveChanged: contentY = 0

            // ---------------------------------------------------------------------------------------------------------
            // THE STAGE: header · error · now playing | video | transport | watching · join
            // ---------------------------------------------------------------------------------------------------------
            Item {
                id: stagePane
                x: 0; y: 0
                width: sheet.wide ? outer.width - sheet.sideWEff : outer.width
                height: sheet.stacked ? stageCol.implicitHeight + sheet.pad * 2 : outer.height
                Flickable {
                    id: stageFlick
                    anchors.fill: parent
                    clip: sheet.wide
                    interactive: sheet.wide && contentHeight > height + 1
                    boundsBehavior: Flickable.StopAtBounds
                    contentWidth: width
                    contentHeight: sheet.wide ? stageCol.implicitHeight + sheet.pad * 2 : height
                    Controls.ScrollBar.vertical: Controls.ScrollBar { policy: stageFlick.interactive ? Controls.ScrollBar.AsNeeded : Controls.ScrollBar.AlwaysOff }

                    ColumnLayout {
                        id: stageCol
                        x: sheet.pad
                        y: sheet.pad
                        width: stagePane.width - sheet.pad * 2 - (sheet.stacked ? sheet.gutter : 0)
                        spacing: (sheet.videoOnly || sheet.stageOnly) ? 0 : Kirigami.Units.largeSpacing
                        // 16:9 as wide as the stage, its height capped so the stage fits the window (QOL §1)
                        readonly property real fixedH: stageTop.implicitHeight + transportBar.implicitHeight + stageBottom.implicitHeight
                                                       + spacing * 3 + sheet.pad * 2
                        readonly property real videoH: sheet.videoOnly ? outer.height
                            : (sheet.stageOnly ? Math.max(120, outer.height - transportBar.implicitHeight - Kirigami.Units.smallSpacing * 2)
                            : (sheet.wide ? Math.max(200, Math.min(stageCol.width * 9 / 16, outer.height - stageCol.fixedH))
                            : Math.min(stageCol.width * 9 / 16, Math.max(200, sheet.height - 160))))
                        readonly property real videoW: (sheet.videoOnly || sheet.stageOnly) ? stageCol.width : Math.min(stageCol.width, stageCol.videoH * 16 / 9)

                        ColumnLayout {
                            id: stageTop
                            Layout.fillWidth: true
                            visible: !sheet.videoOnly && !sheet.stageOnly
                            spacing: Kirigami.Units.largeSpacing
                            // (was the OverlaySheet `header:`) title + live watcher count, now the first content row.
                            RowLayout {
                                Layout.fillWidth: true
                                Kirigami.Heading {
                                    text: "Media Party"
                                    color: Th.watchpartyFg
                                }
                                Controls.Label {
                                    Layout.fillWidth: true
                                    horizontalAlignment: Text.AlignRight
                                    visible: sheet.inParty
                                    textFormat: Text.PlainText
                                    text: sheet.viewers.length + " watching"
                                          + (sheet.amAdvancer ? " · you sync" : "")
                                          + (sheet.hostId.length > 0 ? " · host: " + (sheet.amHost ? "you" : sheet.hostName) : "")   // 10.1 (TH-WP-05)
                                    color: Th.watchpartyMetaFg
                                    font.pointSize: Kirigami.Theme.smallFont.pointSize
                                    elide: Text.ElideRight
                                }
                                ThToolButton {   // 10.1 (TH-WP-05): take / release the remote
                                    app: sheet.app
                                    visible: sheet.inParty
                                    label: stageCol.width < 560 ? "" : (sheet.amHost ? "Release host" : (sheet.hostId.length > 0 ? "Take over as host" : "Become host"))
                                    iconName: "user-identity"
                                    tip: sheet.amHost ? "Let anyone drive playback again"
                                                      : "Only the host can play, pause, seek and skip; everyone can still queue"
                                    onClicked: sheet.runAction("watchParty.host.toggle")
                                }
                            }

                            Controls.Label {
                                Layout.fillWidth: true
                                visible: sheet.errorText.length > 0
                                textFormat: Text.PlainText
                                text: sheet.errorText
                                color: Th.watchpartyErrorFg
                                wrapMode: Text.WordWrap
                            }

                            // ---- 11.4 now-playing header: art · title · artist / duration · added by · source badge ----
                            RowLayout {
                                Layout.fillWidth: true
                                spacing: Kirigami.Units.largeSpacing
                                visible: sheet.nowItem !== null && !sheet.memoryClock
                                Rectangle {
                                    implicitWidth: 48; implicitHeight: 48; radius: 6; clip: true
                                    color: Th.watchpartyQueueRowBg
                                    border.width: 1; border.color: Th.watchpartyQueueRowBorder
                                    Image {
                                        anchors.fill: parent
                                        source: LinkGate.imageSrc(sheet.nowItem ? sheet.nowItem.art : "")   // review #5: https only
                                        visible: status === Image.Ready
                                        fillMode: Image.PreserveAspectCrop
                                        asynchronous: true
                                        sourceSize.width: 96; sourceSize.height: 96
                                    }
                                    Kirigami.Icon {
                                        anchors.centerIn: parent
                                        visible: LinkGate.imageSrc(sheet.nowItem ? sheet.nowItem.art : "") === ""
                                        source: "video-television-symbolic"
                                        color: Th.watchpartyEmptyIcon
                                        implicitWidth: 22; implicitHeight: 22
                                    }
                                }
                                ColumnLayout {
                                    Layout.fillWidth: true
                                    Layout.minimumWidth: 100
                                    spacing: 0
                                    Controls.Label {
                                        Layout.fillWidth: true
                                        textFormat: Text.PlainText
                                        text: sheet.nowItem ? ((sheet.nowItem.title && sheet.nowItem.title.length > 0) ? String(sheet.nowItem.title) : String(sheet.nowItem.url || "")) : ""
                                        color: Th.watchpartyFg
                                        font.bold: true
                                        elide: Text.ElideRight
                                    }
                                    Controls.Label {
                                        Layout.fillWidth: true
                                        readonly property string dur: (sheet.nowItem && sheet.nowItem.durationSec) ? sheet.fmtTime(Number(sheet.nowItem.durationSec)) : (sheet.mpvDuration > 0 ? sheet.fmtTime(sheet.mpvDuration) : "")
                                        readonly property string artist: (sheet.nowItem && sheet.nowItem.artist) ? String(sheet.nowItem.artist) : ""
                                        visible: text.length > 0
                                        textFormat: Text.PlainText
                                        text: artist.length > 0 && dur.length > 0 ? artist + " · " + dur : (artist.length > 0 ? artist : dur)
                                        color: Th.watchpartyMetaFg
                                        font.pointSize: Kirigami.Theme.smallFont.pointSize
                                        elide: Text.ElideRight
                                    }
                                }
                                RowLayout {
                                    visible: !!(sheet.nowItem && sheet.nowItem.addedBy && String(sheet.nowItem.addedBy).length > 0)
                                    spacing: 4
                                    Avatar { app: sheet.app; size: 20; name: sheet.nowItem ? sheet.nameOf(String(sheet.nowItem.addedBy || "")) : ""; seed: sheet.nowItem ? String(sheet.nowItem.addedBy || "") : ""; imageSource: sheet.nowItem ? sheet.avatarOf(String(sheet.nowItem.addedBy || "")) : "" }
                                    Controls.Label { visible: stageCol.width >= 560; textFormat: Text.PlainText; text: sheet.nowItem ? sheet.nameOf(String(sheet.nowItem.addedBy || "")) : ""; color: Th.watchpartyMetaFg; font.pointSize: Kirigami.Theme.smallFont.pointSize; elide: Text.ElideRight; Layout.maximumWidth: 120 }
                                }
                                Rectangle {   // source badge
                                    visible: sheet.badgeOf(sheet.nowItem).length > 0 && stageCol.width >= 640
                                    implicitWidth: npBadge.implicitWidth + 12; implicitHeight: npBadge.implicitHeight + 2; radius: 8
                                    color: Th.watchpartyQueueRowBg; border.width: 1; border.color: Th.watchpartyQueueRowBorder
                                    Controls.Label { id: npBadge; anchors.centerIn: parent; textFormat: Text.PlainText; text: sheet.badgeOf(sheet.nowItem); color: Th.watchpartyMetaFg; font.pointSize: Kirigami.Theme.smallFont.pointSize }
                                }
                                ThButton {
                                    app: sheet.app
                                    visible: !!(sheet.nowItem && sheet.nowItem.source && sheet.nowItem.source.service && sheet.nowItem.source.service !== "youtube") && stageCol.width >= 480
                                    text: sheet.openInLabel(sheet.nowItem && sheet.nowItem.source ? String(sheet.nowItem.source.service || "") : "")
                                    onClicked: LinkGate.open(String(sheet.nowItem.source.url || sheet.nowItem.url))
                                }
                                ThToolButton { app: sheet.app; iconName: "document-share-symbolic"; iconSize: 16; tip: "Share to chat"; onClicked: sheet.shareToChat() }   // 11.4
                            }
                        }

                        // ---- libmpv/yt-dlp video surface (mpvqt MpvAbstractItem registered as MpvVideo) ----
                        // the frame spans the stage; the 16:9 surface sits centred in it (letterbox = watchparty.video.bg)
                        Rectangle {
                            id: videoFrame
                            Layout.fillWidth: true
                            Layout.preferredHeight: stageCol.videoH
                            color: (sheet.videoOnly || sheet.stageOnly) ? Th.watchpartyVideoBg : "transparent"
                            Rectangle {
                                id: videoSurface
                                anchors.centerIn: parent
                                width: stageCol.videoW
                                height: parent.height
                                radius: (sheet.videoOnly || sheet.stageOnly) ? 0 : sheet.theme.radius
                                color: Th.watchpartyVideoBg
                                border.width: (sheet.videoOnly || sheet.stageOnly) ? 0 : 1
                                border.color: Th.watchpartyVideoBorder
                                clip: true
                                // 0.8.30: rounded corners over the mpv surface (see CornerMask.qml).
                                CornerMask { visible: !sheet.videoOnly && !sheet.stageOnly; radius: sheet.theme.radius; color: Th.watchpartyBg; z: 15 }
                                Rectangle { visible: !sheet.videoOnly && !sheet.stageOnly; anchors.fill: parent; z: 16; radius: sheet.theme.radius; color: "transparent"; border.width: 1; border.color: Th.watchpartyVideoBorder }
                                // 11.3 QOL (§2): click = play / pause for everyone, deferred 250 ms so a double-click =
                                // fullscreen without toggling play; a click also takes the keys back from the box
                                Timer { id: clickT; interval: 250; onTriggered: sheet.togglePlay() }
                                TapHandler {
                                    acceptedButtons: Qt.LeftButton
                                    onTapped: (eventPoint, button) => {
                                        body.forceActiveFocus()
                                        if (tapCount >= 2) { clickT.stop(); sheet.setFullscreen(!sheet.fullscreen) }
                                        else if (sheet.nowUrl.length > 0) clickT.restart()
                                    }
                                }
                                // the wheel = my volume ±5 per notch (OSD "Volume 60 %")
                                MouseArea {
                                    anchors.fill: parent
                                    acceptedButtons: Qt.NoButton
                                    z: 1
                                    property real acc: 0
                                    onWheel: (wheel) => {
                                        wheel.accepted = true
                                        acc += wheel.angleDelta.y
                                        while (Math.abs(acc) >= 120) { sheet.stepVolume(acc > 0 ? 5 : -5); acc += acc > 0 ? -120 : 120 }
                                    }
                                }

                                MpvVideo {
                                    id: mpv
                                    anchors.fill: parent
                                    onReady: { mpv.tuneMpv(); mpv.loadCurrent() }

                                    // Single source of truth for loadfile: only (re)load when the now-playing url actually differs.
                                    function loadCurrent() {
                                        if (sheet.nowUrl === sheet.mLoadedUrl)
                                            return
                                        sheet.mLoadedUrl = sheet.nowUrl
                                        sheet.mEofSeen = false
                                        sheet.playbackError = ""
                                        sheet.buffering = false
                                        sheet.mSawPlayback = false
                                        sheet.mLoadStartedAt = Date.now()
                                        sheet.mpvCacheEnd = 0
                                        sheet.mpvPos = 0          // 11.3 QOL: no stale "0:32 / 3:44" once the item changes
                                        sheet.mpvDuration = 0
                                        if (sheet.nowUrl.indexOf("tinhat-memory://") === 0)
                                            mpv.command(["stop"])   // Memories replays play in the Memories window
                                        else if (sheet.nowUrl.length > 0) {
                                            // M8 (security review 2026-10-26): the shared url is room state anyone with the
                                            // power can write — mpv gets it only through watchCtl.mpvTarget (http(s) only;
                                            // yt-dlp only for a YouTube video, everything else via FFmpeg's own reader)
                                            var t = null
                                            try { t = JSON.parse(String(sheet.app.watchCtl.mpvTarget(sheet.nowUrl))) } catch (e) { t = null }
                                            if (!t || t.ok !== true || !t.load) {
                                                mpv.command(["stop"])
                                                sheet.playbackError = (t && t.why) ? String(t.why) : "This link can't play in the Media Party."
                                                return
                                            }
                                            // 11.3 QOL: load paused when the shared clock is paused (mpv starts a file
                                            // playing, so a paused party used to run on locally until the next flip)
                                            mpv.setProperty("pause", !sheet.isPlaying)
                                            // "lavf" for anything but a YouTube video: FFmpeg reads it, so no playlist or
                                            // EDL inside it becomes another file mpv opens; "" (auto) for YouTube, whose
                                            // yt-dlp answer is an EDL. Set right before the load (the calls are in order).
                                            mpv.setProperty("demuxer", String(t.demuxer || ""))
                                            mpv.command(["loadfile", String(t.load)])
                                        }
                                        else
                                            mpv.command(["stop"])
                                    }

                                    // Performance + buffering, applied once up front (and re-applied on onReady). Without hwdec mpv
                                    // software-decodes; the cache knobs give a real readahead buffer so streamed YouTube doesn't stall.
                                    function tuneMpv() {
                                        mpv.setProperty("hwdec", "auto-safe")
                                        mpv.setProperty("cache", "yes")
                                        mpv.setProperty("cache-secs", 30)
                                        mpv.setProperty("demuxer-max-bytes", "150MiB")
                                        mpv.setProperty("demuxer-max-back-bytes", "75MiB")
                                        mpv.setProperty("demuxer-readahead-secs", 30)
                                        // ytdl_hook resolves YouTube via yt-dlp from PATH — never pin an absolute path here.
                                        // M8: the hook is held to the canonical YouTube watch address on every call (its
                                        // exclude list = everything else; mpv delivers this change to the hook before any
                                        // load's hook runs) and yt-dlp to its YouTube extractors (watchparty.rs mpv_options).
                                        var o = null
                                        try { o = JSON.parse(String(sheet.app.watchCtl.mpvOptions())) } catch (e) { o = null }
                                        if (o) {
                                            mpv.setProperty("script-opts", String(o.scriptOpts))
                                            mpv.setProperty("ytdl-raw-options", String(o.ytdlRawOptions))
                                        }
                                        mpv.setProperty("ytdl", !!o)   // no rules, no yt-dlp (YouTube then fails to load)
                                        // M8: a playlist inside a web file may only lead to other web files, and FFmpeg's
                                        // nested requests (HLS segments and keys) stay on the web.
                                        mpv.setProperty("load-unsafe-playlists", false)
                                        mpv.setProperty("demuxer-lavf-o", "protocol_whitelist=[http,https,tls,tcp,crypto,data,httpproxy]")
                                    }
                                    Component.onCompleted: {
                                        mpv.tuneMpv()
                                        mpv.loadCurrent()
                                    }

                                    Connections {
                                        target: sheet
                                        function onNowUrlChanged() { mpv.loadCurrent() }
                                        function onIsPlayingChanged() { mpv.setProperty("pause", !sheet.isPlaying) }
                                    }

                                    // Poll the player's own clock / duration / eof / cache state and feed them back across the
                                    // (string-only) boundary — mpvqt exposes property CHANGES only via the protected MpvController.
                                    Timer {
                                        interval: Motion.focused ? 120 : 1000   // 11.0 (GPU.md §2.8): ≥ 1 s while not focused
                                        repeat: true
                                        running: sheet.visible && sheet.nowUrl.length > 0
                                        onTriggered: {
                                            var t = mpv.getProperty("time-pos")
                                            if (t !== undefined && t !== null && !isNaN(Number(t))) {
                                                sheet.mpvPos = Number(t)
                                                if (Number(t) > 0) { sheet.mSawPlayback = true; if (sheet.playbackError.length > 0) sheet.playbackError = "" }   // errors clear on the next success
                                                sheet.reportPosition(Number(t).toFixed(3))
                                            }
                                            var d = mpv.getProperty("duration")
                                            if (d !== undefined && d !== null && !isNaN(Number(d)))
                                                sheet.mpvDuration = Number(d)
                                            // 11.3 QOL: how far the demuxer has data → the scrub bar's buffered band
                                            var ce = mpv.getProperty("demuxer-cache-time")
                                            sheet.mpvCacheEnd = (ce !== undefined && ce !== null && !isNaN(Number(ce))) ? Number(ce) : 0
                                            // 11.4: mpv waits for data → the buffering spinner
                                            var pc = mpv.getProperty("paused-for-cache")
                                            sheet.buffering = pc === true
                                            // EOF → advance ONCE (edge-latched; `eof-reached` is level-triggered)
                                            var eof = mpv.getProperty("eof-reached")
                                            if (eof === true) {
                                                if (!sheet.mEofSeen) {
                                                    sheet.mEofSeen = true
                                                    sheet.mediaEnded()
                                                }
                                            } else if (eof === false) {
                                                sheet.mEofSeen = false
                                            }
                                            // Local load-failure watchdog: idle with a url chosen and no frame ever = the resolve /
                                            // decode failed (20 s grace covers a slow ytdl resolve).
                                            if (sheet.playbackError.length === 0
                                                    && !sheet.mSawPlayback && !sheet.mEofSeen
                                                    && Date.now() - sheet.mLoadStartedAt > 20000
                                                    && mpv.getProperty("idle-active") === true) {
                                                sheet.playbackError =
                                                    "Playback failed — mpv couldn't load this link. YouTube needs yt-dlp " +
                                                    "(pacman -S yt-dlp); a private, removed or region-locked video fails the same way. Press Retry."
                                            }
                                        }
                                    }
                                }

                                // Empty state while no item is loaded — the house EmptyState above the black surface.
                                EmptyState {
                                    anchors.fill: parent
                                    z: 2
                                    visible: sheet.nowUrl.length === 0
                                    app: sheet.app
                                    iconName: "video-television-symbolic"
                                    title: "Nothing playing"
                                    explanation: sheet.mini ? "" : "Paste a link or search, then pick what to play."
                                }
                                // A synced Memories replay plays in the Memories window.
                                ColumnLayout {
                                    anchors.centerIn: parent
                                    z: 2
                                    width: parent.width - Kirigami.Units.largeSpacing * 2
                                    spacing: Kirigami.Units.smallSpacing
                                    visible: sheet.memoryClock
                                    Kirigami.Icon { Layout.alignment: Qt.AlignHCenter; source: "camera-video-symbolic"; color: Th.watchpartyEmptyIcon; implicitWidth: 44; implicitHeight: 44 }
                                    Controls.Label { Layout.fillWidth: true; horizontalAlignment: Text.AlignHCenter; text: "A synced Memories replay is playing — it plays in the Memories window."; color: Th.watchpartyEmptyFg; wrapMode: Text.WordWrap }
                                }
                                // 11.4: buffering
                                ColumnLayout {
                                    anchors.centerIn: parent
                                    z: 2
                                    spacing: Kirigami.Units.smallSpacing
                                    visible: sheet.buffering && sheet.nowUrl.length > 0 && sheet.playbackError.length === 0
                                    ThBusyIndicator { Layout.alignment: Qt.AlignHCenter; running: sheet.buffering; size: 36; color: Th.watchpartyBusyFg }
                                    Controls.Label { Layout.alignment: Qt.AlignHCenter; text: "Buffering…"; color: Th.watchpartyEmptyFg }
                                }
                                // LOCAL playback-failure overlay (the party keeps running for everyone else).
                                ColumnLayout {
                                    anchors.centerIn: parent
                                    z: 3
                                    width: parent.width - Kirigami.Units.largeSpacing * 2
                                    spacing: Kirigami.Units.smallSpacing
                                    visible: sheet.playbackError.length > 0 && sheet.nowUrl.length > 0
                                    Kirigami.Icon { Layout.alignment: Qt.AlignHCenter; source: "dialog-warning-symbolic"; color: Th.watchpartyFailureIcon; implicitWidth: 40; implicitHeight: 40 }
                                    Controls.Label { Layout.fillWidth: true; horizontalAlignment: Text.AlignHCenter; textFormat: Text.PlainText; text: sheet.playbackError; color: Th.watchpartyFailureFg; wrapMode: Text.WordWrap }
                                    GlowButton {
                                        Layout.alignment: Qt.AlignHCenter
                                        app: sheet.app
                                        text: "Retry"
                                        onClicked: { sheet.mLoadedUrl = ""; mpv.loadCurrent() }
                                    }
                                }
                                // 11.3 QOL (§2): the OSD pill
                                Rectangle {
                                    id: osd
                                    property string text: ""
                                    property string icon: ""
                                    property bool fading: false
                                    anchors.horizontalCenter: parent.horizontalCenter
                                    anchors.top: parent.top
                                    anchors.topMargin: sheet.mini ? 8 : 14
                                    z: 25
                                    opacity: 0
                                    visible: opacity > 0
                                    implicitWidth: osdRow.implicitWidth + 24
                                    implicitHeight: osdRow.implicitHeight + 10
                                    radius: height / 2
                                    color: Th.watchpartyClickToPlayBg
                                    Behavior on opacity { enabled: osd.fading; NumberAnimation { duration: Motion.durLong } }
                                    RowLayout {
                                        id: osdRow
                                        anchors.centerIn: parent
                                        spacing: 6
                                        Kirigami.Icon { visible: osd.icon.length > 0; source: osd.icon; color: Th.watchpartyClickToPlayFg; implicitWidth: 16; implicitHeight: 16 }
                                        Controls.Label {
                                            textFormat: Text.PlainText
                                            text: osd.text
                                            color: Th.watchpartyClickToPlayFg
                                            font.bold: true
                                            elide: Text.ElideRight
                                            Layout.maximumWidth: Math.max(80, videoSurface.width - 72)
                                        }
                                    }
                                }
                            }
                        }

                        // ---- Transport (QOL §3): play/pause · mute · volume · scrub · time · next · Theater · Mini player ·
                        // fullscreen · Keys. Fullscreen / mini: an overlay on the bottom of the video (fades / on hover).
                        Rectangle {
                            id: transportBar
                            Layout.fillWidth: true
                            Layout.leftMargin: sheet.videoOnly ? 16 : (sheet.stageOnly ? Kirigami.Units.smallSpacing : 0)
                            Layout.rightMargin: Layout.leftMargin
                            Layout.topMargin: sheet.videoOnly ? -transportRow.implicitHeight - 12 - (sheet.mini ? 8 : 16)
                                                              : (sheet.stageOnly ? Kirigami.Units.smallSpacing : 0)
                            Layout.bottomMargin: sheet.stageOnly ? Kirigami.Units.smallSpacing : 0
                            implicitHeight: transportRow.implicitHeight + (sheet.videoOnly ? 12 : 0)
                            radius: sheet.theme.radius
                            color: sheet.videoOnly ? Th.watchpartyClickToPlayBg : "transparent"
                            opacity: sheet.chromeShown ? 1 : 0
                            visible: opacity > 0
                            Behavior on opacity { NumberAnimation { duration: Motion.dur } }
                            z: 20
                            HoverHandler { id: transportHover; onHoveredChanged: sheet.poke() }
                            // eat clicks between the buttons (the video's click = play / pause sits underneath)
                            MouseArea { anchors.fill: parent; acceptedButtons: Qt.LeftButton; onClicked: {} }
                            RowLayout {
                                id: transportRow
                                anchors.fill: parent
                                anchors.margins: sheet.videoOnly ? 6 : 0
                                spacing: Kirigami.Units.smallSpacing
                                ThToolButton {
                                    Layout.preferredWidth: sheet.tightBar ? 32 : -1   // 11.3 QOL: narrow windows
                                    app: sheet.app
                                    iconName: sheet.playingNow ? "media-playback-pause" : "media-playback-start"
                                    enabled: sheet.canControl && sheet.nowUrl.length > 0   // 10.1 (TH-WP-05)
                                    tip: !sheet.canControl && sheet.inParty ? "Only the host (" + sheet.hostName + ") controls playback"
                                       : (sheet.playingNow ? "Pause for everyone (Space)" : "Play for everyone (Space)")
                                    onClicked: sheet.togglePlay()
                                }
                                ThToolButton {   // 10.1 (TH-WP-03): volume + mute, only for you.
                                    Layout.preferredWidth: sheet.tightBar ? 32 : -1   // 11.3 QOL: narrow windows
                                    app: sheet.app
                                    iconName: (sheet.muted || sheet.volume === 0) ? "audio-volume-muted-symbolic" : "audio-volume-high-symbolic"
                                    tip: sheet.muted ? "Unmute the video (only for you) (M)" : "Mute the video (only for you) (M)"
                                    onClicked: sheet.runAction("watchParty.mute")
                                }
                                ThSlider {   // 11.3 QOL: hidden under 520 px (the wheel / ↑ ↓ still work)
                                    Layout.preferredWidth: 90
                                    visible: !sheet.mini && sheet.width >= 520
                                    from: 0; to: 100
                                    value: sheet.volume
                                    onMoved: { sheet.volume = value; if (sheet.muted && value > 0) sheet.muted = false }
                                }
                                // Scrub slider — releasing commits a manual seek (bumps seekNonce → everyone jumps). A click on
                                // the track jumps there (host-gated like a drag); hovering shows the time under the pointer.
                                ThSlider {
                                    id: scrub
                                    Layout.fillWidth: true
                                    Layout.minimumWidth: 48
                                    from: 0
                                    to: sheet.mpvDuration > 0 ? sheet.mpvDuration : Math.max(600, sheet.mpvPos + 60)
                                    enabled: sheet.canControl && sheet.nowUrl.length > 0   // 10.1 (TH-WP-05)
                                    value: pressed ? value : sheet.mpvPos
                                    onPressedChanged: if (!pressed) sheet.seekToSecs(value, "")
                                    readonly property real hoverX: scrubHover.point.position.x
                                    readonly property real hoverSecs: scrub.from + Math.max(0, Math.min(1, (hoverX - scrub.leftPadding) / Math.max(1, scrub.availableWidth))) * (scrub.to - scrub.from)
                                    HoverHandler { id: scrubHover }
                                    // the buffered band (mpv demuxer-cache-time) — beyond the played part only, under the handle
                                    Rectangle {
                                        readonly property real x0: scrub.leftPadding + scrub.visualPosition * scrub.availableWidth
                                        readonly property real x1: scrub.leftPadding + Math.max(0, Math.min(1, (sheet.mpvCacheEnd - scrub.from) / Math.max(0.001, scrub.to - scrub.from))) * scrub.availableWidth
                                        visible: sheet.mpvDuration > 0 && x1 > x0 + 1
                                        x: x0; width: Math.max(0, x1 - x0)
                                        height: 4; radius: 2
                                        anchors.verticalCenter: parent.verticalCenter
                                        color: Th.watchpartyMetaFg
                                        opacity: 0.45
                                        z: 0.5
                                    }
                                    Controls.ToolTip {
                                        visible: scrubHover.hovered && sheet.mpvDuration > 0
                                        text: sheet.fmtTime(scrub.hoverSecs)
                                        x: Math.max(0, Math.min(scrub.width - width, scrub.hoverX - width / 2))
                                        y: -height - 4
                                        delay: 0
                                    }
                                }
                                Controls.Label {   // 11.3 QOL: click = elapsed ⇄ remaining ("−3:20 / 4:05"), remembered
                                    textFormat: Text.PlainText
                                    text: (sheet.timeRemaining && sheet.mpvDuration > 0 ? "−" + sheet.fmtTime(Math.max(0, sheet.mpvDuration - sheet.mpvPos)) : sheet.fmtTime(sheet.mpvPos))
                                          + (sheet.mpvDuration > 0 ? " / " + sheet.fmtTime(sheet.mpvDuration) : "")
                                    color: sheet.videoOnly ? Th.watchpartyClickToPlayFg : Th.watchpartyTimeFg
                                    font.pointSize: Kirigami.Theme.smallFont.pointSize
                                    font.features: { "tnum": 1 }
                                    MouseArea {
                                        anchors.fill: parent
                                        cursorShape: Qt.PointingHandCursor
                                        onClicked: { sheet.timeRemaining = !sheet.timeRemaining; sheet.savePref("watchparty-time-remaining", sheet.timeRemaining ? "1" : "0") }
                                    }
                                }
                                ThToolButton {
                                    Layout.preferredWidth: sheet.tightBar ? 32 : -1   // 11.3 QOL: narrow windows
                                    app: sheet.app
                                    iconName: "media-skip-forward"
                                    enabled: sheet.canControl && sheet.nextQueueUrl().length > 0   // 10.1 (TH-WP-05)
                                    tip: "Play next in queue (N)"
                                    onClicked: sheet.runAction("watchParty.next")
                                }
                                ThToolButton {   // 11.3 QOL: theater — the video fills the window (a layout, remembered)
                                    Layout.preferredWidth: sheet.tightBar ? 32 : -1   // 11.3 QOL: narrow windows
                                    app: sheet.app
                                    visible: !sheet.videoOnly && stageCol.width >= 400
                                    iconName: "window-symbolic"
                                    active: sheet.theater
                                    tip: sheet.theater ? "Leave theater (T)" : "Theater (T)"
                                    onClicked: sheet.setTheater(!sheet.theater)
                                }
                                ThToolButton {   // 11.3 QOL: the mini player
                                    Layout.preferredWidth: sheet.tightBar ? 32 : -1   // 11.3 QOL: narrow windows
                                    app: sheet.app
                                    visible: !sheet.mini
                                    iconName: "window-duplicate-symbolic"
                                    tip: "Mini player (I)"
                                    onClicked: sheet.setMini(true)
                                }
                                ThToolButton {   // 11.4: fullscreen
                                    Layout.preferredWidth: sheet.tightBar ? 32 : -1   // 11.3 QOL: narrow windows
                                    app: sheet.app
                                    visible: !sheet.mini
                                    iconName: sheet.fullscreen ? "view-restore-symbolic" : "view-fullscreen-symbolic"
                                    tip: sheet.fullscreen ? "Leave fullscreen (F)" : "Fullscreen (F)"
                                    onClicked: sheet.setFullscreen(!sheet.fullscreen)
                                }
                                ThToolButton {   // 11.3 QOL: the keys card
                                    Layout.preferredWidth: sheet.tightBar ? 32 : -1   // 11.3 QOL: narrow windows
                                    app: sheet.app
                                    visible: !sheet.mini && (sheet.videoOnly || stageCol.width >= 400)
                                    iconName: "input-keyboard-symbolic"
                                    active: keysCard.visible
                                    tip: "Keys (?)"
                                    onClicked: keysCard.visible = !keysCard.visible
                                }
                                ThToolButton {   // mini: back to the full window
                                    Layout.preferredWidth: sheet.tightBar ? 32 : -1   // 11.3 QOL: narrow windows
                                    app: sheet.app
                                    visible: sheet.mini
                                    iconName: "window-duplicate-symbolic"
                                    active: true
                                    tip: "Back to the full window (I)"
                                    onClicked: sheet.setMini(false)
                                }
                            }
                        }

                        ColumnLayout {
                            id: stageBottom
                            Layout.fillWidth: true
                            visible: !sheet.videoOnly && !sheet.stageOnly
                            spacing: Kirigami.Units.largeSpacing
                            // ---- 11.4 who's watching: avatars + names, the host badge ----
                            Flow {
                                Layout.fillWidth: true
                                visible: sheet.inParty && sheet.viewers.length > 0
                                spacing: Kirigami.Units.largeSpacing
                                Controls.Label { text: "Watching"; color: Th.watchpartyMetaFg; font.pointSize: Kirigami.Theme.smallFont.pointSize; height: 20; verticalAlignment: Text.AlignVCenter }
                                Repeater {
                                    model: sheet.viewers
                                    delegate: RowLayout {
                                        required property var modelData
                                        spacing: 4
                                        readonly property string uid: String(modelData.userId || "")
                                        Avatar { app: sheet.app; size: 20; name: sheet.nameOf(parent.uid); seed: parent.uid; imageSource: sheet.avatarOf(parent.uid) }
                                        Controls.Label { textFormat: Text.PlainText; text: sheet.nameOf(parent.uid); color: Th.watchpartyMetaFg; font.pointSize: Kirigami.Theme.smallFont.pointSize; elide: Text.ElideRight; Layout.maximumWidth: 120 }
                                        Rectangle {
                                            visible: sheet.hostId.length > 0 && parent.uid === sheet.hostId
                                            implicitWidth: hostLbl.implicitWidth + 10; implicitHeight: 14; radius: 7
                                            color: Th.watchpartyQueueRowBgHover; border.width: 1; border.color: Th.watchpartyQueueRowBorderHover
                                            Controls.Label { id: hostLbl; anchors.centerIn: parent; text: "host"; font.bold: true; font.pixelSize: 10; color: Th.watchpartyFg }
                                        }
                                    }
                                }
                            }
                            // ---- Join / leave ----
                            RowLayout {
                                Layout.fillWidth: true
                                GlowButton {
                                    app: sheet.app
                                    text: sheet.inParty ? "Leave Media Party" : "Join Media Party"
                                    icon.name: sheet.inParty ? "system-log-out-symbolic" : "media-playlist-append"
                                    onClicked: sheet.inParty ? sheet.leaveParty() : sheet.joinParty()
                                }
                                Item { Layout.fillWidth: true }
                                Controls.Label {
                                    visible: !sheet.inParty
                                    text: "Join to sync playback with the room."
                                    color: Th.watchpartyMetaFg
                                    font.pointSize: Kirigami.Theme.smallFont.pointSize
                                }
                            }
                        }
                    }
                }
            }

            // ---- the divider (WIDE): 1 px line, 6 px grab, drag = the side's width, double-click resets ----
            Item {
                id: divider
                visible: sheet.wide
                x: stagePane.width - 3
                y: 0
                width: 7
                height: outer.height
                z: 5
                Rectangle { x: 3; width: 1; height: parent.height; color: Th.watchpartySectionDivider }
                MouseArea {
                    anchors.fill: parent
                    cursorShape: Qt.SplitHCursor
                    preventStealing: true
                    property real startX: 0
                    property real startW: 0
                    onPressed: (m) => { startX = mapToItem(body, m.x, 0).x; startW = sheet.sideWEff }
                    onPositionChanged: (m) => {
                        if (!pressed) return
                        var gx = mapToItem(body, m.x, 0).x
                        sheet.sideW = Math.round(Math.max(300, Math.min(560, sheet.width * 0.5, startW - (gx - startX))))
                    }
                    onReleased: sheet.savePref("watchparty-side-w", Math.round(sheet.sideWEff))
                    onDoubleClicked: { sheet.sideW = 380; sheet.savePref("watchparty-side-w", 380) }
                }
            }

            // ---------------------------------------------------------------------------------------------------------
            // THE SIDE (WIDE: right of the divider, its own scroll; STACKED: below the stage): queue · the box · browse
            // ---------------------------------------------------------------------------------------------------------
            Item {
                id: sidePane
                visible: sheet.wide || sheet.stacked
                x: sheet.wide ? stagePane.width + 1 : 0
                y: sheet.wide ? 0 : stagePane.height
                width: sheet.wide ? sheet.sideWEff - 1 : outer.width
                height: sheet.wide ? outer.height : sideCol.implicitHeight + sheet.pad * 2
                Flickable {
                    id: sideFlick
                    anchors.fill: parent
                    clip: sheet.wide
                    interactive: sheet.wide
                    boundsBehavior: Flickable.StopAtBounds
                    contentWidth: width
                    contentHeight: sideCol.implicitHeight + sheet.pad * 2
                    Controls.ScrollBar.vertical: Controls.ScrollBar { policy: sheet.wide ? Controls.ScrollBar.AlwaysOn : Controls.ScrollBar.AlwaysOff }

                    ColumnLayout {
                        id: sideCol
                        x: sheet.pad
                        y: sheet.pad
                        width: sidePane.width - sheet.pad * 2 - sheet.gutter
                        spacing: Kirigami.Units.largeSpacing

                        Kirigami.Separator { Layout.fillWidth: true; visible: sheet.stacked }

                        // ---- Queue (QOL §5): "Queue · 12 · 48 min left" + Shuffle · Clear played · Clear ----
                        RowLayout {
                            Layout.fillWidth: true
                            spacing: Kirigami.Units.smallSpacing
                            Controls.Label {
                                Layout.fillWidth: true
                                textFormat: Text.PlainText
                                text: "Queue" + (sheet.queue.length > 0 ? " · " + sheet.queue.length : "") + (sheet.upcomingLabel.length > 0 ? " · " + sheet.upcomingLabel : "")
                                color: Th.watchpartySectionFg
                                font.bold: true
                                elide: Text.ElideRight
                            }
                            ThToolButton {
                                app: sheet.app
                                visible: sheet.upcomingCount >= 2
                                enabled: sheet.canControl
                                iconName: "media-playlist-shuffle-symbolic"
                                iconSize: 16
                                label: sideCol.width >= 470 ? "Shuffle" : ""
                                tip: !sheet.canControl && sheet.inParty ? "Only the host (" + sheet.hostName + ") can shuffle the queue" : "Shuffle — what's up next, for everyone"
                                onClicked: sheet.shuffleUpcoming()
                            }
                            ThToolButton {
                                app: sheet.app
                                visible: sheet.nowIndex > 0
                                enabled: sheet.canControl
                                iconName: "edit-clear-history-symbolic"
                                iconSize: 16
                                label: sideCol.width >= 470 ? "Clear played" : ""
                                tip: !sheet.canControl && sheet.inParty ? "Only the host (" + sheet.hostName + ") can clear the queue" : "Clear played — remove what already played, for everyone"
                                onClicked: sheet.clearPlayed()
                            }
                            ThButton {   // 11.4: Clear (host-gated when a host is set)
                                app: sheet.app
                                flat: true
                                visible: sheet.queue.length > 0
                                enabled: sheet.canControl
                                text: "Clear"
                                Controls.ToolTip.text: !sheet.canControl && sheet.inParty ? "Only the host (" + sheet.hostName + ") can clear the queue" : "Clear the queue (for everyone)"
                                Controls.ToolTip.visible: hovered
                                onClicked: sheet.clearQueue()
                            }
                        }
                        ListView {
                            id: queueView
                            Layout.fillWidth: true
                            Layout.preferredHeight: Math.min(contentHeight, sheet.wide ? Math.max(Kirigami.Units.gridUnit * 12, sidePane.height * 0.42) : Kirigami.Units.gridUnit * 12)
                            visible: sheet.queue.length > 0
                            clip: true
                            model: sheet.queue
                            spacing: Kirigami.Units.smallSpacing
                            cacheBuffer: 4000   // a drag never loses its row to delegate recycling
                            interactive: sheet.dragFrom < 0
                            boundsBehavior: Flickable.StopAtBounds
                            readonly property real rowH: Kirigami.Units.gridUnit * 2.4
                            readonly property real pitch: rowH + spacing
                            Controls.ScrollBar.vertical: Controls.ScrollBar { policy: Controls.ScrollBar.AsNeeded }
                            delegate: Rectangle {
                                id: qRow
                                required property var modelData
                                required property int index
                                readonly property string url: String(modelData.url || "")
                                readonly property bool isNow: url.length > 0 && url === sheet.nowUrl
                                readonly property bool played: sheet.nowIndex >= 0 && index < sheet.nowIndex
                                readonly property bool upNext: sheet.nowIndex >= 0 && index === sheet.nowIndex + 1
                                readonly property bool lit: (qHover.hovered && sheet.dragFrom < 0) || qPlay.activeFocus || qNext.activeFocus || qRemove.activeFocus
                                readonly property string adder: String(modelData.addedBy || "")
                                width: ListView.view ? ListView.view.width : 0
                                height: queueView.rowH
                                radius: sheet.theme.radiusSm
                                opacity: sheet.dragFrom === index ? 0.4 : 1
                                color: qRow.lit ? Th.watchpartyQueueRowBgHover : Th.watchpartyQueueRowBg
                                border.width: 1
                                border.color: (qRow.lit || isNow) ? Th.watchpartyQueueRowBorderHover : Th.watchpartyQueueRowBorder
                                HoverHandler { id: qHover }
                                // the row: right-click = the menu, double-click = Play now, a press + 6 px of travel = drag
                                MouseArea {
                                    id: qMa
                                    anchors.fill: parent
                                    acceptedButtons: Qt.LeftButton | Qt.RightButton
                                    preventStealing: true
                                    property real pressY: 0
                                    onPressed: (m) => { pressY = m.y; if (m.button === Qt.RightButton) sheet.openRowMenu(qRow.index) }
                                    onPositionChanged: (m) => {
                                        if (!(m.buttons & Qt.LeftButton)) return
                                        if (sheet.dragFrom < 0 && Math.abs(m.y - pressY) > 6) sheet.beginDrag(qRow.index)
                                        if (sheet.dragFrom >= 0) sheet.dragMove(mapToItem(queueView, m.x, m.y).y)
                                    }
                                    onReleased: if (sheet.dragFrom >= 0) sheet.endDrag()
                                    onCanceled: sheet.cancelDrag()
                                    onDoubleClicked: (m) => { if (m.button === Qt.LeftButton) sheet.playNowAt(qRow.index) }
                                }
                                RowLayout {
                                    anchors.fill: parent
                                    anchors.leftMargin: 2
                                    anchors.rightMargin: Kirigami.Units.smallSpacing
                                    spacing: Kirigami.Units.smallSpacing
                                    opacity: qRow.played ? 0.55 : 1   // QOL §5: played rows (above the playing one)
                                    Item {   // the grip: press = drag at once
                                        implicitWidth: 18; implicitHeight: qRow.height
                                        Grid {   // the 2 × 3 dotted grip (the web's)
                                            anchors.centerIn: parent
                                            columns: 2; spacing: 3
                                            opacity: qRow.lit ? 1 : 0.55
                                            Repeater { model: 6; delegate: Rectangle { width: 3; height: 3; radius: 1.5; color: Th.watchpartyMetaFg } }
                                        }
                                        MouseArea {
                                            anchors.fill: parent
                                            enabled: sheet.inParty
                                            cursorShape: sheet.dragFrom >= 0 ? Qt.ClosedHandCursor : Qt.OpenHandCursor
                                            preventStealing: true
                                            onPressed: sheet.beginDrag(qRow.index)
                                            onPositionChanged: (m) => sheet.dragMove(mapToItem(queueView, m.x, m.y).y)
                                            onReleased: sheet.endDrag()
                                            onCanceled: sheet.cancelDrag()
                                        }
                                    }
                                    // the number, or the playing row's "Now playing" tag (accent); the next row's "Up next"
                                    Controls.Label { visible: !qRow.isNow; textFormat: Text.PlainText; text: (qRow.index + 1) + "."; color: Th.watchpartyMetaFg; Layout.minimumWidth: 18 }
                                    Rectangle {
                                        visible: qRow.isNow
                                        implicitWidth: nowTag.implicitWidth + 12; implicitHeight: nowTag.implicitHeight + 4; radius: height / 2
                                        color: Th.watchpartyClickToPlayBg
                                        Controls.Label { id: nowTag; anchors.centerIn: parent; text: "Now playing"; font.bold: true; font.pointSize: Kirigami.Theme.smallFont.pointSize; color: Th.watchpartyClickToPlayFg }
                                    }
                                    Rectangle {
                                        visible: qRow.upNext
                                        implicitWidth: nextTag.implicitWidth + 12; implicitHeight: nextTag.implicitHeight + 4; radius: height / 2
                                        color: Th.watchpartyQueueRowBgHover; border.width: 1; border.color: Th.watchpartyQueueRowBorderHover
                                        Controls.Label { id: nextTag; anchors.centerIn: parent; text: "Up next"; font.bold: true; font.pointSize: Kirigami.Theme.smallFont.pointSize; color: Th.watchpartyFg }
                                    }
                                    Image {   // 11.4: the item's art
                                        visible: source.toString().length > 0 && status === Image.Ready
                                        source: LinkGate.imageSrc(qRow.modelData.art)   // review #5: https only
                                        Layout.preferredWidth: 32; Layout.preferredHeight: 24
                                        fillMode: Image.PreserveAspectCrop
                                        asynchronous: true
                                        sourceSize.width: 64; sourceSize.height: 48
                                    }
                                    Controls.Label {
                                        Layout.fillWidth: true
                                        textFormat: Text.PlainText
                                        text: ((qRow.modelData.title && qRow.modelData.title.length > 0) ? qRow.modelData.title : qRow.url)
                                              + ((qRow.modelData.artist && String(qRow.modelData.artist).length > 0) ? " · " + qRow.modelData.artist : "")
                                        color: Th.watchpartyQueueRowFg
                                        elide: Text.ElideRight
                                    }
                                    Rectangle {   // 11.4: source badge
                                        visible: sheet.badgeOf(qRow.modelData).length > 0 && queueView.width >= 480
                                        implicitWidth: qBadge.implicitWidth + 12; implicitHeight: qBadge.implicitHeight + 2; radius: 8
                                        color: Th.watchpartyQueueRowBg; border.width: 1; border.color: Th.watchpartyQueueRowBorder
                                        Controls.Label { id: qBadge; anchors.centerIn: parent; textFormat: Text.PlainText; text: sheet.badgeOf(qRow.modelData); color: Th.watchpartyMetaFg; font.pointSize: Kirigami.Theme.smallFont.pointSize }
                                    }
                                    // 10.1 (TH-WP-12) / 11.3 QOL: who queued it — the avatar, the name in its tooltip
                                    Avatar {
                                        visible: qRow.adder.length > 0
                                        app: sheet.app; size: 18
                                        name: sheet.nameOf(qRow.adder); seed: qRow.adder; imageSource: sheet.avatarOf(qRow.adder)
                                        HoverHandler { id: adderHover }
                                        Controls.ToolTip.visible: adderHover.hovered
                                        Controls.ToolTip.text: "Added by " + (qRow.adder === sheet.myId ? "you" : sheet.nameOf(qRow.adder))
                                        Controls.ToolTip.delay: 400
                                    }
                                }
                                // the hover / keyboard-focus actions float over the row's right end (no reflow: the title
                                // keeps its width; they cover the badge + avatar while shown)
                                Rectangle {
                                    anchors.right: parent.right
                                    anchors.rightMargin: 1
                                    anchors.verticalCenter: parent.verticalCenter
                                    width: qActs.implicitWidth + Kirigami.Units.smallSpacing * 2
                                    height: parent.height - 2
                                    radius: sheet.theme.radiusSm
                                    color: Th.watchpartyQueueRowBgHover
                                    opacity: qRow.lit ? 1 : 0
                                    RowLayout {
                                        id: qActs
                                        anchors.centerIn: parent
                                        spacing: 0
                                        ThToolButton { id: qPlay; app: sheet.app; implicitWidth: 32; implicitHeight: 32; iconName: "media-playback-start"; iconSize: 16; tip: sheet.canControl ? "Play now" : sheet.controlRefusal; enabled: sheet.canControl && !qRow.isNow; onClicked: sheet.playNowAt(qRow.index) }
                                        ThToolButton { id: qNext; app: sheet.app; implicitWidth: 32; implicitHeight: 32; iconName: "media-playlist-append"; iconSize: 16; tip: "Play next"; enabled: sheet.inParty && !qRow.isNow && !qRow.upNext && !(sheet.nowIndex < 0 && qRow.index === 0); onClicked: sheet.moveToNext(qRow.index) }
                                        ThToolButton { id: qRemove; app: sheet.app; implicitWidth: 32; implicitHeight: 32; iconName: "edit-delete-remove-symbolic"; iconSize: 16; tip: "Remove from the queue"; enabled: sheet.inParty; onClicked: sheet.removeAt(qRow.index) }
                                    }
                                }
                            }
                            // the drop line + the dragged row's ghost (children of the content: they scroll with it)
                            Rectangle {
                                visible: sheet.dragFrom >= 0 && sheet.dragIns !== sheet.dragFrom && sheet.dragIns !== sheet.dragFrom + 1
                                x: 0; width: queueView.width; height: 3; radius: 1.5; z: 12
                                y: queueView.originY + sheet.dragIns * queueView.pitch - queueView.spacing / 2 - 1.5
                                color: Th.watchpartyQueueRowBorderHover
                            }
                            Rectangle {
                                visible: sheet.dragFrom >= 0
                                x: 0; width: queueView.width; height: queueView.rowH; z: 11
                                y: queueView.contentY + sheet.dragY - height / 2
                                radius: sheet.theme.radiusSm
                                color: Th.watchpartyQueueRowBgHover
                                border.width: 1; border.color: Th.watchpartyQueueRowBorderHover
                                opacity: 0.85
                                Controls.Label {
                                    anchors.verticalCenter: parent.verticalCenter
                                    x: 26; width: parent.width - 36
                                    readonly property var it: (sheet.dragFrom >= 0 && sheet.dragFrom < sheet.queue.length) ? sheet.queue[sheet.dragFrom] : null
                                    textFormat: Text.PlainText
                                    text: it ? String(it.title || it.url || "") : ""
                                    color: Th.watchpartyQueueRowFg
                                    elide: Text.ElideRight
                                }
                            }
                        }
                        Controls.Label {
                            Layout.fillWidth: true
                            visible: sheet.queue.length === 0
                            text: "Queue is empty — paste a link or search below and add something."
                            color: Th.watchpartyMetaFg
                            font.pointSize: Kirigami.Theme.smallFont.pointSize
                            wrapMode: Text.WordWrap
                        }

                        Kirigami.Separator { Layout.fillWidth: true }

                        // ---- 11.4 the one box: a URL adds, words search YouTube (via the server's Tin Hat API) ----
                        RowLayout {
                            Layout.fillWidth: true
                            Controls.TextField {
                                id: searchField
                                Layout.fillWidth: true
                                placeholderText: sheet.searchOn ? "Paste a link, or search YouTube…" : "Paste a link…"
                                onTextChanged: sheet.onBoxChanged()
                                // selfhost: words + Enter without the server's search say so (the web's add flow)
                                onAccepted: {
                                    sheet.suggest("")
                                    if (sheet.boxIsLink) sheet.submitBox(false)
                                    else if (!sheet.searchOn && searchField.text.trim().length > 0) sheet.addNote = "Paste a link to a video or song — searching isn't available on this server."
                                }
                                // 11.3 QOL (§4): Esc here = hide the suggestions → else clear → else leave the box; never closes
                                Keys.onEscapePressed: (event) => {
                                    event.accepted = true
                                    if (sugBox.visible) sheet.suggest("")
                                    else if (searchField.text.length > 0) searchField.text = ""
                                    else body.forceActiveFocus()
                                }
                            }
                            readonly property bool addable: sheet.boxIsLink && sheet.boxLink.kind !== "playlist" && sheet.boxLink.kind !== "channel"
                            ThButton { app: sheet.app; visible: parent.addable; highlighted: true; enabled: !sheet.adding; text: "Add"; onClicked: sheet.submitBox(false) }
                            ThButton { app: sheet.app; visible: parent.addable; enabled: !sheet.adding && sheet.canControl; text: "Play now"; onClicked: sheet.submitBox(true) }
                            ThBusyIndicator {   // 11.0 (GPU.md §2.5): still
                                running: sheet.adding
                                visible: sheet.adding
                                size: 27
                                color: Th.watchpartyBusyFg
                            }
                        }
                        Controls.Label {
                            Layout.fillWidth: true
                            visible: sheet.addNote.length > 0
                            textFormat: Text.PlainText
                            text: sheet.addNote
                            color: Th.watchpartyMetaFg
                            font.pointSize: Kirigami.Theme.smallFont.pointSize
                            wrapMode: Text.WordWrap
                        }
                        // ---- 11.4 (DESIGN §2.5) a song that stays a card: art · title · artist · badge · Preview 30 s · Open in … ----
                        Rectangle {
                            Layout.fillWidth: true
                            visible: sheet.songCard !== null
                            implicitHeight: cardRow.implicitHeight + 12
                            radius: 6
                            color: Th.watchpartyResultRowBg
                            border.width: 1; border.color: Th.watchpartyResultRowBorder
                            readonly property var meta: (sheet.songCard && sheet.songCard.meta) ? sheet.songCard.meta : ({})
                            readonly property var link: (sheet.songCard && sheet.songCard.link) ? sheet.songCard.link : ({})
                            RowLayout {
                                id: cardRow
                                anchors.fill: parent; anchors.margins: 6
                                spacing: Kirigami.Units.largeSpacing
                                Rectangle {
                                    implicitWidth: 48; implicitHeight: 48; radius: 6; clip: true
                                    color: Th.watchpartyQueueRowBg; border.width: 1; border.color: Th.watchpartyQueueRowBorder
                                    Image { anchors.fill: parent; source: LinkGate.imageSrc(parent.parent.parent.meta.thumb); visible: status === Image.Ready; fillMode: Image.PreserveAspectCrop; asynchronous: true; sourceSize.width: 96; sourceSize.height: 96 }
                                    Kirigami.Icon { anchors.centerIn: parent; visible: LinkGate.imageSrc(parent.parent.parent.meta.thumb) === ""; source: "emblem-music-symbolic"; color: Th.watchpartyEmptyIcon; implicitWidth: 22; implicitHeight: 22 }
                                }
                                ColumnLayout {
                                    Layout.fillWidth: true
                                    spacing: 0
                                    Controls.Label { Layout.fillWidth: true; textFormat: Text.PlainText; text: String(parent.parent.parent.meta.title || parent.parent.parent.link.input || ""); color: Th.watchpartyFg; font.bold: true; elide: Text.ElideRight }
                                    Controls.Label { Layout.fillWidth: true; visible: text.length > 0; textFormat: Text.PlainText; text: [String(parent.parent.parent.meta.artist || ""), String(parent.parent.parent.meta.album || "")].filter(function (x) { return x.length > 0 }).join(" · "); color: Th.watchpartyMetaFg; font.pointSize: Kirigami.Theme.smallFont.pointSize; elide: Text.ElideRight }
                                    Controls.Label { Layout.fillWidth: true; textFormat: Text.PlainText; text: sheet.songCard ? String(sheet.songCard.message || "") : ""; color: Th.watchpartyMetaFg; font.pointSize: Kirigami.Theme.smallFont.pointSize; wrapMode: Text.WordWrap }
                                }
                                Rectangle {   // source badge
                                    readonly property string b: sheet.badgeOf({ source: { service: String(parent.parent.link.service || "") } })
                                    visible: b.length > 0
                                    implicitWidth: cBadge.implicitWidth + 12; implicitHeight: cBadge.implicitHeight + 2; radius: 8
                                    color: Th.watchpartyQueueRowBg; border.width: 1; border.color: Th.watchpartyQueueRowBorder
                                    Controls.Label { id: cBadge; anchors.centerIn: parent; textFormat: Text.PlainText; text: parent.b; color: Th.watchpartyMetaFg; font.pointSize: Kirigami.Theme.smallFont.pointSize }
                                }
                                ThButton {
                                    app: sheet.app
                                    visible: String(parent.parent.meta.preview || "").length > 0
                                    text: sheet.previewPlaying ? "Stop preview" : "Preview 30 s"
                                    icon.name: sheet.previewPlaying ? "media-playback-stop" : "media-playback-start"
                                    onClicked: sheet.togglePreview(String(parent.parent.meta.preview || ""))
                                }
                                ThButton { app: sheet.app; highlighted: true; text: sheet.openInLabel(String(parent.parent.link.service || "")); onClicked: LinkGate.open(String(parent.parent.link.input || "")) }
                                ThToolButton { app: sheet.app; iconName: "window-close-symbolic"; iconSize: 16; tip: "Dismiss"; onClicked: { previewPlayer.stop(); sheet.songCard = null } }
                            }
                        }

                        // ---- 11.4 Browse pane (DESIGN §2.4): tabs · chips · autocomplete · rows · listing ----
                        ColumnLayout {
                            Layout.fillWidth: true
                            spacing: Kirigami.Units.smallSpacing
                            // tabs
                            Rectangle {
                                Layout.fillWidth: true
                                implicitHeight: tabRow.implicitHeight + 1
                                color: "transparent"
                                Rectangle { anchors.left: parent.left; anchors.right: parent.right; anchors.bottom: parent.bottom; height: 1; color: Th.watchpartySectionDivider }
                                Row {
                                    id: tabRow
                                    spacing: 2
                                    Repeater {
                                        // selfhost: without the server's YouTube search (watch: false) only History
                                        model: sheet.searchOn ? [ { id: "search", label: "Search" }, { id: "music", label: "Music" }, { id: "trending", label: "Trending" }, { id: "playlists", label: "Playlists" }, { id: "history", label: "History" } ]
                                                              : [ { id: "history", label: "History" } ]
                                        delegate: Item {
                                            required property var modelData
                                            readonly property bool on: sheet.browseTab === modelData.id
                                            width: tabLbl.implicitWidth + 20; height: tabLbl.implicitHeight + 11
                                            Controls.Label { id: tabLbl; anchors.centerIn: parent; anchors.verticalCenterOffset: -1; textFormat: Text.PlainText; text: parent.modelData.label; font.bold: true; color: (parent.on || tabMa.containsMouse) ? Th.watchpartyFg : Th.watchpartyMetaFg }
                                            Rectangle { anchors.left: parent.left; anchors.right: parent.right; anchors.bottom: parent.bottom; height: 2; visible: parent.on; color: Th.watchpartyQueueRowBorderHover }
                                            MouseArea { id: tabMa; anchors.fill: parent; hoverEnabled: true; cursorShape: Qt.PointingHandCursor; onClicked: sheet.setTab(parent.modelData.id) }
                                        }
                                    }
                                }
                            }
                            // chips: search types / trending categories
                            Flow {
                                Layout.fillWidth: true
                                spacing: 4
                                visible: sheet.browseTab === "search" || sheet.browseTab === "trending"
                                Repeater {
                                    model: sheet.browseTab === "trending"
                                           ? [ { id: 0, label: "All" }, { id: 10, label: "Music" }, { id: 20, label: "Gaming" }, { id: 1, label: "Film" }, { id: 24, label: "Entertainment" }, { id: 17, label: "Sports" }, { id: 28, label: "Science" } ]
                                           : [ { id: "video", label: "Videos" }, { id: "music", label: "Music" }, { id: "playlist", label: "Playlists" }, { id: "channel", label: "Channels" } ]
                                    delegate: Rectangle {
                                        required property var modelData
                                        readonly property bool on: sheet.browseTab === "trending" ? sheet.trendingCat === modelData.id : sheet.searchType === modelData.id
                                        implicitWidth: chipLbl.implicitWidth + 20; implicitHeight: chipLbl.implicitHeight + 4; radius: 10
                                        color: (on || chipMa.containsMouse) ? Th.watchpartyQueueRowBgHover : Th.watchpartyQueueRowBg
                                        border.width: 1; border.color: on ? Th.watchpartyQueueRowBorderHover : Th.watchpartyQueueRowBorder
                                        Controls.Label { id: chipLbl; anchors.centerIn: parent; textFormat: Text.PlainText; text: parent.modelData.label; font.pointSize: Kirigami.Theme.smallFont.pointSize; color: parent.on ? Th.watchpartyFg : Th.watchpartyMetaFg }
                                        MouseArea {
                                            id: chipMa; anchors.fill: parent; hoverEnabled: true; cursorShape: Qt.PointingHandCursor
                                            onClicked: { if (sheet.browseTab === "trending") { sheet.trendingCat = parent.modelData.id; sheet.runBrowse() } else { sheet.searchType = parent.modelData.id; if (searchField.text.trim().length > 0) sheet.runBrowse() } }
                                        }
                                    }
                                }
                            }
                            // autocomplete (Search tab, while typing words)
                            Rectangle {
                                id: sugBox
                                Layout.fillWidth: true
                                visible: sheet.browseTab === "search" && !sheet.boxIsLink && searchField.text.length > 0 && sheet.suggestions.length > 0
                                implicitHeight: sugCol.implicitHeight
                                radius: sheet.theme.radiusSm
                                color: Th.watchpartyResultRowBg
                                border.width: 1; border.color: Th.watchpartyResultRowBorder
                                clip: true
                                ColumnLayout {
                                    id: sugCol
                                    anchors.left: parent.left; anchors.right: parent.right
                                    spacing: 0
                                    Repeater {
                                        model: sheet.suggestions
                                        delegate: Rectangle {
                                            required property var modelData
                                            Layout.fillWidth: true
                                            implicitHeight: sugLbl.implicitHeight + 8
                                            color: sugMa.containsMouse ? Th.watchpartyResultRowBgHover : "transparent"
                                            Controls.Label { id: sugLbl; anchors.left: parent.left; anchors.leftMargin: 8; anchors.verticalCenter: parent.verticalCenter; textFormat: Text.PlainText; text: String(parent.modelData); color: Th.watchpartyResultRowFg }
                                            MouseArea { id: sugMa; anchors.fill: parent; hoverEnabled: true; cursorShape: Qt.PointingHandCursor; onClicked: { searchField.text = String(parent.modelData); sheet.suggest(""); searchField.forceActiveFocus() } }
                                        }
                                    }
                                }
                            }
                            Controls.Label {
                                Layout.fillWidth: true
                                visible: sheet.addNote.length === 0 && sheet.browseError.length > 0 && sheet.browseTab !== "history" && sheet.browseTab !== "playlists"
                                textFormat: Text.PlainText
                                text: sheet.browseError
                                color: Th.watchpartyMetaFg
                                font.pointSize: Kirigami.Theme.smallFont.pointSize
                                wrapMode: Text.WordWrap
                            }
                            ThBusyIndicator {   // 11.0 (GPU.md §2.5): still
                                Layout.alignment: Qt.AlignHCenter
                                running: sheet.browsing
                                visible: sheet.browsing && ((sheet.browseTab !== "playlists" && sheet._browseAcc.length === 0) || (sheet.browseTab === "playlists" && sheet.listing === null))
                                size: 27
                                color: Th.watchpartyBusyFg
                            }
                            // "No results" feedback so a query that returns nothing doesn't just go blank.
                            Controls.Label {
                                Layout.fillWidth: true
                                horizontalAlignment: Text.AlignHCenter
                                visible: (sheet.browseTab === "search" || sheet.browseTab === "music") && !sheet.browsing && !sheet.boxIsLink && searchField.text.trim().length > 0 && sheet._browseAcc.length === 0 && sheet.browseError.length === 0
                                text: "No results"
                                color: Th.watchpartyMetaFg
                                padding: Kirigami.Units.smallSpacing
                            }
                            // Playlists tab: the open listing's header
                            Controls.Label {
                                Layout.fillWidth: true
                                visible: sheet.browseTab === "playlists" && sheet.listing === null && !sheet.browsing
                                textFormat: Text.PlainText
                                text: sheet.browseError.length > 0 ? sheet.browseError : "Paste a playlist or channel link in the box above, or open one from the search results."
                                color: Th.watchpartyMetaFg
                                font.pointSize: Kirigami.Theme.smallFont.pointSize
                                wrapMode: Text.WordWrap
                            }
                            Rectangle {
                                Layout.fillWidth: true
                                visible: sheet.browseTab === "playlists" && sheet.listing !== null
                                implicitHeight: listRow.implicitHeight + 8
                                radius: sheet.theme.radiusSm
                                color: Th.watchpartyQueueRowBg
                                border.width: 1; border.color: Th.watchpartyQueueRowBorder
                                RowLayout {
                                    id: listRow
                                    anchors.fill: parent; anchors.margins: 4
                                    spacing: Kirigami.Units.smallSpacing
                                    Image { visible: source.toString().length > 0; source: LinkGate.imageSrc(sheet.listing ? sheet.listing.thumb : ""); Layout.preferredWidth: 72; Layout.preferredHeight: 43; fillMode: Image.PreserveAspectCrop; asynchronous: true }
                                    ColumnLayout {
                                        Layout.fillWidth: true
                                        spacing: 0
                                        Controls.Label { Layout.fillWidth: true; textFormat: Text.PlainText; text: sheet.listing ? String(sheet.listing.title || "") : ""; color: Th.watchpartyResultRowFg; font.bold: true; elide: Text.ElideRight }
                                        Controls.Label { Layout.fillWidth: true; textFormat: Text.PlainText; text: sheet.listing ? [String(sheet.listing.channel || ""), sheet.listing.itemCount ? sheet.listing.itemCount + " videos" : ""].filter(function (x) { return x.length > 0 }).join(" · ") : ""; color: Th.watchpartyMetaFg; font.pointSize: Kirigami.Theme.smallFont.pointSize; elide: Text.ElideRight }
                                    }
                                    ThButton { app: sheet.app; highlighted: true; text: "Add all"; enabled: sheet.inParty && !!(sheet.listing && sheet.listing.items && sheet.listing.items.length > 0); onClicked: sheet.addAll() }
                                    ThToolButton { app: sheet.app; iconName: "window-close-symbolic"; iconSize: 16; tip: "Close"; onClicked: sheet.listing = null }
                                }
                            }
                            // History tab, empty
                            Controls.Label {
                                Layout.fillWidth: true
                                visible: sheet.browseTab === "history" && sheet.history.length === 0
                                text: "Nothing played here yet — what the party plays shows up here (the last 50)."
                                color: Th.watchpartyMetaFg
                                font.pointSize: Kirigami.Theme.smallFont.pointSize
                                wrapMode: Text.WordWrap
                            }
                            // the rows: search / music / trending items, the listing's items, or the history
                            ListView {
                                id: browseView
                                Layout.fillWidth: true
                                Layout.preferredHeight: Math.min(contentHeight, Kirigami.Units.gridUnit * 18)
                                readonly property bool isHistory: sheet.browseTab === "history"
                                readonly property bool isListing: sheet.browseTab === "playlists"
                                readonly property bool music: sheet.browseTab === "music" || (sheet.browseTab === "search" && sheet.searchType === "music") || (isListing && sheet.listingMusic)
                                model: isHistory ? sheet.history : (isListing ? ((sheet.listing && sheet.listing.items) ? sheet.listing.items : []) : sheet._browseAcc)
                                visible: count > 0
                                clip: true
                                spacing: Kirigami.Units.smallSpacing
                                delegate: Rectangle {
                                    required property var modelData
                                    readonly property bool rowIsHistory: browseView.isHistory
                                    readonly property string rowKind: rowIsHistory ? "video" : String(modelData.kind || "video")
                                    readonly property bool rowQueued: !!sheet.queuedSet[String(modelData.url || "")]
                                    readonly property string metaLine: rowIsHistory
                                        ? [String(modelData.artist || ""), modelData.by ? ("played by " + (String(modelData.by) === sheet.myId ? "you" : sheet.nameOf(String(modelData.by)))) : "", modelData.playedAt ? new Date(Number(modelData.playedAt)).toLocaleDateString() : ""].filter(function (x) { return x.length > 0 }).join(" · ")
                                        : [String(modelData.channel || ""), modelData.durationSec ? sheet.fmtTime(Number(modelData.durationSec)) : "", sheet.fmtViews(modelData.views), modelData.itemCount ? modelData.itemCount + " videos" : ""].filter(function (x) { return x.length > 0 }).join(" · ")
                                    width: ListView.view ? ListView.view.width : 0
                                    height: Kirigami.Units.gridUnit * 3
                                    radius: sheet.theme.radiusSm
                                    color: bMa.containsMouse ? Th.watchpartyResultRowBgHover : Th.watchpartyResultRowBg
                                    border.width: 1
                                    border.color: Th.watchpartyResultRowBorder
                                    function itemJson() { return rowIsHistory ? (function () { var h = JSON.parse(JSON.stringify(modelData)); delete h.playedAt; delete h.by; return h })() : sheet.queueItemOf(modelData, browseView.music) }
                                    RowLayout {
                                        anchors.fill: parent
                                        anchors.margins: Kirigami.Units.smallSpacing
                                        spacing: Kirigami.Units.smallSpacing
                                        Image {
                                            readonly property string src: LinkGate.imageSrc(rowIsHistory ? modelData.art : modelData.thumb)   // review #5
                                            visible: src.length > 0
                                            source: src
                                            Layout.preferredWidth: (rowKind === "channel" || browseView.music) ? 43 : Kirigami.Units.gridUnit * 4
                                            Layout.preferredHeight: Kirigami.Units.gridUnit * 2.4
                                            fillMode: Image.PreserveAspectCrop
                                            asynchronous: true
                                        }
                                        ColumnLayout {
                                            Layout.fillWidth: true
                                            spacing: 0
                                            Controls.Label { Layout.fillWidth: true; textFormat: Text.PlainText; text: modelData.title || modelData.url || ""; color: Th.watchpartyResultRowFg; elide: Text.ElideRight; wrapMode: Text.WordWrap; maximumLineCount: 2 }
                                            Controls.Label { Layout.fillWidth: true; visible: metaLine.length > 0; textFormat: Text.PlainText; text: metaLine; color: Th.watchpartyMetaFg; font.pointSize: Kirigami.Theme.smallFont.pointSize; elide: Text.ElideRight }
                                        }
                                        ThButton { app: sheet.app; visible: rowKind !== "video"; text: rowKind === "playlist" ? "Open playlist" : "Open channel"; onClicked: sheet.openListing(rowKind, String(modelData.id || ""), browseView.music) }
                                        // 11.3 QOL (§6): already queued → a check "In the queue" in place of Add (an Add flips it at once)
                                        RowLayout {
                                            visible: rowKind === "video" && rowQueued
                                            spacing: 4
                                            Kirigami.Icon { source: "checkmark-symbolic"; color: Th.watchpartyMetaFg; implicitWidth: 16; implicitHeight: 16 }
                                            Controls.Label { text: "In the queue"; color: Th.watchpartyMetaFg; font.pointSize: Kirigami.Theme.smallFont.pointSize }
                                        }
                                        ThToolButton { app: sheet.app; visible: rowKind === "video" && !rowQueued; iconName: "list-add"; tip: "Add to queue"; enabled: sheet.inParty; onClicked: { var it = itemJson(); sheet.enqueueItem(JSON.stringify(it)); sheet.markAdded(String(it.url || "")) } }
                                        ThToolButton {
                                            app: sheet.app
                                            visible: rowKind === "video"
                                            iconName: "media-playback-start"
                                            enabled: sheet.canControl   // 10.1 (TH-WP-05)
                                            tip: rowIsHistory ? "Play again" : "Play now"
                                            onClicked: { var it = itemJson(); if (!sheet.queuedSet[String(it.url || "")]) { sheet.enqueueItem(JSON.stringify(it)); sheet.markAdded(String(it.url || "")) } sheet.playUrl(String(it.url || "")) }
                                        }
                                    }
                                    MouseArea { id: bMa; anchors.fill: parent; hoverEnabled: true; acceptedButtons: Qt.NoButton }
                                }
                            }
                            ThButton {
                                app: sheet.app
                                flat: true
                                visible: sheet.browseTab !== "history" && sheet.browseTab !== "trending" && sheet.browseNext.length > 0 && !sheet.browsing
                                         && ((sheet.browseTab === "playlists" && sheet.listing !== null && String(sheet.listing.nextPage || "").length > 0) || (sheet.browseTab !== "playlists" && sheet._browseAcc.length > 0))
                                text: "More"
                                onClicked: sheet.browseMore()
                            }
                        }

                    }
                }
            }
        }

        // ---- 11.3 QOL (§4) the keys card: "?" / the Keys button; Esc / a click outside / "?" close it ----
        Item {
            id: keysCard
            anchors.fill: parent
            visible: false
            z: 100
            Rectangle { anchors.fill: parent; color: Th.scrim }
            MouseArea { anchors.fill: parent; acceptedButtons: Qt.AllButtons; onClicked: keysCard.visible = false; onWheel: (wheel) => { wheel.accepted = true } }
            Rectangle {
                id: keysPanel
                anchors.centerIn: parent
                width: Math.min(parent.width - 24, 560)
                height: Math.min(parent.height - 24, keysCol.implicitHeight + 32)
                radius: sheet.theme.radius
                color: Th.watchpartyResultRowBg
                border.width: 1
                border.color: Th.watchpartyResultRowBorder
                MouseArea { anchors.fill: parent; acceptedButtons: Qt.AllButtons }   // a click on the card stays on it
                Flickable {
                    anchors.fill: parent
                    anchors.margins: 16
                    clip: true
                    contentWidth: width
                    contentHeight: keysCol.implicitHeight
                    boundsBehavior: Flickable.StopAtBounds
                    interactive: contentHeight > height
                    ColumnLayout {
                        id: keysCol
                        width: parent.width
                        spacing: Kirigami.Units.smallSpacing
                        RowLayout {
                            Layout.fillWidth: true
                            Kirigami.Heading { level: 3; text: "Keys"; color: Th.watchpartyFg }
                            Item { Layout.fillWidth: true }
                            ThToolButton { app: sheet.app; iconName: "window-close-symbolic"; iconSize: 16; tip: "Close (Esc)"; onClicked: keysCard.visible = false }
                        }
                        // the shared table (shortcuts.json), grouped Playback · Sound · View · Queue
                        Repeater {
                            model: [
                                { group: "Playback", rows: [ ["Space", "K"], "Play / pause for everyone", ["←", "→"], "Back / forward 5 s", ["J", "L"], "Back / forward 10 s", ["0", "–", "9"], "Jump to 0 – 90 %" ] },
                                { group: "Sound", rows: [ ["↑", "↓"], "Volume up / down (only for you)", ["M"], "Mute (only for you)" ] },
                                { group: "View", rows: [ ["F"], "Fullscreen", ["T"], "Theater (the video fills the window)", ["I"], "Mini player", ["Esc"], "Leave fullscreen or the mini player, else close" ] },
                                { group: "Queue", rows: [ ["N"], "Play next in queue", ["/"], "Search or paste a link", ["?"], "Show the keys" ] }
                            ]
                            delegate: ColumnLayout {
                                id: kGroup
                                required property var modelData
                                Layout.fillWidth: true
                                spacing: 4
                                Controls.Label { textFormat: Text.PlainText; text: kGroup.modelData.group; color: Th.watchpartySectionFg; font.bold: true; Layout.topMargin: 6 }
                                Repeater {
                                    model: kGroup.modelData.rows.length / 2
                                    delegate: RowLayout {
                                        id: kRow
                                        required property int index
                                        readonly property var keys: kGroup.modelData.rows[index * 2]
                                        readonly property string what: kGroup.modelData.rows[index * 2 + 1]
                                        Layout.fillWidth: true
                                        spacing: Kirigami.Units.largeSpacing
                                        Row {
                                            Layout.preferredWidth: 110
                                            spacing: 4
                                            Repeater {
                                                model: kRow.keys
                                                delegate: Rectangle {
                                                    required property var modelData
                                                    readonly property bool sep: String(modelData) === "–"
                                                    implicitWidth: sep ? kcap.implicitWidth : Math.max(24, kcap.implicitWidth + 12)
                                                    implicitHeight: kcap.implicitHeight + 6
                                                    width: implicitWidth; height: implicitHeight
                                                    radius: 5
                                                    color: sep ? "transparent" : Th.watchpartyQueueRowBgHover
                                                    border.width: sep ? 0 : 1
                                                    border.color: Th.watchpartyQueueRowBorder
                                                    Controls.Label { id: kcap; anchors.centerIn: parent; textFormat: Text.PlainText; text: String(parent.modelData); color: Th.watchpartyFg; font.bold: true; font.pointSize: Kirigami.Theme.smallFont.pointSize }
                                                }
                                            }
                                        }
                                        Controls.Label { Layout.fillWidth: true; textFormat: Text.PlainText; text: kRow.what; color: Th.watchpartyMetaFg; wrapMode: Text.WordWrap }
                                    }
                                }
                            }
                        }
                        Controls.Label {
                            Layout.fillWidth: true
                            Layout.topMargin: 6
                            text: "Keys work whenever the search box isn't focused. In the box, Esc hides the suggestions, then clears it, then leaves it. The video: click = play / pause, double-click = fullscreen, wheel = volume."
                            color: Th.watchpartyMetaFg
                            font.pointSize: Kirigami.Theme.smallFont.pointSize
                            wrapMode: Text.WordWrap
                        }
                    }
                }
            }
        }
    }
}
