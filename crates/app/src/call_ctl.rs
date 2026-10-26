//! CallController — full call participation from QML over ONE LiveKit Room
//! (`CallSession`): join/leave, mic mute, screen-share. Mirrors the proven
//! screen-share orchestration (`stream::do_start_share`) but joins as a real
//! participant (mic + remote audio) and folds the screen-share into the same Room.

use core::pin::Pin;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use cxx_qt::Threading;
use cxx_qt_lib::QString;
use tinhat_streamer::call::CallEvent;
use tinhat_streamer::CallSession;
use matrix_sdk::ruma::events::AnyToDeviceEvent;
use matrix_sdk::ruma::serde::Raw;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::Mutex;

#[cxx_qt::bridge]
pub mod qobject {
    extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[auto_cxx_name] // snake_case Rust -> camelCase QML (joinCall, leaveCall, micMuted…)
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(bool, in_call)] //    QML: inCall
        #[qproperty(bool, connecting)] // QML: connecting
        #[qproperty(bool, sharing)] //    QML: sharing — screen-share live
        #[qproperty(bool, push_to_talk)] //  QML: pushToTalk — hold-to-speak instead of toggle-mute
        #[qproperty(bool, ptt_speaking)] //  QML: pttSpeaking — the key is down right now (UI cue)
        #[qproperty(bool, mic_muted)] //  QML: micMuted
        #[qproperty(QString, status)] //  QML: status
        #[qproperty(QString, room_id)] // QML: roomId
        #[qproperty(QString, outputs)] // QML: outputs — JSON [{node,label}] sinks
        #[qproperty(QString, output_name)] // QML: outputName — selected sink node
        #[qproperty(QString, inputs)] // QML: inputs — JSON [{node,label}] mics
        #[qproperty(QString, input_name)] // QML: inputName — selected mic node
        #[qproperty(QString, video_tiles)] // QML: videoTiles — JSON [{id,participant}]
        #[qproperty(bool, rec_active)] // QML: recActive — the Memories recorder is capturing this call (0.8.8: indicator, NOT a tile)
        #[qproperty(QString, conn_test)] // QML: connTest — throughput probe status/result
        #[qproperty(bool, camera_on)] //   QML: cameraOn — local camera live
        #[qproperty(QString, cameras)] //  QML: cameras — JSON [{node,label}] cams
        #[qproperty(QString, camera_name)] // QML: cameraName — selected camera device.path
        #[qproperty(QString, share_quality)] // QML: shareQuality — "maxW maxH fps kbps"
        #[qproperty(QString, cam_quality)] // QML: camQuality — "W H FPS KBPS SIM01"
        #[qproperty(QString, recv_quality)] // QML: recvQuality — auto|high|medium|low
        #[qproperty(bool, music_playing)] // QML: musicPlaying — #7 music 2nd-track live
        #[qproperty(bool, mic_denoise)] // QML: micDenoise — #7 RNNoise mic noise-cancel (default OFF)
        #[qproperty(bool, mic_aec)] //     QML: micAec — acoustic echo cancellation (default ON)
        #[qproperty(bool, mic_agc)] // QML: micAgc — libwebrtc auto-gain-control (SEPARATE; default OFF)
        #[qproperty(QString, mic_ns_mode)] // QML: micNsMode — "off" | "standard" (WebRTC NS in webrtcdsp) | "strong" (RNNoise) (0.8.25)
        #[qproperty(u32, selfview_gen)] // QML: selfviewGen — 0.8.28: bumps whenever the self-view pipeline is rebuilt (device switch / stall restart); the grid re-creates that one tile so it re-attaches
        // Health of the two cancellers, so the UI can grey a control out and say why
        // instead of offering a switch that silently does nothing.
        #[qproperty(bool, nc_available)] //  QML: ncAvailable — the RNNoise element is installed at all
        #[qproperty(bool, ec_degraded)] //   QML: ecDegraded — EC is wanted but is NOT running right now
        #[qproperty(QString, ec_note)] //    QML: ecNote — 0.8.31: what echo cancellation is doing, measured ("" = nothing to say)
        #[qproperty(QString, ec_outputs)] // QML: ecOutputs — 0.8.33: JSON [node…] the speaker outputs summed into the echo reference (""/[] = Tin Hat's own output)
        #[qproperty(QString, aec_engine)] // QML: aecEngine — "webrtc-dsp" (in-app) | "pipewire-module" (legacy Linux) (0.8.25)
        #[qproperty(bool, mic_silent)] //    QML: micSilent — the live mic has delivered only digital silence for 10 s (0.8.25)
        #[qproperty(bool, output_missing)] // QML: outputMissing — no usable output device at join (PipeWire dummy output) (0.8.25)
        #[qproperty(QString, audio_sources)] // QML: audioSources — JSON [{node,label}] shareable sources
        #[qproperty(QString, music_source_name)] // QML: musicSourceName — saved share-source node
        #[qproperty(QString, audio_apps)] // QML: audioApps — JSON [{id,label,detail,active}] per-app share sources (0.8.22)
        #[qproperty(bool, share_rec_only)] // QML: shareRecOnly — our share is / will be "for the recording only"
        #[qproperty(QString, rec_only_flags)] // QML: recOnlyFlags — JSON {tileId:[recOnly,tunedIn]}
        #[qproperty(bool, game_chat)] // QML: gameChat — I am in the game's own voice chat (0.8.23)
        #[qproperty(QString, game_chat_peers)] // QML: gameChatPeers — JSON {me,peers[],muted[]}
        #[qproperty(QString, corner_json)] //     QML: cornerJson — 9.4 corners {mine,active,members[],corners{}}
        #[qproperty(QString, audio_levels)] // QML: audioLevels — #B JSON [{user,category,rms}] live levels
        #[qproperty(QString, mic_level)] // QML: micLevel — #B local mic RMS "0.0".."1.0" as a string
        #[qproperty(QString, mic_level_pre)] // QML: micLevelPre — PRE-denoise mic RMS (noise-cancel before/after meter)
        #[qproperty(QString, output_level)] // QML: outputLevel — speaker/sink RMS "0.0".."1.0" as a string
        #[qproperty(QString, speakers)] // QML: speakers — {"me":identity,"active":[identities]} SFU active-speaker list
        #[qproperty(QString, publish_stats)] // QML: publishStats — outgoing stream health JSON [{kind,enc,w,h,fps,sendKbps,qlim,lossPct,...}]
        #[qproperty(bool, hanging_up)] // QML: hangingUp — leave teardown in progress; block rejoin until done
        #[qproperty(bool, deafened)] // QML: deafened — all incoming call audio muted (output deafen)
        #[qproperty(bool, reconnecting)] // QML: reconnecting — SFU connection dropped, auto-reconnecting
        #[qproperty(bool, call_failed)] // QML: callFailed — call ended unexpectedly (terminal disconnect)
        #[qproperty(QString, fail_reason)] // QML: failReason — 10.1: plain-English reason a JOIN failed ("" for a mid-call drop)
        #[qproperty(bool, mic_monitoring)] // QML: micMonitoring — standalone mic meter running (settings noise pane)
        #[qproperty(bool, output_monitoring)] // QML: outputMonitoring — standalone speaker meter running (settings Audio pane)
        // RBAC (docs/RBAC.md §5.4): server-enforced call moderation.
        #[qproperty(QString, call_perms_json)] // QML: callPermsJson — {"<key>":bool,…} my effective permissions in the call room ("{}" outside a call)
        #[qproperty(bool, can_moderate_call)] // QML: canModerateCall — callPerms.moderate_calls
        #[qproperty(bool, server_muted)] // QML: serverMuted — a moderator muted MY mic for everyone (mic button locked)
        #[qproperty(QString, server_muted_by)] // QML: serverMutedBy — that moderator's display name ("a moderator" when unknown; "" when not muted)
        #[qproperty(QString, removed_by)] // QML: removedBy — display name of the moderator who disconnected me from the last call ("" otherwise; cleared on the next join)
        #[qproperty(i32, moderation_rev)] // QML: moderationRev — bumps whenever the server-muted map changes (tiles carry serverMuted / serverMutedBy)
        #[qproperty(QString, last_error)] // QML: lastError — last failed moderation action, user-facing (the server's own message)
        #[qproperty(i32, error_rev)] // QML: errorRev — bumps on EVERY lastError set (same text twice still toasts)
        #[qproperty(QString, hear_json)] // QML: hearJson — 10.5 "can they hear me?" state (streamer hear.rs; "{}" outside a call)
        #[qproperty(QString, same_room_json)] // QML: sameRoomJson — 11.3 same room {linked,holder,live,ask,pending,spk,groups,rawOk} ("{}" outside a call)
        #[qproperty(bool, same_room_auto)] // QML: sameRoomAuto — 11.3: link up automatically with remembered people (default on, persisted)
        type CallController = super::CallControllerRust;

        #[qinvokable]
        fn join_call(self: Pin<&mut CallController>, room_id: &QString);
        #[qinvokable]
        fn leave_call(self: Pin<&mut CallController>);
        /// QML: callctl.switchCall(roomId) — 10.0: leave the current call (if any) and join
        /// `roomId` the moment the teardown completes; joins directly when not in a call.
        #[qinvokable]
        fn switch_call(self: Pin<&mut CallController>, room_id: &QString);
        #[qinvokable]
        fn share_screen(self: Pin<&mut CallController>);
        /// Windows share picker feed: capturable monitors + windows as JSON
        /// `[{kind:"monitor"|"window", handle:"<int>", label}]`. Returns `[]` on Linux
        /// (the portal pops its own system picker there), which is also QML's cue to
        /// call shareScreen() instead of opening the in-app picker.
        #[qinvokable]
        fn share_sources(self: Pin<&mut CallController>) -> QString;
        /// Start a screen share from a picker selection (kind + handle straight from a
        /// shareSources() entry). Windows-only in practice; no-op on Linux.
        #[qinvokable]
        fn share_screen_source(self: Pin<&mut CallController>, kind: &QString, handle: &QString);
        #[qinvokable]
        fn stop_screen(self: Pin<&mut CallController>);
        #[qinvokable]
        fn toggle_mic(self: Pin<&mut CallController>);
        /// QML: callctl.setPushToTalk(on) — persisted. When on, the mic stays muted
        /// unless the hold key is down.
        // NB: the qproperty above already generates BOTH `set_push_to_talk` and a C++
        // `setPushToTalk`, so this action needs its own name on both sides. It does more
        // than the setter anyway: it persists the choice and applies it to the live mic.
        #[qinvokable]
        #[cxx_name = "applyPushToTalk"]
        fn enable_push_to_talk(self: Pin<&mut CallController>, on: bool);
        #[qinvokable]
        #[cxx_name = "restorePushToTalk"]
        fn restore_push_to_talk(self: Pin<&mut CallController>);
        /// QML: callctl.pttHold(down) — the hold key went down / came up.
        #[qinvokable]
        #[cxx_name = "pttHold"]
        fn ptt_hold(self: Pin<&mut CallController>, down: bool);
        /// Explicitly set mic muted (NOT a toggle) + apply to the live call — idempotent, and
        /// always drives the gst pipeline to the right state (a force-unmute guarantees Playing,
        /// so AFK/BRB return reliably resumes capture). QML: forceMicMuted(bool).
        #[qinvokable]
        fn force_mic_muted(self: Pin<&mut CallController>, muted: bool);
        /// Re-enumerate audio output sinks into `outputs`.
        #[qinvokable]
        fn refresh_outputs(self: Pin<&mut CallController>);
        /// Route remote call audio to a sink node (empty = system default).
        #[qinvokable]
        fn set_output(self: Pin<&mut CallController>, node: &QString);
        /// Re-enumerate audio input mics into `inputs`.
        #[qinvokable]
        fn refresh_inputs(self: Pin<&mut CallController>);
        /// Capture the mic from a source node (empty = system default); applies live.
        #[qinvokable]
        fn set_input(self: Pin<&mut CallController>, node: &QString);
        /// Re-enumerate cameras (Video/Source) into `cameras`.
        #[qinvokable]
        fn refresh_cameras(self: Pin<&mut CallController>);
        /// Capture from a camera device.path (empty = first camera); applies live.
        #[qinvokable]
        fn set_camera(self: Pin<&mut CallController>, node: &QString);
        /// Toggle the local camera on/off (publish + self-view / unpublish).
        #[qinvokable]
        fn toggle_camera(self: Pin<&mut CallController>);
        /// Attach the QML video tile named `id` (a GstGLQt6VideoItem) to its gst
        /// pipeline. Returns true once attached; QML retries until then.
        #[qinvokable]
        fn attach_video(self: Pin<&mut CallController>, id: &QString) -> bool;
        /// Attach tile `tile_id`'s pipeline to an ARBITRARY QML container named
        /// `container` instead of the in-grid tile. NOTE: the PiP pop-out no longer uses
        /// this (it is a CPU viewer — see pip_start); kept for future cross-container
        /// re-points, which REBUILD the pipeline via the shared attach core.
        #[qinvokable]
        fn attach_video_to(
            self: Pin<&mut CallController>,
            tile_id: &QString,
            container: &QString,
        ) -> bool;
        /// #9 Pause-hidden: PAUSE (true) or resume (false) every REMOTE video tile's gst
        /// pipeline — called when the window is minimized/hidden so we stop wasting GPU/decode
        /// on video nobody can see. Skips the local self-view. Safe to call repeatedly.
        #[qinvokable]
        fn pause_remote_tiles(self: Pin<&mut CallController>, paused: bool);
        /// 11.0 (GPU.md §2.11, R17): the app-wide RENDER state ("focused" | "unfocused" | "hidden" — 10.8.1: an
        /// unfocused window arrives as "focused" unless the saver is on) and Settings → Appearance → Performance →
        /// "Save GPU while Tin Hat isn't focused" (default off): unfocused + saver = cameras on the server's LOW layer
        /// at ≤ 15 fps (shares unchanged) and the self-view paused on its last frame; hidden = remote video released at
        /// the SFU unless a visible pop-out shows it, the self-view paused; focused = everything back at once. Safe to
        /// call repeatedly (also re-applies the self-view pause to a pipeline that appeared meanwhile).
        #[qinvokable]
        fn set_window_state(self: Pin<&mut CallController>, state: &QString, saver_on: bool);
        /// 11.0 (GPU.md §2.11): the tile ids a visible pop-out window (PiP) shows, as a JSON array.
        #[qinvokable]
        fn set_popout_tiles(self: Pin<&mut CallController>, ids_json: &QString);
        /// #9 PiP pop-out (CPU viewer): start feeding tile `tile_id`'s track to the pop-out
        /// window's PipVideoItem (a second libwebrtc sink + libyuv -> RGBA; the grid tile's
        /// GL pipeline is untouched). Returns false until the tile's track handle exists —
        /// the PiP window retries. Idempotent.
        #[qinvokable]
        fn pip_start(self: Pin<&mut CallController>, tile_id: &QString) -> bool;
        /// #9 PiP pop-out: stop the viewer tap (pop-out closed). Idempotent.
        #[qinvokable]
        fn pip_stop(self: Pin<&mut CallController>, tile_id: &QString);
        /// Set + persist screen-share quality (resolution cap / fps / bitrate kbps).
        /// Takes effect on the next share. (Named apply_* to avoid colliding with the
        /// share_quality qproperty's auto-generated setter.)
        #[qinvokable]
        fn apply_share_quality(
            self: Pin<&mut CallController>,
            max_w: u32,
            max_h: u32,
            fps: u32,
            bitrate_kbps: u32,
        );
        /// Set + persist just the screen-share encoder (0 = NVENC H.264 / GPU,
        /// 1 = software VP9). Takes effect on the next share.
        #[qinvokable]
        fn apply_share_encoder(self: Pin<&mut CallController>, encoder: i32);
        /// QML: callctl.runConnectionTest() — 20s max-throughput probe (Settings → Calls).
        /// (The missing #[qinvokable] here shipped 0.3.3/0.3.4 with a dead button —
        /// "is not a function" TypeError, caught by a shipped log.)
        #[qinvokable]
        fn run_connection_test(self: Pin<&mut CallController>);
        /// Mirror the persisted screen-share quality into `shareQuality` for QML.
        #[qinvokable]
        fn refresh_share_quality(self: Pin<&mut CallController>);
        /// Set + persist the CAMERA publish quality (resolution / fps / bitrate kbps).
        /// A live camera is re-published so it applies mid-call (encoding is locked at
        /// publish time — a capture-only restart can't change it).
        #[qinvokable]
        fn apply_cam_quality(
            self: Pin<&mut CallController>,
            w: u32,
            h: u32,
            fps: u32,
            bitrate_kbps: u32,
        );
        /// Set + persist camera simulcast (adaptive layers; NVENC/GPU publishes only).
        /// A live camera is re-published so it applies mid-call.
        #[qinvokable]
        fn apply_cam_simulcast(self: Pin<&mut CallController>, on: bool);
        /// Mirror the persisted camera quality into `camQuality` for QML.
        #[qinvokable]
        fn refresh_cam_quality(self: Pin<&mut CallController>);
        /// Set + persist the incoming-video quality cap ("auto"|"high"|"medium"|"low")
        /// and re-signal every subscribed tile at the SFU.
        #[qinvokable]
        fn apply_recv_quality(self: Pin<&mut CallController>, mode: &QString);
        /// Mirror the persisted incoming-video cap into `recvQuality` for QML.
        #[qinvokable]
        fn refresh_recv_quality(self: Pin<&mut CallController>);
        /// Per-tile in-app visibility (AFK cut etc.): SFU-pause/resume ONE remote tile's
        /// video (UpdateTrackSettings{disabled}) — a real bandwidth stop, unlike the
        /// local gst pause. No-op for unknown/self tiles.
        #[qinvokable]
        fn set_tile_video_hidden(self: Pin<&mut CallController>, id: &QString, hidden: bool);
        /// Watch (and hear) a share its owner flagged "recording only", or stop again.
        #[qinvokable]
        fn set_tile_tuned_in(self: Pin<&mut CallController>, id: &QString, tuned: bool);
        /// Flag OUR share as recording-only (announced to peers now if sharing).
        #[qinvokable]
        fn apply_share_recording_only(self: Pin<&mut CallController>, on: bool);
        /// Refresh `recOnlyFlags` (the mirror timer calls this; flags flip without a
        /// tile-list change).
        #[qinvokable]
        fn refresh_rec_only_flags(self: Pin<&mut CallController>);
        /// Game chat on/off — my mic stays live to the room but peers who are ALSO in
        /// game chat don't hear it here (announced to peers; late joiners get a re-send).
        #[qinvokable]
        fn apply_game_chat(self: Pin<&mut CallController>, on: bool);
        /// Refresh `gameChatPeers` / `gameChat` (the mirror timer calls this).
        #[qinvokable]
        fn refresh_game_chat(self: Pin<&mut CallController>);
        /// 9.4 corners: pull `target` (a full identity or a bare mxid) aside into my corner (a
        /// new one if I am in none). Returns "" on success, else the reason to show.
        #[qinvokable]
        #[cxx_name = "pullAside"]
        fn pull_aside(self: Pin<&mut CallController>, target: &QString) -> QString;
        /// 9.4: join the corner `target` is in. "" on success, else the reason.
        #[qinvokable]
        #[cxx_name = "joinCorner"]
        fn join_corner(self: Pin<&mut CallController>, target: &QString) -> QString;
        /// 9.4: leave my corner (the yellow button).
        #[qinvokable]
        #[cxx_name = "leaveCorner"]
        fn leave_corner(self: Pin<&mut CallController>);
        /// 9.4: mirror tick — refresh `cornerJson`, drop out of an empty corner after the grace
        /// period, and publish whatever the streamer queued (corner and game-chat swaps).
        #[qinvokable]
        #[cxx_name = "refreshCorners"]
        fn refresh_corners(self: Pin<&mut CallController>);
        /// The user is (not) watching this share tile — its owner's share audio
        /// plays only while at least one of their share tiles is watched.
        #[qinvokable]
        fn set_tile_watched(self: Pin<&mut CallController>, id: &QString, watched: bool);
        /// Manual per-tile quality pick (the hover dropdown, YouTube-style):
        /// mode = "auto" | "180" | "360" | "720" | "1080". Beats the global
        /// recvQuality AND the tile-size heuristic; unlike auto it also applies
        /// to screenshares (an explicit choice, not an accidental blur).
        #[qinvokable]
        fn apply_tile_quality(self: Pin<&mut CallController>, id: &QString, mode: &QString);
        /// The current manual pick for a tile ("auto" when none) — dropdown checkmark.
        #[qinvokable]
        fn tile_quality(self: Pin<&mut CallController>, id: &QString) -> QString;
        /// Per-tile rendered size (physical px): ask the SFU for the closest simulcast
        /// rung / SVC layer for this tile (quantized; only re-signals on bucket change).
        #[qinvokable]
        fn set_tile_video_size(self: Pin<&mut CallController>, id: &QString, w: u32, h: u32);
        /// The tile's thZoom/thPan/thFill changed — re-apply the video geometry.
        #[qinvokable]
        fn relayout_tile_video(self: Pin<&mut CallController>, id: &QString);
        /// #7 MUSIC: publish a longer-lived "music" 2nd audio track from `url` (an http(s)
        /// URL or a `file://` path) into the active call. NO-OP (status) if not in a call.
        #[qinvokable]
        fn start_music(self: Pin<&mut CallController>, url: &QString);
        /// #7 MUSIC: stop the music stream (unpublish the 2nd track). Idempotent.
        #[qinvokable]
        fn stop_music(self: Pin<&mut CallController>);
        /// #7 MUSIC: reconcile `musicPlaying` with the streamer's actual aux-track state, so a
        /// finite music file that finishes on its own (EOS auto-unpublish, no stop click) clears
        /// the flag. Polled from QML's 120ms mirror. Cheap (a mutex peek); no-op while consistent.
        #[qinvokable]
        fn refresh_music_state(self: Pin<&mut CallController>);
        /// Emitted when a REMOTE participant joins / leaves the call — QML plays the themed
        /// join/leave chime. (The local user's own join doesn't raise these.)
        #[qsignal]
        fn participant_joined(self: Pin<&mut CallController>, identity: QString);
        #[qsignal]
        fn participant_left(self: Pin<&mut CallController>, identity: QString);
        /// #7 RNNoise: enable/disable mic noise-cancellation (default OFF; persisted).
        /// Rebuilds the mic capture pipeline mid-call so it takes effect immediately.
        /// (Named apply_* so it doesn't collide with the mic_denoise qproperty setter.)
        #[qinvokable]
        fn apply_mic_denoise(self: Pin<&mut CallController>, on: bool);
        /// QML: callctl.applyMicAec(on) — toggle acoustic echo cancellation; applies to a
        /// LIVE call at once (rebuilds the AEC module, re-points mic + playback).
        #[qinvokable]
        fn apply_mic_aec(self: Pin<&mut CallController>, on: bool);
        /// QML: callctl.refreshMicAec() — mirror the persisted AEC toggle into `micAec`.
        #[qinvokable]
        fn refresh_mic_aec(self: Pin<&mut CallController>);
        /// QML: applyEcOutputs(json) — 0.8.33: persist the speaker-output list the canceller
        /// listens to (JSON [node…]; [] = Tin Hat's own output) and rebind it live.
        #[qinvokable]
        fn apply_ec_outputs(self: Pin<&mut CallController>, json: &QString);
        /// #7 RNNoise: load the persisted denoise toggle into `micDenoise` for QML.
        #[qinvokable]
        fn refresh_mic_denoise(self: Pin<&mut CallController>);
        /// libwebrtc auto-gain-control: set + persist (SEPARATE from noise-suppression).
        /// Re-publishes the live mic so it applies mid-call (AGC is a source ctor option).
        #[qinvokable]
        fn apply_mic_agc(self: Pin<&mut CallController>, on: bool);
        /// Noise-suppression MODE ("off" | "standard" | "strong"): set + persist, then rebuild
        /// the live mic capture (both modes are pipeline stages) preserving mute. (0.8.25)
        #[qinvokable]
        fn apply_mic_ns_mode(self: Pin<&mut CallController>, mode: &QString);
        /// Load the persisted AGC + NS toggles into `micAgc` / `micNs` for QML.
        #[qinvokable]
        fn refresh_mic_apm(self: Pin<&mut CallController>);
        /// #1 MUSIC: re-enumerate shareable audio sources (sink monitors + inputs) into
        /// `audioSources`, and mirror the persisted choice into `musicSourceName`.
        #[qinvokable]
        fn refresh_audio_sources(self: Pin<&mut CallController>);
        /// #1 MUSIC: start (or replace) the music 2nd-track captured from a chosen PipeWire
        /// source `node` (empty = reuse the persisted source). Persists the choice. NO-OP
        /// (status) if not in a call.
        #[qinvokable]
        fn start_music_source(self: Pin<&mut CallController>, node: &QString);
        /// #4 AFK/BRB: cut/restore INCOMING call media so an away user can't hear the call.
        /// `on=true` mutes all remote audio (reversibly, won't stomp a #5 per-user mute);
        /// `on=false` restores only what AFK muted. Reapplied while AFK as tracks subscribe.
        #[qinvokable]
        fn set_away_privacy(self: Pin<&mut CallController>, on: bool);
        /// Deafen: mute ALL incoming call audio (output). Independent of AFK; composes with
        /// per-user (#5) mutes via the streamer's reversible reconcile. Transient (not persisted).
        /// Named apply_* so it doesn't collide with the `deafened` qproperty's auto setter.
        #[qinvokable]
        fn apply_deafen(self: Pin<&mut CallController>, on: bool);
        /// Noise-cancel before/after meter: start/stop a STANDALONE mic monitor pipeline (works
        /// outside a call) that publishes pre- and post-denoise RMS into micLevelPre / micLevel.
        /// No-op (and harmless) if a call's mic is already feeding the levels.
        #[qinvokable]
        fn start_mic_monitor(self: Pin<&mut CallController>);
        #[qinvokable]
        fn stop_mic_monitor(self: Pin<&mut CallController>);
        /// Speaker level meter: start/stop a STANDALONE output monitor that taps the selected
        /// sink's `.monitor` and publishes its RMS into outputLevel. Works outside a call; the
        /// settings Audio pane opens/closes it. No-op (harmless) if already running/stopped.
        #[qinvokable]
        fn start_output_monitor(self: Pin<&mut CallController>);
        #[qinvokable]
        fn stop_output_monitor(self: Pin<&mut CallController>);
        /// #B: poll the streamer's live audio levels into `audioLevels` (JSON) + `micLevel`,
        /// driving the per-name waveform bars. Only writes on a change (quiet when idle).
        /// Cheap mutex peeks; called from QML's 120ms mirror while in a call.
        #[qinvokable]
        fn refresh_audio_levels(self: Pin<&mut CallController>);
        /// Apply the event drain's authoritative call-tiles JSON to the `videoTiles`
        /// qproperty if it changed. Polled from QML's 120ms mirror so the video grid
        /// self-heals if a queued tile closure was ever dropped (a rejoin race). Quiet
        /// when unchanged (cf. refresh_audio_levels).
        #[qinvokable]
        fn refresh_video_tiles(self: Pin<&mut CallController>);
        /// RBAC: mute `identity`'s mic for everyone (server-enforced). Returns "" when the
        /// request went out (the network part reports through `status` on success and
        /// `lastError`/`errorRev` on failure), else the reason it was refused locally.
        /// `identity` is a LiveKit identity (`@u:server:DEVICE`); a bare mxid resolves to
        /// that user's first device in the call.
        #[qinvokable]
        fn server_mute(self: Pin<&mut CallController>, identity: &QString) -> QString;
        /// RBAC: undo a server mute. Same contract as serverMute.
        #[qinvokable]
        fn server_unmute(self: Pin<&mut CallController>, identity: &QString) -> QString;
        /// RBAC: disconnect `identity` from the call (server-enforced). Same contract.
        #[qinvokable]
        fn disconnect_participant(self: Pin<&mut CallController>, identity: &QString) -> QString;
        /// RBAC: recompute callPermsJson / canModerateCall now (also done on join, on every
        /// participant join and every 30 s while in a call).
        #[qinvokable]
        fn refresh_call_perms(self: Pin<&mut CallController>);
        /// 10.5: the pills' one-click fix for the SYSTEM sound settings — unmute (and raise from
        /// 0 %) the input (`output` false) or the output (`output` true). Linux (pactl); no-op elsewhere.
        #[qinvokable]
        fn fix_system_level(self: Pin<&mut CallController>, output: bool);
        /// 10.5: "The call isn't getting your mic" → publish the mic again, now.
        #[qinvokable]
        fn reconnect_mic(self: Pin<&mut CallController>);
        /// 10.5: the Fix on a tile that says we can't hear `identity` — rebuild that playback now.
        #[qinvokable]
        fn heal_peer_audio(self: Pin<&mut CallController>, identity: &QString);
        /// 10.5: the echo pill's "Add <output> to echo cancellation" (keeps the call's own output).
        #[qinvokable]
        fn add_ec_output(self: Pin<&mut CallController>, node: &QString);
        /// 11.3 same room: ask `target` (identity or mxid) to link up. "" on success, else the reason.
        #[qinvokable]
        #[cxx_name = "sameRoomLink"]
        fn same_room_link(self: Pin<&mut CallController>, target: &QString) -> QString;
        /// 11.3: unlink from everyone in my room.
        #[qinvokable]
        #[cxx_name = "sameRoomUnlink"]
        fn same_room_unlink(self: Pin<&mut CallController>);
        /// 11.3 QOL: take back my pending "link up?" ask (the pending pill's Cancel).
        #[qinvokable]
        #[cxx_name = "sameRoomCancel"]
        fn same_room_cancel(self: Pin<&mut CallController>);
        /// 11.3: answer the "X says you're in the same room" card.
        #[qinvokable]
        #[cxx_name = "sameRoomAnswer"]
        fn same_room_answer(self: Pin<&mut CallController>, yes: bool);
        /// 11.3: hear the call on this device (true) or through a linked peer's speakers (false).
        #[qinvokable]
        #[cxx_name = "sameRoomHearThrough"]
        fn same_room_hear_through(self: Pin<&mut CallController>, on_this_device: bool);
        /// 11.3: forget a remembered pair (Settings › Audio › Same room).
        #[qinvokable]
        #[cxx_name = "sameRoomForget"]
        fn same_room_forget(self: Pin<&mut CallController>, mxid: &QString);
        /// 11.3: the remembered pairs `{"<mxid>":{"spk":bool,"at":ms},…}`.
        #[qinvokable]
        #[cxx_name = "sameRoomPairsJson"]
        fn same_room_pairs_json(self: Pin<&mut CallController>) -> QString;
        /// 11.3: the auto-link preference (persisted; `sameRoomAuto` mirrors it).
        #[qinvokable]
        #[cxx_name = "applySameRoomAuto"]
        fn apply_same_room_auto(self: Pin<&mut CallController>, on: bool);
    }

    impl cxx_qt::Threading for CallController {}
}

