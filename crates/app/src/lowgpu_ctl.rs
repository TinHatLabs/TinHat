//! LowGpuController — the low-GPU contract's pure rules for QML (contract GPU.md §1–§2, modular UI step 3).
//!
//! Every rule lives in `tinhat_widgets::lowgpu` (the web's `src/core/lowgpu.ts`, same constants and test names);
//! this object only carries them across the QML boundary, so the QML never re-types a number or a formula:
//! the window state + motion switch (AppCore `realWinState` / `winState` = the render state / `animate`; 10.8.1: the
//! unfocused rules only while "Save GPU while Tin Hat isn't focused" is on), the duration token's steps, the timer floor,
//! the ring, the busy mark, the GIF run and the level-bar ease. Plus two platform facts: the OS "reduce motion"
//! setting (Windows: Settings → Accessibility → Animation effects; Linux: QML reads KDE's animation speed through
//! Kirigami.Units) and the frame meter switch (`TINHAT_FRAME_METER=1`: every window logs its frames/s — the gates).

use cxx_qt_lib::QString;
use tinhat_widgets::lowgpu::{self, DurStep, WinState};

#[cxx_qt::bridge]
pub mod qobject {
    extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[auto_cxx_name] // snake_case Rust -> camelCase QML
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(bool, platform_reduced_motion)] // QML: platformReducedMotion — the OS asks for no animations (Windows; read at start)
        #[qproperty(bool, frame_meter)] //             QML: frameMeter — TINHAT_FRAME_METER=1: log every window's frames/s
        type LowGpuController = super::LowGpuControllerRust;

        /// QML: computeWinState(visible, focused) → "focused" | "unfocused" | "hidden".
        #[qinvokable]
        fn compute_win_state(self: &LowGpuController, visible: bool, focused: bool) -> QString;
        /// QML: computeRenderState(visible, focused, saverOn) → the RENDER state (10.8.1): an unfocused window renders
        /// as "focused" unless the saver is on; hidden stays hidden.
        #[qinvokable]
        fn compute_render_state(self: &LowGpuController, visible: bool, focused: bool, saver_on: bool) -> QString;
        /// QML: unfocusedSaverOn(raw) — the option from its stored pref value (`call-unfocused-saver`): on only for "1".
        #[qinvokable]
        fn unfocused_saver_on(self: &LowGpuController, raw: &QString) -> bool;
        /// QML: winStateLogText(prev, next, render, saverOn) — the "[winstate]" line both apps log once per change.
        #[qinvokable]
        fn win_state_log_text(self: &LowGpuController, prev: &QString, next: &QString, render: &QString, saver_on: bool) -> QString;
        /// QML: animateFor(winState, reduced) — focused AND NOT reduce motion.
        #[qinvokable]
        fn animate_for(self: &LowGpuController, win: &QString, reduced: bool) -> bool;
        /// QML: durMs(step) — "dur" 120 | "short" 100 | "slow" 180 | "long" 200 | "verylong" 400 (0 for an unknown step).
        #[qinvokable]
        fn dur_ms(self: &LowGpuController, step: &QString) -> i32;
        /// QML: intervalFor(winState, focusedMs, otherMs, hiddenMs) — a repeating timer's period (≥ 1 s unless focused).
        #[qinvokable]
        fn interval_for(self: &LowGpuController, win: &QString, focused_ms: i32, other_ms: i32, hidden_ms: i32) -> i32;
        /// QML: ringOn(ringingSinceMs, nowMs, winState, reduced) — the incoming-call card pulses (ringingSinceMs < 0 = not ringing).
        #[qinvokable]
        fn ring_on(self: &LowGpuController, ringing_since: f64, now: f64, win: &QString, reduced: bool) -> bool;
        /// QML: ringHigh(ringingSinceMs, nowMs) — which half of the 2 Hz blink.
        #[qinvokable]
        fn ring_high(self: &LowGpuController, ringing_since: f64, now: f64) -> bool;
        /// QML: busyDotFilled(i, step) — the busy mark's dot i at step (−1 = at rest, all outlined).
        #[qinvokable]
        fn busy_dot_filled(self: &LowGpuController, i: i32, step: i32) -> bool;
        /// QML: stillLabel(label) — "Loading…" → "Still loading…".
        #[qinvokable]
        fn still_label(self: &LowGpuController, label: &QString) -> QString;
        /// QML: gifRunMs(loopMs, loops, one) — one autoplay run's length.
        #[qinvokable]
        fn gif_run_ms(self: &LowGpuController, loop_ms: f64, loops: i32, one: bool) -> f64;
        /// QML: waveEase(shown, target) — one ease step of a level bar.
        #[qinvokable]
        fn wave_ease(self: &LowGpuController, shown: f64, target: f64) -> f64;
        /// QML: previewPaused(winState, policyOn) — self-view / share preview pause (hidden always; unfocused with the saver).
        #[qinvokable]
        fn preview_paused(self: &LowGpuController, win: &QString, policy_on: bool) -> bool;
        /// QML: num(name) — a contract constant: caretBlinkMs, ringStepMs, ringLimitMs, ringRest, ringHigh, ringLow,
        /// busyStepMs, busyStillMs, gifMaxLoops, gifMaxMs, waveTickMs, speakingLevel, camFps, statsMs, durCap.
        #[qinvokable]
        fn num(self: &LowGpuController, name: &QString) -> f64;
        /// QML: logFrames(window, frames, secs) — the frame meter's line (only while frameMeter).
        #[qinvokable]
        fn log_frames(self: &LowGpuController, window: &QString, frames: i32, secs: f64);
        /// QML: setCaretReduced(on) — Reduce motion (the app's or the platform's): the text caret stays solid (GPU.md §2.1;
        /// otherwise it blinks for 5 s after input — the C++ input filter).
        #[qinvokable]
        fn set_caret_reduced(self: &LowGpuController, on: bool);
    }
}

