//! lowgpu — the pure rules of the low-GPU contract (contract GPU.md §1–§2), a line-for-line port of Tin Hat Web's
//! `src/core/lowgpu.ts` (same constants; the tests below carry the same case names as `lowgpu.test.ts`). Used by the
//! app's QML through `LowGpuController` (crates/app/src/lowgpu_ctl.rs) and by the streamer's receive policy
//! ([`video_policy`]). No Qt, no I/O.
//!
//! ```text
//!   window state  focused | unfocused | hidden (compute_win_state)      GPU.md §1
//!   render state  what the rules below act on: unfocused draws as focused unless the saver is on (compute_render_state)
//!                 §1 amendment 10.8.1 — the option "Save GPU while Tin Hat isn't focused", default off
//!   animate       focused AND NOT reduce motion (animate_for)            §1
//!   duration      ONE token `dur` = 120 ms; named steps are fixed multiples of it; the cap is 400 ms (dur_ms)   §1
//!   timers        ≥ 1 s whenever not focused (interval_for)              §2.8
//!   caret         blinks 5 s after key / pointer / focus, then solid (CARET_BLINK_MS)   §2.1
//!   ring          unfocused AND NOT reduce motion AND < 25 s; 2 Hz two-state blink (ring_on, ring_high)   §2.3
//!   busy mark     static; ≤ 1 step per progress event, ≤ 4 steps/s, only while animate; "Still …" after 10 s   §2.5
//!   GIFs          ≤ 3 loops or 15 s while focused + in view, then the first frame (gif_run_ms)   §2.6
//!   level bars    ≤ 10 updates/s while animate, a short ease that stops at rest (wave_ease)   §2.7
//!   call video    unfocused: cameras → the low layer at ≤ 15 fps; hidden: released per track (video_policy)   §2.11
//! ```

/// The app-wide window state (GPU.md §1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WinState {
    Focused,
    Unfocused,
    Hidden,
}

impl WinState {
    pub fn as_str(self) -> &'static str {
        match self {
            WinState::Focused => "focused",
            WinState::Unfocused => "unfocused",
            WinState::Hidden => "hidden",
        }
    }
    /// "focused" | "unfocused" | "hidden"; anything else reads as focused (the state before the window exists).
    pub fn parse(s: &str) -> WinState {
        match s {
            "unfocused" => WinState::Unfocused,
            "hidden" => WinState::Hidden,
            _ => WinState::Focused,
        }
    }
}

/// The window state from what the app can see. `visible` = the main window is shown and exposed; `focused` = the main
/// window or one of our pop-out windows is the active window.
pub fn compute_win_state(visible: bool, focused: bool) -> WinState {
    if !visible {
        return WinState::Hidden;
    }
    if focused {
        WinState::Focused
    } else {
        WinState::Unfocused
    }
}

/// The motion switch: focused AND NOT reduce motion (reduce motion = the app's pref OR the platform setting).
pub fn animate_for(win: WinState, reduced: bool) -> bool {
    win == WinState::Focused && !reduced
}

// ---- the render state (10.8.1, GPU.md §1 amendment) ---------------------------------------------------------------

