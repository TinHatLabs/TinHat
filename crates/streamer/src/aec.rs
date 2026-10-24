//! Acoustic Echo Cancellation — the thing that stops a friend on SPEAKERS from echoing
//! everyone back into the call.
//!
//! WHY NOT libwebrtc's built-in AEC: it can only cancel what it played itself. This app
//! renders remote audio through its OWN GStreamer/PipeWire playback pipelines, which
//! libwebrtc never sees, so its `echo_cancellation` flag has nothing to subtract and is a
//! no-op here (that is why it has always been hard-coded off). Cancellation has to happen
//! where the loudspeaker signal and the mic signal actually meet: at the device layer.
//!
//! WHAT WE DO: for the lifetime of a call we load PipeWire's `module-echo-cancel`
//! (WebRTC's AEC3 under the hood) bound to the user's REAL mic + speakers, which gives us
//! two virtual nodes:
//!     tinhat_ec_source  — the mic, with the speaker signal subtracted
//!     tinhat_ec_sink    — play here; it forwards to the real speakers AND hands the
//!                              canceller its reference ("what the room is hearing")
//! The mic capture then reads the virtual source and remote-audio playback writes to the
//! virtual sink. Measured on the dev laptop with real laptop speakers + mic: **20.8 dB** of
//! echo removed (raw mic RMS 0.373 -> cancelled 0.034 while a tone played out loud).
//!
//! LIFECYCLE: loaded on call join, unloaded on leave / close / app exit. Nothing about the
//! user's audio setup is changed permanently — if the app dies without cleaning up, a
//! stale module is detected and reclaimed on the next join (see [`ensure_loaded`]).

use std::process::Command;
use std::sync::{Mutex, OnceLock};

pub const EC_SOURCE: &str = "tinhat_ec_source";
pub const EC_SINK: &str = "tinhat_ec_sink";

/// The pactl module index we loaded, so we can unload exactly ours.
fn module_idx() -> &'static Mutex<Option<u32>> {
    static M: OnceLock<Mutex<Option<u32>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(None))
}

/// What the running module is bound to: (resolved mic, resolved speaker, agc). Lets
/// [`ensure_loaded`] be IDEMPOTENT: a re-publish with unchanged binding reuses the running
/// module instead of destroy+recreate. A reload is never free — it kicks every live
/// playback stream off the reference sink (WirePlumber re-homes them to the real device,
/// where they stay, so the canceller loses its reference for the rest of the call) and
/// throws away AEC3's converged filter. This is what made every mid-call AGC/NS toggle
/// silently break echo cancellation.
fn loaded_binding() -> &'static Mutex<Option<(String, String, bool)>> {
    static B: OnceLock<Mutex<Option<(String, String, bool)>>> = OnceLock::new();
    B.get_or_init(|| Mutex::new(None))
}

fn aec_path() -> std::path::PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| {
            std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
        });
    base.join("tinhat/mic-echo-cancel")
}

fn cell() -> &'static Mutex<bool> {
    static C: OnceLock<Mutex<bool>> = OnceLock::new();
    // DEFAULT ON. Every mainstream voice client cancels echo by default, and the failure
    // mode of NOT doing it is inflicted on everyone ELSE in the call (they hear themselves
    // come back), so it cannot be left to the one person who can't hear the problem.
    // Headphone users pay only a little CPU. One toggle turns it off; the choice persists.
    // 0.8.29: default OFF on Linux. Benched on the test rig against our exact caps and
    // property set: the shipped topology (webrtcechoprobe in a second top-level pipeline,
    // which upstream forbids) removes 0.0 dB — output bit-identical to echo-cancel=false —
    // while the exclusively-named probe was the single most repeated cause of a mic chain
    // failing to reach PAUSED in the whole log corpus. 0 dB is 0 dB: turning it off costs
    // nothing and removes the failure surface. Windows keeps the default (WASAPI loopback
    // reference, same-machine measurements pending). A user who switched it on explicitly
    // keeps their choice.
    // 0.8.31: default ON everywhere again. The reference now runs INSIDE the mic
    // pipeline (same clock, same base time — the topology upstream documents), benched on
    // the test rig end to end through the production chain, and a machine where the reference
    // cannot start steps down a rung instead of losing the mic. Most of this group is on
    // speakers; the failure mode of OFF is inflicted on everyone else in the call.
    C.get_or_init(|| {
        Mutex::new(
            std::fs::read_to_string(aec_path()).map(|s| s.trim() != "0").unwrap_or(true),
        )
    })
}

/// Is echo cancellation enabled? (Persisted; default ON.)
pub fn enabled() -> bool {
    *cell().lock().unwrap()
}

