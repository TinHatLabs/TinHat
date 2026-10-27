// MemoriesWindow.qml — the DVR ("Memories") player. 0.8.10 made it RELIVE the moment:
// every visible track renders at once like a live call — the focused stream big in
// the center, other streams/cams as side tiles (click to focus), point-of-view chips
// under the pane. LIPSYNC: the focused POV plays its recorder-built COMBINED file
// (video + the aligned mixed chat audio in one mkv) so mpv's own sync engine owns A/V;
// side tiles are video-only gst pipelines and only need coarse visual correction.
// keep-open=yes everywhere: a track ending early (cam unpublished at hangup) or the
// session ending holds its last frame instead of killing the player.
//
// 0.8.15 polish pass ("a proper final product"): library cards with posters, people
// and sizes; a real transport (skip, volume, mute, theater/fullscreen, keyboard);
// joins-at / left-at awareness on tiles; a POV that hasn't started yet no longer
// yanks the clock forward (P4 — the chat audio carries the timeline until it does);
// resume-where-you-left-off within a session of the app; error/empty states that say
// what happened; clip chips and markers themed; everything on the app's tokens.
//
// SYNC-WITH-CALL rides the watch-party clock (tinhat-memory://room/session, in-call
// only); people outside the call replay independently.
//
// 0.8.16 rail + points-of-view rework (field report: "the right side cards are still
// very odd", "changing the side cars to main focus is not very clear", "points of view
// needs a clean rework"): the rail is a column of uniform 16:9 cards with a name strip
// (avatar · person · camera/stream), a hover "Watch" affordance, and STICKY slots — a
// track keeps its pipeline slot for the whole replay, so re-ordering (active tracks
// first, then people who join later, then people who left) never rebuilds a pipeline,
// and putting a tile on stage is a visible SWAP: the previous stage occupant drops into
// the exact card you clicked. Tiles stay veiled until they are actually playing at the
// master clock (no file-start flash — the source of the green garbage frames in the
// field), and the side pipelines now land EXACT (ACCURATE) seeks. The points-of-view
// bar is grouped by person with plain labels ("Camera", "Stream 2 · joins 1:08:28"), the
// stage state is explicit, and 1-9 put a point of view on stage from the keyboard.
import QtQuick
import QtQuick.Window
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the memories.* tokens
import dev.tinhat.mpv

