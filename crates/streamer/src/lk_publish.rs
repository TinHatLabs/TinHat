//! E2EE screen-share publish via the LiveKit Rust SDK.
//!
//! This is the real, peer-visible path (the gst `livekitwebrtcsink` path publishes
//! an UNencrypted track, which Element Call flags "no encryption" and peers can't
//! decode in an E2EE room). Here libwebrtc's NVENC factory does the hardware H.264
//! encode AND its FrameCryptor does per-participant AES-GCM E2EE that byte-matches
//! Element Call — keyed by HKDF(our key, salt="LKFrameEncryptionKey").
//!
//! Capture stays in GStreamer: `pipewiresrc` (KDE portal fd) -> I420 -> appsink,
//! and each raw frame is handed to a `NativeVideoSource` which libwebrtc encodes +
//! encrypts. The 16-byte media key is set here AND broadcast over Matrix
//! (`io.element.call.encryption_keys`, see crates/app/src/e2ee_keys.rs) so peers
//! install it and decode our stream.

use anyhow::{Context, Result};
use gstreamer as gst;
use gstreamer::prelude::*;

use livekit::e2ee::key_provider::{KeyDerivationAlgorithm, KeyProvider, KeyProviderOptions};
use livekit::e2ee::{E2eeOptions, EncryptionType};
use livekit::options::{TrackPublishOptions, VideoCodec, VideoEncoderBackend};
use livekit::track::{LocalTrack, LocalVideoTrack, TrackSource};
use livekit::webrtc::video_frame::{I420Buffer, VideoFrame, VideoRotation};
use livekit::webrtc::video_source::native::NativeVideoSource;
use livekit::webrtc::video_source::{RtcVideoSource, VideoResolution};
use livekit::id::ParticipantIdentity;
use livekit::{Room, RoomOptions};

use crate::capture::ScreenCast;

// H.264 (incl. NVENC) maxes at 4096 per dimension; ultrawide/4K screens exceed it,
// which silently produces no encoded media (blank tile). We scale the source DOWN to
// fit the chosen quality box PRESERVING ASPECT (no more squishing an ultrawide into
// 16:9).

// ---- Screen-share quality: a resolution cap (box), fps, and bitrate the user picks.
// The source is fit into the box preserving aspect, so the published frame matches the
// monitor's real shape (an ultrawide stays ultrawide).
/// Which encoder the screen-share publishes with. The two viable choices on this
/// stack (libwebrtc's NVENC factory only does H.264/H.265, and browsers can only
/// *decode* H.264 of those over WebRTC):
///   * `NvencH264`  — hardware H.264 on the GPU's NVENC ASIC. ~zero CPU, ideal while
///     gaming (NVENC is a separate block from the shaders) and great for motion.
///   * `SoftwareVp9` — libvpx VP9 on the CPU. Sharpest for static text/documents;
///     trivially cheap on a 9950X3D2. No GPU use.
/// NOTE: VP9/AV1 have NO hardware encoder anywhere (no GPU has a VP9 encode block),
/// so "VP9 on the GPU" is impossible — that's why selecting VP9 idles the NVENC ASIC.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShareEncoder {
    NvencH264,
    SoftwareVp9,
    /// Pin H.264 even when AV1 is available (compat escape hatch).
    ForceH264,
}
impl ShareEncoder {
    pub fn from_code(c: i32) -> Self {
        match c { 1 => Self::SoftwareVp9, 2 => Self::ForceH264, _ => Self::NvencH264 }
    }
    pub fn code(self) -> i32 {
        match self { Self::NvencH264 => 0, Self::SoftwareVp9 => 1, Self::ForceH264 => 2 }
    }
    fn as_tag(self) -> &'static str {
        match self { Self::NvencH264 => "nvenc", Self::SoftwareVp9 => "vp9", Self::ForceH264 => "h264" }
    }
    fn from_tag(s: &str) -> Option<Self> {
        match s {
            "nvenc" => Some(Self::NvencH264),
            "vp9" => Some(Self::SoftwareVp9),
            "h264" => Some(Self::ForceH264),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ShareQuality {
    pub max_w: u32,
    pub max_h: u32,
    pub fps: u32,
    pub bitrate_kbps: u32,
    pub encoder: ShareEncoder,
}
impl Default for ShareQuality {
    fn default() -> Self {
        // "Game High": 1080p box, 30fps, 12 Mbps over hardware NVENC. Aspect-preserving +
        // ÷16-aligned (an ultrawide publishes ~1920x800). Safe default; higher/lower
        // presets are user-selectable.
        Self { max_w: 1920, max_h: 1080, fps: 30, bitrate_kbps: 12_000, encoder: ShareEncoder::NvencH264 }
    }
}

fn share_quality_path() -> std::path::PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| {
            std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
        });
    base.join("tinhat/share-quality")
}
fn load_share_quality() -> ShareQuality {
    let d = ShareQuality::default();
    let Ok(s) = std::fs::read_to_string(share_quality_path()) else { return d };
    // Format: "W H FPS KBPS [encoder-tag]". The 5th token is optional so files written
    // before the encoder choice existed still load (defaulting to NVENC).
    let toks: Vec<&str> = s.split_whitespace().collect();
    let n: Vec<u32> = toks.iter().take(4).filter_map(|x| x.parse().ok()).collect();
    // `0 0` = Auto (fit connection): the resolution rung is chosen at runtime from the
    // MEASURED granted bitrate (see auto_rung machinery below).
    let auto_ok = n.len() == 4 && n[0] == 0 && n[1] == 0 && n[2] > 0;
    if n.len() == 4 && ((n[0] > 0 && n[1] > 0 && n[2] > 0) || auto_ok) {
        let encoder = toks.get(4).and_then(|t| ShareEncoder::from_tag(t)).unwrap_or(d.encoder);
        ShareQuality { max_w: n[0], max_h: n[1], fps: n[2], bitrate_kbps: n[3], encoder }
    } else {
        d
    }
}
fn save_share_quality(q: &ShareQuality) {
    let p = share_quality_path();
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(
        &p,
        format!("{} {} {} {} {}", q.max_w, q.max_h, q.fps, q.bitrate_kbps, q.encoder.as_tag()),
    );
}
fn quality_cell() -> &'static std::sync::Mutex<ShareQuality> {
    static Q: std::sync::OnceLock<std::sync::Mutex<ShareQuality>> = std::sync::OnceLock::new();
    Q.get_or_init(|| std::sync::Mutex::new(load_share_quality()))
}
/// The current screen-share quality (resolution cap / fps / bitrate).
pub fn share_quality() -> ShareQuality {
    *quality_cell().lock().unwrap()
}
/// Set + persist the resolution/fps/bitrate (takes effect on the next share). The
/// encoder choice is preserved — change it separately via [`set_share_encoder`] so the
/// resolution presets and the encoder toggle don't clobber each other.
pub fn set_share_quality(max_w: u32, max_h: u32, fps: u32, bitrate_kbps: u32) {
    let mut q = share_quality();
    if max_w == 0 || max_h == 0 {
        // Auto (fit connection). Start optimistic at the 1080p rung — the stats
        // governor walks it to whatever the connection actually sustains.
        q.max_w = 0;
        q.max_h = 0;
        set_auto_rung(AUTO_RUNGS[2]);
    } else {
        q.max_w = max_w.clamp(2, 4096);
        q.max_h = max_h.clamp(2, 4096);
    }
    q.fps = fps.clamp(1, 120);
    q.bitrate_kbps = bitrate_kbps.max(500);
    *quality_cell().lock().unwrap() = q;
    save_share_quality(&q);
}

// ── Auto resolution (fit connection) ─────────────────────────────────────────
// Field finding (a 0.3.1 log + a game screenshot): the encoder config was
// perfect, but libwebrtc's bandwidth estimator GRANTED 6.1 Mbps of a requested 12–27
// (the user's uplink ceiling, shared with the camera) — and a fixed 1080p60..4K60 game share
// at 6 Mbps H.264 is wall-to-wall macroblock mush at full smoothness. No encoder
// setting beats that arithmetic; the fix is spending the granted bits on a pixel count
// they can actually feed. `Auto` publishes a rung and lets the publisher stats task
// walk it: absolute downshift when the grant sits below the rung's floor, one-rung
// upgrade probe when the grant SATURATES our own cap (BWE can never report more than
// we ask for, so "pinned at cap" is the only upgrade signal that exists).
/// (w, h, base_kbps at the Great tier, floor_kbps to sustain the rung)
pub const AUTO_RUNGS: &[(u32, u32, u32, u32)] = &[
    (3840, 2160, 24_000, 16_000),
    (2560, 1440, 15_000, 9_000),
    (1920, 1080, 9_000, 4_500),
    (1280, 720, 5_000, 2_200),
    (960, 540, 2_800, 0),
];

fn auto_rung_cell() -> &'static std::sync::Mutex<(u32, u32, u32, u32)> {
    static R: std::sync::OnceLock<std::sync::Mutex<(u32, u32, u32, u32)>> =
        std::sync::OnceLock::new();
    R.get_or_init(|| std::sync::Mutex::new(AUTO_RUNGS[2]))
}
/// Is the share resolution in Auto mode? (persisted quality box = 0x0)
pub fn share_auto_res() -> bool {
    let q = share_quality();
    q.max_w == 0 || q.max_h == 0
}
pub fn auto_rung() -> (u32, u32, u32, u32) {
    *auto_rung_cell().lock().unwrap()
}
pub fn set_auto_rung(r: (u32, u32, u32, u32)) {
    *auto_rung_cell().lock().unwrap() = r;
}
/// The user's bandwidth-tier multiplier (Good/Great/Best pick kbps = 12000 × factor
/// for the Auto entry) — applied to every rung's base so Auto respects the tier.
fn auto_factor() -> f64 {
    (share_quality().bitrate_kbps as f64 / 12_000.0).clamp(0.3, 3.0)
}
/// Auto governor inputs: (current rung, its scaled cap kbps, its scaled floor kbps).
pub fn auto_state() -> ((u32, u32, u32, u32), u32, u32) {
    let r = auto_rung();
    let f = auto_factor();
    (r, (r.2 as f64 * f) as u32, (r.3 as f64 * f) as u32)
}
/// Largest rung whose (scaled) floor the measured grant sustains.
pub fn auto_rung_for_kbps(measured: u32) -> (u32, u32, u32, u32) {
    let f = auto_factor();
    for r in AUTO_RUNGS {
        if measured as f64 >= r.3 as f64 * f {
            return *r;
        }
    }
    *AUTO_RUNGS.last().unwrap()
}
/// One rung sharper than `cur`, if any (upgrade probe).
pub fn auto_rung_above(cur: (u32, u32, u32, u32)) -> Option<(u32, u32, u32, u32)> {
    AUTO_RUNGS.iter().position(|r| *r == cur).and_then(|i| i.checked_sub(1)).map(|i| AUTO_RUNGS[i])
}
/// Set + persist just the encoder (0 = NVENC H.264, 1 = software VP9). Takes effect on
/// the next share.
pub fn set_share_encoder(code: i32) {
    let mut q = share_quality();
    q.encoder = ShareEncoder::from_code(code);
    *quality_cell().lock().unwrap() = q;
    save_share_quality(&q);
}
/// Compact "max_w max_h fps kbps encoder-tag" label for the QML mirror.
pub fn share_quality_label() -> String {
    let q = share_quality();
    format!("{} {} {} {} {}", q.max_w, q.max_h, q.fps, q.bitrate_kbps, q.encoder.as_tag())
}