/// Set + persist the echo-cancellation preference. Takes effect on the next mic (re)start —
/// the app re-publishes the mic and re-points playback so it applies mid-call.
pub fn set_enabled(on: bool) {
    *cell().lock().unwrap() = on;
    let p = aec_path();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(&p, if on { "1" } else { "0" });
}

/// 0.8.33: the OUTPUTS whose playback is the echo-cancellation reference — the user's own
/// list of "everything that comes out of my speakers" (Settings › Audio › Speaker outputs):
/// a mixer's Game + Chat + Virtual A, the desktop speakers and the TV, … Every one is captured
/// and SUMMED into the one reference the canceller listens to (call.rs `ec_reference_chain`).
/// Empty (the default) = the one output Tin Hat plays to. Persisted one node per line.
fn ref_outputs_path() -> std::path::PathBuf {
    aec_path().with_file_name("mic-echo-outputs")
}
fn ref_cell() -> &'static Mutex<Vec<String>> {
    static C: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
    C.get_or_init(|| {
        Mutex::new(
            std::fs::read_to_string(ref_outputs_path())
                .map(|s| s.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect())
                .unwrap_or_default(),
        )
    })
}
/// The user's speaker-output list for the echo reference (empty = Tin Hat's own output).
pub fn ref_outputs() -> Vec<String> {
    ref_cell().lock().unwrap().clone()
}
/// Set + persist the speaker-output list. Takes effect on the next mic (re)build — the app
/// calls apply_aec right after, which rebuilds the mic with the new reference branch.
pub fn set_ref_outputs(list: &[String]) {
    let mut v: Vec<String> = Vec::new();
    for o in list {
        let o = o.trim();
        if !o.is_empty() && !v.iter().any(|x| x == o) {
            v.push(o.to_string());
        }
    }
    *ref_cell().lock().unwrap() = v.clone();
    let p = ref_outputs_path();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(&p, v.join("\n"));
}

