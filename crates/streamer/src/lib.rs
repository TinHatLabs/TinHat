//! Tin Hat native streamer — GStreamer hardware-encoded publish core.
//!
//! Rust port of the proven spike (`spike/nvenc-publish.py`): capture → NVENC →
//! `livekitwebrtcsink` → the same Element Call LiveKit room the web/phone clients
//! use. No Chromium/Electron in the media path.
//!
//! Today this uses `videotestsrc`; swapping it for `pipewiresrc` (KDE Wayland
//! screencast portal) turns it into a real screen-share. See `build_publish_pipeline`.

use anyhow::{Context, Result};
use gstreamer as gst;
use gstreamer::glib;
use gstreamer::prelude::*;

// Screen capture: the Linux impl is the xdg-desktop-portal/PipeWire path; the Windows
// file is a same-API stub until the d3d11screencapturesrc port lands (see
// windows/PLATFORM_AUDIT.md §1 / windows/MEDIA_PIPELINE.md) — every consumer
// (lk_publish/call/app) compiles unchanged; pick_screencast() errors at runtime.
#[cfg(target_os = "linux")]
pub mod capture;
#[cfg(not(target_os = "linux"))]
#[path = "capture_windows.rs"]
pub mod capture;
pub use capture::{pick_screencast, ScreenCast};
// Windows share picker: enumerate monitors/windows for the QML dialog; the pick
// comes back through ScreenCast::from_parts. (Linux keeps the portal's own picker.)
#[cfg(not(target_os = "linux"))]
pub use capture::{list_share_sources, ShareSource};

pub mod lk_publish;
pub use lk_publish::{
    auto_rung, connection_probe, encoder_backends, nvenc_available, set_share_encoder,
    set_share_quality, share_quality_label, start_e2ee_screenshare, E2eePublish, ProbeReport,
    PROBE_CAP_BPS,
};

pub mod camera;

// Memories LIVE side tiles (0.8.13): muted video-only playback pipelines for the
// relive grid, attached via the app's video_bridge like live call tiles.
pub mod memtile;

pub mod aec;
pub mod call;
pub mod hear; // 10.5: "can they hear me?" — mic check, muted talk, receipts, echo
pub mod sameroom; // 11.3: "same room" — link up, room-mic election, in-room playback mute
pub mod callstats; // widgets (modular UI step 2): receive stats per remote track, ConnectionQuality, `tinhat.rxq`
pub use call::CallSession;
// Per-track playback gain (Phase 3): the app's AudioPrefsController drives these to
// adjust a single remote participant's level/mute. NO-OP at unity (volume=1.0).
pub use call::{set_track_gain, set_track_muted, prefer_pulse_mic, in_flatpak_sandbox};
pub use call::{prefer_native_audio, pulse_head_wanted, pw_snapshot};
// Phase-3 opt-in 2nd audio track: soundboard one-shots + #7 music stream. NO-OP when not
// in a call; never touches the mic track. Created only when a sound/music is played.
pub use call::{is_music_playing, is_sound_playing, play_url_in_call, start_music, stop_music, stop_url_in_call};
// #1 Music = a SELECTABLE + REMEMBERED PipeWire SOURCE (sink monitor / input) instead of a
// URL. Enumerate sources, persist the chosen node, start the music 2nd-track from it.
pub use call::{list_audio_sources, music_source, set_music_source, start_music_source};
// 0.8.23: share audio only while watched + game chat.
pub use call::{game_chat_json, game_chat_peer, local_game_chat, set_local_game_chat, set_tile_watched, watched_tiles_json};
// 9.4 corners ("pull aside"): receiver-side gain policy + the announcements the app publishes.
pub use call::{corner_factor, corner_join, corner_json, corner_leave, corner_mine, corner_pull, corner_tick, take_corner_announcements, take_game_chat_announce_due, CORNER_OUTSIDE_GAIN};
// RBAC (docs/RBAC.md §5.4): server-enforced call moderation — packets, my own server-mute
// state, tile enrichment.
pub use call::{i_am_server_muted, my_source_allowed, publish_moderation_notice, publish_moderation_packet, resolve_call_participant, send_moderation_packet, server_mute_info, server_muted_json, set_server_muted_from_state, MODERATION_TOPIC};
// 0.8.22: recording-only share + viewer tune-in.
pub use call::{rec_only_flags_json, set_share_recording_only, set_tile_tuned_in, share_recording_only, tile_recording_state};
// #4 AFK/BRB incoming-audio cut: mute ALL remote audio while away (reversible, won't stomp
// a #5 per-user mute). NO-OP unless the user goes AFK.
pub use call::{away_privacy_active, set_away_privacy};
// Deafen: mute ALL incoming call audio (output). Shares the reversible auto-mute with AFK
// (effective = afk || deafened) so it composes with per-user mutes and AFK cleanly.
pub use call::{deafened_active, set_deafened};