/// Settings → Appearance → Performance → "Save GPU while Tin Hat isn't focused" — the pref key. It is the 10.8.0 key
/// (then a calls-only switch that defaulted to on), kept because it can tell a choice from the default: only the
/// Settings switch ever writes it, so a stored "1" / "0" is the user's own choice and stays; absent = never touched =
/// the new default, OFF (2026-09-27: "for lower end machines … it just should be an option").
pub const UNFOCUSED_SAVER_PREF: &str = "call-unfocused-saver";
/// The option from its stored value: on only when the user turned it on ("1"; "true" read alike).
pub fn unfocused_saver_on(raw: Option<&str>) -> bool {
    matches!(raw, Some("1") | Some("true"))
}
/// The RENDER state: the window state every drawing rule acts on (the motion switch, GIFs, meters, timers, call video,
/// previews, widget / stats refresh). The real state, except that a visible-but-unfocused window renders as focused
/// unless the saver is on; hidden stays hidden either way (nothing of it is on screen). What depends on REAL focus —
/// the incoming-call ring, notifications / unread, the caret's 5 s rule, the call chrome — keeps the real state.
pub fn render_state_for(win: WinState, saver_on: bool) -> WinState {
    if win == WinState::Unfocused && !saver_on {
        WinState::Focused
    } else {
        win
    }
}
/// compute_win_state + render_state_for in one: the render state from what the app can see and the option.
pub fn compute_render_state(visible: bool, focused: bool, saver_on: bool) -> WinState {
    render_state_for(compute_win_state(visible, focused), saver_on)
}
/// The line both apps log once per change (tag `winstate`): "focused → unfocused (render: focused, option off)".
pub fn win_state_log_text(prev: WinState, next: WinState, render: WinState, saver_on: bool) -> String {
    let option = if saver_on { "on" } else { "off" };
    format!("{} → {} (render: {}, option {option})", prev.as_str(), next.as_str(), render.as_str())
}

// ---- the duration token -------------------------------------------------------------------------------------------

/// THE duration token `dur` and its named steps, all fixed multiples of it (short 5/6, slow 3/2, long 5/3,
/// verylong 10/3). 0 whenever `animate` is off. Nothing moves longer than `verylong` = 400 ms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DurStep {
    Dur,
    Short,
    Slow,
    Long,
    VeryLong,
}

impl DurStep {
    pub fn as_str(self) -> &'static str {
        match self {
            DurStep::Dur => "dur",
            DurStep::Short => "short",
            DurStep::Slow => "slow",
            DurStep::Long => "long",
            DurStep::VeryLong => "verylong",
        }
    }
    pub fn parse(s: &str) -> Option<DurStep> {
        Some(match s {
            "dur" => DurStep::Dur,
            "short" => DurStep::Short,
            "slow" => DurStep::Slow,
            "long" => DurStep::Long,
            "verylong" => DurStep::VeryLong,
            _ => return None,
        })
    }
}

/// The step's length in ms (web `DUR`).
pub fn dur_ms(step: DurStep) -> u32 {
    match step {
        DurStep::Dur => 120,
        DurStep::Short => 100,
        DurStep::Slow => 180,
        DurStep::Long => 200,
        DurStep::VeryLong => 400,
    }
}
pub const DUR_CAP_MS: u32 = 400;

/// The nearest step for a literal duration of the pre-contract code (the migration rule both apps used: nearest
/// step, ties to the shorter one; anything over 400 ms → verylong).
pub fn nearest_step(ms: f64) -> DurStep {
    let order = [DurStep::Short, DurStep::Dur, DurStep::Slow, DurStep::Long, DurStep::VeryLong];
    let mut best = DurStep::Short;
    for k in order {
        if (dur_ms(k) as f64 - ms).abs() < (dur_ms(best) as f64 - ms).abs() {
            best = k;
        }
    }
    best
}

// ---- timers (§2.8) ------------------------------------------------------------------------------------------------

/// A repeating timer's period in a window state: `focused_ms` while focused, else max(1000, the unfocused / hidden
/// period). The incoming-call ring's 250 ms steps are the one exempt timer (they don't use this). Web
/// `intervalFor(state, focusedMs, otherMs = focusedMs, hiddenMs = otherMs)`.
pub fn interval_for(state: WinState, focused_ms: u32, other_ms: u32, hidden_ms: u32) -> u32 {
    if state == WinState::Focused {
        return focused_ms;
    }
    1000u32.max(if state == WinState::Hidden { hidden_ms } else { other_ms })
}

// ---- the caret (§2.1) ---------------------------------------------------------------------------------------------

pub const CARET_BLINK_MS: u32 = 5000;

