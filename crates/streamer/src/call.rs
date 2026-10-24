//! Unified LiveKit call session (ONE Room for the whole call, so we appear as a
//! single participant). This session owns the E2EE Room and:
//!   * publishes our mic (gst `pipewiresrc` -> `NativeAudioSource`),
//!   * plays remote participants' audio (`NativeAudioStream` -> gst -> output),
//!   * (next) hosts the screen-share publish + remote video tiles.
//!
//! Audio in/out is pure GStreamer/PipeWire — we deliberately avoid libwebrtc's
//! built-in ADM (the "AdmProxy Init failed -1": it's Pulse/ALSA-based and grabs the
//! mic) and push/pull PCM frames ourselves.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{Context, Result};
use futures_util::StreamExt;
use gstreamer as gst;
use gstreamer::glib;
use gstreamer::prelude::*;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

use livekit::e2ee::key_provider::KeyProvider;
use livekit::e2ee::{E2eeOptions, EncryptionType};
use livekit::id::ParticipantIdentity;
use livekit::options::{AudioEncoding, TrackPublishOptions};
use livekit::track::{LocalAudioTrack, LocalTrack, RemoteTrack, TrackSource};
use livekit::webrtc::audio_frame::AudioFrame;
use livekit::webrtc::audio_source::native::NativeAudioSource;
use livekit::webrtc::audio_source::{AudioSourceOptions, RtcAudioSource};
use livekit::webrtc::audio_stream::native::NativeAudioStream;
use livekit::webrtc::video_frame::VideoBuffer;
use livekit::webrtc::video_stream::native::NativeVideoStream;
use livekit::{Room, RoomEvent, RoomOptions};

use crate::camera;
use crate::capture::ScreenCast;
use crate::lk_publish::matrix_key_provider;

const SAMPLE_RATE: u32 = 48_000;

/// Call lifecycle events the app needs to drive the QML video-tile grid. The app
/// drains these (from [`CallSession::connect`]) and builds the render pipeline on
/// the Qt GUI thread (qml6glsink needs GUI-thread affinity, else the tile is black).
pub enum CallEvent {
    /// A remote video track was subscribed: a tile should appear. The app builds the
    /// render pipeline on the GUI thread via [`create_video_tile`] with this `track`.
    VideoAdded {
        id: String,
        participant: String,
        /// True if this is a SCREEN-SHARE track (vs a camera) — the UI labels + spotlights it.
        is_screenshare: bool,
        track: livekit::webrtc::video_track::RtcVideoTrack,
    },
    /// A remote video track went away: remove the tile with this `id`.
    VideoRemoved { id: String },
    /// A remote participant connected to the SFU (LiveKit identity `userId:deviceId`).
    /// The app creates an avatar tile, re-announces our E2EE media key to this peer over
    /// to-device (so a peer who joins AFTER us can decrypt our media), and may rotate the key.
    ParticipantJoined { identity: String },
    /// A remote participant left the SFU — the app removes their avatar tile + rotates the key.
    ParticipantLeft { identity: String },
    /// A remote participant muted/unmuted a track (we care about their MIC for the tile glyph).
    TrackMuteChanged { identity: String, category: String, muted: bool },
    /// LiveKit connection state changed — drives a "reconnecting…"/"call ended" banner and,
    /// on a terminal disconnect, the same teardown as a normal leave (so we don't wedge).
    /// `recoverable` (meaningful only with `disconnected`) says whether auto-rejoin is
    /// appropriate: true for network/token failures, FALSE when the server deliberately
    /// ended our participation (kicked, duplicate identity, room deleted/closed) — the
    /// app must never force its way back into those.
    ConnectionState { reconnecting: bool, disconnected: bool, recoverable: bool },
    /// RBAC (docs/RBAC.md §5.4): `identity` was server-muted / unmuted by moderator `by`
    /// (`by_name` = their display name as the packet carried it, may be ""). Fired for
    /// peers AND for the local participant (identity == ours) — the app locks the mic
    /// button on the latter and republishes the mic when it clears.
    ServerMuteChanged { identity: String, muted: bool, by: String, by_name: String },
    /// RBAC: the SFU removed US (`Disconnected{ParticipantRemoved}`); `by`/`by_name` come
    /// from the `disconnect` packet addressed to us just before, else empty. Always
    /// followed by the terminal `ConnectionState { disconnected: true, recoverable: false }`.
    RemovedByModerator { by: String, by_name: String },
}

/// Stable per-track id / QML objectName for a remote video tile.
fn tile_id(sid: &str) -> String {
    format!("vid_{sid}")
}

/// Simulcast rungs for a camera top layer of `w`x`h`: quarter + half resolution —
/// CLEAN 2x/4x scale ratios. libwebrtc's H.264 simulcast path rejects non-power-of-2
/// ratios: a fixed H360 rung under a 1080p top is 3x, which silently fell the WHOLE
/// SimulcastEncoderAdapter back to OpenH264 (all rungs, subscriber decoded nothing) —
/// caught by the TINHAT_CAM_SIMTEST gate. Deriving from the top layer keeps every ratio
/// exactly 2x/4x at any top size (720p: 320x180+640x360; 1080p: 480x270+960x540).
/// Bitrate/fps follow the LiveKit preset ballpark (H180/H360/H540).
fn camera_simulcast_layers(w: u32, h: u32) -> Vec<livekit::options::VideoPreset> {
    use livekit::options::VideoPreset;
    let mid_kbps: u64 = if u32::max(w, h) >= 1920 { 700 } else { 450 };
    vec![
        VideoPreset::new(w / 4, h / 4, 150_000, 15.0),
        VideoPreset::new(w / 2, h / 2, mid_kbps * 1000, 20.0),
    ]
}

/// The PipeWire sink (node name) remote call audio plays into. `None` = system
/// default. Read by `build_playback` when each remote stream's pipeline is created.
/// File-backed (`~/.config/tinhat/call-output`, mirroring `music_source`) so the chosen
/// SPEAKER is remembered across launches; a stale/removed sink is dropped to None (default).
fn playback_target() -> &'static Mutex<Option<String>> {
    static T: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    T.get_or_init(|| Mutex::new(load_saved_device("call-output", |n| {
        list_devices("Audio/Sink").iter().any(|(d, _)| d == n)
    })))
}

// ── Per-track playback GAIN (Phase 3 infra) ──────────────────────────────────
//
// Each remote audio track's playback pipeline carries a NAMED gst `volume` element
// (`build_playback`), defaulting to volume=1.0 / mute=false — UNITY = transparent, so
// existing call audio is byte-identical until the user explicitly changes a per-user
// volume. We keep a process-global registry of WeakRefs to those elements, keyed by
// `(livekit_identity, category)`, so the app can adjust a single participant's level.
//
// Categories: "mic" (Microphone), "screenshare" (ScreenshareAudio), "music" (Unknown /
// other — reserved for #7 music + soundboard 2nd-track playback). WeakRef so a dropped
// pipeline (TrackUnsubscribed) can't be resurrected; we also remove the entry eagerly.

/// Registry of remote-audio `volume` elements, keyed by `(livekit_identity, category,
/// track_sid)`. The sid is part of the key because a peer can have TWO live tracks in one
/// category (soundboard one-shot + music are both TrackSource::Unknown -> "music"); keyed
/// without it, the second registration overwrote the first and the one-shot's unsubscribe
/// removed the shared entry while the music pipeline was still live — after a deafen/AFK
/// cycle that peer's music stayed muted forever (the restore found no entry).
/// `glib::WeakRef` so a torn-down pipeline's element drops cleanly (upgrade() -> None).
fn track_gains() -> &'static Mutex<HashMap<(String, String, String), glib::WeakRef<gst::Element>>> {
    static G: OnceLock<Mutex<HashMap<(String, String, String), glib::WeakRef<gst::Element>>>> =
        OnceLock::new();
    G.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Desired per-(ui user_id, category) volume/mute, recorded whenever the app pushes one
/// (slider drag, #5 mute, the join-time persisted-pref reapply). Applied to tracks that
/// register LATER — a peer who joins after the app's reapply timers, or who republishes
/// mid-call (their AGC/NS toggle mints a new sid -> a fresh unity/unmuted element),
/// otherwise plays at defaults despite a saved pref. Cleared on call close.
fn desired_track_prefs() -> &'static Mutex<HashMap<(String, String), (f64, bool)>> {
    static D: OnceLock<Mutex<HashMap<(String, String), (f64, bool)>>> = OnceLock::new();
    D.get_or_init(|| Mutex::new(HashMap::new()))
}
/// The recorded desired (volume, muted) whose user_id matches `identity` (bare-mxid
/// prefix rule, same as resolve_gain_elements), if any.
fn desired_pref_for(identity: &str, category: &str) -> Option<(f64, bool)> {
    let map = desired_track_prefs().lock().unwrap();
    map.iter().find_map(|((uid, cat), v)| {
        (cat == category && (identity == uid || identity.starts_with(&format!("{uid}:"))))
            .then_some(*v)
    })
}

/// Deterministic gst element name for a track's `volume` element, from its identity +
/// category. Sanitized (gst element names allow only `[A-Za-z0-9_-]`).
fn gain_element_name(identity: &str, category: &str) -> String {
    let mut s = String::with_capacity(identity.len() + category.len() + 6);
    s.push_str("gain_");
    for ch in identity.chars().chain(std::iter::once('_')).chain(category.chars()) {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            s.push(ch);
        } else {
            s.push('-');
        }
    }
    s
}

/// Map a LiveKit publication source to our gain category string.
fn category_for_source(source: TrackSource) -> &'static str {
    match source {
        TrackSource::Microphone => "mic",
        TrackSource::ScreenshareAudio => "screenshare",
        // Unknown / anything else: reserved for #7 music + soundboard playback.
        _ => "music",
    }
}

/// Register a track's `volume` element so [`set_track_gain`] / [`set_track_muted`] can
/// find it. Stores a WeakRef (a dropped pipeline yields a dead ref, never a leak).
/// Applies any recorded desired pref (see [`desired_track_prefs`]) so late subscribers
/// and mid-call republishes honour saved per-user volume/mute.
fn register_track_gain(identity: &str, category: &str, sid: &str, element: &gst::Element) {
    // 9.4: a track plays at the user's pref × the corner factor (10 % outside my corner, 0 for
    // the mic of a corner I am not in), so one subscribing mid-aside starts at the right level.
    let eff = effective_volume(identity, category);
    if (eff - 1.0).abs() > f64::EPSILON {
        element.set_property("volume", eff);
    }
    if (corner_factor(identity, category) - 1.0).abs() > f64::EPSILON {
        eprintln!("[call] corner: {identity} {category} starts at {:.0}% here", eff * 100.0);
    }
    if let Some((vol, muted)) = desired_pref_for(identity, category) {
        if muted {
            element.set_property("mute", true);
        }
        if (vol - 1.0).abs() > f64::EPSILON || muted {
            // Field evidence for "can't hear X": a persisted 0%/muted pref silently
            // applies to EVERY future track of that user (rejoins included) and is
            // otherwise indistinguishable from a broken call.
            eprintln!(
                "[call] applying saved audio pref to {identity} ({category}): volume={vol:.2} muted={muted}"
            );
        }
    }
    // Local layers that silence this track regardless of the user's pref: recording-only
    // + not-watched (share audio), game chat (mic).
    if layer_muted(identity, category) {
        element.set_property("mute", true);
        eprintln!("[call] {identity} {category} audio starts MUTED ({})", layer_reason(identity, category));
    }
    let weak = element.downgrade();
    track_gains()
        .lock()
        .unwrap()
        .insert((identity.to_owned(), category.to_owned(), sid.to_owned()), weak);
    // A late subscriber inherits the CURRENT deafen/AFK state. Without this, deafen only
    // ever muted the tracks that existed at the moment it was pressed: anyone who joined,
    // unmuted, or republished afterwards came through at full volume while the UI still
    // showed you as deafened. AFK escaped it only because the app re-applies that one on a
    // timer; deafen has no timer, so it was silently partial.
    //
    // Deliberately AFTER the saved-pref block above: reconcile only claims ownership of a
    // track it actually mutes, so a track the user muted themselves is left alone and is
    // not un-muted later when deafen is released.
    reconcile_auto_mute();
}

/// Drop a track's gain registry entry (TrackUnsubscribed / participant left). Idempotent.
fn unregister_track_gain(identity: &str, category: &str, sid: &str) {
    track_gains()
        .lock()
        .unwrap()
        .remove(&(identity.to_owned(), category.to_owned(), sid.to_owned()));
    // The auto-mute layer must not keep owning a dead track's key.
    away_muted_names()
        .lock()
        .unwrap()
        .remove(&(identity.to_owned(), category.to_owned(), sid.to_owned()));
}

/// Drop ALL gain entries for a participant identity (any category) — called when a peer
/// disconnects so no stale (dead) WeakRefs linger.
fn unregister_participant_gains(identity: &str) {
    track_gains().lock().unwrap().retain(|(id, _, _), _| id != identity);
    away_muted_names().lock().unwrap().retain(|(id, _, _)| id != identity);
}

/// Resolve a UI `user_id` (a Matrix mxid like `@u:server`, or a full LiveKit identity
/// `@u:server:DEVICE`) + `category` to the live `volume` element(s). The participant
/// bubbles carry the bare mxid while tracks register under the full LiveKit identity, so
/// we match an entry whose identity equals `user_id` OR starts with `user_id` + ':'.
/// Returns every matching upgraded element (usually one; >1 only if a user is somehow in
/// twice). Dead WeakRefs are skipped (and harmless).
fn resolve_gain_elements(user_id: &str, category: &str) -> Vec<gst::Element> {
    let prefix = format!("{user_id}:");
    track_gains()
        .lock()
        .unwrap()
        .iter()
        .filter(|((id, cat, _), _)| cat == category && (id == user_id || id.starts_with(&prefix)))
        .filter_map(|(_, weak)| weak.upgrade())
        .collect()
}

// ── #B live audio levels (waveform bars) ────────────────────────────────────────
//
// LOW-RISK, no pipeline change: the mic appsink callback and the remote
// NativeAudioStream playback loop ALREADY compute a per-frame peak; we just fold each
// frame to an RMS and stash the latest value here. The app polls these getters from
// QML's 120ms mirror to animate a few small bars next to each name. No gst `level`
// element, no pipeline string touched, so call audio stays byte-identical.

/// Latest mic RMS in `[0.0, 1.0]` (process-global: there is only ever one local mic).
fn mic_level_cell() -> &'static Mutex<f32> {
    static M: OnceLock<Mutex<f32>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(0.0))
}

/// Latest per-remote-track RMS in `[0.0, 1.0]`, keyed by the SAME `(identity, category)`
/// tuple as [`track_gains`] (so the mxid-prefix matching in [`resolve_level`] mirrors
/// [`resolve_gain_elements`]). Plain values (not WeakRefs): cleared on
/// unsubscribe/leave/close alongside `track_gains`.
fn track_levels() -> &'static Mutex<HashMap<(String, String), f32>> {
    static L: OnceLock<Mutex<HashMap<(String, String), f32>>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 10.5: remote AUDIO tracks currently subscribed (sids) — a playback build retry stops when its
/// track goes away. Cleared by close().
fn audio_sids_live() -> &'static Mutex<std::collections::HashSet<String>> {
    static S: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}

/// 10.5 (audio review #1): a remote audio track whose FIRST playback build failed used to be logged
/// and forgotten — the healer only exists once a pipeline does, so "can't hear X" lasted until X
/// rejoined. Retry with the retained track: 1 s, 3 s, 9 s, then every 30 s, or at once when the
/// user presses Fix on X's tile — until it builds, the track goes away, or the call ends.
fn spawn_playback_retry(
    pb: Arc<Mutex<std::collections::HashMap<String, PlaybackHandle>>>,
    track: livekit::webrtc::audio_track::RtcAudioTrack,
    who: String,
    category: String,
    sid: String,
) {
    tokio::spawn(async move {
        let mut attempt: u32 = 0;
        let mut wait = std::time::Duration::from_secs(1);
        loop {
            let started = std::time::Instant::now();
            while started.elapsed() < wait {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                if crate::hear::take_heal_request(&who) {
                    break;
                }
            }
            if !audio_sids_live().lock().unwrap().contains(&sid) || pb.lock().unwrap().contains_key(&sid) {
                return; // unsubscribed (or re-subscribed and built) meanwhile
            }
            attempt += 1;
            match build_playback(track.clone(), who.clone(), category.clone(), sid.clone()) {
                Ok((p, task)) => {
                    // The build took seconds: adopt it only if the track is STILL subscribed, checked and
                    // inserted under the one lock unsubscribe and close() take FIRST (Astra 2026-09-22 #3) —
                    // else it was a PLAYING pipeline and a feeder that nobody would ever tear down.
                    let live = audio_sids_live().lock().unwrap();
                    if !live.contains(&sid) {
                        drop(live);
                        task.abort();
                        let cur = p.lock().unwrap().clone();
                        set_state_bounded(cur.upcast_ref(), gst::State::Null, 1500, "playback retry (track gone)");
                        return;
                    }
                    let displaced = pb.lock().unwrap().insert(sid.clone(), PlaybackHandle { pipeline: p, task });
                    drop(live);
                    eprintln!("[call] playback for {who} ({category}) built on retry {attempt}");
                    crate::hear::note_play_stall(&sid, false);
                    if let Some(old) = displaced {
                        old.task.abort();
                        let cur = old.pipeline.lock().unwrap().clone();
                        set_state_bounded(cur.upcast_ref(), gst::State::Null, 1500, "remote-audio playback");
                    }
                    return;
                }
                Err(e) => {
                    eprintln!("[call] playback for {who} ({category}): retry {attempt} failed — {e:#}");
                    wait = std::time::Duration::from_secs(match attempt {
                        1 => 3,
                        2 => 9,
                        _ => 30,
                    });
                }
            }
        }
    });
}

/// Store the latest mic RMS (called from the mic appsink callback).
fn set_mic_level(rms: f32) {
    *mic_level_cell().lock().unwrap() = rms;
}

/// Store the latest RMS for one remote track (called from the playback loop).
fn set_track_level(identity: &str, category: &str, rms: f32) {
    track_levels()
        .lock()
        .unwrap()
        .insert((identity.to_owned(), category.to_owned()), rms);
}

/// Drop one remote track's level entry (TrackUnsubscribed). Idempotent.
fn unregister_track_level(identity: &str, category: &str) {
    track_levels()
        .lock()
        .unwrap()
        .remove(&(identity.to_owned(), category.to_owned()));
}

/// Drop ALL level entries for a participant identity (participant left). Idempotent.
fn unregister_participant_levels(identity: &str) {
    track_levels().lock().unwrap().retain(|(id, _), _| id != identity);
}

// ── active speakers (voice-overlay / speaking rings) ────────────────────────────
//
// SERVER-side voice detection: the SFU computes "who is speaking" from the RTP
// audio-level header extension (which E2EE does not encrypt) with its own windowing.
// This is deliberately NOT a raw local-RMS threshold: AGC boosts ambient noise above
// any fixed floor (a laptop mic + agc=on reads ~0.03-0.08 RMS in a silent room), and
// a muted track can never appear in the server's list. Replaced wholesale on every
// ActiveSpeakersChanged event; cleared on reconnect/disconnect/close.

/// Identities the SFU currently reports as actively speaking (full LiveKit identities,
/// `@user:server:DEVICE`).
fn active_speakers() -> &'static Mutex<Vec<String>> {
    static S: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(Vec::new()))
}

/// Our own LiveKit identity for the current call — lets the UI tell the self entry
/// apart in `speakers_json`. Set on connect, cleared on close.
fn local_identity_cell() -> &'static Mutex<String> {
    static I: OnceLock<Mutex<String>> = OnceLock::new();
    I.get_or_init(|| Mutex::new(String::new()))
}

fn set_active_speakers(ids: Vec<String>) {
    crate::hear::note_speakers(&ids); // 10.5: "not coming through" + "can't hear X" read the SFU's view
    *active_speakers().lock().unwrap() = ids;
}

/// Change-guard for the count-only "[call] speakers: N active" diagnostics line.
/// usize::MAX = "log the next update whatever it is". Reset at connect + reconnect —
/// a process-lifetime guard would swallow the FIRST line of a new call whenever its
/// speaker count matched the previous call's last value, and that line exists precisely
/// as field evidence that deltas arrive at all.
fn speakers_log_guard() -> &'static std::sync::atomic::AtomicUsize {
    static G: OnceLock<std::sync::atomic::AtomicUsize> = OnceLock::new();
    G.get_or_init(|| std::sync::atomic::AtomicUsize::new(usize::MAX))
}

/// Fold a slice of i16 PCM samples to an RMS in `[0.0, 1.0]` (0 = silence). Cheap; runs
/// on the audio thread once per frame. Empty slice => 0.0.
fn rms_of(samples: &[i16]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    // Sum of squares in f64 to avoid overflow, then normalize by i16 full-scale.
    let sum_sq: f64 = samples.iter().map(|&s| (s as f64) * (s as f64)).sum();
    let mean = sum_sq / samples.len() as f64;
    ((mean.sqrt()) / (i16::MAX as f64)).clamp(0.0, 1.0) as f32
}

/// Cubic SOFT-CLIP safety, applied AFTER the limiter gain. Only the brief attack-overshoot that
/// leaks past the limiter's smoothed (non-instant) attack ever exceeds KNEE, so on normal/steady
/// audio this is a pure pass-through (the limiter target CEIL=0.93 sits BELOW KNEE=0.95 => 0 THD).
/// Maps [KNEE, +inf) -> [KNEE, SC_CEIL] smoothly (zero slope at the ceiling), bounding |out| to
/// SC_CEIL so the i16 narrow can never saturate even on the worst transient onset.
#[inline]
fn soft_clip(y: f32) -> f32 {
    const KNEE: f32 = 0.95;
    const SC_CEIL: f32 = 0.99;
    let a = y.abs();
    if a <= KNEE {
        return y;
    }
    let t = ((a - KNEE) / (SC_CEIL - KNEE)).min(1.0);
    (KNEE + (SC_CEIL - KNEE) * (1.5 * t - 0.5 * t * t * t)).copysign(y)
}

/// Float-domain PEAK LIMITER applied per mic sample BEFORE the 16-bit narrow.
///
/// WHY: Tin Hat is the ONLY client that captures with libwebrtc APM DISABLED (no AGC/limiter)
/// and then narrows F32->S16. So ANY mic transient that reaches 0 dBFS HARD-CLIPS on the narrow
/// = flat-topped clusters = the universal "popping". A controlled source/receiver matrix proved it:
/// native-source pops to web/phone/native alike, web-source never pops — because web/Discord run
/// an APM output limiter before the int conversion and we did not. (The virtual mic in that matrix
/// is extreme — live capture peaked at +10.5 dBFS / |x|=3.37, 25 dB crest — but the bug is universal
/// to any native mic; the user's own PipeWire setup is OFF-LIMITS.) This is the in-app limiter that makes us behave
/// like every other client.
///
/// Algorithm (feed-forward peak limiter, SMOOTHED attack / exponential release):
///   env = max(|x|, decayed-env)            — peak follower (instant attack, ~20 ms release)
///   tg  = CEIL/env if env>CEIL else 1.0    — instantaneous target gain (CEIL = 0.93 ceiling)
///   g   -> tg with a 0.5 ms SMOOTHED attack (NOT a step) and ~20 ms release
///   y   = soft_clip(x * g)                 — final bound to SC_CEIL=0.99
/// r117 used an INSTANT gain attack: on a sharp transient (e.g. a word-final plosive) the gain
/// snapped in ONE sample => a gain STEP => an audible click ("popping when I finish a word",
/// ~1 per 6 s; quieter with RNNoise since it softens transients). r118 ramps the attack over
/// 0.5 ms so there is no step (validated: gain-slew max 0.296 -> 0.018, click-steps 3 -> 0); the
/// smoothing lets a peak briefly overshoot CEIL during the ramp, so a cubic soft_clip (knee 0.95,
/// above CEIL) catches just that leak. Because CEIL < KNEE, steady/normal audio passes the soft
/// clip untouched => THD 0.00% (vs r116 memoryless 10-30%). Cheap: abs + compare + one divide
/// when engaged — NO per-sample log/pow (r116's log10/powf starved the audio thread => warble).
/// State (`env`,`g`) PERSISTS across appsink buffers. `shaped` counts engaged samples.
/// Validated by the EXACT code on a real 17.7 s field capture (+10.5 dBFS peaks): out_peak
/// 0.9900, 0 samples over ceiling, 0 click-steps, THD 0.00%.
#[inline]
fn limit_sample(x: f32, env: &mut f32, g: &mut f32, shaped: &mut u64) -> f32 {
    const CEIL: f32 = 0.93; // limiter target ceiling — kept BELOW soft_clip KNEE (0.95) for 0 THD
    const REL: f32 = 0.998_958_9; // exp(-1/(0.020*48000)) — ~20 ms env + gain release
    const ATK: f32 = 0.959_189_3; // exp(-1/(0.0005*48000)) — ~0.5 ms SMOOTHED gain attack (no step)
    let x = if x.is_finite() {
        x
    } else {
        *shaped += 1;
        0.0 // defensive: NaN/Inf -> silence (never feed garbage to the encoder)
    };
    let a = x.abs();
    *env = if a > *env { a } else { a + (*env - a) * REL }; // instant attack, exp release
    let tg = if *env > CEIL {
        *shaped += 1;
        CEIL / *env
    } else {
        1.0
    };
    // SMOOTHED gain attack (0.5 ms ramp, not a step => no click) + ~20 ms release.
    *g = if tg < *g { ATK * *g + (1.0 - ATK) * tg } else { REL * *g + (1.0 - REL) * tg };
    soft_clip(x * *g) // bounded to SC_CEIL; caller narrows to i16
}

/// #B: the latest local mic RMS in `[0.0, 1.0]` (0 = silent/idle). Cheap mutex peek; the
/// app polls this from QML to animate the self-view waveform. In a call this is the published
/// signal (post-denoise when RNNoise is on); under the standalone monitor it is post-denoise.
pub fn mic_level() -> f64 {
    *mic_level_cell().lock().unwrap() as f64
}

/// PRE-denoise mic RMS, driven ONLY by the standalone mic monitor (see [`start_mic_monitor`])
/// for the noise-cancel before/after meter. 0 when no monitor is running (the in-call mic path
/// is deliberately left byte-identical and does not feed this).
fn mic_level_pre_cell() -> &'static Mutex<f32> {
    static M: OnceLock<Mutex<f32>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(0.0))
}
fn set_mic_level_pre(rms: f32) {
    *mic_level_pre_cell().lock().unwrap() = rms;
}
pub fn mic_level_pre() -> f64 {
    *mic_level_pre_cell().lock().unwrap() as f64
}

/// Latest OUTPUT (speaker/sink) RMS in `[0.0, 1.0]`, driven ONLY by the standalone output
/// monitor (see [`start_output_monitor`]) which taps the selected sink's `.monitor`. 0 when
/// no monitor is running. There is only ever one selected output, so a single process-global
/// cell mirrors the mic pattern.
fn output_level_cell() -> &'static Mutex<f32> {
    static M: OnceLock<Mutex<f32>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(0.0))
}
fn set_output_level(rms: f32) {
    *output_level_cell().lock().unwrap() = rms;
}
pub fn output_level() -> f64 {
    *output_level_cell().lock().unwrap() as f64
}

/// #B: latest per-remote-track audio levels as JSON `[{"user":..,"category":..,"rms":..}]`.
/// `user` is the full LiveKit identity (`@u:server:DEVICE`); the QML side matches a tile's
/// participant by mxid prefix, mirroring [`resolve_gain_elements`]. Built by hand (the
/// streamer crate has no serde_json dep) with minimal string escaping. Cheap mutex peek.
pub fn audio_levels_json() -> String {
    fn esc(s: &str, out: &mut String) {
        for ch in s.chars() {
            match ch {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
                c => out.push(c),
            }
        }
    }
    let levels = track_levels().lock().unwrap();
    let mut s = String::from("[");
    let mut first = true;
    for ((id, cat), rms) in levels.iter() {
        if !first {
            s.push(',');
        }
        first = false;
        s.push_str("{\"user\":\"");
        esc(id, &mut s);
        s.push_str("\",\"category\":\"");
        esc(cat, &mut s);
        // RMS is always finite in [0,1] (rms_of clamps), so a plain format is valid JSON.
        // 3 decimals (matching the mic path) so a near-silent peer's jittering 4th digit
        // doesn't change the JSON every 120ms tick -> no needless setter + QML re-parse.
        s.push_str(&format!("\",\"rms\":{:.3}}}", rms));
    }
    s.push(']');
    s
}

/// `{"me":"<our identity>","active":["identity",...]}` — the SFU's active-speaker list
/// plus our own identity (see [`active_speakers`]). Polled by the app's 120ms mirror
/// next to `audio_levels_json`; stable output (no per-tick jitter) so the change-guard
/// upstream only fires on real speaking transitions.
pub fn speakers_json() -> String {
    fn esc(s: &str, out: &mut String) {
        for ch in s.chars() {
            match ch {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
                c => out.push(c),
            }
        }
    }
    let mut s = String::from("{\"me\":\"");
    esc(&local_identity_cell().lock().unwrap(), &mut s);
    s.push_str("\",\"active\":[");
    let ids = active_speakers().lock().unwrap();
    let mut first = true;
    for id in ids.iter() {
        if !first {
            s.push(',');
        }
        first = false;
        s.push('"');
        esc(id, &mut s);
        s.push('"');
    }
    s.push_str("]}");
    s
}

/// Publisher-side outgoing stream health, keyed by stream kind ("screen", "camera/<rid>",
/// "mic"). Values are pre-built JSON object strings (same no-serde idiom as
/// [`audio_levels_json`]); written every 3s by [`spawn_publish_stats_task`], drained by
/// the app's polled mirror via [`publish_stats_json`]. BTreeMap for stable key order —
/// stable JSON keeps the app-side change-guard quiet between real changes.
fn publish_stats() -> &'static Mutex<std::collections::BTreeMap<String, String>> {
    static S: OnceLock<Mutex<std::collections::BTreeMap<String, String>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(std::collections::BTreeMap::new()))
}

/// Latest outgoing-stream health as a JSON array (see [`publish_stats`]). Cheap mutex peek.
pub fn publish_stats_json() -> String {
    let map = publish_stats().lock().unwrap();
    let mut s = String::from("[");
    for (i, v) in map.values().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(v);
    }
    s.push(']');
    s
}

/// Minimal JSON string escape for the stats entries (encoder labels etc. — no exotic
/// content expected, but never emit invalid JSON).
fn stats_esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Call-lifetime publisher stats for one outgoing track. Every 3s: pull get_stats(),
/// emit the LEGACY [lk-stats]/[lk-stats-cam] log lines (formats preserved VERBATIM —
/// the TINHAT_SHARETEST / TINHAT_CAM_SIMTEST ship gates grep them), and refresh this
/// stream's entries in [`publish_stats`] for the in-call "stream health" UI.
///
/// Replaces the old bounded 16x3s diagnostic loops, which silently stopped ~48s after
/// publish — the owner had no visibility into a share's quality past the first minute.
///
/// `kind`: "screen" | "camera" (one entry per simulcast rid) | "mic".
/// `enc_pick`: the RESOLVED encoder choice (e.g. "H.264/NVENC (GPU)" from
///   resolve_share_encoder) — shown until libwebrtc reports the live implementation,
///   and kept alongside it so the UI can show what was REQUESTED vs what ENGAGED.
/// `stop`: flipped by the owning teardown (bus_stop for video, MicHandle.stats_stop);
///   the task prunes its entries on exit so a stopped stream vanishes from the UI.
///
/// Loss/RTT/jitter come from the SFU's RTCP receiver reports (RemoteInboundRtp), matched
/// to the outbound stream BY SSRC (per simulcast layer). `round_trip_time_measurements
/// == 0` means "no report yet" — emitted as -1 sentinels the UI hides (whether this SFU
/// populates them at all is unverified; serde defaults make the worst case 0/absent,
/// never a crash).
/// Hook the app installs to re-publish the share when the Auto-resolution governor
/// picks a new rung (same closure pattern as `share_dead_hook`; global because the
/// stats task is a free task, not a CallSession method).
fn share_refit_hook_cell() -> &'static std::sync::Mutex<Option<Arc<dyn Fn() + Send + Sync>>> {
    static H: OnceLock<std::sync::Mutex<Option<Arc<dyn Fn() + Send + Sync>>>> = OnceLock::new();
    H.get_or_init(|| std::sync::Mutex::new(None))
}
pub fn set_share_refit_hook(hook: Arc<dyn Fn() + Send + Sync>) {
    *share_refit_hook_cell().lock().unwrap() = Some(hook);
}

fn spawn_publish_stats_task(
    kind: &'static str,
    enc_pick: String,
    track: livekit::track::Track,
    stop: Arc<std::sync::atomic::AtomicBool>,
) {
    // Per-kind OWNERSHIP: a republish (AGC toggle, share-quality change) spawns the
    // successor task while the old one may still be inside a 3s sleep or an in-flight
    // get_stats(). Without ownership, the OLD task's exit prune (or last tick's write)
    // lands AFTER the new task's first write and deletes/overwrites it — the popup's
    // card blinks out (or goes stale) for up to 3s on every toggle. Registering the
    // stop-flag pointer synchronously HERE (before spawn) makes the newest task the
    // owner; a superseded task skips every map mutation and exits silently.
    fn stats_owner() -> &'static Mutex<HashMap<&'static str, usize>> {
        static O: OnceLock<Mutex<HashMap<&'static str, usize>>> = OnceLock::new();
        O.get_or_init(|| Mutex::new(HashMap::new()))
    }
    let my_token = Arc::as_ptr(&stop) as usize;
    stats_owner().lock().unwrap().insert(kind, my_token);
    tokio::spawn(async move {
        use livekit::webrtc::stats::RtcStats;
        let i_own = || stats_owner().lock().unwrap().get(kind) == Some(&my_token);
        let prune_if_owner = move || {
            if stats_owner().lock().unwrap().get(kind) != Some(&my_token) {
                return; // superseded — the successor's entries are not ours to delete
            }
            let mut map = publish_stats().lock().unwrap();
            let prefix = format!("{kind}/");
            map.retain(|k, _| k != kind && !k.starts_with(&prefix));
        };
        // ssrc -> (bytesSent, timestamp µs) for send-bitrate deltas. bytesSent already
        // INCLUDES retransmitted payload per webrtc-stats (retransmittedBytesSent is a
        // subset) — adding rtx on top overread the send rate by exactly the RTX fraction,
        // i.e. precisely when the link was degraded and the number mattered most.
        let mut prev: HashMap<u32, (u64, i64)> = HashMap::new();
        // Auto-resolution governor state (screen kind only; see lk_publish::AUTO_RUNGS).
        let mut auto_ema: f64 = 0.0;
        let mut auto_agree: u32 = 0;
        let mut auto_last_refit: Option<std::time::Instant> = None;
        let mut stats_errs: u32 = 0;
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            if stop.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }
            // 10.5 (audio review #6): an error is not the end of the stream. A full reconnect
            // unpublishes and republishes the SAME track (no transceiver in between = an error
            // here), and the old `break` ended this task for the rest of the call — the health
            // card went stale and nothing measured the mic again. The owner's stop flag ends a
            // stream; an error skips a tick.
            let stats = match track.get_stats().await {
                Ok(s) => {
                    stats_errs = 0;
                    s
                }
                Err(e) => {
                    stats_errs += 1;
                    if stats_errs == 1 {
                        eprintln!("[lk-stats] {kind}: stats unavailable ({e}) — retrying every 3 s");
                    }
                    continue;
                }
            };
            // Re-check AFTER the await: a teardown that flipped the flag mid-poll must
            // not have this stale result written over the successor's fresh entry (or
            // into a just-cleared registry ~3s into the NEXT call).
            if stop.load(std::sync::atomic::Ordering::Relaxed) || !i_own() {
                break;
            }
            // Receiver reports keyed by ssrc.
            let mut rr: HashMap<u32, (f64, f64, f64, u64)> = HashMap::new();
            for s in &stats {
                if let RtcStats::RemoteInboundRtp(r) = s {
                    rr.insert(
                        r.stream.ssrc,
                        (
                            r.remote_inbound.fraction_lost,
                            r.remote_inbound.round_trip_time,
                            r.received.jitter,
                            r.remote_inbound.round_trip_time_measurements,
                        ),
                    );
                }
            }
            let mut entries: Vec<(String, String)> = Vec::new();
            for s in &stats {
                let RtcStats::OutboundRtp(o) = s else { continue };
                let ob = &o.outbound;
                match kind {
                    // Legacy gate lines — DO NOT reformat (ship gates grep these).
                    "screen" => {
                        let tgt_kbps = (ob.target_bitrate / 1000.0) as u64;
                        eprintln!(
                        "[lk-stats] enc='{}' {}x{}@{:.0} target={}kbps qlim={:?} resChanges={} key={} nack={} pli={} fir={}",
                        ob.encoder_implementation,
                        ob.frame_width, ob.frame_height, ob.frames_per_second,
                        tgt_kbps,
                        ob.quality_limitation_reason,
                        ob.quality_limitation_resolution_changes,
                        ob.key_frames_encoded, ob.nack_count, ob.pli_count, ob.fir_count,
                        );
                        // ── Auto-resolution governor ─────────────────────────────
                        // The GRANT (libwebrtc's target_bitrate) is the ground truth
                        // of what this uplink sustains. Downshift when it sits under
                        // the rung's floor; probe one rung up when it saturates our
                        // own cap (the only upgrade signal — BWE never reports more
                        // than we request). 4 agreeing ticks (~12s) + 30s cooldown
                        // keep it from flapping; each refit re-publishes via the
                        // app-installed hook (same portal cast, no re-prompt).
                        if crate::lk_publish::share_auto_res() && tgt_kbps > 0 {
                            auto_ema = if auto_ema == 0.0 {
                                tgt_kbps as f64
                            } else {
                                auto_ema * 0.6 + tgt_kbps as f64 * 0.4
                            };
                            let (cur, cap, floor) = crate::lk_publish::auto_state();
                            let want = if auto_ema >= cap as f64 * 0.92 {
                                crate::lk_publish::auto_rung_above(cur).unwrap_or(cur)
                            } else if auto_ema < floor as f64 * 0.85 {
                                crate::lk_publish::auto_rung_for_kbps(auto_ema as u32)
                            } else {
                                cur
                            };
                            if want != cur {
                                auto_agree += 1;
                            } else {
                                auto_agree = 0;
                            }
                            let cooled = auto_last_refit
                                .map(|t| t.elapsed() >= std::time::Duration::from_secs(30))
                                .unwrap_or(true);
                            if auto_agree >= 4 && cooled {
                                auto_agree = 0;
                                auto_last_refit = Some(std::time::Instant::now());
                                crate::lk_publish::set_auto_rung(want);
                                eprintln!(
                                    "[share-auto] grant ~{auto_ema:.0}kbps vs cap {cap}/floor {floor} -> refit {}x{} (was {}x{})",
                                    want.0, want.1, cur.0, cur.1
                                );
                                auto_ema = 0.0; // fresh measurement at the new rung
                                let hook = share_refit_hook_cell().lock().unwrap().clone();
                                if let Some(h) = hook {
                                    h();
                                }
                            }
                        }
                    }
                    // qlim/resChanges were only on the SCREEN line; without them the
                    // 2026-08 "camera parked at 320x180 for 89 minutes" collapses were
                    // un-triageable from field logs (loss? CPU? quality scaler? — the
                    // limitation reason is exactly the field that says).
                    "camera" => eprintln!(
                        "[lk-stats-cam] rid='{}' enc='{}' {}x{}@{:.0} target={}kbps qlim={:?} resChanges={} key={} nack={} pli={}",
                        ob.rid, ob.encoder_implementation, ob.frame_width, ob.frame_height,
                        ob.frames_per_second, (ob.target_bitrate / 1000.0) as u64,
                        ob.quality_limitation_reason,
                        ob.quality_limitation_resolution_changes,
                        ob.key_frames_encoded, ob.nack_count, ob.pli_count,
                    ),
                    _ => {}
                }
                let bytes = o.sent.bytes_sent; // includes RTX (see `prev` comment above)
                let now_ts = o.rtc.timestamp; // µs
                if kind == "mic" {
                    crate::hear::note_mic_bytes(bytes); // 10.5: "nothing has left this computer"
                }
                // 10.5: the first report has no previous sample, so its rate is not a measurement
                // ("Looking good" used to show over a 0 kbps first tick).
                let send_known = prev.contains_key(&o.stream.ssrc);
                let send_kbps = prev
                    .get(&o.stream.ssrc)
                    .map(|&(pb, pt)| {
                        let db = bytes.saturating_sub(pb) as f64;
                        let dt = ((now_ts - pt) as f64 / 1_000_000.0).max(0.001);
                        (db * 8.0 / dt / 1000.0) as u64
                    })
                    .unwrap_or(0);
                prev.insert(o.stream.ssrc, (bytes, now_ts));
                let (loss_pct, rtt_ms, jit_ms) = rr
                    .get(&o.stream.ssrc)
                    .filter(|v| v.3 > 0) // no RTCP report yet -> sentinels
                    .map(|v| (v.0 * 100.0, (v.1 * 1000.0) as i64, (v.2 * 1000.0) as i64))
                    .unwrap_or((-1.0, -1, -1));
                let key = if kind == "camera" && !ob.rid.is_empty() {
                    format!("camera/{}", ob.rid)
                } else {
                    kind.to_string()
                };
                let enc = if ob.encoder_implementation.is_empty() {
                    enc_pick.clone()
                } else {
                    ob.encoder_implementation.clone()
                };
                entries.push((
                    key.clone(),
                    format!(
                        "{{\"kind\":\"{}\",\"enc\":\"{}\",\"encPick\":\"{}\",\"rid\":\"{}\",\
                         \"w\":{},\"h\":{},\"fps\":{:.0},\"targetKbps\":{},\"sendKbps\":{},\
                         \"qlim\":\"{}\",\"lossPct\":{:.1},\"rttMs\":{},\"jitterMs\":{},\
                         \"key\":{},\"nack\":{},\"pli\":{},\"active\":{},\"sendKnown\":{}}}",
                        stats_esc(&key),
                        stats_esc(&enc),
                        stats_esc(&enc_pick),
                        stats_esc(&ob.rid),
                        ob.frame_width,
                        ob.frame_height,
                        ob.frames_per_second,
                        (ob.target_bitrate / 1000.0) as u64,
                        send_kbps,
                        format!("{:?}", ob.quality_limitation_reason).to_lowercase(),
                        loss_pct,
                        rtt_ms,
                        jit_ms,
                        ob.key_frames_encoded,
                        ob.nack_count,
                        ob.pli_count,
                        ob.active,
                        send_known,
                    ),
                ));
            }
            if !i_own() {
                break; // superseded while building — leave the successor's entries alone
            }
            let mut map = publish_stats().lock().unwrap();
            let prefix = format!("{kind}/");
            map.retain(|k, _| k != kind && !k.starts_with(&prefix)); // prune stale rids
            for (k, v) in entries {
                map.insert(k, v);
            }
        }
        // Exit prune ONLY while still the owner — a superseded task must not delete the
        // successor's freshly-written entries.
        prune_if_owner();
    });
}

/// Set a remote participant's playback gain for one audio `category` ("mic" /
/// "screenshare" / "music"). `gain` is a linear factor (1.0 = unity = no change);
/// clamped to `[0.0, 2.0]` (0–200%). NO-OP (no panic) if the track is absent/dropped.
/// Thread-safe + idempotent; safe to call off the audio thread (gst props are
/// thread-safe). Only TOUCHES audio once the user moves a slider — unity stays
/// byte-identical because `build_playback` already defaults the element to 1.0.
pub fn set_track_gain(user_id: &str, category: &str, gain: f64) {
    // NaN-safe: f64::clamp passes NaN through unchanged, and a NaN on the gst `volume`
    // prop would be a degradation. No current caller can produce NaN, but guard anyway.
    let g = if gain.is_finite() { gain.clamp(0.0, 2.0) } else { 1.0 };
    // Record the desire so a track subscribing LATER under this user/category starts at
    // this volume too (register_track_gain applies it).
    {
        let mut d = desired_track_prefs().lock().unwrap();
        let e = d.entry((user_id.to_owned(), category.to_owned())).or_insert((1.0, false));
        e.0 = g;
    }
    // 9.4: the element plays pref × corner factor — resolve WITH the identity so the factor
    // (which is per full identity) applies; a slider move mid-aside must not undo the 10 %.
    let prefix = format!("{user_id}:");
    let entries: Vec<(String, glib::WeakRef<gst::Element>)> = track_gains()
        .lock()
        .unwrap()
        .iter()
        .filter(|((id, cat, _), _)| cat == category && (id == user_id || id.starts_with(&prefix)))
        .map(|((id, _, _), w)| (id.clone(), w.clone()))
        .collect();
    for (id, w) in entries {
        if let Some(el) = w.upgrade() {
            el.set_property("volume", effective_volume(&id, category));
        }
    }
}

/// Mute/unmute a remote participant's audio for one `category` (uses the `volume`
/// element's `mute` prop, so it composes with the gain and restores cleanly). NO-OP if
/// the track is absent/dropped.
pub fn set_track_muted(user_id: &str, category: &str, muted: bool) {
    // Record the desire for late subscribers (register_track_gain applies it).
    {
        let mut d = desired_track_prefs().lock().unwrap();
        let e = d.entry((user_id.to_owned(), category.to_owned())).or_insert((1.0, false));
        e.1 = muted;
    }
    // Apply through the layered composer (0.8.23): the user's pref is ONE input next to
    // recording-only / not-watched / game-chat, and the deafen/AFK ownership rule lives in
    // one place — an un-mute while deafened/AFK stays silent and is handed to the auto
    // layer (restored when the cut clears); a mute drops any auto ownership so the cut's
    // clear can't clobber it.
    let prefix = format!("{user_id}:");
    let ids: Vec<String> = {
        let g = track_gains().lock().unwrap();
        let mut v: Vec<String> = g
            .keys()
            .filter(|(id, cat, _)| cat == category && (id == user_id || id.starts_with(&prefix)))
            .map(|(id, _, _)| id.clone())
            .collect();
        v.sort();
        v.dedup();
        v
    };
    let _ = muted;
    for id in ids {
        apply_layered_mute(&id, category);
    }
}

// ── #4 AFK/BRB incoming-audio cut (reversible, won't stomp #5 per-user mutes) ─
//
// When the user goes AFK they must stop HEARING the call (privacy). We mute EVERY live
// remote-audio `volume` element, but record which ones we actually changed — i.e. those
// that were NOT already muted (a #5 per-user mute the user set). On clear we unmute ONLY
// the ones AFK muted, so a participant the user muted via #5 stays muted. `away_active`
// guards the no-op: not AFK => never touched, so a non-AFK call is byte-identical.

/// Was the global AFK incoming-audio mute applied? (So `close()`/teardown can reason about it.)
fn away_active() -> &'static Mutex<bool> {
    static A: OnceLock<Mutex<bool>> = OnceLock::new();
    A.get_or_init(|| Mutex::new(false))
}

/// Was the deafen (mute-all-output) state requested? A second independent input to the same
/// reversible auto-mute as AFK: effective mute = `away_active || deaf_active`.
fn deaf_active() -> &'static Mutex<bool> {
    static D: OnceLock<Mutex<bool>> = OnceLock::new();
    D.get_or_init(|| Mutex::new(false))
}

/// The set of tracks (keyed by the SAME `(identity, category)` tuple as [`track_gains`])
/// that the AFK mute itself flipped to muted (i.e. they were unmuted before AFK). On clear
/// we unmute exactly these, leaving #5 user-mutes intact. Keying by the tuple (not the
/// sanitized element name) avoids any name-collision risk and lets the WeakRefs be looked
/// up directly through `track_gains`.
fn away_muted_names() -> &'static Mutex<std::collections::HashSet<(String, String, String)>> {
    static S: OnceLock<Mutex<std::collections::HashSet<(String, String, String)>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}

/// #5/AFK coordination: when the user applies a #5 per-user MUTE (or unmute) while AFK,
/// the AFK layer must stop "owning" that track so an AFK-clear can't clobber the #5 state.
/// Called from the #5 mute path ([`set_track_muted`]) for every (identity, category) that
/// resolves to the given `user_id`. NO-OP when not AFK / nothing recorded.
fn forget_away_muted(user_id: &str, category: &str) {
    let prefix = format!("{user_id}:");
    away_muted_names()
        .lock()
        .unwrap()
        .retain(|(id, cat, _)| !(cat == category && (id == user_id || id.starts_with(&prefix))));
}

/// #4 AFK: mute ALL incoming remote audio (so the away user can't hear the call), recording
/// which elements WE muted so [`set_away_privacy(false)`] can restore them without clobbering
/// a #5 per-user mute. Idempotent: re-applying just (re)mutes any newly-subscribed tracks.
/// When `away == false`, unmute ONLY the elements this AFK mute flipped. NO-OP if `away`
/// already matches the stored flag AND no new tracks appeared (re-applying is harmless).
pub fn set_away_privacy(away: bool) {
    *away_active().lock().unwrap() = away;
    reconcile_auto_mute();
}

/// Deafen: request muting ALL incoming remote audio (the user can't hear the call). A second
/// independent input to the SAME reversible auto-mute as AFK — the effective mute is
/// `afk || deafened`, so audio un-mutes only when BOTH clear, and a #5 per-user mute set while
/// muted is never clobbered (same `forget_away_muted` ownership rule covers both).
pub fn set_deafened(deaf: bool) {
    *deaf_active().lock().unwrap() = deaf;
    reconcile_auto_mute();
}

/// Is the AFK incoming-audio cut currently applied? (The app reapplies it onto newly
/// -subscribed tracks while still AFK.)
pub fn away_privacy_active() -> bool {
    *away_active().lock().unwrap()
}

/// Is deafen currently applied?
pub fn deafened_active() -> bool {
    *deaf_active().lock().unwrap()
}

/// Apply the combined AFK+deafen incoming-audio mute. When effective (`away || deaf`), mute
/// every live remote-audio element that isn't already muted, recording exactly the ones we
/// flip (so a #5 user-mute is left alone); when not effective, unmute only the ones this layer
/// muted. Idempotent — the app re-applies it (~120ms while away) so newly-subscribed tracks
/// inherit the current state. A single owned-set serves both inputs, so the two can't fight.
pub(crate) fn reconcile_auto_mute() {
    // 11.3 same room: "hear the call through <linked peer>'s speakers" cuts my playback like deafen
    let effective = *away_active().lock().unwrap() || *deaf_active().lock().unwrap() || crate::sameroom::speakers_off();
    if effective {
        let mut muted = away_muted_names().lock().unwrap();
        let entries: Vec<((String, String, String), glib::WeakRef<gst::Element>)> = track_gains()
            .lock()
            .unwrap()
            .iter()
            .map(|(k, w)| (k.clone(), w.clone()))
            .collect();
        let mut newly = 0;
        for (key, weak) in entries {
            if let Some(el) = weak.upgrade() {
                if !el.property::<bool>("mute") {
                    el.set_property("mute", true);
                    if muted.insert(key) {
                        newly += 1;
                    }
                }
            }
        }
        // Only log when the cut actually changed (re-applied every ~120ms while away).
        if newly > 0 {
            eprintln!("[privacy] incoming audio cut ({newly} new track(s) muted)");
        }
    } else {
        let keys: Vec<(String, String, String)> = away_muted_names().lock().unwrap().drain().collect();
        // Restore only the tracks this layer muted, by their (identity,category) key. A #5 mute
        // set while muted called forget_away_muted, so its key is already gone — never clobbered.
        let live = track_gains().lock().unwrap();
        let mut restored = 0;
        for key in keys {
            if let Some(el) = live.get(&key).and_then(|w| w.upgrade()) {
                el.set_property("mute", false);
                restored += 1;
            }
        }
        if restored > 0 {
            eprintln!("[privacy] incoming audio restored ({restored} track(s) unmuted)");
        }
    }
}

// ── Public 2nd-AUDIO-TRACK API (soundboard + #7 music) ───────────────────────
//
// These drive the opt-in 2nd audio track (defined below: `publish_aux`/`teardown_aux`).
// They are NO-OPs (clear log, no panic) when not in a call. A call with nothing playing
// never creates a track, so it stays byte-identical to before. The mic is never touched.

/// SOUNDBOARD: play a one-shot sound from `url` into the active call (stop-then-replace any
/// current sound). Auto-unpublishes on EOS. NO-OP (logs) if not in a call. Async (publishes
/// a track); the soundboard controller awaits it on the bridge runtime.
// ── who is watching? ─────────────────────────────────────────────────────────
/// True while at least one call member is NOT a Tin Hat native client. Set by the
/// app from the room's call memberships (rtc::non_native_call_members) at join and on a
/// 5s scan; read by the share-encoder resolver.
static BROWSER_PEERS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Record whether a browser / Element X peer is in the call. Returns true when the
/// value CHANGED, so the caller can re-publish a live share on the flip.
pub fn set_browser_peers_present(on: bool) -> bool {
    BROWSER_PEERS.swap(on, std::sync::atomic::Ordering::Relaxed) != on
}

pub fn browser_peers_present() -> bool {
    BROWSER_PEERS.load(std::sync::atomic::Ordering::Relaxed)
}

// ---- Memories recording (0.8.7) -----------------------------------------------------
// When RECORD_DIR is set (ONLY by the recorder service — never a user client), every
// subscribed remote track gets a CAPTURE chain writing a per-track file into that
// directory instead of a playback/render chain (the service is headless; it has no
// audio device and no GUI). CONSENT IS ENFORCED HERE: an identity whose bare mxid is
// in RECORD_OPTOUTS ("Never record me", advertised in the membership marker and
// refreshed live by the service) gets NO chain — their audio/video never touches disk.

fn record_dir_slot() -> &'static Mutex<Option<String>> {
    static D: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    D.get_or_init(|| Mutex::new(None))
}

/// Set (or clear) the Memories capture directory. recorder service only.
pub fn set_record_dir(dir: Option<String>) {
    *record_dir_slot().lock().unwrap() = dir;
}

fn record_dir() -> Option<String> {
    record_dir_slot().lock().unwrap().clone()
}

fn record_optouts() -> &'static Mutex<std::collections::HashSet<String>> {
    static S: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}

/// Replace the "never record me" set (bare mxids), from the room's membership markers.
pub fn set_record_optouts(mxids: Vec<String>) {
    let mut s = record_optouts().lock().unwrap();
    s.clear();
    s.extend(mxids);
}

/// Bare mxid of a LiveKit identity ("@user:server:DEVICE" → "@user:server").
fn record_mxid_of(identity: &str) -> String {
    let mut it = identity.splitn(3, ':');
    match (it.next(), it.next()) {
        (Some(a), Some(b)) => format!("{a}:{b}"),
        _ => identity.to_string(),
    }
}

/// True = "never record me": build NO capture chain for this identity.
pub fn record_excluded(identity: &str) -> bool {
    record_optouts().lock().unwrap().contains(&record_mxid_of(identity))
}

/// Live capture chains, keyed "a:<sid>" / "v:<sid>" so unsubscribes tear down one.
fn record_chains() -> &'static Mutex<HashMap<String, (gst::Pipeline, tokio::task::JoinHandle<()>)>> {
    static R: OnceLock<Mutex<HashMap<String, (gst::Pipeline, tokio::task::JoinHandle<()>)>>> =
        OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A subscribed track waiting for a consent-checked capture chain. TrackSubscribed
/// only QUEUES — building happens in [`drain_record_pending`], which the service
/// calls right after refreshing the opt-out set. This ordering is the fix for the
/// join race the first live gate caught: a late joiner's track subscribed before
/// the polled opt-out refresh could see their marker, and a "never record me"
/// user's audio reached disk.
pub enum PendingRecord {
    Audio { track: livekit::webrtc::audio_track::RtcAudioTrack, identity: String, sid: String },
    Video {
        track: livekit::webrtc::video_track::RtcVideoTrack,
        identity: String,
        sid: String,
        screenshare: bool,
    },
}

fn record_pending() -> &'static Mutex<Vec<PendingRecord>> {
    static P: OnceLock<Mutex<Vec<PendingRecord>>> = OnceLock::new();
    P.get_or_init(|| Mutex::new(Vec::new()))
}

/// How many tracks are queued (tick diagnostics).
pub fn record_pending_len() -> usize {
    record_pending().lock().unwrap().len()
}

/// Build capture chains for every queued track whose identity has NOT opted out —
/// call ONLY after set_record_optouts() with fresh marker data. Excluded tracks are
/// dropped with a SKIP log (re-queueing would just re-race; an opt-out flip back
/// takes effect at that user's next call join, exactly what the Settings text says).
pub fn drain_record_pending() {
    let Some(dir) = record_dir() else {
        record_pending().lock().unwrap().clear();
        return;
    };
    let drained: Vec<PendingRecord> = record_pending().lock().unwrap().drain(..).collect();
    for p in drained {
        match p {
            PendingRecord::Audio { track, identity, sid } => {
                if record_excluded(&identity) {
                    eprintln!("[vod] SKIP audio {identity} (never-record-me)");
                    continue;
                }
                match build_record_audio(track, &identity, &sid, &dir) {
                    Ok(pt) => {
                        record_chains().lock().unwrap().insert(format!("a:{sid}"), pt);
                    }
                    Err(e) => eprintln!("[vod] audio record build failed: {e}"),
                }
            }
            PendingRecord::Video { track, identity, sid, screenshare } => {
                if record_excluded(&identity) {
                    eprintln!("[vod] SKIP video {identity} (never-record-me)");
                    continue;
                }
                match build_record_video(track, &identity, &sid, screenshare, &dir) {
                    Ok(pt) => {
                        record_chains().lock().unwrap().insert(format!("v:{sid}"), pt);
                    }
                    Err(e) => eprintln!("[vod] video record build failed: {e}"),
                }
            }
        }
    }
}

fn record_fname(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '@' | '.' | '_' | '-') { c } else { '_' })
        .collect()
}

/// Append one captured track to <dir>/tracks.jsonl — the POV timeline: each line's
/// start_ms aligns this file against the others in the session.
fn record_note_track(dir: &str, file: &str, identity: &str, kind: &str) {
    use std::io::Write;
    let start_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    if let Ok(mut f) =
        std::fs::OpenOptions::new().create(true).append(true).open(format!("{dir}/tracks.jsonl"))
    {
        let _ = writeln!(
            f,
            r#"{{"file":"{file}","identity":"{identity}","kind":"{kind}","start_ms":{start_ms}}}"#
        );
    }
}

/// AUDIO capture: appsrc S16LE 48k stereo → Opus → .mka. `streamable=true` so even a
/// crash mid-call leaves a playable file; teardown still sends EOS for a clean finalize.
fn build_record_audio(
    rtc_track: livekit::webrtc::audio_track::RtcAudioTrack,
    identity: &str,
    sid: &str,
    dir: &str,
) -> Result<(gst::Pipeline, tokio::task::JoinHandle<()>)> {
    let file = format!("audio-{}-{}.mka", record_fname(identity), record_fname(sid));
    let path = format!("{dir}/{file}");
    let desc = format!(
        "appsrc name=src is-live=true format=time do-timestamp=true !          audio/x-raw,format=S16LE,rate=48000,channels=2,layout=interleaved !          audioconvert ! opusenc bitrate=96000 ! matroskamux streamable=true !          filesink location={path}"
    );
    let pipeline = gst::parse::launch(&desc)?
        .downcast::<gst::Pipeline>()
        .map_err(|_| anyhow::anyhow!("record-audio: not a Pipeline"))?;
    let appsrc = pipeline
        .by_name("src")
        .context("record-audio appsrc missing")?
        .downcast::<gstreamer_app::AppSrc>()
        .map_err(|_| anyhow::anyhow!("record-audio src not an AppSrc"))?;
    pipeline.set_state(gst::State::Playing)?;
    record_note_track(dir, &file, identity, "audio");
    eprintln!("[vod] recording AUDIO {identity} -> {file}");
    let mut stream = NativeAudioStream::new(rtc_track, SAMPLE_RATE as i32, 2);
    let task = tokio::spawn(async move {
        while let Some(frame) = stream.next().await {
            let mut bytes = Vec::with_capacity(frame.data.len() * 2);
            for s in frame.data.iter() {
                bytes.extend_from_slice(&s.to_le_bytes());
            }
            if appsrc.push_buffer(gst::Buffer::from_slice(bytes)).is_err() {
                break;
            }
        }
        let _ = appsrc.end_of_stream();
    });
    Ok((pipeline, task))
}

/// VIDEO capture: I420 appsrc (caps set by the shared feeder) → NVENC H.264 (the 5090
/// is rigged into this container; x264 fallback keeps a driver hiccup from dropping
/// the recording) → .mkv. Reuses [`spawn_video_feeder`] — the exact same frame pump
/// the render tiles use, so stride/caps handling can't drift.
fn build_record_video(
    rtc_track: livekit::webrtc::video_track::RtcVideoTrack,
    identity: &str,
    sid: &str,
    is_screenshare: bool,
    dir: &str,
) -> Result<(gst::Pipeline, tokio::task::JoinHandle<()>)> {
    let kind = if is_screenshare { "share" } else { "cam" };
    let kbps: u32 = if is_screenshare { 8000 } else { 4000 };
    let file = format!("{kind}-{}-{}.mkv", record_fname(identity), record_fname(sid));
    let path = format!("{dir}/{file}");
    let enc = if gst::ElementFactory::find("nvh264enc").is_some() {
        format!("nvh264enc bitrate={kbps} ! h264parse")
    } else {
        format!("x264enc bitrate={kbps} speed-preset=veryfast key-int-max=120 ! h264parse")
    };
    let desc = format!(
        "appsrc name=src is-live=true format=time do-timestamp=true !          queue max-size-buffers=8 leaky=downstream ! videoconvert ! {enc} !          matroskamux streamable=true ! filesink location={path}"
    );
    let pipeline = gst::parse::launch(&desc)?
        .downcast::<gst::Pipeline>()
        .map_err(|_| anyhow::anyhow!("record-video: not a Pipeline"))?;
    let appsrc = pipeline
        .by_name("src")
        .context("record-video appsrc missing")?
        .downcast::<gstreamer_app::AppSrc>()
        .map_err(|_| anyhow::anyhow!("record-video src not an AppSrc"))?;
    pipeline.set_state(gst::State::Playing)?;
    record_note_track(dir, &file, identity, kind);
    eprintln!("[vod] recording VIDEO ({kind}) {identity} -> {file}");
    let task = spawn_video_feeder(rtc_track, appsrc, format!("vodrec {identity}"));
    Ok((pipeline, task))
}

/// Finalize every capture chain: EOS → brief flush → NULL. Called by close() (so a
/// normal leave finalizes) and by the service between recordings. No-op for clients.
pub fn finalize_recordings() {
    record_pending().lock().unwrap().clear(); // never-built queued tracks just drop
    let drained: Vec<_> = record_chains().lock().unwrap().drain().collect();
    if drained.is_empty() {
        return;
    }
    eprintln!("[vod] finalizing {} capture chain(s)", drained.len());
    for (_k, (p, task)) in drained {
        if let Some(src) = p.by_name("src") {
            if let Ok(a) = src.downcast::<gstreamer_app::AppSrc>() {
                let _ = a.end_of_stream();
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
        let _ = p.set_state(gst::State::Null);
        task.abort();
    }
}

pub async fn play_url_in_call(url: &str) -> Result<()> {
    if aux_participant().lock().unwrap().is_none() {
        eprintln!("[aux/sound] not in a call — join a call to play sounds (no-op)");
        return Ok(());
    }
    publish_aux(AuxKind::Sound, url).await
}

/// SOUNDBOARD: stop the current one-shot sound (unpublish + tear down). Idempotent NO-OP if
/// nothing is playing or not in a call.
pub fn stop_url_in_call() {
    teardown_aux(AuxKind::Sound.track_name());
}

/// #7 MUSIC: start (or replace) the longer-lived "music" 2nd-track from `url`/file. Same
/// infra as a soundboard sound, just a distinct slot so a sound + music can play at once.
/// Auto-unpublishes on EOS. NO-OP (logs) if not in a call.
pub async fn start_music(url: &str) -> Result<()> {
    if aux_participant().lock().unwrap().is_none() {
        eprintln!("[aux/music] not in a call — join a call to play music (no-op)");
        return Ok(());
    }
    publish_aux(AuxKind::Music, url).await
}

/// #7 MUSIC: stop the music stream (unpublish + tear down). Idempotent NO-OP if not playing.
pub fn stop_music() {
    teardown_aux(AuxKind::Music.track_name());
}

/// #7 MUSIC: is the music 2nd-track currently live? Goes false once a finite music file hits
/// EOS (the bus watch tears the slot down). The app polls this from its 120ms mirror to clear
/// `musicPlaying` when a track finishes on its own (no stop click), so "Stop music" doesn't
/// stay falsely enabled.
pub fn is_music_playing() -> bool {
    aux_tracks().lock().unwrap().contains_key(AuxKind::Music.track_name())
}

/// Soundboard (0.8.21): is the "sound" 2nd-track live? The panel keeps its playing
/// state while the call side is still audible even if the LOCAL preview failed, and
/// clears it once both are idle.
pub fn is_sound_playing() -> bool {
    aux_tracks().lock().unwrap().contains_key(AuxKind::Sound.track_name())
}

/// Live remote-audio playback sinks (the named `pipewiresink` in each playback pipeline),
/// so [`set_playback_target`] can re-point ALL of them mid-call, not just future streams.
fn playback_sinks() -> &'static Mutex<Vec<glib::WeakRef<gst::Element>>> {
    static S: OnceLock<Mutex<Vec<glib::WeakRef<gst::Element>>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(Vec::new()))
}

/// Choose where remote call audio plays (a PipeWire sink node name from
/// [`list_audio_outputs`]); `None` = system default. Re-points EVERY live playback sink so a
/// mid-call output switch moves everyone already talking, not just new streams.
pub fn set_playback_target(node: Option<String>) {
    let node = node.filter(|s| !s.is_empty());
    save_saved_device("call-output", node.as_deref()); // remember the SPEAKER across launches
    *playback_target().lock().unwrap() = node.clone();
    *output_picked_at().lock().unwrap() = Some(std::time::Instant::now()); // 10.5.2 (current_ec_plan)
    // While AEC is up, live sinks must STAY on the EC reference sink — what needs
    // re-binding to the new speaker is the AEC module itself, and the call_ctl layer
    // invokes apply_aec() on an output change, which reloads it against the new
    // sink_master and re-points the sinks exactly once. Re-pointing to the raw device
    // here would detach playback from the canceller: everyone else's echo comes back
    // AND AEC3 keeps adapting against a now-empty reference (near-end mangling).
    if aec_nodes().lock().unwrap().is_some() {
        return;
    }
    let _ = repoint_playback_sinks(node.unwrap_or_default()); // "" => default device
    // NOTE: re-pointing the output METER to the new sink (if it's running) is done by the
    // call_ctl layer (CallController::set_output), which drives the stop/start through the
    // OFF-GUI-thread wrappers. Building a pipewiresrc + PLAYING transition synchronously here
    // would stall the Qt GUI thread — set_playback_target is called inline from the QML picker.
}

/// NULL-cycle every live playback sink onto `target` ("" = default device) on a DETACHED
/// thread: NULLing a live pipewiresink disconnects/reconnects its pw_stream (PipeWire
/// round-trips per sink) and callers may be on the Qt GUI thread. pipewiresink only reads
/// `target-object` when it (re)connects its pw_stream — at a state transition, NOT live —
/// so the NULL cycle is what makes the re-point actually take effect (a bare set_property
/// is ignored until some later reconnect). A brief, ~unnoticeable gap; the upstream
/// `queue` absorbs buffers during the flip. Dead WeakRefs are skipped (upgrade() fails)
/// and close() clears the list, so no pruning here.
///
/// Returns the JoinHandle (resolving to `true` when the FULL snapshot was walked,
/// `false` when a newer generation superseded this walk mid-list): AEC teardown must
/// WAIT for a COMPLETE re-point before destroying the node the sinks are leaving
/// (unloading the module under actively-streaming sinks kicks them to whatever
/// WirePlumber picks). The generation guard means a slow worker for an OLDER switch
/// sees a newer generation and abandons, so two rapid re-points can't interleave.
// ── Platform audio elements (windows/MEDIA_PIPELINE.md §3) ───────────────────
// Linux routes audio through PipeWire (`pipewiresrc`/`pipewiresink`, addressed by
// `target-object` = node name; a sink's loopback is its `<node>.monitor` source).
// Windows uses WASAPI via wasapi2 (`wasapi2src`/`wasapi2sink`, addressed by
// `device` = endpoint id; loopback = `loopback=true` on a RENDER endpoint — 1.28's
// wasapi2src DOES implement loopback incl. per-process, contrary to the older doc).
// Everything downstream of the head (audioconvert/audioresample/audiobuffersplit/
// audiorate/audiornnoise/queue/appsink — the whole 10ms crackle-fix feeder chain) is
// cross-platform and shared verbatim. Pure string builders → cfg! not #[cfg].

/// Audio capture head. `None` = system default capture device.
fn audio_src_fragment(node: Option<&str>) -> String {
    // 10.0.3: the PulseAudio-compat head, once the healer decided pipewiresrc is dead here.
    if !cfg!(windows) && pulse_head_wanted() {
        match node {
            Some(n) if !n.starts_with("app:") && !n.starts_with("pid:") => {
                return format!("pulsesrc device=\"{n}\" stream-properties=\"props,{PA_NO_RESTORE}\"")
            }
            None => return format!("pulsesrc stream-properties=\"props,{PA_NO_RESTORE}\""),
            _ => {}
        }
    }
    // 0.8.22: per-APPLICATION capture. Ids come from the app crate's enumerator:
    //   "app:<PipeWire object.serial>" — capture that Stream/Output/Audio node's output
    //   (what `pw-record --target` does; proven in the rig: a pw-play node captured at
    //   -12 dB RMS), i.e. ONE program's sound, never the mix and never our own playback;
    //   "pid:<process id>"            — WASAPI process loopback (include-process-tree),
    //   the device-less per-app capture Windows 10 20H1+ offers (wasapi2src ≥ 1.22).
    if let Some(n) = node {
        if let Some(serial) = n.strip_prefix("app:") {
            if !cfg!(windows) {
                return format!("pipewiresrc target-object=\"{serial}\" stream-properties=\"props,{PW_NO_RESTORE}\"");
            }
        }
        if let Some(pid) = n.strip_prefix("pid:") {
            if cfg!(windows) {
                return format!(
                    "wasapi2src loopback=true loopback-mode=include-process-tree loopback-target-pid={pid}"
                );
            }
        }
    }
    if cfg!(windows) {
        match node {
            // PipeWire's `<sink>.monitor` loopback naming, mapped to WASAPI loopback.
            Some(n) if n.ends_with(".monitor") => {
                let dev = n.trim_end_matches(".monitor");
                // Exclude OUR OWN process from the loopback. A plain endpoint
                // loopback re-captures everything Tin Hat itself plays on that
                // endpoint — every remote voice, soundboard clip and join sound — and
                // publishes it straight back into the call ~200 ms late, so peers hear
                // themselves whenever someone shares audio. Windows 10 20H1+ has process
                // loopback (AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK); wasapi2src
                // exposes it as loopback-mode + loopback-target-pid. In that mode the
                // capture is "every process except ours" and the API is device-less, so
                // the endpoint pick is moot — which is what a "share my game / music"
                // control should mean anyway. Falls back to the endpoint loopback where
                // the element predates the property.
                let excl = wasapi2_process_loopback_fragment();
                if !excl.is_empty() {
                    format!("wasapi2src loopback=true {excl}")
                } else if dev.is_empty() {
                    "wasapi2src loopback=true".to_string()
                } else {
                    format!("wasapi2src loopback=true device=\"{dev}\"")
                }
            }
            Some(n) => format!("wasapi2src device=\"{n}\""),
            None => "wasapi2src".to_string(),
        }
    } else {
        match node {
            Some(n) => format!("pipewiresrc target-object=\"{n}\" stream-properties=\"props,{PW_NO_RESTORE}\""),
            None => format!("pipewiresrc stream-properties=\"props,{PW_NO_RESTORE}\""),
        }
    }
}

/// `loopback-mode=exclude-process-tree loopback-target-pid=<us>` when this GStreamer's
/// wasapi2src has process loopback, else "". Probed by instantiating the element
/// (cheap — nothing is opened until READY) and checking for the two properties.
fn wasapi2_process_loopback_fragment() -> String {
    if !cfg!(windows) {
        return String::new();
    }
    let _ = crate::ensure_init();
    let supported = gst::ElementFactory::make("wasapi2src")
        .build()
        .map(|e| e.find_property("loopback-mode").is_some() && e.find_property("loopback-target-pid").is_some())
        .unwrap_or(false);
    if supported {
        let pid = std::process::id();
        eprintln!("[aux/music] wasapi2src has process loopback -> sharing everything except Tin Hat (pid {pid})");
        format!("loopback-mode=exclude-process-tree loopback-target-pid={pid}")
    } else {
        eprintln!("[aux/music] wasapi2src has no process loopback -> endpoint loopback (our own call audio may echo)");
        String::new()
    }
}

/// Playback sink tail — ALWAYS named `out` so `repoint_playback_sinks` finds it.
/// Empty target = the default render endpoint.
///
/// Windows runs `sync=true`: wasapi2sink is an audiobasesink, and with sync on its
/// built-in clock slaving (`slave-method=skew` default) absorbs the producer/consumer
/// crystal drift the r139 note describes by *resampling smoothly* — with sync=false
/// the same drift can only correct as hard sample splices in the leaky queues, which
/// is the audible END-OF-SPEECH POP found in the first real two-way bare-metal call
/// (masked while speech plays, exposed in the silence right after). Linux keeps
/// sync=false byte-identical: PipeWire's own ring does the rate matching there.
fn audio_sink_fragment(target: &str) -> String {
    audio_sink_fragment_for(target, pulse_head_wanted())
}

/// 10.4.1: the same fragment with the audio API chosen by the caller (the playback healer
/// alternates APIs); Windows has one (WASAPI) and ignores `use_pulse`.
fn audio_sink_fragment_for(target: &str, use_pulse: bool) -> String {
    if cfg!(windows) {
        if target.is_empty() {
            "wasapi2sink sync=true name=out".to_string()
        } else {
            format!("wasapi2sink sync=true name=out device=\"{target}\"")
        }
    } else if use_pulse {
        // 10.0.5 (field report): inside the Flatpak the runtime's pipewiresink (1.4.9) connected and
        // played NOTHING on a user's desktop — the mirror of the pipewiresrc failure — while the
        // PulseAudio-compat sink plays (the Flatpak test machine, inside the sandbox). Same node names; a small ring
        // buffer keeps it near the native path's latency. `device` is the re-point property here.
        // 10.5.2: tagged, so the echo reference can follow where this stream really plays.
        let tag = stream_tag(ROLE_PLAYBACK);
        if target.is_empty() {
            format!("pulsesink sync=false name=out buffer-time=50000 latency-time=10000 stream-properties=\"props,{PA_NO_RESTORE},{tag}\"")
        } else {
            format!("pulsesink sync=false name=out buffer-time=50000 latency-time=10000 device=\"{target}\" stream-properties=\"props,{PA_NO_RESTORE},{tag}\"")
        }
    } else if target.is_empty() {
        format!("pipewiresink sync=false name=out stream-properties=\"props,{PW_NO_RESTORE},{}\"", stream_tag(ROLE_PLAYBACK))
    } else {
        format!("pipewiresink sync=false name=out target-object=\"{target}\" stream-properties=\"props,{PW_NO_RESTORE},{}\"", stream_tag(ROLE_PLAYBACK))
    }
}

/// The sink property a live output re-point writes (NULL-cycle + sync_state).
fn sink_target_prop() -> &'static str {
    if cfg!(windows) || pulse_head_wanted() { "device" } else { "target-object" }
}

/// 10.5 (audio review #4): the re-point property of THIS sink. Since 10.4.1 the playback healer can
/// move one pipeline to the other audio API, so the global preference no longer says what a given
/// sink is — pulsesink/wasapi2sink take `device`, pipewiresink `target-object`, and glib panics on
/// a property the element does not have (on a detached thread: a half-applied switch, not a crash).
fn sink_target_prop_of(el: &gst::Element) -> &'static str {
    match el.factory().map(|f| f.name().to_string()).as_deref() {
        Some("pipewiresink") => "target-object",
        Some(_) => "device",
        None => sink_target_prop(),
    }
}
fn set_sink_target(el: &gst::Element, target: &str) {
    let prop = sink_target_prop_of(el);
    if el.find_property(prop).is_none() {
        eprintln!("[call] output re-point: {} has no `{prop}` — left where it is", el.name());
        return;
    }
    if target.is_empty() {
        el.set_property(prop, None::<&str>);
    } else {
        el.set_property(prop, target);
    }
}

/// 10.0.6: a state change that can never wedge its caller. GStreamer runs the transition on the
/// calling thread, and a PipeWire / PulseAudio element whose stream the server never starts (or
/// never acknowledges a disconnect for) sits inside it for up to 30 s — the hang-up hang of
/// 10.0.4 and the "app freezes around audio faults" field reports. The transition runs on a detached
/// thread; the caller waits at most `timeout_ms`, then carries on and lets the element finish (or
/// not) in the background. Returns whether the transition completed successfully in time.
pub(crate) fn set_state_bounded(el: &gst::Element, state: gst::State, timeout_ms: u64, what: &str) -> bool {
    let el = el.clone();
    let (tx, rx) = std::sync::mpsc::channel::<bool>();
    std::thread::spawn(move || {
        let ok = el.set_state(state).is_ok();
        let _ = tx.send(ok);
    });
    match rx.recv_timeout(std::time::Duration::from_millis(timeout_ms)) {
        Ok(ok) => ok,
        Err(_) => {
            eprintln!(
                "[gst] {what}: the {state:?} transition is taking more than {timeout_ms} ms — \
                 leaving it to finish in the background"
            );
            false
        }
    }
}

fn repoint_playback_sinks(target: String) -> std::thread::JoinHandle<bool> {
    static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let snapshot: Vec<glib::WeakRef<gst::Element>> = playback_sinks().lock().unwrap().clone();
    let generation = GENERATION.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    std::thread::spawn(move || {
        let mut live = 0usize;
        for w in snapshot {
            if GENERATION.load(std::sync::atomic::Ordering::SeqCst) != generation {
                eprintln!("[call] output re-point superseded by a newer switch — abandoning");
                return false;
            }
            let Some(sink) = w.upgrade() else { continue };
            set_state_bounded(&sink, gst::State::Null, 1500, "output re-point");
            // 10.5: the NULL can take seconds — a newer switch that started meanwhile owns the
            // target now; bring this sink back up as it was and let that walk move it.
            if GENERATION.load(std::sync::atomic::Ordering::SeqCst) != generation {
                let _ = sink.sync_state_with_parent();
                eprintln!("[call] output re-point superseded while a sink was stopping — abandoning");
                return false;
            }
            set_sink_target(&sink, &target);
            let _ = sink.sync_state_with_parent();
            live += 1;
        }
        eprintln!("[call] output re-pointed to {:?} ({} live sink(s))", target, live);
        true
    })
}

/// The PipeWire source (node name) our mic captures from. `None` = system default.
/// File-backed (`~/.config/tinhat/call-input`, mirroring `music_source`) so the chosen
/// MIC is remembered across launches; a stale/removed source is dropped to None (default).
/// The virtual echo-cancelled nodes in use for THIS call, if AEC is active: (source, sink).
/// Set on join, cleared on close. While set, the mic captures from the cancelled source and
/// remote audio plays into the reference sink (which forwards to the real speakers).
fn aec_nodes() -> &'static Mutex<Option<(String, String)>> {
    static A: OnceLock<Mutex<Option<(String, String)>>> = OnceLock::new();
    A.get_or_init(|| Mutex::new(None))
}
/// Bring echo cancellation up for this call, bound to the user's CURRENT devices + AGC
/// preference. Idempotent: an unchanged binding reuses the running module. Returns true
/// when the EC nodes were just (re)created OR just went away — i.e. whenever live playback
/// sinks pointed at the previous node (by name) are now attached to a dead node and must
/// be re-pointed (NULL-cycled) by the caller.
#[cfg(not(windows))]
fn aec_start() -> bool {
    if aec_tier() == AecTier::Dsp {
        return dsp_aec_start();
    }
    module_aec_start()
}

/// Legacy Linux tier: PipeWire module-echo-cancel (see [`AecTier::Module`]).
#[cfg(not(windows))]
fn module_aec_start() -> bool {
    let mic = mic_target().lock().unwrap().clone();
    let out = playback_target().lock().unwrap().clone();
    let prev = aec_nodes().lock().unwrap().clone();
    match crate::aec::ensure_loaded(mic.as_deref(), out.as_deref(), mic_agc()) {
        Some((src, sink, fresh)) => {
            *aec_nodes().lock().unwrap() = Some((src, sink));
            aec_degraded_flag().store(false, std::sync::atomic::Ordering::Relaxed);
            // `|| prev.is_none()`: a reused module does NOT imply the sinks are still on
            // it. If a previous aec_start returned None mid-call (transient pactl
            // failure, monitor-source interlude), the caller re-pointed every sink to
            // the RAW device; when the binding comes back the module is "reused" but
            // the sinks are still on raw — without this, echo cancellation silently
            // ran with an empty reference for the rest of the call. At join prev is
            // None too, where the extra repoint is a no-op (no live sinks yet).
            fresh || prev.is_none()
        }
        None => {
            // EC could not be brought up. If it HAD been up, the caller is about to move
            // playback back onto the raw device — correct (the EC sink is gone), but it
            // means this call now has no echo cancellation at all while the toggle still
            // reads ON. That combination is what "echo cancel broken" looks like from the
            // other side of the call, and until now it happened without a single line in
            // the log. Record it so the app can say so and so a retry can fix it.
            *aec_nodes().lock().unwrap() = None;
            if prev.is_some() {
                eprintln!(
                    "[aec] WARNING: echo cancellation DIED mid-call — playback returns to the raw \
                     device and peers will hear your echo until it comes back"
                );
            }
            aec_degraded_flag().store(true, std::sync::atomic::Ordering::Relaxed);
            prev.is_some()
        }
    }
}

/// True when the user WANTS echo cancellation but the module is not up — i.e. the toggle
/// reads ON and there is no canceller. Distinct from "turned off": the user cannot see the
/// difference, but everyone else in the call can.
fn aec_degraded_flag() -> &'static std::sync::atomic::AtomicBool {
    static F: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    &F
}

/// Is echo cancellation wanted but not running? (Surfaced by the app so a dead canceller
/// stops being invisible to the one person who cannot hear it.)
pub fn aec_degraded() -> bool {
    aec_degraded_flag().load(std::sync::atomic::Ordering::Relaxed)
}

/// Retry a canceller that died mid-call. Cheap and idempotent: a no-op when AEC is off,
/// already up, or unsupported. Returns true when it came back (and the caller must
/// re-point playback onto the restored reference sink).
///
/// Without this, one transient `pactl` failure cost echo cancellation for the REST of the
/// call — nothing re-attempted it, because apply_aec only runs on a device or AGC change.
#[cfg(not(windows))]
pub fn aec_retry_if_degraded() -> bool {
    if aec_tier() == AecTier::Dsp {
        return dsp_aec_retry();
    }
    if !crate::aec::enabled() || aec_nodes().lock().unwrap().is_some() {
        return false;
    }
    if !aec_degraded_flag().load(std::sync::atomic::Ordering::Relaxed) {
        return false;
    }
    if aec_start() {
        eprintln!("[aec] echo cancellation recovered — re-pointing playback onto the reference sink");
        let _ = repoint_playback_sinks(effective_playback_node().unwrap_or_default());
        return true;
    }
    false
}

#[cfg(windows)]
pub fn aec_retry_if_degraded() -> bool {
    dsp_aec_retry()
}

/// Dsp-tier retry. 0.8.31: the reference is (re)built WITH the mic, so there is nothing to
/// bring back out of band — re-evaluate the binding (a headset unplugged, an output that
/// came back) and leave the rebuild to the healer's one-shot retry (take_aec_fallback).
fn dsp_aec_retry() -> bool {
    if !crate::aec::enabled() || !aec_degraded_flag().load(std::sync::atomic::Ordering::Relaxed) {
        return false;
    }
    dsp_aec_ensure();
    false
}
/// Tear echo cancellation down (leave/close). Idempotent; leaves the user's audio untouched.
#[cfg(not(windows))]
fn aec_stop() {
    if aec_tier() == AecTier::Dsp {
        dsp_aec_stop();
        return;
    }
    *aec_nodes().lock().unwrap() = None;
    // Stopping on purpose is not degradation — otherwise leaving a call would leave the
    // warning latched and the next call would report a problem it does not have.
    aec_degraded_flag().store(false, std::sync::atomic::Ordering::Relaxed);
    crate::aec::unload();
}

// ── Windows echo cancellation: webrtcdsp + a WASAPI loopback reference ──────
//
// Same philosophy as aec.rs ("cancellation has to happen where the loudspeaker
// signal and the mic signal actually meet: at the device layer"), different
// mechanism: PipeWire's module-echo-cancel doesn't exist here, so the WebRTC
// audio processor runs IN the mic pipeline (`webrtcdsp`, gst-plugins-bad — the
// same AEC3 code the PipeWire module wraps) and its far-end reference is a second
// branch OF THE SAME PIPELINE capturing WHAT THE SPEAKERS ARE ACTUALLY PLAYING via
// WASAPI loopback of the output endpoint (PipeWire: the sink's monitor), ending in
// `webrtcechoprobe` (paired with the dsp by element name — 0.8.31: the probe must
// live in the SAME top-level pipeline, it aligns by per-pipeline running time; a
// separate pipeline measured 0.0 dB). Because the reference is the ENDPOINT, it
// covers every per-track playback pipeline, music share, soundboard — anything
// the app (or the user) plays out those speakers.
//
// Toggle semantics mirror Linux exactly: aec::enabled() (persisted, default ON)
// gates it; AGC rides the same mic_agc() toggle (webrtcdsp gain-control); noise
// suppression stays RNNoise (mic_denoise) — webrtcdsp NS is hard-off; a
// monitor/loopback INPUT skips AEC ("not a microphone in a room"), and so does an
// OUTPUT PipeWire reports as headphones/headset (nothing to cancel). The dsp and
// its reference branch are inserted by build_mic_pipeline only while echo
// cancellation is engaged; a machine where the reference can't start steps down
// one ladder rung (stage without the probe) instead of losing the mic.
/// Which echo-cancellation engine this process uses.
///
/// * `Dsp` — the in-app canceller: `webrtcdsp` (WebRTC AEC3 + NS + AGC, gst-plugins-bad)
///   runs INSIDE the mic capture chain and its far-end reference is a small loopback
///   pipeline of the output device (Windows: WASAPI loopback; Linux: the sink's PipeWire
///   monitor via `stream.capture.sink`). Both platforms, one code path, no virtual
///   devices, no module reloads, no playback re-pointing. 0.8.25 default everywhere.
/// * `Module` — the pre-0.8.25 Linux path: PipeWire's `module-echo-cancel` loaded via
///   pactl (aec.rs), the mic reading its virtual source and playback re-pointed into its
///   virtual sink. Kept as the fallback for a GStreamer without webrtcdsp, and reachable
///   with `TINHAT_AEC_TIER=module` for A/B tests. Field evidence for retiring it as the
///   default: the virtual source delivered 40-60 % of realtime for seconds after every
///   (re)load (a field log: a large rate_add repair burst within seconds, many rebuilds in
///   one call) and every device/AGC change reloaded it, kicking live playback off the
///   reference sink (WirePlumber re-homes it) — the "robot voice" reports.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AecTier {
    Dsp,
    Module,
}

/// Decide the tier once per process (logged). `TINHAT_AEC_TIER=dsp|module` overrides.
pub fn aec_tier() -> AecTier {
    static T: OnceLock<AecTier> = OnceLock::new();
    *T.get_or_init(|| {
        if cfg!(windows) {
            return AecTier::Dsp;
        }
        let _ = crate::ensure_init();
        let have_dsp = gst::ElementFactory::find("webrtcdsp").is_some()
            && gst::ElementFactory::find("webrtcechoprobe").is_some();
        let want = std::env::var("TINHAT_AEC_TIER").unwrap_or_default();
        let tier = match want.as_str() {
            "module" => AecTier::Module,
            "dsp" => AecTier::Dsp,
            _ if have_dsp => AecTier::Dsp,
            _ => AecTier::Module,
        };
        eprintln!(
            "[aec] engine: {} (webrtcdsp {}{})",
            match tier { AecTier::Dsp => "in-app WebRTC AEC3 (webrtcdsp)", AecTier::Module => "PipeWire module-echo-cancel" },
            if have_dsp { "available" } else { "MISSING — install gst-plugins-bad" },
            if want.is_empty() { "" } else { ", TINHAT_AEC_TIER override" }
        );
        tier
    })
}

/// Short engine name for the settings pane / logs.
pub fn aec_tier_name() -> &'static str {
    match aec_tier() {
        AecTier::Dsp => "webrtc-dsp",
        AecTier::Module => "pipewire-module",
    }
}

/// Head of the far-end reference tap: what the speakers are actually playing.
/// Windows: WASAPI loopback of the render endpoint. Linux: the PipeWire sink's monitor
/// (`stream.capture.sink=true` on a capture stream targeting the sink node; no target =
/// the default sink). Endpoint/sink level on purpose — it covers every per-track playback
/// pipeline, music share, soundboard AND the user's game audio, so all of it is cancelled.
fn dsp_tap_head(out: &str, name: &str) -> String {
    tap_head(out, name, ROLE_REFERENCE)
}
fn tap_head(out: &str, name: &str, role: &str) -> String {
    if cfg!(windows) {
        if out.is_empty() {
            format!("wasapi2src name={name} loopback=true")
        } else {
            format!("wasapi2src name={name} loopback=true device=\"{out}\"")
        }
    } else {
        // A chosen output that has gone away: PipeWire silently re-targets the capture to
        // the DEFAULT sink — the same fallback the playback pipelines get — so the reference
        // keeps matching what the speakers play. The harness (TINHAT_AEC_TEST_NOFALLBACK=1)
        // forbids that, so a missing output is a FAILED rung there, not a quiet re-target.
        let extra = if std::env::var("TINHAT_AEC_TEST_NOFALLBACK").map(|v| v == "1").unwrap_or(false) {
            ",node.dont-fallback=(boolean)true"
        } else {
            ""
        };
        // Bench knob (0.8.33): TINHAT_AEC_REF_NOCLOCK=1 keeps the reference heads from providing the
        // pipeline clock (the mic head already does not, 0.8.29) — with several outputs the clock
        // could come from an idle output's stream, an unrelated PipeWire driver.
        let clk = if std::env::var("TINHAT_AEC_REF_NOCLOCK").map(|v| v == "1").unwrap_or(false) { " provide-clock=false" } else { "" };
        // 10.0.4: the PulseAudio-compat reference — the sink's monitor source (default sink
        // resolved through pactl); a sink we cannot name falls through to the native head.
        if pulse_head_wanted() {
            let sink = if out.is_empty() { default_audio_sink().unwrap_or_default() } else { out.to_string() };
            if !sink.is_empty() {
                return format!(
                    "pulsesrc name={name} device=\"{sink}.monitor\" stream-properties=\"props,{NO_RESTORE_TYPED},{}\"",
                    stream_tag(role)
                );
            }
        }
        // 10.5.2: typed no-restore + the tag (see NO_RESTORE_TYPED / stream_tag).
        let tag = stream_tag(role);
        if out.is_empty() {
            format!("pipewiresrc name={name}{clk} stream-properties=\"props,stream.capture.sink=(boolean)true,{NO_RESTORE_TYPED},{tag}{extra}\"")
        } else {
            format!(
                "pipewiresrc name={name}{clk} target-object=\"{out}\" stream-properties=\"props,stream.capture.sink=(boolean)true,{NO_RESTORE_TYPED},{tag}{extra}\""
            )
        }
    }
}

/// 10.5: the same monitor head for the "can they hear me?" system-audio sniffer (hear.rs).
pub(crate) fn monitor_head(out: &str, name: &str) -> String {
    tap_head(out, name, ROLE_SNIFFER)
}

/// 10.5: what hear.rs needs to know about echo cancellation to recommend the right fix.
pub(crate) struct EcFacts {
    pub enabled: bool,
    pub dsp_tier: bool,
    /// The reference branch is in the running mic chain.
    pub engaged: bool,
    pub degraded: bool,
    /// Why it is deliberately off ("" = no skip) — the headset rule.
    pub skip: String,
    /// The outputs the reference covers ("" = the default output).
    pub ref_outputs: Vec<String>,
    pub default_sink: String,
    pub default_sink_label: String,
    /// The output the call plays to, resolved ("" only when nothing could be resolved).
    pub call_output_resolved: String,
    pub out_label: String,
    pub mic_label: String,
}
/// Snapshot of [`EcFacts`] — shells out to pactl (device labels cached 30 s); call off the async threads.
pub(crate) fn ec_facts() -> EcFacts {
    let enabled = crate::aec::enabled();
    let dsp_tier = aec_tier() == AecTier::Dsp;
    let engaged = dsp_tier
        && dsp_aec_active()
        && mic_chain_flags().load(std::sync::atomic::Ordering::Relaxed) & 2 != 0;
    let ref_outputs = if dsp_aec_active() { ec_binding_outputs() } else { Vec::new() };
    let default_sink = default_audio_sink().unwrap_or_default();
    let call_sel = playback_target().lock().unwrap().clone(); // guard released here, before any pactl
    let call_out = call_sel.unwrap_or_default();
    // 10.5.2: where the call really plays (the healer's last look), before what the app picked.
    let observed = recent_playback_sinks(std::time::Duration::from_secs(10)).into_iter().next();
    let call_output_resolved = match observed {
        Some(o) => o,
        None if call_out.is_empty() => default_sink.clone(),
        None => call_out,
    };
    static LABELS: OnceLock<Mutex<Option<(std::time::Instant, Vec<(String, String)>, Vec<(String, String)>)>>> = OnceLock::new();
    let cell = LABELS.get_or_init(|| Mutex::new(None));
    let (sinks, sources) = {
        let fresh = cell.lock().unwrap().as_ref().map(|(t, _, _)| t.elapsed() < std::time::Duration::from_secs(30)).unwrap_or(false);
        if !fresh {
            let v = (list_devices("Audio/Sink"), list_devices("Audio/Source"));
            *cell.lock().unwrap() = Some((std::time::Instant::now(), v.0, v.1));
        }
        let g = cell.lock().unwrap();
        let (_, a, b) = g.as_ref().unwrap();
        (a.clone(), b.clone())
    };
    let label = |list: &[(String, String)], n: &str| {
        list.iter().find(|(x, _)| x == n).map(|(_, d)| d.clone()).unwrap_or_else(|| n.to_string())
    };
    let mic_sel = mic_target().lock().unwrap().clone(); // (Astra #6: the guard used to live through the pactl call)
    let mic = mic_sel.unwrap_or_else(|| default_audio_source().unwrap_or_default());
    EcFacts {
        enabled,
        dsp_tier,
        engaged,
        degraded: aec_degraded(),
        skip: dsp_skip_reason().lock().unwrap().clone(),
        ref_outputs,
        default_sink_label: label(&sinks, &default_sink),
        default_sink,
        out_label: label(&sinks, &call_output_resolved),
        call_output_resolved,
        mic_label: label(&sources, &mic),
    }
}

/// 10.5: the speaker-output list with `node` added (the echo pill's "Add … to echo cancellation").
/// An empty list meant "the output the call plays to" — that output is kept, by name, so adding a
/// second one never drops the first from the reference.
pub fn aec_ref_outputs_with(node: &str) -> Vec<String> {
    let mut list = crate::aec::ref_outputs();
    if list.is_empty() {
        // 10.5.2: every output the call really plays on (the healer's last look), else the pick/default.
        list = recent_playback_sinks(std::time::Duration::from_secs(10));
    }
    if list.is_empty() {
        let call_sel = playback_target().lock().unwrap().clone(); // released before the pactl lookup
        let call_out = call_sel.or_else(default_audio_sink).unwrap_or_default();
        if !call_out.is_empty() {
            list.push(call_out);
        }
    }
    if !node.is_empty() && !list.iter().any(|o| o == node) {
        list.push(node.to_string());
    }
    list
}

/// 0.8.31: the echo-cancellation REFERENCE lives inside the mic pipeline.
///
/// Up to 0.8.30 the far-end reference (`webrtcechoprobe`) ran in its own top-level pipeline
/// and the mic's `webrtcdsp` paired with it by name. Upstream forbids exactly that ("the probe
/// can only be used within the same top level GstPipeline"): the probe aligns reference and
/// capture by comparing RUNNING TIME, which is per pipeline, so ours compared two unrelated
/// clocks — measured on the test rig against our exact caps and property set: 0.0 dB of echo
/// removed, output bit-identical to echo-cancel=false, since the day it shipped. Same
/// pipeline: 30–40 dB. So the reference chain is now a second branch of the SAME
/// `gst::Pipeline` the mic runs in (one parse::launch, two chains, one clock, one base
/// time), rebuilt with the mic on every rebuild, its probe named uniquely per build (no
/// cross-pipeline pairing, no exclusive name to fight over on a rebuild — the 0.8.28 trap).
///
/// This cell only records WHICH output the reference is bound to: `Some(node)` = engaged
/// ("" = the default output), `None` = off or skipped (see [`dsp_skip_reason`]).
fn dsp_tap() -> &'static Mutex<Option<String>> {
    static T: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    T.get_or_init(|| Mutex::new(None))
}
/// Why echo cancellation is deliberately NOT engaged right now ("" = no skip in force).
fn dsp_skip_reason() -> &'static Mutex<String> {
    static R: OnceLock<Mutex<String>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(String::new()))
}
/// Is echo cancellation engaged? (=> build_mic_pipeline inserts webrtcdsp + the reference branch.)
fn dsp_aec_active() -> bool {
    dsp_tap().lock().unwrap().is_some()
}
/// Per-build sequence for the probe's element name: `tinhatecprobe<n>`. A rebuild's new chain
/// must never pair with (or be refused by) the retiring chain's probe.
fn ec_probe_seq() -> &'static std::sync::atomic::AtomicU64 {
    static S: OnceLock<std::sync::atomic::AtomicU64> = OnceLock::new();
    S.get_or_init(|| std::sync::atomic::AtomicU64::new(0))
}
fn ec_probe_name() -> String {
    format!("tinhatecprobe{}", ec_probe_seq().load(std::sync::atomic::Ordering::Relaxed))
}
/// The reference branch: what the speakers are playing → mono 48 k F32 → the probe. Lives in
/// the mic pipeline (see [`dsp_tap`]). CAPS TRAP: the webrtc elements take F32LE only as
/// layout=NON-interleaved (interleaved must be S16LE); for mono the bytes are identical, but
/// the caps string must say so. fakesink because the data is REFERENCE ONLY — real playback
/// keeps flowing through the per-track pipelines untouched; async=false so this branch never
/// holds the mic's state change; the queue never leaks (a dropped reference buffer is a hole
/// the canceller aligns against nothing).
/// Probe name for reference branch `i` of this build: the main one (`tinhatecprobe<n>`) for the
/// first output, `tinhatecprobe<n>x<i>` for every further output (0.8.33).
fn ec_probe_name_n(i: usize) -> String {
    if i == 0 { ec_probe_name() } else { format!("{}x{i}", ec_probe_name()) }
}
/// The outputs the current reference binding covers ([""] = the one default output).
fn ec_binding_outputs() -> Vec<String> {
    let binding = dsp_tap().lock().unwrap().clone().unwrap_or_default();
    binding.split('\n').map(|s| s.to_string()).collect()
}
fn ec_reference_chain(outs: &[String]) -> String {
    const Q: &str = "queue max-size-time=200000000 max-size-buffers=0 max-size-bytes=0 leaky=no";
    const MONO: &str = "audio/x-raw,format=F32LE,rate=48000,channels=1,layout=non-interleaved";
    // 0.8.33: SEVERAL outputs → one reference branch EACH, feeding its own probe and its own
    // echo-only stage in the mic chain (aec_dsp_fragment_with), in series. NOT a mixer: a live
    // audiomixer hands the summed reference over later than the echo reaches the mic (its
    // latency + output buffer; benched on the test rig: 0.0 dB), and AEC3 has to see the render
    // before the capture. Per-output branches keep exactly the single-output timing (54–63 dB).
    let mut s = String::new();
    for (i, out) in outs.iter().enumerate() {
        let (src, q, sink) = if i == 0 {
            ("tinhatecref".to_string(), "tinhatecq".to_string(), "tinhatecsink".to_string())
        } else {
            (format!("tinhatecref{i}"), format!("tinhatecq{i}"), format!("tinhatecsink{i}"))
        };
        if i > 0 {
            s.push(' ');
        }
        s.push_str(&format!(
            "{} ! {Q} name={q} ! audioconvert ! audioresample ! {MONO} ! \
             webrtcechoprobe name={} ! fakesink name={sink} sync=false async=false",
            dsp_tap_head(out, &src),
            ec_probe_name_n(i)
        ));
    }
    s
}
/// Prefix a mic description that carries the probe with its reference branch.
fn ec_with_reference(desc: String) -> String {
    if !desc.contains("probe=") {
        return desc;
    }
    format!("{} {desc}", ec_reference_chain(&ec_binding_outputs()))
}
/// Does PipeWire report the output as headphones / a headset? `Some(reason)` when it does.
/// Only the device's own `device.form-factor` counts (udev / ALSA UCM set it from the USB
/// descriptors of headsets); port names and descriptions are NOT used — desktop speakers on
/// a "headphones" jack are common, and a false skip here means echo for everyone else. Cached
/// per output for 30 s (a pactl round-trip per mic build is fine, per tick is not).
/// `TINHAT_AEC_HEADSET=1|0` overrides (gates + bench).
fn output_headset_reason(out: &str) -> Option<String> {
    if let Ok(v) = std::env::var("TINHAT_AEC_HEADSET") {
        return if v == "1" { Some("TINHAT_AEC_HEADSET override".to_string()) } else { None };
    }
    if cfg!(windows) {
        return None; // WASAPI form factors are not visible through GStreamer — never guess
    }
    static CACHE: OnceLock<Mutex<Option<(String, std::time::Instant, Option<String>)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(None));
    if let Some((o, at, r)) = cache.lock().unwrap().as_ref() {
        if o == out && at.elapsed() < std::time::Duration::from_secs(30) {
            return r.clone();
        }
    }
    let name = if out.is_empty() { default_audio_sink().unwrap_or_default() } else { out.to_string() };
    let listing = pactl_text(&["list", "sinks"]).unwrap_or_default(); // 10.5.2: bounded, C locale
    let mut reason = None;
    if !name.is_empty() {
        for blk in listing.split("\nSink #") {
            let is_ours = blk.lines().any(|l| l.trim() == format!("Name: {name}"));
            if !is_ours {
                continue;
            }
            let desc = blk
                .lines()
                .find_map(|l| l.trim().strip_prefix("Description: "))
                .unwrap_or(name.as_str())
                .to_string();
            for l in blk.lines() {
                let t = l.trim();
                let ff = t
                    .strip_prefix("device.form_factor = \"")
                    .or_else(|| t.strip_prefix("device.form-factor = \""))
                    .map(|v| v.trim_end_matches('"').to_ascii_lowercase());
                if let Some(ff) = ff {
                    if matches!(ff.as_str(), "headset" | "headphone" | "headphones" | "hands-free" | "handset") {
                        reason = Some(format!("{desc} reports as {ff} (device.form-factor)"));
                    }
                }
            }
            break;
        }
    }
    *cache.lock().unwrap() = Some((out.to_string(), std::time::Instant::now(), reason.clone()));
    reason
}
// ── 10.5.2: the automatic echo reference listens where the call ACTUALLY plays ──────────────
//
// Field report: echo cancellation didn't quite work unless the output was selected
// explicitly. Up to 10.5.1 the automatic reference was bound to what the APP believed the output
// was — its own pick, else "the default output" — while the audio server decides where an
// unpinned playback stream really goes: WirePlumber re-applies a route the user once gave "Tin Hat"
// in the desktop's volume applet / pavucontrol (restore-stream; our no-restore markers never
// reached it, see NO_RESTORE_TYPED), a stream moved there mid-call stays moved, a default change
// moves every unpinned stream, and the headset skip was decided once, against the default at join.
// In every one of those the canceller subtracted a device the call was not playing on: no
// cancellation, while the toggle read ON. Picking the output (or ticking it under Speaker
// outputs) pinned both ends to the same device, which is why that "fixed" it.
//
// Now: every call playback sink is tagged (stream_tag), the audio server is asked where those
// streams are (pactl — the one tool that also works inside the Flatpak and sees native PipeWire
// streams too), and the automatic reference is bound BY NAME to exactly those sinks (the resolved
// default before anything plays). The call-lifetime healer re-checks every few seconds
// ([`ec_rebind_wanted`]) and rebinds when the call moved or the reference drifted.

/// Where this process's tagged streams are, as the audio server sees them.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct CallRouting {
    /// sinks the call's playback streams feed (order of appearance, deduplicated)
    pub playback: Vec<String>,
    /// sources the echo-reference heads read ("<sink>.monitor"); None = that query failed (unknown,
    /// never read as "no references")
    pub reference: Option<Vec<String>>,
}

/// Pure: the device indices of the `kind` blocks ("Sink Input" / "Source Output") of `pactl list
/// sink-inputs|source-outputs` whose properties carry `tinhat.owner = owner` and `tinhat.role = role`;
/// `dev_key` is the line naming the device ("Sink" / "Source"). Order of appearance, deduplicated.
pub(crate) fn parse_tagged_streams(text: &str, kind: &str, dev_key: &str, owner: &str, role: &str) -> Vec<u32> {
    let head = format!("{kind} #");
    let dev_prefix = format!("{dev_key}: ");
    let want_owner = format!("tinhat.owner = \"{owner}\"");
    let want_role = format!("tinhat.role = \"{role}\"");
    fn flush(dev: Option<u32>, o: bool, r: bool, out: &mut Vec<u32>) {
        if let (Some(d), true, true) = (dev, o, r) {
            if !out.contains(&d) {
                out.push(d);
            }
        }
    }
    let mut out: Vec<u32> = Vec::new();
    let (mut dev, mut o, mut r, mut inside) = (None::<u32>, false, false, false);
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with(&head) {
            if inside {
                flush(dev, o, r, &mut out);
            }
            (dev, o, r, inside) = (None, false, false, true);
            continue;
        }
        if !inside {
            continue;
        }
        if let Some(v) = t.strip_prefix(&dev_prefix) {
            dev = v.trim().parse::<u32>().ok();
        } else if t == want_owner {
            o = true;
        } else if t == want_role {
            r = true;
        }
    }
    if inside {
        flush(dev, o, r, &mut out);
    }
    out
}

/// Pure: `pactl list short sinks|sources` → index → name.
pub(crate) fn parse_short_list(text: &str) -> HashMap<u32, String> {
    let mut m = HashMap::new();
    for line in text.lines() {
        let mut it = line.split('\t');
        if let (Some(i), Some(n)) = (it.next(), it.next()) {
            if let Ok(i) = i.trim().parse::<u32>() {
                m.insert(i, n.trim().to_string());
            }
        }
    }
    m
}

/// `pactl <args>` in the C locale (pactl translates its block headers — "Sink Input #", "Name:" — so a
/// German desktop would read as "nothing"), bounded: a stalled audio server must never hold the call
/// controls (apply_aec runs under the call mutex) — after PACTL_DEADLINE the child is killed and the
/// answer is "unknown" (None). The output is drained on a thread so a long listing can't fill the pipe.
const PACTL_DEADLINE: std::time::Duration = std::time::Duration::from_millis(2000);
pub(crate) fn pactl_text(args: &[&str]) -> Option<String> {
    pactl_bounded(args, true)
}
/// The bounded runner behind `pactl_text`. `c_locale: false` keeps the caller's locale (pactl_devices,
/// whose Description labels are shown to the user) — 10.5.2 (Astra release review): that listing runs
/// under the call mutex too (apply_aec, ticked outputs) and must not hang on a stuck sound server either.
fn pactl_bounded(args: &[&str], c_locale: bool) -> Option<String> {
    use std::io::Read;
    let mut cmd = std::process::Command::new("pactl");
    cmd.args(args);
    if c_locale {
        cmd.env("LC_ALL", "C");
    }
    let mut child = cmd
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let mut out = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = out.read_to_end(&mut v);
        v
    });
    let t0 = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(st)) => {
                let bytes = reader.join().unwrap_or_default();
                return if st.success() { Some(String::from_utf8_lossy(&bytes).to_string()) } else { None };
            }
            Ok(None) if t0.elapsed() < PACTL_DEADLINE => std::thread::sleep(std::time::Duration::from_millis(5)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                eprintln!("[audio] pactl {} did not answer within {} ms — treated as unknown", args.join(" "), PACTL_DEADLINE.as_millis());
                return None;
            }
        }
    }
}

/// The last routing observation (the healer refreshes it every few seconds in a call) — for the
/// "can they hear me?" facts and the echo pill, which must not shell out on their own tick.
fn last_routing() -> &'static Mutex<Option<(std::time::Instant, CallRouting)>> {
    static L: OnceLock<Mutex<Option<(std::time::Instant, CallRouting)>>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(None))
}
/// Playback sinks from an observation at most `max_age` old (empty = none / unknown).
fn recent_playback_sinks(max_age: std::time::Duration) -> Vec<String> {
    match last_routing().lock().unwrap().as_ref() {
        Some((at, r)) if at.elapsed() <= max_age => r.playback.clone(),
        _ => Vec::new(),
    }
}

/// Ask the audio server where this process's call playback and echo-reference streams are.
/// Linux only (Windows: None — WASAPI has no per-app routing we could read back).
pub(crate) fn observe_call_routing() -> Option<CallRouting> {
    if cfg!(windows) {
        return None;
    }
    let owner = tinhat_owner();
    let inputs = pactl_text(&["list", "sink-inputs"])?;
    let sinks = parse_short_list(&pactl_text(&["list", "short", "sinks"])?);
    let playback: Vec<String> = parse_tagged_streams(&inputs, "Sink Input", "Sink", owner, ROLE_PLAYBACK)
        .into_iter()
        .filter_map(|i| sinks.get(&i).cloned())
        .collect();
    let reference: Option<Vec<String>> = pactl_text(&["list", "source-outputs"]).and_then(|outputs| {
        let refs = parse_tagged_streams(&outputs, "Source Output", "Source", owner, ROLE_REFERENCE);
        if refs.is_empty() {
            return Some(Vec::new());
        }
        let sources = parse_short_list(&pactl_text(&["list", "short", "sources"])?);
        Some(refs.into_iter().filter_map(|i| sources.get(&i).cloned()).collect())
    });
    let r = CallRouting { playback, reference };
    *last_routing().lock().unwrap() = Some((std::time::Instant::now(), r.clone()));
    Some(r)
}

/// What the echo reference should listen to.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum EcPlan {
    /// bind the reference to these outputs ("" = the unnamed default); `explicit` = the user's own
    /// Speaker outputs list; `why` = where the outputs came from, for the log
    Engage { outs: Vec<String>, explicit: bool, why: &'static str },
    /// headphones only — nothing to cancel
    Skip(String),
}
/// Most reference branches one automatic binding carries (each is a monitor capture + a stage).
const EC_MAX_AUTO_OUTPUTS: usize = 4;
const EC_WHY_PICKED: &str = "the speaker outputs you picked";
const EC_WHY_PICKED_PLUS_CALL: &str = "the speaker outputs you picked + where the call plays";
/// Pure (10.5.2): the reference plan.
///  * The user's ticked Speaker outputs (those present) are always referenced — speakers by
///    declaration, never skipped — and the outputs the call is SEEN playing on are added to them unless
///    they are headsets (before anything plays: the ticked list alone — adding the pick/default then
///    would cost every call whose ticked output already is the call's a rebuild once it plays, rig
///    "ticked"). A list ticked while the call played somewhere else (or written by
///    the echo pill's "Add … to echo cancellation", which records where the call played at that
///    moment) must not leave the call itself uncancelled once the server moves it — the same failure
///    as the untouched default (verifier 2026-09-24, rig "tickelse": −15 dB of the far end came back).
///  * Otherwise the reference listens where the call's playback streams ACTUALLY are (the server's
///    view); before anything plays, Tin Hat's own pick, else the default output BY NAME (a named
///    target is never re-routed by a remembered applet route, and a default change is then seen as
///    a change instead of piling a second monitor link onto an unnamed capture); "" only when
///    nothing could be named.
///  * Outputs that report as headphones/headsets drop out of the automatic choice; if that leaves
///    nothing, echo cancellation is skipped (nothing to cancel).
pub(crate) fn plan_ec_reference(
    chosen_present: &[String],
    call_sel: Option<&str>,
    actual: &[String],
    default_sink: Option<&str>,
    headset: &dyn Fn(&str) -> Option<String>,
) -> EcPlan {
    let mut cands: Vec<String> = Vec::new();
    let why;
    if actual.iter().any(|a| !a.is_empty()) {
        for a in actual {
            if !a.is_empty() && !cands.contains(a) {
                cands.push(a.clone());
            }
        }
        // canonical order: which participant's stream the server lists first must not read as a change
        cands.sort();
        why = "where the call plays";
    } else if let Some(s) = call_sel.filter(|s| !s.is_empty()) {
        cands.push(s.to_string());
        why = "the output you picked";
    } else if let Some(d) = default_sink.filter(|d| !d.is_empty()) {
        cands.push(d.to_string());
        why = "the default output";
    } else {
        cands.push(String::new());
        why = "the default output";
    }
    if !chosen_present.is_empty() {
        let mut outs = chosen_present.to_vec();
        let mut added = 0;
        let seen_playing = actual.iter().any(|a| !a.is_empty());
        for c in cands.into_iter().filter(|_| seen_playing) {
            if c.is_empty() || outs.contains(&c) || added >= EC_MAX_AUTO_OUTPUTS || headset(&c).is_some() {
                continue;
            }
            outs.push(c);
            added += 1;
        }
        let why = if added > 0 { EC_WHY_PICKED_PLUS_CALL } else { EC_WHY_PICKED };
        return EcPlan::Engage { outs, explicit: true, why };
    }
    let mut first_reason: Option<String> = None;
    let mut speakers: Vec<String> = Vec::new();
    for c in cands {
        match headset(&c) {
            Some(r) => {
                first_reason.get_or_insert(r);
            }
            None => speakers.push(c),
        }
    }
    if speakers.is_empty() {
        return EcPlan::Skip(first_reason.unwrap_or_else(|| "headphones".to_string()));
    }
    // the cap applies to what is referenced — headsets never take a speaker's place
    speakers.truncate(EC_MAX_AUTO_OUTPUTS);
    EcPlan::Engage { outs: speakers, explicit: false, why }
}

/// The user's ticked Speaker outputs that exist right now (logs the missing ones once per call site).
fn present_ref_outputs(log: bool) -> Vec<String> {
    let chosen = crate::aec::ref_outputs();
    let mut outs: Vec<String> = Vec::new();
    if !chosen.is_empty() {
        let present = list_devices("Audio/Sink");
        for o in &chosen {
            if present.iter().any(|(n, _)| n == o) {
                outs.push(o.clone());
            } else if log {
                eprintln!("[aec] speaker output {o} is not present right now — left out of the reference");
            }
        }
    }
    outs
}
fn mic_is_loopback() -> bool {
    mic_target().lock().unwrap().as_deref().map(|m| m.ends_with(".monitor")).unwrap_or(false)
}
/// When the user last picked Tin Hat's output (set_playback_target). The live sinks are re-pointed on a
/// detached thread (NULL-cycle per sink, up to 1.5 s each) while apply_aec re-plans the reference, so
/// for a few seconds the server can still show the streams on the OLD output: the pick itself is the
/// truth then, and the healer's look confirms (or corrects) it once the move has landed.
fn output_picked_at() -> &'static Mutex<Option<std::time::Instant>> {
    static T: OnceLock<Mutex<Option<std::time::Instant>>> = OnceLock::new();
    T.get_or_init(|| Mutex::new(None))
}
const OUTPUT_PICK_GRACE: std::time::Duration = std::time::Duration::from_secs(6);
/// The plan for right now: the ticked outputs, else a fresh look at where the call plays.
fn current_ec_plan(log: bool, routing: Option<&CallRouting>) -> EcPlan {
    let chosen_present = present_ref_outputs(log);
    let call_sel = playback_target().lock().unwrap().clone(); // guard released before any pactl
    let pick_in_flight = output_picked_at().lock().unwrap().map(|t| t.elapsed() < OUTPUT_PICK_GRACE).unwrap_or(false);
    // (verifier 2026-09-24: with a ticked list too — the call's own output joins it, see plan_ec_reference)
    let (actual, default_sink) = if !cfg!(windows) {
        let actual = if pick_in_flight {
            Vec::new()
        } else {
            match routing {
                Some(r) => r.playback.clone(),
                None => {
                    // the healer's look from a moment ago (a rebind it just asked for) — else look now
                    let cached = match last_routing().lock().unwrap().as_ref() {
                        Some((at, r)) if at.elapsed() < std::time::Duration::from_millis(1500) => Some(r.playback.clone()),
                        _ => None,
                    };
                    cached.unwrap_or_else(|| observe_call_routing().map(|r| r.playback).unwrap_or_default())
                }
            }
        };
        (actual, default_audio_sink())
    } else {
        (Vec::new(), None)
    };
    // Windows keeps a ticked list exactly as ticked (nothing is observed there; unchanged from 10.5.1)
    let call_sel = if cfg!(windows) && !chosen_present.is_empty() { None } else { call_sel };
    plan_ec_reference(&chosen_present, call_sel.as_deref(), &actual, default_sink.as_deref(), &|o| output_headset_reason(o))
}

fn dsp_aec_ensure() {
    if !crate::aec::enabled() {
        aec_stop();
        return;
    }
    if mic_is_loopback() {
        eprintln!("[aec] input is a loopback source — skipping echo cancellation");
        *dsp_skip_reason().lock().unwrap() = "the input is a loopback, not a microphone".to_string();
        aec_stop();
        return;
    }
    // 0.8.33: the user's own list of speaker outputs (Settings › Audio › Speaker outputs) is the
    // reference when set — every one of them, summed — and a ticked output IS speakers by
    // declaration, so the form-factor headset skip applies only to the automatic choice.
    // Outputs that are not present right now are left out of this binding (re-read on every
    // rebind); an emptied list falls back to the automatic choice.
    // 10.5.2: the automatic choice is where the call ACTUALLY plays (plan_ec_reference).
    match current_ec_plan(true, None) {
        EcPlan::Skip(reason) => {
            let was_on = dsp_tap().lock().unwrap().take().is_some();
            let mut r = dsp_skip_reason().lock().unwrap();
            if was_on || *r != reason {
                eprintln!("[aec] output is headphones — nothing to cancel, echo cancellation skipped ({reason})");
            }
            *r = reason;
            aec_degraded_flag().store(false, std::sync::atomic::Ordering::Relaxed);
        }
        EcPlan::Engage { outs, explicit, why } => {
            dsp_skip_reason().lock().unwrap().clear();
            let binding = outs.join("\n");
            let mut guard = dsp_tap().lock().unwrap();
            if guard.as_deref() == Some(binding.as_str()) {
                return; // same speaker binding — the next mic build carries it
            }
            eprintln!(
                "[aec] echo cancellation ON (WebRTC AEC3 in the mic pipeline; reference = {} of {})",
                if cfg!(windows) { "WASAPI loopback" } else { "PipeWire monitor" },
                if explicit && why == EC_WHY_PICKED_PLUS_CALL {
                    format!("{} — {why}", outs.join(" + "))
                } else if explicit {
                    format!("{} speaker output{} you picked: {}", outs.len(), if outs.len() == 1 { "" } else { "s" }, outs.join(" + "))
                } else if outs.len() == 1 && outs[0].is_empty() {
                    "the default output".to_string()
                } else {
                    format!("{} — {why}", outs.join(" + "))
                }
            );
            *guard = Some(binding);
        }
    }
}

/// 10.5.2: the rebind decision, pure (debounce + rate limit) so it can be unit-tested.
#[derive(Default)]
pub(crate) struct RebindGate {
    pending: Option<(String, std::time::Instant)>,
    last_fire: Option<std::time::Instant>,
    /// recent rebinds — at most REBIND_BURST per REBIND_BURST_WINDOW (each one is a mic rebuild)
    fired: std::collections::VecDeque<std::time::Instant>,
}
const REBIND_BURST: usize = 4;
const REBIND_BURST_WINDOW: std::time::Duration = std::time::Duration::from_secs(120);
impl RebindGate {
    /// `mismatch` = Some(key) while the wanted binding differs from the running one (the key names
    /// the mismatch). Fires once the SAME mismatch has been seen for ≥ `settle`, and never more
    /// often than every `min_gap` — a stream mid-move or a default flipping back and forth must not
    /// rebuild the mic on every look.
    pub(crate) fn step(&mut self, now: std::time::Instant, mismatch: Option<String>, settle: std::time::Duration, min_gap: std::time::Duration) -> bool {
        let Some(key) = mismatch else {
            self.pending = None;
            return false;
        };
        match &self.pending {
            Some((k, since)) if *k == key => {
                while self.fired.front().map(|t| now.duration_since(*t) >= REBIND_BURST_WINDOW).unwrap_or(false) {
                    self.fired.pop_front();
                }
                if now.duration_since(*since) >= settle
                    && self.last_fire.map(|t| now.duration_since(t) >= min_gap).unwrap_or(true)
                    && self.fired.len() < REBIND_BURST
                {
                    self.pending = None;
                    self.last_fire = Some(now);
                    self.fired.push_back(now);
                    return true;
                }
                false
            }
            _ => {
                self.pending = Some((key, now));
                false
            }
        }
    }
}

/// 10.5.2: does the echo reference still listen where the call plays? Asked by the call-lifetime
/// healer every 2 s (looks at most every 3 s — four pactl round-trips); `true` = rebind now (the
/// caller runs `apply_aec`, which re-plans and rebuilds the mic with the new reference). Fires on:
///   * the automatic plan changed — the call's playback moved (volume-applet move, a remembered
///     route, a default change, the playback healer's API swap), or the output it moved to is / is
///     not a headset (headset → speakers mid-call re-engages the canceller, and back);
///   * the reference drifted — the server moved a reference head off the output it was bound to.
/// Debounced (the same mismatch twice, ≥ 2 s apart) and at most one rebind per 10 s.
/// 10.5.2 (Astra): the rebind state belongs to ONE call — a rejoin must not inherit the previous
/// call's spent burst budget, and a look that started in the previous call must not act in this one.
fn rebind_state() -> &'static Mutex<(u64, Option<std::time::Instant>, RebindGate)> {
    static S: OnceLock<Mutex<(u64, Option<std::time::Instant>, RebindGate)>> = OnceLock::new();
    S.get_or_init(|| Mutex::new((0, None, RebindGate::default())))
}
/// Start of a call (the call-lifetime healer): fresh gate, fresh generation.
pub fn ec_rebind_reset() {
    let mut s = rebind_state().lock().unwrap();
    s.0 += 1;
    s.1 = None;
    s.2 = RebindGate::default();
    *last_routing().lock().unwrap() = None;
}
/// Pure: is every bound output covered by a reference head, and is no head somewhere else?
/// `refs` = the sources the heads read ("<sink>.monitor"). `None` = in step.
pub(crate) fn reference_drift(outs: &[String], refs: &[String]) -> Option<String> {
    if let Some(s) = refs.iter().find(|s| !outs.iter().any(|o| s.strip_suffix(".monitor") == Some(o.as_str()))) {
        return Some(format!("the echo reference drifted to {s} (bound to {})", outs.join(" + ")));
    }
    if let Some(o) = outs.iter().find(|o| !refs.iter().any(|s| s.strip_suffix(".monitor") == Some(o.as_str()))) {
        return Some(format!("no echo reference listens to {o} (bound to {})", outs.join(" + ")));
    }
    None
}
pub fn ec_rebind_wanted() -> bool {
    use std::time::{Duration, Instant};
    if cfg!(windows) || aec_tier() != AecTier::Dsp || !crate::aec::enabled() || mic_is_loopback() {
        return false;
    }
    let bound = dsp_tap().lock().unwrap().clone();
    let skipped = !dsp_skip_reason().lock().unwrap().is_empty();
    if bound.is_none() && !skipped {
        return false; // not engaged by the call (joining / leaving) — nothing to keep in step
    }
    let generation = {
        let mut s = rebind_state().lock().unwrap();
        if s.1.map(|t| t.elapsed() < Duration::from_millis(2900)).unwrap_or(false) {
            return false;
        }
        s.1 = Some(Instant::now());
        s.0
    };
    let Some(routing) = observe_call_routing() else { return false };
    let plan = current_ec_plan(false, Some(&routing));
    let mismatch = match (&plan, bound.as_deref()) {
        (EcPlan::Skip(_), None) => None,
        (EcPlan::Skip(r), Some(b)) => Some(format!("the call now plays on headphones ({r}) — the reference on {} is not needed", b.replace('\n', " + "))),
        (EcPlan::Engage { outs, .. }, None) => Some(format!("the call now plays on {} — not headphones", outs.join(" + "))),
        (EcPlan::Engage { outs, .. }, Some(b)) => {
            let want = outs.join("\n");
            if want != b {
                Some(format!(
                    "the call plays on {} but the echo reference listens to {}",
                    outs.join(" + "),
                    if b.is_empty() { "the unnamed default output".to_string() } else { b.replace('\n', " + ") }
                ))
            } else if outs.iter().all(|o| !o.is_empty())
                && mic_chain_flags().load(std::sync::atomic::Ordering::Relaxed) & 2 != 0
            {
                // drift: a head the server moved off its output, or an output no head listens to
                // (an unknown reference listing is never read as "none")
                routing.reference.as_deref().and_then(|refs| reference_drift(outs, refs))
            } else {
                None
            }
        }
    };
    // (verifier 2026-09-24) the running reference is in the skew-underflow state: rebuild it (same gate/budget).
    // 10.5.3: only when opted in (TINHAT_EC_UNDERFLOW_REBUILD=1) — off by default, see ec_underflow_rebuild_enabled.
    let mismatch = mismatch
        .or_else(|| ec_wrap_rebuild_reason(bound.is_some(), ec_ref_wrapped_now(), ec_underflow_rebuild_enabled()));
    let fire = {
        let mut s = rebind_state().lock().unwrap();
        if s.0 != generation {
            return false; // a new call started while we looked
        }
        s.2.step(Instant::now(), mismatch.clone(), Duration::from_secs(2), Duration::from_secs(10))
    };
    if fire {
        eprintln!("[aec] {} — rebinding the echo reference", mismatch.unwrap_or_default());
    }
    fire
}


/// Headless proof of the mic-beats-AEC contract (TINHAT_AECMIC_TEST=1, run from app
/// main like the other TINHAT_* gates; real WASAPI devices, no call, no login):
///   1. healthy tap + mic WITH webrtcdsp  -> buffers must flow
///   2. tap Null'd BEHIND the flag (the exact lying-flag state from a field
///      log of 0.3.0: probe deregistered, dsp_aec_active() still true) -> the mic
///      build must fall back to the dsp-free chain and buffers must still flow
pub fn headless_aecmic_test() -> ! {
    let _ = gst::init();
    let pull = |label: &str| -> (bool, bool) {
        match build_mic_pipeline() {
            Ok((p, sink)) => {
                let ok = p.set_state(gst::State::Playing).is_ok()
                    && sink
                        .try_pull_sample(gst::ClockTime::from_seconds(5))
                        .is_some();
                let _ = p.set_state(gst::State::Null);
                let probe = mic_chain_flags().load(std::sync::atomic::Ordering::Relaxed) & 2 != 0;
                eprintln!(
                    "[aecmic] {label}: build+start ok, buffers {}, echo probe {}",
                    if ok { "FLOWING" } else { "ABSENT" },
                    if probe { "IN the chain" } else { "not in the chain" }
                );
                (ok, probe)
            }
            Err(e) => {
                eprintln!("[aecmic] {label}: build/start FAILED: {e}");
                (false, false)
            }
        }
    };
    // Phase 1: engage echo cancellation against the default output, bypassing only the user
    // pref gate — the reference branch + probe + dsp must all come up IN the mic pipeline.
    *dsp_tap().lock().unwrap() = Some(String::new());
    eprintln!("[aecmic] reference bound: true (default output, in the mic pipeline)");
    let (p1, probe1) = pull("phase1 (dsp + reference in chain)");
    // Phase 2: bind the reference to an output that does not exist (the unplugged-speaker /
    // exclusive-grab case). The reference source fails at PAUSED, the ladder must step down
    // to the stage WITHOUT the probe and the mic must still flow.
    *dsp_tap().lock().unwrap() = Some("tinhat-no-such-output-node".to_string());
    reset_dsp_stage_block();
    std::env::set_var("TINHAT_AEC_TEST_NOFALLBACK", "1"); // PipeWire would otherwise quietly use the default sink
    let (p2, probe2) = pull("phase2 (reference output missing)");
    std::env::remove_var("TINHAT_AEC_TEST_NOFALLBACK");
    *dsp_tap().lock().unwrap() = None;
    let pass = p1 && probe1 && p2 && !probe2;
    eprintln!(
        "[aecmic] VERDICT: {}",
        if pass { "PASS — echo cancellation runs in the mic pipeline and its loss never costs the mic" } else { "FAIL" }
    );
    std::process::exit(if pass { 0 } else { 1 });
}

/// 0.8.31: headless echo-cancellation BENCH. Builds the PRODUCTION mic chain (the
/// same build_mic_pipeline the call uses, with the reference branch when EC is engaged), runs
/// it for N seconds against the current default input/output, writes the chain's output raw
/// (F32LE mono 48 k) and prints the gauge every 5 s. The bench plays a far-end signal into
/// the output and the same signal delayed + attenuated (plus near-end speech) into the mic;
/// ERLE = energy(EC off) / energy(EC on) over the far-end-only stretch of the recording.
///   TINHAT_AEC_ERLE_TEST=<secs> [TINHAT_AEC_ERLE_EC=0|1] [TINHAT_AEC_ERLE_OUT=<sink>] TINHAT_AEC_ERLE_FILE=<path>
pub fn headless_aec_erle_test() -> ! {
    use std::io::Write;
    let _ = gst::init();
    let secs: u64 = std::env::var("TINHAT_AEC_ERLE_TEST").ok().and_then(|v| v.parse().ok()).unwrap_or(30);
    let ec = std::env::var("TINHAT_AEC_ERLE_EC").map(|v| v != "0").unwrap_or(true);
    // 0.8.33: several outputs, comma-separated, become the summed reference (the bench's rig).
    let out = std::env::var("TINHAT_AEC_ERLE_OUT").unwrap_or_default().replace(',', "\n");
    let path = std::env::var("TINHAT_AEC_ERLE_FILE").unwrap_or_else(|_| "/tmp/erle-out.f32".to_string());
    *dsp_tap().lock().unwrap() = if ec { Some(out.clone()) } else { None };
    let (p, sink) = match build_mic_pipeline() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[erle] build FAILED: {e}");
            std::process::exit(2);
        }
    };
    let gauge = EcGauge::attach(&p);
    let fifo = EcFifoSet::attach(&p);
    if p.set_state(gst::State::Playing).is_err() {
        eprintln!("[erle] start FAILED");
        std::process::exit(2);
    }
    eprintln!(
        "[erle] running {secs}s: ec={ec} out={:?} chain={:?} probe={} file={path}",
        out,
        mic_chain_label().lock().unwrap().clone(),
        mic_chain_flags().load(std::sync::atomic::Ordering::Relaxed) & 2 != 0
    );
    let mut f = std::fs::File::create(&path).expect("erle output file");
    let start = std::time::Instant::now();
    let mut last = start;
    let mut samples: u64 = 0;
    while start.elapsed().as_secs() < secs {
        if let Some(sample) = sink.try_pull_sample(gst::ClockTime::from_mseconds(500)) {
            if let Some(b) = sample.buffer() {
                if let Ok(m) = b.map_readable() {
                    let _ = f.write_all(m.as_slice());
                    samples += (m.size() / 4) as u64;
                }
            }
        }
        if last.elapsed().as_secs() >= 5 {
            last = std::time::Instant::now();
            eprintln!(
                "[erle] t={:.0}s samples={samples} gauge={} fifo={}",
                start.elapsed().as_secs_f64(),
                gauge.as_ref().map(|g| g.take_window().report()).unwrap_or_else(|| "off".to_string()),
                fifo.as_ref().map(|f| f.report()).unwrap_or_else(|| "off".to_string())
            );
        }
    }
    let _ = p.set_state(gst::State::Null);
    eprintln!("[erle] done: {samples} samples written to {path}");
    std::process::exit(0);
}


/// Windows aec_start twin: (re)bind the reference tap. Always returns false —
/// there are no virtual nodes here, so playback sinks never need re-pointing.
#[cfg(windows)]
fn aec_start() -> bool {
    dsp_aec_start()
}

/// Dsp-tier aec_start (both platforms): (re)bind the reference tap. Always returns false —
/// there are no virtual nodes, so playback sinks never need re-pointing.
fn dsp_aec_start() -> bool {
    dsp_aec_ensure();
    // 0.8.31: whether the canceller is UP is answered by the next mic build (mic_chain_flags
    // bit 1 — build_mic_pipeline records it and raises the degraded flag when it had to step
    // down); there is no separate reference pipeline to be up or down any more.
    false
}
#[cfg(windows)]
fn aec_stop() {
    dsp_aec_stop();
}

/// Dsp-tier aec_stop (both platforms): drop the reference tap.
fn dsp_aec_stop() {
    // Stopping on purpose is not degradation (cf. the module tier).
    aec_degraded_flag().store(false, std::sync::atomic::Ordering::Relaxed);
    if dsp_tap().lock().unwrap().take().is_some() {
        eprintln!("[aec] echo cancellation reference unbound (the next mic build runs without it)");
    }
}
/// Is the RNNoise element actually installed? The noise-cancel toggle is a silent no-op
/// without it — the mic pipeline just omits the element and nothing tells the user, which
/// is how it can look enabled and do nothing for an entire call. Cached: a gst registry
/// lookup per settings repaint is wasteful and the answer cannot change while we run.
pub fn noise_cancel_available() -> bool {
    static OK: OnceLock<bool> = OnceLock::new();
    // 0.8.29: Standard/Strong both live in webrtcdsp now; RNNoise is the fallback engine.
    *OK.get_or_init(|| webrtcdsp_available() || gst::ElementFactory::find("audiornnoise").is_some())
}
/// Is the WebRTC audio-processing element (HPF + NS + AGC + AEC) installed?
pub(crate) fn webrtcdsp_available() -> bool {
    static OK: OnceLock<bool> = OnceLock::new();
    *OK.get_or_init(|| {
        let _ = crate::ensure_init();
        gst::ElementFactory::find("webrtcdsp").is_some()
    })
}
/// 0.8.29: should the capture chain carry the `audiornnoise` (RNNoise) element?
/// Only for Strong AND (the A/B override `TINHAT_NS_ENGINE=rnnoise`, or a GStreamer without
/// webrtcdsp where RNNoise is the only suppressor we have). Everywhere else Strong is the
/// WebRTC stage at `very-high`, which keeps the high-pass filter and AGC in front of the
/// suppressor — the thing Strong used to switch OFF, leaving RNNoise on a raw ungained mic.
pub(crate) fn mic_rnnoise_wanted() -> bool {
    if mic_ns_mode() != NsMode::Strong {
        return false;
    }
    if gst::ElementFactory::find("audiornnoise").is_none() {
        return false;
    }
    let forced = std::env::var("TINHAT_NS_ENGINE").map(|v| v.eq_ignore_ascii_case("rnnoise")).unwrap_or(false);
    forced || !webrtcdsp_available()
}

/// The node the MIC should capture from: the cancelled source when AEC is up, else the
/// user's chosen (or default) input.
fn effective_mic_node() -> Option<String> {
    if let Some((src, _)) = aec_nodes().lock().unwrap().clone() {
        return Some(src);
    }
    mic_target().lock().unwrap().clone()
}
/// The node remote audio should PLAY INTO: the AEC reference sink when up (it forwards to
/// the real speakers and feeds the canceller), else the user's chosen (or default) output.
fn effective_playback_node() -> Option<String> {
    if let Some((_, sink)) = aec_nodes().lock().unwrap().clone() {
        return Some(sink);
    }
    playback_target().lock().unwrap().clone()
}

fn mic_target() -> &'static Mutex<Option<String>> {
    static T: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    T.get_or_init(|| Mutex::new(load_saved_device("call-input", |n| {
        list_devices("Audio/Source").iter().any(|(d, _)| d == n)
    })))
}

/// Choose which input our mic captures (a source node from [`list_audio_inputs`]);
/// `None` = system default. Persisted across launches. Call [`CallSession::restart_mic`]
/// to apply mid-call.
pub fn set_mic_target(node: Option<String>) {
    let node = node.filter(|s| !s.is_empty());
    save_saved_device("call-input", node.as_deref()); // remember the MIC across launches
    *mic_target().lock().unwrap() = node;
}

/// The currently-selected MIC source node (`None` = system default), for the menu checkmark.
pub fn mic_target_node() -> Option<String> {
    mic_target().lock().unwrap().clone()
}

/// The currently-selected SPEAKER sink node (`None` = system default), for the menu checkmark.
pub fn playback_target_node() -> Option<String> {
    playback_target().lock().unwrap().clone()
}

// ── Mic RNNoise denoise toggle (DEFAULT OFF; persisted) ──────────────────────
//
// When OFF (the default), the `audiornnoise` element is NOT inserted and the mic
// capture pipeline string is BYTE-IDENTICAL to before this feature existed. Flip it
// only on an explicit user toggle, then call [`CallSession::restart_mic`] to rebuild.

fn mic_denoise_path() -> std::path::PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| {
            std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
        });
    base.join("tinhat/mic-denoise")
}
/// Mic noise suppression, ONE control (0.8.25). Before this there were two toggles that
/// looked independent: "Noise cancellation (RNNoise)" and "Noise suppression (WebRTC)" —
/// the latter set a libwebrtc AudioSourceOptions flag that is INERT on the fast-path
/// source (no APM ever ran over injected frames), so it did nothing on any machine.
///   * Off      — nothing in the chain.
///   * Standard — WebRTC noise suppression at the moderate level, inside the same
///                webrtcdsp stage the echo canceller uses (light, works everywhere
///                gst-plugins-bad is installed).
///   * Strong   — RNNoise (`audiornnoise`, gst-plugins-rs); the old "noise cancellation".
/// Persisted at `tinhat/mic-ns-mode`; the two pre-0.8.25 flag files migrate on first read.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NsMode {
    Off,
    Standard,
    Strong,
}
impl NsMode {
    pub fn as_str(self) -> &'static str {
        match self {
            NsMode::Off => "off",
            NsMode::Standard => "standard",
            NsMode::Strong => "strong",
        }
    }
    pub fn parse(s: &str) -> Option<NsMode> {
        match s.trim() {
            "off" => Some(NsMode::Off),
            "standard" => Some(NsMode::Standard),
            "strong" => Some(NsMode::Strong),
            _ => None,
        }
    }
}
fn mic_ns_mode_path() -> std::path::PathBuf {
    mic_denoise_path().with_file_name("mic-ns-mode")
}
fn load_mic_ns_mode() -> NsMode {
    if let Some(m) = std::fs::read_to_string(mic_ns_mode_path()).ok().and_then(|s| NsMode::parse(&s)) {
        return m;
    }
    // Migration: the old RNNoise flag wins (it was the one that actually did something).
    if std::fs::read_to_string(mic_denoise_path()).map(|s| s.trim() == "1").unwrap_or(false) {
        return NsMode::Strong;
    }
    if load_mic_apm("mic-noise-suppression") {
        return NsMode::Standard;
    }
    NsMode::Off
}
fn mic_ns_mode_cell() -> &'static Mutex<NsMode> {
    static D: OnceLock<Mutex<NsMode>> = OnceLock::new();
    D.get_or_init(|| Mutex::new(load_mic_ns_mode()))
}
/// The persisted noise-suppression mode (default Off).
pub fn mic_ns_mode() -> NsMode {
    *mic_ns_mode_cell().lock().unwrap()
}
/// Set + persist the noise-suppression mode. Takes effect when the mic capture pipeline is
/// next (re)built — call [`CallSession::restart_mic`] to apply mid-call.
pub fn set_mic_ns_mode(m: NsMode) {
    *mic_ns_mode_cell().lock().unwrap() = m;
    let p = mic_ns_mode_path();
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&p, m.as_str());
    // Keep the legacy flag files coherent for an older build reading the same profile.
    let _ = std::fs::write(mic_denoise_path(), if m == NsMode::Strong { "1" } else { "0" });
    store_mic_apm("mic-noise-suppression", m == NsMode::Standard);
}
/// Is RNNoise ("Strong") mic denoise currently enabled?
pub fn mic_denoise() -> bool {
    mic_ns_mode() == NsMode::Strong
}
/// Legacy boolean setter: on = Strong (RNNoise), off = Off.
pub fn set_mic_denoise(on: bool) {
    set_mic_ns_mode(if on { NsMode::Strong } else { NsMode::Off });
}

// ── Mic libwebrtc APM: Auto-Gain-Control + Noise-Suppression ─────────────────
//
// These are libwebrtc's Audio Processing Module features, applied when the mic's
// NativeAudioSource is CREATED (so a change needs a mic re-publish, not just a pipeline
// rebuild). They are DISTINCT from the RNNoise `audiornnoise` gst toggle above, and from
// EACH OTHER: AGC and NS are independently toggleable (per user request). AGC defaults ON
// (quiet unboosted mics are otherwise inaudible to peers — see mic_agc_cell); NS defaults
// OFF — a hot, already-processed line-level mic (e.g. a RODECaster) fed through AGC+NS
// together pumps and garbles the outgoing audio, and such setups turn AGC off in settings.

fn mic_apm_path(name: &str) -> std::path::PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| {
            std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
        });
    base.join(format!("tinhat/{name}"))
}
fn load_mic_apm(name: &str) -> bool {
    load_mic_apm_default(name, false)
}
/// Like [`load_mic_apm`] but with an explicit value for "the user never toggled this".
fn load_mic_apm_default(name: &str, default: bool) -> bool {
    std::fs::read_to_string(mic_apm_path(name))
        .map(|s| s.trim() == "1")
        .unwrap_or(default)
}
fn store_mic_apm(name: &str, on: bool) {
    let p = mic_apm_path(name);
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&p, if on { "1" } else { "0" });
}
fn mic_agc_cell() -> &'static Mutex<bool> {
    static D: OnceLock<Mutex<bool>> = OnceLock::new();
    // AGC defaults ON when never toggled: every mainstream client (web Element, Discord)
    // ships capture AGC on, and without it an unboosted quiet laptop mic is "extremely
    // low" to every peer with no receive-side rescue (per-user boost caps at 2×). A hot,
    // already-processed line-level source (RODECaster) that pumps under AGC turns it OFF
    // once in settings and the persisted choice wins forever after.
    D.get_or_init(|| Mutex::new(load_mic_apm_default("mic-agc", true)))
}
/// Is mic auto-gain-control enabled? (libwebrtc APM; default ON — see mic_agc_cell.)
pub fn mic_agc() -> bool {
    *mic_agc_cell().lock().unwrap()
}
/// Is the "Standard" (WebRTC) noise-suppression mode selected? (See [`NsMode`].)
pub fn mic_noise_suppression() -> bool {
    mic_ns_mode() == NsMode::Standard
}
/// Set + persist mic auto-gain-control. Takes effect on the next mic (re)publish.
pub fn set_mic_agc(on: bool) {
    *mic_agc_cell().lock().unwrap() = on;
    store_mic_apm("mic-agc", on);
}
/// Legacy boolean setter: on = Standard (WebRTC NS), off = Off.
pub fn set_mic_noise_suppression(on: bool) {
    set_mic_ns_mode(if on { NsMode::Standard } else { NsMode::Off });
}

/// The /dev/video* path our camera captures from. `None` = first camera.
/// File-backed (`~/.config/tinhat/call-camera`, mirroring `music_source`) so the chosen
/// CAMERA is remembered across launches; a stale/removed device is dropped to None (first cam).
fn camera_target() -> &'static Mutex<Option<String>> {
    static T: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    T.get_or_init(|| Mutex::new(load_saved_device("call-camera", |n| {
        camera::list_cameras().iter().any(|(d, _)| d == n)
    })))
}

/// Choose which camera publish_camera captures (a device.path from
/// [`camera::list_cameras`]); `None` = first. Persisted across launches. Call
/// [`CallSession::restart_camera`] to apply mid-call.
pub fn set_camera_target(node: Option<String>) {
    let node = node.filter(|s| !s.is_empty());
    save_saved_device("call-camera", node.as_deref()); // remember the CAMERA across launches
    *camera_target().lock().unwrap() = node;
}

/// The currently-selected CAMERA device.path (`None` = first camera), for the menu checkmark.
pub fn camera_target_node() -> Option<String> {
    camera_target().lock().unwrap().clone()
}

// ── Remembered call devices (MIC / SPEAKER / CAMERA) ─────────────────────────
//
// Each selection persists to a tiny file under `~/.config/tinhat/` (`call-input`,
// `call-output`, `call-camera`), exactly like `music_source` / `mic-denoise`. Loaded on
// the cell's first access (so the saved device is applied at the next call/monitor build —
// no separate startup hook), validated against the live device list so a vanished device
// silently falls back to the default instead of erroring.

fn saved_device_path(name: &str) -> std::path::PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| {
            std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
        });
    base.join(format!("tinhat/{name}"))
}

/// Load a saved device node from `name`, returning `None` (system default) when absent,
/// empty, or when `exists(node)` reports the device is gone (stale -> default, never error).
fn load_saved_device(name: &str, exists: impl Fn(&str) -> bool) -> Option<String> {
    let node = std::fs::read_to_string(saved_device_path(name))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())?;
    if exists(&node) {
        Some(node)
    } else {
        eprintln!("[call] saved device '{node}' ({name}) is gone — using default");
        None
    }
}

/// Persist a saved device node (empty/`None` clears it). Mirrors `set_music_source`'s write;
/// best-effort (the on-disk file is only read at next launch). The file is written by this
/// process, so it is owned by the user running the app.
fn save_saved_device(name: &str, node: Option<&str>) {
    let p = saved_device_path(name);
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&p, node.unwrap_or(""));
}

/// Enumerate camera (Video/Source) devices as `(device_path, label)`.
pub fn list_cameras() -> Vec<(String, String)> {
    camera::list_cameras()
}

/// The active call's E2EE keyring (a clone of the one inside the Room — clones share
/// the same native handle, so installing here affects decryption) TOGETHER WITH the
/// generation that registered it. Set on connect, cleared on close. Lets the app
/// install PEERS' keys without holding the call mutex.
///
/// The generation lives INSIDE the mutex on purpose: a `close()` may only clear a
/// registration it still owns, and checking that with a separate atomic left a window
/// where a newer session's registration could be clobbered between the check and the
/// clear (which is a whole-call-static bug — every later peer key would only buffer).
fn active_key_provider() -> &'static Mutex<Option<(u64, KeyProvider)>> {
    static KP: OnceLock<Mutex<Option<(u64, KeyProvider)>>> = OnceLock::new();
    KP.get_or_init(|| Mutex::new(None))
}

/// Monotonic keyring-registration counter.
static KEYRING_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Where a peer key came from. The two transports carry timestamps from DIFFERENT
/// clocks (to-device is a live push we stamp locally; a room event carries the
/// homeserver's `origin_server_ts`), so they are NEVER compared against each other —
/// each is ordered only within its own domain. See [`install_peer_key_at`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KeySrc {
    /// Encrypted to-device — a real-time push. Arrival order is truth (matches
    /// matrix-js-sdk, whose OutdatedKeyFilter stamps to-device keys on receipt).
    ToDevice,
    /// `io.element.call.encryption_keys` room event, read from history by the poll.
    Room,
}

/// One installed (or buffered) peer key.
#[derive(Clone, Copy)]
pub struct PeerKeyEntry {
    pub key: [u8; 16],
    /// Monotonic local time of the last install into this slot — the only clock both
    /// transports can be reasoned about with.
    pub installed_at: std::time::Instant,
    /// True if the live key came from the to-device push (see `TODEVICE_TRUST_MS`).
    pub from_todevice: bool,
    /// Highest `origin_server_ts` of any ROOM event we have PROCESSED for this slot
    /// (installed or not). Room events are only acted on when strictly newer than
    /// this — that alone makes each poll pass order-independent AND makes a history
    /// re-read a no-op, without ever comparing a server ts to a local one.
    pub room_ts_seen: u64,
}

/// Peer media keys, kept so a key that arrives BEFORE the call's keyring exists isn't
/// lost, AND as the freshness registry that stops a STALE key from stomping a fresh
/// one. The room poll re-reads history every few seconds: unguarded, its first pass
/// after a rejoin re-installed a peer's OLD same-index key over the fresh one they had
/// just pushed us over to-device — permanent static from that peer, one half of the
/// "coin flip until everyone reconnects" bug. Replayed on connect.
fn pending_peer_keys() -> &'static Mutex<HashMap<(String, u8), PeerKeyEntry>> {
    static P: OnceLock<Mutex<HashMap<(String, u8), PeerKeyEntry>>> = OnceLock::new();
    P.get_or_init(|| Mutex::new(HashMap::new()))
}

/// How long a to-device key holds authority over a slot against room-transport
/// history. To-device is a live push, so a key that just arrived that way is by
/// definition current; a room event we are only now READING could have been written
/// at any earlier time, and the two timestamps live in different clock domains, so
/// "newer" is not decidable between them. Rather than guess, the room transport
/// simply yields for this window — after it, a genuinely NEW room event (one with a
/// server ts we have never processed for the slot) is accepted, which is what lets a
/// web peer that fell back to room-only key delivery still re-key us.
const TODEVICE_TRUST_MS: u64 = 15_000;

/// Upper bound on distinct (identity, index) slots kept in [`pending_peer_keys`]. Every
/// slot costs a native key-provider entry too; a hostile sender must not be able to grow
/// either without limit (each is a few dozen bytes, but "unbounded" is the wrong shape).
const MAX_PENDING_PEER_KEYS: usize = 4096;

/// Decide + apply one peer-key install. Returns true if the key was ACCEPTED for the
/// slot (installed live, already the active bytes, or buffered for the connect
/// replay); false when REJECTED (superseded, already-seen history, out-of-ring index).
///
/// `ts_ms` is meaningful for [`KeySrc::Room`] only (the event's `origin_server_ts`);
/// to-device installs pass 0 and are ordered by arrival.
pub fn install_peer_key_at(identity: &str, index: u8, key: [u8; 16], ts_ms: u64, src: KeySrc) -> bool {
    // Index 255 would read one past the cryptor's 255-slot ring (see
    // matrix_key_provider's key_ring_size comment) — refuse it rather than crash.
    if index == u8::MAX {
        eprintln!("[recv3-install] REJECTED identity={identity} index=255 ({src:?}) — outside the usable keyring");
        return false;
    }
    // A received key never replaces the LOCAL sender key: the shared provider holds our
    // own (identity, index) slot too, and an install under our identity would silently
    // re-key everything we send (Astra review 2026-09-21, finding 1). The app layer
    // rejects our Matrix-derived identities before connect; this covers the identity
    // the SFU actually reported once we are in.
    {
        let me = local_identity_cell().lock().unwrap();
        if !me.is_empty() && *me == identity {
            eprintln!("[recv3-install] REJECTED identity={identity} index={index} ({src:?}) — that is OUR OWN identity");
            return false;
        }
    }
    let now = std::time::Instant::now();
    {
        let mut pending = pending_peer_keys().lock().unwrap();
        let slot = (identity.to_owned(), index);
        let prev = pending.get(&slot).copied();
        // Bound the registry: a sender can mint unlimited (identity, index) slots by
        // varying claimed_device_id. Real calls need a few dozen at most.
        if prev.is_none() && pending.len() >= MAX_PENDING_PEER_KEYS {
            eprintln!(
                "[recv3-install] REJECTED identity={identity} index={index} ({src:?}) — peer-key registry full ({MAX_PENDING_PEER_KEYS})"
            );
            return false;
        }
        if let (KeySrc::Room, Some(e)) = (src, prev) {
            // Already processed this event (or an even newer one) for this slot: a
            // plain history re-read. Refuse REGARDLESS of the key bytes — this is
            // what makes a poll pass order-independent (the pass's newest event wins
            // and every older duplicate is inert).
            if ts_ms <= e.room_ts_seen {
                return e.key == key;
            }
            // A genuinely new room event, but to-device recently served this slot:
            // yield (see TODEVICE_TRUST_MS). Still record the ts so the event can't
            // come back and stomp us the moment the window lapses.
            if e.from_todevice
                && e.installed_at.elapsed() < std::time::Duration::from_millis(TODEVICE_TRUST_MS)
                && e.key != key
            {
                eprintln!(
                    "[recv3-stale] REJECTED room key identity={identity} index={index} \
                     (to-device served this slot {}ms ago)",
                    e.installed_at.elapsed().as_millis()
                );
                pending.insert(slot, PeerKeyEntry { room_ts_seen: ts_ms, ..e });
                return false;
            }
        }
        // Same bytes: nothing to install, but keep the room-ts high-water mark moving
        // so re-reads stay inert.
        if let Some(e) = prev {
            if e.key == key {
                let room_ts_seen = match src {
                    KeySrc::Room => e.room_ts_seen.max(ts_ms),
                    KeySrc::ToDevice => e.room_ts_seen,
                };
                pending.insert(
                    slot,
                    PeerKeyEntry {
                        room_ts_seen,
                        from_todevice: e.from_todevice || src == KeySrc::ToDevice,
                        installed_at: if src == KeySrc::ToDevice { now } else { e.installed_at },
                        key,
                    },
                );
                return true;
            }
        }
        let room_ts_seen = match src {
            KeySrc::Room => prev.map(|e| e.room_ts_seen.max(ts_ms)).unwrap_or(ts_ms),
            KeySrc::ToDevice => prev.map(|e| e.room_ts_seen).unwrap_or(0),
        };
        pending.insert(
            slot,
            PeerKeyEntry {
                key,
                installed_at: now,
                from_todevice: src == KeySrc::ToDevice,
                room_ts_seen,
            },
        );
    }
    match active_key_provider().lock().unwrap().as_ref() {
        Some((_gen, kp)) => {
            kp.set_key(&ParticipantIdentity(identity.to_owned()), index as i32, key.to_vec());
            // Decisive trace: which identity string + index the key landed under, and
            // that the call is live. Compare `identity` here against [recv4-lkid]'s
            // LiveKit-reported identity — a byte mismatch is the classic black-tile bug.
            eprintln!("[recv3-install] set_key identity={identity} index={index} ({src:?}) call_active=true");
            true
        }
        // Keyring not registered yet (key arrived during join, before connect finished).
        // Buffered above; connect() replays it. failure_tolerance=-1 ⇒ the cryptor
        // recovers once the key lands, so no track is permanently lost.
        None => {
            eprintln!("[recv3-install] BUFFERED identity={identity} index={index} ({src:?}) call_active=false (replays on connect)");
            true
        }
    }
}

/// Install a PEER's media key from the to-device transport (`identity` = their
/// `userId:deviceId`) — a live push, so arrival order is truth. Room-transport
/// callers use [`install_peer_key_at`] with `KeySrc::Room` + the event's
/// `origin_server_ts`, which can never stomp a freshly-pushed key.
pub fn install_peer_key(identity: &str, index: u8, key: [u8; 16]) -> bool {
    install_peer_key_at(identity, index, key, 0, KeySrc::ToDevice)
}

/// Drop one (identity, index) slot from the peer-key registry: the app's per-device index cap
/// (keyrules::MAX_INDICES_PER_DEVICE) evicts a device's oldest index here, so no sender can
/// fill the registry. The key provider's per-identity ring keeps its bytes (the app bounds the
/// identities per device). Returns whether the slot was held.
pub fn forget_peer_key(identity: &str, index: u8) -> bool {
    pending_peer_keys().lock().unwrap().remove(&(identity.to_owned(), index)).is_some()
}

/// Enumerate audio OUTPUT devices (sinks) as `(node_name, label)`.
pub fn list_audio_outputs() -> Vec<(String, String)> {
    list_devices("Audio/Sink")
}

/// Enumerate audio INPUT devices (sources / mics) as `(node_name, label)`.
pub fn list_audio_inputs() -> Vec<(String, String)> {
    list_devices("Audio/Source")
}

/// Enumerate audio the user can SHARE to the call as the 2nd ("music") audio track:
///   * every output SINK, captured via its `.monitor` — what's PLAYING out (game/app/desktop)
///   * every real INPUT (an `Audio/Source` that isn't itself a sink monitor) — e.g. a line-in
///     or a dedicated virtual feed like "Stream Feed (no mic)" the user routes audio into
/// Labeled "… — output" / "… — input" so the two are distinguishable. (We used to list sink
/// monitors ONLY, which hid the user's real inputs and forced sharing the whole desktop.)
pub fn list_audio_sources() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (node, label) in list_devices("Audio/Sink") {
        out.push((format!("{node}.monitor"), format!("{label} — output")));
    }
    // The node we capture as the call mic: the explicit override, else the PipeWire system default
    // (what a bare `pipewiresrc` binds to). Exclude it so the user can't pick their own mic = echo.
    let mic = mic_target().lock().unwrap().clone().or_else(default_audio_source);
    for (node, label) in list_devices("Audio/Source") {
        if node.ends_with(".monitor") {
            continue; // already covered by the sink-monitor list above
        }
        if mic.as_deref() == Some(node.as_str()) {
            continue; // don't offer the user's OWN call mic as a share source (would echo)
        }
        out.push((node, format!("{label} — input")));
    }
    out
}

/// The PipeWire system-default `Audio/Source` node name (what a bare `pipewiresrc` binds to), so
/// list_audio_sources can exclude the user's own mic even when they haven't picked an explicit one.
fn default_audio_source() -> Option<String> {
    // Windows: no pactl; `None` = "system default" everywhere a target is optional
    // (bare wasapi2src binds the default endpoint), and the own-mic exclusion that
    // needs the resolved name degrades gracefully (MEDIA_PIPELINE.md §3.4).
    if cfg!(windows) {
        return None;
    }
    std::process::Command::new("pactl")
        .arg("get-default-source")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// The PipeWire system-default `Audio/Sink` node name (what playback falls back to when no
/// explicit output is chosen), so the output meter can capture its `.monitor` even when the
/// user hasn't picked a specific sink. Empty/none => the monitor falls back to a bare default.
fn default_audio_sink() -> Option<String> {
    // Windows: `None` — start_output_monitor maps None to default-endpoint loopback.
    if cfg!(windows) {
        return None;
    }
    // 10.5.2: bounded (pactl_text) — the echo reference plan runs it under the call mutex
    pactl_text(&["get-default-sink"]).map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Does a shareable source with this name currently exist — a real `Audio/Source`, or the
/// `.monitor` of a live `Audio/Sink`? Used to REFUSE a stale/empty target instead of letting
/// `pipewiresrc` silently fall back to the DEFAULT source (i.e. the mic — the reported bug).
fn audio_source_exists(node: &str) -> bool {
    if node.trim().is_empty() {
        return false;
    }
    // Per-app ids are validated by the app-side enumerator at pick time; if the
    // program has since quit, the capture pipeline fails loudly and the share stops.
    if node.starts_with("app:") || node.starts_with("pid:") {
        return true;
    }
    if let Some(sink) = node.strip_suffix(".monitor") {
        if list_devices("Audio/Sink").iter().any(|(n, _)| n == sink) {
            return true;
        }
        // Fall through: a genuine Audio/Source literally named "*.monitor" is still valid.
    }
    list_devices("Audio/Source").iter().any(|(n, _)| n == node)
}

// ── #1 Music share-source (a SELECTED + REMEMBERED PipeWire source) ───────────
//
// The music 2nd-track can be fed from a chosen PipeWire SOURCE node (a sink monitor to
// capture an output/app, or a regular input) instead of a URL. The chosen node is
// persisted (like mic-denoise / share-quality) so it's restored across sessions, and
// `start_music`/`start_music_source(None)` reuse the saved node when none is given.

fn music_source_path() -> std::path::PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| {
            std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
        });
    base.join("tinhat/music-source")
}
fn load_music_source() -> Option<String> {
    std::fs::read_to_string(music_source_path())
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    // (No longer restricted to sink ".monitor" — real inputs are shareable too. A stale saved
    // node is rejected at capture time by `audio_source_exists`, never silently becoming the mic.)
}
fn music_source_cell() -> &'static Mutex<Option<String>> {
    static M: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(load_music_source()))
}
/// The persisted music share-source node, if any (the PipeWire `node.name` to capture).
pub fn music_source() -> Option<String> {
    music_source_cell().lock().unwrap().clone()
}
/// Set + persist the chosen music share-source node (empty/`None` clears it). Used by the
/// source picker; `start_music_source(None)` then reuses it.
pub fn set_music_source(node: Option<String>) {
    let node = node.filter(|s| !s.is_empty());
    *music_source_cell().lock().unwrap() = node.clone();
    let p = music_source_path();
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&p, node.as_deref().unwrap_or(""));
}

/// #1 MUSIC: start (or replace) the music 2nd-track captured from a PipeWire SOURCE node
/// (`node = Some(...)` to use + persist that source; `None` reuses the saved source). This
/// is the PRIMARY music UX (share an audio output you pick), distinct from `start_music`'s
/// URL path. NO-OP (logs) if not in a call. Auto-unpublishes if the source pipeline errors.
pub async fn start_music_source(node: Option<String>) -> Result<()> {
    if aux_participant().lock().unwrap().is_none() {
        eprintln!("[aux/music] not in a call — join a call to share audio (no-op)");
        return Ok(());
    }
    if node.is_some() {
        set_music_source(node.clone());
    }
    let chosen = node.or_else(music_source).context(
        "no music share-source selected — pick an audio source first",
    )?;
    if !audio_source_exists(&chosen) {
        anyhow::bail!(
            "share-source '{chosen}' isn't available right now — pick another \
             (refusing to silently fall back to the microphone)"
        );
    }
    publish_aux_pipewire(AuxKind::Music, &chosen).await
}

/// 10.1.3: ONE process-wide device monitor, started once and never stopped. Every enumeration
/// used to create a `gst::DeviceMonitor`, start it, read it and stop it — and the stop is what
/// crashed a user's Flatpak (SIGSEGV in the runtime's gstpipewire 1.4.9 device provider:
/// `node_event_info` → `resync` on a core the provider had just released, i.e. a PipeWire node
/// event still in flight while the provider tore down). In a call there are a dozen of our own
/// nodes, so that window is wide, and "open Settings → Audio" or "pick a speaker" mid-call
/// killed the app (reproduced on the Flatpak test machine, core dump 2026-09-17). A monitor that never
/// stops has no such window; it is also cheaper — the provider keeps the list current itself
/// instead of rescanning on every menu open — and hot-plugged devices appear without a rescan.
pub(crate) fn device_monitor() -> Option<&'static gst::DeviceMonitor> {
    static M: OnceLock<Option<gst::DeviceMonitor>> = OnceLock::new();
    M.get_or_init(|| {
        if crate::ensure_init().is_err() {
            return None;
        }
        let m = gst::DeviceMonitor::new();
        for class in ["Audio/Source", "Audio/Sink", "Video/Source"] {
            let _ = m.add_filter(Some(class), None);
        }
        match m.start() {
            Ok(()) => Some(m),
            Err(e) => {
                eprintln!("[devices] device monitor failed to start: {e} — device lists come from pactl only");
                None
            }
        }
    })
    .as_ref()
}

fn list_devices(class: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    // Inside the Flatpak (the PulseAudio-compat heads are in use) the runtime's gstpipewire
    // device provider is not consulted for AUDIO at all: it is the plugin that crashed (above)
    // and that asserted on some node names (10.0.3), and pipewire-pulse lists the same nodes
    // under the same names. Native builds keep the provider's labels, via the shared monitor.
    let skip_gst_audio = !cfg!(windows) && pulse_head_wanted();
    if !skip_gst_audio {
        if let Some(monitor) = device_monitor() {
            for d in monitor.devices().iter() {
                if !d.has_classes(class) {
                    continue;
                }
                let label = d.display_name().to_string();
                if let Some(props) = d.properties() {
                    // PipeWire keys devices by `node.name`; the WASAPI provider has no
                    // `node.name` — its stable key is `device.id` (the IMMDevice endpoint
                    // id, which is exactly what wasapi2src/sink's `device` property takes).
                    let key = props
                        .get::<String>("node.name")
                        .or_else(|_| props.get::<String>("device.id"));
                    if let Ok(node) = key {
                        out.push((node, label));
                    }
                }
            }
        }
    }
    // 10.0.3: the gst pipewire device provider is only as good as the plugin it ships with —
    // the Flatpak runtime's (PipeWire 1.4.9) lists neither filter-chain nor loopback sources (seen on
    // the Flatpak test machine) and asserts on filter-chain virtual nodes (a virtual mic, a stream
    // feed). PulseAudio-compat (pipewire-pulse) sees the same nodes under the same names, so union
    // its view in; the monitor's labels win on overlap.
    if !cfg!(windows) {
        for (node, label) in pactl_devices(class) {
            if !out.iter().any(|(n, _)| n == &node) {
                out.push((node, label));
            }
        }
    }
    out
}

/// `pactl list sinks|sources` → (node.name, description). Monitor sources are skipped for
/// "Audio/Source" (the DeviceMonitor never lists them either; list_audio_sources derives them).
fn pactl_devices(class: &str) -> Vec<(String, String)> {
    let kind = match class {
        "Audio/Sink" => "sinks",
        "Audio/Source" => "sources",
        _ => return Vec::new(),
    };
    // 10.5.2: bounded (PACTL_DEADLINE) — apply_aec enumerates ticked outputs with this under the call mutex
    let Some(text) = pactl_bounded(&["list", kind], false) else { return Vec::new() };
    let mut res = Vec::new();
    let mut name = String::new();
    for line in text.lines() {
        let t = line.trim();
        if let Some(n) = t.strip_prefix("Name: ") {
            name = n.trim().to_string();
        } else if let Some(d) = t.strip_prefix("Description: ") {
            if !name.is_empty() && !(kind == "sources" && name.ends_with(".monitor")) {
                res.push((name.clone(), d.trim().to_string()));
            }
            name.clear();
        }
    }
    res
}

/// 10.0.3: inside the Flatpak, when two pipewiresrc rungs in a row never delivered a buffer,
/// the mic head switches to pulsesrc (pipewire-pulse over the sandbox's pulse socket). Same nodes, same
/// names, one hop more latency — a mic that works beats a mic that is theoretically better.
pub fn prefer_pulse_mic() -> &'static std::sync::atomic::AtomicBool {
    static F: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    &F
}
/// 10.0.6: the mirror of `prefer_pulse_mic` — set by the healer when the PulseAudio-compat path
/// itself never delivers inside the Flatpak, so the next rebuilds go back to native PipeWire.
pub fn prefer_native_audio() -> &'static std::sync::atomic::AtomicBool {
    static F: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    &F
}

/// 10.0.6: stream properties every PipeWire / PulseAudio element we build carries. WirePlumber's
/// restore-stream remembers per-stream volume, mute and routing across launches and re-applies
/// them to any later stream with the same key — a mute or a zero volume set on an earlier build
/// in the desktop's volume applet, or a route to a device that is gone, would silence a stream
/// that is otherwise connected and running. The app manages its own gains and targets.
const PW_NO_RESTORE: &str = "state.restore-props=(boolean)false,state.restore-target=(boolean)false";
const PA_NO_RESTORE: &str = "state.restore-props=false,state.restore-target=false";
/// 10.5.2: the same markers as WirePlumber actually reads them. Neither constant above ever reached
/// it intact: an untyped `false` (and `(boolean)false`) is a GStreamer BOOLEAN, which gstpipewire hands
/// to PipeWire as the string "FALSE" (WirePlumber's state-stream compares with "false", so restore
/// stayed ON) and gstpulse drops altogether (it maps string fields only) — measured on the rig with
/// pw-dump, 2026-09-23. `(string)` survives both. Used on the echo-reference and sniffer heads: they are
/// internal taps and share their restore key with the MIC (WirePlumber keys streams by app/media name),
/// so a route or mute the user once gave "Tin Hat" in the desktop's volume applet was re-applied to
/// the canceller's reference. The playback sinks and the mic keep the old (inert) markers on purpose —
/// whether Tin Hat should stop honouring applet routing there is a product decision, not an echo fix.
const NO_RESTORE_TYPED: &str = "state.restore-props=(string)false,state.restore-target=(string)false";
/// 10.5.2: a per-process tag on every call playback sink and every echo-reference head, so the audio
/// server's view of THIS process's streams can be read back (see [`observe_call_routing`]).
/// `tinhat.owner` is unique per process — two Tin Hats on one PipeWire (the rig) never read each
/// other's streams — and `tinhat.role` says which kind the stream is. `(string)`-typed so both
/// gstpipewire and gstpulse pass it through (see [`NO_RESTORE_TYPED`]).
fn tinhat_owner() -> &'static str {
    static O: OnceLock<String> = OnceLock::new();
    O.get_or_init(|| {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        format!("{}-{:08x}", std::process::id(), t)
    })
}
const ROLE_PLAYBACK: &str = "call-playback";
const ROLE_REFERENCE: &str = "ec-reference";
const ROLE_SNIFFER: &str = "hear-sniffer";
fn stream_tag(role: &str) -> String {
    format!("tinhat.role=(string){role},tinhat.owner=(string){}", tinhat_owner())
}
pub fn in_flatpak_sandbox() -> bool {
    std::path::Path::new("/.flatpak-info").exists()
}
/// 10.0.4: inside the Flatpak the mic and the echo-cancel reference capture through
/// PulseAudio-compat (pipewire-pulse) by DEFAULT — on a field desktop the runtime's pipewiresrc (1.4.9)
/// connected, delivered nothing, and then blocked the hangup in its NULL transition, while the pulse
/// path reads the same nodes under the same names (proven inside the sandbox on the Flatpak test machine). Per-app
/// captures (app:<serial>) stay native: pulse cannot do them. TINHAT_MIC_PIPEWIRE=1 keeps the native
/// head for testing; TINHAT_MIC_PULSE=1 forces pulse anywhere; the healer's flag still wins.
pub fn pulse_head_wanted() -> bool {
    static DEFAULT: OnceLock<bool> = OnceLock::new();
    let by_default = *DEFAULT.get_or_init(|| {
        if cfg!(windows) {
            false
        } else if std::env::var_os("TINHAT_MIC_PULSE").is_some() {
            true
        } else if std::env::var_os("TINHAT_MIC_PIPEWIRE").is_some() {
            false
        } else {
            in_flatpak_sandbox()
        }
    });
    if prefer_native_audio().load(std::sync::atomic::Ordering::Relaxed)
        && std::env::var_os("TINHAT_MIC_PULSE").is_none()
    {
        return false;
    }
    by_default || prefer_pulse_mic().load(std::sync::atomic::Ordering::Relaxed)
}

/// A live call: the E2EE Room + the gst pipelines and tasks that drive audio/video.
pub struct CallSession {
    room: Room,
    event_task: tokio::task::JoinHandle<()>,
    // Which keyring registration this session owns (see ACTIVE_KEYRING_GEN): close()
    // must not clear a registration a NEWER session has already replaced.
    keyring_gen: u64,
    mic: Option<MicHandle>,
    // The screen-share publish (NVENC), if active: pipeline + portal cast + track sid
    // + the bus-watcher stop flag (NULL posts no EOS — flip it or the thread leaks).
    screenshare: Option<(gst::Pipeline, ScreenCast, livekit::id::TrackSid, Arc<std::sync::atomic::AtomicBool>)>,
    /// Codec of the live share: AV1 (native-only viewers) or not.
    share_is_av1: bool,
    // The local camera publish, if active. The pipeline may carry a qml6glsink
    // self-view, so it's created in NULL and set PLAYING by the app on the GUI thread.
    camera: Option<CameraHandle>,
    // Remote-audio playback pipelines, keyed by track sid so each is NULLed + removed on
    // TrackUnsubscribed (a Vec leaked one PLAYING pipeline per unsubscribe for the call life).
    _playback: Arc<Mutex<std::collections::HashMap<String, PlaybackHandle>>>,
    // Remote-video render pipelines live in a process-global ([`video_tiles`]) because
    // they must be CREATED on the Qt GUI thread (qml6glsink affinity); drained in close().
    // Fired (via watch_pipeline_bus_notify) when the SHARE capture dies on its own —
    // the shared window was closed / capture source vanished. The app installs it
    // (Windows-only today) to auto-stop the share instead of freezing the last frame
    // forever. None = old log-only behavior (Linux unchanged).
    share_dead_hook: Option<Arc<dyn Fn() + Send + Sync>>,
    // Fired when the CAMERA capture dies on its own (virtual cams — NVIDIA
    // Broadcast — error out with -5 when their backing physical camera is held
    // elsewhere, e.g. by our OWN just-released direct publish; reproduced on a
    // dev machine, and a field log shows 3 mid-stream deaths). The app
    // uses it to retry once after a grace delay.
    camera_dead_hook: Option<Arc<dyn Fn() + Send + Sync>>,
    /// 0.8.28: the camera capture went QUIET (arg = ms without a real frame while
    /// the pipeline still says PLAYING) — the app restarts the capture, the way users did
    /// by hand with camera off / camera on. See lk_publish::wire_appsink_to_source_ext.
    camera_quiet_hook: Option<Arc<dyn Fn(u64) + Send + Sync>>,
    /// 10.5: the "can they hear me?" analyzer (hear.rs); None for the Memories recorder.
    hear_task: Option<tokio::task::JoinHandle<()>>,
}

/// Process-global remote-video render pipelines, keyed by tile id. Created by the app
/// on the GUI thread via [`create_video_tile`] (NOT on the LiveKit event-loop thread —
/// a qml6glsink built off the GUI thread renders black).
fn video_tiles() -> &'static Mutex<HashMap<String, VideoHandle>> {
    static T: OnceLock<Mutex<HashMap<String, VideoHandle>>> = OnceLock::new();
    T.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Build a remote-video render pipeline + feeder for `track` ON THE CURRENT THREAD
/// (call from the Qt GUI thread). Stores it under `id` and returns the pipeline as a
/// `GstElement*` (usize) for the C++ qml6glsink attach. The frame feeder is spawned
/// on the runtime Handle captured in `connect` (NOT the GUI thread's TLS).
pub fn create_video_tile(
    id: &str,
    track: livekit::webrtc::video_track::RtcVideoTrack,
) -> Result<usize> {
    let (pipeline, appsrc) = build_video_pipeline()?;
    // Light diagnostics: confirm the first decoded buffer reaches qml6glsink, and
    // surface gst/GL errors during playback (otherwise silent). The bus thread exits
    // on EOS (track ended) so it doesn't outlive the tile.
    if let Some(sink) = pipeline.by_name("sink") {
        if let Some(pad) = sink.static_pad("sink") {
            let logged = std::sync::atomic::AtomicBool::new(false);
            let label = id.to_string();
            pad.add_probe(gst::PadProbeType::BUFFER, move |_p, _info| {
                if !logged.swap(true, std::sync::atomic::Ordering::Relaxed) {
                    eprintln!("[call] {label}: first buffer reached qml6glsink (rendering)");
                }
                gst::PadProbeReturn::Ok
            });
        }
    }
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    if let Some(bus) = pipeline.bus() {
        let label = id.to_string();
        let stop_w = stop.clone();
        std::thread::spawn(move || loop {
            // Exit when the tile is torn down — NULLing the pipeline posts no EOS, so without
            // this the thread would block on timed_pop forever (one leaked thread per tile).
            if stop_w.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }
            let Some(msg) = bus.timed_pop(gst::ClockTime::from_mseconds(500)) else { continue };
            use gst::MessageView;
            match msg.view() {
                MessageView::Error(e) => eprintln!("[call] {label} gst ERROR: {} ({:?})", e.error(), e.debug()),
                MessageView::Warning(w) => eprintln!("[call] {label} gst WARN: {}", w.error()),
                MessageView::Eos(_) => break,
                _ => {}
            }
        });
    }
    let track_keep = track.clone(); // retained for cross-window rebuilds (PiP)
    let task = spawn_video_feeder(track, appsrc, id.to_string());
    let ptr = pipeline.as_ptr() as usize;
    video_tiles()
        .lock()
        .unwrap()
        .insert(id.to_string(), VideoHandle { pipeline, task, stop, track: track_keep });
    eprintln!("[call] video tile {id} pipeline created (GUI thread)");
    Ok(ptr)
}

/// Rebuild tile `id`'s render pipeline from its retained track — the cross-WINDOW
/// re-point path (PiP pop-out / pop-back). qml6glsink cannot migrate a LIVE pipeline
/// between windows: its window-system init runs only during NULL->READY (a widget
/// swapped in mid-PLAYING stays uninitialized -> permanently black, verified against
/// gst 1.28.5 qt6glitem.cc), and every in-place reset strategy DEADLOCKED live
/// (GUI-thread NULL blocks against the render thread; a render-job NULL blocks the GUI
/// thread inside QQuickWindow::event; a worker-thread NULL with the feeder alive blocks
/// in gst_gl_context_thread_add — gdb-verified). So don't migrate: abort the feeder,
/// retire the old pipeline on a DETACHED thread (if gst ever wedges there we leak one
/// idle pipeline instead of freezing the app), and build a fresh pipeline + feeder from
/// the track. The new pipeline starts in NULL, so the standard attach dance initializes
/// it against whichever window hosts the target surface; the feeder pushes DECODED
/// frames, so there is no keyframe wait. MUST run on the Qt GUI thread (same qml6glsink
/// affinity as [`create_video_tile`]). Returns the NEW pipeline ptr.
/// Is `id` a rebuildable remote tile (present in the streamer registry)? The app's
/// SELF-VIEW pipeline is a camera capture owned by CallSession, not a remote tile —
/// it cannot (and must not) be rebuilt by [`rebuild_video_tile`].
pub fn has_video_tile(id: &str) -> bool {
    video_tiles().lock().unwrap().contains_key(id)
}

pub fn rebuild_video_tile(id: &str) -> Option<usize> {
    let old = video_tiles().lock().unwrap().remove(id)?;
    let track = old.track.clone();
    old.stop.store(true, std::sync::atomic::Ordering::Relaxed); // let the bus thread exit
    old.task.abort(); // stop the feeder FIRST — an in-flight GL buffer is what wedges a NULL
    let label = id.to_string();
    std::thread::spawn(move || {
        let _ = old.pipeline.set_state(gst::State::Null);
        eprintln!("[video] {label}: old pipeline retired (cross-window rebuild)");
    });
    match create_video_tile(id, track) {
        Ok(ptr) => Some(ptr),
        Err(e) => {
            eprintln!("[call] tile rebuild failed: {e}");
            None
        }
    }
}

/// Tear down one remote-video tile (call on the GUI thread).
pub fn remove_video_tile(id: &str) {
    pip_tap_stop(id); // a popped-out tile's viewer must not outlive its track handle
    if let Some(h) = video_tiles().lock().unwrap().remove(id) {
        h.stop.store(true, std::sync::atomic::Ordering::Relaxed); // let the bus thread exit
        h.task.abort();
        let _ = h.pipeline.set_state(gst::State::Null);
    }
}

/// Tear down all remote-video tiles (call on the GUI thread / leave).
pub fn clear_video_tiles() {
    pip_tap_stop_all();
    forget_all_rec_only();
    forget_all_layers();
    for (_, h) in video_tiles().lock().unwrap().drain() {
        h.stop.store(true, std::sync::atomic::Ordering::Relaxed); // let the bus thread exit
        h.task.abort();
        let _ = h.pipeline.set_state(gst::State::Null);
    }
}

/// Remote video tracks whose render pipeline hasn't been built yet, keyed by tile id.
/// Lets the app self-heal a missed/dropped `VideoAdded` GUI closure (e.g. a rejoin race):
/// the app stashes the track here, and [`create_video_tile_from_pending`] — called from
/// the eager GUI closure AND the QML attach poll — builds the pipeline from it. The
/// `livekit` track type lives here (the app doesn't depend on `livekit`).
fn pending_video_tracks(
) -> &'static Mutex<HashMap<String, livekit::webrtc::video_track::RtcVideoTrack>> {
    static P: OnceLock<Mutex<HashMap<String, livekit::webrtc::video_track::RtcVideoTrack>>> =
        OnceLock::new();
    P.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Stash a remote video track so its tile pipeline can be (re)built later from any thread.
/// Forwarded the moved track value by the app's event drain — the app never names the type.
pub fn stash_pending_video_track(id: &str, track: livekit::webrtc::video_track::RtcVideoTrack) {
    pending_video_tracks().lock().unwrap().insert(id.to_string(), track);
}

/// Build tile `id`'s render pipeline from its stashed track (removing it from the pending
/// set) and return the pipeline ptr; `None` if no pending track (already built / never
/// stashed). Idempotent across the eager + lazy callers because the track is removed here.
/// MUST run on the Qt GUI thread (qml6glsink affinity), like [`create_video_tile`].
pub fn create_video_tile_from_pending(id: &str) -> Option<usize> {
    let track = pending_video_tracks().lock().unwrap().remove(id)?;
    match create_video_tile(id, track) {
        Ok(ptr) => Some(ptr),
        Err(e) => {
            eprintln!("[call] tile create (from pending) failed: {e}");
            None
        }
    }
}

/// Forget a pending track (peer left before its tile was ever built).
pub fn drop_pending_video_track(id: &str) {
    pending_video_tracks().lock().unwrap().remove(id);
}

/// Drop all pending tracks (call teardown).
pub fn clear_pending_video_tracks() {
    pending_video_tracks().lock().unwrap().clear();
}

// ── #9 PiP pop-out: driver-independent CPU viewer tap ─────────────────────────
//
// The pop-out window does NOT borrow the grid tile's GL pipeline. qml6glsink in a
// SECOND QQuickWindow depends on cross-context GL texture sharing, and that is
// driver-dependent: Qt 6.11 SILENTLY drops EGL share-group membership when
// share-creation fails (qeglplatformcontext.cpp retries without sharing), and
// gstqsg6material.cc bind() adopts the gst texture id into Qt's context with NO
// share-group check — a foreign id samples opaque BLACK with zero errors. Observed
// live: the owner's NVIDIA desktop rendered a black pop-out from a provably healthy
// pipeline ("first buffer reached qml6glsink", no GL errors) while the identical
// r137 build was fine on Intel/Mesa. Un-fixable app-side on the GL route.
//
// So the PiP is a pure VIEWER instead: a SECOND libwebrtc sink on the same
// RtcVideoTrack (multiple sinks per track are supported) feeds decoded I420 frames
// through libyuv to the app's PipVideoItem — a QQuickPaintedItem whose texture Qt
// creates and owns inside the PiP window's own context. The only hand-off is a CPU
// RGBA buffer, which no driver can get wrong. The grid tile's GL pipeline is never
// touched by pop-out or pop-back (no rebuild, no attach dance, no SFU impact).

/// The app-installed router delivering RGBA frames to the PipVideoItem registry
/// (keyed by tile id). Must return quickly; the app side coalesces (latest-wins).
type PipFrameRouter = Box<dyn Fn(&str, u32, u32, Vec<u8>) + Send + Sync>;

fn pip_router() -> &'static Mutex<Option<PipFrameRouter>> {
    static R: OnceLock<Mutex<Option<PipFrameRouter>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(None))
}

/// Install the app's PiP frame router (once, at startup, before any call).
pub fn set_pip_frame_router(f: PipFrameRouter) {
    *pip_router().lock().unwrap() = Some(f);
}

/// Live PiP taps: one CPU feeder task per popped-out tile.
fn pip_taps() -> &'static Mutex<HashMap<String, tokio::task::JoinHandle<()>>> {
    static T: OnceLock<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>> = OnceLock::new();
    T.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Start feeding tile `id`'s track to the PiP viewer (idempotent). Returns false if
/// the tile has no live handle yet — the PiP window retries on the same 120ms rhythm
/// as the grid attach poll. Callable from any thread.
/// The stream popover's live preview of OUR OWN share. There is no remote track to
/// tap, so the share capture callback (lk_publish) pushes a ~5 fps RGBA copy to the PiP
/// router under this id while a viewer wants it.
pub const LOCAL_SHARE_PREVIEW_ID: &str = "vid_selfshare";
static LOCAL_PREVIEW_WANTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub(crate) fn local_preview_wanted() -> bool {
    LOCAL_PREVIEW_WANTED.load(std::sync::atomic::Ordering::Relaxed)
}
pub(crate) fn route_local_frame(id: &str, w: u32, h: u32, rgba: Vec<u8>) {
    if let Some(router) = pip_router().lock().unwrap().as_ref() {
        router(id, w, h, rgba);
    }
}

pub fn pip_tap_start(id: &str) -> bool {
    if id == LOCAL_SHARE_PREVIEW_ID {
        LOCAL_PREVIEW_WANTED.store(true, std::sync::atomic::Ordering::Relaxed);
        eprintln!("[pip] local share preview ON");
        return true;
    }
    if pip_taps().lock().unwrap().contains_key(id) {
        return true;
    }
    let track = match video_tiles().lock().unwrap().get(id) {
        Some(h) => h.track.clone(),
        None => return false,
    };
    let label = id.to_string();
    let fut = async move {
        let mut stream = NativeVideoStream::new(track);
        let mut first = true;
        while let Some(frame) = stream.next().await {
            let Some(i420) = frame.buffer.as_i420() else { continue };
            let (w, h) = (i420.width(), i420.height());
            if w == 0 || h == 0 {
                continue;
            }
            // Convert only when a router is installed (it is, from app startup); the
            // registry lookup inside the router makes a viewer-less frame a cheap no-op,
            // but the convert itself is per-frame work — bail first if there's nowhere
            // to send it.
            let guard = pip_router().lock().unwrap();
            let Some(router) = guard.as_ref() else { continue };
            let (sy, su, sv) = i420.data();
            let (stride_y, stride_u, stride_v) = i420.strides();
            let mut rgba = vec![0u8; (w as usize) * (h as usize) * 4];
            // libyuv "ABGR" = R,G,B,A byte order in memory == QImage Format_RGBA8888.
            livekit::webrtc::native::yuv_helper::i420_to_abgr(
                sy, stride_y, su, stride_u, sv, stride_v, &mut rgba, w * 4, w as i32, h as i32,
            );
            if first {
                first = false;
                eprintln!("[pip] {label}: first frame converted ({w}x{h} RGBA) -> viewer");
            }
            router(&label, w, h, rgba);
        }
        eprintln!("[pip] {label}: track ended — viewer tap done");
    };
    // Same runtime story as spawn_video_feeder: the caller is the Qt GUI thread (no
    // tokio TLS) — spawn on the Handle captured in connect.
    let task = match aux_runtime().lock().unwrap().clone() {
        Some(rt) => rt.spawn(fut),
        None => {
            eprintln!("[pip] tap: no runtime handle captured — using tokio::spawn");
            tokio::spawn(fut)
        }
    };
    pip_taps().lock().unwrap().insert(id.to_string(), task);
    eprintln!("[pip] viewer tap started for {id}");
    true
}

/// Stop tile `id`'s PiP viewer tap (pop-out closed). Idempotent.
pub fn pip_tap_stop(id: &str) {
    if id == LOCAL_SHARE_PREVIEW_ID {
        LOCAL_PREVIEW_WANTED.store(false, std::sync::atomic::Ordering::Relaxed);
        eprintln!("[pip] local share preview OFF");
        return;
    }
    if let Some(task) = pip_taps().lock().unwrap().remove(id) {
        task.abort();
        eprintln!("[pip] viewer tap stopped for {id}");
    }
}

/// Stop every PiP viewer tap (call teardown).
fn pip_tap_stop_all() {
    for (id, task) in pip_taps().lock().unwrap().drain() {
        task.abort();
        eprintln!("[pip] viewer tap stopped for {id} (teardown)");
    }
}

/// Our published mic: the capture pipeline + its feeder task + the libwebrtc source
/// (kept so the capture can be re-pointed to a different input mid-call).
struct MicHandle {
    pipeline: gst::Pipeline,
    task: tokio::task::JoinHandle<()>,
    source: NativeAudioSource,
    // The published track — kept so mute can signal LiveKit (so peers see the muted
    // indicator), not just stop feeding frames locally.
    track: LocalAudioTrack,
    // The track sid — kept so a re-publish (e.g. an AGC/NS toggle, which needs a new
    // source) can unpublish the old track, and so close() can tear the mic down explicitly.
    sid: livekit::id::TrackSid,
    // Ends the call-lifetime publish-stats task (spawn_publish_stats_task) — flipped on
    // re-publish and close. restart_mic deliberately does NOT flip it: the track (and its
    // stats stream) survives a capture rebuild.
    stats_stop: Arc<std::sync::atomic::AtomicBool>,
}

/// Our published camera: the capture pipeline (possibly with a self-view tee branch),
/// the libwebrtc source (kept so capture can be re-pointed mid-call), and the track
/// sid (to unpublish). Frames are pumped by the pipeline's appsink callback (like the
/// screen-share), so there's no separate feeder task.
struct CameraHandle {
    pipeline: gst::Pipeline,
    #[allow(dead_code)]
    source: livekit::webrtc::video_source::native::NativeVideoSource,
    sid: livekit::id::TrackSid,
    #[allow(dead_code)]
    device: Option<String>,
    with_selfview: bool,
    // Stop flag for the bus-watch thread (NULL posts no EOS — see VideoHandle.stop).
    bus_stop: Arc<std::sync::atomic::AtomicBool>,
    // Ends the call-lifetime publish-stats task. SEPARATE from bus_stop: restart_camera
    // flips bus_stop for its per-pipeline watcher but REUSES the published track/source —
    // the stats stream must survive a capture rebuild (sharing bus_stop killed camera
    // stream-health + the [lk-stats-cam] gate lines permanently on a device switch).
    // Flipped on unpublish/close only.
    stats_stop: Arc<std::sync::atomic::AtomicBool>,
}

/// A remote video tile: its render pipeline (appsrc -> glupload -> qml6glsink), the task
/// feeding decoded frames into it, and a stop flag for its bus-watch thread.
struct VideoHandle {
    pipeline: gst::Pipeline,
    task: tokio::task::JoinHandle<()>,
    // Setting the pipeline to NULL posts NO EOS, so the bus-watch std::thread would block on
    // timed_pop forever (one leaked OS thread per removed tile). Signal it to exit on teardown
    // (mirrors the aux-audio AuxAudioHandle.stop pattern).
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    // The remote track, retained so the tile can be REBUILT for a cross-window re-point
    // (PiP pop-out / pop-back) — see rebuild_video_tile.
    track: livekit::webrtc::video_track::RtcVideoTrack,
}

/// One remote-audio playback: its pipeline + the feeder task pumping decoded PCM into it.
/// The task is stored so close()/unsubscribe can ABORT it — without this the bare
/// `tokio::spawn` feeder outlives the NULLed pipeline (the orphaned "[call] playback:" loop
/// that kept the SFU session alive and blocked rejoin). Mirrors `VideoHandle`.
struct PlaybackHandle {
    /// 10.4.1: a slot, not a pipeline — the feeder's healer replaces the whole pipeline in place
    /// (see build_playback), and close / unsubscribe must NULL whichever one is current.
    pipeline: Arc<Mutex<gst::Pipeline>>,
    task: tokio::task::JoinHandle<()>,
}

// ── Phase-3 2nd AUDIO track (soundboard + #7 music) ──────────────────────────
//
// A SEPARATE, short-/long-lived LocalAudioTrack (TrackSource::Unknown, dtx:false),
// COMPLETELY independent of the mic track + its NativeAudioSource. A GStreamer pipeline
// (`uridecodebin` fetches+decodes any http(s)/file URL — no Rust HTTP/mp3 crate) feeds
// mono S16LE 48k PCM into its own NativeAudioSource, exactly like the mic appsink loop.
//
// It rides the EXISTING per-sender index-0 FrameCryptor key (same Room, same encryption),
// so NO new key event is needed. The track is created ONLY when the user explicitly plays
// a sound / starts music, and unpublished on EOS or stop — so a call with nothing playing
// is byte-identical to before (the mic path is never touched).
//
// Two slots, keyed by a small enum so soundboard ("sound", ephemeral, stop-on-EOS) and
// music ("music", longer-lived) coexist without disturbing each other.

/// Which 2nd-track slot: ephemeral soundboard sound vs. longer-lived music stream.
#[derive(Clone, Copy, PartialEq, Eq)]
enum AuxKind {
    Sound,
    Music,
}

impl AuxKind {
    /// Track name (also the gst pipeline label). Distinct so a sound + music can run at once.
    fn track_name(self) -> &'static str {
        match self {
            AuxKind::Sound => "sound",
            AuxKind::Music => "music",
        }
    }
}

/// One live 2nd-audio-track publish: the gst fetch/decode pipeline, its feeder task, the
/// NativeAudioSource it pushes into (kept so it isn't dropped mid-stream), and the track
/// sid (to unpublish). Entirely separate from `MicHandle`.
struct AuxAudioHandle {
    pipeline: gst::Pipeline,
    task: tokio::task::JoinHandle<()>,
    #[allow(dead_code)]
    source: NativeAudioSource,
    sid: livekit::id::TrackSid,
    /// Set by `teardown_aux`/`close` to break the bus-watch thread out of its `timed_pop`
    /// loop on an explicit stop (NULLing the pipeline posts no EOS, so the watcher needs an
    /// out-of-band exit signal or it leaks one OS thread per stopped/replaced aux track).
    stop: Arc<std::sync::atomic::AtomicBool>,
}

/// Process-global registry of live 2nd-audio-track publishes, keyed by slot. Process-global
/// (not a `CallSession` field) so the bus-watch EOS/ERROR cleanup can tear a finished
/// sound/music down WITHOUT `&mut self` — it just removes its slot here + unpublishes via
/// [`aux_participant`]. Cleared in `close()`.
fn aux_tracks() -> &'static Mutex<HashMap<&'static str, AuxAudioHandle>> {
    static A: OnceLock<Mutex<HashMap<&'static str, AuxAudioHandle>>> = OnceLock::new();
    A.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A clone of the active call's `LocalParticipant`, captured on connect, so the 2nd-track
/// API (and its async EOS cleanup) can publish/unpublish without holding the `CallSession`.
/// `LocalParticipant` clones share the same underlying participant. Cleared on close.
fn aux_participant() -> &'static Mutex<Option<livekit::participant::LocalParticipant>> {
    static P: OnceLock<Mutex<Option<livekit::participant::LocalParticipant>>> = OnceLock::new();
    P.get_or_init(|| Mutex::new(None))
}

/// A handle to the tokio runtime the call lives on, captured (once) inside `connect` while
/// in-runtime. `teardown_aux` uses this to schedule the async `unpublish_track` from threads
/// that have NO runtime in their thread-local context — the bus-watch `std::thread` (EOS/ERROR)
/// and the Qt GUI thread (`stop_music`/`stop_url_in_call`). A bare `tokio::spawn` from those
/// would PANIC ("there is no reactor running…"), killing the unpublish and wedging the SFU with
/// a dead 2nd-track publication. `handle.spawn` is runtime-agnostic, so it works from any thread.
fn aux_runtime() -> &'static Mutex<Option<tokio::runtime::Handle>> {
    static H: OnceLock<Mutex<Option<tokio::runtime::Handle>>> = OnceLock::new();
    H.get_or_init(|| Mutex::new(None))
}

// ── Adaptive receive: per-tile subscriber control over the SFU ───────────────
//
// The SDK has NO automatic adaptiveStream engine (RoomOptions.adaptive_stream is only a
// join flag) — the client must drive the standard subscriber protocol itself. We retain
// each remote video RemoteTrackPublication (keyed by tile id) and reconcile a small
// policy onto it via the two SFU-signalled levers:
//   * set_enabled(false)          -> UpdateTrackSettings{disabled} — a true per-subscriber
//                                    PAUSE (the SFU stops forwarding; nobody else affected).
//   * update_video_dimensions(d)  -> UpdateTrackSettings{width,height} — the SFU picks the
//                                    closest simulcast rung / SVC layer for us. Preferred
//                                    over set_video_quality (which no-ops on non-simulcast
//                                    tracks); against a single-layer publisher it's harmless.
// Policy sources compose: window minimized/hidden (all tiles), per-tile hidden (AFK cut /
// zero-size slot), a persisted receive-quality cap, the per-tile rendered size, and (11.0, GPU.md
// §2.11) the app-wide window state: unfocused → cameras on the LOW layer at ≤ 15 fps, hidden →
// released unless a visible pop-out shows the tile (tinhat_widgets::lowgpu::video_policy). All
// SDK calls are scheduled onto the call's runtime Handle (their internal
// livekit_runtime::spawn panics outside a runtime — e.g. straight off the Qt GUI thread).

/// Remote VIDEO publications by tile id (`vid_<sid>`), retained on TrackSubscribed so the
/// app can signal the SFU per tile; dropped on TrackUnsubscribed / session close. The bool
/// marks a screen-share publication (exempt from dimension steering, see recv_reconcile_one).
fn remote_video_pubs(
) -> &'static Mutex<HashMap<String, (livekit::publication::RemoteTrackPublication, bool)>> {
    static P: OnceLock<
        Mutex<HashMap<String, (livekit::publication::RemoteTrackPublication, bool)>>,
    > = OnceLock::new();
    P.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Local track sids re-issued by an SDK FULL RECONNECT: handle_restarted republishes every
/// local track under a NEW server sid and announces it via LocalTrackRepublished. Our
/// handles (mic/camera/screenshare/aux) cache the publish-time sid, so any SFU-signaled op
/// through a stale sid after a reconnect silently fails — unpublish leaves a frozen ghost
/// track on peers, a quality-change republish duplicates the tile, and mute falls back to
/// the unsignaled local flag. Keyed by our fixed local track NAMES ("mic"/"camera"/
/// "screen"/aux slots), which are stable across reconnects (chain-proof, unlike sid→sid).
fn republished_sids() -> &'static Mutex<HashMap<String, livekit::id::TrackSid>> {
    static M: OnceLock<Mutex<HashMap<String, livekit::id::TrackSid>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(HashMap::new()))
}
/// The CURRENT server sid for local track `name`: the reconnect-reissued one if any,
/// else the cached publish-time `sid`.
fn live_sid(name: &str, sid: &livekit::id::TrackSid) -> livekit::id::TrackSid {
    republished_sids().lock().unwrap().get(name).cloned().unwrap_or_else(|| sid.clone())
}
/// Drop `name`'s reconnect remap entry — MUST be called by every MANUAL (re)publish the
/// moment it stores a fresh publication sid: the remap only bridges reconnect-reissued
/// sids to the cached ones, and a manual republish makes the cached sid current again.
/// Left stale, live_sid() would steer every later unpublish/mute at the pre-republish
/// (dead) sid — the new track then leaks to peers as a frozen ghost.
fn clear_republished_sid(name: &str) {
    republished_sids().lock().unwrap().remove(name);
}

#[derive(Default)]
struct RecvPolicy {
    /// Main window minimized/hidden -> pause every tile (the PiP exception is handled in
    /// QML: it simply never sets this while a pop-out is open).
    window_paused: bool,
    /// Tiles individually hidden in-app (AFK cut, zero-size slot).
    hidden: std::collections::HashSet<String>,
    /// Last known rendered size per tile, already quantized to a layer bucket.
    dims: HashMap<String, (u32, u32)>,
    /// 11.0 (GPU.md §2.11, R17): the app-wide window state "focused" | "unfocused" | "hidden" ("" = focused) — since
    /// 10.8.1 the RENDER state (an unfocused window arrives as "focused" unless the saver is on).
    win_state: String,
    /// Settings → Appearance → Performance → "Save GPU while Tin Hat isn't focused" (pref `call-unfocused-saver`;
    /// 10.8.1: default OFF, so the derived default `false` is right).
    unfocused_saver_on: bool,
    /// Tiles a VISIBLE pop-out window (the PiP) shows: they keep playing while the main window is hidden.
    popouts: std::collections::HashSet<String>,
    /// Per-tile MANUAL quality picks (the hover dropdown, YouTube-style): id →
    /// "180" | "360" | "720" | "1080". Absent = auto (tile-size steering). A manual
    /// pick beats both the global recvQuality mode and the tile-size heuristic —
    /// and, unlike auto, it applies to SCREENSHARES too: the auto exemption exists
    /// to protect shared text from an *accidental* blur, not from an explicit
    /// user choice.
    quality_overrides: HashMap<String, String>,
}
fn recv_policy() -> &'static Mutex<RecvPolicy> {
    static P: OnceLock<Mutex<RecvPolicy>> = OnceLock::new();
    P.get_or_init(|| Mutex::new(RecvPolicy::default()))
}

fn recv_quality_path() -> std::path::PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| {
            std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
        });
    base.join("tinhat/recv-quality")
}
fn recv_quality_cell() -> &'static Mutex<String> {
    static Q: OnceLock<Mutex<String>> = OnceLock::new();
    Q.get_or_init(|| {
        let s = std::fs::read_to_string(recv_quality_path()).unwrap_or_default();
        let s = s.trim();
        let mode = match s {
            "high" | "medium" | "low" => s.to_string(),
            _ => "auto".to_string(),
        };
        Mutex::new(mode)
    })
}
/// The persisted incoming-video quality mode: "auto" (tile-size driven), "high",
/// "medium", or "low" (data-saver).
pub fn recv_quality() -> String {
    recv_quality_cell().lock().unwrap().clone()
}
/// Set + persist the incoming-video quality mode and re-signal every subscribed tile.
pub fn set_recv_quality(mode: &str) {
    let mode = match mode {
        "high" | "medium" | "low" => mode,
        _ => "auto",
    };
    *recv_quality_cell().lock().unwrap() = mode.to_string();
    let p = recv_quality_path();
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&p, mode);
    spawn_recv_reconcile(None);
}

/// Quantize a rendered tile size to the nearest simulcast/SVC layer bucket, so grid
/// animations can't spam UpdateTrackSettings with per-pixel changes.
fn quantize_dims(_w: u32, h: u32) -> (u32, u32) {
    if h <= 270 {
        (320, 180)
    } else if h <= 540 {
        (640, 360)
    } else if h <= 810 {
        (1280, 720)
    } else {
        (1920, 1080)
    }
}

/// Dims for a manual per-tile pick ("180"/"360"/"720"/"1080").
fn override_dims(mode: &str) -> Option<(u32, u32)> {
    match mode {
        "180" => Some((320, 180)),
        "360" => Some((640, 360)),
        "720" => Some((1280, 720)),
        "1080" => Some((1920, 1080)),
        _ => None,
    }
}

/// The dimensions to request for tile `id` under the current mode, or None for "no
/// preference" (auto with no measured size yet -> leave the SFU default alone).
/// A manual per-tile pick (the hover dropdown) beats the global mode.
fn recv_target_dims(id: &str) -> Option<(u32, u32)> {
    if let Some(mode) = recv_policy().lock().unwrap().quality_overrides.get(id) {
        if let Some(d) = override_dims(mode) {
            return Some(d);
        }
    }
    match recv_quality().as_str() {
        "low" => Some((320, 180)),
        "medium" => Some((640, 360)),
        "high" => Some((1920, 1080)),
        _ => recv_policy().lock().unwrap().dims.get(id).copied(),
    }
}

/// Reconcile the policy onto one retained publication. MUST run inside the call runtime.
///
/// Screen-shares are EXEMPT from dimension steering: a quality cap or a small tile must
/// never blur shared text — a web (Element Call) peer's screen-share can be SVC/simulcast,
/// where update_video_dimensions(320×180) would genuinely downgrade the received layer.
/// Pause (set_enabled) still applies, so a minimized window stops the bandwidth either way.
fn recv_reconcile_one(
    id: &str,
    pubn: &livekit::publication::RemoteTrackPublication,
    is_screenshare: bool,
) {
    // 11.0 (GPU.md §2.11, R17): the window-state rule per track, then the older sources on top
    let (enable, low) = {
        let pol = recv_policy().lock().unwrap();
        let vp = tinhat_widgets::lowgpu::video_policy(tinhat_widgets::lowgpu::VideoPolicyIn {
            win: tinhat_widgets::lowgpu::WinState::parse(&pol.win_state),
            policy_on: pol.unfocused_saver_on,
            is_share: is_screenshare,
            in_visible_popout: pol.popouts.contains(id),
        });
        (vp.enabled && !pol.window_paused && !pol.hidden.contains(id), vp.low)
    };
    // A publication whose track just unsubscribed panics inside set_enabled (SDK unwraps
    // track()); a racing TrackUnsubscribed would kill this whole reconcile pass.
    if pubn.track().is_none() {
        return;
    }
    if enable {
        // ENABLE FIRST, dims second: both levers spawn independent UpdateTrackSettings
        // sends, and the dims task snapshots `disabled: !is_enabled()` — sent after the
        // enable in the reverse order, a stale disabled:true could land LAST and leave
        // the tile SFU-paused while the client believes it's enabled (frozen tile after
        // un-minimize). Enabled-first means both messages carry disabled:false.
        pubn.set_enabled(true);
        if low {
            // 11.0 (GPU.md §2.11): the window isn't focused — a camera comes in on the server's LOW layer (320×180 =
            // the publishers' w/4 rung, itself 15 fps on both apps) with an explicit 15 fps request. 10.8: ONE message
            // (update_video_settings) — as two sends (set_video_fps, then update_video_dimensions) the fps one, which
            // carries the OLD dimensions, went out last and kept the tile on its old layer.
            pubn.update_video_settings(livekit::track::TrackDimension(320, 180), tinhat_widgets::lowgpu::UNFOCUSED_CAM_FPS);
            return;
        }
        // Back to full: an explicit full-rate request (0 would leave the SFU's lowered temporal layer in place),
        // in the same message as the tile's dimensions (10.8: two sends could leave the tile on 320×180).
        let fps = match pubn.video_fps() {
            0 => 0,
            _ => 60,
        };
        // Screenshare exemption applies only to AUTO steering (protecting shared text
        // from accidental blur). A MANUAL per-tile pick is the user's explicit choice
        // and is honored on every kind of tile.
        let manual =
            recv_policy().lock().unwrap().quality_overrides.contains_key(id);
        let dims = if !is_screenshare || manual { recv_target_dims(id) } else { None };
        match dims {
            Some((w, h)) => pubn.update_video_settings(livekit::track::TrackDimension(w, h), fps),
            None if pubn.video_fps() != fps => pubn.set_video_fps(fps),
            None => {}
        }
    } else {
        pubn.set_enabled(false);
    }
}

/// Schedule a reconcile of one tile (or all, `id=None`) onto the call runtime. No-op
/// when not in a call. Safe from any thread (Qt GUI thread included).
fn spawn_recv_reconcile(id: Option<String>) {
    let handle = aux_runtime().lock().unwrap().clone();
    let Some(handle) = handle else { return };
    handle.spawn(async move {
        let pubs: Vec<(String, (livekit::publication::RemoteTrackPublication, bool))> = {
            let reg = remote_video_pubs().lock().unwrap();
            match &id {
                Some(id) => reg.get(id).map(|p| (id.clone(), p.clone())).into_iter().collect(),
                None => reg.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            }
        };
        for (tid, (pubn, is_screenshare)) in pubs {
            // set_enabled unwraps the publication's track; a TrackUnsubscribed landing
            // between our is_none() pre-check and the call still panics (SDK TOCTOU).
            // Contain it per-publication — one racing tile must not abort the pass and
            // leave the remaining tiles unreconciled (frozen after un-minimize).
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                recv_reconcile_one(&tid, &pubn, is_screenshare);
            }));
            if r.is_err() {
                eprintln!("[recv-adapt] reconcile of {tid} raced an unsubscribe — skipped");
            }
        }
    });
}

/// 11.0 (GPU.md §2.11, R17): the app-wide RENDER state ("focused" | "unfocused" | "hidden"; 10.8.1: "unfocused" only
/// arrives with the saver on) and Settings → Appearance → Performance → "Save GPU while Tin Hat isn't focused"
/// (`saver_on`, default off). Unfocused + saver: camera tiles on the LOW layer at ≤ 15 fps
/// (shares unchanged — the watched share stays sharp); hidden: every tile released at the SFU (no decode) unless a
/// visible pop-out shows it; focused: the tile-size / picked / Settings layer and full rate again, at once.
pub fn set_window_state(state: &str, saver_on: bool) {
    {
        let mut pol = recv_policy().lock().unwrap();
        if pol.win_state == state && pol.unfocused_saver_on == saver_on {
            return;
        }
        pol.win_state = state.to_string();
        pol.unfocused_saver_on = saver_on;
    }
    eprintln!("[recv-adapt] window {state} (saver {}) -> reconcile remote video", if saver_on { "on" } else { "off" });
    spawn_recv_reconcile(None);
}

/// 11.0 (GPU.md §2.11): the tiles a visible pop-out window shows (they keep playing while the main window is hidden).
pub fn set_popout_tiles(ids: Vec<String>) {
    {
        let mut pol = recv_policy().lock().unwrap();
        let next: std::collections::HashSet<String> = ids.into_iter().collect();
        if pol.popouts == next {
            return;
        }
        pol.popouts = next;
    }
    spawn_recv_reconcile(None);
}

/// Window minimized/hidden: SFU-pause (or resume) EVERY remote video tile. This is the
/// real bandwidth stop behind the existing local gst pause — the SFU stops forwarding
/// those streams to US; the publisher and every other receiver are unaffected.
pub fn set_window_video_paused(paused: bool) {
    {
        let mut pol = recv_policy().lock().unwrap();
        if pol.window_paused == paused {
            return;
        }
        pol.window_paused = paused;
    }
    eprintln!("[recv-adapt] window {} -> {} remote video at the SFU",
        if paused { "hidden" } else { "shown" }, if paused { "pausing" } else { "resuming" });
    spawn_recv_reconcile(None);
}

// ── Recording-only share (0.8.22) ──────────────────────────────────────────────
// A publisher can mark their screen share "for the recording only": it is published
// normally (the Memories recorder captures it like any other track) but every receiver
// hides it (SFU-paused = zero bandwidth) and mutes its audio until they TUNE IN. The
// flag rides a LiveKit participant ATTRIBUTE (`tinhat.recording_only` = the share's sid,
// "" = none) — no track metadata exists in this tree and the track NAME is load-bearing
// for reconnect sid remapping, so neither could carry it.
/// Data-message topic. (A participant ATTRIBUTE was the first design; our LiveKit tokens
/// come from Element's lk-jwt-service without the canUpdateOwnMetadata grant — the SFU
/// answered NotAllowed — while publishing data IS granted. Reliable messages + a re-send
/// to every late joiner give the same "everyone knows" property.)
pub const REC_ONLY_TOPIC: &str = "tinhat.recording_only";
/// identity -> share sid that peer announced as recording-only.
fn announced_rec_only() -> &'static Mutex<HashMap<String, String>> {
    static M: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(HashMap::new()))
}
/// What WE last announced (Some(sid) while our recording-only share is live) — re-sent
/// to participants who join later.
fn my_rec_only_sid() -> &'static Mutex<Option<String>> {
    static M: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(None))
}
pub(crate) fn rec_only_packet(sid: &str) -> livekit::DataPacket {
    livekit::DataPacket {
        payload: format!("{{\"sid\":\"{sid}\"}}").into_bytes(),
        topic: Some(REC_ONLY_TOPIC.to_string()),
        reliable: true,
        destination_identities: Vec::new(),
    }
}
/// A peer told us which of their shares is recording-only ("" = none): remember it and
/// apply it to the tiles we already have (a later TrackSubscribed reads the map).
fn handle_rec_only_announcement(who: &str, sid: &str) {
    if who.is_empty() {
        return;
    }
    if sid.is_empty() {
        announced_rec_only().lock().unwrap().remove(who);
    } else {
        announced_rec_only().lock().unwrap().insert(who.to_string(), sid.to_string());
    }
    let flagged_id = if sid.is_empty() { None } else { Some(tile_id(sid)) };
    let stale: Vec<String> = rec_only_tiles()
        .lock()
        .unwrap()
        .iter()
        .filter(|(id, owner)| owner.as_str() == who && Some((*id).clone()) != flagged_id)
        .map(|(id, _)| id.clone())
        .collect();
    for id in stale {
        apply_rec_only(&id, who, false);
    }
    if let Some(id) = flagged_id {
        let known = remote_video_pubs().lock().unwrap().contains_key(&id);
        if known {
            apply_rec_only(&id, who, true);
        }
    }
}
static SHARE_REC_ONLY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Publisher side: flag OUR next/current share as recording-only (announced on publish).
pub fn set_share_recording_only(on: bool) {
    SHARE_REC_ONLY.store(on, std::sync::atomic::Ordering::Relaxed);
}
pub fn share_recording_only() -> bool {
    SHARE_REC_ONLY.load(std::sync::atomic::Ordering::Relaxed)
}
/// Receiver side: tile id -> owner identity for shares flagged recording-only.
fn rec_only_tiles() -> &'static Mutex<HashMap<String, String>> {
    static M: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(HashMap::new()))
}
/// Recording-only tiles this user chose to watch anyway.
fn tuned_in_tiles() -> &'static Mutex<std::collections::HashSet<String>> {
    static M: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}
/// Identities whose "screenshare" audio is muted because their share is recording-only.
fn rec_only_muted() -> &'static Mutex<std::collections::HashSet<String>> {
    static M: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}
/// Recorder-side: recording-only shares already logged as "capturing anyway" (log
/// dedupe — the publisher re-announces every 5 s and on every join).
fn vod_rec_only_noted() -> &'static Mutex<std::collections::HashSet<String>> {
    static S: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}
/// `{"<tile id>":[rec_only,tuned_in],…}` for every recording-only tile — the app polls this
/// (the tile list itself doesn't change when a flag flips).
pub fn rec_only_flags_json() -> String {
    let ro = rec_only_tiles().lock().unwrap();
    let tuned = tuned_in_tiles().lock().unwrap();
    let mut s = String::from("{");
    for (i, (id, _)) in ro.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!("\"{}\":[true,{}]", id, tuned.contains(id)));
    }
    s.push('}');
    s
}
/// (recording_only, tuned_in) for a tile — the app puts both on the tile JSON.
pub fn tile_recording_state(id: &str) -> (bool, bool) {
    (
        rec_only_tiles().lock().unwrap().contains_key(id),
        tuned_in_tiles().lock().unwrap().contains(id),
    )
}
fn set_rec_only_audio_mute(identity: &str, muted: bool) {
    if muted {
        rec_only_muted().lock().unwrap().insert(identity.to_string());
    } else {
        rec_only_muted().lock().unwrap().remove(identity);
    }
    apply_layered_mute(identity, "screenshare");
}
/// A share's recording-only status became known or changed (subscribe / attribute event).
fn apply_rec_only(id: &str, identity: &str, rec_only: bool) {
    // Memories recorder (RECORD_DIR set — the recorder service only): "recording only"
    // means "for the Memories recorder". The recorder is the ONE subscriber that must
    // never SFU-pause (hide) or mute such a share. Without this guard the announcement
    // paused the share at the SFU for the recorder too and the Memories share file came
    // out empty (caught 2026-09-07, gate-vodrec.sh, before the live service picked up
    // the flag-aware build).
    if record_dir().is_some() {
        if rec_only && vod_rec_only_noted().lock().unwrap().insert(id.to_string()) {
            eprintln!(
                "[vod] {identity}'s share {id} is flagged recording-only — capturing it (that is what the flag is for)"
            );
        }
        return;
    }
    let was = rec_only_tiles().lock().unwrap().contains_key(id);
    if rec_only == was {
        return;
    }
    if rec_only {
        rec_only_tiles().lock().unwrap().insert(id.to_string(), identity.to_string());
        let tuned = tuned_in_tiles().lock().unwrap().contains(id);
        eprintln!("[call] {identity}'s share {id} is RECORDING-ONLY (tuned_in={tuned}) — hidden + muted unless tuned in");
        if !tuned {
            set_tile_video_hidden(id, true);
            set_rec_only_audio_mute(identity, true);
        }
    } else {
        rec_only_tiles().lock().unwrap().remove(id);
        let tuned = tuned_in_tiles().lock().unwrap().remove(id);
        eprintln!("[call] {identity}'s share {id} is no longer recording-only");
        if !tuned {
            set_tile_video_hidden(id, false);
            set_rec_only_audio_mute(identity, false);
        }
    }
}
/// Forget a tile's recording-only bookkeeping (track gone / call closed).
fn forget_rec_only(id: &str) {
    vod_rec_only_noted().lock().unwrap().remove(id);
    if let Some(identity) = rec_only_tiles().lock().unwrap().remove(id) {
        tuned_in_tiles().lock().unwrap().remove(id);
        rec_only_muted().lock().unwrap().remove(&identity);
    }
}
fn forget_all_rec_only() {
    vod_rec_only_noted().lock().unwrap().clear();
    announced_rec_only().lock().unwrap().clear();
    *my_rec_only_sid().lock().unwrap() = None;
    rec_only_tiles().lock().unwrap().clear();
    tuned_in_tiles().lock().unwrap().clear();
    rec_only_muted().lock().unwrap().clear();
}
/// The viewer's choice: watch (and hear) a recording-only share, or stop again.
pub fn set_tile_tuned_in(id: &str, tuned: bool) {
    let Some(identity) = rec_only_tiles().lock().unwrap().get(id).cloned() else { return };
    let changed = if tuned {
        tuned_in_tiles().lock().unwrap().insert(id.to_string())
    } else {
        tuned_in_tiles().lock().unwrap().remove(id)
    };
    if !changed {
        return;
    }
    eprintln!("[call] recording-only share {id}: tuned {}", if tuned { "IN" } else { "OUT" });
    set_tile_video_hidden(id, !tuned);
    set_rec_only_audio_mute(&identity, !tuned);
}

// ── 0.8.23: local mute layers — share audio only while WATCHED, GAME CHAT ─────────────────
//
// Both sit on the same per-track `volume` elements as the user's saved pref, and both stay
// OUT of `desired_track_prefs` (never persist a mute the user did not set). Composition is
// centralised in [`apply_layered_mute`]:
//   screenshare: recording-only || not watched || user pref
//   mic:         game chat                     || user pref
// …then the deafen/AFK ownership rule: never un-mute through an active cut (hand the element
// to the auto layer so the cut's clear restores it); a mute drops auto ownership.

/// Every share VIDEO tile id -> owner identity (not just recording-only ones): the link from
/// "the tile I am watching" to "whose share audio to unmute". Recorder never fills it.
fn share_tile_owner() -> &'static Mutex<HashMap<String, String>> {
    static M: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(HashMap::new()))
}
/// Share tiles the app says the user is WATCHING (pinned / the only stream / popped out /
/// an explicit Listen). Session-only; QML is the source of truth and re-pushes it.
fn watched_share_tiles() -> &'static Mutex<std::collections::HashSet<String>> {
    static M: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}
/// Identities whose share audio is muted because none of their share tiles is watched.
fn share_watch_muted() -> &'static Mutex<std::collections::HashSet<String>> {
    static M: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}
/// The non-user local layers for one (full identity, category).
fn layer_muted(identity: &str, category: &str) -> bool {
    // 11.3 same room: we hear a linked peer in person — Tin Hat never plays them (mic, share audio, sounds).
    if crate::sameroom::is_linked_peer(identity) {
        return true;
    }
    match category {
        "screenshare" => {
            rec_only_muted().lock().unwrap().contains(identity)
                || share_watch_muted().lock().unwrap().contains(identity)
        }
        "mic" => game_chat_muted().lock().unwrap().contains(identity),
        _ => false,
    }
}
/// Why [`layer_muted`] is true (log text).
fn layer_reason(identity: &str, category: &str) -> &'static str {
    match category {
        "screenshare" if rec_only_muted().lock().unwrap().contains(identity) => "recording-only, not tuned in",
        "screenshare" => "stream not watched",
        _ if crate::sameroom::is_linked_peer(identity) => "same room — you hear each other in person",
        "mic" => "game chat — you both hear each other in-game",
        _ => "",
    }
}
/// Apply every local mute layer for one (full identity, category) onto its live elements,
/// honouring the deafen/AFK ownership rule (see the module note above).
fn apply_layered_mute(identity: &str, category: &str) {
    let want = layer_muted(identity, category)
        || desired_pref_for(identity, category).map(|p| p.1).unwrap_or(false);
    let effective = *away_active().lock().unwrap() || *deaf_active().lock().unwrap();
    let entries: Vec<((String, String, String), glib::WeakRef<gst::Element>)> = track_gains()
        .lock()
        .unwrap()
        .iter()
        .filter(|((id, cat, _), _)| cat == category && id == identity)
        .map(|(k, w)| (k.clone(), w.clone()))
        .collect();
    for (key, weak) in entries {
        let Some(el) = weak.upgrade() else { continue };
        if want {
            el.set_property("mute", true);
            away_muted_names().lock().unwrap().remove(&key); // ours: a cut's clear must not undo it
        } else if effective {
            el.set_property("mute", true);
            away_muted_names().lock().unwrap().insert(key); // restored when the cut clears
        } else {
            el.set_property("mute", false);
        }
    }
}

/// The app tells us which share tiles the user is WATCHING. Idempotent.
pub fn set_tile_watched(id: &str, watched: bool) {
    let changed = if watched {
        watched_share_tiles().lock().unwrap().insert(id.to_string())
    } else {
        watched_share_tiles().lock().unwrap().remove(id)
    };
    if !changed {
        return;
    }
    let Some(identity) = share_tile_owner().lock().unwrap().get(id).cloned() else { return };
    eprintln!(
        "[call] share tile {id} ({identity}) {}",
        if watched { "WATCHED — its audio plays" } else { "not watched — its audio is muted" }
    );
    reconcile_share_watch(&identity);
}
/// A share video tile appeared / left: keep the owner map and the mute in step.
fn note_share_tile(id: &str, identity: &str) {
    share_tile_owner().lock().unwrap().insert(id.to_string(), identity.to_string());
    reconcile_share_watch(identity);
}
fn forget_share_tile(id: &str) {
    let owner = share_tile_owner().lock().unwrap().remove(id);
    watched_share_tiles().lock().unwrap().remove(id);
    if let Some(owner) = owner {
        reconcile_share_watch(&owner);
    }
}
/// Recompute the not-watched mute for one identity: muted iff they own >= 1 share VIDEO
/// tile and none of those tiles is watched. An audio-only share (no tile to watch) is
/// NEVER gated — it would be silent forever.
fn reconcile_share_watch(identity: &str) {
    let tiles: Vec<String> = share_tile_owner()
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, o)| o.as_str() == identity)
        .map(|(t, _)| t.clone())
        .collect();
    let any_watched = {
        let w = watched_share_tiles().lock().unwrap();
        tiles.iter().any(|t| w.contains(t))
    };
    let mute = !tiles.is_empty() && !any_watched;
    let changed = if mute {
        share_watch_muted().lock().unwrap().insert(identity.to_string())
    } else {
        share_watch_muted().lock().unwrap().remove(identity)
    };
    if changed {
        eprintln!(
            "[call] {identity} share audio {}",
            if mute { "MUTED (stream not watched)" } else { "audible (watched, or no video tile to watch)" }
        );
    }
    apply_layered_mute(identity, "screenshare");
}
/// `{"<tile id>":true,…}` for every watched share tile (diagnostics / gates).
pub fn watched_tiles_json() -> String {
    let w = watched_share_tiles().lock().unwrap();
    let mut s = String::from("{");
    for (i, id) in w.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!("\"{id}\":true"));
    }
    s.push('}');
    s
}

// Game chat: "I am also in the game's own voice chat". My mic stays live to the room;
// a peer who ALSO has the flag does not hear my mic here (they hear me in-game) and I do not
// hear theirs — applied symmetrically on the receiver side, so nobody is heard twice and
// everyone else hears everyone. Same data-message route as recording-only (attributes are
// NotAllowed with lk-jwt tokens): `{"on":bool}` on `tinhat.game_chat`, re-sent to late joiners.
pub const GAME_CHAT_TOPIC: &str = "tinhat.game_chat";
static MY_GAME_CHAT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Peers who announced game chat ON (full identities).
fn game_chat_peers() -> &'static Mutex<std::collections::HashSet<String>> {
    static M: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}
/// Peers whose MIC we currently silence for game chat (= peers ∩ my flag on).
fn game_chat_muted() -> &'static Mutex<std::collections::HashSet<String>> {
    static M: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}
pub(crate) fn game_chat_packet(on: bool) -> livekit::DataPacket {
    livekit::DataPacket {
        payload: format!("{{\"on\":{on}}}").into_bytes(),
        topic: Some(GAME_CHAT_TOPIC.to_string()),
        reliable: true,
        destination_identities: Vec::new(),
    }
}
/// Flip MY game-chat flag (the app announces it to peers separately). Off at every join.
pub fn set_local_game_chat(on: bool) {
    let was = MY_GAME_CHAT.swap(on, std::sync::atomic::Ordering::Relaxed);
    if was != on {
        eprintln!("[call] game chat {}", if on { "ON — peers in game chat won't hear my mic here" } else { "OFF" });
    }
    reconcile_game_chat();
}
pub fn local_game_chat() -> bool {
    MY_GAME_CHAT.load(std::sync::atomic::Ordering::Relaxed)
}
/// Is this peer in game chat (for badges)?
pub fn game_chat_peer(identity: &str) -> bool {
    game_chat_peers().lock().unwrap().contains(identity)
}
fn handle_game_chat_announcement(who: &str, on: bool) {
    if who.is_empty() {
        return;
    }
    let changed = if on {
        game_chat_peers().lock().unwrap().insert(who.to_string())
    } else {
        game_chat_peers().lock().unwrap().remove(who)
    };
    if changed {
        eprintln!("[call] {who}: game chat {}", if on { "ON" } else { "OFF" });
    }
    reconcile_game_chat();
}
/// 11.3 same room: the latest decoded RMS of one identity's track in `category` (0 when none / silent).
pub(crate) fn track_level_of(identity: &str, category: &str) -> f32 {
    track_levels().lock().unwrap().get(&(identity.to_string(), category.to_string())).copied().unwrap_or(0.0)
}
/// 11.3 same room: re-evaluate the mute layers of these identities (link / unlink / leave) — every
/// category, since a linked peer is heard in person whatever they send.
pub(crate) fn reconcile_same_room(ids: &[String]) {
    for id in ids {
        for cat in ["mic", "screenshare", "music"] {
            apply_layered_mute(id, cat);
        }
        eprintln!("[sr] playback of {id}: {}", if crate::sameroom::is_linked_peer(id) { "muted (same room — you hear each other in person)" } else { "audible again" });
    }
}
/// desired = peers ∩ (my flag on); mute the newcomers, restore the leavers.
fn reconcile_game_chat() {
    let desired: std::collections::HashSet<String> = if local_game_chat() {
        game_chat_peers().lock().unwrap().clone()
    } else {
        std::collections::HashSet::new()
    };
    let current: std::collections::HashSet<String> = game_chat_muted().lock().unwrap().clone();
    if desired == current {
        return;
    }
    for id in desired.difference(&current) {
        eprintln!("[call] game chat: muting {id}'s mic here (you hear each other in-game)");
    }
    for id in current.difference(&desired) {
        eprintln!("[call] game chat: {id}'s mic audible here again");
    }
    *game_chat_muted().lock().unwrap() = desired.clone();
    for id in current.union(&desired) {
        apply_layered_mute(id, "mic");
    }
}
/// `{"me":bool,"peers":["<identity>",…],"muted":["<identity>",…]}` — the app mirrors this.
pub fn game_chat_json() -> String {
    let peers = game_chat_peers().lock().unwrap();
    let muted = game_chat_muted().lock().unwrap();
    let list = |set: &std::collections::HashSet<String>| -> String {
        let mut v: Vec<&String> = set.iter().collect();
        v.sort();
        v.iter().map(|s| format!("\"{}\"", s.replace('"', ""))).collect::<Vec<_>>().join(",")
    };
    format!("{{\"me\":{},\"peers\":[{}],\"muted\":[{}]}}", local_game_chat(), list(&peers), list(&muted))
}
fn forget_game_chat_peer(identity: &str) {
    game_chat_peers().lock().unwrap().remove(identity);
    reconcile_game_chat();
}
/// Call closed: drop every 0.8.23 layer (the next call starts clean: no game chat, nothing
/// watched — QML re-pushes what is watched as tiles appear).
fn forget_all_layers() {
    MY_GAME_CHAT.store(false, std::sync::atomic::Ordering::Relaxed);
    game_chat_peers().lock().unwrap().clear();
    game_chat_muted().lock().unwrap().clear();
    share_tile_owner().lock().unwrap().clear();
    watched_share_tiles().lock().unwrap().clear();
    share_watch_muted().lock().unwrap().clear();
    *corners().lock().unwrap() = Corners::default();
    forget_all_moderation();
}

// ── 9.4 corners — "pull aside" (2026-09-16) ────────────────────────────────────────────
// In a noisy call, right-click someone → "Pull aside": the two of you hear everyone else at
// 10 % (mic, stream, soundboard) and nobody outside the corner hears your mics; either of you
// leaves with the yellow button. Several corners can exist at once, a corner can hold more than
// two, and right-clicking someone already aside offers "Join" instead. Late joiners learn the
// corners the way they learn game chat: a re-announce when they connect.
//
// Model: every participant announces the corner id they are in — `{"corner":"<id>"}` on
// `tinhat.corner`, "" = none; a pull adds `"pull":"<identity>"` and the target's client, if free,
// takes that id and announces it. A corner is ACTIVE once two announce it (me included); a
// member alone in an id (a pull nobody answered, the other side hung up) drops out after the
// grace period. Everything is receiver-side, exactly like game chat: my playback gains for
// OTHERS change, nothing I publish does — the SFU, E2EE and the recorder are untouched (the
// recorder still records the whole room).
//
// Game chat and a corner never overlap: entering an active corner suspends my game-chat flag
// (announced OFF) and leaving it restores it (announced ON) — the yellow button stands in for
// the blue one while the corner lasts.
pub const CORNER_TOPIC: &str = "tinhat.corner";
/// What everyone outside my corner plays at while I am in one.
pub const CORNER_OUTSIDE_GAIN: f64 = 0.1;
/// How long a member may sit alone in a corner id before the client drops it (ms; a test hook).
static CORNER_ALONE_GRACE_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(4000);

#[derive(Default)]
struct Corners {
    /// My corner id ("" = none). Set the moment I pull/join; ACTIVE once a peer shares it.
    mine: String,
    /// Peer identity -> corner id (only non-empty ids are kept).
    peers: HashMap<String, String>,
    /// When I last found myself alone in `mine` (None = not alone / not in one).
    alone_since: Option<std::time::Instant>,
    /// Game chat was ON when the corner became active — put it back when the corner ends.
    resume_game_chat: bool,
    /// Was my corner active at the last reconcile (edge detection for the game-chat swap).
    was_active: bool,
    /// Announcements the app must publish: (corner id, pull target). Drained by the app's mirror
    /// tick — the streamer has no runtime handle on the GUI thread.
    pending: Vec<(String, String)>,
    /// A game-chat (re)announce is due (the swap changed my flag).
    game_chat_announce_due: bool,
    /// Identities in the call (a pull target may arrive as a bare mxid).
    participants: std::collections::HashSet<String>,
}
fn corners() -> &'static Mutex<Corners> {
    static C: OnceLock<Mutex<Corners>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(Corners::default()))
}
pub(crate) fn corner_packet(id: &str, pull: &str) -> livekit::DataPacket {
    let esc = |s: &str| s.replace(['\\', '"'], "");
    let payload = if pull.is_empty() {
        format!("{{\"corner\":\"{}\"}}", esc(id))
    } else {
        format!("{{\"corner\":\"{}\",\"pull\":\"{}\"}}", esc(id), esc(pull))
    };
    livekit::DataPacket {
        payload: payload.into_bytes(),
        topic: Some(CORNER_TOPIC.to_string()),
        reliable: true,
        destination_identities: Vec::new(),
    }
}
/// One string field of a corner packet (no serde on this path, same as the sid parse above).
fn corner_field<'a>(text: &'a str, key: &str) -> &'a str {
    let pat = format!("\"{key}\":\"");
    text.split(pat.as_str()).nth(1).and_then(|r| r.split('"').next()).unwrap_or("")
}
fn corner_new_id() -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    local_identity_cell().lock().unwrap().hash(&mut h);
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    format!("k{:x}{:x}", h.finish() & 0xffff_ffff, ms & 0xff_ffff)
}
/// Peers announcing `id` (sorted).
fn corner_peer_members(c: &Corners, id: &str) -> Vec<String> {
    let mut v: Vec<String> =
        c.peers.iter().filter(|(_, cid)| cid.as_str() == id).map(|(p, _)| p.clone()).collect();
    v.sort();
    v
}
/// My corner id when it is ACTIVE (a peer shares it).
fn my_active_corner(c: &Corners) -> Option<String> {
    (!c.mine.is_empty() && c.peers.values().any(|cid| *cid == c.mine)).then(|| c.mine.clone())
}
/// The ACTIVE corner this peer is in (two announcers, me included).
fn active_corner_of(c: &Corners, identity: &str) -> Option<String> {
    let id = c.peers.get(identity)?;
    let n = c.peers.values().filter(|cid| *cid == id).count() + usize::from(c.mine == *id);
    (n >= 2).then(|| id.clone())
}
fn corner_factor_in(c: &Corners, identity: &str, category: &str) -> f64 {
    let mine = my_active_corner(c);
    let theirs = active_corner_of(c, identity);
    let mut f = 1.0;
    if mine.is_some() && theirs != mine {
        f = CORNER_OUTSIDE_GAIN; // everyone outside my corner is background noise
    }
    if category == "mic" && theirs.is_some() && theirs != mine {
        f = 0.0; // a corner's mics are private to it
    }
    f
}
/// The playback factor a corner imposes on `identity`'s `category` here.
pub fn corner_factor(identity: &str, category: &str) -> f64 {
    corner_factor_in(&corners().lock().unwrap(), identity, category)
}
/// What a track plays at here: the user's saved pref × the corner factor.
fn effective_volume(identity: &str, category: &str) -> f64 {
    let pref = desired_pref_for(identity, category).map(|p| p.0).unwrap_or(1.0);
    pref * corner_factor(identity, category)
}
/// A participant is (or was found) in the call — pull targets resolve against this.
fn corner_add_participant(identity: &str) {
    if !identity.is_empty() {
        corners().lock().unwrap().participants.insert(identity.to_string());
    }
}
/// `target` is a full identity or a bare mxid; resolve it to someone in the call.
fn resolve_corner_target(c: &Corners, target: &str) -> Option<String> {
    if c.participants.contains(target) {
        return Some(target.to_string());
    }
    let prefix = format!("{target}:");
    let mut v: Vec<&String> = c.participants.iter().filter(|p| p.starts_with(&prefix)).collect();
    v.sort();
    v.first().map(|s| s.to_string())
}
/// Re-apply every registered track's volume (pref × factor) and run the game-chat swap on
/// the active edge. Logs what changed, once per (identity, category).
fn reconcile_corners() {
    let (suspend, restore) = {
        let mut c = corners().lock().unwrap();
        let now_active = my_active_corner(&c).is_some();
        let mut suspend = false;
        let mut restore = false;
        if now_active && !c.was_active {
            if local_game_chat() {
                c.resume_game_chat = true;
                suspend = true;
            }
        } else if !now_active && c.was_active {
            // The others left (or hung up): the corner is over — end it now, no grace (the
            // grace is for a pull that has not been answered yet).
            if !c.mine.is_empty() {
                eprintln!("[call] corner: the others left {} — back with everyone", c.mine);
                c.mine.clear();
                c.pending.push((String::new(), String::new()));
            }
            if c.resume_game_chat {
                c.resume_game_chat = false;
                restore = true;
            }
        }
        c.was_active = now_active;
        if !c.mine.is_empty() && !now_active {
            if c.alone_since.is_none() {
                c.alone_since = Some(std::time::Instant::now());
            }
        } else {
            c.alone_since = None;
        }
        (suspend, restore)
    };
    if suspend {
        eprintln!("[call] corner: game chat suspended while aside");
        set_local_game_chat(false);
        corners().lock().unwrap().game_chat_announce_due = true;
    }
    if restore {
        eprintln!("[call] corner: back with everyone — game chat on again");
        set_local_game_chat(true);
        corners().lock().unwrap().game_chat_announce_due = true;
    }
    let entries: Vec<((String, String, String), glib::WeakRef<gst::Element>)> =
        track_gains().lock().unwrap().iter().map(|(k, w)| (k.clone(), w.clone())).collect();
    let mut logged: std::collections::HashSet<(String, String)> = std::collections::HashSet::new();
    for ((id, cat, _sid), w) in entries {
        let Some(el) = w.upgrade() else { continue };
        let v = effective_volume(&id, &cat);
        let cur: f64 = el.property("volume");
        if (cur - v).abs() > 1e-6 {
            el.set_property("volume", v);
            if logged.insert((id.clone(), cat.clone())) {
                eprintln!("[call] corner: {id} {cat} plays at {:.0}% here", v * 100.0);
            }
        }
    }
}
/// Pull `target` aside into my corner (a new one if I am in none). Ok(identity) or why not.
pub fn corner_pull(target: &str) -> Result<String, String> {
    let t = {
        let mut c = corners().lock().unwrap();
        let t = resolve_corner_target(&c, target).ok_or_else(|| format!("{target} is not in this call"))?;
        if active_corner_of(&c, &t).is_some() {
            return Err("They are already aside — join them instead".to_string());
        }
        if c.mine.is_empty() {
            c.mine = corner_new_id();
        }
        let id = c.mine.clone();
        c.pending.push((id.clone(), t.clone()));
        c.alone_since = None; // a fresh pull gets its full grace period
        eprintln!("[call] corner: pulling {t} aside into {id}");
        t
    };
    reconcile_corners();
    Ok(t)
}
/// Join the corner `target` is in. Ok(corner id) or why not.
pub fn corner_join(target: &str) -> Result<String, String> {
    let id = {
        let mut c = corners().lock().unwrap();
        let t = resolve_corner_target(&c, target).ok_or_else(|| format!("{target} is not in this call"))?;
        let id = active_corner_of(&c, &t).ok_or_else(|| "They are not aside right now".to_string())?;
        if c.mine != id {
            c.mine = id.clone();
            c.pending.push((id.clone(), String::new()));
            eprintln!("[call] corner: joining {id} with {t}");
        }
        id
    };
    reconcile_corners();
    Ok(id)
}
/// Leave my corner (the yellow button; also the alone-grace and the game-chat guard).
pub fn corner_leave() {
    {
        let mut c = corners().lock().unwrap();
        if c.mine.is_empty() {
            return;
        }
        eprintln!("[call] corner: leaving {}", c.mine);
        c.mine.clear();
        c.alone_since = None;
        c.pending.push((String::new(), String::new()));
    }
    reconcile_corners();
}
/// Am I in a corner (pending or active)?
pub fn corner_mine() -> String {
    corners().lock().unwrap().mine.clone()
}
fn handle_corner_announcement(who: &str, id: &str, pull: &str) {
    if who.is_empty() {
        return;
    }
    let me = local_identity_cell().lock().unwrap().clone();
    {
        let mut c = corners().lock().unwrap();
        c.participants.insert(who.to_string());
        let changed = if id.is_empty() {
            c.peers.remove(who).is_some()
        } else {
            c.peers.insert(who.to_string(), id.to_string()).as_deref() != Some(id)
        };
        if changed {
            eprintln!("[call] {who}: corner {}", if id.is_empty() { "none" } else { id });
        }
        // A pull addressed to me: take the corner if I am free (already in it = nothing to do).
        if !pull.is_empty() && pull == me && !id.is_empty() {
            if c.mine.is_empty() {
                c.mine = id.to_string();
                c.alone_since = None;
                c.pending.push((id.to_string(), String::new()));
                eprintln!("[call] corner: {who} pulled me aside — joining {id}");
            } else if c.mine != id {
                eprintln!("[call] corner: {who} tried to pull me aside, but I am already in {}", c.mine);
            }
        }
    }
    reconcile_corners();
}
fn forget_corner_peer(identity: &str) {
    {
        let mut c = corners().lock().unwrap();
        c.peers.remove(identity);
        c.participants.remove(identity);
    }
    reconcile_corners();
}
/// The app's mirror tick: drop out of a corner I have been alone in for the grace period.
pub fn corner_tick() {
    let leave = {
        let c = corners().lock().unwrap();
        let grace = std::time::Duration::from_millis(CORNER_ALONE_GRACE_MS.load(std::sync::atomic::Ordering::Relaxed));
        !c.mine.is_empty() && c.alone_since.map_or(false, |t| t.elapsed() >= grace)
    };
    if leave {
        eprintln!("[call] corner: nobody else here — leaving");
        corner_leave();
    }
}
/// Announcements queued since the last drain: (corner id, pull target).
pub fn take_corner_announcements() -> Vec<(String, String)> {
    std::mem::take(&mut corners().lock().unwrap().pending)
}
/// Did a corner swap change my game-chat flag since the last drain (re-announce it)?
pub fn take_game_chat_announce_due() -> bool {
    std::mem::take(&mut corners().lock().unwrap().game_chat_announce_due)
}
/// `{"mine":"<id>","active":bool,"members":[…],"corners":{"<id>":[…identities, me included…]}}`
/// — only ACTIVE corners are listed; `mine` may be set while still waiting for an answer.
pub fn corner_json() -> String {
    let c = corners().lock().unwrap();
    let me = local_identity_cell().lock().unwrap().clone();
    let esc = |s: &str| s.replace(['\\', '"'], "");
    let quote = |v: &[String]| v.iter().map(|s| format!("\"{}\"", esc(s))).collect::<Vec<_>>().join(",");
    let active = my_active_corner(&c);
    let members = active.as_deref().map(|id| corner_peer_members(&c, id)).unwrap_or_default();
    let mut ids: Vec<String> = c.peers.values().cloned().collect();
    if !c.mine.is_empty() {
        ids.push(c.mine.clone());
    }
    ids.sort();
    ids.dedup();
    let mut out = Vec::new();
    for id in ids {
        let mut m = corner_peer_members(&c, &id);
        if c.mine == id && !me.is_empty() {
            m.push(me.clone());
            m.sort();
        }
        if m.len() >= 2 {
            out.push(format!("\"{}\":[{}]", esc(&id), quote(&m)));
        }
    }
    format!(
        "{{\"mine\":\"{}\",\"active\":{},\"members\":[{}],\"corners\":{{{}}}}}",
        esc(&c.mine),
        active.is_some(),
        quote(&members),
        out.join(",")
    )
}

/// Per-tile in-app visibility (AFK cut, zero-size slot): SFU-pause just this tile.
pub fn set_tile_video_hidden(id: &str, hidden: bool) {
    {
        let mut pol = recv_policy().lock().unwrap();
        let changed = if hidden { pol.hidden.insert(id.to_string()) } else { pol.hidden.remove(id) };
        if !changed {
            return;
        }
    }
    // Log EVERY changed transition: this pause is process-global and survives delegate/
    // pipeline rebuilds, so an unmatched "hidden" here is a remote video that stays
    // frozen/black until something re-fires the shown edge. A long silent outage of
    // exactly that shape (room-nav delegate teardown paused the tiles; the recreated
    // delegates were born visible and never emitted the unhide) once left ZERO log
    // trace — never again.
    eprintln!(
        "[recv-adapt] tile {id} -> {} at the SFU",
        if hidden { "hidden (paused)" } else { "shown (resuming)" }
    );
    spawn_recv_reconcile(Some(id.to_string()));
}

/// Manual per-tile quality pick (the hover dropdown): mode is "auto" (clear the
/// override) or "180"/"360"/"720"/"1080". Takes effect immediately via reconcile.
pub fn set_tile_quality(id: &str, mode: &str) {
    {
        let mut pol = recv_policy().lock().unwrap();
        let changed = if mode == "auto" || override_dims(mode).is_none() {
            pol.quality_overrides.remove(id).is_some()
        } else {
            pol.quality_overrides.insert(id.to_string(), mode.to_string())
                != Some(mode.to_string())
        };
        if !changed {
            return;
        }
    }
    eprintln!("[recv] tile {id} quality -> {mode}");
    spawn_recv_reconcile(Some(id.to_string()));
}

/// The current manual pick for a tile ("auto" when none) — for the dropdown checkmark.
pub fn tile_quality(id: &str) -> String {
    recv_policy()
        .lock()
        .unwrap()
        .quality_overrides
        .get(id)
        .cloned()
        .unwrap_or_else(|| "auto".to_string())
}

/// Per-tile rendered size (physical px): ask the SFU for the closest layer. Quantized to
/// layer buckets; only re-signals when the bucket changes. Only meaningful in "auto".
pub fn set_tile_video_size(id: &str, w: u32, h: u32) {
    if w == 0 || h == 0 {
        return;
    }
    let q = quantize_dims(w, h);
    {
        let mut pol = recv_policy().lock().unwrap();
        if pol.dims.get(id) == Some(&q) {
            return;
        }
        pol.dims.insert(id.to_string(), q);
    }
    if recv_quality() == "auto" {
        spawn_recv_reconcile(Some(id.to_string()));
    }
}

// ── Call moderation (RBAC, docs/RBAC.md §5.4) ─────────────────────────────────────────
// A moderator (role permission `moderate_calls`) can mute someone's mic for everyone or
// disconnect them from the call. ENFORCEMENT is the Tin Hat server (its
// `POST /api/moderation/call`) acting on the SFU with its admin key: it mutes the target's
// audio tracks and takes the MICROPHONE source out of their publish permission (soundboard
// one-shots ride TrackSource::Unknown, so they go with it), or removes the participant.
// This block is the CLIENT side: a `tinhat.moderation` data packet tells everyone who did what
// to whom (the SFU only ever says "your permission changed"), the local permission handler
// keeps our own mic + soundboard honest, and `server_muted_json` feeds the tile shields.
pub const MODERATION_TOPIC: &str = "tinhat.moderation";

/// A `tinhat.moderation` packet: `{"action":"mute"|"unmute"|"disconnect","target":"<identity>",
/// "by":"<identity>","name":"<display name of by>"}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModerationPacket {
    pub action: String,
    pub target: String,
    pub by: String,
    pub name: String,
}

pub(crate) fn moderation_packet_json(action: &str, target: &str, by: &str, name: &str) -> String {
    serde_json::json!({ "action": action, "target": target, "by": by, "name": name }).to_string()
}

/// Parse a packet; `None` unless it carries a known action and a target.
pub(crate) fn parse_moderation_packet(text: &str) -> Option<ModerationPacket> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let p = ModerationPacket { action: s("action"), target: s("target"), by: s("by"), name: s("name") };
    if p.target.is_empty() || !matches!(p.action.as_str(), "mute" | "unmute" | "disconnect") {
        return None;
    }
    Some(p)
}

fn moderation_data_packet(json: &str, destination: Vec<String>) -> livekit::DataPacket {
    livekit::DataPacket {
        payload: json.as_bytes().to_vec(),
        topic: Some(MODERATION_TOPIC.to_string()),
        reliable: true,
        destination_identities: destination.into_iter().map(ParticipantIdentity).collect(),
    }
}

/// proto `TrackSource` values (livekit_protocol) — what the SFU's `can_publish_sources` lists.
pub const SRC_UNKNOWN: i32 = 0;
pub const SRC_CAMERA: i32 = 1;
pub const SRC_MICROPHONE: i32 = 2;
pub const SRC_SCREEN_SHARE: i32 = 3;
pub const SRC_SCREEN_SHARE_AUDIO: i32 = 4;

fn proto_source(src: TrackSource) -> i32 {
    match src {
        TrackSource::Camera => SRC_CAMERA,
        TrackSource::Microphone => SRC_MICROPHONE,
        TrackSource::Screenshare => SRC_SCREEN_SHARE,
        TrackSource::ScreenshareAudio => SRC_SCREEN_SHARE_AUDIO,
        _ => SRC_UNKNOWN,
    }
}

/// May a participant with this permission publish `source`? An empty source list means
/// "every source" (LiveKit's default); `can_publish=false` blocks everything.
pub fn source_allowed(can_publish: bool, sources: &[i32], source: i32) -> bool {
    can_publish && (sources.is_empty() || sources.contains(&source))
}

/// The one question the mic pipeline asks of a permission update.
pub fn mic_allowed(can_publish: bool, sources: &[i32]) -> bool {
    source_allowed(can_publish, sources, SRC_MICROPHONE)
}

#[derive(Default)]
struct Moderation {
    /// identity -> (moderator identity, moderator display name) for everyone server-muted
    /// (me included). Fed by packets (peers), by my own sends (the SFU never echoes a data
    /// packet to its sender), by the join-time state read, and by my permission updates.
    muted: HashMap<String, (String, String)>,
    /// I am server-muted: a packet / the room state / the SFU said so. Locks the mic button
    /// and refuses soundboard publishes.
    me_muted: bool,
    /// The SFU itself has taken my mic source away (as opposed to a packet/state-only mute).
    /// Only a permission update that restores the mic clears a mute the SFU applied; a
    /// permission that merely says "everything allowed" must NOT clear a state-based mute
    /// (a fresh session's permission is always the full set — the moderator re-enforces).
    sfu_muted: bool,
    /// My current publish permission per the SFU: (can_publish, can_publish_sources).
    /// None = never told (everything allowed).
    my_permission: Option<(bool, Vec<i32>)>,
    /// The last `disconnect` packet addressed to me: (by, name, when) — consumed by the
    /// `Disconnected{ParticipantRemoved}` that follows it.
    removal_notice: Option<(String, String, std::time::Instant)>,
}
fn moderation() -> &'static Mutex<Moderation> {
    static M: OnceLock<Mutex<Moderation>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(Moderation::default()))
}
fn forget_all_moderation() {
    *moderation().lock().unwrap() = Moderation::default();
}
/// A clone of the event channel's sender, so out-of-loop paths (my own moderation sends)
/// can raise `CallEvent`s. Set on connect, cleared on close.
fn event_sender() -> &'static Mutex<Option<tokio::sync::mpsc::UnboundedSender<CallEvent>>> {
    static S: OnceLock<Mutex<Option<tokio::sync::mpsc::UnboundedSender<CallEvent>>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(None))
}
fn emit_call_event(ev: CallEvent) {
    if let Some(tx) = event_sender().lock().unwrap().as_ref() {
        let _ = tx.send(ev);
    }
}

/// Am I server-muted right now?
pub fn i_am_server_muted() -> bool {
    moderation().lock().unwrap().me_muted
}
/// `(by identity, by display name)` for a server-muted identity, if any.
pub fn server_mute_info(identity: &str) -> Option<(String, String)> {
    moderation().lock().unwrap().muted.get(identity).cloned()
}
/// `{"<identity>":{"by":"<identity>","name":"<name>"}}` — tile enrichment.
pub fn server_muted_json() -> String {
    let m = moderation().lock().unwrap();
    let mut map = serde_json::Map::new();
    for (id, (by, name)) in m.muted.iter() {
        map.insert(id.clone(), serde_json::json!({ "by": by, "name": name }));
    }
    serde_json::Value::Object(map).to_string()
}
/// Is this source publishable under my CURRENT permission (per the SFU)?
pub fn my_source_allowed(source: TrackSource) -> bool {
    match &moderation().lock().unwrap().my_permission {
        Some((cp, srcs)) => source_allowed(*cp, srcs, proto_source(source)),
        None => true,
    }
}
/// A full identity or a bare mxid -> someone in the call (bare = first device, sorted).
pub fn resolve_call_participant(target: &str) -> Option<String> {
    resolve_corner_target(&corners().lock().unwrap(), target)
}

/// Join-time: the call room's `dev.tinhat.call.moderation` state says a moderator muted me
/// earlier (the app read it; this NEW session's SFU permission knows nothing yet). Mark me
/// muted so the mic button locks and the soundboard refuses, until an `unmute` packet or a
/// permission update that restores the mic clears it. Call AFTER connect (identity known).
pub fn set_server_muted_from_state(by: &str, by_name: &str) {
    let me = local_identity_cell().lock().unwrap().clone();
    let mut m = moderation().lock().unwrap();
    m.me_muted = true;
    if !me.is_empty() {
        m.muted.insert(me, (by.to_string(), by_name.to_string()));
    }
    eprintln!("[call] moderation: joined server-muted per room state (by {by:?})");
}

/// Mute our published mic through its PUBLICATION (signals the SFU, like set_mic_muted).
/// Used when a server mute lands: the SFU has already muted the track on its side; this
/// stops us pushing frames into a track nobody will ever be forwarded.
fn mute_local_mic_publication() {
    let Some(lp) = aux_participant().lock().unwrap().clone() else { return };
    for (_sid, p) in lp.track_publications() {
        if matches!(p.source(), TrackSource::Microphone) && !p.is_muted() {
            p.mute();
            eprintln!("[call] moderation: mic publication muted (server mute)");
        }
    }
}
/// Tear down every live 2nd audio track (soundboard "sound" / "music") whose source the SFU
/// no longer lets us publish. Soundboard one-shots are TrackSource::Unknown — a server mute
/// always takes that away; music rides ScreenshareAudio and only stops if the SFU says so.
fn stop_disallowed_aux(force_sound: bool) {
    let live: Vec<&'static str> = aux_tracks().lock().unwrap().keys().copied().collect();
    for slot in live {
        let src = if slot == AuxKind::Music.track_name() {
            TrackSource::ScreenshareAudio
        } else {
            TrackSource::Unknown
        };
        let blocked = !my_source_allowed(src) || (force_sound && matches!(src, TrackSource::Unknown));
        if blocked {
            eprintln!("[aux/{slot}] stopped — a moderator muted us for everyone");
            teardown_aux(slot);
        }
    }
}

/// A `tinhat.moderation` packet arrived. The SENDER is the moderator whatever the packet
/// claims (`by` in the JSON is informational; the SFU's participant identity is not
/// spoofable). Runs on the event loop (in-runtime).
fn handle_moderation_packet(who: &str, pkt: ModerationPacket, tx: &tokio::sync::mpsc::UnboundedSender<CallEvent>) {
    if who.is_empty() {
        return;
    }
    let by = who.to_string();
    let name = pkt.name;
    let me = local_identity_cell().lock().unwrap().clone();
    let for_me = !me.is_empty() && pkt.target == me;
    eprintln!(
        "[call] moderation: {who} -> {} {}{}",
        pkt.action,
        pkt.target,
        if for_me { " (me)" } else { "" }
    );
    match pkt.action.as_str() {
        "mute" => {
            {
                let mut m = moderation().lock().unwrap();
                m.muted.insert(pkt.target.clone(), (by.clone(), name.clone()));
                if for_me {
                    m.me_muted = true;
                }
            }
            if for_me {
                // The server acted BEFORE this packet was broadcast (2xx first): honour it now
                // even if the SFU's permission update is still in flight.
                mute_local_mic_publication();
                stop_disallowed_aux(true);
            }
            let _ = tx.send(CallEvent::ServerMuteChanged { identity: pkt.target, muted: true, by, by_name: name });
        }
        "unmute" => {
            {
                let mut m = moderation().lock().unwrap();
                m.muted.remove(&pkt.target);
                if for_me {
                    m.me_muted = false;
                    m.sfu_muted = false;
                }
            }
            let _ = tx.send(CallEvent::ServerMuteChanged { identity: pkt.target, muted: false, by, by_name: name });
        }
        "disconnect" => {
            if for_me {
                moderation().lock().unwrap().removal_notice = Some((by, name, std::time::Instant::now()));
            }
        }
        _ => {}
    }
}

/// My own publish permission changed at the SFU (the server-side mute/unmute lands here).
/// `mic_allowed` false → stop the mic + whatever 2nd tracks are now blocked, mark me muted;
/// true (after an SFU mute) → clear, and the app republishes the mic unless the user had
/// muted themselves (Discord behaviour).
fn handle_my_permission_changed(
    me: &str,
    can_publish: bool,
    sources: Vec<i32>,
    tx: &tokio::sync::mpsc::UnboundedSender<CallEvent>,
) {
    let allowed = mic_allowed(can_publish, &sources);
    eprintln!(
        "[call] moderation: my publish permission changed — can_publish={can_publish} sources={sources:?} mic_allowed={allowed}"
    );
    let (flipped, by, name) = {
        let mut m = moderation().lock().unwrap();
        m.my_permission = Some((can_publish, sources));
        if !allowed {
            m.sfu_muted = true;
            if !m.me_muted {
                m.me_muted = true;
                let e = m.muted.entry(me.to_string()).or_insert_with(|| (String::new(), String::new())).clone();
                (true, e.0, e.1)
            } else {
                (false, String::new(), String::new())
            }
        } else if m.sfu_muted {
            m.sfu_muted = false;
            if m.me_muted {
                m.me_muted = false;
                let e = m.muted.remove(me).unwrap_or_default();
                (true, e.0, e.1)
            } else {
                (false, String::new(), String::new())
            }
        } else {
            (false, String::new(), String::new())
        }
    };
    if !allowed {
        mute_local_mic_publication();
        stop_disallowed_aux(false);
    }
    if flipped {
        let _ = tx.send(CallEvent::ServerMuteChanged { identity: me.to_string(), muted: !allowed, by, by_name: name });
    }
}

/// The targeted half of a `disconnect`: tell the TARGET alone who is about to remove them.
/// Awaitable — the app sends this BEFORE it asks the server to act (the SFU cuts them off
/// the moment it does; a broadcast after that never reaches them). Nothing has happened
/// yet, so no map / event update here; the receiver only parks a removal notice that the
/// `Disconnected{ParticipantRemoved}` consumes (or that expires unused if the server refused).
pub async fn publish_moderation_notice(target: &str, by_name: &str) {
    let Some(lp) = aux_participant().lock().unwrap().clone() else {
        eprintln!("[call] moderation: not in a call — disconnect notice to {target} not sent");
        return;
    };
    let by = local_identity_cell().lock().unwrap().clone();
    let json = moderation_packet_json("disconnect", target, &by, by_name);
    match lp.publish_data(moderation_data_packet(&json, vec![target.to_string()])).await {
        Ok(()) => eprintln!("[call] moderation: disconnect notice sent to {target}"),
        Err(e) => eprintln!("[call] moderation: disconnect notice to {target} failed: {e}"),
    }
}

/// Broadcast a moderation packet (reliable, to everyone). Awaitable — the app sends it
/// AFTER the server said 2xx (mute/unmute AND disconnect), so a refused request never
/// paints a shield. The sender's own map is updated here (the SFU never echoes a data
/// packet back to its sender), so the moderator's tiles show the shield too.
pub async fn publish_moderation_packet(action: &str, target: &str, by_name: &str) {
    let Some(lp) = aux_participant().lock().unwrap().clone() else {
        eprintln!("[call] moderation: not in a call — {action} {target} not announced");
        return;
    };
    let by = local_identity_cell().lock().unwrap().clone();
    let json = moderation_packet_json(action, target, &by, by_name);
    match lp.publish_data(moderation_data_packet(&json, Vec::new())).await {
        Ok(()) => eprintln!("[call] moderation: announced {action} {target}"),
        Err(e) => eprintln!("[call] moderation: announce {action} {target} failed: {e}"),
    }
    match action {
        "mute" => {
            moderation().lock().unwrap().muted.insert(target.to_string(), (by.clone(), by_name.to_string()));
            emit_call_event(CallEvent::ServerMuteChanged {
                identity: target.to_string(),
                muted: true,
                by,
                by_name: by_name.to_string(),
            });
        }
        "unmute" => {
            moderation().lock().unwrap().muted.remove(target);
            emit_call_event(CallEvent::ServerMuteChanged {
                identity: target.to_string(),
                muted: false,
                by,
                by_name: by_name.to_string(),
            });
        }
        _ => {}
    }
}
/// Fire-and-forget from any thread (Qt GUI thread included): `disconnect` goes to the
/// target alone FIRST ([`publish_moderation_notice`]), then the broadcast
/// ([`publish_moderation_packet`]). The app's own flow awaits the two halves around its
/// server request instead; this is the one-shot convenience per docs/RBAC.md §5.4.
pub fn send_moderation_packet(action: &str, target: &str, by_name: &str) {
    let (action, target, by_name) = (action.to_string(), target.to_string(), by_name.to_string());
    let rt = aux_runtime().lock().unwrap().clone();
    match rt {
        Some(rt) => {
            rt.spawn(async move {
                if action == "disconnect" {
                    publish_moderation_notice(&target, &by_name).await;
                }
                publish_moderation_packet(&action, &target, &by_name).await
            });
        }
        None => eprintln!("[call] moderation: no runtime handle — {action} {target} not announced"),
    }
}

/// Clear the shared "in a call" globals after an ABANDONED connect: do_join's 15s timeout
/// drops the half-built session without close(), but connect() has often already set
/// aux_participant/aux_runtime/active_key_provider — leaving the soundboard's "in a call"
/// checks passing against a dead room until the next successful join.
pub fn abandon_partial_connect() {
    crate::hear::session_end(); // 10.5: an abandoned join's analyzer stops with it
    crate::callstats::session_end(); // widgets: the call-stats sampler too
    mic_wanted_flag().store(false, std::sync::atomic::Ordering::Relaxed);
    *aux_participant().lock().unwrap() = None;
    *event_sender().lock().unwrap() = None;
    forget_all_moderation();
    *aux_runtime().lock().unwrap() = None;
    *active_key_provider().lock().unwrap() = None;
    clear_recv_state();
    republished_sids().lock().unwrap().clear(); // nothing published — safe to drop now
    // connect() brings AEC up BEFORE Room::connect; when do_join's 15s timeout DROPS the
    // connect future mid-await, connect()'s own failure-path aec_stop never runs. Without
    // this, the echo-cancel module (and aec_nodes = Some) outlives the abandoned join —
    // EC devices linger in the user's device list and set_playback_target refuses live
    // re-points outside any call. Idempotent (double-stop on other paths is a no-op).
    aec_stop();
}

/// Drop all per-call receive state (registry + transient policy; the quality mode is a
/// persisted user setting and survives).
fn clear_recv_state() {
    remote_video_pubs().lock().unwrap().clear();
    missing_key_since().lock().unwrap().clear(); // per-call episodes; never carry across
    // NOTE: republished_sids is deliberately NOT cleared here — close() calls this in its
    // synchronous Phase 1, but Phase 2's unpublishes still need live_sid() to resolve
    // reconnect-reissued sids. close() clears it itself after Phase 2.
    let mut pol = recv_policy().lock().unwrap();
    pol.window_paused = false;
    pol.hidden.clear();
    pol.dims.clear();
    pol.quality_overrides.clear(); // manual picks are per-call, like hides
}

/// Tear down (NULL the pipeline, abort the feeder, stop the bus-watch thread) + unpublish the
/// 2nd-track in `slot`, if any. Idempotent + thread-safe; safe to call from ANY thread — the
/// async unpublish is scheduled via the runtime `Handle` captured on connect (NOT a bare
/// `tokio::spawn`, which would panic from the bus-watch std::thread or the Qt GUI thread).
/// Never touches the mic track.
fn teardown_aux(slot: &'static str) {
    let handle = aux_tracks().lock().unwrap().remove(slot);
    if let Some(h) = handle {
        retire_aux(slot, h, true);
    }
}

/// Stop + unpublish one aux publish (see [`teardown_aux`]). `current`: it was the slot's live track, so its sid goes
/// through the reconnect remap ([`live_sid`]); a displaced handle (10.8, [`store_aux`]) is unpublished by its own sid.
fn retire_aux(slot: &'static str, h: AuxAudioHandle, current: bool) {
    // Signal the bus-watch thread to exit BEFORE NULLing (NULL posts no EOS, so the
    // watcher would otherwise block on timed_pop forever — one leaked thread per stop).
    h.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    set_state_bounded(h.pipeline.upcast_ref(), gst::State::Null, 1500, "share-audio capture");
    h.task.abort();
    let participant = aux_participant().lock().unwrap().clone();
    if let Some(p) = participant {
        // Unpublish off-thread (it's async). Use the captured runtime Handle so this is
        // safe from the bus-watch std::thread / Qt GUI thread (which have no runtime in
        // their TLS); a bare tokio::spawn would panic and leave the track wedged.
        let rt = aux_runtime().lock().unwrap().clone();
        if let Some(rt) = rt {
            let sid = if current { live_sid(slot, &h.sid) } else { h.sid.clone() };
            rt.spawn(async move {
                // Bounded like every other network teardown op — a dead SFU otherwise
                // accumulates one hung detached task per soundboard stop/replace.
                let _ = tokio::time::timeout(std::time::Duration::from_secs(3), p.unpublish_track(&sid)).await;
                eprintln!("[aux/{slot}] unpublished");
            });
        } else {
            eprintln!("[aux/{slot}] no runtime handle — track left published (should not happen)");
        }
    }
}

/// Store a fresh aux publish in its slot. 10.8 (leak track): two plays that overlapped the publish await (the soundboard,
/// the join sound and the music share all spawn one per click) used to make the second `insert` silently DROP the first
/// handle — its pipeline kept playing into a track that stayed published for the rest of the call (encoder + two
/// libwebrtc threads, a bus thread, the feeder), and its EOS later tore down the newer sound. Publishes of a slot are
/// serialized now ([`aux_publish_gate`]); a handle that is displaced anyway is retired properly.
fn store_aux(slot: &'static str, h: AuxAudioHandle) {
    let displaced = aux_tracks().lock().unwrap().insert(slot, h);
    if let Some(old) = displaced {
        eprintln!("[aux/{slot}] a newer publish displaced a live one — retiring the older track");
        retire_aux(slot, old, false);
    }
}

/// One publish at a time per aux slot (10.8, see [`store_aux`]): held across teardown → publish → store.
fn aux_publish_gate(slot: &'static str) -> Arc<tokio::sync::Mutex<()>> {
    static GATES: OnceLock<Mutex<HashMap<&'static str, Arc<tokio::sync::Mutex<()>>>>> = OnceLock::new();
    GATES.get_or_init(Default::default).lock().unwrap().entry(slot).or_default().clone()
}

/// Publish a fresh 2nd audio track for `kind` from `url` (stop-then-replace any existing one
/// in that slot first). Creates a SEPARATE NativeAudioSource + LocalAudioTrack
/// (TrackSource::Unknown, dtx:false), publishes it into the active Room (riding the existing
/// E2EE key), and starts the fetch/decode pipeline. The mic track + its source are untouched.
/// NO-OP error if not in a call. Idempotent per slot.
async fn publish_aux(kind: AuxKind, url: &str) -> Result<()> {
    // Receivers key their per-user gain on the publication SOURCE
    // (category_for_source): ScreenshareAudio -> "screenshare", which is what the
    // left bar's "Stream" dial and the share tile's slider drive. Every desktop-audio
    // share used to go out as Unknown -> "music", so those two controls were no-ops
    // and only the "Soundboard" dial moved a stream. Soundboard one-shots stay
    // Unknown on purpose (that IS the soundboard dial).
    let aux_source = match kind {
        AuxKind::Music => TrackSource::ScreenshareAudio,
        AuxKind::Sound => TrackSource::Unknown,
    };
    // RBAC: a server-muted participant can't play sounds (the SFU rejects Unknown-source
    // tracks after a server mute; music rides ScreenshareAudio and is only refused if the
    // SFU took that away too). Say so instead of a NotAllowed from the SFU.
    if i_am_server_muted() && (matches!(aux_source, TrackSource::Unknown) || !my_source_allowed(aux_source)) {
        anyhow::bail!("A moderator muted you for everyone — sounds are off until you're unmuted");
    }
    let slot = kind.track_name();
    let gate = aux_publish_gate(slot);
    let _one_at_a_time = gate.lock().await;
    // Stop-then-play: drop any current publish in this slot before starting a new one.
    teardown_aux(slot);

    let participant = aux_participant()
        .lock()
        .unwrap()
        .clone()
        .context("not in a call (no active participant for 2nd audio track)")?;

    // A DEDICATED source/track — never the mic's. STEREO (music/game audio keeps its
    // imaging; the mic stays mono). 1s queue (re-chunked downstream like mic).
    let source = NativeAudioSource::new(
        AudioSourceOptions { echo_cancellation: false, noise_suppression: false, auto_gain_control: false },
        SAMPLE_RATE,
        2,
        1000,
    );
    let track = LocalAudioTrack::create_audio_track(slot, RtcAudioSource::Native(source.clone()));
    let publication = participant
        .publish_track(
            LocalTrack::Audio(track),
            TrackPublishOptions {
                // Music -> ScreenshareAudio, Sound -> Unknown (see aux_source above);
                // dtx off so steady music/sound isn't gated by discontinuous-transmission.
                source: aux_source,
                dtx: false,
                // 128 kbps Opus (MUSIC_HIGH_QUALITY_STEREO) — the SDK default caps
                // aux audio at 48 kbps, audibly below Opus transparency for music.
                audio_encoding: Some(AudioEncoding { max_bitrate: 128_000 }),
                ..Default::default()
            },
        )
        .await
        .context("publish 2nd audio track failed")?;
    let sid = publication.sid();

    let (pipeline, task, stop) =
        match start_aux_capture(url, slot, source.clone(), move || teardown_aux(slot)) {
            Ok(v) => v,
            Err(e) => {
                // Don't strand a negotiated-but-silent aux track on peers (see
                // unpublish_orphaned_track — same failure shape, free-function context).
                eprintln!("[aux/{slot}] capture failed after publish — unpublishing");
                let _ = tokio::time::timeout(
                    std::time::Duration::from_secs(3),
                    participant.unpublish_track(&sid),
                )
                .await;
                return Err(e);
            }
        };
    eprintln!("[aux/{slot}] published + playing {url}");
    clear_republished_sid(slot); // fresh publish: the cached sid is current again
    store_aux(slot, AuxAudioHandle { pipeline, task, source, sid, stop });
    Ok(())
}

/// #1 MUSIC: publish a fresh 2nd audio track for `kind` captured from a PipeWire SOURCE
/// `node` (stop-then-replace any existing one in that slot first). Identical publish/track
/// plumbing to [`publish_aux`] (a SEPARATE NativeAudioSource + LocalAudioTrack, TrackSource
/// ::Unknown, riding the existing E2EE key) — only the capture pipeline differs: a
/// `pipewiresrc` on `node` instead of `uridecodebin` on a URL. The mic track is untouched.
/// NO-OP error if not in a call. Idempotent per slot.
async fn publish_aux_pipewire(kind: AuxKind, node: &str) -> Result<()> {
    let slot = kind.track_name();
    let gate = aux_publish_gate(slot);
    let _one_at_a_time = gate.lock().await;
    teardown_aux(slot);

    let participant = aux_participant()
        .lock()
        .unwrap()
        .clone()
        .context("not in a call (no active participant for 2nd audio track)")?;

    let source = NativeAudioSource::new(
        AudioSourceOptions { echo_cancellation: false, noise_suppression: false, auto_gain_control: false },
        SAMPLE_RATE,
        2,
        1000,
    );
    let track = LocalAudioTrack::create_audio_track(slot, RtcAudioSource::Native(source.clone()));
    let publication = participant
        .publish_track(
            LocalTrack::Audio(track),
            TrackPublishOptions {
                // This IS the desktop/stream audio share — publish it as
                // ScreenshareAudio so receivers file it under their "Stream" dial.
                source: TrackSource::ScreenshareAudio,
                dtx: false,
                // 128 kbps Opus (MUSIC_HIGH_QUALITY_STEREO); SDK default is 48 kbps.
                audio_encoding: Some(AudioEncoding { max_bitrate: 128_000 }),
                ..Default::default()
            },
        )
        .await
        .context("publish music source 2nd audio track failed")?;
    let sid = publication.sid();

    let node_owned = node.to_string();
    let (pipeline, task, stop) =
        match start_aux_pipewire_capture(&node_owned, slot, source.clone(), move || {
            teardown_aux(slot)
        }) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[aux/{slot}] capture failed after publish — unpublishing");
                let _ = tokio::time::timeout(
                    std::time::Duration::from_secs(3),
                    participant.unpublish_track(&sid),
                )
                .await;
                return Err(e);
            }
        };
    eprintln!("[aux/{slot}] published + capturing PipeWire source {node}");
    clear_republished_sid(slot); // fresh publish: the cached sid is current again
    store_aux(slot, AuxAudioHandle { pipeline, task, source, sid, stop });
    Ok(())
}

/// Build the music capture pipeline string for a PipeWire SOURCE `node`:
/// `pipewiresrc target-object="node" -> audioconvert -> audioresample ->
/// S16LE/48k/stereo -> appsink`. Stereo (unlike the mono mic) so shared game audio /
/// music keeps its imaging; otherwise the `build_aux_audio_pipeline` tail, just from a
/// live source node (e.g. a sink `.monitor`) instead of `uridecodebin`. Returns the
/// pipeline + its named `appsink`.
fn build_aux_pipewire_desc(node: &str) -> String {
    let src = audio_src_fragment(Some(node));
    format!(
        "{src} ! audioconvert ! audioresample ! \
         audio/x-raw,format=S16LE,rate=48000,channels=2,layout=interleaved ! \
         queue leaky=downstream max-size-buffers=16 ! \
         appsink name=aux emit-signals=false max-buffers=8 drop=true sync=false"
    )
}

fn build_aux_pipewire_pipeline(node: &str) -> Result<(gst::Pipeline, gstreamer_app::AppSink)> {
    let desc = build_aux_pipewire_desc(node);
    let pipeline = gst::parse::launch(&desc)?
        .downcast::<gst::Pipeline>()
        .map_err(|_| anyhow::anyhow!("aux pipewire: not a Pipeline"))?;
    let appsink = pipeline
        .by_name("aux")
        .context("aux pipewire appsink missing")?
        .downcast::<gstreamer_app::AppSink>()
        .map_err(|_| anyhow::anyhow!("aux pipewire sink not an AppSink"))?;
    Ok((pipeline, appsink))
}

/// Start the music PipeWire-source capture feeding `source` (same push loop + bus watch as
/// [`start_aux_capture`], just a live `pipewiresrc` head). A live source never reaches EOS,
/// so `on_done` fires only on a pipeline ERROR (or an explicit teardown via the `stop` flag).
fn start_aux_pipewire_capture(
    node: &str,
    label: &str,
    source: NativeAudioSource,
    on_done: impl Fn() + Send + 'static,
) -> Result<(gst::Pipeline, tokio::task::JoinHandle<()>, Arc<std::sync::atomic::AtomicBool>)> {
    let (pipeline, appsink) = build_aux_pipewire_pipeline(node)?;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Vec<i16>>();
    appsink.set_callbacks(
        gstreamer_app::AppSinkCallbacks::builder()
            .new_sample(move |sink| {
                let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                let buffer = sample.buffer().ok_or(gst::FlowError::Error)?;
                let map = buffer.map_readable().map_err(|_| gst::FlowError::Error)?;
                let samples: Vec<i16> = map
                    .as_slice()
                    .chunks_exact(2)
                    .map(|b| i16::from_le_bytes([b[0], b[1]]))
                    .collect();
                let _ = tx.send(samples);
                Ok(gst::FlowSuccess::Ok)
            })
            .build(),
    );
    let task = tokio::spawn(async move {
        while let Some(mut samples) = rx.recv().await {
            if samples.len() < 2 {
                continue;
            }
            if samples.len() % 2 != 0 {
                samples.pop(); // never let a stray sample flip the L/R interleave phase
            }
            let spc = (samples.len() / 2) as u32; // interleaved stereo
            let frame = AudioFrame {
                data: Cow::Owned(samples),
                sample_rate: SAMPLE_RATE,
                num_channels: 2,
                samples_per_channel: spc,
            };
            let _ = source.capture_frame(&frame).await;
        }
    });

    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    if let Some(bus) = pipeline.bus() {
        let label = label.to_string();
        let stop = stop.clone();
        std::thread::spawn(move || {
            use gst::MessageView;
            loop {
                if stop.load(std::sync::atomic::Ordering::Relaxed) {
                    break;
                }
                let Some(msg) = bus.timed_pop(gst::ClockTime::from_mseconds(500)) else { continue };
                match msg.view() {
                    MessageView::Eos(_) => {
                        eprintln!("[aux/{label}] source EOS — finished");
                        on_done();
                        break;
                    }
                    MessageView::Error(e) => {
                        eprintln!("[aux/{label}] source gst ERROR: {} ({:?})", e.error(), e.debug());
                        on_done();
                        break;
                    }
                    _ => {}
                }
            }
        });
    }

    if !set_state_bounded(pipeline.upcast_ref(), gst::State::Playing, 6000, "share-audio capture start") {
        // Unblock the bus-watch thread spawned above and NULL the half-started pipeline —
        // returning bare Err leaked one OS thread spinning on timed_pop per failed start.
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        task.abort();
        set_state_bounded(pipeline.upcast_ref(), gst::State::Null, 1500, "share-audio capture");
        return Err(anyhow::anyhow!("aux pipewire pipeline start failed (the source never started)"));
    }
    Ok((pipeline, task, stop))
}

/// Build the 2nd-track FETCH+DECODE pipeline: `uridecodebin` (HTTP/file -> decoded PCM) ->
/// audioconvert -> audioresample -> caps S16LE/48k/stereo -> appsink. `uridecodebin`'s src
/// pad is dynamic (appears once the media type is known), so it's linked in a pad-added
/// callback to the conversion head. Stereo (unlike the mono mic) so music keeps its
/// imaging; the same push loop feeds a NativeAudioSource. Returns the pipeline + its appsink.
fn build_aux_audio_pipeline(url: &str, label: &str) -> Result<(gst::Pipeline, gstreamer_app::AppSink)> {
    // Built element-by-element (not parse::launch) because uridecodebin has NO static
    // sink — its decoded src pad is created at runtime and must be linked then.
    let pipeline = gst::Pipeline::with_name(&format!("aux_{label}"));
    let src = gst::ElementFactory::make("uridecodebin")
        .property("uri", url)
        .build()
        .context("aux: uridecodebin")?;
    let convert = gst::ElementFactory::make("audioconvert").build().context("aux: audioconvert")?;
    let resample = gst::ElementFactory::make("audioresample").build().context("aux: audioresample")?;
    let capsfilter = gst::ElementFactory::make("capsfilter")
        .property(
            "caps",
            &gst::Caps::builder("audio/x-raw")
                .field("format", "S16LE")
                .field("rate", 48_000i32)
                .field("channels", 2i32)
                .field("layout", "interleaved")
                .build(),
        )
        .build()
        .context("aux: capsfilter")?;
    let appsink = gst::ElementFactory::make("appsink")
        .name("aux")
        .property("emit-signals", false)
        .property("max-buffers", 8u32)
        .property("drop", false) // music/sound: don't drop, just back-pressure the 1s source queue
        // sync=TRUE: pace this uridecodebin (file/URL) decode to the pipeline clock, so EOS
        // lands at the sound's REAL end. With sync=false it decoded faster-than-real-time,
        // front-loaded the 1s source queue, and hit EOS — and unpublished the track — before
        // peers finished subscribing, so they never heard it. (Live pipewiresrc/mic sinks
        // stay sync=false; they're already real-time paced and have no EOS.)
        .property("sync", true)
        .build()
        .context("aux: appsink")?;
    pipeline
        .add_many([&src, &convert, &resample, &capsfilter, &appsink])
        .context("aux: add elements")?;
    gst::Element::link_many([&convert, &resample, &capsfilter, &appsink]).context("aux: link tail")?;

    // uridecodebin's decoded src pad is dynamic — link it to the conversion head when it
    // appears, but ONLY if it's audio (a video/x-… pad on an AV file is left unlinked).
    let convert_weak = convert.downgrade();
    src.connect_pad_added(move |_dbin, pad| {
        let Some(convert) = convert_weak.upgrade() else { return };
        let Some(sink_pad) = convert.static_pad("sink") else { return };
        if sink_pad.is_linked() {
            return; // already fed (e.g. a 2nd audio stream) — ignore extras
        }
        let is_audio = pad
            .current_caps()
            .and_then(|c| c.structure(0).map(|s| s.name().starts_with("audio/")))
            .unwrap_or(false);
        if is_audio {
            let _ = pad.link(&sink_pad);
        }
    });

    let appsink = appsink
        .downcast::<gstreamer_app::AppSink>()
        .map_err(|_| anyhow::anyhow!("aux sink not an AppSink"))?;
    Ok((pipeline, appsink))
}

/// Start the 2nd-track fetch/decode pipeline feeding `source`, pushing decoded stereo S16LE
/// PCM frames in exactly like the mic loop. On EOS (track finished) or pipeline ERROR, the
/// track is auto-unpublished via `on_done` so the SFU participant isn't left with a dead
/// publication. Returns the pipeline + feeder task.
fn start_aux_capture(
    url: &str,
    label: &str,
    source: NativeAudioSource,
    on_done: impl Fn() + Send + 'static,
) -> Result<(gst::Pipeline, tokio::task::JoinHandle<()>, Arc<std::sync::atomic::AtomicBool>)> {
    let (pipeline, appsink) = build_aux_audio_pipeline(url, label)?;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Vec<i16>>();
    appsink.set_callbacks(
        gstreamer_app::AppSinkCallbacks::builder()
            .new_sample(move |sink| {
                let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                let buffer = sample.buffer().ok_or(gst::FlowError::Error)?;
                let map = buffer.map_readable().map_err(|_| gst::FlowError::Error)?;
                let samples: Vec<i16> = map
                    .as_slice()
                    .chunks_exact(2)
                    .map(|b| i16::from_le_bytes([b[0], b[1]]))
                    .collect();
                let _ = tx.send(samples);
                Ok(gst::FlowSuccess::Ok)
            })
            .build(),
    );
    let task = tokio::spawn(async move {
        while let Some(mut samples) = rx.recv().await {
            if samples.len() < 2 {
                continue;
            }
            if samples.len() % 2 != 0 {
                samples.pop(); // never let a stray sample flip the L/R interleave phase
            }
            let spc = (samples.len() / 2) as u32; // interleaved stereo
            let frame = AudioFrame {
                data: Cow::Owned(samples),
                sample_rate: SAMPLE_RATE,
                num_channels: 2,
                samples_per_channel: spc,
            };
            let _ = source.capture_frame(&frame).await;
        }
    });

    // Watch the bus for EOS/ERROR on a dedicated thread; fire `on_done` so the session can
    // unpublish + tear down (a finished sound/music must not wedge the SFU participant). The
    // `stop` flag lets an explicit teardown (which NULLs the pipeline, posting no EOS) break
    // this loop so the thread exits instead of spinning forever (one leak per stop otherwise).
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    if let Some(bus) = pipeline.bus() {
        let label = label.to_string();
        let stop = stop.clone();
        std::thread::spawn(move || {
            use gst::MessageView;
            loop {
                if stop.load(std::sync::atomic::Ordering::Relaxed) {
                    break; // explicit teardown/close — pipeline was NULLed without an EOS
                }
                let Some(msg) = bus.timed_pop(gst::ClockTime::from_mseconds(500)) else { continue };
                match msg.view() {
                    MessageView::Eos(_) => {
                        eprintln!("[aux/{label}] EOS — finished");
                        // Grace period before unpublish: a very short clip can reach EOS
                        // before the SFU has finished forwarding it + a peer has subscribed +
                        // spun up its FrameCryptor. Hold the track ~1.5s past EOS so even
                        // sub-second sounds are actually received. (Abort early if an explicit
                        // teardown/replace set the stop flag meanwhile.)
                        for _ in 0..15 {
                            if stop.load(std::sync::atomic::Ordering::Relaxed) {
                                break;
                            }
                            std::thread::sleep(std::time::Duration::from_millis(100));
                        }
                        if !stop.load(std::sync::atomic::Ordering::Relaxed) {
                            on_done();
                        }
                        break;
                    }
                    MessageView::Error(e) => {
                        eprintln!("[aux/{label}] gst ERROR: {} ({:?})", e.error(), e.debug());
                        on_done();
                        break;
                    }
                    _ => {}
                }
            }
        });
    }

    if let Err(e) = pipeline.set_state(gst::State::Playing).context("aux pipeline start failed")
    {
        // Unblock the bus-watch thread spawned above and NULL the half-started pipeline —
        // returning bare Err leaked one OS thread spinning on timed_pop per failed start.
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        task.abort();
        let _ = pipeline.set_state(gst::State::Null);
        return Err(e);
    }
    Ok((pipeline, task, stop))
}

impl CallSession {
    /// Connect to the SFU with per-participant E2EE (our key at `key_index` under our
    /// identity), and start the event loop that plays remote participants' audio and
    /// builds remote-video tiles. Returns the session plus a receiver of
    /// [`CallEvent`]s the app uses to drive the QML video grid.
    ///
    /// `key_index` advances across joins (persisted app-side): re-minting at a FIXED
    /// index made every rejoin collide with the previous session's key in every peer's
    /// per-(identity,index) tracking — web EC's OutdatedKeyFilter could silently discard
    /// the fresh key (mixed timestamp domains), and our own room-poll could stomp it.
    /// A fresh index has no history anywhere, so the whole collision class vanishes.
    pub async fn connect(
        ws_url: &str,
        jwt: &str,
        identity: &str,
        key: [u8; 16],
        key_index: u8,
    ) -> Result<(Self, UnboundedReceiver<CallEvent>)> {
        Self::connect_opts(ws_url, jwt, identity, key, key_index, true).await
    }

    /// [`connect`] with an explicit choice about end-to-end encryption.
    ///
    /// `e2ee = false` exists for ONE reason: guest calls. A visitor joining from a
    /// browser has no Matrix identity, so there is nobody to exchange media keys with —
    /// an encrypted call would simply be silence and black tiles for them. Every normal
    /// call passes `true`, and `connect` above keeps that the default so no existing
    /// caller can lose encryption by omission.
    pub async fn connect_opts(
        ws_url: &str,
        jwt: &str,
        identity: &str,
        key: [u8; 16],
        key_index: u8,
        e2ee: bool,
    ) -> Result<(Self, UnboundedReceiver<CallEvent>)> {
        crate::ensure_init()?;

        let key_provider = matrix_key_provider();
        key_provider.set_key(&ParticipantIdentity(identity.to_owned()), key_index as i32, key.to_vec());
        // Register a clone globally so the app can install peers' keys without the
        // call mutex (clones share the native keyring, so it affects decryption).
        // The generation stamp lets close() know whether the registration is still
        // ITS OWN: do_join's defensive close of a leftover session used to run AFTER
        // the new session registered, and its unconditional clear left the live call
        // with NO keyring — every later peer key only "BUFFERED" and the whole call
        // was static until a full rejoin.
        let keyring_gen = KEYRING_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        *active_key_provider().lock().unwrap() = Some((keyring_gen, key_provider.clone()));
        // Replay peer keys that arrived during join (EC sends its key the moment it
        // sees our membership, which we publish before this connect — those installs
        // were buffered, not lost). Without this they'd be dropped → MissingKey →
        // black tiles + silence even though decryption is fully working.
        {
            let pending = pending_peer_keys().lock().unwrap();
            for ((id, idx), e) in pending.iter() {
                // Never replay a buffered key onto OUR slot (set just above): the install
                // guard only knows our identity after the SFU reports it (finding 1).
                if id == identity {
                    eprintln!("[recv3-replay] SKIPPED identity={id} index={idx} — our own identity");
                    continue;
                }
                key_provider.set_key(&ParticipantIdentity(id.clone()), *idx as i32, e.key.to_vec());
                eprintln!("[recv3-replay] set_key identity={id} index={idx} (buffered pre-connect)");
            }
            if !pending.is_empty() {
                eprintln!("[call] replayed {} buffered peer key(s) into keyring", pending.len());
            }
        }
        let mut options = RoomOptions::default();
        // 0.8.23: data-channel packets stay PLAINTEXT. The SDK encrypts them with the key
        // provider's global latest index (a peer's), so our identity has no key there and
        // every announce failed (recording-only between real clients, game chat). Our
        // packets are flags only — media stays E2EE.
        options.encrypt_data_channel = false;
        if e2ee {
            options.encryption = Some(E2eeOptions {
                encryption_type: EncryptionType::Gcm,
                key_provider,
            });
        } else {
            // Say it out loud in the log: an unencrypted call is a deliberate, visible
            // choice, never something to discover later from a packet capture.
            eprintln!("[call] E2EE DISABLED for this session (guest call)");
        }
        // DYNACAST: OFF. r124 turned this on for publisher-side uplink savings on the
        // ASSUMPTION that an SFU which never signals a native publisher would simply mean
        // "no savings, not breakage". That assumption is WRONG and it is what broke
        // screen-share (peers saw a permanently blank tile; a field report,
        // reproduced on a test machine).
        //
        // Why it breaks the SHARE but not the CAMERA: dynacast applies the SFU's
        // SubscribedQualityUpdate to our sender encodings in
        // LocalVideoTrack::set_publishing_layers, which resolves each encoding by RID:
        //     .find(|q| q.quality == quality_for_rid(encoding.rid))
        //     .map(|q| q.enabled)
        //     .unwrap_or(false)      // <-- NO MATCH => the layer is DISABLED
        // The camera publishes SIMULCAST, so its encodings carry real rids (q/h/f) that
        // match the update's entries and stay enabled. The screen-share deliberately
        // publishes a SINGLE sharp layer (simulcast: false), whose encoding has an EMPTY
        // rid — when the update doesn't carry a matching entry, that single encoding gets
        // switched OFF and the encoder stops after its first keyframe. Signature (exactly
        // what the owner's log and our repro show): `[lk-stats] ... 1920x800@0 key=1`, then
        // `0x0@0` forever, while subscribed peers PLI-storm for a picture that never comes.
        //
        // The uplink saving was never measured, the feature was never live-tested, and the
        // r124 author's own simulcast_loopback gate disables dynacast for this very reason
        // (see its doc comment). Correctness first: leave it OFF.
        options.dynacast = false;

        // Bring echo cancellation up BEFORE the room connects. The event task below builds
        // playback pipelines for peers ALREADY in the call essentially the moment it starts
        // (their TrackSubscribed events are queued while Room::connect blocks on the
        // subscriber PC), and each pipeline bakes its sink target in at build time. If the
        // EC sink doesn't exist yet, every pre-existing peer's audio binds the REAL sink —
        // the canceller's reference is then missing exactly the voices already in the call:
        // their echo is never cancelled and AEC3 adapts against a wrong reference, mangling
        // outgoing near-end speech. This was near-deterministic on every join into an
        // ongoing call, not a rare race. No-op when AEC is off/unavailable; idempotent when
        // already up. (The one-time module load + node warm-up adds ~0.5s before connect —
        // deliberately BEFORE, so the wait also guarantees the mic later binds a live node.)
        audio_health_probe();
        let _ = aec_start();

        // Seed the bandwidth estimator BEFORE the transport exists. The
        // webrtc-sys SetLocalDescription patch registers full BitrateSettings
        // (min/start/max — a lone start does not survive the configurator merge)
        // on the first negotiation, so the connect-time probe clusters fire at
        // 3x/6x of the seed instead of ~5 Mbps. Measured effect: grant 83 Mbps
        // at t=1 versus 4.4 Mbps unseeded — this is what actually removed the
        // ~6 Mbps share crawl. 10 Mbps seed ⇒ 30/60 Mbps probe bursts (~10-25ms
        // each) at join, before any media flows; a weak path simply measures
        // low and the estimate lands at its truth. Cleared after the join
        // window so later renegotiations aren't re-seeded.
        std::env::set_var("LK_BWE_START_KBPS", "10000");
        tokio::spawn(async {
            tokio::time::sleep(std::time::Duration::from_secs(8)).await;
            std::env::remove_var("LK_BWE_START_KBPS");
        });
        eprintln!("[call] connecting… (BWE seeded at 10000 kbps)");
        let (room, mut events) = match Room::connect(ws_url, jwt, options).await {
            Ok(v) => v,
            Err(e) => {
                aec_stop(); // don't leave the module up for a call that never happened
                return Err(e).context("LiveKit connect failed");
            }
        };
        eprintln!("[call] connected as {}", room.local_participant().identity().0);
        *local_identity_cell().lock().unwrap() = room.local_participant().identity().0.clone();
        // 10.5: "can they hear me?" — BEFORE the event task, so no subscription is missed. Not for
        // the Memories recorder: it has no mic, plays nothing and must not send receipts.
        mic_wanted_flag().store(false, std::sync::atomic::Ordering::Relaxed);
        if record_dir().is_none() {
            crate::hear::session_start(&room.local_participant().identity().0);
            crate::sameroom::session_start(&room.local_participant().identity().0); // 11.3
            crate::callstats::session_start(&room.local_participant().identity().0); // widgets: call stats
            for (id, _) in room.remote_participants() {
                crate::hear::participant_present(&id.0);
                crate::sameroom::participant_joined(&id.0); // 11.3: people already in the call
                crate::callstats::participant_present(&id.0);
            }
        }
        for (id, _) in room.remote_participants() {
            corner_add_participant(&id.0); // 9.4: pull targets resolve against who is here
        }
        // Make every LOCAL sender cryptor encrypt at OUR key index. Upstream creates
        // sender cryptors hardwired to index 0, so any other announced index silently
        // diverged from the frames' trailer index — peers would install a key the
        // frames never reference (undecryptable). Set BEFORE any track publishes.
        room.e2ee_manager()
            .set_sender_key_index(&room.local_participant().identity(), key_index as i32);
        set_active_speakers(Vec::new()); // never inherit a previous call's speakers
        speakers_log_guard().store(usize::MAX, std::sync::atomic::Ordering::Relaxed); // first delta of a new call always logs

        // Capture a clone of our LocalParticipant so the (opt-in) 2nd audio track —
        // soundboard / #7 music — can publish/unpublish without the CallSession. A clone
        // shares the same participant; this stays None-equivalent until a sound/music plays.
        *aux_participant().lock().unwrap() = Some(room.local_participant());
        // Capture the runtime Handle (we're in-runtime here) so `teardown_aux` can schedule
        // the async unpublish from the bus-watch std::thread / Qt GUI thread without panicking.
        *aux_runtime().lock().unwrap() = Some(tokio::runtime::Handle::current());

        let playback: Arc<Mutex<std::collections::HashMap<String, PlaybackHandle>>> =
            Arc::new(Mutex::new(std::collections::HashMap::new()));
        let (tx, rx) = unbounded_channel::<CallEvent>();
        forget_all_moderation(); // a new call starts with nobody server-muted
        *event_sender().lock().unwrap() = Some(tx.clone());
        // Test gate (TINHAT_TEST_TERMINAL_DROP_AFTER_S=N): inject a recoverable TERMINAL
        // disconnect N seconds after connect, to exercise the app's auto-rejoin chain
        // (decision → teardown → fresh join → mute restore) on demand. The real trigger
        // (SFU 401s an in-place reconnect once the 10-min join token has expired) needs
        // >10 min and an unluckily-timed network blip to reproduce; this doesn't.
        if let Some(secs) =
            std::env::var("TINHAT_TEST_TERMINAL_DROP_AFTER_S").ok().and_then(|s| s.parse::<u64>().ok())
        {
            let tx_test = tx.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
                eprintln!("[call] TEST: injecting terminal disconnect (recoverable)");
                let _ = tx_test.send(CallEvent::ConnectionState {
                    reconnecting: false,
                    disconnected: true,
                    recoverable: true,
                });
            });
        }
        let hear_task = record_dir().is_none().then(|| crate::hear::spawn_analyzer(room.local_participant()));
        if record_dir().is_none() {
            // 11.3 same room: the 50 ms sender (levels, link packets, election); exits with the session
            let _ = crate::sameroom::spawn_sender(room.local_participant());
        }
        if record_dir().is_none() {
            crate::callstats::spawn(room.local_participant()); // widgets: receive stats + `tinhat.rxq`
        }
        let pb = playback.clone();
        let event_task = tokio::spawn(async move {
            while let Some(ev) = events.recv().await {
                match ev {
                    RoomEvent::TrackSubscribed { track: RemoteTrack::Audio(t), publication, participant } => {
                        // [recv4-lkid] THE decisive line: the identity string the SFU
                        // assigned this peer (what the FrameCryptor looks the key up under)
                        // + whether the SFU signaled the track encrypted. Compare identity
                        // to [recv3-install]; enc_type=None => no cryptor, no key helps.
                        eprintln!(
                            "[recv4-lkid] audio TrackSubscribed LK_IDENTITY={:?} sid={} enc_type={:?}",
                            participant.identity().0, publication.sid().as_str(), publication.encryption_type()
                        );
                        let identity = participant.identity().0.clone();
                        let category = category_for_source(publication.source()).to_string();
                        let sid = publication.sid().as_str().to_string();
                        crate::hear::recv_track_added(&identity, &category, &sid); // 10.5 (no-op in the recorder)
                        crate::callstats::track_added(&identity, &sid, publication.source(), RemoteTrack::Audio(t.clone()));
                        if publication.is_muted() {
                            // muted before we subscribed: no TrackMuted transition will ever say so
                            crate::hear::note_track_muted(&identity, &category, true);
                        }
                        audio_sids_live().lock().unwrap().insert(sid.clone());
                        // Memories recorder service (0.8.7): QUEUE for a consent-checked capture
                        // chain (built by drain_record_pending AFTER the next opt-out
                        // refresh — building here raced a late joiner's marker).
                        if record_dir().is_some() {
                            eprintln!("[vod] queued audio {identity} sid={sid} (awaiting consent check)");
                            record_pending().lock().unwrap().push(PendingRecord::Audio {
                                track: t.rtc_track(),
                                identity: identity.clone(),
                                sid: sid.clone(),
                            });
                            continue;
                        }
                        match build_playback(t.rtc_track(), identity.clone(), category.clone(), sid.clone()) {
                            Ok((p, task)) => {
                                // NULL + ABORT any displaced playback (re-subscribe of the same
                                // sid): HashMap::insert drops the old value, but a bare gst Drop
                                // only unrefs — it never NULLs a PLAYING pipeline nor stops the
                                // feeder task, so it would leak (orphaned "[call] playback:").
                                if let Some(old) = pb.lock().unwrap().insert(
                                    publication.sid().as_str().to_string(),
                                    PlaybackHandle { pipeline: p, task },
                                ) {
                                    old.task.abort();
                                    let cur = old.pipeline.lock().unwrap().clone();
                                    set_state_bounded(cur.upcast_ref(), gst::State::Null, 1500, "remote-audio playback");
                                }
                            }
                            Err(e) => {
                                eprintln!("[call] playback build failed for {identity} ({category}): {e:#} — retrying");
                                crate::hear::note_play_stall(&sid, true); // "can't hear X — repairing"
                                spawn_playback_retry(pb.clone(), t.rtc_track(), identity, category, sid);
                            }
                        }
                    }
                    RoomEvent::DataReceived { payload, topic, participant, .. } => {
                        if topic.as_deref() == Some(REC_ONLY_TOPIC) {
                            let who = participant.as_ref().map(|p| p.identity().0.clone()).unwrap_or_default();
                            let text = String::from_utf8_lossy(&payload[..]);
                            let sid = text
                                .split("\"sid\":\"")
                                .nth(1)
                                .and_then(|r| r.split('"').next())
                                .unwrap_or("")
                                .to_string();
                            eprintln!("[call] {who}: recording-only announcement -> {sid:?}");
                            handle_rec_only_announcement(&who, &sid);
                        } else if topic.as_deref() == Some(GAME_CHAT_TOPIC) {
                            let who = participant.as_ref().map(|p| p.identity().0.clone()).unwrap_or_default();
                            let text = String::from_utf8_lossy(&payload[..]);
                            let on = text.replace(' ', "").contains("\"on\":true");
                            handle_game_chat_announcement(&who, on);
                        } else if topic.as_deref() == Some(CORNER_TOPIC) {
                            // 9.4: who is in which corner (+ a pull addressed to someone).
                            let who = participant.as_ref().map(|p| p.identity().0.clone()).unwrap_or_default();
                            let text = String::from_utf8_lossy(&payload[..]).to_string();
                            let id = corner_field(&text, "corner").to_string();
                            let pull = corner_field(&text, "pull").to_string();
                            handle_corner_announcement(&who, &id, &pull);
                        } else if topic.as_deref() == Some(MODERATION_TOPIC) {
                            // RBAC: who muted / unmuted / disconnected whom.
                            let who = participant.as_ref().map(|p| p.identity().0.clone()).unwrap_or_default();
                            let text = String::from_utf8_lossy(&payload[..]);
                            match parse_moderation_packet(&text) {
                                Some(pkt) => handle_moderation_packet(&who, pkt, &tx),
                                None => eprintln!("[call] moderation: unparseable packet from {who}: {text}"),
                            }
                        } else if topic.as_deref() == Some(crate::hear::RX_TOPIC) {
                            // 10.5: what this person receives from each of us ("Heard by 3 of 3").
                            let who = participant.as_ref().map(|p| p.identity().0.clone()).unwrap_or_default();
                            crate::hear::handle_rx(&who, &String::from_utf8_lossy(&payload[..]));
                        } else if topic.as_deref() == Some(crate::hear::ECHO_TOPIC) {
                            // 10.5: someone hears themselves through OUR mic.
                            let who = participant.as_ref().map(|p| p.identity().0.clone()).unwrap_or_default();
                            crate::hear::handle_echo(&who, &String::from_utf8_lossy(&payload[..]));
                        } else if topic.as_deref() == Some(crate::sameroom::LINK_TOPIC) {
                            // 11.3 same room: ask / yes / no / unlink between two people in one room.
                            let who = participant.as_ref().map(|p| p.identity().0.clone()).unwrap_or_default();
                            crate::sameroom::handle_link(&who, &String::from_utf8_lossy(&payload[..]));
                        } else if topic.as_deref() == Some(crate::sameroom::LVL_TOPIC) {
                            // 11.3 same room: a linked peer's raw mic level (20 Hz, lossy) for the room-mic election.
                            let who = participant.as_ref().map(|p| p.identity().0.clone()).unwrap_or_default();
                            crate::sameroom::handle_lvl(&who, &String::from_utf8_lossy(&payload[..]));
                        } else if topic.as_deref() == Some(crate::callstats::RXQ_TOPIC) {
                            // widgets: how this person receives US (the Call stats widget's outgoing lane).
                            let who = participant.as_ref().map(|p| p.identity().0.clone()).unwrap_or_default();
                            crate::callstats::handle_rxq(&who, &payload[..]);
                        }
                    }
                    RoomEvent::TrackSubscribed { track: RemoteTrack::Video(t), publication, participant } => {
                        let id = tile_id(publication.sid().as_str());
                        let who = participant.identity().0.clone();
                        let is_screenshare = matches!(publication.source(), TrackSource::Screenshare);
                        crate::callstats::track_added(&who, publication.sid().as_str(), publication.source(), RemoteTrack::Video(t.clone()));
                        eprintln!(
                            "[recv4-lkid] video TrackSubscribed LK_IDENTITY={:?} sid={} screenshare={is_screenshare} enc_type={:?} simulcast={}",
                            who, publication.sid().as_str(), publication.encryption_type(), publication.simulcasted()
                        );
                        // Retain the publication so the app can signal the SFU per tile
                        // (pause / layer selection), and apply the CURRENT policy to this
                        // late-subscribed track (a track arriving while minimized or under
                        // a receive cap must inherit it, cf. the away-privacy reapply).
                        remote_video_pubs()
                            .lock()
                            .unwrap()
                            .insert(id.clone(), (publication.clone(), is_screenshare));
                        // Every share tile is "not watched" until the app says so
                        // (its owner's share audio starts muted). Never in the recorder.
                        if is_screenshare && record_dir().is_none() {
                            note_share_tile(&id, &who);
                        }
                        // A share the publisher flagged "recording only" (attribute
                        // already set, or arriving via ParticipantAttributesChanged below).
                        if is_screenshare {
                            let flagged = announced_rec_only()
                                .lock()
                                .unwrap()
                                .get(&who)
                                .map(|v| v == publication.sid().as_str())
                                .unwrap_or(false);
                            if flagged {
                                apply_rec_only(&id, &who, true);
                            }
                        }
                        spawn_recv_reconcile(Some(id.clone()));
                        // Memories recorder service (0.8.7): QUEUE for a consent-checked capture
                        // chain (see the audio arm — same join-race fix).
                        if record_dir().is_some() {
                            eprintln!(
                                "[vod] queued video {who} sid={} (awaiting consent check)",
                                publication.sid().as_str()
                            );
                            record_pending().lock().unwrap().push(PendingRecord::Video {
                                track: t.rtc_track(),
                                identity: who.clone(),
                                sid: publication.sid().as_str().to_string(),
                                screenshare: is_screenshare,
                            });
                            continue;
                        }
                        // Build the render pipeline on the GUI thread (in the app), not here.
                        let _ = tx.send(CallEvent::VideoAdded { id, participant: who, is_screenshare, track: t.rtc_track() });
                    }
                    RoomEvent::E2eeStateChanged { participant, state } => {
                        // [recv5-e2ee] splits the crypto failure modes: MissingKey => wrong
                        // identity/index or key not installed; DecryptionFailed => wrong key
                        // bytes/KDF; Ok => crypto solved (any remaining black = render).
                        eprintln!(
                            "[recv5-e2ee] participant={:?} state={state:?}",
                            participant.identity().0
                        );
                        // Track MissingKey episodes per participant; the app's key loop
                        // polls stuck_missing_keys() every few seconds and escalates. The
                        // state events themselves are too rare to drive a watchdog (the
                        // 2026-08-19 zombie call logged exactly TWO of them in 12 min).
                        {
                            let id = participant.identity().0.clone();
                            let is_missing = format!("{state:?}").contains("MissingKey");
                            let mut map = missing_key_since().lock().unwrap();
                            if is_missing {
                                map.entry(id).or_insert_with(|| (std::time::Instant::now(), false));
                            } else {
                                map.remove(&id);
                            }
                        }
                    }
                    RoomEvent::TrackUnsubscribed { publication, participant, .. } => {
                        // Drop this track's gain entry (audio) so no stale WeakRef lingers,
                        // and tear down its video tile (the id is harmless for audio tracks).
                        let identity = participant.identity().0.clone();
                        let cat = category_for_source(publication.source());
                        unregister_track_gain(&identity, cat, publication.sid().as_str());
                        unregister_track_level(&identity, cat); // #B drop this track's level
                        crate::hear::recv_track_removed(publication.sid().as_str()); // 10.5
                        crate::callstats::track_removed(publication.sid().as_str()); // widgets
                        audio_sids_live().lock().unwrap().remove(publication.sid().as_str());
                        // #17: NULL + drop this track's remote-audio playback pipeline (a Vec
                        // never pruned it, leaking one PLAYING pipeline per unsubscribe).
                        if let Some(h) = pb.lock().unwrap().remove(publication.sid().as_str()) {
                            h.task.abort(); // stop the feeder so it can't outlive the pipeline
                            let cur = h.pipeline.lock().unwrap().clone();
                            set_state_bounded(cur.upcast_ref(), gst::State::Null, 1500, "remote-audio playback");
                        }
                        let id = tile_id(publication.sid().as_str());
                        // Drop the retained publication + this tile's transient policy.
                        remote_video_pubs().lock().unwrap().remove(&id);
                        forget_rec_only(&id);
                        forget_share_tile(&id);
                        {
                            let mut pol = recv_policy().lock().unwrap();
                            pol.hidden.remove(&id);
                            pol.dims.remove(&id);
                        }
                        eprintln!("[call] remote track gone from {identity} ({id})");
                        let _ = tx.send(CallEvent::VideoRemoved { id });
                    }
                    RoomEvent::ParticipantConnected(p) => {
                        let identity = p.identity().0.clone();
                        eprintln!("[call] participant joined: {identity}");
                        corner_add_participant(&identity);
                        crate::hear::participant_present(&identity); // 10.5
                        crate::sameroom::participant_joined(&identity); // 11.3
                        crate::callstats::participant_present(&identity); // widgets
                        // RBAC: a late joiner must learn who I server-muted (the SFU says
                        // nothing about it) — same idea as the corner / game-chat re-announce.
                        {
                            let me = local_identity_cell().lock().unwrap().clone();
                            let mine: Vec<(String, String)> = moderation()
                                .lock()
                                .unwrap()
                                .muted
                                .iter()
                                .filter(|(_, (by, _))| *by == me)
                                .map(|(t, (_, n))| (t.clone(), n.clone()))
                                .collect();
                            if !me.is_empty() && !mine.is_empty() {
                                if let Some(lp) = aux_participant().lock().unwrap().clone() {
                                    let dest = identity.clone();
                                    tokio::spawn(async move {
                                        for (target, name) in mine {
                                            let json = moderation_packet_json("mute", &target, &me, &name);
                                            if let Err(e) = lp
                                                .publish_data(moderation_data_packet(&json, vec![dest.clone()]))
                                                .await
                                            {
                                                eprintln!("[call] moderation re-announce failed: {e}");
                                            }
                                        }
                                    });
                                }
                            }
                        }
                        // 9.4: a late joiner must learn which corner we are in.
                        let mine = corner_mine();
                        if !mine.is_empty() {
                            if let Some(lp) = aux_participant().lock().unwrap().clone() {
                                tokio::spawn(async move {
                                    if let Err(e) = lp.publish_data(corner_packet(&mine, "")).await {
                                        eprintln!("[call] corner re-announce failed: {e}");
                                    }
                                });
                            }
                        }
                        // A late joiner must learn about our recording-only share.
                        if let Some(sid) = my_rec_only_sid().lock().unwrap().clone() {
                            if let Some(lp) = aux_participant().lock().unwrap().clone() {
                                tokio::spawn(async move {
                                    if let Err(e) = lp.publish_data(rec_only_packet(&sid)).await {
                                        eprintln!("[call] recording-only re-announce failed: {e}");
                                    }
                                });
                            }
                        }
                        // A late joiner must learn that we are in game chat.
                        if local_game_chat() {
                            if let Some(lp) = aux_participant().lock().unwrap().clone() {
                                tokio::spawn(async move {
                                    if let Err(e) = lp.publish_data(game_chat_packet(true)).await {
                                        eprintln!("[call] game chat re-announce failed: {e}");
                                    }
                                });
                            }
                        }
                        // Tell the app so it can create an avatar tile, (re)announce our media
                        // key to this peer (essential when they join AFTER us), and rotate.
                        let _ = tx.send(CallEvent::ParticipantJoined { identity });
                    }
                    RoomEvent::ParticipantDisconnected(p) => {
                        // Purge every gain entry for this peer (all categories) so no stale
                        // (dead) WeakRef survives a re-join.
                        let identity = p.identity().0.clone();
                        unregister_participant_gains(&identity);
                        unregister_participant_levels(&identity); // #B drop their levels
                        crate::hear::participant_left(&identity); // 10.5
                        crate::sameroom::participant_left(&identity); // 11.3
                        crate::callstats::participant_left(&identity); // widgets
                        announced_rec_only().lock().unwrap().remove(&identity);
                        forget_game_chat_peer(&identity);
                        forget_corner_peer(&identity);
                        // RBAC: a server-muted peer who leaves takes their shield with them.
                        if moderation().lock().unwrap().muted.remove(&identity).is_some() {
                            let _ = tx.send(CallEvent::ServerMuteChanged {
                                identity: identity.clone(),
                                muted: false,
                                by: String::new(),
                                by_name: String::new(),
                            });
                        }
                        share_watch_muted().lock().unwrap().remove(&identity);
                        eprintln!("[call] participant left: {identity}");
                        // Remove their avatar tile + trigger an E2EE key rotation (forward secrecy).
                        let _ = tx.send(CallEvent::ParticipantLeft { identity });
                    }
                    RoomEvent::TrackMuted { participant, publication } => {
                        let identity = participant.identity().0.clone();
                        let category = category_for_source(publication.source()).to_string();
                        // Log VIDEO mutes: Element X publishes a muted placeholder track
                        // when the camera is off (and streams black when backgrounded) —
                        // the receiver's tile just sat dead/black with zero log trace
                        // (2026-08-20 audit). The mic glyph consumed this event; video
                        // was silently dropped on the floor.
                        if category != "mic" {
                            eprintln!("[call] remote {category} track MUTED by {identity}");
                        }
                        crate::hear::note_track_muted(&identity, &category, true); // 10.5
                        let _ = tx.send(CallEvent::TrackMuteChanged { identity, category, muted: true });
                    }
                    RoomEvent::TrackUnmuted { participant, publication } => {
                        let identity = participant.identity().0.clone();
                        let category = category_for_source(publication.source()).to_string();
                        if category != "mic" {
                            eprintln!("[call] remote {category} track UNMUTED by {identity}");
                        }
                        crate::hear::note_track_muted(&identity, &category, false); // 10.5
                        let _ = tx.send(CallEvent::TrackMuteChanged { identity, category, muted: false });
                    }
                    RoomEvent::ActiveSpeakersChanged { speakers } => {
                        // Server-side speaking detection (voice overlay / speaking rings).
                        // Count-only log (no identities): field evidence that remote
                        // speakers actually arrive post-merge (vendored SDK patch) —
                        // safe for the redacted diagnostics report.
                        {
                            let n = speakers.len();
                            if speakers_log_guard().swap(n, std::sync::atomic::Ordering::Relaxed) != n {
                                eprintln!("[call] speakers: {n} active");
                            }
                        }
                        set_active_speakers(speakers.iter().map(|p| p.identity().0.clone()).collect());
                    }
                    RoomEvent::Reconnecting => {
                        eprintln!("[call] SFU connection lost — reconnecting…");
                        crate::hear::set_reconnecting(true); // 10.5: not "your fault" while the link is down
                        set_active_speakers(Vec::new()); // don't freeze a stale "speaking" ring
                        speakers_log_guard().store(usize::MAX, std::sync::atomic::Ordering::Relaxed); // first post-resume delta always logs
                        let _ = tx.send(CallEvent::ConnectionState { reconnecting: true, disconnected: false, recoverable: true });
                    }
                    RoomEvent::Reconnected => {
                        eprintln!("[call] SFU reconnected");
                        crate::hear::set_reconnecting(false);
                        let _ = tx.send(CallEvent::ConnectionState { reconnecting: false, disconnected: false, recoverable: true });
                    }
                    RoomEvent::Disconnected { reason } => {
                        eprintln!("[call] SFU disconnected: {reason:?}");
                        set_active_speakers(Vec::new());
                        // RBAC: a moderator removed us. Say who (the `disconnect` packet
                        // addressed to us moments before) BEFORE the terminal ConnectionState
                        // so the app can label the call-ended banner.
                        if matches!(reason, livekit::DisconnectReason::ParticipantRemoved) {
                            let notice = moderation().lock().unwrap().removal_notice.take();
                            let (by, by_name) = match notice {
                                Some((b, n, t)) if t.elapsed() < std::time::Duration::from_secs(60) => (b, n),
                                _ => (String::new(), String::new()),
                            };
                            eprintln!("[call] moderation: removed from the call by {by:?}");
                            let _ = tx.send(CallEvent::RemovedByModerator { by, by_name });
                        }
                        // Deliberate server-side removals must NOT be auto-rejoined: a
                        // kicked user forcing their way back ~2s after every removal, or
                        // every client resurrecting a deleted room, is worse than the
                        // disconnect. Network/token failures (UnknownReason, SignalClose,
                        // timeouts, ServerShutdown, …) stay recoverable — that includes
                        // the 10-min-token 401 loop this event usually is.
                        use livekit::DisconnectReason as DR;
                        let recoverable = !matches!(
                            reason,
                            DR::ClientInitiated
                                | DR::DuplicateIdentity
                                | DR::ParticipantRemoved
                                | DR::RoomDeleted
                                | DR::RoomClosed
                                | DR::UserRejected
                                | DR::UserUnavailable
                        );
                        let _ = tx.send(CallEvent::ConnectionState { reconnecting: false, disconnected: true, recoverable });
                    }
                    RoomEvent::Connected { participants_with_tracks } => {
                        // Peers ALREADY in the call when WE join are surfaced here ONCE (not via
                        // ParticipantConnected). Seed them so a camera-off/voice-only peer who was
                        // here before us still gets an avatar tile.
                        for (p, tracks) in participants_with_tracks {
                            let identity = p.identity().0.clone();
                            crate::hear::participant_present(&identity); // 10.5
                            crate::callstats::participant_present(&identity); // widgets
                            let _ = tx.send(CallEvent::ParticipantJoined { identity: identity.clone() });
                            // LiveKit only fires TrackMuted/TrackUnmuted on a TRANSITION, so a peer
                            // who was ALREADY muted before we (re)joined never produces a delta event
                            // and the app's `muted` set (which starts empty every join) would show
                            // them un-muted. Seed the CURRENT mute state from each present track's
                            // publication so the per-tile mic glyph is correct on (re)join. Emit only
                            // the muted ones (the empty set already means "everyone unmuted"); we
                            // mirror the delta arms above, including category_for_source.
                            for pubn in &tracks {
                                if pubn.is_muted() {
                                    let _ = tx.send(CallEvent::TrackMuteChanged {
                                        identity: identity.clone(),
                                        category: category_for_source(pubn.source()).to_string(),
                                        muted: true,
                                    });
                                }
                            }
                        }
                    }
                    RoomEvent::LocalTrackRepublished { previous_sid, publication, .. } => {
                        // Full reconnect re-issued this local track's sid — record it so
                        // every cached-sid consumer (unpublish/mute/republish) signals the
                        // LIVE track, not the pre-reconnect ghost. See republished_sids().
                        let name = publication.name();
                        let sid = publication.sid();
                        eprintln!(
                            "[call] local track '{name}' republished by reconnect: {previous_sid} -> {sid}"
                        );
                        republished_sids().lock().unwrap().insert(name, sid);
                    }
                    RoomEvent::ConnectionQualityChanged { quality, participant } => {
                        // widgets: LiveKit's own view of each connection (the Call stats widget's dot).
                        let local = matches!(participant, livekit::participant::Participant::Local(_));
                        crate::callstats::connection_quality(&participant.identity().0, local, quality);
                    }
                    RoomEvent::TrackSubscriptionFailed { participant, error, track_sid } => {
                        // 10.5 (audio review #1): this vanished in the catch-all arm — a track we
                        // will never hear, with nothing in the log to say so.
                        eprintln!(
                            "[call] subscription FAILED for {}'s track {}: {error}",
                            participant.identity().0,
                            track_sid.as_str()
                        );
                    }
                    RoomEvent::ParticipantPermissionChanged { participant, permission } => {
                        // RBAC: the server-side mute/unmute lands here for US (the SFU
                        // rewrote our publish permission). Peers' permission changes carry
                        // no information we act on (their packet + track mute already did).
                        if let livekit::participant::Participant::Local(_) = &participant {
                            let me = participant.identity().0.clone();
                            let (cp, srcs) = permission
                                .as_ref()
                                .map(|p| (p.can_publish, p.can_publish_sources.clone()))
                                .unwrap_or((true, Vec::new()));
                            handle_my_permission_changed(&me, cp, srcs, &tx);
                        }
                    }
                    _ => {}
                }
            }
        });

        Ok((
            Self {
                room,
                event_task,
                keyring_gen,
                mic: None,
                screenshare: None,
            share_is_av1: false,
                camera: None,
                _playback: playback,
                share_dead_hook: None,
                camera_dead_hook: None,
                camera_quiet_hook: None,
                hear_task,
            },
            rx,
        ))
    }

    /// Install the capture-died hook for screen shares (see `share_dead_hook`).
    /// Call once after connect, before any share starts.
    pub fn set_share_dead_hook(&mut self, hook: Arc<dyn Fn() + Send + Sync>) {
        self.share_dead_hook = Some(hook);
    }

    /// Install the capture-died hook for the camera (see `camera_dead_hook`).
    pub fn set_camera_dead_hook(&mut self, hook: Arc<dyn Fn() + Send + Sync>) {
        self.camera_dead_hook = Some(hook);
    }

    /// Install the capture-went-quiet hook for the camera (see `camera_quiet_hook`).
    pub fn set_camera_quiet_hook(&mut self, hook: Arc<dyn Fn(u64) + Send + Sync>) {
        self.camera_quiet_hook = Some(hook);
    }

    /// Build + START the screen-share capture, preferring the GPU (CUDA) scale path and
    /// FALLING BACK to the CPU one if it can't reach PLAYING. The GPU path needs a known
    /// output size (no mid-flight caps re-pin), so it's used only when the portal told us the
    /// source size — which is exactly the expensive case (a whole 4K/5K monitor). A hybrid or
    /// AMD box whose compositor DMABuf won't import into CUDA simply lands on the CPU path,
    /// same as before. `TINHAT_CAPTURE_CPU=1` forces the old path.
    fn start_share_capture(
        screencast: &ScreenCast,
        q: crate::lk_publish::ShareQuality,
        tw: u32,
        th: u32,
        fps: u32,
        source: livekit::webrtc::video_source::native::NativeVideoSource,
    ) -> Result<gst::Pipeline> {
        // GPU scale-path policy:
        //   Linux   — CUDA convert/scale when the portal reported a size (unchanged).
        //   Windows — the capture textures are already on the D3D11 device; scaling
        //     there and downloading only the small finished frame is the "near-zero
        //     game impact" contract of the NVENC preset (the CPU path's full-res 5K
        //     readback + lanczos is the frametime hazard). Windows tries, in order:
        //     CUDA interop (cudaupload can import D3D11 textures on builds that
        //     support it) -> d3d11convert to I420 -> d3d11convert to NV12 + small CPU
        //     convert. First candidate that negotiates wins; the winner is remembered
        //     so re-shares skip the losers' 5s negotiation waits.
        //   The VP9 preset deliberately KEEPS the CPU lanczos path on Windows: it is
        //     the "sharpest static text" choice and bilinear GPU sampling would undo
        //     exactly that. TINHAT_CAPTURE_CPU forces CPU everywhere;
        //     TINHAT_CAPTURE_GPU forces the GPU attempt regardless of preset (bench aid).
        let gpu_pref_ok = if cfg!(windows) {
            q.encoder == crate::lk_publish::ShareEncoder::NvencH264
                || std::env::var_os("TINHAT_CAPTURE_GPU").is_some()
        } else {
            true
        };
        #[cfg(windows)]
        let has_gpu_elements =
            crate::lk_publish::cuda_scale_available() || crate::lk_publish::d3d11_scale_available();
        #[cfg(not(windows))]
        let has_gpu_elements = crate::lk_publish::cuda_scale_available();
        let gpu_ok = gpu_pref_ok && has_gpu_elements && std::env::var_os("TINHAT_CAPTURE_CPU").is_none();
        let want_gpu = screencast.size().is_some() && gpu_ok;
        // No size known (KDE's portal gives none for a window and often none for a monitor; the stream probe
        // failed on every share in the field logs) → the CUDA path that pins its output from the FIRST frame, instead of
        // the CPU path that published 11–22 fps for a 3–5K source whatever the settings. Linux only: on Windows the
        // picker always knows the size.
        let want_gpu_learn = !want_gpu && gpu_ok && cfg!(not(windows)) && crate::lk_publish::cuda_scale_available();
        // the quality box the real size is fitted into (Auto = the current rung; the raw 0x0 published 16x16)
        let (box_w, box_h) = crate::lk_publish::share_box(q);

        if want_gpu {
            type Builder = fn(&ScreenCast, u32, u32, u32) -> Result<gst::Pipeline>;
            let mut candidates: Vec<(&'static str, Builder)> = Vec::new();
            if crate::lk_publish::cuda_scale_available() {
                #[cfg(windows)]
                candidates.push(("GPU (CUDA) scale, source-paced", |sc, w, h, f| {
                    crate::lk_publish::build_capture_pipeline_cuda(sc, w, h, f, true)
                }));
                candidates.push(("GPU (CUDA) scale", |sc, w, h, f| {
                    crate::lk_publish::build_capture_pipeline_cuda(sc, w, h, f, false)
                }));
            }
            #[cfg(windows)]
            if crate::lk_publish::d3d11_scale_available() {
                // Source-paced variants FIRST (the 165Hz-game judder fix); unpaced
                // stay as automatic fallbacks for setups that can't negotiate them.
                candidates.push(("GPU (D3D11) scale, I420, source-paced", |sc, w, h, f| {
                    crate::lk_publish::build_capture_pipeline_d3d11(sc, w, h, f, false, true)
                }));
                candidates.push(("GPU (D3D11) scale, NV12+convert, source-paced", |sc, w, h, f| {
                    crate::lk_publish::build_capture_pipeline_d3d11(sc, w, h, f, true, true)
                }));
                candidates.push(("GPU (D3D11) scale, I420", |sc, w, h, f| {
                    crate::lk_publish::build_capture_pipeline_d3d11(sc, w, h, f, false, false)
                }));
                candidates.push(("GPU (D3D11) scale, NV12+convert", |sc, w, h, f| {
                    crate::lk_publish::build_capture_pipeline_d3d11(sc, w, h, f, true, false)
                }));
            }
            // Try last session's winner first so a re-share never re-pays the
            // failed candidates' negotiation timeouts.
            static LAST_GOOD: std::sync::atomic::AtomicUsize =
                std::sync::atomic::AtomicUsize::new(usize::MAX);
            let last = LAST_GOOD.load(std::sync::atomic::Ordering::Relaxed);
            let order: Vec<usize> = if last < candidates.len() {
                std::iter::once(last)
                    .chain((0..candidates.len()).filter(|&i| i != last))
                    .collect()
            } else {
                (0..candidates.len()).collect()
            };

            for idx in order {
                let (label, build) = candidates[idx];
                match build(screencast, tw, th, fps) {
                    Ok(pipeline) => {
                        let wired = pipeline
                            .by_name("sink")
                            .and_then(|s| s.downcast::<gstreamer_app::AppSink>().ok());
                        if let Some(appsink) = wired {
                            crate::lk_publish::wire_appsink_to_source(&appsink, source.clone());
                            if set_state_bounded(pipeline.upcast_ref(), gst::State::Playing, 6000, "screen capture start") {
                                // Confirm it actually NEGOTIATED (memory-import failures
                                // surface only here); otherwise try the next candidate.
                                match pipeline.state(gst::ClockTime::from_seconds(5)).0 {
                                    Ok(_) => {
                                        LAST_GOOD
                                            .store(idx, std::sync::atomic::Ordering::Relaxed);
                                        eprintln!("[call] screen-share capture: {label} path");
                                        return Ok(pipeline);
                                    }
                                    Err(e) => eprintln!(
                                        "[call] {label} capture failed to negotiate ({e}) — trying next"
                                    ),
                                }
                            } else {
                                eprintln!("[call] {label} capture failed to start — trying next");
                            }
                        }
                        set_state_bounded(pipeline.upcast_ref(), gst::State::Null, 1500, "screen capture");
                    }
                    Err(e) => eprintln!("[call] {label} capture unavailable ({e})"),
                }
            }
            eprintln!("[call] no GPU capture candidate negotiated — falling back to CPU");
        }

        if want_gpu_learn {
            match Self::start_share_capture_gpu_learn(screencast, box_w, box_h, fps, &source) {
                Ok(pipeline) => return Ok(pipeline),
                Err(why) => eprintln!("[call] GPU (CUDA) capture, size from the first frame: {why} — falling back to CPU"),
            }
        }

        // CPU path: videoconvert + lanczos videoscale (threaded since 10.8), with the one-shot re-pin probe.
        let pipeline = crate::lk_publish::build_capture_pipeline(screencast, box_w, box_h, fps)?;
        let appsink = pipeline
            .by_name("sink")
            .context("screenshare appsink missing")?
            .downcast::<gstreamer_app::AppSink>()
            .map_err(|_| anyhow::anyhow!("screenshare sink not an AppSink"))?;
        crate::lk_publish::wire_appsink_to_source(&appsink, source);
        if !set_state_bounded(pipeline.upcast_ref(), gst::State::Playing, 8000, "screen capture start") {
            set_state_bounded(pipeline.upcast_ref(), gst::State::Null, 1500, "screen capture");
            anyhow::bail!("screenshare pipeline start failed (the capture never started)");
        }
        eprintln!("[call] screen-share capture: CPU scale path");
        Ok(pipeline)
    }

    /// Start the CUDA capture whose output size comes from the first frame
    /// ([`crate::lk_publish::build_capture_pipeline_cuda_learn`]) and commit to it only once a frame has come all the way
    /// to the appsink within 4 s with no bus error — a DMABuf that will not import into CUDA fails exactly there, and the
    /// caller then falls back to the CPU path as before. Err = the reason (the pipeline is already stopped).
    fn start_share_capture_gpu_learn(
        screencast: &ScreenCast,
        box_w: u32,
        box_h: u32,
        fps: u32,
        source: &livekit::webrtc::video_source::native::NativeVideoSource,
    ) -> std::result::Result<gst::Pipeline, String> {
        let pipeline = crate::lk_publish::build_capture_pipeline_cuda_learn(screencast, box_w, box_h, fps)
            .map_err(|e| format!("unavailable ({e})"))?;
        let stop = |p: &gst::Pipeline| {
            set_state_bounded(p.upcast_ref(), gst::State::Null, 1500, "screen capture");
        };
        let Some(appsink) = pipeline.by_name("sink").and_then(|s| s.downcast::<gstreamer_app::AppSink>().ok()) else {
            return Err("no appsink".into());
        };
        let flowing = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        if let Some(pad) = appsink.static_pad("sink") {
            let f = flowing.clone();
            pad.add_probe(gst::PadProbeType::BUFFER, move |_, _| {
                f.store(true, std::sync::atomic::Ordering::SeqCst);
                gst::PadProbeReturn::Remove
            });
        }
        crate::lk_publish::wire_appsink_to_source(&appsink, source.clone());
        if !set_state_bounded(pipeline.upcast_ref(), gst::State::Playing, 6000, "screen capture start") {
            stop(&pipeline);
            return Err("did not start".into());
        }
        let bus = pipeline.bus();
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(4000);
        while !flowing.load(std::sync::atomic::Ordering::SeqCst) {
            if std::time::Instant::now() >= deadline {
                stop(&pipeline);
                return Err("no frame in 4 s".into());
            }
            let err = bus.as_ref().and_then(|b| b.timed_pop_filtered(gst::ClockTime::from_mseconds(50), &[gst::MessageType::Error]));
            if let Some(msg) = err {
                let why = match msg.view() {
                    gst::MessageView::Error(e) => format!("error: {} ({:?})", e.error(), e.debug()),
                    _ => "error".to_string(),
                };
                stop(&pipeline);
                return Err(why);
            }
        }
        eprintln!("[call] screen-share capture: GPU (CUDA) scale, size from the first frame path");
        Ok(pipeline)
    }

    /// Bounded best-effort unpublish for "publish succeeded but capture failed" paths.
    /// Without it, peers keep a negotiated track that never emits a frame — a permanent
    /// black tile (video) or dead slot (audio) until the call ends.
    async fn unpublish_orphaned_track(&self, sid: livekit::id::TrackSid, what: &str) {
        eprintln!("[call] {what}: capture failed after publish — unpublishing orphaned track");
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            self.room.local_participant().unpublish_track(&sid),
        )
        .await;
    }

    /// Resolve the user's ShareEncoder choice against the backends libwebrtc actually
    /// offers — same ladder the camera publish uses (call.rs::publish_camera). An NVENC
    /// request on a machine where NVENC is absent or failing must NOT be pinned anyway:
    /// libwebrtc then silently falls back to software OpenH264, which is frequently not
    /// compiled in → the track negotiates H.264 but emits no frames → peers see a black
    /// share tile. Prefer VAAPI (still hardware H.264), else software VP9 (always present).
    fn resolve_share_encoder(
        encoder: crate::lk_publish::ShareEncoder,
    ) -> (livekit::options::VideoCodec, livekit::options::VideoEncoderBackend, &'static str) {
        // Single source of truth, shared with the connection probe. AV1+E2EE was
        // cracked 2026-08-12 (see the AV1 + E2EE note in the ladder): the patched
        // libwebrtc (tinhat-av1-e2ee-v2) wraps the ciphertext in a synthetic OBU,
        // so AV1 is offered for encrypted and unencrypted sessions alike.
        // Gate: TINHAT_E2EE_CODEC=av1 loopback must PASS before any webrtc bump.
        //
        // 2026-08-27: …unless a BROWSER is watching. livekit-client's
        // FrameCryptor throws "av1 is not yet supported for end to end encryption", so
        // a web / Element X viewer of an encrypted AV1 share can never decode it — it
        // PLIs forever, the SFU forwards one a second, and the encoder answers each
        // with a keyframe. Every native-only session in the field logs shows pli ≤ 2;
        // the one session with web viewers shows pli 6 → 3972 (key == pli), i.e. the
        // share stuttered for EVERYONE. H.264 decrypts in every client we have, so
        // that is what a mixed call gets. The app re-publishes a live AV1 share the
        // moment a browser joins (call_ctl::maybe_rescan_peers).
        let av1_ok = !browser_peers_present();
        if !av1_ok {
            eprintln!("[call] browser viewer in the call -> share codec pinned to H.264 (AV1 is native-only)");
        }
        crate::lk_publish::resolve_encoder_ladder(encoder, av1_ok)
    }

    /// Is the live screen share (if any) published as AV1? Drives the "a browser just
    /// joined — re-publish as H.264" decision in the app.
    pub fn screenshare_is_av1(&self) -> bool {
        self.screenshare.is_some() && self.share_is_av1
    }

    /// Publish a hardware-encoded (NVENC) screen-share into THIS call's Room (so it
    /// rides the same E2EE key + appears under our single participant). Mirrors the
    /// proven standalone publish, reusing its capture helpers.
    pub async fn publish_screenshare(&mut self, screencast: ScreenCast) -> Result<()> {
        use livekit::options::{VideoEncoderBackend, VideoEncoding};
        use livekit::track::LocalVideoTrack;
        use livekit::webrtc::video_source::native::NativeVideoSource;
        use livekit::webrtc::video_source::{RtcVideoSource, VideoResolution};

        if self.screenshare.is_some() {
            return Ok(());
        }
        // Publish at the source's REAL aspect ratio (fit into the quality box) so an
        // ultrawide isn't squished into 16:9; honour the user's quality/fps/bitrate.
        let q = crate::lk_publish::share_quality();
        let (tw, th, fps, bitrate_bps) = crate::lk_publish::publish_target(screencast.size(), q);
        // Encoder choice (see ShareEncoder): NVENC H.264 = GPU/hardware, ideal for motion
        // & gaming; software VP9 = sharpest text, no GPU. Browsers can decode both.
        // Resolved against the available backends (NVENC -> VAAPI -> VP9 ladder).
        let (codec, backend, codec_label) = Self::resolve_share_encoder(q.encoder);
        let backends: Vec<_> = VideoEncoderBackend::list_available().into_iter().collect();
        eprintln!(
            "[call] screen-share source {:?} -> publishing {tw}x{th}@{fps} ~{}kbps as {codec_label}; encoder backends={:?}",
            screencast.size(),
            bitrate_bps / 1000,
            backends
        );
        let source = NativeVideoSource::new(VideoResolution { width: tw, height: th }, true);
        let track = LocalVideoTrack::create_video_track("screen", RtcVideoSource::Native(source.clone()));
        let stats_track = track.clone();
        // Seed the bandwidth estimator for the renegotiation this publish
        // triggers (the webrtc-sys SetLocalDescription patch reads this env var).
        // Without it GCC starts near zero and climbs ~8-10%/s, so every share spent
        // its first minutes at ~6 Mbps regardless of preset, machine, or link.
        // Seed = the preset, clamped to 15 Mbps: enough to kill the crawl outright
        // (every non-Unleashed preset seeds at full value), while an Unleashed pick
        // on a weak uplink overshoots by at most ~15M for the ~1s the delay-based
        // controller needs to adapt down — audio survives that (priority pacing).
        let seed_kbps = (bitrate_bps / 1000).min(15_000);
        std::env::set_var("LK_BWE_START_KBPS", seed_kbps.to_string());
        eprintln!("[call] seeding BWE at {seed_kbps} kbps for the share publish");
        let publish_result = self
            .room
            .local_participant()
            .publish_track(
                LocalTrack::Video(track),
                TrackPublishOptions {
                    video_codec: codec,
                    // Pin the encoder backend so a VP9 request stays on software and an
                    // H.264 request goes to NVENC (not libwebrtc's blocky software OpenH264).
                    video_encoder: backend,
                    source: TrackSource::Screenshare,
                    video_encoding: Some(VideoEncoding {
                        max_bitrate: bitrate_bps,
                        max_framerate: fps as f64,
                    }),
                    simulcast: false, // single sharp layer for screen text
                    ..Default::default()
                },
            )
            .await;
        // Clear the seed on a 3s DELAY: the engine debounces publisher renegotiation,
        // so the SetLocalDescription that reads the var fires AFTER publish_track
        // resolves — an immediate remove_var races it and the seed silently never
        // applies (measured). 3s is still early enough that a camera toggle in the
        // same breath can't re-seed a meaningfully-grown estimate downward.
        tokio::spawn(async {
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            std::env::remove_var("LK_BWE_START_KBPS");
        });
        let publication = publish_result.context("publish screenshare failed")?;

        let pipeline = match Self::start_share_capture(&screencast, q, tw, th, fps, source) {
            Ok(p) => p,
            Err(e) => {
                self.unpublish_orphaned_track(publication.sid(), "screen-share").await;
                return Err(e);
            }
        };
        // Auto-end when the capture dies on its own (shared window closed): the hook
        // (installed by the app; Windows-only today) stops the share instead of the
        // old freeze-last-frame-forever behavior. Deliberate stop/restart never fires
        // it (the stop flag flips first — watch_pipeline_bus_notify checks it).
        let on_dead: Option<Box<dyn FnOnce() + Send>> = self
            .share_dead_hook
            .clone()
            .map(|h| Box::new(move || h()) as Box<dyn FnOnce() + Send>);
        let bus_stop = crate::lk_publish::watch_pipeline_bus_notify(&pipeline, on_dead);
        // Call-lifetime publisher stats (legacy [lk-stats] 3s log — the ship-gate format —
        // plus the in-call "stream health" feed). Shares the bus watcher's stop flag, so
        // stop/restart/close all end it. Replaces the old 16x3s bounded diagnostic loop.
        spawn_publish_stats_task(
            "screen",
            codec_label.to_string(),
            livekit::track::Track::LocalVideo(stats_track),
            bus_stop.clone(),
        );
        eprintln!("[call] screen-share published");
        std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_secs(3));
            pw_snapshot("share started");
        });
        clear_republished_sid("screen"); // fresh publish: the cached sid is current again
        self.screenshare = Some((pipeline, screencast, publication.sid(), bus_stop));
        self.announce_recording_only().await;
        self.share_is_av1 = matches!(codec, livekit::options::VideoCodec::AV1);
        Ok(())
    }

    /// Stop sharing the screen (unpublish the track, stop capture, close the portal).
    pub async fn stop_screenshare(&mut self) {
        if let Some((pipeline, screencast, sid, bus_stop)) = self.screenshare.take() {
            bus_stop.store(true, std::sync::atomic::Ordering::Relaxed); // let the bus thread exit
            set_state_bounded(pipeline.upcast_ref(), gst::State::Null, 1500, "screen capture");
            let _ = self.room.local_participant().unpublish_track(&live_sid("screen", &sid)).await;
            screencast.close().await;
            eprintln!("[call] screen-share stopped");
            self.announce_recording_only().await;
        }
    }

    /// Tell peers whether the live share is "for the recording only" — the
    /// participant attribute carries the share sid ("" = no recording-only share).
    pub async fn announce_recording_only(&self) {
        let sid = match (&self.screenshare, share_recording_only()) {
            (Some((_, _, sid, _)), true) => sid.as_str().to_string(),
            _ => String::new(),
        };
        *my_rec_only_sid().lock().unwrap() = if sid.is_empty() { None } else { Some(sid.clone()) };
        match self.room.local_participant().publish_data(rec_only_packet(&sid)).await {
            Ok(()) => eprintln!("[call] recording-only announced to peers: {sid:?}"),
            Err(e) => eprintln!("[call] recording-only announce FAILED: {e}"),
        }
    }

    /// Tell peers our current game-chat flag (`{"on":bool}` on `tinhat.game_chat`).
    pub async fn announce_game_chat(&self) {
        let on = local_game_chat();
        match self.room.local_participant().publish_data(game_chat_packet(on)).await {
            Ok(()) => eprintln!("[call] game chat announced to peers: {on}"),
            Err(e) => eprintln!("[call] game chat announce FAILED: {e}"),
        }
    }

    /// 9.4: tell peers which corner I am in (`{"corner":"<id>"}`, plus a pull target).
    pub async fn announce_corner(&self, id: &str, pull: &str) {
        match self.room.local_participant().publish_data(corner_packet(id, pull)).await {
            Ok(()) => eprintln!("[call] corner announced to peers: {id:?} pull={pull:?}"),
            Err(e) => eprintln!("[call] corner announce FAILED: {e}"),
        }
    }

    /// Is our mic track published (RBAC: a server-muted join skips the publish, so the
    /// unmute path must PUBLISH rather than re-publish).
    pub fn has_mic(&self) -> bool {
        self.mic.is_some()
    }

    /// Is a screen-share currently active in this call?
    pub fn is_sharing(&self) -> bool {
        self.screenshare.is_some()
    }

    /// Re-publish the LIVE screen-share with the CURRENT quality (resolution / fps /
    /// bitrate / encoder), WITHOUT re-popping the portal picker. NO-OP if not sharing.
    ///
    /// Why a full re-publish (mirrors [`republish_mic`], not [`restart_mic`]): every
    /// quality knob bakes into objects fixed at publish time, none of which a live track
    /// exposes a setter for —
    ///   * resolution → the `NativeVideoSource` dimensions + the capture caps,
    ///   * fps + bitrate → the `VideoEncoding` passed to `publish_track`,
    ///   * encoder → the `video_encoder` backend (NVENC H.264 vs software VP9, which also
    ///     flips the negotiated codec).
    /// libwebrtc's `VideoEncoding`/codec is locked once the track is published; there is no
    /// `set_max_bitrate`/`set_framerate` on `LocalVideoTrack`, and switching codec/backend
    /// is impossible without re-negotiation. So we unpublish + re-publish.
    ///
    /// LiveKit implication: the new track gets a NEW sid; peers see TrackUnsubscribed then
    /// TrackSubscribed and re-subscribe (a brief tile flicker on their end). That's the
    /// same contract `republish_mic` already relies on.
    ///
    /// CRITICAL: the portal `ScreenCast` is REUSED (its fd/node_id stay valid for the life
    /// of the D-Bus session), so the user is NOT re-prompted. We move it out of the tuple,
    /// rebuild against its existing fd/node_id, and store it back under the new sid.
    /// Bounded: the unpublish is wrapped in a 3s timeout so a dead SFU can't hang the caller.
    pub async fn restart_screenshare(&mut self) -> Result<()> {
        use livekit::options::VideoEncoding;
        use livekit::track::LocalVideoTrack;
        use livekit::webrtc::video_source::native::NativeVideoSource;
        use livekit::webrtc::video_source::{RtcVideoSource, VideoResolution};

        // Take the live share apart; if not sharing, nothing to do.
        let Some((old_pipeline, screencast, old_sid, old_bus_stop)) = self.screenshare.take() else {
            return Ok(());
        };

        // 1) Stop the old capture IMMEDIATELY (NULL the pipeline) so the readback loop and
        //    encoder feed halt before we touch the network — same ordering as close().
        old_bus_stop.store(true, std::sync::atomic::Ordering::Relaxed); // let the old bus thread exit
        let _ = old_pipeline.set_state(gst::State::Null);

        // 2) Unpublish the old track, BOUNDED so a dead SFU can't park us forever.
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            self.room.local_participant().unpublish_track(&live_sid("screen", &old_sid)),
        )
        .await;

        // 3) Re-publish at the NEW quality, reusing the SAME portal cast (no re-prompt).
        let q = crate::lk_publish::share_quality();
        let (tw, th, fps, bitrate_bps) = crate::lk_publish::publish_target(screencast.size(), q);
        let (codec, backend, codec_label) = Self::resolve_share_encoder(q.encoder);
        eprintln!(
            "[call] re-publishing screen-share at NEW quality {tw}x{th}@{fps} ~{}kbps as {codec_label}",
            bitrate_bps / 1000
        );

        let source = NativeVideoSource::new(VideoResolution { width: tw, height: th }, true);
        let track =
            LocalVideoTrack::create_video_track("screen", RtcVideoSource::Native(source.clone()));
        let stats_track = track.clone();
        let publication = self
            .room
            .local_participant()
            .publish_track(
                LocalTrack::Video(track),
                TrackPublishOptions {
                    video_codec: codec,
                    video_encoder: backend,
                    source: TrackSource::Screenshare,
                    video_encoding: Some(VideoEncoding {
                        max_bitrate: bitrate_bps,
                        max_framerate: fps as f64,
                    }),
                    simulcast: false,
                    ..Default::default()
                },
            )
            .await
            .context("re-publish screenshare failed")?;

        // 4) Rebuild + start the capture pipeline against the EXISTING portal fd/node_id.
        let pipeline = match Self::start_share_capture(&screencast, q, tw, th, fps, source) {
            Ok(p) => p,
            Err(e) => {
                self.unpublish_orphaned_track(publication.sid(), "screen-share restart").await;
                return Err(e);
            }
        };
        // Auto-end when the capture dies on its own (shared window closed): the hook
        // (installed by the app; Windows-only today) stops the share instead of the
        // old freeze-last-frame-forever behavior. Deliberate stop/restart never fires
        // it (the stop flag flips first — watch_pipeline_bus_notify checks it).
        let on_dead: Option<Box<dyn FnOnce() + Send>> = self
            .share_dead_hook
            .clone()
            .map(|h| Box::new(move || h()) as Box<dyn FnOnce() + Send>);
        let bus_stop = crate::lk_publish::watch_pipeline_bus_notify(&pipeline, on_dead);
        // Fresh stats task for the fresh track (the old one exited via old_bus_stop).
        spawn_publish_stats_task(
            "screen",
            codec_label.to_string(),
            livekit::track::Track::LocalVideo(stats_track),
            bus_stop.clone(),
        );
        eprintln!("[call] screen-share re-published with new quality");
        clear_republished_sid("screen"); // fresh publish: the cached sid is current again
        self.screenshare = Some((pipeline, screencast, publication.sid(), bus_stop));
        self.announce_recording_only().await;
        self.share_is_av1 = matches!(codec, livekit::options::VideoCodec::AV1);
        Ok(())
    }

    /// Publish the local camera as our video track (TrackSource::Camera) into THIS
    /// call's Room (same E2EE key, same single participant). Mirrors
    /// `publish_screenshare`. If `with_selfview` the capture pipeline carries a
    /// `qml6glsink` self-view branch and is returned in NULL (the app sets it PLAYING
    /// on the GUI thread via the C++ attach, like a remote tile); otherwise the
    /// pipeline is set PLAYING here. Returns the pipeline ptr (usize) for the GUI
    /// attach when a self-view was built, else 0.
    pub async fn publish_camera(&mut self, device: Option<String>, with_selfview: bool) -> Result<usize> {
        use livekit::options::{VideoCodec, VideoEncoderBackend, VideoEncoding};
        use livekit::track::LocalVideoTrack;
        use livekit::webrtc::video_source::native::NativeVideoSource;
        use livekit::webrtc::video_source::{RtcVideoSource, VideoResolution};

        if self.camera.is_some() {
            return Ok(0);
        }
        if device.is_some() {
            set_camera_target(device.clone());
        }
        let dev = device.or_else(|| camera_target().lock().unwrap().clone());

        // Resolve the user's quality request against the device's REAL modes: >720p only
        // engages when a strictly-larger capture mode exists (the active-downscale
        // invariant), else this returns the proven 1280x720 (see camera::effective_output).
        let q = camera::cam_quality();
        let (cam_w, cam_h, cam_fps) = camera::effective_output(dev.as_deref());
        let source = NativeVideoSource::new(
            VideoResolution { width: cam_w, height: cam_h },
            // is_screencast=FALSE: this is a camera, not screen content. True (copied from
            // the screen-share path) pins maintain-resolution degradation + VP9 screen-
            // content tools — wrong for a noisy webcam (choppy under pressure, wasted bits).
            false,
        );
        let track = LocalVideoTrack::create_video_track("camera", RtcVideoSource::Native(source.clone()));
        let stats_track = track.clone();

        // Pin the encoder exactly like the PROVEN screen-share path. The camera used to
        // publish H.264 with NO backend pinned → libwebrtc's default, which for H.264 is
        // software OpenH264 — a codec frequently NOT compiled into the libwebrtc build.
        // So the track negotiated H.264 but emitted no frames → peers saw a blank tile
        // even though our self-view (pre-encoder, local tee) worked. Prefer hardware
        // H.264 (NVENC, else VAAPI); fall back to software VP9 (always present) so
        // AMD / no-hardware friends still publish a visible camera.
        let backends: Vec<_> = VideoEncoderBackend::list_available().into_iter().collect();
        let (codec, backend) = if backends.iter().any(|b| matches!(b, VideoEncoderBackend::Nvenc)) {
            (VideoCodec::H264, VideoEncoderBackend::Nvenc)
        } else if backends.iter().any(|b| matches!(b, VideoEncoderBackend::Vaapi)) {
            (VideoCodec::H264, VideoEncoderBackend::Vaapi)
        } else {
            (VideoCodec::VP9, VideoEncoderBackend::Software)
        };
        // SIMULCAST (adaptive layers): NVENC-only for now — the SimulcastEncoderAdapter
        // instantiates one encoder PER RUNG, and 3 software encoders on a weak machine is
        // real CPU (plus the VAAPI path is unvalidated under the adapter). Explicit H180 +
        // H360 rungs under the user's top layer; every rung is plain H.264 constrained-
        // baseline, so web (Element Call) subscribers' adaptiveStream picks rungs with no
        // web-side changes. Gate-validated via TINHAT_CAM_SIMTEST (per-rid outbound stats).
        let simulcast_on = q.simulcast && matches!(backend, VideoEncoderBackend::Nvenc);
        eprintln!(
            "[call] publishing camera as {codec:?}/{backend:?} {cam_w}x{cam_h}@{cam_fps} \
             target={}kbps simulcast={simulcast_on} (backends={backends:?})",
            q.bitrate_kbps
        );
        // Publish FIRST (negotiates from config, not frames), THEN start capture —
        // same ordering the screen-share publish proved necessary.
        let publication = self
            .room
            .local_participant()
            .publish_track(
                LocalTrack::Video(track),
                TrackPublishOptions {
                    video_codec: codec,
                    // Pin the backend (avoid the absent/blocky software OpenH264 default).
                    video_encoder: backend,
                    source: TrackSource::Camera,
                    video_encoding: Some(VideoEncoding {
                        max_bitrate: (q.bitrate_kbps as u64) * 1000,
                        max_framerate: cam_fps as f64,
                    }),
                    simulcast: simulcast_on,
                    simulcast_layers: simulcast_on
                        .then(|| camera_simulcast_layers(cam_w, cam_h)),
                    ..Default::default()
                },
            )
            .await
            .context("publish camera failed")?;

        // Outbound-RTP stats (mirrors screen-share): call-lifetime task — proves the
        // camera is actually ENCODING + sending, one legacy [lk-stats-cam] line PER RID
        // under simulcast (the TINHAT_CAM_SIMTEST ship-gate signal that every rung is
        // really encoding; format preserved verbatim), and feeds the in-call "stream
        // health" UI one entry per rid. The stop flag is the bus watcher's, cloned in
        // below once the capture pipeline exists.
        let cam_enc_label = format!("{codec:?}/{backend:?}");

        // With a self-view the pipeline carries a tee+qml6glsink and is left in NULL
        // for the GUI thread to start; the publish appsink is "pubsink". Without, it's
        // capture-only with the appsink named "sink".
        let capture = (|| -> Result<(gst::Pipeline, gstreamer_app::AppSink)> {
            let (pipeline, appsink_name) = if with_selfview {
                (camera::build_camera_selfview_pipeline(dev.as_deref())?, "pubsink")
            } else {
                (camera::build_camera_capture_pipeline(dev.as_deref())?, "sink")
            };
            let appsink = pipeline
                .by_name(appsink_name)
                .context("camera appsink missing")?
                .downcast::<gstreamer_app::AppSink>()
                .map_err(|_| anyhow::anyhow!("camera sink not an AppSink"))?;
            Ok((pipeline, appsink))
        })();
        let (pipeline, appsink) = match capture {
            Ok(v) => v,
            Err(e) => {
                self.unpublish_orphaned_track(publication.sid(), "camera").await;
                return Err(e);
            }
        };
        // Reuse the proven screen-share frame pump verbatim — it's generic over any
        // I420 appsink feeding a NativeVideoSource.
        crate::lk_publish::wire_appsink_to_source_ext(&appsink, source.clone(), "camera", self.camera_quiet_hook.clone());
        let cam_on_dead: Option<Box<dyn FnOnce() + Send>> = self
            .camera_dead_hook
            .clone()
            .map(|h| Box::new(move || h()) as Box<dyn FnOnce() + Send>);
        let bus_stop = crate::lk_publish::watch_pipeline_bus_notify(&pipeline, cam_on_dead);
        // Own flag, NOT bus_stop: the stats stream belongs to the TRACK, which survives
        // restart_camera's pipeline rebuild (see CameraHandle.stats_stop).
        let stats_stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        spawn_publish_stats_task(
            "camera",
            cam_enc_label,
            livekit::track::Track::LocalVideo(stats_track),
            stats_stop.clone(),
        );

        let ptr = if with_selfview {
            // Leave in NULL; the app sets it PLAYING on the GUI thread (qml6glsink GL
            // affinity). Return the ptr so attach_video can bind the self-view "sink".
            pipeline.as_ptr() as usize
        } else {
            match pipeline.set_state(gst::State::Playing).context("camera capture start failed") {
                Ok(_) => 0,
                Err(e) => {
                    bus_stop.store(true, std::sync::atomic::Ordering::Relaxed);
                    stats_stop.store(true, std::sync::atomic::Ordering::Relaxed); // task never stored — end it here
                    let _ = pipeline.set_state(gst::State::Null);
                    self.unpublish_orphaned_track(publication.sid(), "camera").await;
                    return Err(e);
                }
            }
        };
        eprintln!(
            "[call] camera published sid={} (selfview={with_selfview}, device={dev:?}, simulcast={simulcast_on})",
            publication.sid()
        );
        clear_republished_sid("camera"); // fresh publish: the cached sid is current again
        self.camera = Some(CameraHandle {
            pipeline,
            source,
            sid: publication.sid(),
            device: dev,
            with_selfview,
            bus_stop,
            stats_stop,
        });
        Ok(ptr)
    }

    /// Stop the camera (unpublish the track, stop + null the capture pipeline).
    pub async fn unpublish_camera(&mut self) {
        if let Some(h) = self.camera.take() {
            h.bus_stop.store(true, std::sync::atomic::Ordering::Relaxed); // let the bus thread exit
            h.stats_stop.store(true, std::sync::atomic::Ordering::Relaxed); // end the publish-stats task
            // Bounded: toggle_camera holds the current_call mutex across this call, so a
            // wedged driver here used to pin the mutex (and with it hang-up) for good.
            set_state_bounded(h.pipeline.upcast_ref(), gst::State::Null, 1500, "camera capture"); // release /dev/video0 first
            // BOUND the SFU unpublish: toggle_camera (e.g. BRB turning the camera off) holds the
            // current_call async-mutex ACROSS this await, so on a dying SFU an unbounded
            // unpublish_track would pin the mutex forever — and then teardown_call_session /
            // leave_on_exit can never take() the call to close it (the zombie). 3s cap = the
            // mutex is always released, so quit/teardown can proceed.
            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(3),
                self.room.local_participant().unpublish_track(&live_sid("camera", &h.sid)),
            )
            .await;
            eprintln!("[call] camera stopped");
        }
    }

    /// Is the local camera currently published?
    pub fn has_camera(&self) -> bool {
        self.camera.is_some()
    }

    /// Mute/unmute the camera by pausing/playing its capture pipeline (track stays
    /// published, just stops sending frames). Mirrors `set_mic_muted`.
    pub fn set_camera_muted(&self, muted: bool) {
        if let Some(h) = &self.camera {
            let _ = h.pipeline.set_state(if muted { gst::State::Paused } else { gst::State::Playing });
        }
    }

    /// Re-point the camera at the currently-selected device (after
    /// `set_camera_target`): rebuild only the capture pipeline, feeding the SAME
    /// published track + (if it had one) self-view. Returns the new pipeline ptr when
    /// a self-view was rebuilt (the app re-attaches it on the GUI thread), else 0.
    pub fn restart_camera(&mut self) -> Result<usize> {
        let Some(h) = self.camera.as_mut() else { return Ok(0) };
        // Flip the OLD watcher's flag alongside the NULL (a stale-true flag on the NULLed
        // pipeline is harmless if the rebuild below fails).
        h.bus_stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = h.pipeline.set_state(gst::State::Null);
        let dev = camera_target().lock().unwrap().clone();
        let (pipeline, appsink_name) = if h.with_selfview {
            (camera::build_camera_selfview_pipeline(dev.as_deref())?, "pubsink")
        } else {
            (camera::build_camera_capture_pipeline(dev.as_deref())?, "sink")
        };
        let appsink = pipeline
            .by_name(appsink_name)
            .context("camera appsink missing")?
            .downcast::<gstreamer_app::AppSink>()
            .map_err(|_| anyhow::anyhow!("camera sink not an AppSink"))?;
        crate::lk_publish::wire_appsink_to_source_ext(&appsink, h.source.clone(), "camera", self.camera_quiet_hook.clone());
        // NOTIFY watcher, same dead-hook as the original pipeline: the plain watcher
        // here meant a RETRIED pipeline's death was silent — camera_on stayed true and
        // the tile sat black with no retry and no message (NVIDIA Broadcast, 2026-08-17
        // field logs). With the hook re-armed, a second death within the hook's 10s
        // cooldown surfaces the "camera keeps dying" guidance and turns the camera off.
        let bus_stop = match self.camera_dead_hook.clone() {
            Some(hook) => crate::lk_publish::watch_pipeline_bus_notify(
                &pipeline,
                Some(Box::new(move || hook())),
            ),
            None => crate::lk_publish::watch_pipeline_bus(&pipeline),
        };
        let ptr = if h.with_selfview {
            pipeline.as_ptr() as usize
        } else {
            // One retry after a beat: unlike the mic, the camera is EXCLUSIVE-open
            // (same-device restarts — quality toggles — NEED the NULL-first order
            // above), and some drivers release the device a moment after NULL.
            if pipeline.set_state(gst::State::Playing).is_err() {
                std::thread::sleep(std::time::Duration::from_millis(300));
                pipeline.set_state(gst::State::Playing).context("camera restart failed")?;
            }
            0
        };
        h.pipeline = pipeline;
        h.device = dev;
        h.bus_stop = bus_stop;
        eprintln!("[call] camera re-pointed to selected device");
        Ok(ptr)
    }

    /// Mute/unmute the mic. MUST go through the PUBLICATION (`LocalTrackPublication::
    /// mute/unmute`), which sends a MuteTrackRequest to the SFU — the raw `LocalAudioTrack::
    /// mute/unmute` only flips a local `rtc_track.set_enabled` flag the SDK never signals, so
    /// the SFU keeps relaying the (now silent or stale) track and an unmute never re-opens the
    /// audio to peers. This was why coming back from BRB (and manual unmute) left us silent to
    /// the phone. Falls back to the raw track only if the publication can't be resolved.
    pub fn set_mic_muted(&self, muted: bool) {
        crate::hear::set_mic_muted(muted); // 10.5: muted talk / "not coming through" judge by it
        crate::sameroom::set_mic_muted(muted); // 11.3: the room-mic gate yields to the user's mute
        if let Some(h) = &self.mic {
            match self.room.local_participant().get_track_publication(&live_sid("mic", &h.sid)) {
                Some(pubn) => {
                    if muted {
                        pubn.mute();
                    } else {
                        pubn.unmute();
                    }
                }
                None => {
                    if muted {
                        h.track.mute();
                    } else {
                        h.track.unmute();
                    }
                }
            }
            // Keep the capture pipeline PLAYING even when muted. Muting the publication/track
            // above already stops transmission to peers; PAUSING pipewiresrc on mute left it
            // unable to resume capture on unmute (the mic stayed silent after BRB/un-mute —
            // `[mic] capturing` never restarted, esp. post PipeWire 1.6.7). Capturing-while-muted
            // is cheap and makes unmute instant + reliable (nothing to "resume").
            let _ = h.pipeline.set_state(gst::State::Playing);
        }
    }

    /// Capture the default mic and publish it as our audio track.
    pub async fn publish_mic(&mut self) -> Result<()> {
        self.publish_mic_muted(false).await
    }

    /// Publish the mic, optionally MUTED FROM THE FIRST FRAME. `start_muted` mutes the raw
    /// track (rtc_track.set_enabled(false)) BEFORE publish + before capture reaches Playing, so
    /// (a) the SFU announces the track muted (AddTrackReq.muted = is_muted()) and (b) no audio
    /// ever transmits in the gap between publish and a post-hoc mute. Without this, re-publishing
    /// while muted (an AGC/NS toggle during BRB/mute) leaked a sub-second of live mic to peers.
    pub async fn publish_mic_muted(&mut self, start_muted: bool) -> Result<()> {
        reset_dsp_stage_block(); // 0.8.28: the WebRTC-stage ladder is per call
        if self.mic.is_some() {
            return Ok(());
        }
        // Ensure echo cancellation is up BEFORE capture, so the mic pipeline binds to the
        // cancelled virtual source. On a normal join this is an idempotent no-op — connect()
        // already brought AEC up before any playback pipeline was built. It RELOADS when the
        // binding changed (an AGC toggle re-publish: the module's gain-control arg follows
        // mic_agc), and in that case the live playback sinks are attached to the DEAD old
        // node by name and must be NULL-cycled onto the recreated reference sink.
        if aec_start() {
            let _ = repoint_playback_sinks(effective_playback_node().unwrap_or_default());
        }
        // These AudioSourceOptions are INERT on the queue_size_ms=0 fast path: the vendored
        // webrtc-sys InternalSource stores them but never runs an APM over injected frames
        // (capture_frame hands buffers straight to the track sinks). They are kept only so
        // a future non-fast-path source picks up the user's persisted choices. The gain
        // control users actually HEAR is the EC module's webrtc.gain_control (aec.rs),
        // tied to the same mic_agc() toggle; noise suppression is RNNoise (mic_denoise) in
        // the capture pipeline. libwebrtc echo_cancellation stays hard-off (it could only
        // cancel audio libwebrtc itself played — ours plays via GStreamer/PipeWire).
        let source = NativeAudioSource::new(
            AudioSourceOptions {
                echo_cancellation: false,
                noise_suppression: mic_noise_suppression(),
                auto_gain_control: mic_agc(),
            },
            SAMPLE_RATE,
            1,
            0, // FAST PATH: queue_size_ms=0 => libwebrtc has NO ring, NO 10ms system-clock
               // RepeatingTask, and NO hard-zero silence splice (the confirmed crackle root
               // cause is DELETED). capture_frame() runs OnData synchronously on OUR cadence;
               // start_mic_capture's monotonic 10ms feeder is the sole pacing clock and the
               // far-end NetEq absorbs residual USB capture-clock drift. We push EXACT 480.
        );
        let track = LocalAudioTrack::create_audio_track("mic", RtcAudioSource::Native(source.clone()));
        let mute_track = track.clone(); // handle kept for mute signaling
        if start_muted {
            mute_track.mute(); // disable the rtc_track BEFORE publish -> announced muted, silent
        }
        let publication = self
            .room
            .local_participant()
            .publish_track(
                LocalTrack::Audio(track),
                TrackPublishOptions { source: TrackSource::Microphone, ..Default::default() },
            )
            .await
            .context("publish mic failed")?;

        let (pipeline, task) = match start_mic_capture(source.clone()) {
            Ok(v) => v,
            Err(e) => {
                self.unpublish_orphaned_track(publication.sid(), "mic").await;
                return Err(e);
            }
        };
        // NOTE: deliberately NOT pausing the pipeline when start_muted — the publication
        // mute above already guarantees silence to peers (AddTrackReq.muted + no track
        // output), and a PAUSED pipewiresrc is the documented can't-resume failure mode
        // (set_mic_muted's own comment: "the mic stayed silent after BRB/un-mute", esp.
        // post PipeWire 1.6.7). Capturing-while-muted is what set_mic_muted itself does;
        // an AGC/NS re-publish during mute/BRB now behaves identically.
        let _ = start_muted; // (kept for the publication-mute above)
        eprintln!(
            "[call] mic published + capturing (agc={}, ns={}, aec={}, muted={start_muted})",
            mic_agc(),
            mic_ns_mode().as_str(),
            if dsp_aec_active() {
                "dsp"
            } else if aec_nodes().lock().unwrap().is_some() {
                "module"
            } else {
                "off"
            }
        );
        crate::hear::set_mic_muted(start_muted);
        mic_wanted_flag().store(true, std::sync::atomic::Ordering::Relaxed); // 10.5: see ensure_mic
        clear_republished_sid("mic"); // fresh publish: the cached sid is current again
        // Call-lifetime mic stats for the "stream health" UI (send bitrate + the SFU's
        // receiver-report loss/RTT — the "is MY audio arriving badly" signal).
        let stats_stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        spawn_publish_stats_task(
            "mic",
            "Opus".to_string(),
            livekit::track::Track::LocalAudio(mute_track.clone()),
            stats_stop.clone(),
        );
        self.mic = Some(MicHandle {
            pipeline,
            task,
            source,
            track: mute_track,
            sid: publication.sid(),
            stats_stop,
        });
        Ok(())
    }

    /// Re-publish the mic with the CURRENT APM settings (AGC/NS). Those are NativeAudioSource
    /// constructor options, so unlike [`restart_mic`] (which only rebuilds the capture pipeline)
    /// this needs a fresh source + track: unpublish the old one (bounded), then publish anew —
    /// PRE-MUTED if we were muted, so the re-publish never leaks live mic to peers. NO-OP if not
    /// currently published.
    pub async fn republish_mic(&mut self, was_muted: bool) -> Result<()> {
        if self.mic.is_none() {
            return Ok(());
        }
        if let Some(h) = self.mic.take() {
            h.task.abort();
            // 10.5 (audio review #5): BOUNDED. This runs under the call mutex (an AGC/NS toggle),
            // and it was the last synchronous mic NULL left after 10.0.4 — a source that never
            // finishes stopping froze every call control, hang-up included.
            set_state_bounded(h.pipeline.upcast_ref(), gst::State::Null, 1500, "mic republish");
            h.stats_stop.store(true, std::sync::atomic::Ordering::Relaxed);
            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(3),
                self.room.local_participant().unpublish_track(&live_sid("mic", &h.sid)),
            )
            .await;
        }
        let r = self.publish_mic_muted(was_muted).await;
        if let Err(e) = &r {
            // The old mic is gone and the new one did not come up. It used to stay that way, with
            // one log line; now the lifetime healer sees mic_wanted() && !has_mic() and publishes
            // it again (ensure_mic), and the UI's receipts say nobody receives us meanwhile.
            eprintln!("[call] mic re-publish failed ({e:#}) — the healer will publish it again");
            let _ = raise_mic_unhealthy();
        }
        r
    }

    /// 10.5 (audio review #5 / #6): make sure the mic we mean to have is PUBLISHED — after a failed
    /// re-publish (no handle) or a full reconnect whose republish failed (the SDK still reports
    /// Reconnected), with the user's current mute intent. Returns true when it re-published.
    pub async fn ensure_mic(&mut self) -> Result<bool> {
        let muted = crate::hear::mic_muted_now();
        if !mic_wanted() {
            return Ok(false);
        }
        let published = match &self.mic {
            None => false,
            Some(h) => self.room.local_participant().get_track_publication(&live_sid("mic", &h.sid)).is_some(),
        };
        if published {
            return Ok(false);
        }
        eprintln!(
            "[call] the mic is not published ({}) — publishing it again",
            if self.mic.is_none() { "no mic after a failed re-publish" } else { "its publication is gone" }
        );
        if let Some(h) = self.mic.take() {
            h.task.abort();
            set_state_bounded(h.pipeline.upcast_ref(), gst::State::Null, 1500, "mic (re-ensure)");
            h.stats_stop.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.publish_mic_muted(muted || i_am_server_muted()).await?;
        Ok(true)
    }

    /// Apply an echo-cancellation toggle or an audio DEVICE change to a LIVE call: rebind
    /// the AEC module to the current devices/AGC, re-point every playback sink at the
    /// effective output node, and rebuild the mic capture so it binds the effective input
    /// node. The published track/source are reused, so peers see no re-negotiation.
    ///
    /// Direction matters:
    ///  * enabling / re-binding: (re)load the module FIRST, then NULL-cycle the sinks onto
    ///    the (new) EC sink. A bare set_property("target-object") is NOT enough —
    ///    pipewiresink only reads it on a pw_stream reconnect (see repoint_playback_sinks;
    ///    the old code here did exactly that no-op, which is why toggling AEC ON mid-call
    ///    left playback on the real sink: echo stayed AND the canceller adapted against an
    ///    empty reference, mangling outgoing speech).
    ///  * disabling: re-point the sinks AWAY first and WAIT for it, THEN unload — the
    ///    module's nodes must never be destroyed under actively-streaming sinks.
    pub fn apply_aec(&mut self) -> Result<()> {
        if aec_tier() == AecTier::Dsp {
            // In-app canceller (both platforms): re-evaluate the reference binding (current
            // output, headset skip) or drop it when the toggle is off. Nothing to re-point —
            // playback always runs on the real device; the mic rebuild below carries (or
            // omits) webrtcdsp AND its reference branch (0.8.31: same pipeline).
            if crate::aec::enabled() {
                let _ = aec_start();
            } else {
                aec_stop();
            }
            return self.restart_mic();
        }
        if crate::aec::enabled() {
            if aec_start() {
                // DETACHED: nothing is destroyed on this path (the node the sinks are
                // heading TO was just created), and restart_mic below doesn't depend on
                // the re-point — joining here would only hold the current_call mutex
                // (which every call control queues on) through one PipeWire round-trip
                // per live sink for no benefit.
                let _ = repoint_playback_sinks(effective_playback_node().unwrap_or_default());
            }
        } else {
            // OFF: move playback AWAY from the EC sink and WAIT for the full walk BEFORE
            // destroying it. aec_nodes deliberately stays Some until the walk completes:
            // while it is Some, set_playback_target's guard keeps a concurrent output
            // pick from spawning a competing (generation-bumping) re-point that would
            // abandon ours mid-list and let unload() destroy the node under still-
            // attached sinks. The pick still lands in playback_target, which each retry
            // re-reads — so the sinks converge onto the newest choice.
            #[cfg(not(windows))]
            {
                for attempt in 0..3 {
                    let target = playback_target().lock().unwrap().clone().unwrap_or_default();
                    match repoint_playback_sinks(target).join() {
                        Ok(true) => break,
                        Ok(false) if attempt < 2 => continue, // superseded — walk again
                        _ => break,
                    }
                }
                *aec_nodes().lock().unwrap() = None; // effective_* now answer with the raw devices
                crate::aec::unload();
            }
            // Windows: no virtual nodes exist, so there is nothing to re-point —
            // playback always runs on the real endpoints. Just drop the loopback
            // reference tap; the mic rebuild below then leaves webrtcdsp out.
            // (The still-running old mic pipeline keeps a gst ref on the probe
            // element until restart_mic NULLs it, so teardown order is safe.)
            #[cfg(windows)]
            aec_stop();
        }
        self.restart_mic()
    }

    /// Re-point the mic at the currently-selected input (after `set_mic_target`):
    /// rebuild only the capture pipeline, feeding the SAME published track.
    pub fn restart_mic(&mut self) -> Result<()> {
        use std::sync::atomic::Ordering;
        let Some(h) = &mut self.mic else { return Ok(()) };
        // 0.8.28: the WebRTC stage's echo probe is EXCLUSIVE. Build-then-swap
        // (the else branch) always built the new chain while the old one still held the
        // probe, so a chain with echo-cancel could never reach PAUSED on a rebuild — every
        // heal and every noise-suppression change silently dropped the whole stage (two
        // users' 0.8.27 logs: "start failed with AEC dsp in the chain" on EVERY rebuild,
        // never once succeeding). When the new chain wants the probe, retire the old capture
        // FIRST — off-thread with a bounded wait, so a wedged device can't freeze the call
        // controls — then build; and never leave the mic dead: plain chain, then the default
        // input, then the lifetime healer's backoff.
        let wants_probe = crate::aec::enabled()
            && aec_tier() == AecTier::Dsp
            && dsp_aec_active()
            && !dsp_ec_blocked()
            && !dsp_blocked()
            && !mic_target()
                .lock()
                .unwrap()
                .as_deref()
                .map(|m| m.ends_with(".monitor"))
                .unwrap_or(false);
        if wants_probe {
            h.task.abort();
            let old_pipeline = h.pipeline.clone();
            let (tx, rx) = std::sync::mpsc::channel::<()>();
            std::thread::spawn(move || {
                let _ = old_pipeline.set_state(gst::State::Null);
                let _ = tx.send(());
            });
            if rx.recv_timeout(std::time::Duration::from_millis(1500)).is_err() {
                eprintln!(
                    "[call] the old mic capture is slow to stop — building the new one alongside it \
                     (the echo canceller may have to step down)"
                );
            }
            let source = h.source.clone();
            let built = start_mic_capture(source.clone()).or_else(|e| {
                eprintln!(
                    "[call] processed mic chain failed after the old capture stopped ({e}) — \
                     retrying on the plain chain"
                );
                force_plain_mic_once().store(true, Ordering::Relaxed);
                start_mic_capture(source.clone()).or_else(|e2| {
                    eprintln!(
                        "[call] plain mic chain failed too ({e2}) — retrying on the system default input"
                    );
                    force_plain_mic_once().store(true, Ordering::Relaxed);
                    force_default_mic_once().store(true, Ordering::Relaxed);
                    start_mic_capture(source.clone())
                })
            });
            match built {
                Ok((pipeline, task)) => {
                    h.pipeline = pipeline;
                    h.task = task;
                    eprintln!("[call] mic rebuilt (old capture stopped first so the echo canceller could rebind)");
                    Ok(())
                }
                Err(e) => {
                    // Leave the flag up: the lifetime healer keeps retrying with backoff
                    // instead of the mic staying dead until the user rejoins.
                    // 10.5 (audio review #2c): and mark the capture DOWN — no feeder is left to
                    // re-raise the flag, so without this the healer's 25 s "clean" rule dropped the
                    // request and the mic stayed dead for the rest of the call.
                    mic_down_flag().store(true, Ordering::Relaxed);
                    let _ = raise_mic_unhealthy();
                    Err(e.context("mic rebuild failed on every chain"))
                }
            }
        } else {
            // BUILD-THEN-SWAP (live friends feedback: "changing the mic broke the
            // call"). The old order NULLed the working pipeline BEFORE the fallible
            // rebuild: a bad/vanished device left the mic permanently dead — with the
            // feeder (and its stall detector, i.e. the self-heal signal) already
            // aborted, so peers heard silence for the rest of the call. Now the new
            // capture must come up FIRST; only then is the old one torn down. On
            // failure the previous device keeps running and the caller can tell the
            // user the switch didn't take.
            let (pipeline, task) = start_mic_capture(h.source.clone())?;
            let old_pipeline = std::mem::replace(&mut h.pipeline, pipeline);
            let old_task = std::mem::replace(&mut h.task, task);
            old_task.abort();
            // NULL the old pipeline OFF the caller's thread: wasapi2 state changes on
            // a yanked/wedged endpoint can block, and every caller holds the call
            // mutex — a blocked NULL here used to freeze EVERY control (even hang-up).
            // Worst case the detached thread leaks one dead pipeline; the call lives.
            std::thread::spawn(move || {
                let _ = old_pipeline.set_state(gst::State::Null);
            });
            eprintln!("[call] mic re-pointed to selected input (old capture retired off-thread)");
            Ok(())
        }
    }

    /// Leave the call and tear everything down.
    ///
    /// Two phases, deliberately ordered so a dead/slow SFU can NEVER keep the camera live or
    /// hang the caller (the zombie-broadcast + stuck-hang-up bug):
    ///   1. Stop ALL local capture SYNCHRONOUSLY and FIRST — NULL every gst pipeline + abort
    ///      every feeder. This releases /dev/video0 and stops feeding the encoder INSTANTLY,
    ///      independent of any network round-trip.
    ///   2. Network teardown (unpublish_track / screencast.close / room.close) — each bounded
    ///      with a 3s timeout so an unresponsive SFU can't park the await forever. Callers
    ///      (`leave_call`, `leave_on_exit`) therefore always make progress.
    pub async fn close(mut self) {
        use std::sync::atomic::Ordering;
        use std::time::Duration;
        const NET_T: Duration = Duration::from_secs(3);

        // ── Phase 1: synchronous local teardown (no awaits) ──────────────────────────
        // 10.5: the "can they hear me?" analyzer and its bookkeeping go first (the UI shows nothing
        // outside a call), and a mic we no longer want must not be re-published by anyone.
        if let Some(t) = self.hear_task.take() {
            t.abort();
        }
        crate::hear::session_end();
        crate::sameroom::session_end(); // 11.3
        crate::callstats::session_end(); // widgets: the call-stats sampler stops with the call
        mic_wanted_flag().store(false, Ordering::Relaxed);
        audio_sids_live().lock().unwrap().clear();
        // Drain the aux (soundboard/music) tracks and NULL them now; keep the handles so we
        // can unpublish their sids in phase 2.
        let aux: Vec<(&'static str, AuxAudioHandle)> = aux_tracks().lock().unwrap().drain().collect();
        for (_slot, h) in &aux {
            h.stop.store(true, Ordering::Relaxed); // exit the bus watcher
            set_state_bounded(h.pipeline.upcast_ref(), gst::State::Null, 1500, "share-audio capture");
            h.task.abort();
        }
        *aux_participant().lock().unwrap() = None;
        *aux_runtime().lock().unwrap() = None;
        *event_sender().lock().unwrap() = None;

        let screenshare = self.screenshare.take();
        if let Some((pipeline, _, _, bus_stop)) = &screenshare {
            bus_stop.store(true, Ordering::Relaxed); // exit the bus watcher
            set_state_bounded(pipeline.upcast_ref(), gst::State::Null, 1500, "screen capture");
        }
        // The mic FEEDER stops before any capture pipeline is touched: a camera driver that
        // wedges in its NULL transition must not keep our microphone on the air (Astra
        // review 2026-09-21, finding 7). The mic capture itself is NULLed below, bounded.
        let mic = self.mic.take();
        if let Some(h) = &mic {
            h.task.abort();
            h.stats_stop.store(true, Ordering::Relaxed); // exit the mic publish-stats task
        }
        let camera = self.camera.take();
        if let Some(h) = &camera {
            h.bus_stop.store(true, Ordering::Relaxed); // exit the bus watcher
            h.stats_stop.store(true, Ordering::Relaxed); // exit the publish-stats task
            // Bounded like every other capture here: a synchronous NULL on a wedged v4l2
            // driver blocked this whole (async) close — and the 12 s hang-up timeout around
            // it can never fire while the task is stuck inside a blocking call.
            set_state_bounded(h.pipeline.upcast_ref(), gst::State::Null, 1500, "camera capture"); // releases /dev/video0
        }
        if let Some(h) = &mic {
            // 10.0.4: NULL the capture OFF this thread with a bounded wait. On a field desktop the
            // Flatpak runtime's pipewiresrc connected, delivered nothing, and then never came back from
            // its NULL transition — this synchronous stop was where "hang up" hung, and no rejoin was
            // possible. A source that will not stop is left to finish in the background; the call ends.
            let p = h.pipeline.clone();
            let (tx, rx) = std::sync::mpsc::channel::<()>();
            std::thread::spawn(move || {
                let _ = p.set_state(gst::State::Null);
                let _ = tx.send(());
            });
            if rx.recv_timeout(std::time::Duration::from_millis(1500)).is_err() {
                eprintln!("[call] the mic capture is slow to stop — leaving it to finish in the background so the call can end");
            }
        }
        self.event_task.abort();
        publish_stats().lock().unwrap().clear(); // stream-health entries die with the call
        finalize_recordings(); // Memories (0.8.7): EOS-finalize capture chains (client no-op)
        for (_sid, h) in self._playback.lock().unwrap().drain() {
            h.task.abort(); // stop the remote-audio feeder (orphan fix: it kept the SFU alive)
            let cur = h.pipeline.lock().unwrap().clone();
            set_state_bounded(cur.upcast_ref(), gst::State::Null, 1500, "remote-audio playback");
        }
        track_gains().lock().unwrap().clear(); // drop all per-track gain WeakRefs
        track_levels().lock().unwrap().clear(); // #B drop all per-track audio levels
        playback_sinks().lock().unwrap().clear(); // drop output-sink WeakRefs (next call re-registers)
        clear_recv_state(); // drop retained remote publications + transient receive policy
        *away_active().lock().unwrap() = false; // next call starts not-AFK
        *deaf_active().lock().unwrap() = false; // …and not-deafened
        away_muted_names().lock().unwrap().clear();
        desired_track_prefs().lock().unwrap().clear(); // next call re-seeds from persisted prefs
        clear_video_tiles();
        // Tear down the keyring registration only if it is still OURS — checked and
        // cleared under ONE lock, so a newer session registering concurrently can never
        // be clobbered between the check and the clear (that would leave the live call
        // with no keyring: every later peer key merely buffers ⇒ whole-call static).
        {
            let mut kp = active_key_provider().lock().unwrap();
            if kp.as_ref().map(|(gen, _)| *gen) == Some(self.keyring_gen) {
                *kp = None;
                pending_peer_keys().lock().unwrap().clear(); // next call mints fresh keys
            } else {
                eprintln!("[call] close: keyring already re-registered by a newer session — left intact");
            }
        }
        aec_stop(); // release the echo-cancel module; the user's audio devices are untouched
        eprintln!("[call] local capture stopped (camera/mic/screenshare/aux NULLed)");

        // ── Phase 2: bounded network teardown ────────────────────────────────────────
        let lp = self.room.local_participant();
        for (slot, h) in aux {
            let _ = tokio::time::timeout(NET_T, lp.unpublish_track(&live_sid(slot, &h.sid))).await;
        }
        if let Some((_pipeline, screencast, sid, _bus_stop)) = screenshare {
            let _ = tokio::time::timeout(NET_T, lp.unpublish_track(&live_sid("screen", &sid))).await;
            let _ = tokio::time::timeout(NET_T, screencast.close()).await;
        }
        if let Some(h) = camera {
            let _ = tokio::time::timeout(NET_T, lp.unpublish_track(&live_sid("camera", &h.sid))).await;
        }
        if let Some(h) = mic {
            let _ = tokio::time::timeout(NET_T, lp.unpublish_track(&live_sid("mic", &h.sid))).await;
        }
        let _ = tokio::time::timeout(NET_T, self.room.close()).await;
        // Now that every live_sid-based unpublish above has run, drop the reconnect remap
        // (and the mic-heal flag) so the NEXT call starts clean.
        republished_sids().lock().unwrap().clear();
        mic_capture_unhealthy_flag().store(false, std::sync::atomic::Ordering::Relaxed);
        mic_primed_flag().store(false, std::sync::atomic::Ordering::Relaxed); // (#10: the Settings meter reads it)
        // Cleared HERE (not phase 1): the event_task's abort only lands at its next await,
        // so an ActiveSpeakersChanged arm already in flight could re-populate a phase-1
        // clear. By the end of phase 2 the task is long dead. (A rejoin's connect() also
        // re-clears + re-registers, and every teardown path serializes old-close before
        // new-connect via the hangingUp gate / do_join's defensive take.)
        set_active_speakers(Vec::new());
        local_identity_cell().lock().unwrap().clear();
        eprintln!("[call] room closed — teardown complete");
    }

    /// Self-test (no devices, no peers): connect with E2EE + publish a synthetic
    /// sine-ish audio track for `secs`, to validate the audio publish path headless.
    pub async fn selftest_audio(ws_url: &str, jwt: &str, identity: &str, secs: u64) -> Result<()> {
        crate::ensure_init()?;
        let key_provider = matrix_key_provider();
        key_provider.set_key(&ParticipantIdentity(identity.to_owned()), 0, vec![0u8; 16]);
        let mut options = RoomOptions::default();
        options.encryption = Some(E2eeOptions { encryption_type: EncryptionType::Gcm, key_provider });

        eprintln!("[call-selftest] connecting…");
        let (room, mut events) = Room::connect(ws_url, jwt, options).await.context("connect")?;
        eprintln!("[call-selftest] connected as {}", room.local_participant().identity().0);
        tokio::spawn(async move { while events.recv().await.is_some() {} });

        let source = NativeAudioSource::new(AudioSourceOptions::default(), SAMPLE_RATE, 1, 1000);
        let track = LocalAudioTrack::create_audio_track("mic", RtcAudioSource::Native(source.clone()));
        room.local_participant()
            .publish_track(
                LocalTrack::Audio(track),
                TrackPublishOptions { source: TrackSource::Microphone, ..Default::default() },
            )
            .await
            .context("publish")?;
        eprintln!("[call-selftest] mic track published; feeding {secs}s of 10ms frames");

        let frames_per_sec = 100; // 10ms frames
        let total = secs * frames_per_sec;
        let spc = SAMPLE_RATE / frames_per_sec as u32; // samples per 10ms
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(10));
        let mut phase: f32 = 0.0;
        for _ in 0..total {
            interval.tick().await;
            let mut data = vec![0i16; spc as usize];
            for s in data.iter_mut() {
                phase += 2.0 * std::f32::consts::PI * 440.0 / SAMPLE_RATE as f32;
                *s = (phase.sin() * 3000.0) as i16;
            }
            let frame = AudioFrame { data: Cow::Owned(data), sample_rate: SAMPLE_RATE, num_channels: 1, samples_per_channel: spc };
            let _ = source.capture_frame(&frame).await;
        }
        eprintln!("[call-selftest] done; closing");
        room.close().await.ok();
        Ok(())
    }

    /// Self-test (no GUI): connect with E2EE + publish the (default or `device`)
    /// camera for `secs`, to validate the camera capture+publish path headless. Uses
    /// the capture-only pipeline (no qml6glsink), so it runs with no display.
    pub async fn selftest_camera(
        ws_url: &str,
        jwt: &str,
        identity: &str,
        device: Option<String>,
        secs: u64,
    ) -> Result<()> {
        use livekit::options::VideoCodec;
        use livekit::track::LocalVideoTrack;
        use livekit::webrtc::video_source::native::NativeVideoSource;
        use livekit::webrtc::video_source::{RtcVideoSource, VideoResolution};

        crate::ensure_init()?;
        let key_provider = matrix_key_provider();
        key_provider.set_key(&ParticipantIdentity(identity.to_owned()), 0, vec![0u8; 16]);
        let mut options = RoomOptions::default();
        options.encryption = Some(E2eeOptions { encryption_type: EncryptionType::Gcm, key_provider });

        eprintln!("[cam-selftest] connecting…");
        let (room, mut events) = Room::connect(ws_url, jwt, options).await.context("connect")?;
        eprintln!("[cam-selftest] connected as {}", room.local_participant().identity().0);
        tokio::spawn(async move { while events.recv().await.is_some() {} });

        let source = NativeVideoSource::new(
            VideoResolution { width: camera::CAM_W, height: camera::CAM_H },
            false, // is_screencast=false: camera content (matches publish_camera)
        );
        let track = LocalVideoTrack::create_video_track("camera", RtcVideoSource::Native(source.clone()));
        room.local_participant()
            .publish_track(
                LocalTrack::Video(track),
                TrackPublishOptions { video_codec: VideoCodec::H264, source: TrackSource::Camera, simulcast: false, ..Default::default() },
            )
            .await
            .context("publish")?;
        eprintln!("[cam-selftest] camera track published; capturing {secs}s from {device:?}");

        let pipeline = camera::build_camera_capture_pipeline(device.as_deref())?;
        let appsink = pipeline
            .by_name("sink")
            .context("cam appsink missing")?
            .downcast::<gstreamer_app::AppSink>()
            .map_err(|_| anyhow::anyhow!("cam sink not an AppSink"))?;
        crate::lk_publish::wire_appsink_to_source(&appsink, source);
        let bus_stop = crate::lk_publish::watch_pipeline_bus(&pipeline);
        pipeline.set_state(gst::State::Playing).context("cam capture start")?;

        tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
        eprintln!("[cam-selftest] done; closing");
        bus_stop.store(true, std::sync::atomic::Ordering::Relaxed); // let the bus thread exit
        let _ = pipeline.set_state(gst::State::Null);
        room.close().await.ok();
        Ok(())
    }
}

/// Result of one [`e2ee_loopback`] run.
#[derive(Clone, Debug)]
pub struct LoopbackReport {
    /// The publisher's LiveKit identity (the SFU/lk-jwt `sub`) — reveals whether the
    /// deployment uses `userId:deviceId` (legacy) or a hashed identity.
    pub pub_identity: String,
    /// The subscriber's LiveKit identity (must differ from `pub_identity`, else
    /// lk-jwt ignored our device_id and the test is invalid).
    pub sub_identity: String,
    /// Did the subscriber receive the publisher's video track at all?
    pub subscribed: bool,
    /// E2ee decrypt states observed for the subscribed track (e.g. ["…=Ok"]).
    pub e2ee_states: Vec<String>,
    /// Count of frames the subscriber actually DECODED (decrypt succeeded).
    pub frames_decoded: u64,
    /// Peak luma over decoded frames (0 = black / nothing decoded).
    pub luma_peak: u8,
}

/// NO-HUMAN E2EE self-test: in ONE process, connect a publisher and a subscriber to
/// the same LiveKit room (two distinct identities minted from one login via the
/// lk-jwt device_id), publish a synthetic GCM-encrypted H.264 track, and decode it
/// on the subscriber. This exercises the EXACT receive crypto + identity-mapping +
/// decode path (`set_key` under the peer's real identity → FrameCryptor decrypt →
/// NativeVideoStream) with zero live friends and zero Matrix key transport.
///
/// `install_key=false` is the NEGATIVE CONTROL: the subscriber installs no key, so a
/// correct E2EE setup MUST yield `MissingKey` + 0 decoded frames (proving encryption
/// is genuinely on and a green result isn't a fluke).
pub async fn e2ee_loopback(
    ws_url: &str,
    pub_jwt: &str,
    sub_jwt: &str,
    secs: u64,
    install_key: bool,
) -> Result<LoopbackReport> {
    e2ee_loopback_codec(ws_url, pub_jwt, sub_jwt, secs, install_key, "h264").await
}

/// Same loopback, with the publish codec chosen by name ("h264" | "vp9" | "av1").
/// `av1` is the AV1 + E2EE question: whether the pinned libwebrtc's frame cryptor leaves the
/// AV1 OBU headers in the clear so the packetizer can still parse them after encryption.
pub async fn e2ee_loopback_codec(
    ws_url: &str,
    pub_jwt: &str,
    sub_jwt: &str,
    secs: u64,
    install_key: bool,
    codec: &str,
) -> Result<LoopbackReport> {
    use livekit::options::VideoCodec;
    use livekit::track::LocalVideoTrack;
    use livekit::webrtc::video_frame::{I420Buffer, VideoFrame, VideoRotation};
    use livekit::webrtc::video_source::native::NativeVideoSource;
    use livekit::webrtc::video_source::{RtcVideoSource, VideoResolution};
    use std::time::Duration;

    crate::ensure_init()?;
    // The share publish's BWE seed, loopback edition: without it the estimator starts
    // at webrtc default (~300 kbps) and a high-res run starves for most of its
    // window — the decode-ratio gate then measures the BWE ramp, not decode
    // throughput. Measured: a healthy binary false-failed at 51%, and a
    // BWE-starved 75% masqueraded as the stale-dav1d signature.
    std::env::set_var("LK_BWE_START_KBPS", "15000");
    let key: [u8; 16] = [0x5A; 16]; // fixed known media key

    // ---- Publisher ----
    let kp_pub = matrix_key_provider();
    let mut pub_opts = RoomOptions::default();
    // TINHAT_E2EE_PLAINTEXT=1: unencrypted CONTROL — same harness, same synthetic
    // track, no cryptor on either side. Separates "the cryptor broke this
    // codec" from "this codec never decoded in the loopback to begin with".
    let plaintext = std::env::var_os("TINHAT_E2EE_PLAINTEXT").is_some();
    if plaintext {
        eprintln!("[loopback] PLAINTEXT CONTROL — no E2EE on either side");
    } else {
        pub_opts.encryption = Some(E2eeOptions { encryption_type: EncryptionType::Gcm, key_provider: kp_pub.clone() });
    }
    let (pub_room, mut pub_events) = Room::connect(ws_url, pub_jwt, pub_opts).await.context("loopback pub connect")?;
    let pub_identity = pub_room.local_participant().identity().0;
    eprintln!("[loopback] PUBLISHER LiveKit identity = {pub_identity:?}");
    // Encrypt our outgoing track under our ACTUAL identity (so encryption engages
    // regardless of the identity format the SFU assigns).
    kp_pub.set_key(&ParticipantIdentity(pub_identity.clone()), 0, key.to_vec());
    tokio::spawn(async move { while pub_events.recv().await.is_some() {} });

    // TINHAT_E2EE_RES=WxH (default 640x360): the decode-threading fixes only engage at high
    // resolution (tile grid >=2560 wide, frame threading under load), so the gate
    // must be able to run where they live.
    let (res_w, res_h) = std::env::var("TINHAT_E2EE_RES")
        .ok()
        .and_then(|s| {
            let (a, b) = s.split_once('x')?;
            Some((a.parse::<u32>().ok()?, b.parse::<u32>().ok()?))
        })
        .unwrap_or((640, 360));
    eprintln!("[loopback] synthetic source {res_w}x{res_h}");
    let source = NativeVideoSource::new(VideoResolution { width: res_w, height: res_h }, true);
    let track = LocalVideoTrack::create_video_track("loopback", RtcVideoSource::Native(source.clone()));
    pub_room
        .local_participant()
        .publish_track(
            LocalTrack::Video(track),
            TrackPublishOptions {
                video_codec: match codec {
                    "av1" => VideoCodec::AV1,
                    "vp9" => VideoCodec::VP9,
                    _ => VideoCodec::H264,
                },
                source: TrackSource::Screenshare,
                // Production shape (lk_publish.rs): screen shares are a SINGLE
                // sharp layer, simulcast off. The SDK default of 2-layer
                // simulcast for Screenshare is a config production never
                // publishes, and it fed one NVENC-AV1 instance into two RTP
                // streams — dav1d saw interleaved garbage. Gate what we ship,
                // not what defaults do.
                simulcast: false,
                ..Default::default()
            },
        )
        .await
        .context("loopback publish")?;
    let feed = tokio::spawn(async move {
        let mut tick: i64 = 0;
        let mut interval = tokio::time::interval(Duration::from_millis(33));
        loop {
            interval.tick().await;
            let mut buf = I420Buffer::new(res_w, res_h);
            let (y, u, v) = buf.data_mut();
            let s = (tick % 255) as u8;
            // Bright, moving gradient so a decoded frame has clearly non-zero luma.
            y.iter_mut().enumerate().for_each(|(i, p)| *p = ((i as u8) ^ s) | 0x40);
            u.iter_mut().for_each(|p| *p = 128);
            v.iter_mut().for_each(|p| *p = 128);
            source.capture_frame(&VideoFrame {
                rotation: VideoRotation::VideoRotation0,
                timestamp_us: tick * 33_333,
                frame_metadata: None,
                buffer: buf,
            });
            tick += 1;
        }
    });

    // ---- Subscriber ----
    let kp_sub = matrix_key_provider();
    if install_key {
        kp_sub.set_key(&ParticipantIdentity(pub_identity.clone()), 0, key.to_vec());
        eprintln!("[loopback] subscriber installed key under {pub_identity:?} (POSITIVE)");
    } else {
        eprintln!("[loopback] subscriber installed NO key (NEGATIVE control)");
    }
    let mut sub_opts = RoomOptions::default();
    if !plaintext {
        sub_opts.encryption = Some(E2eeOptions { encryption_type: EncryptionType::Gcm, key_provider: kp_sub });
    }
    let (sub_room, mut sub_events) = Room::connect(ws_url, sub_jwt, sub_opts).await.context("loopback sub connect")?;
    let sub_identity = sub_room.local_participant().identity().0;
    eprintln!("[loopback] SUBSCRIBER LiveKit identity = {sub_identity:?}");

    let report = Arc::new(Mutex::new(LoopbackReport {
        pub_identity: pub_identity.clone(),
        sub_identity: sub_identity.clone(),
        subscribed: false,
        e2ee_states: Vec::new(),
        frames_decoded: 0,
        luma_peak: 0,
    }));
    let r2 = report.clone();
    let drain = tokio::spawn(async move {
        while let Some(ev) = sub_events.recv().await {
            match ev {
                RoomEvent::TrackSubscribed { track: RemoteTrack::Video(t), participant, .. } => {
                    eprintln!("[loopback] sub subscribed to video from {:?}", participant.identity().0);
                    r2.lock().unwrap().subscribed = true;
                    let r3 = r2.clone();
                    let rtc = t.rtc_track();
                    tokio::spawn(async move {
                        let mut stream = NativeVideoStream::new(rtc);
                        while let Some(frame) = stream.next().await {
                            if let Some(i420) = frame.buffer.as_i420() {
                                let (sy, _, _) = i420.data();
                                let peak = sy.iter().copied().max().unwrap_or(0);
                                let mut g = r3.lock().unwrap();
                                g.frames_decoded += 1;
                                if peak > g.luma_peak {
                                    g.luma_peak = peak;
                                }
                            }
                        }
                    });
                }
                RoomEvent::E2eeStateChanged { participant, state } => {
                    let id = participant.identity().0;
                    eprintln!("[loopback] E2eeStateChanged {id:?} -> {state:?}");
                    r2.lock().unwrap().e2ee_states.push(format!("{id}={state:?}"));
                }
                _ => {}
            }
        }
    });

    tokio::time::sleep(Duration::from_secs(secs)).await;
    feed.abort();
    drain.abort();
    pub_room.close().await.ok();
    sub_room.close().await.ok();
    let mut out = report.lock().unwrap().clone();
    // Lesson learned: a decoder erroring on every other frame still "decodes
    // frames" — the 0.7.2 regression shipped behind a PASS that showed 226 of
    // ~360 expected frames. Judge the RATE. The synthetic source feeds ~30fps;
    // require 60% of expected on the POSITIVE run (install_key) so pipeline
    // stalls, EAGAIN loops and PLI storms fail the gate instead of shading it.
    // Only when TINHAT_E2EE_RES is set: the default 640x360 screenshare publish
    // intentionally encodes at ~5fps (content-type degradation), so a 30fps
    // expectation there is fiction — ~50 decoded frames is healthy. The
    // EAGAIN/stall class this catches only discriminates at feed-rate encodes.
    if install_key && out.frames_decoded > 0 && std::env::var_os("TINHAT_E2EE_RES").is_some() {
        let expected = (secs.saturating_sub(2)) * 30; // startup grace
        let pct = 100 * out.frames_decoded as u64 / expected.max(1);
        eprintln!(
            "[loopback] decode ratio: {} of ~{expected} expected frames ({pct}%)",
            out.frames_decoded
        );
        // Threshold calibrated from measured populations, not hope: catastrophic
        // stall modes (keyframe-only cadence — the 0.7.2/0.7.3 field failures)
        // run at 3-8%; healthy runs range 51-108% because SFU/load transients
        // move the whole-window ratio (measured 64/54/108 across three seeded
        // runs). 60% false-failed healthy builds; 25% separates the
        // populations with margin on both sides.
        if pct < 25 {
            out.frames_decoded = 0; // force the verdict to FAIL loudly
            eprintln!("[loopback] DECODE RATIO FAIL: {pct}% < 25% — catastrophic decode stall (keyframe-only cadence class)");
        }
    }
    Ok(out)
}

/// Result of one [`simulcast_loopback`] resolution phase.
#[derive(Clone, Debug, Default)]
pub struct SimulcastReport {
    /// Publish size tested, e.g. "1280x720".
    pub phase: String,
    /// One formatted line per outbound rid (rid, encoder impl, size, fps, target).
    pub rung_lines: Vec<String>,
    /// Rids observed actively encoding (frames_encoded > 30).
    pub rungs_encoding: usize,
    /// Of those, rids whose encoder implementation looks like NVENC (vs OpenH264 fallback).
    pub nvenc_rungs: usize,
    /// Frames the subscriber decoded during the phase.
    pub sub_frames: u64,
    /// Subscriber's decoded frame size before / after update_video_dimensions(320x180) —
    /// a real layer switch shows the SFU honours UpdateTrackSettings from this client.
    pub dims_before: (u32, u32),
    /// Whether this phase was asked to run the lever probes at all (only the first phase
    /// does) — the gate must not demand dims from a phase that never probes.
    pub levers_expected: bool,
    pub dims_after: (u32, u32),
    /// Did set_enabled(false) actually stop frames, and set_enabled(true) restart them?
    pub pause_worked: bool,
    pub resume_worked: bool,
}

/// NO-HUMAN SIMULCAST GATE (mirrors [`e2ee_loopback`]'s two-identity pattern): publish a
/// synthetic camera-style track with the EXACT production options (H.264 + NVENC pinned +
/// simulcast + explicit H180/H360 rungs) and verify, per resolution:
///   1. every rung actually encodes (per-rid outbound-rtp stats — the silent-OpenH264-
///      fallback / blank-low-rung failure is exactly what this catches),
///   2. the subscriber's update_video_dimensions(320,180) produces a REAL layer switch
///      (decoded frame size drops), proving this SFU honours UpdateTrackSettings from us,
///   3. set_enabled(false/true) genuinely stops/restarts forwarding (the visibility-pause
///      lever).
/// Publisher dynacast stays OFF here so the SFU can't pause unwatched rungs and fake a
/// rung failure. Run via `TINHAT_CAM_SIMTEST=1` (see main.rs).
/// Headless SHARE gate (two live participants on the real SFU): publish a track EXACTLY the
/// way `publish_screenshare` does — TrackSource::Screenshare, `simulcast: false` (one sharp
/// layer, EMPTY rid), H.264/NVENC, E2EE on, and the app's real RoomOptions — then subscribe
/// from a second participant and require that decoded frames KEEP flowing.
///
/// This exists because of the r124 dynacast regression: with dynacast on, the SFU's quality
/// update matched the share's empty-rid encoding against nothing and switched the ONLY layer
/// off — one keyframe, then a permanently blank tile for every peer, which no publish-side
/// stat gate could see (encode stats looked "quiet", not broken). The ONLY reliable proof is
/// a second participant counting decoded frames over time, which is what this does. Run via
/// `TINHAT_SHARETEST=1` (see main.rs).
pub async fn share_loopback(ws_url: &str, pub_jwt: &str, sub_jwt: &str) -> Result<(u64, u64)> {
    use livekit::options::{VideoCodec, VideoEncoderBackend, VideoEncoding};
    use livekit::track::LocalVideoTrack;
    use livekit::webrtc::video_frame::{I420Buffer, VideoFrame, VideoRotation};
    use livekit::webrtc::video_source::native::NativeVideoSource;
    use livekit::webrtc::video_source::{RtcVideoSource, VideoResolution};
    use std::time::Duration;

    crate::ensure_init()?;
    let key: [u8; 16] = [0xA5; 16];

    // Publisher: the APP'S room options (dynacast included) — the gate must exercise
    // exactly what production ships, so a future dynacast re-enable trips it.
    let kp_pub = matrix_key_provider();
    let mut pub_opts = RoomOptions::default();
    pub_opts.encryption =
        Some(E2eeOptions { encryption_type: EncryptionType::Gcm, key_provider: kp_pub.clone() });
    pub_opts.dynacast = false; // KEEP IN LOCKSTEP with CallSession::connect (see comment there)
    let (pub_room, mut pub_events) =
        Room::connect(ws_url, pub_jwt, pub_opts).await.context("sharetest pub connect")?;
    let pub_identity = pub_room.local_participant().identity().0;
    kp_pub.set_key(&ParticipantIdentity(pub_identity.clone()), 0, key.to_vec());
    tokio::spawn(async move { while pub_events.recv().await.is_some() {} });

    // Subscriber counts decoded frames.
    let frames = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let kp_sub = matrix_key_provider();
    kp_sub.set_key(&ParticipantIdentity(pub_identity.clone()), 0, key.to_vec());
    let mut sub_opts = RoomOptions::default();
    sub_opts.encryption =
        Some(E2eeOptions { encryption_type: EncryptionType::Gcm, key_provider: kp_sub });
    let (_sub_room, mut sub_events) =
        Room::connect(ws_url, sub_jwt, sub_opts).await.context("sharetest sub connect")?;
    let fr2 = frames.clone();
    let drain = tokio::spawn(async move {
        while let Some(ev) = sub_events.recv().await {
            if let RoomEvent::TrackSubscribed { track: RemoteTrack::Video(t), publication, .. } = ev
            {
                eprintln!(
                    "[sharetest] subscriber got video sid={} source={:?}",
                    publication.sid().as_str(),
                    publication.source()
                );
                let fr3 = fr2.clone();
                let rtc = t.rtc_track();
                tokio::spawn(async move {
                    let mut stream = NativeVideoStream::new(rtc);
                    while stream.next().await.is_some() {
                        fr3.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                });
            }
        }
    });

    // Publish EXACTLY like publish_screenshare (H264/NVENC, Screenshare source, ONE layer).
    let (tw, th, fps) = (1920u32, 800u32, 30u32); // the owner's real share dims
    let source = NativeVideoSource::new(VideoResolution { width: tw, height: th }, true);
    let track = LocalVideoTrack::create_video_track("screen", RtcVideoSource::Native(source.clone()));
    let publication = pub_room
        .local_participant()
        .publish_track(
            LocalTrack::Video(track),
            TrackPublishOptions {
                video_codec: VideoCodec::H264,
                video_encoder: VideoEncoderBackend::Nvenc,
                source: TrackSource::Screenshare,
                video_encoding: Some(VideoEncoding { max_bitrate: 8_000_000, max_framerate: fps as f64 }),
                simulcast: false,
                ..Default::default()
            },
        )
        .await
        .context("sharetest publish")?;

    // Synthetic 30fps feed with changing content (so the encoder has real work).
    let feed = tokio::spawn(async move {
        let mut n: u8 = 0;
        let mut tick = tokio::time::interval(Duration::from_millis(33));
        loop {
            tick.tick().await;
            let mut buf = I420Buffer::new(tw, th);
            {
                let (sy, _, _) = buf.strides();
                let (dy, du, dv) = buf.data_mut();
                for (row, chunk) in dy.chunks_mut(sy as usize).enumerate() {
                    let v = (row as u8).wrapping_add(n);
                    chunk.iter_mut().for_each(|p| *p = v);
                }
                du.iter_mut().for_each(|p| *p = 128u8.wrapping_add(n / 2));
                dv.iter_mut().for_each(|p| *p = 128u8.wrapping_sub(n / 2));
            }
            n = n.wrapping_add(3);
            source.capture_frame(&VideoFrame {
                rotation: VideoRotation::VideoRotation0,
                timestamp_us: 0,
                frame_metadata: None,
                buffer: buf,
            });
        }
    });

    // Sample decode counts over two windows — the dynacast failure decodes ~1 frame in
    // window A and ZERO in window B, so requiring BOTH windows nonzero catches it.
    tokio::time::sleep(Duration::from_secs(10)).await;
    let a = frames.load(std::sync::atomic::Ordering::Relaxed);
    tokio::time::sleep(Duration::from_secs(10)).await;
    let b = frames.load(std::sync::atomic::Ordering::Relaxed) - a;
    eprintln!("[sharetest] decoded frames: first 10s = {a}, second 10s = {b}");

    feed.abort();
    drain.abort();
    let _ = tokio::time::timeout(
        Duration::from_secs(3),
        pub_room.local_participant().unpublish_track(&publication.sid()),
    )
    .await;
    let _ = tokio::time::timeout(Duration::from_secs(3), pub_room.close()).await;
    Ok((a, b))
}

/// Headless ACTIVE-SPEAKERS gate, parent side. Two participants on the real SFU, in
/// TWO PROCESSES. (History: an early single-process A+B run scored only one connection,
/// which was first misread as a per-process SFU limit — the actual cause was the
/// per-participant self-key requirement: a keyless matrix_key_provider() makes the
/// sender FrameCryptor drop EVERY frame, so that connection sent zero RTP. See the
/// CRITICAL set_key comments below. The two-process shape was kept anyway: separate SDK
/// stacks match real users and keep the gate immune to any future shared-state quirk.)
/// The harness (main.rs) forks this same binary as a CHILD (TINHAT_SPEAKERTEST_CHILD)
/// that runs [`speakers_publish_steady`]: participant A, a constant tone — a steady
/// talker whose smoothed level converges (0.250 -> 0.375) and who is then NEVER
/// mentioned again in the SFU's changed-only SpeakersChanged deltas.
///
/// THIS side (B) publishes a slow LOUD staircase (multi-second dwells — sub-second
/// bursts never register on this SFU's smoothing) whose level keeps stepping and dips
/// to silence: every resulting B-only delta was, under the old replace semantics, a
/// wholesale wipe of the still-talking A from active_speakers. B tracks its
/// ActiveSpeakersChanged events (exactly what the app's overlay consumes) and requires
/// A to stay present with no gap >1s across an 18s window. A run that generated <4
/// deltas proved nothing (replace == merge without churn) and FAILS as
/// non-discriminating.
///
/// This exists because of the r134 overlay regression class: the vendored SDK treated
/// the changed-only deltas as full snapshots — remote pills never showed in the voice
/// HUD while the self pill worked, and no single-participant gate can see that. Returns
/// (samples with A active, total samples, max A-absence gap in ms). Run via
/// `TINHAT_SPEAKERTEST=1` (see main.rs).
pub async fn speakers_loopback(
    ws_url: &str,
    b_jwt: &str,
    on_ready: impl FnOnce(),
) -> Result<(u64, u64, u64)> {
    use std::time::Duration;

    crate::ensure_init()?;

    // B: the observer whose own level-step deltas used to do the wiping.
    let kp_b = matrix_key_provider();
    let mut b_opts = RoomOptions::default();
    b_opts.encryption =
        Some(E2eeOptions { encryption_type: EncryptionType::Gcm, key_provider: kp_b.clone() });
    b_opts.dynacast = false; // KEEP IN LOCKSTEP with CallSession::connect
    let (b_room, mut b_events) =
        Room::connect(ws_url, b_jwt, b_opts).await.context("speakertest B connect")?;
    let b_identity = b_room.local_participant().identity().0;
    // CRITICAL: a key under OUR OWN identity, or the FrameCryptor cannot encrypt our
    // outgoing audio and the track sends NO RTP AT ALL — the SFU then has nothing to
    // score and the gate silently proves nothing (measured: MissingKey => zero deltas).
    kp_b.set_key(&ParticipantIdentity(b_identity.clone()), 0, vec![0xA5; 16]);

    // Track the latest ActiveSpeakersChanged list + delta churn. Any active identity
    // that isn't B is the child (the room is isolated; only A and B are ever in it).
    let b_active: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let b_churn = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let (b_active2, b_churn2) = (b_active.clone(), b_churn.clone());
    let b_drain = tokio::spawn(async move {
        while let Some(ev) = b_events.recv().await {
            match &ev {
                RoomEvent::ActiveSpeakersChanged { speakers } => {
                    b_churn2.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    *b_active2.lock().unwrap() =
                        speakers.iter().map(|p| p.identity().0.clone()).collect();
                }
                RoomEvent::ParticipantConnected(p) => {
                    eprintln!("[speakertest] child joined: {}", p.identity().0);
                }
                RoomEvent::E2eeStateChanged { state, .. } => {
                    // MissingKey here means a cryptor is sending NOTHING — the failure
                    // mode that silently voided early versions of this gate.
                    eprintln!("[speakertest] e2ee state: {state:?}");
                }
                _ => {}
            }
        }
    });

    // B is connected and LISTENING — only now may the harness spawn the child talker.
    // The SFU's speaker map sends changed-only deltas from join time forward (no snapshot
    // for late joiners): a steady child's ONLY mentions are its brief level-convergence
    // steps right after publish, and a child spawned before B's listener was live could
    // burn those mentions into the void — B would then never see A at all and a healthy
    // build would false-FAIL.
    on_ready();

    // B's staircase: 9s period — 3s medium, 3s loud, 3s silent (the silence phase also
    // exercises active=false removals).
    let b_src = spk_publish_tone(&b_room, "bmic").await?;
    let b_feed = spk_spawn_tone(b_src, |t| match ((t / 3.0) as u64) % 3 {
        0 => 0.18,
        1 => 0.8,
        _ => 0.0,
    });

    // Warm-up: child connects + A's level converges + first deltas land; then sample
    // B's view every 100ms across two full B cycles.
    tokio::time::sleep(Duration::from_secs(10)).await;
    let churn_baseline = b_churn.load(std::sync::atomic::Ordering::Relaxed);
    let (mut samples, mut a_samples, mut gap_ms, mut max_gap_ms) = (0u64, 0u64, 0u64, 0u64);
    let mut seen_once = false;
    for _ in 0..180 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        samples += 1;
        let has_a = b_active.lock().unwrap().iter().any(|id| id != &b_identity);
        if has_a {
            a_samples += 1;
            seen_once = true;
            gap_ms = 0;
        } else if seen_once {
            gap_ms += 100;
            max_gap_ms = max_gap_ms.max(gap_ms);
        }
    }
    if !seen_once {
        max_gap_ms = 18_000; // A never appeared on B's client at all
    }
    let churn = b_churn.load(std::sync::atomic::Ordering::Relaxed) - churn_baseline;
    eprintln!(
        "[speakertest] A active in {a_samples}/{samples} samples on B's client, \
         max gap {max_gap_ms}ms, {churn} speaker deltas during the window"
    );

    b_feed.abort();
    b_drain.abort();
    let _ = tokio::time::timeout(Duration::from_secs(3), b_room.close()).await;
    if churn < 4 {
        // No wiping deltas were generated -> replace == merge and the run proved
        // NOTHING. Report as a failure so a scenario regression can't hide the bug.
        eprintln!("[speakertest] NON-DISCRIMINATING: only {churn} deltas in the window (need >=4)");
        return Ok((0, samples, max_gap_ms.max(18_000)));
    }
    Ok((a_samples, samples, max_gap_ms))
}

/// Dev/verification helper (TINHAT_VIDPUB, see main.rs): connect to a LiveKit room and
/// publish a synthetic ANIMATED screen-share-shaped track for `secs`, then leave. Used to
/// put a real remote video tile in front of the GUI (e.g. to exercise the PiP pop-out)
/// without a second human. Deliberately NO E2EE: an unencrypted track is signaled
/// enc_type=None, gets no FrameCryptor on the receiving app, and decodes plainly — so the
/// helper needs no Matrix key exchange.
/// DEV HARNESS (`TINHAT_AUDPUB=<room>`, see main.rs): publish a synthetic speech-shaped
/// AUDIO track into a room for `secs`, with NO E2EE, so a GUI instance joining that
/// room exercises the REAL remote-audio path end to end — `build_playback`'s gst
/// pipeline, the receive skew servo, the cushion prefill, and the `[play-rt]` health
/// line. (The audio twin of `share_publish_synthetic`; the E2EE gates prove decode,
/// but only this proves the pipeline that actually reaches the speakers.)
pub async fn audio_publish_synthetic(ws_url: &str, jwt: &str, secs: u64) -> Result<()> {
    use std::time::Duration;
    crate::ensure_init()?;
    let (room, mut events) =
        Room::connect(ws_url, jwt, RoomOptions::default()).await.context("audpub connect")?;
    eprintln!("[audpub] connected as {}", room.local_participant().identity().0);
    tokio::spawn(async move { while events.recv().await.is_some() {} });

    // APM OFF: libwebrtc's noise suppressor treats a steady synthetic tone as stationary
    // noise and attenuates it to digital silence (peak=0 at the receiver) — which would
    // make this harness prove nothing. The real mic path disables the APM too (see
    // publish_mic_muted's comment on the queue_size_ms=0 fast path).
    let source = NativeAudioSource::new(
        AudioSourceOptions {
            echo_cancellation: false,
            noise_suppression: false,
            auto_gain_control: false,
        },
        SAMPLE_RATE,
        1,
        1000,
    );
    let track = LocalAudioTrack::create_audio_track("mic", RtcAudioSource::Native(source.clone()));
    room.local_participant()
        .publish_track(
            LocalTrack::Audio(track),
            TrackPublishOptions { source: TrackSource::Microphone, ..Default::default() },
        )
        .await
        .context("audpub publish")?;
    eprintln!("[audpub] audio track published (no E2EE, APM off); feeding {secs}s");

    let spc = SAMPLE_RATE / 100; // 10ms
    let mut interval = tokio::time::interval(Duration::from_millis(10));
    let mut phase: f32 = 0.0;
    let mut t: u64 = 0;
    for _ in 0..(secs * 100) {
        interval.tick().await;
        let mut data = vec![0i16; spc as usize];
        // Speech-ish: 1s of a warbling tone, 0.5s of near-silence — so a receiver's
        // servo sees both talk and gap phases (and any splice would be audible).
        let talking = (t / 100) % 3 != 2;
        for s in data.iter_mut() {
            let f = 220.0 + 60.0 * ((t as f32) / 40.0).sin();
            phase += 2.0 * std::f32::consts::PI * f / SAMPLE_RATE as f32;
            *s = if talking { (phase.sin() * 9000.0) as i16 } else { 0 };
        }
        let frame = AudioFrame {
            data: Cow::Owned(data),
            sample_rate: SAMPLE_RATE,
            num_channels: 1,
            samples_per_channel: spc,
        };
        let _ = source.capture_frame(&frame).await;
        t += 1;
    }
    eprintln!("[audpub] done; closing");
    room.close().await.ok();
    Ok(())
}

pub async fn share_publish_synthetic(ws_url: &str, jwt: &str, secs: u64) -> Result<()> {
    use livekit::options::{VideoCodec, VideoEncoderBackend, VideoEncoding};
    use livekit::track::LocalVideoTrack;
    use livekit::webrtc::video_frame::{I420Buffer, VideoFrame, VideoRotation};
    use livekit::webrtc::video_source::native::NativeVideoSource;
    use livekit::webrtc::video_source::{RtcVideoSource, VideoResolution};
    use std::time::Duration;

    crate::ensure_init()?;
    let mut opts = RoomOptions::default();
    opts.dynacast = false;
    let (room, mut events) =
        Room::connect(ws_url, jwt, opts).await.context("vidpub connect")?;
    eprintln!("[vidpub] connected as {}", room.local_participant().identity().0);
    tokio::spawn(async move { while events.recv().await.is_some() {} });

    let (tw, th, fps) = (1280u32, 720u32, 30u32);
    let source = NativeVideoSource::new(VideoResolution { width: tw, height: th }, true);
    let track =
        LocalVideoTrack::create_video_track("screen", RtcVideoSource::Native(source.clone()));
    let publication = room
        .local_participant()
        .publish_track(
            LocalTrack::Video(track),
            TrackPublishOptions {
                video_codec: VideoCodec::H264,
                video_encoder: VideoEncoderBackend::Nvenc,
                source: TrackSource::Screenshare,
                video_encoding: Some(VideoEncoding {
                    max_bitrate: 4_000_000,
                    max_framerate: fps as f64,
                }),
                simulcast: false,
                ..Default::default()
            },
        )
        .await
        .context("vidpub publish")?;
    // Recording-only gate: TINHAT_VIDPUB_RECONLY=1 flags the synthetic share "recording only" so a
    // receiver under test must hide + mute it (and can tune in).
    if std::env::var_os("TINHAT_VIDPUB_RECONLY").is_some() {
        let lp = room.local_participant().clone();
        let sid = publication.sid().as_str().to_string();
        let rounds = (secs / 5).max(1);
        tokio::spawn(async move {
            for _ in 0..rounds {
                match lp.publish_data(rec_only_packet(&sid)).await {
                    Ok(()) => eprintln!("[vidpub] recording-only announced"),
                    Err(e) => eprintln!("[vidpub] announce failed: {e}"),
                }
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        });
    }
    // Share-audio gate: TINHAT_VIDPUB_AUDIO=1 also publishes a 440 Hz ScreenshareAudio track for
    // `secs` — a receiver under test must keep it muted until it watches the share.
    if std::env::var_os("TINHAT_VIDPUB_AUDIO").is_some() {
        let asrc = NativeAudioSource::new(AudioSourceOptions::default(), SAMPLE_RATE, 1, 1000);
        let atrack = LocalAudioTrack::create_audio_track("music", RtcAudioSource::Native(asrc.clone()));
        room.local_participant()
            .publish_track(
                LocalTrack::Audio(atrack),
                TrackPublishOptions { source: TrackSource::ScreenshareAudio, ..Default::default() },
            )
            .await
            .context("vidpub audio publish")?;
        eprintln!("[vidpub] synthetic share AUDIO published");
        tokio::spawn(async move {
            let spc = SAMPLE_RATE / 100;
            let mut interval = tokio::time::interval(Duration::from_millis(10));
            let mut phase: f32 = 0.0;
            for _ in 0..(secs * 100) {
                interval.tick().await;
                let mut data = vec![0i16; spc as usize];
                for smp in data.iter_mut() {
                    phase += 2.0 * std::f32::consts::PI * 440.0 / SAMPLE_RATE as f32;
                    *smp = (phase.sin() * 3000.0) as i16;
                }
                let frame = AudioFrame { data: Cow::Owned(data), sample_rate: SAMPLE_RATE, num_channels: 1, samples_per_channel: spc };
                let _ = asrc.capture_frame(&frame).await;
            }
        });
    }
    eprintln!("[vidpub] synthetic share up for {secs}s");

    let feed = tokio::spawn(async move {
        let mut n: u8 = 0;
        let mut tick = tokio::time::interval(Duration::from_millis(33));
        loop {
            tick.tick().await;
            let mut buf = I420Buffer::new(tw, th);
            {
                let (sy, _, _) = buf.strides();
                let (dy, du, dv) = buf.data_mut();
                for (row, chunk) in dy.chunks_mut(sy as usize).enumerate() {
                    let v = (row as u8).wrapping_add(n);
                    chunk.iter_mut().for_each(|p| *p = v);
                }
                du.iter_mut().for_each(|p| *p = 128u8.wrapping_add(n));
                dv.iter_mut().for_each(|p| *p = 128u8.wrapping_sub(n / 2));
            }
            n = n.wrapping_add(5); // fast scroll + hue sweep: motion is unmistakable
            source.capture_frame(&VideoFrame {
                rotation: VideoRotation::VideoRotation0,
                timestamp_us: 0,
                frame_metadata: None,
                buffer: buf,
            });
        }
    });
    tokio::time::sleep(Duration::from_secs(secs)).await;
    feed.abort();
    let _ = tokio::time::timeout(
        Duration::from_secs(3),
        room.local_participant().unpublish_track(&publication.sid()),
    )
    .await;
    let _ = tokio::time::timeout(Duration::from_secs(3), room.close()).await;
    Ok(())
}

/// Child side of TINHAT_SPEAKERTEST (see [`speakers_loopback`]): connect as participant A
/// and publish a steady constant tone for `secs`, then leave. Runs in its OWN process —
/// see the gate doc for why (separate SDK stacks; the historical single-process failure
/// was a missing self-key, not a per-process limit).
pub async fn speakers_publish_steady(ws_url: &str, jwt: &str, secs: u64) -> Result<()> {
    crate::ensure_init()?;
    let kp = matrix_key_provider();
    let mut opts = RoomOptions::default();
    opts.encryption =
        Some(E2eeOptions { encryption_type: EncryptionType::Gcm, key_provider: kp.clone() });
    opts.dynacast = false;
    let (room, mut events) =
        Room::connect(ws_url, jwt, opts).await.context("speakertest child connect")?;
    // Own-identity key or the cryptor sends nothing (see speakers_loopback).
    let identity = room.local_participant().identity().0.clone();
    kp.set_key(&ParticipantIdentity(identity), 0, vec![0xA5; 16]);
    tokio::spawn(async move { while events.recv().await.is_some() {} });
    let src = spk_publish_tone(&room, "amic").await?;
    let feed = spk_spawn_tone(src, |_| 0.5);
    eprintln!("[speakertest-child] steady tone up for {secs}s");
    tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
    feed.abort();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(3), room.close()).await;
    Ok(())
}

/// 10ms 300Hz tone feeder for the speakers gate; amplitude is a function of elapsed
/// seconds.
fn spk_spawn_tone(
    source: NativeAudioSource,
    amp: impl Fn(f64) -> f64 + Send + 'static,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut t = 0f64;
        let mut phase = 0f64;
        let mut tick = tokio::time::interval(std::time::Duration::from_millis(10));
        loop {
            tick.tick().await;
            let a = amp(t);
            let mut data = vec![0i16; 480];
            for s in data.iter_mut() {
                phase += 2.0 * std::f64::consts::PI * 300.0 / 48_000.0;
                *s = (phase.sin() * a * 30_000.0) as i16;
            }
            t += 0.01;
            let _ = source
                .capture_frame(&AudioFrame {
                    data: Cow::Owned(data),
                    sample_rate: 48_000,
                    num_channels: 1,
                    samples_per_channel: 480,
                })
                .await;
        }
    })
}

/// Publish a fast-path mic-shaped audio track for the speakers gate.
async fn spk_publish_tone(room: &Room, name: &str) -> Result<NativeAudioSource> {
    use livekit::track::LocalAudioTrack;
    let source = NativeAudioSource::new(AudioSourceOptions::default(), 48_000, 1, 0);
    let track = LocalAudioTrack::create_audio_track(name, RtcAudioSource::Native(source.clone()));
    room.local_participant()
        .publish_track(
            LocalTrack::Audio(track),
            TrackPublishOptions { source: TrackSource::Microphone, ..Default::default() },
        )
        .await
        .with_context(|| format!("speakertest publish {name}"))?;
    Ok(source)
}

pub async fn simulcast_loopback(
    ws_url: &str,
    pub_jwt: &str,
    sub_jwt: &str,
) -> Result<Vec<SimulcastReport>> {
    use livekit::options::{VideoCodec, VideoEncoderBackend, VideoEncoding};
    use livekit::track::LocalVideoTrack;
    use livekit::webrtc::video_frame::{I420Buffer, VideoFrame, VideoRotation};
    use livekit::webrtc::video_source::native::NativeVideoSource;
    use livekit::webrtc::video_source::{RtcVideoSource, VideoResolution};
    use std::time::Duration;

    crate::ensure_init()?;
    if !crate::lk_publish::nvenc_available() {
        anyhow::bail!("NVENC not available — the simulcast gate is NVENC-specific");
    }
    let key: [u8; 16] = [0x5A; 16];

    // ---- Publisher (E2EE on, like a real call; dynacast OFF — see doc comment) ----
    let kp_pub = matrix_key_provider();
    let mut pub_opts = RoomOptions::default();
    pub_opts.encryption =
        Some(E2eeOptions { encryption_type: EncryptionType::Gcm, key_provider: kp_pub.clone() });
    let (pub_room, mut pub_events) =
        Room::connect(ws_url, pub_jwt, pub_opts).await.context("simtest pub connect")?;
    let pub_identity = pub_room.local_participant().identity().0;
    kp_pub.set_key(&ParticipantIdentity(pub_identity.clone()), 0, key.to_vec());
    tokio::spawn(async move { while pub_events.recv().await.is_some() {} });

    // ---- Subscriber (key installed; retains the publication = the receive levers) ----
    struct SubState {
        pubn: Option<livekit::publication::RemoteTrackPublication>,
        frames: u64,
        last_w: u32,
        last_h: u32,
    }
    let sub_state = Arc::new(Mutex::new(SubState { pubn: None, frames: 0, last_w: 0, last_h: 0 }));
    let kp_sub = matrix_key_provider();
    kp_sub.set_key(&ParticipantIdentity(pub_identity.clone()), 0, key.to_vec());
    let mut sub_opts = RoomOptions::default();
    sub_opts.encryption =
        Some(E2eeOptions { encryption_type: EncryptionType::Gcm, key_provider: kp_sub });
    let (sub_room, mut sub_events) =
        Room::connect(ws_url, sub_jwt, sub_opts).await.context("simtest sub connect")?;
    let st2 = sub_state.clone();
    let drain = tokio::spawn(async move {
        while let Some(ev) = sub_events.recv().await {
            match ev {
                RoomEvent::TrackSubscribed { track: RemoteTrack::Video(t), publication, .. } => {
                    eprintln!(
                        "[simtest] subscriber got video sid={} simulcasted={}",
                        publication.sid().as_str(),
                        publication.simulcasted()
                    );
                    st2.lock().unwrap().pubn = Some(publication);
                    let st3 = st2.clone();
                    let rtc = t.rtc_track();
                    tokio::spawn(async move {
                        let mut stream = NativeVideoStream::new(rtc);
                        while let Some(frame) = stream.next().await {
                            let mut g = st3.lock().unwrap();
                            g.frames += 1;
                            g.last_w = frame.buffer.width();
                            g.last_h = frame.buffer.height();
                        }
                    });
                }
                RoomEvent::TrackUnsubscribed { .. } => {
                    st2.lock().unwrap().pubn = None;
                }
                _ => {}
            }
        }
    });

    let mut reports = Vec::new();
    for (w, h, kbps, probe_levers) in [(1280u32, 720u32, 2_500u64, true), (1920, 1080, 3_000, false)] {
        let mut rep =
            SimulcastReport { phase: format!("{w}x{h}"), ..Default::default() };
        eprintln!("[simtest] ── phase {} ──", rep.phase);
        let source =
            NativeVideoSource::new(VideoResolution { width: w, height: h }, false);
        let track =
            LocalVideoTrack::create_video_track("camera", RtcVideoSource::Native(source.clone()));
        let stats_track = track.clone();
        let publication = pub_room
            .local_participant()
            .publish_track(
                LocalTrack::Video(track),
                TrackPublishOptions {
                    video_codec: VideoCodec::H264,
                    video_encoder: VideoEncoderBackend::Nvenc,
                    source: TrackSource::Camera,
                    video_encoding: Some(VideoEncoding {
                        max_bitrate: kbps * 1000,
                        max_framerate: 30.0,
                    }),
                    simulcast: true,
                    // The EXACT production rung derivation (2x/4x ratios) — the gate
                    // must exercise what publish_camera ships.
                    simulcast_layers: Some(camera_simulcast_layers(w, h)),
                    ..Default::default()
                },
            )
            .await
            .context("simtest publish")?;
        // Synthetic moving gradient at 30fps (cf. e2ee_loopback) so every rung has content.
        let feed = tokio::spawn(async move {
            let mut tick: i64 = 0;
            let mut interval = tokio::time::interval(Duration::from_millis(33));
            loop {
                interval.tick().await;
                let mut buf = I420Buffer::new(w, h);
                let (y, u, v) = buf.data_mut();
                let s = (tick % 255) as u8;
                y.iter_mut().enumerate().for_each(|(i, p)| *p = ((i as u8) ^ s) | 0x40);
                u.iter_mut().for_each(|p| *p = 128);
                v.iter_mut().for_each(|p| *p = 128);
                source.capture_frame(&VideoFrame {
                    rotation: VideoRotation::VideoRotation0,
                    timestamp_us: tick * 33_333,
                    frame_metadata: None,
                    buffer: buf,
                });
                tick += 1;
            }
        });

        // Warm-up, then read per-rid outbound stats.
        // Wait for the encoder to actually spin up EVERY rung. A single fixed-time sample
        // flaked: under a slow BWE ramp the top rung can still be below the frames_encoded
        // threshold at t=10s, so a perfectly healthy stream reported rungs_encoding=2 and
        // the gate FAILED. Poll instead — same assertion, just measured once the stream has
        // settled (bounded, so a genuinely dead rung still fails).
        {
            use livekit::webrtc::stats::RtcStats;
            let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
            loop {
                tokio::time::sleep(Duration::from_secs(5)).await;
                let ready = match stats_track.get_stats().await {
                    Ok(st) => st
                        .iter()
                        .filter(|s| {
                            matches!(s, RtcStats::OutboundRtp(o) if o.outbound.frames_encoded > 30)
                        })
                        .count(),
                    Err(_) => 0,
                };
                if ready >= 3 || tokio::time::Instant::now() >= deadline {
                    break;
                }
            }
        }
        {
            use livekit::webrtc::stats::RtcStats;
            let stats = stats_track.get_stats().await.context("simtest stats")?;
            for s in &stats {
                if let RtcStats::OutboundRtp(o) = s {
                    let o = &o.outbound;
                    let line = format!(
                        "rid='{}' enc='{}' {}x{}@{:.0} frames={} target={}kbps",
                        o.rid, o.encoder_implementation, o.frame_width, o.frame_height,
                        o.frames_per_second, o.frames_encoded,
                        (o.target_bitrate / 1000.0) as u64
                    );
                    eprintln!("[simtest] {line}");
                    if o.frames_encoded > 30 {
                        rep.rungs_encoding += 1;
                        let enc = o.encoder_implementation.to_lowercase();
                        if enc.contains("nvidia") || enc.contains("nvenc") {
                            rep.nvenc_rungs += 1;
                        }
                    }
                    rep.rung_lines.push(line);
                }
            }
        }

        rep.levers_expected = probe_levers;
        if probe_levers {
            // ── Receive levers, proven against THIS SFU ──
            let sub_pub = sub_state.lock().unwrap().pubn.clone();
            if let Some(sp) = sub_pub {
                {
                    let g = sub_state.lock().unwrap();
                    rep.dims_before = (g.last_w, g.last_h);
                }
                eprintln!("[simtest] subscriber requesting 320x180 (layer switch)…");
                sp.update_video_dimensions(livekit::track::TrackDimension(320, 180));
                tokio::time::sleep(Duration::from_secs(6)).await;
                {
                    let g = sub_state.lock().unwrap();
                    rep.dims_after = (g.last_w, g.last_h);
                }
                eprintln!(
                    "[simtest] decoded size {}x{} -> {}x{}",
                    rep.dims_before.0, rep.dims_before.1, rep.dims_after.0, rep.dims_after.1
                );
                // Pause: frames must STOP flowing.
                let before_pause = sub_state.lock().unwrap().frames;
                eprintln!("[simtest] subscriber set_enabled(false) (SFU pause)…");
                sp.set_enabled(false);
                tokio::time::sleep(Duration::from_secs(4)).await;
                let paused_delta = sub_state.lock().unwrap().frames - before_pause;
                rep.pause_worked = paused_delta < 15; // allow in-flight stragglers
                eprintln!("[simtest] frames during pause: {paused_delta} (pause_worked={})", rep.pause_worked);
                // Resume: frames must FLOW again.
                let before_resume = sub_state.lock().unwrap().frames;
                sp.set_enabled(true);
                tokio::time::sleep(Duration::from_secs(4)).await;
                let resumed_delta = sub_state.lock().unwrap().frames - before_resume;
                rep.resume_worked = resumed_delta > 15;
                eprintln!("[simtest] frames after resume: {resumed_delta} (resume_worked={})", rep.resume_worked);
            } else {
                eprintln!("[simtest] subscriber never subscribed — lever probe skipped");
            }
        }
        rep.sub_frames = sub_state.lock().unwrap().frames;

        feed.abort();
        let _ = tokio::time::timeout(
            Duration::from_secs(3),
            pub_room.local_participant().unpublish_track(&publication.sid()),
        )
        .await;
        // Let the unpublish settle (subscriber TrackUnsubscribed) before the next phase.
        tokio::time::sleep(Duration::from_secs(2)).await;
        sub_state.lock().unwrap().frames = 0;
        reports.push(rep);
    }

    drain.abort();
    pub_room.close().await.ok();
    sub_room.close().await.ok();
    Ok(reports)
}

/// Build the mic capture pipeline + feeder task that pushes PCM into `source`.
/// Set by the mic feeder when audiorate reports heavy timestamp corrections right after
/// capture start — the join-time "static mic" signature (a cold PipeWire node hands the
/// graph broken timestamps; audiorate's fill/drop corrections ARE the garble). Consumed
/// (one-shot) by the app's post-join self-heal, which rebuilds the capture pipeline exactly
/// like the RNNoise toggle does — the manual fix users discovered, automated.
/// Participants stuck in MissingKey longer than `threshold_secs`, each reported ONCE per
/// episode (the map entry is marked). Polled by the app's peer-key loop, which is already
/// the retry mechanism — this makes a stuck episode LOUD instead of observable-only (the
/// 2026-08-19 zombie call ran 12 minutes black/silent with two quiet recv5 lines).
pub fn stuck_missing_keys(threshold_secs: u64) -> Vec<(String, u64)> {
    let mut out = Vec::new();
    let mut map = missing_key_since().lock().unwrap();
    for (id, entry) in map.iter_mut() {
        let secs = entry.0.elapsed().as_secs();
        if !entry.1 && secs >= threshold_secs {
            entry.1 = true;
            out.push((id.clone(), secs));
        }
    }
    out
}

/// 10.5: is `identity` in a MissingKey episode at least `secs` old? ("can't hear X — no key")
pub(crate) fn missing_key_for(identity: &str, secs: u64) -> bool {
    missing_key_since()
        .lock()
        .unwrap()
        .get(identity)
        .map(|e| e.0.elapsed().as_secs() >= secs)
        .unwrap_or(false)
}

/// 10.5 (audio review #5): we published a mic in this call and mean to keep one. A failed
/// re-publish (AGC/NS toggle) or a reconnect that dropped the publication leaves `has_mic()`
/// false while this stays true — the lifetime healer publishes it again. Cleared by close().
fn mic_wanted_flag() -> &'static std::sync::atomic::AtomicBool {
    static F: OnceLock<std::sync::atomic::AtomicBool> = OnceLock::new();
    F.get_or_init(|| std::sync::atomic::AtomicBool::new(false))
}
pub fn mic_wanted() -> bool {
    mic_wanted_flag().load(std::sync::atomic::Ordering::Relaxed)
}
/// 10.5 (audio review #2c): the capture is DOWN — a rebuild retired the old capture and every
/// chain after it failed. Nothing is left to re-raise the unhealthy flag, so the healer must not
/// treat the silence as "clean". Cleared when a capture commits.
fn mic_down_flag() -> &'static std::sync::atomic::AtomicBool {
    static F: OnceLock<std::sync::atomic::AtomicBool> = OnceLock::new();
    F.get_or_init(|| std::sync::atomic::AtomicBool::new(false))
}
pub fn mic_capture_down() -> bool {
    mic_down_flag().load(std::sync::atomic::Ordering::Relaxed)
}
/// Capture ids: every start_mic_capture takes the next one; only a capture that reached PLAYING
/// becomes the CURRENT one ([`mic_gen`]) — see the commit point in start_mic_capture.
fn mic_gen_alloc() -> &'static std::sync::atomic::AtomicU64 {
    static G: OnceLock<std::sync::atomic::AtomicU64> = OnceLock::new();
    G.get_or_init(|| std::sync::atomic::AtomicU64::new(0))
}

/// Per-participant "MissingKey since when" (+ whether the loud line already fired this
/// episode) — see the E2eeStateChanged watchdog.
fn missing_key_since()
-> &'static Mutex<std::collections::HashMap<String, (std::time::Instant, bool)>> {
    static M: OnceLock<Mutex<std::collections::HashMap<String, (std::time::Instant, bool)>>> =
        OnceLock::new();
    M.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

fn mic_capture_unhealthy_flag() -> &'static std::sync::atomic::AtomicBool {
    static F: OnceLock<std::sync::atomic::AtomicBool> = OnceLock::new();
    F.get_or_init(|| std::sync::atomic::AtomicBool::new(false))
}

// ── 0.8.29: WHEN the capture was last seen unhealthy ────────────────────────────────
// The app-side healer used to act on a latch with no idea how old it was: one user's heal 7
// tore down 299 s of flawless capture on a flag raised five minutes earlier, the
// replacement never primed, and the 300 s backoff left them mute. Every raise now stamps
// the time (a stall re-raises every 10 ms, a degraded capture every 10 s window), so
// "nothing re-raised for N seconds" means "clean for N seconds" and the healer can drop
// a stale request instead of destroying a healthy capture.
fn process_epoch() -> std::time::Instant {
    static E: OnceLock<std::time::Instant> = OnceLock::new();
    *E.get_or_init(std::time::Instant::now)
}
fn mic_bad_stamp() -> &'static std::sync::atomic::AtomicU64 {
    static S: OnceLock<std::sync::atomic::AtomicU64> = OnceLock::new();
    S.get_or_init(|| std::sync::atomic::AtomicU64::new(0))
}
fn mic_mark_bad() {
    let ms = process_epoch().elapsed().as_millis() as u64 + 1; // +1: 0 = never
    mic_bad_stamp().store(ms, std::sync::atomic::Ordering::Relaxed);
}
/// Raise the unhealthy flag AND stamp the time. Returns true when this call raised it
/// (it was down before) — the "log once" idiom the feeder's detectors use.
fn raise_mic_unhealthy() -> bool {
    mic_mark_bad();
    !mic_capture_unhealthy_flag().swap(true, std::sync::atomic::Ordering::Relaxed)
}
/// How long ago the capture was last observed unhealthy (None = never in this process).
pub fn mic_capture_bad_age() -> Option<std::time::Duration> {
    let s = mic_bad_stamp().load(std::sync::atomic::Ordering::Relaxed);
    if s == 0 {
        return None;
    }
    let now = process_epoch().elapsed().as_millis() as u64;
    Some(std::time::Duration::from_millis(now.saturating_sub(s - 1)))
}
/// Set when a capture stalled WITHOUT EVER PRIMING (dead chain, rung already blocked by
/// the ladder). The healer rebuilds immediately on this instead of waiting out a backoff.
fn mic_never_primed_flag() -> &'static std::sync::atomic::AtomicBool {
    static F: OnceLock<std::sync::atomic::AtomicBool> = OnceLock::new();
    F.get_or_init(|| std::sync::atomic::AtomicBool::new(false))
}
pub fn take_mic_never_primed() -> bool {
    mic_never_primed_flag().swap(false, std::sync::atomic::Ordering::Relaxed)
}
fn mic_primed_flag() -> &'static std::sync::atomic::AtomicBool {
    static F: OnceLock<std::sync::atomic::AtomicBool> = OnceLock::new();
    F.get_or_init(|| std::sync::atomic::AtomicBool::new(false))
}
/// Has the CURRENT mic capture primed (delivered a stable cushion and started feeding)?
pub fn mic_capture_primed() -> bool {
    mic_primed_flag().load(std::sync::atomic::Ordering::Relaxed)
}
/// Capture GENERATION: bumped by every start_mic_capture. A feeder whose generation is no
/// longer current is a dying task (its pipeline was swapped out) and must not raise flags —
/// the old build-then-swap race re-raised "never primed" from the retired feeder for the
/// ~10 ms between the new build and the abort, and the healer then rebuilt a healthy capture.
fn mic_gen() -> &'static std::sync::atomic::AtomicU64 {
    static G: OnceLock<std::sync::atomic::AtomicU64> = OnceLock::new();
    G.get_or_init(|| std::sync::atomic::AtomicU64::new(0))
}
/// Human label of the chain rung the current capture runs on (for the [mic-rt] line).
fn mic_chain_label() -> &'static Mutex<String> {
    static L: OnceLock<Mutex<String>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(String::new()))
}

// ── Audio-device health (0.8.25) ──────────────────────────────────────────────
//
// Field case (Linux): for a whole call a user's mic captured digital silence (feeder
// ema 0.0001, peers decoded peak 0/32767) and at exit PipeWire showed only its "Dummy
// Output" — the machine had no working audio devices at all. The app said nothing, so
// it looked as if an update had broken the audio. These two flags let the
// call UI say what the capture path can see: a mic that delivers only silence, and an
// output side with no real sink.

/// Does the mic need the user's attention? 10.5: the mic check's red states (a flat line for a
/// minute unmuted, or muted / at 0 % in the system settings) — see hear.rs. The 0.8.25 rule
/// (processed audio below −74 dBFS for 10 s) fired on every pause of a gated mic.
pub fn mic_silent() -> bool {
    crate::hear::mic_needs_attention()
}
fn output_missing_flag() -> &'static std::sync::atomic::AtomicBool {
    static F: OnceLock<std::sync::atomic::AtomicBool> = OnceLock::new();
    F.get_or_init(|| std::sync::atomic::AtomicBool::new(false))
}
/// Did the join-time probe find no usable output device? (PipeWire "Dummy Output" only,
/// or no sink at all.)
pub fn output_missing() -> bool {
    output_missing_flag().load(std::sync::atomic::Ordering::Relaxed)
}
/// 10.0.6: what PipeWire sees of OUR streams, logged inside the Flatpak (or with
/// TINHAT_PW_SNAPSHOT=1) at the moments a report needs it: the server version and graph clock, our
/// clients' access level and API, every node of ours with its state, mute, volumes and target,
/// and its links. A "connected but silent" stream shows here as MUTED, VOLUME 0, NO LINKS or a
/// peer that is not running — the facts a bug report could not carry until now. Runs off the
/// caller's thread; at most one every 15 s unless it is a stall.
pub fn pw_snapshot(reason: &str) {
    if cfg!(windows) || !(in_flatpak_sandbox() || std::env::var_os("TINHAT_PW_SNAPSHOT").is_some()) {
        return;
    }
    static LAST: Mutex<Option<std::time::Instant>> = Mutex::new(None);
    static BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    {
        let mut last = LAST.lock().unwrap();
        if !reason.contains("stall") {
            if let Some(t) = *last {
                if t.elapsed() < std::time::Duration::from_secs(15) {
                    return;
                }
            }
        }
        *last = Some(std::time::Instant::now());
    }
    if BUSY.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let reason = reason.to_string();
    std::thread::spawn(move || {
        for line in pw_snapshot_lines(&reason) {
            eprintln!("[pw-snapshot] {line}");
        }
        BUSY.store(false, std::sync::atomic::Ordering::SeqCst);
    });
}

fn pw_snapshot_lines(reason: &str) -> Vec<String> {
    use serde_json::Value;
    let out = match std::process::Command::new("pw-dump").stderr(std::process::Stdio::null()).output() {
        Ok(o) if o.status.success() => o.stdout,
        Ok(o) => return vec![format!("{reason}: pw-dump failed ({})", o.status)],
        Err(e) => return vec![format!("{reason}: pw-dump unavailable ({e})")],
    };
    let v: Value = match serde_json::from_slice(&out) {
        Ok(v) => v,
        Err(e) => return vec![format!("{reason}: pw-dump output unreadable ({e})")],
    };
    let Some(arr) = v.as_array() else {
        return vec![format!("{reason}: pw-dump output is not a list")];
    };
    fn s(v: &Value) -> String {
        match v {
            Value::String(t) => t.clone(),
            Value::Null => "-".to_string(),
            o => o.to_string(),
        }
    }
    let my_pid = std::process::id().to_string();
    let mut lines = Vec::new();
    let mut server = String::from("?");
    let mut settings = String::from("?");
    let mut my_clients: std::collections::HashSet<u64> = Default::default();
    let mut names: std::collections::HashMap<u64, String> = Default::default();
    let mut states: std::collections::HashMap<u64, String> = Default::default();
    for o in arr {
        let id = o["id"].as_u64().unwrap_or(0);
        let props = &o["info"]["props"];
        match s(&o["type"]).as_str() {
            "PipeWire:Interface:Core" => server = s(&o["info"]["version"]),
            // (metadata objects carry their props at the top level, not under "info")
            "PipeWire:Interface:Metadata" if s(&o["props"]["metadata.name"]) == "settings" => {
                if let Some(m) = o["metadata"].as_array() {
                    let get = |k: &str| {
                        m.iter().find(|e| s(&e["key"]) == k).map(|e| s(&e["value"])).unwrap_or_else(|| "-".into())
                    };
                    settings = format!(
                        "rate {} quantum {} (min {} max {}, forced rate {} quantum {})",
                        get("clock.rate"),
                        get("clock.quantum"),
                        get("clock.min-quantum"),
                        get("clock.max-quantum"),
                        get("clock.force-rate"),
                        get("clock.force-quantum")
                    );
                }
            }
            "PipeWire:Interface:Client" => {
                if s(&props["application.process.id"]) == my_pid {
                    my_clients.insert(id);
                    lines.push(format!(
                        "  client {id} access={} api={} flatpak={} name={:?}",
                        s(&props["pipewire.access"]),
                        s(&props["client.api"]),
                        s(&props["pipewire.sec.flatpak"]),
                        s(&props["application.name"])
                    ));
                }
            }
            "PipeWire:Interface:Node" => {
                names.insert(id, s(&props["node.name"]));
                states.insert(id, s(&o["info"]["state"]));
            }
            _ => {}
        }
    }
    let mut my_nodes: std::collections::HashSet<u64> = Default::default();
    for o in arr {
        if s(&o["type"]) != "PipeWire:Interface:Node" {
            continue;
        }
        let props = &o["info"]["props"];
        if !props["client.id"].as_u64().map(|c| my_clients.contains(&c)).unwrap_or(false) {
            continue;
        }
        let id = o["id"].as_u64().unwrap_or(0);
        my_nodes.insert(id);
        let p = &o["info"]["params"]["Props"][0];
        let mute = s(&p["mute"]);
        let vols = p["channelVolumes"]
            .as_array()
            .map(|a| a.iter().map(|x| format!("{:.2}", x.as_f64().unwrap_or(-1.0))).collect::<Vec<_>>().join(","))
            .unwrap_or_else(|| "-".into());
        let state = s(&o["info"]["state"]);
        let target = if props["target.object"].is_null() { s(&props["node.target"]) } else { s(&props["target.object"]) };
        let mut flags: Vec<&str> = Vec::new();
        if mute == "true" {
            flags.push("MUTED");
        }
        if vols != "-" && vols.split(',').all(|v| v == "0.00") {
            flags.push("VOLUME 0");
        }
        if state != "running" {
            flags.push("NOT RUNNING");
        }
        lines.push(format!(
            "  node {id} {:?} {} state={state} mute={mute} vol=[{vols}] target={target}{}",
            s(&props["node.name"]),
            s(&props["media.class"]),
            if flags.is_empty() { String::new() } else { format!("  <-- {}", flags.join(", ")) }
        ));
    }
    let mut links: std::collections::BTreeMap<String, usize> = Default::default();
    let mut linked: std::collections::HashSet<u64> = Default::default();
    for o in arr {
        if s(&o["type"]) != "PipeWire:Interface:Link" {
            continue;
        }
        let (Some(a), Some(b)) = (o["info"]["output-node-id"].as_u64(), o["info"]["input-node-id"].as_u64()) else {
            continue;
        };
        if !(my_nodes.contains(&a) || my_nodes.contains(&b)) {
            continue;
        }
        linked.insert(a);
        linked.insert(b);
        let nm = |n: u64| format!("{} {:?} [{}]", n, names.get(&n).cloned().unwrap_or_default(), states.get(&n).cloned().unwrap_or_default());
        *links.entry(format!("    link {} -> {} ({})", nm(a), nm(b), s(&o["info"]["state"]))).or_insert(0) += 1;
    }
    for (l, c) in links {
        lines.push(if c > 1 { format!("{l} x{c}") } else { l });
    }
    for n in &my_nodes {
        if !linked.contains(n) {
            lines.push(format!("    node {n} has NO LINKS"));
        }
    }
    if my_nodes.is_empty() {
        lines.push("  (no nodes of ours in the graph)".to_string());
    }
    let ours = gst::ElementFactory::find("pipewiresrc")
        .and_then(|f| f.plugin())
        .map(|p| p.version().to_string())
        .unwrap_or_else(|| "?".into());
    lines.insert(0, format!("{reason}: server {server}, our gstpipewire {ours}, graph {settings}"));
    lines
}

/// Enumerate the audio devices once at join and log the verdict. Cheap (two DeviceMonitor
/// scans, ~100 ms, off the GUI thread — connect() is async).
pub fn audio_health_probe() {
    let sinks = list_devices("Audio/Sink");
    let sources = list_devices("Audio/Source");
    let def_sink = default_audio_sink().unwrap_or_default();
    let def_src = default_audio_source().unwrap_or_default();
    let missing = sinks.is_empty() || def_sink == "auto_null";
    output_missing_flag().store(missing, std::sync::atomic::Ordering::Relaxed);
    eprintln!(
        "[audio-health] outputs={} default_sink={:?} inputs={} default_source={:?}{}",
        sinks.len(),
        def_sink,
        sources.len(),
        def_src,
        if missing { " — NO USABLE OUTPUT DEVICE (PipeWire dummy output / no sink): nothing can be heard" } else { "" }
    );
}
/// Read-and-clear the "mic capture started unhealthy" flag (see the feeder's early-window
/// audiorate watch). Returns true at most once per detection.
pub fn take_mic_capture_unhealthy() -> bool {
    mic_capture_unhealthy_flag().swap(false, std::sync::atomic::Ordering::Relaxed)
}

/// Raised when the Windows mic builder had to DROP the echo-cancel dsp from the chain
/// ("mic beats AEC" fallback). Consumed by the app-side call healer, which schedules ONE
/// delayed mic rebuild so AEC re-attaches once the reference tap has settled — before
/// this, the fallback was permanent for the rest of the call with no indication (the
/// user became an echo source; a field log).
fn aec_fallback_flag() -> &'static std::sync::atomic::AtomicBool {
    static F: OnceLock<std::sync::atomic::AtomicBool> = OnceLock::new();
    F.get_or_init(|| std::sync::atomic::AtomicBool::new(false))
}
/// Read-and-clear the "AEC was dropped from the mic chain" flag.
pub fn take_aec_fallback() -> bool {
    aec_fallback_flag().swap(false, std::sync::atomic::Ordering::Relaxed)
}

// ── 0.8.28: the WebRTC-stage LADDER ─────────────────────────────────────────────────
// Field logs (five sessions from two users — 0.8.25…0.8.27 on real Linux hardware)
// all show the same shape: the FIRST mic chain of every call (webrtcdsp + echo probe)
// delivers nothing for 10 s, the healer rebuilds, and EVERY rebuild then fails to reach
// PAUSED because the retiring pipeline still held the probe — so the whole stage was
// silently dropped for the rest of the call: no echo cancellation, no "Standard" noise
// suppression, no AGC, while the settings still read ON. A chain variant that stalls is
// now blocked for the rest of the call and the next build steps down ONE rung:
//   0 = full stage (echo-cancel + NS + AGC)  →  1 = stage without echo-cancel  →  2 = plain.
fn dsp_stage_block() -> &'static std::sync::atomic::AtomicU8 {
    static B: OnceLock<std::sync::atomic::AtomicU8> = OnceLock::new();
    B.get_or_init(|| std::sync::atomic::AtomicU8::new(0))
}
/// Echo cancellation is off the table for this call (its chain stalled once).
pub(crate) fn dsp_ec_blocked() -> bool {
    dsp_stage_block().load(std::sync::atomic::Ordering::Relaxed) >= 1
}
/// The whole WebRTC stage is off the table for this call (it stalled even without the probe).
pub(crate) fn dsp_blocked() -> bool {
    dsp_stage_block().load(std::sync::atomic::Ordering::Relaxed) >= 2
}
fn dsp_stage_step_down(had_probe: bool) {
    let to = if had_probe { 1 } else { 2 };
    let prev = dsp_stage_block().fetch_max(to, std::sync::atomic::Ordering::Relaxed);
    if prev < to {
        if had_probe {
            eprintln!(
                "[mic] the echo-cancel chain delivered nothing on this machine — for the rest of \
                 this call the WebRTC stage runs WITHOUT echo cancellation (noise suppression + AGC stay)"
            );
        } else {
            eprintln!(
                "[mic] the WebRTC stage delivered nothing even without the echo probe — for the rest \
                 of this call the mic is the plain capture (no in-app NS/AGC; RNNoise still available)"
            );
        }
        aec_degraded_flag().store(true, std::sync::atomic::Ordering::Relaxed);
    }
}
/// New call ⇒ a fresh ladder (a transient at join must not degrade every later call).
pub(crate) fn reset_dsp_stage_block() {
    dsp_stage_block().store(0, std::sync::atomic::Ordering::Relaxed);
}
/// Bits describing the chain the last `build_mic_pipeline` produced: bit0 = webrtcdsp present,
/// bit1 = echo probe bound. Read by the feeder so a never-primed stall can blame the right rung.
fn mic_chain_flags() -> &'static std::sync::atomic::AtomicU8 {
    static F: OnceLock<std::sync::atomic::AtomicU8> = OnceLock::new();
    F.get_or_init(|| std::sync::atomic::AtomicU8::new(0))
}
/// One-shot overrides for the NEXT `build_mic_pipeline`: plain chain only / default input.
fn force_plain_mic_once() -> &'static std::sync::atomic::AtomicBool {
    static F: OnceLock<std::sync::atomic::AtomicBool> = OnceLock::new();
    F.get_or_init(|| std::sync::atomic::AtomicBool::new(false))
}
fn force_default_mic_once() -> &'static std::sync::atomic::AtomicBool {
    static F: OnceLock<std::sync::atomic::AtomicBool> = OnceLock::new();
    F.get_or_init(|| std::sync::atomic::AtomicBool::new(false))
}

// ── 0.8.31: the echo-cancellation gauge ────────────────────────────────────────────────
//
// Energy of the mic signal entering and leaving the WebRTC stage, split by whether the far
// end (the reference the canceller subtracts) was active within the last 400 ms. `sup_fe` =
// dB the stage removed while the far end was playing, `sup_ne` = dB it removed while it was
// not (noise suppression / high-pass / AGC — the stage's non-echo effect); their difference
// is the echo return loss enhancement actually delivered on THIS machine, in THIS call —
// the number the field reports asked for, instead of a listening test. Atomics only: it runs on the
// streaming thread, 480 floats per 10 ms, three taps.
pub(crate) struct EcGauge {
    t0: std::time::Instant,
    ref_hot_until_ns: std::sync::atomic::AtomicU64,
    last_pre: std::sync::atomic::AtomicU64, // energy of the frame entering the stage (×1e9)
    fe: [std::sync::atomic::AtomicU64; 3],  // frames / pre energy / post energy — far end active
    ne: [std::sync::atomic::AtomicU64; 3],  // same — far end quiet
    ref_frames: std::sync::atomic::AtomicU64,
    ref_hot_frames: std::sync::atomic::AtomicU64,
    // 1 s sub-windows: frames / far-end frames / pre / post — and the best far-end-only second
    // of the window (dB ×10, as i64). Double talk drags the window MEAN down (near speech
    // dominates post), so the peak is what says "the canceller works on this machine".
    sub: [std::sync::atomic::AtomicU64; 4],
    best_db_x10: std::sync::atomic::AtomicI64,
}
const EC_E_SCALE: f64 = 1e9;
fn ec_frame_energy(b: &gst::BufferRef) -> (f64, usize) {
    match b.map_readable() {
        Ok(m) => {
            let s = m.as_slice();
            let e: f64 = s
                .chunks_exact(4)
                .map(|c| {
                    let v = f32::from_le_bytes([c[0], c[1], c[2], c[3]]) as f64;
                    v * v
                })
                .sum();
            (e, s.len() / 4)
        }
        Err(_) => (0.0, 0),
    }
}
pub(crate) struct EcWindow {
    pub frames: u64,
    pub fe_frames: u64,
    pub sup_fe_db: f64,
    pub sup_ne_db: f64,
    pub erle_db: f64,
    pub peak_db: f64, // best far-end-only second of the window (NAN when there was none)
}
impl EcWindow {
    pub fn fe_frac(&self) -> f64 {
        if self.frames == 0 { 0.0 } else { self.fe_frames as f64 / self.frames as f64 }
    }
    pub fn report(&self) -> String {
        if self.frames == 0 {
            return "n/a".to_string();
        }
        let pct = (self.fe_frac() * 100.0).round() as u32;
        if self.fe_frames < 100 {
            format!("n/a(far end quiet,fe={pct}%)")
        } else {
            format!(
                "{:.1}dB(peak={},fe={pct}%,sup={:.1}/{:.1})",
                self.erle_db,
                if self.peak_db.is_nan() { "n/a".to_string() } else { format!("{:.1}dB", self.peak_db) },
                self.sup_fe_db,
                self.sup_ne_db
            )
        }
    }
}
impl EcGauge {
    /// Attach to the chain's `tinhatdsp` + this build's probe. `None` when the chain has no probe.
    pub(crate) fn attach(pipeline: &gst::Pipeline) -> Option<Arc<EcGauge>> {
        use std::sync::atomic::{AtomicU64, Ordering};
        let dsp = pipeline.by_name("tinhatdsp")?;
        let probe = pipeline.by_name(&ec_probe_name())?;
        let g = Arc::new(EcGauge {
            t0: std::time::Instant::now(),
            ref_hot_until_ns: AtomicU64::new(0),
            last_pre: AtomicU64::new(0),
            fe: [AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0)],
            ne: [AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0)],
            ref_frames: AtomicU64::new(0),
            ref_hot_frames: AtomicU64::new(0),
            sub: [AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0)],
            best_db_x10: std::sync::atomic::AtomicI64::new(i64::MIN),
        });
        // 0.8.33: one probe per speaker output — any of them marks the far end hot; "pre" is
        // the FIRST stage's input (the per-output echo-only stages sit ahead of tinhatdsp).
        let mut probes = vec![probe];
        let mut i = 1;
        while let Some(p) = pipeline.by_name(&ec_probe_name_n(i)) {
            probes.push(p);
            i += 1;
        }
        let first = pipeline.by_name("tinhatdspx1").unwrap_or_else(|| dsp.clone());
        // Reference: the far end is "hot" for 400 ms after any frame above ≈ −50 dBFS RMS
        // (covers the speaker→mic path plus the canceller's own delay estimate).
        for probe in &probes {
        if let Some(pad) = probe.static_pad("sink") {
            let gg = g.clone();
            pad.add_probe(gst::PadProbeType::BUFFER, move |_pad, info| {
                if let Some(gst::PadProbeData::Buffer(ref b)) = info.data {
                    let (e, n) = ec_frame_energy(b);
                    gg.ref_frames.fetch_add(1, Ordering::Relaxed);
                    if n > 0 && e / n as f64 > 1e-5 {
                        gg.ref_hot_frames.fetch_add(1, Ordering::Relaxed);
                        gg.ref_hot_until_ns.store(gg.now_ns() + 400_000_000, Ordering::Relaxed);
                    }
                }
                gst::PadProbeReturn::Ok
            });
        }
        }
        if let Some(pad) = first.static_pad("sink") {
            let gg = g.clone();
            pad.add_probe(gst::PadProbeType::BUFFER, move |_pad, info| {
                if let Some(gst::PadProbeData::Buffer(ref b)) = info.data {
                    let (e, _) = ec_frame_energy(b);
                    gg.last_pre.store((e * EC_E_SCALE) as u64, Ordering::Relaxed);
                }
                gst::PadProbeReturn::Ok
            });
        }
        if let Some(pad) = dsp.static_pad("src") {
            let gg = g.clone();
            pad.add_probe(gst::PadProbeType::BUFFER, move |_pad, info| {
                if let Some(gst::PadProbeData::Buffer(ref b)) = info.data {
                    let (e, _) = ec_frame_energy(b);
                    let pre = gg.last_pre.swap(0, Ordering::Relaxed);
                    let post = (e * EC_E_SCALE) as u64;
                    let hot = gg.now_ns() < gg.ref_hot_until_ns.load(Ordering::Relaxed);
                    let acc = if hot { &gg.fe } else { &gg.ne };
                    acc[0].fetch_add(1, Ordering::Relaxed);
                    acc[1].fetch_add(pre, Ordering::Relaxed);
                    acc[2].fetch_add(post, Ordering::Relaxed);
                    // the 1 s sub-window (all on this one streaming thread)
                    let n = gg.sub[0].fetch_add(1, Ordering::Relaxed) + 1;
                    if hot {
                        gg.sub[1].fetch_add(1, Ordering::Relaxed);
                    }
                    gg.sub[2].fetch_add(pre, Ordering::Relaxed);
                    gg.sub[3].fetch_add(post, Ordering::Relaxed);
                    if n >= 100 {
                        let fe = gg.sub[1].swap(0, Ordering::Relaxed);
                        let p = gg.sub[2].swap(0, Ordering::Relaxed);
                        let q = gg.sub[3].swap(0, Ordering::Relaxed);
                        gg.sub[0].store(0, Ordering::Relaxed);
                        if fe >= 90 && p > 0 && q > 0 {
                            let d = (10.0 * (p as f64 / q as f64).log10() * 10.0).round() as i64;
                            gg.best_db_x10.fetch_max(d, Ordering::Relaxed);
                        }
                    }
                }
                gst::PadProbeReturn::Ok
            });
        }
        Some(g)
    }
    fn now_ns(&self) -> u64 {
        self.t0.elapsed().as_nanos() as u64
    }
    /// Read-and-reset the window.
    pub(crate) fn take_window(&self) -> EcWindow {
        use std::sync::atomic::Ordering;
        let take = |a: &[std::sync::atomic::AtomicU64; 3]| {
            (a[0].swap(0, Ordering::Relaxed), a[1].swap(0, Ordering::Relaxed), a[2].swap(0, Ordering::Relaxed))
        };
        let (fe_n, fe_pre, fe_post) = take(&self.fe);
        let (ne_n, ne_pre, ne_post) = take(&self.ne);
        self.ref_frames.store(0, Ordering::Relaxed);
        self.ref_hot_frames.store(0, Ordering::Relaxed);
        let sup = |pre: u64, post: u64| -> f64 {
            if pre == 0 || post == 0 { 0.0 } else { 10.0 * (pre as f64 / post as f64).log10() }
        };
        let sup_fe_db = sup(fe_pre, fe_post);
        let sup_ne_db = if ne_n >= 100 { sup(ne_pre, ne_post) } else { 0.0 };
        let best = self.best_db_x10.swap(i64::MIN, Ordering::Relaxed);
        let peak_db = if best == i64::MIN { f64::NAN } else { best as f64 / 10.0 };
        EcWindow { frames: fe_n + ne_n, fe_frames: fe_n, sup_fe_db, sup_ne_db, erle_db: sup_fe_db - sup_ne_db, peak_db }
    }
}

// ── 9.2.1: the echo reference is read FIRST-IN-FIRST-OUT, not by timestamp ────────────────
//
// webrtcechoprobe hands webrtcdsp the 10 ms of reference audio it believes matches each
// capture block by arithmetic on the capture buffer's PTS, the newest reference PTS, the
// pipeline latency and the reference branch's upstream latency (gstwebrtcechoprobe.cpp,
// gst_webrtc_echo_probe_read). When that says the reference is "ahead" it pads the block with
// silence — a whole block of it once the gap reaches 10 ms — and when it says "behind" it
// flushes reference history. Both depend on the PipeWire quantum and on which of our two
// capture streams happens to be delivered first in a graph cycle. On a laptop at the stock
// 1024-sample quantum (21 ms buffers — three field laptops: `srcbuf=46.9/s`)
// that fed AEC3 a reference that kept slipping: the field gauge showed 1–5 dB removed with
// 30–50 dB peaks in the moments it lined up (the field report: "echo cancellation is basically not
// working from the laptop"), and the rig bench at the same quantum decayed from 40 dB to 3 dB
// inside 25 s of the same call.
//
// The probe has a second path: a capture buffer WITHOUT a timestamp gets the next 10 ms in
// arrival order (its "delay agnostic" mode). That is how every WebRTC host feeds AEC3 — render
// and capture in lockstep, the canceller estimating the delay itself — and once paired it
// cannot slip, because PipeWire rate-locks both capture streams to one graph clock. So:
//
//  * the mic buffers lose their PTS on the way INTO the stage (a pad probe) and get a
//    continuous one back on the way OUT (10 ms per block from the first PTS);
//  * the pairing is fixed ONCE, by timestamp, at the start: reference buffers are dropped at
//    the probe's sink until the mic has delivered its first block, and then every reference
//    buffer that ends before that block's PTS is dropped too — so the first reference the
//    probe ever pairs starts at or after the first capture block (the reference leads by up
//    to one reference buffer, never lags; AEC3 wants exactly that);
//  * a read that would find fewer than 10 ms of reference waits for it (the mic streaming
//    thread sleeps, at most two reference buffers + 10 ms, and only while the reference is
//    still arriving) — a silence read is the one thing that would move the pairing.
//
// Nothing is ever trimmed afterwards: with lockstep consumption the pairing offset is a
// constant, and the probe's adapter holds whatever history the delivery phase leaves (a few
// buffers). The offset is measured every read from the PTS the mic buffer carried and the
// newest reference PTS, and reported per window (`o=`) so a field log shows the alignment
// AEC3 is working with. `TINHAT_AEC_TS=1` keeps the old timestamp mode (bench A/B).
/// 10.5.2 (verifier, 2026-09-24): the echo reference's timestamps jumped ~2^32 segments ahead of the mic's.
/// GstAudioBaseSrc's skew slaving (pulsesrc, and every other audiobasesrc — the PulseAudio-compat head the Flatpak
/// uses) resyncs at its first buffer with `segment_diff = running_time_segment - last_written_segment` in unsigned
/// arithmetic and `gst_audio_ring_buffer_advance(guint)`: when the device has already written more than the
/// pipeline's running time, that underflows and the source starts ≈497 days (2^32 × 10 ms) ahead. In that state AEC3
/// removed nothing for the whole call (the Flatpak on the test machine, output on default: far end back at −2.5 dB, ERLE ≤ 0; seen
/// in 10 of 15 calls of 10.5.2 candidates, 0 of 6 with 10.5.1 — the trigger is timing). Set by the FIFO's reference
/// probe and owned by ONE mic build (a generation: a retired pipeline's late probe can neither flag nor clear the
/// running one — Astra 2026-09-24); the call-lifetime healer rebuilds the mic (ec_rebind_wanted), which starts a fresh
/// reference.
static EC_FIFO_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// the generation whose reference wrapped (0 = none)
static EC_REF_WRAPPED_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// A reference this far ahead of the mic is the underflow above, never a real clock offset.
const EC_REF_WRAP_NS: u64 = 3_600_000_000_000;
/// Pure: does a reference buffer ending at `end` (ns) belong to the underflowed skew resync, given the mic's NEWEST
/// capture timestamp `mic_now` (ns)? (Against the newest, not the first: both clocks run on in a healthy call — a
/// comparison with the first block tripped every call after an hour, Astra 2026-09-24.)
fn ec_ref_is_wrapped(end: u64, mic_now: u64) -> bool {
    end > mic_now.saturating_add(EC_REF_WRAP_NS)
}
/// Is the RUNNING mic build's reference in the wrapped state?
fn ec_ref_wrapped_now() -> bool {
    use std::sync::atomic::Ordering;
    let g = EC_REF_WRAPPED_GEN.load(Ordering::Relaxed);
    g != 0 && g == EC_FIFO_GEN.load(Ordering::Relaxed)
}
/// 10.5.3: the underflow-triggered mic rebuild is OPT-IN — off by default, `TINHAT_EC_UNDERFLOW_REBUILD=1` turns it on.
/// The 10.5.2 sandbox gate (the Flatpak on the test machine, 2026-09-24) found that rebuilding the mic after the underflow gave
/// the listener 15–40 s of lag jumps (3 of 3 rebuild runs), while 10.5.1 in the same underflow state keeps a steady
/// lag (and cancels nothing, as before). The detector still logs the underflow, so field logs show when it happens.
/// Read once per process.
fn ec_underflow_rebuild_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| ec_underflow_rebuild_opt_in(std::env::var("TINHAT_EC_UNDERFLOW_REBUILD").ok().as_deref()))
}
/// Pure: `TINHAT_EC_UNDERFLOW_REBUILD`'s value → rebuild? Only "1" (the TINHAT_* convention); unset/""/"0"/other = off.
fn ec_underflow_rebuild_opt_in(v: Option<&str>) -> bool {
    v == Some("1")
}
/// Pure: the call-lifetime healer's reason to rebuild the mic for a wrapped reference (`None` = no rebuild).
fn ec_wrap_rebuild_reason(bound: bool, wrapped: bool, opted_in: bool) -> Option<String> {
    (bound && wrapped && opted_in)
        .then(|| "the echo reference started out of step with the mic (its clock jumped ahead ~497 days)".to_string())
}
/// Pure: the detector's log line (kept in every build so field logs show the underflow; the tail says whether the
/// healer will rebuild — never on Windows, where ec_rebind_wanted is off).
fn ec_wrap_log_line(days: u64, rebuild: bool) -> String {
    format!(
        "[aec] the echo reference started {days} days ahead of the mic (GStreamer skew resync underflow) — it cancels nothing; {}",
        if rebuild { "rebuilding" } else { "not rebuilding (TINHAT_EC_UNDERFLOW_REBUILD=1 turns the rebuild on)" }
    )
}

pub(crate) struct EcFifo {
    /// reference samples in the probe's adapter (pushed − read) — an estimate, kept honest by
    /// the fact that a real starvation or over-read pushes it back toward the truth
    backlog: std::sync::atomic::AtomicI64,
    primed: std::sync::atomic::AtomicBool,
    last_ref_ns: std::sync::atomic::AtomicU64,
    /// samples per reference buffer (the reference stream's quantum), from the first one
    ref_buf: std::sync::atomic::AtomicI64,
    /// PTS (ns + 1) at which the newest reference buffer ENDS
    ref_end_pts: std::sync::atomic::AtomicU64,
    silence_reads: std::sync::atomic::AtomicU64,
    /// reference buffers dropped: before the mic started / older than its first block
    pre_drops: std::sync::atomic::AtomicU64,
    old_drops: std::sync::atomic::AtomicU64,
    holds: std::sync::atomic::AtomicU64,
    hold_ms: std::sync::atomic::AtomicU64,
    min_backlog: std::sync::atomic::AtomicI64,
    max_backlog: std::sync::atomic::AtomicI64,
    /// pairing offset (capture PTS − paired reference PTS), ms, last / window min / window max
    o_last: std::sync::atomic::AtomicI64,
    o_min: std::sync::atomic::AtomicI64,
    o_max: std::sync::atomic::AtomicI64,
}
pub(crate) struct EcFifoSet {
    t0: std::time::Instant,
    first_pts: std::sync::atomic::AtomicU64, // ns + 1 (0 = the mic has not delivered yet)
    /// the newest capture PTS seen at the stage (ns + 1) — the wrap check's base
    last_cap_pts: std::sync::atomic::AtomicU64,
    /// this mic build's generation (EC_FIFO_GEN)
    gen: u64,
    blocks_out: std::sync::atomic::AtomicU64,
    fifos: Vec<Arc<EcFifo>>,
}
const EC_BLOCK: i64 = 480; // 10 ms at 48 k
impl EcFifoSet {
    /// FIFO reference for every WebRTC stage in the chain (None: no stage / no probe /
    /// TINHAT_AEC_TS=1). Attach BEFORE the pipeline starts.
    pub(crate) fn attach(pipeline: &gst::Pipeline) -> Option<Arc<EcFifoSet>> {
        use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
        if std::env::var("TINHAT_AEC_TS").map(|v| v == "1").unwrap_or(false) {
            return None;
        }
        let main = pipeline.by_name("tinhatdsp")?;
        let xafter = std::env::var("TINHAT_AEC_XAFTER").map(|v| v == "1").unwrap_or(false);
        let mut extras: Vec<gst::Element> = Vec::new();
        let mut i = 1;
        while let Some(d) = pipeline.by_name(&format!("tinhatdspx{i}")) {
            extras.push(d);
            i += 1;
        }
        let mut dsps: Vec<gst::Element> = Vec::new();
        if xafter {
            dsps.push(main);
            dsps.extend(extras);
        } else {
            dsps.extend(extras);
            dsps.push(main);
        }
        // every stage that has a probe, in chain order
        let mut stages: Vec<(gst::Element, gst::Element, Arc<EcFifo>)> = Vec::new();
        for dsp in &dsps {
            let probe_name: Option<String> = dsp.property("probe");
            let probe = match probe_name.as_deref().filter(|s| !s.is_empty()).and_then(|n| pipeline.by_name(n)) {
                Some(p) => p,
                None => continue,
            };
            let f = Arc::new(EcFifo {
                backlog: AtomicI64::new(0),
                primed: AtomicBool::new(false),
                last_ref_ns: AtomicU64::new(0),
                ref_buf: AtomicI64::new(0),
                ref_end_pts: AtomicU64::new(0),
                silence_reads: AtomicU64::new(0),
                pre_drops: AtomicU64::new(0),
                old_drops: AtomicU64::new(0),
                holds: AtomicU64::new(0),
                hold_ms: AtomicU64::new(0),
                min_backlog: AtomicI64::new(i64::MAX),
                max_backlog: AtomicI64::new(i64::MIN),
                o_last: AtomicI64::new(i64::MIN),
                o_min: AtomicI64::new(i64::MAX),
                o_max: AtomicI64::new(i64::MIN),
            });
            stages.push((dsp.clone(), probe, f));
        }
        if stages.is_empty() {
            return None;
        }
        let set = Arc::new(EcFifoSet {
            t0: std::time::Instant::now(),
            first_pts: AtomicU64::new(0),
            last_cap_pts: AtomicU64::new(0),
            gen: EC_FIFO_GEN.fetch_add(1, Ordering::Relaxed) + 1,
            blocks_out: AtomicU64::new(0),
            fifos: stages.iter().map(|(_, _, f)| f.clone()).collect(),
        });
        let n_stages = stages.len();
        for (k, (dsp, probe, f)) in stages.iter().enumerate() {
            let f = f.clone();
            // Reference IN: nothing before the mic's first block ever reaches the adapter.
            if let Some(pad) = probe.static_pad("sink") {
                let ff = f.clone();
                let ss = set.clone();
                pad.add_probe(gst::PadProbeType::BUFFER, move |_pad, info| {
                    if let Some(gst::PadProbeData::Buffer(ref b)) = info.data {
                        let mic_first = ss.first_pts.load(Ordering::Relaxed);
                        if mic_first == 0 {
                            ff.pre_drops.fetch_add(1, Ordering::Relaxed);
                            return gst::PadProbeReturn::Drop;
                        }
                        if let Some(p) = b.pts() {
                            let end = p.nseconds() + (b.size() as u64 / 4) * 1_000_000_000 / 48_000;
                            let first = mic_first - 1;
                            let cap = ss.last_cap_pts.load(Ordering::Relaxed);
                            let mic_now = if cap > 0 { cap - 1 } else { first };
                            if ec_ref_is_wrapped(end, mic_now) && EC_REF_WRAPPED_GEN.swap(ss.gen, Ordering::Relaxed) != ss.gen {
                                eprintln!(
                                    "{}",
                                    ec_wrap_log_line(
                                        (end - first) / 86_400_000_000_000,
                                        !cfg!(windows) && ec_underflow_rebuild_enabled()
                                    )
                                );
                            }
                            // (a buffer more than 5 s away is another clock domain — pair as is)
                            if end <= first && first - end < 5_000_000_000 {
                                ff.old_drops.fetch_add(1, Ordering::Relaxed);
                                return gst::PadProbeReturn::Drop;
                            }
                        }
                    }
                    gst::PadProbeReturn::Ok
                });
            }
            // Reference THROUGH (after the probe copied it into its adapter): account for it.
            if let Some(pad) = probe.static_pad("src") {
                let ff = f.clone();
                let ss = set.clone();
                pad.add_probe(gst::PadProbeType::BUFFER, move |_pad, info| {
                    if let Some(gst::PadProbeData::Buffer(ref b)) = info.data {
                        let n = (b.size() / 4) as i64;
                        if n > 0 && ff.ref_buf.load(Ordering::Relaxed) == 0 {
                            ff.ref_buf.store(n, Ordering::Relaxed);
                        }
                        ff.backlog.fetch_add(n, Ordering::Relaxed);
                        if let Some(p) = b.pts() {
                            ff.ref_end_pts.store(p.nseconds() + (n as u64) * 1_000_000_000 / 48_000 + 1, Ordering::Relaxed);
                        }
                        ff.last_ref_ns.store(ss.t0.elapsed().as_nanos() as u64, Ordering::Relaxed);
                    }
                    gst::PadProbeReturn::Ok
                });
            }
            // Capture IN: wait for the reference when a read would starve, measure the pairing,
            // then drop the PTS so the probe reads in arrival order.
            if let Some(pad) = dsp.static_pad("sink") {
                let ff = f.clone();
                let ss = set.clone();
                pad.add_probe(gst::PadProbeType::BUFFER, move |_pad, info| {
                    if let Some(gst::PadProbeData::Buffer(ref mut b)) = info.data {
                        let pts = b.pts().map(|p| p.nseconds());
                        if let Some(p) = pts {
                            ss.last_cap_pts.store(p + 1, Ordering::Relaxed);
                        }
                        if k == 0 && ss.first_pts.load(Ordering::Relaxed) == 0 {
                            if let Some(p) = pts {
                                ss.first_pts.store(p + 1, Ordering::Relaxed);
                            }
                        }
                        if ff.backlog.load(Ordering::Relaxed) < EC_BLOCK {
                            let now = ss.t0.elapsed().as_nanos() as u64;
                            let last = ff.last_ref_ns.load(Ordering::Relaxed);
                            let primed = ff.primed.load(Ordering::Relaxed);
                            // never throttle the mic behind a reference that has stopped
                            if !primed || now.saturating_sub(last) < 250_000_000 {
                                let q_ms = (ff.ref_buf.load(Ordering::Relaxed).max(480) / 48) as u128;
                                let limit = if primed { (2 * q_ms + 10).max(60) } else { 250 };
                                let t = std::time::Instant::now();
                                while ff.backlog.load(Ordering::Relaxed) < EC_BLOCK && t.elapsed().as_millis() < limit {
                                    std::thread::sleep(std::time::Duration::from_millis(1));
                                }
                                ff.holds.fetch_add(1, Ordering::Relaxed);
                                ff.hold_ms.fetch_add(t.elapsed().as_millis() as u64, Ordering::Relaxed);
                            }
                        }
                        ff.primed.store(true, Ordering::Relaxed);
                        // pairing offset: this capture block's time minus the time of the reference
                        // block the probe will hand out (newest end − what is still queued)
                        if let Some(p) = pts {
                            let end = ff.ref_end_pts.load(Ordering::Relaxed);
                            if end > 0 {
                                let paired = (end - 1) as i64 - ff.backlog.load(Ordering::Relaxed) * 1_000_000_000 / 48_000;
                                let o = (p as i64 - paired) / 1_000_000;
                                ff.o_last.store(o, Ordering::Relaxed);
                                ff.o_min.fetch_min(o, Ordering::Relaxed);
                                ff.o_max.fetch_max(o, Ordering::Relaxed);
                            }
                        }
                        let bm = b.make_mut();
                        bm.set_pts(gst::ClockTime::NONE);
                        bm.set_dts(gst::ClockTime::NONE);
                    }
                    gst::PadProbeReturn::Ok
                });
            }
            // Capture OUT: one 10 ms read per output block; the last stage restores a
            // continuous timestamp for everything downstream (RNNoise, the feeder's gauges).
            if let Some(pad) = dsp.static_pad("src") {
                let ff = f.clone();
                let ss = set.clone();
                let last = k + 1 == n_stages;
                pad.add_probe(gst::PadProbeType::BUFFER, move |_pad, info| {
                    if let Some(gst::PadProbeData::Buffer(ref mut b)) = info.data {
                        let cur = ff.backlog.load(Ordering::Relaxed);
                        if cur >= EC_BLOCK {
                            ff.backlog.fetch_sub(EC_BLOCK, Ordering::Relaxed);
                        } else {
                            ff.silence_reads.fetch_add(1, Ordering::Relaxed);
                        }
                        ff.min_backlog.fetch_min(cur, Ordering::Relaxed);
                        ff.max_backlog.fetch_max(cur, Ordering::Relaxed);
                        if last {
                            let n = ss.blocks_out.fetch_add(1, Ordering::Relaxed);
                            let base = ss.first_pts.load(Ordering::Relaxed).saturating_sub(1);
                            let bm = b.make_mut();
                            bm.set_pts(gst::ClockTime::from_nseconds(base + n * 10_000_000));
                            bm.set_dts(gst::ClockTime::NONE);
                            bm.set_duration(gst::ClockTime::from_mseconds(10));
                        }
                    }
                    gst::PadProbeReturn::Ok
                });
            }
        }
        Some(set)
    }
    /// One window's summary, counters reset, per stage: `o=<pairing offset ms, last(min..max)>
    /// ref=<samples per reference buffer> backlog=cur(min..max)ms holds=N(Nms) silence=N
    /// dropped=<before the mic started>+<older than its first block>`. `o` is capture minus
    /// reference: 0 = paired at the same instant; negative = the reference leads (fine, AEC3
    /// buffers it); positive beyond the speaker→mic delay = the canceller cannot see the echo.
    pub(crate) fn report(&self) -> String {
        use std::sync::atomic::Ordering;
        let mut parts: Vec<String> = Vec::new();
        for f in &self.fifos {
            let cur = f.backlog.load(Ordering::Relaxed);
            let mn = f.min_backlog.swap(i64::MAX, Ordering::Relaxed);
            let mx = f.max_backlog.swap(i64::MIN, Ordering::Relaxed);
            let (mn, mx) = if mn == i64::MAX { (cur, cur) } else { (mn, mx) };
            let o = f.o_last.load(Ordering::Relaxed);
            let omn = f.o_min.swap(i64::MAX, Ordering::Relaxed);
            let omx = f.o_max.swap(i64::MIN, Ordering::Relaxed);
            let os = if o == i64::MIN {
                "n/a".to_string()
            } else if omn == i64::MAX {
                format!("{o}ms")
            } else {
                format!("{o}({omn}..{omx})ms")
            };
            parts.push(format!(
                "o={os} ref={} backlog={}({}..{})ms holds={}({}ms) silence={} dropped={}+{}",
                f.ref_buf.load(Ordering::Relaxed),
                cur / 48,
                mn / 48,
                mx / 48,
                f.holds.swap(0, Ordering::Relaxed),
                f.hold_ms.swap(0, Ordering::Relaxed),
                f.silence_reads.swap(0, Ordering::Relaxed),
                f.pre_drops.swap(0, Ordering::Relaxed),
                f.old_drops.swap(0, Ordering::Relaxed)
            ));
        }
        parts.join(" | ")
    }
}
/// The most recent gauge window of the live mic (ERLE dB, far-end fraction, when).
fn ec_gauge_last() -> &'static Mutex<Option<(f64, f64, f64, std::time::Instant)>> {
    static L: OnceLock<Mutex<Option<(f64, f64, f64, std::time::Instant)>>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(None))
}
/// One human line on what echo cancellation is doing right now, for the settings pane
/// ("" = nothing worth saying: off, legacy tier, no call).
pub fn aec_status_note() -> String {
    if !crate::aec::enabled() || aec_tier() != AecTier::Dsp {
        return String::new();
    }
    let skip = dsp_skip_reason().lock().unwrap().clone();
    if !skip.is_empty() {
        return format!("not needed — {skip}");
    }
    if !dsp_aec_active() {
        return String::new();
    }
    let flags = mic_chain_flags().load(std::sync::atomic::Ordering::Relaxed);
    if flags & 2 == 0 {
        return if aec_degraded() {
            "switched on but NOT running — the mic is up without it, so others may hear their own voice back; the app retries".to_string()
        } else {
            String::new()
        };
    }
    // 0.8.33: say when the reference is the user's own output list (summed).
    let n_ref = dsp_tap().lock().unwrap().as_deref().map(|b| b.split('\n').count()).unwrap_or(1);
    let refs = if n_ref > 1 { format!(" · listening to {n_ref} speaker outputs at once") } else { String::new() };
    match *ec_gauge_last().lock().unwrap() {
        Some((erle, peak, fe, at)) if at.elapsed() < std::time::Duration::from_secs(90) => {
            if fe >= 0.1 && !peak.is_nan() {
                format!("running — removing up to {peak:.0} dB of what your speakers play from your mic (last 30 s; {erle:.0} dB averaged over everything, talking included){refs}")
            } else if fe >= 0.1 {
                format!("running — {erle:.0} dB removed while others talked (no far-end-only second yet to measure the peak){refs}")
            } else {
                format!("running — it measures itself as soon as someone else talks{refs}")
            }
        }
        _ => format!("running{refs}"),
    }
}

fn start_mic_capture(
    source: NativeAudioSource,
) -> Result<(gst::Pipeline, tokio::task::JoinHandle<()>)> {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicU64, Ordering};

    const FRAME: usize = (SAMPLE_RATE / 100) as usize; // 480 samples == 10ms @ 48k mono
    const TARGET: usize = 4; // 40ms jitter setpoint (OUR cushion; libwebrtc has no ring now)
    const HIGH: usize = 8; // 80ms — above this, drop one QUIET frame to recover latency
    const CAP: usize = 20; // 200ms hard ceiling — UNGATED front-leak so latency can't run away
    const GATE_RMS: f32 = 0.02; // silence gate (~-34 dBFS): floor ~0.0003, speech > 0.05

    let (pipeline, appsink) = build_mic_pipeline()?;
    // 0.8.28: which rung this chain is (so a never-primed stall can block the right one).
    let chain_flags = mic_chain_flags().load(Ordering::Relaxed);
    let chain_had_dsp = chain_flags & 1 != 0;
    let chain_had_probe = chain_flags & 2 != 0;
    // 10.5 (audio review #2b): this capture gets its id now but becomes the CURRENT one only at the
    // commit point at the end, once it is PLAYING. It used to bump the generation — and clear the
    // health flags — right here, before anyone knew it would start: a candidate that failed left the
    // SURVIVING capture non-current, its stall reports disqualified for the rest of the call, and its
    // raised unhealthy flag wiped. The clean bill of health (a flag raised by a PREVIOUS capture
    // describes hardware that no longer exists) is issued at the commit instead.
    let my_gen = mic_gen_alloc().fetch_add(1, Ordering::Relaxed) + 1;
    let chain_label = mic_chain_label().lock().unwrap().clone();
    // 0.8.29 (Tier 0 #0.2): a bus watch on the mic pipeline. It never had one — a
    // start failure's debug string (the gstwebrtcdsp.cpp file:line, the PipeWire error
    // text) was thrown away every time, and CLOCK_LOST / LATENCY posts went unseen. Sync
    // handler: cheap, no allocation beyond the (rare) message itself.
    if let Some(bus) = pipeline.bus() {
        bus.set_sync_handler(|_, msg| {
            use gst::MessageView;
            let who = msg.src().map(|s| s.path_string().to_string()).unwrap_or_default();
            match msg.view() {
                MessageView::Error(e) => {
                    eprintln!(
                        "[micbus] ERROR from {who}: {} ({})",
                        e.error(),
                        e.debug().map(|d| d.to_string()).unwrap_or_default()
                    );
                    // 0.8.31: the echo reference branch died (output unplugged / exclusive grab).
                    // The mic keeps flowing — uncancelled — so say so and let the healer's
                    // one-shot retry rebuild against whatever output is there now.
                    if who.contains("tinhatec") {
                        eprintln!("[aec] the echo reference stopped mid-call — mic continues WITHOUT echo cancellation until the retry");
                        aec_degraded_flag().store(true, std::sync::atomic::Ordering::Relaxed);
                        aec_fallback_flag().store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                }
                MessageView::Warning(w) => eprintln!(
                    "[micbus] warning from {who}: {} ({})",
                    w.error(),
                    w.debug().map(|d| d.to_string()).unwrap_or_default()
                ),
                MessageView::ClockLost(_) => eprintln!("[micbus] clock lost (from {who})"),
                MessageView::Latency(_) => eprintln!("[micbus] latency changed (from {who})"),
                MessageView::Eos(_) => eprintln!("[micbus] EOS from {who}"),
                _ => {}
            }
            gst::BusSyncReply::Drop
        });
    }
    // audiorate's own correction counters (samples it ADDED to fill timestamp gaps /
    // DROPPED on overlaps). Nonzero-and-climbing here while the far end reports garbled
    // audio = the capture graph's timestamps are broken (cold PipeWire node), NOT the
    // feeder or E2EE — the decisive discriminator for join-time static reports.
    let micrate = pipeline.by_name("micrate");
    // 0.8.31: the echo-cancellation gauge (None when the chain has no probe).
    let ec_gauge = if chain_had_probe { EcGauge::attach(&pipeline) } else { None };
    // 9.2.1: the reference is read in arrival order (see EcFifoSet).
    let ec_fifo = if chain_had_probe { EcFifoSet::attach(&pipeline) } else { None };
    // 0.8.29 (Tier 0 #0.5): what the SOURCE delivers, measured on audiorate's sink
    // pad (post convert/resample, so units are 48 k mono samples): buffers, samples, PTS
    // advance and DISCONT flags. This is the probe that settles "which element loses the
    // samples" — the 75.3/60.0/50.2 buf/s plateaus were only ever inferable downstream.
    // Atomics only: this runs on the streaming thread.
    let src_stats = std::sync::Arc::new([
        AtomicU64::new(0), // buffers
        AtomicU64::new(0), // samples
        AtomicU64::new(0), // discont flags
        AtomicU64::new(0), // last pts (ns)
        AtomicU64::new(0), // pts advance (ns)
    ]);
    if let Some(pad) = micrate.as_ref().and_then(|el| el.static_pad("sink")) {
        let st = src_stats.clone();
        pad.add_probe(gst::PadProbeType::BUFFER, move |_pad, info| {
            if let Some(gst::PadProbeData::Buffer(ref b)) = info.data {
                st[0].fetch_add(1, Ordering::Relaxed);
                // 10.5: the RAW capture (F32 48 k mono, before any processing) for the mic check
                // in hear.rs — "is this device producing anything at all?" — current capture only.
                if mic_gen().load(Ordering::Relaxed) == my_gen {
                    if let Ok(m) = b.map_readable() {
                        let f: Vec<f32> = m
                            .as_slice()
                            .chunks_exact(4)
                            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                            .collect();
                        crate::hear::note_raw(&f);
                        crate::sameroom::note_raw(&f); // 11.3: the room-mic election's level
                    }
                }
                st[1].fetch_add((b.size() / 4) as u64, Ordering::Relaxed);
                if b.flags().contains(gst::BufferFlags::DISCONT) {
                    st[2].fetch_add(1, Ordering::Relaxed);
                }
                if let Some(pts) = b.pts() {
                    let p = pts.nseconds();
                    let prev = st[3].swap(p, Ordering::Relaxed);
                    if prev != 0 && p > prev {
                        st[4].fetch_add(p - prev, Ordering::Relaxed);
                    }
                }
            }
            gst::PadProbeReturn::Ok
        });
    }
    let sink_samples = std::sync::Arc::new(AtomicU64::new(0));
    let sink_samples_cb = sink_samples.clone();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Vec<i16>>();
    // Retained lock-free count of samples the in-app peak limiter ENGAGED on (|x| above the
    // CEIL ceiling, i.e. a transient it scaled down before the narrow), read by the 30s health
    // log and printed as clip=. It climbs while the user talks loudly and idles in silence —
    // a live limiter-engagement gauge, not an error count. Atomic add on the streaming thread.
    let clipped = std::sync::Arc::new(AtomicU64::new(0));
    let clipped_cb = clipped.clone();
    // In-app peak-limiter STATE, moved into the appsink callback so it PERSISTS across buffers
    // (a peak's release tail must carry over the buffer seam, exactly like the gst stream is
    // continuous). envelope follower + smoothed gain — see limit_sample().
    let mut limiter_env = 0.0f32;
    let mut limiter_g = 1.0f32;

    // Appsink callback: GStreamer STREAMING THREAD. Stays cheap — NO file I/O. (r114's HIGHPEAK /
    // [mic] / [mic-diag] eprintln on THIS thread stalled it and made the leaky 200ms queue drop
    // buffers, manufacturing the very underruns we are fixing — all removed.)
    appsink.set_callbacks(
        gstreamer_app::AppSinkCallbacks::builder()
            .new_sample(move |sink| {
                let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                let buffer = sample.buffer().ok_or(gst::FlowError::Error)?;
                let map = buffer.map_readable().map_err(|_| gst::FlowError::Error)?;
                // F32LE -> peak-limit -> i16. The pipeline hands the appsink FLOAT so transient
                // overshoots arrive INTACT (gst no longer narrows to S16 in-pipeline, which
                // hard-saturated any >=0 dBFS peak into the flat-topped clusters that ARE the
                // universal native-capture pop — proven by a controlled source/receiver matrix).
                // limit_sample() is a peak limiter (smoothed 0.5ms attack so a sharp transient
                // can't produce a gain STEP/click, + cubic soft_clip safety) bounding |out| just
                // under FS — the in-app equivalent of the APM output limiter every other client
                // has; for steady/normal signal it is pure scaling => ~0 THD (the r116 memoryless
                // soft-clip added 10-30% THD = "way worse"; r117's instant attack still step-
                // clicked on word-final plosives; r118 ramps the attack). State (limiter_env/
                // limiter_g) persists across buffers. Then WE narrow to i16.
                let mut shaped = 0u64;
                let samples: Vec<i16> = map
                    .as_slice()
                    .chunks_exact(4)
                    .map(|b| {
                        let x = f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
                        // limit_sample returns |y|<=CEIL (0.97), so this never saturates i16.
                        (limit_sample(x, &mut limiter_env, &mut limiter_g, &mut shaped) * 32767.0)
                            .round() as i16
                    })
                    .collect();
                if shaped != 0 {
                    clipped_cb.fetch_add(shaped, Ordering::Relaxed);
                }
                sink_samples_cb.fetch_add(samples.len() as u64, Ordering::Relaxed);
                set_mic_level(rms_of(&samples)); // #B self-view waveform meter (cheap, no I/O)
                let _ = tx.send(samples);
                Ok(gst::FlowSuccess::Ok)
            })
            .build(),
    );

    // Real-time-paced feeder: tokio WORKER thread (multi-thread runtime), NOT the streaming thread.
    // queue_size_ms=0 => libwebrtc has no ring / no RepeatingTask / no hard-zero splice; this
    // monotonic 10ms interval is the sole pacing clock, locked to the same system clock the
    // far-end NetEq/RTP run on (0 ppm by construction). A silence-gated controller pays back the
    // slow-USB-crystal deficit inaudibly during conversational gaps.
    let task = tokio::spawn(async move {
        let mut jitter: VecDeque<i16> = VecDeque::with_capacity((CAP + 2) * FRAME);
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(10));
        // Burst: after any worker stall, catch up missed ticks so the LONG-RUN feed rate is
        // EXACTLY 100 frames/wall-sec (the invariant the far end + the [mic-rt] log rely on).
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Burst);
        let mut primed = false; // hold off feeding until a TARGET cushion exists
        let mut primed_published = false; // mic_primed_flag set by THIS feeder (see the commit point)
        let mut level_ema: f32 = 0.0; // lagged silence-gate history
        let mut last_frame = vec![0i16; FRAME]; // concealment source (refreshed from REAL audio)
        let mut fed: u64 = 0; // real frames popped + fed
        let mut injected: u64 = 0; // synthetic frames (deficit payback + underflow conceal)
        let mut dropped: u64 = 0; // frames discarded (quiet catch-up + ceiling leak)
        let mut underruns: u64 = 0; // ticks the jitter was fully empty at feed time
        let mut pushed: u64 = 0; // frames received from the appsink (device delivery)
        let mut last_log = std::time::Instant::now();
        let mut first_log = true; // first [mic-rt] at +5s (join-window evidence), then every 30s
        let started = std::time::Instant::now(); // capture-start health window anchor
        let mut health_ticks: u64 = 0;
        let mut last_push = std::time::Instant::now(); // WALL-CLOCK time of last delivery
        let mut stable_ticks: u32 = 0; // push ticks seen without a >100ms delivery gap
        let mut last_pushed_seen: u64 = 0;

        // ── Skew servo: hold fill at TARGET by re-timing consumption ±MAX_SKEW, so
        // capture-clock drift is paid back CONTINUOUSLY and inaudibly instead of by
        // whole-frame dup/drop splices (which needed silence the AGC floor never gave).
        const MAX_SKEW: f64 = 0.002; // ±2000 ppm — far beyond any real crystal error
        const SKEW_KP: f64 = 0.0004; // ratio offset per frame of fill error
        const SKEW_KI: f64 = 0.000_003; // integral gain per frame-error per tick
        let mut resampler = SkewResampler::new();
        let mut skew_i: f64 = 0.0; // integral term (anti-windup clamped)
        #[allow(unused_assignments)] // the servo re-computes this every tick before use
        let mut ratio: f64 = 1.0; // last applied ratio (also logged as skew ppm)
        // 0.8.29 (Tier 0 #0.6): servo SATURATION alarm. A 25 % delivery deficit is
        // 250,000 ppm against a ±2000 ppm clamp — 125× — so the servo pinned at its clamp
        // for a full second is categorically NOT clock drift; it is the capture being
        // short. Would have fired within ~2 s in every damaged capture in the corpus.
        let mut sat_ticks: u32 = 0;
        let mut sat_logged = false;
        // Only the CURRENT capture's feeder may raise health flags (see mic_gen()).
        let current = move || mic_gen().load(Ordering::Relaxed) == my_gen;
        let raise_unhealthy = move || current() && raise_mic_unhealthy();
        // Per-window snapshots of the source-side counters (the probe is cumulative).
        let mut src_base = [0u64; 5];
        let mut sink_base: u64 = 0;
        // ── Seam/concealment state ──
        let mut prev_last: i16 = 0; // final sample of the previous emitted frame
        let mut consecutive_injects: u32 = 0; // concealment streak (drives decay)
        let mut seam_pending = false; // next real frame must be seam-smoothed
        let mut fade_in_pending = false; // first frame after prime ramps 0→1
        // (10.5: device health is judged on the RAW capture in hear.rs, not here — see note_raw.)
        // ── Mid-call stall detector (the "his mic went dead / last syllable buzz"
        // class): a primed capture that stops delivering re-raises the unhealthy flag
        // so the app-side lifetime healer rebuilds it — the old watch died at +8s.
        let mut stall_logged = false;
        let mut last_lim: u64 = 0; // limiter-engagement count at the previous [mic-rt]
        // ── Mid-call degradation watch (rolling 10s window, runs for the WHOLE call) ──
        let mut win_ticks: u64 = 0; // ticks accumulated in the current window
        let mut win_injected: u64 = 0; // concealment frames emitted in the current window
        let mut win_rate_base: u64 = 0; // audiorate add+drop at the window boundary
        let mut bad_windows: u32 = 0; // consecutive damaged windows (2 ⇒ flag)
        let mut degraded_logged = false; // one [mic-degraded] line per fault episode

        loop {
            interval.tick().await;
            // Early-window capture-health watch (~1/s for the first 8s only). Two detectors:
            //  (a) audiorate corrections — a GROSS timeline break (>40ms per-buffer jitter
            //      sustained past audiobuffersplit's stream-align window).
            //  (b) feeder-visible damage — audiobuffersplit (GstAudioStreamAlign, 40ms
            //      alignment-threshold / 1s discont-wait defaults) RE-TIMESTAMPS sub-threshold
            //      cold-start jitter into a clean timeline, so a garbling capture can hold
            //      micrate at 0 forever. That blind spot is why the r129 heal never fired
            //      while users could cure the static by toggling RNNoise (same rebuild). The
            //      feeder experiences that damage directly: post-prime underruns/duplicate-
            //      frame concealment during the first seconds ARE the audible garble.
            // Flag once; the app's post-join self-heal rebuilds the capture.
            health_ticks += 1;
            if health_ticks % 100 == 0 && started.elapsed().as_secs() < 8 {
                let corrected = micrate
                    .as_ref()
                    .map(|el| el.property::<u64>("add") + el.property::<u64>("drop"))
                    .unwrap_or(0);
                let feeder_damage = primed && (underruns > 20 || (injected > 100 && pushed > 0));
                if (corrected > 4800 || feeder_damage)
                    && raise_unhealthy()
                {
                    eprintln!(
                        "[mic-heal] capture unhealthy within {:.1}s of start \
                         (rate_corrected={corrected} under={underruns} inj={injected} \
                         pushed={pushed}) — flagging for rebuild",
                        started.elapsed().as_secs_f32()
                    );
                }
            }
            // Mid-call capture DECAY — the "his mic starts popping ~an hour in, and leaving
            // the call and coming back fixes it" class. Both detectors around this one are
            // blind to it: the join watch above is time-boxed to the first 8s, and the stall
            // detector below needs a FULL 2s with no delivery at all. A capture that rots
            // into chronic sub-second holes therefore ran damaged until the user rebuilt it
            // BY HAND (rejoin == restart_mic), which is exactly what peers were reporting.
            //
            // Field evidence (a long call, a USB audio interface):
            // a long clean stretch (inj=0 drop=0 rate_add=0), then within about a minute inj
            // grew by two orders of magnitude per 3000 ticks while audiorate's repair counter
            // ran into the hundreds of thousands of samples. About a QUARTER of the outgoing
            // audio had become repeat-frame concealment splices —
            // that IS the popping. Nothing flagged; the capture never recovered on its own.
            //
            // So: evaluate the same feeder-visible damage the join watch uses, but on a
            // rolling 10s window for the whole call life. Two CONSECUTIVE damaged windows
            // are required so a one-off system hiccup (suspend/resume, CPU spike) can't
            // spend a rebuild — a real decay escalates and trips within ~20-30s. The flag is
            // all we own; the app-side lifetime healer (2s poll, 5s→5min backoff, resets
            // after 2 minutes clean) does the rebuild.
            if primed && started.elapsed() >= std::time::Duration::from_secs(8) {
                if win_ticks == 0 {
                    // Re-seed at every window START, not from 0: audiorate's counters are
                    // CUMULATIVE, so a capture that took gross corrections during the join
                    // window would otherwise charge them to this window forever after.
                    win_rate_base = micrate
                        .as_ref()
                        .map(|el| el.property::<u64>("add") + el.property::<u64>("drop"))
                        .unwrap_or(0);
                }
                win_ticks += 1;
                if win_ticks >= 1000 {
                    let rate_now = micrate
                        .as_ref()
                        .map(|el| el.property::<u64>("add") + el.property::<u64>("drop"))
                        .unwrap_or(0);
                    let rate_delta = rate_now.saturating_sub(win_rate_base);
                    // 1% of ticks concealed, or 100ms of audiorate timeline repair, per 10s.
                    // A healthy capture holds BOTH at exactly 0 indefinitely (40 unbroken
                    // minutes of it in the report above), so the margin here is the entire
                    // scale; 4800 samples is the same bar the join-window watch uses.
                    if win_injected >= 10 || rate_delta >= 4800 {
                        bad_windows += 1;
                        if bad_windows >= 2
                            && raise_unhealthy()
                            && !degraded_logged
                        {
                            degraded_logged = true;
                            eprintln!(
                                "[mic-degraded] capture decayed {:.0}s into this capture \
                                 (inj={win_injected}/1000 ticks, audiorate={rate_delta} \
                                 samples/10s) — flagging for rebuild",
                                started.elapsed().as_secs_f32()
                            );
                        }
                    } else {
                        bad_windows = 0;
                        degraded_logged = false; // a later, unrelated episode logs again
                    }
                    win_ticks = 0; // next tick re-seeds win_rate_base
                    win_injected = 0;
                }
            }
            // 1) Drain everything the appsink produced (non-blocking; the timer drives cadence).
            //    A discont/EOS short final buffer is repacked by the VecDeque into exact 480-frames,
            //    so no audio is dropped and the fast-path 480 invariant still holds.
            loop {
                match rx.try_recv() {
                    Ok(s) => {
                        pushed += 1;
                        jitter.extend(s);
                    }
                    Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                    Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                        // The capture side is GONE (pipeline dropped ⇒ appsink callback
                        // ⇒ tx dropped). Exit instead of ticking forever: a leaked
                        // feeder's stall watch would otherwise re-latch the unhealthy
                        // flag every 10ms for the process lifetime and drive the join
                        // healer to tear down every subsequent healthy capture.
                        eprintln!("[mic-rt] capture channel closed — feeder exiting");
                        return;
                    }
                }
            }
            // Delivery-cadence tracker (drives the priming gate). WALL-CLOCK based: after
            // a worker stall, MissedTickBehavior::Burst replays the missed ticks within
            // microseconds — the first catch-up tick drains every queued push and the
            // rest see "no delivery", which a tick-counted gap misreads as a device gap
            // (spuriously resetting the stability streak at exactly the moment joins
            // stall: connect, AEC settle, pipeline builds). Real time can't be fooled.
            let pushed_this_tick = pushed > last_pushed_seen;
            if pushed_this_tick {
                last_push = std::time::Instant::now();
                stall_logged = false;
            }
            last_pushed_seen = pushed;
            // Mid-call stall: a PRIMED capture that stops delivering for 2s is broken
            // hardware/graph state (mute keeps capturing, so this is never a mute), and
            // a never-primed capture that has delivered nothing by +10s is a dead start.
            // Raise the unhealthy flag EVERY time it is observed low during a stall —
            // the app-side lifetime healer consumes it and rebuilds the capture, which
            // is exactly what users discovered the NC toggles do by hand.
            let stalled = if primed {
                last_push.elapsed() > std::time::Duration::from_secs(2)
            } else {
                // 0.8.28: 4 s, not 10 — a real device delivers its first buffer well within a
                // second (the cadence fallback primes at 3 s); every extra second here was a
                // second of dead mic at every join on the machines where the stage stalls.
                // 10.5 (audio review #2a): "delivered nothing" was `pushed == 0` — a capture that
                // delivered 10–30 ms and then stopped could neither prime (that takes 40 ms) nor
                // stall, a permanently silent mic the healer believed was starting. Unprimed past
                // 4 s with no delivery for 2 s is a dead start either way.
                started.elapsed() > std::time::Duration::from_secs(4)
                    && last_push.elapsed() > std::time::Duration::from_secs(2)
            };
            if stalled {
                // 0.8.29: a chain that never primed is a dead chain, not a hiccup —
                // tell the healer so it rebuilds NOW instead of waiting out its backoff.
                if !primed && current() {
                    mic_never_primed_flag().store(true, Ordering::Relaxed);
                }
                if raise_unhealthy() && !stall_logged {
                    stall_logged = true;
                    eprintln!(
                        "[mic-stall] no capture delivery for {:.1}s (primed={primed}) — flagging for rebuild",
                        last_push.elapsed().as_secs_f32()
                    );
                    // A chain that never delivered a single buffer: blame its rung so the
                    // rebuild steps down instead of rebuilding the same dead chain.
                    if !primed && chain_had_dsp {
                        dsp_stage_step_down(chain_had_probe);
                    }
                }
            }
            // 2) Hard ceiling: UNGATED front-leak so latency can NEVER run away (fast-clock device
            //    or post-stall backlog) even during continuous speech.
            while jitter.len() > CAP * FRAME {
                let _ = jitter.drain(..FRAME);
                dropped += 1;
                seam_pending = true; // the splice point gets smoothed on the next emit
            }

            let mut fill = jitter.len();
            let mut fill_frames = fill / FRAME;

            // 3) Pre-roll: idle until a TARGET cushion exists AND delivery has been gapless
            //    for ~50 push ticks. A cold node — the first PipeWire start, or a freshly-
            //    created EC source whose masters are still resuming from suspend — delivers
            //    erratically for its first moments; audio buffered during that window IS the
            //    join-time garble, and the old fill-only gate happily fed it to peers.
            //    Waiting for a stable cadence and then keeping only the newest TARGET frames
            //    means the first audio peers hear is post-warm-up. (Bursty-but-healthy
            //    delivery still primes: the streak only resets on a real >100ms wall-clock
            //    gap.) A pipeline that never delivers (no mic) just idles here as before.
            if primed && !primed_published && current() {
                // Primed before the commit made it current (Astra 2026-09-22 #13): the commit cleared the
                // global flag, and the branch below never runs again for this feeder.
                mic_primed_flag().store(true, Ordering::Relaxed);
                primed_published = true;
            }
            if !primed {
                if last_push.elapsed() > std::time::Duration::from_millis(100) {
                    stable_ticks = 0; // real >100ms delivery gap: cold node still glitching
                } else if pushed_this_tick {
                    stable_ticks = stable_ticks.saturating_add(1);
                }
                let stable = stable_ticks >= 50;
                // Cadence FALLBACK: a healthy device whose burst period exceeds the 100ms
                // gap bar (large-quantum PipeWire configs, some Bluetooth paths) can never
                // build a 50-tick streak — without this it would sit unprimed FOREVER, a
                // permanently silent mic with no diagnostic. After 3s with a full cushion
                // and real deliveries, prime anyway; the warm-up discard still applies.
                let fallback = started.elapsed() >= std::time::Duration::from_secs(3) && pushed > 0;
                if fill_frames >= TARGET && (stable || fallback) {
                    primed = true;
                    if current() {
                        mic_primed_flag().store(true, Ordering::Relaxed);
                        primed_published = true;
                    }
                    // the source-side counters ran during warm-up too: window them from here
                    src_base = std::array::from_fn(|i| src_stats[i].load(Ordering::Relaxed));
                    sink_base = sink_samples.load(Ordering::Relaxed);
                    // Pre-stability audio is suspect — discard all but the newest cushion.
                    // (Deliberately not counted in `dropped`: this is by-design warm-up
                    // discard, not a drift correction.)
                    while jitter.len() > TARGET * FRAME {
                        let _ = jitter.drain(..FRAME);
                    }
                    // Refresh the snapshot so THIS tick's branch selection sees the trimmed
                    // queue (the stale pre-trim count read as "backlog", triggering an
                    // immediate catch-up drop out of the fresh cushion).
                    fill = jitter.len();
                    fill_frames = fill / FRAME;
                    // Anchor the first [mic-rt] window at prime, not task start — feedrate
                    // is defined over FED time; counting the idle pre-prime seconds printed
                    // 40-90/s against the line's own "(100.00=locked)" annotation.
                    last_log = std::time::Instant::now();
                    fade_in_pending = true; // ramp the stream onset (no mid-waveform step)
                    eprintln!(
                        "[mic] primed after {:.2}s ({})",
                        started.elapsed().as_secs_f32(),
                        if stable { "stable delivery" } else { "cadence fallback" }
                    );
                    pw_snapshot("mic primed");
                } else {
                    continue;
                }
            }

            // Fast-attack gate for the remaining GROSS correction (whole-frame drop at
            // high backlog): both the imminent frame's energy AND the lagged EMA must be
            // quiet. Ordinary drift never reaches this branch anymore — the skew servo
            // below pays it back continuously, speech or not.
            let front_quiet = if fill >= FRAME {
                let e: f64 = jitter.iter().take(FRAME).map(|&s| (s as f64) * (s as f64)).sum();
                ((e / FRAME as f64).sqrt() / i16::MAX as f64) < GATE_RMS as f64
            } else {
                true
            };
            let quiet = level_ema < GATE_RMS && front_quiet;

            // Skew servo update: consume slightly faster/slower than realtime so fill
            // converges to TARGET without ever splicing. Clamped to ±2000 ppm (inaudible;
            // real crystals are well under that). Integral handles the steady drift, the
            // proportional term the excursions.
            {
                let err = fill as f64 / FRAME as f64 - TARGET as f64;
                skew_i = (skew_i + SKEW_KI * err).clamp(-MAX_SKEW, MAX_SKEW);
                ratio = 1.0 + (SKEW_KP * err + skew_i).clamp(-MAX_SKEW, MAX_SKEW);
            }
            if primed && (ratio - 1.0).abs() >= MAX_SKEW - 1e-9 {
                sat_ticks += 1;
                if sat_ticks >= 100 && !sat_logged {
                    sat_logged = true;
                    eprintln!(
                        "[mic-servo] pinned at {:+.0} ppm for 1 s — the capture is short by far more \
                         than any clock drift (a delivery deficit, not drift) — flagging",
                        (ratio - 1.0) * 1e6
                    );
                    let _ = raise_unhealthy();
                }
            } else {
                sat_ticks = 0;
            }

            // 4) Choose this tick's EXACTLY-480 frame (every branch outputs exactly one frame).
            let need = resampler.need(FRAME, ratio);
            let (mut frame, real): (Vec<i16>, bool) = if fill < need {
                // UNDERFLOW: conceal with the last real frame — NEVER a hard-zero click.
                // 0.8.29: repeat the last real frame ONCE, then fade it -6 dB per
                // frame (inaudible after ~60 ms). The old rule decayed only from the THIRD
                // consecutive repeat, which at a steady 25 % delivery deficit (3 real : 1
                // repeat) never happened — every fourth 10 ms of speech was a verbatim
                // repeat of the previous 10 ms, a 25 Hz amplitude comb laid over the voice,
                // inaudible while quiet and "instant static / robotic" the moment the user
                // spoke. At total collapse the same code decayed to zero and read as "muted
                // while talking". One fault, two symptoms; the decay threshold was the switch.
                injected += 1;
                win_injected += 1; // rolling-window copy (the 30s log counters reset)
                if fill == 0 {
                    underruns += 1;
                }
                consecutive_injects += 1;
                if consecutive_injects >= 2 {
                    for s in last_frame.iter_mut() {
                        *s = (*s as f32 * 0.5) as i16;
                    }
                }
                seam_pending = true; // the frame after this one restarts the real stream
                (last_frame.clone(), false)
            } else {
                if quiet && fill_frames > HIGH {
                    // Gross backlog while quiet: drop a whole frame — instant, inaudible
                    // in a gap, and faster than the servo for post-stall recovery.
                    let _ = jitter.drain(..FRAME);
                    dropped += 1;
                    seam_pending = true;
                }
                let mut buf = vec![0i16; FRAME];
                let consumed = resampler.resample(&jitter, ratio, &mut buf);
                let _ = jitter.drain(..consumed);
                consecutive_injects = 0;
                (buf, true)
            };

            // Seam handling: smooth the step where this frame doesn't continue the
            // previous one (conceal entry/exit, drop splices), and ramp the very first
            // post-prime frame in from silence.
            if fade_in_pending && real {
                fade_in_pending = false;
                let len = frame.len() as f32;
                for (n, s) in frame.iter_mut().enumerate() {
                    *s = (*s as f32 * (n as f32 / len)) as i16;
                }
                seam_pending = false;
            } else if seam_pending {
                smooth_seam(&mut frame, prev_last);
                if real {
                    seam_pending = false; // real stream resumed; conceal frames re-arm above
                }
            }
            prev_last = *frame.last().unwrap_or(&0);

            if real {
                fed += 1;
                last_frame.copy_from_slice(&frame); // conceal from REAL audio (repeat speech, not silence)
                let r = rms_of(&frame);
                level_ema = level_ema * 0.9 + r * 0.1;
                // (0.8.25's "digital silence for 10 s" rule lived here and judged THIS audio — after
                // noise suppression and the canceller — so every pause of a gated mic tripped it:
                // 373 times in one field call. 10.5 judges the raw capture in hear.rs.)
            } else {
                // Injected frames must not poison the gate history: a loud looped frame
                // held `quiet` false forever, locking out the very corrections that would
                // have recovered the cushion. Decay toward silence instead.
                level_ema *= 0.9;
            }

            // 10.5: what the call gets, for "talking while muted", "not coming through" and the
            // echo checks (hear.rs) — the current capture only.
            if current() {
                crate::hear::note_sent(&frame);
                crate::sameroom::note_sent_rms(rms_of(&frame)); // 11.3: fallback level when the raw probe is silent
            }
            // 11.3 same room: a linked peer holds the room mic → the encoder gets silence (the track stays
            // published; no mute icon flaps on anyone's tile).
            let frame = if crate::sameroom::tx_live() { frame } else { vec![0i16; frame.len()] };
            // 5) Feed libwebrtc. queue_size_ms=0 => OnData runs synchronously here; the await
            //    returns immediately (no back-pressure, no rust-sdks #408/#420 hang).
            let af = AudioFrame {
                data: Cow::Owned(frame),
                sample_rate: SAMPLE_RATE,
                num_channels: 1,
                samples_per_channel: FRAME as u32,
            };
            // Test gate (TINHAT_TEST_TX_DROP_AFTER_S=N): stop handing frames to the encoder N s after
            // start — the send path dying mid-call, for the "not coming through" E2E (10.5).
            if !tx_test_dropping() {
                let _ = source.capture_frame(&af).await;
            }

            // 6) Retained verification hook: ONE line / 30s, OFF the streaming thread. Every tick
            //    outputs exactly one frame => fed+injected == ticks => feedrate pins to 100.00/s.
            //    The FIRST line lands at +5s so a short join-static repro carries evidence
            //    (the old +30s first line outlived most repros).
            if last_log.elapsed().as_secs() >= if first_log { 5 } else { 30 } {
                let secs = last_log.elapsed().as_secs_f64().max(0.001);
                let (rate_add, rate_drop) = micrate
                    .as_ref()
                    .map(|el| (el.property::<u64>("add"), el.property::<u64>("drop")))
                    .unwrap_or((0, 0));
                // lim= is the PER-WINDOW count of samples the peak limiter engaged on
                // (was the cumulative-forever clip=, which read like runaway clipping);
                // skew= is the servo's current re-timing in ppm — the field evidence of
                // how fast this machine's capture clock actually drifts.
                let lim_now = clipped.load(Ordering::Relaxed);
                // 0.8.29: source-side truth per window — srcbuf/srcsamp = what the device
                // path delivered to audiorate (per second), srcpts = how far its timestamps
                // advanced (per second; > srcsamp means the source SKIPPED time), disc =
                // DISCONT flags, sinksamp = samples that reached the appsink. On a healthy
                // capture srcsamp ≈ srcpts ≈ sinksamp ≈ 48000; a deficit shows srcsamp <
                // 48000 with rate_add making up the difference; sinksamp < srcsamp blames an
                // element in between.
                let src_now: [u64; 5] = std::array::from_fn(|i| src_stats[i].load(Ordering::Relaxed));
                let sink_now = sink_samples.load(Ordering::Relaxed);
                let per_s = |now: u64, base: u64| (now.saturating_sub(base)) as f64 / secs;
                eprintln!(
                    "[mic-rt] fed={fed} inj={injected} drop={dropped} under={underruns} \
                     pushed={pushed} fill={}fr ema={level_ema:.4} lim={} \
                     skew={:+.0}ppm rate_add={rate_add} rate_drop={rate_drop} \
                     feedrate={:.2}/s (100.00=locked to libwebrtc/NetEq clock) \
                     srcbuf={:.1}/s srcsamp={:.0}/s srcpts={:.0}/s disc={} sinksamp={:.0}/s \
                     chain={:?} ns={} ec={} erle={} fifo={}",
                    jitter.len() / FRAME, // the cushion actually HELD (post-consumption)
                    lim_now.saturating_sub(last_lim),
                    (ratio - 1.0) * 1e6,
                    (fed + injected) as f64 / secs,
                    per_s(src_now[0], src_base[0]),
                    per_s(src_now[1], src_base[1]),
                    per_s(src_now[4], src_base[4]) * 48000.0 / 1e9,
                    src_now[2].saturating_sub(src_base[2]),
                    per_s(sink_now, sink_base),
                    chain_label,
                    mic_ns_mode().as_str(),
                    if chain_had_probe { "on" } else { "off" },
                    ec_gauge
                        .as_ref()
                        .map(|g| {
                            let w = g.take_window();
                            *ec_gauge_last().lock().unwrap() = Some((w.erle_db, w.peak_db, w.fe_frac(), std::time::Instant::now()));
                            w.report()
                        })
                        .unwrap_or_else(|| "off".to_string()),
                    ec_fifo.as_ref().map(|f| f.report()).unwrap_or_else(|| "off".to_string()),
                );
                src_base = src_now;
                sink_base = sink_now;
                last_lim = lim_now;
                fed = 0;
                injected = 0;
                dropped = 0;
                underruns = 0;
                pushed = 0;
                last_pushed_seen = 0; // keep the delivery-gap tracker consistent with the reset
                first_log = false;
                last_log = std::time::Instant::now();
            }
        }
    });

    if !set_state_bounded(pipeline.upcast_ref(), gst::State::Playing, 8000, "mic capture start") {
        // ABORT THE FEEDER before surfacing the error: dropped un-aborted, the leaked
        // task's never-primed stall watch re-latched mic_capture_unhealthy every 10ms
        // FOREVER (it has no exit condition of its own), so the join healer tore down
        // each subsequent HEALTHY capture at every backoff expiry — for the rest of the
        // process lifetime, across calls (a field incident).
        // 10.5: and touch NO shared flag. This candidate never became current (see the commit
        // below), so its feeder could not latch anything — and the capture that IS current still
        // owns the flags (clearing its unhealthy flag here was audio review #2b).
        task.abort();
        set_state_bounded(pipeline.upcast_ref(), gst::State::Null, 1500, "mic capture");
        return Err(anyhow::anyhow!("mic pipeline start failed (the capture never started)"));
    }
    // 10.5 — COMMIT: the capture is PLAYING; from here it is the current one. A clean bill of
    // health (a flag raised by the previous capture describes hardware that no longer exists —
    // left set, the healer burned a rebuild on it), unprimed, and the mic check starts over for it.
    mic_capture_unhealthy_flag().store(false, Ordering::Relaxed);
    mic_never_primed_flag().store(false, Ordering::Relaxed);
    mic_primed_flag().store(false, Ordering::Relaxed);
    mic_gen().store(my_gen, Ordering::Relaxed);
    mic_down_flag().store(false, Ordering::Relaxed);
    crate::hear::capture_committed(&mic_target().lock().unwrap().clone().unwrap_or_default());
    Ok((pipeline, task))
}

/// TINHAT_TEST_TX_DROP_AFTER_S=N (tests only): true from N seconds after process start.
fn tx_test_dropping() -> bool {
    static AT: OnceLock<Option<std::time::Instant>> = OnceLock::new();
    AT.get_or_init(|| {
        std::env::var("TINHAT_TEST_TX_DROP_AFTER_S")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .map(|s| std::time::Instant::now() + std::time::Duration::from_secs(s))
    })
    .map(|t| {
        let now = std::time::Instant::now();
        static LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if now >= t && !LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
            eprintln!("[call] TEST: TINHAT_TEST_TX_DROP_AFTER_S reached — the mic stops sending now");
        }
        now >= t
    })
    .unwrap_or(false)
}

/// Persistent-phase linear resampler behind the feeder's skew servo: each call emits
/// EXACTLY `out.len()` samples while consuming `≈ out.len() * ratio` input samples, so a
/// sustained ratio offset of N ppm re-times the stream by N ppm — the continuous,
/// inaudible replacement for whole-frame dup/drop corrections (which were silence-gated,
/// and an AGC-boosted noise floor defeats the gate permanently: every correction then
/// landed MID-SPEECH as a hard splice — the robotic repeats and pops users heard).
struct SkewResampler {
    /// Fractional read position within the input queue (0 ≤ phase < 1 between calls).
    phase: f64,
}

impl SkewResampler {
    fn new() -> Self {
        Self { phase: 0.0 }
    }

    /// Input samples that must be queued for `resample(ratio)` to succeed.
    fn need(&self, out_len: usize, ratio: f64) -> usize {
        if ratio == 1.0 && self.phase == 0.0 {
            return out_len; // bit-exact fast path consumes exactly out_len
        }
        (self.phase + (out_len as f64) * ratio).ceil() as usize + 1
    }

    /// Fill `out` by linear interpolation at stride `ratio`; returns how many whole
    /// input samples were consumed (the caller drains them). `src` must hold at least
    /// `need(out.len(), ratio)` samples.
    fn resample(
        &mut self,
        src: &std::collections::VecDeque<i16>,
        ratio: f64,
        out: &mut [i16],
    ) -> usize {
        // Bit-exact fast path — the servo snaps here whenever fill sits on target.
        if ratio == 1.0 && self.phase == 0.0 {
            for (n, o) in out.iter_mut().enumerate() {
                *o = src[n];
            }
            return out.len();
        }
        let mut pos = self.phase;
        for o in out.iter_mut() {
            let i = pos as usize;
            let frac = pos - i as f64;
            let a = src[i] as f64;
            let b = src[i + 1] as f64;
            *o = (a + (b - a) * frac).round() as i16;
            pos += ratio;
        }
        let consumed = pos as usize;
        self.phase = pos - consumed as f64;
        consumed
    }
}

/// Remove the step discontinuity where an output frame doesn't continue the previous
/// one (concealment entry/exit, a dropped-frame splice, resampler phase reset): ramp a
/// decaying DC offset over the first ~2ms so the waveform stays continuous instead of
/// clicking. This is the cheap end of PLC seam handling — no spectral smoothing, just
/// no step — and it is what makes the remaining gross corrections inaudible.
fn smooth_seam(frame: &mut [i16], prev_last: i16) {
    const RAMP: usize = 96; // 2ms @ 48k
    let delta = prev_last as i32 - frame[0] as i32;
    if delta == 0 {
        return;
    }
    let n_ramp = RAMP.min(frame.len());
    for (n, s) in frame.iter_mut().take(n_ramp).enumerate() {
        let w = 1.0 - (n as f32 + 1.0) / (n_ramp as f32 + 1.0); // 1→0 across the ramp
        *s = (*s as i32 + (delta as f32 * w) as i32).clamp(i16::MIN as i32, i16::MAX as i32) as i16;
    }
}

/// Build the mic capture pipeline DESCRIPTION string from the source-element prefix
/// `src` and the denoise toggle. BOTH branches now deliver F32LE FLOAT all the way to the
/// appsink (NO gst S16 narrow): the appsink applies an in-app soft-knee limiter (see
/// [`soft_limit`]) and narrows to i16 itself. AutoMic's autogain (+20 dB) / compressor push
/// float transients PAST ±1.0 (live probe: |x|=1.114 on signal); letting gst audioconvert
/// narrow those to S16 in-pipeline used to HARD-SATURATE them into the flat-topped
/// ±full-scale clusters that ARE the crackle. Keeping the stream in float to the appsink lets
/// the overshoot reach our limiter intact, where it is tamed BEFORE any int conversion —
/// exactly what a normal WebRTC/Discord capture does via its APM AGC2 output limiter (we run
/// APM-disabled manual-push, so we limit in-app). With `denoise == true` an `audiornnoise`
/// element runs on mono 48k F32LE (its native format) first and stays in float straight into
/// the appsink. RNNoise is a 48k model, so we resample to 48k first.
///
/// audiobuffersplit / audiorate are raw-audio format-agnostic (they size frames from
/// GstAudioInfo bytes-per-frame), so F32LE flows through them unchanged; the explicit F32LE
/// capsfilter just makes negotiation deterministic. No PipeWire/AutoMic change — AutoMic is
/// clean in every other app, so the fix is 100% in Tin Hat's capture path.
/// Windows-only: the `webrtcdsp` fragment for the mic chain, or `""`. Inserted
/// right after the head caps (AEC runs on the rawest signal, BEFORE RNNoise —
/// same staging as the Linux module) and only while the loopback reference tap
/// is up, so its named `webrtcechoprobe` is guaranteed to exist. F32 in/out at
/// the chain's native 48k mono keeps the float-to-appsink limiter contract
/// intact. One job per stage, mirroring aec.rs: echo-cancel + high-pass here,
/// NS stays RNNoise (hard-off), AGC follows the user's persisted mic_agc().
fn aec_dsp_fragment() -> String {
    aec_dsp_fragment_with(true)
}

/// `want_ec=false` = the stage WITHOUT the echo probe (noise suppression + AGC only): the
/// middle rung of the 0.8.28 ladder, and the fallback when the probe can't be bound.
fn aec_dsp_fragment_with(want_ec: bool) -> String {
    // One WebRTC audio-processing stage, both platforms:
    //   * echo-cancel  — only while the reference tap is up (its probe must exist;
    //                    a dsp without a probe hard-fails to start);
    //   * noise-suppression — the "Standard" NS mode (moderate level: the strong
    //                    levels are what gives WebRTC NS its underwater/robot reputation);
    //   * gain-control — adaptive-digital AGC, following the persisted toggle. On the
    //                    legacy module tier the module owns AGC, so it stays out here
    //                    (never two gain stages in series).
    // RNNoise ("Strong") is a separate element AFTER this one (build_mic_desc_dsp), and
    // the Standard and Strong modes are exclusive, so NS never runs twice either.
    let ec = want_ec && dsp_aec_active() && !dsp_ec_blocked();
    // 0.8.29: ONE always-present processing stage, like every reference APM
    // (pre-gain → HPF → AEC → NS → AGC). The noise-suppression MODE is now only a knob on
    // this stage — Off: noise-suppression=false; Standard: moderate; Strong: very-high
    // (Chromium pins WebRTC NS to high for every noiseSuppression:true track) — and the
    // high-pass filter + the user's AGC are there in every mode. Before, Strong set
    // ns=false, the whole fragment vanished unless EC/AGC happened to be on, and RNNoise
    // ran on a raw ungained laptop mic (a field log: level EMA 0.03 → 0.0000 = gated to
    // silence); and on machines where the ladder had dropped the stage, Standard was
    // byte-identical to Off. RNNoise survives only as mic_rnnoise_wanted().
    let mode = mic_ns_mode();
    let ns = mode != NsMode::Off && !mic_rnnoise_wanted();
    let ns_level = if mode == NsMode::Strong { "very-high" } else { "moderate" };
    let agc = mic_agc() && (cfg!(windows) || aec_tier() == AecTier::Dsp);
    if !webrtcdsp_available() {
        return String::new();
    }
    // Deterministic even for a tap that vanishes between this check and element start:
    // build_mic_pipeline smoke-tests at PAUSED and falls back to a dsp-free chain.
    // 0.8.31: the probe is THIS build's (ec_probe_name), living in the same pipeline.
    let probe = if ec { format!("probe={} ", ec_probe_name()) } else { String::new() };
    // 0.8.33: one extra echo-only stage per FURTHER speaker output, ahead of the main stage —
    // each pairs with its own reference branch / probe (ec_reference_chain). HPF / NS / AGC run
    // once, in the main stage, after every output's echo is gone.
    // Bench knobs (the test rig's multi-output AEC bench): TINHAT_AEC_XHPF=0 drops the high-pass filter from the extra
    // stages; TINHAT_AEC_XAFTER=1 puts them AFTER the main stage instead of ahead of it.
    let xhpf = std::env::var("TINHAT_AEC_XHPF").map(|v| v != "0").unwrap_or(true);
    let xafter = std::env::var("TINHAT_AEC_XAFTER").map(|v| v == "1").unwrap_or(false);
    let extra: String = if ec {
        (1..ec_binding_outputs().len())
            .map(|i| {
                format!(
                    "webrtcdsp name=tinhatdspx{i} probe={} echo-cancel=true noise-suppression=false \
                     voice-detection=false high-pass-filter={xhpf} gain-control=false ! \
                     audio/x-raw,format=F32LE,rate=48000,channels=1,layout=non-interleaved ! ",
                    ec_probe_name_n(i)
                )
            })
            .collect()
    } else {
        String::new()
    };
    // CAPS TRAP (matches the reference tap): webrtcdsp's float path is
    // layout=NON-interleaved only. Mono makes the two layouts byte-identical,
    // so the flanking audioconverts are caps-relabels, not data shuffles —
    // the signal stays F32 end-to-end (limiter contract intact).
    let (before, after) = if xafter { (String::new(), extra) } else { (extra, String::new()) };
    format!(
        "audioconvert ! \
         audio/x-raw,format=F32LE,rate=48000,channels=1,layout=non-interleaved ! \
         {before}webrtcdsp name=tinhatdsp {probe}echo-cancel={ec} noise-suppression={ns} noise-suppression-level={ns_level} \
         voice-detection=false high-pass-filter=true gain-control={agc} ! \
         audio/x-raw,format=F32LE,rate=48000,channels=1,layout=non-interleaved ! \
         {after}         audioconvert ! \
         audio/x-raw,format=F32LE,rate=48000,channels=1,layout=interleaved ! "
    )
}

fn build_mic_desc(src: &str, denoise: bool) -> String {
    build_mic_desc_dsp(src, denoise, true)
}

fn build_mic_desc_dsp(src: &str, denoise: bool, allow_dsp: bool) -> String {
    // `audiobuffersplit output-buffer-duration=1/100` re-blocks the source's quantum-sized
    // buffers (a RODECaster at a 256-sample/~5.33ms quantum) into EXACT 10ms (480-sample)
    // buffers aligned to the clock. libwebrtc's NativeAudioSource drains on its own 10ms
    // RepeatingTask; fed variable 5.33ms chunks it underran each tick and injected 10ms of
    // SILENCE — the periodic POPPING. Exact 10ms frames + a time-based jitter queue keep the
    // drain fed. (Feed loop also guards out any non-480 final/short buffer.)
    // "" on Linux — descriptions there stay byte-identical. `allow_dsp=false` is the
    // Windows no-AEC fallback (probe unavailable must never cost the microphone).
    let dsp = if allow_dsp && !dsp_blocked() { aec_dsp_fragment() } else { String::new() };
    build_mic_desc_frag(src, denoise, dsp)
}

/// 0.8.28: the middle rung — the WebRTC stage WITHOUT the echo probe (NS + AGC stay).
fn build_mic_desc_no_ec(src: &str, denoise: bool) -> String {
    build_mic_desc_frag(src, denoise, aec_dsp_fragment_with(false))
}

fn build_mic_desc_frag(src: &str, denoise: bool, dsp: String) -> String {
    // 0.8.29 — ORDER MATTERS, and this order was benched on the test rig against the
    // exact field deficit (`/tmp/micsim3.py`, `/tmp/micsim4.py`):
    //
    //   src ! queue ! convert/resample ! caps ! audiorate ! [webrtcdsp] ! [audiornnoise] !
    //   audiobuffersplit ! queue(leaky=no) ! appsink(drop=false)
    //
    //  * queue right after the source (Tier 1 #1.2): the source's streaming task now only
    //    dequeues + hands off; every DSP cycle used to run ON the pipewiresrc task, so a
    //    slow DSP cost PipeWire capture cycles. (Our own AEC tap already did this.)
    //  * audiorate BEFORE audiobuffersplit (Tier 1 #1.1): the split absorbed a delivery
    //    deficit for exactly 1 s (discont-wait), then emitted ONE discontinuity; audiorate
    //    repaired it as a single ~280 ms buffer; the old 200 ms leaky queue could not hold
    //    it, so 100 % of the repair was discarded — every second, forever. Repair first,
    //    in 10 ms-sized pieces, and `micrate.add` becomes a truthful gauge of the deficit
    //    instead of one with a 1 s blind spot. Bench: shipping tail 36,060 samples/s at a
    //    3/4 deficit, this order 48,060 (the full 100 frames/s the feeder needs).
    //  * tolerance=40 ms: with audiorate first it sees RAW source timestamps; tolerance=0
    //    turned a ±2-sample PTS jitter into 84 single-sample inserts+drops per second
    //    (clicks). 40 ms absorbs jitter (bench: ±40 samples → add=0 drop=0) and still
    //    repairs a steady deficit in ≥40 ms blocks (bench: 48,060/s at 3/4, 47,940 at 3/5).
    //  * audiobuffersplit right after audiorate, BEFORE the WebRTC stage: webrtcdsp (gst 1.28)
    //    emits NOTHING for input buffers shorter than its 10 ms period and never accumulates
    //    them (bench: 479 samples → 0 output, 480 → fine). PipeWire delivers 256-sample buffers
    //    whenever the graph quantum is 256 — which any browser call or Discord session causes
    //    (10 ms requests round DOWN to a power of two) — so the stage rung "delivered nothing"
    //    exactly on the machines with other voice apps open. Exact 10 ms blocks in, always.
    //  * leaky=no + drop=false: captured audio is NEVER deleted on a microphone. A full
    //    queue must back-pressure (it can't here — the appsink hands off to an unbounded
    //    channel), not silently destroy the user's voice.
    let rnnoise = if denoise {
        // audiornnoise wants F32 mono @ its model rate (48k), stays in float to the appsink.
        "audiornnoise ! audio/x-raw,format=F32LE,rate=48000,channels=1,layout=interleaved ! "
    } else {
        ""
    };
    format!(
        "{src} ! \
         queue name=micq max-size-time=200000000 max-size-buffers=0 max-size-bytes=0 leaky=no ! \
         audioconvert ! audioresample ! \
         audio/x-raw,format=F32LE,rate=48000,channels=1,layout=interleaved ! \
         audiorate name=micrate tolerance=40000000 skip-to-first=true ! \
         audiobuffersplit output-buffer-duration=1/100 ! \
         {dsp}{rnnoise}queue max-size-time=400000000 max-size-buffers=0 max-size-bytes=0 leaky=no ! \
         appsink name=mic emit-signals=false max-buffers=8 drop=false sync=false"
    )
}

/// gstpipewire < 1.6 (the Flatpak runtime's PipeWire 1.4.9, older distros) has no `provide-clock`
/// on pipewiresrc; naming it in a parse-launch description fails the WHOLE mic head (10.0.1).
fn pipewiresrc_has_provide_clock() -> bool {
    static HAS: OnceLock<bool> = OnceLock::new();
    *HAS.get_or_init(|| {
        let has = gst::ElementFactory::make("pipewiresrc")
            .build()
            .ok()
            .map(|e| e.find_property("provide-clock").is_some())
            .unwrap_or(false);
        if !has {
            eprintln!("[mic] pipewiresrc has no provide-clock property (gstpipewire < 1.6) — pinning the system clock on the mic pipeline instead");
        }
        has
    })
}

/// Mic capture: selected (or default) source -> mono F32LE 48k -> appsink (which soft-limits
/// and narrows to i16 itself).
fn build_mic_pipeline() -> Result<(gst::Pipeline, gstreamer_app::AppSink)> {
    use std::sync::atomic::Ordering;
    ec_probe_seq().fetch_add(1, Ordering::Relaxed); // 0.8.31: a fresh probe name per build
    let mut node = effective_mic_node();
    if force_default_mic_once().swap(false, Ordering::Relaxed) && node.is_some() {
        eprintln!(
            "[mic] selected input {:?} would not start — capturing from the system default input instead",
            node
        );
        node = None;
    }
    let src = audio_src_fragment(node.as_deref());
    // 0.8.29 (Tier 0 #0.3) — the MIC head only (app/monitor captures keep their defaults):
    //   provide-clock=false   pipewiresrc posts CLOCK_LOST on every PipeWire suspend / driver
    //                         change and nothing here syncs to it (appsink sync=false); the
    //                         system clock is the pipeline clock.
    //   min-buffers=32        headroom on the negotiated buffer count.
    // Deliberately NOT requested: node.latency. PipeWire rounds the quantum DOWN to a power of
    // two by default, so asking for 480/48000 yields 256-sample buffers — and webrtcdsp emits
    // NOTHING for buffers shorter than its 10 ms period (bench, gst 1.28.7: 479 samples → 0
    // output, 480 → fine). The chain re-blocks to exact 10 ms before the stage instead, so the
    // graph quantum (which any browser call or Discord session drags to 256) no longer matters.
    //   10.0.1: `provide-clock` only exists in gstpipewire >= 1.6. The Flatpak runtime
    //   (org.kde.Platform 6.11 = PipeWire 1.4.9) has no such property, parse_launch refused the
    //   whole head, the mic never built and EVERY join failed ("no property provide-clock in
    //   element pipewiresrc"). Probe once; without the property the pipeline is pinned to the
    //   system clock below, which is what provide-clock=false achieved.
    let head_props = if pipewiresrc_has_provide_clock() { "provide-clock=false min-buffers=32" } else { "min-buffers=32" };
    let src = if src.starts_with("pipewiresrc") { format!("{src} {head_props}") } else { src };
    if src.starts_with("pulsesrc") {
        eprintln!(
            "[mic] head: pulsesrc (PulseAudio-compat — {})",
            if prefer_pulse_mic().load(std::sync::atomic::Ordering::Relaxed) {
                "pipewiresrc delivered nothing here"
            } else {
                "the Flatpak default; TINHAT_MIC_PIPEWIRE=1 forces native PipeWire"
            }
        );
    }
    // BENCH ONLY — TINHAT_MICSIM_DROP=<0..1>: drop that fraction of the source's buffers right
    // after the head, timestamps untouched, i.e. exactly the field fault (a source delivering
    // a fixed fraction of realtime with gaps in its timeline). Lets the end-to-end mic bench
    // reproduce the "robotic / static" capture on demand and prove a chain repairs it.
    let src = match std::env::var("TINHAT_MICSIM_DROP").ok().and_then(|v| v.parse::<f64>().ok()) {
        Some(p) if p > 0.0 && p < 1.0 => {
            eprintln!("[mic] BENCH: dropping {:.0}% of source buffers (TINHAT_MICSIM_DROP)", p * 100.0);
            format!("{src} ! identity drop-probability={p:.3}")
        }
        _ => src,
    };
    let plain_only = force_plain_mic_once().swap(false, Ordering::Relaxed);
    // 0.8.28 LADDER: full WebRTC stage → stage without echo-cancel → plain capture. A rung is
    // skipped when the per-call block says it stalled before (see dsp_stage_block) or when the
    // caller asked for the plain chain outright (restart_mic's last resort).
    let mut rungs: Vec<(&str, String)> = Vec::new();
    // 0.8.29: RNNoise rides only when mic_rnnoise_wanted(); the plain rung is PLAIN — it
    // used to keep audiornnoise while dropping the stage that fed it gain.
    let rnnoise = mic_rnnoise_wanted();
    if !plain_only && !dsp_blocked() {
        // 0.8.31: a chain that carries the probe carries its reference branch too.
        let full = ec_with_reference(build_mic_desc(&src, rnnoise));
        if full.contains("webrtcdsp") {
            let had_probe = full.contains("probe=");
            rungs.push((if had_probe { "WebRTC stage" } else { "WebRTC stage (no echo probe)" }, full));
            if had_probe {
                let no_ec = build_mic_desc_no_ec(&src, rnnoise);
                if no_ec.contains("webrtcdsp") {
                    rungs.push(("WebRTC stage without echo cancellation", no_ec));
                }
            }
        }
    }
    rungs.push(("plain capture", build_mic_desc_dsp(&src, rnnoise && !webrtcdsp_available(), false)));
    let n = rungs.len();
    let mut chosen: Option<(gst::Pipeline, String, &str)> = None;
    for (i, (label, desc)) in rungs.into_iter().enumerate() {
        let pipeline = gst::parse::launch(&desc)?
            .downcast::<gst::Pipeline>()
            .map_err(|_| anyhow::anyhow!("mic: not a Pipeline"))?;
        // 10.0.1: the system clock is THE pipeline clock whatever pipewiresrc offers — the same
        // outcome provide-clock=false gives on gstpipewire >= 1.6, now also on runtimes without
        // that property (the Flatpak's 1.4.9). Nothing in the chain syncs to a clock (appsink
        // sync=false), so a pipewire clock that vanishes on suspend must never be selected.
        if desc.contains("pipewiresrc") && !pipewiresrc_has_provide_clock() {
            pipeline.use_clock(Some(&gst::SystemClock::obtain()));
        }
        // AEC coupling trap (a 0.3.0 field log — verified with gst-launch): webrtcdsp
        // HARD-FAILS element start ("No echo probe with name … found", can't reach PAUSED)
        // whenever its probe — which lives in the separate reference-tap pipeline — can't
        // be acquired. The mic must never die for want of echo cancellation: smoke-test any
        // stage-bearing chain at PAUSED, and step down a rung on failure. Then back to NULL,
        // so the real start below is the same NULL→PLAYING every chain gets (0.8.25–0.8.27
        // left the stage-bearing chain parked in PAUSED — the one thing the always-working
        // plain chain never did).
        if desc.contains("webrtcdsp") {
            // 0.8.31: the reference source (a sink monitor / WASAPI loopback) fails at PAUSED
            // when its output is gone — pipewiresrc posts the error during the transition;
            // read it off the bus (non-blocking) so it counts as a failed rung, not a chain
            // that starts and then runs uncancelled behind an "ON" label.
            let paused_ok = set_state_bounded(pipeline.upcast_ref(), gst::State::Paused, 8000, "mic chain probe");
            let bus_err = pipeline
                .bus()
                .and_then(|b| b.pop_filtered(&[gst::MessageType::Error]))
                .map(|m| match m.view() {
                    gst::MessageView::Error(e) => format!("{} (from {})", e.error(), m.src().map(|s| s.path_string().to_string()).unwrap_or_default()),
                    _ => String::from("error"),
                });
            if !paused_ok || bus_err.is_some() {
                set_state_bounded(pipeline.upcast_ref(), gst::State::Null, 1500, "mic chain probe");
                eprintln!(
                    "[mic] start failed with the {label} in the chain — {}{}",
                    if i + 1 < n { "stepping down one rung" } else { "no rung left" },
                    bus_err.map(|e| format!(" [{e}]")).unwrap_or_default()
                );
                aec_fallback_flag().store(true, Ordering::Relaxed);
                continue;
            }
            set_state_bounded(pipeline.upcast_ref(), gst::State::Null, 1500, "mic chain probe");
        }
        chosen = Some((pipeline, desc, label));
        break;
    }
    let (pipeline, desc, label) =
        chosen.ok_or_else(|| anyhow::anyhow!("mic: every capture chain failed to build"))?;
    let flags = (desc.contains("webrtcdsp") as u8) | ((desc.contains("probe=") as u8) << 1);
    mic_chain_flags().store(flags, Ordering::Relaxed);
    // 0.8.31: the canceller's health IS this verdict now — wanted but not in the
    // chain = degraded (the settings pane says so; the healer's one-shot retry rebuilds).
    if aec_tier() == AecTier::Dsp && crate::aec::enabled() && dsp_aec_active() {
        aec_degraded_flag().store(flags & 2 == 0, Ordering::Relaxed);
    }
    *mic_chain_label().lock().unwrap() = label.to_string();
    eprintln!("[mic] capture chain: {label}{}", if desc.contains("audiornnoise") { " + RNNoise" } else { "" });
    let appsink = pipeline
        .by_name("mic")
        .context("mic appsink missing")?
        .downcast::<gstreamer_app::AppSink>()
        .map_err(|_| anyhow::anyhow!("mic sink not an AppSink"))?;
    Ok((pipeline, appsink))
}

// ── Standalone mic monitor (noise-cancel before/after meter) ─────────────────
//
// A self-contained pipeline (NO call, NO LiveKit) that taps the mic at TWO points via a
// `tee`: raw (pre-denoise) and through `audiornnoise` (post-denoise), folding each to RMS in
// `mic_level_pre` / `mic_level`. Lets the settings noise-cancel pane prove RNNoise works even
// outside a call. The in-call mic path is untouched (kept byte-identical).
fn mic_monitor_cell() -> &'static Mutex<Option<gst::Pipeline>> {
    static M: OnceLock<Mutex<Option<gst::Pipeline>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(None))
}

/// Fold one S16LE appsink sample to RMS and hand it to `store`. Shared by both monitor taps.
fn monitor_tap(sink: &gstreamer_app::AppSink, store: fn(f32)) -> Result<gst::FlowSuccess, gst::FlowError> {
    let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
    let buffer = sample.buffer().ok_or(gst::FlowError::Error)?;
    let map = buffer.map_readable().map_err(|_| gst::FlowError::Error)?;
    let samples: Vec<i16> = map
        .as_slice()
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]))
        .collect();
    store(rms_of(&samples));
    Ok(gst::FlowSuccess::Ok)
}

/// Start the standalone pre/post-denoise mic monitor. Returns false if one is already running
/// or the pipeline can't be built. Safe to call alongside a live call (separate pipewiresrc).
pub fn start_mic_monitor() -> bool {
    let mut cell = mic_monitor_cell().lock().unwrap();
    if cell.is_some() {
        // Idempotent SUCCESS: a live monitor satisfies the request (the Settings→noise-test
        // handoff relies on this — its superseded stop leaves the previous monitor running,
        // and returning false here made the caller show "plugin unavailable" over working
        // meters).
        return true;
    }
    if crate::ensure_init().is_err() {
        return false;
    }
    // audiornnoise (gst-plugins-bad) drives the post-denoise tap; without it parse::launch
    // would fail with a confusing error. Pre-flight check so the caller can show a clear
    // "unavailable" hint (micMonitoring stays false) instead of frozen meters.
    if gst::ElementFactory::find("audiornnoise").is_none() {
        eprintln!(
            "[mic-monitor] audiornnoise unavailable — noise cancellation is OFF. It ships in \
             gst-plugin-rsaudiofx (gst-plugins-rs), NOT gst-plugins-bad; the old message named \
             the wrong package and sent people to reinstall something they already had."
        );
        return false;
    }
    let src = audio_src_fragment(mic_target().lock().unwrap().clone().as_deref());
    // tee: monpre = raw mic; monpost = through audiornnoise. Both S16LE/48k/mono, sync=false
    // (meter only). audiornnoise wants F32/48k, so convert up front and feed the tee in F32.
    let desc = format!(
        "{src} ! audioconvert ! audioresample ! \
         audio/x-raw,format=F32LE,rate=48000,channels=1,layout=interleaved ! tee name=mt \
         mt. ! queue max-size-buffers=8 leaky=downstream ! audioconvert ! \
           audio/x-raw,format=S16LE,rate=48000,channels=1,layout=interleaved ! \
           appsink name=monpre emit-signals=false max-buffers=4 drop=true sync=false \
         mt. ! queue max-size-buffers=8 leaky=downstream ! audiornnoise ! audioconvert ! \
           audio/x-raw,format=S16LE,rate=48000,channels=1,layout=interleaved ! \
           appsink name=monpost emit-signals=false max-buffers=4 drop=true sync=false"
    );
    let pipeline = match gst::parse::launch(&desc) {
        Ok(el) => match el.downcast::<gst::Pipeline>() {
            Ok(p) => p,
            Err(_) => {
                eprintln!("[mic-monitor] parsed element is not a Pipeline");
                return false;
            }
        },
        Err(e) => {
            eprintln!("[mic-monitor] build failed: {e}");
            return false;
        }
    };
    if let Some(pre) = pipeline
        .by_name("monpre")
        .and_then(|e| e.downcast::<gstreamer_app::AppSink>().ok())
    {
        pre.set_callbacks(
            gstreamer_app::AppSinkCallbacks::builder()
                .new_sample(|sink| monitor_tap(sink, set_mic_level_pre))
                .build(),
        );
    }
    if let Some(post) = pipeline
        .by_name("monpost")
        .and_then(|e| e.downcast::<gstreamer_app::AppSink>().ok())
    {
        post.set_callbacks(
            gstreamer_app::AppSinkCallbacks::builder()
                // 10.5 (audio review #3): during a call the preview must not drive the CALL's
                // meter — a working preview animated it while the call's own capture was dead.
                .new_sample(|sink| monitor_tap(sink, |r| {
                    if !crate::hear::session_active() && !mic_capture_primed() {
                        set_mic_level(r)
                    }
                }))
                .build(),
        );
    }
    if !set_state_bounded(pipeline.upcast_ref(), gst::State::Playing, 6000, "mic meter start") {
        eprintln!("[mic-monitor] start failed (the capture never started)");
        set_state_bounded(pipeline.upcast_ref(), gst::State::Null, 1500, "mic meter");
        return false;
    }
    eprintln!("[mic-monitor] started (pre/post-denoise meter)");
    *cell = Some(pipeline);
    true
}

/// Stop the standalone mic monitor + zero the meters. Idempotent; never touches a live call.
pub fn stop_mic_monitor() {
    if let Some(p) = mic_monitor_cell().lock().unwrap().take() {
        set_state_bounded(p.upcast_ref(), gst::State::Null, 1500, "mic meter");
        // Zero the meters ONLY when we actually tore a monitor down — otherwise a stop
        // issued for a panel opened during a call (where the monitor's levels and the live
        // call's published mic level share `mic_level_cell`) would briefly stomp the live
        // call's level. (A no-op stop must not touch the cell.)
        set_mic_level(0.0);
        set_mic_level_pre(0.0);
        eprintln!("[mic-monitor] stopped");
    }
}

// ── Standalone OUTPUT monitor (speaker/sink level meter) ─────────────────────
//
// A self-contained pipeline (NO call, NO LiveKit) that taps the SELECTED output sink's
// `.monitor` and folds it to RMS in `output_level`. Lets the settings Audio pane show a
// live speaker meter (everything playing out: game/app/desktop + call) and verify the
// chosen output is actually producing sound — independent of, and analogous to, the mic
// monitor. Touches no playback pipeline, so call audio stays byte-identical.
fn output_monitor_cell() -> &'static Mutex<Option<gst::Pipeline>> {
    static M: OnceLock<Mutex<Option<gst::Pipeline>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(None))
}

/// Start the standalone output meter, tapping the currently-selected sink's `.monitor` (or
/// the system default's, resolved via `pactl get-default-sink`). Returns false if one is
/// already running or the pipeline can't be built. Safe to call alongside a live call
/// (separate pipewiresrc). Re-point it on a device switch via [`set_playback_target`].
pub fn start_output_monitor() -> bool {
    let mut cell = output_monitor_cell().lock().unwrap();
    if cell.is_some() {
        return false;
    }
    if crate::ensure_init().is_err() {
        return false;
    }
    // The selected output sink (set_playback_target / list_audio_outputs), else the system
    // default. We capture its `.monitor` (the loopback of what's playing out). If a default
    // can't be resolved, fall back to a bare `pipewiresrc` (binds to the default monitor).
    let target = playback_target().lock().unwrap().clone().or_else(default_audio_sink);
    // The `.monitor` suffix routes through audio_src_fragment's loopback mapping on
    // Windows (wasapi2src loopback=true on the render endpoint); a bare default head
    // on Windows must ALSO be loopback (a bare wasapi2src would capture the MIC).
    let src = match target {
        Some(sink) => audio_src_fragment(Some(&format!("{sink}.monitor"))),
        None if cfg!(windows) => audio_src_fragment(Some(".monitor")),
        None => audio_src_fragment(None),
    };
    // Mono S16LE 48k -> appsink (meter only, sync=false). audioconvert collapses the sink's
    // (usually stereo) monitor to mono so rms_of sees one interleaved stream.
    let desc = format!(
        "{src} ! audioconvert ! audioresample ! \
         audio/x-raw,format=S16LE,rate=48000,channels=1,layout=interleaved ! \
         appsink name=outmon emit-signals=false max-buffers=4 drop=true sync=false"
    );
    let pipeline = match gst::parse::launch(&desc) {
        Ok(el) => match el.downcast::<gst::Pipeline>() {
            Ok(p) => p,
            Err(_) => {
                eprintln!("[output-monitor] parsed element is not a Pipeline");
                return false;
            }
        },
        Err(e) => {
            eprintln!("[output-monitor] build failed: {e}");
            return false;
        }
    };
    if let Some(out) = pipeline
        .by_name("outmon")
        .and_then(|e| e.downcast::<gstreamer_app::AppSink>().ok())
    {
        out.set_callbacks(
            gstreamer_app::AppSinkCallbacks::builder()
                .new_sample(|sink| monitor_tap(sink, set_output_level))
                .build(),
        );
    }
    if !set_state_bounded(pipeline.upcast_ref(), gst::State::Playing, 6000, "speaker meter start") {
        eprintln!("[output-monitor] start failed (the capture never started)");
        set_state_bounded(pipeline.upcast_ref(), gst::State::Null, 1500, "speaker meter");
        return false;
    }
    eprintln!("[output-monitor] started (speaker meter)");
    *cell = Some(pipeline);
    true
}

/// Stop the standalone output meter + zero the level. Idempotent; never touches a live call.
pub fn stop_output_monitor() {
    if let Some(p) = output_monitor_cell().lock().unwrap().take() {
        set_state_bounded(p.upcast_ref(), gst::State::Null, 1500, "speaker meter");
        set_output_level(0.0);
        eprintln!("[output-monitor] stopped");
    }
}

/// Remote audio playback: appsrc (S16LE 48k stereo) -> [named `volume` @ unity] ->
/// default output, fed from a LiveKit NativeAudioStream in a task. The `volume` element
/// defaults to 1.0 (UNITY = transparent: existing call audio is byte-identical until the
/// user explicitly changes a per-user volume) and is registered in [`track_gains`] under
/// `(identity, category)` so the app can adjust this one participant's level.
/// Returns the running pipeline (keep alive).
/// 10.4.1: ONE playback pipeline for `who`'s `category` track — parsed, its sink and gain registered,
/// started, self-repointed, the 40 ms cushion primed. `use_pulse` None = the platform default
/// (`audio_sink_fragment`); Some(api) = the healer's choice. Returns (pipeline, appsrc, queue,
/// built_with_pulse). The feeder in build_playback owns the result and can replace it mid-call.
fn spawn_playback_pipeline(
    who: &str,
    category: &str,
    sid: &str,
    use_pulse: Option<bool>,
) -> Result<(gst::Pipeline, gstreamer_app::AppSrc, Option<gst::Element>, bool, Arc<std::sync::atomic::AtomicBool>)> {
    // Route to the chosen PipeWire sink, else the system default. ALWAYS a named sink (never
    // autoaudiosink) so set_playback_target can re-point it live mid-call. Empty target = default.
    let target = effective_playback_node().unwrap_or_default();
    let sink = match use_pulse {
        Some(p) => audio_sink_fragment_for(&target, p),
        None => audio_sink_fragment(&target),
    };
    let built_with_pulse = sink.starts_with("pulsesink");
    let gain_name = gain_element_name(who, category);
    eprintln!("[call] building remote-audio playback ({category}) -> {sink} [gain={gain_name}]");
    // Insert a per-track `volume` element after format conversion, at UNITY (1.0). It's
    // a no-op until set_track_gain/set_track_muted moves it.
    // Bounded, leaky receive chain: the producer (libwebrtc's 10ms system-clock tick) and
    // the consumer (DAC-paced sink) run on different clocks, so an unbounded queue
    // accumulates their drift as ever-growing latency over a long call. leaky=downstream
    // trims oldest-first instead; appsrc max-bytes=38400 (~200ms S16LE 48k stereo) +
    // leaky-type=downstream (GStreamer >= 1.20) bounds appsrc's internal queue too (the
    // push loop ignores enough-data). max-size-buffers=64 is a backstop should a buffer
    // ever lack the PTS the queue's time level needs.
    let desc = format!(
        "appsrc name=src is-live=true format=time do-timestamp=true max-bytes=38400 leaky-type=downstream ! \
         audio/x-raw,format=S16LE,rate=48000,channels=2,layout=interleaved ! \
         audioconvert ! audioresample ! volume name={gain_name} volume=1.0 mute=false ! \
         queue name=q max-size-time=200000000 max-size-buffers=64 max-size-bytes=0 leaky=downstream ! {sink}"
    );
    let pipeline = gst::parse::launch(&desc)?
        .downcast::<gst::Pipeline>()
        .map_err(|_| anyhow::anyhow!("playback: not a Pipeline"))?;
    let appsrc = pipeline
        .by_name("src")
        .context("playback appsrc missing")?
        .downcast::<gstreamer_app::AppSrc>()
        .map_err(|_| anyhow::anyhow!("src not an AppSrc"))?;
    // Register this pipeline's sink so a mid-call output switch (set_playback_target) re-points
    // it live. (Stale WeakRefs are skipped there; close() clears the list on teardown.)
    if let Some(out) = pipeline.by_name("out") {
        playback_sinks().lock().unwrap().push(out.downgrade());
    }
    // Register the volume element so the app can adjust this participant's gain. Keep a
    // WeakRef (the pipeline owns the element; dropping the pipeline drops the ref).
    if let Some(gain) = pipeline.by_name(&gain_name) {
        register_track_gain(who, category, sid, &gain);
        // If incoming audio is cut (AFK *or* deafened) when this track subscribes, mute it
        // from the FIRST frame so a peer who joins/republishes/unmutes mid-cut is never
        // audible, and record it in the shared auto-mute set so the AFK/deafen clear restores
        // it. Deafen (unlike AFK) isn't re-applied on a ~120ms poll, so without this guard a
        // peer joining while deafened stayed fully audible. (Skip the ownership record if a
        // #5 pref already muted it in register_track_gain — the user owns that mute.)
        if (away_privacy_active() || deafened_active())
            && !desired_pref_for(who, category).map(|(_, m)| m).unwrap_or(false)
        {
            gain.set_property("mute", true);
            away_muted_names()
                .lock()
                .unwrap()
                .insert((who.to_string(), category.to_string(), sid.to_string()));
        }
    } else {
        eprintln!("[call] WARN: gain element {gain_name} not found after parse (gain control disabled for this track)");
    }
    if !set_state_bounded(pipeline.upcast_ref(), gst::State::Playing, 6000, "remote-audio playback start") {
        set_state_bounded(pipeline.upcast_ref(), gst::State::Null, 1500, "remote-audio playback");
        anyhow::bail!("playback start failed (the output never started)");
    }
    {
        // 10.0.6: three seconds in, log what PipeWire sees of this stream (Flatpak / TINHAT_PW_SNAPSHOT).
        std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_secs(3));
            pw_snapshot("playback running");
        });
    }
    // Self-repoint: if the output routing changed while this pipeline was being built
    // (an AEC toggle mid-subscribe — the module reload/unload window), the target we
    // baked above may already be a dead node, and repoint_playback_sinks walked a
    // snapshot taken BEFORE our sink registered. Detect and NULL-cycle ourselves onto
    // the current effective node (pipewiresink only re-reads target-object on a
    // pw_stream reconnect, so a bare set_property would be a no-op).
    {
        let now_target = effective_playback_node().unwrap_or_default();
        if now_target != target {
            if let Some(out) = pipeline.by_name("out") {
                eprintln!(
                    "[call] playback ({category}) built during an output transition — re-pointing \
                     \"{target}\" -> \"{now_target}\""
                );
                set_state_bounded(&out, gst::State::Null, 1500, "remote-audio playback re-point");
                set_sink_target(&out, &now_target);
                let _ = out.sync_state_with_parent();
            }
        }
    }
    // Prime the cushion with PLAY_TARGET_MS of silence before the first real frame.
    // The servo can only build buffer at its ±2000ppm clamp (~2ms/s), so a cold track
    // would otherwise spend its first half-minute near-empty — every producer hiccup a
    // sink underrun (a pop) exactly when a peer starts talking. One silent buffer costs
    // 40ms of added latency and removes the whole window.
    {
        let pre = vec![0u8; (PLAY_TARGET_MS as usize) * 192]; // 192 B/ms @ 48k stereo S16
        let _ = appsrc.push_buffer(gst::Buffer::from_slice(pre));
    }
    let queue_el = pipeline.by_name("q");
    // 10.4.1: the second stall signal. A sink whose stream the server dropped posts an ERROR on the
    // bus (pulsesink: "Disconnected", pipewiresink: a stream error) — sometimes with the queue
    // filling behind it (the 10.4.1 playback-stall signature), sometimes with the data quietly discarded (no
    // level to watch). The feeder polls the flag once a second and heals on it like on a full queue.
    // 10.8 (leak track, notes/cpu-idle.md §3): the thread ends with its pipeline. It used to wait in
    // timed_pop_filtered(NONE) — a NULLed pipeline's bus is flushing and never wakes a waiter, so the
    // wait never returned: one parked OS thread (holding the bus) for EVERY playback pipeline ever
    // built — every remote mic / soundboard / music track subscribed, every healer rebuild, every
    // reconnect — for the life of the process. Now it polls every 500 ms and leaves once the pipeline
    // is gone, or has sat in NULL for 10 s (a teardown; a NULL→PLAYING cycle is far shorter).
    let errored = Arc::new(std::sync::atomic::AtomicBool::new(false));
    if let Some(bus) = pipeline.bus() {
        let flag = errored.clone();
        let who2 = who.to_string();
        let cat2 = category.to_string();
        let weak = pipeline.downgrade();
        let spawned = std::thread::Builder::new().name("tinhat-play-bus".into()).spawn(move || {
            let mut null_polls = 0u32;
            loop {
                if let Some(msg) =
                    bus.timed_pop_filtered(gst::ClockTime::from_mseconds(500), &[gst::MessageType::Error])
                {
                    if let gst::MessageView::Error(e) = msg.view() {
                        eprintln!(
                            "[play-heal] {who2} ({cat2}): the output pipeline reported an error — {} ({})",
                            e.error(),
                            e.debug().map(|d| d.to_string()).unwrap_or_default().lines().next().unwrap_or("")
                        );
                    }
                    flag.store(true, std::sync::atomic::Ordering::SeqCst);
                    continue;
                }
                if !playback_bus_watch_continues(weak.upgrade().map(|p| (p.current_state(), p.pending_state())), &mut null_polls) {
                    break;
                }
            }
        });
        if let Err(e) = spawned {
            eprintln!("[play-heal] {who} ({category}): no error watcher for this output ({e})");
        }
    }
    Ok((pipeline, appsrc, queue_el, built_with_pulse, errored))
}

/// The playback bus watcher's keep-going rule (10.8 leak track): `pipeline` = the watched pipeline's (current, pending)
/// state, None once it has been freed. It stops for a freed pipeline and after 20 polls (10 s) in a settled NULL; any
/// other state resets the count (a NULL→PLAYING cycle — the self-repoint, a heal — is not a teardown).
fn playback_bus_watch_continues(pipeline: Option<(gst::State, gst::State)>, null_polls: &mut u32) -> bool {
    match pipeline {
        None => false,
        Some((gst::State::Null, gst::State::VoidPending)) => {
            *null_polls += 1;
            *null_polls < 20
        }
        Some(_) => {
            *null_polls = 0;
            true
        }
    }
}

/// Receive-side drift NOTE (r139): the producer (libwebrtc's 10ms system-clock tick)
/// and the consumer (the DAC) run on different crystals, so playback latency slowly
/// drifts; when the bounded leaky queues saturate, each excess buffer is dropped —
/// a hard splice. The obvious fix, a skew servo like the mic feeder's, was built and
/// then REMOVED here: its only observable feedback (appsrc `current-level-bytes` +
/// the gst queue's level) reads ~0 on a live call because the real jitter buffer sits
/// inside PipeWire's own ring, which GStreamer cannot see. A servo on that signal
/// pins itself at the clamp and permanently over-produces samples — it would CAUSE
/// the latency creep it is meant to prevent. So: no rate control here, but `[play-rt]`
/// now reports the level it CAN see, per participant, which is the field evidence a
/// real fix (clock-slaved sink, or a PipeWire-side rate match) would need.
const PLAY_TARGET_MS: f64 = 40.0; // cushion pre-filled in spawn_playback_pipeline

type RebuiltPlayback = Result<(gst::Pipeline, gstreamer_app::AppSrc, Option<gst::Element>, bool, Arc<std::sync::atomic::AtomicBool>)>;
/// 10.5 (audio review #12, Astra 2026-09-22 #5): a rebuilt playback pipeline travels to its feeder in
/// this guard, which NULLs it on drop unless the feeder took it. Dropped PLAYING — the feeder gone
/// before the send, OR aborted with the result still queued — it leaked, and a leaked pulsesink keeps
/// the process's one PulseAudio context alive: exactly the corpse the healer retires before a fresh
/// pulsesink will start.
struct RebuiltGuard(Option<RebuiltPlayback>);
impl Drop for RebuiltGuard {
    fn drop(&mut self) {
        if let Some(Ok((p, ..))) = self.0.take() {
            std::thread::spawn(move || {
                set_state_bounded(p.upcast_ref(), gst::State::Null, 1500, "unadopted playback rebuild");
            });
            eprintln!("[play-heal] a rebuild finished after its track was gone — retired");
        }
    }
}
fn send_or_retire(tx: &std::sync::mpsc::Sender<RebuiltGuard>, built: RebuiltPlayback) {
    let _ = tx.send(RebuiltGuard(Some(built))); // a failed send drops the guard → retired
}

fn build_playback(
    rtc_track: livekit::webrtc::audio_track::RtcAudioTrack,
    who: String,
    category: String,
    sid: String,
) -> Result<(Arc<Mutex<gst::Pipeline>>, tokio::task::JoinHandle<()>)> {
    let (pipeline, appsrc, queue_el, built_with_pulse, errored) = spawn_playback_pipeline(&who, &category, &sid, None)?;
    let mut errored = errored;
    let slot = Arc::new(Mutex::new(pipeline.clone()));
    let slot_task = slot.clone();
    let mut appsrc = appsrc;
    let mut queue_el = queue_el;
    let mut appsrc_el = appsrc.clone().upcast::<gst::Element>();
    let mut pipeline_w = pipeline.downgrade();
    drop(pipeline); // the slot owns it from here

    let mut stream = NativeAudioStream::new(rtc_track, SAMPLE_RATE as i32, 2);
    let task = tokio::spawn(async move {
        // 10.0.6 stall watchdog: the bounded leaky chain sits at ~40 ms while the output drains
        // and pins at its ~200 ms cap when the output stream is connected but never runs (the
        // "can't hear anyone" reports).
        //
        // 10.4.1 playback HEALER (a field laptop: one voice stopped ~5 min into two
        // calls; the [pw-snapshot] at the stall showed our playback streams — and the PulseAudio-
        // compat client that owned them — gone from the graph while the mic streams survived).
        // The 10.0.6 repair switched to the OTHER API once and then declared the output dead;
        // on that machine native PipeWire from the sandbox never plays (why 10.0.4 chose pulse),
        // so the one switch could not help and the call stayed silent. The mic, which has a
        // healer that rebuilds on the SAME path, recovered in 0.6 s. So, symmetric to it: after
        // 4 s of a full queue rebuild the output on the same API (a fresh element = a fresh
        // PipeWire/Pulse client and stream); if it is still full 8 s later, the other API; then
        // keep alternating every 8 s for as long as the stall lasts. Recovery is logged, and the
        // "dead" verdict is gone — a call is never given up on.
        // Why the WHOLE pipeline and not just the sink: when the server drops the stream the old
        // sink returns an error, the queue's streaming task pauses on that error and nothing
        // restarts it — an element swapped in after that never gets a buffer (proved on the Flatpak test machine:
        // ten in-place swaps, none drained). A fresh pipeline has a fresh task; the feeder just
        // points at the new appsrc, and the old pipeline is retired on a detached thread.
        let mut full_checks: u32 = 0;
        let mut heals: u32 = 0;
        let mut next_heal_check: u32 = 4;
        let mut cur_pulse = built_with_pulse;
        let mut stalled_since: Option<std::time::Instant> = None;
        let mut pending: Option<std::sync::mpsc::Receiver<RebuiltGuard>> = None;
        // A bus ERROR or a refused push means the pipeline is broken NOW — heal on the next check
        // without waiting for the queue to fill (it may never, when the sink drops data quietly).
        let mut force_heal = false;
        let mut push_errs: u64 = 0;
        // Coming home: a heal that landed on the OTHER API keeps the call audible, but on the
        // machines that matter (a Linux desktop running the Flatpak) native PipeWire "plays" silence, which no
        // level can see. So once healthy on the other API, try the preferred one again after 30 s
        // (then 60, 120, … up to 5 min between tries); a return that starts is adopted, one that
        // fails is retried later. Builds the new pipeline FIRST here (the old one is alive).
        let preferred_pulse = built_with_pulse;
        let mut healthy_on_other: u32 = 0;
        let mut return_backoff: u32 = 30;
        let mut returning = false;
        // Frame counter: if these tick up, the peer's audio is DECRYPTING + arriving
        // (so silence = output routing). If it stays 0, it's keys/decryption.
        let mut n: u64 = 0;
        let mut last_check = std::time::Instant::now();
        let mut last_heal_poll = std::time::Instant::now();
        loop {
            // 10.5 (audio review #1): a 1 s wall-clock arm. The healer lived entirely inside
            // `stream.next()` — when the frames stopped its clock stopped with them: a finished
            // rebuild was never adopted, a bus error never acted on, a Fix from the tile unheard.
            let frame = match tokio::time::timeout(std::time::Duration::from_secs(1), stream.next()).await {
                Ok(Some(f)) => Some(f),
                Ok(None) => break,
                Err(_) => None,
            };
            if let Some(frame) = frame.as_ref() {
                // #B: stash this frame's RMS so the app can animate this peer's waveform.
                set_track_level(&who, &category, rms_of(&frame.data));
                crate::hear::note_recv(&sid, &frame.data); // 10.5: "can't hear X", receipts, echo
            }
            // A rebuild that finished on its thread: adopt the new pipeline, retire the old one.
            if let Some(rx) = pending.as_ref() {
                match rx.try_recv().map(|mut g| g.0.take()) {
                    Ok(None) => {
                        pending = None;
                    }
                    Ok(Some(Ok((p2, a2, q2, pulse2, e2)))) => {
                        pending = None;
                        let was_return = returning;
                        returning = false;
                        let old = std::mem::replace(&mut *slot_task.lock().unwrap(), p2.clone());
                        appsrc = a2;
                        appsrc_el = appsrc.clone().upcast::<gst::Element>();
                        queue_el = q2;
                        pipeline_w = p2.downgrade();
                        cur_pulse = pulse2;
                        errored = e2;
                        push_errs = 0;
                        let who2 = who.clone();
                        std::thread::spawn(move || {
                            set_state_bounded(old.upcast_ref(), gst::State::Null, 3000, "stalled playback (replaced)");
                            eprintln!("[play-heal] {who2}: the stalled pipeline was retired");
                        });
                        if was_return {
                            healthy_on_other = 0;
                            eprintln!(
                                "[play-heal] {who} ({category}): back on {} (the preferred output)",
                                if pulse2 { "pulsesink" } else { "pipewiresink" }
                            );
                        } else {
                            eprintln!(
                                "[play-heal] {who} ({category}): output now {} (fresh pipeline)",
                                if pulse2 { "pulsesink" } else { "pipewiresink" }
                            );
                        }
                    }
                    Ok(Some(Err(e))) => {
                        pending = None;
                        if returning {
                            returning = false;
                            healthy_on_other = 0;
                            eprintln!("[play-heal] {who} ({category}): the preferred output would not start — staying put ({e:#})");
                        } else {
                            eprintln!("[play-heal] {who} ({category}): rebuild failed: {e:#}");
                        }
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {}
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        pending = None;
                        returning = false;
                    }
                }
            }
            // 10.5: the Fix on this person's call tile = rebuild the output now.
            if category == "mic"
                && pending.is_none()
                && last_heal_poll.elapsed() >= std::time::Duration::from_millis(250)
            {
                last_heal_poll = std::time::Instant::now();
                if crate::hear::take_heal_request(&who) {
                    eprintln!("[play-heal] {who} ({category}): repair requested from the call tile — rebuilding");
                    force_heal = true;
                }
            }
            // Once a second of WALL time (Astra 2026-09-22 #14: counted in frames, a track delivering
            // one frame per 900 ms stretched a check — and a requested repair — to minutes), and at
            // once for a repair request or a bus error.
            let due = last_check.elapsed() >= std::time::Duration::from_secs(1);
            let urgent = force_heal || errored.load(std::sync::atomic::Ordering::SeqCst);
            let frame_check = frame.is_some() && n > 0 && (due || urgent);
            let idle_check = frame.is_none() && urgent;
            if frame_check || idle_check {
                last_check = std::time::Instant::now();
                let buffered_ms = appsrc_el.property::<u64>("current-level-bytes") as f64 / 192.0
                    + queue_el
                        .as_ref()
                        .map(|q| q.property::<u64>("current-level-time") as f64 / 1e6)
                        .unwrap_or(0.0);
                if errored.swap(false, std::sync::atomic::Ordering::SeqCst) {
                    force_heal = true;
                }
                if buffered_ms >= 170.0 || force_heal {
                    full_checks += 1;
                } else if frame_check {
                    // (Only a FRAME-driven check may call this healthy: with no frames arriving an
                    // empty queue says nothing about the output.)
                    if heals > 0 {
                        eprintln!(
                            "[play-heal] {who} ({category}): playing again on {} after {heals} rebuild(s), {:.0} s of silence",
                            if cur_pulse { "pulsesink" } else { "pipewiresink" },
                            stalled_since.map(|t| t.elapsed().as_secs_f64()).unwrap_or(0.0)
                        );
                        crate::hear::note_play_stall(&sid, false);
                    }
                    full_checks = 0;
                    heals = 0;
                    next_heal_check = 4;
                    stalled_since = None;
                    // Healthy on the other API: count towards a try at coming home.
                    if cur_pulse != preferred_pulse && pending.is_none() {
                        healthy_on_other += 1;
                        if healthy_on_other >= return_backoff {
                            healthy_on_other = 0;
                            return_backoff = (return_backoff * 2).min(300);
                            returning = true;
                            eprintln!(
                                "[play-heal] {who} ({category}): trying the preferred output again ({})",
                                if preferred_pulse { "pulsesink" } else { "pipewiresink" }
                            );
                            let (tx, rx) = std::sync::mpsc::channel::<RebuiltGuard>();
                            pending = Some(rx);
                            let (w2, c2, s2) = (who.clone(), category.clone(), sid.clone());
                            std::thread::spawn(move || {
                                send_or_retire(&tx, spawn_playback_pipeline(&w2, &c2, &s2, Some(preferred_pulse)));
                            });
                        }
                    }
                }
                if (full_checks >= next_heal_check || force_heal) && pending.is_none() {
                    force_heal = false;
                    if stalled_since.is_none() {
                        stalled_since = Some(std::time::Instant::now());
                        crate::hear::note_play_stall(&sid, true); // 10.5: "can't hear X — repairing"
                    }
                    // Rung 1: the same API, fresh. Rungs 2+: alternate.
                    let use_pulse = if heals == 0 { cur_pulse } else { !cur_pulse };
                    heals += 1;
                    next_heal_check = full_checks + 8;
                    eprintln!(
                        "[play-heal] {who} ({category}): the output stream is not draining (queue full for {full_checks} s) — \
                         rebuild {heals} on {}",
                        if use_pulse { "PulseAudio-compat (pulsesink)" } else { "native PipeWire (pipewiresink)" }
                    );
                    let (tx, rx) = std::sync::mpsc::channel::<RebuiltGuard>();
                    pending = Some(rx);
                    returning = false;
                    let (w2, c2, s2) = (who.clone(), category.clone(), sid.clone());
                    // Retire the broken pipeline FIRST: GStreamer's pulsesinks share one PulseAudio
                    // context per process, and a dead one stays around while any element still holds
                    // it — a fresh pulsesink built beside the old one joined the corpse and never
                    // started (the Flatpak test machine: "playback start failed (the output never started)"). NULLing
                    // the old pipeline drops its reference; the feeder's pushes into it are refused
                    // meanwhile (that path no longer ends the feeder).
                    let dead = slot_task.lock().unwrap().clone();
                    std::thread::spawn(move || {
                        set_state_bounded(dead.upcast_ref(), gst::State::Null, 3000, "stalled playback (retire first)");
                        send_or_retire(&tx, spawn_playback_pipeline(&w2, &c2, &s2, Some(use_pulse)));
                    });
                    if heals == 1 {
                        pw_snapshot("playback stalled");
                    } else if heals == 3 {
                        pw_snapshot("playback still stalled after two rebuilds");
                    }
                }
            }
            let Some(frame) = frame else { continue }; // an idle tick: checks only
            // Diagnostics on the first frame and every 30s (scan for peak only there).
            if n == 0 || n % 3000 == 0 {
                let peak = frame.data.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
                if n == 0 {
                    // [recv6-decoded] the peer's audio is DECRYPTING + arriving; if it stays
                    // silent now, the issue is output routing, not keys/decryption.
                    eprintln!("[recv6-decoded] first decoded AUDIO frame from {who} (peak={peak}/32767)");
                }
                // What gst can still see of the path to the DAC (see the drift NOTE
                // above): if this climbs toward the 400ms ceiling over a long call, the
                // queues are saturating and the receive path IS dropping buffers.
                let buffered_ms = appsrc_el.property::<u64>("current-level-bytes") as f64 / 192.0
                    + queue_el
                        .as_ref()
                        .map(|q| q.property::<u64>("current-level-time") as f64 / 1e6)
                        .unwrap_or(0.0);
                // [play-rt] identity-attributed receive health: WHOSE stream, its track
                // kind, whether audio is still arriving, how deep the queues are. The old
                // anonymous "[call] playback:" line made a silent peer un-attributable.
                eprintln!(
                    "[play-rt] {who} ({category}): {} frames decoded, peak={peak}/32767 \
                     gstqueue={buffered_ms:.0}ms",
                    n + 1,
                );
            }
            n += 1;
            let mut bytes = Vec::with_capacity(frame.data.len() * 2);
            for s in frame.data.iter() {
                bytes.extend_from_slice(&s.to_le_bytes());
            }
            let buffer = gst::Buffer::from_slice(bytes);
            if appsrc.push_buffer(buffer).is_err() {
                // 10.4.1: the old code ENDED the feeder here — a pipeline that failed (NULL / flushing
                // after a stream error) took the track's audio with it for the rest of the call. Now
                // it is the third stall signal: keep the frames flowing (dropped until the rebuild
                // lands) and heal.
                push_errs += 1;
                if push_errs == 1 {
                    eprintln!("[play-heal] {who} ({category}): the output pipeline refused a buffer — rebuilding");
                    force_heal = true;
                }
            }
        }
        eprintln!("[play-rt] {who} ({category}): stream ended after {n} frames");
    });
    Ok((slot, task))
}

/// The remote-video tile's head: the appsrc the feeder pushes into (bounded, 10.8) + a leaky queue. Shared by both display
/// tails and the tests.
const VIDEO_TILE_HEAD: &str = "appsrc name=src is-live=true format=time do-timestamp=true block=false \
     max-buffers=3 leaky-type=downstream ! queue max-size-buffers=4 leaky=downstream";

/// Build a remote-video render pipeline: `appsrc` (raw I420, caps set on the first
/// frame) -> `glupload` -> `glcolorconvert` -> `qml6glsink`. Created in NULL; the
/// C++ bridge ([`crate`] consumers) attaches it to a QML `GstGLQt6VideoItem` and
/// sets it PLAYING from Qt's render thread so it shares Qt's GL context.
fn build_video_pipeline() -> Result<(gst::Pipeline, gstreamer_app::AppSrc)> {
    // Windows display tail (MEDIA_PIPELINE.md §6.3): no qml6glsink in any upstream
    // Windows GStreamer build, and Qt's default RHI is D3D11 — the tail is a plain
    // `appsink` (same name so the attach shim finds it); the C++ bridge copies I420
    // into QVideoFrames for a QML VideoOutput's QVideoSink (the proven TINHAT_QVS_TEST
    // path). Linux keeps the GL zero-ish-copy tail unchanged.
    // 10.8 (leak track): the appsrc's own queue is bounded too (max-buffers + leaky-type, GStreamer ≥ 1.20). With
    // block=false and no bound, a tile set PAUSED (the window minimized/hidden, pause_remote_tiles) queued EVERY frame
    // the feeder still pushed — ~3 MB per 1080p frame — until the SFU pause took effect or forever when it did not
    // (a track subscribed while hidden, a republished share): the one GB-scale growth path in the tile code.
    let desc = if cfg!(windows) {
        format!("{VIDEO_TILE_HEAD} ! appsink name=sink emit-signals=false max-buffers=2 drop=true sync=false")
    } else {
        format!("{VIDEO_TILE_HEAD} ! glupload ! glcolorconvert ! qml6glsink name=sink sync=false")
    };
    let pipeline = gst::parse::launch(&desc)?
        .downcast::<gst::Pipeline>()
        .map_err(|_| anyhow::anyhow!("video: not a Pipeline"))?;
    let appsrc = pipeline
        .by_name("src")
        .context("video appsrc missing")?
        .downcast::<gstreamer_app::AppSrc>()
        .map_err(|_| anyhow::anyhow!("video src not an AppSrc"))?;
    Ok((pipeline, appsrc))
}

/// Feed a remote video track's decoded I420 frames into a render pipeline's appsrc.
/// Frames before the pipeline is PLAYING (attach pending) just fail to push and are
/// dropped; the stream ending (`None`) tears the feeder down.
fn spawn_video_feeder(
    rtc_track: livekit::webrtc::video_track::RtcVideoTrack,
    appsrc: gstreamer_app::AppSrc,
    label: String,
) -> tokio::task::JoinHandle<()> {
    let fut = async move {
        let mut stream = NativeVideoStream::new(rtc_track);
        let mut caps_set: Option<(u32, u32)> = None;
        let mut count: u64 = 0;
        let mut warned_fmt = false;
        // A field lesson: "it lags" reports need NUMBERS: wall-clock the 300-frame
        // periodic log below into a decode-side fps, so the next slow-stream report
        // says "28.4 fps" instead of "smooth decay, randomly catches up".
        let mut rate_t = std::time::Instant::now();
        while let Some(frame) = stream.next().await {
            // Received tracks decode to I420 in practice; skip anything exotic.
            let i420 = match frame.buffer.as_i420() {
                Some(b) => b,
                None => {
                    if !warned_fmt {
                        warned_fmt = true;
                        eprintln!(
                            "[call] video tile: frame is {:?}, not I420 — skipping (needs conversion)",
                            frame.buffer.buffer_type()
                        );
                    }
                    continue;
                }
            };
            let w = i420.width();
            let h = i420.height();
            if w == 0 || h == 0 {
                continue;
            }
            let info = match gstreamer_video::VideoInfo::builder(
                gstreamer_video::VideoFormat::I420,
                w,
                h,
            )
            .build()
            {
                Ok(i) => i,
                Err(_) => continue,
            };
            if caps_set != Some((w, h)) {
                if let Ok(caps) = info.to_caps() {
                    appsrc.set_caps(Some(&caps));
                }
                caps_set = Some((w, h));
                eprintln!("[call] video tile stream is {w}x{h}");
            }
            let (sy, su, sv) = i420.data();
            let (stride_y, stride_u, stride_v) = i420.strides();
            // Luma peak/mean over the Y plane — distinguishes a DECODED frame (real
            // content) from an all-black tile, so "frames arrive but tile is black" =
            // a render bug, not a decrypt bug.
            if count == 0 || count % 300 == 0 {
                let n = sy.len().max(1);
                let (mut peak, mut sum): (u8, u64) = (0, 0);
                for &p in sy.iter() {
                    if p > peak { peak = p; }
                    sum += p as u64;
                }
                let mean = sum / n as u64;
                if count == 0 {
                    eprintln!("[recv6-decoded] first decoded VIDEO frame {label} {w}x{h} luma peak={peak} mean={mean}");
                } else {
                    let el = rate_t.elapsed().as_secs_f64();
                    let fps = if el > 0.0 { 300.0 / el } else { 0.0 };
                    eprintln!("[call] video tile {label}: {count} frames, luma peak={peak} mean={mean}, ~{fps:.1} fps");
                }
                rate_t = std::time::Instant::now();
            }
            let bytes = pack_i420(
                &info,
                [
                    (sy, stride_y as usize),
                    (su, stride_u as usize),
                    (sv, stride_v as usize),
                ],
                w,
                h,
            );
            let buffer = gst::Buffer::from_mut_slice(bytes);
            let _ = appsrc.push_buffer(buffer);
            count += 1;
        }
        let _ = appsrc.end_of_stream();
    };
    // create_video_tile runs on the Qt GUI thread (qml6glsink affinity), which has NO
    // tokio runtime in its TLS — a bare `tokio::spawn` here would PANIC ("there is no
    // reactor running…") the instant a peer publishes video, crashing the app and
    // leaving the remote tile black. Spawn on the runtime Handle captured in `connect`
    // (same pattern as `teardown_aux`), which is runtime-agnostic across threads.
    match aux_runtime().lock().unwrap().clone() {
        Some(rt) => rt.spawn(fut),
        None => {
            eprintln!("[call] video feeder: no runtime handle captured — using tokio::spawn");
            tokio::spawn(fut)
        }
    }
}

/// Copy a decoded I420 frame (its own per-plane strides) into a tightly-laid-out
/// buffer matching GStreamer's `VideoInfo` strides/offsets for I420 WxH.
fn pack_i420(
    info: &gstreamer_video::VideoInfo,
    planes: [(&[u8], usize); 3],
    w: u32,
    h: u32,
) -> Vec<u8> {
    let mut out = vec![0u8; info.size()];
    let offsets = info.offset();
    let strides = info.stride();
    let dims = [
        (w as usize, h as usize),                                 // Y
        (((w + 1) / 2) as usize, ((h + 1) / 2) as usize),         // U
        (((w + 1) / 2) as usize, ((h + 1) / 2) as usize),         // V
    ];
    for p in 0..3 {
        let (src, src_stride) = planes[p];
        let (row_bytes, rows) = dims[p];
        let off = offsets[p];
        let dst_stride = strides[p] as usize;
        let copy = row_bytes.min(src_stride).min(dst_stride);
        for row in 0..rows {
            let s = row * src_stride;
            let d = off + row * dst_stride;
            if s + copy <= src.len() && d + copy <= out.len() {
                out[d..d + copy].copy_from_slice(&src[s..s + copy]);
            }
        }
    }
    out
}

/// Report from [`rejoin_keytest`].
pub struct RejoinReport {
    /// Loud (peak > 2000) decoded frames from the publisher's FIRST session.
    pub phase1_loud: u64,
    /// Loud decoded frames from the REJOINED session's new track — counted only after
    /// its (deliberately late) key install, so they prove cryptor recovery too.
    pub phase2_loud: u64,
    /// The freshness registry rejected a stale same-index re-install.
    pub stale_rejected: bool,
    /// A byte-identical reinstall reported success (idempotence).
    pub reinstall_ok: bool,
}

/// Headless REJOIN KEY GATE (`TINHAT_REJOIN_KEYTEST=1`, see main.rs): prove the flow that
/// used to be the join/leave coin flip — a peer leaves and rejoins with a NEW key at an
/// ADVANCED index — decodes end-to-end:
///  1. subscriber hears publisher session 1 (key K1 @ index 7, sender-index pinned);
///  2. publisher closes and reconnects under the SAME identity, minting K2 @ index 8;
///     the subscriber's key install is DELAYED 3s past publish, so frames arrive
///     undecryptable first and the cryptor must RECOVER on set_key (failure_tolerance
///     -1 semantics) — the exact shape of a real announce lag;
///  3. a stale-key stomp at the same index (the old room-poll bug) must be REJECTED by
///     the freshness registry while a byte-identical reinstall stays accepted.
/// Uses the REAL install path (`install_peer_key_at` against the registered keyring).
pub async fn rejoin_keytest(
    ws_url: &str,
    sub_jwt: &str,
    pub_jwt_1: &str,
    pub_jwt_2: &str,
) -> Result<RejoinReport> {
    use std::time::Duration;
    crate::ensure_init()?;

    const IDX1: u8 = 7;
    const IDX2: u8 = 8;
    let k1: [u8; 16] = [0x21; 16];
    let k2: [u8; 16] = [0x42; 16];
    let now_ms = || {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    };

    // ---- Subscriber: register its keyring EXACTLY like a real call does ----
    let kp_sub = matrix_key_provider();
    pending_peer_keys().lock().unwrap().clear();
    let gen = KEYRING_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    *active_key_provider().lock().unwrap() = Some((gen, kp_sub.clone()));
    let mut sub_opts = RoomOptions::default();
    sub_opts.encryption =
        Some(E2eeOptions { encryption_type: EncryptionType::Gcm, key_provider: kp_sub });
    let (sub_room, mut sub_events) =
        Room::connect(ws_url, sub_jwt, sub_opts).await.context("rejoin sub connect")?;
    eprintln!("[rejoin] SUB connected as {:?}", sub_room.local_participant().identity().0);

    // Per-track loud-frame counters, keyed by track sid.
    type Counters = Arc<Mutex<std::collections::HashMap<String, (u64, u64)>>>;
    let counters: Counters = Arc::new(Mutex::new(std::collections::HashMap::new()));
    let c2 = counters.clone();
    let drain = tokio::spawn(async move {
        while let Some(ev) = sub_events.recv().await {
            if let RoomEvent::TrackSubscribed {
                track: RemoteTrack::Audio(t), publication, participant,
            } = ev
            {
                let sid = publication.sid().as_str().to_string();
                eprintln!(
                    "[rejoin] sub got audio sid={sid} from {:?}",
                    participant.identity().0
                );
                let c3 = c2.clone();
                let rtc = t.rtc_track();
                tokio::spawn(async move {
                    let mut stream = NativeAudioStream::new(rtc, SAMPLE_RATE as i32, 1);
                    while let Some(frame) = stream.next().await {
                        let peak = frame.data.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
                        let mut g = c3.lock().unwrap();
                        let e = g.entry(sid.clone()).or_insert((0, 0));
                        e.0 += 1;
                        if peak > 2000 {
                            e.1 += 1;
                        }
                    }
                });
            }
        }
    });

    // One publisher session: connect, pin sender index, publish a 440Hz tone.
    async fn tone_session(
        ws_url: &str,
        jwt: &str,
        key: [u8; 16],
        index: u8,
    ) -> Result<(Room, tokio::task::JoinHandle<()>, String)> {
        let kp = matrix_key_provider();
        let mut opts = RoomOptions::default();
        opts.encryption =
            Some(E2eeOptions { encryption_type: EncryptionType::Gcm, key_provider: kp.clone() });
        let (room, mut events) = Room::connect(ws_url, jwt, opts).await.context("pub connect")?;
        let identity = room.local_participant().identity().0;
        kp.set_key(&ParticipantIdentity(identity.clone()), index as i32, key.to_vec());
        room.e2ee_manager().set_sender_key_index(&room.local_participant().identity(), index as i32);
        tokio::spawn(async move { while events.recv().await.is_some() {} });
        let source = NativeAudioSource::new(AudioSourceOptions::default(), SAMPLE_RATE, 1, 1000);
        let track =
            LocalAudioTrack::create_audio_track("mic", RtcAudioSource::Native(source.clone()));
        room.local_participant()
            .publish_track(
                LocalTrack::Audio(track),
                TrackPublishOptions { source: TrackSource::Microphone, ..Default::default() },
            )
            .await
            .context("pub publish")?;
        let feed = tokio::spawn(async move {
            let spc = SAMPLE_RATE / 100;
            let mut interval = tokio::time::interval(Duration::from_millis(10));
            let mut phase: f32 = 0.0;
            loop {
                interval.tick().await;
                let mut data = vec![0i16; spc as usize];
                for s in data.iter_mut() {
                    phase += 2.0 * std::f32::consts::PI * 440.0 / SAMPLE_RATE as f32;
                    *s = (phase.sin() * 8000.0) as i16;
                }
                let frame = AudioFrame {
                    data: Cow::Owned(data),
                    sample_rate: SAMPLE_RATE,
                    num_channels: 1,
                    samples_per_channel: spc,
                };
                let _ = source.capture_frame(&frame).await;
            }
        });
        Ok((room, feed, identity))
    }

    // ---- Phase 1 ----
    let (room1, feed1, pub_identity) = tone_session(ws_url, pub_jwt_1, k1, IDX1).await?;
    eprintln!("[rejoin] PUB session 1 as {pub_identity:?} (K1 @ index {IDX1})");
    install_peer_key_at(&pub_identity, IDX1, k1, now_ms(), KeySrc::Room);
    tokio::time::sleep(Duration::from_secs(8)).await;
    let phase1_sids: Vec<String> = counters.lock().unwrap().keys().cloned().collect();
    let phase1_loud: u64 = counters.lock().unwrap().values().map(|v| v.1).sum();
    eprintln!("[rejoin] phase1: {} track(s), loud={phase1_loud}", phase1_sids.len());

    // ---- Leave ----
    feed1.abort();
    room1.close().await.ok();
    tokio::time::sleep(Duration::from_secs(2)).await;

    // ---- Phase 2: same identity, NEW key at the NEXT index ----
    let (room2, feed2, pub_identity2) = tone_session(ws_url, pub_jwt_2, k2, IDX2).await?;
    eprintln!("[rejoin] PUB session 2 as {pub_identity2:?} (K2 @ index {IDX2}, key install delayed 3s)");
    tokio::time::sleep(Duration::from_secs(3)).await; // announce lag: frames undecryptable first
    let t2 = now_ms();
    // The real path: a live to-device push of the rejoined peer's key.
    install_peer_key(&pub_identity2, IDX2, k2);
    // Stale stomps (the room-poll bug class), all of which must be REFUSED:
    //  a) an old room event for the same slot — even one only 1s older than the fresh
    //     key's arrival (the "within grace" ordering hazard);
    //  b) a genuinely older history event.
    // A byte-identical re-read must still report success (idempotence).
    let stale_rejected = !install_peer_key_at(&pub_identity2, IDX2, k1, t2 - 1_000, KeySrc::Room)
        && !install_peer_key_at(&pub_identity2, IDX2, k1, t2 - 60_000, KeySrc::Room);
    let reinstall_ok = install_peer_key(&pub_identity2, IDX2, k2);
    // Only count phase-2 loudness from AFTER the install (proves recovery, not luck).
    let baseline: std::collections::HashMap<String, u64> = counters
        .lock()
        .unwrap()
        .iter()
        .map(|(sid, v)| (sid.clone(), v.1))
        .collect();
    tokio::time::sleep(Duration::from_secs(10)).await;
    let phase2_loud: u64 = counters
        .lock()
        .unwrap()
        .iter()
        .filter(|(sid, _)| !phase1_sids.contains(sid))
        .map(|(sid, v)| v.1 - baseline.get(sid.as_str()).copied().unwrap_or(0))
        .sum();

    feed2.abort();
    drain.abort();
    room2.close().await.ok();
    sub_room.close().await.ok();
    *active_key_provider().lock().unwrap() = None;
    pending_peer_keys().lock().unwrap().clear();

    Ok(RejoinReport { phase1_loud, phase2_loud, stale_rejected, reinstall_ok })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The mute layers are process-global; tests in this module must not interleave.
    fn test_lock() -> std::sync::MutexGuard<'static, ()> {
        static L: OnceLock<Mutex<()>> = OnceLock::new();
        L.get_or_init(|| Mutex::new(())).lock().unwrap_or_else(|e| e.into_inner())
    }
    fn reset_layers() {
        track_gains().lock().unwrap().clear();
        away_muted_names().lock().unwrap().clear();
        desired_track_prefs().lock().unwrap().clear();
        *away_active().lock().unwrap() = false;
        *deaf_active().lock().unwrap() = false;
        forget_all_rec_only();
        forget_all_layers();
        recv_policy().lock().unwrap().hidden.clear();
    }

    /// RBAC: the `tinhat.moderation` packet round-trips (quotes and all), rejects unknown
    /// actions / missing targets, and is a reliable packet on its topic.
    #[test]
    fn moderation_packet_roundtrip() {
        let json = moderation_packet_json("mute", "@a:s:DEV", "@m:s:DEV2", "Mod \"Q\"");
        let p = parse_moderation_packet(&json).unwrap();
        assert_eq!(
            p,
            ModerationPacket {
                action: "mute".into(),
                target: "@a:s:DEV".into(),
                by: "@m:s:DEV2".into(),
                name: "Mod \"Q\"".into(),
            }
        );
        assert!(parse_moderation_packet("{\"action\":\"mute\"}").is_none());
        assert!(parse_moderation_packet("{\"action\":\"deafen\",\"target\":\"x\"}").is_none());
        assert!(parse_moderation_packet("not json").is_none());
        let d = moderation_data_packet(&json, vec!["@a:s:DEV".into()]);
        assert_eq!(d.topic.as_deref(), Some(MODERATION_TOPIC));
        assert!(d.reliable);
        assert_eq!(d.destination_identities.len(), 1);
        assert_eq!(d.destination_identities[0].0, "@a:s:DEV");
        assert!(moderation_data_packet(&json, Vec::new()).destination_identities.is_empty());
    }

    /// RBAC: `mic_allowed` follows LiveKit's `can_publish` + `can_publish_sources` rules
    /// (empty list = every source; the server's mute lists everything but MICROPHONE).
    #[test]
    fn mic_allowed_follows_publish_sources() {
        assert!(mic_allowed(true, &[]));
        assert!(mic_allowed(true, &[SRC_CAMERA, SRC_MICROPHONE]));
        assert!(!mic_allowed(true, &[SRC_CAMERA, SRC_SCREEN_SHARE, SRC_SCREEN_SHARE_AUDIO]));
        assert!(!mic_allowed(false, &[]));
        assert!(!mic_allowed(false, &[SRC_MICROPHONE]));
        // Soundboard one-shots (Unknown) go with the mic; music (ScreenshareAudio) stays.
        let muted = [SRC_CAMERA, SRC_SCREEN_SHARE, SRC_SCREEN_SHARE_AUDIO];
        assert!(!source_allowed(true, &muted, SRC_UNKNOWN));
        assert!(source_allowed(true, &muted, SRC_SCREEN_SHARE_AUDIO));
        assert_eq!(proto_source(TrackSource::Microphone), SRC_MICROPHONE);
        assert_eq!(proto_source(TrackSource::Unknown), SRC_UNKNOWN);
    }

    /// RBAC: the tile-enrichment JSON lists every server-muted identity with who did it.
    #[test]
    fn server_muted_json_lists_entries() {
        let _g = test_lock();
        forget_all_moderation();
        assert_eq!(server_muted_json(), "{}");
        assert!(!i_am_server_muted());
        moderation().lock().unwrap().muted.insert("@a:s:D".into(), ("@m:s:E".into(), "Mod".into()));
        let v: serde_json::Value = serde_json::from_str(&server_muted_json()).unwrap();
        assert_eq!(v["@a:s:D"]["by"], "@m:s:E");
        assert_eq!(v["@a:s:D"]["name"], "Mod");
        assert_eq!(server_mute_info("@a:s:D"), Some(("@m:s:E".to_string(), "Mod".to_string())));
        forget_all_moderation();
        assert_eq!(server_muted_json(), "{}");
    }

    /// RBAC: the SFU taking MICROPHONE away flips me server-muted exactly once (moderator
    /// unknown until the packet names them), restoring it flips back exactly once, and a
    /// join-time STATE mute is not cleared by a permission that merely says "everything"
    /// (a fresh session always starts with the full set — only a packet clears it).
    #[test]
    fn my_permission_changed_flips_once_each_way() {
        let _g = test_lock();
        forget_all_moderation();
        let me = "@me:s:DEV";
        *local_identity_cell().lock().unwrap() = me.to_string();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<CallEvent>();
        let pkt = |action: &str| ModerationPacket {
            action: action.into(),
            target: me.into(),
            by: "@mod:s:X".into(),
            name: "Mod".into(),
        };
        let muted_srcs = vec![SRC_CAMERA, SRC_SCREEN_SHARE, SRC_SCREEN_SHARE_AUDIO];

        // SFU mute lands before any packet: flipped, moderator unknown, soundboard blocked,
        // music (ScreenshareAudio) still allowed.
        handle_my_permission_changed(me, true, muted_srcs.clone(), &tx);
        assert!(i_am_server_muted());
        assert!(!my_source_allowed(TrackSource::Microphone));
        assert!(!my_source_allowed(TrackSource::Unknown));
        assert!(my_source_allowed(TrackSource::ScreenshareAudio));
        match rx.try_recv() {
            Ok(CallEvent::ServerMuteChanged { identity, muted, by, by_name }) => {
                assert_eq!(identity, me);
                assert!(muted);
                assert_eq!((by.as_str(), by_name.as_str()), ("", ""));
            }
            _ => panic!("expected ServerMuteChanged{{muted:true}}"),
        }
        // The same permission again is not a second flip.
        handle_my_permission_changed(me, true, muted_srcs.clone(), &tx);
        assert!(rx.try_recv().is_err());

        // The packet names the moderator (sender identity wins over the JSON's `by`).
        handle_moderation_packet("@mod:s:X", pkt("mute"), &tx);
        assert_eq!(server_mute_info(me), Some(("@mod:s:X".to_string(), "Mod".to_string())));
        match rx.try_recv() {
            Ok(CallEvent::ServerMuteChanged { muted: true, by_name, .. }) => assert_eq!(by_name, "Mod"),
            _ => panic!("expected the named ServerMuteChanged{{muted:true}}"),
        }

        // Restore: flips back once, carrying who had muted us; then quiet.
        handle_my_permission_changed(me, true, Vec::new(), &tx);
        assert!(!i_am_server_muted());
        assert_eq!(server_mute_info(me), None);
        assert!(my_source_allowed(TrackSource::Microphone));
        match rx.try_recv() {
            Ok(CallEvent::ServerMuteChanged { muted: false, by, .. }) => assert_eq!(by, "@mod:s:X"),
            _ => panic!("expected ServerMuteChanged{{muted:false}}"),
        }
        handle_my_permission_changed(me, true, Vec::new(), &tx);
        assert!(rx.try_recv().is_err());

        // Join-time state mute: a full permission does NOT clear it…
        set_server_muted_from_state("@mod:s", "Mod");
        assert!(i_am_server_muted());
        assert_eq!(server_mute_info(me), Some(("@mod:s".to_string(), "Mod".to_string())));
        handle_my_permission_changed(me, true, Vec::new(), &tx);
        assert!(i_am_server_muted());
        assert!(rx.try_recv().is_err());
        // …an `unmute` packet does.
        handle_moderation_packet("@mod:s:X", pkt("unmute"), &tx);
        assert!(!i_am_server_muted());
        assert_eq!(server_mute_info(me), None);
        assert!(matches!(rx.try_recv(), Ok(CallEvent::ServerMuteChanged { muted: false, .. })));

        // A `disconnect` addressed to me parks the removal notice (no event yet — the
        // SFU's Disconnected{ParticipantRemoved} consumes it); one for someone else doesn't.
        handle_moderation_packet("@mod:s:X", pkt("disconnect"), &tx);
        let notice = moderation().lock().unwrap().removal_notice.clone();
        assert_eq!(notice.map(|(b, n, _)| (b, n)), Some(("@mod:s:X".to_string(), "Mod".to_string())));
        assert!(rx.try_recv().is_err());
        forget_all_moderation();
        handle_moderation_packet(
            "@mod:s:X",
            ModerationPacket { action: "disconnect".into(), target: "@other:s:Q".into(), by: String::new(), name: String::new() },
            &tx,
        );
        assert!(moderation().lock().unwrap().removal_notice.is_none());
        // A packet with no sender identity (never happens on the SFU) is ignored.
        handle_moderation_packet("", pkt("mute"), &tx);
        assert!(!i_am_server_muted());
        assert!(rx.try_recv().is_err());

        forget_all_moderation();
        *local_identity_cell().lock().unwrap() = String::new();
    }

    /// Game chat silences a peer's MIC here only while BOTH of us have the flag;
    /// it never touches their share audio, composes with the user's own mute, and never
    /// un-mutes through an active deafen.
    #[test]
    fn game_chat_mutes_only_mutual_peers() {
        let _g = test_lock();
        let _ = gst::init();
        reset_layers();
        let mk = |name: &str| gst::ElementFactory::make("volume").name(name).build().unwrap();
        let a_mic = mk("gc_a_mic");
        let a_share = mk("gc_a_share");
        let b_mic = mk("gc_b_mic");
        register_track_gain("@a:example.com:D1", "mic", "sid-a", &a_mic);
        register_track_gain("@a:example.com:D1", "screenshare", "sid-as", &a_share);
        register_track_gain("@b:example.com:D2", "mic", "sid-b", &b_mic);

        // A announces game chat; I am not in it -> I still hear A.
        handle_game_chat_announcement("@a:example.com:D1", true);
        assert!(!a_mic.property::<bool>("mute"), "not in game chat myself: A stays audible");
        // I join game chat -> A's mic is silenced here, B (not in game chat) is not.
        set_local_game_chat(true);
        assert!(a_mic.property::<bool>("mute"), "mutual game chat mutes A's mic");
        assert!(!a_share.property::<bool>("mute"), "game chat never touches share audio");
        assert!(!b_mic.property::<bool>("mute"), "B is not in game chat: audible");
        // A late-registering track of A inherits the layer.
        let a_mic2 = mk("gc_a_mic2");
        register_track_gain("@a:example.com:D1", "mic", "sid-a2", &a_mic2);
        assert!(a_mic2.property::<bool>("mute"), "late mic track of a game-chat peer starts muted");
        // A leaves game chat -> audible again.
        handle_game_chat_announcement("@a:example.com:D1", false);
        assert!(!a_mic.property::<bool>("mute") && !a_mic2.property::<bool>("mute"));
        // Back on, then I turn mine off -> restored.
        handle_game_chat_announcement("@a:example.com:D1", true);
        assert!(a_mic.property::<bool>("mute"));
        set_local_game_chat(false);
        assert!(!a_mic.property::<bool>("mute"), "my flag off restores everyone");
        // The user's own mute of A survives a game-chat cycle.
        set_track_muted("@a:example.com", "mic", true);
        set_local_game_chat(true);
        set_local_game_chat(false);
        assert!(a_mic.property::<bool>("mute"), "user mute survives game chat on/off");
        set_track_muted("@a:example.com", "mic", false);
        assert!(!a_mic.property::<bool>("mute"));
        // Deafened: leaving game chat must NOT leak A through the cut; undeafen restores.
        set_local_game_chat(true);
        set_deafened(true);
        set_local_game_chat(false);
        assert!(a_mic.property::<bool>("mute"), "no un-mute through an active deafen");
        set_deafened(false);
        assert!(!a_mic.property::<bool>("mute"), "undeafen restores the handed-over track");
        assert_eq!(game_chat_json(), "{\"me\":false,\"peers\":[\"@a:example.com:D1\"],\"muted\":[]}");
        reset_layers();
    }

    /// 9.4 corners: pulling A aside puts everyone else at 10 % here (every category); for an
    /// outsider a corner's mics are silent; a lone announcer is not a corner; prefs compose;
    /// join / leave / alone-grace / a pull addressed to me / the game-chat swap; the JSON.
    #[test]
    fn corners_pull_aside_join_leave() {
        let _g = test_lock();
        let _ = gst::init();
        reset_layers();
        *local_identity_cell().lock().unwrap() = "@me:example.com:M0".to_string();
        CORNER_ALONE_GRACE_MS.store(0, std::sync::atomic::Ordering::Relaxed);
        let mk = |name: &str| gst::ElementFactory::make("volume").name(name).build().unwrap();
        let vol = |el: &gst::Element| el.property::<f64>("volume");
        let near = |a: f64, b: f64| (a - b).abs() < 1e-6; // the volume element keeps float precision
        let (a_mic, a_music, b_mic, c_mic, c_share) =
            (mk("co_a_mic"), mk("co_a_music"), mk("co_b_mic"), mk("co_c_mic"), mk("co_c_share"));
        register_track_gain("@a:example.com:D1", "mic", "sid-a", &a_mic);
        register_track_gain("@a:example.com:D1", "music", "sid-am", &a_music);
        register_track_gain("@b:example.com:D2", "mic", "sid-b", &b_mic);
        register_track_gain("@c:example.com:D3", "mic", "sid-c", &c_mic);
        register_track_gain("@c:example.com:D3", "screenshare", "sid-cs", &c_share);
        for id in ["@a:example.com:D1", "@b:example.com:D2", "@c:example.com:D3"] {
            corner_add_participant(id);
        }
        // I pull A aside by bare mxid: an announcement with the pull is queued; nothing changes
        // until A answers (my corner is not active yet).
        assert_eq!(corner_pull("@a:example.com").unwrap(), "@a:example.com:D1");
        let pend = take_corner_announcements();
        assert_eq!(pend.len(), 1);
        let cid = pend[0].0.clone();
        assert!(!cid.is_empty() && pend[0].1 == "@a:example.com:D1");
        assert!(near(vol(&b_mic), 1.0), "not active yet: nothing changes");
        assert!(corner_json().contains("\"active\":false"));
        // A answers -> active: B and C at 10 % in every category, A untouched.
        handle_corner_announcement("@a:example.com:D1", &cid, "");
        assert!(near(vol(&b_mic), 0.1) && near(vol(&c_mic), 0.1) && near(vol(&c_share), 0.1));
        assert!(near(vol(&a_mic), 1.0) && near(vol(&a_music), 1.0));
        // A user pref set while aside composes: B at 50 % -> plays 5 %.
        set_track_gain("@b:example.com", "mic", 0.5);
        assert!(near(vol(&b_mic), 0.05));
        // A late track of C starts at 10 %.
        let c_music = mk("co_c_music");
        register_track_gain("@c:example.com:D3", "music", "sid-cm", &c_music);
        assert!(near(vol(&c_music), 0.1));
        let j = corner_json();
        assert!(j.contains(&format!("\"mine\":\"{cid}\"")) && j.contains("\"active\":true"), "{j}");
        assert!(j.contains("\"members\":[\"@a:example.com:D1\"]") && j.contains("\"@me:example.com:M0\""), "{j}");
        // Pulling someone who is already aside is refused (the UI offers Join instead).
        handle_corner_announcement("@b:example.com:D2", "other", "");
        assert!(near(vol(&b_mic), 0.05), "a lone announcer is not a corner");
        handle_corner_announcement("@c:example.com:D3", "other", "");
        assert!(corner_pull("@b:example.com:D2").is_err());
        // B and C are a corner I am not in: their mics are silent here; C's share stays at 10 %.
        assert!(near(vol(&b_mic), 0.0) && near(vol(&c_mic), 0.0) && near(vol(&c_share), 0.1));
        // I join theirs: B (pref 50 %) and C at full, A (now alone in mine) is background at 10 %.
        assert_eq!(corner_join("@b:example.com:D2").unwrap(), "other");
        assert_eq!(take_corner_announcements(), vec![("other".to_string(), String::new())]);
        assert!(near(vol(&b_mic), 0.5) && near(vol(&c_mic), 1.0) && near(vol(&c_share), 1.0));
        assert!(near(vol(&a_mic), 0.1) && near(vol(&a_music), 0.1));
        // I leave: A back to full; B and C stay a corner -> their mics silent for me, share full.
        corner_leave();
        assert_eq!(take_corner_announcements(), vec![(String::new(), String::new())]);
        assert!(near(vol(&a_mic), 1.0) && near(vol(&a_music), 1.0));
        assert!(near(vol(&b_mic), 0.0) && near(vol(&c_mic), 0.0) && near(vol(&c_share), 1.0));
        // Their corner dissolves -> prefs only.
        handle_corner_announcement("@b:example.com:D2", "", "");
        handle_corner_announcement("@c:example.com:D3", "", "");
        assert!(near(vol(&b_mic), 0.5) && near(vol(&c_mic), 1.0));
        // A pull addressed to me while free: I take the id and answer; while in one: ignored.
        handle_corner_announcement("@a:example.com:D1", "x9", "@me:example.com:M0");
        assert_eq!(corner_mine(), "x9");
        assert_eq!(take_corner_announcements(), vec![("x9".to_string(), String::new())]);
        assert!(near(vol(&b_mic), 0.05), "active with A: B is background");
        handle_corner_announcement("@c:example.com:D3", "z1", "@me:example.com:M0");
        assert_eq!(corner_mine(), "x9", "already aside: a second pull is ignored");
        corner_leave();
        let _ = take_corner_announcements();
        // Alone grace: a pull nobody answers ends by itself on the tick.
        corner_pull("@c:example.com:D3").unwrap();
        assert!(!corner_mine().is_empty());
        corner_tick();
        assert!(corner_mine().is_empty(), "alone past the grace: left");
        let _ = take_corner_announcements();
        // Game chat and a corner never overlap: the flag is suspended while aside, restored after.
        set_local_game_chat(true);
        let _ = take_game_chat_announce_due();
        corner_pull("@a:example.com:D1").unwrap();
        let cid2 = take_corner_announcements()[0].0.clone();
        handle_corner_announcement("@a:example.com:D1", &cid2, "");
        assert!(!local_game_chat() && take_game_chat_announce_due(), "game chat suspended while aside");
        corner_leave();
        assert!(local_game_chat() && take_game_chat_announce_due(), "game chat back after the corner");
        set_local_game_chat(false);
        let _ = take_corner_announcements();
        // The others leaving ends my corner at once (no grace), and everyone plays at their pref.
        corner_pull("@a:example.com:D1").unwrap();
        let cid3 = take_corner_announcements()[0].0.clone();
        handle_corner_announcement("@a:example.com:D1", &cid3, "");
        assert!(near(vol(&b_mic), 0.05), "active again: B is background");
        handle_corner_announcement("@a:example.com:D1", "", "");
        assert!(corner_mine().is_empty(), "A left: my corner is over immediately");
        assert_eq!(take_corner_announcements(), vec![(String::new(), String::new())]);
        assert!(near(vol(&b_mic), 0.5) && near(vol(&a_mic), 1.0));
        CORNER_ALONE_GRACE_MS.store(4000, std::sync::atomic::Ordering::Relaxed);
        local_identity_cell().lock().unwrap().clear();
        reset_layers();
    }

    /// A peer's share audio is muted until one of their share tiles is watched;
    /// an audio-only share (no tile) is never gated; the user's mute and recording-only
    /// still win; tile teardown restores an audio-only sharer.
    #[test]
    fn share_audio_follows_watching() {
        let _g = test_lock();
        let _ = gst::init();
        reset_layers();
        let mk = |name: &str| gst::ElementFactory::make("volume").name(name).build().unwrap();
        let who = "@s:example.com:DEV";
        // Audio-only sharer first: no tile -> never gated.
        let share_audio = mk("w_s_share");
        register_track_gain(who, "screenshare", "sid-sa", &share_audio);
        assert!(!share_audio.property::<bool>("mute"), "audio-only share is never gated");
        // Their share VIDEO tile appears -> unwatched -> muted.
        note_share_tile("vid_s1", who);
        assert!(share_audio.property::<bool>("mute"), "unwatched share mutes its audio");
        // Watching it un-mutes; un-watching mutes again.
        set_tile_watched("vid_s1", true);
        assert!(!share_audio.property::<bool>("mute"));
        set_tile_watched("vid_s1", false);
        assert!(share_audio.property::<bool>("mute"));
        // A second tile of theirs watched is enough.
        note_share_tile("vid_s2", who);
        set_tile_watched("vid_s2", true);
        assert!(!share_audio.property::<bool>("mute"), "any watched tile of theirs un-mutes");
        // User mute wins while watched; clearing it restores.
        set_track_muted("@s:example.com", "screenshare", true);
        assert!(share_audio.property::<bool>("mute"));
        set_track_muted("@s:example.com", "screenshare", false);
        assert!(!share_audio.property::<bool>("mute"));
        // Recording-only wins over watched.
        set_rec_only_audio_mute(who, true);
        assert!(share_audio.property::<bool>("mute"), "recording-only beats watched");
        set_rec_only_audio_mute(who, false);
        assert!(!share_audio.property::<bool>("mute"));
        // Tiles go away -> audio-only again -> audible.
        forget_share_tile("vid_s2");
        assert!(share_audio.property::<bool>("mute"), "s1 still there and unwatched");
        forget_share_tile("vid_s1");
        assert!(!share_audio.property::<bool>("mute"), "no tiles left: never gated");
        // A late share-audio track of an unwatched sharer starts muted.
        note_share_tile("vid_s3", who);
        let late = mk("w_s_share_late");
        register_track_gain(who, "screenshare", "sid-sa2", &late);
        assert!(late.property::<bool>("mute"), "late share audio of an unwatched sharer starts muted");
        assert_eq!(watched_tiles_json(), "{}");
        reset_layers();
    }

    /// A "recording only" share is hidden + muted for VIEWERS, but the Memories
    /// recorder (RECORD_DIR set) is who the flag is FOR — the same event must leave the
    /// recorder's subscription enabled and unmuted, or the Memories file comes out empty.
    #[test]
    fn recording_only_never_pauses_or_mutes_the_recorder() {
        let _g = test_lock();
        forget_all_rec_only();
        recv_policy().lock().unwrap().hidden.clear();
        let id = "vid_TR_test_recOnly";
        let who = "@sharer:example.com:DEVX";

        // As the recorder.
        set_record_dir(Some("/tmp/tinhat-test-vod".into()));
        apply_rec_only(id, who, true);
        assert!(
            !recv_policy().lock().unwrap().hidden.contains(id),
            "the recorder must not SFU-pause a recording-only share"
        );
        assert!(
            !rec_only_muted().lock().unwrap().contains(who),
            "the recorder must not mute a recording-only share"
        );
        assert!(rec_only_tiles().lock().unwrap().is_empty(), "no viewer bookkeeping in the recorder");
        set_record_dir(None);
        forget_all_rec_only();

        // As a viewer: the very same event hides + mutes until tuned in.
        apply_rec_only(id, who, true);
        assert!(recv_policy().lock().unwrap().hidden.contains(id), "viewers hide it");
        assert!(rec_only_muted().lock().unwrap().contains(who), "viewers mute it");
        set_tile_tuned_in(id, true);
        assert!(!recv_policy().lock().unwrap().hidden.contains(id), "tune-in shows it");
        assert!(!rec_only_muted().lock().unwrap().contains(who), "tune-in unmutes it");

        forget_all_rec_only();
        recv_policy().lock().unwrap().hidden.clear();
    }

    /// Deafen has to cover people who arrive AFTER it is pressed, and it must not eat a
    /// mute the user set themselves. Both are real gst `volume` elements driven through
    /// the real registration path — asserting the property that actually silences audio,
    /// not that some flag was set.
    #[test]
    fn deafen_covers_late_subscribers_and_spares_user_mutes() {
        let _g = test_lock();
        let _ = gst::init();
        // Isolate from any other test in this binary.
        track_gains().lock().unwrap().clear();
        away_muted_names().lock().unwrap().clear();
        desired_track_prefs().lock().unwrap().clear();
        *away_active().lock().unwrap() = false;
        *deaf_active().lock().unwrap() = false;

        let mk = |name: &str| gst::ElementFactory::make("volume").name(name).build().unwrap();

        // Someone is already in the call when we deafen.
        let early = mk("gain_early");
        register_track_gain("@early:example.com:DEV1", "mic", "sid-early", &early);
        set_deafened(true);
        assert!(early.property::<bool>("mute"), "deafen must mute the tracks already present");

        // Someone joins WHILE we are deafened — the case that was broken.
        let late = mk("gain_late");
        register_track_gain("@late:example.com:DEV2", "mic", "sid-late", &late);
        assert!(
            late.property::<bool>("mute"),
            "a track subscribed while deafened must arrive muted"
        );

        // Undeafen restores both.
        set_deafened(false);
        assert!(!early.property::<bool>("mute"), "undeafen must restore the early track");
        assert!(!late.property::<bool>("mute"), "undeafen must restore the late track");

        // A mute the USER set must survive a deafen/undeafen cycle — undeafening is not
        // permission to un-mute someone they silenced on purpose.
        set_track_muted("@early:example.com", "mic", true);
        assert!(early.property::<bool>("mute"));
        set_deafened(true);
        set_deafened(false);
        assert!(
            early.property::<bool>("mute"),
            "undeafen must NOT clear a per-user mute the user set"
        );
        assert!(!late.property::<bool>("mute"), "the untouched track is still restored");

        // Leaving the call must not carry deafen into the next one.
        set_track_muted("@early:example.com", "mic", false);
        set_deafened(true);
        *deaf_active().lock().unwrap() = false; // what CallSession teardown does
        away_muted_names().lock().unwrap().clear();
        track_gains().lock().unwrap().clear();
        let next_call = mk("gain_next");
        register_track_gain("@next:example.com:DEV3", "mic", "sid-next", &next_call);
        assert!(
            !next_call.property::<bool>("mute"),
            "a fresh call must start audible — deafen is per-call"
        );
    }

    /// The mic feeder's skew resampler must (a) be bit-exact at ratio 1.0 (steady
    /// state costs nothing), (b) conserve rate at a skewed ratio — consumed input ≈
    /// output × ratio over many frames — and (c) never produce a sample-to-sample step
    /// larger than the source signal's own slew (no splice artifacts by construction).
    #[test]
    fn skew_resampler_conserves_rate_and_continuity() {
        const FRAME: usize = 480;
        // 200ppm-slow "device": the servo would settle near ratio 0.9998.
        for &ratio in &[1.0f64, 0.9998, 1.0002, 1.002, 0.998] {
            let mut rs = SkewResampler::new();
            let mut src: std::collections::VecDeque<i16> = std::collections::VecDeque::new();
            let mut phase: f32 = 0.0;
            let mut produced: u64 = 0;
            let mut consumed: u64 = 0;
            let mut prev: i16 = 0;
            let mut max_step = 0i32;
            for _ in 0..1000 {
                // top the queue up with a 440Hz sine (max slew ~1900/sample at 30k amp)
                while src.len() < rs.need(FRAME, ratio) + FRAME {
                    phase += 2.0 * std::f32::consts::PI * 440.0 / 48_000.0;
                    src.push_back((phase.sin() * 30000.0) as i16);
                }
                let mut out = [0i16; FRAME];
                let used = rs.resample(&src, ratio, &mut out);
                let _ = src.drain(..used);
                consumed += used as u64;
                produced += FRAME as u64;
                for &s in &out {
                    max_step = max_step.max((s as i32 - prev as i32).abs());
                    prev = s;
                }
            }
            let expect = produced as f64 * ratio;
            let err = (consumed as f64 - expect).abs();
            assert!(err < 2.0, "ratio {ratio}: consumed {consumed} vs expected {expect:.1}");
            // 440Hz @30k amplitude slews ≤ ~1730/sample; interpolation can't exceed it.
            assert!(max_step < 2000, "ratio {ratio}: discontinuity step {max_step}");
        }
        // Bit-exactness at unity from a clean phase.
        let mut rs = SkewResampler::new();
        let src: std::collections::VecDeque<i16> = (0..960).map(|i| (i * 13 % 251) as i16).collect();
        let mut out = [0i16; 480];
        let used = rs.resample(&src, 1.0, &mut out);
        assert_eq!(used, 480);
        for (n, &s) in out.iter().enumerate() {
            assert_eq!(s, src[n]);
        }
    }

    /// smooth_seam must remove the step at the splice point (first sample lands next to
    /// prev_last, not at the raw frame value) and leave the frame's tail untouched.
    #[test]
    fn smooth_seam_removes_step_and_preserves_tail() {
        let mut frame = vec![-8000i16; 480];
        smooth_seam(&mut frame, 8000);
        // First sample pulled almost all the way to prev_last...
        assert!((frame[0] - 8000).abs() < 400, "first sample {}", frame[0]);
        // ...monotonic decay across the ramp...
        for w in frame.windows(2).take(95) {
            assert!(w[1] <= w[0]);
        }
        // ...and the tail is untouched.
        assert!(frame[96..].iter().all(|&s| s == -8000));
        // No-op when already continuous.
        let mut same = vec![123i16; 480];
        smooth_seam(&mut same, 123);
        assert!(same.iter().all(|&s| s == 123));
    }

    /// The peer-key registry's transport rules — the heart of the join/leave coin-flip
    /// fix. To-device is a live push (arrival order wins); room events are history
    /// reads, ordered ONLY against the highest room ts already processed for the slot,
    /// and they yield to a to-device push that just served it. One test fn on purpose:
    /// the registry is process-global state.
    #[test]
    fn peer_key_registry_transport_rules() {
        let id = "@keytest:unit.test:DEVX";
        let clear = || {
            pending_peer_keys().lock().unwrap().retain(|(i, _), _| !i.starts_with("@keytest:"));
        };
        clear();
        let k1 = [0x11u8; 16];
        let k2 = [0x22u8; 16];
        let k3 = [0x33u8; 16];
        let t0: u64 = 1_700_000_000_000;

        // ── Room-transport ordering: a poll pass must END on the NEWEST event, no
        // matter which order the (backward-paginated) history hands them to us, and a
        // re-read of the same history must be completely inert.
        assert!(install_peer_key_at(id, 3, k2, t0 + 2_000, KeySrc::Room)); // newest first
        assert!(!install_peer_key_at(id, 3, k1, t0, KeySrc::Room)); // older: refused…
        assert!(!install_peer_key_at(id, 3, k1, t0 + 1_999, KeySrc::Room)); // …even 1ms older
        assert!(install_peer_key_at(id, 3, k2, t0 + 2_000, KeySrc::Room)); // same bytes: ok
        assert_eq!(pending_peer_keys().lock().unwrap()[&(id.to_string(), 3)].key, k2);
        // A genuinely NEW room event (higher server ts) still wins — a web peer that
        // fell back to room-only key delivery must be able to re-key us.
        assert!(install_peer_key_at(id, 3, k3, t0 + 9_000, KeySrc::Room));
        assert_eq!(pending_peer_keys().lock().unwrap()[&(id.to_string(), 3)].key, k3);

        // ── To-device authority: a live push wins, and history CANNOT stomp it —
        // not with an old ts, and not with a "new" one (the exact rejoin bug).
        clear();
        assert!(install_peer_key(id, 4, k2)); // to-device push (the rejoin's fresh key)
        assert!(!install_peer_key_at(id, 4, k1, t0, KeySrc::Room)); // stale history
        assert!(!install_peer_key_at(id, 4, k1, t0 + 60_000, KeySrc::Room)); // "newer" history
        assert_eq!(pending_peer_keys().lock().unwrap()[&(id.to_string(), 4)].key, k2);
        // …and once refused, that event can't come back when the trust window lapses.
        {
            let mut g = pending_peer_keys().lock().unwrap();
            let e = g.get_mut(&(id.to_string(), 4)).unwrap();
            e.installed_at = std::time::Instant::now()
                - std::time::Duration::from_millis(TODEVICE_TRUST_MS + 1_000);
        }
        assert!(!install_peer_key_at(id, 4, k1, t0 + 60_000, KeySrc::Room)); // already seen
        assert_eq!(pending_peer_keys().lock().unwrap()[&(id.to_string(), 4)].key, k2);
        // A later to-device push always wins (arrival order = truth).
        assert!(install_peer_key(id, 4, k3));
        assert_eq!(pending_peer_keys().lock().unwrap()[&(id.to_string(), 4)].key, k3);

        // Index 255 would read past the 255-slot cryptor ring.
        assert!(!install_peer_key(id, 255, k1));
        clear();
    }

    /// The exact downstream chain `build_playback` inserts (appsrc + caps swapped for an
    /// `audiotestsrc` so the head is launchable headlessly) must PARSE + LINK, the named
    /// `volume` element must resolve via `by_name` (matching the real code path), and its
    /// props must default to UNITY (volume=1.0, mute=false) so call audio stays
    /// byte-identical until a user moves a slider.
    #[test]
    fn build_playback_chain_parses_links_and_is_unity() {
        gst::init().expect("gst init");
        let gain_name = gain_element_name("@u:server:DEV", "mic");
        assert_eq!(gain_name, "gain_-u-server-DEV_mic"); // sanitized
        // Same downstream description as build_playback, with a launchable head + fakesink.
        let desc = format!(
            "audiotestsrc num-buffers=1 ! \
             audio/x-raw,format=S16LE,rate=48000,channels=2,layout=interleaved ! \
             audioconvert ! audioresample ! volume name={gain_name} volume=1.0 mute=false ! \
             queue max-size-time=200000000 max-size-buffers=64 max-size-bytes=0 leaky=downstream ! fakesink sync=false"
        );
        let pipeline = gst::parse::launch(&desc)
            .expect("playback chain must parse+link")
            .downcast::<gst::Pipeline>()
            .expect("must be a Pipeline");
        let gain = pipeline
            .by_name(&gain_name)
            .expect("named volume element must resolve via by_name");
        assert_eq!(gain.property::<f64>("volume"), 1.0, "default gain must be unity");
        assert!(!gain.property::<bool>("mute"), "default must be unmuted");
        // set_track_gain's NaN guard maps a non-finite gain back to unity.
        let g = {
            let gain = f64::NAN;
            if gain.is_finite() { gain.clamp(0.0, 2.0) } else { 1.0 }
        };
        assert_eq!(g, 1.0);
    }

    /// The Phase-3 2nd-audio-track FETCH+DECODE pipeline (`uridecodebin -> audioconvert ->
    /// audioresample -> caps S16LE/48k/stereo -> appsink`) must construct, add + LINK its tail,
    /// and expose the named `appsink`. uridecodebin's src pad is dynamic (linked at runtime),
    /// so this PARSE/LINK check covers the static tail + element wiring; the dynamic link is
    /// exercised at play time. Uses a `file://` URL (no network, no call join).
    #[test]
    fn aux_audio_pipeline_builds_links_and_has_sink() {
        gst::init().expect("gst init");
        let (pipeline, appsink) =
            build_aux_audio_pipeline("file:///tmp/tinhat-aux-test.mp3", "sound")
                .expect("aux pipeline must build + link its tail");
        // The named appsink must resolve (mirrors the real start_aux_capture by_name path).
        assert!(pipeline.by_name("aux").is_some(), "named appsink 'aux' must resolve");
        // Tail caps must request stereo S16LE 48k (aux/music tracks publish stereo).
        let caps = appsink
            .static_pad("sink")
            .and_then(|p| p.peer())
            .and_then(|p| p.current_caps().or_else(|| p.allowed_caps()));
        // current_caps is None until PLAYING; just assert the capsfilter element carries the
        // right caps so the format/rate/channels are pinned regardless of negotiation state.
        let cf = pipeline
            .iterate_elements()
            .into_iter()
            .filter_map(|e| e.ok())
            .find(|e| e.factory().map(|f| f.name() == "capsfilter").unwrap_or(false))
            .expect("capsfilter must be present");
        let want: gst::Caps = cf.property("caps");
        let s = want.structure(0).expect("capsfilter caps structure");
        assert_eq!(s.name(), "audio/x-raw");
        assert_eq!(s.get::<String>("format").as_deref(), Ok("S16LE"));
        assert_eq!(s.get::<i32>("rate"), Ok(48_000));
        assert_eq!(s.get::<i32>("channels"), Ok(2));
        let _ = caps; // negotiation-state caps are incidental; the pinned capsfilter is the contract
        let _ = pipeline.set_state(gst::State::Null);
    }

    /// #1 MUSIC source-share: the `pipewiresrc target-object=… -> audioconvert ->
    /// audioresample -> S16LE/48k/stereo -> appsink` capture string must PARSE + LINK and
    /// expose the named `aux` appsink with the stereo S16LE 48k caps. This is the
    /// gst-parse check the task requires for the new music pipewiresrc string. Uses a fake
    /// node name (parse only sets up the graph; no PipeWire connection is made until PLAYING).
    #[test]
    fn aux_pipewire_pipeline_builds_links_and_has_sink() {
        gst::init().expect("gst init");
        let (pipeline, appsink) = build_aux_pipewire_pipeline("alsa_output.test.monitor")
            .expect("aux pipewire pipeline must build + link");
        assert!(pipeline.by_name("aux").is_some(), "named appsink 'aux' must resolve");
        let cf = pipeline
            .iterate_elements()
            .into_iter()
            .filter_map(|e| e.ok())
            .find(|e| e.factory().map(|f| f.name() == "capsfilter").unwrap_or(false))
            .expect("capsfilter must be present");
        let want: gst::Caps = cf.property("caps");
        let s = want.structure(0).expect("capsfilter caps structure");
        assert_eq!(s.name(), "audio/x-raw");
        assert_eq!(s.get::<String>("format").as_deref(), Ok("S16LE"));
        assert_eq!(s.get::<i32>("rate"), Ok(48_000));
        assert_eq!(s.get::<i32>("channels"), Ok(2));
        // The source node must land in the pipewiresrc target-object (the whole point).
        let src = pipeline
            .iterate_elements()
            .into_iter()
            .filter_map(|e| e.ok())
            .find(|e| e.factory().map(|f| f.name() == "pipewiresrc").unwrap_or(false))
            .expect("pipewiresrc must be present");
        assert_eq!(src.property::<String>("target-object"), "alsa_output.test.monitor");
        let _ = appsink;
        let _ = pipeline.set_state(gst::State::Null);
    }
    // ── 10.5.2: the echo reference follows where the call plays ──────────────────────────────

    const PACTL_INPUTS: &str = "Sink Input #151\n\tDriver: PipeWire\n\tOwner Module: n/a\n\tClient: 150\n\tSink: 14925\n\
\tProperties:\n\t\tmedia.name = \"tinhat\"\n\t\ttinhat.role = \"call-playback\"\n\t\ttinhat.owner = \"77-0000abcd\"\n\
Sink Input #205\n\tDriver: PipeWire\n\tSink: 14935\n\tProperties:\n\t\ttinhat.owner = \"77-0000abcd\"\n\t\ttinhat.role = \"call-playback\"\n\
Sink Input #300\n\tSink: 14925\n\tProperties:\n\t\ttinhat.role = \"call-playback\"\n\t\ttinhat.owner = \"78-0000ffff\"\n\
Sink Input #301\n\tSink: 8361\n\tProperties:\n\t\tapplication.name = \"pw-play\"\n\
Sink Input #302\n\tSink: 14925\n\tProperties:\n\t\ttinhat.role = \"call-playback\"\n\t\ttinhat.owner = \"77-0000abcd\"\n";

    /// Only THIS process's tagged streams of the asked role count; devices deduplicated, in order.
    #[test]
    fn tagged_streams_parse_owner_and_role() {
        assert_eq!(parse_tagged_streams(PACTL_INPUTS, "Sink Input", "Sink", "77-0000abcd", "call-playback"), vec![14925, 14935]);
        assert_eq!(parse_tagged_streams(PACTL_INPUTS, "Sink Input", "Sink", "78-0000ffff", "call-playback"), vec![14925]);
        assert!(parse_tagged_streams(PACTL_INPUTS, "Sink Input", "Sink", "77-0000abcd", "ec-reference").is_empty());
        assert!(parse_tagged_streams("", "Sink Input", "Sink", "77-0000abcd", "call-playback").is_empty());
        let outs = "Source Output #9\n\tSource: 14925\n\tProperties:\n\t\ttinhat.role = \"ec-reference\"\n\t\ttinhat.owner = \"77-0000abcd\"\n";
        assert_eq!(parse_tagged_streams(outs, "Source Output", "Source", "77-0000abcd", "ec-reference"), vec![14925]);
        let short = "2239\tbenchsink.monitor\tPipeWire\tfloat32le 1ch 48000Hz\tRUNNING\n14925\theareoutB\tPipeWire\tx\tIDLE\nbad line\n";
        let m = parse_short_list(short);
        assert_eq!(m.get(&14925).map(String::as_str), Some("heareoutB"));
        assert_eq!(m.get(&2239).map(String::as_str), Some("benchsink.monitor"));
        assert_eq!(m.len(), 2);
    }

    fn no_headset(_: &str) -> Option<String> {
        None
    }
    fn hs(o: &str) -> Option<String> {
        if o.contains("headset") { Some(format!("{o} reports as headset")) } else { None }
    }
    fn outs_of(p: &EcPlan) -> Vec<String> {
        match p {
            EcPlan::Engage { outs, .. } => outs.clone(),
            EcPlan::Skip(_) => vec!["<skip>".into()],
        }
    }

    /// The automatic reference is where the call ACTUALLY plays — not the app's pick, not the default.
    #[test]
    fn ec_plan_follows_actual_playback() {
        let v = |x: &[&str]| x.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        // left on default, but the server routes the call to the speakers (remembered applet route)
        let p = plan_ec_reference(&[], None, &v(&["speakers"]), Some("hdmi"), &no_headset);
        assert_eq!(p, EcPlan::Engage { outs: v(&["speakers"]), explicit: false, why: "where the call plays" });
        // picked in the app, then moved in the applet mid-call: the move wins
        assert_eq!(outs_of(&plan_ec_reference(&[], Some("picked"), &v(&["moved"]), Some("d"), &no_headset)), v(&["moved"]));
        // participants on two outputs: both are referenced, deduplicated, in a canonical order, capped
        assert_eq!(outs_of(&plan_ec_reference(&[], None, &v(&["a", "b", "a"]), None, &no_headset)), v(&["a", "b"]));
        assert_eq!(outs_of(&plan_ec_reference(&[], None, &v(&["b", "a"]), None, &no_headset)), v(&["a", "b"]), "listing order is not a change");
        assert_eq!(outs_of(&plan_ec_reference(&[], None, &v(&["a", "b", "c", "d", "e"]), None, &no_headset)).len(), EC_MAX_AUTO_OUTPUTS);
        // nothing playing yet: the pick, else the default BY NAME, else the unnamed default
        assert_eq!(outs_of(&plan_ec_reference(&[], Some("picked"), &[], Some("d"), &no_headset)), v(&["picked"]));
        assert_eq!(outs_of(&plan_ec_reference(&[], None, &[], Some("d"), &no_headset)), v(&["d"]));
        assert_eq!(outs_of(&plan_ec_reference(&[], None, &[], None, &no_headset)), v(&[""]));
        assert_eq!(outs_of(&plan_ec_reference(&[], Some(""), &[], Some(""), &no_headset)), v(&[""]));
        // the user's ticked Speaker outputs are never skipped — and the call's own output joins them
        let p = plan_ec_reference(&v(&["headset-x", "game"]), None, &v(&["chat"]), Some("d"), &hs);
        assert_eq!(p, EcPlan::Engage { outs: v(&["headset-x", "game", "chat"]), explicit: true, why: EC_WHY_PICKED_PLUS_CALL });
    }

    /// The skew-underflow signature (verifier 2026-09-24): the Flatpak's reference started 2^32 × 10 ms ahead
    /// (`fifo=o=-42949673000ms`); real offsets (tens of ms, a restarted pipeline) never trip it.
    #[test]
    fn reference_wrap_signature() {
        let first = 12_345_000_000u64; // the mic's newest block, 12.3 s of running time
        assert!(ec_ref_is_wrapped(first + 42_949_672_960_000_000, first), "2^32 segments of 10 ms ahead");
        assert!(ec_ref_is_wrapped(first + 42_949_672_930_000_000, first), "minus a few segments");
        assert!(!ec_ref_is_wrapped(first + 30_000_000, first), "30 ms ahead: a normal pairing");
        assert!(!ec_ref_is_wrapped(first - 20_000_000, first), "behind: never");
        assert!(!ec_ref_is_wrapped(first + 5_000_000_000, first), "5 s (the FIFO's other-domain guard): not this");
        assert!(!ec_ref_is_wrapped(u64::MAX, u64::MAX), "no overflow at the top");
        // a healthy call hours long: the reference and the newest capture advance together (Astra 2026-09-24)
        let ten_hours = 36_000_000_000_000u64;
        assert!(!ec_ref_is_wrapped(ten_hours + 30_000_000, ten_hours), "10 h in: still in step");
        // generations: a retired build's wrap is not the running build's
        use std::sync::atomic::Ordering;
        let g = EC_FIFO_GEN.fetch_add(1, Ordering::Relaxed) + 1;
        EC_REF_WRAPPED_GEN.store(g, Ordering::Relaxed);
        assert!(ec_ref_wrapped_now(), "the running build wrapped");
        EC_FIFO_GEN.fetch_add(1, Ordering::Relaxed); // a new build attaches
        assert!(!ec_ref_wrapped_now(), "the old build's flag does not carry over");
        EC_REF_WRAPPED_GEN.store(0, Ordering::Relaxed);
    }

    /// 10.5.3: the underflow-triggered mic rebuild is opt-in. Default OFF (the 10.5.2 sandbox gate: the rebuild gave
    /// listeners 15–40 s of lag jumps); `TINHAT_EC_UNDERFLOW_REBUILD=1` turns it on; the detector's line is always logged.
    #[test]
    fn underflow_rebuild_is_opt_in() {
        assert!(!ec_underflow_rebuild_opt_in(None), "unset: off (the default)");
        assert!(!ec_underflow_rebuild_opt_in(Some("")), "empty: off");
        assert!(!ec_underflow_rebuild_opt_in(Some("0")), "0: off");
        assert!(!ec_underflow_rebuild_opt_in(Some("true")), "only 1 turns it on");
        assert!(!ec_underflow_rebuild_opt_in(Some(" 1")), "only exactly 1");
        assert!(ec_underflow_rebuild_opt_in(Some("1")), "1: on");
        // the healer, for a bound reference that wrapped: logged only by default, rebuilt only when opted in
        assert_eq!(ec_wrap_rebuild_reason(true, true, false), None, "default: no rebuild");
        assert!(ec_wrap_rebuild_reason(true, true, true).is_some(), "opted in: rebuild (the 10.5.2 behaviour)");
        assert_eq!(ec_wrap_rebuild_reason(true, false, true), None, "no wrap: nothing");
        assert_eq!(ec_wrap_rebuild_reason(false, true, true), None, "not engaged: nothing");
        // the detector's line stays greppable either way
        let off = ec_wrap_log_line(497, false);
        let on = ec_wrap_log_line(497, true);
        for l in [&off, &on] {
            assert!(l.starts_with("[aec] the echo reference started 497 days ahead of the mic"), "{l}");
        }
        assert!(off.ends_with("not rebuilding (TINHAT_EC_UNDERFLOW_REBUILD=1 turns the rebuild on)"), "{off}");
        assert!(on.ends_with("; rebuilding"), "{on}");
        // this process: the default, unless whoever runs the tests opted in
        if std::env::var_os("TINHAT_EC_UNDERFLOW_REBUILD").is_none() {
            assert!(!ec_underflow_rebuild_enabled(), "the process default is off");
        }
    }

    /// A ticked list never leaves the call's own output out (verifier 2026-09-24, rig "tickelse").
    #[test]
    fn ec_plan_ticked_list_keeps_the_call_output() {
        let v = |x: &[&str]| x.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let picked = |o: &[&str]| EcPlan::Engage { outs: v(o), explicit: true, why: EC_WHY_PICKED };
        let plus = |o: &[&str]| EcPlan::Engage { outs: v(o), explicit: true, why: EC_WHY_PICKED_PLUS_CALL };
        // the call plays on a ticked output → exactly the ticked list (no rebuild for a mixer setup)
        assert_eq!(plan_ec_reference(&v(&["game", "chat"]), None, &v(&["chat"]), Some("chat"), &no_headset), picked(&["game", "chat"]));
        // ticked while the call played elsewhere, then the server moved it → the call's output is added
        assert_eq!(plan_ec_reference(&v(&["hearsys"]), None, &v(&["heareoutB"]), Some("hearsys"), &no_headset), plus(&["hearsys", "heareoutB"]));
        // the call on a headset → nothing added (no echo path); the ticked outputs stay
        assert_eq!(plan_ec_reference(&v(&["game"]), None, &v(&["usb-headset"]), Some("game"), &hs), picked(&["game"]));
        // nothing plays yet: exactly the ticked list (no pick/default guess → no rebuild once the call plays there)
        assert_eq!(plan_ec_reference(&v(&["game"]), Some("spk"), &[], Some("d"), &no_headset), picked(&["game"]));
        assert_eq!(plan_ec_reference(&v(&["game"]), None, &[], Some("d"), &no_headset), picked(&["game"]));
        assert_eq!(plan_ec_reference(&v(&["game"]), None, &[], None, &no_headset), picked(&["game"]));
        // The ticked-outputs workaround (rig "ticked"): ticked = where the call ends up playing, default elsewhere → the same
        // plan before and after the call starts playing (the first build caught a rebuild here)
        assert_eq!(plan_ec_reference(&v(&["heareoutB"]), None, &[], Some("hearsys"), &no_headset), picked(&["heareoutB"]));
        assert_eq!(plan_ec_reference(&v(&["heareoutB"]), None, &v(&["heareoutB"]), Some("hearsys"), &no_headset), picked(&["heareoutB"]));
        // several call outputs: sorted, deduplicated, ticked ones not repeated
        assert_eq!(plan_ec_reference(&v(&["b"]), None, &v(&["c", "b", "a", "c"]), None, &no_headset), plus(&["b", "a", "c"]));
    }

    /// Headphones: skipped only when EVERY automatic output is a headset; speakers next to one stay.
    #[test]
    fn ec_plan_headset_rules() {
        let v = |x: &[&str]| x.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        // default output is a headset at join → skip; the call moves to speakers → engage
        assert!(matches!(plan_ec_reference(&[], None, &[], Some("usb-headset"), &hs), EcPlan::Skip(r) if r.contains("usb-headset")));
        assert_eq!(outs_of(&plan_ec_reference(&[], None, &v(&["speakers"]), Some("usb-headset"), &hs)), v(&["speakers"]));
        // the call plays on a headset although the default is speakers → skip
        assert!(matches!(plan_ec_reference(&[], None, &v(&["usb-headset"]), Some("speakers"), &hs), EcPlan::Skip(_)));
        // one participant on the headset, one on speakers → the speakers are referenced
        assert_eq!(outs_of(&plan_ec_reference(&[], None, &v(&["usb-headset", "speakers"]), None, &hs)), v(&["speakers"]));
        // headsets never use up the cap (Astra): four headsets + one speaker output → the speaker
        let many = v(&["headset-1", "headset-2", "headset-3", "headset-4", "zspeakers"]);
        assert_eq!(outs_of(&plan_ec_reference(&[], None, &many, None, &hs)), v(&["zspeakers"]));
    }

    /// The rebind gate: the same mismatch must persist ≥ settle, fires once, then waits min_gap.
    #[test]
    fn rebind_gate_debounces_and_rate_limits() {
        use std::time::{Duration, Instant};
        let (settle, gap) = (Duration::from_secs(2), Duration::from_secs(10));
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let mut g = RebindGate::default();
        assert!(!g.step(at(0), None, settle, gap));
        assert!(!g.step(at(0), Some("x".into()), settle, gap), "first sight only arms");
        assert!(!g.step(at(1), Some("x".into()), settle, gap), "not settled yet");
        assert!(g.step(at(3), Some("x".into()), settle, gap), "settled → fire");
        // a new mismatch right after a rebind waits for the gap
        assert!(!g.step(at(4), Some("y".into()), settle, gap));
        assert!(!g.step(at(7), Some("y".into()), settle, gap), "settled but inside min_gap");
        assert!(g.step(at(13), Some("y".into()), settle, gap), "after the gap");
        // a mismatch that flips before settling never fires; clearing resets
        assert!(!g.step(at(30), Some("a".into()), settle, gap));
        assert!(!g.step(at(33), Some("b".into()), settle, gap), "changed key re-arms");
        assert!(!g.step(at(34), None, settle, gap));
        assert!(!g.step(at(40), Some("b".into()), settle, gap), "cleared → arms again");
        assert!(g.step(at(42), Some("b".into()), settle, gap));
        // a routing that keeps moving: at most REBIND_BURST rebuilds per window, then it waits
        let mut g = RebindGate::default();
        let mut fired: Vec<u64> = Vec::new();
        for i in 0..200u64 {
            let key = format!("k{}", i / 2); // a new mismatch every 2 looks, settled on the second
            let t = 200 + i * 3;
            if g.step(at(t), Some(key), settle, gap) {
                fired.push(t);
            }
        }
        assert!(fired.len() > REBIND_BURST, "it keeps working after a burst: {fired:?}");
        for w in fired.windows(REBIND_BURST + 1) {
            assert!(w[REBIND_BURST] - w[0] >= REBIND_BURST_WINDOW.as_secs(), "more than {REBIND_BURST} rebuilds in 2 min: {w:?}");
        }
        assert_eq!(&fired[..REBIND_BURST], &[203, 215, 227, 239], "the first burst: every 12 s (gap + settle)");
    }

    fn structure_str(el: &gst::Element, field: &str) -> Option<(String, glib::Type)> {
        let s: Option<gst::Structure> = el.property("stream-properties");
        let s = s?;
        let v = s.value(field).ok()?;
        Some((v.get::<String>().unwrap_or_else(|_| format!("{v:?}")), v.type_()))
    }

    /// Drift: every bound output needs a head, and no head may sit elsewhere (Astra: coverage).
    #[test]
    fn reference_drift_needs_full_coverage() {
        let v = |x: &[&str]| x.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(reference_drift(&v(&["a", "b"]), &v(&["a.monitor", "b.monitor"])), None);
        assert!(reference_drift(&v(&["a", "b"]), &v(&["a.monitor"])).unwrap().contains("no echo reference listens to b"));
        assert!(reference_drift(&v(&["a"]), &v(&["a.monitor", "c.monitor"])).unwrap().contains("drifted to c.monitor"));
        assert!(reference_drift(&v(&["a"]), &[]).is_some(), "bound but no head at all");
    }

    /// The echo-reference head's markers reach the audio server as STRINGS ("false", the tag) — a
    /// GStreamer boolean reaches WirePlumber as "FALSE" (ignored) and is dropped by gstpulse.
    /// Linux elements only (Windows builds wasapi2 heads, which have no stream properties).
    #[cfg(not(windows))]
    #[test]
    fn reference_head_markers_are_strings_and_tagged() {
        gst::init().expect("gst init");
        let desc = format!("{} ! fakesink", tap_head("heareoutB", "tinhateccheck", ROLE_REFERENCE));
        let p = gst::parse::launch(&desc).expect("reference head parses");
        let bin = p.downcast::<gst::Bin>().unwrap();
        let el = bin.by_name("tinhateccheck").expect("named head");
        assert_eq!(structure_str(&el, "state.restore-target"), Some(("false".to_string(), glib::Type::STRING)));
        assert_eq!(structure_str(&el, "state.restore-props"), Some(("false".to_string(), glib::Type::STRING)));
        assert_eq!(structure_str(&el, "tinhat.role"), Some((ROLE_REFERENCE.to_string(), glib::Type::STRING)));
        assert_eq!(structure_str(&el, "tinhat.owner"), Some((tinhat_owner().to_string(), glib::Type::STRING)));
        let _ = bin.set_state(gst::State::Null);
        // playback sinks carry the tag on both APIs (gstpulse maps string fields only)
        for use_pulse in [false, true] {
            let frag = audio_sink_fragment_for("", use_pulse);
            let p = gst::parse::launch(&format!("audiotestsrc num-buffers=1 ! {frag}")).expect("sink parses");
            let bin = p.downcast::<gst::Bin>().unwrap();
            let out = bin.by_name("out").expect("named sink");
            assert_eq!(structure_str(&out, "tinhat.role"), Some((ROLE_PLAYBACK.to_string(), glib::Type::STRING)), "pulse={use_pulse}");
            assert_eq!(structure_str(&out, "tinhat.owner"), Some((tinhat_owner().to_string(), glib::Type::STRING)), "pulse={use_pulse}");
        }
    }

    /// 10.8 leak track: the playback bus watcher keeps going while its pipeline lives, and stops once it is freed or has
    /// sat in NULL for 10 s — a short NULL → PLAYING cycle does not end it.
    #[test]
    fn playback_bus_watch_ends_with_its_pipeline() {
        use gst::State::{Null, Paused, Playing, VoidPending};
        let mut n = 0;
        assert!(playback_bus_watch_continues(Some((Playing, VoidPending)), &mut n));
        assert!(!playback_bus_watch_continues(None, &mut n), "a freed pipeline ends the watcher");
        // a NULL cycle shorter than 10 s (self-repoint / heal) keeps it
        let mut n = 0;
        for _ in 0..10 {
            assert!(playback_bus_watch_continues(Some((Null, VoidPending)), &mut n));
        }
        assert!(playback_bus_watch_continues(Some((Null, Playing)), &mut n), "a NULL with a pending state is not settled");
        assert!(playback_bus_watch_continues(Some((Paused, Playing)), &mut n));
        assert_eq!(n, 0, "any other state resets the count");
        // 20 settled NULL polls (10 s) = a teardown
        let mut alive = true;
        for _ in 0..20 {
            alive = playback_bus_watch_continues(Some((Null, VoidPending)), &mut n);
        }
        assert!(!alive, "a pipeline left in NULL ends the watcher");
    }

    /// 10.8 leak track: a PAUSED remote-video tile (window minimized) whose feeder keeps pushing must not queue frames
    /// without bound in its appsrc (it queued every 1080p frame before — ~3 MB each).
    #[test]
    fn paused_video_tile_does_not_queue_frames_without_bound() {
        let _ = gst::init();
        // the tile's own head; the display tail (qml6glsink) cannot run headless, a fakesink stands in
        let desc = format!("{VIDEO_TILE_HEAD} ! fakesink name=sink sync=false");
        let pipeline = gst::parse::launch(&desc).unwrap().downcast::<gst::Pipeline>().unwrap();
        let src = pipeline.by_name("src").unwrap().downcast::<gstreamer_app::AppSrc>().unwrap();
        let caps = gst::Caps::builder("video/x-raw")
            .field("format", "I420")
            .field("width", 1920i32)
            .field("height", 1080i32)
            .field("framerate", gst::Fraction::new(30, 1))
            .build();
        src.set_caps(Some(&caps));
        let _ = pipeline.set_state(gst::State::Paused);
        let frame = 1920 * 1080 * 3 / 2;
        for _ in 0..120 {
            let _ = src.push_buffer(gst::Buffer::with_size(frame).unwrap());
        }
        let queued = src.property::<u64>("current-level-buffers");
        let _ = pipeline.set_state(gst::State::Null);
        assert!(queued <= 3, "a paused tile queued {queued} frames (~{} MB)", queued as usize * frame / 1_000_000);
    }
}