/// Echo cancellation health: aec_degraded is true when the user wants EC but the module
/// is not up (the toggle reads ON and there is no canceller — invisible to them, obvious to
/// everyone else in the call). aec_retry_if_degraded brings it back; the app calls it on a
/// throttle from the in-call tick so one transient pactl failure no longer costs echo
/// cancellation for the rest of the call.
pub use call::{aec_degraded, aec_retry_if_degraded};
/// 0.8.31: one human line on what echo cancellation is doing right now (measured).
pub use call::aec_status_note;
/// Is RNNoise installed? False = the noise-cancel toggle cannot do anything, and the
/// UI must say so rather than offer a control that silently does nothing.
pub use call::noise_cancel_available;
// #7 RNNoise mic noise-cancellation toggle (DEFAULT OFF, persisted). When OFF the mic
// capture pipeline is byte-identical to before; flip via the call menu, then restart_mic.
pub use call::{mic_denoise, set_mic_denoise};
// libwebrtc APM mic toggles — AGC + noise-suppression, SEPARATE + independently persisted
// (AGC default ON, NS default OFF; applied on mic (re)publish via CallSession::republish_mic).
pub use call::{mic_agc, mic_noise_suppression, set_mic_agc, set_mic_noise_suppression};
// 0.8.25: ONE noise-suppression control (off / standard = webrtcdsp NS / strong = RNNoise),
// the echo-cancellation engine name, and the audio-device health flags the call UI shows.
pub use call::{aec_tier_name, audio_health_probe, mic_ns_mode, mic_silent, output_missing, set_mic_ns_mode, NsMode};
// Join-time "static mic" self-heal: read-and-clear the capture-unhealthy flag the mic
// feeder raises when audiorate corrects heavily right after start (see do_join's healer).
pub use call::take_mic_capture_unhealthy;
// 0.8.29: healer inputs — how stale a latch is, a never-primed rebuild, primed state.
pub use call::{mic_capture_bad_age, mic_capture_primed, take_mic_never_primed};
// Acoustic echo cancellation (PipeWire module-echo-cancel, bound to the real devices for the
// call's lifetime). Default ON — see aec.rs for why libwebrtc's own AEC cannot work here.
pub use aec::{enabled as aec_enabled, set_enabled as set_aec_enabled};
// 10.5: "can they hear me?" (hear.rs) + the healer's view of the mic we mean to have.
pub use hear::{fix_system_level as hear_fix_system_level, request_heal as hear_request_heal, state_json as hear_json};
// 11.3 same room: the app's invokables + the UI snapshot + the remembered pairs store.
pub use sameroom::{answer_ask as same_room_answer, cancel_ask as same_room_cancel, auto_link as same_room_auto, forget_pair as same_room_forget_pair, link_with as same_room_link, pairs_json as same_room_pairs_json, set_auto as same_room_set_auto, set_hear_through as same_room_hear_through, set_store_path as same_room_set_store_path, state_json as same_room_json, unlink as same_room_unlink};
pub use call::{mic_capture_down, mic_wanted};
// 0.8.33: the speaker outputs summed into the echo reference (empty = Tin Hat's own output).
pub use aec::{ref_outputs as aec_ref_outputs, set_ref_outputs as set_aec_ref_outputs};
// Abandoned-connect cleanup (do_join timeout path) — see call.rs::abandon_partial_connect.
pub use call::abandon_partial_connect;
// #B live audio levels for the per-name waveform bars. Read-only RMS getters folded from
// the existing mic + playback per-frame peaks (no `level` element, no pipeline change).
// speakers_json: the SFU's active-speaker list (server-side voice detection) for the
// voice overlay — robust where a raw RMS floor is not (AGC boosts ambient noise).
pub use call::{audio_levels_json, mic_level, speakers_json};
// Outgoing "stream health": publisher-side stats (encoder, res/fps, send bitrate, quality
// limitation, receiver-report loss/RTT) per outgoing stream, refreshed every 3s while
// screen/camera/mic tracks are live. Feeds the in-call transparency popup.
pub use call::publish_stats_json;
// Noise-cancel before/after meter: a standalone mic monitor (no call needed) feeding the
// pre- (raw) and post-denoise RMS cells. mic_level_pre is the raw mic; mic_level is post.
pub use call::{mic_level_pre, start_mic_monitor, stop_mic_monitor};
// Speaker (output sink) level meter: a standalone monitor tapping the selected sink's
// `.monitor` into output_level. Analogous to the mic monitor; no playback pipeline change.
pub use call::{output_level, start_output_monitor, stop_output_monitor};