#[derive(Default)]
pub struct CallControllerRust {
    same_room_json: QString, // 11.3 same room (mirror of tinhat_streamer::same_room_json)
    same_room_auto: bool,    // 11.3 same room: auto-link preference
    in_call: bool,
    connecting: bool,
    sharing: bool,
    mic_muted: bool,
    push_to_talk: bool,
    ptt_speaking: bool,
    status: QString,
    conn_test: QString,
    room_id: QString,
    outputs: QString,
    output_name: QString,
    inputs: QString,
    input_name: QString,
    video_tiles: QString,
    rec_active: bool,
    camera_on: bool,
    cameras: QString,
    camera_name: QString,
    share_quality: QString,
    cam_quality: QString,  // "W H FPS KBPS SIM01" camera publish quality mirror
    recv_quality: QString, // "auto"|"high"|"medium"|"low" incoming-video cap mirror
    music_playing: bool,
    mic_denoise: bool,
    mic_aec: bool,
    mic_agc: bool,
    mic_ns_mode: QString,
    selfview_gen: u32,
    nc_available: bool,
    ec_degraded: bool,
    ec_note: QString,
    ec_outputs: QString, // 0.8.33: JSON [node…] speaker outputs for the echo reference
    aec_engine: QString,
    mic_silent: bool,
    output_missing: bool,
    audio_sources: QString,
    music_source_name: QString,
    audio_apps: QString,
    share_rec_only: bool,
    rec_only_flags: QString,
    game_chat: bool,
    game_chat_peers: QString,
    corner_json: QString,
    audio_levels: QString, // #B JSON [{user,category,rms}]
    mic_level: QString,    // #B local mic RMS as a string ("0.0".."1.0")
    mic_level_pre: QString, // PRE-denoise mic RMS (noise-cancel before/after meter)
    output_level: QString,  // speaker/sink RMS as a string ("0.0".."1.0")
    speakers: QString,      // {"me":identity,"active":[identities]} SFU active-speaker list
    publish_stats: QString, // outgoing stream health JSON (see streamer publish_stats_json)
    hanging_up: bool,      // leave teardown in progress; rejoin blocked until done
    deafened: bool,        // all incoming call audio muted (output deafen)
    reconnecting: bool,    // SFU connection dropped, auto-reconnecting
    call_failed: bool,     // call ended unexpectedly (terminal disconnect)
    fail_reason: QString,  // 10.1 (TH-CALL-03): why the last join failed, for the banner
    mic_monitoring: bool,  // standalone mic meter running (settings noise pane)
    output_monitoring: bool, // standalone speaker meter running (settings Audio pane)
    // RBAC (docs/RBAC.md §5.4)
    call_perms_json: QString, // {"<key>":bool,…} effective perms in the call room ("{}" outside a call)
    can_moderate_call: bool,  // callPerms.moderate_calls
    server_muted: bool,       // a moderator muted MY mic for everyone
    server_muted_by: QString, // their display name ("a moderator" when unknown)
    removed_by: QString,      // who disconnected me from the last call ("" otherwise)
    moderation_rev: i32,      // bumps when the server-muted map changes
    last_error: QString,      // last failed moderation action (user-facing)
    error_rev: i32,           // bumps on every last_error set
    hear_json: QString,       // 10.5 "can they hear me?" (streamer hear.rs)
}

/// Remote video tiles' gst pipeline pointers, keyed by tile id (== QML objectName),
/// populated by the call's event drain and read by `attach_video` on the Qt thread.
fn video_registry() -> &'static std::sync::Mutex<HashMap<String, usize>> {
    static R: OnceLock<std::sync::Mutex<HashMap<String, usize>>> = OnceLock::new();
    R.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

/// The authoritative video-tiles JSON for the current call, written SYNCHRONOUSLY by the
/// event drain (off the Qt thread, so it can't be dropped) and applied to the `videoTiles`
/// qproperty by the polled `refresh_video_tiles` invokable. This decouples the QML model
/// update from the one-shot `qt.queue` closures: if a closure is ever missed (a rejoin
/// race was observed where a queued tile-create closure never ran), the next 120ms poll
/// still publishes the correct tiles so the delegate appears and its attach Timer can
/// lazily build the pipeline. Empty string = "not set / leave handles it".
fn desired_video_tiles() -> &'static std::sync::Mutex<String> {
    static D: OnceLock<std::sync::Mutex<String>> = OnceLock::new();
    D.get_or_init(|| std::sync::Mutex::new(String::new()))
}

fn set_desired_tiles(json: &str) {
    *desired_video_tiles().lock().unwrap() = json.to_string();
}

/// Whether the standalone mic monitor is currently WANTED (noise panel open). Set by
/// start_mic_monitor, cleared by stop_mic_monitor. The async start re-checks this after
/// building so a close that races ahead of the build still tears the pipeline down.
fn mic_monitor_wanted() -> &'static std::sync::atomic::AtomicBool {
    static W: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    &W
}

/// Whether the standalone OUTPUT meter is currently WANTED (settings Audio pane open). Set by
/// start_output_monitor, cleared by stop_output_monitor. The async start re-checks this after
/// building so a close that races ahead of the build still tears the pipeline down.
fn output_monitor_wanted() -> &'static std::sync::atomic::AtomicBool {
    static W: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    &W
}

/// Serialize the tile list `[(id, participant)]` to the JSON the QML grid parses.
/// The local self-view tile id (objectName in the QML grid, looked up by the C++
/// attach). One per call — just another entry in the grid model + video_registry.
const SELFVIEW_ID: &str = "vid_selfview";

/// Whether the local self-view tile is currently in the grid (so the drain loop's
/// tile rebuild includes it). Set on JOIN — the tile is present for the whole call
/// (avatar placeholder until the camera turns on) — and cleared on leave.
fn selfview_active() -> &'static std::sync::atomic::AtomicBool {
    static A: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    &A
}

/// Add a `{id:"vid_selfview", participant:"You"}` entry to a tiles JSON array if not
/// present (prepended). Lets toggle_camera show the self-view without touching the
/// drain loop's remote-tile vec.
fn inject_selfview(json: &str) -> String {
    let mut arr: Vec<serde_json::Value> = serde_json::from_str(json).unwrap_or_default();
    let exists = arr.iter().any(|v| v.get("id").and_then(|s| s.as_str()) == Some(SELFVIEW_ID));
    if !exists {
        arr.insert(0, serde_json::json!({ "id": SELFVIEW_ID, "participant": "You" }));
    }
    serde_json::to_string(&arr).unwrap_or_else(|_| "[]".to_string())
}

/// Remove the self-view entry from a tiles JSON array.
fn strip_selfview(json: &str) -> String {
    let arr: Vec<serde_json::Value> = serde_json::from_str(json).unwrap_or_default();
    let kept: Vec<serde_json::Value> = arr
        .into_iter()
        .filter(|v| v.get("id").and_then(|s| s.as_str()) != Some(SELFVIEW_ID))
        .collect();
    serde_json::to_string(&kept).unwrap_or_else(|_| "[]".to_string())
}

/// Cached display names for call participants, keyed by bare mxid (`@user:server`). Filled
/// asynchronously from the room member list so the synchronous tile rebuild can look names up
/// without blocking. Empty = fall back to the localpart in QML.
fn call_names() -> &'static std::sync::Mutex<std::collections::HashMap<String, String>> {
    static N: OnceLock<std::sync::Mutex<std::collections::HashMap<String, String>>> = OnceLock::new();
    N.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}
/// Headless gate for push-to-talk (QOL 3.4a). Pure decision table — no call, no audio
/// device, no keyboard:  TINHAT_PTT_TEST=1 ./tinhat
pub fn headless_ptt_test() -> bool {
    let mut fails = 0;
    let mut check = |label: &str, ok: bool| {
        eprintln!("[ptt] {} {label}", if ok { "PASS" } else { "FAIL" });
        if !ok { fails += 1; }
    };
    use crate::call_ctl::qobject::CallController as C;
    // With PTT off, the manual mute button is the only thing that decides.
    check("ptt off: unmuted stays unmuted", !C::ptt_target_muted(false, false, false));
    check("ptt off: muted stays muted", C::ptt_target_muted(false, false, true));
    // With PTT on, holding the key speaks and releasing it stops — regardless of what
    // the mute button was left on, which is the point: you cannot forget yourself live.
    check("ptt on, key down: speaking", !C::ptt_target_muted(true, true, false));
    check("ptt on, key up: muted", C::ptt_target_muted(true, false, false));
    check("ptt on overrides a stale unmute", C::ptt_target_muted(true, false, false));
    check("ptt on, key down beats a stale mute", !C::ptt_target_muted(true, true, true));
    eprintln!("[ptt] VERDICT: {}", if fails == 0 { "PASS" } else { "FAIL" });
    fails == 0
}

/// Persisted push-to-talk preference, a `tinhat/` config-dir flag file like every other toggle
/// (cf. notify.rs). Default OFF: a client that silently required a held key to be heard
/// would look broken to anyone who did not read release notes.
fn ptt_pref_path() -> std::path::PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| {
            std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
        });
    base.join("tinhat/push-to-talk")
}

pub(crate) fn ptt_pref() -> bool {
    std::fs::read_to_string(ptt_pref_path()).map(|s| s.trim() == "1").unwrap_or(false)
}

fn set_ptt_pref(on: bool) {
    let p = ptt_pref_path();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(&p, if on { "1" } else { "0" });
    eprintln!("[ptt] push-to-talk {}", if on { "ON" } else { "OFF" });
}

/// 11.3 same room: "link up automatically with people I've linked with before" — a `tinhat/` config-dir flag file
/// like push-to-talk; ABSENT = on (the default), "0" = off.
fn same_room_auto_pref_path() -> std::path::PathBuf {
    ptt_pref_path().with_file_name("sameroom-auto")
}
pub(crate) fn same_room_auto_pref() -> bool {
    std::fs::read_to_string(same_room_auto_pref_path()).map(|s| s.trim() != "0").unwrap_or(true)
}
fn set_same_room_auto_pref(on: bool) {
    let p = same_room_auto_pref_path();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(&p, if on { "1" } else { "0" });
    eprintln!("[sr] auto-link {}", if on { "ON" } else { "OFF" });
}

/// Resolved avatars for the people in this call: mxid -> local `file://` url, filled by
/// [`refresh_call_names`] alongside the display names. Empty = no photo set (or not
/// resolved yet), which QML renders as the generated fox/initial.
///
/// Without this the call grid had NO avatar data at all and EVERY participant fell back
/// to the placeholder, even though the room list resolved their real photo for the same
/// people at the same moment (QOL Phase 1.5.0).
fn call_avatars() -> &'static std::sync::Mutex<std::collections::HashMap<String, String>> {
    static A: OnceLock<std::sync::Mutex<std::collections::HashMap<String, String>>> = OnceLock::new();
    A.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

// ── RBAC: call moderation helpers (docs/RBAC.md §5.4) ───────────────────────────────

/// Mirror of `canModerateCall` for the async paths (the drain's auto re-enforcement runs
/// off the Qt thread and cannot read the property).
fn can_moderate_flag() -> &'static std::sync::atomic::AtomicBool {
    static F: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    &F
}

/// My effective permissions in `room_id` — (`{"<key>":bool,…}`, moderate_calls).
async fn compute_call_perms(room_id: &str) -> (String, bool) {
    let me = crate::session::client().and_then(|c| c.user_id().map(|u| u.to_string())).unwrap_or_default();
    if room_id.is_empty() || me.is_empty() {
        return ("{}".to_string(), false);
    }
    let p = crate::roles::effective_perms(room_id, &me).await;
    (p.json(), p.has("moderate_calls"))
}

/// Our own LiveKit identity (`@user:server:DEVICE`), "" when not signed in.
fn my_call_identity() -> String {
    let Some(c) = crate::session::client() else { return String::new() };
    format!(
        "{}:{}",
        c.user_id().map(|u| u.to_string()).unwrap_or_default(),
        c.device_id().map(|d| d.to_string()).unwrap_or_default()
    )
}
fn name_for_mxid(mxid: &str) -> String {
    call_names().lock().unwrap().get(mxid).cloned().unwrap_or_default()
}
fn localpart(mxid: &str) -> String {
    mxid.trim_start_matches('@').split(':').next().unwrap_or("").to_string()
}
/// A person's label for toasts: display name, else the mxid localpart.
fn person_label(identity: &str) -> String {
    let n = name_for(identity);
    if n.is_empty() { localpart(&mxid_of(identity)) } else { n }
}
/// "muted by <name>": the packet's own `name`, else the name we resolved for `by` (an
/// identity OR a bare mxid — the room state stores the latter), else "a moderator".
fn moderator_label(by: &str, by_name: &str) -> String {
    if !by_name.is_empty() {
        return by_name.to_string();
    }
    if !by.is_empty() {
        let n = name_for(by);
        if !n.is_empty() {
            return n;
        }
        let n = name_for_mxid(by);
        if !n.is_empty() {
            return n;
        }
    }
    "a moderator".to_string()
}
/// (serverMuted, serverMutedBy) for a tile identity.
fn server_mute_fields(identity: &str) -> (bool, String) {
    match tinhat_streamer::server_mute_info(identity) {
        Some((by, name)) => (true, moderator_label(&by, &name)),
        None => (false, String::new()),
    }
}

/// The async half of serverMute / serverUnmute / disconnectParticipant (and of the
/// auto re-enforcement, `announce=false`). Order per §5.4:
///   1. `disconnect` only: a packet to the TARGET alone (they must learn who removed them
///      before the SFU cuts them off — a broadcast after that never reaches them);
///   2. POST `/moderation/call` (the server verifies our token + roles and acts on the SFU);
///   3. on 2xx: broadcast the packet (so a refused request never paints a shield), and for
///      mute/unmute write / clear `dev.tinhat.call.moderation` in the call room;
///   4. `status` on success, `lastError`/`errorRev` (the server's own text) on failure.
/// Auto re-enforcement (`announce=false`) skips 3's state write — the room already says
/// muted, by the ORIGINAL moderator with the original `ts`, and that record must survive —
/// and only logs its outcome.
async fn moderate_async(
    qt: cxx_qt::CxxQtThread<qobject::CallController>,
    room_id: String,
    action: &'static str,
    target: String,
    announce: bool,
) {
    let me_mxid = crate::session::client().and_then(|c| c.user_id().map(|u| u.to_string())).unwrap_or_default();
    let my_name = name_for_mxid(&me_mxid);
    let who = person_label(&target);
    if action == "disconnect" {
        tinhat_streamer::publish_moderation_notice(&target, &my_name).await;
    }
    match crate::moderation::call_action(&room_id, action, &target).await {
        Ok(_) => {
            tinhat_streamer::publish_moderation_packet(action, &target, &my_name).await;
            if action != "disconnect" && announce {
                let target_mxid = mxid_of(&target);
                if let Err(e) =
                    crate::moderation::write_mute_state(&room_id, &target_mxid, action == "mute", &me_mxid).await
                {
                    eprintln!("[moderation] state write failed ({action} {target_mxid}): {e}");
                }
            }
            if announce {
                let text = match action {
                    "mute" => format!("Muted {who} for everyone"),
                    "unmute" => format!("Unmuted {who} for everyone"),
                    _ => format!("Disconnected {who} from the call"),
                };
                let _ = qt.queue(move |mut o| o.as_mut().set_status(QString::from(&text)));
            }
        }
        Err(e) => {
            let text = match action {
                "mute" => format!("Couldn't mute {who}: {e}"),
                "unmute" => format!("Couldn't unmute {who}: {e}"),
                _ => format!("Couldn't disconnect {who}: {e}"),
            };
            if announce {
                let _ = qt.queue(move |mut o| o.as_mut().report_error(&text));
            } else {
                eprintln!("[moderation] auto re-enforcement: {text}");
            }
        }
    }
}

/// The bare mxid for a LiveKit identity (`@user:server:DEVICE` -> `@user:server`).
fn mxid_of(identity: &str) -> String {
    identity
        .rsplit_once(':')
        .map(|(m, _)| m.to_string())
        .unwrap_or_else(|| identity.to_string())
}
fn name_for(identity: &str) -> String {
    call_names()
        .lock()
        .unwrap()
        .get(&mxid_of(identity))
        .cloned()
        .unwrap_or_default()
}
/// A participant's avatar url for the tile JSON ("" when they have no photo).
pub(crate) fn avatar_for(identity: &str) -> String {
    call_avatars()
        .lock()
        .unwrap()
        .get(&mxid_of(identity))
        .cloned()
        .unwrap_or_default()
}

/// Resolve display names for the room's members into [`call_names`] (off the Qt thread).
async fn refresh_call_names(room_id: &str) {
    let Some(client) = crate::session::client() else { return };
    let Ok(rid) = matrix_sdk::ruma::RoomId::parse(room_id) else { return };
    let Some(room) = client.get_room(&rid) else { return };
    if let Ok(members) = room.members_no_sync(matrix_sdk::RoomMemberships::ACTIVE).await {
        // Resolve avatars BEFORE taking the names lock: resolve_avatar awaits (cache
        // miss = a media fetch), and holding a std::sync Mutex across an await would
        // both deadlock-risk and stall every synchronous tile rebuild behind the network.
        let mut avatars: Vec<(String, String)> = Vec::new();
        for m in &members {
            if let Some(mxc) = m.avatar_url() {
                if let Some(url) = crate::bridge::resolve_avatar(&client, mxc).await {
                    avatars.push((m.user_id().to_string(), url));
                }
            }
        }
        {
            let mut amap = call_avatars().lock().unwrap();
            for (mxid, url) in avatars {
                // The self-view tile identifies as "You" rather than an mxid, so mirror
                // our own photo under that key — otherwise the one tile you always see
                // is the only one still showing a placeholder.
                if client.user_id().map(|u| u.as_str()) == Some(mxid.as_str()) {
                    amap.insert("You".to_string(), url.clone());
                }
                amap.insert(mxid, url);
            }
        }
        let mut map = call_names().lock().unwrap();
        for m in members {
            if let Some(name) = m.display_name() {
                map.insert(m.user_id().to_string(), name.to_string());
            }
        }
    }
}

/// Is this LiveKit identity (`@user:server:DEVICE`) the Memories recorder the user's server announces? 0.8.8: its
/// presence drives the 🔴 REC indicator instead of a participant tile (a deliberate choice: it is not shown as another
/// user). With no recorder announced nothing is one (room.rs recorder_mxid).
pub(crate) fn recorder_identity(identity: &str, recorder: Option<&str>) -> bool {
    recorder.is_some_and(|r| crate::server_offer::is_recorder_id(identity, r))
}
fn is_vod_bot(identity: &str) -> bool {
    recorder_identity(identity, crate::room::recorder_mxid().as_deref())
}

/// Build the enriched tiles JSON the QML grid consumes:
///   `[{id, participant, kind:"video"|"screenshare"|"avatar", name, screenshare, muted, avatar}]`
/// — video/screenshare tiles from subscribed tracks PLUS an avatar tile for every connected
/// participant who has no camera tile (so voice-only / camera-off peers still appear), PLUS
/// the self-view when active. `name` is the resolved display name ("" => QML localpart fallback).
fn tiles_json_enriched(
    tiles: &[(String, String, bool)],
    participants: &std::collections::HashSet<String>,
    muted: &std::collections::HashSet<String>,
) -> String {
    use std::sync::atomic::Ordering;
    let mut arr: Vec<serde_json::Value> = Vec::new();
    if selfview_active().load(Ordering::Relaxed) {
        let (sm, sm_by) = server_mute_fields(&my_call_identity());
        arr.push(serde_json::json!({
            "id": SELFVIEW_ID, "participant": "You", "kind": "video",
            "name": "You", "screenshare": false, "muted": false,
            "avatar": avatar_for("You"),
            "serverMuted": sm, "serverMutedBy": sm_by,
        }));
    }
    // identities that already have a CAMERA (non-screenshare) tile — no avatar tile for them.
    let cam: std::collections::HashSet<&String> =
        tiles.iter().filter(|(_, _, ss)| !ss).map(|(_, id, _)| id).collect();
    for (id, identity, ss) in tiles {
        let (rec_only, tuned_in) = tinhat_streamer::tile_recording_state(id);
        let (sm, sm_by) = server_mute_fields(identity);
        arr.push(serde_json::json!({
            "id": id, "participant": identity,
            "kind": if *ss { "screenshare" } else { "video" },
            "name": name_for(identity), "screenshare": ss,
            "muted": muted.contains(identity),
            "avatar": avatar_for(identity),
            "recordingOnly": rec_only, "tunedIn": tuned_in,
            "serverMuted": sm, "serverMutedBy": sm_by,
        }));
    }
    for identity in participants {
        if !cam.contains(identity) {
            let (sm, sm_by) = server_mute_fields(identity);
            arr.push(serde_json::json!({
                "id": format!("avatar_{identity}"), "participant": identity,
                "kind": "avatar", "name": name_for(identity),
                "screenshare": false, "muted": muted.contains(identity),
                "avatar": avatar_for(identity),
                "serverMuted": sm, "serverMutedBy": sm_by,
            }));
        }
    }
    serde_json::to_string(&arr).unwrap_or_else(|_| "[]".to_string())
}

/// Headless gate for QOL Phase 1.5.0 — proves a participant's real profile picture
/// reaches the call tiles, which is the thing that was silently missing:
///   TINHAT_CALLAVATAR=1 TINHAT_USER=… TINHAT_PASS=… ./tinhat
///
/// Sets a real avatar on a scratch account, creates a room, runs the SAME
/// `refresh_call_names` the call uses, and asserts the tile JSON carries a usable
/// avatar for that participant. Exits 0/1.
pub async fn headless_avatar_test() -> bool {
    let mut fails = 0;
    let mut check = |label: &str, ok: bool| {
        eprintln!("[callavatar] {} {label}", if ok { "PASS" } else { "FAIL" });
        if !ok {
            fails += 1;
        }
    };

    let (Ok(user), Ok(pass)) = (std::env::var("TINHAT_USER"), std::env::var("TINHAT_PASS")) else {
        eprintln!("[callavatar] FAIL: need TINHAT_USER + TINHAT_PASS");
        return false;
    };
    let hs = match crate::bridge::env_homeserver() {
        Ok(h) => h,
        Err(e) => {
            eprintln!("[callavatar] FAIL: {e}");
            return false;
        }
    };
    let client = match crate::bridge::headless_login(&hs, &user, &pass).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[callavatar] FAIL: login: {e}");
            return false;
        }
    };
    crate::session::set_client(client.clone());

    // A 1x1 PNG is enough: we are testing the RESOLUTION PATH, not image decoding.
    const PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];
    let mime: mime::Mime = "image/png".parse().unwrap();
    match client.account().upload_avatar(&mime, PNG.to_vec()).await {
        Ok(_) => check("avatar uploaded + set on the account", true),
        Err(e) => check(&format!("avatar upload ({e})"), false),
    }

    use matrix_sdk::ruma::api::client::room::create_room;
    let room = match client.create_room(create_room::v3::Request::new()).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[callavatar] FAIL: create room: {e}");
            return false;
        }
    };
    let room_id = room.room_id().to_string();

    // refresh_call_names reads the LOCAL store (members_no_sync) — in the running app the
    // background sync has always filled it, but a headless run has synced nothing, so the
    // member list is empty and every avatar resolves to "". Sync first, exactly like the
    // app does, or this gate measures the harness instead of the feature.
    let sync_settings =
        crate::bridge::sync_settings().timeout(std::time::Duration::from_secs(5));
    let mut resolved = false;
    for i in 0..10 {
        if let Err(e) = client.sync_once(sync_settings.clone()).await {
            eprintln!("[callavatar] sync_once: {e}");
        }
        refresh_call_names(&room_id).await;
        if !avatar_for("You").is_empty() {
            eprintln!("[callavatar] resolved after {} sync pass(es)", i + 1);
            resolved = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    check("resolves within a few sync passes", resolved);

    let me = client.user_id().map(|u| u.to_string()).unwrap_or_default();
    let identity = format!("{me}:TESTDEVICE");
    let by_identity = avatar_for(&identity);
    check("avatar resolves for a LiveKit identity", !by_identity.is_empty());
    check("self-view ('You') resolves too", !avatar_for("You").is_empty());

    // The resolved url must be a file that actually exists — an empty or dangling path
    // renders as the placeholder, which is the bug we are fixing.
    let path = crate::platform::path_from_file_url(&by_identity);
    check(
        "resolved avatar file exists on disk",
        !path.is_empty() && std::path::Path::new(&path).exists(),
    );

    // And it must survive into the JSON the QML grid actually binds to.
    let mut participants = std::collections::HashSet::new();
    participants.insert(identity.clone());
    let json = tiles_json_enriched(&[], &participants, &std::collections::HashSet::new());
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap_or_default();
    let tile_avatar = parsed
        .as_array()
        .and_then(|a| a.iter().find(|t| t["kind"] == "avatar"))
        .and_then(|t| t["avatar"].as_str())
        .unwrap_or("");
    check("call-tile JSON carries the avatar", !tile_avatar.is_empty());

    let _ = room.leave().await;
    eprintln!("[callavatar] VERDICT: {}", if fails == 0 { "PASS" } else { "FAIL" });
    fails == 0
}

/// Our LiveKit media key (raw 16 bytes + index) for the active call, so peer-join events
/// can re-announce it over to-device. Set in `do_join`, cleared on leave/exit.
fn current_media_key() -> &'static std::sync::Mutex<Option<([u8; 16], u8)>> {
    static K: OnceLock<std::sync::Mutex<Option<([u8; 16], u8)>>> = OnceLock::new();
    K.get_or_init(|| std::sync::Mutex::new(None))
}

/// Announce our media key over ENCRYPTED to-device to every OTHER live call-member device,
/// so Element Call peers can decrypt our outgoing audio/video. Resolves each membership's
/// crypto `Device` and hands them to [`crate::e2ee_keys::send_our_key_to_device`]. Logs the
/// reachable/failed counts. Best-effort: a peer we can't resolve yet is retried when it
/// raises `ParticipantJoined`.
pub(crate) async fn announce_key_to_members(
    client: &matrix_sdk::Client,
    room: &matrix_sdk::Room,
    user_id: &str,
    device_id: &str,
    key: [u8; 16],
    index: u8,
) {
    let our_member_id = format!("{user_id}:{device_id}");
    let mut targets: Vec<matrix_sdk::encryption::identities::Device> = Vec::new();
    for (peer_user, peer_dev) in crate::rtc::call_member_devices(room).await {
        if peer_user == user_id && peer_dev == device_id {
            continue; // skip ourselves
        }
        if let Ok(uid) = matrix_sdk::ruma::UserId::parse(&peer_user) {
            let did = matrix_sdk::ruma::OwnedDeviceId::from(peer_dev.as_str());
            let mut dev = client.encryption().get_device(&uid, &did).await.ok().flatten();
            if dev.is_none() {
                // The device isn't in our local crypto store yet (e.g. a phone we haven't
                // key-queried this session) — get_device reads the local store only. Force a
                // keys_query for this user, then retry ONCE. Without this the peer is silently
                // dropped and never receives our key, so our audio/video stays silent/black to
                // them. Best-effort; the returned identity is ignored.
                let _ = client.encryption().request_user_identity(&uid).await;
                dev = client.encryption().get_device(&uid, &did).await.ok().flatten();
                if dev.is_none() {
                    eprintln!("[send-key-td] device {peer_user}/{peer_dev} unresolved after key-query — skipping");
                }
            }
            if let Some(dev) = dev {
                targets.push(dev);
            }
        }
    }
    match crate::e2ee_keys::send_our_key_to_device(
        client, room.room_id(), device_id, &our_member_id, &key, index, &targets,
    )
    .await
    {
        Ok(failed) if failed.is_empty() => {
            eprintln!("[send-key-td] announced media key to {} device(s) over to-device", targets.len())
        }
        Ok(failed) => eprintln!(
            "[send-key-td] announced to {} device(s), {} FAILED: {failed:?}",
            targets.len(),
            failed.len()
        ),
        Err(e) => eprintln!("[send-key-td] to-device key announce failed: {e}"),
    }
}