/// Video encoder backends libwebrtc offers at runtime, as debug strings (e.g.
/// `["Auto", "Software", "Nvenc"]`). `Nvenc` present ⇒ the NVENC factory compiled in
/// AND a CUDA GPU was detected (libcuda dlopen + probe succeeded).
pub fn encoder_backends() -> Vec<String> {
    VideoEncoderBackend::list_available().into_iter().map(|b| format!("{b:?}")).collect()
}
/// Is hardware H.264 (NVENC) available right now?
pub fn nvenc_available() -> bool {
    VideoEncoderBackend::list_available()
        .into_iter()
        .any(|b| matches!(b, VideoEncoderBackend::Nvenc))
}

/// Round down to a multiple of 16 (H.264 macroblock alignment).
fn align16(x: u32) -> u32 {
    (x / 16) * 16
}

/// Fit (sw,sh) into the (max_w,max_h) box preserving aspect, never upscaling, with BOTH
/// dimensions aligned to a multiple of 16. H.264 (incl. NVENC / libwebrtc) needs ÷16
/// dimensions — a non-aligned width (e.g. 3412) silently encodes to a BLACK frame for
/// peers even though local capture looks fine (the bug that broke ultrawide shares).
/// The ≤15px aspect drift from alignment is imperceptible.
pub fn fit_even(sw: u32, sh: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    if sw == 0 || sh == 0 {
        return (align16(max_w).max(16), align16(max_h).max(16));
    }
    let cap_w = max_w.min(4096) as f64;
    let cap_h = max_h.min(4096) as f64;
    let scale = (cap_w / sw as f64).min(cap_h / sh as f64).min(1.0);
    let w = align16((sw as f64 * scale).round() as u32).max(16);
    let h = align16((sh as f64 * scale).round() as u32).max(16);
    (w, h)
}

/// The quality BOX a capture fits the real source into — the preset's, or in Auto the current rung's. The
/// capture builders used to take `q.max_w/max_h` raw, which is 0x0 in Auto: a capture that learned its size from the
/// first frame then fitted it into 0x0 and published 16x16.
pub fn share_box(q: ShareQuality) -> (u32, u32) {
    if q.max_w == 0 || q.max_h == 0 {
        let (rung, _, _) = auto_state();
        (rung.0, rung.1)
    } else {
        (q.max_w, q.max_h)
    }
}

/// Publish (width, height, fps, bitrate_bps) for a captured source of (sw,sh) under the
/// current quality setting. Falls back to the box if the source size is unknown.
pub fn publish_target(src: Option<(u32, u32)>, q: ShareQuality) -> (u32, u32, u32, u64) {
    // Auto: the box + bitrate come from the CURRENT rung (scaled by the user's
    // bandwidth tier), not the persisted 0x0 sentinel.
    if q.max_w == 0 || q.max_h == 0 {
        let (rung, cap_kbps, _) = auto_state();
        let (sw, sh) = src.unwrap_or((rung.0, rung.1));
        let (w, h) = fit_even(sw, sh, rung.0, rung.1);
        return (w, h, q.fps, (cap_kbps as u64) * 1000);
    }
    let (sw, sh) = src.unwrap_or((q.max_w, q.max_h));
    let (w, h) = fit_even(sw, sh, q.max_w, q.max_h);
    (w, h, q.fps, (q.bitrate_kbps as u64) * 1000)
}

/// Resolve an encoder preference against the backends libwebrtc actually offers —
/// the NVENC → VAAPI → software-VP9 ladder. ONE implementation, used by BOTH the real
/// share publish (call.rs) and the connection probe below.
///
/// The ladder exists because pinning H.264 on a machine without a hardware encoder is
/// not a graceful degrade: libwebrtc falls back to software OpenH264, which is
/// frequently not compiled in → the track negotiates H.264 but emits no frames.
/// On a share that is a black tile; on the probe it was worse — the grant crawled and
/// the tiny number was blamed on "this computer's encoding" (the 0.8.x bandwidth misverdict).
pub fn resolve_encoder_ladder(
    pref: ShareEncoder,
    allow_av1: bool,
) -> (livekit::options::VideoCodec, livekit::options::VideoEncoderBackend, &'static str) {
    use livekit::options::{VideoCodec, VideoEncoderBackend};
    let h264_ladder = || {
        let backends: Vec<_> = VideoEncoderBackend::list_available().into_iter().collect();
        if backends.iter().any(|b| matches!(b, VideoEncoderBackend::Nvenc)) {
            (VideoCodec::H264, VideoEncoderBackend::Nvenc, "H.264/NVENC (GPU)")
        } else if backends.iter().any(|b| matches!(b, VideoEncoderBackend::Vaapi)) {
            eprintln!("[encoder] NVENC unavailable -> falling back to VAAPI");
            (VideoCodec::H264, VideoEncoderBackend::Vaapi, "H.264/VAAPI (GPU)")
        } else {
            eprintln!("[encoder] no hardware H.264 available -> falling back to software VP9");
            (VideoCodec::VP9, VideoEncoderBackend::Software, "VP9 (software fallback)")
        }
    };
    match pref {
        ShareEncoder::SoftwareVp9 => (VideoCodec::VP9, VideoEncoderBackend::Software, "VP9 (software)"),
        ShareEncoder::ForceH264 => h264_ladder(),
        ShareEncoder::NvencH264 => {
            // A deliberate choice: AV1 is the standard where the GPU encodes
            // it — ~2x quality per megabit and no 4096-wide H.264 wall. Verified
            // available BEFORE pinning; a wrong pin is a black tile (the OpenH264
            // lesson). Everything decodes AV1: hardware on ~2021+ devices, dav1d
            // software as the floor in every client we ship.
            // AV1 + E2EE (the black tile) — SOLVED 2026-08-12. Three stacked bugs:
            // (1) the frame cryptor encrypted the WHOLE AV1 payload and the
            // packetizer rewrote "OBU headers" inside the ciphertext (fixed in
            // our patched libwebrtc, tinhat-av1-e2ee-v2: ciphertext rides inside
            // one synthetic OBU_FRAME, keyframes lead with an empty synthetic
            // sequence header so the packetizer sets the N bit); (2) NvEncoder
            // IVF-wrapped every AV1 frame, so NVENC-AV1 over RTP had never
            // delivered a decodable frame even unencrypted (bUseIVFContainer
            // now false); (3) `allow_av1` here blocked E2EE sessions — that
            // gate is gone, the caller passes true. `allow_av1` remains as an
            // emergency escape hatch only. Gate before any webrtc-sys bump:
            // TINHAT_E2EE_CODEC=av1 loopback PASS + negative control MissingKey.
            // 0.8.20: say WHY when AV1 is not taken — a share that silently came out
            // H.264 on the same machine that did AV1 two days earlier had no
            // line in the log to explain it.
            let av1_hw = VideoEncoderBackend::nvenc_av1_available();
            if allow_av1 && av1_hw {
                (VideoCodec::AV1, VideoEncoderBackend::Nvenc, "AV1/NVENC (GPU)")
            } else {
                eprintln!(
                    "[encoder] AV1 preferred but not taken: allow_av1={allow_av1} nvenc_av1_available={av1_hw} -> H.264 ladder"
                );
                h264_ladder()
            }
        }
    }
}

