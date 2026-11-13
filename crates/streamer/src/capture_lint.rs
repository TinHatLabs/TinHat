//! Round 9 (closed set of captures): a LINT, run with the streamer's unit tests, over the whole desktop tree
//! (crates/streamer/src + crates/app/src). It fails on
//!   A) any capture source element made outside the allow-list — a gst capture element name used as an element (in a
//!      launch string, an `ElementFactory::make`, a fragment), or a ScreenCast portal start;
//!   B) any PLAYING transition outside the allow-list (every capture start must go through `call::leased_play`, which
//!      gates the capture at its source under the lease (or its one-element / CLI forms leased_play_element, leased_start), or be a
//!      proven non-capture).
//! Each allow-list entry is (file, enclosing function) and names its gate. A NEW preview, meter or capture path anywhere
//! makes this test fail until it is gated and listed — `selftest_*` prove the scanner catches a planted one.
//! Round 10: no exemption class — each entry is a Lease (naming its gate) or a proven NoCapture (see [`Gate`]).
//! Records: share claude/tinhat-pad/WATCH-LEASE/README.md "ROUND 9" / "ROUND 10" (the same table).

/// gst capture source elements (any platform)
const CAPTURE_ELEMENTS: [&str; 21] = [
    "pipewiresrc", "v4l2src", "pulsesrc", "wasapi2src", "wasapisrc", "d3d11screencapturesrc", "ximagesrc", "autoaudiosrc",
    "autovideosrc", "ksvideosrc", "mfvideosrc", "dshowvideosrc", "directsoundsrc", "osxaudiosrc", "avfvideosrc", "libcamerasrc",
    "alsasrc", "dxgiscreencapsrc", "gdiscreencapsrc", "jackaudiosrc", "openalsrc",
];
/// ScreenCast portal starts, and calls of `source_head(`: the one function that writes the screen share's source
/// element (capture.rs, 12.1.12), so its callers build a capture source without naming the element
const PORTAL_CALLS: [&str; 3] = ["Screencast::new(", "pick_screencast(", "source_head("];