/// Re-announce our media key to ALL current call-member devices, triggered when a peer
/// joins the SFU — essential when the peer joins AFTER us (the initial members-announce in
/// `do_join` couldn't reach a peer that wasn't there yet). We re-announce to EVERYONE
/// rather than invert the LiveKit identity, because that identity may be the colon form
/// (`userId:deviceId`) OR a hashed MSC4143/RTC form — re-sending to already-keyed peers is
/// harmless, and the member enumeration is the format-agnostic source of truth. `identity`
/// is used only for the log line.
async fn announce_key_to_peer(room_id: String, identity: String) {
    let Some(client) = crate::session::client() else { return };
    let Ok(rid) = matrix_sdk::ruma::RoomId::parse(&room_id) else { return };
    let Some(room) = client.get_room(&rid) else { return };
    let Some((key, index)) = *current_media_key().lock().unwrap() else { return };
    let our_device = client.device_id().map(|d| d.to_string()).unwrap_or_default();
    let our_user = client.user_id().map(|u| u.to_string()).unwrap_or_default();
    eprintln!("[send-key-td] peer {identity} joined — re-announcing key to call members");
    // Re-announce LADDER: the peer's LiveKit join routinely BEATS their m.call.member
    // state syncing to us, in which case the enumeration inside announce_key_to_members
    // silently misses them and nothing re-sends until the 60s steady re-announce loop
    // catches up — our audio/video stays static/black for that peer the whole time.
    // A few spaced re-sends cover the sync lag; re-keying already-keyed peers is
    // harmless (byte-identical key, receivers overwrite).
    for (i, delay_s) in [0u64, 3, 10, 30, 60, 120].into_iter().enumerate() {
        if delay_s > 0 {
            tokio::time::sleep(std::time::Duration::from_secs(delay_s)).await;
        }
        // Stop if the call ended or the key changed while we waited.
        if active_call_room().lock().unwrap().as_deref() != Some(room_id.as_str()) {
            return;
        }
        if *current_media_key().lock().unwrap() != Some((key, index)) {
            return;
        }
        if i > 0 {
            eprintln!("[send-key-td] re-announce #{i} for {identity} (membership sync-lag cover)");
        }
        announce_key_to_members(&client, &room, &our_user, &our_device, key, index).await;
        // (The one-shot room-event mirror that used to follow the first rung was retired
        // on 2026-09-21 — see do_join. To-device is the whole transport.)
    }
}

/// The one active call, shared with async tasks (the QObject is `!Send` + lives on
/// the Qt thread, so the session lives here behind an async mutex).
fn current_call() -> Arc<Mutex<Option<CallSession>>> {
    static CALL: OnceLock<Arc<Mutex<Option<CallSession>>>> = OnceLock::new();
    CALL.get_or_init(|| Arc::new(Mutex::new(None))).clone()
}

/// The JoinHandle of `leave_call`'s in-flight teardown task, if any. On app-close,
/// `leave_on_exit` awaits this when it loses the `current_call().take()` race — so the
/// process does NOT exit until the teardown's SFU LeaveRequest has actually been sent
/// (otherwise a window-close right after pressing Hang up orphans a ghost participant that
/// wedges the next rejoin). A `std::sync::Mutex` so the sync `leave_call` can store without
/// awaiting; never held across an `.await`.
fn teardown_handle() -> &'static std::sync::Mutex<Option<tokio::task::JoinHandle<()>>> {
    static H: OnceLock<std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>> = OnceLock::new();
    H.get_or_init(|| std::sync::Mutex::new(None))
}

/// Is there an active call right now? Used by the soundboard to decide between playing a
/// sound and a friendly "join a call" no-op. Tracks `current_call`, which is set in lockstep
/// with the streamer's 2nd-track participant (set on join, cleared on close/leave).
pub async fn in_active_call() -> bool {
    current_call().lock().await.is_some()
}

/// Re-publish the live mic (if in a call) so a changed AGC/NS setting takes effect now,
/// PRESERVING the current mute state. No-op when not in a call — the new persisted value is
/// read on the next mic publish.
fn republish_live_mic(was_muted: bool) {
    crate::bridge::runtime().spawn(async move {
        let cell = current_call();
        let mut guard = cell.lock().await;
        if let Some(call) = guard.as_mut() {
            if let Err(e) = call.republish_mic(was_muted).await {
                eprintln!("[call] mic re-publish (AGC/NS change) failed: {e}");
            }
        }
    });
}

/// The active call's Matrix room id (CallSession holds the LiveKit room, not the Matrix
/// one). Lets leave-on-exit withdraw our MatrixRTC membership without the (gone) QObject.
/// 10.0: the room a switch_call() is heading for while the previous call tears down.
fn pending_switch() -> &'static std::sync::Mutex<Option<String>> {
    static P: std::sync::OnceLock<std::sync::Mutex<Option<String>>> = std::sync::OnceLock::new();
    P.get_or_init(|| std::sync::Mutex::new(None))
}

/// 10.0: the room we are joining / in / switching to — the incoming-call watcher must not ring
/// for our own call. Set synchronously at the top of join_call (before any await).
fn join_target() -> &'static std::sync::Mutex<Option<String>> {
    static T: std::sync::OnceLock<std::sync::Mutex<Option<String>>> = std::sync::OnceLock::new();
    T.get_or_init(|| std::sync::Mutex::new(None))
}

pub fn is_our_call_room(room_id: &str) -> bool {
    active_call_room().lock().unwrap().as_deref() == Some(room_id)
        || join_target().lock().unwrap().as_deref() == Some(room_id)
        || pending_switch().lock().unwrap().as_deref() == Some(room_id)
}

fn active_call_room() -> &'static std::sync::Mutex<Option<String>> {
    static R: OnceLock<std::sync::Mutex<Option<String>>> = OnceLock::new();
    R.get_or_init(|| std::sync::Mutex::new(None))
}

/// The room a join attempt in progress is joining, with that attempt's generation. Owned by a
/// [`JoiningMark`] that lives for the whole `do_join` (set by the newest attempt only, cleared when that
/// attempt returns, success or failure), and it only counts while its generation is current.
fn joining_room() -> &'static std::sync::Mutex<Option<(u64, String)>> {
    static R: OnceLock<std::sync::Mutex<Option<(u64, String)>>> = OnceLock::new();
    R.get_or_init(|| std::sync::Mutex::new(None))
}

struct JoiningMark(u64);
impl JoiningMark {
    fn set(gen: u64, room_id: &str) -> Self {
        let mut m = joining_room().lock().unwrap();
        // a superseded attempt running late never overwrites a newer one's mark
        let newer = m.as_ref().map(|(g, _)| *g > gen).unwrap_or(false);
        if !newer && !join_gen_stale(gen) {
            *m = Some((gen, room_id.to_string()));
        }
        JoiningMark(gen)
    }
}
impl Drop for JoiningMark {
    fn drop(&mut self) {
        let mut m = joining_room().lock().unwrap();
        if m.as_ref().map(|(g, _)| *g == self.0).unwrap_or(false) {
            *m = None;
        }
    }
}

/// The call peers' media keys are accepted for (key_intake / keyrules rule 3), with its join generation:
/// a join in progress (its room — peers start pushing keys the moment our membership lands, a moment
/// before `active_call_room` is set — it is the newest intent, so it wins over a call being left), else
/// the active call. The generation changes with every join and leave, so a held key never crosses from
/// one call into the next, even in the same room. None = no call.
/// The LiveKit focus (broker URL) of the call we are in, for [`republish_membership`].
fn active_focus_url() -> &'static std::sync::Mutex<Option<String>> {
    static F: OnceLock<std::sync::Mutex<Option<String>>> = OnceLock::new();
    F.get_or_init(|| std::sync::Mutex::new(None))
}

/// Pad (pad_ctl.rs, docs/PAD.md): re-publish our call membership NOW — Settings → Calls → Pad changed its `pad1`
/// cap, which would otherwise wait for the 30-min refresh. The same path as that refresh (join_rtc_session keeps the
/// original created_ts, so no peer rotates its key). Nothing outside a call.
pub(crate) async fn republish_membership() {
    let my_gen = join_gen().load(std::sync::atomic::Ordering::SeqCst);
    let Some(rid) = active_call_room().lock().unwrap().clone() else { return };
    if current_call().lock().await.is_none() {
        return;
    }
    let Some(svc) = active_focus_url().lock().unwrap().clone() else { return };
    let Some(client) = crate::session::client() else { return };
    let Some(room) = matrix_sdk::ruma::RoomId::parse(rid.as_str()).ok().and_then(|r| client.get_room(&r)) else { return };
    // bound to this join: a leave / rejoin since (it bumps the generation) must not get a stale membership back
    if active_call_room().lock().unwrap().as_deref() != Some(rid.as_str()) || join_gen_stale(my_gen) {
        return;
    }
    match crate::rtc::join_rtc_session(&room, &svc).await {
        Ok(()) => eprintln!("[call] membership re-published (Pad setting changed)"),
        Err(e) => eprintln!("[call] membership re-publish failed: {e}"),
    }
}

pub(crate) fn key_intake_room() -> Option<(String, u64)> {
    if let Some((gen, room)) = joining_room().lock().unwrap().clone() {
        if !join_gen_stale(gen) {
            return Some((room, gen));
        }
    }
    let room = active_call_room().lock().unwrap().clone()?;
    Some((room, join_gen().load(std::sync::atomic::Ordering::SeqCst)))
}

/// Monotonic join-attempt generation, bumped by every `join_call` AND every `leave_call`.
/// A join attempt whose captured generation is no longer current has been SUPERSEDED
/// (the user cancelled it, and possibly already rejoined) and must not run its
/// destructive cleanup. Field incident ("zombie call"): a cancelled join's
/// 15s connect timeout fired ~6s into the NEXT join of the same room — its
/// `abandon_partial_connect()` nulled the new session's keyring and its membership
/// withdrawal tombstoned the fresh membership. Result: minutes of black/silent peers
/// (every arriving key buffered as call_active=false) while the sending peer's encoder
/// was PLI-stormed into a keyframe every 0.6s. Only a manual rejoin fixed it.
fn join_gen() -> &'static std::sync::atomic::AtomicU64 {
    static G: OnceLock<std::sync::atomic::AtomicU64> = OnceLock::new();
    G.get_or_init(|| std::sync::atomic::AtomicU64::new(0))
}

/// True when `my_gen` is no longer the newest join attempt (a later join_call or
/// leave_call has bumped the generation).
fn join_gen_stale(my_gen: u64) -> bool {
    join_gen().load(std::sync::atomic::Ordering::SeqCst) != my_gen
}

/// Count of call teardowns currently in flight (leave_call's + the terminal-disconnect
/// teardown_call_session can overlap). The `hangingUp` rejoin gate must stay closed until
/// the LAST one finishes: each teardown clearing it unconditionally let the faster one
/// re-open the gate while the slower was still about to null active_call_room/
/// current_media_key — the exact E2EE-killing rejoin race the gate exists to prevent.
fn teardowns_in_flight() -> &'static std::sync::atomic::AtomicU64 {
    static N: OnceLock<std::sync::atomic::AtomicU64> = OnceLock::new();
    N.get_or_init(|| std::sync::atomic::AtomicU64::new(0))
}

/// Instant of the last successful join. Auto-rejoin only fires for a call that had been
/// HEALTHY for a while — a call that dies seconds after joining is a join problem, and
/// auto-retrying that would loop.
fn call_started() -> &'static std::sync::Mutex<Option<std::time::Instant>> {
    static T: OnceLock<std::sync::Mutex<Option<std::time::Instant>>> = OnceLock::new();
    T.get_or_init(|| std::sync::Mutex::new(None))
}

/// Consecutive auto-rejoin attempts; reset whenever a call proves healthy. Capped at
/// [`MAX_AUTO_REJOINS`], after which the manual Rejoin banner takes over.
fn auto_rejoins() -> &'static std::sync::atomic::AtomicU32 {
    static N: OnceLock<std::sync::atomic::AtomicU32> = OnceLock::new();
    N.get_or_init(|| std::sync::atomic::AtomicU32::new(0))
}
const MAX_AUTO_REJOINS: u32 = 2;

/// Pre-drop mic mute, set by the auto-rejoin path and consumed by the next join — a
/// muted user must never be silently rejoined with a HOT mic.
fn rejoin_mic_muted() -> &'static std::sync::Mutex<Option<bool>> {
    static M: OnceLock<std::sync::Mutex<Option<bool>>> = OnceLock::new();
    M.get_or_init(|| std::sync::Mutex::new(None))
}

/// Tear down the active call's session + shared state — close LiveKit (releases camera/mic +
/// disconnects), clear key/registry, and withdraw our MatrixRTC membership. Used by the
/// TERMINAL-DISCONNECT path so a dropped call doesn't leave a zombie capturing session, a ghost
/// `m.call.member` (peers keep seeing us), or stale registries that bleed into the next rejoin.
/// `qt`: when present, `hangingUp` is cleared on the Qt object once teardown finishes —
/// the disconnect path sets it (same gate as leave_call) so a rejoin can't interleave with
/// this teardown; unguarded, the rejoin's freshly-set `active_call_room`/`current_media_key`
/// got clobbered by the lines below, silently killing E2EE for the whole rejoined call.
/// NOTE: callers must `teardowns_in_flight().fetch_add(1)` on the SPAWNING thread before
/// spawning this — counting inside the task leaves a gap where another teardown's final
/// decrement opens the hangingUp gate before this one is first polled.
async fn teardown_call_session(
    room_id: String,
    qt: Option<cxx_qt::CxxQtThread<qobject::CallController>>,
) {
    crate::afk_ctl::clear_intent(); // a torn-down call must not carry away-state into a rejoin
    let taken = current_call().lock().await.take();
    let close_fut = async {
        if let Some(call) = taken {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(10), call.close()).await;
        }
    };
    let leave_fut = async {
        if let Some(client) = crate::session::client() {
            if let Ok(rid) = matrix_sdk::ruma::RoomId::parse(&room_id) {
                if let Some(room) = client.get_room(&rid) {
                    let _ = tokio::time::timeout(
                        std::time::Duration::from_secs(8),
                        crate::rtc::leave_rtc_session(&room),
                    )
                    .await;
                }
            }
        }
    };
    tokio::join!(close_fut, leave_fut);
    *active_call_room().lock().unwrap() = None;
    *current_media_key().lock().unwrap() = None;
    video_registry().lock().unwrap().clear();
    eprintln!("[call] session torn down after disconnect");
    let last = teardowns_in_flight().fetch_sub(1, std::sync::atomic::Ordering::SeqCst) == 1;
    if let Some(qt) = qt {
        // Clear the rejoin gate only when NO other teardown is still in flight (see
        // teardowns_in_flight) — but always then, regardless of how teardown went.
        if last {
            let _ = qt.queue(|mut o| o.as_mut().set_hanging_up(false));
        }
    }
}

fn unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 0.8.28: restart the camera capture off the Qt thread and re-bind the self-view —
/// the same dance the camera-died retry does. `status` shows while the restart runs.
fn spawn_camera_restart(qt: cxx_qt::CxxQtThread<qobject::CallController>, status: &'static str) {
    crate::bridge::runtime().spawn(async move {
        let _ = qt.queue(move |mut o| o.as_mut().set_status(QString::from(status)));
        let cell = current_call();
        let mut guard = cell.lock().await;
        let Some(call) = guard.as_mut() else { return };
        let result = call.restart_camera();
        drop(guard);
        let _ = qt.queue(move |mut o| match result {
            Ok(ptr) => {
                if ptr != 0 {
                    video_registry().lock().unwrap().insert(SELFVIEW_ID.to_string(), ptr);
                    let cur = o.as_ref().video_tiles().to_string();
                    o.as_mut().set_video_tiles(QString::from(&strip_selfview(&cur)));
                    let cur2 = o.as_ref().video_tiles().to_string();
                    o.as_mut().set_video_tiles(QString::from(&inject_selfview(&cur2)));
                    let g = *o.as_ref().selfview_gen();
                    o.as_mut().set_selfview_gen(g.wrapping_add(1));
                }
                o.as_mut().set_status(QString::from("Camera restarted — waiting for video…"));
            }
            Err(e) => {
                o.as_mut().set_camera_on(false);
                o.as_mut().set_status(QString::from(&format!("Camera restart failed: {e}")));
            }
        });
    });
}