/// Connection params for publishing into a LiveKit room.
#[derive(Clone, Debug)]
pub struct PublishConfig {
    pub ws_url: String,
    pub api_key: String,
    pub secret_key: String,
    pub room: String,
    pub identity: String,
    pub participant_name: String,
}

impl PublishConfig {
    /// Build from env (matches the spike's variables). The raw api-key/secret path
    /// is a development shortcut — the real app gets a token from `lk-jwt-service`.
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            ws_url: std::env::var("LIVEKIT_URL").context("LIVEKIT_URL not set")?,
            api_key: std::env::var("LIVEKIT_API_KEY").context("LIVEKIT_API_KEY not set")?,
            secret_key: std::env::var("LIVEKIT_API_SECRET").context("LIVEKIT_API_SECRET not set")?,
            room: std::env::var("LK_ROOM").unwrap_or_else(|_| "nvenc-spike".into()),
            identity: std::env::var("LK_IDENTITY").unwrap_or_else(|_| "nvenc-spike-pub".into()),
            participant_name: std::env::var("LK_NAME")
                .unwrap_or_else(|_| "Tin Hat Stream".into()),
        })
    }
}

/// Deterministically land on hardware NVENC: derank software H.264 encoders and
/// boost `nvh264enc`, so `webrtcsink`'s encoder discovery picks the GPU encoder.
/// (Only matters for `publish` mode, where webrtcsink chooses the encoder.)
pub fn force_hardware_h264() {
    let registry = gst::Registry::get();
    for soft in ["x264enc", "openh264enc"] {
        if let Some(f) = registry.lookup_feature(soft) {
            f.set_rank(gst::Rank::NONE);
            println!("[rank] deranked {soft}");
        }
    }
    if let Some(f) = registry.lookup_feature("nvh264enc") {
        f.set_rank(gst::Rank::PRIMARY);
        println!("[rank] boosted nvh264enc");
    }
}