// ── Throttle opener ─────────────────────────────────────────────────────────
/// Walk the sender's `max_bitrate` allocation up to `target_bps` in doubling
/// steps. Each CHANGE of the allocation makes libwebrtc's probe controller fire
/// an allocation probe, so the bandwidth estimate roughly doubles per step on a
/// path that can carry it — reaching tens of Mbps in seconds instead of the
/// additive ~8-10%/s crawl that had every share stuck near 6 Mbps for minutes.
///
/// Why this and not a start-bitrate seed: all of SetBitrate(start), local-SDP
/// and remote-SDP `x-google-start-bitrate` munging were tried and MEASURED to
/// do nothing on an already-connected transport (the estimate is clamped to the
/// existing delay-based limit). Allocation probes are the one mechanism GCC
/// honors mid-call — and the allocation set AT publish is itself dropped,
/// because it races the initial probe sequence (state != kProbingComplete), so
/// somebody has to change it afterwards. That somebody is this task.
///
/// Fortitude: a probe is a short padding burst; if the path can't carry a step,
/// the probe simply fails to raise the estimate and the delay-based truth
/// stands — weak links converge to the same place they do today, just without
/// the ceiling on strong ones. The walk always ENDS at `target_bps`, so the
/// steady-state allocation equals what the caller asked to publish.
pub fn spawn_throttle_opener(track: livekit::track::LocalVideoTrack, target_bps: u64) {
    tokio::spawn(async move {
        // Let the publish negotiation settle and the initial exponential probes
        // finish (their race is why the publish-time allocation was dropped).
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        let Some(sender) = track.rtp_sender() else {
            eprintln!("[throttle] no RTP sender on the track — skipping");
            return;
        };
        // Doubling ladder ending exactly at target: target/16, /8, /4, /2, target
        // (floored at 4 Mbps so the first probes are worth firing).
        let mut steps: Vec<u64> = (0..4).rev().map(|s| target_bps >> (s + 1)).collect();
        steps.push(target_bps);
        let steps: Vec<u64> = steps.into_iter().filter(|b| *b >= 4_000_000).collect();
        for step in steps {
            let mut params = sender.parameters();
            if params.encodings.is_empty() {
                eprintln!("[throttle] sender has no encodings yet — stopping");
                return;
            }
            for enc in params.encodings.iter_mut() {
                enc.max_bitrate = Some(step);
            }
            match sender.set_parameters(params) {
                Ok(()) => eprintln!("[throttle] allocation -> {} kbps", step / 1000),
                Err(e) => {
                    eprintln!("[throttle] set_parameters failed: {e} — stopping");
                    return;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(900)).await;
        }
    });
}

// ── Connection throughput probe ("open the throttle") ───────────────────────
// Joins a SCRATCH LiveKit room and publishes 20s of maximally-incompressible
// snow at PROBE_CAP_BPS, then reports what the path actually granted. Measures
// the REAL media path (UDP vs TURN relaying included) — the number that
// predicts share quality, unlike a plain TCP speedtest.

/// The probe's own bitrate ceiling. It exists only to bound the test's traffic —
/// the scratch room has no subscribers. It MUST comfortably exceed the highest real
/// share preset (Source × Unleashed ≈ 45 Mbps, Auto-4K × Unleashed ≈ 60 Mbps),
/// because a result pinned at this cap says NOTHING about the encoder or the link —
/// the verdict has to treat cap-pinned as "both have headroom", not "encoder-limited"
/// (0.8.x: a 10G-LAN user was told their 9950X3D "limits" them at the old 50M cap).
pub const PROBE_CAP_BPS: u64 = 100_000_000;

#[derive(Debug, Clone)]
pub struct ProbeReport {
    /// Highest single-sample grant (libwebrtc target_bitrate), kbps.
    pub peak_grant_kbps: u64,
    /// Mean grant over the last 5 samples (the sustained figure), kbps.
    pub tail_grant_kbps: u64,
    /// Actual bytes-on-wire send rate at the tail, kbps.
    pub send_kbps: u64,
    /// The estimator's own available_outgoing_bitrate at the tail, kbps (0 = n/a).
    pub bwe_kbps: u64,
    /// "direct UDP" | "RELAYED (TURN/<proto>)" | "unknown" — relayed paths choke
    /// bandwidth estimation regardless of line speeds; this is the tunnel verdict.
    pub path: String,
    /// Which encoder the probe actually measured (the same ladder a real share
    /// resolves), e.g. "H.264/NVENC (GPU)" or "VP9 (software fallback)". Without
    /// this in the result, an encoding verdict is unfalsifiable for the reader.
    pub encoder_label: &'static str,
    /// True if the grant was still climbing when the probe hit its time wall —
    /// the path outlasted the test, so `peak_grant_kbps` is a floor, not a
    /// ceiling, and the verdict must not name a limiter.
    pub still_rising: bool,
}

pub async fn connection_probe(url: &str, token: &str, secs: u32) -> Result<ProbeReport> {
    use livekit::options::{TrackPublishOptions, VideoEncoding};
    use livekit::track::{LocalTrack, LocalVideoTrack, TrackSource};
    use livekit::webrtc::stats::RtcStats;
    use livekit::webrtc::video_source::native::NativeVideoSource;
    use livekit::webrtc::video_source::{RtcVideoSource, VideoResolution};

    crate::ensure_init()?;
    // Probe with the encoder a real share would resolve to under the user's CURRENT
    // preference — the point is to predict share quality, so measure the same path.
    let (codec, backend, encoder_label) =
        resolve_encoder_ladder(share_quality().encoder, true); // probe room is unencrypted
    eprintln!("[conntest] probing via {encoder_label}");
    // The seed must be in place BEFORE Room::connect. The estimator
    // initializes with the transport (the data-channel negotiation), and that
    // is the ONLY moment this libwebrtc build honors a start rate — seeding at
    // publish time was measured to be clamped to the already-running estimate.
    let seed_kbps: u32 = std::env::var("LK_PROBE_SEED_KBPS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(15_000);
    std::env::set_var("LK_BWE_START_KBPS", seed_kbps.to_string());
    eprintln!("[conntest] seeding BWE at {seed_kbps} kbps (pre-connect)");
    let (room, _events) = Room::connect(url, token, RoomOptions::default())
        .await
        .map_err(|e| anyhow::anyhow!("SFU connect failed: {e}"))?;
    let source = NativeVideoSource::new(VideoResolution { width: 1920, height: 1080 }, true);
    let track =
        LocalVideoTrack::create_video_track("conntest", RtcVideoSource::Native(source.clone()));
    let stats_track = track.clone();
    let publish_result = room
        .local_participant()
        .publish_track(
            LocalTrack::Video(track),
            TrackPublishOptions {
                source: TrackSource::Screenshare,
                video_codec: codec,
                video_encoder: backend,
                video_encoding: Some(VideoEncoding {
                    max_bitrate: PROBE_CAP_BPS,
                    max_framerate: 60.0,
                }),
                simulcast: false,
                ..Default::default()
            },
        )
        .await;
    // Clear on a DELAY, not immediately: the livekit engine debounces publisher
    // renegotiation, so the SetLocalDescription that reads the var fires AFTER
    // publish_track resolves — an immediate remove_var raced it and the seed never
    // applied (verified: t=1 grant was the unseeded ~4.8M with an immediate clear).
    // 3s cannot mis-seed anything: within 3s of share start the estimate ≈ seed.
    tokio::spawn(async {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        std::env::remove_var("LK_BWE_START_KBPS");
    });
    let publication = publish_result.map_err(|e| anyhow::anyhow!("probe publish failed: {e}"))?;
    // Open the throttle: allocation-probe the estimator up to the probe cap so
    // the plateau arrives in seconds, not at the 45s wall.
    spawn_throttle_opener(stats_track.clone(), PROBE_CAP_BPS);

    // Snow is incompressible, so the encoder genuinely produces up to the cap and
    // the bandwidth estimator has a real load to probe against.
    let desc = "videotestsrc is-live=true pattern=snow ! \
                video/x-raw,format=I420,width=1920,height=1080,framerate=60/1 ! \
                queue ! appsink name=probe max-buffers=4 drop=true sync=false";
    let pipeline = gst::parse::launch(desc)?
        .downcast::<gst::Pipeline>()
        .map_err(|_| anyhow::anyhow!("probe: not a Pipeline"))?;
    let appsink = pipeline
        .by_name("probe")
        .context("probe appsink missing")?
        .downcast::<gstreamer_app::AppSink>()
        .map_err(|_| anyhow::anyhow!("probe sink not an AppSink"))?;
    wire_appsink_to_source(&appsink, source);
    pipeline
        .set_state(gst::State::Playing)
        .context("probe pipeline start failed")?;

    let mut peak = 0u64;
    let mut tail: Vec<u64> = Vec::new();
    let mut send_kbps = 0u64;
    let mut bwe_kbps = 0u64;
    let mut path = "unknown".to_string();
    let mut prev_bytes: Option<(u64, i64)> = None;
    // ── Run-to-plateau (second finding) ────────────────────────────────────
    // libwebrtc's congestion controller RAMPS at roughly 8-10%/s once the initial
    // probe settles (measured on a LAN with hundreds of Mbps free: 14 Mbps after
    // 15s, send == grant every second — the encoder was filling every allocation
    // and NOTHING had plateaued). A fixed-length probe therefore measures
    // ramp-rate × duration, not the path OR the encoder, and the old verdict
    // pinned that artifact on "this computer's encoding".
    //
    // `secs` becomes the MINIMUM. After it, keep going while the grant is still
    // climbing (>4% over the trailing 5s) up to a hard wall. Slow links plateau in
    // seconds — they now finish EARLIER than the old fixed 20s. If we hit the wall
    // still climbing, `still_rising` tells the verdict to say "at least this,
    // the test ran out before the path did" instead of naming a limiter.
    let max_secs = secs.max(45);
    let mut grants: Vec<u64> = Vec::new();
    let mut still_rising = false;
    let mut i = 0u32;
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        let stats = match livekit::track::Track::LocalVideo(stats_track.clone()).get_stats().await {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[conntest] get_stats failed mid-probe: {e}");
                break;
            }
        };
        let mut grant = 0u64;
        let mut nominated_local: Option<String> = None;
        let mut locals: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        for s in &stats {
            match s {
                RtcStats::OutboundRtp(o) => {
                    grant = (o.outbound.target_bitrate / 1000.0) as u64;
                    let bytes = o.sent.bytes_sent;
                    let ts = o.rtc.timestamp;
                    if let Some((pb, pt)) = prev_bytes {
                        let dt = ((ts - pt) as f64 / 1_000_000.0).max(0.001);
                        send_kbps = ((bytes.saturating_sub(pb)) as f64 * 8.0 / dt / 1000.0) as u64;
                    }
                    prev_bytes = Some((bytes, ts));
                }
                RtcStats::CandidatePair(p) => {
                    if p.candidate_pair.nominated {
                        nominated_local = Some(p.candidate_pair.local_candidate_id.clone());
                        if p.candidate_pair.available_outgoing_bitrate > 0.0 {
                            bwe_kbps = (p.candidate_pair.available_outgoing_bitrate / 1000.0) as u64;
                        }
                    }
                }
                RtcStats::LocalCandidate(c) => {
                    use livekit::webrtc::stats::IceCandidateType;
                    let label = match c.local_candidate.candidate_type {
                        Some(IceCandidateType::Relay) => format!(
                            "RELAYED (TURN{})",
                            c.local_candidate
                                .relay_protocol
                                .map(|p| format!("/{p:?}"))
                                .unwrap_or_default()
                        ),
                        Some(_) => "direct UDP".to_string(),
                        None => "unknown".to_string(),
                    };
                    locals.insert(c.rtc.id.clone(), label);
                }
                _ => {}
            }
        }
        if let Some(id) = nominated_local {
            if let Some(l) = locals.get(&id) {
                path = l.clone();
            }
        }
        peak = peak.max(grant);
        grants.push(grant);
        eprintln!(
            "[conntest] t={}s grant={grant}kbps send={send_kbps}kbps bwe={bwe_kbps}kbps path={path}",
            i + 1
        );
        i += 1;
        if i >= secs {
            // Plateau check over the trailing 5 samples: still climbing >4%?
            let n = grants.len();
            let rising = n >= 5 && {
                let then = grants[n - 5].max(1);
                (grants[n - 1] as f64 / then as f64) > 1.04
            };
            if !rising {
                eprintln!("[conntest] grant plateaued after {i}s — stopping");
                break;
            }
            if i >= max_secs {
                still_rising = true;
                eprintln!(
                    "[conntest] {max_secs}s wall hit with the grant STILL climbing — \
                     the path outlasted the test"
                );
                break;
            }
        }
    }
    // Tail = the last 5 samples, wherever the loop ended.
    tail.extend(grants.iter().rev().take(5).rev());
    let _ = pipeline.set_state(gst::State::Null);
    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        room.local_participant().unpublish_track(&publication.sid()),
    )
    .await;
    room.close().await.ok();
    let tail_grant_kbps = if tail.is_empty() { 0 } else { tail.iter().sum::<u64>() / tail.len() as u64 };
    Ok(ProbeReport { peak_grant_kbps: peak, tail_grant_kbps, send_kbps, bwe_kbps, path, encoder_label, still_rising })
}