impl qobject::CallController {
    pub fn join_call(mut self: Pin<&mut Self>, room_id: &QString) {
        // Block (re)join while a previous hang-up is still tearing down: rejoining before
        // CallSession::close() finishes leaves TWO live participants under our identity
        // (user:device) and the SFU wedges the new join in "connecting" forever.
        if *self.in_call() || *self.connecting() || *self.hanging_up() {
            return;
        }
        let room_id = room_id.to_string();
        *join_target().lock().unwrap() = Some(room_id.clone()); // 10.0: the watcher must not ring for this room
        self.as_mut().set_room_id(QString::from(&room_id));
        self.as_mut().set_connecting(true);
        self.as_mut().set_status(QString::from("Joining call…"));

        // This attempt's generation. Any later join_call/leave_call bump supersedes us:
        // every cleanup/registration below must check join_gen_stale(my_gen) first.
        let my_gen = join_gen().fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        // 10.1 (TH-CALL-02): a plain join starts unmuted, so clear whatever the LAST call left in
        // mic_muted now — from here until the join completes, mic_muted is the user's intent for
        // THIS call (they may press mute while "Connecting…"; the Ok branch honours it).
        if rejoin_mic_muted().lock().unwrap().is_none() {
            self.as_mut().set_mic_muted(false);
        }
        self.as_mut().set_fail_reason(QString::default());
        // RBAC: a new call starts with no moderation state (removedBy is "cleared on the next join").
        self.as_mut().set_removed_by(QString::default());
        self.as_mut().set_server_muted(false);
        self.as_mut().set_server_muted_by(QString::default());
        // 10.5 (Astra 2026-09-22 #2): push-to-talk mutes the mic BEFORE it is published and captured;
        // applied after the join (as first written) the room went out live for the seconds in between.
        let ptt_muted = Self::ptt_target_muted(*self.push_to_talk(), *self.ptt_speaking(), false);
        if ptt_muted {
            self.as_mut().set_mic_muted(true);
        }
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            // Auto-rejoin restores the pre-drop mute; a plain user-initiated join starts
            // unmuted as before (the cell is only ever set by the auto-rejoin path).
            let start_muted = rejoin_mic_muted().lock().unwrap().take().unwrap_or(false) || ptt_muted;
            match do_join(&room_id, start_muted, my_gen).await {
                Ok(mut rx) => {
                    // Superseded between do_join's guarded store and here (the user
                    // cancelled and possibly rejoined): stand down WITHOUT touching
                    // current_call — it may already hold the NEWER attempt's session,
                    // and whoever bumped the generation (leave_call's teardown or the
                    // next join's leftover-session close) owns closing ours.
                    if join_gen_stale(my_gen) {
                        eprintln!("[call] join (gen {my_gen}) superseded after connect — standing down");
                        return;
                    }
                    // Auto-end screen shares whose capture dies on its own — on Windows
                    // that's "the shared window was closed" (WGC posts a bus error).
                    // Without this the share zombie-froze on the last frame while the
                    // UI still said "Sharing" (live friends feedback). Windows-only so
                    // the Linux build's behavior stays byte-identical.
                    if cfg!(windows) {
                        let qt_hook = qt.clone();
                        let hook = std::sync::Arc::new(move || {
                            let qt_hook = qt_hook.clone();
                            crate::bridge::runtime().spawn(async move {
                                let cell = current_call();
                                let mut guard = cell.lock().await;
                                if let Some(call) = guard.as_mut() {
                                    call.stop_screenshare().await;
                                }
                                drop(guard);
                                let _ = qt_hook.queue(|mut o| {
                                    o.as_mut().set_sharing(false);
                                    o.as_mut().set_status(QString::from(
                                        "Screen share ended — the shared window closed",
                                    ));
                                });
                            });
                        });
                        let cell = current_call();
                        let mut guard = cell.lock().await;
                        if let Some(call) = guard.as_mut() {
                            call.set_share_dead_hook(hook);
                        }
                    }
                    // Camera capture died on its own (virtual-cam input hogging — see
                    // camera_dead_hook): retry ONCE after a grace delay so a frame
                    // server (NVIDIA Broadcast) can re-acquire the physical camera we
                    // may have just released. 10s cooldown stops a dead-for-real
                    // camera from retry-looping.
                    {
                        let qt_hook = qt.clone();
                        let hook = std::sync::Arc::new(move || {
                            use std::sync::atomic::{AtomicU64, Ordering};
                            static LAST_RETRY: AtomicU64 = AtomicU64::new(0);
                            let now = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_secs())
                                .unwrap_or(0);
                            if now.saturating_sub(LAST_RETRY.load(Ordering::Relaxed)) < 10 {
                                let qt_give_up = qt_hook.clone();
                                let _ = qt_give_up.queue(|mut o| {
                                    o.as_mut().set_camera_on(false);
                                    o.as_mut().set_status(QString::from(
                                        "Camera feed keeps dying — if this is a virtual camera \
                                         (NVIDIA Broadcast), make sure its input camera isn't in \
                                         use by another app (or by this one).",
                                    ));
                                });
                                return;
                            }
                            LAST_RETRY.store(now, Ordering::Relaxed);
                            let qt_retry = qt_hook.clone();
                            crate::bridge::runtime().spawn(async move {
                                let _ = qt_retry.queue(|mut o| {
                                    o.as_mut().set_status(QString::from(
                                        "Camera feed died — retrying in a moment (virtual cameras \
                                         can take a few seconds to start)…",
                                    ));
                                });
                                tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
                                let cell = current_call();
                                let mut guard = cell.lock().await;
                                let Some(call) = guard.as_mut() else { return };
                                let result = call.restart_camera();
                                drop(guard);
                                // Same selfview re-bind dance as set_camera: a nonzero
                                // ptr means the rebuilt pipeline must be re-attached by
                                // bouncing the self-view tile.
                                let _ = qt_retry.queue(move |mut o| match result {
                                    Ok(ptr) => {
                                        if ptr != 0 {
                                            video_registry()
                                                .lock()
                                                .unwrap()
                                                .insert(SELFVIEW_ID.to_string(), ptr);
                                            let cur = o.as_ref().video_tiles().to_string();
                                            o.as_mut().set_video_tiles(QString::from(&strip_selfview(&cur)));
                                            let cur2 = o.as_ref().video_tiles().to_string();
                                            o.as_mut().set_video_tiles(QString::from(&inject_selfview(&cur2)));
                                            let g = *o.as_ref().selfview_gen();
                                            o.as_mut().set_selfview_gen(g.wrapping_add(1));
                                        }
                                        // Honest wording: restart_camera returning Ok only
                                        // means the pipeline STARTED — no frame has been
                                        // captured yet ("Camera recovered" used to appear
                                        // on a pipeline that died again seconds later).
                                        // A repeat death re-fires the dead hook (now also
                                        // armed on retried pipelines) and lands in the
                                        // "camera keeps dying" branch above.
                                        o.as_mut().set_status(QString::from(
                                            "Camera restarted — waiting for video…",
                                        ));
                                    }
                                    Err(e) => {
                                        o.as_mut().set_camera_on(false);
                                        o.as_mut().set_status(QString::from(&format!(
                                            "Camera retry failed: {e}"
                                        )));
                                    }
                                });
                            });
                        });
                        let cell = current_call();
                        let mut guard = cell.lock().await;
                        if let Some(call) = guard.as_mut() {
                            call.set_camera_dead_hook(hook);
                        }
                    }
                    // 0.8.28: the camera capture went QUIET — frames stopped while
                    // the pipeline still said PLAYING and the keepalive re-pushed a frozen
                    // picture to everyone (an Insta360 webcam: 30 s+ stretches, 27 in one night;
                    // the fix people found was camera off / camera on). Restart the capture
                    // automatically, bounded: one per 20 s, three per 10 minutes — after
                    // that the frozen-frame keepalive stays and the status line says what to
                    // try (USB bandwidth / another app / another port).
                    {
                        let qt_hook = qt.clone();
                        let hook = std::sync::Arc::new(move |quiet_ms: u64| {
                            use std::sync::atomic::{AtomicU64, Ordering};
                            static LAST_AUTO: AtomicU64 = AtomicU64::new(0);
                            static WINDOW_START: AtomicU64 = AtomicU64::new(0);
                            static WINDOW_COUNT: AtomicU64 = AtomicU64::new(0);
                            let now = unix_secs();
                            if now.saturating_sub(LAST_AUTO.load(Ordering::Relaxed)) < 20 {
                                return;
                            }
                            if now.saturating_sub(WINDOW_START.load(Ordering::Relaxed)) > 600 {
                                WINDOW_START.store(now, Ordering::Relaxed);
                                WINDOW_COUNT.store(0, Ordering::Relaxed);
                            }
                            let n = WINDOW_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
                            LAST_AUTO.store(now, Ordering::Relaxed);
                            if n > 3 {
                                eprintln!(
                                    "[camera] capture quiet again ({quiet_ms} ms) — {n} stalls in 10 min, \
                                     not auto-restarting any more (frozen-frame keepalive stays)"
                                );
                                let _ = qt_hook.queue(|mut o| {
                                    o.as_mut().set_status(QString::from(
                                        "Camera keeps stalling — try another USB port or camera, or turn it off and on",
                                    ));
                                });
                                return;
                            }
                            eprintln!(
                                "[camera] capture quiet for {quiet_ms} ms — restarting the capture \
                                 (auto, like camera off/on; {n}/3 this 10 min)"
                            );
                            spawn_camera_restart(qt_hook.clone(), "Camera stalled — restarting its capture…");
                        });
                        let cell = current_call();
                        let mut guard = cell.lock().await;
                        if let Some(call) = guard.as_mut() {
                            call.set_camera_quiet_hook(hook);
                        }
                    }
                    // Auto-resolution refit (both platforms): the stats governor picked
                    // a new rung — re-publish the share through the SAME bounded path a
                    // manual quality change takes, and say so in the status line.
                    {
                        let qt_hook = qt.clone();
                        tinhat_streamer::call::set_share_refit_hook(std::sync::Arc::new(
                            move || {
                                let qt_inner = qt_hook.clone();
                                let _ = qt_inner.queue(|mut o| {
                                    if *o.sharing() {
                                        let (w, h) = {
                                            let r = tinhat_streamer::auto_rung();
                                            (r.0, r.1)
                                        };
                                        o.as_mut().set_status(QString::from(&format!(
                                            "Share auto-fit: adjusting to {w}x{h} for your connection"
                                        )));
                                        o.as_mut().restart_share_if_live();
                                    }
                                });
                            },
                        ));
                    }
                    // Mark the self-view present + publish the authoritative tiles JSON
                    // SYNCHRONOUSLY (off the Qt thread, so neither can be lost if a queued
                    // closure is dropped). The polled `refresh_video_tiles` applies `desired`
                    // to QML, so the grid self-heals even if a closure below never runs.
                    selfview_active().store(true, std::sync::atomic::Ordering::Relaxed);
                    *call_started().lock().unwrap() = Some(std::time::Instant::now());
                    // Our join sound, published into the call we just joined (QOL 1.5.2).
                    // Detached: a slow or missing sound must never delay being IN the call.
                    {
                        let rid = room_id.clone();
                        tokio::spawn(async move { crate::joinsound::play_on_join(&rid).await });
                    }
                    // RBAC: my permissions in this room, computed BEFORE the drain below
                    // processes the seeded ParticipantJoined burst (auto re-enforcement of
                    // peers already present needs canModerateCall to be right by then).
                    let (perms_json, can_moderate) = compute_call_perms(&room_id).await;
                    can_moderate_flag().store(can_moderate, std::sync::atomic::Ordering::Relaxed);
                    // …and every 30 s for the life of this call (roles change mid-call).
                    {
                        let qt_p = qt.clone();
                        let rid = room_id.clone();
                        crate::bridge::runtime().spawn(async move {
                            loop {
                                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                                if current_call().lock().await.is_none()
                                    || active_call_room().lock().unwrap().as_deref() != Some(rid.as_str())
                                    || join_gen_stale(my_gen)
                                {
                                    break;
                                }
                                let (json, can) = compute_call_perms(&rid).await;
                                can_moderate_flag().store(can, std::sync::atomic::Ordering::Relaxed);
                                if qt_p.queue(move |mut o| o.as_mut().apply_call_perms(&json, can)).is_err() {
                                    break;
                                }
                            }
                        });
                    }
                    // RBAC: do_join skipped the mic publish because the room state says a
                    // moderator muted me — mirror that into the UI (button locked, banner).
                    let join_muted_by = tinhat_streamer::server_mute_info(&my_call_identity())
                        .map(|(by, name)| moderator_label(&by, &name));
                    let empty = std::collections::HashSet::new();
                    set_desired_tiles(&tiles_json_enriched(&[], &empty, &empty));
                    let _ = qt.queue(move |mut o| {
                        o.as_mut().set_connecting(false);
                        o.as_mut().set_in_call(true);
                        o.as_mut().apply_call_perms(&perms_json, can_moderate);
                        o.as_mut().set_server_muted(join_muted_by.is_some());
                        o.as_mut().set_server_muted_by(QString::from(join_muted_by.as_deref().unwrap_or("")));
                        // 10.1 (TH-CALL-02): a mute pressed while "Connecting…" was an intent, not a
                        // glitch — keep it (the button showed red; going live unmuted here was the bug).
                        // 10.5: and push-to-talk means muted until the key is held — a join used to
                        // start LIVE with push-to-talk on (nothing applied it until the first Ctrl press).
                        let want_muted = start_muted
                            || *o.mic_muted()
                            || qobject::CallController::ptt_target_muted(*o.push_to_talk(), *o.ptt_speaking(), false);
                        o.as_mut().set_mic_muted(want_muted);
                        if want_muted != start_muted {
                            crate::bridge::runtime().spawn(async move {
                                let cell = current_call();
                                let guard = cell.lock().await;
                                if let Some(call) = guard.as_ref() {
                                    call.set_mic_muted(want_muted);
                                }
                            });
                        }
                        // A successful (re)join clears any prior connection-trouble banner.
                        o.as_mut().set_call_failed(false);
                        o.as_mut().set_reconnecting(false);
                        // Authoritatively NOT away on a fresh join — so a Rejoin to the SAME room
                        // never inherits sticky BRB intent and instantly re-arms the away rising
                        // edge (mute mic / unpublish camera / cut audio = "can't recover").
                        crate::afk_ctl::clear_intent();
                        o.as_mut().set_status(QString::from("In call"));
                        // Show the local self-view tile from the moment we join (avatar
                        // placeholder until the camera turns on), so the user always has a
                        // square representing their presence — not only once they enable video.
                        let empty = std::collections::HashSet::new();
                        o.as_mut().set_video_tiles(QString::from(&tiles_json_enriched(&[], &empty, &empty)));
                        // Device pickers are now populated LAZILY (on the gear-menu open via
                        // onSettingsRequested), NOT eagerly on join. Scanning Audio/Source on
                        // connect probed the AutoMic/StreamFeed filter-chains' degenerate caps and
                        // spewed a burst of GstIntRange CRITICALs every call. Only the cheap
                        // persisted-toggle mirrors run here.
                        o.as_mut().refresh_mic_denoise(); // mirror persisted denoise toggle
                        o.as_mut().refresh_mic_aec(); // mirror persisted echo-cancel toggle
                        o.as_mut().refresh_mic_apm(); // mirror persisted AGC + NS toggles
                        o.as_mut().refresh_audio_sources(); // mirror persisted share-audio sink (Audio/Sink, no flood)
                    });
                    // Install peers' E2EE media keys so their audio/video decrypt
                    // (else black tiles + silence). Runs until the call ends.
                    crate::bridge::runtime().spawn(install_peer_keys_loop(room_id.clone()));
                    // Drain remote-video tile events into the QML grid for the call's
                    // life; ends when the call closes (sender dropped).
                    let qt2 = qt.clone();
                    // (tile id, participant identity, is_screenshare) for every subscribed video.
                    let mut tiles: Vec<(String, String, bool)> = Vec::new();
                    // Connected participant identities — drives an AVATAR tile for camera-off /
                    // voice-only peers (so a voice call no longer shows an empty grid).
                    let mut participants: std::collections::HashSet<String> = std::collections::HashSet::new();
                    // Fresh call: clear a stale REC indicator from a previous session.
                    let _ = qt2.queue(|mut o| o.as_mut().set_rec_active(false));
                    // Identities whose MIC is muted — drives the per-tile mic-off glyph.
                    let mut muted: std::collections::HashSet<String> = std::collections::HashSet::new();
                    // RBAC: peers whose room-state mute we already re-enforced this join (cleared
                    // when they leave, so a rejoin is re-enforced again).
                    let mut enforced: std::collections::HashSet<String> = std::collections::HashSet::new();
                    let my_identity = my_call_identity();
                    // Resolve display names up front so tiles show real names, not raw mxids.
                    crate::bridge::runtime().spawn({
                        let rid = room_id.clone();
                        async move { refresh_call_names(&rid).await; }
                    });
                    while let Some(ev) = rx.recv().await {
                        match ev {
                            CallEvent::VideoAdded { id, participant, is_screenshare, track } => {
                                if is_vod_bot(&participant) {
                                    // 0.8.8: the recorder never renders as a tile (it also
                                    // publishes no video; this is a defensive drop).
                                    drop(track);
                                    let _ = (id, is_screenshare);
                                    continue;
                                }
                                if !tiles.iter().any(|(t, _, _)| t == &id) {
                                    tiles.push((id.clone(), participant.clone(), is_screenshare));
                                }
                                participants.insert(participant.clone());
                                let json = tiles_json_enriched(&tiles, &participants, &muted);
                                tinhat_streamer::call::stash_pending_video_track(&id, track);
                                set_desired_tiles(&json);
                                eprintln!("[call] drain VideoAdded id={id} ss={is_screenshare} ({} tile(s))", tiles.len());
                                let id2 = id.clone();
                                if let Err(e) = qt2.queue(move |mut o| {
                                    // Build the qml6glsink pipeline HERE (GUI thread) so it renders.
                                    if !video_registry().lock().unwrap().contains_key(&id2) {
                                        if let Some(ptr) =
                                            tinhat_streamer::call::create_video_tile_from_pending(&id2)
                                        {
                                            video_registry().lock().unwrap().insert(id2.clone(), ptr);
                                        }
                                    } else {
                                        tinhat_streamer::call::drop_pending_video_track(&id2);
                                    }
                                    o.as_mut().set_video_tiles(QString::from(&json));
                                }) {
                                    eprintln!("[call] queue VideoAdded({id}) failed: {e:?} — poll/lazy will self-heal");
                                }
                            }
                            CallEvent::VideoRemoved { id } => {
                                tiles.retain(|(t, _, _)| t != &id);
                                let json = tiles_json_enriched(&tiles, &participants, &muted);
                                tinhat_streamer::call::drop_pending_video_track(&id);
                                set_desired_tiles(&json);
                                if let Err(e) = qt2.queue(move |mut o| {
                                    // Teardown ORDER matters (avoids a crash on leave): unbind the
                                    // qml6glsink widget while the QML item still exists, THEN abort
                                    // the feeder + NULL the pipeline, THEN drop the tile from the
                                    // model so QML destroys the delegate/item last.
                                    if let Some(ptr) = video_registry().lock().unwrap().get(&id).copied() {
                                        crate::video::predetach(ptr);
                                    }
                                    tinhat_streamer::call::remove_video_tile(&id);
                                    video_registry().lock().unwrap().remove(&id);
                                    o.as_mut().set_video_tiles(QString::from(&json));
                                }) {
                                    eprintln!("[call] queue VideoRemoved failed: {e:?}");
                                }
                            }
                            CallEvent::ParticipantJoined { identity } => {
                                if is_vod_bot(&identity) {
                                    // Indicator, not a person (0.8.8). STILL announce our media
                                    // key to it — that is how it can decrypt (and record) us.
                                    crate::bridge::runtime().spawn({
                                        let rid = room_id.clone();
                                        let id = identity.clone();
                                        async move { announce_key_to_peer(rid, id).await; }
                                    });
                                    let _ = qt2.queue(|mut o| o.as_mut().set_rec_active(true));
                                    continue;
                                }
                                participants.insert(identity.clone());
                                // RBAC (§5.4): if the call room's state says this person is
                                // server-muted and I may moderate, re-apply it — the SFU forgot
                                // their permission when they left. Once per identity per join.
                                if can_moderate_flag().load(std::sync::atomic::Ordering::Relaxed)
                                    && enforced.insert(identity.clone())
                                {
                                    let qt3 = qt2.clone();
                                    let rid = room_id.clone();
                                    let id = identity.clone();
                                    crate::bridge::runtime().spawn(async move {
                                        if crate::moderation::read_muted_by(&rid, &mxid_of(&id)).await.is_some() {
                                            eprintln!("[moderation] re-enforcing server mute on {id} (room state says muted)");
                                            moderate_async(qt3, rid, "mute", id, false).await;
                                        }
                                    });
                                }
                                // Perms are cheap (local store) — keep canModerateCall fresh on join events.
                                {
                                    let qt3 = qt2.clone();
                                    let rid = room_id.clone();
                                    crate::bridge::runtime().spawn(async move {
                                        let (json, can) = compute_call_perms(&rid).await;
                                        can_moderate_flag().store(can, std::sync::atomic::Ordering::Relaxed);
                                        let _ = qt3.queue(move |mut o| o.as_mut().apply_call_perms(&json, can));
                                    });
                                }
                                // (Re)announce our media key to this peer over to-device (so a peer
                                // who joins AFTER us can decrypt our media) + refresh names.
                                crate::bridge::runtime().spawn({
                                    let rid = room_id.clone();
                                    let id = identity.clone();
                                    async move {
                                        refresh_call_names(&rid).await;
                                        announce_key_to_peer(rid, id).await;
                                    }
                                });
                                let json = tiles_json_enriched(&tiles, &participants, &muted);
                                set_desired_tiles(&json);
                                let id_sig = identity.clone();
                                let _ = qt2.queue(move |mut o| {
                                    o.as_mut().set_video_tiles(QString::from(&json));
                                    o.as_mut().participant_joined(QString::from(&id_sig)); // -> join chime
                                });
                            }
                            CallEvent::ParticipantLeft { identity } => {
                                if is_vod_bot(&identity) {
                                    let _ = qt2.queue(|mut o| o.as_mut().set_rec_active(false));
                                    continue;
                                }
                                participants.remove(&identity);
                                muted.remove(&identity);
                                enforced.remove(&identity);
                                let json = tiles_json_enriched(&tiles, &participants, &muted);
                                set_desired_tiles(&json);
                                let id_sig = identity.clone();
                                let _ = qt2.queue(move |mut o| {
                                    o.as_mut().set_video_tiles(QString::from(&json));
                                    o.as_mut().participant_left(QString::from(&id_sig)); // -> leave chime
                                });
                            }
                            CallEvent::TrackMuteChanged { identity, category, muted: m } => {
                                if category == "mic" {
                                    if m { muted.insert(identity); } else { muted.remove(&identity); }
                                    let json = tiles_json_enriched(&tiles, &participants, &muted);
                                    // MUST also publish to desired_tiles, else the 120ms recovery
                                    // poll re-pushes the pre-mute JSON and the glyph flickers off.
                                    set_desired_tiles(&json);
                                    let _ = qt2.queue(move |mut o| o.as_mut().set_video_tiles(QString::from(&json)));
                                }
                            }
                            CallEvent::ServerMuteChanged { identity, muted: m, by, by_name } => {
                                // RBAC: the tile shields come from the streamer's map (already
                                // updated) — re-render + bump the rev. For ME: lock/unlock the
                                // mic button; on unlock, go live again unless the user had muted
                                // themselves (Discord behaviour). The SFU dropped our mic track
                                // with the mute, so "live again" means a (re)publish.
                                let mine = identity == my_identity;
                                let label = moderator_label(&by, &by_name);
                                let json = tiles_json_enriched(&tiles, &participants, &muted);
                                set_desired_tiles(&json);
                                eprintln!(
                                    "[moderation] {identity} server-{} by {label}{}",
                                    if m { "muted" } else { "unmuted" },
                                    if mine { " (me)" } else { "" }
                                );
                                let _ = qt2.queue(move |mut o| {
                                    o.as_mut().set_video_tiles(QString::from(&json));
                                    let r = *o.moderation_rev();
                                    o.as_mut().set_moderation_rev(r.wrapping_add(1));
                                    if !mine {
                                        return;
                                    }
                                    let was = *o.server_muted();
                                    o.as_mut().set_server_muted(m);
                                    o.as_mut().set_server_muted_by(QString::from(if m { label.as_str() } else { "" }));
                                    if m || !was {
                                        return;
                                    }
                                    let want = qobject::CallController::ptt_target_muted(
                                        *o.push_to_talk(),
                                        *o.ptt_speaking(),
                                        *o.mic_muted(),
                                    );
                                    crate::bridge::runtime().spawn(async move {
                                        let cell = current_call();
                                        let mut guard = cell.lock().await;
                                        let Some(call) = guard.as_mut() else { return };
                                        let r = if call.has_mic() {
                                            call.republish_mic(want).await
                                        } else {
                                            call.publish_mic_muted(want).await
                                        };
                                        match r {
                                            Ok(()) => eprintln!("[moderation] mic back after server unmute (muted={want})"),
                                            Err(e) => eprintln!("[moderation] mic re-publish after server unmute failed: {e}"),
                                        }
                                    });
                                });
                            }
                            CallEvent::RemovedByModerator { by, by_name } => {
                                // RBAC: the terminal ConnectionState follows at once and reads
                                // removedBy for its banner text.
                                let label = moderator_label(&by, &by_name);
                                eprintln!("[moderation] {label} disconnected me from the call");
                                let _ = qt2.queue(move |mut o| o.as_mut().set_removed_by(QString::from(&label)));
                            }
                            CallEvent::ConnectionState { reconnecting, disconnected, recoverable } => {
                                if !reconnecting && !disconnected {
                                    // 10.5 (audio review #6): the SDK reports Reconnected even when
                                    // republishing our mic failed — look, and publish it again.
                                    crate::bridge::runtime().spawn(async move {
                                        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                                        let cell = current_call();
                                        let mut guard = cell.lock().await;
                                        if let Some(call) = guard.as_mut() {
                                            match tokio::time::timeout(std::time::Duration::from_secs(6), call.ensure_mic()).await {
                                                Ok(Ok(true)) => eprintln!("[call] after the reconnect the mic was not published — published it again"),
                                                Ok(Ok(false)) => {}
                                                Ok(Err(e)) => eprintln!("[call] after the reconnect the mic could not be published again: {e:#}"),
                                                Err(_) => eprintln!("[call] after the reconnect, publishing the mic again took over 6 s — the healer retries"),
                                            }
                                        }
                                    });
                                }
                                let rid = room_id.clone();
                                // Auto-rejoin: an unexpected terminal drop of a previously-
                                // HEALTHY call heals itself with a FRESH join (fresh SFU token)
                                // instead of parking on the Rejoin banner. This is the fix for
                                // the observed 10-minute failure mode: the SDK's in-place
                                // reconnect re-presents the ORIGINAL join token, which our SFU
                                // mints with exp=10min — so any signal drop later than that
                                // 401s forever. A full rejoin mints a new token. Guard rails:
                                //   * `recoverable` — never force our way back after a kick /
                                //     duplicate identity / deleted room (server said leave).
                                //   * call_started()==None — leave_call ran; the user hung up
                                //     while this event was in flight. Never resurrect that.
                                //   * first attempt requires 45s of healthy call (a call that
                                //     dies young is a JOIN problem, not a link blip); follow-up
                                //     attempts are capped at MAX_AUTO_REJOINS before parking on
                                //     the manual Rejoin banner.
                                let auto_rejoin = if disconnected && recoverable {
                                    match *call_started().lock().unwrap() {
                                        None => false, // user already hung up — stay hung up
                                        Some(t) => {
                                            let healthy = t.elapsed() >= std::time::Duration::from_secs(45);
                                            if healthy {
                                                auto_rejoins().store(0, std::sync::atomic::Ordering::SeqCst);
                                            }
                                            let n = auto_rejoins().fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                                            n <= MAX_AUTO_REJOINS && (healthy || n > 1)
                                        }
                                    }
                                } else {
                                    false
                                };
                                if disconnected && !auto_rejoin {
                                    // This call is over for good — don't let its clock leak
                                    // eligibility into anything later.
                                    *call_started().lock().unwrap() = None;
                                }
                                let queued = qt2.queue(move |mut o| {
                                    o.as_mut().set_reconnecting(reconnecting);
                                    if disconnected {
                                        // Preserve the user's mute across the auto-rejoin: a
                                        // muted user must never be silently rejoined HOT.
                                        if auto_rejoin {
                                            *rejoin_mic_muted().lock().unwrap() = Some(*o.mic_muted());
                                        }
                                        // Terminal drop: replicate leave_call's GUI-side teardown
                                        // ORDER (predetach every sink while the QML items still
                                        // exist, THEN NULL the pipelines, THEN drop the tiles from
                                        // the model) BEFORE the async teardown runs — spawning
                                        // teardown_call_session first let close() free the tile
                                        // pipelines off the Qt thread while video_registry still
                                        // held their pointers (use-after-free on attach/pause).
                                        for (_k, &ptr) in video_registry().lock().unwrap().iter() {
                                            crate::video::predetach(ptr);
                                        }
                                        tinhat_streamer::call::clear_video_tiles();
                                        tinhat_streamer::call::clear_pending_video_tracks();
                                        video_registry().lock().unwrap().clear();
                                        set_desired_tiles("[]");
                                        o.as_mut().set_video_tiles(QString::from("[]"));
                                        selfview_active().store(false, std::sync::atomic::Ordering::Relaxed);
                                        o.as_mut().set_camera_on(false);
                                        // call_failed drives the red "Call ended — Rejoin" banner.
                                        // While auto-rejoin is working, show the RECONNECTING state
                                        // instead — the banner appears only if we give up (the
                                        // rejoin chain / join-failure path sets it back).
                                        o.as_mut().set_call_failed(!auto_rejoin);
                                        o.as_mut().set_reconnecting(auto_rejoin);
                                        o.as_mut().set_in_call(false);
                                        o.as_mut().set_connecting(false);
                                        // Gate rejoin until the teardown below completes (same
                                        // hangingUp gate as leave_call): a rejoin interleaved with
                                        // teardown had its active_call_room/current_media_key
                                        // clobbered — E2EE silently dead for the rejoined call.
                                        o.as_mut().set_hanging_up(true);
                                        let removed = o.removed_by().to_string();
                                        o.as_mut().set_status(QString::from(&if !removed.is_empty() {
                                            format!("{removed} disconnected you from the call")
                                        } else if auto_rejoin {
                                            "Connection lost — reconnecting…".to_string()
                                        } else {
                                            "Call ended (connection lost)".to_string()
                                        }));
                                        o.as_mut().clear_moderation_view(); // RBAC: per-call state
                                        // LAST: actually TEAR DOWN the session (release the camera/
                                        // mic + withdraw membership) so we don't zombie-capture or
                                        // leave a ghost participant. Spawned from inside this closure
                                        // so close() always finds the tiles already cleared (no-op).
                                        // If eligible, chain the auto-rejoin AFTER teardown fully
                                        // completes (same hangingUp gate the Rejoin button honors).
                                        let qt3 = o.qt_thread();
                                        // Counted at SPAWN time (not inside the task) so the
                                        // hangingUp gate can never clear in the gap before the
                                        // scheduler first polls this teardown.
                                        teardowns_in_flight().fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                                        crate::bridge::runtime().spawn(async move {
                                            teardown_call_session(rid.clone(), Some(qt3.clone())).await;
                                            if !auto_rejoin {
                                                return;
                                            }
                                            // Brief pause so the SFU forgets the old participant
                                            // before the same identity joins again.
                                            tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
                                            let n = auto_rejoins().load(std::sync::atomic::Ordering::SeqCst);
                                            eprintln!("[call] auto-rejoin (attempt {n}) after unexpected disconnect");
                                            let _ = qt3.queue(move |mut o| {
                                                // Still parked? The user may have rejoined manually,
                                                // joined elsewhere, hung up during the pause
                                                // (call_started None), or another teardown may run.
                                                if !o.in_call()
                                                    && !o.connecting()
                                                    && !o.hanging_up()
                                                    && call_started().lock().unwrap().is_some()
                                                {
                                                    o.as_mut().join_call(&QString::from(&rid));
                                                } else {
                                                    // Something else took over — stop claiming to
                                                    // reconnect (whatever ran owns the state now),
                                                    // and drop the un-consumed mute restore so it
                                                    // can't leak into an unrelated later join.
                                                    o.as_mut().set_reconnecting(false);
                                                    *rejoin_mic_muted().lock().unwrap() = None;
                                                }
                                            });
                                        });
                                    } else if !reconnecting {
                                        o.as_mut().set_call_failed(false);
                                    }
                                });
                                if disconnected && queued.is_err() {
                                    // Qt engine gone (shutdown) — the teardown must still run so
                                    // the camera is released and our membership withdrawn.
                                    teardowns_in_flight().fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                                    crate::bridge::runtime().spawn(teardown_call_session(room_id.clone(), None));
                                }
                            }
                        }
                    }
                    set_desired_tiles("[]");
                    let _ = qt2.queue(|mut o| {
                        o.as_mut().set_video_tiles(QString::from("[]"));
                        // Drain ended (call closed) — clear reconnecting, but NOT call_failed:
                        // the drain also ends when the terminal-disconnect teardown aborts the
                        // event task, and clearing here dismissed the "Call ended — Rejoin"
                        // banner milliseconds after it appeared. A successful (re)join clears
                        // call_failed itself.
                        o.as_mut().set_reconnecting(false);
                    });
                }
                Err(e) => {
                    eprintln!("[call] join failed: {e}");
                    // A superseded attempt's late failure must not clobber the LIVE
                    // join's UI ("Join failed" toast + connecting=false while the newer
                    // attempt is mid-connect — the misleading toast from 2026-08-19).
                    if join_gen_stale(my_gen) {
                        return;
                    }
                    let reason = join_fail_reason(&e);
                    let _ = qt.queue(move |mut o| {
                        o.as_mut().set_connecting(false);
                        // 10.1 (TH-CALL-03): EVERY failed join keeps the call panel up with the
                        // banner ("Couldn't join" + Try again). It used to vanish behind a 4 s
                        // toast carrying a raw error — "I clicked the call button and nothing
                        // happened". The auto-rejoin case already worked this way.
                        o.as_mut().set_reconnecting(false);
                        o.as_mut().set_call_failed(true);
                        o.as_mut().set_fail_reason(QString::from(&reason));
                        o.as_mut().set_status(QString::from(&format!("Couldn't join the call — {reason}")));
                    });
                }
            }
        });
    }

    pub fn switch_call(mut self: Pin<&mut Self>, room_id: &QString) {
        let target = room_id.to_string();
        if target.is_empty() {
            return;
        }
        if !*self.in_call() && !*self.connecting() && !*self.hanging_up() {
            self.as_mut().join_call(room_id);
            return;
        }
        if *self.in_call() && self.room_id().to_string() == target {
            return; // already there
        }
        eprintln!("[call] switch requested -> {target} (leaving the current call first)");
        *pending_switch().lock().unwrap() = Some(target);
        if *self.hanging_up() {
            return; // a teardown is already in flight — it will pick the target up
        }
        self.as_mut().leave_call();
    }

    pub fn leave_call(mut self: Pin<&mut Self>) {
        if *self.hanging_up() {
            return; // a teardown is already in flight — don't spawn a duplicate
        }
        // Supersede any in-flight join attempt: its timeout/error cleanup checks
        // join_gen_stale and stands down instead of destroying whatever call state the
        // user moves to next (the zombie-call incident, see join_gen).
        join_gen().fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        // A deliberate hang-up is not a failure: forget the health clock, any pending
        // auto-rejoin budget, and any un-consumed mute restore so the next call starts
        // with a clean slate.
        *call_started().lock().unwrap() = None;
        auto_rejoins().store(0, std::sync::atomic::Ordering::SeqCst);
        *rejoin_mic_muted().lock().unwrap() = None;
        self.as_mut().set_in_call(false);
        self.as_mut().set_sharing(false);
        self.as_mut().set_connecting(false);
        // 10.1: hanging up is also how you dismiss a "Couldn't join" / "connection lost" banner.
        self.as_mut().set_call_failed(false);
        self.as_mut().set_fail_reason(QString::default());
        // hangingUp gates rejoin (see join_call): the LiveKit LeaveRequest + membership
        // withdrawal below are async, and rejoining before they land orphans a ghost
        // participant that wedges the next "connecting". Cleared when teardown completes.
        self.as_mut().set_hanging_up(true);
        self.as_mut().set_deafened(false); // a fresh call starts un-deafened
        self.as_mut().set_game_chat(false); // …and out of game chat (per-call)
        self.as_mut().clear_moderation_view(); // RBAC: serverMuted / callPerms are per-call
        tinhat_streamer::set_local_game_chat(false);
        crate::afk_ctl::clear_intent(); // don't carry BRB into the next call (rejoin would re-arm)
        self.as_mut().set_status(QString::from("Hanging up…"));
        // Teardown order (avoids the participant-leave crash): unbind EVERY tile's
        // qml6glsink widget while the QML items still exist, THEN NULL the pipelines, THEN
        // drop the tiles from the model so QML destroys the delegates last. This MUST
        // include the self-view: when the camera is on, its registry ptr is the live camera
        // pipeline (still PLAYING) whose sink widget points at the self-view item — that
        // item is destroyed synchronously by set_video_tiles("[]") below, while the camera
        // pipeline is only NULLed later (async) in CallSession::close(); unbinding here
        // closes the same sink/QQuickItem race for a camera-on leave.
        for (_k, &ptr) in video_registry().lock().unwrap().iter() {
            crate::video::predetach(ptr);
        }
        tinhat_streamer::call::clear_video_tiles(); // abort feeders + NULL pipelines
        tinhat_streamer::call::clear_pending_video_tracks(); // drop any unbuilt tiles
        // Clear the registry HERE (sync, Qt thread) — the pipelines above are already freed,
        // so attach/pause must not see the stale ptrs during the async close() below.
        video_registry().lock().unwrap().clear();
        set_desired_tiles("[]"); // stop the poll from re-publishing stale tiles
        self.as_mut().set_video_tiles(QString::from("[]")); // QML destroys delegates last
        selfview_active().store(false, std::sync::atomic::Ordering::Relaxed);
        self.as_mut().set_camera_on(false);
        self.as_mut().set_music_playing(false); // CallSession::close tears the music track down
        let room_id = self.room_id().to_string();
        let qt = self.qt_thread();
        // Counted at SPAWN time (not as the task's first statement): another teardown's
        // final decrement landing before the scheduler first polls this task would see
        // count==0, compute last=true, and open the hangingUp gate while THIS teardown
        // hasn't even started.
        teardowns_in_flight().fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let handle = crate::bridge::runtime().spawn(async move {
            let cell = current_call();
            let taken = cell.lock().await.take(); // releases the mutex immediately
            // No stored session = the user cancelled a join that was still connecting.
            // Its half-built globals (aux runtime/keyring/AEC — set by the in-flight
            // connect) are cleaned HERE, deterministically, because the superseded
            // attempt's own timeout cleanup now stands down (join_gen guard). Idempotent
            // when there is genuinely nothing to clean.
            if taken.is_none() {
                tinhat_streamer::abandon_partial_connect();
            }

            // Run the LiveKit teardown (close) and the Matrix membership withdrawal
            // CONCURRENTLY. The phone enumerates participants from the m.call.member state
            // event (NOT the SFU), so the withdrawal is what actually removes the ghost — it
            // must NOT be starved by a slow close(). Independent timeouts; both bounded so
            // hangingUp always clears.
            let close_fut = async {
                if let Some(call) = taken {
                    if tokio::time::timeout(std::time::Duration::from_secs(12), call.close())
                        .await
                        .is_err()
                    {
                        eprintln!("[call] WARN: close() exceeded 12s — proceeding with teardown");
                    }
                }
            };
            let leave_fut = async {
                if let Some(client) = crate::session::client() {
                    if let Ok(rid) = matrix_sdk::ruma::RoomId::parse(&room_id) {
                        if let Some(room) = client.get_room(&rid) {
                            match tokio::time::timeout(
                                std::time::Duration::from_secs(8),
                                crate::rtc::leave_rtc_session(&room),
                            )
                            .await
                            {
                                Ok(Ok(())) => eprintln!("[call] membership withdrawn"),
                                Ok(Err(e)) => eprintln!("[call] membership withdrawal failed: {e}"),
                                Err(_) => eprintln!("[call] membership withdrawal timed out"),
                            }
                        }
                    }
                }
            };
            tokio::join!(close_fut, leave_fut);

            *active_call_room().lock().unwrap() = None;
            *current_media_key().lock().unwrap() = None;
            video_registry().lock().unwrap().clear();
            eprintln!("[call] hangup teardown complete — rejoin unblocked");
            // Clear hangingUp only when no OTHER teardown is still in flight (a terminal
            // disconnect's teardown can overlap this one; the gate must outlive BOTH) —
            // but always then, regardless of how teardown went, so the UI never wedges.
            let last = teardowns_in_flight().fetch_sub(1, std::sync::atomic::Ordering::SeqCst) == 1;
            let _ = qt.queue(move |mut o| {
                if last {
                    o.as_mut().set_hanging_up(false);
                    // 10.0 call switch: the pending target joins the moment the teardown is done.
                    if let Some(target) = pending_switch().lock().unwrap().take() {
                        eprintln!("[call] switching into {target}");
                        o.as_mut().join_call(&QString::from(&target));
                        return;
                    }
                }
                o.as_mut().set_status(QString::from("Left call"));
            });
        });
        // Record the in-flight teardown so app-close (leave_on_exit) can wait for it to finish
        // (its SFU LeaveRequest in particular) rather than racing it to process exit.
        *teardown_handle().lock().unwrap() = Some(handle);
    }

    /// Set + persist the screen-share quality; mirror it back to `shareQuality`. If a
    /// share is LIVE, re-publish it at the new quality on the fly (no Stop/Start needed).
    pub fn apply_share_quality(
        mut self: Pin<&mut Self>,
        max_w: u32,
        max_h: u32,
        fps: u32,
        bitrate_kbps: u32,
    ) {
        tinhat_streamer::set_share_quality(max_w, max_h, fps, bitrate_kbps);
        let label = tinhat_streamer::share_quality_label();
        self.as_mut().set_share_quality(QString::from(&label));
        self.as_mut().restart_share_if_live();
    }

    /// TWO-PHASE test, because one number could never answer the question users
    /// actually have ("how good can my share be?"):
    ///
    ///   phase 1 — LINK: a plain HTTPS upload to the server's discard sink. No codec
    ///             involved, so it measures the uplink itself.
    ///   phase 2 — MEDIA: the 20s LiveKit probe from a SCRATCH room
    ///             (`tinhat-conntest-<localpart>-<suffix>`, never a real call room),
    ///             publishing incompressible snow at a 50 Mbps cap.
    ///
    /// Reporting both is the point. The media probe alone is NOT a link measurement:
    /// congestion control only raises its estimate while the sender actually uses its
    /// allocation, so on a machine that cannot encode fast enough the estimate stalls
    /// and the result describes the ENCODER, not the connection (measured: 5.7 Mbps
    /// reported across a path with 33 Gbps of real headroom). With both
    /// numbers we can name the limiter instead of misattributing it to the network.
    ///
    /// Refuses while in a call — the probe would fight the call for the uplink and
    /// poison both measurements.
    pub fn run_connection_test(mut self: Pin<&mut Self>) {
        if *self.in_call() {
            self.as_mut()
                .set_conn_test(QString::from("End the call first — the test needs the line to itself."));
            return;
        }
        self.as_mut()
            .set_conn_test(QString::from("Connection test: measuring your uplink…"));
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            macro_rules! fail {
                ($qt:expr, $msg:expr) => {{
                    let m: String = $msg;
                    let _ = $qt.queue(move |mut o| {
                        o.as_mut()
                            .set_conn_test(QString::from(&format!("Connection test failed: {m}")));
                    });
                    return;
                }};
            }
            // ---- phase 1: raw uplink (no codec in the way) ----------------------
            let link_mbps = uplink_probe().await;
            if let Some(m) = link_mbps {
                let _ = qt.queue(move |mut o| {
                    o.as_mut().set_conn_test(QString::from(&format!(
                        "Uplink {m:.1} Mbps · now measuring the media path (20-45s, until it plateaus)…"
                    )));
                });
            }

            let Some(client) = crate::session::client() else {
                fail!(qt, "not logged in".to_string());
            };
            let localpart: String = client
                .user_id()
                .map(|u| u.localpart().to_string())
                .unwrap_or_else(|| "user".into())
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                .collect();
            // Unique scratch room so two friends testing at once never subscribe each
            // other's 50 Mbps snow.
            let suffix = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() % 100_000)
                .unwrap_or(0);
            let probe_room = format!("tinhat-conntest-{localpart}-{suffix}");
            // the server's own call service, as a call would use (Tin Hat Web's connection test: resolveFocus)
            let focus_url = crate::rtc::focus_service_url(&client).await;
            let cfg = match crate::livekit::request_sfu_config(
                &client,
                &focus_url,
                &probe_room,
            )
            .await
            {
                Ok(c) => c,
                Err(e) => {
                    fail!(qt, format!("token broker refused the probe room ({e})"));
                }
            };
            let _ = qt.queue(|mut o| {
                o.as_mut().set_conn_test(QString::from(
                    "Connection test: probing until the estimate plateaus (20-45s, cap 100 Mbps)…",
                ));
            });
            match tinhat_streamer::connection_probe(&cfg.url, &cfg.jwt, 20).await {
                Ok(r) => {
                    // What the media path actually achieved. `peak_grant` can never
                    // exceed what we managed to send, so this is the SHARE ceiling —
                    // encoder and link together, whichever gave out first.
                    let media = r.peak_grant_kbps as f64 / 1000.0;
                    // The probe caps its own traffic (scratch room, bounded test). A
                    // result pinned at that cap is a FLOOR, not a measurement — the
                    // old verdict compared it against a multi-hundred-Mbps LAN uplink
                    // and could therefore never say anything except "limited by this
                    // computer's encoding" on a fast link, no matter the hardware
                    // (a 9950X3D + 10G user indicted by arithmetic). Within 8%
                    // of the cap ⇒ both sides out-ran the probe; say so.
                    let cap = tinhat_streamer::PROBE_CAP_BPS as f64 / 1_000_000.0;
                    let capped = media >= cap * 0.92;
                    // The probe runs to plateau (45s wall). If the grant was STILL
                    // climbing at the wall, `media` is a floor — the path outlasted
                    // the test, and naming a limiter would be that same lie again.
                    let rising = r.still_rising && !capped;
                    // Name the limiter. If the raw uplink is comfortably above what the
                    // media path reached, the ENCODER — named, so the reader can judge
                    // (software VP9 topping out ≠ NVENC topping out) — is the cap.
                    // An 8 MB sample cannot resolve a very fast link — on a LAN it
                    // finishes in ~20ms and the arithmetic yields silly figures like
                    // "2929 Mbps". Past a point, say "fast" instead of a number we
                    // cannot stand behind; the verdict only needs link vs media.
                    let link_str = |m: f64| {
                        if m >= 200.0 { "200+ Mbps".to_string() } else { format!("{m:.0} Mbps") }
                    };
                    let enc = r.encoder_label;
                    let verdict = match link_mbps {
                        Some(link) if capped => format!(
                            "Uplink {} · share pushed {media:.0} Mbps — the probe's own \
                             {cap:.0} Mbps ceiling; {enc} and your connection both have \
                             headroom beyond every share preset",
                            link_str(link)
                        ),
                        Some(link) if rising => format!(
                            "Uplink {} · share reached {media:.0} Mbps and was still \
                             climbing when the test ended — the real ceiling is higher; \
                             {enc} kept up the whole way",
                            link_str(link)
                        ),
                        Some(link) if link > media * 1.5 => format!(
                            "Uplink {} · share tops out at {media:.1} Mbps — limited by \
                             the {enc} encoder, not your connection",
                            link_str(link)
                        ),
                        Some(link) => format!(
                            "Uplink {} · share tops out at {media:.1} Mbps — \
                             limited by your connection",
                            link_str(link)
                        ),
                        // Uplink probe unavailable (offline sink / blocked): report the
                        // media number alone rather than guessing at a cause.
                        None if capped => format!(
                            "Share pushed {media:.0} Mbps — the probe's own {cap:.0} Mbps \
                             ceiling ({enc})"
                        ),
                        None if rising => format!(
                            "Share reached {media:.0} Mbps, still climbing at test end ({enc})"
                        ),
                        None => format!("Share tops out at {media:.1} Mbps ({enc})"),
                    };
                    let msg = format!(
                        "{verdict} · sustained {:.1} · estimator {:.1} · {}",
                        r.tail_grant_kbps as f64 / 1000.0,
                        r.bwe_kbps as f64 / 1000.0,
                        r.path
                    );
                    eprintln!("[conntest] RESULT: {msg}");
                    let _ = qt.queue(move |mut o| {
                        o.as_mut().set_conn_test(QString::from(&msg));
                    });
                }
                Err(e) => fail!(qt, e.to_string()),
            }
        });
    }

    /// Set + persist the screen-share encoder; mirror the label back to `shareQuality`. If
    /// a share is LIVE, re-publish it with the new encoder on the fly.
    pub fn apply_share_encoder(mut self: Pin<&mut Self>, encoder: i32) {
        tinhat_streamer::set_share_encoder(encoder);
        let label = tinhat_streamer::share_quality_label();
        self.as_mut().set_share_quality(QString::from(&label));
        self.as_mut().restart_share_if_live();
    }

    /// If a screen-share is currently LIVE, rebuild + re-publish it with the CURRENT
    /// (just-persisted) quality/encoder, reusing the open portal session (no re-prompt).
    /// NO-OP if not sharing. Mirrors `set_input`'s restart_mic dispatch: spawn on the
    /// runtime, lock the call, call `restart_screenshare` (bounded internally). On failure
    /// the old share is already torn down, so flip `sharing` off and surface the error.
    fn restart_share_if_live(self: Pin<&mut Self>) {
        if !*self.sharing() {
            return;
        }
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let cell = current_call();
            let mut guard = cell.lock().await;
            let Some(call) = guard.as_mut() else { return };
            match call.restart_screenshare().await {
                Ok(()) => {
                    let _ = qt.queue(|mut o| o.as_mut().set_status(QString::from("Sharing screen")));
                }
                Err(e) => {
                    eprintln!("[call] share quality re-apply failed: {e}");
                    let _ = qt.queue(move |mut o| {
                        o.as_mut().set_sharing(false);
                        o.as_mut().set_status(QString::from(&format!("Share restart failed: {e}")));
                    });
                }
            }
        });
    }

    /// Load the persisted screen-share quality into the `shareQuality` qproperty.
    pub fn refresh_share_quality(mut self: Pin<&mut Self>) {
        let label = tinhat_streamer::share_quality_label();
        self.as_mut().set_share_quality(QString::from(&label));
    }

    /// Set + persist the CAMERA quality; mirror it back to `camQuality`. A live camera is
    /// fully RE-PUBLISHED (encoding + simulcast layers are locked at publish time — the
    /// capture-only `restart_camera` can't change them).
    pub fn apply_cam_quality(
        mut self: Pin<&mut Self>,
        w: u32,
        h: u32,
        fps: u32,
        bitrate_kbps: u32,
    ) {
        tinhat_streamer::camera::set_cam_quality(w, h, fps, bitrate_kbps);
        let label = tinhat_streamer::camera::cam_quality_label();
        self.as_mut().set_cam_quality(QString::from(&label));
        self.as_mut().republish_camera_if_live();
    }

    /// Set + persist camera simulcast; mirror the label; re-publish a live camera.
    pub fn apply_cam_simulcast(mut self: Pin<&mut Self>, on: bool) {
        tinhat_streamer::camera::set_cam_simulcast(on);
        let label = tinhat_streamer::camera::cam_quality_label();
        self.as_mut().set_cam_quality(QString::from(&label));
        self.as_mut().republish_camera_if_live();
    }

    /// Load the persisted camera quality into the `camQuality` qproperty.
    pub fn refresh_cam_quality(mut self: Pin<&mut Self>) {
        let label = tinhat_streamer::camera::cam_quality_label();
        self.as_mut().set_cam_quality(QString::from(&label));
    }

    /// If the camera is LIVE, unpublish + re-publish it under the CURRENT (just-persisted)
    /// quality/simulcast options, then re-bind the self-view (registry insert + tile bounce,
    /// mirroring the set_camera re-point flow). NO-OP when the camera is off.
    fn republish_camera_if_live(self: Pin<&mut Self>) {
        if !*self.camera_on() {
            return;
        }
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let cell = current_call();
            let mut guard = cell.lock().await;
            let Some(call) = guard.as_mut() else { return };
            call.unpublish_camera().await;
            match call.publish_camera(None, true).await {
                Ok(ptr) => {
                    if ptr != 0 {
                        video_registry().lock().unwrap().insert(SELFVIEW_ID.to_string(), ptr);
                        let _ = qt.queue(|mut o| {
                            // Bounce the self-view tile so the QML delegate recreates
                            // and its attachVideo Timer re-binds the new "sink".
                            let cur = o.as_ref().video_tiles().to_string();
                            o.as_mut().set_video_tiles(QString::from(&strip_selfview(&cur)));
                            let cur2 = o.as_ref().video_tiles().to_string();
                            o.as_mut().set_video_tiles(QString::from(&inject_selfview(&cur2)));
                                            let g = *o.as_ref().selfview_gen();
                                            o.as_mut().set_selfview_gen(g.wrapping_add(1));
                            o.as_mut().set_status(QString::from("Camera quality applied"));
                        });
                    }
                }
                Err(e) => {
                    eprintln!("[call] camera quality re-apply failed: {e}");
                    let _ = qt.queue(move |mut o| {
                        o.as_mut().set_camera_on(false);
                        o.as_mut().set_status(QString::from(&format!("Camera restart failed: {e}")));
                    });
                }
            }
        });
    }

    /// Set + persist the incoming-video cap and re-signal every subscribed tile.
    pub fn apply_recv_quality(mut self: Pin<&mut Self>, mode: &QString) {
        let mode = mode.to_string();
        tinhat_streamer::call::set_recv_quality(&mode);
        let cur = tinhat_streamer::call::recv_quality();
        self.as_mut().set_recv_quality(QString::from(&cur));
    }

    /// Load the persisted incoming-video cap into the `recvQuality` qproperty.
    pub fn refresh_recv_quality(mut self: Pin<&mut Self>) {
        let cur = tinhat_streamer::call::recv_quality();
        self.as_mut().set_recv_quality(QString::from(&cur));
    }

    /// Per-tile in-app visibility -> SFU pause/resume for that tile. Cheap (a mutex poke
    /// + a runtime task); safe on the Qt GUI thread.
    pub fn set_tile_video_hidden(self: Pin<&mut Self>, id: &QString, hidden: bool) {
        tinhat_streamer::call::set_tile_video_hidden(&id.to_string(), hidden);
    }

    /// Manual per-tile receive-quality pick -> SFU layer request (see streamer).
    pub fn apply_tile_quality(self: Pin<&mut Self>, id: &QString, mode: &QString) {
        tinhat_streamer::call::set_tile_quality(&id.to_string(), &mode.to_string());
    }

    /// Current manual pick for the dropdown checkmark ("auto" when none).
    pub fn tile_quality(self: Pin<&mut Self>, id: &QString) -> QString {
        QString::from(&tinhat_streamer::call::tile_quality(&id.to_string()))
    }

    /// Per-tile rendered size -> SFU layer request (quantized in the streamer).
    pub fn set_tile_video_size(self: Pin<&mut Self>, id: &QString, w: u32, h: u32) {
        tinhat_streamer::call::set_tile_video_size(&id.to_string(), w, h);
    }

    pub fn relayout_tile_video(self: Pin<&mut Self>, id: &QString) {
        crate::video::relayout(&id.to_string());
    }

    /// #7 MUSIC: publish the "music" 2nd audio track from `url` into the active call. The
    /// 2nd-track infra (call.rs) no-ops when not in a call; here we also guard on `inCall`
    /// so the UI gives feedback. `url` may be an http(s) URL or a `file://` path (gst's
    /// uridecodebin fetches+decodes either).
    pub fn start_music(mut self: Pin<&mut Self>, url: &QString) {
        let url = url.to_string();
        if !*self.in_call() || url.trim().is_empty() {
            self.as_mut().set_status(QString::from("Join a call to play music"));
            return;
        }
        self.as_mut().set_music_playing(true);
        self.as_mut().set_status(QString::from("Playing music…"));
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            if let Err(e) = tinhat_streamer::start_music(&url).await {
                eprintln!("[call] start_music failed: {e}");
                let _ = qt.queue(move |mut o| {
                    o.as_mut().set_music_playing(false);
                    o.as_mut().set_status(QString::from(&format!("Music failed: {e}")));
                });
            }
        });
    }

    /// #7 MUSIC: stop the music 2nd-track (unpublish). Idempotent.
    pub fn stop_music(mut self: Pin<&mut Self>) {
        self.as_mut().set_music_playing(false);
        if *self.in_call() {
            self.as_mut().set_status(QString::from("In call"));
        }
        tinhat_streamer::stop_music();
    }

    /// #7 MUSIC: reconcile `musicPlaying` with the streamer's live aux state. Clears the flag
    /// when a finite music file reached EOS and auto-unpublished (the streamer tore the slot
    /// down but nothing told the QObject). Only writes on a change, so the qproperty's changed
    /// signal doesn't fire every 120ms.
    pub fn refresh_music_state(mut self: Pin<&mut Self>) {
        let live = tinhat_streamer::is_music_playing();
        if *self.music_playing() && !live {
            self.as_mut().set_music_playing(false);
            if *self.in_call() {
                self.as_mut().set_status(QString::from("In call"));
            }
        }
    }

    /// #B: poll the streamer's live audio levels into `audioLevels` (JSON) + `micLevel`.
    /// Folds the existing mic/playback per-frame peaks the streamer already stashes; no
    /// pipeline change. Only writes on a change so the changed-signals stay quiet when idle
    /// (cf. refresh_music_state). Cheap mutex peeks; called from QML's 120ms mirror.
    pub fn refresh_audio_levels(mut self: Pin<&mut Self>) {
        // Echo cancellation can die mid-call on one transient pactl failure (a field report):
        // the module goes, playback returns to the raw device, the toggle still reads ON,
        // and everyone ELSE starts hearing the echo. Nothing used to retry, so a blip cost
        // the rest of the call. Retry from this existing tick, throttled — each attempt
        // shells out to pactl, so 8x/sec would be absurd; every ~5s is invisible to the user
        // and recovers long before a conversation is ruined.
        if *self.in_call() {
            use std::sync::atomic::{AtomicU64, Ordering};
            static LAST_TRY: AtomicU64 = AtomicU64::new(0);
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let last = LAST_TRY.load(Ordering::Relaxed);
            if now.saturating_sub(last) >= 5 {
                LAST_TRY.store(now, Ordering::Relaxed);
                // 10.5.2 (Astra release review #3): the retry re-plans the echo reference (where the call
                // plays, the default sink, headset or not: bounded pactl round-trips, up to 2 s each on a
                // stuck sound server), so it runs on a worker thread, never this GUI tick. One at a time;
                // the flag clears even if the retry panics. The settings mirror below picks the result up
                // on a later tick.
                use std::sync::atomic::AtomicBool;
                static RETRY_IN_FLIGHT: AtomicBool = AtomicBool::new(false);
                struct Done;
                impl Drop for Done {
                    fn drop(&mut self) {
                        RETRY_IN_FLIGHT.store(false, Ordering::Release);
                    }
                }
                if !RETRY_IN_FLIGHT.swap(true, Ordering::AcqRel) {
                    let spawned = std::thread::Builder::new().name("aec-retry".into()).spawn(|| {
                        let _done = Done;
                        tinhat_streamer::aec_retry_if_degraded();
                    });
                    if spawned.is_err() {
                        RETRY_IN_FLIGHT.store(false, Ordering::Release);
                    }
                }
            }
        }
        // Mirror canceller health for the settings pane. Only written on a change so
        // the qproperty's changed-signal stays quiet on the 120ms tick.
        let ec_bad = tinhat_streamer::aec_degraded();
        if *self.ec_degraded() != ec_bad {
            self.as_mut().set_ec_degraded(ec_bad);
        }
        // 0.8.31: the measured state of the canceller for the settings pane.
        let note = QString::from(tinhat_streamer::aec_status_note().as_str());
        if *self.ec_note() != note {
            self.as_mut().set_ec_note(note);
        }
        // Device health (0.8.25): a mic delivering only silence / no usable output device.
        // Change-only writes, like the rest of this tick.
        let silent = tinhat_streamer::mic_silent();
        if *self.mic_silent() != silent {
            self.as_mut().set_mic_silent(silent);
        }
        let no_out = tinhat_streamer::output_missing();
        if *self.output_missing() != no_out {
            self.as_mut().set_output_missing(no_out);
        }
        let nc_ok = tinhat_streamer::noise_cancel_available();
        if *self.nc_available() != nc_ok {
            self.as_mut().set_nc_available(nc_ok);
        }
        let levels = tinhat_streamer::audio_levels_json();
        if self.audio_levels().to_string() != levels {
            self.as_mut().set_audio_levels(QString::from(&levels));
        }
        // Format the mic RMS to a short fixed string so the change-check doesn't fire on
        // imperceptible float jitter every tick (3 decimals ≈ the bars' visual resolution).
        let mic = format!("{:.3}", tinhat_streamer::mic_level());
        if self.mic_level().to_string() != mic {
            self.as_mut().set_mic_level(QString::from(&mic));
        }
        // PRE-denoise mic RMS for the noise-cancel before/after meter (equals mic_level when
        // denoise is off / no monitor is running — i.e. nothing to compare).
        let micpre = format!("{:.3}", tinhat_streamer::mic_level_pre());
        if self.mic_level_pre().to_string() != micpre {
            self.as_mut().set_mic_level_pre(QString::from(&micpre));
        }
        // Speaker (output sink) RMS for the settings Audio-pane meter (0 when no output
        // monitor is running). Same 3-decimal change-guard as the mic levels.
        let out = format!("{:.3}", tinhat_streamer::output_level());
        if self.output_level().to_string() != out {
            self.as_mut().set_output_level(QString::from(&out));
        }
        // SFU active-speaker list (voice overlay / speaking rings). Piggybacks this poll:
        // the JSON only changes on real speaking transitions, so the guard stays quiet.
        let spk = tinhat_streamer::speakers_json();
        if self.speakers().to_string() != spk {
            self.as_mut().set_speakers(QString::from(&spk));
        }
        // Outgoing stream health (screen/camera/mic publisher stats). Also piggybacked:
        // the writers refresh every 3s, so this 120ms poll's change-guard fires ~1/3s max.
        let ps = tinhat_streamer::publish_stats_json();
        if self.publish_stats().to_string() != ps {
            self.as_mut().set_publish_stats(QString::from(&ps));
        }
        // 10.5: "can they hear me?" — the streamer rebuilds it 4x/s, so this guard fires at most that often.
        let hj = tinhat_streamer::hear_json();
        if self.hear_json().to_string() != hj {
            self.as_mut().set_hear_json(QString::from(&hj));
        }
        // 11.3 same room: rebuilt only on a change (link, ask, holder, speakers), so the guard is quiet.
        let sj = tinhat_streamer::same_room_json();
        if self.same_room_json().to_string() != sj {
            self.as_mut().set_same_room_json(QString::from(&sj));
        }
    }

    // ── 11.3 same room ─────────────────────────────────────────────────────────────────
    pub fn same_room_link(self: Pin<&mut Self>, target: &QString) -> QString {
        match tinhat_streamer::same_room_link(&target.to_string()) {
            Ok(()) => QString::default(),
            Err(e) => QString::from(e.as_str()),
        }
    }
    pub fn same_room_unlink(self: Pin<&mut Self>) {
        tinhat_streamer::same_room_unlink();
    }
    pub fn same_room_cancel(self: Pin<&mut Self>) {
        tinhat_streamer::same_room_cancel();
    }
    pub fn same_room_answer(self: Pin<&mut Self>, yes: bool) {
        tinhat_streamer::same_room_answer(yes);
    }
    pub fn same_room_hear_through(self: Pin<&mut Self>, on_this_device: bool) {
        tinhat_streamer::same_room_hear_through(on_this_device);
    }
    pub fn same_room_forget(self: Pin<&mut Self>, mxid: &QString) {
        tinhat_streamer::same_room_forget_pair(&mxid.to_string());
    }
    pub fn same_room_pairs_json(self: Pin<&mut Self>) -> QString {
        QString::from(&tinhat_streamer::same_room_pairs_json())
    }
    pub fn apply_same_room_auto(mut self: Pin<&mut Self>, on: bool) {
        set_same_room_auto_pref(on);
        tinhat_streamer::same_room_set_auto(on);
        self.as_mut().set_same_room_auto(on);
    }

    pub fn fix_system_level(self: Pin<&mut Self>, output: bool) {
        crate::bridge::runtime().spawn(async move {
            let _ = tokio::task::spawn_blocking(move || tinhat_streamer::hear_fix_system_level(output)).await;
        });
    }

    pub fn reconnect_mic(mut self: Pin<&mut Self>) {
        let was_muted = *self.mic_muted();
        self.as_mut().set_status(QString::from("Reconnecting your mic…"));
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let cell = current_call();
            let mut guard = cell.lock().await;
            let Some(call) = guard.as_mut() else { return };
            let r = match tokio::time::timeout(std::time::Duration::from_secs(8), async {
                if call.has_mic() {
                    call.republish_mic(was_muted).await
                } else {
                    call.ensure_mic().await.map(|_| ())
                }
            })
            .await
            {
                Ok(Ok(())) => Ok(()),
                Ok(Err(e)) => Err(format!("{e:#}")),
                Err(_) => Err("took over 8 s".to_string()),
            };
            let msg = match r {
                Ok(()) => "Mic reconnected".to_string(),
                Err(e) => {
                    eprintln!("[call] reconnect mic (from the pill) failed: {e}");
                    "Couldn't reconnect the mic — the app keeps trying".to_string()
                }
            };
            let _ = qt.queue(move |mut o| o.as_mut().set_status(QString::from(&msg)));
        });
    }

    pub fn heal_peer_audio(self: Pin<&mut Self>, identity: &QString) {
        let id = identity.to_string();
        eprintln!("[call] repair requested for what we hear from {id}");
        tinhat_streamer::hear_request_heal(&id);
    }

    pub fn add_ec_output(mut self: Pin<&mut Self>, node: &QString) {
        let list = tinhat_streamer::call::aec_ref_outputs_with(&node.to_string());
        let json = serde_json::to_string(&list).unwrap_or_else(|_| "[]".into());
        self.as_mut().apply_ec_outputs(&QString::from(&json));
    }

    /// Apply the event drain's authoritative `desired_video_tiles` to `videoTiles` if it
    /// differs. Decouples the QML model from the one-shot tile-create queue closures: if a
    /// closure is ever dropped, the next poll still publishes the tile so its delegate +
    /// attach Timer appear and attach_video lazily builds the pipeline. Empty desired
    /// (no active call) is skipped so this never clobbers state outside a call.
    pub fn refresh_video_tiles(mut self: Pin<&mut Self>) {
        let want = desired_video_tiles().lock().unwrap().clone();
        if !want.is_empty() && self.video_tiles().to_string() != want {
            self.as_mut().set_video_tiles(QString::from(&want));
        }
        self.as_mut().maybe_rescan_peers();
    }

    // ── RBAC: call moderation (docs/RBAC.md §5.4) ──────────────────────────────────

    pub fn server_mute(self: Pin<&mut Self>, identity: &QString) -> QString {
        self.moderate("mute", &identity.to_string())
    }

    pub fn server_unmute(self: Pin<&mut Self>, identity: &QString) -> QString {
        self.moderate("unmute", &identity.to_string())
    }

    pub fn disconnect_participant(self: Pin<&mut Self>, identity: &QString) -> QString {
        self.moderate("disconnect", &identity.to_string())
    }

    pub fn refresh_call_perms(self: Pin<&mut Self>) {
        let room_id = if *self.in_call() { self.room_id().to_string() } else { String::new() };
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let (json, can) = compute_call_perms(&room_id).await;
            can_moderate_flag().store(can, std::sync::atomic::Ordering::Relaxed);
            let _ = qt.queue(move |mut o| o.as_mut().apply_call_perms(&json, can));
        });
    }

    /// Set callPermsJson / canModerateCall (quiet when unchanged) + the async mirror.
    fn apply_call_perms(mut self: Pin<&mut Self>, json: &str, can_moderate: bool) {
        can_moderate_flag().store(can_moderate, std::sync::atomic::Ordering::Relaxed);
        if self.call_perms_json().to_string() != json {
            self.as_mut().set_call_perms_json(QString::from(json));
        }
        if *self.can_moderate_call() != can_moderate {
            self.as_mut().set_can_moderate_call(can_moderate);
        }
    }

    /// `lastError` + `errorRev` (QML toasts on the rev, so the same text twice still shows).
    fn report_error(mut self: Pin<&mut Self>, text: &str) {
        self.as_mut().set_last_error(QString::from(text));
        let r = *self.error_rev();
        self.as_mut().set_error_rev(r.wrapping_add(1));
    }

    /// Call over (leave / terminal disconnect): the per-call moderation view goes back to
    /// "not in a call". `removedBy` deliberately survives until the next join (the banner).
    fn clear_moderation_view(mut self: Pin<&mut Self>) {
        self.as_mut().set_server_muted(false);
        self.as_mut().set_server_muted_by(QString::default());
        self.as_mut().apply_call_perms("{}", false);
    }

    /// Shared front half of the three invokables: fast local checks (a human message back
    /// to QML), then the async packet → POST → state flow in `moderate_async`.
    fn moderate(self: Pin<&mut Self>, action: &'static str, target: &str) -> QString {
        if !*self.in_call() {
            return QString::from("You're not in a call");
        }
        if !*self.can_moderate_call() {
            return QString::from("You can't moderate calls in this space");
        }
        let Some(target) = tinhat_streamer::resolve_call_participant(target) else {
            return QString::from("They're not in the call");
        };
        let me_mxid = crate::session::client().and_then(|c| c.user_id().map(|u| u.to_string())).unwrap_or_default();
        if mxid_of(&target) == me_mxid {
            return QString::from(if action == "disconnect" {
                "You can't disconnect yourself — hang up instead"
            } else {
                "You can't server-mute yourself"
            });
        }
        let room_id = self.room_id().to_string();
        let qt = self.qt_thread();
        eprintln!("[moderation] {action} {target} requested");
        crate::bridge::runtime().spawn(async move { moderate_async(qt, room_id, action, target, true).await });
        QString::default()
    }

    /// Every ~5s while in a call, re-read the room's call memberships and note
    /// whether a browser / Element X client is among them. On the flip to "yes" while
    /// an AV1 share is live, re-publish the share as H.264 (same portal cast, no
    /// re-prompt) — otherwise the newcomer sees nothing and their PLI storm degrades
    /// the share for everyone else too. Cheap: one state-store read, no network. Rides
    /// the QML tick that already calls refresh_video_tiles only while in a call.
    fn maybe_rescan_peers(self: Pin<&mut Self>) {
        use std::time::{Duration, Instant};
        static LAST: std::sync::Mutex<Option<Instant>> = std::sync::Mutex::new(None);
        {
            let mut last = LAST.lock().unwrap();
            if last.map(|t| t.elapsed() < Duration::from_secs(5)).unwrap_or(false) {
                return;
            }
            *last = Some(Instant::now());
        }
        let room_id = self.room_id().to_string();
        if room_id.is_empty() {
            return;
        }
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let Some(client) = crate::session::client() else { return };
            let Ok(rid) = matrix_sdk::ruma::RoomId::parse(&room_id) else { return };
            let Some(room) = client.get_room(&rid) else { return };
            let others = crate::rtc::non_native_call_members(&room).await;
            let present = !others.is_empty();
            if !tinhat_streamer::call::set_browser_peers_present(present) {
                return; // unchanged
            }
            eprintln!("[call] non-native peers in call: {present} {others:?}");
            if !present {
                return; // back to native-only: the NEXT share may use AV1 again; don't churn a live one
            }
            let cell = current_call();
            let mut guard = cell.lock().await;
            let Some(call) = guard.as_mut() else { return };
            if !call.screenshare_is_av1() {
                return;
            }
            eprintln!("[call] browser viewer joined during an AV1 share -> re-publishing as H.264");
            match call.restart_screenshare().await {
                Ok(()) => {
                    let _ = qt.queue(|mut o| {
                        o.as_mut().set_status(QString::from("Sharing screen (H.264 — a browser viewer joined)"))
                    });
                }
                Err(e) => {
                    eprintln!("[call] share codec re-publish failed: {e}");
                    let _ = qt.queue(move |mut o| {
                        o.as_mut().set_sharing(false);
                        o.as_mut().set_status(QString::from(&format!("Share restart failed: {e}")));
                    });
                }
            }
        });
    }

    /// #7 RNNoise: set + persist the mic denoise toggle, mirror it into `micDenoise`, and
    /// (if in a call) rebuild the live mic capture pipeline so it applies immediately. When
    /// OFF the mic string is byte-identical to today — the element is simply not inserted.
    pub fn apply_mic_denoise(mut self: Pin<&mut Self>, on: bool) {
        tinhat_streamer::set_mic_denoise(on);
        self.as_mut().set_mic_denoise(on);
        // Rebuild the mic ONLY on this explicit toggle (never silently elsewhere). Preserve mute.
        let was_muted = *self.mic_muted();
        crate::bridge::runtime().spawn(async move {
            let cell = current_call();
            let mut guard = cell.lock().await;
            if let Some(call) = guard.as_mut() {
                if let Err(e) = call.restart_mic() {
                    eprintln!("[call] mic denoise restart failed: {e}");
                }
                call.set_mic_muted(was_muted);
            }
        });
    }

    /// Echo cancellation: persist + mirror the toggle, then apply it to the live call
    /// (module load/unload + sink re-point + capture rebuild). Mute is preserved across
    /// the rebuild, same as set_input.
    pub fn apply_mic_aec(mut self: Pin<&mut Self>, on: bool) {
        tinhat_streamer::set_aec_enabled(on);
        self.as_mut().set_mic_aec(on);
        let was_muted = *self.mic_muted();
        crate::bridge::runtime().spawn(async move {
            let cell = current_call();
            let mut guard = cell.lock().await;
            if let Some(call) = guard.as_mut() {
                if let Err(e) = call.apply_aec() {
                    eprintln!("[aec] live apply failed: {e}");
                }
                call.set_mic_muted(was_muted);
            }
        });
    }

    /// 0.8.33: the speaker outputs the canceller listens to (Settings › Audio › Speaker outputs).
    /// Persist + mirror, then rebind the reference live (apply_aec ends in a mic rebuild that
    /// carries the new reference branch; mute is preserved like every other rebuild).
    pub fn apply_ec_outputs(mut self: Pin<&mut Self>, json: &QString) {
        let list: Vec<String> = serde_json::from_str(&json.to_string()).unwrap_or_default();
        tinhat_streamer::set_aec_ref_outputs(&list);
        let back = serde_json::to_string(&tinhat_streamer::aec_ref_outputs()).unwrap_or_else(|_| "[]".into());
        self.as_mut().set_ec_outputs(QString::from(&back));
        eprintln!(
            "[aec] speaker outputs for the echo reference -> {}",
            if list.is_empty() { "the output Tin Hat plays to".to_string() } else { list.join(", ") }
        );
        let was_muted = *self.mic_muted();
        crate::bridge::runtime().spawn(async move {
            let cell = current_call();
            let mut guard = cell.lock().await;
            if let Some(call) = guard.as_mut() {
                if tinhat_streamer::aec_enabled() {
                    if let Err(e) = call.apply_aec() {
                        eprintln!("[aec] reference rebind failed: {e}");
                    }
                    call.set_mic_muted(was_muted);
                }
            }
        });
    }

    /// Mirror the persisted echo-cancellation toggle into `micAec` for QML.
    pub fn refresh_mic_aec(mut self: Pin<&mut Self>) {
        let on = tinhat_streamer::aec_enabled();
        self.as_mut().set_mic_aec(on);
        let outs = serde_json::to_string(&tinhat_streamer::aec_ref_outputs()).unwrap_or_else(|_| "[]".into());
        if self.ec_outputs().to_string() != outs {
            self.as_mut().set_ec_outputs(QString::from(&outs));
        }
        let engine = tinhat_streamer::aec_tier_name();
        if self.aec_engine().to_string() != engine {
            self.as_mut().set_aec_engine(QString::from(engine));
        }
    }

    pub fn refresh_mic_denoise(mut self: Pin<&mut Self>) {
        let on = tinhat_streamer::mic_denoise();
        self.as_mut().set_mic_denoise(on);
    }

    /// Auto-gain-control: persist + mirror into `micAgc`, then re-publish the live mic so
    /// it applies mid-call. The gain stage users actually hear is the AEC module's
    /// webrtc.gain_control (the NativeAudioSource APM option is inert on the fast path —
    /// see publish_mic_muted): the re-publish runs aec_start(), whose binding fingerprint
    /// includes the AGC flag, so the module reloads with the new gain setting and the
    /// playback sinks are re-pointed onto the recreated reference sink.
    pub fn apply_mic_agc(mut self: Pin<&mut Self>, on: bool) {
        let was_muted = *self.mic_muted();
        tinhat_streamer::set_mic_agc(on);
        self.as_mut().set_mic_agc(on);
        republish_live_mic(was_muted);
    }

    /// libwebrtc noise-suppression: persist + mirror into `micNs`, then re-publish the live mic.
    pub fn apply_mic_ns_mode(mut self: Pin<&mut Self>, mode: &QString) {
        let Some(m) = tinhat_streamer::NsMode::parse(&mode.to_string()) else {
            eprintln!("[call] ignoring unknown noise-suppression mode {:?}", mode.to_string());
            return;
        };
        let prev = tinhat_streamer::mic_ns_mode();
        tinhat_streamer::set_mic_ns_mode(m);
        self.as_mut().set_mic_ns_mode(QString::from(m.as_str()));
        self.as_mut().set_mic_denoise(m == tinhat_streamer::NsMode::Strong);
        eprintln!("[call] noise suppression -> {}", m.as_str());
        // Standard and Strong are both levels of the one WebRTC stage in the capture
        // pipeline (0.8.29): rebuild it in place, preserving mute (same as apply_mic_denoise).
        // 0.8.29: if the rebuild fails on EVERY chain, the user's remedy for bad
        // audio must not leave them silent with a setting that cannot be applied — revert
        // the persisted mode and say so (before: one eprintln, mode stuck, mic dead).
        let was_muted = *self.mic_muted();
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let cell = current_call();
            let mut guard = cell.lock().await;
            if let Some(call) = guard.as_mut() {
                if let Err(e) = call.restart_mic() {
                    eprintln!(
                        "[call] mic rebuild for the noise-suppression change failed: {e} — \
                         reverting noise suppression to {}",
                        prev.as_str()
                    );
                    tinhat_streamer::set_mic_ns_mode(prev);
                    let _ = call.restart_mic();
                    let _ = qt.queue(move |mut o| {
                        o.as_mut().set_mic_ns_mode(QString::from(prev.as_str()));
                        o.as_mut()
                            .set_mic_denoise(prev == tinhat_streamer::NsMode::Strong);
                        o.as_mut().set_status(QString::from(&format!(
                            "Noise suppression could not be changed — back to {}",
                            prev.as_str()
                        )));
                    });
                }
                call.set_mic_muted(was_muted);
            }
        });
    }

    /// Load the persisted AGC + NS toggles into `micAgc` / `micNs` for QML.
    pub fn refresh_mic_apm(mut self: Pin<&mut Self>) {
        let agc = tinhat_streamer::mic_agc();
        let ns = tinhat_streamer::mic_ns_mode();
        self.as_mut().set_mic_agc(agc);
        self.as_mut().set_mic_ns_mode(QString::from(ns.as_str()));
        self.as_mut().set_mic_denoise(ns == tinhat_streamer::NsMode::Strong);
    }

    /// #1 MUSIC: enumerate shareable audio sources OFF the Qt thread (a synchronous gst
    /// DeviceMonitor scan stalls the UI thread), then mirror the JSON list + the persisted
    /// source choice back into `audioSources` / `musicSourceName`.
    pub fn refresh_audio_sources(mut self: Pin<&mut Self>) {
        let saved = tinhat_streamer::music_source().unwrap_or_default();
        self.as_mut().set_music_source_name(QString::from(&saved));
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let json = enumerate(tinhat_streamer::list_audio_sources());
            let apps = crate::audioapps::list_audio_apps_json();
            let _ = qt.queue(move |mut o| {
                o.as_mut().set_audio_sources(QString::from(&json));
                o.as_mut().set_audio_apps(QString::from(&apps));
            });
        });
    }

    /// #1 MUSIC: start the music 2nd-track from a chosen PipeWire source `node` (empty =
    /// reuse the persisted source). Persists the choice; the streamer no-ops when not in a
    /// call, and we guard on `inCall` here for UI feedback.
    pub fn start_music_source(mut self: Pin<&mut Self>, node: &QString) {
        let node = node.to_string();
        // Persist + mirror FIRST — including outside a call: both the gear menu and the
        // Settings Audio pane advertise pick-ahead ("the selected source is remembered"),
        // so the choice must stick and the check mark must move even when the share
        // itself can't start yet.
        let chosen = if node.is_empty() {
            tinhat_streamer::music_source().unwrap_or_default()
        } else {
            tinhat_streamer::set_music_source(Some(node.clone()));
            node.clone()
        };
        if !chosen.is_empty() {
            self.as_mut().set_music_source_name(QString::from(&chosen));
        }
        if !*self.in_call() {
            self.as_mut().set_status(QString::from("Join a call to share audio"));
            return;
        }
        if chosen.is_empty() {
            self.as_mut().set_status(QString::from("Pick an audio source to share"));
            return;
        }
        self.as_mut().set_music_playing(true);
        self.as_mut().set_status(QString::from("Sharing audio…"));
        let arg = if node.is_empty() { None } else { Some(node) };
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            if let Err(e) = tinhat_streamer::start_music_source(arg).await {
                eprintln!("[call] start_music_source failed: {e}");
                let _ = qt.queue(move |mut o| {
                    o.as_mut().set_music_playing(false);
                    o.as_mut().set_status(QString::from(&format!("Audio share failed: {e}")));
                });
            }
        });
    }

    /// #4 AFK/BRB: cut/restore INCOMING remote audio (the streamer mutes all remote-audio
    /// gain elements reversibly without stomping #5 per-user mutes). Hiding/stopping the
    /// remote VIDEO tiles is done in QML (the tile delegates go invisible while away).
    pub fn set_away_privacy(self: Pin<&mut Self>, on: bool) {
        tinhat_streamer::set_away_privacy(on);
    }

    /// Deafen: mute/unmute ALL incoming call audio (output). Transient toggle (reset on each
    /// fresh call, see leave_call). Composes with AFK + #5 per-user mutes via the streamer's
    /// reversible reconcile, so un-deafening won't clobber a track the user muted individually.
    pub fn apply_deafen(mut self: Pin<&mut Self>, on: bool) {
        tinhat_streamer::set_deafened(on);
        self.as_mut().set_deafened(on);
    }

    /// Start a STANDALONE mic monitor (no call needed) feeding micLevelPre (raw) + micLevel
    /// (post-denoise) so the settings noise-cancel pane can show a live before/after meter.
    /// No-op if a monitor is already running.
    pub fn start_mic_monitor(self: Pin<&mut Self>) {
        // Build the gst monitor pipeline OFF the Qt GUI thread — matches every other gst
        // invokable in this file. A synchronous pipewiresrc build + PLAYING transition on
        // the Qt GUI thread (which is also driving the qml6glsink GL pipelines during a
        // call) is what crashed/stalled the app when the noise panel opened. Mirror the
        // started flag back on the Qt thread (false also drives the QML "unavailable" hint).
        mic_monitor_wanted().store(true, std::sync::atomic::Ordering::Relaxed);
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let ok = tinhat_streamer::start_mic_monitor();
            // If the panel was closed before the (async) build finished, the user no longer
            // wants the monitor — tear the just-built pipeline down so it can't leak / hold
            // the mic open forever (defeats the close-before-start-completes race).
            if ok && !mic_monitor_wanted().load(std::sync::atomic::Ordering::Relaxed) {
                tinhat_streamer::stop_mic_monitor();
                let _ = qt.queue(move |mut o| o.as_mut().set_mic_monitoring(false));
                return;
            }
            let _ = qt.queue(move |mut o| o.as_mut().set_mic_monitoring(ok));
        });
    }

    /// Stop the standalone mic monitor. Idempotent; never touches a live call's own mic.
    pub fn stop_mic_monitor(mut self: Pin<&mut Self>) {
        // Clear "wanted" (so an in-flight start tears itself down) + flip the flag
        // immediately (stops the QML level poll), then tear the pipeline down OFF the GUI
        // thread (NULLing a PipeWire pipeline can also block). stop_mic_monitor is idempotent
        // and only zeroes the shared mic level when it actually had a monitor running.
        mic_monitor_wanted().store(false, std::sync::atomic::Ordering::Relaxed);
        self.as_mut().set_mic_monitoring(false);
        crate::bridge::runtime().spawn(async move {
            // A newer start request (wanted flipped back to true while this task waited
            // for a runtime worker) supersedes this stop — don't tear down the monitor it
            // owns or is about to build. This is the Settings→noise-test handoff: close()
            // queues a stop and open() queues a start back-to-back.
            if !mic_monitor_wanted().load(std::sync::atomic::Ordering::Relaxed) {
                tinhat_streamer::stop_mic_monitor();
            }
        });
    }

    /// Start a STANDALONE speaker meter (no call needed) tapping the selected output sink's
    /// `.monitor` into outputLevel, so the settings Audio pane can show a live speaker meter.
    /// No-op if one is already running. Mirrors start_mic_monitor (off-GUI-thread build).
    pub fn start_output_monitor(self: Pin<&mut Self>) {
        // Build the gst monitor pipeline OFF the Qt GUI thread — a synchronous pipewiresrc
        // build + PLAYING transition on the GUI thread stalls it (same reason as the mic
        // monitor). Mirror the started flag back on the Qt thread.
        output_monitor_wanted().store(true, std::sync::atomic::Ordering::Relaxed);
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let ok = tinhat_streamer::start_output_monitor();
            // If the pane was closed before the (async) build finished, tear the just-built
            // pipeline down so it can't leak / hold the monitor open forever.
            if ok && !output_monitor_wanted().load(std::sync::atomic::Ordering::Relaxed) {
                tinhat_streamer::stop_output_monitor();
                let _ = qt.queue(move |mut o| o.as_mut().set_output_monitoring(false));
                return;
            }
            let _ = qt.queue(move |mut o| o.as_mut().set_output_monitoring(ok));
        });
    }

    /// Stop the standalone speaker meter. Idempotent; never touches a live call's playback.
    pub fn stop_output_monitor(mut self: Pin<&mut Self>) {
        // Clear "wanted" (so an in-flight start tears itself down) + flip the flag immediately
        // (stops the QML poll), then tear the pipeline down OFF the GUI thread.
        output_monitor_wanted().store(false, std::sync::atomic::Ordering::Relaxed);
        self.as_mut().set_output_monitoring(false);
        crate::bridge::runtime().spawn(async move {
            tinhat_streamer::stop_output_monitor();
        });
    }

    pub fn share_screen(mut self: Pin<&mut Self>) {
        if !*self.in_call() || *self.sharing() {
            return;
        }
        self.as_mut().set_status(QString::from("Preparing screen share…"));
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let result = async {
                let screencast = tinhat_streamer::pick_screencast()
                    .await
                    .map_err(|e| format!("portal: {e}"))?;
                let cell = current_call();
                let mut guard = cell.lock().await;
                let call = guard.as_mut().ok_or_else(|| "not in a call".to_string())?;
                call.publish_screenshare(screencast)
                    .await
                    .map_err(|e| format!("share: {e}"))
            }
            .await;
            let _ = qt.queue(move |mut o| match result {
                Ok(()) => {
                    o.as_mut().set_sharing(true);
                    o.as_mut().set_status(QString::from("Sharing screen"));
                }
                Err(e) => {
                    eprintln!("[call] share failed: {e}");
                    o.as_mut().set_status(QString::from(&format!("Share failed: {e}")));
                }
            });
        });
    }

    /// Windows: enumerate capturable monitors + top-level windows for the QML picker.
    /// Cheap synchronous Win32 walk — safe on the GUI thread. Linux: always `[]`.
    pub fn share_sources(self: Pin<&mut Self>) -> QString {
        #[cfg(windows)]
        {
            let list: Vec<serde_json::Value> = tinhat_streamer::list_share_sources()
                .into_iter()
                .map(|s| {
                    serde_json::json!({
                        // handle as STRING: HWNDs are 64-bit and JS numbers lose
                        // integer precision past 2^53.
                        "kind": s.kind, "handle": s.handle.to_string(), "label": s.label,
                    })
                })
                .collect();
            return QString::from(&serde_json::Value::Array(list).to_string());
        }
        #[allow(unreachable_code)]
        QString::from("[]")
    }

    /// Start sharing the picker-chosen source. Same publish flow as share_screen,
    /// with the portal pick replaced by ScreenCast::from_parts (which re-validates
    /// the handle — the window may have closed since the list was built).
    pub fn share_screen_source(mut self: Pin<&mut Self>, kind: &QString, handle: &QString) {
        #[cfg(not(windows))]
        {
            let _ = (kind, handle);
        }
        #[cfg(windows)]
        {
            if !*self.in_call() || *self.sharing() {
                return;
            }
            let kind = kind.to_string();
            let handle: u64 = handle.to_string().parse().unwrap_or(0);
            self.as_mut().set_status(QString::from("Preparing screen share…"));
            let qt = self.qt_thread();
            crate::bridge::runtime().spawn(async move {
                let result = async {
                    let screencast = tinhat_streamer::ScreenCast::from_parts(&kind, handle)
                        .map_err(|e| format!("capture: {e}"))?;
                    let cell = current_call();
                    let mut guard = cell.lock().await;
                    let call = guard.as_mut().ok_or_else(|| "not in a call".to_string())?;
                    call.publish_screenshare(screencast)
                        .await
                        .map_err(|e| format!("share: {e}"))
                }
                .await;
                let _ = qt.queue(move |mut o| match result {
                    Ok(()) => {
                        o.as_mut().set_sharing(true);
                        o.as_mut().set_status(QString::from("Sharing screen"));
                    }
                    Err(e) => {
                        eprintln!("[call] share failed: {e}");
                        o.as_mut().set_status(QString::from(&format!("Share failed: {e}")));
                    }
                });
            });
        }
    }

    pub fn set_tile_tuned_in(self: Pin<&mut Self>, id: &QString, tuned: bool) {
        tinhat_streamer::set_tile_tuned_in(&id.to_string(), tuned);
    }

    pub fn apply_share_recording_only(mut self: Pin<&mut Self>, on: bool) {
        tinhat_streamer::set_share_recording_only(on);
        self.as_mut().set_share_rec_only(on);
        crate::bridge::runtime().spawn(async move {
            let cell = current_call();
            let guard = cell.lock().await;
            if let Some(call) = guard.as_ref() {
                call.announce_recording_only().await;
            }
        });
    }

    pub fn refresh_rec_only_flags(mut self: Pin<&mut Self>) {
        let j = QString::from(&tinhat_streamer::rec_only_flags_json());
        if *self.rec_only_flags() != j {
            self.as_mut().set_rec_only_flags(j);
        }
    }

    pub fn apply_game_chat(mut self: Pin<&mut Self>, on: bool) {
        // 9.4: game chat and a corner never overlap — switching game chat on leaves the corner.
        if on && !tinhat_streamer::corner_mine().is_empty() {
            tinhat_streamer::corner_leave();
        }
        tinhat_streamer::set_local_game_chat(on);
        self.as_mut().set_game_chat(on);
        crate::bridge::runtime().spawn(async move {
            let cell = current_call();
            let guard = cell.lock().await;
            if let Some(call) = guard.as_ref() {
                call.announce_game_chat().await;
            }
        });
    }

    pub fn refresh_game_chat(mut self: Pin<&mut Self>) {
        let j = QString::from(&tinhat_streamer::game_chat_json());
        if *self.game_chat_peers() != j {
            self.as_mut().set_game_chat_peers(j);
        }
        let on = tinhat_streamer::local_game_chat();
        if *self.game_chat() != on {
            self.as_mut().set_game_chat(on);
        }
    }

    pub fn pull_aside(mut self: Pin<&mut Self>, target: &QString) -> QString {
        match tinhat_streamer::corner_pull(&target.to_string()) {
            Ok(_) => {
                self.as_mut().refresh_corners();
                QString::default()
            }
            Err(e) => QString::from(e.as_str()),
        }
    }

    pub fn join_corner(mut self: Pin<&mut Self>, target: &QString) -> QString {
        match tinhat_streamer::corner_join(&target.to_string()) {
            Ok(_) => {
                self.as_mut().refresh_corners();
                QString::default()
            }
            Err(e) => QString::from(e.as_str()),
        }
    }

    pub fn leave_corner(mut self: Pin<&mut Self>) {
        tinhat_streamer::corner_leave();
        self.as_mut().refresh_corners();
    }

    pub fn refresh_corners(mut self: Pin<&mut Self>) {
        tinhat_streamer::corner_tick();
        let pend = tinhat_streamer::take_corner_announcements();
        let gc = tinhat_streamer::take_game_chat_announce_due();
        if !pend.is_empty() || gc {
            crate::bridge::runtime().spawn(async move {
                let cell = current_call();
                let guard = cell.lock().await;
                if let Some(call) = guard.as_ref() {
                    for (id, pull) in pend {
                        call.announce_corner(&id, &pull).await;
                    }
                    if gc {
                        call.announce_game_chat().await;
                    }
                }
            });
        }
        let j = QString::from(&tinhat_streamer::corner_json());
        if *self.corner_json() != j {
            self.as_mut().set_corner_json(j);
        }
    }

    pub fn set_tile_watched(self: Pin<&mut Self>, id: &QString, watched: bool) {
        tinhat_streamer::set_tile_watched(&id.to_string(), watched);
    }

    pub fn stop_screen(mut self: Pin<&mut Self>) {
        self.as_mut().set_sharing(false);
        self.as_mut().set_status(QString::from("In call"));
        crate::bridge::runtime().spawn(async move {
            let cell = current_call();
            let mut guard = cell.lock().await;
            if let Some(call) = guard.as_mut() {
                call.stop_screenshare().await;
            }
        });
    }

    /// Where the mic should be, given the three inputs that decide it. Pure so the gate
    /// can check every combination without a call, a device, or a keyboard.
    ///
    /// The rule that matters: with push-to-talk ON the mic is muted unless the key is
    /// held, and toggling mute by hand does NOT override that — otherwise you press the
    /// mute button once, forget, and broadcast your room for the rest of the evening.
    pub(crate) fn ptt_target_muted(ptt_on: bool, holding: bool, manual_muted: bool) -> bool {
        if ptt_on {
            !holding
        } else {
            manual_muted
        }
    }

    /// Turn push-to-talk on or off (persisted). Switching it ON mutes immediately, so
    /// enabling it mid-call cannot leave you live while you look for the key.
    /// Load the persisted preference into the property. Called once at startup; without
    /// it the toggle reads OFF every launch while the file says otherwise.
    pub fn restore_push_to_talk(mut self: Pin<&mut Self>) {
        let on = ptt_pref();
        self.as_mut().set_push_to_talk(on);
        // 11.3 same room: the remembered-pairs store + the auto-link pref (once, at startup).
        tinhat_streamer::same_room_set_store_path(crate::platform::app_data_dir().join("sameroom_pairs.json"));
        let auto = same_room_auto_pref();
        tinhat_streamer::same_room_set_auto(auto);
        self.as_mut().set_same_room_auto(auto);
        if self.same_room_json().is_empty() {
            self.as_mut().set_same_room_json(QString::from("{}"));
        }
        // RBAC: callPermsJson is "{}" outside a call (a derived Default leaves it "").
        if self.call_perms_json().is_empty() {
            self.as_mut().set_call_perms_json(QString::from("{}"));
        }
    }

    pub fn enable_push_to_talk(mut self: Pin<&mut Self>, on: bool) {
        self.as_mut().set_push_to_talk(on);
        set_ptt_pref(on);
        let manual = *self.mic_muted();
        let target = Self::ptt_target_muted(on, false, manual);
        self.as_mut().set_ptt_speaking(false);
        self.force_mic_muted(target);
    }

    /// The hold key changed state. Idempotent: a key repeat that slips through, or a
    /// release we already handled, must not toggle anything.
    pub fn ptt_hold(mut self: Pin<&mut Self>, down: bool) {
        if !*self.push_to_talk() || *self.ptt_speaking() == down {
            return;
        }
        self.as_mut().set_ptt_speaking(down);
        let target = Self::ptt_target_muted(true, down, true);
        self.force_mic_muted(target);
    }

    pub fn toggle_mic(mut self: Pin<&mut Self>) {
        // RBAC: a server mute locks the button — say why instead of flipping a mic the
        // SFU won't take anyway.
        if *self.server_muted() {
            let by = self.server_muted_by().to_string();
            let by = if by.is_empty() { "A moderator".to_string() } else { by };
            self.as_mut().set_status(QString::from(&format!(
                "{by} muted your mic for everyone — ask them to unmute you"
            )));
            return;
        }
        let muted = !*self.mic_muted();
        self.as_mut().set_mic_muted(muted);
        crate::bridge::runtime().spawn(async move {
            let cell = current_call();
            let guard = cell.lock().await;
            if let Some(call) = guard.as_ref() {
                call.set_mic_muted(muted);
            }
        });
    }

    /// Explicitly set the mic muted state (NOT a toggle) and apply it to the live call. Unlike
    /// `toggle_mic` this is idempotent and direction-safe: `force_mic_muted(false)` ALWAYS
    /// unmutes + drives the gst pipeline to Playing (resuming capture), even if `mic_muted`
    /// already read false-but-Paused. The AFK/BRB restore uses it so coming back can't be
    /// defeated by a stale toggle/mirror state.
    pub fn force_mic_muted(mut self: Pin<&mut Self>, muted: bool) {
        self.as_mut().set_mic_muted(muted);
        // RBAC: while server-muted the INTENT is recorded (it decides what happens when the
        // moderator unmutes us) but no unmute reaches the SFU.
        if !muted && *self.server_muted() {
            return;
        }
        crate::bridge::runtime().spawn(async move {
            let cell = current_call();
            let guard = cell.lock().await;
            if let Some(call) = guard.as_ref() {
                call.set_mic_muted(muted);
            }
        });
    }

    /// Toggle the local camera: publish (with a qml6glsink self-view tile bound on the
    /// GUI thread via the existing attach loop) / unpublish. The self-view rides the
    /// SAME videoGrid Repeater as remote tiles (id "vid_selfview").
    pub fn toggle_camera(mut self: Pin<&mut Self>) {
        if !*self.in_call() {
            return;
        }
        let turning_on = !*self.camera_on();
        self.as_mut().set_camera_on(turning_on);
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let cell = current_call();
            let mut guard = cell.lock().await;
            let Some(call) = guard.as_mut() else { return };
            if turning_on {
                match call.publish_camera(None, true).await {
                    Ok(ptr) => {
                        if ptr != 0 {
                            video_registry().lock().unwrap().insert(SELFVIEW_ID.to_string(), ptr);
                        }
                        selfview_active().store(true, std::sync::atomic::Ordering::Relaxed);
                        let _ = qt.queue(|mut o| {
                            o.as_mut().set_status(QString::from("Camera on"));
                            let cur = o.as_ref().video_tiles().to_string();
                            o.as_mut().set_video_tiles(QString::from(&inject_selfview(&cur)));
                        });
                    }
                    Err(e) => {
                        // Camera failed to start, but we're still in the call — keep the
                        // self-view tile (it just stays on the avatar placeholder).
                        let _ = qt.queue(move |mut o| {
                            o.as_mut().set_camera_on(false);
                            o.as_mut().set_status(QString::from(&format!("Camera failed: {e}")));
                        });
                    }
                }
            } else {
                // Turning the camera OFF: tear the self-view pipeline down but KEEP the
                // tile in the grid (selfview stays active for the whole call). The C++
                // detach removes the GL surface so the avatar placeholder shows again,
                // and the QML self-tile resets so toggling the camera back on re-attaches.
                call.unpublish_camera().await;
                let _ = qt.queue(|mut o| {
                    video_registry().lock().unwrap().remove(SELFVIEW_ID);
                    crate::video::detach(SELFVIEW_ID); // remove GL surface -> reveal avatar
                    o.as_mut().set_status(QString::from("Camera off"));
                });
            }
        });
    }

    pub fn refresh_outputs(self: Pin<&mut Self>) {
        // Enumerate AND read the persisted SPEAKER choice OFF the Qt thread, then mirror both
        // the menu checkmark name and the device list back. The first read of the saved-device
        // cell can run a synchronous gst DeviceMonitor scan (validation), which on the UI thread
        // stalls/crashes it — so it must not run inline here.
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let saved = tinhat_streamer::call::playback_target_node().unwrap_or_default();
            let json = enumerate(tinhat_streamer::call::list_audio_outputs());
            let _ = qt.queue(move |mut o| {
                o.as_mut().set_output_name(QString::from(&saved));
                o.as_mut().set_outputs(QString::from(&json));
            });
        });
    }

    pub fn set_output(mut self: Pin<&mut Self>, node: &QString) {
        let node = node.to_string();
        tinhat_streamer::call::set_playback_target(
            if node.is_empty() { None } else { Some(node.clone()) },
        );
        self.as_mut().set_output_name(QString::from(&node));
        // Under AEC, set_playback_target deliberately does NOT touch the live sinks (they
        // must stay on the EC reference sink) — the module's sink_master is what follows
        // the new speaker. apply_aec reloads it against the new device and re-points the
        // sinks exactly once; it ends in restart_mic, so preserve mute like set_input does.
        let was_muted = *self.mic_muted();
        crate::bridge::runtime().spawn(async move {
            let cell = current_call();
            let mut guard = cell.lock().await;
            if let Some(call) = guard.as_mut() {
                if tinhat_streamer::aec_enabled() {
                    if let Err(e) = call.apply_aec() {
                        eprintln!("[aec] output rebind failed: {e}");
                    }
                    call.set_mic_muted(was_muted);
                }
            }
        });
        // If the speaker meter is running (settings Audio pane open), re-point it at the NEW
        // sink's `.monitor` — OFF the Qt GUI thread (a pipewiresrc build + PLAYING transition
        // would stall it). stop-then-start in ONE task so they can't race (a separate stop/start
        // spawn pair could let start no-op against the not-yet-torn-down old pipeline).
        if output_monitor_wanted().load(std::sync::atomic::Ordering::Relaxed) {
            let qt = self.qt_thread();
            crate::bridge::runtime().spawn(async move {
                tinhat_streamer::stop_output_monitor();
                let ok = tinhat_streamer::start_output_monitor();
                let _ = qt.queue(move |mut o| o.as_mut().set_output_monitoring(ok));
            });
        }
    }

    pub fn refresh_inputs(self: Pin<&mut Self>) {
        // Read the persisted MIC choice + enumerate OFF the Qt thread (the first saved-device
        // cell read can trigger a synchronous gst DeviceMonitor scan that stalls the UI), then
        // mirror the checkmark name + device list back.
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let saved = tinhat_streamer::call::mic_target_node().unwrap_or_default();
            let json = enumerate(tinhat_streamer::call::list_audio_inputs());
            let _ = qt.queue(move |mut o| {
                o.as_mut().set_input_name(QString::from(&saved));
                o.as_mut().set_inputs(QString::from(&json));
            });
        });
    }

    pub fn set_input(mut self: Pin<&mut Self>, node: &QString) {
        let node = node.to_string();
        tinhat_streamer::call::set_mic_target(
            if node.is_empty() { None } else { Some(node.clone()) },
        );
        self.as_mut().set_input_name(QString::from(&node));
        // Re-point the live mic at the new source, PRESERVING mute (restart_mic rebuilds the
        // capture pipeline to Playing — without this a muted mic would resume capturing locally
        // and the self-waveform would animate while the UI still says muted).
        //
        // Under AEC the capture reads the EC source, NOT the chosen device — what must be
        // re-bound to the new mic is the module's source_master, so route through apply_aec
        // (a bare restart_mic would rebuild against the ec_source still wired to the OLD
        // mic, making the device picker a silent no-op while AEC is on).
        let was_muted = *self.mic_muted();
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let cell = current_call();
            let mut guard = cell.lock().await;
            if let Some(call) = guard.as_mut() {
                let r = if tinhat_streamer::aec_enabled() {
                    call.apply_aec()
                } else {
                    call.restart_mic()
                };
                if let Err(e) = r {
                    eprintln!("[call] mic re-point failed: {e}");
                    // Surface it (live friends feedback: switches failed silently and
                    // read as "the call broke"). restart_mic keeps the OLD device
                    // running on failure, so say exactly that.
                    // 10.5 (audio review #8): with echo cancellation the old capture is retired FIRST,
                    // so "still using the previous microphone" was not always true.
                    let down = tinhat_streamer::mic_capture_down();
                    let _ = qt.queue(move |mut o| {
                        o.as_mut().set_status(QString::from(if down {
                            "Mic switch failed — the mic is off for now; the app keeps retrying (or pick another input)"
                        } else {
                            "Mic switch failed — still using the previous microphone"
                        }));
                    });
                }
                call.set_mic_muted(was_muted);
            }
        });
    }

    pub fn refresh_cameras(self: Pin<&mut Self>) {
        // Read the persisted CAMERA choice + enumerate OFF the Qt thread (the first saved-device
        // cell read can trigger a synchronous gst DeviceMonitor scan that stalls the UI), then
        // mirror the checkmark name + device list back.
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let saved = tinhat_streamer::call::camera_target_node().unwrap_or_default();
            let json = enumerate(tinhat_streamer::call::list_cameras());
            let _ = qt.queue(move |mut o| {
                o.as_mut().set_camera_name(QString::from(&saved));
                o.as_mut().set_cameras(QString::from(&json));
            });
        });
    }

    pub fn set_camera(mut self: Pin<&mut Self>, node: &QString) {
        let node = node.to_string();
        tinhat_streamer::call::set_camera_target(
            if node.is_empty() { None } else { Some(node.clone()) },
        );
        self.as_mut().set_camera_name(QString::from(&node));
        // Re-point the live camera at the new device, re-attaching the self-view.
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let cell = current_call();
            let mut guard = cell.lock().await;
            if let Some(call) = guard.as_mut() {
                match call.restart_camera() {
                    Ok(ptr) => {
                        if ptr != 0 {
                            video_registry().lock().unwrap().insert(SELFVIEW_ID.to_string(), ptr);
                            let _ = qt.queue(|mut o| {
                                // Bounce the self-view tile so the QML delegate recreates
                                // and its attachVideo Timer re-binds the new "sink".
                                let cur = o.as_ref().video_tiles().to_string();
                                o.as_mut().set_video_tiles(QString::from(&strip_selfview(&cur)));
                                let cur2 = o.as_ref().video_tiles().to_string();
                                o.as_mut().set_video_tiles(QString::from(&inject_selfview(&cur2)));
                                            let g = *o.as_ref().selfview_gen();
                                            o.as_mut().set_selfview_gen(g.wrapping_add(1));
                            });
                        }
                    }
                    Err(e) => {
                        // Was silently swallowed (`if let Ok`) — camera published
                        // nothing and the user had no idea why (live feedback).
                        eprintln!("[call] camera re-point failed: {e}");
                        let _ = qt.queue(move |mut o| {
                            o.as_mut().set_status(QString::from(
                                "Camera switch failed — try another camera or toggle it off/on",
                            ));
                        });
                    }
                }
            }
        });
    }

    /// Attach the QML tile `id` (GstGLQt6VideoItem) to its gst pipeline + start it on
    /// the render thread. Runs on the Qt GUI thread (QML invokable). Returns true once
    /// the item is realized + attached; QML retries on false.
    pub fn attach_video(self: Pin<&mut Self>, id: &QString) -> bool {
        let id = id.to_string();
        // Self-heal: if the eager VideoAdded GUI closure was missed (a rejoin race did
        // exactly this), build the pipeline now from the stashed track — we ARE on the Qt
        // GUI thread, which create_video_tile requires. Idempotent with the eager path
        // (the stashed track is consumed on first build).
        if !video_registry().lock().unwrap().contains_key(&id) {
            if let Some(ptr) = tinhat_streamer::call::create_video_tile_from_pending(&id) {
                video_registry().lock().unwrap().insert(id.clone(), ptr);
                eprintln!("[call] attach_video: lazily built tile {id} from pending track");
            }
        }
        Self::attach_tile_to(&id, &id)
    }

    /// #9 PiP pop-out: like `attach_video`, but bind tile `tile_id`'s pipeline to the QML
    /// item named `container` (a pop-out window's surface). Returns true once attached; QML
    /// retries on false. Runs on the Qt GUI thread.
    pub fn attach_video_to(self: Pin<&mut Self>, tile_id: &QString, container: &QString) -> bool {
        Self::attach_tile_to(&tile_id.to_string(), &container.to_string())
    }

    /// Shared attach core: bind tile `tile_id`'s pipeline to the QML container named
    /// `container`, REBUILDING the pipeline first when it is already live — that is the
    /// re-point case (PiP pop-out, pop-back, or a re-created grid delegate), and
    /// qml6glsink cannot re-bind a live pipeline to a new item (winsys init is
    /// NULL->READY-only: the popped-out stream rendered a permanently black box).
    /// rebuild_video_tile retires the old pipeline off-thread and returns a fresh one in
    /// NULL, which the normal attach dance then initializes against the container's own
    /// window. Runs on the Qt GUI thread.
    fn attach_tile_to(tile_id: &str, container: &str) -> bool {
        let ptr = video_registry().lock().unwrap().get(tile_id).copied();
        let Some(mut ptr) = ptr else { return false };
        // Only remote tiles are rebuildable; the SELF-VIEW is CallSession's camera
        // pipeline and keeps the legacy attach-as-is path (its existing GL item retains
        // a valid winsys, and its window never changes — the self-view can't pop out).
        if crate::video::is_live(ptr) && tinhat_streamer::call::has_video_tile(tile_id) {
            crate::video::predetach(ptr); // unbind the old item before the old pipeline dies
            match tinhat_streamer::call::rebuild_video_tile(tile_id) {
                Some(new_ptr) => {
                    video_registry().lock().unwrap().insert(tile_id.to_string(), new_ptr);
                    eprintln!("[call] {tile_id}: rebuilt pipeline for re-point -> {container}");
                    ptr = new_ptr;
                }
                None => return false,
            }
        }
        crate::video::attach(ptr, container)
    }

    /// #9 PiP (CPU viewer): start/stop the pop-out viewer tap. Pure pass-through to the
    /// streamer — no pipeline, registry, or GL interaction on this side.
    pub fn pip_start(self: Pin<&mut Self>, tile_id: &QString) -> bool {
        tinhat_streamer::call::pip_tap_start(&tile_id.to_string())
    }

    pub fn pip_stop(self: Pin<&mut Self>, tile_id: &QString) {
        tinhat_streamer::call::pip_tap_stop(&tile_id.to_string());
    }

    /// #9 Pause-hidden: pause/resume every remote tile pipeline. Skips the self-view (our own
    /// local preview, not a decode worth throttling). Runs on the Qt GUI thread.
    pub fn pause_remote_tiles(self: Pin<&mut Self>, paused: bool) {
        {
            let reg = video_registry().lock().unwrap();
            for (id, &ptr) in reg.iter() {
                if id == SELFVIEW_ID {
                    continue;
                }
                // NEVER touch a pipeline still in NULL (not yet attached): driving it to
                // PAUSED/PLAYING before the attach dance binds the widget makes the sink
                // fail its READY transition ("Required property 'widget' not set") and
                // leaves a POISONED gl_api=NONE context — glupload then rejects every
                // buffer and the tile is black for its whole life. Observed live: a
                // window-visibility resume racing a fresh tile's attach. The attach
                // dance is the only thing allowed to take a pipeline out of NULL.
                if !crate::video::is_live(ptr) {
                    continue;
                }
                crate::video::set_paused(ptr, paused);
            }
        }
        // Beyond the local gst pause (which only stops OUR decode), tell the SFU to stop
        // FORWARDING these streams to us — the real bandwidth stop. Publisher and other
        // receivers are unaffected; resuming re-signals every tile (idempotent).
        tinhat_streamer::call::set_window_video_paused(paused);
    }

    pub fn set_window_state(self: Pin<&mut Self>, state: &QString, saver_on: bool) {
        let state = state.to_string();
        let win = tinhat_widgets::lowgpu::WinState::parse(&state);
        // the self-view (a local preview) pauses on its last frame while hidden, or unfocused with the saver on
        let paused = tinhat_widgets::lowgpu::preview_paused(win, saver_on);
        {
            let reg = video_registry().lock().unwrap();
            if let Some(&ptr) = reg.get(SELFVIEW_ID) {
                // never drive a pipeline out of NULL before its attach (see pause_remote_tiles)
                if crate::video::is_live(ptr) {
                    crate::video::set_paused(ptr, paused);
                }
            }
        }
        tinhat_streamer::call::set_window_state(win.as_str(), saver_on);
    }

    pub fn set_popout_tiles(self: Pin<&mut Self>, ids_json: &QString) {
        let ids: Vec<String> = serde_json::from_str(&ids_json.to_string()).unwrap_or_default();
        tinhat_streamer::call::set_popout_tiles(ids);
    }
}