/// Memories side tiles decode files through decodebin into SYSTEM memory
/// (`caps=video/x-raw`). Hardware decoders whose preferred output lives elsewhere
/// (nvh264dec → CUDAMemory, va*dec → VAMemory, d3d11/d3d12 → D3D memory) outrank the
/// software decoders, get autoplugged first, fix their caps to that memory type before
/// anything downstream is linked, and decodebin then fails the whole chain as
/// "Missing decoder: H.264 (…)" — the exact 0.8.13 field error, reproduced on the
/// build box the moment nvcodec became usable. GStreamer decoders are used by the app
/// ONLY for these tiles (calls decode in libwebrtc, the watch party in mpv), so
/// deranking every video decoder whose FIRST advertised src structure carries a
/// non-system-memory feature is safe: decodebin lands on avdec_*/openh264dec, which
/// the tiles negotiate every time. Generic on purpose — no allowlist (D3).
pub fn derank_offsystem_video_decoders() {
    let list = gst::ElementFactory::factories_with_type(
        gst::ElementFactoryType::DECODER | gst::ElementFactoryType::MEDIA_VIDEO,
        gst::Rank::MARGINAL,
    );
    for f in list.iter() {
        let mut off_system = false;
        for t in f.static_pad_templates() {
            if t.direction() != gst::PadDirection::Src {
                continue;
            }
            let caps = t.caps();
            if caps.is_any() || caps.is_empty() {
                continue;
            }
            // Preferred output = the first structure's features.
            let feats = caps.features(0);
            let system = match feats {
                None => true,
                Some(fe) => fe.is_any() || fe.contains("memory:SystemMemory") || fe.is_equal(&gst::CapsFeatures::new_empty()),
            };
            if !system {
                off_system = true;
            }
            break;
        }
        if off_system {
            f.set_rank(gst::Rank::NONE);
            println!("[rank] deranked video decoder {} (non-system-memory output)", f.name());
        }
    }
}

/// `videotestsrc -> nvh264enc -> fakesink`. Proves NVENC works in-process, no network.
pub fn build_local_smoke_pipeline() -> Result<gst::Pipeline> {
    gst::parse::launch(
        "videotestsrc is-live=true pattern=smpte ! \
         video/x-raw,format=NV12,width=1280,height=720,framerate=30/1 ! \
         nvh264enc ! h264parse ! fakesink",
    )?
    .downcast::<gst::Pipeline>()
    .map_err(|_| anyhow::anyhow!("parsed element is not a Pipeline"))
}

/// `videotestsrc -> nvh264enc -> livekitwebrtcsink` — publishes into the LiveKit room.
/// Swap `videotestsrc` for `pipewiresrc` (+ the portal fd) for real screen capture.
pub fn build_publish_pipeline(cfg: &PublishConfig) -> Result<gst::Pipeline> {
    let pipeline = gst::parse::launch(
        "videotestsrc is-live=true pattern=smpte ! \
         video/x-raw,format=NV12,width=1280,height=720,framerate=30/1 ! \
         queue ! livekitwebrtcsink name=sink video-caps=video/x-h264",
    )?
    .downcast::<gst::Pipeline>()
    .map_err(|_| anyhow::anyhow!("parsed element is not a Pipeline"))?;

    let sink = pipeline.by_name("sink").context("livekitwebrtcsink 'sink' not found")?;
    let signaller: glib::Object = sink.property("signaller");
    signaller.set_property("ws-url", &cfg.ws_url);
    signaller.set_property("api-key", &cfg.api_key);
    signaller.set_property("secret-key", &cfg.secret_key);
    signaller.set_property("room-name", &cfg.room);
    signaller.set_property("identity", &cfg.identity);
    signaller.set_property("participant-name", &cfg.participant_name);
    Ok(pipeline)
}