/// A live E2EE publish — keep it alive for the whole share; `close()` tears it down.
pub struct E2eePublish {
    room: Room,
    pipeline: gst::Pipeline,
    screencast: Option<ScreenCast>,
    // Stop flag for the pipeline's bus-watch thread (NULL posts no EOS — see watch_pipeline_bus).
    bus_stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl E2eePublish {
    /// Stop the pipeline, leave the LiveKit room, and close the portal session.
    pub async fn close(mut self) {
        self.bus_stop.store(true, std::sync::atomic::Ordering::Relaxed); // let the bus thread exit
        let _ = self.pipeline.set_state(gst::State::Null);
        self.room.close().await.ok();
        if let Some(sc) = self.screencast.take() {
            sc.close().await;
        }
    }
}

/// Build the E2EE KeyProvider that matches Element Call's deployed config: per
/// participant (not shared), HKDF derivation, salt "LKFrameEncryptionKey".
pub(crate) fn matrix_key_provider() -> KeyProvider {
    let mut opts = KeyProviderOptions::default(); // salt already "LKFrameEncryptionKey"
    opts.key_derivation_algorithm = KeyDerivationAlgorithm::HKDF; // EC uses HKDF (livekit default is PBKDF2)
    // element-call's index space is % 256, but libwebrtc's ParticipantKeyHandler clamps
    // the ring to MAX_KEYRING_SIZE=255 slots while decryptFrame bounds-checks against the
    // UNCLAMPED option — key_ring_size=256 let a frame with trailer index 255 read one
    // past the ring (OOB/abort). 255 turns that frame into a clean kMissingKey drop
    // instead; install_peer_key_at refuses index 255 with a log for the same reason.
    opts.key_ring_size = 255;
    KeyProvider::new(opts)
}

/// Connect to the SFU with E2EE, set our media key, and publish a hardware-encoded
/// (NVENC) H.264 screen-share track fed from the KDE portal capture.
///
/// * `ws_url`/`jwt` — from lk-jwt-service `/sfu/get`.
/// * `identity` — our LiveKit identity = `userId:deviceId` (what the JWT encodes and
///   what EC keys the media key by); the FrameCryptor encrypts our track with the
///   key set for this identity.
/// * `key` — the raw 16 bytes we ALSO broadcast over Matrix (same bytes + index).
/// * `key_index` — the keyring slot; advances per session so a re-share can never
///   collide with a previous key in peers' (identity,index) tracking.
pub async fn start_e2ee_screenshare(
    ws_url: &str,
    jwt: &str,
    identity: &str,
    key: [u8; 16],
    key_index: u8,
    screencast: ScreenCast,
) -> Result<E2eePublish> {
    crate::ensure_init()?; // gst init (NVENC ranking irrelevant here; libwebrtc encodes)

    let key_provider = matrix_key_provider();
    // Encrypt our outgoing track with `key` at `key_index` under our own identity.
    key_provider.set_key(&ParticipantIdentity(identity.to_owned()), key_index as i32, key.to_vec());

    // RoomOptions is #[non_exhaustive] — build via Default then set the field.
    let mut options = RoomOptions::default();
    options.encryption = Some(E2eeOptions {
        encryption_type: EncryptionType::Gcm,
        key_provider,
    });

    eprintln!("[lk] connecting…");
    let (room, _events) = Room::connect(ws_url, jwt, options)
        .await
        .context("LiveKit connect failed")?;
    eprintln!("[lk] connected");
    // Sender cryptors are created at index 0 — pin them to the announced index.
    room.e2ee_manager()
        .set_sender_key_index(&room.local_participant().identity(), key_index as i32);

    // Drain room events so the channel doesn't grow unbounded (publish-only).
    {
        let mut rx = _events;
        tokio::spawn(async move { while rx.recv().await.is_some() {} });
    }

    // Publish at the source's REAL aspect ratio (fit into the quality box), so an
    // ultrawide stays ultrawide instead of being squished into 16:9.
    let q = share_quality();
    let (tw, th, fps, bitrate_bps) = publish_target(screencast.size(), q);
    eprintln!(
        "[lk] screen-share source {:?} -> publishing {tw}x{th}@{fps} ~{}kbps",
        screencast.size(),
        bitrate_bps / 1000
    );
    let source = NativeVideoSource::new(VideoResolution { width: tw, height: th }, true);

    // Publish FIRST (track negotiates from its config, not frames), THEN start the
    // capture. Starting gst capture before publish runs the gst streaming thread
    // (capture_frame) concurrently with publish negotiation, which deadlocked.
    let track = LocalVideoTrack::create_video_track("screen", RtcVideoSource::Native(source.clone()));
    eprintln!("[lk] publishing track…");
    // Same estimator seed as the in-call share path (see call.rs
    // publish_screenshare) — without it this publish crawls from ~zero too.
    let seed_kbps = (bitrate_bps / 1000).min(15_000);
    std::env::set_var("LK_BWE_START_KBPS", seed_kbps.to_string());
    let publish_result = room
        .local_participant()
        .publish_track(
            LocalTrack::Video(track),
            TrackPublishOptions {
                video_codec: VideoCodec::H264,
                source: TrackSource::Screenshare,
                video_encoding: Some(livekit::options::VideoEncoding {
                    max_bitrate: bitrate_bps,
                    max_framerate: fps as f64,
                }),
                simulcast: false, // single sharp layer for screen text
                video_encoder: VideoEncoderBackend::Nvenc, // hardware H.264 (avoid blocky software OpenH264)
                ..Default::default()
            },
        )
        .await;
    // Delayed clear — see connection_probe for why immediate removal races the
    // engine's debounced renegotiation and silently skips the seed.
    tokio::spawn(async {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        std::env::remove_var("LK_BWE_START_KBPS");
    });
    publish_result.context("publish_track failed")?;
    eprintln!("[lk] track published; starting capture");

    // GStreamer capture: portal fd -> raw I420 -> appsink (feeds the source).
    let pipeline = build_capture_pipeline(&screencast, tw, th, fps)?;
    let appsink = pipeline
        .by_name("sink")
        .context("appsink not found")?
        .downcast::<gstreamer_app::AppSink>()
        .map_err(|_| anyhow::anyhow!("sink is not an AppSink"))?;
    wire_appsink_to_source(&appsink, source);

    pipeline
        .set_state(gst::State::Playing)
        .context("failed to start capture pipeline")?;
    eprintln!("[lk] capture pipeline PLAYING");
    let bus_stop = watch_pipeline_bus(&pipeline); // surface gst errors (no longer silent)

    Ok(E2eePublish {
        room,
        pipeline,
        screencast: Some(screencast),
        bus_stop,
    })
}

/// Self-test (no portal, no Matrix): connect with E2EE + publish synthetic frames,
/// to check whether LiveKit connect/publish work inside the app binary's
/// environment (libwebrtc linked alongside Qt/matrix-sdk/gst). Run via the app's
/// `TINHAT_LKTEST` path. Mirrors the standalone spike.
pub async fn selftest(ws_url: &str, jwt: &str, identity: &str, secs: u64) -> Result<()> {
    let key_provider = matrix_key_provider();
    key_provider.set_key(&ParticipantIdentity(identity.to_owned()), 0, vec![0u8; 16]);
    let mut options = RoomOptions::default();
    options.encryption = Some(E2eeOptions {
        encryption_type: EncryptionType::Gcm,
        key_provider,
    });

    eprintln!("[selftest] connecting…");
    let (room, mut events) = Room::connect(ws_url, jwt, options)
        .await
        .context("connect failed")?;
    eprintln!("[selftest] connected as {}", room.local_participant().identity().0);
    tokio::spawn(async move { while events.recv().await.is_some() {} });

    let source = NativeVideoSource::new(VideoResolution { width: 1280, height: 720 }, true);
    let track = LocalVideoTrack::create_video_track("screen", RtcVideoSource::Native(source.clone()));
    eprintln!("[selftest] publishing…");
    room.local_participant()
        .publish_track(
            LocalTrack::Video(track),
            TrackPublishOptions {
                video_codec: VideoCodec::H264,
                source: TrackSource::Screenshare,
                ..Default::default()
            },
        )
        .await
        .context("publish failed")?;
    eprintln!("[selftest] published; feeding {secs}s of synthetic frames");

    let mut tick: u64 = 0;
    let frames = secs * 30;
    let mut interval = tokio::time::interval(std::time::Duration::from_millis(33));
    while tick < frames {
        interval.tick().await;
        let mut buf = I420Buffer::new(1280, 720);
        let (y, u, v) = buf.data_mut();
        let s = (tick % 255) as u8;
        y.iter_mut().enumerate().for_each(|(i, p)| *p = (i as u8) ^ s);
        u.iter_mut().for_each(|p| *p = 128);
        v.iter_mut().for_each(|p| *p = 128);
        source.capture_frame(&VideoFrame {
            rotation: VideoRotation::VideoRotation0,
            timestamp_us: (tick as i64) * 33_333,
            frame_metadata: None,
            buffer: buf,
        });
        tick += 1;
    }
    eprintln!("[selftest] done; closing");
    room.close().await.ok();
    Ok(())
}

/// `max_w`/`max_h` are the quality BOX (not exact output dims). The published frame size is
/// derived from the REAL captured source size — read from the negotiated caps via a one-shot
/// pad probe and `fit_even()`d into the box — because KDE's portal reports NO size for a
/// WINDOW share (`stream.size()==None`). Forcing a fixed box-shaped output instead letterboxed
/// the window into a 16:9 frame, which the peer cropped to look horizontally squished.
/// Are the NVIDIA CUDA scaling elements present? (Cached.) The capture's convert+downscale of
/// a 4K/5K desktop is the single most expensive thing the app does while sharing — it is a
/// full-resolution GPU->CPU download plus a CPU lanczos resize on EVERY frame, and on a big
/// source it cannot keep up (measured on the dev laptop: the CPU path delivered 17.5fps and
/// burned 34% of a core; the CUDA path delivered 42.3fps for 24%). Doing it on the GPU also
/// means only the FINAL, small frame crosses the bus.
pub(crate) fn cuda_scale_available() -> bool {
    static OK: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OK.get_or_init(|| {
        ["cudaupload", "cudaconvertscale", "cudadownload"]
            .iter()
            .all(|n| gst::ElementFactory::find(n).is_some())
    })
}

/// GPU capture path: scale + convert on the NVIDIA GPU, download only the finished frame.
/// Requires a KNOWN output size (so no mid-flight caps re-pin is needed) — callers use it only
/// when the portal reported the source size (monitor shares, i.e. the expensive case). Falls
/// back to [`build_capture_pipeline`] if it can't reach PLAYING (e.g. the compositor's DMABuf
/// can't be imported into CUDA on a hybrid/AMD box).
pub(crate) fn build_capture_pipeline_cuda(
    screencast: &ScreenCast,
    out_w: u32,
    out_h: u32,
    fps: u32,
    paced: bool,
) -> Result<gst::Pipeline> {
    // Windows: optionally pin the capture rate on the source (see the d3d11 builder).
    // Linux portal sources negotiate a variable rate; videorate handles pacing there.
    #[cfg(windows)]
    let src = if paced { screencast.src_fragment_paced(fps) } else { screencast.src_fragment() };
    #[cfg(not(windows))]
    let src = {
        let _ = paced;
        screencast.src_fragment()
    };
    let desc = format!(
        "{src} ! \
         videorate drop-only=true ! video/x-raw(ANY),framerate={fps}/1 ! \
         cudaupload ! cudaconvertscale ! \
         video/x-raw(memory:CUDAMemory),format=I420,width={out_w},height={out_h},pixel-aspect-ratio=1/1 ! \
         cudadownload ! \
         queue max-size-buffers=4 leaky=downstream ! \
         appsink name=sink emit-signals=false max-buffers=2 drop=true sync=false"
    );
    gst::parse::launch(&desc)?
        .downcast::<gst::Pipeline>()
        .map_err(|_| anyhow::anyhow!("cuda capture: parsed element is not a Pipeline"))
}

/// The GPU (CUDA) capture for a source whose size nobody told us (KDE's portal gives none for a window share,
/// and often none for a monitor; the stream probe failed on every share in the field logs). Same chain as
/// [`build_capture_pipeline_cuda`], but the output size is pinned from the REAL size on the first frame — the one-shot
/// re-pin the CPU path has always used ([`install_first_frame_repin`]) — instead of at build time. Until 10.8 such a
/// share fell back to the CPU path: a full-resolution DMABuf read-back + convert + lanczos per frame on one core, which
/// published 11–22 fps for a 3–5K source whatever the settings. The caller commits to it only once a
/// frame has come through (call.rs start_share_capture), so a box whose DMABuf will not import into CUDA still falls
/// back to the CPU path.
pub(crate) fn build_capture_pipeline_cuda_learn(
    screencast: &ScreenCast,
    box_w: u32,
    box_h: u32,
    fps: u32,
) -> Result<gst::Pipeline> {
    let desc = cuda_learn_desc(&screencast.src_fragment(), fps);
    let pipeline = gst::parse::launch(&desc)?
        .downcast::<gst::Pipeline>()
        .map_err(|_| anyhow::anyhow!("cuda capture: parsed element is not a Pipeline"))?;
    install_first_frame_repin(&pipeline, box_w, box_h, Some("memory:CUDAMemory"), screencast.size_memo(), "GPU");
    Ok(pipeline)
}

/// The launch line of [`build_capture_pipeline_cuda_learn`] for a source head (tests feed a videotestsrc).
pub(crate) fn cuda_learn_desc(src: &str, fps: u32) -> String {
    format!(
        "{src} ! \
         videorate drop-only=true ! video/x-raw(ANY),framerate={fps}/1 ! \
         cudaupload ! cudaconvertscale name=vscale ! \
         capsfilter name=outcaps caps=video/x-raw(memory:CUDAMemory),format=I420 ! \
         cudadownload ! \
         queue max-size-buffers=4 leaky=downstream ! \
         appsink name=sink emit-signals=false max-buffers=2 drop=true sync=false"
    )
}

/// The one-shot output re-pin of the capture builders: on the FIRST buffer at the scaler's sink pad, read the real
/// source size off the negotiated caps, pin `outcaps` to fit_even(real, box) (I420, square pixels, in `feature` memory
/// when given) and remember the size in `memo` (so a re-publish of the same cast knows it). Removes itself.
pub(crate) fn install_first_frame_repin(
    pipeline: &gst::Pipeline,
    box_w: u32,
    box_h: u32,
    feature: Option<&'static str>,
    memo: std::sync::Arc<std::sync::Mutex<Option<(u32, u32)>>>,
    path: &'static str,
) {
    let (Some(outcaps), Some(vscale)) = (pipeline.by_name("outcaps"), pipeline.by_name("vscale")) else { return };
    let Some(sinkpad) = vscale.static_pad("sink") else { return };
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    sinkpad.add_probe(gst::PadProbeType::BUFFER, move |pad, _info| {
        if done.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return gst::PadProbeReturn::Remove;
        }
        if let Some(caps) = pad.current_caps() {
            if let Some(s) = caps.structure(0) {
                if let (Ok(rw), Ok(rh)) = (s.get::<i32>("width"), s.get::<i32>("height")) {
                    let (rw, rh) = (rw.max(0) as u32, rh.max(0) as u32);
                    let (ow, oh) = fit_even(rw, rh, box_w, box_h);
                    let mut nc = gst::Caps::builder("video/x-raw")
                        .field("format", "I420")
                        .field("width", ow as i32)
                        .field("height", oh as i32)
                        .field("pixel-aspect-ratio", gst::Fraction::new(1, 1))
                        .build();
                    if let (Some(f), Some(c)) = (feature, nc.get_mut()) {
                        c.set_features(0, Some(gst::CapsFeatures::new([f])));
                    }
                    outcaps.set_property("caps", &nc);
                    if rw > 0 && rh > 0 {
                        *memo.lock().unwrap() = Some((rw, rh));
                    }
                    eprintln!("[lk] capture real {rw}x{rh} -> publishing {ow}x{oh} (aspect-correct, {path})");
                }
            }
        }
        gst::PadProbeReturn::Remove
    });
}