/// Serialize a device list `[(node, label)]` to the JSON the QML pickers parse.
fn enumerate(list: Vec<(String, String)>) -> String {
    let arr: Vec<serde_json::Value> = list
        .into_iter()
        .map(|(node, label)| serde_json::json!({ "node": node, "label": label }))
        .collect();
    serde_json::to_string(&arr).unwrap_or_else(|_| "[]".to_string())
}

/// Resolve the room's active call, mint a token, announce membership + our media key,
/// connect the E2EE Room and publish our mic. Returns the call-event receiver the
/// caller drains to drive the video-tile grid.
/// Leave any active call on app exit (called from main.rs after `app.exec()` returns, i.e.
/// a clean window-close). The load-bearing half is `CallSession::close()` — it sends the
/// LiveKit LeaveRequest so the SFU drops our participant immediately; otherwise a ghost
/// lingers under our `userId:deviceId` identity and can wedge the NEXT rejoin in
/// "connecting…". Then best-effort withdraws our MatrixRTC membership, bounded by a 5s
/// timeout so a dead network can't hang the quit. No QObject access (the engine is gone).
pub async fn leave_on_exit() {
    let taken = current_call().lock().await.take();
    let Some(call) = taken else {
        // A hang-up teardown won the take() and is still in flight. Wait for it (so its SFU
        // LeaveRequest actually lands) instead of racing it to process exit — otherwise we'd
        // orphan a ghost participant. Bounded by main.rs's outer 20s quit timeout.
        let pending = teardown_handle().lock().unwrap().take();
        if let Some(h) = pending {
            let _ = h.await;
        }
        // A join may have PUBLISHED our m.call.member but not yet stored the session into
        // current_call (do_join sets active_call_room right after join_rtc_session) — e.g.
        // quitting during an auto-rejoin's connect. Withdraw that membership too, or peers
        // keep seeing a ghost until the 4h expiry.
        let rid_str = active_call_room().lock().unwrap().take();
        if let (Some(rid_str), Some(client)) = (rid_str, crate::session::client()) {
            if let Ok(rid) = matrix_sdk::ruma::RoomId::parse(&rid_str) {
                if let Some(room) = client.get_room(&rid) {
                    let _ = tokio::time::timeout(
                        std::time::Duration::from_secs(5),
                        crate::rtc::leave_rtc_session(&room),
                    )
                    .await;
                    eprintln!("[call] withdrew in-flight join's membership on exit");
                }
            }
        }
        return;
    };
    // close() is internally bounded, but wrap it so a stall can never hang app quit. Phase 1
    // of close() (the synchronous local NULLs that release /dev/video0) runs before the first
    // await, so the camera is freed even if the timeout fires on the network phase.
    if tokio::time::timeout(std::time::Duration::from_secs(10), call.close())
        .await
        .is_err()
    {
        eprintln!("[call] WARN: close() on exit exceeded 10s — quitting anyway");
    }
    *current_media_key().lock().unwrap() = None;
    let rid_str = active_call_room().lock().unwrap().take();
    if let (Some(rid_str), Some(client)) = (rid_str, crate::session::client()) {
        if let Ok(rid) = matrix_sdk::ruma::RoomId::parse(&rid_str) {
            if let Some(room) = client.get_room(&rid) {
                let _ = tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    crate::rtc::leave_rtc_session(&room),
                )
                .await;
            }
        }
    }
    eprintln!("[call] left call on exit");
}