/// Like `build_publish_pipeline` but authenticates with a pre-minted LiveKit JWT
/// (from `lk-jwt-service`) via the signaller's `auth-token` — the real-app path
/// (no raw api-key/secret in the client). `room` must match the JWT's room grant.
pub fn build_publish_pipeline_with_token(
    ws_url: &str,
    jwt: &str,
    room: &str,
) -> Result<gst::Pipeline> {
    let pipeline = gst::parse::launch(
        "videotestsrc is-live=true pattern=smpte ! \
         video/x-raw,format=NV12,width=1280,height=720,framerate=30/1 ! \
         queue ! livekitwebrtcsink name=sink video-caps=video/x-h264",
    )?
    .downcast::<gst::Pipeline>()
    .map_err(|_| anyhow::anyhow!("parsed element is not a Pipeline"))?;

    let sink = pipeline.by_name("sink").context("livekitwebrtcsink 'sink' not found")?;
    let signaller: glib::Object = sink.property("signaller");
    signaller.set_property("ws-url", ws_url);
    signaller.set_property("auth-token", jwt);
    signaller.set_property("room-name", room);
    Ok(pipeline)
}

/// REAL screen-share: `pipewiresrc (KDE portal fd) -> NVENC -> livekitwebrtcsink`,
/// authenticated with a pre-minted lk-jwt-service JWT. `fd`/`node_id` come from
/// [`pick_screencast`]; keep the owning [`ScreenCast`] alive while this runs.
///
/// NOTE: this publishes the track with LiveKit `source = Camera` (hardcoded in
/// gst-plugin-webrtc 0.15.2). To make Element Call render it as a *screen-share*
/// tile the track must be `source = SCREEN_SHARE`, which requires a vendored
/// patched rswebrtc plugin (key the source off the track name) + building the
/// pipeline element-by-element so the video pad's `msid` can be set to "screen".
/// Deferred until the publish-path/E2EE architecture is settled.
pub fn build_screenshare_pipeline_with_token(
    fd: i32,
    node_id: u32,
    ws_url: &str,
    jwt: &str,
    room: &str,
) -> Result<gst::Pipeline> {
    let desc = format!(
        "pipewiresrc fd={fd} path={node_id} do-timestamp=true keepalive-time=1000 ! \
         videoconvert ! videoscale ! \
         video/x-raw,format=NV12 ! \
         queue max-size-buffers=4 leaky=downstream ! \
         livekitwebrtcsink name=sink video-caps=video/x-h264"
    );
    let pipeline = gst::parse::launch(&desc)?
        .downcast::<gst::Pipeline>()
        .map_err(|_| anyhow::anyhow!("parsed element is not a Pipeline"))?;

    let sink = pipeline.by_name("sink").context("livekitwebrtcsink 'sink' not found")?;
    let signaller: glib::Object = sink.property("signaller");
    signaller.set_property("ws-url", ws_url);
    signaller.set_property("auth-token", jwt);
    signaller.set_property("room-name", room);
    Ok(pipeline)
}

/// One-time GStreamer init + hardware-encoder ranking. Idempotent.
pub fn ensure_init() -> Result<()> {
    // Cache the FIRST init's outcome so EVERY caller gets the truth. The old `Once` + a local
    // `result` returned a fresh `Ok(())` on every call after the first — regardless of whether
    // gst actually initialized — so callers (call.rs, lk_publish.rs) were lied to and went on
    // to build pipelines against an uninitialized GStreamer.
    static INIT_OK: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let ok = INIT_OK.get_or_init(|| match gst::init() {
        Ok(()) => {
            force_hardware_h264();
            derank_offsystem_video_decoders();
            true
        }
        Err(e) => {
            eprintln!("[streamer] GStreamer init failed: {e}");
            false
        }
    });
    if *ok {
        Ok(())
    } else {
        Err(anyhow::anyhow!("GStreamer initialization failed (see earlier log)"))
    }
}

/// A bus event worth surfacing to the UI.
#[derive(Clone, Debug)]
pub enum PublishEvent {
    /// The pipeline errored (publish failed / encoder/portal died).
    Error(String),
    /// End-of-stream (the capture ended, e.g. the user closed the shared window).
    Eos,
}

