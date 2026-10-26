// PadHost.qml — Pad's receiver glue (docs/PAD.md; pad_ctl.rs / pad_engine.rs). Two jobs, no UI:
//   * the EXECUTOR: padCtl.exec(token, cmds) — a command my other verified device sent and the rules accepted — is
//     CLAIMED first (padCtl.claim: still that session, Pad on, the device not refused, the very call this device is in
//     right now, ≤ 1.5 s old; no = nothing runs, the refusal goes back from Rust), then runs the SAME functions this
//     device's own buttons run (the soundboard's startSound / stopCurrent, the mic button's
//     forceMicMuted(true) / toggleMic, the deafen button's applyDeafen, BRB's setAfk, the per-person menu's
//     pushVolume / setVolume / setMuted, Leave's leaveCall), so every gate a click meets still applies (the
//     soundboard's padGate / use_soundboard, the server-mute lock, push-to-talk). Then it publishes the new snapshot
//     and says executed(token): the answer carries the effect.
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
        }
        console.warn("[pad] unknown command", JSON.stringify(c))
        return false
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
            people.push({ u: e.u, talk: e.talk, micOff: e._mics > 0 && e._muted === e._mics, away: e.away, vol: vol, muted: muted })
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
            favourites: favs
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