/// Sign-out (bridge.rs end_session): the call ends as a deliberate hang-up (no auto-rejoin brings it back, no join in
/// flight finishes, no pending switch runs), then as on exit (leave_on_exit: the call service told, devices released,
/// the membership withdrawn while the token still works). Bounded like the quit. The call's controls are reset by the
/// shell when the sign-in screen comes back (Main.qml onMLoggedInChanged → leaveCall).
pub async fn leave_for_sign_out() {
    join_gen().fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    *call_started().lock().unwrap() = None;
    auto_rejoins().store(0, std::sync::atomic::Ordering::SeqCst);
    *rejoin_mic_muted().lock().unwrap() = None;
    *pending_switch().lock().unwrap() = None;
    if tokio::time::timeout(std::time::Duration::from_secs(15), leave_on_exit()).await.is_err() {
        eprintln!("[call] WARN: leaving the call at sign-out took over 15 s — signing out all the same");
    }
}

/// 10.1 (TH-CALL-03): the three common join failures in plain words; anything else keeps the
/// raw text (it is still shown, just under a sentence a person can act on).
fn join_fail_reason(raw: &str) -> String {
    let l = raw.to_ascii_lowercase();
    if l.contains("timed out") || l.contains("timeout") || l.contains("deadline") {
        "the call server didn't answer. Check your connection and try again.".to_string()
    } else if l.contains("already") && (l.contains("call") || l.contains("participant") || l.contains("identity")) {
        "you're already in this call on another device.".to_string()
    } else if l.contains("dns") || l.contains("resolve") || l.contains("connection refused") || l.contains("unreachable") || l.contains("network") || l.contains("connect") {
        "couldn't reach the call server. Check your connection and try again.".to_string()
    } else if l.contains("not signed in") {
        "you're not signed in.".to_string()
    } else {
        raw.to_string()
    }
}

