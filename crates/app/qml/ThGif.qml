// ThGif.qml — an animated image (GIF / animated WebP / APNG) that plays by the low-GPU rule (11.0, contract GPU.md
// §2.6, R12 — the web's ui/AnimatedImage.tsx, the same states; they looped forever at their own rate, ListView's cache
// just off-screen included):
//   • it plays while the window is focused AND the image is in view, for at most 3 loops or 15 s (the file's own smaller
//     loop count wins — QMovie stops by itself), then rests on its FIRST frame;
//   • hovering it replays a run (a click too, where the click isn't taken — `clickPlays`); leaving the view or the window
//     losing the focus stops it at once (the first frame again) — it autoplays again when both come back, unless its run
//     had finished (then only after it left the view and came back, or a hover);
//   • Reduce motion (the app's or the OS's): it never plays on its own — first frame; a click plays ONE loop
//     (`playOnMount: "one"` = the click that opened it, the lightbox);
//   • Settings → Appearance → "Play GIFs automatically" (Motion.gifAutoplay, pref `gif-autoplay`, default on) off: the
//     first frame until a hover / click;
//   • no mask or effect layer (rounded corners come from a CornerMask where needed).
// A still image (one frame) draws exactly as before.
import QtQuick
import dev.tinhat.app   // Motion: the window state, the motion switch, the GIF pref

AnimatedImage {
    id: g
    property bool inView: true            // the caller narrows it (a timeline delegate inside the viewport)
    property bool clickPlays: true        // false where a click opens something (the timeline → the lightbox)
    property string playOnMount: ""       // "one": the mount itself was a click (the lightbox)
    property string runKind: ""           // "" = no run · "auto" · "user" (hover / click) · "one" (a click under Reduce motion)
    property bool done: false             // the last run finished: no autoplay until the image leaves the view
    property int _loops: 0
    property int _last: -1
    property bool _mounted: false
    readonly property bool animated: g.status === Image.Ready && g.frameCount > 1
    playing: false
    // 10.8 fix: every decision below reads its inputs AT THE SOURCE. `animated` (above), Motion.focused and Motion.animate
    // are bindings, and a binding is not guaranteed to have re-evaluated inside a change handler of its own dependency
    // (measured on the islands build b1f29c40: in onStatusChanged status Ready + frameCount 60, `animated` still false — the
    // autoplay check returned and nothing ran it again, so a GIF that finished loading while already in view never played;
    // the islands timeline is sized before its rows load, so that became the normal case). Likewise Motion.focused inside
    // onWinStateChanged, and Motion.animate, which AppCore may write before or after winState. The base values: the
    // image's status + frameCount, Motion.winState / reducedMotion / gifAutoplay — each change of one re-evaluates.
    function isAnimated() { return g.status === Image.Ready && g.frameCount > 1 }
    function _focused() { return Motion.winState === "focused" }
    function _animate() { return Motion.winState === "focused" && !Motion.reducedMotion }   // R2: focused AND NOT reduce motion
    // 10.8 (the CPU/memory track, notes/cpu-idle.md §3): a local file (m.image — decrypted to the media cache) is decoded
    // frame by frame while a run plays instead of keeping EVERY decoded frame (QMovie::CacheAll) for as long as the
    // delegate lives — w × h × 4 bytes × frames, 30–150 MB for a typical Giphy-sized GIF, per GIF in the loaded
    // timeline. Remote sources (a GIF link's embed) keep the cache: QMovie cannot rewind a finished network reply, so
    // without it the loop / the rest on frame 0 would break.
    cache: !String(g.source).startsWith("file:")

    function _rest() {
        runCap.stop()
        g.runKind = ""
        if (g.playing) g.playing = false
        if (g.frameCount > 0 && g.currentFrame !== 0) g.currentFrame = 0
    }
    function startRun(kind) {
        if (!g.isAnimated()) return
        g.runKind = ""
        if (g.playing) g.playing = false
        g._loops = 0
        g._last = -1
        g.currentFrame = 0
        g.runKind = kind
        g.playing = true
        runCap.restart()
    }
    function stopRun(finished) {
        g._rest()
        if (finished) g.done = true
    }
    Timer { id: runCap; interval: 15000; onTriggered: g.stopRun(true) }   // GIF_MAX_MS
    onFrameChanged: {
        if (g.runKind === "" || !g.playing) return
        if (g._last >= 0 && g.currentFrame < g._last) {                    // wrapped: one loop played
            g._loops++
            if (g._loops >= (g.runKind === "one" ? 1 : 3)) { g.stopRun(true); return }   // GIF_MAX_LOOPS
        }
        g._last = g.currentFrame
    }
    onPlayingChanged: if (!g.playing && g.runKind !== "") g.stopRun(true)   // the file's own play count ran out

    function evaluate() {
        if (!g.isAnimated()) return
        g._mountRun()
        // focus lost / out of view: every run stops; Reduce motion on: every run but the one loop a click plays under it
        if (g.runKind !== "" && (!g._focused() || !g.inView || !g.visible || (Motion.reducedMotion && g.runKind !== "one"))) {
            g.stopRun(false)
            return
        }
        if (g.runKind === "" && g._animate() && Motion.gifAutoplay && g.inView && g.visible && !g.done) g.startRun("auto")
    }
    // playOnMount "one" (the lightbox: the click that opened it): under Reduce motion its one loop plays once the image
    // is SHOWN — the lightbox sets the source before it shows itself, and a run started at load was cut by the same
    // evaluate() (not in view yet) and never came back. Each showing counts once (leaving the view re-arms it).
    function _mountRun() {
        if (g._mounted || g.playOnMount !== "one" || !g.inView || !g.visible) return
        g._mounted = true
        if (Motion.reducedMotion && g._focused()) g.startRun("one")
    }
    onInViewChanged: { if (!g.inView) { g.done = false; g._mounted = false } g.evaluate() }
    onVisibleChanged: g.evaluate()
    onSourceChanged: { g.done = false; g._mounted = false; g._rest() }   // a new source is a new image
    onFrameCountChanged: g.evaluate()   // (a load lands with statusChanged; a count known only later re-evaluates too)
    onStatusChanged: {
        if (g.status !== Image.Ready) return
        g._rest()
        g.evaluate()
    }
    Connections {
        target: Motion
        function onWinStateChanged() { g.evaluate() }
        function onAnimateChanged() { g.evaluate() }
        function onReducedMotionChanged() { g.evaluate() }
        function onGifAutoplayChanged() { g.evaluate() }
    }
    // hover replays a run — not under Reduce motion (there only a click plays, one loop)
    HoverHandler {
        onHoveredChanged: if (hovered && g.isAnimated() && g.runKind === "" && g._animate()) g.startRun("user")
    }
    TapHandler {
        enabled: g.clickPlays
        onTapped: if (g.isAnimated() && g.runKind === "" && g._focused()) g.startRun(Motion.reducedMotion ? "one" : "user")
    }
}