pub struct LowGpuControllerRust {
    platform_reduced_motion: bool,
    frame_meter: bool,
}

impl Default for LowGpuControllerRust {
    fn default() -> Self {
        Self {
            platform_reduced_motion: crate::video::platform_reduced_motion(),
            frame_meter: std::env::var("TINHAT_FRAME_METER").map(|v| v == "1").unwrap_or(false),
        }
    }
}

fn win(s: &QString) -> WinState {
    WinState::parse(&s.to_string())
}

impl qobject::LowGpuController {
    fn compute_win_state(&self, visible: bool, focused: bool) -> QString {
        QString::from(lowgpu::compute_win_state(visible, focused).as_str())
    }
    fn compute_render_state(&self, visible: bool, focused: bool, saver_on: bool) -> QString {
        QString::from(lowgpu::compute_render_state(visible, focused, saver_on).as_str())
    }
    fn unfocused_saver_on(&self, raw: &QString) -> bool {
        lowgpu::unfocused_saver_on(Some(raw.to_string().as_str()))
    }
    fn win_state_log_text(&self, prev: &QString, next: &QString, render: &QString, saver_on: bool) -> QString {
        QString::from(lowgpu::win_state_log_text(win(prev), win(next), win(render), saver_on).as_str())
    }
    fn animate_for(&self, w: &QString, reduced: bool) -> bool {
        lowgpu::animate_for(win(w), reduced)
    }
    fn dur_ms(&self, step: &QString) -> i32 {
        DurStep::parse(&step.to_string()).map(|s| lowgpu::dur_ms(s) as i32).unwrap_or(0)
    }
    fn interval_for(&self, w: &QString, focused_ms: i32, other_ms: i32, hidden_ms: i32) -> i32 {
        let c = |v: i32| v.max(0) as u32;
        lowgpu::interval_for(win(w), c(focused_ms), c(other_ms), c(hidden_ms)) as i32
    }
    fn ring_on(&self, ringing_since: f64, now: f64, w: &QString, reduced: bool) -> bool {
        let since = if ringing_since >= 0.0 { Some(ringing_since) } else { None };
        lowgpu::ring_on(since, now, win(w), reduced)
    }
    fn ring_high(&self, ringing_since: f64, now: f64) -> bool {
        lowgpu::ring_high(ringing_since, now)
    }
    fn busy_dot_filled(&self, i: i32, step: i32) -> bool {
        lowgpu::busy_dot_filled(i as i64, step as i64)
    }
    fn still_label(&self, label: &QString) -> QString {
        QString::from(lowgpu::still_label(&label.to_string()).as_str())
    }
    fn gif_run_ms(&self, loop_ms: f64, loops: i32, one: bool) -> f64 {
        lowgpu::gif_run_ms(loop_ms, loops as i64, one)
    }
    fn wave_ease(&self, shown: f64, target: f64) -> f64 {
        lowgpu::wave_ease(shown, target)
    }
    fn preview_paused(&self, w: &QString, policy_on: bool) -> bool {
        lowgpu::preview_paused(win(w), policy_on)
    }
    fn num(&self, name: &QString) -> f64 {
        match name.to_string().as_str() {
            "caretBlinkMs" => lowgpu::CARET_BLINK_MS as f64,
            "ringStepMs" => lowgpu::RING_STEP_MS as f64,
            "ringLimitMs" => lowgpu::RING_LIMIT_MS,
            "ringRest" => lowgpu::RING_OPACITY_REST,
            "ringHigh" => lowgpu::RING_OPACITY_HIGH,
            "ringLow" => lowgpu::RING_OPACITY_LOW,
            "busyStepMs" => lowgpu::BUSY_STEP_MS as f64,
            "busyStillMs" => lowgpu::BUSY_STILL_MS as f64,
            "gifMaxLoops" => lowgpu::GIF_MAX_LOOPS as f64,
            "gifMaxMs" => lowgpu::GIF_MAX_MS,
            "waveTickMs" => lowgpu::WAVE_TICK_MS as f64,
            "speakingLevel" => lowgpu::SPEAKING_LEVEL,
            "camFps" => lowgpu::UNFOCUSED_CAM_FPS as f64,
            "statsMs" => lowgpu::UNFOCUSED_STATS_MS as f64,
            "durCap" => lowgpu::DUR_CAP_MS as f64,
            other => {
                eprintln!("[lowgpu] unknown constant {other:?}");
                0.0
            }
        }
    }
    fn set_caret_reduced(&self, on: bool) {
        crate::video::caret_set_reduced(on);
    }
    fn log_frames(&self, window: &QString, frames: i32, secs: f64) {
        if !self.frame_meter {
            return;
        }
        let rate = if secs > 0.0 { frames as f64 / secs } else { 0.0 };
        eprintln!("[frames] {} {} in {:.1} s = {:.2}/s", window, frames, secs, rate);
    }
}