/// What an allow-list entry is. Round 10: there is NO exemption class. A capture is listed only with the LEASE gate it
/// goes through (named in the note, and the note must name `leased_play`, `leased_play_element`, `leased_start` or
/// `commit_opening`); anything else is listed as NOT A CAPTURE, and the tree test PROVES that mechanically:
///   rule A NoCapture = the element is made but the function never starts anything (no PLAYING/PAUSED in its body);
///   rule B NoCapture = the function starts a pipeline with no capture source (no capture element, no capture fragment
///   call in its body: playback, decoders, the recorder, synthetic test sources).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Gate {
    Lease(&'static str),
    NoCapture(&'static str),
}
use Gate::{Lease, NoCapture};

/// the capture fragments / builders: a rule-B NoCapture function may call none of them
const CAPTURE_FRAGMENTS: [&str; 10] = [
    "audio_src_fragment(", "camera_src_chain(", "src_fragment(", "tap_head(", "monitor_head(", "build_mic_pipeline(",
    "build_camera_capture_pipeline(", "build_screenshare_pipeline", "pick_screencast(", "Screencast::new(",
];
const LEASE_GATES: [&str; 4] = ["leased_play", "leased_play_element", "leased_start", "commit_opening"];

/// Rule A allow-list: (file, function, its gate)
pub const ALLOW_SOURCES: &[(&str, &str, Gate)] = &[
    ("crates/app/src/call_ctl.rs", "share_screen", Lease("the portal start on the user's Share press (refused while watching); the cast's frames flow only through leased_play (publish_screenshare, probe_stream_size)")),
    ("crates/app/src/stream.rs", "do_start_share", Lease("portal start of the StreamController share; its capture starts through leased_play (start_e2ee_screenshare)")),
    ("crates/streamer/src/bin/captest.rs", "main", Lease("dev binary captest: its capture pipeline starts through leased_start")),
    ("crates/streamer/src/bin/captest.rs", "portal_cast", Lease("dev binary captest: the cast feeds only main's pipeline, started through leased_start")),
    ("crates/streamer/src/call.rs", "audio_src_fragment", Lease("fragment: its pipelines start through leased_play (the mic, the mic meter, aux PipeWire, the speaker meter, the voice-note recorder) or leased_start (the CLI self-tests, the voice-note self-test)")),
    ("crates/streamer/src/call.rs", "build_mic_pipeline", Lease("builder: started only through leased_play (start_mic_capture) or leased_start (the CLI self-tests)")),
    ("crates/streamer/src/call.rs", "pipewiresrc_has_provide_clock", NoCapture("capability probe: element made in NULL to read a property, never started")),
    ("crates/streamer/src/call.rs", "wasapi2_process_loopback_fragment", NoCapture("capability probe: element made in NULL to read a property, never started")),
    ("crates/streamer/src/call.rs", "tap_head", Lease("fragment of the speaker-OUTPUT taps: the echo reference (inside the mic pipeline: leased_play; an in-place restart: leased_play_element) and the hear sniffer (leased_play)")),
    ("crates/streamer/src/call.rs", "aux_pipewire_pipeline_builds_links_and_has_sink", NoCapture("unit test: the pipeline is built, never started")),
    ("crates/streamer/src/camera.rs", "camera_src_chain", Lease("fragment: camera pipelines start through leased_play (publish_camera, restart_camera), gate_closed + commit_opening (the self-view) or leased_start (the CLI self-tests)")),
    ("crates/streamer/src/camera.rs", "probe_camera_caps", NoCapture("caps probe: READY then NULL, never PLAYING")),
    ("crates/streamer/src/capture.rs", "pick_screencast", Lease("the portal start itself (user press); its frames flow only through a leased_play pipewiresrc start")),
    ("crates/streamer/src/capture.rs", "probe_stream_size", Lease("the size probe: leased_play under its own lease (refused while watching)")),
    ("crates/streamer/src/capture.rs", "src_fragment", Lease("fragment: every screen pipeline starts through leased_play")),
    ("crates/streamer/src/capture.rs", "source_head", Lease("fragment: the share's source element (src_fragment, the size probe); every screen pipeline starts through leased_play")),
    ("crates/streamer/src/capture_windows.rs", "src_fragment", Lease("fragment: every screen pipeline starts through leased_play")),
    ("crates/streamer/src/lib.rs", "build_screenshare_pipeline_with_token", Lease("builder for the tinhat-publish dev binary: started only in start_screenshare_publish through leased_start")),
    // 12.1.8/12.1.9 share tests, listed at the 12.1.9 merge (they predate the lease on their branches)
    ("crates/streamer/src/lk_publish.rs", "source_head_carries_the_max_framerate", NoCapture("unit test: a launch-line string, never started")),
    ("crates/streamer/src/lk_publish.rs", "dmabuf_head_accepts_only_dmabuf", NoCapture("unit test: a launch-line string and caps, never started")),
    ("crates/streamer/src/lk_publish.rs", "dmabuf_capture_gate", Lease("rig gate (TINHAT_DMABUF_GATE): its capture starts through leased_start")),
    ("crates/streamer/src/lk_publish.rs", "rate_cap_gate", Lease("rig gate (TINHAT_RATECAP_GATE): its capture starts through leased_start")),
    // 12.1.12 share stream-buffer tests
    ("crates/streamer/src/lk_publish.rs", "share_head_holds_no_compositor_buffer", NoCapture("unit test: the element is made in NULL to read its properties, never started")),
    ("crates/streamer/src/lk_publish.rs", "buffer_gate_head", Lease("rig gates (TINHAT_BUFGATE, TINHAT_BUFLOCK_NODE, TINHAT_STILL_NODE): their source head; their captures start through leased_play under their own lease")),
];
/// Rule B allow-list: (file, function, its gate)
pub const ALLOW_PLAYING: &[(&str, &str, Gate)] = &[
    ("crates/streamer/src/call.rs", "leased_play", Lease("THE gated start: leased_play (source gate + lease)")),
    ("crates/streamer/src/call.rs", "leased_play_element", Lease("THE gated start of one element in a running pipeline: leased_play_element (source gate + lease)")),
    ("crates/streamer/src/call.rs", "set_camera_muted", Lease("re-PLAYs a camera started through leased_play: its source gate stays closed after a Watch press")),
    ("crates/streamer/src/call.rs", "set_mic_muted", Lease("re-PLAYs a mic started through leased_play: its source gate stays closed after a Watch press")),
    ("crates/streamer/src/call.rs", "build_record_audio", NoCapture("recorder: decodes remote tracks")),
    ("crates/streamer/src/call.rs", "build_record_video", NoCapture("recorder: decodes remote tracks")),
    ("crates/streamer/src/call.rs", "spawn_playback_pipeline", NoCapture("remote-audio playback")),
    ("crates/streamer/src/call.rs", "start_aux_capture", NoCapture("a sound / music FILE or URL (uridecodebin); the aux PipeWire capture is publish_aux_pipewire, leased")),
    ("crates/app/src/qvideo.rs", "run_selftest_pipeline", NoCapture("CLI self-test: videotestsrc")),
    ("crates/streamer/src/memtile.rs", "run_probe_decode", NoCapture("decoder probe")),
    ("crates/streamer/src/memtile.rs", "set_paused", NoCapture("memory tile playback")),
    ("crates/streamer/src/call.rs", "press_in_gap", NoCapture("unit test (synthetic source)")),
    ("crates/streamer/src/call.rs", "run_once", NoCapture("unit test (synthetic source)")),
    ("crates/streamer/src/call.rs", "selfview_press", NoCapture("unit test (synthetic source)")),
    ("crates/streamer/src/call.rs", "round10_echo_reference_restart_is_gated_and_refused_after_the_press", NoCapture("unit test (synthetic source)")),
    ("crates/streamer/src/lk_publish.rs", "camera_quiet_hook_fires_after_frames_stop", NoCapture("unit test (synthetic source)")),
    ("crates/streamer/src/lk_publish.rs", "cpu_capture_keeps_60_from_a_5k_source", NoCapture("unit test (synthetic source)")),
    ("crates/streamer/src/lk_publish.rs", "drop", NoCapture("unit test guard (synthetic source)")),
    ("crates/streamer/src/lk_publish.rs", "run", NoCapture("unit test helper (synthetic source)")),
    ("crates/streamer/src/lk_publish.rs", "share_start_gate", NoCapture("unit test (synthetic source)")),
    ("crates/streamer/src/lk_publish.rs", "gl_dmabuf_tail_scales_to_i420", NoCapture("unit test (synthetic source: videotestsrc)")),
    ("crates/streamer/src/lk_publish.rs", "capped_head_negotiates_without_a_max_framerate_field", NoCapture("unit test (synthetic source: videotestsrc)")),
    ("crates/streamer/src/lk_publish.rs", "presized_cpu_capture_never_renegotiates_its_source", NoCapture("unit test (synthetic source: videotestsrc)")),
    ("crates/streamer/src/voicenote.rs", "decode_with", NoCapture("decoder: reads back a finished voice-note file (filesrc ! oggdemux ! opusdec)")),
    ("crates/streamer/src/voicenote.rs", "decode_failure_after_valid_audio_is_refused", NoCapture("unit test (synthetic source: audiotestsrc encoded into a file)")),
];

/// Voice notes (docs/VOICE-NOTES.md §4): the recorder builds its capture head from a capture FRAGMENT (no element name,
/// so rules A/B never see it). Rule C: in the voice-note file every function that calls a capture fragment is listed
/// here with its lease gate, the file starts its pipelines only through those gates, and no function in it starts
/// anything else that captures (rule B covers the PLAYING lines).
pub const VOICENOTE_FILE: &str = "crates/streamer/src/voicenote.rs";
pub const ALLOW_FRAGMENT_CALLERS: &[(&str, &str, Gate)] = &[
    ("crates/streamer/src/voicenote.rs", "build_recorder", Lease("builder of the voice-note recorder (Kind::VoiceNote lease): started only by start through leased_play (refused while watching / in a call), or by headless_voicenote_test through leased_start")),
];
/// the voice-note functions that start a recorder build, and the gate each must call
const VOICENOTE_STARTS: &[(&str, &str)] = &[("start", "leased_play("), ("headless_voicenote_test", "leased_start(")];

/// Rule C over the bodies: the failures.
pub fn voicenote_failures(bodies: &[(String, String, String)]) -> Vec<String> {
    let mut bad = class_failures_of(ALLOW_FRAGMENT_CALLERS, &[], bodies);
    let mine: Vec<&(String, String, String)> = bodies.iter().filter(|(f, _, _)| f == VOICENOTE_FILE).collect();
    for (f, func, body) in &mine {
        let calls = CAPTURE_FRAGMENTS.iter().any(|x| body.contains(x));
        let listed = ALLOW_FRAGMENT_CALLERS.iter().any(|(lf, ln, _)| lf == f && ln == func);
        if calls && !listed {
            bad.push(format!("C: {f} {func} builds a capture head but is not a listed, lease-gated builder"));
        }
        if body.contains("build_recorder(") && !body.contains("fn build_recorder(") {
            match VOICENOTE_STARTS.iter().find(|(n, _)| n == func) {
                Some((_, gate)) if body.contains(gate) => {}
                _ => bad.push(format!("C: {f} {func} starts a recorder build outside its lease gate")),
            }
        }
    }
    for (lf, ln, _) in ALLOW_FRAGMENT_CALLERS {
        if !mine.iter().any(|(f, n, b)| f == lf && n == ln && CAPTURE_FRAGMENTS.iter().any(|x| b.contains(x))) {
            bad.push(format!("C: stale entry {lf} {ln} (no capture fragment call in it)"));
        }
    }
    for (n, gate) in VOICENOTE_STARTS {
        if !mine.iter().any(|(_, f, b)| f == n && b.contains("build_recorder(") && b.contains(gate)) {
            bad.push(format!("C: {VOICENOTE_FILE} {n} no longer starts the recorder through {gate}..)"));
        }
    }
    bad
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub file: String,
    pub line: usize,
    pub func: String,
    pub what: String,
    pub rule: char,
}

fn fn_name(line: &str) -> Option<String> {
    let t = line.trim_start();
    let mut rest = t;
    for p in ["pub(crate) ", "pub(super) ", "pub ", "async ", "unsafe ", "const "] {
        if let Some(r) = rest.strip_prefix(p) {
            rest = r;
        }
    }
    for p in ["async ", "unsafe "] {
        if let Some(r) = rest.strip_prefix(p) {
            rest = r;
        }
    }
    let r = rest.strip_prefix("fn ")?;
    let name: String = r.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
    (!name.is_empty()).then_some(name)
}

fn element_used(line: &str, name: &str) -> bool {
    let mut from = 0;
    while let Some(i) = line[from..].find(name).map(|i| i + from) {
        let pre = line[..i].chars().last().unwrap_or(' ');
        let rest = line[i + name.len()..].trim_start_matches(' ');
        let next = rest.chars().next();
        let prop = rest.split_once('=').is_some_and(|(k, _)| !k.is_empty() && k.chars().all(|c| c.is_ascii_lowercase() || c == '-'));
        if !(pre.is_alphanumeric() || pre == '_') && (next.is_none() || matches!(next, Some('"' | '!' | '{' | '\\')) || prop) {
            return true;
        }
        from = i + name.len();
    }
    false
}

/// Scan one file's text. `file` is the repo-relative path.
pub fn scan(file: &str, text: &str) -> Vec<Hit> {
    let mut out = vec![];
    let mut func = String::from("(top)");
    for (i, line) in text.lines().enumerate() {
        if let Some(n) = fn_name(line) {
            func = n;
        }
        let t = line.trim_start();
        if t.starts_with("//") || line.contains("ElementFactory::find(") {
            continue;
        }
        for n in CAPTURE_ELEMENTS {
            if element_used(line, n) {
                out.push(Hit { file: file.into(), line: i + 1, func: func.clone(), what: n.into(), rule: 'A' });
            }
        }
        for p in PORTAL_CALLS {
            if line.contains(p) && fn_name(line).is_none() {
                out.push(Hit { file: file.into(), line: i + 1, func: func.clone(), what: p.into(), rule: 'A' });
            }
        }
        let playing = line.contains("State::Playing")
            && (line.contains("set_state") || line.contains("else {"))
            && !["current_state", "target_state", "pending_state", "==", "!="].iter().any(|x| line.contains(x));
        if playing {
            out.push(Hit { file: file.into(), line: i + 1, func: func.clone(), what: "PLAYING".into(), rule: 'B' });
        }
    }
    out
}

/// Each function's body text (per file + function name; same-named functions in one file are joined).
pub fn bodies(file: &str, text: &str) -> Vec<(String, String, String)> {
    let mut out: Vec<(String, String, String)> = vec![];
    let mut func = String::from("(top)");
    for line in text.lines() {
        if let Some(n) = fn_name(line) {
            func = n;
        }
        if line.trim_start().starts_with("//") {
            continue;
        }
        match out.iter_mut().find(|(f, n, _)| f == file && *n == func) {
            Some(e) => {
                e.2.push_str(line);
                e.2.push('\n');
            }
            None => out.push((file.to_string(), func.clone(), format!("{line}\n"))),
        }
    }
    out
}

/// Round 10: the entries' classes, checked against the bodies — a Lease note names a lease gate; a NoCapture entry is
/// proven not to be a capture (see [`Gate`]). Returns the failures.
pub fn class_failures(bodies: &[(String, String, String)]) -> Vec<String> {
    class_failures_of(ALLOW_SOURCES, ALLOW_PLAYING, bodies)
}
pub fn class_failures_of(
    sources: &[(&str, &str, Gate)],
    playing: &[(&str, &str, Gate)],
    bodies: &[(String, String, String)],
) -> Vec<String> {
    let mut bad = vec![];
    for (rule, list) in [('A', sources), ('B', playing)] {
        for (f, func, g) in list {
            let body: String = bodies.iter().filter(|(bf, bn, _)| bf == f && bn == func).map(|(_, _, b)| b.as_str()).collect();
            match g {
                Lease(note) => {
                    if !LEASE_GATES.iter().any(|x| note.contains(x)) {
                        bad.push(format!("{f} {func}: a Lease entry must name its lease gate ({note})"));
                    }
                    if rule == 'A' && scan(f, &body).iter().any(|h| h.rule == 'B') {
                        bad.push(format!("{f} {func}: a listed capture source starts itself (PLAYING outside a lease gate)"));
                    }
                }
                NoCapture(note) => {
                    if rule == 'A' && (body.contains("State::Playing") || body.contains("State::Paused")) {
                        bad.push(format!("{f} {func}: NoCapture ({note}) but it starts the element"));
                    }
                    if rule == 'B' {
                        let cap = scan(f, &body).iter().any(|h| h.rule == 'A') || CAPTURE_FRAGMENTS.iter().any(|x| body.contains(x));
                        if cap {
                            bad.push(format!("{f} {func}: NoCapture ({note}) but its body builds a capture source"));
                        }
                    }
                }
            }
            for word in ["never sent", "what we play"] {
                if format!("{g:?}").contains(word) {
                    bad.push(format!("{f} {func}: the round-9 \"{word}\" exemption is gone"));
                }
            }
        }
    }
    bad
}

/// Round 12 (launch-gate ruling A): taps on RECEIVED remote tracks are outside clause (2) only when proven to be fed by
/// received tracks alone. The desktop's two: the per-track playback level (`set_track_level`) and the "can they hear me"
/// receive tap (`hear::note_recv`). PROOF, over the function bodies:
///   P1 both are called only in `call.rs::build_playback`;
///   P2 there, their input is the decoded frame of `NativeAudioStream::new(rtc_track, …)` (`stream.next()`), `rtc_track`
///      is the function's own `RtcAudioTrack` parameter, never reassigned, and the body builds no capture source / local
///      track / output tap (no capture element, no capture fragment, no NativeAudioSource / LocalAudioTrack / capture_frame);
///   P3 every `build_playback(` / `spawn_playback_retry(` call passes `t.rtc_track()` of a `RemoteTrack::Audio(t)` bound by
///      the nearest TrackSubscribed match above it (no `let t` in between), or — inside spawn_playback_retry — its own
///      retained `track` parameter;
///   P4 the level table is written (`.insert(`) only by `set_track_level`.
pub fn remote_tap_failures(bodies: &[(String, String, String)]) -> Vec<String> {
    let mut bad = vec![];
    let is_def = |l: &str, name: &str| l.contains(&format!("fn {name}("));
    // P1
    for (f, func, b) in bodies {
        for l in b.lines() {
            for tap in ["set_track_level(", "note_recv("] {
                let name = tap.trim_end_matches('(');
                if l.contains(tap) && !is_def(l, name) && !(f == "crates/streamer/src/call.rs" && func == "build_playback") {
                    bad.push(format!("P1: {f} {func} feeds a remote-track tap ({name}) outside build_playback"));
                }
            }
        }
    }
    // P2
    let bp: String = bodies.iter().filter(|(f, n, _)| f == "crates/streamer/src/call.rs" && n == "build_playback").map(|x| x.2.as_str()).collect();
    for need in ["track: livekit::track::RemoteAudioTrack", "crate::remote_audio::RemoteAudioStream::new(&track,", "stream.next()", "set_track_level(&who, &category, frame)", "note_recv(&sid, frame)"] {
        if !bp.contains(need) {
            bad.push(format!("P2: build_playback no longer shows `{need}`"));
        }
    }
    let local: Vec<&str> = CAPTURE_ELEMENTS.iter().copied().chain(CAPTURE_FRAGMENTS.iter().copied()).chain(["NativeAudioSource", "LocalAudioTrack", "capture_frame(", "NativeAudioStream::new(", "let track", "track ="]).collect();
    for x in &local {
        let hit = if CAPTURE_ELEMENTS.contains(x) { bp.lines().any(|l| element_used(l, x)) } else { bp.contains(x) };
        if hit {
            bad.push(format!("P2: build_playback also has a local source / output tap / rebinding: `{x}`"));
        }
    }
    // P3
    for (f, func, b) in bodies {
        let lines: Vec<&str> = b.lines().collect();
        for (i, l) in lines.iter().enumerate() {
            for callee in ["build_playback(", "spawn_playback_retry("] {
                if !l.contains(callee) || is_def(l, callee.trim_end_matches('(')) {
                    continue;
                }
                let ok = if func == "spawn_playback_retry" && f == "crates/streamer/src/call.rs" {
                    l.contains("build_playback(track.clone()") && b.contains("track: livekit::track::RemoteAudioTrack") && !b.contains("let track")
                } else {
                    let arg_ok = l.contains("(t.clone()") || l.contains("pb.clone(), t.clone()");
                    let bound = lines[..i].iter().rposition(|x| x.contains("RemoteTrack::Audio(t)"));
                    arg_ok && bound.is_some_and(|k| !lines[k + 1..i].iter().any(|x| x.contains("let t ") || x.contains("let t:") || x.contains("let t=") || x.contains("let mut t ")))
                };
                if !ok {
                    bad.push(format!("P3: {f} {func} passes a track to {callee}..) that is not proven a received remote track: {}", l.trim()));
                }
            }
        }
    }
    // P4
    for (f, func, b) in bodies {
        if b.lines().any(|l| l.contains("track_levels()") && b.contains(".insert(") && l.contains("track_levels()")) && func != "set_track_level" {
            let ins = b.lines().collect::<Vec<_>>().windows(4).any(|w| w[0].contains("track_levels()") && w.iter().any(|x| x.contains(".insert(")));
            if ins {
                bad.push(format!("P4: {f} {func} writes the level table"));
            }
        }
    }
    bad
}

/// Hits not covered by the allow-lists.
pub fn violations(hits: &[Hit]) -> Vec<Hit> {
    hits.iter()
        .filter(|h| {
            let list = if h.rule == 'A' { ALLOW_SOURCES } else { ALLOW_PLAYING };
            !list.iter().any(|(f, func, _)| h.file == *f && h.func == *func)
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> Vec<(String, String)> {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut out = vec![];
        for d in ["crates/streamer/src", "crates/app/src"] {
            let mut stack = vec![root.join(d)];
            while let Some(p) = stack.pop() {
                for e in std::fs::read_dir(&p).expect("read_dir").flatten() {
                    let path = e.path();
                    if path.is_dir() {
                        stack.push(path);
                    } else if path.extension().is_some_and(|x| x == "rs") {
                        let rel = path.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
                        out.push((rel, std::fs::read_to_string(&path).expect("read")));
                    }
                }
            }
        }
        out
    }

    #[test]
    fn every_capture_source_and_start_in_the_tree_is_gated_or_listed() {
        let mut hits = vec![];
        let mut all_bodies = vec![];
        for (f, t) in tree() {
            if f.ends_with("capture_lint.rs") {
                continue; // (this file names the elements in its lists and its selftests)
            }
            hits.extend(scan(&f, &t));
            all_bodies.extend(bodies(&f, &t));
        }
        // (review aid: CAPTURE_LINT_DUMP=<file> writes the raw hit list, rule A then rule B, each with its entry's class)
        if let Ok(p) = std::env::var("CAPTURE_LINT_DUMP") {
            let cls = |h: &Hit| {
                let list = if h.rule == 'A' { ALLOW_SOURCES } else { ALLOW_PLAYING };
                list.iter().find(|(f, n, _)| h.file == *f && h.func == *n).map(|(_, _, g)| format!("{g:?}")).unwrap_or_else(|| "UNLISTED".into())
            };
            let mut s = String::new();
            for r in ['A', 'B'] {
                s.push_str(&format!("---- rule {r}\n"));
                for h in hits.iter().filter(|h| h.rule == r) {
                    s.push_str(&format!("{}:{}\t{}\t{}\t{}\n", h.file, h.line, h.func, h.what, cls(h)));
                }
            }
            std::fs::write(p, s).expect("dump");
        }
        let vn = voicenote_failures(&all_bodies);
        assert!(vn.is_empty(), "voice-note recorder not proven lease-gated (rule C):\n  {}", vn.join("\n  "));
        let rt = remote_tap_failures(&all_bodies);
        assert!(rt.is_empty(), "remote-track taps not proven fed only by received tracks:\n  {}", rt.join("\n  "));
        let bad = class_failures(&all_bodies);
        assert!(bad.is_empty(), "allow-list entries whose class does not hold:\n  {}", bad.join("\n  "));
        assert!(hits.len() > 20, "the scanner sees the tree ({} hits)", hits.len());
        let v = violations(&hits);
        assert!(v.is_empty(), "capture sources / starts outside the gated set (gate them through leased_play and list them):\n{}",
            v.iter().map(|h| format!("  {}:{} [{}] in fn {} ({})", h.file, h.line, h.rule, h.func, h.what)).collect::<Vec<_>>().join("\n"));
        // and every allow-list entry is still in use (no stale entry quietly allowing a future function of that name)
        for (f, func, _) in ALLOW_SOURCES.iter().chain(ALLOW_PLAYING.iter()) {
            assert!(hits.iter().any(|h| h.file == *f && h.func == *func), "stale allow-list entry {f} {func}");
        }
    }

    #[test]
    fn selftest_a_new_ungated_preview_fails_the_lint() {
        let planted = r#"
pub fn new_camera_preview() {
    let p = gst::parse::launch("v4l2src device=/dev/video0 ! videoconvert ! autovideosink").unwrap();
    let _ = p.set_state(gst::State::Playing);
}
"#;
        let v = violations(&scan("crates/app/src/new_feature.rs", planted));
        assert_eq!(v.iter().map(|h| h.rule).collect::<String>(), "AB", "{v:?}");
    }

    #[test]
    fn selftest_a_new_portal_start_and_a_new_mic_start_fail_the_lint() {
        let planted = "async fn quick_share() {\n    let cast = crate::capture::pick_screencast().await;\n}\nfn mic_test() {\n    let d = format!(\"pulsesrc device={x} ! fakesink\");\n    p.set_state(gst::State::Playing).ok();\n}\n";
        let v = violations(&scan("crates/streamer/src/call.rs", planted));
        assert_eq!(v.len(), 3, "{v:?}");
    }

    #[test]
    fn selftest_an_existing_name_in_a_log_line_or_a_comment_is_not_a_capture() {
        let planted = "fn x() {\n    // pipewiresrc fd= in a comment\n    eprintln!(\"[mic] head: pulsesrc (PulseAudio-compat)\");\n    let ours = gst::ElementFactory::find(\"pipewiresrc\");\n}\n";
        assert!(violations(&scan("crates/streamer/src/call.rs", planted)).is_empty());
    }

    #[test]
    fn selftest_round10_a_tap_listed_as_no_capture_fails_and_a_lease_note_must_name_its_gate() {
        // a speaker-output tap smuggled in as "not a capture": its body builds a monitor head
        let planted = "pub fn start_output_monitor() -> bool {\n    let src = audio_src_fragment(Some(&mon));\n    p.set_state(gst::State::Playing).ok();\n    true\n}\n";
        let b = bodies("crates/streamer/src/call.rs", planted);
        let f = "crates/streamer/src/call.rs";
        let bad = class_failures_of(&[], &[(f, "start_output_monitor", NoCapture("speaker meter, never sent"))], &b);
        assert_eq!(bad.len(), 2, "the NoCapture proof fails it AND the round-9 wording is refused: {bad:?}");
        let bad = class_failures_of(&[], &[(f, "start_output_monitor", Lease("gated somehow"))], &b);
        assert_eq!(bad.len(), 1, "a Lease note that names no lease gate fails: {bad:?}");
        let src = "fn tap_fragment() -> String {\n    let p = gst::parse::launch(\"pulsesrc ! fakesink\").unwrap();\n    p.set_state(gst::State::Playing).ok();\n    String::new()\n}\n";
        let bad = class_failures_of(&[(f, "tap_fragment", Lease("fragment: starts through leased_play"))], &[], &bodies(f, src));
        assert_eq!(bad.len(), 1, "a listed source that starts itself fails: {bad:?}");
        // the real list: the speaker meter / sniffer / echo reference are not listed at all any more (they start
        // through leased_play / leased_play_element), and no entry keeps the round-9 exemption
        for (_, func, _) in ALLOW_PLAYING {
            assert!(!["start_output_monitor", "sniffer_start", "ec_ref_restart"].contains(func), "{func} is still exempted");
        }
        for (_, _, g) in ALLOW_SOURCES.iter().chain(ALLOW_PLAYING.iter()) {
            if let Lease(n) = g {
                assert!(LEASE_GATES.iter().any(|x| n.contains(x)), "{n}");
            }
        }
    }

    #[test]
    fn selftest_round10_a_new_ungated_output_tap_fails_the_lint() {
        let planted = "fn new_speaker_tap() {\n    let d = format!(\"pipewiresrc target-object={sink} stream-properties=props,stream.capture.sink=true ! fakesink\");\n    let p = gst::parse::launch(&d).unwrap();\n    p.set_state(gst::State::Playing).ok();\n}\n";
        let v = violations(&scan("crates/streamer/src/hear.rs", planted));
        assert_eq!(v.iter().map(|h| h.rule).collect::<String>(), "AB", "{v:?}");
    }

    fn real_bodies() -> Vec<(String, String, String)> {
        let mut out = vec![];
        for (f, t) in tree() {
            if !f.ends_with("capture_lint.rs") {
                out.extend(bodies(&f, &t));
            }
        }
        out
    }
    fn mutate(b: &[(String, String, String)], func: &str, from: &str, to: &str) -> Vec<(String, String, String)> {
        let mut n = 0;
        let out = b.iter().map(|(f, fun, body)| {
            if fun == func && body.contains(from) {
                n += 1;
                (f.clone(), fun.clone(), body.replacen(from, to, 1))
            } else {
                (f.clone(), fun.clone(), body.clone())
            }
        }).collect();
        assert_eq!(n, 1, "the plant `{from}` in {func} applies once");
        out
    }

    #[test]
    fn selftest_voicenote_an_ungated_recorder_start_fails_the_lint() {
        let real = real_bodies();
        assert!(voicenote_failures(&real).is_empty(), "{:?}", voicenote_failures(&real));
        // a new function in the voice-note file building its own capture head
        let mut planted = real.clone();
        planted.push((VOICENOTE_FILE.into(), "quick_record".into(), "    let src = crate::call::audio_src_fragment(None);\n".into()));
        assert!(voicenote_failures(&planted).iter().any(|x| x.contains("quick_record")));
        // the recorder started without its lease gate
        let planted = mutate(&real, "start", "crate::call::leased_play(&lease, &pipeline, 6000, \"voice note start\")", "pipeline.set_state(gst::State::Playing).is_ok()");
        assert!(!voicenote_failures(&planted).is_empty());
        // and the same plant as text is a rule-B violation too
        let v = violations(&scan(VOICENOTE_FILE, "pub fn start() {\n    let _ = pipeline.set_state(gst::State::Playing);\n}\n"));
        assert_eq!(v.len(), 1, "{v:?}");
        // a second caller starting a build through no gate
        let mut planted = real.clone();
        planted.push((VOICENOTE_FILE.into(), "preview".into(), "    let (p, l) = build_recorder(&path)?;\n".into()));
        assert!(voicenote_failures(&planted).iter().any(|x| x.contains("preview")));
    }

    #[test]
    fn selftest_round12_a_local_source_or_the_output_mix_into_a_remote_tap_fails() {
        let real = real_bodies();
        assert!(remote_tap_failures(&real).is_empty(), "{:?}", remote_tap_failures(&real));
        // the mic's frames into the level table (a new caller outside build_playback)
        let mut planted = real.clone();
        planted.push(("crates/streamer/src/call.rs".into(), "start_mic_capture_meter".into(), "    set_track_level(\"me\", \"mic\", rms_of(&mic_frame));\n".into()));
        assert!(remote_tap_failures(&planted).iter().any(|x| x.starts_with("P1")));
        // build_playback fed from the output mix (a sink-monitor head) instead of the received stream
        let planted = mutate(&real, "build_playback", "let mut stream = crate::remote_audio::RemoteAudioStream::new(&track, SAMPLE_RATE as i32, 2);", "let mon = monitor_head(\"\", \"x\");\n    let mut stream = crate::remote_audio::RemoteAudioStream::new(&track, SAMPLE_RATE as i32, 2);");
        assert!(remote_tap_failures(&planted).iter().any(|x| x.starts_with("P2")));
        // build_playback fed from a LOCAL track (the mic's rtc track rebound)
        let planted = mutate(&real, "build_playback", "let mut stream = crate::remote_audio::RemoteAudioStream::new(&track, SAMPLE_RATE as i32, 2);", "let mut stream = NativeAudioStream::new(local_mic.rtc_track(), SAMPLE_RATE as i32, 2);");
        assert!(remote_tap_failures(&planted).iter().any(|x| x.starts_with("P2")));
        // a subscribe path passing a local track into playback
        let planted = mutate(&real, "connect_opts", "match build_playback(t.clone(),", "match build_playback(mic_track.clone(),");
        assert!(remote_tap_failures(&planted).iter().any(|x| x.starts_with("P3")));
        // `t` rebound to something else between the TrackSubscribed match and the call
        let planted = mutate(&real, "connect_opts", "match build_playback(t.clone(),", "let t = local_audio_track();\n                        match build_playback(t.clone(),");
        assert!(remote_tap_failures(&planted).iter().any(|x| x.starts_with("P3")));
    }
}