/// Windows GPU capture path: convert + downscale on the D3D11 device the capture
/// textures already live on, then download only the FINISHED small frame. This is the
/// "near-zero game impact" capture: the full-res 5K readback + CPU lanczos of the CPU
/// path is replaced by an on-GPU scale, so the one GPU->CPU copy shrinks from ~42 MB
/// (5120x2160 I420) to ~2.2 MB (1920x800) per frame and no CPU resize runs at all.
/// Scaling quality is the D3D11 sampler (bilinear-ish) — that's why the VP9 "sharp
/// text" preset keeps the CPU lanczos path (see start_share_capture's policy).
/// `add-borders=false` mirrors the CPU path: out dims already carry the source aspect
/// via fit_even, the ≤15px alignment drift is stretched, not letterboxed.
/// `nv12_download` = download NV12 and convert to I420 on the CPU (small frame, cheap
/// chroma reshuffle) — fallback for d3d11convert builds that can't emit planar I420 in
/// D3D11 memory.
#[cfg(windows)]
pub(crate) fn build_capture_pipeline_d3d11(
    screencast: &ScreenCast,
    out_w: u32,
    out_h: u32,
    fps: u32,
    nv12_download: bool,
    paced: bool,
) -> Result<gst::Pipeline> {
    // paced=true pins framerate on the SOURCE caps (capture element runs at {fps});
    // videorate stays as a passthrough belt. Preferred cascade candidate — a setup
    // that can't negotiate it falls back to the unpaced variant automatically.
    let src = if paced { screencast.src_fragment_paced(fps) } else { screencast.src_fragment() };
    let tail = if nv12_download {
        format!(
            "video/x-raw(memory:D3D11Memory),format=NV12,width={out_w},height={out_h},pixel-aspect-ratio=1/1 ! \
             d3d11download ! videoconvert ! video/x-raw,format=I420"
        )
    } else {
        format!(
            "video/x-raw(memory:D3D11Memory),format=I420,width={out_w},height={out_h},pixel-aspect-ratio=1/1 ! \
             d3d11download"
        )
    };
    let desc = format!(
        "{src} ! \
         videorate drop-only=true ! video/x-raw(ANY),framerate={fps}/1 ! \
         d3d11convert add-borders=false ! \
         {tail} ! \
         queue max-size-buffers=4 leaky=downstream ! \
         appsink name=sink emit-signals=false max-buffers=2 drop=true sync=false"
    );
    gst::parse::launch(&desc)?
        .downcast::<gst::Pipeline>()
        .map_err(|_| anyhow::anyhow!("d3d11 capture: parsed element is not a Pipeline"))
}

/// Are the gstd3d11 convert/download elements present? (Cached.)
#[cfg(windows)]
pub(crate) fn d3d11_scale_available() -> bool {
    static OK: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OK.get_or_init(|| {
        ["d3d11convert", "d3d11download"]
            .iter()
            .all(|n| gst::ElementFactory::find(n).is_some())
    })
}