async fn do_join(
    room_id: &str,
    start_muted: bool,
    my_gen: u64,
) -> Result<UnboundedReceiver<CallEvent>, String> {
    let client = crate::session::client().ok_or_else(|| "Not signed in".to_string())?;
    let rid = matrix_sdk::ruma::RoomId::parse(room_id).map_err(|e| format!("bad room id: {e}"))?;
    let room = client
        .get_room(&rid)
        .ok_or_else(|| "Room not found".to_string())?;
    let _joining = JoiningMark::set(my_gen, room_id); // peers' keys for this room count from here (key_intake_room)
    dump_memberships(&room).await; // ground-truth: what membership format peers use



    // Join the active call's focus if one exists; otherwise START a call on the server's own call service (its
    // well-known; the homeserver's /livekit/jwt when it names none) — we become the first member. As Tin Hat Web's resolveFocus.
    let focus = match crate::rtc::resolve_focus(&room).await.map_err(|e| format!("call lookup failed: {e}"))? {
        Some(f) => f,
        None => {
            let f = crate::rtc::default_focus(&client, &room).await;
            eprintln!("[call] no active call — starting one on the server's call service ({})", f.service_url);
            f
        }
    };
    // Calls are ALWAYS end-to-end encrypted. Guest calls (0.8.5) no longer
    // divert the call into an unencrypted named LiveKit room: a browser guest now gets
    // a temporary Matrix account and joins THIS call like anyone else, so there is no
    // second room for anyone to sit alone in and nothing to switch encryption off for.
    let e2ee = true;
    // Seed the "is a browser watching?" flag before anything is published, so
    // the first share picks the right codec instead of waiting for the periodic scan.
    {
        let others = crate::rtc::non_native_call_members(&room).await;
        tinhat_streamer::call::set_browser_peers_present(!others.is_empty());
        if !others.is_empty() {
            eprintln!("[call] non-native peers already in call: {others:?} (share codec = H.264)");
        }
    }
    // Bounded: lk-jwt-service is reached with a client that has no timeout of its own, and a
    // join stalled here is exactly the attempt that later resumes and tramples a NEWER
    // call (Astra review 2026-09-21, finding 5).
    let sfu = match tokio::time::timeout(
        std::time::Duration::from_secs(20),
        crate::livekit::request_sfu_config(&client, &focus.service_url, &focus.alias),
    )
    .await
    {
        Ok(r) => r?,
        Err(_) => return Err("lk-jwt-service request timed out (20s)".to_string()),
    };
    // Finding 5 checkpoints: `my_gen` is bumped by any later leave_call/join_call. From here
    // on every step that touches SHARED call state re-checks it first, so a superseded
    // attempt stands down instead of closing the newer call's session, republishing a
    // stale membership, overwriting current_media_key or publishing its microphone.
    if join_gen_stale(my_gen) {
        return Err("superseded while fetching the SFU config".to_string());
    }

    // Close any leftover session FIRST — before we publish our membership, and long
    // before connect registers the new keyring. Two reasons it must be here and not
    // later: (a) close() clears the buffered peer keys, and the moment our membership
    // lands peers start pushing us theirs — closing after that publish would discard
    // exactly the keys the rejoin needs; (b) close() used to clear the NEW session's
    // keyring registration outright (now also generation-guarded, as a belt).
    if let Some(old) = current_call().lock().await.take() {
        eprintln!("[call] closing leftover session before rejoin");
        let _ = tokio::time::timeout(std::time::Duration::from_secs(10), old.close()).await;
    }
    if join_gen_stale(my_gen) {
        return Err("superseded before publishing the membership".to_string());
    }

    // Publish our m.call.member with real retries. Peers pick to-device key recipients
    // straight from these memberships — an unpublished membership makes us invisible to
    // EC key distribution in BOTH directions (we hear static, they hear silence), so a
    // one-shot "warning" here was a whole-call failure. If all attempts fail, keep
    // retrying in the background until it lands or the call ends.
    {
        let mut mem_ok = false;
        for (attempt, backoff_ms) in [0u64, 1_000, 3_000].into_iter().enumerate() {
            if backoff_ms > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
            }
            match crate::rtc::join_rtc_session(&room, &focus.service_url).await {
                Ok(()) => {
                    mem_ok = true;
                    break;
                }
                Err(e) => eprintln!("[call] membership publish attempt {} failed: {e}", attempt + 1),
            }
        }
        if !mem_ok {
            eprintln!("[call] membership NOT published after 3 attempts — peers can't key us; retrying in background");
            let room = room.clone();
            let svc = focus.service_url.clone();
            let rid = room_id.to_string();
            crate::bridge::runtime().spawn(async move {
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(15)).await;
                    if active_call_room().lock().unwrap().as_deref() != Some(rid.as_str()) {
                        break;
                    }
                    match crate::rtc::join_rtc_session(&room, &svc).await {
                        Ok(()) => {
                            // The call may have ENDED while that request was in flight —
                            // leave's withdrawal would then already have run, and this
                            // publish would resurrect our membership as a 4h ghost that
                            // peers keep showing (and keying). Compensate immediately.
                            if active_call_room().lock().unwrap().as_deref() != Some(rid.as_str()) {
                                eprintln!("[call] membership retry landed after the call ended — withdrawing");
                                withdraw_membership_best_effort(&room).await;
                            } else {
                                eprintln!("[call] membership published on background retry");
                            }
                            break; // the 60s key re-announce loop covers the key from here
                        }
                        Err(e) => eprintln!("[call] membership background retry failed: {e}"),
                    }
                }
            });
        }
    }
    // Our m.call.member is now (best-effort) PUBLISHED: record the room immediately so
    // leave_on_exit can withdraw it even if the app quits before this join finishes
    // (current_call is only set at the very end). Failure paths below clear it.
    *active_call_room().lock().unwrap() = Some(room_id.to_string());
    *active_focus_url().lock().unwrap() = Some(focus.service_url.clone()); // Pad: republish_membership()

    // Refresh our MatrixRTC membership before EC's 4h expiry lapses, so peers keep
    // enumerating us as a key recipient (and re-send rotations) on long calls. Ends
    // when the call closes (current_call cleared) OR when a same-room rejoin
    // supersedes this join (join_gen check — the room-id/current_call guards alone
    // are blind to leave+rejoin of the SAME room, so the old loop survived it and
    // every long call after a rejoin carried PAIRED 30-min refreshes, each pair
    // triggering a synchronized web-peer key-rotation storm; seen in field logs).
    {
        let room = room.clone();
        let svc = focus.service_url.clone();
        let rid = room_id.to_string();
        crate::bridge::runtime().spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(30 * 60)).await;
                // Scoped to THIS call, not "any call": an "is a call active?" check alone
                // kept the loop alive across room switches — a later tick then re-minted a
                // ghost m.call.member for a room we LEFT (and announced the CURRENT call's
                // media key to it). The current_call check stays: teardown clears
                // active_call_room only AFTER its bounded close/leave (up to ~12s later).
                let call_gone = current_call().lock().await.is_none();
                let room_switched =
                    active_call_room().lock().unwrap().as_deref() != Some(rid.as_str());
                if call_gone || room_switched || join_gen_stale(my_gen) {
                    break;
                }
                match crate::rtc::join_rtc_session(&room, &svc).await {
                    Ok(()) => {
                        eprintln!("[call] membership refreshed");
                        // Re-announce our (unchanged) media key too, so a peer who missed the
                        // original to-device delivery and won't re-fire ParticipantJoined (they
                        // were already connected) can still decrypt us. Idempotent: same
                        // index+key, so already-keyed peers are unaffected.
                        let cur = *current_media_key().lock().unwrap();
                        if let Some((key, index)) = cur {
                            if let Some(c) = crate::session::client() {
                                let uid = c.user_id().map(|u| u.to_string()).unwrap_or_default();
                                let did = c.device_id().map(|d| d.to_string()).unwrap_or_default();
                                announce_key_to_members(&c, &room, &uid, &did, key, index).await;
                            }
                        }
                    }
                    Err(e) => eprintln!("[call] membership refresh failed: {e}"),
                }
            }
        });
    }

    // Superseded after our membership landed: don't mint/publish a key or set
    // current_media_key (they would belong to the newer attempt). The membership in THIS
    // room is withdrawn only if the newer attempt is not (yet) claiming this room.
    if join_gen_stale(my_gen) {
        if active_call_room().lock().unwrap().as_deref() != Some(room_id) {
            withdraw_membership_best_effort(&room).await;
        }
        return Err("superseded after publishing the membership".to_string());
    }
    let device_id = client.device_id().map(|d| d.to_string()).unwrap_or_default();
    let user_id = client.user_id().map(|u| u.to_string()).unwrap_or_default();
    let identity = format!("{user_id}:{device_id}");
    // Mint a fresh key at an ADVANCING index (persisted): re-minting at a fixed index 0
    // made every rejoin collide with our previous key in every peer's (identity,index)
    // tracking — web EC's OutdatedKeyFilter could silently discard the fresh key and our
    // own room poll could stomp it. A never-before-used index has no history anywhere.
    let key_index = crate::e2ee_keys::advance_persisted_key_index();
    let (key, _) = crate::e2ee_keys::make_sender_key();
    // The ROOM-EVENT key mirror is retired (2026-09-21). It was a plaintext leak in
    // unencrypted rooms (Astra review, finding 3) and, in encrypted rooms, every mirror
    // landed on the server as `m.room.encrypted` — a type no push rule can quiet — so a
    // long call chimed every participant every 15 minutes (field report). EC
    // never read it; Tin Hat peers get our key over encrypted to-device below, on every
    // peer join, and every steady-loop tick.
    // Never start a call already flagged away to peers (leave/quit/crash while BRB was on
    // leaves the state event set — clear_intent only resets the local atomic).
    crate::afk_ctl::clear_stale_away(&room).await;
    // Stash our key for peer-join re-announces, and announce it NOW over ENCRYPTED
    // to-device to peers already in the call (EC's ToDeviceKeyTransport ignores the room
    // event above). A peer that joins LATER is covered by the ParticipantJoined re-announce.
    *current_media_key().lock().unwrap() = Some((key, key_index));
    announce_key_to_members(&client, &room, &user_id, &device_id, key, key_index).await;

    // (Any leftover session was closed at the top of do_join, BEFORE the membership
    // publish — see the comment there: closing it here would wipe the peer keys that
    // arrive the instant our membership lands.)

    // Bound the connect so a stale call (a ghost participant left by a previous unclean
    // exit) can't wedge us in "connecting…" forever — fail out so the user can just retry
    // (by which point the SFU has usually timed the ghost out) instead of leave+restart.
    let (mut call, rx) = match tokio::time::timeout(
        std::time::Duration::from_secs(15),
        CallSession::connect_opts(&sfu.url, &sfu.jwt, &identity, key, key_index, e2ee),
    )
    .await
    {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            // connect() may have set the shared "in a call" globals before failing, and
            // the membership was already published — clean both up (else: soundboard
            // "in a call" against a dead room + a ghost participant for peers).
            // UNLESS superseded: then the globals, the membership state key and
            // active_call_room all belong to the NEWER join — destroying them here was
            // the 2026-08-19 zombie-call bug.
            if join_gen_stale(my_gen) {
                eprintln!("[call] stale join (gen {my_gen}) failed late — skipping cleanup, a newer join owns the call state");
                return Err(format!("connect (superseded): {e}"));
            }
            tinhat_streamer::abandon_partial_connect();
            withdraw_membership_best_effort(&room).await;
            *active_call_room().lock().unwrap() = None;
            return Err(format!("connect: {e}"));
        }
        Err(_) => {
            if join_gen_stale(my_gen) {
                eprintln!("[call] stale join (gen {my_gen}) timed out — skipping cleanup, a newer join owns the call state");
                return Err("connect timed out (superseded)".to_string());
            }
            tinhat_streamer::abandon_partial_connect();
            withdraw_membership_best_effort(&room).await;
            *active_call_room().lock().unwrap() = None;
            return Err("connect timed out — the call may be stale, try again".to_string());
        }
    };
    // RBAC (§5.4 "own state"): if the call room's `dev.tinhat.call.moderation` says a
    // moderator muted me, join WITHOUT a mic track — the streamer marks me server-muted (mic
    // button locked, soundboard refused) until an `unmute` packet / the SFU says otherwise,
    // and a present moderator's client re-enforces the SFU side on our ParticipantJoined.
    let mic_result = match crate::moderation::read_muted_by(room_id, &user_id).await {
        Some(by) => {
            refresh_call_names(room_id).await; // so "muted by <name>" resolves at once
            let by_name = name_for_mxid(&by);
            eprintln!("[moderation] room state says {by:?} muted me — joining without a mic track");
            tinhat_streamer::set_server_muted_from_state(&by, &by_name);
            Ok(())
        }
        None => {
            // Last checkpoint before our microphone goes on the air: a superseded attempt
            // must not publish it at all (finding 5's "briefly live mic").
            if join_gen_stale(my_gen) {
                eprintln!("[call] join (gen {my_gen}) superseded before the mic publish — closing this session");
                call.close().await;
                return Err("superseded by a newer join".to_string());
            }
            call.publish_mic_muted(start_muted).await
        }
    };
    if let Err(e) = mic_result {
        // connect() set the streamer's aux_participant; if mic publish fails the CallSession is
        // dropped WITHOUT close(), so clear it here (else a stale dead participant clone lingers
        // until the next join). current_call is never set on this path, so the soundboard still
        // correctly no-ops, but we clear defensively so nothing half-built leaks. close() does
        // not withdraw the already-published membership — do that explicitly (ghost otherwise).
        call.close().await;
        // Only clean the shared state if this attempt still owns it (see the connect
        // arms above). Closing OUR OWN session is always correct.
        if !join_gen_stale(my_gen) {
            withdraw_membership_best_effort(&room).await;
            *active_call_room().lock().unwrap() = None;
        }
        return Err(format!("mic: {e}"));
    }
    // Superseded while connecting: storing this session would clobber (and silently
    // drop, un-closed) the newer attempt's session in current_call. Close ours and
    // stand down; the membership state key already belongs to the newer attempt.
    if join_gen_stale(my_gen) {
        eprintln!("[call] join (gen {my_gen}) superseded during connect — closing this session");
        call.close().await;
        return Err("superseded by a newer join".to_string());
    }
    let cell = current_call();
    // (Any leftover session was already closed BEFORE connect — see above.)
    *cell.lock().await = Some(call);
    *active_call_room().lock().unwrap() = Some(room_id.to_string()); // for leave-on-exit
    // Steady key re-announce: every 60s, re-send our (unchanged) media key over encrypted
    // to-device to every live call-member device, and mirror it as a room event every 15th
    // tick (see below — the delivery-critical room mirrors are event-driven now, so this
    // one is a lost-in-flight backstop). This is the safety net for every one-shot
    // announce failure mode — a device
    // that wasn't resolvable yet (keys_query lag), membership sync lag past the ladder's
    // 30s horizon, a lost to-device message, or a web client that fell back to the
    // room-only key transport (it permanently IGNORES to-device keys from then on).
    // Idempotent: identical bytes at the same index — receivers just overwrite.
    {
        let room = room.clone();
        let rid = room_id.to_string();
        let our_key = (key, key_index);
        crate::bridge::runtime().spawn(async move {
            let mut tick: u64 = 0;
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                if current_call().lock().await.is_none()
                    || active_call_room().lock().unwrap().as_deref() != Some(rid.as_str())
                {
                    break;
                }
                // A fast leave+rejoin of the SAME room mints a new key; the rejoin's own
                // loop owns it from there — this stale loop must stand down, not race it.
                if *current_media_key().lock().unwrap() != Some(our_key) {
                    break;
                }
                let (key, index) = our_key;
                let Some(c) = crate::session::client() else { break };
                let uid = c.user_id().map(|u| u.to_string()).unwrap_or_default();
                let did = c.device_id().map(|d| d.to_string()).unwrap_or_default();
                announce_key_to_members(&c, &room, &uid, &did, key, index).await;
                tick += 1;
                // No room-event mirror any more (retired 2026-09-21, see do_join): the
                // to-device re-announce above is the whole transport. Those mirrors landed
                // in the TIMELINE forever (one room: 2265 of its last 2400 events)
                // and, encrypted, rang every peer's notifier every 15 minutes of a call.
            }
        });
    }
    // One-shot self-heal for the join-time "static mic": the capture feeder flags either
    // heavy audiorate corrections (gross timeline breaks) OR feeder-visible damage
    // (post-prime underruns / duplicate-frame concealment — the class audiobuffersplit's
    // re-timestamping hides from audiorate, which is why the r129 corrections-only
    // detector never fired) in its first seconds. A capture rebuild — the same thing the
    // RNNoise toggle does, which users discovered fixes it manually — clears it. Checked
    // twice early, loudly logged. restart_mic reuses the published track/source, so mute
    // state and E2EE are untouched. The stable-delivery priming gate in the feeder should
    // keep the garble from ever reaching peers; this heal is the backstop.
    {
        let rid = room_id.to_string();
        let our_key = (key, key_index);
        crate::bridge::runtime().spawn(async move {
            // CALL-LIFETIME healer (was a two-shot at +3s/+8.5s): the feeder now flags
            // BOTH the join-time cold-node signature AND mid-call stalls (a primed
            // capture that stops delivering — the "his mic just died until he toggled
            // something" class). start_mic_capture clears the flag on every fresh
            // capture, so a consumed flag is always about the CURRENT one; a persisting
            // fault re-raises it.
            //
            // Rebuild spacing uses EXPONENTIAL BACKOFF (5s → 10s → … → 5min) rather than
            // a lifetime cap: a device that dies and stays dead (USB unplug, dock reset)
            // would burn a fixed budget in the first minute and then never heal again
            // when it came back. Backoff bounds the rebuild rate the same way while
            // keeping the healer alive for the whole call, and it RESETS after a
            // sustained-healthy stretch so a later, unrelated fault heals promptly.
            let mut backoff = std::time::Duration::from_secs(5);
            let mut last_heal: Option<std::time::Instant> = None;
            let mut heals: u32 = 0;
            // 0.8.29: rebuilds taken WITHOUT backoff because the previous chain
            // never primed (the streamer already blocked its rung). Bounded per call.
            let mut fast_heals: u32 = 0;
            // Locally-latched "a heal is owed". The flag MUST be latched rather than
            // re-read later: the early-window watch stops raising it after +8s, so a
            // flag consumed-and-discarded during backoff cost an extra ~18s of dead mic
            // waiting for the slow rolling-window detector to re-raise it (a field
            // incident: 28s to heal instead of ~10s).
            let mut heal_pending = false;
            // One-shot AEC recovery: the Windows mic builder drops echo-cancel from the
            // chain when webrtcdsp can't bind its reference probe mid-rebind ("mic
            // beats AEC"). Nothing retried before, so the user ran echo-cancel-free for
            // the rest of the call with no indication (a field report). One delayed
            // rebuild re-attaches AEC once the tap has settled; if the probe is still
            // broken the builder falls back again and we stop (single retry per call).
            let mut aec_retry_at: Option<std::time::Instant> = None;
            let mut aec_retried = false;
            let mut last_ensure: Option<std::time::Instant> = None;
            tinhat_streamer::call::ec_rebind_reset(); // 10.5.2: the echo-reference follower is per call
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                // Same guards as the key loops: a fast leave+rejoin of the SAME room
                // gets its own healer, so this one must stand down rather than fight it
                // (the room id alone is identical across a rejoin).
                if active_call_room().lock().unwrap().as_deref() != Some(rid.as_str())
                    || *current_media_key().lock().unwrap() != Some(our_key)
                    || current_call().lock().await.is_none()
                {
                    break;
                }
                if tinhat_streamer::call::take_aec_fallback() && !aec_retried {
                    aec_retry_at =
                        Some(std::time::Instant::now() + std::time::Duration::from_secs(10));
                }
                if aec_retry_at.map(|t| std::time::Instant::now() >= t).unwrap_or(false) {
                    aec_retry_at = None;
                    aec_retried = true;
                    let cell = current_call();
                    let mut guard = cell.lock().await;
                    if let Some(call) = guard.as_mut() {
                        eprintln!("[aec] retrying the echo-cancel mic chain after the earlier fallback");
                        if let Err(e) = call.restart_mic() {
                            eprintln!("[aec] retry rebuild failed: {e}");
                        }
                    }
                    continue;
                }
                // 10.5.2 (echo cancellation only worked well when the outputs were selected
                // explicitly): the automatic echo reference follows where the call ACTUALLY plays — a
                // volume-applet move, a remembered route, a default change, headset ↔ speakers. The look
                // is a few pactl round-trips, so it runs off the async workers; a rebind re-plans the
                // reference and rebuilds the mic (apply_aec), exactly like an output pick does.
                if tokio::task::spawn_blocking(tinhat_streamer::call::ec_rebind_wanted).await.unwrap_or(false) {
                    // (Astra) the look took a moment: act only if this is still the same call
                    let same_call = || {
                        active_call_room().lock().unwrap().as_deref() == Some(rid.as_str())
                            && *current_media_key().lock().unwrap() == Some(our_key)
                    };
                    if same_call() {
                        let cell = current_call();
                        let mut guard = cell.lock().await;
                        if let Some(call) = guard.as_mut().filter(|_| same_call()) {
                            if let Err(e) = call.apply_aec() {
                                eprintln!("[aec] rebinding the echo reference failed: {e}");
                            }
                        }
                    }
                    continue;
                }
                // 10.5 (audio review #5): the mic we mean to have is not published at all (a failed
                // AGC/NS re-publish left no handle) — publish it again, every 10 s until it takes.
                // Bounded (Astra 2026-09-22 #1): this holds the call mutex, which hang-up needs before its
                // own timeout starts — a publish waiting on the SDK's reconnect must not hold it for long;
                // and while the link is down there is nothing to publish into.
                if tinhat_streamer::mic_wanted()
                    && !tinhat_streamer::hear::reconnecting()
                    && last_ensure.map(|t| t.elapsed() >= std::time::Duration::from_secs(10)).unwrap_or(true)
                {
                    let cell = current_call();
                    let mut guard = cell.lock().await;
                    if let Some(call) = guard.as_mut() {
                        if !call.has_mic() {
                            last_ensure = Some(std::time::Instant::now());
                            match tokio::time::timeout(std::time::Duration::from_secs(6), call.ensure_mic()).await {
                                Ok(Ok(true)) => eprintln!("[mic-heal] the mic is published again"),
                                Ok(Ok(false)) => {}
                                Ok(Err(e)) => eprintln!("[mic-heal] publishing the mic again failed: {e:#} — retrying in 10 s"),
                                Err(_) => eprintln!("[mic-heal] publishing the mic again took over 6 s — retrying in 10 s"),
                            }
                            continue;
                        }
                    }
                }
                if tinhat_streamer::take_mic_capture_unhealthy() {
                    heal_pending = true;
                }
                // 10.5 (audio review #2c): a capture that is DOWN (every rebuild chain failed after
                // the old capture was retired) has no feeder left to re-raise the flag — so the
                // silence below is not "clean", and the request must survive until something captures.
                let down = tinhat_streamer::mic_capture_down();
                if down {
                    heal_pending = true;
                }
                // 0.8.29: a latch is only worth acting on while the fault is STILL
                // being observed. The streamer stamps every raise (a stall re-raises every
                // 10 ms, a degraded capture every 10 s window); nothing re-raised for
                // CLEAN_S seconds means the capture has been clean that long — drop the
                // request instead of tearing down a healthy capture. (A field log: heal 7
                // destroyed 299 s of flawless audio on a latch five minutes old, the
                // replacement never primed, and the 300 s backoff left him mute.)
                const CLEAN_S: u64 = 25;
                let bad_age = tinhat_streamer::mic_capture_bad_age();
                let still_bad = bad_age.map(|a| a.as_secs() < CLEAN_S).unwrap_or(false);
                if heal_pending && !still_bad && !down {
                    eprintln!(
                        "[mic-heal] dropping a stale heal request — the capture has been clean for {} s",
                        bad_age.map(|a| a.as_secs()).unwrap_or(0)
                    );
                    heal_pending = false;
                }
                // Backoff reset on sustained health — evaluated UNCONDITIONALLY. It used to
                // sit behind `if !heal_pending`, unreachable while a latch was held through
                // a backoff, so the backoff could only ever grow.
                if !still_bad && last_heal.map(|t| t.elapsed().as_secs() > 120).unwrap_or(false) {
                    backoff = std::time::Duration::from_secs(5);
                    last_heal = None;
                }
                // A rebuilt chain that never delivered a single buffer is a dead chain, not a
                // hiccup: the streamer has already blocked its rung, so rebuild NOW (bounded
                // per call) instead of waiting out a backoff with a silent mic.
                let never_primed = tinhat_streamer::take_mic_never_primed();
                if never_primed && fast_heals < 3 && !tinhat_streamer::mic_capture_primed() {
                    fast_heals += 1;
                    heal_pending = true;
                    // 10.0.3: two pipewiresrc chains that never delivered a buffer inside the
                    // Flatpak = the runtime's PipeWire plugin cannot read this source; the third
                    // rebuild captures through PulseAudio-compat instead (same node, same name).
                    // 10.0.6: symmetric — whichever audio API the dead captures used, the next
                    // rebuild tries the other one (PulseAudio-compat <-> native PipeWire).
                    if fast_heals >= 2 && tinhat_streamer::in_flatpak_sandbox() {
                        if !tinhat_streamer::pulse_head_wanted() {
                            tinhat_streamer::prefer_native_audio().store(false, std::sync::atomic::Ordering::Relaxed);
                            tinhat_streamer::prefer_pulse_mic().store(true, std::sync::atomic::Ordering::Relaxed);
                            eprintln!("[mic-heal] two PipeWire captures never delivered a buffer inside the Flatpak — the next rebuild uses the PulseAudio path (pulsesrc)");
                        } else if !tinhat_streamer::prefer_native_audio().load(std::sync::atomic::Ordering::Relaxed) {
                            tinhat_streamer::prefer_native_audio().store(true, std::sync::atomic::Ordering::Relaxed);
                            eprintln!("[mic-heal] two PulseAudio-compat captures never delivered a buffer inside the Flatpak — the next rebuild uses native PipeWire (pipewiresrc)");
                        }
                    }
                    tinhat_streamer::pw_snapshot("mic never primed");
                    eprintln!(
                        "[mic-heal] the last capture never primed — rebuilding on the next rung \
                         without waiting out the backoff ({fast_heals}/3)"
                    );
                } else if !heal_pending {
                    continue;
                } else if last_heal.map(|t| t.elapsed() < backoff).unwrap_or(false) {
                    continue; // too soon; the latch stays until the backoff expires or it goes stale
                }
                let cell = current_call();
                let mut guard = cell.lock().await;
                if let Some(call) = guard.as_mut() {
                    heals += 1;
                    last_heal = Some(std::time::Instant::now());
                    heal_pending = false; // this latch is spent; a persisting fault re-raises
                    eprintln!(
                        "[mic-heal] rebuilding mic capture (heal {heals}, next retry ≥{}s)",
                        backoff.as_secs()
                    );
                    if let Err(e) = call.restart_mic() {
                        eprintln!("[mic-heal] rebuild failed: {e}");
                    }
                    // Cap 60 s (was 300): with the stale-latch guard above a rebuild can no
                    // longer hit a healthy capture, so a long lockout buys nothing.
                    if !never_primed {
                        backoff = (backoff * 2).min(std::time::Duration::from_secs(60));
                    }
                }
            }
        });
    }
    Ok(rx)
}

