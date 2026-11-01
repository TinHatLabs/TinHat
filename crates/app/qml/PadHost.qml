// PadHost.qml — Pad's receiver glue (docs/PAD.md; pad_ctl.rs / pad_engine.rs). Two jobs, no UI:
//   * the EXECUTOR: padCtl.exec(token, cmds) — a command my other verified device sent and the rules accepted — is
//     CLAIMED first (padCtl.claim: still that session, Pad on, the device not refused, the very call this device is in
//     right now, ≤ 1.5 s old; no = nothing runs, the refusal goes back from Rust), then runs the SAME functions this
//     device's own buttons run (the soundboard's startSound / stopCurrent, the mic button's
//     forceMicMuted(true) / toggleMic, the deafen button's applyDeafen, BRB's setAfk, the per-person menu's
//     pushVolume / setVolume / setMuted, Leave's leaveCall), so every gate a click meets still applies (the
//     soundboard's padGate / use_soundboard, the server-mute lock, push-to-talk). Then it publishes the new snapshot
//     and says executed(token): the answer carries the effect. Pad QOL (2026-10-31): a search result
//     (`sound.play.web` → the same startSound) and the call room's Media Party through the window's own functions
//     (watchCtl.joinParty / play / pause / seek / playUrl(next) / addLink), each re-checking right before it runs that
//     the url it was meant for still plays and that this device may still drive it.
//   * the SNAPSHOT: this device's call as the Pad shows it (publishState, at most 4×/s and only when it changed):
//     {call: {room, since} | null, me: {micMuted, ptt, serverMuted, deaf, brb, talk, sound, gate}, people: [{u, talk,
//     micOff, away, vol, muted}], recent, favourites}. Read from the controllers' live getters (not the 120 ms / 1 s
//     mirrors): the PC is usually not the focused window while a Pad drives it.
// Instantiated once in Main.qml with `app: root`; every controller call goes through `app.<controller>` (the
// boundary gate checks those). scripts/pad-executor-test.qml drives `runOne` with recording fakes.
import QtQuick