fn pactl(args: &[&str]) -> Option<String> {
    let out = Command::new("pactl").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Is our virtual source already present? (A previous run that died without cleaning up.)
fn nodes_present() -> bool {
    pactl(&["list", "short", "sources"]).map(|s| s.contains(EC_SOURCE)).unwrap_or(false)
}

/// Unload any echo-cancel module we (or a crashed previous run) left behind.
pub fn unload() {
    *loaded_binding().lock().unwrap() = None;
    if let Some(idx) = module_idx().lock().unwrap().take() {
        let _ = pactl(&["unload-module", &idx.to_string()]);
    }
    // Reclaim a stale one from a crashed run: match by our own node names so we never
    // unload an echo-cancel module the USER set up for something else.
    if nodes_present() {
        if let Some(list) = pactl(&["list", "short", "modules"]) {
            for line in list.lines() {
                if line.contains("module-echo-cancel") && line.contains(EC_SOURCE) {
                    if let Some(idx) = line.split_whitespace().next() {
                        let _ = pactl(&["unload-module", idx]);
                        eprintln!("[aec] reclaimed a stale echo-cancel module from a previous run");
                    }
                }
            }
        }
    }
}

/// Load echo-cancel bound to `mic` / `speaker` (node names; `None` = the system default),
/// with the module's gain control tied to `agc` (the user's AGC toggle — see below).
/// Returns the virtual (source, sink) node names to capture from / play into plus a
/// `fresh` flag (true = the nodes were just (re)created, so live playback sinks pointed
/// at the old node must be re-pointed), or `None` if AEC is off or unavailable — in which
/// case the caller uses the raw devices exactly as before, so a machine without the
/// module simply behaves like it always did.
pub fn ensure_loaded(mic: Option<&str>, speaker: Option<&str>, agc: bool) -> Option<(String, String, bool)> {
    // Every early-None below UNLOADS first: returning None while a module keeps running
    // would leave callers' consumers on the raw devices with loaded_binding still
    // describing the orphan — a later matching call would then "reuse" a module nothing
    // is attached to, silently voiding the cancellation for the rest of the call.
    if !enabled() {
        unload();
        return None;
    }

    let Some(mic) = mic.map(str::to_string).or_else(|| pactl(&["get-default-source"])) else {
        eprintln!("[aec] could not resolve a microphone (pactl get-default-source failed) — echo cancellation DOWN");
        unload();
        return None;
    };
    let Some(speaker) = speaker.map(str::to_string).or_else(|| pactl(&["get-default-sink"])) else {
        // Silent until now, and it is the failure a field user hit: no speaker means no
        // reference signal, so the canceller is torn down and everyone else in the call
        // starts hearing them come back. Never let that happen without saying so.
        eprintln!("[aec] could not resolve an output device (pactl get-default-sink failed) — echo cancellation DOWN");
        unload();
        return None;
    };
    // Don't try to cancel against a MONITOR/loopback source — that's a "share my desktop
    // audio" style input, not a microphone in a room with a speaker.
    if mic.contains(".monitor") {
        eprintln!("[aec] input is a monitor source — skipping echo cancellation");
        unload();
        return None;
    }

    // IDEMPOTENT: same binding + module still up => reuse it (see loaded_binding()).
    if module_idx().lock().unwrap().is_some()
        && loaded_binding().lock().unwrap().as_ref() == Some(&(mic.clone(), speaker.clone(), agc))
        && nodes_present()
    {
        return Some((EC_SOURCE.to_string(), EC_SINK.to_string(), false));
    }

    unload(); // never stack modules; also reclaims a stale one

    // aec_args, EXACT semantics (empirically proven on this stack, PipeWire 1.6.7,
    // journal 2026-07-12; plugin = spa/plugins/aec/aec-webrtc.cpp "webrtc version 1"):
    //
    //  * The value MUST be double-quoted. pw_properties tokenizes the module argument
    //    string on whitespace, so the old unquoted multi-key form silently truncated to
    //    its FIRST token — the shipped r133 config actually delivered
    //    `aec.args = { webrtc.gain_control: false }` and NOTHING else (proven via
    //    pipewire-pulse debug journal). Every other key fell through as an inert
    //    top-level module arg (also proven: the module accepts `frobnicate=1`).
    //  * Unset keys take the PLUGIN's defaults, which are aggressive: noise_suppression
    //    defaults TRUE at kHigh, transient_suppression TRUE, voice_detection TRUE. That
    //    suite — NS-kHigh gating + the keyboard-click "transient suppressor" chopping
    //    speech — is the "static until you toggle something" / "AEC weirdness" sound.
    //    So every knob we care about is set EXPLICITLY, never defaulted.
    //  * ONE processing stage per job: this module does echo cancellation + high-pass
    //    ONLY. Noise suppression stays RNNoise (the in-app opt-in denoise toggle);
    //    module NS is hard-coded off. Gain control: the libwebrtc APM options on the
    //    fast-path NativeAudioSource are INERT (no APM runs over injected frames), so
    //    the module's AGC is the one real gain stage users have — it follows the
    //    existing persisted AGC toggle. `digital_gain_control` and `analog_gain_control`
    //    both rename to the SAME `webrtc.gain_control` key (pipewire-pulse rename
    //    table), so exactly one is passed. `webrtc.transient_suppression` has no pulse
    //    rename and must be passed as the dotted key (passes through verbatim).
    let aec_args = format!(
        "aec_args=\"digital_gain_control={} noise_suppression=0 voice_detection=0 \
         high_pass_filter=1 webrtc.transient_suppression=0\"",
        if agc { 1 } else { 0 }
    );
    let idx = pactl(&[
        "load-module",
        "module-echo-cancel",
        "aec_method=webrtc",
        &format!("source_master={mic}"),
        &format!("sink_master={speaker}"),
        &format!("source_name={EC_SOURCE}"),
        &format!("sink_name={EC_SINK}"),
        &aec_args,
    ])
    .and_then(|s| s.parse::<u32>().ok());

    match idx {
        Some(i) => {
            *module_idx().lock().unwrap() = Some(i);
            *loaded_binding().lock().unwrap() = Some((mic.clone(), speaker.clone(), agc));
            wait_ready();
            eprintln!("[aec] echo cancellation ON (mic={mic} speaker={speaker} agc={agc})");
            Some((EC_SOURCE.to_string(), EC_SINK.to_string(), true))
        }
        None => {
            // No module (or PulseAudio-less box): degrade to exactly the old behaviour.
            eprintln!("[aec] module-echo-cancel unavailable — continuing without AEC");
            None
        }
    }
}

/// Block until the freshly-created EC source is actually listable, then one settle beat.
/// A capture pipeline built the instant load-module returns binds a node that is still
/// assembling — suspended masters resuming, the module's play buffer priming — and that
/// cold window is where the join-time garble is born (delivery starts erratic; the
/// feeder conceals the gaps with duplicate frames, which is the audible static).
fn wait_ready() {
    let t0 = std::time::Instant::now();
    while t0.elapsed() < std::time::Duration::from_secs(2) {
        if nodes_present() {
            eprintln!("[aec] EC nodes visible after {}ms (+300ms settle)", t0.elapsed().as_millis());
            std::thread::sleep(std::time::Duration::from_millis(300));
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    eprintln!("[aec] WARNING: EC source not visible within 2s of load — proceeding anyway");
}