// ---- the incoming-call ring (§2.3) --------------------------------------------------------------------------------

pub const RING_LIMIT_MS: f64 = 25_000.0;
pub const RING_STEP_MS: u32 = 250;
/// `ring` = ringing AND unfocused (visible, not focused) AND NOT reduce motion AND < 25 s since it started ringing.
pub fn ring_on(ringing_since: Option<f64>, now: f64, win: WinState, reduced: bool) -> bool {
    match ringing_since {
        Some(s) => win == WinState::Unfocused && !reduced && now - s >= 0.0 && now - s < RING_LIMIT_MS,
        None => false,
    }
}
/// The ring's two-state blink: 250 ms per state (2 Hz = 4 visual steps/s), `high` first.
pub fn ring_high(ringing_since: f64, now: f64) -> bool {
    ((0f64.max(now - ringing_since)) / RING_STEP_MS as f64).floor() as i64 % 2 == 0
}
/// The pulse layer's opacity: rest 0.12 (static — focused, reduce motion, after 25 s), ringing 0.22 / 0.06.
pub const RING_OPACITY_REST: f64 = 0.12;
pub const RING_OPACITY_HIGH: f64 = 0.22;
pub const RING_OPACITY_LOW: f64 = 0.06;

// ---- the busy mark (§2.5) -----------------------------------------------------------------------------------------

/// At most one visual step per 250 ms (≤ 4 steps/s).
pub const BUSY_STEP_MS: u32 = 250;
/// After this long without a progress event the label changes once ("Still loading…").
pub const BUSY_STILL_MS: u32 = 10_000;
/// The house busy mark = Controls.BusyIndicator's ten dots. Step -1 (no progress yet) = all ten outlined (the resting
/// glyph); step k ≥ 0 fills dot i when (k − i) mod 20 < 10 and i ≤ k (the native fill/empty order, one dot per step
/// instead of one per 100 ms).
pub fn busy_dot_filled(i: i64, step: i64) -> bool {
    if step < 0 || i > step {
        return false;
    }
    ((step - i) % 20 + 20) % 20 < 10
}
/// "Loading…" → "Still loading…" (the label changes once after BUSY_STILL_MS without progress). A label that already
/// starts with the word "Still" (any case) stays.
pub fn still_label(label: &str) -> String {
    if label.is_empty() {
        return String::new();
    }
    // JS /^still\b/i: the five letters, then the end or a non-word character ([A-Za-z0-9_] are word characters)
    let b = label.as_bytes();
    if b.len() >= 5 && b[..5].eq_ignore_ascii_case(b"still") {
        match b.get(5) {
            None => return label.to_string(),
            Some(c) if !(c.is_ascii_alphanumeric() || *c == b'_') => return label.to_string(),
            _ => {}
        }
    }
    let mut chars = label.chars();
    let first = chars.next().map(|c| c.to_lowercase().collect::<String>()).unwrap_or_default();
    format!("Still {}{}", first, chars.as_str())
}

// ---- GIFs / animated WebP (§2.6) ----------------------------------------------------------------------------------

pub const GIF_MAX_LOOPS: i64 = 3;
pub const GIF_MAX_MS: f64 = 15_000.0;
/// How long one autoplay run lasts: at most 3 loops (or the file's own smaller loop count) and at most 15 s.
/// `loop_ms` ≤ 0 (unknown) → 15 s. `loops` 0 = the file loops forever. `one` = a single loop (the click under
/// reduce motion).
pub fn gif_run_ms(loop_ms: f64, loops: i64, one: bool) -> f64 {
    if !(loop_ms > 0.0) {
        return GIF_MAX_MS;
    }
    let n = if one {
        1
    } else if loops > 0 {
        loops.min(GIF_MAX_LOOPS)
    } else {
        GIF_MAX_LOOPS
    };
    GIF_MAX_MS.min(loop_ms * n as f64)
}
/// Browsers play a GIF frame delay of 0 or 10 ms as 100 ms (Chromium/Firefox/Safari agree).
pub fn gif_frame_ms(centiseconds: u32) -> u32 {
    if centiseconds <= 1 {
        100
    } else {
        centiseconds * 10
    }
}

