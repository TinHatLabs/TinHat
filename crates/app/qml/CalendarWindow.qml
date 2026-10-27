// CalendarWindow.qml — the squad calendar (0.8.26, 2026-09-08).
//
// One calendar per SPACE (a room outside any space gets its own). Month grid with event
// chips, an Upcoming agenda, and a Games & rules list; the side pane shows the selected day,
// an event's detail (when · who's in · RSVP · proposal votes · rules), or the editor.
// Kinds: Group session · Free play · Scheduled progression ("boss run — join if you can") ·
// Proposal (vote on times, the organiser locks one in) · Cool-off game. Creating an event can
// post a summary to the room the calendar was opened from.
//
// Data + intent flow the app's usual way: plain props mirrored from CalendarController by
// Main.qml, signals back up (CXX-Qt: no direct controller binding). Every time crosses the
// boundary as epoch milliseconds; this file renders them in the local zone.
import QtQuick
import QtQuick.Window
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the calendar.* tokens

Window {
    id: cal
    required property var app
    // 10.1 (TH-CAL-22 / TH-MEM-08 / TH-WP-15): remembered size (+ position off Wayland) across runs.
    WindowMemory { target: cal; key: "calendar"; store: (cal.app && cal.app.prefStore) ? cal.app.prefStore : null }
    property string roomId: ""
    property string roomName: ""
    property string containerId: ""
    property string containerLabel: ""
    property var events: []
    property var games: []
    property var containers: []
    property bool canEdit: false
    property bool isMod: false
    property string myId: ""
    property bool busy: false
    property bool reminders: true
    property string errorText: ""
    // 0.8.27 — space membership: people in the room I opened from who are not in the space,
    // whether I may invite, and the parent space I am not in (why the calendar is the room's own).
    property var missing: []
    property bool canInvite: false
    property string unjoinedParent: ""
    property string sourceLabel: ""

    signal refreshRequested()
    signal openContainer(string id)
    signal saveEvent(string json)
    signal deleteEvent(string id, string when)
    signal rsvp(string id, string status, string optionsJson)
    signal lockOption(string id, int index, string when)
    // 0.8.35: connect the cards of events from before 0.8.33 — [{id, when}] + the room to look in first.
    signal linkLegacyCards(string json, string hintRoom)
    signal saveGame(string json)
    signal deleteGame(string slug)
    signal setReminders(bool on)
    signal inviteMissing()
    signal pickGame(string id)   // 9.0 Game Reel: "Pick tonight's game" on a session without one

    FallbackTheme { id: fallbackTheme }
    readonly property var theme: app ? app : fallbackTheme
    readonly property int dur: Motion.dur   // 11.0 (GPU.md R3): was `theme.dur ? theme.dur : 120` — Reduce motion's 0 became 120

    // NEVER exactly "Tin Hat": the boot smoke asserts the main window by that exact title.
    title: "Calendar" + (containerLabel.length > 0 ? " · " + containerLabel : "") + " — Tin Hat"   // 10.1 (TH-SHELL-19)
    width: 1140
    height: 780
    minimumWidth: 900
    minimumHeight: 600
    color: Th.calendarBg

    // ---- view state -------------------------------------------------------------------
    property int viewYear: new Date().getFullYear()
    property int viewMonth: new Date().getMonth()   // 0-11, like JS and MonthGrid
    property string tab: "month"                    // month | upcoming | games
    property string selectedDay: dayKey(new Date())
    property string selectedId: ""
    property bool editing: false
    property bool gameEditing: false
    property string gameEditSlug: ""

    // ---- editor draft (separate props so bindings update) ----------------------------
    property string dId: ""
    property string dTitle: ""
    property string dGame: ""
    property string dKind: "session"
    property string dDate: dayKey(new Date())
    property int dTime: 19 * 60                     // minutes since midnight
    property int dDuration: 180                     // minutes, 0 = open-ended
    property string dNotes: ""
    property string dRules: ""
    property bool dFreePlay: true
    property bool dJoin: false
    property bool dAnnounce: true
    property var dOptions: []                       // [{date: key, time: minutes}]
    property int dChosen: -1
    property string dEndDate: dayKey(new Date())    // 0.8.27: last day of a "main game" range

    // ---- helpers ----------------------------------------------------------------------
    function pad2(n) { return (n < 10 ? "0" : "") + n }
    function dayKey(d) { return d.getFullYear() + "-" + pad2(d.getMonth() + 1) + "-" + pad2(d.getDate()) }
    function keyToDate(key, minutes) {
        var p = String(key).split("-")
        var d = new Date(Number(p[0]), Number(p[1]) - 1, Number(p[2]), 0, 0, 0, 0)
        if (minutes !== undefined) d.setMinutes(minutes)
        return d
    }
    function validKey(key) {
        var p = String(key).split("-")
        if (p.length !== 3) return false
        var d = keyToDate(key)
        return !isNaN(d.getTime()) && d.getMonth() === Number(p[1]) - 1
    }
    function fmtTime(ms) { return Qt.formatTime(new Date(ms), "h:mm AP") }
    function fmtDay(ms) { return Qt.formatDate(new Date(ms), "ddd, MMM d") }
    function fmtDayLong(ms) { return Qt.formatDate(new Date(ms), "dddd, MMMM d") }
    property bool busyLong: false
    Timer { interval: 700; running: cal.busy; onTriggered: cal.busyLong = true }
    Connections { target: cal; function onBusyChanged() { if (!cal.busy) cal.busyLong = false } }
    // 10.1 (TH-CAL-07): the zone of the EVENT's moment (EDT in July even when planned in March), not of now.
    function tz(ms) { return Qt.formatTime(new Date((ms !== undefined && ms > 0) ? ms : Date.now()), "t") }
    function startOfDay(d) { return new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime() }
    function relDay(ms) {
        var diff = Math.round((startOfDay(new Date(ms)) - startOfDay(new Date())) / 86400000)
        if (diff === 0) return "Today"
        if (diff === 1) return "Tomorrow"
        if (diff === -1) return "Yesterday"
        if (diff > 1 && diff < 7) return "In " + diff + " days"
        if (diff < -1 && diff > -7) return (-diff) + " days ago"
        return ""
    }
    function whenLine(ev) {
        if (!ev || !ev.effStart) return "Time to be decided"
        if (ev.kind === "focus") return fmtDay(ev.effStart) + " – " + fmtDay(ev.end || ev.effStart)
        var s = ev.effStart, e = ev.end || 0
        var line = fmtDay(s) + " · " + fmtTime(s)
        if (e > s) line += " – " + fmtTime(e)
        var r = relDay(s)
        return r.length > 0 ? (r + " · " + line) : line
    }
    function kindLabel(k) {
        switch (k) {
        case "freeplay": return "Free play"
        case "milestone": return "Scheduled progression"
        case "proposal": return "Proposal"
        case "cooloff": return "Cool-off game"
        case "focus": return "Main game"
        default: return "Group session"
        }
    }
    function kindHint(k) {
        switch (k) {
        case "freeplay": return "Play whenever — no need to be there"
        case "milestone": return "A boss run, a story beat — join if you can"
        case "proposal": return "Offer a few times, everyone votes, you lock one in"
        case "cooloff": return "A lighter game between the main ones"
        case "focus": return "The game we're on for a while — colours those days on the calendar"
        default: return "The squad plays together at a set time"
        }
    }
    function kindColor(k) {
        switch (k) {
        case "freeplay": return Th.calendarKindFreeplayFill
        case "milestone": return Th.calendarKindMilestoneFill
        case "proposal": return Th.calendarKindProposalFill
        case "cooloff": return Th.calendarKindCooloffFill
        case "focus": return Th.calendarKindFocusFill
        default: return Th.calendarKindSessionFill
        }
    }
    function chipColor(ev) { return (ev.gameColor && ev.gameColor.length > 0) ? ev.gameColor : kindColor(ev.kind) }
    // Washes of CONTENT colours at fixed alphas (a game's colour, a kind's or an RSVP fill — P1c decision 9: the tone is
    // content or already a token; no wash tokens). Theme colours never go through here.
    function tint(c, a) { var col = Qt.color(c); return Qt.rgba(col.r, col.g, col.b, a) }   // theme-lint: ok content washes
    function myStatus(ev) { return (ev && ev.my) ? String(ev.my.status || "") : "" }
    // 0.8.33: the chat card's "when" line for an EXISTING event (the card is edited to match).
    // `chosenIndex` overrides for a lock/unlock (index < 0 = back to voting).
    function whenTextFor(ev, chosenIndex) {
        if (!ev) return ""
        var opts = ev.options || []
        if (ev.kind === "focus") return fmtDay(ev.effStart || ev.start) + " – " + fmtDay(ev.effEnd || ev.end)
        var start = ev.start || 0, end = ev.end || 0
        if (chosenIndex !== undefined) {
            if (chosenIndex >= 0 && opts[chosenIndex]) { start = opts[chosenIndex].start; end = opts[chosenIndex].end || 0 }
            else if (chosenIndex < 0) { start = 0; end = 0 }
        }
        if (start > 0) return fmtDay(start) + " · " + fmtTime(start) + (end > start ? " – " + fmtTime(end) : "") + " " + tz(start)
        return "vote for a time: " + opts.map(function (o) { return fmtDay(o.start) + " " + fmtTime(o.start) }).join(" / ") + " " + tz()
    }
    function eventById(id) {
        var evs = cal.events || []
        for (var i = 0; i < evs.length; i++) if (evs[i].id === id) return evs[i]
        return null
    }
    function myOptions(ev) { return (ev && ev.my && ev.my.options) ? ev.my.options : [] }
    function gameFor(ev) {
        if (!ev || !ev.game) return null
        var g = String(ev.game).toLowerCase()
        for (var i = 0; i < games.length; ++i)
            if (String(games[i].name).toLowerCase() === g) return games[i]
        return null
    }

    // 0.8.27: "main game" ranges are a backdrop (tinted days), never day chips.
    readonly property var focusRanges: (cal.events || []).filter(function (ev) { return ev.kind === "focus" && ev.effStart && !ev.canceled })
    function focusOn(key) {
        var d0 = keyToDate(key).getTime(), d1 = d0 + 86400000 - 1
        for (var i = 0; i < focusRanges.length; ++i) {
            var f = focusRanges[i]
            if (f.effStart <= d1 && (f.end || f.effStart) >= d0) return f
        }
        return null
    }
    readonly property var byDay: {
        var m = {}
        var list = cal.events || []
        for (var i = 0; i < list.length; ++i) {
            var ev = list[i]
            if (!ev.effStart || ev.kind === "focus") continue
            var k = dayKey(new Date(ev.effStart))
            if (!m[k]) m[k] = []
            m[k].push(ev)
        }
        return m
    }
    readonly property var undated: (cal.events || []).filter(function (ev) { return !ev.effStart })
    readonly property var selected: {
        if (selectedId.length === 0) return null
        for (var i = 0; i < events.length; ++i) if (events[i].id === selectedId) return events[i]
        return null
    }
    readonly property var dayEvents: byDay[selectedDay] || []
    readonly property var upcoming: {
        var t0 = startOfDay(new Date())
        var out = (cal.events || []).filter(function (ev) {
            return ev.effStart && (ev.effEnd || ev.effStart) >= t0 && !ev.canceled && ev.kind !== "focus"
        })
        return out.concat(undated)
    }
    readonly property var upcomingFocus: {
        var t0 = startOfDay(new Date())
        return focusRanges.filter(function (f) { return (f.end || f.effStart) >= t0 })
    }
    readonly property var pastEvents: {
        var t0 = startOfDay(new Date())
        return (cal.events || []).filter(function (ev) { return ev.effStart && (ev.effEnd || ev.effStart) < t0 }).reverse()
    }
    readonly property var timeSlots: {
        var a = []
        for (var m = 0; m < 24 * 60; m += 15) a.push(Qt.formatTime(new Date(2000, 0, 1, Math.floor(m / 60), m % 60), "h:mm AP"))
        return a
    }
    readonly property var durations: [
        { label: "30 min", m: 30 }, { label: "1 hour", m: 60 }, { label: "1½ hours", m: 90 },
        { label: "2 hours", m: 120 }, { label: "3 hours", m: 180 }, { label: "4 hours", m: 240 },
        { label: "5 hours", m: 300 }, { label: "Open-ended", m: 0 }
    ]
    function durationIndex(mins) {
        for (var i = 0; i < durations.length; ++i) if (durations[i].m === mins) return i
        return 4
    }
    readonly property var kinds: [
        { k: "session", label: "Group session" }, { k: "milestone", label: "Progression" },
        { k: "freeplay", label: "Free play" }, { k: "proposal", label: "Proposal" }, { k: "cooloff", label: "Cool-off" },
        { k: "focus", label: "Main game" }
    ]
    readonly property var gameNames: (cal.games || []).map(function (g) { return g.name })
    readonly property var colorChoices: ["#7cc4ff", "#ffb347", "#8ee08e", "#ff7c9c", "#c39bff", "#ffd75e", "#5ee0d0", "#ff9e6b"]

    // ---- actions -----------------------------------------------------------------------
    function openOrRaise() {
        if (cal.visible) { cal.raise(); cal.requestActivate() }
        else { cal.show(); cal.raise(); cal.requestActivate() }
        cal.refreshRequested()
    }
    function goToday() {
        var d = new Date()
        viewYear = d.getFullYear(); viewMonth = d.getMonth(); selectedDay = dayKey(d)
    }
    function shiftMonth(delta) {
        var m = viewMonth + delta, y = viewYear
        while (m < 0) { m += 12; y-- }
        while (m > 11) { m -= 12; y++ }
        viewMonth = m; viewYear = y
    }
    function focusEvent(id) {
        selectedId = id; editing = false; tab = "month"
        var ev = selected
        if (ev && ev.effStart) {
            var d = new Date(ev.effStart)
            viewYear = d.getFullYear(); viewMonth = d.getMonth(); selectedDay = dayKey(d)
        }
    }
    function startNew(key) {
        dId = ""; dTitle = ""; dGame = ""; dKind = "session"
        dDate = (key && key.length > 0) ? key : selectedDay
        dTime = 19 * 60; dDuration = 180; dNotes = ""; dRules = ""
        dFreePlay = true; dJoin = false; dAnnounce = roomId.length > 0
        dOptions = []; dChosen = -1; dEndDate = dDate
        selectedId = ""; editing = true; tab = "month"
        syncGameCombo()
        titleField.forceActiveFocus()
        _draftBase = draftKey()
    }
    // 10.1 (TH-CAL-14): a half-written event is not thrown away by a stray Escape or Cancel.
    property string _draftBase: ""
    function draftKey() {
        return JSON.stringify([dId, dTitle, dGame, dKind, dDate, dTime, dDuration, dNotes, dRules, dFreePlay, dJoin, dOptions, dChosen, dEndDate])
    }
    function closeEditor(force) {
        if (!force && editing && draftKey() !== _draftBase) { discardConfirm.open(); return }
        editing = false
    }
    function startEdit(ev) {
        if (!ev) return
        dId = ev.id; dTitle = ev.title; dGame = ev.game || ""; dKind = ev.kind || "session"
        var base = ev.start || ev.effStart || 0
        if (base > 0) {
            var d = new Date(base)
            dDate = dayKey(d); dTime = d.getHours() * 60 + Math.floor(d.getMinutes() / 15) * 15
        } else { dDate = selectedDay; dTime = 19 * 60 }
        var e = ev.end || 0
        dDuration = (e > base && base > 0) ? Math.round((e - base) / 60000) : 0
        // 10.1 (TH-CAL-13): a duration that is not one of the presets is KEPT (opening someone's
        // event to fix a typo used to silently move its end time to +3 h).
        dNotes = ev.notes || ""; dRules = ev.rules || ""
        dFreePlay = !!ev.freePlay; dJoin = !!ev.joinIfYouCan; dAnnounce = false
        var opts = []
        for (var i = 0; i < (ev.options || []).length; ++i) {
            var od = new Date(ev.options[i].start)
            opts.push({ date: dayKey(od), time: od.getHours() * 60 + Math.floor(od.getMinutes() / 15) * 15,
                        voted: (ev.options[i].votes || []).length })   // 10.1 (TH-CAL-04)
        }
        dOptions = opts; dChosen = (ev.chosen !== undefined) ? ev.chosen : -1
        dEndDate = (ev.end && ev.end > 0) ? dayKey(new Date(ev.end)) : dDate
        editing = true
        syncGameCombo()
        _draftBase = draftKey()
    }
    // 0.8.36: the game picker shows the draft's game and opens its list on that row (currentIndex
    // only when the game is one of the listed ones — setting -1 would clear a typed name).
    function syncGameCombo() {
        if (gameCombo.editText !== dGame) gameCombo.editText = dGame
        var gi = dGame.length > 0 ? gameCombo.find(dGame, Qt.MatchFixedString) : -1
        if (gi >= 0 && gameCombo.currentIndex !== gi) gameCombo.currentIndex = gi
    }
    function addOption() {
        if (!validKey(dDate)) return
        dOptions = dOptions.concat([{ date: dDate, time: dTime }])
    }
    function removeOption(i) {
        var a = dOptions.slice(); a.splice(i, 1); dOptions = a
        if (dChosen === i) dChosen = -1; else if (dChosen > i) dChosen--
    }
    function commitDraft() {
        if (dTitle.trim().length === 0) { titleField.forceActiveFocus(); return }
        if (!validKey(dDate) && dKind !== "proposal") { dateField.forceActiveFocus(); return }
        var isNew = dId.length === 0
        var start = 0, end = 0
        if (dKind === "focus") {
            if (!validKey(dEndDate)) { endDateField.forceActiveFocus(); return }
            start = keyToDate(dDate, 0).getTime()
            end = keyToDate(dEndDate, 0).getTime() + 86400000 - 1
            if (end < start) { endDateField.forceActiveFocus(); return }
        } else if (dKind !== "proposal") {
            start = keyToDate(dDate, dTime).getTime()
            end = dDuration > 0 ? start + dDuration * 60000 : 0
        }
        var opts = []
        for (var i = 0; i < dOptions.length; ++i) {
            var os = keyToDate(dOptions[i].date, dOptions[i].time).getTime()
            opts.push({ start: os, end: dDuration > 0 ? os + dDuration * 60000 : 0 })
        }
        if (dKind === "proposal" && dChosen >= 0 && opts[dChosen]) { start = opts[dChosen].start; end = opts[dChosen].end }
        var obj = {
            id: dId, title: dTitle.trim(), game: dGame.trim(), kind: dKind, start: start, end: end, allDay: false,
            notes: dNotes, rules: dRules, freePlay: dFreePlay, joinIfYouCan: dJoin, options: opts,
            chosen: dKind === "proposal" ? dChosen : -1
        }
        // The "when" line always rides along (0.8.33): a CREATE with announce posts the chat card,
        // every later save edits that card to match.
        if (dKind === "focus")
            obj.announceWhen = fmtDay(start) + " – " + fmtDay(end)
        else if (start > 0)
            obj.announceWhen = fmtDay(start) + " · " + fmtTime(start) + (end > start ? " – " + fmtTime(end) : "") + " " + tz(start)
        else
            obj.announceWhen = "vote for a time: " + opts.map(function (o) { return fmtDay(o.start) + " " + fmtTime(o.start) }).join(" / ") + " " + tz()
        // 0.8.35: the room this calendar was opened from is the first place to look for an old event's
        // card (announceHint); the box posts on CREATE, or a FRESH card for an existing event.
        if (dAnnounce && roomId.length > 0) obj.announceRoom = roomId
        if (roomId.length > 0) obj.announceHint = roomId
        cal.saveEvent(JSON.stringify(obj))
        editing = false
    }
    // 10.1 (TH-CAL-05): call an event off (or put it back on) WITHOUT deleting it — the CANCELLED
    // chrome in the grid, the detail pane and the chat card was built for this and had no way to
    // ever light up; the only exit was the bin, which took everyone's RSVPs with it.
    function setCanceled(ev, on) {
        if (!ev || !ev.id) return
        var opts = (ev.options || []).map(function (o) { return { start: o.start || 0, end: o.end || 0 } })
        var obj = {
            id: ev.id, title: ev.title || "", game: ev.game || "", kind: ev.kind || "session",
            start: ev.start || 0, end: ev.end || 0, allDay: !!ev.allDay,
            notes: ev.notes || "", rules: ev.rules || "", freePlay: !!ev.freePlay, joinIfYouCan: !!ev.joinIfYouCan,
            options: opts, chosen: (ev.chosen !== undefined && ev.chosen !== null) ? ev.chosen : -1,
            canceled: on, announceWhen: whenTextFor(ev)
        }
        if (roomId.length > 0) obj.announceHint = roomId
        cal.saveEvent(JSON.stringify(obj))
    }
    function setRsvp(ev, status) {
        var s = (myStatus(ev) === status) ? "" : status
        cal.rsvp(ev.id, s, JSON.stringify(myOptions(ev)))
    }
    function toggleVote(ev, idx) {
        var arr = myOptions(ev).slice()
        var i = arr.indexOf(idx)
        if (i >= 0) arr.splice(i, 1); else arr.push(idx)
        cal.rsvp(ev.id, myStatus(ev), JSON.stringify(arr))
    }
    function startGameEdit(g) {
        gameEditSlug = g ? g.slug : ""
        gName.text = g ? g.name : ""
        gServer.text = g ? (g.server || "") : ""
        gRules.text = g ? (g.rules || "") : ""
        gColor.currentIndex = g ? Math.max(0, colorChoices.indexOf(String(g.color || "").toLowerCase())) : 0
        gameEditing = true
    }
    function commitGame() {
        if (gName.text.trim().length === 0) return
        cal.saveGame(JSON.stringify({ slug: gameEditSlug, name: gName.text.trim(), color: colorChoices[gColor.currentIndex],
                                      server: gServer.text.trim(), rules: gRules.text }))
        gameEditing = false
    }

    onEventsChanged: {
        if (selectedId.length > 0 && !selected) selectedId = ""
        connectLegacyCards()
    }
    // 0.8.35: once per container per session, hand every event without a card ref (Rust keeps
    // only mine) + its pre-formatted "when" line to the controller, which finds my earlier card,
    // edits it into a live one and records it — old cards connect without anyone editing them.
    property var _legacyLinked: ({})
    onContainerIdChanged: connectLegacyCards()   // events and container are mirrored by separate handlers
    function connectLegacyCards() {
        if (containerId.length === 0 || _legacyLinked[containerId]) return
        var evs = cal.events || [], list = []
        for (var i = 0; i < evs.length; i++) {
            var ev = evs[i]
            if (ev.hasCard || !ev.canEdit) continue
            list.push({ id: ev.id, when: whenTextFor(ev, ev.kind === "proposal" ? ev.chosen : undefined) })
        }
        if (list.length === 0) return
        _legacyLinked[containerId] = true
        cal.linkLegacyCards(JSON.stringify(list), roomId)
    }
    onVisibleChanged: if (visible) refreshRequested()
    Timer { interval: 30000; repeat: true; running: cal.visible; onTriggered: cal.refreshRequested() }
    Shortcut {
        sequence: "Escape"
        onActivated: {
            if (cal.editing) cal.closeEditor(false)
            else if (cal.gameEditing) cal.gameEditing = false
            else if (cal.selectedId.length > 0) cal.selectedId = ""
            else cal.close()
        }
    }

    ConfirmDialog {
        id: deleteConfirm
        app: cal.app
        property string targetId: ""
        dialogTitle: "Remove this event?"
        message: "It disappears from everyone's calendar. RSVPs go with it."
        confirmText: "Remove"
        destructive: true
        onConfirmed: { cal.deleteEvent(targetId, cal.whenTextFor(cal.eventById(targetId))); cal.selectedId = "" }
    }
    ConfirmDialog {
        id: callOffConfirm
        app: cal.app
        property var targetEv: null
        dialogTitle: "Call it off?"
        message: "It stays on the calendar, struck through, with everyone's RSVPs kept — so it can go back on. The chat card will say CANCELLED."
        confirmText: "Call it off"
        destructive: true
        onConfirmed: cal.setCanceled(targetEv, true)
    }
    ConfirmDialog {
        id: discardConfirm
        app: cal.app
        dialogTitle: "Discard this event?"
        message: "You have unsaved changes. Keep editing, or throw them away?"
        confirmText: "Discard"
        cancelText: "Keep editing"
        destructive: true
        onConfirmed: cal.closeEditor(true)
    }
    ConfirmDialog {
        id: deleteGameConfirm
        app: cal.app
        property string targetSlug: ""
        dialogTitle: "Remove this game?"
        message: "Its rules go too. Events that mention it stay."
        confirmText: "Remove"
        destructive: true
        onConfirmed: cal.deleteGame(targetSlug)
    }

    // =====================================================================================
    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        // ---- header ----------------------------------------------------------------------
        Rectangle {
            Layout.fillWidth: true
            implicitHeight: 60
            gradient: Gradient {
                GradientStop { position: 0.0; color: Th.calendarHeaderHighlight }
                GradientStop { position: 1.0; color: Th.calendarHeaderBg }
            }
            RowLayout {
                anchors.fill: parent
                anchors.leftMargin: Kirigami.Units.largeSpacing
                anchors.rightMargin: Kirigami.Units.largeSpacing
                spacing: Kirigami.Units.smallSpacing

                ThIcon {
                    app: cal.app; name: "view-calendar-symbolic"; color: Th.calendarHeaderIcon
                    width: 26; height: 26; active: true
                }
                Kirigami.Heading {
                    textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                    level: 2
                    visible: cal.containers.length <= 1   // 10.1 (TH-VIS-19): the picker already names the room
                    text: cal.containerLabel.length > 0 ? cal.containerLabel : "Calendar"
                    color: Th.calendarFg
                    elide: Text.ElideRight
                    Layout.maximumWidth: 320
                }
                ThComboBox {
                    id: spacePicker
                    visible: cal.containers.length > 1
                    Layout.preferredWidth: 220
                    model: cal.containers.map(function (c) { return c.label })
                    currentIndex: {
                        for (var i = 0; i < cal.containers.length; ++i) if (cal.containers[i].id === cal.containerId) return i
                        return -1
                    }
                    onActivated: (i) => { if (cal.containers[i]) cal.openContainer(cal.containers[i].id) }
                    Controls.ToolTip.text: "Which space's calendar to show"
                    Controls.ToolTip.visible: hovered
                }
                Item { Layout.fillWidth: true }

                // tabs
                Row {
                    spacing: 2
                    Repeater {
                        model: [ { k: "month", l: "Month", i: "view-calendar-month-symbolic" },
                                 { k: "upcoming", l: "Upcoming", i: "view-calendar-list-symbolic" },
                                 { k: "games", l: "Games & rules", i: "applications-games-symbolic" } ]
                        delegate: Controls.AbstractButton {
                            id: tabBtn
                            required property var modelData
                            readonly property bool on: cal.tab === modelData.k
                            hoverEnabled: true
                            implicitHeight: 34
                            implicitWidth: tabRow.implicitWidth + 24
                            onClicked: cal.tab = modelData.k
                            background: Rectangle {
                                radius: 17
                                color: tabBtn.on ? Th.calendarTabBgActive
                                     : (tabBtn.hovered ? Th.calendarTabBgHover : "transparent")
                                border.width: tabBtn.on ? 1 : 0
                                border.color: Th.calendarTabBorderActive
                            }
                            contentItem: Item {
                                Row {
                                    id: tabRow
                                    spacing: 6
                                    anchors.centerIn: parent
                                    ThIcon {
                                        anchors.verticalCenter: parent.verticalCenter
                                        app: cal.app; name: tabBtn.modelData.i
                                        width: 16; height: 16
                                        color: tabBtn.on ? Th.calendarTabIconActive : Th.calendarTabIcon
                                        hovered: tabBtn.hovered; active: tabBtn.on
                                    }
                                    Controls.Label {
                                        textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                        anchors.verticalCenter: parent.verticalCenter
                                        text: tabBtn.modelData.l
                                        color: tabBtn.on ? Th.calendarTabFgActive : Th.calendarTabFg
                                        font.bold: tabBtn.on
                                    }
                                }
                            }
                        }
                    }
                }
                Item { Layout.fillWidth: true }

                Controls.ToolButton {
                    icon.name: cal.reminders ? "notifications" : "notifications-disabled-symbolic"
                    checkable: true
                    checked: cal.reminders
                    onToggled: cal.setReminders(checked)
                    Controls.ToolTip.text: cal.reminders ? "Reminders on — 30 min before events you're going to"
                                                         : "Reminders off"
                    Controls.ToolTip.visible: hovered
                }
                // 10.1 (TH-CAL-20): only a refresh that takes a while shows a spinner — the background
                // ones (every 30–60 s) finish in a blink and used to flash it twice a minute.
                ThBusyIndicator { running: cal.busy && cal.busyLong; visible: running; size: 22; color: Th.loaderSpinnerFg }   // 11.0: still
                GlowButton {
                    app: cal.app
                    text: "New event"
                    icon.name: "appointment-new-symbolic"
                    enabled: cal.containerId.length > 0
                    onClicked: cal.startNew(cal.selectedDay)
                }
            }
        }
        Kirigami.Separator { Layout.fillWidth: true; opacity: 0.6 }

        // 0.8.27 — this room belongs to a space I am not in: I only see the room's own calendar.
        Rectangle {
            Layout.fillWidth: true
            visible: cal.unjoinedParent.length > 0 && cal.containerId === cal.roomId
            implicitHeight: visible ? unjoinedLbl.implicitHeight + 14 : 0
            color: Th.calendarBannerUnjoinedBg
            Controls.Label {
                id: unjoinedLbl
                anchors.centerIn: parent
                width: parent.width - 24
                wrapMode: Text.WordWrap
                horizontalAlignment: Text.AlignHCenter
                text: "This room belongs to a space you haven't joined, so this is the room's own calendar. The squad calendar lives in the space — ask an admin to invite you (right-click the space → Invite people)."
                color: Th.calendarBannerFg
                font.pointSize: Kirigami.Theme.smallFont.pointSize
            }
        }
        // 0.8.27 — people in the room I opened from who are not in this space (they can't see this).
        Rectangle {
            Layout.fillWidth: true
            visible: cal.missing.length > 0 && cal.containerId !== cal.roomId && cal.containerId.length > 0
            implicitHeight: visible ? missingRow.implicitHeight + 14 : 0
            color: Th.calendarBannerMissingBg
            RowLayout {
                id: missingRow
                anchors.fill: parent
                anchors.leftMargin: 12
                anchors.rightMargin: 12
                spacing: 10
                Controls.Label {
                    textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                    Layout.fillWidth: true
                    wrapMode: Text.WordWrap
                    text: cal.missing.length + (cal.missing.length === 1 ? " person" : " people") + " in " + cal.sourceLabel + (cal.missing.length === 1 ? " isn't" : " aren't") + " in " + cal.containerLabel
                          + " yet, so they can't see this calendar: " + cal.missing.map(function (p) { return p.name }).join(", ")
                          + (cal.canInvite ? "" : ". A space admin can invite them.")
                    color: Th.calendarBannerFg
                    font.pointSize: Kirigami.Theme.smallFont.pointSize
                }
                ThButton {
                    app: cal.app;
                    visible: cal.canInvite
                    text: "Invite them to " + cal.containerLabel
                    icon.name: "list-add-user"
                    onClicked: cal.inviteMissing()
                }
            }
        }
        // permission hint — a member whose space admin never opened the calendar
        Rectangle {
            Layout.fillWidth: true
            visible: !cal.canEdit && cal.containerId.length > 0 && !cal.busy
            implicitHeight: visible ? 30 : 0
            color: Th.calendarBannerHintBg
            Controls.Label {
                anchors.centerIn: parent
                text: "Scheduling here unlocks the first time a space admin opens this space in Tin Hat 10 — ask an admin to open it once."
                color: Th.calendarBannerFg
                font.pointSize: Kirigami.Theme.smallFont.pointSize
            }
        }
        Rectangle {
            Layout.fillWidth: true
            visible: cal.errorText.length > 0
            implicitHeight: visible ? 30 : 0
            color: Th.calendarBannerErrorBg
            Controls.Label { textFormat: Text.PlainText; anchors.centerIn: parent; text: cal.errorText; color: Th.calendarBannerFg; font.pointSize: Kirigami.Theme.smallFont.pointSize }
        }

        // ---- body -------------------------------------------------------------------------
        RowLayout {
            Layout.fillWidth: true
            Layout.fillHeight: true
            Layout.margins: Kirigami.Units.largeSpacing
            spacing: Kirigami.Units.largeSpacing

            StackLayout {
                Layout.fillWidth: true
                Layout.fillHeight: true
                currentIndex: cal.tab === "month" ? 0 : (cal.tab === "upcoming" ? 1 : 2)

                // ===== MONTH =====
                ColumnLayout {
                    spacing: Kirigami.Units.smallSpacing
                    RowLayout {
                        Layout.fillWidth: true
                        Controls.ToolButton { icon.name: "go-previous-symbolic"; onClicked: cal.shiftMonth(-1); Controls.ToolTip.text: "Previous month"; Controls.ToolTip.visible: hovered; Accessible.name: "Previous month" }
                        Kirigami.Heading {
                            textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                            level: 3
                            text: Qt.formatDate(new Date(cal.viewYear, cal.viewMonth, 1), "MMMM yyyy")
                            color: Th.calendarFg
                            Layout.preferredWidth: 200
                            horizontalAlignment: Text.AlignHCenter
                        }
                        Controls.ToolButton { icon.name: "go-next-symbolic"; onClicked: cal.shiftMonth(1); Controls.ToolTip.text: "Next month"; Controls.ToolTip.visible: hovered; Accessible.name: "Next month" }
                        Controls.ToolButton { text: "Today"; onClicked: cal.goToday() }
                        Item { Layout.fillWidth: true }
                        // 12.0: the two hints give way (elide, like the web's) instead of holding the row at their full
                        // width — a fixed-width label made the month column ~580 px wide at least, so at the window's
                        // 900 px minimum the whole window (header, grid, day panel) ran ~100 px off its right edge.
                        // At most their text width, so a wide window draws them exactly as before.
                        Controls.Label {
                            textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                            visible: cal.undated.length > 0
                            Layout.fillWidth: true
                            Layout.preferredWidth: Math.ceil(implicitWidth)   // whole px: a fractional width elided the last letters
                            Layout.maximumWidth: Math.ceil(implicitWidth)
                            elide: Text.ElideRight
                            text: cal.undated.length + (cal.undated.length === 1 ? " proposal" : " proposals") + " awaiting a time — see Upcoming"
                            color: Th.calendarMetaFg
                            font.pointSize: Kirigami.Theme.smallFont.pointSize
                        }
                        Controls.Label {
                            visible: cal.canEdit   // 10.1 (TH-CAL-21): don't invite an action that does nothing
                            Layout.fillWidth: true
                            Layout.preferredWidth: Math.ceil(implicitWidth)   // whole px: a fractional width elided the last letters
                            Layout.maximumWidth: Math.ceil(implicitWidth)
                            elide: Text.ElideRight
                            text: "Double-click a day to schedule"
                            color: Th.calendarMetaFg
                            font.pointSize: Kirigami.Theme.smallFont.pointSize
                        }
                    }
                    Controls.DayOfWeekRow {
                        Layout.fillWidth: true
                        locale: grid.locale
                        delegate: Controls.Label {
                            textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                            required property var model
                            text: model.shortName
                            horizontalAlignment: Text.AlignHCenter
                            color: Th.calendarWeekdayFg
                            font.bold: true
                        }
                    }
                    Controls.MonthGrid {
                        id: grid
                        Layout.fillWidth: true
                        Layout.fillHeight: true
                        month: cal.viewMonth
                        year: cal.viewYear
                        locale: Qt.locale()
                        delegate: Item {
                            id: cell
                            clip: true   // 10.1 (TH-CAL-02): chips never draw over the row below
                            required property var model
                            readonly property bool inMonth: model.month === grid.month
                            // How many chips fit: the cell minus its margins, the day number and the "+N" line.
                            readonly property int chipCap: Math.max(1, Math.floor((cell.height - 14 - 20 - 16) / 21))
                            // 0.8.32: NEVER key a cell off model.date — on Windows that JS Date is
                            // UTC midnight (Linux: local midnight), so local getters named the previous day
                            // west of UTC and every event drew one cell late ("tomorrow"). The year/month/day
                            // roles are plain integers; a Date built from them is the cell's local midnight.
                            readonly property var cellDate: new Date(model.year, model.month, model.day)
                            readonly property string key: cal.dayKey(cellDate)
                            readonly property var evs: cal.byDay[key] || []
                            readonly property bool sel: cal.selectedDay === key
                            readonly property bool draftDay: cal.editing && cal.dDate === key
                            readonly property var mainGame: cal.focusOn(key)       // 0.8.27 main-game backdrop (not "focus": FINAL on Item)
                            readonly property bool mainGameStart: mainGame !== null && (cellDate.getDay() === grid.locale.firstDayOfWeek || cal.dayKey(new Date(mainGame.effStart)) === key || model.day === 1)   // 10.1 (TH-CAL-19): the locale's week start, not Monday
                            Rectangle {
                                anchors.fill: parent
                                anchors.margins: 2
                                radius: 10
                                // 10.1 (TH-CAL-17 / TH-VIS-20): hover blends over selection; out-of-month days keep a cell;
                                // today is a filled day-number pill (below) so selection alone owns the outline.
                                color: cell.sel ? (cellMa.containsMouse ? Th.calendarDaySelectedBgHover : Th.calendarDayBgSelected)
                                     : (cellMa.containsMouse ? Th.calendarDayBgHover
                                        : (cell.mainGame ? cal.tint(cal.chipColor(cell.mainGame), cell.inMonth ? 0.13 : 0.06)
                                           : (cell.inMonth ? Th.calendarDayBg : Th.calendarDayOutsideBg)))
                                border.width: cell.draftDay ? 2 : (cell.sel ? 2 : 0)
                                border.color: cell.draftDay ? Th.calendarDayDraftBorder : Th.calendarDayBorderSelected
                                Behavior on color { ColorAnimation { duration: cal.dur } }
                            }
                            Column {
                                anchors.fill: parent
                                anchors.margins: 7
                                spacing: 3
                                Item {
                                    width: dayNum.implicitWidth + (model.today ? 12 : 0)
                                    height: dayNum.implicitHeight + (model.today ? 2 : 0)
                                    Rectangle { anchors.fill: parent; radius: height / 2; color: Th.calendarDayTodayBg; visible: model.today }
                                    Controls.Label {
                                        textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                        id: dayNum
                                        anchors.centerIn: parent
                                        text: model.day
                                        color: model.today ? Th.calendarDayTodayFg : (cell.inMonth ? Th.calendarDayNumberFg : Th.calendarDayOutsideFg)
                                        opacity: cell.inMonth ? 1 : 0.5
                                        font.bold: model.today || cell.sel
                                    }
                                }
                                Repeater {
                                    model: cell.evs.slice(0, cell.chipCap)
                                    delegate: Rectangle {
                                        required property var modelData
                                        width: parent.width
                                        height: 18
                                        radius: 5
                                        color: cal.tint(cal.chipColor(modelData), modelData.canceled ? 0.10 : 0.26)
                                        border.width: cal.selectedId === modelData.id ? 1 : 0
                                        border.color: cal.chipColor(modelData)
                                        Row {
                                            anchors.fill: parent
                                            anchors.leftMargin: 5
                                            anchors.rightMargin: 4
                                            spacing: 4
                                            Rectangle {
                                                width: 6; height: 6; radius: 3
                                                anchors.verticalCenter: parent.verticalCenter
                                                color: cal.chipColor(modelData)
                                            }
                                            // 10.1 (TH-CAL-02): the GAME is what survives; the time sits dim on the right
                                            // (and drops off a narrow cell) — "7:00 PM Val…" said nothing.
                                            Controls.Label {
                                                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                                width: parent.width - 10 - (timeLbl.visible ? timeLbl.implicitWidth + 4 : 0)
                                                anchors.verticalCenter: parent.verticalCenter
                                                text: (modelData.game && modelData.game.length > 0 ? modelData.game : modelData.title)
                                                color: Th.calendarFg
                                                font.pointSize: Kirigami.Theme.smallFont.pointSize
                                                font.strikeout: modelData.canceled
                                                elide: Text.ElideRight
                                            }
                                            Controls.Label {
                                                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                                id: timeLbl
                                                visible: !!modelData.effStart && cell.width > 96
                                                anchors.verticalCenter: parent.verticalCenter
                                                text: modelData.effStart ? cal.fmtTime(modelData.effStart).replace(":00", "").replace(" ", "").toLowerCase() : ""
                                                color: Th.calendarChipTimeFg
                                                font.pointSize: Kirigami.Theme.smallFont.pointSize * 0.9
                                            }
                                        }
                                        MouseArea {
                                            anchors.fill: parent
                                            onClicked: { cal.selectedDay = cell.key; cal.selectedId = modelData.id; cal.editing = false }
                                        }
                                    }
                                }
                                Controls.Label {
                                    textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                    visible: cell.evs.length > cell.chipCap
                                    text: "+" + (cell.evs.length - cell.chipCap) + " more" + (cellMa.containsMouse ? " — click the day" : "")
                                    color: cellMa.containsMouse ? Th.calendarDayMoreFgHover : Th.calendarDayMoreFg
                                    font.pointSize: Kirigami.Theme.smallFont.pointSize
                                }
                            }
                            // main-game label: on the range's first day and at the start of each week
                            Controls.Label {
                                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                visible: cell.mainGameStart && cell.inMonth
                                anchors.left: parent.left; anchors.bottom: parent.bottom
                                anchors.leftMargin: 8; anchors.bottomMargin: 6
                                text: cell.mainGame ? ((cell.mainGame.game && cell.mainGame.game.length > 0) ? cell.mainGame.game : cell.mainGame.title) : ""
                                color: cell.mainGame ? cal.chipColor(cell.mainGame) : "transparent"
                                font.pointSize: Kirigami.Theme.smallFont.pointSize * 0.9
                                font.bold: true
                                opacity: 0.9
                                elide: Text.ElideRight
                                width: parent.width - 16
                            }
                            MouseArea {
                                id: cellMa
                                cursorShape: Qt.PointingHandCursor   // 10.1 (TH-CAL-17): the grid reads as clickable
                                anchors.fill: parent
                                hoverEnabled: true
                                z: -1
                                onClicked: {
                                    cal.selectedDay = cell.key
                                    if (cal.editing) cal.dDate = cell.key; else cal.selectedId = ""
                                }
                                onDoubleClicked: if (!cal.editing && cal.canEdit) cal.startNew(cell.key)
                            }
                        }
                    }
                }

                // ===== UPCOMING =====
                ColumnLayout {
                    spacing: Kirigami.Units.smallSpacing
                    Kirigami.Heading { level: 3; text: "Coming up"; color: Th.calendarFg }
                    Controls.ScrollView {
                        Layout.fillWidth: true
                        Layout.fillHeight: true
                        clip: true
                        ColumnLayout {
                            width: parent.width
                            spacing: 6
                            Repeater {
                                model: cal.upcomingFocus
                                delegate: Rectangle {
                                    required property var modelData
                                    Layout.fillWidth: true
                                    implicitHeight: 40
                                    radius: 10
                                    color: cal.tint(cal.chipColor(modelData), 0.18)
                                    border.width: 1; border.color: cal.tint(cal.chipColor(modelData), 0.6)
                                    RowLayout {
                                        anchors.fill: parent; anchors.leftMargin: 12; anchors.rightMargin: 12; spacing: 10
                                        Controls.Label { text: "Main game"; color: cal.chipColor(modelData); font.bold: true }
                                        Controls.Label { textFormat: Text.PlainText; Layout.fillWidth: true; text: ((modelData.game && modelData.game.length > 0) ? modelData.game : modelData.title) + "  ·  " + cal.whenLine(modelData); color: Th.calendarFg; elide: Text.ElideRight }
                                    }
                                    MouseArea { anchors.fill: parent; onClicked: { cal.selectedId = modelData.id; cal.editing = false } }
                                }
                            }
                            EmptyState {
                                visible: cal.upcoming.length === 0 && cal.upcomingFocus.length === 0
                                Layout.fillWidth: true
                                app: cal.app
                                iconName: "view-calendar-upcoming-events-symbolic"
                                title: "Nothing scheduled yet"
                                explanation: cal.canEdit ? "Hit New event — a game night, a boss run, or a proposal to find a time." : "Nothing on the calendar right now."
                            }
                            Repeater {
                                model: cal.upcoming
                                delegate: Rectangle {
                                    required property var modelData
                                    required property int index
                                    Layout.fillWidth: true
                                    implicitHeight: upRow.implicitHeight + 20
                                    radius: 10
                                    color: cal.selectedId === modelData.id ? Th.calendarUpcomingRowBgSelected
                                         : (upMa.containsMouse ? Th.calendarUpcomingRowBgHover : Th.calendarUpcomingRowBg)
                                    border.width: 1
                                    border.color: cal.selectedId === modelData.id ? Th.calendarUpcomingRowBorderSelected : Th.calendarUpcomingRowBorder
                                    RowLayout {
                                        id: upRow
                                        anchors.fill: parent
                                        anchors.margins: 10
                                        spacing: 12
                                        Rectangle { Layout.preferredWidth: 5; Layout.fillHeight: true; radius: 3; color: cal.chipColor(modelData) }
                                        ColumnLayout {
                                            Layout.preferredWidth: 130
                                            spacing: 0
                                            Controls.Label {
                                                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                                text: modelData.effStart ? (cal.relDay(modelData.effStart) || Qt.formatDate(new Date(modelData.effStart), "ddd")) : "Vote"
                                                color: Th.calendarEmphasisFg
                                                font.bold: true
                                            }
                                            Controls.Label {
                                                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                                text: modelData.effStart ? (cal.fmtDay(modelData.effStart) + "\n" + cal.fmtTime(modelData.effStart)) : "time to be decided"
                                                color: Th.calendarMetaFg
                                                font.pointSize: Kirigami.Theme.smallFont.pointSize
                                            }
                                        }
                                        ColumnLayout {
                                            Layout.fillWidth: true
                                            spacing: 2
                                            Controls.Label { textFormat: Text.PlainText; text: modelData.title; color: Th.calendarFg; font.bold: true; elide: Text.ElideRight; Layout.fillWidth: true }
                                            Controls.Label {
                                                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                                text: (modelData.game ? modelData.game + "  ·  " : "") + cal.kindLabel(modelData.kind)
                                                      + (modelData.freePlay && modelData.kind !== "freeplay" ? "  ·  free play allowed" : "")
                                                      + (modelData.joinIfYouCan ? "  ·  join if you can" : "")
                                                color: Th.calendarMetaFg
                                                font.pointSize: Kirigami.Theme.smallFont.pointSize
                                                elide: Text.ElideRight
                                                Layout.fillWidth: true
                                            }
                                        }
                                        Row {
                                            spacing: -8
                                            Repeater {
                                                model: (modelData.rsvp ? modelData.rsvp.yes : []).slice(0, 6)
                                                delegate: Avatar {
                                                    required property var modelData
                                                    app: cal.app; size: 26
                                                    name: modelData.name; seed: modelData.mxid; imageSource: modelData.avatar
                                                }
                                            }
                                        }
                                        Controls.Label {
                                            textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                            text: (modelData.rsvp ? modelData.rsvp.yes.length : 0) + " going"
                                                  + ((modelData.rsvp && modelData.rsvp.maybe.length > 0) ? " · " + modelData.rsvp.maybe.length + " maybe" : "")
                                            color: Th.calendarMetaFg
                                            font.pointSize: Kirigami.Theme.smallFont.pointSize
                                        }
                                        Rectangle {
                                            visible: cal.myStatus(modelData).length > 0
                                            radius: 9; implicitHeight: 18; implicitWidth: myLbl.implicitWidth + 12
                                            color: (cal.myStatus(modelData) === "yes" ? Th.calendarStatusYesBg : (cal.myStatus(modelData) === "maybe" ? Th.calendarStatusMaybeBg : Th.calendarStatusNoBg))
                                            Controls.Label {
                                                id: myLbl; anchors.centerIn: parent
                                                text: cal.myStatus(modelData) === "yes" ? "you're in" : (cal.myStatus(modelData) === "maybe" ? "maybe" : "can't")
                                                color: Th.calendarFg; font.pointSize: Kirigami.Theme.smallFont.pointSize * 0.9
                                            }
                                        }
                                    }
                                    MouseArea {
                                        id: upMa
                                        anchors.fill: parent
                                        hoverEnabled: true
                                        onClicked: { cal.selectedId = modelData.id; cal.editing = false }
                                    }
                                }
                            }
                            Kirigami.Heading { visible: cal.pastEvents.length > 0; level: 4; text: "Past"; color: Th.calendarMetaFg; Layout.topMargin: 12 }
                            Repeater {
                                model: cal.pastEvents.slice(0, 12)
                                delegate: Controls.ItemDelegate {
                                    required property var modelData
                                    Layout.fillWidth: true
                                    text: LinkGate.plain(cal.fmtDay(modelData.effStart) + "  ·  " + modelData.title + (modelData.game ? "  ·  " + modelData.game : ""))
                                    onClicked: { cal.selectedId = modelData.id; cal.editing = false }
                                }
                            }
                        }
                    }
                }

                // ===== GAMES & RULES =====
                ColumnLayout {
                    spacing: Kirigami.Units.smallSpacing
                    RowLayout {
                        Layout.fillWidth: true
                        Kirigami.Heading { level: 3; text: "Games & standing rules"; color: Th.calendarFg }
                        Item { Layout.fillWidth: true }
                        ThButton {
                            app: cal.app;
                            text: "Add game"
                            icon.name: "list-add-symbolic"
                            visible: cal.canEdit && !cal.gameEditing
                            onClicked: cal.startGameEdit(null)
                        }
                    }
                    Controls.Label {
                        Layout.fillWidth: true
                        wrapMode: Text.WordWrap
                        color: Th.calendarMetaFg
                        text: "House rules that apply every time a game comes up (free play, progression etiquette, join address). Sessions can add their own notes on top."
                    }
                    Rectangle {
                        visible: cal.gameEditing
                        Layout.fillWidth: true
                        implicitHeight: gEdit.implicitHeight + 24
                        radius: 10
                        color: Th.calendarGameFormBg
                        border.width: 1; border.color: Th.calendarGameFormBorder
                        ColumnLayout {
                            id: gEdit
                            anchors.fill: parent
                            anchors.margins: 12
                            spacing: 8
                            RowLayout {
                                Layout.fillWidth: true
                                ThTextField {
                                    id: gName; Layout.fillWidth: true; placeholderText: "Game name" }
                                ThComboBox {
                                    id: gColor
                                    Layout.preferredWidth: 120
                                    model: cal.colorChoices
                                    delegate: Controls.ItemDelegate {
                                        required property var modelData
                                        required property int index
                                        width: gColor.width
                                        contentItem: Row {
                                            spacing: 8
                                            Rectangle { width: 14; height: 14; radius: 7; color: modelData; anchors.verticalCenter: parent.verticalCenter }
                                            Controls.Label { textFormat: Text.PlainText; text: "Colour " + (index + 1); anchors.verticalCenter: parent.verticalCenter }
                                        }
                                    }
                                    contentItem: Row {
                                        spacing: 8
                                        leftPadding: 6
                                        Rectangle { width: 14; height: 14; radius: 7; color: cal.colorChoices[gColor.currentIndex]; anchors.verticalCenter: parent.verticalCenter }
                                        Controls.Label { text: "Colour"; anchors.verticalCenter: parent.verticalCenter }
                                    }
                                }
                            }
                            ThTextField {
                                id: gServer; Layout.fillWidth: true; placeholderText: "Join address (e.g. play.example.com:2456) — optional" }
                            ThTextArea {
                                id: gRules
                                Layout.fillWidth: true
                                Layout.preferredHeight: 120
                                placeholderText: "Standing rules — free play allowed? boss progression only as a group? what to bring…"
                                wrapMode: TextEdit.Wrap
                            }
                            RowLayout {
                                Item { Layout.fillWidth: true }
                                ThButton {
                                    app: cal.app; text: "Cancel"; onClicked: cal.gameEditing = false }
                                GlowButton { app: cal.app; text: "Save game"; onClicked: cal.commitGame() }
                            }
                        }
                    }
                    Controls.ScrollView {
                        Layout.fillWidth: true
                        Layout.fillHeight: true
                        clip: true
                        ColumnLayout {
                            width: parent.width
                            spacing: 8
                            EmptyState {
                                visible: cal.games.length === 0 && !cal.gameEditing
                                Layout.fillWidth: true
                                app: cal.app
                                iconName: "applications-games-symbolic"
                                title: "No games yet"
                                explanation: "Games are added here automatically the first time an event mentions them. Add rules when you have them."
                            }
                            Repeater {
                                model: cal.games
                                delegate: Rectangle {
                                    required property var modelData
                                    Layout.fillWidth: true
                                    implicitHeight: gCol.implicitHeight + 24
                                    radius: 10
                                    color: Th.calendarGameCardBg
                                    border.width: 1; border.color: Th.calendarGameCardBorder
                                    ColumnLayout {
                                        id: gCol
                                        anchors.fill: parent
                                        anchors.margins: 12
                                        spacing: 4
                                        RowLayout {
                                            Layout.fillWidth: true
                                            Rectangle { Layout.preferredWidth: 12; Layout.preferredHeight: 12; radius: 6; color: modelData.color }   // 10.1 (TH-CAL-09): a layout child sizes from Layout.*, so this drew 0×0
                                            Kirigami.Heading { textFormat: Text.PlainText; level: 4; text: modelData.name; color: Th.calendarFg }
                                            Controls.Label {
                                                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                                visible: !!(modelData.server && modelData.server.length > 0)
                                                text: modelData.server
                                                color: Th.calendarEmphasisFg
                                                font.family: Th.fontMono
                                            }
                                            Controls.ToolButton {
                                                visible: !!(modelData.server && modelData.server.length > 0)
                                                icon.name: "edit-copy-symbolic"
                                                Controls.ToolTip.text: "Copy the join address"
                                                Controls.ToolTip.visible: hovered
                                                onClicked: { clipHelper.text = modelData.server; clipHelper.selectAll(); clipHelper.copy() }
                                            }
                                            Item { Layout.fillWidth: true }
                                            Controls.ToolButton { visible: cal.canEdit; icon.name: "document-edit-symbolic"; onClicked: cal.startGameEdit(modelData); Controls.ToolTip.text: "Edit this game's rules"; Controls.ToolTip.visible: hovered; Accessible.name: "Edit game" }
                                            Controls.ToolButton {
                                                visible: cal.isMod
                                                icon.name: "edit-delete-symbolic"
                                                Controls.ToolTip.text: "Remove this game and its rules"; Controls.ToolTip.visible: hovered; Accessible.name: "Remove game"
                                                onClicked: { deleteGameConfirm.targetSlug = modelData.slug; deleteGameConfirm.open() }
                                            }
                                        }
                                        Controls.Label {
                                            textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                            Layout.fillWidth: true
                                            wrapMode: Text.WordWrap
                                            text: (modelData.rules && modelData.rules.length > 0) ? modelData.rules : "No standing rules yet."
                                            color: (modelData.rules && modelData.rules.length > 0) ? Th.calendarFg : Th.calendarMetaFg
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // ---- side pane (month + upcoming) ------------------------------------------------
            Rectangle {
                visible: cal.tab !== "games"
                Layout.preferredWidth: 400
                Layout.fillHeight: true
                radius: 12
                color: Th.calendarSideBg
                border.width: 1
                border.color: Th.calendarSideBorder

                Controls.ScrollView {
                    id: sideScroll
                    anchors.left: parent.left
                    anchors.right: parent.right
                    anchors.top: parent.top
                    anchors.bottom: editorFooter.top   // 10.1 (TH-CAL-15): the Save row lives below the scroll
                    anchors.margins: 2
                    clip: true
                    contentWidth: availableWidth

                    ColumnLayout {
                        width: sideScroll.availableWidth
                        spacing: 10

                        // ===== EDITOR =====
                        ColumnLayout {
                            visible: cal.editing
                            Layout.fillWidth: true
                            Layout.margins: 12
                            spacing: 8
                            Kirigami.Heading { level: 3; text: cal.dId.length > 0 ? "Edit event" : "New event"; color: Th.calendarFg }
                            ThTextField {
                                id: titleField
                                Layout.fillWidth: true
                                placeholderText: "Title — e.g. Valheim 1.0 launch night"
                                text: cal.dTitle
                                onTextEdited: cal.dTitle = text
                                onAccepted: cal.commitDraft()
                            }
                            ThComboBox {
                                id: gameCombo
                                Layout.fillWidth: true
                                editable: true
                                model: cal.gameNames
                                // 0.8.36: editText is NOT bound to dGame — openNew / startEdit assign
                                // it (which dropped the binding on the first open anyway), typing and picking a
                                // row (ThComboBox.onActivated) change it, and it feeds dGame from here.
                                onEditTextChanged: if (editText !== cal.dGame) cal.dGame = editText
                                onActivated: (i) => { cal.dGame = cal.gameNames[i] || ""; if (editText !== cal.dGame) editText = cal.dGame }
                                Component.onCompleted: cal.syncGameCombo()
                                Controls.ToolTip.text: "Which game — pick one or type a new one"
                                Controls.ToolTip.visible: hovered
                            }
                            Controls.Label { text: "What kind of night"; color: Th.calendarMetaFg; font.pointSize: Kirigami.Theme.smallFont.pointSize }
                            Flow {
                                Layout.fillWidth: true
                                spacing: 6
                                Repeater {
                                    model: cal.kinds
                                    delegate: ThButton {
                                        app: cal.app;
                                        required property var modelData
                                        text: modelData.label
                                        checkable: true
                                        checked: cal.dKind === modelData.k
                                        onClicked: cal.dKind = modelData.k
                                        Controls.ToolTip.text: cal.kindHint(modelData.k)
                                        Controls.ToolTip.visible: hovered
                                        Controls.ToolTip.delay: 300
                                    }
                                }
                            }
                            Controls.Label {
                                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                Layout.fillWidth: true
                                wrapMode: Text.WordWrap
                                text: cal.kindHint(cal.dKind)
                                color: Th.calendarMetaFg
                                font.pointSize: Kirigami.Theme.smallFont.pointSize
                            }
                            // 0.8.27: a main-game range is whole days — first day … last day.
                            RowLayout {
                                visible: cal.dKind === "focus"
                                Layout.fillWidth: true
                                spacing: 6
                                ThTextField {
                                    Layout.preferredWidth: 120
                                    text: cal.dDate
                                    placeholderText: "first day"
                                    onTextEdited: cal.dDate = text
                                    color: cal.validKey(text) ? Th.inputFg : Th.calendarDateFieldFgError
                                }
                                Controls.Label { text: "to"; color: Th.calendarMetaFg }
                                ThTextField {
                                    id: endDateField
                                    Layout.preferredWidth: 120
                                    text: cal.dEndDate
                                    placeholderText: "last day"
                                    onTextEdited: cal.dEndDate = text
                                    color: cal.validKey(text) ? Th.inputFg : Th.calendarDateFieldFgError
                                }
                                Controls.Label {
                                    Layout.fillWidth: true
                                    text: "Click days in the grid to set the first day"
                                    color: Th.calendarMetaFg
                                    font.pointSize: Kirigami.Theme.smallFont.pointSize
                                    elide: Text.ElideRight
                                }
                            }
                            RowLayout {
                                visible: cal.dKind !== "focus"
                                Layout.fillWidth: true
                                spacing: 6
                                ThTextField {
                                    id: dateField
                                    Layout.preferredWidth: 120
                                    text: cal.dDate
                                    placeholderText: "YYYY-MM-DD"
                                    onTextEdited: cal.dDate = text
                                    color: cal.validKey(text) ? Th.inputFg : Th.calendarDateFieldFgError
                                    Controls.ToolTip.text: "Or click a day in the grid"
                                    Controls.ToolTip.visible: hovered
                                }
                                ThComboBox {
                                    Layout.fillWidth: true
                                    model: cal.timeSlots
                                    currentIndex: Math.floor(cal.dTime / 15)
                                    onActivated: (i) => { cal.dTime = i * 15 }
                                }
                                ThComboBox {
                                    Layout.preferredWidth: 120
                                    model: cal.durations.map(function (d) { return d.label })
                                    currentIndex: cal.durationIndex(cal.dDuration)
                                    onActivated: (i) => { cal.dDuration = cal.durations[i].m }
                                }
                            }
                            Controls.Label {
                                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                visible: cal.dKind !== "focus"
                                text: cal.validKey(cal.dDate) ? (cal.fmtDayLong(cal.keyToDate(cal.dDate).getTime()) + " · " + cal.timeSlots[Math.floor(cal.dTime / 15)] + " " + cal.tz()) : "Pick a valid date"
                                color: Th.calendarMetaFg
                                font.pointSize: Kirigami.Theme.smallFont.pointSize
                            }
                            // proposal options
                            ColumnLayout {
                                visible: cal.dKind === "proposal"
                                Layout.fillWidth: true
                                spacing: 4
                                Controls.Label { text: "Proposed times (people vote, you lock one in)"; color: Th.calendarMetaFg; font.pointSize: Kirigami.Theme.smallFont.pointSize }
                                Repeater {
                                    model: cal.dOptions
                                    delegate: RowLayout {
                                        required property var modelData
                                        required property int index
                                        Layout.fillWidth: true
                                        Controls.Label {
                                            textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                            Layout.fillWidth: true
                                            text: (index + 1) + ".  " + cal.fmtDay(cal.keyToDate(modelData.date, modelData.time).getTime()) + " · " + cal.timeSlots[Math.floor(modelData.time / 15)]
                                            color: Th.calendarFg
                                        }
                                        Controls.ToolButton {
                                            icon.name: "list-remove-symbolic"
                                            // 10.1 (TH-CAL-04): votes are stored by option INDEX, so removing a time
                                            // people have voted on would silently move their answer onto the next
                                            // one. Add a new time instead.
                                            enabled: !(modelData.voted > 0)
                                            Controls.ToolTip.text: modelData.voted > 0 ? "People have voted on this time — add a new one instead" : "Remove this option"
                                            Controls.ToolTip.visible: hovered
                                            Controls.ToolTip.delay: 400
                                            onClicked: cal.removeOption(index)
                                        }
                                    }
                                }
                                ThButton {
                                    app: cal.app;
                                    text: "Add the date + time above as an option"
                                    icon.name: "list-add-symbolic"
                                    enabled: cal.validKey(cal.dDate)
                                    onClicked: cal.addOption()
                                }
                            }
                            ThCheckBox {
                                visible: cal.dKind !== "freeplay" && cal.dKind !== "focus"
                                text: "Free play is allowed outside this"
                                checked: cal.dFreePlay
                                onToggled: cal.dFreePlay = checked
                            }
                            ThCheckBox {
                                visible: cal.dKind !== "focus"
                                text: "Join if you can (no pressure)"
                                checked: cal.dJoin
                                onToggled: cal.dJoin = checked
                            }
                            ThTextArea {
                                Layout.fillWidth: true
                                Layout.preferredHeight: 90
                                placeholderText: "Notes — what we're doing, what to bring, voice channel…"
                                wrapMode: TextEdit.Wrap
                                text: cal.dNotes
                                onTextChanged: if (text !== cal.dNotes) cal.dNotes = text
                            }
                            ThTextArea {
                                Layout.fillWidth: true
                                Layout.preferredHeight: 70
                                placeholderText: "Rules for this one (the game's standing rules apply anyway)"
                                wrapMode: TextEdit.Wrap
                                text: cal.dRules
                                onTextChanged: if (text !== cal.dRules) cal.dRules = text
                            }
                            ThCheckBox {
                                visible: cal.roomId.length > 0
                                text: (cal.dId.length === 0 ? "Post it in " : "Post a fresh card in ") + (cal.roomName.length > 0 ? cal.roomName : "this room")
                                checked: cal.dAnnounce
                                onToggled: cal.dAnnounce = checked
                            }
                        }

                        // ===== EVENT DETAIL =====
                        ColumnLayout {
                            visible: !cal.editing && cal.selected !== null
                            Layout.fillWidth: true
                            Layout.margins: 12
                            spacing: 8
                            readonly property var ev: cal.selected || ({})
                            id: detail

                            RowLayout {
                                Layout.fillWidth: true
                                Rectangle {
                                    radius: 9; implicitHeight: 18; implicitWidth: kindLbl.implicitWidth + 14
                                    color: cal.tint(cal.kindColor(detail.ev.kind), 0.22)
                                    border.width: 1; border.color: cal.kindColor(detail.ev.kind)
                                    Controls.Label { textFormat: Text.PlainText; id: kindLbl; anchors.centerIn: parent; text: cal.kindLabel(detail.ev.kind); color: Th.calendarFg; font.pointSize: Kirigami.Theme.smallFont.pointSize * 0.9 }
                                }
                                Rectangle {
                                    visible: !!(detail.ev.game && detail.ev.game.length > 0)
                                    radius: 9; implicitHeight: 18; implicitWidth: gameLbl.implicitWidth + 14
                                    color: cal.tint(cal.chipColor(detail.ev), 0.22)
                                    Controls.Label { textFormat: Text.PlainText; id: gameLbl; anchors.centerIn: parent; text: detail.ev.game || ""; color: Th.calendarFg; font.pointSize: Kirigami.Theme.smallFont.pointSize * 0.9 }
                                }
                                // 9.0 Game Reel: a session with no game yet offers the picker (DESIGN §4.1).
                                Controls.AbstractButton {
                                    id: pickBtn
                                    visible: !(detail.ev.game && detail.ev.game.length > 0) && detail.ev.kind !== "focus" && detail.ev.canceled !== true
                                    implicitHeight: 18; implicitWidth: pickLbl.implicitWidth + 16
                                    hoverEnabled: true
                                    onClicked: cal.pickGame(detail.ev.id)
                                    background: Rectangle { radius: 9; color: (pickBtn.hovered ? Th.calendarPickGameBgHover : Th.calendarPickGameBg); border.width: 1; border.color: Th.calendarPickGameBorder }
                                    contentItem: Controls.Label { id: pickLbl; text: "🎲 Game TBD · pick tonight's game"; color: Th.calendarPickGameFg; font.pointSize: Kirigami.Theme.smallFont.pointSize * 0.9; horizontalAlignment: Text.AlignHCenter; verticalAlignment: Text.AlignVCenter }
                                }
                                Rectangle {
                                    visible: detail.ev.canceled === true
                                    radius: 9; implicitHeight: 18; implicitWidth: canLbl.implicitWidth + 14
                                    color: Th.calendarCanceledBg
                                    Controls.Label { id: canLbl; anchors.centerIn: parent; text: "Canceled"; color: Th.calendarFg; font.pointSize: Kirigami.Theme.smallFont.pointSize * 0.9 }
                                }
                                Item { Layout.fillWidth: true }
                                Controls.ToolButton { visible: detail.ev.canEdit === true; icon.name: "document-edit-symbolic"; Controls.ToolTip.text: "Edit"; Controls.ToolTip.visible: hovered; onClicked: cal.startEdit(detail.ev) }
                                Controls.ToolButton {
                                    visible: detail.ev.canEdit === true && detail.ev.kind !== "focus"
                                    icon.name: detail.ev.canceled === true ? "edit-undo-symbolic" : "dialog-cancel-symbolic"
                                    Controls.ToolTip.text: detail.ev.canceled === true ? "Put it back on" : "Call it off — keeps the event and everyone's RSVPs"
                                    Controls.ToolTip.visible: hovered
                                    onClicked: {
                                        if (detail.ev.canceled === true) cal.setCanceled(detail.ev, false)
                                        else { callOffConfirm.targetEv = detail.ev; callOffConfirm.open() }
                                    }
                                }
                                Controls.ToolButton {
                                    visible: detail.ev.canEdit === true
                                    icon.name: "edit-delete-symbolic"
                                    Controls.ToolTip.text: "Remove for good"; Controls.ToolTip.visible: hovered
                                    onClicked: { deleteConfirm.targetId = detail.ev.id; deleteConfirm.open() }
                                }
                            }
                            Kirigami.Heading { textFormat: Text.PlainText; Layout.fillWidth: true; level: 2; text: detail.ev.title || ""; color: Th.calendarFg; wrapMode: Text.WordWrap }
                            RowLayout {
                                spacing: 8
                                ThIcon { app: cal.app; name: "view-calendar-upcoming-events-symbolic"; width: 18; height: 18; color: Th.calendarDetailWhenIcon }
                                Controls.Label { textFormat: Text.PlainText; Layout.fillWidth: true; text: cal.whenLine(detail.ev) + (detail.ev.effStart ? "  " + cal.tz() : ""); color: Th.calendarFg; font.bold: true; wrapMode: Text.WordWrap }
                            }
                            Flow {
                                Layout.fillWidth: true
                                spacing: 6
                                Controls.Label { visible: detail.ev.freePlay === true && detail.ev.kind !== "freeplay"; text: "✓ free play allowed"; color: Th.calendarDetailFreePlayFg; font.pointSize: Kirigami.Theme.smallFont.pointSize }
                                Controls.Label { visible: detail.ev.joinIfYouCan === true; text: "✓ join if you can"; color: Th.calendarDetailJoinIfYouCanFg; font.pointSize: Kirigami.Theme.smallFont.pointSize }
                            }
                            RowLayout {
                                spacing: 6
                                Avatar { app: cal.app; size: 20; name: detail.ev.creator ? detail.ev.creator.name : ""; seed: detail.ev.createdBy || ""; imageSource: detail.ev.creator ? detail.ev.creator.avatar : "" }
                                Controls.Label { textFormat: Text.PlainText; text: "Organised by " + (detail.ev.creator ? detail.ev.creator.name : ""); color: Th.calendarMetaFg; font.pointSize: Kirigami.Theme.smallFont.pointSize }
                            }
                            Controls.Label {
                                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                visible: !!(detail.ev.notes && detail.ev.notes.length > 0)
                                Layout.fillWidth: true
                                text: detail.ev.notes || ""
                                color: Th.calendarFg
                                wrapMode: Text.WordWrap
                            }
                            // rules: the session's own + the game's standing rules
                            ColumnLayout {
                                Layout.fillWidth: true
                                spacing: 2
                                visible: !!((detail.ev.rules && detail.ev.rules.length > 0) || (cal.gameFor(detail.ev) && cal.gameFor(detail.ev).rules && cal.gameFor(detail.ev).rules.length > 0))
                                Controls.Label { text: "Rules"; color: Th.calendarMetaFg; font.bold: true; font.pointSize: Kirigami.Theme.smallFont.pointSize }
                                Controls.Label { textFormat: Text.PlainText; visible: !!(detail.ev.rules && detail.ev.rules.length > 0); Layout.fillWidth: true; text: detail.ev.rules || ""; color: Th.calendarFg; wrapMode: Text.WordWrap }
                                Controls.Label {
                                    textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                    visible: !!(cal.gameFor(detail.ev) && cal.gameFor(detail.ev).rules && cal.gameFor(detail.ev).rules.length > 0)
                                    Layout.fillWidth: true
                                    text: cal.gameFor(detail.ev) ? (cal.gameFor(detail.ev).name + " standing rules: " + cal.gameFor(detail.ev).rules) : ""
                                    color: Th.calendarMetaFg
                                    wrapMode: Text.WordWrap
                                }
                                Controls.Label {
                                    textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                    visible: !!(cal.gameFor(detail.ev) && cal.gameFor(detail.ev).server && cal.gameFor(detail.ev).server.length > 0)
                                    text: cal.gameFor(detail.ev) ? ("Join: " + cal.gameFor(detail.ev).server) : ""
                                    color: Th.calendarEmphasisFg
                                    font.family: Th.fontMono
                                }
                            }
                            Kirigami.Separator { Layout.fillWidth: true; opacity: 0.5 }

                            // proposal voting
                            ColumnLayout {
                                visible: detail.ev.kind === "proposal" && detail.ev.options && detail.ev.options.length > 0
                                Layout.fillWidth: true
                                spacing: 6
                                Controls.Label {
                                    text: detail.ev.chosen >= 0 ? "Time locked in" : "When can you make it? (tick every time that works)"
                                    color: Th.calendarMetaFg; font.bold: true; font.pointSize: Kirigami.Theme.smallFont.pointSize
                                }
                                Repeater {
                                    model: detail.ev.options || []
                                    delegate: Rectangle {
                                        required property var modelData
                                        required property int index
                                        Layout.fillWidth: true
                                        implicitHeight: optRow.implicitHeight + 14
                                        radius: 8
                                        color: detail.ev.chosen === index ? Th.calendarOptionBgSelected : Th.calendarOptionBg
                                        border.width: detail.ev.chosen === index ? 1 : 0
                                        border.color: Th.calendarOptionBorderSelected
                                        RowLayout {
                                            id: optRow
                                            anchors.fill: parent
                                            anchors.margins: 7
                                            spacing: 8
                                            ThCheckBox {
                                                enabled: cal.canEdit && detail.ev.chosen < 0
                                                checked: cal.myOptions(detail.ev).indexOf(index) >= 0
                                                onToggled: cal.toggleVote(detail.ev, index)
                                            }
                                            ColumnLayout {
                                                Layout.fillWidth: true
                                                spacing: 0
                                                Controls.Label { textFormat: Text.PlainText; text: cal.fmtDay(modelData.start) + " · " + cal.fmtTime(modelData.start); color: Th.calendarFg; font.bold: true }
                                                Row {
                                                    spacing: -6
                                                    Repeater {
                                                        model: (modelData.votes || []).slice(0, 8)
                                                        delegate: Avatar { required property var modelData; app: cal.app; size: 18; name: modelData.name; seed: modelData.mxid; imageSource: modelData.avatar }
                                                    }
                                                }
                                            }
                                            Controls.Label { textFormat: Text.PlainText; text: (modelData.votes ? modelData.votes.length : 0) + (modelData.votes && modelData.votes.length === 1 ? " can" : " can"); color: Th.calendarMetaFg; font.pointSize: Kirigami.Theme.smallFont.pointSize }
                                            ThButton {
                                                app: cal.app;
                                                visible: detail.ev.canEdit === true && detail.ev.chosen < 0
                                                text: "Lock in"
                                                onClicked: cal.lockOption(detail.ev.id, index, cal.whenTextFor(detail.ev, index))
                                            }
                                            Controls.Label { visible: detail.ev.chosen === index; text: "✓ locked"; color: Th.calendarOptionLockedFg; font.bold: true }
                                        }
                                    }
                                }
                                ThButton {
                                    app: cal.app;
                                    visible: detail.ev.canEdit === true && detail.ev.chosen >= 0
                                    text: "Unlock — vote again"
                                    onClicked: cal.lockOption(detail.ev.id, -1, cal.whenTextFor(detail.ev, -1))
                                }
                            }

                            // RSVP
                            Controls.Label { text: detail.ev.past === true ? "This one already happened" : "Are you in?"; color: Th.calendarMetaFg; font.bold: true; font.pointSize: Kirigami.Theme.smallFont.pointSize }   // 10.1 (TH-CAL-18)
                            RowLayout {
                                Layout.fillWidth: true
                                spacing: 6
                                Repeater {
                                    model: [ { s: "yes", l: "Going", c: Th.calendarRsvpYesFill }, { s: "maybe", l: "Maybe", c: Th.calendarRsvpMaybeFill }, { s: "no", l: "Can't", c: Th.calendarRsvpNoFill } ]
                                    delegate: Controls.AbstractButton {
                                        id: rsvpBtn
                                        required property var modelData
                                        readonly property bool on: cal.myStatus(detail.ev) === modelData.s
                                        readonly property int n: detail.ev.rsvp ? detail.ev.rsvp[modelData.s].length : 0
                                        Layout.fillWidth: true
                                        implicitHeight: 34
                                        enabled: cal.canEdit
                                        hoverEnabled: true
                                        onClicked: cal.setRsvp(detail.ev, modelData.s)
                                        background: Rectangle {
                                            radius: 17
                                            color: rsvpBtn.on ? cal.tint(rsvpBtn.modelData.c, 0.35) : (rsvpBtn.hovered ? Th.calendarRsvpBgHover : Th.calendarRsvpBg)
                                            border.width: rsvpBtn.on ? 2 : 1
                                            border.color: rsvpBtn.on ? rsvpBtn.modelData.c : Th.calendarRsvpBorder
                                        }
                                        contentItem: Controls.Label {
                                            textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                            text: rsvpBtn.modelData.l + "  " + rsvpBtn.n
                                            horizontalAlignment: Text.AlignHCenter
                                            verticalAlignment: Text.AlignVCenter
                                            color: Th.calendarRsvpFg
                                            font.bold: rsvpBtn.on
                                        }
                                    }
                                }
                            }
                            Controls.Label {
                                visible: !cal.canEdit
                                text: "RSVPs open once a space admin has opened this calendar."
                                color: Th.calendarMetaFg; font.pointSize: Kirigami.Theme.smallFont.pointSize
                            }
                            // who's in
                            ColumnLayout {
                                Layout.fillWidth: true
                                spacing: 4
                                Controls.Label { text: "Who's in"; color: Th.calendarMetaFg; font.bold: true; font.pointSize: Kirigami.Theme.smallFont.pointSize }
                                Controls.Label {
                                    visible: !!(!detail.ev.rsvp || (detail.ev.rsvp.yes.length === 0 && detail.ev.rsvp.maybe.length === 0))
                                    text: "Nobody yet — be the first."
                                    color: Th.calendarMetaFg
                                }
                                Flow {
                                    Layout.fillWidth: true
                                    spacing: 6
                                    Repeater {
                                        model: detail.ev.rsvp ? detail.ev.rsvp.yes : []
                                        delegate: Rectangle {
                                            required property var modelData
                                            radius: 13; implicitHeight: 26; implicitWidth: yesRow.implicitWidth + 12
                                            color: Th.calendarWhoYesBg
                                            Row {
                                                id: yesRow; anchors.centerIn: parent; spacing: 5
                                                Avatar { app: cal.app; size: 20; name: modelData.name; seed: modelData.mxid; imageSource: modelData.avatar; anchors.verticalCenter: parent.verticalCenter }
                                                Controls.Label { textFormat: Text.PlainText; text: modelData.name; color: Th.calendarFg; anchors.verticalCenter: parent.verticalCenter }
                                            }
                                        }
                                    }
                                    Repeater {
                                        model: detail.ev.rsvp ? detail.ev.rsvp.maybe : []
                                        delegate: Rectangle {
                                            required property var modelData
                                            radius: 13; implicitHeight: 26; implicitWidth: mayRow.implicitWidth + 12
                                            color: Th.calendarWhoMaybeBg
                                            Row {
                                                id: mayRow; anchors.centerIn: parent; spacing: 5
                                                Avatar { app: cal.app; size: 20; name: modelData.name; seed: modelData.mxid; imageSource: modelData.avatar; anchors.verticalCenter: parent.verticalCenter }
                                                Controls.Label { textFormat: Text.PlainText; text: modelData.name + " (maybe)"; color: Th.calendarFg; anchors.verticalCenter: parent.verticalCenter }
                                            }
                                        }
                                    }
                                }
                                Controls.Label {
                                    textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                    visible: !!(detail.ev.rsvp && detail.ev.rsvp.no.length > 0)
                                    Layout.fillWidth: true
                                    text: "Can't make it: " + (detail.ev.rsvp ? detail.ev.rsvp.no.map(function (p) { return p.name }).join(", ") : "")
                                    color: Th.calendarMetaFg
                                    font.pointSize: Kirigami.Theme.smallFont.pointSize
                                    wrapMode: Text.WordWrap
                                }
                            }
                        }

                        // ===== DAY AGENDA =====
                        ColumnLayout {
                            visible: !cal.editing && cal.selected === null
                            Layout.fillWidth: true
                            Layout.margins: 12
                            spacing: 8
                            Kirigami.Heading {
                                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                level: 3
                                text: cal.validKey(cal.selectedDay) ? cal.fmtDayLong(cal.keyToDate(cal.selectedDay).getTime()) : ""
                                color: Th.calendarFg
                            }
                            Controls.Label {
                                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                visible: !!(cal.validKey(cal.selectedDay) && cal.relDay(cal.keyToDate(cal.selectedDay).getTime()).length > 0)
                                text: cal.validKey(cal.selectedDay) ? cal.relDay(cal.keyToDate(cal.selectedDay).getTime()) : ""
                                color: Th.calendarEmphasisFg
                                font.bold: true
                            }
                            Controls.Label {
                                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                visible: cal.focusOn(cal.selectedDay) !== null
                                Layout.fillWidth: true
                                text: cal.focusOn(cal.selectedDay) ? ("Main game: " + ((cal.focusOn(cal.selectedDay).game && cal.focusOn(cal.selectedDay).game.length > 0) ? cal.focusOn(cal.selectedDay).game : cal.focusOn(cal.selectedDay).title) + "  ·  " + cal.whenLine(cal.focusOn(cal.selectedDay))) : ""
                                color: cal.focusOn(cal.selectedDay) ? cal.chipColor(cal.focusOn(cal.selectedDay)) : Th.calendarMetaFg
                                font.bold: true
                                wrapMode: Text.WordWrap
                            }
                            EmptyState {   // 10.1 (TH-STATE-09): the house empty state, like the two sections above it
                                visible: cal.dayEvents.length === 0
                                Layout.fillWidth: true
                                app: cal.app
                                iconName: "view-calendar-symbolic"
                                title: "Nothing on this day"
                                explanation: cal.canEdit ? "Pick another day, or add something below." : "Pick another day."
                            }
                            Repeater {
                                model: cal.dayEvents
                                delegate: Rectangle {
                                    required property var modelData
                                    Layout.fillWidth: true
                                    implicitHeight: dayRow.implicitHeight + 18
                                    radius: 10
                                    color: dayMa.containsMouse ? Th.calendarAgendaRowBgHover : Th.calendarAgendaRowBg
                                    border.width: 1; border.color: cal.tint(cal.chipColor(modelData), 0.6)
                                    RowLayout {
                                        id: dayRow
                                        anchors.fill: parent
                                        anchors.margins: 9
                                        spacing: 10
                                        Rectangle { Layout.preferredWidth: 5; Layout.fillHeight: true; radius: 3; color: cal.chipColor(modelData) }
                                        ColumnLayout {
                                            Layout.fillWidth: true
                                            spacing: 1
                                            Controls.Label { textFormat: Text.PlainText; text: modelData.title; color: Th.calendarFg; font.bold: true; elide: Text.ElideRight; Layout.fillWidth: true; font.strikeout: modelData.canceled === true }
                                            Controls.Label {
                                                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                                text: cal.fmtTime(modelData.effStart) + (modelData.end > modelData.effStart ? " – " + cal.fmtTime(modelData.end) : "")
                                                      + (modelData.game ? "  ·  " + modelData.game : "") + "  ·  " + cal.kindLabel(modelData.kind)
                                                color: Th.calendarMetaFg; font.pointSize: Kirigami.Theme.smallFont.pointSize; elide: Text.ElideRight; Layout.fillWidth: true
                                            }
                                            Controls.Label {
                                                textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                                                text: (modelData.rsvp ? modelData.rsvp.yes.length : 0) + " going"
                                                      + ((modelData.rsvp && modelData.rsvp.maybe.length > 0) ? " · " + modelData.rsvp.maybe.length + " maybe" : "")
                                                      + (cal.myStatus(modelData) === "yes" ? "  ·  you're in" : "")
                                                color: Th.calendarMetaFg; font.pointSize: Kirigami.Theme.smallFont.pointSize
                                            }
                                        }
                                    }
                                    MouseArea { id: dayMa; anchors.fill: parent; hoverEnabled: true; onClicked: cal.selectedId = modelData.id }
                                }
                            }
                            ThButton {
                                app: cal.app;
                                visible: cal.canEdit
                                text: "New event on this day"
                                icon.name: "appointment-new-symbolic"
                                onClicked: cal.startNew(cal.selectedDay)
                            }
                            Controls.Label {
                                Layout.fillWidth: true
                                Layout.topMargin: 8
                                wrapMode: Text.WordWrap
                                text: cal.reminders ? "You get a desktop reminder 30 minutes before anything you're going to, and a heads-up when someone schedules something new."
                                                    : "Reminders are off (bell in the header)."
                                color: Th.calendarMetaFg
                                font.pointSize: Kirigami.Theme.smallFont.pointSize
                            }
                        }
                    }
                }
                // 10.1 (TH-CAL-15): Cancel / Save is a fixed footer under the scroll, not the last
                // row inside it — at the window's minimum height it used to sit below the fold.
                Rectangle {
                    id: editorFooter
                    anchors.left: parent.left
                    anchors.right: parent.right
                    anchors.bottom: parent.bottom
                    anchors.margins: 2
                    visible: cal.editing
                    height: visible ? footerRow.implicitHeight + 16 : 0
                    radius: 10
                    color: Th.calendarFooterBg
                    Rectangle { anchors.left: parent.left; anchors.right: parent.right; anchors.top: parent.top; height: 1; color: Th.calendarFooterDivider }
                    RowLayout {
                        id: footerRow
                        anchors.fill: parent
                        anchors.margins: 8
                        anchors.leftMargin: 12
                        anchors.rightMargin: 12
                        Item { Layout.fillWidth: true }
                        ThButton { app: cal.app; text: "Cancel"; onClicked: cal.closeEditor(false) }
                        GlowButton {
                            app: cal.app
                            text: cal.dId.length > 0 ? "Save changes" : "Schedule it"
                            enabled: cal.dTitle.trim().length > 0 && (cal.dKind === "proposal" ? (cal.dOptions.length > 0 || cal.validKey(cal.dDate)) : (cal.dKind === "focus" ? (cal.validKey(cal.dDate) && cal.validKey(cal.dEndDate)) : cal.validKey(cal.dDate)))
                            onClicked: cal.commitDraft()
                        }
                    }
                }
            }
        }
    }
    // Clipboard helper for the "copy join address" button (TextEdit.copy is the portable route).
    TextEdit { id: clipHelper; visible: false }
}