/// Best-effort, bounded withdrawal of our m.call.member — for do_join failure paths that
/// run AFTER the membership publish: without it a failed join leaves a ghost participant
/// in every peer's UI (they enumerate from the state event) for up to the 4h expiry.
async fn withdraw_membership_best_effort(room: &matrix_sdk::Room) {
    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(8),
        crate::rtc::leave_rtc_session(room),
    )
    .await;
}

/// One-shot diagnostic: dump peers' raw call-membership events so we can see the
/// EXACT format this deployment uses (legacy m.call.member vs new m.rtc.member) and
/// replicate it, so peers' membership manager recognizes our device.
async fn dump_memberships(room: &matrix_sdk::Room) {
    let mut opts = matrix_sdk::room::MessagesOptions::backward();
    opts.limit =
        matrix_sdk::ruma::UInt::new(120).unwrap_or_else(|| matrix_sdk::ruma::UInt::from(120u16));
    opts.filter.types = Some(vec![
        "m.call.member".to_owned(),
        "m.rtc.member".to_owned(),
        "org.matrix.msc3401.call.member".to_owned(),
    ]);
    match room.messages(opts).await {
        Ok(resp) => {
            let mut seen = std::collections::HashSet::new();
            let mut n = 0;
            for ev in &resp.chunk {
                if let Ok(v) = ev.raw().deserialize_as::<serde_json::Value>() {
                    let ty = v.get("type").and_then(|s| s.as_str()).unwrap_or("?").to_string();
                    let sk = v.get("state_key").and_then(|s| s.as_str()).unwrap_or("").to_string();
                    if seen.insert((ty.clone(), sk.clone())) {
                        n += 1;
                        eprintln!("[memdump] {}", serde_json::to_string(&v).unwrap_or_default());
                    }
                }
            }
            eprintln!("[memdump] {n} distinct membership events dumped");
        }
        Err(e) => eprintln!("[memdump] failed: {e}"),
    }
}

/// Log one non-key to-device event (`v` = the WHOLE event {type, sender, content}). Peers'
/// `io.element.call.encryption_keys` never install from here: the sync loop hands the
/// Olm-decrypted ones to crate::key_intake (the trust rules) and rejects the rest.
fn ingest_todevice_value(v: &serde_json::Value, src: &str) {
    let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    if ty == crate::e2ee_keys::ENCRYPTION_KEYS_EVENT {
        // Media keys are taken ONLY through crate::key_intake (the trust rules); nothing here installs one.
        eprintln!(
            "[call] encryption_keys ({src}) from {} not taken outside the key intake",
            v.get("sender").and_then(|s| s.as_str()).unwrap_or("?")
        );
    } else if ty != "m.room.key" && ty != "m.room_key" && ty != "m.room.encrypted" {
        eprintln!("[td/{src}] {ty} from {}", v.get("sender").and_then(|s| s.as_str()).unwrap_or("?"));
    }
}

/// Process one to-device event straight from the sync response (more reliable than
/// a to-device `add_event_handler` under the manual `sync_once` loop): logs non-megolm
/// types so we can see exactly what to-device traffic peers send us (media keys go
/// through crate::key_intake instead).
pub fn ingest_todevice_raw(raw: &Raw<AnyToDeviceEvent>) {
    if let Ok(v) = raw.deserialize_as::<serde_json::Value>() {
        ingest_todevice_value(&v, "sync");
    }
}

/// Watch the call's E2EE key state while it lasts: every few seconds, escalate peers stuck in
/// MissingKey (>10 s = the user has a black/silent peer). Peers' keys arrive ONLY over
/// Olm-encrypted to-device through crate::key_intake: the room-event key intake (the retired
/// Element Call / pre-2026-09-21 natives' room mirror) is removed — room events carry no Olm
/// device binding and no call-membership check, and let any room member fill the peer-key
/// registry (the web removed it 2026-09-23; natives stopped writing them 2026-09-21).
async fn install_peer_keys_loop(room_id: String) {
    eprintln!("[recv0-keywatch] key watch started for {room_id} (to-device keys only)");
    let mut tick: u32 = 0;
    loop {
        if current_call().lock().await.is_none()
            || active_call_room().lock().unwrap().as_deref() != Some(room_id.as_str())
        {
            break; // call ended (or the active call moved to another room)
        }
        for (id, secs) in tinhat_streamer::call::stuck_missing_keys(10) {
            eprintln!(
                "[recv5-e2ee] WARN: {id} stuck in MissingKey ≥{secs}s — their media key is not reaching us; their tiles are black/silent for this user (peers re-announce on the ladder)"
            );
        }
        tick += 1;
        let secs = if tick < 30 { 2 } else { 4 };
        tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
    }
    eprintln!("[recv0-keywatch] key watch ended");
}

/// Headless verification (no GUI): dump the peer keys the ROOM transport currently
/// exposes for a room, so we can confirm they parse into a key under the live peer's
/// identity. Run via `TINHAT_DUMP_ROOMKEYS=<room_id>`.
pub async fn dump_room_keys(room_id: &str) {
    let Some(client) = crate::session::client() else {
        eprintln!("[dumpkeys] not signed in");
        return;
    };
    let Ok(rid) = matrix_sdk::ruma::RoomId::parse(room_id) else {
        eprintln!("[dumpkeys] bad room id");
        return;
    };
    let Some(room) = client.get_room(&rid) else {
        eprintln!("[dumpkeys] room not found in store");
        return;
    };
    match fetch_peer_keys(&room).await {
        Ok(keys) => {
            eprintln!("[dumpkeys] {} key entries from room transport:", keys.len());
            for (k, ts) in &keys {
                eprintln!("[dumpkeys]   identity={} index={} ts={ts}", k.identity, k.index);
            }
        }
        Err(e) => eprintln!("[dumpkeys] fetch failed: {e}"),
    }
}

/// Memories (0.8.7): join the room's call as the recorder and STAY until `stop` flips
/// (marker cleared / call emptied — the service watches that) or the SFU ends us.
/// Same plumbing as [`headless_join`]; the actual capture happens inside the streamer
/// via `set_record_dir` (per-track chains built on TrackSubscribed).
pub async fn vod_join_until(room_id: &str, stop: std::sync::Arc<std::sync::atomic::AtomicBool>) {
    let my_gen = join_gen().fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    match do_join(room_id, false, my_gen).await {
        Ok(mut rx) => {
            eprintln!("[vod] joined {room_id} for recording");
            crate::bridge::runtime().spawn(install_peer_keys_loop(room_id.to_string()));
            // SFU-truth peer tracking: Matrix memberships can ghost for hours after an
            // unclean (or headless) exit, but ParticipantJoined/Left is what's REALLY
            // in the call. Self-leave once the call has been empty for EMPTY_GRACE —
            // counted from the last leave (or from join, if nobody ever shows up).
            const EMPTY_GRACE: std::time::Duration = std::time::Duration::from_secs(90);
            let mut live_peers: u32 = 0;
            let mut empty_since = tokio::time::Instant::now();
            loop {
                if stop.load(std::sync::atomic::Ordering::Relaxed) {
                    break;
                }
                if live_peers == 0 && empty_since.elapsed() >= EMPTY_GRACE {
                    eprintln!("[vod] call empty for {}s — ending recording session", EMPTY_GRACE.as_secs());
                    break;
                }
                match tokio::time::timeout(std::time::Duration::from_millis(500), rx.recv()).await {
                    Ok(Some(CallEvent::ConnectionState { disconnected: true, .. })) => {
                        eprintln!("[vod] SFU disconnected — ending recording session");
                        break;
                    }
                    Ok(Some(CallEvent::ParticipantJoined { .. })) => {
                        live_peers += 1;
                    }
                    Ok(Some(CallEvent::ParticipantLeft { .. })) => {
                        live_peers = live_peers.saturating_sub(1);
                        if live_peers == 0 {
                            empty_since = tokio::time::Instant::now();
                            eprintln!("[vod] last participant left — leaving in {}s unless someone returns", EMPTY_GRACE.as_secs());
                        }
                    }
                    Ok(Some(_)) => {}
                    Ok(None) => break,
                    Err(_) => {} // 500ms tick
                }
            }
            if let Some(call) = current_call().lock().await.take() {
                call.close().await; // close() finalizes the capture chains
            }
            eprintln!("[vod] left {room_id}; recording finalized");
        }
        Err(e) => eprintln!("[vod] join failed for {room_id}: {e}"),
    }
}

/// Headless verification (no GUI/render): join the room's call, install peer keys
/// (room transport), and drain call events for `secs`, then leave. The `[recv4-lkid]`
/// / `[recv5-e2ee]` traces show whether a LIVE peer's media DECRYPTS (state=Ok) — i.e.
/// validates the decrypt path end-to-end without the QML render layer or a GUI click.
/// Run via `TINHAT_AUTOJOIN=<room_id>`.
pub async fn headless_join(room_id: &str, secs: u64) {
    let my_gen = join_gen().fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    match do_join(room_id, false, my_gen).await {
        Ok(mut rx) => {
            eprintln!("[autojoin] joined {room_id}; draining events for {secs}s");
            crate::bridge::runtime().spawn(install_peer_keys_loop(room_id.to_string()));
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(secs);
            while tokio::time::Instant::now() < deadline {
                match tokio::time::timeout(std::time::Duration::from_millis(500), rx.recv()).await {
                    Ok(Some(CallEvent::VideoAdded { id, participant, .. })) => {
                        eprintln!("[autojoin] remote video tile {id} from {participant}")
                    }
                    Ok(Some(CallEvent::VideoRemoved { id })) => eprintln!("[autojoin] video gone {id}"),
                    Ok(Some(CallEvent::ParticipantJoined { identity })) => {
                        eprintln!("[autojoin] participant joined {identity}")
                    }
                    Ok(Some(_)) => {} // participant-left / mute / connection-state — not needed here
                    Ok(None) => break,
                    Err(_) => {} // 500ms tick — keep waiting
                }
            }
            if let Some(call) = current_call().lock().await.take() {
                call.close().await;
            }
            eprintln!("[autojoin] left call");
        }
        Err(e) => eprintln!("[autojoin] join failed: {e}"),
    }
}

/// DIAGNOSTIC ONLY (`TINHAT_DUMP_ROOMKEYS`; nothing installs these any more): read recent
/// `io.element.call.encryption_keys` room events (plaintext, or wrapped in
/// `m.room.encrypted` and decrypted by the SDK) and parse out the media keys they carry,
/// each paired with its event's `origin_server_ts` (the freshness stamp
/// `install_peer_key_at` compares — history re-reads must lose to fresher installs).
async fn fetch_peer_keys(
    room: &matrix_sdk::Room,
) -> Result<Vec<(crate::e2ee_keys::ReceivedKey, u64)>, String> {
    let mut opts = matrix_sdk::room::MessagesOptions::backward();
    opts.limit = matrix_sdk::ruma::UInt::new(100).unwrap_or_else(|| matrix_sdk::ruma::UInt::from(100u16));
    opts.filter.types = Some(vec![
        crate::e2ee_keys::ENCRYPTION_KEYS_EVENT.to_owned(),
        "m.room.encrypted".to_owned(),
    ]);
    let resp = room.messages(opts).await.map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for ev in &resp.chunk {
        let Ok(v) = ev.raw().deserialize_as::<serde_json::Value>() else {
            continue;
        };
        if v.get("type").and_then(|t| t.as_str()) != Some(crate::e2ee_keys::ENCRYPTION_KEYS_EVENT) {
            continue;
        }
        let sender = v.get("sender").and_then(|s| s.as_str()).unwrap_or("");
        let ts_ms = v.get("origin_server_ts").and_then(|t| t.as_u64()).unwrap_or(0);
        if let Some(content) = v.get("content") {
            out.extend(
                crate::e2ee_keys::parse_encryption_keys_any(sender, content)
                    .into_iter()
                    .map(|k| (k, ts_ms)),
            );
        }
    }
    Ok(out)
}

/// Raw uplink measurement — a plain HTTPS POST to the user's server's discard sink
/// (`<api>/speedtest/up`: it counts the bytes and answers `{bytes, ms}`, so the bytes
/// really do cross the wire and are really timed).
///
/// This exists because the LiveKit probe CANNOT measure a link on its own:
/// congestion control only raises its estimate while the sender is actually using
/// its allocation, so a machine that can't encode fast enough reports its own
/// encoder speed as though it were the connection (measured: 5.7 Mbps reported over a
/// path with 33 Gbps of headroom). Measuring the uplink without a codec in the way
/// is what lets us tell the user WHICH side is the limit.
///
/// Starts small and grows while each round is quick, so a slow line is not made to
/// push a huge body: worst case a few seconds, best case a good sample. Returns
/// None if the sink is unreachable — the caller then reports the media number alone
/// rather than inventing a cause.
async fn uplink_probe() -> Option<f64> {
    let base = crate::proxy::base();
    if base.is_empty() {
        eprintln!("[conntest] uplink probe skipped — not signed in to a server");
        return None;
    }
    let sink = format!("{base}/speedtest/up");
    // ONE fixed 8 MB round — deliberately not adaptive. 8 MB takes ~2s on a 30 Mbps
    // uplink (a good sample) and is trivially small for the server; growing the body
    // until the transfer felt "long enough" would have turned our own endpoint into a
    // traffic sink, so the size is capped and the server caps it again at 12 MB.
    const BYTES: usize = 8 * 1024 * 1024;
    let client = matrix_sdk::reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(25))
        .build()
        .ok()?;
    let t0 = std::time::Instant::now();
    let resp = client.post(&sink).body(vec![0x5Au8; BYTES]).send().await;
    let dt = t0.elapsed().as_secs_f64();
    match resp {
        Ok(r) if r.status().is_success() => {
            if dt <= 0.0 {
                return None;
            }
            let mbps = (BYTES as f64 * 8.0) / dt / 1.0e6;
            eprintln!("[conntest] uplink: {BYTES} bytes in {dt:.2}s = {mbps:.1} Mbps");
            Some(mbps)
        }
        // 429 = two other people are already testing. Back off rather than pile on;
        // the caller just reports the media number without naming a limiter.
        Ok(r) if r.status().as_u16() == 429 => {
            eprintln!("[conntest] uplink probe skipped — server busy (429)");
            None
        }
        Ok(r) => {
            eprintln!("[conntest] uplink sink returned HTTP {}", r.status());
            None
        }
        Err(e) => {
            eprintln!("[conntest] uplink probe failed: {e}");
            None
        }
    }
}

#[cfg(test)]
mod recorder_tests {
    use super::*;

    #[test]
    fn the_recorder_is_whoever_the_server_announces() {
        let rec = Some("@recorder:maplehouse.tinhat.me");
        assert!(recorder_identity("@recorder:maplehouse.tinhat.me:DEV1", rec));
        assert!(recorder_identity("@recorder:maplehouse.tinhat.me", rec));
        assert!(!recorder_identity("@ana:maplehouse.tinhat.me:DEV1", rec));
        assert!(!recorder_identity("@recorder:other.test:DEV1", rec));
        assert!(!recorder_identity("recorder", rec));
        // no recorder announced: nobody is one
        assert!(!recorder_identity("@recorder:example.com:ABC", None));
    }
}
