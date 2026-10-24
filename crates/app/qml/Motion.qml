pragma Singleton
// Motion.qml — the app-wide window state and motion switch (11.0, contract GPU.md §1; the web's core/winstate.ts +
// ui/motion.ts). AppCore binds its inputs at start (LowGpuController → tinhat_widgets::lowgpu); everything that moves
// reads it — the main window's components, the pop-out windows and the fallback themes alike, with no plumbing:
//   winState   the RENDER state "focused" | "unfocused" | "hidden" — what every rule below acts on (10.8.1, GPU.md §1
//              amendment): the real state, except that an unfocused window renders as "focused" unless Settings →
//              Appearance → Performance → "Save GPU while Tin Hat isn't focused" is on (default off)
//   realWinState  the REAL state — the main window or a pop-out of ours is the active window / neither / the main
//              window is not shown or not exposed (minimised, unmapped, suspended). Only for what needs real focus
//              (the incoming-call ring); everything that draws reads winState / focused / animate.
//   animate    focused AND NOT reduce motion (the app's switch OR the platform's). While it is off every transition's
//              duration is 0 — a change snaps into place — and nothing loops (GPU.md §1 "motion kinds").
//   dur …      THE duration token (dur = 120 ms) and its named steps, fixed multiples of it: short 100, slow 180,
//              long 200, verylong 400 = the cap. All 0 while `animate` is off. No literal durations anywhere: a
//              pre-contract literal became the NEAREST step (ties to the shorter; > 400 → verylong).
import QtQuick

QtObject {
    property string winState: "focused"
    property string realWinState: "focused"
    property bool reducedMotion: false
    property bool animate: true
    // the steps' lengths — AppCore fills them from LowGpuController.durMs (tinhat_widgets::lowgpu::dur_ms)
    property int stepDur: 120
    property int stepShort: 100
    property int stepSlow: 180
    property int stepLong: 200
    property int stepVeryLong: 400
    // 11.0 step 5 (THEMES-UI.md §15.2): the theme editor's Pick tool swaps the probe colours in and back within one call;
    // while it does, every duration is 0 so no Behavior animates towards or away from a probe colour
    property bool probing: false
    readonly property int dur: animate && !probing ? stepDur : 0
    readonly property int durShort: animate && !probing ? stepShort : 0
    readonly property int durSlow: animate && !probing ? stepSlow : 0
    readonly property int durLong: animate && !probing ? stepLong : 0
    readonly property int durVeryLong: animate && !probing ? stepVeryLong : 0
    readonly property bool focused: winState === "focused"
    // Settings → Appearance → Motion → "Play GIFs automatically" (pref `gif-autoplay`, default on; GPU.md §2.6, ThGif)
    property bool gifAutoplay: true
    readonly property bool hidden: winState === "hidden"
}