// ---- level bars, waveforms, speaking borders (§2.7) --------------------------------------------------------------

/// ≤ 10 updates/s while animate.
pub const WAVE_TICK_MS: u32 = 100;
/// One ease step of a bar's shown amplitude toward its target: halves the gap each tick, lands exactly when the gap
/// is < 0.015 (so the ticker stops at rest).
pub fn wave_ease(shown: f64, target: f64) -> f64 {
    let d = target - shown;
    if d.abs() < 0.015 {
        target
    } else {
        shown + d * 0.5
    }
}
/// Speaking = the level is over the threshold (the native 0.02).
pub const SPEAKING_LEVEL: f64 = 0.02;

// ---- calls while not focused (§2.11) ------------------------------------------------------------------------------

pub const UNFOCUSED_CAM_FPS: u32 = 15;
/// Stats refresh: every `focused_ms` while focused, every 5 s otherwise (DESIGN: "call stats every 5 s").
pub const UNFOCUSED_STATS_MS: u32 = 5000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VideoPolicyIn {
    /// the RENDER state (render_state_for): with the saver off an unfocused window arrives here as focused
    pub win: WinState,
    /// Settings → Appearance → Performance → "Save GPU while Tin Hat isn't focused" (default off since 10.8.1)
    pub policy_on: bool,
    pub is_share: bool,
    /// a visible Tin Hat pop-out window (the tile PiP) shows this track
    pub in_visible_popout: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VideoPolicyOut {
    /// keep receiving + decoding (false = released at the SFU, no decode)
    pub enabled: bool,
    /// receive the server's low layer at ≤ UNFOCUSED_CAM_FPS instead of the tile-size layer
    pub low: bool,
}
/// Per remote video track. Hidden: released unless a visible pop-out shows it (per track — "hidden" is about what the
/// user can see). Unfocused (and the policy on): camera tracks take the low layer at ≤ 15 fps; shares keep their
/// quality (the watched share stays full). Focused: unchanged.
pub fn video_policy(p: VideoPolicyIn) -> VideoPolicyOut {
    if p.win == WinState::Hidden {
        return VideoPolicyOut { enabled: p.in_visible_popout, low: p.in_visible_popout && p.policy_on && !p.is_share };
    }
    if p.win == WinState::Unfocused && p.policy_on && !p.is_share {
        return VideoPolicyOut { enabled: true, low: true };
    }
    VideoPolicyOut { enabled: true, low: false }
}
/// Local previews (self-view camera, the share popover's preview) pause while hidden (nobody sees them — whatever the
/// option), and while unfocused when the policy is on (pass the render state: with the saver off an unfocused window
/// renders as focused and its previews run).
pub fn preview_paused(win: WinState, policy_on: bool) -> bool {
    win == WinState::Hidden || (win == WinState::Unfocused && policy_on)
}

// The web's lowgpu.test.ts, case for case (same names).
#[cfg(test)]
mod tests {
    use super::WinState::{Focused, Hidden, Unfocused};
    use super::*;

    // window state + motion switch
    #[test]
    fn win_hidden_wins() {
        assert!(compute_win_state(false, true) == Hidden && compute_win_state(false, false) == Hidden);
    }
    #[test]
    fn win_focused() {
        assert_eq!(compute_win_state(true, true), Focused);
    }
    #[test]
    fn win_unfocused() {
        assert_eq!(compute_win_state(true, false), Unfocused);
    }
    #[test]
    fn animate_only_focused_unreduced() {
        assert!(animate_for(Focused, false) && !animate_for(Focused, true) && !animate_for(Unfocused, false) && !animate_for(Hidden, false));
    }

    // the render state (10.8.1: the unfocused rules only while the option is on; default off)
    #[test]
    fn saver_pref_default_off() {
        assert!(!unfocused_saver_on(None) && !unfocused_saver_on(Some("")) && !unfocused_saver_on(Some("x")) && UNFOCUSED_SAVER_PREF == "call-unfocused-saver");
    }
    #[test]
    fn saver_pref_explicit_kept() {
        assert!(unfocused_saver_on(Some("1")) && unfocused_saver_on(Some("true")) && !unfocused_saver_on(Some("0")));
    }
    #[test]
    fn render_saver_off_unfocused_draws_focused() {
        assert!(compute_render_state(true, false, false) == Focused && render_state_for(Unfocused, false) == Focused);
    }
    #[test]
    fn render_saver_on_keeps_unfocused() {
        assert!(compute_render_state(true, false, true) == Unfocused && render_state_for(Unfocused, true) == Unfocused);
    }
    #[test]
    fn render_focused_either_way() {
        assert!(compute_render_state(true, true, false) == Focused && compute_render_state(true, true, true) == Focused);
    }
    #[test]
    fn render_hidden_either_way() {
        assert!(compute_render_state(false, true, false) == Hidden && compute_render_state(false, false, false) == Hidden && compute_render_state(false, false, true) == Hidden);
    }
    #[test]
    fn render_rules_saver_off_as_focused() {
        let r = compute_render_state(true, false, false);
        let v = video_policy(VideoPolicyIn { win: r, policy_on: false, is_share: false, in_visible_popout: false });
        assert!(animate_for(r, false) && interval_for(r, 120, 1000, 1000) == 120 && (v.enabled, v.low) == (true, false) && !preview_paused(r, false));
    }
    #[test]
    fn render_rules_saver_on_unfocused() {
        let r = compute_render_state(true, false, true);
        let v = video_policy(VideoPolicyIn { win: r, policy_on: true, is_share: false, in_visible_popout: false });
        assert!(!animate_for(r, false) && interval_for(r, 120, 1000, 1000) == 1000 && (v.enabled, v.low) == (true, true) && preview_paused(r, true));
    }
    #[test]
    fn ring_keeps_real_focus() {
        // the ring is fed the REAL state: it rings while unfocused even though the window renders as focused
        let (real, render) = (compute_win_state(true, false), compute_render_state(true, false, false));
        assert!(render == Focused && ring_on(Some(T0), T0 + 1000.0, real, false) && !ring_on(Some(T0), T0 + 1000.0, compute_win_state(true, true), false));
    }
    #[test]
    fn winstate_log_line() {
        assert_eq!(win_state_log_text(Focused, Unfocused, Focused, false), "focused → unfocused (render: focused, option off)");
        assert_eq!(win_state_log_text(Unfocused, Unfocused, Unfocused, true), "unfocused → unfocused (render: unfocused, option on)");
    }

    // the duration token
    #[test]
    fn dur_steps_are_multiples_of_dur() {
        let d = dur_ms(DurStep::Dur) as f64;
        assert_eq!(dur_ms(DurStep::Short) as f64, d * 5.0 / 6.0);
        assert_eq!(dur_ms(DurStep::Slow) as f64, d * 3.0 / 2.0);
        assert_eq!(dur_ms(DurStep::Long) as f64, (d * 5.0 / 3.0).round());
        assert_eq!(dur_ms(DurStep::VeryLong) as f64, (d * 10.0 / 3.0).round());
    }
    #[test]
    fn dur_cap_400() {
        let all = [DurStep::Dur, DurStep::Short, DurStep::Slow, DurStep::Long, DurStep::VeryLong];
        assert!(all.iter().all(|s| dur_ms(*s) <= DUR_CAP_MS) && dur_ms(DurStep::VeryLong) == 400);
    }
    #[test]
    fn dur_nearest_step() {
        let got: Vec<&str> = [80., 90., 110., 120., 140., 150., 160., 170., 180., 240., 250., 260., 470., 520., 900.]
            .iter()
            .map(|ms| nearest_step(*ms).as_str())
            .collect();
        assert_eq!(
            got,
            ["short", "short", "short", "dur", "dur", "dur", "slow", "slow", "slow", "long", "long", "long", "verylong", "verylong", "verylong"]
        );
    }

    // timers
    #[test]
    fn timer_focused_as_asked() {
        assert_eq!(interval_for(Focused, 250, 5000, 5000), 250);
    }
    #[test]
    fn timer_unfocused_floor_1s() {
        assert!(interval_for(Unfocused, 250, 250, 250) == 1000 && interval_for(Unfocused, 250, 400, 400) == 1000 && interval_for(Unfocused, 3000, 5000, 5000) == 5000);
    }
    #[test]
    fn timer_hidden() {
        assert!(interval_for(Hidden, 1000, 5000, 5000) == 5000 && interval_for(Hidden, 100, 300, 60000) == 60000);
    }

    // caret
    #[test]
    fn caret_5s() {
        assert_eq!(CARET_BLINK_MS, 5000);
    }

    // the ring
    const T0: f64 = 1_000_000.0;
    #[test]
    fn ring_unfocused_only() {
        assert!(ring_on(Some(T0), T0 + 1000.0, Unfocused, false) && !ring_on(Some(T0), T0 + 1000.0, Focused, false) && !ring_on(Some(T0), T0 + 1000.0, Hidden, false));
    }
    #[test]
    fn ring_reduce_motion_static() {
        assert!(!ring_on(Some(T0), T0 + 1000.0, Unfocused, true));
    }
    #[test]
    fn ring_25s_limit() {
        assert!(ring_on(Some(T0), T0 + 24_999.0, Unfocused, false) && !ring_on(Some(T0), T0 + 25_000.0, Unfocused, false));
    }
    #[test]
    fn ring_not_ringing() {
        assert!(!ring_on(None, T0, Unfocused, false));
    }
    #[test]
    fn ring_2hz_two_state() {
        let got: Vec<bool> = [0., 249., 250., 499., 500., 750., 1000.].iter().map(|d| ring_high(T0, T0 + d)).collect();
        assert_eq!(got, [true, true, false, false, true, false, true]);
    }
    #[test]
    fn ring_4_steps_per_s() {
        assert_eq!(RING_STEP_MS, 250);
    }

    // busy mark
    fn dots(step: i64) -> Vec<bool> {
        (0..10).map(|i| busy_dot_filled(i, step)).collect()
    }
    #[test]
    fn busy_rest_all_outlined() {
        assert!(dots(-1).iter().all(|f| !f));
    }
    #[test]
    fn busy_fill_then_empty() {
        assert_eq!(dots(0), [true, false, false, false, false, false, false, false, false, false]);
    }
    #[test]
    fn busy_step9_all() {
        assert!(dots(9).iter().all(|f| *f));
    }
    #[test]
    fn busy_step10_first_empties() {
        assert!(!busy_dot_filled(0, 10) && busy_dot_filled(1, 10) && busy_dot_filled(9, 10));
    }
    #[test]
    fn busy_step18_last_only() {
        assert_eq!(dots(18), [false, false, false, false, false, false, false, false, false, true]);
    }
    #[test]
    fn busy_step19_empty_step20_restarts() {
        assert!(dots(19).iter().all(|f| !f) && busy_dot_filled(0, 20) && !busy_dot_filled(1, 20));
    }
    #[test]
    fn busy_step_cap_4_per_s() {
        assert!(BUSY_STEP_MS == 250 && BUSY_STILL_MS == 10_000);
    }
    #[test]
    fn busy_still_label() {
        assert_eq!(still_label("Loading…"), "Still loading…");
        assert_eq!(still_label("Reconnecting…"), "Still reconnecting…");
        assert_eq!(still_label("Still loading…"), "Still loading…");
        assert_eq!(still_label(""), "");
    }

    // GIFs
    #[test]
    fn gif_three_loops() {
        assert_eq!(gif_run_ms(1000.0, 0, false), 3000.0);
    }
    #[test]
    fn gif_15s_cap() {
        assert_eq!(gif_run_ms(8000.0, 0, false), 15_000.0);
    }
    #[test]
    fn gif_own_loop_count() {
        assert!(gif_run_ms(1000.0, 1, false) == 1000.0 && gif_run_ms(1000.0, 2, false) == 2000.0 && gif_run_ms(1000.0, 5, false) == 3000.0);
    }
    #[test]
    fn gif_unknown_length() {
        assert_eq!(gif_run_ms(0.0, 0, false), 15_000.0);
    }
    #[test]
    fn gif_one_loop_click() {
        assert_eq!(gif_run_ms(1200.0, 0, true), 1200.0);
    }
    #[test]
    fn gif_frame_delay_clamp() {
        assert!(gif_frame_ms(0) == 100 && gif_frame_ms(1) == 100 && gif_frame_ms(2) == 20 && gif_frame_ms(7) == 70);
    }

    // level bars
    #[test]
    fn wave_10_per_s() {
        assert_eq!(WAVE_TICK_MS, 100);
    }
    #[test]
    fn wave_ease_halves() {
        assert!(wave_ease(0.0, 1.0) == 0.5 && wave_ease(0.5, 1.0) == 0.75);
    }
    #[test]
    fn wave_ease_lands() {
        assert!(wave_ease(0.99, 1.0) == 1.0 && wave_ease(0.01, 0.0) == 0.0);
    }
    #[test]
    fn wave_ease_stops_at_rest() {
        let mut s = 0.9;
        let mut steps = 0;
        while s != 0.0 && steps < 100 {
            s = wave_ease(s, 0.0);
            steps += 1;
        }
        assert!(s == 0.0 && steps <= 8, "{steps} steps");
    }

    // call video
    fn vp(win: WinState, policy_on: bool, is_share: bool, in_visible_popout: bool) -> (bool, bool) {
        let o = video_policy(VideoPolicyIn { win, policy_on, is_share, in_visible_popout });
        (o.enabled, o.low)
    }
    #[test]
    fn video_focused_full() {
        assert!(vp(Focused, true, false, false) == (true, false) && vp(Focused, true, true, false) == (true, false));
    }
    #[test]
    fn video_unfocused_camera_low() {
        assert_eq!(vp(Unfocused, true, false, false), (true, true));
    }
    #[test]
    fn video_unfocused_share_full() {
        assert_eq!(vp(Unfocused, true, true, false), (true, false));
    }
    #[test]
    fn video_unfocused_policy_off() {
        assert_eq!(vp(Unfocused, false, false, false), (true, false));
    }
    #[test]
    fn video_hidden_released() {
        assert!(vp(Hidden, true, false, false) == (false, false) && vp(Hidden, true, true, false) == (false, false));
    }
    #[test]
    fn video_hidden_popout_keeps_track() {
        assert!(vp(Hidden, true, false, true) == (true, true) && vp(Hidden, true, true, true) == (true, false));
    }
    #[test]
    fn video_cam_fps_15() {
        assert!(UNFOCUSED_CAM_FPS == 15 && UNFOCUSED_STATS_MS == 5000);
    }
    #[test]
    fn preview_pause() {
        assert!(preview_paused(Unfocused, true) && preview_paused(Hidden, true) && !preview_paused(Focused, true) && !preview_paused(Unfocused, false));
    }
    #[test]
    fn preview_pause_hidden_always() {
        assert!(preview_paused(Hidden, false) && !preview_paused(Focused, false));
    }
}