/// A running publish. Holds the pipeline + the portal cast handle alive; stop it
/// (or drop it) to tear everything down (pipeline -> NULL, then fd/session close).
pub struct PublishHandle {
    pipeline: gst::Pipeline,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    // dropped last, AFTER the pipeline is NULLed, so pipewiresrc's fd stays valid
    // until capture has fully stopped.
    _screencast: Option<ScreenCast>,
}

impl PublishHandle {
    /// Stop publishing (pipeline -> NULL). Safe to call repeatedly; Drop also does it.
    pub fn stop(&self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        let _ = self.pipeline.set_state(gst::State::Null);
    }

    /// Stop the pipeline and take the portal cast out, so the caller can `await
    /// screencast.close()` (closing the portal D-Bus session). Pipeline is NULLed
    /// here first; remaining Drop is a harmless no-op.
    pub fn into_screencast(mut self) -> Option<ScreenCast> {
        self.stop();
        self._screencast.take()
    }
}

impl Drop for PublishHandle {
    fn drop(&mut self) {
        self.stop();
        // _screencast (if still present) drops here, closing only its fd. For a
        // clean portal-session close prefer into_screencast() + ScreenCast::close().
    }
}

/// Start a real screen-share publish from a portal [`ScreenCast`] and a minted
/// lk-jwt token. Sets the pipeline PLAYING and spawns a bus-watch thread that
/// calls `on_event` on error/EOS. Returns a handle that keeps it all alive.
pub fn start_screenshare_publish(
    screencast: ScreenCast,
    ws_url: &str,
    jwt: &str,
    room: &str,
    on_event: impl Fn(PublishEvent) + Send + 'static,
) -> Result<PublishHandle> {
    ensure_init()?;
    let pipeline =
        build_screenshare_pipeline_with_token(screencast.raw_fd(), screencast.node_id(), ws_url, jwt, room)?;

    pipeline
        .set_state(gst::State::Playing)
        .context("failed to start the screen-share pipeline")?;

    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    spawn_bus_watch(&pipeline, stop.clone(), on_event);

    Ok(PublishHandle {
        pipeline,
        stop,
        _screencast: Some(screencast),
    })
}

/// Watch a pipeline's bus on a dedicated thread; report errors/EOS via `on_event`
/// until the `stop` flag is set (PublishHandle::stop/drop) or the stream ends.
fn spawn_bus_watch(
    pipeline: &gst::Pipeline,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    on_event: impl Fn(PublishEvent) + Send + 'static,
) {
    let Some(bus) = pipeline.bus() else { return };
    std::thread::spawn(move || {
        use gst::MessageView;
        use std::sync::atomic::Ordering;
        while !stop.load(Ordering::SeqCst) {
            let Some(msg) = bus.timed_pop(gst::ClockTime::from_mseconds(250)) else {
                continue;
            };
            match msg.view() {
                MessageView::Error(err) => {
                    on_event(PublishEvent::Error(format!(
                        "{} ({:?})",
                        err.error(),
                        err.debug()
                    )));
                    break;
                }
                MessageView::Eos(_) => {
                    on_event(PublishEvent::Eos);
                    break;
                }
                _ => {}
            }
        }
    });
}

/// Play a pipeline for `secs` seconds, surfacing bus errors. Always returns it to NULL.
pub fn run_for(pipeline: &gst::Pipeline, secs: u64) -> Result<()> {
    pipeline.set_state(gst::State::Playing)?;
    let bus = pipeline.bus().context("pipeline has no bus")?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);

    let result = (|| -> Result<()> {
        while std::time::Instant::now() < deadline {
            if let Some(msg) = bus.timed_pop(gst::ClockTime::from_mseconds(200)) {
                use gst::MessageView;
                match msg.view() {
                    MessageView::Error(err) => {
                        anyhow::bail!("pipeline error: {} ({:?})", err.error(), err.debug());
                    }
                    MessageView::Eos(_) => break,
                    _ => {}
                }
            }
        }
        Ok(())
    })();

    pipeline.set_state(gst::State::Null)?;
    result
}