pub(crate) fn build_capture_pipeline(screencast: &ScreenCast, max_w: u32, max_h: u32, fps: u32) -> Result<gst::Pipeline> {
    // Frame-rate cap goes FIRST, before videoconvert, to kill the in-game frametime hitch.
    //
    // KDE's portal PipeWire stream delivers frames on-damage at the monitor's full rate
    // (<=165Hz under a fullscreen game) and advertises a VARIABLE framerate (caps carry
    // framerate=0/1). If videoconvert runs ahead of the rate cap it maps/reads-back the
    // portal DMABuf (GPU->CPU) on EVERY captured frame, and each readback forces a GPU sync
    // that stalls the game. So `videorate` must drop to {fps} BEFORE any element that touches
    // pixels.
    //
    // Two non-obvious constraints make this work where past attempts broke the stream:
    //   1. A bare `videorate max-rate={fps}` head (no fixed-framerate capsfilter) CANNOT
    //      derive a drop cadence from a framerate=0/1 source and emits ~1fps (the
    //      "smoke-signal" regression). videorate must be FOLLOWED by a capsfilter pinning a
    //      fixed `framerate={fps}/1`. drop-only=true never duplicates up to {fps}.
    //   2. That capsfilter MUST carry the `(ANY)` caps-feature. videorate is a passthrough
    //      GstBaseTransform that advertises video/x-raw(ANY) on both pads, so it forwards
    //      whatever memory the portal hands it (memory:DMABuf OR system memory) untouched and
    //      only re-times. A FEATURELESS `video/x-raw,framerate={fps}/1` capsfilter matches
    //      ONLY system memory: against a DMABuf source it either fails to link (videorate
    //      "can't handle caps") or shoves the GPU->CPU download up to videorate's INPUT side
    //      at full source rate — defeating the whole fix. `video/x-raw(ANY)` constrains ONLY
    //      framerate while preserving the memory feature, so the drop happens on GPU buffers
    //      and the single unavoidable readback (videoconvert, required by libwebrtc's CPU
    //      NativeVideoSource) runs only at {fps}.
    // The output capsfilter pins ONLY format at build (no width/height) + add-borders=false; a
    // one-shot probe below reads the REAL source size from videoscale's sink pad and pins
    // width/height = fit_even(realW, realH, box) so the published frame matches the source
    // aspect exactly (no stretch, no letterbox bars).
    // videoscale method=lanczos: the default bilinear (2-tap) skips source pixels past a 2x
    // downscale (5K -> the 1080p box is 2.67x), so HUD/chat text aliases and shimmers — which
    // the encoder then wastes bitrate on. Cost scales with OUTPUT pixels; when the source
    // already fits the box videoscale stays passthrough and method is inert.
    let src = screencast.src_fragment();
    // Windows: d3d11screencapturesrc hands out D3D11 GPU textures; videorate's (ANY)
    // passthrough re-times them untouched (same reasoning as the DMABuf case above) and
    // `d3d11download` then performs the one GPU->CPU readback AFTER the rate cap — the
    // exact Windows analogue of "the single unavoidable readback runs only at {fps}".
    let download = if cfg!(windows) { "d3d11download ! " } else { "" };
    let desc = cpu_capture_desc(&src, download, fps, cpu_capture_threads());
    let pipeline = gst::parse::launch(&desc)?
        .downcast::<gst::Pipeline>()
        .map_err(|_| anyhow::anyhow!("parsed element is not a Pipeline"))?;
    install_first_frame_repin(&pipeline, max_w, max_h, None, screencast.size_memo(), "CPU");
    Ok(pipeline)
}

/// Threads for the CPU path's full-resolution convert + lanczos scale. Both elements ran on ONE thread
/// (GStreamer's default n-threads=1): 16 ms per 3245x2067 frame on the rig, i.e. the stream's frame rate was capped by one
/// core while the rest idled. Half the cores, at most 4 (measured on the rig: 21 → 10 ms per frame with 4).
pub(crate) fn cpu_capture_threads() -> u32 {
    let n = std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(2);
    (n / 2).clamp(1, 4)
}

/// The launch line of the CPU capture (tests feed a videotestsrc head).
pub(crate) fn cpu_capture_desc(src: &str, download: &str, fps: u32, threads: u32) -> String {
    format!(
        "{src} ! \
         videorate drop-only=true ! \
         video/x-raw(ANY),framerate={fps}/1 ! \
         {download}videoconvert n-threads={threads} ! \
         videoscale method=lanczos add-borders=false n-threads={threads} name=vscale ! \
         capsfilter name=outcaps caps=video/x-raw,format=I420 ! \
         queue max-size-buffers=4 leaky=downstream ! \
         appsink name=sink emit-signals=false max-buffers=2 drop=true sync=false"
    )
}

/// Log gst bus errors/EOS for the capture pipeline (otherwise silent). Returns a stop
/// flag the OWNER MUST flip on teardown: every watched pipeline is torn down via
/// set_state(Null), which posts NO EOS, so without the flag the thread polls the
/// orphaned bus forever — one leaked OS thread per camera toggle / share stop / quality
/// restart (mirrors the VideoHandle.stop pattern in call.rs).
pub(crate) fn watch_pipeline_bus(pipeline: &gst::Pipeline) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
    watch_pipeline_bus_notify(pipeline, None)
}

/// `watch_pipeline_bus` + an optional one-shot `on_dead` callback fired when the
/// pipeline dies ON ITS OWN (bus Error/EOS — e.g. the shared window was closed).
/// Deliberate teardown never fires it: owners flip the stop flag BEFORE NULLing
/// (and NULL posts no EOS), and the callback is skipped once the flag is set.
pub(crate) fn watch_pipeline_bus_notify(
    pipeline: &gst::Pipeline,
    on_dead: Option<Box<dyn FnOnce() + Send>>,
) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let Some(bus) = pipeline.bus() else { return stop };
    let stop_w = stop.clone();
    let mut on_dead = on_dead;
    std::thread::spawn(move || {
        use gst::MessageView;
        loop {
            if stop_w.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }
            let Some(msg) = bus.timed_pop(gst::ClockTime::from_mseconds(500)) else {
                continue;
            };
            match msg.view() {
                MessageView::Error(e) => {
                    eprintln!("[lk] capture gst ERROR: {} ({:?})", e.error(), e.debug());
                    if !stop_w.load(std::sync::atomic::Ordering::Relaxed) {
                        if let Some(cb) = on_dead.take() {
                            cb();
                        }
                    }
                    break;
                }
                MessageView::Eos(_) => {
                    eprintln!("[lk] capture gst EOS");
                    if !stop_w.load(std::sync::atomic::Ordering::Relaxed) {
                        if let Some(cb) = on_dead.take() {
                            cb();
                        }
                    }
                    break;
                }
                _ => {}
            }
        }
    });
    stop
}

/// Pull each I420 sample and hand it to the LiveKit `NativeVideoSource`.
///
/// KEEPALIVE (the blank-screen-share fix): a Wayland/portal screen capture is DAMAGE-DRIVEN
/// — a screen whose content isn't changing produces NO frames at all (`pipewiresrc`'s own
/// `keepalive-time` resend does not survive `videorate`/libwebrtc's frame pacing). libwebrtc
/// needs a CONTINUOUS frame feed: with the source silent the encoder starves, so it emits
/// exactly ONE keyframe and then nothing, peers PLI-storm for a picture that never comes, and
/// the share renders as a permanently BLANK tile. (Reproduced verbatim on the dev laptop:
/// idle screen => `[lk-stats] 0x0@0 key=1` forever — the user's exact log.)
///
/// So we re-push the LAST captured frame on a timer whenever the capture goes quiet. A static
/// duplicate costs the encoder almost nothing (a few-byte P-frame) but keeps the stream alive,
/// keeps peers' decoders fed, and lets NVENC answer a PLI with a fresh keyframe. This mirrors
/// what the mic feeder already does for audio (concealment frames on underrun).
///
/// The repeater self-terminates: it holds only a Weak ref to the pipeline, so when the share
/// stops (pipeline NULLed + dropped) the upgrade fails and the thread exits — no handle
/// plumbing, no leak per share/camera toggle.
pub(crate) fn wire_appsink_to_source(appsink: &gstreamer_app::AppSink, source: NativeVideoSource) {
    wire_appsink_to_source_ext(appsink, source, "video", None)
}