Window {
    id: mem
    required property var app
    // 10.1 (TH-CAL-22 / TH-MEM-08 / TH-WP-15): remembered size (+ position off Wayland) across runs.
    WindowMemory { target: mem; key: "memories"; store: (mem.app && mem.app.prefStore) ? mem.app.prefStore : null }
    property string roomId: ""
    property string roomName: ""
    property var memories: []
    property string memoriesError: ""
    // 10.1 (TH-MEM-02): the list is being fetched — "No memories here yet" waits for the answer.
    property bool loading: false
    onRefreshRequested: { mem.loading = true; loadingCap.restart() }
    onMemoriesChanged: { mem.loading = false; mem._openPending() }
    // widgets W2 (the Memories widget, Main.openMemory): open THIS session once the room's list holds it — through
    // tryOpen, so a live or processing one shows its notice like a click on its card; forgotten after 20 s
    property string pendingOpen: ""
    onPendingOpenChanged: if (mem.pendingOpen.length > 0) { pendingCap.restart(); mem._openPending() }
    Timer { id: pendingCap; interval: 20000; onTriggered: mem.pendingOpen = "" }
    function _openPending() {
        if (mem.pendingOpen.length === 0) return
        var list = mem.memories || []
        for (var i = 0; i < list.length; ++i) {
            var s = list[i]
            if (String(s.id) !== mem.pendingOpen || (s.room && String(s.room) !== mem.roomId)) continue
            mem.pendingOpen = ""
            if (mem.sel !== null) mem.backToList()
            mem.tryOpen(s)
            return
        }
    }
    onMemoriesErrorChanged: if (memoriesError.length > 0) mem.loading = false
    Timer { id: loadingCap; interval: 8000; onTriggered: mem.loading = false }   // never spins forever
    property var clips: []
    property bool inCall: false
    property var watchPlayback: ({})
    property bool watchInParty: false

    signal refreshRequested()
    function shortError(e) {
        var s = String(e || "").replace(/<[^>]*>/g, " ").replace(/\s+/g, " ").trim()
        return s.length > 90 ? s.substring(0, 90) + "…" : s
    }
    signal syncStart(string memUrl)
    signal syncStop()
    signal syncPlay()
    signal syncPause()
    signal syncSeek(string posSecs)
    signal syncReport(string posSecs)

    FallbackTheme { id: fallbackTheme }
    readonly property var theme: app ? app : fallbackTheme

    // ---- selection / playback state ----
    property var sel: null                 // open session (null = list view)
    property string focusedFile: ""        // file of the focused VIDEO track ("" = audio-only)
    property var hiddenFiles: ({})         // file -> true (show/hide chips)
    property bool playing: false
    property real globalPos: 0             // seconds since session start
    property string audioUrl: ""
    property real audioOffset: 0
    property real audioDur: 0
    property bool mainReady: false
    property string mMainLoaded: ""        // "video|audio" load signature latch
    property bool audReady: false
    property string mAudLoaded: ""
    property bool mAudioAttached: false
    // P2: replay reached the end (focused POV or whole session) — playing is false,
    // clock parked at the end; ▶ restarts from 0. Cleared by any seek/open.
    property bool atEnd: false
    // P7: the user explicitly chose 🎧 Audio only (vs landing there by accident, the
    // recurring audio-only symptom the self-heal below corrects).
    property bool userChoseAudioOnly: false
    // P3: audio-only surface load latch — audioMpv has no duration right after a
    // loadfile, so a plain seek was dropped and playback restarted at 0:00.
    property bool audioLoadPending: false
    property real audioPendingSeek: -1
    // T3: the machine's gst H.264 decoder is unusable (probe verdict 2) — banner up,
    // tiles run the thumbnail fallback.
    property bool decoderBroken: false
    // P8: transient list notice (tap a live/processing card) + whether any card is
    // still live/processing (drives the auto-refresh).
    property string listNotice: ""
    property string listNoticeKind: "info"    // info | live | warn
    readonly property bool anyPending: {
        if (mem.sel !== null || !mem.memories) return false
        for (var i = 0; i < mem.memories.length; i++) {
            var m = mem.memories[i]
            if (!m.end_ms || (m.ready === false && !m.empty)) return true
        }
        return false
    }
    // 0.8.15: transport extras. 10.1 (TH-MEM-08): all three survive a restart now.
    property real volume: 100
    property bool muted: false
    property var resumeAt: ({})            // session id -> seconds left off at
    readonly property var store: (mem.app && mem.app.prefStore) ? mem.app.prefStore : null
    property bool _memRestored: false
    // 10.1 (TH-MEM-21): the per-5 s [memsync] / 4 Hz [memtrace] instrumentation only under the gate hooks
    // (or --tinhat-mem-debug) — a two-hour replay wrote ~1,500 long lines that were shipped with bug reports.
    readonly property bool memDebug: Qt.application.arguments.indexOf("--tinhat-memtest") >= 0
                                     || Qt.application.arguments.indexOf("--tinhat-memtest2") >= 0
                                     || Qt.application.arguments.indexOf("--tinhat-mem-debug") >= 0
    function _restoreTransport() {
        if (!store || _memRestored) return
        _memRestored = true
        var v = parseFloat(store.uiPref("memories-volume"))
        if (!isNaN(v) && v >= 0 && v <= 100) mem.volume = v
        mem.muted = store.uiPref("memories-muted") === "1"
        try { var raw = store.uiBlob("memories-resume"); if (raw && raw.length > 0) mem.resumeAt = JSON.parse(raw) } catch (e) {}
    }
    Timer { id: memSaveTimer; interval: 800; onTriggered: {
        if (!mem.store || !mem._memRestored) return
        mem.store.applyUiPref("memories-volume", String(Math.round(mem.volume)))
        mem.store.applyUiPref("memories-muted", mem.muted ? "1" : "0")
        // Keep the 80 most recent positions (keys are session ids; the newest sort last).
        var keys = Object.keys(mem.resumeAt || {}).sort(), out = {}
        for (var i = Math.max(0, keys.length - 80); i < keys.length; i++) out[keys[i]] = mem.resumeAt[keys[i]]
        mem.store.applyUiBlob("memories-resume", JSON.stringify(out))
    } }
    onResumeAtChanged: memSaveTimer.restart()
    onVisibleChanged: if (visible) _restoreTransport()
    property string toast: ""
    readonly property bool theater: mem.visibility === Window.FullScreen
    // 10.1 (TH-MEM-10): in fullscreen the header, margins and side rail go; the transport shows
    // while the pointer moves and fades 2.5 s after it stops (unless it is ON the transport).
    property bool chromeIdle: false
    readonly property bool chromeShown: !theater || !chromeIdle
    function _chromeTouch() { chromeIdle = false; chromeIdleT.restart() }
    Timer { id: chromeIdleT; interval: 2500; onTriggered: if (mem.theater && !transportHover.hovered) mem.chromeIdle = true }
    onTheaterChanged: _chromeTouch()

    // Live side tiles: the RoomController bridging QML -> streamer::memtile.
    property var vodctl: null
    // Broadcast a transport landing to every side tile (hard = always seek).
    signal sideResync(bool hard)
    // Focus-swap/scrub load latch: while the main mpv is switching files, hold the
    // clock (masterPos -1), queue seeks (pendingSeek), and land the queued seek +
    // pause state only once mpv reports the NEW file loaded — seeking mid-load was
    // the "changing focus breaks it" bug (aid=auto/avsync=NaN transients
    // and stray "seek absolute" MPV errors in the field log).
    property bool loadPending: false
    property string loadTarget: ""
    property real pendingSeek: -1
    property int _loadTicks: 0

    // ---- 0.8.16 rail state: sticky pipeline slots + display order ----
    // slotFiles[slot] = the track file living in that pipeline slot ("" = free). Slots
    // 0-4 carry LIVE gst pipelines, 5-7 are snapshot-only overflow. A file keeps its
    // slot until it leaves the rail (focused/hidden/session closed), so the visual
    // re-ordering below is free — nothing is rebuilt (P5, finally).
    property var slotFiles: ["", "", "", "", "", "", "", ""]
    // railOrder = files in display order (top → bottom). Recomputed at track
    // boundaries (someone joins/leaves) and on rail membership changes — never per tick.
    property var railOrder: []
    property string _railSig: ""
    property var _slotHint: ({})          // file -> preferred slot (the stage swap)
    property bool _railFrozen: false      // openSession in progress: assign once, at the end
    readonly property real railGap: Kirigami.Units.smallSpacing
    // stage transition: a short dip + the name of what just went on stage
    property string stageFlashText: ""

    readonly property var videoTracks: {
        if (!sel || !sel.tracks) return []
        var v = []
        for (var i = 0; i < sel.tracks.length; i++)
            if (sel.tracks[i].kind === "share" || sel.tracks[i].kind === "cam") v.push(sel.tracks[i])
        // streams (shares) first — they lead the grid, cams follow
        v.sort(function(a, b) {
            var k = (a.kind === "share" ? 0 : 1) - (b.kind === "share" ? 0 : 1)
            if (k !== 0) return k
            return a.file < b.file ? -1 : 1   // deterministic tiebreak (JS sort isn't stable here)
        })
        return v
    }
    function trackByFile(f) {
        for (var i = 0; i < mem.videoTracks.length; i++)
            if (mem.videoTracks[i].file === f) return mem.videoTracks[i]
        return null
    }
    // The video track to open on / heal to: the EARLIEST-starting one so the replay
    // begins at the top (videoTracks[0] is shares-first + filename order, which
    // put a +4109s share first and opened the memory 68 minutes in). Shares win only
    // as a tiebreak among tracks that started within 0.5s of each other.
    function earliestVideoFile() {
        var vts = mem.videoTracks
        if (vts.length === 0) return ""
        var best = null
        for (var j = 0; j < vts.length; j++) {
            var t = vts[j]
            if (!best) { best = t; continue }
            var d = Number(t.start_ms) - Number(best.start_ms)
            if (d < -500) best = t
            else if (Math.abs(d) <= 500 && t.kind === "share" && best.kind !== "share") best = t
        }
        return best ? best.file : ""
    }
    function trackOffset(t) {
        return (t && mem.sel && mem.sel.start_ms) ? Math.max(0, (Number(t.start_ms) - Number(mem.sel.start_ms)) / 1000) : 0
    }
    // Seconds (session clock) at which a track ENDED, or -1 when unknown (sessions
    // recorded before 0.8.15 carry no per-track end_ms until the prep pass ran).
    function trackEnd(t) {
        return (t && t.end_ms && mem.sel && mem.sel.start_ms) ? Math.max(0, (Number(t.end_ms) - Number(mem.sel.start_ms)) / 1000) : -1
    }
    readonly property var focusedTrack: trackByFile(mem.focusedFile)
    readonly property real focusedOffset: (mem.focusedTrack && mem.sel && mem.sel.start_ms)
        ? Math.max(0, (mem.focusedTrack.start_ms - mem.sel.start_ms) / 1000) : 0
    // P4: the focused POV hasn't started yet at this moment of the timeline. The
    // mixed chat audio carries the clock (audioMpv), the stage says when it joins,
    // and the video takes over the instant the timeline reaches its start — focusing
    // a late joiner never yanks everyone's place forward. Needs chat audio to exist.
    readonly property bool focusWaiting: mem.focusedTrack !== null && mem.audioUrl.length > 0
        && mem.globalPos < mem.focusedOffset - 0.3
    // side tiles = visible video tracks that are not the focused one
    readonly property var sideTracks: {
        var out = []
        for (var i = 0; i < mem.videoTracks.length; i++) {
            var t = mem.videoTracks[i]
            if (t.file !== mem.focusedFile && !mem.hiddenFiles[t.file]) out.push(t)
        }
        return out
    }
    // 0 = playing right now, 1 = hasn't joined yet, 2 = already left. Depends on
    // globalPos, so it is evaluated at the 250ms tick (boundary detection), not bound.
    function railRank(t) {
        if (!t) return 3
        var off = mem.trackOffset(t), end = mem.trackEnd(t)
        if (mem.globalPos < off - 0.5) return 1
        if (end > 0 && mem.globalPos > end + 0.5) return 2
        return 0
    }
    function personKey(t) { return mem.shortName(t ? t.identity : "").toLowerCase() }
    // Display order: what's live first (people's cameras before their streams, then by
    // start), then the not-yet-joined by when they join, then the departed (most
    // recent first). Stable tie-break on file so equal keys never shuffle.
    function railOrderedFiles() {
        var arr = []
        var sts = mem.sideTracks
        for (var i = 0; i < sts.length; i++) arr.push(sts[i])
        arr.sort(function(a, b) {
            var ra = mem.railRank(a), rb = mem.railRank(b)
            if (ra !== rb) return ra - rb
            if (ra === 1) { var d1 = mem.trackOffset(a) - mem.trackOffset(b); if (Math.abs(d1) > 0.01) return d1 }
            if (ra === 2) { var d2 = mem.trackEnd(b) - mem.trackEnd(a); if (Math.abs(d2) > 0.01) return d2 }
            var pa = mem.personKey(a), pb = mem.personKey(b)
            if (pa !== pb) return pa < pb ? -1 : 1
            var ka = (a.kind === "cam" ? 0 : 1), kb = (b.kind === "cam" ? 0 : 1)
            if (ka !== kb) return ka - kb
            var d3 = Number(a.start_ms) - Number(b.start_ms)
            if (d3 !== 0) return d3
            return a.file < b.file ? -1 : 1
        })
        var out = []
        for (var j = 0; j < arr.length; j++) out.push(arr[j].file)
        return out
    }
    function slotOfFile(f) {
        for (var i = 0; i < mem.slotFiles.length; i++) if (mem.slotFiles[i] === f) return i
        return -1
    }
    // Sticky assignment. Files that left the rail free their slot; newcomers take
    // (1) the slot the stage swap asked for, (2) a slot just freed, (3) a free live
    // slot when they're playing now / a free overflow slot when they aren't, (4) any
    // free slot. Then one rebalance pass: a live track stuck in an overflow slot swaps
    // with a resting track holding a live slot (tracks only change rank at their own
    // start/end, so this fires at most a few times per replay).
    function assignSlots() {
        if (mem._railFrozen) return
        var order = mem.railOrderedFiles()
        var want = {}
        for (var i = 0; i < order.length; i++) want[order[i]] = true
        var slots = mem.slotFiles.slice()
        var freed = []
        for (var k = 0; k < slots.length; k++)
            if (slots[k].length > 0 && !want[slots[k]]) { freed.push(k); slots[k] = "" }
        var placed = {}
        for (var m = 0; m < slots.length; m++) if (slots[m].length > 0) placed[slots[m]] = m
        var hint = mem._slotHint || {}
        var pending = []
        for (var n = 0; n < order.length; n++) {
            var f = order[n]
            if (placed[f] !== undefined) continue
            if (hint[f] !== undefined && slots[hint[f]] === "") { slots[hint[f]] = f; placed[f] = hint[f] }
            else pending.push(f)
        }
        mem._slotHint = ({})
        function pick(live) {
            var q
            for (q = 0; q < freed.length; q++) { var fs = freed[q]; if (slots[fs] === "" && ((fs < 5) === live)) return fs }
            for (q = 0; q < slots.length; q++) if (slots[q] === "" && ((q < 5) === live)) return q
            for (q = 0; q < freed.length; q++) if (slots[freed[q]] === "") return freed[q]
            for (q = 0; q < slots.length; q++) if (slots[q] === "") return q
            return -1
        }
        for (var p = 0; p < pending.length; p++) {
            var t = mem.trackByFile(pending[p])
            var s = pick(mem.railRank(t) === 0)
            if (s >= 0) slots[s] = pending[p]
        }
        // rebalance: an active file in an overflow slot takes an EMPTY live slot, else
        // swaps with a resting file holding one
        for (var a = 5; a < slots.length; a++) {
            if (slots[a] === "" || mem.railRank(mem.trackByFile(slots[a])) !== 0) continue
            var moved = false
            for (var e = 0; e < 5; e++) if (slots[e] === "") { slots[e] = slots[a]; slots[a] = ""; moved = true; break }
            if (moved) continue
            for (var b = 0; b < 5; b++) {
                if (slots[b] !== "" && mem.railRank(mem.trackByFile(slots[b])) !== 0) {
                    var tmp = slots[a]; slots[a] = slots[b]; slots[b] = tmp
                    break
                }
            }
        }
        var changed = false
        for (var c = 0; c < slots.length; c++) if (slots[c] !== mem.slotFiles[c]) { changed = true; break }
        if (changed) mem.slotFiles = slots
        mem.railOrder = order
    }
    onSideTracksChanged: mem.assignSlots()
    function resetSlots() {
        mem.slotFiles = ["", "", "", "", "", "", "", ""]
        mem.railOrder = []
        mem._railSig = ""
        mem._slotHint = ({})
        mem._railFrozen = false
    }

    // ---- points of view, grouped by person ----
    // [{ user, name, povs: [{ file, kind, label, off, end, key }] }], people in order
    // of first appearance; within a person: camera(s) first, then streams by start.
    // Labels are plain words — "Camera", "Stream", "Stream 2" — instead of a glyph and
    // a name repeated N times. `key` is the 1-9 keyboard slot.
    readonly property var povGroups: {
        var groups = [], byUser = {}
        var vts = mem.videoTracks.slice()
        vts.sort(function(a, b) { return Number(a.start_ms) - Number(b.start_ms) || (a.file < b.file ? -1 : 1) })
        for (var i = 0; i < vts.length; i++) {
            var u = mem.userIdOf(vts[i].identity)
            if (byUser[u] === undefined) { byUser[u] = groups.length; groups.push({ user: u, name: mem.shortName(vts[i].identity), cams: [], shares: [] }) }
            (vts[i].kind === "cam" ? groups[byUser[u]].cams : groups[byUser[u]].shares).push(vts[i])
        }
        var key = 1
        for (var g = 0; g < groups.length; g++) {
            var povs = []
            for (var c = 0; c < groups[g].cams.length; c++)
                povs.push({ file: groups[g].cams[c].file, kind: "cam", off: mem.trackOffset(groups[g].cams[c]), end: mem.trackEnd(groups[g].cams[c]),
                            label: "Camera" + (groups[g].cams.length > 1 ? " " + (c + 1) : ""), key: key++ })
            for (var sh = 0; sh < groups[g].shares.length; sh++)
                povs.push({ file: groups[g].shares[sh].file, kind: "share", off: mem.trackOffset(groups[g].shares[sh]), end: mem.trackEnd(groups[g].shares[sh]),
                            label: "Stream" + (groups[g].shares.length > 1 ? " " + (sh + 1) : ""), key: key++ })
            groups[g].povs = povs
        }
        return groups
    }
    function povByKey(k) {
        for (var g = 0; g < mem.povGroups.length; g++)
            for (var p = 0; p < mem.povGroups[g].povs.length; p++)
                if (mem.povGroups[g].povs[p].key === k) return mem.povGroups[g].povs[p]
        return null
    }
    function povDescr(t) { // "alex's camera" / "alex's stream 2"
        if (!t) return ""
        for (var g = 0; g < mem.povGroups.length; g++)
            for (var p = 0; p < mem.povGroups[g].povs.length; p++)
                if (mem.povGroups[g].povs[p].file === t.file)
                    return mem.povGroups[g].name + "'s " + mem.povGroups[g].povs[p].label.toLowerCase()
        return mem.shortName(t.identity) + "'s " + mem.kindWord(t.kind)
    }
    readonly property int hiddenCount: { var n = 0; for (var k in mem.hiddenFiles) if (mem.hiddenFiles[k]) n++; return n }
    function showAll() { mem.hiddenFiles = ({}) }

    readonly property real durSecs: {
        if (!sel) return 0
        if (sel.end_ms && sel.start_ms) return Math.max(1, (sel.end_ms - sel.start_ms) / 1000)
        return Math.max(1, mem.audioDur + mem.audioOffset)
    }
    readonly property string memUrl: sel ? ("tinhat-memory://" + mem.roomId + "/" + sel.id) : ""
    // inCall is REQUIRED (the spec: "only synced to users IN a call") — without it a
    // stale party left in room state after a group replay resurrects on app start
    // and silently drives the player of someone just browsing memories.
    readonly property bool synced: mem.inCall && mem.watchInParty && mem.sel !== null
        && String((mem.watchPlayback && mem.watchPlayback.url) || "") === mem.memUrl
    readonly property var sessionClips: {
        if (!sel) return []
        var out = []
        var s = Number(sel.start_ms)
        var e = sel.end_ms ? Number(sel.end_ms) : (s + mem.durSecs * 1000)
        for (var i = 0; i < mem.clips.length; i++) {
            var c = mem.clips[i]
            if (c.at >= s && c.at <= e) out.push({ at: c.at, by: c.by, off: (c.at - s) / 1000 })
        }
        out.sort(function(a, b) { return a.off - b.off })
        return out
    }
    readonly property string selRoomName: mem.sel ? String(mem.sel.room_name || mem.roomName || "") : ""

    function fmtTime(s) {
        s = Math.max(0, Math.floor(s))
        var m = Math.floor(s / 60), sec = ("0" + (s % 60)).slice(-2)
        if (m >= 60) return Math.floor(m / 60) + ":" + ("0" + (m % 60)).slice(-2) + ":" + sec
        return m + ":" + sec
    }
    function fmtDate(ms) { return new Date(Number(ms)).toLocaleString(Qt.locale(), "ddd MMM d · h:mm AP") }
    function fmtDateLong(ms) { return new Date(Number(ms)).toLocaleString(Qt.locale(), "dddd, MMMM d · h:mm AP") }
    function fmtBytes(b) {
        b = Number(b || 0)
        if (b >= 1e9) return (b / 1e9).toFixed(b >= 1e10 ? 0 : 1) + " GB"
        if (b >= 1e6) return Math.round(b / 1e6) + " MB"
        return b > 0 ? Math.max(1, Math.round(b / 1e3)) + " KB" : ""
    }
    function shortName(identity) {
        var m = /^(@[^:]+)/.exec(identity || "")
        return m ? m[1].slice(1) : (identity || "?")
    }
    function userIdOf(identity) {
        var m = /^(@[^:]+:[^:]+)/.exec(identity || "")
        return m ? m[1] : String(identity || "")
    }
    // A list from the vods JSON reaches delegates as a Qt variant list, for which
    // Array.isArray is FALSE (the G1 gate: no posters/people/counts on any card, and
    // the ec1ff29 open path silently lost the audio offset on a card click). Duck-type.
    function isList(x) { return x !== null && x !== undefined && typeof x.length === "number" }
    function kindGlyph(kind) { return kind === "share" ? "🖥" : (kind === "cam" ? "📷" : "🎙") }
    function kindWord(kind) { return kind === "share" ? "stream" : (kind === "cam" ? "camera" : "voice") }
    // Unique people behind a session (server field since 0.8.15; derived for older payloads)
    function peopleOf(s) {
        var out = []
        if (s && mem.isList(s.people)) {
            for (var p = 0; p < s.people.length; p++) out.push(String(s.people[p]))
            return out
        }
        var ts = (s && mem.isList(s.tracks)) ? s.tracks : []
        for (var i = 0; i < ts.length; i++) {
            var u = mem.userIdOf(ts[i].identity)
            if (u.length > 0 && out.indexOf(u) < 0) out.push(u)
        }
        return out
    }
    function countKinds(s) {
        var a = 0, sh = 0, c = 0
        var ts = (s && mem.isList(s.tracks)) ? s.tracks : []
        for (var i = 0; i < ts.length; i++) {
            if (ts[i].kind === "audio") a++
            else if (ts[i].kind === "share") sh++
            else c++
        }
        return { voices: a, streams: sh, cams: c }
    }
    // A representative frame for the card: the earliest video POV a few seconds in.
    function posterUrl(s) {
        if (!s || !mem.isList(s.tracks) || !s.end_ms) return ""
        var best = null
        for (var i = 0; i < s.tracks.length; i++) {
            var t = s.tracks[i]
            if (t.kind !== "share" && t.kind !== "cam") continue
            if (!best || Number(t.start_ms) < Number(best.start_ms)
                || (Math.abs(Number(t.start_ms) - Number(best.start_ms)) <= 500 && t.kind === "share" && best.kind !== "share"))
                best = t
        }
        if (!best || !best.url) return ""
        var dur = Math.max(1, (Number(s.end_ms) - Number(s.start_ms)) / 1000)
        var off = Math.max(0, (Number(best.start_ms) - Number(s.start_ms)) / 1000)
        var t0 = Math.floor(Math.min(dur - 1, Math.max(off + 1, Math.min(off + 8, dur / 3))))
        return String(best.url).replace("/api/vods/file/", "/api/vods/thumb/") + "&t=" + Math.max(0, t0 - Math.floor(off))
    }
    function showToast(t) { mem.toast = t; toastTimer.restart() }
    Timer { id: toastTimer; interval: 3200; onTriggered: mem.toast = "" }

    // ---- the audible master: focused video + external mixed audio in ONE mpv ----
    // audio-delay maps content: at video pts t you hear audio pts t - delay; we want
    // audio pts t + vidOff - audOff there, so delay = audOff - vidOff.
    function applyMain() {
        if (!mem.mainReady) return
        var vurl = mem.focusedTrack ? mem.focusedTrack.url : ""
        var sig = vurl + "|" + mem.audioUrl
        if (sig === mem.mMainLoaded) return
        mem.mMainLoaded = sig
        if (vurl.length > 0) {
            // Options go in as PROPERTIES before a plain 2-arg loadfile: mpv >= 0.38
            // moved loadfile's options to the 4th positional slot, so the 3-arg
            // "url replace opts" form fails with MPV_ERROR_INVALID_PARAMETER (the
            // first relive gate: four dead loads, black surfaces).
            // COMBINED files are the lipsync path: the recorder muxes each POV's
            // video with the aligned mixed audio at session close (stream copy), so
            // playback is ONE ordinary file — mpv-native A/V sync, and a plain
            // loadfile provably paints (every external-audio attach method blacked
            // the FBO while vid=1/vo-configured=true). Old sessions without a
            // combined file fall back to external attach (may show black video —
            // audio still plays; re-recording fixes it).
            var comb = (mem.focusedTrack && mem.focusedTrack.combined_url) ? mem.focusedTrack.combined_url : ""
            if (comb.length > 0) {
                mainMpv.setProperty("audio-delay", 0)
                mainMpv.command(["loadfile", comb, "replace"])
            } else {
                var delay = (mem.audioUrl.length > 0) ? (mem.audioOffset - mem.focusedOffset) : 0
                mainMpv.setProperty("audio-delay", delay)
                if (mem.audioUrl.length > 0)
                    mainMpv.command(["loadfile", vurl, "replace", "-1", "audio-files=" + mem.audioUrl])
                else
                    mainMpv.command(["loadfile", vurl, "replace"])
            }
            mem.mAudioAttached = comb.length > 0 // fallback verify only for non-combined
            // P4: a POV that hasn't started yet parks at its first frame, muted by
            // pause, until the timeline reaches it.
            mainMpv.setProperty("pause", !mem.playing || mem.focusWaiting)
            mem.loadPending = true
            mem._loadTicks = 0
            mem.loadTarget = (comb.length > 0) ? comb : vurl
        } else {
            mainMpv.command(["stop"])
            mem.loadPending = false
            mem.loadTarget = ""
        }
        mem.applyAud() // audio-only fallback surface follows focus state
    }
    // Audio-only surface: active when no video is focused OR the focused POV hasn't
    // started yet (P4) — otherwise the main mpv carries the audio (two audio clocks
    // would fight).
    function applyAud() {
        if (!mem.audReady) return
        var want = ((mem.focusedTrack === null || mem.focusWaiting) && mem.sel !== null) ? mem.audioUrl : ""
        if (want === mem.mAudLoaded) return
        mem.mAudLoaded = want
        if (want.length > 0) {
            audioMpv.command(["loadfile", want, "replace"])
            audioMpv.setProperty("pause", !mem.playing)
        } else {
            audioMpv.command(["stop"])
        }
    }
    function applyVolume() {
        mainMpv.setProperty("volume", Math.round(mem.volume))
        audioMpv.setProperty("volume", Math.round(mem.volume))
        mainMpv.setProperty("mute", mem.muted ? "yes" : "no")
        audioMpv.setProperty("mute", mem.muted ? "yes" : "no")
    }
    onVolumeChanged: { if (mem.mainReady) mem.applyVolume(); memSaveTimer.restart() }
    onMutedChanged: { if (mem.mainReady) mem.applyVolume(); memSaveTimer.restart() }
    // One main-surface seek per landing: the hand-over and seekMaster both wanted to
    // seek to the same spot within a millisecond (two AO flushes back to back). Also
    // skip a seek that would land where the surface already is.
    property real _lastMainSeekPos: -1
    property double _lastMainSeekAt: 0
    function seekMain(pos) {
        var now = Date.now()
        if (Math.abs(pos - mem._lastMainSeekPos) < 0.05 && now - mem._lastMainSeekAt < 400) return
        var cur = mainMpv.getProperty("time-pos")
        if (cur !== undefined && cur !== null && !isNaN(Number(cur)) && Math.abs(Number(cur) - pos) < 0.05
            && mainMpv.getProperty("seeking") !== true && mem._lastMainSeekPos >= 0) return
        mem._lastMainSeekPos = pos; mem._lastMainSeekAt = now
        mainMpv.command(["seek", String(pos), "absolute+exact"])
    }
    // master clock: focused mpv when a video is focused (and started), else the audio surface
    function masterPos() {
        if (mem.loadPending) return -1   // mid-file-switch: hold the last clock value
        var useAudio = (mem.focusedTrack === null) || mem.focusWaiting
        var src = useAudio ? audioMpv : mainMpv
        var off = useAudio ? mem.audioOffset : mem.focusedOffset
        var t = src.getProperty("time-pos")
        if (t === undefined || t === null || isNaN(Number(t))) return -1
        return Number(t) + off
    }
    // P4 watchdog: the wait rides the audio surface's clock. If that clock does not
    // move for ~3s while we're meant to be playing (no audio device, mix shorter than
    // the session), don't strand the user in the veil — skip to the POV's start.
    property real _waitLastG: -1
    property int _waitStall: 0
    function waitWatchdog() {
        if (!mem.focusWaiting || !mem.playing || mem.audioLoadPending || mem.loadPending) { mem._waitStall = 0; mem._waitLastG = -1; return }
        if (Math.abs(mem.globalPos - mem._waitLastG) < 0.01) mem._waitStall++
        else mem._waitStall = 0
        mem._waitLastG = mem.globalPos
        if (mem._waitStall >= 12) {
            mem._waitStall = 0
            console.log("[memories] wait clock stalled -> skipping to " + mem.focusedOffset.toFixed(1))
            mem.showToast("Skipped ahead to " + mem.fmtTime(mem.focusedOffset))
            mem.commitSeek(mem.focusedOffset)
        }
    }
    // P4 hand-over: the timeline reached (or a seek crossed) the focused POV's start.
    onFocusWaitingChanged: {
        if (mem.sel === null || mem.focusedTrack === null) return
        if (mem.focusWaiting) {
            mainMpv.setProperty("pause", true)
            // the audio surface takes the clock over from here: land it at NOW once
            // its (re)load reports a duration, never at 0:00
            mem.audioPendingSeek = mem.globalPos
            mem.audioLoadPending = true
            mem.applyAud()
            return
        }
        var g = mem.globalPos
        mem.applyAud()                      // stops the audio surface — the video owns audio now
        if (!mem.loadPending) {
            var d = mainMpv.getProperty("duration")
            if (d !== undefined && d !== null && Number(d) > 0)
                mem.seekMain(Math.max(0, g - mem.focusedOffset))
            mainMpv.setProperty("pause", !mem.playing)
        }
        mem.sideResync(true)
    }

    // P8: don't open a session that has nothing to play yet. LIVE = the call is
    // still recording; !ready = post-close processing (mixed audio + per-POV lipsync
    // files) isn't finished. Both flip automatically; the list auto-refreshes.
    function tryOpen(s) {
        if (!s.end_ms) {
            mem.listNoticeKind = "live"
            mem.listNotice = "This call is still being recorded — come back after it ends to relive it."
            return
        }
        if (s.empty) {
            mem.listNoticeKind = "warn"
            mem.listNotice = "Nothing was captured in this session (nobody had a stream or mic on)."
            return
        }
        if (s.ready === false) {
            mem.listNoticeKind = "warn"
            var mins = s.end_ms ? Math.round((Date.now() - s.end_ms) / 60000) : 0
            // 10.1 (TH-MEM-04): after ten minutes it is not "a moment" any more.
            mem.listNotice = mins > 10
                ? "This memory has been processing for " + mins + " minutes — the recorder probably failed on it. It stays listed, and the server's owner can check the recorder's log."
                : "This memory is still processing (building lipsync and the timeline). It'll be ready in a moment."
            return
        }
        mem.listNotice = ""
        mem.openSession(s)
    }
    function openSession(s) {
        mem.resetSlots()
        mem._railFrozen = true   // one assignment at the end of this function, with the real clock + focus
        mem.sel = s
        mem.playing = false
        mem.globalPos = 0
        mem.atEnd = false
        mem.userChoseAudioOnly = false
        mem.hiddenFiles = ({})
        mem.toast = ""
        // reset the genuinely-stale carry-overs (P8) so opening B never inherits A's
        // duration/audio state
        mem.audioDur = 0
        mem.mAudioAttached = false
        mem._loadTicks = 0
        mem.audioLoadPending = false; mem.audioPendingSeek = -1
        mem.pendingSeek = -1; mem.loadPending = false; mem.loadTarget = ""
        var aUrl = s.mixed_url || ""
        var minA = -1
        var firstAudioUrl = ""
        if (mem.isList(s.tracks)) for (var i = 0; i < s.tracks.length; i++) {
            var t = s.tracks[i]
            if (t.kind === "audio" && (minA < 0 || t.start_ms < minA)) { minA = t.start_ms; firstAudioUrl = t.url }
        }
        if (!aUrl) aUrl = firstAudioUrl
        mem.audioUrl = aUrl
        mem.audioOffset = (aUrl && minA >= 0 && s.start_ms) ? Math.max(0, (minA - s.start_ms) / 1000) : 0
        mem.focusedFile = mem.earliestVideoFile()   // P1: open at the beginning
        // …which is the moment the first video BEGAN: the recorder joins a few seconds
        // before anyone's stream lands, and opening at 0:00 would start every memory
        // in the "joins at 0:03" veil (the G2 gate) — nothing visible happens before.
        if (mem.focusedTrack && mem.focusedOffset > 0.3) mem.globalPos = mem.focusedOffset
        mem.mMainLoaded = ""; mem.mAudLoaded = ""
        console.log("[memories] open id=" + s.id + " vts=" + mem.videoTracks.length
            + " focused=" + (mem.focusedFile || "audio-only") + " at=" + mem.globalPos.toFixed(1)
            + " dur=" + mem.durSecs.toFixed(1))
        mem.applyMain()
        mem._railFrozen = false
        mem.assignSlots()
        // Resume where this session was left off earlier in this app run (never on
        // the first open — a memory always starts at the top, P1).
        var r = Number(mem.resumeAt[String(s.id)] || 0)
        if (r > 5 && r < mem.durSecs - 5) {
            if (mem.focusedTrack) mem.pendingSeek = r
            else { mem.audioPendingSeek = r; mem.audioLoadPending = true }
            mem.showToast("Resumed from " + mem.fmtTime(r))
        }
        mem.setPlaying(true)
    }
    function focusTrack(f) {
        if (mem.focusedFile === f) return
        var g = mem.globalPos
        console.log("[memories] focus -> " + (f || "audio-only") + " at g=" + g.toFixed(2))
        mem.atEnd = false
        mem.userChoseAudioOnly = (f === "")
        // The swap: whatever was on stage drops into the card the new one vacates.
        var hint = {}
        var vacated = mem.slotOfFile(f)
        if (mem.focusedFile.length > 0 && vacated >= 0) hint[mem.focusedFile] = vacated
        mem._slotHint = hint
        mem.focusedFile = f
        var nt = mem.trackByFile(f)
        mem.stageFlashText = nt ? ("▶ " + mem.povDescr(nt) + " on stage") : "🎧 voices only"
        stageFlash.restart()
        if (mem.focusedTrack && !mem.focusWaiting) {
            // land the new master at the moment we were reliving — QUEUED until the
            // new file reports loaded (seeking mid-load was the focus break)
            mem.pendingSeek = g
            mem.fineTrace = 60
            mem.applyMain()
        } else {
            // audio-only, or a POV that hasn't started yet (P4): the mixed audio
            // carries the clock. audioMpv has NO duration right after a loadfile — a
            // plain seekMaster(g) would be dropped and playback would restart at
            // 0:00. Latch it and let the tick land it once audioMpv
            // reports a duration. The video (if any) parks at its first frame.
            mem.audioPendingSeek = g
            mem.audioLoadPending = true
            mem.mAudLoaded = ""                                // force applyAud to reload
            mem.pendingSeek = -1
            mem.applyMain()
            if (mem.focusedTrack)
                mem.showToast(mem.kindGlyph(mem.focusedTrack.kind) + " " + mem.shortName(mem.focusedTrack.identity)
                    + " joins at " + mem.fmtTime(mem.focusedOffset) + " — the replay keeps going until then")
        }
    }
    function toggleHidden(f) {
        var h = {}
        for (var k in mem.hiddenFiles) h[k] = mem.hiddenFiles[k]
        if (h[f]) delete h[f]; else h[f] = true
        mem.hiddenFiles = h
        if (h[f] && mem.focusedFile === f) {
            // hiding the focused track → focus the next visible one (or audio-only)
            var next = ""
            for (var i = 0; i < mem.videoTracks.length; i++) {
                var t = mem.videoTracks[i]
                if (t.file !== f && !h[t.file]) { next = t.file; break }
            }
            mem.focusTrack(next)
        }
    }
    function seekMaster(g) {
        g = Math.max(0, Math.min(g, mem.durSecs))
        mem.globalPos = g
        if (mem.atEnd && g < mem.durSecs - 1) mem.playing = true   // scrub back from the end = play again
        mem.atEnd = false
        if (mem.loadPending) {
            mem.pendingSeek = g   // mid-file-switch: the load-resolution tick lands it
            return
        }
        if (mem.focusedTrack && !mem.focusWaiting) {
            var d = mainMpv.getProperty("duration")
            if (d !== undefined && d !== null && Number(d) > 0)
                mem.seekMain(Math.max(0, g - mem.focusedOffset))
        } else {
            var ad = audioMpv.getProperty("duration")
            if (ad !== undefined && ad !== null && Number(ad) > 0)
                audioMpv.command(["seek", String(Math.max(0, g - mem.audioOffset)), "absolute+exact"])
            else {
                // surface not ready (just re-loaded by the wait hand-back): land it once
                // it reports a duration (P3 latch) — never let it start from 0:00
                mem.audioPendingSeek = g
                mem.audioLoadPending = true
            }
            if (mem.focusedTrack) {
                // P4: hold the not-yet-started video at its first frame
                mainMpv.command(["seek", "0", "absolute+exact"])
                mainMpv.setProperty("pause", true)
            }
        }
        // keep-open leaves mpv PAUSED after an EOF; a scrub back must resume if we
        // were playing (field report: "could not scrub back without closing and reopening").
        Qt.callLater(function() { mem.setPlaying(mem.playing) })
    }
    function setGlobalPos(g) { mem.seekMaster(g) }        // remote-seek entry point
    function applyRemoteSeek(posSecs) {
        if (!mem.synced) return   // an unsynced player NEVER obeys a remote clock
        var p = Number(posSecs)
        if (isFinite(p) && p >= 0) {
            mem.seekMaster(p)
            mem.sideResync(true)
        }
    }
    function setPlaying(on) {
        if (on !== mem.playing) console.log("[memories] " + (on ? "play" : "pause") + " at g=" + mem.globalPos.toFixed(2))
        mem.playing = on
        mainMpv.setProperty("pause", !on || mem.focusWaiting)
        audioMpv.setProperty("pause", !on)
    }
    function togglePlay() {
        if (mem.synced) { if (mem.playing) mem.syncPause(); else mem.syncPlay(); return }
        if (mem.atEnd) {
            mem.atEnd = false
            if (mem.globalPos >= mem.durSecs - 1.5) mem.seekMaster(0)
            mem.setPlaying(true)
            return
        }
        mem.setPlaying(!mem.playing)
    }
    function skipBy(d) {
        if (mem.sel === null) return
        mem.commitSeek(Math.max(0, Math.min(mem.durSecs, mem.globalPos + d)))
        mem.showToast((d < 0 ? "⏪ " : "⏩ ") + mem.fmtTime(mem.globalPos))
    }
    function setTheater(on) { mem.visibility = on ? Window.FullScreen : Window.Windowed }
    property double _lastSyncSeekAt: 0
    property double _lastSideSyncAt: 0
    function userSeek(g) {
        // LOCAL preview always (mid-drag included); the SHARED seek (a Matrix state
        // write) at most every 1.2s — every drag tick used to write state until
        // Synapse 429'd the user and the party clock went stale. Side tiles
        // ride a gentler 800ms throttle (a flush seek per tile per drag tick would
        // storm the proxy with HTTPS range reads); commitSeek lands the release.
        mem.seekMaster(g)
        var now = Date.now()
        if (mem.synced && now - mem._lastSyncSeekAt > 1200) {
            mem._lastSyncSeekAt = now
            mem.syncSeek(String(g))
        }
        // 10.1 (TH-MEM-17): no hard side-tile resync mid-drag (a flush+accurate seek per tile per
        // 800 ms stormed the proxy over the WAN); commitSeek lands ONE on release.
    }
    // Final commit when the drag ends — the throttles above may have swallowed the
    // last position, and the release is the position the user actually meant.
    function commitSeek(g) {
        console.log("[memories] seek -> " + Number(g).toFixed(2) + " (from " + mem.globalPos.toFixed(2) + (mem.focusWaiting ? ", waiting" : "") + ")")
        mem.fineTrace = 60   // 15 s of 250 ms samples after every landing
        if (mem.synced) {
            mem._lastSyncSeekAt = Date.now()
            mem.syncSeek(String(g))
        }
        mem.seekMaster(g)
        mem.sideResync(true)
    }
    function backToList() {
        if (mem.sel !== null && mem.globalPos > 5 && mem.globalPos < mem.durSecs - 5) {
            var r = {}
            for (var k in mem.resumeAt) r[k] = mem.resumeAt[k]
            r[String(mem.sel.id)] = mem.globalPos
            mem.resumeAt = r
        }
        if (mem.synced) mem.syncStop()
        mainMpv.command(["stop"]); audioMpv.command(["stop"])
        mem.sel = null; mem.focusedFile = ""; mem.playing = false; mem.globalPos = 0
        mem.mMainLoaded = ""; mem.mAudLoaded = ""; mem.hiddenFiles = ({})
        mem.loadPending = false; mem.loadTarget = ""; mem.pendingSeek = -1
        mem.atEnd = false; mem.userChoseAudioOnly = false; mem.audioDur = 0
        mem.audioLoadPending = false; mem.audioPendingSeek = -1; mem.mAudioAttached = false
        mem.toast = ""
        mem.resetSlots()
        if (mem.theater) mem.setTheater(false)
        if (mem.vodctl) mem.vodctl.memtileClear()   // tiles also self-destroy via track->null
    }
    function openOrRaise() {
        if (mem.visible) { mem.raise(); mem.requestActivate() }
        else { mem.show(); mem.raise(); mem.requestActivate() }
    }
    onClosing: mem.backToList()
    // Y3: the call ended (or we left it) while synced — stop following the shared
    // clock; a synced replay is defined as in-call only.
    // 11.4: only a SYNCED REPLAY (a tinhat-memory:// clock) ends with the call — a YouTube / music Media Party has nothing
    // to do with the call and keeps going (web docs/native-bugs/media.md: "a call ending leaves ANY joined watch party").
    onInCallChanged: if (!mem.inCall && mem.watchInParty && String((mem.watchPlayback && mem.watchPlayback.url) || "").indexOf("tinhat-memory://") === 0) mem.syncStop()
    // The app switched rooms under us: the list is another room's now.
    onRoomIdChanged: if (mem.sel !== null) mem.backToList()

    width: Kirigami.Units.gridUnit * 48
    height: Kirigami.Units.gridUnit * 40
    minimumWidth: 480
    minimumHeight: 420
    color: Th.memoriesBg
    title: mem.sel ? (mem.fmtDate(mem.sel.start_ms) + (mem.selRoomName.length > 0 ? " · " + mem.selRoomName : "") + " · Memories — Tin Hat")
                   : "Memories 📼"
    flags: Qt.Window
    objectName: "memories"
    visible: false

    // ---- keyboard: the usual player keys ----
    Shortcut { sequence: "Esc"; enabled: mem.visible && (mem.sel !== null || mem.theater)
               onActivated: if (mem.theater) mem.setTheater(false); else mem.backToList() }
    Shortcut { sequence: "Space"; enabled: mem.visible && mem.sel !== null; onActivated: mem.togglePlay() }
    Shortcut { sequence: "K"; enabled: mem.visible && mem.sel !== null; onActivated: mem.togglePlay() }
    Shortcut { sequence: "Left"; enabled: mem.visible && mem.sel !== null; onActivated: mem.skipBy(-10) }
    Shortcut { sequence: "Right"; enabled: mem.visible && mem.sel !== null; onActivated: mem.skipBy(10) }
    Shortcut { sequence: "J"; enabled: mem.visible && mem.sel !== null; onActivated: mem.skipBy(-10) }
    Shortcut { sequence: "L"; enabled: mem.visible && mem.sel !== null; onActivated: mem.skipBy(10) }
    Shortcut { sequence: "Shift+Left"; enabled: mem.visible && mem.sel !== null; onActivated: mem.skipBy(-60) }
    Shortcut { sequence: "Shift+Right"; enabled: mem.visible && mem.sel !== null; onActivated: mem.skipBy(60) }
    Shortcut { sequence: "M"; enabled: mem.visible && mem.sel !== null; onActivated: mem.muted = !mem.muted }
    Shortcut { sequence: "F"; enabled: mem.visible && mem.sel !== null; onActivated: mem.setTheater(!mem.theater) }
    // 1-9: put that point of view on stage (the number sits on each POV button)
    Item {
        visible: false
        Repeater {
            model: 9
            delegate: Item {
                required property int index
                Shortcut {
                    sequence: String(index + 1)
                    enabled: mem.visible && mem.sel !== null
                    onActivated: { var p = mem.povByKey(index + 1); if (p) { if (mem.hiddenFiles[p.file]) mem.toggleHidden(p.file); mem.focusTrack(p.file) } }
                }
            }
        }
    }
    Shortcut { sequence: "0"; enabled: mem.visible && mem.sel !== null && mem.audioUrl.length > 0; onActivated: mem.focusTrack("") }

    // A side tile: LIVE muted gst playback (streamer::memtile via vodctl), attached
    // by the SAME video_bridge machinery as live call-grid tiles — mpvqt reliably
    // paints only ONE surface per process, so extra mpvs were never an option
    // (0.8.10 shipped 2s ffmpeg thumbnails instead; field report: "the side car feeds are
    // 1fps"). The FOCUSED view still carries audio + exact seeks; side tiles ride
    // muted ACCURATE syncs off the master clock. If the pipeline can't be built on
    // this machine (missing gst plugin), the thumbnail path returns as a per-tile
    // FALLBACK. Click a tile to put it on stage — it becomes the audible one and the
    // previous stage occupant takes this card.
    // 0.8.16: a tile owns a SLOT (its pipeline) for as long as its track is in the
    // rail; its `rank` in mem.railOrder is only where it is drawn. Slots >= 5 are
    // OVERFLOW tiles: thumbnails only (the live-pipeline pool is capped at 5 — D12).
    component SideTile: Item {
        id: st
        property int slot: 0
        readonly property string file: (mem.slotFiles && mem.slotFiles.length > slot) ? String(mem.slotFiles[slot] || "") : ""
        readonly property var track: st.file.length > 0 ? mem.trackByFile(st.file) : null
        readonly property int rank: st.file.length > 0 ? mem.railOrder.indexOf(st.file) : -1
        readonly property bool liveAllowed: slot < 5
        readonly property string tileId: "memtile" + slot
        property bool created: false
        property bool attached: false
        property string createdFor: ""
        visible: track !== null && rank >= 0
        width: parent ? parent.width : 200
        height: Math.round(width * 9 / 16)
        y: Math.max(0, rank) * (height + mem.railGap)
        Behavior on y { NumberAnimation { duration: mem.theme.durSlow; easing.type: Easing.OutCubic } }
        readonly property string personName: st.track ? mem.shortName(st.track.identity) : ""
        readonly property string povLabel: {
            if (!st.track) return ""
            for (var g = 0; g < mem.povGroups.length; g++)
                for (var p = 0; p < mem.povGroups[g].povs.length; p++)
                    if (mem.povGroups[g].povs[p].file === st.file) return mem.povGroups[g].povs[p].label
            return mem.kindWord(st.track.kind)
        }
        property real tileOffset: (st.track && mem.sel && mem.sel.start_ms)
            ? Math.max(0, (st.track.start_ms - mem.sel.start_ms) / 1000) : 0
        readonly property real tileEnd: mem.trackEnd(st.track)
        // joins-at / left-at awareness: outside the track's own span the tile shows a
        // label instead of a misleading frozen frame, and its pipeline rests.
        readonly property bool notStarted: st.track !== null && mem.globalPos < st.tileOffset - 0.5
        readonly property bool ended: st.tileEnd > 0 && mem.globalPos > st.tileEnd + 0.5
        readonly property bool resting: st.notStarted || st.ended
        function tilePos() { return Math.max(0, Math.min(mem.globalPos, st.tileEnd > 0 ? st.tileEnd : mem.globalPos) - st.tileOffset) }
        function ensurePipeline() {
            if (!mem.vodctl) return
            var f = st.track ? String(st.track.url) : ""
            if (f === st.createdFor) return
            if (st.createdFor.length > 0) {
                mem.vodctl.memtileDestroy(st.tileId)
                st.created = false
                st.attached = false
            }
            st.createdFor = f
            st.settled = false
            if (f.length > 0) {
                // Skip the live pipeline entirely on a machine whose decoder probe
                // already came back broken (verdict 2) — go straight to the thumbnail
                // fallback instead of building a doomed pipeline (T2/T3). Overflow
                // slots never get a pipeline.
                if (mem.decoderBroken || !st.liveAllowed) { st.created = false; st.attached = false; return }
                st.created = mem.vodctl.memtileCreate(st.tileId, f)
                st.attached = false
                // position + transport land in the first-frame poll below.
            }
        }
        onTrackChanged: ensurePipeline()
        Component.onCompleted: ensurePipeline()
        Component.onDestruction: if (st.createdFor.length > 0 && mem.vodctl) mem.vodctl.memtileDestroy(st.tileId)
        // The native video surface is created INSIDE this container by the attach
        // shim (video_bridge finds it by objectName across top-level windows).
        // ---- settling: the surface is veiled until it has painted at the master
        // clock. A fresh pipeline prerolls at ITS FILE'S START before the first sync
        // lands; a hard seek flushes and re-prerolls — either way the frames before
        // that are the wrong moment (the field's "green garbage" tile was one of
        // those, still on screen). frames-since-sync is the honest signal.
        property bool settled: false
        property int syncFrames: 0
        property int settleTicks: 0
        function hardSync() {
            if (!(st.created && st.attached)) return
            st.syncFrames = mem.vodctl.memtileFrames(st.tileId)
            st.settled = false
            st.settleTicks = 0
            mem.vodctl.memtileSync(st.tileId, String(st.tilePos()), true)
            mem.vodctl.memtileSetPaused(st.tileId, !mem.playing || st.resting)
        }
        Timer {
            interval: Motion.focused ? 150 : 1000; repeat: true   // 11.0 (GPU.md §2.8): ≥ 1 s while not focused
            running: st.created && st.attached && st.firstSynced && !st.settled && st.visible && mem.visible
            onTriggered: {
                st.settleTicks++
                var fr = mem.vodctl.memtileFrames(st.tileId)
                var need = st.syncFrames + (mem.playing && !st.resting ? 2 : 1)
                if (fr >= need || st.settleTicks > 24) st.settled = true   // 3.6s: never veil a working tile forever
            }
        }
        Timer { // attach poll (mirrors the live call grid's dance)
            interval: Motion.focused ? 150 : 1000; repeat: true   // 11.0 (GPU.md §2.8): ≥ 1 s while not focused
            running: st.created && !st.attached && st.visible && mem.visible
            onTriggered: if (mem.vodctl.memtileAttach(st.tileId)) {
                st.attached = true
                console.log("[memtile-qml] attached " + st.tileId)
            }
        }
        // P6: land the FIRST seek+pause only once a frame has actually rendered.
        // Doing it at attach raced the render-thread SetPlaying (sink not yet READY):
        // a state change on a widget-less sink erred and the seek could land on a
        // not-yet-playing pipeline. Frame count > 0 means the pipeline is truly up.
        property bool firstSynced: false
        onAttachedChanged: { if (!st.attached) { st.firstSynced = false; st.settled = false }; st.healthTicks = 0 }
        Timer {
            interval: Motion.focused ? 200 : 1000; repeat: true   // 11.0 (GPU.md §2.8): ≥ 1 s while not focused
            running: st.created && st.attached && !st.firstSynced && st.visible && mem.visible
            onTriggered: if (mem.vodctl.memtileFrames(st.tileId) > 0) {
                st.firstSynced = true
                st.hardSync()
            }
        }
        onRestingChanged: if (st.created && st.attached && st.firstSynced) st.hardSync()
        Connections { // transport landings: scrub commit / focus swap / remote seek
            target: mem
            enabled: st.created && st.attached && st.firstSynced
            function onSideResync(hard) {
                if (hard) st.hardSync()
                else {
                    mem.vodctl.memtileSync(st.tileId, String(st.tilePos()), false)
                    mem.vodctl.memtileSetPaused(st.tileId, !mem.playing || st.resting)
                }
            }
            function onPlayingChanged() {
                mem.vodctl.memtileSetPaused(st.tileId, !mem.playing || st.resting)
            }
        }
        Timer { // gentle 5s drift check (the Rust side no-ops under 0.75s drift)
            interval: 5000; repeat: true
            running: st.created && st.attached && st.firstSynced && mem.playing && !st.resting && mem.visible
            onTriggered: mem.vodctl.memtileSync(st.tileId, String(st.tilePos()), false)
        }
        property int healthTicks: 0
        Timer { // health poll: flip to the thumbnail fallback when the
            // pipeline is dead — an explicit bus ERROR (missing decoder), OR zero
            // frames rendered ~6s after attach while playing (the decoder-less
            // repro WEDGES in preroll with no error at all; frame progress is the
            // only honest signal). createdFor is KEPT so the same broken url is
            // not endlessly rebuilt.
            interval: 1500; repeat: true
            running: st.created && st.visible && mem.visible
            onTriggered: {
                var frames = mem.vodctl.memtileFrames(st.tileId)
                var starved = st.attached && mem.playing && !st.resting && frames === 0
                if (starved) st.healthTicks++
                else st.healthTicks = 0
                if (mem.vodctl.memtileDead(st.tileId) || st.healthTicks >= 4) {
                    console.log("[memtile-qml] " + st.tileId + " dead (frames=" + frames
                        + ") -> thumbnail fallback")
                    mem.vodctl.memtileDestroy(st.tileId)
                    st.created = false
                    st.attached = false
                    mem.decoderBroken = true   // T3: banner + stop building live tiles
                }
            }
        }
        // ---- thumbnail FALLBACK (live pipeline unavailable on this machine) ----
        property int thumbT: -1
        Timer {
            // Slot-staggered cadence, NO triggeredOnStart (an in-phase first-shot
            // burst across 5 tiles is exactly what spiked the proxy). First frame
            // lands after the per-slot offset; steady state is gentle. Resting tiles
            // hold their one frame.
            interval: 3000 + st.slot * 700; repeat: true; triggeredOnStart: false
            running: !st.created && st.visible && mem.visible && mem.sel !== null
            onTriggered: {
                var want = Math.max(0, Math.floor(st.tilePos()))
                if (want !== st.thumbT) st.thumbT = want
            }
        }

        // ======================= the card =======================
        Rectangle {
            id: tileCard
            anchors.fill: parent
            radius: mem.theme.radius
            color: Th.memoriesTileBg
            border.width: stHover.hovered ? 2 : 1
            border.color: stHover.hovered ? Th.memoriesTileBorderHover : Th.memoriesTileBorder
            clip: true
            Behavior on border.color { ColorAnimation { duration: mem.theme.dur } }
            // 0.8.30: the GL video item paints square over the rounded card; bite the corners
            // in the rail background above it (below the interaction layer + the border).
            CornerMask { radius: mem.theme.radius; color: Th.memoriesBg; z: 15 }
            Rectangle { anchors.fill: parent; z: 16; radius: mem.theme.radius; color: "transparent"; border.width: tileCard.border.width; border.color: tileCard.border.color }
            // Interaction layer ABOVE the video surface (0.8.18): the GStreamer GL item the
            // attach shim creates inside the container accepts pointer events itself (it
            // forwards them as GstNavigation), so a wheel or click over a live tile never
            // reached the card — the harness proved wheel over the library scrolls while
            // wheel over the rail did nothing. Everything interactive lives here now.
            Item {
                anchors.fill: parent
                z: 20
                HoverHandler { id: stHover; cursorShape: Qt.PointingHandCursor }
                TapHandler { onTapped: if (st.track) mem.focusTrack(st.track.file) }
                WheelHandler {
                    acceptedDevices: PointerDevice.Mouse | PointerDevice.TouchPad
                    onWheel: (event) => {
                        var step = event.pixelDelta.y !== 0 ? event.pixelDelta.y
                                                            : (event.angleDelta.y / 120) * tileCol.tileH * 0.6
                        var maxY = Math.max(0, railFlick.contentHeight - railFlick.height)
                        railFlick.contentY = Math.max(0, Math.min(maxY, railFlick.contentY - step))
                        event.accepted = true
                    }
                }
            }
            Controls.ToolTip.text: st.track ? ("Put " + mem.povDescr(st.track) + " on stage") : ""
            Controls.ToolTip.visible: stHover.hovered && st.track !== null
            Controls.ToolTip.delay: 700

            // live surface (the sink item is created inside this container)
            Item {
                objectName: st.tileId
                anchors.fill: parent
                visible: st.created
            }
            // snapshot fallback
            Image {
                id: thumbImg
                anchors.fill: parent
                visible: !st.created
                fillMode: Image.PreserveAspectFit
                cache: false
                asynchronous: true
                source: (!st.created && st.track && st.thumbT >= 0)
                    ? st.track.url.replace("/api/vods/file/", "/api/vods/thumb/") + "&t=" + st.thumbT
                    : ""
                opacity: status === Image.Ready ? 1 : 0
                Behavior on opacity { NumberAnimation { duration: mem.theme.durSlow } }
            }
            // not yet painted at the clock (live) / no snapshot yet: a quiet placeholder
            // (opaque until settled, then fades — the GL surface underneath is never layered)
            Rectangle {
                anchors.fill: parent
                readonly property bool needed: !st.resting && ((st.created && !st.settled) || (!st.created && thumbImg.status !== Image.Ready))
                visible: opacity > 0.01
                opacity: needed ? 1 : 0
                Behavior on opacity { NumberAnimation { duration: mem.theme.durSlow } }
                z: 2
                gradient: Gradient {
                    GradientStop { position: 0.0; color: Th.memoriesTilePlaceholderHighlight }
                    GradientStop { position: 1.0; color: Th.memoriesTilePlaceholderBg }
                }
                Controls.Label {
                    anchors.centerIn: parent
                    text: st.created ? "syncing…" : "loading…"
                    color: Th.memoriesTilePlaceholderFg
                    font.pointSize: Kirigami.Theme.smallFont.pointSize
                    opacity: 0.8
                }
            }
            // joins-at / left-at veil
            Rectangle {
                anchors.fill: parent
                visible: st.resting
                color: Th.memoriesTileRestOverlay
                z: 3
                ColumnLayout {
                    anchors.centerIn: parent
                    spacing: 2
                    Avatar {
                        Layout.alignment: Qt.AlignHCenter
                        app: mem.app
                        name: st.personName
                        seed: st.track ? mem.userIdOf(st.track.identity) : ""
                        size: Math.max(24, Math.min(40, Math.round(st.height * 0.24)))
                    }
                    Controls.Label {
                        textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                        Layout.alignment: Qt.AlignHCenter
                        text: st.notStarted ? "joins at " + mem.fmtTime(st.tileOffset) : "left at " + mem.fmtTime(st.tileEnd)
                        color: Th.memoriesTileRestFg
                        font.bold: true
                        font.pointSize: Kirigami.Theme.smallFont.pointSize
                    }
                }
            }
            // name strip (always): avatar · person · Camera/Stream
            Rectangle {
                anchors.left: parent.left; anchors.right: parent.right; anchors.top: parent.top
                height: Math.max(26, Math.round(st.height * 0.17))
                z: 4
                gradient: Gradient {
                    GradientStop { position: 0.0; color: Th.memoriesTileStripBg }
                    GradientStop { position: 1.0; color: Qt.alpha(Th.memoriesTileStripBg, 0) }   // fades to transparent
                }
                RowLayout {
                    anchors.fill: parent
                    anchors.leftMargin: 7; anchors.rightMargin: 7; anchors.topMargin: 4
                    spacing: 6
                    Avatar {
                        app: mem.app
                        name: st.personName
                        seed: st.track ? mem.userIdOf(st.track.identity) : ""
                        size: 18
                    }
                    Controls.Label {
                        textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                        text: st.personName
                        color: Th.memoriesTileNameFg
                        font.bold: true
                        font.pointSize: Kirigami.Theme.smallFont.pointSize
                        elide: Text.ElideRight
                        Layout.maximumWidth: st.width * 0.5
                    }
                    Controls.Label {
                        textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                        text: mem.kindGlyph(st.track ? st.track.kind : "") + (st.width >= 250 ? " " + st.povLabel : "")
                        color: Th.memoriesTileMetaFg
                        font.pointSize: Kirigami.Theme.smallFont.pointSize
                        elide: Text.ElideRight
                        Layout.fillWidth: true
                    }
                    Controls.Label { textFormat: Text.PlainText; // keyboard number
                        visible: st.povKey > 0
                        text: String(st.povKey)
                        color: Th.memoriesTileMetaFg
                        font.pointSize: Kirigami.Theme.smallFont.pointSize
                        opacity: 0.7
                    }
                }
            }
            // hover: the affordance — what a click does
            Rectangle {
                anchors.fill: parent
                z: 5
                color: Th.memoriesTileOverlayHover
                visible: stHover.hovered && st.track !== null
                Rectangle {
                    anchors.centerIn: parent
                    radius: height / 2
                    color: Th.memoriesTileStageButtonBg
                    border.width: 1
                    border.color: Th.memoriesTileStageButtonBorder
                    width: watchLbl.implicitWidth + 24
                    height: watchLbl.implicitHeight + 12
                    Controls.Label {
                        id: watchLbl
                        anchors.centerIn: parent
                        text: "▶  Put on stage"
                        color: Th.memoriesTileStageButtonFg
                        font.bold: true
                    }
                }
                Kirigami.Icon {
                    anchors.right: parent.right; anchors.bottom: parent.bottom; anchors.margins: 6
                    source: "view-fullscreen-symbolic"
                    isMask: true
                    color: Th.memoriesTileIcon
                    width: Kirigami.Units.iconSizes.small; height: width
                    opacity: 0.85
                }
            }
            // bottom-right tag: live motion vs snapshots
            Pill {
                anchors.right: parent.right; anchors.bottom: parent.bottom; anchors.margins: 6
                z: 4
                visible: st.track !== null && !st.resting && !stHover.hovered
                text: st.created ? "● live" : "snapshots"
                fg: st.created ? Th.memoriesTileTagLiveFg : Th.memoriesTileTagFg
                bg: Th.memoriesTileTagBg
            }
        }
        readonly property int povKey: {
            if (!st.track) return 0
            for (var g = 0; g < mem.povGroups.length; g++)
                for (var p = 0; p < mem.povGroups[g].povs.length; p++)
                    if (mem.povGroups[g].povs[p].file === st.file) return mem.povGroups[g].povs[p].key
            return 0
        }
    }

    // hidden audio-only surface
    MpvVideo {
        id: audioMpv
        width: 1; height: 1; opacity: 0.01
        z: -1
        onReady: {
            audioMpv.setProperty("ao", "pulse,pipewire,alsa")
            audioMpv.setProperty("force-seekable", "yes")
            audioMpv.setProperty("hr-seek", "yes")
            audioMpv.setProperty("keep-open", "yes")
            audioMpv.setProperty("volume", Math.round(mem.volume))
            audioMpv.setProperty("mute", mem.muted ? "yes" : "no")
            mem.audReady = true
            mem.applyAud()
        }
    }

    // clock + side-tile correction + sync feed + [memsync] instrumentation
    property int _tick: 0
    property int fineTrace: 0
    function surfaceSnap() {
        var f = function(x) { return (x !== undefined && x !== null && !isNaN(Number(x))) ? Number(x).toFixed(2) : "-" }
        return "main[tp=" + f(mainMpv.getProperty("time-pos")) + " apts=" + f(mainMpv.getProperty("audio-pts"))
            + " pause=" + mainMpv.getProperty("pause") + " cache=" + mainMpv.getProperty("paused-for-cache")
            + " speed=" + f(mainMpv.getProperty("speed")) + " hw=" + mainMpv.getProperty("hwdec-current")
            + " seeking=" + mainMpv.getProperty("seeking") + " dc=" + f(mainMpv.getProperty("demuxer-cache-duration")) + "]"
            + " aud[tp=" + f(audioMpv.getProperty("time-pos")) + " idle=" + audioMpv.getProperty("idle-active")
            + " pause=" + audioMpv.getProperty("pause") + "]"
    }
    Timer {
        interval: Motion.focused ? 250 : 1000; repeat: true   // 11.0 (GPU.md §2.8): ≥ 1 s while not focused
        running: mem.visible && mem.sel !== null
        onTriggered: {
            if (mem.fineTrace > 0) {
                mem.fineTrace--
                if (mem.memDebug) console.log("[memtrace] g=" + mem.globalPos.toFixed(2) + " wait=" + mem.focusWaiting + " lp=" + mem.loadPending + " " + mem.surfaceSnap())
            }
            // Load-latch resolution: the new file is up once mpv's path matches what
            // we asked for AND it reports a real duration — only then land the
            // queued seek + pause state (keep-open can leave a fresh load paused).
            if (mem.loadPending) {
                mem._loadTicks++
                var lpPath = mainMpv.getProperty("path")
                var lpDur = mainMpv.getProperty("duration")
                if (lpPath === mem.loadTarget && lpDur !== undefined && lpDur !== null && Number(lpDur) > 0) {
                    mem.loadPending = false
                    mem._loadTicks = 0
                    var ps = mem.pendingSeek
                    mem.pendingSeek = -1
                    if (ps >= 0) mem.seekMaster(ps)
                    mainMpv.setProperty("pause", !mem.playing || mem.focusWaiting)
                    mem.sideResync(true)
                } else if (mem._loadTicks > 40) {   // ~10s: load failed or wedged
                    console.log("[memories] load timeout for " + mem.loadTarget)
                    mem.loadPending = false
                    mem._loadTicks = 0
                    mem.pendingSeek = -1
                    mem.showToast("Couldn't load this stream — try another point of view")
                }
            }
            // P3: resolve the audio-only load latch once audioMpv has a duration.
            if (mem.audioLoadPending) {
                var adur = audioMpv.getProperty("duration")
                if (audioMpv.getProperty("idle-active") === true && !(mem.focusedTrack === null || mem.focusWaiting)) {
                    mem.audioLoadPending = false; mem.audioPendingSeek = -1   // surface stopped: nothing to land
                } else if (adur !== undefined && adur !== null && Number(adur) > 0) {
                    mem.audioLoadPending = false
                    var aps = mem.audioPendingSeek; mem.audioPendingSeek = -1
                    if (aps >= 0) mem.seekMaster(aps)
                    audioMpv.setProperty("pause", !mem.playing)
                }
            }
            var g = mem.masterPos()
            if (g >= 0) mem.globalPos = Math.min(g, mem.durSecs)   // P2: clamp — a bad/legacy
                                                                    // combined file can't pin us past the end
            // rail order follows track boundaries (join/leave) — a cheap signature
            // per tick, real work only when it changes
            if (mem.sel !== null) {
                var sig = ""
                var sts = mem.sideTracks
                for (var ri = 0; ri < sts.length; ri++) sig += mem.railRank(sts[ri])
                if (sig !== mem._railSig) { mem._railSig = sig; mem.assignSlots() }
            }
            mem.waitWatchdog()
            // P2: ended-state machine. keep-open pauses mpv at EOF; without this the
            // clock just froze at the end and looked broken (field: global=5622.08).
            if (mem.playing && !mem.loadPending && !mem.audioLoadPending) {
                var eofSrc = (mem.focusedTrack && !mem.focusWaiting) ? mainMpv : audioMpv
                if (eofSrc.getProperty("eof-reached") === true) {
                    mem.setPlaying(false)
                    mem.atEnd = true
                    mem.globalPos = mem.durSecs
                    console.log("[memories] replay ended at " + mem.globalPos.toFixed(1)
                        + (mem.focusedTrack ? " (focused=" + mem.focusedFile + ")" : " (audio-only)"))
                }
            }
            // P7: self-heal — never sit in unlabeled audio-only when video exists and
            // the user didn't ask for it (the recurring audio-only symptom).
            if (mem.sel !== null && mem.focusedFile === "" && !mem.userChoseAudioOnly
                && !mem.loadPending && mem.videoTracks.length > 0) {
                var heal = mem.earliestVideoFile()
                if (heal.length > 0) {
                    console.log("[memories] self-heal audio-only -> " + heal + " (vt=" + mem.videoTracks.length + ")")
                    mem.focusTrack(heal)
                }
            }
            // FALLBACK external-audio attach: only if the load-time audio-files
            // option didn't take (aid stays false once the file is truly up). A
            // runtime audio-add costs a brief VO blank, so it's last resort only.
            if (mem.focusedTrack && !mem.focusWaiting && mem.audioUrl.length > 0 && !mem.mAudioAttached) {
                var mtp = mainMpv.getProperty("time-pos")
                if (mtp !== undefined && mtp !== null && !isNaN(Number(mtp))) {
                    mem.mAudioAttached = true
                    var aidNow = mainMpv.getProperty("aid")
                    if (aidNow === false || aidNow === "no")
                        mainMpv.command(["audio-add", mem.audioUrl, "select"])
                    // VO revival nudge: attaching external audio blacks the video
                    // output (clock keeps running); a REAL absolute+exact reposition
                    // re-inits decode and brings the picture back (a 0-relative seek
                    // is a no-op — gate-proven). Seek to where we already are.
                    mainMpv.command(["seek", String(Math.max(0, Number(mtp))), "absolute+exact"])
                }
            }
            var ad = audioMpv.getProperty("duration")
            if (ad !== undefined && ad !== null && !isNaN(Number(ad)) && Number(ad) > 0)
                mem.audioDur = Number(ad)
            else {
                var md = mainMpv.getProperty("duration")
                if (mem.focusedTrack && md !== undefined && md !== null && Number(md) > 0)
                    mem.audioDur = Number(md) + mem.focusedOffset - mem.audioOffset
            }
            mem._tick++
            if (mem._tick % 20 === 0) {
                var av = mainMpv.getProperty("avsync")
                var aid = mainMpv.getProperty("aid")
                var atp = audioMpv.getProperty("time-pos"), aidle = audioMpv.getProperty("idle-active"), apause = audioMpv.getProperty("pause")
                var mapts = mainMpv.getProperty("audio-pts")
                if (mem.memDebug) console.log("[memsync] global=" + mem.globalPos.toFixed(2)
                    + " avsync=" + (av !== undefined && av !== null ? Number(av).toFixed(3) : "n/a")
                    + " apts=" + (mapts !== undefined && mapts !== null && !isNaN(Number(mapts)) ? Number(mapts).toFixed(2) : "n/a")
                    + " mpause=" + mainMpv.getProperty("pause")
                    + " aud[tp=" + (atp !== undefined && atp !== null && !isNaN(Number(atp)) ? Number(atp).toFixed(2) : "-")
                    + " idle=" + aidle + " pause=" + apause + " ldp=" + mem.audioLoadPending + "]"
                    + " aid=" + aid
                    + " vid=" + mainMpv.getProperty("vid")
                    + " voconf=" + mainMpv.getProperty("vo-configured")
                    + " dw=" + mainMpv.getProperty("dwidth")
                    + " hw=" + mainMpv.getProperty("hwdec-current") + " ao=" + mainMpv.getProperty("current-ao") + " synced=" + mem.synced
                    + " rail[h=" + Math.round(railFlick.height) + " ch=" + Math.round(railFlick.contentHeight) + " cy=" + Math.round(railFlick.contentY) + " tileH=" + tileCol.tileH + " n=" + mem.railOrder.length + " int=" + railFlick.interactive + "]" + " inCall=" + mem.inCall
                    + " vt=" + mem.videoTracks.length + " st=" + mem.sideTracks.length + " rail=" + mem.railOrder.length
                    + " lp=" + mem.loadPending + " wait=" + mem.focusWaiting
                    + " focused=" + (mem.focusedFile || "audio-only"))
            }
        }
    }
    // Sync-with-call follower
    Timer {
        interval: 1000; repeat: true
        running: mem.visible && mem.synced
        onTriggered: {
            var pb = mem.watchPlayback || {}
            // Y5: never chase a stale clock. A shared clock older than 60s is a
            // leftover from a finished replay — force pause, follow nothing.
            if ((Date.now() - Number(pb.updatedAt || 0)) > 60000) {
                if (mem.playing) mem.setPlaying(false)
                return
            }
            var target = Number(pb.positionSecs || 0)
            if (pb.playing) target += (Date.now() - Number(pb.updatedAt || Date.now())) / 1000
            if (isFinite(target) && Math.abs(target - mem.globalPos) > 2.0) {
                mem.seekMaster(Math.min(target, mem.durSecs))
                mem.sideResync(true)
            }
            var wantPlaying = !!pb.playing
            if (wantPlaying !== mem.playing) mem.setPlaying(wantPlaying)
        }
    }
    // report our clock while synced (string across the boundary)
    Timer {
        interval: 1000; repeat: true
        running: mem.visible && mem.synced && mem.playing
        onTriggered: mem.syncReport(mem.globalPos.toFixed(3))
    }

    // --tinhat-memopen=<sessionId|latest>: gate hook — auto-open a session once
    // the memories model lands (list cards have no stable click coordinates under
    // Xvfb, same reason --tinhat-memories exists for the window itself).
    Timer {
        id: memopen
        interval: 1500; repeat: true
        readonly property string want: {
            var a = Qt.application.arguments
            for (var i = 0; i < a.length; i++)
                if (String(a[i]).indexOf("--tinhat-memopen=") === 0)
                    return String(a[i]).slice("--tinhat-memopen=".length)
            return ""
        }
        running: want.length > 0 && mem.visible && mem.sel === null
                 && mem.memories && mem.memories.length > 0
        onTriggered: {
            var pick = null
            for (var i = 0; i < mem.memories.length; i++) {
                var s = mem.memories[i]
                if (memopen.want !== "latest") {
                    if (String(s.id) === memopen.want) { pick = s; break }
                } else if (!pick || Number(s.start_ms) > Number(pick.start_ms)) {
                    pick = s
                }
            }
            if (pick) {
                console.log("[memopen] opening session " + pick.id)
                mem.openSession(pick)
            }
        }
    }

    // --tinhat-memtest: scripted EOF/scrub/avsync self-test → [memtest] log lines.
    Timer {
        id: memtest
        property int step: 0
        property real preSwapG: 0
        interval: 6000; repeat: true
        running: Qt.application.arguments.indexOf("--tinhat-memtest") >= 0 && mem.visible && mem.sel !== null
        onTriggered: {
            step++
            if (step === 1) {
                console.log("[memtest] baseline global=" + mem.globalPos.toFixed(2) + " dur=" + mem.durSecs.toFixed(2))
                mem.userSeek(Math.max(0, mem.durSecs - 2))
                console.log("[memtest] seeked near end")
            } else if (step === 2) {
                console.log("[memtest] past-end global=" + mem.globalPos.toFixed(2)
                    + " eof=" + mainMpv.getProperty("eof-reached"))
                mem.userSeek(5)
                console.log("[memtest] scrubbed back to 5")
            } else if (step === 3) {
                var av = mainMpv.getProperty("avsync")
                // Verdict must come from mpv's OWN clock — the first gate passed on a
                // stale QML variable while every load had silently failed.
                var tp = mem.focusedTrack ? mainMpv.getProperty("time-pos") : audioMpv.getProperty("time-pos")
                var real = (tp !== undefined && tp !== null && !isNaN(Number(tp)))
                var g = real ? Number(tp) + (mem.focusedTrack ? mem.focusedOffset : mem.audioOffset) : -1
                console.log("[memtest] after-scrub mpvpos=" + (real ? g.toFixed(2) : "NONE")
                    + " avsync=" + (av !== undefined && av !== null ? Number(av).toFixed(3) : "n/a")
                    + " verdict=" + ((real && g > 3 && g < 40) ? "SCRUB-OK" : "SCRUB-BROKEN"))
            } else if (step === 4) {
                // Regression: swapping focus mid-play must land the new POV at
                // the same moment, with the load latch resolved and a live clock.
                if (mem.videoTracks.length > 1) {
                    memtest.preSwapG = mem.globalPos
                    var tgt = (mem.videoTracks[1].file === mem.focusedFile)
                        ? mem.videoTracks[0].file : mem.videoTracks[1].file
                    console.log("[memtest] focus swap -> " + tgt + " at g=" + mem.globalPos.toFixed(2))
                    mem.focusTrack(tgt)
                } else {
                    console.log("[memtest] focus swap SKIPPED (single video track)")
                    running = false
                }
            } else if (step === 5) {
                var tp2 = mainMpv.getProperty("time-pos")
                var real2 = (tp2 !== undefined && tp2 !== null && !isNaN(Number(tp2)))
                var g2 = real2 ? Number(tp2) + mem.focusedOffset : -1
                var av2 = mainMpv.getProperty("avsync")
                // The queued landing clamps to the new track's own start (a track
                // that began later than the swap moment plays from its beginning).
                var expLand = Math.max(memtest.preSwapG, mem.focusedOffset)
                var drift = real2 ? (g2 - expLand) : 999
                // P4: swapping to a POV that starts LATER than now must NOT move the
                // clock — the video parks at its first frame, the audio keeps the
                // timeline near where we were, and the stage says when it joins.
                var held = mem.focusWaiting && mem.globalPos < mem.focusedOffset - 0.3
                    && mem.globalPos > memtest.preSwapG - 1 && mem.globalPos < memtest.preSwapG + 12
                var verdict = !real2 || mem.loadPending ? "FOCUS-BROKEN"
                    : (mem.focusWaiting ? (held ? "FOCUS-WAIT-OK" : "FOCUS-WAIT-BROKEN")
                                        : ((drift > -1 && drift < 9) ? "FOCUS-OK" : "FOCUS-BROKEN"))
                console.log("[memtest] after-swap mpvpos=" + (real2 ? g2.toFixed(2) : "NONE")
                    + " global=" + mem.globalPos.toFixed(2) + " off=" + mem.focusedOffset.toFixed(2)
                    + " lp=" + mem.loadPending + " wait=" + mem.focusWaiting
                    + " avsync=" + (av2 !== undefined && av2 !== null ? Number(av2).toFixed(3) : "n/a")
                    + " verdict=" + verdict)
                running = false
            }
        }
    }

    // --tinhat-memtest2: the field-report sequence — focus a late POV (wait), "Skip to
    // it", scrub, switch back — logging BOTH mpv surfaces. The audio surface must be
    // idle whenever wait=false, and main's audio-pts must track its time-pos.
    Timer {
        id: memtest2
        property int step: 0
        property real expectG: 0
        interval: 6000; repeat: true
        running: Qt.application.arguments.indexOf("--tinhat-memtest2") >= 0 && mem.visible && mem.sel !== null
        function snap(tag) {
            var mtp = mainMpv.getProperty("time-pos"), apts = mainMpv.getProperty("audio-pts")
            var atp = audioMpv.getProperty("time-pos"), aidle = audioMpv.getProperty("idle-active"), apause = audioMpv.getProperty("pause")
            var f = function(x) { return (x !== undefined && x !== null && !isNaN(Number(x))) ? Number(x).toFixed(2) : "-" }
            var mainOk = mem.focusWaiting || (f(mtp) !== "-" && f(apts) !== "-" && Math.abs(Number(mtp) - Number(apts)) < 0.35)
            var audOk = mem.focusWaiting || mem.focusedTrack === null || aidle === true || apause === true
            console.log("[memtest2] " + tag + " global=" + mem.globalPos.toFixed(2) + " wait=" + mem.focusWaiting
                + " lp=" + mem.loadPending + " playing=" + mem.playing
                + " main[tp=" + f(mtp) + " apts=" + f(apts) + " pause=" + mainMpv.getProperty("pause") + " avsync=" + f(mainMpv.getProperty("avsync")) + "]"
                + " aud[tp=" + f(atp) + " idle=" + aidle + " pause=" + apause + " ldp=" + mem.audioLoadPending + " pend=" + mem.audioPendingSeek.toFixed(1) + "]"
                + " verdict=" + (mainOk && audOk ? "OK" : (!mainOk ? "MAIN-APTS-DRIFT" : "AUDIO-SURFACE-LEAK")))
        }
        onTriggered: {
            step++
            var late = null
            for (var i = 0; i < mem.videoTracks.length; i++)
                if (mem.trackOffset(mem.videoTracks[i]) > 600 && (!late || mem.trackOffset(mem.videoTracks[i]) > mem.trackOffset(late))) late = mem.videoTracks[i]
            if (step === 1) { snap("baseline"); if (!late) { console.log("[memtest2] no late track — stop"); running = false; return }
                              console.log("[memtest2] focus late " + late.file + " (off " + mem.trackOffset(late).toFixed(1) + ")"); mem.focusTrack(late.file) }
            else if (step === 2) { snap("waiting"); console.log("[memtest2] Skip to it"); mem.commitSeek(mem.focusedOffset) }
            else if (step === 3) { snap("after-skip") }
            else if (step === 4) { snap("after-skip+6s"); console.log("[memtest2] scrub +60"); mem.userSeek(mem.globalPos + 60); mem.commitSeek(mem.globalPos) }
            else if (step === 5) { snap("after-scrub"); console.log("[memtest2] scrub BACK across the start (off-30)"); memtest2.expectG = mem.focusedOffset - 30; mem.commitSeek(memtest2.expectG) }
            else if (step === 6) { snap("waiting-after-scrubback"); var d6 = mem.globalPos - memtest2.expectG
                                   console.log("[memtest2] scrubback-verdict=" + ((mem.focusWaiting && d6 > -2 && d6 < 12) ? "SCRUBBACK-OK" : "SCRUBBACK-BROKEN (global=" + mem.globalPos.toFixed(1) + " expected~" + memtest2.expectG.toFixed(1) + ")")) }
            else if (step === 7) { snap("waiting-after-scrubback+6s"); var back = mem.earliestVideoFile(); console.log("[memtest2] focus back " + back); mem.focusTrack(back) }
            else if (step === 8) { snap("after-back") }
            else if (step === 9) { snap("after-back+6s"); console.log("[memtest2] scrub -600 + focus late (pre-start)"); mem.commitSeek(Math.max(0, mem.globalPos - 600)); mem.focusTrack(late.file) }
            else if (step === 10) { snap("waiting-again") }
            else if (step === 11) { snap("waiting-again+6s"); running = false; console.log("[memtest2] done") }
        }
    }

    // ---- small themed pieces ----
    component Pill: Rectangle {
        property string text: ""
        property color fg: Th.memoriesPillFg
        property color bg: Th.memoriesPillBg
        property bool bold: false
        radius: height / 2
        color: bg
        implicitWidth: pillLbl.implicitWidth + 14
        implicitHeight: pillLbl.implicitHeight + 6
        Controls.Label {
            textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
            id: pillLbl
            anchors.centerIn: parent
            text: parent.text
            color: parent.fg
            font.bold: parent.bold
            font.pointSize: Kirigami.Theme.smallFont.pointSize
        }
    }
    // One point of view (a person's camera or stream). Plain label, explicit state
    // (on stage / hidden / joins at / left at), the keyboard number, and the hide
    // toggle only on hover — it is a secondary action.
    component PovButton: Rectangle {
        id: pb
        property var pov: null          // { file, kind, label, off, end, key }
        readonly property bool onStage: pov !== null && pov.file === mem.focusedFile
        readonly property bool hidden: pov !== null && !!mem.hiddenFiles[pov.file]
        readonly property bool upcoming: pov !== null && mem.globalPos < pov.off - 0.5
        readonly property bool gone: pov !== null && pov.end > 0 && mem.globalPos > pov.end + 0.5
        radius: height / 2
        color: onStage ? Th.memoriesPovBgActive : (pbHover.hovered ? Th.memoriesPovBgHover : Th.memoriesPovBg)
        border.width: 1
        border.color: onStage ? Th.memoriesPovBorderActive : Th.memoriesPovBorder
        opacity: hidden ? 0.5 : (gone ? 0.75 : 1)
        implicitWidth: pbRow.implicitWidth + 22
        implicitHeight: pbRow.implicitHeight + 10
        Behavior on color { ColorAnimation { duration: mem.theme.dur } }
        HoverHandler { id: pbHover; cursorShape: Qt.PointingHandCursor }
        TapHandler {
            onTapped: {
                if (!pb.pov) return
                if (pb.hidden) mem.toggleHidden(pb.pov.file)   // show…
                mem.focusTrack(pb.pov.file)                     // …and put on stage
            }
        }
        Controls.ToolTip.text: pb.pov
            ? ((pb.onStage ? "On stage now" : "Put this on stage (key " + pb.pov.key + ")")
               + (pb.upcoming ? " · joins at " + mem.fmtTime(pb.pov.off) : "")
               + (pb.gone ? " · left at " + mem.fmtTime(pb.pov.end) : "")
               + (pb.hidden ? " · hidden from the side rail" : ""))
            : ""
        Controls.ToolTip.visible: pbHover.hovered && pb.pov !== null
        Controls.ToolTip.delay: 500
        Row {
            id: pbRow
            anchors.centerIn: parent
            spacing: 6
            Controls.Label {
                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                anchors.verticalCenter: parent.verticalCenter
                text: pb.pov ? mem.kindGlyph(pb.pov.kind) : ""
                color: pb.onStage ? Th.memoriesPovMetaFgActive : Th.memoriesPovMetaFg
                font.pointSize: Kirigami.Theme.smallFont.pointSize
            }
            Controls.Label {
                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                anchors.verticalCenter: parent.verticalCenter
                text: pb.pov ? pb.pov.label : ""
                color: pb.onStage ? Th.memoriesPovFgActive : Th.memoriesPovFg
                font.bold: pb.onStage
                font.strikeout: pb.hidden
            }
            Controls.Label {
                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                anchors.verticalCenter: parent.verticalCenter
                visible: pb.onStage || pb.upcoming || pb.gone
                text: pb.onStage ? "· on stage" : (pb.upcoming ? "· joins " + mem.fmtTime(pb.pov.off) : "· left " + mem.fmtTime(pb.pov.end))
                color: pb.onStage ? Th.memoriesPovMetaFgActive : Th.memoriesPovMetaFg
                font.pointSize: Kirigami.Theme.smallFont.pointSize
                opacity: pb.onStage ? 0.9 : 1
            }
            Controls.Label { textFormat: Text.PlainText; // keyboard number
                anchors.verticalCenter: parent.verticalCenter
                text: pb.pov ? String(pb.pov.key) : ""
                color: pb.onStage ? Th.memoriesPovMetaFgActive : Th.memoriesPovMetaFg
                font.pointSize: Kirigami.Theme.smallFont.pointSize
                opacity: 0.6
            }
            // eye: hide/show this POV in the rail (secondary; appears on hover or when hidden)
            Rectangle {
                width: 20; height: 20; radius: 10
                anchors.verticalCenter: parent.verticalCenter
                visible: pbHover.hovered || pb.hidden
                color: eyeHover.hovered ? Th.memoriesPovEyeBgHover : "transparent"
                Controls.Label {
                    anchors.centerIn: parent
                    text: pb.hidden ? "🚫" : "👁"
                    font.pointSize: Kirigami.Theme.smallFont.pointSize
                    color: pb.onStage ? Th.memoriesPovMetaFgActive : Th.memoriesPovMetaFg
                }
                HoverHandler { id: eyeHover }
                TapHandler {
                    gesturePolicy: TapHandler.ReleaseWithinBounds
                    onTapped: if (pb.pov) mem.toggleHidden(pb.pov.file)
                }
                Controls.ToolTip.text: pb.hidden ? "Show in the side rail" : "Hide from the side rail"
                Controls.ToolTip.visible: eyeHover.hovered
                Controls.ToolTip.delay: 400
            }
        }
    }

    Item {
        anchors.fill: parent
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

        // 0.8.18: NOT a page-level ScrollView any more. The Breeze style's ScrollView
        // wheel handling ate every wheel event over the nested rail Flickable (field report:
        // "that side part is not scrollable"; reproduced in the harness — 0 px moved).
        // The player is a plain column that fills the window; only the library scrolls.
        ColumnLayout {
            id: memCol
            anchors.fill: parent
            anchors.margins: mem.theater ? 0 : Kirigami.Units.largeSpacing   // 10.1 (TH-MEM-10)
            anchors.topMargin: mem.theater ? 0 : Kirigami.Units.smallSpacing
            spacing: mem.theater ? 0 : Kirigami.Units.largeSpacing

            // ============================ HEADER ============================
            RowLayout {
                visible: !mem.theater   // 10.1 (TH-MEM-10): Esc / F / double-click leave fullscreen
                Layout.fillWidth: true
                Layout.topMargin: Kirigami.Units.smallSpacing
                spacing: Kirigami.Units.smallSpacing
                HeaderIconButton {
                    visible: mem.sel !== null
                    app: mem.app
                    iconName: "go-previous-symbolic"
                    tip: "All memories (Esc)"
                    onClicked: mem.backToList()
                }
                ColumnLayout {
                    spacing: 0
                    Kirigami.Heading {
                        textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                        text: mem.sel ? mem.fmtDateLong(mem.sel.start_ms) : "Memories"
                        color: Th.memoriesTitleFg
                        elide: Text.ElideRight
                        Layout.fillWidth: true
                    }
                    Controls.Label {
                        textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                        Layout.fillWidth: true
                        elide: Text.ElideRight
                        color: Th.memoriesMetaFg
                        font.pointSize: Kirigami.Theme.smallFont.pointSize
                        text: {
                            if (mem.sel) {
                                var ppl = mem.peopleOf(mem.sel)
                                var names = []
                                for (var i = 0; i < Math.min(4, ppl.length); i++) names.push(mem.shortName(ppl[i]))
                                if (ppl.length > 4) names.push("+" + (ppl.length - 4))
                                return (mem.selRoomName.length > 0 ? mem.selRoomName + " · " : "")
                                     + mem.fmtTime(mem.durSecs)
                                     + (names.length > 0 ? " · " + names.join(", ") : "")
                            }
                            var n = mem.memories ? mem.memories.length : 0
                            return (mem.roomName.length > 0 ? mem.roomName + " · " : "")
                                 + (n === 0 ? "recordings of this room's calls" : n + (n === 1 ? " memory" : " memories"))
                        }
                    }
                }
                Item { Layout.fillWidth: true }
                Pill {
                    visible: mem.synced
                    text: "⟲ synced with the call"
                    fg: Th.memoriesSyncPillFg
                    bg: Th.memoriesSyncPillBg
                }
                Pill {
                    visible: mem.sel !== null && mem.focusWaiting
                    text: "▶ " + (mem.focusedTrack ? mem.shortName(mem.focusedTrack.identity) : "") + " joins at " + mem.fmtTime(mem.focusedOffset)
                    fg: Th.memoriesWaitPillFg
                    bg: Th.memoriesWaitPillBg
                }
                HeaderIconButton {
                    visible: mem.sel === null
                    app: mem.app
                    iconName: "view-refresh-symbolic"
                    tip: "Refresh the list"
                    onClicked: mem.refreshRequested()
                }
                HeaderIconButton {
                    visible: mem.sel !== null
                    app: mem.app
                    iconName: mem.theater ? "view-restore-symbolic" : "view-fullscreen-symbolic"
                    tip: mem.theater ? "Leave fullscreen (F)" : "Fullscreen (F)"
                    onClicked: mem.setTheater(!mem.theater)
                }
            }

            // P8: transient notice when tapping a live/processing/empty card
            Rectangle {
                visible: mem.sel === null && mem.listNotice.length > 0
                Layout.fillWidth: true
                radius: mem.theme.radius
                color: mem.listNoticeKind === "live" ? Th.memoriesNoticeLiveBg
                     : mem.listNoticeKind === "warn" ? Th.memoriesNoticeWarnBg : Th.memoriesNoticeBg
                border.width: 1
                border.color: mem.listNoticeKind === "live" ? Th.memoriesNoticeLiveBorder
                            : mem.listNoticeKind === "warn" ? Th.memoriesNoticeWarnBorder : Th.memoriesNoticeBorder
                implicitHeight: noticeLbl.implicitHeight + Kirigami.Units.largeSpacing * 1.5
                Controls.Label {
                    textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                    id: noticeLbl
                    anchors.fill: parent
                    anchors.margins: Kirigami.Units.largeSpacing * 0.75
                    wrapMode: Text.Wrap
                    color: Th.memoriesNoticeFg
                    text: (mem.listNoticeKind === "live" ? "🔴 " : (mem.listNoticeKind === "warn" ? "⏳ " : "")) + mem.listNotice
                }
            }
            Timer { // clear the notice after a few seconds
                running: mem.listNotice.length > 0
                interval: 6000
                onTriggered: mem.listNotice = ""
            }
            Timer { // P8: auto-refresh the list while a session is still live/processing
                interval: 20000; repeat: true
                running: mem.visible && mem.sel === null && mem.anyPending
                onTriggered: mem.refreshRequested()
            }

            // ============================ LIST VIEW ============================
            Controls.ScrollView {
                id: libraryScroll
                visible: mem.sel === null
                Layout.fillWidth: true
                Layout.fillHeight: true
                contentWidth: availableWidth
                Controls.ScrollBar.horizontal.policy: Controls.ScrollBar.AlwaysOff
                ColumnLayout {
                    id: libCol
                    spacing: Kirigami.Units.largeSpacing
                    width: Math.min(libraryScroll.availableWidth, Kirigami.Units.gridUnit * 46)
                    anchors.horizontalCenter: parent.horizontalCenter
            // couldn't load (never had a list) → say so, offer retry
            Item {
                visible: mem.sel === null && mem.memories.length === 0 && mem.memoriesError.length > 0
                Layout.fillWidth: true
                Layout.preferredHeight: Kirigami.Units.gridUnit * 16
                EmptyState {
                    anchors.fill: parent
                    app: mem.app
                    iconName: "network-disconnect-symbolic"
                    title: "Couldn't load memories"
                    // 10.1 (TH-MEM-05): one short line of the error, never a whole HTML page.
                    explanation: "The recordings server didn't answer (" + mem.shortError(mem.memoriesError) + "). Your list is safe — try again in a moment."
                }
                GlowButton {
                    anchors.horizontalCenter: parent.horizontalCenter
                    anchors.bottom: parent.bottom
                    app: mem.app
                    text: "Try again"
                    icon.name: "view-refresh-symbolic"
                    onClicked: mem.refreshRequested()
                }
            }
            // 10.1 (TH-MEM-02): fetching
            Item {
                visible: mem.sel === null && mem.memories.length === 0 && mem.memoriesError.length === 0 && mem.loading
                Layout.fillWidth: true
                Layout.preferredHeight: Kirigami.Units.gridUnit * 16
                EmptyState {
                    anchors.fill: parent
                    app: mem.app
                    loading: true
                    title: "Loading memories…"
                }
            }
            // nothing recorded yet
            Item {
                visible: mem.sel === null && mem.memories.length === 0 && mem.memoriesError.length === 0 && !mem.loading
                Layout.fillWidth: true
                Layout.preferredHeight: Kirigami.Units.gridUnit * 16
                EmptyState {
                    anchors.fill: parent
                    app: mem.app
                    iconName: "camera-video-symbolic"
                    title: "No memories here yet"
                    explanation: "Press ⏺ Rec in a call and every stream and voice is saved for reliving later — with 🔖 Clip to bookmark the good bits."
                }
            }
            Repeater {
                model: mem.sel === null ? mem.memories : []
                delegate: Rectangle {
                    id: card
                    required property var modelData
                    required property int index
                    readonly property bool live: !modelData.end_ms
                    readonly property bool processing: !!modelData.end_ms && modelData.ready === false && !modelData.empty
                    // 10.1 (TH-MEM-04): minutes since the session closed (cards rebuild on every refresh).
                    readonly property int processingMin: processing ? Math.max(0, Math.round((Date.now() - modelData.end_ms) / 60000)) : 0
                    readonly property bool stuck: processing && processingMin > 10
                    readonly property bool empty: !!modelData.empty
                    readonly property bool openable: !live && !processing && !empty
                    readonly property var kinds: mem.countKinds(modelData)
                    readonly property var people: mem.peopleOf(modelData)
                    Layout.fillWidth: true
                    implicitHeight: Math.max(poster.height, cardCol.implicitHeight) + Kirigami.Units.largeSpacing * 2
                    radius: mem.theme.radius
                    color: cardHover.hovered ? Th.memoriesCardBgHover : Th.memoriesCardBg
                    border.width: 1
                    border.color: cardHover.hovered ? Th.memoriesCardBorderHover : Th.memoriesCardBorder
                    opacity: card.empty ? 0.6 : 1
                    Behavior on border.color { ColorAnimation { duration: mem.theme.dur } }
                    Behavior on color { ColorAnimation { duration: mem.theme.dur } }
                    HoverHandler { id: cardHover; cursorShape: Qt.PointingHandCursor }
                    TapHandler { onTapped: mem.tryOpen(card.modelData) }
                    RowLayout {
                        anchors.fill: parent
                        anchors.margins: Kirigami.Units.largeSpacing
                        spacing: Kirigami.Units.largeSpacing
                        // ---- poster ----
                        Rectangle {
                            id: poster
                            Layout.preferredWidth: 176
                            Layout.preferredHeight: 99
                            Layout.alignment: Qt.AlignTop
                            radius: mem.theme.radiusSm
                            clip: true
                            color: Th.memoriesCardPosterBg
                            border.width: 1
                            border.color: Th.memoriesCardPosterBorder
                            gradient: Gradient {
                                GradientStop { position: 0.0; color: Th.memoriesCardPosterHighlight }
                                GradientStop { position: 1.0; color: Th.memoriesCardPosterBg }
                            }
                            property int retries: 0
                            readonly property string wantUrl: card.openable ? mem.posterUrl(card.modelData) : ""
                            property string loadUrl: ""
                            // stagger the poster fetches (the thumb endpoint is capacity-capped;
                            // a burst of N at open is exactly what 503s)
                            Timer {
                                interval: 250 + card.index * 350
                                running: poster.wantUrl.length > 0 && poster.loadUrl.length === 0 && mem.visible
                                onTriggered: poster.loadUrl = poster.wantUrl
                            }
                            Timer {
                                id: posterRetry
                                interval: 2500 + card.index * 300
                                onTriggered: poster.loadUrl = poster.wantUrl + "&r=" + poster.retries
                            }
                            Image {
                                id: posterImg
                                anchors.fill: parent
                                fillMode: Image.PreserveAspectCrop
                                asynchronous: true
                                cache: true
                                sourceSize.width: 352
                                source: poster.loadUrl
                                opacity: status === Image.Ready ? 1 : 0
                                Behavior on opacity { NumberAnimation { duration: mem.theme.durSlow } }
                                onStatusChanged: if (status === Image.Error && poster.retries < 3) {
                                    poster.retries++
                                    posterRetry.restart()
                                }
                            }
                            Controls.Label {
                                anchors.centerIn: parent
                                visible: posterImg.status !== Image.Ready
                                text: card.live ? "🔴" : (card.empty ? "∅" : (card.processing ? "⏳" : "📼"))
                                font.pointSize: Kirigami.Theme.defaultFont.pointSize * 1.8
                                color: Th.memoriesCardPosterFg
                            }
                            // duration badge
                            Pill {
                                anchors.right: parent.right
                                anchors.bottom: parent.bottom
                                anchors.margins: 5
                                visible: !!card.modelData.end_ms && !card.empty
                                text: mem.fmtTime((card.modelData.end_ms - card.modelData.start_ms) / 1000)
                                fg: Th.memoriesCardDurationFg
                                bg: Th.memoriesCardDurationBg
                                bold: true
                            }
                        }
                        // ---- text ----
                        ColumnLayout {
                            id: cardCol
                            Layout.fillWidth: true
                            Layout.alignment: Qt.AlignVCenter
                            spacing: 4
                            RowLayout {
                                Layout.fillWidth: true
                                spacing: Kirigami.Units.smallSpacing
                                Controls.Label {
                                    textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                    text: mem.fmtDate(card.modelData.start_ms)
                                    color: Th.memoriesCardTitleFg
                                    font.bold: true
                                    font.pointSize: Kirigami.Theme.defaultFont.pointSize * 1.1
                                    elide: Text.ElideRight
                                }
                                // 11.0 (GPU.md §2.2, R6): STATIC — shown lit (it blinked every 1.2 s for the length of the call)
                                Pill { visible: card.live; text: "● LIVE"; fg: Th.memoriesCardLiveFg; bg: Th.memoriesCardLiveBg; bold: true }
                                Pill { visible: card.processing; text: card.stuck ? "⏳ Processing — stuck?" : "⏳ Processing"; fg: card.stuck ? Th.memoriesCardProcessingFgError : Th.memoriesCardProcessingFg; bg: card.stuck ? Th.memoriesCardProcessingBgError : Th.memoriesCardProcessingBg }   // 10.1 (TH-MEM-04)
                                Pill { visible: card.empty; text: "nothing captured"; fg: Th.memoriesPillFg; bg: Th.memoriesPillBg }
                                Item { Layout.fillWidth: true }
                            }
                            Controls.Label {
                                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                Layout.fillWidth: true
                                elide: Text.ElideRight
                                color: Th.memoriesCardMetaFg
                                font.pointSize: Kirigami.Theme.smallFont.pointSize
                                text: (card.modelData.room_name ? card.modelData.room_name + " · " : "")
                                    + "started by " + mem.shortName(card.modelData.started_by)
                            }
                            // people row
                            RowLayout {
                                Layout.fillWidth: true
                                spacing: Kirigami.Units.smallSpacing
                                visible: card.people.length > 0
                                Row {
                                    spacing: -6
                                    Repeater {
                                        model: card.people.slice(0, 5)
                                        delegate: Avatar {
                                            required property var modelData
                                            app: mem.app
                                            name: mem.shortName(modelData)
                                            seed: String(modelData)
                                            size: 22
                                        }
                                    }
                                }
                                Controls.Label {
                                    textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                    Layout.fillWidth: true
                                    elide: Text.ElideRight
                                    color: Th.memoriesCardMetaFg
                                    font.pointSize: Kirigami.Theme.smallFont.pointSize
                                    text: {
                                        var names = []
                                        for (var i = 0; i < Math.min(3, card.people.length); i++) names.push(mem.shortName(card.people[i]))
                                        if (card.people.length > 3) names.push("+" + (card.people.length - 3) + " more")
                                        return names.join(", ")
                                    }
                                }
                            }
                            // counts + size
                            RowLayout {
                                Layout.fillWidth: true
                                spacing: Kirigami.Units.smallSpacing
                                Pill { visible: card.kinds.streams > 0; text: "🖥 " + card.kinds.streams + (card.kinds.streams === 1 ? " stream" : " streams") }
                                Pill { visible: card.kinds.cams > 0; text: "📷 " + card.kinds.cams + (card.kinds.cams === 1 ? " camera" : " cameras") }
                                Pill { visible: card.kinds.voices > 0; text: "🎙 " + card.kinds.voices + (card.kinds.voices === 1 ? " voice" : " voices") }
                                Pill { visible: Number(card.modelData.bytes || 0) > 0; text: mem.fmtBytes(card.modelData.bytes) }
                                Item { Layout.fillWidth: true }
                                Controls.Label {
                                    textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                    visible: card.openable && Number(mem.resumeAt[String(card.modelData.id)] || 0) > 5
                                    text: "resume at " + mem.fmtTime(Number(mem.resumeAt[String(card.modelData.id)] || 0))
                                    color: Th.memoriesCardResumeFg
                                    font.pointSize: Kirigami.Theme.smallFont.pointSize
                                }
                            }
                        }
                        Kirigami.Icon {
                            Layout.alignment: Qt.AlignVCenter
                            visible: card.openable
                            source: "media-playback-start-symbolic"
                            isMask: true
                            color: cardHover.hovered ? Th.memoriesCardIconHover : Th.memoriesCardIcon
                            width: Kirigami.Units.iconSizes.smallMedium
                            height: width
                        }
                    }
                }
            }

                }
            }

            // T3: decoder-missing banner — the machine has no usable gst H.264
            // decoder, so side tiles show snapshots instead of live motion. Tell the
            // user the exact one-time fix.
            Rectangle {
                visible: mem.sel !== null && mem.decoderBroken && !mem.theater
                Layout.fillWidth: true
                radius: mem.theme.radius
                color: Th.memoriesDecoderBg
                border.width: 1
                border.color: Th.memoriesDecoderBorder
                implicitHeight: decLbl.implicitHeight + Kirigami.Units.largeSpacing * 1.5
                Controls.Label {
                    id: decLbl
                    anchors.fill: parent
                    anchors.margins: Kirigami.Units.largeSpacing * 0.75
                    wrapMode: Text.Wrap
                    color: Th.memoriesFg
                    text: "⚠️ Live side tiles need a video decoder this machine is missing, so they're showing snapshots. "
                        + "On Arch: sudo pacman -S gst-libav — then restart Tin Hat. The main view still plays normally."
                }
            }

            // ====================== PLAYER: the relive grid ======================
            RowLayout {
                id: stageRow
                visible: mem.sel !== null
                Layout.fillWidth: true
                Layout.fillHeight: true
                spacing: Kirigami.Units.smallSpacing
                // main (focused) surface — the ONE mpv that also carries the audio
                Rectangle {
                    id: mainSurface
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    Layout.minimumHeight: 240
                    radius: mem.theme.radius
                    color: Th.memoriesStageBg
                    border.width: 1
                    border.color: Th.memoriesStageBorder
                    clip: true
                    // 0.8.30: rounded corners over the mpv surface (see CornerMask.qml).
                    CornerMask { radius: mem.theme.radius; color: Th.memoriesBg; z: 15 }
                    Rectangle { anchors.fill: parent; z: 16; radius: mem.theme.radius; color: "transparent"; border.width: 1; border.color: Th.memoriesStageBorder }
                    MpvVideo {
                        id: mainMpv
                        anchors.fill: parent
                        onReady: {
                            mainMpv.setProperty("ao", "pulse,pipewire,alsa")
                            mainMpv.setProperty("hwdec", "auto-safe")
                            mainMpv.setProperty("cache", "yes")
                            mainMpv.setProperty("cache-secs", 20)
                            mainMpv.setProperty("force-seekable", "yes")
                            mainMpv.setProperty("hr-seek", "yes")
                            mainMpv.setProperty("keep-open", "yes")
                            mem.mainReady = true
                            mem.applyVolume()
                            mem.applyMain()
                        }
                    }
                    // click = play/pause NOW (10.1, TH-MEM-16: every click used to land 260 ms late);
                    // a double-click toggles fullscreen and undoes the first click's toggle, so it
                    // ends in the playback state it started in.
                    TapHandler {
                        onSingleTapped: mem.togglePlay()
                        onDoubleTapped: { mem.togglePlay(); mem.setTheater(!mem.theater) }
                    }
                    HoverHandler { id: stageHover; onPointChanged: mem._chromeTouch() }   // 10.1 (TH-MEM-10): movement reveals the transport
                    // P4: focused POV not started yet — the veil says when
                    Rectangle {
                        anchors.fill: parent
                        visible: mem.sel !== null && mem.focusedTrack !== null && mem.focusWaiting
                        gradient: Gradient {
                            GradientStop { position: 0.0; color: Th.memoriesStageWaitHighlight }
                            GradientStop { position: 1.0; color: Th.memoriesStageWaitBg }
                        }
                        ColumnLayout {
                            anchors.centerIn: parent
                            spacing: Kirigami.Units.smallSpacing
                            Avatar {
                                Layout.alignment: Qt.AlignHCenter
                                app: mem.app
                                name: mem.focusedTrack ? mem.shortName(mem.focusedTrack.identity) : ""
                                seed: mem.focusedTrack ? mem.userIdOf(mem.focusedTrack.identity) : ""
                                size: 72
                            }
                            Controls.Label {
                                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                Layout.alignment: Qt.AlignHCenter
                                text: mem.focusedTrack ? mem.povDescr(mem.focusedTrack) : ""
                                color: Th.memoriesStageFg
                                font.bold: true
                                font.pointSize: Kirigami.Theme.defaultFont.pointSize + 2
                            }
                            Controls.Label {
                                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                Layout.alignment: Qt.AlignHCenter
                                text: "starts at " + mem.fmtTime(mem.focusedOffset) + " · " + mem.fmtTime(Math.max(0, mem.focusedOffset - mem.globalPos)) + " to go"
                                color: Th.memoriesStageMetaFg
                            }
                            Controls.Button {
                                Layout.alignment: Qt.AlignHCenter
                                text: "Skip to it"
                                icon.name: "media-skip-forward"
                                focusPolicy: Qt.NoFocus
                                onClicked: mem.commitSeek(mem.focusedOffset)
                            }
                        }
                    }
                    // audio-only stage
                    ColumnLayout {
                        anchors.centerIn: parent
                        visible: mem.sel !== null && mem.focusedTrack === null
                        spacing: Kirigami.Units.smallSpacing
                        Controls.Label {
                            Layout.alignment: Qt.AlignHCenter
                            text: "🎧"
                            font.pointSize: Kirigami.Theme.defaultFont.pointSize * 3
                        }
                        Controls.Label {
                            Layout.alignment: Qt.AlignHCenter
                            text: "Audio only"
                            color: Th.memoriesStageFg
                            font.bold: true
                        }
                        Controls.Label {
                            Layout.alignment: Qt.AlignHCenter
                            text: mem.videoTracks.length > 0 ? "pick a point of view below to see it" : "this memory has voices only"
                            color: Th.memoriesStageMetaFg
                            font.pointSize: Kirigami.Theme.smallFont.pointSize
                        }
                    }
                    // stage caption (who + what is on stage)
                    Rectangle {
                        anchors.left: parent.left
                        anchors.bottom: parent.bottom
                        anchors.margins: 10
                        visible: mem.focusedTrack !== null && !mem.focusWaiting
                        radius: height / 2
                        color: Th.memoriesCaptionBg
                        border.width: 1
                        border.color: Th.memoriesCaptionBorder
                        width: stageCapRow.implicitWidth + 20
                        height: stageCapRow.implicitHeight + 10
                        RowLayout {
                            id: stageCapRow
                            anchors.centerIn: parent
                            spacing: 7
                            Avatar {
                                app: mem.app
                                name: mem.focusedTrack ? mem.shortName(mem.focusedTrack.identity) : ""
                                seed: mem.focusedTrack ? mem.userIdOf(mem.focusedTrack.identity) : ""
                                size: 22
                            }
                            Controls.Label {
                                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                text: mem.focusedTrack ? mem.shortName(mem.focusedTrack.identity) : ""
                                color: Th.memoriesCaptionFg
                                font.bold: true
                            }
                            Controls.Label {
                                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                text: mem.focusedTrack ? (mem.kindGlyph(mem.focusedTrack.kind) + " " + mem.povDescr(mem.focusedTrack).replace(/^.*'s /, "")) : ""
                                color: Th.memoriesCaptionMetaFg
                            }
                        }
                    }
                    // stage transition: a quick dip with the name of what just went on stage
                    Rectangle {
                        id: stageFlashRect
                        anchors.fill: parent
                        color: Th.memoriesFlashOverlay
                        opacity: 0
                        z: 6
                        Controls.Label {
                            textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                            anchors.centerIn: parent
                            text: mem.stageFlashText
                            color: Th.memoriesStageFg
                            font.bold: true
                            font.pointSize: Kirigami.Theme.defaultFont.pointSize + 3
                            opacity: stageFlashRect.opacity * 1.6
                        }
                    }
                    SequentialAnimation {
                        id: stageFlash
                        // 11.0 (GPU.md R3): ONE `verylong` step (400 ms) — up in its first quarter, out in the rest (it was
                        // 90 + 260 + 320 = 670 ms; the web's stageFlash keyframes)
                        NumberAnimation { target: stageFlashRect; property: "opacity"; to: 0.6; duration: Math.round(Motion.durVeryLong / 4) }
                        NumberAnimation { target: stageFlashRect; property: "opacity"; to: 0; duration: Motion.durVeryLong - Math.round(Motion.durVeryLong / 4); easing.type: Easing.InQuad }
                    }
                    // paused glyph (hover-only, subtle)
                    Rectangle {
                        anchors.centerIn: parent
                        visible: mem.sel !== null && !mem.playing && !mem.atEnd && mem.focusedTrack !== null && !mem.focusWaiting
                        width: 64; height: 64; radius: 32
                        color: Th.memoriesPausedBg
                        opacity: stageHover.hovered ? 0.95 : 0.6
                        ThIcon { anchors.centerIn: parent; width: 30; height: 30; app: mem.app; name: "media-playback-start-symbolic"; color: Th.memoriesPausedIcon; isMask: true }   // 0.8.30: pack glyph
                    }
                    // toast
                    Rectangle {
                        anchors.horizontalCenter: parent.horizontalCenter
                        anchors.bottom: parent.bottom
                        anchors.bottomMargin: 14
                        visible: mem.toast.length > 0
                        radius: height / 2
                        color: Th.memoriesToastBg
                        border.width: 1
                        border.color: Th.memoriesToastBorder
                        width: toastLbl.implicitWidth + 22
                        height: toastLbl.implicitHeight + 10
                        Controls.Label { textFormat: Text.PlainText; id: toastLbl; anchors.centerIn: parent; text: mem.toast; color: Th.memoriesToastFg }
                    }
                    // P2: ended-state overlay — replaces the frozen-at-end look
                    Rectangle {
                        anchors.fill: parent
                        visible: mem.atEnd
                        color: Th.memoriesEndOverlay
                        ColumnLayout {
                            anchors.centerIn: parent
                            spacing: Kirigami.Units.largeSpacing
                            Controls.Label {
                                Layout.alignment: Qt.AlignHCenter
                                text: "That's the end of this one"
                                color: Th.memoriesStageFg
                                font.pointSize: Kirigami.Theme.defaultFont.pointSize + 4
                                font.bold: true
                            }
                            Controls.Label {
                                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                Layout.alignment: Qt.AlignHCenter
                                text: mem.fmtTime(mem.durSecs) + " relived"
                                color: Th.memoriesStageMetaFg
                            }
                            RowLayout {
                                Layout.alignment: Qt.AlignHCenter
                                spacing: Kirigami.Units.largeSpacing
                                GlowButton {
                                    app: mem.app
                                    text: "Watch again"
                                    icon.name: "view-refresh-symbolic"
                                    onClicked: mem.togglePlay()
                                }
                                Controls.Button {
                                    text: "All memories"
                                    icon.name: "go-previous-symbolic"
                                    focusPolicy: Qt.NoFocus
                                    onClicked: mem.backToList()
                                }
                            }
                        }
                    }
                }
                // side rail — every OTHER visible track, live-call style; click = on stage.
                // Tiles are positioned by rank (mem.railOrder), not declaration order,
                // so a re-order is a slide, never a rebuild (sticky slots).
                Item {
                    id: sideCol
                    visible: mem.sideTracks.length > 0
                    Layout.minimumWidth: Math.min(480, Math.max(200, memCol.width * 0.24))
                    Layout.preferredWidth: Math.min(480, Math.max(200, memCol.width * 0.24))
                    Layout.fillHeight: true
                    Layout.alignment: Qt.AlignTop
                    Flickable {
                        id: railFlick
                        anchors.fill: parent
                        contentHeight: tileCol.height
                        contentWidth: width
                        clip: true
                        boundsBehavior: Flickable.StopAtBounds
                        // Explicit wheel scrolling (0.8.18): the Flickable's own wheel handling
                        // never fired in the field ("that side part is not scrollable") nor in
                        // the harness, while dragging worked — a pointer handler receives wheel
                        // events directly. Two notches ≈ one card; trackpads use pixel deltas.
                        WheelHandler {
                            acceptedDevices: PointerDevice.Mouse | PointerDevice.TouchPad
                            onWheel: (event) => {
                                var step = event.pixelDelta.y !== 0 ? event.pixelDelta.y
                                                                    : (event.angleDelta.y / 120) * tileCol.tileH * 0.6
                                var maxY = Math.max(0, railFlick.contentHeight - railFlick.height)
                                railFlick.contentY = Math.max(0, Math.min(maxY, railFlick.contentY - step))
                                event.accepted = true
                            }
                        }
                        Controls.ScrollBar.vertical: Controls.ScrollBar { policy: Controls.ScrollBar.AsNeeded }
                        Item {
                            id: tileCol
                            width: parent.width
                            readonly property real tileH: Math.round(width * 9 / 16)
                            height: Math.max(0, mem.railOrder.length * (tileH + mem.railGap) - mem.railGap)
                            // static pool: 5 LIVE slots (a real Gaming Central session runs
                            // 2 cams + 3 shares) + 3 snapshot-only overflow slots, so an
                            // 8-track session shows every POV.
                            SideTile { slot: 0 }
                            SideTile { slot: 1 }
                            SideTile { slot: 2 }
                            SideTile { slot: 3 }
                            SideTile { slot: 4 }
                            SideTile { slot: 5 }
                            SideTile { slot: 6 }
                            SideTile { slot: 7 }
                        }
                    }
                }
            }

            // ============================ TRANSPORT ============================
            Rectangle {
                visible: mem.sel !== null && (mem.chromeShown || opacity > 0.01)
                opacity: mem.chromeShown ? 1 : 0   // 10.1 (TH-MEM-10): fades out in fullscreen when idle
                Behavior on opacity { NumberAnimation { duration: Motion.durSlow } }
                HoverHandler { id: transportHover; onHoveredChanged: if (hovered) mem._chromeTouch() }
                Layout.fillWidth: true
                Layout.leftMargin: mem.theater ? Kirigami.Units.largeSpacing : 0
                Layout.rightMargin: mem.theater ? Kirigami.Units.largeSpacing : 0
                Layout.bottomMargin: mem.theater ? Kirigami.Units.largeSpacing : 0
                radius: mem.theme.radius
                color: Th.memoriesTransportBg
                border.width: 1
                border.color: Th.memoriesTransportBorder
                implicitHeight: transportCol.implicitHeight + Kirigami.Units.largeSpacing
                ColumnLayout {
                    id: transportCol
                    anchors.fill: parent
                    anchors.margins: Kirigami.Units.largeSpacing / 2
                    anchors.leftMargin: Kirigami.Units.largeSpacing
                    anchors.rightMargin: Kirigami.Units.largeSpacing
                    spacing: 2
                    // timeline
                    Item {
                        Layout.fillWidth: true
                        Layout.preferredHeight: seekSlider.implicitHeight
                        ThSlider {
                            id: seekSlider
                            anchors.fill: parent
                            from: 0
                            to: Math.max(1, mem.durSecs)
                            value: pressed ? value : mem.globalPos
                            focusPolicy: Qt.NoFocus
                            onMoved: mem.userSeek(value)
                            onPressedChanged: if (!pressed) mem.commitSeek(value)
                            Controls.ToolTip.text: mem.fmtTime(seekSlider.value)
                            Controls.ToolTip.visible: seekSlider.pressed
                        }
                        // clip markers
                        Repeater {
                            model: mem.sessionClips
                            delegate: Rectangle {
                                required property var modelData
                                width: 4; height: 14; radius: 2
                                color: Th.memoriesClipMarkFill
                                border.width: 1
                                border.color: Th.memoriesClipMarkBorder
                                y: (seekSlider.height - height) / 2
                                x: Math.max(0, Math.min(seekSlider.width - width,
                                    (modelData.off / Math.max(1, mem.durSecs)) * seekSlider.width))
                                scale: tickHover.hovered ? 1.4 : 1
                                Behavior on scale { NumberAnimation { duration: mem.theme.dur } }
                                Controls.ToolTip.text: "🔖 " + mem.shortName(modelData.by) + " · " + mem.fmtTime(modelData.off)
                                Controls.ToolTip.visible: tickHover.hovered
                                HoverHandler { id: tickHover; cursorShape: Qt.PointingHandCursor }
                                TapHandler { onTapped: mem.commitSeek(parent.modelData.off) }
                            }
                        }
                    }
                    // buttons
                    RowLayout {
                        Layout.fillWidth: true
                        spacing: Kirigami.Units.smallSpacing
                        HeaderIconButton {
                            app: mem.app
                            iconName: "media-skip-backward"
                            tip: "Back 10s (←)"
                            onClicked: mem.skipBy(-10)
                        }
                        Rectangle {
                            width: 40; height: 40; radius: 20
                            color: playHover.hovered ? Th.memoriesPlayBgHover : Th.memoriesPlayBg
                            border.width: 1
                            border.color: Th.memoriesPlayBorder
                            Behavior on color { ColorAnimation { duration: mem.theme.dur } }
                            Kirigami.Icon {
                                anchors.centerIn: parent
                                width: Kirigami.Units.iconSizes.smallMedium
                                height: width
                                source: mem.atEnd ? "view-refresh-symbolic" : (mem.playing ? "media-playback-pause" : "media-playback-start")
                                isMask: true
                                color: Th.memoriesPlayIcon
                            }
                            HoverHandler { id: playHover; cursorShape: Qt.PointingHandCursor }
                            TapHandler { onTapped: mem.togglePlay() }
                            Controls.ToolTip.text: mem.synced ? (mem.playing ? "Pause for everyone" : "Play for everyone")
                                                              : (mem.atEnd ? "Watch again" : (mem.playing ? "Pause (Space)" : "Play (Space)"))
                            Controls.ToolTip.visible: playHover.hovered
                            Controls.ToolTip.delay: 500
                        }
                        HeaderIconButton {
                            app: mem.app
                            iconName: "media-skip-forward"
                            tip: "Forward 10s (→)"
                            onClicked: mem.skipBy(10)
                        }
                        Controls.Label {
                            textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                            text: mem.fmtTime(mem.globalPos) + "  /  " + mem.fmtTime(mem.durSecs)
                            color: Th.memoriesTimeFg
                            font.family: Th.fontMono
                            Layout.leftMargin: Kirigami.Units.smallSpacing
                        }
                        Item { Layout.fillWidth: true }
                        // volume
                        HeaderIconButton {
                            app: mem.app
                            iconName: (mem.muted || mem.volume === 0) ? "audio-volume-muted-symbolic"
                                    : (mem.volume < 50 ? "audio-volume-low-symbolic" : "audio-volume-high-symbolic")
                            tip: mem.muted ? "Unmute (M)" : "Mute (M)"
                            onClicked: mem.muted = !mem.muted
                        }
                        ThSlider {
                            id: volSlider
                            Layout.preferredWidth: 96
                            from: 0; to: 100
                            value: mem.volume
                            focusPolicy: Qt.NoFocus
                            onMoved: { mem.volume = value; if (mem.muted && value > 0) mem.muted = false }
                            Controls.ToolTip.text: Math.round(volSlider.value) + "%"
                            Controls.ToolTip.visible: volSlider.pressed || volSlider.hovered
                        }
                        Controls.Button {
                            text: mem.synced ? "Stop sync" : "⟲ Sync with call"
                            enabled: mem.inCall
                            highlighted: mem.synced
                            focusPolicy: Qt.NoFocus
                            hoverEnabled: true
                            Controls.ToolTip.text: !mem.inCall
                                ? "Join the room's call to relive this together — everyone in the call who syncs follows one clock"
                                : (mem.synced ? "Back to replaying on your own"
                                              : "Everyone in the call who opens this memory follows the same clock")
                            Controls.ToolTip.visible: hovered
                            onClicked: mem.synced ? mem.syncStop() : mem.syncStart(mem.memUrl)
                        }
                    }
                }
            }

            // points of view — grouped by person; click one to put it on stage
            ColumnLayout {
                visible: mem.sel !== null && mem.videoTracks.length > 0 && !mem.theater
                Layout.fillWidth: true
                spacing: 6
                RowLayout {
                    Layout.fillWidth: true
                    spacing: Kirigami.Units.smallSpacing
                    Controls.Label {
                        text: "Points of view"
                        color: Th.memoriesFg
                        font.bold: true
                    }
                    Controls.Label {
                        text: "· click one to put it on stage, or press its number"
                        color: Th.memoriesMetaFg
                        font.pointSize: Kirigami.Theme.smallFont.pointSize
                        elide: Text.ElideRight
                        Layout.fillWidth: true
                    }
                    Controls.Button {
                        visible: mem.hiddenCount > 0
                        text: LinkGate.plain("Show all (" + mem.hiddenCount + " hidden)")
                        flat: true
                        focusPolicy: Qt.NoFocus
                        onClicked: mem.showAll()
                    }
                }
                Flow {
                    Layout.fillWidth: true
                    spacing: Kirigami.Units.largeSpacing
                    Repeater {
                        model: mem.povGroups
                        delegate: Row {
                            id: povGroup
                            required property var modelData
                            spacing: 6
                            Avatar {
                                anchors.verticalCenter: parent.verticalCenter
                                app: mem.app
                                name: povGroup.modelData.name
                                seed: povGroup.modelData.user
                                size: 24
                            }
                            Controls.Label {
                                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                anchors.verticalCenter: parent.verticalCenter
                                text: povGroup.modelData.name
                                color: Th.memoriesFg
                                font.bold: true
                                rightPadding: 2
                            }
                            Repeater {
                                model: povGroup.modelData.povs
                                delegate: PovButton {
                                    required property var modelData
                                    anchors.verticalCenter: parent.verticalCenter
                                    pov: modelData
                                }
                            }
                        }
                    }
                    // voices only
                    Rectangle {
                        readonly property bool on: mem.focusedFile === "" && mem.sel !== null
                        visible: mem.audioUrl.length > 0
                        radius: height / 2
                        color: on ? Th.memoriesPovBgActive : (aoHover.hovered ? Th.memoriesPovBgHover : Th.memoriesPovBg)
                        border.width: 1
                        border.color: on ? Th.memoriesPovBorderActive : Th.memoriesPovBorder
                        implicitWidth: aoLbl.implicitWidth + 22
                        implicitHeight: aoLbl.implicitHeight + 10
                        Behavior on color { ColorAnimation { duration: mem.theme.dur } }
                        Controls.Label {
                            textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                            id: aoLbl
                            anchors.centerIn: parent
                            text: "🎧 Voices only" + (parent.on ? " · on stage" : "") + "  0"
                            color: parent.on ? Th.memoriesPovFgActive : Th.memoriesPovFg
                            font.bold: parent.on
                        }
                        HoverHandler { id: aoHover; cursorShape: Qt.PointingHandCursor }
                        TapHandler { onTapped: mem.focusTrack("") }
                        Controls.ToolTip.text: "Just the voices — no video on stage (key 0)"
                        Controls.ToolTip.visible: aoHover.hovered
                        Controls.ToolTip.delay: 500
                    }
                }
            }

            // clip jump chips
            ColumnLayout {
                visible: mem.sel !== null && mem.sessionClips.length > 0 && !mem.theater
                Layout.fillWidth: true
                Layout.bottomMargin: Kirigami.Units.largeSpacing
                spacing: 4
                Controls.Label {
                    text: "Clips"
                    color: Th.memoriesMetaFg
                    font.pointSize: Kirigami.Theme.smallFont.pointSize
                    font.bold: true
                }
                Flow {
                    Layout.fillWidth: true
                    spacing: Kirigami.Units.smallSpacing
                    Repeater {
                        model: mem.sessionClips
                        delegate: Rectangle {
                            required property var modelData
                            radius: height / 2
                            color: clipHover.hovered ? Th.memoriesClipBgHover : Th.memoriesClipBg
                            border.width: 1
                            border.color: Th.memoriesClipBorder
                            implicitWidth: clipLbl.implicitWidth + 20
                            implicitHeight: clipLbl.implicitHeight + 10
                            Behavior on color { ColorAnimation { duration: mem.theme.dur } }
                            Controls.Label {
                                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                id: clipLbl
                                anchors.centerIn: parent
                                text: "🔖 " + mem.fmtTime(modelData.off) + " · " + mem.shortName(modelData.by)
                                color: Th.memoriesClipFg
                            }
                            HoverHandler { id: clipHover; cursorShape: Qt.PointingHandCursor }
                            TapHandler { onTapped: mem.commitSeek(parent.modelData.off) }
                        }
                    }
                }
            }
            // keyboard hint
            Controls.Label {
                visible: mem.sel !== null && !mem.theater
                Layout.fillWidth: true
                Layout.bottomMargin: Kirigami.Units.largeSpacing
                horizontalAlignment: Text.AlignHCenter
                color: Th.memoriesMetaFg
                opacity: 0.7
                font.pointSize: Kirigami.Theme.smallFont.pointSize
                text: "Space play/pause · ← → 10s · Shift+← → 1 min · 1-9 put a point of view on stage · 0 voices only · M mute · F fullscreen · Esc back"
            }
        }
    }
}