Item {
    id: host
    visible: false
    property var app: null
    // the Media Party window (Main.qml's), for party.join's "Join + open" — the call bar's own pair
    property var watchPartyWindowRef: null
    // test hook: the executor's calls in order (scripts/pad-executor-test.qml)
    property var calls: []
    property bool recording: false
    function _rec(name, args) { if (host.recording) host.calls = host.calls.concat([{ fn: name, args: args }]) }

    // ── the executor ──
    // A sound's name / emoji for the soundboard's Recent (favourites, then Recent; the url's file name otherwise).
    function soundInfo(url) {
        var favs = (host.app && host.app.mPersonalSounds) || []
        for (var i = 0; i < favs.length; i++) if (favs[i] && favs[i].sourceUrl === url) return { name: String(favs[i].name || ""), emoji: String(favs[i].emoji || "") }
        var rec = (host.app && host.app.mSbRecent) || []
        for (var j = 0; j < rec.length; j++) if (rec[j] && rec[j].url === url) return { name: String(rec[j].name || ""), emoji: String(rec[j].emoji || "") }
        var base = String(url).replace(/[?#].*$/, "")
        base = base.substring(base.lastIndexOf("/") + 1).replace(/\.[a-z0-9]+$/i, "").replace(/[_-]+/g, " ")
        try { base = decodeURIComponent(base) } catch (e) {}
        return { name: base, emoji: "" }
    }
    // One command → the button's own function (true = known). Every op arrives as a SET; the one toggle (the mic
    // button's) runs only while the mic really is muted.
    function runOne(c) {
        switch (String(c && c.do)) {
        case "startSound": {
            // the name + emoji the receiver's own lists give it (pad_net.rs: favourites, the call room's sounds), else
            // the soundboard's copies / Recent, else the url's file name
            var d = (typeof c.name === "string" && c.name.length > 0) ? { name: c.name, emoji: String(c.emoji || "") } : host.soundInfo(String(c.url))
            host._rec("startSound", [String(c.url), d.name, d.emoji]); app.soundboardPanel.startSound(String(c.url), d.name, d.emoji); return true
        }
        case "stopSound": host._rec("stopCurrent", []); app.soundboardPanel.stopCurrent(); return true
        case "muteMic": host._rec("forceMicMuted", [true]); app.callctl.forceMicMuted(true); return true
        case "unmuteMic": if (app.callctl.micMuted) { host._rec("toggleMic", []); app.callctl.toggleMic() } return true
        case "deafen": host._rec("applyDeafen", [c.on === true]); app.callctl.applyDeafen(c.on === true); return true
        case "brb": host._rec("setAfk", [c.on === true]); app.afkCtl.setAfk(c.on === true); return true
        case "pushVolume": host._rec("pushVolume", [String(c.user), String(c.cat), Number(c.level)]); app.audioPrefsCtl.pushVolume(String(c.user), String(c.cat), Number(c.level)); return true
        case "setVolume": host._rec("setVolume", [String(c.user), String(c.cat), Number(c.level)]); app.audioPrefsCtl.setVolume(String(c.user), String(c.cat), Number(c.level)); return true
        case "setMuted": host._rec("setMuted", [String(c.user), String(c.cat), c.muted === true]); app.audioPrefsCtl.setMuted(String(c.user), String(c.cat), c.muted === true); return true
        case "leave": host._rec("leaveCall", []); app.callctl.leaveCall(); return true
        case "partyJoin": {
            // the call bar's Media Party "Join": join (never re-join: joinParty is destructive) + open the window
            var room = String(app.callctl.roomId || "")
            host._rec("joinParty", [room])
            if (room.length === 0) return true
            // in ANOTHER room's party: switch (leave, then join this one) — review #6; never re-join the same one
            if (app.watchCtl.inParty && String(app.watchCtl.roomId) !== room) app.watchCtl.leaveParty()
            if (!app.watchCtl.inParty || String(app.watchCtl.roomId) !== room) app.watchCtl.joinParty(room)
            var wpw = host.watchPartyWindowRef
            if (wpw) { if (wpw.openOrRaise) wpw.openOrRaise(); else wpw.show() }
            return true
        }
        case "partyPlay":
            if (!host.partyStill(String(c.url))) return true
            host._rec(c.playing === true ? "play" : "pause", []); if (c.playing === true) app.watchCtl.play(); else app.watchCtl.pause(); return true
        case "partySeek":
            if (!host.partyStill(String(c.url))) return true
            host._rec("seek", [String(Math.max(0, Math.floor(Number(c.pos) || 0)))]); app.watchCtl.seek(String(Math.max(0, Math.floor(Number(c.pos) || 0)))); return true
        case "partyNext": {
            if (!host.partyStill(String(c.url))) return true
            var nx = host.nextAfter(String(c.url))
            host._rec("playUrl", [nx]); if (nx.length > 0) app.watchCtl.playUrl(nx); return true
        }
        case "partyAdd": {
            var r2 = String(app.callctl.roomId || "")
            var sid = String(c.sid || ""), re = Number(c.re || 0)
            if (!app.watchCtl.inParty || String(app.watchCtl.roomId) !== r2) { console.warn("[pad] party.add: not in this call room's party"); host.reportAdd(sid, re, false); return true }
            // the receiver's own link rules agree with the rules' host list (youtube / ytmusic / spotify / apple only)
            var kind = String(host._json(app.watchCtl.classifyLink(String(c.url)), {}).kind || "")
            if (["youtube", "ytmusic", "spotify", "apple"].indexOf(kind) === -1) { console.warn("[pad] party.add: a " + kind + " link is not taken from a Pad"); host.reportAdd(sid, re, false); return true }
            // one Pad add at a time (the controller's lookup is single-flight: its outcome must be OURS — review #3)
            if (app.watchCtl.adding === true || host._addWait) { console.warn("[pad] party.add: a link is still being looked up"); host.reportAdd(sid, re, false); return true }
            // queue first; "play now" only after the lookup, if the call session + party are unchanged (the epoch) and I
            // may still drive it (review #4)
            host._addWait = { url: String(c.url), room: r2, now: c.now === true, at: Date.now(), epoch: host._epoch, started: false, sid: sid, re: re }
            // LOOK UP only; nothing is queued until the result is checked (Astra pass 3)
            host._rec("lookupLink", [String(c.url)]); app.watchCtl.lookupLink(String(c.url)); return true
        }
        }
        console.warn("[pad] unknown command", JSON.stringify(c))
        return false
    }
    // ── Media Party (the call room's) ──
    function _wpParty() { return host._json(app.watchCtl ? app.watchCtl.partyJson : "", {}) }
    function _wpPlayback() { return host._json(app.watchCtl ? app.watchCtl.playbackJson : "", {}) }
    function partyHost() {
        if (!app.watchCtl || !app.watchCtl.inParty) return false
        var h = String(host._wpParty().hostId || "")
        return h.length === 0 || h === String(app.mUserId || "")
    }
    // the command's url still plays in the call room's party and I may still drive it (else nothing runs: a log line)
    function partyStill(url) {
        var ok = !!app.watchCtl && app.watchCtl.inParty && String(app.watchCtl.roomId) === String(app.callctl.roomId)
                 && String(host._wpPlayback().url || "") === url && host.partyHost()
        if (!ok) console.warn("[pad] party command: what plays (or who drives it) changed — not run")
        return ok
    }
    function nextAfter(url) {
        var q = host._wpParty().queue || []
        if (q.length === 0) return ""
        if (url.length === 0) return String(q[0].url || "")
        for (var i = 0; i < q.length; i++) if (q[i] && q[i].url === url) return i + 1 < q.length ? String(q[i + 1].url || "") : ""
        return String(q[0].url || "")
    }
    property var _addWait: null
    // the outcome of the latest Pad party.add ({sid, re, ok}: call.party.add — that Pad says "couldn't add that link")
    property var _lastAdd: null
    function reportAdd(sid, re, ok) {
        if (String(sid).length === 0) return
        host._lastAdd = { sid: String(sid), re: Number(re), ok: ok === true }
        host.publish(true)
    }
    // bumps whenever the call session or this device's party (in it / which room) changes: a lookup that started
    // before a bump never acts after it (leave + rejoin of the same room is a new session)
    property int _epoch: 0
    Connections {
        target: host.app ? host.app.callctl : null
        ignoreUnknownSignals: true
        function onInCallChanged() { host._epoch++ }
    }
    Connections {
        target: host.app ? host.app.watchCtl : null
        ignoreUnknownSignals: true
        function onInPartyChanged() { host._epoch++ }
        function onRoomIdChanged() { host._epoch++ }
        // the lookup is ours from the moment `adding` rises after our call until it falls: then linkJson is its outcome
        function onAddingChanged() {
            var w = host._addWait
            if (!w) return
            if (host.app.watchCtl.adding === true) { w.started = true; return }
            if (!w.started) return
            host._addWait = null
            var out = host._json(host.app.watchCtl.linkJson, {})
            if (w.epoch !== host._epoch) { console.warn("[pad] party.add: the call or the party changed during the lookup"); host.reportAdd(w.sid, w.re, false); return }
            if (out.status !== "resolved") { console.warn("[pad] party.add: " + String(out.status || "no outcome")); host.reportAdd(w.sid, w.re, false); return }
            var it = out.item || {}
            // the lookup's result must be THIS link's (a YouTube id in the request has to be the item's)
            var m = /(?:[?&]v=|youtu\.be\/)([A-Za-z0-9_-]{11})/.exec(w.url)
            if (m && String(it.url || "").indexOf(m[1]) === -1) { console.warn("[pad] party.add: the lookup answered for another link — not added"); host.reportAdd(w.sid, w.re, false); return }
            if (!host.app.watchCtl.inParty || String(host.app.watchCtl.roomId) !== w.room || String(host.app.callctl.roomId) !== w.room || String(it.url || "").length === 0) {
                console.warn("[pad] party.add: no longer this call room's party — not added"); host.reportAdd(w.sid, w.re, false); return
            }
            host._rec("enqueueItem", [String(it.url)]); host.app.watchCtl.enqueueItem(JSON.stringify(it))
            host.reportAdd(w.sid, w.re, true)
            if (!w.now) return
            if (!host.partyHost() || String(host.app.callctl.roomId) !== w.room || String(it.url || "").length === 0) {
                console.warn("[pad] party.add: no longer allowed to drive the party — queued only")
                return
            }
            host._rec("playUrl", [String(it.url)]); host.app.watchCtl.playUrl(String(it.url))
        }
    }
    // a lookup that never reports back (no `adding` rise within 20 s) is given up: the Pad hears "couldn't add"
    Timer {
        interval: 5000; repeat: true; running: !!host._addWait
        onTriggered: { var w = host._addWait; if (w && Date.now() - w.at > 20000 && !(host.app.watchCtl && host.app.watchCtl.adding === true)) { host._addWait = null; host.reportAdd(w.sid, w.re, false) } }
    }
    function partySnap(room) {
        var w = app.watchCtl
        if (!w) return null
        var inP = w.inParty === true && String(w.roomId) === room
        var cp = host._json(w.callPartyJson, {})
        if (!inP && cp.live !== true) return null
        var pb = inP ? host._wpPlayback() : { url: cp.url || "", playing: cp.playing === true, positionSecs: 0, updatedAt: 0 }
        var party = inP ? host._wpParty() : { queue: [], hostId: cp.hostId || "" }
        var q = party.queue || []
        var url = String(pb.url || "")
        var idx = -1
        for (var i = 0; i < q.length; i++) if (q[i] && q[i].url === url) { idx = i; break }
        var item = idx >= 0 ? q[idx] : (cp.item || null)
        var up = idx >= 0 ? q.slice(idx + 1) : q
        var next = []
        for (var k = 0; k < up.length && next.length < 5; k++) next.push(String((up[k] && (up[k].title || up[k].url)) || ""))
        var h = String(party.hostId || "")
        return {
            in: inP, host: inP && (h.length === 0 || h === String(app.mUserId || "")),
            url: url, title: String((item && item.title) || ""), playing: pb.playing === true || pb.playing === "true",
            pos: Math.max(0, Math.floor(Number(pb.positionSecs) || 0)), at: Math.max(0, Math.floor(Number(pb.updatedAt) || 0)),
            dur: Math.max(0, Math.floor(Number(item && (item.durationSec || item.duration)) || 0)), next: next,
            add: host._lastAdd
        }
    }
    function run(token, cmdsJson) {
        if (!host.app || !host.app.padCtl || !app.padCtl.claim(token)) return
        var cmds = []
        try { cmds = JSON.parse(String(cmdsJson)) } catch (e) { cmds = [] }
        for (var i = 0; i < cmds.length; i++) {
            try { host.runOne(cmds[i]) } catch (e) { console.warn("[pad] command failed:", e) }
        }
        host.publish(true)
        if (host.app && host.app.padCtl) host.app.padCtl.executed(token)
    }
    Connections {
        target: host.app ? host.app.padCtl : null
        ignoreUnknownSignals: true
        function onExec(token, cmds) { host.run(token, cmds) }
    }

    // ── the snapshot ──
    property double _since: 0
    property string _callRoom: ""
    function _json(s, dflt) { try { var v = JSON.parse(String(s)); return v === null || v === undefined ? dflt : v } catch (e) { return dflt } }
    function snapshot() {
        var c = host.app ? app.callctl : null
        if (!c || !c.inCall || String(c.roomId).length === 0) { host._callRoom = ""; return { call: null } }
        if (host._callRoom !== String(c.roomId)) { host._callRoom = String(c.roomId); host._since = Date.now() }
        var spk = host._json(c.speakers, {})
        var active = spk.active || []
        var tiles = host._json(c.videoTiles, [])
        var away = app.afkCtl ? host._json(app.afkCtl.awayJson, []) : []
        var mine = String(app.mUserId || "")
        var cats = ["mic", "screenshare", "music"]
        var byMxid = {}, order = []
        for (var i = 0; i < tiles.length; i++) {
            var t = tiles[i]
            if (!t || !t.participant || t.id === "vid_selfview" || t.id === "vid_selfshare" || t.participant === "You") continue
            var m = String(app.tileMxid(t.participant))
            // never my own devices, never the Memories recorder
            if (m.length === 0 || m === mine || app.isRecorder(t.participant) || app.isRecorder(m)) continue
            var p = byMxid[m]
            if (!p) {
                p = { u: m, talk: false, micOff: false, away: away.indexOf(m) !== -1, _mics: 0, _muted: 0 }
                byMxid[m] = p; order.push(m)
            }
            // folded per person: talking = any device talking, mic off = all their devices' mics off
            if (active.indexOf(t.participant) !== -1) p.talk = true
            if (t.kind !== "screenshare") { p._mics += 1; if (t.muted === true) p._muted += 1 }
            else p.share = true
        }
        var people = []
        for (var k = 0; k < order.length; k++) {
            var e = byMxid[order[k]]
            var vol = {}, muted = {}
            for (var j = 0; j < cats.length; j++) {
                var v = parseFloat(app.audioPrefsCtl.volumeFor(e.u, cats[j]))
                if (isFinite(v) && Math.abs(v - 1) > 0.001) vol[cats[j]] = v
                if (app.audioPrefsCtl.mutedFor(e.u, cats[j])) muted[cats[j]] = true
            }
            people.push({ u: e.u, talk: e.talk, micOff: e._mics > 0 && e._muted === e._mics, away: e.away, share: e.share === true, vol: vol, muted: muted })
        }
        var gate = host._json(app.soundboardCtl.padGate(true, JSON.stringify(app.mCallPerms || {})), { allowed: true, hint: "" })
        var favs = (app.mPersonalSounds || []).map(function (d) { return String((d && d.sourceUrl) || "") }).filter(function (u) { return u.length > 0 })
        return {
            call: { room: String(c.roomId), since: Math.round(host._since) },
            me: {
                micMuted: c.micMuted === true, ptt: c.pushToTalk === true, serverMuted: c.serverMuted === true,
                deaf: c.deafened === true, brb: !!app.afkCtl && app.afkCtl.localAfk === true,
                talk: spk.me ? active.indexOf(spk.me) !== -1 : false,
                sound: app.soundboardPanel ? String(app.soundboardPanel.playingUrl || "") : "",
                gate: { allowed: gate.allowed !== false, hint: String(gate.hint || "") }
            },
            people: people,
            recent: (app.mSbRecent || []).slice(0, 8),
            favourites: favs,
            party: host.partySnap(String(c.roomId))
        }
    }
    property string _last: ""
    function publish(force) {
        if (!host.app || !host.app.padCtl || host.app.mLoggedIn !== true) return
        var s = JSON.stringify(host.snapshot())
        if (!force && s === host._last) return
        host._last = s
        host.app.padCtl.publishState(s)
    }
    // ≤ 4×/s, and only a change goes out (the engine paces what reaches each Pad: ≤ 4/s, talking-only ≤ 2/s). Only
    // around a call (GPU.md: no timer that wakes the app forever): in one, and one more tick after it, which publishes
    // `call: null` (the Pads get "left the call"). Not tied to focus: the PC is rarely the focused window then.
    Timer {
        interval: 250; repeat: true
        running: !!host.app && host.app.mLoggedIn === true && !host.recording && (host.app.mInCall === true || host._callRoom.length > 0)
        onTriggered: host.publish(false)
    }
}