/// 0.8.28: the labelled form. `label` names the feeder in the keepalive log line
/// (camera / share — "capture quiet" used to read the same for both, which misled triage), and
/// `on_quiet` is called ONCE per stall, after `QUIET_HOOK_MS` without a real frame while the
/// pipeline still reports PLAYING and at least one frame was seen before (a camera that stops
/// mid-call — a field user's webcam froze for 30+ s stretches, 27 times in one night; the repeater
/// dutifully kept the frozen picture alive for everyone). Re-armed when frames flow again.
pub(crate) fn wire_appsink_to_source_ext(
    appsink: &gstreamer_app::AppSink,
    source: NativeVideoSource,
    label: &'static str,
    on_quiet: Option<std::sync::Arc<dyn Fn(u64) + Send + Sync>>,
) {
    let frame_count = std::sync::atomic::AtomicU64::new(0);
    // Last frame (I420 planes + dims), shared with the repeater thread below.
    let last: std::sync::Arc<std::sync::Mutex<Option<(Vec<u8>, Vec<u8>, Vec<u8>, u32, u32)>>> =
        std::sync::Arc::new(std::sync::Mutex::new(None));
    // Millis (monotonic-ish) of the most recent push, so the repeater only fires when the
    // real capture has gone quiet — it never competes with a live, moving screen.
    let last_push = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let started = std::time::Instant::now();
    let now_ms = move || started.elapsed().as_millis() as u64;

    // ── Frame PACING (tinhat-capture-pts-v1) ────────────────────────────────────────────
    // Screen-share frames used to go out with `timestamp_us: 0`, which makes the SDK stamp
    // each one with SystemTime::now() at the moment the appsink callback runs — i.e. with
    // its ARRIVAL time, not the time it was captured.
    //
    // That is indistinguishable from correct while the GPU is idle, because frames arrive
    // as evenly as they were captured. Under a GPU-bound game it is not: the portal
    // DMABuf import, cudaconvertscale and download all queue behind the game's work, so
    // frames land in CLUMPS — several within a couple of milliseconds, then a long gap.
    // Stamping arrival times bakes that jitter into the encoder's timeline, so the pacer
    // and the receiver faithfully reproduce judder even though every individual frame is
    // encoded at full quality. Which is exactly the reported symptom: per-frame quality
    // fine, motion jittery, and only in GPU-heavy games.
    //
    // `videorate` upstream has already snapped PTS onto a clean fixed-fps grid, so the
    // capture timeline is the honest one. Re-base it onto the wall clock of the first
    // frame to stay in the epoch the SDK expects (micros since UNIX epoch), and keep it
    // strictly increasing — a non-monotonic capture timestamp is worse than jitter.
    //
    // TINHAT_CAPTURE_ARRIVAL_TS=1 restores the old arrival-time behaviour.
    let arrival_ts_mode = std::env::var_os("TINHAT_CAPTURE_ARRIVAL_TS").is_some();
    if arrival_ts_mode {
        eprintln!("[lk] TINHAT_CAPTURE_ARRIVAL_TS=1 — stamping frames on arrival (pre-0.8.1 behaviour)");
    }
    // (first capture PTS in us, wall clock in us at that moment)
    let pts_base: std::sync::Arc<std::sync::Mutex<Option<(i64, i64)>>> =
        std::sync::Arc::new(std::sync::Mutex::new(None));
    let last_ts_us = std::sync::Arc::new(std::sync::atomic::AtomicI64::new(0));
    // Rolling evidence for the next "it is still jittery" report: how far apart the
    // CAPTURE clock and the ARRIVAL clock drift. Big numbers here mean the capture chain
    // is being starved, and say so without needing a profiler on the user's machine.
    let jitter_sum_us = std::sync::Arc::new(std::sync::atomic::AtomicI64::new(0));
    let jitter_max_us = std::sync::Arc::new(std::sync::atomic::AtomicI64::new(0));
    let prev_arrival_us = std::sync::Arc::new(std::sync::atomic::AtomicI64::new(0));
    let prev_pts_us = std::sync::Arc::new(std::sync::atomic::AtomicI64::new(0));
    let (pts_base_cb, last_ts_cb) = (pts_base.clone(), last_ts_us.clone());
    let (jsum_cb, jmax_cb) = (jitter_sum_us.clone(), jitter_max_us.clone());
    let (parr_cb, ppts_cb) = (prev_arrival_us.clone(), prev_pts_us.clone());

    let last_cb = last.clone();
    // Throttle for the local share preview (see crate::call::local_preview_wanted).
    let preview_last = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let preview_cb = preview_last.clone();
    let push_cb = last_push.clone();
    let now_cb = now_ms;
    let source_rep = source.clone(); // for the repeater thread (the callback consumes `source`)
    appsink.set_callbacks(
        gstreamer_app::AppSinkCallbacks::builder()
            .new_sample(move |sink| {
                let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                let buffer = sample.buffer().ok_or(gst::FlowError::Error)?;
                let caps = sample.caps().ok_or(gst::FlowError::Error)?;
                let info =
                    gstreamer_video::VideoInfo::from_caps(caps).map_err(|_| gst::FlowError::Error)?;
                let src_frame = gstreamer_video::VideoFrameRef::from_buffer_ref_readable(buffer, &info)
                    .map_err(|_| gst::FlowError::Error)?;

                let w = info.width();
                let h = info.height();
                // Periodic counter (not just the first frame) so a live test reveals
                // whether frames KEEP flowing to the encoder vs stalling after one.
                let n = frame_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if n == 0 {
                    eprintln!("[lk] first capture frame {w}x{h} — feeding encoder");
                } else if n % 150 == 0 {
                    eprintln!("[lk] fed {n} frames to encoder ({w}x{h})");
                }
                let mut dst = I420Buffer::new(w, h);
                copy_i420(&src_frame, &info, &mut dst, w, h);

                // Stash a copy of this frame so the repeater can re-push it if the screen
                // goes static (cheap: only on real capture frames, not on repeats).
                {
                    let (sy, su, sv) = dst.strides();
                    let (dy, du, dv) = dst.data();
                    let mut g = last_cb.lock().unwrap();
                    *g = Some((
                        dy[..(sy as usize * h as usize).min(dy.len())].to_vec(),
                        du[..(su as usize * ((h as usize + 1) / 2)).min(du.len())].to_vec(),
                        dv[..(sv as usize * ((h as usize + 1) / 2)).min(dv.len())].to_vec(),
                        w,
                        h,
                    ));
                }
                push_cb.store(now_cb(), std::sync::atomic::Ordering::Relaxed);
                // The stream popover's live preview of what we're sending — a
                // ~5 fps RGBA copy to the PiP router, only while a viewer asked for it.
                if crate::call::local_preview_wanted() {
                    let t = now_cb();
                    if t.saturating_sub(preview_cb.load(std::sync::atomic::Ordering::Relaxed)) >= 200 {
                        preview_cb.store(t, std::sync::atomic::Ordering::Relaxed);
                        let (sy, su, sv) = dst.strides();
                        let (dy, du, dv) = dst.data();
                        let mut rgba = vec![0u8; (w as usize) * (h as usize) * 4];
                        livekit::webrtc::native::yuv_helper::i420_to_abgr(
                            dy, sy, du, su, dv, sv, &mut rgba, w * 4, w as i32, h as i32,
                        );
                        crate::call::route_local_frame(crate::call::LOCAL_SHARE_PREVIEW_ID, w, h, rgba);
                    }
                }

                // Capture-clock timestamp (see tinhat-capture-pts-v1 above). Falls back to
                // 0 (= SDK stamps on arrival) when the source gives us no PTS.
                let now_us = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_micros() as i64)
                    .unwrap_or(0);
                let pts_us = buffer.pts().map(|t| t.useconds() as i64);
                let stamp_us = if arrival_ts_mode {
                    0
                } else if let Some(pts) = pts_us {
                    let ts = rebase_capture_ts(pts, now_us, &pts_base_cb, &last_ts_cb);

                    // Measure how badly delivery is clumping vs the capture cadence.
                    let pa = parr_cb.swap(now_us, std::sync::atomic::Ordering::Relaxed);
                    let pp = ppts_cb.swap(pts, std::sync::atomic::Ordering::Relaxed);
                    if pa != 0 && pp != 0 {
                        let skew = ((now_us - pa) - (pts - pp)).abs();
                        jsum_cb.fetch_add(skew, std::sync::atomic::Ordering::Relaxed);
                        jmax_cb.fetch_max(skew, std::sync::atomic::Ordering::Relaxed);
                    }
                    ts
                } else {
                    0
                };
                if n > 0 && n % 150 == 0 {
                    let sum = jsum_cb.swap(0, std::sync::atomic::Ordering::Relaxed);
                    let max = jmax_cb.swap(0, std::sync::atomic::Ordering::Relaxed);
                    eprintln!(
                        "[lk/pacing] last 150 frames: mean delivery-vs-capture skew {:.1}ms, worst {:.1}ms",
                        sum as f64 / 150.0 / 1000.0,
                        max as f64 / 1000.0
                    );
                }

                let frame = VideoFrame {
                    rotation: VideoRotation::VideoRotation0,
                    timestamp_us: stamp_us,
                    frame_metadata: None,
                    buffer: dst,
                };
                source.capture_frame(&frame);
                Ok(gst::FlowSuccess::Ok)
            })
            .build(),
    );

    // ── The keepalive repeater ────────────────────────────────────────────────────────
    // Re-push the last frame whenever the capture has been quiet for QUIET_MS. ~10/s is
    // plenty to hold a stream open + satisfy PLIs, and is inaudible in CPU/bitrate terms
    // for a still image.
    const QUIET_MS: u64 = 200; // no real frame for this long => the screen is static
    const QUIET_HOOK_MS: u64 = 3000; // 0.8.28: this long with a frame seen before => a stalled capture
    const TICK: std::time::Duration = std::time::Duration::from_millis(100);
    let weak_sink = appsink.downgrade();
    std::thread::spawn(move || {
        let mut repeats: u64 = 0;
        let mut quiet_hook_fired = false;
        loop {
            std::thread::sleep(TICK);
            // Pipeline gone (share stopped / camera off) -> the appsink is dropped -> exit.
            let Some(sink) = weak_sink.upgrade() else { break };
            if sink.current_state() != gst::State::Playing {
                // Paused (e.g. camera muted): don't repeat, but keep waiting.
                continue;
            }
            let quiet = now_ms().saturating_sub(last_push.load(std::sync::atomic::Ordering::Relaxed));
            if quiet < QUIET_MS {
                quiet_hook_fired = false; // frames again — re-arm the stall hook
                continue; // the real screen is producing frames — stay out of the way
            }
            let snapshot = { last.lock().unwrap().clone() };
            let Some((py, pu, pv, w, h)) = snapshot else { continue };
            // 0.8.28: a stalled capture, not a static screen — tell the owner once.
            if !quiet_hook_fired && quiet >= QUIET_HOOK_MS {
                if let Some(cb) = on_quiet.as_ref() {
                    quiet_hook_fired = true;
                    eprintln!("[lk] keepalive({label}): capture quiet for {quiet} ms — asking the owner to restart it");
                    cb(quiet);
                }
            }
            let mut dst = I420Buffer::new(w, h);
            {
                let (sy, su, sv) = dst.strides();
                let (dy, du, dv) = dst.data_mut();
                let cp = |src: &[u8], dst: &mut [u8], stride: u32, rows: usize| {
                    let n = (stride as usize * rows).min(src.len()).min(dst.len());
                    dst[..n].copy_from_slice(&src[..n]);
                };
                cp(&py, dy, sy, h as usize);
                cp(&pu, du, su, (h as usize + 1) / 2);
                cp(&pv, dv, sv, (h as usize + 1) / 2);
            }
            let frame = VideoFrame {
                rotation: VideoRotation::VideoRotation0,
                timestamp_us: 0, // SDK stamps it with the capture clock (see native/video_source.rs)
                frame_metadata: None,
                buffer: dst,
            };
            source_rep.capture_frame(&frame);
            repeats += 1;
            // Neutral wording — this repeater is wired to BOTH the camera and the
            // screen publish ("screen static" on a frozen camera misled 2026-08-20
            // triage), and the first repeat logs too so a real capture freeze is
            // visible from its first 200ms instead of its 10th cumulative second.
            if repeats == 1 || repeats % 100 == 0 {
                eprintln!("[lk] keepalive({label}): capture quiet — re-pushed last frame {repeats}x (stream stays alive)");
            }
        }
    });
}

/// Copy a GStreamer I420 frame (with its strides) into a LiveKit I420Buffer
/// (with its own strides), plane by plane, row by row.
fn copy_i420(
    src: &gstreamer_video::VideoFrameRef<&gst::BufferRef>,
    info: &gstreamer_video::VideoInfo,
    dst: &mut I420Buffer,
    w: u32,
    h: u32,
) {
    // VideoFrameExt brings plane_stride()/info() into scope (they reflect the MAPPED
    // frame's real strides, honoring any GstVideoMeta padding — unlike the caps info).
    use gstreamer_video::prelude::VideoFrameExt;
    let (dst_sy, dst_su, dst_sv) = dst.strides();
    let (dy, du, dv) = dst.data_mut();
    // Use the FRAME's actual plane strides — v4l2/videoscale buffers are commonly
    // row-padded, and reading them at the unpadded caps stride shears the image
    // (green/magenta horizontal tearing). `info` is kept for the API/signature.
    let _ = info;
    let src_strides = src.plane_stride(); // &[i32], one per plane (real, padded)
    let cw = (w as usize + 1) / 2; // chroma width
    let ch = (h as usize + 1) / 2; // chroma height
    let planes: [(usize, usize, &mut [u8], usize, usize); 3] = [
        (0, w as usize, dy, dst_sy as usize, h as usize),
        (1, cw, du, dst_su as usize, ch),
        (2, cw, dv, dst_sv as usize, ch),
    ];
    for (plane, row_bytes, dst_plane, dst_stride, rows) in planes {
        let src_plane = src.plane_data(plane as u32).unwrap_or(&[]);
        let src_stride = *src_strides.get(plane).unwrap_or(&0) as usize;
        let copy = row_bytes.min(src_stride).min(dst_stride);
        for row in 0..rows {
            let s = row * src_stride;
            let d = row * dst_stride;
            if s + copy <= src_plane.len() && d + copy <= dst_plane.len() {
                dst_plane[d..d + copy].copy_from_slice(&src_plane[s..s + copy]);
            }
        }
    }
}

