pragma Singleton
// NowPlaying.qml — one player at a time (QOL A00.1). A voice note, an audio clip, a video or an embedded video that
// starts playing pauses the one that was playing, and starting a voice recording pauses whatever plays (timeline audio
// must not bleed into the microphone).
//   newId(kind)   — a fresh id for one player instance (the same event can be on screen in two views)
//   playing(id)   — this player started (or resumed); the one playing before is asked to pause
//   stopped(id)   — this player paused / stopped / ended / went away
//   recording()   — a recording is about to start: whatever plays is asked to pause
//   pauseRequested(id) — a player pauses itself when its id is named
// SHARED LOGIC: npStep between the markers is Tin Hat Web's nowPlaying.ts npStep, rule for rule; tools/now-playing-test.mjs
// runs it on the shared vectors (crates/app/src/testvectors/now-playing.json).
import QtQuick

QtObject {
    id: np

    signal pauseRequested(string id)

    // BEGIN now-playing pure (tools/now-playing-test.mjs)
    /// One step of the rule: {st, pause}; st = {cur: id or null}, op = {play: id} | {end: id} | {record: true};
    /// pause = the player id to pause, or null.
    function npStep(st, op) {
        var cur = st && st.cur ? st.cur : null
        if (op && typeof op.play === "string") return { st: { cur: op.play }, pause: cur && cur !== op.play ? cur : null }
        if (op && typeof op.end === "string") return { st: { cur: cur === op.end ? null : cur }, pause: null }
        if (op && op.record === true) return { st: { cur: null }, pause: cur }
        return { st: { cur: cur }, pause: null }
    }
    // END now-playing pure

    property var _st: ({ cur: null })
    property int _seq: 0
    function _apply(op) {
        var r = npStep(np._st, op)
        np._st = r.st
        if (r.pause) np.pauseRequested(r.pause)
    }
    function newId(kind) { np._seq += 1; return kind + ":" + np._seq }
    function playing(id) { if (id) np._apply({ play: id }) }
    function stopped(id) { if (id) np._apply({ end: id }) }
    function recording() { np._apply({ record: true }) }
}