/// Re-base a capture PTS onto the wall clock of the first frame, keeping the result
/// strictly increasing. See `tinhat-capture-pts-v1` in `start_e2ee_screenshare`.
///
/// The point is that the SPACING comes from the capture clock (which `videorate` has
/// already snapped to a fixed-fps grid) while the EPOCH comes from the wall clock (which
/// is what WebRTC expects). Delivery jitter — frames landing in clumps because a game is
/// monopolising the GPU — then no longer reaches the encoder's timeline.
fn rebase_capture_ts(
    pts_us: i64,
    now_us: i64,
    base: &std::sync::Mutex<Option<(i64, i64)>>,
    last: &std::sync::atomic::AtomicI64,
) -> i64 {
    let mut guard = base.lock().unwrap();
    let (first_pts, first_wall) = *guard.get_or_insert((pts_us, now_us));
    let want = first_wall + (pts_us - first_pts);
    // Never go backwards: WebRTC discards a frame whose capture time regressed, so a
    // non-monotonic rebase would turn jitter into dropped frames.
    let prev = last.load(std::sync::atomic::Ordering::Relaxed);
    let ts = if want <= prev { prev + 1 } else { want };
    last.store(ts, std::sync::atomic::Ordering::Relaxed);
    ts
}

#[cfg(test)]
mod pacing_tests {
    use super::rebase_capture_ts;
    use std::sync::{atomic::AtomicI64, Mutex};

    /// The reported bug in miniature: a 30fps capture whose frames are DELIVERED in
    /// clumps (the GPU is busy) must still leave the encoder on an even 33.3ms cadence.
    /// Stamping arrival times — what `timestamp_us: 0` does — reproduces the clumping,
    /// which is the judder.
    #[test]
    fn clumped_delivery_still_yields_an_even_capture_cadence() {
        let base = Mutex::new(None);
        let last = AtomicI64::new(0);
        const STEP: i64 = 33_333; // 30fps
        let wall0 = 1_700_000_000_000_000i64;

        // Capture is even; delivery is not: three frames land almost together, then a
        // long stall, repeatedly. This is what a GPU-bound game does to the portal.
        let delivery_offsets = [0i64, 1_000, 2_000, 130_000, 131_000, 132_000, 260_000];
        let mut out = Vec::new();
        for (i, off) in delivery_offsets.iter().enumerate() {
            let pts = i as i64 * STEP;
            out.push(rebase_capture_ts(pts, wall0 + off, &base, &last));
        }

        for w in out.windows(2) {
            let d = w[1] - w[0];
            assert_eq!(d, STEP, "capture cadence must stay even, got {d}us between frames");
        }
        // And the arrival times it replaced were emphatically not even.
        let worst = delivery_offsets
            .windows(2)
            .map(|w| (w[1] - w[0] - STEP).abs())
            .max()
            .unwrap();
        assert!(worst > 90_000, "the test's own delivery pattern should be badly clumped");
    }

    /// A capture clock that steps backwards (source restart, portal renegotiation) must
    /// never emit a regressing timestamp — WebRTC drops those frames outright, turning a
    /// glitch into a freeze.
    #[test]
    fn timestamps_never_regress() {
        let base = Mutex::new(None);
        let last = AtomicI64::new(0);
        let wall0 = 1_700_000_000_000_000i64;
        let a = rebase_capture_ts(100_000, wall0, &base, &last);
        let b = rebase_capture_ts(133_333, wall0 + 33_000, &base, &last);
        let c = rebase_capture_ts(0, wall0 + 66_000, &base, &last); // PTS jumped back
        let d = rebase_capture_ts(33_333, wall0 + 99_000, &base, &last);
        assert!(b > a, "normal progression");
        assert!(c > b, "a backwards PTS must still move the stamp forward");
        assert!(d > c, "and recovery must keep moving forward");
    }
}

#[cfg(test)]
mod quiet_hook_tests {
    use super::*;

    /// 0.8.28: a camera feeder whose frames stop must fire the quiet hook (≥ 3 s
    /// after the last real frame) while the pipeline still reports PLAYING.
    #[test]
    fn camera_quiet_hook_fires_after_frames_stop() {
        let _ = gst::init();
        // NativeVideoSource::new spawns onto the LiveKit runtime — give this thread one.
        let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
        let _guard = rt.enter();
        let desc = "videotestsrc num-buffers=6 is-live=true ! \
                    video/x-raw,format=I420,width=64,height=48,framerate=30/1 ! \
                    appsink name=sink emit-signals=false max-buffers=2 drop=true sync=false";
        let pipeline = match gst::parse::launch(desc) {
            Ok(p) => p.downcast::<gst::Pipeline>().unwrap(),
            Err(e) => {
                eprintln!("skipping: {e}");
                return;
            }
        };
        let appsink = pipeline
            .by_name("sink")
            .unwrap()
            .downcast::<gstreamer_app::AppSink>()
            .unwrap();
        let source = NativeVideoSource::new(
            livekit::webrtc::video_source::VideoResolution { width: 64, height: 48 },
            false,
        );
        let fired = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let f2 = fired.clone();
        wire_appsink_to_source_ext(
            &appsink,
            source,
            "camera",
            Some(std::sync::Arc::new(move |q| {
                f2.store(q, std::sync::atomic::Ordering::Relaxed);
            })),
        );
        pipeline.set_state(gst::State::Playing).unwrap();
        let t0 = std::time::Instant::now();
        while fired.load(std::sync::atomic::Ordering::Relaxed) == 0
            && t0.elapsed() < std::time::Duration::from_secs(10)
        {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let q = fired.load(std::sync::atomic::Ordering::Relaxed);
        let _ = pipeline.set_state(gst::State::Null);
        assert!(q >= 3000, "quiet hook did not fire (quiet={q} ms)");
    }
}

/// The capture paths for a source whose size nobody told us (KDE's portal: a window share, often a monitor).
/// A videotestsrc at a field window size (3245x2067 BGRx, from a 10.6.1 log) stands in for the portal stream.
#[cfg(test)]
mod share_capture_tests {
    use super::*;

    const SRC: &str = "videotestsrc is-live=true pattern=smpte ! video/x-raw,format=BGRx,width=3245,height=2067,framerate=60/1";

    /// Run a capture launch line with the first-frame re-pin for a 1920x1080 box; the first sample's size + the
    /// size the re-pin remembered + how many samples arrived in `secs`. None when the pipeline cannot run here.
    fn run(desc: &str, feature: Option<&'static str>, secs: f64) -> Option<((u32, u32), Option<(u32, u32)>, u32)> {
        let _ = gst::init();
        let pipeline = gst::parse::launch(desc).ok()?.downcast::<gst::Pipeline>().ok()?;
        let memo: std::sync::Arc<std::sync::Mutex<Option<(u32, u32)>>> = Default::default();
        install_first_frame_repin(&pipeline, 1920, 1080, feature, memo.clone(), "test");
        let sink = pipeline.by_name("sink")?.downcast::<gstreamer_app::AppSink>().ok()?;
        pipeline.set_state(gst::State::Playing).ok()?;
        let mut first = None;
        let mut n = 0u32;
        let t0 = std::time::Instant::now();
        while t0.elapsed().as_secs_f64() < secs {
            let Some(sample) = sink.try_pull_sample(gst::ClockTime::from_mseconds(200)) else { continue };
            let info = gstreamer_video::VideoInfo::from_caps(sample.caps()?).ok()?;
            assert_eq!(info.format(), gstreamer_video::VideoFormat::I420);
            first.get_or_insert((info.width(), info.height()));
            n += 1;
        }
        let _ = pipeline.set_state(gst::State::Null);
        let got = *memo.lock().unwrap();
        Some((first?, got, n))
    }

    /// Auto's 0x0 quality box used to reach the capture builders raw: fit_even(real, 0, 0) = 16x16.
    #[test]
    fn auto_quality_has_a_real_box() {
        let preset = ShareQuality { max_w: 2560, max_h: 1440, ..ShareQuality::default() };
        assert_eq!(share_box(preset), (2560, 1440));
        let auto = ShareQuality { max_w: 0, max_h: 0, ..preset };
        let (w, h) = share_box(auto);
        assert!(w >= 960 && h >= 540, "Auto must fit into the current rung, got {w}x{h}");
        assert_ne!(fit_even(3245, 2067, w, h), (16, 16));
    }

    /// The CPU path: the FIRST frame already comes out at fit_even(real, box) — not the source size, not the box.
    #[test]
    fn cpu_capture_pins_the_first_frame_from_the_real_size() {
        let desc = cpu_capture_desc(SRC, "", 60, cpu_capture_threads());
        let Some((first, memo, n)) = run(&desc, None, 2.0) else {
            eprintln!("skipping: the CPU capture pipeline cannot run here");
            return;
        };
        assert_eq!(first, (1696, 1072), "first frame {first:?} (the field log: 3245x2067 -> 1696x1072)");
        assert_eq!(memo, Some((3245, 2067)));
        eprintln!("cpu capture ({} threads): {n} frames in 2 s", cpu_capture_threads());
        assert!(n > 0);
    }

    /// The GPU path for an unknown size: same first-frame pin, in CUDA memory. Skips without CUDA.
    #[test]
    fn cuda_capture_learns_the_size_from_the_first_frame() {
        let _ = gst::init();
        if !cuda_scale_available() {
            eprintln!("skipping: no CUDA elements");
            return;
        }
        let desc = cuda_learn_desc(SRC, 60);
        let Some((first, memo, n)) = run(&desc, Some("memory:CUDAMemory"), 2.0) else {
            eprintln!("skipping: the CUDA capture pipeline cannot run here (no GPU?)");
            return;
        };
        assert_eq!(first, (1696, 1072), "first frame {first:?} (the field log: 3245x2067 -> 1696x1072)");
        assert_eq!(memo, Some((3245, 2067)));
        eprintln!("cuda capture: {n} frames in 2 s");
        assert!(n > 0);
    }
}
